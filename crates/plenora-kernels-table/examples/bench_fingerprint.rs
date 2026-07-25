//! Benchmark autonomo per i kernel `table.stable_fingerprint` e
//! `table.hmac_sha256` (filone ottimizzazioni kernel, sweep2 candidati 1-2).
//!
//! Fixture deterministica: IDENTICA a `base_fixture` di `bench_sweep.rs` /
//! `bench_sweep2.rs` (seed logico 42 via xorshift64*, 9 draw per riga, 6
//! colonne id/num/grp/text/key/path) per confrontabilita' diretta con le
//! misure dello sweep (1.5M righe/s baseline su entrambe le op).
//!
//! Uso: `bench_fingerprint <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::security::{
    hmac_sha256, stable_fingerprint, FingerprintAlgorithm, HmacNullPolicy, HmacSha256,
    StableFingerprint,
};
use serde_json::json;

/// Variabile d'ambiente con la chiave HMAC del benchmark (impostata in main).
const HMAC_KEY_ENV: &str = "PLENORA_BENCH_HMAC_KEY";

/// RNG deterministico (xorshift64*, identico a `bench_sweep`/`bench_sweep2`).
struct Rng(u64);

impl Rng {
    fn seeded() -> Self {
        Self(42)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Fixture base: identica a `base_fixture` di `bench_sweep2.rs` (seed 42).
fn fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng::seeded();
    let mut ids = Vec::with_capacity(rows);
    let mut nums = Vec::with_capacity(rows);
    let mut groups = Vec::with_capacity(rows);
    let mut texts = Vec::with_capacity(rows);
    let mut keys = Vec::with_capacity(rows);
    let mut paths = Vec::with_capacity(rows);
    for row in 0..rows {
        ids.push(i64::try_from(row).ok());
        nums.push(Some((rng.next() % 1_000_000) as f64 / 100.0));
        groups.push(format!("g{}", rng.next() % 1_024));
        texts.push(format!(
            "{:016x}{:016x}{:08x}",
            rng.next(),
            rng.next(),
            rng.next() & 0xffff_ffff
        ));
        keys.push((rng.next() % 1_000_000) as i64);
        paths.push(format!(
            "p{:03}/q{:03}/r{:03}",
            rng.next() % 500,
            rng.next() % 500,
            rng.next() % 500
        ));
    }
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("num", DataType::Float64, false),
            Field::new("grp", DataType::Utf8, false),
            Field::new("text", DataType::Utf8, false),
            Field::new("key", DataType::Int64, false),
            Field::new("path", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(nums)),
            Arc::new(StringArray::from(groups)),
            Arc::new(StringArray::from(texts)),
            Arc::new(Int64Array::from(keys)),
            Arc::new(StringArray::from(paths)),
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
    input: &RecordBatch,
    execute: impl Fn(&RecordBatch) -> RecordBatch,
) {
    black_box(execute(input));
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_rows = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = execute(input);
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
    let mut args = std::env::args().skip(1);
    let rows: usize = args
        .next()
        .as_deref()
        .unwrap_or("1000000")
        .parse()
        .expect("rows");
    let repetitions: usize = args.next().as_deref().unwrap_or("3").parse().expect("reps");
    // Chiave HMAC del benchmark: solo il NOME della variabile entra nella
    // config del kernel, il valore resta fuori da piano e output.
    std::env::set_var(HMAC_KEY_ENV, "bench-fingerprint-hmac-key-seed-42");
    let input = fixture(rows);

    // Stessa config dello sweep2: sha256 su tutte e 6 le colonne.
    let fingerprint_sha256 = StableFingerprint {
        columns: Vec::new(),
        output_column: "fingerprint".into(),
        algorithm: FingerprintAlgorithm::Sha256,
    };
    run_scenario(
        "stable_fingerprint_sha256_all6",
        rows,
        repetitions,
        &input,
        |batch| stable_fingerprint(batch, &fingerprint_sha256).expect("fingerprint sha256"),
    );

    let fingerprint_md5 = StableFingerprint {
        algorithm: FingerprintAlgorithm::Md5,
        ..fingerprint_sha256
    };
    run_scenario(
        "stable_fingerprint_md5_all6",
        rows,
        repetitions,
        &input,
        |batch| stable_fingerprint(batch, &fingerprint_md5).expect("fingerprint md5"),
    );

    // Stessa config dello sweep2: 2 colonne (id+text), chiave da env.
    let hmac_config = HmacSha256 {
        columns: vec!["id".into(), "text".into()],
        key_env: HMAC_KEY_ENV.into(),
        output_column: "hmac".into(),
        null_policy: HmacNullPolicy::Empty,
    };
    run_scenario("hmac_sha256_id_text", rows, repetitions, &input, |batch| {
        hmac_sha256(batch, &hmac_config).expect("hmac")
    });
}
