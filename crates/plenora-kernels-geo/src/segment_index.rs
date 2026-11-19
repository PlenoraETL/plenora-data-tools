//! Gerarchia bilanciata di inviluppi razionali chiusi, senza arrotondamenti.
//! Ordinamento iniziale O(n log n), costruzione O(n), memoria O(n).
//! Query O(visite + k log k), fino a O(n log n); nessun candidato
//! viene eliminato in base a distanze floating point.

use crate::noding::{sorted, CountedVec, Edge, Work, P, Q};
use crate::predicates::PredicateError;

struct Node {
    bounds: [usize; 4],
    children: Option<(usize, usize)>,
    item: usize,
}

#[derive(Default)]
pub struct SegmentIndex {
    bounds: CountedVec<[Q; 4]>,
    nodes: CountedVec<Node>,
    root: Option<usize>,
}

impl SegmentIndex {
    pub fn new(bounds: CountedVec<[Q; 4]>, work: &mut Work<'_>) -> Result<Self, PredicateError> {
        let mut ids = CountedVec::new();
        for id in 0..bounds.len() {
            work.push(&mut ids, id)?;
        }
        sorted(
            &mut ids,
            |a, b| {
                Ok(bounds[*a][0]
                    .try_cmp(&bounds[*b][0])?
                    .then(bounds[*a][2].try_cmp(&bounds[*b][2])?)
                    .then(a.cmp(b)))
            },
            work,
        )?;
        let mut index = Self {
            bounds,
            nodes: CountedVec::new(),
            root: None,
        };
        if !ids.is_empty() {
            index.root = Some(index.build(&ids, work)?);
        }
        Ok(index)
    }

