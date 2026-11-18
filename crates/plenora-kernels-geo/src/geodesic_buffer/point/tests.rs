use super::*;
use geo::{Area, MultiPoint, Point};
use geographiclib_rs::InverseGeodesic;
use plenora_core::crs::resolve_crs;
use std::sync::atomic::Ordering;
use std::time::Instant;

#[test]
#[deny(clippy::disallowed_methods)]
fn primitive_against_equator_and_published_vector() {
    // G1/G2: all'equatore s=a*delta_lambda; oracolo analitico senza inverse.
    for (axis, inverse_flattening) in [(6e6, 250.0), (7e6, 350.0)] {
        let ellipsoid = EllissoideGeodetico::nuovo(plenora_core::crs::Ellipsoid {
            semi_major_axis_metre: axis,
            inverse_flattening,
        })
        .unwrap();
        for radius in [1.0, 100.0, 60_000.0] {
            let (lat, lon): (f64, f64) = ellipsoid.geodetica().direct(0.0, 0.0, 90.0, radius);
            assert!(lat.abs() * axis <= 1e-7);
            assert!(lon.to_radians().mul_add(axis, -radius).abs() <= 1e-7);
        }
    }
    // G2, geographiclib-rs 0.2.7 README, esempio JFK direct: valori pubblicati.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    let (lat, lon): (f64, f64) = ellipsoid.geodetica().direct(40.64, -73.78, 45.0, 10e6);
    assert!((lat - 32.621_100_463_725_796).abs() * 125_000.0 <= 1e-7);
    assert!((lon - 49.052_487_092_959_82).abs() * 125_000.0 <= 1e-7);
}

