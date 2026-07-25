use std::borrow::Cow;
use std::cmp::Ordering;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int64Array,
    RecordBatch, StringArray, TimestampMillisecondArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, TimeUnit};
use chrono::{Datelike, NaiveDate};
use num_traits::ToPrimitive;
use serde::Deserialize;
use serde_json::Value;

use plenora_core::{PlenoraError, Result};
use crate::{column_index, replace_or_append, scalar_as_f64, scalar_as_string};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    Auto,
    Number,
    Boolean,
    Text,
}

const fn default_output_type() -> OutputType {
    OutputType::Auto
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionTransform {
    pub output_column: String,
    pub expression: Expression,
    #[serde(default = "default_output_type")]
    pub output_type: OutputType,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expression {
    Column {
        name: String,
    },
    Literal {
        value: Value,
    },
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
        branches: Vec<CaseBranch>,
        else_value: Box<Self>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseBranch {
    pub when: Expression,
    pub then: Expression,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOperator {
    Not,
    Negate,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Function {
    Coalesce,
    NullIf,
    Lower,
    Upper,
    Trim,
    Length,
    Concat,
    Contains,
    StartsWith,
    EndsWith,
    Abs,
    Round,
    Year,
}

#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    Null,
    Number(f64),
    Boolean(bool),
    Text(String),
}

fn literal(value: &Value) -> Result<Scalar> {
    match value {
        Value::Null => Ok(Scalar::Null),
        Value::Bool(value) => Ok(Scalar::Boolean(*value)),
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Scalar::Number)
            .ok_or_else(|| PlenoraError::Contract("literal numerico non finito".into())),
        Value::String(value) => Ok(Scalar::Text(value.clone())),
        Value::Array(_) | Value::Object(_) => Err(PlenoraError::Contract(
            "literal expression deve essere scalare".into(),
        )),
    }
}

fn column(batch: &RecordBatch, name: &str, row: usize) -> Result<Scalar> {
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
        return scalar_as_f64(value.as_ref(), row)?.map_or(Ok(Scalar::Null), |value| {
            if value.is_finite() {
                Ok(Scalar::Number(value))
            } else {
                Err(PlenoraError::Schema(
                    "expression non accetta numeri non finiti".into(),
                ))
            }
        });
    }
    scalar_as_string(value.as_ref(), row)?.map_or(Ok(Scalar::Null), |value| Ok(Scalar::Text(value)))
}

fn boolean(value: &Scalar, context: &str) -> Result<Option<bool>> {
    match value {
        Scalar::Null => Ok(None),
        Scalar::Boolean(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!(
            "{context} richiede un booleano"
        ))),
    }
}

fn number(value: &Scalar, context: &str) -> Result<Option<f64>> {
    match value {
        Scalar::Null => Ok(None),
        Scalar::Number(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!("{context} richiede un numero"))),
    }
}

fn text(value: Scalar, context: &str) -> Result<Option<String>> {
    match value {
        Scalar::Null => Ok(None),
        Scalar::Text(value) => Ok(Some(value)),
        _ => Err(PlenoraError::Schema(format!("{context} richiede testo"))),
    }
}

fn compare(left: Scalar, right: Scalar) -> Result<Option<Ordering>> {
    match (left, right) {
        (Scalar::Null, _) | (_, Scalar::Null) => Ok(None),
        (Scalar::Number(left), Scalar::Number(right)) => Ok(Some(left.total_cmp(&right))),
        (Scalar::Text(left), Scalar::Text(right)) => Ok(Some(left.cmp(&right))),
        (Scalar::Boolean(left), Scalar::Boolean(right)) => Ok(Some(left.cmp(&right))),
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
            return Err(PlenoraError::Schema("divisione per zero".into()));
        }
        BinaryOperator::Divide => left / right,
        _ => {
            return Err(PlenoraError::Contract(
                "operatore aritmetico inatteso".into(),
            ))
        }
    };
    if value.is_finite() {
        Ok(Scalar::Number(value))
    } else {
        Err(PlenoraError::Schema(
            "risultato expression non finito".into(),
        ))
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
        _ => return Err(PlenoraError::Contract("operatore logico inatteso".into())),
    })
}

