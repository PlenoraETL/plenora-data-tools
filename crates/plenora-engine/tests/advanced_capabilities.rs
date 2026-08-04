use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::array::{
    builder::StringDictionaryBuilder,
    types::{Int32Type, Int64Type},
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Float64Array,
    Int64Array, ListArray, RecordBatch, StringArray, TimestampMillisecondArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::{
    execute_binary, execute_complete_batch as execute_batch, Limits, Plan, Step, ValidatedPlan,
};
use serde_json::{json, Value};

fn plan(operation: &str, config: Value, limits: Limits) -> ValidatedPlan {
    Plan {
        schema_version: 1,
        limits,
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .unwrap_or_else(|error| panic!("{operation}: {error}"))
}

fn strings(values: Vec<Option<&str>>) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(values))],
    )
    .expect("strings")
}

#[test]
#[allow(clippy::too_many_lines)] // One matrix verifies all newly supported physical Arrow types.
fn native_arrow_casts_are_exact_nullable_and_fail_closed() {
    let input = strings(vec![Some("1970-01-01"), Some("2024-02-29"), None]);
    let date = execute_batch(
        input,
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"date32","errors":"coerce"}),
            Limits::default(),
        ),
    )
    .expect("date32");
    let values = date
        .column(0)
        .as_any()
        .downcast_ref::<Date32Array>()
        .expect("date32 array");
    assert_eq!(values.value(0), 0);
    assert!(values.is_null(2));

    let invalid_date = execute_batch(
        strings(vec![Some("invalid")]),
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"date32","errors":"coerce"}),
            Limits::default(),
        ),
    )
    .expect_err("date invalida coercita a null");
    let diagnostics = invalid_date
        .row_diagnostics()
        .expect("diagnostica row-scoped persa");
    assert_eq!(diagnostics.observed_total, 1);
    assert_eq!(diagnostics.examples[0].source_index, 0);

    let timestamp = execute_batch(
        strings(vec![Some("1970-01-01T00:00:01Z")]),
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"timestamp_millis","timezone":"Europe/Rome","errors":"coerce"}),
            Limits::default(),
        ),
    )
    .expect("timestamp");
    let values = timestamp
        .column(0)
        .as_any()
        .downcast_ref::<TimestampMillisecondArray>()
        .expect("timestamp array");
    assert_eq!(values.value(0), 1_000);
    let invalid_timestamp = execute_batch(
        strings(vec![Some("2026-03-29 02:30:00")]),
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"timestamp_millis","timezone":"Europe/Rome","errors":"coerce"}),
            Limits::default(),
        ),
    )
    .expect_err("DST gap must never be guessed or accepted");
    assert_eq!(
        invalid_timestamp
            .row_diagnostics()
            .expect("timestamp diagnostics")
            .counts,
        std::collections::BTreeMap::from([("conversion.invalid_timestamp".to_owned(), 1)])
    );

    let decimal = execute_batch(
        strings(vec![Some("12.3"), Some("-0.01"), None]),
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"decimal128","precision":6,"scale":2,"errors":"coerce"}),
            Limits::default(),
        ),
    )
    .expect("decimal");
    let values = decimal
        .column(0)
        .as_any()
        .downcast_ref::<Decimal128Array>()
        .expect("decimal array");
    assert_eq!(values.value(0), 1_230);
    assert_eq!(values.value(1), -1);
    assert!(values.is_null(2));
    assert_eq!(
        decimal.schema().field(0).data_type(),
        &DataType::Decimal128(6, 2)
    );
    let invalid_decimal = execute_batch(
        strings(vec![Some("1.234")]),
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"decimal128","precision":6,"scale":2,"errors":"coerce"}),
            Limits::default(),
        ),
    )
    .expect_err("decimal non rappresentabile accettato");
    assert!(invalid_decimal.row_diagnostics().is_some());

    let unsigned = execute_batch(
        strings(vec![Some("0"), Some("18446744073709551615")]),
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"uint64","errors":"coerce"}),
            Limits::default(),
        ),
    )
    .expect("uint64");
    let values = unsigned
        .column(0)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("uint64 array");
    assert_eq!(values.value(1), u64::MAX);
    let invalid_unsigned = execute_batch(
        strings(vec![Some("-1")]),
        &plan(
            "type_cast",
            json!({"column":"value","target_type":"uint64","errors":"coerce"}),
            Limits::default(),
        ),
    )
    .expect_err("intero unsigned invalido accettato");
    assert!(invalid_unsigned.row_diagnostics().is_some());

    for (target, expected) in [
        ("binary_utf8", DataType::Binary),
        (
            "dictionary_utf8",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
        ),
    ] {
        let output = execute_batch(
            strings(vec![Some("á"), None, Some("á")]),
            &plan(
                "type_cast",
                json!({"column":"value","target_type":target,"errors":"raise"}),
                Limits::default(),
            ),
        )
        .expect(target);
        assert_eq!(output.schema().field(0).data_type(), &expected);
        if target == "binary_utf8" {
            let bytes = output
                .column(0)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .expect("binary");
            assert_eq!(bytes.value(0), "á".as_bytes());
        }
    }

    assert!(Plan {
        schema_version: 1,
        limits: Limits::default(),
        steps: vec![Step {
            operation: "type_cast".into(),
            config: json!({"column":"value","target_type":"decimal128","precision":39,"scale":2}),
        }],
    }
    .validate()
    .is_err());
}

