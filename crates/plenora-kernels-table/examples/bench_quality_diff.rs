//! Benchmark autonomo per i kernel `table.reconcile`,
//! `table.assert_foreign_key`, `table.table_diff` e `table.mask_data`
//! (filone ottimizzazioni kernel, batch 4: quality/governance/diff/security).
//!
//! Fixture deterministica IDENTICA a `bench_sweep` (seed logico 42,
//! xorshift64*, stesse colonne e stesse configurazioni), cosi' le misure
//! sono confrontabili con le baseline di `benchmarks/sweep/sweep.json`:
//! - fixture base: `id` int64, `num` float64, `grp` utf8 (1024 gruppi),
//!   `text` utf8 (40 char esadecimali), `key` int64 (1M valori distinti
//!   possibili), `path` utf8;
//! - fixture destra: stessa chiave `id` 0..rows, `num` perturbato sul 10%
//!   delle righe (per `table_diff`), colonna extra `rval`.
//!
//! Uso: `bench_quality_diff <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::governance::{
    assert_foreign_key, reconcile, ForeignKey, Reconcile,
};
use plenora_kernels_table::reshape::{table_diff, TableDiff};
use plenora_kernels_table::security::{mask_data, MaskData, MaskType, Masking};
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

/// Fixture base condivisa: identica a `bench_sweep::base_fixture`.
fn base_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let mut ids = Vec::with_capacity(rows);
    let mut nums = Vec::with_capacity(rows);
    let mut groups = Vec::with_capacity(rows);
    let mut texts = Vec::with_capacity(rows);
    let mut keys = Vec::with_capacity(rows);
    let mut paths = Vec::with_capacity(rows);
    for row in 0..rows {
        ids.push(i64::try_from(row).ok());
        // Bound evidente: draw % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
        #[allow(clippy::cast_precision_loss)]
        nums.push(Some((rng.next() % 1_000_000) as f64 / 100.0));
        groups.push(format!("g{}", rng.next() % 1_024));
        texts.push(format!(
            "{:016x}{:016x}{:08x}",
            rng.next(),
            rng.next(),
            rng.next() & 0xffff_ffff
        ));
        // Bound evidente: draw % 1_000_000 <= 999_999, entra in i64 senza wrap.
        #[allow(clippy::cast_possible_wrap)]
        keys.push((rng.next() % 1_000_000) as i64);
        paths.push(format!(
            "p{:03}/q{:03}/r{:03}",
            rng.next() % 500,
            rng.next() % 500,
            rng.next() % 500
        ));
    }
    let schema = Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("grp", DataType::Utf8, false),
            Field::new("text", DataType::Utf8, false),
            Field::new("key", DataType::Int64, false),
            Field::new("path", DataType::Utf8, false),
        ],
        std::iter::once(("source".to_owned(), "bench_quality_diff".to_owned())).collect(),
    );
    RecordBatch::try_new(
        Arc::new(schema),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(groups)),
            Arc::new(StringArray::from(texts)),
            Arc::new(Int64Array::from(keys)),
            Arc::new(StringArray::from(paths)),
        ],
    )
    .expect("fixture base")
}

/// Fixture destra per diff/FK: identica a `bench_sweep::right_fixture`.
fn right_fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let mut ids = Vec::with_capacity(rows);
    let mut nums = Vec::with_capacity(rows);
    let mut rvals = Vec::with_capacity(rows);
    for row in 0..rows {
        ids.push(i64::try_from(row).ok());
        // Bound evidente: draw % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
        #[allow(clippy::cast_precision_loss)]
        let base = (rng.next() % 1_000_000) as f64 / 100.0;
        nums.push(Some(if row % 10 == 0 { base + 1.0 } else { base }));
        rvals.push(format!("r{:016x}", rng.next()));
    }
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("rval", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(rvals)),
        ],
    )
    .expect("fixture destra")
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

/// Limiti allargati per le scale di benchmark (come `bench_sweep`).
fn bench_limits() -> Limits {
    Limits {
        max_rows: 40_000_000,
        max_memory_bytes: 6 * 1024 * 1024 * 1024,
        ..Limits::default()
    }
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
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1_000_000);
    let repetitions = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(3);
    let limits = bench_limits();
    let left = base_fixture(rows);
    let right = right_fixture(rows);

    let table_diff_config = TableDiff {
        left_keys: vec!["id".into()],
        right_keys: vec!["id".into()],
        compare_columns: vec!["num".into()],
        include_unchanged: "no".into(),
        separator: ", ".into(),
    };
    measure("table.table_diff", rows, repetitions, "1M x 1M, chiave id, diff su num", || {
        table_diff(&left, &right, &table_diff_config, &limits).expect("table_diff")
    });

    let foreign_key_config = ForeignKey {
        left_keys: vec!["key".into()],
        right_keys: vec!["id".into()],
        allow_null: false,
    };
    measure(
        "table.assert_foreign_key",
        rows,
        repetitions,
        "1M chiavi vs 1M referenze",
        || {
            assert_foreign_key(&left, &right, &foreign_key_config, &limits)
                .expect("assert_foreign_key")
        },
    );

    let reconcile_config = Reconcile {
        left_keys: vec!["key".into()],
        right_keys: vec!["id".into()],
        nulls_equal: true,
    };
    measure("table.reconcile", rows, repetitions, "1M x 1M, frequenze chiave", || {
        reconcile(&left, &right, &reconcile_config, &limits).expect("reconcile")
    });

    let mask_config = MaskData {
        maskings: vec![Masking {
            column: "text".into(),
            mask_type: MaskType::Custom,
            chars_start: 3,
            chars_end: 3,
            mask_char: "*".into(),
        }],
        overwrite: true,
    };
    measure("table.mask_data", rows, repetitions, "mask custom 3+3 su text", || {
        mask_data(&left, &mask_config).expect("mask_data")
    });
}
