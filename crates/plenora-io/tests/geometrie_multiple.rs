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
        assert_eq!(c.geometries.len(), after.geometries.len());
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
fn metadati_invariati_conservati_e_riferimenti_pendenti_rifiutati() {
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
        assert_eq!(
            prepared
                .tabella
                .schema()
                .field(index)
                .metadata()
                .get("plenora.geometry.native.geoparquet.orientation")
                .map(String::as_str),
            Some("obsoleto")
        );
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
        if key == "plenora.geometry.dimensions" {
            let mut legacy: serde_json::Value =
                serde_json::from_str(md.get("geo").unwrap()).unwrap();
            legacy["dimensions"] = value.into();
            md.insert("geo", legacy.to_string());
        }
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

#[test]
fn secondaria_senza_crs_rifiuta_scrittura() {
    let input = fixture_io();
    let mut fields: Vec<_> = input
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    let mut md = plenora_core::arrow::Metadata::new();
    md.insert("ARROW:extension:name", "geoarrow.wkb");
    md.insert(
        "plenora.field_id",
        fields[2]
            .metadata()
            .get("plenora.field_id")
            .unwrap()
            .clone(),
    );
    fields[2] = fields[2].clone().with_metadata(md);
    let input = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema().metadata().clone(),
        )),
        input.columns().to_vec(),
    )
    .unwrap();
    assert_eq!(
        prepara(&input).err().unwrap().category(),
        ErrorCategory::Crs
    );
}

#[test]
fn estensione_vuota_conservata_in_scrittura_multigeometria() {
    let input = fixture_io();
    let fields: Vec<_> = input
        .schema()
        .fields()
        .iter()
        .map(|f| {
            let mut md = f.metadata().clone();
            if md
                .get("ARROW:extension:name")
                .is_some_and(|v| v == "geoarrow.wkb")
            {
                md.insert("ARROW:extension:metadata", " ");
            }
            f.as_ref().clone().with_metadata(md)
        })
        .collect();
    let input = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema().metadata().clone(),
        )),
        input.columns().to_vec(),
    )
    .unwrap();
    let prepared = prepara(&input).unwrap().unwrap();
    assert_eq!(
        prepared
            .tabella
            .schema()
            .field(0)
            .metadata()
            .get("ARROW:extension:metadata")
            .map(String::as_str),
        Some(" ")
    );
}

#[test]
fn io700_covering_ripulito_ed_epoch_per_colonna_in_parquet() {
    let input = plenora_io::leggi_tabella(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/riferimenti/io700/epoch-covering.arrow"),
        None,
        u64::MAX,
    )
    .unwrap();
    let source = input
        .schema()
        .fields()
        .iter()
        .position(|f| {
            f.metadata()
                .get("ARROW:extension:name")
                .is_some_and(|v| v == "geoarrow.wkb")
        })
        .unwrap();
    let mut fields = Vec::new();
    for (index, name) in ["prima", "secondaria"].into_iter().enumerate() {
        let mut md = input.schema().field(source).metadata().clone();
        md.insert("plenora.field_id", index.to_string());
        let mut geo: serde_json::Value =
            serde_json::from_str(md.get("geo").map_or("{}", String::as_str)).unwrap();
        geo["covering"] = serde_json::json!({"bbox":{"xmin":["assente"]}});
        md.insert("geo", geo.to_string());
        fields.push(
            input
                .schema()
                .field(source)
                .clone()
                .with_name(name)
                .with_metadata(md),
        );
    }
    let mut md = plenora_core::arrow::Metadata::new();
    md.insert("plenora.contract.version", "1");
    md.insert("plenora.primary_geometry", "1");
    let input = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, md)),
        vec![input.column(source).clone(); 2],
    )
    .unwrap();
    let prepared = prepara(&input).unwrap().unwrap();
    let read = applica(&prepared.tabella, &prepared.geo).unwrap();
    for field in read.schema().fields() {
        assert_eq!(
            field
                .metadata()
                .get("plenora.geometry.native.geoparquet.epoch")
                .map(String::as_str),
            Some("2023.5")
        );
        let geo: serde_json::Value =
            serde_json::from_str(field.metadata().get("geo").unwrap()).unwrap();
        assert!(geo.get("covering").is_none());
    }
}

