//! Regressioni di trasporto per dichiarazioni geometriche derivate obsolete.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod comune;

use plenora_core::arrow::{Field, RecordBatch, Schema};
use plenora_core::contract::arrow_metadata::*;
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::crs::resolve_crs;
use plenora_core::ErrorCategory;
use plenora_io::{leggi_tabella, scrivi_tabella, OpzioniScrittura};
use plenora_pipeline::Pipeline;
use serde_json::json;
use std::sync::Arc;

const NATIVE: &[&str] = &[
    "geoparquet.primary_column",
    "geoparquet.column",
    "geoparquet.version",
    "geoparquet.compatibilita",
    "geoparquet.edges",
    "geoparquet.orientation",
    "geoparquet.bbox",
    "geoparquet.epoch",
    "geoparquet.altre_colonne",
    "gpkg.geometry_type_name",
    "gpkg.z",
    "gpkg.m",
    "gpkg.rtree_index",
    "shp.shape_type",
    "shp.measure_no_data",
    "filegdb.ogr_geometry_type",
    "filegdb.geometry_field_count",
    "dxf.geometry_model",
    "dxf.z_inference",
    "csv.geometry_encoding",
    "xlsx.geometry_encoding",
    "futuro.non_interpretato",
];

fn sorgente() -> RecordBatch {
    let batch = comune::utm32_integrata();
    let fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| {
            let mut md = field.metadata().clone();
            md.insert("utente.nota".to_owned(), "conservata".to_owned());
            md.insert("driver.native", "conservata");
            if field.name() == "geometry" {
                // Testo letterale di GEO_CRS_KEY di IO-tools, non costante del codec.
                md.insert("crs".to_owned(), "EPSG:32632".to_owned());
                md.insert(
                    PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
                    plenora_io::crs_projjson::projjson_di("EPSG:32632")
                        .unwrap()
                        .to_string(),
                );
                md.insert(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "projjson");
                for suffix in NATIVE {
                    md.insert(
                        format!("plenora.geometry.native.{suffix}"),
                        "vecchio".to_owned(),
                    );
                }
                md.insert(PLENORA_GEOMETRY_SRID_KEY, "100001");
                md.insert("plenora.geometry.native.geoparquet.epoch", "2023.5");
                md.insert(
                    "plenora.geometry.native.geoparquet.orientation",
                    "counterclockwise",
                );
                md.insert("plenora.geometry.futura".to_owned(), "vecchio".to_owned());
                for key in ["bbox", "covering", "geometry_types"] {
                    md.insert(key.to_owned(), "vecchio".to_owned());
                }
                let mut geo: serde_json::Value =
                    serde_json::from_str(&md[GEO_METADATA_KEY]).unwrap();
                for key in [
                    "bbox",
                    "covering",
                    "epoch",
                    "geometry_types",
                    "precision",
                    "axis_order",
                ] {
                    geo[key] = json!("vecchio");
                }
                md.insert(GEO_METADATA_KEY.to_owned(), geo.to_string());
                md.insert("ARROW:extension:metadata".to_owned(), json!({
                "crs": plenora_io::crs_projjson::projjson_di("EPSG:32632").unwrap(),
                "crs_type": "projjson",
                "bbox": "vecchio", "covering": "vecchio", "epoch": "vecchio", "edges": "planar"
            }).to_string());
                md.insert(
                    PLENORA_GEOMETRY_PRECISION_KEY.to_owned(),
                    "native".to_owned(),
                );
            }
            field.as_ref().clone().with_metadata(md)
        })
        .collect();
    let mut md = batch.schema().metadata().clone();
    md.insert("utente.dataset".to_owned(), "conservato".to_owned());
    md.insert(
        "plenora.geometry.native.geoparquet.column".to_owned(),
        "vecchio".to_owned(),
    );
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, md)),
        batch.columns().to_vec(),
    )
    .unwrap()
}

