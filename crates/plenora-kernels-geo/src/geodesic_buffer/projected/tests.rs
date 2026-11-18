use super::*;
use crate::polygon_boolean::PolygonBooleanError;
use crate::predicates::PredicateError;
use geo::{CoordsIter, MultiLineString};
use plenora_core::crs::resolve_crs;

#[deny(clippy::disallowed_methods)]
fn limits() -> UnionBufferLimits {
    UnionBufferLimits {
        overlay: crate::polygon_boolean::PolygonBooleanLimits {
            work: crate::predicates::RelateLimits {
                max_steps: 2_000_000_000,
                max_nodes: 100_000_000,
                ..Default::default()
            },
            ..Default::default()
        },
        max_results: 20,
        max_vertices: 1_000_000,
    }
}

#[deny(clippy::disallowed_methods)]
fn segment(latitude: f64) -> Geometry {
    Geometry::LineString(LineString::from(vec![(9.0, latitude), (9.00001, latitude)]))
}

#[deny(clippy::disallowed_methods)]
fn distance_to_segment(p: Coord, a: Coord, b: Coord, ellipsoid: &EllissoideGeodetico) -> f64 {
    let geodesic = ellipsoid.geodetica();
    let (length, azimuth, _, _): (f64, f64, f64, f64) = geodesic.inverse(a.y, a.x, b.y, b.x);
    let distance = |fraction: f64| {
        let (lat, lon): (f64, f64) = geodesic.direct(a.y, a.x, azimuth, length * fraction);
        let d: f64 = geodesic.inverse(p.y, p.x, lat, lon);
        d
    };
    let mut lower = 0.0;
    let mut upper = 1.0;
    for _ in 0..64 {
        let left = (upper - lower) / 3.0 + lower;
        let right = upper - (upper - lower) / 3.0;
        if distance(left) < distance(right) {
            upper = right;
        } else {
            lower = left;
        }
    }
    distance(lower.midpoint(upper))
        .min(distance(0.0))
        .min(distance(1.0))
}

#[deny(clippy::disallowed_methods)]
fn boundary_witness(point: Coord, geometry: &Geometry, ellipsoid: &EllissoideGeodetico) -> f64 {
    let polygons = match geometry {
        Geometry::Polygon(p) => std::slice::from_ref(p),
        Geometry::MultiPolygon(p) => p.0.as_slice(),
        _ => panic!("superficie richiesta"),
    };
    let mut best = f64::INFINITY;
    for polygon in polygons {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            for pair in ring.0.windows(2) {
                let a = pair[0];
                let b = pair[1];
                let dx = b.x - a.x;
                let dy = b.y - a.y;
                let fraction = ((point.x - a.x).mul_add(dx, (point.y - a.y) * dy)
                    / dx.mul_add(dx, dy * dy))
                .clamp(0.0, 1.0);
                let lon = dx.mul_add(fraction, a.x);
                let lat = dy.mul_add(fraction, a.y);
                let delta_lon = lon - point.x;
                let delta_lat = lat - point.y;
                // Il filtro scarta solo testimoni lontani; ogni accettazione
                // richiede comunque una distanza Karney dal lato pubblicato.
                if delta_lon.mul_add(delta_lon, delta_lat * delta_lat) < 1e-8 {
                    let distance: f64 = ellipsoid.geodetica().inverse(point.y, point.x, lat, lon);
                    best = best.min(distance);
                }
            }
        }
    }
    best
}

