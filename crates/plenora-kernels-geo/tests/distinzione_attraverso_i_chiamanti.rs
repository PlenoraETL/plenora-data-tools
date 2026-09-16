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
//! # Stato col candidato esatto (BOZZA NON ADOTTATA, vedi Cargo.toml)
//!
//! Il reperto sotto conclude `Internal` (`ValidazioneNonConclusa`) con le
//! asserzioni di debug attive, senza il diff 1 (`vendor/geo-0.33.1-exact`):
//! il segno corretto di `orient2d` toglie la causa dell'asserzione, e `geo`
//! conclude sempre — misurato qui, non dedotto. Con questo reperto non resta
//! piu' un ingresso che interrompa la validazione di NESSUNO dei tre
//! chiamanti sotto: i casi qui provano solo la meta' "geo conclude, l'ingresso
//! e' davvero invalido".
//!
//! La meta' "interrotta" **non e' scoperta**: vive come prova sintetica
//! attraverso la conversione reale di ciascun chiamante, nel modulo di test
//! dello stesso file sorgente — `advanced::tests::classifica_punto_*` per
//! `voronoi_cells`, `topology::tests::classifica_geometria_*` per `dissolve`,
//! `predicates::tests::classifica_lato_*` per `evaluate`. L'innesco e' un
//! `EsitoValidazione::NonConclusa` costruito a mano (nessun reperto reale la
//! raggiunge piu'), ma la funzione chiamata e' quella vera di ciascun
//! chiamante, estratta a parte proprio per essere testabile cosi'. Ogni
//! coppia include la controprova: lo stesso chiamante su un esito concluso
//! produce l'altra variante, mai la stessa — prova che la distinzione non
//! collassa in nessuna delle due direzioni.

use geo::Geometry;
use geozero::{wkb::Wkb, ToGeo};
use plenora_kernels_geo::advanced::{voronoi_cells, AdvancedError};
use plenora_kernels_geo::predicates::{evaluate, PredicateError, SpatialPredicate};
use plenora_kernels_geo::topology::{dissolve, TopologyError};

/// Il reperto del 5 settembre 2026, decodificato **senza** validazione OGC:
/// serve la geometria grezza, perche' il punto e' quello che i chiamanti
/// fanno quando la validano loro.
fn reperto() -> Geometry<f64> {
    const BYTE: &[u8] = &[
        1, 6, 0, 0, 0, 3, 0, 0, 0, 1, 3, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 1, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 46, 254, 255, 255, 253, 15, 0, 0, 16, 64, 64, 64, 64, 0, 0,
        1, 3, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 0, 0, 0, 4,
        0, 4, 0, 0, 8, 116, 116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 1, 3, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0,
        6, 0, 0, 0, 0, 0, 0, 0, 5, 46, 254, 255, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 212, 0, 0, 0, 0, 0, 4, 0, 0, 8, 116, 116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
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
fn voronoi_su_reperto_concluso_resta_invalidpoint_con_contesto() {
    let geometrie = [reperto(), quadrato()];
    let errore = voronoi_cells(&geometrie, 16).expect_err("il reperto non passa");
    assert!(
        matches!(errore, AdvancedError::InvalidPoint { .. }),
        "col candidato esatto `geo` conclude sempre: l'ingresso e' davvero invalido — {errore:?}"
    );
}

/// **`dissolve` distingue, con una variante a tupla.**
///
/// L'altra forma di conversione presente nel crate: `InvalidGeometry(String)`
/// invece di una variante a campi. La distinzione deve reggere in entrambe.
#[test]
fn dissolve_su_reperto_concluso_resta_invalidgeometry() {
    let geometrie = [reperto(), quadrato()];
    let errore = dissolve(&geometrie).expect_err("il reperto non passa");
    assert!(
        matches!(errore, TopologyError::InvalidGeometry(_)),
        "col candidato esatto `geo` conclude sempre: {errore:?}"
    );
}

/// **`evaluate` distingue, e nomina comunque il lato.**
///
/// Il caso serve anche a fissare che la distinzione non costa il contesto:
/// dove l'ingresso e' davvero invalido, l'errore continua a dire quale lato.
#[test]
fn i_predicati_su_reperto_concluso_nominano_il_lato() {
    let errore = evaluate(&reperto(), &quadrato(), SpatialPredicate::Intersects)
        .expect_err("il reperto non passa");
    assert!(
        matches!(errore, PredicateError::InvalidGeometry { side: "left", .. }),
        "col candidato esatto `geo` conclude sempre, e il lato resta nominato: {errore:?}"
    );
}

// La prova "il testo dell'esito interrotto non porta dati dell'ingresso" non
// vive piu' qui: senza un reperto reale che interrompa questi tre chiamanti,
// non c'e' un ingresso su cui osservarla in questo file. Ogni
// `classifica_*_non_appiattisce_l_interruzione` (advanced.rs, topology.rs,
// predicates.rs) verifica il testo esatto reso dalla propria conversione su
// un `EsitoValidazione::NonConclusa` sintetico — la stessa proprieta',
// provata dove l'innesco puo' essere reale.
