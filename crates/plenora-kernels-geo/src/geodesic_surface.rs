//! Topologia degli anelli geodetici, indipendente dal piano lon/lat.
#![allow(
    clippy::redundant_pub_crate,
    reason = "Visibilita esplicita del supporto condiviso interno al crate"
)]
//!
//! Il certificato d'area scala con perimetro, condizionamento polare e
//! accumulazione; quello di separazione con l'errore posizionale effettivo.

#![allow(clippy::float_cmp, reason = "Identita geodetiche esatte")]
#![allow(
    clippy::suboptimal_flops,
    reason = "Ordine esplicito delle operazioni nelle maggiorazioni numeriche"
)]
pub(crate) mod metric;
use crate::geodesic_distance::GeodesicDistanceError;
use geo::{Coord, LineString};
use geographiclib_rs::{InverseGeodesic, PolygonArea, Winding};
use metric::{minimum, GeometryIndex, MetricBall, Work};

pub(crate) struct SurfaceRing {
    pub(crate) begin: usize,
    pub(crate) end: usize,
    pub(crate) orientation: f64,
    pub(crate) shell: usize,
}

// Pezzi in un intorno strettamente convesso: raggio molto inferiore al
// raggio di convessita' degli ellissoidi terrestri qualificati. Due pezzi
// adiacenti distinti non si incontrano nuovamente in questo intorno.
pub(crate) fn append_short_pieces(
    ball: MetricBall,
    nodes: &mut Vec<MetricBall>,
    work: &mut Work<'_>,
    depth: u32,
) -> Result<(), GeodesicDistanceError> {
    work.step()?;
    if ball.arc.hi == ball.arc.lo {
        return Ok(());
    }
    if depth > 32 {
        return Err(GeodesicDistanceError::AccuracyUnproven);
    }
    if ball.radius <= 250_000.0 {
        return work.push(nodes, ball);
    }
    let (left, right) = ball.split(&[], work)?;
    append_short_pieces(left, nodes, work, depth + 1)?;
    append_short_pieces(right, nodes, work, depth + 1)
}

pub(crate) fn ring_area(
    ring: &LineString,
    work: &mut Work<'_>,
) -> Result<f64, GeodesicDistanceError> {
    if ring.0.len() < 4 {
        return Err(GeodesicDistanceError::InvalidGeometry);
    }
    let (Some(&first), Some(&last)) = (ring.0.first(), ring.0.last()) else {
        return Err(GeodesicDistanceError::InvalidGeometry);
    };
    if work.inverse(first, last)?.0 != 0.0 {
        return Err(GeodesicDistanceError::InvalidGeometry);
    }
    let ellipsoid = work.ellipsoid;
    let mut polygon = PolygonArea::new(ellipsoid.geodetica(), Winding::CounterClockwise);
    match local_orientation(ring, work) {
        Ok(Some(orientation)) => return Ok(orientation),
        Err(GeodesicDistanceError::AccuracyUnproven) | Ok(None) => {}
        Err(error) => return Err(error),
    }
    let mut margin = 0.0;
    let mut magnitude = 0.0;
    for pair in ring.0.windows(2) {
        work.step()?;
        let (_, _, _, _, _, _, area, _): (f64, f64, f64, f64, f64, f64, f64, f64) = ellipsoid
            .geodetica()
            .inverse(pair[0].y, pair[0].x, pair[1].y, pair[1].x);
        if !area.is_finite() {
            return Err(GeodesicDistanceError::AccuracyUnproven);
        }
        magnitude = (magnitude + area.abs()).next_up();
    }
    for point in &ring.0 {
        work.step()?;
        polygon.add_point(point.y, point.x);
        margin += 1.0;
    }
    work.step()?;
    let (perimeter, area, _) = polygon.compute(true);
    let epsilon = margin * f64::EPSILON;
    if epsilon >= 0.5 {
        return Err(GeodesicDistanceError::AccuracyUnproven);
    }
    margin += perimeter * work.position_margin + epsilon / (1.0 - epsilon) * magnitude;
    if !margin.is_finite() {
        return Err(GeodesicDistanceError::AccuracyUnproven);
    }
    if !area.is_finite() || area.abs() <= margin {
        return Ok(0.0);
    }
    if ellipsoid.geodetica().area() * 0.5 - area.abs() <= margin {
        return Err(GeodesicDistanceError::UnsupportedSurface);
    }
    Ok(area)
}

