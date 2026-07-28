//! Benchmark autonomo per i kernel `table.melt` e `table.pivot`
//! (filone ottimizzazioni kernel, ultimo batch: reshape).
//!
//! Fixture deterministica (seed logico 42, xorshift64):
//! - melt: `id` int64 + `grp` utf8 (100 gruppi) + 8 colonne float64
//!   (wide -> long, 10M righe x 8 colonne = 80M righe in output);
//! - melt eterogeneo (`type_policy`='string'): 4 colonne miste
//!   int64/utf8/float64/bool;
//! - pivot: `k` int64 (rows/100 chiavi) x `p` utf8 (100 valori distinti)
//!   -> wide ~100 colonne, aggr sum/first su `v` float64.
//!
//! Uso: `bench_reshape <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::reshape::{melt, pivot, Melt, Pivot, PivotAgg};
use plenora_kernels_table::Limits;
use serde_json::json;

/// xorshift64 deterministico (seed 42): stessi dati ad ogni run.
struct Rng(u64);

impl Rng {
    const fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn melt_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng(42);
    let ids = (0..rows)
        .map(|row| i64::try_from(row).expect("fixture"))
        .collect::<Vec<_>>();
    let groups = (0..rows)
        .map(|row| format!("g{:03}", row % 100))
        .collect::<Vec<_>>();
    let mut fields = vec![
        Field::new("id", DataType::Int64, false),
        Field::new("grp", DataType::Utf8, false),
    ];
    let mut columns: Vec<Arc<dyn plenora_core::arrow::array::Array>> = vec![
        Arc::new(Int64Array::from(ids)),
        Arc::new(StringArray::from(groups)),
    ];
    for column in 0..8 {
        fields.push(Field::new(format!("v{column}"), DataType::Float64, true));
        let values = (0..rows)
            .map(|_| {
                let raw = rng.next();
                // ~3% di null, valori nell'intervallo [0, 10000).
                if raw % 97 < 3 {
                    None
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    Some((raw % 10_000) as f64 + (raw % 97) as f64 / 97.0)
                }
            })
            .collect::<Vec<_>>();
        columns.push(Arc::new(Float64Array::from(values)));
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("benchmark fixture")
}

fn melt_hetero_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng(42);
    let ids = (0..rows)
        .map(|row| i64::try_from(row).expect("fixture"))
        .collect::<Vec<_>>();
    let numbers = (0..rows)
        .map(|_| {
            let raw = rng.next();
            if raw % 89 < 3 {
                None
            } else {
                Some(i64::try_from(raw % 100_000).expect("fixture"))
            }
        })
        .collect::<Vec<_>>();
    let texts = (0..rows)
        .map(|row| {
            if row % 83 < 3 {
                None
            } else {
                Some(format!("txt_{:06}", row % 500_000))
            }
        })
        .collect::<Vec<_>>();
    let floats = (0..rows)
        .map(|_| {
            let raw = rng.next();
            if raw % 79 < 3 {
                None
            } else {
                #[allow(clippy::cast_precision_loss)]
                Some((raw % 10_000) as f64 / 7.0)
            }
        })
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("n", DataType::Int64, true),
            Field::new("t", DataType::Utf8, true),
            Field::new("f", DataType::Float64, true),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Int64Array::from(numbers)),
            Arc::new(StringArray::from(texts)),
            Arc::new(Float64Array::from(floats)),
        ],
    )
    .expect("benchmark fixture")
}

fn pivot_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng(42);
    let keys = rows / 100;
    let key_column = (0..rows)
        .map(|row| i64::try_from(row % keys).expect("fixture"))
        .collect::<Vec<_>>();
    let pivot_column = (0..rows)
        .map(|row| format!("p{:03}", (row / keys) % 100))
        .collect::<Vec<_>>();
    let values = (0..rows)
        .map(|_| {
            let raw = rng.next();
            if raw % 89 < 3 {
                None
            } else {
                #[allow(clippy::cast_precision_loss)]
                Some((raw % 10_000) as f64 + (raw % 89) as f64 / 89.0)
            }
        })
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", DataType::Int64, false),
            Field::new("p", DataType::Utf8, false),
            Field::new("v", DataType::Float64, true),
        ])),
        vec![
            Arc::new(Int64Array::from(key_column)),
            Arc::new(StringArray::from(pivot_column)),
            Arc::new(Float64Array::from(values)),
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
    run: impl Fn() -> plenora_core::Result<RecordBatch>,
) {
    black_box(run().expect("warmup"));
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_rows = 0;
    let mut output_columns = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = run().expect("scenario");
        durations.push(start.elapsed().as_secs_f64());
        output_rows = output.num_rows();
        output_columns = output.num_columns();
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
            "output_columns": output_columns,
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
    let limits = Limits {
        max_rows: 200_000_000,
        ..Limits::default()
    };

    let melt_input = melt_fixture(rows);
    let melt_config = Melt {
        id_columns: vec!["id".into(), "grp".into()],
        value_columns: (0..8).map(|column| format!("v{column}")).collect(),
        var_name: "variable".into(),
        value_name: "value".into(),
        type_policy: plenora_kernels_table::reshape::HeterogeneousTypePolicy::Reject,
    };
    run_scenario("melt_wide_to_long_8f64", rows, repetitions, || {
        melt(&melt_input, &melt_config, &limits)
    });

    let hetero_rows = rows.min(2_000_000);
    let hetero_input = melt_hetero_fixture(hetero_rows);
    let hetero_config = Melt {
        id_columns: vec!["id".into()],
        value_columns: vec!["n".into(), "t".into(), "f".into()],
        var_name: "variable".into(),
        value_name: "value".into(),
        type_policy: plenora_kernels_table::reshape::HeterogeneousTypePolicy::String,
    };
    run_scenario(
        "melt_heterogeneous_string_policy",
        hetero_rows,
        repetitions,
        || melt(&hetero_input, &hetero_config, &limits),
    );

    let pivot_input = pivot_fixture(rows);
    let pivot_sum = Pivot {
        index_col: "k".into(),
        column: "p".into(),
        value_col: "v".into(),
        aggr_func: PivotAgg::Sum,
        mapping: std::collections::BTreeMap::new(),
    };
    run_scenario("pivot_wide_100_cols_sum", rows, repetitions, || {
        pivot(&pivot_input, &pivot_sum, &limits)
    });
    let pivot_first = Pivot {
        index_col: "k".into(),
        column: "p".into(),
        value_col: "v".into(),
        aggr_func: PivotAgg::First,
        mapping: std::collections::BTreeMap::new(),
    };
    run_scenario("pivot_wide_100_cols_first", rows, repetitions, || {
        pivot(&pivot_input, &pivot_first, &limits)
    });
}
