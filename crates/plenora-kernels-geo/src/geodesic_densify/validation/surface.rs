//! Topologia degli anelli geodetici, indipendente dal piano lon/lat.

#![allow(
    clippy::suboptimal_flops,
    reason = "Ordine esplicito delle operazioni nelle maggiorazioni numeriche"
)]
use super::{minimum, GeodesicDensifyError, GeometryIndex, MetricBall, Work, GAP_M, ROUND_M};
use geo::{Coord, LineString};
use geographiclib_rs::{PolygonArea, Winding};

pub(super) struct SurfaceRing {
    pub begin: usize,
    pub end: usize,
    pub area: f64,
    pub shell: usize,
}

// Pezzi in un intorno strettamente convesso: raggio molto inferiore al
// raggio di convessita' degli ellissoidi terrestri qualificati. Due pezzi
// adiacenti distinti non si incontrano nuovamente in questo intorno.
pub(super) fn append_short_pieces(
    ball: MetricBall,
    nodes: &mut Vec<MetricBall>,
    work: &mut Work<'_>,
    depth: u32,
) -> Result<(), GeodesicDensifyError> {
    work.step()?;
    if ball.arc.hi == ball.arc.lo {
        return Err(GeodesicDensifyError::UnsupportedSurface);
    }
    if depth > 32 {
        return Err(GeodesicDensifyError::AccuracyUnproven);
    }
    if ball.radius <= 250_000.0 {
        return work.push(nodes, ball);
    }
    let (left, right) = ball.split(&[], work)?;
    append_short_pieces(left, nodes, work, depth + 1)?;
    append_short_pieces(right, nodes, work, depth + 1)
}

pub(super) fn ring_area(
    ring: &LineString,
    work: &mut Work<'_>,
) -> Result<f64, GeodesicDensifyError> {
    if ring.0.len() < 4 {
        return Err(GeodesicDensifyError::InvalidGeometry);
    }
    let (Some(&first), Some(&last)) = (ring.0.first(), ring.0.last()) else {
        return Err(GeodesicDensifyError::InvalidGeometry);
    };
    if work.inverse(first, last)?.0 != 0.0 {
        return Err(GeodesicDensifyError::InvalidGeometry);
    }
    let ellipsoid = work.ellipsoid;
    let mut polygon = PolygonArea::new(ellipsoid.geodetica(), Winding::CounterClockwise);
    // n e' esatto in binary64; limita anche il contatore i64 di PolygonArea.
    let n = f64::from(u32::try_from(ring.0.len()).map_err(|_| GeodesicDensifyError::WorkLimit)?);
    for point in &ring.0 {
        work.step()?;
        polygon.add_point(point.y, point.x);
    }
    work.step()?;
    let (perimeter, area, _) = polygon.compute(true);
    // Somma non compensata: |S12| <= A, errore <= gamma_n*n*A.
    // 2*epsilon maggiora arrotondamenti del bound; 8 copre la riduzione
    // modulo A e l'area totale. 1 m2/lato copre le primitive G1/G2.
    let total_area = ellipsoid.geodetica().area();
    let round = (n + 8.0) * (2.0 * f64::EPSILON);
    let margin = n + round / (1.0 - round) * n * total_area + perimeter * ROUND_M;
    if !area.is_finite() || area.abs() <= margin || total_area * 0.5 - area.abs() <= margin {
        return Err(GeodesicDensifyError::UnsupportedSurface);
    }
    Ok(area)
}

fn adjacent(a: MetricBall, b: MetricBall, work: &mut Work<'_>) -> Result<(), GeodesicDensifyError> {
    let join = work.at(a.arc, a.arc.hi)?;
    let before = work.at(a.arc, a.arc.lo)?;
    let after = work.at(b.arc, b.arc.hi)?;
    let (back_length, backwards, _, _) = work.inverse(join, before)?;
    let (forward_length, forwards, _, _) = work.inverse(join, after)?;
    let gap = (backwards - forwards).abs();
    let angle_error = (8.0 * ROUND_M / back_length.min(forward_length)).to_degrees() + 1e-8;
    if gap.min(360.0 - gap) <= angle_error {
        return Err(GeodesicDensifyError::UnsupportedSurface);
    }
    Ok(())
}

pub(super) fn validate(
    index: &GeometryIndex,
    work: &mut Work<'_>,
) -> Result<(), GeodesicDensifyError> {
    for (ri, ring) in index.rings.iter().enumerate() {
        for i in ring.begin..ring.end {
            let next = if i + 1 == ring.end { ring.begin } else { i + 1 };
            adjacent(index.nodes[i], index.nodes[next], work)?;
            for (rj, other) in index.rings.iter().enumerate().skip(ri) {
                for j in other.begin..other.end {
                    work.step()?;
                    if ri == rj && (j <= i || j == next || (i == ring.begin && j + 1 == ring.end)) {
                        continue;
                    }
                    let a = index.nodes[i];
                    let b = index.nodes[j];
                    let lower =
                        work.inverse(a.centre, b.centre)?.0 - a.radius - b.radius - 4.0 * ROUND_M;
                    if lower > GAP_M {
                        continue;
                    }
                    if minimum(a, b, &[], &[], true, work)? <= GAP_M {
                        return Err(GeodesicDensifyError::UnsupportedSurface);
                    }
                }
            }
        }
        if ring.shell != ri {
            let point = index.nodes[ring.begin].centre;
            if !in_ring(point, &index.rings[ring.shell], &index.nodes, work)? {
                return Err(GeodesicDensifyError::InvalidGeometry);
            }
            for other in &index.rings[ring.shell + 1..ri] {
                if in_ring(point, other, &index.nodes, work)?
                    || in_ring(index.nodes[other.begin].centre, ring, &index.nodes, work)?
                {
                    return Err(GeodesicDensifyError::UnsupportedSurface);
                }
            }
        }
    }
    // Le componenti di MultiPolygon non possono avere interni sovrapposti.
    for (i, ring) in index.rings.iter().enumerate() {
        if ring.shell != i {
            continue;
        }
        for (j, other) in index.rings.iter().enumerate().skip(i + 1) {
            if other.shell != j {
                continue;
            }
            if in_polygon(index.nodes[ring.begin].centre, j, index, work)?
                || in_polygon(index.nodes[other.begin].centre, i, index, work)?
            {
                return Err(GeodesicDensifyError::UnsupportedSurface);
            }
        }
    }
    Ok(())
}

