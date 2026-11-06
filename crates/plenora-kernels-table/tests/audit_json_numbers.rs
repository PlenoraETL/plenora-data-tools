//! Regressioni della precisione numerica nelle celle JSON.
use plenora_core::arrow::array::{RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::PlenoraError;
use plenora_kernels_table::{
    analysis::{flatten_json, FlattenJson},
    Limits,
};
use serde_json::json;
use std::sync::Arc;

#[test]
fn numeri_json_inesatti_rifiutati_in_ogni_percorso() {
    for document in [
        r#"{"n":9007199254740993.0}"#,
        r#"{"n":0.30000000000000001}"#,
        r#"{"n":[9007199254740993.0]}"#,
        r#"{"a.b":9007199254740993.0}"#,
        r#"{"n":{"a":{"b":0.30000000000000001}}}"#,
        r#"{"$serde_json::private::RawValue":"{\"n\":9007199254740993.0}"}"#,
        r#"{"n":[{"$serde_json::private::RawValue":"9007199254740993.0"}]}"#,
        r#"{"\u0024serde_json::private::RawValue":"9007199254740993.0"}"#,
    ] {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("doc", DataType::Utf8, false)])),
            vec![Arc::new(StringArray::from(vec![document]))],
        )
        .unwrap();
        for outputs in [json!([]), json!(["doc_n"]), json!(["doc_missing"])] {
            let config: FlattenJson =
                serde_json::from_value(json!({"column":"doc", "output_columns": outputs})).unwrap();
            let error = flatten_json(&batch, &config, &Limits::default())
                .expect_err("rifiuto atteso: categoria verificata dalla prova");
            assert!(matches!(error, PlenoraError::DataMapping(_)));
            assert!(!error.to_string().contains("9007199254740993"));
            assert!(!error.to_string().contains("0.30000000000000001"));
        }
    }
}

#[test]
fn numeri_json_esatti_resta_il_valore() {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("doc", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec![
            r#"{"n":9007199254740993}"#,
            r#"{"n":0.3}"#,
        ]))],
    )
    .unwrap();
    let config =
        serde_json::from_value(json!({"column":"doc", "output_columns":["doc_n"]})).unwrap();
    let result = flatten_json(&batch, &config, &Limits::default()).unwrap();
    let values = result
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(values.value(0), "9007199254740993");
    assert_eq!(values.value(1), "0.3");
}
