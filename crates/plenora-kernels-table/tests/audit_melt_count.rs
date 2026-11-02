//! Cardinalita' di melt verificata senza materializzare le righe.
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractProperty, DataContract, FieldAllocator, PropertyConfidence, PropertyScope,
};
use plenora_core::PlenoraError;
use plenora_kernels_table::{analyze::analyze_table_contract, Limits};
use serde_json::json;
use std::sync::Arc;

#[test]
fn melt_non_attesta_un_conteggio_saturato() {
    for rows in [0, 1, u64::MAX / 2, 1_u64 << 63, u64::MAX] {
        for confidence in [
            PropertyConfidence::Proven(rows),
            PropertyConfidence::Declared(rows),
            PropertyConfidence::Estimated(rows),
        ] {
            let mut input = DataContract::tabular(Arc::new(Schema::new(vec![
                Field::new("a", DataType::Int64, false),
                Field::new("b", DataType::Int64, false),
            ])));
            input.properties.row_count =
                Some(ContractProperty::new(confidence, PropertyScope::Dataset));
            let result = analyze_table_contract(
                "table.melt",
                &[input],
                &json!({"id_columns":[],"value_columns":["a","b"]}),
                &mut FieldAllocator::default(),
                &Limits::default(),
            );
            match rows.checked_mul(2) {
                Some(expected) => assert_eq!(
                    result
                        .unwrap()
                        .properties
                        .row_count
                        .unwrap()
                        .confidence
                        .value(),
                    Some(&expected)
                ),
                None => assert!(matches!(result, Err(PlenoraError::ResourceLimit(_)))),
            }
        }
    }
}
