//! WKT2 ai confini pubblici: numeri esatti, assi e definizioni reali.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod comune;

use std::{collections::HashMap, sync::Arc};

use plenora_core::arrow::array::{BinaryArray, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::arrow_metadata::*;
use plenora_core::contract::arrow_schema::{
    contract_from_arrow_schema, verify_declared_coherence, GEOARROW_EXTENSION_METADATA_KEY,
};
use plenora_core::contract::{AxisOrder, ContractCrs, CrsDefinitionFormat};
use plenora_core::crs::resolve_crs;
use plenora_io::{leggi_tabella, scrivi_tabella, OpzioniScrittura};
use serde_json::json;

fn wkt(id: &str) -> String {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../plenora-core/src/crs/definizioni/wkt_corpus.json"
    ))
    .unwrap();
    corpus[id]["wkt2_2019"].as_str().unwrap().into()
}

fn campo(testo: &str, forma: u8, assi: &str) -> Field {
    let mut m = HashMap::from([
        (GEOARROW_EXTENSION_KEY.into(), GEOARROW_WKB_EXTENSION.into()),
        (PLENORA_GEOMETRY_AXIS_ORDER_KEY.into(), assi.into()),
    ]);
    m.insert(PLENORA_GEOMETRY_DIMENSIONS_KEY.into(), "xy".into());
    match forma {
        0 => {
            m.insert(PLENORA_GEOMETRY_CRS_DEFINITION_KEY.into(), testo.into());
            m.insert(
                PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY.into(),
                "wkt2".into(),
            );
        }
        1 => {
            m.insert(GEO_METADATA_KEY.into(), json!({"crs":testo}).to_string());
        }
        2 | 3 => {
            let mut metadati = json!({"crs":testo});
            if forma == 3 {
                metadati["crs_type"] = json!("wkt2:2019");
            }
            m.insert(GEOARROW_EXTENSION_METADATA_KEY.into(), metadati.to_string());
        }
        _ => unreachable!(),
    }
    Field::new("geometry", DataType::Binary, false).with_metadata(m)
}

#[test]
fn round_trip_ipc_e_geoparquet_non_scambiano_ordinate() {
    for (id, x, y, assi) in [
        ("EPSG:4326", 12.5_f64, 45.25_f64, "lon_lat"),
        ("OGC:CRS84", 12.5, 45.25, "lon_lat"),
        ("EPSG:6707", 500_123.0, 4_987_654.0, "easting_northing"),
        ("EPSG:7791", 500_123.0, 4_987_654.0, "easting_northing"),
    ] {
        for forma in 0..4 {
            let testo = wkt(id);
            let schema = Arc::new(schema(campo(&testo, forma, assi)));
            let mut wkb = vec![1];
            wkb.extend(1_u32.to_le_bytes());
            wkb.extend(x.to_le_bytes());
            wkb.extend(y.to_le_bytes());
            let batch = RecordBatch::try_new(
                schema,
                vec![Arc::new(BinaryArray::from_vec(vec![wkb.as_slice()]))],
            )
            .unwrap();
            let dir = comune::cartella();
            for ext in ["arrow", "parquet"] {
                let p = dir.path().join(format!("punti.{ext}"));
                scrivi_tabella(&batch, &p, &OpzioniScrittura::default()).unwrap();
                let letto = leggi_tabella(&p, None, u64::MAX).unwrap();
                assert_eq!(
                    letto
                        .column(0)
                        .as_any()
                        .downcast_ref::<BinaryArray>()
                        .unwrap()
                        .value(0),
                    wkb
                );
                let c = contract_from_arrow_schema(letto.schema(), resolve_crs).unwrap();
                assert!(c.geometries[0]
                    .crs
                    .as_resolved()
                    .unwrap()
                    .semantically_equals(&resolve_crs(id, "crs").unwrap()));
                if ext == "arrow" {
                    assert_eq!(
                        c.geometries[0].crs.as_resolved().unwrap().definition(),
                        testo
                    );
                }
            }
        }
    }
}

