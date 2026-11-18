//! Validazione metrica delle superfici geodetiche GOP-005.
#![allow(
    clippy::suboptimal_flops,
    reason = "Ordine delle maggiorazioni numeriche"
)]
#![allow(clippy::float_cmp, reason = "Identita geodetiche esatte")]
use super::{GeodesicDensifyError, GeodesicGeometryDensifyLimits};
use crate::geodetica::EllissoideGeodetico;
use geo::{Coord, Geometry, LineString, Polygon};
use geographiclib_rs::{DirectGeodesic, InverseGeodesic};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
mod surface;
const GAP_M: f64 = 0.004;
const ROUND_M: f64 = 1e-6;
struct Work<'a> {
    limits: GeodesicGeometryDensifyLimits,
    cancel: Option<&'a AtomicBool>,
    count: u64,
    bytes: u64,
    peak: u64,
    ellipsoid: &'a EllissoideGeodetico,
}

impl Work<'_> {
    fn checkpoint(&self) -> Result<(), GeodesicDensifyError> {
        if self
            .limits
            .output
            .deadline
            .is_some_and(|end| Instant::now() >= end)
        {
            return Err(GeodesicDensifyError::DeadlineExceeded);
        }
        if self
            .cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        {
            return Err(GeodesicDensifyError::Cancelled);
        }
        Ok(())
    }

    fn step(&mut self) -> Result<(), GeodesicDensifyError> {
        self.checkpoint()?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or(GeodesicDensifyError::WorkLimit)?;
        if self.count > self.limits.max_validation_work {
            return Err(GeodesicDensifyError::WorkLimit);
        }
        Ok(())
    }

    fn reserve(&mut self, bytes: usize) -> Result<(), GeodesicDensifyError> {
        self.checkpoint()?;
        let bytes = u64::try_from(bytes).map_err(|_| GeodesicDensifyError::MemoryLimit)?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(GeodesicDensifyError::MemoryLimit)?;
        if self.bytes > self.limits.output.max_reserved_bytes {
            return Err(GeodesicDensifyError::MemoryLimit);
        }
        self.peak = self.peak.max(self.bytes);
        Ok(())
    }

    fn push<T>(&mut self, values: &mut Vec<T>, value: T) -> Result<(), GeodesicDensifyError> {
        self.checkpoint()?;
        if values.len() == values.capacity() {
            let growth = values.capacity().max(1);
            self.reserve(
                growth
                    .checked_mul(size_of::<T>())
                    .ok_or(GeodesicDensifyError::MemoryLimit)?,
            )?;
            values
                .try_reserve_exact(growth)
                .map_err(|_| GeodesicDensifyError::Allocation)?;
        }
        values.push(value);
        Ok(())
    }

    fn inverse(
        &mut self,
        a: Coord,
        b: Coord,
    ) -> Result<(f64, f64, f64, f64), GeodesicDensifyError> {
        self.step()?;
        let result: (f64, f64, f64, f64) = self.ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
        if !(result.0.is_finite()
            && result.1.is_finite()
            && result.2.is_finite()
            && result.3.is_finite())
        {
            return Err(GeodesicDensifyError::AccuracyUnproven);
        }
        Ok(result)
    }

    fn at(&mut self, arc: ArcPiece, offset: f64) -> Result<Coord, GeodesicDensifyError> {
        if offset == 0.0 {
            return Ok(arc.origin);
        }
        self.step()?;
        let (lat, lon): (f64, f64) =
            self.ellipsoid
                .geodetica()
                .direct(arc.origin.y, arc.origin.x, arc.azimuth, offset);
        coordinate(Coord { x: lon, y: lat })
    }
}

fn coordinate(point: Coord) -> Result<Coord, GeodesicDensifyError> {
    if !((-180.0..=180.0).contains(&point.x) && (-90.0..=90.0).contains(&point.y)) {
        return Err(GeodesicDensifyError::InvalidGeometry);
    }
    Ok(point)
}

#[derive(Clone, Copy, Debug)]
struct ArcPiece {
    origin: Coord,
    azimuth: f64,
    lo: f64,
    hi: f64,
}

#[derive(Clone, Copy, Debug)]
struct MetricBall {
    centre: Coord,
    radius: f64,
    arc: ArcPiece,
    children: Option<(usize, usize)>,
}

impl MetricBall {
    const fn point(point: Coord) -> Self {
        Self {
            centre: point,
            radius: 0.0,
            arc: ArcPiece {
                origin: point,
                azimuth: 0.0,
                lo: 0.0,
                hi: 0.0,
            },
            children: None,
        }
    }

