//! Benchmark autonomo per i kernel `table.filter` e `table.sort`
//! (filone ottimizzazioni kernel, Fase post-2A).
//!
//! Fixture deterministica (seed logico 42): stessa forma di
//! `benchmarks/baseline/harness` — `id` int64, `num` float64 (0..9999),
//! `group` utf8 (1024 gruppi), `text` utf8.
//!
//! Uso: `bench_filter_sort <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::aggregation::{sort, Sort};
use plenora_kernels_table::filtering::{filter, Filter, Operator};
use serde_json::{json, Value};

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

fn peak_rss_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

fn run_scenario(
    name: &str,
    rows: usize,
    repetitions: usize,
    input: &RecordBatch,
    execute: impl Fn(&RecordBatch) -> RecordBatch,
) {
    black_box(execute(input));
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_rows = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = execute(input);
        durations.push(start.elapsed().as_secs_f64());
        output_rows = output.num_rows();
        black_box(output);
    }
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    #[allow(clippy::cast_precision_loss)]
    let rows_per_second = rows as f64 / median;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "scenario": name,
            "rows": rows,
            "repetitions": repetitions,
            "median_seconds": median,
            "rows_per_second": rows_per_second,
            "output_rows": output_rows,
            "peak_rss_kib": peak_rss_kib(),
        }))
        .expect("JSON")
    );
}

fn main() {
    let rows: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let repetitions: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    assert!(rows > 0 && repetitions > 0);
    let input = fixture(rows);

    let filter_num = Filter {
        column: "num".into(),
        operator: Operator::Eq,
        value: Value::from(42),
    };
    run_scenario("filter_num_eq", rows, repetitions, &input, |batch| {
        filter(batch, &filter_num).expect("filter num")
    });

    let filter_group = Filter {
        column: "group".into(),
        operator: Operator::Eq,
        value: Value::from("g42"),
    };
    run_scenario("filter_utf8_eq", rows, repetitions, &input, |batch| {
        filter(batch, &filter_group).expect("filter group")
    });

    let sort_num = Sort {
        columns: vec!["num".into()],
        ascending: true,
    };
    run_scenario("sort_num", rows, repetitions, &input, |batch| {
        sort(batch, &sort_num).expect("sort num")
    });

    let sort_group_num = Sort {
        columns: vec!["group".into(), "num".into()],
        ascending: true,
    };
    run_scenario("sort_group_num", rows, repetitions, &input, |batch| {
        sort(batch, &sort_group_num).expect("sort group+num")
    });
}
