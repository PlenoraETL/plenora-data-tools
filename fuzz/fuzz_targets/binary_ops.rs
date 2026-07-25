#![no_main]

use std::sync::Arc;

use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_engine::{execute_binary, Limits, Plan, Step};
use serde_json::json;

fn table(payload: &[u8], side: &str) -> RecordBatch {
    let rows = payload.iter().take(64).enumerate().collect::<Vec<_>>();
    let ids = rows
        .iter()
        .map(|(index, byte)| (index % 9 != 0).then_some(i64::from(**byte)))
        .collect::<Vec<_>>();
    let values = rows
        .iter()
        .map(|(_, byte)| Some(format!("{side}-{byte}")))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("value", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(StringArray::from(values)),
        ],
    )
    .expect("bounded fixture")
}

fuzz_target!(|payload: &[u8]| {
    let selector = payload.first().copied().unwrap_or_default() % 4;
    let (operation, config) = match selector {
        0 => (
            "join",
            json!({"left_keys":["id"],"right_keys":["id"],"how":"outer"}),
        ),
        1 => ("concat", json!({"ignore_index":true})),
        2 => ("cross_join", json!({})),
        _ => (
            "table_diff",
            json!({"left_keys":["id"],"right_keys":["id"],"compare_columns":["value"],"include_unchanged":"yes","separator":"#"}),
        ),
    };
    let plan = Plan {
        schema_version: 1,
        limits: Limits {
            max_rows: 4_096,
            max_columns: 32,
            ..Limits::default()
        },
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .expect("static plan");
    if let Ok(output) = execute_binary(&table(payload, "L"), &table(payload, "R"), &plan) {
        assert!(output.num_rows() <= plan.limits().max_rows);
        assert!(output.num_columns() <= plan.limits().max_columns);
        assert!(output
            .columns()
            .iter()
            .all(|column| column.len() == output.num_rows()));
    }
});
