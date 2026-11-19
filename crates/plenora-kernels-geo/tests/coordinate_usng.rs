#![deny(clippy::disallowed_methods)]
//! C1 FGDC-STD-011-2001, dicembre 2001, 3-5, figure 1-3, annessi A/B/E.
//! <https://www.fgdc.gov/standards/projects/FGDC-standards-projects/usng/fgdc_std_011_2001_usng.pdf>
//! Oracolo numerico offline: `scripts/usng_oracle.py`, integrale meridiano
//! indipendente dal codec Rust, ellissoidi EPSG 7019 e 7008.

use plenora_core::crs::riproiezione::utm::{project_utm, GridEllipsoid};
use plenora_kernels_geo::{
    coordinate_usng::{format_usng, parse_usng, UsngDatum, UsngError},
    grid_text::CellRepresentative::{Center, Southwest},
};

#[test]
#[deny(clippy::disallowed_methods)]
fn independent_numeric_oracle() {
    for line in include_str!("fixtures/usng-oracle.csv")
        .lines()
        .filter(|s| !s.starts_with('#'))
    {
        let values: Vec<_> = line.split(',').collect();
        let (datum, ellipsoid) = match values[0] {
            "NAD83" => (UsngDatum::Nad83, GridEllipsoid::Grs80),
            "NAD27" => (UsngDatum::Nad27, GridEllipsoid::Clarke1866),
            _ => unreachable!("fixture datum"),
        };
        let lon: f64 = values[1].parse().expect("fixture longitude");
        let lat: f64 = values[2].parse().expect("fixture latitude");
        let x: f64 = values[3].parse().expect("fixture easting");
        let y: f64 = values[4].parse().expect("fixture northing");
        let expected = values[5];
        let fields: Vec<_> = expected.split(' ').collect();
        let suffix = if datum == UsngDatum::Nad27 {
            " (NAD 27)"
        } else {
            ""
        };
        for digits in 0..=5_u8 {
            let width = usize::from(digits);
            let text = if digits == 0 {
                format!("{} {}{suffix}", fields[0], fields[1])
            } else {
                format!(
                    "{} {} {} {}{suffix}",
                    fields[0],
                    fields[1],
                    &fields[2][..width],
                    &fields[3][..width]
                )
            };
            assert_eq!(
                format_usng(lon, lat, digits, datum).expect("oracle encoding"),
                text
            );
            let center = parse_usng(&text, datum, Center).expect("oracle cell");
            let sw = parse_usng(&text, datum, Southwest).expect("southwest cell");
            let side = 10_u32.pow(u32::from(5 - digits));
            assert_eq!(center.cell, sw.cell);
            assert_eq!(center.datum, datum);
            assert_eq!(center.representative, Center);
            assert_eq!(sw.representative, Southwest);
            assert_eq!(center.cell.east - center.cell.west, side);
            assert_eq!(center.cell.north - center.cell.south, side);
            assert!(x >= f64::from(center.cell.west) && x < f64::from(center.cell.east));
            assert!(y >= f64::from(center.cell.south) && y < f64::from(center.cell.north));
            if digits == 5 {
                // Integrale inverso e sviluppo locale: 1e-9 gradi < 0.12 mm.
                for (actual, index) in [
                    (sw.longitude, 6),
                    (sw.latitude, 7),
                    (center.longitude, 8),
                    (center.latitude, 9),
                ] {
                    let expected_angle: f64 = values[index].parse().expect("oracle angle");
                    assert!((actual - expected_angle).abs() < 1e-9);
                }
                let (east, north) =
                    project_utm(ellipsoid, center.cell.zone, lat >= 0.0, false, lon, lat)
                        .expect("projection");
                assert!((east - x).hypot(north - y) < 0.001);
            }
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn fgdc_worked_examples() {
    // FGDC 3.3.3 e annesso B tabella 1: 323483.168 E, 4306479.498 N.
    for (body, west, south, side) in [
        ("18S UJ", 300_000, 4_300_000, 100_000),
        ("18S UJ 2 0", 320_000, 4_300_000, 10_000),
        ("18S UJ 23 06", 323_000, 4_306_000, 1_000),
        ("18S UJ 234 064", 323_400, 4_306_400, 100),
        ("18S UJ 2348 0647", 323_480, 4_306_470, 10),
        ("18S UJ 23483 06479", 323_483, 4_306_479, 1),
    ] {
        // Annesso E: identica griglia di lettere per entrambi i datum.
        for (datum, suffix) in [(UsngDatum::Nad83, ""), (UsngDatum::Nad27, " (NAD 27)")] {
            let decoded =
                parse_usng(&format!("{body}{suffix}"), datum, Southwest).expect("FGDC example");
            assert_eq!(
                (
                    decoded.cell.west,
                    decoded.cell.south,
                    decoded.cell.east,
                    decoded.cell.north
                ),
                (west, south, west + side, south + side)
            );
        }
    }
    // Annessi A/E: esempio Washington Monument NAD27, senza trasformare NAD83.
    let nad27 =
        parse_usng("18S UJ 2345 0626 (NAD 27)", UsngDatum::Nad27, Southwest).expect("annex E");
    assert_eq!((nad27.cell.west, nad27.cell.south), (323_450, 4_306_260));
    let nad83 =
        parse_usng("18S UJ 2345 0626", UsngDatum::Nad83, Southwest).expect("same grid numbers");
    assert!((nad83.latitude - nad27.latitude).abs() > 0.001);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn boundaries_and_representatives() {
    for (datum, suffix) in [(UsngDatum::Nad83, ""), (UsngDatum::Nad27, " (NAD 27)")] {
        // FGDC figura 1, NGA tabelle 6-8: fusi, bande, Norvegia/Svalbard.
        for (lon, lat, prefix) in [
            (-180.0, 0.0, "1N "),
            (180.0, 0.0, "60N "),
            (0.0_f64.next_down(), 0.0, "30N "),
            (0.0, 0.0, "31N "),
            (3.0_f64.next_down(), 56.0, "31V "),
            (3.0, 56.0, "32V "),
            (12.0, 56.0, "33V "),
            (9.0_f64.next_down(), 72.0, "31X "),
            (9.0, 72.0, "33X "),
            (21.0, 72.0, "35X "),
            (33.0, 72.0, "37X "),
            (42.0, 72.0, "38X "),
            (0.0, -80.0, "31C "),
            (0.0, 84.0, "31X "),
            (3.0, 8.0_f64.next_down(), "31N "),
            (3.0, 8.0, "31P "),
        ] {
            for digits in 0..=5 {
                let text = format_usng(lon, lat, digits, datum).expect("boundary");
                assert!(text.starts_with(prefix));
                assert_eq!(
                    parse_usng(&text, datum, Center)
                        .expect("boundary cell")
                        .representative,
                    Center
                );
            }
        }
        // FGDC figura 3: origine esatta e lato ovest del meridiano centrale.
        assert_eq!(
            format_usng(3.0, 0.0, 5, datum),
            Ok(format!("31N EA 00000 00000{suffix}"))
        );
        assert_eq!(
            format_usng(3.0_f64.next_down(), 0.0, 5, datum),
            Ok(format!("31N DA 99999 00000{suffix}"))
        );
        assert_eq!(
            format_usng(3.0, -f64::from_bits(1), 5, datum),
            Ok(format!("31M EV 00000 99999{suffix}"))
        );
        let sw =
            parse_usng(&format!("31N EA 00000 00000{suffix}"), datum, Southwest).expect("origin");
        assert!((sw.longitude - 3.0).abs() < 1e-12 && sw.latitude.abs() < 1e-12);
        let center =
            parse_usng(&format!("31N EA 00000 00000{suffix}"), datum, Center).expect("center");
        assert!(center.longitude > sw.longitude && center.latitude > sw.latitude);
        // GOP-024: non agganciare il centro al fuso se la cella lo attraversa.
        let crossing =
            parse_usng(&format!("31V DC{suffix}"), datum, Center).expect("crossing cell");
        assert_eq!(crossing.cell.east, 500_000);
        assert_eq!(
            parse_usng(&format!("31V EC{suffix}"), datum, Center),
            Err(UsngError::DataMapping)
        );
        for lat in [(-80.0_f64).next_down(), 84.0_f64.next_up(), -90.0, 90.0] {
            assert_eq!(format_usng(0.0, lat, 5, datum), Err(UsngError::Unsupported));
        }
        for polar in ["A ZN", "B AN", "Y ZH", "Z AH"] {
            assert_eq!(
                parse_usng(&format!("{polar}{suffix}"), datum, Center),
                Err(UsngError::Unsupported)
            );
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn strict_grammar_datum_and_errors() {
    // GOP-023/025: niente abbreviazioni, spazi alternativi, Unicode o inferenze.
    for text in [
        "",
        "18S",
        "UJ",
        "UJ 2348 0647",
        "2348 0647",
        "18SUJ23480647",
        "18S  UJ",
        "18S UJ ",
        " 18S UJ",
        "18S\tUJ",
        "18s UJ",
        "18S uj",
        "18S UJ 1",
        "18S UJ 1 12",
        "18S UJ  1",
        "18S UJ 000000 000000",
        "18S UJ +1 01",
        "18S UJ １ １",
        "18S UJ 01 01\n",
        "018S UJ",
        "01N EA",
        "0N EA",
        "61N EA",
        "31I EA",
        "32X NA",
        "31N EI",
        "31N IA",
        "18S UJ (NAD 27)",
        "18S UJ (NAD 83)",
        "Z ZZ",
        "Z",
        "Z AH 1",
    ] {
        assert_eq!(
            parse_usng(text, UsngDatum::Nad83, Center),
            Err(UsngError::DataMapping)
        );
    }
    for text in [
        "18S UJ",
        "18S UJ 2348 0647",
        "18S UJ (NAD27)",
        "18S UJ (nad 27)",
        "18S UJ  (NAD 27)",
        "18S UJ (NAD 27) ",
        "18S UJ (NAD 83)",
    ] {
        assert_eq!(
            parse_usng(text, UsngDatum::Nad27, Center),
            Err(UsngError::DataMapping)
        );
    }
    assert_eq!(
        parse_usng(&"X".repeat(28), UsngDatum::Nad83, Center),
        Err(UsngError::ResourceLimit)
    );
    assert_eq!(
        format_usng(0.0, 0.0, 6, UsngDatum::Nad83),
        Err(UsngError::InvalidConfiguration)
    );
    for (lon, lat) in [
        (f64::NAN, 0.0),
        (0.0, f64::INFINITY),
        (180.0_f64.next_up(), 0.0),
        (0.0, 90.0_f64.next_up()),
    ] {
        assert_eq!(
            format_usng(lon, lat, 5, UsngDatum::Nad83),
            Err(UsngError::DataMapping)
        );
    }
}
