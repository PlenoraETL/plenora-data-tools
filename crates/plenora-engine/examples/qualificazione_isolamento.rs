//! L'immagine che il gate ostile riesegue: supervisore, spawner e worker.
//!
//! Non e' un `bin` e non finisce in nessuna installazione: `cargo install` gli
//! esempi non li produce. Serve **anche** `--cfg qualificazione_isolamento`,
//! che non e' una feature di Cargo e che nessuna unificazione accende.
//!
//! Si costruisce cosi':
//!
//! ```text
//! RUSTFLAGS='--cfg qualificazione_isolamento' \
//!   cargo build --locked --features internals --example qualificazione_isolamento
//! ```
//!
//! e lo fa `scripts/verifica_isolamento_linux.sh`, che e' il suo unico
//! chiamante.

/// L'immagine costruita nel perimetro di qualificazione.
#[cfg(all(target_os = "linux", qualificazione_isolamento))]
fn main() -> std::process::ExitCode {
    plenora_engine::qualificazione::principale()
}

/// L'immagine costruita **fuori** da quel perimetro, che rifiuta di girare.
///
/// # Perche' rifiuta invece di non esistere
///
/// Perche' un esempio che non compilasse renderebbe rosso `cargo clippy
/// --all-targets`, che e' un gate ordinario e non sa niente di questo `cfg`.
/// E perche' un binario che girasse a meta' sarebbe peggio: il gate lo
/// invocherebbe, non troverebbe le righe che aspetta, e la diagnosi sarebbe
/// «il gate non ha osservato niente» invece di «il gate e' stato costruito
/// male».
#[cfg(not(all(target_os = "linux", qualificazione_isolamento)))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "questa immagine vale solo su Linux e solo con \
         RUSTFLAGS='--cfg qualificazione_isolamento'"
    );
    std::process::ExitCode::FAILURE
}
