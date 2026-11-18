#![deny(clippy::disallowed_methods)]
//! C1: T3 `NGA.STND.0037_2.0.0_GRIDS`, cap. 5 (2014-02-28), 5-1/5-2;
//! <https://geotimege.com/wp-content/uploads/2023/12/27.national-geospatial-intelligence-agency.pdf>
//! Scelte di prodotto: GEOMETRY-OPERATIONS-1.0 GOP-022/024/025.
use plenora_kernels_geo::coordinate_text::{
    format_coordinate, parse_coordinate,
    AngularNotation::{self, Dd, Ddm, Dms},
    CoordinateTextError::{Domain, Length, Precision, Syntax},
    GeographicAxis::{Latitude, Longitude},
};

#[test]
#[deny(clippy::disallowed_methods)]
fn analytic_written_expectations() {
    let cases = [
        // T3 5-1: 0.5 gradi = 30 minuti = 1800 secondi.
        (45.5, Latitude, Dd, 2, "45.50N"),
        // T3 5-1: 0.5 * 60 = 30.
        (45.5, Latitude, Ddm, 2, "45 30.00N"),
        // T3 5-1: 45 + 30/60 + 0/3600.
        (45.5, Latitude, Dms, 0, "45 30 00N"),
        // T3 5-1: sud; GOP-022: frazione decimale esplicita.
        (-0.0001, Latitude, Dd, 4, "0.0001S"),
        // T3 5-1: 0.0001 * 60 = 0.006 minuti sud.
        (-0.0001, Latitude, Ddm, 3, "0 00.006S"),
        // T3 5-1: 0.0001 * 3600 = 0.36 secondi sud.
        (-0.0001, Latitude, Dms, 2, "0 00 00.36S"),
        // GOP-022: riporto 59.964 secondi al grado 180.
        (179.99999, Longitude, Dms, 0, "180 00 00E"),
        // GOP-022: riporto 59.9994 minuti al grado 180.
        (179.99999, Longitude, Ddm, 2, "180 00.00E"),
        // GOP-022: riporto dei gradi decimali.
        (179.99999, Longitude, Dd, 4, "180.0000E"),
        // GOP-022: riporto e segno occidentale.
        (-179.99999, Longitude, Dms, 0, "180 00 00W"),
        // T3 5-1: polo nord esatto.
        (90.0, Latitude, Dms, 2, "90 00 00.00N"),
        // T3 5-1: polo sud esatto.
        (-90.0, Latitude, Ddm, 0, "90 00S"),
        // GOP-022: zero negativo canonico.
        (-0.0, Longitude, Dd, 0, "0E"),
        // GOP-022: zero arrotondato canonico.
        (-0.0001, Latitude, Dd, 0, "0N"),
        // GOP-022: meta' al pari, 2.5 -> 2.
        (2.5, Latitude, Dd, 0, "2N"),
        // GOP-022: meta' al pari, 3.5 -> 4.
        (-3.5, Longitude, Dd, 0, "4W"),
        // GOP-022: 0.125 gradi = 7.5 minuti -> 8.
        (0.125, Latitude, Ddm, 0, "0 08N"),
        // GOP-022: 0.03125 gradi = 112.5 secondi -> 112.
        (0.03125, Latitude, Dms, 0, "0 01 52N"),
        // GOP-022: 0.09375 gradi = 337.5 secondi -> 338.
        (0.09375, Latitude, Dms, 0, "0 05 38N"),
        // GOP-022: 59.9999 secondi arrotondati a due decimali.
        (59.9999 / 3600.0, Latitude, Dms, 2, "0 01 00.00N"),
        // GOP-022: precisione massima e padding.
        (45.5, Latitude, Dms, 8, "45 30 00.00000000N"),
        // GOP-022: minimo subnormale arrotondato a zero.
        (f64::from_bits(1), Latitude, Dd, 8, "0.00000000N"),
        // GOP-022: vicini binari della meta', nessun doppio arrotondamento.
        (2.5_f64.next_down(), Latitude, Dd, 0, "2N"),
        // GOP-022: vicino sopra la meta'.
        (2.5_f64.next_up(), Latitude, Dd, 0, "3N"),
    ];
    for (value, axis, notation, precision, expected) in cases {
        assert_eq!(
            format_coordinate(value, axis, notation, precision),
            Ok(expected.to_owned())
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn parsing_written_expectations() {
    for (text, axis, notation, expected) in [
        // T3 5-1: frazione decimale sud.
        ("0.0001S", Latitude, Dd, -0.0001_f64),
        // T3 5-1: 45 + 30/60.
        ("045 030.000N", Latitude, Ddm, 45.5),
        // T3 5-1: 45 + 30/60 + 0/3600, ovest.
        ("45 30 00W", Longitude, Dms, -45.5),
        // T3 5-1: 1 secondo = 1/3600 gradi.
        ("0 00 01N", Latitude, Dms, 1.0 / 3600.0),
        // T3 5-1: estremo comune dei meridiani.
        ("180E", Longitude, Dd, 180.0),
        // T3 5-1: polo nord.
        ("90 00 00N", Latitude, Dms, 90.0),
        // GOP-022: zero normalizzato positivo.
        ("0S", Latitude, Dd, 0.0),
        // GOP-022: massimo numero di cifre decimali.
        ("89.99999999N", Latitude, Dd, 89.999_999_99),
    ] {
        assert_eq!(
            parse_coordinate(text, axis, notation)
                .expect("valid analytic fixture")
                .to_bits(),
            expected.to_bits()
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn grammar_rejections() {
    // GOP-022/025: una voce per violazione della grammatica chiusa.
    for (text, notation, error) in [
        ("", Dd, Syntax),                    // Campo assente.
        ("N", Dd, Syntax),                   // Magnitudine assente.
        ("1", Dd, Syntax),                   // Emisfero assente.
        ("N1", Dd, Syntax),                  // Emisfero iniziale.
        ("1n", Dd, Syntax),                  // Emisfero minuscolo.
        ("1E", Dd, Syntax),                  // Asse incompatibile.
        ("-1N", Dd, Syntax),                 // Segno in conflitto.
        ("+1S", Dd, Syntax),                 // Segno in conflitto.
        ("-1S", Dd, Syntax),                 // Segno concordante vietato.
        ("+1N", Dd, Syntax),                 // Segno concordante vietato.
        ("1NS", Dd, Syntax),                 // Due emisferi.
        (" 1N", Dd, Syntax),                 // Spazio iniziale.
        ("1N ", Dd, Syntax),                 // Spazio finale.
        ("1 N", Dd, Syntax),                 // Spazio prima dell'emisfero.
        ("1  2N", Ddm, Syntax),              // Due spazi.
        ("1\t2N", Ddm, Syntax),              // Tab.
        ("1\n2N", Ddm, Syntax),              // Newline.
        ("1\u{a0}2N", Ddm, Syntax),          // Spazio Unicode.
        ("1°N", Dd, Syntax),                 // Simbolo dei gradi.
        ("1 2'N", Ddm, Syntax),              // Simbolo dei minuti.
        ("1 2 3\"N", Dms, Syntax),           // Simbolo dei secondi.
        ("1,2N", Dd, Syntax),                // Virgola decimale.
        ("1e1N", Dd, Syntax),                // Esponente.
        ("NaNN", Dd, Syntax),                // Non numero.
        ("infN", Dd, Syntax),                // Infinito.
        ("１N", Dd, Syntax),                 // Cifra non ASCII.
        ("1\0N", Dd, Syntax),                // Carattere estraneo.
        (".5N", Dd, Syntax),                 // Parte intera mancante.
        ("1.N", Dd, Syntax),                 // Frazione mancante.
        ("1.2.3N", Dd, Syntax),              // Due punti decimali.
        ("0000N", Dd, Syntax),               // Piu' di tre cifre intere.
        ("1.000000000N", Dd, Syntax),        // Piu' di otto decimali.
        ("1 2N", Ddm, Syntax),               // Minuti senza padding.
        ("1 02 3N", Dms, Syntax),            // Secondi senza padding.
        ("1N", Ddm, Syntax),                 // Minuti assenti.
        ("1 2N", Dms, Syntax),               // Secondi assenti.
        ("1 2 3N", Ddm, Syntax),             // Campo aggiuntivo.
        ("1.0 2N", Ddm, Syntax),             // Gradi frazionari.
        ("1 2.0 3N", Dms, Syntax),           // Minuti frazionari.
        ("1 60N", Ddm, Domain),              // Minuti >= 60.
        ("1 60 00N", Dms, Domain),           // Minuti >= 60.
        ("1 00 60N", Dms, Domain),           // Secondi >= 60.
        ("91N", Dd, Domain),                 // Latitudine oltre 90.
        ("90 00.00000001N", Ddm, Domain),    // Frazione oltre polo.
        ("90 00 00.00000001N", Dms, Domain), // Frazione oltre polo.
    ] {
        assert_eq!(parse_coordinate(text, Latitude, notation), Err(error));
    }
    // GOP-025: meridiano limite, nessun avvolgimento.
    assert_eq!(
        parse_coordinate("180 00 00.00000001E", Longitude, Dms),
        Err(Domain)
    );
    // GOP-025: limite byte prima dell'analisi, 32 byte restano sintassi invalida.
    assert_eq!(parse_coordinate(&"x".repeat(32), Latitude, Dd), Err(Syntax));
    // GOP-025: 33 byte rifiutati per lunghezza.
    assert_eq!(parse_coordinate(&"x".repeat(33), Latitude, Dd), Err(Length));
    // GOP-025: conta byte UTF-8, non caratteri.
    assert_eq!(parse_coordinate(&"°".repeat(17), Latitude, Dd), Err(Length));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn domain_and_precision_rejections() {
    // GOP-022/025: dominio controllato prima della quantizzazione.
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        90.0_f64.next_up(),
        -90.0_f64.next_up(),
    ] {
        assert_eq!(format_coordinate(value, Latitude, Dd, 0), Err(Domain));
    }
    // GOP-022/025: longitudine oltre il limite non normalizzata a 180.
    assert_eq!(
        format_coordinate(180.0_f64.next_up(), Longitude, Dms, 0),
        Err(Domain)
    );
    // GOP-022: precisione obbligatoria nel dominio 0..8.
    for decimals in [9, 255] {
        assert_eq!(
            format_coordinate(0.0, Latitude, Dd, decimals),
            Err(Precision)
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn deterministic_round_trip_bound() {
    for axis in [Latitude, Longitude] {
        for notation in [Dd, Ddm, Dms] {
            let units = match notation {
                Dd => 1.0,
                Ddm => 60.0,
                Dms => 3600.0,
            };
            for precision in 0..=8 {
                // GOP-022/024: mezzo passo + mezzo ULP per la lettura f64.
                let half_step = 0.5 / (10_f64.powi(i32::from(precision)) * units);
                for i in -720..=720 {
                    let value = f64::from(i) / 8.125;
                    let text = format_coordinate(value, axis, notation, precision)
                        .expect("valid coordinate");
                    let decoded = parse_coordinate(&text, axis, notation).expect("canonical text");
                    assert!(
                        (decoded - value).abs() <= f64::EPSILON.mul_add(value.abs(), half_step)
                    );
                    assert!(text.len() <= 32);
                }
            }
        }
    }
    // GOP-022/024: estremi e coordinate non diadiche, entrambi gli emisferi.
    for value in [-180.0, -179.99999, -0.0001, 0.0001, 179.99999, 180.0] {
        for notation in [AngularNotation::Dd, Ddm, Dms] {
            let text = format_coordinate(value, Longitude, notation, 8).expect("valid coordinate");
            let decoded = parse_coordinate(&text, Longitude, notation).expect("canonical text");
            assert!((decoded - value).abs() <= 1e-8);
        }
    }
}
