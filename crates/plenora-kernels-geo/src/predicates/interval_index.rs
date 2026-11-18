//! Intervalli chiusi razionali: visita O(log n + k), memoria O(n).
use super::PredicateError;
use crate::noding::{CountedVec, Work, Q};
use std::cmp::Ordering;

struct ClosedSpan {
    low: Q,
    high: Q,
}

struct Branch {
    center: usize,
    lower: CountedVec<usize>,
    upper: CountedVec<usize>,
    left: Option<usize>,
    right: Option<usize>,
}

#[derive(Default)]
pub struct IntervalIndex {
    intervals: CountedVec<ClosedSpan>,
    branches: CountedVec<Branch>,
    root: Option<usize>,
}

// Heapsort fallibile, senza allocazioni e con ogni confronto contato.
pub fn sort_ids(
    ids: &mut [usize],
    cmp: impl Fn(usize, usize) -> Result<Ordering, PredicateError>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    fn sift(
        ids: &mut [usize],
        mut root: usize,
        cmp: &impl Fn(usize, usize) -> Result<Ordering, PredicateError>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        while root < ids.len() / 2 {
            let mut child = root * 2 + 1;
            if child + 1 < ids.len() {
                work.step()?;
                if cmp(ids[child], ids[child + 1])?.is_lt() {
                    child += 1;
                }
            }
            work.step()?;
            if !cmp(ids[root], ids[child])?.is_lt() {
                break;
            }
            ids.swap(root, child);
            root = child;
        }
        Ok(())
    }
    for root in (0..ids.len() / 2).rev() {
        sift(ids, root, &cmp, work)?;
    }
    for end in (1..ids.len()).rev() {
        work.step()?;
        ids.swap(0, end);
        sift(&mut ids[..end], 0, &cmp, work)?;
    }
    Ok(())
}