fn contract_is_invalid(operation: &str, config: Value, limits: Limits) -> bool {
    Plan {
        schema_version: 1,
        limits,
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .is_err()
}

#[test]
fn advanced_contract_matrix_rejects_every_unsafe_parameter_combination() {
    for config in [
        json!({"column":"value","target_type":"decimal128","scale":2}),
        json!({"column":"value","target_type":"decimal128","precision":8}),
        json!({"column":"value","target_type":"decimal128","precision":8,"scale":2,
            "timezone":"UTC"}),
        json!({"column":"value","target_type":"timestamp_millis","precision":8}),
        json!({"column":"value","target_type":"timestamp_millis","timezone":"Invalid/Zone"}),
        json!({"column":"value","target_type":"uint64","timezone":"UTC"}),
    ] {
        assert!(contract_is_invalid("type_cast", config, Limits::default()));
    }
    for config in [
        json!({}),
        json!({"exact_rows":1,"min_rows":1}),
        json!({"min_rows":2,"max_rows":1}),
        json!({"exact_rows":11}),
    ] {
        assert!(contract_is_invalid(
            "assert_cardinality",
            config,
            Limits {
                max_rows: 10,
                ..Limits::default()
            }
        ));
    }
    for (config, limits) in [
        (json!({"expected":{}}), Limits::default()),
        (
            json!({"expected":{"a":"1","b":"2"}}),
            Limits {
                max_columns: 1,
                ..Limits::default()
            },
        ),
        (json!({"expected":{"":"x"}}), Limits::default()),
        (
            json!({"expected":{"key":"toolong"}}),
            Limits {
                max_string_bytes: 2,
                ..Limits::default()
            },
        ),
    ] {
        assert!(contract_is_invalid("assert_metadata", config, limits));
    }
    for operation in ["assert_foreign_key", "reconcile"] {
        assert!(contract_is_invalid(
            operation,
            json!({"left_keys":["id"],"right_keys":["id","other"]}),
            Limits::default()
        ));
        assert!(contract_is_invalid(
            operation,
            json!({"left_keys":[],"right_keys":[]}),
            Limits::default()
        ));
    }
    for config in [
        json!({"column":"value","function":"ntile","group_by":null,
            "order_column":null,"output_column":"out"}),
        json!({"column":"value","function":"ntile","buckets":0,"group_by":null,
            "order_column":null,"output_column":"out"}),
        json!({"column":"value","function":"rank","buckets":2,"group_by":null,
            "order_column":null,"output_column":"out"}),
    ] {
        assert!(contract_is_invalid(
            "window_function",
            config,
            Limits::default()
        ));
    }
    for partitions in [0, 1, 4_097] {
        assert!(contract_is_invalid(
            "drop_columns",
            json!({"columns":[]}),
            Limits {
                spill_partitions: partitions,
                ..Limits::default()
            }
        ));
    }
}

fn expression_input() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("amount", DataType::Float64, true),
            Field::new("name", DataType::Utf8, true),
            Field::new("enabled", DataType::Boolean, true),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![Some(10.0), Some(-2.0), None])),
            Arc::new(StringArray::from(vec![Some(" Alpha "), Some("beta"), None])),
            Arc::new(BooleanArray::from(vec![Some(true), None, Some(false)])),
        ],
    )
    .expect("expression fixture")
}

