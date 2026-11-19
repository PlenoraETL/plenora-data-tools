#![deny(clippy::disallowed_methods)]
//! C1: GEOMETRY-OPERATIONS-1.0 GOP-021..025; `NGA.STND.0037_2.0.0` (2014),
//! capitoli 2..5, appendici A/B; FGDC-STD-011-2001 (2001), figure 1/3, annesso E.
//! Le attese analitiche e gli esempi di griglia citano la fonte accanto al vettore.

use plenora_kernels_geo::{
    coordinate_api::*,
    grid_text::{CellRepresentative, GarsResolution},
};

const fn request(format: CoordinateFormat, options: &[CoordinateOption]) -> CoordinateRequest<'_> {
    CoordinateRequest {
        profile: STANDARD_V1,
        format,
        options,
    }
}

fn options(format: CoordinateFormat, direction: CoordinateDirection) -> Vec<CoordinateOption> {
    use CoordinateFormat::{Dd, Ddm, Dms, Gars, GeoRef, Mgrs, Usng, Utm};
    use CoordinateOption::{Datum, Precision, Representative, Resolution};
    let datum = if format == Usng {
        CoordinateDatum::Nad83
    } else {
        CoordinateDatum::Wgs84
    };
    let mut result = vec![Datum(datum)];
    match (direction, format) {
        (CoordinateDirection::Encode, Dd | Ddm | Dms | GeoRef) => result.push(Precision(2)),
        (CoordinateDirection::Encode, Mgrs | Usng) => result.push(Precision(5)),
        (CoordinateDirection::Encode, Utm) => {
            result.push(Resolution(CoordinateResolution::Metres(1)));
        }
        (CoordinateDirection::Encode, Gars) => result.push(Resolution(CoordinateResolution::Gars(
            GarsResolution::Minutes30,
        ))),
        (CoordinateDirection::Decode, Mgrs | Usng | Gars | GeoRef) => {
            result.push(Representative(CellRepresentative::Southwest));
        }
        (CoordinateDirection::Decode, Dd | Ddm | Dms | Utm) => {}
    }
    result
}

