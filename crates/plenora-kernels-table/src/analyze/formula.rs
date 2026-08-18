//! Analyzer a secco di `formula` ed `expression`
//! (kernel `formula.rs` / `expressions.rs`).
//!
//! Le regole di tipo NON vivono qui: stanno accanto ai kernel che le
//! applicano (`expressions::static_type` e `formula::column_formula_type`), e
//! questo modulo si limita a chiamarle con lo schema del contratto. E' la
//! stessa funzione che il kernel usa per decidere il tipo della colonna
//! prodotta, quindi contratto dichiarato e schema prodotto non possono
//! divergere.

use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{analyze_append, check_output_name, field_of, typed};
use crate::{expressions, formula, Limits};

/// Numero massimo di nodi AST accettati nell'audit di `table.expression`
/// (limite statico dell'analisi a secco; il kernel riceve il valore dal
/// chiamante, qui non esiste un `Limits` dedicato).
const MAX_EXPRESSION_NODES: usize = 4_096;

// ---------------------------------------------------------------------------
// formula.rs / expressions.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_formula(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: formula::Formula = typed(op, config)?;
    let input = &inputs[0];
    formula::validate(&config, Limits::default().max_string_bytes)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: {error}")))?;
    // La stessa classificazione che il kernel applica allo schema del batch.
    let inferred = formula::infer_formula_type(&config, &|name| {
        let field = field_of(op, input, name)?;
        formula::column_formula_type(field.data_type(), name)
            .map_err(|errore| PlenoraError::InvalidPlan(format!("{op}: {errore}")))
    })?;
    analyze_append(
        input,
        fields,
        &[(config.new_column, inferred.data_type(), true)],
    )
}

pub(in crate::analyze) fn analyze_expression(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: expressions::ExpressionTransform = typed(op, config)?;
    let input = &inputs[0];
    expressions::validate(&config, MAX_EXPRESSION_NODES)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: {error}")))?;
    check_output_name(op, &config.output_column)?;
    // L'AST si analizza SEMPRE, anche quando `output_type` e' dichiarato: il
    // tipo dichiarato non dice niente sulle colonne referenziate ne' sugli
    // operandi.
    let possibili = expressions::static_type::infer(op, &config.expression, &|name| {
        field_of(op, input, name).map(|field| field.data_type().clone())
    })?;
    let kind = expressions::static_type::resolve_output(op, possibili, config.output_type)?;
    analyze_append(
        input,
        fields,
        &[(config.output_column, kind.data_type(), true)],
    )
}