#[test]
fn expression_ast_supports_typed_composition_without_code_execution() {
    let computed = execute_batch(
        expression_input(),
        &plan(
            "expression",
            json!({
                "output_column":"result",
                "output_type":"number",
                "expression":{
                    "kind":"case",
                    "branches":[{
                        "when":{"kind":"binary","op":"and",
                            "left":{"kind":"column","name":"enabled"},
                            "right":{"kind":"binary","op":"greater",
                                "left":{"kind":"column","name":"amount"},
                                "right":{"kind":"literal","value":0}}},
                        "then":{"kind":"binary","op":"multiply",
                            "left":{"kind":"column","name":"amount"},
                            "right":{"kind":"literal","value":2}}
                    }],
                    "else_value":{"kind":"literal","value":0}
                }
            }),
            Limits::default(),
        ),
    )
    .expect("case");
    let result = computed
        .column_by_name("result")
        .expect("result")
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("numbers");
    assert_eq!(result.values(), &[20.0, 0.0, 0.0]);

    let normalized = execute_batch(
        expression_input(),
        &plan(
            "expression",
            json!({
                "output_column":"normalized",
                "expression":{"kind":"function","name":"lower","args":[
                    {"kind":"function","name":"trim","args":[{"kind":"column","name":"name"}]}
                ]}
            }),
            Limits::default(),
        ),
    )
    .expect("nested function");
    let result = normalized
        .column_by_name("normalized")
        .expect("normalized")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("text");
    assert_eq!(result.value(0), "alpha");
    assert!(result.is_null(2));

    let divide_by_zero = plan(
        "expression",
        json!({"output_column":"x","expression":{"kind":"binary","op":"divide",
            "left":{"kind":"literal","value":1},"right":{"kind":"literal","value":0}}}),
        Limits::default(),
    );
    assert!(execute_batch(expression_input(), &divide_by_zero).is_err());

    let mut nested = json!({"kind":"literal","value":1});
    for _ in 0..65 {
        nested = json!({"kind":"unary","op":"negate","value":nested});
    }
    assert!(Plan {
        schema_version: 1,
        limits: Limits::default(),
        steps: vec![Step {
            operation: "expression".into(),
            config: json!({"output_column":"x","expression":nested}),
        }],
    }
    .validate()
    .is_err());
    assert!(serde_json::from_value::<Plan>(json!({
        "schema_version":1,
        "steps":[{"operation":"expression","config":{
            "output_column":"x",
            "expression":{"kind":"literal","value":1},
            "arbitrary_code":"rm -rf"
        }}]
    }))
    .is_ok_and(|candidate| candidate.validate().is_err()));
}

#[allow(clippy::needless_pass_by_value)] // JSON test values are intentionally ergonomic temporaries.
fn run_expression(
    expression: Value,
    output_type: Option<&str>,
) -> plenora_core::Result<RecordBatch> {
    let mut config = json!({"output_column":"out","expression":expression});
    if let Some(output_type) = output_type {
        config
            .as_object_mut()
            .expect("expression config")
            .insert("output_type".into(), json!(output_type));
    }
    execute_batch(
        expression_input(),
        &plan("expression", config, Limits::default()),
    )
}

