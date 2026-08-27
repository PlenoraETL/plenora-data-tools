//! Benchmark autonomo per i kernel `table.distinct`, `table.dedup_advanced`,
//! `table.window_function` e `table.rolling_window` (filone ottimizzazioni
//! kernel, batch 4, guidato dallo sweep tabellare
//! `benchmarks/sweep/sweep.json`).
//!
//! Stessa fixture e stessi scenari di `bench_sweep` (seed logico 42 via
//! xorshift, 6 colonne: id/num/grp/text/key/path), stesse scale dello sweep
//! (`distinct` a 10M, le altre a 1M), mediana di 3 run, righe/s e peak RSS
//! (`VmHWM` da `/proc/self/status`): i numeri sono confrontabili con la
//! baseline di `benchmarks/sweep/sweep.json`.
//!
//! Uso: `bench_window` — stampa una riga JSON per scenario.

#[path = "comune/mod.rs"]
mod comune;

use comune::fixture::base_fixture;

use comune::measure;

use std::sync::OnceLock;

use plenora_core::arrow::array::RecordBatch;
use plenora_kernels_table::aggregation::{
    dedup_advanced, distinct, rolling_window, window_function, DedupAdvanced, Distinct, Keep,
    RollingKind, RollingWindow, WindowFunction, WindowKind,
};

const M1: usize = 1_000_000;
const M10: usize = 10_000_000;

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
