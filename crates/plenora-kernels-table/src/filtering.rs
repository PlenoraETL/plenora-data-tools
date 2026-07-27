use std::cmp::Ordering;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};
use crate::{
    column_index, compare_f64, compare_i64, compare_u64, replace_or_append, scalar_as_f64,
    scalar_as_string, select_rows, NumericBound,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operator {
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    Contains,
    Startswith,
    Endswith,
    Isnull,
    Notnull,
    Between,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub column: String,
    pub operator: Operator,
    #[serde(default)]
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    #[serde(default = "default_operator")]
    pub operator: Operator,
    #[serde(default)]
    pub value: serde_json::Value,
    #[serde(default)]
    pub result: serde_json::Value,
}

const fn default_operator() -> Operator {
    Operator::Eq
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conditional {
    pub column: String,
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub default_value: serde_json::Value,
    #[serde(default = "default_output")]
    pub output_column: String,
}

fn default_output() -> String {
    "result".into()
}

fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(v) => v.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[allow(clippy::too_many_lines)] // dispatcher esaustivo per operatore: un solo corpo tiene allineati generico e fast path
fn evaluate(
    array: &dyn Array,
    row: usize,
    operator: &Operator,
    value: &serde_json::Value,
) -> Result<bool> {
    match operator {
        Operator::Isnull => return Ok(array.is_null(row)),
        Operator::Notnull => return Ok(!array.is_null(row)),
        _ if array.is_null(row) => return Ok(false),
        _ => {}
    }
    let expected = json_text(value);
    match operator {
        Operator::Eq | Operator::Ne => {
            let equal = if array.data_type() == &DataType::Int64 {
                // Confronto esatto nativo: il letterale intero resta intero
                // (nessun collasso oltre 2^53); il misto intero<->double e'
                // esatto (vedi `NumericBound` in lib.rs).
                let bound = NumericBound::parse(&expected).ok_or_else(|| {
                    PlenoraError::Contract("confronto numerico con valore non numerico".into())
                })?;
                let values = array
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| PlenoraError::Schema("array Int64 incoerente".into()))?;
                compare_i64(values.value(row), bound) == Some(Ordering::Equal)
            } else if array.data_type() == &DataType::Float64 {
                // Semantica storica sui double (`total_cmp`, 0.0 == -0.0,
                // NaN uguale a NaN); un letterale intero oltre 2^53 usa il
                // confronto misto esatto invece dell'arrotondamento a f64.
                let bound = NumericBound::parse(&expected).ok_or_else(|| {
                    PlenoraError::Contract("confronto numerico con valore non numerico".into())
                })?;
                let values = array
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| PlenoraError::Schema("array Float64 incoerente".into()))?;
                let actual = values.value(row);
                match bound {
                    NumericBound::F64(number) => {
                        actual.total_cmp(&number) == Ordering::Equal
                            || (actual.abs().total_cmp(&0.0) == Ordering::Equal
                                && number.abs().total_cmp(&0.0) == Ordering::Equal)
                    }
                    bound => compare_f64(actual, bound) == Some(Ordering::Equal),
                }
            } else {
                scalar_as_string(array, row)?.is_some_and(|actual| actual == expected)
            };
            Ok(if matches!(operator, Operator::Ne) {
                !equal
            } else {
                equal
            })
        }
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
            let bound = NumericBound::parse(&expected).ok_or_else(|| {
                PlenoraError::Contract("confronto ordinato richiede un valore numerico".into())
            })?;
            // Int64/UInt64 nativi esatti; Float64 in misto esatto contro i
            // letterali interi; gli altri tipi numerici (Decimal128, Date32,
            // Timestamp(ms), Utf8 numerico) restano sul profilo f64 storico.
            let ordering = if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                compare_i64(values.value(row), bound)
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                compare_u64(values.value(row), bound)
            } else if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
                compare_f64(values.value(row), bound)
            } else {
                scalar_as_f64(array, row)?
                    .ok_or_else(|| PlenoraError::Schema("valore nullo inatteso".into()))?
                    .partial_cmp(&bound_as_f64(bound))
            };
            Ok(ordered_typed(ordering, operator))
        }
        Operator::Contains | Operator::Startswith | Operator::Endswith => {
            let actual = scalar_as_string(array, row)?.unwrap_or_default();
            Ok(match operator {
                Operator::Contains => actual.to_lowercase().contains(&expected.to_lowercase()),
                Operator::Startswith => actual.starts_with(&expected),
                Operator::Endswith => actual.ends_with(&expected),
                _ => unreachable!(),
            })
        }
        Operator::Between => {
            let (low, high) = expected
                .split_once(',')
                .ok_or_else(|| PlenoraError::Contract("between richiede min,max".into()))?;
            let low = NumericBound::parse(low.trim())
                .ok_or_else(|| PlenoraError::Contract("min between non valido".into()))?;
            let high = NumericBound::parse(high.trim())
                .ok_or_else(|| PlenoraError::Contract("max between non valido".into()))?;
            let within = if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                within_bounds(
                    compare_i64(values.value(row), low),
                    compare_i64(values.value(row), high),
                )
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                within_bounds(
                    compare_u64(values.value(row), low),
                    compare_u64(values.value(row), high),
                )
            } else if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
                within_bounds(
                    compare_f64(values.value(row), low),
                    compare_f64(values.value(row), high),
                )
            } else {
                let actual = scalar_as_f64(array, row)?
                    .ok_or_else(|| PlenoraError::Schema("valore nullo inatteso".into()))?;
                actual >= bound_as_f64(low) && actual <= bound_as_f64(high)
            };
            Ok(within)
        }
        Operator::Isnull | Operator::Notnull => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// Fast path tipizzati (ottimizzazione kernel `table.filter`, Fase post-2A).
