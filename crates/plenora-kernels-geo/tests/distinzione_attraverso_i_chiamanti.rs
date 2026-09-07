//! La validazione interrotta resta **distinta** attraverso i chiamanti.
//!
//! # Perche' non basta `geometry_from_wkb`
//!
//! Perche' quella porta rende `PlenoraError`, e la barriera la classifica bene
//! da sola. Gli altri chiamanti hanno un errore **proprio**, e ognuno di loro
//! puo' appiattire la distinzione nella sua variante «ingresso non valido»:
//! il tipo la conserva all'origine, ma e' la conversione a poterla perdere.
//! Un caso che guardasse solo la porta WKB non vedrebbe mai quella perdita.
//!
//! # Perche' l'attesa dipende dal profilo
//!
//! Perche' il panico di `geo` e' un `debug_assert!`: con le asserzioni di debug
//! spente non c'e' nulla da distinguere, e l'esito e' il rifiuto ordinario.
//! Si pretende percio' l'esito giusto per il profilo, in entrambi i casi.

use geo::Geometry;
use geozero::{ToGeo, wkb::Wkb};
use plenora_kernels_geo::advanced::{voronoi_cells, AdvancedError};
use plenora_kernels_geo::predicates::{evaluate, PredicateError, SpatialPredicate};
use plenora_kernels_geo::topology::{dissolve, TopologyError};

/// Il reperto del 5 settembre 2026, decodificato **senza** validazione OGC:
/// serve la geometria grezza, perche' il punto e' quello che i chiamanti
/// fanno quando la validano loro.
fn reperto() -> Geometry<f64> {
    const BYTE: &[u8] = &[
    1, 6, 0, 0, 0, 3, 0, 0, 0, 1, 3, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 1, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 5, 46, 254, 255, 255, 253, 15, 0, 0, 16, 64, 64, 64, 64, 0, 0, 1, 3, 0, 0, 0,
    1, 0, 0, 0, 7, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 0, 0, 0, 4, 0, 4, 0, 0, 8, 116,
    116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 0, 1,
    0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 5, 46, 254,
    255, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 0, 0, 0, 0, 0, 4, 0,
    0, 8, 116, 116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
    Wkb(BYTE).to_geo().expect("decodifica grezza")
}

/// Una geometria valida, per il secondo argomento dei predicati.
fn quadrato() -> Geometry<f64> {
    use geo::{Coord, LineString, Polygon};
    Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 1.0, y: 0.0 },
            Coord { x: 1.0, y: 1.0 },
            Coord { x: 0.0, y: 1.0 },
            Coord { x: 0.0, y: 0.0 },
        ]),
        Vec::new(),
    ))
}

/// **`voronoi_cells` non chiama «punto non valido» una validazione interrotta.**
///
/// La sua conversione ha una variante a campi, `InvalidPoint { index, reason }`:
/// costruirla da qualunque esito manda chi legge a correggere il punto
/// all'indice `index`, che nessuno ha stabilito essere sbagliato. La distinzione
/// regge percio' anche dove l'errore del chiamante porta contesto.
#[test]
fn voronoi_distingue_la_validazione_interrotta() {
    let geometrie = [reperto(), quadrato()];
    let errore = voronoi_cells(&geometrie, 16).expect_err("il reperto non passa");
    if cfg!(debug_assertions) {
        assert!(
            matches!(errore, AdvancedError::ValidazioneNonConclusa(_)),
            "la validazione si e' interrotta, non ha giudicato il punto: {errore:?}"
        );
    } else {
        assert!(
            matches!(errore, AdvancedError::InvalidPoint { .. }),
            "qui `geo` conclude: l'ingresso e' davvero invalido — {errore:?}"
        );
    }
}

/// **`dissolve` distingue, con una variante a tupla.**
///
/// L'altra forma di conversione presente nel crate: `InvalidGeometry(String)`
/// invece di una variante a campi. La distinzione deve reggere in entrambe.
#[test]
fn dissolve_distingue_la_validazione_interrotta() {
    let geometrie = [reperto(), quadrato()];
    let errore = dissolve(&geometrie).expect_err("il reperto non passa");
    if cfg!(debug_assertions) {
        assert!(
            matches!(errore, TopologyError::ValidazioneNonConclusa(_)),
            "atteso l'esito interrotto: {errore:?}"
        );
    } else {
        assert!(
            matches!(errore, TopologyError::InvalidGeometry(_)),
            "qui `geo` conclude: {errore:?}"
        );
    }
}

/// **`evaluate` distingue, e nomina comunque il lato.**
///
/// Il caso serve anche a fissare che la distinzione non costa il contesto:
/// dove l'ingresso e' davvero invalido, l'errore continua a dire quale lato.
#[test]
fn i_predicati_distinguono_la_validazione_interrotta() {
    let errore = evaluate(&reperto(), &quadrato(), SpatialPredicate::Intersects)
        .expect_err("il reperto non passa");
    if cfg!(debug_assertions) {
        assert!(
            matches!(errore, PredicateError::ValidazioneNonConclusa(_)),
            "atteso l'esito interrotto: {errore:?}"
        );
    } else {
        assert!(
            matches!(errore, PredicateError::InvalidGeometry { side: "left", .. }),
            "qui `geo` conclude, e il lato resta nominato: {errore:?}"
        );
    }
}

/// **Il testo dell'esito interrotto non porta dati dell'ingresso.**
///
/// La distinzione non deve costare la sanitizzazione: la variante nuova porta
/// la *forma* del payload, e il caso lo pretende su tutti e tre i chiamanti.
#[test]
fn nessun_chiamante_pubblica_il_contenuto() {
    if !cfg!(debug_assertions) {
        // Senza il panico non esiste l'esito interrotto da esaminare: la
        // ragione e' verificata da `barriera_privacy_processo.rs`.
        return;
    }
    let geometrie = [reperto(), quadrato()];
    let testi = [
        voronoi_cells(&geometrie, 16).expect_err("voronoi").to_string(),
        dissolve(&geometrie).expect_err("dissolve").to_string(),
        evaluate(&reperto(), &quadrato(), SpatialPredicate::Intersects)
            .expect_err("evaluate")
            .to_string(),
    ];
    for testo in testi {
        assert!(
            testo.contains("validazione OGC non conclusa")
                && testo.contains("contenuto non pubblicato"),
            "forma inattesa: {testo}"
        );
        for frammento in ["e-270", "e-312", "COORD", "topology", "right_location"] {
            assert!(
                !testo.contains(frammento),
                "il testo porta «{frammento}»: {testo}"
            );
        }
    }
}
