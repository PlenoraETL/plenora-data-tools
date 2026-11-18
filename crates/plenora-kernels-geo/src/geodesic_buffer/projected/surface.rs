//! Validazione geodetica locale mediante separazione continua dei lati.

use super::{
    parts, polygons, BufferFailure, CountedVec, EllissoideGeodetico, UnionBufferError, Work,
};
use geo::{Coord, Geometry};
use geographiclib_rs::{DirectGeodesic, InverseGeodesic};

struct SurfaceEdge {
    a: Coord,
    b: Coord,
    ring: usize,
    index: usize,
    count: usize,
    error: f64,
}

pub(super) fn validate(
    input: &Geometry,
    ellipsoid: &EllissoideGeodetico,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    validate_impl(input, ellipsoid, false, work)
}

pub(super) fn validate_contacts(
    input: &Geometry,
    ellipsoid: &EllissoideGeodetico,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    validate_impl(input, ellipsoid, true, work)
}

#[allow(clippy::too_many_lines)]
fn validate_impl(
    input: &Geometry,
    ellipsoid: &EllissoideGeodetico,
    contacts: bool,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    for polygon in polygons(input) {
        work.step()?;
        if polygon.exterior().0.is_empty() && !polygon.interiors().is_empty() {
            return Err(BufferFailure::InvalidGeometry.into());
        }
        for hole in polygon.interiors() {
            work.step()?;
            if hole.0.is_empty() {
                return Err(BufferFailure::InvalidGeometry.into());
            }
        }
    }
    let mut edges = CountedVec::new();
    let mut south = f64::INFINITY;
    let mut north = f64::NEG_INFINITY;
    for (ring_id, ring) in parts(input)?.enumerate() {
        work.step()?;
        if ring.0.is_empty() {
            continue;
        }
        if ring.0.len() < 4 || ring.0.first() != ring.0.last() {
            return Err(BufferFailure::InvalidGeometry.into());
        }
        let count = ring.0.len() - 1;
        for (index, pair) in ring.0.windows(2).enumerate() {
            work.step()?;
            let a = pair[0];
            let b = pair[1];
            south = south.min(a.y);
            north = north.max(a.y);
            let (length, _, _, _): (f64, f64, f64, f64) =
                ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
            if !length.is_finite() || length <= 0.0 || length > 60_000.0 {
                return Err(BufferFailure::UnsupportedSurface.into());
            }
            let h = (length + 2e-6).next_up();
            if h > 60_000.0 {
                return Err(BufferFailure::UnsupportedSurface.into());
            }
            let error = (((8e-6 * h).next_up() * h).next_up() + 0.00002).next_up();
            work.push(
                &mut edges,
                SurfaceEdge {
                    a,
                    b,
                    ring: ring_id,
                    index,
                    count,
                    error,
                },
            )?;
            let before = ring.0[if index == 0 { count - 1 } else { index - 1 }];
            work.step()?;
            let (back, backwards, _, _): (f64, f64, f64, f64) =
                ellipsoid.geodetica().inverse(a.y, a.x, before.y, before.x);
            work.step()?;
            let (_, forwards, _, _): (f64, f64, f64, f64) =
                ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
            for (end, azimuth, distance) in [(before, backwards, back), (b, forwards, length)] {
                work.step()?;
                let (lat, lon): (f64, f64) =
                    ellipsoid.geodetica().direct(a.y, a.x, azimuth, distance);
                work.step()?;
                let residual: f64 = ellipsoid.geodetica().inverse(lat, lon, end.y, end.x);
                if !residual.is_finite() || residual > 1e-6 {
                    return Err(BufferFailure::AccuracyUnproven.into());
                }
            }
            let gap = (backwards - forwards).abs();
            let angle_error = (8e-6 / back.min(length)).to_degrees() + 1e-8;
            if !angle_error.is_finite() || gap.min(360.0 - gap) <= angle_error {
                return Err(BufferFailure::UnsupportedSurface.into());
            }
        }
    }
    // Insieme a span longitudinale <1 grado, contiene gli archi in una
    // palla fortemente convessa: due lati adiacenti non si reincontrano.
    if (north - south).next_up() >= 1.0 {
        return Err(BufferFailure::UnsupportedSurface.into());
    }
    for (i, a) in edges.iter().enumerate() {
        for b in edges.iter().skip(i + 1) {
            work.step()?;
            if a.ring == b.ring
                && (b.index == a.index + 1 || (a.index == 0 && b.index + 1 == a.count))
            {
                continue;
            }
            // Archi minimi locali con un estremo comune non si incontrano
            // nuovamente nella palla fortemente convessa; duplicati coincidono.
            if contacts && a.ring != b.ring {
                let shared = [a.a, a.b]
                    .into_iter()
                    .find(|p| *p == b.a || *p == b.b || axis_contains(b.a, b.b, *p))
                    .or_else(|| [b.a, b.b].into_iter().find(|p| axis_contains(a.a, a.b, *p)));
                if let Some(shared) = shared {
                    junction(shared, &edges, ellipsoid, work)?;
                    continue;
                }
            }
            let gap =
                axis_gap(a.a.x, a.b.x, b.a.x, b.b.x).max(axis_gap(a.a.y, a.b.y, b.a.y, b.b.y));
            // M e N*cos(phi) >50000 m/grado per |phi|<53 nel dominio.
            // Il lemma dei lati copre tutta l'omotopia, non solo i vertici.
            let separation = (gap * 50_000.0).next_down();
            if separation <= (a.error + b.error).next_up() {
                return Err(BufferFailure::AccuracyUnproven.into());
            }
        }
    }
    Ok(())
}

