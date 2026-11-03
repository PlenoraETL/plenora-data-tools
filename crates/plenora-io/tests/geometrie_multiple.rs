//! Geometrie multiple e primaria al confine dei file.
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
fn scrittura_senza_primaria_rifiutata_prima_dei_byte() {
    for (count, primary) in [(2, None), (1, Some("none"))] {
        let result = prepara(&batch(count, primary));
        let Err(error) = result else {
            panic!("atteso rifiuto")
        };
        assert_eq!(error.category(), ErrorCategory::Unsupported);
    }
}

#[test]
fn lettura_multi_verifica_byte_e_rifiuta_primaria_contraddittoria() {
    let multi = r#"{"version":"1.1.0","primary_column":"g1","columns":{"g0":{"encoding":"WKB","geometry_types":[],"crs":null},"g1":{"encoding":"WKB","geometry_types":[],"crs":null}}}"#;
    assert_eq!(
        applica(&batch(2, Some("1")), multi)
            .expect_err("multi")
            .category(),
        ErrorCategory::DataMapping
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

fn fixture_io() -> RecordBatch {
    plenora_io::leggi_tabella(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/dati/io700_multigeometria.parquet"),
        None,
        u64::MAX,
    )
    .unwrap()
}

fn contratto(t: &RecordBatch) -> plenora_core::contract::DataContract {
    plenora_core::contract::arrow_schema::contract_from_arrow_schema(
        t.schema(),
        plenora_core::crs::resolve_crs,
    )
    .unwrap()
}

#[test]
fn io700_geoparquet_ipc_file_stream_conservano_tutte_le_geometrie() {
    let t = fixture_io();
    let c = contratto(&t);
    assert_eq!(t.num_rows(), 3);
    assert_eq!(c.geometries.len(), 2);
    assert_eq!(c.active_geometry_column().unwrap().name, "posizione");
    assert_eq!(c.geometries[0].name, "traccia");
    let dir = tempfile::tempdir().unwrap();
    for ext in ["parquet", "arrow", "arrows"] {
        let path = dir.path().join(format!("roundtrip.{ext}"));
        plenora_io::scrivi_tabella(&t, &path, &plenora_io::OpzioniScrittura::default()).unwrap();
        let read = plenora_io::leggi_tabella(&path, None, u64::MAX).unwrap();
        assert_eq!(t.columns(), read.columns());
        let after = contratto(&read);
        for (before, after) in c.geometries.iter().zip(&after.geometries) {
            assert_eq!(before.name, after.name);
            assert_eq!(before.encoding, after.encoding);
            assert_eq!(before.dimensions, after.dimensions);
            assert_eq!(before.nullable, after.nullable);
            assert_eq!(before.types, after.types);
            assert!(before
                .crs
                .as_resolved()
                .unwrap()
                .semantically_equals(after.crs.as_resolved().unwrap()));
        }
        assert_eq!(c.active_geometry, after.active_geometry);
        for name in ["traccia", "posizione"] {
            assert_eq!(
                t.schema().field_with_name(name).unwrap().metadata()["plenora.field_id"],
                read.schema().field_with_name(name).unwrap().metadata()["plenora.field_id"]
            );
        }
    }
}

#[test]
fn ipc_conserva_assenza_primaria_anche_con_una_sola_geometria() {
    let t = fixture_io();
    let dir = tempfile::tempdir().unwrap();
    for indices in [vec![0, 1, 2], vec![0]] {
        let t = t.project(&indices).unwrap();
        let mut md = t.schema().metadata().clone();
        md.insert("plenora.primary_geometry", "none");
        let t = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(t.schema().fields().clone(), md)),
            t.columns().to_vec(),
        )
        .unwrap();
        for ext in ["arrow", "arrows"] {
            let path = dir.path().join(format!("none-{}.{ext}", indices.len()));
            plenora_io::scrivi_tabella(&t, &path, &plenora_io::OpzioniScrittura::default())
                .unwrap();
            let read = plenora_io::leggi_tabella(&path, None, u64::MAX).unwrap();
            assert_eq!(t.columns(), read.columns());
            assert!(contratto(&read).active_geometry.is_none());
        }
        assert_eq!(
            prepara(&t).err().unwrap().category(),
            ErrorCategory::Unsupported
        );
    }
}

#[test]
fn secondaria_invalida_non_pubblica_ne_sostituisce() {
    let t = fixture_io();
    let dir = tempfile::tempdir().unwrap();
    let mut columns = t.columns().to_vec();
    columns[0] = Arc::new(BinaryArray::from(vec![
        Some(b"segreto".as_slice()),
        None,
        None,
    ]));
    let invalid = RecordBatch::try_new(t.schema(), columns).unwrap();
    for existing in [false, true] {
        let path = dir.path().join(format!("invalid-{existing}.parquet"));
        if existing {
            std::fs::write(&path, b"precedente").unwrap();
        }
        let options = plenora_io::OpzioniScrittura {
            sovrascrivi: true,
            ..Default::default()
        };
        let error = plenora_io::scrivi_tabella(&invalid, &path, &options).unwrap_err();
        assert_eq!(error.category(), ErrorCategory::DataMapping);
        assert!(!error.to_string().contains("segreto"));
        if existing {
            assert_eq!(std::fs::read(path).unwrap(), b"precedente");
        } else {
            assert!(!path.exists());
        }
    }
}

