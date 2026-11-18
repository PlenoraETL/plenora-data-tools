#![deny(clippy::disallowed_methods)]
//! C1: `NGA.STND.0037_2.0.0_GRIDS` (2014-02-28), capitolo 3, appendici A/B/C.
//! <https://geotimege.com/wp-content/uploads/2023/12/27.national-geospatial-intelligence-agency.pdf>
//! Fixture indipendenti: `scripts/mgrs_oracle.py`, integrazione del raggio
//! meridiano WGS84 e stereografica ellissoidale, senza il codec sotto test.

use geographiclib_rs::{Geodesic, InverseGeodesic};
use plenora_core::crs::riproiezione::{ups::project_ups_wgs84, utm::project_utm_wgs84};
use plenora_kernels_geo::{
    coordinate_mgrs::{format_mgrs, parse_mgrs, MgrsError},
    grid_text::CellRepresentative::{Center, Southwest},
};

#[test]
#[deny(clippy::disallowed_methods)]
fn independent_numeric_oracle() {
    for line in include_str!("fixtures/mgrs-oracle.csv")
        .lines()
        .filter(|s| !s.starts_with('#'))
    {
        let values: Vec<_> = line.split(',').collect();
        let lon: f64 = values[0].parse().expect("fixture longitude");
        let lat: f64 = values[1].parse().expect("fixture latitude");
        let x: f64 = values[2].parse().expect("fixture easting");
        let y: f64 = values[3].parse().expect("fixture northing");
        let expected = values[4];
        assert_eq!(format_mgrs(lon, lat, 5).expect("oracle encoding"), expected);
        let prefix = expected.len() - 10;
        for digits in 0..=5_u8 {
            let width = usize::from(digits);
            let text = format!(
                "{}{}{}",
                &expected[..prefix],
                &expected[prefix..prefix + width],
                &expected[prefix + 5..prefix + 5 + width]
            );
            assert_eq!(format_mgrs(lon, lat, digits).expect("truncation"), text);
            let decoded = parse_mgrs(&text, Center).expect("oracle cell");
            let cell = decoded.cell;
            let side = 10_u32.pow(u32::from(5 - digits));
            assert_eq!(cell.east - cell.west, side);
            assert_eq!(cell.north - cell.south, side);
            assert!(x >= f64::from(cell.west) - 1e-8 && x < f64::from(cell.east));
            assert!(y >= f64::from(cell.south) - 1e-8 && y < f64::from(cell.north));
            let sw = parse_mgrs(&text, Southwest).expect("southwest cell");
            assert_eq!(sw.cell, cell);
            assert_eq!(sw.representative, Southwest);
            if digits == 5 {
                // Oracolo inverso: integrale meridiano e bisezione del raggio UPS.
                let expected_sw_lon: f64 = values[5].parse().expect("oracle southwest longitude");
                let expected_sw_lat: f64 = values[6].parse().expect("oracle southwest latitude");
                let expected_center_lon: f64 = values[7].parse().expect("oracle center longitude");
                let expected_center_lat: f64 = values[8].parse().expect("oracle center latitude");
                let sw_error: f64 = Geodesic::wgs84().inverse(
                    sw.latitude,
                    sw.longitude,
                    expected_sw_lat,
                    expected_sw_lon,
                );
                let center_error: f64 = Geodesic::wgs84().inverse(
                    decoded.latitude,
                    decoded.longitude,
                    expected_center_lat,
                    expected_center_lon,
                );
                assert!(sw_error < 0.001);
                assert!(center_error < 0.001);
            }
        }
        // Confronto numerico diretto con formule indipendenti, non round-trip.
        let projection = if prefix == 3 {
            project_ups_wgs84(lat > 0.0, false, lon, lat)
        } else {
            project_utm_wgs84(
                expected[..2].parse().expect("zone"),
                lat >= 0.0,
                false,
                lon,
                lat,
            )
        }
        .expect("projection");
        assert!((projection.0 - x).hypot(projection.1 - y) < 0.001);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn source_worked_examples_and_representatives() {
    // NGA 3-4 stampa UU, che in figura B-3 per WGS84 e' a 3300 km, fuori S.
    assert_eq!(
        parse_mgrs("18SUU8362601432", Center),
        Err(MgrsError::DataMapping)
    );
    // NGA 3-4: cifre dei raffinamenti; figura B-3 WGS84: UJ a 300km/4300km.
    // UU in 3-4 e' compatibile con lo schema alternativo C-4, non WGS84.
    for (text, west, south, side) in [
        ("18SUJ", 300_000, 4_300_000, 100_000),
        ("18SUJ80", 380_000, 4_300_000, 10_000),
        ("18SUJ8301", 383_000, 4_301_000, 1_000),
        ("18SUJ836014", 383_600, 4_301_400, 100),
        ("18SUJ83620143", 383_620, 4_301_430, 10),
        ("18SUJ8362601432", 383_626, 4_301_432, 1),
        // NGA B-5/B-6: esempi scalati pubblicati, non coordinate geografiche esatte.
        ("YXK3543", 1_735_000, 2_243_000, 1_000),
        ("45SXT4791", 647_000, 3_791_000, 1_000),
    ] {
        let value = parse_mgrs(text, Southwest).expect("NGA example");
        assert_eq!(
            (
                value.cell.west,
                value.cell.south,
                value.cell.east,
                value.cell.north
            ),
            (west, south, west + side, south + side)
        );
    }
    // NGA tabelle 1/4/5 e figure B-1/B-2: origini esatte delle tre griglie.
    for (text, lon, lat) in [
        ("31NEA0000000000", 3.0, 0.0),
        ("ZAH0000000000", 0.0, 90.0),
        ("BAN0000000000", 0.0, -90.0),
    ] {
        let value = parse_mgrs(text, Southwest).expect("origin");
        assert!((value.longitude - lon).abs() < 1e-12);
        assert!((value.latitude - lat).abs() < 1e-12);
        // GOP-024: il centro usa un offset di 0.5 m su entrambi gli assi.
        let center = parse_mgrs(text, Center).expect("center");
        assert_ne!(
            (center.longitude, center.latitude),
            (value.longitude, value.latitude)
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn zone_band_and_polar_boundaries() {
    // NGA B-1/B-2: lati degli assi UPS anche quando il falso est assorbe l'offset.
    assert!(format_mgrs(-f64::from_bits(1), 85.0, 5)
        .expect("western polar side")
        .starts_with('Y'));
    assert!(format_mgrs(-f64::from_bits(1), -85.0, 5)
        .expect("western polar side")
        .starts_with('A'));
    assert_eq!(
        format_mgrs(0.0, 90.0_f64.next_down(), 5),
        Ok("ZAG0000099999".into())
    );
    assert_eq!(
        format_mgrs(180.0, (-90.0_f64).next_up(), 5),
        Ok("BAM0000099999".into())
    );
    // NGA tabelle 6/7/8 e figura 1; GOP-023 proprieta' dei bordi.
    for (lon, lat, prefix) in [
        (-180.0, 0.0, "01N"),
        (180.0, 0.0, "60N"),
        (0.0_f64.next_down(), 0.0, "30N"),
        (0.0, 0.0, "31N"),
        (3.0_f64.next_down(), 56.0, "31V"),
        (3.0, 56.0, "32V"),
        (12.0, 56.0, "33V"),
        (9.0_f64.next_down(), 72.0, "31X"),
        (9.0, 72.0, "33X"),
        (21.0, 72.0, "35X"),
        (33.0, 72.0, "37X"),
        (42.0, 72.0, "38X"),
        (0.0, -80.0, "31C"),
        (0.0, 84.0, "31X"),
        (-90.0, (-80.0_f64).next_down(), "A"),
        (90.0, (-80.0_f64).next_down(), "B"),
        (-90.0, 84.0_f64.next_up(), "Y"),
        (90.0, 84.0_f64.next_up(), "Z"),
    ] {
        for digits in 0..=5 {
            let text = format_mgrs(lon, lat, digits).expect("boundary encoding");
            assert!(text.starts_with(prefix));
            let decoded = parse_mgrs(&text, Center).expect("boundary cell");
            assert_eq!(decoded.representative, Center);
        }
    }
    // GOP-023: limite sud dell'equatore, falso northing e ultimo metro della banda M.
    assert_eq!(
        format_mgrs(3.0, -f64::from_bits(1), 5),
        Ok("31MEV0000099999".into())
    );
    assert_eq!(
        parse_mgrs("31MEV0000099999", Center)
            .expect("southern equator")
            .cell
            .south,
        9_999_999
    );
    // NGA tabella 7: 31V termina a 3E, E=500000 e' gia' fuori dal fuso.
    assert_eq!(parse_mgrs("31VEC", Center), Err(MgrsError::DataMapping));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn strict_errors_without_caller_data() {
    // GOP-025: grammatica canonica ASCII e combinazioni inesistenti.
    for text in [
        "",
        "31",
        "31N",
        "EA",
        "31NE",
        "1NEA",
        "00NEA",
        "61NEA",
        "31IEA",
        "32XNA",
        "34XDA",
        "36XWA",
        "31NEI",
        "31NIA",
        "ZAA000",
        "ZAH0",
        "ZAH 0000",
        "zah0000",
        "31NEA１２",
        "31NEA+000",
        "31NEA000",
        "31NEA0000\n",
        "YAH",
        "ZZA",
        "BZZ",
        "YRA0000000000",
    ] {
        assert_eq!(
            parse_mgrs(text, Center),
            Err(MgrsError::DataMapping),
            "invalid fixture"
        );
    }
    assert_eq!(
        parse_mgrs(&"X".repeat(16), Center),
        Err(MgrsError::ResourceLimit)
    );
    assert_eq!(
        format_mgrs(0.0, 0.0, 6),
        Err(MgrsError::InvalidConfiguration)
    );
    for (lon, lat) in [
        (f64::NAN, 0.0),
        (0.0, f64::INFINITY),
        (180.0_f64.next_up(), 0.0),
        (0.0, 90.0_f64.next_up()),
    ] {
        assert_eq!(format_mgrs(lon, lat, 5), Err(MgrsError::DataMapping));
    }
    assert!(matches!(
        project_ups_wgs84(true, true, 1e308, 1e308),
        Err(plenora_core::crs::CrsError::ReprojectionNotConverged)
    ));
    assert!(matches!(
        project_ups_wgs84(false, true, 1e308, 1e308),
        Err(plenora_core::crs::CrsError::ReprojectionNotConverged)
    ));
}
