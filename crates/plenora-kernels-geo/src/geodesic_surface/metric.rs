//! Indice metrico e ricerca certificata comuni alle superfici e alla distanza.
#![allow(
    clippy::redundant_pub_crate,
    reason = "Visibilita esplicita del supporto condiviso interno al crate"
)]
#![allow(
    clippy::suboptimal_flops,
    reason = "Ordine delle maggiorazioni numeriche"
)]
#![allow(clippy::float_cmp, reason = "Identita geodetiche esatte")]
use super as surface;
use crate::geodesic_distance::{GeodesicDistanceError, GeodesicDistanceLimits};
use crate::geodetica::EllissoideGeodetico;
use geo::{Coord, Geometry, LineString, Polygon};
use geographiclib_rs::{DirectGeodesic, InverseGeodesic};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::atomic::AtomicBool;
pub(crate) const GAP_M: f64 = 0.004;
// G2, Additional properties e Background:
// https://geographiclib.sourceforge.io/html/python/geodesics.html
// La perturbazione trasversa e' m12*dazi. Un residuo agli estremi di
// 32 nm amplificato di al piu' 7.1e6/1024 resta sotto 0.25 mm lungo
// l'arco. edge rifiuta m12<1024 m sui lati lunghi; 0.5 mm copre anche
// direct e aritmetica; il margine effettivo cresce da ROUND_M con 1/m12.
// Le inverse fra soli punti non richiedono un ramo.
pub(crate) const ROUND_M: f64 = 1e-6;

pub(crate) struct Work<'a> {
    limits: GeodesicDistanceLimits,
    cancel: Option<&'a AtomicBool>,
    pub(crate) count: u64,
    pub(crate) bytes: u64,
    pub(crate) peak: u64,
    pub(crate) ellipsoid: EllissoideGeodetico,
    pub(crate) taylor: bool,
    pub(crate) position_margin: f64,
}

impl<'a> Work<'a> {
    pub(crate) const fn new(
        limits: GeodesicDistanceLimits,
        cancel: Option<&'a AtomicBool>,
        ellipsoid: &EllissoideGeodetico,
    ) -> Self {
        Self {
            limits,
            cancel,
            count: 0,
            bytes: 0,
            peak: 0,
            ellipsoid: *ellipsoid,
            taylor: true,
            position_margin: ROUND_M,
        }
    }

    pub(crate) fn checkpoint(&self) -> Result<(), GeodesicDistanceError> {
        crate::noding::check_interruption(
            self.limits.deadline,
            self.cancel,
            GeodesicDistanceError::DeadlineExceeded,
            GeodesicDistanceError::Cancelled,
        )?;
        Ok(())
    }

    pub(crate) fn step(&mut self) -> Result<(), GeodesicDistanceError> {
        self.checkpoint()?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or(GeodesicDistanceError::WorkLimit)?;
        if self.count > self.limits.max_work {
            return Err(GeodesicDistanceError::WorkLimit);
        }
        Ok(())
    }

    pub(crate) fn reserve(&mut self, bytes: usize) -> Result<(), GeodesicDistanceError> {
        self.checkpoint()?;
        let bytes = u64::try_from(bytes).map_err(|_| GeodesicDistanceError::MemoryLimit)?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(GeodesicDistanceError::MemoryLimit)?;
        if self.bytes > self.limits.max_reserved_bytes {
            return Err(GeodesicDistanceError::MemoryLimit);
        }
        self.peak = self.peak.max(self.bytes);
        Ok(())
    }

    pub(crate) fn push<T>(
        &mut self,
        values: &mut Vec<T>,
        value: T,
    ) -> Result<(), GeodesicDistanceError> {
        self.checkpoint()?;
        if values.len() == values.capacity() {
            let growth = values.capacity().max(1);
            self.reserve(
                growth
                    .checked_mul(size_of::<T>())
                    .ok_or(GeodesicDistanceError::MemoryLimit)?,
            )?;
            values
                .try_reserve_exact(growth)
                .map_err(|_| GeodesicDistanceError::Allocation)?;
        }
        values.push(value);
        Ok(())
    }

