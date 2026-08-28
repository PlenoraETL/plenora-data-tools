#![no_main]

//! Differenziale fast-vs-oracolo sui kernel ottimizzati.
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
//!   (stessa semantica: null, NaN, -0.0 vs 0.0, confronti interi ESATTI
//!   nativi — i generatori coprono valori oltre 2^53 e u64 grandi, dove la
//!   conversione a f64 collasserebbe valori distinti — confronto testuale
//!   per i tipi non numerici). Gli output devono essere fisicamente
//!   identici (stesse righe via `take`).
//! - `coalesce`: `cleansing::coalesce_fast` (pubblico) contro l'oracolo
//!   `arrow_select::coalesce::coalesce` (stessa semantica "primo non-null").

use std::cmp::Ordering;
use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_core::PlenoraError;
use plenora_kernels_table::cleansing::coalesce_fast;
use plenora_kernels_table::filtering::{filter, Filter, Operator};
use plenora_kernels_table::{
    compare_bounds, compare_f64, compare_i64, compare_u64, scalar_as_string, select_rows,
    NumericBound,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Generatori deterministici dal payload
// ---------------------------------------------------------------------------

fn edge_float(byte: u8) -> f64 {
    match byte % 10 {
        0 => 0.0,
        1 => -0.0,
        2 => f64::NAN,
        3 => f64::INFINITY,
        4 => f64::NEG_INFINITY,
        5 => f64::from(byte) * 0.5,
        6 => -f64::from(byte),
        7 => f64::from(byte),
        8 => 9_007_199_254_740_992.0,   // 2^53: ultimo intero esatto in f64
        _ => -9_007_199_254_740_992.0,
    }
}

/// Interi guidati dal byte: piccoli, oltre 2^53 (dove due interi distinti
/// collassano sullo stesso double) ed estremi di gamma i64.
fn edge_int(byte: u8) -> i64 {
    match byte % 6 {
        0 => i64::from(byte) - 128,
        1 => 9_007_199_254_740_992 + i64::from(byte % 4),  // 2^53 + k
        2 => -9_007_199_254_740_992 - i64::from(byte % 4), // -(2^53) - k
        3 => i64::MAX - i64::from(byte % 8),
        4 => i64::MIN + i64::from(byte % 8),
        _ => i64::from(byte) * 1_000_000 - 128_000_000,
    }
}

/// u64 guidati dal byte: piccoli, oltre 2^53 e vicini a u64::MAX.
fn edge_uint(byte: u8) -> u64 {
    match byte % 5 {
        0 => u64::from(byte),
        1 => 9_007_199_254_740_992 + u64::from(byte % 4), // 2^53 + k
        2 => u64::MAX - u64::from(byte % 8),
        3 => u64::from(byte) * 1_000_000,
        _ => 18_446_744_073_709_551_610 + u64::from(byte % 6), // u64::MAX - k
    }
}

/// Letterali numerici testuali che separano il confronto esatto da quello
/// via `f64`: e' su questi che i due confronti danno risposte diverse.
const EDGE_NUMERIC_TEXT: [&str; 12] = [
    // Scala positiva: due scritture dello stesso valore, e due valori che
    // `f64` non distingue.
    "10.5",
    "10.50",
    "0.1",
    "0.10000000000000001",
    // Scala negativa (notazione esponenziale con esponente positivo).
    "1e3",
    "-2.5e2",
    // Oltre 2^53: interi distinti che `f64` collassa sullo stesso double.
    "9007199254740992",
    "9007199254740993",
    "-9007199254740993",
    "18446744073709551615",
    // Decimale con piu' cifre di quante ne tenga un i128: non rappresentabile
    // in forma esatta, deve ricadere su `F64` senza rompere il confronto.
    "0.123456789012345678901234567890123456789012345",
    "-0.000000000000000000000000000000000000001",
];

fn edge_string(index: usize, chunk: &[u8]) -> String {
    match index % 11 {
        0 => "true".to_owned(),
        1 => "false".to_owned(),
        2 => "0".to_owned(),
        3 => "-0.0".to_owned(),
        4 => "NaN".to_owned(),
        5 => "🙂é\u{0}".to_owned(),
        6 => EDGE_NUMERIC_TEXT[index % EDGE_NUMERIC_TEXT.len()].to_owned(),
        7 => EDGE_NUMERIC_TEXT[(index / 3) % EDGE_NUMERIC_TEXT.len()].to_owned(),
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
                .map(|row| (row % null_period != 0).then(|| edge_int(payload[row].wrapping_add(variant as u8))))
                .collect::<Vec<_>>(),
        )) as ArrayRef,
        1 => Arc::new(UInt64Array::from(
            (0..rows)
                .map(|row| (row % null_period != 0).then(|| edge_uint(payload[row].wrapping_add(variant as u8))))
                .collect::<Vec<_>>(),
        )) as ArrayRef,
        2 => Arc::new(Float64Array::from(
            (0..rows)
                .map(|row| {
                    (row % null_period != 0).then(|| edge_float(payload[row].wrapping_add(variant as u8)))
                })
                .collect::<Vec<_>>(),
        )) as ArrayRef,
        3 => Arc::new(BooleanArray::from(
            (0..rows)
                .map(|row| (row % null_period != 0).then_some(payload[row].wrapping_add(variant as u8) % 2 == 0))
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
        // Oltre 2^53: interi che collassano sullo stesso double.
        "9007199254740992", "9007199254740993", "-9007199254740993",
        "18446744073709551614",
        // Decimali esatti: costruiscono `NumericBound::Decimal`, confrontato
        // in `i128` scalato. Scale positive, scala negativa (esponenziale) e
        // un decimale oltre la capacita' di `i128`, che ricade su `F64`.
        "10.5", "10.50", "0.1", "0.10000000000000001", "-2.5e2",
        "0.123456789012345678901234567890123456789012345",
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
            1 => Value::from(edge_int(payload.get(1).copied().unwrap_or_default())),
            2 => Value::from(edge_float(payload.get(1).copied().unwrap_or_default())),
            _ => Value::String(String::from_utf8_lossy(payload).into_owned()),
        },
        Operator::Contains | Operator::Startswith | Operator::Endswith => {
            Value::String(edge_string(pick, &payload[..payload.len().min(8)]))
        }
    }
}

