//! PROJJSON ai confini pubblici: numeri esatti, assi e definizioni reali.

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
use plenora_io::crs_projjson::{identificatori, projjson_di};
use plenora_io::{leggi_tabella, scrivi_tabella, OpzioniScrittura};
use serde_json::json;

fn rifiutato(testo: &str, codice: &str) {
    let e = resolve_crs(testo, "crs").unwrap_err();
    assert_eq!(e.code().as_str(), codice);
    assert!(!e.to_string().contains("SENTINELLA"));
}

#[test]
fn corpus_intero_conserva_identita_domini_assi_e_testo() {
    let ids = identificatori().unwrap();
    assert_eq!(ids.len(), 169);
    for id in ids {
        let testo = projjson_di(&id).unwrap().to_string();
        let d = resolve_crs(&testo, "crs").unwrap();
        let i = resolve_crs(&id, "crs").unwrap();
        assert!(d.semantically_equals(&i), "{id}");
        assert_eq!(d.definition(), testo);
        assert_eq!(d.authority_axis_order(), i.authority_axis_order());
        assert_eq!(d.validity_domain(), i.validity_domain());
        assert_eq!(d.area_of_use(), i.area_of_use());
    }
}

#[test]
fn projjson_reale_estratto_dalla_fixture_arrow_io_tools() {
    let testo = include_str!("dati/io_tools_3003.json").trim();
    let d = resolve_crs(testo, "crs").unwrap();
    assert!(d.semantically_equals(&resolve_crs("EPSG:3003", "crs").unwrap()));
    assert_eq!(d.definition(), testo);
}

#[test]
fn datum_storico_io_tools_non_equivale_all_ensemble() {
    // PROJJSON_CRS84 di IO-tools, test definizione_crs_in_scrittura.rs:
    // il datum storico resta distinto, anche con lo stesso ellissoide.
    let storico = r#"{"type":"GeographicCRS","name":"WGS 84","datum":{"type":"GeodeticReferenceFrame","name":"World Geodetic System 1984","ellipsoid":{"name":"WGS 84","semi_major_axis":6378137,"inverse_flattening":298.257223563}},"coordinate_system":{"subtype":"ellipsoidal","axis":[{"name":"Geodetic longitude","abbreviation":"Lon","direction":"east","unit":"degree"},{"name":"Geodetic latitude","abbreviation":"Lat","direction":"north","unit":"degree"}]},"id":{"authority":"OGC","code":"CRS84"}}"#;
    rifiutato(storico, "CRS_NOT_BUILTIN");
}

#[test]
fn mutazioni_semantiche_singole_non_si_nascondono_dietro_id() {
    let originale = projjson_di("EPSG:32632").unwrap();
    for (path, value) in [
        ("/base_crs/id/code", json!(4326 + 1)),
        ("/base_crs/datum_ensemble/id/code", json!(6258)),
        ("/base_crs/datum_ensemble/accuracy", json!("1.0")),
        (
            "/base_crs/datum_ensemble/ellipsoid/inverse_flattening",
            json!(298.257_223_564),
        ),
        (
            "/base_crs/datum_ensemble/ellipsoid/semi_major_axis",
            json!(6_378_138),
        ),
        ("/conversion/method/id/code", json!(9808)),
        ("/conversion/parameters/0/value", json!(1)),
        ("/conversion/parameters/1/value", json!(10)),
        ("/conversion/parameters/2/value", json!(1)),
        ("/conversion/parameters/3/value", json!(500_001)),
        ("/conversion/parameters/4/value", json!(1)),
        ("/conversion/parameters/3/unit", json!("degree")),
        ("/coordinate_system/axis/0/direction", json!("north")),
        ("/coordinate_system/subtype", json!("ellipsoidal")),
        ("/id/code", json!(32631)),
    ] {
        let mut v = originale.clone();
        *v.pointer_mut(path).unwrap() = value;
        rifiutato(&v.to_string(), "CRS_NOT_BUILTIN");
    }
    for path in [
        "/conversion/parameters",
        "/base_crs/datum_ensemble/members",
        "/coordinate_system/axis",
    ] {
        let mut v = originale.clone();
        let a = v.pointer_mut(path).unwrap().as_array_mut().unwrap();
        a.push(a[0].clone());
        rifiutato(&v.to_string(), "CRS_NOT_BUILTIN");
    }
    for value in [
        json!({"name":"Greenwich","longitude":1}),
        json!({"name":"Greenwich","longitude":0,"id":{"authority":"EPSG","code":8902}}),
    ] {
        let mut v = originale.clone();
        v["base_crs"]["datum_ensemble"]["prime_meridian"] = value;
        rifiutato(&v.to_string(), "CRS_NOT_BUILTIN");
    }
    let mut v = originale;
    v["SENTINELLA"] = json!(true);
    rifiutato(&v.to_string(), "CRS_NOT_BUILTIN");
}