const fn point(longitude: f64, latitude: f64) -> CoordinatePoint {
    CoordinatePoint {
        longitude,
        latitude,
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn normative_vectors_both_directions() -> Result<(), Box<dyn std::error::Error>> {
    use CoordinateFormat::{Dd, Ddm, Dms, Gars, GeoRef, Mgrs, Usng, Utm};
    for (format, input, expected, decoded, tolerance) in [
        // NGA cap.5: grado=60 minuti=3600 secondi; GOP-022 coppia canonica.
        (
            Dd,
            point(-12.25, 45.5),
            "45.50N 12.25W",
            point(-12.25, 45.5),
            0.0,
        ),
        (
            Ddm,
            point(-12.25, 45.5),
            "45 30.00N 12 15.00W",
            point(-12.25, 45.5),
            0.0,
        ),
        (
            Dms,
            point(-12.25, 45.5),
            "45 30 00.00N 12 15 00.00W",
            point(-12.25, 45.5),
            0.0,
        ),
        // NGA 2-3, tabella 1: origine UTM; GOP-023 serializzazione del punto.
        (
            Utm,
            point(3.0, 0.0),
            "31 N 500000 0",
            point(3.0, 0.0),
            1e-12,
        ),
        // NGA tabelle 1/4/5: origine del fuso 31, quadrato EA.
        (
            Mgrs,
            point(3.0, 0.0),
            "31NEA0000000000",
            point(3.0, 0.0),
            1e-12,
        ),
        // FGDC figura 3: origine UTM e griglia EA; annesso E, NAD83.
        (
            Usng,
            point(3.0, 0.0),
            "31N EA 00000 00000",
            point(3.0, 0.0),
            1e-12,
        ),
        // NGA 4-2 a/b/c: cella 006AG, sudovest -177.5/-87.
        (
            Gars,
            point(-177.25, -86.75),
            "006AG",
            point(-177.5, -87.0),
            0.0,
        ),
        // NGA 4-3 e: Salisbury Cathedral MK PG 12 04; GOP-023 compatta.
        (
            GeoRef,
            point(-1.79, 51.075),
            "MKPG1204",
            point(-1.8, 3064.0 / 60.0),
            1e-14,
        ),
    ] {
        let enc = options(format, CoordinateDirection::Encode);
        let dec = options(format, CoordinateDirection::Decode);
        assert_eq!(encode_coordinate(request(format, &enc), input)?, expected);
        let result = decode_coordinate(request(format, &dec), expected)?;
        assert!((result.point.longitude - decoded.longitude).abs() <= tolerance);
        assert!((result.point.latitude - decoded.latitude).abs() <= tolerance);
        assert_eq!(
            encode_coordinates(request(format, &enc), &[input, input])?,
            vec![expected, expected]
        );
        assert_eq!(
            decode_coordinates(request(format, &dec), &[expected, expected])?,
            vec![result, result]
        );
        assert_eq!(
            result.cell.is_some(),
            matches!(format, Mgrs | Usng | Gars | GeoRef)
        );
        if let Some(cell) = result.cell {
            assert_eq!(cell.representative, CellRepresentative::Southwest);
        }
    }
    Ok(())
}

#[test]
#[deny(clippy::disallowed_methods)]
fn enumeration_and_every_inapplicable_option() {
    use CoordinateOptionKind::{Datum, Precision, Representative, Resolution};
    let formats = ["DD", "DMS", "DDM", "UTM", "MGRS", "USNG", "GARS", "GeoRef"];
    let keys = [Precision, Resolution, Representative, Datum];
    assert_eq!(CoordinateOptionKind::ALL, keys);
    assert_eq!(CoordinateFormat::ALL.len(), formats.len());
    assert_eq!(FORMAT_RULES.len(), formats.len());
    for (format_name, rule) in formats.into_iter().zip(FORMAT_RULES) {
        assert_eq!(format_name.parse::<CoordinateFormat>(), Ok(rule.format));
        assert_eq!(rule.format.rule(), *rule);
        assert_eq!(rule.encode.len(), keys.len());
        assert_eq!(rule.decode.len(), keys.len());
        assert_eq!(rule.defaults, [None; 4]);
        for direction in [CoordinateDirection::Encode, CoordinateDirection::Decode] {
            let valid = options(rule.format, direction);
            let requirements = match direction {
                CoordinateDirection::Encode => rule.encode,
                CoordinateDirection::Decode => rule.decode,
            };
            for (key, policy) in keys.into_iter().zip(requirements) {
                let value = match key {
                    Precision => CoordinateOption::Precision(2),
                    Resolution => CoordinateOption::Resolution(CoordinateResolution::Metres(1)),
                    Representative => CoordinateOption::Representative(CellRepresentative::Center),
                    Datum => CoordinateOption::Datum(CoordinateDatum::Wgs84),
                };
                let mut invalid = valid.clone();
                match policy {
                    OptionRequirement::Forbidden => invalid.push(value),
                    OptionRequirement::Required => invalid.retain(|v| v.kind() != key),
                }
                let req = request(rule.format, &invalid);
                let result = match direction {
                    CoordinateDirection::Encode => encode_coordinates(req, &[]).map(|_| ()),
                    CoordinateDirection::Decode => decode_coordinates(req, &[]).map(|_| ()),
                };
                assert_eq!(
                    result,
                    Err(CoordinateApiError {
                        kind: CoordinateErrorKind::InvalidConfiguration,
                        index: None
                    })
                );
            }
            let mut duplicate = valid.clone();
            duplicate.push(valid[0]);
            let req = request(rule.format, &duplicate);
            let result = match direction {
                CoordinateDirection::Encode => encode_coordinates(req, &[]).map(|_| ()),
                CoordinateDirection::Decode => decode_coordinates(req, &[]).map(|_| ()),
            };
            assert_eq!(
                result,
                Err(CoordinateApiError {
                    kind: CoordinateErrorKind::InvalidConfiguration,
                    index: None
                })
            );
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn datums_profiles_and_option_values_are_closed() {
    // GOP-021/025: esclusivita' datum e opzioni, validazione anche senza elementi.
    for &format in CoordinateFormat::ALL {
        for direction in [CoordinateDirection::Encode, CoordinateDirection::Decode] {
            for datum in [
                CoordinateDatum::Wgs84,
                CoordinateDatum::Nad83,
                CoordinateDatum::Nad27,
            ] {
                let mut config = options(format, direction);
                config[0] = CoordinateOption::Datum(datum);
                let req = request(format, &config);
                let actual = match direction {
                    CoordinateDirection::Encode => encode_coordinates(req, &[]).map(|_| ()),
                    CoordinateDirection::Decode => decode_coordinates(req, &[]).map(|_| ()),
                };
                let expected = if (format == CoordinateFormat::Usng
                    && datum != CoordinateDatum::Wgs84)
                    || (format != CoordinateFormat::Usng && datum == CoordinateDatum::Wgs84)
                {
                    Ok(())
                } else {
                    Err(CoordinateApiError {
                        kind: CoordinateErrorKind::Crs,
                        index: None,
                    })
                };
                assert_eq!(actual, expected);
            }
            let config = options(format, direction);
            let req = CoordinateRequest {
                profile: "unknown-private-value",
                ..request(format, &config)
            };
            let actual = match direction {
                CoordinateDirection::Encode => encode_coordinates(req, &[]).map(|_| ()),
                CoordinateDirection::Decode => decode_coordinates(req, &[]).map(|_| ()),
            };
            assert_eq!(
                actual,
                Err(CoordinateApiError {
                    kind: CoordinateErrorKind::InvalidConfiguration,
                    index: None
                })
            );
        }
        let expected_precision = match format {
            CoordinateFormat::Dd | CoordinateFormat::Ddm | CoordinateFormat::Dms => Some((0, 8)),
            CoordinateFormat::Mgrs | CoordinateFormat::Usng => Some((0, 5)),
            CoordinateFormat::GeoRef => Some((2, 5)),
            CoordinateFormat::Utm | CoordinateFormat::Gars => None,
        };
        assert_eq!(format.rule().precision, expected_precision);
        if let Some((min, max)) = expected_precision {
            for digits in 0..=u8::MAX {
                let config = [
                    options(format, CoordinateDirection::Encode)[0],
                    CoordinateOption::Precision(digits),
                ];
                let actual = encode_coordinates(request(format, &config), &[]).map(|_| ());
                let expected = if (min..=max).contains(&digits) {
                    Ok(())
                } else {
                    Err(CoordinateApiError {
                        kind: CoordinateErrorKind::InvalidConfiguration,
                        index: None,
                    })
                };
                assert_eq!(actual, expected);
            }
        }
    }
    assert_eq!(
        "unknown-private-value".parse::<CoordinateDatum>(),
        Err(CoordinateErrorKind::Crs.into())
    );
    assert_eq!(
        "unknown-private-value".parse::<CoordinateFormat>(),
        Err(CoordinateErrorKind::InvalidConfiguration.into())
    );
    assert_eq!(
        "unknown-private-value".parse::<CoordinateOptionKind>(),
        Err(CoordinateErrorKind::InvalidConfiguration.into())
    );
    for (format, resolution) in [
        (CoordinateFormat::Gars, CoordinateResolution::Metres(1)),
        (
            CoordinateFormat::Utm,
            CoordinateResolution::Gars(GarsResolution::Minutes30),
        ),
        (CoordinateFormat::Utm, CoordinateResolution::Metres(0)),
        (CoordinateFormat::Utm, CoordinateResolution::Metres(2)),
    ] {
        let config = [
            CoordinateOption::Datum(CoordinateDatum::Wgs84),
            CoordinateOption::Resolution(resolution),
        ];
        assert_eq!(
            encode_coordinates(request(format, &config), &[]),
            Err(CoordinateErrorKind::InvalidConfiguration.into())
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn atomic_limits_and_private_errors() -> Result<(), Box<dyn std::error::Error>> {
    // GOP-025: limite inclusivo di byte e cardinalita', nessuna sostituzione null.
    for &format in CoordinateFormat::ALL {
        let enc = options(format, CoordinateDirection::Encode);
        let dec = options(format, CoordinateDirection::Decode);
        let valid = encode_coordinate(request(format, &enc), point(3.0, 0.0))?;
        for text in [
            "x".repeat(256),
            "é".repeat(128),
            "x".repeat(257),
            "é".repeat(129),
        ] {
            let kind = if text.len() > MAX_TEXT_BYTES {
                CoordinateErrorKind::ResourceLimit
            } else {
                CoordinateErrorKind::DataMapping
            };
            let error = CoordinateApiError {
                kind,
                index: Some(1),
            };
            assert_eq!(
                decode_coordinates(request(format, &dec), &[&valid, &text, &valid]),
                Err(error)
            );
            assert_eq!(
                decode_coordinate(request(format, &dec), &text),
                Err(CoordinateApiError { kind, index: None })
            );
        }
        for invalid in [
            point(f64::NAN, 0.0),
            point(0.0, f64::INFINITY),
            point(181.0, 0.0),
        ] {
            assert_eq!(
                encode_coordinates(request(format, &enc), &[point(3.0, 0.0), invalid]),
                Err(CoordinateApiError {
                    kind: CoordinateErrorKind::DataMapping,
                    index: Some(1)
                })
            );
        }
        assert_eq!(decode_coordinates(request(format, &dec), &[])?, vec![]);
        assert_eq!(
            encode_coordinates(request(format, &enc), &[])?,
            Vec::<String>::new()
        );
        let error = decode_coordinates(request(format, &dec), &[&valid, "SECRET-ROW-0123"])
            .expect_err("private input rejected");
        assert_eq!(error.to_string(), "data_mapping");
        assert_eq!(
            format!("{error:?}"),
            "CoordinateApiError { kind: DataMapping, index: Some(1) }"
        );
    }
    let enc = options(CoordinateFormat::Dd, CoordinateDirection::Encode);
    let dec = options(CoordinateFormat::Dd, CoordinateDirection::Decode);
    let points = vec![point(3.0, 0.0); MAX_BATCH_ITEMS];
    let texts = vec!["0.00N 3.00E"; MAX_BATCH_ITEMS];
    assert_eq!(
        encode_coordinates(request(CoordinateFormat::Dd, &enc), &points)?.len(),
        MAX_BATCH_ITEMS
    );
    assert_eq!(
        decode_coordinates(request(CoordinateFormat::Dd, &dec), &texts)?.len(),
        MAX_BATCH_ITEMS
    );
    assert_eq!(
        encode_coordinates(
            request(CoordinateFormat::Dd, &enc),
            &vec![point(3.0, 0.0); MAX_BATCH_ITEMS + 1]
        ),
        Err(CoordinateErrorKind::ResourceLimit.into())
    );
    assert_eq!(
        decode_coordinates(
            request(CoordinateFormat::Dd, &dec),
            &vec!["0N 3E"; MAX_BATCH_ITEMS + 1]
        ),
        Err(CoordinateErrorKind::ResourceLimit.into())
    );
    Ok(())
}

#[test]
#[deny(clippy::disallowed_methods)]
fn cell_extents_representatives_and_nad27() -> Result<(), Box<dyn std::error::Error>> {
    // FGDC annesso E: stessa griglia NAD27, suffisso obbligatorio; nessuno shift.
    let enc = [
        CoordinateOption::Datum(CoordinateDatum::Nad27),
        CoordinateOption::Precision(5),
    ];
    let dec = [
        CoordinateOption::Datum(CoordinateDatum::Nad27),
        CoordinateOption::Representative(CellRepresentative::Southwest),
    ];
    assert_eq!(
        encode_coordinate(request(CoordinateFormat::Usng, &enc), point(3.0, 0.0))?,
        "31N EA 00000 00000 (NAD 27)"
    );
    let value = decode_coordinate(
        request(CoordinateFormat::Usng, &dec),
        "31N EA 00000 00000 (NAD 27)",
    )?;
    assert_eq!(value.datum, CoordinateDatum::Nad27);
    assert!((value.point.longitude - 3.0).abs() < 1e-12 && value.point.latitude.abs() < 1e-12);
    assert_eq!(
        decode_coordinate(request(CoordinateFormat::Usng, &dec), "31N EA 00000 00000"),
        Err(CoordinateErrorKind::DataMapping.into())
    );
    let nad83 = options(CoordinateFormat::Usng, CoordinateDirection::Decode);
    assert_eq!(
        decode_coordinate(
            request(CoordinateFormat::Usng, &nad83),
            "31N EA 00000 00000 (NAD 27)"
        ),
        Err(CoordinateErrorKind::DataMapping.into())
    );
    // NGA 4-2/4-3 e figure B-1/B-2: extent sorgente invariato al rappresentante.
    for (format, text, expected) in [
        (
            CoordinateFormat::Mgrs,
            "31NEA0000000000",
            (500_000, 0, 500_001, 1),
        ),
        (
            CoordinateFormat::Usng,
            "31N EA 00000 00000",
            (500_000, 0, 500_001, 1),
        ),
    ] {
        let mut dec = options(format, CoordinateDirection::Decode);
        let sw = decode_coordinate(request(format, &dec), text)?;
        dec[1] = CoordinateOption::Representative(CellRepresentative::Center);
        let center = decode_coordinate(request(format, &dec), text)?;
        let sw_cell = sw.cell.expect("cell");
        let center_cell = center.cell.expect("cell");
        assert_eq!(sw_cell.extent, center_cell.extent);
        let CoordinateExtent::Projected(cell) = center_cell.extent else {
            panic!("projected extent")
        };
        assert_eq!((cell.west, cell.south, cell.east, cell.north), expected);
        assert_eq!(center_cell.representative, CellRepresentative::Center);
        assert!(
            center.point.longitude > sw.point.longitude
                && center.point.latitude > sw.point.latitude
        );
    }
    Ok(())
}

#[test]
#[deny(clippy::disallowed_methods)]
fn angular_pair_grammar_and_rounding() -> Result<(), Box<dyn std::error::Error>> {
    // GOP-022: due assi, emisferi e separatori rigidi, zeri iniziali ammessi.
    let dec = options(CoordinateFormat::Dms, CoordinateDirection::Decode);
    for text in [
        "12 00 00E 45 00 00N",
        "45 00 00N  12 00 00E",
        "45 00 00N\t12 00 00E",
        "45 60 00N 12 00 00E",
        "45 00 00N 12 00 00E extra",
        "-45 00 00S 12 00 00E",
        "45 00 00N 12 00 00E ",
    ] {
        assert_eq!(
            decode_coordinate(request(CoordinateFormat::Dms, &dec), text),
            Err(CoordinateErrorKind::DataMapping.into())
        );
    }
    assert_eq!(
        decode_coordinate(
            request(CoordinateFormat::Dms, &dec),
            "045 00 00N 012 00 00E"
        )?
        .point,
        point(12.0, 45.0)
    );
    let enc = [
        CoordinateOption::Datum(CoordinateDatum::Wgs84),
        CoordinateOption::Precision(0),
    ];
    assert_eq!(
        encode_coordinate(request(CoordinateFormat::Dd, &enc), point(3.5, 2.5))?,
        "2N 4E"
    );
    assert_eq!(
        encode_coordinate(request(CoordinateFormat::Dms, &enc), point(179.99999, -0.0))?,
        "0 00 00N 180 00 00E"
    );
    Ok(())
}

#[test]
#[deny(clippy::disallowed_methods)]
fn geographic_cells_and_batch_order() -> Result<(), Box<dyn std::error::Error>> {
    // NGA 4-2 a/f/i: cella, quadrante sudovest e tasto 9 della riga sud.
    for (resolution, text, scale, west, south) in [
        (GarsResolution::Minutes30, "006AG", 2, -355, -174),
        (GarsResolution::Minutes15, "006AG3", 4, -710, -348),
        (GarsResolution::Minutes5, "006AG39", 12, -2128, -1044),
    ] {
        let enc = [
            CoordinateOption::Datum(CoordinateDatum::Wgs84),
            CoordinateOption::Resolution(CoordinateResolution::Gars(resolution)),
        ];
        for representative in [CellRepresentative::Center, CellRepresentative::Southwest] {
            let dec = [
                CoordinateOption::Datum(CoordinateDatum::Wgs84),
                CoordinateOption::Representative(representative),
            ];
            let decoded = decode_coordinate(request(CoordinateFormat::Gars, &dec), text)?;
            let cell = decoded.cell.expect("GARS extent");
            assert_eq!(cell.representative, representative);
            let CoordinateExtent::Geographic(extent) = cell.extent else {
                panic!("geographic extent")
            };
            assert_eq!(
                (
                    extent.ticks_per_degree,
                    extent.west,
                    extent.south,
                    extent.east,
                    extent.north
                ),
                (scale, west, south, west + 1, south + 1)
            );
            assert_eq!(
                encode_coordinate(request(CoordinateFormat::Gars, &enc), decoded.point)?,
                text
            );
        }
    }
    // NGA 4-3: intervalli di un minuto e sue suddivisioni decimali.
    for (precision, text, scale) in [
        (2, "NGAA3030", 60),
        (3, "NGAA300300", 600),
        (4, "NGAA30003000", 6000),
        (5, "NGAA3000030000", 60000),
    ] {
        let enc = [
            CoordinateOption::Datum(CoordinateDatum::Wgs84),
            CoordinateOption::Precision(precision),
        ];
        assert_eq!(
            encode_coordinate(request(CoordinateFormat::GeoRef, &enc), point(0.5, 0.5))?,
            text
        );
        for representative in [CellRepresentative::Center, CellRepresentative::Southwest] {
            let dec = [
                CoordinateOption::Datum(CoordinateDatum::Wgs84),
                CoordinateOption::Representative(representative),
            ];
            let result = decode_coordinate(request(CoordinateFormat::GeoRef, &dec), text)?;
            let cell = result.cell.expect("GeoRef extent");
            let CoordinateExtent::Geographic(extent) = cell.extent else {
                panic!("geographic extent")
            };
            assert_eq!(extent.ticks_per_degree, scale);
            assert_eq!(
                (extent.west, extent.south, extent.east, extent.north),
                (
                    i32::try_from(scale / 2)?,
                    i32::try_from(scale / 2)?,
                    i32::try_from(scale / 2 + 1)?,
                    i32::try_from(scale / 2 + 1)?
                )
            );
            assert_eq!(cell.representative, representative);
        }
    }
    // GOP-025: ordine e duplicati preservati, anche con segni diversi.
    let enc = options(CoordinateFormat::Dd, CoordinateDirection::Encode);
    let dec = options(CoordinateFormat::Dd, CoordinateDirection::Decode);
    let points = [
        point(12.25, 45.5),
        point(-12.25, -45.5),
        point(0.0, 0.0),
        point(12.25, 45.5),
    ];
    let texts = [
        "45.50N 12.25E",
        "45.50S 12.25W",
        "0.00N 0.00E",
        "45.50N 12.25E",
    ];
    assert_eq!(
        encode_coordinates(request(CoordinateFormat::Dd, &enc), &points)?,
        texts
    );
    let decoded = decode_coordinates(request(CoordinateFormat::Dd, &dec), &texts)?;
    assert_eq!(
        decoded
            .into_iter()
            .map(|value| value.point)
            .collect::<Vec<_>>(),
        points
    );
    Ok(())
}

#[test]
#[deny(clippy::disallowed_methods)]
fn option_order_cannot_change_error_category() {
    let a = CoordinateOption::Precision(9);
    let b = CoordinateOption::Datum(CoordinateDatum::Nad83);
    let point = CoordinatePoint {
        longitude: 0.0,
        latitude: 0.0,
    };
    for values in [[a, b], [b, a]] {
        assert_eq!(
            encode_coordinate(request(CoordinateFormat::Mgrs, &values), point),
            Err(CoordinateErrorKind::InvalidConfiguration.into())
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn batch_deadline_precedes_cancel_even_when_empty() {
    use std::sync::atomic::AtomicBool;
    let flag = AtomicBool::new(true);
    let limits = CoordinateLimits {
        max_items: 0,
        deadline: Some(std::time::Instant::now()),
    };
    let encode = options(CoordinateFormat::Dd, CoordinateDirection::Encode);
    let decode = options(CoordinateFormat::Dd, CoordinateDirection::Decode);
    assert_eq!(
        encode_coordinates_with_limits(
            request(CoordinateFormat::Dd, &encode),
            &[],
            limits,
            Some(&flag)
        ),
        Err(CoordinateErrorKind::DeadlineExceeded.into())
    );
    assert_eq!(
        decode_coordinates_with_limits(
            request(CoordinateFormat::Dd, &decode),
            &["0N 0E"],
            limits,
            Some(&flag)
        ),
        Err(CoordinateErrorKind::DeadlineExceeded.into())
    );
    assert_eq!(
        encode_coordinates_with_limits(
            request(CoordinateFormat::Dd, &encode),
            &[],
            CoordinateLimits::default(),
            Some(&flag)
        ),
        Err(CoordinateErrorKind::Cancelled.into())
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn angular_leading_zeros_are_an_explicit_grammar_exception() {
    for (format, text) in [
        (CoordinateFormat::Dms, "10 000 000.0N 020 000 000.0E"),
        (CoordinateFormat::Ddm, "10 000.0N 020 000.0E"),
    ] {
        let options = options(format, CoordinateDirection::Decode);
        let decoded = decode_coordinate(request(format, &options), text).unwrap();
        assert_eq!(
            decoded.point,
            CoordinatePoint {
                longitude: 20.0,
                latitude: 10.0
            }
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn codec_errors_keep_the_public_category_without_codec_details() {
    use plenora_kernels_geo::{
        coordinate_mgrs::MgrsError, coordinate_text::CoordinateTextError,
        coordinate_usng::UsngError, coordinate_utm::UtmError, grid_text::GridTextError,
    };
    use CoordinateErrorKind as Kind;
    // GOP-025, contratto coordinate_api: errori grammaticali entro 256 byte
    // sono data_mapping; la proiezione e' execution, mai unsupported o CRS.
    for (actual, expected) in [
        (
            Kind::from(CoordinateTextError::Precision),
            Kind::InvalidConfiguration,
        ),
        (Kind::from(CoordinateTextError::Domain), Kind::DataMapping),
        (Kind::from(CoordinateTextError::Syntax), Kind::DataMapping),
        (Kind::from(CoordinateTextError::Length), Kind::DataMapping),
        (Kind::from(UtmError::Precision), Kind::InvalidConfiguration),
        (Kind::from(UtmError::Projection), Kind::Execution),
        (Kind::from(UtmError::Domain), Kind::DataMapping),
        (Kind::from(UtmError::Syntax), Kind::DataMapping),
        (Kind::from(UtmError::Zone), Kind::DataMapping),
        (Kind::from(UtmError::Hemisphere), Kind::DataMapping),
        (Kind::from(UtmError::Length), Kind::DataMapping),
        (
            Kind::from(MgrsError::InvalidConfiguration),
            Kind::InvalidConfiguration,
        ),
        (Kind::from(MgrsError::Execution), Kind::Execution),
        (Kind::from(MgrsError::DataMapping), Kind::DataMapping),
        (Kind::from(MgrsError::ResourceLimit), Kind::DataMapping),
        (
            Kind::from(UsngError::InvalidConfiguration),
            Kind::InvalidConfiguration,
        ),
        (Kind::from(UsngError::Execution), Kind::Execution),
        (Kind::from(UsngError::Unsupported), Kind::Unsupported),
        (Kind::from(UsngError::DataMapping), Kind::DataMapping),
        (Kind::from(UsngError::ResourceLimit), Kind::DataMapping),
        (
            Kind::from(GridTextError::InvalidConfiguration),
            Kind::InvalidConfiguration,
        ),
        (Kind::from(GridTextError::DataMapping), Kind::DataMapping),
        (
            Kind::from(GridTextError::ResourceLimit),
            Kind::ResourceLimit,
        ),
    ] {
        assert_eq!(actual, expected);
        assert_eq!(CoordinateApiError::from(actual).index, None);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn custom_batch_limit_is_inclusive_in_both_directions() {
    let enc = options(CoordinateFormat::Dd, CoordinateDirection::Encode);
    let dec = options(CoordinateFormat::Dd, CoordinateDirection::Decode);
    let points = [point(3.0, 0.0), point(-3.0, 0.0)];
    let texts = ["0.00N 3.00E", "0.00N 3.00W"];
    for max_items in [0, 1, 2, MAX_BATCH_ITEMS + 1] {
        let limits = CoordinateLimits {
            max_items,
            deadline: None,
        };
        let encoded = encode_coordinates_with_limits(
            request(CoordinateFormat::Dd, &enc),
            &points,
            limits,
            None,
        );
        let decoded = decode_coordinates_with_limits(
            request(CoordinateFormat::Dd, &dec),
            &texts,
            limits,
            None,
        );
        // GOP-025: il limite riguarda l'intero batch, quindi nessun indice.
        if max_items < points.len() {
            assert_eq!(encoded, Err(CoordinateErrorKind::ResourceLimit.into()));
            assert_eq!(decoded, Err(CoordinateErrorKind::ResourceLimit.into()));
        } else {
            // NGA cap.5: zero latitudine, tre gradi est/ovest, ordine invariato.
            assert_eq!(encoded.unwrap(), texts);
            assert_eq!(
                decoded
                    .unwrap()
                    .iter()
                    .map(|value| value.point)
                    .collect::<Vec<_>>(),
                points
            );
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn public_names_are_case_sensitive_and_do_not_infer_aliases() {
    // GOP-021/025, standard_v1: nomi espliciti, nessun alias o normalizzazione.
    for (name, expected) in [
        ("WGS84", CoordinateDatum::Wgs84),
        ("NAD83", CoordinateDatum::Nad83),
        ("NAD27", CoordinateDatum::Nad27),
    ] {
        assert_eq!(name.parse::<CoordinateDatum>(), Ok(expected));
    }
    for (name, expected) in [
        ("precision", CoordinateOptionKind::Precision),
        ("resolution", CoordinateOptionKind::Resolution),
        ("representative", CoordinateOptionKind::Representative),
        ("datum", CoordinateOptionKind::Datum),
    ] {
        assert_eq!(name.parse::<CoordinateOptionKind>(), Ok(expected));
    }
    for name in ["wgs84", "WGS84 ", " NAD83"] {
        assert_eq!(
            name.parse::<CoordinateDatum>(),
            Err(CoordinateErrorKind::Crs.into())
        );
    }
    for name in ["Precision", "resolution ", ""] {
        assert_eq!(
            name.parse::<CoordinateOptionKind>(),
            Err(CoordinateErrorKind::InvalidConfiguration.into())
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn utm_and_usng_do_not_silently_switch_to_polar_grids() {
    // GOP-023/024 e FGDC figura 1: dominio UTM inclusivo [-80,84],
    // nessuna selezione automatica UPS. Coordinate globalmente invalide
    // restano data_mapping; coordinate polari valide sono unsupported.
    for format in [CoordinateFormat::Utm, CoordinateFormat::Usng] {
        let enc = options(format, CoordinateDirection::Encode);
        for (latitude, kind) in [
            (84.0_f64.next_up(), CoordinateErrorKind::Unsupported),
            ((-80.0_f64).next_down(), CoordinateErrorKind::Unsupported),
            (91.0, CoordinateErrorKind::DataMapping),
        ] {
            assert_eq!(
                encode_coordinate(request(format, &enc), point(3.0, latitude)),
                Err(kind.into())
            );
            assert_eq!(
                encode_coordinates(
                    request(format, &enc),
                    &[point(3.0, 0.0), point(3.0, latitude)]
                ),
                Err(CoordinateApiError {
                    kind,
                    index: Some(1)
                })
            );
        }
    }
    let enc = options(CoordinateFormat::Utm, CoordinateDirection::Encode);
    let dec = options(CoordinateFormat::Utm, CoordinateDirection::Decode);
    for latitude in [-80.0, 84.0] {
        let text =
            encode_coordinate(request(CoordinateFormat::Utm, &enc), point(3.0, latitude)).unwrap();
        let result = decode_coordinate(request(CoordinateFormat::Utm, &dec), &text).unwrap();
        // Il meridiano centrale resta 3 gradi; la quantizzazione a metri
        // e il collare GOP-024 ammettono <0.722 m, quindi <1e-5 gradi lat.
        assert!((result.point.longitude - 3.0).abs() < 1e-9);
        assert!((result.point.latitude - latitude).abs() < 1e-5);
        assert_eq!(result.cell, None);
        assert_eq!(result.datum, CoordinateDatum::Wgs84);
    }
}
