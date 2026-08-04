use std::borrow::Cow;
use std::cmp::Ordering;
use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use num_traits::ToPrimitive;
use serde_json::Value;

use super::interpreter::root_temporal_type;
use super::temporal::{literal_unit, trunc_date32_days, trunc_timestamp_ms_value, TruncUnit};
use super::{BinaryOperator, Expression, ExpressionTransform, Function, OutputType, UnaryOperator};
use crate::{
    column_index, replace_or_append, scalar_as_f64, scalar_as_string, DIVISION_BY_ZERO_MESSAGE,
    NON_FINITE_INPUT_MESSAGE, NON_FINITE_RESULT_MESSAGE,
};
use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int64Array,
    RecordBatch, StringArray, TimestampMillisecondArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, TimeUnit};
use plenora_core::{PlenoraError, Result};

// ---------------------------------------------------------------------------
// Fast path compilato (ottimizzazione kernel `table.expression`, ultimo batch).
//
// L'AST viene compilato UNA VOLTA in un albero di `FastNode`: indici di
// colonna risolti e downcast degli array fatti in compilazione, letterali
// pre-materializzati (`literal` eseguito una sola volta), testo delle colonne
// Utf8 e dei letterali preso in prestito (`Cow`, nessun clone per riga).
// Semantica IDENTICA a `evaluate`:
// - errori lazy come nel generico: un letterale non valido o una colonna
//   assente in un ramo `case` non percorso non falliscono (`FastNode::Error`
//   rilascia l'errore solo quando il nodo viene valutato);
// - numeri non finiti in colonna rifiutati, risultati aritmetici non finiti
//   rifiutati, divisione per zero (`-0.0 == 0.0` incluso);
// - confronti numerici via `total_cmp` (NaN ordinato, -0.0 < 0.0);
// - colonne di tipo non coperto (o array incoerente con lo schema) usano lo
//   stesso codice del generico riga per riga (`FastColumn::Other`).
// ---------------------------------------------------------------------------

/// Letterale pre-materializzato: conversione di `literal` fatta una volta.
#[derive(Clone, Copy)]
enum FastLiteral<'a> {
    Null,
    Number(f64),
    Boolean(bool),
    Text(&'a str),
}

impl<'a> FastLiteral<'a> {
    const fn value(self) -> FastValue<'a> {
        match self {
            Self::Null => FastValue::Null,
            Self::Number(value) => FastValue::Number(value),
            Self::Boolean(value) => FastValue::Boolean(value),
            Self::Text(value) => FastValue::Text(Cow::Borrowed(value)),
        }
    }
}

/// Errore rilasciato solo quando il nodo viene valutato (lazy, come nel
/// generico): letterale non valido o colonna assente.
enum LazyError {
    InvalidPlan(String),
    Schema(String),
}

impl LazyError {
    fn build(&self) -> PlenoraError {
        match self {
            Self::InvalidPlan(message) => PlenoraError::InvalidPlan(message.clone()),
            Self::Schema(message) => PlenoraError::Schema(message.clone()),
        }
    }
}

/// Valore di lavoro: come `Scalar`, ma con testo preso in prestito dalle
/// colonne Utf8 e dai letterali (nessun clone per riga).
#[derive(Clone)]
enum FastValue<'a> {
    Null,
    Number(f64),
    Boolean(bool),
    Text(Cow<'a, str>),
    /// Data nativa (giorni dall'epoca): prodotta solo da `date_trunc`.
    Date32(i32),
    /// Timestamp nativo (ms dall'epoca, UTC naive): solo da `date_trunc`.
    TimestampMs(i64),
}

