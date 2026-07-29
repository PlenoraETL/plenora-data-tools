use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::array::{Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use chrono::format::{Item, Parsed};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use serde::Deserialize;
use uuid::Uuid;

use plenora_core::{PlenoraError, Result};
use crate::dates::{compile_items, parse_with_items};
use crate::{column_index, replace_or_append, scalar_as_string, validate_output_name};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddRowNumber {
    #[serde(default = "default_row_name")]
    pub output_column: String,
    #[serde(default = "default_start")]
    pub start: i64,
    pub partition_column: Option<String>,
    pub order_column: Option<String>,
    #[serde(default = "default_true")]
    pub ascending: bool,
}

fn default_row_name() -> String {
    "row_number".into()
}
const fn default_start() -> i64 {
    1
}
const fn default_true() -> bool {
    true
}

/// Colonna Int64 con il numero di riga progressivo a partire da
/// `config.start`.
///
/// Con `partition_column` il conteggio riparte a ogni partizione (chiave =
/// valore testuale della colonna); l'ordinamento non e' gestito da questo
/// kernel.
///
/// # Errors
///
/// - `Contract`: nome della colonna di output non valido, `order_column`
///   valorizzato (l'ordinamento e' delegato al kernel blocking sort),
///   overflow i64 del contatore, valore di partizione non rappresentabile
///   come testo (come `scalar_as_string`);
/// - `Schema`: `partition_column` assente dal batch.
pub fn add_row_number(batch: &RecordBatch, config: &AddRowNumber) -> Result<RecordBatch> {
    let _ = config.ascending;
    validate_output_name(&config.output_column)?;
    if config.order_column.is_some() {
        return Err(PlenoraError::InvalidPlan("add_row_number con ordinamento verra' eseguito dal kernel blocking sort; profilo corrente richiede order_column nullo".into()));
    }
    let values = if let Some(partition) = &config.partition_column {
        let index = column_index(batch, partition)?;
        let source = batch.column(index);
        let mut counters: HashMap<Option<String>, i64> = HashMap::new();
        (0..batch.num_rows())
            .map(|row| {
                let key = scalar_as_string(source.as_ref(), row)?;
                let value = counters.entry(key).or_insert(config.start);
                let current = *value;
                *value = value
                    .checked_add(1)
                    .ok_or_else(|| PlenoraError::InvalidPlan("overflow row number".into()))?;
                Ok(Some(current))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        (0..batch.num_rows())
            .map(|row| {
                i64::try_from(row)
                    .ok()
                    .and_then(|row| config.start.checked_add(row))
                    .map(Some)
                    .ok_or_else(|| PlenoraError::InvalidPlan("overflow row number".into()))
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Int64,
        false,
        Arc::new(Int64Array::from(values)),
    )
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatePart {
    Year,
    Month,
    Day,
    Quarter,
    Weekday,
    Week,
    Hour,
    Minute,
    Second,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateExtract {
    pub column: String,
    #[serde(default = "default_parts")]
    pub parts: Vec<DatePart>,
    #[serde(default)]
    pub prefix: String,
    /// Formato chrono esplicito. Se omesso viene usato il parser deterministico
    /// multi-formato del profilo legacy.
    pub date_format: Option<String>,
    #[serde(default = "default_invalid_date_policy")]
    pub invalid: InvalidDatePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidDatePolicy {
    Null,
    Error,
}

const fn default_invalid_date_policy() -> InvalidDatePolicy {
    InvalidDatePolicy::Null
}

fn default_parts() -> Vec<DatePart> {
    vec![DatePart::Year]
}

fn parse_datetime(value: &str, explicit_format: Option<&str>) -> Option<NaiveDateTime> {
    if let Some(format) = explicit_format {
        return NaiveDateTime::parse_from_str(value, format)
            .ok()
            .or_else(|| {
                NaiveDate::parse_from_str(value, format)
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
            });
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%d/%m/%Y %H:%M:%S",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed);
        }
    }
    for format in ["%Y-%m-%d", "%d/%m/%Y", "%d-%m-%Y", "%Y/%m/%d"] {
        if let Ok(parsed) = NaiveDate::parse_from_str(value, format) {
            return parsed.and_hms_opt(0, 0, 0);
        }
    }
    None
}

// Fast path `date_extract` (secondo batch filone ottimizzazioni kernel):
// formati chrono compilati una volta e loop sui `&str` nativi della colonna
// Utf8; per gli altri tipi Arrow si ricade sul percorso generico.

/// Item precompilati dei formati datetime del parser multi-formato di
/// default (stesso ordine di `parse_datetime`).
fn default_datetime_items() -> [Vec<Item<'static>>; 3] {
    [
        compile_items("%Y-%m-%dT%H:%M:%S"),
        compile_items("%Y-%m-%d %H:%M:%S"),
        compile_items("%d/%m/%Y %H:%M:%S"),
    ]
}

/// Item precompilati dei formati date-only del parser multi-formato di
/// default (stesso ordine di `parse_datetime`).
fn default_date_items() -> [Vec<Item<'static>>; 4] {
    [
        compile_items("%Y-%m-%d"),
        compile_items("%d/%m/%Y"),
        compile_items("%d-%m-%Y"),
        compile_items("%Y/%m/%d"),
    ]
}

/// Parser multi-formato di default con item precompilati, semantica identica
/// a `parse_datetime(value, None)`: prima i formati datetime, poi quelli
/// date-only (mezzanotte).
fn parse_datetime_default(
    value: &str,
    datetime_items: &[Vec<Item<'static>>],
    date_items: &[Vec<Item<'static>>],
) -> Option<NaiveDateTime> {
    for items in datetime_items {
        let mut parsed = Parsed::new();
        if chrono::format::parse(&mut parsed, value, items.iter()).is_ok() {
            // Stessa risoluzione di `NaiveDateTime::parse_from_str`.
            if let Ok(datetime) = parsed.to_naive_datetime_with_offset(0) {
                return Some(datetime);
            }
        }
    }
    for items in date_items {
        let mut parsed = Parsed::new();
        if chrono::format::parse(&mut parsed, value, items.iter()).is_ok() {
            if let Ok(date) = parsed.to_naive_date() {
                return date.and_hms_opt(0, 0, 0);
            }
        }
    }
    None
}

/// Estrae le parti di data/ora richieste in colonne Int64 `<prefix><parte>`.
///
/// Il parsing usa `date_format` se dato, altrimenti il parser deterministico
/// multi-formato del profilo legacy; i valori non parsabili producono null
/// oppure errore secondo `config.invalid`.
///
/// # Errors
///
/// - `Contract`: valore non parsabile con `invalid = error`, nome di colonna
///   di output non valido, valore non rappresentabile come testo (come
///   `scalar_as_string`, percorso non-Utf8), guardia interna su parser
///   compilato singolo;
/// - `Schema`: colonna assente dal batch.
// Sequenza lineare (parsing esplicito o multi-formato, poi estrazione delle
// parti in colonne): lunga per costruzione, uno spezzone artificiale
// peggiorerebbe solo la leggibilita'.
#[allow(clippy::too_many_lines)]
pub fn date_extract(batch: &RecordBatch, config: &DateExtract) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let parsed: Vec<Option<NaiveDateTime>> = if let Some(column) =
        source.as_any().downcast_ref::<StringArray>()
    {
        let explicit_items = config.date_format.as_deref().map(compile_items);
        let default_items = if explicit_items.is_none() {
            Some((default_datetime_items(), default_date_items()))
        } else {
            None
        };
        let mut parsed = Vec::with_capacity(column.len());
        for row in 0..column.len() {
            if column.is_null(row) {
                parsed.push(None);
                continue;
            }
            let value = column.value(row);
            let parsed_value = match (&explicit_items, &default_items) {
                (Some(items), None) => parse_with_items(value, items),
                (None, Some((datetime_items, date_items))) => {
                    parse_datetime_default(value, datetime_items, date_items)
                }
                _ => {
                    return Err(PlenoraError::InvalidPlan(
                        "internal error: un solo parser compilato".into(),
                    ));
                }
            };
            match parsed_value {
                Some(value) => parsed.push(Some(value)),
                None if matches!(config.invalid, InvalidDatePolicy::Null) => parsed.push(None),
                None => {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "date_extract: valore non valido alla riga {row}"
                    )));
                }
            }
        }
        parsed
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    return Ok(None);
                };
                match parse_datetime(&value, config.date_format.as_deref()) {
                    Some(parsed) => Ok(Some(parsed)),
                    None if matches!(config.invalid, InvalidDatePolicy::Null) => Ok(None),
                    None => Err(PlenoraError::InvalidPlan(format!(
                        "date_extract: valore non valido alla riga {row}"
                    ))),
                }
            })
            .collect::<Result<Vec<_>>>()?
    };
    let prefix = if config.prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.prefix.clone()
    };
    let mut result = batch.clone();
    for part in &config.parts {
        let suffix = match part {
            DatePart::Year => "year",
            DatePart::Month => "month",
            DatePart::Day => "day",
            DatePart::Quarter => "quarter",
            DatePart::Weekday => "weekday",
            DatePart::Week => "week",
            DatePart::Hour => "hour",
            DatePart::Minute => "minute",
            DatePart::Second => "second",
        };
        let name = format!("{prefix}{suffix}");
        validate_output_name(&name)?;
        let values = parsed
            .iter()
            .map(|value| {
                value.map(|value| match part {
                    DatePart::Year => i64::from(value.year()),
                    DatePart::Month => i64::from(value.month()),
                    DatePart::Day => i64::from(value.day()),
                    DatePart::Quarter => i64::from((value.month() - 1) / 3 + 1),
                    DatePart::Weekday => i64::from(value.weekday().num_days_from_monday()),
                    DatePart::Week => i64::from(value.iso_week().week()),
                    DatePart::Hour => i64::from(value.hour()),
                    DatePart::Minute => i64::from(value.minute()),
                    DatePart::Second => i64::from(value.second()),
                })
            })
            .collect::<Vec<_>>();
        result = replace_or_append(
            &result,
            &name,
            DataType::Int64,
            true,
            Arc::new(Int64Array::from(values)),
        )?;
    }
    Ok(result)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    pub n: u64,
    #[serde(default)]
    pub offset: u64,
}

/// Prime `n` righe dopo `offset` (estensione v1.1). Schema invariato, ordine
/// delle righe preservato (slicing puro, zero-copy sui dati Arrow).
///
/// Semantica per-batch: come tutte le op `Streaming` del profilo, il kernel
/// opera sul singolo `RecordBatch` che riceve e non mantiene stato
/// inter-batch; su uno stream di piu' batch l'executor applica l'operazione
/// a ciascun batch (stesso modello di `filter`/`distinct`). Se in futuro
/// servisse un limite globale sull'intero stream servira' un operatore con
/// stato nel layer engine, non un kernel puro.
///
/// # Errors
///
/// - `Contract`: numero di righe non rappresentabile come u64, `offset` o
///   `n` non rappresentabili come usize.
pub fn limit(batch: &RecordBatch, config: &Limit) -> Result<RecordBatch> {
    let rows = u64::try_from(batch.num_rows())
        .map_err(|_| PlenoraError::InvalidPlan("limit: righe oltre u64".into()))?;
    let start = config.offset.min(rows);
    let count = config.n.min(rows - start);
    let start = usize::try_from(start)
        .map_err(|_| PlenoraError::InvalidPlan("limit: offset oltre usize".into()))?;
    let count = usize::try_from(count)
        .map_err(|_| PlenoraError::InvalidPlan("limit: n oltre usize".into()))?;
    if start == 0 && count == batch.num_rows() {
        // Finestra che copre l'intero batch: nessuna copia.
        return Ok(batch.clone());
    }
    Ok(batch.slice(start, count))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UuidGenerator {
    #[serde(default = "default_uuid_name")]
    pub output_column: String,
}
fn default_uuid_name() -> String {
    "uuid".into()
}

/// Colonna con un UUID v4 (formato hyphenated) per riga.
///
/// # Errors
///
/// - `Contract`: nome della colonna di output non valido.
pub fn uuid_generator(batch: &RecordBatch, config: &UuidGenerator) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    let values = (0..batch.num_rows())
        .map(|_| Uuid::new_v4().hyphenated().to_string())
        .collect::<Vec<_>>();
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(StringArray::from(values)),
    )
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::json;

    use super::*;

    /// Percorso generico pre-ottimizzazione: riferimento per l'equivalenza
    /// semantica (oracolo) del fast path di `date_extract`.
    fn generic_date_extract(batch: &RecordBatch, config: &DateExtract) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let source = batch.column(index);
        let parsed = (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    return Ok(None);
                };
                match parse_datetime(&value, config.date_format.as_deref()) {
                    Some(parsed) => Ok(Some(parsed)),
                    None if matches!(config.invalid, InvalidDatePolicy::Null) => Ok(None),
                    None => Err(PlenoraError::InvalidPlan(format!(
                        "date_extract: valore non valido alla riga {row}"
                    ))),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let prefix = if config.prefix.is_empty() {
            format!("{}_", config.column)
        } else {
            config.prefix.clone()
        };
        let mut result = batch.clone();
        for part in &config.parts {
            let suffix = match part {
                DatePart::Year => "year",
                DatePart::Month => "month",
                DatePart::Day => "day",
                DatePart::Quarter => "quarter",
                DatePart::Weekday => "weekday",
                DatePart::Week => "week",
                DatePart::Hour => "hour",
                DatePart::Minute => "minute",
                DatePart::Second => "second",
            };
            let name = format!("{prefix}{suffix}");
            validate_output_name(&name)?;
            let values = parsed
                .iter()
                .map(|value| {
                    value.map(|value| match part {
                        DatePart::Year => i64::from(value.year()),
                        DatePart::Month => i64::from(value.month()),
                        DatePart::Day => i64::from(value.day()),
                        DatePart::Quarter => i64::from((value.month() - 1) / 3 + 1),
                        DatePart::Weekday => i64::from(value.weekday().num_days_from_monday()),
                        DatePart::Week => i64::from(value.iso_week().week()),
                        DatePart::Hour => i64::from(value.hour()),
                        DatePart::Minute => i64::from(value.minute()),
                        DatePart::Second => i64::from(value.second()),
                    })
                })
                .collect::<Vec<_>>();
            result = replace_or_append(
                &result,
                &name,
                DataType::Int64,
                true,
                Arc::new(Int64Array::from(values)),
            )?;
        }
        Ok(result)
    }

    fn all_parts() -> Vec<DatePart> {
        vec![
            DatePart::Year,
            DatePart::Month,
            DatePart::Day,
            DatePart::Quarter,
            DatePart::Weekday,
            DatePart::Week,
            DatePart::Hour,
            DatePart::Minute,
            DatePart::Second,
        ]
    }

    fn assert_equivalent(fast: Result<RecordBatch>, generic: Result<RecordBatch>) {
        match (fast, generic) {
            (Ok(fast), Ok(generic)) => assert_eq!(fast, generic),
            (fast, generic) => assert_eq!(fast.is_err(), generic.is_err()),
        }
    }

    #[test]
    fn limit_slices_rows_and_preserves_schema() {
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3, 4])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("d"),
                    Some("e"),
                ])),
            ],
        )
        .expect("fixture");
        let ids = |batch: &RecordBatch| {
            batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("ids")
                .values()
                .to_vec()
        };
        // n semplice.
        let first = limit(&input, &Limit { n: 2, offset: 0 }).expect("limit");
        assert_eq!(ids(&first), vec![0, 1]);
        assert_eq!(first.schema(), input.schema());
        // offset + n.
        let window = limit(&input, &Limit { n: 2, offset: 1 }).expect("window");
        assert_eq!(ids(&window), vec![1, 2]);
        // null preservato nella finestra.
        assert!(window.column(1).is_null(1));
        // n = 0: batch vuoto con schema invariato.
        let empty = limit(&input, &Limit { n: 0, offset: 0 }).expect("empty");
        assert_eq!(empty.num_rows(), 0);
        assert_eq!(empty.schema(), input.schema());
        // n > righe: tutto il batch.
        let all = limit(&input, &Limit { n: 100, offset: 0 }).expect("all");
        assert_eq!(all.num_rows(), 5);
        // offset oltre le righe: vuoto; offset parziale: clampa.
        assert_eq!(
            limit(&input, &Limit { n: 3, offset: 10 })
                .expect("beyond")
                .num_rows(),
            0
        );
        assert_eq!(ids(&limit(&input, &Limit { n: 3, offset: 3 }).expect("tail")), vec![3, 4]);
        // Default serde: offset 0; config strict.
        let decoded: Limit = serde_json::from_value(json!({"n": 1})).expect("default offset");
        assert_eq!(decoded.offset, 0);
        assert!(serde_json::from_value::<Limit>(json!({"n": 1, "bogus": 1})).is_err());
    }

    #[test]
    fn date_extract_fast_path_matches_generic_with_explicit_format() {
        // Date limite: epoch, pre-1970, bisestile, confine ISO week, date-only.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("ts", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("1970-01-01 00:00:00"), // epoch, ISO week 1, giovedi'
                Some("1969-12-31 23:59:59"), // pre-epoch
                Some("2000-02-29 12:30:45"), // bisestile
                Some("2021-01-01 06:07:08"), // ISO week 53 del 2020
                Some("2019-12-30 23:59:59"), // ISO week 1 del 2020
                Some("2023-02-29 00:00:00"), // inesistente
                Some("2024-02-29"),          // date-only: fallisce
                Some(""),
                None,
            ]))],
        )
        .expect("fixture");
        for invalid in [InvalidDatePolicy::Null, InvalidDatePolicy::Error] {
            let config = DateExtract {
                column: "ts".into(),
                parts: all_parts(),
                prefix: String::new(),
                date_format: Some("%Y-%m-%d %H:%M:%S".into()),
                invalid,
            };
            assert_equivalent(
                date_extract(&batch, &config),
                generic_date_extract(&batch, &config),
            );
        }
        // Formato esplicito date-only (fallback `NaiveDate`).
        let config = DateExtract {
            column: "ts".into(),
            parts: all_parts(),
            prefix: "p_".into(),
            date_format: Some("%Y-%m-%d".into()),
            invalid: InvalidDatePolicy::Null,
        };
        assert_equivalent(
            date_extract(&batch, &config),
            generic_date_extract(&batch, &config),
        );
    }

    #[test]
    fn date_extract_fast_path_matches_generic_with_default_multi_format() {
        // Tutti i formati del parser di default, piu' valori non parsabili.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("ts", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("2024-01-15T10:30:00"),
                Some("2024-01-15 10:30:00"),
                Some("15/01/2024 10:30:00"),
                Some("2024-01-15"),
                Some("15/01/2024"),
                Some("15-01-2024"),
                Some("2024/01/15"),
                Some("1970-01-01"),
                Some("1969-12-31 23:59:59"),
                Some("29/02/2000"),
                Some("29/02/2023"), // inesistente
                Some("2024-13-01"), // mese 13
                Some("non una data"),
                Some(""),
                None,
            ]))],
        )
        .expect("fixture");
        for invalid in [InvalidDatePolicy::Null, InvalidDatePolicy::Error] {
            let config = DateExtract {
                column: "ts".into(),
                parts: all_parts(),
                prefix: String::new(),
                date_format: None,
                invalid,
            };
            assert_equivalent(
                date_extract(&batch, &config),
                generic_date_extract(&batch, &config),
            );
        }
    }
}