    pub(crate) fn inverse(
        &mut self,
        a: Coord,
        b: Coord,
    ) -> Result<(f64, f64, f64, f64), GeodesicDistanceError> {
        self.step()?;
        #[cfg(test)]
        crate::noding::gop028::unit();
        let result: (f64, f64, f64, f64) = self.ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
        if !(result.0.is_finite()
            && result.1.is_finite()
            && result.2.is_finite()
            && result.3.is_finite())
        {
            return Err(GeodesicDistanceError::AccuracyUnproven);
        }
        Ok(result)
    }

    pub(crate) fn at(
        &mut self,
        arc: ArcPiece,
        offset: f64,
    ) -> Result<Coord, GeodesicDistanceError> {
        if offset == 0.0 {
            return Ok(arc.origin);
        }
        if offset == arc.length {
            return Ok(arc.end);
        }
        self.step()?;
        #[cfg(test)]
        crate::noding::gop028::unit();
        let (lat, lon): (f64, f64) =
            self.ellipsoid
                .geodetica()
                .direct(arc.origin.y, arc.origin.x, arc.azimuth, offset);
        coordinate(Coord { x: lon, y: lat }).map_err(|_| GeodesicDistanceError::AccuracyUnproven)
    }
}

pub(crate) fn coordinate(point: Coord) -> Result<Coord, GeodesicDistanceError> {
    if !((-180.0..=180.0).contains(&point.x) && (-90.0..=90.0).contains(&point.y)) {
        return Err(GeodesicDistanceError::InvalidGeometry);
    }
    Ok(point)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ArcPiece {
    pub(crate) origin: Coord,
    pub(crate) end: Coord,
    pub(crate) length: f64,
    pub(crate) azimuth: f64,
    pub(crate) lo: f64,
    pub(crate) hi: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MetricBall {
    pub(crate) centre: Coord,
    pub(crate) radius: f64,
    pub(crate) tangent: f64,
    pub(crate) arc: ArcPiece,
    pub(crate) children: Option<(usize, usize)>,
}

impl MetricBall {
    pub(crate) const fn point(point: Coord) -> Self {
        Self {
            centre: point,
            radius: 0.0,
            tangent: 0.0,
            arc: ArcPiece {
                origin: point,
                end: point,
                length: 0.0,
                azimuth: 0.0,
                lo: 0.0,
                hi: 0.0,
            },
            children: None,
        }
    }

    pub(crate) fn piece(arc: ArcPiece, work: &mut Work<'_>) -> Result<Self, GeodesicDistanceError> {
        work.step()?;
        let middle = (arc.hi - arc.lo).mul_add(0.5, arc.lo);
        let (lat, lon, tangent): (f64, f64, f64) =
            work.ellipsoid
                .geodetica()
                .direct(arc.origin.y, arc.origin.x, arc.azimuth, middle);
        if !tangent.is_finite() {
            return Err(GeodesicDistanceError::AccuracyUnproven);
        }
        Ok(Self {
            centre: coordinate(Coord { x: lon, y: lat })
                .map_err(|_| GeodesicDistanceError::AccuracyUnproven)?,
            tangent,
            radius: (arc.hi - arc.lo) * 0.5 + work.position_margin,
            arc,
            children: None,
        })
    }

    pub(crate) fn split(
        self,
        nodes: &[Self],
        work: &mut Work<'_>,
    ) -> Result<(Self, Self), GeodesicDistanceError> {
        work.step()?;
        if let Some((left, right)) = self.children {
            return Ok((nodes[left], nodes[right]));
        }
        let mid = self.arc.lo + (self.arc.hi - self.arc.lo) * 0.5;
        if mid <= self.arc.lo || mid >= self.arc.hi {
            return Err(GeodesicDistanceError::AccuracyUnproven);
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

pub(crate) fn edge(
    a: Coord,
    b: Coord,
    work: &mut Work<'_>,
) -> Result<MetricBall, GeodesicDistanceError> {
    let (length, azimuth, final_azimuth, _) = work.inverse(a, b)?;
    if length == 0.0 {
        return Ok(MetricBall::point(a));
    }
    // G2 Multiple shortest geodesics, ellissoide oblato.
    if a.y == -b.y && (a.y.abs() == 90.0 || azimuth != final_azimuth) {
        return Err(GeodesicDistanceError::AmbiguousGeodesic);
    }
    let gap = (a.x - b.x).abs();
    if gap.min(360.0 - gap) > 178.0
        && (a.y + b.y).abs() < 1e-12
        && a.y != -b.y
        && azimuth != final_azimuth
    {
        return Err(GeodesicDistanceError::AccuracyUnproven);
    }
    if length > 1_000_000.0 {
        work.step()?;
        let (_, _, _, reduced, _, _, _, _): (f64, f64, f64, f64, f64, f64, f64, f64) =
            work.ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
        if !reduced.is_finite() || reduced < 1024.0 {
            return Err(GeodesicDistanceError::AccuracyUnproven);
        }
        work.position_margin = work.position_margin.max(32e-9 * 7.1e6 / reduced + ROUND_M);
    }
    MetricBall::piece(
        ArcPiece {
            origin: a,
            end: b,
            length,
            azimuth,
            lo: 0.0,
            hi: length,
        },
        work,
    )
}

pub(crate) struct GeometryIndex {
    pub(crate) nodes: Vec<MetricBall>,
    pub(crate) rings: Vec<surface::SurfaceRing>,
    pub(crate) representatives: Vec<Coord>,
    pub(crate) root: usize,
}

impl GeometryIndex {
    pub(crate) fn line(
        &mut self,
        line: &LineString,
        ring: bool,
        work: &mut Work<'_>,
    ) -> Result<(), GeodesicDistanceError> {
        if line.0.len() < 2 {
            return Err(GeodesicDistanceError::InvalidGeometry);
        }
        for &point in &line.0 {
            work.step()?;
            coordinate(point)?;
        }
        work.push(&mut self.representatives, line.0[0])?;
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

    pub(crate) fn polygon(
        &mut self,
        polygon: &Polygon,
        work: &mut Work<'_>,
    ) -> Result<(), GeodesicDistanceError> {
        let shell = self.rings.len();
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let begin = self.nodes.len();
            self.line(ring, true, work)?;
            if self.nodes.len() - begin < 3 {
                return Err(GeodesicDistanceError::UnsupportedSurface);
            }
            let orientation = surface::ring_area(ring, work)?;
            work.push(
                &mut self.rings,
                surface::SurfaceRing {
                    begin,
                    end: self.nodes.len(),
                    orientation,
                    shell,
                },
            )?;
        }
        Ok(())
    }

    pub(crate) fn new(
        geometry: &Geometry,
        work: &mut Work<'_>,
    ) -> Result<Self, GeodesicDistanceError> {
        let mut index = Self {
            nodes: Vec::new(),
            rings: Vec::new(),
            representatives: Vec::new(),
            root: 0,
        };
        match geometry {
            Geometry::Point(point) => {
                work.step()?;
                work.push(&mut index.nodes, MetricBall::point(coordinate(point.0)?))?;
                work.push(&mut index.representatives, point.0)?;
            }
            Geometry::MultiPoint(points) => {
                for point in points {
                    work.step()?;
                    work.push(&mut index.nodes, MetricBall::point(coordinate(point.0)?))?;
                    work.push(&mut index.representatives, point.0)?;
                }
            }
            Geometry::LineString(line) => index.line(line, false, work)?,
            Geometry::MultiLineString(lines) => {
                for line in lines {
                    index.line(line, false, work)?;
                }
            }
            Geometry::Polygon(polygon) => index.polygon(polygon, work)?,
            Geometry::MultiPolygon(polygons) => {
                for polygon in polygons {
                    index.polygon(polygon, work)?;
                }
            }
            _ => return Err(GeodesicDistanceError::UnsupportedGeometry),
        }
        if index.nodes.is_empty() {
            return Err(GeodesicDistanceError::EmptyGeometry);
        }
        index.root = index.branch(0, index.nodes.len(), work)?;
        surface::validate(&index, work)?;
        Ok(index)
    }

    pub(crate) fn branch(
        &mut self,
        begin: usize,
        end: usize,
        work: &mut Work<'_>,
    ) -> Result<usize, GeodesicDistanceError> {
        work.step()?;
        if end - begin == 1 {
            return Ok(begin);
        }
        let middle = begin + (end - begin) / 2;
        let left = self.branch(begin, middle, work)?;
        let right = self.branch(middle, end, work)?;
        let a = self.nodes[left];
        let b = self.nodes[right];
        let radius = a
            .radius
            .max(work.inverse(a.centre, b.centre)?.0 + b.radius + work.position_margin);
        let position = self.nodes.len();
        work.push(
            &mut self.nodes,
            MetricBall {
                radius,
                children: Some((left, right)),
                ..a
            },
        )?;
        Ok(position)
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

// G1, equazioni della deviazione geodetica: la Hessiana della distanza
// fra due geodetiche ha coefficienti M12/m12, M21/m12 e -1/m12,
// moltiplicati per seni delle tangenti. Nel dominio d<1000 km,
// 0<K<1/(5.9e6)^2 implica 0<M<=1 e m>=R*sin(d/R)>0.99*d.
// Per ogni punto della cella d>=lower: il resto di Taylor e' quindi
// <= (r1+r2)^2/lower; il fattore 2 maggiora anche il round-off.
// Oltre questo dominio rimane il bound metrico senza ipotesi locali.
pub(crate) fn taylor_lower(
    a: MetricBall,
    b: MetricBall,
    distance: f64,
    azi1: f64,
    azi2: f64,
    lower: f64,
    margin: f64,
) -> f64 {
    let derivative_error = 16.0 * margin / lower + 1e-9;
    let da = (a.tangent - azi1).to_radians().cos().abs() + derivative_error;
    let db = (b.tangent - azi2).to_radians().cos().abs() + derivative_error;
    distance
        - da * a.radius
        - db * b.radius
        - 2.0 * (a.radius + b.radius).powi(2) / lower
        - 8.0 * margin
}

impl SearchQueue {
    fn add(
        &mut self,
        a: MetricBall,
        b: MetricBall,
        work: &mut Work<'_>,
    ) -> Result<(), GeodesicDistanceError> {
        let (distance, azi1, azi2, _) = work.inverse(a.centre, b.centre)?;
        self.best = self.best.min(distance + 4.0 * work.position_margin);
        let mut lower = (distance - a.radius - b.radius - 4.0 * work.position_margin).max(0.0);
        if work.taylor
            && a.children.is_none()
            && b.children.is_none()
            && lower > 1.0
            && distance + a.radius + b.radius < 1_000_000.0
        {
            lower = lower.max(taylor_lower(
                a,
                b,
                distance,
                azi1,
                azi2,
                lower,
                work.position_margin,
            ));
        }
        if if self.separation_only {
            lower > 16.0 * work.position_margin
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
                    .ok_or(GeodesicDistanceError::MemoryLimit)?,
            )?;
            self.heap
                .try_reserve_exact(growth)
                .map_err(|_| GeodesicDistanceError::Allocation)?;
        }
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or(GeodesicDistanceError::WorkLimit)?;
        self.heap.push(Candidate {
            a,
            b,
            lower,
            serial: self.serial,
        });
        Ok(())
    }
}

pub(crate) fn minimum(
    a: MetricBall,
    b: MetricBall,
    left: &[MetricBall],
    right: &[MetricBall],
    separation_only: bool,
    work: &mut Work<'_>,
) -> Result<f64, GeodesicDistanceError> {
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
        if queue.best
            <= if separation_only {
                32.0 * work.position_margin
            } else {
                GAP_M
            }
        {
            break;
        }
        if !separation_only && candidate.lower >= queue.best - GAP_M {
            continue;
        }
        if separation_only && candidate.lower > 16.0 * work.position_margin {
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
        if best
            <= if separation_only {
                32.0 * work.position_margin
            } else {
                GAP_M
            }
        {
            0.0
        } else {
            best
        },
    )
}
