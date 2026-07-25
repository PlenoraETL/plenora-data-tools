use std::collections::HashSet;
use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, Array, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
    UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Schema};
use serde::Deserialize;

use crate::Limits;
use plenora_core::{PlenoraError, Result};
use crate::select_rows;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetOperation {}

enum KeyColumn<'a> {
    Utf8(&'a StringArray),
    Int64(&'a Int64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    UInt64(&'a UInt64Array),
    Date32(&'a Date32Array),
    TimestampMillis(&'a TimestampMillisecondArray),
    Decimal128(&'a Decimal128Array),
    Binary(&'a BinaryArray),
    DictionaryUtf8(&'a DictionaryArray<Int32Type>, &'a StringArray),
}

impl KeyColumn<'_> {
    fn encode(&self, row: usize, output: &mut Vec<u8>) -> Result<()> {
        let is_null = match self {
            Self::Utf8(values) => values.is_null(row),
            Self::Int64(values) => values.is_null(row),
            Self::Float64(values) => values.is_null(row),
            Self::Boolean(values) => values.is_null(row),
            Self::UInt64(values) => values.is_null(row),
            Self::Date32(values) => values.is_null(row),
            Self::TimestampMillis(values) => values.is_null(row),
            Self::Decimal128(values) => values.is_null(row),
            Self::Binary(values) => values.is_null(row),
            Self::DictionaryUtf8(values, _) => values.is_null(row),
        };
        if is_null {
            output.push(0);
            return Ok(());
        }
        output.push(1);
        match self {
            Self::Utf8(values) => {
                let value = values.value(row).as_bytes();
                let length = value.len() as u64;
                output.extend_from_slice(&length.to_be_bytes());
                output.extend_from_slice(value);
            }
            Self::Int64(values) => output.extend_from_slice(&values.value(row).to_be_bytes()),
            Self::Float64(values) => {
                let value = values.value(row);
                // The previous string representation treated every NaN as the
                // same value but kept +0 and -0 distinct. Preserve that exact
                // contract while avoiding a temporary String allocation.
                let bits = if value.is_nan() {
                    f64::NAN.to_bits()
                } else {
                    value.to_bits()
                };
                output.extend_from_slice(&bits.to_be_bytes());
            }
            Self::Boolean(values) => output.push(u8::from(values.value(row))),
            Self::UInt64(values) => output.extend_from_slice(&values.value(row).to_be_bytes()),
            Self::Date32(values) => output.extend_from_slice(&values.value(row).to_be_bytes()),
            Self::TimestampMillis(values) => {
                output.extend_from_slice(&values.value(row).to_be_bytes());
            }
            Self::Decimal128(values) => {
                output.extend_from_slice(&values.value(row).to_be_bytes());
            }
            Self::Binary(values) => encode_variable(values.value(row), output),
            Self::DictionaryUtf8(values, dictionary) => {
                let key = usize::try_from(values.keys().value(row))
                    .map_err(|_| PlenoraError::Schema("chiave dictionary negativa".into()))?;
                encode_variable(dictionary.value(key).as_bytes(), output);
            }
        }
        Ok(())
    }
}

fn encode_variable(value: &[u8], output: &mut Vec<u8>) {
    let length = value.len() as u64;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
}

pub struct CompactRowEncoder<'a> {
    columns: Vec<KeyColumn<'a>>,
}

impl<'a> CompactRowEncoder<'a> {
    pub fn try_new(batch: &'a RecordBatch) -> Result<Self> {
        let columns = batch
            .columns()
            .iter()
            .map(|column| match column.data_type() {
                DataType::Utf8 => column
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .map(KeyColumn::Utf8)
                    .ok_or_else(|| PlenoraError::Schema("array Utf8 incoerente".into())),
                DataType::Int64 => column
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .map(KeyColumn::Int64)
                    .ok_or_else(|| PlenoraError::Schema("array Int64 incoerente".into())),
                DataType::Float64 => column
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .map(KeyColumn::Float64)
                    .ok_or_else(|| PlenoraError::Schema("array Float64 incoerente".into())),
                DataType::Boolean => column
                    .as_any()
                    .downcast_ref::<BooleanArray>()
                    .map(KeyColumn::Boolean)
                    .ok_or_else(|| PlenoraError::Schema("array Boolean incoerente".into())),
                DataType::UInt64 => column
                    .as_any()
                    .downcast_ref::<UInt64Array>()
                    .map(KeyColumn::UInt64)
                    .ok_or_else(|| PlenoraError::Schema("array UInt64 incoerente".into())),
                DataType::Date32 => column
                    .as_any()
                    .downcast_ref::<Date32Array>()
                    .map(KeyColumn::Date32)
                    .ok_or_else(|| PlenoraError::Schema("array Date32 incoerente".into())),
                DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, _) => column
                    .as_any()
                    .downcast_ref::<TimestampMillisecondArray>()
                    .map(KeyColumn::TimestampMillis)
                    .ok_or_else(|| PlenoraError::Schema("array TimestampMillis incoerente".into())),
                DataType::Decimal128(_, _) => column
                    .as_any()
                    .downcast_ref::<Decimal128Array>()
                    .map(KeyColumn::Decimal128)
                    .ok_or_else(|| PlenoraError::Schema("array Decimal128 incoerente".into())),
                DataType::Binary => column
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .map(KeyColumn::Binary)
                    .ok_or_else(|| PlenoraError::Schema("array Binary incoerente".into())),
                DataType::Dictionary(key, value)
                    if key.as_ref() == &DataType::Int32 && value.as_ref() == &DataType::Utf8 =>
                {
                    let values = column
                        .as_any()
                        .downcast_ref::<DictionaryArray<Int32Type>>()
                        .ok_or_else(|| PlenoraError::Schema("array Dictionary incoerente".into()))?;
                    let dictionary = values
                        .values()
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| PlenoraError::Schema("dictionary non Utf8".into()))?;
                    Ok(KeyColumn::DictionaryUtf8(values, dictionary))
                }
                other => Err(PlenoraError::Schema(format!(
                    "tipo {other:?} non supportato dalle set operation"
                ))),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { columns })
    }

    pub fn encode_into(&self, row: usize, output: &mut Vec<u8>) -> Result<()> {
        output.clear();
        for column in &self.columns {
            column.encode(row, output)?;
        }
        Ok(())
    }
}

pub fn validate_schema(left: &RecordBatch, right: &RecordBatch) -> Result<()> {
    if left.num_columns() != right.num_columns()
        || left
            .schema()
            .fields()
            .iter()
            .zip(right.schema().fields())
            .any(|(left, right)| {
                left.name() != right.name() || left.data_type() != right.data_type()
            })
    {
        return Err(PlenoraError::Schema(
            "set operation richiede nomi e tipi Arrow identici".into(),
        ));
    }
    Ok(())
}

pub fn concat_compatible(
    left: &RecordBatch,
    right: &RecordBatch,
    limits: &Limits,
) -> Result<RecordBatch> {
    validate_schema(left, right)?;
    let rows = left
        .num_rows()
        .checked_add(right.num_rows())
        .ok_or_else(|| PlenoraError::Contract("overflow union_distinct".into()))?;
    if rows > limits.max_rows {
        return Err(PlenoraError::Contract(
            "union_distinct supera max_rows".into(),
        ));
    }
    let columns = left
        .columns()
        .iter()
        .zip(right.columns())
        .map(|(left, right)| plenora_core::arrow::select::concat::concat(&[left.as_ref(), right.as_ref()]))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let fields = left
        .schema()
        .fields()
        .iter()
        .zip(right.schema().fields())
        .map(|(left, right)| {
            left.as_ref()
                .clone()
                .with_nullable(left.is_nullable() || right.is_nullable())
        })
        .collect::<Vec<_>>();
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            left.schema().metadata().clone(),
        )),
        columns,
    )?)
}

