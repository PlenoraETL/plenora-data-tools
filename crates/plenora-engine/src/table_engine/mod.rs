//! Engine tabellare: contratto del piano, validazione fail-closed ed
//! esecuzione della catena di kernel su `RecordBatch`.
//!
//! Superficie a compatibilita' congelata: messaggi ed esiti sono quelli che
//! i piani legacy si aspettano.
//!
//! - gli errori sono [`plenora_core::PlenoraError`]; l'indice del passo
//!   diventa il nodo (`node: index.to_string()`);
//! - `Limits` e' [`plenora_kernels_table::Limits`]; la validazione dei valori
//!   resta qui (`validate_limits`);
//! - gli id "nudi" dei piani legacy si risolvono con
//!   [`plenora_core::catalog::find_operation`] filtrando su `Family::Table`,
//!   cosi' un id geo resta "operazione sconosciuta"; gli id `table.*` sono
//!   accettati e ricondotti al nome nudo ([`dispatch_name`]).
//!
//! Spill (architettura.md#memoria): `sort`/`distinct`/`aggregate` passano
//! alla variante `*_spilled` quando i byte stimati dell'input superano
//! `max_governed_memory_bytes`; [`execute_batch_with_spill`] instrada i file
//! nella directory del `TempStore` e raccoglie le `SpillMetrics`.

mod contract;
mod executor;

pub use contract::{dispatch_name, Plan, Step, ValidatedPlan, SCHEMA_VERSION};
pub(crate) use executor::execute_batch_with_spill_row_diagnostics;
pub(crate) use executor::unary_spill_capable;
pub use executor::{
    execute_batch, execute_batch_with_spill, execute_binary, execute_complete_batch,
};
pub use plenora_kernels_table::Limits;
