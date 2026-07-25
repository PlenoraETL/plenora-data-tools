//! Benchmark autonomo per i kernel `table.join`, `table.semi_join` e
//! `table.anti_join` (filone ottimizzazioni kernel, terzo batch).
//!
//! Fixture deterministica (seed logico 42, LCG): tabella sinistra con chiave
//! `k` (Int64/UInt64/Float64/Utf8 a seconda dello scenario) piu' payload
//! `lv` int64 e `lt` utf8, tabella destra con chiave `k` e payload `rv`.
//!
//! Uso: `bench_joins <left_rows> <right_rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::joins::{anti_join, join, semi_join, Join, JoinHow, MembershipJoin};
use plenora_kernels_table::Limits;
use serde_json::json;

/// LCG deterministico (Knuth MMIX), seed logico 42.
struct Lcg(u64);

impl Lcg {
    fn seeded() -> Self {
        Self(42)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

fn left_table(rows: usize, keys: ArrayRef) -> RecordBatch {
    let payload = (0..rows)
        .map(|row| i64::try_from(row).ok())
        .collect::<Vec<_>>();
    let tags = (0..rows).map(|row| format!("t{}", row % 97)).collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", keys.data_type().clone(), false),
            Field::new("lv", DataType::Int64, false),
            Field::new("lt", DataType::Utf8, false),
        ])),
        vec![keys, Arc::new(Int64Array::from(payload)), Arc::new(StringArray::from(tags))],
    )
    .expect("fixture sinistra")
}

fn right_table(rows: usize, keys: ArrayRef) -> RecordBatch {
    let payload = (0..rows)
        .map(|row| i64::try_from(row).ok())
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", keys.data_type().clone(), false),
            Field::new("rv", DataType::Int64, false),
        ])),
        vec![keys, Arc::new(Int64Array::from(payload))],
    )
    .expect("fixture destra")
}

fn int64_keys(rows: usize, key_space: u64) -> ArrayRef {
    let mut rng = Lcg::seeded();
    let keys = (0..rows)
        .map(|_| i64::try_from(rng.below(key_space)).ok())
        .collect::<Vec<_>>();
    Arc::new(Int64Array::from(keys))
}

fn utf8_keys(rows: usize, key_space: u64) -> ArrayRef {
    let mut rng = Lcg::seeded();
    let keys = (0..rows)
        .map(|_| format!("k{:07}", rng.below(key_space)))
        .collect::<Vec<_>>();
    Arc::new(StringArray::from(keys))
}

fn float64_keys(rows: usize, key_space: u64) -> ArrayRef {
    let mut rng = Lcg::seeded();
    let keys = (0..rows)
        .map(|_| {
            #[allow(clippy::cast_precision_loss)]
            Some((rng.below(key_space) as f64) + 0.5)
        })
        .collect::<Vec<_>>();
    Arc::new(Float64Array::from(keys))
}

fn join_config(how: JoinHow) -> Join {
    Join {
        left_keys: vec!["k".into()],
        right_keys: vec!["k".into()],
        how,
    }
}

fn membership_config() -> MembershipJoin {
    MembershipJoin {
        left_keys: vec!["k".into()],
        right_keys: vec!["k".into()],
    }
}

fn limits() -> Limits {
    Limits {
        max_rows: 1_000_000_000,
        ..Limits::default()
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
    let left_rows: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(10_000_000);
    let right_rows: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let repetitions: usize = std::env::args()
        .nth(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    assert!(left_rows > 0 && right_rows > 0 && repetitions > 0);
    let limits = limits();

    // Chiavi int64 a bassa duplicazione: destra univoca 0..right_rows,
    // sinistra uniforme su uno spazio il 25% piu' ampio (~80% di match).
    let key_space = u64::try_from(right_rows).expect("right_rows") * 5 / 4;
    let left_i64 = left_table(left_rows, int64_keys(left_rows, key_space));
    let right_i64 = right_table(
        right_rows,
        Arc::new(Int64Array::from(
            (0..right_rows)
                .map(|row| i64::try_from(row).ok())
                .collect::<Vec<_>>(),
        )),
    );

    run_scenario("join_inner_int64", left_rows, repetitions, || {
        join(&left_i64, &right_i64, &join_config(JoinHow::Inner), &limits).expect("join inner")
    });
    run_scenario("join_left_int64", left_rows, repetitions, || {
        join(&left_i64, &right_i64, &join_config(JoinHow::Left), &limits).expect("join left")
    });
    run_scenario("semi_join_int64", left_rows, repetitions, || {
        semi_join(&left_i64, &right_i64, &membership_config()).expect("semi join")
    });
    run_scenario("anti_join_int64", left_rows, repetitions, || {
        anti_join(&left_i64, &right_i64, &membership_config()).expect("anti join")
    });

    // Chiavi int64 ad alta duplicazione: 10_000 chiavi distinte, destra
    // piccola e univoca (output = left_rows righe).
    let left_dup = left_table(left_rows, int64_keys(left_rows, 10_000));
    let right_dup = right_table(
        10_000,
        Arc::new(Int64Array::from(
            (0..10_000_i64).map(Some).collect::<Vec<_>>(),
        )),
    );
    run_scenario("join_inner_int64_highdup", left_rows, repetitions, || {
        join(&left_dup, &right_dup, &join_config(JoinHow::Inner), &limits).expect("join highdup")
    });

    // Chiavi utf8 e float64, stessa distribuzione della bassa duplicazione.
    let left_utf8 = left_table(left_rows, utf8_keys(left_rows, key_space));
    let right_utf8 = right_table(
        right_rows,
        Arc::new(StringArray::from(
            (0..right_rows)
                .map(|row| format!("k{row:07}"))
                .collect::<Vec<_>>(),
        )),
    );
    run_scenario("join_inner_utf8", left_rows, repetitions, || {
        join(&left_utf8, &right_utf8, &join_config(JoinHow::Inner), &limits).expect("join utf8")
    });

    let left_f64 = left_table(left_rows, float64_keys(left_rows, key_space));
    let right_f64 = right_table(
        right_rows,
        Arc::new(Float64Array::from(
            (0..right_rows)
                .map(|row| {
                    #[allow(clippy::cast_precision_loss)]
                    Some(row as f64 + 0.5)
                })
                .collect::<Vec<_>>(),
        )),
    );
    run_scenario("join_inner_float64", left_rows, repetitions, || {
        join(&left_f64, &right_f64, &join_config(JoinHow::Inner), &limits).expect("join float64")
    });
}
