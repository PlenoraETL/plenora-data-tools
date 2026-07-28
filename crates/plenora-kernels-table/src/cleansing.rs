use std::sync::Arc;

use plenora_core::arrow::array::{
    builder::{BinaryBuilder, BooleanBuilder, PrimitiveBuilder, StringBuilder, StringDictionaryBuilder},
    types::{ArrowPrimitiveType, Float64Type, Int32Type, Int64Type, UInt64Type},
    Array, ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int64Array,
    PrimitiveArray, RecordBatch, StringArray, TimestampMillisecondArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use chrono::{DateTime, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use num_traits::ToPrimitive;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use plenora_core::{PlenoraError, Result};
use crate::{column_index, replace_or_append, scalar_as_string};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillMethod {
    Value,
    Ffill,
    Bfill,
}

const fn default_fill_method() -> FillMethod {
    FillMethod::Value
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillNa {
    pub column: Option<String>,
    #[serde(default = "default_fill_method")]
    pub method: FillMethod,
    #[serde(default)]
    pub value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replace {
    pub column: String,
    pub old_value: String,
    pub new_value: String,
    #[serde(default)]
    pub regex: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    Str,
    Int,
    Float,
    Bool,
    Date,
    Datetime,
    Date32,
    TimestampMillis,
    Decimal128,
    BinaryUtf8,
    Uint64,
    DictionaryUtf8,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CastErrors {
    Coerce,
    Raise,
    Ignore,
}

const fn default_errors() -> CastErrors {
    CastErrors::Coerce
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeCast {
    pub column: String,
    #[serde(default = "default_target")]
    pub target_type: TargetType,
    #[serde(default)]
    pub date_format: String,
    #[serde(default = "default_errors")]
    pub errors: CastErrors,
    #[serde(default)]
    pub precision: Option<u8>,
    #[serde(default)]
    pub scale: Option<i8>,
    #[serde(default)]
    pub timezone: Option<String>,
}

const fn default_target() -> TargetType {
    TargetType::Str
}

// ---------------------------------------------------------------------------
// Fast path tipizzati (secondo batch ottimizzazioni kernel, dopo filter/sort).
//
// fill_na: i percorsi originali materializzavano `Vec<Option<T>>` con un clone
// per riga (stringhe incluse) e ricostruivano l'array; qui si lavora sui
// valori nativi Arrow: buffer valori clonato + scrittura dei soli slot nulli
// (method=value), una passata per ffill/bfill, e copia dell'Arc quando
// l'operazione e' l'identita' (nessun null, o fill con `null`). Semantica
// IDENTICA all'originale: stessi errori per valori di fill non validi, stesso
// errore Schema per i tipi non coperti (UInt64 incluso), null in testa (ffill)
// o in coda (bfill) che restano null.
// ---------------------------------------------------------------------------

fn utf8_data_len(values: &StringArray) -> usize {
    let offsets = values.offsets();
    usize::try_from(offsets[values.len()] - offsets[0]).unwrap_or(0)
}

fn fill_utf8(values: &StringArray, method: &FillMethod, value: &Value) -> ArrayRef {
    let fixed = match value {
        Value::Null => None,
        Value::String(v) => Some(v.clone()),
        other => Some(other.to_string()),
    };
    if values.null_count() == 0 || (matches!(method, FillMethod::Value) && fixed.is_none()) {
        return Arc::new(values.clone());
    }
    match method {
        FillMethod::Value => {
            // La guardia sopra esce gia' per `Value` con fill `null`
            // (identita'): qui `fixed` e' sempre `Some`, come impone il tipo.
            let Some(fixed) = fixed else {
                return Arc::new(values.clone());
            };
            let capacity = utf8_data_len(values) + fixed.len() * values.null_count();
            let mut builder = StringBuilder::with_capacity(values.len(), capacity);
            for row in 0..values.len() {
                if values.is_null(row) {
                    builder.append_value(&fixed);
                } else {
                    builder.append_value(values.value(row));
                }
            }
            Arc::new(builder.finish())
        }
        FillMethod::Ffill => {
            let mut builder = StringBuilder::with_capacity(values.len(), utf8_data_len(values));
            let mut previous: Option<&str> = None;
            for row in 0..values.len() {
                if values.is_null(row) {
                    match previous {
                        Some(value) => builder.append_value(value),
                        None => builder.append_null(),
                    }
                } else {
                    let value = values.value(row);
                    builder.append_value(value);
                    previous = Some(value);
                }
            }
            Arc::new(builder.finish())
        }
        FillMethod::Bfill => {
            let mut out: Vec<Option<&str>> = vec![None; values.len()];
            let mut following = None;
            for row in (0..values.len()).rev() {
                if values.is_null(row) {
                    out[row] = following;
                } else {
                    let value = values.value(row);
                    out[row] = Some(value);
                    following = Some(value);
                }
            }
            Arc::new(StringArray::from(out))
        }
    }
}

fn fill_primitive<T>(
    values: &PrimitiveArray<T>,
    method: &FillMethod,
    fixed: Option<T::Native>,
) -> ArrayRef
where
    T: ArrowPrimitiveType,
{
    if values.null_count() == 0 || (matches!(method, FillMethod::Value) && fixed.is_none()) {
        return Arc::new(values.clone());
    }
    match method {
        FillMethod::Value => {
            // La guardia sopra esce gia' per `Value` con fill `null`
            // (identita'): qui `fixed` e' sempre `Some`, come impone il tipo.
            let Some(fixed) = fixed else {
                return Arc::new(values.clone());
            };
            let mut buffer = values.values().to_vec();
            for (row, slot) in buffer.iter_mut().enumerate() {
                if values.is_null(row) {
                    *slot = fixed;
                }
            }
            Arc::new(PrimitiveArray::<T>::new(buffer.into(), None))
        }
        FillMethod::Ffill => {
            let mut builder = PrimitiveBuilder::<T>::with_capacity(values.len());
            let mut previous = None;
            for row in 0..values.len() {
                if values.is_null(row) {
                    match previous {
                        Some(value) => builder.append_value(value),
                        None => builder.append_null(),
                    }
                } else {
                    let value = values.value(row);
                    builder.append_value(value);
                    previous = Some(value);
                }
            }
            Arc::new(builder.finish())
        }
        FillMethod::Bfill => {
            let mut out = vec![None; values.len()];
            let mut following = None;
            for row in (0..values.len()).rev() {
                if values.is_null(row) {
                    out[row] = following;
                } else {
                    let value = values.value(row);
                    out[row] = Some(value);
                    following = Some(value);
                }
            }
            Arc::new(out.into_iter().collect::<PrimitiveArray<T>>())
        }
    }
}

fn fill_boolean(values: &BooleanArray, method: &FillMethod, fixed: Option<bool>) -> ArrayRef {
    if values.null_count() == 0 || (matches!(method, FillMethod::Value) && fixed.is_none()) {
        return Arc::new(values.clone());
    }
    match method {
        FillMethod::Value => {
            // La guardia sopra esce gia' per `Value` con fill `null`
            // (identita'): qui `fixed` e' sempre `Some`, come impone il tipo.
            let Some(fixed) = fixed else {
                return Arc::new(values.clone());
            };
            let out: Vec<bool> = (0..values.len())
                .map(|row| if values.is_null(row) { fixed } else { values.value(row) })
                .collect();
            Arc::new(BooleanArray::from(out))
        }
        FillMethod::Ffill => {
            let mut out = Vec::with_capacity(values.len());
            let mut previous = None;
            for row in 0..values.len() {
                if values.is_null(row) {
                    out.push(previous);
                } else {
                    let value = values.value(row);
                    out.push(Some(value));
                    previous = Some(value);
                }
            }
            Arc::new(BooleanArray::from(out))
        }
        FillMethod::Bfill => {
            let mut out = vec![None; values.len()];
            let mut following = None;
            for row in (0..values.len()).rev() {
                if values.is_null(row) {
                    out[row] = following;
                } else {
                    let value = values.value(row);
                    out[row] = Some(value);
                    following = Some(value);
                }
            }
            Arc::new(BooleanArray::from(out))
        }
    }
}

fn fill_array(array: &dyn Array, method: &FillMethod, value: &Value) -> Result<ArrayRef> {
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(fill_utf8(values, method, value));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        let fixed = match value {
            Value::Null => None,
            Value::Number(n) => n.as_i64(),
            Value::String(s) => Some(
                s.parse()
                    .map_err(|_| PlenoraError::Contract("fill int non valido".into()))?,
            ),
            _ => return Err(PlenoraError::Contract("fill int non valido".into())),
        };
        return Ok(fill_primitive(values, method, fixed));
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        let fixed = match value {
            Value::Null => None,
            Value::Number(n) => n.as_f64(),
            Value::String(s) => Some(
                s.replace(',', ".")
                    .parse()
                    .map_err(|_| PlenoraError::Contract("fill float non valido".into()))?,
            ),
            _ => return Err(PlenoraError::Contract("fill float non valido".into())),
        };
        return Ok(fill_primitive(values, method, fixed));
    }
    if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
        let fixed = match value {
            Value::Null => None,
            Value::Bool(v) => Some(*v),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
            _ => return Err(PlenoraError::Contract("fill bool non valido".into())),
        };
        return Ok(fill_boolean(values, method, fixed));
    }
    Err(PlenoraError::Schema(format!(
        "fill_na non supporta {:?}",
        array.data_type()
    )))
}

pub fn fill_na(batch: &RecordBatch, config: &FillNa) -> Result<RecordBatch> {
    let targets: Vec<usize> = if let Some(name) = &config.column {
        vec![column_index(batch, name)?]
    } else {
        (0..batch.num_columns()).collect()
    };
    let mut out = batch.clone();
    for index in targets {
        let name = out.schema().field(index).name().clone();
        let array = fill_array(out.column(index).as_ref(), &config.method, &config.value)?;
        out = replace_or_append(&out, &name, array.data_type().clone(), true, array)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Fast path per `table.coalesce`: il kernel vive in quality.rs, che chiama
// `coalesce_fast` e ricade sul generico concat+take quando torna `None`.
// Per ogni riga vince il primo valore non nullo nell'ordine delle colonne,
// come il `position` + `take` del generico; se la prima colonna non ha null
// l'output e' la colonna stessa (take con indici tutti validi e' l'identita').
// Coperti: Int64, Float64, UInt64, Boolean, Utf8; gli altri tipi vanno al
// generico. Null handling e ordine colonne identici all'originale.
// ---------------------------------------------------------------------------

fn coalesce_primitive<T>(batch: &RecordBatch, indices: &[usize]) -> Option<ArrayRef>
where
    T: ArrowPrimitiveType,
{
    let columns = indices
        .iter()
        .map(|index| {
            batch
                .column(*index)
                .as_any()
                .downcast_ref::<PrimitiveArray<T>>()
        })
        .collect::<Option<Vec<_>>>()?;
    let mut builder = PrimitiveBuilder::<T>::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        match columns.iter().find(|column| !column.is_null(row)) {
            Some(column) => builder.append_value(column.value(row)),
            None => builder.append_null(),
        }
    }
    Some(Arc::new(builder.finish()))
}

#[must_use]
pub fn coalesce_fast(batch: &RecordBatch, indices: &[usize]) -> Option<ArrayRef> {
    let first = batch.column(indices[0]);
    if first.null_count() == 0 {
        return Some(first.clone());
    }
    match first.data_type() {
        DataType::Int64 => coalesce_primitive::<Int64Type>(batch, indices),
        DataType::Float64 => coalesce_primitive::<Float64Type>(batch, indices),
        DataType::UInt64 => coalesce_primitive::<UInt64Type>(batch, indices),
        DataType::Boolean => {
            let columns = indices
                .iter()
                .map(|index| batch.column(*index).as_any().downcast_ref::<BooleanArray>())
                .collect::<Option<Vec<_>>>()?;
            let mut builder = BooleanBuilder::with_capacity(batch.num_rows());
            for row in 0..batch.num_rows() {
                match columns.iter().find(|column| !column.is_null(row)) {
                    Some(column) => builder.append_value(column.value(row)),
                    None => builder.append_null(),
                }
            }
            Some(Arc::new(builder.finish()))
        }
        DataType::Utf8 => {
            let columns = indices
                .iter()
                .map(|index| batch.column(*index).as_any().downcast_ref::<StringArray>())
                .collect::<Option<Vec<_>>>()?;
            let capacity = columns.first().map_or(0, |column| utf8_data_len(column));
            let mut builder = StringBuilder::with_capacity(batch.num_rows(), capacity);
            for row in 0..batch.num_rows() {
                match columns.iter().find(|column| !column.is_null(row)) {
                    Some(column) => builder.append_value(column.value(row)),
                    None => builder.append_null(),
                }
            }
            Some(Arc::new(builder.finish()))
        }
        _ => None,
    }
}

pub fn replace(batch: &RecordBatch, config: &Replace) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let values = batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PlenoraError::Schema("replace safe profile richiede Utf8".into()))?;
    let regex = config
        .regex
        .then(|| Regex::new(&config.old_value))
        .transpose()
        .map_err(|e| PlenoraError::Contract(format!("regex non valida: {e}")))?;
    let out: StringArray = values
        .iter()
        .map(|item| {
            item.map(|text| {
                regex.as_ref().map_or_else(
                    || {
                        if text == config.old_value {
                            config.new_value.clone()
                        } else {
                            text.to_owned()
                        }
                    },
                    |pattern| {
                        pattern
                            .replace_all(text, config.new_value.as_str())
                            .into_owned()
                    },
                )
            })
        })
        .collect();
    replace_or_append(batch, &config.column, DataType::Utf8, true, Arc::new(out))
}

fn cast_failure<T>(errors: &CastErrors, message: &str) -> Result<Option<T>> {
    match errors {
        CastErrors::Coerce => Ok(None),
        CastErrors::Raise => Err(PlenoraError::Schema(message.into())),
        CastErrors::Ignore => Err(PlenoraError::Contract(
            "errors=ignore non puo' garantire un tipo Arrow omogeneo; usare coerce o raise".into(),
        )),
    }
}

fn parse_date(value: &str, format: &str, datetime: bool) -> Option<String> {
    if !format.is_empty() {
        if datetime {
            return NaiveDateTime::parse_from_str(value, format)
                .ok()
                .map(|v| v.format("%Y-%m-%dT%H:%M:%S").to_string());
        }
        return NaiveDate::parse_from_str(value, format)
            .ok()
            .map(|v| v.format("%Y-%m-%d").to_string());
    }
    let date_formats = ["%Y-%m-%d", "%d/%m/%Y", "%d-%m-%Y", "%Y/%m/%d"];
    if datetime {
        for pattern in [
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%d %H:%M:%S",
            "%d/%m/%Y %H:%M:%S",
        ] {
            if let Ok(value) = NaiveDateTime::parse_from_str(value, pattern) {
                return Some(value.format("%Y-%m-%dT%H:%M:%S").to_string());
            }
        }
    }
    date_formats
        .iter()
        .find_map(|pattern| NaiveDate::parse_from_str(value, pattern).ok())
        .map(|v| {
            if datetime {
                format!("{}T00:00:00", v.format("%Y-%m-%d"))
            } else {
                v.format("%Y-%m-%d").to_string()
            }
        })
}

fn parse_date32(value: &str, format: &str) -> Option<i32> {
    let normalized = parse_date(value, format, false)?;
    let date = NaiveDate::parse_from_str(&normalized, "%Y-%m-%d").ok()?;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    i32::try_from(date.signed_duration_since(epoch).num_days()).ok()
}

fn parse_timestamp_millis(value: &str, format: &str, timezone: Option<&str>) -> Option<i64> {
    if format.is_empty() {
        if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
            return Some(timestamp.timestamp_millis());
        }
    }
    let normalized = parse_date(value, format, true)?;
    let naive = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S").ok()?;
    let timestamp = if let Some(name) = timezone {
        let zone = name.parse::<chrono_tz::Tz>().ok()?;
        match zone.from_local_datetime(&naive) {
            LocalResult::Single(value) => value.with_timezone(&Utc),
            LocalResult::Ambiguous(_, _) | LocalResult::None => return None,
        }
    } else {
        Utc.from_utc_datetime(&naive)
    };
    Some(timestamp.timestamp_millis())
}

fn parse_decimal128(value: &str, precision: u8, scale: i8) -> Option<i128> {
    let scale = u32::try_from(scale).ok()?;
    let value = value.trim();
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |rest| (true, rest));
    let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
    let mut pieces = unsigned.split('.');
    let whole = pieces.next()?;
    let fraction = pieces.next().unwrap_or("");
    if pieces.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > usize::try_from(scale).ok()?
    {
        return None;
    }
    let factor = 10_i128.checked_pow(scale)?;
    let whole = whole.parse::<i128>().ok()?.checked_mul(factor)?;
    let missing = scale.checked_sub(u32::try_from(fraction.len()).ok()?)?;
    let fractional = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i128>()
            .ok()?
            .checked_mul(10_i128.checked_pow(missing)?)?
    };
    let magnitude = whole.checked_add(fractional)?;
    let signed = if negative {
        magnitude.checked_neg()?
    } else {
        magnitude
    };
    let digits = signed
        .unsigned_abs()
        .to_string()
        .trim_start_matches('0')
        .len()
        .max(1);
    (digits <= usize::from(precision)).then_some(signed)
}