    fn piece(arc: ArcPiece, work: &mut Work<'_>) -> Result<Self, GeodesicDensifyError> {
        work.step()?;
        let middle = (arc.hi - arc.lo).mul_add(0.5, arc.lo);
        let (lat, lon, tangent): (f64, f64, f64) =
            work.ellipsoid
                .geodetica()
                .direct(arc.origin.y, arc.origin.x, arc.azimuth, middle);
        if !tangent.is_finite() {
            return Err(GeodesicDensifyError::AccuracyUnproven);
        }
        Ok(Self {
            centre: coordinate(Coord { x: lon, y: lat })?,
            radius: (arc.hi - arc.lo) * 0.5 + ROUND_M,
            arc,
            children: None,
        })
    }

    fn split(
        self,
        nodes: &[Self],
        work: &mut Work<'_>,
    ) -> Result<(Self, Self), GeodesicDensifyError> {
        work.step()?;
        if let Some((left, right)) = self.children {
            return Ok((nodes[left], nodes[right]));
        }
        let mid = self.arc.lo + (self.arc.hi - self.arc.lo) * 0.5;
        if mid <= self.arc.lo || mid >= self.arc.hi {
            return Err(GeodesicDensifyError::AccuracyUnproven);
        }
        Ok((
            Self::piece(
                ArcPiece {
                    hi: mid,
                    ..self.arc
                },
                work,
            )?,
            Self::piece(
                ArcPiece {
                    lo: mid,
                    ..self.arc
                },
                work,
            )?,
        ))
    }
}

fn edge(a: Coord, b: Coord, work: &mut Work<'_>) -> Result<MetricBall, GeodesicDensifyError> {
    let (length, azimuth, final_azimuth, _) = work.inverse(a, b)?;
    if length == 0.0 {
        return Ok(MetricBall::point(a));
    }
    // G2 Multiple shortest geodesics, ellissoide oblato.
    if a.y == -b.y && (a.y.abs() == 90.0 || azimuth != final_azimuth) {
        return Err(GeodesicDensifyError::AmbiguousGeodesic);
    }
    let gap = (a.x - b.x).abs();
    if gap.min(360.0 - gap) > 178.0
        && (a.y + b.y).abs() < 1e-12
        && a.y != -b.y
        && azimuth != final_azimuth
    {
        return Err(GeodesicDensifyError::AccuracyUnproven);
    }
    MetricBall::piece(
        ArcPiece {
            origin: a,
            azimuth,
            lo: 0.0,
            hi: length,
        },
        work,
    )
}

struct GeometryIndex {
    nodes: Vec<MetricBall>,
    rings: Vec<surface::SurfaceRing>,
}

impl GeometryIndex {
    fn line(
        &mut self,
        line: &LineString,
        ring: bool,
        work: &mut Work<'_>,
    ) -> Result<(), GeodesicDensifyError> {
        if line.0.len() < 2 {
            return Err(GeodesicDensifyError::InvalidGeometry);
        }
        for &point in &line.0 {
            work.step()?;
            coordinate(point)?;
        }
        for pair in line.0.windows(2) {
            let ball = edge(pair[0], pair[1], work)?;
            if ring {
                surface::append_short_pieces(ball, &mut self.nodes, work, 0)?;
            } else {
                work.push(&mut self.nodes, ball)?;
            }
        }
        Ok(())
    }