#[test]
#[allow(clippy::too_many_lines)] // Exhaustive AST operator/function/error matrix.
fn every_expression_operator_function_and_error_policy_is_exercised() {
    let number = |value: i64| json!({"kind":"literal","value":value});
    let text = |value: &str| json!({"kind":"literal","value":value});
    let boolean = |value: bool| json!({"kind":"literal","value":value});
    let null_literal = || json!({"kind":"literal","value":null});

    for operator in ["add", "subtract", "multiply", "divide"] {
        assert!(run_expression(
            json!({"kind":"binary","op":operator,"left":number(8),"right":number(2)}),
            None,
        )
        .is_ok());
    }
    for operator in [
        "equal",
        "not_equal",
        "greater",
        "greater_equal",
        "less",
        "less_equal",
    ] {
        assert!(run_expression(
            json!({"kind":"binary","op":operator,"left":number(1),"right":number(2)}),
            None,
        )
        .is_ok());
    }
    for operator in [
        "equal",
        "not_equal",
        "greater",
        "greater_equal",
        "less",
        "less_equal",
    ] {
        assert!(run_expression(
            json!({"kind":"binary","op":operator,"left":number(2),"right":number(2)}),
            None,
        )
        .is_ok());
        assert!(run_expression(
            json!({"kind":"binary","op":operator,"left":number(3),"right":number(2)}),
            None,
        )
        .is_ok());
    }
    for operator in ["and", "or"] {
        assert!(run_expression(
            json!({"kind":"binary","op":operator,"left":boolean(true),"right":null_literal()}),
            None,
        )
        .is_ok());
        assert!(run_expression(
            json!({"kind":"binary","op":operator,"left":boolean(false),"right":boolean(false)}),
            None,
        )
        .is_ok());
    }
    for (operator, value) in [
        ("not", boolean(true)),
        ("negate", number(3)),
        ("is_null", null_literal()),
        ("is_not_null", text("x")),
    ] {
        assert!(run_expression(json!({"kind":"unary","op":operator,"value":value}), None,).is_ok());
    }

    let functions = [
        ("coalesce", json!([null_literal(), text("fallback")])),
        ("null_if", json!([text("x"), text("x")])),
        ("lower", json!([text("ÄBC")])),
        ("upper", json!([text("abc")])),
        ("trim", json!([text(" x ")])),
        ("length", json!([text("a💾")])),
        ("concat", json!([text("a"), text("b")])),
        ("contains", json!([text("abc"), text("b")])),
        ("starts_with", json!([text("abc"), text("a")])),
        ("ends_with", json!([text("abc"), text("c")])),
        ("abs", json!([number(-2)])),
        ("round", json!([json!({"kind":"literal","value":1.6})])),
        ("year", json!([text("2026-07-23T10:00:00")])),
    ];
    for (name, args) in functions {
        assert!(
            run_expression(json!({"kind":"function","name":name,"args":args}), None).is_ok(),
            "{name}"
        );
    }

    for expression in [
        json!({"kind":"unary","op":"not","value":number(1)}),
        json!({"kind":"unary","op":"negate","value":text("x")}),
        json!({"kind":"binary","op":"add","left":text("x"),"right":number(1)}),
        json!({"kind":"binary","op":"equal","left":text("1"),"right":number(1)}),
        json!({"kind":"function","name":"coalesce","args":[]}),
        json!({"kind":"function","name":"null_if","args":[text("x")]}),
        json!({"kind":"function","name":"concat","args":[number(1)]}),
        json!({"kind":"function","name":"year","args":[text("not-a-date")]}),
        json!({"kind":"case","branches":[{"when":number(1),"then":text("x")}],
            "else_value":text("y")}),
        json!({"kind":"column","name":"missing"}),
    ] {
        assert!(run_expression(expression, None).is_err());
    }
    assert!(run_expression(number(1), Some("boolean")).is_err());
    assert!(run_expression(text("x"), Some("number")).is_err());
    assert!(run_expression(number(1), Some("text")).is_err());
    assert!(run_expression(
        json!({"kind":"binary","op":"add","left":null_literal(),"right":number(1)}),
        None,
    )
    .is_ok());
    assert!(run_expression(
        json!({"kind":"binary","op":"multiply",
            "left":{"kind":"literal","value":1e308},
            "right":{"kind":"literal","value":1e308}}),
        None,
    )
    .is_err());
    assert!(run_expression(
        json!({"kind":"binary","op":"equal","left":boolean(true),"right":boolean(false)}),
        None,
    )
    .is_ok());
    assert!(run_expression(
        json!({"kind":"binary","op":"or","left":boolean(false),"right":null_literal()}),
        None,
    )
    .is_ok());
    let nan_input = single_column("amount", Arc::new(Float64Array::from(vec![f64::NAN])));
    assert!(execute_batch(
        nan_input,
        &plan(
            "expression",
            json!({"output_column":"out","expression":{"kind":"column","name":"amount"}}),
            Limits::default(),
        )
    )
    .is_err());
    for (name, args) in [
        ("null_if", json!([text("x"), text("y")])),
        ("lower", json!([null_literal()])),
        ("concat", json!([])),
        ("concat", json!([null_literal(), text("x")])),
        ("contains", json!([null_literal(), text("x")])),
        ("abs", json!([null_literal()])),
    ] {
        let expect_error = name == "concat" && args.as_array().is_some_and(Vec::is_empty);
        let result = run_expression(json!({"kind":"function","name":name,"args":args}), None);
        if expect_error {
            assert!(result.is_err());
        } else {
            assert!(result.is_ok());
        }
    }
    assert!(run_expression(
        json!({"kind":"case","branches":[
            {"when":{"kind":"column","name":"enabled"},"then":number(1)}
        ],"else_value":text("x")}),
        None,
    )
    .is_err());
    assert!(run_expression(null_literal(), None).is_ok());

    for invalid in [
        json!({"output_column":"out","expression":{"kind":"literal","value":[1,2]}}),
        json!({"output_column":"out","expression":{"kind":"case","branches":[],
            "else_value":{"kind":"literal","value":null}}}),
        json!({"output_column":"out","expression":{"kind":"function","name":"concat",
            "args":(0..65).map(|_| json!({"kind":"literal","value":"x"})).collect::<Vec<_>>()}}),
        json!({"output_column":"out","expression":{"kind":"column","name":""}}),
    ] {
        assert!(Plan {
            schema_version: 1,
            limits: Limits::default(),
            steps: vec![Step {
                operation: "expression".into(),
                config: invalid,
            }],
        }
        .validate()
        .is_err());
    }
    let wide = (0..17)
        .map(|_| json!({"kind":"literal","value":"x"}))
        .collect::<Vec<_>>();
    assert!(contract_is_invalid(
        "expression",
        json!({"output_column":"out","expression":{"kind":"function","name":"concat","args":wide}}),
        Limits {
            max_columns: 1,
            ..Limits::default()
        },
    ));
}

