//! Oracoli C1: G1 Karney 2013, equazioni della metrica ellissoidica;
//! G2 `GeographicLib` Python 2.1, esempi e Multiple shortest geodesics.

#![allow(clippy::float_cmp, reason = "Identita esatte di poli e vertici")]
#![allow(
    clippy::suboptimal_flops,
    reason = "Formule indipendenti scritte nella forma matematica"
)]

use super::*;
use plenora_core::crs::resolve_crs;

fn limits() -> GeodesicDensifyLimits {
    GeodesicDensifyLimits {
        max_points: 10_000,
        max_reserved_bytes: 1_000_000,
        deadline: None,
    }
}

fn densify(parts: &[LineString<f64>], maximum: f64) -> Vec<LineString<f64>> {
    geodesic_densify_with_limits(
        parts,
        &resolve_crs("EPSG:4326", "crs").unwrap(),
        maximum,
        limits(),
        None,
    )
    .unwrap()
}

// G1 equazioni 1-3: ds/dphi = a(1-e^2)/(1-e^2 sin^2(phi))^(3/2).
// Simpson con 16384 intervalli su [0,pi/2]: errore sotto 1 micrometro
// (derivata quarta liscia per e^2<0.007); non usa GeographicLib.
fn meridian(latitude: f64) -> f64 {
    let flattening = 1.0 / 298.257_223_563;
    let eccentricity = flattening * (2.0 - flattening);
    let radius = |phi: f64| {
        6_378_137.0 * (1.0 - eccentricity) / (1.0 - eccentricity * phi.sin().powi(2)).powf(1.5)
    };
    let intervals = 16_384_u32;
    let step = latitude.to_radians() / f64::from(intervals);
    let mut sum = radius(0.0) + radius(latitude.to_radians());
    for i in 1..intervals {
        sum += if i % 2 == 0 { 2.0 } else { 4.0 } * radius(f64::from(i) * step);
    }
    sum * step / 3.0
}

fn verify_path(input: &LineString<f64>, output: &LineString<f64>, maximum: f64) {
    let ellipsoid = crate::geodetica::wgs84_di_prova();
    let first = input.0[0];
    let last = input.0[1];
    let (length, azimuth, _, _): (f64, f64, f64, f64) = ellipsoid
        .geodetica()
        .inverse(first.y, first.x, last.y, last.x);
    let divisions = u32::try_from(output.0.len() - 1).unwrap();
    assert_eq!(output.0.first(), input.0.first());
    assert_eq!(output.0.last(), input.0.last());
    for (i, point) in output.0.iter().enumerate().skip(1) {
        let (distance, bearing, _, _): (f64, f64, f64, f64) = ellipsoid
            .geodetica()
            .inverse(first.y, first.x, point.y, point.x);
        // G2 Additional properties: additivita' delle distanze sullo stesso arco.
        assert!(
            (distance - length * f64::from(u32::try_from(i).unwrap()) / f64::from(divisions)).abs()
                < 1e-6
        );
        if point.y.abs() != 90.0 {
            assert!((bearing - azimuth).abs() < 1e-7);
        }
    }
    for pair in output.0.windows(2) {
        let distance: f64 = ellipsoid
            .geodetica()
            .inverse(pair[0].y, pair[0].x, pair[1].y, pair[1].x);
        assert!(distance <= maximum);
    }
}

#[test]
fn equator_analytic_and_exact_budget() {
    let line = LineString::from(vec![(0.0, 0.0), (2.0, 0.0)]);
    // G1 metrica all'equatore: L=a*delta_lambda; tre parti di 2/3 grado.
    let result = densify(std::slice::from_ref(&line), 100_000.0);
    assert_eq!(result[0].0.len(), 4);
    for (i, point) in result[0].0.iter().enumerate() {
        assert!((point.x - f64::from(u32::try_from(i).unwrap()) * 2.0 / 3.0).abs() < 1e-12);
        assert!(point.y.abs() < 1e-12);
    }
    verify_path(&line, &result[0], 100_000.0);
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    // Capacita' richieste: una parte + quattro coordinate, nessun piano temporaneo.
    let bytes = (size_of::<LineString<f64>>() + 4 * size_of::<Coord<f64>>()) as u64;
    let exact = GeodesicDensifyLimits {
        max_points: 4,
        max_reserved_bytes: bytes,
        deadline: None,
    };
    assert_eq!(
        geodesic_densify_with_limits(std::slice::from_ref(&line), &crs, 100_000.0, exact, None)
            .unwrap(),
        result
    );
    for (budget, error) in [
        (
            GeodesicDensifyLimits {
                max_points: 3,
                ..exact
            },
            GeodesicDensifyError::PointLimit,
        ),
        (
            GeodesicDensifyLimits {
                max_reserved_bytes: bytes - 1,
                ..exact
            },
            GeodesicDensifyError::MemoryLimit,
        ),
    ] {
        assert_eq!(
            geodesic_densify_with_limits(
                std::slice::from_ref(&line),
                &crs,
                100_000.0,
                budget,
                None
            ),
            Err(error)
        );
    }
    assert_eq!(
        geodesic_densify_with_limits(&[line], &crs, f64::MIN_POSITIVE, exact, None),
        Err(GeodesicDensifyError::PointLimit)
    );
}