#[test]
fn chiavi_derivate_non_sopravvivono_ai_tre_trasporti() {
    let dir = comune::cartella();
    for input_ext in ["arrow", "arrows", "parquet"] {
        let input_path = dir.path().join(format!("input.{input_ext}"));
        scrivi_tabella(&sorgente(), &input_path, &OpzioniScrittura::default()).unwrap();
        let input = leggi_tabella(&input_path, None, u64::MAX).unwrap();
        for (op, config, target) in [
            (
                "geo.reproject",
                json!({"target_crs":"EPSG:3857"}),
                "EPSG:3857",
            ),
            ("geo.snap_to_grid", json!({"grid_size":0.01}), "EPSG:32632"),
            ("geo.buffer", json!({"distance":1.0}), "EPSG:32632"),
            ("geo.explode", json!({}), "EPSG:32632"),
        ] {
            let plan = Pipeline::from_json(
                &json!({"version":1,"inputs":["t"],
                "steps":[{"out":"o","op":op,"in":["t"],"config":config}],"outputs":["o"]})
                .to_string(),
            )
            .unwrap();
            let result = plan
                .validate(&[("t", input.schema())])
                .unwrap()
                .run(vec![("t".to_owned(), input.clone())])
                .unwrap();
            for ext in ["arrow", "arrows", "parquet"] {
                let path = dir.path().join(format!("{input_ext}-{op}.{ext}"));
                scrivi_tabella(&result.outputs[0].1, &path, &OpzioniScrittura::default()).unwrap();
                let output = leggi_tabella(&path, None, u64::MAX).unwrap();
                let schema = output.schema();
                let field = schema.field_with_name("geometry").unwrap();
                let md = field.metadata();
                assert_eq!(md["utente.nota"], "conservata");
                assert_eq!(md["driver.native"], "conservata");
                assert_eq!(schema.metadata()["utente.dataset"], "conservato");
                for suffix in NATIVE {
                    assert!(
                        !md.contains_key(&format!("plenora.geometry.native.{suffix}")),
                        "{op}: {suffix}"
                    );
                }
                assert!(!md.contains_key("plenora.geometry.futura"));
                for key in ["bbox", "covering", "geometry_types"] {
                    assert!(!md.contains_key(key));
                }
                for key in [GEO_METADATA_KEY, "ARROW:extension:metadata"] {
                    assert!(
                        !md.get(key).is_some_and(|value| value.contains("vecchio")),
                        "{op}: {key}"
                    );
                }
                assert!(!schema.metadata().values().any(|v| v.contains("vecchio")));
                if op == "geo.reproject" {
                    assert!(!md.contains_key("crs"));
                    if let Some(raw) = md.get("ARROW:extension:metadata") {
                        let extension: serde_json::Value = serde_json::from_str(raw).unwrap();
                        assert!(extension.get("crs").is_none());
                        assert!(extension.get("crs_type").is_none());
                    }
                }
                let keys = read_geometry_contract_keys(field).unwrap();
                assert_eq!(
                    keys.precision.unwrap().as_str(),
                    if op == "table.filter" {
                        "native"
                    } else {
                        "float64"
                    }
                );
                if op == "geo.buffer" {
                    assert_eq!(md[PLENORA_GEOMETRY_TYPES_KEY], "multipolygon");
                }
                assert_eq!(
                    keys.srid,
                    Some(if op == "geo.reproject" { 3857 } else { 100_001 })
                );
                assert_eq!(keys.dimensions.unwrap().as_str(), "xy");
                assert_eq!(keys.encoding.unwrap().as_str(), "wkb");
                let contract = contract_from_arrow_schema(schema, resolve_crs).unwrap();
                assert!(contract.geometries[0]
                    .crs
                    .as_resolved()
                    .unwrap()
                    .semantically_equals(&resolve_crs(target, "crs").unwrap()));
            }
        }
    }
}

#[test]
fn crs_io_discordante_rifiutato_in_ingresso_senza_dati() {
    let missing_with_axes = Field::new("geometry", plenora_core::arrow::DataType::Binary, true)
        .with_metadata(std::collections::HashMap::from([
            (
                GEOARROW_EXTENSION_KEY.to_owned(),
                GEOARROW_WKB_EXTENSION.to_owned(),
            ),
            (
                "ARROW:extension:metadata".to_owned(),
                "{\"crs\":null}".to_owned(),
            ),
            (
                PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                "unknown".to_owned(),
            ),
        ]));
    assert_eq!(
        read_geometry_contract_keys(&missing_with_axes)
            .unwrap_err()
            .category(),
        ErrorCategory::Crs
    );
    for key in [
        PLENORA_GEOMETRY_CRS_ID_KEY,
        PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
        GEO_METADATA_KEY,
        "ARROW:extension:metadata",
    ] {
        let mut md = std::collections::HashMap::from([
            (
                GEOARROW_EXTENSION_KEY.to_owned(),
                GEOARROW_WKB_EXTENSION.to_owned(),
            ),
            ("crs".to_owned(), "EPSG:32632".to_owned()),
        ]);
        match key {
            GEO_METADATA_KEY => {
                md.insert(key.to_owned(), json!({"crs":"EPSG:3857"}).to_string());
            }
            "ARROW:extension:metadata" => {
                md.insert(
                    key.to_owned(),
                    json!({"crs":plenora_io::crs_projjson::projjson_di("EPSG:3857").unwrap()})
                        .to_string(),
                );
            }
            PLENORA_GEOMETRY_CRS_DEFINITION_KEY => {
                md.insert(
                    key.to_owned(),
                    plenora_io::crs_projjson::projjson_di("EPSG:3857")
                        .unwrap()
                        .to_string(),
                );
                md.insert(
                    PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY.to_owned(),
                    "projjson".to_owned(),
                );
                // L'ID concordante non deve nascondere la definizione discordante.
                md.insert(
                    PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(),
                    "EPSG:32632".to_owned(),
                );
            }
            _ => {
                md.insert(key.to_owned(), "EPSG:3857".to_owned());
            }
        }
        let field =
            Field::new("geometry", plenora_core::arrow::DataType::Binary, true).with_metadata(md);
        let err = read_geometry_contract_keys(&field).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Crs);
        assert!(!err.to_string().contains("EPSG"));
    }
}

