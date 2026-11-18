//! C1: G1 Karney 2013, metrica ellissoidica; G2 `GeographicLib` 2.1.

#![allow(
    clippy::suboptimal_flops,
    reason = "Ordine esplicito delle operazioni nelle maggiorazioni numeriche"
)]
use super::*;
use geo::{MultiLineString, MultiPoint, MultiPolygon, Point};
use plenora_core::crs::resolve_crs;

fn limits() -> GeodesicDistanceLimits {
    GeodesicDistanceLimits {
        max_work: 10_000_000,
        max_reserved_bytes: 128_000_000,
        deadline: None,
    }
}

fn point(x: f64, y: f64) -> Geometry {
    Point::new(x, y).into()
}
fn line(coords: &[(f64, f64)]) -> Geometry {
    LineString::from(coords.to_vec()).into()
}
fn distance(a: &Geometry, b: &Geometry) -> GeodesicDistanceResult {
    geodesic_distance_with_limits(
        a,
        b,
        &resolve_crs("EPSG:4326", "crs").unwrap(),
        limits(),
        None,
    )
    .unwrap()
}
fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= ABSOLUTE_ERROR_M,
        "distance outside certificate"
    );
}

// G1 equations 1-3: quadratura di Simpson della metrica meridiana,
// 16384 intervalli, errore inferiore a 1 micrometro per questi archi.
fn meridian(latitude: f64) -> f64 {
    let f = 1.0 / 298.257_223_563;
    let e2 = f * (2.0 - f);
    let radius = |phi: f64| 6_378_137.0 * (1.0 - e2) / (1.0 - e2 * phi.sin().powi(2)).powf(1.5);
    let n = 16_384_u32;
    let h = latitude.to_radians() / f64::from(n);
    let mut sum = radius(0.0) + radius(latitude.to_radians());
    for i in 1..n {
        sum += (if i % 2 == 0 { 2.0 } else { 4.0 }) * radius(f64::from(i) * h);
    }
    sum * h / 3.0
}

#[test]
fn internal_equator_and_meridian_minima() {
    // G1 metrica: la normale all'equatore e' un meridiano.
    close(
        distance(&point(0.0, 1.0), &line(&[(-2.0, 0.0), (2.0, 0.0)])).metres,
        meridian(1.0),
    );
    // G1: normale al meridiano all'equatore, arco equatoriale a*lambda.
    close(
        distance(&point(1.0, 0.0), &line(&[(0.0, -2.0), (0.0, 2.0)])).metres,
        6_378_137.0 * 1.0_f64.to_radians(),
    );
}

#[test]
fn crossing_lines_and_endpoint_minimum() {
    // G1: equatore e meridiano si intersecano in (0,0), interno ai lati.
    assert_eq!(
        distance(
            &line(&[(-1.0, 0.0), (2.0, 0.0)]),
            &line(&[(0.0, -2.0), (0.0, 1.0)])
        )
        .metres,
        0.0
    );
    // G1: intervalli disgiunti dello stesso equatore, gap di un grado.
    close(
        distance(
            &line(&[(0.0, 0.0), (1.0, 0.0)]),
            &line(&[(2.0, 0.0), (3.0, 0.0)]),
        )
        .metres,
        6_378_137.0 * 1.0_f64.to_radians(),
    );
}

#[test]
fn dateline_and_symmetry() {
    let a = point(180.0, 1.0);
    let b = line(&[(179.0, 0.0), (-179.0, 0.0)]);
    // GOP-004 e G1: seam continuo, minimo sul meridiano 180.
    close(distance(&a, &b).metres, meridian(1.0));
    close(distance(&a, &b).metres, distance(&b, &a).metres);
}

fn rectangle(low: f64, high: f64) -> LineString {
    LineString::from(vec![
        (low, low),
        (high, low),
        (high, high),
        (low, high),
        (low, low),
    ])
}

