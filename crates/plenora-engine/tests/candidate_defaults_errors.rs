use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, BooleanArray, Float64Array, Int64Array, LargeStringArray, RecordBatch, StringArray,
    UInt32Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::table_engine::SCHEMA_VERSION;
use plenora_engine::{
    execute_batch as execute_batch_local, execute_binary, execute_complete_batch as execute_batch,
    Limits, Plan, Step, ValidatedPlan,
};
use serde_json::{json, Value};

fn batch() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("num", DataType::Float64, true),
            Field::new("flag", DataType::Boolean, true),
            Field::new("group", DataType::Utf8, true),
            Field::new("text", DataType::Utf8, true),
            Field::new("date", DataType::Utf8, true),
            Field::new("json", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![Some(1), Some(2), None])),
            Arc::new(Float64Array::from(vec![Some(1.5), None, Some(1.5)])),
            Arc::new(BooleanArray::from(vec![Some(true), None, Some(false)])),
            Arc::new(StringArray::from(vec![Some("a"), Some("a"), None])),
            Arc::new(StringArray::from(vec![Some("a1 b2"), None, Some("false")])),
            Arc::new(StringArray::from(vec![
                Some("31-12-2026"),
                Some("2026/01/02"),
                None,
            ])),
            Arc::new(StringArray::from(vec![
                Some(r#"{"a":{"b":1}}"#),
                Some("{}"),
                None,
            ])),
        ],
    )
    .expect("fixture")
}

fn plan(operation: &str, config: Value) -> ValidatedPlan {
    Plan {
        schema_version: SCHEMA_VERSION,
        limits: Limits::default(),
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .unwrap_or_else(|error| panic!("{operation}: {error}"))
}

fn run(operation: &str, config: Value) -> RecordBatch {
    execute_batch(batch(), &plan(operation, config))
        .unwrap_or_else(|error| panic!("{operation}: {error}"))
}

fn utf8<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    batch
        .column_by_name(name)
        .expect("column")
        .as_any()
        .downcast_ref()
        .expect("Utf8")
}

#[test]
fn serde_defaults_are_executable_not_merely_deserializable() {
    assert_eq!(run("fill_na", json!({"column":"text"})).num_rows(), 3);
    assert_eq!(
        run("type_cast", json!({"column":"id"}))
            .column_by_name("id")
            .expect("id")
            .data_type(),
        &DataType::Utf8
    );
    assert_eq!(
        run("conditional", json!({"column":"group","conditions":[{}]})).num_columns(),
        8
    );
    assert_eq!(
        run("lookup", json!({"column":"group","mapping":{}})).num_columns(),
        7
    );
    assert!(run("bin", json!({"column":"num"}))
        .column_by_name("num_bin")
        .is_some());
    assert!(run("flatten_json", json!({"column":"json"}))
        .column_by_name("json_a.b")
        .is_some());
    assert_eq!(
        run("statistics", json!({"column":"num","group_by":null})).num_columns(),
        13
    );
    assert!(
        run(
            "sample",
            json!({"fraction":null,"random_state":null,"stratify_column":null})
        )
        .num_rows()
            <= 3
    );
    assert_eq!(run("sort", json!({"columns":["id"]})).num_rows(), 3);
    assert_eq!(run("distinct", json!({"subset":[]})).num_rows(), 3);
    assert_eq!(
        run(
            "aggregate",
            json!({"group_by":["group"],"aggregations":[{"column":"num"}]})
        )
        .num_columns(),
        2
    );
    assert!(run(
        "window_function",
        json!({"column":"num","group_by":null,"order_column":null,"output_column":null})
    )
    .column_by_name("num_rank")
    .is_some());
    assert_eq!(
        run(
            "melt",
            json!({"id_columns":["id"],"value_columns":["text"]})
        )
        .schema()
        .field(1)
        .name(),
        "variable"
    );
    assert_eq!(
        run(
            "pivot",
            json!({"index_col":"group","pivot_col":"id","value_col":"text"})
        )
        .num_rows(),
        2
    );
    assert_eq!(
        run(
            "transpose",
            json!({"id_column":null,"type_policy":"string"})
        )
        .num_columns(),
        4
    );
}

