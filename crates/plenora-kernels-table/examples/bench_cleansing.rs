//! Benchmark autonomo per i kernel `table.fill_na`, `table.coalesce` e
//! `table.type_cast` (filone ottimizzazioni kernel, secondo batch).
//!
//! Fixture deterministica (seed logico 42): stessa forma di
//! `bench_filter_sort` con pattern di null regolari e colonne stringa
//! pronte per i cast (int, float, bool, date).
//!
//! Uso: `bench_cleansing <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::cleansing::{
    fill_na, type_cast, CastErrors, FillMethod, FillNa, TargetType, TypeCast,
};
use plenora_kernels_table::quality::{coalesce, Coalesce};
use serde_json::{json, Value};

const fn nullable(row: usize, modulus: usize, remainder: usize) -> bool {
    row % modulus == remainder
}

fn fixture(rows: usize) -> RecordBatch {
    // Interi con/senza null.
    let to_i64 = |row: usize| i64::try_from(row).expect("fixture rows < i64::MAX");
    let number = |row: usize| f64::from(u32::try_from(row % 10_000).expect("mod")) + 0.5;
    let seq: Vec<i64> = (0..rows).map(to_i64).collect();
    let id = (0..rows)
        .map(|row| (!nullable(row, 7, 3)).then(|| to_i64(row)))
        .collect::<Vec<_>>();
    let fnum: Vec<f64> = (0..rows).map(number).collect();
    let num = (0..rows)
        .map(|row| (!nullable(row, 5, 2)).then(|| number(row)))
        .collect::<Vec<_>>();
    // Stringhe con null (fill/coalesce) e senza null (cast).
    let group = (0..rows)
        .map(|row| (!nullable(row, 11, 4)).then(|| format!("g{}", row % 1_024)))
        .collect::<Vec<_>>();
    let text: Vec<String> = (0..rows).map(|row| format!("{}", row % 100_000)).collect();
    let ftext: Vec<String> = (0..rows)
        .map(|row| {
            if row % 997 == 0 {
                format!("{},{}", row % 1_000, row % 10)
            } else {
                format!("{}.{}", row % 1_000, row % 10)
            }
        })
        .collect();
    let btext: Vec<String> = (0..rows)
        .map(|row| if row % 2 == 0 { "true" } else { "false" }.to_owned())
        .collect();
    let dtext: Vec<String> = (0..rows)
        .map(|row| format!("2024-01-{:02}", row % 28 + 1))
        .collect();
    // Terne di colonne per coalesce (pattern di null sfalsati).
    let ca = (0..rows)
        .map(|row| (!nullable(row, 3, 0)).then(|| to_i64(row)))
        .collect::<Vec<_>>();
    let cb = (0..rows)
        .map(|row| (!nullable(row, 5, 1)).then(|| to_i64(row * 2)))
        .collect::<Vec<_>>();
    let cc = (0..rows)
        .map(|row| (!nullable(row, 7, 2)).then(|| to_i64(row * 3)))
        .collect::<Vec<_>>();
    let sa = (0..rows)
        .map(|row| (!nullable(row, 3, 0)).then(|| format!("a{}", row % 512)))
        .collect::<Vec<_>>();
    let sb = (0..rows)
        .map(|row| (!nullable(row, 5, 1)).then(|| format!("b{}", row % 512)))
        .collect::<Vec<_>>();
    let sc = (0..rows)
        .map(|row| (!nullable(row, 7, 2)).then(|| format!("c{}", row % 512)))
        .collect::<Vec<_>>();

    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("seq", DataType::Int64, false),
            Field::new("id", DataType::Int64, true),
            Field::new("fnum", DataType::Float64, false),
            Field::new("num", DataType::Float64, true),
            Field::new("group", DataType::Utf8, true),
            Field::new("text", DataType::Utf8, false),
            Field::new("ftext", DataType::Utf8, false),
            Field::new("btext", DataType::Utf8, false),
            Field::new("dtext", DataType::Utf8, false),
            Field::new("ca", DataType::Int64, true),
            Field::new("cb", DataType::Int64, true),
            Field::new("cc", DataType::Int64, true),
            Field::new("sa", DataType::Utf8, true),
            Field::new("sb", DataType::Utf8, true),
            Field::new("sc", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(seq)),
            Arc::new(Int64Array::from(id)),
            Arc::new(Float64Array::from(fnum)),
            Arc::new(Float64Array::from(num)),
            Arc::new(StringArray::from(group)),
            Arc::new(StringArray::from(text)),
            Arc::new(StringArray::from(ftext)),
            Arc::new(StringArray::from(btext)),
            Arc::new(StringArray::from(dtext)),
            Arc::new(Int64Array::from(ca)),
            Arc::new(Int64Array::from(cb)),
            Arc::new(Int64Array::from(cc)),
            Arc::new(StringArray::from(sa)),
            Arc::new(StringArray::from(sb)),
            Arc::new(StringArray::from(sc)),
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

fn cast(column: &str, target_type: TargetType) -> TypeCast {
    TypeCast {
        column: column.into(),
        target_type,
        date_format: String::new(),
        errors: CastErrors::Coerce,
        precision: None,
        scale: None,
        timezone: None,
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
    let input = fixture(rows);

    let fill_id = FillNa {
        column: Some("id".into()),
        method: FillMethod::Value,
        value: Value::from(0),
    };
    run_scenario("fill_na_int_value", rows, repetitions, &input, |batch| {
        fill_na(batch, &fill_id).expect("fill id")
    });

    let fill_num = FillNa {
        column: Some("num".into()),
        method: FillMethod::Ffill,
        value: Value::Null,
    };
    run_scenario("fill_na_float_ffill", rows, repetitions, &input, |batch| {
        fill_na(batch, &fill_num).expect("fill num")
    });

    let fill_group = FillNa {
        column: Some("group".into()),
        method: FillMethod::Value,
        value: Value::from("n/d"),
    };
    run_scenario("fill_na_utf8_value", rows, repetitions, &input, |batch| {
        fill_na(batch, &fill_group).expect("fill group")
    });

    let fill_group_ffill = FillNa {
        column: Some("group".into()),
        method: FillMethod::Ffill,
        value: Value::Null,
    };
    run_scenario("fill_na_utf8_ffill", rows, repetitions, &input, |batch| {
        fill_na(batch, &fill_group_ffill).expect("fill group ffill")
    });

    let coalesce_int = Coalesce {
        columns: vec!["ca".into(), "cb".into(), "cc".into()],
        output_column: "coalesced_int".into(),
    };
    run_scenario("coalesce_int64", rows, repetitions, &input, |batch| {
        coalesce(batch, &coalesce_int).expect("coalesce int")
    });

    let coalesce_str = Coalesce {
        columns: vec!["sa".into(), "sb".into(), "sc".into()],
        output_column: "coalesced_str".into(),
    };
    run_scenario("coalesce_utf8", rows, repetitions, &input, |batch| {
        coalesce(batch, &coalesce_str).expect("coalesce str")
    });

    let cast_text_int = cast("text", TargetType::Int);
    run_scenario("type_cast_utf8_int", rows, repetitions, &input, |batch| {
        type_cast(batch, &cast_text_int).expect("cast text->int")
    });

    let cast_ftext_float = cast("ftext", TargetType::Float);
    run_scenario("type_cast_utf8_float", rows, repetitions, &input, |batch| {
        type_cast(batch, &cast_ftext_float).expect("cast ftext->float")
    });

    let cast_btext_bool = cast("btext", TargetType::Bool);
    run_scenario("type_cast_utf8_bool", rows, repetitions, &input, |batch| {
        type_cast(batch, &cast_btext_bool).expect("cast btext->bool")
    });

    let cast_dtext_date32 = cast("dtext", TargetType::Date32);
    run_scenario(
        "type_cast_utf8_date32",
        rows,
        repetitions,
        &input,
        |batch| type_cast(batch, &cast_dtext_date32).expect("cast dtext->date32"),
    );

    let cast_seq_str = cast("seq", TargetType::Str);
    run_scenario("type_cast_int_utf8", rows, repetitions, &input, |batch| {
        type_cast(batch, &cast_seq_str).expect("cast seq->str")
    });

    let cast_fnum_int = cast("fnum", TargetType::Int);
    run_scenario("type_cast_float_int", rows, repetitions, &input, |batch| {
        type_cast(batch, &cast_fnum_int).expect("cast fnum->int")
    });
}
