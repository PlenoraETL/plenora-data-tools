//! Oracolo dello spill: il percorso su disco coincide bit a bit con quello
//! in memoria, sullo stesso input.
//!
//! Il sort passa dall'engine come un piano reale: lo spill si attiva quando
//! i byte stimati dell'input superano `max_governed_memory_bytes`
//! (`should_spill_unary`), e qui il budget e' una frazione casuale della
//! stima, quindi lo spill parte sempre e le run hanno da una a `righe/4`
//! righe, la stessa distribuzione che vede un piano in produzione.
//!
//! `distinct`, `aggregate` e le set operation chiamano direttamente le
//! varianti spilled del kernel, con un budget di memoria ampio: nell'engine
//! lo stesso budget decide sia l'attivazione sia il working set in lettura,
//! e un budget sotto la stima dell'input rifiuterebbe (in modo esplicito)
//! quasi ogni caso. Qui conta l'altra meta' del contratto: partizioni da
//! molte righe, in ordine diverso dall'input, devono dare l'output in
//! memoria. L'atteso e' sempre il kernel in memoria.

use std::sync::Arc;

use plenora_core::arrow::array::{
    BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::{execute_batch, execute_batch_with_spill, execute_binary, Limits};
use plenora_kernels_table::aggregation::{self, Aggregate, Distinct};
use plenora_kernels_table::spill::{
    aggregate_spilled, distinct_spilled, estimated_batch_bytes, execute_set_operation,
};
use proptest::prelude::*;
use serde_json::json;

// Ogni file usa una parte delle fixture: il resto serve agli altri.
#[allow(dead_code)]
mod fixture_table;
use fixture_table::plan_with_limits;

/// Valori Float64 che il comparatore distingue solo con `total_cmp`: NaN di
/// entrambi i segni, zeri con segno, infiniti, null.
const FLOATS: [Option<f64>; 9] = [
    Some(f64::NAN),
    Some(-f64::NAN),
    Some(-0.0),
    Some(0.0),
    Some(1.5),
    Some(-2.25),
    Some(f64::INFINITY),
    Some(f64::NEG_INFINITY),
    None,
];

const TEXTS: [Option<&str>; 6] = [
    Some("alfa"),
    Some("beta"),
    Some(""),
    Some("Zeta"),
    Some("\u{e4}"),
    None,
];

/// Una riga generata: chiavi con pochi valori distinti (molti pareggi) e null.
type Riga = (Option<i64>, usize, usize, Option<bool>);

fn riga() -> impl Strategy<Value = Riga> {
    (
        proptest::option::weighted(0.85, -4_i64..5),
        0..FLOATS.len(),
        0..TEXTS.len(),
        proptest::option::weighted(0.8, any::<bool>()),
    )
}

/// Batch con le chiavi generate e un `id` che rende leggibile ogni
/// permutazione sbagliata.
fn batch(righe: &[Riga]) -> RecordBatch {
    let ids = (0..righe.len())
        .map(|row| i64::try_from(row).expect("id"))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("i", DataType::Int64, true),
            Field::new("f", DataType::Float64, true),
            Field::new("s", DataType::Utf8, true),
            Field::new("b", DataType::Boolean, true),
            Field::new("id", DataType::Int64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(
                righe.iter().map(|riga| riga.0).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                righe.iter().map(|riga| FLOATS[riga.1]).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                righe.iter().map(|riga| TEXTS[riga.2]).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                righe.iter().map(|riga| riga.3).collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(ids)),
        ],
    )
    .expect("batch generato")
}

/// Limiti del kernel con memoria ampia: il working set in lettura non e'
/// mai il vincolo, le partizioni sono quelle date.
fn limiti_del_kernel(spill_partitions: usize) -> Limits {
    Limits {
        max_governed_memory_bytes: 1 << 30,
        spill_partitions,
        ..Limits::default()
    }
}