fn unique_rows(batch: &RecordBatch, predicate: impl Fn(&[u8]) -> bool) -> Result<Vec<usize>> {
    let encoder = CompactRowEncoder::try_new(batch)?;
    let mut emitted: HashSet<Box<[u8]>> = HashSet::new();
    let mut rows = Vec::new();
    let mut key = Vec::new();
    for row in 0..batch.num_rows() {
        encoder.encode_into(row, &mut key)?;
        if predicate(&key) && !emitted.contains(key.as_slice()) {
            emitted.insert(key.into_boxed_slice());
            rows.push(row);
            key = Vec::new();
        }
    }
    Ok(rows)
}

pub fn union_distinct(
    left: &RecordBatch,
    right: &RecordBatch,
    _config: &SetOperation,
    limits: &Limits,
) -> Result<RecordBatch> {
    let combined = concat_compatible(left, right, limits)?;
    let rows = unique_rows(&combined, |_| true)?;
    select_rows(&combined, &rows)
}

fn right_keys(right: &RecordBatch) -> Result<HashSet<Box<[u8]>>> {
    let encoder = CompactRowEncoder::try_new(right)?;
    let mut keys = HashSet::with_capacity(right.num_rows());
    let mut key = Vec::new();
    for row in 0..right.num_rows() {
        encoder.encode_into(row, &mut key)?;
        if !keys.contains(key.as_slice()) {
            keys.insert(key.into_boxed_slice());
            key = Vec::new();
        }
    }
    Ok(keys)
}

