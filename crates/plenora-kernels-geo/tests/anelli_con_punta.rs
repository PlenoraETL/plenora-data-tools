//! Un anello che torna indietro su se stesso non e' semplice, quindi il
//! poligono non e' valido: la validazione del crate lo rifiuta, e lo fa
//! **prima** di `relate`.
//!
//! La ricerca di auto-intersezioni di `geo` 0.33.1 salta le coppie di segmenti
//! adiacenti, e una punta sta proprio li'. I tre reperti del fuzz target
//! `wkt_operations` hanno un guscio di tre punti collineari: per `geo` sono
//! validi, e su di loro `relate` rende una matrice che fa «contenere» un buco
//! esterno al guscio, oppure va in panico dentro `point_on_surface`.

use plenora_kernels_geo::construction::{geometry_from_wkt, ConstructionError};

fn rifiutato_per_auto_intersezione(testo: &str) {
    match geometry_from_wkt(testo) {
        Err(ConstructionError::InvalidOutput(ragione)) => assert!(
            ragione.contains("auto-intersezione"),
            "ragione inattesa per {testo:?}: {ragione}"
        ),
        altro => panic!("atteso il rifiuto per auto-intersezione di {testo:?}, ottenuto {altro:?}"),
    }
}

fn accettato(testo: &str) {
    assert!(
        geometry_from_wkt(testo).is_ok(),
        "{testo:?} e' valido e deve restarlo"
    );
}

/// I tre reperti, nella forma normalizzata che ne da' il parser.
#[test]
fn i_reperti_del_fuzz_sono_rifiutati() {
    rifiutato_per_auto_intersezione("POLYGON((12 52,1 74,0 76,12 52),(0 76,83 8,9 52,0 76))");
    rifiutato_per_auto_intersezione(
        "POLYGON((12 52,1 74,0 76,12 52),(0 76,8 376,12 52,1 74,0 76),(0 76,8 38,9 52,0 76))",
    );
    rifiutato_per_auto_intersezione(
        "POLYGON((12 52,1 74,0 76,12 52),(0 76,24 52,1 74,0 76),(0 76,8 38,9 52,0 76))",
    );
}

#[test]
fn un_guscio_di_punti_collineari_e_rifiutato() {
    rifiutato_per_auto_intersezione("POLYGON((0 0,1 1,2 2,0 0))");
    rifiutato_per_auto_intersezione("POLYGON((0 0,0 5,0 2,0 0))");
}

/// Una punta dentro un anello che per il resto ha area: il segmento
/// `(10 10)-(10 5)` ripercorre all'indietro `(10 0)-(10 10)`.
#[test]
fn una_punta_nel_guscio_e_rifiutata() {
    rifiutato_per_auto_intersezione("POLYGON((0 0,10 0,10 10,10 5,0 10,0 0))");
}

#[test]
fn una_punta_in_un_buco_e_rifiutata() {
    rifiutato_per_auto_intersezione(
        "POLYGON((0 0,20 0,20 20,0 20,0 0),(5 5,10 5,10 10,10 7,5 10,5 5))",
    );
}

#[test]
fn una_punta_in_un_componente_di_multipoligono_e_rifiutata() {
    rifiutato_per_auto_intersezione(
        "MULTIPOLYGON(((0 0,1 0,1 1,0 1,0 0)),((5 5,10 5,10 10,10 7,5 10,5 5)))",
    );
}

/// La punta che attraversa la chiusura dell'anello: il vertice che la forma e'
/// il primo, e il controllo deve riprenderlo facendo il giro.
#[test]
fn una_punta_sul_vertice_di_chiusura_e_rifiutata() {
    rifiutato_per_auto_intersezione("POLYGON((10 0,10 10,0 10,0 0,10 0,5 0,10 0))");
}

/// Controcaso: vertici collineari che **proseguono** nello stesso verso non
/// sono una punta. Senza questo, il controllo potrebbe rifiutare tutto cio'
/// che ha tre punti allineati e resterebbe verde.
#[test]
fn vertici_collineari_nello_stesso_verso_restano_validi() {
    accettato("POLYGON((0 0,5 0,10 0,10 10,0 10,0 0))");
    accettato("POLYGON((0 0,10 0,10 5,10 10,0 10,0 5,0 0))");
    accettato("POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,4 2,6 2,6 6,2 6,2 2))");
}

/// Controcaso: vertici ripetuti consecutivi non sono una punta.
#[test]
fn vertici_ripetuti_non_sono_una_punta() {
    accettato("POLYGON((0 0,10 0,10 0,10 10,0 10,0 0))");
}

/// Quasi collineare non e' collineare: il controllo e' esatto, senza
/// tolleranza. Il vertice `(5 1e-300)` torna quasi sul segmento precedente ma
/// resta sopra la retta, e l'anello e' semplice: un poligono sottilissimo,
/// valido.
#[test]
fn un_vertice_quasi_collineare_non_e_una_punta() {
    accettato("POLYGON((0 0,10 0,5 1e-300,0 10,0 0))");
}
