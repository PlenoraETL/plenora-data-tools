//! Vettori Arrow Geometry Semantics al pin adottato, sulle API pubbliche.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use plenora_core::arrow::{
    array::{ArrayRef, BinaryArray, Int64Array, LargeBinaryArray, RecordBatch, StringArray},
    schema::{DataType, Field, Schema},
};
use plenora_io::Ingresso;
use plenora_pipeline::{Interruzione, Pipeline};
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::Arc};

fn mappa(v: &Value) -> HashMap<String, String> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
        .collect()
}
fn tabella(v: &Value) -> RecordBatch {
    let rows = v["rows"].as_array().unwrap();
    let mut fields = Vec::new();
    let mut arrays: Vec<ArrayRef> = Vec::new();
    for f in v["fields"].as_array().unwrap() {
        let name = f["name"].as_str().unwrap();
        let (dt, a): (DataType, ArrayRef) = match f["type"].as_str().unwrap() {
            "int64" => (
                DataType::Int64,
                Arc::new(Int64Array::from(
                    rows.iter().map(|r| r[name].as_i64()).collect::<Vec<_>>(),
                )),
            ),
            "utf8" => (
                DataType::Utf8,
                Arc::new(StringArray::from(
                    rows.iter().map(|r| r[name].as_str()).collect::<Vec<_>>(),
                )),
            ),
            t @ ("binary" | "large_binary") => {
                let bytes: Vec<Option<Vec<u8>>> = rows
                    .iter()
                    .map(|r| {
                        r[name].as_str().map(|s| {
                            (0..s.len())
                                .step_by(2)
                                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                                .collect()
                        })
                    })
                    .collect();
                let refs: Vec<_> = bytes.iter().map(|b| b.as_deref()).collect();
                if t == "binary" {
                    (DataType::Binary, Arc::new(BinaryArray::from(refs)))
                } else {
                    (
                        DataType::LargeBinary,
                        Arc::new(LargeBinaryArray::from(refs)),
                    )
                }
            }
            _ => panic!("tipo di fixture non supportato"),
        };
        fields.push(
            Field::new(name, dt, f["nullable"].as_bool().unwrap())
                .with_metadata(mappa(&f["metadata"])),
        );
        arrays.push(a);
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            mappa(&v["schema_metadata"]),
        )),
        arrays,
    )
    .unwrap()
}

fn definizione_integrata(name: &str) -> bool {
    // Elenco esplicito indipendente dal risolutore: queste definizioni
    // del corpus coincidono interamente con la tabella integrata.
    matches!(
        name,
        "crs-proj-epsg-4326-projjson.json"
            | "crs-proj-epsg-3003-wkt2.json"
            | "crs-proj-epsg-32632-wkt2.json"
            | "crs-proj-epsg-3857-wkt2.json"
            | "crs-proj-epsg-4326-wkt2.json"
    )
}

