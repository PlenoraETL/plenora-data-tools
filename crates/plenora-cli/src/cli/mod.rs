//! Struttura interna della CLI.
//!
//! Parsing, dispatch, scoperta dei contratti, rendering, comandi a
//! compatibilita' congelata e gestione del processo stanno ciascuno nel
//! proprio modulo, e non in `main.rs`.
//!
//! Spostare codice fra questi moduli non deve cambiare cio' che l'utente
//! vede. Per le invocazioni **deterministiche e senza stato** — help,
//! versione, capability, catalogo, errori di invocazione — quel criterio e'
//! verificato byte per byte da `tests/oracolo_superficie_cli.rs`: se quel
//! confronto cambia, e' cambiata la CLI. Tutto cio' che dipende da file,
//! tempi o percorsi assoluti resta fuori da quella matrice, ed e' coperto
//! dagli oracoli che sanno normalizzarlo.

pub mod args;
pub mod commands;
pub mod contract_discovery;
pub mod error_envelope;
pub mod process;
pub mod rendering;
