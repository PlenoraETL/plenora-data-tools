use std::sync::Arc;

use plenora_core::arrow::array::{Array, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::table_engine::SCHEMA_VERSION;
use plenora_engine::{execute_batch, Limits, Plan, Step, ValidatedPlan};
use serde_json::{json, Value};

fn fixture() -> RecordBatch {
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
            Arc::new(Int64Array::from(vec![
                Some(1),
                Some(2),
                Some(2),
                Some(4),
                None,
            ])),
            Arc::new(Float64Array::from(vec![
                Some(3.0),
                Some(1.0),
                Some(1.0),
                None,
                Some(5.0),
            ])),
            Arc::new(BooleanArray::from(vec![
                Some(true),
                None,
                Some(false),
                None,
                Some(true),
            ])),
            Arc::new(StringArray::from(vec![
                Some("b"),
                Some("a"),
                Some("a"),
                Some("b"),
                None,
            ])),
            Arc::new(StringArray::from(vec![
                Some(" Alpha "),
                Some("2"),
                Some("bad"),
                None,
                Some("false"),
            ])),
            Arc::new(StringArray::from(vec![
                Some("2026-01-02 03:04:05"),
                Some("03/02/2026"),
                Some("bad"),
                None,
                Some("2024-12-31"),
            ])),
            Arc::new(StringArray::from(vec![
                Some(r#"{"a":1,"nested":{"b":"x"}}"#),
                Some(r#"{"a":2}"#),
                Some("bad"),
                None,
                Some(r#"{"nested":{"b":"z"}}"#),
            ])),
        ],
    )
    .expect("valid fixture")
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
    execute_batch(fixture(), &plan(operation, config))
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

fn i64s<'a>(batch: &'a RecordBatch, name: &str) -> &'a Int64Array {
    batch
        .column_by_name(name)
        .expect("column")
        .as_any()
        .downcast_ref()
        .expect("Int64")
}

fn f64s<'a>(batch: &'a RecordBatch, name: &str) -> &'a Float64Array {
    batch
        .column_by_name(name)
        .expect("column")
        .as_any()
        .downcast_ref()
        .expect("Float64")
}

#[test]
fn cleansing_profiles_preserve_types_and_fail_closed() {
    let forward = run(
        "fill_na",
        json!({"column":"num","method":"ffill","value":null}),
    );
    assert_eq!(f64s(&forward, "num").value(3).to_bits(), 1.0_f64.to_bits());
    let backward = run(
        "fill_na",
        json!({"column":"flag","method":"bfill","value":null}),
    );
    assert!(!backward
        .column_by_name("flag")
        .expect("flag")
        .as_any()
        .downcast_ref::<BooleanArray>()
        .expect("bool")
        .value(1));
    let fixed = run(
        "fill_na",
        json!({"column":"id","method":"value","value":"7"}),
    );
    assert_eq!(i64s(&fixed, "id").value(4), 7);
    let replaced = run(
        "replace",
        json!({"column":"text","old_value":"\\s+","new_value":"_","regex":true}),
    );
    assert_eq!(utf8(&replaced, "text").value(0), "_Alpha_");

    let int_cast = run(
        "type_cast",
        json!({"column":"text","target_type":"int","errors":"coerce"}),
    );
    assert_eq!(i64s(&int_cast, "text").value(1), 2);
    assert!(i64s(&int_cast, "text").is_null(2));
    let float_cast = run(
        "type_cast",
        json!({"column":"text","target_type":"float","errors":"coerce"}),
    );
    assert_eq!(
        f64s(&float_cast, "text").value(1).to_bits(),
        2.0_f64.to_bits()
    );
    let bool_cast = run(
        "type_cast",
        json!({"column":"text","target_type":"bool","errors":"coerce"}),
    );
    let bools = bool_cast
        .column_by_name("text")
        .expect("text")
        .as_any()
        .downcast_ref::<BooleanArray>()
        .expect("bool");
    assert!(!bools.value(4));
    assert!(bools.is_null(2));
    let date = run(
        "type_cast",
        json!({"column":"date","target_type":"date","errors":"coerce"}),
    );
    assert_eq!(utf8(&date, "date").value(1), "2026-02-03");
    let datetime = run(
        "type_cast",
        json!({"column":"date","target_type":"datetime","errors":"coerce"}),
    );
    assert_eq!(utf8(&datetime, "date").value(0), "2026-01-02T03:04:05");

    assert!(execute_batch(
        fixture(),
        &plan(
            "type_cast",
            json!({"column":"text","target_type":"int","errors":"raise"})
        )
    )
    .is_err());
    assert!(execute_batch(
        fixture(),
        &plan(
            "type_cast",
            json!({"column":"text","target_type":"bool","errors":"ignore"})
        )
    )
    .is_err());
    assert!(execute_batch(
        fixture(),
        &plan(
            "fill_na",
            json!({"column":"id","method":"value","value":true})
        )
    )
    .is_err());
}