#[test]
fn extraction_casting_and_formula_error_surfaces_are_explicit() {
    let named = run(
        "string_extract",
        json!({"column":"text","pattern":"(?P<letter>[a-z])(?P<digit>[0-9])","output_column":null,"extract_all":false}),
    );
    assert_eq!(utf8(&named, "letter").value(0), "a");
    let all = run(
        "string_extract",
        json!({"column":"text","pattern":"([0-9])","output_column":null,"extract_all":true}),
    );
    assert_eq!(utf8(&all, "text_extracted").value(0), "1,2");
    assert!(execute_batch(
        batch(),
        &plan(
            "type_cast",
            json!({"column":"date","target_type":"date","date_format":"%Y","errors":"raise"})
        )
    )
    .is_err());
    for formula in [
        "",
        ".",
        "1e",
        "'open",
        "'\\x'",
        "num $ 1",
        "num +",
        "-text",
        "text * text",
    ] {
        let raw = Plan {
            schema_version: 1,
            limits: Limits::default(),
            steps: vec![Step {
                operation: "formula".into(),
                config: json!({"new_column":"x","formula":formula}),
            }],
        };
        if let Ok(valid) = raw.validate() {
            assert!(execute_batch(batch(), &valid).is_err(), "{formula}");
        }
    }
}

#[test]
fn masking_defaults_and_real_formats_have_exact_non_destructive_output() {
    let values = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "secret",
            DataType::Utf8,
            true,
        )])),
        vec![Arc::new(StringArray::from(vec![
            Some("RSSMRA80A01H501U"),
            Some("mario.rossi@example.it"),
            Some("+39 333 1234567"),
            Some("IT60X0542811101000000123456"),
            Some("abc"),
            Some("c@example.it"),
            None,
        ]))],
    )
    .expect("secrets");
    for kind in ["cf", "email", "phone", "iban"] {
        let output = execute_batch(
            values.clone(),
            &plan(
                "mask_data",
                json!({"maskings":[{"column":"secret","mask_type":kind}],"overwrite":true}),
            ),
        )
        .expect("mask");
        assert_ne!(utf8(&output, "secret").value(0), "", "{kind}");
        assert!(utf8(&output, "secret").is_null(6));
        if kind == "email" {
            assert_eq!(utf8(&output, "secret").value(5), "*@example.it");
        }
    }
    let defaults = execute_batch(
        values.clone(),
        &plan("mask_data", json!({"maskings":[{"column":"secret"}]})),
    )
    .expect("defaults");
    assert!(defaults.column_by_name("secret_masked").is_some());
    // Semantica storica: con null_policy di default
    // (Empty) il null e' sostituito da stringa vuota — digest dell'hash
    // storico della sola colonna, nessun rifiuto. Il rifiuto row-scoped
    // resta SOLO per null_policy=error (test dedicati in kernels-table).
    let hash = execute_batch(values, &plan("md5_hash", json!({"columns":["secret"]})))
        .expect("hash default: sostituzione Empty storica");
    assert_eq!(
        utf8(&hash, "md5_hash").value(6),
        "d41d8cd98f00b204e9800998ecf8427e",
        "null -> md5 di stringa vuota"
    );
}

#[test]
fn scalar_profiles_reject_unsupported_arrow_types_without_panics() {
    let unsupported = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("u", DataType::UInt32, true)])),
        vec![Arc::new(UInt32Array::from(vec![Some(1), None]))],
    )
    .expect("UInt32");
    for (operation, config) in [
        ("fill_na", json!({"column":"u","method":"value","value":1})),
        ("formula", json!({"new_column":"x","formula":"u + 1"})),
        ("filter", json!({"column":"u","operator":">","value":0})),
        (
            "md5_hash",
            json!({"columns":["u"],"output_column":"h","normalize":false}),
        ),
    ] {
        assert!(
            execute_batch(unsupported.clone(), &plan(operation, config)).is_err(),
            "{operation}"
        );
    }
}