fn binary(op: BinaryOperator, left: Scalar, right: Scalar) -> Result<Scalar> {
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

fn exact_args<'a>(args: &'a [Scalar], count: usize, name: &str) -> Result<&'a [Scalar]> {
    if args.len() == count {
        Ok(args)
    } else {
        Err(PlenoraError::Contract(format!(
            "{name} richiede {count} argomenti"
        )))
    }
}

#[allow(clippy::too_many_lines)]
fn function(name: Function, args: Vec<Scalar>) -> Result<Scalar> {
    match name {
        Function::Coalesce => {
            if args.is_empty() {
                return Err(PlenoraError::Contract("coalesce richiede argomenti".into()));
            }
            Ok(args
                .into_iter()
                .find(|value| value != &Scalar::Null)
                .unwrap_or(Scalar::Null))
        }
        Function::NullIf => {
            exact_args(&args, 2, "null_if")?;
            if args[0] != Scalar::Null
                && compare(args[0].clone(), args[1].clone())? == Some(Ordering::Equal)
            {
                Ok(Scalar::Null)
            } else {
                Ok(args[0].clone())
            }
        }
        Function::Lower | Function::Upper | Function::Trim | Function::Length | Function::Year => {
            exact_args(&args, 1, "funzione unaria")?;
            let Some(value) = text(args[0].clone(), "funzione")? else {
                return Ok(Scalar::Null);
            };
            Ok(match name {
                Function::Lower => Scalar::Text(value.to_lowercase()),
                Function::Upper => Scalar::Text(value.to_uppercase()),
                Function::Trim => Scalar::Text(value.trim().to_owned()),
                Function::Length => Scalar::Number(
                    u32::try_from(value.chars().count())
                        .map(f64::from)
                        .map_err(|_| PlenoraError::Contract("testo troppo lungo".into()))?,
                ),
                Function::Year => {
                    let date =
                        NaiveDate::parse_from_str(value.get(..10).unwrap_or(&value), "%Y-%m-%d")
                            .map_err(|_| PlenoraError::Schema("year: data non valida".into()))?;
                    Scalar::Number(f64::from(date.year()))
                }
                _ => unreachable!(),
            })
        }
        Function::Concat => {
            if args.is_empty() {
                return Err(PlenoraError::Contract("concat richiede argomenti".into()));
            }
            let mut output = String::new();
            for value in args {
                let Some(value) = text(value, "concat")? else {
                    return Ok(Scalar::Null);
                };
                output.push_str(&value);
            }
            Ok(Scalar::Text(output))
        }
        Function::Contains | Function::StartsWith | Function::EndsWith => {
            exact_args(&args, 2, "funzione testo")?;
            let (Some(value), Some(pattern)) = (
                text(args[0].clone(), "funzione testo")?,
                text(args[1].clone(), "funzione testo")?,
            ) else {
                return Ok(Scalar::Null);
            };
            Ok(Scalar::Boolean(match name {
                Function::Contains => value.contains(&pattern),
                Function::StartsWith => value.starts_with(&pattern),
                Function::EndsWith => value.ends_with(&pattern),
                _ => unreachable!(),
            }))
        }
        Function::Abs | Function::Round => {
            exact_args(&args, 1, "funzione numerica")?;
            let Some(value) = number(&args[0], "funzione numerica")? else {
                return Ok(Scalar::Null);
            };
            Ok(Scalar::Number(match name {
                Function::Abs => value.abs(),
                Function::Round => value.round(),
                _ => unreachable!(),
            }))
        }
    }
}

