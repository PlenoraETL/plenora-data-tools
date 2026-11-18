#![deny(clippy::disallowed_methods)]
//! C1: `NGA.STND.0037_2.0.0_GRIDS` (2014-02-28), 4-2/4-3;
//! <https://geotimege.com/wp-content/uploads/2023/12/27.national-geospatial-intelligence-agency.pdf>
//! Convenzioni di prodotto: GEOMETRY-OPERATIONS-1.0 GOP-023/024/025.
use plenora_kernels_geo::grid_text::{
    format_gars, format_georef, parse_gars, parse_georef,
    CellRepresentative::{Center, Southwest},
    GarsResolution::{Minutes15, Minutes30, Minutes5},
    GridTextError::{DataMapping, InvalidConfiguration, ResourceLimit},
};

#[test]
#[deny(clippy::disallowed_methods)]
fn standard_examples_and_edges() -> Result<(), Box<dyn std::error::Error>> {
    for (text, resolution, west, south, scale) in [
        // NGA 4-2 a/b/c: esempio 006AG, origine -177.5, -87.
        ("006AG", Minutes30, -355, -174, 2),
        // NGA 4-2 f: esempio quadrante sudovest.
        ("006AG3", Minutes15, -710, -348, 4),
        // NGA 4-2 i: esempio key 9, est della riga sud.
        ("006AG39", Minutes5, -2128, -1044, 12),
        // GOP-023: estremi; NGA 4-2 b/c, prima e ultima cella.
        ("001AA", Minutes30, -360, -180, 2),
        ("720QZ", Minutes30, 359, 179, 2),
    ] {
        let cell = parse_gars(text, resolution, Southwest)?;
        assert_eq!(cell.extent.west, west);
        assert_eq!(cell.extent.south, south);
        assert_eq!(cell.extent.ticks_per_degree, scale);
        assert!(cell.longitude >= f64::from(west) / f64::from(scale));
        assert!(cell.latitude >= f64::from(south) / f64::from(scale));
        let center = parse_gars(text, resolution, Center)?;
        assert_eq!(
            format_gars(center.latitude, center.longitude, resolution)?,
            text
        );
    }
    // NGA 4-3 e: Salisbury Cathedral MK PG 12 04; GOP-023 senza spazi.
    let salisbury = parse_georef("MKPG1204", 2, Southwest)?;
    assert_eq!(salisbury.extent.west, -108);
    assert_eq!(salisbury.extent.south, 3064);
    assert_eq!(format_georef(51.075, -1.79, 2)?, "MKPG1204");
    // GOP-023/024: estensioni 0.1, 0.01, 0.001 minuti, troncamento.
    for (p, expected) in [
        (2, "NGAA3030"),
        (3, "NGAA300300"),
        (4, "NGAA30003000"),
        (5, "NGAA3000030000"),
    ] {
        assert_eq!(format_georef(0.5, 0.5, p)?, expected);
        let cell = parse_georef(expected, p, Southwest)?;
        assert_eq!((cell.latitude, cell.longitude), (0.5, 0.5));
    }
    // GOP-023: +180 e +90 ultima cella, -180 e -90 prima; NGA ordinamento.
    for (lat, lon, g30, g15, g5, georef) in [
        (
            -90.0,
            -180.0,
            "001AA",
            "001AA3",
            "001AA37",
            "AAAA0000000000".to_owned(),
        ),
        (
            90.0,
            180.0,
            "720QZ",
            "720QZ2",
            "720QZ23",
            "ZMQQ5999959999".to_owned(),
        ),
        (
            90.0,
            -180.0,
            "001QZ",
            "001QZ1",
            "001QZ11",
            "AMAQ0000059999".to_owned(),
        ),
        (
            -90.0,
            180.0,
            "720AA",
            "720AA4",
            "720AA49",
            "ZAQA5999900000".to_owned(),
        ),
    ] {
        for (r, expected) in [(Minutes30, g30), (Minutes15, g15), (Minutes5, g5)] {
            assert_eq!(format_gars(lat, lon, r)?, expected);
        }
        assert_eq!(format_georef(lat, lon, 5)?, georef);
    }
    Ok(())
}

