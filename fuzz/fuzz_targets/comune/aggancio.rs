//! L'hook di panico comune a tutti i target.
//!
//! `libfuzzer-sys` 0.4.10 installa in `LLVMFuzzerInitialize` un hook che
//! chiama `abort()` **prima** dell'unwinding, cosi' che nessun `catch_unwind`
//! del codice sotto prova possa nascondere un panico al fuzzer. Tratta pero'
//! allo stesso modo due cose diverse:
//!
//! - il panico **atteso** di una dipendenza nominata, dentro una
//!   `plenora_core::panic_policy::barriera_di_dipendenza` che lo trasforma in
//!   un esito classificato — `relate` di `geo`, `fb_to_schema` di `arrow-ipc`;
//! - qualunque altro panico, cioe' un nostro difetto, anche quando una rete di
//!   sicurezza come il `catch_unwind` dell'executor lo intercetta.
//!
//! Col solo hook di `libfuzzer-sys` il primo tiene rosso un target a barriera
//! funzionante, e un target sempre rosso smette di essere letto. Senza hook il
//! secondo sparirebbe. Questo hook distingue: dentro una barriera di dipendenza
//! tace, altrove passa la mano all'hook di `libfuzzer-sys`, che stampa e
//! interrompe.
//!
//! Un panico fuori da ogni `catch_unwind` resta comunque un crash: risale fino
//! a `test_input_wrap`, che lo intercetta e chiama `abort()`.

/// Sostituisce l'hook di `libfuzzer-sys` con quello che distingue.
///
/// Va nel blocco `init:` di `fuzz_target!`, che gira dopo
/// `libfuzzer_sys::initialize`: l'hook che si prende qui e' quello che
/// interrompe.
pub fn installa() {
    let di_libfuzzer = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if plenora_core::panic_policy::dentro_una_barriera_di_dipendenza() {
            return;
        }
        di_libfuzzer(info);
    }));
}
