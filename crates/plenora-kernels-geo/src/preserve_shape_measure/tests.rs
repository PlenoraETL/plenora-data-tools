//! C1: G1 Karney 2013, metrica ellissoidica; G2 `GeographicLib` Python 2.1.
//! Ogni riferimento e' indipendente dal campionamento del kernel.

#![allow(
    clippy::float_cmp,
    reason = "Identita di risultati vuoti e determinismo"
)]
#![allow(
    clippy::suboptimal_flops,
    reason = "Formule degli oracoli indipendenti"
)]

use super::*;
use plenora_core::crs::resolve_crs;
use std::sync::atomic::Ordering;

fn limits() -> ShapeMeasureLimits {
    ShapeMeasureLimits {
        max_work: 8_000_000,
        deadline: None,
    }
}

const fn tolerance(absolute: f64) -> ShapeTolerance {
    ShapeTolerance {
        absolute,
        relative: 0.0,
    }
}

fn length(parts: &[LineString<f64>], code: &str, absolute: f64) -> ShapeMeasure {
    preserve_shape_length_with_limits(
        parts,
        &resolve_crs(code, "crs").unwrap(),
        tolerance(absolute),
        limits(),
        None,
    )
    .unwrap()
}

fn area(polygons: &[Polygon<f64>], absolute: f64) -> ShapeMeasure {
    preserve_shape_area_with_limits(
        polygons,
        &resolve_crs("EPSG:4326", "crs").unwrap(),
        tolerance(absolute),
        limits(),
        None,
    )
    .unwrap()
}

fn rectangle(west: f64, south: f64, east: f64, north: f64) -> LineString<f64> {
    LineString::from(vec![
        (west, south),
        (east, south),
        (east, north),
        (west, north),
        (west, south),
    ])
}

// G1 equazioni 1-3: r(phi)=a cos(phi)/sqrt(1-e^2 sin^2(phi)).
fn parallel(a: f64, inverse_flattening: f64, latitude: f64, longitude_span: f64) -> f64 {
    let f = 1.0 / inverse_flattening;
    let phi = latitude.to_radians();
    a * phi.cos() / (1.0 - f * (2.0 - f) * phi.sin().powi(2)).sqrt() * longitude_span.to_radians()
}

