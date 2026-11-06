//! Le risoluzioni per nome non devono scegliere la prima colonna omonima.
use plenora_core::arrow::array::{Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::PlenoraError;
use plenora_kernels_table::{
    column_index,
    joins::{concat_by_name, ConcatByName},
    replace_or_append, Limits,
};
use std::sync::Arc;

#[test]
fn colonne_omonime_rifiutate_anche_con_un_solo_batch() {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, false),
            Field::new("x", DataType::Int64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1])),
            Arc::new(Int64Array::from(vec![2])),
        ],
    )
    .unwrap();
    for strict in [false, true] {
        let error = concat_by_name(&[&batch], &ConcatByName { strict }, &Limits::default())
            .expect_err("rifiuto atteso: categoria verificata dalla prova");
        assert!(matches!(error, PlenoraError::Schema(_)));
        assert!(error.to_string().ends_with("nomi di colonna duplicati"));
    }
    assert!(matches!(
        column_index(&batch, "x"),
        Err(PlenoraError::Schema(_))
    ));
    assert!(matches!(
        replace_or_append(
            &batch,
            "x",
            DataType::Int64,
            false,
            Arc::new(Int64Array::from(vec![3]))
        ),
        Err(PlenoraError::Schema(_))
    ));
}

#[test]
fn ricerca_geometrica_rifiuta_colonne_omonime() {
    use plenora_core::contract::arrow_metadata::{geometry_column_index, geometry_output_field};
    let field = geometry_output_field("g", "EPSG:3857").unwrap();
    let schema = Schema::new(vec![field.clone(), field]);
    assert!(matches!(
        geometry_column_index(&schema, "g"),
        Err(PlenoraError::Schema(_))
    ));
}
