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

fn signed_distance(
    point: Coord,
    geometry: &Geometry,
    work: &mut Work<'_>,
) -> Result<ErosionRange, UnionBufferError> {
    let mut distance = ErosionRange::exact(f64::INFINITY);
    let mut inside = false;
    let mut uncertain = false;
    for ring in parts(geometry)? {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::signed_distance:1");
        work.step()?;
        for edge in ring.0.windows(2) {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::signed_distance:2");
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
            let t = px
                .mul(dx)
                .add(py.mul(dy))
                .divide(dx.square().add(dy.square()))?;
            let t = ErosionRange {
                lo: t.lo.clamp(0.0, 1.0),
                hi: t.hi.clamp(0.0, 1.0),
            };
            let squared = px.sub(t.mul(dx)).square().add(py.sub(t.mul(dy)).square());
            if !squared.lo.is_finite() || !squared.hi.is_finite() {
                return Err(BufferFailure::AccuracyUnproven.into());
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
    for edge in line.0.windows(2) {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::clearance:1");
        work.checkpoint()?;
        let mut pending = CountedVec::new();
        work.push(&mut pending, (0.0_f64, 1.0_f64, 0_u32))?;
        while let Some((lo, hi, depth)) = pending.pop() {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::clearance:2");
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
            let reach = cell.reach(center);
            let distance = signed_distance(center, container, work)?;
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
    fn reach(self, p: Coord) -> f64 {
        let x = (p.x - self.low.x)
            .abs()
            .next_up()
            .max((p.x - self.high.x).abs().next_up());
        let y = (p.y - self.low.y)
            .abs()
            .next_up()
            .max((p.y - self.high.y).abs().next_up());
        ((x * x).next_up() + (y * y).next_up())
            .next_up()
            .sqrt()
            .next_up()
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
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::certify:1");
        for ring in parts(geometry)? {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::certify:2");
            work.step()?;
            for p in &ring.0 {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::certify:3");
                work.step()?;
                root.low.x = root.low.x.min(p.x);
                root.low.y = root.low.y.min(p.y);
                root.high.x = root.high.x.max(p.x);
                root.high.y = root.high.y.max(p.y);
            }
        }
    }
    let mut pending = CountedVec::new();
    work.push(&mut pending, root)?;
    while let Some(cell) = pending.pop() {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::certify:4");
        work.step()?;
        let center = cell.center();
        let half = cell.reach(center);
        let source = signed_distance(center, operand, work)?;
        let output = signed_distance(center, result, work)?;
        if (source.hi + half).next_up() < lower && (output.hi + half).next_up() < 0.0 {
            continue;
        }
        if (source.lo - half).next_down() > upper && (output.lo - half).next_down() > 0.0 {
            continue;
        }
        if half <= reach / 8.0 && covered(cell, reach, upper, operand, result, work)? {
            continue;
        }
        if half <= reach / 128.0
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
    operand: &Geometry,
    result: &Geometry,
    work: &mut Work<'_>,
) -> Result<bool, UnionBufferError> {
    let center = cell.center();
    // Direzioni intermedie nei settori acuti; la distanza euclidea
    // arrotondata verso l'alto certifica ogni candidato sull'intera cella.
    for scale in [0.0, 0.25, 0.5, 0.75, 0.875, 0.9375, 0.96875] {
        #[cfg(test)]
        crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::covered:1");
        for index in 0..=64 {
            #[cfg(test)]
            crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::covered:2");
            let x = f64::from(index) / 64.0;
            let y = 1.0 - x;
            for (x, y) in [(x, y), (-y, x), (-x, -y), (y, -x)] {
                #[cfg(test)]
                crate::noding::gop028::cycle("geodesic_buffer/projected/erosion.rs::covered:3");
                work.step()?;
                let distance = reach * scale / x.hypot(y);
                let witness = Coord {
                    x: distance.mul_add(x, center.x),
                    y: distance.mul_add(y, center.y),
                };
                if cell.reach(witness) <= reach
                    && signed_distance(witness, operand, work)?.lo > upper
                    && signed_distance(witness, result, work)?.lo > 0.0
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
    #[deny(clippy::disallowed_methods)]
    fn interval_signed_distance_against_square_and_subnormal_rejection() {
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
        for x in -4..=12 {
            for y in -4..=12 {
                let point = Coord {
                    x: f64::from(x) / 4.0,
                    y: f64::from(y) / 4.0,
                };
                let distance = signed_distance(point, &square, &mut work).unwrap();
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
        assert!(matches!(
            ErosionRange::exact(0.0).divide(ErosionRange::exact(f64::from_bits(1))),
            Err(UnionBufferError::Disk(BufferFailure::AccuracyUnproven))
        ));
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
        // C1: triangolo rettangolo 3-4-5, anche traslato nel dominio TM.
        for shift in [0.0, 6e6] {
            let rectangle = ErosionCell {
                low: Coord { x: shift, y: shift },
                high: Coord {
                    x: shift + 3.0,
                    y: shift + 4.0,
                },
            };
            let bound = rectangle.reach(Coord { x: shift, y: shift });
            assert!((5.0..=5.0 + 1e-12).contains(&bound));
            let bound = rectangle.reach(rectangle.center());
            assert!((2.5..=2.5 + 1e-12).contains(&bound));
        }
        let witness = Coord {
            x: 0.00328,
            y: -0.00047,
        };
        assert!(cell.reach(witness) < 0.005);
        assert!(-witness.y > 0.0003);
        assert!((witness.y + witness.x / 4.0) / (1.0625_f64).sqrt() > 0.0003);
        let mut work = Work::new(crate::predicates::RelateLimits::default(), None);
        assert!(covered(cell, 0.005, 0.0003, &triangle, &triangle, &mut work).unwrap());
    }
}
