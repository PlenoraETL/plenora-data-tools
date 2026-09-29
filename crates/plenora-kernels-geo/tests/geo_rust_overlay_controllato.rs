//! `make_valid` non perde area in silenzio.
//!
//! Due strade verso lo stesso difetto, trovate dalla revisione:
//!
//! - un segno d'area non decidibile (coordinate fuori dal dominio esatto)
//!   tradotto in «area non positiva» avviava la riparazione di un poligono
//!   valido;
//! - l'overlay della riparazione passa per la griglia intera di `i_overlay`:
//!   una cornice piu' sottile della griglia collassava a vuoto, o ne restava
//!   meta', e il risultato superava la validazione finale.
//!
//! Ora ogni overlay e' preceduto da una precondizione di risolvibilita'
//! sulla griglia (`make_valid::overlay_precondition`): ogni esito e' la
//! geometria con l'area esatta o `PrecisionInsufficient`.

use std::cmp::Ordering;

use geo::{BoundingRect, Geometry, LineString, MultiPolygon, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_kernels_geo::geometry_from_wkb;
use plenora_kernels_geo::rust_backend::exact::{confronta_aree, AreaPoligono};
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

fn farfalla() -> Polygon<f64> {
    Polygon::new(
        LineString::from(vec![
            (5.0, 5.0),
            (7.0, 7.0),
            (5.0, 7.0),
            (7.0, 5.0),
            (5.0, 5.0),
        ]),
        Vec::new(),
    )
}

/// I poligoni dell'output dentro `[0, 1]^2`: la cornice, separata dalle
/// facce della farfalla.
fn poligoni_della_cornice(output: &Geometry<f64>) -> Vec<Polygon<f64>> {
    let mut poligoni = Vec::new();
    let mut raccogli = |poligono: &Polygon<f64>| {
        if poligono
            .bounding_rect()
            .is_some_and(|rettangolo| rettangolo.max().x <= 1.0)
        {
            poligoni.push(poligono.clone());
        }
    };
    let mut pila = vec![output];
    while let Some(geometria) = pila.pop() {
        match geometria {
            Geometry::Polygon(singolo) => raccogli(singolo),
            Geometry::MultiPolygon(multi) => multi.0.iter().for_each(&mut raccogli),
            Geometry::GeometryCollection(collezione) => pila.extend(collezione.0.iter()),
            _ => {}
        }
    }
    poligoni
}

/// La cornice dell'output ha esattamente l'area di quella d'ingresso.
fn cornice_intatta(output: &Geometry<f64>, attesa: &Polygon<f64>) -> bool {
    let candidati = poligoni_della_cornice(output);
    let [trovato] = candidati.as_slice() else {
        return false;
    };
    confronta_aree(
        trovato,
        AreaPoligono::di(trovato),
        attesa,
        AreaPoligono::di(attesa),
    ) == Ok(Ordering::Equal)
}

/// Cornice nel dominio con margine vicino alla griglia dell'overlay
/// (`2^-30` dell'estensione normalizzata), accanto a una farfalla che
/// obbliga alla riparazione. Prima, da `h = 2^-28` in giu', ne restava meta'
/// o niente. Ora la cornice esce con l'area esatta (confronto esatto, solo
/// la cornice) oppure l'operazione fallisce con `PrecisionInsufficient`.
/// Le soglie seguono la precondizione `>= 8` passi di griglia: `STRUCTURE`
/// sovrappone la cornice da sola (estensione 1, riesce fino a `2^-26`),
/// `LINEWORK` la sovrappone alla farfalla (estensione 7, fino a `2^-24`).
#[test]
fn cornice_sottile_non_sparisce_in_silenzio() {
    for esponente in [10, 20, 24, 25, 26, 27, 28, 29, 30, 31, 32, 36, 40, 45, 50] {
        let sottile = cornice(1.0, 2_f64.powi(-esponente));
        let input = Geometry::MultiPolygon(MultiPolygon::new(vec![sottile.clone(), farfalla()]));
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            let soglia = match method {
                RepairMethod::Structure => 26,
                RepairMethod::Linework => 24,
            };
            for keep_collapsed in [false, true] {
                match make_valid_geometry_rust(&input, method, keep_collapsed) {
                    Ok(output) => assert!(
                        esponente <= soglia && cornice_intatta(&output, &sottile),
                        "2^-{esponente} {method:?}: cornice alterata o esito inatteso"
                    ),
                    Err(errore) => assert!(
                        esponente > soglia
                            && matches!(errore, MakeValidError::PrecisionInsufficient),
                        "2^-{esponente} {method:?}: errore inatteso {errore}"
                    ),
                }
            }
        }
        // L'adapter WKB, con entrambi i metodi.
        let payload = input.to_wkb(CoordDimensions::xy()).expect("wkb");
        for (method, soglia) in [
            (MetodoAdapter::Structure, 26),
            (MetodoAdapter::Linework, 24),
        ] {
            match make_valid_wkb(&payload, method, true) {
                Ok(output) => {
                    let output = geometry_from_wkb(&output).expect("valida");
                    assert!(
                        esponente <= soglia && cornice_intatta(&output, &sottile),
                        "2^-{esponente} adapter {method:?}: cornice alterata o esito inatteso"
                    );
                }
                Err(errore) => assert!(
                    esponente > soglia && matches!(errore, RustBackendError::PrecisionInsufficient),
                    "2^-{esponente} adapter {method:?}: errore inatteso {errore}"
                ),
            }
        }
    }
}

/// Primo controesempio della terza revisione, dalla riparazione: buco
/// `[0.5, 0.5 + 2^-40] x [0.25, 0.75]` in `[0, 1]^2`, accanto a una
/// farfalla. Il vecchio controllo a posteriori accettava il buco perso; la
/// precondizione rifiuta prima dell'overlay.
#[test]
fn buco_piu_sottile_della_griglia_e_un_errore() {
    let largo = 0.5 + 2_f64.powi(-40);
    let buco = LineString::from(vec![
        (0.5, 0.25),
        (largo, 0.25),
        (largo, 0.75),
        (0.5, 0.75),
        (0.5, 0.25),
    ]);
    let poligono = Polygon::new(quadrato(0.0, 1.0), vec![buco]);
    let input = Geometry::MultiPolygon(MultiPolygon::new(vec![poligono, farfalla()]));
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        let esito = make_valid_geometry_rust(&input, method, false);
        assert!(
            matches!(esito, Err(MakeValidError::PrecisionInsufficient)),
            "{method:?}: {esito:?}"
        );
    }
}

/// La cornice larga resta riparata come prima: la precondizione non rifiuta
/// i casi che la griglia risolve.
#[test]
fn cornice_larga_resta_riparata() {
    let larga = cornice(1.0, 0.25);
    let input = Geometry::MultiPolygon(MultiPolygon::new(vec![larga.clone(), farfalla()]));
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        let output = make_valid_geometry_rust(&input, method, false).expect("riparata");
        assert!(cornice_intatta(&output, &larga), "{method:?}");
    }
}