/// Accessore di colonna pre-risolto (indice + downcast fatti una volta).
enum FastColumn<'a> {
    Boolean(&'a BooleanArray),
    F64(&'a Float64Array),
    I64(&'a Int64Array),
    U64(&'a UInt64Array),
    Date32(&'a Date32Array),
    TimestampMs(&'a TimestampMillisecondArray),
    /// Array decimale e fattore di scala precomputato (`10^scale`).
    Decimal128(&'a Decimal128Array, f64),
    Str(&'a StringArray),
    /// Tipo non coperto o array incoerente con lo schema: stessa logica del
    /// generico riga per riga (errori di conversione inclusi).
    Other(&'a ArrayRef),
}

impl<'a> FastColumn<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        let any = array.as_any();
        match array.data_type() {
            DataType::Boolean => any
                .downcast_ref::<BooleanArray>()
                .map_or(Self::Other(array), Self::Boolean),
            DataType::Float64 => any
                .downcast_ref::<Float64Array>()
                .map_or(Self::Other(array), Self::F64),
            DataType::Int64 => any
                .downcast_ref::<Int64Array>()
                .map_or(Self::Other(array), Self::I64),
            DataType::UInt64 => any
                .downcast_ref::<UInt64Array>()
                .map_or(Self::Other(array), Self::U64),
            DataType::Date32 => any
                .downcast_ref::<Date32Array>()
                .map_or(Self::Other(array), Self::Date32),
            DataType::Timestamp(TimeUnit::Millisecond, _) => any
                .downcast_ref::<TimestampMillisecondArray>()
                .map_or(Self::Other(array), Self::TimestampMs),
            DataType::Decimal128(_, scale) => any
                .downcast_ref::<Decimal128Array>()
                .map_or(Self::Other(array), |values| {
                    Self::Decimal128(values, 10_f64.powi(i32::from(*scale)))
                }),
            DataType::Utf8 => any
                .downcast_ref::<StringArray>()
                .map_or(Self::Other(array), Self::Str),
            _ => Self::Other(array),
        }
    }

    fn get(&self, row: usize) -> Result<FastValue<'a>> {
        match self {
            Self::Boolean(values) => Ok(if values.is_null(row) {
                FastValue::Null
            } else {
                FastValue::Boolean(values.value(row))
            }),
            Self::F64(values) => {
                if values.is_null(row) {
                    Ok(FastValue::Null)
                } else {
                    finite_number(values.value(row))
                }
            }
            Self::I64(values) => {
                if values.is_null(row) {
                    Ok(FastValue::Null)
                } else {
                    finite_number(values.value(row).to_f64().ok_or_else(|| {
                        PlenoraError::Schema("intero non rappresentabile come f64".into())
                    })?)
                }
            }
            Self::U64(values) => {
                if values.is_null(row) {
                    Ok(FastValue::Null)
                } else {
                    finite_number(values.value(row).to_f64().ok_or_else(|| {
                        PlenoraError::Schema("uint64 non rappresentabile come f64".into())
                    })?)
                }
            }
            Self::Date32(values) => {
                if values.is_null(row) {
                    Ok(FastValue::Null)
                } else {
                    finite_number(f64::from(values.value(row)))
                }
            }
            Self::TimestampMs(values) => {
                if values.is_null(row) {
                    Ok(FastValue::Null)
                } else {
                    finite_number(values.value(row).to_f64().ok_or_else(|| {
                        PlenoraError::Schema("timestamp non rappresentabile come f64".into())
                    })?)
                }
            }
            Self::Decimal128(values, factor) => {
                if values.is_null(row) {
                    Ok(FastValue::Null)
                } else {
                    finite_number(
                        values.value(row).to_f64().ok_or_else(|| {
                            PlenoraError::Schema("decimal128 non rappresentabile come f64".into())
                        })? / factor,
                    )
                }
            }
            Self::Str(values) => Ok(if values.is_null(row) {
                FastValue::Null
            } else {
                FastValue::Text(Cow::Borrowed(values.value(row)))
            }),
            Self::Other(array) => other_column(array, row),
        }
    }
}

/// Controllo di finitezza del ramo numerico di `column`.
fn finite_number(value: f64) -> Result<FastValue<'static>> {
    if value.is_finite() {
        Ok(FastValue::Number(value))
    } else {
        Err(PlenoraError::Schema(NON_FINITE_INPUT_MESSAGE.into()))
    }
}

/// Lettura colonna per i tipi non coperti: replica esatta di `column` con
/// indice gia' risolto.
fn other_column(array: &ArrayRef, row: usize) -> Result<FastValue<'static>> {
    if array.is_null(row) {
        return Ok(FastValue::Null);
    }
    if array.data_type() == &DataType::Boolean {
        let values = array
            .as_any()
            .downcast_ref::<BooleanArray>()
            .ok_or_else(|| PlenoraError::Schema("array Boolean incoerente".into()))?;
        return Ok(FastValue::Boolean(values.value(row)));
    }
    if matches!(
        array.data_type(),
        DataType::Int64
            | DataType::UInt64
            | DataType::Float64
            | DataType::Decimal128(_, _)
            | DataType::Date32
            | DataType::Timestamp(_, _)
    ) {
        return scalar_as_f64(array.as_ref(), row)?
            .map_or_else(|| Ok(FastValue::Null), finite_number);
    }
    Ok(scalar_as_string(array.as_ref(), row)?.map_or_else(
        || FastValue::Null,
        |value| FastValue::Text(Cow::Owned(value)),
    ))
}

/// Equivalente di `boolean` su `FastValue`.
fn fast_boolean(value: &FastValue<'_>, context: &str) -> Result<Option<bool>> {
    match value {
        FastValue::Null => Ok(None),
        FastValue::Boolean(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!(
            "{context} richiede un booleano"
        ))),
    }
}

/// Equivalente di `number` su `FastValue`.
fn fast_number(value: &FastValue<'_>, context: &str) -> Result<Option<f64>> {
    match value {
        FastValue::Null => Ok(None),
        FastValue::Number(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!(
            "{context} richiede un numero"
        ))),
    }
}

/// Equivalente di `text` su `FastValue` (senza clone: restituisce `&str`).
fn fast_text<'a>(value: &'a FastValue<'_>, context: &str) -> Result<Option<&'a str>> {
    match value {
        FastValue::Null => Ok(None),
        FastValue::Text(value) => Ok(Some(value.as_ref())),
        _ => Err(PlenoraError::Schema(format!("{context} richiede testo"))),
    }
}

/// Equivalente di `compare` su `FastValue` (senza clone).
fn fast_compare(left: &FastValue<'_>, right: &FastValue<'_>) -> Result<Option<Ordering>> {
    match (left, right) {
        (FastValue::Null, _) | (_, FastValue::Null) => Ok(None),
        (FastValue::Number(left), FastValue::Number(right)) => Ok(Some(left.total_cmp(right))),
        (FastValue::Text(left), FastValue::Text(right)) => Ok(Some(left.cmp(right))),
        (FastValue::Boolean(left), FastValue::Boolean(right)) => Ok(Some(left.cmp(right))),
        (FastValue::Date32(left), FastValue::Date32(right)) => Ok(Some(left.cmp(right))),
        (FastValue::TimestampMs(left), FastValue::TimestampMs(right)) => Ok(Some(left.cmp(right))),
        _ => Err(PlenoraError::Schema(
            "confronto expression fra tipi incompatibili".into(),
        )),
    }
}

/// Equivalente di `arithmetic` su `FastValue`.
fn fast_arithmetic<'a>(
    op: BinaryOperator,
    left: &FastValue<'a>,
    right: &FastValue<'a>,
) -> Result<FastValue<'a>> {
    let (Some(left), Some(right)) = (
        fast_number(left, "operatore")?,
        fast_number(right, "operatore")?,
    ) else {
        return Ok(FastValue::Null);
    };
    let value = match op {
        BinaryOperator::Add => left + right,
        BinaryOperator::Subtract => left - right,
        BinaryOperator::Multiply => left * right,
        BinaryOperator::Divide if right == 0.0 => {
            return Err(PlenoraError::Schema(DIVISION_BY_ZERO_MESSAGE.into()));
        }
        BinaryOperator::Divide => left / right,
        _ => {
            return Err(PlenoraError::InvalidPlan(
                "operatore aritmetico inatteso".into(),
            ));
        }
    };
    if value.is_finite() {
        Ok(FastValue::Number(value))
    } else {
        Err(PlenoraError::Schema(NON_FINITE_RESULT_MESSAGE.into()))
    }
}

