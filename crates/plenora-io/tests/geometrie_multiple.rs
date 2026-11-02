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

#[test]
fn cross_join_sort_preserva_primaria_esportabile_geoparquet() {
    use plenora_core::arrow::array::Int64Array;
    let geometry = batch(1, None);
    let mut metadata = geometry.schema().field(0).metadata().clone();
    metadata.insert("plenora.geometry.axis_order", "easting_northing");
    metadata.insert("geo", r#"{"crs":"EPSG:3857"}"#);
    let schema = Arc::new(Schema::new_with_metadata(
        vec![geometry.schema().field(0).clone().with_metadata(metadata)],
        geometry.schema().metadata().clone(),
    ));
    let left = RecordBatch::try_new(
        schema,
        vec![Arc::new(BinaryArray::from(vec![None::<&[u8]>]))],
    )
    .expect("geometria nulla");
    let right = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("key", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1]))],
    )
    .expect("tabella");
    for side in 0..2 {
        let (a, b) = if side == 0 {
            (left.clone(), right.clone())
        } else {
            (right.clone(), left.clone())
        };
        let pipeline = plenora_pipeline::Pipeline::from_json(r#"{"version":1,"inputs":["a","b"],"steps":[{"op":"table.cross_join","in":["a","b"],"out":"joined","config":{}},{"op":"table.sort","in":["joined"],"out":"sorted","config":{"columns":["key"]}}],"outputs":["sorted"]}"#).expect("piano");
        let valid = pipeline
            .validate(&[("a", a.schema()), ("b", b.schema())])
            .expect("validazione");
        let output = valid
            .run([("a".into(), a), ("b".into(), b)].into())
            .expect("esecuzione");
        let result = &output.outputs[0].1;
        assert!(!result
            .schema()
            .metadata()
            .contains_key("plenora.primary_geometry"));
        let _ = prepara(result).unwrap_or_else(|error| panic!("GeoParquet: {error}"));
    }
}