#[test]
fn polygon_interior_hole_and_contained_polygon() {
    let solid: Geometry = Polygon::new(rectangle(-2.0, 2.0), vec![]).into();
    let holed: Geometry = Polygon::new(rectangle(-2.0, 2.0), vec![rectangle(-0.5, 0.5)]).into();
    // GOP-005: l'origine e' interna alla shell e interna al buco esplicito.
    assert_eq!(distance(&point(0.0, 0.0), &solid).metres, 0.0);
    // G2 Python 2.1: punto medio del lato (-.5,-.5)-(.5,-.5),
    // normale meridiana per simmetria, inverse dall'origine.
    close(
        distance(&point(0.0, 0.0), &holed).metres,
        55_289.271_330_091_884,
    );
    let small: Geometry = Polygon::new(rectangle(-0.1, 0.1), vec![]).into();
    // GOP-005: insieme pieno, non distanza fra i soli bordi.
    assert_eq!(distance(&small, &solid).metres, 0.0);
    assert!(distance(&small, &holed).metres > 40_000.0);
}

#[test]
fn polar_cap_and_antimeridian_surface() {
    let cap: Geometry = Polygon::new(
        LineString::from(vec![
            (0.0, 85.0),
            (120.0, 85.0),
            (-120.0, 85.0),
            (0.0, 85.0),
        ]),
        vec![],
    )
    .into();
    // GOP-005: il polo interno alla calotta piccola e' ammesso.
    assert_eq!(distance(&point(0.0, 90.0), &cap).metres, 0.0);
    let seam: Geometry = Polygon::new(
        LineString::from(vec![
            (179.0, -1.0),
            (-179.0, -1.0),
            (-179.0, 1.0),
            (179.0, 1.0),
            (179.0, -1.0),
        ]),
        vec![],
    )
    .into();
    // GOP-004/005: il seam non apre l'anello.
    assert_eq!(distance(&point(180.0, 0.0), &seam).metres, 0.0);
}

#[test]
fn geographiclib_reference_and_antipodes() {
    // G2 Python 2.1 documentation, Wellington-Salamanca inverse example.
    close(
        distance(&point(174.81, -41.32), &point(-5.50, 40.96)).metres,
        19_959_679.267_353_82,
    );
    // G1: punti antipodali, distanza = due quarti di meridiano WGS84.
    close(
        distance(&point(0.0, 0.0), &point(180.0, 0.0)).metres,
        2.0 * meridian(90.0),
    );
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    // G2 Multiple shortest geodesics: percorso equatoriale antipodale ambiguo.
    assert!(matches!(
        geodesic_distance_with_limits(
            &point(0.0, 10.0),
            &line(&[(0.0, 0.0), (180.0, 0.0)]),
            &crs,
            limits(),
            None
        ),
        Err(GeodesicDistanceError::AmbiguousGeodesic)
    ));
    // GOP-004: il percorso quasi antipodale unico resta ammesso.
    assert_eq!(
        distance(&point(0.0, 0.0), &line(&[(0.0, 0.0), (179.5, 0.1)])).metres,
        0.0
    );
}

#[test]
fn exact_work_memory_cancel_deadline_and_crs() {
    let a = point(1.0, 2.0);
    let b = point(3.0, 4.0);
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let measured = distance(&a, &b);
    let mut budget = limits();
    budget.max_work = measured.work;
    budget.max_reserved_bytes = measured.peak_reserved_bytes;
    // GOP-026: il limite esatto e' ammesso, l'addebito successivo fallisce.
    assert_eq!(
        geodesic_distance_with_limits(&a, &b, &crs, budget, None)
            .unwrap()
            .metres,
        measured.metres
    );
    budget.max_work -= 1;
    assert!(matches!(
        geodesic_distance_with_limits(&a, &b, &crs, budget, None),
        Err(GeodesicDistanceError::WorkLimit)
    ));
    budget = limits();
    budget.max_reserved_bytes = measured.peak_reserved_bytes - 1;
    assert!(matches!(
        geodesic_distance_with_limits(&a, &b, &crs, budget, None),
        Err(GeodesicDistanceError::MemoryLimit)
    ));
    let cancelled = AtomicBool::new(true);
    // GOP-028: deadline prevale al checkpoint condiviso.
    assert!(matches!(
        geodesic_distance_with_limits(&a, &b, &crs, limits(), Some(&cancelled)),
        Err(GeodesicDistanceError::Cancelled)
    ));
    budget = limits();
    budget.deadline = Some(Instant::now());
    assert!(matches!(
        geodesic_distance_with_limits(&a, &b, &crs, budget, Some(&cancelled)),
        Err(GeodesicDistanceError::DeadlineExceeded)
    ));
    assert!(matches!(
        geodesic_distance_with_limits(
            &a,
            &b,
            &resolve_crs("EPSG:3857", "crs").unwrap(),
            limits(),
            None
        ),
        Err(GeodesicDistanceError::GeographicRequired)
    ));
}