/// Equivalente di `logical` su `FastValue`.
fn fast_logical<'a>(
    op: BinaryOperator,
    left: &FastValue<'a>,
    right: &FastValue<'a>,
) -> Result<FastValue<'a>> {
    let left = fast_boolean(left, "operatore logico")?;
    let right = fast_boolean(right, "operatore logico")?;
    Ok(match op {
        BinaryOperator::And => match (left, right) {
            (Some(false), _) | (_, Some(false)) => FastValue::Boolean(false),
            (Some(true), Some(true)) => FastValue::Boolean(true),
            _ => FastValue::Null,
        },
        BinaryOperator::Or => match (left, right) {
            (Some(true), _) | (_, Some(true)) => FastValue::Boolean(true),
            (Some(false), Some(false)) => FastValue::Boolean(false),
            _ => FastValue::Null,
        },
        _ => {
            return Err(PlenoraError::InvalidPlan(
                "operatore logico inatteso".into(),
            ))
        }
    })
}

/// Equivalente di `binary` su `FastValue`.
fn fast_binary<'a>(
    op: BinaryOperator,
    left: &FastValue<'a>,
    right: &FastValue<'a>,
) -> Result<FastValue<'a>> {
    match op {
        BinaryOperator::Add
        | BinaryOperator::Subtract
        | BinaryOperator::Multiply
        | BinaryOperator::Divide => fast_arithmetic(op, left, right),
        BinaryOperator::And | BinaryOperator::Or => fast_logical(op, left, right),
        BinaryOperator::Equal => Ok(fast_compare(left, right)?.map_or(FastValue::Null, |value| {
            FastValue::Boolean(value == Ordering::Equal)
        })),
        BinaryOperator::NotEqual => Ok(fast_compare(left, right)?
            .map_or(FastValue::Null, |value| {
                FastValue::Boolean(value != Ordering::Equal)
            })),
        BinaryOperator::Greater => Ok(fast_compare(left, right)?
            .map_or(FastValue::Null, |value| {
                FastValue::Boolean(value == Ordering::Greater)
            })),
        BinaryOperator::GreaterEqual => Ok(fast_compare(left, right)?
            .map_or(FastValue::Null, |value| {
                FastValue::Boolean(value != Ordering::Less)
            })),
        BinaryOperator::Less => Ok(fast_compare(left, right)?.map_or(FastValue::Null, |value| {
            FastValue::Boolean(value == Ordering::Less)
        })),
        BinaryOperator::LessEqual => Ok(fast_compare(left, right)?
            .map_or(FastValue::Null, |value| {
                FastValue::Boolean(value != Ordering::Greater)
            })),
    }
}

