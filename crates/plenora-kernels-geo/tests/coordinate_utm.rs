#![deny(clippy::disallowed_methods)]
//! C1: `NGA.STND.0037_2.0.0_GRIDS` (2014-02-28), 2-3/2-4, 3-2, appendice A.
//! <https://geotimege.com/wp-content/uploads/2023/12/27.national-geospatial-intelligence-agency.pdf>
//! `GeographicLib` `GeoConvert`, manuale distribuito con 2.1.2, DESCRIPTION/EXAMPLES:
//! <https://geographiclib.sourceforge.io/html/GeoConvert.1.html>
//! <https://sources.debian.org/src/geographiclib/2.1.2-1/man/GeoConvert.1.html>
//! Nessun eseguibile esterno richiesto; attese scritte, non generate dal codec.

use geographiclib_rs::{Geodesic, InverseGeodesic};
use plenora_kernels_geo::coordinate_utm::{
    format_utm, from_utm, parse_utm, to_utm, UtmCoordinate,
    UtmError::{Domain, Hemisphere, Length, Precision, Syntax, Zone},
};

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    Geodesic::wgs84().inverse(a.1, a.0, b.1, b.0)
}

#[test]
#[deny(clippy::disallowed_methods)]
fn published_and_analytic_references() {
    // GeographicLib 2.1.2 GeoConvert DESCRIPTION: Baghdad 33.3N 44.4E,
    // 38n 444140.54 3684706.36 (valori pubblicati al centimetro).
    let baghdad = to_utm(44.4, 33.3).expect("published point");
    assert_eq!((baghdad.zone, baghdad.hemisphere), (38, 'N'));
    assert!((baghdad.easting - 444_140.54).abs() < 0.005);
    assert!((baghdad.northing - 3_684_706.36).abs() < 0.005);
    // GOP-024: arrotondamento di coordinate di punto, non cella MGRS.
    assert_eq!(format_utm(44.4, 33.3, 1), Ok("38 N 444141 3684706".into()));
    // GeographicLib 2.1.2 GeoConvert EXAMPLES: 38n 444500 3688500 ->
    // 33.33424 44.40363, pubblicato a 5 decimali: tolleranza 0.8 m.
    let point = parse_utm("38 N 444500 3688500").expect("published inverse");
    assert!(distance(point, (44.40363, 33.33424)) < 0.8);
    // NGA 2-3 tabelle 1/2: parita' della TM e falso northing sud 10^7 m.
    let south = to_utm(44.4, -33.3).expect("southern reflection");
    assert_eq!((south.zone, south.hemisphere), (38, 'S'));
    assert!((south.easting - 444_140.54).abs() < 0.005);
    assert!((south.northing - 6_315_293.64).abs() < 0.005);
    assert_eq!(format_utm(44.4, -33.3, 1), Ok("38 S 444141 6315294".into()));
    assert!(
        distance(
            parse_utm("38 S 444141 6315294").expect("southern text"),
            (44.4, -33.3)
        ) < 0.722
    );
    // NGA 2-3 tabelle 1/3: tutti i meridiani centrali all'equatore (500km,0).
    for zone in 1_u8..=60 {
        let lon = f64::from(zone).mul_add(6.0, -183.0);
        let value = to_utm(lon, 0.0).expect("central meridian");
        assert_eq!((value.zone, value.hemisphere), (zone, 'N'));
        assert!((value.easting - 500_000.0).abs() < 1e-8);
        assert_eq!(value.northing.to_bits(), 0.0_f64.to_bits());
        assert!(distance(from_utm(value).expect("inverse origin"), (lon, 0.0)) < 0.01);
    }
    // NGA A-4 metri UTM, riscritti nella sintassi normativa GOP-023.
    let point = parse_utm("15 S 486911 3852087").expect("NGA inverse");
    let projected = to_utm(point.0, point.1).expect("NGA grid");
    assert_eq!((projected.zone, projected.hemisphere), (15, 'S'));
    assert!((projected.easting - 486_911.0).abs() < 1e-7);
    assert!((projected.northing - 3_852_087.0).abs() < 1e-7);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn zone_and_hemisphere_boundaries() {
    // NGA 2-4 tabelle 6/7/8: limiti normali, Norvegia e Svalbard.
    for (lon, lat, zone, band) in [
        (-180.0, 0.0, 1, 'N'),
        (180.0, 0.0, 60, 'N'),
        (0.0_f64.next_down(), 0.0, 30, 'N'),
        (0.0, 0.0, 31, 'N'),
        (6.0_f64.next_down(), 0.0, 31, 'N'),
        (6.0, 0.0, 32, 'N'),
        (3.0_f64.next_down(), 56.0, 31, 'N'),
        (3.0, 56.0, 32, 'N'),
        (3.0, 56.0_f64.next_down(), 31, 'N'),
        (3.0, 64.0, 31, 'N'),
        (12.0, 56.0, 33, 'N'),
        (0.0, 72.0, 31, 'N'),
        (9.0_f64.next_down(), 72.0, 31, 'N'),
        (9.0, 72.0, 33, 'N'),
        (21.0, 72.0, 35, 'N'),
        (33.0, 72.0, 37, 'N'),
        (42.0, 72.0, 38, 'N'),
        (9.0, 72.0_f64.next_down(), 32, 'N'),
        (9.0, 84.0, 33, 'N'),
        (9.0, -80.0, 32, 'S'),
    ] {
        let value = to_utm(lon, lat).expect("zone boundary");
        assert_eq!((value.zone, value.hemisphere), (zone, band), "{lon} {lat}");
        assert!(distance(from_utm(value).expect("numeric boundary"), (lon, lat)) < 0.01);
        let text = format_utm(lon, lat, 1).expect("boundary text");
        assert!(distance(parse_utm(&text).expect("text boundary"), (lon, lat)) < 0.722);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn invalid_text_is_enumerated_and_typed() {
    // GOP-025 e grammatica canonica GOP-023: nessun tentativo di indovinare.
    for (text, expected) in [
        ("", Syntax),
        ("38 N 444141", Syntax),
        ("38 N 444141 3684706 x", Syntax),
        ("38 N  444141 3684706", Syntax),
        (" 38 N 444141 3684706", Syntax),
        ("38 N 444141 3684706 ", Syntax),
        ("38 N\t444141 3684706", Syntax),
        ("38 N +444141 3684706", Syntax),
        ("38 N -444141 3684706", Syntax),
        ("38 N 444141.0 3684706", Syntax),
        ("38 N 4e5 3684706", Syntax),
        ("38 N 444,141 3684706", Syntax),
        ("38 N NaN 3684706", Syntax),
        ("38 N \u{ff14} 0", Syntax),
        ("038 N 444141 3684706", Syntax),
        ("38 N 0444141 3684706", Syntax),
        ("38S 444141 3684706", Syntax),
        ("38 N 444141 3684706\n", Syntax),
        ("38 n 444141 3684706", Hemisphere),
        ("61 N 444141 3684706", Zone),
        ("0 N 444141 3684706", Zone),
        ("38 I 444141 3684706", Hemisphere),
        ("38 O 444141 3684706", Hemisphere),
        ("32 N 500000 8500000", Domain),
        ("34 N 500000 8500000", Domain),
        ("36 N 500000 8500000", Domain),
        ("38 C 444141 3684706", Hemisphere),
        ("38 N 99999999 3684706", Domain),
        ("38 N 444141 99999999", Domain),
        ("Z 3000000 2000000", Syntax),
        ("Y 2200000 2000000", Syntax),
        ("N 2000000 2000000", Syntax),
        ("Z 2000000 2000000 extra", Syntax),
    ] {
        assert_eq!(parse_utm(text), Err(expected), "{text}");
        assert_eq!(parse_utm(text), parse_utm(text));
    }
    // GOP-025: limite byte, prima di qualsiasi parsing/allocazione.
    assert_eq!(parse_utm(&"x".repeat(24)), Err(Syntax));
    assert_eq!(parse_utm(&"x".repeat(25)), Err(Length));
    assert_eq!(parse_utm(&"é".repeat(13)), Err(Length));
    // GOP-024: risoluzione obbligatoria di 1 metro.
    for precision in [0, 2, 255] {
        assert_eq!(format_utm(0.0, 0.0, precision), Err(Precision));
    }
    // GOP-025: finitezza e dominio prima della quantizzazione.
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        180.0_f64.next_up(),
        -181.0,
    ] {
        assert_eq!(to_utm(value, 0.0), Err(Domain));
    }
    for value in [
        f64::NAN,
        f64::INFINITY,
        84.0_f64.next_up(),
        (-80.0_f64).next_down(),
        -90.0,
        90.0,
    ] {
        assert_eq!(to_utm(0.0, value), Err(Domain));
        assert_eq!(format_utm(0.0, value, 1), Err(Domain));
    }
    let valid = UtmCoordinate {
        zone: 31,
        hemisphere: 'N',
        easting: 500_000.0,
        northing: 0.0,
    };
    assert_eq!(from_utm(UtmCoordinate { zone: 255, ..valid }), Err(Zone));
    assert_eq!(from_utm(UtmCoordinate { zone: 0, ..valid }), Err(Zone));
    assert_eq!(
        from_utm(UtmCoordinate {
            hemisphere: 'I',
            ..valid
        }),
        Err(Hemisphere)
    );
    assert_eq!(
        from_utm(UtmCoordinate {
            easting: f64::NAN,
            ..valid
        }),
        Err(Domain)
    );
    assert_eq!(
        from_utm(UtmCoordinate {
            northing: f64::INFINITY,
            ..valid
        }),
        Err(Domain)
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn round_trip_ground_bound() {
    // GOP-024: zero negativo normalizzato e vicino ai limiti/antimeridiano.
    for point in [
        (3.0, -0.0),
        (0.0, -0.0),
        (-180.0, 83.999_999),
        (180.0, -79.999_999),
        (179.999_999_999, 84.0),
        (-179.999_999_999, 84.0),
    ] {
        let text = format_utm(point.0, point.1, 1).expect("special coordinate");
        assert!(distance(parse_utm(&text).expect("special text"), point) <= 0.722);
    }
    // GOP-024: 0.5 m per asse / scala minima UTM .9996 -> <.712 m;
    // piu' .01 m di errore di calcolo. Distanza geodetica indipendente dal codec.
    for longitude in -180..=180 {
        for latitude in -80..=84 {
            let point = (f64::from(longitude), f64::from(latitude));
            let value = to_utm(point.0, point.1).expect("world grid");
            let back = from_utm(value).expect("world numeric inverse");
            assert!(
                distance(point, back) <= 0.01,
                "{point:?} {value:?} {back:?}"
            );
            let text = format_utm(point.0, point.1, 1).expect("world format");
            let back = parse_utm(&text).expect("world text inverse");
            assert!(distance(point, back) <= 0.722, "{point:?} {text} {back:?}");
            assert!(text.len() <= 24);
        }
    }
}
