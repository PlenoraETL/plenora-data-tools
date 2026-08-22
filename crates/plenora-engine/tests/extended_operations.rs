use std::sync::Arc;

use plenora_core::arrow::array::types::Int64Type;
use plenora_core::arrow::array::{
    Array, ArrayRef, Float64Array, Int64Array, ListArray, RecordBatch, StringArray, StructArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::{
    execute_binary, execute_complete_batch as execute_batch, Limits, Plan, Step, ValidatedPlan,
};
use serde_json::{json, Value};

fn plan(operation: &str, config: Value) -> ValidatedPlan {
    Plan {
        schema_version: 1,
        limits: Limits::default(),
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .unwrap_or_else(|error| panic!("{operation}: {error}"))
}

fn flat() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("group", DataType::Utf8, false),
            Field::new("value", DataType::Float64, true),
            Field::new("fallback", DataType::Float64, true),
            Field::new("start", DataType::Utf8, false),
            Field::new("end", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3])),
            Arc::new(StringArray::from(vec!["a", "a", "b"])),
            Arc::new(Float64Array::from(vec![Some(1.0), None, Some(3.0)])),
            Arc::new(Float64Array::from(vec![Some(9.0), Some(2.0), None])),
            Arc::new(StringArray::from(vec![
                "2024-01-31 01:00:00",
                "2024-03-31 01:30:00",
                "2024-10-27 02:30:00",
            ])),
            Arc::new(StringArray::from(vec![
                "2024-02-01 01:00:00",
                "2024-04-01 03:30:00",
                "2024-10-28 02:30:00",
            ])),
        ],
    )
    .expect("flat fixture")
}

#[test]
fn quality_guards_and_coalesce_are_fail_closed() {
    let input = flat();
    for (operation, config) in [
        (
            "assert_schema",
            json!({"fields":[{"name":"id","data_type":"int64","nullable":false}],"allow_extra":true,"ordered":true}),
        ),
        ("assert_not_null", json!({"columns":["id","group"]})),
        (
            "assert_unique",
            json!({"columns":["id"],"nulls_equal":true}),
        ),
        (
            "assert_range",
            json!({"column":"id","min":1.0,"max":3.0,"allow_null":false}),
        ),
        (
            "assert_regex",
            json!({"column":"group","pattern":"^[ab]$","allow_null":false}),
        ),
    ] {
        assert_eq!(
            execute_batch(input.clone(), &plan(operation, config))
                .expect(operation)
                .num_rows(),
            3
        );
    }
    let result = execute_batch(
        input.clone(),
        &plan(
            "coalesce",
            json!({"columns":["value","fallback"],"output_column":"chosen"}),
        ),
    )
    .expect("coalesce");
    let chosen = result
        .column_by_name("chosen")
        .expect("chosen")
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("float");
    assert_eq!(chosen.values(), &[1.0, 2.0, 3.0]);
    assert!(execute_batch(
        input.clone(),
        &plan("assert_not_null", json!({"columns":["value"]}))
    )
    .is_err());
    assert!(execute_batch(
        input,
        &plan("assert_regex", json!({"column":"group","pattern":"^z$"}))
    )
    .is_err());
}

#[test]
fn temporal_operations_cover_calendar_diff_and_dst() {
    let input = flat();
    let formatted = execute_batch(input.clone(), &plan("date_format", json!({"column":"start","input_format":"%Y-%m-%d %H:%M:%S","output_format":"%d/%m/%Y","output_column":"formatted","invalid":"error"}))).expect("format");
    let formatted = formatted
        .column_by_name("formatted")
        .expect("formatted")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("utf8");
    assert_eq!(formatted.value(0), "31/01/2024");

    let added = execute_batch(input.clone(), &plan("date_add", json!({"column":"start","input_format":"%Y-%m-%d %H:%M:%S","output_format":"%Y-%m-%d","amount":1,"unit":"months","output_column":"added","invalid":"error"}))).expect("add");
    let added = added
        .column_by_name("added")
        .expect("added")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("utf8");
    assert_eq!(added.value(0), "2024-02-29");

    let diff = execute_batch(input.clone(), &plan("date_diff", json!({"start_column":"start","end_column":"end","input_format":"%Y-%m-%d %H:%M:%S","unit":"hours","output_column":"hours","invalid":"error"}))).expect("diff");
    let hours = diff
        .column_by_name("hours")
        .expect("hours")
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("float");
    assert!((hours.value(0) - 24.0).abs() < f64::EPSILON);

    let converted = execute_batch(input, &plan("timezone_convert", json!({"column":"start","input_format":"%Y-%m-%d %H:%M:%S","output_format":"%Y-%m-%d %H:%M:%S %Z","source_timezone":"Europe/Rome","target_timezone":"UTC","output_column":"utc","invalid":"error","ambiguous":"earliest"}))).expect_err("ambiguita' DST rimediata");
    assert_eq!(
        converted.row_diagnostics().expect("diagnostica DST").counts
            ["conversion.ambiguous_local_time"],
        1
    );
}

