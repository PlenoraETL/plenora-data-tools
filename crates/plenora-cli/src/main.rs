//! `plenora-data`: il binario della CLI. Tutta la logica è nella libreria
//! (`plenora_cli`); qui solo ciò che è stato del processo.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use plenora_core::panic_policy::{install, panici_fuori_dalle_barriere, PanicPolicy};

fn main() -> ExitCode {
    // Nessun testo di panico su stderr (CLI 2.0, sezione 4): il panico
    // diventa l'inviluppo `internal` nella libreria, e l'hook conta i panici
    // di ogni thread (anche quello del gestore di Ctrl-C), che la libreria
    // controlla prima di dichiarare un successo. Questa è la prima
    // installazione del processo.
    let _ = install(PanicPolicy::Silent);
    // La base del conto, prima di avviare il thread del gestore: un suo
    // panico, anche immediato, cade dopo la base e si vede.
    let panici_base = panici_fuori_dalle_barriere();
    let segnale = Arc::new(AtomicBool::new(false));
    let alzato = Arc::clone(&segnale);
    let segnale = ctrlc::set_handler(move || alzato.store(true, Ordering::Release))
        .ok()
        .map(|()| segnale);
    let uscita = plenora_cli::esegui_invocazione_os(
        std::env::args_os().skip(1).collect(),
        &segnale,
        panici_base,
    );
    // Uno stdout non scrivibile dà il codice della categoria `io`, senza un
    // secondo documento e senza stderr.
    ExitCode::from(plenora_cli::consegna(
        &uscita,
        &mut std::io::stdout().lock(),
    ))
}
