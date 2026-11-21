//! Copertura continua dell'erosione: docs/buffer-proiezione.md.

use super::{parts, BufferFailure, CountedVec, ProjectedBufferCertificate, UnionBufferError, Work};
use geo::{Coord, Geometry};

/// Intervalli IEEE diretti, anche per distanza e test del raggio orizzontale.
#[derive(Clone, Copy)]
struct ErosionRange {
    lo: f64,
    hi: f64,
}

impl ErosionRange {
    const fn exact(x: f64) -> Self {
        Self { lo: x, hi: x }
    }
    fn add(self, b: Self) -> Self {
        Self {
            lo: (self.lo + b.lo).next_down(),
            hi: (self.hi + b.hi).next_up(),
        }
    }
    fn sub(self, b: Self) -> Self {
        Self {
            lo: (self.lo - b.hi).next_down(),
            hi: (self.hi - b.lo).next_up(),
        }
    }
    fn mul(self, b: Self) -> Self {
        let v = [
            self.lo * b.lo,
            self.lo * b.hi,
            self.hi * b.lo,
            self.hi * b.hi,
        ];
        Self {
            lo: v.into_iter().fold(f64::INFINITY, f64::min).next_down(),
            hi: v.into_iter().fold(f64::NEG_INFINITY, f64::max).next_up(),
        }
    }
    fn square(self) -> Self {
        let lower = if self.lo <= 0.0 && self.hi >= 0.0 {
            0.0
        } else {
            self.lo.abs().min(self.hi.abs())
        };
        let upper = self.lo.abs().max(self.hi.abs());
        Self {
            lo: (lower * lower).next_down().max(0.0),
            hi: (upper * upper).next_up(),
        }
    }
    fn divide(self, b: Self) -> Result<Self, UnionBufferError> {
        if b.lo <= 0.0 || !(1.0 / b.lo).is_finite() || !self.lo.is_finite() || !self.hi.is_finite()
        {
            return Err(BufferFailure::AccuracyUnproven.into());
        }
        let reciprocal = Self {
            lo: (1.0 / b.hi).next_down(),
            hi: (1.0 / b.lo).next_up(),
        };
        if !reciprocal.lo.is_finite() || !reciprocal.hi.is_finite() {
            return Err(BufferFailure::AccuracyUnproven.into());
        }
        Ok(self.mul(reciprocal))
    }
}

struct Boundary<'a> {
    geometry: &'a Geometry,
    edges: CountedVec<crate::noding::Edge>,
    index: crate::segment_index::SegmentIndex,
}

impl<'a> Boundary<'a> {
    fn new(geometry: &'a Geometry, work: &mut Work<'_>) -> Result<Self, UnionBufferError> {
        let mut edges = CountedVec::new();
        for ring in parts(geometry)? {
            for pair in ring.0.windows(2) {
                work.step()?;
                if pair[0] != pair[1] {
                    let edge = crate::noding::Edge {
                        a: crate::noding::P::of_in(pair[0], work)?,
                        b: crate::noding::P::of_in(pair[1], work)?,
                    };
                    work.push(&mut edges, edge)?;
                }
            }
        }
        let index = crate::segment_index::SegmentIndex::edges(edges.iter(), work)?;
        Ok(Self {
            geometry,
            edges,
            index,
        })
    }
}

fn signed_distance(
    point: Coord,
    boundary: &Boundary<'_>,
    work: &mut Work<'_>,
) -> Result<ErosionRange, UnionBufferError> {
    signed_distance_impl(point, boundary, work, true)
}

