//! Benchmark autonomo per i kernel `table.distinct`, `table.dedup_advanced`,
//! `table.window_function` e `table.rolling_window`, guidato dallo sweep
//! tabellare `benchmarks/sweep/sweep.json`.
//!
//! Stessa fixture e stessi scenari di `bench_sweep` (seed logico 42 via
//! xorshift, 6 colonne: id/num/grp/text/key/path), stesse scale dello sweep
//! (`distinct` a 10M, le altre a 1M), mediana di 3 run, righe/s e peak RSS
//! (`VmHWM` da `/proc/self/status`): i numeri sono confrontabili con la
//! baseline di `benchmarks/sweep/sweep.json`.
//!
//! In piu', `table.window_function` rank **senza** `group_by` a 1M e 5M su
//! una fixture stretta come quella del catalogo di memoria (`id` Int64,
//! `value` Float64 = `riga % 100 + 0.5`): una sola partizione con molti
//! pari merito, il caso in cui il rango per riga costava due ricerche
//! binarie.
//!
//! Uso: `bench_window` — stampa una riga JSON per scenario.

#[path = "comune/mod.rs"]
mod comune;

use comune::fixture::base_fixture;

use comune::measure;

use std::sync::OnceLock;

use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::aggregation::{
    dedup_advanced, distinct, rolling_window, window_function, DedupAdvanced, Distinct, Keep,
    RollingKind, RollingWindow, WindowFunction, WindowKind,
};

const M1: usize = 1_000_000;
const M5: usize = 5_000_000;
const M10: usize = 10_000_000;

static BASE_1M: OnceLock<RecordBatch> = OnceLock::new();
static BASE_10M: OnceLock<RecordBatch> = OnceLock::new();

fn base_1m() -> &'static RecordBatch {
    BASE_1M.get_or_init(|| base_fixture(M1))
}

fn base_10m() -> &'static RecordBatch {
    BASE_10M.get_or_init(|| base_fixture(M10))
}

/// Fixture stretta del catalogo di memoria: 100 valori distinti.
fn fixture_stretta(rows: usize) -> RecordBatch {
    let ids = (0..rows)
        .map(|row| i64::try_from(row).expect("riga in i64"))
        .collect::<Vec<_>>();
    let values = (0..rows)
        .map(|row| f64::from(u32::try_from(row % 100).expect("resto < 100")) + 0.5)
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("value", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(values)),
        ],
    )
    .expect("fixture stretta")
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

    let rank_config = WindowFunction {
        column: "value".into(),
        function: WindowKind::Rank,
        group_by: None,
        order_column: None,
        offset: 1,
        buckets: None,
        output_column: Some("ranked".into()),
    };
    for rows in [M1, M5] {
        let batch = fixture_stretta(rows);
        measure(
            "table.window_function",
            rows,
            3,
            "rank senza group_by, 100 valori distinti",
            || window_function(&batch, &rank_config).expect("window_function"),
        );
    }

    eprintln!("bench_window completato: 6 scenari");
}
