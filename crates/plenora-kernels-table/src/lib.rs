//! plenora-kernels-table — kernel tabellari puri `&RecordBatch -> Result<RecordBatch>`
//! (Architetture.md par. 3.2).
//!
//! Fase 1 "coesistenza": trasloco meccanico dei 17 moduli kernel da
//! `plenora-nogeo-tools/src/kernels/` (`columns`, `strings`, `cleansing`,
//! `filtering`, `dates`, `utility`, `analysis`, `aggregation`, `reshape`,
//! `joins`, `setops`, `security`, `quality`, `governance`, `formula`,
//! `expressions`, `spill`) con gli helper condivisi, senza modifiche di
//! comportamento.

use serde::{Deserialize, Serialize};

/// Limiti dei kernel tabellari, traslocati identici da
/// `plenora-nogeo-tools/src/contract.rs` (Fase 1, zero modifiche di
/// comportamento).
///
/// NOTA (punto aperto per la fase engine): il `Limits` unificato di
/// `plenora_core::limits` (decisione D19, ADR 6) non copre `max_columns` e
/// `max_split_columns`, e sostituisce il singolo `max_rows` con la famiglia
/// semantica `RowLimits` (`max_input_rows` / `max_output_rows` /
/// `max_rows_per_edge`). La mappatura di questa struct su
/// `plenora_core::limits::Limits` e' una decisione semantica demandata alla
/// fase engine, non un adattamento meccanico.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_rows: usize,
    pub max_columns: usize,
    pub max_string_bytes: usize,
    pub max_regex_bytes: usize,
    pub max_split_columns: usize,
    pub max_memory_bytes: usize,
    pub max_temp_bytes: u64,
    pub spill_partitions: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rows: 10_000_000,
            max_columns: 4_096,
            max_string_bytes: 16 * 1024 * 1024,
            max_regex_bytes: 4_096,
            max_split_columns: 256,
            max_memory_bytes: 512 * 1024 * 1024,
            max_temp_bytes: 8 * 1024 * 1024 * 1024,
            spill_partitions: 64,
        }
    }
}

pub mod aggregation;
pub mod analysis;
pub mod analyze;
pub mod cleansing;
pub mod columns;
pub mod dates;
pub mod expressions;
pub mod filtering;
pub mod formula;
pub mod fuzzy;
pub mod governance;
pub mod joins;
pub mod quality;
pub mod reshape;
pub mod security;
pub mod setops;
pub mod spill;
pub mod strings;
pub mod utility;

use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
    UInt32Array, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use num_traits::ToPrimitive;

use plenora_core::{PlenoraError, Result};

pub fn column_index(batch: &RecordBatch, name: &str) -> Result<usize> {
    batch
        .schema()
        .index_of(name)
        .map_err(|_| PlenoraError::Schema(format!("colonna non trovata: {name}")))
}

pub fn utf8_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a plenora_core::arrow::array::StringArray> {
    let index = column_index(batch, name)?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::StringArray>()
        .ok_or_else(|| PlenoraError::Schema(format!("la colonna {name} deve essere Utf8")))
}

pub fn replace_or_append(
    batch: &RecordBatch,
    name: &str,
    data_type: DataType,
    nullable: bool,
    array: ArrayRef,
) -> Result<RecordBatch> {
    if array.len() != batch.num_rows() {
        return Err(PlenoraError::Schema(format!(
            "lunghezza output {} diversa dalle righe {}",
            array.len(),
            batch.num_rows()
        )));
    }
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    let mut columns = batch.columns().to_vec();
    if let Ok(index) = batch.schema().index_of(name) {
        fields[index] = Field::new(name, data_type, nullable);
        columns[index] = array;
    } else {
        fields.push(Field::new(name, data_type, nullable));
        columns.push(array);
    }
    let schema = Schema::new_with_metadata(fields, batch.schema().metadata().clone());
    Ok(RecordBatch::try_new(Arc::new(schema), columns)?)
}

pub fn validate_output_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(PlenoraError::Contract(
            "il nome della colonna di output e' vuoto".into(),
        ));
    }
    if name.len() > 1_024 {
        return Err(PlenoraError::Contract(
            "il nome della colonna supera 1024 byte".into(),
        ));
    }
    Ok(())
}

