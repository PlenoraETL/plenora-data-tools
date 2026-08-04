//! Benchmark autonomo per i kernel `table.distinct`, `table.dedup_advanced`,
//! `table.window_function` e `table.rolling_window` (filone ottimizzazioni
//! kernel, batch 4, guidato da `benchmarks/sweep/sweep.md`).
//!
//! Stessa fixture e stessi scenari di `bench_sweep` (seed logico 42 via
//! xorshift, 6 colonne: id/num/grp/text/key/path), stesse scale dello sweep
//! (`distinct` a 10M, le altre a 1M), mediana di 3 run, righe/s e peak RSS
//! (`VmHWM` da `/proc/self/status`): i numeri sono confrontabili con la
//! baseline di `benchmarks/sweep/sweep.json`.
//!
//! Uso: `bench_window` — stampa una riga JSON per scenario.

use std::hint::black_box;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::aggregation::{
    dedup_advanced, distinct, rolling_window, window_function, DedupAdvanced, Distinct, Keep,
    RollingKind, RollingWindow, WindowFunction, WindowKind,
};
use serde_json::json;

const M1: usize = 1_000_000;
const M10: usize = 10_000_000;

/// RNG deterministico (xorshift64*, identico a `bench_sweep`).
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

/// Fixture base dello sweep, copiata verbatim: `id` int64, `num` float64,
/// `grp` utf8 (1024 gruppi), `text` utf8 (40 char), `key` int64 (~1M
/// distinti), `path` utf8.
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

static BASE_1M: OnceLock<RecordBatch> = OnceLock::new();
static BASE_10M: OnceLock<RecordBatch> = OnceLock::new();

fn base_1m() -> &'static RecordBatch {
    BASE_1M.get_or_init(|| base_fixture(M1))
}

fn base_10m() -> &'static RecordBatch {
    BASE_10M.get_or_init(|| base_fixture(M10))
}

fn main() {
    let distinct_config = Distinct {
        subset: vec!["key".into()],
        keep: Keep::First,
    };
    measure(
        "table.distinct",
        M10,
        3,
        "subset key, ~1M valori distinti su 10M righe [10M]",
        || distinct(base_10m(), &distinct_config).expect("distinct"),
    );

    let dedup_config = DedupAdvanced {
        subset: vec!["key".into()],
        keep: Keep::First,
        order_column: Some("id".into()),
        ascending: true,
    };
    measure(
        "table.dedup_advanced",
        M1,
        3,
        "subset key, order id",
        || dedup_advanced(base_1m(), &dedup_config).expect("dedup_advanced"),
    );

    let rolling_config = RollingWindow {
        column: "num".into(),
        function: RollingKind::Mean,
        group_by: Some("grp".into()),
        order_column: Some("id".into()),
        window: 10,
        min_periods: 1,
        ddof: 1,
        output_column: "num_roll".into(),
    };
    measure(
        "table.rolling_window",
        M1,
        3,
        "mean w=10, partizione grp",
        || rolling_window(base_1m(), &rolling_config).expect("rolling_window"),
    );

    let window_config = WindowFunction {
        column: "num".into(),
        function: WindowKind::Rank,
        group_by: Some("grp".into()),
        order_column: Some("num".into()),
        offset: 1,
        buckets: None,
        output_column: Some("num_rank".into()),
    };
    measure(
        "table.window_function",
        M1,
        3,
        "rank, partizione grp, order num",
        || window_function(base_1m(), &window_config).expect("window_function"),
    );

    eprintln!("bench_window completato: 4 scenari");
}