/// Controllo argomenti identico a `exact_args`.
fn exact_args_fast(args: &[FastValue<'_>], count: usize, name: &str) -> Result<()> {
    if args.len() == count {
        Ok(())
    } else {
        Err(PlenoraError::InvalidPlan(format!(
            "{name} richiede {count} argomenti"
        )))
    }
}

/// Equivalente di `function` su `FastValue`.
#[allow(clippy::too_many_lines)]
fn fast_function(name: Function, args: Vec<FastValue<'_>>) -> Result<FastValue<'_>> {
    match name {
        Function::Coalesce => {
            if args.is_empty() {
                return Err(PlenoraError::InvalidPlan(
                    "coalesce richiede argomenti".into(),
                ));
            }
            Ok(args
                .into_iter()
                .find(|value| !matches!(value, FastValue::Null))
                .unwrap_or(FastValue::Null))
        }
        Function::NullIf => {
            exact_args_fast(&args, 2, "null_if")?;
            if !matches!(args[0], FastValue::Null)
                && fast_compare(&args[0], &args[1])? == Some(Ordering::Equal)
            {
                Ok(FastValue::Null)
            } else {
                Ok(args.into_iter().next().unwrap_or(FastValue::Null))
            }
        }
        Function::Lower | Function::Upper | Function::Trim | Function::Length | Function::Year => {
            exact_args_fast(&args, 1, "funzione unaria")?;
            let Some(value) = fast_text(&args[0], "funzione")? else {
                return Ok(FastValue::Null);
            };
            Ok(match name {
                Function::Lower => FastValue::Text(Cow::Owned(value.to_lowercase())),
                Function::Upper => FastValue::Text(Cow::Owned(value.to_uppercase())),
                Function::Trim => FastValue::Text(Cow::Owned(value.trim().to_owned())),
                Function::Length => FastValue::Number(
                    u32::try_from(value.chars().count())
                        .map(f64::from)
                        .map_err(|_| PlenoraError::InvalidPlan("testo troppo lungo".into()))?,
                ),
                Function::Year => {
                    let date =
                        NaiveDate::parse_from_str(value.get(..10).unwrap_or(value), "%Y-%m-%d")
                            .map_err(|_| PlenoraError::Schema("year: data non valida".into()))?;
                    FastValue::Number(f64::from(date.year()))
                }
                _ => {
                    return Err(PlenoraError::Internal(
                        "il ramo unario ammette solo lower/upper/trim/length/year".into(),
                    ));
                }
            })
        }
        Function::Concat => {
            if args.is_empty() {
                return Err(PlenoraError::InvalidPlan(
                    "concat richiede argomenti".into(),
                ));
            }
            let mut output = String::new();
            for value in &args {
                let Some(value) = fast_text(value, "concat")? else {
                    return Ok(FastValue::Null);
                };
                output.push_str(value);
            }
            Ok(FastValue::Text(Cow::Owned(output)))
        }
        Function::Contains | Function::StartsWith | Function::EndsWith => {
            exact_args_fast(&args, 2, "funzione testo")?;
            let (Some(value), Some(pattern)) = (
                fast_text(&args[0], "funzione testo")?,
                fast_text(&args[1], "funzione testo")?,
            ) else {
                return Ok(FastValue::Null);
            };
            Ok(FastValue::Boolean(match name {
                Function::Contains => value.contains(pattern),
                Function::StartsWith => value.starts_with(pattern),
                Function::EndsWith => value.ends_with(pattern),
                _ => {
                    return Err(PlenoraError::Internal(
                        "il ramo testo ammette solo contains/starts_with/ends_with".into(),
                    ));
                }
            }))
        }
        Function::Abs | Function::Round => {
            exact_args_fast(&args, 1, "funzione numerica")?;
            let Some(value) = fast_number(&args[0], "funzione numerica")? else {
                return Ok(FastValue::Null);
            };
            Ok(FastValue::Number(match name {
                Function::Abs => value.abs(),
                Function::Round => value.round(),
                _ => {
                    return Err(PlenoraError::Internal(
                        "il ramo numerico ammette solo abs/round".into(),
                    ));
                }
            }))
        }
        Function::Floor | Function::Ceil => {
            exact_args_fast(&args, 1, "funzione numerica")?;
            let Some(value) = fast_number(&args[0], "funzione numerica")? else {
                return Ok(FastValue::Null);
            };
            Ok(FastValue::Number(match name {
                Function::Floor => value.floor(),
                Function::Ceil => value.ceil(),
                _ => {
                    return Err(PlenoraError::Internal(
                        "il ramo numerico ammette solo floor/ceil".into(),
                    ));
                }
            }))
        }
        Function::Power => {
            exact_args_fast(&args, 2, "power")?;
            let (Some(base), Some(exponent)) = (
                fast_number(&args[0], "power")?,
                fast_number(&args[1], "power")?,
            ) else {
                return Ok(FastValue::Null);
            };
            let value = base.powf(exponent);
            if value.is_finite() {
                Ok(FastValue::Number(value))
            } else {
                Err(PlenoraError::Schema(NON_FINITE_RESULT_MESSAGE.into()))
            }
        }
        Function::Substring => {
            if !(2..=3).contains(&args.len()) {
                return Err(PlenoraError::InvalidPlan(
                    "substring richiede 2 o 3 argomenti".into(),
                ));
            }
            let Some(value) = fast_text(&args[0], "substring")? else {
                return Ok(FastValue::Null);
            };
            let Some(start) = fast_substring_index(&args[1], "substring: start")? else {
                return Ok(FastValue::Null);
            };
            let len = match args.get(2) {
                Some(arg) => match fast_substring_index(arg, "substring: len")? {
                    Some(len) => Some(len),
                    // len null -> Null (non equivale a "fino a fine stringa").
                    None => return Ok(FastValue::Null),
                },
                None => None,
            };
            let mut chars = value.chars().skip(start);
            Ok(FastValue::Text(Cow::Owned(match len {
                Some(len) => chars.by_ref().take(len).collect(),
                None => chars.collect(),
            })))
        }
        Function::Between => {
            exact_args_fast(&args, 3, "between")?;
            // Inclusivo su entrambi gli estremi; null in qualsiasi posizione
            // -> Null (stessa tri-state dei confronti binari).
            if args.iter().any(|arg| matches!(arg, FastValue::Null)) {
                return Ok(FastValue::Null);
            }
            let low = fast_compare(&args[0], &args[1])?;
            let high = fast_compare(&args[0], &args[2])?;
            Ok(match (low, high) {
                (Some(low), Some(high)) => {
                    FastValue::Boolean(low != Ordering::Less && high != Ordering::Greater)
                }
                _ => FastValue::Null,
            })
        }
        Function::Greatest | Function::Least => {
            let label = if matches!(name, Function::Greatest) {
                "greatest"
            } else {
                "least"
            };
            if args.is_empty() {
                return Err(PlenoraError::InvalidPlan(format!(
                    "{label} richiede argomenti"
                )));
            }
            let mut best = args[0].clone();
            for value in &args[1..] {
                let Some(ordering) = fast_compare(&best, value)? else {
                    // Null propagato come nei confronti binari.
                    return Ok(FastValue::Null);
                };
                let replace = match name {
                    Function::Greatest => ordering == Ordering::Less,
                    _ => ordering == Ordering::Greater,
                };
                if replace {
                    best = value.clone();
                }
            }
            Ok(best)
        }
        Function::RegexReplace | Function::DateTrunc | Function::In => Err(PlenoraError::Internal(
            "regex_replace/date_trunc/in hanno nodi dedicati in evaluate_fast".into(),
        )),
    }
}

/// Equivalente di `substring_index` su `FastValue`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn fast_substring_index(value: &FastValue<'_>, context: &str) -> Result<Option<usize>> {
    let Some(value) = fast_number(value, context)? else {
        return Ok(None);
    };
    if value < 0.0 {
        return Err(PlenoraError::InvalidPlan(format!("{context} negativo")));
    }
    Ok(Some(value as usize))
}

/// Nodo compilato: colonne risolte, letterali pre-materializzati, errori lazy.
enum FastNode<'a> {
    Literal(FastLiteral<'a>),
    Column(FastColumn<'a>),
    Error(LazyError),
    Unary {
        op: UnaryOperator,
        value: Box<Self>,
    },
    Binary {
        op: BinaryOperator,
        left: Box<Self>,
        right: Box<Self>,
    },
    Function {
        name: Function,
        args: Vec<Self>,
    },
    Case {
        branches: Vec<(Self, Self)>,
        else_value: Box<Self>,
    },
    /// `date_trunc`: unita' pre-validata, sorgente temporale letta nativamente.
    DateTrunc {
        unit: TruncUnit,
        source: TemporalSource<'a>,
    },
    /// `in`: lista di letterali pre-compilati; gli errori degli elementi
    /// restano lazy come nel generico (rilasciati solo se raggiunti).
    In {
        value: Box<Self>,
        list: Vec<Self>,
    },
    /// `regex_replace`: il pattern letterale e' compilato UNA VOLTA in
    /// compilazione.
    ///
    /// L'errore di una regex non valida e' rilasciato in valutazione, nella
    /// stessa posizione del generico (dopo i null check).
    RegexReplace {
        value: Box<Self>,
        pattern: RegexSource<'a>,
        replacement: Box<Self>,
    },
}

/// Sorgente temporale pre-risolta di `date_trunc` (downcast fatto una volta).
enum TemporalSource<'a> {
    Date32(&'a Date32Array),
    TimestampMs(&'a TimestampMillisecondArray),
    /// `date_trunc` annidato.
    Nested(Box<FastNode<'a>>),
    NullLiteral,
    /// Colonna mancante, tipo non temporale o timezone-aware: errore lazy
    /// (rilasciato solo se il nodo viene valutato, come nel generico).
    Error(LazyError),
}

/// Pattern di `regex_replace`: letterale pre-compilato o espressione
/// dinamica (compilata per riga, come nel generico).
enum RegexSource<'a> {
    /// `Result` della compilazione unica + testo del pattern.
    Compiled(std::result::Result<regex::Regex, String>, &'a str),
    Dynamic(Box<FastNode<'a>>),
}

impl LazyError {
    /// Converte un errore di validazione (solo Contract/Schema attesi) nella
    /// forma lazy del fast path.
    fn from_validation(error: &PlenoraError) -> Self {
        match error {
            PlenoraError::InvalidPlan(message) => Self::InvalidPlan(message.clone()),
            PlenoraError::Schema(message) => Self::Schema(message.clone()),
            // Non atteso: i percorsi di validazione emettono solo Contract/Schema.
            other => Self::Schema(other.to_string()),
        }
    }
}

fn compile_literal(value: &Value) -> FastNode<'_> {
    match value {
        Value::Null => FastNode::Literal(FastLiteral::Null),
        Value::Bool(value) => FastNode::Literal(FastLiteral::Boolean(*value)),
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map_or_else(
                || FastNode::Error(LazyError::InvalidPlan("literal numerico non finito".into())),
                |value| FastNode::Literal(FastLiteral::Number(value)),
            ),
        Value::String(value) => FastNode::Literal(FastLiteral::Text(value.as_str())),
        Value::Array(_) | Value::Object(_) => FastNode::Error(LazyError::InvalidPlan(
            "literal expression deve essere scalare".into(),
        )),
    }
}

fn compile_expression<'a>(expression: &'a Expression, batch: &'a RecordBatch) -> FastNode<'a> {
    match expression {
        Expression::Column { name } => column_index(batch, name).map_or_else(
            |_| FastNode::Error(LazyError::Schema(format!("colonna non trovata: {name}"))),
            |index| FastNode::Column(FastColumn::new(batch.column(index))),
        ),
        Expression::Literal { value } => compile_literal(value),
        Expression::Unary { op, value } => FastNode::Unary {
            op: *op,
            value: Box::new(compile_expression(value, batch)),
        },
        Expression::Binary { op, left, right } => FastNode::Binary {
            op: *op,
            left: Box::new(compile_expression(left, batch)),
            right: Box::new(compile_expression(right, batch)),
        },
        Expression::Function { name, args } => match name {
            Function::DateTrunc => compile_date_trunc(args, batch),
            Function::In => compile_in(args, batch),
            Function::RegexReplace => compile_regex_replace(args, batch),
            _ => FastNode::Function {
                name: *name,
                args: args
                    .iter()
                    .map(|arg| compile_expression(arg, batch))
                    .collect(),
            },
        },
        Expression::Case {
            branches,
            else_value,
        } => FastNode::Case {
            branches: branches
                .iter()
                .map(|branch| {
                    (
                        compile_expression(&branch.when, batch),
                        compile_expression(&branch.then, batch),
                    )
                })
                .collect(),
            else_value: Box::new(compile_expression(else_value, batch)),
        },
    }
}

fn compile_date_trunc<'a>(args: &'a [Expression], batch: &'a RecordBatch) -> FastNode<'a> {
    if args.len() != 2 {
        return FastNode::Error(LazyError::InvalidPlan(
            "date_trunc richiede 2 argomenti".into(),
        ));
    }
    let unit = match literal_unit(&args[0]) {
        Ok(unit) => unit,
        Err(error) => return FastNode::Error(LazyError::from_validation(&error)),
    };
    FastNode::DateTrunc {
        unit,
        source: compile_temporal(&args[1], batch),
    }
}

/// Equivalente statico di `eval_temporal`: risolve la colonna temporale una
/// volta sola mantenendo gli errori lazy (stessa posizione del generico).
fn compile_temporal<'a>(expression: &'a Expression, batch: &'a RecordBatch) -> TemporalSource<'a> {
    match expression {
        Expression::Column { name } => {
            let Ok(index) = column_index(batch, name) else {
                return TemporalSource::Error(LazyError::Schema(format!(
                    "colonna non trovata: {name}"
                )));
            };
            let array = batch.column(index);
            match array.data_type() {
                DataType::Date32 => array.as_any().downcast_ref::<Date32Array>().map_or_else(
                    || TemporalSource::Error(LazyError::Schema("array Date32 incoerente".into())),
                    TemporalSource::Date32,
                ),
                DataType::Timestamp(TimeUnit::Millisecond, timezone) => {
                    if timezone.is_some() {
                        return TemporalSource::Error(LazyError::Schema(
                            "date_trunc: timestamp timezone-aware non supportato".into(),
                        ));
                    }
                    array
                        .as_any()
                        .downcast_ref::<TimestampMillisecondArray>()
                        .map_or_else(
                            || {
                                TemporalSource::Error(LazyError::Schema(
                                    "array Timestamp(ms) incoerente".into(),
                                ))
                            },
                            TemporalSource::TimestampMs,
                        )
                }
                other => TemporalSource::Error(LazyError::Schema(format!(
                    "date_trunc richiede una colonna Date32 o Timestamp(ms), trovato {other:?}"
                ))),
            }
        }
        Expression::Function {
            name: Function::DateTrunc,
            args,
        } => TemporalSource::Nested(Box::new(compile_date_trunc(args, batch))),
        Expression::Literal { value: Value::Null } => TemporalSource::NullLiteral,
        _ => TemporalSource::Error(LazyError::InvalidPlan(
            "date_trunc: il valore deve essere una colonna temporale".into(),
        )),
    }
}

fn compile_in<'a>(args: &'a [Expression], batch: &'a RecordBatch) -> FastNode<'a> {
    if args.len() != 2 {
        return FastNode::Error(LazyError::InvalidPlan("in richiede 2 argomenti".into()));
    }
    let Expression::Literal {
        value: Value::Array(items),
    } = &args[1]
    else {
        return FastNode::Error(LazyError::InvalidPlan(
            "in richiede una lista di letterali come secondo argomento".into(),
        ));
    };
    FastNode::In {
        value: Box::new(compile_expression(&args[0], batch)),
        list: items.iter().map(compile_literal).collect(),
    }
}

fn compile_regex_replace<'a>(args: &'a [Expression], batch: &'a RecordBatch) -> FastNode<'a> {
    if args.len() != 3 {
        return FastNode::Error(LazyError::InvalidPlan(
            "regex_replace richiede 3 argomenti".into(),
        ));
    }
    // Pattern letterale: compilato una volta; il testo resta disponibile per
    // la verifica dei null in valutazione (ordine identico al generico).
    let pattern = match &args[1] {
        Expression::Literal {
            value: Value::String(pattern),
        } => RegexSource::Compiled(
            regex::Regex::new(pattern)
                .map_err(|error| format!("regex_replace: regex non valida: {error}")),
            pattern.as_str(),
        ),
        other => RegexSource::Dynamic(Box::new(compile_expression(other, batch))),
    };
    FastNode::RegexReplace {
        value: Box::new(compile_expression(&args[0], batch)),
        pattern,
        replacement: Box::new(compile_expression(&args[2], batch)),
    }
}

// Dispatcher esaustivo su `FastNode`: la lunghezza e' data dalla sequenza
// lineare dei casi del contratto (uno per variante), non da complessita'
// logica; spezzarla peggiorerebbe solo la leggibilita'.
#[allow(clippy::too_many_lines)]
fn evaluate_fast<'e, 'a: 'e>(node: &'e FastNode<'a>, row: usize) -> Result<FastValue<'e>> {
    match node {
        FastNode::Literal(literal) => Ok(literal.value()),
        FastNode::Column(column) => column.get(row),
        FastNode::Error(error) => Err(error.build()),
        FastNode::Unary { op, value } => {
            let value = evaluate_fast(value, row)?;
            Ok(match op {
                UnaryOperator::IsNull => FastValue::Boolean(matches!(value, FastValue::Null)),
                UnaryOperator::IsNotNull => FastValue::Boolean(!matches!(value, FastValue::Null)),
                UnaryOperator::Not => fast_boolean(&value, "not")?
                    .map_or(FastValue::Null, |value| FastValue::Boolean(!value)),
                UnaryOperator::Negate => fast_number(&value, "negate")?
                    .map_or(FastValue::Null, |value| FastValue::Number(-value)),
            })
        }
        FastNode::Binary { op, left, right } => {
            let left = evaluate_fast(left, row)?;
            let right = evaluate_fast(right, row)?;
            fast_binary(*op, &left, &right)
        }
        FastNode::Function { name, args } => fast_function(
            *name,
            args.iter()
                .map(|arg| evaluate_fast(arg, row))
                .collect::<Result<Vec<_>>>()?,
        ),
        FastNode::Case {
            branches,
            else_value,
        } => {
            for (when, then) in branches {
                if fast_boolean(&evaluate_fast(when, row)?, "case when")? == Some(true) {
                    return evaluate_fast(then, row);
                }
            }
            evaluate_fast(else_value, row)
        }
        FastNode::DateTrunc { unit, source } => {
            let value = match source {
                TemporalSource::Date32(values) => {
                    if values.is_null(row) {
                        FastValue::Null
                    } else {
                        FastValue::Date32(values.value(row))
                    }
                }
                TemporalSource::TimestampMs(values) => {
                    if values.is_null(row) {
                        FastValue::Null
                    } else {
                        FastValue::TimestampMs(values.value(row))
                    }
                }
                TemporalSource::Nested(node) => evaluate_fast(node, row)?,
                TemporalSource::NullLiteral => FastValue::Null,
                TemporalSource::Error(error) => return Err(error.build()),
            };
            match value {
                FastValue::Null => Ok(FastValue::Null),
                FastValue::Date32(days) => Ok(FastValue::Date32(trunc_date32_days(days, *unit)?)),
                FastValue::TimestampMs(ms) => {
                    Ok(FastValue::TimestampMs(trunc_timestamp_ms_value(ms, *unit)?))
                }
                _ => Err(PlenoraError::Internal(
                    "la sorgente temporale produce solo valori temporali".into(),
                )),
            }
        }
        FastNode::In { value, list } => {
            let value = evaluate_fast(value, row)?;
            if matches!(value, FastValue::Null) {
                return Ok(FastValue::Null);
            }
            for item in list {
                if fast_compare(&value, &evaluate_fast(item, row)?)? == Some(Ordering::Equal) {
                    return Ok(FastValue::Boolean(true));
                }
            }
            Ok(FastValue::Boolean(false))
        }
        FastNode::RegexReplace {
            value,
            pattern,
            replacement,
        } => {
            // Stesso ordine del generico: valutazione argomenti, estrazione
            // testi (errori di tipo anche su righe null), poi null check.
            let value = evaluate_fast(value, row)?;
            let dynamic = match pattern {
                RegexSource::Compiled(..) => None,
                RegexSource::Dynamic(node) => Some(evaluate_fast(node, row)?),
            };
            let replacement = evaluate_fast(replacement, row)?;
            let value = fast_text(&value, "regex_replace")?;
            let pattern_text = match (pattern, &dynamic) {
                (RegexSource::Compiled(_, text), None) => Some(*text),
                (RegexSource::Dynamic(_), Some(value)) => fast_text(value, "regex_replace")?,
                _ => {
                    return Err(PlenoraError::Internal(
                        "sorgente regex e valore dinamico incoerenti".into(),
                    ));
                }
            };
            let replacement = fast_text(&replacement, "regex_replace")?;
            let (Some(value), Some(pattern_text), Some(replacement)) =
                (value, pattern_text, replacement)
            else {
                return Ok(FastValue::Null);
            };
            let owned;
            let regex = match pattern {
                RegexSource::Compiled(compiled, _) => compiled
                    .as_ref()
                    .map_err(|message| PlenoraError::InvalidPlan(message.clone()))?,
                RegexSource::Dynamic(_) => {
                    owned = regex::Regex::new(pattern_text).map_err(|error| {
                        PlenoraError::InvalidPlan(format!(
                            "regex_replace: regex non valida: {error}"
                        ))
                    })?;
                    &owned
                }
            };
            Ok(FastValue::Text(Cow::Owned(
                regex.replace_all(value, replacement).into_owned(),
            )))
        }
    }
}

/// Equivalente di `resolved_output_type` su `FastValue`.
fn resolved_output_type_fast(
    values: &[FastValue<'_>],
    configured: OutputType,
) -> Result<OutputType> {
    if !matches!(configured, OutputType::Auto) {
        return Ok(configured);
    }
    let mut resolved = None;
    for value in values {
        let current = match value {
            FastValue::Null => continue,
            FastValue::Number(_) => OutputType::Number,
            FastValue::Boolean(_) => OutputType::Boolean,
            FastValue::Text(_) => OutputType::Text,
            FastValue::Date32(_) => OutputType::Date32,
            FastValue::TimestampMs(_) => OutputType::TimestampMs,
        };
        if resolved.is_some_and(|previous| {
            std::mem::discriminant(&previous) != std::mem::discriminant(&current)
        }) {
            return Err(PlenoraError::Schema(
                "expression produce tipi eterogenei; dichiarare output_type".into(),
            ));
        }
        resolved = Some(current);
    }
    Ok(resolved.unwrap_or(OutputType::Text))
}

/// Equivalente di `scalar_date32` su `FastValue`.
fn fast_scalar_date32(value: &FastValue<'_>, context: &str) -> Result<Option<i32>> {
    match value {
        FastValue::Null => Ok(None),
        FastValue::Date32(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!("{context} richiede una data"))),
    }
}

/// Equivalente di `scalar_timestamp_ms` su `FastValue`.
fn fast_scalar_timestamp_ms(value: &FastValue<'_>, context: &str) -> Result<Option<i64>> {
    match value {
        FastValue::Null => Ok(None),
        FastValue::TimestampMs(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!(
            "{context} richiede un timestamp"
        ))),
    }
}

pub struct FastProgram<'a> {
    root: FastNode<'a>,
}

impl<'a> FastProgram<'a> {
    pub fn compile(expression: &'a Expression, batch: &'a RecordBatch) -> Self {
        Self {
            root: compile_expression(expression, batch),
        }
    }

    pub fn run(&self, batch: &RecordBatch, config: &ExpressionTransform) -> Result<RecordBatch> {
        let mut values = Vec::with_capacity(batch.num_rows());
        let mut rejections = Vec::new();
        for row in 0..batch.num_rows() {
            match evaluate_fast(&self.root, row) {
                Ok(value) => values.push(value),
                Err(error) => {
                    let Some(cause) = crate::row_eval_failure_cause(&error) else {
                        return Err(error);
                    };
                    rejections.push(crate::RowRejection {
                        row,
                        cause,
                        column: None,
                    });
                    // Placeholder mai pubblicato: `reject_rows` chiude prima
                    // dell'uso di `values`.
                    values.push(FastValue::Null);
                }
            }
        }
        crate::reject_rows(
            &rejections,
            "valori expression rifiutati; consultare row_diagnostics",
        )?;
        let mut resolved = resolved_output_type_fast(&values, config.output_type)?;
        // Auto con tutti i valori null: una radice date_trunc risolve il tipo
        // temporale dallo schema di input, mai Utf8 (come il generico).
        if matches!(config.output_type, OutputType::Auto)
            && values.iter().all(|value| matches!(value, FastValue::Null))
        {
            if let Some(temporal) = root_temporal_type(&config.expression, batch)? {
                resolved = temporal;
            }
        }
        match resolved {
            OutputType::Auto => Err(PlenoraError::Internal(
                "output_type Auto non risolto".into(),
            )),
            OutputType::Number => replace_or_append(
                batch,
                &config.output_column,
                DataType::Float64,
                true,
                Arc::new(Float64Array::from(
                    values
                        .iter()
                        .map(|value| fast_number(value, "output_type=number"))
                        .collect::<Result<Vec<_>>>()?,
                )),
            ),
            OutputType::Boolean => replace_or_append(
                batch,
                &config.output_column,
                DataType::Boolean,
                true,
                Arc::new(BooleanArray::from(
                    values
                        .iter()
                        .map(|value| fast_boolean(value, "output_type=boolean"))
                        .collect::<Result<Vec<_>>>()?,
                )),
            ),
            OutputType::Text => replace_or_append(
                batch,
                &config.output_column,
                DataType::Utf8,
                true,
                Arc::new(StringArray::from(
                    values
                        .iter()
                        .map(|value| fast_text(value, "output_type=text"))
                        .collect::<Result<Vec<_>>>()?,
                )),
            ),
            OutputType::Date32 => replace_or_append(
                batch,
                &config.output_column,
                DataType::Date32,
                true,
                Arc::new(Date32Array::from(
                    values
                        .iter()
                        .map(|value| fast_scalar_date32(value, "output_type=date32"))
                        .collect::<Result<Vec<_>>>()?,
                )),
            ),
            // Timestamp timezone-aware rifiutati in ingresso: l'output e'
            // sempre Timestamp(ms) senza timezone (decisione documentata).
            OutputType::TimestampMs => replace_or_append(
                batch,
                &config.output_column,
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
                Arc::new(TimestampMillisecondArray::from(
                    values
                        .iter()
                        .map(|value| fast_scalar_timestamp_ms(value, "output_type=timestamp_ms"))
                        .collect::<Result<Vec<_>>>()?,
                )),
            ),
        }
    }
}
