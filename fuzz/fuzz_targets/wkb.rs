#![no_main]

//! WKB di una cella geometria: il decoder validante dei kernel geo
//! (`geometry_from_wkb`) e la camminata del confine dei file
//! (`plenora_io::wkb::scansiona_cella`) sugli stessi byte.
//!
//! Invarianti: mai panico; una geometria accettata si ricodifica e si
//! rilegge uguale, e la ricodifica è stabile; ciò che il decoder dei kernel
//! accetta (2D, OGC valido) il confine dei file lo accetta.

use libfuzzer_sys::fuzz_target;
use plenora_io::wkb::{scansiona_cella, Sommario};
use plenora_kernels_geo::arrow_adapter::encode_geometry;
use plenora_kernels_geo::geometry_from_wkb;

#[path = "comune/aggancio.rs"]
mod aggancio;

fuzz_target!(init: aggancio::installa(), |dati: &[u8]| {
    let mut sommario = Sommario::default();
    let scansione = scansiona_cella(dati, &mut sommario);
    let Ok(geometria) = geometry_from_wkb(dati) else {
        return;
    };
    assert!(scansione.is_ok(), "il decoder accetta, il confine dei file no");

    let Ok(codificata) = encode_geometry(&geometria) else {
        return;
    };
    let riletta = geometry_from_wkb(&codificata).expect("ricodifica rileggibile");
    assert_eq!(riletta, geometria);
    let di_nuovo = encode_geometry(&riletta).expect("ricodifica della ricodifica");
    assert_eq!(di_nuovo, codificata);
});