fn native_batch() -> RecordBatch {
    let timestamp: ArrayRef =
        Arc::new(TimestampMillisecondArray::from(vec![Some(1_000), None]).with_timezone("UTC"));
    let decimal: ArrayRef = Arc::new(
        Decimal128Array::from(vec![Some(1_234), None])
            .with_precision_and_scale(8, 2)
            .expect("decimal metadata"),
    );
    let mut dictionary = StringDictionaryBuilder::<Int32Type>::new();
    dictionary.append("alpha").expect("dictionary value");
    dictionary.append_null();
    let arrays: Vec<(&str, ArrayRef)> = vec![
        ("text", Arc::new(StringArray::from(vec![Some("x"), None]))),
        ("integer", Arc::new(Int64Array::from(vec![Some(-1), None]))),
        ("float", Arc::new(Float64Array::from(vec![Some(1.5), None]))),
        ("flag", Arc::new(BooleanArray::from(vec![Some(true), None]))),
        ("unsigned", Arc::new(UInt64Array::from(vec![Some(2), None]))),
        ("date", Arc::new(Date32Array::from(vec![Some(0), None]))),
        ("timestamp", timestamp),
        ("decimal", decimal),
        (
            "binary",
            Arc::new(BinaryArray::from(vec![Some(&b"bytes"[..]), None])),
        ),
        ("dictionary", Arc::new(dictionary.finish())),
    ];
    RecordBatch::try_new(
        Arc::new(Schema::new(
            arrays
                .iter()
                .map(|(name, array)| Field::new(*name, array.data_type().clone(), true))
                .collect::<Vec<_>>(),
        )),
        arrays.into_iter().map(|(_, array)| array).collect(),
    )
    .expect("native batch")
}

