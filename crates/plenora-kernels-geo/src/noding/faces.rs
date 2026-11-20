//! Facce razionali e provenienza dei lati del noder comune.
use super::{sorted, CountedVec, Edge, Work, P};
use crate::predicates::interval_index::{BoxIndex, IntervalIndex};
use crate::predicates::PredicateError;
use std::cmp::Ordering;

/// Lato non orientato; sources contiene gli indici dei segmenti d'ingresso
/// del noder, ordinati e senza duplicati. Coincidenze conservano tutti gli id.
pub struct FaceAtom {
    pub from: usize,
    pub to: usize,
    pub sources: CountedVec<usize>,
}

pub struct FaceGraph {
    pub points: CountedVec<P>,
    pub atoms: CountedVec<FaceAtom>,
    pub rings: CountedVec<FaceRing>,
    pub holes: CountedVec<CountedVec<usize>>,
}

/// Riceve percorsi prodotti da node, nello stesso ordine dei lati sorgente.
/// Nessuna nuova intersezione viene costruita. Ogni anello conserva gli id
/// dei lati atomici nello stesso ordine dei vertici; ponti e dangles esclusi.
#[allow(
    clippy::too_many_lines,
    reason = "Sonde O-M3 per ciclo, assenti in produzione"
)]
pub fn enumerate(
    nodes: &[CountedVec<P>],
    work: &mut Work<'_>,
) -> Result<FaceGraph, PredicateError> {
    work.step()?;
    let mut points = CountedVec::new();
    for line in nodes {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::enumerate:1");
        work.checkpoint()?;
        for p in line {
            #[cfg(test)]
            crate::noding::gop028::cycle("noding/faces.rs::enumerate:2");
            work.checkpoint()?;
            work.push(&mut points, p.try_clone()?)?;
        }
    }
    sorted(&mut points, compare, work)?;
    points.dedup_by(|a, b| Ok(a == b), work)?;
    let (mut arcs, atoms) = graph(nodes, &points, work)?;
    let rings = rings(&mut arcs, &points, work)?;
    let mut holes = CountedVec::new();
    for _ in &rings {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::enumerate:3");
        work.checkpoint()?;
        work.push(&mut holes, CountedVec::new())?;
    }
    let mut bounds = CountedVec::new();
    let mut owners = CountedVec::new();
    for _ in &atoms {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::enumerate:4");
        work.checkpoint()?;
        work.push(&mut owners, None)?;
    }
    for (id, ring) in rings.iter().enumerate() {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::enumerate:5");
        work.checkpoint()?;
        let p = &points[ring.vertices[0]];
        let mut bb = [
            p.x.try_clone()?,
            p.x.try_clone()?,
            p.y.try_clone()?,
            p.y.try_clone()?,
        ];
        for &v in &ring.vertices {
            #[cfg(test)]
            crate::noding::gop028::cycle("noding/faces.rs::enumerate:6");
            work.checkpoint()?;
            for (i, q) in [&points[v].x, &points[v].x, &points[v].y, &points[v].y]
                .into_iter()
                .enumerate()
            {
                #[cfg(test)]
                crate::noding::gop028::cycle("noding/faces.rs::enumerate:7");
                work.step()?;
                let order = q.try_cmp(&bb[i])?;
                if (i % 2 == 0 && order.is_lt()) || (i % 2 == 1 && order.is_gt()) {
                    bb[i] = q.try_clone()?;
                }
            }
        }
        work.push(&mut bounds, bb)?;
        if ring.orientation.is_gt() {
            for &atom in &ring.atoms {
                #[cfg(test)]
                crate::noding::gop028::cycle("noding/faces.rs::enumerate:8");
                work.step()?;
                owners[atom] = Some(id);
            }
        }
    }
    let index = BoxIndex::new(bounds, work)?;
    for (h, hole) in rings.iter().enumerate() {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::enumerate:9");
        work.step()?;
        if !hole.orientation.is_lt() {
            continue;
        }
        let mut parent: Option<usize> = None;
        for i in index.point_candidates(&points[hole.vertices[0]], work)? {
            #[cfg(test)]
            crate::noding::gop028::cycle("noding/faces.rs::enumerate:10");
            work.checkpoint()?;
            let shell = &rings[i];
            work.step()?;
            if !shell.orientation.is_gt() || owners[hole.atoms[0]] == Some(i) {
                continue;
            }
            // Un vertice strettamente interno basta: cicli del grafo gia' nodato
            // non possono attraversarsi. I contatti sul bordo vengono saltati.
            let contained = nested(hole, shell, &points, work)?;
            if contained
                && match parent {
                    None => true,
                    Some(j) => nested(shell, &rings[j], &points, work)?,
                }
            {
                parent = Some(i);
            }
        }
        if let Some(i) = parent {
            work.push(&mut holes[i], h)?;
        }
    }
    Ok(FaceGraph {
        points,
        atoms,
        rings,
        holes,
    })
}

