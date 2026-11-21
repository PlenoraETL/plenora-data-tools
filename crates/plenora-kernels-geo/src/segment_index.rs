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
        Self::new_along(bounds, 0, work)
    }

    fn new_along(
        bounds: CountedVec<[Q; 4]>,
        axis: usize,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        let mut ids = CountedVec::new();
        for id in 0..bounds.len() {
            work.push(&mut ids, id)?;
        }
        sorted(
            &mut ids,
            |a, b| {
                Ok(bounds[*a][axis]
                    .try_cmp(&bounds[*b][axis])?
                    .then(bounds[*a][2 - axis].try_cmp(&bounds[*b][2 - axis])?)
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
        Self::edges_along(edges, 0, work)
    }

    /// L'asse Y raggruppa i lati attraversati da un raggio orizzontale;
    /// inviluppi, confronti inclusivi e ID restituiti restano esatti.
    pub fn edges_for_rays<'a>(
        edges: impl Iterator<Item = &'a Edge>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        Self::edges_along(edges, 2, work)
    }

    fn edges_along<'a>(
        edges: impl Iterator<Item = &'a Edge>,
        axis: usize,
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
        Self::new_along(bounds, axis, work)
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

/// Distanza quadrata razionale: selezione e pruning precedono ogni
/// arrotondamento. Tutti gli interi appartengono al bilancio del Work.
pub struct DistanceKey(Q);

impl DistanceKey {
    pub fn squared(value: Q) -> Result<Self, PredicateError> {
        if value.sign().is_lt() {
            return Err(PredicateError::AccuracyUnproven);
        }
        Ok(Self(value))
    }

    pub fn zero(work: &Work<'_>) -> Result<Self, PredicateError> {
        Ok(Self(work.integer(0)?))
    }
    pub fn point_segment(p: &P, edge: &Edge, work: &mut Work<'_>) -> Result<Self, PredicateError> {
        work.step()?;
        let direction = edge.b.try_sub(&edge.a)?;
        let offset = p.try_sub(&edge.a)?;
        let length = norm_squared(&direction)?;
        let projection = offset
            .x
            .try_mul(&direction.x)?
            .try_add(&offset.y.try_mul(&direction.y)?)?;
        if length.sign().is_eq() || projection.sign().is_le() {
            return Ok(Self(norm_squared(&offset)?));
        }
        if projection.try_cmp(&length)?.is_ge() {
            return Ok(Self(norm_squared(&p.try_sub(&edge.b)?)?));
        }
        let cross = offset.try_cross(&direction)?;
        Ok(Self(cross.try_mul(&cross)?.try_div(&length)?))
    }

    pub fn segments(a: &Edge, b: &Edge, work: &mut Work<'_>) -> Result<Self, PredicateError> {
        Self::segments_impl(a, b, work, true)
    }

    fn segments_impl(
        a: &Edge,
        b: &Edge,
        work: &mut Work<'_>,
        point_query: bool,
    ) -> Result<Self, PredicateError> {
        work.pair()?;
        let direction = a.b.try_sub(&a.a)?;
        // Una query puntuale ha un solo candidato: la distanza dal punto
        // al lato; i suoi estremi non possono essere piu' vicini del lato.
        // La coppia resta contabilizzata come nel percorso generico.
        if point_query && direction.x.sign().is_eq() && direction.y.sign().is_eq() {
            return Self::point_segment(&a.a, b, work);
        }
        let other = b.b.try_sub(&b.a)?;
        let denominator = direction.try_cross(&other)?;
        if !denominator.sign().is_eq() {
            let offset = b.a.try_sub(&a.a)?;
            let t = offset.try_cross(&other)?.try_div(&denominator)?;
            let u = offset.try_cross(&direction)?.try_div(&denominator)?;
            let one = work.integer(1)?;
            if t.sign().is_ge()
                && t.try_cmp(&one)?.is_le()
                && u.sign().is_ge()
                && u.try_cmp(&one)?.is_le()
            {
                return Ok(Self(work.integer(0)?));
            }
        }
        let mut best = Self::point_segment(&a.a, b, work)?;
        for candidate in [
            Self::point_segment(&a.b, b, work)?,
            Self::point_segment(&b.a, a, work)?,
            Self::point_segment(&b.b, a, work)?,
        ] {
            best.select(candidate)?;
        }
        Ok(best)
    }

    pub fn select(&mut self, candidate: Self) -> Result<(), PredicateError> {
        if candidate.0.try_cmp(&self.0)?.is_lt() {
            *self = candidate;
        }
        Ok(())
    }

    /// Radice correttamente arrotondata, ties-to-even, anche subnormale.
    /// Rifiuta distanze oltre il massimo finito invece di emettere infinito.
    pub fn rounded(&self, work: &mut Work<'_>) -> Result<f64, PredicateError> {
        self.rounded_impl(work, true)
    }

    fn rounded_impl(&self, work: &mut Work<'_>, fast: bool) -> Result<f64, PredicateError> {
        if self.0.sign().is_eq() {
            return Ok(0.0);
        }
        if fast {
            if let Some(hint) = self.0.sqrt_hint() {
                work.step()?;
                let q = |x| P::of_in(geo::Coord { x, y: 0.0 }, work).map(|p| p.x);
                let middle = q(hint)?;
                let low = q(hint.next_down())?.try_midpoint(&middle)?;
                let high = q(hint.next_up())?.try_midpoint(&middle)?;
                let lo = self.0.try_cmp(&low.try_mul(&low)?)?;
                let hi = self.0.try_cmp(&high.try_mul(&high)?)?;
                let even = hint.to_bits() % 2 == 0;
                if (lo.is_gt() || (lo.is_eq() && even)) && (hi.is_lt() || (hi.is_eq() && even)) {
                    return Ok(hint);
                }
            }
        }
        let scalar = |bits, work: &Work<'_>| {
            P::of_in(
                geo::Coord {
                    x: f64::from_bits(bits),
                    y: 0.0,
                },
                work,
            )
            .map(|p| p.x)
        };
        let mut lo = 0_u64;
        let mut hi = f64::MAX.to_bits();
        let maximum = scalar(hi, work)?;
        if self.0.try_cmp(&maximum.try_mul(&maximum)?)?.is_gt() {
            return Err(PredicateError::AccuracyUnproven);
        }
        while lo < hi {
            work.step()?;
            let middle = lo + (hi - lo).div_ceil(2);
            let q = scalar(middle, work)?;
            if q.try_mul(&q)?.try_cmp(&self.0)?.is_le() {
                lo = middle;
            } else {
                hi = middle - 1;
            }
        }
        if lo == f64::MAX.to_bits() {
            return Ok(f64::MAX);
        }
        let midpoint = scalar(lo, work)?.try_midpoint(&scalar(lo + 1, work)?)?;
        let order = self.0.try_cmp(&midpoint.try_mul(&midpoint)?)?;
        let bits = if order.is_gt() || (order.is_eq() && lo % 2 == 1) {
            lo + 1
        } else {
            lo
        };
        if bits == 0 && !self.0.sign().is_eq() {
            return Err(PredicateError::AccuracyUnproven);
        }
        Ok(f64::from_bits(bits))
    }
}

fn norm_squared(p: &P) -> Result<Q, PredicateError> {
    p.x.try_mul(&p.x)?.try_add(&p.y.try_mul(&p.y)?)
}

impl SegmentIndex {
    /// Branch-and-bound esatto: una distanza fra inviluppi e' un limite
    /// inferiore, mai una distanza candidata. Visita prima il figlio vicino.
    pub fn nearest(
        &self,
        query: &Edge,
        edges: &[Edge],
        best: &mut Option<DistanceKey>,
        work: &mut Work<'_>,
    ) -> Result<(), PredicateError> {
        let bound = |node: usize, work: &Work<'_>| -> Result<DistanceKey, PredicateError> {
            let node = &self.nodes[node];
            let mut sum = work.integer(0)?;
            for (axis, a, b) in [(0, &query.a.x, &query.b.x), (2, &query.a.y, &query.b.y)] {
                let (low, high) = if a.try_cmp(b)?.is_le() {
                    (a, b)
                } else {
                    (b, a)
                };
                let min = &self.bounds[node.bounds[axis]][axis];
                let max = &self.bounds[node.bounds[axis + 1]][axis + 1];
                let delta = if low.try_cmp(max)?.is_gt() {
                    low.try_sub(max)?
                } else if high.try_cmp(min)?.is_lt() {
                    min.try_sub(high)?
                } else {
                    work.integer(0)?
                };
                sum = sum.try_add(&delta.try_mul(&delta)?)?;
            }
            Ok(DistanceKey(sum))
        };
        let mut pending = CountedVec::new();
        if let Some(root) = self.root {
            work.push(&mut pending, root)?;
        }
        while let Some(id) = pending.pop() {
            work.step()?;
            let lower = bound(id, work)?;
            if let Some(current) = best.as_ref() {
                if lower.0.try_cmp(&current.0)?.is_gt() {
                    continue;
                }
            }
            let node = &self.nodes[id];
            if let Some((left, right)) = node.children {
                let order = bound(left, work)?.0.try_cmp(&bound(right, work)?.0)?;
                let (near, far) = if order.is_le() {
                    (left, right)
                } else {
                    (right, left)
                };
                work.push(&mut pending, far)?;
                work.push(&mut pending, near)?;
            } else {
                let candidate = DistanceKey::segments(query, &edges[node.item], work)?;
                if let Some(current) = best {
                    current.select(candidate)?;
                } else {
                    *best = Some(candidate);
                }
            }
        }
        Ok(())
    }
}