fn evaluate(expression: &Expression, batch: &RecordBatch, row: usize) -> Result<Scalar> {
    match expression {
        Expression::Column { name } => column(batch, name, row),
        Expression::Literal { value } => literal(value),
        Expression::Unary { op, value } => {
            let value = evaluate(value, batch, row)?;
            Ok(match op {
                UnaryOperator::IsNull => Scalar::Boolean(value == Scalar::Null),
                UnaryOperator::IsNotNull => Scalar::Boolean(value != Scalar::Null),
                UnaryOperator::Not => {
                    boolean(&value, "not")?.map_or(Scalar::Null, |value| Scalar::Boolean(!value))
                }
                UnaryOperator::Negate => {
                    number(&value, "negate")?.map_or(Scalar::Null, |value| Scalar::Number(-value))
                }
            })
        }
        Expression::Binary { op, left, right } => binary(
            *op,
            evaluate(left, batch, row)?,
            evaluate(right, batch, row)?,
        ),
        Expression::Function { name, args } => function(
            *name,
            args.iter()
                .map(|arg| evaluate(arg, batch, row))
                .collect::<Result<Vec<_>>>()?,
        ),
        Expression::Case {
            branches,
            else_value,
        } => {
            for branch in branches {
                if boolean(&evaluate(&branch.when, batch, row)?, "case when")? == Some(true) {
                    return evaluate(&branch.then, batch, row);
                }
            }
            evaluate(else_value, batch, row)
        }
    }
}

fn audit(expression: &Expression, depth: usize, nodes: &mut usize, max_nodes: usize) -> Result<()> {
    if depth > 64 {
        return Err(PlenoraError::Contract(
            "expression supera la profondita' massima".into(),
        ));
    }
    *nodes = nodes
        .checked_add(1)
        .ok_or_else(|| PlenoraError::Contract("overflow nodi expression".into()))?;
    if *nodes > max_nodes {
        return Err(PlenoraError::Contract(
            "expression supera il numero massimo di nodi".into(),
        ));
    }
    match expression {
        Expression::Column { name } if name.trim().is_empty() => {
            Err(PlenoraError::Contract("colonna expression vuota".into()))
        }
        Expression::Literal { value } => literal(value).map(|_| ()),
        Expression::Unary { value, .. } => audit(value, depth + 1, nodes, max_nodes),
        Expression::Binary { left, right, .. } => {
            audit(left, depth + 1, nodes, max_nodes)?;
            audit(right, depth + 1, nodes, max_nodes)
        }
        Expression::Function { args, .. } => {
            if args.len() > 64 {
                return Err(PlenoraError::Contract("troppi argomenti expression".into()));
            }
            for arg in args {
                audit(arg, depth + 1, nodes, max_nodes)?;
            }
            Ok(())
        }
        Expression::Case {
            branches,
            else_value,
        } => {
            if branches.is_empty() || branches.len() > 64 {
                return Err(PlenoraError::Contract("numero rami case non valido".into()));
            }
            for branch in branches {
                audit(&branch.when, depth + 1, nodes, max_nodes)?;
                audit(&branch.then, depth + 1, nodes, max_nodes)?;
            }
            audit(else_value, depth + 1, nodes, max_nodes)
        }
        Expression::Column { .. } => Ok(()),
    }
}

pub fn validate(config: &ExpressionTransform, max_nodes: usize) -> Result<()> {
    crate::validate_output_name(&config.output_column)?;
    audit(&config.expression, 1, &mut 0, max_nodes)
}

