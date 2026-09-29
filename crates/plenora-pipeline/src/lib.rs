//! plenora-pipeline — runner minimo di pipeline sui kernel tabellari.
//!
//! Un piano ([`Pipeline`]) nomina le tabelle in ingresso, una sequenza di
//! passi in forma SSA (ogni nome definito una volta) e le tabelle in uscita.
//! [`Pipeline::validate`] controlla tutto ciò che si può dire senza i dati,
//! contro gli schemi degli input, e rende una [`PipelineValidata`];
//! [`PipelineValidata::run`] la esegue sulle tabelle e rende un [`Esito`]
//! con gli output e un [`Report`] per passo (righe, byte nuovi, byte vivi).
//!
//! Le tabelle sono intere in memoria, un `RecordBatch` per nome: niente
//! streaming. Le operazioni geo non sono ancora nel dispatch e si rifiutano
//! in validazione.

mod dispatch;
mod esecuzione;
pub mod piano;
mod validazione;
mod verifica_config;

pub use esecuzione::{Esito, Report, ReportPasso};
pub use piano::{LimitiParziali, Passo, Pipeline, VERSIONE_PIANO};
pub use plenora_core::memoria::byte_vivi;
pub use validazione::PipelineValidata;
