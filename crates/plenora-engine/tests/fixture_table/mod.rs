//! Le impalcature tabellari condivise dai test di integrazione.
//!
//! Come `fixture_geo`, qui sta **solo la costruzione**: il piano da
//! validare e l'accesso tipizzato alle colonne di un batch. L'atteso e il
//! confronto restano in ogni prova, che li ricava per conto suo.
//!
//! Ogni file ne usa una parte: la dichiarazione del modulo porta un
//! `allow(dead_code)` mirato, perche' il resto non e' codice morto ma codice
//! di un altro file.

use plenora_core::arrow::array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_engine::table_engine::SCHEMA_VERSION;
use plenora_engine::{Limits, Plan, Step, ValidatedPlan};
use serde_json::Value;

/// Il piano con i passi dati, ancora da validare.
pub fn piano_a_passi(passi: Vec<(&str, Value)>, limits: Limits) -> Plan {
    Plan {
        schema_version: SCHEMA_VERSION,
        limits,
        steps: passi
            .into_iter()
            .map(|(operation, config)| Step {
                operation: operation.into(),
                config,
            })
            .collect(),
    }
}

/// Il piano a un passo, ancora da validare: chi prova un rifiuto lo valida da
/// se'.
pub fn piano(operation: &str, config: Value, limits: Limits) -> Plan {
    piano_a_passi(vec![(operation, config)], limits)
}

/// Il piano a un passo validato. Un rifiuto e' un panico che nomina
/// l'operazione.
pub fn plan_with_limits(operation: &str, config: Value, limits: Limits) -> ValidatedPlan {
    piano(operation, config, limits)
        .validate()
        .unwrap_or_else(|error| panic!("{operation}: {error}"))
}

/// Come [`plan_with_limits`], con i limiti di default.
pub fn plan(operation: &str, config: Value) -> ValidatedPlan {
    plan_with_limits(operation, config, Limits::default())
}

/// La colonna `name` di `batch`, con il tipo concreto atteso.
pub fn colonna<'a, A: Array + 'static>(batch: &'a RecordBatch, name: &str) -> &'a A {
    batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("colonna `{name}` assente"))
        .as_any()
        .downcast_ref()
        .unwrap_or_else(|| panic!("colonna `{name}` di tipo inatteso"))
}

/// La colonna `name` come Utf8.
pub fn utf8<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    colonna(batch, name)
}

/// La colonna `name` come Int64.
pub fn i64s<'a>(batch: &'a RecordBatch, name: &str) -> &'a Int64Array {
    colonna(batch, name)
}

/// La colonna `name` come Float64.
pub fn f64s<'a>(batch: &'a RecordBatch, name: &str) -> &'a Float64Array {
    colonna(batch, name)
}