#[test]
fn binaria_geo_non_copia_primaria_dal_destro() {
    let right = fixture_io();
    let mut md = right.schema().metadata().clone();
    md.insert("plenora.primary_geometry", "none");
    let left = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            right.schema().fields().clone(),
            md,
        )),
        right.columns().to_vec(),
    )
    .unwrap();
    let pipeline = plenora_pipeline::Pipeline::from_json(r#"{"version":1,"inputs":["l","r"],"steps":[{"op":"geo.nearest","in":["l","r"],"out":"o","config":{"left_geometry":"traccia","right_geometry":"traccia"}}],"outputs":["o"]}"#).unwrap();
    let valid = pipeline
        .validate(&[("l", left.schema()), ("r", right.schema())])
        .unwrap();
    let output = valid
        .run(vec![("l".into(), left), ("r".into(), right)])
        .unwrap();
    assert!(contratto(&output.outputs[0].1).active_geometry.is_none());
}

#[test]
#[allow(clippy::too_many_lines)] // Stessa fixture e oracolo invertito per i tre kernel diretti.
fn fusioni_dirette_multigeometria_senza_bbox_e_crs_discordanti_a_zero_righe() {
    let input = fixture_io().project(&[0, 1, 2]).unwrap();
    let marked = |different: bool| {
        let fields: Vec<_> = input
            .schema()
            .fields()
            .iter()
            .map(|f| {
                let mut md = f.metadata().clone();
                if md
                    .get("ARROW:extension:name")
                    .is_some_and(|v| v == "geoarrow.wkb")
                {
                    md.insert(
                        "plenora.geometry.native.geoparquet.bbox",
                        if different {
                            "-180,-90,180,90"
                        } else {
                            "-200,-100,200,100"
                        },
                    );
                    md.insert(
                        "plenora.geometry.native.geoparquet.orientation",
                        if different { "destra" } else { "sinistra" },
                    );
                }
                f.as_ref().clone().with_metadata(md)
            })
            .collect();
        RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                fields,
                input.schema().metadata().clone(),
            )),
            input.columns().to_vec(),
        )
        .unwrap()
    };
    let left = marked(false);
    let right = marked(true);
    let run = |op, left: &RecordBatch, right: &RecordBatch| {
        let limits = plenora_kernels_table::Limits::default();
        match op {
            0 => plenora_kernels_table::joins::concat(
                left,
                right,
                &serde_json::from_str("{}").unwrap(),
                &limits,
            ),
            1 => plenora_kernels_table::joins::concat_by_name(
                &[left, right],
                &serde_json::from_str("{}").unwrap(),
                &limits,
            ),
            _ => plenora_kernels_table::setops::union_distinct(
                left,
                right,
                &serde_json::from_str("{}").unwrap(),
                &limits,
            ),
        }
    };
    for op in 0..3 {
        let output = run(op, &left, &right).unwrap();
        let reversed = run(op, &right, &left).unwrap();
        assert_eq!(output.schema(), reversed.schema());
        for f in output.schema().fields() {
            assert!(!f
                .metadata()
                .contains_key("plenora.geometry.native.geoparquet.bbox"));
            assert!(!f
                .metadata()
                .contains_key("plenora.geometry.native.geoparquet.orientation"));
        }
        let mut fields: Vec<_> = right
            .schema()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect();
        let mut replacement = plenora_core::contract::arrow_metadata::geometry_output_field(
            fields[2].name(),
            "EPSG:3857",
        )
        .unwrap();
        let mut md = replacement.metadata().clone();
        md.insert(
            "plenora.field_id",
            fields[2]
                .metadata()
                .get("plenora.field_id")
                .unwrap()
                .clone(),
        );
        replacement = replacement.with_metadata(md);
        fields[2] = replacement;
        let wrong = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                fields,
                right.schema().metadata().clone(),
            )),
            right.columns().to_vec(),
        )
        .unwrap();
        for count in [0, left.num_rows()] {
            let error = run(op, &left.slice(0, count), &wrong.slice(0, count)).unwrap_err();
            assert_eq!(error.category(), ErrorCategory::Crs);
        }
    }
}

