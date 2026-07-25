//! Benchmark autonomo per i kernel `table.formula` e `table.expression`
//! (filone ottimizzazioni kernel, ultimo batch: interpreti AST).
//!
//! Fixture deterministica (seed logico 42, LCG): `num`/`other` float64,
//! `val` int64, `name`/`code` utf8.
//!
//! Uso: `bench_formula <rows> <repetitions>`
//! Emette una riga JSON per scenario con mediana dei tempi, righe/s e
//! peak RSS (`VmHWM` da `/proc/self/status`).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::expressions::{self, ExpressionTransform};
use plenora_kernels_table::formula::{self, Formula};
use serde_json::json;

/// PRNG deterministico (LCG Knuth), seed 42: riproduce la stessa fixture
/// a ogni run senza dipendenze esterne.
struct Lcg(u64);

impl Lcg {
    fn next_f64(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        #[allow(clippy::cast_precision_loss)]
        let value = (self.0 >> 11) as f64 / (1_u64 << 53) as f64;
        value
    }
}

fn fixture(rows: usize) -> RecordBatch {
    let mut rng = Lcg(42);
    let mut num = Vec::with_capacity(rows);
    let mut other = Vec::with_capacity(rows);
    let mut val = Vec::with_capacity(rows);
    let mut name = Vec::with_capacity(rows);
    let mut code = Vec::with_capacity(rows);
    for row in 0..rows {
        num.push(rng.next_f64() * 10_000.0);
        other.push(rng.next_f64() * 100.0);
        #[allow(clippy::cast_possible_truncation)]
        val.push((rng.next_f64() * 100_000.0) as i64);
        name.push(format!("item{:06}", row % 100_000));
        code.push(format!("c{:03}", row % 500));
    }
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("num", DataType::Float64, false),
            Field::new("other", DataType::Float64, false),
            Field::new("val", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("code", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Float64Array::from(num)),
            Arc::new(Float64Array::from(other)),
            Arc::new(Int64Array::from(val)),
            Arc::new(StringArray::from(name)),
            Arc::new(StringArray::from(code)),
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
    run: &dyn Fn(&RecordBatch) -> RecordBatch,
) {
    black_box(run(input));
    let mut durations = Vec::with_capacity(repetitions);
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = run(input);
        durations.push(start.elapsed().as_secs_f64());
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
            "peak_rss_kib": peak_rss_kib(),
        }))
        .expect("JSON")
    );
}

fn run_formula(input: &RecordBatch, text: &str) -> RecordBatch {
    let config = Formula {
        new_column: "out".into(),
        formula: text.into(),
    };
    formula::formula(input, &config).expect("formula")
}

fn run_expression(input: &RecordBatch, expression: serde_json::Value) -> RecordBatch {
    let config: ExpressionTransform = serde_json::from_value(json!({
        "output_column": "out",
        "expression": expression,
    }))
    .expect("config expression");
    expressions::expression(input, &config).expect("expression")
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

    // 1) formula aritmetica pura (tier numerico del fast path).
    run_scenario("formula_arith", rows, repetitions, &input, &|batch| {
        run_formula(batch, "num * 2.5 + other / 4 - val")
    });

    // 2) formula mista con concatenazione testuale via `+`.
    run_scenario("formula_mixed_strings", rows, repetitions, &input, &|batch| {
        run_formula(batch, "name + '-' + code")
    });

    // 3) expression aritmetica: add(mul(num, 2.5), subtract(other, val)).
    let arith = json!({
        "kind": "binary", "op": "add",
        "left": {
            "kind": "binary", "op": "multiply",
            "left": {"kind": "column", "name": "num"},
            "right": {"kind": "literal", "value": 2.5}
        },
        "right": {
            "kind": "binary", "op": "subtract",
            "left": {"kind": "column", "name": "other"},
            "right": {"kind": "column", "name": "val"}
        }
    });
    run_scenario("expression_arith", rows, repetitions, &input, &|batch| {
        run_expression(batch, arith.clone())
    });

    // 4) expression mista con stringhe: case su contains + upper/concat.
    let mixed = json!({
        "kind": "case",
        "branches": [{
            "when": {
                "kind": "function", "name": "contains",
                "args": [
                    {"kind": "column", "name": "name"},
                    {"kind": "literal", "value": "42"}
                ]
            },
            "then": {
                "kind": "function", "name": "upper",
                "args": [{"kind": "column", "name": "name"}]
            }
        }],
        "else_value": {
            "kind": "function", "name": "concat",
            "args": [
                {"kind": "column", "name": "name"},
                {"kind": "literal", "value": "-"},
                {"kind": "column", "name": "code"}
            ]
        }
    });
    run_scenario("expression_mixed_strings", rows, repetitions, &input, &|batch| {
        run_expression(batch, mixed.clone())
    });
}
