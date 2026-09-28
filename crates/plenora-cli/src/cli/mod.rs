//! Struttura interna della CLI.
//!
//! Spostare codice fra questi moduli non deve cambiare cio' che l'utente
//! vede: per le invocazioni deterministiche e senza stato lo verifica byte
//! per byte `tests/oracolo_superficie_cli.rs`; cio' che dipende da file,
//! tempi o percorsi e' coperto dagli oracoli che sanno normalizzarlo.

pub mod args;
pub mod commands;
pub mod contract_discovery;
pub mod error_envelope;
pub mod formato;
pub mod piano;
pub mod process;
pub mod pubblicazione;
pub mod rendering;
pub mod segnali;
