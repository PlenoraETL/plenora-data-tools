use std::collections::HashSet;
use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, RecordBatch, UInt64Array};
use plenora_core::arrow::schema::DataType;
use regex::Regex;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};
use crate::{
    column_index, replace_or_append, scalar_as_f64, scalar_as_string, validate_output_name,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaExpectation {
    pub name: String,
    pub data_type: String,
    pub nullable: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertSchema {
    pub fields: Vec<SchemaExpectation>,
    #[serde(default)]
    pub allow_extra: bool,
    #[serde(default = "default_true")]
    pub ordered: bool,
}

const fn default_true() -> bool {
    true
}

fn expected_type(value: &str) -> Result<DataType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "utf8" | "string" => Ok(DataType::Utf8),
        "int64" | "integer" => Ok(DataType::Int64),
        "float64" | "float" | "double" => Ok(DataType::Float64),
        "boolean" | "bool" => Ok(DataType::Boolean),
        "uint64" | "unsigned" => Ok(DataType::UInt64),
        "date32" => Ok(DataType::Date32),
        "timestamp_millis" => Ok(DataType::Timestamp(
            plenora_core::arrow::schema::TimeUnit::Millisecond,
            None,
        )),
        "decimal128" => Ok(DataType::Decimal128(38, 0)),
        "binary" => Ok(DataType::Binary),
        "dictionary_utf8" => Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        )),
        "list" => Ok(DataType::List(Arc::new(plenora_core::arrow::schema::Field::new(
            "item",
            DataType::Null,
            true,
        )))),
        "struct" => Ok(DataType::Struct(plenora_core::arrow::schema::Fields::empty())),
        other => Err(PlenoraError::Contract(format!(
            "assert_schema: tipo non supportato {other}"
        ))),
    }
}

fn type_matches(actual: &DataType, expected: &DataType) -> bool {
    match expected {
        DataType::List(_) => matches!(actual, DataType::List(_)),
        DataType::Struct(_) => matches!(actual, DataType::Struct(_)),
        DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, None) => {
            matches!(
                actual,
                DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, _)
            )
        }
        DataType::Decimal128(_, _) => matches!(actual, DataType::Decimal128(_, _)),
        _ => actual == expected,
    }
}

pub fn assert_schema(batch: &RecordBatch, config: &AssertSchema) -> Result<RecordBatch> {
    if !config.allow_extra && batch.num_columns() != config.fields.len() {
        return Err(PlenoraError::Schema(format!(
            "assert_schema: attese {} colonne, trovate {}",
            config.fields.len(),
            batch.num_columns()
        )));
    }
    for (position, expectation) in config.fields.iter().enumerate() {
        let index = if config.ordered {
            position
        } else {
            column_index(batch, &expectation.name)?
        };
        let field = batch.schema().fields().get(index).cloned().ok_or_else(|| {
            PlenoraError::Schema(format!(
                "assert_schema: colonna mancante {}",
                expectation.name
            ))
        })?;
        if field.name() != &expectation.name {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: attesa {} in posizione {position}, trovata {}",
                expectation.name,
                field.name()
            )));
        }
        let expected = expected_type(&expectation.data_type)?;
        if !type_matches(field.data_type(), &expected) {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: tipo errato per {}: atteso {}, trovato {}",
                expectation.name,
                expectation.data_type,
                field.data_type()
            )));
        }
        if expectation
            .nullable
            .is_some_and(|nullable| nullable != field.is_nullable())
        {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: nullability errata per {}",
                expectation.name
            )));
        }
    }
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertNotNull {
    pub columns: Vec<String>,
}

pub fn assert_not_null(batch: &RecordBatch, config: &AssertNotNull) -> Result<RecordBatch> {
    for name in &config.columns {
        let index = column_index(batch, name)?;
        if let Some(row) = (0..batch.num_rows()).find(|row| batch.column(index).is_null(*row)) {
            return Err(PlenoraError::Contract(format!(
                "assert_not_null: null in {name} alla riga {row}"
            )));
        }
    }
    Ok(batch.clone())
}

