//! Rifiuti transitori `GeoParquet` prima di interpretare i valori geometrici.
use plenora_core::{
    arrow::{
        array::{BinaryArray, RecordBatch},
        schema::{DataType, Field, Schema},
    },
    ErrorCategory,
};
use plenora_io::geoparquet::{applica, prepara};
use std::{collections::HashMap, sync::Arc};

fn batch(count: usize, primary: Option<&str>) -> RecordBatch {
    let fields: Vec<_> = (0..count)
        .map(|i| {
            Field::new(format!("g{i}"), DataType::Binary, true).with_metadata(HashMap::from([
                ("ARROW:extension:name".into(), "geoarrow.wkb".into()),
                ("plenora.field_id".into(), i.to_string()),
            ]))
        })
        .collect();
    let mut metadata = HashMap::from([("plenora.contract.version".into(), "1".into())]);
    if let Some(primary) = primary {
        metadata.insert("plenora.primary_geometry".into(), primary.into());
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        (0..count)
            .map(|_| Arc::new(BinaryArray::from(vec![Some(&[255][..])])) as _)
            .collect(),
    )
    .expect("fixture")
}

#[test]
fn scrittura_multi_e_senza_primaria_rifiutate_prima_dei_byte() {
    for (count, primary) in [(2, None), (2, Some("1")), (1, Some("none"))] {
        let result = prepara(&batch(count, primary));
        let Err(error) = result else {
            panic!("atteso rifiuto")
        };
        assert_eq!(error.category(), ErrorCategory::Unsupported);
    }
}

#[test]
fn lettura_multi_e_primaria_contraddittoria_rifiutate_prima_dei_byte() {
    let multi = r#"{"version":"1.1.0","primary_column":"g1","columns":{"g0":{"encoding":"WKB","geometry_types":[],"crs":null},"g1":{"encoding":"WKB","geometry_types":[],"crs":null}}}"#;
    assert_eq!(
        applica(&batch(2, Some("1")), multi)
            .expect_err("multi")
            .category(),
        ErrorCategory::Unsupported
    );
    let single = r#"{"version":"1.1.0","primary_column":"g0","columns":{"g0":{"encoding":"WKB","geometry_types":[],"crs":null}}}"#;
    assert_eq!(
        applica(&batch(1, Some("none")), single)
            .expect_err("discordanza")
            .category(),
        ErrorCategory::Schema
    );
}
