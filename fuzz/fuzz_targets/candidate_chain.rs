#![no_main]

use std::sync::Arc;

use arrow_array::{BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_engine::{execute_batch, Limits, Plan, Step};
use serde_json::{json, Value};

fn fixture(payload: &[u8]) -> RecordBatch {
    let rows = payload.chunks(4).take(128).enumerate().collect::<Vec<_>>();
    let texts = rows
        .iter()
        .map(|(index, bytes)| (index % 7 != 0).then(|| String::from_utf8_lossy(bytes).into_owned()))
        .collect::<Vec<_>>();
    let groups = rows
        .iter()
        .map(|(index, _)| Some(if index % 2 == 0 { "a" } else { "b" }))
        .collect::<Vec<_>>();
    let integers = rows
        .iter()
        .map(|(index, _)| (index % 5 != 0).then_some(i64::try_from(*index).unwrap_or(i64::MAX)))
        .collect::<Vec<_>>();
    let numbers = integers
        .iter()
        .map(|value| value.map(|value| num_traits::ToPrimitive::to_f64(&value).unwrap_or_default()))
        .collect::<Vec<_>>();
    let flags = rows
        .iter()
        .map(|(index, _)| (index % 3 != 0).then_some(index % 2 == 0))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("num", DataType::Float64, true),
            Field::new("flag", DataType::Boolean, true),
            Field::new("group", DataType::Utf8, true),
            Field::new("text", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(integers)),
            Arc::new(Float64Array::from(numbers)),
            Arc::new(BooleanArray::from(flags)),
            Arc::new(StringArray::from(groups)),
            Arc::new(StringArray::from(texts)),
        ],
    )
    .expect("bounded fixture")
}

fn case(selector: usize) -> (&'static str, Value) {
    let cases = [
        (
            "fill_na",
            json!({"column":"text","method":"ffill","value":null}),
        ),
        (
            "replace",
            json!({"column":"text","old_value":".","new_value":"x","regex":true}),
        ),
        (
            "type_cast",
            json!({"column":"text","target_type":"int","errors":"coerce"}),
        ),
        ("filter", json!({"column":"num","operator":">=","value":3})),
        (
            "conditional",
            json!({"column":"num","conditions":[{"operator":">","value":3,"result":"high"}],"default_value":"low","output_column":"class"}),
        ),
        (
            "string_extract",
            json!({"column":"text","pattern":"(.)","output_column":"first","extract_all":false}),
        ),
        (
            "lookup",
            json!({"column":"group","mapping":{"a":"A"},"default":"B","output_column":"mapped"}),
        ),
        (
            "mask_data",
            json!({"maskings":[{"column":"text","mask_type":"custom","chars_start":1,"chars_end":1,"mask_char":"*"}],"overwrite":false}),
        ),
        (
            "md5_hash",
            json!({"columns":["group","text"],"output_column":"hash","normalize":true}),
        ),
        (
            "add_row_number",
            json!({"output_column":"row","start":1,"partition_column":"group","order_column":null,"ascending":true}),
        ),
        (
            "bin",
            json!({"column":"num","bins":4,"labels":null,"output_column":"band"}),
        ),
        (
            "sample",
            json!({"n":16,"fraction":null,"random_state":1,"stratify_column":"group"}),
        ),
        (
            "statistics",
            json!({"column":"num","group_by":"group","stats":["count","mean","std"],"output_prefix":"s_"}),
        ),
        ("sort", json!({"columns":["group","num"],"ascending":true})),
        ("distinct", json!({"subset":["text"],"keep":"first"})),
        (
            "dedup_advanced",
            json!({"subset":["text"],"keep":"last","order_column":"num","ascending":true}),
        ),
        (
            "aggregate",
            json!({"group_by":["group"],"aggregations":[{"column":"num","function":"sum","alias":"total"}]}),
        ),
        (
            "window_function",
            json!({"column":"num","function":"rank","group_by":"group","order_column":null,"offset":1,"output_column":"rank"}),
        ),
        (
            "formula",
            json!({"new_column":"calc","formula":"num * 2 + 1"}),
        ),
    ];
    cases[selector % cases.len()].clone()
}

fuzz_target!(|payload: &[u8]| {
    let (operation, config) = case(payload.first().copied().unwrap_or_default().into());
    let plan = Plan {
        schema_version: 1,
        limits: Limits {
            max_rows: 10_000,
            max_columns: 128,
            max_string_bytes: 1_048_576,
            max_regex_bytes: 256,
            max_split_columns: 32,
            ..Limits::default()
        },
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .expect("static plan");
    if let Ok(output) = execute_batch(fixture(payload), &plan) {
        assert!(output.num_rows() <= plan.limits().max_rows);
        assert!(output.num_columns() <= plan.limits().max_columns);
        assert!(output
            .columns()
            .iter()
            .all(|column| column.len() == output.num_rows()));
    }
});
