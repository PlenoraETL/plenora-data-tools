#![no_main]

//! `table.sort` contro un oracolo indipendente, sotto le 64 righe: invarianti in
//! `comune/ordinamento.rs`.

use libfuzzer_sys::fuzz_target;

#[path = "comune/aggancio.rs"]
mod aggancio;
#[path = "comune/ordinamento.rs"]
mod ordinamento;

fuzz_target!(init: aggancio::installa(), |dati: &[u8]| {
    ordinamento::prova(dati, false);
});