//
// Per i tipi Arrow principali (Int64, UInt64, Float64, Utf8, Boolean) il
// confronto avviene sui valori nativi, senza conversione scalare per riga ne'
// allocazioni; la semantica e' IDENTICA a `evaluate` (null handling, NaN,
// -0.0 vs 0.0, confronto esatto nativo per Int64/UInt64 — nessun collasso
// oltre 2^53 — confronto misto intero<->double via `NumericBound`, confronto
// testuale per UInt64/Boolean in `==`/`!=`, ordine righe). `fast_rows`
// restituisce `None` quando tipo/operatore non sono coperti o quando il
// valore di confronto non e' canonico: il chiamante ricade sul percorso
// generico riga-per-riga, che riproduce esattamente lo stesso comportamento
// (errori di contratto inclusi).
// ---------------------------------------------------------------------------

/// Righe non nulle di `array` per cui `pred` e' vera, in ordine crescente.
fn rows_where<A: Array>(array: &A, mut pred: impl FnMut(usize) -> bool) -> Vec<usize> {
    (0..array.len())
        .filter(|row| !array.is_null(*row) && pred(*row))
        .collect()
}

/// Uguaglianza numerica di `evaluate` sui double: `total_cmp`, con
/// `0.0 == -0.0` (i valori assoluti nulli sono considerati uguali) e NaN
/// uguale a NaN.
fn numeric_eq(actual: f64, expected: f64) -> bool {
    actual.total_cmp(&expected) == Ordering::Equal || (actual == 0.0 && expected == 0.0)
}

/// Confronto ordinato condiviso (`>`/`>=`/`<`/`<=`): `None` (NaN) rende falso
/// ogni confronto, come nell'IEEE 754.
const fn ordered_typed(ordering: Option<Ordering>, operator: &Operator) -> bool {
    match operator {
        Operator::Gt => matches!(ordering, Some(Ordering::Greater)),
        Operator::Ge => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        Operator::Lt => matches!(ordering, Some(Ordering::Less)),
        Operator::Le => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        _ => unreachable!(),
    }
}

/// `between`: il valore non e' sotto il minimo ne' sopra il massimo; `None`
/// (estremo o valore NaN) esclude la riga, come `>=`/`<=` IEEE.
const fn within_bounds(low: Option<Ordering>, high: Option<Ordering>) -> bool {
    !matches!(low, None | Some(Ordering::Less))
        && !matches!(high, None | Some(Ordering::Greater))
}

/// Forma f64 di un bound, per i tipi rimasti sul profilo f64 storico
/// (Decimal128, Date32, Timestamp(ms), Utf8 numerico): stesso valore del
/// parse f64 diretto del percorso originale.
#[allow(clippy::cast_precision_loss)] // qui la conversione a f64 e' voluta: profilo storico
const fn bound_as_f64(bound: NumericBound) -> f64 {
    match bound {
        NumericBound::I64(value) => value as f64,
        NumericBound::U64(value) => value as f64,
        NumericBound::F64(value) => value,
    }
}