fn signed_distance_impl(
    point: Coord,
    boundary: &Boundary<'_>,
    work: &mut Work<'_>,
    indexed: bool,
) -> Result<ErosionRange, UnionBufferError> {
    let geometry = boundary.geometry;
    if boundary.edges.is_empty() && parts(geometry)?.all(|ring| ring.0.is_empty()) {
        return Ok(ErosionRange::exact(f64::NEG_INFINITY));
    }
    let mut inside = false;
    let mut uncertain = false;
    for ring in parts(geometry)? {
        for edge in ring.0.windows(2) {
            work.step()?;
            let a = edge[0];
            let b = edge[1];
            if a == b {
                continue;
            }
            let dx = ErosionRange::exact(b.x).sub(ErosionRange::exact(a.x));
            let dy = ErosionRange::exact(b.y).sub(ErosionRange::exact(a.y));
            let px = ErosionRange::exact(point.x).sub(ErosionRange::exact(a.x));
            let py = ErosionRange::exact(point.y).sub(ErosionRange::exact(a.y));
            if (a.y > point.y) != (b.y > point.y) {
                let cross = dx.mul(py).sub(dy.mul(px));
                if cross.lo > 0.0 {
                    inside ^= b.y > a.y;
                } else if cross.hi < 0.0 {
                    inside ^= b.y < a.y;
                } else {
                    uncertain = true;
                }
            }
        }
    }
    let query = crate::noding::Edge {
        a: crate::noding::P::of_in(point, work)?,
        b: crate::noding::P::of_in(point, work)?,
    };
    let mut best = None;
    if indexed {
        boundary
            .index
            .nearest(&query, &boundary.edges, &mut best, work)?;
    } else {
        for edge in &boundary.edges {
            let candidate = crate::segment_index::DistanceKey::point_segment(&query.a, edge, work)?;
            if let Some(value) = &mut best {
                value.select(candidate)?;
            } else {
                best = Some(candidate);
            }
        }
    }
    let rounded = best.ok_or(BufferFailure::AccuracyUnproven)?.rounded(work)?;
    // La radice e' correttamente arrotondata: i vicini racchiudono l'esatto.
    let distance = ErosionRange {
        lo: rounded.next_down().max(0.0),
        hi: rounded.next_up(),
    };
    if uncertain {
        let point = Geometry::Point(geo::Point(point));
        inside = crate::predicates::arrangement_indexed::relate(geometry, &point, work)?.cells()[0]
            != b'F';
    }
    if inside {
        Ok(distance)
    } else {
        Ok(ErosionRange {
            lo: -distance.hi,
            hi: -distance.lo,
        })
    }
}

fn distance_filter(
    point: Coord,
    geometry: &Geometry,
    work: &mut Work<'_>,
) -> Result<Option<ErosionRange>, UnionBufferError> {
    let mut distance = ErosionRange::exact(f64::INFINITY);
    let mut inside = false;
    let mut uncertain = false;
    for ring in parts(geometry)? {
        for edge in ring.0.windows(2) {
            work.step()?;
            let a = edge[0];
            let b = edge[1];
            if a == b {
                continue;
            }
            let dx = ErosionRange::exact(b.x).sub(ErosionRange::exact(a.x));
            let dy = ErosionRange::exact(b.y).sub(ErosionRange::exact(a.y));
            let px = ErosionRange::exact(point.x).sub(ErosionRange::exact(a.x));
            let py = ErosionRange::exact(point.y).sub(ErosionRange::exact(a.y));
            let Ok(t) = px
                .mul(dx)
                .add(py.mul(dy))
                .divide(dx.square().add(dy.square()))
            else {
                return Ok(None);
            };
            let t = ErosionRange {
                lo: t.lo.clamp(0.0, 1.0),
                hi: t.hi.clamp(0.0, 1.0),
            };
            let squared = px.sub(t.mul(dx)).square().add(py.sub(t.mul(dy)).square());
            if !squared.lo.is_finite() || !squared.hi.is_finite() {
                return Ok(None);
            }
            distance.lo = distance
                .lo
                .min(squared.lo.max(0.0).sqrt().next_down().max(0.0));
            distance.hi = distance.hi.min(squared.hi.sqrt().next_up());
            if (a.y > point.y) != (b.y > point.y) {
                let cross = dx.mul(py).sub(dy.mul(px));
                if cross.lo > 0.0 {
                    inside ^= b.y > a.y;
                } else if cross.hi < 0.0 {
                    inside ^= b.y < a.y;
                } else {
                    uncertain = true;
                }
            }
        }
    }
    if uncertain
        || distance.lo < f64::MIN_POSITIVE
        || !distance.lo.is_finite()
        || !distance.hi.is_finite()
        || distance.hi >= f64::MAX
    {
        return Ok(None);
    }
    // Racchiude anche RN(d).next_down() e RN(d).next_up(), non solo d.
    distance.lo = distance.lo.next_down().next_down().max(0.0);
    distance.hi = distance.hi.next_up().next_up();
    if inside {
        Ok(Some(distance))
    } else {
        Ok(Some(ErosionRange {
            lo: -distance.hi,
            hi: -distance.lo,
        }))
    }
}

