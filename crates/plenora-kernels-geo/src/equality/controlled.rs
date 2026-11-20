//! Contesto unico e primitive fallibili dell'uguaglianza di rappresentazione.
use super::{cmp_coord, coord_eq};
use geo::{Coord, Geometry, LineString, Polygon};
use std::cmp::Ordering;
use std::convert::Infallible;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// Limiti opt-in cumulativi del confronto e della normalizzazione.
/// Non impongono un budget di memoria; i temporanei degli anelli sono O(n).
#[derive(Clone, Copy, Debug)]
pub struct EqualityLimits {
    /// Visite di geometrie, copie/confronti di coordinate e passi di ordinamento.
    /// Il checkpoint di ingresso e quello di pubblicazione non consumano lavoro.
    pub max_work: u64,
    /// Scadenza monotona, osservata prima del token e del budget.
    pub deadline: Option<Instant>,
}

/// Esiti senza dati dell'ingresso, mai risultati parziali.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EqualityError {
    /// Budget cumulativo esaurito.
    #[error("limite di lavoro del confronto geometrico superato")]
    WorkLimit,
    /// Token di annullamento attivo.
    #[error("confronto geometrico annullato")]
    Cancelled,
    /// Scadenza monotona raggiunta.
    #[error("scadenza del confronto geometrico raggiunta")]
    DeadlineExceeded,
}

pub(super) trait Control {
    type Error;
    fn step(&mut self) -> Result<(), Self::Error>;
}
pub(super) struct Unlimited;
impl Control for Unlimited {
    type Error = Infallible;
    fn step(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}
pub(super) fn infallible<T>(value: Result<T, Infallible>) -> T {
    match value {
        Ok(value) => value,
        Err(never) => match never {},
    }
}

pub(super) struct EqualityWork<'a> {
    limits: EqualityLimits,
    cancelled: Option<&'a AtomicBool>,
    pub(super) used: u64,
}
impl<'a> EqualityWork<'a> {
    pub(super) const fn new(limits: EqualityLimits, cancelled: Option<&'a AtomicBool>) -> Self {
        Self {
            limits,
            cancelled,
            used: 0,
        }
    }
    pub(super) fn checkpoint(&self) -> Result<(), EqualityError> {
        crate::noding::check_interruption(
            self.limits.deadline,
            self.cancelled,
            EqualityError::DeadlineExceeded,
            EqualityError::Cancelled,
        )
    }
}
impl Control for EqualityWork<'_> {
    type Error = EqualityError;
    fn step(&mut self) -> Result<(), EqualityError> {
        self.checkpoint()?;
        self.used = self
            .used
            .checked_add(1)
            .filter(|n| *n <= self.limits.max_work)
            .ok_or(EqualityError::WorkLimit)?;
        Ok(())
    }
}

pub(super) fn compare_coords<W: Control>(
    a: &[Coord],
    b: &[Coord],
    work: &mut W,
) -> Result<Ordering, W::Error> {
    for (a, b) in a.iter().zip(b) {
        work.step()?;
        let order = cmp_coord(a, b);
        if !order.is_eq() {
            return Ok(order);
        }
    }
    work.step()?;
    Ok(a.len().cmp(&b.len()))
}
pub(super) fn compare_polygon<W: Control>(
    a: &Polygon,
    b: &Polygon,
    work: &mut W,
) -> Result<Ordering, W::Error> {
    let order = compare_coords(&a.exterior().0, &b.exterior().0, work)?;
    if !order.is_eq() {
        return Ok(order);
    }
    for (a, b) in a.interiors().iter().zip(b.interiors()) {
        let order = compare_coords(&a.0, &b.0, work)?;
        if !order.is_eq() {
            return Ok(order);
        }
    }
    Ok(a.interiors().len().cmp(&b.interiors().len()))
}

