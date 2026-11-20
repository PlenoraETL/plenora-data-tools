use super::*;
use geo::{Contains, CoordsIter, MultiPoint, Point};
use geographiclib_rs::{DirectGeodesic, InverseGeodesic};
use plenora_core::crs::resolve_crs;

#[deny(clippy::disallowed_methods)]
fn limits() -> UnionBufferLimits {
    UnionBufferLimits {
        overlay: PolygonBooleanLimits {
            work: crate::predicates::RelateLimits {
                max_steps: 2_000_000_000,
                max_nodes: 100_000_000,
                ..Default::default()
            },
            ..Default::default()
        },
        max_results: 20,
        max_vertices: 100_000,
    }
}

#[deny(clippy::disallowed_methods)]
fn input(offsets: &[f64]) -> Geometry {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    Geometry::MultiPoint(MultiPoint::new(
        offsets
            .iter()
            .map(|&distance| {
                let (lat, lon) = ellipsoid.geodetica().direct(45.0, 9.0, 90.0, distance);
                Point::new(lon, lat)
            })
            .collect(),
    ))
}

#[deny(clippy::disallowed_methods)]
fn polygons(g: &Geometry) -> Vec<&geo::Polygon> {
    match g {
        Geometry::Polygon(p) => vec![p],
        Geometry::MultiPolygon(p) => p.0.iter().collect(),
        _ => panic!("superficie richiesta"),
    }
}

