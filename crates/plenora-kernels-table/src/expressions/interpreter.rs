use std::cmp::Ordering;
use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use serde_json::Value;

use super::fast::FastProgram;
use super::scalar::{binary, boolean, column, compare, literal, number, text, Scalar};
use super::static_type::{self, Kind};
use super::temporal::{date_trunc_generic, in_generic, literal_unit};
use super::{Expression, ExpressionTransform, Function, UnaryOperator};
use crate::{column_index, replace_or_append, NON_FINITE_RESULT_MESSAGE};
use plenora_core::arrow::array::{
    BooleanArray, Date32Array, Float64Array, RecordBatch, StringArray, TimestampMillisecondArray,
};
use plenora_core::arrow::schema::{DataType, TimeUnit};
use plenora_core::{PlenoraError, Result};

fn exact_args<'a>(args: &'a [Scalar], count: usize, name: &str) -> Result<&'a [Scalar]> {
    if args.len() == count {
        Ok(args)
    } else {
        Err(PlenoraError::InvalidPlan(format!(
            "{name} richiede {count} argomenti"
        )))
    }
}

#[allow(clippy::too_many_lines)]
fn function(name: Function, args: Vec<Scalar>) -> Result<Scalar> {
    match name {
        Function::Coalesce => {
            if args.is_empty() {
                return Err(PlenoraError::InvalidPlan(
                    "coalesce richiede argomenti".into(),
                ));
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
                        .map_err(|_| PlenoraError::ResourceLimit("testo troppo lungo".into()))?,
                ),
                Function::Year => {
                    let date =
                        NaiveDate::parse_from_str(value.get(..10).unwrap_or(&value), "%Y-%m-%d")
                            .map_err(|_| PlenoraError::Schema("year: data non valida".into()))?;
                    Scalar::Number(f64::from(date.year()))
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
                _ => {
                    return Err(PlenoraError::Internal(
                        "il ramo testo ammette solo contains/starts_with/ends_with".into(),
                    ));
                }
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
                _ => {
                    return Err(PlenoraError::Internal(
                        "il ramo numerico ammette solo abs/round".into(),
                    ));
                }
            }))
        }
        Function::Floor | Function::Ceil => {
            exact_args(&args, 1, "funzione numerica")?;
            let Some(value) = number(&args[0], "funzione numerica")? else {
                return Ok(Scalar::Null);
            };
            Ok(Scalar::Number(match name {
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
            exact_args(&args, 2, "power")?;
            let (Some(base), Some(exponent)) =
                (number(&args[0], "power")?, number(&args[1], "power")?)
            else {
                return Ok(Scalar::Null);
            };
            let value = base.powf(exponent);
            if value.is_finite() {
                Ok(Scalar::Number(value))
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
            let Some(value) = text(args[0].clone(), "substring")? else {
                return Ok(Scalar::Null);
            };
            let Some(start) = substring_index(&args[1], "substring: start")? else {
                return Ok(Scalar::Null);
            };
            let len = match args.get(2) {
                Some(arg) => match substring_index(arg, "substring: len")? {
                    Some(len) => Some(len),
                    // len null -> Null (non equivale a "fino a fine stringa").
                    None => return Ok(Scalar::Null),
                },
                None => None,
            };
            let mut chars = value.chars().skip(start);
            Ok(Scalar::Text(match len {
                Some(len) => chars.by_ref().take(len).collect(),
                None => chars.collect(),
            }))
        }
        Function::RegexReplace => {
            exact_args(&args, 3, "regex_replace")?;
            let (Some(value), Some(pattern), Some(replacement)) = (
                text(args[0].clone(), "regex_replace")?,
                text(args[1].clone(), "regex_replace")?,
                text(args[2].clone(), "regex_replace")?,
            ) else {
                return Ok(Scalar::Null);
            };
            let regex = regex::Regex::new(&pattern).map_err(|error| {
                PlenoraError::InvalidPlan(format!("regex_replace: regex non valida: {error}"))
            })?;
            Ok(Scalar::Text(
                regex.replace_all(&value, replacement.as_str()).into_owned(),
            ))
        }
        Function::Between => {
            exact_args(&args, 3, "between")?;
            // Inclusivo su entrambi gli estremi; null in qualsiasi posizione
            // -> Null (stessa tri-state dei confronti binari).
            if args.contains(&Scalar::Null) {
                return Ok(Scalar::Null);
            }
            let low = compare(args[0].clone(), args[1].clone())?;
            let high = compare(args[0].clone(), args[2].clone())?;
            Ok(match (low, high) {
                (Some(low), Some(high)) => {
                    Scalar::Boolean(low != Ordering::Less && high != Ordering::Greater)
                }
                _ => Scalar::Null,
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
                let Some(ordering) = compare(best.clone(), value.clone())? else {
                    // Null propagato come nei confronti binari.
                    return Ok(Scalar::Null);
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
        Function::DateTrunc | Function::In => Err(PlenoraError::Internal(
            "date_trunc/in sono valutati in evaluate (accesso all'AST degli argomenti)".into(),
        )),
    }
}

/// Indice di `substring`: numero >= 0 troncato verso zero (`-0.0` vale 0),
/// saturato a `usize::MAX`; null propagato dal chiamante.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn substring_index(value: &Scalar, context: &str) -> Result<Option<usize>> {
    let Some(value) = number(value, context)? else {
        return Ok(None);
    };
    if value < 0.0 {
        return Err(PlenoraError::InvalidPlan(format!("{context} negativo")));
    }
    Ok(Some(value as usize))
}

pub fn evaluate(expression: &Expression, batch: &RecordBatch, row: usize) -> Result<Scalar> {
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
        Expression::Function { name, args } => match name {
            // Nodi speciali: richiedono l'AST degli argomenti (colonna
            // temporale nativa / lista di letterali), non scalari valutati.
            Function::DateTrunc => date_trunc_generic(args, batch, row),
            Function::In => in_generic(args, batch, row),
            _ => function(
                *name,
                args.iter()
                    .map(|arg| evaluate(arg, batch, row))
                    .collect::<Result<Vec<_>>>()?,
            ),
        },
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
        return Err(PlenoraError::InvalidPlan(
            "expression supera la profondita' massima".into(),
        ));
    }
    *nodes = nodes
        .checked_add(1)
        .ok_or_else(|| PlenoraError::InvalidPlan("overflow nodi expression".into()))?;
    if *nodes > max_nodes {
        return Err(PlenoraError::InvalidPlan(
            "expression supera il numero massimo di nodi".into(),
        ));
    }
    match expression {
        Expression::Column { name } if name.trim().is_empty() => {
            Err(PlenoraError::InvalidPlan("colonna expression vuota".into()))
        }
        Expression::Literal { value } => literal(value).map(|_| ()),
        Expression::Unary { value, .. } => audit(value, depth + 1, nodes, max_nodes),
        Expression::Binary { left, right, .. } => {
            audit(left, depth + 1, nodes, max_nodes)?;
            audit(right, depth + 1, nodes, max_nodes)
        }
        Expression::Function { name, args } => {
            if args.len() > 64 {
                return Err(PlenoraError::InvalidPlan(
                    "troppi argomenti expression".into(),
                ));
            }
            // date_trunc: unita' letterale del set chiuso, rifiutata qui.
            if matches!(name, Function::DateTrunc) {
                if args.len() != 2 {
                    return Err(PlenoraError::InvalidPlan(
                        "date_trunc richiede 2 argomenti".into(),
                    ));
                }
                literal_unit(&args[0])?;
            }
            // in: il secondo argomento e' una lista di letterali scalari
            // (altrimenti `literal` rifiuterebbe l'array come non scalare).
            if matches!(name, Function::In) {
                if args.len() != 2 {
                    return Err(PlenoraError::InvalidPlan("in richiede 2 argomenti".into()));
                }
                match &args[1] {
                    Expression::Literal {
                        value: Value::Array(items),
                    } => {
                        for item in items {
                            literal(item)?;
                        }
                    }
                    _ => {
                        return Err(PlenoraError::InvalidPlan(
                            "in richiede una lista di letterali come secondo argomento".into(),
                        ));
                    }
                }
                return audit(&args[0], depth + 1, nodes, max_nodes);
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
                return Err(PlenoraError::InvalidPlan(
                    "numero rami case non valido".into(),
                ));
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

/// Validazione statica della config: nome di output e forma dell'AST, senza
/// toccare i dati.
///
/// # Errors
///
/// - `InvalidPlan`: nome colonna di output vuoto o oltre 1024 byte (come
///   `validate_output_name`); AST oltre la profondita' massima o oltre
///   `max_nodes`; nome colonna vuoto; letterale non scalare o non finito;
///   troppi argomenti o rami `case`; unita' di `date_trunc` non letterale o
///   fuori dal set chiuso; `in` senza lista di letterali scalari.
pub fn validate(config: &ExpressionTransform, max_nodes: usize) -> Result<()> {
    crate::validate_output_name(&config.output_column)?;
    audit(&config.expression, 1, &mut 0, max_nodes)
}

/// Converte uno `Scalar` in giorni Date32 per l'output `date32`.
fn scalar_date32(value: &Scalar, context: &str) -> Result<Option<i32>> {
    match value {
        Scalar::Null => Ok(None),
        Scalar::Date32(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!("{context} richiede una data"))),
    }
}

/// Converte uno `Scalar` in ms Timestamp per l'output `timestamp_ms`.
fn scalar_timestamp_ms(value: &Scalar, context: &str) -> Result<Option<i64>> {
    match value {
        Scalar::Null => Ok(None),
        Scalar::TimestampMs(value) => Ok(Some(*value)),
        _ => Err(PlenoraError::Schema(format!(
            "{context} richiede un timestamp"
        ))),
    }
}

/// Valuta l'espressione su ogni riga e appende/sostituisce la colonna di
/// output.
///
/// Su batch con righe usa il fast path compilato (stessa semantica del
/// generico, oracolo dei test); su batch vuoti valuta il percorso generico.
///
/// Il tipo della colonna prodotta e' deciso dallo SCHEMA prima di valutare
/// qualunque riga, con la stessa funzione che usa l'analizzatore del
/// contratto ([`super::static_type`]): non dipende dai valori, quindi batch
/// pieni, tutti null e vuoti con lo stesso schema producono lo stesso tipo.
///
/// # Errors
///
/// - `InvalidPlan` (in TESTA, prima di valutare, dall'analisi statica):
///   argomento di tipo errato per operatore o funzione; confronto fra tipi
///   eterogenei; tipi eterogenei con `output_type = auto`; `output_type`
///   dichiarato che l'espressione non puo' produrre; colonna di tipo non
///   valutabile (`Timestamp` non-millisecondo, tipo non convertibile in
///   testo); `date_trunc` su colonna non temporale o timestamp
///   timezone-aware, o unita' non valida; letterale non scalare o non
///   finito; numero di argomenti errato;
/// - `Schema`: colonna assente — anche su un batch VUOTO, perche' senza
///   risolverla il tipo di output non e' determinabile; divisione per zero;
///   numero non finito in colonna o risultato non finito; valore di tipo
///   diverso da quello dichiarato con `output_type` esplicito; errore Arrow
///   nella sostituzione;
/// - `InvalidPlan` (durante la valutazione): regex non valida in
///   `regex_replace`; indice di `substring` negativo; invarianti interne
///   violate (errore Internal).
pub fn expression(batch: &RecordBatch, config: &ExpressionTransform) -> Result<RecordBatch> {
    super::reject_literal_zero_divisor(&config.expression)?;
    // Il tipo della colonna prodotta si decide dallo SCHEMA, MAI dai valori
    // calcolati: e' la stessa funzione che usa l'analizzatore del contratto,
    // quindi lo schema promesso e quello prodotto non possono divergere.
    // Risolverlo dai valori significherebbe che un batch vuoto o tutto null
    // ripiega su `Utf8` anche dove il contratto dice `Boolean` o `Float64`.
    let kind = static_output_kind(batch, config)?;
    // Batch vuoto: non c'e' niente da compilare, quindi si passa dal
    // generico. Le colonne sono gia' state risolte qui sopra per decidere il
    // tipo, quindi un batch vuoto non e' piu' un caso permissivo.
    if batch.num_rows() > 0 {
        return FastProgram::compile(&config.expression, batch).run(batch, config, kind);
    }
    expression_generic_kind(batch, config, kind)
}

/// Tipo della colonna prodotta, dal solo schema del batch.
///
/// # Errors
///
/// Gli stessi di [`super::static_type::infer`] e
/// [`super::static_type::resolve_output`], con la colonna assente riportata
/// come `Schema` (l'errore che i kernel danno da sempre per una colonna che
/// non c'e').
pub(super) fn static_output_kind(
    batch: &RecordBatch,
    config: &ExpressionTransform,
) -> Result<Kind> {
    let possibili = static_type::infer("table.expression", &config.expression, &|name| {
        let index = column_index(batch, name)?;
        Ok(batch.schema_ref().field(index).data_type().clone())
    })?;
    static_type::resolve_output("table.expression", possibili, config.output_type)
}

/// Percorso generico originale: interprete ricorsivo sull'AST, usato sui
/// batch vuoti e come oracolo dei test.
#[cfg(test)]
pub fn expression_generic(
    batch: &RecordBatch,
    config: &ExpressionTransform,
) -> Result<RecordBatch> {
    expression_generic_kind(batch, config, static_output_kind(batch, config)?)
}

/// Come [`expression_generic`], col tipo di output gia' risolto.
fn expression_generic_kind(
    batch: &RecordBatch,
    config: &ExpressionTransform,
    kind: Kind,
) -> Result<RecordBatch> {
    let mut values = Vec::with_capacity(batch.num_rows());
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        match evaluate(&config.expression, batch, row) {
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
                values.push(Scalar::Null);
            }
        }
    }
    crate::reject_rows(
        &rejections,
        "valori expression rifiutati; consultare row_diagnostics",
    )?;
    match kind {
        Kind::Number => replace_or_append(
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
        Kind::Boolean => replace_or_append(
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
        Kind::Text => replace_or_append(
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
        Kind::Date32 => replace_or_append(
            batch,
            &config.output_column,
            DataType::Date32,
            true,
            Arc::new(Date32Array::from(
                values
                    .into_iter()
                    .map(|value| scalar_date32(&value, "output_type=date32"))
                    .collect::<Result<Vec<_>>>()?,
            )),
        ),
        // Timestamp timezone-aware rifiutati in ingresso: l'output e'
        // sempre Timestamp(ms) senza timezone (decisione documentata).
        Kind::TimestampMs => replace_or_append(
            batch,
            &config.output_column,
            DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
            Arc::new(TimestampMillisecondArray::from(
                values
                    .into_iter()
                    .map(|value| scalar_timestamp_ms(&value, "output_type=timestamp_ms"))
                    .collect::<Result<Vec<_>>>()?,
            )),
        ),
    }
}