#[test]
fn produttori_non_ereditano_riepiloghi_di_schema() {
    use plenora_core::arrow::array::{Float64Array, StringArray};
    use plenora_core::arrow::DataType;
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, false),
            Field::new("wkt", DataType::Utf8, false),
        ],
        std::collections::HashMap::from([
            ("geo".to_owned(), "obsoleto".to_owned()),
            ("bbox".to_owned(), "obsoleto".to_owned()),
            (
                "plenora.geometry.native.geoparquet.column".to_owned(),
                "obsoleto".to_owned(),
            ),
            ("plenora.contract.version".to_owned(), "1".to_owned()),
            ("utente.dataset".to_owned(), "conservato".to_owned()),
        ]),
    ));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![0.0])),
            Arc::new(Float64Array::from(vec![0.0])),
            Arc::new(StringArray::from(vec!["POINT(0 0)"])),
        ],
    )
    .unwrap();
    let dir = comune::cartella();
    for (op, config) in [
        ("geo.from_coords", json!({"crs":"EPSG:3857"})),
        (
            "geo.from_wkt",
            json!({"crs":"EPSG:3857","wkt_column":"wkt"}),
        ),
        (
            "geo.generate_grid",
            json!({"crs":"EPSG:3857","extent":{"xmin":0,"ymin":0,"xmax":1,"ymax":1},"cell_size":1}),
        ),
    ] {
        let plan = Pipeline::from_json(
            &json!({"version":1,"inputs":["t"],
            "steps":[{"out":"o","op":op,"in":["t"],"config":config}],"outputs":["o"]})
            .to_string(),
        )
        .unwrap();
        let result = plan
            .validate(&[("t", batch.schema())])
            .unwrap()
            .run(vec![("t".to_owned(), batch.clone())])
            .unwrap();
        for ext in ["arrow", "arrows", "parquet"] {
            let path = dir.path().join(format!("{op}.{ext}"));
            scrivi_tabella(&result.outputs[0].1, &path, &OpzioniScrittura::default()).unwrap();
            let output = leggi_tabella(&path, None, u64::MAX).unwrap();
            assert!(!output
                .schema()
                .metadata()
                .values()
                .any(|v| v.contains("obsoleto")));
            assert_eq!(output.schema().metadata()["utente.dataset"], "conservato");
            assert_eq!(
                read_geometry_contract_keys(output.schema().field_with_name("geometry").unwrap())
                    .unwrap()
                    .precision
                    .unwrap()
                    .as_str(),
                "float64"
            );
        }
    }
}