#[test]
fn multipart_and_invalid_surfaces() {
    // GOP-002/005: unione delle parti, senza deduplicare righe o vertici.
    let points: Geometry =
        MultiPoint::new(vec![Point::new(0.0, 0.0), Point::new(10.0, 10.0)]).into();
    let lines: Geometry =
        MultiLineString::new(vec![LineString::from(vec![(-1.0, 0.0), (1.0, 0.0)])]).into();
    assert_eq!(distance(&points, &lines).metres, 0.0);
    let polygons: Geometry =
        MultiPolygon::new(vec![Polygon::new(rectangle(-1.0, 1.0), vec![])]).into();
    assert_eq!(distance(&points, &polygons).metres, 0.0);
    let crossed: Geometry = Polygon::new(
        LineString::from(vec![
            (-1.0, -1.0),
            (1.0, 1.0),
            (-1.0, 1.0),
            (1.0, -1.0),
            (-1.0, -1.0),
        ]),
        vec![],
    )
    .into();
    // GOP-005: anello non semplice, mai riparato implicitamente.
    assert!(matches!(
        geodesic_distance_with_limits(
            &point(3.0, 3.0),
            &crossed,
            &resolve_crs("EPSG:4326", "crs").unwrap(),
            limits(),
            None
        ),
        Err(GeodesicDistanceError::UnsupportedSurface)
    ));
}

#[test]
fn index_matches_generic_pairs() {
    // Oracolo differenziale: stessa ricerca certificata, tutte le coppie
    // di segmenti separate, senza gerarchia o pruning fra parti.
    let a = line(&[(1.0, 2.0), (1.2, 2.1), (1.4, 2.0)]);
    let b = line(&[(2.0, 3.0), (2.2, 3.1), (2.4, 3.0)]);
    let Geometry::LineString(ref la) = a else {
        unreachable!()
    };
    let Geometry::LineString(ref lb) = b else {
        unreachable!()
    };
    let mut generic = f64::INFINITY;
    for pair_a in la.0.windows(2) {
        for pair_b in lb.0.windows(2) {
            let one: Geometry = LineString::new(pair_a.to_vec()).into();
            let two: Geometry = LineString::new(pair_b.to_vec()).into();
            generic = generic.min(distance(&one, &two).metres);
        }
    }
    close(distance(&a, &b).metres, generic);
}

#[test]
fn independent_geographiclib_global_brackets() {
    // G2 Python 2.1, reference.py: heap di intervalli metrici indipendente,
    // bracket certificati di ampiezza 0.5 mm, senza chiamate al crate Rust.
    for (p, a, b, lower, upper) in [
        (
            (35.0, 14.1),
            (34.5, 12.3),
            (37.2, 13.7),
            150_096.521_265_349_5,
            150_096.521_765_323_5,
        ),
        (
            (170.0, 72.0),
            (165.0, 70.0),
            (-175.0, 73.0),
            91_841.342_551_391_78,
            91_841.343_051_371_47,
        ),
    ] {
        let result = distance(&point(p.0, p.1), &line(&[a, b])).metres;
        assert!(result >= lower - ABSOLUTE_ERROR_M && result <= upper + ABSOLUTE_ERROR_M);
    }
}

#[test]
fn taylor_matches_metric_oracle() {
    // Oracolo differenziale senza bound di Taylor: solo disuguaglianza
    // triangolare e raffinamento globale, sulle identiche coppie di archi.
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    for latitude in [0.0, 45.0, 85.0] {
        for skew in [0.0, 0.0001] {
            let a = line(&[(1.0, latitude), (1.0002, latitude + skew)]);
            let b = line(&[
                (1.00005, latitude + 0.0001),
                (1.00015, latitude + 0.0001 + skew),
            ]);
            let mut work = Work {
                limits: limits(),
                cancel: None,
                count: 0,
                bytes: 0,
                peak: 0,
                ellipsoid: EllissoideGeodetico::da_crs(&crs).unwrap(),
                taylor: false,
            };
            let left = GeometryIndex::new(&a, &mut work).unwrap();
            let right = GeometryIndex::new(&b, &mut work).unwrap();
            let expected = minimum(
                left.nodes[left.root],
                right.nodes[right.root],
                &left.nodes,
                &right.nodes,
                false,
                &mut work,
            )
            .unwrap();
            close(distance(&a, &b).metres, expected);
            close(distance(&b, &a).metres, expected);
        }
    }
}