impl DistanceKey {
    pub fn compare(&self, other: &Self) -> Result<std::cmp::Ordering, PredicateError> {
        self.0.try_cmp(&other.0)
    }

    /// Confronta sqrt(self) <= factor*sqrt(other)+offset senza radici.
    pub fn within_sum_scaled(
        &self,
        other: &Self,
        factor: f64,
        offset: f64,
        work: &Work<'_>,
    ) -> Result<bool, PredicateError> {
        if !factor.is_finite() || factor < 0.0 || !offset.is_finite() || offset < 0.0 {
            return Err(PredicateError::AccuracyUnproven);
        }
        let f = P::of_in((factor, offset).into(), work)?;
        let scaled = other.0.try_mul(&f.x.try_mul(&f.x)?)?;
        let offset2 = f.y.try_mul(&f.y)?;
        let remainder = self.0.try_sub(&scaled)?.try_sub(&offset2)?;
        if remainder.sign().is_le() {
            return Ok(true);
        }
        Ok(remainder
            .try_mul(&remainder)?
            .try_cmp(&scaled.try_mul(&offset2)?.try_mul(&work.integer(4)?)?)?
            .is_le())
    }

    pub fn within(&self, limit: f64, work: &Work<'_>) -> Result<bool, PredicateError> {
        if !limit.is_finite() || limit < 0.0 {
            return Err(PredicateError::AccuracyUnproven);
        }
        let q = P::of_in(geo::Coord { x: limit, y: 0.0 }, work)?.x;
        Ok(self.0.try_cmp(&q.try_mul(&q)?)?.is_le())
    }

