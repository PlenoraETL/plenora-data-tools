//! Facce razionali del grafo planare del linework.
#![allow(clippy::float_cmp)]
use super::split_limited::publication_error;
use crate::noding::faces::FaceRing;
use crate::noding::{CountedVec, Edge, Work, P};
use crate::predicates::arrangement_indexed::line_boolean::{
    verify_publication, Originals, PublishedEdge,
};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Geometry, LineString, Polygon};

/// Costruisce tutte le facce limitate del linework, con interni disgiunti.
///
/// Accetta `LineString`, `MultiLineString` e collezioni annidate di questi tipi.
/// Linee vuote ammesse; linee non vuote con meno di due vertici rifiutate.
/// Segmenti di lunghezza zero ignorati. Il noder comune divide incroci e
/// sovrapposizioni esattamente; lati coincidenti sono presenti una sola volta.
/// Dangles e ponti non delimitano facce e sono scartati. Anelli annidati
/// producono una faccia interna e una faccia esterna con buco, ricorsivamente.
/// Ordine deterministico per lo stesso input; nessuna ammissione n*m.
///
/// Usa coppie effettive, incidenze di nodi, passi e memoria contabile di
/// [`RelateLimits`], inclusi grafo, facce e output. Valgono le limitazioni
/// contabili di quel tipo (non e' una certificazione dell'heap fisico).
/// Deadline scaduta con precedenza sulla cancellazione.
/// XY planari nelle unita' del CRS: topologia razionale esatta, pubblicazione
/// GOP-015A al binary64 piu' vicino, meta' al pari, <= mezza ulp per ordinata.
/// Vertici originali invariati; per siti con zeri di segno diverso prevale
/// il primo in ingresso. Collassi, nuovi incroci/overlap e
/// inversioni di orientamento dopo la conversione sono rifiutati. Anche
/// la verifica del grafo convertito consuma coppie, nodi, passi e memoria.
/// La polygonize pubblicata in 6.0.0 conserva il proprio contratto.
///
/// # Errors
/// [`PredicateError`] per coordinate non finite, struttura o tipi non
/// supportati, limiti, allocazione, cancellazione, deadline;
/// `InvalidRoundedTopology` se la pubblicazione altera la topologia.
pub fn polygonize_with_limits(
    linework: &Geometry<f64>,
    limits: RelateLimits,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<Polygon<f64>>, PredicateError> {
    polygonize(linework, &mut Work::new(limits, cancelled))
}

fn collect(
    linework: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<CountedVec<Edge>, PredicateError> {
    let mut pending = CountedVec::new();
    let mut edges = CountedVec::new();
    work.push(&mut pending, linework)?;
    while let Some(g) = pending.pop() {
        work.step()?;
        match g {
            Geometry::LineString(line) => add_line(line, &mut edges, work)?,
            Geometry::MultiLineString(lines) => {
                for line in &lines.0 {
                    add_line(line, &mut edges, work)?;
                }
            }
            Geometry::GeometryCollection(gs) => {
                for g in gs.0.iter().rev() {
                    work.push(&mut pending, g)?;
                }
            }
            _ => return Err(PredicateError::UnsupportedGeometry),
        }
    }
    Ok(edges)
}

fn add_line(
    line: &LineString<f64>,
    edges: &mut CountedVec<Edge>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    if line.0.len() == 1 {
        return Err(PredicateError::InvalidGeometry {
            side: "linework",
            reason: "linea senza segmenti".to_owned(),
        });
    }
    for p in &line.0 {
        work.step()?;
        if !p.x.is_finite() || !p.y.is_finite() {
            return Err(PredicateError::NonFiniteCoordinate { side: "linework" });
        }
    }
    for pair in line.0.windows(2) {
        work.step()?;
        if pair[0] != pair[1] {
            work.nodes(2)?;
            let e = Edge {
                a: P::of_in(pair[0], work)?,
                b: P::of_in(pair[1], work)?,
            };
            work.push(edges, e)?;
        }
    }
    Ok(())
}

fn output_ring(
    ring: &FaceRing,
    coords: &[geo::Coord<f64>],
    work: &mut Work<'_>,
) -> Result<LineString<f64>, PredicateError> {
    let mut out = CountedVec::new();
    for i in 0..ring.atoms.len() {
        work.push(&mut out, coords[ring.vertices[i]])?;
    }
    work.push(&mut out, coords[ring.vertices[0]])?;
    Ok(LineString(out.export(work)?))
}

fn publish_faces(
    linework: &Geometry<f64>,
    points: &[P],
    rings: &[FaceRing],
    holes: &[CountedVec<usize>],
    work: &mut Work<'_>,
) -> Result<Vec<Polygon<f64>>, PredicateError> {
    let originals = Originals::for_editing(std::iter::empty(), linework, work)?;
    let mut selected = CountedVec::new();
    for (i, ring) in rings.iter().enumerate() {
        work.step()?;
        if ring.orientation.is_gt() {
            work.push(&mut selected, i)?;
            for &h in &holes[i] {
                work.push(&mut selected, h)?;
            }
        }
    }
    let mut boundary = CountedVec::new();
    let mut coords = CountedVec::new();
    for _ in points {
        work.push(&mut coords, geo::Coord { x: 0.0, y: 0.0 })?;
    }
    for &i in &selected {
        let vertices = &rings[i].vertices;
        for j in 0..vertices.len() {
            work.step()?;
            let (a, b) = (vertices[j], vertices[(j + 1) % vertices.len()]);
            coords[a] = originals
                .coordinate(&points[a], work)
                .map_err(publication_error)?;
            work.nodes(2)?;
            work.push(
                &mut boundary,
                (
                    Edge {
                        a: points[a].try_clone()?,
                        b: points[b].try_clone()?,
                    },
                    a,
                    b,
                ),
            )?;
        }
    }
    let mut output = CountedVec::new();
    for (i, ring) in rings.iter().enumerate() {
        work.step()?;
        if !ring.orientation.is_gt() {
            continue;
        }
        let exterior = output_ring(ring, &coords, work)?;
        let mut interiors = CountedVec::new();
        for &h in &holes[i] {
            let line = output_ring(&rings[h], &coords, work)?;
            work.push(&mut interiors, line)?;
        }
        let polygon = Polygon::new(exterior, interiors.export(work)?);
        work.push(&mut output, Geometry::Polygon(polygon))?;
    }
    let geometry = Geometry::GeometryCollection(geo::GeometryCollection(output.export(work)?));
    let mut published = CountedVec::new();
    for (edge, a, b) in &boundary {
        let mut certificate = PublishedEdge::new(edge, [coords[*a], coords[*b]], work)?;
        certificate.surface = true;
        work.push(&mut published, certificate)?;
    }
    verify_publication(&published, &originals, false, Some(&geometry), work)
        .map_err(publication_error)?;
    let Geometry::GeometryCollection(collection) = geometry else {
        return Err(PredicateError::CalcoloNonConcluso(
            "contenitore delle facce",
        ));
    };
    let mut output = CountedVec::new();
    for geometry in collection.0 {
        let Geometry::Polygon(polygon) = geometry else {
            return Err(PredicateError::CalcoloNonConcluso(
                "contenitore delle facce",
            ));
        };
        work.push(&mut output, polygon)?;
    }
    output.export(work)
}

fn polygonize(
    linework: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<Vec<Polygon<f64>>, PredicateError> {
    work.step()?;
    let edges = collect(linework, work)?;
    let mut refs = CountedVec::new();
    for e in &edges {
        work.push(&mut refs, e)?;
    }
    let nodes = crate::noding::node(&refs, work)?;
    let graph = crate::noding::faces::enumerate(&nodes, work)?;
    publish_faces(linework, &graph.points, &graph.rings, &graph.holes, work)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
