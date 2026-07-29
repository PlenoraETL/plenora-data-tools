//! Analyzer a secco delle op di quality e governance
//! (kernel `quality.rs` / `governance.rs`).

use std::collections::HashSet;
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};
use plenora_core::contract::{
    ContractProperties, ContractProperty, DataContract, FieldAllocator, PropertyConfidence,
    PropertyScope,
};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::filter::json_text;
use super::helpers::{
    analyze_append, check_output_name, contract_error, field_of, finish, require_numeric,
    require_scalar_string, require_utf8, typed,
};
use crate::{governance, quality, Limits};

// ---------------------------------------------------------------------------
// quality.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_assert_schema(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertSchema = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    if !config.allow_extra && input.schema.fields().len() != config.fields.len() {
        return contract_error(
            op,
            format!(
                "attese {} colonne, trovate {}",
                config.fields.len(),
                input.schema.fields().len()
            ),
        );
    }
    for (position, expectation) in config.fields.iter().enumerate() {
        let field = if config.ordered {
            input.schema.fields().get(position).ok_or_else(|| {
                PlenoraError::InvalidPlan(format!(
                    "{op}: colonna mancante in posizione {position}"
                ))
            })?
        } else {
            input
                .schema
                .field_with_name(&expectation.name)
                .map_err(|_| {
                    PlenoraError::InvalidPlan(format!(
                        "{op}: colonna mancante {}",
                        expectation.name
                    ))
                })?
        };
        if field.name() != &expectation.name {
            return contract_error(
                op,
                format!(
                    "attesa {} in posizione {position}, trovata {}",
                    expectation.name,
                    field.name()
                ),
            );
        }
        let expected = expected_type(op, &expectation.data_type)?;
        if !type_matches(field.data_type(), &expected) {
            return contract_error(
                op,
                format!(
                    "tipo errato per {}: atteso {}, trovato {}",
                    expectation.name,
                    expectation.data_type,
                    field.data_type()
                ),
            );
        }
        if expectation
            .nullable
            .is_some_and(|nullable| nullable != field.is_nullable())
        {
            return contract_error(
                op,
                format!("nullability errata per {}", expectation.name),
            );
        }
    }
    Ok(input.clone())
}

/// Replica `expected_type` del kernel quality.
fn expected_type(op: &str, value: &str) -> Result<DataType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "utf8" | "string" => Ok(DataType::Utf8),
        "int64" | "integer" => Ok(DataType::Int64),
        "float64" | "float" | "double" => Ok(DataType::Float64),
        "boolean" | "bool" => Ok(DataType::Boolean),
        "uint64" | "unsigned" => Ok(DataType::UInt64),
        "date32" => Ok(DataType::Date32),
        "timestamp_millis" => Ok(DataType::Timestamp(TimeUnit::Millisecond, None)),
        "decimal128" => Ok(DataType::Decimal128(38, 0)),
        "binary" => Ok(DataType::Binary),
        "dictionary_utf8" => Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        )),
        "list" => Ok(DataType::List(Arc::new(Field::new(
            "item",
            DataType::Null,
            true,
        )))),
        "struct" => Ok(DataType::Struct(plenora_core::arrow::schema::Fields::empty())),
        other => contract_error(op, format!("tipo non supportato {other}")),
    }
}

/// Replica `type_matches` del kernel quality (famiglie per tipi parametrici).
fn type_matches(actual: &DataType, expected: &DataType) -> bool {
    match expected {
        DataType::List(_) => matches!(actual, DataType::List(_)),
        DataType::Struct(_) => matches!(actual, DataType::Struct(_)),
        DataType::Timestamp(TimeUnit::Millisecond, None) => {
            matches!(actual, DataType::Timestamp(TimeUnit::Millisecond, _))
        }
        DataType::Decimal128(_, _) => matches!(actual, DataType::Decimal128(_, _)),
        _ => actual == expected,
    }
}

pub(in crate::analyze) fn analyze_assert_not_null(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertNotNull = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    for name in &config.columns {
        field_of(op, input, name)?;
    }
    Ok(input.clone())
}

pub(in crate::analyze) fn analyze_assert_unique(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertUnique = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    for name in &config.columns {
        require_scalar_string(op, input, name)?;
    }
    Ok(input.clone())
}

pub(in crate::analyze) fn analyze_assert_range(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertRange = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    require_numeric(op, input, &config.column)?;
    Ok(input.clone())
}

