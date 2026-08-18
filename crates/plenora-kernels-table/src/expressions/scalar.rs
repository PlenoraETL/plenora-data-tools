use std::cmp::Ordering;

use plenora_core::arrow::array::{Array, BooleanArray, RecordBatch};
use plenora_core::arrow::schema::DataType;
use serde_json::Value;

use super::BinaryOperator;
use crate::{
    column_index, scalar_as_f64_rounded, scalar_as_string, DIVISION_BY_ZERO_MESSAGE,
    NON_FINITE_INPUT_MESSAGE, NON_FINITE_RESULT_MESSAGE,
};
use plenora_core::{PlenoraError, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Null,
    Number(f64),
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
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Scalar::Number)
            .ok_or_else(|| PlenoraError::InvalidPlan("literal numerico non finito".into())),
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
        return scalar_as_f64_rounded(value.as_ref(), row)?.map_or(Ok(Scalar::Null), |value| {
            if value.is_finite() {
                Ok(Scalar::Number(value))
            } else {
                Err(PlenoraError::Schema(NON_FINITE_INPUT_MESSAGE.into()))
            }
        });
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
        (Scalar::Number(left), Scalar::Number(right)) => Ok(Some(left.total_cmp(&right))),
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
        Ok(Scalar::Number(value))
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
