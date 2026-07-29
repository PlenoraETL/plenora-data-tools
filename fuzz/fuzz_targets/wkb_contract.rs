#![no_main]

use geozero::{ToGeo, wkb::Wkb};
use libfuzzer_sys::fuzz_target;
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, validate_wkb_contract, Operation};

fuzz_target!(|payload: &[u8]| {
    // Oracolo differenziale ADR-0011: il decoder validante (via
    // `geometry_from_wkb`) e il percorso precedente (validatore strutturale
    // + geozero) devono accettare/rifiutare gli stessi payload e produrre
    // la stessa geometria, coordinata per coordinata.
    let reference = validate_wkb_contract(payload)
        .ok()
        .and_then(|()| Wkb(payload).to_geo().ok());
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
