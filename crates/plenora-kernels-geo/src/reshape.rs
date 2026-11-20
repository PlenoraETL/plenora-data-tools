//! Reshape planare XY GOP-019, costruzione esatta e pubblicazione GOP-015A.
//!
//! CRS, assi e unita' sono risolti dal chiamante. Nessuna riproiezione,
//! eliminazione di Z/M/curve, riparazione o tolleranza implicita.
//! Budget cumulativo ed esclusioni di `RelateLimits`, crescita prenotata.
//! Ogni ciclo ha checkpoint; deadline precede cancellazione. Al massimo 256
//! primitive razionali fra checkpoint, con interi limitati dal noder condiviso.
//! Allocazioni e drop esclusi dal conteggio; nessuna garanzia wall-clock.

use crate::noding::{CountedVec, Work, P, Q};
use crate::predicates::{PredicateError, RelateLimits};
use crate::trim_extend::{append, edges, publish_paths, read_path, simple, TrimExtendError};
use geo::{Geometry, LineString};
use std::cmp::Ordering;
use std::sync::atomic::AtomicBool;

/// Errori bounded senza dati dell'ingresso.
#[derive(Debug, thiserror::Error)]
pub enum ReshapeError {
    /// Struttura insufficiente o coordinate non finite.
    #[error("struttura della polilinea non valida")]
    InvalidGeometry,
    /// Dominio escluso, contatto ambiguo o ricostruzione non semplice.
    #[error("costruzione reshape non supportata")]
    Unsupported,
    /// La pubblicazione binary64 altera la topologia esatta.
    #[error("topologia non conservata dalla pubblicazione binary64")]
    InvalidRoundedTopology,
    /// Coordinata costruita non rappresentabile come binary64 finito.
    #[error("coordinata costruita fuori dal dominio binary64 finito")]
    CoordinateOutOfRange,
    /// Contabilita', aritmetica, cancellazione e deadline comuni.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
}

impl From<TrimExtendError> for ReshapeError {
    fn from(error: TrimExtendError) -> Self {
        match error {
            TrimExtendError::InvalidGeometry => Self::InvalidGeometry,
            TrimExtendError::Unsupported => Self::Unsupported,
            TrimExtendError::InvalidRoundedTopology => Self::InvalidRoundedTopology,
            TrimExtendError::CoordinateOutOfRange => Self::CoordinateOutOfRange,
            TrimExtendError::Predicate(error) => Self::Predicate(error),
        }
    }
}

