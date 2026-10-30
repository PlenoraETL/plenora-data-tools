#![no_main]

//! PROJJSON, WKT1/WKT2 e identificatori attraverso il risolutore pubblico.
//! Invarianti: nessun panico (anche intercettato), determinismo completo,
//! limiti testuali, errori da un elenco chiuso senza dati del documento.
//! Ogni successo coincide con la risoluzione indipendente dell'ID integrato,
//! conserva il testo e sopravvive a una seconda risoluzione.

use libfuzzer_sys::fuzz_target;

#[path = "comune/aggancio.rs"]
mod aggancio;
#[path = "comune/crs.rs"]
mod crs;
#[path = "comune/esiti.rs"]
mod esiti;

fuzz_target!(init: aggancio::installa(), |dati: &[u8]| {
    let prima = aggancio::panici_in_barriera();
    if let Ok(testo) = std::str::from_utf8(dati) {
        crs::verifica(testo);
    }
    assert_eq!(prima, aggancio::panici_in_barriera());
});