// Il filtro racchiude gli endpoint prodotti dal percorso razionale: una
// decisione si prende solo quando tutto l'intervallo sta dallo stesso lato.
fn above(
    point: Coord,
    boundary: &Boundary<'_>,
    limit: f64,
    lower: bool,
    work: &mut Work<'_>,
) -> Result<bool, UnionBufferError> {
    work.step()?;
    if let Some(range) = distance_filter(point, boundary.geometry, work)? {
        if range.lo > limit {
            return Ok(true);
        }
        if range.hi <= limit {
            return Ok(false);
        }
    }
    let distance = signed_distance(point, boundary, work)?;
    Ok(if lower { distance.lo } else { distance.hi } > limit)
}

fn separated(
    point: Coord,
    boundary: &Boundary<'_>,
    cell: ErosionCell,
    limit: f64,
    inside: bool,
    work: &mut Work<'_>,
) -> Result<bool, UnionBufferError> {
    work.step()?;
    if let (Some(distance), Some(half)) = (
        distance_filter(point, boundary.geometry, work)?,
        cell.reach_filter(point),
    ) {
        let value = if inside {
            distance.sub(half)
        } else {
            distance.add(half)
        };
        // Un ulteriore vicino copre il next_down/next_up del confronto esatto.
        let lo = value.lo.next_down();
        let hi = value.hi.next_up();
        if inside {
            if lo > limit {
                return Ok(true);
            }
            if hi <= limit {
                return Ok(false);
            }
        } else {
            if hi < limit {
                return Ok(true);
            }
            if lo >= limit {
                return Ok(false);
            }
        }
    }
    let distance = signed_distance(point, boundary, work)?;
    let half = cell.reach(point, work)?;
    Ok(if inside {
        (distance.lo - half).next_down() > limit
    } else {
        (distance.hi + half).next_up() < limit
    })
}

