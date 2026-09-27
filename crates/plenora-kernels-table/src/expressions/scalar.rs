use std::cmp::Ordering;

use plenora_core::arrow::array::{Array, BooleanArray, RecordBatch};
use plenora_core::arrow::schema::DataType;
use serde_json::Value;

use super::BinaryOperator;
use crate::{
    column_index, compare_bounds, scalar_as_f64_rounded, scalar_as_string, NumericBound,
    DIVISION_BY_ZERO_MESSAGE, NON_FINITE_INPUT_MESSAGE, NON_FINITE_RESULT_MESSAGE,
};
use plenora_core::{PlenoraError, Result};

/// Un numero dell'espressione: il double su cui si calcola e il valore esatto
/// su cui si confronta.
///
/// `table.expression` produce `Float64` per contratto, quindi l'aritmetica e
/// l'uscita restano sul double
/// (errori-e-limiti.md#arrotondamento-nelle-operazioni-a-risultato-float64).
/// Un confronto invece decide, e decide sul valore esatto: un `Int64` oltre
/// 2^53 o un `Decimal128` frazionario non collassano sul double vicino. Chi
/// nasce da una colonna o da un letterale porta il valore d'origine; chi nasce
/// da un calcolo porta il proprio double, che e' il suo valore.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Numero {
    /// Il double del calcolo e dell'uscita.
    pub valore: f64,
    /// Il valore esatto del confronto.
    pub esatto: NumericBound,
}

impl Numero {
    /// Un numero che e' il proprio double: risultato di un calcolo.
    #[must_use]
    pub const fn double(valore: f64) -> Self {
        Self {
            valore,
            esatto: NumericBound::F64(valore),
        }
    }

    /// Il numero opposto, esatto come l'originale.
    ///
    /// # Errors
    ///
    /// `Schema` per un `Decimal128` il cui opposto non sta in `i128`: e'
    /// fuori da ogni precisione valida, e Arrow non verifica i valori.
    pub fn opposto(self) -> Result<Self> {
        Ok(Self {
            valore: -self.valore,
            esatto: match self.esatto {
                NumericBound::I64(value) => value.checked_neg().map_or_else(
                    || NumericBound::Decimal {
                        unscaled: -i128::from(value),
                        scale: 0,
                    },
                    NumericBound::I64,
                ),
                NumericBound::U64(value) => NumericBound::Decimal {
                    unscaled: -i128::from(value),
                    scale: 0,
                },
                NumericBound::Decimal { unscaled, scale } => NumericBound::Decimal {
                    unscaled: unscaled.checked_neg().ok_or_else(decimale_fuori_dominio)?,
                    scale,
                },
                NumericBound::F64(value) => NumericBound::F64(-value),
            },
        })
    }

    /// Il valore assoluto, esatto come l'originale.
    ///
    /// # Errors
    ///
    /// Come [`Self::opposto`].
    pub fn assoluto(self) -> Result<Self> {
        Ok(Self {
            valore: self.valore.abs(),
            esatto: match self.esatto {
                NumericBound::I64(value) => NumericBound::U64(value.unsigned_abs()),
                NumericBound::Decimal { unscaled, scale } => NumericBound::Decimal {
                    unscaled: unscaled.checked_abs().ok_or_else(decimale_fuori_dominio)?,
                    scale,
                },
                NumericBound::F64(value) => NumericBound::F64(value.abs()),
                esatto @ NumericBound::U64(_) => esatto,
            },
        })
    }
}

fn decimale_fuori_dominio() -> PlenoraError {
    PlenoraError::Schema("decimal128 fuori dal dominio della precisione".into())
}

/// Confronto esatto di due numeri (`compare_bounds`).
///
/// # Errors
///
/// `Internal` se il confronto non e' definito: i numeri delle espressioni
/// sono finiti per costruzione, quindi un NaN qui e' un difetto nostro.
pub fn confronta_numeri(left: Numero, right: Numero) -> Result<Ordering> {
    compare_bounds(left.esatto, right.esatto).ok_or_else(|| {
        PlenoraError::Internal("confronto expression fra numeri non definito".into())
    })
}