fn resolved_output_type(values: &[Scalar], configured: OutputType) -> Result<OutputType> {
    if !matches!(configured, OutputType::Auto) {
        return Ok(configured);
    }
    let mut resolved = None;
    for value in values {
        let current = match value {
            Scalar::Null => continue,
            Scalar::Number(_) => OutputType::Number,
            Scalar::Boolean(_) => OutputType::Boolean,
            Scalar::Text(_) => OutputType::Text,
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

pub fn expression(batch: &RecordBatch, config: &ExpressionTransform) -> Result<RecordBatch> {
    // Batch vuoto: il generico non valuta mai i nodi (colonne non risolte,
    // letterali non convertiti); si mantiene quel comportamento saltando la
    // compilazione.
    if batch.num_rows() > 0 {
        return FastProgram::compile(&config.expression, batch).run(batch, config);
    }
    expression_generic(batch, config)
}

/// Percorso generico originale: interprete ricorsivo sull'AST, usato sui
/// batch vuoti e come oracolo dei test.
fn expression_generic(batch: &RecordBatch, config: &ExpressionTransform) -> Result<RecordBatch> {
    let values = (0..batch.num_rows())
        .map(|row| evaluate(&config.expression, batch, row))
        .collect::<Result<Vec<_>>>()?;
    match resolved_output_type(&values, config.output_type)? {
        OutputType::Auto => unreachable!(),
        OutputType::Number => replace_or_append(
            batch,
            &config.output_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(
                values
                    .into_iter()
                    .map(|value| number(&value, "output_type=number"))
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
                    .into_iter()
                    .map(|value| boolean(&value, "output_type=boolean"))
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
                    .into_iter()
                    .map(|value| text(value, "output_type=text"))
                    .collect::<Result<Vec<_>>>()?,
            )),
        ),
    }
}

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
    Contract(String),
    Schema(String),
}

impl LazyError {
    fn build(&self) -> PlenoraError {
        match self {
            Self::Contract(message) => PlenoraError::Contract(message.clone()),
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
                    finite_number(values.value(row).to_f64().ok_or_else(|| {
                        PlenoraError::Schema("decimal128 non rappresentabile come f64".into())
                    })? / factor)
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
        Err(PlenoraError::Schema(
            "expression non accetta numeri non finiti".into(),
        ))
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
        return match scalar_as_f64(array.as_ref(), row)? {
            None => Ok(FastValue::Null),
            Some(value) => finite_number(value),
        };
    }
    Ok(match scalar_as_string(array.as_ref(), row)? {
        None => FastValue::Null,
        Some(value) => FastValue::Text(Cow::Owned(value)),
    })
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
        _ => Err(PlenoraError::Schema(format!("{context} richiede un numero"))),
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
    let (Some(left), Some(right)) =
        (fast_number(left, "operatore")?, fast_number(right, "operatore")?)
    else {
        return Ok(FastValue::Null);
    };
    let value = match op {
        BinaryOperator::Add => left + right,
        BinaryOperator::Subtract => left - right,
        BinaryOperator::Multiply => left * right,
        BinaryOperator::Divide if right == 0.0 => {
            return Err(PlenoraError::Schema("divisione per zero".into()));
        }
        BinaryOperator::Divide => left / right,
        _ => {
            return Err(PlenoraError::Contract(
                "operatore aritmetico inatteso".into(),
            ));
        }
    };
    if value.is_finite() {
        Ok(FastValue::Number(value))
    } else {
        Err(PlenoraError::Schema(
            "risultato expression non finito".into(),
        ))
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
        _ => return Err(PlenoraError::Contract("operatore logico inatteso".into())),
    })
}

/// Equivalente di `binary` su `FastValue`.
fn fast_binary<'a>(
    op: BinaryOperator,
    left: FastValue<'a>,
    right: FastValue<'a>,
) -> Result<FastValue<'a>> {
    match op {
        BinaryOperator::Add
        | BinaryOperator::Subtract
        | BinaryOperator::Multiply
        | BinaryOperator::Divide => fast_arithmetic(op, &left, &right),
        BinaryOperator::And | BinaryOperator::Or => fast_logical(op, &left, &right),
        BinaryOperator::Equal => {
            Ok(fast_compare(&left, &right)?.map_or(FastValue::Null, |value| {
                FastValue::Boolean(value == Ordering::Equal)
            }))
        }
        BinaryOperator::NotEqual => {
            Ok(fast_compare(&left, &right)?.map_or(FastValue::Null, |value| {
                FastValue::Boolean(value != Ordering::Equal)
            }))
        }
        BinaryOperator::Greater => {
            Ok(fast_compare(&left, &right)?.map_or(FastValue::Null, |value| {
                FastValue::Boolean(value == Ordering::Greater)
            }))
        }
        BinaryOperator::GreaterEqual => {
            Ok(fast_compare(&left, &right)?.map_or(FastValue::Null, |value| {
                FastValue::Boolean(value != Ordering::Less)
            }))
        }
        BinaryOperator::Less => {
            Ok(fast_compare(&left, &right)?.map_or(FastValue::Null, |value| {
                FastValue::Boolean(value == Ordering::Less)
            }))
        }
        BinaryOperator::LessEqual => {
            Ok(fast_compare(&left, &right)?.map_or(FastValue::Null, |value| {
                FastValue::Boolean(value != Ordering::Greater)
            }))
        }
    }
}