    pub fn edges<'a>(
        edges: impl Iterator<Item = &'a Edge>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        let mut bounds = CountedVec::new();
        for e in edges {
            work.step()?;
            let (x0, x1) = if e.a.x.try_cmp(&e.b.x)?.is_le() {
                (&e.a.x, &e.b.x)
            } else {
                (&e.b.x, &e.a.x)
            };
            let (y0, y1) = if e.a.y.try_cmp(&e.b.y)?.is_le() {
                (&e.a.y, &e.b.y)
            } else {
                (&e.b.y, &e.a.y)
            };
            work.push(
                &mut bounds,
                [
                    x0.try_clone()?,
                    x1.try_clone()?,
                    y0.try_clone()?,
                    y1.try_clone()?,
                ],
            )?;
        }
        Self::new(bounds, work)
    }

    pub fn points<'a>(
        points: impl Iterator<Item = &'a P>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        let mut bounds = CountedVec::new();
        for p in points {
            work.step()?;
            work.push(
                &mut bounds,
                [
                    p.x.try_clone()?,
                    p.x.try_clone()?,
                    p.y.try_clone()?,
                    p.y.try_clone()?,
                ],
            )?;
        }
        Self::new(bounds, work)
    }

    fn build(&mut self, ids: &[usize], work: &mut Work<'_>) -> Result<usize, PredicateError> {
        work.step()?;
        let (bounds, children) = if ids.len() == 1 {
            ([ids[0]; 4], None)
        } else {
            let (a, b) = ids.split_at(ids.len() / 2);
            let left = self.build(a, work)?;
            let right = self.build(b, work)?;
            let mut bounds = self.nodes[left].bounds;
            for (i, value) in self.nodes[right].bounds.iter().enumerate() {
                let order = self.bounds[*value][i].try_cmp(&self.bounds[bounds[i]][i])?;
                if (i % 2 == 0 && order.is_lt()) || (i % 2 == 1 && order.is_gt()) {
                    bounds[i] = *value;
                }
            }
            (bounds, Some((left, right)))
        };
        let id = self.nodes.len();
        work.push(
            &mut self.nodes,
            Node {
                bounds,
                children,
                item: ids[0],
            },
        )?;
        Ok(id)
    }

    fn query(
        &self,
        accept: impl Fn([&Q; 4]) -> Result<bool, PredicateError>,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<usize>, PredicateError> {
        let mut result = CountedVec::new();
        let mut pending = CountedVec::new();
        if let Some(root) = self.root {
            work.push(&mut pending, root)?;
        }
        while let Some(id) = pending.pop() {
            work.step()?;
            let node = &self.nodes[id];
            if !accept(std::array::from_fn(|axis| {
                &self.bounds[node.bounds[axis]][axis]
            }))? {
                continue;
            }
            if let Some((left, right)) = node.children {
                work.push(&mut pending, right)?;
                work.push(&mut pending, left)?;
            } else {
                work.push(&mut result, node.item)?;
            }
        }
        // ID in ordine d'ingresso, indipendenti dalla forma dell'albero.
        sorted(&mut result, |a, b| Ok(a.cmp(b)), work)?;
        Ok(result)
    }

    pub fn candidates(
        &self,
        b: &[Q; 4],
        work: &mut Work<'_>,
    ) -> Result<CountedVec<usize>, PredicateError> {
        self.query(
            |a| {
                Ok(a[0].try_cmp(&b[1])?.is_le()
                    && a[1].try_cmp(&b[0])?.is_ge()
                    && a[2].try_cmp(&b[3])?.is_le()
                    && a[3].try_cmp(&b[2])?.is_ge())
            },
            work,
        )
    }

    pub fn point_candidates(
        &self,
        p: &P,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<usize>, PredicateError> {
        self.query(
            |a| {
                Ok(a[0].try_cmp(&p.x)?.is_le()
                    && a[1].try_cmp(&p.x)?.is_ge()
                    && a[2].try_cmp(&p.y)?.is_le()
                    && a[3].try_cmp(&p.y)?.is_ge())
            },
            work,
        )
    }

    /// Raggio orizzontale chiuso verso destra. L'inclusione del bordo
    /// conserva anche le perturbazioni infinitesime della classificazione.
    pub fn ray_candidates(
        &self,
        p: &P,
        work: &mut Work<'_>,
    ) -> Result<CountedVec<usize>, PredicateError> {
        self.query(
            |a| {
                Ok(a[1].try_cmp(&p.x)?.is_ge()
                    && a[2].try_cmp(&p.y)?.is_le()
                    && a[3].try_cmp(&p.y)?.is_ge())
            },
            work,
        )
    }

    pub fn at(&self, id: usize) -> &[Q; 4] {
        &self.bounds[id]
    }
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::predicates::RelateLimits;
    use proptest::prelude::*;

    fn point(x: f64, y: f64, work: &Work<'_>) -> P {
        P::of_in((x, y).into(), work).expect("coordinate finite")
    }

    fn check(lines: &[(f64, f64, f64, f64)], queries: &[(f64, f64)]) {
        let mut work = Work::new(RelateLimits::default(), None);
        let mut edges = CountedVec::new();
        for &(x, y, u, v) in lines {
            let e = Edge {
                a: point(x, y, &work),
                b: point(u, v, &work),
            };
            work.push(&mut edges, e).expect("edge");
        }
        let index = SegmentIndex::edges(edges.iter(), &mut work).expect("index");
        for &(x, y) in queries {
            let p = point(x, y, &work);
            let candidates = index.point_candidates(&p, &mut work).expect("point query");
            let actual: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|&i| edges[i].contains(&p).expect("exact"))
                .collect();
            let expected: Vec<_> = edges
                .iter()
                .enumerate()
                .filter_map(|(i, e)| e.contains(&p).expect("exact").then_some(i))
                .collect();
            assert_eq!(actual, expected);
            let rays: Vec<_> = index
                .ray_candidates(&p, &mut work)
                .expect("ray query")
                .into_iter()
                .collect();
            let expected: Vec<_> = (0..lines.len())
                .filter(|&i| {
                    let b = index.at(i);
                    b[1].try_cmp(&p.x).expect("x").is_ge()
                        && b[2].try_cmp(&p.y).expect("y").is_le()
                        && b[3].try_cmp(&p.y).expect("y").is_ge()
                })
                .collect();
            assert_eq!(rays, expected);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        #[test]
        fn candidates_match_exhaustive(
            lines in prop::collection::vec((-30_i32..30, -30_i32..30, -30_i32..30, -30_i32..30), 0..48),
            queries in prop::collection::vec((-32_i32..32, -32_i32..32), 0..24)
        ) {
            check(&lines.into_iter().map(|(x,y,u,v)| (f64::from(x), f64::from(y), f64::from(u), f64::from(v))).collect::<Vec<_>>(),
                &queries.into_iter().map(|(x,y)| (f64::from(x), f64::from(y))).collect::<Vec<_>>());
        }
    }

    #[test]
    fn collinear_overlapping_extreme_and_subnormal() {
        for scale in [1.0, 1e300, 1e-300, f64::from_bits(1)] {
            check(
                &[
                    (0., 0., scale, scale),
                    (scale, scale, 0., 0.),
                    (0., 0., 2. * scale, 2. * scale),
                    (scale, scale, scale, scale),
                    (-scale, scale, scale, -scale),
                ],
                &[
                    (0., 0.),
                    (scale, scale),
                    (2. * scale, 2. * scale),
                    (0., scale),
                    (-scale, scale),
                ],
            );
        }
    }

    #[test]
    fn sparse_vertical_queries_scale_and_limits() {
        let mut previous = None;
        for count in [1024, 2048, 4096] {
            let mut work = Work::new(RelateLimits::default(), None);
            let mut edges = CountedVec::new();
            for i in 0..count {
                let y = f64::from(i) * 4.;
                let e = Edge {
                    a: point(0., y, &work),
                    b: point(1., y + 1., &work),
                };
                work.push(&mut edges, e).expect("edge");
            }
            let index = SegmentIndex::edges(edges.iter(), &mut work).expect("index");
            for e in &edges {
                assert_eq!(
                    index
                        .point_candidates(&e.a, &mut work)
                        .expect("query")
                        .len(),
                    1
                );
            }
            println!(
                "segment-index n={count} steps={} peak={}",
                work.steps,
                work.peak_bytes()
            );
            if let Some(before) = previous {
                assert!(work.steps < before * 5 / 2);
            }
            previous = Some(work.steps);
        }
        let mut work = Work::new(
            RelateLimits {
                max_steps: 1,
                ..RelateLimits::default()
            },
            None,
        );
        assert!(matches!(
            SegmentIndex::points(std::iter::once(&point(0., 0., &work)), &mut work),
            Err(PredicateError::WorkLimit)
        ));
        let mut work = Work::new(
            RelateLimits {
                max_reserved_bytes: 1,
                ..RelateLimits::default()
            },
            None,
        );
        assert!(matches!(
            work.integer(0),
            Err(PredicateError::ReservationLimit)
        ));
        assert_eq!(
            SegmentIndex::new(CountedVec::new(), &mut work)
                .expect("indice vuoto")
                .root,
            None
        );
    }
}