pub(in crate::analyze) fn analyze_assert_regex(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertRegex = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    require_utf8(op, input, &config.column)?;
    regex::Regex::new(&config.pattern)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: regex non valida: {error}")))?;
    Ok(input.clone())
}

pub(in crate::analyze) fn analyze_coalesce(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::Coalesce = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.columns.is_empty() {
        return contract_error(op, "coalesce richiede almeno una colonna");
    }
    let data_type = field_of(op, input, &config.columns[0])?.data_type().clone();
    for name in &config.columns[1..] {
        let field = field_of(op, input, name)?;
        if field.data_type() != &data_type {
            return contract_error(op, "coalesce richiede colonne con tipi Arrow identici");
        }
    }
    analyze_append(input, fields, &[(config.output_column, data_type, true)])
}

// ---------------------------------------------------------------------------
// governance.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_assert_cardinality(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::AssertCardinality = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    // Con row_count Proven in input la cardinalita' e' verificabile a secco.
    if let Some(proven) = input
        .properties
        .row_count
        .as_ref()
        .and_then(|property| property.confidence.proven_value())
    {
        let rows = usize::try_from(*proven).unwrap_or(usize::MAX);
        let violated = config.exact_rows.map_or_else(
            || {
                config.min_rows.is_some_and(|min| min > rows)
                    || config.max_rows.is_some_and(|max| max < rows)
            },
            |exact| exact != rows,
        );
        if violated {
            return contract_error(
                op,
                format!("cardinalita' attestata incompatibile con row_count Proven({proven})"),
            );
        }
    }
    Ok(input.clone())
}

pub(in crate::analyze) fn analyze_assert_metadata(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::AssertMetadata = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let metadata = input.schema.metadata();
    for (key, value) in &config.expected {
        if metadata.get(key) != Some(value) {
            return contract_error(op, format!("metadata {key:?} non conforme"));
        }
    }
    if !config.allow_extra && metadata.len() != config.expected.len() {
        return contract_error(op, "metadata extra non ammessi");
    }
    Ok(input.clone())
}

/// Chiavi di un'op binaria: esistenza nei due schemi e tipi delle coppie
/// identici (zip come i kernel governance, senza check di pari cardinalita').
pub(in crate::analyze) fn check_foreign_keys(
    op: &str,
    left: &DataContract,
    right: &DataContract,
    left_keys: &[String],
    right_keys: &[String],
) -> Result<()> {
    for (left_key, right_key) in left_keys.iter().zip(right_keys) {
        let left_field = field_of(op, left, left_key)?;
        let right_field = field_of(op, right, right_key)?;
        if left_field.data_type() != right_field.data_type() {
            return contract_error(
                op,
                format!("chiavi {left_key}/{right_key} con tipi Arrow diversi"),
            );
        }
    }
    Ok(())
}

pub(in crate::analyze) fn analyze_assert_foreign_key(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::ForeignKey = typed(op, config)?;
    let _ = fields;
    check_foreign_keys(op, &inputs[0], &inputs[1], &config.left_keys, &config.right_keys)?;
    // Right non contribuisce allo schema: output = left invariato.
    Ok(inputs[0].clone())
}

pub(in crate::analyze) fn analyze_reconcile(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::Reconcile = typed(op, config)?;
    let _ = fields;
    check_foreign_keys(op, &inputs[0], &inputs[1], &config.left_keys, &config.right_keys)?;
    // Schema fisso: 5 righe di metriche, indipendente dagli input.
    // R2.4: dataset derivato — nessuna colonna degli input sopravvive e i
    // metadata di schema degli input NON si ereditano (descriverebbero il
    // risultato con le proprieta' dell'ingresso, R5.1). Deroga segnalata.
    let schema = Schema::new(vec![
        Field::new("metric", DataType::Utf8, false),
        Field::new("value", DataType::UInt64, false),
    ]);
    finish(
        schema,
        None,
        None,
        ContractProperties {
            sorted_by: None,
            row_count: Some(ContractProperty::new(
                PropertyConfidence::Proven(5),
                PropertyScope::Dataset,
            )),
        },
    )
}

