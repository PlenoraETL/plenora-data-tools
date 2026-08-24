//! Struttura interna della CLI.
//!
//! `main.rs` era un file unico da oltre cinquemila righe in cui convivevano
//! parsing, dispatch, scoperta dei contratti, rendering, i comandi ereditati
//! dai due binari di origine e il processo. Questo modulo lo scompone, un
//! pezzo per volta e **senza cambiare comportamento**: il criterio e' che
//! `tests/oracolo_superficie_cli.snap` non cambi di un byte.
//!
//! Chi legge un diff di questa fase deve poterlo verificare guardando che
//! il codice sia lo stesso, non ragionando su che cosa faccia.

pub mod args;
pub mod commands;
pub mod contract_discovery;
pub mod error_envelope;
pub mod process;
pub mod rendering;
