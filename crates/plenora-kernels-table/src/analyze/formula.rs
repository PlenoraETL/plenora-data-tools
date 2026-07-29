//! Analyzer a secco di `formula` ed `expression`
//! (kernel `formula.rs` / `expressions.rs`).

use plenora_core::arrow::schema::{DataType, TimeUnit};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    analyze_append, check_output_name, contract_error, field_of, is_scalar_string, typed,
};
use crate::{expressions, formula, Limits};

/// Numero massimo di nodi AST accettati nell'audit di `table.expression`
/// (limite statico dell'analisi a secco; il kernel riceve il valore dal
/// chiamante, qui non esiste un `Limits` dedicato).
const MAX_EXPRESSION_NODES: usize = 4_096;

// ---------------------------------------------------------------------------
// formula.rs / expressions.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_formula(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: formula::Formula = typed(op, config)?;
    let input = &inputs[0];
    formula::validate(&config, Limits::default().max_string_bytes)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: {error}")))?;
    let inferred = formula::infer_formula_type(&config, &|name| {
        let field = field_of(op, input, name)?;
        if matches!(field.data_type(), DataType::Int64 | DataType::Float64) {
            Ok(formula::FormulaType::Number)
        } else if is_scalar_string(field.data_type()) {
            Ok(formula::FormulaType::Text)
        } else {
            contract_error(
                op,
                format!("colonna {name}: tipo {:?} non valutabile", field.data_type()),
            )
        }
    })?;
    let data_type = match inferred {
        formula::FormulaType::Number => DataType::Float64,
        formula::FormulaType::Text => DataType::Utf8,
    };
    analyze_append(input, fields, &[(config.new_column, data_type, true)])
}

/// Tipo statico di un sotto-albero `Expression` (`Any` = letterale null,
/// tipo deciso dai dati ma coerente con qualsiasi altro). `Date32` e
/// `TimestampMs` sono i tipi temporali NATIVI prodotti da `date_trunc`
/// (decisione registrata: nessuna degradazione a Number per l'output di
/// `date_trunc`; le colonne Date32/Timestamp lette direttamente restano
/// `Number`, come nel kernel).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StaticType {
    Any,
    Number,
    Boolean,
    Text,
    Date32,
    TimestampMs,
}

fn meet_types(op: &str, left: StaticType, right: StaticType) -> Result<StaticType> {
    match (left, right) {
        (StaticType::Any, other) | (other, StaticType::Any) => Ok(other),
        (left, right) if left == right => Ok(left),
        _ => contract_error(
            op,
            "tipi eterogenei nell'espressione: dichiarare output_type esplicito",
        ),
    }
}

fn expect_type(
    op: &str,
    actual: StaticType,
    expected: StaticType,
    context: &str,
) -> Result<StaticType> {
    if actual == StaticType::Any || actual == expected {
        Ok(expected)
    } else {
        contract_error(op, format!("{context} richiede un operando {expected:?}"))
    }
}