#[test]
#[allow(clippy::too_many_lines)] // Full native-type interoperability matrix.
fn native_types_interoperate_with_setops_expressions_and_governance() {
    let input = native_batch();
    let schema_fields = [
        ("text", "utf8"),
        ("integer", "int64"),
        ("float", "float64"),
        ("flag", "boolean"),
        ("unsigned", "uint64"),
        ("date", "date32"),
        ("timestamp", "timestamp_millis"),
        ("decimal", "decimal128"),
        ("binary", "binary"),
        ("dictionary", "dictionary_utf8"),
    ]
    .into_iter()
    .map(|(name, data_type)| json!({"name":name,"data_type":data_type,"nullable":true}))
    .collect::<Vec<_>>();
    assert!(execute_batch(
        input.clone(),
        &plan(
            "assert_schema",
            json!({"fields":schema_fields,"allow_extra":false,"ordered":true}),
            Limits::default(),
        )
    )
    .is_ok());
    for operation in ["union_distinct", "intersect", "except"] {
        assert!(execute_binary(
            &input,
            &input,
            &plan(operation, json!({}), Limits::default())
        )
        .is_ok());
    }
    for column in ["unsigned", "date", "timestamp", "decimal"] {
        let output = execute_batch(
            input.clone(),
            &plan(
                "expression",
                json!({"output_column":"out","output_type":"number",
                    "expression":{"kind":"column","name":column}}),
                Limits::default(),
            ),
        )
        .expect(column);
        assert_eq!(output.num_rows(), 2);
    }
    for column in ["binary", "dictionary"] {
        let output = execute_batch(
            input.clone(),
            &plan(
                "expression",
                json!({"output_column":"out","output_type":"text",
                    "expression":{"kind":"column","name":column}}),
                Limits::default(),
            ),
        )
        .expect(column);
        assert_eq!(output.num_rows(), 2);
    }
    for column in [
        "unsigned",
        "date",
        "timestamp",
        "decimal",
        "binary",
        "dictionary",
    ] {
        assert!(execute_batch(
            input.clone(),
            &plan(
                "type_cast",
                json!({"column":column,"target_type":"str","errors":"raise"}),
                Limits::default(),
            )
        )
        .is_ok());
    }
    let keys = input
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect::<Vec<_>>();
    assert!(execute_binary(
        &input,
        &input,
        &plan(
            "assert_foreign_key",
            json!({"left_keys":keys,"right_keys":keys,"allow_null":true}),
            Limits::default(),
        )
    )
    .is_ok());
    assert!(execute_batch(
        input,
        &plan(
            "assert_unique",
            json!({"columns":keys,"nulls_equal":true}),
            Limits::default(),
        )
    )
    .is_ok());
}

fn single_column(name: &str, array: ArrayRef) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            name,
            array.data_type().clone(),
            true,
        )])),
        vec![array],
    )
    .expect("single column")
}

#[test]
fn invalid_native_scalar_encodings_are_rejected_without_guessing() {
    let invalid_binary = single_column(
        "value",
        Arc::new(BinaryArray::from(vec![Some(&[0xff_u8][..])])),
    );
    let extreme_date = single_column("value", Arc::new(Date32Array::from(vec![i32::MAX])));
    let invalid_timestamp = single_column(
        "value",
        Arc::new(TimestampMillisecondArray::from(vec![i64::MAX]).with_timezone("Invalid/Timezone")),
    );
    let negative_scale = single_column(
        "value",
        Arc::new(
            Decimal128Array::from(vec![123_i128])
                .with_precision_and_scale(8, -2)
                .expect("negative Arrow scale"),
        ),
    );
    let unsupported_list =
        ListArray::from_iter_primitive::<Int64Type, _, _>(vec![Some(vec![Some(1_i64)])]);
    for (label, input) in [
        ("binary", invalid_binary),
        ("list", single_column("value", Arc::new(unsupported_list))),
    ] {
        assert!(
            execute_batch(
                input,
                &plan(
                    "expression",
                    json!({"output_column":"out","expression":{"kind":"column","name":"value"}}),
                    Limits::default(),
                )
            )
            .is_err(),
            "{label}"
        );
    }
    assert!(execute_batch(
        extreme_date,
        &plan(
            "expression",
            json!({"output_column":"out","expression":{"kind":"column","name":"value"}}),
            Limits::default(),
        )
    )
    .is_ok());
    for input in [invalid_timestamp, negative_scale] {
        assert!(execute_batch(
            input,
            &plan(
                "type_cast",
                json!({"column":"value","target_type":"str","errors":"raise"}),
                Limits::default(),
            )
        )
        .is_err());
    }
}