#[test]
fn filters_conditionals_lookup_and_bins_cover_operator_boundaries() {
    let cases = [
        (json!({"column":"num","operator":"==","value":1}), 2),
        (json!({"column":"num","operator":"!=","value":1}), 2),
        (json!({"column":"num","operator":">","value":1}), 2),
        (json!({"column":"num","operator":"<=","value":1}), 2),
        (
            json!({"column":"text","operator":"contains","value":"alp"}),
            1,
        ),
        (
            json!({"column":"text","operator":"startswith","value":" Alpha"}),
            1,
        ),
        (
            json!({"column":"text","operator":"endswith","value":"bad"}),
            1,
        ),
        (json!({"column":"text","operator":"isnull"}), 1),
        (json!({"column":"text","operator":"notnull"}), 4),
        (
            json!({"column":"num","operator":"between","value":"1,3"}),
            3,
        ),
        (json!({"column":"text","operator":"==","value":2}), 1),
    ];
    for (config, expected) in cases {
        assert_eq!(run("filter", config).num_rows(), expected);
    }
    let conditional = run(
        "conditional",
        json!({"column":"num","conditions":[{"operator":">=","value":3,"result":10},{"operator":"<","value":3,"result":5}],"default_value":null,"output_column":"score"}),
    );
    assert_eq!(
        f64s(&conditional, "score").value(0).to_bits(),
        10.0_f64.to_bits()
    );
    assert!(f64s(&conditional, "score").is_null(3));
    let lookup = run(
        "lookup",
        json!({"column":"group","mapping":{"a":"A"},"default":"other","output_column":"mapped"}),
    );
    assert_eq!(utf8(&lookup, "mapped").value(1), "A");
    assert_eq!(utf8(&lookup, "mapped").value(0), "other");
    let bins = run(
        "bin",
        json!({"column":"num","bins":[0,2,4,6],"labels":["low","mid","high"],"output_column":"band"}),
    );
    assert_eq!(utf8(&bins, "band").value(4), "high");
    let boundaries = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("n", DataType::Float64, false)])),
        vec![Arc::new(Float64Array::from(vec![1.0, 2.0, 3.0, 4.0]))],
    )
    .expect("bin boundaries");
    let auto_bins = execute_batch(
        boundaries,
        &plan(
            "bin",
            json!({"column":"n","bins":3,"labels":["low","mid","high"],"output_column":"band"}),
        ),
    )
    .expect("automatic bins");
    assert_eq!(
        utf8(&auto_bins, "band").iter().collect::<Vec<_>>(),
        vec![Some("low"), Some("low"), Some("mid"), Some("high")]
    );
    assert!(execute_batch(
        fixture(),
        &plan(
            "filter",
            json!({"column":"num","operator":"==","value":"x"})
        )
    )
    .is_err());
    assert!(execute_batch(
        fixture(),
        &plan("bin", json!({"column":"num","bins":[0,2,1]}))
    )
    .is_err());
}

#[test]
fn analysis_profiles_are_deterministic_and_bounded() {
    let flat = run(
        "flatten_json",
        json!({"column":"json","prefix":"j_","max_level":2,"output_columns":[]}),
    );
    assert_eq!(utf8(&flat, "j_nested.b").value(0), "x");
    assert!(utf8(&flat, "j_a").is_null(2));
    let stats = run(
        "statistics",
        json!({"column":"num","group_by":"group","stats":["count","min","max","sum","mean","median","std","var","q25","q75"],"output_prefix":"s_"}),
    );
    assert_eq!(
        f64s(&stats, "s_count").value(1).to_bits(),
        2.0_f64.to_bits()
    );
    assert_eq!(
        f64s(&stats, "s_median").value(0).to_bits(),
        3.0_f64.to_bits()
    );
    let first = run(
        "sample",
        json!({"n":3,"fraction":null,"random_state":99,"stratify_column":null}),
    );
    let second = run(
        "sample",
        json!({"n":3,"fraction":null,"random_state":99,"stratify_column":null}),
    );
    assert_eq!(i64s(&first, "id"), i64s(&second, "id"));
    let fraction = run(
        "sample",
        json!({"n":99,"fraction":0.4,"random_state":1,"stratify_column":null}),
    );
    assert_eq!(fraction.num_rows(), 2);
    let stratified = run(
        "sample",
        json!({"n":4,"fraction":null,"random_state":1,"stratify_column":"group"}),
    );
    assert!(stratified.num_rows() <= 4);
    assert!(execute_batch(
        fixture(),
        &plan(
            "flatten_json",
            json!({"column":"json","prefix":"j_","max_level":6,"output_columns":[]})
        )
    )
    .is_err());
    assert!(execute_batch(
        fixture(),
        &plan(
            "sample",
            json!({"n":1,"fraction":1.1,"random_state":1,"stratify_column":null})
        )
    )
    .is_err());
}