// Controlli fail-closed in sequenza lineare: lunghezza intrinseca, non complessita'.
#[allow(clippy::too_many_lines)]
pub(in crate::analyze) fn analyze_validate_rules(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::ValidateRules = typed(op, config)?;
    let input = &inputs[0];
    if config.rules.is_empty() {
        return contract_error(op, "rules vuoto");
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for rule in &config.rules {
        if rule.name.trim().is_empty() {
            return contract_error(op, "nome regola vuoto");
        }
        if !seen.insert(rule.name.as_str()) {
            return contract_error(op, format!("regola ripetuta: {}", rule.name));
        }
        let Some(column) = rule.column.as_deref() else {
            return contract_error(op, format!("regola {} senza column", rule.name));
        };
        let field = field_of(op, input, column)?;
        let needs_value = !matches!(
            rule.operator,
            governance::RuleOperator::Isnull | governance::RuleOperator::Notnull
        );
        if needs_value != rule.value.is_some() {
            return contract_error(
                op,
                format!(
                    "regola {}: value {} per l'operatore",
                    rule.name,
                    if needs_value { "obbligatorio" } else { "non ammesso" }
                ),
            );
        }
        let expected = rule.value.as_ref().map_or_else(String::new, json_text);
        match rule.operator {
            governance::RuleOperator::Isnull | governance::RuleOperator::Notnull => {}
            governance::RuleOperator::Eq | governance::RuleOperator::Ne => {
                if !governance::is_rule_comparable(field.data_type()) {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: tipo {:?} non confrontabile",
                            rule.name,
                            field.data_type()
                        ),
                    );
                }
                if governance::is_rule_numeric(field.data_type())
                    && expected.parse::<f64>().is_err()
                {
                    return contract_error(
                        op,
                        format!("regola {}: confronto numerico con valore non numerico", rule.name),
                    );
                }
            }
            governance::RuleOperator::Gt
            | governance::RuleOperator::Ge
            | governance::RuleOperator::Lt
            | governance::RuleOperator::Le => {
                if !governance::is_rule_numeric(field.data_type()) {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: confronto ordinato richiede colonna numerica (tipo {:?})",
                            rule.name,
                            field.data_type()
                        ),
                    );
                }
                if expected.parse::<f64>().is_err() {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: confronto ordinato richiede un valore numerico",
                            rule.name
                        ),
                    );
                }
            }
            governance::RuleOperator::Range => {
                if !governance::is_rule_numeric(field.data_type()) {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: range richiede colonna numerica (tipo {:?})",
                            rule.name,
                            field.data_type()
                        ),
                    );
                }
                let Some((low, high)) = expected.split_once(',') else {
                    return contract_error(op, format!("regola {}: range richiede min,max", rule.name));
                };
                if low.trim().parse::<f64>().is_err() || high.trim().parse::<f64>().is_err() {
                    return contract_error(
                        op,
                        format!("regola {}: estremi range non numerici", rule.name),
                    );
                }
            }
            governance::RuleOperator::Regex => {
                if field.data_type() != &DataType::Utf8 {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: regex richiede colonna Utf8 (tipo {:?})",
                            rule.name,
                            field.data_type()
                        ),
                    );
                }
                if expected.len() > Limits::default().max_regex_bytes {
                    return contract_error(op, format!("regola {}: pattern oltre max_regex_bytes", rule.name));
                }
                regex::Regex::new(&expected).map_err(|error| {
                    PlenoraError::InvalidPlan(format!(
                        "{op}: regola {}: regex non valida: {error}",
                        rule.name
                    ))
                })?;
            }
        }
    }
    match config.output_mode {
        governance::ValidateOutputMode::Annotate => analyze_append(
            input,
            fields,
            &[
                ("_valid".to_owned(), DataType::Boolean, false),
                ("_errors".to_owned(), DataType::Utf8, false),
                ("_warnings".to_owned(), DataType::Utf8, false),
            ],
        ),
        governance::ValidateOutputMode::Summary => {
            // Dataset nuovo: una riga per regola, nessuna colonna d'input.
            // R2.4: dataset derivato — i metadata di schema dell'input NON si
            // ereditano (come `reconcile`, R5.1). Deroga segnalata.
            for name in ["name", "errors", "warnings"] {
                fields.derive(name);
            }
            let schema = Schema::new(vec![
                Field::new("name", DataType::Utf8, false),
                Field::new("errors", DataType::Int64, false),
                Field::new("warnings", DataType::Int64, false),
            ]);
            finish(schema, None, None, ContractProperties::default())
        }
    }
}

