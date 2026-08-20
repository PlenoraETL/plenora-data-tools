#![no_main]

use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_engine::{execute_batch, execute_binary, Limits, Plan, Step};
use serde_json::json;

fn fixture(payload: &[u8]) -> RecordBatch {
    let values = payload.iter().take(128).copied().collect::<Vec<_>>();
    let numbers = values.iter().map(|value| f64::from(*value)).collect::<Vec<_>>();
    let ids = values.iter().map(|value| i64::from(*value % 32)).collect::<Vec<_>>();
    let text = values
        .iter()
        .map(|value| format!(" {} ", value))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("number", DataType::Float64, false),
            Field::new("text", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(Float64Array::from(numbers)),
            Arc::new(StringArray::from(text)),
        ],
    )
    .expect("bounded fixture")
}

fn limits() -> Limits {
    Limits {
        max_rows: 1_024,
        max_columns: 32,
        max_string_bytes: 4_096,
        max_governed_memory_bytes: 2_048,
        max_temp_bytes: 2 * 1024 * 1024,
        spill_partitions: 16,
        ..Limits::default()
    }
}

fn plan(operation: &str, config: serde_json::Value) -> plenora_engine::ValidatedPlan {
    Plan {
        schema_version: 1,
        limits: limits(),
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .expect("static advanced plan")
}

fuzz_target!(|payload: &[u8]| {
    let batch = fixture(payload);
    let selector = payload.first().copied().unwrap_or_default() % 10;
    if selector < 6 {
        let (operation, config) = match selector {
            0 => (
                "expression",
                json!({"output_column":"out","output_type":"number",
                    "expression":{"kind":"binary","op":"add",
                        "left":{"kind":"column","name":"number"},
                        "right":{"kind":"literal","value":1}}}),
            ),
            1 => (
                "expression",
                json!({"output_column":"out",
                    "expression":{"kind":"function","name":"trim",
                        "args":[{"kind":"column","name":"text"}]}}),
            ),
            2 => (
                "window_function",
                json!({"column":"number","function":"percent_rank","group_by":null,
                    "order_column":null,"output_column":"out"}),
            ),
            3 => (
                "window_function",
                json!({"column":"number","function":"ntile","buckets":7,"group_by":null,
                    "order_column":null,"output_column":"out"}),
            ),
            4 => (
                "assert_cardinality",
                json!({"min_rows":0,"max_rows":128}),
            ),
            _ => (
                "type_cast",
                json!({"column":"text","target_type":"binary_utf8","errors":"raise"}),
            ),
        };
        if let Ok(output) = execute_batch(batch, &plan(operation, config)) {
            assert!(output.num_rows() <= 1_024);
            assert!(output.columns().iter().all(|column| column.len() == output.num_rows()));
        }
    } else {
        let (operation, config) = match selector {
            6 => (
                "assert_foreign_key",
                json!({"left_keys":["id"],"right_keys":["id"],"allow_null":false}),
            ),
            7 => (
                "reconcile",
                json!({"left_keys":["id"],"right_keys":["id"],"nulls_equal":true}),
            ),
            8 => ("intersect", json!({})),
            _ => ("except", json!({})),
        };
        if let Ok(output) = execute_binary(&batch, &batch, &plan(operation, config)) {
            assert!(output.num_rows() <= 1_024);
            assert!(output.columns().iter().all(|column| column.len() == output.num_rows()));
        }
    }
});