/// Mergesort stabile degli indici, poi permutazione in posto dei valori.
/// La stabilita' preserva i bit anche quando le chiavi equiparano +/-0.
/// Ogni scansione e confronto e' interrompibile senza simulare un ordine.
pub(super) fn stable_sort<T, W: Control>(
    values: &mut [T],
    compare: impl Fn(&T, &T, &mut W) -> Result<Ordering, W::Error>,
    work: &mut W,
) -> Result<(), W::Error> {
    let n = values.len();
    let mut order = Vec::with_capacity(n);
    let mut scratch = Vec::with_capacity(n);
    for i in 0..n {
        work.step()?;
        order.push(i);
        scratch.push(i);
    }
    let mut width = 1;
    while width < n {
        for start in (0..n).step_by(width.saturating_mul(2)) {
            let middle = start.saturating_add(width).min(n);
            let end = middle.saturating_add(width).min(n);
            let (mut left, mut right) = (start, middle);
            for slot in &mut scratch[start..end] {
                work.step()?;
                if left < middle
                    && (right == end
                        || !compare(&values[order[left]], &values[order[right]], work)?.is_gt())
                {
                    *slot = order[left];
                    left += 1;
                } else {
                    *slot = order[right];
                    right += 1;
                }
            }
        }
        std::mem::swap(&mut order, &mut scratch);
        width = width.saturating_mul(2);
    }
    for (destination, source) in order.into_iter().enumerate() {
        work.step()?;
        scratch[source] = destination;
    }
    for i in 0..n {
        while scratch[i] != i {
            work.step()?;
            let j = scratch[i];
            values.swap(i, j);
            scratch.swap(i, j);
        }
    }
    Ok(())
}

fn sequence_eq<T, W: Control>(
    a: &[T],
    b: &[T],
    work: &mut W,
    compare: impl Fn(&T, &T, &mut W) -> Result<bool, W::Error>,
) -> Result<bool, W::Error> {
    work.step()?;
    if a.len() != b.len() {
        return Ok(false);
    }
    for (a, b) in a.iter().zip(b) {
        work.step()?;
        if !compare(a, b, work)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn line_eq<W: Control>(
    a: &LineString,
    b: &LineString,
    tolerance: f64,
    work: &mut W,
) -> Result<bool, W::Error> {
    sequence_eq(&a.0, &b.0, work, |a, b, _| Ok(coord_eq(a, b, tolerance)))
}
fn polygon_eq<W: Control>(
    a: &Polygon,
    b: &Polygon,
    tolerance: f64,
    work: &mut W,
) -> Result<bool, W::Error> {
    Ok(line_eq(a.exterior(), b.exterior(), tolerance, work)?
        && sequence_eq(a.interiors(), b.interiors(), work, |a, b, w| {
            line_eq(a, b, tolerance, w)
        })?)
}
pub(super) fn equal<W: Control>(
    a: &Geometry,
    b: &Geometry,
    tolerance: f64,
    work: &mut W,
) -> Result<bool, W::Error> {
    work.step()?;
    Ok(match (a, b) {
        (Geometry::LineString(a), Geometry::LineString(b)) => line_eq(a, b, tolerance, work)?,
        (Geometry::Polygon(a), Geometry::Polygon(b)) => polygon_eq(a, b, tolerance, work)?,
        (Geometry::MultiPoint(a), Geometry::MultiPoint(b)) => {
            sequence_eq(&a.0, &b.0, work, |a, b, _| {
                Ok(coord_eq(&a.0, &b.0, tolerance))
            })?
        }
        (Geometry::MultiLineString(a), Geometry::MultiLineString(b)) => {
            sequence_eq(&a.0, &b.0, work, |a, b, w| line_eq(a, b, tolerance, w))?
        }
        (Geometry::MultiPolygon(a), Geometry::MultiPolygon(b)) => {
            sequence_eq(&a.0, &b.0, work, |a, b, w| polygon_eq(a, b, tolerance, w))?
        }
        (Geometry::GeometryCollection(a), Geometry::GeometryCollection(b)) => {
            sequence_eq(&a.0, &b.0, work, |a, b, w| equal(a, b, tolerance, w))?
        }
        // Le altre varianti contengono al massimo tre coordinate.
        _ => super::geometry_eq(a, b, tolerance),
    })
}