#[deny(clippy::disallowed_methods)]
fn dense_exact_boundary(
    surface: &ProjectedBufferSurface<'_>,
    latitude: f64,
    radius: f64,
    ellipsoid: &EllissoideGeodetico,
) {
    // C1: campo di Fermi e calotte terminali, docs/buffer-punto.md; Karney G1.
    // 2052 campioni del confine esatto confrontati con testimoni dell'output.
    let geodesic = ellipsoid.geodetica();
    let (length, azimuth, end_azimuth, _): (f64, f64, f64, f64) =
        geodesic.inverse(latitude, 9.0, latitude, 9.00001);
    for i in 0..=512 {
        let fraction = f64::from(i) / 512.0;
        for (longitude, start) in [(9.0, azimuth + 90.0), (9.00001, end_azimuth - 90.0)] {
            let (y, x): (f64, f64) = geodesic.direct(
                latitude,
                longitude,
                180.0_f64.mul_add(fraction, start),
                radius,
            );
            assert!(
                boundary_witness(Coord { x, y }, &surface.geometry, ellipsoid)
                    <= surface.certificate.absolute_m
            );
        }
        let (lat, lon, tangent): (f64, f64, f64) =
            geodesic.direct(latitude, 9.0, azimuth, length * fraction);
        for side in [-90.0, 90.0] {
            let (y, x): (f64, f64) = geodesic.direct(lat, lon, tangent + side, radius);
            assert!(
                boundary_witness(Coord { x, y }, &surface.geometry, ellipsoid)
                    <= surface.certificate.absolute_m
            );
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn equatorial_width_and_dense_boundary() {
    // C1: equatore geodetico, normali meridiane; Karney G1 direct/inverse.
    // Il piede sul segmento equatoriale ha la stessa longitudine, se interna.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    let input = segment(0.0);
    let result =
        geodesic_line_buffer_with_limits(&[input], &crs, &[100.0], 0.01, false, limits(), None)
            .unwrap();
    let surface = &result.surfaces[0];
    for p in surface.geometry.coords_iter() {
        let foot = p.x.clamp(9.0, 9.00001);
        let distance: f64 = ellipsoid.geodetica().inverse(p.y, p.x, 0.0, foot);
        assert!((distance - 100.0).abs() <= surface.certificate.absolute_m);
    }
    let polygons = match &surface.geometry {
        Geometry::Polygon(p) => std::slice::from_ref(p),
        Geometry::MultiPolygon(p) => p.0.as_slice(),
        _ => panic!("superficie richiesta"),
    };
    for (longitude, start) in [(9.0, 180.0), (9.00001, 0.0)] {
        for i in 0..=512 {
            let azimuth = start + f64::from(i) * 180.0 / 512.0;
            let (y, x): (f64, f64) = ellipsoid.geodetica().direct(0.0, longitude, azimuth, 100.0);
            let mut nearest = f64::INFINITY;
            for polygon in polygons {
                for pair in polygon.exterior().0.windows(2) {
                    let a = pair[0];
                    let b = pair[1];
                    let dx = b.x - a.x;
                    let dy = b.y - a.y;
                    let t = ((x - a.x).mul_add(dx, (y - a.y) * dy) / dx.mul_add(dx, dy * dy))
                        .clamp(0.0, 1.0);
                    let distance: f64 =
                        ellipsoid
                            .geodetica()
                            .inverse(y, x, dy.mul_add(t, a.y), dx.mul_add(t, a.x));
                    nearest = nearest.min(distance);
                }
            }
            assert!(nearest <= surface.certificate.absolute_m);
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn metric_matrix_and_typed_accuracy_rejections() {
    // C1: Karney G1, minimo della distanza lungo la geodetica sorgente.
    // Le soglie grandi e 70 gradi sono fuori dal certificato TM di questa API.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    for latitude in [0.0, 45.0, 70.0] {
        for radius in [100.0, 1000.0, 10000.0, 60000.0] {
            let result = geodesic_line_buffer_with_limits(
                &[segment(latitude)],
                &crs,
                &[radius],
                0.01,
                false,
                limits(),
                None,
            );
            if latitude == 70.0 {
                assert!(matches!(
                    result,
                    Err(UnionBufferError::Disk(BufferFailure::UnsupportedSurface))
                ));
            } else if radius >= 10000.0 {
                assert!(matches!(
                    result,
                    Err(UnionBufferError::Disk(BufferFailure::AccuracyUnproven))
                ));
            } else {
                let surface = result.unwrap().surfaces.remove(0);
                for point in surface.geometry.coords_iter() {
                    let distance = distance_to_segment(
                        point,
                        Coord {
                            x: 9.0,
                            y: latitude,
                        },
                        Coord {
                            x: 9.00001,
                            y: latitude,
                        },
                        &ellipsoid,
                    );
                    assert!((distance - radius).abs() <= surface.certificate.absolute_m);
                }
                assert!(surface.certificate.absolute_m <= 0.01);
                assert!(surface.certificate.relative >= surface.certificate.absolute_m / radius);
                dense_exact_boundary(&surface, latitude, radius, &ellipsoid);
            }
        }
    }
    assert!(matches!(
        geodesic_line_buffer_with_limits(
            &[segment(0.0)],
            &crs,
            &[100.0],
            1e-5,
            false,
            limits(),
            None
        ),
        Err(UnionBufferError::Disk(BufferFailure::AccuracyUnproven))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn angles_multiline_distance_major_and_zero() {
    // C1: GOP-008 e composizione delle unioni; ordine distanza poi ingresso.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let line = LineString::from(vec![(9.0, 0.0), (9.00001, 0.0), (9.00001, 0.00001)]);
    let inputs = [
        Geometry::LineString(line.clone()),
        Geometry::MultiLineString(MultiLineString(vec![line])),
    ];
    let result =
        geodesic_line_buffer_with_limits(&inputs, &crs, &[0.0, 1.0], 0.01, false, limits(), None)
            .unwrap();
    assert_eq!(result.surfaces.len(), 4);
    for s in &result.surfaces[..2] {
        assert_eq!(s.geometry, Geometry::Polygon(Polygon::empty()));
    }
    assert_eq!(result.surfaces[2].geometry, result.surfaces[3].geometry);
    let united =
        geodesic_line_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, true, limits(), None)
            .unwrap();
    assert_eq!(united.surfaces.len(), 1);
    assert_eq!(united.surfaces[0].geometry, result.surfaces[2].geometry);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn cumulative_budget_and_interruptions() {
    // C1: GOP-027/028, soglia misurata e segnali iniettati senza corse tra thread.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let inputs = [segment(0.0)];
    let baseline =
        geodesic_line_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, limits(), None)
            .unwrap();
    let mut exact = limits();
    exact.overlay.work.max_steps = baseline.work;
    geodesic_line_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, exact, None).unwrap();
    exact.overlay.work.max_steps -= 1;
    assert!(matches!(
        geodesic_line_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, exact, None),
        Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
            PredicateError::WorkLimit
        )))
    ));
    for at in [0, 10, 100, baseline.work / 2, baseline.work - 2] {
        for deadline in [false, true] {
            let flag = AtomicBool::new(false);
            let mut work = Work::new(limits().overlay.work, Some(&flag)).with_deadline_precedence();
            work.interrupt_at = Some((at, deadline));
            let error = execute(&inputs, &crs, &[1.0], 0.01, false, limits(), &mut work);
            if deadline {
                assert!(matches!(
                    error,
                    Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                        PredicateError::DeadlineExceeded
                    )))
                ));
            } else {
                assert!(matches!(
                    error,
                    Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                        PredicateError::Cancelled
                    )))
                ));
            }
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn resource_rejections_and_deadline_precedence() {
    // C1: GOP-027/028, zero e' un budget effettivo, deadline prima del token.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let inputs = [segment(0.0)];
    let mut zero = limits();
    zero.max_results = 0;
    assert!(matches!(
        geodesic_line_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, zero, None),
        Err(UnionBufferError::Disk(BufferFailure::OutputLimit))
    ));
    zero = limits();
    zero.max_vertices = 0;
    assert!(matches!(
        geodesic_line_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, zero, None),
        Err(UnionBufferError::Disk(BufferFailure::VertexLimit))
    ));
    zero = limits();
    zero.overlay.work.max_reserved_bytes = 0;
    assert!(matches!(
        geodesic_line_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, zero, None),
        Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
            PredicateError::ReservationLimit
        )))
    ));
    zero = limits();
    zero.overlay.work.deadline = Some(std::time::Instant::now());
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        geodesic_line_buffer_with_limits(
            &inputs,
            &crs,
            &[1.0],
            0.01,
            false,
            zero,
            Some(&cancelled)
        ),
        Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        )))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn local_scale_central_identity_and_domain_edges() {
    // C1: TM scala unitaria sull'asse centrale; dominio continuo della prova.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let projection = LocalTransverseMercator::new(&crs, 9.0).unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    let certificate = TransverseCertificate::new(
        projection.inverse_shape_bounds(),
        ellipsoid.parametri().inverse_flattening,
    )
    .unwrap();
    let (minimum, maximum) = certificate
        .local_metric_bounds((0.0, -1e6), (0.0, 1e6))
        .unwrap();
    assert!(minimum <= 1.0 && maximum >= 1.0);
    assert!(maximum - minimum < 1e-14);
    let a = ellipsoid.parametri().semi_major_axis_metre;
    for (x, y) in [
        (a * 0.01, 0.0),
        (-a * 0.01, 0.0),
        (0.0, a * 0.9),
        (0.0, -a * 0.9),
    ] {
        assert!(matches!(
            certificate.local_metric_bounds((x, y), (x, y)),
            Err(crate::preserve_shape_measure::ShapeMeasureError::AccuracyUnproven)
        ));
    }
    for latitude in [0.0, 45.0, -45.0] {
        let (x, y) = projection.forward(9.0, latitude).unwrap();
        certificate.certify(x, y, 9.0, latitude).unwrap();
        let distance: f64 = ellipsoid.geodetica().inverse(0.0, 9.0, latitude, 9.0);
        assert!((y.abs() - distance).abs() < 1e-6);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn equatorial_input_sampling_against_analytic_positions() {
    // C1: sull'equatore s=a*delta_lambda, dunque frazione d'arco e lon coincidono.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let line = LineString::from(vec![(9.0, 0.0), (9.001, 0.0)]);
    let inputs = [Geometry::LineString(line.clone())];
    let mut work = Work::new(limits().overlay.work, None).with_deadline_precedence();
    let local = context(&inputs, &crs, &mut work).unwrap();
    let mut vertices = 0;
    let (sampled, bound) =
        projected_line(&line, &local, 0.01, limits(), &mut vertices, &mut work).unwrap();
    assert!(sampled.0.len() > 2);
    assert!(bound > 0.0 && bound < 0.001);
    let divisions = u32::try_from(sampled.0.len() - 1).unwrap();
    for (index, &point) in sampled.0.iter().enumerate() {
        let geographic = local.inverse(point, &mut work).unwrap();
        let fraction = f64::from(u32::try_from(index).unwrap()) / f64::from(divisions);
        let expected = 0.001_f64.mul_add(fraction, 9.0);
        let error = (geographic.x - expected).abs().to_radians()
            * local.ellipsoid.parametri().semi_major_axis_metre;
        assert!(error < 1e-6);
        assert_eq!(geographic.y, 0.0);
    }
}