/// Il numero di una cella numerica: il double di `scalar_as_f64_rounded`
/// (errori inclusi) e il valore esatto del tipo nativo.
///
/// # Errors
///
/// Come `scalar_as_f64_rounded`; `Schema` per un double non finito;
/// `Internal` per un tipo che quella conversione accetta e qui non ha un
/// valore esatto.
pub fn numero_della_cella(array: &dyn Array, row: usize) -> Result<Option<Numero>> {
    use plenora_core::arrow::array::{
        Date32Array, Decimal128Array, Float64Array, Int64Array, TimestampMillisecondArray,
        UInt64Array,
    };
    let Some(valore) = scalar_as_f64_rounded(array, row)? else {
        return Ok(None);
    };
    if !valore.is_finite() {
        return Err(PlenoraError::Schema(NON_FINITE_INPUT_MESSAGE.into()));
    }
    let any = array.as_any();
    let esatto = if let Some(values) = any.downcast_ref::<Int64Array>() {
        NumericBound::I64(values.value(row))
    } else if let Some(values) = any.downcast_ref::<UInt64Array>() {
        NumericBound::U64(values.value(row))
    } else if let Some(values) = any.downcast_ref::<TimestampMillisecondArray>() {
        NumericBound::I64(values.value(row))
    } else if let Some(values) = any.downcast_ref::<Date32Array>() {
        NumericBound::I64(i64::from(values.value(row)))
    } else if let Some(values) = any.downcast_ref::<Float64Array>() {
        NumericBound::F64(values.value(row))
    } else if let Some(values) = any.downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        NumericBound::Decimal {
            unscaled: values.value(row),
            scale: *scale,
        }
    } else {
        return Err(PlenoraError::Internal(
            "tipo numerico expression senza valore esatto".into(),
        ));
    };
    Ok(Some(Numero { valore, esatto }))
}

/// Il numero di un letterale JSON: il double di `as_f64` e il valore esatto
/// letto come `table.filter` legge il proprio (`NumericBound::parse` del
/// testo), quindi `0.1` e' il decimale 0,1 e non il double vicino.
///
/// # Errors
///
/// `InvalidPlan` se il letterale non e' un numero finito.
pub fn numero_del_letterale(number: &serde_json::Number) -> Result<Numero> {
    let valore = number
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| PlenoraError::InvalidPlan("literal numerico non finito".into()))?;
    let esatto = match (number.as_i64(), number.as_u64()) {
        (Some(value), _) => NumericBound::I64(value),
        (None, Some(value)) => NumericBound::U64(value),
        (None, None) => {
            NumericBound::parse(&number.to_string()).unwrap_or(NumericBound::F64(valore))
        }
    };
    Ok(Numero { valore, esatto })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Null,
    Number(Numero),
    Boolean(bool),
    Text(String),
    /// Data nativa (giorni dall'epoca): prodotta solo da `date_trunc`.
    Date32(i32),
    /// Timestamp nativo (ms dall'epoca, UTC naive): solo da `date_trunc`.
    TimestampMs(i64),
}

pub fn literal(value: &Value) -> Result<Scalar> {
    match value {
        Value::Null => Ok(Scalar::Null),
        Value::Bool(value) => Ok(Scalar::Boolean(*value)),
        Value::Number(value) => numero_del_letterale(value).map(Scalar::Number),
        Value::String(value) => Ok(Scalar::Text(value.clone())),
        Value::Array(_) | Value::Object(_) => Err(PlenoraError::InvalidPlan(
            "literal expression deve essere scalare".into(),
        )),
    }
}

pub fn column(batch: &RecordBatch, name: &str, row: usize) -> Result<Scalar> {
    let index = column_index(batch, name)?;
    let value = batch.column(index);
    if value.is_null(row) {
        return Ok(Scalar::Null);
    }
    if value.data_type() == &DataType::Boolean {
        let values = value
            .as_any()
            .downcast_ref::<BooleanArray>()
            .ok_or_else(|| PlenoraError::Schema("array Boolean incoerente".into()))?;
        return Ok(Scalar::Boolean(values.value(row)));
    }
    if matches!(
        value.data_type(),
        DataType::Int64
            | DataType::UInt64
            | DataType::Float64
            | DataType::Decimal128(_, _)
            | DataType::Date32
            | DataType::Timestamp(_, _)
    ) {
        return Ok(numero_della_cella(value.as_ref(), row)?.map_or(Scalar::Null, Scalar::Number));
    }
    scalar_as_string(value.as_ref(), row)?.map_or(Ok(Scalar::Null), |value| Ok(Scalar::Text(value)))
}

pub fn boolean(value: &Scalar, context: &str) -> Result<Option<bool>> {
    match value {
        Scalar::Null => Ok(None),
        Scalar::Boolean(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!(
            "{context} richiede un booleano"
        ))),
    }
}

