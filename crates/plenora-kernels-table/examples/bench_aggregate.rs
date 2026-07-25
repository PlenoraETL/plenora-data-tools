//! Benchmark autonomo per il kernel `table.aggregate`
//! (filone ottimizzazioni kernel, secondo batch dopo filter/sort).
//!
//! Fixture deterministica (seed logico 42): `g100` utf8 (100 gruppi),
//! `g1m` int64 (1_000_000 gruppi), `num` float64 (0..9999), `val` int64.
//!
//! Uso: `bench_aggregate <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::aggregation::{aggregate, AggFunction, Aggregate, Aggregation};
use serde_json::json;

fn fixture(rows: usize) -> RecordBatch {
    let small = (0..rows)
        .map(|row| format!("g{:03}", row % 100))
        .collect::<Vec<_>>();
    let large = (0..rows)
        .map(|row| i64::try_from(row % 1_000_000).ok())
        .collect::<Vec<_>>();
    let numbers = (0..rows)
        .map(|row| u32::try_from(row % 10_000).ok().map(f64::from))
        .collect::<Vec<_>>();
    let values = (0..rows)
        .map(|row| i64::try_from(row % 100_000).ok())
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("g100", DataType::Utf8, false),
            Field::new("g1m", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("val", DataType::Int64, false),
        ])),
        vec![
            Arc::new(StringArray::from(small)),
            Arc::new(Int64Array::from(large)),
            Arc::new(Float64Array::from(numbers)),
            Arc::new(Int64Array::from(values)),
        ],
    )
    .expect("benchmark fixture")
}

fn aggregation(column: &str, function: AggFunction) -> Aggregation {
    Aggregation {
        column: column.into(),
        function,
        separator: ", ".into(),
        distinct: false,
        skip_null: true,
        alias: String::new(),
        quantile: None,
        ddof: 1,
    }
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
    config: &Aggregate,
) {
    black_box(aggregate(input, config).expect("warmup"));
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_rows = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = aggregate(input, config).expect("aggregate");
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

    // Gruppi piccoli (~100), mix di funzioni numeriche + count + nunique.
    let small_mixed = Aggregate {
        group_by: vec!["g100".into()],
        aggregations: vec![
            aggregation("num", AggFunction::Mean),
            aggregation("num", AggFunction::Min),
            aggregation("num", AggFunction::Max),
            aggregation("val", AggFunction::Sum),
            aggregation("val", AggFunction::Count),
            aggregation("val", AggFunction::Nunique),
        ],
    };
    run_scenario(
        "aggregate_small_groups_mixed",
        rows,
        repetitions,
        &input,
        &small_mixed,
    );

    // Gruppi grandi (~1M chiavi distinte), singola aggregazione.
    let large_sum = Aggregate {
        group_by: vec!["g1m".into()],
        aggregations: vec![aggregation("num", AggFunction::Sum)],
    };
    run_scenario(
        "aggregate_large_groups_sum",
        rows,
        repetitions,
        &input,
        &large_sum,
    );

    // Gruppi grandi, piu' aggregazioni sulla stessa passata di gruppi.
    let large_multi = Aggregate {
        group_by: vec!["g1m".into()],
        aggregations: vec![
            aggregation("num", AggFunction::Mean),
            aggregation("num", AggFunction::Variance),
            aggregation("val", AggFunction::Sum),
        ],
    };
    run_scenario(
        "aggregate_large_groups_multi",
        rows,
        repetitions,
        &input,
        &large_multi,
    );
}