// La mappa normale preserva orientamento. Kmin>=(1-f)^2/a^2 e
// area_totale>=4pi*a^2*(1-f)^2: area normale <6 sr certifica la regione
// minore anche per f=1/250. Il ventaglio deve restare ben condizionato.
// Certifichiamo il segno, non un'area ellissoidica da pubblicare.
fn local_orientation(
    ring: &LineString,
    work: &mut Work<'_>,
) -> Result<Option<f64>, GeodesicDistanceError> {
    let mut perimeter = 0.0;
    let mut strip = 0.0;
    for pair in ring.0.windows(2) {
        let length = work.inverse(pair[0], pair[1])?.0 + metric::ROUND_M;
        perimeter = (perimeter + length).next_up();
        strip = (strip + 16.0 * (length / 5.9e6).powi(3)).next_up();
    }
    let origin = ring.0[0];
    let normal = |point: Coord| {
        let latitude = point.y.to_radians();
        let base = origin.y.to_radians();
        let delta = (point.y - origin.y).to_radians();
        let longitude = (point.x - origin.x).to_radians();
        let half_lon = (longitude * 0.5).sin().powi(2);
        [
            latitude.cos() * longitude.sin(),
            delta.sin() + 2.0 * latitude.cos() * base.sin() * half_lon,
            1.0 - 2.0 * ((delta * 0.5).sin().powi(2) + latitude.cos() * base.cos() * half_lon),
        ]
    };
    let mut sum = 0.0;
    let mut magnitude = 0.0;
    let mut rounding = 0.0;
    let mut count = 0.0;
    for pair in ring.0[1..].windows(2) {
        work.step()?;
        let a = normal(pair[0]);
        let b = normal(pair[1]);
        let denominator = 1.0 + a[2] + b[2] + a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let first = a[0] * b[1];
        let second = a[1] * b[0];
        if (first - second).hypot(denominator) <= 0.5 {
            return Ok(None);
        }
        let angle = 2.0 * (first - second).atan2(denominator);
        sum += angle;
        magnitude += angle.abs();
        rounding += 128.0 * f64::EPSILON * (first.abs() + second.abs());
        count += 1.0;
    }
    let epsilon = count * f64::EPSILON;
    if epsilon >= 0.5 {
        return Err(GeodesicDistanceError::AccuracyUnproven);
    }
    let error = strip
        + 512.0 * perimeter * work.position_margin / 5.9e6_f64.powi(2)
        + rounding
        + epsilon / (1.0 - epsilon) * magnitude;
    let error = error + 32.0 * f64::EPSILON * sum.abs();
    let full = 4.0 * std::f64::consts::PI;
    let sum = if sum.abs() <= 2.0 * std::f64::consts::PI {
        sum
    } else {
        (sum + 2.0 * std::f64::consts::PI).rem_euclid(full) - 2.0 * std::f64::consts::PI
    };
    if sum.abs() + error >= 6.0 {
        return Ok(None);
    }
    if !sum.is_finite() || !error.is_finite() || sum.abs() <= error {
        return Err(GeodesicDistanceError::AccuracyUnproven);
    }
    Ok(Some(sum.signum()))
}

fn adjacent(
    a: MetricBall,
    b: MetricBall,
    work: &mut Work<'_>,
) -> Result<(), GeodesicDistanceError> {
    let join = work.at(a.arc, a.arc.hi)?;
    let before = work.at(a.arc, a.arc.lo)?;
    let after = work.at(b.arc, b.arc.hi)?;
    let (back_length, backwards, _, _) = work.inverse(join, before)?;
    let (forward_length, forwards, _, _) = work.inverse(join, after)?;
    let gap = (backwards - forwards).abs();
    let angle_error =
        (8.0 * work.position_margin / back_length.min(forward_length)).to_degrees() + 1e-8;
    if gap.min(360.0 - gap) <= angle_error {
        let meridian_overlap = join.x == before.x
            && join.x == after.x
            && (before.y - join.y).signum() == (after.y - join.y).signum();
        let equator_overlap = join.y == 0.0
            && before.y == 0.0
            && after.y == 0.0
            && (before.x - join.x).abs() < 180.0
            && (after.x - join.x).abs() < 180.0
            && (before.x - join.x).signum() == (after.x - join.x).signum();
        return Err(if before == after || meridian_overlap || equator_overlap {
            GeodesicDistanceError::UnsupportedSurface
        } else {
            GeodesicDistanceError::AccuracyUnproven
        });
    }
    Ok(())
}