#[test]
fn forme_equivalenti_e_numeri_non_arrotondabili() {
    let originale = projjson_di("EPSG:32632").unwrap();
    for forma in ["500000.0", "5e5", "50000000e-2"] {
        let t = originale.to_string().replace("500000", forma);
        assert!(resolve_crs(&t, "crs")
            .unwrap()
            .semantically_equals(&resolve_crs("EPSG:32632", "crs").unwrap()));
    }
    for forma in ["500000.00000000001", "1e99999", "1e-99999"] {
        rifiutato(
            &originale.to_string().replace("500000", forma),
            "CRS_INVALID",
        );
    }
    let mut v = originale;
    let id = v.as_object_mut().unwrap().remove("id").unwrap();
    v["ids"] = json!([id]);
    resolve_crs(&v.to_string(), "crs").unwrap();
    v["id"] = id;
    rifiutato(&v.to_string(), "CRS_NOT_BUILTIN");
    for testo in ["{", "{\"id\":1,\"id\":2}", "{\"x\":NaN}"] {
        rifiutato(testo, "CRS_INVALID");
    }
    let profondo = format!("{{\"x\":{}0{}}}", "[".repeat(33), "]".repeat(33));
    rifiutato(&profondo, "CRS_INVALID");
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
                "projjson".into(),
            );
        }
        1 => {
            m.insert(GEO_METADATA_KEY.into(), format!("{{\"crs\":{testo}}}"));
        }
        _ => {
            m.insert(
                GEOARROW_EXTENSION_METADATA_KEY.into(),
                format!("{{\"crs\":{testo}}}"),
            );
        }
    }
    Field::new("geometry", DataType::Binary, false).with_metadata(m)
}

#[test]
fn arrotondamenti_rifiutati_prima_del_value_arrow() {
    let testo = projjson_di("EPSG:32632")
        .unwrap()
        .to_string()
        .replace("500000", "500000.00000000001");
    for forma in 0..3 {
        let schema = Arc::new(schema(campo(&testo, forma, "easting_northing")));
        assert_eq!(
            contract_from_arrow_schema(schema, resolve_crs)
                .unwrap_err()
                .public_projection()
                .code(),
            Some("CRS_INVALID")
        );
    }
    assert!(geo_metadata_json(&testo).is_err());
}

#[test]
fn round_trip_ipc_e_geoparquet_non_scambiano_ordinate() {
    for (id, x, y, assi) in [
        ("EPSG:4326", 12.5_f64, 45.25_f64, "lon_lat"),
        ("OGC:CRS84", 12.5, 45.25, "lon_lat"),
        ("EPSG:6707", 500_123.0, 4_987_654.0, "easting_northing"),
        ("EPSG:7791", 500_123.0, 4_987_654.0, "easting_northing"),
    ] {
        for forma in 0..3 {
            let testo = projjson_di(id).unwrap().to_string();
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
            }
        }
    }
}

#[test]
fn crs84_coerente_e_assi_unknown_non_inferiti() {
    let t = projjson_di("OGC:CRS84").unwrap().to_string();
    let d = resolve_crs(&t, "crs").unwrap();
    assert!(matches!(
        verify_declared_coherence(
            d.clone(),
            "OGC:CRS84",
            &t,
            Some(CrsDefinitionFormat::Projjson),
            resolve_crs
        ),
        ContractCrs::Resolved(_)
    ));
    assert!(matches!(
        verify_declared_coherence(
            d,
            "EPSG:4326",
            &t,
            Some(CrsDefinitionFormat::Projjson),
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

fn schema(f: Field) -> Schema {
    Schema::new(vec![f]).with_metadata(HashMap::from([(
        PLENORA_CONTRACT_VERSION_KEY.into(),
        "1".into(),
    )]))
}

#[test]
fn target_reproject_projjson_uguale_al_codice_anche_da_geoarrow() {
    use plenora_io::{esegui_in_memoria, Ingresso};
    use plenora_pipeline::Interruzione;
    use plenora_pipeline::{Passo, Pipeline};
    let t = projjson_di("EPSG:4326").unwrap().to_string();
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
    let b = esegui(projjson_di("EPSG:32632").unwrap().to_string()).unwrap();
    assert_eq!(a.outputs[0].1.columns(), b.outputs[0].1.columns());
    for esito in [&a, &b] {
        let c = contract_from_arrow_schema(esito.outputs[0].1.schema(), resolve_crs).unwrap();
        assert!(c.geometries[0]
            .crs
            .as_resolved()
            .unwrap()
            .semantically_equals(&resolve_crs("EPSG:32632", "crs").unwrap()));
    }
    let mut mutato = projjson_di("EPSG:32632").unwrap();
    mutato["conversion"]["parameters"][0]["value"] = json!(1);
    assert_eq!(
        esegui(mutato.to_string())
            .unwrap_err()
            .public_projection()
            .code(),
        Some("CRS_NOT_BUILTIN")
    );
}

#[test]
fn geoarrow_non_risolve_una_dichiarazione_esplicitamente_irrisolta() {
    let t = projjson_di("EPSG:4326").unwrap().to_string();
    let f = campo(&t, 2, "unknown");
    let mut m = f.metadata().clone();
    m.insert(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "declared_unresolved");
    m.insert(
        PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
        "GEOGCS[\"SENTINELLA\"]",
    );
    m.insert(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt");
    let c = contract_from_arrow_schema(Arc::new(schema(f.with_metadata(m))), resolve_crs).unwrap();
    assert!(matches!(
        c.geometries[0].crs,
        ContractCrs::DeclaredUnresolved { .. }
    ));
}
