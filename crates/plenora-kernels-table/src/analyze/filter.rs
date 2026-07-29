//! Analyzer a secco delle op di filtro e conditional (kernel `filtering.rs`).

use plenora_core::arrow::schema::DataType;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::Result;
use serde_json::Value;

use super::helpers::{analyze_append, contract_error, field_of, sorted_only, typed};
use crate::filtering;

// ---------------------------------------------------------------------------
// filtering.rs
// ---------------------------------------------------------------------------

/// Replica `json_text` del kernel filtering.
pub(in crate::analyze) fn json_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Validazioni config-dipendenti di `filter`/`conditional` (valori attesi
/// numerici per confronti ordinati, formato `min,max` di Between).
fn check_operator_value(op: &str, operator: &filtering::Operator, value: &Value) -> Result<()> {
    let expected = json_text(value);
    match operator {
        filtering::Operator::Gt
        | filtering::Operator::Ge
        | filtering::Operator::Lt
        | filtering::Operator::Le => {
            if expected.parse::<f64>().is_err() {
                return contract_error(op, "confronto ordinato richiede un valore numerico");
            }
        }
        filtering::Operator::Between => {
            let Some((low, high)) = expected.split_once(',') else {
                return contract_error(op, "between richiede min,max");
            };
            if low.trim().parse::<f64>().is_err() || high.trim().parse::<f64>().is_err() {
                return contract_error(op, "estremi between non numerici");
            }
        }
        _ => {}
    }
    Ok(())
}

pub(in crate::analyze) fn analyze_filter(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: filtering::Filter = typed(op, config)?;
    let input = &inputs[0];
    let field = field_of(op, input, &config.column)?;
    // Eq/Ne su colonna numerica con valore non numerico: errore certo a runtime.
    if matches!(config.operator, filtering::Operator::Eq | filtering::Operator::Ne)
        && matches!(field.data_type(), DataType::Int64 | DataType::Float64)
        && json_text(&config.value).parse::<f64>().is_err()
    {
        return contract_error(op, "confronto numerico con valore non numerico");
    }
    check_operator_value(op, &config.operator, &config.value)?;
    let _ = fields;
    // Righe rimosse, ordine relativo e schema invariati.
    let mut output = input.clone();
    output.properties = sorted_only(input);
    Ok(output)
}

pub(in crate::analyze) fn analyze_conditional(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: filtering::Conditional = typed(op, config)?;
    let input = &inputs[0];
    field_of(op, input, &config.column)?;
    for condition in &config.conditions {
        check_operator_value(op, &condition.operator, &condition.value)?;
    }
    // Il tipo dipende solo dai letterali di config: tutti vuoti o numerici ->
    // Float64 nullable, altrimenti Utf8 non nullable.
    let numeric = config
        .conditions
        .iter()
        .map(|condition| json_text(&condition.result))
        .chain(std::iter::once(json_text(&config.default_value)))
        .all(|text| text.is_empty() || text.replace(',', ".").parse::<f64>().is_ok());
    let (data_type, nullable) = if numeric {
        (DataType::Float64, true)
    } else {
        (DataType::Utf8, false)
    };
    analyze_append(input, fields, &[(config.output_column, data_type, nullable)])
}