pub fn intersect(
    left: &RecordBatch,
    right: &RecordBatch,
    _config: &SetOperation,
) -> Result<RecordBatch> {
    validate_schema(left, right)?;
    let mut right = right_keys(right)?;
    let encoder = CompactRowEncoder::try_new(left)?;
    let mut rows = Vec::with_capacity(left.num_rows().min(right.len()));
    let mut key = Vec::new();
    for row in 0..left.num_rows() {
        encoder.encode_into(row, &mut key)?;
        // Removing the exact byte key both proves membership and guarantees
        // DISTINCT semantics without retaining a second HashSet for the left.
        if right.remove(key.as_slice()) {
            rows.push(row);
        }
    }
    select_rows(left, &rows)
}

pub fn except(
    left: &RecordBatch,
    right: &RecordBatch,
    _config: &SetOperation,
) -> Result<RecordBatch> {
    validate_schema(left, right)?;
    let right = right_keys(right)?;
    let rows = unique_rows(left, |key| !right.contains(key))?;
    select_rows(left, &rows)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::schema::{DataType, Field};

    use super::*;

    #[test]
    fn compact_keys_are_unambiguous_and_preserve_float_contract() {
        let nan_a = f64::from_bits(0x7ff8_0000_0000_0001);
        let nan_b = f64::from_bits(0x7ff8_0000_0000_0042);
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("left", DataType::Utf8, true),
                Field::new("right", DataType::Utf8, true),
                Field::new("number", DataType::Int64, false),
                Field::new("float", DataType::Float64, false),
                Field::new("flag", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("ab"),
                    Some("a"),
                    Some(""),
                    None,
                    Some("same"),
                    Some("same"),
                    Some("same"),
                    Some("same"),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("c"),
                    Some("bc"),
                    None,
                    Some(""),
                    Some("same"),
                    Some("same"),
                    Some("same"),
                    Some("same"),
                ])),
                Arc::new(Int64Array::from(vec![7; 8])),
                Arc::new(Float64Array::from(vec![
                    1.0, 1.0, 1.0, 1.0, nan_a, nan_b, 0.0, -0.0,
                ])),
                Arc::new(BooleanArray::from(vec![true; 8])),
            ],
        )
        .expect("set key fixture");
        let encoder = CompactRowEncoder::try_new(&batch).expect("encoder");
        let keys = (0..batch.num_rows())
            .map(|row| {
                let mut key = Vec::new();
                encoder.encode_into(row, &mut key).expect("key");
                key
            })
            .collect::<Vec<_>>();

        assert_ne!(keys[0], keys[1], "column boundaries must be framed");
        assert_ne!(keys[2], keys[3], "null and empty must remain distinct");
        assert_eq!(keys[4], keys[5], "all NaN payloads share set semantics");
        assert_ne!(keys[6], keys[7], "+0 and -0 preserve the prior contract");
    }
}