#[test]
fn sha256_is_stable_framed_and_column_order_independent() {
    let input = flat();
    let first = execute_batch(input.clone(), &plan("sha256_hash", json!({"columns":["group","id"],"output_column":"digest","normalize":true,"null_policy":"literal","null_literal":"NULL"}))).expect("sha");
    let second = execute_batch(input, &plan("sha256_hash", json!({"columns":["id","group"],"output_column":"digest","normalize":true,"null_policy":"literal","null_literal":"NULL"}))).expect("sha reordered");
    let first = first
        .column_by_name("digest")
        .expect("digest")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("utf8");
    let second = second
        .column_by_name("digest")
        .expect("digest")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("utf8");
    assert_eq!(first, second);
    assert_eq!(first.value(0).len(), 64);
    assert_ne!(first.value(0), first.value(1));
}

#[test]
fn explode_and_unnest_preserve_arrow_types() {
    let lists = ListArray::from_iter_primitive::<Int64Type, _, _>(vec![
        Some(vec![Some(1), Some(2)]),
        None,
        Some(Vec::<Option<i64>>::new()),
    ]);
    let list_type = lists.data_type().clone();
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("items", list_type, true)])),
        vec![Arc::new(lists)],
    )
    .expect("list fixture");
    let exploded = execute_batch(
        input,
        &plan(
            "explode",
            json!({"column":"items","output_column":"item","empty_policy":"null"}),
        ),
    )
    .expect("explode");
    assert_eq!(exploded.num_rows(), 4);
    assert_eq!(
        exploded.column_by_name("item").expect("item").data_type(),
        &DataType::Int64
    );

    let struct_array = StructArray::from(vec![
        (
            Arc::new(Field::new("x", DataType::Int64, false)),
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
        ),
        (
            Arc::new(Field::new("label", DataType::Utf8, true)),
            Arc::new(StringArray::from(vec![Some("a"), None])) as ArrayRef,
        ),
    ]);
    let struct_type = struct_array.data_type().clone();
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("payload", struct_type, false)])),
        vec![Arc::new(struct_array)],
    )
    .expect("struct fixture");
    let unnested = execute_batch(
        input,
        &plan(
            "unnest",
            json!({"column":"payload","prefix":"p_","drop_source":true}),
        ),
    )
    .expect("unnest");
    assert_eq!(
        unnested
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect::<Vec<_>>(),
        vec!["p_x", "p_label"]
    );
}

fn pair() -> (RecordBatch, RecordBatch) {
    let left = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("time", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3, 3])),
            Arc::new(Float64Array::from(vec![1.0, 4.0, 7.0, 7.0])),
        ],
    )
    .expect("left");
    let right = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("time", DataType::Float64, false),
            Field::new("label", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![2, 3, 4])),
            Arc::new(Float64Array::from(vec![2.0, 6.0, 9.0])),
            Arc::new(StringArray::from(vec!["two", "three", "four"])),
        ],
    )
    .expect("right");
    (left, right)
}

#[test]
fn membership_and_asof_joins_have_stable_cardinality() {
    let (left, right) = pair();
    let semi = execute_binary(
        &left,
        &right,
        &plan("semi_join", json!({"left_keys":["id"],"right_keys":["id"]})),
    )
    .expect("semi");
    let anti = execute_binary(
        &left,
        &right,
        &plan("anti_join", json!({"left_keys":["id"],"right_keys":["id"]})),
    )
    .expect("anti");
    assert_eq!((semi.num_rows(), anti.num_rows()), (3, 1));
    let asof = execute_binary(&left, &right, &plan("asof_join", json!({"left_on":"time","right_on":"time","direction":"backward","tolerance":2.0,"allow_exact":true}))).expect("asof");
    assert_eq!(asof.num_rows(), left.num_rows());
    assert!(asof.column_by_name("label").expect("label").is_null(0));
}

fn set_pair() -> (RecordBatch, RecordBatch) {
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    (
        RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 2, 3]))],
        )
        .expect("left set"),
        RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![2, 3, 4]))])
            .expect("right set"),
    )
}

#[test]
fn set_operations_are_distinct_and_stable() {
    let (left, right) = set_pair();
    let union = execute_binary(&left, &right, &plan("union_distinct", json!({}))).expect("union");
    let intersection =
        execute_binary(&left, &right, &plan("intersect", json!({}))).expect("intersect");
    let difference = execute_binary(&left, &right, &plan("except", json!({}))).expect("except");
    assert_eq!(
        (
            union.num_rows(),
            intersection.num_rows(),
            difference.num_rows()
        ),
        (4, 2, 1)
    );
}

