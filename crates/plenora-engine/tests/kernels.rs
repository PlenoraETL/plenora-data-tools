use std::sync::Arc;

use plenora_core::arrow::array::{Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::{execute_batch, Limits, Plan, Step};
use proptest::prelude::*;
use serde_json::{json, Value};

fn batch() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("code", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(StringArray::from(vec![
                Some("  Citta'  "),
                None,
                Some("ÉCOLE   Roma"),
            ])),
            Arc::new(StringArray::from(vec![Some("7"), Some("12"), None])),
        ],
    )
    .expect("valid fixture")
}

fn plan(steps: Vec<(&str, Value)>) -> plenora_engine::ValidatedPlan {
    Plan {
        schema_version: 1,
        limits: Limits::default(),
        steps: steps
            .into_iter()
            .map(|(operation, config)| Step {
                operation: operation.into(),
                config,
            })
            .collect(),
    }
    .validate()
    .expect("valid plan")
}

fn strings<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    batch
        .column_by_name(name)
        .expect("column")
        .as_any()
        .downcast_ref()
        .expect("string column")
}

#[test]
fn executes_a_schema_changing_chain_without_serialization_between_steps() {
    let chain = plan(vec![
        (
            "text_normalize",
            json!({"columns": ["name"], "operations": "full", "overwrite": false}),
        ),
        (
            "string_length",
            json!({"column": "name_norm", "output_column": "chars"}),
        ),
        (
            "rename",
            json!({"renames": [{"old_name": "code", "new_name": "id"}]}),
        ),
        (
            "reorder_columns",
            json!({"columns": ["id", "name_norm", "chars"]}),
        ),
    ]);
    let result = execute_batch(batch(), &chain).expect("chain succeeds");
    assert_eq!(
        result
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name_norm", "chars", "name"]
    );
    let normalized = strings(&result, "name_norm");
    assert_eq!(normalized.value(0), "citta'");
    assert!(normalized.is_null(1));
    assert_eq!(normalized.value(2), "ecole roma");
    let lengths = result
        .column_by_name("chars")
        .expect("chars")
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("int64");
    assert_eq!(lengths.value(0), 6);
    assert!(lengths.is_null(1));
}

#[test]
fn concat_has_explicit_null_semantics() {
    let skip = plan(vec![(
        "concat_columns",
        json!({"columns": ["name", "code"], "output_column": "joined", "separator": "|", "skip_null": true}),
    )]);
    let result = execute_batch(batch(), &skip).expect("concat");
    let joined = strings(&result, "joined");
    assert_eq!(joined.value(0), "  Citta'  |7");
    assert_eq!(joined.value(1), "12");
    assert_eq!(joined.value(2), "ÉCOLE   Roma");
}

#[test]
fn split_preserves_nulls_and_bounds_expansion() {
    let chain = plan(vec![(
        "split_column",
        json!({"column": "name", "delimiter": " ", "new_columns": ["first", "rest"], "max_splits": 1}),
    )]);
    let result = execute_batch(batch(), &chain).expect("split");
    assert_eq!(strings(&result, "first").value(0), "");
    assert_eq!(strings(&result, "rest").value(0), " Citta'  ");
    assert!(strings(&result, "first").is_null(1));
}

#[test]
fn drop_all_columns_keeps_row_count() {
    let chain = plan(vec![("drop_columns", json!({"columns": ["name", "code"]}))]);
    let result = execute_batch(batch(), &chain).expect("drop");
    assert_eq!(result.num_columns(), 0);
    assert_eq!(result.num_rows(), 3);
}

#[test]
fn collisions_and_malformed_config_fail_closed() {
    let collision = plan(vec![(
        "rename",
        json!({"renames": [{"old_name": "name", "new_name": "code"}]}),
    )]);
    assert!(execute_batch(batch(), &collision).is_err());

    let malformed = Plan {
        schema_version: 1,
        limits: Limits::default(),
        steps: vec![Step {
            operation: "string_length".into(),
            config: json!({"column": "name", "typo": true}),
        }],
    };
    assert!(malformed.validate().is_err());
}

#[test]
fn unicode_padding_counts_characters_not_bytes() {
    let chain = plan(vec![(
        "string_pad",
        json!({"column": "code", "width": 3, "side": "left", "fill_char": "🙂", "output_column": "padded"}),
    )]);
    let result = execute_batch(batch(), &chain).expect("pad");
    assert_eq!(strings(&result, "padded").value(0), "🙂🙂7");
    assert_eq!(strings(&result, "padded").value(1), "🙂12");
    assert!(strings(&result, "padded").is_null(2));
}

proptest! {
    #[test]
    fn string_length_matches_unicode_scalar_count(value in ".{0,256}") {
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, false)])),
            vec![Arc::new(StringArray::from(vec![value.as_str()]))],
        ).expect("batch");
        let chain = plan(vec![("string_length", json!({"column": "value", "output_column": "length"}))]);
        let output = execute_batch(input, &chain).expect("length");
        let lengths = output.column_by_name("length").expect("length")
            .as_any().downcast_ref::<Int64Array>().expect("int64");
        prop_assert_eq!(lengths.value(0), i64::try_from(value.chars().count()).expect("bounded"));
    }
}