/// Sostituisce il sottopercorso fra due incroci trasversali interni distinti.
///
/// Target singolo aperto semplice, reshaper singolo semplice. Il tratto del
/// reshaper segue la direzione del target, anche se richiede l'ordine inverso.
/// Per un reshaper chiuso si usa l'intervallo fra gli incroci nell'ordine
/// memorizzato, senza attraversare la giunzione iniziale/finale.
/// Certificato GOP-015A: nodi razionali esatti, nearest-even per ordinata con
/// errore <= mezza ulp, originali invariati e verifica dopo arrotondamento.
/// Modello e unita' planari dell'ingresso; nessuna trasformazione o
/// discretizzazione, incertezza del datum esclusa. Nessuna serializzazione.
///
/// # Errors
/// Geometria malformata, esclusioni GOP-019, topologia alterata dal rounding,
/// risorse esaurite, cancellazione o deadline. Nessun risultato parziale.
pub fn reshape_with_limits(
    target: &Geometry<f64>,
    reshaper: &Geometry<f64>,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<LineString<f64>, ReshapeError> {
    execute(target, reshaper, &mut Work::new(limits, cancelled))
}

struct PathSite {
    edge: usize,
    fraction: Q,
    before: usize,
    after: usize,
}

impl PathSite {
    fn compare(&self, other: &Self) -> Result<Ordering, PredicateError> {
        match self.edge.cmp(&other.edge) {
            Ordering::Equal => self.fraction.try_cmp(&other.fraction),
            order => Ok(order),
        }
    }
}

fn locate(
    points: &[P],
    edge: usize,
    point: &P,
    work: &mut Work<'_>,
) -> Result<PathSite, PredicateError> {
    work.step()?;
    let vertex = if point == &points[edge] {
        Some(edge)
    } else if point == &points[edge + 1] {
        Some(edge + 1)
    } else {
        None
    };
    if let Some(index) = vertex {
        return Ok(PathSite {
            edge: index,
            fraction: work.integer(0)?,
            before: index.saturating_sub(1),
            after: index + 1,
        });
    }
    let delta = points[edge + 1].try_sub(&points[edge])?;
    let offset = point.try_sub(&points[edge])?;
    let fraction = if delta.x.sign().is_eq() {
        offset.y.try_div(&delta.y)?
    } else {
        offset.x.try_div(&delta.x)?
    };
    Ok(PathSite {
        edge,
        fraction,
        before: edge,
        after: edge + 1,
    })
}

struct Crossing {
    point: P,
    target: PathSite,
    reshaper: PathSite,
}

fn upper(ray: &P) -> bool {
    ray.y.sign().is_gt() || (ray.y.sign().is_eq() && ray.x.sign().is_gt())
}

fn transverse(
    hit: &Crossing,
    target: &[P],
    reshaper: &[P],
    work: &mut Work<'_>,
) -> Result<bool, PredicateError> {
    work.step()?;
    if hit.target.after >= target.len()
        || hit.reshaper.after >= reshaper.len()
        || (hit.target.edge == 0 && hit.target.fraction.sign().is_eq())
        || (hit.reshaper.edge == 0 && hit.reshaper.fraction.sign().is_eq())
    {
        return Ok(false);
    }
    let mut rays = CountedVec::new();
    for (owner, point) in [
        (0, &target[hit.target.before]),
        (0, &target[hit.target.after]),
        (1, &reshaper[hit.reshaper.before]),
        (1, &reshaper[hit.reshaper.after]),
    ] {
        work.nodes(1)?;
        work.push(&mut rays, (owner, point.try_sub(&hit.point)?))?;
    }
    crate::noding::sorted(
        &mut rays,
        |a, b| {
            Ok(upper(&b.1)
                .cmp(&upper(&a.1))
                .then(a.1.try_cross(&b.1)?.sign().reverse()))
        },
        work,
    )?;
    for i in 0..4 {
        work.step()?;
        let a = &rays[i];
        let b = &rays[(i + 1) % 4];
        if a.0 == b.0 || (upper(&a.1) == upper(&b.1) && a.1.try_cross(&b.1)?.sign().is_eq()) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn crossings(
    target: &[P],
    reshaper: &[P],
    work: &mut Work<'_>,
) -> Result<CountedVec<Crossing>, ReshapeError> {
    let left = edges(target, work)?;
    let right = edges(reshaper, work)?;
    let mut refs = CountedVec::new();
    for edge in left.iter().chain(right.iter()) {
        work.push(&mut refs, edge)?;
    }
    let mut hits: CountedVec<Crossing> = CountedVec::new();
    let mut excluded = false;
    let scan = crate::noding::visit(&refs, work, |i, j, pair, work| {
        if (i < left.len()) == (j < left.len()) {
            return Ok(());
        }
        let (i, j) = if i < left.len() {
            (i, j - left.len())
        } else {
            (j, i - left.len())
        };
        if pair[1].is_some() {
            excluded = true;
        }
        if let [Some(point), _] = pair {
            for hit in &hits {
                work.step()?;
                if hit.point == point {
                    return Ok(());
                }
            }
            if hits.len() == 2 {
                excluded = true;
                return Ok(());
            }
            let hit = Crossing {
                target: locate(target, i, &point, work)?,
                reshaper: locate(reshaper, j, &point, work)?,
                point,
            };
            work.push(&mut hits, hit)?;
        }
        Ok(())
    });
    if excluded {
        return Err(ReshapeError::Unsupported);
    }
    scan?;
    if hits.len() != 2 {
        return Err(ReshapeError::Unsupported);
    }
    for hit in &hits {
        if !transverse(hit, target, reshaper, work)? {
            return Err(ReshapeError::Unsupported);
        }
    }
    work.step()?;
    if hits[0].target.compare(&hits[1].target)?.is_gt() {
        hits.swap(0, 1);
    }
    Ok(hits)
}

fn execute(
    target: &Geometry<f64>,
    reshaper: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<LineString<f64>, ReshapeError> {
    work.step()?;
    let target_points = read_path(target, false, work)?;
    let reshaper_points = read_path(reshaper, false, work)?;
    let hits = crossings(&target_points, &reshaper_points, work)?;
    let start = &hits[0];
    let end = &hits[1];
    let mut output = CountedVec::new();
    for point in &target_points[..=start.target.before] {
        append(&mut output, point, work)?;
    }
    append(&mut output, &start.point, work)?;
    work.step()?;
    if start.reshaper.compare(&end.reshaper)?.is_lt() {
        for i in start.reshaper.after..=end.reshaper.before {
            append(&mut output, &reshaper_points[i], work)?;
        }
    } else {
        for i in (end.reshaper.after..=start.reshaper.before).rev() {
            append(&mut output, &reshaper_points[i], work)?;
        }
    }
    append(&mut output, &end.point, work)?;
    for point in &target_points[end.target.after..] {
        append(&mut output, point, work)?;
    }
    simple(&output, work)?;
    Ok(publish_paths(&output, target, reshaper, work)?)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;

/// Ingressi con CRS verificato e consenso planare esplicito.
pub mod with_crs {
    use super::{AtomicBool, Geometry, LineString, RelateLimits, ReshapeError};

    crate::crs::checked_kernel!(reshape_with_limits, "geo.split", 2,
        (target: &Geometry<f64>, reshaper: &Geometry<f64>, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (LineString<f64>, ReshapeError),
        |context| {  super::reshape_with_limits(target, reshaper, limits, cancelled) });
}
