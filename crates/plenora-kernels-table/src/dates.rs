use std::sync::Arc;

use plenora_core::arrow::array::{Array, Float64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use chrono::format::{Item, Parsed, StrftimeItems};
use chrono::{LocalResult, Months, NaiveDate, NaiveDateTime, TimeDelta, TimeZone};
use chrono_tz::Tz;
use num_traits::ToPrimitive;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};
use crate::utility::InvalidDatePolicy;
use crate::{column_index, replace_or_append, scalar_as_string};

fn default_output_format() -> String {
    "%Y-%m-%d %H:%M:%S".into()
}

fn parse(value: &str, format: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, format)
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(value, format)
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
        })
}

// ---------------------------------------------------------------------------
// Fast path (ottimizzazione kernel data, secondo batch filone kernel).
//
// Il costo dominante dei kernel data e' doppio: la conversione scalare per
// riga (`scalar_as_string`, con allocazione `String` per ogni valore) e il
// parsing strftime dei formati chrono, rieseguito a ogni riga sia in lettura
// (`parse_from_str`) sia in scrittura (`format`). Qui gli item strftime sono
// compilati UNA VOLTA (`compile_items`) e il loop lavora sui `&str` nativi di
// una colonna Utf8; per gli altri tipi Arrow si ricade sul percorso generico
// riga-per-riga, che riproduce esattamente il comportamento originale.
// ---------------------------------------------------------------------------

/// Item strftime precompilati di un formato chrono (prestazione, semantica
/// invariata: stessi item che chrono ri-parserizzerebbe a ogni riga).
pub(crate) fn compile_items(format: &str) -> Vec<Item<'_>> {
    StrftimeItems::new(format).collect()
}