#[test]
#[allow(clippy::too_many_lines)] // One compact semantic matrix covers every aggregation and window variant.
fn aggregation_window_and_formula_variants_have_exact_semantics() {
    let sorted = run("sort", json!({"columns":["group","num"],"ascending":false}));
    assert!(i64s(&sorted, "id").is_null(0));
    assert_eq!(
        run("distinct", json!({"subset":["id"],"keep":"first"})).num_rows(),
        4
    );
    assert_eq!(
        run("distinct", json!({"subset":["id"],"keep":"last"})).num_rows(),
        4
    );
    assert_eq!(
        run("distinct", json!({"subset":["id"],"keep":"false"})).num_rows(),
        3
    );
    let dedup = run(
        "dedup_advanced",
        json!({"subset":["id"],"keep":"first","order_column":"num","ascending":false}),
    );
    assert_eq!(dedup.num_rows(), 4);
    let aggregate = run(
        "aggregate",
        json!({"group_by":["group"],"aggregations":[
            {"column":"num","function":"count","alias":"n"},
            {"column":"num","function":"sum","alias":"sum"},
            {"column":"num","function":"avg","alias":"avg"},
            {"column":"num","function":"mean","alias":"mean"},
            {"column":"num","function":"min","alias":"min"},
            {"column":"num","function":"max","alias":"max"},
            {"column":"text","function":"first","alias":"first"},
            {"column":"text","function":"last","alias":"last"},
            {"column":"text","function":"concat","separator":"|","distinct":true,"skip_null":false,"alias":"joined"}
        ]}),
    );
    assert_eq!(aggregate.num_rows(), 3);
    assert_eq!(
        run("aggregate", json!({"group_by":["group"],"aggregations":[]})).num_columns(),
        2
    );
    for function in [
        "rank",
        "dense_rank",
        "cumsum",
        "cumcount",
        "lag",
        "lead",
        "pct_change",
        "running_mean",
    ] {
        let output = run(
            "window_function",
            json!({"column":"num","function":function,"group_by":"group","order_column":null,"offset":1,"output_column":"w"}),
        );
        assert_eq!(output.num_rows(), 5, "{function}");
    }
    let rank = run(
        "window_function",
        json!({"column":"num","function":"rank","group_by":"group","order_column":null,"offset":1,"output_column":"w"}),
    );
    assert_eq!(f64s(&rank, "w").value(1).to_bits(), 1.5_f64.to_bits());
    let numeric = run(
        "formula",
        json!({"new_column":"calc","formula":"-(num + 2) * 1e1 / 2"}),
    );
    assert_eq!(
        f64s(&numeric, "calc").value(0).to_bits(),
        (-25.0_f64).to_bits()
    );
    let text = run(
        "formula",
        json!({"new_column":"label","formula":"group + '-' + text"}),
    );
    assert_eq!(utf8(&text, "label").value(1), "a-2");
    assert!(execute_batch(
        fixture(),
        &plan("formula", json!({"new_column":"x","formula":"num / 0"}))
    )
    .is_err());
    assert!(Plan {
        schema_version: SCHEMA_VERSION,
        limits: Limits::default(),
        steps: vec![Step {
            operation: "formula".into(),
            config: json!({"new_column":"x","formula":"(num"})
        }]
    }
    .validate()
    .is_err());
}

