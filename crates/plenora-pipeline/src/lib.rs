//! plenora-pipeline — runner minimo di pipeline sui kernel tabellari e geo.
//!
//! Un piano ([`Pipeline`]) nomina le tabelle in ingresso, una sequenza di
//! passi in forma SSA (ogni nome definito una volta) e le tabelle in uscita.
//! [`Pipeline::validate`] controlla tutto ciò che si può dire senza i dati,
//! contro gli schemi degli input, e rende una [`PipelineValidata`];
//! [`PipelineValidata::run`] la esegue sulle tabelle e rende un [`Esito`]
//! con gli output e un [`Report`] per passo (righe, byte nuovi, byte vivi,
//! previsione del budget, tabelle liberate).
//!
//! Il budget di memoria (`max_governed_memory_bytes`) si applica passo per
//! passo con il modello di costo di [`budget`], generato dalle misure in
//! [`costi_operazioni`]: ogni tabella si libera appena ha girato il suo
//! ultimo consumatore, e un passo che non sta si rifiuta prima di eseguirlo.
//! Niente va su disco: non c'è ripiego per un passo che non sta.
//!
//! Le tabelle sono intere in memoria, un `RecordBatch` per nome: niente
//! streaming. Le operazioni geo ([`geo`]) passano dall'analisi e dai kernel
//! di `plenora-kernels-geo`, con lo stesso budget.

pub mod budget;
pub mod costi_geo;
pub mod costi_operazioni;
mod dispatch;
pub mod disponibilita;
mod esecuzione;
mod geo;
pub mod piano;
#[doc(hidden)]
pub mod sonda;
mod validazione;

pub use esecuzione::{Esito, Interruzione, Report, ReportPasso};
pub use piano::{LimitiParziali, Passo, Pipeline, VERSIONE_PIANO};
pub use plenora_core::memoria::byte_vivi;
pub use validazione::{normalizza_schema, BaseIndici, PipelineValidata};