#[allow(clippy::too_many_lines)] // specchio di `evaluate`: la simmetria riga a riga e' la garanzia di parita'
fn fast_rows(array: &ArrayRef, operator: &Operator, value: &serde_json::Value) -> Option<Result<Vec<usize>>> {
    let rows = match operator {
        Operator::Isnull => return Some(Ok((0..array.len()).filter(|r| array.is_null(*r)).collect())),
        Operator::Notnull => return Some(Ok((0..array.len()).filter(|r| !array.is_null(*r)).collect())),
        Operator::Eq | Operator::Ne => {
            let negate = matches!(operator, Operator::Ne);
            if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                // Il ramo numerico del generico fallisce il parse solo su
                // righe non nulle: in caso di valore non numerico si ricade
                // sul generico, che riproduce errore o selezione vuota.
                let bound = NumericBound::parse(&json_text(value))?;
                rows_where(values, |row| {
                    (compare_i64(values.value(row), bound) == Some(Ordering::Equal)) != negate
                })
            } else if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
                let bound = NumericBound::parse(&json_text(value))?;
                rows_where(values, |row| {
                    let equal = match bound {
                        NumericBound::F64(number) => numeric_eq(values.value(row), number),
                        bound => compare_f64(values.value(row), bound) == Some(Ordering::Equal),
                    };
                    equal != negate
                })
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                // Il generico confronta UInt64 come stringa (`to_string`):
                // il fast path vale solo per la forma decimale canonica.
                let expected = json_text(value);
                let parsed = expected.parse::<u64>().ok()?;
                if parsed.to_string() != expected {
                    return None;
                }
                rows_where(values, |row| (values.value(row) == parsed) != negate)
            } else if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
                let expected = json_text(value);
                rows_where(values, |row| (values.value(row) == expected) != negate)
            } else if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
                // Il generico confronta "true"/"false": equivalente al
                // confronto nativo sui soli valori booleani possibili.
                let expected = json_text(value);
                rows_where(values, |row| {
                    ((expected == "true" && values.value(row))
                        || (expected == "false" && !values.value(row)))
                        != negate
                })
            } else {
                return None;
            }
        }
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
            let bound = NumericBound::parse(&json_text(value))?;
            if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                rows_where(values, |row| {
                    ordered_typed(compare_i64(values.value(row), bound), operator)
                })
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                rows_where(values, |row| {
                    ordered_typed(compare_u64(values.value(row), bound), operator)
                })
            } else if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
                rows_where(values, |row| {
                    ordered_typed(compare_f64(values.value(row), bound), operator)
                })
            } else {
                return None;
            }
        }
        Operator::Between => {
            let expected = json_text(value);
            let (low, high) = expected.split_once(',')?;
            let low = NumericBound::parse(low.trim())?;
            let high = NumericBound::parse(high.trim())?;
            if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                rows_where(values, |row| {
                    within_bounds(
                        compare_i64(values.value(row), low),
                        compare_i64(values.value(row), high),
                    )
                })
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                rows_where(values, |row| {
                    within_bounds(
                        compare_u64(values.value(row), low),
                        compare_u64(values.value(row), high),
                    )
                })
            } else if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
                rows_where(values, |row| {
                    within_bounds(
                        compare_f64(values.value(row), low),
                        compare_f64(values.value(row), high),
                    )
                })
            } else {
                return None;
            }
        }
        Operator::Contains | Operator::Startswith | Operator::Endswith => {
            let Some(values) = array.as_any().downcast_ref::<StringArray>() else {
                return None;
            };
            let expected = json_text(value);
            match operator {
                Operator::Contains => {
                    let needle = expected.to_lowercase();
                    rows_where(values, |row| values.value(row).to_lowercase().contains(&needle))
                }
                Operator::Startswith => {
                    rows_where(values, |row| values.value(row).starts_with(&expected))
                }
                Operator::Endswith => {
                    rows_where(values, |row| values.value(row).ends_with(&expected))
                }
                _ => unreachable!("solo operatori testuali"),
            }
        }
    };
    Some(Ok(rows))
}

