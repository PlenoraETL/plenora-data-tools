//! La precisione dichiarata (1 cm a terra) nelle operazioni che passano
//! dalla griglia intera di `i_overlay`: booleane, `dissolve`, `clip`,
//! `overlay`, `clean_topology`, `buffer`, `subdivide`, `coverage_validate`.
//!
//! Per ogni operazione, con coordinate in metri e 1 cm:
//!
//! - un'estensione di circa 1.300 km (l'Italia) passa: passo della griglia
//!   `2^-10` m, spostamento a priori `(1 + sqrt(2)) g` circa 2,4 mm;
//! - un'estensione di circa 20.000 km e' rifiutata con
//!   `PrecisionInsufficient`: passo `2^-6` m, 1,56 cm da solo;
//! - coordinate oltre la guardia di modulo (`2^45` m, dove la spaziatura
//!   dei `f64` supera `p / 64`) sono rifiutate anche con un'estensione di
//!   pochi metri.

// Le fixture si leggono come frazioni del lato: `origine + lato * f` e' la
// forma voluta, il valore esatto non conta.
#![allow(clippy::suboptimal_flops)]

use geo::{Area, Coord, Geometry, LineString, MultiPoint, Point, Polygon};
use plenora_core::PlenoraError;
use plenora_kernels_geo::extensions::ExtensionError;
use plenora_kernels_geo::extensions2::subdivide;
use plenora_kernels_geo::extensions3::{coverage_validate, DEFAULT_MAX_ISSUES};
use plenora_kernels_geo::operations::{buffer_with_cap, BufferCapStyle, OperationError};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::topology::{
    boolean_operation, boolean_operation_validated, clean_valid_polygon_topology, clip_to_mask,
    dissolve, polygon_overlay, BooleanOperation, OverlayMode, TopologyError,
};

fn centimetro() -> Precision {
    Precision::new(0.01).expect("precisione")
}

/// Le tre scale: (origine, lato) in metri.
const ITALIA: (f64, f64) = (300_000.0, 1_300_000.0);
const MONDO: (f64, f64) = (0.0, 20_000_000.0);
/// `2^45` m: `ulp = 2^-7` m, oltre `0.01 / 64`.
const LONTANO: (f64, f64) = (35_184_372_088_832.0, 10.0);

fn rettangolo(x0: f64, y0: f64, x1: f64, y1: f64) -> Geometry<f64> {
    Geometry::Polygon(Polygon::new(
        LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]),
        vec![],
    ))
}

/// Due rettangoli che si sovrappongono, nella scala data: il secondo
/// spostato di una frazione non rappresentabile sulla griglia, cosi' gli
/// incroci cadono fra i punti della griglia.
fn coppia((origine, lato): (f64, f64)) -> (Geometry<f64>, Geometry<f64>) {
    let sinistra = rettangolo(origine, origine, origine + lato, origine + lato * 0.6);
    let spostamento = lato * 0.371_337_1;
    let destra = rettangolo(
        origine + spostamento,
        origine + lato * 0.123_457,
        origine + lato * 0.987_654_3,
        origine + lato * 0.8,
    );
    (sinistra, destra)
}

/// Un poligono di molti vertici (per `subdivide`) nella scala data.
fn stella((origine, lato): (f64, f64), punte: usize) -> Geometry<f64> {
    let centro = Coord {
        x: origine + lato * 0.5,
        y: origine + lato * 0.5,
    };
    let mut anello = Vec::with_capacity(2 * punte + 1);
    for indice in 0..2 * punte {
        #[allow(clippy::cast_precision_loss)]
        let angolo = std::f64::consts::PI * indice as f64 / punte as f64;
        let raggio = if indice % 2 == 0 { 0.5 } else { 0.3 } * lato;
        anello.push((
            raggio.mul_add(angolo.cos(), centro.x),
            raggio.mul_add(angolo.sin(), centro.y),
        ));
    }
    anello.push(anello[0]);
    Geometry::Polygon(Polygon::new(LineString::from(anello), vec![]))
}

#[allow(clippy::needless_pass_by_value)] // l'esito si consuma nella chiamata
fn topologia_rifiutata<T: std::fmt::Debug>(esito: Result<T, TopologyError>, caso: &str) {
    assert!(
        matches!(esito, Err(TopologyError::PrecisionInsufficient)),
        "{caso}: atteso PrecisionInsufficient, ottenuto {esito:?}"
    );
}