fn source_strings(source: &ArrayRef) -> Result<Vec<Option<String>>> {
    (0..source.len())
        .map(|row| scalar_as_string(source.as_ref(), row))
        .collect()
}

// ---------------------------------------------------------------------------
// Fast path per `table.type_cast`: il percorso generico converte ogni riga in
// `String` con `scalar_as_string` (catena di downcast + allocazione per riga)
// e poi la riparsa. Qui il downcast e' fatto una volta sola e i cast numerici
// avvengono sui valori nativi; le combinazioni non coperte (sorgenti Date32,
// Timestamp, Decimal128, Binary, Dictionary, e target data/decimal da sorgenti
// numeriche) ricadono su `type_cast_generic`, che resta anche l'oracolo dei
// test di equivalenza. Semantica byte-identica: `coerce` -> null, `raise` ->
// errore Schema sulla PRIMA riga fallita, `ignore` -> errore Contract; i cast
// da f64 riproducono `to_string().trim().parse()` (Display di f64 non usa
// notazione esponenziale).
// ---------------------------------------------------------------------------

/// `to_string(value).trim().parse::<i64>()` del generico su valori nativi:
/// riesce per i finiti interi. Sotto 2^53 il Display e' esatto e il parse e'
/// sempre in range; sopra 2^53 il Display stampa la rappresentazione decimale
/// piu' corta (es. -2^63 -> "-9223372036854776000", che NON parsa come i64),
/// quindi si riproduce il parse testuale. "-0" (da -0.0) parse a 0.
fn cast_f64_i64(value: f64) -> Option<i64> {
    const EXACT: f64 = 9_007_199_254_740_992.0; // 2^53
    if !value.is_finite() || value != value.trunc() {
        return None;
    }
    if value.abs() < EXACT {
        return value.to_i64();
    }
    value.to_string().parse::<i64>().ok()
}