pub fn filter(batch: &RecordBatch, config: &Filter) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let array = batch.column(index);
    let rows = match fast_rows(array, &config.operator, &config.value) {
        Some(result) => result?,
        None => (0..batch.num_rows())
            .filter_map(
                |row| match evaluate(array.as_ref(), row, &config.operator, &config.value) {
                    Ok(true) => Some(Ok(row)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .collect::<Result<Vec<_>>>()?,
    };
    select_rows(batch, &rows)
}

pub fn conditional(batch: &RecordBatch, config: &Conditional) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let values = (0..batch.num_rows())
        .map(|row| {
            for condition in &config.conditions {
                if evaluate(source.as_ref(), row, &condition.operator, &condition.value)? {
                    return Ok(json_text(&condition.result));
                }
            }
            Ok(json_text(&config.default_value))
        })
        .collect::<Result<Vec<_>>>()?;
    let numeric = config
        .conditions
        .iter()
        .map(|c| json_text(&c.result))
        .chain(std::iter::once(json_text(&config.default_value)))
        .all(|v| v.is_empty() || v.replace(',', ".").parse::<f64>().is_ok());
    if numeric {
        let out = values
            .into_iter()
            .map(|v| {
                if v.is_empty() {
                    None
                } else {
                    v.replace(',', ".").parse().ok()
                }
            })
            .collect::<Vec<Option<f64>>>();
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(out)),
        )
    } else {
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            false,
            Arc::new(StringArray::from(values)),
        )
    }
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{
        types::Int32Type, DictionaryArray, LargeStringArray, UInt64Array,
    };
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::json;

    use super::*;

    /// Percorso generico pre-ottimizzazione: riferimento per l'equivalenza
    /// semantica del fast path.
    fn generic_filter(batch: &RecordBatch, config: &Filter) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let array = batch.column(index);
        let rows = (0..batch.num_rows())
            .filter_map(
                |row| match evaluate(array.as_ref(), row, &config.operator, &config.value) {
                    Ok(true) => Some(Ok(row)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .collect::<Result<Vec<_>>>()?;
        select_rows(batch, &rows)
    }

    fn single_column_batch(column: ArrayRef, data_type: DataType, nullable: bool) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("c", data_type, nullable)])),
            vec![column],
        )
        .expect("fixture")
    }

    fn config(operator: Operator, value: serde_json::Value) -> Filter {
        Filter {
            column: "c".into(),
            operator,
            value,
        }
    }

    /// Equivalenza fast path / generico (risultato o errore) su una matrice
    /// di operatori e valori.
    fn assert_equivalent(batch: &RecordBatch, operator: Operator, value: serde_json::Value) {
        let config = config(operator, value);
        let fast = filter(batch, &config);
        let generic = generic_filter(batch, &config);
        match (fast, generic) {
            (Ok(fast), Ok(generic)) => assert_eq!(fast, generic),
            (fast, generic) => assert_eq!(fast.is_err(), generic.is_err()),
        }
    }

    #[test]
    fn fast_path_matches_generic_on_float64_edge_values() {
        let batch = single_column_batch(
            Arc::new(Float64Array::from(vec![
                Some(1.0),
                Some(f64::NAN),
                Some(-0.0),
                Some(0.0),
                None,
                Some(-1.5),
                Some(f64::INFINITY),
            ])),
            DataType::Float64,
            true,
        );
        // NaN e' uguale a NaN (total_cmp) e -0.0 == 0.0: la matrice fissa la
        // semantica esistente, inclusi i casi limite.
        for value in [json!(0.0), json!(-0.0), json!("NaN"), json!(1.0), json!(-1.5)] {
            assert_equivalent(&batch, Operator::Eq, value.clone());
            assert_equivalent(&batch, Operator::Ne, value);
        }
        for value in [json!(0.0), json!(-1.5), json!("NaN")] {
            assert_equivalent(&batch, Operator::Gt, value.clone());
            assert_equivalent(&batch, Operator::Ge, value.clone());
            assert_equivalent(&batch, Operator::Lt, value.clone());
            assert_equivalent(&batch, Operator::Le, value);
        }
        assert_equivalent(&batch, Operator::Between, json!("-1,1"));
        assert_equivalent(&batch, Operator::Isnull, json!(null));
        assert_equivalent(&batch, Operator::Notnull, json!(null));
        // Valore non numerico: il generico fallisce sulle righe non nulle.
        assert!(filter(&batch, &config(Operator::Eq, json!("x"))).is_err());
        assert_equivalent(&batch, Operator::Eq, json!("x"));
    }

    #[test]
    fn signed_zero_equality_and_nan_match_are_exact() {
        let batch = single_column_batch(
            Arc::new(Float64Array::from(vec![
                Some(-0.0),
                Some(f64::NAN),
                Some(2.0),
            ])),
            DataType::Float64,
            true,
        );
        let zero = filter(&batch, &config(Operator::Eq, json!(0.0))).expect("eq 0");
        assert_eq!(zero.num_rows(), 1); // -0.0 == 0.0
        let nan = filter(&batch, &config(Operator::Eq, json!("NaN"))).expect("eq NaN");
        assert_eq!(nan.num_rows(), 1); // NaN uguale a NaN (total_cmp)
        let gt_zero = filter(&batch, &config(Operator::Gt, json!(0.0))).expect("gt 0");
        assert_eq!(gt_zero.num_rows(), 1); // solo 2.0: -0.0 e NaN esclusi
    }

    #[test]
    fn fast_path_matches_generic_on_int64_and_uint64() {
        let ints = single_column_batch(
            Arc::new(Int64Array::from(vec![
                Some(i64::MIN),
                Some(-1),
                Some(0),
                None,
                Some(9_007_199_254_740_993), // oltre 2^53: confronto esatto, nessun rounding
                Some(i64::MAX),
            ])),
            DataType::Int64,
            true,
        );
        for value in [json!(0), json!(-1), json!(9_007_199_254_740_992_i64)] {
            assert_equivalent(&ints, Operator::Eq, value.clone());
            assert_equivalent(&ints, Operator::Ne, value.clone());
            assert_equivalent(&ints, Operator::Gt, value.clone());
            assert_equivalent(&ints, Operator::Le, value);
        }
        assert_equivalent(&ints, Operator::Between, json!("-10, 10"));

        let uints = single_column_batch(
            Arc::new(UInt64Array::from(vec![Some(10), Some(9), None, Some(42)])),
            DataType::UInt64,
            true,
        );
        // == canonico: numerico; forme non canoniche ricadono sul generico.
        assert_equivalent(&uints, Operator::Eq, json!(42));
        assert_equivalent(&uints, Operator::Eq, json!("42"));
        assert_equivalent(&uints, Operator::Eq, json!("042"));
        assert_equivalent(&uints, Operator::Ne, json!(9));
        // Ordinati: numerici anche su UInt64 (ramo f64 del generico).
        assert_equivalent(&uints, Operator::Gt, json!(9));
        assert_equivalent(&uints, Operator::Le, json!(10));
    }

    #[test]
    fn fast_path_matches_generic_on_utf8_and_boolean() {
        let strings = single_column_batch(
            Arc::new(StringArray::from(vec![
                Some("Alpha"),
                Some("beta"),
                None,
                Some("alphabet"),
                Some("BETA"),
            ])),
            DataType::Utf8,
            true,
        );
        assert_equivalent(&strings, Operator::Eq, json!("beta"));
        assert_equivalent(&strings, Operator::Ne, json!("beta"));
        assert_equivalent(&strings, Operator::Contains, json!("ALPH"));
        assert_equivalent(&strings, Operator::Startswith, json!("Al"));
        assert_equivalent(&strings, Operator::Endswith, json!("TA"));

        let booleans = single_column_batch(
            Arc::new(BooleanArray::from(vec![Some(true), Some(false), None])),
            DataType::Boolean,
            true,
        );
        assert_equivalent(&booleans, Operator::Eq, json!(true));
        assert_equivalent(&booleans, Operator::Eq, json!("false"));
        assert_equivalent(&booleans, Operator::Ne, json!(false));
        assert_equivalent(&booleans, Operator::Eq, json!("x"));
    }

    #[test]
    fn null_handling_is_identical_across_operators() {
        let batch = single_column_batch(
            Arc::new(Int64Array::from(vec![None, Some(1), None])),
            DataType::Int64,
            true,
        );
        let nulls = filter(&batch, &config(Operator::Isnull, json!(null))).expect("isnull");
        assert_eq!(nulls.num_rows(), 2);
        let not_nulls = filter(&batch, &config(Operator::Notnull, json!(null))).expect("notnull");
        assert_eq!(not_nulls.num_rows(), 1);
        // I null sono esclusi da ogni altro operatore, != compreso.
        let ne = filter(&batch, &config(Operator::Ne, json!(7))).expect("ne");
        assert_eq!(ne.num_rows(), 1);
    }

    #[test]
    fn dictionary_column_uses_generic_path_with_same_results() {
        let keys =
            plenora_core::arrow::array::Int32Array::from(vec![Some(0), Some(1), None, Some(0)]);
        let values = StringArray::from(vec!["a", "b"]);
        let dictionary = DictionaryArray::<Int32Type>::new(keys, Arc::new(values));
        let batch = single_column_batch(
            Arc::new(dictionary),
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        );
        let eq = filter(&batch, &config(Operator::Eq, json!("a"))).expect("dictionary eq");
        assert_eq!(eq.num_rows(), 2);
        assert_equivalent(&batch, Operator::Eq, json!("a"));
        assert_equivalent(&batch, Operator::Ne, json!("a"));
        assert_equivalent(&batch, Operator::Contains, json!("a"));
    }

    #[test]
    fn large_utf8_keeps_the_generic_error_on_comparison() {
        let batch = single_column_batch(
            Arc::new(LargeStringArray::from(vec![Some("a"), None])),
            DataType::LargeUtf8,
            true,
        );
        // LargeUtf8 non e' nel profilo scalare: il confronto resta un errore
        // di schema (semantica invariata), isnull/notnull funzionano.
        assert!(filter(&batch, &config(Operator::Eq, json!("a"))).is_err());
        assert_equivalent(&batch, Operator::Eq, json!("a"));
        let nulls = filter(&batch, &config(Operator::Isnull, json!(null))).expect("isnull");
        assert_eq!(nulls.num_rows(), 1);
    }

    #[test]
    fn empty_and_single_row_inputs() {
        let empty = single_column_batch(
            Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
            DataType::Int64,
            true,
        );
        assert_eq!(
            filter(&empty, &config(Operator::Eq, json!(1)))
                .expect("empty")
                .num_rows(),
            0
        );
        let single = single_column_batch(
            Arc::new(Int64Array::from(vec![Some(1)])),
            DataType::Int64,
            true,
        );
        assert_eq!(
            filter(&single, &config(Operator::Eq, json!(1)))
                .expect("single")
                .num_rows(),
            1
        );
    }

    #[test]
    fn int64_comparisons_are_exact_beyond_2_pow_53() {
        // Classe "confronti via f64": 2^53 e 2^53+1 collassano sullo stesso
        // double; eq/ordine/between devono restare esatti (fast e generico).
        let batch = single_column_batch(
            Arc::new(Int64Array::from(vec![
                Some(9_007_199_254_740_992), // 2^53
                Some(9_007_199_254_740_993), // 2^53 + 1
                None,
            ])),
            DataType::Int64,
            true,
        );
        let eq_hi = filter(&batch, &config(Operator::Eq, json!(9_007_199_254_740_993_i64)))
            .expect("eq 2^53+1");
        assert_eq!(eq_hi.num_rows(), 1);
        let eq_lo = filter(&batch, &config(Operator::Eq, json!("9007199254740992")))
            .expect("eq 2^53");
        assert_eq!(eq_lo.num_rows(), 1);
        // Il bound double 9007199254740992.0 e' minore dell'intero 2^53+1.
        let gt = filter(&batch, &config(Operator::Gt, json!(9_007_199_254_740_992.0)))
            .expect("gt 2^53.0");
        assert_eq!(gt.num_rows(), 1);
        let lt = filter(&batch, &config(Operator::Lt, json!(9_007_199_254_740_993_i64)))
            .expect("lt 2^53+1");
        assert_eq!(lt.num_rows(), 1);
        let between = filter(&batch, &config(Operator::Between, json!("9007199254740993, 9007199254740993")))
            .expect("between esatto");
        assert_eq!(between.num_rows(), 1);
        // Parita' fast/generico su tutta la matrice di questi valori.
        for value in [
            json!(9_007_199_254_740_992_i64),
            json!(9_007_199_254_740_993_i64),
            json!("9007199254740993"),
            json!(9_007_199_254_740_992.0),
        ] {
            assert_equivalent(&batch, Operator::Eq, value.clone());
            assert_equivalent(&batch, Operator::Ne, value.clone());
            assert_equivalent(&batch, Operator::Gt, value.clone());
            assert_equivalent(&batch, Operator::Ge, value.clone());
            assert_equivalent(&batch, Operator::Lt, value.clone());
            assert_equivalent(&batch, Operator::Le, value);
        }
        assert_equivalent(&batch, Operator::Between, json!("9007199254740992,9007199254740993"));
    }

    #[test]
    fn uint64_ordered_comparisons_are_native_not_textual() {
        // Ordine numerico (9 < 10), non testuale ("10" < "9"), e nessun
        // collasso oltre 2^53: u64::MAX-1 e u64::MAX sono lo stesso double.
        let batch = single_column_batch(
            Arc::new(UInt64Array::from(vec![
                Some(10),
                Some(9),
                Some(u64::MAX - 1),
                Some(u64::MAX),
                None,
            ])),
            DataType::UInt64,
            true,
        );
        let gt = filter(&batch, &config(Operator::Gt, json!(9))).expect("gt 9");
        assert_eq!(gt.num_rows(), 3);
        let le = filter(&batch, &config(Operator::Le, json!(10))).expect("le 10");
        assert_eq!(le.num_rows(), 2);
        let top = filter(
            &batch,
            &config(Operator::Eq, json!("18446744073709551615")),
        )
        .expect("eq u64::MAX");
        assert_eq!(top.num_rows(), 1);
        let gt_max_minus_one = filter(
            &batch,
            &config(Operator::Gt, json!("18446744073709551614")),
        )
        .expect("gt u64::MAX-1");
        assert_eq!(gt_max_minus_one.num_rows(), 1);
        let between = filter(
            &batch,
            &config(Operator::Between, json!("18446744073709551614,18446744073709551615")),
        )
        .expect("between u64 top");
        assert_eq!(between.num_rows(), 2);
        for value in [json!(9), json!(10), json!("18446744073709551614")] {
            assert_equivalent(&batch, Operator::Gt, value.clone());
            assert_equivalent(&batch, Operator::Ge, value.clone());
            assert_equivalent(&batch, Operator::Lt, value.clone());
            assert_equivalent(&batch, Operator::Le, value);
        }
        assert_equivalent(&batch, Operator::Between, json!("9,10"));
        assert_equivalent(
            &batch,
            Operator::Between,
            json!("18446744073709551614,18446744073709551615"),
        );
    }

    #[test]
    fn float64_column_compares_exactly_against_integer_literals() {
        // Letterale intero oltre 2^53 contro colonna Float64: il double
        // 9007199254740992.0 NON e' uguale all'intero 9007199254740993.
        let batch = single_column_batch(
            Arc::new(Float64Array::from(vec![Some(9_007_199_254_740_992.0), None])),
            DataType::Float64,
            true,
        );
        let eq = filter(&batch, &config(Operator::Eq, json!(9_007_199_254_740_993_i64)))
            .expect("eq intero oltre 2^53");
        assert_eq!(eq.num_rows(), 0);
        let lt = filter(&batch, &config(Operator::Lt, json!(9_007_199_254_740_993_i64)))
            .expect("lt intero oltre 2^53");
        assert_eq!(lt.num_rows(), 1);
        assert_equivalent(&batch, Operator::Eq, json!(9_007_199_254_740_993_i64));
        assert_equivalent(&batch, Operator::Ne, json!(9_007_199_254_740_993_i64));
        assert_equivalent(&batch, Operator::Le, json!(9_007_199_254_740_993_i64));
        assert_equivalent(&batch, Operator::Gt, json!(9_007_199_254_740_993_i64));
    }

    #[test]
    fn mixed_columns_and_row_order_are_preserved() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("c", DataType::Float64, true),
                Field::new("label", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![10, 20, 30, 40])),
                Arc::new(Float64Array::from(vec![
                    Some(2.0),
                    None,
                    Some(1.0),
                    Some(3.0),
                ])),
                Arc::new(StringArray::from(vec!["a", "b", "c", "d"])),
            ],
        )
        .expect("fixture");
        let config = Filter {
            column: "c".into(),
            operator: Operator::Gt,
            value: json!(1.5),
        };
        let fast = filter(&batch, &config).expect("fast");
        let generic = generic_filter(&batch, &config).expect("generic");
        assert_eq!(fast, generic);
        let ids = fast
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("ids");
        // Ordine originale delle righe selezionate: 10, 40 (riga null esclusa).
        assert_eq!(ids.values(), &[10, 40]);
    }
}
