//! Il processo: dagli argomenti all'exit code.
//!
//! Rende il codice invece di uscire, cosi' `main` puo' avvolgere tutto nella
//! barriera anti-panico (`catch_unwind`); l'esito passa sempre dall'envelope.

use std::env;

use plenora_core::PlenoraError;

use super::error_envelope::{emit_error_envelope, error_envelope, error_exit_code, EXIT_INTERNO};

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
