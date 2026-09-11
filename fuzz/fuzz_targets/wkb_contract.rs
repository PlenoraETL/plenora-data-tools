#![no_main]

use geo::algorithm::validation::Validation;
use geozero::{ToGeo, wkb::Wkb};
use libfuzzer_sys::fuzz_target;
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, validate_wkb_contract, Operation};

/// Il giudizio del riferimento su una geometria: accetta, rifiuta, o **non
/// conclude**.
///
/// # Che cosa questo `catch_unwind` NON ottiene
///
/// **Non impedisce al panico di terminare il target.** `libfuzzer-sys` 0.4.10
/// installa un hook che chiama `abort()` *prima* dell'unwinding: quando
/// `check_validation` panica, il processo muore dentro l'hook e questo
/// `catch_unwind` non viene mai raggiunto. Misurato — `cargo fuzz run
/// wkb_contract` sui due reperti del 4 e 5 settembre 2026 termina con `deadly
/// signal`, e il difetto resta aperto.
///
/// Il ramo `None` regge percio' soltanto nei processi che consentono
/// l'unwinding: la batteria ordinaria, dove i due reperti sono casi versionati.
/// Resta scritto perche' e' il contratto corretto del riferimento, non perche'
/// protegga questo target.
///
/// # Perche' `catch_unwind` qui e non la barriera di produzione
///
/// Perche' il riferimento deve restare **indipendente** da cio' che giudica:
/// se usasse `ValidazioneProtetta`, l'oracolo misurerebbe la stessa difesa che
/// e' incaricato di sorvegliare, e un difetto di quella difesa passerebbe
/// inosservato da entrambe le parti. Chiama percio' `geo` direttamente.
///
/// Rende `None` quando la validazione non conclude: un riferimento che non
/// conclude non ha giudicato niente, e trattarlo come «invalida» direbbe che il
/// decoder diverge quando invece nessuno ha deciso.
fn riferimento_giudica(geometry: &geo::Geometry<f64>) -> Option<bool> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        geometry.check_validation().is_ok()
    }))
    .ok()
}

fuzz_target!(|payload: &[u8]| {
    // Oracolo differenziale architettura.md#geometrie: il decoder validante (via
    // `geometry_from_wkb`) e il percorso precedente (validatore strutturale
    // + geozero) devono accettare/rifiutare gli stessi payload e produrre
    // la stessa geometria, coordinata per coordinata.
    //
    // I due percorsi non hanno pero' lo stesso contratto: `geometry_from_wkb`
    // applica anche la validazione OGC, che geozero non fa. Senza il filtro
    // qui sotto l'oracolo segnala come divergenza ogni geometria
    // strutturalmente ben formata ma non valida — per esempio
    // `LINESTRING(0 0, 0 0, 0 0, 0 0)`, che geozero decodifica e il decoder
    // rifiuta correttamente con "line string must have at least 2 distinct
    // points". Un target rosso per questo motivo non puo' segnalare
    // divergenze vere.
    // Il decoder si esercita **sempre**: che non panichi e' un'invariante di
    // questo target, indipendente dal fatto che il riferimento sappia giudicare.
    let decoded = geometry_from_wkb(payload).ok();

    let grezzo = validate_wkb_contract(payload)
        .ok()
        .and_then(|()| Wkb(payload).to_geo().ok());
    // Tre esiti del riferimento, non due: accetta, rifiuta, oppure **non
    // conclude**. Il terzo sospende il confronto invece di inventarne uno.
    //
    // Sui due reperti della campagna schedulata quel terzo ramo non si
    // raggiunge: l'hook di libFuzzer aborta prima, e il target muore. Vale nei
    // processi che consentono l'unwinding, non qui.
    let confronto = match grezzo {
        None => Some(None),
        Some(geometry) => match riferimento_giudica(&geometry) {
            Some(true) => Some(Some(geometry)),
            Some(false) => Some(None),
            None => None,
        },
    };
    if let Some(reference) = confronto {
        match (&reference, &decoded) {
            (Some(expected), Some(actual)) => assert_eq!(expected, actual),
            (None, None) => {}
            (Some(_), None) => panic!("divergenza decoder: riferimento Ok, decoder Err"),
            (None, Some(_)) => panic!("divergenza decoder: riferimento Err, decoder Ok"),
        }
    }
    if decoded.is_some() {
        for operation in Operation::ALL {
            if let Ok(output) = transform_wkb(operation, payload) {
                assert!(geometry_from_wkb(&output).is_ok());
            }
        }
    }
});