/// Controllo argomenti identico a `exact_args`.
fn exact_args_fast(args: &[FastValue<'_>], count: usize, name: &str) -> Result<()> {
    if args.len() == count {
        Ok(())
    } else {
        Err(PlenoraError::Contract(format!(
            "{name} richiede {count} argomenti"
        )))
    }
}

/// Equivalente di `function` su `FastValue`.
#[allow(clippy::too_many_lines)]
fn fast_function<'a>(name: Function, args: Vec<FastValue<'a>>) -> Result<FastValue<'a>> {
    match name {
        Function::Coalesce => {
            if args.is_empty() {
                return Err(PlenoraError::Contract("coalesce richiede argomenti".into()));
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
                        .map_err(|_| PlenoraError::Contract("testo troppo lungo".into()))?,
                ),
                Function::Year => {
                    let date =
                        NaiveDate::parse_from_str(value.get(..10).unwrap_or(value), "%Y-%m-%d")
                            .map_err(|_| PlenoraError::Schema("year: data non valida".into()))?;
                    FastValue::Number(f64::from(date.year()))
                }
                _ => unreachable!(),
            })
        }
        Function::Concat => {
            if args.is_empty() {
                return Err(PlenoraError::Contract("concat richiede argomenti".into()));
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
                _ => unreachable!(),
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
                _ => unreachable!(),
            }))
        }
    }
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
}

