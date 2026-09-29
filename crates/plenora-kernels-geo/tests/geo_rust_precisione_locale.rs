//! La precisione dichiarata (1 cm a terra) nel noding di `polygonize`
//! (e quindi di `split`): i controesempi delle revisioni, ciascuno oltre il
//! centimetro e silenzioso prima della correzione.
//!
//! - Il noding arrotondava in `f64` il punto d'incrocio: a `2^52`
//!   `(B + 1.5, B + 1.5)` diventava `(B + 2, B + 2)`, 0.707 m fuori da uno
//!   dei segmenti.
//! - Oltre `ulp(max |coordinata|) > p / 64` nessun punto calcolato puo'
//!   restare entro la precisione: il kernel non calcola.
//!
//! Il controesempio di `split` (un buco omesso dall'output) riguarda i
//! controlli a posteriori, privati: e' nei test di `rust_backend::split`.
//!
//! Qui le coordinate sono metri e la precisione e' 1 cm.

use geo::{Coord, Geometry, LineString, MultiLineString};
use plenora_kernels_geo::rust_backend::polygonize::{
    polygonize_linework_rust, PolygonizeError, PolygonizeLimits, PolygonizeOptions,
};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::rust_backend::{polygonize_linework, RustBackendError};

/// 1 cm in metri.
const CENTIMETRO: f64 = 0.01;

fn precisione() -> Precision {
    Precision::new(CENTIMETRO).expect("precisione")
}

fn diagonali(base: f64) -> Geometry<f64> {
    Geometry::MultiLineString(MultiLineString::new(vec![
        LineString::from(vec![(base, base), (base + 3.0, base + 3.0)]),
        LineString::from(vec![(base, base + 3.0), (base + 3.0, base)]),
    ]))
}

/// A `B = 2^52` l'incrocio esatto `(B + 1.5, B + 1.5)` non e'
/// rappresentabile: arrotondato a `(B + 2, B + 2)` dista 0.707 m dal
/// secondo segmento. Il noding si rifiuta, sia nel kernel sia
/// nell'adapter.
#[test]
fn noding_arrotondato_oltre_il_centimetro_e_un_errore() {
    let linee = diagonali(2_f64.powi(52));
    let kernel = polygonize_linework_rust(
        &linee,
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: PolygonizeLimits::unlimited(),
            precision: CENTIMETRO,
        },
    );
    assert_eq!(kernel, Err(PolygonizeError::PrecisionInsufficient));
    assert!(matches!(
        polygonize_linework(&linee, true, false, 100, 1_000, 100, 100, precisione()),
        Err(RustBackendError::PrecisionInsufficient)
    ));
}

/// Controprova: a `2^30` lo stesso incrocio e' esatto, e il noding divide i
/// due segmenti in quattro linee residue.
#[test]
fn noding_esatto_lontano_dall_origine_resta() {
    let base = 2_f64.powi(30);
    let risultato = polygonize_linework(
        &diagonali(base),
        true,
        false,
        100,
        1_000,
        100,
        100,
        precisione(),
    )
    .expect("polygonize");
    assert!(risultato.polygons.is_empty());
    let incrocio = Coord {
        x: base + 1.5,
        y: base + 1.5,
    };
    let linee = risultato
        .cut_edges
        .iter()
        .chain(&risultato.dangles)
        .collect::<Vec<_>>();
    assert_eq!(linee.len(), 4);
    assert!(linee.iter().all(|linea| linea.0.contains(&incrocio)));
}

/// La precisione e' un argomento esplicito anche del polygonize: non
/// finita o non positiva e' un errore, prima di toccare i dati.
#[test]
fn precisione_del_polygonize_non_valida_e_un_errore() {
    for precision in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            polygonize_linework_rust(
                &diagonali(0.0),
                PolygonizeOptions {
                    node_input: true,
                    require_complete: false,
                    limits: PolygonizeLimits::unlimited(),
                    precision,
                },
            ),
            Err(PolygonizeError::InvalidPrecision)
        );
    }
}

/// Spaziatura delle coordinate: a `2^40` l'unita' in ultima posizione
/// (`2^-12` m) supera `1 cm / 64`, e il polygonize non calcola nemmeno un
/// incrocio esatto; a `2^39` (`2^-13` m) si'.
#[test]
fn coordinate_troppo_rade_per_la_precisione_sono_un_errore() {
    assert!(matches!(
        polygonize_linework(
            &diagonali(2_f64.powi(40)),
            true,
            false,
            100,
            1_000,
            100,
            100,
            precisione()
        ),
        Err(RustBackendError::PrecisionInsufficient)
    ));
    let risultato = polygonize_linework(
        &diagonali(2_f64.powi(39)),
        true,
        false,
        100,
        1_000,
        100,
        100,
        precisione(),
    )
    .expect("polygonize");
    assert_eq!(risultato.cut_edges.len() + risultato.dangles.len(), 4);
}
