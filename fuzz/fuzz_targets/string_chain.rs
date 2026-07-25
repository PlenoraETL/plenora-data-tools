#![no_main]

use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_engine::{execute_batch, Limits, Plan, Step};
use serde_json::json;

fuzz_target!(|payload: &[u8]| {
    let rows: Vec<Option<String>> = payload
        .chunks(32)
        .take(256)
        .enumerate()
        .map(|(index, chunk)| (index % 7 != 0).then(|| String::from_utf8_lossy(chunk).into_owned()))
        .collect();
    let row_count = rows.len();
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(rows))],
    )
    .expect("bounded fuzz fixture");
    let plan = Plan {
        schema_version: 1,
        limits: Limits::default(),
        steps: vec![
            Step {
                operation: "text_normalize".into(),
                config: json!({"columns": ["value"], "operations": "full", "overwrite": true}),
            },
            Step {
                operation: "string_pad".into(),
                config: json!({"column": "value", "width": 40, "side": "right", "fill_char": "🙂", "output_column": "padded"}),
            },
            Step {
                operation: "string_length".into(),
                config: json!({"column": "padded", "output_column": "length"}),
            },
        ],
    }
    .validate()
    .expect("static fuzz plan");
    let output = execute_batch(batch, &plan).expect("bounded string chain");
    assert_eq!(output.num_rows(), row_count);
    assert_eq!(output.num_columns(), 3);
    for column in output.columns() {
        assert_eq!(column.len(), row_count);
    }
});
