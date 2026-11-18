//! Facce razionali del grafo planare del linework.
#![allow(clippy::float_cmp)]
use crate::noding::{sorted, CountedVec, Edge, Work, P, Q};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Geometry, LineString, Polygon};
use std::cmp::Ordering;

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

fn compare(a: &P, b: &P) -> Result<Ordering, PredicateError> {
    let x = a.x.try_cmp(&b.x)?;
    if x.is_eq() {
        a.y.try_cmp(&b.y)
    } else {
        Ok(x)
    }
}

struct ArcSide {
    from: usize,
    to: usize,
    next: usize,
    seen: bool,
}
struct FaceRing {
    vertices: CountedVec<usize>,
    area: Q,
}

fn vertex(points: &[P], p: &P, work: &mut Work<'_>) -> Result<usize, PredicateError> {
    let (mut lo, mut hi) = (0, points.len());
    while lo < hi {
        work.step()?;
        let mid = lo + (hi - lo) / 2;
        match compare(&points[mid], p)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(mid),
        }
    }
    Err(PredicateError::CalcoloNonConcluso("vertice del grafo"))
}

fn angle(a: &P, b: &P) -> Result<Ordering, PredicateError> {
    let half = |p: &P| p.y.sign().is_lt() || (p.y.sign().is_eq() && p.x.sign().is_lt());
    let order = half(a).cmp(&half(b));
    if order.is_eq() {
        Ok(a.try_cross(b)?.sign().reverse())
    } else {
        Ok(order)
    }
}

fn graph(
    nodes: &[CountedVec<P>],
    points: &[P],
    work: &mut Work<'_>,
) -> Result<CountedVec<ArcSide>, PredicateError> {
    let mut pairs = CountedVec::new();
    for line in nodes {
        for pair in line.windows(2) {
            let a = vertex(points, &pair[0], work)?;
            let b = vertex(points, &pair[1], work)?;
            work.push(&mut pairs, (a.min(b), a.max(b)))?;
        }
    }
    sorted(&mut pairs, |a, b| Ok(a.cmp(b)), work)?;
    pairs.dedup_by(|a, b| Ok(a == b), work)?;
    let mut arcs = CountedVec::new();
    let mut outgoing = CountedVec::new();
    for _ in points {
        work.push(&mut outgoing, CountedVec::new())?;
    }
    for (a, b) in pairs {
        for (from, to) in [(a, b), (b, a)] {
            work.push(&mut outgoing[from], arcs.len())?;
            work.push(
                &mut arcs,
                ArcSide {
                    from,
                    to,
                    next: 0,
                    seen: false,
                },
            )?;
        }
    }
    for (v, list) in outgoing.iter_mut().enumerate() {
        sorted(
            list,
            |a, b| {
                angle(
                    &points[arcs[*a].to].try_sub(&points[v])?,
                    &points[arcs[*b].to].try_sub(&points[v])?,
                )
            },
            work,
        )?;
        for i in 0..list.len() {
            work.step()?;
            // Predecessore antiorario del gemello: mantiene la faccia a sinistra.
            arcs[list[i] ^ 1].next = list[if i == 0 { list.len() - 1 } else { i - 1 }];
        }
    }
    Ok(arcs)
}

fn rings(
    arcs: &mut [ArcSide],
    points: &[P],
    work: &mut Work<'_>,
) -> Result<CountedVec<FaceRing>, PredicateError> {
    let mut result = CountedVec::new();
    let mut positions = CountedVec::new();
    for _ in points {
        work.push(&mut positions, None)?;
    }
    for start in 0..arcs.len() {
        work.step()?;
        if arcs[start].seen {
            continue;
        }
        let mut path = CountedVec::new();
        let mut current = start;
        loop {
            work.step()?;
            let v = arcs[current].from;
            if let Some(pos) = positions[v] {
                let mut vertices = CountedVec::new();
                let mut area = work.integer(0)?;
                for i in pos..path.len() {
                    work.step()?;
                    let a = path[i];
                    let b = if i + 1 == path.len() { v } else { path[i + 1] };
                    area = area.try_add(&points[a].try_cross(&points[b])?)?;
                    work.push(&mut vertices, a)?;
                }
                while path.len() > pos {
                    work.step()?;
                    if let Some(a) = path.pop() {
                        positions[a] = None;
                    }
                }
                if !area.sign().is_eq() {
                    work.push(&mut result, FaceRing { vertices, area })?;
                }
            }
            if arcs[current].seen {
                break;
            }
            positions[v] = Some(path.len());
            work.push(&mut path, v)?;
            arcs[current].seen = true;
            current = arcs[current].next;
        }
        if !path.is_empty() {
            return Err(PredicateError::CalcoloNonConcluso("cammino non chiuso"));
        }
    }
    Ok(result)
}