fn normal(point: Coord) -> [f64; 3] {
    let (sin_lat, cos_lat) = point.y.to_radians().sin_cos();
    let (sin_lon, cos_lon) = point.x.to_radians().sin_cos();
    [cos_lat * cos_lon, cos_lat * sin_lon, sin_lat]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0].mul_add(b[0], a[1].mul_add(b[1], a[2] * b[2]))
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1].mul_add(b[2], -a[2] * b[1]),
        a[2].mul_add(b[0], -a[0] * b[2]),
        a[0].mul_add(b[1], -a[1] * b[0]),
    ]
}

struct FanSum {
    angle: f64,
    error: f64,
}

// La mappa normale dell'ellissoide e' un diffeomorfismo della sfera.
// Una triangolazione a ventaglio dall'antipode di q ha area orientata A
// se q e' fuori, A-4pi se q e' dentro (verso positivo). Non usa la
// differenza dei winding in q e -q, che fallirebbe con entrambi interni.
fn fan_piece(
    point: Coord,
    ball: MetricBall,
    maximum: f64,
    sum: &mut FanSum,
    work: &mut Work<'_>,
    depth: u32,
) -> Result<(), GeodesicDensifyError> {
    work.step()?;
    if depth > 60 {
        return Err(GeodesicDensifyError::AccuracyUnproven);
    }
    let distance = work.inverse(point, ball.centre)?.0;
    // Raggi principali compresi fra 5.9e6 e 7.1e6 m nel dominio pubblico.
    // La palla immagine non contiene q; arco e corda sono omotopi senza q.
    if ball.radius > maximum || distance <= 8.0 * ball.radius + 16.0 * ROUND_M {
        let (a, b) = ball.split(&[], work)?;
        fan_piece(point, a, maximum, sum, work, depth + 1)?;
        return fan_piece(point, b, maximum, sum, work, depth + 1);
    }
    let q = normal(point).map(|v| -v);
    let a = normal(work.at(ball.arc, ball.arc.lo)?);
    let b = normal(work.at(ball.arc, ball.arc.hi)?);
    let numerator = dot(q, cross(a, b));
    let denominator = 1.0 + dot(q, a) + dot(a, b) + dot(b, q);
    let condition = numerator.hypot(denominator);
    if condition <= 1e-12 {
        return Err(GeodesicDensifyError::AccuracyUnproven);
    }
    let angle = 2.0 * numerator.atan2(denominator);
    sum.angle += angle;
    // Limite conservativo della striscia tra arco normale e corda:
    // |n''(s)|<4/Rmin^2, lunghezza<2L/Rmin, area<16(L/Rmin)^3.
    // Il termine condizionato copre coordinate direct, trigonometria,
    // prodotti scalari, atan2 e accumulazione in binary64.
    sum.error += 16.0 * ((ball.arc.hi - ball.arc.lo) / 5.9e6).powi(3)
        + 2e-12 / condition
        + sum.angle.abs() * 1e-15;
    Ok(())
}

fn in_ring(
    point: Coord,
    ring: &SurfaceRing,
    nodes: &[MetricBall],
    work: &mut Work<'_>,
) -> Result<bool, GeodesicDensifyError> {
    let mut maximum = 250_000.0;
    for _ in 0..32 {
        let mut sum = FanSum {
            angle: 0.0,
            error: 0.0,
        };
        for &ball in &nodes[ring.begin..ring.end] {
            fan_piece(point, ball, maximum, &mut sum, work, 0)?;
        }
        if sum.angle.abs() > sum.error {
            return Ok(sum.angle.is_sign_positive() != ring.area.is_sign_positive());
        }
        maximum *= 0.5;
    }
    Err(GeodesicDensifyError::AccuracyUnproven)
}

fn in_polygon(
    point: Coord,
    shell: usize,
    index: &GeometryIndex,
    work: &mut Work<'_>,
) -> Result<bool, GeodesicDensifyError> {
    if !in_ring(point, &index.rings[shell], &index.nodes, work)? {
        return Ok(false);
    }
    for hole in index
        .rings
        .iter()
        .skip(shell + 1)
        .take_while(|ring| ring.shell == shell)
    {
        if in_ring(point, hole, &index.nodes, work)? {
            return Ok(false);
        }
    }
    Ok(true)
}