const OPERAZIONI: [BooleanOperation; 4] = [
    BooleanOperation::Intersection,
    BooleanOperation::Union,
    BooleanOperation::Difference,
    BooleanOperation::SymmetricDifference,
];

#[test]
fn booleane_alle_tre_scale() {
    let (sinistra, destra) = coppia(ITALIA);
    for operazione in OPERAZIONI {
        let esito = boolean_operation(&sinistra, &destra, operazione, centimetro())
            .unwrap_or_else(|errore| panic!("{operazione:?} a 1.300 km: {errore}"));
        assert!(esito.unsigned_area() > 0.0, "{operazione:?}");
        assert_eq!(
            boolean_operation_validated(&sinistra, &destra, operazione, centimetro()).unwrap(),
            esito
        );
    }
    for scala in [MONDO, LONTANO] {
        let (sinistra, destra) = coppia(scala);
        for operazione in OPERAZIONI {
            topologia_rifiutata(
                boolean_operation(&sinistra, &destra, operazione, centimetro()),
                "booleana",
            );
            topologia_rifiutata(
                boolean_operation_validated(&sinistra, &destra, operazione, centimetro()),
                "booleana validated",
            );
        }
    }
}

#[test]
fn dissolve_clip_overlay_alle_tre_scale() {
    let (sinistra, destra) = coppia(ITALIA);
    let righe = [sinistra.clone(), destra.clone()];
    assert!(dissolve(&righe, centimetro()).is_ok());
    let tagliate = clip_to_mask(
        std::slice::from_ref(&sinistra),
        std::slice::from_ref(&destra),
        centimetro(),
    )
    .expect("clip a 1.300 km");
    assert!(tagliate[0].is_some());
    for modo in [
        OverlayMode::Intersection,
        OverlayMode::Union,
        OverlayMode::Identity,
        OverlayMode::SymmetricDifference,
    ] {
        let pezzi = polygon_overlay(
            std::slice::from_ref(&sinistra),
            std::slice::from_ref(&destra),
            modo,
            100,
            100,
            centimetro(),
        )
        .unwrap_or_else(|errore| panic!("{modo:?} a 1.300 km: {errore}"));
        assert!(!pezzi.is_empty(), "{modo:?}");
    }

    for scala in [MONDO, LONTANO] {
        let (sinistra, destra) = coppia(scala);
        let righe = [sinistra.clone(), destra.clone()];
        topologia_rifiutata(dissolve(&righe, centimetro()), "dissolve");
        topologia_rifiutata(
            clip_to_mask(
                std::slice::from_ref(&sinistra),
                std::slice::from_ref(&destra),
                centimetro(),
            ),
            "clip",
        );
        for modo in [OverlayMode::Intersection, OverlayMode::Union] {
            topologia_rifiutata(
                polygon_overlay(
                    std::slice::from_ref(&sinistra),
                    std::slice::from_ref(&destra),
                    modo,
                    100,
                    100,
                    centimetro(),
                ),
                "overlay",
            );
        }
    }
}

#[test]
fn clean_topology_alle_tre_scale() {
    let righe: [Geometry<f64>; 2] = coppia(ITALIA).into();
    // Morfologia (buffer +-1 m) e rimozione delle sovrapposizioni.
    let pulite = clean_valid_polygon_topology(&righe, 1.0, true, true, 10, 1_000, centimetro())
        .expect("clean a 1.300 km");
    assert!(pulite.iter().all(Option::is_some));
    for scala in [MONDO, LONTANO] {
        let righe: [Geometry<f64>; 2] = coppia(scala).into();
        for (fill_gaps, remove_overlaps) in [(true, false), (false, true), (true, true)] {
            topologia_rifiutata(
                clean_valid_polygon_topology(
                    &righe,
                    1.0,
                    remove_overlaps,
                    fill_gaps,
                    10,
                    1_000,
                    centimetro(),
                ),
                "clean",
            );
        }
    }
}