// Le regole di tipo dell'AST sono intrinsecamente ramificate.
// match_same_arms: bracci identici ma di funzioni semanticamente distinte
// (es. coalesce/null_if vs greatest/least) restano separati per documentare
// ogni caso del dispatcher, come da prassi safety-critical.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn infer_expression_type(
    op: &str,
    input: &DataContract,
    expression: &expressions::Expression,
) -> Result<StaticType> {
    use expressions::{BinaryOperator, Expression, Function, UnaryOperator};
    match expression {
        Expression::Column { name } => {
            let field = field_of(op, input, name)?;
            Ok(match field.data_type() {
                DataType::Boolean => StaticType::Boolean,
                DataType::Int64
                | DataType::UInt64
                | DataType::Float64
                | DataType::Decimal128(_, _)
                | DataType::Date32
                | DataType::Timestamp(_, _) => StaticType::Number,
                data_type if is_scalar_string(data_type) => StaticType::Text,
                data_type => {
                    return contract_error(
                        op,
                        format!("colonna {name}: tipo {data_type:?} non valutabile"),
                    );
                }
            })
        }
        Expression::Literal { value } => match value {
            Value::Null => Ok(StaticType::Any),
            Value::Bool(_) => Ok(StaticType::Boolean),
            Value::Number(_) => Ok(StaticType::Number),
            Value::String(_) => Ok(StaticType::Text),
            Value::Array(_) | Value::Object(_) => {
                contract_error(op, "literal expression deve essere scalare")
            }
        },
        Expression::Unary { op: operator, value } => {
            let operand = infer_expression_type(op, input, value)?;
            match operator {
                UnaryOperator::Not => expect_type(op, operand, StaticType::Boolean, "not"),
                UnaryOperator::Negate => expect_type(op, operand, StaticType::Number, "negate"),
                UnaryOperator::IsNull | UnaryOperator::IsNotNull => Ok(StaticType::Boolean),
            }
        }
        Expression::Binary {
            op: operator,
            left,
            right,
        } => {
            let left = infer_expression_type(op, input, left)?;
            let right = infer_expression_type(op, input, right)?;
            match operator {
                BinaryOperator::Add
                | BinaryOperator::Subtract
                | BinaryOperator::Multiply
                | BinaryOperator::Divide => {
                    expect_type(op, left, StaticType::Number, "operatore aritmetico")?;
                    expect_type(op, right, StaticType::Number, "operatore aritmetico")
                }
                BinaryOperator::And | BinaryOperator::Or => {
                    expect_type(op, left, StaticType::Boolean, "operatore logico")?;
                    expect_type(op, right, StaticType::Boolean, "operatore logico")
                }
                BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual
                | BinaryOperator::Less
                | BinaryOperator::LessEqual => {
                    meet_types(op, left, right)?;
                    Ok(StaticType::Boolean)
                }
            }
        }
        Expression::Function { name, args } => {
            // Nodi speciali: la lista di `in` non e' uno scalare valutabile
            // e `date_trunc` ha regole di tipo temporali native dedicate.
            if matches!(name, Function::DateTrunc) {
                return infer_date_trunc_type(op, input, args);
            }
            if matches!(name, Function::In) {
                if args.len() != 2 {
                    return contract_error(op, "in richiede 2 argomenti");
                }
                infer_expression_type(op, input, &args[0])?;
                return Ok(StaticType::Boolean);
            }
            let types = args
                .iter()
                .map(|argument| infer_expression_type(op, input, argument))
                .collect::<Result<Vec<_>>>()?;
            let fold = |types: &[StaticType]| {
                types.iter().try_fold(StaticType::Any, |acc, item| {
                    meet_types(op, acc, *item)
                })
            };
            match name {
                Function::Coalesce | Function::NullIf => fold(&types),
                Function::Lower | Function::Upper | Function::Trim => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Text, "funzione testuale")?;
                    }
                    Ok(StaticType::Text)
                }
                Function::Concat => Ok(StaticType::Text),
                Function::Length => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Text, "length")?;
                    }
                    Ok(StaticType::Number)
                }
                Function::Contains | Function::StartsWith | Function::EndsWith => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Text, "predicato testuale")?;
                    }
                    Ok(StaticType::Boolean)
                }
                Function::Abs | Function::Round | Function::Year => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Number, "funzione numerica")?;
                    }
                    Ok(StaticType::Number)
                }
                Function::Substring => {
                    // (testo, numero, numero?) -> testo
                    if let Some((first, rest)) = types.split_first() {
                        expect_type(op, *first, StaticType::Text, "substring")?;
                        for item in rest {
                            expect_type(op, *item, StaticType::Number, "substring")?;
                        }
                    }
                    Ok(StaticType::Text)
                }
                Function::RegexReplace => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Text, "regex_replace")?;
                    }
                    Ok(StaticType::Text)
                }
                Function::Between => {
                    // Omogeneita' degli operandi come i confronti binari.
                    fold(&types)?;
                    Ok(StaticType::Boolean)
                }
                Function::Greatest | Function::Least => fold(&types),
                Function::Floor | Function::Ceil | Function::Power => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Number, "funzione numerica")?;
                    }
                    Ok(StaticType::Number)
                }
                Function::DateTrunc | Function::In => {
                    contract_error(op, "internal error: date_trunc/in hanno nodi dedicati")
                }
            }
        }
        Expression::Case {
            branches,
            else_value,
        } => {
            let mut result = infer_expression_type(op, input, else_value)?;
            for branch in branches {
                infer_expression_type(op, input, &branch.when)?;
                result = meet_types(op, result, infer_expression_type(op, input, &branch.then)?)?;
            }
            Ok(result)
        }
    }
}

