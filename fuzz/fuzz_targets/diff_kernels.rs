#![no_main]

//! Differenziale fast-vs-oracolo sui kernel ottimizzati (Fase post-2A).
//!
//! Gli entry point del fast path con flag (`join_impl(fast)`,
//! `membership_impl(fast)`, `fast_rows`, i fast path di aggregate/dates/
//! strings) sono privati: dall'esterno il differenziale possibile e'
//! fast-path-vs-oracolo, dove l'oracolo e' la riproduzione letterale del
//! percorso generico (che gli stessi test del crate usano come riferimento):
//!
//! - `table.filter`: il kernel pubblico `filtering::filter` usa `fast_rows`
//!   sui tipi coperti (Int64, UInt64, Float64, Utf8, Boolean) e ricade sul
//!   generico altrove; l'oracolo qui sotto replica `evaluate` riga per riga
//!   (stessa semantica: null, NaN, -0.0 vs 0.0, confronto testuale). Gli
//!   output devono essere fisicamente identici (stesse righe via `take`).
//! - `coalesce`: `cleansing::coalesce_fast` (pubblico) contro l'oracolo
//!   `arrow_select::coalesce::coalesce` (stessa semantica "primo non-null").

use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_core::PlenoraError;
use plenora_kernels_table::cleansing::coalesce_fast;
use plenora_kernels_table::filtering::{filter, Filter, Operator};
use plenora_kernels_table::{scalar_as_f64, scalar_as_string, select_rows};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Generatori deterministici dal payload
// ---------------------------------------------------------------------------

fn edge_float(byte: u8) -> f64 {
    match byte % 8 {
        0 => 0.0,
        1 => -0.0,
        2 => f64::NAN,
        3 => f64::INFINITY,
        4 => f64::NEG_INFINITY,
        5 => f64::from(byte) * 0.5,
        6 => -f64::from(byte),
        _ => f64::from(byte),
    }
}

fn edge_string(index: usize, chunk: &[u8]) -> String {
    match index % 9 {
        0 => "true".to_owned(),
        1 => "false".to_owned(),
        2 => "0".to_owned(),
        3 => "-0.0".to_owned(),
        4 => "NaN".to_owned(),
        5 => "🙂é\u{0}".to_owned(),
        _ => String::from_utf8_lossy(chunk).into_owned(),
    }
}

/// Colonna tipizzata dal payload; `variant` cambia pattern di null e valori
/// mantenendo identica la lunghezza (stesso payload => stesso n. righe).
fn typed_column(kind: u8, payload: &[u8], variant: usize) -> ArrayRef {
    let rows = payload.len().min(96);
    let null_period = 3 + (kind as usize + variant) % 5;
    match kind % 5 {
        0 => Arc::new(Int64Array::from(
            (0..rows)
                .map(|row| (row % null_period != 0).then(|| i64::from(payload[row]) - 128))
                .collect::<Vec<_>>(),
        )) as ArrayRef,
        1 => Arc::new(UInt64Array::from(
            (0..rows)
                .map(|row| (row % null_period != 0).then(|| u64::from(payload[row])))
                .collect::<Vec<_>>(),
        )) as ArrayRef,
        2 => Arc::new(Float64Array::from(
            (0..rows)
                .map(|row| {
                    (row % null_period != 0).then(|| edge_float(payload[row] + variant as u8))
                })
                .collect::<Vec<_>>(),
        )) as ArrayRef,
        3 => Arc::new(BooleanArray::from(
            (0..rows)
                .map(|row| (row % null_period != 0).then_some((payload[row] + variant as u8) % 2 == 0))
                .collect::<Vec<_>>(),
        )) as ArrayRef,
        _ => Arc::new(StringArray::from(
            payload
                .chunks(8)
                .take(96)
                .enumerate()
                .map(|(index, chunk)| {
                    (index % null_period != 0).then(|| edge_string(index + variant, chunk))
                })
                .collect::<Vec<_>>(),
        )) as ArrayRef,
    }
}

fn operator(byte: u8) -> Operator {
    match byte % 12 {
        0 => Operator::Eq,
        1 => Operator::Ne,
        2 => Operator::Gt,
        3 => Operator::Ge,
        4 => Operator::Lt,
        5 => Operator::Le,
        6 => Operator::Contains,
        7 => Operator::Startswith,
        8 => Operator::Endswith,
        9 => Operator::Isnull,
        10 => Operator::Notnull,
        _ => Operator::Between,
    }
}

fn filter_value(op: &Operator, payload: &[u8]) -> Value {
    let numeric = [
        "0", "-0.0", "0.0", "NaN", "nan", "inf", "-inf", "1e3", "3.", ".5", "+3", "",
        "18446744073709551615", "9223372036854775807", "-9223372036854775808",
    ];
    let pick = payload.first().copied().unwrap_or_default() as usize;
    match op {
        Operator::Isnull | Operator::Notnull => Value::Null,
        Operator::Between => Value::String(format!(
            "{},{}",
            numeric[pick % numeric.len()],
            numeric[(pick / 3 + 1) % numeric.len()]
        )),
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le | Operator::Eq
        | Operator::Ne => match pick % 4 {
            // Forme testuali (incluse non canoniche) e numeri JSON.
            0 => Value::String(numeric[(pick / 4) % numeric.len()].to_owned()),
            1 => Value::from(i64::from(payload.get(1).copied().unwrap_or_default()) - 128),
            2 => Value::from(f64::from(payload.get(1).copied().unwrap_or_default()) / 2.0),
            _ => Value::String(String::from_utf8_lossy(payload).into_owned()),
        },
        Operator::Contains | Operator::Startswith | Operator::Endswith => {
            Value::String(edge_string(pick, &payload[..payload.len().min(8)]))
        }
    }
}

