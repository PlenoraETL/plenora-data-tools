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