#[test]
fn extended_aggregation_and_rolling_statistics_are_exact() {
    let input = flat();
    for (function, extra) in [
        ("nunique", json!({})),
        ("variance", json!({"ddof":1})),
        ("stddev", json!({"ddof":1})),
        ("quantile", json!({"quantile":0.5})),
    ] {
        let mut aggregation = json!({"column":"id","function":function,"alias":"result"});
        aggregation
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("extra").clone());
        let output = execute_batch(
            input.clone(),
            &plan(
                "aggregate",
                json!({"group_by":["group"],"aggregations":[aggregation]}),
            ),
        )
        .expect(function);
        assert_eq!(output.num_rows(), 2);
    }
    let rolling = execute_batch(input, &plan("rolling_window", json!({"column":"id","function":"mean","group_by":"group","order_column":"id","window":2,"min_periods":1,"output_column":"rolling"}))).expect("rolling");
    let values = rolling
        .column_by_name("rolling")
        .expect("rolling")
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("float");
    assert_eq!(values.values(), &[1.0, 1.5, 3.0]);
}

#[test]
fn invalid_extended_contracts_are_rejected_before_execution() {
    for (operation, config) in [
        ("assert_range", json!({"column":"id"})),
        ("assert_regex", json!({"column":"group","pattern":"("})),
        ("coalesce", json!({"columns":[],"output_column":"x"})),
        (
            "date_format",
            json!({"column":"start","input_format":"%Q","output_column":"x"}),
        ),
        (
            "timezone_convert",
            json!({"column":"start","input_format":"%Y","source_timezone":"Mars/Olympus","target_timezone":"UTC","output_column":"x"}),
        ),
        (
            "asof_join",
            json!({"left_on":"time","right_on":"time","tolerance":-1.0}),
        ),
        (
            "rolling_window",
            json!({"column":"id","function":"mean","window":0,"output_column":"x"}),
        ),
        (
            "aggregate",
            json!({"group_by":["group"],"aggregations":[{"column":"id","function":"quantile","quantile":2.0}]}),
        ),
    ] {
        assert!(
            Plan {
                schema_version: 1,
                limits: Limits::default(),
                steps: vec![Step {
                    operation: operation.into(),
                    config
                }]
            }
            .validate()
            .is_err(),
            "{operation}"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn quality_policy_matrix_covers_null_type_order_and_boundary_failures() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("text", DataType::Utf8, true),
            Field::new("integer", DataType::Int64, true),
            Field::new("float", DataType::Float64, true),
            Field::new("flag", DataType::Boolean, true),
        ])),
        vec![
            Arc::new(StringArray::from(vec![Some("ok"), None, Some("bad")])),
            Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])),
            Arc::new(Float64Array::from(vec![Some(1.0), Some(f64::NAN), None])),
            Arc::new(plenora_core::arrow::array::BooleanArray::from(vec![
                Some(true),
                None,
                Some(false),
            ])),
        ],
    )
    .expect("quality fixture");

    let schema = plan(
        "assert_schema",
        json!({"fields":[
            {"name":"flag","data_type":"bool","nullable":true},
            {"name":"text","data_type":"string","nullable":true},
            {"name":"float","data_type":"double","nullable":true},
            {"name":"integer","data_type":"integer","nullable":true}
        ],"allow_extra":false,"ordered":false}),
    );
    assert!(execute_batch(input.clone(), &schema).is_ok());
    for config in [
        json!({"fields":[{"name":"text","data_type":"utf8"}],"allow_extra":false}),
        json!({"fields":[{"name":"integer","data_type":"int64"}],"allow_extra":true,"ordered":true}),
        json!({"fields":[{"name":"text","data_type":"float64"}],"allow_extra":true}),
        json!({"fields":[{"name":"text","data_type":"utf8","nullable":false}],"allow_extra":true}),
    ] {
        assert!(execute_batch(input.clone(), &plan("assert_schema", config)).is_err());
    }

    assert!(execute_batch(
        input.clone(),
        &plan(
            "assert_unique",
            json!({"columns":["integer"],"nulls_equal":false})
        )
    )
    .is_ok());
    let duplicates = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, true)])),
        vec![Arc::new(Int64Array::from(vec![
            Some(1),
            Some(1),
            None,
            None,
        ]))],
    )
    .expect("duplicates");
    assert!(execute_batch(
        duplicates.clone(),
        &plan("assert_unique", json!({"columns":["x"],"nulls_equal":true}))
    )
    .is_err());
    assert!(execute_batch(
        duplicates,
        &plan(
            "assert_unique",
            json!({"columns":["x"],"nulls_equal":false})
        )
    )
    .is_err());

    assert!(execute_batch(
        input.clone(),
        &plan(
            "assert_range",
            json!({"column":"integer","min":0.0,"max":4.0,"allow_null":true})
        )
    )
    .is_ok());
    for config in [
        json!({"column":"integer","min":1.0,"inclusive_min":false,"allow_null":true}),
        json!({"column":"integer","max":3.0,"inclusive_max":false,"allow_null":true}),
        json!({"column":"integer","min":0.0,"max":4.0,"allow_null":false}),
        json!({"column":"float","min":0.0,"max":4.0,"allow_null":true}),
        json!({"column":"text","min":0.0,"allow_null":true}),
    ] {
        assert!(execute_batch(input.clone(), &plan("assert_range", config)).is_err());
    }
    assert!(execute_batch(
        input.clone(),
        &plan(
            "assert_regex",
            json!({"column":"text","pattern":"^(ok|bad)$","allow_null":true})
        )
    )
    .is_ok());
    assert!(execute_batch(
        input.clone(),
        &plan("assert_regex", json!({"column":"integer","pattern":".*"}))
    )
    .is_err());
    assert!(execute_batch(
        input,
        &plan(
            "coalesce",
            json!({"columns":["integer","float"],"output_column":"x"})
        )
    )
    .is_err());
}

