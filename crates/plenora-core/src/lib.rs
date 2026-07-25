//! plenora-core — fondamenta condivise del workspace (Architetture.md par. 3.1).
//!
//! Ospita:
//! - il re-export unico di Arrow (decisione D0: un solo punto di versione);
//! - [`error`]: `PlenoraError`, fusione di `EngineError` e `GeoEngineError`;
//! - [`limits`]: le tre famiglie di limiti (decisione D19, ADR 6);
//! - [`catalog`]: `OperationDescriptor` unificato con versioni per-componente
//!   (decisione D17, ADR 4);
//! - [`contract`]: contratti dati del grafo (`DataContract`, `FieldId`,
//!   provenienza/scope delle proprietà, `RuntimeStatistic`, `BatchSequence`) —
//!   Fase 2A, decisioni D6/D16/D25, ADR 1 e ADR 5;
//! - [`crs`]: contratto CRS fail-closed (trasloco da `crs.rs` di
//!   plenora-geo-tools-arrow previsto in Fase 1).

pub mod catalog;
pub mod contract;
pub mod crs;
pub mod error;
pub mod limits;

pub use error::{PlenoraError, Result};

/// Re-export unico di Arrow: tutti i crate del workspace dipendono da Arrow
/// solo tramite questo modulo.
pub mod arrow {
    pub use arrow_array as array;
    pub use arrow_ipc as ipc;
    pub use arrow_schema as schema;
    pub use arrow_select as select;

    pub use arrow_array::RecordBatch;
    pub use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
}
