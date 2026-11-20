//! Noding razionale comune: inviluppi esatti, coppie candidate e nodi.
//!
//! Sweep sugli estremi X ordinati: il filtro inclusivo X/Y non perde
//! contatti e non converte mai un razionale in floating point.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use crate::predicates::{PredicateError, RelateLimits};
use geo::Coord;
use std::cmp::Ordering;
use std::rc::Rc;

pub mod bounds;
mod exact;
pub mod faces;
#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod oracle;
mod storage;
pub use storage::CountedVec;

/// GOP-028: un solo ordine di osservazione per tutti i checkpoint.
pub fn check_interruption<E>(
    deadline: Option<std::time::Instant>,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
    deadline_error: E,
    cancelled_error: E,
) -> Result<(), E> {
    #[cfg(test)]
    let deadline = gop028::checkpoint(deadline, cancelled);
    plenora_core::tempo::verifica_interruzione(
        deadline,
        cancelled,
        || deadline_error,
        || cancelled_error,
    )
}

/// Checkpoint esterni entro 256 primitive Q; ogni gcd ha meno di
/// 290346 resti su interi di al massimo 145173 bit e controlla la deadline
/// a ogni resto. Il flag e' osservato dal checkpoint esterno.
pub struct Work<'a> {
    limits: RelateLimits,
    cancelled: Option<&'a std::sync::atomic::AtomicBool>,
    pub steps: u64,
    pub pairs: u64,
    budget: Rc<storage::Budget>,
    pub nodes: u64,
    pub polygon_counts: [u64; 3],
    exports: CountedVec<storage::Lease>,
    #[cfg(test)]
    pub interrupt_at: Option<(u64, bool)>,
    #[cfg(test)]
    pub polygon_active: bool,
}
impl<'a> Work<'a> {
    pub fn new(limits: RelateLimits, cancelled: Option<&'a std::sync::atomic::AtomicBool>) -> Self {
        let mut budget = storage::Budget::new(limits.max_reserved_bytes);
        budget.deadline = limits.deadline;
        Self {
            limits,
            cancelled,
            steps: 0,
            pairs: 0,
            budget: Rc::new(budget),
            nodes: 0,
            polygon_counts: [0; 3],
            exports: CountedVec::new(),
            #[cfg(test)]
            interrupt_at: None,
            #[cfg(test)]
            polygon_active: false,
        }
    }
    /// Prenota un buffer esportato prima della sua allocazione.
    /// La ricevuta resta viva fino al termine dell'intera operazione.
    pub fn reserve_export(&mut self, bytes: u64) -> Result<(), PredicateError> {
        self.step()?;
        let lease = storage::Lease::created(&self.budget, bytes)?;
        self.exports.push(lease, &self.budget)
    }

    pub fn integer(&self, value: i32) -> Result<Q, PredicateError> {
        Q::int_in(value, &self.budget)
    }