#[test]
#[allow(clippy::too_many_lines)]
fn temporal_policy_matrix_covers_every_unit_and_dst_failure_mode() {
    let temporal = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("value", DataType::Utf8, true),
            Field::new("other", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(StringArray::from(vec![
                Some("2024-02-29 12:30:45"),
                Some("invalid"),
                None,
            ])),
            Arc::new(StringArray::from(vec![
                Some("2024-03-01 13:31:46"),
                Some("2024-01-01 00:00:00"),
                None,
            ])),
        ],
    )
    .expect("temporal fixture");
    let format = "%Y-%m-%d %H:%M:%S";
    let formatted = execute_batch(
        temporal.clone(),
        &plan(
            "date_format",
            json!({"column":"value","input_format":format,"output_column":"out","invalid":"null"}),
        ),
    )
    .expect_err("policy null legacy ha rimediato data invalida");
    assert!(formatted.row_diagnostics().is_some());
    assert!(execute_batch(
        temporal.clone(),
        &plan(
            "date_format",
            json!({"column":"value","input_format":format,"output_column":"out","invalid":"error"})
        )
    )
    .is_err());
    for unit in [
        "years", "months", "weeks", "days", "hours", "minutes", "seconds",
    ] {
        assert!(execute_batch(
            temporal.clone(),
            &plan(
                "date_add",
                json!({"column":"value","input_format":format,"amount":-1,"unit":unit,"output_column":"out","invalid":"null"})
            )
        )
        .is_err());
    }
    assert!(execute_batch(
        temporal.clone(),
        &plan(
            "date_add",
            json!({"column":"value","input_format":format,"amount":9_223_372_036_854_775_807_i64,"unit":"years","output_column":"out","invalid":"error"})
        )
    )
    .is_err());
    for unit in ["days", "hours", "minutes", "seconds"] {
        assert!(execute_batch(
            temporal.clone(),
            &plan(
                "date_diff",
                json!({"start_column":"value","end_column":"other","input_format":format,"unit":unit,"output_column":"out","invalid":"null"})
            )
        )
        .is_err());
    }
    assert!(execute_batch(
        temporal,
        &plan(
            "date_diff",
            json!({"start_column":"value","end_column":"other","input_format":format,"unit":"seconds","output_column":"out","invalid":"error"})
        )
    )
    .is_err());

    let dst = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("local", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(vec![
            Some("2024-10-27 02:30:00"),
            Some("2024-03-31 02:30:00"),
            Some("invalid"),
            None,
        ]))],
    )
    .expect("dst fixture");
    for ambiguous in ["earliest", "latest", "null"] {
        assert!(execute_batch(
            dst.clone(),
            &plan(
                "timezone_convert",
                json!({"column":"local","input_format":format,"source_timezone":"Europe/Rome","target_timezone":"UTC","output_column":"out","invalid":"null","ambiguous":ambiguous})
            )
        )
        .is_err());
    }
    assert!(execute_batch(
        dst,
        &plan(
            "timezone_convert",
            json!({"column":"local","input_format":format,"source_timezone":"Europe/Rome","target_timezone":"UTC","output_column":"out","invalid":"error","ambiguous":"error"})
        )
    )
    .is_err());
}

