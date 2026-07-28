//! Benchmark autonomo per il kernel `table.fuzzy_join` (estensione v1.3).
//!
//! Fixture deterministica (seed logico 42, LCG): anagrafica destra di nomi
//! sintetici "puliti" e anagrafica sinistra con errori di battitura
//! (trasposizione/sostituzione di un carattere), scenario tipico di
//! riconciliazione per similarita' testuale.
//!
//! Uso: `bench_fuzzy_join <left_rows> <right_rows> <repetitions>`
//! (default 100000 10000 3, lo scenario del report). Emette una riga JSON per
//! scenario con mediana dei tempi, righe/s e peak RSS (`VmHWM`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::fuzzy::{fuzzy_join, FuzzyJoin};
use plenora_kernels_table::Limits;
use serde_json::json;

/// LCG deterministico (Knuth MMIX), seed logico 42.
struct Lcg(u64);

impl Lcg {
    const fn seeded() -> Self {
        Self(42)
    }

    const fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    const fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

const SYLLABLES: &[&str] = &[
    "mar", "ros", "ber", "lan", "gio", "van", "fer", "col", "ric", "fra", "gal", "ben", "pas",
    "lom", "tre", "vis", "zan", "qui", "dor", "mel", "nar", "fos", "bru", "car", "sil", "tam",
    "ver", "oni", "etti", "ini",
];

/// Nome sintetico da tre sillabe (lettera iniziale maiuscola).
fn name_of(seed: u64) -> String {
    let mut lcg = Lcg(seed.wrapping_add(1));
    // Bound evidente: below(n) < n e n = SYLLABLES.len() deriva da usize,
    // quindi il risultato rientra in usize.
    #[allow(clippy::cast_possible_truncation)]
    let pick = |lcg: &mut Lcg| SYLLABLES[lcg.below(SYLLABLES.len() as u64) as usize];
    let first = pick(&mut lcg);
    let mut name = String::with_capacity(12);
    let mut chars = first.chars();
    if let Some(head) = chars.next() {
        name.extend(head.to_uppercase());
    }
    name.push_str(chars.as_str());
    name.push_str(pick(&mut lcg));
    name.push_str(pick(&mut lcg));
    name
}

/// Errore di battitura deterministico: trasposizione di due caratteri
/// adiacenti o sostituzione con una lettera vicina.
fn typo(name: &str, seed: u64) -> String {
    let mut lcg = Lcg(seed.wrapping_add(7));
    let mut chars: Vec<char> = name.chars().collect();
    if chars.len() < 2 {
        return name.to_owned();
    }
    // Bound evidente: below(n) < n e n = chars.len() - 1 deriva da usize,
    // quindi il risultato rientra in usize.
    #[allow(clippy::cast_possible_truncation)]
    let position = lcg.below((chars.len() - 1) as u64) as usize;
    if lcg.below(2) == 0 {
        chars.swap(position, position + 1);
    } else {
        let replacement = (b'a' + u8::try_from(lcg.below(26)).unwrap_or(0)) as char;
        chars[position + 1] = replacement;
    }
    chars.into_iter().collect()
}

fn table(rows: usize, names: &[String], payload_prefix: i64) -> RecordBatch {
    let payload = (0..rows)
        .map(|row| Some(payload_prefix + i64::try_from(row).unwrap_or(0)))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("payload", DataType::Int64, true),
        ])),
        vec![
            Arc::new(StringArray::from(names.to_vec())),
            Arc::new(Int64Array::from(payload)),
        ],
    )
    .expect("fixture")
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

fn config(metric: &str, blocking: &str) -> FuzzyJoin {
    serde_json::from_value(json!({
        "left_key": "name",
        "right_key": "name",
        "metric": metric,
        "threshold": 0.85,
        "blocking": blocking,
        // Prefissi di 2 caratteri su ~30 sillabe iniziali: blocchi da
        // qualche centinaio di righe sul lato destro.
        "max_candidates": 2_000,
    }))
    .expect("config benchmark")
}

fn main() {
    let left_rows: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000);
    let right_rows: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(10_000);
    let repetitions: usize = std::env::args()
        .nth(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    assert!(left_rows > 0 && right_rows > 0 && repetitions > 0);
    let limits = Limits {
        max_rows: 1_000_000_000,
        ..Limits::default()
    };

    // Destra: nomi puliti (con possibili duplicati, come nelle anagrafiche).
    let clean: Vec<String> = (0..right_rows)
        .map(|row| name_of(u64::try_from(row).unwrap_or(0)))
        .collect();
    // Sinistra: ogni riga deriva da un nome destro con un errore di battitura.
    let mut lcg = Lcg::seeded();
    let dirty: Vec<String> = (0..left_rows)
        .map(|row| {
            // Bound evidente: below(n) < n e n = right_rows deriva da usize,
            // quindi il risultato rientra in usize.
            #[allow(clippy::cast_possible_truncation)]
            let source = &clean[lcg.below(right_rows as u64) as usize];
            if lcg.below(4) == 0 {
                source.clone() // ~25% gia' puliti
            } else {
                typo(source, u64::try_from(row).unwrap_or(0))
            }
        })
        .collect();
    let left = table(left_rows, &dirty, 0);
    let right = table(right_rows, &clean, 1_000_000);

    run_scenario("fuzzy_jaro_winkler_prefix", left_rows, repetitions, || {
        fuzzy_join(&left, &right, &config("jaro_winkler", "prefix"), &limits)
            .expect("fuzzy jaro_winkler")
    });
    run_scenario("fuzzy_levenshtein_prefix", left_rows, repetitions, || {
        fuzzy_join(&left, &right, &config("levenshtein", "prefix"), &limits)
            .expect("fuzzy levenshtein")
    });
    run_scenario("fuzzy_jaro_winkler_soundex", left_rows, repetitions, || {
        fuzzy_join(&left, &right, &config("jaro_winkler", "soundex"), &limits)
            .expect("fuzzy soundex")
    });
}