#[test]
fn numeric_text_and_null_error_boundaries_are_stable() {
    for (operation, config) in [
        (
            "filter",
            json!({"column":"num","operator":"between","value":"x,2"}),
        ),
        (
            "filter",
            json!({"column":"num","operator":"between","value":"1"}),
        ),
        ("filter", json!({"column":"num","operator":">","value":"x"})),
        (
            "bin",
            json!({"column":"num","bins":1,"labels":null,"output_column":null}),
        ),
        (
            "bin",
            json!({"column":"num","bins":[0,1,2],"labels":["one"],"output_column":null}),
        ),
    ] {
        assert!(
            execute_batch(batch(), &plan(operation, config)).is_err(),
            "{operation}"
        );
    }
    assert!(Plan {
        schema_version: SCHEMA_VERSION,
        limits: Limits::default(),
        steps: vec![Step {
            operation: "window_function".into(),
            config: json!({"column":"num","function":"lag","group_by":null,"order_column":null,"offset":0,"output_column":null}),
        }],
    }
    .validate()
    .is_err());
    let all_null = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "num",
            DataType::Float64,
            true,
        )])),
        vec![Arc::new(Float64Array::from(vec![None, None]))],
    )
    .expect("nulls");
    assert!(execute_batch(
        all_null,
        &plan(
            "bin",
            json!({"column":"num","bins":3,"labels":null,"output_column":null})
        )
    )
    .is_err());
}

#[test]
fn joins_coalesce_every_supported_key_type() {
    let schemas_and_columns: Vec<(DataType, Arc<dyn Array>)> = vec![
        (DataType::Utf8, Arc::new(StringArray::from(vec!["a", "b"]))),
        (
            DataType::Float64,
            Arc::new(Float64Array::from(vec![1.0, 2.0])),
        ),
        (
            DataType::Boolean,
            Arc::new(BooleanArray::from(vec![true, false])),
        ),
    ];
    for (data_type, column) in schemas_and_columns {
        let table = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("key", data_type, false)])),
            vec![column],
        )
        .expect("table");
        let output = execute_binary(
            &table,
            &table,
            &plan(
                "join",
                json!({"left_keys":["key"],"right_keys":["key"],"how":"outer"}),
            ),
        )
        .expect("join");
        assert_eq!(output.num_rows(), 2);
    }
}

#[test]
fn utility_defaults_and_alternate_date_formats_are_covered() {
    let numbered = run(
        "add_row_number",
        json!({"partition_column":null,"order_column":null}),
    );
    assert!(numbered.column_by_name("row_number").is_some());
    let dates = run("date_extract", json!({"column":"date"}));
    assert_eq!(
        dates
            .column_by_name("date_year")
            .expect("year")
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64")
            .value(0),
        2026
    );
    let uuid = run("uuid_generator", json!({}));
    assert!(uuid.column_by_name("uuid").is_some());
}

#[test]
fn large_utf8_from_modern_pyarrow_is_normalized_without_data_loss() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::LargeUtf8,
            true,
        )])),
        vec![Arc::new(LargeStringArray::from(vec![
            Some("città"),
            None,
            Some("東京"),
        ]))],
    )
    .expect("LargeUtf8");
    let output = execute_batch(
        input.clone(),
        &plan(
            "string_length",
            json!({"column":"value","output_column":"length"}),
        ),
    )
    .expect("normalize unary");
    assert_eq!(output.schema().field(0).data_type(), &DataType::Utf8);
    assert_eq!(utf8(&output, "value").value(0), "città");
    assert_eq!(
        output
            .column_by_name("length")
            .expect("length")
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64")
            .value(2),
        2
    );
    let binary = execute_binary(
        &input,
        &input,
        &plan("concat", json!({"ignore_index":true})),
    )
    .expect("normalize binary");
    assert_eq!(binary.num_rows(), 6);
    assert_eq!(binary.schema().field(0).data_type(), &DataType::Utf8);
}