fn errore(contesto: &str) -> impl Fn(plenora_core::PlenoraError) -> TestCaseError + '_ {
    move |error| TestCaseError::fail(format!("{contesto}: {error}"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn sort_spilled_is_the_in_memory_sort(
        righe in proptest::collection::vec(riga(), 2..400),
        permille in 1_usize..1_000,
        columns in proptest::sample::subsequence(vec!["i", "f", "s", "b"], 1..=4)
            .prop_shuffle(),
        ascending in any::<bool>(),
    ) {
        let input = batch(&righe);
        let config = json!({"columns": columns, "ascending": ascending});
        let spill = Limits {
            max_governed_memory_bytes: (estimated_batch_bytes(&input) * permille / 1_000).max(1),
            ..Limits::default()
        };
        let in_memoria = execute_batch(
            input.clone(),
            &plan_with_limits("sort", config.clone(), Limits::default()),
        )
        .map_err(errore("sort in memoria"))?;
        let (spilled, metriche) = execute_batch_with_spill(
            input,
            &plan_with_limits("sort", config.clone(), spill),
            None,
        )
        .map_err(errore("sort spilled"))?;
        prop_assert!(metriche.files > 1, "lo spill non e' partito: {:?}", metriche);
        prop_assert_eq!(&spilled, &in_memoria, "{}", config);
    }

    #[test]
    fn distinct_and_aggregate_spilled_match_in_memory(
        righe in proptest::collection::vec(riga(), 2..250),
        spill_partitions in 2_usize..12,
        subset in proptest::sample::subsequence(vec!["i", "f", "s", "b"], 1..=4)
            .prop_shuffle(),
        keep in proptest::sample::select(vec!["first", "last", "false"]),
    ) {
        let input = batch(&righe);
        let limits = limiti_del_kernel(spill_partitions);
        let config: Distinct = serde_json::from_value(json!({"subset": subset, "keep": keep}))
            .expect("config distinct");
        let in_memoria = aggregation::distinct(&input, &config)
            .map_err(errore("distinct in memoria"))?;
        let (spilled, _) = distinct_spilled(&input, &config, &limits)
            .map_err(errore("distinct spilled"))?;
        prop_assert_eq!(&spilled, &in_memoria, "{:?}", config);

        // first/last/concat e la somma Float64 dipendono dall'ordine delle
        // righe dentro il gruppo: la partizione deve conservarlo.
        let config: Aggregate = serde_json::from_value(json!({
            "group_by": subset,
            "aggregations": [
                {"column": "id", "function": "first", "alias": "id_first"},
                {"column": "id", "function": "last", "alias": "id_last"},
                {"column": "s", "function": "concat", "separator": "|", "alias": "testi"},
                {"column": "f", "function": "sum", "alias": "somma"},
                {"column": "i", "function": "count", "alias": "conta"},
            ],
        }))
        .expect("config aggregate");
        let in_memoria = aggregation::aggregate(&input, &config)
            .map_err(errore("aggregate in memoria"))?;
        let (spilled, _) = aggregate_spilled(&input, &config, &limits)
            .map_err(errore("aggregate spilled"))?;
        prop_assert_eq!(&spilled, &in_memoria, "{:?}", config);
    }

    #[test]
    fn set_operations_spilled_match_in_memory_on_several_key_types(
        sinistra in proptest::collection::vec(riga(), 1..200),
        destra in proptest::collection::vec(riga(), 1..200),
        spill_partitions in 2_usize..12,
    ) {
        // Le set operation confrontano righe intere: l'`id` renderebbe ogni
        // riga unica, quindi qui resta fuori dallo schema.
        let senza_id = |righe: &[Riga]| batch(righe).project(&[0, 1, 2, 3]).expect("proiezione");
        let left = senza_id(&sinistra);
        let right = senza_id(&destra);
        let limits = limiti_del_kernel(spill_partitions);
        for operation in ["union_distinct", "intersect", "except"] {
            let in_memoria = execute_binary(
                &left,
                &right,
                &plan_with_limits(operation, json!({}), Limits::default()),
            )
            .map_err(errore(operation))?;
            let spilled = execute_set_operation(operation, &left, &right, &limits)
                .map_err(errore(operation))?;
            prop_assert_eq!(&spilled, &in_memoria, "{}", operation);
        }
    }
}