#[test]
fn advanced_windows_have_exact_tie_and_partition_semantics() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("value", DataType::Float64, false),
            Field::new("group", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![10.0, 20.0, 20.0, 40.0])),
            Arc::new(StringArray::from(vec!["a", "a", "a", "a"])),
        ],
    )
    .expect("window fixture");
    for (function, extra, expected) in [
        (
            "percent_rank",
            json!({}),
            vec![0.0, 1.0 / 3.0, 1.0 / 3.0, 1.0],
        ),
        ("cume_dist", json!({}), vec![0.25, 0.75, 0.75, 1.0]),
        ("ntile", json!({"buckets":2}), vec![1.0, 1.0, 2.0, 2.0]),
    ] {
        let mut config = json!({
            "column":"value",
            "function":function,
            "group_by":"group",
            "order_column":null,
            "output_column":"out"
        });
        config
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("extra").clone());
        let output = execute_batch(
            input.clone(),
            &plan("window_function", config, Limits::default()),
        )
        .expect(function);
        let actual = output
            .column_by_name("out")
            .expect("out")
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("window values");
        assert_eq!(actual.values(), expected.as_slice(), "{function}");
    }
}

#[test]
fn aggregation_and_window_null_edge_policies_are_explicit() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("group", DataType::Utf8, false),
            Field::new("number", DataType::Float64, true),
            Field::new("text", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["a", "a", "b"])),
            Arc::new(Float64Array::from(vec![None, None, Some(4.0)])),
            Arc::new(StringArray::from(vec![None, Some("x"), None])),
        ],
    )
    .expect("aggregation edges");
    for aggregation in [
        json!({"column":"text","function":"nunique","skip_null":false,"alias":"out"}),
        json!({"column":"text","function":"concat","skip_null":false,"alias":"out"}),
        json!({"column":"number","function":"sum","skip_null":true,"alias":"out"}),
        json!({"column":"number","function":"sum","skip_null":false,"alias":"out"}),
        json!({"column":"number","function":"variance","ddof":2,"alias":"out"}),
        json!({"column":"number","function":"stddev","ddof":2,"alias":"out"}),
    ] {
        assert!(execute_batch(
            input.clone(),
            &plan(
                "aggregate",
                json!({"group_by":["group"],"aggregations":[aggregation]}),
                Limits::default(),
            )
        )
        .is_ok());
    }
    for function in ["min", "max", "stddev"] {
        assert!(execute_batch(
            input.clone(),
            &plan(
                "rolling_window",
                json!({"column":"number","function":function,"group_by":"group",
                    "order_column":null,"window":2,"min_periods":1,"ddof":2,
                    "output_column":"out"}),
                Limits::default(),
            )
        )
        .is_ok());
    }
    assert!(execute_batch(
        input,
        &plan(
            "window_function",
            json!({"column":"number","function":"percent_rank","group_by":"group",
                "order_column":null,"output_column":"out"}),
            Limits::default(),
        )
    )
    .is_ok());
}

fn ids(values: Vec<Option<i64>>, metadata: HashMap<String, String>) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            vec![Field::new("id", DataType::Int64, true)],
            metadata,
        )),
        vec![Arc::new(Int64Array::from(values))],
    )
    .expect("ids")
}