#[test]
fn primaria_implicita_senza_id_produttori_catena_e_trasporti() {
    use plenora_core::arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
    use plenora_core::contract::arrow_metadata::geometry_output_field;
    use plenora_core::contract::arrow_schema::primary_geometry_position;
    use serde_json::json;
    for producer in ["geo.from_coords", "geo.from_wkt"] {
        for position in [0, 3] {
            let mut fields = vec![
                Field::new("x", DataType::Float64, false),
                Field::new("y", DataType::Float64, false),
                Field::new("wkt", DataType::Utf8, false),
            ];
            let mut columns: Vec<ArrayRef> = vec![
                Arc::new(Float64Array::from(vec![1., 2.])),
                Arc::new(Float64Array::from(vec![3., 4.])),
                Arc::new(StringArray::from(vec!["POINT(1 3)", "POINT(2 4)"])),
            ];
            let field = geometry_output_field("old", "EPSG:3857").unwrap();
            let mut metadata = field.metadata().clone();
            metadata.insert("plenora.geometry.axis_order", "easting_northing");
            fields.insert(position, field.with_metadata(metadata));
            columns.insert(
                position,
                Arc::new(BinaryArray::from(vec![None::<&[u8]>, None])),
            );
            let input = RecordBatch::try_new(
                Arc::new(Schema::new_with_metadata(
                    fields,
                    HashMap::from([("plenora.contract.version".into(), "1".into())]),
                )),
                columns,
            )
            .unwrap();
            let right = RecordBatch::try_new(
                Arc::new(Schema::new_with_metadata(
                    vec![Field::new("key", DataType::Int64, false)
                        .with_metadata(HashMap::from([("plenora.field_id".into(), "0".into())]))],
                    HashMap::from([("plenora.contract.version".into(), "1".into())]),
                )),
                vec![Arc::new(Int64Array::from(vec![1]))],
            )
            .unwrap();
            let config = if producer == "geo.from_coords" {
                json!({"x_column":"x", "y_column":"y", "geometry_column":"new", "crs":"EPSG:3857"})
            } else {
                json!({"wkt_column":"wkt", "output_column":"new", "crs":"EPSG:3857"})
            };
            let plan = plenora_pipeline::Pipeline::from_json(&json!({"version":1,"inputs":["t","r"],"steps":[
                {"op":producer,"in":["t"],"out":"added","config":config},
                {"op":"table.rename","in":["added"],"out":"renamed","config":{"renames":[{"old_name":"old","new_name":"primary"}]}},
                {"op":"table.reorder_columns","in":["renamed"],"out":"ordered","config":{"columns":["new","primary"]}},
                {"op":"table.cross_join","in":["ordered","r"],"out":"joined","config":{}}
            ],"outputs":["added","joined"]}).to_string()).unwrap();
            for rows in [0, 2] {
                let t = input.slice(0, rows);
                let valid = plan
                    .validate(&[("t", t.schema()), ("r", right.schema())])
                    .unwrap();
                let output = valid
                    .run(vec![("t".into(), t.clone()), ("r".into(), right.clone())])
                    .unwrap();
                let mut new_ids = std::collections::BTreeSet::new();
                for (name, table) in &output.outputs {
                    let expected = if name == "added" { "old" } else { "primary" };
                    let schema = table.schema();
                    let p = primary_geometry_position(&schema).unwrap().unwrap();
                    assert_eq!(schema.field(p).name(), expected);
                    assert_eq!(table.column(p), t.column(position));
                    for field in schema.fields() {
                        let id = &field.metadata()["plenora.field_id"];
                        if field.name() == "key" {
                            assert_eq!(id, "0");
                        } else {
                            assert_ne!(id, "0");
                            assert!(new_ids.insert(id.clone()));
                        }
                    }
                    let dir = tempfile::tempdir().unwrap();
                    for ext in ["arrow", "arrows", "parquet"] {
                        let path = dir.path().join(format!("roundtrip.{ext}"));
                        plenora_io::scrivi_tabella(
                            table,
                            &path,
                            &plenora_io::OpzioniScrittura::default(),
                        )
                        .unwrap();
                        let read = plenora_io::leggi_tabella(&path, None, u64::MAX).unwrap();
                        assert_eq!(read.columns(), table.columns());
                        assert_eq!(
                            contratto(&read).active_geometry_column().unwrap().name,
                            expected
                        );
                        assert_eq!(
                            read.schema().field(p).metadata()["plenora.field_id"],
                            schema.field(p).metadata()["plenora.field_id"]
                        );
                    }
                }
            }
        }
    }
}

