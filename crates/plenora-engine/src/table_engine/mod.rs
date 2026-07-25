//! Engine tabellare: contratto del piano, validazione fail-closed ed
//! esecuzione della catena di kernel su `RecordBatch`.
//!
//! Port Fase 1 ("coesistenza") da `plenora-nogeo-tools/src/contract.rs` ed
//! `engine.rs`, senza modifiche di comportamento:
//!
//! - `Plan`/`Step`/`ValidatedPlan` e la validazione del contratto
//!   (`schema_version`, limiti, regola "catena binaria = 1 step", config
//!   fail-closed per le 66 operazioni tabellari);
//! - `execute_batch` ed `execute_binary` con normalizzazione
//!   `LargeUtf8 -> Utf8` e rimozione dei metadata `pandas`.
//!
//! Adattamenti rispetto al sorgente (nessuno dei quali cambia il
//! comportamento sui piani legacy):
//!
//! - gli errori `EngineError` sono mappati su [`plenora_core::PlenoraError`]
//!   (la variante `Step { index, .. }` diventa `Step { node: index
//!   .to_string(), .. }`; i messaggi Display seguono il formato inglese di
//!   `PlenoraError`);
//! - `Limits` NON e' duplicato: e' riusato [`plenora_kernels_table::Limits`],
//!   struct identica a quella storica di `contract.rs` (stessi campi, stessi
//!   default, stessi attributi serde) gia' usata dai kernel. La validazione
//!   dei valori resta qui (`validate_limits`);
//! - gli id operazione "nudi" dei piani legacy (es. `filter`) sono risolti
//!   verso il catalogo unificato con
//!   [`plenora_core::catalog::find_operation`] (che copre anche gli alias
//!   storici) e filtrati su `Family::Table`: gli id geo restano "operazione
//!   sconosciuta" esattamente come nel catalogo storico di nogeo. Gli id
//!   canonici `table.*` sono accettati come superset e ricondotti al nome
//!   nudo prima del dispatch ([`dispatch_name`]);
//! - `inputs == 2` del catalogo storico corrisponde ad `arity != Unary`
//!   (11 `BinaryOrdered` + `concat` `NAry`); `execution != Streaming`
//!   corrisponde a `execution_class != Streaming`.
//!
//! L'unificazione con il trasporto geo in un DAG unico e' Fase 2.

mod contract;
mod executor;

pub use contract::{dispatch_name, Plan, Step, ValidatedPlan, SCHEMA_VERSION};
pub use executor::{execute_batch, execute_binary};
pub use plenora_kernels_table::Limits;