#[test]
fn meridian_and_pole_from_metric_integral() {
    let line = LineString::from(vec![(0.0, 0.0), (0.0, 80.0)]);
    let result = densify(std::slice::from_ref(&line), 2_000_000.0);
    let divisions = u32::try_from(result[0].0.len() - 1).unwrap();
    for (i, point) in result[0].0.iter().enumerate() {
        // G1 metrica: integrale meridiano indipendente, tolleranza 1 micrometro.
        let expected = meridian(80.0) * f64::from(u32::try_from(i).unwrap()) / f64::from(divisions);
        assert!((meridian(point.y) - expected).abs() < 1e-6);
        assert_eq!(point.x, 0.0);
    }
    verify_path(&line, &result[0], 2_000_000.0);
    let polar = LineString::from(vec![(0.0, 80.0), (180.0, 80.0)]);
    let result = densify(std::slice::from_ref(&polar), 1_200_000.0);
    // G1 simmetria del meridiano: meta' arco e' il polo nord, lon canonica 0.
    assert_eq!(result[0].0[1], Coord { x: 0.0, y: 90.0 });
    verify_path(&polar, &result[0], 1_200_000.0);
    let ellipsoid = crate::geodetica::wgs84_di_prova();
    let actual: f64 = ellipsoid.geodetica().inverse(80.0, 0.0, 80.0, 180.0);
    assert!((actual - 2.0 * (meridian(90.0) - meridian(80.0))).abs() < 1e-6);
}

#[test]
fn published_long_oblique_and_unique_near_antipodes() {
    let line = LineString::from(vec![(174.81, -41.32), (-5.50, 40.96)]);
    let result = densify(std::slice::from_ref(&line), 1_000_000.0);
    verify_path(&line, &result[0], 1_000_000.0);
    let ellipsoid = crate::geodetica::wgs84_di_prova();
    let mut total = 0.0;
    for pair in result[0].0.windows(2) {
        let distance: f64 = ellipsoid
            .geodetica()
            .inverse(pair[0].y, pair[0].x, pair[1].y, pair[1].x);
        total += distance;
    }
    // G2 Python 2.1 Examples / Basic geodesic calculations, Wellington-Salamanca:
    // https://geographiclib.sourceforge.io/html/python/examples.html
    // Pubblicato a 0.001 m: intervallo di arrotondamento 0.0005 m + 1 micrometro.
    assert!((total - 19_959_679.267).abs() <= 0.000_501);
    assert_eq!(result[0].0.len(), 21);
    // G2 Python 2.1 API / geographiclib.geodesic / Example, valori completi:
    // https://geographiclib.sourceforge.io/html/python/code.html
    // La tolleranza di 1 micrometro supera la precisione di stampa e G1.
    assert!((total - 19_959_679.267_353_82).abs() < 1e-6);
    for (i, point) in result[0].0.iter().enumerate().skip(1) {
        let (distance, azimuth, _, _): (f64, f64, f64, f64) = ellipsoid
            .geodetica()
            .inverse(-41.32, 174.81, point.y, point.x);
        assert!((azimuth - 161.067_669_986_158_82).abs() < 1e-7);
        let expected = 19_959_679.267_353_82 * f64::from(u32::try_from(i).unwrap()) / 20.0;
        assert!((distance - expected).abs() < 1e-6);
    }
}