    pub fn step(&mut self) -> Result<(), PredicateError> {
        #[cfg(test)]
        if let Some((at, deadline)) = self.interrupt_at {
            if self.steps == at {
                if deadline {
                    self.limits.deadline = Some(std::time::Instant::now());
                } else if let Some(flag) = self.cancelled {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        crate::noding::check_interruption(
            self.limits.deadline,
            self.cancelled,
            PredicateError::DeadlineExceeded,
            PredicateError::Cancelled,
        )?;
        self.steps = self
            .steps
            .checked_add(1)
            .filter(|n| *n <= self.limits.max_steps)
            .ok_or(PredicateError::WorkLimit)?;
        Ok(())
    }
    pub fn pair(&mut self) -> Result<(), PredicateError> {
        self.step()?;
        self.pairs = self
            .pairs
            .checked_add(1)
            .filter(|n| *n <= self.limits.max_segment_pairs)
            .ok_or(PredicateError::WorkLimit)?;
        Ok(())
    }
    pub fn nodes(&mut self, count: u64) -> Result<(), PredicateError> {
        self.step()?;
        self.nodes = self
            .nodes
            .checked_add(count)
            .filter(|n| *n <= self.limits.max_nodes)
            .ok_or(PredicateError::WorkLimit)?;
        Ok(())
    }
    pub fn peak_bytes(&self) -> u64 {
        self.budget.peak()
    }

    pub fn push<T>(&mut self, vector: &mut CountedVec<T>, value: T) -> Result<(), PredicateError> {
        self.step()?;
        vector.push(value, &self.budget)
    }
}

pub fn sorted<T>(
    values: &mut [T],
    compare: impl Fn(&T, &T) -> Result<Ordering, PredicateError>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    sorted_with_work(values, |a, b, _| compare(a, b), work)
}
pub fn sorted_with_work<T>(
    values: &mut [T],
    compare: impl Fn(&T, &T, &mut Work<'_>) -> Result<Ordering, PredicateError>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    fn sift<T>(
        values: &mut [T],
        mut root: usize,
        compare: &impl Fn(&T, &T, &mut Work<'_>) -> Result<Ordering, PredicateError>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        while root < values.len() / 2 {
            let mut child = root * 2 + 1;
            if child + 1 < values.len() {
                work.step()?;
                if compare(&values[child], &values[child + 1], work)?.is_lt() {
                    child += 1;
                }
            }
            work.step()?;
            if !compare(&values[root], &values[child], work)?.is_lt() {
                break;
            }
            values.swap(root, child);
            root = child;
        }
        Ok(())
    }
    for root in (0..values.len() / 2).rev() {
        sift(values, root, &compare, work)?;
    }
    for end in (1..values.len()).rev() {
        work.step()?;
        values.swap(0, end);
        sift(&mut values[..end], 0, &compare, work)?;
    }
    Ok(())
}

pub use exact::{sum_sign, Q};

#[derive(Debug, Eq, PartialEq)]
pub struct P {
    pub x: Q,
    pub y: Q,
}
impl P {
    pub fn of_in(p: Coord<f64>, work: &Work<'_>) -> Result<Self, PredicateError> {
        Ok(Self {
            x: Q::float_in(p.x, &work.budget)?,
            y: Q::float_in(p.y, &work.budget)?,
        })
    }
    pub fn try_clone(&self) -> Result<Self, PredicateError> {
        Ok(Self {
            x: self.x.try_clone()?,
            y: self.y.try_clone()?,
        })
    }
    pub fn try_sub(&self, b: &Self) -> Result<Self, PredicateError> {
        Ok(Self {
            x: self.x.try_sub(&b.x)?,
            y: self.y.try_sub(&b.y)?,
        })
    }
    pub fn try_add(&self, b: &Self) -> Result<Self, PredicateError> {
        Ok(Self {
            x: self.x.try_add(&b.x)?,
            y: self.y.try_add(&b.y)?,
        })
    }
    pub fn try_cross(&self, b: &Self) -> Result<Q, PredicateError> {
        self.x.try_mul(&b.y)?.try_sub(&self.y.try_mul(&b.x)?)
    }
}

pub struct Edge {
    pub a: P,
    pub b: P,
}
impl Edge {
    pub fn at_x(&self, x: &Q) -> Result<Q, PredicateError> {
        self.a.y.try_add(
            &self
                .b
                .y
                .try_sub(&self.a.y)?
                .try_mul(&x.try_sub(&self.a.x)?)?
                .try_div(&self.b.x.try_sub(&self.a.x)?)?,
        )
    }
    pub fn contains(&self, p: &P) -> Result<bool, PredicateError> {
        Ok(between(&p.x, &self.a.x, &self.b.x)?
            && between(&p.y, &self.a.y, &self.b.y)?
            && self
                .b
                .try_sub(&self.a)?
                .try_cross(&p.try_sub(&self.a)?)?
                .sign()
                == Ordering::Equal)
    }
}
pub fn between(x: &Q, a: &Q, b: &Q) -> Result<bool, PredicateError> {
    Ok((!a.try_cmp(x)?.is_gt() && !x.try_cmp(b)?.is_gt())
        || (!b.try_cmp(x)?.is_gt() && !x.try_cmp(a)?.is_gt()))
}

/// Intersezione parametrica esatta; gli overlap restituiscono i due estremi.
fn intersections(
    a: &Edge,
    b: &Edge,
    work: &mut Work<'_>,
) -> Result<[Option<P>; 2], PredicateError> {
    let r = a.b.try_sub(&a.a)?;
    let s = b.b.try_sub(&b.a)?;
    let denominator = r.try_cross(&s)?;
    if denominator.sign() != Ordering::Equal {
        let offset = b.a.try_sub(&a.a)?;
        let t = offset.try_cross(&s)?.try_div(&denominator)?;
        let u = offset.try_cross(&r)?.try_div(&denominator)?;
        if between(&t, &work.integer(0)?, &work.integer(1)?)?
            && between(&u, &work.integer(0)?, &work.integer(1)?)?
        {
            work.nodes(2)?;
            return Ok([
                Some(P {
                    x: a.a.x.try_add(&r.x.try_mul(&t)?)?,
                    y: a.a.y.try_add(&r.y.try_mul(&t)?)?,
                }),
                None,
            ]);
        }
        return Ok([None, None]);
    }
    let mut first = None;
    let mut last = None;
    for p in [&a.a, &a.b, &b.a, &b.b] {
        if a.contains(p)? && b.contains(p)? {
            if match first {
                None => true,
                Some(q) => point_cmp(p, q)?.is_lt(),
            } {
                first = Some(p);
            }
            if match last {
                None => true,
                Some(q) => point_cmp(p, q)?.is_gt(),
            } {
                last = Some(p);
            }
        }
    }
    if first == last {
        last = None;
    }
    work.nodes(2 * (u64::from(first.is_some()) + u64::from(last.is_some())))?;
    Ok([
        first.map(P::try_clone).transpose()?,
        last.map(P::try_clone).transpose()?,
    ])
}

fn point_cmp(a: &P, b: &P) -> Result<Ordering, PredicateError> {
    let x = a.x.try_cmp(&b.x)?;
    if x.is_eq() {
        a.y.try_cmp(&b.y)
    } else {
        Ok(x)
    }
}
fn index_cmp(a: usize, b: usize, edges: &[&Edge]) -> Result<Ordering, PredicateError> {
    Ok(edges[a]
        .a
        .x
        .min_ref(&edges[a].b.x)?
        .try_cmp(edges[b].a.x.min_ref(&edges[b].b.x)?)?
        .then_with(|| a.cmp(&b)))
}
impl Q {
    fn min_ref<'a>(&'a self, other: &'a Self) -> Result<&'a Self, PredicateError> {
        Ok(if self.try_cmp(other)?.is_le() {
            self
        } else {
            other
        })
    }
    fn max_ref<'a>(&'a self, other: &'a Self) -> Result<&'a Self, PredicateError> {
        Ok(if self.try_cmp(other)?.is_ge() {
            self
        } else {
            other
        })
    }
}

// Heapsort in place: ogni confronto ha un checkpoint, senza scratch heap.
fn sift(
    index: &mut [usize],
    mut root: usize,
    edges: &[&Edge],
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    while root < index.len() / 2 {
        work.step()?;
        let mut child = root * 2 + 1;
        if child + 1 < index.len() && index_cmp(index[child], index[child + 1], edges)?.is_lt() {
            child += 1;
        }
        if !index_cmp(index[root], index[child], edges)?.is_lt() {
            break;
        }
        index.swap(root, child);
        root = child;
    }
    Ok(())
}

/// Ogni test esatto dopo entrambi i filtri costa una coppia. Le visite
/// degli inviluppi consumano passi, mai coppie. L'ordine ha spareggio per id.
pub fn visit(
    edges: &[&Edge],
    work: &mut Work<'_>,
    mut emit: impl FnMut(usize, usize, [Option<P>; 2], &mut Work<'_>) -> Result<(), PredicateError>,
) -> Result<(), PredicateError> {
    work.step()?;
    let mut index = CountedVec::new();
    for (i, edge) in edges.iter().enumerate() {
        work.step()?;
        for point in [&edge.a, &edge.b] {
            if !point.x.belongs_to(&work.budget) || !point.y.belongs_to(&work.budget) {
                return Err(PredicateError::CalcoloNonConcluso(
                    "proprietario del budget",
                ));
            }
        }
        work.push(&mut index, i)?;
    }
    for root in (0..index.len() / 2).rev() {
        sift(&mut index, root, edges, work)?;
    }
    for end in (1..index.len()).rev() {
        work.step()?;
        index.swap(0, end);
        sift(&mut index[..end], 0, edges, work)?;
    }
    for (position, &i) in index.iter().enumerate() {
        let a = edges[i];
        for &j in &index[position + 1..] {
            work.step()?;
            let b = edges[j];
            if b.a
                .x
                .min_ref(&b.b.x)?
                .try_cmp(a.a.x.max_ref(&a.b.x)?)?
                .is_gt()
            {
                break;
            }
            if b.a
                .y
                .min_ref(&b.b.y)?
                .try_cmp(a.a.y.max_ref(&a.b.y)?)?
                .is_gt()
                || b.a
                    .y
                    .max_ref(&b.b.y)?
                    .try_cmp(a.a.y.min_ref(&a.b.y)?)?
                    .is_lt()
            {
                continue;
            }
            work.pair()?;
            let hits = intersections(a, b, work)?;
            emit(i, j, hits, work)?;
        }
    }
    Ok(())
}

/// Vertici ordinati lungo ciascun segmento, duplicati rimossi esattamente.
/// Gli id dei segmenti mantengono la provenienza anche per lati coincidenti.
pub fn node(
    edges: &[&Edge],
    work: &mut Work<'_>,
) -> Result<CountedVec<CountedVec<P>>, PredicateError> {
    work.step()?;
    let mut result = CountedVec::new();
    for edge in edges {
        let mut points = CountedVec::new();
        work.nodes(2)?;
        work.push(&mut points, edge.a.try_clone()?)?;
        work.push(&mut points, edge.b.try_clone()?)?;
        work.push(&mut result, points)?;
    }
    visit(edges, work, |i, j, hits, work| {
        for p in hits.into_iter().flatten() {
            work.push(&mut result[i], p.try_clone()?)?;
            work.push(&mut result[j], p)?;
        }
        Ok(())
    })?;
    for (points, edge) in result.iter_mut().zip(edges) {
        sorted(points, point_cmp, work)?;
        let mut kept = 0;
        for read in 0..points.len() {
            work.step()?;
            if kept == 0 || points[read] != points[kept - 1] {
                points.swap(kept, read);
                kept += 1;
            }
        }
        while points.len() > kept {
            work.step()?;
            points.pop();
        }
        if point_cmp(&edge.a, &edge.b)?.is_gt() {
            for i in 0..points.len() / 2 {
                work.step()?;
                let other = points.len() - 1 - i;
                points.swap(i, other);
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::vec::Vec;

    #[test]
    fn heapsort_matches_generic_with_duplicates() {
        for count in [0, 1, 2, 16, 64, 511] {
            let mut actual: Vec<_> = (0..count).map(|i| (i * 37 + 11) % 23).collect();
            let mut expected = actual.clone();
            expected.sort_unstable();
            let mut work = Work::new(RelateLimits::default(), None);
            sorted(&mut actual, |a, b| Ok(a.cmp(b)), &mut work).expect("ordinamento");
            assert_eq!(actual, expected);
        }
    }

    fn q(n: i32) -> Q {
        Work::new(RelateLimits::default(), None)
            .integer(n)
            .expect("intero")
    }

    fn edge(a: (f64, f64), b: (f64, f64)) -> geo::Line<f64> {
        geo::Line::new(a, b)
    }

    fn inputs(
        edges: &[&geo::Line<f64>],
        work: &mut Work<'_>,
    ) -> Result<CountedVec<Edge>, PredicateError> {
        let mut result = CountedVec::new();
        for edge in edges {
            let value = Edge {
                a: P::of_in(edge.start, work)?,
                b: P::of_in(edge.end, work)?,
            };
            work.push(&mut result, value)?;
        }
        Ok(result)
    }

    fn node(
        edges: &[&geo::Line<f64>],
        work: &mut Work<'_>,
    ) -> Result<CountedVec<CountedVec<P>>, PredicateError> {
        let edges = inputs(edges, work)?;
        super::node(&edges.iter().collect::<Vec<_>>(), work)
    }

    fn visit(
        edges: &[&geo::Line<f64>],
        work: &mut Work<'_>,
        emit: impl FnMut(usize, usize, [Option<P>; 2], &mut Work<'_>) -> Result<(), PredicateError>,
    ) -> Result<(), PredicateError> {
        let edges = inputs(edges, work)?;
        super::visit(&edges.iter().collect::<Vec<_>>(), work, emit)
    }

    fn intersections(
        a: &geo::Line<f64>,
        b: &geo::Line<f64>,
        work: &mut Work<'_>,
    ) -> Result<[Option<P>; 2], PredicateError> {
        let edges = inputs(&[a, b], work)?;
        super::intersections(&edges[0], &edges[1], work)
    }

    /// O-M1: y=x e y=1-2x hanno unico incrocio (1/3,1/3).
    /// SFA 1.2.1 6.1.7: gli estremi dell'overlap dividono entrambi i lati.
    #[test]
    fn rational_crossing_and_collinear_endpoints() {
        let edges = [edge((0., 0.), (1., 1.)), edge((0., 1.), (1., -1.))];
        let mut work = Work::new(RelateLimits::default(), None);
        let nodes = node(&edges.iter().collect::<Vec<_>>(), &mut work).expect("noding");
        let third = work
            .integer(1)
            .expect("uno")
            .try_div(&work.integer(3).expect("tre"))
            .expect("terzo");
        assert_eq!(
            nodes[0][1],
            P {
                x: third.try_clone().expect("copia"),
                y: third
            }
        );
        assert_eq!(nodes[0][1], nodes[1][1]);
        assert_eq!((work.pairs, work.nodes), (1, 6));
        let edges = [edge((0., 0.), (3., 0.)), edge((2., 0.), (1., 0.))];
        let nodes = node(&edges.iter().collect::<Vec<_>>(), &mut work).expect("overlap");
        assert_eq!(
            nodes[0]
                .iter()
                .map(|p| p.x.try_clone().expect("copia"))
                .collect::<Vec<_>>(),
            [0, 1, 2, 3].map(q)
        );
        assert_eq!(
            nodes[1]
                .iter()
                .map(|p| p.x.try_clone().expect("copia"))
                .collect::<Vec<_>>(),
            [2, 1].map(q)
        );
    }

    /// O-M1/M3: n orizzontali interne e m verticali interne, nessun
    /// endpoint condiviso: nm incroci, nm coppie, 2(n+m)+2nm incidenze.
    #[test]
    fn grids_count_exact_pairs_and_segment_node_incidences() {
        for (n, m) in [(3, 5), (6, 10), (12, 20)] {
            let mut edges = Vec::new();
            for y in 1..=n {
                edges.push(edge((0., f64::from(y)), (f64::from(m + 1), f64::from(y))));
            }
            for x in 1..=m {
                edges.push(edge((f64::from(x), 0.), (f64::from(x), f64::from(n + 1))));
            }
            let mut work = Work::new(RelateLimits::default(), None);
            node(&edges.iter().collect::<Vec<_>>(), &mut work).expect("griglia");
            assert_eq!(work.pairs, u64::try_from(n * m).expect("piccolo"));
            assert_eq!(
                work.nodes,
                u64::try_from(2 * (n + m) + 2 * n * m).expect("piccolo")
            );
        }
    }

    /// O-M1/M3: pettine con n denti interni al dorso: n coppie e
    /// 2(n+1)+2n incidenze. Strisce orizzontali con X sovrapposte:
    /// il filtro Y elimina tutte le coppie, restano solo 2n estremi.
    #[test]
    fn combs_and_stripes_have_analytic_counts() {
        for n in [4, 16, 64] {
            let mut comb = vec![edge((0., 0.), (f64::from(n + 1), 0.))];
            for x in 1..=n {
                comb.push(edge((f64::from(x), 0.), (f64::from(x), 1.)));
            }
            let mut work = Work::new(RelateLimits::default(), None);
            node(&comb.iter().collect::<Vec<_>>(), &mut work).expect("pettine");
            assert_eq!(work.pairs, u64::try_from(n).expect("piccolo"));
            assert_eq!(work.nodes, u64::try_from(4 * n + 2).expect("piccolo"));

            let stripes: Vec<_> = (0..n)
                .map(|y| edge((0., f64::from(y)), (100., f64::from(y))))
                .collect();
            let mut work = Work::new(
                RelateLimits {
                    max_segment_pairs: 0,
                    ..RelateLimits::default()
                },
                None,
            );
            node(&stripes.iter().collect::<Vec<_>>(), &mut work).expect("strisce");
            assert_eq!(work.pairs, 0);
            assert_eq!(work.nodes, u64::try_from(2 * n).expect("piccolo"));
        }
    }

    /// O-M3: 20 002 segmenti disgiunti, prodotto maggiore di 10^8;
    /// zero rettangoli sovrapposti, quindi zero test esatti e 40 004 estremi.
    #[test]
    fn large_sparse_case_has_no_product_admission() {
        let edges: Vec<_> = (0..20_002)
            .map(|i| {
                let x = f64::from(i) * 3.;
                edge((x, 0.), (x + 1., 1.))
            })
            .collect();
        let mut work = Work::new(
            RelateLimits {
                max_segment_pairs: 0,
                ..RelateLimits::default()
            },
            None,
        );
        node(&edges.iter().collect::<Vec<_>>(), &mut work).expect("sparse");
        assert_eq!(work.pairs, 0);
        assert_eq!(work.nodes, 40_004);
    }

    /// GOP-026: limite esatto ammesso, prossimo test rifiutato dopo
    /// lavoro reale. GOP-028: token alzato dopo il primo test esatto.
    #[test]
    fn work_limit_and_cancellation_follow_actual_progress() {
        let edges = [
            edge((0., 0.), (3., 0.)),
            edge((1., -1.), (1., 1.)),
            edge((2., -1.), (2., 1.)),
        ];
        let refs = edges.iter().collect::<Vec<_>>();
        let mut work = Work::new(
            RelateLimits {
                max_segment_pairs: 1,
                ..RelateLimits::default()
            },
            None,
        );
        let mut visited = 0;
        assert!(matches!(
            visit(&refs, &mut work, |_, _, _, _| {
                visited += 1;
                Ok(())
            }),
            Err(PredicateError::WorkLimit)
        ));
        assert_eq!(visited, 1);
        assert_eq!(work.pairs, 1);
        let flag = AtomicBool::new(false);
        let mut work = Work::new(RelateLimits::default(), Some(&flag));
        assert!(matches!(
            visit(&refs, &mut work, |_, _, _, _| {
                flag.store(true, AtomicOrdering::Relaxed);
                Ok(())
            }),
            Err(PredicateError::Cancelled)
        ));
        assert_eq!(work.pairs, 1);
    }

    /// D-b/GOP-027: due estremi coincidenti restano incidenze distinte
    /// prima della deduplicazione. Il prossimo evento non viene emesso.
    #[test]
    fn duplicate_nodes_are_limited_before_emission() {
        let edges = [edge((0., 0.), (3., 0.)), edge((2., 0.), (1., 0.))];
        let refs = edges.iter().collect::<Vec<_>>();
        let mut exact = Work::new(
            RelateLimits {
                max_nodes: 8,
                ..RelateLimits::default()
            },
            None,
        );
        let result = node(&refs, &mut exact).expect("otto incidenze prima di dedup");
        assert_eq!(exact.nodes, 8);
        assert_eq!(result.iter().map(|points| points.len()).sum::<usize>(), 6);
        let mut short = Work::new(
            RelateLimits {
                max_nodes: 7,
                ..RelateLimits::default()
            },
            None,
        );
        assert!(matches!(
            node(&refs, &mut short),
            Err(PredicateError::WorkLimit)
        ));
        assert_eq!(short.nodes, 4);
        let mut empty = Work::new(
            RelateLimits {
                max_nodes: 0,
                ..RelateLimits::default()
            },
            None,
        );
        assert!(matches!(
            node(&refs, &mut empty),
            Err(PredicateError::WorkLimit)
        ));
        assert!(empty.peak_bytes() > 0);
    }

    /// GOP-028: anche l'ingresso vuoto controlla deadline e token. Nessun
    /// sleep o dipendenza dalla velocita' della macchina nella prova.
    #[test]
    fn deadline_is_checked_next_to_cancellation() {
        let limits = RelateLimits {
            deadline: Some(std::time::Instant::now()),
            ..RelateLimits::default()
        };
        let mut expired = Work::new(limits, None);
        assert!(matches!(
            visit(&[], &mut expired, |_, _, _, _| Ok(())),
            Err(PredicateError::DeadlineExceeded)
        ));
        assert_eq!(expired.steps, 0);
        let flag = AtomicBool::new(true);
        let mut cancelled = Work::new(limits, Some(&flag));
        assert!(matches!(
            cancelled.step(),
            Err(PredicateError::DeadlineExceeded)
        ));
    }

    /// GOP-026/027 e O-M1/M3: reticolo 3x3, pettine, strisce e segmenti
    /// sparsi. Fonte: formule di incidenza documentate nelle prove sopra.
    /// La soglia viene misurata sul percorso completo, inclusa l'aritmetica;
    /// a soglia e soglia+1 passa, a soglia-1 fallisce senza ricevute residue.
    #[test]
    fn noding_live_arithmetic_and_exact_resource_thresholds() {
        let grid: Vec<_> = (1..=3)
            .flat_map(|i| {
                let v = f64::from(i);
                [edge((0., v), (4., v)), edge((v, 0.), (v, 4.))]
            })
            .collect();
        let comb: Vec<_> = std::iter::once(edge((0., 0.), (5., 0.)))
            .chain((1..=4).map(|i| edge((f64::from(i), 0.), (f64::from(i), 1.))))
            .collect();
        let stripes: Vec<_> = (0..8)
            .map(|i| edge((0., f64::from(i)), (8., f64::from(i))))
            .collect();
        let sparse: Vec<_> = (0..64)
            .map(|i| {
                let v = 3. * f64::from(i);
                edge((v, 0.), (v + 1., 1.))
            })
            .collect();
        for edges in [grid, comb, stripes, sparse] {
            let refs = edges.iter().collect::<Vec<_>>();
            let mut measured = Work::new(RelateLimits::default(), None);
            let result = node(&refs, &mut measured).expect("misura");
            let peak = measured.peak_bytes();
            let structural_output = std::mem::size_of::<CountedVec<P>>() * result.len()
                + result
                    .iter()
                    .map(|points| std::mem::size_of::<P>() * points.len())
                    .sum::<usize>();
            assert!(measured.budget.live() > u64::try_from(structural_output).expect("dimensione"));
            drop(result);
            assert_eq!(measured.budget.live(), 0);
            for bytes in [peak, peak + 1] {
                let mut work = Work::new(
                    RelateLimits {
                        max_reserved_bytes: bytes,
                        ..RelateLimits::default()
                    },
                    None,
                );
                drop(node(&refs, &mut work).expect("soglia"));
                assert_eq!(work.peak_bytes(), peak);
                assert_eq!(work.budget.live(), 0);
            }
            let mut short = Work::new(
                RelateLimits {
                    max_reserved_bytes: peak - 1,
                    ..RelateLimits::default()
                },
                None,
            );
            assert!(matches!(
                node(&refs, &mut short),
                Err(PredicateError::ReservationLimit)
            ));
            assert_eq!(short.budget.live(), 0);
            assert!(
                short.budget.attempted_peak().saturating_sub(peak - 1) <= exact::MAX_VALUE_BYTES
            );
            for (steps, nodes, succeeds) in [
                (measured.steps, measured.nodes, true),
                (measured.steps - 1, measured.nodes, false),
                (measured.steps, measured.nodes - 1, false),
            ] {
                let mut work = Work::new(
                    RelateLimits {
                        max_steps: steps,
                        max_nodes: nodes,
                        ..RelateLimits::default()
                    },
                    None,
                );
                let result = node(&refs, &mut work);
                if succeeds {
                    drop(result.expect("soglie esatte"));
                } else {
                    assert!(matches!(result, Err(PredicateError::WorkLimit)));
                }
                assert_eq!(work.budget.live(), 0);
            }
        }
    }

    /// GOP-027: un noder non puo' contabilizzare gli intermedi su un
    /// bilancio diverso da quello che ne governa lavoro e contenitori.
    #[test]
    fn foreign_edge_budget_is_rejected() {
        let owner = Work::new(RelateLimits::default(), None);
        let edge = Edge {
            a: P::of_in((0., 0.).into(), &owner).expect("a"),
            b: P::of_in((1., 1.).into(), &owner).expect("b"),
        };
        let mut other = Work::new(RelateLimits::default(), None);
        assert!(matches!(
            super::visit(&[&edge], &mut other, |_, _, _, _| Ok(())),
            Err(PredicateError::CalcoloNonConcluso(_))
        ));
        assert_eq!(other.budget.live(), 0);
    }

    /// AGENTS regola 3: l'indice contro l'enumerazione generica di tutte
    /// le coppie, input per input. Il kernel esatto resta lo stesso oracolo
    /// di ottimizzazione; le attese matematiche sono nei test separati.
    #[test]
    fn indexed_intersections_match_exhaustive_pairs() {
        let mut state = 0x5449_4231_u64;
        for _ in 0..64 {
            let mut next = || {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                f64::from(u32::try_from((state >> 32) % 11).expect("modulo")) - 5.
            };
            let edges: Vec<_> = (0..12)
                .map(|_| edge((next(), next()), (next(), next())))
                .collect();
            let mut exhaustive = Vec::new();
            for (i, a) in edges.iter().enumerate() {
                for (j, b) in edges.iter().enumerate().skip(i + 1) {
                    let hits = intersections(a, b, &mut Work::new(RelateLimits::default(), None))
                        .expect("oracolo esaustivo");
                    if hits[0].is_some() {
                        exhaustive.push((i, j, hits));
                    }
                }
            }
            let mut indexed = Vec::new();
            visit(
                &edges.iter().collect::<Vec<_>>(),
                &mut Work::new(RelateLimits::default(), None),
                |i, j, hits, _| {
                    if hits[0].is_some() {
                        indexed.push((i.min(j), i.max(j), hits));
                    }
                    Ok(())
                },
            )
            .expect("indice");
            indexed.sort_by_key(|(i, j, _)| (*i, *j));
            assert_eq!(indexed, exhaustive);
        }
    }
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod gop028 {
    use super::*;
    use crate::coordinate_api::*;
    use crate::geodesic_densify::*;
    use crate::geodesic_distance::*;
    use crate::preserve_shape_measure::*;
    use crate::{general_boolean, line_boolean, point_boolean, polygon_boolean};
    use geo::{Geometry, LineString, Point, Polygon};
    use std::cell::Cell;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::time::Instant;

    // Sonda locale al thread, assente in produzione. Simula il trascorrere
    // del tempo al checkpoint k e alza il vero token prima del controllo.
    thread_local! {
        static PROBE: Cell<Option<(usize, Option<usize>)>> = const { Cell::new(None) };
    }
    pub(super) fn checkpoint(
        deadline: Option<Instant>,
        flag: Option<&AtomicBool>,
    ) -> Option<Instant> {
        let Some(flag) = flag else {
            return deadline;
        };
        PROBE.with(|probe| {
            let Some((count, at)) = probe.get() else {
                return deadline;
            };
            probe.set(Some((count + 1, at)));
            if at == Some(count) {
                flag.store(true, AtomicOrdering::Relaxed);
                Some(Instant::now())
            } else {
                deadline
            }
        })
    }
    struct Probe;
    impl Probe {
        fn start(at: Option<usize>) -> Self {
            PROBE.with(|probe| {
                assert!(probe.get().is_none());
                probe.set(Some((0, at)));
            });
            Self
        }
        fn count() -> usize {
            probe_count()
        }
    }
    fn probe_count() -> usize {
        PROBE.with(|probe| probe.get().unwrap().0)
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            PROBE.with(|probe| probe.set(None));
        }
    }
    struct Fixture {
        line: LineString,
        line_geometry: Geometry,
        guide: Geometry,
        reshaper: Geometry,
        point: Geometry,
        polygon: Polygon,
        polygon_geometry: Geometry,
        crs: plenora_core::crs::ResolvedCrs,
        options: Vec<CoordinateOption>,
    }
    impl Fixture {
        fn new(reverse: bool) -> Self {
            let line = LineString::from(vec![(0., 0.), (10., 0.)]);
            let polygon = Polygon::new(
                LineString::from(vec![(0., 0.), (1., 0.), (1., 1.), (0., 1.), (0., 0.)]),
                vec![],
            );
            let mut options = vec![
                CoordinateOption::Precision(4),
                CoordinateOption::Datum(CoordinateDatum::Wgs84),
            ];
            if reverse {
                options.reverse();
            }
            Self {
                line_geometry: line.clone().into(),
                line,
                guide: LineString::from(vec![(2., 10.), (2., 11.)]).into(),
                reshaper: LineString::from(vec![(2., -2.), (2., 2.), (8., 2.), (8., -2.)]).into(),
                point: Point::new(0., 0.).into(),
                polygon_geometry: polygon.clone().into(),
                polygon,
                crs: plenora_core::crs::resolve_crs("EPSG:4326", "crs").unwrap(),
                options,
            }
        }
        fn request(&self) -> CoordinateRequest<'_> {
            CoordinateRequest {
                profile: STANDARD_V1,
                format: CoordinateFormat::Dd,
                options: &self.options,
            }
        }
    }
    fn limits(deadline: Option<Instant>) -> RelateLimits {
        RelateLimits {
            deadline,
            ..RelateLimits::default()
        }
    }
    fn polygons(deadline: Option<Instant>) -> polygon_boolean::PolygonBooleanLimits {
        polygon_boolean::PolygonBooleanLimits {
            work: limits(deadline),
            ..Default::default()
        }
    }
    fn densify(deadline: Option<Instant>) -> GeodesicDensifyLimits {
        GeodesicDensifyLimits {
            max_points: 100_000,
            max_reserved_bytes: 10_000_000,
            deadline,
        }
    }
    fn measure(deadline: Option<Instant>) -> ShapeMeasureLimits {
        ShapeMeasureLimits {
            max_work: 1_000_000,
            deadline,
        }
    }
    fn tolerance() -> ShapeTolerance {
        ShapeTolerance {
            absolute: 1.,
            relative: 0.0001,
        }
    }
    struct Case {
        source: &'static str,
        name: &'static str,
        run: fn(&Fixture, Option<Instant>, &AtomicBool, bool),
    }
    macro_rules! case {
        ($source:literal, $name:ident, |$f:ident, $d:ident, $t:ident| $call:expr, $error:pat) => {
            Case {
                source: $source,
                name: stringify!($name),
                run: |$f, $d, $t, interrupted| {
                    let result = $call;
                    if interrupted {
                        assert!(
                            matches!(result, Err($error)),
                            "{}: {result:?}",
                            stringify!($name)
                        );
                    } else {
                        result.unwrap();
                    }
                },
            }
        };
    }
    fn buffer_limits(deadline: Option<Instant>) -> crate::geodesic_buffer::DegenerateBufferLimits {
        crate::geodesic_buffer::DegenerateBufferLimits {
            max_work: 100_000_000,
            max_output_items: 10,
            max_reserved_bytes: 100_000_000,
            deadline,
        }
    }
    fn buffer_union_limits(
        deadline: Option<Instant>,
    ) -> crate::geodesic_buffer::point::union::UnionBufferLimits {
        crate::geodesic_buffer::point::union::UnionBufferLimits {
            overlay: polygons(deadline),
            max_results: 10,
            max_vertices: 100_000,
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Registro unico eseguibile delle API GOP-028"
    )]
    fn cases() -> Vec<Case> {
        vec![
            case!(
                "extended_algorithms/delaunay_limited.rs",
                delaunay_points_with_limits,
                |f, d, t| crate::extended_algorithms::delaunay_points_with_limits(
                    &f.polygon.exterior().0,
                    10,
                    limits(d),
                    Some(t),
                ),
                crate::extended_algorithms::DelaunayBuildError::Resource(
                    PredicateError::DeadlineExceeded
                )
            ),
            case!(
                "advanced.rs",
                materialize_with_limits,
                |f, d, t| {
                    let inputs: Vec<_> = f
                        .polygon
                        .exterior()
                        .0
                        .iter()
                        .copied()
                        .map(|p| Geometry::Point(Point(p)))
                        .collect();
                    let cells = crate::advanced::voronoi_cells_distinct(
                        &inputs,
                        5,
                        crate::rust_backend::precision::Precision::new(0.01).unwrap(),
                    )
                    .unwrap();
                    cells.materialize_with_limits(limits(d), Some(t))
                },
                PredicateError::DeadlineExceeded
            ),
            case!(
                "geodesic_buffer.rs",
                geodesic_empty_buffer_with_limits,
                |f, d, t| crate::geodesic_buffer::geodesic_empty_buffer_with_limits(
                    std::slice::from_ref(&f.point),
                    &f.crs,
                    &[0.0],
                    false,
                    buffer_limits(d),
                    Some(t)
                ),
                crate::geodesic_buffer::DegenerateBufferError::DeadlineExceeded
            ),
            case!(
                "geodesic_buffer/point.rs",
                geodesic_point_buffer_with_limits,
                |f, d, t| crate::geodesic_buffer::point::geodesic_point_buffer_with_limits(
                    std::slice::from_ref(&f.point),
                    &f.crs,
                    &[1.0],
                    0.01,
                    crate::geodesic_buffer::point::PointBufferLimits {
                        batch: buffer_limits(d),
                        max_vertices: 100_000
                    },
                    Some(t)
                ),
                crate::geodesic_buffer::DegenerateBufferError::DeadlineExceeded
            ),
            case!(
                "geodesic_buffer/point/union.rs",
                geodesic_multipoint_buffer_with_limits,
                |f, d, t| {
                    crate::geodesic_buffer::point::union::geodesic_multipoint_buffer_with_limits(
                        std::slice::from_ref(&f.point),
                        &f.crs,
                        &[1.0],
                        0.01,
                        false,
                        buffer_union_limits(d),
                        Some(t),
                    )
                },
                crate::geodesic_buffer::point::union::UnionBufferError::Overlay(
                    polygon_boolean::PolygonBooleanError::Predicate(
                        PredicateError::DeadlineExceeded
                    )
                )
            ),
            case!(
                "geodesic_buffer/projected.rs",
                geodesic_line_buffer_with_limits,
                |f, d, t| crate::geodesic_buffer::projected::geodesic_line_buffer_with_limits(
                    &[geo::LineString::from(vec![(0.0, 0.0), (0.00001, 0.0)]).into()],
                    &f.crs,
                    &[1.0],
                    0.01,
                    false,
                    buffer_union_limits(d),
                    Some(t)
                ),
                crate::geodesic_buffer::point::union::UnionBufferError::Overlay(
                    polygon_boolean::PolygonBooleanError::Predicate(
                        PredicateError::DeadlineExceeded
                    )
                )
            ),
            case!(
                "predicates.rs",
                relate_matrix_with_limits,
                |f, d, t| crate::predicates::relate_matrix_with_limits(
                    &f.line_geometry,
                    &f.point,
                    limits(d),
                    Some(t)
                ),
                PredicateError::DeadlineExceeded
            ),
            case!(
                "predicates/arrangement_indexed/point_boolean.rs",
                boolean_with_limits,
                |f, d, t| point_boolean::boolean_with_limits(
                    &f.point,
                    &f.point,
                    point_boolean::PointBooleanOp::Union,
                    None,
                    limits(d),
                    Some(t)
                ),
                point_boolean::PointBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/point_boolean.rs",
                union_with_limits,
                |f, d, t| point_boolean::union_with_limits(
                    &f.point,
                    &f.point,
                    None,
                    limits(d),
                    Some(t)
                ),
                point_boolean::PointBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/point_boolean.rs",
                intersection_with_limits,
                |f, d, t| point_boolean::intersection_with_limits(
                    &f.point,
                    &f.point,
                    None,
                    limits(d),
                    Some(t)
                ),
                point_boolean::PointBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/point_boolean.rs",
                difference_with_limits,
                |f, d, t| point_boolean::difference_with_limits(
                    &f.point,
                    &f.point,
                    None,
                    limits(d),
                    Some(t)
                ),
                point_boolean::PointBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/line_boolean.rs",
                boolean_with_limits,
                |f, d, t| line_boolean::boolean_with_limits(
                    &f.line_geometry,
                    &f.line_geometry,
                    line_boolean::LineBooleanOp::Union,
                    None,
                    limits(d),
                    Some(t)
                ),
                line_boolean::LineBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/line_boolean.rs",
                union_with_limits,
                |f, d, t| line_boolean::union_with_limits(
                    &f.line_geometry,
                    &f.line_geometry,
                    None,
                    limits(d),
                    Some(t)
                ),
                line_boolean::LineBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/line_boolean.rs",
                intersection_with_limits,
                |f, d, t| line_boolean::intersection_with_limits(
                    &f.line_geometry,
                    &f.line_geometry,
                    None,
                    limits(d),
                    Some(t)
                ),
                line_boolean::LineBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/line_boolean.rs",
                difference_with_limits,
                |f, d, t| line_boolean::difference_with_limits(
                    &f.line_geometry,
                    &f.line_geometry,
                    None,
                    limits(d),
                    Some(t)
                ),
                line_boolean::LineBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/polygon_boolean.rs",
                boolean_with_limits,
                |f, d, t| polygon_boolean::boolean_with_limits(
                    &f.polygon_geometry,
                    &f.polygon_geometry,
                    polygon_boolean::PolygonBooleanOp::Union,
                    None,
                    polygons(d),
                    Some(t)
                ),
                polygon_boolean::PolygonBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/polygon_boolean.rs",
                union_with_limits,
                |f, d, t| polygon_boolean::union_with_limits(
                    &f.polygon_geometry,
                    &f.polygon_geometry,
                    polygons(d),
                    Some(t)
                ),
                polygon_boolean::PolygonBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/polygon_boolean.rs",
                intersection_with_limits,
                |f, d, t| polygon_boolean::intersection_with_limits(
                    &f.polygon_geometry,
                    &f.polygon_geometry,
                    polygons(d),
                    Some(t)
                ),
                polygon_boolean::PolygonBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/polygon_boolean.rs",
                difference_with_limits,
                |f, d, t| polygon_boolean::difference_with_limits(
                    &f.polygon_geometry,
                    &f.polygon_geometry,
                    polygons(d),
                    Some(t)
                ),
                polygon_boolean::PolygonBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/general_boolean.rs",
                boolean_with_limits,
                |f, d, t| general_boolean::boolean_with_limits(
                    &f.polygon_geometry,
                    &f.polygon_geometry,
                    general_boolean::GeneralBooleanOp::Union,
                    None,
                    polygons(d),
                    Some(t)
                ),
                general_boolean::GeneralBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/general_boolean.rs",
                union_with_limits,
                |f, d, t| general_boolean::union_with_limits(
                    std::slice::from_ref(&f.polygon_geometry),
                    None,
                    polygons(d),
                    Some(t)
                ),
                general_boolean::GeneralBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/general_boolean.rs",
                intersection_with_limits,
                |f, d, t| general_boolean::intersection_with_limits(
                    std::slice::from_ref(&f.polygon_geometry),
                    &f.polygon_geometry,
                    None,
                    polygons(d),
                    Some(t)
                ),
                general_boolean::GeneralBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/general_boolean.rs",
                difference_with_limits,
                |f, d, t| general_boolean::difference_with_limits(
                    std::slice::from_ref(&f.polygon_geometry),
                    &f.polygon_geometry,
                    None,
                    polygons(d),
                    Some(t)
                ),
                general_boolean::GeneralBooleanError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "predicates/arrangement_indexed/polygon_completion.rs",
                polygon_completion_with_limits,
                |f, d, t| crate::polygon_completion_with_limits(
                    std::slice::from_ref(&f.polygon),
                    &LineString::from(vec![(1., 0.), (2., 0.), (2., 1.), (1., 1.)]).into(),
                    limits(d),
                    Some(t)
                ),
                crate::polygon_completion::PolygonCompletionError::Predicate(
                    PredicateError::DeadlineExceeded
                )
            ),
            case!(
                "extended_algorithms/split_limited.rs",
                split_line_with_limits,
                |f, d, t| crate::extended_algorithms::split_line_with_limits(
                    &f.line,
                    &f.point,
                    limits(d),
                    Some(t)
                ),
                PredicateError::DeadlineExceeded
            ),
            case!(
                "extended_algorithms/polygonize_limited.rs",
                polygonize_with_limits,
                |f, d, t| crate::extended_algorithms::polygonize_with_limits(
                    &f.line_geometry,
                    limits(d),
                    Some(t)
                ),
                PredicateError::DeadlineExceeded
            ),
            case!(
                "offset.rs",
                offset_with_limits,
                |f, d, t| crate::offset::offset_with_limits(
                    &f.line_geometry,
                    crate::offset::OffsetOptions::new(1., 0.01),
                    limits(d),
                    Some(t)
                ),
                crate::offset::OffsetError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "trim_extend.rs",
                trim_extend_with_limits,
                |f, d, t| crate::trim_extend::trim_extend_with_limits(
                    &f.line_geometry,
                    &f.guide,
                    crate::trim_extend::TrimExtendOptions::default(),
                    limits(d),
                    Some(t)
                ),
                crate::trim_extend::TrimExtendError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "reshape.rs",
                reshape_with_limits,
                |f, d, t| crate::reshape::reshape_with_limits(
                    &f.line_geometry,
                    &f.reshaper,
                    limits(d),
                    Some(t)
                ),
                crate::reshape::ReshapeError::Predicate(PredicateError::DeadlineExceeded)
            ),
            case!(
                "coordinate_api.rs",
                encode_coordinates_with_limits,
                |f, d, t| encode_coordinates_with_limits(
                    f.request(),
                    &[CoordinatePoint {
                        longitude: 1.,
                        latitude: 1.
                    }; 3],
                    CoordinateLimits {
                        deadline: d,
                        ..Default::default()
                    },
                    Some(t)
                ),
                CoordinateApiError {
                    kind: CoordinateErrorKind::DeadlineExceeded,
                    ..
                }
            ),
            case!(
                "coordinate_api.rs",
                decode_coordinates_with_limits,
                |f, d, t| decode_coordinates_with_limits(
                    CoordinateRequest {
                        options: &[CoordinateOption::Datum(CoordinateDatum::Wgs84)],
                        ..f.request()
                    },
                    &["1.0000N 1.0000E"; 3],
                    CoordinateLimits {
                        deadline: d,
                        ..Default::default()
                    },
                    Some(t)
                ),
                CoordinateApiError {
                    kind: CoordinateErrorKind::DeadlineExceeded,
                    ..
                }
            ),
            case!(
                "preserve_shape_measure.rs",
                preserve_shape_length_with_limits,
                |f, d, t| preserve_shape_length_with_limits(
                    std::slice::from_ref(&f.line),
                    &f.crs,
                    tolerance(),
                    measure(d),
                    Some(t)
                ),
                ShapeMeasureError::DeadlineExceeded
            ),
            case!(
                "preserve_shape_measure.rs",
                preserve_shape_area_with_limits,
                |f, d, t| preserve_shape_area_with_limits(
                    std::slice::from_ref(&f.polygon),
                    &f.crs,
                    tolerance(),
                    measure(d),
                    Some(t)
                ),
                ShapeMeasureError::DeadlineExceeded
            ),
            case!(
                "geodesic_densify.rs",
                geodesic_densify_with_limits,
                |f, d, t| geodesic_densify_with_limits(
                    std::slice::from_ref(&f.line),
                    &f.crs,
                    100_000.,
                    densify(d),
                    Some(t)
                ),
                GeodesicDensifyError::DeadlineExceeded
            ),
            case!(
                "geodesic_densify/geometry.rs",
                geodesic_densify_geometry_with_limits,
                |f, d, t| geodesic_densify_geometry_with_limits(
                    &f.polygon_geometry,
                    &f.crs,
                    100_000.,
                    GeodesicGeometryDensifyLimits {
                        output: densify(d),
                        max_validation_work: 1_000_000
                    },
                    Some(t)
                ),
                GeodesicDensifyError::DeadlineExceeded
            ),
            case!(
                "geodesic_distance.rs",
                geodesic_distance_with_limits,
                |f, d, t| geodesic_distance_with_limits(
                    &f.line_geometry,
                    &f.point,
                    &f.crs,
                    GeodesicDistanceLimits {
                        max_work: 1_000_000,
                        max_reserved_bytes: 10_000_000,
                        deadline: d
                    },
                    Some(t)
                ),
                GeodesicDistanceError::DeadlineExceeded
            ),
        ]
    }
    /// Fonte GOP-028: stesso esito pubblico all'ingresso e dopo lavoro reale.
    #[test]
    fn all_public_limits_deadline_precedence() {
        for case in cases() {
            for reverse in [false, true] {
                let fixture = Fixture::new(reverse);
                let flag = AtomicBool::new(false);
                let probe = Probe::start(None);
                (case.run)(&fixture, None, &flag, false);
                let total = Probe::count();
                drop(probe);
                assert!(total > 2, "{}: {}", case.source, case.name);
                // Deadline reale scaduta e token gia' alzato all'ingresso.
                flag.store(true, AtomicOrdering::Relaxed);
                (case.run)(&fixture, Some(Instant::now()), &flag, true);
                // Primo checkpoint interno, centro e ultima pubblicazione.
                for at in [1, total / 2, total - 1] {
                    flag.store(false, AtomicOrdering::Relaxed);
                    let _probe = Probe::start(Some(at));
                    (case.run)(&fixture, None, &flag, true);
                    assert_eq!(Probe::count(), at + 1, "{}: {}", case.source, case.name);
                    assert!(flag.load(AtomicOrdering::Relaxed));
                }
            }
        }
    }
    // Anche API prive di token/deadline devono essere censite esplicitamente.
    const BUDGET_ONLY: &[(&str, &str)] = &[
        ("rust_backend/mod.rs", "make_valid_wkb_with_limits"),
        (
            "rust_backend/make_valid.rs",
            "make_valid_geometry_rust_with_limits",
        ),
    ];
    fn discover(directory: &Path, root: &Path, found: &mut BTreeSet<(String, String)>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                discover(&path, root, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                // Tokenizzazione minima: whitespace e punteggiatura, firme multilinea incluse.
                let tokens: Vec<_> = source
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .filter(|s| !s.is_empty())
                    .collect();
                for triple in tokens.windows(3) {
                    if triple[0] == "pub"
                        && triple[1] == "fn"
                        && triple[2].ends_with("_with_limits")
                    {
                        assert!(found.insert((
                            path.strip_prefix(root)
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .replace('\\', "/"),
                            triple[2].to_owned()
                        )));
                    }
                }
            }
        }
    }
    /// Fonte GOP-028: ogni nuova firma pubblica deve avere una riga eseguibile.
    #[test]
    fn public_limits_registry_is_complete() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = BTreeSet::new();
        discover(&root, &root, &mut found);
        let cases = cases();
        let mut registered: BTreeSet<_> = cases
            .iter()
            .map(|case| (case.source.to_owned(), case.name.to_owned()))
            .collect();
        assert_eq!(registered.len(), cases.len());
        registered.extend(
            BUDGET_ONLY
                .iter()
                .map(|(source, name)| ((*source).to_owned(), (*name).to_owned())),
        );
        assert_eq!(found, registered);
    }
}
