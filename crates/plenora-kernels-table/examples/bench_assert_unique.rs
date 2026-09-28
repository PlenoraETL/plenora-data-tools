//! Benchmark autonomo per `table.assert_unique` (baseline
//! kernel: quality).
//!
//! Fixture deterministica IDENTICA a `bench_sweep2` (seed logico 42,
//! xorshift64, stesse colonne), cosi' lo scenario `unique_id` e'
//! confrontabile con la baseline di `benchmarks/sweep/sweep2.json`
//! (`table.assert_unique`, "chiave id unica", 5.03M righe/s).
//!
//! Scenari (duplicati in posizioni diverse, piu' la scansione completa):
//! - `unique_id`: chiave `id` int64 unica, nessun duplicato (full scan);
//! - `dup_adjacent_first`: duplicato alle righe 0 e 1 (fail-fast);
//! - `dup_middle`: duplicato della riga 0 a meta' batch;
//! - `dup_last`: duplicato della riga 0 all'ultima riga (caso peggiore);
//! - `unique_multicol`: chiave (`id`, `grp`) unica, due colonne.
//!
//! Uso: `bench_assert_unique <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

#[path = "comune/mod.rs"]
mod comune;

use comune::peak_rss_kib;
use comune::rng::Rng;

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::quality::{assert_unique, AssertUnique};
use serde_json::json;

/// Fixture base condivisa: identica a `bench_sweep2::base_fixture`.
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
        std::iter::once(("source".to_owned(), "bench_assert_unique".to_owned())).collect(),
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

/// Variante con la chiave `id` della riga `at` forzata uguale alla riga 0.
fn fixture_with_duplicate(rows: usize, at: usize) -> RecordBatch {
    let batch = base_fixture(rows);
    let ids = batch
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("colonna id int64");
    let mut values: Vec<Option<i64>> = (0..rows).map(|row| Some(ids.value(row))).collect();
    values[at] = values[0];
    RecordBatch::try_new(
        batch.schema(),
        vec![
            Arc::new(Int64Array::from(values)),
            batch.column(1).clone(),
            batch.column(2).clone(),
            batch.column(3).clone(),
            batch.column(4).clone(),
            batch.column(5).clone(),
        ],
    )
    .expect("fixture con duplicato")
}

fn measure(
    op: &'static str,
    rows: usize,
    repetitions: usize,
    note: &str,
    execute: impl Fn() -> plenora_core::Result<RecordBatch>,
) {
    let _ = black_box(execute());
    let mut durations = Vec::with_capacity(repetitions);
    let mut outcome = String::from("ok");
    for _ in 0..repetitions {
        let start = Instant::now();
        match execute() {
            Ok(output) => {
                durations.push(start.elapsed().as_secs_f64());
                black_box(output);
            }
            Err(error) => {
                durations.push(start.elapsed().as_secs_f64());
                outcome = error.to_string();
            }
        }
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
        "outcome": outcome,
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

    let unique = base_fixture(rows);
    let unique_config = AssertUnique {
        columns: vec!["id".into()],
        nulls_equal: true,
    };
    measure(
        "assert_unique/unique_id",
        rows,
        repetitions,
        "chiave id unica (full scan)",
        || assert_unique(&unique, &unique_config),
    );

    let multicol_config = AssertUnique {
        columns: vec!["id".into(), "grp".into()],
        nulls_equal: true,
    };
    measure(
        "assert_unique/unique_multicol",
        rows,
        repetitions,
        "chiave (id, grp) unica, due colonne",
        || assert_unique(&unique, &multicol_config),
    );

    if rows >= 2 {
        let dup_first = fixture_with_duplicate(rows, 1);
        measure(
            "assert_unique/dup_adjacent_first",
            rows,
            repetitions,
            "duplicato righe 0 e 1 (fail-fast)",
            || assert_unique(&dup_first, &unique_config),
        );

        let dup_middle = fixture_with_duplicate(rows, rows / 2);
        measure(
            "assert_unique/dup_middle",
            rows,
            repetitions,
            "duplicato della riga 0 a meta' batch",
            || assert_unique(&dup_middle, &unique_config),
        );

        let dup_last = fixture_with_duplicate(rows, rows - 1);
        measure(
            "assert_unique/dup_last",
            rows,
            repetitions,
            "duplicato della riga 0 all'ultima riga (caso peggiore)",
            || assert_unique(&dup_last, &unique_config),
        );
    }
}