// ---------------------------------------------------------------------------
// Oracolo: replica letterale del percorso generico di `filtering::evaluate`
// (riga per riga, scalari convertiti) — e' il riferimento dei fast path.
// ---------------------------------------------------------------------------

fn json_text(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn oracle_evaluate(
    array: &dyn Array,
    row: usize,
    operator: &Operator,
    value: &Value,
) -> Result<bool, PlenoraError> {
    match operator {
        Operator::Isnull => return Ok(array.is_null(row)),
        Operator::Notnull => return Ok(!array.is_null(row)),
        _ if array.is_null(row) => return Ok(false),
        _ => {}
    }
    let expected = json_text(value);
    match operator {
        Operator::Eq | Operator::Ne => {
            let equal = if matches!(array.data_type(), DataType::Int64 | DataType::Float64) {
                let number = expected.parse::<f64>().map_err(|_| {
                    PlenoraError::Contract("confronto numerico con valore non numerico".into())
                })?;
                scalar_as_f64(array, row)?.is_some_and(|actual| {
                    actual.total_cmp(&number) == std::cmp::Ordering::Equal
                        || (actual.abs().total_cmp(&0.0) == std::cmp::Ordering::Equal
                            && number.abs().total_cmp(&0.0) == std::cmp::Ordering::Equal)
                })
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
            let expected = expected.parse::<f64>().map_err(|_| {
                PlenoraError::Contract("confronto ordinato richiede un valore numerico".into())
            })?;
            let actual = scalar_as_f64(array, row)?
                .ok_or_else(|| PlenoraError::Schema("valore nullo inatteso".into()))?;
            Ok(match operator {
                Operator::Gt => actual > expected,
                Operator::Ge => actual >= expected,
                Operator::Lt => actual < expected,
                Operator::Le => actual <= expected,
                _ => unreachable!(),
            })
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
            let low: f64 = low
                .trim()
                .parse()
                .map_err(|_| PlenoraError::Contract("min between non valido".into()))?;
            let high: f64 = high
                .trim()
                .parse()
                .map_err(|_| PlenoraError::Contract("max between non valido".into()))?;
            let actual = scalar_as_f64(array, row)?
                .ok_or_else(|| PlenoraError::Schema("valore nullo inatteso".into()))?;
            Ok(actual >= low && actual <= high)
        }
        Operator::Isnull | Operator::Notnull => unreachable!(),
    }
}

fn oracle_filter(batch: &RecordBatch, config: &Filter) -> Result<RecordBatch, PlenoraError> {
    let array = batch.column(0).clone();
    let mut rows = Vec::new();
    for row in 0..batch.num_rows() {
        if oracle_evaluate(array.as_ref(), row, &config.operator, &config.value)? {
            rows.push(row);
        }
    }
    select_rows(batch, &rows)
}

fn assert_identical(kernel: &RecordBatch, oracle: &RecordBatch, context: &str) {
    assert_eq!(
        kernel.num_rows(),
        oracle.num_rows(),
        "{context}: righe diverse (kernel {} vs oracolo {})",
        kernel.num_rows(),
        oracle.num_rows()
    );
    for index in 0..kernel.num_columns() {
        assert!(
            kernel.column(index).to_data() == oracle.column(index).to_data(),
            "{context}: colonna {index} diversa"
        );
    }
}

fn diff_filter(selector: u8, payload: &[u8]) {
    let column = typed_column(selector, payload);
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "c",
            column.data_type().clone(),
            true,
        )])),
        vec![column],
    )
    .expect("fixture");
    let op = operator(payload.get(1).copied().unwrap_or_default());
    let config = Filter {
        column: "c".into(),
        value: filter_value(&op, &payload[2..]),
        operator: op,
    };
    let kernel = filter(&batch, &config);
    let oracle = oracle_filter(&batch, &config);
    match (kernel, oracle) {
        (Ok(kernel), Ok(oracle)) => assert_identical(&kernel, &oracle, "filter"),
        (Err(_), Err(_)) => {}
        (kernel, oracle) => panic!(
            "filter: esito diverso kernel/oracolo (kernel {}, oracolo {})",
            kernel.is_ok(),
            oracle.is_ok()
        ),
    }
}

fn diff_coalesce(selector: u8, payload: &[u8]) {
    let columns = 1 + payload.first().copied().unwrap_or_default() as usize % 3;
    let arrays: Vec<ArrayRef> = (0..columns)
        .map(|n| typed_column(selector, &payload[n..]))
        .collect();
    let fields: Vec<Field> = (0..columns)
        .map(|n| Field::new(format!("c{n}"), arrays[n].data_type().clone(), true))
        .collect();
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).expect("fixture");
    let indices: Vec<usize> = (0..columns).collect();
    let Some(fast) = coalesce_fast(&batch, &indices) else {
        return; // tipo non coperto dal fast path: niente differenziale
    };
    let refs: Vec<&dyn Array> = batch.columns().iter().map(|c| c.as_ref()).collect();
    let oracle = arrow_select::coalesce::coalesce(&refs).expect("oracolo coalesce");
    assert!(
        fast.to_data() == oracle.to_data(),
        "coalesce: fast path diverso dall'oracolo"
    );
}

fuzz_target!(|payload: &[u8]| {
    if payload.len() < 3 {
        return;
    }
    let selector = payload[0];
    match selector % 3 {
        0 | 1 => diff_filter(selector / 3, &payload[1..]),
        _ => diff_coalesce(selector / 3, &payload[1..]),
    }
});