#[test]
#[deny(clippy::disallowed_methods)]
fn quadrant_keypad_and_exact_boundary_ownership() -> Result<(), Box<dyn std::error::Error>> {
    // NGA 4-2 e/h: nordovest -> est, righe da nord a sud.
    for (lat, lon, quadrant) in [
        (0.375, 0.125, 1),
        (0.375, 0.375, 2),
        (0.125, 0.125, 3),
        (0.125, 0.375, 4),
    ] {
        assert_eq!(
            format_gars(lat, lon, Minutes15)?,
            format!("361HN{quadrant}")
        );
        for row in 0..3 {
            for col in 0..3 {
                let y = lat - 0.125 + f64::from(5 - 2 * row) / 24.0;
                let x = lon - 0.125 + f64::from(2 * col + 1) / 24.0;
                assert_eq!(
                    format_gars(y, x, Minutes5)?,
                    format!("361HN{quadrant}{}", 1 + row * 3 + col)
                );
            }
        }
    }
    // GOP-023/024: non sommare l'origine prima di troncare; vicini esatti di zero.
    for r in [Minutes30, Minutes15, Minutes5] {
        assert!(format_gars(-f64::from_bits(1), -f64::from_bits(1), r)?.starts_with("360HM"));
        assert!(format_gars(0.0, -0.0, r)?.starts_with("361HN"));
    }
    // GOP-023: confine esatto 0.5, e i due f64 adiacenti.
    for (v, expected) in [
        (0.5_f64.next_down(), "361HN"),
        (0.5, "362HP"),
        (0.5_f64.next_up(), "362HP"),
    ] {
        assert_eq!(format_gars(v, v, Minutes30)?, expected);
    }
    // GOP-024: 0.5 gradi = 30 minuti esatti, immediatamente sotto = 29.999 minuti.
    assert_eq!(
        format_georef(0.5_f64.next_down(), 0.5_f64.next_down(), 5)?,
        "NGAA2999929999"
    );
    assert_eq!(
        format_georef(-f64::from_bits(1), -f64::from_bits(1), 5)?,
        "MFQQ5999959999"
    );
    Ok(())
}