// ---------------------------------------------------------------------------
// Oracolo: replica letterale del percorso generico di `filtering::evaluate`
// (riga per riga, confronti tipizzati esatti via `NumericBound`) — e' il
// riferimento dei fast path.
// ---------------------------------------------------------------------------

/// Replica di `filtering::numeric_eq` (uguaglianza sui double: total_cmp,
/// 0.0 == -0.0, NaN uguale a NaN).
fn numeric_eq(actual: f64, expected: f64) -> bool {
    actual.total_cmp(&expected) == Ordering::Equal || (actual == 0.0 && expected == 0.0)
}

/// Replica di `filtering::ordered_typed`.
fn ordered_typed(ordering: Option<Ordering>, operator: &Operator) -> bool {
    match operator {
        Operator::Gt => ordering == Some(Ordering::Greater),
        Operator::Ge => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        Operator::Lt => ordering == Some(Ordering::Less),
        Operator::Le => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        _ => unreachable!("solo operatori ordinati"),
    }
}

/// Replica di `filtering::within_bounds`.
fn within_bounds(low: Option<Ordering>, high: Option<Ordering>) -> bool {
    !matches!(low, None | Some(Ordering::Less))
        && !matches!(high, None | Some(Ordering::Greater))
}

/// Confronto sul percorso NON tipizzato, come `scalar_compare` del kernel.
///
/// Nessun `bound_as_f64` qui: degradare a `f64` sia il valore sia l'estremo
/// replicherebbe una semantica che il kernel non ha. I confronti stanno nel
/// dominio esatto (`i64`, `i128` scalato per i decimali) proprio perche'
/// `f64` sopra 2^53 collassa interi distinti e sui decimali cambia l'esito.
/// Un oracolo differenziale che replica una semantica assente dal prodotto
/// non e' un oracolo: o segnala differenze inesistenti, o concorda per la
/// ragione sbagliata.
///
/// **Fail-closed**: cio' che l'oracolo non sa confrontare esattamente e' un
/// errore, mai un confronto approssimato. Il kernel su quei tipi fallisce a
/// sua volta (`scalar_compare` non ha un ramo per Boolean), e il target
/// confronta gli errori come tali.
fn compare_scalar_exact(
    array: &dyn Array,
    row: usize,
    bound: NumericBound,
) -> Result<Option<Ordering>, PlenoraError> {
    let Some(values) = array.as_any().downcast_ref::<StringArray>() else {
        return Err(PlenoraError::Schema(format!(
            "tipo {:?} non confrontabile esattamente",
            array.data_type()
        )));
    };
    // Stessa normalizzazione del kernel (trim, virgola decimale), ma il
    // letterale resta intero o decimale quando lo e': nessun arrotondamento.
    let text = values.value(row).trim().replace(',', ".");
    let actual = NumericBound::parse(&text)
        .ok_or_else(|| PlenoraError::Schema("valore non convertibile in numero".into()))?;
    Ok(compare_bounds(actual, bound))
}

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
            let equal = if array.data_type() == &DataType::Int64 {
                let bound = NumericBound::parse(&expected).ok_or_else(|| {
                    PlenoraError::InvalidPlan("confronto numerico con valore non numerico".into())
                })?;
                let values = array
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| PlenoraError::Schema("array Int64 incoerente".into()))?;
                compare_i64(values.value(row), bound) == Some(Ordering::Equal)
            } else if array.data_type() == &DataType::Float64 {
                let bound = NumericBound::parse(&expected).ok_or_else(|| {
                    PlenoraError::InvalidPlan("confronto numerico con valore non numerico".into())
                })?;
                let values = array
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| PlenoraError::Schema("array Float64 incoerente".into()))?;
                let actual = values.value(row);
                match bound {
                    NumericBound::F64(number) => numeric_eq(actual, number),
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
                PlenoraError::InvalidPlan("confronto ordinato richiede un valore numerico".into())
            })?;
            let ordering = if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                compare_i64(values.value(row), bound)
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                compare_u64(values.value(row), bound)
            } else if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
                compare_f64(values.value(row), bound)
            } else {
                compare_scalar_exact(array, row, bound)?
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
                .ok_or_else(|| PlenoraError::InvalidPlan("between richiede min,max".into()))?;
            let low = NumericBound::parse(low.trim())
                .ok_or_else(|| PlenoraError::InvalidPlan("min between non valido".into()))?;
            let high = NumericBound::parse(high.trim())
                .ok_or_else(|| PlenoraError::InvalidPlan("max between non valido".into()))?;
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
                within_bounds(
                    compare_scalar_exact(array, row, low)?,
                    compare_scalar_exact(array, row, high)?,
                )
            };
            Ok(within)
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
    let column = typed_column(selector, payload, 0);
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
        .map(|n| typed_column(selector, payload, n))
        .collect();
    let fields: Vec<Field> = (0..columns)
        .map(|n| Field::new(format!("c{n}"), arrays[n].data_type().clone(), true))
        .collect();
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).expect("fixture");
    let indices: Vec<usize> = (0..columns).collect();
    let Some(fast) = coalesce_fast(&batch, &indices) else {
        return; // tipo non coperto dal fast path: niente differenziale
    };
    // Oracolo: replica del percorso generico (`quality::coalesce_generic`,
    // pub(crate)): concat delle colonne + take del primo non-null per riga.
    let refs: Vec<&dyn Array> = batch.columns().iter().map(|c| c.as_ref()).collect();
    let combined = arrow_select::concat::concat(&refs).expect("concat oracolo");
    let take_indices: Vec<Option<u64>> = (0..batch.num_rows())
        .map(|row| {
            indices
                .iter()
                .position(|index| !batch.column(*index).is_null(row))
                .map(|position| (position * batch.num_rows() + row) as u64)
        })
        .collect();
    let oracle = arrow_select::take::take(
        combined.as_ref(),
        &UInt64Array::from(take_indices),
        None,
    )
    .expect("take oracolo");
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