fn expected<'a>(v: &'a Value, name: &str, compute: bool) -> (&'a str, &'static str, bool) {
    let geometry = v["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["metadata"]["ARROW:extension:name"] == "geoarrow.wkb")
        .collect::<Vec<_>>();
    let value_defect = matches!(
        name,
        "invalid-dimensions-differ.json"
            | "invalid-ewkb-srid-differs.json"
            | "invalid-ewkb-srid-on-member.json"
            | "invalid-ewkb-srid-only-on-member.json"
            | "invalid-ewkb-srid-without-field-srid.json"
            | "invalid-precedence-first-row.json"
            | "invalid-type-outside-declaration.json"
            | "invalid-wkb-truncated.json"
            | "invalid-wkb-with-srid-flag.json"
    );
    if name.starts_with("invalid-contract-version")
        || name == "invalid-precedence-version-before-crs.json"
    {
        return (v["category"].as_str().unwrap(), "validate", false);
    }
    if geometry.len() > 1 {
        return ("unsupported", "validate", false);
    }
    if geometry
        .iter()
        .any(|f| f["metadata"]["plenora.geometry.spatial_semantics"] == "geography")
    {
        // Il profilo permette il passaggio; il componente lo rifiuta ancora.
        return ("unsupported", "validate", !compute);
    }
    if name == "invalid-unknown-crs-and-truncated.json" {
        return ("crs", "validate", !compute);
    }
    if name == "invalid-precedence-vocabulary-before-crs.json" {
        // DT-ARROW-003 completa encoding, resta il CRS contraddittorio.
        return ("crs", "validate", false);
    }
    if v["expect"] == "invalid"
        && !value_defect
        && !matches!(
            name,
            "invalid-axis-order-missing.json" | "invalid-precision-missing.json"
        )
    {
        return (v["category"].as_str().unwrap(), "validate", false);
    }
    if (!definizione_integrata(name)
        && geometry.iter().any(|f| {
            f["metadata"]
                .get("plenora.geometry.crs_definition")
                .is_some()
        }))
        || matches!(
            name,
            "axis-other-polar.json" | "crs-unknown-identifier.json"
        )
    {
        // Deviazioni GEO-000/004/006/015/017/019: risolutore solo integrato.
        return (
            "crs",
            "validate",
            !compute || v["computation"] == "accepted",
        );
    }
    if !compute {
        return ("ok", "", false);
    }
    if name == "crs-proj-epsg-3003-wkt2.json" {
        // Monte Mario risolve, ma il cambio datum verso WGS84 non e' supportato.
        return ("crs", "validate", v["computation"] == "accepted");
    }
    if geometry
        .iter()
        .any(|f| f["metadata"]["plenora.geometry.crs_resolution"] == "declared_unresolved")
        || matches!(
            name,
            "axis-lat-lon-stored.json" | "invalid-axis-order-missing.json"
        )
    {
        return ("crs", "validate", false);
    }
    if geometry
        .iter()
        .any(|f| f["metadata"]["plenora.geometry.dimensions"] != "xy")
    {
        return ("unsupported", "validate", false);
    }
    if value_defect {
        return (v["category"].as_str().unwrap(), "read", false);
    }
    if name == "ewkb-srid-matches-field.json" {
        return ("unsupported", "validate", true);
    }
    ("ok", "", false)
}

fn same_values(input: &RecordBatch, output: &RecordBatch) {
    assert_eq!(input.num_rows(), output.num_rows());
    assert_eq!(input.num_columns(), output.num_columns());
    for (left, right) in input.columns().iter().zip(output.columns()) {
        assert_eq!(
            left.to_data(),
            right.to_data(),
            "passaggio: valori alterati"
        );
    }
    for (left, right) in input.schema().fields().iter().zip(output.schema().fields()) {
        for (key, value) in left.metadata() {
            assert_eq!(
                right.metadata().get(key),
                Some(value),
                "passaggio: metadato alterato"
            );
        }
    }
}

#[test]
fn vettori_geometry_semantics() {
    use plenora_core::{RemoteEffect, RetryDisposition};
    use sha2::{Digest, Sha256};
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/arrow-data-v1");
    let provenance: Value =
        serde_json::from_slice(&std::fs::read(dir.join("provenienza.json")).unwrap()).unwrap();
    let adoption: Value = serde_json::from_str(include_str!("../adozione.json")).unwrap();
    assert_eq!(
        provenance["commit"],
        adoption["contracts_source"]["revision"]
    );
    assert_eq!(provenance["file"].as_object().unwrap().len(), 86);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 87);
    let mut conformi = 0;
    let mut deviazioni = 0;
    for (name, hash) in provenance["file"].as_object().unwrap() {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        assert_eq!(
            plenora_core::esadecimale::esadecimale(&Sha256::digest(&bytes)),
            hash.as_str().unwrap()
        );
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        for compute in [false, true] {
            let (steps, output) = if compute {
                (
                    r#"[{"out":"o","op":"geo.reproject","in":["t"],"config":{"target_crs":"EPSG:3857"}}]"#,
                    "o",
                )
            } else {
                ("[]", "t")
            };
            let plan =
                format!(r#"{{"version":1,"inputs":["t"],"steps":{steps},"outputs":["{output}"]}}"#);
            let p = Pipeline::from_json(&plan).unwrap();
            let input = tabella(&v);
            let (category, phase, deviation) = expected(&v, name, compute);
            if deviation {
                deviazioni += 1;
            } else {
                conformi += 1;
            }
            let result = plenora_cli::api::esegui_in_memoria(
                &p,
                vec![Ingresso::Tabella {
                    nome: "t".into(),
                    tabella: input.clone(),
                }],
                &Interruzione::default(),
            );
            match result {
                Ok((_, tables)) => {
                    assert_eq!(category, "ok", "{name}, calcolo={compute}");
                    if compute {
                        verifica_calcolo(&tables[0].1, name);
                    } else {
                        same_values(&input, &tables[0].1);
                    }
                }
                Err(e) => {
                    assert_eq!(e.category().as_str(), category, "{name}, calcolo={compute}");
                    assert_eq!(e.phase().as_str(), phase, "{name}, calcolo={compute}");
                    assert_eq!(e.remote_effect(), RemoteEffect::None);
                    assert_eq!(e.retry_disposition(), RetryDisposition::Never);
                }
            }
            verifica_cli(&input, &plan, category, phase, name, compute);
        }
    }
    println!("86 vettori, API e CLI: {conformi} scenari conformi, {deviazioni} con deviazione dichiarata");
}

#[test]
fn il_filtro_precede_la_lettura_dei_valori_e_srid_malformato_e_schema() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/arrow-data-v1/invalid-ewkb-srid-without-field-srid.json");
    let v: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let p = Pipeline::from_json(
        r#"{"version":1,"inputs":["t"],"steps":[
        {"out":"empty","in":["t"],"op":"table.limit","config":{"n":0}},
        {"out":"o","in":["empty"],"op":"geo.reproject","config":{"target_crs":"EPSG:3857"}}
    ],"outputs":["o"]}"#,
    )
    .unwrap();
    let (_, out) = plenora_cli::api::esegui_in_memoria(
        &p,
        vec![Ingresso::Tabella {
            nome: "t".into(),
            tabella: tabella(&v),
        }],
        &Interruzione::default(),
    )
    .unwrap();
    assert_eq!(out[0].1.num_rows(), 0);
    let mut bad = v;
    bad["fields"][1]["metadata"]["plenora.geometry.srid"] = Value::String("NON_UN_NUMERO".into());
    let p =
        Pipeline::from_json(r#"{"version":1,"inputs":["t"],"steps":[],"outputs":["t"]}"#).unwrap();
    let e = plenora_cli::api::esegui_in_memoria(
        &p,
        vec![Ingresso::Tabella {
            nome: "t".into(),
            tabella: tabella(&bad),
        }],
        &Interruzione::default(),
    )
    .unwrap_err();
    assert_eq!(e.category().as_str(), "schema");
    assert!(!e.to_string().contains("NON_UN_NUMERO"));
}

