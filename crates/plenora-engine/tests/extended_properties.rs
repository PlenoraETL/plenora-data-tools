#![allow(clippy::float_cmp)] // Generated bounded integer sums are exactly representable as f64.

use std::collections::HashSet;
use std::sync::Arc;

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::{execute_batch, execute_binary, Limits, Plan, Step, ValidatedPlan};
use proptest::prelude::*;
use serde_json::{json, Value};

fn plan(operation: &str, config: Value) -> ValidatedPlan {
    Plan {
        schema_version: 1,
        limits: Limits {
            max_rows: 10_000,
            ..Limits::default()
        },
        steps: vec![Step {
            operation: operation.into(),
            config,
        }],
    }
    .validate()
    .expect("static property plan")
}

fn integers(name: &str, values: Vec<i64>) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(name, DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(values))],
    )
    .expect("integer fixture")
}

fn string_pairs(values: &[(String, String)]) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("first", DataType::Utf8, false),
            Field::new("second", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(
                values
                    .iter()
                    .map(|value| value.0.as_str())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                values
                    .iter()
                    .map(|value| value.1.as_str())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("string pair fixture")
}

proptest! {
    #[test]
    fn semi_and_anti_form_a_stable_partition(
        left in prop::collection::vec(-20_i64..=20, 0..80),
        right in prop::collection::vec(-20_i64..=20, 0..80),
    ) {
        let left_batch = integers("key", left.clone());
        let right_batch = integers("key", right.clone());
        let config = json!({"left_keys":["key"],"right_keys":["key"]});
        let semi = execute_binary(&left_batch, &right_batch, &plan("semi_join", config.clone())).expect("semi");
        let anti = execute_binary(&left_batch, &right_batch, &plan("anti_join", config)).expect("anti");
        let members = right.into_iter().collect::<HashSet<_>>();
        let expected_semi = left.iter().filter(|value| members.contains(value)).count();
        prop_assert_eq!(semi.num_rows(), expected_semi);
        prop_assert_eq!(semi.num_rows() + anti.num_rows(), left.len());
    }

    #[test]
    fn set_operations_match_mathematical_sets_with_stable_distinct_rows(
        left in prop::collection::vec(-20_i64..=20, 0..80),
        right in prop::collection::vec(-20_i64..=20, 0..80),
    ) {
        let left_batch = integers("value", left.clone());
        let right_batch = integers("value", right.clone());
        let left_set = left.iter().copied().collect::<HashSet<_>>();
        let right_set = right.iter().copied().collect::<HashSet<_>>();
        let union = execute_binary(&left_batch, &right_batch, &plan("union_distinct", json!({}))).expect("union");
        let intersect = execute_binary(&left_batch, &right_batch, &plan("intersect", json!({}))).expect("intersect");
        let except = execute_binary(&left_batch, &right_batch, &plan("except", json!({}))).expect("except");
        prop_assert_eq!(union.num_rows(), left_set.union(&right_set).count());
        prop_assert_eq!(intersect.num_rows(), left_set.intersection(&right_set).count());
        prop_assert_eq!(except.num_rows(), left_set.difference(&right_set).count());
    }

    #[test]
    fn intersect_string_pairs_matches_tuple_equality_without_framing_collisions(
        left in prop::collection::vec(("[a-c]{0,4}", "[a-c]{0,4}"), 0..80),
        right in prop::collection::vec(("[a-c]{0,4}", "[a-c]{0,4}"), 0..80),
    ) {
        let right_set = right.iter().cloned().collect::<HashSet<_>>();
        let mut emitted = HashSet::new();
        let expected = left
            .iter()
            .filter(|value| right_set.contains(*value) && emitted.insert((*value).clone()))
            .cloned()
            .collect::<Vec<_>>();
        let output = execute_binary(
            &string_pairs(&left),
            &string_pairs(&right),
            &plan("intersect", json!({})),
        )
        .expect("intersect string pairs");
        let first = output
            .column_by_name("first")
            .expect("first")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        let second = output
            .column_by_name("second")
            .expect("second")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        let actual = first
            .iter()
            .zip(second.iter())
            .map(|(first, second)| {
                (
                    first.expect("non-null").to_owned(),
                    second.expect("non-null").to_owned(),
                )
            })
            .collect::<Vec<_>>();
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn coalesce_selects_the_first_non_null_without_type_loss(
        pairs in prop::collection::vec((prop::option::of(-1_000_i64..=1_000), prop::option::of(-1_000_i64..=1_000)), 0..100),
    ) {
        let first = pairs.iter().map(|pair| pair.0).collect::<Vec<_>>();
        let second = pairs.iter().map(|pair| pair.1).collect::<Vec<_>>();
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Int64, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![Arc::new(Int64Array::from(first)), Arc::new(Int64Array::from(second))],
        ).expect("coalesce fixture");
        let output = execute_batch(input, &plan("coalesce", json!({"columns":["a","b"],"output_column":"out"}))).expect("coalesce");
        let values = output.column_by_name("out").expect("out").as_any().downcast_ref::<Int64Array>().expect("int64");
        for (row, pair) in pairs.iter().enumerate() {
            prop_assert_eq!((!values.is_null(row)).then(|| values.value(row)), pair.0.or(pair.1));
        }
    }

    #[test]
    fn rolling_sum_matches_a_simple_bounded_reference(
        values in prop::collection::vec(-100_i64..=100, 1..100),
        window in 1_usize..20,
    ) {
        let input = integers("value", values.clone());
        let output = execute_batch(input, &plan("rolling_window", json!({"column":"value","function":"sum","window":window,"min_periods":1,"output_column":"rolling"}))).expect("rolling");
        let rolling = output.column_by_name("rolling").expect("rolling").as_any().downcast_ref::<Float64Array>().expect("float");
        for row in 0..values.len() {
            let start = (row + 1).saturating_sub(window);
            let expected = values[start..=row].iter().sum::<i64>().to_f64().expect("bounded sum");
            prop_assert!((rolling.value(row) - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn sha256_is_deterministic_and_fixed_width(
        values in prop::collection::vec("[a-zA-Z0-9]{0,32}", 0..80),
    ) {
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, false)])),
            vec![Arc::new(StringArray::from(values))],
        ).expect("hash fixture");
        let hash_plan = plan("sha256_hash", json!({"columns":["value"],"output_column":"hash"}));
        let first = execute_batch(input.clone(), &hash_plan).expect("first");
        let second = execute_batch(input, &hash_plan).expect("second");
        let first = first.column_by_name("hash").expect("hash").as_any().downcast_ref::<StringArray>().expect("utf8");
        let second = second.column_by_name("hash").expect("hash").as_any().downcast_ref::<StringArray>().expect("utf8");
        prop_assert_eq!(first, second);
        prop_assert!(first.iter().flatten().all(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn forced_spill_matches_the_in_memory_set_contract(
        left in proptest::collection::vec(-100_i64..100, 1..128),
        right in proptest::collection::vec(-100_i64..100, 1..128),
    ) {
        let make_batch = |values: Vec<i64>| {
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
                vec![Arc::new(Int64Array::from(values))],
            )
            .expect("property batch")
        };
        let left = make_batch(left);
        let right = make_batch(right);
        for operation in ["union_distinct", "intersect", "except"] {
            let expected = execute_binary(&left, &right, &plan(operation, json!({})))
                .expect("in-memory set operation");
            let forced = Plan {
                schema_version: 1,
                limits: Limits {
                    max_governed_memory_bytes: 2_048,
                    spill_partitions: 32,
                    ..Limits::default()
                },
                steps: vec![Step {
                    operation: operation.into(),
                    config: json!({}),
                }],
            }
            .validate()
            .expect("spill plan");
            let actual = execute_binary(&left, &right, &forced)
                .expect("spilled set operation");
            let expected = expected.column(0).as_any().downcast_ref::<Int64Array>().expect("expected");
            let actual = actual.column(0).as_any().downcast_ref::<Int64Array>().expect("actual");
            prop_assert_eq!(actual.values(), expected.values());
        }
    }

    #[test]
    fn expression_arithmetic_matches_the_scalar_reference(
        left in -32_000_i64..32_000,
        right in -32_000_i64..32_000,
    ) {
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("left", DataType::Int64, false),
                Field::new("right", DataType::Int64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![left])),
                Arc::new(Int64Array::from(vec![right])),
            ],
        )
        .expect("expression input");
        let output = execute_batch(
            input,
            &plan(
                "expression",
                json!({
                    "output_column":"result",
                    "output_type":"number",
                    "expression":{"kind":"binary","op":"add",
                        "left":{"kind":"column","name":"left"},
                        "right":{"kind":"column","name":"right"}}
                }),
            ),
        )
        .expect("expression");
        let actual = output
            .column_by_name("result")
            .expect("result")
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("float")
            .value(0);
        prop_assert_eq!(actual, (left + right).to_f64().expect("bounded"));
    }

    #[test]
    fn reconciliation_totals_are_conservative(
        left in proptest::collection::vec(0_i64..32, 0..128),
        right in proptest::collection::vec(0_i64..32, 0..128),
    ) {
        let make_batch = |values: &[i64]| {
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
                vec![Arc::new(Int64Array::from(values.to_vec()))],
            )
            .expect("reconcile input")
        };
        let output = execute_binary(
            &make_batch(&left),
            &make_batch(&right),
            &plan(
                "reconcile",
                json!({"left_keys":["id"],"right_keys":["id"],"nulls_equal":true}),
            ),
        )
        .expect("reconcile");
        let values = output
            .column(1)
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::UInt64Array>()
            .expect("metrics");
        prop_assert_eq!(values.value(0) + values.value(1), u64::try_from(left.len()).expect("left"));
        prop_assert_eq!(values.value(0) + values.value(2), u64::try_from(right.len()).expect("right"));
    }
}