// La matrice confronta le classi sulla stessa fixture e sullo stesso oracolo.
#[test]
#[allow(clippy::too_many_lines)]
fn primaria_senza_id_composizioni_e_derivate_non_promosse() {
    use plenora_core::arrow::array::{ArrayRef, Int64Array, StringArray, StructArray};
    use plenora_core::contract::arrow_metadata::geometry_output_field;
    use plenora_core::contract::arrow_schema::primary_geometry_position;
    use plenora_kernels_table::{aggregation, joins, reshape, setops, Limits};
    use serde_json::json;
    let limits = Limits::default();
    let make = |name: &str| {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("key", DataType::Int64, false),
                geometry_output_field(name, "EPSG:3857").unwrap(),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1, 1])),
                Arc::new(BinaryArray::from(vec![None::<&[u8]>, None])),
            ],
        )
        .unwrap()
    };
    let a = make("a");
    let b = make("b");
    let primary = |batch: &RecordBatch| {
        primary_geometry_position(&batch.schema())
            .unwrap()
            .map(|p| batch.schema().field(p).name().clone())
    };
    for rows in [0, 2] {
        let a = a.slice(0, rows);
        let b = b.slice(0, rows);
        for op in ["table.join", "table.concat_by_name", "table.union_distinct"] {
            let config = match op {
                "table.join" => json!({"left_keys":["key"],"right_keys":["key"]}),
                "table.concat_by_name" => json!({"strict":false}),
                _ => json!({}),
            };
            // Union richiede gli stessi nomi: conserva l'unica primaria implicita.
            let right = if op == "table.union_distinct" { &a } else { &b };
            let direct = match op {
                "table.join" => joins::join(
                    &a,
                    right,
                    &serde_json::from_value(config.clone()).unwrap(),
                    &limits,
                ),
                "table.concat_by_name" => joins::concat_by_name(
                    &[&a, right],
                    &serde_json::from_value(config.clone()).unwrap(),
                    &limits,
                ),
                _ => setops::union_distinct(
                    &a,
                    right,
                    &serde_json::from_value(config.clone()).unwrap(),
                    &limits,
                ),
            }
            .unwrap();
            let expected = (op == "table.union_distinct").then(|| "a".to_owned());
            assert_eq!(primary(&direct), expected, "{op}");
            let plan = plenora_pipeline::Pipeline::from_json(&json!({"version":1,"inputs":["l","r"],"steps":[{"op":op,"in":["l","r"],"out":"o","config":config}],"outputs":["o"]}).to_string()).unwrap();
            let result = plan
                .validate(&[("l", a.schema()), ("r", right.schema())])
                .unwrap()
                .run(vec![("l".into(), a.clone()), ("r".into(), right.clone())])
                .unwrap();
            assert_eq!(primary(&result.outputs[0].1), expected, "{op}");
        }
        let appended =
            joins::concat(&a, &a, &serde_json::from_value(json!({})).unwrap(), &limits).unwrap();
        assert_eq!(primary(&appended), Some("a".into()));
        let config = json!({"group_by":["key"],"aggregations":[{"column":"a","function":"first","alias":"a"},{"column":"a","function":"last","alias":"copy"}]});
        let aggregated =
            aggregation::aggregate(&a, &serde_json::from_value(config.clone()).unwrap()).unwrap();
        assert_eq!(primary(&aggregated), None);
        let plan = plenora_pipeline::Pipeline::from_json(&json!({"version":1,"inputs":["t"],"steps":[{"op":"table.aggregate","in":["t"],"out":"o","config":config}],"outputs":["o"]}).to_string()).unwrap();
        assert_eq!(
            primary(
                &plan
                    .validate(&[("t", a.schema())])
                    .unwrap()
                    .run(vec![("t".into(), a.clone())])
                    .unwrap()
                    .outputs[0]
                    .1
            ),
            None
        );
        let pivot = reshape::pivot(&a, &serde_json::from_value(json!({"index_col":"key","pivot_col":"key","value_col":"a","mapping":{"1":"a","2":"copy"}})).unwrap(), &limits).unwrap();
        assert_eq!(primary(&pivot), None);
    }
    let transposed = reshape::transpose(
        &a.project(&[1]).unwrap(),
        &serde_json::from_value(json!({})).unwrap(),
        &limits,
    )
    .unwrap();
    assert_eq!(primary(&transposed), None);
    // Unnest non puo' introdurre geometrie: i figli geometrici sono rifiutati.
    let child = Arc::new(geometry_output_field("nested", "EPSG:3857").unwrap());
    let structure = StructArray::new(
        vec![child.clone()].into(),
        vec![Arc::new(BinaryArray::from(vec![None::<&[u8]>, None])) as ArrayRef],
        None,
    );
    let nested = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            a.schema().field(1).clone(),
            Field::new("s", DataType::Struct(vec![child].into()), false),
        ])),
        vec![a.column(1).clone(), Arc::new(structure)],
    )
    .unwrap();
    for rows in [0, 2] {
        let nested = nested.slice(0, rows);
        assert_eq!(
            reshape::unnest(
                &nested,
                &serde_json::from_value(json!({"column":"s"})).unwrap(),
                &limits
            )
            .unwrap_err()
            .category(),
            ErrorCategory::Crs
        );
        let plan = plenora_pipeline::Pipeline::from_json(&json!({"version":1,"inputs":["t"],"steps":[{"op":"table.unnest","in":["t"],"out":"o","config":{"column":"s"}}],"outputs":["o"]}).to_string()).unwrap();
        assert_eq!(
            plan.validate(&[("t", nested.schema())])
                .unwrap_err()
                .category(),
            ErrorCategory::Crs
        );
    }
    // Join da un solo ingresso geometrico conserva la primaria senza ID.
    let plain = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "label",
            DataType::Utf8,
            false,
        )])),
        vec![Arc::new(StringArray::from(vec!["x"]))],
    )
    .unwrap();
    for (left, right) in [(&a, &plain), (&plain, &a)] {
        let joined = joins::cross_join(
            left,
            right,
            &serde_json::from_value(json!({})).unwrap(),
            &limits,
        )
        .unwrap();
        assert_eq!(primary(&joined), Some("a".into()));
    }
}