/// `to_string(value).trim().parse::<u64>()` del generico: come sopra, con il
/// segno: i negativi falliscono sempre, -0.0 fallisce ("-0").
fn cast_f64_u64(value: f64) -> Option<u64> {
    const EXACT: f64 = 9_007_199_254_740_992.0; // 2^53
    if !value.is_finite()
        || value != value.trunc()
        || value < 0.0
        || (value == 0.0 && value.is_sign_negative())
    {
        return None;
    }
    if value < EXACT {
        return value.to_u64();
    }
    value.to_string().parse::<u64>().ok()
}

fn cast_or_failure<T>(parsed: Option<T>, errors: &CastErrors, message: &str) -> Result<Option<T>> {
    parsed.map_or_else(|| cast_failure(errors, message), |value| Ok(Some(value)))
}

/// `parse_date32` senza doppio parse chrono per il formato ISO canonico
/// "YYYY-MM-DD" (il primo provato da `parse_date` a formato vuoto): il parse
/// manuale usa la stessa validazione di `NaiveDate` (`from_ymd_opt`), quindi
/// produce lo stesso risultato; ogni altra stringa ricade su `parse_date32`.
fn parse_date32_fast(value: &str, format: &str) -> Option<i32> {
    if format.is_empty() {
        let bytes = value.as_bytes();
        if bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[..4]
                .iter()
                .chain(&bytes[5..7])
                .chain(&bytes[8..10])
                .all(u8::is_ascii_digit)
        {
            let year = value[..4].parse::<i32>().ok()?;
            let month = value[5..7].parse::<u32>().ok()?;
            let day = value[8..10].parse::<u32>().ok()?;
            let date = NaiveDate::from_ymd_opt(year, month, day)?;
            let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
            return i32::try_from(date.signed_duration_since(epoch).num_days()).ok();
        }
    }
    parse_date32(value, format)
}