pub fn scalar_as_string(array: &dyn Array, row: usize) -> Result<Option<String>> {
    if array.is_null(row) {
        return Ok(None);
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(Some(values.value(row).to_owned()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Date32Array>() {
        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
            .ok_or_else(|| PlenoraError::Contract("epoch date32 non valida".into()))?;
        let date = epoch
            .checked_add_signed(chrono::TimeDelta::days(i64::from(values.value(row))))
            .ok_or_else(|| PlenoraError::Schema("date32 fuori intervallo".into()))?;
        return Ok(Some(date.format("%Y-%m-%d").to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<TimestampMillisecondArray>() {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(values.value(row))
            .ok_or_else(|| PlenoraError::Schema("timestamp fuori intervallo".into()))?;
        if let DataType::Timestamp(_, Some(timezone)) = values.data_type() {
            let timezone = timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| PlenoraError::Schema("timezone Arrow non valida".into()))?;
            return Ok(Some(timestamp.with_timezone(&timezone).to_rfc3339()));
        }
        return Ok(Some(timestamp.to_rfc3339()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        let value = values.value(row);
        let scale = u32::try_from(*scale)
            .map_err(|_| PlenoraError::Schema("scala decimal negativa non supportata".into()))?;
        let factor = 10_i128
            .checked_pow(scale)
            .ok_or_else(|| PlenoraError::Schema("scala decimal fuori intervallo".into()))?;
        let magnitude = value.unsigned_abs();
        let whole = magnitude / factor.unsigned_abs();
        let fraction = magnitude % factor.unsigned_abs();
        let sign = if value < 0 { "-" } else { "" };
        return if scale == 0 {
            Ok(Some(format!("{sign}{whole}")))
        } else {
            Ok(Some(format!(
                "{sign}{whole}.{fraction:0width$}",
                width = usize::try_from(scale).unwrap_or_default()
            )))
        };
    }
    if let Some(values) = array.as_any().downcast_ref::<BinaryArray>() {
        return std::str::from_utf8(values.value(row))
            .map(|value| Some(value.to_owned()))
            .map_err(|_| PlenoraError::Schema("binary non contiene UTF-8 valido".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>() {
        let dictionary = values
            .values()
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| PlenoraError::Schema("dictionary non contiene Utf8".into()))?;
        let key = usize::try_from(values.keys().value(row))
            .map_err(|_| PlenoraError::Schema("chiave dictionary negativa".into()))?;
        return Ok(Some(dictionary.value(key).to_owned()));
    }
    Err(PlenoraError::Schema(format!(
        "tipo {:?} non supportato dal profilo scalare",
        array.data_type()
    )))
}

pub fn scalar_as_f64(array: &dyn Array, row: usize) -> Result<Option<f64>> {
    if array.is_null(row) {
        return Ok(None);
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return Ok(Some(values.value(row)));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return values
            .value(row)
            .to_f64()
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("intero non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return values
            .value(row)
            .to_f64()
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("uint64 non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Date32Array>() {
        return Ok(Some(f64::from(values.value(row))));
    }
    if let Some(values) = array.as_any().downcast_ref::<TimestampMillisecondArray>() {
        return values
            .value(row)
            .to_f64()
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("timestamp non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        let factor = 10_f64.powi(i32::from(*scale));
        return values
            .value(row)
            .to_f64()
            .map(|value| Some(value / factor))
            .ok_or_else(|| PlenoraError::Schema("decimal128 non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return values
            .value(row)
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .map(Some)
            .map_err(|_| PlenoraError::Schema("valore non convertibile in numero".into()));
    }
    Err(PlenoraError::Schema(format!(
        "tipo {:?} non convertibile in numero",
        array.data_type()
    )))
}

pub fn select_rows(batch: &RecordBatch, rows: &[usize]) -> Result<RecordBatch> {
    let indices: UInt32Array = rows
        .iter()
        .map(|row| {
            u32::try_from(*row).map_err(|_| PlenoraError::Contract("indice riga oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?
        .into();
    let columns = batch
        .columns()
        .iter()
        .map(|column| {
            plenora_core::arrow::select::take::take(column.as_ref(), &indices, None).map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RecordBatch::try_new(batch.schema(), columns)?)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{Int64Array, StringArray};
    use plenora_core::arrow::schema::{DataType, Field, Schema};

    use super::*;

    fn batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("a", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![Some("x"), None]))],
        )
        .expect("fixture")
    }

    #[test]
    fn helper_guards_cover_type_length_and_names() {
        let input = batch();
        assert!(replace_or_append(
            &input,
            "bad",
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(vec![Some("only one")]))
        )
        .is_err());
        assert!(validate_output_name(" ").is_err());
        assert!(validate_output_name(&"x".repeat(1_025)).is_err());
        assert!(column_index(&input, "missing").is_err());

        let integers = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1]))],
        )
        .expect("integers");
        assert!(utf8_column(&integers, "n").is_err());
    }
}
