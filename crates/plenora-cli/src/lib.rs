//! plenora-cli — la CLI pubblica `plenora-data` e la superficie Rust delle
//! stesse operazioni.
//!
//! Adotta il profilo data-tools versione 2 di `plenora-contracts` (commit
//! `3c395a8db96df739024e203b22794af340dc8a7f`): CLI 2.0 (un documento JSON
//! su stdout, niente su stderr, codici d'uscita per categoria), Typed Errors
//! 1.0 (la proiezione pubblica di `PlenoraError`), Capability Discovery 2.0,
//! Surface Bindings 1.0 (comandi `catalog`, `describe`, `validate`, `run`).
//! Nessuna deviazione: i contratti adottati sono in docs/cli.md, «CLI
//! `plenora-data`».
//!
//! Struttura:
//!
//! - [`operazioni`]: la tabella delle quattro operazioni pubbliche, unica
//!   fonte di comandi, aiuto, capacità e mappe degli export Rust e Python;
//! - [`catalogo`]: il registro dei kernel (`data.catalog`), derivato dal
//!   catalogo di `plenora-core`;
//! - [`api`]: le operazioni come funzioni Rust, che la CLI e l'SDK Python
//!   (`crates/plenora-data-py`) chiamano;
//! - [`capacita`]: Capability Discovery 2.0 (CLI e SDK Python), mappa Rust e
//!   mappa Python;
//! - [`argomenti`], [`inviluppo`], [`esegui_invocazione`]: la CLI.
//!
//! `main.rs` installa l'hook di panico silenzioso e il gestore di Ctrl-C,
//! chiama [`esegui_invocazione`] e scrive il risultato.

pub mod api;
pub mod argomenti;
mod artefatti;
pub mod capacita;
pub mod catalogo;
mod cli;
pub mod inviluppo;
pub mod operazioni;

pub use cli::{
    consegna, esegui_invocazione, esegui_invocazione_dal, esegui_invocazione_os, testo_aiuto,
    Segnale, CODICE_STDOUT_NON_SCRIVIBILE, CONTRATTO_AIUTO, CONTRATTO_VERSIONE,
};
pub use inviluppo::Uscita;

/// Identificatore stabile del componente (Public Surfaces 1.0, SURF-001).
pub const COMPONENTE: &str = "plenora-data-tools";
/// Nome dell'artefatto CLI (`bindings/cli-v1.json`).
pub const ARTEFATTO_CLI: &str = "plenora-data";
/// Nome dell'artefatto Rust che porta gli export di [`api`].
pub const ARTEFATTO_RUST: &str = "plenora-cli";
/// Distribuzione dell'SDK Python (Python SDK 1.0, sezione 2:
/// `plenora-<domain>`), costruita da `crates/plenora-data-py`.
pub const ARTEFATTO_PYTHON: &str = "plenora-data";
/// Pacchetto d'import dell'SDK Python (`plenora_<domain>`).
pub const IMPORT_PYTHON: &str = "plenora_data";
/// Versione del componente: quella del workspace.
pub const VERSIONE_COMPONENTE: &str = env!("CARGO_PKG_VERSION");
/// Versione del protocollo CLI adottato.
pub const PROTOCOLLO_CLI: u32 = 2;