#[test]
fn riproiezione_secondaria_ricalcola_footer_e_preserva_primaria() {
    let t = fixture_io();
    let pipeline = plenora_pipeline::Pipeline::from_json(r#"{"version":1,"inputs":["t"],"steps":[{"op":"geo.reproject","in":["t"],"out":"r","config":{"input_geometry":"traccia","target_crs":"EPSG:3857"}}],"outputs":["r"]}"#).unwrap();
    let valid = pipeline.validate(&[("t", t.schema())]).unwrap();
    let output = valid.run([("t".into(), t.clone())].into()).unwrap();
    let r = &output.outputs[0].1;
    assert_eq!(t.column(2), r.column(2));
    assert_ne!(t.column(0), r.column(0));
    assert_eq!(
        contratto(r).active_geometry_column().unwrap().name,
        "posizione"
    );
    let prepared = prepara(r).unwrap().unwrap();
    let geo: serde_json::Value = serde_json::from_str(&prepared.geo).unwrap();
    assert_eq!(geo["primary_column"], "posizione");
    assert_eq!(geo["columns"].as_object().unwrap().len(), 2);
    assert!(geo["columns"]["traccia"].get("covering").is_none());
    assert!(geo["columns"]["traccia"]["bbox"][0].as_f64().unwrap() > 900_000.0);
    let read = applica(&prepared.tabella, &prepared.geo).unwrap();
    assert_eq!(r.columns(), read.columns());
}

#[test]
fn io700_crs_ridotto_non_diventa_equivalente_per_solo_id() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/dati/io700_multigeometria_crs_ridotto.parquet");
    assert_eq!(
        plenora_io::leggi_tabella(&path, None, u64::MAX)
            .unwrap_err()
            .category(),
        ErrorCategory::Crs
    );
}

#[test]
fn dichiarazioni_secondarie_incoerenti_rifiutate_anche_senza_righe() {
    let t = fixture_io();
    let prepared = prepara(&t).unwrap().unwrap();
    for rows in [0, 3] {
        for (key, value, category) in [
            ("geo", r#"{"encoding":"ewkb"}"#, ErrorCategory::Schema),
            ("geo", r#"{"dimensions":"xyz"}"#, ErrorCategory::Schema),
            ("plenora.geometry.crs_id", "EPSG:3857", ErrorCategory::Crs),
        ] {
            let mut fields: Vec<_> = prepared
                .tabella
                .schema()
                .fields()
                .iter()
                .map(|f| f.as_ref().clone())
                .collect();
            let mut md = fields[0].metadata().clone();
            // Anche il solo legacy deve essere confrontato prima di sostituirlo.
            if key == "geo" {
                md.remove("plenora.geometry.encoding");
                md.remove("plenora.geometry.dimensions");
            }
            md.insert(key, value);
            fields[0] = fields[0].clone().with_metadata(md);
            let tab = RecordBatch::try_new(
                Arc::new(Schema::new_with_metadata(
                    fields,
                    prepared.tabella.schema().metadata().clone(),
                )),
                prepared.tabella.columns().to_vec(),
            )
            .unwrap()
            .slice(0, rows);
            assert_eq!(
                applica(&tab, &prepared.geo).unwrap_err().category(),
                category
            );
        }
    }
}

#[test]
fn metadati_derivati_non_si_riusano_e_riferimenti_pendenti_si_rifiutano() {
    let t = fixture_io();
    let mut fields: Vec<_> = t
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    for index in [0, 2] {
        let mut md = fields[index].metadata().clone();
        for key in ["bbox", "covering", "orientation"] {
            md.insert(
                format!("plenora.geometry.native.geoparquet.{key}"),
                "obsoleto",
            );
        }
        fields[index] = fields[index].clone().with_metadata(md);
    }
    let t = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            t.schema().metadata().clone(),
        )),
        t.columns().to_vec(),
    )
    .unwrap();
    let prepared = prepara(&t).unwrap().unwrap();
    for index in [0, 2] {
        assert!(!prepared
            .tabella
            .schema()
            .field(index)
            .metadata()
            .values()
            .any(|v| v == "obsoleto"));
    }
    let mut geo: serde_json::Value = serde_json::from_str(&prepared.geo).unwrap();
    geo["primary_column"] = serde_json::json!("assente");
    assert_eq!(
        applica(&prepared.tabella, &geo.to_string())
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidPlan
    );
    geo["primary_column"] = serde_json::json!("posizione");
    geo["columns"]["assente"] = geo["columns"]["traccia"].clone();
    assert_eq!(
        applica(&prepared.tabella, &geo.to_string())
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidPlan
    );
}

#[test]
fn footer_senza_tipi_non_nasconde_dichiarazioni_secondarie() {
    let t = fixture_io();
    let prepared = prepara(&t).unwrap().unwrap();
    for (key, value, category) in [
        (
            "plenora.geometry.types",
            "point",
            ErrorCategory::DataMapping,
        ),
        (
            "plenora.geometry.dimensions",
            "xyz",
            ErrorCategory::DataMapping,
        ),
        (
            "plenora.geometry.dimensions",
            "xym",
            ErrorCategory::Unsupported,
        ),
        (
            "plenora.geometry.dimensions",
            "xyzm",
            ErrorCategory::Unsupported,
        ),
    ] {
        let mut fields: Vec<_> = prepared
            .tabella
            .schema()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect();
        let mut md = fields[0].metadata().clone();
        md.insert(key, value);
        fields[0] = fields[0].clone().with_metadata(md);
        let t = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                fields,
                prepared.tabella.schema().metadata().clone(),
            )),
            prepared.tabella.columns().to_vec(),
        )
        .unwrap();
        let mut geo: serde_json::Value = serde_json::from_str(&prepared.geo).unwrap();
        geo["columns"]["traccia"]["geometry_types"] = serde_json::json!([]);
        assert_eq!(
            applica(&t, &geo.to_string()).unwrap_err().category(),
            category
        );
        if category == ErrorCategory::Unsupported {
            assert_eq!(
                applica(&t.slice(0, 0), &geo.to_string())
                    .unwrap_err()
                    .category(),
                category
            );
        }
    }
}
