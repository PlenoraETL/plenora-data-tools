#![no_main]

use std::sync::Arc;

use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_engine::{execute_batch, Limits, Plan, Step};
use serde_json::json;

fn fixture(payload: &[u8]) -> RecordBatch {
    let bytes = if payload.is_empty() {
        &[0_u8][..]
    } else {
        payload
    };
    let rows = bytes.len().min(64);
    let ids = (0..rows).map(|row| format!("r{row}")).collect::<Vec<_>>();
    let left = bytes
        .iter()
        .take(rows)
        .map(|value| Some(i64::from(*value)))
        .collect::<Vec<_>>();
    let right = bytes
        .iter()
        .rev()
        .take(rows)
        .map(|value| Some(i64::from(*value)))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("a", DataType::Int64, true),
            Field::new("b", DataType::Int64, true),
        ])),
        vec![
            Arc::new(StringArray::from(ids)),
            Arc::new(Int64Array::from(left)),
            Arc::new(Int64Array::from(right)),
        ],
    )
    .expect("bounded reshape fixture")
}

fuzz_target!(|payload: &[u8]| {
    let selector = payload.first().copied().unwrap_or_default() % 4;
    let (operation, config, expected_column, expected_type) = match selector {
        0 => (
            "melt",
            json!({"id_columns":["id"],"value_columns":["a","b"]}),
            2,
            DataType::Int64,
        ),
        1 => ("transpose", json!({"id_column":"id"}), 1, DataType::Int64),
        2 => (
            "melt",
            json!({"id_columns":["b"],"value_columns":["id","a"],"type_policy":"string"}),
            2,
            DataType::Utf8,
        ),
        _ => (
            "transpose",
            json!({"id_column":null,"type_policy":"string"}),
            1,
            DataType::Utf8,
        ),
    };
    let plan = Plan {
        schema_version: 1,
        limits: Limits {
            max_rows: 4_096,
            max_columns: 256,
            max_string_bytes: 1_024,
            max_regex_bytes: 256,
            max_split_columns: 32,
            ..Limits::default()
        },
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .expect("static reshape plan");
    let output = execute_batch(fixture(payload), &plan).expect("reshape safe profile");
    assert!(output.num_rows() <= plan.limits().max_rows);
    assert!(output.num_columns() <= plan.limits().max_columns);
    assert_eq!(output.column(expected_column).data_type(), &expected_type);
    assert!(output
        .columns()
        .iter()
        .all(|column| column.len() == output.num_rows()));
});