#[test]
fn ambiguities_domains_and_controls() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    // G2 Multiple shortest geodesics: latitudini opposte e azimut distinti;
    // poli opposti, indipendentemente dalla longitudine.
    for endpoints in [
        vec![(0.0, 0.0), (180.0, 0.0)],
        vec![(0.0, 10.0), (179.9, -10.0)],
        vec![(12.0, 90.0), (17.0, -90.0)],
    ] {
        assert_eq!(
            geodesic_densify_with_limits(&[LineString::from(endpoints)], &crs, 1e6, limits(), None),
            Err(GeodesicDensifyError::AmbiguousGeodesic)
        );
    }
    let parts = [LineString::from(vec![(0.0, 0.0), (1.0, 0.0)])];
    assert_eq!(
        geodesic_densify_with_limits(
            &parts,
            &resolve_crs("EPSG:3857", "crs").unwrap(),
            1e6,
            limits(),
            None
        ),
        Err(GeodesicDensifyError::GeographicRequired)
    );
    for maximum in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            geodesic_densify_with_limits(&parts, &crs, maximum, limits(), None),
            Err(GeodesicDensifyError::InvalidMaximum)
        );
    }
    for point in [(181.0, 0.0), (0.0, 91.0), (f64::NAN, 0.0)] {
        assert_eq!(
            geodesic_densify_with_limits(
                &[LineString::from(vec![point])],
                &crs,
                1e6,
                limits(),
                None
            ),
            Err(GeodesicDensifyError::InvalidCoordinate)
        );
    }
    let expired = GeodesicDensifyLimits {
        deadline: Some(Instant::now()),
        ..limits()
    };
    assert_eq!(
        geodesic_densify_with_limits(&parts, &crs, 1e6, expired, None),
        Err(GeodesicDensifyError::DeadlineExceeded)
    );
    assert_eq!(
        geodesic_densify_with_limits(&parts, &crs, 1e6, expired, Some(&AtomicBool::new(true))),
        Err(GeodesicDensifyError::DeadlineExceeded)
    );
    let mut work = DensifyWork {
        limits: limits(),
        cancelled: None,
        points: 0,
        bytes: 0,
    };
    let flag = AtomicBool::new(false);
    work.cancelled = Some(&flag);
    work.checkpoint().unwrap();
    flag.store(true, Ordering::Relaxed);
    assert_eq!(work.checkpoint(), Err(GeodesicDensifyError::Cancelled));
    work.cancelled = None;
    work.limits.deadline = Some(Instant::now());
    assert_eq!(
        work.checkpoint(),
        Err(GeodesicDensifyError::DeadlineExceeded)
    );
}

#[test]
fn multipart_empty_closure_seam_and_duplicates() {
    let ring = LineString::from(vec![(179.0, 0.0), (-179.0, 0.0), (179.0, 0.0)]);
    let parts = [
        LineString::new(vec![]),
        ring.clone(),
        LineString::from(vec![(10.0, 90.0), (20.0, 90.0)]),
    ];
    let result = densify(&parts, 150_000.0);
    // GOP-004/007: parti vuote, originali e chiusura invariati; seam canonico.
    assert_eq!(result[0], parts[0]);
    assert_eq!(
        result[1].0,
        vec![
            ring.0[0],
            Coord { x: -180.0, y: 0.0 },
            ring.0[1],
            Coord { x: -180.0, y: 0.0 },
            ring.0[2]
        ]
    );
    assert_eq!(result[2], parts[2]);
    assert_eq!(densify(&[], 1.0), Vec::<LineString<f64>>::new());
}

#[test]
fn rounding_cannot_choose_an_antipodal_branch_or_hide_a_short_edge() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    // G1 sezione 5: la perturbazione dalla simmetria decide il ramo minimo;
    // G2 AngRound non deve trasformare il caso unico in un ramo arbitrario.
    for line in [
        LineString::from(vec![(0.0, -1e-18), (179.8, -1e-18)]),
        LineString::from(vec![(0.0, 1e-18), (179.8, 1e-18)]),
        LineString::from(vec![(0.0, 0.0), (0.0, 1e-20)]),
    ] {
        assert_eq!(
            geodesic_densify_with_limits(&[line], &crs, 1e6, limits(), None),
            Err(GeodesicDensifyError::AccuracyUnproven)
        );
    }
}

#[test]
fn exact_seam_identity_and_numeric_length_threshold() {
    // Identita' geografica GOP-004: solo gli estremi esatti +/-180 coincidono.
    let first = Coord { x: 180.0, y: 0.0 };
    let same = Coord { x: -180.0, y: 0.0 };
    let nearby = Coord {
        x: (-180.0_f64).next_up(),
        y: 0.0,
    };
    assert!(coincident(first, same));
    assert!(!coincident(first, nearby));
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    assert_eq!(
        geodesic_densify_with_limits(
            &[],
            &crs,
            1.0,
            GeodesicDensifyLimits {
                max_points: 0,
                max_reserved_bytes: 0,
                deadline: None
            },
            None
        )
        .unwrap(),
        vec![]
    );
    let line = LineString::from(vec![(0.0, 0.0), (1.0, 0.0)]);
    // G1 equatore L=a*pi/180; il margine dichiarato raffina la soglia esatta.
    let length = 6_378_137.0 * std::f64::consts::PI / 180.0;
    assert_eq!(densify(std::slice::from_ref(&line), length)[0].0.len(), 3);
    assert_eq!(
        densify(std::slice::from_ref(&line), length + 2e-6)[0]
            .0
            .len(),
        2
    );
    assert_eq!(
        densify(std::slice::from_ref(&line), length / 2.0)[0]
            .0
            .len(),
        4
    );
    // La stima enorme di punti si rifiuta prima di riservare il vettore.
    assert_eq!(
        geodesic_densify_with_limits(
            &[line],
            &crs,
            0.001,
            GeodesicDensifyLimits {
                max_points: 2,
                ..limits()
            },
            None
        ),
        Err(GeodesicDensifyError::PointLimit)
    );
}