fn verifica_cli(
    input: &RecordBatch,
    plan: &str,
    category: &str,
    phase: &str,
    name: &str,
    compute: bool,
) {
    // Stessi vettori sul confine CLI: IPC da produttore esterno,
    // uscita assente a ogni rifiuto, assi dell'errore e exit code.
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("input.arrow");
    plenora_io::ipc::scrivi(input, std::fs::File::create(&source).unwrap()).unwrap();
    let plan_path = tmp.path().join("plan.json");
    std::fs::write(&plan_path, plan).unwrap();
    let target = tmp.path().join("output.arrow");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_plenora-data"))
        .args([
            "run",
            "--plan",
            plan_path.to_str().unwrap(),
            "--input",
            &format!("t={}", source.display()),
            "--output",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(run.stderr.is_empty());
    let doc: Value = serde_json::from_slice(&run.stdout).unwrap();
    if category == "ok" {
        assert_eq!(run.status.code(), Some(0), "{name}, calcolo={compute}");
        let out = plenora_io::leggi_tabella(&target, None, u64::MAX).unwrap();
        if compute {
            verifica_calcolo(&out, name);
        } else {
            same_values(input, &out);
        }
    } else {
        assert_eq!(run.status.code(), Some(3), "{name}, calcolo={compute}");
        assert!(!target.exists());
        assert_eq!(
            doc["error"]["category"], category,
            "{name}, calcolo={compute}"
        );
        assert_eq!(doc["error"]["phase"], phase, "{name}, calcolo={compute}");
        assert_eq!(doc["error"]["remote_effect"], "none");
        assert_eq!(doc["error"]["retry"]["kind"], "never");
    }
}

#[test]
fn legacy_senza_prova_degli_assi_resta_unknown_e_identita_visibile() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/arrow-data-v1/axis-lon-lat-epsg4326.json");
    let base: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let p =
        Pipeline::from_json(r#"{"version":1,"inputs":["t"],"steps":[],"outputs":["t"]}"#).unwrap();
    let run = |v: &Value| {
        plenora_cli::api::esegui_in_memoria(
            &p,
            vec![Ingresso::Tabella {
                nome: "t".into(),
                tabella: tabella(v),
            }],
            &Interruzione::default(),
        )
        .unwrap()
        .1
        .remove(0)
        .1
    };
    let mut legacy = base.clone();
    legacy["fields"][1]["metadata"] = serde_json::json!({
        "ARROW:extension:name":"geoarrow.wkb",
        "plenora.field_id":"1",
        "geo":r#"{"crs":"EPSG:4326","dimensions":"xy"}"#
    });
    let out = run(&legacy);
    assert_eq!(
        out.schema()
            .field(1)
            .metadata()
            .get("plenora.geometry.axis_order")
            .map(String::as_str),
        Some("unknown"),
        "GEO-001: il solo identificatore CRS non prova l ordine fisico"
    );
    let mut ids = base;
    ids["fields"][0]["metadata"]["plenora.field_id"] = Value::String("8".into());
    ids["fields"][1]["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("plenora.field_id");
    let out = run(&ids);
    assert_eq!(
        out.schema()
            .field(1)
            .metadata()
            .get("plenora.field_id")
            .map(String::as_str),
        Some("9"),
        "deviazione GEO-013: il minimo libero sarebbe 0"
    );
}

fn verifica_calcolo(output: &RecordBatch, name: &str) {
    let schema = output.schema();
    let field = schema.field_with_name("geometry").unwrap();
    assert_eq!(
        field
            .metadata()
            .get("plenora.geometry.axis_order")
            .map(String::as_str),
        Some("easting_northing")
    );
    assert_eq!(
        field
            .metadata()
            .get("plenora.geometry.crs_id")
            .map(String::as_str),
        Some("EPSG:3857")
    );
    if name == "axis-lon-lat-epsg4326.json" {
        let cells = output
            .column_by_name("geometry")
            .unwrap()
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let bytes = cells.value(0);
        assert_eq!(&bytes[..5], &[1, 1, 0, 0, 0]);
        let x = f64::from_le_bytes(bytes[5..13].try_into().unwrap());
        let y = f64::from_le_bytes(bytes[13..21].try_into().unwrap());
        // Oracolo analitico Mercatore sferico, ordinate asimmetriche del
        // vettore al pin. Tolleranza geografica del repository: 1 cm.
        let expected_x = 6_378_137.0 * (-120.5_f64).to_radians();
        let expected_y = 6_378_137.0
            * (std::f64::consts::FRAC_PI_4 + 45.25_f64.to_radians() / 2.0)
                .tan()
                .ln();
        assert!((x - expected_x).abs() < 0.01);
        assert!((y - expected_y).abs() < 0.01);
    }
}

// GEO-001 e policy fail-closed: valori plausibili in entrambi gli ordini
// non costituiscono una prova della convenzione fisica.
#[test]
fn assi_ambigui_rifiutati_prima_dei_valori_su_api_e_cli() {
    use plenora_core::{RemoteEffect, RetryDisposition};
    use serde_json::json;
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/arrow-data-v1/axis-lon-lat-epsg4326.json");
    let base: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for coords in [
        [(60.0_f64, 10.0_f64), (60.0, 11.0)],
        [(10.0, 60.0), (11.0, 60.0)],
    ] {
        let mut bytes = vec![1_u8];
        bytes.extend(2_u32.to_le_bytes());
        bytes.extend(2_u32.to_le_bytes());
        for (x, y) in coords {
            bytes.extend(x.to_le_bytes());
            bytes.extend(y.to_le_bytes());
        }
        let hex = plenora_core::esadecimale::esadecimale(&bytes);
        for axis in [
            None,
            Some("unknown"),
            Some("lat_lon"),
            Some("easting_northing"),
            Some("northing_easting"),
        ] {
            let mut v = base.clone();
            v["fields"][1]["metadata"]["plenora.geometry.types"] = json!("linestring");
            let md = v["fields"][1]["metadata"].as_object_mut().unwrap();
            md.remove("plenora.geometry.axis_order");
            if let Some(axis) = axis {
                md.insert("plenora.geometry.axis_order".into(), json!(axis));
            }
            v["rows"] = json!([{"id":1,"geometry":hex}]);
            for op in [
                "geo.geodesic_line_length",
                "geo.geodesic_area",
                "geo.geodesic_distance",
                "geo.haversine_distance",
                "geo.bearing",
                "geo.reproject",
            ] {
                let plan = json!({"version":1,"inputs":["t"],"steps":[{"out":"o","in":["t"],"op":op,"config":{}}],"outputs":["o"]}).to_string();
                let p = Pipeline::from_json(&plan).unwrap();
                let input = tabella(&v);
                let err = plenora_cli::api::esegui_in_memoria(
                    &p,
                    vec![Ingresso::Tabella {
                        nome: "t".into(),
                        tabella: input.clone(),
                    }],
                    &Interruzione::default(),
                )
                .unwrap_err();
                assert_eq!(err.category().as_str(), "crs");
                assert_eq!(err.phase().as_str(), "validate");
                assert_eq!(err.remote_effect(), RemoteEffect::None);
                assert_eq!(err.retry_disposition(), RetryDisposition::Never);
                verifica_cli(&input, &plan, "crs", "validate", op, true);
            }
            let plan = Pipeline::from_json(r#"{"version":1,"inputs":["t"],"steps":[{"out":"o","in":["t"],"op":"geo.vertex_count","config":{}}],"outputs":["o"]}"#).unwrap();
            plenora_cli::api::esegui_in_memoria(
                &plan,
                vec![Ingresso::Tabella {
                    nome: "t".into(),
                    tabella: tabella(&v),
                }],
                &Interruzione::default(),
            )
            .unwrap();
        }
        let mut v = base.clone();
        v["fields"][1]["metadata"]["plenora.geometry.types"] = json!("linestring");
        v["rows"] = json!([{"id":1,"geometry":hex}]);
        let plan = Pipeline::from_json(r#"{"version":1,"inputs":["t"],"steps":[{"out":"o","in":["t"],"op":"geo.geodesic_line_length","config":{}}],"outputs":["o"]}"#).unwrap();
        let (_, output) = plenora_cli::api::esegui_in_memoria(
            &plan,
            vec![Ingresso::Tabella {
                nome: "t".into(),
                tabella: tabella(&v),
            }],
            &Interruzione::default(),
        )
        .unwrap();
        let batch = &output[0].1;
        let value = batch
            .column(batch.num_columns() - 1)
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::Float64Array>()
            .unwrap()
            .value(0);
        // Due interpretazioni esplicite diverse: circa 111 km e 56 km.
        let expected = if coords[0].0.to_bits() == 60.0_f64.to_bits() {
            110_611.186_562_063_13
        } else {
            55_799.470_393_260_4
        };
        assert!((value - expected).abs() < 0.01, "{value}");
    }
}

#[test]
fn sonde_interop_assi_precedenza_e_centroid_geografico() {
    // Sonde dei casi nominati nel referto, non identificativi della matrice
    // originale non disponibile. Centroid richiede un CRS proiettato.
    for name in [
        "axis-lat-lon-stored.json",
        "invalid-wkb-truncated.json",
        "invalid-ewkb-srid-differs.json",
        "invalid-type-outside-declaration.json",
        "invalid-unknown-crs-and-truncated.json",
        "invalid-precedence-vocabulary-before-crs.json",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/arrow-data-v1")
            .join(name);
        let v: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let plan = r#"{"version":1,"inputs":["t"],"steps":[{"out":"o","in":["t"],"op":"geo.centroid","config":{}}],"outputs":["o"]}"#;
        let p = Pipeline::from_json(plan).unwrap();
        let input = tabella(&v);
        let error = plenora_cli::api::esegui_in_memoria(
            &p,
            vec![Ingresso::Tabella {
                nome: "t".into(),
                tabella: input.clone(),
            }],
            &Interruzione::default(),
        )
        .unwrap_err();
        assert_eq!(error.category().as_str(), "crs", "{name}");
        assert_eq!(error.phase().as_str(), "validate", "{name}");
        verifica_cli(&input, plan, "crs", "validate", name, true);
    }
}

#[test]
fn operazioni_strutturali_non_interpretano_il_dominio_geografico() {
    use serde_json::json;
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/arrow-data-v1/axis-lon-lat-epsg4326.json");
    let base: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut bytes = vec![1_u8];
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend(45.0_f64.to_le_bytes());
    bytes.extend(120.0_f64.to_le_bytes());
    for axis in ["unknown", "lat_lon"] {
        let mut v = base.clone();
        v["fields"][1]["metadata"]["plenora.geometry.axis_order"] = json!(axis);
        v["rows"] = json!([{"id":1,"geometry":plenora_core::esadecimale::esadecimale(&bytes)}]);
        for op in [
            "geo.to_wkt",
            "geo.vertex_count",
            "geo.explode",
            "geo.collect",
            "geo.geometry_accessors",
            "geo.geometry_diagnostics",
        ] {
            let config = if op == "geo.collect" {
                json!({"group_by":["id"]})
            } else {
                json!({})
            };
            let plan = json!({"version":1,"inputs":["t"],"steps":[{"out":"o","in":["t"],"op":op,"config":config}],"outputs":["o"]}).to_string();
            let p = Pipeline::from_json(&plan).unwrap();
            plenora_cli::api::esegui_in_memoria(
                &p,
                vec![Ingresso::Tabella {
                    nome: "t".into(),
                    tabella: tabella(&v),
                }],
                &Interruzione::default(),
            )
            .unwrap_or_else(|e| panic!("{op}: {e}"));
        }
    }
}

#[test]
fn fusioni_geometriche_rifiutano_in_validazione_senza_effetti() {
    use plenora_core::{RemoteEffect, RetryDisposition};
    use serde_json::json;
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/arrow-data-v1/axis-lon-lat-epsg4326.json");
    let base: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for axis in ["lat_lon", "unknown"] {
        let mut other = base.clone();
        other["fields"][1]["metadata"]["plenora.geometry.axis_order"] = json!(axis);
        let mut wkb = vec![1, 1, 0, 0, 0];
        wkb.extend(45.0_f64.to_le_bytes());
        wkb.extend(12.0_f64.to_le_bytes());
        other["rows"] = json!([{"id":1,"geometry":plenora_core::esadecimale::esadecimale(&wkb)}]);
        for (op, config) in [
            ("table.concat", json!({})),
            ("table.concat_by_name", json!({"strict":true})),
            ("table.concat_by_name", json!({"strict":false})),
            ("table.union_distinct", json!({})),
            ("table.intersect", json!({})),
            ("table.except", json!({})),
        ] {
            let plan = json!({"version":1,"inputs":["a","b"],"steps":[{"out":"o","in":["a","b"],"op":op,"config":config}],"outputs":["o"]});
            let plan = Pipeline::from_json(&plan.to_string()).unwrap();
            let err = plenora_cli::api::esegui_in_memoria(
                &plan,
                vec![
                    Ingresso::Tabella {
                        nome: "a".into(),
                        tabella: tabella(&base),
                    },
                    Ingresso::Tabella {
                        nome: "b".into(),
                        tabella: tabella(&other),
                    },
                ],
                &Interruzione::default(),
            )
            .unwrap_err();
            assert_eq!(err.category().as_str(), "crs", "{op}");
            assert_eq!(err.phase().as_str(), "validate", "{op}");
            assert_eq!(err.remote_effect(), RemoteEffect::None);
            assert_eq!(err.retry_disposition(), RetryDisposition::Never);
        }
    }
}
