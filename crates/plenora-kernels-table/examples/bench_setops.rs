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

#[path = "comune/mod.rs"]
mod comune;

use comune::fixture::{base_fixture, setop_right_fixture};

use comune::run_scenario;

use plenora_kernels_table::setops::{except, intersect, union_distinct, SetOperation};
use plenora_kernels_table::Limits;

/// Limiti allargati per le scale di benchmark (come `bench_sweep`).
fn bench_limits() -> Limits {
    Limits {
        max_rows: 40_000_000,
        max_governed_memory_bytes: 6 * 1024 * 1024 * 1024,
        ..Limits::default()
    }
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
