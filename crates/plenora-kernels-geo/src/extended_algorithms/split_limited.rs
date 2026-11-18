//! Split per incidenze razionali ordinate lungo la sorgente.
#![allow(clippy::float_cmp)]
use crate::noding::{sorted, CountedVec, Edge, Work, P, Q};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Coord, Geometry, LineString};
use std::cmp::Ordering;

/// Taglia la sorgente nei punti esatti di incontro con punti, linee e bordi
/// di poligoni.
///
/// Gli overlap tagliano ai loro estremi. Collezioni annidate
/// ammesse. Conserva verso, ordine e vertici della sorgente. Nessuna
/// tolleranza di proiezione o fusione: e' un ingresso additivo, distinto da
/// [`super::split_line`], che conserva integralmente il contratto 6.0.0.
///
/// Usa il noder comune e [`RelateLimits`], senza ammissione quadratica.
/// Coppie e nodi includono anche gli incontri fra primitive dello stesso
/// operando; passi, memoria contabile e controlli cooperativi includono
/// validazione e costruzione dell'output. Il flag precede la deadline.
/// La memoria ha gli stessi limiti contabili (non heap fisico) di `RelateLimits`.
///
/// XY planari in metri, stesso CRS per entrambi gli operandi. Ordinamento
/// e deduplicazione sono razionali esatti; solo i nuovi estremi vengono
/// arrotondati in f64, con errore euclideo verificato <= 1 cm. Un collasso
/// di estremi distinti e' rifiutato. Nessuna lunghezza cumulativa floating
/// point interviene nel taglio. Linea vuota: un unico pezzo vuoto.
///
/// # Errors
/// [`PredicateError`]: ingressi non finiti o invalidi, limiti di lavoro,
/// nodi o prenotazioni, allocazione fallita, cancellazione e deadline.
/// `CalcoloNonConcluso` per splitter Line/Rect/Triangle non supportati,
/// arrotondamento oltre 1 cm o collasso di estremi distinti.
pub fn split_line_with_limits(
    source: &LineString<f64>,
    splitter: &Geometry<f64>,
    limits: RelateLimits,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<LineString<f64>>, PredicateError> {
    split(source, splitter, &mut Work::new(limits, cancelled))
}

fn invalid(side: &'static str) -> PredicateError {
    PredicateError::InvalidGeometry {
        side,
        reason: "struttura geometrica non valida".to_owned(),
    }
}

fn edge(
    a: Coord<f64>,
    b: Coord<f64>,
    side: &'static str,
    work: &mut Work<'_>,
) -> Result<Edge, PredicateError> {
    work.step()?;
    if !a.x.is_finite() || !a.y.is_finite() || !b.x.is_finite() || !b.y.is_finite() {
        return Err(PredicateError::NonFiniteCoordinate { side });
    }
    work.nodes(2)?;
    Ok(Edge {
        a: P::of_in(a, work)?,
        b: P::of_in(b, work)?,
    })
}

fn line_edges(
    line: &LineString<f64>,
    side: &'static str,
    edges: &mut CountedVec<Edge>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut distinct = false;
    for p in &line.0 {
        work.step()?;
        if !p.x.is_finite() || !p.y.is_finite() {
            return Err(PredicateError::NonFiniteCoordinate { side });
        }
        distinct |= Some(p) != line.0.first();
    }
    if !line.0.is_empty() && !distinct {
        return Err(invalid(side));
    }
    for pair in line.0.windows(2) {
        let value = edge(pair[0], pair[1], side, work)?;
        work.push(edges, value)?;
    }
    Ok(())
}

fn cutters(
    geometry: &Geometry<f64>,
    edges: &mut CountedVec<Edge>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut pending = CountedVec::new();
    work.push(&mut pending, geometry)?;
    while let Some(g) = pending.pop() {
        work.step()?;
        match g {
            Geometry::Point(p) => {
                let e = edge(p.0, p.0, "right", work)?;
                work.push(edges, e)?;
            }
            Geometry::MultiPoint(ps) => {
                for p in &ps.0 {
                    let e = edge(p.0, p.0, "right", work)?;
                    work.push(edges, e)?;
                }
            }
            Geometry::LineString(l) => line_edges(l, "right", edges, work)?,
            Geometry::MultiLineString(ls) => {
                for l in &ls.0 {
                    line_edges(l, "right", edges, work)?;
                }
            }
            Geometry::GeometryCollection(gs) => {
                for g in gs.0.iter().rev() {
                    work.push(&mut pending, g)?;
                }
            }
            Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {
                // Riusa la validazione poligonale esatta, sullo stesso budget.
                crate::predicates::arrangement_indexed::relate(
                    &Geometry::GeometryCollection(geo::GeometryCollection::empty()),
                    g,
                    work,
                )?;
                match g {
                    Geometry::Polygon(p) => {
                        for l in std::iter::once(p.exterior()).chain(p.interiors()) {
                            line_edges(l, "right", edges, work)?;
                        }
                    }
                    Geometry::MultiPolygon(ps) => {
                        for p in &ps.0 {
                            for l in std::iter::once(p.exterior()).chain(p.interiors()) {
                                line_edges(l, "right", edges, work)?;
                            }
                        }
                    }
                    _ => return Err(PredicateError::CalcoloNonConcluso("tipo poligonale")),
                }
            }
            _ => {
                return Err(PredicateError::CalcoloNonConcluso(
                    "tipo di tagliatore non supportato",
                ))
            }
        }
    }
    Ok(())
}

fn compare(a: &P, b: &P) -> Result<Ordering, PredicateError> {
    let x = a.x.try_cmp(&b.x)?;
    if x.is_eq() {
        a.y.try_cmp(&b.y)
    } else {
        Ok(x)
    }
}

// Ricerca dei due f64 adiacenti: nessuna conversione BigInt approssimata.
// Al piu' 63 confronti razionali per coordinata; pareggio verso zero.
fn rounded(value: &Q, work: &mut Work<'_>) -> Result<f64, PredicateError> {
    let negative = value.sign().is_lt();
    let magnitude = if negative {
        work.integer(0)?.try_sub(value)?
    } else {
        value.try_clone()?
    };
    let mut low = 0_u64;
    let mut high = f64::MAX.to_bits();
    while low < high {
        work.step()?;
        let middle = low + (high - low).div_ceil(2);
        let candidate = P::of_in(
            Coord {
                x: f64::from_bits(middle),
                y: 0.0,
            },
            work,
        )?;
        if candidate.x.try_cmp(&magnitude)?.is_gt() {
            high = middle - 1;
        } else {
            low = middle;
        }
    }
    let lower = f64::from_bits(low);
    let exact_lower = P::of_in(Coord { x: lower, y: 0.0 }, work)?;
    let result = if exact_lower.x == magnitude {
        lower
    } else {
        if low == f64::MAX.to_bits() {
            return Err(PredicateError::CalcoloNonConcluso("conversione non finita"));
        }
        let upper = f64::from_bits(low + 1);
        let exact_upper = P::of_in(Coord { x: upper, y: 0.0 }, work)?;
        if magnitude
            .try_cmp(&exact_lower.x.try_midpoint(&exact_upper.x)?)?
            .is_gt()
        {
            upper
        } else {
            lower
        }
    };
    Ok(if negative { -result } else { result })
}

pub(super) fn coordinate(point: &P, work: &mut Work<'_>) -> Result<Coord<f64>, PredicateError> {
    let result = Coord {
        x: rounded(&point.x, work)?,
        y: rounded(&point.y, work)?,
    };
    let delta = P::of_in(result, work)?.try_sub(point)?;
    let distance_squared = delta
        .x
        .try_mul(&delta.x)?
        .try_add(&delta.y.try_mul(&delta.y)?)?;
    // Fonte: contratto geografico 1 cm; (1/100)^2 = 1/10000 esatto.
    let bound = work.integer(1)?.try_div(&work.integer(10_000)?)?;
    if distance_squared.try_cmp(&bound)?.is_gt() {
        return Err(PredicateError::CalcoloNonConcluso("precisione del nodo"));
    }
    Ok(result)
}

fn finish(
    piece: &mut CountedVec<Coord<f64>>,
    output: &mut CountedVec<LineString<f64>>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let coordinates = std::mem::take(piece).export(work)?;
    work.push(output, LineString(coordinates))
}

fn split(
    source: &LineString<f64>,
    splitter: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<Vec<LineString<f64>>, PredicateError> {
    work.step()?;
    let mut edges = CountedVec::new();
    line_edges(source, "left", &mut edges, work)?;
    let source_count = edges.len();
    cutters(splitter, &mut edges, work)?;
    let mut cuts = CountedVec::new();
    for _ in 0..source_count {
        work.push(&mut cuts, CountedVec::new())?;
    }
    let mut references = CountedVec::new();
    for e in &edges {
        work.push(&mut references, e)?;
    }
    crate::noding::visit(&references, work, |i, j, hits, work| {
        let source_index = if i < source_count && j >= source_count {
            i
        } else if j < source_count && i >= source_count {
            j
        } else {
            return Ok(());
        };
        for p in hits.into_iter().flatten() {
            work.push(&mut cuts[source_index], p)?;
        }
        Ok(())
    })?;
    let mut last_nondegenerate = 0;
    for (i, e) in edges[..source_count].iter().enumerate() {
        work.step()?;
        if e.a != e.b {
            last_nondegenerate = i;
        }
    }
    let mut piece_has_length = false;
    let mut output = CountedVec::new();
    let mut piece = CountedVec::new();
    if let Some(first) = source.0.first() {
        work.push(&mut piece, *first)?;
    }
    for (index, points) in cuts.iter_mut().enumerate() {
        let e = &edges[index];
        let reverse = compare(&e.a, &e.b)?.is_gt();
        sorted(
            points,
            |a, b| compare(a, b).map(|order| if reverse { order.reverse() } else { order }),
            work,
        )?;
        points.dedup_by(|a, b| Ok(a == b), work)?;
        let mut previous = &e.a;
        for p in points.iter() {
            work.step()?;
            if *p == e.a || *p == e.b {
                continue;
            }
            let c = coordinate(p, work)?;
            if Some(&c) == piece.last() && *previous != *p {
                return Err(PredicateError::CalcoloNonConcluso("collasso del nodo"));
            }
            work.push(&mut piece, c)?;
            finish(&mut piece, &mut output, work)?;
            work.push(&mut piece, c)?;
            previous = p;
        }
        let end = source.0[index + 1];
        if Some(&end) == piece.last() && *previous != e.b {
            return Err(PredicateError::CalcoloNonConcluso("collasso del nodo"));
        }
        work.push(&mut piece, end)?;
        piece_has_length |= e.a != e.b;
        let mut cut_end = false;
        for p in points.iter() {
            work.step()?;
            cut_end |= *p == e.b;
        }
        if index < last_nondegenerate && cut_end && piece_has_length {
            finish(&mut piece, &mut output, work)?;
            work.push(&mut piece, end)?;
            piece_has_length = false;
        }
    }
    finish(&mut piece, &mut output, work)?;
    output.export(work)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