    pub fn coordinates(
        point: geo::Coord<f64>,
        start: geo::Coord<f64>,
        end: geo::Coord<f64>,
        work: &mut Work<'_>,
    ) -> Result<Self, PredicateError> {
        work.nodes(3)?;
        let p = P::of_in(point, work)?;
        let edge = Edge {
            a: P::of_in(start, work)?,
            b: P::of_in(end, work)?,
        };
        Self::point_segment(&p, &edge, work)
    }
}

/// Finestra conservativa: arrotondamento verso l'esterno su entrambi i
/// bordi. Gli infiniti da overflow allargano la query, mai la restringono.
pub fn metric_window(point: [f64; 2], radius: f64) -> rstar::AABB<[f64; 2]> {
    rstar::AABB::from_corners(
        point.map(|v| (v - radius).next_down()),
        point.map(|v| (v + radius).next_up()),
    )
}
/// Filtro conservativo comune. Input non validi restano candidati per
/// conservare il successivo errore della primitiva esatta.
pub fn metric_candidate(
    point: geo::Coord<f64>,
    start: geo::Coord<f64>,
    end: geo::Coord<f64>,
    radius: f64,
    work: &mut Work<'_>,
) -> Result<bool, PredicateError> {
    use rstar::Envelope;
    work.step()?;
    if !radius.is_finite()
        || radius < 0.0
        || ![point, start, end]
            .iter()
            .all(|p| p.x.is_finite() && p.y.is_finite())
    {
        return Ok(true);
    }
    let window = metric_window([point.x, point.y], radius);
    let segment = rstar::AABB::from_corners(
        [start.x.min(end.x), start.y.min(end.y)],
        [start.x.max(end.x), start.y.max(end.y)],
    );
    Ok(window.intersects(&segment))
}