#[test]
fn buffer_alle_tre_scale() {
    let stili = [
        BufferCapStyle::Round,
        BufferCapStyle::Flat,
        BufferCapStyle::Square,
    ];
    let geometrie = |(origine, lato): (f64, f64)| {
        vec![
            coppia((origine, lato)).0,
            Geometry::LineString(LineString::from(vec![
                (origine, origine),
                (origine + lato * 0.5, origine + lato * 0.2),
                (origine + lato, origine + lato),
            ])),
            Geometry::MultiPoint(MultiPoint::new(vec![
                Point::new(origine, origine),
                Point::new(origine + lato, origine + lato * 0.5),
            ])),
        ]
    };
    for geometria in geometrie(ITALIA) {
        for stile in stili {
            for distanza in [1_000.0, -1_000.0, 0.0] {
                let esito = buffer_with_cap(&geometria, distanza, stile, centimetro());
                assert!(
                    esito.is_ok(),
                    "{stile:?} a {distanza} m su 1.300 km: {esito:?}"
                );
            }
        }
    }
    for scala in [MONDO, LONTANO] {
        for geometria in geometrie(scala) {
            for stile in stili {
                let esito = buffer_with_cap(&geometria, 1.0, stile, centimetro());
                assert!(
                    matches!(esito, Err(OperationError::PrecisionInsufficient)),
                    "{stile:?}: {esito:?}"
                );
            }
        }
    }
}

/// La griglia del buffer e' quella dell'ingresso allargato della
/// distanza: un punto (ingombro nullo) con una distanza di 10.000 km e'
/// rifiutato, con 100 km no.
#[test]
fn la_griglia_del_buffer_comprende_la_distanza() {
    let punti = Geometry::MultiPoint(MultiPoint::new(vec![
        Point::new(500_000.0, 4_000_000.0),
        Point::new(500_001.0, 4_000_000.0),
    ]));
    assert!(buffer_with_cap(&punti, 100_000.0, BufferCapStyle::Round, centimetro()).is_ok());
    assert!(matches!(
        buffer_with_cap(&punti, 10_000_000.0, BufferCapStyle::Round, centimetro()),
        Err(OperationError::PrecisionInsufficient)
    ));
}

/// Revisione: una parte dell'ingresso che la griglia di `i_overlay` riduce
/// a un punto sparisce dal buffer senza errore della dipendenza (un anello
/// d'area intera nulla e' saltato, una linea su un solo punto della griglia
/// non da' segmenti). Il suo buffer e' spesso `2 |d|`: errore esplicito.
#[test]
fn il_buffer_non_perde_parti_ridotte_a_un_punto() {
    let corta = Geometry::LineString(LineString::from(vec![
        (500_000.0, 4_000_000.0),
        (500_000.000_000_01, 4_000_000.0),
    ]));
    let sottile = Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (500_000.0, 4_000_000.0),
            (500_100.0, 4_000_000.0),
            (500_050.0, 4_000_000.000_000_02),
            (500_000.0, 4_000_000.0),
        ]),
        vec![],
    ));
    // Una linea di 0,4 mm accanto a una normale, a scala italiana.
    let componenti = Geometry::MultiLineString(geo::MultiLineString::new(vec![
        LineString::from(vec![(300_000.0, 4_000_000.0), (1_600_000.0, 4_000_000.0)]),
        LineString::from(vec![(900_000.0, 4_500_000.0), (900_000.000_4, 4_500_000.0)]),
    ]));
    for (caso, geometria) in [
        ("corta", &corta),
        ("sottile", &sottile),
        ("componenti", &componenti),
    ] {
        let esito = buffer_with_cap(geometria, 10.0, BufferCapStyle::Round, centimetro());
        assert!(
            matches!(esito, Err(OperationError::PrecisionInsufficient)),
            "{caso}: {esito:?}"
        );
    }
    topologia_rifiutata(
        clean_valid_polygon_topology(&[sottile], 0.5, false, true, 10, 1_000, centimetro()),
        "clean con morfologia",
    );
}

#[test]
fn subdivide_alle_tre_scale() {
    let parti = subdivide(&stella(ITALIA, 40), 16, centimetro()).expect("subdivide a 1.300 km");
    assert!(parti.len() > 1);
    let area: f64 = parti.iter().map(Area::unsigned_area).sum();
    let attesa = stella(ITALIA, 40).unsigned_area();
    assert!((area - attesa).abs() <= attesa * 1e-9);
    for scala in [MONDO, LONTANO] {
        let esito = subdivide(&stella(scala, 40), 16, centimetro());
        assert!(
            matches!(esito, Err(ExtensionError::PrecisionInsufficient)),
            "{esito:?}"
        );
    }
    // Sotto la soglia di vertici nessun overlay: la geometria passa
    // invariata anche lontano.
    let piccola = coppia(LONTANO).0;
    assert_eq!(
        subdivide(&piccola, 16, centimetro()).unwrap(),
        vec![piccola]
    );
}