#[test]
fn concat_per_nome_esplicita_primaria_implicita_senza_id() {
    use plenora_core::contract::arrow_schema::primary_geometry_position;
    use plenora_kernels_table::{joins, Limits};
    use serde_json::json;
    let mut old = batch(1, None);
    let mut field = old.schema().field(0).clone();
    let mut metadata = field.metadata().clone();
    metadata.remove("plenora.field_id");
    field = field.with_metadata(metadata);
    old = RecordBatch::try_new(Arc::new(Schema::new(vec![field])), old.columns().to_vec()).unwrap();
    let multi = batch(2, Some("0"));
    for reverse in [false, true] {
        let inputs = if reverse {
            [&multi, &old]
        } else {
            [&old, &multi]
        };
        let direct = joins::concat_by_name(
            &inputs,
            &serde_json::from_value(json!({})).unwrap(),
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(
            primary_geometry_position(&direct.schema()).unwrap(),
            Some(0)
        );
        assert_eq!(
            direct.schema().metadata()["plenora.primary_geometry"],
            direct.schema().field(0).metadata()["plenora.field_id"]
        );
        let plan = plenora_pipeline::Pipeline::from_json(&json!({"version":1,"inputs":["a","b"],"steps":[{"op":"table.concat_by_name","in":["a","b"],"out":"o","config":{}}],"outputs":["o"]}).to_string()).unwrap();
        let result = plan
            .validate(&[("a", inputs[0].schema()), ("b", inputs[1].schema())])
            .unwrap()
            .run(vec![
                ("a".into(), inputs[0].clone()),
                ("b".into(), inputs[1].clone()),
            ])
            .unwrap();
        assert_eq!(
            primary_geometry_position(&result.outputs[0].1.schema()).unwrap(),
            Some(0)
        );
    }
}