#[test]
fn stale_pandas_metadata_cannot_override_a_physical_type_cast() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "pandas".to_owned(),
        r#"{"columns":[{"name":"value","field_name":"value","pandas_type":"unicode","numpy_type":"string"}]}"#.to_owned(),
    );
    metadata.insert("plenora.contract".to_owned(), "keep-me".to_owned());
    let input = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            vec![Field::new("value", DataType::Utf8, true)],
            metadata,
        )),
        vec![Arc::new(StringArray::from(vec![Some("1"), None]))],
    )
    .expect("metadata input");
    let output = execute_batch(
        input,
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"int","errors":"raise"}),
        ),
    )
    .expect("cast");
    assert_eq!(output.schema().field(0).data_type(), &DataType::Int64);
    assert!(!output.schema().metadata().contains_key("pandas"));
    assert_eq!(
        output.schema().metadata().get("plenora.contract"),
        Some(&"keep-me".to_owned())
    );
}

#[test]
fn batch_local_api_fails_closed_and_complete_api_preserves_row_diagnostics() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "effective_date",
            DataType::Utf8,
            true,
        )])),
        vec![Arc::new(StringArray::from(vec![
            Some("2026-08-02"),
            Some("not-a-date"),
        ]))],
    )
    .expect("date input");
    let validated = plan(
        "type_cast",
        json!({
            "column":"effective_date",
            "target_type":"date32",
            "errors":"coerce"
        }),
    );
    let local = execute_batch_local(input.clone(), &validated)
        .expect_err("API batch-local ha pubblicato completezza inventata");
    assert_eq!(local.category(), plenora_core::ErrorCategory::Unsupported);
    assert!(local.row_diagnostics().is_none());

    let error = plenora_engine::execute_complete_batch(input, &validated)
        .expect_err("date invalida accettata");

    assert_eq!(error.category(), plenora_core::ErrorCategory::DataMapping);
    assert_eq!(error.phase(), plenora_core::ErrorPhase::Read);
    let diagnostics = error.row_diagnostics().expect("diagnostica persa");
    assert_eq!(diagnostics.observed_total, 1);
    assert_eq!(diagnostics.examples[0].source_index, 1);
}

#[test]
fn cleansing_fill_variants_and_exact_replacement_cover_every_scalar_type() {
    let cases = [
        ("text", json!(42)),
        ("id", Value::Null),
        ("id", json!(9)),
        ("num", json!(2.5)),
        ("num", json!("2,5")),
        ("flag", json!(true)),
        ("flag", json!("true")),
        ("flag", json!("false")),
    ];
    for (column, value) in cases {
        assert!(execute_batch(
            batch(),
            &plan(
                "fill_na",
                json!({"column":column,"method":"value","value":value})
            )
        )
        .is_ok());
    }
    for (column, value) in [("num", json!(true)), ("flag", json!(3))] {
        assert!(execute_batch(
            batch(),
            &plan(
                "fill_na",
                json!({"column":column,"method":"value","value":value})
            )
        )
        .is_err());
    }
    assert!(execute_batch(
        batch(),
        &plan(
            "fill_na",
            json!({"column":null,"method":"ffill","value":null})
        )
    )
    .is_ok());
    let replaced = run(
        "replace",
        json!({"column":"text","old_value":"false","new_value":"no","regex":false}),
    );
    assert_eq!(utf8(&replaced, "text").value(2), "no");
    let custom_input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("date", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec!["31-12-2026"]))],
    )
    .expect("custom date");
    let custom = execute_batch(
        custom_input,
        &plan(
            "type_cast",
            json!({"column":"date","target_type":"date","date_format":"%d-%m-%Y","errors":"coerce"}),
        ),
    )
    .expect("custom date cast");
    assert_eq!(utf8(&custom, "date").value(0), "2026-12-31");
    let invalid_custom = execute_batch(
        batch(),
        &plan(
            "type_cast",
            json!({"column":"date","target_type":"date","date_format":"%d-%m-%Y","errors":"coerce"}),
        ),
    )
    .expect_err("data fuori formato accettata");
    let diagnostics = invalid_custom
        .row_diagnostics()
        .expect("diagnostica custom date");
    assert_eq!(diagnostics.observed_total, 1);
    assert_eq!(diagnostics.examples[0].source_index, 1);
    let custom_datetime = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("v", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec!["31-12-2026 05:06:07"]))],
    )
    .expect("custom datetime");
    let custom_datetime = execute_batch(
        custom_datetime,
        &plan("type_cast", json!({"column":"v","target_type":"datetime","date_format":"%d-%m-%Y %H:%M:%S","errors":"raise"})),
    )
    .expect("custom datetime cast");
    assert_eq!(utf8(&custom_datetime, "v").value(0), "2026-12-31T05:06:07");
    let truthy = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("v", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec![
            "yes", "sì", "vero", "1", "y", "s",
        ]))],
    )
    .expect("truthy");
    assert!(execute_batch(
        truthy,
        &plan(
            "type_cast",
            json!({"column":"v","target_type":"bool","errors":"raise"})
        )
    )
    .is_ok());
}

