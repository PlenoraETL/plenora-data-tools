//! Benchmark autonomo per i kernel `table.filter` e `table.sort`.
//!
//! Fixture deterministica (seed logico 42): `id` int64, `num` float64
//! (0..9999), `group` utf8 (1024 gruppi), `text` utf8.
//!
//! Uso: `bench_filter_sort <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

#[path = "comune/mod.rs"]
mod comune;

use comune::run_scenario;

use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::aggregation::{sort, Sort};
use plenora_kernels_table::filtering::{filter, Filter, Operator};
use serde_json::Value;

fn fixture(rows: usize) -> RecordBatch {
    let ids = (0..rows)
        .map(|row| i64::try_from(row).ok())
        .collect::<Vec<_>>();
    let numbers = (0..rows)
        .map(|row| u32::try_from(row % 10_000).ok().map(f64::from))
        .collect::<Vec<_>>();
    let groups = (0..rows)
        .map(|row| format!("g{}", row % 1_024))
        .collect::<Vec<_>>();
    let text = (0..rows)
        .map(|row| format!("{}", row % 100_000))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("group", DataType::Utf8, false),
            Field::new("text", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(numbers)),
            Arc::new(StringArray::from(groups)),
            Arc::new(StringArray::from(text)),
        ],
    )
    .expect("benchmark fixture")
}

fn main() {
    let rows = comune::argomenti::intero_positivo_arg(1, "rows", 1_000_000);
    let repetitions = comune::argomenti::intero_positivo_arg(2, "repetitions", 3);
    let input = fixture(rows);

    let filter_num = Filter {
        column: "num".into(),
        operator: Operator::Eq,
        value: Value::from(42),
    };
    run_scenario("filter_num_eq", rows, repetitions, || {
        filter(&input, &filter_num).expect("filter num")
    });

    let filter_group = Filter {
        column: "group".into(),
        operator: Operator::Eq,
        value: Value::from("g42"),
    };
    run_scenario("filter_utf8_eq", rows, repetitions, || {
        filter(&input, &filter_group).expect("filter group")
    });

    let sort_num = Sort {
        columns: vec!["num".into()],
        ascending: true,
    };
    run_scenario("sort_num", rows, repetitions, || {
        sort(&input, &sort_num).expect("sort num")
    });

    let sort_group_num = Sort {
        columns: vec!["group".into(), "num".into()],
        ascending: true,
    };
    run_scenario("sort_group_num", rows, repetitions, || {
        sort(&input, &sort_group_num).expect("sort group+num")
    });
}
