//! Benchmark di baseline per le operazioni di sola proiezione/rinomina
//! di `plenora-nogeo-tools`, non coperte dagli esempi `benchmark` e
//! `candidate_benchmark` del progetto di origine.
//!
//! Uso: `nogeo-projection-bench <rows> <repetitions> <operation>`
//! con operation in {rename, drop_columns, reorder_columns}.
//! La fixture e il formato JSON replicano `examples/candidate_benchmark.rs`.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use arrow_array::{Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use plenora_nogeo_tools::{execute_batch, Limits, Plan, Step, ValidatedPlan};
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

fn plan(operation: &str, config: Value, rows: usize) -> ValidatedPlan {
    Plan {
        schema_version: 1,
        limits: Limits {
            max_rows: rows.saturating_mul(2).max(1),
            ..Limits::default()
        },
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .expect("benchmark plan")
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

fn main() {
    let rows: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let repetitions: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(5);
    let operation = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "rename".into());
    assert!(rows > 0 && repetitions > 0);
    let plan = match operation.as_str() {
        "rename" => plan(
            "rename",
            json!({"renames":[
                {"old_name":"group","new_name":"grp"},
                {"old_name":"text","new_name":"txt"}
            ]}),
            rows,
        ),
        "drop_columns" => plan("drop_columns", json!({"columns":["text"]}), rows),
        "reorder_columns" => plan(
            "reorder_columns",
            json!({"columns":["text","num","group","id"]}),
            rows,
        ),
        other => panic!("benchmark sconosciuto: {other}"),
    };
    let input = fixture(rows);
    black_box(execute_batch(input.clone(), &plan).expect("warmup"));
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_bytes = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = execute_batch(input.clone(), &plan).expect("execution");
        durations.push(start.elapsed().as_secs_f64());
        output_bytes = output.get_array_memory_size();
        black_box(output);
    }
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "operation": operation,
            "rows": rows,
            "repetitions": repetitions,
            "median_seconds": median,
            "rows_per_second": rows as f64 / median,
            "input_bytes": input.get_array_memory_size(),
            "output_bytes": output_bytes,
            "peak_rss_kib": peak_rss_kib(),
        }))
        .expect("JSON")
    );
}
