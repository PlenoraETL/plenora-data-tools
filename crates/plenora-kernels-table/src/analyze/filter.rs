//! Analyzer a secco delle op di filtro e conditional (kernel `filtering.rs`).

use plenora_core::arrow::schema::{DataType, Field};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::Result;
use serde_json::Value;

use super::helpers::{
    analyze_append, check_json_text, check_output_name, con_op, contract_error, field_of,
    require_scalar_string_field, sorted_only, typed,
};
use crate::{filtering, scalar_compare_supported, Limits, NumericBound};

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

/// Un operatore di `filter`/`conditional` sulla colonna del suo tipo: i
/// rifiuti che `evaluate` del kernel darebbe alla prima riga non nulla,
/// qualunque sia il valore della cella.
///
/// - `==`/`!=` su Int64 e Float64 confrontano numeri: il valore atteso deve
///   esserlo (`NumericBound::parse`, lo stesso parse del kernel); su ogni
///   altro tipo confrontano testo, e la colonna deve essere leggibile come
///   scalare testuale;
/// - `contains`, `startswith`, `endswith` leggono la colonna come testo;
/// - `>`, `>=`, `<`, `<=` e `between` passano da `scalar_compare`: tipo della
///   colonna fra quelli che confronta ([`scalar_compare_supported`]), valore
///   atteso numerico, `between` nella forma `min,max`;
/// - `isnull`/`notnull` guardano solo la presenza del valore.
fn check_operator(
    op: &str,
    field: &Field,
    operator: &filtering::Operator,
    value: &Value,
) -> Result<()> {
    let expected = json_text(value);
    match operator {
        filtering::Operator::Eq | filtering::Operator::Ne => {
            if matches!(field.data_type(), DataType::Int64 | DataType::Float64) {
                if NumericBound::parse(&expected).is_none() {
                    return contract_error(op, "confronto numerico con valore non numerico");
                }
            } else {
                require_scalar_string_field(op, field)?;
            }
        }
        filtering::Operator::Contains
        | filtering::Operator::Startswith
        | filtering::Operator::Endswith => require_scalar_string_field(op, field)?,
        filtering::Operator::Gt
        | filtering::Operator::Ge
        | filtering::Operator::Lt
        | filtering::Operator::Le => {
            require_ordered(op, field)?;
            if NumericBound::parse(&expected).is_none() {
                return contract_error(op, "confronto ordinato richiede un valore numerico");
            }
        }
        filtering::Operator::Between => {
            require_ordered(op, field)?;
            let Some((low, high)) = expected.split_once(',') else {
                return contract_error(op, "between richiede min,max");
            };
            if NumericBound::parse(low.trim()).is_none()
                || NumericBound::parse(high.trim()).is_none()
            {
                return contract_error(op, "estremi between non numerici");
            }
        }
        filtering::Operator::Isnull | filtering::Operator::Notnull => {}
    }
    Ok(())
}

fn require_ordered(op: &str, field: &Field) -> Result<()> {
    if scalar_compare_supported(field.data_type()) {
        Ok(())
    } else {
        contract_error(
            op,
            format!(
                "colonna {} di tipo {}: nessun confronto ordinato",
                field.name(),
                plenora_core::tipo_arrow::descrivi_tipo(field.data_type())
            ),
        )
    }
}

pub(in crate::analyze) fn analyze_filter(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: filtering::Filter = typed(op, config)?;
    let input = &inputs[0];
    let _ = (fields, limits);
    let field = field_of(op, input, &config.column)?;
    con_op(
        op,
        filtering::verifica_valore(&config.operator, config.value.as_ref()),
    )?;
    check_operator(op, field, &config.operator, config.valore())?;
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
    limits: &Limits,
) -> Result<DataContract> {
    let config: filtering::Conditional = typed(op, config)?;
    let input = &inputs[0];
    let field = field_of(op, input, &config.column)?;
    // Senza condizioni ogni riga riceve il default: la config non esprime
    // alcuna condizione.
    if config.conditions.is_empty() {
        return contract_error(op, "conditional richiede almeno una condizione");
    }
    if config.conditions.len() > limits.max_columns {
        return contract_error(op, "numero di condizioni oltre il limite");
    }
    for condition in &config.conditions {
        con_op(
            op,
            filtering::verifica_valore(&condition.operator, condition.value.as_ref()),
        )?;
        check_operator(op, field, &condition.operator, condition.valore())?;
    }
    check_output_name(op, &config.output_column)?;
    con_op(op, config.verifica_risultati())?;
    // Risultati e default finiscono nelle celle.
    for condition in &config.conditions {
        check_json_text(op, &condition.result, limits.max_string_bytes, "result")?;
    }
    check_json_text(
        op,
        &config.default_value,
        limits.max_string_bytes,
        "default_value",
    )?;
    // Il tipo dipende solo dai letterali di config: tutti vuoti o numerici ->
    // Float64 nullable, altrimenti Utf8 non nullable. La regola e' quella del
    // kernel (`risultati_numerici`), con il suo rifiuto degli interi inesatti.
    let testi = config
        .conditions
        .iter()
        .map(|condition| json_text(&condition.result))
        .chain(std::iter::once(json_text(&config.default_value)))
        .collect::<Vec<_>>();
    let numeric = super::helpers::con_op(
        op,
        filtering::risultati_numerici(testi.iter().map(String::as_str)),
    )?
    .is_some();
    let (data_type, nullable) = if numeric {
        (DataType::Float64, true)
    } else {
        (DataType::Utf8, false)
    };
    analyze_append(
        input,
        fields,
        &[(config.output_column, data_type, nullable)],
    )
}
