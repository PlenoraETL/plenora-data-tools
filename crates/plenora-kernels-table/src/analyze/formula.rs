//! Analyzer a secco di `formula` ed `expression`
//! (kernel `formula.rs` / `expressions/`).
//!
//! Le regole di tipo stanno accanto ai kernel (`expressions::static_type` e
//! `formula::column_formula_type`): questo modulo le chiama con lo schema del
//! contratto, quindi contratto dichiarato e schema prodotto non divergono.

use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{analyze_append, check_output_name, con_op, field_of, typed};
use crate::{expressions, formula, Limits};

/// Numero massimo di nodi AST accettati nell'audit di `table.expression`
/// (limite statico dell'analisi a secco: `Limits` non ha un campo dedicato,
/// e il kernel non ripete l'audit).
const MAX_EXPRESSION_NODES: usize = 4_096;

// ---------------------------------------------------------------------------
// formula.rs / expressions.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_formula(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: formula::Formula = typed(op, config)?;
    let input = &inputs[0];
    formula::validate(&config, limits.max_string_bytes)
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
    limits: &Limits,
) -> Result<DataContract> {
    let config: expressions::ExpressionTransform = typed(op, config)?;
    let input = &inputs[0];
    expressions::validate(&config, MAX_EXPRESSION_NODES)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: {error}")))?;
    // Le stesse regole del kernel: `on_division_by_zero` senza divisioni,
    // divisore letterale zero (prima l'analisi non lo vedeva e il piano
    // falliva solo in esecuzione), letterali oltre i limiti.
    con_op(op, config.verifica_parametri(limits))?;
    check_output_name(op, &config.output_column)?;
    // L'AST si analizza SEMPRE, anche quando `output_type` e' dichiarato: il
    // tipo dichiarato non dice niente sulle colonne referenziate ne' sugli
    // operandi.
    let lookup = |name: &str| field_of(op, input, name).map(|field| field.data_type().clone());
    expressions::static_type::verifica_domini_temporali(op, &config.expression, &lookup)?;
    let possibili = expressions::static_type::infer(op, &config.expression, &lookup)?;
    let kind = expressions::static_type::resolve_output(op, possibili, config.output_type)?;
    expressions::static_type::verifica_letterali(op, &config.expression, &|name| {
        field_of(op, input, name).map(|field| field.data_type().clone())
    })?;
    analyze_append(
        input,
        fields,
        &[(config.output_column, kind.data_type(), true)],
    )
}