pub fn key_for_row(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<Vec<u8>> {
    let mut key = Vec::new();
    for index in indices {
        let column = batch.column(*index);
        let type_name = column.data_type().to_string();
        let type_len = type_name.len() as u64;
        key.extend_from_slice(&type_len.to_be_bytes());
        key.extend_from_slice(type_name.as_bytes());
        match scalar_as_string(column.as_ref(), row)? {
            Some(value) => {
                key.push(1);
                let value_len = value.len() as u64;
                key.extend_from_slice(&value_len.to_be_bytes());
                key.extend_from_slice(value.as_bytes());
            }
            None => key.push(0),
        }
    }
    Ok(key)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertUnique {
    pub columns: Vec<String>,
    #[serde(default = "default_true")]
    pub nulls_equal: bool,
}

pub fn assert_unique(batch: &RecordBatch, config: &AssertUnique) -> Result<RecordBatch> {
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let mut seen = HashSet::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        if !config.nulls_equal
            && indices
                .iter()
                .any(|index| batch.column(*index).is_null(row))
        {
            continue;
        }
        if !seen.insert(key_for_row(batch, &indices, row)?) {
            return Err(PlenoraError::Contract(format!(
                "assert_unique: duplicato alla riga {row}"
            )));
        }
    }
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertRange {
    pub column: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    #[serde(default = "default_true")]
    pub inclusive_min: bool,
    #[serde(default = "default_true")]
    pub inclusive_max: bool,
    #[serde(default)]
    pub allow_null: bool,
}

pub fn assert_range(batch: &RecordBatch, config: &AssertRange) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    for row in 0..batch.num_rows() {
        let value = scalar_as_f64(batch.column(index).as_ref(), row)?;
        let Some(value) = value else {
            if config.allow_null {
                continue;
            }
            return Err(PlenoraError::Contract(format!(
                "assert_range: null alla riga {row}"
            )));
        };
        let below = config.min.is_some_and(|min| {
            if config.inclusive_min {
                value < min
            } else {
                value <= min
            }
        });
        let above = config.max.is_some_and(|max| {
            if config.inclusive_max {
                value > max
            } else {
                value >= max
            }
        });
        if !value.is_finite() || below || above {
            return Err(PlenoraError::Contract(format!(
                "assert_range: valore fuori intervallo alla riga {row}"
            )));
        }
    }
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertRegex {
    pub column: String,
    pub pattern: String,
    #[serde(default)]
    pub allow_null: bool,
}

pub fn assert_regex(batch: &RecordBatch, config: &AssertRegex) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    if batch.column(index).data_type() != &DataType::Utf8 {
        return Err(PlenoraError::Schema(
            "assert_regex richiede una colonna Utf8".into(),
        ));
    }
    let pattern = Regex::new(&config.pattern)
        .map_err(|error| PlenoraError::Contract(format!("regex non valida: {error}")))?;
    for row in 0..batch.num_rows() {
        match scalar_as_string(batch.column(index).as_ref(), row)? {
            Some(value) if pattern.is_match(&value) => {}
            None if config.allow_null => {}
            _ => {
                return Err(PlenoraError::Contract(format!(
                    "assert_regex: valore non conforme alla riga {row}"
                )))
            }
        }
    }
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coalesce {
    pub columns: Vec<String>,
    pub output_column: String,
}

pub fn coalesce(batch: &RecordBatch, config: &Coalesce) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    if config.columns.is_empty() {
        return Err(PlenoraError::Contract(
            "coalesce richiede almeno una colonna".into(),
        ));
    }
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let data_type = batch.column(indices[0]).data_type().clone();
    if indices
        .iter()
        .any(|index| batch.column(*index).data_type() != &data_type)
    {
        return Err(PlenoraError::Schema(
            "coalesce richiede colonne con tipi Arrow identici".into(),
        ));
    }
    // Fast path tipizzato (secondo batch ottimizzazioni kernel): copre Int64,
    // Float64, UInt64, Boolean, Utf8 con semantica identica al generico.
    if let Some(values) = crate::cleansing::coalesce_fast(batch, &indices) {
        return replace_or_append(batch, &config.output_column, data_type, true, values);
    }
    let values = coalesce_generic(batch, &indices)?;
    replace_or_append(batch, &config.output_column, data_type, true, values)
}

/// Percorso generico originale (concat + take): fallback per i tipi non
/// coperti da `cleansing::coalesce_fast` e oracolo dei test di equivalenza.
pub(crate) fn coalesce_generic(batch: &RecordBatch, indices: &[usize]) -> Result<ArrayRef> {
    let arrays = indices
        .iter()
        .map(|index| batch.column(*index).as_ref())
        .collect::<Vec<_>>();
    let combined = plenora_core::arrow::select::concat::concat(&arrays)?;
    let take_indices = (0..batch.num_rows())
        .map(|row| {
            indices
                .iter()
                .position(|index| !batch.column(*index).is_null(row))
                .map(|position| {
                    position
                        .checked_mul(batch.num_rows())
                        .and_then(|offset| offset.checked_add(row))
                        .ok_or_else(|| PlenoraError::Contract("overflow indice coalesce".into()))
                })
                .transpose()?
                .map(u64::try_from)
                .transpose()
                .map_err(|_| PlenoraError::Contract("indice coalesce oltre u64".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(plenora_core::arrow::select::take::take(
        combined.as_ref(),
        &UInt64Array::from(take_indices),
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{Int64Array, StringArray};
    use plenora_core::arrow::schema::{Field, Schema};

    use super::*;

    fn fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("text", DataType::Utf8, true),
                Field::new("id", DataType::Int64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("ok"), None])),
                Arc::new(Int64Array::from(vec![1, 2])),
            ],
        )
        .expect("quality fixture")
    }

    #[test]
    fn defensive_runtime_guards_remain_fail_closed_without_plan_validation() {
        let input = fixture();
        assert!(assert_regex(
            &input,
            &AssertRegex {
                column: "text".into(),
                pattern: "(".into(),
                allow_null: true,
            },
        )
        .is_err());
        assert!(assert_schema(
            &input,
            &AssertSchema {
                fields: vec![SchemaExpectation {
                    name: "text".into(),
                    data_type: "decimal128".into(),
                    nullable: None,
                }],
                allow_extra: true,
                ordered: true,
            },
        )
        .is_err());
        assert!(assert_not_null(
            &input,
            &AssertNotNull {
                columns: vec!["missing".into()],
            },
        )
        .is_err());
        assert!(coalesce(
            &input,
            &Coalesce {
                columns: Vec::new(),
                output_column: "result".into(),
            },
        )
        .is_err());
    }
}
