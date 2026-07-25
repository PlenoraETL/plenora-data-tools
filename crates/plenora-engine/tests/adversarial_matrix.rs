use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::{execute_batch, Limits, Plan, Step, ValidatedPlan};
use serde_json::{json, Value};

fn validated(operation: &str, config: Value, limits: Limits) -> Result<ValidatedPlan, String> {
    Plan {
        schema_version: 1,
        limits,
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .map_err(|error| error.to_string())
}

fn strings(columns: &[(&str, Vec<Option<&str>>)]) -> RecordBatch {
    let fields = columns
        .iter()
        .map(|(name, _)| Field::new(*name, DataType::Utf8, true))
        .collect::<Vec<_>>();
    let arrays = columns
        .iter()
        .map(|(_, values)| Arc::new(StringArray::from(values.clone())) as ArrayRef)
        .collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).expect("fixture")
}

#[test]
fn semantic_contract_matrix_is_fail_closed() {
    let tiny = Limits {
        max_columns: 1,
        ..Limits::default()
    };
    assert!(validated("drop_columns", json!({"columns": ["a", "b"]}), tiny).is_err());
    assert!(validated(
        "rename",
        json!({"renames": [
            {"old_name": "a", "new_name": "x"}, {"old_name": "a", "new_name": "y"}
        ]}),
        Limits::default()
    )
    .is_err());
    assert!(validated(
        "reorder_columns",
        json!({"columns": ["a", "a"]}),
        Limits::default()
    )
    .is_err());
    assert!(validated(
        "concat_columns",
        json!({"columns": [], "output_column": "x"}),
        Limits::default()
    )
    .is_err());
    assert!(validated(
        "concat_columns",
        json!({"columns": ["a"], "output_column": " "}),
        Limits::default()
    )
    .is_err());
    assert!(validated(
        "split_column",
        json!({"column": "a", "delimiter": ",", "new_columns": [], "max_splits": -1}),
        Limits::default()
    )
    .is_err());
    assert!(validated(
        "split_column",
        json!({"column": "a", "delimiter": ",", "new_columns": ["x", "x"], "max_splits": -1}),
        Limits::default()
    )
    .is_err());
    assert!(validated(
        "string_length",
        json!({"column": "a", "output_column": ""}),
        Limits::default()
    )
    .is_err());
    assert!(validated(
        "text_normalize",
        json!({"columns": [], "operations": "full", "overwrite": true}),
        Limits::default()
    )
    .is_err());
    assert!(validated(
        "text_normalize",
        json!({"columns": ["a", "a"], "operations": "full", "overwrite": true}),
        Limits::default()
    )
    .is_err());

    let tiny = Limits {
        max_string_bytes: 1,
        ..Limits::default()
    };
    assert!(validated(
        "concat_columns",
        json!({"columns": ["a"], "output_column": "x", "separator": "--"}),
        tiny.clone()
    )
    .is_err());
    assert!(validated(
        "split_column",
        json!({"column": "a", "delimiter": "--", "new_columns": ["x"], "max_splits": -1}),
        tiny.clone()
    )
    .is_err());
    assert!(validated(
        "string_pad",
        json!({"column": "a", "width": 2, "side": "left", "fill_char": "0", "output_column": null}),
        tiny
    )
    .is_err());
}

#[test]
fn batch_and_post_step_resource_limits_are_enforced() {
    let batch = strings(&[
        ("a", vec![Some("x"), Some("y")]),
        ("b", vec![Some("1"), Some("2")]),
    ]);
    let limits = Limits {
        max_rows: 1,
        ..Limits::default()
    };
    let plan = validated("drop_columns", json!({"columns": []}), limits).expect("plan");
    assert!(execute_batch(batch.clone(), &plan).is_err());

    let limits = Limits {
        max_columns: 1,
        ..Limits::default()
    };
    let plan = validated("drop_columns", json!({"columns": []}), limits).expect("plan");
    assert!(execute_batch(batch.clone(), &plan).is_err());

    let duplicate = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("same", DataType::Utf8, true),
            Field::new("same", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(StringArray::from(vec![Some("x")])),
            Arc::new(StringArray::from(vec![Some("y")])),
        ],
    )
    .expect("Arrow permits duplicate field names");
    let plan = validated("drop_columns", json!({"columns": []}), Limits::default()).expect("plan");
    assert!(execute_batch(duplicate, &plan).is_err());

    let limits = Limits {
        max_columns: 2,
        ..Limits::default()
    };
    let plan = validated(
        "string_length",
        json!({"column": "a", "output_column": "length"}),
        limits,
    )
    .expect("plan");
    assert!(execute_batch(batch, &plan).is_err());
}

#[test]
fn type_errors_missing_columns_and_output_limits_are_explicit() {
    let integer_batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("a", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1, 2]))],
    )
    .expect("fixture");
    let plan = validated(
        "string_length",
        json!({"column": "a", "output_column": null}),
        Limits::default(),
    )
    .expect("plan");
    assert!(execute_batch(integer_batch, &plan).is_err());

    let batch = strings(&[("a", vec![Some("abcd")])]);
    let plan = validated(
        "reorder_columns",
        json!({"columns": ["missing"]}),
        Limits::default(),
    )
    .expect("plan");
    assert!(execute_batch(batch.clone(), &plan).is_err());

    let limits = Limits {
        max_string_bytes: 3,
        ..Limits::default()
    };
    let plan = validated(
        "concat_columns",
        json!({"columns": ["a"], "output_column": "joined", "separator": "", "skip_null": true}),
        limits.clone(),
    )
    .expect("plan");
    assert!(execute_batch(batch.clone(), &plan).is_err());
    let plan = validated(
        "text_normalize",
        json!({"columns": ["a"], "operations": "full", "overwrite": true}),
        limits,
    )
    .expect("plan");
    assert!(execute_batch(batch, &plan).is_err());
}
