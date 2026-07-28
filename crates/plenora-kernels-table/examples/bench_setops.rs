//! Benchmark autonomo per i kernel `table.union_distinct`, `table.intersect`
//! e `table.except` (ondata stabilizzazione setops).
//!
//! Fixture identica a quella dello sweep (`bench_sweep.rs`): xorshift64
//! seed logico 42, tabella base a 6 colonne (`id` int64, `num` float64,
//! `grp`/`text`/`path` utf8, `key` int64) e tabella destra con overlap 50%
//! sulle righe intere (righe identiche alla base nell'intervallo
//! [rows/2, rows)).
//!
//! Uso: `bench_setops <rows> <repetitions>` (default `1_000_000`, 3).
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::setops::{except, intersect, union_distinct, SetOperation};
use plenora_kernels_table::Limits;
use serde_json::json;

/// Limiti allargati per le scale di benchmark (come `bench_sweep`).
fn bench_limits() -> Limits {
    Limits {
        max_rows: 40_000_000,
        max_memory_bytes: 6 * 1024 * 1024 * 1024,
        ..Limits::default()
    }
}

/// RNG deterministico (xorshift64, stesso schema di `bench_sweep`).
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

/// Fixture base condivisa: `id` int64, `num` float64, `grp` utf8 (1024
/// gruppi), `text` utf8 (40 char esadecimali), `key` int64 (1M valori
/// distinti possibili), `path` utf8.
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
        std::iter::once(("source".to_owned(), "bench_sweep".to_owned())).collect(),
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

/// Fixture destra per set operation con overlap 50% sulle righe intere:
/// righe identiche alla base nell'intervallo [rows/2, rows). Lo stream
/// xorshift della fixture base (9 draw per riga) e' precalcolato in O(n).
fn setop_right_fixture(rows: usize) -> RecordBatch {
    const DRAWS_PER_ROW: usize = 9;
    let mut base_rng = Rng::seeded();
    let stream = (0..rows * DRAWS_PER_ROW)
        .map(|_| base_rng.next())
        .collect::<Vec<_>>();
    let mut rng = Rng(43); // meta' non sovrapposta: seme diverso
    let mut ids = Vec::with_capacity(rows);
    let mut nums = Vec::with_capacity(rows);
    let mut groups = Vec::with_capacity(rows);
    let mut texts = Vec::with_capacity(rows);
    let mut keys = Vec::with_capacity(rows);
    let mut paths = Vec::with_capacity(rows);
    for row in 0..rows {
        let draws: [u64; DRAWS_PER_ROW] = if row >= rows / 2 {
            let base = row * DRAWS_PER_ROW;
            stream[base..base + DRAWS_PER_ROW].try_into().expect("draws")
        } else {
            [(); DRAWS_PER_ROW].map(|()| rng.next())
        };
        ids.push(i64::try_from(row).ok());
        // Bound evidente: draws[0] % 1_000_000 <= 999_999 < 2^53, cast esatto in f64.
        #[allow(clippy::cast_precision_loss)]
        nums.push(Some((draws[0] % 1_000_000) as f64 / 100.0));
        groups.push(format!("g{}", draws[1] % 1_024));
        texts.push(format!(
            "{:016x}{:016x}{:08x}",
            draws[2],
            draws[3],
            draws[4] & 0xffff_ffff
        ));
        // Bound evidente: draws[5] % 1_000_000 <= 999_999, entra in i64 senza wrap.
        #[allow(clippy::cast_possible_wrap)]
        keys.push((draws[5] % 1_000_000) as i64);
        paths.push(format!(
            "p{:03}/q{:03}/r{:03}",
            draws[6] % 500,
            draws[7] % 500,
            draws[8] % 500
        ));
    }
    let schema = base_fixture(1).schema();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(groups)),
            Arc::new(StringArray::from(texts)),
            Arc::new(Int64Array::from(keys)),
            Arc::new(StringArray::from(paths)),
        ],
    )
    .expect("fixture setop destra")
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
    mut operation: impl FnMut() -> RecordBatch,
) {
    black_box(operation());
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_rows = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = operation();
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
    let limits = bench_limits();
    let config = SetOperation {};

    let left = base_fixture(rows);
    let right = setop_right_fixture(rows);

    run_scenario("union_distinct", rows, repetitions, || {
        union_distinct(&left, &right, &config, &limits).expect("union_distinct")
    });
    run_scenario("intersect", rows, repetitions, || {
        intersect(&left, &right, &config).expect("intersect")
    });
    run_scenario("except", rows, repetitions, || {
        except(&left, &right, &config).expect("except")
    });
}