fn compile_literal(value: &Value) -> FastNode<'_> {
    match value {
        Value::Null => FastNode::Literal(FastLiteral::Null),
        Value::Bool(value) => FastNode::Literal(FastLiteral::Boolean(*value)),
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map_or_else(
                || FastNode::Error(LazyError::Contract("literal numerico non finito".into())),
                |value| FastNode::Literal(FastLiteral::Number(value)),
            ),
        Value::String(value) => FastNode::Literal(FastLiteral::Text(value.as_str())),
        Value::Array(_) | Value::Object(_) => FastNode::Error(LazyError::Contract(
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
        Expression::Function { name, args } => FastNode::Function {
            name: *name,
            args: args
                .iter()
                .map(|arg| compile_expression(arg, batch))
                .collect(),
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
        FastNode::Binary { op, left, right } => fast_binary(
            *op,
            evaluate_fast(left, row)?,
            evaluate_fast(right, row)?,
        ),
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

struct FastProgram<'a> {
    root: FastNode<'a>,
}

impl<'a> FastProgram<'a> {
    fn compile(expression: &'a Expression, batch: &'a RecordBatch) -> Self {
        Self {
            root: compile_expression(expression, batch),
        }
    }

    fn run(&self, batch: &RecordBatch, config: &ExpressionTransform) -> Result<RecordBatch> {
        let values = (0..batch.num_rows())
            .map(|row| evaluate_fast(&self.root, row))
            .collect::<Result<Vec<_>>>()?;
        match resolved_output_type_fast(&values, config.output_type)? {
            OutputType::Auto => unreachable!(),
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
        }
    }
}

// ---------------------------------------------------------------------------
// Test-oracolo: fast path compilato vs interprete generico.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{
        BooleanArray, Date32Array, Float64Array, Int64Array, StringArray, UInt64Array,
    };
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::json;

    use super::*;

    /// Fixture con null, -0.0, zeri, testi (anche data-like) e booleani.
    /// La colonna `nan` contiene NaN: la lettura deve fallire in entrambi i
    /// percorsi ("expression non accetta numeri non finiti").
    fn fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("n", DataType::Float64, true),
                Field::new("nan", DataType::Float64, true),
                Field::new("i", DataType::Int64, true),
                Field::new("u", DataType::UInt64, true),
                Field::new("d", DataType::Date32, true),
                Field::new("s", DataType::Utf8, true),
                Field::new("b", DataType::Boolean, true),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    None,
                    Some(-0.0),
                    Some(4.0),
                    Some(0.0),
                    Some(7.25),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.0),
                    Some(f64::NAN),
                    Some(2.0),
                    None,
                    Some(0.0),
                    Some(-0.0),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(3),
                    None,
                    Some(0),
                    Some(-2),
                    Some(10),
                    Some(1),
                ])),
                Arc::new(UInt64Array::from(vec![
                    Some(5),
                    None,
                    Some(u64::MAX),
                    Some(2),
                    Some(0),
                    Some(9),
                ])),
                Arc::new(Date32Array::from(vec![
                    Some(0),
                    None,
                    Some(19_000),
                    Some(-1),
                    Some(1),
                    Some(20_000),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("Ciao"),
                    None,
                    Some("2024-05-06"),
                    Some(""),
                    Some("x42y"),
                    Some("ab"),
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    None,
                    Some(false),
                    Some(true),
                    Some(false),
                    None,
                ])),
            ],
        )
        .expect("fixture")
    }

    fn col(name: &str) -> Value {
        json!({"kind": "column", "name": name})
    }

    fn lit(value: Value) -> Value {
        json!({"kind": "literal", "value": value})
    }

    fn bin(op: &str, left: Value, right: Value) -> Value {
        json!({"kind": "binary", "op": op, "left": left, "right": right})
    }

    fn un(op: &str, value: Value) -> Value {
        json!({"kind": "unary", "op": op, "value": value})
    }

    fn func(name: &str, args: Vec<Value>) -> Value {
        json!({"kind": "function", "name": name, "args": args})
    }

    fn case(branches: Vec<(Value, Value)>, else_value: Value) -> Value {
        json!({
            "kind": "case",
            "branches": branches
                .into_iter()
                .map(|(when, then)| json!({"when": when, "then": then}))
                .collect::<Vec<_>>(),
            "else_value": else_value,
        })
    }

    fn config(expression: Value, output_type: Option<&str>) -> ExpressionTransform {
        let mut value = json!({"output_column": "out", "expression": expression});
        if let Some(output_type) = output_type {
            value["output_type"] = json!(output_type);
        }
        serde_json::from_value(value).expect("config valida")
    }

    fn assert_equivalent(batch: &RecordBatch, expression: Value, output_type: Option<&str>) {
        let config = config(expression, output_type);
        let fast = FastProgram::compile(&config.expression, batch).run(batch, &config);
        let generic = expression_generic(batch, &config);
        match (fast, generic) {
            (Ok(fast), Ok(generic)) => assert_eq!(fast, generic),
            (fast, generic) => {
                assert_eq!(
                    fast.as_ref().map_err(ToString::to_string).map(|_| ()),
                    generic.as_ref().map_err(ToString::to_string).map(|_| ()),
                );
            }
        }
    }

    #[test]
    fn oracle_aritmetica_e_confronti() {
        let batch = fixture();
        for op in ["add", "subtract", "multiply", "divide"] {
            assert_equivalent(&batch, bin(op, col("n"), col("i")), None);
            assert_equivalent(&batch, bin(op, col("n"), lit(json!(2.5))), None);
            assert_equivalent(&batch, bin(op, lit(json!(10)), col("i")), None);
        }
        for op in [
            "equal",
            "not_equal",
            "greater",
            "greater_equal",
            "less",
            "less_equal",
        ] {
            // Numeri: include -0.0 vs 0.0 (total_cmp: -0.0 < 0.0).
            assert_equivalent(&batch, bin(op, col("n"), lit(json!(0.0))), None);
            assert_equivalent(&batch, bin(op, col("n"), col("i")), None);
            // Testi e booleani.
            assert_equivalent(&batch, bin(op, col("s"), lit(json!("x42y"))), None);
            assert_equivalent(&batch, bin(op, col("b"), lit(json!(true))), None);
            // Tipi misti: errore di confronto incompatibile.
            assert_equivalent(&batch, bin(op, col("n"), col("s")), None);
            assert_equivalent(&batch, bin(op, col("b"), col("n")), None);
        }
        // UInt64 (anche u64::MAX) e Date32 come numeri.
        assert_equivalent(&batch, bin("add", col("u"), lit(json!(1))), None);
        assert_equivalent(&batch, bin("add", col("d"), lit(json!(1))), None);
        assert_equivalent(&batch, bin("greater", col("u"), col("i")), None);
    }

    #[test]
    fn oracle_logica_e_unari() {
        let batch = fixture();
        for op in ["and", "or"] {
            assert_equivalent(&batch, bin(op, col("b"), lit(json!(true))), None);
            assert_equivalent(&batch, bin(op, col("b"), col("b")), None);
            // Logica su non booleani: errore.
            assert_equivalent(&batch, bin(op, col("n"), col("b")), None);
        }
        for op in ["not", "negate", "is_null", "is_not_null"] {
            assert_equivalent(&batch, un(op, col("b")), None);
            assert_equivalent(&batch, un(op, col("n")), None);
            assert_equivalent(&batch, un(op, col("s")), None);
            assert_equivalent(&batch, un(op, lit(Value::Null)), None);
        }
    }

    #[test]
    fn oracle_funzioni() {
        let batch = fixture();
        let cases = vec![
            func("coalesce", vec![col("s"), lit(json!("fb"))]),
            func("coalesce", vec![col("n"), col("i"), lit(json!(0))]),
            func("coalesce", vec![lit(Value::Null)]),
            func("null_if", vec![col("s"), lit(json!("ab"))]),
            func("null_if", vec![col("n"), lit(json!(0.0))]),
            func("lower", vec![col("s")]),
            func("upper", vec![col("s")]),
            func("trim", vec![col("s")]),
            func("length", vec![col("s")]),
            func("year", vec![col("s")]),
            func("year", vec![lit(json!("2024-12-31"))]),
            func("concat", vec![col("s"), lit(json!("-")), col("s")]),
            func("concat", vec![lit(json!("solo"))]),
            func("contains", vec![col("s"), lit(json!("42"))]),
            func("starts_with", vec![col("s"), lit(json!("Ci"))]),
            func("ends_with", vec![col("s"), lit(json!("y"))]),
            func("abs", vec![col("n")]),
            func("round", vec![col("n")]),
            func("abs", vec![col("s")]),
            func("lower", vec![col("n")]),
            func("null_if", vec![col("s")]),
            func("concat", vec![]),
            func("coalesce", vec![]),
            func("contains", vec![col("s")]),
        ];
        for expression in cases {
            assert_equivalent(&batch, expression, None);
        }
    }

    #[test]
    fn oracle_case_e_errori_lazy() {
        let batch = fixture();
        // Case base su booleani con null.
        assert_equivalent(
            &batch,
            case(
                vec![(col("b"), col("n")), (lit(json!(true)), col("i"))],
                lit(json!(0)),
            ),
            None,
        );
        // Ramo non percorso con colonna mancante: nessun errore (lazy).
        assert_equivalent(
            &batch,
            case(
                vec![(lit(json!(false)), col("missing"))],
                lit(json!(1)),
            ),
            None,
        );
        // Ramo percorso con colonna mancante: errore identico.
        assert_equivalent(
            &batch,
            case(vec![(lit(json!(true)), col("missing"))], lit(json!(1))),
            None,
        );
        // Ramo non percorso con letterale non scalare: nessun errore (lazy).
        assert_equivalent(
            &batch,
            case(
                vec![(lit(json!(false)), lit(json!([1, 2])))],
                lit(json!(1)),
            ),
            None,
        );
        // Letterale non scalare valutato: errore identico.
        assert_equivalent(&batch, lit(json!([1, 2])), None);
        assert_equivalent(&batch, lit(json!({"a": 1})), None);
        // When non booleano: errore identico.
        assert_equivalent(
            &batch,
            case(vec![(col("n"), col("i"))], lit(json!(0))),
            None,
        );
        // Output eterogeneo in auto: errore identico.
        assert_equivalent(
            &batch,
            case(vec![(col("b"), col("n"))], col("s")),
            None,
        );
    }

    #[test]
    fn oracle_errori_di_dominio() {
        let batch = fixture();
        // Divisione per zero (anche -0.0) a righe diverse.
        assert_equivalent(&batch, bin("divide", col("n"), lit(json!(0.0))), None);
        assert_equivalent(&batch, bin("divide", col("n"), lit(json!(-0.0))), None);
        assert_equivalent(&batch, bin("divide", col("n"), col("i")), None);
        // Risultato non finito.
        assert_equivalent(
            &batch,
            bin("multiply", lit(json!(1e308)), lit(json!(10.0))),
            None,
        );
        // NaN in colonna: lettura rifiutata in entrambi i percorsi.
        assert_equivalent(&batch, col("nan"), None);
        assert_equivalent(&batch, bin("equal", col("nan"), col("nan")), None);
        // Colonna mancante in testa.
        assert_equivalent(&batch, bin("add", col("missing"), lit(json!(1))), None);
        // output_type dichiarato con conversione impossibile.
        assert_equivalent(&batch, col("s"), Some("number"));
        assert_equivalent(&batch, col("n"), Some("text"));
        assert_equivalent(&batch, col("b"), Some("boolean"));
        assert_equivalent(&batch, col("n"), Some("number"));
    }

    #[test]
    fn oracle_ast_profondo_e_batch_vuoto() {
        let batch = fixture();
        // Catena di negate annidati (profondita' 60, entro il limite di audit).
        let mut deep = col("n");
        for _ in 0..60 {
            deep = un("negate", deep);
        }
        assert_equivalent(&batch, deep, None);
        // Catena binaria profonda a sinistra.
        let mut left_deep = lit(json!(1));
        for _ in 0..60 {
            left_deep = bin("add", left_deep, col("i"));
        }
        assert_equivalent(&batch, left_deep, None);

        // Batch vuoto: nessuna valutazione, colonna mancante e letterale non
        // scalare non falliscono; output Auto risolto come Utf8 vuoto.
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("n", DataType::Float64, true)])),
            vec![Arc::new(Float64Array::from(Vec::<f64>::new()))],
        )
        .expect("empty");
        for ast in [col("missing"), lit(json!([1, 2])), col("n")] {
            let config = config(ast, None);
            let output = expression(&empty, &config).expect("zero righe: nessun errore");
            assert_eq!(output.num_rows(), 0);
            let generic = expression_generic(&empty, &config).expect("generico");
            assert_eq!(output, generic);
        }
    }
}
