//! Dichiarazione, capacita' e derivazione sono livelli distinti.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use plenora_core::arrow::{DataType, Field, Metadata, Schema};
use plenora_core::contract::arrow_metadata::{read_geometry_contract_keys, GEOARROW_CRS_FORMS};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::contract::ContractCrs;
use plenora_core::crs::{resolve_crs, CrsError, ResolvedCrs};
use std::sync::Arc;

const fn no_resolver(_: &str, _: &'static str) -> Result<ResolvedCrs, CrsError> {
    Err(CrsError::BackendUnavailable)
}

#[test]
fn forme_per_stato_presenza_e_accordo() {
    for form in GEOARROW_CRS_FORMS.iter().map(|row| row.0) {
        let corpus: serde_json::Value = serde_json::from_str(if form == "wkt2:2019" {
            include_str!("../src/crs/definizioni/wkt_corpus.json")
        } else {
            include_str!("../../plenora-io/data/projjson_integrati.json")
        })
        .unwrap();
        let definition = if form == "wkt2:2019" {
            corpus["EPSG:32632"]["wkt2_2019"]
                .as_str()
                .unwrap()
                .to_owned()
        } else {
            corpus["EPSG:32632"].to_string()
        };
        resolve_crs(&definition, "crs").unwrap();
        let definition = definition.as_str();
        for state in ["resolved", "declared_unresolved", "missing"] {
            for canonical in [false, true] {
                for extension in [false, true] {
                    for agrees in [false, true] {
                        let mut md = Metadata::new();
                        md.insert("ARROW:extension:name", "geoarrow.wkb");
                        if canonical {
                            md.insert("plenora.geometry.crs_resolution", state);
                            if state != "missing" {
                                md.insert(
                                    "plenora.geometry.crs_id",
                                    if agrees { "EPSG:32632" } else { "EPSG:4326" },
                                );
                                md.insert("plenora.geometry.axis_order", "easting_northing");
                                if form != "authority_code"
                                    && agrees
                                    && matches!(form, "projjson" | "wkt2:2019")
                                {
                                    md.insert("plenora.geometry.crs_definition", definition);
                                    md.insert(
                                        "plenora.geometry.crs_definition_format",
                                        if form == "projjson" {
                                            "projjson"
                                        } else {
                                            "wkt2"
                                        },
                                    );
                                }
                            }
                        }
                        let raw = match form {
                            "authority_code" => serde_json::json!({"crs_type":form,"crs":"EPSG:32632"}),
                            "projjson" => serde_json::json!({"crs_type":form,"crs":serde_json::from_str::<serde_json::Value>(definition).unwrap()}),
                            "wkt2:2019" => serde_json::json!({"crs_type":form,"crs":definition}),
                            "null" => serde_json::json!({"crs":null}),
                            _ => serde_json::json!({}),
                        }.to_string();
                        if extension {
                            md.insert("ARROW:extension:metadata", raw.clone());
                        }
                        let field = Field::new("g", DataType::Binary, true).with_metadata(md);
                        let result = read_geometry_contract_keys(&field);
                        let conflict = canonical
                            && extension
                            && form != "absent"
                            && if form == "null" {
                                state != "missing"
                            } else {
                                state == "missing" || !agrees
                            };
                        if conflict {
                            let err = result.expect_err("conflitto");
                            assert_eq!(err.category().as_str(), "crs");
                        } else {
                            result.unwrap_or_else(|error| {
                                panic!("{form}/{state}/{canonical}/{extension}/{agrees}: {error}")
                            });
                            if extension {
                                assert_eq!(
                                    field.metadata().get("ARROW:extension:metadata"),
                                    Some(&raw)
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn local_unresolved_non_promosso_dal_srid_o_dall_estensione() {
    let mut md = Metadata::new();
    for (key, value) in [
        ("ARROW:extension:name", "geoarrow.wkb"),
        ("plenora.geometry.crs_resolution", "declared_unresolved"),
        ("plenora.geometry.crs_id", "LOCAL:sentinel"),
        ("plenora.geometry.axis_order", "unknown"),
        ("plenora.geometry.srid", "4326"),
        (
            "ARROW:extension:metadata",
            r#"{ "crs_type":"authority_code", "crs":"LOCAL:sentinel" }"#,
        ),
    ] {
        md.insert(key, value);
    }
    let mut sm = Metadata::new();
    sm.insert("plenora.contract.version", "1");
    let schema = Arc::new(Schema::new_with_metadata(
        vec![Field::new("g", DataType::Binary, true).with_metadata(md)],
        sm,
    ));
    for resolver in [
        no_resolver as plenora_core::contract::arrow_schema::CrsResolver,
        resolve_crs,
    ] {
        let contract = contract_from_arrow_schema(schema.clone(), resolver).unwrap();
        assert!(matches!(
            contract.geometries[0].crs,
            ContractCrs::DeclaredUnresolved { .. }
        ));
        assert_eq!(
            contract.schema.field(0).metadata(),
            schema.field(0).metadata()
        );
    }
}

#[test]
fn autorita_concorde_case_e_definizione_esterna_senza_risoluzione() {
    for (id, definition) in [
        ("EPSG:32632", None),
        (
            "LOCAL:sentinel",
            Some(r#"{"type":"ProjectedCRS","id":{"authority":"LOCAL","code":"sentinel"}}"#),
        ),
    ] {
        let mut md = Metadata::new();
        md.insert("ARROW:extension:name", "geoarrow.wkb");
        md.insert("plenora.geometry.crs_resolution", "declared_unresolved");
        md.insert("plenora.geometry.axis_order", "unknown");
        if let Some(definition) = definition {
            md.insert("plenora.geometry.crs_definition", definition);
            md.insert("plenora.geometry.crs_definition_format", "projjson");
        } else {
            md.insert("plenora.geometry.crs_id", id);
        }
        let (authority, code) = id.split_once(':').unwrap();
        let raw = format!(
            r#"{{"crs_type":"authority_code","crs":"{}:{code}"}}"#,
            authority.to_ascii_lowercase()
        );
        md.insert("ARROW:extension:metadata", raw);
        let mut sm = Metadata::new();
        sm.insert("plenora.contract.version", "1");
        let schema = Arc::new(Schema::new_with_metadata(
            vec![Field::new("g", DataType::Binary, true).with_metadata(md)],
            sm,
        ));
        let output = contract_from_arrow_schema(schema.clone(), no_resolver).unwrap();
        assert_eq!(
            output.schema.field(0).metadata(),
            schema.field(0).metadata()
        );
        assert!(matches!(
            output.geometries[0].crs,
            ContractCrs::DeclaredUnresolved { .. }
        ));
    }
}

#[test]
fn duplicati_tipi_e_conflitti_non_entrano_nel_contratto() {
    for (raw, category) in [
        (r#"{"crs":null,"crs":"SENTINEL"}"#, "schema"),
        (r#"{"crs_type":"authority_code","crs":42}"#, "crs"),
        (r#"{"crs_type":"projjson","crs":"SENTINEL"}"#, "crs"),
        (r#"{"edges":42}"#, "crs"),
        (r#"{"crs_type":"projjson","crs":{}}"#, "crs"),
    ] {
        let mut md = Metadata::new();
        md.insert("ARROW:extension:name", "geoarrow.wkb");
        md.insert("ARROW:extension:metadata", raw);
        let error =
            read_geometry_contract_keys(&Field::new("g", DataType::Binary, true).with_metadata(md))
                .expect_err("rifiuto");
        assert_eq!(error.category().as_str(), category);
        assert!(!format!("{error:?}").contains("SENTINEL"));
    }
}