/// Target `str`: i null diventano stringa vuota (`unwrap_or_default` del generico).
fn cast_to_str(source: &ArrayRef) -> Option<ArrayRef> {
    let len = source.len();
    let mut builder = StringBuilder::with_capacity(len, len * 8);
    if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_value("");
            } else {
                builder.append_value(values.value(row));
            }
        }
    } else if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_value("");
            } else {
                builder.append_value(values.value(row).to_string());
            }
        }
    } else if let Some(values) = source.as_any().downcast_ref::<UInt64Array>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_value("");
            } else {
                builder.append_value(values.value(row).to_string());
            }
        }
    } else if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_value("");
            } else {
                builder.append_value(values.value(row).to_string());
            }
        }
    } else if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_value("");
            } else {
                builder.append_value(values.value(row).to_string());
            }
        }
    } else {
        return None;
    }
    Some(Arc::new(builder.finish()))
}

/// Target `int` (Int64).
fn cast_to_int(source: &ArrayRef, errors: &CastErrors) -> Result<Option<ArrayRef>> {
    const MESSAGE: &str = "conversione int fallita";
    if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        // to_string + parse e' sempre l'identita' su Int64.
        return Ok(Some(Arc::new(values.clone())));
    }
    let len = source.len();
    let out: Vec<Option<i64>> = if let Some(values) =
        source.as_any().downcast_ref::<UInt64Array>()
    {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_or_failure(i64::try_from(values.value(row)).ok(), errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_or_failure(cast_f64_i64(values.value(row)), errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        values
            .iter()
            .map(|value| {
                value.map_or(Ok(None), |value| {
                    cast_or_failure(value.trim().parse::<i64>().ok(), errors, MESSAGE)
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        // "true"/"false" non parsano come i64: ogni riga non nulla fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_failure(errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else {
        return Ok(None);
    };
    Ok(Some(Arc::new(Int64Array::from(out))))
}

/// Target `float` (Float64).
fn cast_to_float(source: &ArrayRef, errors: &CastErrors) -> Result<Option<ArrayRef>> {
    const MESSAGE: &str = "conversione float fallita";
    if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        return Ok(Some(Arc::new(values.clone())));
    }
    let len = source.len();
    let out: Vec<Option<f64>> = if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        // to_string + parse riesce sempre e arrotonda come `to_f64` (mai None su i64).
        (0..len)
            .map(|row| {
                Ok(if values.is_null(row) {
                    None
                } else {
                    Some(values.value(row).to_f64().unwrap_or(f64::NAN))
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<UInt64Array>() {
        (0..len)
            .map(|row| {
                Ok(if values.is_null(row) {
                    None
                } else {
                    Some(values.value(row).to_f64().unwrap_or(f64::NAN))
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        values
            .iter()
            .map(|value| {
                value.map_or(Ok(None), |value| {
                    let trimmed = value.trim();
                    let parsed = if trimmed.contains(',') {
                        trimmed.replace(',', ".").parse::<f64>()
                    } else {
                        trimmed.parse::<f64>()
                    };
                    cast_or_failure(parsed.ok(), errors, MESSAGE)
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        // "true"/"false" non parsano come f64: ogni riga non nulla fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_failure(errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else {
        return Ok(None);
    };
    Ok(Some(Arc::new(Float64Array::from(out))))
}

/// Target `bool` (Boolean).
fn cast_to_bool(source: &ArrayRef, errors: &CastErrors) -> Result<Option<ArrayRef>> {
    const MESSAGE: &str = "conversione bool fallita";
    if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        return Ok(Some(Arc::new(values.clone())));
    }
    let len = source.len();
    let out: Vec<Option<bool>> = if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        // Il generico parsa la stringa: "1" -> true, "0" -> false, altro fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    match values.value(row) {
                        1 => Ok(Some(true)),
                        0 => Ok(Some(false)),
                        _ => cast_failure(errors, MESSAGE),
                    }
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<UInt64Array>() {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    match values.value(row) {
                        1 => Ok(Some(true)),
                        0 => Ok(Some(false)),
                        _ => cast_failure(errors, MESSAGE),
                    }
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        // to_string: "1" -> true, "0" -> false; "-0" (da -0.0) fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    let value = values.value(row);
                    if value == 1.0 {
                        Ok(Some(true))
                    } else if value == 0.0 && value.is_sign_positive() {
                        Ok(Some(false))
                    } else {
                        cast_failure(errors, MESSAGE)
                    }
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        values
            .iter()
            .map(|value| {
                value.map_or(Ok(None), |value| {
                    let lower = value.trim().to_lowercase();
                    match lower.as_str() {
                        "true" | "1" | "yes" | "si" | "sì" | "vero" | "t" | "y" | "s" => {
                            Ok(Some(true))
                        }
                        "false" | "0" | "no" | "falso" | "f" | "n" => Ok(Some(false)),
                        _ => cast_failure(errors, MESSAGE),
                    }
                })
            })
            .collect::<Result<_>>()?
    } else {
        return Ok(None);
    };
    Ok(Some(Arc::new(BooleanArray::from(out))))
}

/// Target `uint64` (`UInt64`).
fn cast_to_uint64(source: &ArrayRef, errors: &CastErrors) -> Result<Option<ArrayRef>> {
    const MESSAGE: &str = "conversione uint64 fallita";
    if let Some(values) = source.as_any().downcast_ref::<UInt64Array>() {
        return Ok(Some(Arc::new(values.clone())));
    }
    let len = source.len();
    let out: Vec<Option<u64>> = if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_or_failure(u64::try_from(values.value(row)).ok(), errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_or_failure(cast_f64_u64(values.value(row)), errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        values
            .iter()
            .map(|value| {
                value.map_or(Ok(None), |value| {
                    cast_or_failure(value.trim().parse::<u64>().ok(), errors, MESSAGE)
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        // "true"/"false" non parsano come u64: ogni riga non nulla fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_failure(errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else {
        return Ok(None);
    };
    Ok(Some(Arc::new(UInt64Array::from(out))))
}

/// Fast path: `Ok(None)` se la combinazione sorgente/target non e' coperta
/// (il chiamante ricade sul generico).
fn type_cast_fast(source: &ArrayRef, config: &TypeCast) -> Result<Option<ArrayRef>> {
    let array: ArrayRef = match config.target_type {
        TargetType::Str => match cast_to_str(source) {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Int => match cast_to_int(source, &config.errors)? {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Float => match cast_to_float(source, &config.errors)? {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Bool => match cast_to_bool(source, &config.errors)? {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Uint64 => match cast_to_uint64(source, &config.errors)? {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Date | TargetType::Datetime => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let datetime = matches!(config.target_type, TargetType::Datetime);
            Arc::new(StringArray::from(
                values
                    .iter()
                    .map(|value| {
                        value.map_or(Ok(None), |value| {
                            cast_or_failure(
                                parse_date(value, &config.date_format, datetime),
                                &config.errors,
                                "conversione data fallita",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        TargetType::Date32 => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            Arc::new(Date32Array::from(
                values
                    .iter()
                    .map(|value| {
                        value.map_or(Ok(None), |value| {
                            cast_or_failure(
                                parse_date32_fast(value, &config.date_format),
                                &config.errors,
                                "conversione date32 fallita",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        TargetType::TimestampMillis => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let out = values
                .iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        cast_or_failure(
                            parse_timestamp_millis(
                                value,
                                &config.date_format,
                                config.timezone.as_deref(),
                            ),
                            &config.errors,
                            "conversione timestamp fallita",
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Arc::new(TimestampMillisecondArray::from(out).with_timezone_opt(config.timezone.clone()))
        }
        TargetType::Decimal128 => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let precision = config
                .precision
                .ok_or_else(|| PlenoraError::Contract("decimal128 richiede precision".into()))?;
            let scale = config
                .scale
                .ok_or_else(|| PlenoraError::Contract("decimal128 richiede scale".into()))?;
            let out = values
                .iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        cast_or_failure(
                            parse_decimal128(value, precision, scale),
                            &config.errors,
                            "conversione decimal128 fallita",
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Arc::new(Decimal128Array::from(out).with_precision_and_scale(precision, scale)?)
        }
        TargetType::BinaryUtf8 => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let mut builder = BinaryBuilder::new();
            for value in values {
                if let Some(value) = value {
                    builder.append_value(value);
                } else {
                    builder.append_null();
                }
            }
            Arc::new(builder.finish())
        }
        TargetType::DictionaryUtf8 => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let mut builder = StringDictionaryBuilder::<Int32Type>::new();
            for value in values {
                if let Some(value) = value {
                    builder.append(value)?;
                } else {
                    builder.append_null();
                }
            }
            Arc::new(builder.finish())
        }
    };
    Ok(Some(array))
}

pub fn type_cast(batch: &RecordBatch, config: &TypeCast) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let array = match type_cast_fast(source, config)? {
        Some(array) => array,
        None => type_cast_generic(source, config)?,
    };
    replace_or_append(
        batch,
        &config.column,
        array.data_type().clone(),
        true,
        array,
    )
}

/// Percorso generico originale (conversione scalare per riga): fallback per le
/// combinazioni non coperte dal fast path e oracolo dei test di equivalenza.
#[allow(clippy::too_many_lines)] // One exhaustive dispatcher keeps all cast policies auditable.
fn type_cast_generic(source: &ArrayRef, config: &TypeCast) -> Result<ArrayRef> {
    let array: ArrayRef = match config.target_type {
        TargetType::Str => Arc::new(StringArray::from(
            (0..source.len())
                .map(|row| {
                    scalar_as_string(source.as_ref(), row).map(|v| Some(v.unwrap_or_default()))
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::Int => Arc::new(Int64Array::from(
            (0..source.len())
                .map(|row| {
                    scalar_as_string(source.as_ref(), row)?.map_or_else(
                        || Ok(None),
                        |value| {
                            value.trim().parse::<i64>().map_or_else(
                                |_| cast_failure(&config.errors, "conversione int fallita"),
                                |value| Ok(Some(value)),
                            )
                        },
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::Float => Arc::new(Float64Array::from(
            (0..source.len())
                .map(|row| {
                    scalar_as_string(source.as_ref(), row)?.map_or_else(
                        || Ok(None),
                        |value| {
                            value.trim().replace(',', ".").parse::<f64>().map_or_else(
                                |_| cast_failure(&config.errors, "conversione float fallita"),
                                |value| Ok(Some(value)),
                            )
                        },
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::Bool => Arc::new(BooleanArray::from(
            (0..source.len())
                .map(|row| {
                    scalar_as_string(source.as_ref(), row)?.map_or_else(
                        || Ok(None),
                        |value| {
                            let lower = value.trim().to_lowercase();
                            match lower.as_str() {
                                "true" | "1" | "yes" | "si" | "sì" | "vero" | "t" | "y" | "s" => {
                                    Ok(Some(true))
                                }
                                "false" | "0" | "no" | "falso" | "f" | "n" => Ok(Some(false)),
                                _ => cast_failure(&config.errors, "conversione bool fallita"),
                            }
                        },
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::Date | TargetType::Datetime => {
            let datetime = matches!(config.target_type, TargetType::Datetime);
            Arc::new(StringArray::from(
                (0..source.len())
                    .map(|row| {
                        scalar_as_string(source.as_ref(), row)?.map_or_else(
                            || Ok(None),
                            |value| {
                                parse_date(&value, &config.date_format, datetime).map_or_else(
                                    || cast_failure(&config.errors, "conversione data fallita"),
                                    |value| Ok(Some(value)),
                                )
                            },
                        )
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        TargetType::Date32 => Arc::new(Date32Array::from(
            source_strings(source)?
                .into_iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        parse_date32(&value, &config.date_format).map_or_else(
                            || cast_failure(&config.errors, "conversione date32 fallita"),
                            |value| Ok(Some(value)),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::TimestampMillis => {
            let values = source_strings(source)?
                .into_iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        parse_timestamp_millis(
                            &value,
                            &config.date_format,
                            config.timezone.as_deref(),
                        )
                        .map_or_else(
                            || cast_failure(&config.errors, "conversione timestamp fallita"),
                            |value| Ok(Some(value)),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let array =
                TimestampMillisecondArray::from(values).with_timezone_opt(config.timezone.clone());
            Arc::new(array)
        }
        TargetType::Decimal128 => {
            let precision = config
                .precision
                .ok_or_else(|| PlenoraError::Contract("decimal128 richiede precision".into()))?;
            let scale = config
                .scale
                .ok_or_else(|| PlenoraError::Contract("decimal128 richiede scale".into()))?;
            let values = source_strings(source)?
                .into_iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        parse_decimal128(&value, precision, scale).map_or_else(
                            || cast_failure(&config.errors, "conversione decimal128 fallita"),
                            |value| Ok(Some(value)),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Arc::new(Decimal128Array::from(values).with_precision_and_scale(precision, scale)?)
        }
        TargetType::BinaryUtf8 => {
            let mut builder = BinaryBuilder::new();
            for value in source_strings(source)? {
                if let Some(value) = value {
                    builder.append_value(value);
                } else {
                    builder.append_null();
                }
            }
            Arc::new(builder.finish())
        }
        TargetType::Uint64 => Arc::new(UInt64Array::from(
            source_strings(source)?
                .into_iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        value.trim().parse::<u64>().map_or_else(
                            |_| cast_failure(&config.errors, "conversione uint64 fallita"),
                            |value| Ok(Some(value)),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::DictionaryUtf8 => {
            let mut builder = StringDictionaryBuilder::<Int32Type>::new();
            for value in source_strings(source)? {
                if let Some(value) = value {
                    builder.append(value)?;
                } else {
                    builder.append_null();
                }
            }
            Arc::new(builder.finish())
        }
    };
    Ok(array)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{
        ArrayRef, BooleanArray, Date32Array, Float64Array, Int64Array, LargeStringArray,
        StringArray, TimestampMillisecondArray, UInt64Array,
    };
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::{json, Value};

    use super::*;

    // ------------------------------------------------------------------
    // Oracolo fill_na: implementazione pre-ottimizzazione (Vec<Option<T>>
    // riga per riga + fill_options), mantenuta per i test di equivalenza.
    // ------------------------------------------------------------------

    fn fill_options<T: Clone>(out: &mut [Option<T>], method: &FillMethod, fixed: Option<T>) {
        match method {
            FillMethod::Value => {
                if let Some(value) = fixed {
                    for item in out {
                        if item.is_none() {
                            *item = Some(value.clone());
                        }
                    }
                }
            }
            FillMethod::Ffill => {
                let mut previous = None;
                for item in out {
                    if item.is_none() {
                        *item = previous.clone();
                    } else {
                        previous.clone_from(item);
                    }
                }
            }
            FillMethod::Bfill => {
                let mut following = None;
                for item in out.iter_mut().rev() {
                    if item.is_none() {
                        *item = following.clone();
                    } else {
                        following.clone_from(item);
                    }
                }
            }
        }
    }

    fn oracle_fill_array(
        array: &dyn Array,
        method: &FillMethod,
        value: &Value,
    ) -> Result<ArrayRef> {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            let fixed = match value {
                Value::Null => None,
                Value::String(v) => Some(v.clone()),
                other => Some(other.to_string()),
            };
            let mut out: Vec<Option<String>> = values.iter().map(|v| v.map(str::to_owned)).collect();
            fill_options(&mut out, method, fixed);
            return Ok(Arc::new(StringArray::from(out)));
        }
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            let fixed = match value {
                Value::Null => None,
                Value::Number(n) => n.as_i64(),
                Value::String(s) => Some(
                    s.parse()
                        .map_err(|_| PlenoraError::Contract("fill int non valido".into()))?,
                ),
                _ => return Err(PlenoraError::Contract("fill int non valido".into())),
            };
            let mut out: Vec<Option<i64>> = values.iter().collect();
            fill_options(&mut out, method, fixed);
            return Ok(Arc::new(Int64Array::from(out)));
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            let fixed = match value {
                Value::Null => None,
                Value::Number(n) => n.as_f64(),
                Value::String(s) => Some(
                    s.replace(',', ".")
                        .parse()
                        .map_err(|_| PlenoraError::Contract("fill float non valido".into()))?,
                ),
                _ => return Err(PlenoraError::Contract("fill float non valido".into())),
            };
            let mut out: Vec<Option<f64>> = values.iter().collect();
            fill_options(&mut out, method, fixed);
            return Ok(Arc::new(Float64Array::from(out)));
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            let fixed = match value {
                Value::Null => None,
                Value::Bool(v) => Some(*v),
                Value::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
                Value::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
                _ => return Err(PlenoraError::Contract("fill bool non valido".into())),
            };
            let mut out: Vec<Option<bool>> = values.iter().collect();
            fill_options(&mut out, method, fixed);
            return Ok(Arc::new(BooleanArray::from(out)));
        }
        Err(PlenoraError::Schema(format!(
            "fill_na non supporta {:?}",
            array.data_type()
        )))
    }

    // ------------------------------------------------------------------
    // Helper di equivalenza
    // ------------------------------------------------------------------

    fn single_batch(array: ArrayRef) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "c",
                array.data_type().clone(),
                true,
            )])),
            vec![array],
        )
        .expect("batch")
    }

    fn fill_method(index: usize) -> FillMethod {
        match index {
            0 => FillMethod::Value,
            1 => FillMethod::Ffill,
            _ => FillMethod::Bfill,
        }
    }

    fn assert_fill_equiv(array: &ArrayRef, method: &FillMethod, value: &Value) {
        let fast = fill_array(array.as_ref(), method, value).map_err(|e| format!("{e:?}"));
        let generic =
            oracle_fill_array(array.as_ref(), method, value).map_err(|e| format!("{e:?}"));
        match (fast, generic) {
            (Ok(fast), Ok(generic)) => assert_eq!(single_batch(fast), single_batch(generic)),
            (Err(fast), Err(generic)) => assert_eq!(fast, generic),
            (fast, generic) => panic!(
                "fill_na: fast/generico divergono (fast err={}, generico err={})",
                fast.is_err(),
                generic.is_err()
            ),
        }
    }

    fn cast_config(target_type: TargetType, errors: CastErrors) -> TypeCast {
        TypeCast {
            column: "c".into(),
            target_type,
            date_format: String::new(),
            errors,
            precision: Some(10),
            scale: Some(2),
            timezone: None,
        }
    }

    fn assert_cast_equiv(array: &ArrayRef, config: &TypeCast) {
        let fast = type_cast_fast(array, config).map_err(|e| format!("{e:?}"));
        let generic = type_cast_generic(array, config).map_err(|e| format!("{e:?}"));
        match (fast, generic) {
            (Ok(Some(fast)), Ok(generic)) => assert_eq!(
                single_batch(fast),
                single_batch(generic),
                "type_cast verso {:?} diverge",
                config.target_type
            ),
            // Combinazione non coperta: il fallback e' il generico stesso.
            (Ok(None), _) => {}
            (Err(fast), Err(generic)) => assert_eq!(fast, generic),
            (fast, generic) => panic!(
                "type_cast verso {:?}: fast/generico divergono (fast ok={}, generico ok={})",
                config.target_type,
                fast.is_ok(),
                generic.is_ok()
            ),
        }
    }

    fn assert_cast_covered(array: &ArrayRef, config: &TypeCast) {
        if let Ok(result) = type_cast_fast(array, config) {
            // Con raise/ignore l'errore puo' essere legittimo: lo verifica
            // l'equivalenza; qui si controlla solo la copertura.
            assert!(
                result.is_some(),
                "combinazione {:?} attesa coperta dal fast path",
                config.target_type
            );
        }
        assert_cast_equiv(array, config);
    }

    fn all_targets() -> Vec<TargetType> {
        vec![
            TargetType::Str,
            TargetType::Int,
            TargetType::Float,
            TargetType::Bool,
            TargetType::Date,
            TargetType::Datetime,
            TargetType::Date32,
            TargetType::TimestampMillis,
            TargetType::Decimal128,
            TargetType::BinaryUtf8,
            TargetType::Uint64,
            TargetType::DictionaryUtf8,
        ]
    }

    fn all_errors() -> Vec<CastErrors> {
        vec![CastErrors::Coerce, CastErrors::Raise, CastErrors::Ignore]
    }

    // ------------------------------------------------------------------
    // fill_na
    // ------------------------------------------------------------------

    #[test]
    fn fill_fast_matches_oracle_on_int64_matrices() {
        let cases: Vec<Vec<Option<i64>>> = vec![
            vec![],
            vec![Some(1)],
            vec![None],
            vec![Some(1), None, Some(3), None, None, Some(6)],
            vec![None, None, Some(2)],
            vec![Some(i64::MIN), Some(i64::MAX), None],
            vec![None, None, None],
        ];
        let values = [
            Value::Null,
            json!(0),
            json!(-7),
            json!("11"),
            json!(1.5),
            json!(true),
        ];
        for case in &cases {
            let array: ArrayRef = Arc::new(Int64Array::from(case.clone()));
            for method in 0..3 {
                for value in &values {
                    assert_fill_equiv(&array, &fill_method(method), value);
                }
            }
        }
    }

    #[test]
    fn fill_fast_matches_oracle_on_float64_matrices() {
        let cases: Vec<Vec<Option<f64>>> = vec![
            vec![],
            vec![Some(0.0)],
            vec![None],
            vec![
                Some(f64::NAN),
                Some(-0.0),
                None,
                Some(f64::INFINITY),
                Some(f64::MIN),
                Some(f64::MAX),
            ],
            vec![None, Some(2.5), None, None],
            vec![None, None, None],
        ];
        let values = [Value::Null, json!(0), json!(2.5), json!("3,5"), json!("x")];
        for case in &cases {
            let array: ArrayRef = Arc::new(Float64Array::from(case.clone()));
            for method in 0..3 {
                for value in &values {
                    assert_fill_equiv(&array, &fill_method(method), value);
                }
            }
        }
    }

    #[test]
    fn fill_fast_matches_oracle_on_boolean_and_utf8_matrices() {
        let bool_cases: Vec<Vec<Option<bool>>> = vec![
            vec![],
            vec![Some(true)],
            vec![None],
            vec![Some(true), None, Some(false), None],
            vec![None, None],
        ];
        let bool_values = [
            Value::Null,
            json!(true),
            json!(false),
            json!("TRUE"),
            json!("no"),
            json!(1),
        ];
        for case in &bool_cases {
            let array: ArrayRef = Arc::new(BooleanArray::from(case.clone()));
            for method in 0..3 {
                for value in &bool_values {
                    assert_fill_equiv(&array, &fill_method(method), value);
                }
            }
        }
        let utf8_cases: Vec<Vec<Option<&str>>> = vec![
            vec![],
            vec![Some("a")],
            vec![None],
            vec![Some("x"), None, Some(""), None, None, Some("fine")],
            vec![None, None, Some("coda")],
            vec![None, None, None],
        ];
        let utf8_values = [Value::Null, json!("riempi"), json!(42), json!(false)];
        for case in &utf8_cases {
            let array: ArrayRef = Arc::new(StringArray::from(case.clone()));
            for method in 0..3 {
                for value in &utf8_values {
                    assert_fill_equiv(&array, &fill_method(method), value);
                }
            }
        }
    }

    #[test]
    fn fill_unsupported_types_keep_the_schema_error() {
        let array: ArrayRef = Arc::new(UInt64Array::from(vec![Some(1_u64), None]));
        for method in 0..3 {
            for value in [Value::Null, json!(7)] {
                assert_fill_equiv(&array, &fill_method(method), &value);
            }
        }
    }

    // ------------------------------------------------------------------
    // type_cast
    // ------------------------------------------------------------------

    #[test]
    fn cast_fast_matches_generic_on_utf8_matrix() {
        let texts: Vec<Option<&str>> = vec![
            Some("42"),
            Some("-7"),
            Some(" 8 "),
            Some("abc"),
            Some(""),
            Some("  "),
            Some("9.5"),
            Some("3,14"),
            Some("1e3"),
            Some("NaN"),
            Some("inf"),
            Some("-inf"),
            Some("true"),
            Some("FALSE"),
            Some("sì"),
            Some("SÌ"),
            Some("no"),
            Some("vero"),
            Some("0"),
            Some("1"),
            Some("+5"),
            Some("-0"),
            Some("18446744073709551615"),
            Some("18446744073709551616"),
            Some("9223372036854775807"),
            Some("9223372036854775808"),
            Some("-9223372036854775809"),
            Some("2024-01-31"),
            Some("31/01/2024"),
            Some("2024-13-40"),
            Some("2024-01-31T10:20:30"),
            Some("2024-01-31 10:20:30"),
            None,
        ];
        let array: ArrayRef = Arc::new(StringArray::from(texts));
        for target in all_targets() {
            for errors in all_errors() {
                assert_cast_covered(&array, &cast_config(target, errors));
            }
        }
    }

    #[test]
    fn cast_fast_matches_generic_with_date_format_and_timezone() {
        let dates: Vec<Option<&str>> = vec![
            Some("31/01/2024"),
            Some("2024-01-31"),
            Some("30/02/2024"),
            Some("bad"),
            None,
        ];
        let array: ArrayRef = Arc::new(StringArray::from(dates));
        for target in [
            TargetType::Date,
            TargetType::Datetime,
            TargetType::Date32,
            TargetType::TimestampMillis,
        ] {
            for errors in all_errors() {
                let mut config = cast_config(target, errors);
                config.date_format = "%d/%m/%Y".into();
                assert_cast_covered(&array, &config);
            }
        }
        // Timestamp con timezone, incluse date inesistenti (buco DST) e rfc3339.
        let stamps: Vec<Option<&str>> = vec![
            Some("2024-01-31 10:20:30"),
            Some("2024-03-31 02:30:00"),
            Some("2024-10-27 02:30:00"),
            Some("2024-01-31T10:20:30+01:00"),
            Some("bad"),
            None,
        ];
        let array: ArrayRef = Arc::new(StringArray::from(stamps));
        for errors in all_errors() {
            let mut config = cast_config(TargetType::TimestampMillis, errors);
            config.timezone = Some("Europe/Rome".into());
            assert_cast_covered(&array, &config);
        }
    }

    #[test]
    fn cast_fast_matches_generic_on_numeric_matrices() {
        let int64s: ArrayRef = Arc::new(Int64Array::from(vec![
            Some(0),
            Some(1),
            Some(-1),
            Some(i64::MAX),
            Some(i64::MIN),
            None,
            Some(42),
        ]));
        let uint64s: ArrayRef = Arc::new(UInt64Array::from(vec![
            Some(0),
            Some(1),
            Some(u64::MAX),
            None,
            Some(7),
        ]));
        let float64s: ArrayRef = Arc::new(Float64Array::from(vec![
            Some(0.0),
            Some(-0.0),
            Some(1.0),
            Some(-1.0),
            Some(42.0),
            Some(42.5),
            Some(-0.5),
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(f64::NEG_INFINITY),
            Some(9_223_372_036_854_775_808.0),
            Some(-9_223_372_036_854_775_808.0),
            Some(18_446_744_073_709_551_616.0),
            Some(1e300),
            Some(1e-300),
            None,
        ]));
        let bools: ArrayRef = Arc::new(BooleanArray::from(vec![Some(true), Some(false), None]));
        for array in [&int64s, &uint64s, &float64s, &bools] {
            for target in all_targets() {
                for errors in all_errors() {
                    assert_cast_equiv(array, &cast_config(target, errors));
                }
            }
            // Copertura esplicita dei fast path numerici.
            for target in [
                TargetType::Str,
                TargetType::Int,
                TargetType::Float,
                TargetType::Bool,
                TargetType::Uint64,
            ] {
                assert_cast_covered(array, &cast_config(target, CastErrors::Coerce));
            }
        }
    }

    #[test]
    fn cast_fast_matches_generic_on_empty_and_single_row() {
        let cases: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
            Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
            Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
            Arc::new(BooleanArray::from(Vec::<Option<bool>>::new())),
            Arc::new(UInt64Array::from(Vec::<Option<u64>>::new())),
            Arc::new(StringArray::from(vec![Some("42")])),
            Arc::new(StringArray::from(vec![Option::<&str>::None])),
            Arc::new(Int64Array::from(vec![Option::<i64>::None])),
            Arc::new(Float64Array::from(vec![Some(-0.0)])),
            Arc::new(BooleanArray::from(vec![Some(true)])),
        ];
        for array in &cases {
            for target in all_targets() {
                for errors in all_errors() {
                    assert_cast_equiv(array, &cast_config(target, errors));
                }
            }
        }
    }

    #[test]
    fn cast_fallback_combinations_match_generic_through_public_entry() {
        let sources: Vec<ArrayRef> = vec![
            Arc::new(Date32Array::from(vec![Some(0), Some(19000), Some(-1), None])),
            Arc::new(TimestampMillisecondArray::from(vec![
                Some(0),
                Some(1_700_000_000_000),
                Some(-1),
                None,
            ])),
        ];
        for source in sources {
            let batch = single_batch(source.clone());
            for target in all_targets() {
                for errors in all_errors() {
                    let config = cast_config(target, errors);
                    let production = type_cast(&batch, &config).map_err(|e| format!("{e:?}"));
                    let generic = type_cast_generic(batch.column(0), &config)
                        .map(single_batch)
                        .map_err(|e| format!("{e:?}"));
                    match (production, generic) {
                        (Ok(production), Ok(generic)) => assert_eq!(production, generic),
                        (Err(production), Err(generic)) => assert_eq!(production, generic),
                        (production, generic) => panic!(
                            "fallback {:?}: produzione/generico divergono (prod ok={}, gen ok={})",
                            config.target_type,
                            production.is_ok(),
                            generic.is_ok()
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn cast_large_utf8_keeps_the_scalar_profile_error() {
        let batch = single_batch(Arc::new(LargeStringArray::from(vec![Some("42"), None])));
        for target in all_targets() {
            let config = cast_config(target, CastErrors::Coerce);
            assert!(type_cast(&batch, &config).is_err());
        }
    }

    // ------------------------------------------------------------------
    // coalesce
    // ------------------------------------------------------------------

    fn batch_of(columns: Vec<(&str, ArrayRef)>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(
                columns
                    .iter()
                    .map(|(name, array)| Field::new(*name, array.data_type().clone(), true))
                    .collect::<Vec<_>>(),
            )),
            columns.into_iter().map(|(_, array)| array).collect(),
        )
        .expect("batch")
    }

    fn assert_coalesce_equiv(columns: Vec<(&str, ArrayRef)>) {
        let batch = batch_of(columns);
        let indices: Vec<usize> = (0..batch.num_columns()).collect();
        let generic = crate::quality::coalesce_generic(&batch, &indices).expect("generico");
        if let Some(fast) = coalesce_fast(&batch, &indices) {
            assert_eq!(single_batch(fast), single_batch(generic));
        }
    }

    #[test]
    fn coalesce_fast_matches_generic_on_numeric_types() {
        assert_coalesce_equiv(vec![
            ("a", Arc::new(Int64Array::from(vec![None, Some(1), None, Some(4)]))),
            ("b", Arc::new(Int64Array::from(vec![None, None, Some(3), Some(5)]))),
            ("c", Arc::new(Int64Array::from(vec![Some(9), Some(9), Some(9), None]))),
        ]);
        assert_coalesce_equiv(vec![
            ("a", Arc::new(UInt64Array::from(vec![None, Some(u64::MAX)]))),
            ("b", Arc::new(UInt64Array::from(vec![Some(1), None]))),
        ]);
        assert_coalesce_equiv(vec![
            (
                "a",
                Arc::new(Float64Array::from(vec![None, Some(f64::NAN), Some(-0.0)])),
            ),
            (
                "b",
                Arc::new(Float64Array::from(vec![Some(2.5), None, Some(f64::INFINITY)])),
            ),
        ]);
        assert_coalesce_equiv(vec![
            ("a", Arc::new(BooleanArray::from(vec![None, Some(true), None]))),
            ("b", Arc::new(BooleanArray::from(vec![Some(false), None, Some(true)]))),
        ]);
    }

    #[test]
    fn coalesce_fast_matches_generic_on_utf8_and_edge_cases() {
        assert_coalesce_equiv(vec![
            ("a", Arc::new(StringArray::from(vec![None, Some("x"), None]))),
            ("b", Arc::new(StringArray::from(vec![None, None, Some("y")]))),
            ("c", Arc::new(StringArray::from(vec![Some("z"), Some("z"), None]))),
        ]);
        // Prima colonna senza null: scorciatoia identita'.
        assert_coalesce_equiv(vec![
            ("a", Arc::new(Int64Array::from(vec![Some(1), Some(2)]))),
            ("b", Arc::new(Int64Array::from(vec![None, Some(3)]))),
        ]);
        // Colonna singola, tutti null, batch vuoto, riga singola.
        assert_coalesce_equiv(vec![(
            "a",
            Arc::new(Int64Array::from(vec![Option::<i64>::None])),
        )]);
        assert_coalesce_equiv(vec![(
            "a",
            Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
        )]);
        assert_coalesce_equiv(vec![("a", Arc::new(StringArray::from(vec![Some("solo")])))]);
    }

    #[test]
    fn coalesce_uncovered_types_fall_back_to_the_generic_path() {
        let batch = batch_of(vec![
            ("a", Arc::new(Date32Array::from(vec![None, Some(0)]))),
            ("b", Arc::new(Date32Array::from(vec![Some(19000), None]))),
        ]);
        assert!(coalesce_fast(&batch, &[0, 1]).is_none());
        let config = crate::quality::Coalesce {
            columns: vec!["a".into(), "b".into()],
            output_column: "out".into(),
        };
        let production = crate::quality::coalesce(&batch, &config).expect("coalesce");
        let generic = crate::quality::coalesce_generic(&batch, &[0, 1]).expect("generico");
        assert_eq!(
            single_batch(production.column(2).clone()),
            single_batch(generic)
        );
    }
}