// Parita' del raggio orizzontale; un punto sul bordo non e' interno.
fn inside(
    p: &P,
    ring: &FaceRing,
    points: &[P],
    work: &mut Work<'_>,
) -> Result<bool, PredicateError> {
    let mut odd = false;
    for i in 0..ring.vertices.len() {
        work.step()?;
        let a = &points[ring.vertices[i]];
        let b = &points[ring.vertices[(i + 1) % ring.vertices.len()]];
        let edge = Edge {
            a: a.try_clone()?,
            b: b.try_clone()?,
        };
        if edge.contains(p)? {
            return Ok(false);
        }
        if a.y.try_cmp(&p.y)?.is_gt() != b.y.try_cmp(&p.y)?.is_gt() {
            let cross = b.try_sub(a)?.try_cross(&p.try_sub(a)?)?.sign();
            if cross == b.y.try_cmp(&a.y)? {
                odd = !odd;
            }
        }
    }
    Ok(odd)
}

fn output_ring(
    ring: &FaceRing,
    coords: &[geo::Coord<f64>],
    work: &mut Work<'_>,
) -> Result<LineString<f64>, PredicateError> {
    let mut out = CountedVec::new();
    for &v in &ring.vertices {
        work.push(&mut out, coords[v])?;
    }
    work.push(&mut out, coords[ring.vertices[0]])?;
    Ok(LineString(out.export(work)?))
}

// Verifica anche interazioni tra facce: validare un poligono alla volta
// non rileverebbe un nuovo incrocio fra due componenti arrotondate.
fn verify_output(
    arcs: &[ArcSide],
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
    for arc in arcs.iter().step_by(2) {
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
    let mut points = CountedVec::new();
    for line in &nodes {
        for p in line {
            work.push(&mut points, p.try_clone()?)?;
        }
    }
    sorted(&mut points, compare, work)?;
    points.dedup_by(|a, b| Ok(a == b), work)?;
    let mut arcs = graph(&nodes, &points, work)?;
    let rings = rings(&mut arcs, &points, work)?;
    let mut holes = CountedVec::new();
    for _ in &rings {
        work.push(&mut holes, CountedVec::new())?;
    }
    for (h, hole) in rings.iter().enumerate() {
        work.step()?;
        if !hole.area.sign().is_lt() {
            continue;
        }
        let mut parent: Option<usize> = None;
        for (i, shell) in rings.iter().enumerate() {
            work.step()?;
            if !shell.area.sign().is_gt() {
                continue;
            }
            // Un vertice strettamente interno basta: cicli del grafo gia' nodato
            // non possono attraversarsi. I contatti sul bordo vengono saltati.
            let mut contained = false;
            for &v in &hole.vertices {
                if inside(&points[v], shell, &points, work)? {
                    contained = true;
                    break;
                }
            }
            if contained
                && match parent {
                    None => true,
                    Some(j) => shell.area.try_cmp(&rings[j].area)?.is_lt(),
                }
            {
                parent = Some(i);
            }
        }
        if let Some(i) = parent {
            work.push(&mut holes[i], h)?;
        }
    }
    let mut coords = CountedVec::new();
    for p in &points {
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
    verify_output(&arcs, &rings, &coords, work)?;
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