pub(crate) fn validate(
    index: &GeometryIndex,
    work: &mut Work<'_>,
) -> Result<(), GeodesicDistanceError> {
    if !index.rings.is_empty() {
        separate(index.root, index.root, index, work)?;
    }
    for (ri, ring) in index.rings.iter().enumerate() {
        for i in ring.begin..ring.end {
            let next = if i + 1 == ring.end { ring.begin } else { i + 1 };
            adjacent(index.nodes[i], index.nodes[next], work)?;
        }
        if ring.orientation == 0.0 {
            return Err(GeodesicDistanceError::AccuracyUnproven);
        }
        if ring.shell != ri {
            let point = index.nodes[ring.begin].centre;
            if !in_ring(point, &index.rings[ring.shell], &index.nodes, work)? {
                return Err(GeodesicDistanceError::InvalidGeometry);
            }
            for other in &index.rings[ring.shell + 1..ri] {
                if in_ring(point, other, &index.nodes, work)?
                    || in_ring(index.nodes[other.begin].centre, ring, &index.nodes, work)?
                {
                    return Err(GeodesicDistanceError::InvalidGeometry);
                }
            }
        }
    }
    // Le componenti di MultiPolygon non possono avere interni sovrapposti.
    for (i, ring) in index.rings.iter().enumerate() {
        work.step()?;
        if ring.shell != i {
            continue;
        }
        for (j, other) in index.rings.iter().enumerate().skip(i + 1) {
            work.step()?;
            if other.shell != j {
                continue;
            }
            if in_polygon(index.nodes[ring.begin].centre, j, index, work)?
                || in_polygon(index.nodes[other.begin].centre, i, index, work)?
            {
                return Err(GeodesicDistanceError::InvalidGeometry);
            }
        }
    }
    Ok(())
}

// Visita duale della gerarchia: le coppie di sottoalberi separati si
// scartano in blocco. Solo le foglie adiacenti condividono un estremo.
fn separate(
    left: usize,
    right: usize,
    index: &GeometryIndex,
    work: &mut Work<'_>,
) -> Result<(), GeodesicDistanceError> {
    work.step()?;
    let a = index.nodes[left];
    let b = index.nodes[right];
    if left == right {
        if let Some((one, two)) = a.children {
            separate(one, one, index, work)?;
            separate(one, two, index, work)?;
            separate(two, two, index, work)?;
        }
        return Ok(());
    }
    if work.inverse(a.centre, b.centre)?.0 - a.radius - b.radius - 4.0 * work.position_margin
        > 16.0 * work.position_margin
    {
        return Ok(());
    }
    if let Some((one, two)) = a
        .children
        .filter(|_| b.children.is_none() || a.radius >= b.radius)
    {
        separate(one, right, index, work)?;
        return separate(two, right, index, work);
    }
    if let Some((one, two)) = b.children {
        separate(left, one, index, work)?;
        return separate(left, two, index, work);
    }
    let lo = left.min(right);
    let hi = left.max(right);
    let ri = index.rings.partition_point(|ring| ring.end <= lo);
    let ring = &index.rings[ri];
    if hi < ring.end && (hi == lo + 1 || (lo == ring.begin && hi + 1 == ring.end)) {
        return Ok(());
    }
    if minimum(a, b, &[], &[], true, work)? == 0.0 {
        return Err(if crossing(a, b, work)? {
            GeodesicDistanceError::UnsupportedSurface
        } else {
            GeodesicDistanceError::AccuracyUnproven
        });
    }
    Ok(())
}