#[test]
fn blocking_cardinality_guards_are_reached_after_valid_inputs() {
    let repeated = |ids: Vec<i64>, name: &str| {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new(name, DataType::Utf8, false),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(vec![name, name])),
            ],
        )
        .expect("table")
    };
    let left = repeated(vec![1, 1], "left");
    let right = repeated(vec![1, 1], "right");
    let limits = Limits {
        max_rows: 2,
        ..Limits::default()
    };
    let limited = |operation: &str, config: Value| {
        Plan {
            schema_version: 1,
            limits: limits.clone(),
            steps: vec![Step {
                operation: operation.into(),
                config,
            }],
        }
        .validate()
        .expect("plan")
    };
    assert!(execute_binary(
        &left,
        &right,
        &limited(
            "join",
            json!({"left_keys":["id"],"right_keys":["id"],"how":"inner"})
        )
    )
    .is_err());
    assert!(execute_binary(
        &left,
        &right,
        &limited("concat", json!({"ignore_index":true}))
    )
    .is_err());
    let unmatched_left = repeated(vec![1, 2], "left");
    let unmatched_right = repeated(vec![3, 4], "right");
    assert!(execute_binary(
        &unmatched_left,
        &unmatched_right,
        &limited(
            "join",
            json!({"left_keys":["id"],"right_keys":["id"],"how":"outer"})
        )
    )
    .is_err());
    assert!(execute_binary(
        &left,
        &right,
        &plan(
            "join",
            json!({"left_keys":["id"],"right_keys":["id","right"],"how":"inner"})
        )
    )
    .is_err());
    let default_join = execute_binary(
        &left,
        &right,
        &plan("join", json!({"left_keys":["id"],"right_keys":["id"]})),
    )
    .expect("default inner");
    assert_eq!(default_join.num_rows(), 4);
}

