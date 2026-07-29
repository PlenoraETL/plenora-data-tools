use std::cmp::Ordering;

use chrono::{Datelike, NaiveDate, TimeDelta};
use serde_json::Value;

use plenora_core::arrow::array::{Array, Date32Array, RecordBatch, TimestampMillisecondArray};
use plenora_core::arrow::schema::{DataType, TimeUnit};
use plenora_core::{PlenoraError, Result};
use crate::column_index;
use super::interpreter::evaluate;
use super::scalar::{compare, literal, Scalar};
use super::{Expression, Function};

// ---------------------------------------------------------------------------
// date_trunc / in: nodi speciali valutati in `evaluate` (non in `function`)
// perche' richiedono accesso all'AST degli argomenti: `date_trunc` legge la
// colonna temporalmente tipizzata (Date32/Timestamp ms) in modo nativo e `in`
// accetta una lista di letterali (non uno scalare).
// ---------------------------------------------------------------------------

/// Unita' di troncamento di `date_trunc`: set chiuso, validato staticamente
/// in `validate`/`analyze` (l'unita' deve essere un letterale stringa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncUnit {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
}

fn trunc_unit(value: &str) -> Result<TruncUnit> {
    match value {
        "year" => Ok(TruncUnit::Year),
        "month" => Ok(TruncUnit::Month),
        "day" => Ok(TruncUnit::Day),
        "hour" => Ok(TruncUnit::Hour),
        "minute" => Ok(TruncUnit::Minute),
        "second" => Ok(TruncUnit::Second),
        other => Err(PlenoraError::InvalidPlan(format!(
            "date_trunc: unita' non valida: {other}"
        ))),
    }
}

/// L'unita' di `date_trunc` deve essere un letterale stringa del set chiuso:
/// cosi' la validazione a secco rifiuta unita' sconosciute senza dati.
pub fn literal_unit(expression: &Expression) -> Result<TruncUnit> {
    match expression {
        Expression::Literal {
            value: Value::String(unit),
        } => trunc_unit(unit),
        _ => Err(PlenoraError::InvalidPlan(
            "date_trunc: unit deve essere un letterale stringa".into(),
        )),
    }
}

fn date32_epoch() -> Result<NaiveDate> {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .ok_or_else(|| PlenoraError::Internal("epoca Date32 valida".into()))
}

/// Unita' sub-day non ammesse su Date32 (una data non ha componente oraria).
pub fn check_date32_unit(unit: TruncUnit) -> Result<()> {
    if matches!(unit, TruncUnit::Hour | TruncUnit::Minute | TruncUnit::Second) {
        return Err(PlenoraError::InvalidPlan(
            "date_trunc: unita' sub-day non ammessa su Date32".into(),
        ));
    }
    Ok(())
}

/// Troncamento Date32 (giorni dall'epoca) a year/month/day.
pub fn trunc_date32_days(days: i32, unit: TruncUnit) -> Result<i32> {
    check_date32_unit(unit)?;
    let date = date32_epoch()? + TimeDelta::days(i64::from(days));
    let truncated = match unit {
        TruncUnit::Year => NaiveDate::from_ymd_opt(date.year(), 1, 1),
        TruncUnit::Month => NaiveDate::from_ymd_opt(date.year(), date.month(), 1),
        TruncUnit::Day => Some(date),
        TruncUnit::Hour | TruncUnit::Minute | TruncUnit::Second => {
            return Err(PlenoraError::Internal(
                "unita' sub-day gia' rifiutata da check_date32_unit".into(),
            ));
        }
    }
    .ok_or_else(|| PlenoraError::Schema("date_trunc: data fuori range".into()))?;
    i32::try_from((truncated - date32_epoch()?).num_days())
        .map_err(|_| PlenoraError::Schema("date_trunc: data fuori range Date32".into()))
}

/// Troncamento Timestamp(ms) naive UTC: year/month via calendario, unita'
/// day e inferiori per aritmetica sui millisecondi (`rem_euclid` copre i
/// timestamp pre-1970).
pub fn trunc_timestamp_ms_value(ms: i64, unit: TruncUnit) -> Result<i64> {
    Ok(match unit {
        TruncUnit::Second => ms - ms.rem_euclid(1_000),
        TruncUnit::Minute => ms - ms.rem_euclid(60_000),
        TruncUnit::Hour => ms - ms.rem_euclid(3_600_000),
        TruncUnit::Day => ms - ms.rem_euclid(86_400_000),
        TruncUnit::Year | TruncUnit::Month => {
            let datetime = chrono::DateTime::from_timestamp_millis(ms)
                .ok_or_else(|| PlenoraError::Schema("date_trunc: timestamp fuori range".into()))?;
            let date = datetime.date_naive();
            let first = match unit {
                TruncUnit::Year => NaiveDate::from_ymd_opt(date.year(), 1, 1),
                _ => NaiveDate::from_ymd_opt(date.year(), date.month(), 1),
            }
            .ok_or_else(|| PlenoraError::Schema("date_trunc: data fuori range".into()))?;
            first
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| PlenoraError::Schema("date_trunc: orario non valido".into()))?
                .and_utc()
                .timestamp_millis()
        }
    })
}

