//! Benchmark autonomo per i kernel `table.statistics` e `table.flatten_json`
//! (filone ottimizzazioni kernel, batch analysis).
//!
//! Fixture deterministiche identiche a `bench_sweep` (seed logico 42,
//! xorshift64, stesso ordine di draw), cosi' i numeri sono confrontabili
//! con `benchmarks/sweep/sweep.json`:
//! - statistics: `num` float64 x `grp` utf8 (1024 gruppi), tutte le 10
//!   statistiche (config dello sweep);
//! - `flatten_json` discovery: JSON annidati 3 livelli, `output_columns`
//!   vuote (config dello sweep);
//! - `flatten_json` selective: stessi documenti, 3 `output_columns`
//!   esplicite (parsing selettivo dei soli path richiesti).
//!
//! Uso: `bench_analysis <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::analysis::{flatten_json, statistics, FlattenJson, Stat, Statistics};
use plenora_kernels_table::Limits;
use serde_json::json;

/// RNG deterministico (xorshift64*, stesso schema di `bench_sweep`).
struct Rng(u64);

impl Rng {
    const fn seeded() -> Self {
        Self(42)
    }

    const fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Fixture base di `bench_sweep` (stesso stream xorshift: 9 draw per riga
/// nello stesso ordine); servono solo `num` e `grp`, ma i draw extra
/// mantengono lo stream allineato allo sweep.
fn stats_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let mut nums = Vec::with_capacity(rows);
    let mut groups = Vec::with_capacity(rows);
    for _ in 0..rows {
        // Bound evidente: draw % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
        #[allow(clippy::cast_precision_loss)]
        nums.push(Some((rng.next() % 1_000_000) as f64 / 100.0));
        groups.push(format!("g{}", rng.next() % 1_024));
        for _ in 0..7 {
            let _ = rng.next();
        }
    }
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("num", DataType::Float64, false),
            Field::new("grp", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(groups)),
        ],
    )
    .expect("fixture statistics")
}

/// Fixture JSON annidati (3 livelli), identica a `bench_sweep::json_fixture`.
fn json_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let ids = (0..rows)
        .map(|row| i64::try_from(row).ok())
        .collect::<Vec<_>>();
    let docs = (0..rows)
        .map(|_| {
            format!(
                "{{\"a\":{},\"b\":{{\"c\":{},\"d\":{{\"e\":\"{:08x}\",\"f\":[1,2,3]}}}},\"g\":\"{:08x}\"}}",
                rng.next() % 1000,
                rng.next() % 1000,
                rng.next() & 0xffff_ffff,
                rng.next() & 0xffff_ffff
            )
        })
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("doc", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(StringArray::from(docs)),
        ],
    )
    .expect("fixture json")
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

fn measure(
    op: &'static str,
    rows: usize,
    repetitions: usize,
    note: &str,
    execute: impl Fn() -> RecordBatch,
) {
    black_box(execute());
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_rows = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = execute();
        durations.push(start.elapsed().as_secs_f64());
        output_rows = output.num_rows();
        black_box(output);
    }
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    #[allow(clippy::cast_precision_loss)]
    let rows_per_second = rows as f64 / median;
    let record = json!({
        "scenario": op,
        "rows": rows,
        "repetitions": repetitions,
        "median_seconds": median,
        "rows_per_second": rows_per_second,
        "output_rows": output_rows,
        "peak_rss_kib": peak_rss_kib(),
        "note": note,
    });
    println!("{}", serde_json::to_string(&record).expect("JSON"));
}

fn main() {
    let rows = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(1_000_000);
    let repetitions = std::env::args()
        .nth(2)
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(3);
    let limits = Limits {
        max_rows: 40_000_000,
        max_memory_bytes: 6 * 1024 * 1024 * 1024,
        ..Limits::default()
    };

    let statistics_config = Statistics {
        column: "num".into(),
        group_by: Some("grp".into()),
        stats: vec![
            Stat::Count,
            Stat::Min,
            Stat::Max,
            Stat::Sum,
            Stat::Mean,
            Stat::Median,
            Stat::Std,
            Stat::Var,
            Stat::Q25,
            Stat::Q75,
        ],
        output_prefix: String::new(),
    };
    let stats_input = stats_fixture(rows);
    measure(
        "table.statistics",
        rows,
        repetitions,
        "10 statistiche x 1024 gruppi",
        || statistics(&stats_input, &statistics_config).expect("statistics"),
    );

    let discovery_config = FlattenJson {
        column: "doc".into(),
        prefix: String::new(),
        max_level: 3,
        output_columns: Vec::new(),
    };
    let json_input = json_fixture(rows);
    measure(
        "table.flatten_json",
        rows,
        repetitions,
        "JSON annidati 3 livelli (discovery)",
        || flatten_json(&json_input, &discovery_config, &limits).expect("flatten_json"),
    );

    let selective_config = FlattenJson {
        column: "doc".into(),
        prefix: String::new(),
        max_level: 3,
        output_columns: vec!["doc_a".into(), "doc_b.d.e".into(), "doc_g".into()],
    };
    measure(
        "table.flatten_json",
        rows,
        repetitions,
        "JSON annidati 3 livelli (3 output_columns)",
        || flatten_json(&json_input, &selective_config, &limits).expect("flatten_json"),
    );
}
