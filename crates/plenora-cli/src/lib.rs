//! plenora-cli — la CLI pubblica `plenora-data` e la superficie Rust delle
//! stesse operazioni.
//!
//! Adotta il profilo data-tools di `plenora-contracts` (revisione
//! `ade868cf89c6652cffe20019e7194b383384ee78`): CLI 2.0 (un documento JSON
//! su stdout, niente su stderr, codici d'uscita per categoria), Typed Errors
//! 1.0 (la proiezione pubblica di `PlenoraError`), Capability Discovery 2.0,
//! Surface Bindings 1.0 (comandi `catalog`, `describe`, `validate`, `run`).
//! Le deviazioni volute sono nel README, «CLI `plenora-data`».
//!
//! Struttura:
//!
//! - [`operazioni`]: la tabella delle quattro operazioni pubbliche, unica
//!   fonte di comandi, aiuto, capacità e mappa degli export Rust;
//! - [`catalogo`]: il registro dei kernel (`data.catalog`), derivato dal
//!   catalogo di `plenora-core`;
//! - [`api`]: le operazioni come funzioni Rust, che la CLI chiama;
//! - [`capacita`]: Capability Discovery 2.0 e mappa Rust;
//! - [`argomenti`], [`inviluppo`], [`esegui_invocazione`]: la CLI.
//!
//! `main.rs` installa l'hook di panico silenzioso e il gestore di Ctrl-C,
//! chiama [`esegui_invocazione`] e scrive il risultato.

pub mod api;
pub mod argomenti;
pub mod capacita;
pub mod catalogo;
mod cli;
pub mod inviluppo;
pub mod operazioni;

pub use cli::{
    esegui_invocazione, esegui_invocazione_os, testo_aiuto, Segnale, CONTRATTO_AIUTO,
    CONTRATTO_VERSIONE,
};
pub use inviluppo::Uscita;

/// Identificatore stabile del componente (Public Surfaces 1.0, SURF-001).
pub const COMPONENTE: &str = "plenora-data-tools";
/// Nome dell'artefatto CLI (`bindings/cli-v1.json`).
pub const ARTEFATTO_CLI: &str = "plenora-data";
/// Nome dell'artefatto Rust che porta gli export di [`api`].
pub const ARTEFATTO_RUST: &str = "plenora-cli";
/// Versione del componente: quella del workspace.
pub const VERSIONE_COMPONENTE: &str = env!("CARGO_PKG_VERSION");
/// Versione del protocollo CLI adottato.
pub const PROTOCOLLO_CLI: u32 = 2;
