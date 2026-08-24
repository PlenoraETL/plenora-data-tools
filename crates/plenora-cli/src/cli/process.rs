//! Il processo: dagli argomenti all'exit code.
//!
//! Sta fra `main` e il dispatch. `main` installa la politica anti-panico e
//! avvolge questa funzione in `catch_unwind`; qui si raccolgono gli
//! argomenti, si delega al dispatch e si traduce l'esito in un exit code,
//! passando sempre dall'envelope.
//!
//! Restituire il codice invece di uscire e' cio' che permette a `main` di
//! avvolgere il tutto: `std::process::exit` da qui salterebbe la barriera.

use std::env;

use plenora_core::PlenoraError;

use super::error_envelope::{emit_error_envelope, error_envelope, error_exit_code, EXIT_INTERNO};

/// Descrizione PUBBLICA del payload di un panico: la forma, mai il contenuto.
///
/// Il testo di un panico non e' scritto da noi. Un `assert_eq!` dentro una
/// dipendenza puo' includere i valori confrontati, cioe' dati della riga:
/// pubblicarlo nell'envelope significherebbe esfiltrare contenuto dell'input
/// nei log di chi ci invoca. Stessa scelta del confine IPC e dell'executor.
pub fn descrivi_panico_locale(panico: &Box<dyn std::any::Any + Send>) -> &'static str {
    plenora_core::panic_policy::forma_payload(panico.as_ref())
}

/// Il processo vero e proprio: restituisce l'exit code invece di uscire, cosi'
/// la barriera anti-panico di `main` puo' avvolgerlo.
pub fn esegui_processo() -> i32 {
    let args: Vec<String> = env::args().skip(1).collect();
    let args = match crate::strip_output_format(args) {
        Ok(args) => args,
        Err(error) => {
            let envelope = error_envelope(&error, false);
            let _ = emit_error_envelope(std::io::stdout().lock(), &envelope);
            return error_exit_code(&envelope);
        }
    };
    if let Err(error) = crate::run_with_args(&args) {
        // Cancellazione cooperativa (errori-e-limiti.md#cancellazione): exit
        // code dedicato; il
        // publish atomico garantisce che nessun output parziale sia stato
        // pubblicato. Envelope §9 anche per la cancellazione (categoria
        // dedicata, fase/effetto/retry dagli assi).
        let cancelled = error
            .downcast_ref::<PlenoraError>()
            .is_some_and(PlenoraError::is_cancelled);
        let envelope = error_envelope(error.as_ref(), cancelled);
        let exit_code = error_exit_code(&envelope);
        if emit_error_envelope(std::io::stdout().lock(), &envelope).is_err() {
            return EXIT_INTERNO;
        }
        return exit_code;
    }
    0
}
