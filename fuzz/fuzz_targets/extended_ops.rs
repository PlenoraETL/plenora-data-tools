#![no_main]

use std::sync::Arc;

use arrow_array::types::Int64Type;
use arrow_array::{
    Array, ArrayRef, Float64Array, Int64Array, ListArray, RecordBatch, StringArray, StructArray,
};
use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_engine::{execute_batch, execute_binary, Limits, Plan, Step, ValidatedPlan};
use serde_json::{json, Value};

fn plan(operation: &str, config: Value) -> ValidatedPlan {
    Plan {
        schema_version: 1,
        limits: Limits {
            max_rows: 8_192,
            max_columns: 128,
            max_string_bytes: 8_192,
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
    .expect("static extended plan")
}

fn fixture(payload: &[u8]) -> RecordBatch {
    let bytes = if payload.is_empty() {
        &[0][..]
    } else {
        payload
    };
    let rows = bytes.len().min(64);
    let ids = bytes
        .iter()
        .take(rows)
        .map(|value| i64::from(*value))
        .collect::<Vec<_>>();
    let groups = bytes
        .iter()
        .take(rows)
        .map(|value| format!("g{:02x}", value % 8))
        .collect::<Vec<_>>();
    let values = bytes
        .iter()
        .take(rows)
        .enumerate()
        .map(|(row, value)| (row % 5 != 0).then_some(f64::from(*value)))
        .collect::<Vec<_>>();
    let fallback = bytes
        .iter()
        .rev()
        .take(rows)
        .map(|value| Some(f64::from(*value)))
        .collect::<Vec<_>>();
    let dates = bytes
        .iter()
        .take(rows)
        .map(|value| format!("2024-{:02}-{:02}", value % 12 + 1, value % 28 + 1))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("group", DataType::Utf8, false),
            Field::new("value", DataType::Float64, true),
            Field::new("fallback", DataType::Float64, true),
            Field::new("date", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(StringArray::from(groups)),
            Arc::new(Float64Array::from(values)),
            Arc::new(Float64Array::from(fallback)),
            Arc::new(StringArray::from(dates)),
        ],
    )
    .expect("bounded fixture")
}

fn assert_bounded(output: &RecordBatch, plan: &ValidatedPlan) {
    assert!(output.num_rows() <= plan.limits().max_rows);
    assert!(output.num_columns() <= plan.limits().max_columns);
    assert!(output
        .columns()
        .iter()
        .all(|column| column.len() == output.num_rows()));
}

fuzz_target!(|payload: &[u8]| {
    let input = fixture(payload);
    let selector = payload.first().copied().unwrap_or_default() % 14;
    let unary = match selector {
        0 => Some((
            "coalesce",
            json!({"columns":["value","fallback"],"output_column":"out"}),
        )),
        1 => Some((
            "sha256_hash",
            json!({"columns":["id","group"],"output_column":"hash","normalize":true}),
        )),
        2 => Some((
            "rolling_window",
            json!({"column":"value","function":"mean","group_by":"group","window":8,"min_periods":1,"output_column":"rolling"}),
        )),
        3 => Some(("assert_unique", json!({"columns":["id","group"]}))),
        4 => Some(("assert_range", json!({"column":"id","min":0.0,"max":255.0}))),
        5 => Some((
            "assert_regex",
            json!({"column":"group","pattern":"^g[0-9a-f]{2}$"}),
        )),
        6 => Some((
            "date_format",
            json!({"column":"date","input_format":"%Y-%m-%d","output_format":"%d/%m/%Y","output_column":"formatted","invalid":"error"}),
        )),
        7 => Some((
            "date_add",
            json!({"column":"date","input_format":"%Y-%m-%d","output_format":"%Y-%m-%d","amount":1,"unit":"months","output_column":"shifted","invalid":"error"}),
        )),
        _ => None,
    };
    if let Some((operation, config)) = unary {
        let operation_plan = plan(operation, config);
        if let Ok(output) = execute_batch(input.clone(), &operation_plan) {
            assert_bounded(&output, &operation_plan);
        }
    } else {
        let (operation, config) = match selector {
            8 => ("semi_join", json!({"left_keys":["id"],"right_keys":["id"]})),
            9 => ("anti_join", json!({"left_keys":["id"],"right_keys":["id"]})),
            10 => ("union_distinct", json!({})),
            11 => ("intersect", json!({})),
            12 => ("except", json!({})),
            _ => (
                "asof_join",
                json!({"left_on":"id","right_on":"id","direction":"nearest","tolerance":16.0,"allow_exact":false}),
            ),
        };
        let operation_plan = plan(operation, config);
        if let Ok(output) = execute_binary(&input, &input, &operation_plan) {
            assert_bounded(&output, &operation_plan);
        }
    }

    let lists = ListArray::from_iter_primitive::<Int64Type, _, _>(payload.chunks(4).take(64).map(
        |chunk| {
            Some(
                chunk
                    .iter()
                    .map(|value| Some(i64::from(*value)))
                    .collect::<Vec<_>>(),
            )
        },
    ));
    let list_input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "items",
            lists.data_type().clone(),
            false,
        )])),
        vec![Arc::new(lists)],
    )
    .expect("list fixture");
    let explode_plan = plan(
        "explode",
        json!({"column":"items","output_column":"item","empty_policy":"null"}),
    );
    let exploded = execute_batch(list_input, &explode_plan).expect("bounded explode");
    assert_bounded(&exploded, &explode_plan);

    let rows = payload.len().min(64);
    let structure = StructArray::from(vec![
        (
            Arc::new(Field::new("x", DataType::Int64, false)),
            Arc::new(Int64Array::from(
                (0..rows)
                    .map(|row| i64::try_from(row).expect("bounded"))
                    .collect::<Vec<_>>(),
            )) as ArrayRef,
        ),
        (
            Arc::new(Field::new("s", DataType::Utf8, false)),
            Arc::new(StringArray::from(
                (0..rows).map(|row| format!("v{row}")).collect::<Vec<_>>(),
            )) as ArrayRef,
        ),
    ]);
    let struct_input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "payload",
            structure.data_type().clone(),
            false,
        )])),
        vec![Arc::new(structure)],
    )
    .expect("struct fixture");
    let unnest_plan = plan(
        "unnest",
        json!({"column":"payload","prefix":"p_","drop_source":true}),
    );
    let unnested = execute_batch(struct_input, &unnest_plan).expect("bounded unnest");
    assert_bounded(&unnested, &unnest_plan);
});