/// Parametro della proiezione ortogonale, clamp esatto prima del solo
/// arrotondamento d'uscita. I segmenti degeneri hanno parametro zero.
pub fn projection_parameter(
    point: geo::Coord<f64>,
    start: geo::Coord<f64>,
    end: geo::Coord<f64>,
    work: &mut Work<'_>,
) -> Result<f64, PredicateError> {
    projection_parameter_impl(point, start, end, work, true)
}

fn projection_parameter_impl(
    point: geo::Coord<f64>,
    start: geo::Coord<f64>,
    end: geo::Coord<f64>,
    work: &mut Work<'_>,
    filtered: bool,
) -> Result<f64, PredicateError> {
    work.step()?;
    if filtered {
        if let Some(value) = projection_endpoint(point, start, end) {
            return Ok(value);
        }
    }
    let direction = P::of_in(end, work)?.try_sub(&P::of_in(start, work)?)?;
    let offset = P::of_in(point, work)?.try_sub(&P::of_in(start, work)?)?;
    let length = norm_squared(&direction)?;
    let dot = offset
        .x
        .try_mul(&direction.x)?
        .try_add(&offset.y.try_mul(&direction.y)?)?;
    if length.sign().is_eq() || dot.sign().is_le() {
        return Ok(0.0);
    }
    if dot.try_cmp(&length)?.is_ge() {
        return Ok(1.0);
    }
    let value = crate::predicates::arrangement_indexed::line_boolean::ordinate(
        &dot.try_div(&length)?,
        work,
    )
    .map_err(|error| match error {
        crate::predicates::arrangement_indexed::line_boolean::LineBooleanError::Predicate(
            error,
        ) => error,
        _ => PredicateError::AccuracyUnproven,
    })?;
    if value <= 0.0 {
        Err(PredicateError::AccuracyUnproven)
    } else {
        Ok(value)
    }
}

