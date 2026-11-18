//! Noding razionale comune: inviluppi esatti, coppie candidate e nodi.
//!
//! Sweep sugli estremi X ordinati: il filtro inclusivo X/Y non perde
//! contatti e non converte mai un razionale in floating point.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use crate::predicates::{PredicateError, RelateLimits};
use geo::Coord;
use num_bigint::{BigInt, Sign};
use std::cmp::Ordering;
use std::rc::Rc;

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod bounds;
mod storage;
pub use storage::CountedVec;

pub struct Work<'a> {
    limits: RelateLimits,
    cancelled: Option<&'a std::sync::atomic::AtomicBool>,
    pub steps: u64,
    pub pairs: u64,
    budget: Rc<storage::Budget>,
    pub nodes: u64,
}
impl<'a> Work<'a> {
    pub fn new(limits: RelateLimits, cancelled: Option<&'a std::sync::atomic::AtomicBool>) -> Self {
        Self {
            limits,
            cancelled,
            steps: 0,
            pairs: 0,
            budget: Rc::new(storage::Budget::new(limits.max_reserved_bytes)),
            nodes: 0,
        }
    }
    pub fn step(&mut self) -> Result<(), PredicateError> {
        if self
            .cancelled
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        {
            return Err(PredicateError::Cancelled);
        }
        if self
            .limits
            .deadline
            .is_some_and(|d| std::time::Instant::now() >= d)
        {
            return Err(PredicateError::DeadlineExceeded);
        }
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
    #[cfg(test)]
    #[deny(clippy::disallowed_methods)]
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
    compare: impl Fn(&T, &T) -> Ordering,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    // Insertion sort: ogni confronto e' interrompibile, nessuna allocazione.
    for i in 1..values.len() {
        let mut j = i;
        while j > 0 {
            work.step()?;
            if compare(&values[j - 1], &values[j]) != Ordering::Greater {
                break;
            }
            values.swap(j - 1, j);
            j -= 1;
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Q {
    n: BigInt,
    d: BigInt,
}

impl Q {
    pub fn new(mut n: BigInt, mut d: BigInt) -> Self {
        if d.sign() == Sign::Minus {
            n = -n;
            d = -d;
        }
        let mut a = if n.sign() == Sign::Minus {
            -&n
        } else {
            n.clone()
        };
        let mut b = d.clone();
        while b != BigInt::from(0) {
            let r = &a % &b;
            a = b;
            b = r;
        }
        Self {
            n: n / &a,
            d: d / a,
        }
    }
    pub fn int(n: i32) -> Self {
        Self {
            n: BigInt::from(n),
            d: BigInt::from(1),
        }
    }
    pub fn float(x: f64) -> Self {
        let bits = x.to_bits();
        let exponent = ((bits >> 52) & 2047) as i32;
        let fraction = bits & ((1_u64 << 52) - 1);
        let mantissa = if exponent == 0 {
            fraction
        } else {
            fraction | (1_u64 << 52)
        };
        let shift = if exponent == 0 {
            -1074
        } else {
            exponent - 1075
        };
        let mut n = BigInt::from(mantissa);
        if bits >> 63 != 0 {
            n = -n;
        }
        if shift >= 0 {
            Self::new(n << shift.unsigned_abs(), BigInt::from(1))
        } else {
            Self::new(n, BigInt::from(1) << shift.unsigned_abs())
        }
    }
    pub fn add(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d + &b.n * &self.d, &self.d * &b.d)
    }
    pub fn sub(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d - &b.n * &self.d, &self.d * &b.d)
    }
    pub fn mul(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.n, &self.d * &b.d)
    }
    pub fn div(&self, b: &Self) -> Self {
        Self::new(&self.n * &b.d, &self.d * &b.n)
    }
    pub fn sign(&self) -> Ordering {
        self.n.cmp(&BigInt::from(0))
    }
    pub fn midpoint(&self, b: &Self) -> Self {
        self.add(b).div(&Self::int(2))
    }
}
impl Ord for Q {
    fn cmp(&self, b: &Self) -> Ordering {
        (&self.n * &b.d).cmp(&(&b.n * &self.d))
    }
}
impl PartialOrd for Q {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct P {
    pub x: Q,
    pub y: Q,
}
impl P {
    pub fn of(p: Coord<f64>) -> Self {
        Self {
            x: Q::float(p.x),
            y: Q::float(p.y),
        }
    }
    pub fn sub(&self, b: &Self) -> Self {
        Self {
            x: self.x.sub(&b.x),
            y: self.y.sub(&b.y),
        }
    }
    pub fn add(&self, b: &Self) -> Self {
        Self {
            x: self.x.add(&b.x),
            y: self.y.add(&b.y),
        }
    }
    pub fn cross(&self, b: &Self) -> Q {
        self.x.mul(&b.y).sub(&self.y.mul(&b.x))
    }
}

#[derive(Clone)]
pub struct Edge {
    pub a: P,
    pub b: P,
}
impl Edge {
    pub fn at_x(&self, x: &Q) -> Q {
        self.a.y.add(
            &self
                .b
                .y
                .sub(&self.a.y)
                .mul(&x.sub(&self.a.x))
                .div(&self.b.x.sub(&self.a.x)),
        )
    }
    pub fn contains(&self, p: &P) -> bool {
        between(&p.x, &self.a.x, &self.b.x)
            && between(&p.y, &self.a.y, &self.b.y)
            && self.b.sub(&self.a).cross(&p.sub(&self.a)).sign() == Ordering::Equal
    }
}
pub fn between(x: &Q, a: &Q, b: &Q) -> bool {
    (a <= x && x <= b) || (b <= x && x <= a)
}

/// Intersezione parametrica esatta; gli overlap restituiscono i due estremi.
fn intersections(
    a: &Edge,
    b: &Edge,
    work: &mut Work<'_>,
) -> Result<[Option<P>; 2], PredicateError> {
    let r = a.b.sub(&a.a);
    let s = b.b.sub(&b.a);
    let denominator = r.cross(&s);
    if denominator.sign() != Ordering::Equal {
        let offset = b.a.sub(&a.a);
        let t = offset.cross(&s).div(&denominator);
        let u = offset.cross(&r).div(&denominator);
        if between(&t, &Q::int(0), &Q::int(1)) && between(&u, &Q::int(0), &Q::int(1)) {
            work.nodes(2)?;
            return Ok([
                Some(P {
                    x: a.a.x.add(&r.x.mul(&t)),
                    y: a.a.y.add(&r.y.mul(&t)),
                }),
                None,
            ]);
        }
        return Ok([None, None]);
    }
    let mut first = None;
    let mut last = None;
    for p in [&a.a, &a.b, &b.a, &b.b] {
        if a.contains(p) && b.contains(p) {
            if first.is_none_or(|q: &P| point_cmp(p, q).is_lt()) {
                first = Some(p);
            }
            if last.is_none_or(|q: &P| point_cmp(p, q).is_gt()) {
                last = Some(p);
            }
        }
    }
    if first == last {
        last = None;
    }
    work.nodes(2 * (u64::from(first.is_some()) + u64::from(last.is_some())))?;
    Ok([first.cloned(), last.cloned()])
}

fn point_cmp(a: &P, b: &P) -> Ordering {
    a.x.cmp(&b.x).then_with(|| a.y.cmp(&b.y))
}

fn index_cmp(a: usize, b: usize, edges: &[&Edge]) -> Ordering {
    edges[a]
        .a
        .x
        .min_ref(&edges[a].b.x)
        .cmp(edges[b].a.x.min_ref(&edges[b].b.x))
        .then_with(|| a.cmp(&b))
}

impl Q {
    fn min_ref<'a>(&'a self, other: &'a Self) -> &'a Self {
        if self <= other {
            self
        } else {
            other
        }
    }
    fn max_ref<'a>(&'a self, other: &'a Self) -> &'a Self {
        if self >= other {
            self
        } else {
            other
        }
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
        if child + 1 < index.len() && index_cmp(index[child], index[child + 1], edges).is_lt() {
            child += 1;
        }
        if !index_cmp(index[root], index[child], edges).is_lt() {
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
    for i in 0..edges.len() {
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
            if b.a.x.min_ref(&b.b.x) > a.a.x.max_ref(&a.b.x) {
                break;
            }
            if b.a.y.min_ref(&b.b.y) > a.a.y.max_ref(&a.b.y)
                || b.a.y.max_ref(&b.b.y) < a.a.y.min_ref(&a.b.y)
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
#[cfg(test)]
#[deny(clippy::disallowed_methods)]
pub fn node(
    edges: &[&Edge],
    work: &mut Work<'_>,
) -> Result<CountedVec<CountedVec<P>>, PredicateError> {
    work.step()?;
    let mut result = CountedVec::new();
    for edge in edges {
        let mut points = CountedVec::new();
        work.nodes(2)?;
        work.push(&mut points, edge.a.clone())?;
        work.push(&mut points, edge.b.clone())?;
        work.push(&mut result, points)?;
    }
    visit(edges, work, |i, j, hits, work| {
        for p in hits.into_iter().flatten() {
            work.push(&mut result[i], p.clone())?;
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
        if point_cmp(&edge.a, &edge.b).is_gt() {
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

    fn edge(a: (f64, f64), b: (f64, f64)) -> Edge {
        Edge {
            a: P::of(a.into()),
            b: P::of(b.into()),
        }
    }

    /// O-M1: y=x e y=1-2x hanno unico incrocio (1/3,1/3).
    /// SFA 1.2.1 6.1.7: gli estremi dell'overlap dividono entrambi i lati.
    #[test]
    fn rational_crossing_and_collinear_endpoints() {
        let edges = [edge((0., 0.), (1., 1.)), edge((0., 1.), (1., -1.))];
        let mut work = Work::new(RelateLimits::default(), None);
        let nodes = node(&edges.iter().collect::<Vec<_>>(), &mut work).expect("noding");
        let third = Q::int(1).div(&Q::int(3));
        assert_eq!(
            nodes[0][1],
            P {
                x: third.clone(),
                y: third
            }
        );
        assert_eq!(nodes[0][1], nodes[1][1]);
        assert_eq!((work.pairs, work.nodes), (1, 6));
        let edges = [edge((0., 0.), (3., 0.)), edge((2., 0.), (1., 0.))];
        let nodes = node(&edges.iter().collect::<Vec<_>>(), &mut work).expect("overlap");
        assert_eq!(
            nodes[0].iter().map(|p| p.x.clone()).collect::<Vec<_>>(),
            [0, 1, 2, 3].map(Q::int)
        );
        assert_eq!(
            nodes[1].iter().map(|p| p.x.clone()).collect::<Vec<_>>(),
            [2, 1].map(Q::int)
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
        assert_eq!(empty.peak_bytes(), 0);
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
        assert!(matches!(cancelled.step(), Err(PredicateError::Cancelled)));
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
