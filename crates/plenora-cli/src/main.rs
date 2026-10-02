//! `plenora-data`: il binario della CLI. Tutta la logica è nella libreria
//! (`plenora_cli`); qui solo ciò che è stato del processo.

use std::io::Write as _;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use plenora_core::panic_policy::{install, PanicPolicy};

fn main() -> ExitCode {
    // Nessun testo di panico su stderr (CLI 2.0, sezione 4): il panico
    // diventa l'inviluppo `internal` nella libreria. Se un altro `install`
    // fosse già avvenuto l'hook resterebbe il suo; qui è la prima chiamata
    // del processo.
    let _ = install(PanicPolicy::Silent);
    let segnale = Arc::new(AtomicBool::new(false));
    let alzato = Arc::clone(&segnale);
    let segnale = ctrlc::set_handler(move || alzato.store(true, Ordering::Release))
        .ok()
        .map(|()| segnale);
    let uscita =
        plenora_cli::esegui_invocazione_os(std::env::args_os().skip(1).collect(), &segnale);
    // Uno stdout chiuso non ha un canale su cui dirlo: il codice d'uscita
    // resta quello dell'esito.
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(uscita.stdout.as_bytes());
    let _ = stdout.flush();
    ExitCode::from(uscita.codice)
}