    fn polygon(
        &mut self,
        polygon: &Polygon,
        work: &mut Work<'_>,
    ) -> Result<(), GeodesicDensifyError> {
        work.step()?;
        if polygon.exterior().0.is_empty() {
            return if polygon.interiors().is_empty() {
                Ok(())
            } else {
                Err(GeodesicDensifyError::InvalidGeometry)
            };
        }
        let shell = self.rings.len();
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let begin = self.nodes.len();
            self.line(ring, true, work)?;
            let area = surface::ring_area(ring, work)?;
            work.push(
                &mut self.rings,
                surface::SurfaceRing {
                    begin,
                    end: self.nodes.len(),
                    area,
                    shell,
                },
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    a: MetricBall,
    b: MetricBall,
    lower: f64,
    serial: u64,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .lower
            .total_cmp(&self.lower)
            .then_with(|| other.serial.cmp(&self.serial))
    }
}

struct SearchQueue {
    heap: BinaryHeap<Candidate>,
    serial: u64,
    best: f64,
    separation_only: bool,
}

impl SearchQueue {
    fn add(
        &mut self,
        a: MetricBall,
        b: MetricBall,
        work: &mut Work<'_>,
    ) -> Result<(), GeodesicDensifyError> {
        let (distance, _, _, _) = work.inverse(a.centre, b.centre)?;
        self.best = self.best.min(distance + 4.0 * ROUND_M);
        let lower = (distance - a.radius - b.radius - 4.0 * ROUND_M).max(0.0);
        if if self.separation_only {
            lower > GAP_M
        } else {
            lower >= self.best - GAP_M
        } {
            return Ok(());
        }
        if self.heap.len() == self.heap.capacity() {
            let growth = self.heap.capacity().max(1);
            work.reserve(
                growth
                    .checked_mul(size_of::<Candidate>())
                    .ok_or(GeodesicDensifyError::MemoryLimit)?,
            )?;
            self.heap
                .try_reserve_exact(growth)
                .map_err(|_| GeodesicDensifyError::Allocation)?;
        }
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or(GeodesicDensifyError::WorkLimit)?;
        self.heap.push(Candidate {
            a,
            b,
            lower,
            serial: self.serial,
        });
        Ok(())
    }
}

fn minimum(
    a: MetricBall,
    b: MetricBall,
    left: &[MetricBall],
    right: &[MetricBall],
    separation_only: bool,
    work: &mut Work<'_>,
) -> Result<f64, GeodesicDensifyError> {
    let before = work.bytes;
    let mut queue = SearchQueue {
        heap: BinaryHeap::new(),
        serial: 0,
        best: f64::INFINITY,
        separation_only,
    };
    queue.add(a, b, work)?;
    while let Some(candidate) = queue.heap.pop() {
        work.step()?;
        if queue.best <= if separation_only { 2.0 * GAP_M } else { GAP_M } {
            break;
        }
        if !separation_only && candidate.lower >= queue.best - GAP_M {
            continue;
        }
        if separation_only && candidate.lower > GAP_M {
            continue;
        }
        if candidate.a.radius >= candidate.b.radius {
            let (one, two) = candidate.a.split(left, work)?;
            queue.add(one, candidate.b, work)?;
            queue.add(two, candidate.b, work)?;
        } else {
            let (one, two) = candidate.b.split(right, work)?;
            queue.add(candidate.a, one, work)?;
            queue.add(candidate.a, two, work)?;
        }
    }
    let best = queue.best;
    drop(queue);
    work.bytes = before;
    Ok(
        if best <= if separation_only { 2.0 * GAP_M } else { GAP_M } {
            0.0
        } else {
            best
        },
    )
}

pub(super) fn validate(
    geometry: &Geometry,
    ellipsoid: &EllissoideGeodetico,
    limits: GeodesicGeometryDensifyLimits,
    cancel: Option<&AtomicBool>,
) -> Result<u64, GeodesicDensifyError> {
    let mut work = Work {
        limits,
        cancel,
        count: 0,
        bytes: 0,
        peak: 0,
        ellipsoid,
    };
    let mut index = GeometryIndex {
        nodes: Vec::new(),
        rings: Vec::new(),
    };
    match geometry {
        Geometry::Polygon(polygon) => index.polygon(polygon, &mut work)?,
        Geometry::MultiPolygon(polygons) => {
            for polygon in polygons {
                index.polygon(polygon, &mut work)?;
            }
        }
        _ => return Ok(0),
    }
    surface::validate(&index, &mut work)?;
    work.checkpoint()?;
    Ok(work.count)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::geodesic_densify::{geodesic_densify_geometry_with_limits, GeodesicDensifyLimits};
    use plenora_core::crs::resolve_crs;

    #[test]
    fn exact_live_memory_and_cumulative_validation_work() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
        let polygon = Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 1.0),
                (0.0, 1.0),
                (0.0, 0.0),
            ]),
            vec![],
        );
        let limits = GeodesicGeometryDensifyLimits {
            output: GeodesicDensifyLimits {
                max_points: 5,
                max_reserved_bytes: 1_000_000,
                deadline: None,
            },
            max_validation_work: 1_000_000,
        };
        let mut work = Work {
            limits,
            cancel: None,
            count: 0,
            bytes: 0,
            peak: 0,
            ellipsoid: &ellipsoid,
        };
        let mut index = GeometryIndex {
            nodes: Vec::new(),
            rings: Vec::new(),
        };
        index.polygon(&polygon, &mut work).unwrap();
        surface::validate(&index, &mut work).unwrap();
        // GOP-026/027: due validazioni identiche senza suddivisioni; al secondo
        // picco e' vivo anche l'output di cinque Coord, nessun Vec di buchi.
        let exact = GeodesicGeometryDensifyLimits {
            output: GeodesicDensifyLimits {
                max_reserved_bytes: work.peak + 5 * size_of::<Coord>() as u64,
                ..limits.output
            },
            max_validation_work: 2 * work.count,
        };
        let input = Geometry::Polygon(polygon);
        let run = |budget| geodesic_densify_geometry_with_limits(&input, &crs, 1e6, budget, None);
        assert_eq!(run(exact).unwrap(), input);
        assert_eq!(
            run(GeodesicGeometryDensifyLimits {
                max_validation_work: exact.max_validation_work - 1,
                ..exact
            }),
            Err(GeodesicDensifyError::WorkLimit)
        );
        assert_eq!(
            run(GeodesicGeometryDensifyLimits {
                output: GeodesicDensifyLimits {
                    max_reserved_bytes: exact.output.max_reserved_bytes - 1,
                    ..exact.output
                },
                ..exact
            }),
            Err(GeodesicDensifyError::MemoryLimit)
        );
    }
}
