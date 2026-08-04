use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::table_engine::SCHEMA_VERSION;
use plenora_engine::{execute_binary, execute_complete_batch as execute_batch, Limits, Plan, Step};
use serde_json::{json, Value};

fn fixture() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("group", DataType::Utf8, false),
            Field::new("text", DataType::Utf8, true),
            Field::new("num", DataType::Float64, true),
            Field::new("date", DataType::Utf8, true),
            Field::new("json", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 2])),
            Arc::new(StringArray::from(vec!["a", "a", "b"])),
            Arc::new(StringArray::from(vec![Some(" A-10 "), Some("B-20"), None])),
            Arc::new(Float64Array::from(vec![Some(1.0), Some(2.0), Some(3.0)])),
            Arc::new(StringArray::from(vec![
                Some("2026-01-02"),
                Some("03/02/2026"),
                None,
            ])),
            Arc::new(StringArray::from(vec![
                Some(r#"{"x":"one"}"#),
                Some(r#"{"x":"two"}"#),
                None,
            ])),
        ],
    )
    .expect("fixture")
}

fn plan(operation: &str, config: Value) -> plenora_engine::ValidatedPlan {
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

#[test]
#[allow(clippy::too_many_lines)] // The matrix is intentionally explicit: one audited case per unary catalog entry.
fn every_unary_operation_executes_its_safe_profile() {
    let cases = vec![
        (
            "add_row_number",
            json!({"output_column":"row","start":1,"partition_column":null,"order_column":null,"ascending":true}),
        ),
        (
            "aggregate",
            json!({"group_by":["group"],"aggregations":[{"column":"num","function":"sum","alias":"total"}]}),
        ),
        (
            "bin",
            json!({"column":"num","bins":2,"labels":["low","high"],"output_column":"band"}),
        ),
        (
            "concat_columns",
            json!({"columns":["group","text"],"output_column":"joined","separator":"|","skip_null":true}),
        ),
        (
            "conditional",
            json!({"column":"num","conditions":[{"operator":">","value":1,"result":"yes"}],"default_value":"no","output_column":"flag"}),
        ),
        (
            "date_extract",
            json!({"column":"date","parts":["year","month","weekday"],"prefix":"d_"}),
        ),
        (
            "dedup_advanced",
            json!({"subset":["id"],"keep":"first","order_column":"num","ascending":false}),
        ),
        ("distinct", json!({"subset":["id"],"keep":"first"})),
        ("drop_columns", json!({"columns":["json"]})),
        (
            "fill_na",
            json!({"column":"text","method":"value","value":"missing"}),
        ),
        ("filter", json!({"column":"num","operator":">=","value":2})),
        (
            "flatten_json",
            json!({"column":"json","prefix":"json_","max_level":1,"output_columns":["json_x"]}),
        ),
        (
            "formula",
            json!({"new_column":"calculated","formula":"num * 2 + 1"}),
        ),
        (
            "lookup",
            json!({"column":"group","mapping":{"a":"alpha"},"default":null,"output_column":"mapped"}),
        ),
        (
            "melt",
            json!({"id_columns":["id"],"value_columns":["text","num"],"var_name":"variable","value_name":"value","type_policy":"string"}),
        ),
        (
            "pivot",
            json!({"index_col":"group","pivot_col":"id","value_col":"num","aggr_func":"sum","mapping":{"1":"one","2":"two"}}),
        ),
        (
            "rename",
            json!({"renames":[{"old_name":"text","new_name":"label"}]}),
        ),
        (
            "reorder_columns",
            json!({"columns":["text","id"],"sort_alphabetical":false}),
        ),
        (
            "replace",
            json!({"column":"text","old_value":"-","new_value":"/","regex":false}),
        ),
        (
            "sample",
            json!({"n":2,"fraction":null,"random_state":42,"stratify_column":null}),
        ),
        ("sort", json!({"columns":["num"],"ascending":false})),
        (
            "split_column",
            json!({"column":"text","delimiter":"-","new_columns":["left","right"],"max_splits":1}),
        ),
        (
            "statistics",
            json!({"column":"num","group_by":"group","stats":["count","mean","q25"],"output_prefix":"stat_"}),
        ),
        (
            "string_extract",
            json!({"column":"text","pattern":"([A-Z])","output_column":"letter","extract_all":false}),
        ),
        (
            "string_length",
            json!({"column":"text","output_column":"length"}),
        ),
        (
            "string_pad",
            json!({"column":"group","width":3,"side":"left","fill_char":"0","output_column":"padded"}),
        ),
        (
            "text_normalize",
            json!({"columns":["text"],"operations":"full","overwrite":true}),
        ),
        (
            "transpose",
            json!({"id_column":null,"output_columns":["r1","r2","r3"],"type_policy":"string"}),
        ),
        (
            "type_cast",
            json!({"column":"num","target_type":"str","date_format":"","errors":"coerce"}),
        ),
        ("uuid_generator", json!({"output_column":"uuid"})),
        (
            "window_function",
            json!({"column":"num","function":"cumsum","group_by":"group","order_column":"num","offset":1,"output_column":"running"}),
        ),
        (
            "mask_data",
            json!({"maskings":[{"column":"text","mask_type":"custom","chars_start":1,"chars_end":1,"mask_char":"*"}],"overwrite":false}),
        ),
        (
            "md5_hash",
            json!({"columns":["id","group"],"output_column":"hash","normalize":true}),
        ),
        // Estensioni table v1.1: raggiungibili via id canonico (come le
        // estensioni geo v1.1, nessun alias legacy aggiunto al catalogo).
        ("table.select_columns", json!({"columns":["id","num"]})),
        ("table.limit", json!({"n":2,"offset":1})),
        (
            "table.top_n",
            json!({"columns":["num"],"n":2,"descending":true}),
        ),
        (
            "table.stable_fingerprint",
            json!({"columns":["id","group"],"algorithm":"sha256"}),
        ),
        // Estensioni table v1.2: anche qui id canonici, nessun alias legacy.
        (
            "table.align_schema",
            json!({"columns":[{"name":"id","type":"Int64"},{"name":"note","type":"Utf8","default":"n/d"}]}),
        ),
        (
            "table.hmac_sha256",
            json!({"columns":["id","group"],"key_env":"PLENORA_FULL_CATALOG_HMAC_KEY"}),
        ),
        (
            "table.validate_rules",
            json!({"rules":[{"name":"id_pos","operator":"gt","column":"id","value":0}]}),
        ),
    ];
    let input = fixture();
    // La chiave HMAC arriva solo dall'ambiente (mai dal piano).
    std::env::set_var("PLENORA_FULL_CATALOG_HMAC_KEY", "full-catalog-key");
    for (operation, config) in cases {
        let output = execute_batch(input.clone(), &plan(operation, config));
        assert!(output.is_ok(), "{operation}: {:?}", output.err());
    }
}

#[test]
fn every_binary_operation_executes_its_safe_profile() {
    let input = fixture();
    let cases = [
        ("concat", json!({"ignore_index":true})),
        ("table.concat_by_name", json!({})),
        ("cross_join", json!({})),
        (
            "join",
            json!({"left_keys":["id"],"right_keys":["id"],"how":"inner"}),
        ),
        (
            "table_diff",
            json!({"left_keys":["id","group"],"right_keys":["id","group"],"compare_columns":["text","num"],"include_unchanged":"yes","separator":"#"}),
        ),
        // Estensione table v1.3: id canonico, nessun alias legacy.
        (
            "table.fuzzy_join",
            json!({"left_key":"text","right_key":"text","metric":"jaro_winkler","threshold":0.8,"blocking":"prefix"}),
        ),
    ];
    for (operation, config) in cases {
        let output = execute_binary(&input, &input, &plan(operation, config));
        assert!(output.is_ok(), "{operation}: {:?}", output.err());
    }
}