#[test]
fn blocking_schema_collision_and_output_limits_fail_closed() {
    let collision_left = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("x", DataType::Utf8, false),
            Field::new("x_R", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1])),
            Arc::new(StringArray::from(vec!["a"])),
            Arc::new(StringArray::from(vec!["b"])),
        ],
    )
    .expect("left");
    let collision_right = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("x", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1])),
            Arc::new(StringArray::from(vec!["c"])),
        ],
    )
    .expect("right");
    let compatible = execute_binary(
        &collision_left,
        &collision_right,
        &plan(
            "join",
            json!({"left_keys":["id"],"right_keys":["id"],"how":"inner"}),
        ),
    )
    .expect("suffixes avoid source collisions");
    assert!(compatible.column_by_name("x_L").is_some());
    assert!(compatible.column_by_name("x_R_L").is_some());
    assert!(compatible.column_by_name("x_R").is_some());
    let true_collision_left = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("x_L", DataType::Int64, false),
            Field::new("x", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1])),
            Arc::new(StringArray::from(vec!["a"])),
        ],
    )
    .expect("collision left");
    let differently_named_key = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1]))],
    )
    .expect("collision right");
    assert!(execute_binary(
        &true_collision_left,
        &differently_named_key,
        &plan(
            "join",
            json!({"left_keys":["x_L"],"right_keys":["id"],"how":"inner"})
        )
    )
    .is_err());
    let two_columns = Limits {
        max_columns: 2,
        ..Limits::default()
    };
    let limited = Plan {
        schema_version: 1,
        limits: two_columns,
        steps: vec![Step {
            operation: "join".into(),
            config: json!({"left_keys":["id"],"right_keys":["id"],"how":"inner"}),
        }],
    }
    .validate()
    .expect("plan");
    let small_left = collision_left.project(&[0, 1]).expect("project");
    assert!(execute_binary(&small_left, &collision_right, &limited).is_err());
}

#[test]
fn analysis_and_reshape_post_input_limits_are_exercised() {
    let three = batch();
    let columns_limit = Limits {
        max_columns: three.num_columns(),
        ..Limits::default()
    };
    let flatten = Plan {
        schema_version: 1,
        limits: columns_limit,
        steps: vec![Step {
            operation: "flatten_json".into(),
            config: json!({"column":"json","prefix":"j_","max_level":2,"output_columns":[]}),
        }],
    }
    .validate()
    .expect("flatten plan");
    assert!(execute_batch(three.clone(), &flatten).is_err());
    assert!(execute_batch(
        three.clone(),
        &plan(
            "flatten_json",
            json!({"column":"json","prefix":"j_","max_level":2,"output_columns":["wrong"]})
        )
    )
    .is_err());
    let stratified = run(
        "sample",
        json!({"n":99,"fraction":0.5,"random_state":3,"stratify_column":"group"}),
    );
    assert!(stratified.num_rows() <= 2);

    let no_values = execute_batch(
        three.clone(),
        &plan(
            "melt",
            json!({"id_columns":["id","num","flag","group","text","date","json"],"value_columns":[],"var_name":"v","value_name":"x"}),
        ),
    );
    assert!(no_values.is_err());
    let melt_limits = Limits {
        max_rows: 3,
        ..Limits::default()
    };
    let melt = Plan { schema_version: 1, limits: melt_limits, steps: vec![Step { operation: "melt".into(), config: json!({"id_columns":["id"],"value_columns":["text","num"],"var_name":"v","value_name":"x"}) }] }.validate().expect("melt plan");
    assert!(execute_batch(three.clone(), &melt).is_err());
    let empty = RecordBatch::new_empty(three.schema());
    assert_eq!(
        execute_batch(
            empty,
            &plan("transpose", json!({"id_column":null,"output_columns":[]}))
        )
        .expect("empty transpose")
        .num_rows(),
        0
    );
    let transpose_limits = Limits {
        max_rows: 6,
        max_columns: 3,
        ..Limits::default()
    };
    let transpose = Plan {
        schema_version: 1,
        limits: transpose_limits,
        steps: vec![Step {
            operation: "transpose".into(),
            config: json!({"id_column":null,"output_columns":[]}),
        }],
    }
    .validate()
    .expect("transpose plan");
    assert!(execute_batch(three.clone(), &transpose).is_err());

    assert!(execute_binary(
        &three,
        &three,
        &plan(
            "table_diff",
            json!({"left_keys":["id"],"right_keys":["id","group"],"compare_columns":["text"]})
        )
    )
    .is_err());
    let duplicate_right = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1, 1]))],
    )
    .expect("duplicates");
    assert!(execute_binary(
        &duplicate_right,
        &duplicate_right,
        &plan(
            "table_diff",
            json!({"left_keys":["id"],"right_keys":["id"],"compare_columns":[]})
        )
    )
    .is_err());
}