#[test]
#[deny(clippy::disallowed_methods)]
fn parallel_is_not_the_shortest_geodesic_and_converges() {
    let line = LineString::from(vec![(0.0, 45.0), (20.0, 45.0)]);
    let expected = parallel(6_378_137.0, 298.257_223_563, 45.0, 20.0);
    let coarse = length(std::slice::from_ref(&line), "EPSG:4326", 1.0);
    let fine = length(std::slice::from_ref(&line), "EPSG:4326", 0.5);
    // G1 metrica del parallelo: attesa analitica, bound del modello in metri.
    assert!((coarse.value - expected).abs() <= coarse.absolute_error_bound);
    assert!((fine.value - expected).abs() <= fine.absolute_error_bound);
    assert!((fine.value - expected).abs() < (coarse.value - expected).abs());
    assert!(fine.work > coarse.work);
    let shortest: f64 = geographiclib_rs::Geodesic::wgs84().inverse(45.0, 0.0, 45.0, 20.0);
    assert!(fine.value - shortest > 3_000.0);
    assert!(fine.absolute_error_bound <= 0.5);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn projected_straight_line_against_independent_dense_reference() {
    let line = LineString::from(vec![(0.0, 4_000_000.0), (2_000_000.0, 6_000_000.0)]);
    let measured = length(&[line], "EPSG:3857", 0.2);
    // G2 Python 2.1: 65536 inverse su campioni uniformi EPSG 1024.
    // Driver versionato reference.py; il raddoppio indipendente differisce <1e-4 m.
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/reference.json"
    ))
    .unwrap();
    let expected = fixture["65536"]["web_length_m"].as_f64().unwrap();
    let previous = fixture["32768"]["web_length_m"].as_f64().unwrap();
    assert!((expected - previous).abs() < 1e-4);
    assert!((measured.value - expected).abs() <= measured.absolute_error_bound + 1e-4);
    assert!((measured.value - 2_000_000.0_f64.hypot(2_000_000.0)).abs() > 100_000.0);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn rectangles_holes_orientation_and_area_convergence() {
    let shell = rectangle(10.0, 40.0, 20.0, 50.0);
    let hole = rectangle(12.0, 42.0, 14.0, 44.0);
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/reference.json"
    ))
    .unwrap();
    // G2 Python 2.1, PolygonArea con 65536 sottotratti per lato, stesso datum.
    let expected = fixture["65536"]["rectangle_m2"].as_f64().unwrap();
    let expected_hole = fixture["65536"]["hole_m2"].as_f64().unwrap();
    let previous = fixture["32768"]["rectangle_m2"].as_f64().unwrap();
    assert!((expected - previous).abs() < 1.0);
    let polygon = Polygon::new(shell.clone(), vec![]);
    let coarse = area(std::slice::from_ref(&polygon), 100.0);
    let fine = area(&[polygon], 50.0);
    assert!((fine.value - expected).abs() <= fine.absolute_error_bound + 1.0);
    assert!((fine.value - expected).abs() < (coarse.value - expected).abs());
    let with_hole = area(&[Polygon::new(shell.clone(), vec![hole.clone()])], 100.0);
    assert!(
        (with_hole.value - (expected - expected_hole)).abs()
            <= with_hole.absolute_error_bound + 2.0
    );
    let mut reversed_shell = shell;
    let mut reversed_hole = hole;
    reversed_shell.0.reverse();
    reversed_hole.0.reverse();
    let reversed = area(&[Polygon::new(reversed_shell, vec![reversed_hole])], 100.0);
    // GOP-005: il ruolo esplicito dei buchi non dipende dall'orientazione.
    assert!(
        (reversed.value - with_hole.value).abs()
            <= reversed.absolute_error_bound + with_hole.absolute_error_bound
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn datum_seam_empty_and_multipart() {
    let crs = resolve_crs("EPSG:4267", "crs").unwrap();
    let parameters = crs.ellipsoid().unwrap();
    let parts = [LineString::from(vec![(179.0, 45.0), (-179.0, 45.0)])];
    let measured =
        preserve_shape_length_with_limits(&parts, &crs, tolerance(30.0), limits(), None).unwrap();
    // GOP-003/007, G1: la retta attraversa 358 gradi, non due; datum NAD27.
    let expected = parallel(
        parameters.semi_major_axis_metre,
        parameters.inverse_flattening,
        45.0,
        358.0,
    );
    assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
    assert_eq!(measured.ellipsoid, parameters);
    let empty = length(&[LineString::new(vec![])], "EPSG:4326", 1.0);
    assert_eq!(empty.value, 0.0);
    assert_eq!(area(&[], 1.0).value, 0.0);
    assert_eq!(
        area(&[Polygon::new(LineString::new(vec![]), vec![])], 1.0).value,
        0.0
    );
    let first = LineString::from(vec![(0.0, 0.0), (1.0, 0.0)]);
    let second = LineString::from(vec![(1.0, 0.0), (1.0, 0.0), (2.0, 0.0)]);
    let multi = length(&[first, second], "EPSG:4326", 1.0);
    // G1: equatore, L=a*delta_lambda; ripetizioni contribuiscono zero.
    assert!((multi.value - 6_378_137.0 * 2.0_f64.to_radians()).abs() < 1e-6);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_work_thresholds_and_interruption() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let parts = [LineString::from(vec![(0.0, 45.0), (1.0, 45.0)])];
    let baseline = length(&parts, "EPSG:4326", 1.0);
    let at_limit = ShapeMeasureLimits {
        max_work: baseline.work,
        deadline: None,
    };
    let accepted =
        preserve_shape_length_with_limits(&parts, &crs, tolerance(1.0), at_limit, None).unwrap();
    assert_eq!(accepted.value, baseline.value);
    let below = ShapeMeasureLimits {
        max_work: baseline.work - 1,
        deadline: None,
    };
    assert_eq!(
        preserve_shape_length_with_limits(&parts, &crs, tolerance(1.0), below, None),
        Err(ShapeMeasureError::WorkLimit)
    );
    let cancelled = AtomicBool::new(true);
    let expired = ShapeMeasureLimits {
        max_work: 0,
        deadline: Some(Instant::now()),
    };
    // GOP-028: precedenza deadline, poi cancellazione, poi risorse.
    assert_eq!(
        preserve_shape_length_with_limits(&parts, &crs, tolerance(1.0), expired, Some(&cancelled)),
        Err(ShapeMeasureError::DeadlineExceeded)
    );
    assert_eq!(
        preserve_shape_area_with_limits(&[], &crs, tolerance(1.0), expired, None),
        Err(ShapeMeasureError::DeadlineExceeded)
    );
    cancelled.store(false, Ordering::Relaxed);
    let mut work = MeasureWork::new(limits(), Some(&cancelled));
    work.charge().unwrap();
    cancelled.store(true, Ordering::Relaxed);
    assert_eq!(work.charge(), Err(ShapeMeasureError::Cancelled));
    cancelled.store(false, Ordering::Relaxed);
    work = MeasureWork::new(
        ShapeMeasureLimits {
            deadline: Some(Instant::now()),
            ..limits()
        },
        Some(&cancelled),
    );
    assert_eq!(work.charge(), Err(ShapeMeasureError::DeadlineExceeded));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn area_budget_includes_topology_and_all_refinements() {
    let polygons = [Polygon::new(rectangle(0.0, 0.0, 1.0, 1.0), vec![])];
    let measured = area(&polygons, 100.0);
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let below = ShapeMeasureLimits {
        max_work: measured.work - 1,
        deadline: None,
    };
    assert_eq!(
        preserve_shape_area_with_limits(&polygons, &crs, tolerance(100.0), below, None),
        Err(ShapeMeasureError::WorkLimit)
    );
    let at_limit = ShapeMeasureLimits {
        max_work: measured.work,
        deadline: None,
    };
    assert_eq!(
        preserve_shape_area_with_limits(&polygons, &crs, tolerance(100.0), at_limit, None)
            .unwrap()
            .value,
        measured.value
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn typed_domain_and_topology_errors_without_data() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let invalid = [LineString::from(vec![(0.0, 91.0)])];
    assert_eq!(
        preserve_shape_length_with_limits(&invalid, &crs, tolerance(1.0), limits(), None),
        Err(ShapeMeasureError::CrsDomain)
    );
    for absolute in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
        assert_eq!(
            preserve_shape_length_with_limits(&[], &crs, tolerance(absolute), limits(), None),
            Err(ShapeMeasureError::InvalidTolerance)
        );
    }
    let bad = [Polygon::new(
        rectangle(0.0, 0.0, 2.0, 2.0),
        vec![rectangle(3.0, 3.0, 4.0, 4.0)],
    )];
    assert_eq!(
        preserve_shape_area_with_limits(&bad, &crs, tolerance(1.0), limits(), None),
        Err(ShapeMeasureError::UnsupportedTopology)
    );
    let crossed = [Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (2.0, 2.0),
            (0.0, 2.0),
            (2.0, 0.0),
            (0.0, 0.0),
        ]),
        vec![],
    )];
    assert_eq!(
        preserve_shape_area_with_limits(&crossed, &crs, tolerance(1.0), limits(), None),
        Err(ShapeMeasureError::UnsupportedTopology)
    );
    let huge = [Polygon::new(rectangle(-100.0, 0.0, 100.0, 1.0), vec![])];
    assert_eq!(
        preserve_shape_area_with_limits(&huge, &crs, tolerance(1.0), limits(), None),
        Err(ShapeMeasureError::UnsupportedSurface)
    );
    assert_eq!(
        ShapeMeasureError::CrsDomain.to_string(),
        "dominio CRS della misura non supportato"
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn relative_bound_is_against_true_value() {
    let parts = [LineString::from(vec![(0.0, 45.0), (20.0, 45.0)])];
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let requested = ShapeTolerance {
        absolute: 0.0,
        relative: 1e-6,
    };
    let measured =
        preserve_shape_length_with_limits(&parts, &crs, requested, limits(), None).unwrap();
    let expected = parallel(6_378_137.0, 298.257_223_563, 45.0, 20.0);
    // GOP-006: relativo rispetto al valore esatto, non al valore approssimato.
    assert!((measured.value - expected).abs() <= requested.relative * expected);
    assert!(measured.absolute_error_bound <= requested.relative * expected);
    assert!(!accepted(f64::MAX, f64::INFINITY, requested));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn numerical_underflow_is_not_a_zero_certificate() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let polygons = [Polygon::new(rectangle(0.0, 0.0, 1e-322, 1.0), vec![])];
    let requested = ShapeTolerance {
        absolute: 0.0,
        relative: 0.1,
    };
    // GOP-006: area matematica positiva, underflow non autorizza un certificato nullo.
    assert_eq!(
        preserve_shape_area_with_limits(&polygons, &crs, requested, limits(), None),
        Err(ShapeMeasureError::AccuracyUnproven)
    );
    // G1: tutti i valori di longitudine allo stesso polo sono lo stesso punto.
    let pole = length(
        &[LineString::from(vec![(0.0, 90.0), (10.0, 90.0)])],
        "EPSG:4326",
        1e-9,
    );
    assert_eq!(pole.value, 0.0);
    assert_eq!(pole.absolute_error_bound, 0.0);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn geographic_and_projected_area_against_analytic_surface_integral() {
    // G1 metrica: dA=a^2(1-e^2) cos(phi)/(1-e^2 sin^2(phi))^2 dphi dlambda.
    // Primitiva chiusa indipendente dalla serie e dalla quadratura del kernel.
    let flattening = 1.0 / 298.257_223_563;
    let eccentricity = (flattening * (2.0_f64 - flattening)).sqrt();
    let primitive = |latitude: f64| {
        let sine = latitude.to_radians().sin();
        6_378_137.0_f64.powi(2) * (1.0 - eccentricity.powi(2)) / 2.0
            * (sine / (1.0 - eccentricity.powi(2) * sine * sine)
                + (eccentricity * sine).atanh() / eccentricity)
    };
    let expected = 10.0_f64.to_radians() * (primitive(50.0) - primitive(40.0));
    let ring = rectangle(10.0, 40.0, 20.0, 50.0);
    let measured = area(&[Polygon::new(ring.clone(), vec![])], 100.0);
    assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
    // EPSG 1024: rettangolo Web Mercator, inversa con identica curva lon/lat.
    let projected = LineString::new(
        ring.0
            .iter()
            .map(|point| Coord {
                x: 6_378_137.0 * point.x.to_radians(),
                y: 6_378_137.0
                    * (std::f64::consts::FRAC_PI_4 + point.y.to_radians() / 2.0)
                        .tan()
                        .ln(),
            })
            .collect(),
    );
    let crs = resolve_crs("EPSG:3857", "crs").unwrap();
    let measured = preserve_shape_area_with_limits(
        &[Polygon::new(projected, vec![])],
        &crs,
        tolerance(100.0),
        limits(),
        None,
    )
    .unwrap();
    assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
}

// C1: Snyder 1987 eq. 7-7/7-9, bisezione Python e GeographicLib 2.1;
// fixture prodotta da tests/fixtures/preserve_shape/mercator.py, senza Rust.
#[test]
#[deny(clippy::disallowed_methods)]
fn mercator_a_line_and_polygon_against_independent_reference() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/mercator.json"
    ))
    .unwrap();
    let line = LineString::from(vec![(0.0, 4_000_000.0), (2_000_000.0, 6_000_000.0)]);
    let measured = length(&[line], "EPSG:3395", 0.2);
    let expected = fixture["32768"]["length_m"].as_f64().unwrap();
    assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
    let polygon = Polygon::new(
        LineString::from(vec![
            (0.0, 4_000_000.0),
            (2_000_000.0, 4_200_000.0),
            (1_800_000.0, 6_000_000.0),
            (-100_000.0, 5_700_000.0),
            (0.0, 4_000_000.0),
        ]),
        vec![],
    );
    let crs = resolve_crs("EPSG:3395", "crs").unwrap();
    let measured =
        preserve_shape_area_with_limits(&[polygon], &crs, tolerance(100.0), limits(), None)
            .unwrap();
    let expected = fixture["32768"]["area_m2"].as_f64().unwrap();
    assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
}