fn axis_gap(a: f64, b: f64, c: f64, d: f64) -> f64 {
    (a.min(b) - c.max(d))
        .next_down()
        .max((c.min(d) - a.max(b)).next_down())
        .max(0.0)
}

struct JunctionRay {
    end: Coord,
    planar: f64,
    geodesic: f64,
    error: f64,
}

fn junction(
    origin: Coord,
    edges: &[SurfaceEdge],
    ellipsoid: &EllissoideGeodetico,
    work: &mut Work<'_>,
) -> Result<(), UnionBufferError> {
    let mut rays: CountedVec<JunctionRay> = CountedVec::new();
    for edge in edges {
        work.step()?;
        let endpoints = if edge.a == origin {
            [Some(edge.b), None]
        } else if edge.b == origin {
            [Some(edge.a), None]
        } else if axis_contains(edge.a, edge.b, origin) {
            [Some(edge.a), Some(edge.b)]
        } else {
            [None, None]
        };
        for end in endpoints.into_iter().flatten() {
            let mut duplicate = false;
            for ray in &rays[..] {
                work.step()?;
                duplicate |= ray.end == end
                    || axis_contains(origin, ray.end, end)
                    || axis_contains(origin, end, ray.end);
            }
            if duplicate {
                continue;
            }
            let (length, azimuth, _, _): (f64, f64, f64, f64) = ellipsoid
                .geodetica()
                .inverse(origin.y, origin.x, end.y, end.x);
            work.step()?;
            let (y, x): (f64, f64) = ellipsoid
                .geodetica()
                .direct(origin.y, origin.x, azimuth, length);
            work.step()?;
            let residual: f64 = ellipsoid.geodetica().inverse(y, x, end.y, end.x);
            if !residual.is_finite() || residual > 1e-6 {
                return Err(BufferFailure::AccuracyUnproven.into());
            }
            let planar = (end.x - origin.x).atan2(end.y - origin.y).to_degrees();
            work.push(
                &mut rays,
                JunctionRay {
                    end,
                    planar,
                    geodesic: azimuth,
                    error: (8e-6 / length).to_degrees() + 1e-8,
                },
            )?;
        }
    }
    let first = rays.first().ok_or(BufferFailure::AccuracyUnproven)?;
    // L'ordine circolare dei raggi qualifica i settori dell'omotopia al
    // contatto. Un duplicato intero e' lo stesso arco geodetico minimo.
    for (index, a) in rays.iter().enumerate().skip(1) {
        let plane_a = (a.planar - first.planar).rem_euclid(360.0);
        let geo_a = (a.geodesic - first.geodesic).rem_euclid(360.0);
        if plane_a.min(360.0 - plane_a) <= 1e-8 || geo_a.min(360.0 - geo_a) <= a.error + first.error
        {
            return Err(BufferFailure::AccuracyUnproven.into());
        }
        for b in rays.iter().skip(index + 1) {
            work.step()?;
            let plane_b = (b.planar - first.planar).rem_euclid(360.0);
            let geo_b = (b.geodesic - first.geodesic).rem_euclid(360.0);
            if (plane_a - plane_b).abs() <= 1e-8
                || (geo_a - geo_b).abs() <= a.error + b.error
                || (plane_a < plane_b) != (geo_a < geo_b)
            {
                return Err(BufferFailure::AccuracyUnproven.into());
            }
        }
    }
    Ok(())
}

/// Sottoarchi esatti in coordinate: meridiani ed equatore nel dominio locale.
pub(super) fn axis_contains(a: Coord, b: Coord, p: Coord) -> bool {
    (a.x == b.x && p.x == a.x && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y))
        || (a.y == 0.0 && b.y == 0.0 && p.y == 0.0 && p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x))
}