/// Valutazione di `date_trunc(unit, value)`: null propagato (tri-state), il
/// valore e' letto nativamente da `eval_temporal`.
pub fn date_trunc_generic(args: &[Expression], batch: &RecordBatch, row: usize) -> Result<Scalar> {
    if args.len() != 2 {
        return Err(PlenoraError::InvalidPlan(
            "date_trunc richiede 2 argomenti".into(),
        ));
    }
    let unit = literal_unit(&args[0])?;
    match eval_temporal(&args[1], batch, row)? {
        Scalar::Null => Ok(Scalar::Null),
        Scalar::Date32(days) => Ok(Scalar::Date32(trunc_date32_days(days, unit)?)),
        Scalar::TimestampMs(ms) => Ok(Scalar::TimestampMs(trunc_timestamp_ms_value(ms, unit)?)),
        _ => Err(PlenoraError::Internal(
            "eval_temporal produce solo valori temporali".into(),
        )),
    }
}

/// Sorgente temporale di `date_trunc`.
///
/// Colonna Date32 o Timestamp(ms) letta nativamente, `date_trunc` annidato,
/// letterale null. Nessun parsing implicito di stringhe; timestamp
/// timezone-aware rifiutati (decisione documentata: la semantica tz del
/// troncamento non e' definibile in modo sicuro, quindi l'output Timestamp
/// e' sempre senza timezone).
pub fn eval_temporal(expression: &Expression, batch: &RecordBatch, row: usize) -> Result<Scalar> {
    match expression {
        Expression::Column { name } => {
            let index = column_index(batch, name)?;
            let array = batch.column(index);
            match array.data_type() {
                DataType::Date32 => {
                    let values = array
                        .as_any()
                        .downcast_ref::<Date32Array>()
                        .ok_or_else(|| PlenoraError::Schema("array Date32 incoerente".into()))?;
                    Ok(if values.is_null(row) {
                        Scalar::Null
                    } else {
                        Scalar::Date32(values.value(row))
                    })
                }
                DataType::Timestamp(TimeUnit::Millisecond, timezone) => {
                    if timezone.is_some() {
                        return Err(PlenoraError::Schema(
                            "date_trunc: timestamp timezone-aware non supportato".into(),
                        ));
                    }
                    let values = array
                        .as_any()
                        .downcast_ref::<TimestampMillisecondArray>()
                        .ok_or_else(|| {
                            PlenoraError::Schema("array Timestamp(ms) incoerente".into())
                        })?;
                    Ok(if values.is_null(row) {
                        Scalar::Null
                    } else {
                        Scalar::TimestampMs(values.value(row))
                    })
                }
                other => Err(PlenoraError::Schema(format!(
                    "date_trunc richiede una colonna Date32 o Timestamp(ms), trovato {other:?}"
                ))),
            }
        }
        Expression::Function {
            name: Function::DateTrunc,
            args,
        } => date_trunc_generic(args, batch, row),
        Expression::Literal {
            value: Value::Null,
        } => Ok(Scalar::Null),
        _ => Err(PlenoraError::InvalidPlan(
            "date_trunc: il valore deve essere una colonna temporale".into(),
        )),
    }
}

/// Valutazione di `in(value, [letterali])`.
///
/// Null propagato; confronti con la stessa semantica dei `BinaryOperator`
/// (tipi incompatibili -> errore, elementi null mai uguali). Lista vuota
/// ammessa: sempre `false`.
pub fn in_generic(args: &[Expression], batch: &RecordBatch, row: usize) -> Result<Scalar> {
    if args.len() != 2 {
        return Err(PlenoraError::InvalidPlan("in richiede 2 argomenti".into()));
    }
    let Expression::Literal {
        value: Value::Array(items),
    } = &args[1]
    else {
        return Err(PlenoraError::InvalidPlan(
            "in richiede una lista di letterali come secondo argomento".into(),
        ));
    };
    let value = evaluate(&args[0], batch, row)?;
    if value == Scalar::Null {
        return Ok(Scalar::Null);
    }
    for item in items {
        if compare(value.clone(), literal(item)?)? == Some(Ordering::Equal) {
            return Ok(Scalar::Boolean(true));
        }
    }
    Ok(Scalar::Boolean(false))
}