#[test]
fn coverage_validate_alle_tre_scale() {
    let righe: [Geometry<f64>; 2] = coppia(ITALIA).into();
    let issues = coverage_validate(&righe, 0.0, DEFAULT_MAX_ISSUES, centimetro())
        .expect("coverage a 1.300 km");
    assert_eq!(issues.len(), 1);
    for scala in [MONDO, LONTANO] {
        let righe: [Geometry<f64>; 2] = coppia(scala).into();
        let esito = coverage_validate(&righe, 0.0, DEFAULT_MAX_ISSUES, centimetro());
        assert!(
            matches!(esito, Err(ExtensionError::PrecisionInsufficient)),
            "{esito:?}"
        );
    }
}

/// Il rifiuto e' `Unsupported`, come quello di `polygonize` e `split`, e
/// il messaggio non porta dati.
#[test]
fn il_rifiuto_e_unsupported_senza_dati() {
    let errore = ExtensionError::PrecisionInsufficient.del_passo("geo.subdivide");
    assert!(matches!(errore, PlenoraError::Unsupported(_)), "{errore:?}");
    assert_eq!(
        errore.to_string(),
        PlenoraError::Unsupported(
            "geo.subdivide: geometria troppo estesa per la precisione dichiarata".to_owned()
        )
        .to_string()
    );
    for messaggio in [
        TopologyError::PrecisionInsufficient.to_string(),
        OperationError::PrecisionInsufficient.to_string(),
    ] {
        assert_eq!(
            messaggio,
            "geometria troppo estesa per la precisione dichiarata"
        );
    }
}

mod senza_rifiuti_spuri {
    //! Il controllo a posteriori non rifiuta risultati corretti: stelle a
    //! scala italiana, centri e raggi casuali, tutte le booleane e il buffer.
    use super::*;
    use proptest::prelude::*;

    fn stella_in(cx: f64, cy: f64, raggio: f64, punte: usize, rotazione: f64) -> Geometry<f64> {
        let mut anello = Vec::with_capacity(2 * punte + 1);
        for indice in 0..2 * punte {
            #[allow(clippy::cast_precision_loss)]
            let angolo = std::f64::consts::PI.mul_add(indice as f64 / punte as f64, rotazione);
            let r = if indice % 2 == 0 { 1.0 } else { 0.45 } * raggio;
            anello.push((r.mul_add(angolo.cos(), cx), r.mul_add(angolo.sin(), cy)));
        }
        anello.push(anello[0]);
        Geometry::Polygon(Polygon::new(LineString::from(anello), vec![]))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        #[test]
        fn booleane_e_buffer_non_rifiutano_a_scala_italiana(
            ax in 300_000.0..1_000_000.0_f64,
            ay in 4_000_000.0..4_600_000.0_f64,
            bx in 300_000.0..1_000_000.0_f64,
            by in 4_000_000.0..4_600_000.0_f64,
            ra in 50_000.0..400_000.0_f64,
            rb in 50_000.0..400_000.0_f64,
            punte_a in 3_usize..12,
            punte_b in 3_usize..12,
            rotazione in 0.0..1.0_f64,
            distanza in -20_000.0..20_000.0_f64,
        ) {
            let sinistra = stella_in(ax, ay, ra, punte_a, 0.0);
            let destra = stella_in(bx, by, rb, punte_b, rotazione);
            for operazione in OPERAZIONI {
                let esito = boolean_operation(&sinistra, &destra, operazione, centimetro());
                prop_assert!(
                    !matches!(esito, Err(TopologyError::PrecisionInsufficient)),
                    "{:?}", operazione
                );
            }
            let esito = dissolve(&[sinistra.clone(), destra], centimetro());
            prop_assert!(!matches!(esito, Err(TopologyError::PrecisionInsufficient)));
            for stile in [BufferCapStyle::Round, BufferCapStyle::Flat, BufferCapStyle::Square] {
                let esito = buffer_with_cap(&sinistra, distanza, stile, centimetro());
                prop_assert!(
                    !matches!(esito, Err(OperationError::PrecisionInsufficient)),
                    "{:?}", stile
                );
                let linea = Geometry::LineString(LineString::from(vec![
                    (ax, ay), (bx, by), (ax + ra, by - rb),
                ]));
                let esito = buffer_with_cap(&linea, distanza.abs(), stile, centimetro());
                prop_assert!(
                    !matches!(esito, Err(OperationError::PrecisionInsufficient)),
                    "{:?} linea", stile
                );
            }
        }
    }
}