#[test]
#[allow(clippy::too_many_lines)]
fn aggregation_and_asof_policy_matrix_covers_alternative_branches() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("group", DataType::Utf8, false),
            Field::new("value", DataType::Float64, true),
            Field::new("order", DataType::Int64, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["a", "a", "a", "b"])),
            Arc::new(Float64Array::from(vec![
                Some(1.0),
                Some(1.0),
                None,
                Some(4.0),
            ])),
            Arc::new(Int64Array::from(vec![3, 1, 2, 1])),
        ],
    )
    .expect("aggregation fixture");
    let output = execute_batch(
        input.clone(),
        &plan(
            "aggregate",
            json!({"group_by":["group"],"aggregations":[
                {"column":"value","function":"count","alias":"count"},
                {"column":"value","function":"nunique","alias":"unique_with_null","skip_null":false},
                {"column":"value","function":"sum","alias":"distinct_sum","distinct":true},
                {"column":"value","function":"avg","alias":"invalid_mean","skip_null":false},
                {"column":"value","function":"min","alias":"min"},
                {"column":"value","function":"max","alias":"max"},
                {"column":"group","function":"first","alias":"first"},
                {"column":"group","function":"last","alias":"last"},
                {"column":"group","function":"concat","alias":"joined","distinct":true,"separator":"|"},
                {"column":"value","function":"variance","alias":"variance","ddof":10},
                {"column":"value","function":"stddev","alias":"stddev","ddof":0},
                {"column":"value","function":"quantile","alias":"q25","quantile":0.25}
            ]}),
        ),
    )
    .expect("aggregation matrix");
    assert_eq!(output.num_rows(), 2);
    assert!(output
        .column_by_name("invalid_mean")
        .expect("invalid mean")
        .is_null(0));
    assert!(output
        .column_by_name("variance")
        .expect("variance")
        .is_null(0));

    for function in ["sum", "mean", "min", "max", "stddev"] {
        let rolling = execute_batch(
            input.clone(),
            &plan(
                "rolling_window",
                json!({"column":"value","function":function,"group_by":"group","order_column":"order","window":3,"min_periods":2,"ddof":5,"output_column":"rolling"}),
            ),
        )
        .expect("rolling variant");
        assert_eq!(rolling.num_rows(), input.num_rows());
    }

    let left = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "time",
            DataType::Float64,
            false,
        )])),
        vec![Arc::new(Float64Array::from(vec![2.0, 5.0, 8.0]))],
    )
    .expect("asof left");
    let right = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("time", DataType::Float64, false),
            Field::new("label", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![2.0, 4.0, 6.0, 9.0])),
            Arc::new(StringArray::from(vec!["exact", "four", "six", "nine"])),
        ],
    )
    .expect("asof right");
    for direction in ["backward", "forward", "nearest"] {
        let output = execute_binary(
            &left,
            &right,
            &plan(
                "asof_join",
                json!({"left_on":"time","right_on":"time","direction":direction,"allow_exact":false}),
            ),
        )
        .expect("asof direction");
        assert_eq!(output.num_rows(), left.num_rows());
    }
    let nearest = execute_binary(
        &left,
        &right,
        &plan(
            "asof_join",
            json!({"left_on":"time","right_on":"time","direction":"nearest","allow_exact":true}),
        ),
    )
    .expect("asof nearest");
    let labels = nearest
        .column_by_name("label")
        .expect("label")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("utf8");
    assert_eq!(labels.value(0), "exact");
    assert_eq!(labels.value(1), "four");
}

#[test]
fn nested_and_set_operations_enforce_resource_and_schema_guards() {
    let lists =
        ListArray::from_iter_primitive::<Int64Type, _, _>(vec![Some(vec![Some(1), Some(2)])]);
    let list_type = lists.data_type().clone();
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("items", list_type, false)])),
        vec![Arc::new(lists)],
    )
    .expect("list fixture");
    let limited = Plan {
        schema_version: 1,
        limits: Limits {
            max_rows: 1,
            ..Limits::default()
        },
        steps: vec![Step {
            operation: "explode".into(),
            config: json!({"column":"items","empty_policy":"drop"}),
        }],
    }
    .validate()
    .expect("limited explode");
    assert!(execute_batch(input.clone(), &limited).is_err());
    for operation in ["union_distinct", "intersect", "except"] {
        assert!(execute_binary(&input, &input, &plan(operation, json!({}))).is_err());
    }
    assert!(execute_batch(
        flat(),
        &plan("explode", json!({"column":"id","empty_policy":"drop"}))
    )
    .is_err());

    let structure = StructArray::from(vec![(
        Arc::new(Field::new("x", DataType::Int64, false)),
        Arc::new(Int64Array::from(vec![1])) as ArrayRef,
    )]);
    let struct_type = structure.data_type().clone();
    let collision = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("payload", struct_type, false),
            Field::new("p_x", DataType::Int64, false),
        ])),
        vec![Arc::new(structure), Arc::new(Int64Array::from(vec![9]))],
    )
    .expect("collision fixture");
    assert!(execute_batch(
        collision,
        &plan(
            "unnest",
            json!({"column":"payload","prefix":"p_","drop_source":false})
        )
    )
    .is_err());
    assert!(execute_batch(
        flat(),
        &plan("unnest", json!({"column":"id","drop_source":true}))
    )
    .is_err());

    let (left, _) = set_pair();
    let incompatible = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "other",
            DataType::Int64,
            false,
        )])),
        vec![Arc::new(Int64Array::from(vec![1]))],
    )
    .expect("incompatible set");
    for operation in ["union_distinct", "intersect", "except"] {
        assert!(execute_binary(&left, &incompatible, &plan(operation, json!({}))).is_err());
    }
    let union_limit = Plan {
        schema_version: 1,
        limits: Limits {
            max_rows: 4,
            ..Limits::default()
        },
        steps: vec![Step {
            operation: "union_distinct".into(),
            config: json!({}),
        }],
    }
    .validate()
    .expect("union limit");
    assert!(execute_binary(&left, &left, &union_limit).is_err());
}

fn utf8_column<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .expect("colonna utf8")
}