fn compare(a: &P, b: &P) -> Result<Ordering, PredicateError> {
    let x = a.x.try_cmp(&b.x)?;
    if x.is_eq() {
        a.y.try_cmp(&b.y)
    } else {
        Ok(x)
    }
}

pub struct ArcSide {
    pub from: usize,
    pub to: usize,
    pub next: usize,
    pub seen: bool,
    pub atom: usize,
}
pub struct FaceRing {
    index: IntervalIndex,
    pub vertices: CountedVec<usize>,
    pub orientation: Ordering,
    pub atoms: CountedVec<usize>,
}

fn vertex(points: &[P], p: &P, work: &mut Work<'_>) -> Result<usize, PredicateError> {
    let (mut lo, mut hi) = (0, points.len());
    while lo < hi {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::vertex:1");
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
) -> Result<(CountedVec<ArcSide>, CountedVec<FaceAtom>), PredicateError> {
    let mut pairs = CountedVec::new();
    for (source, line) in nodes.iter().enumerate() {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::graph:1");
        work.checkpoint()?;
        for pair in line.windows(2) {
            #[cfg(test)]
            crate::noding::gop028::cycle("noding/faces.rs::graph:2");
            work.checkpoint()?;
            let a = vertex(points, &pair[0], work)?;
            let b = vertex(points, &pair[1], work)?;
            work.push(&mut pairs, (a.min(b), a.max(b), source))?;
        }
    }
    sorted(&mut pairs, |a, b| Ok(a.cmp(b)), work)?;

    let mut arcs = CountedVec::new();
    let mut outgoing = CountedVec::new();
    for _ in points {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::graph:3");
        work.checkpoint()?;
        work.push(&mut outgoing, CountedVec::new())?;
    }
    let mut atoms: CountedVec<FaceAtom> = CountedVec::new();
    for (a, b, source) in pairs {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::graph:4");
        work.step()?;
        if let Some(atom) = atoms.last_mut() {
            if (atom.from, atom.to) == (a, b) {
                if atom.sources.last() != Some(&source) {
                    work.push(&mut atom.sources, source)?;
                }
                continue;
            }
        }
        let mut sources = CountedVec::new();
        work.push(&mut sources, source)?;
        let atom = atoms.len();
        work.push(
            &mut atoms,
            FaceAtom {
                from: a,
                to: b,
                sources,
            },
        )?;
        for (from, to) in [(a, b), (b, a)] {
            #[cfg(test)]
            crate::noding::gop028::cycle("noding/faces.rs::graph:5");
            work.push(&mut outgoing[from], arcs.len())?;
            work.push(
                &mut arcs,
                ArcSide {
                    from,
                    to,
                    next: 0,
                    seen: false,
                    atom,
                },
            )?;
        }
    }
    for (v, list) in outgoing.iter_mut().enumerate() {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::graph:6");
        work.checkpoint()?;
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
            #[cfg(test)]
            crate::noding::gop028::cycle("noding/faces.rs::graph:7");
            work.step()?;
            // Predecessore antiorario del gemello: mantiene la faccia a sinistra.
            arcs[list[i] ^ 1].next = list[if i == 0 { list.len() - 1 } else { i - 1 }];
        }
    }
    Ok((arcs, atoms))
}

/// Un ciclo semplice ha il verso della svolta al minimo lessicografico.
fn orientation(
    vertices: &[usize],
    points: &[P],
    work: &mut Work<'_>,
) -> Result<Ordering, PredicateError> {
    if vertices.len() < 3 {
        return Ok(Ordering::Equal);
    }
    let mut minimum = 0;
    for i in 1..vertices.len() {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::orientation:1");
        work.step()?;
        if vertices[i] < vertices[minimum] {
            minimum = i;
        }
    }
    let n = vertices.len();
    let p = &points[vertices[minimum]];
    let prev = &points[vertices[(minimum + n - 1) % n]];
    let next = &points[vertices[(minimum + 1) % n]];
    Ok(p.try_sub(prev)?.try_cross(&next.try_sub(p)?)?.sign())
}