#[test]
fn utility_and_security_profiles_cover_unicode_nulls_and_identifiers() {
    let numbered = run(
        "add_row_number",
        json!({"output_column":"row","start":10,"partition_column":"group","order_column":null,"ascending":true}),
    );
    assert_eq!(i64s(&numbered, "row").values(), &[10, 10, 11, 11, 10]);
    let dates = run(
        "date_extract",
        json!({"column":"date","parts":["year","month","day","quarter","weekday","week","hour","minute","second"],"prefix":"d_"}),
    );
    assert_eq!(i64s(&dates, "d_year").value(0), 2026);
    assert_eq!(i64s(&dates, "d_hour").value(0), 3);
    assert!(i64s(&dates, "d_year").is_null(2));
    let uuids = run("uuid_generator", json!({"output_column":"uuid"}));
    assert_eq!(utf8(&uuids, "uuid").value(0).len(), 36);
    assert_ne!(utf8(&uuids, "uuid").value(0), utf8(&uuids, "uuid").value(1));
    let hash_a = run(
        "md5_hash",
        json!({"columns":["text","group"],"output_column":"hash","normalize":true}),
    );
    let hash_b = run(
        "md5_hash",
        json!({"columns":["group","text"],"output_column":"hash","normalize":true}),
    );
    assert_eq!(utf8(&hash_a, "hash"), utf8(&hash_b, "hash"));
    for mask_type in ["cf", "email", "phone", "iban", "custom"] {
        let output = run(
            "mask_data",
            json!({"maskings":[{"column":"text","mask_type":mask_type,"chars_start":1,"chars_end":1,"mask_char":"•"}],"overwrite":false}),
        );
        assert_eq!(output.num_rows(), 5, "{mask_type}");
    }
    assert!(execute_batch(fixture(), &plan("mask_data", json!({"maskings":[{"column":"text","mask_type":"custom","chars_start":1,"chars_end":1,"mask_char":"xx"}],"overwrite":false}))).is_err());
}

#[test]
fn date_and_hash_ambiguity_policies_are_explicit_and_fail_closed() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(vec![
            Some("03/02/2026"),
            Some("2026-02-03"),
            Some("bad"),
            None,
        ]))],
    )
    .expect("policy fixture");
    let exact = execute_batch(
        input.clone(),
        &plan(
            "date_extract",
            json!({"column":"value","parts":["year","month","day"],"prefix":"x_","date_format":"%d/%m/%Y","invalid":"null"}),
        ),
    )
    .expect("exact date format");
    assert_eq!(i64s(&exact, "x_day").value(0), 3);
    assert_eq!(i64s(&exact, "x_month").value(0), 2);
    assert!(i64s(&exact, "x_day").is_null(1));
    assert!(execute_batch(
        input.clone(),
        &plan(
            "date_extract",
            json!({"column":"value","date_format":"%d/%m/%Y","invalid":"error"}),
        ),
    )
    .is_err());
    assert!(Plan {
        schema_version: SCHEMA_VERSION,
        limits: Limits::default(),
        steps: vec![Step {
            operation: "date_extract".into(),
            config: json!({"column":"value","date_format":""}),
        }],
    }
    .validate()
    .is_err());

    let empty = execute_batch(
        input.clone(),
        &plan(
            "md5_hash",
            json!({"columns":["value"],"output_column":"hash","normalize":false,"null_policy":"empty"}),
        ),
    )
    .expect("empty null policy");
    let literal = execute_batch(
        input.clone(),
        &plan(
            "md5_hash",
            json!({"columns":["value"],"output_column":"hash","normalize":false,"null_policy":"literal","null_literal":"<na>"}),
        ),
    )
    .expect("literal null policy");
    assert_ne!(
        utf8(&empty, "hash").value(3),
        utf8(&literal, "hash").value(3)
    );
    assert!(execute_batch(
        input,
        &plan(
            "md5_hash",
            json!({"columns":["value"],"null_policy":"error"}),
        ),
    )
    .is_err());
    let short_strings = Limits {
        max_string_bytes: 1,
        ..Limits::default()
    };
    assert!(Plan {
        schema_version: SCHEMA_VERSION,
        limits: short_strings,
        steps: vec![Step {
            operation: "md5_hash".into(),
            config: json!({"columns":["value"],"null_literal":"xx"}),
        }],
    }
    .validate()
    .is_err());
}

#[test]
fn dedup_advanced_respects_descending_priority() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("priority", DataType::Int64, false),
            Field::new("label", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 1, 2])),
            Arc::new(Int64Array::from(vec![1, 9, 5])),
            Arc::new(StringArray::from(vec!["low", "high", "single"])),
        ],
    )
    .expect("dedup fixture");
    let output = execute_batch(
        input,
        &plan(
            "dedup_advanced",
            json!({"subset":["id"],"keep":"first","order_column":"priority","ascending":false}),
        ),
    )
    .expect("descending dedup");
    assert_eq!(utf8(&output, "label").value(0), "high");
    assert!(Plan {
        schema_version: SCHEMA_VERSION,
        limits: Limits::default(),
        steps: vec![Step {
            operation: "dedup_advanced".into(),
            config: json!({"subset":["id"],"keep":"first","order_column":null,"ascending":false}),
        }],
    }
    .validate()
    .is_err());
}
