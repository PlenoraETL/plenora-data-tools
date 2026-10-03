//! L'hook di panico comune a tutti i target.
//!
//! `libfuzzer-sys` 0.4.13 installa in `LLVMFuzzerInitialize` un hook che
//! chiama `abort()` **prima** dell'unwinding, cosi' che nessun `catch_unwind`
//! del codice sotto prova possa nascondere un panico al fuzzer. Tratta pero'
//! allo stesso modo due cose diverse:
//!
//! - il panico **atteso** di una dipendenza nominata, dentro una
//!   `plenora_core::panic_policy::barriera_di_dipendenza` che lo trasforma in
//!   un esito classificato — `relate` di `geo`, `arrow-ipc` e `parquet` su file malformati;
//! - qualunque altro panico, cioe' un nostro difetto, anche quando una rete di
//!   sicurezza come il `catch_unwind` della CLI lo intercetta.
//!
//! Col solo hook di `libfuzzer-sys` il primo tiene rosso un target a barriera
//! funzionante, e un target sempre rosso smette di essere letto. Senza hook il
//! secondo sparirebbe. Questo hook distingue: dentro una barriera di dipendenza
//! tace, altrove passa la mano all'hook di `libfuzzer-sys`, che stampa e
//! interrompe.
//!
//! Un panico fuori da ogni `catch_unwind` resta comunque un crash: risale fino
//! a `test_input_wrap`, che lo intercetta e chiama `abort()`.

use std::sync::atomic::{AtomicU64, Ordering};

/// Panici di dipendenza intercettati da una barriera, dall'avvio del
/// processo. Un `Internal` documentato («validazione non conclusa») nasce da
/// uno di questi: [`panici_in_barriera`] permette ai target di ammetterlo
/// solo se, durante l'ingresso corrente, una barriera ha davvero scattato.
static PANICI_IN_BARRIERA: AtomicU64 = AtomicU64::new(0);

/// Il conteggio di [`PANICI_IN_BARRIERA`].
#[allow(dead_code)] // Non tutti i target ammettono un `Internal` di barriera.
pub fn panici_in_barriera() -> u64 {
    PANICI_IN_BARRIERA.load(Ordering::SeqCst)
}

/// Sostituisce l'hook di `libfuzzer-sys` con quello che distingue.
///
/// Va nel blocco `init:` di `fuzz_target!`, che gira dopo
/// `libfuzzer_sys::initialize`: l'hook che si prende qui e' quello che
/// interrompe.
pub fn installa() {
    let di_libfuzzer = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if plenora_core::panic_policy::dentro_una_barriera_di_dipendenza() {
            PANICI_IN_BARRIERA.fetch_add(1, Ordering::SeqCst);
            return;
        }
        di_libfuzzer(info);
    }));
}