fn rings(
    arcs: &mut [ArcSide],
    points: &[P],
    work: &mut Work<'_>,
) -> Result<CountedVec<FaceRing>, PredicateError> {
    let mut result = CountedVec::new();
    walk(
        arcs,
        points.len(),
        work,
        |_| Ok(()),
        |path, arcs, work| {
            let mut vertices = CountedVec::new();
            let mut atoms = CountedVec::new();
            for &id in path {
                #[cfg(test)]
                crate::noding::gop028::cycle("noding/faces.rs::rings:1");
                work.step()?;
                let arc = &arcs[id];
                work.push(&mut vertices, arc.from)?;
                work.push(&mut atoms, arc.atom)?;
            }
            let orientation = orientation(&vertices, points, work)?;
            if !orientation.is_eq() {
                let index = IntervalIndex::new(
                    vertices.iter().enumerate().map(|(i, &v)| {
                        (&points[v].y, &points[vertices[(i + 1) % vertices.len()]].y)
                    }),
                    work,
                )?;
                work.push(
                    &mut result,
                    FaceRing {
                        index,
                        vertices,
                        orientation,
                        atoms,
                    },
                )?;
            }
            Ok(())
        },
    )?;
    Ok(result)
}

/// Separa i cammini ai vertici ripetuti, senza sommare aree razionali.
/// I callback addebitano facce/anelli prima di materializzarli.
pub fn walk(
    arcs: &mut [ArcSide],
    vertex_count: usize,
    work: &mut Work<'_>,
    mut face: impl FnMut(&mut Work<'_>) -> Result<(), PredicateError>,
    mut ring: impl FnMut(&[usize], &[ArcSide], &mut Work<'_>) -> Result<(), PredicateError>,
) -> Result<(), PredicateError> {
    let mut positions = CountedVec::new();
    for _ in 0..vertex_count {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::walk:1");
        work.checkpoint()?;
        work.push(&mut positions, None)?;
    }
    for start in 0..arcs.len() {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::walk:2");
        work.step()?;
        if arcs[start].seen {
            continue;
        }
        face(work)?;
        let mut path = CountedVec::new();
        let mut current = start;
        loop {
            #[cfg(test)]
            crate::noding::gop028::cycle("noding/faces.rs::walk:3");
            work.step()?;
            let v = arcs[current].from;
            if let Some(pos) = positions[v] {
                ring(&path[pos..], arcs, work)?;
                while path.len() > pos {
                    #[cfg(test)]
                    crate::noding::gop028::cycle("noding/faces.rs::walk:4");
                    work.step()?;
                    if let Some(a) = path.pop() {
                        positions[arcs[a].from] = None;
                    }
                }
            }
            if arcs[current].seen {
                break;
            }
            positions[v] = Some(path.len());
            work.push(&mut path, current)?;
            arcs[current].seen = true;
            current = arcs[current].next;
        }
        if !path.is_empty() {
            return Err(PredicateError::CalcoloNonConcluso("cammino non chiuso"));
        }
    }
    Ok(())
}

fn nested(
    inner: &FaceRing,
    outer: &FaceRing,
    points: &[P],
    work: &mut Work<'_>,
) -> Result<bool, PredicateError> {
    for &v in &inner.vertices {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::nested:1");
        work.step()?;
        if let Some(inside) = inside(&points[v], outer, points, work)? {
            return Ok(inside);
        }
    }
    Ok(false)
}

// Parita' del raggio orizzontale; None distingue il bordo dall'esterno.
fn inside(
    p: &P,
    ring: &FaceRing,
    points: &[P],
    work: &mut Work<'_>,
) -> Result<Option<bool>, PredicateError> {
    let mut odd = false;
    for i in ring.index.candidates(&p.y, work)? {
        #[cfg(test)]
        crate::noding::gop028::cycle("noding/faces.rs::inside:1");
        work.step()?;
        let a = &points[ring.vertices[i]];
        let b = &points[ring.vertices[(i + 1) % ring.vertices.len()]];
        let edge = Edge {
            a: a.try_clone()?,
            b: b.try_clone()?,
        };
        if edge.contains(p)? {
            return Ok(None);
        }
        if a.y.try_cmp(&p.y)?.is_gt() != b.y.try_cmp(&p.y)?.is_gt() {
            let cross = b.try_sub(a)?.try_cross(&p.try_sub(a)?)?.sign();
            if cross == b.y.try_cmp(&a.y)? {
                odd = !odd;
            }
        }
    }
    Ok(Some(odd))
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
