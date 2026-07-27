//! Benchmark autonomo per il kernel `table.string_extract` (ultimo batch del
//! filone ottimizzazioni kernel).
//!
//! Fixture deterministica (seed 42, xorshift64): colonna `code` utf8 con
//! codici tipo `LO2244_FV01_II01_GEO001` (una riga ogni 97 nulla, una ogni 89
//! senza match) e colonna `text` utf8 con unicode (accenti, emoji) e numeri
//! ripetuti per lo scenario `extract_all`.
//!
//! Uso: `bench_string_extract <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::strings::{string_extract, StringExtract};
use plenora_kernels_table::Limits;
use serde_json::json;

/// xorshift64* deterministico, seed 42: decide null/match e varia i suffissi.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

const WORDS: [&str; 8] = [
    "Café",
    "ÉLÈVE",
    "straße",
    "naïve",
    "München",
    "Ångström",
    "curaçao",
    "Æsop",
];

fn fixture(rows: usize) -> RecordBatch {
    let mut rng = Rng(42);
    let codes = (0..rows)
        .map(|_| {
            let roll = rng.next();
            if roll.is_multiple_of(97) {
                return None;
            }
            let code = if roll.is_multiple_of(89) {
                // Riga senza match: nessun gruppo numerico finale.
                format!("LO{:04}_FVXX_IIYY_GEOZZZ", roll % 10_000)
            } else {
                format!(
                    "LO{:04}_FV{:02}_II{:02}_GEO{:03}",
                    roll % 10_000,
                    roll % 90,
                    (roll / 7) % 90,
                    roll % 1_000,
                )
            };
            Some(code)
        })
        .collect::<Vec<_>>();
    let text = (0..rows)
        .map(|row| {
            let roll = rng.next();
            if roll % 97 == 3 {
                return None;
            }
            Some(format!(
                "{} 🎉 {} ord-{}-{}-{} {}",
                WORDS[(roll % 8) as usize],
                WORDS[((roll / 8) % 8) as usize],
                roll % 1_000,
                (roll / 13) % 1_000,
                (roll / 17) % 1_000,
                row % 1_000,
            ))
        })
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("code", DataType::Utf8, true),
            Field::new("text", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(StringArray::from(codes)),
            Arc::new(StringArray::from(text)),
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
    let rows: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let repetitions: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    assert!(rows > 0 && repetitions > 0);
    let input = fixture(rows);

    // Pattern semplice: un gruppo anonimo su un suffisso numerico.
    let simple = StringExtract {
        column: "code".into(),
        pattern: "GEO(\\d{3})".into(),
        output_column: Some("simple".into()),
        extract_all: false,
    };
    run_scenario("string_extract_simple", rows, repetitions, &input, |batch| {
        string_extract(batch, &simple, &Limits::default()).expect("simple")
    });

    // Pattern complesso: gruppi nominati multipli (una colonna per gruppo).
    let named = StringExtract {
        column: "code".into(),
        pattern: "(?P<site>LO\\d{4})_(?P<area>[A-Z]{2}\\d{2})_(?P<system>[A-Z]{2}\\d{2})_GEO(?P<num>\\d{3})".into(),
        output_column: None,
        extract_all: false,
    };
    run_scenario("string_extract_named", rows, repetitions, &input, |batch| {
        string_extract(batch, &named, &Limits::default()).expect("named")
    });

    // extract_all su testo unicode con numeri ripetuti (join con virgola).
    let extract_all = StringExtract {
        column: "text".into(),
        pattern: "(\\d+)".into(),
        output_column: Some("all".into()),
        extract_all: true,
    };
    run_scenario(
        "string_extract_all_unicode",
        rows,
        repetitions,
        &input,
        |batch| string_extract(batch, &extract_all, &Limits::default()).expect("extract_all"),
    );
}