// La distanza firmata e' 1-Lipschitz: ogni cella copre l'intero segmento,
// non soltanto il campione centrale. Il margine trasporta le due omotopie.
pub(super) fn clearance(
    line: &geo::LineString,
    container: &Geometry,
    margin: f64,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    if !margin.is_finite() || margin < 0.0 {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    let container = Boundary::new(container, work)?;
    for edge in line.0.windows(2) {
        let mut pending = CountedVec::new();
        work.push(&mut pending, (0.0_f64, 1.0_f64, 0_u32))?;
        while let Some((lo, hi, depth)) = pending.pop() {
            work.step()?;
            let parameter = ErosionRange { lo, hi };
            let coordinate = |a, b| {
                ErosionRange::exact(a)
                    .add(parameter.mul(ErosionRange::exact(b).sub(ErosionRange::exact(a))))
            };
            let x = coordinate(edge[0].x, edge[1].x);
            let y = coordinate(edge[0].y, edge[1].y);
            let cell = ErosionCell {
                low: Coord { x: x.lo, y: y.lo },
                high: Coord { x: x.hi, y: y.hi },
            };
            let center = cell.center();
            let reach = cell.reach(center, work)?;
            let distance = signed_distance(center, &container, work)?;
            if distance.lo > (reach + margin).next_up() {
                continue;
            }
            let middle = lo.midpoint(hi);
            if distance.hi <= margin || depth >= 48 || middle == lo || middle == hi {
                return Err(BufferFailure::InvalidTopology.into());
            }
            work.push(&mut pending, (middle, hi, depth + 1))?;
            work.push(&mut pending, (lo, middle, depth + 1))?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct ErosionCell {
    low: Coord,
    high: Coord,
}

impl ErosionCell {
    const fn center(self) -> Coord {
        Coord {
            x: self.low.x.midpoint(self.high.x),
            y: self.low.y.midpoint(self.high.y),
        }
    }
    fn reach_filter(self, p: Coord) -> Option<ErosionRange> {
        let x = (p.x - self.low.x)
            .abs()
            .next_up()
            .max((p.x - self.high.x).abs().next_up());
        let y = (p.y - self.low.y)
            .abs()
            .next_up()
            .max((p.y - self.high.y).abs().next_up());
        let squared = ErosionRange::exact(x)
            .square()
            .add(ErosionRange::exact(y).square());
        let lo = squared.lo.max(0.0).sqrt().next_down().next_down().max(0.0);
        let hi = squared.hi.sqrt().next_up().next_up().next_up();
        (lo >= f64::MIN_POSITIVE && hi.is_finite() && hi < f64::MAX)
            .then_some(ErosionRange { lo, hi })
    }
    fn reach_le(self, p: Coord, limit: f64, work: &mut Work<'_>) -> Result<bool, UnionBufferError> {
        work.step()?;
        if let Some(range) = self.reach_filter(p) {
            if range.hi <= limit {
                return Ok(true);
            }
            if range.lo > limit {
                return Ok(false);
            }
        }
        Ok(self.reach(p, work)? <= limit)
    }
    fn reach(self, p: Coord, work: &mut Work<'_>) -> Result<f64, UnionBufferError> {
        let x = (p.x - self.low.x)
            .abs()
            .next_up()
            .max((p.x - self.high.x).abs().next_up());
        let y = (p.y - self.low.y)
            .abs()
            .next_up()
            .max((p.y - self.high.y).abs().next_up());
        let origin = Coord { x: 0.0, y: 0.0 };
        Ok(
            crate::segment_index::DistanceKey::coordinates(Coord { x, y }, origin, origin, work)?
                .rounded(work)?
                .next_up(),
        )
    }
}

/// Ogni cella e' esclusa dai due pieni oppure coperta da un testimone
/// interno a entrambi. La distanza firmata e' 1-Lipschitz sul piano.
pub(super) fn certify(
    operand: &Geometry,
    result: &Geometry,
    radius: f64,
    certificate: &ProjectedBufferCertificate<'_>,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    let error = (2.0 * (certificate.input_model_m + certificate.numerical_m).next_up()).next_up();
    let lower = ((radius - error).next_down() / certificate.inverse_scale_max).next_down();
    let upper = ((radius + error).next_up() / certificate.inverse_scale_min).next_up();
    if lower <= 0.0 {
        return Err(BufferFailure::AccuracyUnproven.into());
    }
    let reach = (certificate.stability_m / certificate.inverse_scale_max).next_down();
    let mut root = ErosionCell {
        low: Coord {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        high: Coord {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        },
    };
    for geometry in [operand, result] {
        for ring in parts(geometry)? {
            for p in &ring.0 {
                work.step()?;
                root.low.x = root.low.x.min(p.x);
                root.low.y = root.low.y.min(p.y);
                root.high.x = root.high.x.max(p.x);
                root.high.y = root.high.y.max(p.y);
            }
        }
    }
    let operand = Boundary::new(operand, work)?;
    let result = Boundary::new(result, work)?;
    let mut pending = CountedVec::new();
    work.push(&mut pending, root)?;
    while let Some(cell) = pending.pop() {
        work.step()?;
        let center = cell.center();
        if separated(center, &operand, cell, lower, false, work)?
            && separated(center, &result, cell, 0.0, false, work)?
        {
            continue;
        }
        if separated(center, &operand, cell, upper, true, work)?
            && separated(center, &result, cell, 0.0, true, work)?
        {
            continue;
        }
        if cell.reach_le(center, reach / 8.0, work)?
            && covered(cell, reach, upper, &operand, &result, work)?
        {
            continue;
        }
        if cell.reach_le(center, reach / 128.0, work)?
            || center.x == cell.low.x
            || center.x == cell.high.x
            || center.y == cell.low.y
            || center.y == cell.high.y
        {
            return Err(BufferFailure::AccuracyUnproven.into());
        }
        // Bisezione sull'asse maggiore: memoria viva proporzionale alla profondita'.
        let (mut left, mut right) = (cell, cell);
        if cell.high.x - cell.low.x >= cell.high.y - cell.low.y {
            left.high.x = center.x;
            right.low.x = center.x;
        } else {
            left.high.y = center.y;
            right.low.y = center.y;
        }
        work.push(&mut pending, right)?;
        work.push(&mut pending, left)?;
    }
    Ok(())
}

fn covered(
    cell: ErosionCell,
    reach: f64,
    upper: f64,
    operand: &Boundary<'_>,
    result: &Boundary<'_>,
    work: &mut Work<'_>,
) -> Result<bool, UnionBufferError> {
    let center = cell.center();
    // Direzioni intermedie nei settori acuti; la distanza euclidea
    // arrotondata verso l'alto certifica ogni candidato sull'intera cella.
    for scale in [0.0, 0.25, 0.5, 0.75, 0.875, 0.9375, 0.96875] {
        for index in 0..=64 {
            let x = f64::from(index) / 64.0;
            let y = 1.0 - x;
            for (x, y) in [(x, y), (-y, x), (-x, -y), (y, -x)] {
                work.step()?;
                let distance = reach * scale / x.hypot(y);
                let witness = Coord {
                    x: distance.mul_add(x, center.x),
                    y: distance.mul_add(y, center.y),
                };
                if cell.reach_le(witness, reach, work)?
                    && above(witness, operand, upper, true, work)?
                    && above(witness, result, 0.0, true, work)?
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use geo::{LineString, Polygon};

    #[test]
    fn filtered_decisions_match_exact_thresholds() {
        // C1: stessi confronti razionali, soglie uguali e IEEE adiacenti;
        // lati obliqui, foro, traslazione TM e scale fuori dal filtro.
        for scale in [f64::from_bits(16), 1e-200, 1.0, 1e150] {
            for shift in [0.0, 6e6] {
                if scale < 1.0 && shift != 0.0 {
                    continue;
                }
                let ring = |low: f64, high: f64| {
                    LineString::from(vec![
                        (low.mul_add(scale, shift), low.mul_add(scale, shift)),
                        (high.mul_add(scale, shift), low.mul_add(scale, shift)),
                        (
                            (high + 0.5).mul_add(scale, shift),
                            high.mul_add(scale, shift),
                        ),
                        (low.mul_add(scale, shift), high.mul_add(scale, shift)),
                        (low.mul_add(scale, shift), low.mul_add(scale, shift)),
                    ])
                };
                let geometry =
                    Geometry::Polygon(Polygon::new(ring(0.0, 4.0), vec![ring(1.0, 2.0)]));
                let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
                let boundary = Boundary::new(&geometry, &mut work).unwrap();
                for (x, y) in [
                    (-1.0_f64, 0.5_f64),
                    (0.5, 0.75),
                    (1.5, 1.5),
                    (3.0, 3.0),
                    (4.5, 4.0),
                ] {
                    let point = Coord {
                        x: x.mul_add(scale, shift),
                        y: y.mul_add(scale, shift),
                    };
                    let distance = signed_distance(point, &boundary, &mut work).unwrap();
                    if let Some(range) = distance_filter(point, &geometry, &mut work).unwrap() {
                        assert!(range.lo <= distance.lo && range.hi >= distance.hi);
                    }
                    for lower in [false, true] {
                        let value = if lower { distance.lo } else { distance.hi };
                        for limit in [
                            value.next_down(),
                            value,
                            value.next_up(),
                            -scale,
                            0.0,
                            scale,
                        ] {
                            assert_eq!(
                                above(point, &boundary, limit, lower, &mut work).unwrap(),
                                value > limit
                            );
                        }
                    }
                    let cell = ErosionCell {
                        low: point,
                        high: Coord {
                            x: point.x + scale / 8.0,
                            y: point.y + scale / 4.0,
                        },
                    };
                    let half = cell.reach(point, &mut work).unwrap();
                    for limit in [half.next_down(), half, half.next_up()] {
                        assert_eq!(
                            cell.reach_le(point, limit, &mut work).unwrap(),
                            half <= limit
                        );
                    }
                    for inside in [false, true] {
                        let value = if inside {
                            (distance.lo - half).next_down()
                        } else {
                            (distance.hi + half).next_up()
                        };
                        for limit in [value.next_down(), value, value.next_up()] {
                            assert_eq!(
                                separated(point, &boundary, cell, limit, inside, &mut work)
                                    .unwrap(),
                                if inside { value > limit } else { value < limit }
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn filters_preserve_extreme_rejections_and_work_limits() {
        // C1: underflow positivo, overflow e checkpoint prima del filtro.
        let tiny = f64::from_bits(1);
        let geometry = Geometry::Polygon(Polygon::new(
            LineString::from(vec![(0.0, 0.0), (tiny, 1.0), (1.0, 1.0), (0.0, 0.0)]),
            vec![],
        ));
        let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
        let boundary = Boundary::new(&geometry, &mut work).unwrap();
        let point = Coord { x: 0.0, y: tiny };
        assert!(matches!(
            signed_distance(point, &boundary, &mut work),
            Err(UnionBufferError::Overlay(
                crate::polygon_boolean::PolygonBooleanError::Predicate(
                    crate::predicates::PredicateError::AccuracyUnproven
                )
            ))
        ));
        assert!(matches!(
            above(point, &boundary, 1.0, true, &mut work),
            Err(UnionBufferError::Overlay(
                crate::polygon_boolean::PolygonBooleanError::Predicate(
                    crate::predicates::PredicateError::AccuracyUnproven
                )
            ))
        ));
        let cell = ErosionCell {
            low: Coord {
                x: -f64::MAX,
                y: 0.0,
            },
            high: Coord {
                x: f64::MAX,
                y: 1.0,
            },
        };
        assert!(matches!(
            cell.reach_le(cell.low, 1.0, &mut work),
            Err(UnionBufferError::Overlay(
                crate::polygon_boolean::PolygonBooleanError::Predicate(
                    crate::predicates::PredicateError::CalcoloNonConcluso("coordinata non finita")
                )
            ))
        ));
        let limits = crate::predicates::RelateLimits {
            max_steps: 0,
            ..Default::default()
        };
        let mut work = Work::new(limits, None);
        assert!(matches!(
            distance_filter(point, &geometry, &mut work),
            Err(UnionBufferError::Overlay(
                crate::polygon_boolean::PolygonBooleanError::Predicate(
                    crate::predicates::PredicateError::WorkLimit
                )
            ))
        ));
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn indexed_signed_distance_against_exhaustive_and_square() {
        // C1: distanza firmata analitica del quadrato; aritmetica IEEE
        // diretta della copertura, docs/buffer-proiezione.md.
        let square = Geometry::Polygon(Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (2.0, 0.0),
                (2.0, 2.0),
                (0.0, 2.0),
                (0.0, 0.0),
            ]),
            vec![],
        ));
        let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
        let boundary = Boundary::new(&square, &mut work).unwrap();
        for x in -4..=12 {
            for y in -4..=12 {
                let point = Coord {
                    x: f64::from(x) / 4.0,
                    y: f64::from(y) / 4.0,
                };
                let distance = signed_distance(point, &boundary, &mut work).unwrap();
                let oracle = signed_distance_impl(point, &boundary, &mut work, false).unwrap();
                assert_eq!((distance.lo, distance.hi), (oracle.lo, oracle.hi));
                let dx = (point.x - 1.0).abs() - 1.0;
                let dy = (point.y - 1.0).abs() - 1.0;
                let exact = if dx <= 0.0 && dy <= 0.0 {
                    -dx.max(dy)
                } else {
                    -dx.max(0.0).hypot(dy.max(0.0))
                };
                assert!(distance.lo <= exact && distance.hi >= exact);
            }
        }
    }
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn indexed_signed_distance_preserves_tiny_positive_clearance() {
        // C1: centro del quadrato, distanza esatta scale anche subnormale.
        for scale in [1e-200, f64::from_bits(16)] {
            let square = Geometry::Polygon(Polygon::new(
                LineString::from(vec![
                    (0.0, 0.0),
                    (2.0 * scale, 0.0),
                    (2.0 * scale, 2.0 * scale),
                    (0.0, 2.0 * scale),
                    (0.0, 0.0),
                ]),
                vec![],
            ));
            let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
            let boundary = Boundary::new(&square, &mut work).unwrap();
            let point = Coord { x: scale, y: scale };
            let distance = signed_distance(point, &boundary, &mut work).unwrap();
            let oracle = signed_distance_impl(point, &boundary, &mut work, false).unwrap();
            assert_eq!((distance.lo, distance.hi), (oracle.lo, oracle.hi));
            assert!(distance.lo > 0.0);
            assert!(distance.lo <= scale && distance.hi >= scale);
        }
    }

    #[test]
    #[deny(clippy::disallowed_methods)]
    fn acute_sector_has_a_continuous_common_witness() {
        // C1: triangolo analitico 0>=y>=-x/4; un testimone interno deve
        // evitare sia l'asse sia la diagonale. Copertura continua della guida.
        let triangle = Geometry::Polygon(Polygon::new(
            LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (1.0, -0.25), (0.0, 0.0)]),
            vec![],
        ));
        let cell = ErosionCell {
            low: Coord { x: -1e-6, y: -1e-6 },
            high: Coord { x: 1e-6, y: 1e-6 },
        };
        let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
        // C1: triangolo rettangolo 3-4-5, anche traslato nel dominio TM.
        for shift in [0.0, 6e6] {
            let rectangle = ErosionCell {
                low: Coord { x: shift, y: shift },
                high: Coord {
                    x: shift + 3.0,
                    y: shift + 4.0,
                },
            };
            let bound = rectangle
                .reach(Coord { x: shift, y: shift }, &mut work)
                .unwrap();
            assert!((5.0..=5.0 + 1e-12).contains(&bound));
            let bound = rectangle.reach(rectangle.center(), &mut work).unwrap();
            assert!((2.5..=2.5 + 1e-12).contains(&bound));
        }
        let witness = Coord {
            x: 0.00328,
            y: -0.00047,
        };
        assert!(cell.reach(witness, &mut work).unwrap() < 0.005);
        assert!(-witness.y > 0.0003);
        assert!((witness.y + witness.x / 4.0) / (1.0625_f64).sqrt() > 0.0003);
        let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
        let boundary = Boundary::new(&triangle, &mut work).unwrap();
        assert!(covered(cell, 0.005, 0.0003, &boundary, &boundary, &mut work).unwrap());
    }
}