#[deny(clippy::disallowed_methods)]
fn boundary_oracle(g: &Geometry, centers: &Geometry, radius: f64, bound: f64) {
    // G1 Karney direct/inverse; GOP-006 e composizione in docs/buffer-punto.md.
    // Si campiona il confine esatto esposto, escludendo gli archi coperti.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    let centers: Vec<_> = points(centers).unwrap().collect();
    for center in &centers {
        for k in 0..96 {
            let (lat, lon): (f64, f64) =
                ellipsoid
                    .geodetica()
                    .direct(center.y, center.x, f64::from(k) * 3.75, radius);
            if centers.iter().any(|c| {
                let distance: f64 = ellipsoid.geodetica().inverse(lat, lon, c.y, c.x);
                distance < radius - 1e-7
            }) {
                continue;
            }
            let mut best = f64::INFINITY;
            for polygon in polygons(g) {
                for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                    for edge in ring.0.windows(2) {
                        let (a, b) = (edge[0], edge[1]);
                        let (dx, dy) = (b.x - a.x, b.y - a.y);
                        let t = ((lat - a.y).mul_add(dy, (lon - a.x) * dx)
                            / dy.mul_add(dy, dx * dx))
                        .clamp(0.0, 1.0);
                        let distance: f64 =
                            ellipsoid
                                .geodetica()
                                .inverse(lat, lon, a.y + t * dy, a.x + t * dx);
                        best = best.min(distance);
                    }
                }
            }
            assert!(best <= bound);
        }
    }
    for c in g.coords_iter() {
        let nearest = centers
            .iter()
            .map(|center| {
                let d: f64 = ellipsoid.geodetica().inverse(c.y, c.x, center.y, center.x);
                (d - radius).max(0.0)
            })
            .fold(f64::INFINITY, f64::min);
        assert!(nearest <= bound);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn disjoint_overlap_and_three_centers() {
    // GOP-006: unione dei dischi esatti; G1 Karney come oracolo metrico.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    for (offsets, parts) in [
        (vec![0.0, 3.0], 2),
        (vec![0.0, 1.0], 1),
        (vec![0.0, 1.0, 2.0], 1),
    ] {
        let input = input(&offsets);
        let result = geodesic_multipoint_buffer_with_limits(
            std::slice::from_ref(&input),
            &crs,
            &[1.0],
            0.01,
            false,
            limits(),
            None,
        )
        .unwrap();
        let surface = &result.surfaces[0];
        assert_eq!(polygons(&surface.geometry).len(), parts);
        assert!(surface.absolute_m >= surface.disk_certificate.absolute_m + surface.overlay_m);
        assert!(surface.disk_certificate.model_change_m > 0.0);
        boundary_oracle(&surface.geometry, &input, 1.0, surface.absolute_m);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn distance_major_nested_radii_and_union_per_distance() {
    // GOP-006/008: i buffer a raggio crescente sono inclusi; unione per distanza.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let inputs = [input(&[0.0]), input(&[0.25])];
    let separate = geodesic_multipoint_buffer_with_limits(
        &inputs,
        &crs,
        &[1.0, 2.0],
        0.01,
        false,
        limits(),
        None,
    )
    .unwrap();
    assert_eq!(separate.surfaces.len(), 4);
    assert!(polygons(&separate.surfaces[3].geometry)[0]
        .contains(polygons(&separate.surfaces[0].geometry)[0]));
    let united = geodesic_multipoint_buffer_with_limits(
        &inputs,
        &crs,
        &[1.0, 2.0],
        0.01,
        true,
        limits(),
        None,
    )
    .unwrap();
    assert_eq!(united.surfaces.len(), 2);
    for (surface, radius) in united.surfaces.iter().zip([1.0, 2.0]) {
        assert_eq!(polygons(&surface.geometry).len(), 1);
        boundary_oracle(
            &surface.geometry,
            &input(&[0.0, 0.25]),
            radius,
            surface.absolute_m,
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn near_tangency_is_certified_or_typed_rejection() {
    // GOP-015A: il rounding ambiguo deve essere rifiutato dalla 1c-c.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let input = input(&[0.0, 2.0_f64.next_down()]);
    match geodesic_multipoint_buffer_with_limits(
        std::slice::from_ref(&input),
        &crs,
        &[1.0],
        0.01,
        false,
        limits(),
        None,
    ) {
        Ok(result) => boundary_oracle(
            &result.surfaces[0].geometry,
            &input,
            1.0,
            result.surfaces[0].absolute_m,
        ),
        Err(UnionBufferError::Overlay(
            PolygonBooleanError::InvalidRoundedTopology | PolygonBooleanError::UnsupportedOperands,
        )) => (),
        Err(error) => panic!("errore inatteso: {error}"),
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn cumulative_exact_budget_and_interruptions_inside_union() {
    // GOP-027/028: un unico Work per tutte le unioni; soglie esatte e checkpoint.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let inputs = [input(&[0.0, 1.0, 2.0])];
    let limits = limits();
    let mut measured = Work::new(limits.overlay.work, None);
    let result = execute(&inputs, &crs, &[1.0], 0.01, false, limits, &mut measured).unwrap();
    let mut exact = limits;
    exact.overlay.work.max_steps = result.work;
    exact.overlay.work.max_nodes = measured.nodes;
    exact.overlay.work.max_segment_pairs = measured.pairs;
    exact.overlay.work.max_reserved_bytes = measured.peak_bytes();
    exact.overlay.max_edges = measured.polygon_counts[0];
    exact.overlay.max_faces = measured.polygon_counts[1];
    exact.overlay.max_rings = measured.polygon_counts[2];
    geodesic_multipoint_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, exact, None)
        .unwrap();
    for field in 0..4 {
        let mut short = exact;
        match field {
            0 => short.overlay.work.max_steps -= 1,
            1 => short.overlay.max_edges -= 1,
            2 => short.overlay.max_faces -= 1,
            _ => short.overlay.max_rings -= 1,
        }
        assert!(matches!(
            geodesic_multipoint_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, short, None),
            Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                PredicateError::WorkLimit
            )))
        ));
    }
    // Il marcatore conferma che il checkpoint interrompe la booleana.
    for deadline in [false, true] {
        let flag = AtomicBool::new(false);
        let mut work = Work::new(limits.overlay.work, Some(&flag));
        work.interrupt_at = Some((result.work / 2, deadline));
        let error = execute(&inputs, &crs, &[1.0], 0.01, false, limits, &mut work);
        assert!(work.polygon_counts[0] > 0);
        assert!(work.polygon_active);
        assert!(match error {
            Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                PredicateError::DeadlineExceeded,
            ))) => deadline,
            Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                PredicateError::Cancelled,
            ))) => !deadline,
            _ => false,
        });
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn global_surface_is_rejected_before_generation() {
    // GOP-005: la fascia di ogni unione deve restare sotto mezzo ellissoide.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let inputs = [
        Geometry::Point(Point::new(-100.0, 0.0)),
        Geometry::Point(Point::new(100.0, 0.0)),
    ];
    assert!(matches!(
        geodesic_multipoint_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, true, limits(), None),
        Err(UnionBufferError::Disk(
            DegenerateBufferError::UnsupportedSurface
        ))
    ));
    let separate =
        geodesic_multipoint_buffer_with_limits(&inputs, &crs, &[1.0], 0.01, false, limits(), None)
            .unwrap();
    assert_eq!(separate.surfaces.len(), 2);
}
