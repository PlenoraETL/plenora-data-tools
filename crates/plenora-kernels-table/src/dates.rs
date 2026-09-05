use std::sync::Arc;

use chrono::format::{Item, Parsed, StrftimeItems};
use chrono::{LocalResult, Months, NaiveDate, NaiveDateTime, TimeDelta, TimeZone};
use chrono_tz::Tz;
use plenora_core::arrow::array::{Array, Float64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use crate::utility::InvalidDatePolicy;
use crate::{column_index, reject_rows, replace_or_append, scalar_as_string, RowRejection};
use plenora_core::{PlenoraError, Result};

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
// Fast path dei kernel data.
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

/// Valida un formato strftime (`label` identifica il campo nei messaggi).
///
/// # Errors
///
/// - `InvalidPlan`: formato vuoto o oltre `max_bytes`, oppure contenente
///   item strftime non riconosciuti.
pub fn validate_format(format: &str, label: &str, max_bytes: usize) -> Result<()> {
    if format.is_empty() || format.len() > max_bytes {
        return Err(PlenoraError::InvalidPlan(format!("{label} non valido")));
    }
    if StrftimeItems::new(format).any(|item| matches!(item, Item::Error)) {
        return Err(PlenoraError::InvalidPlan(format!(
            "{label} non riconosciuto"
        )));
    }
    Ok(())
}

fn invalid<T>(_policy: &InvalidDatePolicy, operation: &str, _row: usize) -> Result<Option<T>> {
    Err(PlenoraError::Internal(format!(
        "prevalidazione row-scoped incoerente in {operation}"
    )))
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

/// Riformatta la colonna `column` da `input_format` a `output_format`
/// nella colonna `output_column`.
///
/// Fast path su colonne Utf8 (item strftime precompilati), percorso
/// generico riga-per-riga sugli altri tipi Arrow. Il token `invalid` resta
/// compatibile in input, ma i valori non parsabili sono sempre rifiutati con
/// diagnostica row-scoped.
///
/// # Errors
///
/// - `DataMapping`: uno o piu' valori non parsabili, con row diagnostics;
/// - `Schema`: colonna assente (come `column_index`) o tipo non
///   supportato dal profilo scalare (come `scalar_as_string`);
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna
///   di `replace_or_append`).
pub fn date_format(batch: &RecordBatch, config: &DateFormat) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        if scalar_as_string(source.as_ref(), row)?
            .is_some_and(|value| parse(&value, &config.input_format).is_none())
        {
            rejections.push(RowRejection {
                row,
                cause: "conversion.invalid_datetime",
                column: Some(&config.column),
            });
        }
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
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

/// Somma `amount` unita' (`unit`) ai valori della colonna `column`,
/// riscritti con `output_format` nella colonna `output_column`.
///
/// Anni e mesi usano aritmetica di calendario (`Months`), le altre
/// unita' durate fisse; valori non parsabili e overflow di data o delta sono
/// sempre rifiutati con diagnostica row-scoped.
///
/// # Errors
///
/// - `DataMapping`: valore non parsabile, oppure data risultante o delta
///   fuori range, con row diagnostics;
/// - `Schema`: colonna assente (come `column_index`) o tipo non
///   supportato dal profilo scalare (come `scalar_as_string`);
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna
///   di `replace_or_append`).
pub fn date_add(batch: &RecordBatch, config: &DateAdd) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        let Some(value) = scalar_as_string(source.as_ref(), row)? else {
            continue;
        };
        let cause = match parse(&value, &config.input_format) {
            None => "conversion.invalid_datetime",
            Some(value) if shift(value, config.amount, &config.unit).is_none() => {
                "conversion.datetime_range"
            }
            Some(_) => continue,
        };
        rejections.push(RowRejection {
            row,
            cause,
            column: Some(&config.column),
        });
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
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
            let shifted = parse_with_items(column.value(row), &input_items).and_then(shift_row);
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
///
/// # Arrotondamento dichiarato
///
/// Il risultato e' per contratto un `Float64` in unita' frazionarie, quindi
/// la conversione dei nanosecondi a `f64` e' volutamente arrotondata: oltre
/// 2^53 nanosecondi — circa 104 giorni — il conteggio esatto non entra in un
/// double, e pretendere l'esattezza qui rifiuterebbe ogni intervallo di
/// qualche mese, che e' l'uso normale dell'operazione.
///
/// «Fuori scala» riguarda percio' i soli nanosecondi oltre `i64`
/// (`num_nanoseconds` restituisce `None`): circa 292 anni. Affiancarvi un
/// `to_f64()`, che non fallisce mai, dichiarerebbe un controllo di
/// rappresentabilita' inesistente.
#[allow(clippy::cast_precision_loss)] // Arrotondamento voluto: l'output e' Float64 per contratto.
fn diff_value(start: NaiveDateTime, end: NaiveDateTime, divisor: f64, _row: usize) -> Result<f64> {
    end.signed_duration_since(start)
        .num_nanoseconds()
        .map(|nanoseconds| nanoseconds as f64 / 1_000_000_000.0 / divisor)
        .ok_or_else(|| PlenoraError::InvalidPlan("date_diff: intervallo fuori scala".into()))
}

/// Differenza `end_column - start_column` in unita' frazionarie
/// (`unit`), scritta come Float64 in `output_column`.
///
/// Un estremo null propaga null; un estremo non parsabile o un intervallo fuori
/// scala rifiuta sempre l'output con diagnostica row-scoped.
///
/// # Errors
///
/// - `DataMapping`: valore non parsabile oppure intervallo fuori scala
///   (nanosecondi oltre `i64` o non rappresentabili in `f64`), con row
///   diagnostics;
/// - `Schema`: colonna assente (come `column_index`) o tipo non
///   supportato dal profilo scalare (come `scalar_as_string`);
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna
///   di `replace_or_append`).
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
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        let start = scalar_as_string(start_source.as_ref(), row)?;
        let end = scalar_as_string(end_source.as_ref(), row)?;
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        let parsed_start = parse(&start, &config.input_format);
        let parsed_end = parse(&end, &config.input_format);
        let (cause, column) = match (parsed_start, parsed_end) {
            (None, _) => (
                "conversion.invalid_datetime",
                Some(config.start_column.as_str()),
            ),
            (_, None) => (
                "conversion.invalid_datetime",
                Some(config.end_column.as_str()),
            ),
            (Some(start), Some(end))
                if end.signed_duration_since(start).num_nanoseconds().is_none() =>
            {
                ("conversion.datetime_range", None)
            }
            (Some(_), Some(_)) => continue,
        };
        rejections.push(RowRejection { row, cause, column });
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
    let values = if let (Some(starts), Some(ends)) = (
        start_source.as_any().downcast_ref::<StringArray>(),
        end_source.as_any().downcast_ref::<StringArray>(),
    ) {
        let input_items = compile_items(&config.input_format);
        let mut values = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            if starts.is_null(row) || ends.is_null(row) {
                values.push(None);
                continue;
            }
            let parsed = parse_with_items(starts.value(row), &input_items)
                .zip(parse_with_items(ends.value(row), &input_items));
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
                if start.is_none() || end.is_none() {
                    return Ok(None);
                }
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
    _policy: &AmbiguousPolicy,
    _row: usize,
) -> Result<Option<chrono::DateTime<Tz>>> {
    match timezone.from_local_datetime(&value) {
        LocalResult::Single(value) => Ok(Some(value)),
        LocalResult::Ambiguous(_, _) | LocalResult::None => Err(PlenoraError::Internal(
            "prevalidazione row-scoped incoerente in timezone_convert".into(),
        )),
    }
}

/// Converte la colonna `column` da `source_timezone` a
/// `target_timezone`, riscritta con `output_format` nella colonna
/// `output_column`.
///
/// Le policy `ambiguous` e `invalid` restano token di compatibilita': ore
/// ambigue/inesistenti e valori non parsabili sono sempre rifiutati con
/// diagnostica row-scoped, senza scelta o null sintetico.
///
/// # Errors
///
/// - `InvalidPlan`: `source_timezone` o `target_timezone` non valida;
/// - `DataMapping`: ora ambigua/inesistente o valore non parsabile, con row
///   diagnostics;
/// - `Schema`: colonna assente (come `column_index`) o tipo non
///   supportato dal profilo scalare (come `scalar_as_string`);
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna
///   di `replace_or_append`).
pub fn timezone_convert(batch: &RecordBatch, config: &TimezoneConvert) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let source_tz: Tz = config
        .source_timezone
        .parse()
        .map_err(|_| PlenoraError::InvalidPlan("source_timezone non valida".into()))?;
    let target_tz: Tz = config
        .target_timezone
        .parse()
        .map_err(|_| PlenoraError::InvalidPlan("target_timezone non valida".into()))?;
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        let Some(value) = scalar_as_string(source.as_ref(), row)? else {
            continue;
        };
        let Some(parsed) = parse(&value, &config.input_format) else {
            rejections.push(RowRejection {
                row,
                cause: "conversion.invalid_datetime",
                column: Some(&config.column),
            });
            continue;
        };
        let cause = match source_tz.from_local_datetime(&parsed) {
            LocalResult::Single(_) => continue,
            LocalResult::Ambiguous(_, _) => "conversion.ambiguous_local_time",
            LocalResult::None => "conversion.nonexistent_local_time",
        };
        rejections.push(RowRejection {
            row,
            cause,
            column: Some(&config.column),
        });
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
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
    use plenora_core::diagnostics::RowDiagnosticsCompleteness;

    use super::*;

    // -----------------------------------------------------------------------
    // Percorsi generici, indipendenti dai fast path: sono l'oracolo della
    // loro equivalenza semantica.
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
            .map_err(|_| PlenoraError::InvalidPlan("source_timezone non valida".into()))?;
        let target: Tz = config
            .target_timezone
            .parse()
            .map_err(|_| PlenoraError::InvalidPlan("target_timezone non valida".into()))?;
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
            (Err(fast), _) if fast.row_diagnostics().is_some() => {}
            (fast, generic) => assert_eq!(fast.is_err(), generic.is_err()),
        }
    }

    fn assert_complete_rows(error: &PlenoraError, expected: &[u64], column: &str) {
        let report = error
            .row_diagnostics()
            .expect("diagnostica row-scoped mancante");
        assert_eq!(report.completeness, RowDiagnosticsCompleteness::Complete);
        assert_eq!(
            report.observed_total,
            u64::try_from(expected.len()).expect("fixture")
        );
        assert_eq!(report.total, Some(report.observed_total));
        assert_eq!(
            report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(report
            .examples
            .iter()
            .all(|example| example.column.as_deref() == Some(column)));
    }

    #[test]
    fn temporal_transforms_reject_all_invalid_rows_even_with_legacy_null_policy() {
        let batch = utf8_batch(vec![
            Some("2024-01-01 00:00:00"),
            Some("non una data"),
            None,
            Some("2023-02-29 00:00:00"),
        ]);
        let format = format_config(InvalidDatePolicy::Null);
        assert_complete_rows(
            &date_format(&batch, &format).expect_err("date_format ha accettato righe invalide"),
            &[1, 3],
            "ts",
        );
        let add = DateAdd {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%Y-%m-%d %H:%M:%S".into(),
            amount: 1,
            unit: DateUnit::Days,
            output_column: "out".into(),
            invalid: InvalidDatePolicy::Null,
        };
        assert_complete_rows(
            &date_add(&batch, &add).expect_err("date_add ha accettato righe invalide"),
            &[1, 3],
            "ts",
        );

        let timezone = TimezoneConvert {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%Y-%m-%d %H:%M:%S".into(),
            source_timezone: "Europe/Rome".into(),
            target_timezone: "UTC".into(),
            output_column: "out".into(),
            invalid: InvalidDatePolicy::Null,
            ambiguous: AmbiguousPolicy::Earliest,
        };
        let dst_batch = utf8_batch(vec![
            Some("2024-01-01 00:00:00"),
            Some("2024-10-27 02:30:00"),
            Some("non una data"),
        ]);
        assert_complete_rows(
            &timezone_convert(&dst_batch, &timezone)
                .expect_err("timezone_convert ha rimediato righe invalide"),
            &[1, 2],
            "ts",
        );
    }

    #[test]
    fn date_diff_preserves_null_as_valid_missing_data() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("start", DataType::Utf8, true),
                Field::new("end", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![None, Some("2024-01-01 00:00:00")])),
                Arc::new(StringArray::from(vec![Some("2024-01-02 00:00:00"), None])),
            ],
        )
        .expect("fixture");
        let output = date_diff(
            &batch,
            &DateDiff {
                start_column: "start".into(),
                end_column: "end".into(),
                input_format: "%Y-%m-%d %H:%M:%S".into(),
                unit: DiffUnit::Days,
                output_column: "out".into(),
                invalid: InvalidDatePolicy::Error,
            },
        )
        .expect("null ammessi");
        assert_eq!(output.column(2).null_count(), 2);
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
            assert_equivalent(
                date_format(&batch, &config),
                generic_date_format(&batch, &config),
            );
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
                assert_equivalent(
                    date_diff(&batch, &config),
                    generic_date_diff(&batch, &config),
                );
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
        let error = date_format(&batch, &config).expect_err("valore non temporale accettato");
        let report = error
            .row_diagnostics()
            .expect("diagnostica fallback mancante");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.examples[0].cause, "conversion.invalid_datetime");
    }
}
