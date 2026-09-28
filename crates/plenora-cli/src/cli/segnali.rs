//! I gestori di Ctrl-C e dei segnali del profilo isolato.

use plenora_core::PlenoraError;
use plenora_engine::CancellationToken;

use crate::cli::error_envelope::EXIT_CANCELLED;
use crate::contract;

/// Handler Ctrl-C (errori-e-limiti.md#cancellazione).
///
/// Il primo Ctrl-C cancella il token: l'executor si ferma al prossimo confine
/// cooperativo e la CLI esce con [`EXIT_CANCELLED`] senza pubblicare nulla.
/// Il secondo forza l'uscita immediata. `ctrlc::set_handler` si installa una
/// volta per processo, quindi un fallimento e' un errore vero.
pub fn install_ctrlc_handler(token: &CancellationToken) -> Result<(), PlenoraError> {
    let token = token.clone();
    let requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    ctrlc::set_handler(move || {
        // Gli avvisi sono per una PERSONA davanti a un terminale. Se stderr
        // non e' un terminale c'e' un programma dall'altro lato, e per lui il
        // canale resta vuoto: l'esito della cancellazione arriva comunque
        // come envelope su stdout con categoria `cancelled` ed exit 130.
        // La garanzia «stderr vuoto» (errori-e-limiti.md#envelope-e-canali)
        // vale quindi senza eccezioni
        // per ogni consumatore non interattivo.
        let interattivo = std::io::IsTerminal::is_terminal(&std::io::stderr());
        if requested.swap(true, std::sync::atomic::Ordering::SeqCst) {
            if interattivo {
                eprintln!("plenora-data-tools: secondo ctrl-c: uscita forzata");
            }
            std::process::exit(EXIT_CANCELLED);
        }
        if interattivo {
            eprintln!(
                "plenora-data-tools: ctrl-c: annullamento in corso (un secondo ctrl-c forza l'uscita)..."
            );
        }
        token.cancel();
    })
    .map_err(|error| contract(format!("handler ctrl-c non installabile: {error}")))
}

/// Gestore SIGINT del **percorso isolato** (`isolamento::esecuzione_isolata`).
///
/// Non usa [`install_ctrlc_handler`]: `ctrlc::set_handler` crea un thread
/// permanente, che farebbe fallire `isolamento::canale::accerta_monothread` a
/// entrambe le finestre di spawn. `signal_hook::flag::register` e
/// `register_conditional_shutdown` non creano thread: installano una
/// `sigaction` per tutta la vita del processo e scrivono direttamente
/// [`CancellationToken::condividi_flag`], quindi coprono worker, transizione
/// e verificatore.
///
/// La chiusura di shutdown va registrata per prima (lo impone la sua doc): al
/// primo Ctrl-C trova il flag `false` e non esce, al secondo lo trova `true`
/// ed esce con `signal_hook::low_level::exit`.
///
/// Differenze da [`install_ctrlc_handler`] (errori-e-limiti.md#cancellazione):
/// nessun messaggio interattivo, perche' le chiusure sono quelle fisse della
/// crate e iniettarne uno richiederebbe `unsafe`; il secondo Ctrl-C esce con
/// lo stesso codice ([`EXIT_CANCELLED`]) ma senza `atexit` ne' flush.
///
/// # Errors
///
/// Se la registrazione presso il kernel fallisce (rarissimo: segnale
/// vietato o gia' in uno stato incompatibile).
#[cfg(target_os = "linux")]
pub fn installa_gestore_segnale_isolato(token: &CancellationToken) -> Result<(), PlenoraError> {
    let bit = token.condividi_flag();
    signal_hook::flag::register_conditional_shutdown(
        signal_hook::consts::SIGINT,
        EXIT_CANCELLED,
        bit.clone(),
    )
    .map_err(|errore| {
        contract(format!(
            "gestore segnale isolato non installabile: {errore}"
        ))
    })?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, bit).map_err(|errore| {
        contract(format!(
            "gestore segnale isolato non installabile: {errore}"
        ))
    })?;
    Ok(())
}