#[test]
fn crs84_coerente_e_assi_unknown_non_inferiti() {
    let t = wkt("OGC:CRS84");
    let d = resolve_crs(&t, "crs").unwrap();
    assert!(matches!(
        verify_declared_coherence(
            d.clone(),
            "OGC:CRS84",
            &t,
            Some(CrsDefinitionFormat::Wkt2),
            resolve_crs
        ),
        ContractCrs::Resolved(_)
    ));
    assert!(matches!(
        verify_declared_coherence(
            d,
            "EPSG:4326",
            &t,
            Some(CrsDefinitionFormat::Wkt2),
            resolve_crs
        ),
        ContractCrs::DeclaredUnresolved { .. }
    ));
    let f = campo(&t, 0, "unknown");
    assert_eq!(
        read_geometry_contract_keys(&f).unwrap().axis_order,
        Some(AxisOrder::Unknown)
    );
}

#[test]
fn geoarrow_formato_coerente_e_mutazioni_rifiutate() {
    let t = wkt("EPSG:32632");
    for metadati in [
        json!({"crs":t, "crs_type":"projjson"}),
        json!({"crs":{}, "crs_type":"wkt2:2019"}),
        json!({"crs":t, "crs_type":"SENTINELLA"}),
        json!({"crs":t, "crs_type":null}),
        json!({"crs":42}),
    ] {
        let f = campo(&t, 2, "easting_northing");
        let mut m = f.metadata().clone();
        m.insert(GEOARROW_EXTENSION_METADATA_KEY, metadati.to_string());
        let errore = read_geometry_contract_keys(&f.with_metadata(m)).unwrap_err();
        assert!(matches!(errore, plenora_core::PlenoraError::Unsupported(_)));
        assert!(!errore.to_string().contains("SENTINELLA"));
    }
    for forma in [2, 3] {
        let f = campo(
            &t.replace("500000", "500000.00000000001"),
            forma,
            "easting_northing",
        );
        let errore = read_geometry_contract_keys(&f).unwrap_err();
        assert_eq!(errore.public_projection().code(), Some("CRS_NOT_BUILTIN"));
        let f = campo(&t, forma, "easting_northing");
        let keys = read_geometry_contract_keys(&f).unwrap();
        assert_eq!(keys.crs_definition.as_deref(), Some(t.as_str()));
        assert_eq!(keys.crs_definition_format, Some(CrsDefinitionFormat::Wkt2));
    }
}

fn schema(f: Field) -> Schema {
    Schema::new(vec![f]).with_metadata(HashMap::from([(
        PLENORA_CONTRACT_VERSION_KEY.into(),
        "1".into(),
    )]))
}

#[test]
fn target_reproject_wkt2_uguale_al_codice_anche_da_geoarrow() {
    use plenora_io::{esegui_in_memoria, Ingresso};
    use plenora_pipeline::Interruzione;
    use plenora_pipeline::{Passo, Pipeline};
    let t = wkt("EPSG:4326");
    let mut wkb = vec![1];
    wkb.extend(1_u32.to_le_bytes());
    wkb.extend(12.5_f64.to_le_bytes());
    wkb.extend(45.25_f64.to_le_bytes());
    let batch = RecordBatch::try_new(
        Arc::new(schema(campo(&t, 2, "lon_lat"))),
        vec![Arc::new(BinaryArray::from_vec(vec![wkb.as_slice()]))],
    )
    .unwrap();
    let esegui = |target: String| {
        let p = Pipeline {
            version: 1,
            inputs: vec!["t".into()],
            crs: None,
            limits: None,
            steps: vec![Passo {
                out: "r".into(),
                op: "geo.reproject".into(),
                inputs: vec!["t".into()],
                config: json!({"target_crs":target}),
            }],
            outputs: vec!["r".into()],
        };
        esegui_in_memoria(
            &p,
            vec![Ingresso::Tabella {
                nome: "t".into(),
                tabella: batch.clone(),
            }],
            &Interruzione::default(),
        )
    };
    let a = esegui("EPSG:32632".into()).unwrap();
    let b = esegui(wkt("EPSG:32632")).unwrap();
    assert_eq!(a.outputs[0].1.columns(), b.outputs[0].1.columns());
    for esito in [&a, &b] {
        let c = contract_from_arrow_schema(esito.outputs[0].1.schema(), resolve_crs).unwrap();
        assert!(c.geometries[0]
            .crs
            .as_resolved()
            .unwrap()
            .semantically_equals(&resolve_crs("EPSG:32632", "crs").unwrap()));
    }
    let mutato = wkt("EPSG:32632").replace("500000", "500001");
    assert_eq!(
        esegui(mutato).unwrap_err().public_projection().code(),
        Some("CRS_NOT_BUILTIN")
    );
}