/// Parsing con item precompilati, semantica identica a `parse`: prima il
/// ramo `NaiveDateTime::parse_from_str` (campi orario di default a
/// mezzanotte), poi il fallback `NaiveDate::parse_from_str`.
pub(crate) fn parse_with_items(value: &str, items: &[Item<'_>]) -> Option<NaiveDateTime> {
    let mut parsed = Parsed::new();
    if chrono::format::parse(&mut parsed, value, items.iter()).is_ok() {
        // Stessa risoluzione di `NaiveDateTime::parse_from_str`
        // (`to_naive_datetime_with_offset(0)` in chrono 0.4.42).
        if let Ok(datetime) = parsed.to_naive_datetime_with_offset(0) {
            return Some(datetime);
        }
    }
    let mut parsed = Parsed::new();
    if chrono::format::parse(&mut parsed, value, items.iter()).is_ok() {
        if let Ok(date) = parsed.to_naive_date() {
            return date.and_hms_opt(0, 0, 0);
        }
    }
    None
}

pub fn validate_format(format: &str, label: &str, max_bytes: usize) -> Result<()> {
    if format.is_empty() || format.len() > max_bytes {
        return Err(PlenoraError::Contract(format!("{label} non valido")));
    }
    if StrftimeItems::new(format).any(|item| matches!(item, Item::Error)) {
        return Err(PlenoraError::Contract(format!("{label} non riconosciuto")));
    }
    Ok(())
}

fn invalid<T>(policy: &InvalidDatePolicy, operation: &str, row: usize) -> Result<Option<T>> {
    if matches!(policy, InvalidDatePolicy::Null) {
        Ok(None)
    } else {
        Err(PlenoraError::Contract(format!(
            "{operation}: valore temporale non valido alla riga {row}"
        )))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateFormat {
    pub column: String,
    pub input_format: String,
    #[serde(default = "default_output_format")]
    pub output_format: String,
    pub output_column: String,
    #[serde(default = "default_invalid")]
    pub invalid: InvalidDatePolicy,
}

const fn default_invalid() -> InvalidDatePolicy {
    InvalidDatePolicy::Null
}

pub fn date_format(batch: &RecordBatch, config: &DateFormat) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let values = if let Some(column) = source.as_any().downcast_ref::<StringArray>() {
        let input_items = compile_items(&config.input_format);
        let output_items = compile_items(&config.output_format);
        let mut values = Vec::with_capacity(column.len());
        for row in 0..column.len() {
            if column.is_null(row) {
                values.push(None);
                continue;
            }
            let parsed = parse_with_items(column.value(row), &input_items);
            values.push(match parsed {
                Some(value) => Some(value.format_with_items(output_items.iter()).to_string()),
                None => invalid(&config.invalid, "date_format", row)?,
            });
        }
        values
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    return Ok(None);
                };
                parse(&value, &config.input_format).map_or_else(
                    || invalid(&config.invalid, "date_format", row),
                    |value| Ok(Some(value.format(&config.output_format).to_string())),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateUnit {
    Years,
    Months,
    Weeks,
    Days,
    Hours,
    Minutes,
    Seconds,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateAdd {
    pub column: String,
    pub input_format: String,
    #[serde(default = "default_output_format")]
    pub output_format: String,
    pub amount: i64,
    pub unit: DateUnit,
    pub output_column: String,
    #[serde(default = "default_invalid")]
    pub invalid: InvalidDatePolicy,
}

fn shift_months(value: NaiveDateTime, amount: i64, multiplier: u32) -> Option<NaiveDateTime> {
    let count = amount.unsigned_abs().checked_mul(u64::from(multiplier))?;
    let months = Months::new(u32::try_from(count).ok()?);
    if amount < 0 {
        value.checked_sub_months(months)
    } else {
        value.checked_add_months(months)
    }
}

fn shift(value: NaiveDateTime, amount: i64, unit: &DateUnit) -> Option<NaiveDateTime> {
    let delta = match unit {
        DateUnit::Years => return shift_months(value, amount, 12),
        DateUnit::Months => return shift_months(value, amount, 1),
        DateUnit::Weeks => TimeDelta::try_weeks(amount),
        DateUnit::Days => TimeDelta::try_days(amount),
        DateUnit::Hours => TimeDelta::try_hours(amount),
        DateUnit::Minutes => TimeDelta::try_minutes(amount),
        DateUnit::Seconds => TimeDelta::try_seconds(amount),
    }?;
    value.checked_add_signed(delta)
}

pub fn date_add(batch: &RecordBatch, config: &DateAdd) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let values = if let Some(column) = source.as_any().downcast_ref::<StringArray>() {
        let input_items = compile_items(&config.input_format);
        let output_items = compile_items(&config.output_format);
        // Delta precomputato per le unita' a durata fissa (mai ricalcolato
        // per riga); anni/mesi restano sul percorso `Months` per-riga.
        let fixed_delta = match config.unit {
            DateUnit::Years | DateUnit::Months => None,
            DateUnit::Weeks => Some(TimeDelta::try_weeks(config.amount)),
            DateUnit::Days => Some(TimeDelta::try_days(config.amount)),
            DateUnit::Hours => Some(TimeDelta::try_hours(config.amount)),
            DateUnit::Minutes => Some(TimeDelta::try_minutes(config.amount)),
            DateUnit::Seconds => Some(TimeDelta::try_seconds(config.amount)),
        };
        let shift_row = |value: NaiveDateTime| -> Option<NaiveDateTime> {
            match fixed_delta {
                Some(delta) => delta.and_then(|delta| value.checked_add_signed(delta)),
                None if matches!(config.unit, DateUnit::Years) => {
                    shift_months(value, config.amount, 12)
                }
                None => shift_months(value, config.amount, 1),
            }
        };
        let mut values = Vec::with_capacity(column.len());
        for row in 0..column.len() {
            if column.is_null(row) {
                values.push(None);
                continue;
            }
            let shifted =
                parse_with_items(column.value(row), &input_items).and_then(shift_row);
            values.push(match shifted {
                Some(value) => Some(value.format_with_items(output_items.iter()).to_string()),
                None => invalid(&config.invalid, "date_add", row)?,
            });
        }
        values
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    return Ok(None);
                };
                let shifted = parse(&value, &config.input_format)
                    .and_then(|value| shift(value, config.amount, &config.unit));
                shifted.map_or_else(
                    || invalid(&config.invalid, "date_add", row),
                    |value| Ok(Some(value.format(&config.output_format).to_string())),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffUnit {
    Days,
    Hours,
    Minutes,
    Seconds,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateDiff {
    pub start_column: String,
    pub end_column: String,
    pub input_format: String,
    pub unit: DiffUnit,
    pub output_column: String,
    #[serde(default = "default_invalid")]
    pub invalid: InvalidDatePolicy,
}

/// Differenza in unita' frazionarie, identica al percorso generico
/// (errore "intervallo fuori scala" incluso).
fn diff_value(
    start: NaiveDateTime,
    end: NaiveDateTime,
    divisor: f64,
    row: usize,
) -> Result<f64> {
    end.signed_duration_since(start)
        .num_nanoseconds()
        .and_then(|nanoseconds| nanoseconds.to_f64())
        .map(|nanoseconds| nanoseconds / 1_000_000_000.0 / divisor)
        .ok_or_else(|| {
            PlenoraError::Contract(format!(
                "date_diff: intervallo fuori scala alla riga {row}"
            ))
        })
}

pub fn date_diff(batch: &RecordBatch, config: &DateDiff) -> Result<RecordBatch> {
    let start_index = column_index(batch, &config.start_column)?;
    let end_index = column_index(batch, &config.end_column)?;
    let divisor = match config.unit {
        DiffUnit::Days => 86_400.0,
        DiffUnit::Hours => 3_600.0,
        DiffUnit::Minutes => 60.0,
        DiffUnit::Seconds => 1.0,
    };
    let start_source = batch.column(start_index);
    let end_source = batch.column(end_index);
    let values = if let (Some(starts), Some(ends)) = (
        start_source.as_any().downcast_ref::<StringArray>(),
        end_source.as_any().downcast_ref::<StringArray>(),
    ) {
        let input_items = compile_items(&config.input_format);
        let mut values = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            let parsed = if starts.is_null(row) || ends.is_null(row) {
                None
            } else {
                parse_with_items(starts.value(row), &input_items)
                    .zip(parse_with_items(ends.value(row), &input_items))
            };
            values.push(match parsed {
                Some((start, end)) => Some(diff_value(start, end, divisor, row)?),
                None => invalid(&config.invalid, "date_diff", row)?,
            });
        }
        values
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let start = scalar_as_string(start_source.as_ref(), row)?;
                let end = scalar_as_string(end_source.as_ref(), row)?;
                let parsed = start
                    .as_deref()
                    .and_then(|value| parse(value, &config.input_format))
                    .zip(
                        end.as_deref()
                            .and_then(|value| parse(value, &config.input_format)),
                    );
                parsed.map_or_else(
                    || invalid(&config.invalid, "date_diff", row),
                    |(start, end)| diff_value(start, end, divisor, row).map(Some),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Float64,
        true,
        Arc::new(Float64Array::from(values)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguousPolicy {
    Error,
    Null,
    Earliest,
    Latest,
}

const fn default_ambiguous() -> AmbiguousPolicy {
    AmbiguousPolicy::Error
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimezoneConvert {
    pub column: String,
    pub input_format: String,
    #[serde(default = "default_output_format")]
    pub output_format: String,
    pub source_timezone: String,
    pub target_timezone: String,
    pub output_column: String,
    #[serde(default = "default_invalid")]
    pub invalid: InvalidDatePolicy,
    #[serde(default = "default_ambiguous")]
    pub ambiguous: AmbiguousPolicy,
}

fn localize(
    timezone: Tz,
    value: NaiveDateTime,
    policy: &AmbiguousPolicy,
    row: usize,
) -> Result<Option<chrono::DateTime<Tz>>> {
    match timezone.from_local_datetime(&value) {
        LocalResult::Single(value) => Ok(Some(value)),
        LocalResult::Ambiguous(first, second) => match policy {
            AmbiguousPolicy::Earliest => Ok(Some(first.min(second))),
            AmbiguousPolicy::Latest => Ok(Some(first.max(second))),
            AmbiguousPolicy::Null => Ok(None),
            AmbiguousPolicy::Error => Err(PlenoraError::Contract(format!(
                "timezone_convert: ora ambigua alla riga {row}"
            ))),
        },
        LocalResult::None => Ok(None),
    }
}

pub fn timezone_convert(
    batch: &RecordBatch,
    config: &TimezoneConvert,
) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let source_tz: Tz = config
        .source_timezone
        .parse()
        .map_err(|_| PlenoraError::Contract("source_timezone non valida".into()))?;
    let target_tz: Tz = config
        .target_timezone
        .parse()
        .map_err(|_| PlenoraError::Contract("target_timezone non valida".into()))?;
    let values = if let Some(column) = source.as_any().downcast_ref::<StringArray>() {
        let input_items = compile_items(&config.input_format);
        let output_items = compile_items(&config.output_format);
        let mut values = Vec::with_capacity(column.len());
        for row in 0..column.len() {
            if column.is_null(row) {
                values.push(None);
                continue;
            }
            let Some(parsed) = parse_with_items(column.value(row), &input_items) else {
                values.push(invalid(&config.invalid, "timezone_convert", row)?);
                continue;
            };
            let localized = localize(source_tz, parsed, &config.ambiguous, row)?;
            values.push(match localized {
                Some(value) => Some(
                    value
                        .with_timezone(&target_tz)
                        .format_with_items(output_items.iter())
                        .to_string(),
                ),
                None => invalid(&config.invalid, "timezone_convert", row)?,
            });
        }
        values
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    return Ok(None);
                };
                let Some(parsed) = parse(&value, &config.input_format) else {
                    return invalid(&config.invalid, "timezone_convert", row);
                };
                let localized = localize(source_tz, parsed, &config.ambiguous, row)?;
                localized.map_or_else(
                    || invalid(&config.invalid, "timezone_convert", row),
                    |value| {
                        Ok(Some(
                            value
                                .with_timezone(&target_tz)
                                .format(&config.output_format)
                                .to_string(),
                        ))
                    },
                )
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{ArrayRef, Int64Array};
    use plenora_core::arrow::schema::{Field, Schema};

    use super::*;

    // -----------------------------------------------------------------------
    // Percorsi generici pre-ottimizzazione: riferimento per l'equivalenza
    // semantica (oracolo) dei fast path.
    // -----------------------------------------------------------------------

    fn generic_date_format(batch: &RecordBatch, config: &DateFormat) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let values = (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(batch.column(index).as_ref(), row)? else {
                    return Ok(None);
                };
                parse(&value, &config.input_format).map_or_else(
                    || invalid(&config.invalid, "date_format", row),
                    |value| Ok(Some(value.format(&config.output_format).to_string())),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(values)),
        )
    }

    fn generic_date_add(batch: &RecordBatch, config: &DateAdd) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let values = (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(batch.column(index).as_ref(), row)? else {
                    return Ok(None);
                };
                let shifted = parse(&value, &config.input_format)
                    .and_then(|value| shift(value, config.amount, &config.unit));
                shifted.map_or_else(
                    || invalid(&config.invalid, "date_add", row),
                    |value| Ok(Some(value.format(&config.output_format).to_string())),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(values)),
        )
    }

    fn generic_date_diff(batch: &RecordBatch, config: &DateDiff) -> Result<RecordBatch> {
        let start_index = column_index(batch, &config.start_column)?;
        let end_index = column_index(batch, &config.end_column)?;
        let divisor = match config.unit {
            DiffUnit::Days => 86_400.0,
            DiffUnit::Hours => 3_600.0,
            DiffUnit::Minutes => 60.0,
            DiffUnit::Seconds => 1.0,
        };
        let values = (0..batch.num_rows())
            .map(|row| {
                let start = scalar_as_string(batch.column(start_index).as_ref(), row)?;
                let end = scalar_as_string(batch.column(end_index).as_ref(), row)?;
                let parsed = start
                    .as_deref()
                    .and_then(|value| parse(value, &config.input_format))
                    .zip(
                        end.as_deref()
                            .and_then(|value| parse(value, &config.input_format)),
                    );
                parsed.map_or_else(
                    || invalid(&config.invalid, "date_diff", row),
                    |(start, end)| diff_value(start, end, divisor, row).map(Some),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(values)),
        )
    }

    fn generic_timezone_convert(
        batch: &RecordBatch,
        config: &TimezoneConvert,
    ) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let source: Tz = config
            .source_timezone
            .parse()
            .map_err(|_| PlenoraError::Contract("source_timezone non valida".into()))?;
        let target: Tz = config
            .target_timezone
            .parse()
            .map_err(|_| PlenoraError::Contract("target_timezone non valida".into()))?;
        let values = (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(batch.column(index).as_ref(), row)? else {
                    return Ok(None);
                };
                let Some(parsed) = parse(&value, &config.input_format) else {
                    return invalid(&config.invalid, "timezone_convert", row);
                };
                let localized = localize(source, parsed, &config.ambiguous, row)?;
                localized.map_or_else(
                    || invalid(&config.invalid, "timezone_convert", row),
                    |value| {
                        Ok(Some(
                            value
                                .with_timezone(&target)
                                .format(&config.output_format)
                                .to_string(),
                        ))
                    },
                )
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(values)),
        )
    }

    fn utf8_batch(values: Vec<Option<&str>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("ts", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(values))],
        )
        .expect("fixture")
    }

    /// Date limite: epoch, pre-1970, anni bisestili e non, non valida, vuota,
    /// date-only, estremi di range chrono, transizioni DST Europe/Rome
    /// (ora inesistente e ora ambigua), null.
    fn edge_values() -> Vec<Option<&'static str>> {
        vec![
            Some("1970-01-01 00:00:00"), // epoch
            Some("1969-12-31 23:59:59"), // pre-epoch
            Some("1900-01-01 00:00:00"),
            Some("2000-02-29 12:30:45"), // bisestile (divisibile per 400)
            Some("2100-02-28 23:59:59"), // 2100 NON bisestile
            Some("2023-02-29 00:00:00"), // data inesistente
            Some("2024-10-27 02:30:00"), // ambigua Europe/Rome (fine DST)
            Some("2024-03-31 02:30:00"), // inesistente Europe/Rome (inizio DST)
            Some("2024-02-29"),          // date-only (formato datetime: fallisce)
            Some(""),                    // vuota
            Some("non una data"),
            Some("9999-12-31 23:59:59"),
            None,
        ]
    }

    fn assert_equivalent(fast: Result<RecordBatch>, generic: Result<RecordBatch>) {
        match (fast, generic) {
            (Ok(fast), Ok(generic)) => assert_eq!(fast, generic),
            (fast, generic) => assert_eq!(fast.is_err(), generic.is_err()),
        }
    }

    fn format_config(invalid: InvalidDatePolicy) -> DateFormat {
        DateFormat {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%d/%m/%Y %H:%M:%S".into(),
            output_column: "out".into(),
            invalid,
        }
    }

    #[test]
    fn date_format_fast_path_matches_generic_on_edge_dates() {
        let batch = utf8_batch(edge_values());
        for invalid in [InvalidDatePolicy::Null, InvalidDatePolicy::Error] {
            let config = format_config(invalid);
            assert_equivalent(date_format(&batch, &config), generic_date_format(&batch, &config));
        }
        // Formato date-only: il fallback `NaiveDate` deve coincidere.
        let dates_only = utf8_batch(vec![
            Some("1970-01-01"),
            Some("2000-02-29"),
            Some("2023-02-29"),
            Some("2024-02-29 10:00:00"), // trailing input: fallisce
            None,
        ]);
        let config = DateFormat {
            input_format: "%Y-%m-%d".into(),
            ..format_config(InvalidDatePolicy::Null)
        };
        assert_equivalent(
            date_format(&dates_only, &config),
            generic_date_format(&dates_only, &config),
        );
        let config = DateFormat {
            input_format: "%Y-%m-%d".into(),
            ..format_config(InvalidDatePolicy::Error)
        };
        assert_equivalent(
            date_format(&dates_only, &config),
            generic_date_format(&dates_only, &config),
        );
    }

    fn date_unit(code: u8) -> DateUnit {
        match code {
            0 => DateUnit::Years,
            1 => DateUnit::Months,
            2 => DateUnit::Weeks,
            3 => DateUnit::Days,
            4 => DateUnit::Hours,
            5 => DateUnit::Minutes,
            _ => DateUnit::Seconds,
        }
    }

    fn diff_unit(code: u8) -> DiffUnit {
        match code {
            0 => DiffUnit::Days,
            1 => DiffUnit::Hours,
            2 => DiffUnit::Minutes,
            _ => DiffUnit::Seconds,
        }
    }

    fn ambiguous_policy(code: u8) -> AmbiguousPolicy {
        match code {
            0 => AmbiguousPolicy::Error,
            1 => AmbiguousPolicy::Null,
            2 => AmbiguousPolicy::Earliest,
            _ => AmbiguousPolicy::Latest,
        }
    }

    fn invalid_policy(error: bool) -> InvalidDatePolicy {
        if error {
            InvalidDatePolicy::Error
        } else {
            InvalidDatePolicy::Null
        }
    }

    #[test]
    fn date_add_fast_path_matches_generic_on_all_units() {
        let batch = utf8_batch(edge_values());
        for unit_code in 0..7 {
            for (amount, error_policy) in [
                (7, false),
                (-30, false),
                (90_061, true),
                (i64::MAX, false), // overflow delta
            ] {
                let config = DateAdd {
                    column: "ts".into(),
                    input_format: "%Y-%m-%d %H:%M:%S".into(),
                    output_format: "%Y-%m-%d %H:%M:%S".into(),
                    amount,
                    unit: date_unit(unit_code),
                    output_column: "out".into(),
                    invalid: invalid_policy(error_policy),
                };
                assert_equivalent(date_add(&batch, &config), generic_date_add(&batch, &config));
            }
        }
    }

    #[test]
    fn date_diff_fast_path_matches_generic_including_out_of_scale() {
        let starts = vec![
            Some("1970-01-01 00:00:00"),
            Some("2024-03-31 01:59:59"), // attraversa il cambio DST
            Some("2024-10-27 03:00:00"),
            Some("1900-01-01 00:00:00"),
            Some("2024-02-29 00:00:00"),
            Some("non una data"),
            None,
            Some("1900-01-01 00:00:00"), // ~500 anni: nanosecondi fuori i64
        ];
        let ends = vec![
            Some("1969-12-31 23:59:59"), // differenza negativa
            Some("2024-03-31 03:00:01"),
            Some("2024-10-27 01:30:00"),
            Some("2100-01-01 00:00:00"),
            Some("2024-02-29 00:00:00"), // zero
            Some("2024-01-01 00:00:00"),
            Some("2024-01-01 00:00:00"),
            Some("2400-01-01 00:00:00"),
        ];
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("start", DataType::Utf8, true),
                Field::new("end", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(starts)),
                Arc::new(StringArray::from(ends)),
            ],
        )
        .expect("fixture");
        for unit_code in 0..4 {
            for error_policy in [false, true] {
                let config = DateDiff {
                    start_column: "start".into(),
                    end_column: "end".into(),
                    input_format: "%Y-%m-%d %H:%M:%S".into(),
                    unit: diff_unit(unit_code),
                    output_column: "out".into(),
                    invalid: invalid_policy(error_policy),
                };
                assert_equivalent(date_diff(&batch, &config), generic_date_diff(&batch, &config));
            }
        }
    }

    #[test]
    fn timezone_convert_fast_path_matches_generic_on_dst_transitions() {
        let batch = utf8_batch(edge_values());
        for ambiguous_code in 0..4 {
            for error_policy in [false, true] {
                let config = TimezoneConvert {
                    column: "ts".into(),
                    input_format: "%Y-%m-%d %H:%M:%S".into(),
                    output_format: "%Y-%m-%d %H:%M:%S".into(),
                    source_timezone: "Europe/Rome".into(),
                    target_timezone: "UTC".into(),
                    output_column: "out".into(),
                    invalid: invalid_policy(error_policy),
                    ambiguous: ambiguous_policy(ambiguous_code),
                };
                assert_equivalent(
                    timezone_convert(&batch, &config),
                    generic_timezone_convert(&batch, &config),
                );
            }
        }
        // Coppia di timezone senza DST e con DST diversa.
        let config = TimezoneConvert {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%Y-%m-%d %H:%M:%S".into(),
            source_timezone: "America/New_York".into(),
            target_timezone: "Asia/Tokyo".into(),
            output_column: "out".into(),
            invalid: InvalidDatePolicy::Null,
            ambiguous: AmbiguousPolicy::Latest,
        };
        assert_equivalent(
            timezone_convert(&batch, &config),
            generic_timezone_convert(&batch, &config),
        );
        // Timezone non valida: stesso errore prima del loop.
        let bad = TimezoneConvert {
            source_timezone: "Marte/Olympus".into(),
            ..TimezoneConvert {
                column: "ts".into(),
                input_format: "%Y-%m-%d %H:%M:%S".into(),
                output_format: "%Y-%m-%d %H:%M:%S".into(),
                source_timezone: String::new(),
                target_timezone: "UTC".into(),
                output_column: "out".into(),
                invalid: InvalidDatePolicy::Null,
                ambiguous: AmbiguousPolicy::Error,
            }
        };
        assert_equivalent(
            timezone_convert(&batch, &bad),
            generic_timezone_convert(&batch, &bad),
        );
    }

    #[test]
    fn non_utf8_columns_fall_back_to_generic_path() {
        // Colonna Int64: nessun fast path, comportamento del generico.
        let columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(vec![Some(2_024), None]))];
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, true)])),
            columns,
        )
        .expect("fixture");
        let config = DateFormat {
            column: "n".into(),
            ..format_config(InvalidDatePolicy::Null)
        };
        let output = date_format(&batch, &config).expect("fallback");
        assert_eq!(output.num_rows(), 2);
    }
}
