#![no_main]

//! Confine di lettura Parquet (e GeoParquet): byte arbitrari come
//! file, invarianti in `comune/lettura.rs`.

use libfuzzer_sys::fuzz_target;
use plenora_io::Formato;

#[path = "comune/aggancio.rs"]
mod aggancio;
#[path = "comune/esiti.rs"]
mod esiti;
#[path = "comune/lettura.rs"]
mod lettura;

fuzz_target!(init: aggancio::installa(), |dati: &[u8]| {
    lettura::prova(dati, Formato::Parquet);
});