#[test]
fn hash_null_policies_and_defaults_are_explicit() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(vec![Some(" A "), None]))],
    )
    .expect("hash null fixture");
    // Semantica storica: solo null_policy=error rifiuta;
    // Empty(default)/Literal sostituiscono. Oracolo: sostituire il null col
    // valore dichiarato in una colonna tutta valida deve dare lo STESSO
    // digest (la sostituzione e' la semantica dichiarata, non remediation).
    for (config, substitute) in [
        (json!({"columns":["value"]}), Some("")),
        (
            json!({"columns":["value"],"output_column":"hash","normalize":false,"null_policy":"empty"}),
            Some(""),
        ),
        (
            json!({"columns":["value"],"output_column":"hash","normalize":false,"null_policy":"literal","null_literal":" Missing "}),
            Some(" Missing "),
        ),
        (
            json!({"columns":["value"],"output_column":"hash","normalize":true,"null_policy":"literal","null_literal":" Missing "}),
            Some(" Missing "),
        ),
    ] {
        let output_column = config
            .get("output_column")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("sha256_hash")
            .to_owned();
        let output = execute_batch(input.clone(), &plan("sha256_hash", config.clone()))
            .expect("policy storica: sostituzione, non rifiuto");
        let substituted = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![Some(" A "), substitute]))],
        )
        .expect("substituted fixture");
        let oracle = execute_batch(substituted, &plan("sha256_hash", config))
            .expect("oracolo su colonna tutta valida");
        assert_eq!(
            utf8_column(&output, &output_column).value(1),
            utf8_column(&oracle, &output_column).value(1),
            "il null sostituito deve dare il digest storico del sostituto"
        );
    }
    assert!(execute_batch(
        input,
        &plan(
            "sha256_hash",
            json!({"columns":["value"],"output_column":"hash","null_policy":"error"})
        )
    )
    .is_err());
}

#[test]
fn aggregate_empty_and_rolling_stddev_cover_default_paths() {
    let input = flat();
    let counts = execute_batch(
        input.clone(),
        &plan("aggregate", json!({"group_by":["group"],"aggregations":[]})),
    )
    .expect("implicit count");
    assert_eq!(counts.num_rows(), 2);
    let duplicate_names = execute_batch(
        input.clone(),
        &plan(
            "aggregate",
            json!({"group_by":["group"],"aggregations":[
                {"column":"id","function":"sum"},
                {"column":"id","function":"mean"}
            ]}),
        ),
    )
    .expect("derived names");
    assert!(duplicate_names.column_by_name("id_sum").is_some());
    assert!(duplicate_names.column_by_name("id_mean").is_some());
    let rolling = execute_batch(
        input,
        &plan(
            "rolling_window",
            json!({"column":"id","function":"stddev","window":3,"min_periods":2,"ddof":0,"output_column":"rolling"}),
        ),
    )
    .expect("rolling default partition");
    assert!(!rolling
        .column_by_name("rolling")
        .expect("rolling")
        .is_null(2));
}

#[test]
fn asof_groups_nulls_and_type_errors_are_fail_closed() {
    let left = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("time", DataType::Float64, true),
            Field::new("group", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![Some(2.0), None, Some(f64::NAN)])),
            Arc::new(StringArray::from(vec![Some("a"), Some("a"), None])),
        ],
    )
    .expect("asof nullable left");
    let right = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("time", DataType::Float64, true),
            Field::new("group", DataType::Utf8, true),
            Field::new("label", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![Some(1.0), None, Some(f64::NAN)])),
            Arc::new(StringArray::from(vec![Some("a"), Some("a"), None])),
            Arc::new(StringArray::from(vec!["one", "null", "nan"])),
        ],
    )
    .expect("asof nullable right");
    let output = execute_binary(
        &left,
        &right,
        &plan(
            "asof_join",
            json!({"left_on":"time","right_on":"time","left_by":["group"],"right_by":["group"],"direction":"backward"}),
        ),
    )
    .expect("asof grouped");
    assert_eq!(output.num_rows(), 3);
    assert!(output.column_by_name("label").expect("label").is_null(1));

    let wrong_type = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("time", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec!["1"]))],
    )
    .expect("wrong type");
    assert!(execute_binary(
        &wrong_type,
        &wrong_type,
        &plan("asof_join", json!({"left_on":"time","right_on":"time"}))
    )
    .is_err());
    let right_bad_group = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("time", DataType::Float64, false),
            Field::new("group", DataType::Int64, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![1.0])),
            Arc::new(Int64Array::from(vec![1])),
        ],
    )
    .expect("bad group");
    assert!(execute_binary(
        &left,
        &right_bad_group,
        &plan(
            "asof_join",
            json!({"left_on":"time","right_on":"time","left_by":["group"],"right_by":["group"]})
        )
    )
    .is_err());
}