// C1: Snyder 7-7, dominio aperto senza taglio; GOP-006 per metodi non provati.
#[test]
#[deny(clippy::disallowed_methods)]
fn projected_domain_edges_and_certified_empty_hotine() {
    for code in ["EPSG:3395", "EPSG:3857"] {
        let crs = resolve_crs(code, "crs").unwrap();
        for (x, y) in [
            (std::f64::consts::PI * 6_378_137.0, 0.0),
            (-std::f64::consts::PI * 6_378_137.0, 0.0),
            (0.0, 21_000_000.0),
            (0.0, -21_000_000.0),
        ] {
            let line = LineString::from(vec![(0.0, 0.0), (x, y)]);
            assert_eq!(
                preserve_shape_length_with_limits(&[line], &crs, tolerance(1.0), limits(), None),
                Err(ShapeMeasureError::CrsDomain)
            );
            let polygon = Polygon::new(rectangle(0.0, 0.0, x, y), vec![]);
            assert_eq!(
                preserve_shape_area_with_limits(&[polygon], &crs, tolerance(1.0), limits(), None),
                Err(ShapeMeasureError::CrsDomain)
            );
        }
    }
    // GOP-006: un metodo certificato ammette misure vuote esattamente nulle.
    let crs = resolve_crs("EPSG:2056", "crs").unwrap();
    for measured in [
        preserve_shape_length_with_limits(&[], &crs, tolerance(1.0), limits(), None).unwrap(),
        preserve_shape_area_with_limits(&[], &crs, tolerance(1.0), limits(), None).unwrap(),
    ] {
        assert_eq!(measured.value, 0.0);
        assert_eq!(measured.absolute_error_bound, 0.0);
        assert_eq!(measured.work, 0);
    }
}