pub fn number(value: &Scalar, context: &str) -> Result<Option<f64>> {
    Ok(numero(value, context)?.map(|numero| numero.valore))
}

/// Come [`number`], con il valore esatto.
pub fn numero(value: &Scalar, context: &str) -> Result<Option<Numero>> {
    match value {
        Scalar::Null => Ok(None),
        Scalar::Number(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!(
            "{context} richiede un numero"
        ))),
    }
}

pub fn text(value: Scalar, context: &str) -> Result<Option<String>> {
    match value {
        Scalar::Null => Ok(None),
        Scalar::Text(value) => Ok(Some(value)),
        _ => Err(PlenoraError::Schema(format!("{context} richiede testo"))),
    }
}

pub fn compare(left: Scalar, right: Scalar) -> Result<Option<Ordering>> {
    match (left, right) {
        (Scalar::Null, _) | (_, Scalar::Null) => Ok(None),
        (Scalar::Number(left), Scalar::Number(right)) => confronta_numeri(left, right).map(Some),
        (Scalar::Text(left), Scalar::Text(right)) => Ok(Some(left.cmp(&right))),
        (Scalar::Boolean(left), Scalar::Boolean(right)) => Ok(Some(left.cmp(&right))),
        (Scalar::Date32(left), Scalar::Date32(right)) => Ok(Some(left.cmp(&right))),
        (Scalar::TimestampMs(left), Scalar::TimestampMs(right)) => Ok(Some(left.cmp(&right))),
        _ => Err(PlenoraError::Schema(
            "confronto expression fra tipi incompatibili".into(),
        )),
    }
}

fn arithmetic(op: BinaryOperator, left: &Scalar, right: &Scalar) -> Result<Scalar> {
    let (Some(left), Some(right)) = (number(left, "operatore")?, number(right, "operatore")?)
    else {
        return Ok(Scalar::Null);
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
            ))
        }
    };
    if value.is_finite() {
        Ok(Scalar::Number(Numero::double(value)))
    } else {
        Err(PlenoraError::Schema(NON_FINITE_RESULT_MESSAGE.into()))
    }
}

fn logical(op: BinaryOperator, left: &Scalar, right: &Scalar) -> Result<Scalar> {
    let left = boolean(left, "operatore logico")?;
    let right = boolean(right, "operatore logico")?;
    Ok(match op {
        BinaryOperator::And => match (left, right) {
            (Some(false), _) | (_, Some(false)) => Scalar::Boolean(false),
            (Some(true), Some(true)) => Scalar::Boolean(true),
            _ => Scalar::Null,
        },
        BinaryOperator::Or => match (left, right) {
            (Some(true), _) | (_, Some(true)) => Scalar::Boolean(true),
            (Some(false), Some(false)) => Scalar::Boolean(false),
            _ => Scalar::Null,
        },
        _ => {
            return Err(PlenoraError::InvalidPlan(
                "operatore logico inatteso".into(),
            ))
        }
    })
}

pub fn binary(op: BinaryOperator, left: Scalar, right: Scalar) -> Result<Scalar> {
    match op {
        BinaryOperator::Add
        | BinaryOperator::Subtract
        | BinaryOperator::Multiply
        | BinaryOperator::Divide => arithmetic(op, &left, &right),
        BinaryOperator::And | BinaryOperator::Or => logical(op, &left, &right),
        BinaryOperator::Equal => Ok(compare(left, right)?.map_or(Scalar::Null, |value| {
            Scalar::Boolean(value == Ordering::Equal)
        })),
        BinaryOperator::NotEqual => Ok(compare(left, right)?.map_or(Scalar::Null, |value| {
            Scalar::Boolean(value != Ordering::Equal)
        })),
        BinaryOperator::Greater => Ok(compare(left, right)?.map_or(Scalar::Null, |value| {
            Scalar::Boolean(value == Ordering::Greater)
        })),
        BinaryOperator::GreaterEqual => Ok(compare(left, right)?.map_or(Scalar::Null, |value| {
            Scalar::Boolean(value != Ordering::Less)
        })),
        BinaryOperator::Less => Ok(compare(left, right)?.map_or(Scalar::Null, |value| {
            Scalar::Boolean(value == Ordering::Less)
        })),
        BinaryOperator::LessEqual => Ok(compare(left, right)?.map_or(Scalar::Null, |value| {
            Scalar::Boolean(value != Ordering::Greater)
        })),
    }
}