#[test]
fn extended_contract_guard_matrix_reaches_every_validation_family() {
    let oversized = "x".repeat(32);
    let limits = Limits {
        max_rows: 2,
        max_columns: 2,
        max_string_bytes: 8,
        max_regex_bytes: 4,
        max_split_columns: 2,
        ..Limits::default()
    };
    let cases = [
        (
            "aggregate",
            json!({"group_by":["g"],"aggregations":[{"column":"x","function":"sum","quantile":0.5}]}),
        ),
        (
            "aggregate",
            json!({"group_by":["g"],"aggregations":[{"column":"x","function":"sum","separator":oversized}]}),
        ),
        (
            "rolling_window",
            json!({"column":"x","function":"sum","window":3,"output_column":"out"}),
        ),
        (
            "rolling_window",
            json!({"column":"x","function":"sum","group_by":"","window":1,"output_column":"out"}),
        ),
        ("assert_schema", json!({"fields":[]})),
        (
            "assert_schema",
            json!({"fields":[{"name":"x","data_type":"decimal"}]}),
        ),
        (
            "assert_schema",
            json!({"fields":[{"name":"x","data_type":"int64"},{"name":"x","data_type":"int64"}]}),
        ),
        ("assert_not_null", json!({"columns":[]})),
        ("assert_unique", json!({"columns":["x","x"]})),
        ("assert_range", json!({"column":"x","min":2.0,"max":1.0})),
        ("assert_regex", json!({"column":"x","pattern":"abcde"})),
        (
            "date_format",
            json!({"column":"x","input_format":"%Y","output_format":"","output_column":"out"}),
        ),
        (
            "date_add",
            json!({"column":"x","input_format":"%Y","output_format":"%Q","amount":1,"unit":"days","output_column":"out"}),
        ),
        (
            "date_diff",
            json!({"start_column":"x","end_column":"y","input_format":"","unit":"days","output_column":"out"}),
        ),
        (
            "timezone_convert",
            json!({"column":"x","input_format":"%Y","source_timezone":"UTC","target_timezone":"Bad/Zone","output_column":"out"}),
        ),
        (
            "sha256_hash",
            json!({"columns":["x"],"null_literal":oversized}),
        ),
        ("explode", json!({"column":"x","output_column":""})),
        ("unnest", json!({"column":"x","prefix":oversized})),
        (
            "semi_join",
            json!({"left_keys":["x"],"right_keys":["x","y"]}),
        ),
        (
            "anti_join",
            json!({"left_keys":["x","x"],"right_keys":["x","y"]}),
        ),
        (
            "asof_join",
            json!({"left_on":"x","right_on":"y","left_by":["g"],"right_by":[]}),
        ),
    ];
    for (operation, config) in cases {
        assert!(
            Plan {
                schema_version: 1,
                limits: limits.clone(),
                steps: vec![Step {
                    operation: operation.into(),
                    config,
                }],
            }
            .validate()
            .is_err(),
            "{operation}"
        );
    }
}

#[test]
fn sort_distinct_and_dedup_cover_null_and_failure_branches() {
    let lists = ListArray::from_iter_primitive::<Int64Type, _, _>(vec![
        Some(vec![Some(1)]),
        Some(vec![Some(2)]),
        None,
        Some(vec![Some(1)]),
    ]);
    let list_type = lists.data_type().clone();
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("text", DataType::Utf8, true),
            Field::new("number", DataType::Float64, true),
            Field::new("items", list_type, true),
        ])),
        vec![
            Arc::new(StringArray::from(vec![Some("b"), None, None, Some("a")])),
            Arc::new(Float64Array::from(vec![None, None, Some(2.0), Some(1.0)])),
            Arc::new(lists),
        ],
    )
    .expect("aggregation edge fixture");
    for config in [
        json!({"columns":["text"],"ascending":true}),
        json!({"columns":["number"],"ascending":false}),
    ] {
        assert!(execute_batch(input.clone(), &plan("sort", config)).is_ok());
    }
    assert!(execute_batch(input.clone(), &plan("sort", json!({"columns":["items"]}))).is_err());
    for keep in ["first", "last", "false"] {
        assert!(execute_batch(
            input.clone(),
            &plan("distinct", json!({"subset":[],"keep":keep}))
        )
        .is_err());
    }

    let scalar = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, true),
            Field::new("priority", DataType::Int64, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec![Some("a"), Some("a"), None, None])),
            Arc::new(Int64Array::from(vec![2, 1, 4, 3])),
        ],
    )
    .expect("scalar dedup fixture");
    for keep in ["first", "last", "false"] {
        assert!(execute_batch(
            scalar.clone(),
            &plan("distinct", json!({"subset":["key"],"keep":keep}))
        )
        .is_ok());
    }
    assert!(execute_batch(
        scalar.clone(),
        &plan(
            "dedup_advanced",
            json!({"subset":["key"],"keep":"last","order_column":null,"ascending":true})
        )
    )
    .is_ok());
    assert!(execute_batch(
        scalar,
        &plan(
            "dedup_advanced",
            json!({"subset":["key"],"keep":"false","order_column":"priority","ascending":true})
        )
    )
    .is_err());
}

