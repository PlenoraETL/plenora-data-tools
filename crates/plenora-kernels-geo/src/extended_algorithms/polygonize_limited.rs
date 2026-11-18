//! Facce razionali del grafo planare del linework.
#![allow(clippy::float_cmp)]
use crate::noding::faces::{FaceAtom, FaceRing};
use crate::noding::{CountedVec, Edge, Work, P};
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
/// Cancellazione cooperativa con precedenza sulla deadline.
/// XY planari in metri: topologia razionale esatta, conversione finale con
/// errore euclideo verificato <= 1 cm. Collassi, nuovi incroci/overlap e
/// inversioni di orientamento dopo la conversione sono rifiutati. Anche
/// la verifica del grafo convertito consuma coppie, nodi, passi e memoria.
/// La polygonize pubblicata in 6.0.0 conserva il proprio contratto.
///
/// # Errors
/// [`PredicateError`] per coordinate non finite, struttura o tipi non
/// supportati, limiti, allocazione, cancellazione, deadline; `CalcoloNonConcluso`
/// per conversioni oltre 1 cm o collassi nell'output.
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
            _ => {
                return Err(PredicateError::CalcoloNonConcluso(
                    "tipo di linework non supportato",
                ))
            }
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

// Verifica anche interazioni tra facce: validare un poligono alla volta
// non rileverebbe un nuovo incrocio fra due componenti arrotondate.
fn verify_output(
    atoms: &[FaceAtom],
    rings: &[FaceRing],
    coords: &[geo::Coord<f64>],
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    let mut points = CountedVec::new();
    for &c in coords {
        work.nodes(1)?;
        work.push(&mut points, P::of_in(c, work)?)?;
    }
    let mut edges = CountedVec::new();
    for arc in atoms {
        work.step()?;
        let edge = Edge {
            a: points[arc.from].try_clone()?,
            b: points[arc.to].try_clone()?,
        };
        work.push(&mut edges, edge)?;
    }
    let mut refs = CountedVec::new();
    for edge in &edges {
        work.push(&mut refs, edge)?;
    }
    crate::noding::visit(&refs, work, |i, j, hits, _work| {
        if hits[1].is_some()
            || hits[0].as_ref().is_some_and(|p| {
                (p != &edges[i].a && p != &edges[i].b) || (p != &edges[j].a && p != &edges[j].b)
            })
        {
            return Err(PredicateError::CalcoloNonConcluso(
                "topologia del grafo convertito",
            ));
        }
        Ok(())
    })?;
    for ring in rings {
        let mut area = work.integer(0)?;
        for i in 0..ring.vertices.len() {
            work.step()?;
            area = area.try_add(
                &points[ring.vertices[i]]
                    .try_cross(&points[ring.vertices[(i + 1) % ring.vertices.len()]])?,
            )?;
        }
        if area.sign() != ring.area.sign() {
            return Err(PredicateError::CalcoloNonConcluso(
                "orientamento convertito",
            ));
        }
    }
    Ok(())
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
    let points = &graph.points;
    let atoms = &graph.atoms;
    let rings = &graph.rings;
    let holes = &graph.holes;
    let mut coords = CountedVec::new();
    for p in points {
        let c = super::split_limited::coordinate(p, work)?;
        // Controllo globale: anche vertici non adiacenti non possono fondersi.
        for old in &coords {
            work.step()?;
            if *old == c {
                return Err(PredicateError::CalcoloNonConcluso("collasso del nodo"));
            }
        }
        work.push(&mut coords, c)?;
    }
    verify_output(atoms, rings, &coords, work)?;
    let mut output = CountedVec::new();
    for (i, ring) in rings.iter().enumerate() {
        work.step()?;
        if !ring.area.sign().is_gt() {
            continue;
        }
        let exterior = output_ring(ring, &coords, work)?;
        let mut interiors = CountedVec::new();
        for &h in &holes[i] {
            let line = output_ring(&rings[h], &coords, work)?;
            work.push(&mut interiors, line)?;
        }
        let polygon = Polygon::new(exterior, interiors.export(work)?);
        work.push(&mut output, polygon)?;
    }
    output.export(work)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
