//! Il conto dei panici fuori dalle barriere (`panic_policy`), con deltas
//! esatti.
//!
//! Il conto e l'hook sono del processo: questo binario di test ha **un solo
//! test**, così nessun altro panico del processo lo sporca e i delta si
//! confrontano per uguaglianza.

use plenora_core::panic_policy::{
    barriera_di_dipendenza, install, panici_fuori_dalle_barriere, PanicPolicy,
};

#[test]
fn barriera_piu_zero_altro_thread_piu_uno() {
    assert!(
        install(PanicPolicy::Silent),
        "prima installazione del processo"
    );
    let base = panici_fuori_dalle_barriere();
    assert_eq!(base, 0, "nessun panico prima");

    // Un panico dentro una barriera, su questo thread: +0.
    assert!(barriera_di_dipendenza(|| -> () { std::panic::panic_any("atteso") }).is_err());
    assert_eq!(panici_fuori_dalle_barriere(), base);

    // Dentro una barriera aperta su un altro thread: +0.
    let figlio = std::thread::spawn(|| {
        barriera_di_dipendenza(|| -> () { std::panic::panic_any("atteso") }).is_err()
    });
    assert!(figlio.join().expect("il thread finisce"));
    assert_eq!(panici_fuori_dalle_barriere(), base);

    // Fuori da ogni barriera, su un altro thread che muore: +1 esatto.
    let figlio = std::thread::spawn(|| std::panic::panic_any(String::from("payload")));
    assert!(figlio.join().is_err());
    assert_eq!(panici_fuori_dalle_barriere(), base + 1);

    // Fuori da ogni barriera, su questo thread, intercettato da un
    // `catch_unwind` che non è una barriera: +1 esatto.
    assert!(std::panic::catch_unwind(|| -> () { std::panic::panic_any(0_u8) }).is_err());
    assert_eq!(panici_fuori_dalle_barriere(), base + 2);
}