// C1: Karney 2011 eq. 14,16,18,35,36; driver tm.py e GeographicLib Python 2.1.
// La fixture usa bisezione della latitudine isometrica e nessun import Rust.
#[test]
#[deny(clippy::disallowed_methods)]
fn transverse_line_and_polygon_against_independent_reference() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/preserve_shape/tm.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let code = format!("EPSG:{}", case["epsg"].as_u64().unwrap());
        let crs = resolve_crs(&code, "crs").unwrap();
        let coordinates = |name: &str| {
            LineString::from(
                case[name]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|point| (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()))
                    .collect::<Vec<_>>(),
            )
        };
        let measured = preserve_shape_length_with_limits(
            &[coordinates("line_xy")],
            &crs,
            tolerance(2.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["length_m"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
        let measured = preserve_shape_area_with_limits(
            &[Polygon::new(coordinates("ring_xy"), vec![])],
            &crs,
            tolerance(1_000_000.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["area_m2"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
    }
}

// C1: dominio normalizzato dimostrato in maggioranti-forma, EPSG 9807.
#[test]
#[deny(clippy::disallowed_methods)]
fn transverse_proof_domain_boundary_is_rejected() {
    let crs = resolve_crs("EPSG:32632", "crs").unwrap();
    let divisor = 6_378_137.0 * 0.9996;
    for (x, y) in [
        (500_000.0 + 0.1 * divisor, 5_000_000.0),
        (500_000.0 - 0.1 * divisor, 5_000_000.0),
        (500_000.0, 1.1 * divisor),
        (500_000.0, -1.1 * divisor),
    ] {
        // Il core CRS e' l'oracolo del dominio dell'inversa, distinto dal certificato.
        let context = ShapeContext::new(&crs, tolerance(2.0)).unwrap();
        let expected = match context.projection.geografiche_sorgente(x, y) {
            Ok(_) => ShapeMeasureError::CertifiedDomain,
            Err(_) => ShapeMeasureError::CrsDomain,
        };
        let line = LineString::from(vec![(500_000.0, 5_000_000.0), (x, y)]);
        assert_eq!(
            preserve_shape_length_with_limits(&[line], &crs, tolerance(2.0), limits(), None),
            Err(expected)
        );
        let polygon = Polygon::new(rectangle(490_000.0, 4_900_000.0, x, y), vec![]);
        assert_eq!(
            preserve_shape_area_with_limits(
                &[polygon],
                &crs,
                tolerance(1_000_000.0),
                limits(),
                None
            ),
            Err(expected)
        );
    }
}

// C1: Snyder 1987 cap. 15; EPSG GN 7-2 3.1.1.1; driver lcc.py e GeographicLib Python 2.1.
// La fixture usa bisezione della latitudine isometrica e nessun import Rust.
#[test]
#[deny(clippy::disallowed_methods)]
fn lambert_line_and_polygon_against_independent_reference() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/preserve_shape/lcc.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let code = format!("EPSG:{}", case["epsg"].as_u64().unwrap());
        let crs = resolve_crs(&code, "crs").unwrap();
        let coordinates = |name: &str| {
            LineString::from(
                case[name]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|point| (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()))
                    .collect::<Vec<_>>(),
            )
        };
        let measured = preserve_shape_length_with_limits(
            &[coordinates("line_xy")],
            &crs,
            tolerance(2.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["length_m"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
        let measured = preserve_shape_area_with_limits(
            &[Polygon::new(coordinates("ring_xy"), vec![])],
            &crs,
            tolerance(1_000_000.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["area_m2"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
    }
}

// C1: Snyder cap. 15; dominio convesso LCC, apice e taglio esclusi analiticamente.
#[test]
#[deny(clippy::disallowed_methods)]
fn lambert_proof_domain_boundary_is_rejected() {
    let crs = resolve_crs("EPSG:2154", "crs").unwrap();
    let axis = 6_378_137.0;
    // Raggio d'origine da formula indipendente Snyder, non dal kernel.
    let radius = 6_055_612.049_875_986;
    for (x, y) in [
        (700_000.0 + 0.25 * axis, 6_600_000.0),
        (700_000.0 - 0.25 * axis, 6_600_000.0),
        (700_000.0, 6_600_000.0 + radius - 0.75 * axis),
        (700_000.0, 6_600_000.0 + radius - 1.5 * axis),
        (700_000.0, 6_600_000.0 + radius),
        (700_000.0, 6_600_000.0 + radius + axis),
    ] {
        // Il core CRS e' l'oracolo del dominio dell'inversa, distinto dal certificato.
        let context = ShapeContext::new(&crs, tolerance(2.0)).unwrap();
        let expected = match context.projection.geografiche_sorgente(x, y) {
            Ok(_) => ShapeMeasureError::CertifiedDomain,
            Err(_) => ShapeMeasureError::CrsDomain,
        };
        let line = LineString::from(vec![(700_000.0, 6_600_000.0), (x, y)]);
        assert_eq!(
            preserve_shape_length_with_limits(&[line], &crs, tolerance(2.0), limits(), None),
            Err(expected)
        );
        let polygon = Polygon::new(rectangle(690_000.0, 6_500_000.0, x, y), vec![]);
        assert_eq!(
            preserve_shape_area_with_limits(
                &[polygon],
                &crs,
                tolerance(1_000_000.0),
                limits(),
                None
            ),
            Err(expected)
        );
    }
}

// C1: Snyder 1987 cap. 24; EPSG GN 7-2 3.3.2; driver laea.py e GeographicLib Python 2.1.
// La fixture usa bisezione della latitudine autalica e nessun import Rust.
#[test]
#[deny(clippy::disallowed_methods)]
fn azimuthal_line_and_polygon_against_independent_reference() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/laea.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let code = format!("EPSG:{}", case["epsg"].as_u64().unwrap());
        let crs = resolve_crs(&code, "crs").unwrap();
        let coordinates = |name: &str| {
            LineString::from(
                case[name]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|point| (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()))
                    .collect::<Vec<_>>(),
            )
        };
        let measured = preserve_shape_length_with_limits(
            &[coordinates("line_xy")],
            &crs,
            tolerance(2.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["length_m"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
        let measured = preserve_shape_area_with_limits(
            &[Polygon::new(coordinates("ring_xy"), vec![])],
            &crs,
            tolerance(10_000_000.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["area_m2"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
    }
}

// C1: Snyder cap. 24, disco rho<=2 Rq; rettangolo qualificato e bordo del disco rifiutati.
#[test]
#[deny(clippy::disallowed_methods)]
fn azimuthal_proof_domain_and_disk_boundary_are_rejected() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/laea.json"
    ))
    .unwrap();
    let crs = resolve_crs("EPSG:3035", "crs").unwrap();
    let dx = fixture["x_divisor"].as_f64().unwrap();
    let dy = fixture["y_divisor"].as_f64().unwrap();
    for (u, v) in [
        (0.1, 0.0),
        (-0.1, 0.0),
        (0.0, 0.1),
        (0.0, -0.1),
        (2.0, 0.0),
        (0.0, -2.0),
        (2.1, 0.0),
    ] {
        let x = 4_321_000.0 + u * dx;
        let y = 3_210_000.0 + v * dy;
        // Il core CRS e' l'oracolo del dominio dell'inversa, distinto dal certificato.
        let context = ShapeContext::new(&crs, tolerance(2.0)).unwrap();
        let expected = match context.projection.geografiche_sorgente(x, y) {
            Ok(_) => ShapeMeasureError::CertifiedDomain,
            Err(_) => ShapeMeasureError::CrsDomain,
        };
        let line = LineString::from(vec![(4_321_000.0, 3_210_000.0), (x, y)]);
        assert_eq!(
            preserve_shape_length_with_limits(&[line], &crs, tolerance(2.0), limits(), None),
            Err(expected)
        );
        let polygon = Polygon::new(rectangle(4_311_000.0, 3_200_000.0, x, y), vec![]);
        assert_eq!(
            preserve_shape_area_with_limits(
                &[polygon],
                &crs,
                tolerance(1_000_000.0),
                limits(),
                None
            ),
            Err(expected)
        );
    }
}

// C1: EPSG GN 7-2 3.3.1.1; Snyder cap. 21 sferico; driver stereographic.py e GeographicLib Python 2.1.
// La fixture usa bisezione della latitudine isometrica e nessun import Rust.
#[test]
#[deny(clippy::disallowed_methods)]
fn stereographic_line_and_polygon_against_independent_reference() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/stereographic.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let code = format!("EPSG:{}", case["epsg"].as_u64().unwrap());
        let crs = resolve_crs(&code, "crs").unwrap();
        let coordinates = |name: &str| {
            LineString::from(
                case[name]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|point| (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()))
                    .collect::<Vec<_>>(),
            )
        };
        let measured = preserve_shape_length_with_limits(
            &[coordinates("line_xy")],
            &crs,
            tolerance(2.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["length_m"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
        let measured = preserve_shape_area_with_limits(
            &[Polygon::new(coordinates("ring_xy"), vec![])],
            &crs,
            tolerance(10_000_000.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["area_m2"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
    }
}

// C1: EPSG GN 7-2 3.3.1.1; il bordo del rettangolo qualificato e' rifiutato.
#[test]
#[deny(clippy::disallowed_methods)]
fn stereographic_proof_domain_boundary_is_rejected() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/stereographic.json"
    ))
    .unwrap();
    let crs = resolve_crs("EPSG:28992", "crs").unwrap();
    let dx = fixture["divisor"].as_f64().unwrap();
    let dy = fixture["divisor"].as_f64().unwrap();
    for (u, v) in [
        (0.1, 0.0),
        (-0.1, 0.0),
        (0.0, 0.1),
        (0.0, -0.1),
        (2.0, 0.0),
        (0.0, -2.0),
        (2.1, 0.0),
    ] {
        let x = 155_000.0 + u * dx;
        let y = 463_000.0 + v * dy;
        // Il core CRS e' l'oracolo del dominio dell'inversa, distinto dal certificato.
        let context = ShapeContext::new(&crs, tolerance(2.0)).unwrap();
        let expected = match context.projection.geografiche_sorgente(x, y) {
            Ok(_) => ShapeMeasureError::CertifiedDomain,
            Err(_) => ShapeMeasureError::CrsDomain,
        };
        let line = LineString::from(vec![(155_000.0, 463_000.0), (x, y)]);
        assert_eq!(
            preserve_shape_length_with_limits(&[line], &crs, tolerance(2.0), limits(), None),
            Err(expected)
        );
        let polygon = Polygon::new(rectangle(145_000.0, 453_000.0, x, y), vec![]);
        assert_eq!(
            preserve_shape_area_with_limits(
                &[polygon],
                &crs,
                tolerance(1_000_000.0),
                limits(),
                None
            ),
            Err(expected)
        );
    }
}

// C1: EPSG GN 7-2 3.2.4.1; Snyder cap. 9; formula svizzera del motore; driver hotine.py e GeographicLib Python 2.1.
// La fixture usa bisezione della latitudine isometrica e nessun import Rust.
#[test]
#[deny(clippy::disallowed_methods)]
fn hotine_line_and_polygon_against_independent_reference() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/hotine.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let code = format!("EPSG:{}", case["epsg"].as_u64().unwrap());
        let crs = resolve_crs(&code, "crs").unwrap();
        let coordinates = |name: &str| {
            LineString::from(
                case[name]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|point| (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()))
                    .collect::<Vec<_>>(),
            )
        };
        let measured = preserve_shape_length_with_limits(
            &[coordinates("line_xy")],
            &crs,
            tolerance(2.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["length_m"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
        let measured = preserve_shape_area_with_limits(
            &[Polygon::new(coordinates("ring_xy"), vec![])],
            &crs,
            tolerance(10_000_000.0),
            limits(),
            None,
        )
        .unwrap();
        let expected = case["measurements"]["32768"]["area_m2"].as_f64().unwrap();
        assert!((measured.value - expected).abs() <= measured.absolute_error_bound);
    }
}

// C1: EPSG GN 7-2 3.2.4.1; il bordo del rettangolo qualificato e' rifiutato.
#[test]
#[deny(clippy::disallowed_methods)]
fn hotine_proof_domain_boundary_is_rejected() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/preserve_shape/hotine.json"
    ))
    .unwrap();
    let crs = resolve_crs("EPSG:2056", "crs").unwrap();
    let dx = fixture["divisor"].as_f64().unwrap();
    let dy = fixture["divisor"].as_f64().unwrap();
    for (u, v) in [
        (0.1, 0.0),
        (-0.1, 0.0),
        (0.0, 0.1),
        (0.0, -0.1),
        (2.0, 0.0),
        (0.0, -2.0),
        (2.1, 0.0),
    ] {
        let x = 2_600_000.0 + u * dx;
        let y = 1_200_000.0 + v * dy;
        // Il core CRS e' l'oracolo del dominio dell'inversa, distinto dal certificato.
        let context = ShapeContext::new(&crs, tolerance(2.0)).unwrap();
        let expected = match context.projection.geografiche_sorgente(x, y) {
            Ok(_) => ShapeMeasureError::CertifiedDomain,
            Err(_) => ShapeMeasureError::CrsDomain,
        };
        let line = LineString::from(vec![(2_600_000.0, 1_200_000.0), (x, y)]);
        assert_eq!(
            preserve_shape_length_with_limits(&[line], &crs, tolerance(2.0), limits(), None),
            Err(expected)
        );
        let polygon = Polygon::new(rectangle(2_590_000.0, 1_190_000.0, x, y), vec![]);
        assert_eq!(
            preserve_shape_area_with_limits(
                &[polygon],
                &crs,
                tolerance(1_000_000.0),
                limits(),
                None
            ),
            Err(expected)
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn certificate_components_are_public_and_sum_to_absolute_bound() {
    let parts = [LineString::from(vec![(0.0, 30.0), (0.01, 30.0)])];
    let measured = length(&parts, "EPSG:4326", 1.0);
    let c = measured.error_components;
    assert_eq!(measured.relative_error_bound, 0.0);
    assert_eq!(c.transformation, 0.0);
    assert_eq!(
        (c.transformation + c.discretization) + c.numerical,
        measured.absolute_error_bound
    );
    assert!(c.discretization >= 0.0 && c.numerical >= 0.0);
    // Parallelo ellissoidico: N(phi)*cos(phi)*delta_lambda.
    let f = 1.0 / 298.257_223_563;
    let phi = 30.0_f64.to_radians();
    let radius = 6_378_137.0 / (1.0 - f * (2.0 - f) * phi.sin().powi(2)).sqrt();
    let exact = radius * phi.cos() * 0.01_f64.to_radians();
    assert!((measured.value - exact).abs() <= measured.absolute_error_bound);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn overlapping_independent_area_operands_are_summed() {
    let polygon = Polygon::new(rectangle(0.0, 0.0, 0.01, 0.01), vec![]);
    let single = area(std::slice::from_ref(&polygon), 1.0);
    let doubled = area(&[polygon.clone(), polygon], 2.0);
    assert!(
        (doubled.value - 2.0 * single.value).abs()
            <= doubled.absolute_error_bound + 2.0 * single.absolute_error_bound
    );
}
