//! Una colonna sostituita non conserva prove sul vecchio `FieldId`.
use plenora_core::arrow::array::{Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_kernels_table::{aggregation, analyze::analyze_table_contract, Limits};
use serde_json::json;
use std::sync::Arc;

#[test]
fn finestre_che_sostituiscono_la_chiave_invalidano_sorted_by() {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", DataType::Int64, false),
            Field::new("v", DataType::Int64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 2])),
            Arc::new(Int64Array::from(vec![20, 10])),
        ],
    )
    .unwrap();
    for rolling in [false, true] {
        for explicit_order in [false, true] {
            for overwrite in [false, true] {
                let mut fields = FieldAllocator::default();
                let input = analyze_table_contract(
                    "table.sort",
                    &[DataContract::tabular(batch.schema())],
                    &json!({"columns":["k"]}),
                    &mut fields,
                    &Limits::default(),
                )
                .unwrap();
                let mut config = if rolling {
                    json!({"column":"v","function":"min","window":1})
                } else {
                    json!({"column":"v","function":"lag","offset":1})
                };
                config["output_column"] = json!(if overwrite { "k" } else { "result" });
                if explicit_order {
                    config["order_column"] = json!("k");
                }
                let op = if rolling {
                    "table.rolling_window"
                } else {
                    "table.window_function"
                };
                let output =
                    analyze_table_contract(op, &[input], &config, &mut fields, &Limits::default())
                        .unwrap();
                assert_eq!(output.properties.sorted_by.is_none(), overwrite);
                let actual = if rolling {
                    aggregation::rolling_window(&batch, &serde_json::from_value(config).unwrap())
                } else {
                    aggregation::window_function(&batch, &serde_json::from_value(config).unwrap())
                }
                .unwrap();
                let index = if overwrite { 0 } else { 2 };
                let values = actual
                    .column(index)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap();
                if rolling {
                    assert_eq!((values.value(0), values.value(1)), (20, 10));
                } else {
                    use plenora_core::arrow::array::Array;
                    assert!(values.is_null(0));
                    assert_eq!(values.value(1), 20);
                }
            }
        }
    }
}

#[test]
fn rinomina_simultanea_conserva_identita_della_chiave() {
    for second_target in ["a", "c"] {
        let mut input = DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int64, false),
            Field::new("b", DataType::Int64, false),
        ])));
        let mut fields = FieldAllocator::default();
        let a = fields.intern("a").unwrap();
        let b = fields.intern("b").unwrap();
        // La provenienza e' nel contratto, non nella cache dei nomi dell'allocatore.
        input.field_ids.insert("a".into(), a);
        input.field_ids.insert("b".into(), b);
        let sorted = analyze_table_contract(
            "table.sort",
            &[input],
            &json!({"columns":["a"]}),
            &mut fields,
            &Limits::default(),
        )
        .unwrap();
        let output = analyze_table_contract("table.rename",&[sorted],&json!({"renames":[{"old_name":"a","new_name":"b"},{"old_name":"b","new_name":second_target}]}),&mut fields,&Limits::default()).unwrap();
        assert_eq!(fields.intern("b").unwrap(), a);
        assert_eq!(fields.intern(second_target).unwrap(), b);
        assert_eq!(
            output
                .properties
                .sorted_by
                .unwrap()
                .confidence
                .value()
                .unwrap()
                .keys,
            vec![a]
        );
    }
}
