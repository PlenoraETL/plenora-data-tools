//! Contratto 10: aggancio e fusione indipendenti, solo opt-in.
use super::{
    compare, split, CountedVec, Edge, Geometry, LineString, PredicateError, RelateLimits, Work, P,
};
use crate::noding::{sorted, Q};
use std::sync::atomic::AtomicBool;

/// Distanze XY nelle unita' della sorgente, finite e non negative.
#[derive(Clone, Copy, Debug, Default)]
pub struct SplitLineTolerances {
    /// Distanza inclusiva punto-segmento per la proiezione dei soli punti.
    pub snap_distance: f64,
    /// Distanza curvilinea inclusiva dal precedente taglio conservato.
    pub merge_distance: f64,
}

/// Errori atomici senza valori dell'ingresso.
#[derive(Debug, thiserror::Error)]
pub enum SplitLineError {
    /// Una tolleranza non e' finita o e' negativa.
    #[error("tolleranza split line non valida")]
    InvalidTolerance,
    /// Validazione, risorse, interruzione o pubblicazione non certificata.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
}

/// Split planare con due tolleranze indipendenti (contratto 10).
///
/// Ogni punto viene proiettato su ogni segmento entro `snap_distance`,
/// estremi inclusi, senza slack. Linee e bordi tagliano solo per incidenza.
/// I tagli sono ordinati lungo la sorgente: resta il primo e si scartano
/// quelli a distanza curvilinea <= `merge_distance` dall'ultimo conservato.
/// Gli estremi globali non sono tagli e non assorbono tagli interni.
/// Una tolleranza di fusione >= lunghezza conserva quindi il primo taglio.
/// Linee chiuse seguono l'ordine della sequenza, senza fusione ciclica.
/// Zero fonde solo occorrenze alla medesima ascissa curvilinea.
///
/// Conserva vertici, verso e pubblicazione verificata GOP-015A dello split
/// esatto. Budget cumulativo `RelateLimits`, comprese proiezioni esaustive,
/// confronti metrici, aritmetica e verifica; deadline prima del flag.
/// La fusione usa intervalli razionali certificati delle radici, massimo
/// 4096 raffinamenti: un confronto non deciso viene rifiutato esplicitamente.
/// Nessuna modifica alle API precedenti, al catalogo o agli adapter.
///
/// # Errors
/// `InvalidTolerance`; errori `PredicateError` per ingressi invalidi,
/// risorse, cancellazione, deadline, `AccuracyUnproven` per confronti
/// non certificati e `InvalidRoundedTopology` per pubblicazione non valida.
pub fn split_line_with_tolerances_with_limits(
    source: &LineString<f64>,
    splitter: &Geometry<f64>,
    tolerances: SplitLineTolerances,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<LineString<f64>>, SplitLineError> {
    let mut work = Work::new(limits, cancelled);
    work.step()?;
    for value in [tolerances.snap_distance, tolerances.merge_distance] {
        if !value.is_finite() || value < 0.0 {
            return Err(SplitLineError::InvalidTolerance);
        }
    }
    Ok(split(source, splitter, Some(tolerances), &mut work)?)
}

fn rational(value: f64, work: &Work<'_>) -> Result<Q, PredicateError> {
    Ok(P::of_in(geo::Coord { x: value, y: 0.0 }, work)?.x)
}

// Interfaccia razionale condivisa su main: tutte le operazioni e i loro
// temporanei possiedono ricevute GOP-027. Nessun confronto floating point.
fn dot(a: &P, b: &P) -> Result<Q, PredicateError> {
    a.x.try_mul(&b.x)?.try_add(&a.y.try_mul(&b.y)?)
}

fn square_distance(a: &P, b: &P) -> Result<Q, PredicateError> {
    let d = a.try_sub(b)?;
    dot(&d, &d)
}

fn project(point: &P, edge: &Edge, work: &mut Work<'_>) -> Result<P, PredicateError> {
    work.pair()?;
    work.nodes(1)?;
    let delta = edge.b.try_sub(&edge.a)?;
    let norm = dot(&delta, &delta)?;
    if norm.sign().is_eq() {
        return edge.a.try_clone();
    }
    let numerator = dot(&point.try_sub(&edge.a)?, &delta)?;
    if !numerator.sign().is_gt() {
        return edge.a.try_clone();
    }
    if !numerator.try_cmp(&norm)?.is_lt() {
        return edge.b.try_clone();
    }
    let t = numerator.try_div(&norm)?;
    edge.a.try_add(&P {
        x: delta.x.try_mul(&t)?,
        y: delta.y.try_mul(&t)?,
    })
}

pub(super) fn prepare(
    edges: &[Edge],
    splitter: &Geometry<f64>,
    cuts: &mut [CountedVec<P>],
    options: SplitLineTolerances,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let distance = rational(options.snap_distance, work)?;
    let squared = distance.try_mul(&distance)?;
    let mut pending = CountedVec::new();
    work.push(&mut pending, splitter)?;
    while let Some(geometry) = pending.pop() {
        work.step()?;
        match geometry {
            Geometry::Point(point) => snap(point.0, edges, cuts, &squared, work)?,
            Geometry::MultiPoint(points) => {
                for point in &points.0 {
                    snap(point.0, edges, cuts, &squared, work)?;
                }
            }
            Geometry::GeometryCollection(collection) => {
                for child in collection.0.iter().rev() {
                    work.push(&mut pending, child)?;
                }
            }
            _ => {}
        }
    }
    normalize_vertices(edges, cuts, work)?;
    merge(edges, cuts, &rational(options.merge_distance, work)?, work)
}

fn normalize_vertices(
    edges: &[Edge],
    cuts: &mut [CountedVec<P>],
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut preceding = None;
    for (index, edge) in edges.iter().enumerate() {
        let mut at_start = false;
        for p in &cuts[index] {
            work.step()?;
            at_start |= *p == edge.a;
        }
        if let Some(previous) = preceding {
            if at_start {
                work.nodes(1)?;
                work.push(&mut cuts[previous], edge.a.try_clone()?)?;
            }
        }
        work.step()?;
        if edge.a != edge.b {
            preceding = Some(index);
        }
    }
    Ok(())
}

fn snap(
    coordinate: geo::Coord<f64>,
    edges: &[Edge],
    cuts: &mut [CountedVec<P>],
    squared: &Q,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    work.nodes(1)?;
    let point = P::of_in(coordinate, work)?;
    for (index, edge) in edges.iter().enumerate() {
        let p = project(&point, edge, work)?;
        if !square_distance(&p, &point)?.try_cmp(squared)?.is_gt() {
            work.push(&mut cuts[index], p)?;
        }
    }
    Ok(())
}

fn merge(
    edges: &[Edge],
    cuts: &mut [CountedVec<P>],
    tolerance: &Q,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut gaps = CountedVec::new();
    let mut have_cut = false;
    let mut last = 0;
    for (i, edge) in edges.iter().enumerate() {
        work.step()?;
        if edge.a != edge.b {
            last = i;
        }
    }
    for (index, points) in cuts.iter_mut().enumerate() {
        let edge = &edges[index];
        let reverse = compare(&edge.a, &edge.b)?.is_gt();
        sorted(
            points,
            |a, b| compare(a, b).map(|o| if reverse { o.reverse() } else { o }),
            work,
        )?;
        points.dedup_by(|a, b| Ok(a == b), work)?;
        let mut kept = CountedVec::new();
        let mut previous = &edge.a;
        for p in points.iter() {
            work.step()?;
            // Un vertice condiviso appartiene al segmento che vi termina.
            if *p == edge.a || (index >= last && *p == edge.b) {
                continue;
            }
            if have_cut {
                work.push(&mut gaps, square_distance(previous, p)?)?;
            }
            previous = p;
            if !have_cut || !within(&gaps, tolerance, work)? {
                work.nodes(1)?;
                work.push(&mut kept, p.try_clone()?)?;
                have_cut = true;
                gaps = CountedVec::new();
            }
        }
        if have_cut {
            work.push(&mut gaps, square_distance(previous, &edge.b)?)?;
        }
        *points = kept;
    }
    Ok(())
}

// Oracolo matematico: somma di lunghezze sqrt(dx^2+dy^2); intervalli
// inclusivi, bisezione razionale e radici razionali riconosciute esattamente.
fn within(squares: &[Q], tolerance: &Q, work: &mut Work<'_>) -> Result<bool, PredicateError> {
    let mut intervals = CountedVec::new();
    for square in squares {
        work.step()?;
        let interval = if let Some(root) = square.try_sqrt_exact()? {
            (root.try_clone()?, root)
        } else {
            (work.integer(0)?, square.try_add(&work.integer(1)?)?)
        };
        work.push(&mut intervals, interval)?;
    }
    for _ in 0..4096 {
        let mut low = work.integer(0)?;
        let mut high = work.integer(0)?;
        for (lo, hi) in &intervals {
            work.step()?;
            low = low.try_add(lo)?;
            high = high.try_add(hi)?;
        }
        if !high.try_cmp(tolerance)?.is_gt() {
            return Ok(true);
        }
        if low.try_cmp(tolerance)?.is_gt() {
            return Ok(false);
        }
        for ((lo, hi), square) in intervals.iter_mut().zip(squares) {
            work.step()?;
            if lo == hi {
                continue;
            }
            let mid = lo.try_midpoint(hi)?;
            if mid.try_mul(&mid)?.try_cmp(square)?.is_gt() {
                *hi = mid;
            } else {
                *lo = mid;
            }
        }
    }
    Err(PredicateError::AccuracyUnproven)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
