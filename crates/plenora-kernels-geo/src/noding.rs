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

pub struct Work<'a> {
    limits: RelateLimits,
    deadline_first: bool,
    cancelled: Option<&'a std::sync::atomic::AtomicBool>,
    pub steps: u64,
    pub pairs: u64,
    budget: Rc<storage::Budget>,
    pub nodes: u64,
    exports: CountedVec<storage::Lease>,
    #[cfg(test)]
    pub interrupt_at: Option<(u64, bool)>,
}
impl<'a> Work<'a> {
    pub fn new(limits: RelateLimits, cancelled: Option<&'a std::sync::atomic::AtomicBool>) -> Self {
        Self {
            limits,
            deadline_first: false,
            cancelled,
            steps: 0,
            pairs: 0,
            budget: Rc::new(storage::Budget::new(limits.max_reserved_bytes)),
            nodes: 0,
            exports: CountedVec::new(),
            #[cfg(test)]
            interrupt_at: None,
        }
    }
    pub(crate) fn with_deadline_precedence(
        limits: RelateLimits,
        cancelled: Option<&'a std::sync::atomic::AtomicBool>,
    ) -> Self {
        let mut work = Self::new(limits, cancelled);
        work.deadline_first = true;
        work
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
        if self.deadline_first
            && self
                .limits
                .deadline
                .is_some_and(|d| std::time::Instant::now() >= d)
        {
            return Err(PredicateError::DeadlineExceeded);
        }
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
    compare: impl Fn(&T, &T) -> Result<Ordering, PredicateError>,
    work: &mut Work<'_>,
) -> Result<(), PredicateError> {
    // Insertion sort: ogni confronto e' interrompibile, nessuna allocazione.
    for i in 1..values.len() {
        let mut j = i;
        while j > 0 {
            work.step()?;
            if compare(&values[j - 1], &values[j])? != Ordering::Greater {
                break;
            }
            values.swap(j - 1, j);
            j -= 1;
        }
    }
    Ok(())
}
pub use exact::Q;

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
        assert!(matches!(cancelled.step(), Err(PredicateError::Cancelled)));
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
