//! Split per incidenze razionali ordinate lungo la sorgente.
#![allow(clippy::float_cmp)]
use crate::noding::{sorted, CountedVec, Edge, Work, P};
use crate::predicates::arrangement_indexed::line_boolean::{
    verify_publication, LineBooleanError, Originals, PublishedEdge,
};
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
/// XY planari, stesso CRS e unita' per entrambi gli operandi. Ordinamento
/// e deduplicazione sono razionali esatti; solo i nuovi estremi vengono
/// pubblicati secondo GOP-015A: binary64 piu' vicino, meta' al pari,
/// errore <= mezza ulp per ordinata. La topologia viene verificata dopo
/// l'arrotondamento, compresi gli autoincroci della sorgente. Nessuna lunghezza cumulativa floating
/// point interviene nel taglio. Linea vuota: un unico pezzo vuoto.
///
/// # Errors
/// [`PredicateError`]: ingressi non finiti o invalidi, limiti di lavoro,
/// nodi o prenotazioni, allocazione fallita, cancellazione e deadline.
/// `CalcoloNonConcluso` per splitter Line/Rect/Triangle non supportati;
/// `InvalidRoundedTopology` se la pubblicazione altera la topologia.
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
            _ => return Err(PredicateError::UnsupportedGeometry),
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

pub(super) fn publication_error(error: LineBooleanError) -> PredicateError {
    match error {
        LineBooleanError::Predicate(error) => error,
        LineBooleanError::InvalidRoundedTopology => PredicateError::InvalidRoundedTopology,
        LineBooleanError::CoordinateOutOfRange => {
            PredicateError::CalcoloNonConcluso("coordinata costruita non finita")
        }
        LineBooleanError::UnsupportedOperands | LineBooleanError::InvalidOutputDimension => {
            PredicateError::CalcoloNonConcluso("certificato di pubblicazione non valido")
        }
    }
}

fn finish(
    piece: &mut CountedVec<Coord<f64>>,
    output: &mut CountedVec<LineString<f64>>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let coordinates = std::mem::take(piece).export(work)?;
    work.push(output, LineString(coordinates))
}

fn record_path<'a>(
    topology: &'a [P],
    cursor: &mut usize,
    end: &P,
    coords: [Coord<f64>; 2],
    paths: &mut CountedVec<CountedVec<&'a P>>,
    converted: &mut CountedVec<Edge>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut path = CountedVec::new();
    loop {
        work.step()?;
        let p = topology
            .get(*cursor)
            .ok_or(PredicateError::InvalidRoundedTopology)?;
        work.push(&mut path, p)?;
        if p == end {
            break;
        }
        *cursor += 1;
    }
    work.push(paths, path)?;
    work.nodes(2)?;
    let edge = Edge {
        a: P::of_in(coords[0], work)?,
        b: P::of_in(coords[1], work)?,
    };
    work.push(converted, edge)
}

struct SplitNodes {
    cuts: CountedVec<CountedVec<P>>,
    topology: CountedVec<CountedVec<P>>,
}

fn split_nodes(
    edges: &[Edge],
    source_count: usize,
    work: &mut Work<'_>,
) -> Result<SplitNodes, PredicateError> {
    let mut topology = CountedVec::new();
    let mut cuts = CountedVec::new();
    for e in &edges[..source_count] {
        let mut path = CountedVec::new();
        work.nodes(2)?;
        work.push(&mut path, e.a.try_clone()?)?;
        work.push(&mut path, e.b.try_clone()?)?;
        work.push(&mut topology, path)?;
        work.push(&mut cuts, CountedVec::new())?;
    }
    let mut references = CountedVec::new();
    for e in edges {
        work.push(&mut references, e)?;
    }
    crate::noding::visit(&references, work, |i, j, hits, work| {
        for index in [i, j] {
            if index < source_count {
                for p in hits.iter().flatten() {
                    work.nodes(1)?;
                    work.push(&mut topology[index], p.try_clone()?)?;
                }
            }
        }
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
    for (e, path) in edges[..source_count].iter().zip(topology.iter_mut()) {
        let reverse = compare(&e.a, &e.b)?.is_gt();
        sorted(
            path,
            |a, b| compare(a, b).map(|o| if reverse { o.reverse() } else { o }),
            work,
        )?;
        path.dedup_by(|a, b| Ok(a == b), work)?;
    }
    Ok(SplitNodes { cuts, topology })
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
    let originals = Originals::for_editing(source.0.iter().copied(), splitter, work)?;
    let SplitNodes { mut cuts, topology } = split_nodes(&edges, source_count, work)?;
    let mut last_nondegenerate = 0;
    for (i, e) in edges[..source_count].iter().enumerate() {
        work.step()?;
        if e.a != e.b {
            last_nondegenerate = i;
        }
    }
    let mut paths = CountedVec::new();
    let mut converted = CountedVec::new();
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
        let mut previous_coord = source.0[index];
        let mut cursor = 0;
        for p in points.iter() {
            work.step()?;
            if *p == e.a || *p == e.b {
                continue;
            }
            let c = originals.coordinate(p, work).map_err(publication_error)?;
            if Some(&c) == piece.last() && *previous != *p {
                return Err(PredicateError::InvalidRoundedTopology);
            }
            record_path(
                &topology[index],
                &mut cursor,
                p,
                [previous_coord, c],
                &mut paths,
                &mut converted,
                work,
            )?;
            previous_coord = c;
            work.push(&mut piece, c)?;
            finish(&mut piece, &mut output, work)?;
            work.push(&mut piece, c)?;
            previous = p;
        }
        let end = source.0[index + 1];
        if Some(&end) == piece.last() && *previous != e.b {
            return Err(PredicateError::InvalidRoundedTopology);
        }
        if *previous != e.b {
            record_path(
                &topology[index],
                &mut cursor,
                &e.b,
                [previous_coord, end],
                &mut paths,
                &mut converted,
                work,
            )?;
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
    certify_paths(&paths, &converted, &originals, work)?;
    output.export(work)
}

fn certify_paths(
    paths: &[CountedVec<&P>],
    converted: &[Edge],
    originals: &Originals,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut refs = CountedVec::new();
    for edge in converted {
        work.push(&mut refs, edge)?;
    }
    let rounded_paths = crate::noding::node(&refs, work)?;
    let mut published = CountedVec::new();
    for (exact, rounded) in paths.iter().zip(&rounded_paths) {
        work.step()?;
        if exact.len() < 2 || exact.len() != rounded.len() {
            return Err(PredicateError::InvalidRoundedTopology);
        }
        for i in 0..exact.len() - 1 {
            let certificate = PublishedEdge::noded(
                [exact[i], exact[i + 1]],
                [&rounded[i], &rounded[i + 1]],
                work,
            )?;
            work.push(&mut published, certificate)?;
        }
    }
    verify_publication(&published, originals, false, None, work).map_err(publication_error)?;
    Ok(())
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
