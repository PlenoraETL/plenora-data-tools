//! Facce razionali e provenienza dei lati del noder comune.
use super::{sorted, CountedVec, Edge, Work, P, Q};
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
pub fn enumerate(
    nodes: &[CountedVec<P>],
    work: &mut Work<'_>,
) -> Result<FaceGraph, PredicateError> {
    work.step()?;
    let mut points = CountedVec::new();
    for line in nodes {
        for p in line {
            work.push(&mut points, p.try_clone()?)?;
        }
    }
    sorted(&mut points, compare, work)?;
    points.dedup_by(|a, b| Ok(a == b), work)?;
    let (mut arcs, atoms) = graph(nodes, &points, work)?;
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

struct ArcSide {
    pub from: usize,
    pub to: usize,
    next: usize,
    seen: bool,
    pub atom: usize,
}
pub struct FaceRing {
    pub vertices: CountedVec<usize>,
    pub area: Q,
    pub atoms: CountedVec<usize>,
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
) -> Result<(CountedVec<ArcSide>, CountedVec<FaceAtom>), PredicateError> {
    let mut pairs = CountedVec::new();
    for (source, line) in nodes.iter().enumerate() {
        for pair in line.windows(2) {
            let a = vertex(points, &pair[0], work)?;
            let b = vertex(points, &pair[1], work)?;
            work.push(&mut pairs, (a.min(b), a.max(b), source))?;
        }
    }
    sorted(&mut pairs, |a, b| Ok(a.cmp(b)), work)?;

    let mut arcs = CountedVec::new();
    let mut outgoing = CountedVec::new();
    for _ in points {
        work.push(&mut outgoing, CountedVec::new())?;
    }
    let mut atoms: CountedVec<FaceAtom> = CountedVec::new();
    for (a, b, source) in pairs {
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
    Ok((arcs, atoms))
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
        let mut path_atoms = CountedVec::new();
        let mut current = start;
        loop {
            work.step()?;
            let v = arcs[current].from;
            if let Some(pos) = positions[v] {
                let mut vertices = CountedVec::new();
                let mut atoms = CountedVec::new();
                let mut area = work.integer(0)?;
                for i in pos..path.len() {
                    work.step()?;
                    let a = path[i];
                    let b = if i + 1 == path.len() { v } else { path[i + 1] };
                    area = area.try_add(&points[a].try_cross(&points[b])?)?;
                    work.push(&mut vertices, a)?;
                    work.push(&mut atoms, path_atoms[i])?;
                }
                while path.len() > pos {
                    work.step()?;
                    path_atoms.pop();
                    if let Some(a) = path.pop() {
                        positions[a] = None;
                    }
                }
                if !area.sign().is_eq() {
                    work.push(
                        &mut result,
                        FaceRing {
                            vertices,
                            area,
                            atoms,
                        },
                    )?;
                }
            }
            if arcs[current].seen {
                break;
            }
            positions[v] = Some(path.len());
            work.push(&mut path, v)?;
            work.push(&mut path_atoms, arcs[current].atom)?;
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

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
