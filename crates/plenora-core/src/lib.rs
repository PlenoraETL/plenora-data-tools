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

pub use error::{ErrorCategory, PlenoraError, Result};

/// Re-export unico di Arrow: tutti i crate del workspace dipendono da Arrow
/// solo tramite questo modulo.
pub mod arrow {
    pub use arrow_array as array;
    pub use arrow_ipc as ipc;
    pub use arrow_schema as schema;
    pub use arrow_select as select;

    pub use arrow_array::RecordBatch;
    pub use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};

    /// Versione dei crate Arrow in uso (`arrow-schema` & co., unica per
    /// decisione D0). I crate Arrow non espongono la propria versione a
    /// runtime: la costante e' tenuta allineata al pin del workspace
    /// (`arrow-schema = "=…"` nel `Cargo.toml` di root) da un test dedicato.
    /// Entra nell'identita' dei grafi validati (ADR 4).
    pub const VERSION: &str = "59.1.0";
}

#[cfg(test)]
mod tests {
    /// `arrow::VERSION` deve restare allineata al pin del workspace: un bump
    /// di Arrow senza aggiornarla renderebbe silenziosamente falso il check
    /// di versione nell'identita' dei grafi (ADR 4).
    #[test]
    fn arrow_version_matches_the_workspace_pin() {
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
            .expect("Cargo.toml del workspace leggibile");
        let pin = format!("arrow-schema = \"={}\"", super::arrow::VERSION);
        assert!(
            manifest.contains(&pin),
            "pin di arrow-schema non allineato ad arrow::VERSION: atteso `{pin}`"
        );
    }
}