// Filtro dei soli clamp: i prodotti scalari agli estremi sono valutati
// con intervalli IEEE diretti. Nessun parametro interno e' approssimato.
fn projection_endpoint(
    point: geo::Coord<f64>,
    start: geo::Coord<f64>,
    end: geo::Coord<f64>,
) -> Option<f64> {
    if ![point, start, end]
        .iter()
        .all(|p| p.x.is_finite() && p.y.is_finite())
    {
        return None;
    }
    if start == end {
        return Some(0.0);
    }
    let sub = |a: f64, b: f64| [(a - b).next_down(), (a - b).next_up()];
    let mul = |a: [f64; 2], b: [f64; 2]| -> Option<[f64; 2]> {
        let values = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
        if !values.iter().all(|v| v.is_finite()) {
            return None;
        }
        Some([
            values.into_iter().fold(f64::INFINITY, f64::min).next_down(),
            values
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)
                .next_up(),
        ])
    };
    let dx = sub(end.x, start.x);
    let dy = sub(end.y, start.y);
    let dot = |origin: geo::Coord<f64>| -> Option<[f64; 2]> {
        let x = mul(sub(point.x, origin.x), dx)?;
        let y = mul(sub(point.y, origin.y), dy)?;
        Some([(x[0] + y[0]).next_down(), (x[1] + y[1]).next_up()])
    };
    if dot(start)?[1] < 0.0 {
        return Some(0.0);
    }
    if dot(end)?[0] > 0.0 {
        return Some(1.0);
    }
    None
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::predicates::RelateLimits;
    use proptest::prelude::*;

    #[test]
    fn projection_endpoint_filter_matches_exact_bits() {
        // C1: clamp certificato contro proiezione razionale senza filtro,
        // segmenti degeneri, perpendicolari, obliqui e scale IEEE estreme.
        for scale in [f64::from_bits(1), 1e-200, 1.0, 1e200, f64::MAX / 16.0] {
            for (a, b) in [
                ((0.0, 0.0), (0.0, 0.0)),
                ((0.0, 0.0), (2.0, 1.0)),
                ((0.0, 0.0), (0.0, 2.0)),
            ] {
                for (x, y) in [
                    (-2.0, -1.0),
                    (0.0, 0.0),
                    (1.0, 0.5),
                    (2.0, 1.0),
                    (3.0, 2.0),
                    (0.0, 1.0),
                ] {
                    let mut work = Work::new(RelateLimits::default(), None);
                    let point = (x * scale, y * scale).into();
                    let a = (a.0 * scale, a.1 * scale).into();
                    let b = (b.0 * scale, b.1 * scale).into();
                    assert_eq!(
                        projection_parameter_impl(point, a, b, &mut work, true)
                            .map(f64::to_bits)
                            .map_err(|e| e.to_string()),
                        projection_parameter_impl(point, a, b, &mut work, false)
                            .map(f64::to_bits)
                            .map_err(|e| e.to_string())
                    );
                }
            }
        }
    }

    #[test]
    fn point_query_distance_matches_generic_segment_path() {
        // C1: scorciatoia della query puntuale contro i quattro candidati
        // generici, anche degeneri e alle scale estreme IEEE.
        for scale in [f64::from_bits(1), 1e-200, 1.0, 1e200, f64::MAX / 8.0] {
            let mut work = Work::new(RelateLimits::default(), None);
            for x in -2..=2 {
                for y in -2..=2 {
                    for (ax, ay, bx, by) in [
                        (-3.0, 0.0, 3.0, 0.0),
                        (0.0, -3.0, 0.0, 3.0),
                        (-3.0, -2.0, 3.0, 2.0),
                        (-3.0, 2.0, 3.0, -2.0),
                        (1.0, 1.0, 1.0, 1.0),
                    ] {
                        let query = Edge {
                            a: point(f64::from(x) * scale, f64::from(y) * scale, &work),
                            b: point(f64::from(x) * scale, f64::from(y) * scale, &work),
                        };
                        let edge = Edge {
                            a: point(ax * scale, ay * scale, &work),
                            b: point(bx * scale, by * scale, &work),
                        };
                        let pairs = work.pairs;
                        let fast = DistanceKey::segments(&query, &edge, &mut work).unwrap();
                        assert_eq!(work.pairs, pairs + 1);
                        let generic =
                            DistanceKey::segments_impl(&query, &edge, &mut work, false).unwrap();
                        assert_eq!(work.pairs, pairs + 2);
                        assert_eq!(fast.compare(&generic).unwrap(), std::cmp::Ordering::Equal);
                        let reversed = DistanceKey::segments(&edge, &query, &mut work).unwrap();
                        assert_eq!(fast.compare(&reversed).unwrap(), std::cmp::Ordering::Equal);
                    }
                }
            }
        }
    }

    #[test]
    fn metric_rounding_certification_and_nonrepresentable_values() {
        let mut work = Work::new(RelateLimits::default(), None);
        for scale in [f64::from_bits(1), 1e-300, 1.0, 1e300] {
            for (x, y) in [(0., 0.), (1., 0.), (3., 4.), (1., 1.), (2., 3.)] {
                let p = point(x * scale, y * scale, &work);
                let key = DistanceKey(norm_squared(&p).unwrap());
                assert_eq!(
                    key.rounded_impl(&mut work, true)
                        .map(f64::to_bits)
                        .map_err(|e| e.to_string()),
                    key.rounded_impl(&mut work, false)
                        .map(f64::to_bits)
                        .map_err(|e| e.to_string())
                );
            }
        }
        let tiny = f64::from_bits(1);
        let key = DistanceKey::coordinates(
            (0., tiny).into(),
            (0., 0.).into(),
            (tiny, 2. * tiny).into(),
            &mut work,
        )
        .unwrap();
        assert!(matches!(
            key.rounded(&mut work),
            Err(PredicateError::AccuracyUnproven)
        ));
        let key = DistanceKey::coordinates(
            (f64::MAX, 0.).into(),
            (-f64::MAX, 0.).into(),
            (-f64::MAX, 0.).into(),
            &mut work,
        )
        .unwrap();
        assert!(matches!(
            key.rounded(&mut work),
            Err(PredicateError::AccuracyUnproven)
        ));
    }

    #[test]
    fn metric_thresholds_compare_before_rounding() {
        let mut work = Work::new(RelateLimits::default(), None);
        let key = DistanceKey::coordinates(
            (1., 2_f64.powi(-27)).into(),
            (0., 0.).into(),
            (0., 0.).into(),
            &mut work,
        )
        .unwrap();
        assert_eq!(key.rounded(&mut work).unwrap().to_bits(), 1_f64.to_bits());
        assert!(!key.within(1., &work).unwrap());
        let zero = DistanceKey::zero(&work).unwrap();
        assert!(!key.within_sum_scaled(&zero, 0.75, 1., &work).unwrap());
        for scale in [f64::from_bits(1), 1e-300, 1., 1e300] {
            let unit = DistanceKey::coordinates(
                (scale, 0.).into(),
                (0., 0.).into(),
                (0., 0.).into(),
                &mut work,
            )
            .unwrap();
            assert!(unit.within(scale, &work).unwrap());
            assert!(!unit.within(scale.next_down(), &work).unwrap());
            assert!(unit.within_sum_scaled(&unit, 0.75, scale, &work).unwrap());
            assert!(!unit.within_sum_scaled(&unit, 0.75, 0., &work).unwrap());
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        #[test]
        fn metric_fast_rounding_matches_binary_search(x in -100_000i32..100_000,y in -100_000i32..100_000) {
            let mut work=Work::new(RelateLimits::default(),None);
            let p=point(f64::from(x)/7.0,f64::from(y)/11.0,&work);
            let key=DistanceKey(norm_squared(&p).unwrap());
            prop_assert_eq!(key.rounded_impl(&mut work,true).unwrap().to_bits(),key.rounded_impl(&mut work,false).unwrap().to_bits());
        }
    }

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
        // C1: entrambi gli assi contro la scansione esatta, stessi ID e parita'.
        for index in [
            SegmentIndex::edges(edges.iter(), &mut work).expect("index X"),
            SegmentIndex::edges_for_rays(edges.iter(), &mut work).expect("index Y"),
        ] {
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