pub(in crate::analyze) fn analyze_expression(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: expressions::ExpressionTransform = typed(op, config)?;
    let input = &inputs[0];
    expressions::validate(&config, MAX_EXPRESSION_NODES)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: {error}")))?;
    check_output_name(op, &config.output_column)?;
    let data_type = match config.output_type {
        expressions::OutputType::Number => DataType::Float64,
        expressions::OutputType::Boolean => DataType::Boolean,
        expressions::OutputType::Text => DataType::Utf8,
        expressions::OutputType::Date32 => DataType::Date32,
        expressions::OutputType::TimestampMs => DataType::Timestamp(TimeUnit::Millisecond, None),
        expressions::OutputType::Auto => {
            // Auto e' risolto dal kernel sui dati: l'analisi statica prova a
            // determinarlo dall'AST; tutto-null -> Text come il kernel, salvo
            // radice date_trunc (tipo temporale dalla colonna di input).
            match infer_expression_type(op, input, &config.expression)? {
                StaticType::Number => DataType::Float64,
                StaticType::Boolean => DataType::Boolean,
                StaticType::Any | StaticType::Text => DataType::Utf8,
                StaticType::Date32 => DataType::Date32,
                StaticType::TimestampMs => DataType::Timestamp(TimeUnit::Millisecond, None),
            }
        }
    };
    analyze_append(input, fields, &[(config.output_column, data_type, true)])
}

/// Regole di tipo di `date_trunc` (tipi temporali nativi, decisione
/// registrata): l'unita' e' un letterale del set chiuso; il tipo di output
/// discende dal tipo della colonna di input (anche su dati tutti null);
/// nessun parsing implicito di stringhe; timestamp timezone-aware rifiutati
/// (semantica tz del troncamento non definibile in modo sicuro: l'output e'
/// sempre naive).
fn infer_date_trunc_type(
    op: &str,
    input: &DataContract,
    args: &[expressions::Expression],
) -> Result<StaticType> {
    if args.len() != 2 {
        return contract_error(op, "date_trunc richiede 2 argomenti");
    }
    let expressions::Expression::Literal {
        value: Value::String(unit),
    } = &args[0]
    else {
        return contract_error(op, "date_trunc: unit deve essere un letterale stringa");
    };
    if !matches!(
        unit.as_str(),
        "year" | "month" | "day" | "hour" | "minute" | "second"
    ) {
        return contract_error(op, format!("date_trunc: unita' non valida: {unit}"));
    }
    temporal_static_type(op, input, &args[1], unit)
}

/// Tipo temporale statico della sorgente di `date_trunc`; `unit` e' l'unita'
/// del livello corrente (sub-day rifiutata su Date32).
fn temporal_static_type(
    op: &str,
    input: &DataContract,
    expression: &expressions::Expression,
    unit: &str,
) -> Result<StaticType> {
    use expressions::Expression;
    match expression {
        Expression::Column { name } => {
            let field = field_of(op, input, name)?;
            match field.data_type() {
                DataType::Date32 => {
                    if matches!(unit, "hour" | "minute" | "second") {
                        return contract_error(
                            op,
                            "date_trunc: unita' sub-day non ammessa su Date32",
                        );
                    }
                    Ok(StaticType::Date32)
                }
                DataType::Timestamp(TimeUnit::Millisecond, timezone) => {
                    if timezone.is_some() {
                        return contract_error(
                            op,
                            "date_trunc: timestamp timezone-aware non supportato",
                        );
                    }
                    Ok(StaticType::TimestampMs)
                }
                other => contract_error(
                    op,
                    format!(
                        "date_trunc richiede una colonna Date32 o Timestamp(ms), trovato {other:?}"
                    ),
                ),
            }
        }
        Expression::Function {
            name: expressions::Function::DateTrunc,
            args,
        } => {
            let kind = infer_date_trunc_type(op, input, args)?;
            if kind == StaticType::Date32 && matches!(unit, "hour" | "minute" | "second") {
                return contract_error(op, "date_trunc: unita' sub-day non ammessa su Date32");
            }
            Ok(kind)
        }
        Expression::Literal {
            value: Value::Null,
        } => Ok(StaticType::Any),
        _ => contract_error(op, "date_trunc: il valore deve essere una colonna temporale"),
    }
}

