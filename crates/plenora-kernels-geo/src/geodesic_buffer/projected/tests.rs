use super::*;
use crate::polygon_boolean::PolygonBooleanError;
use crate::predicates::PredicateError;
use geo::{CoordsIter, MapCoords, MultiLineString};
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

// L'indice si costruisce una volta per superficie. Le distanze floating
// point scelgono solo il candidato iniziale; il raggio e il minimo sono esatti.
struct BoundaryWitness {
    edges: Vec<(Coord, Coord)>,
    tree: rstar::RTree<
        rstar::primitives::GeomWithData<rstar::primitives::Rectangle<[f64; 2]>, usize>,
    >,
}

#[deny(clippy::disallowed_methods)]
impl BoundaryWitness {
    fn new(geometry: &Geometry) -> Self {
        let polygons = match geometry {
            Geometry::Polygon(p) => std::slice::from_ref(p),
            Geometry::MultiPolygon(p) => p.0.as_slice(),
            _ => panic!("superficie richiesta"),
        };
        let edges: Vec<_> = polygons
            .iter()
            .flat_map(|p| {
                std::iter::once(p.exterior())
                    .chain(p.interiors())
                    .flat_map(|ring| ring.0.windows(2).map(|pair| (pair[0], pair[1])))
            })
            .collect();
        let tree = rstar::RTree::bulk_load(
            edges
                .iter()
                .enumerate()
                .map(|(id, (a, b))| {
                    rstar::primitives::GeomWithData::new(
                        rstar::primitives::Rectangle::from_corners(
                            [a.x.min(b.x), a.y.min(b.y)],
                            [a.x.max(b.x), a.y.max(b.y)],
                        ),
                        id,
                    )
                })
                .collect(),
        );
        Self { edges, tree }
    }

    fn distance(&self, point: Coord, ellipsoid: &EllissoideGeodetico, indexed: bool) -> f64 {
        let mut work = Work::new(limits().overlay.work, None);
        let candidates = if indexed {
            let Some(seed) = self.tree.nearest_neighbor(&[point.x, point.y]) else {
                return f64::INFINITY;
            };
            let (a, b) = self.edges[seed.data];
            let key =
                crate::segment_index::DistanceKey::coordinates(point, a, b, &mut work).unwrap();
            // Se la radice non e' rappresentabile, nessun pruning: il minimo
            // razionale resta definito e viene confrontato senza arrotondarlo.
            let radius = match key.rounded(&mut work) {
                Ok(value) => value.next_up(),
                Err(PredicateError::AccuracyUnproven) => f64::INFINITY,
                Err(error) => panic!("{error}"),
            };
            let window = crate::segment_index::metric_window([point.x, point.y], radius);
            let mut ids: Vec<_> = self
                .tree
                .locate_in_envelope_intersecting(&window)
                .map(|e| e.data)
                .collect();
            ids.sort_unstable();
            ids
        } else {
            (0..self.edges.len()).collect()
        };
        let mut best: Option<crate::segment_index::DistanceKey> = None;
        let mut witness = (point, point);
        for id in candidates {
            let (a, b) = self.edges[id];
            let distance =
                crate::segment_index::DistanceKey::coordinates(point, a, b, &mut work).unwrap();
            if best
                .as_ref()
                .is_none_or(|previous| distance.compare(previous).unwrap().is_lt())
            {
                best = Some(distance);
                witness = (a, b);
            }
        }
        if best.is_none() {
            return f64::INFINITY;
        }
        let (a, b) = witness;
        let fraction = crate::segment_index::projection_parameter(point, a, b, &mut work).unwrap();
        let witness = Coord {
            x: (b.x - a.x).mul_add(fraction, a.x),
            y: (b.y - a.y).mul_add(fraction, a.y),
        };
        ellipsoid
            .geodetica()
            .inverse(point.y, point.x, witness.y, witness.x)
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn indexed_boundary_witness_matches_exhaustive_bits() {
    // C1: identico lato vincente, tie in ordine d'ingresso, piede e Karney;
    // foro, lati degeneri/obliqui e punti dentro, fuori e sul bordo.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    let geometry = Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (0.0, 0.0),
            (0.04, 0.0),
            (0.05, 0.04),
            (0.0, 0.04),
            (0.0, 0.0),
        ]),
        vec![LineString::from(vec![
            (0.01, 0.01),
            (0.02, 0.01),
            (0.02, 0.02),
            (0.01, 0.02),
            (0.01, 0.01),
        ])],
    ));
    let boundary = BoundaryWitness::new(&geometry);
    for x in -1..=6 {
        for y in -1..=5 {
            let point = Coord {
                x: f64::from(x) / 100.0,
                y: f64::from(y) / 100.0,
            };
            assert_eq!(
                boundary.distance(point, &ellipsoid, true).to_bits(),
                boundary.distance(point, &ellipsoid, false).to_bits()
            );
        }
    }
}

