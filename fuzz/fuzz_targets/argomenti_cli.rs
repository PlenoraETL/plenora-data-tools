#![no_main]

//! Argomenti della CLI `plenora-data`: token arbitrari (separati da byte
//! zero) in `argomenti::leggi`, senza eseguire nulla.
//!
//! Invarianti: mai panico; un rifiuto è `InvalidConfiguration`; la lettura è
//! deterministica; `nome_comando` riconosce un comando ogni volta che la
//! lettura riesce.

use libfuzzer_sys::fuzz_target;
use plenora_cli::argomenti::{leggi, nome_comando};
use plenora_core::ErrorCategory;

#[path = "comune/aggancio.rs"]
mod aggancio;
#[path = "comune/esiti.rs"]
mod esiti;

fuzz_target!(init: aggancio::installa(), |dati: &[u8]| {
    let Ok(testo) = std::str::from_utf8(dati) else {
        return;
    };
    let argomenti: Vec<String> = testo.split('\0').map(str::to_owned).collect();
    let primo = leggi(&argomenti);
    let secondo = leggi(&argomenti);
    match (&primo, &secondo) {
        (Ok(primo), Ok(secondo)) => {
            assert_eq!(primo, secondo);
            assert!(nome_comando(&argomenti).is_some(), "invocazione letta senza comando");
        }
        (Err(primo), Err(secondo)) => {
            esiti::stesso_errore("argomenti", primo, secondo);
            assert_eq!(primo.category(), ErrorCategory::InvalidConfiguration, "{primo}");
        }
        _ => panic!("lettura degli argomenti non deterministica"),
    }
});
