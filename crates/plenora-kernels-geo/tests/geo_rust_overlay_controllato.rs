//! `make_valid` non perde area in silenzio.
//!
//! Due strade verso lo stesso difetto, trovate dalla seconda revisione:
//!
//! - un segno d'area non decidibile (coordinate fuori dal dominio esatto)
//!   tradotto in «area non positiva» avviava la riparazione di un poligono
//!   valido;
//! - l'overlay della riparazione passa per la griglia intera di `i_overlay`:
//!   una cornice piu' sottile della griglia collassava a vuoto, o ne restava
//!   meta', e il risultato superava la validazione finale.
//!
//! Qui ogni esito e' un errore esplicito o la geometria con l'area giusta.

use geo::{Area, Geometry, LineString, MultiPolygon, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_kernels_geo::geometry_from_wkb;
use plenora_kernels_geo::rust_backend::make_valid::{
    make_valid_geometry_rust, make_valid_geometry_rust_bounded,
    make_valid_geometry_rust_with_limits, MakeValidError, MakeValidLimits, RepairMethod,
};
use plenora_kernels_geo::rust_backend::{
    make_valid_wkb, RepairMethod as MetodoAdapter, RustBackendError,
};

fn quadrato(minimo: f64, massimo: f64) -> LineString<f64> {
    LineString::from(vec![
        (minimo, minimo),
        (massimo, minimo),
        (massimo, massimo),
        (minimo, massimo),
        (minimo, minimo),
    ])
}

fn cornice(lato: f64, margine: f64) -> Polygon<f64> {
    Polygon::new(quadrato(0.0, lato), vec![quadrato(margine, lato - margine)])
}

const LIMITI: MakeValidLimits = MakeValidLimits {
    max_input_coordinates: 1_000_000,
    max_noding_work: 1_000_000_000,
    max_output_geometries: 1_000_000,
    max_output_coordinates: 1_000_000,
};

/// Il controesempio della revisione: cornice valida con `L = 2^500`,
/// `h = 2^448`. Il filtro sull'area totale non decide, lo stadio esatto
/// rifiuta le coordinate, e prima l'esito diventava «ripara»: con
/// `STRUCTURE` la differenza shell - buco usciva vuota.
#[test]
fn cornice_fuori_dominio_e_un_errore_esplicito() {
    let input = Geometry::Polygon(cornice(2_f64.powi(500), 2_f64.powi(448)));
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        for keep_collapsed in [false, true] {
            let esiti = [
                make_valid_geometry_rust(&input, method, keep_collapsed),
                make_valid_geometry_rust_with_limits(&input, method, keep_collapsed, LIMITI),
                make_valid_geometry_rust_bounded(&input, method, keep_collapsed, LIMITI),
            ];
            for esito in esiti {
                assert!(
                    matches!(esito, Err(MakeValidError::NumericRange)),
                    "{method:?}: atteso NumericRange, ottenuto {esito:?}"
                );
            }
        }
    }
}

/// Cornice nel dominio con margine vicino alla griglia dell'overlay,
/// accanto a una farfalla che obbliga alla riparazione: la cornice deve
/// uscire con la sua area, oppure l'operazione deve fallire in modo
/// esplicito. Prima, da `h = 2^-28` in giu', ne restava meta' o niente;
/// fino a `2^-27` la riparazione deve riuscire (il controllo non rifiuta
/// cio' che l'overlay tratta bene).
#[test]
fn cornice_sottile_non_sparisce_in_silenzio() {
    let farfalla = Polygon::new(
        LineString::from(vec![
            (5.0, 5.0),
            (7.0, 7.0),
            (5.0, 7.0),
            (7.0, 5.0),
            (5.0, 5.0),
        ]),
        Vec::new(),
    );
    for esponente in [10, 20, 26, 28, 29, 30, 31, 32, 36, 40, 45, 50] {
        let sottile = cornice(1.0, 2_f64.powi(-esponente));
        let attesa = sottile.unsigned_area() + 2.0;
        let input = Geometry::MultiPolygon(MultiPolygon::new(vec![sottile, farfalla.clone()]));
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            match make_valid_geometry_rust(&input, method, false) {
                Ok(output) => {
                    let area = output.unsigned_area();
                    assert!(
                        (area - attesa).abs() <= attesa * 1e-15,
                        "2^-{esponente} {method:?}: area {area:e}, attesa {attesa:e}"
                    );
                }
                Err(errore) => assert!(
                    esponente > 27
                        && matches!(
                            errore,
                            MakeValidError::OverlayLoss | MakeValidError::NumericRange
                        ),
                    "2^-{esponente} {method:?}: errore inatteso {errore}"
                ),
            }
            // Stesso esito dall'adapter WKB: errore esplicito o area giusta.
            let payload = input.to_wkb(CoordDimensions::xy()).expect("wkb");
            match make_valid_wkb(&payload, MetodoAdapter::Linework, true) {
                Ok(output) => {
                    let area = geometry_from_wkb(&output).expect("valida").unsigned_area();
                    assert!(
                        (area - attesa).abs() <= attesa * 1e-15,
                        "2^-{esponente} adapter: area {area:e}, attesa {attesa:e}"
                    );
                }
                Err(errore) => assert!(
                    esponente > 27
                        && matches!(
                            errore,
                            RustBackendError::OverlayLoss | RustBackendError::NumericRange
                        ),
                    "2^-{esponente} adapter: errore inatteso {errore}"
                ),
            }
        }
    }
}

/// La cornice larga resta riparata come prima: il controllo non rifiuta i
/// casi che l'overlay tratta correttamente.
#[test]
fn cornice_larga_resta_riparata() {
    let farfalla = Polygon::new(
        LineString::from(vec![
            (5.0, 5.0),
            (7.0, 7.0),
            (5.0, 7.0),
            (7.0, 5.0),
            (5.0, 5.0),
        ]),
        Vec::new(),
    );
    let larga = cornice(1.0, 0.25);
    let attesa = larga.unsigned_area() + 2.0;
    let input = Geometry::MultiPolygon(MultiPolygon::new(vec![larga, farfalla]));
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        let area = make_valid_geometry_rust(&input, method, false)
            .expect("riparata")
            .unsigned_area();
        assert!((area - attesa).abs() <= 1e-12, "{method:?}: {area}");
    }
}