#[test]
#[deny(clippy::disallowed_methods)]
fn closed_grammar_and_typed_errors() {
    // NGA 4-2 c: prima lettera A..Q senza I/O, seconda A..Z senza I/O.
    for (position, illegal) in [(3, "IORSTUVWXYZ"), (4, "IO")] {
        for byte in illegal.bytes().chain(b'a'..=b'z') {
            let mut text = b"001AA".to_vec();
            text[position] = byte;
            let text = String::from_utf8_lossy(&text);
            assert_eq!(parse_gars(&text, Minutes30, Center), Err(DataMapping));
        }
    }
    // NGA 4-3 c/d: alfabeti specifici per tutte le quattro posizioni.
    for (position, illegal) in [
        (0, "IO"),
        (1, "INOPQRSTUVWXYZ"),
        (2, "IORSTUVWXYZ"),
        (3, "IORSTUVWXYZ"),
    ] {
        for byte in illegal.bytes().chain(b'a'..=b'z') {
            let mut text = b"AAAA0000".to_vec();
            text[position] = byte;
            assert_eq!(
                parse_georef(&String::from_utf8_lossy(&text), 2, Center),
                Err(DataMapping)
            );
        }
    }
    // GOP-024/025: solo grammatica canonica, nessuna normalizzazione.
    for text in [
        "", "000AA", "721AA", "01AA", "001AA ", " 001AA", "001Aé", "001A\n", "+01AA", "001AA0",
        "001AA5", "001AA10", "001AA1:",
    ] {
        for r in [Minutes30, Minutes15, Minutes5] {
            assert_eq!(parse_gars(text, r, Center), Err(DataMapping));
        }
    }
    for text in [
        "",
        "AAAA",
        "AAAA000",
        "AAAA00000",
        "AAAA6000",
        "AAAA0060",
        "AAAA-100",
        "AAAA00 0",
        "aaaa0000",
        "AAAA００",
        "AAAA00\n0",
    ] {
        assert_eq!(parse_georef(text, 2, Center), Err(DataMapping));
    }
    // GOP-025: limite inclusivo 256; 257 resource_limit.
    assert_eq!(
        parse_gars(&"A".repeat(256), Minutes30, Center),
        Err(DataMapping)
    );
    assert_eq!(
        parse_gars(&"A".repeat(257), Minutes30, Center),
        Err(ResourceLimit)
    );
    assert_eq!(
        parse_georef(&"A".repeat(257), 2, Center),
        Err(ResourceLimit)
    );
    for p in [0, 1, 6, 8, 255] {
        assert_eq!(format_georef(0.0, 0.0, p), Err(InvalidConfiguration));
        assert_eq!(
            parse_georef("AAAA0000", p, Center),
            Err(InvalidConfiguration)
        );
    }
    // GOP-025: non finiti, dominio chiuso senza avvolgimenti.
    for (lat, lon) in [
        (f64::NAN, 0.0),
        (0.0, f64::NAN),
        (f64::INFINITY, 0.0),
        (0.0, f64::NEG_INFINITY),
        (90.0_f64.next_up(), 0.0),
        ((-90.0_f64).next_down(), 0.0),
        (0.0, 180.0_f64.next_up()),
        (0.0, (-180.0_f64).next_down()),
    ] {
        assert_eq!(format_gars(lat, lon, Minutes30), Err(DataMapping));
        assert_eq!(format_georef(lat, lon, 2), Err(DataMapping));
    }
    // GOP-025: testo statico privo di valori del chiamante.
    assert_eq!(DataMapping.to_string(), "invalid grid coordinate");
    assert_eq!(
        InvalidConfiguration.to_string(),
        "invalid grid configuration"
    );
    assert_eq!(
        ResourceLimit.to_string(),
        "grid text resource limit exceeded"
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn cells_and_canonical_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
    // GOP-023/024: oracolo razionale indipendente su punti diadici interni.
    for i in -1440..1440 {
        let lon = f64::from(i) / 8.0;
        let lat = f64::from(i / 2) / 8.0;
        for p in 2..=5 {
            let text = format_georef(lat, lon, p)?;
            let cell = parse_georef(&text, p, Center)?;
            assert_eq!(text.len(), 4 + 2 * usize::from(p));
            assert!(text
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()));
            assert_eq!(format_georef(cell.latitude, cell.longitude, p)?, text);
            let sw = parse_georef(&text, p, Southwest)?;
            assert_eq!(format_georef(sw.latitude, sw.longitude, p)?, text);
            let scale = i * 60 * 10_i32.pow(u32::from(p - 2));
            assert_eq!(cell.extent.west, scale.div_euclid(8));
        }
        for r in [Minutes30, Minutes15, Minutes5] {
            let text = format_gars(lat, lon, r)?;
            let cell = parse_gars(&text, r, Center)?;
            let scale = i32::try_from(cell.extent.ticks_per_degree)?;
            assert_eq!(cell.extent.west, (i * scale).div_euclid(8));
            assert_eq!(cell.extent.south, ((i / 2) * scale).div_euclid(8));
            assert_eq!(format_gars(cell.latitude, cell.longitude, r)?, text);
            let sw = parse_gars(&text, r, Southwest)?;
            assert_eq!(format_gars(sw.latitude, sw.longitude, r)?, text);
        }
    }
    Ok(())
}