#[test]
fn conflitti_crs_nei_file_rifiutati_prima_del_piano() {
    use plenora_core::arrow::ipc::writer::{FileWriter, StreamWriter};
    use plenora_core::arrow::DataType;
    let dir = comune::cartella();
    let plan =
        Pipeline::from_json(r#"{"version":1,"inputs":["t"],"steps":[],"outputs":["t"]}"#).unwrap();
    for (n, extra) in [
        (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:3857".to_owned()),
        ("ARROW:extension:metadata", "{\"crs\":null}".to_owned()),
    ]
    .into_iter()
    .enumerate()
    {
        let md = std::collections::HashMap::from([
            (
                GEOARROW_EXTENSION_KEY.to_owned(),
                GEOARROW_WKB_EXTENSION.to_owned(),
            ),
            ("crs".to_owned(), "EPSG:32632".to_owned()),
            (extra.0.to_owned(), extra.1),
        ]);
        let schema = Arc::new(Schema::new_with_metadata(
            vec![Field::new("geometry", DataType::Binary, true).with_metadata(md)],
            canonical_schema_version_metadata(),
        ));
        let batch = plenora_core::batch_vuoto(schema.clone()).unwrap();
        for ext in ["arrow", "arrows", "parquet"] {
            let path = dir.path().join(format!("conflitto-{n}.{ext}"));
            let file = std::fs::File::create(&path).unwrap();
            match ext {
                "arrow" => {
                    let mut w = FileWriter::try_new(file, &schema).unwrap();
                    w.write(&batch).unwrap();
                    w.finish().unwrap();
                }
                "arrows" => {
                    let mut w = StreamWriter::try_new(file, &schema).unwrap();
                    w.write(&batch).unwrap();
                    w.finish().unwrap();
                }
                _ => {
                    let geo = json!({
                        "version":"1.1.0", "primary_column":"geometry",
                        "columns":{"geometry":{"encoding":"WKB", "geometry_types":[],
                            "crs":plenora_io::crs_projjson::projjson_di("EPSG:32632").unwrap()}}
                    });
                    let properties = parquet::file::properties::WriterProperties::builder()
                        .set_key_value_metadata(Some(vec![parquet::file::metadata::KeyValue {
                            key: "geo".to_owned(),
                            value: Some(geo.to_string()),
                        }]))
                        .build();
                    let mut w = parquet::arrow::ArrowWriter::try_new(
                        file,
                        schema.clone(),
                        Some(properties),
                    )
                    .unwrap();
                    w.write(&batch).unwrap();
                    w.close().unwrap();
                }
            }
            let error = match leggi_tabella(&path, None, u64::MAX) {
                Err(error) => error,
                Ok(batch) => plan.validate(&[("t", batch.schema())]).unwrap_err(),
            };
            assert_eq!(error.category(), ErrorCategory::Crs);
            assert!(!error.to_string().contains("EPSG"));
        }
    }
}

#[test]
fn colonne_invariate_conservano_le_dichiarazioni_native() {
    let input = sorgente();
    let dir = comune::cartella();
    for (op, config) in [
        ("", json!({})),
        (
            "table.select_columns",
            json!({"columns":["geometry", "id"]}),
        ),
        (
            "table.filter",
            json!({"column":"id","operator":">","value":1}),
        ),
        ("table.sort", json!({"columns":["id"]})),
    ] {
        let steps = if op.is_empty() {
            json!([])
        } else {
            json!([{"out":"o","op":op,"in":["t"],"config":config}])
        };
        let plan = Pipeline::from_json(
            &json!({"version":1,"inputs":["t"],
            "steps":steps,"outputs":[if op.is_empty() {"t"} else {"o"}]})
            .to_string(),
        )
        .unwrap();
        let result = plan
            .validate(&[("t", input.schema())])
            .unwrap()
            .run(vec![("t".to_owned(), input.clone())])
            .unwrap();
        for ext in ["arrow", "arrows", "parquet"] {
            let path = dir.path().join(format!("invariata-{op}.{ext}"));
            scrivi_tabella(&result.outputs[0].1, &path, &OpzioniScrittura::default()).unwrap();
            let restored = leggi_tabella(&path, None, u64::MAX).unwrap();
            let schema = restored.schema();
            let md = schema.field_with_name("geometry").unwrap().metadata();
            assert_eq!(md["plenora.geometry.native.geoparquet.epoch"], "2023.5");
            assert_eq!(
                md["plenora.geometry.native.geoparquet.orientation"],
                "counterclockwise"
            );
            assert_eq!(md[PLENORA_GEOMETRY_SRID_KEY], "100001");
        }
        let schema = result.outputs[0].1.schema();
        let field = schema.field_with_name("geometry").unwrap();
        let original = input.schema();
        let original = original.field_with_name("geometry").unwrap();
        for suffix in NATIVE {
            let key = format!("plenora.geometry.native.{suffix}");
            assert_eq!(
                field.metadata().get(&key),
                original.metadata().get(&key),
                "{op}: {key}"
            );
        }
        for key in [
            "geo",
            "ARROW:extension:metadata",
            "bbox",
            "covering",
            "geometry_types",
        ] {
            assert_eq!(
                field.metadata().get(key),
                original.metadata().get(key),
                "{op}: {key}"
            );
        }
    }
}

#[test]
fn fusione_con_epoche_diverse_rifiutata() {
    let left = sorgente();
    let schema = left.schema();
    let fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| {
            let mut md = field.metadata().clone();
            if field.name() == "geometry" {
                md.insert("plenora.geometry.native.geoparquet.epoch", "2025.0");
            }
            field.as_ref().clone().with_metadata(md)
        })
        .collect();
    let right = Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));
    for op in [
        "table.concat",
        "table.concat_by_name",
        "table.union_distinct",
    ] {
        let plan = Pipeline::from_json(
            &json!({"version":1,"inputs":["a", "b"],
            "steps":[{"out":"o","op":op,"in":["a","b"],"config":{}}],"outputs":["o"]})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            plan.validate(&[("a", schema.clone()), ("b", right.clone())])
                .unwrap_err()
                .category(),
            ErrorCategory::Schema
        );
    }
}