// Entro un intorno convesso, due archi con estremi su lati opposti
// delle rispettive geodetiche hanno un incrocio trasverso.
fn crossing(
    a: MetricBall,
    b: MetricBall,
    work: &mut Work<'_>,
) -> Result<bool, GeodesicDistanceError> {
    let endpoints = [
        work.at(a.arc, a.arc.lo)?,
        work.at(a.arc, a.arc.hi)?,
        work.at(b.arc, b.arc.lo)?,
        work.at(b.arc, b.arc.hi)?,
    ];
    for x in &endpoints[..2] {
        for y in &endpoints[2..] {
            if x == y {
                return Ok(true);
            }
        }
    }
    for (p, q, r) in [
        (endpoints[0], endpoints[1], endpoints[2]),
        (endpoints[0], endpoints[1], endpoints[3]),
        (endpoints[2], endpoints[3], endpoints[0]),
        (endpoints[2], endpoints[3], endpoints[1]),
    ] {
        if (p.x == q.x && p.x == r.x && (p.y.min(q.y)..=p.y.max(q.y)).contains(&r.y))
            || (p.y == 0.0
                && q.y == 0.0
                && r.y == 0.0
                && (p.x - q.x).abs() < 180.0
                && (p.x.min(q.x)..=p.x.max(q.x)).contains(&r.x))
        {
            return Ok(true);
        }
    }
    let mut opposite = true;
    for (p, q, r, t) in [
        endpoints.into(),
        (endpoints[2], endpoints[3], endpoints[0], endpoints[1]),
    ] {
        let (base_length, axis, _, _) = work.inverse(p, q)?;
        let (dr, ar, _, _) = work.inverse(p, r)?;
        let (dt, at, _, _) = work.inverse(p, t)?;
        if dr.max(dt) > 1_000_000.0 {
            return Ok(false);
        }
        let sr = (ar - axis).to_radians().sin();
        let st = (at - axis).to_radians().sin();
        let uncertainty = 32.0 * work.position_margin / base_length.min(dr).min(dt) + 1e-10;
        opposite &= sr.abs() > uncertainty && st.abs() > uncertainty && sr.signum() != st.signum();
    }
    Ok(opposite)
}

// (n(vertex)-n(point)) nel riferimento est/nord/normale di point.
// Le identita' a mezzo angolo evitano la sottrazione di vettori unitari
// quasi uguali. La normalizzazione elimina il fattore distanza al quadrato
// dall'atan2 del triangolo con apice -n(point).
fn local_chord(point: Coord, vertex: Coord) -> [f64; 3] {
    let latitude = point.y.to_radians();
    let other = vertex.y.to_radians();
    let delta = (vertex.y - point.y).to_radians();
    let longitude = (vertex.x - point.x).to_radians();
    let half_lon = (longitude * 0.5).sin().powi(2);
    let east = other.cos() * longitude.sin();
    let north = delta.sin() + 2.0 * other.cos() * latitude.sin() * half_lon;
    let radial = -2.0 * ((delta * 0.5).sin().powi(2) + other.cos() * latitude.cos() * half_lon);
    let length = east.hypot(north).hypot(radial);
    [east / length, north / length, radial / length]
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
    sum: &mut FanSum,
    work: &mut Work<'_>,
    depth: u32,
) -> Result<(), GeodesicDistanceError> {
    work.step()?;
    if depth > 60 {
        return Err(GeodesicDistanceError::AccuracyUnproven);
    }
    let distance = work.inverse(point, ball.centre)?.0;
    // Raggi principali compresi fra 5.9e6 e 7.1e6 m nel dominio pubblico.
    // La palla immagine non contiene q; arco e corda sono omotopi senza q.
    if distance <= 4.0 * (ball.arc.hi - ball.arc.lo) + 16.0 * work.position_margin {
        let (a, b) = ball.split(&[], work)?;
        fan_piece(point, a, sum, work, depth + 1)?;
        return fan_piece(point, b, sum, work, depth + 1);
    }
    let a = local_chord(point, work.at(ball.arc, ball.arc.lo)?);
    let b = local_chord(point, work.at(ball.arc, ball.arc.hi)?);
    let numerator = a[1].mul_add(b[0], -a[0] * b[1]);
    let denominator = a[0].mul_add(b[0], a[1].mul_add(b[1], a[2] * b[2]));
    let condition = numerator.hypot(denominator);
    if !condition.is_finite() || condition < 0.5 {
        return Err(GeodesicDistanceError::AccuracyUnproven);
    }
    sum.angle += 2.0 * numerator.atan2(denominator);
    // Striscia arco normale/corda: |n''|<4/Rmin^2 e area<16(L/Rmin)^3.
    // Il condizionamento locale e' limitato; l'errore angolare include
    // la perturbazione delle posizioni, senza dividere per l'area dell'anello.
    sum.error += 16.0 * ((ball.arc.hi - ball.arc.lo) / 5.9e6).powi(3)
        + 64.0 * work.position_margin / (distance - ball.radius)
        + 2e-12
        + sum.angle.abs() * 1e-15;
    Ok(())
}