#[deny(clippy::disallowed_methods)]
fn dense_exact_boundary(
    surface: &ProjectedBufferSurface<'_>,
    latitude: f64,
    radius: f64,
    ellipsoid: &EllissoideGeodetico,
) -> f64 {
    // C1: campo di Fermi e calotte terminali, docs/buffer-punto.md; Karney G1.
    // 2052 campioni del confine esatto confrontati con testimoni dell'output.
    let geodesic = ellipsoid.geodetica();
    let (length, azimuth, end_azimuth, _): (f64, f64, f64, f64) =
        geodesic.inverse(latitude, 9.0, latitude, 9.00001);
    let boundary = BoundaryWitness::new(&surface.geometry);
    let mut measured = 0.0_f64;
    for i in 0..=512 {
        let fraction = f64::from(i) / 512.0;
        for (longitude, start) in [(9.0, azimuth + 90.0), (9.00001, end_azimuth - 90.0)] {
            let (y, x): (f64, f64) = geodesic.direct(
                latitude,
                longitude,
                180.0_f64.mul_add(fraction, start),
                radius,
            );
            let error = boundary.distance(Coord { x, y }, ellipsoid, true);
            measured = measured.max(error);
            assert!(error <= surface.certificate.absolute_m);
        }
        let (lat, lon, tangent): (f64, f64, f64) =
            geodesic.direct(latitude, 9.0, azimuth, length * fraction);
        for side in [-90.0, 90.0] {
            let (y, x): (f64, f64) = geodesic.direct(lat, lon, tangent + side, radius);
            let error = boundary.distance(Coord { x, y }, ellipsoid, true);
            measured = measured.max(error);
            assert!(error <= surface.certificate.absolute_m);
        }
    }
    measured
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
        let mut work = Work::new(limits().overlay.work, None);
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
                    let t =
                        crate::segment_index::projection_parameter(Coord { x, y }, a, b, &mut work)
                            .unwrap();
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
    // Tolleranze esplicite per il bound quadratico; 70 gradi fuori dominio TM.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    for latitude in [0.0, 45.0, 70.0] {
        for radius in [100.0, 1000.0, 10000.0, 60000.0] {
            let tolerance = if radius < 10000.0 {
                0.01
            } else if radius == 10000.0 {
                10.0
            } else {
                2000.0
            };
            let result = geodesic_line_buffer_with_limits(
                &[segment(latitude)],
                &crs,
                &[radius],
                tolerance,
                false,
                limits(),
                None,
            );
            if latitude == 70.0 {
                assert!(matches!(
                    result,
                    Err(UnionBufferError::Disk(BufferFailure::UnsupportedSurface))
                ));
            } else {
                let surface = result.unwrap().surfaces.remove(0);
                let mut measured = 0.0_f64;
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
                    let error = (distance - radius).abs();
                    measured = measured.max(error);
                    assert!(error <= surface.certificate.absolute_m);
                }
                assert!(surface.certificate.absolute_m <= tolerance);
                assert!(surface.certificate.relative >= surface.certificate.absolute_m / radius);
                measured =
                    measured.max(dense_exact_boundary(&surface, latitude, radius, &ellipsoid));
                eprintln!("matrix lat={latitude} radius={radius} measured={measured:e} certified={:e} measured/r={:e} certified/r={:e}", surface.certificate.absolute_m, measured / radius, surface.certificate.relative);
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
            let mut work = Work::new(limits().overlay.work, Some(&flag));
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
        (a * 0.03, 0.0),
        (-a * 0.03, 0.0),
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
        for offset in [-0.029, -0.02, 0.0, 0.02, 0.029] {
            let x = a * offset;
            let (low, high) = certificate
                .local_metric_bounds((x - 1.0, y - 1.0), (x + 1.0, y + 1.0))
                .unwrap();
            let (lon, lat) = projection.inverse(x, y).unwrap();
            certificate.certify(x, y, lon, lat).unwrap();
            for (dx, dy) in [(1.0, 0.0), (0.0, 1.0)] {
                let (end_lon, end_lat) = projection.inverse(x + dx, y + dy).unwrap();
                let distance: f64 = ellipsoid.geodetica().inverse(lat, lon, end_lat, end_lon);
                assert!(distance + 1e-6 >= low && distance - 1e-6 <= high);
            }
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn equatorial_input_sampling_against_analytic_positions() {
    // C1: sull'equatore s=a*delta_lambda, dunque frazione d'arco e lon coincidono.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let line = LineString::from(vec![(9.0, 0.0), (9.001, 0.0)]);
    let inputs = [Geometry::LineString(line.clone())];
    let mut work = Work::new(limits().overlay.work, None);
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

#[deny(clippy::disallowed_methods)]
fn square_with_hole(latitude: f64) -> Polygon {
    let ring = |width: f64| {
        LineString::from(vec![
            (9.0 - width, latitude - width),
            (9.0 + width, latitude - width),
            (9.0 + width, latitude + width),
            (9.0 - width, latitude + width),
            (9.0 - width, latitude - width),
        ])
    };
    let mut hole = ring(0.000_003);
    hole.0.reverse();
    Polygon::new(ring(0.00001), vec![hole])
}

#[deny(clippy::disallowed_methods)]
fn distance_to_boundary(point: Coord, polygon: &Polygon, ellipsoid: &EllissoideGeodetico) -> f64 {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .flat_map(|ring| ring.0.windows(2))
        .map(|pair| distance_to_segment(point, pair[0], pair[1], ellipsoid))
        .fold(f64::INFINITY, f64::min)
}

#[deny(clippy::disallowed_methods)]
fn dense_surface_boundary(
    surface: &ProjectedBufferSurface<'_>,
    source: &Polygon,
    radius: f64,
    ellipsoid: &EllissoideGeodetico,
) -> f64 {
    use geo::Contains;
    // C1: GOP-008, confine del vicinato metrico: campi di Fermi e cerchi
    // ai vertici (Karney G1). Si tengono i campioni esposti all'unione.
    let mut measured = 0.0_f64;
    let mut accepted = 0;
    let boundary = BoundaryWitness::new(&surface.geometry);
    let mut check = |point: Coord| {
        if !source.contains(&geo::Point(point))
            && (distance_to_boundary(point, source, ellipsoid) - radius).abs() < 1e-6
        {
            let error = boundary.distance(point, ellipsoid, true);
            assert!(error <= surface.certificate.absolute_m);
            measured = measured.max(error);
            accepted += 1;
        }
    };
    for ring in std::iter::once(source.exterior()).chain(source.interiors()) {
        for pair in ring.0.windows(2) {
            let a = pair[0];
            let b = pair[1];
            let (length, azimuth, _, _): (f64, f64, f64, f64) =
                ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
            for i in 0..=128 {
                let fraction = f64::from(i) / 128.0;
                let (lat, lon, tangent): (f64, f64, f64) =
                    ellipsoid
                        .geodetica()
                        .direct(a.y, a.x, azimuth, length * fraction);
                for normal in [-90.0, 90.0] {
                    let (y, x): (f64, f64) =
                        ellipsoid
                            .geodetica()
                            .direct(lat, lon, tangent + normal, radius);
                    check(Coord { x, y });
                }
                let (y, x): (f64, f64) =
                    ellipsoid
                        .geodetica()
                        .direct(a.y, a.x, 360.0 * fraction, radius);
                check(Coord { x, y });
            }
        }
    }
    assert!(accepted >= 500);
    measured
}

#[test]
#[deny(clippy::disallowed_methods)]
fn surface_hole_zero_multipart_and_certified_collapse() {
    // C1: GOP-008, operando a zero, dilatazione di guscio e buchi, vuoto tipizzato.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    let polygon = square_with_hole(0.0);
    let input = Geometry::Polygon(polygon.clone());
    let multi = Geometry::MultiPolygon(MultiPolygon(vec![polygon.clone()]));
    let result = geodesic_line_buffer_with_limits(
        &[input.clone(), multi.clone()],
        &crs,
        &[0.0, -100.0],
        0.1,
        false,
        limits(),
        None,
    )
    .unwrap();
    assert_eq!(result.surfaces[0].geometry, input);
    assert_eq!(result.surfaces[1].geometry, multi);
    for surface in &result.surfaces[..2] {
        assert!(surface.preserves_geodesic_operand);
        assert_eq!(surface.certificate.absolute_m, 0.0);
    }
    for surface in &result.surfaces[2..] {
        assert_eq!(surface.geometry, Geometry::Polygon(Polygon::empty()));
        assert_eq!(surface.certificate.absolute_m, 0.0);
    }
    let buffered =
        geodesic_line_buffer_with_limits(&[input, multi], &crs, &[0.1], 0.1, true, limits(), None)
            .unwrap();
    let surface = &buffered.surfaces[0];
    assert!(!surface.preserves_geodesic_operand);
    let output = match &surface.geometry {
        Geometry::Polygon(polygon) => std::slice::from_ref(polygon),
        Geometry::MultiPolygon(polygons) => &polygons.0,
        _ => panic!("superficie richiesta"),
    };
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].interiors().len(), 1);
    for point in surface.geometry.coords_iter() {
        assert!(
            (distance_to_boundary(point, &polygon, &ellipsoid) - 0.1).abs()
                <= surface.certificate.absolute_m
        );
    }
    dense_surface_boundary(surface, &polygon, 0.1, &ellipsoid);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn surface_metric_matrix() {
    // C1: GOP-006/008, Karney G1; certificato contro vertici e confine esatto denso.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    for latitude in [0.0, 45.0, 70.0] {
        for (radius, tolerance) in [
            (100.0, 0.1),
            (1000.0, 1.0),
            (10000.0, 10.0),
            (60000.0, 2000.0),
        ] {
            let polygon = square_with_hole(latitude);
            let result = geodesic_line_buffer_with_limits(
                &[Geometry::Polygon(polygon.clone())],
                &crs,
                &[radius],
                tolerance,
                false,
                limits(),
                None,
            );
            if latitude == 70.0 {
                assert!(matches!(
                    result,
                    Err(UnionBufferError::Disk(BufferFailure::UnsupportedSurface))
                ));
                continue;
            }
            let surface = result.unwrap().surfaces.remove(0);
            let mut measured = 0.0_f64;
            for point in surface.geometry.coords_iter() {
                let error = (distance_to_boundary(point, &polygon, &ellipsoid) - radius).abs();
                assert!(error <= surface.certificate.absolute_m);
                measured = measured.max(error);
            }
            measured = measured.max(dense_surface_boundary(
                &surface, &polygon, radius, &ellipsoid,
            ));
            eprintln!("surface lat={latitude} radius={radius} measured={measured:e} certified={:e} measured/r={:e} certified/r={:e}", surface.certificate.absolute_m, measured / radius, surface.certificate.relative);
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn surface_work_interruptions_and_explicit_limits() {
    // C1: GOP-027/028, un unico contatore anche in validazione, copia e collasso.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let inputs = [Geometry::Polygon(square_with_hole(0.0))];
    let baseline =
        geodesic_line_buffer_with_limits(&inputs, &crs, &[0.0, -100.0], 0.1, false, limits(), None)
            .unwrap();
    let mut exact = limits();
    exact.overlay.work.max_steps = baseline.work;
    geodesic_line_buffer_with_limits(&inputs, &crs, &[0.0, -100.0], 0.1, false, exact, None)
        .unwrap();
    exact.overlay.work.max_steps -= 1;
    assert!(matches!(
        geodesic_line_buffer_with_limits(&inputs, &crs, &[0.0, -100.0], 0.1, false, exact, None),
        Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
            PredicateError::WorkLimit
        )))
    ));
    for at in [0, 30, baseline.work / 2, baseline.work - 2] {
        for deadline in [false, true] {
            let flag = AtomicBool::new(false);
            let mut work = Work::new(limits().overlay.work, Some(&flag));
            work.interrupt_at = Some((at, deadline));
            let result = execute(
                &inputs,
                &crs,
                &[0.0, -100.0],
                0.1,
                false,
                limits(),
                &mut work,
            );
            if deadline {
                assert!(matches!(
                    result,
                    Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                        PredicateError::DeadlineExceeded
                    )))
                ));
            } else {
                assert!(matches!(
                    result,
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
fn surface_zero_union_and_domain_limits() {
    // C1: GOP-008, identita', unione disgiunta, vuoti e rifiuti del dominio.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let inputs = [Geometry::Polygon(square_with_hole(0.0))];
    let identical = geodesic_line_buffer_with_limits(
        &[inputs[0].clone(), inputs[0].clone()],
        &crs,
        &[0.0],
        0.1,
        true,
        limits(),
        None,
    )
    .unwrap();
    assert_eq!(identical.surfaces[0].geometry, inputs[0]);
    let second = square_with_hole(0.0).map_coords(|point| Coord {
        x: point.x + 0.001,
        y: point.y,
    });
    let disjoint = geodesic_line_buffer_with_limits(
        &[inputs[0].clone(), Geometry::Polygon(second)],
        &crs,
        &[0.0],
        0.1,
        true,
        limits(),
        None,
    )
    .unwrap();
    let Geometry::MultiPolygon(parts) = &disjoint.surfaces[0].geometry else {
        panic!("multipoligono richiesto")
    };
    assert_eq!(parts.0.len(), 2);
    assert!(matches!(
        geodesic_line_buffer_with_limits(
            &[segment(0.0)],
            &crs,
            &[-1.0],
            0.1,
            false,
            limits(),
            None
        ),
        Err(UnionBufferError::Disk(BufferFailure::InvalidRadius))
    ));
    assert!(matches!(
        geodesic_line_buffer_with_limits(&inputs, &crs, &[10000.0], 0.01, false, limits(), None),
        Err(UnionBufferError::Disk(BufferFailure::AccuracyUnproven))
    ));
    let empty = Geometry::MultiPolygon(MultiPolygon(Vec::new()));
    let result = geodesic_line_buffer_with_limits(
        &[empty],
        &crs,
        &[0.0, -1.0, 1.0],
        0.1,
        false,
        limits(),
        None,
    )
    .unwrap();
    for surface in result.surfaces {
        assert_eq!(surface.geometry, Geometry::Polygon(Polygon::empty()));
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn surface_rejects_orphan_and_empty_holes() {
    // C1: GOP-002/005, un buco non puo' sostituire il guscio vuoto.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ring = square_with_hole(0.0).exterior().clone();
    for polygon in [
        Polygon::new(LineString::new(Vec::new()), vec![ring.clone()]),
        Polygon::new(ring, vec![LineString::new(Vec::new())]),
    ] {
        for radius in [0.0, 1.0, -100.0] {
            assert!(matches!(
                geodesic_line_buffer_with_limits(
                    &[Geometry::Polygon(polygon.clone())],
                    &crs,
                    &[radius],
                    0.1,
                    false,
                    limits(),
                    None
                ),
                Err(UnionBufferError::Disk(BufferFailure::InvalidGeometry))
            ));
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn erosion_square_and_hole_karney_width() {
    // C1: GOP-006/008; Karney G1 inverse, docs/buffer-proiezione.md,
    // copertura continua dell'erosione e distanza delle due frontiere opposte.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
    // Questo e' un oracolo geometrico; le soglie di risorsa sono provate
    // separatamente. Le query metriche esatte restano tutte contabilizzate.
    let mut budget = limits();
    budget.overlay.work.max_steps = 16_000_000_000;
    budget.overlay.work.max_segment_pairs = 100_000_000;
    for latitude in [0.0, 45.0] {
        for hole in [false, true] {
            let source = square_with_hole(latitude);
            let source = if hole {
                source
            } else {
                Polygon::new(source.exterior().clone(), vec![])
            };
            let input = Geometry::Polygon(source.clone());
            let surface = geodesic_line_buffer_with_limits(
                &[input],
                &crs,
                &[-0.1],
                0.01,
                false,
                budget,
                None,
            )
            .unwrap()
            .surfaces
            .remove(0);
            assert!(surface.geometry.coords_iter().next().is_some());
            for point in surface.geometry.coords_iter() {
                let error = (distance_to_boundary(point, &source, &ellipsoid) - 0.1).abs();
                assert!(error <= surface.certificate.absolute_m);
            }
            // Intersezione con il parallelo centrale: i testimoni sono sui
            // segmenti lon/lat pubblicati; inverse misura la larghezza residua.
            let mut crossings = Vec::new();
            for ring in parts(&surface.geometry).unwrap() {
                for pair in ring.0.windows(2) {
                    if (pair[0].y > latitude) != (pair[1].y > latitude) {
                        let fraction = (latitude - pair[0].y) / (pair[1].y - pair[0].y);
                        crossings.push((pair[1].x - pair[0].x).mul_add(fraction, pair[0].x));
                    }
                }
            }
            crossings.sort_by(f64::total_cmp);
            assert_eq!(crossings.len(), if hole { 4 } else { 2 });
            let original_width: f64 = if hole {
                ellipsoid
                    .geodetica()
                    .inverse(latitude, 9.0 - 0.00001, latitude, 9.0 - 0.000_003)
            } else {
                ellipsoid
                    .geodetica()
                    .inverse(latitude, 9.0 - 0.00001, latitude, 9.0 + 0.00001)
            };
            let width: f64 =
                ellipsoid
                    .geodetica()
                    .inverse(latitude, crossings[0], latitude, crossings[1]);
            let error = (width - (original_width - 0.2)).abs();
            assert!(error <= 2.0 * surface.certificate.absolute_m);
            eprintln!("erosion lat={latitude} hole={hole} width={width:e} error={error:e} certificate={:e}", surface.certificate.absolute_m);
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn erosion_partial_and_total_collapse() {
    // C1: GOP-008, collasso e componenti residue; copertura continua in
    // docs/buffer-proiezione.md, nessuna inferenza dal solo numero di vertici.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let large = Polygon::new(square_with_hole(0.0).exterior().clone(), vec![]);
    let small = large.map_coords(|p| Coord {
        x: 9.001 + (p.x - 9.0) / 20.0,
        y: p.y / 20.0,
    });
    let input = Geometry::MultiPolygon(MultiPolygon(vec![large.clone(), small]));
    let partial =
        geodesic_line_buffer_with_limits(&[input], &crs, &[-0.1], 0.01, false, limits(), None)
            .unwrap();
    assert_eq!(polygons(&partial.surfaces[0].geometry).len(), 1);
    for radius in [1.2, 100.0] {
        let total = geodesic_line_buffer_with_limits(
            &[Geometry::Polygon(large.clone())],
            &crs,
            &[-radius],
            0.01,
            false,
            limits(),
            None,
        )
        .unwrap();
        assert_eq!(
            total.surfaces[0].geometry,
            Geometry::Polygon(Polygon::empty())
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn erosion_hole_reaches_shell() {
    // C1: GOP-008, differenza dei corridoi; quattro residui dopo il
    // contatto buco/guscio e collasso totale, docs/buffer-proiezione.md.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let input = Geometry::Polygon(square_with_hole(0.0));
    let split = geodesic_line_buffer_with_limits(
        std::slice::from_ref(&input),
        &crs,
        &[-0.4],
        0.01,
        false,
        limits(),
        None,
    )
    .unwrap();
    assert_eq!(polygons(&split.surfaces[0].geometry).len(), 4);
    let total =
        geodesic_line_buffer_with_limits(&[input], &crs, &[-0.6], 0.01, false, limits(), None)
            .unwrap();
    assert_eq!(
        total.surfaces[0].geometry,
        Geometry::Polygon(Polygon::empty())
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn zero_union_touching_surfaces() {
    // C1: GOP-008; unione booleana di parti con lato o vertice comune.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let a = Polygon::new(
        LineString::from(vec![
            (9.0, 0.0),
            (9.00001, 0.0),
            (9.00001, 0.00001),
            (9.0, 0.00001),
            (9.0, 0.0),
        ]),
        vec![],
    );
    for offset in [0.0, 0.00001] {
        let b = Polygon::new(
            LineString::from(vec![
                (9.00001, offset),
                (9.00002, offset),
                (9.00002, offset + 0.00001),
                (9.00001, offset + 0.00001),
                (9.00001, offset),
            ]),
            vec![],
        );
        let inputs = [Geometry::Polygon(a.clone()), Geometry::Polygon(b.clone())];
        let expected = polygon_boolean::boolean_with_limits(
            &inputs[0],
            &inputs[1],
            PolygonBooleanOp::Union,
            Some(2),
            limits().overlay,
            None,
        )
        .unwrap();
        for (inputs, union) in [
            (
                vec![Geometry::MultiPolygon(MultiPolygon(vec![a.clone(), b]))],
                false,
            ),
            (inputs.to_vec(), true),
        ] {
            let result = geodesic_line_buffer_with_limits(
                &inputs,
                &crs,
                &[0.0],
                0.01,
                union,
                limits(),
                None,
            )
            .unwrap();
            assert_eq!(polygons(&result.surfaces[0].geometry), polygons(&expected));
            assert_eq!(result.surfaces[0].certificate.absolute_m, 0.0);
            assert!(result.surfaces[0].preserves_geodesic_operand);
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
#[allow(clippy::too_many_lines)]
fn erosion_difference_thresholds_and_interruptions() {
    // C1: GOP-027/028; soglie e iniezioni nel Work della differenza e della
    // copertura continua, docs/buffer-proiezione.md. Rettangoli analitici.
    // La fixture di 20 cm mantiene le fasi e le soglie dinamiche, senza
    // moltiplicare i campioni metrici esatti lungo un bordo di due metri.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let ring = |low, high| {
        LineString::from(vec![
            (low, low),
            (high, low),
            (high, high),
            (low, high),
            (low, low),
        ])
    };
    let operand = Geometry::Polygon(Polygon::new(ring(0.0, 0.2), vec![]));
    let corridors = Geometry::Polygon(Polygon::new(ring(-0.01, 0.21), vec![ring(0.01, 0.19)]));
    let mut certificate = empty_certificate(&crs);
    certificate.numerical_m = 0.0002;
    certificate.stability_m = 0.005;
    let mut work = Work::new(limits().overlay.work, None);
    polygon_boolean::execute(
        &operand,
        &corridors,
        PolygonBooleanOp::Difference,
        Some(2),
        limits().overlay,
        &mut work,
    )
    .unwrap();
    let overlay_steps = work.steps;
    let mut work = Work::new(limits().overlay.work, None);
    difference(
        &operand,
        &corridors,
        0.01,
        &mut { certificate },
        limits(),
        &mut work,
    )
    .unwrap();
    let total = work.steps;
    for budget in [total - 1, total] {
        let mut bound = limits();
        bound.overlay.work.max_steps = budget;
        let mut work = Work::new(bound.overlay.work, None);
        let outcome = difference(
            &operand,
            &corridors,
            0.01,
            &mut { certificate },
            bound,
            &mut work,
        );
        if budget == total {
            outcome.unwrap();
        } else {
            assert!(matches!(
                outcome,
                Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                    PredicateError::WorkLimit
                )))
            ));
        }
    }
    for at in [
        0,
        overlay_steps / 2,
        overlay_steps - 1,
        overlay_steps + 1,
        total / 2,
        total - 1,
    ] {
        for deadline in [false, true] {
            let flag = AtomicBool::new(false);
            let mut work = Work::new(limits().overlay.work, Some(&flag));
            work.interrupt_at = Some((at, deadline));
            let outcome = difference(
                &operand,
                &corridors,
                0.01,
                &mut { certificate },
                limits(),
                &mut work,
            );
            if deadline {
                assert!(matches!(
                    outcome,
                    Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                        PredicateError::DeadlineExceeded
                    )))
                ));
            } else {
                assert!(matches!(
                    outcome,
                    Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                        PredicateError::Cancelled
                    )))
                ));
            }
        }
    }
    for resource in 0..4 {
        let mut bound = limits();
        match resource {
            0 => bound.overlay.work.max_reserved_bytes = 0,
            1 => bound.overlay.work.max_nodes = 0,
            2 => bound.overlay.max_edges = 0,
            _ => bound.overlay.max_rings = 0,
        }
        let mut work = Work::new(bound.overlay.work, None);
        let outcome = difference(
            &operand,
            &corridors,
            0.01,
            &mut { certificate },
            bound,
            &mut work,
        );
        if resource == 0 {
            assert!(matches!(
                outcome,
                Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                    PredicateError::ReservationLimit
                )))
            ));
        } else {
            assert!(matches!(
                outcome,
                Err(UnionBufferError::Overlay(PolygonBooleanError::Predicate(
                    PredicateError::WorkLimit
                )))
            ));
        }
    }
    let mut work = Work::new(limits().overlay.work, None);
    assert!(matches!(
        erosion::certify(
            &operand,
            &Geometry::Polygon(Polygon::empty()),
            0.01,
            &certificate,
            &mut work
        ),
        Err(UnionBufferError::Disk(BufferFailure::AccuracyUnproven))
    ));
    let mut work = Work::new(limits().overlay.work, None);
    assert!(matches!(
        erosion::certify(&operand, &operand, 0.0999, &certificate, &mut work),
        Err(UnionBufferError::Disk(BufferFailure::AccuracyUnproven))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn erosion_union_is_union_of_individual_erosions() {
    // C1: GOP-008, erosion e unione non commutano: due quadrati sovrapposti,
    // oracolo compositivo delle differenze certificate di ciascun ingresso.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let a = Polygon::new(square_with_hole(0.0).exterior().clone(), vec![]);
    let b = a.map_coords(|p| Coord {
        x: p.x + 0.00001,
        y: p.y,
    });
    let inputs = [Geometry::Polygon(a), Geometry::Polygon(b)];
    let united =
        geodesic_line_buffer_with_limits(&inputs, &crs, &[-0.1], 0.01, true, limits(), None)
            .unwrap();
    // Oracolo compositivo: differenze individuali nella stessa proiezione,
    // poi booleana binaria generica. Nessuna booleana sui lati densificati.
    let mut work = Work::new(limits().overlay.work, None);
    let local = context(&inputs, &crs, &mut work).unwrap();
    let mut vertices = 0;
    let individual: Vec<_> = inputs
        .iter()
        .map(|input| {
            build_local(
                std::slice::from_ref(input),
                &crs,
                -0.1,
                0.01,
                limits(),
                &mut vertices,
                &mut work,
                &local,
                false,
            )
            .unwrap()
            .geometry
        })
        .collect();
    let planar = polygon_boolean::execute(
        &individual[0],
        &individual[1],
        PolygonBooleanOp::Union,
        Some(2),
        limits().overlay,
        &mut work,
    )
    .unwrap();
    let expected = publish(
        planar,
        &local,
        united.surfaces[0].certificate.output_model_m,
        limits(),
        &mut vertices,
        &mut work,
    )
    .unwrap();
    assert_eq!(united.surfaces[0].geometry, expected);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn zero_union_meridian_t_contact_and_multiple_point_contacts() {
    // C1: GOP-008; il meridiano e' una geodetica, quindi i nodi del
    // contatto T conservano esattamente i sottoarchi originali.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let rectangle = |west, east, south, north| {
        Polygon::new(
            LineString::from(vec![
                (west, south),
                (east, south),
                (east, north),
                (west, north),
                (west, south),
            ]),
            vec![],
        )
    };
    let a = rectangle(9.0, 9.00001, 0.0, 0.00002);
    let b = rectangle(9.00001, 9.00002, 0.000_005, 0.000_015);
    let expected = polygon_boolean::boolean_with_limits(
        &Geometry::Polygon(a.clone()),
        &Geometry::Polygon(b.clone()),
        PolygonBooleanOp::Union,
        Some(2),
        limits().overlay,
        None,
    )
    .unwrap();
    let result = geodesic_line_buffer_with_limits(
        &[Geometry::MultiPolygon(MultiPolygon(vec![a, b]))],
        &crs,
        &[0.0],
        0.01,
        false,
        limits(),
        None,
    )
    .unwrap();
    assert_eq!(polygons(&result.surfaces[0].geometry), polygons(&expected));
    let parts = vec![
        rectangle(9.0, 9.00001, 0.0, 0.00001),
        rectangle(9.00001, 9.00002, 0.00001, 0.00002),
        rectangle(9.00002, 9.00003, 0.00002, 0.00003),
    ];
    let result = geodesic_line_buffer_with_limits(
        &[Geometry::MultiPolygon(MultiPolygon(parts))],
        &crs,
        &[0.0],
        0.01,
        false,
        limits(),
        None,
    )
    .unwrap();
    assert_eq!(polygons(&result.surfaces[0].geometry).len(), 3);
    assert_eq!(result.surfaces[0].certificate.absolute_m, 0.0);
}

/// B1/B2: oracolo insiemistico esatto, rettangoli sovrapposti e lati lunghi.
#[test]
#[deny(clippy::disallowed_methods)]
fn many_corridors_exact_topology_and_publication_after_overlay() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let mut work = Work::new(limits().overlay.work, None);
    let mut operands = CountedVec::new();
    for i in 0..12 {
        let x = f64::from(i);
        let polygon = Polygon::new(
            LineString::from(vec![
                (x, 0.0),
                (x + 2.0, 0.0),
                (x + 2.0, 1.0),
                (x, 1.0),
                (x, 0.0),
            ]),
            vec![],
        );
        work.push(&mut operands, Geometry::Polygon(polygon))
            .unwrap();
    }
    let result = union_many(&operands, limits(), &mut work).unwrap();
    let expected = Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (13.0, 0.0),
            (13.0, 1.0),
            (0.0, 1.0),
            (0.0, 0.0),
        ]),
        vec![],
    ));
    super::super::verify_coverage(&result, &expected, &mut work).unwrap();
    super::super::verify_coverage(&expected, &result, &mut work).unwrap();
    let local = context(&[segment(0.0)], &crs, &mut work).unwrap();
    let mut vertices = 0;
    let published = publish(result, &local, 1e-6, limits(), &mut vertices, &mut work).unwrap();
    super::super::verify_surface(&published, &mut work).unwrap();
    assert!(vertices > 5);
    // GeographicLib G1: gli estremi inversi restano nel certificato metrico.
    for point in published.coords_iter() {
        let projected = local.forward(point, &mut work).unwrap();
        assert!((-0.0002..=13.0002).contains(&projected.x));
        assert!((-0.0002..=1.0002).contains(&projected.y));
    }
}

/// B3: oracolo topologico esatto, nessuna riparazione dei risultati invalidi.
#[test]
#[deny(clippy::disallowed_methods)]
fn published_topology_rejects_crossings_orientation_and_lost_input() {
    let mut work = Work::new(limits().overlay.work, None);
    for points in [
        vec![(0., 0.), (2., 2.), (0., 2.), (2., 0.), (0., 0.)],
        vec![(0., 0.), (0., 2.), (2., 2.), (2., 0.), (0., 0.)],
    ] {
        let invalid = Geometry::Polygon(Polygon::new(LineString::from(points), vec![]));
        assert!(matches!(
            super::super::verify_surface(&invalid, &mut work),
            Err(UnionBufferError::Disk(BufferFailure::InvalidTopology))
        ));
    }
    let square = Geometry::Polygon(Polygon::new(
        LineString::from(vec![(0., 0.), (2., 0.), (2., 2.), (0., 2.), (0., 0.)]),
        vec![],
    ));
    let outside = Geometry::Point(geo::Point::new(3., 1.));
    assert!(matches!(
        super::super::verify_coverage(&square, &outside, &mut work),
        Err(UnionBufferError::Disk(BufferFailure::InvalidTopology))
    ));
    erosion::clearance(
        &LineString::from(vec![(0.5, 0.5), (1.5, 1.5)]),
        &square,
        0.1,
        &mut work,
    )
    .unwrap();
    assert!(matches!(
        erosion::clearance(
            &LineString::from(vec![(0.5, 0.5), (3., 1.)]),
            &square,
            0.1,
            &mut work
        ),
        Err(UnionBufferError::Disk(BufferFailure::InvalidTopology))
    ));
}

/// GOP-005/008: normalizzazione del verso senza spostare siti a raggio zero.
#[test]
#[deny(clippy::disallowed_methods)]
fn zero_orients_rings_without_moving_vertices() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let expected = square_with_hole(0.0);
    let (mut shell, mut holes) = expected.clone().into_inner();
    shell.0.reverse();
    holes[0].0.reverse();
    let input = Geometry::Polygon(Polygon::new(shell, holes));
    for union in [false, true] {
        let result = geodesic_line_buffer_with_limits(
            &[input.clone(), input.clone()],
            &crs,
            &[0.0],
            0.01,
            union,
            limits(),
            None,
        )
        .unwrap();
        assert_eq!(result.surfaces.len(), if union { 1 } else { 2 });
        for surface in result.surfaces {
            assert_eq!(surface.geometry, Geometry::Polygon(expected.clone()));
            assert_eq!(surface.certificate.absolute_m, 0.0);
        }
    }
}

/// IEEE-754 nearest-even sul reticolo; `GeographicLib` G1 per lo spostamento a terra.
#[test]
#[deny(clippy::disallowed_methods)]
fn primitive_lattice_ties_and_geodesic_displacement() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let mut work = Work::new(limits().overlay.work, None);
    let local = context(&[segment(0.0)], &crs, &mut work).unwrap();
    let grid = 1.0 / 1_048_576.0;
    let source = Polygon::new(
        LineString::from(vec![
            (-1.5 * grid, -0.5 * grid),
            (2.5 * grid, 0.5 * grid),
            (10.5 * grid, 10.5 * grid),
            (-1.5 * grid, -0.5 * grid),
        ]),
        vec![],
    );
    let rounded = lattice(source.clone(), &mut work).unwrap();
    let expected = [(-2.0, 0.0), (2.0, 0.0), (10.0, 10.0), (-2.0, 0.0)];
    for ((before, after), (x, y)) in source
        .exterior()
        .0
        .iter()
        .zip(&rounded.exterior().0)
        .zip(expected)
    {
        assert_eq!(
            *after,
            Coord {
                x: x * grid,
                y: y * grid
            }
        );
        assert!((before.x - after.x).abs() <= grid / 2.0);
        assert!((before.y - after.y).abs() <= grid / 2.0);
        let a = local.inverse(*before, &mut work).unwrap();
        let b = local.inverse(*after, &mut work).unwrap();
        let distance: f64 = local.ellipsoid.geodetica().inverse(a.y, a.x, b.y, b.x);
        assert!(distance < 0.000_001);
    }
    let outside = source.map_coords(|p| Coord {
        x: p.x + 8e6,
        y: p.y,
    });
    assert!(matches!(
        lattice(outside, &mut work),
        Err(UnionBufferError::Disk(BufferFailure::AccuracyUnproven))
    ));
}