#[test]
fn interruption_during_indexing_and_no_product_admission() {
    use std::time::Duration;
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let many: Geometry = MultiPoint::new(vec![Point::new(1.0, 2.0); 200_000]).into();
    let cancelled = AtomicBool::new(false);
    // GOP-028: interruzione durante una fase lunga, non solo all'ingresso.
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(2));
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert!(matches!(
            geodesic_distance_with_limits(
                &many,
                &point(1.0, 2.0),
                &crs,
                limits(),
                Some(&cancelled)
            ),
            Err(GeodesicDistanceError::Cancelled)
        ));
    });
    let mut budget = limits();
    budget.deadline = Some(Instant::now() + Duration::from_millis(2));
    assert!(matches!(
        geodesic_distance_with_limits(&many, &point(1.0, 2.0), &crs, budget, None),
        Err(GeodesicDistanceError::DeadlineExceeded)
    ));
    let parts: Geometry = MultiPoint::new(vec![Point::new(1.0, 2.0); 2_000]).into();
    budget = limits();
    budget.max_work = 20_000;
    // GOP-026: quattro milioni di coppie teoriche, meno di 20000 addebiti.
    assert_eq!(
        geodesic_distance_with_limits(&parts, &parts, &crs, budget, None)
            .unwrap()
            .metres,
        0.0
    );
    budget.max_work = 0;
    assert!(matches!(
        geodesic_distance_with_limits(&point(1.0, 2.0), &point(1.0, 2.0), &crs, budget, None),
        Err(GeodesicDistanceError::WorkLimit)
    ));
}

#[test]
fn surface_orientation_and_complement() {
    let original = rectangle(-1.0, 1.0);
    let mut reversed = original.clone();
    reversed.0.reverse();
    let a: Geometry = Polygon::new(original, vec![]).into();
    let b: Geometry = Polygon::new(reversed, vec![]).into();
    // GOP-005: il verso di serializzazione non sceglie il complemento.
    for p in [point(0.0, 0.0), point(3.0, 0.0), point(180.0, 0.0)] {
        close(distance(&p, &a).metres, distance(&p, &b).metres);
    }
    assert!(distance(&point(180.0, 0.0), &a).metres > 19_000_000.0);
}

#[test]
fn refinement_limits_and_interruptions() {
    use std::time::Duration;
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let a = point(0.0, 1.0);
    let b = line(&[(-2.0, 0.0), (2.0, 0.0)]);
    let measured = distance(&a, &b);
    let mut budget = limits();
    budget.max_work = measured.work;
    // GOP-026: include direct, inverse e raffinamenti, non solo i vertici.
    close(
        geodesic_distance_with_limits(&a, &b, &crs, budget, None)
            .unwrap()
            .metres,
        meridian(1.0),
    );
    budget.max_work -= 1;
    assert!(matches!(
        geodesic_distance_with_limits(&a, &b, &crs, budget, None),
        Err(GeodesicDistanceError::WorkLimit)
    ));
    let far = point(0.0, 30.0);
    let long = line(&[(-80.0, 0.0), (80.0, 0.0)]);
    let cancelled = AtomicBool::new(false);
    // GOP-028: indice di due foglie, ricerca metrica lunga oltre il dominio
    // di Taylor; l'interruzione avviene nei raffinamenti.
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(2));
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert!(matches!(
            geodesic_distance_with_limits(&far, &long, &crs, limits(), Some(&cancelled)),
            Err(GeodesicDistanceError::Cancelled)
        ));
    });
    budget = limits();
    budget.deadline = Some(Instant::now() + Duration::from_millis(2));
    assert!(matches!(
        geodesic_distance_with_limits(&far, &long, &crs, budget, None),
        Err(GeodesicDistanceError::DeadlineExceeded)
    ));
}