#[test]
#[allow(clippy::too_many_lines)] // Governance success and failure policies belong to one fixture.
fn governance_checks_cardinality_metadata_foreign_keys_and_reconciliation() {
    let metadata = HashMap::from([
        ("source".to_owned(), "erp".to_owned()),
        ("contract".to_owned(), "v2".to_owned()),
    ]);
    let left = ids(vec![Some(1), Some(1), Some(2), None], metadata.clone());
    let right = ids(vec![Some(1), Some(3), Some(3)], metadata);
    assert!(execute_batch(
        left.clone(),
        &plan(
            "assert_cardinality",
            json!({"min_rows":3,"max_rows":4}),
            Limits::default(),
        ),
    )
    .is_ok());
    assert!(execute_batch(
        left.clone(),
        &plan(
            "assert_cardinality",
            json!({"exact_rows":3}),
            Limits::default(),
        ),
    )
    .is_err());
    assert!(execute_batch(
        left.clone(),
        &plan(
            "assert_metadata",
            json!({"expected":{"source":"erp"},"allow_extra":true}),
            Limits::default(),
        ),
    )
    .is_ok());
    assert!(execute_batch(
        left.clone(),
        &plan(
            "assert_metadata",
            json!({"expected":{"source":"erp"},"allow_extra":false}),
            Limits::default(),
        ),
    )
    .is_err());

    let valid_fk = plan(
        "assert_foreign_key",
        json!({"left_keys":["id"],"right_keys":["id"],"allow_null":true}),
        Limits::default(),
    );
    assert!(execute_binary(&ids(vec![Some(1), None], HashMap::new()), &right, &valid_fk).is_ok());
    assert!(execute_binary(&left, &right, &valid_fk).is_err());
    let reject_null_fk = plan(
        "assert_foreign_key",
        json!({"left_keys":["id"],"right_keys":["id"],"allow_null":false}),
        Limits::default(),
    );
    assert!(execute_binary(&ids(vec![None], HashMap::new()), &right, &reject_null_fk).is_err());
    let tiny_fk = plan(
        "assert_foreign_key",
        json!({"left_keys":["id"],"right_keys":["id"],"allow_null":true}),
        Limits {
            max_memory_bytes: 1,
            ..Limits::default()
        },
    );
    assert!(execute_binary(&left, &right, &tiny_fk).is_err());
    assert!(execute_binary(&left, &strings(vec![Some("1")]), &valid_fk).is_err());

    let reconciliation = execute_binary(
        &left,
        &right,
        &plan(
            "reconcile",
            json!({"left_keys":["id"],"right_keys":["id"],"nulls_equal":false}),
            Limits::default(),
        ),
    )
    .expect("reconcile");
    let metrics = reconciliation
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("metrics");
    let values = reconciliation
        .column(1)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("values");
    let actual = (0..reconciliation.num_rows())
        .map(|row| (metrics.value(row), values.value(row)))
        .collect::<HashMap<_, _>>();
    assert_eq!(actual["matched_rows"], 1);
    assert_eq!(actual["left_only_rows"], 3);
    assert_eq!(actual["right_only_rows"], 2);
    assert_eq!(actual["left_duplicate_rows"], 1);
    assert_eq!(actual["right_duplicate_rows"], 1);

    let tiny_reconcile = plan(
        "reconcile",
        json!({"left_keys":["id"],"right_keys":["id"],"nulls_equal":true}),
        Limits {
            max_memory_bytes: 1,
            ..Limits::default()
        },
    );
    assert!(execute_binary(&left, &right, &tiny_reconcile).is_err());
    assert!(execute_binary(
        &left,
        &strings(vec![Some("1")]),
        &plan(
            "reconcile",
            json!({"left_keys":["id"],"right_keys":["value"]}),
            Limits::default(),
        )
    )
    .is_err());
}

fn set_batch(values: &[i64]) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(values.to_vec()))],
    )
    .expect("set batch")
}

fn int_values(batch: &RecordBatch) -> Vec<i64> {
    batch
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("ints")
        .values()
        .to_vec()
}

#[test]
fn disk_spill_is_semantically_identical_ordered_and_quota_bounded() {
    let left_values = (0..400)
        .map(|value| i64::from(value % 173))
        .collect::<Vec<_>>();
    let right_values = (80..500)
        .map(|value| i64::from(value % 211))
        .collect::<Vec<_>>();
    let left = set_batch(&left_values);
    let right = set_batch(&right_values);
    for operation in ["union_distinct", "intersect", "except"] {
        let in_memory = execute_binary(
            &left,
            &right,
            &plan(operation, json!({}), Limits::default()),
        )
        .expect("in memory");
        let spill_limits = Limits {
            max_memory_bytes: 2_048,
            spill_partitions: 64,
            ..Limits::default()
        };
        let spilled = execute_binary(&left, &right, &plan(operation, json!({}), spill_limits))
            .unwrap_or_else(|error| panic!("{operation}: {error}"));
        assert_eq!(int_values(&spilled), int_values(&in_memory), "{operation}");
    }

    let quota = Limits {
        max_memory_bytes: 2_048,
        max_temp_bytes: 32,
        spill_partitions: 8,
        ..Limits::default()
    };
    assert!(execute_binary(&left, &right, &plan("intersect", json!({}), quota)).is_err());
}