impl IntervalIndex {
    pub fn new<'a>(
        bounds: impl Iterator<Item = (&'a Q, &'a Q)>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        let mut index = Self::default();
        let mut ids = CountedVec::new();
        for (a, b) in bounds {
            work.step()?;
            let (low, high) = if a.try_cmp(b)?.is_le() {
                (a, b)
            } else {
                (b, a)
            };
            work.push(&mut ids, index.intervals.len())?;
            work.push(
                &mut index.intervals,
                ClosedSpan {
                    low: low.try_clone()?,
                    high: high.try_clone()?,
                },
            )?;
        }
        sort_ids(
            &mut ids,
            |a, b| {
                Ok(index.intervals[a]
                    .low
                    .try_cmp(&index.intervals[b].low)?
                    .then(a.cmp(&b)))
            },
            work,
        )?;
        index.root = index.build(ids, work)?;
        Ok(index)
    }

    fn build(
        &mut self,
        ids: CountedVec<usize>,
        work: &mut Work<'_>,
    ) -> Result<Option<usize>, PredicateError> {
        if ids.is_empty() {
            return Ok(None);
        }
        // La mediana degli inizi lascia al massimo n/2 intervalli per figlio.
        let center = ids[ids.len() / 2];
        let cut = &self.intervals[center].low;
        let mut lower = CountedVec::new();
        let mut upper = CountedVec::new();
        let mut left = CountedVec::new();
        let mut right = CountedVec::new();
        for id in ids {
            work.step()?;
            let interval = &self.intervals[id];
            if interval.high.try_cmp(cut)?.is_lt() {
                work.push(&mut left, id)?;
            } else if interval.low.try_cmp(cut)?.is_gt() {
                work.push(&mut right, id)?;
            } else {
                work.push(&mut lower, id)?;
                work.push(&mut upper, id)?;
            }
        }
        sort_ids(
            &mut upper,
            |a, b| {
                Ok(self.intervals[b]
                    .high
                    .try_cmp(&self.intervals[a].high)?
                    .then(a.cmp(&b)))
            },
            work,
        )?;
        let left = self.build(left, work)?;
        let right = self.build(right, work)?;
        let id = self.branches.len();
        work.push(
            &mut self.branches,
            Branch {
                center,
                lower,
                upper,
                left,
                right,
            },
        )?;
        Ok(Some(id))
    }

    /// Intervalli che intersecano [low, high], inclusi i contatti.
    pub fn overlapping(
        &self,
        low: &Q,
        high: &Q,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<usize>, PredicateError> {
        let mut pending = CountedVec::new();
        let mut result = CountedVec::new();
        if let Some(root) = self.root {
            work.push(&mut pending, root)?;
        }
        while let Some(id) = pending.pop() {
            work.step()?;
            let branch = &self.branches[id];
            let center = &self.intervals[branch.center].low;
            let left = low.try_cmp(center)?.is_le();
            let right = high.try_cmp(center)?.is_ge();
            let entries = if left { &branch.lower } else { &branch.upper };
            for &entry in entries {
                work.step()?;
                let span = &self.intervals[entry];
                if (left && span.low.try_cmp(high)?.is_gt())
                    || (!left && span.high.try_cmp(low)?.is_lt())
                {
                    break;
                }
                work.push(&mut result, entry)?;
            }
            if left {
                if let Some(id) = branch.left {
                    work.push(&mut pending, id)?;
                }
            }
            if right {
                if let Some(id) = branch.right {
                    work.push(&mut pending, id)?;
                }
            }
        }
        Ok(result)
    }

    pub fn candidates(
        &self,
        value: &Q,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<usize>, PredicateError> {
        let mut result = CountedVec::new();
        let mut current = self.root;
        while let Some(id) = current {
            work.step()?;
            let branch = &self.branches[id];
            let side = value.try_cmp(&self.intervals[branch.center].low)?;
            let entries = if side.is_gt() {
                &branch.upper
            } else {
                &branch.lower
            };
            for &entry in entries {
                work.step()?;
                let interval = &self.intervals[entry];
                if (side.is_gt() && interval.high.try_cmp(value)?.is_lt())
                    || (!side.is_gt() && interval.low.try_cmp(value)?.is_gt())
                {
                    break;
                }
                work.push(&mut result, entry)?;
            }
            current = match side {
                Ordering::Less => branch.left,
                Ordering::Greater => branch.right,
                Ordering::Equal => None,
            };
        }
        Ok(result)
    }
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::predicates::RelateLimits;

    #[test]
    fn ranges_and_boxes_match_exhaustive_candidates() {
        let mut work = Work::new(RelateLimits::default(), None);
        let mut bounds = CountedVec::new();
        for x in -4..=4 {
            for y in -3..=3 {
                let b = [
                    work.integer(x).expect("x"),
                    work.integer(x + 2).expect("x"),
                    work.integer(y).expect("y"),
                    work.integer(y + 1).expect("y"),
                ];
                work.push(&mut bounds, b).expect("bounds");
            }
        }
        let index = BoxIndex::new(bounds, &mut work).expect("index");
        for x in -6..=6 {
            for y in -5..=5 {
                let q = [
                    work.integer(x).expect("x"),
                    work.integer(x + 1).expect("x"),
                    work.integer(y).expect("y"),
                    work.integer(y + 2).expect("y"),
                ];
                let mut actual: Vec<_> = index
                    .candidates(&q, &mut work)
                    .expect("query")
                    .into_iter()
                    .collect();
                actual.sort_unstable();
                let expected: Vec<_> = index
                    .bounds
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| {
                        b[0].try_cmp(&q[1]).expect("cmp").is_le()
                            && b[1].try_cmp(&q[0]).expect("cmp").is_ge()
                            && b[2].try_cmp(&q[3]).expect("cmp").is_le()
                            && b[3].try_cmp(&q[2]).expect("cmp").is_ge()
                    })
                    .map(|(i, _)| i)
                    .collect();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn sparse_queries_have_analytic_work() {
        for height in 1..=9 {
            let mut work = Work::new(RelateLimits::default(), None);
            let count = (1 << height) - 1;
            let bounds: Vec<_> = (0..count)
                .map(|i| {
                    (
                        work.integer(4 * i).expect("low"),
                        work.integer(4 * i + 1).expect("high"),
                    )
                })
                .collect();
            let index =
                IntervalIndex::new(bounds.iter().map(|(a, b)| (a, b)), &mut work).expect("indice");
            // Albero perfetto: h nodi, una prova per nodo, un solo risultato.
            let start = work.steps;
            assert_eq!(
                &*index
                    .candidates(&work.integer(1).expect("query"), &mut work)
                    .expect("candidati"),
                &[0]
            );
            assert_eq!(
                work.steps - start,
                2 * u64::try_from(height).expect("altezza") + 1
            );
            let start = work.steps;
            assert!(index
                .candidates(&work.integer(-1).expect("query"), &mut work)
                .expect("candidati")
                .is_empty());
            assert_eq!(
                work.steps - start,
                2 * u64::try_from(height).expect("altezza")
            );
        }
    }

    #[test]
    fn grid_queries_have_analytic_work() {
        let mut work = Work::new(RelateLimits::default(), None);
        let bounds: Vec<_> = (0..8)
            .flat_map(|i| [(0, 7), (i, i)])
            .map(|(a, b)| {
                (
                    work.integer(a).expect("low"),
                    work.integer(b).expect("high"),
                )
            })
            .collect();
        let index =
            IntervalIndex::new(bounds.iter().map(|(a, b)| (a, b)), &mut work).expect("indice");
        let start = work.steps;
        let mut candidates = 0;
        for i in 0..8 {
            candidates += index
                .candidates(&work.integer(i).expect("query"), &mut work)
                .expect("candidati")
                .len();
        }
        // 8 orizzontali e una verticale per ascissa; radice a zero e
        // sottoalbero perfetto di 7 punti, somma delle profondita' 17.
        assert_eq!(candidates, 8 * 9);
        assert_eq!(work.steps - start, 19 + 7 * 18 + 2 * 17 + 7);
    }

    #[test]
    fn interval_candidates_match_exhaustive_including_reversed_bounds() {
        let mut work = Work::new(RelateLimits::default(), None);
        let bounds: Vec<_> = (-5..=5)
            .flat_map(|a| (-5..=5).map(move |b| (a, b)))
            .map(|(a, b)| {
                (
                    work.integer(a).expect("low"),
                    work.integer(b).expect("high"),
                )
            })
            .collect();
        let index =
            IntervalIndex::new(bounds.iter().map(|(a, b)| (a, b)), &mut work).expect("indice");
        for i in -12..=12 {
            let query = work
                .integer(i)
                .expect("numeratore")
                .try_midpoint(&work.integer(0).expect("zero"))
                .expect("razionale");
            let mut actual: Vec<_> = index
                .candidates(&query, &mut work)
                .expect("candidati")
                .into_iter()
                .collect();
            actual.sort_unstable();
            let expected: Vec<_> = bounds
                .iter()
                .enumerate()
                .filter_map(|(id, (a, b))| {
                    crate::noding::between(&query, a, b)
                        .expect("confronto")
                        .then_some(id)
                })
                .collect();
            assert_eq!(actual, expected);
        }
    }
}

/// Inviluppi razionali chiusi; il filtro Y non introduce arrotondamenti.
#[derive(Default)]
pub struct BoxIndex {
    bounds: CountedVec<[Q; 4]>,
    x: IntervalIndex,
}
impl BoxIndex {
    pub fn new(bounds: CountedVec<[Q; 4]>, work: &mut Work<'_>) -> Result<Self, PredicateError> {
        let x = IntervalIndex::new(bounds.iter().map(|b| (&b[0], &b[1])), work)?;
        Ok(Self { bounds, x })
    }
    pub fn candidates(
        &self,
        bounds: &[Q; 4],
        work: &mut Work<'_>,
    ) -> Result<CountedVec<usize>, PredicateError> {
        let mut result = CountedVec::new();
        for id in self.x.overlapping(&bounds[0], &bounds[1], work)? {
            work.step()?;
            let b = &self.bounds[id];
            if b[2].try_cmp(&bounds[3])?.is_le() && b[3].try_cmp(&bounds[2])?.is_ge() {
                work.push(&mut result, id)?;
            }
        }
        Ok(result)
    }
    pub fn point_candidates(
        &self,
        p: &crate::noding::P,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<usize>, PredicateError> {
        let mut result = CountedVec::new();
        for id in self.x.candidates(&p.x, work)? {
            work.step()?;
            let b = &self.bounds[id];
            if b[2].try_cmp(&p.y)?.is_le() && b[3].try_cmp(&p.y)?.is_ge() {
                work.push(&mut result, id)?;
            }
        }
        Ok(result)
    }
    pub fn at(&self, id: usize) -> &[Q; 4] {
        &self.bounds[id]
    }
}

pub fn geometry_bounds(
    geometry: &geo::Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<[Q; 4], PredicateError> {
    use geo::CoordsIter;
    let mut bounds = [0.0_f64; 4];
    for (i, c) in geometry.coords_iter().enumerate() {
        work.step()?;
        if i == 0 {
            bounds = [c.x, c.x, c.y, c.y];
        } else {
            bounds = [
                bounds[0].min(c.x),
                bounds[1].max(c.x),
                bounds[2].min(c.y),
                bounds[3].max(c.y),
            ];
        }
    }
    let a = crate::noding::P::of_in((bounds[0], bounds[2]).into(), work)?;
    let b = crate::noding::P::of_in((bounds[1], bounds[3]).into(), work)?;
    Ok([a.x, b.x, a.y, b.y])
}
