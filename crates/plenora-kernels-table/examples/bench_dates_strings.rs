//! Benchmark autonomo per i kernel data (`table.date_format`, `table.date_add`,
//! `table.date_diff`, `table.date_extract`, `table.timezone_convert`) e per
//! `table.text_normalize` (secondo batch del filone ottimizzazioni kernel,
//! Fase post-2A).
//!
//! Fixture deterministica (seed logico 42): `ts`/`ts2` utf8 con datetime
//! `%Y-%m-%d %H:%M:%S` distribuiti su anni 2000-2025, `text` utf8 con case
//! misto, accenti e spazi doppi; una riga ogni 97 e' nulla.
//!
//! Uso: `bench_dates_strings <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::dates::{
    date_add, date_diff, date_format, timezone_convert, AmbiguousPolicy, DateAdd, DateDiff,
    DateFormat, DateUnit, DiffUnit, TimezoneConvert,
};
use plenora_kernels_table::strings::{text_normalize, NormalizeOperation, TextNormalize};
use plenora_kernels_table::utility::{date_extract, DateExtract, DatePart, InvalidDatePolicy};
use plenora_kernels_table::Limits;
use serde_json::json;

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

fn timestamp(row: usize, second_shift: usize) -> String {
    let year = row % 26;
    let month = row % 12 + 1;
    let day = row % 28 + 1;
    let hour = row % 24;
    let minute = row % 60;
    let second = (row * 7 + second_shift) % 60;
    format!("20{year:02}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

fn fixture(rows: usize) -> RecordBatch {
    let timestamps = (0..rows)
        .map(|row| (row % 97 != 0).then(|| timestamp(row, 0)))
        .collect::<Vec<_>>();
    let timestamps_end = (0..rows)
        .map(|row| (row % 89 != 0).then(|| timestamp(row, 3_600)))
        .collect::<Vec<_>>();
    let text = (0..rows)
        .map(|row| {
            (row % 97 != 3).then(|| {
                format!(
                    "  {}  {} {}  ",
                    WORDS[row % WORDS.len()],
                    WORDS[(row / 8) % WORDS.len()],
                    row % 1_000
                )
            })
        })
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("ts", DataType::Utf8, true),
            Field::new("ts2", DataType::Utf8, true),
            Field::new("text", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(StringArray::from(timestamps)),
            Arc::new(StringArray::from(timestamps_end)),
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

    let format_config = DateFormat {
        column: "ts".into(),
        input_format: "%Y-%m-%d %H:%M:%S".into(),
        output_format: "%d/%m/%Y %H:%M".into(),
        output_column: "fmt".into(),
        invalid: InvalidDatePolicy::Null,
    };
    run_scenario("date_format", rows, repetitions, &input, |batch| {
        date_format(batch, &format_config).expect("date_format")
    });

    let add_config = DateAdd {
        column: "ts".into(),
        input_format: "%Y-%m-%d %H:%M:%S".into(),
        output_format: "%Y-%m-%d %H:%M:%S".into(),
        amount: 7,
        unit: DateUnit::Days,
        output_column: "shifted".into(),
        invalid: InvalidDatePolicy::Null,
    };
    run_scenario("date_add", rows, repetitions, &input, |batch| {
        date_add(batch, &add_config).expect("date_add")
    });

    let diff_config = DateDiff {
        start_column: "ts".into(),
        end_column: "ts2".into(),
        input_format: "%Y-%m-%d %H:%M:%S".into(),
        unit: DiffUnit::Seconds,
        output_column: "diff".into(),
        invalid: InvalidDatePolicy::Null,
    };
    run_scenario("date_diff", rows, repetitions, &input, |batch| {
        date_diff(batch, &diff_config).expect("date_diff")
    });

    let extract_config = DateExtract {
        column: "ts".into(),
        parts: vec![
            DatePart::Year,
            DatePart::Month,
            DatePart::Day,
            DatePart::Hour,
        ],
        prefix: String::new(),
        date_format: Some("%Y-%m-%d %H:%M:%S".into()),
        invalid: InvalidDatePolicy::Null,
    };
    run_scenario("date_extract", rows, repetitions, &input, |batch| {
        date_extract(batch, &extract_config).expect("date_extract")
    });

    let timezone_config = TimezoneConvert {
        column: "ts".into(),
        input_format: "%Y-%m-%d %H:%M:%S".into(),
        output_format: "%Y-%m-%d %H:%M:%S".into(),
        source_timezone: "Europe/Rome".into(),
        target_timezone: "UTC".into(),
        output_column: "utc".into(),
        invalid: InvalidDatePolicy::Null,
        ambiguous: AmbiguousPolicy::Null,
    };
    run_scenario("timezone_convert", rows, repetitions, &input, |batch| {
        timezone_convert(batch, &timezone_config).expect("timezone_convert")
    });

    let normalize_config = TextNormalize {
        columns: vec!["text".into()],
        operations: NormalizeOperation::Full,
        overwrite: true,
    };
    run_scenario("text_normalize_full", rows, repetitions, &input, |batch| {
        text_normalize(batch, &normalize_config, &Limits::default()).expect("text_normalize")
    });
}