#[test]
fn schema_assertion_recognizes_list_and_struct_logical_types() {
    let lists = ListArray::from_iter_primitive::<Int64Type, _, _>(vec![Some(vec![Some(1)])]);
    let structure = StructArray::from(vec![(
        Arc::new(Field::new("x", DataType::Int64, false)),
        Arc::new(Int64Array::from(vec![1])) as ArrayRef,
    )]);
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("items", lists.data_type().clone(), false),
            Field::new("payload", structure.data_type().clone(), false),
        ])),
        vec![Arc::new(lists), Arc::new(structure)],
    )
    .expect("nested schema fixture");
    assert!(execute_batch(
        input,
        &plan(
            "assert_schema",
            json!({"fields":[
                {"name":"items","data_type":"list","nullable":false},
                {"name":"payload","data_type":"struct","nullable":false}
            ]})
        )
    )
    .is_ok());
}

#[test]
fn runtime_missing_columns_and_binary_type_mismatches_are_contextual() {
    let input = flat();
    for (operation, config) in [
        (
            "assert_schema",
            json!({"fields":[{"name":"missing","data_type":"int64"}],"allow_extra":true,"ordered":false}),
        ),
        (
            "assert_schema",
            json!({"fields":[
            {"name":"id","data_type":"int64"},
            {"name":"group","data_type":"utf8"},
            {"name":"value","data_type":"float64"},
            {"name":"fallback","data_type":"float64"},
            {"name":"start","data_type":"utf8"},
            {"name":"end","data_type":"utf8"},
            {"name":"missing","data_type":"utf8"}
        ],"allow_extra":true,"ordered":true}),
        ),
        ("assert_not_null", json!({"columns":["missing"]})),
        ("assert_unique", json!({"columns":["missing"]})),
        ("assert_range", json!({"column":"missing","min":0.0})),
        ("assert_regex", json!({"column":"missing","pattern":".*"})),
        (
            "coalesce",
            json!({"columns":["value","missing"],"output_column":"out"}),
        ),
    ] {
        assert!(execute_batch(input.clone(), &plan(operation, config)).is_err());
    }

    let left = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("key", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1]))],
    )
    .expect("integer key");
    let right = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("key", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec!["1"]))],
    )
    .expect("string key");
    for operation in ["semi_join", "anti_join"] {
        assert!(execute_binary(
            &left,
            &right,
            &plan(operation, json!({"left_keys":["key"],"right_keys":["key"]}))
        )
        .is_err());
    }
    assert!(execute_binary(
        &left,
        &left,
        &plan("asof_join", json!({"left_on":"missing","right_on":"key"}))
    )
    .is_err());
}

#[test]
fn explode_drop_and_unnest_column_limit_are_enforced() {
    let lists = ListArray::from_iter_primitive::<Int64Type, _, _>(vec![
        Some(Vec::<Option<i64>>::new()),
        None,
    ]);
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "items",
            lists.data_type().clone(),
            true,
        )])),
        vec![Arc::new(lists)],
    )
    .expect("empty lists");
    let error = execute_batch(
        input,
        &plan("explode", json!({"column":"items","empty_policy":"drop"})),
    )
    .expect_err("drop implicito di righe vuote accettato");
    assert!(error.to_string().contains("remediation implicita"));

    let structure = StructArray::from(vec![
        (
            Arc::new(Field::new("x", DataType::Int64, false)),
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
        ),
        (
            Arc::new(Field::new("y", DataType::Int64, false)),
            Arc::new(Int64Array::from(vec![2])) as ArrayRef,
        ),
    ]);
    let nested = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "payload",
            structure.data_type().clone(),
            false,
        )])),
        vec![Arc::new(structure)],
    )
    .expect("nested");
    let limited = Plan {
        schema_version: 1,
        limits: Limits {
            max_columns: 1,
            ..Limits::default()
        },
        steps: vec![Step {
            operation: "unnest".into(),
            config: json!({"column":"payload","drop_source":true}),
        }],
    }
    .validate()
    .expect("limited unnest");
    assert!(execute_batch(nested, &limited).is_err());
}

#[test]
fn remaining_kernel_lookup_guards_reject_missing_sources() {
    let input = flat();
    for (operation, config) in [
        ("sort", json!({"columns":["missing"]})),
        (
            "aggregate",
            json!({"group_by":["group"],"aggregations":[{"column":"missing","function":"sum"}]}),
        ),
        (
            "rolling_window",
            json!({"column":"missing","function":"sum","window":1,"output_column":"out"}),
        ),
        (
            "window_function",
            json!({"column":"missing","function":"rank","group_by":null,"order_column":null,"output_column":"out"}),
        ),
        ("explode", json!({"column":"missing"})),
        ("unnest", json!({"column":"missing"})),
    ] {
        assert!(execute_batch(input.clone(), &plan(operation, config)).is_err());
    }
    assert!(execute_binary(
        &input,
        &input,
        &plan(
            "semi_join",
            json!({"left_keys":["missing"],"right_keys":["id"]})
        )
    )
    .is_err());
}