fn in_ring(
    point: Coord,
    ring: &SurfaceRing,
    nodes: &[MetricBall],
    work: &mut Work<'_>,
) -> Result<bool, GeodesicDistanceError> {
    let mut sum = FanSum {
        angle: 0.0,
        error: 0.0,
    };
    for &ball in &nodes[ring.begin..ring.end] {
        fan_piece(point, ball, &mut sum, work, 0)?;
    }
    // K<=1/(a^2*(1-f)^2), area totale<=4pi*a^2: la regione minore
    // ha area normale A<2pi/(1-f)^2<6.34 nel dominio f<=1/250.
    // Con il verso positivo Sigma=A fuori e A-4pi<-6.22 dentro.
    // La soglia -pi separa i due intervalli anche per A arbitrariamente
    // piccola e quando punto e antipode appartengono entrambi alla regione.
    let oriented = sum.angle * ring.orientation.signum();
    let threshold = -std::f64::consts::PI;
    if oriented + sum.error < threshold {
        Ok(true)
    } else if oriented - sum.error > threshold {
        Ok(false)
    } else {
        Err(GeodesicDistanceError::AccuracyUnproven)
    }
}

fn in_polygon(
    point: Coord,
    shell: usize,
    index: &GeometryIndex,
    work: &mut Work<'_>,
) -> Result<bool, GeodesicDistanceError> {
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

pub(crate) fn contains_any(
    surface: &GeometryIndex,
    other: &GeometryIndex,
    work: &mut Work<'_>,
) -> Result<bool, GeodesicDistanceError> {
    for (i, ring) in surface.rings.iter().enumerate() {
        work.step()?;
        if ring.shell != i {
            continue;
        }
        for &point in &other.representatives {
            work.step()?;
            if in_polygon(point, i, surface, work)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
pub(crate) mod fixtures;

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use std::path::Path;

    fn classification_definitions(source: &str, name: &str) -> usize {
        source
            .lines()
            .filter(|line| {
                let Some((_, definition)) = line.split_once("fn ") else {
                    return false;
                };
                definition
                    .split_once('(')
                    .is_some_and(|(identifier, _)| identifier.trim() == name)
            })
            .count()
    }

    fn count_in_sources(path: &Path, name: &str) -> usize {
        if path.is_dir() {
            return std::fs::read_dir(path)
                .unwrap()
                .map(|entry| count_in_sources(&entry.unwrap().path(), name))
                .sum();
        }
        if path.extension().is_some_and(|extension| extension == "rs") {
            classification_definitions(&std::fs::read_to_string(path).unwrap(), name)
        } else {
            0
        }
    }

    #[test]
    fn ring_classification_has_one_implementation() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for name in ["in_ring", "in_polygon", "local_chord", "fan_piece"] {
            assert_eq!(count_in_sources(&source, name), 1);
        }
        let duplicate = ["fn ", "in_ring", "() {}\n"].concat().repeat(2);
        assert_eq!(classification_definitions(&duplicate, "in_ring"), 2);
    }
    #[test]
    #[deny(clippy::disallowed_methods)]
    fn normal_orientation_matches_area_oracle_for_dense_global_and_local_rings() {
        use super::*;
        use crate::geodesic_densify::{geodesic_densify_with_limits, GeodesicDensifyLimits};
        use crate::geodesic_distance::GeodesicDistanceLimits;
        use crate::geodetica::EllissoideGeodetico;
        let crs = plenora_core::crs::resolve_crs("EPSG:4326", "crs").unwrap();
        for (west, east, south, north, step) in [
            (-10.0, 10.0, 45.0, 45.000_001, 100.0),
            (10.0, 10.00001, 45.0, 45.00001, 0.01),
            (179.0, -179.0, -1.0, 1.0, 10_000.0),
        ] {
            let ring = LineString::from(vec![
                (west, south),
                (east, south),
                (east, north),
                (west, north),
                (west, south),
            ]);
            let dense = geodesic_densify_with_limits(
                &[ring],
                &crs,
                step,
                GeodesicDensifyLimits {
                    max_points: 100_000,
                    max_reserved_bytes: 10_000_000,
                    deadline: None,
                },
                None,
            )
            .unwrap();
            for reverse in [false, true] {
                let mut ring = dense[0].clone();
                if reverse {
                    ring.0.reverse();
                }
                let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
                let mut polygon =
                    PolygonArea::new(ellipsoid.geodetica(), Winding::CounterClockwise);
                for point in &ring.0 {
                    polygon.add_point(point.y, point.x);
                }
                let (_, area, _) = polygon.compute(true);
                let mut work = Work::new(
                    GeodesicDistanceLimits {
                        max_work: 1_000_000,
                        max_reserved_bytes: 10_000_000,
                        deadline: None,
                    },
                    None,
                    &ellipsoid,
                );
                assert_eq!(
                    local_orientation(&ring, &mut work).unwrap(),
                    Some(area.signum())
                );
            }
        }
    }
}
