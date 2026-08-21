#![no_main]

use geo::algorithm::validation::Validation;
use geozero::{ToGeo, wkb::Wkb};
use libfuzzer_sys::fuzz_target;
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, validate_wkb_contract, Operation};

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
    // points". Il target era rosso per questo motivo, e finche' lo restava
    // non poteva segnalare divergenze vere.
    let reference = validate_wkb_contract(payload)
        .ok()
        .and_then(|()| Wkb(payload).to_geo().ok())
        .filter(|geometry| geometry.check_validation().is_ok());
    let decoded = geometry_from_wkb(payload).ok();
    match (&reference, &decoded) {
        (Some(expected), Some(actual)) => assert_eq!(expected, actual),
        (None, None) => {}
        (Some(_), None) => panic!("divergenza decoder: riferimento Ok, decoder Err"),
        (None, Some(_)) => panic!("divergenza decoder: riferimento Err, decoder Ok"),
    }
    if decoded.is_some() {
        for operation in Operation::ALL {
            if let Ok(output) = transform_wkb(operation, payload) {
                assert!(geometry_from_wkb(&output).is_ok());
            }
        }
    }
});
