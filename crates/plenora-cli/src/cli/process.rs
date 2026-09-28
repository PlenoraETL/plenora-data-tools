//! Il processo: dagli argomenti all'exit code.
//!
//! Rende il codice invece di uscire, cosi' `main` puo' avvolgere tutto nella
//! barriera anti-panico (`catch_unwind`); l'esito passa sempre dall'envelope.

use std::env;

use plenora_core::PlenoraError;

use super::error_envelope::{emetti_e_codice, error_envelope};

/// Il processo vero e proprio: restituisce l'exit code invece di uscire, cosi'
/// la barriera anti-panico di `main` puo' avvolgerlo.
pub fn esegui_processo() -> i32 {
    let args: Vec<String> = env::args().skip(1).collect();
    let args = match crate::strip_output_format(args) {
        Ok(args) => args,
        Err(error) => return emetti_e_codice(&error_envelope(&error, false)),
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
        return emetti_e_codice(&error_envelope(error.as_ref(), cancelled));
    }
    0
}
