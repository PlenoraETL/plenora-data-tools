//! Enumerazione comune dei cicli di un grafo planare orientato gia' nodato.
use super::{CountedVec, Work};
use crate::predicates::PredicateError;

pub struct ArcSide {
    pub from: usize,
    pub to: usize,
    pub next: usize,
    pub seen: bool,
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
        work.push(&mut positions, None)?;
    }
    for start in 0..arcs.len() {
        work.step()?;
        if arcs[start].seen {
            continue;
        }
        face(work)?;
        let mut path = CountedVec::new();
        let mut current = start;
        loop {
            work.step()?;
            let v = arcs[current].from;
            if let Some(pos) = positions[v] {
                ring(&path[pos..], arcs, work)?;
                while path.len() > pos {
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