fn limits() -> PointBufferLimits {
    PointBufferLimits {
        batch: DegenerateBufferLimits {
            max_work: 10_000_000,
            max_output_items: 100,
            max_reserved_bytes: 100_000_000,
            deadline: None,
        },
        max_vertices: 1_000_000,
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn karney_vertices_dense_boundary_and_reverse_edges() {
    // G1 Karney 2013 direct/inverse; docs/buffer-punto.md, prova metrica GOP-006.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    for p in [
        Point::new(9.0, 45.0),
        Point::new(-170.0, -79.0),
        Point::new(0.0, 0.0),
    ] {
        let result =
            geodesic_point_buffer_with_limits(&[p.into()], &crs, &[2.0], 0.01, limits(), None)
                .unwrap();
        let disk = &result.disks[0];
        let coords = &disk.polygon.exterior().0;
        assert_eq!(coords.first(), coords.last());
        assert!(disk.polygon.signed_area() > 0.0);
        assert!(disk.polygon.interiors().is_empty());
        assert!(disk.certificate.crs.semantically_equals(&crs));
        assert!(disk.certificate.absolute_m <= 0.01);
        assert!(disk.certificate.relative >= disk.certificate.absolute_m / 2.0);
        assert!(
            disk.certificate.absolute_m
                >= disk.certificate.discretization_m
                    + disk.certificate.geographiclib_m
                    + disk.certificate.rounding_m
                    + disk.certificate.model_change_m
        );
        let n = u32::try_from(coords.len() - 1).unwrap();
        for (k, edge) in coords.windows(2).enumerate() {
            let a = edge[0];
            let b = edge[1];
            let radial: f64 = ellipsoid.geodetica().inverse(p.y(), p.x(), a.y, a.x);
            assert!(
                (radial - 2.0).abs()
                    <= disk.certificate.geographiclib_m + disk.certificate.rounding_m
            );
            let (length, azimuth, _, _): (f64, f64, f64, f64) =
                ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
            for j in 0..8_u32 {
                let t = f64::from(j) / 8.0;
                let angle = -360.0 * (f64::from(u32::try_from(k).unwrap()) + t) / f64::from(n);
                let (lat, lon): (f64, f64) = ellipsoid.geodetica().direct(p.y(), p.x(), angle, 2.0);
                // Un estremo appartiene all'anello: distanza dal bordo <= questo testimone.
                let witness = if j <= 4 { a } else { b };
                let distance: f64 = ellipsoid
                    .geodetica()
                    .inverse(lat, lon, witness.y, witness.x);
                assert!(distance <= disk.certificate.absolute_m);
                let (lat, lon): (f64, f64) =
                    ellipsoid.geodetica().direct(a.y, a.x, azimuth, length * t);
                let radial: f64 = ellipsoid.geodetica().inverse(p.y(), p.x(), lat, lon);
                // La geodetica radiale incontra il confine esatto a distanza r.
                assert!((radial - 2.0).abs() <= disk.certificate.absolute_m);
                let linear_lat = (b.y - a.y).mul_add(t, a.y);
                let linear_lon = (b.x - a.x).mul_add(t, a.x);
                let planar_radial: f64 =
                    ellipsoid
                        .geodetica()
                        .inverse(p.y(), p.x(), linear_lat, linear_lon);
                assert!((planar_radial - 2.0).abs() <= disk.certificate.absolute_m);
            }
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn model_change_bound_against_dense_karney_arc() {
    // C1: equazioni metriche e interpolazione, docs/buffer-punto.md.
    // Il punto allo stesso parametro sul segmento e' un testimone nei due versi.
    for (axis, flattening) in [(6e6, 250.0), (7e6, 350.0)] {
        let ellipsoid = EllissoideGeodetico::nuovo(plenora_core::crs::Ellipsoid {
            semi_major_axis_metre: axis,
            inverse_flattening: flattening,
        })
        .unwrap();
        for latitude in [-80.0, -45.0, 0.0, 45.0, 80.0] {
            for azimuth in [0.0, 30.0, 90.0, 135.0, 180.0, 270.0] {
                for h in [1.0, 10.0, 1000.0, 60_000.0] {
                    let (end_lat, end_lon): (f64, f64) =
                        ellipsoid.geodetica().direct(latitude, 10.0, azimuth, h);
                    let bound = side_model_error_m(h).unwrap();
                    for j in 0..=128 {
                        let t = f64::from(j) / 128.0;
                        let (lat, lon): (f64, f64) =
                            ellipsoid.geodetica().direct(latitude, 10.0, azimuth, h * t);
                        let linear_lat = (end_lat - latitude).mul_add(t, latitude);
                        let linear_lon = (end_lon - 10.0).mul_add(t, 10.0);
                        let measured: f64 = ellipsoid
                            .geodetica()
                            .inverse(lat, lon, linear_lat, linear_lon);
                        assert!(measured <= bound);
                    }
                }
            }
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn model_change_controls_planning_and_rejects_invalid_bounds() {
    // C1: il termine quadratico entra nel budget prima della generazione.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let (n, certificate) = plan(1.0, 0.01, &crs).unwrap();
    let old_bound = ((certificate.discretization_m + NUMERIC_M).next_up() + ROUNDING_M).next_up();
    let (refined, finer) = plan(1.0, old_bound, &crs).unwrap();
    assert_eq!(refined, n * 2);
    assert!(finer.absolute_m <= old_bound);
    assert!(finer.model_change_m < certificate.model_change_m);
    assert_eq!(side_model_error_m(0.0).unwrap(), 0.0);
    for h in [-1.0, f64::NAN, f64::INFINITY, 60_000.0_f64.next_up()] {
        assert_eq!(
            side_model_error_m(h),
            Err(DegenerateBufferError::AccuracyUnproven)
        );
    }
    for h in [1e-150, 0.001, 1.0, 1000.0, 60_000.0] {
        assert!(side_model_error_m(h / 2.0).unwrap() < side_model_error_m(h).unwrap());
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn planar_fan_rejects_folded_and_multiple_winding_rings() {
    // C1: orientamenti esatti e numero di giri distinguono un disco da una stella.
    let center = Coord { x: 0.0, y: 0.0 };
    let ring = [
        Coord { x: 1.0, y: 0.0 },
        Coord { x: 0.0, y: 1.0 },
        Coord { x: -1.0, y: 0.0 },
        Coord { x: 0.0, y: -1.0 },
        Coord { x: 1.0, y: 0.0 },
    ];
    verify_planar_fan(center, &ring, || Ok(())).unwrap();
    let mut reversed = ring;
    reversed.reverse();
    assert_eq!(
        verify_planar_fan(center, &reversed, || Ok(())),
        Err(DegenerateBufferError::AccuracyUnproven)
    );
    let doubled: Vec<_> = ring[..4].iter().chain(ring.iter()).copied().collect();
    assert_eq!(
        verify_planar_fan(center, &doubled, || Ok(())),
        Err(DegenerateBufferError::AccuracyUnproven)
    );
    assert_eq!(
        verify_planar_fan(center, &[], || Ok(())),
        Err(DegenerateBufferError::AccuracyUnproven)
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn refinement_duplicates_and_distance_major() {
    // GOP-006: budget dimezzato, piu' campioni e maggiorante minore.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let p = Point::new(10.0, 20.0);
    let inputs = [p.into(), MultiPoint::new(vec![p, p, p]).into()];
    let a = geodesic_point_buffer_with_limits(&inputs, &crs, &[1.0, 2.0], 0.01, limits(), None)
        .unwrap();
    let b = geodesic_point_buffer_with_limits(&inputs, &crs, &[1.0, 2.0], 0.005, limits(), None)
        .unwrap();
    assert_eq!(a.disks.len(), 4);
    assert_eq!(a.disks[0].polygon, a.disks[1].polygon);
    assert_eq!(a.disks[2].polygon, a.disks[3].polygon);
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    for (i, (coarse, fine)) in a.disks.iter().zip(&b.disks).enumerate() {
        assert!(fine.polygon.exterior().0.len() > coarse.polygon.exterior().0.len());
        assert!(fine.certificate.absolute_m < coarse.certificate.absolute_m);
        let q = coarse.polygon.exterior().0[0];
        let distance: f64 = ellipsoid.geodetica().inverse(p.y(), p.x(), q.y, q.x);
        let expected = if i < 2 { 1.0 } else { 2.0 };
        assert!((distance - expected).abs() <= coarse.certificate.absolute_m);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn rejects_surface_profile_and_precision_domains() {
    // GOP-005: nessun complemento globale, polo o seam accettato implicitamente.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    for (p, r) in [
        (Point::new(0.0, 0.0), 10_000_000.0),
        (Point::new(0.0, 89.999), 1000.0),
        (Point::new(179.999, 0.0), 1000.0),
        (Point::new(-179.999, 0.0), 1000.0),
        (Point::new(0.0, 0.0), 0.5),
    ] {
        assert!(matches!(
            geodesic_point_buffer_with_limits(&[p.into()], &crs, &[r], 0.01, limits(), None),
            Err(DegenerateBufferError::UnsupportedSurface)
        ));
    }
    // Profilo senza unione: distinti, vuoti, linee e superfici non sono dischi.
    for input in [
        MultiPoint::new(vec![Point::new(0.0, 0.0), Point::new(1.0, 0.0)]).into(),
        MultiPoint::new(Vec::new()).into(),
        LineString::from(vec![(0.0, 0.0), (1.0, 0.0)]).into(),
        Polygon::new(
            LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.0, 0.0)]),
            vec![],
        )
        .into(),
    ] {
        assert!(matches!(
            geodesic_point_buffer_with_limits(&[input], &crs, &[1.0], 0.01, limits(), None),
            Err(DegenerateBufferError::UnsupportedProfile)
        ));
    }
    // GOP-006: precisione non raggiungibile distinta dalla configurazione invalida.
    assert!(matches!(
        geodesic_point_buffer_with_limits(
            &[Point::new(0.0, 0.0).into()],
            &crs,
            &[1.0],
            1e-8,
            limits(),
            None
        ),
        Err(DegenerateBufferError::AccuracyUnproven)
    ));
    for tolerance in [0.0, f64::NAN, f64::INFINITY, -1.0] {
        assert!(matches!(
            geodesic_point_buffer_with_limits(&[], &crs, &[], tolerance, limits(), None),
            Err(DegenerateBufferError::InvalidTolerance)
        ));
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_vertex_memory_work_thresholds_and_atomic_batch() {
    // GOP-026/027: limite incluso, un'unita' in meno rifiutata prima della crescita.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let inputs = [Point::new(0.0, 0.0).into()];
    let result =
        geodesic_point_buffer_with_limits(&inputs, &crs, &[1.0, 2.0], 0.01, limits(), None)
            .unwrap();
    let mut exact = limits();
    exact.max_vertices = result.vertices;
    exact.batch.max_reserved_bytes = result.peak_reserved_bytes;
    exact.batch.max_work = result.work;
    exact.batch.max_output_items = 2;
    assert_eq!(
        geodesic_point_buffer_with_limits(&inputs, &crs, &[1.0, 2.0], 0.01, exact, None)
            .unwrap()
            .vertices,
        result.vertices
    );
    for error in [
        DegenerateBufferError::VertexLimit,
        DegenerateBufferError::MemoryLimit,
        DegenerateBufferError::WorkLimit,
        DegenerateBufferError::OutputLimit,
    ] {
        let mut budget = exact;
        match error {
            DegenerateBufferError::VertexLimit => budget.max_vertices -= 1,
            DegenerateBufferError::MemoryLimit => budget.batch.max_reserved_bytes -= 1,
            DegenerateBufferError::WorkLimit => budget.batch.max_work -= 1,
            _ => budget.batch.max_output_items -= 1,
        }
        assert!(
            matches!(geodesic_point_buffer_with_limits(&inputs, &crs, &[1.0, 2.0], 0.01, budget, None), Err(actual) if actual == error)
        );
    }
    assert!(matches!(
        geodesic_point_buffer_with_limits(&inputs, &crs, &[1.0, f64::NAN], 0.01, limits(), None),
        Err(DegenerateBufferError::InvalidRadius)
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn cancellation_and_deadline_inside_generation() {
    // GOP-028: stessa ring di produzione, segnale al checkpoint dopo 511 direct.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    for deadline in [false, true] {
        let cancel = AtomicBool::new(false);
        let mut work = BufferWork {
            limits: limits().batch,
            cancel: Some(&cancel),
            count: 0,
        };
        let mut checkpoints = 0;
        let result = ring(Coord { x: 0.0, y: 0.0 }, 2.0, 1024, &ellipsoid, || {
            checkpoints += 1;
            if checkpoints == 513 {
                cancel.store(true, Ordering::Relaxed);
                if deadline {
                    work.limits.deadline = Some(Instant::now());
                }
            }
            work.step()
        });
        assert_eq!(checkpoints, 513);
        assert_eq!(work.count, 512);
        let expected = if deadline {
            DegenerateBufferError::DeadlineExceeded
        } else {
            DegenerateBufferError::Cancelled
        };
        assert!(matches!(result, Err(actual) if actual == expected));
    }
}
