//! Enumerazione della politica: catalogo e vocabolario sono le uniche liste.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[macro_use]
#[path = "../../../tests/comune/esito.rs"]
mod esito;

use plenora_core::catalog::CATALOG;
use plenora_core::contract::geometry_metadata_policy::{self as policy, Class, Effect, KEYS};

#[test]
fn ogni_chiave_per_ogni_operazione_e_fusione() {
    let mut combinations = 0;
    for &(key, class) in KEYS {
        for op in CATALOG {
            let effect = policy::operation_effect(op);
            for equal in [false, true] {
                let expected = match class {
                    Class::Crs => effect != Effect::Crs,
                    Class::Derived => {
                        effect == Effect::Unchanged
                            || (key.starts_with("plenora.postgres.") && effect == Effect::Rows)
                            || (effect == Effect::Rows
                                && matches!(
                                    key,
                                    "bbox" | "plenora.geometry.native.geoparquet.bbox"
                                ))
                    }
                    Class::Types => effect != Effect::Types && (effect != Effect::Merge || equal),
                    Class::Representation => {
                        matches!(effect, Effect::Unchanged | Effect::Rows)
                            || (effect == Effect::Merge && equal)
                    }
                };
                assert_eq!(
                    policy::keep(key, effect, equal),
                    expected,
                    "{key}: {}",
                    op.id
                );
                combinations += 1;
            }
        }
        for op in CATALOG.iter().filter(|op| {
            matches!(
                op.id,
                "table.concat" | "table.concat_by_name" | "table.union_distinct" | "table.join"
            )
        }) {
            for equal in [false, true] {
                let expected = match class {
                    Class::Crs => true,
                    Class::Derived => false,
                    Class::Representation | Class::Types => equal,
                };
                assert_eq!(
                    policy::keep(key, Effect::Merge, equal),
                    expected,
                    "{key}: {}",
                    op.id
                );
                combinations += 1;
            }
        }
    }
    println!(
        "POLICY_COMBINATIONS={combinations}; KEYS={}; OPERATIONS={}",
        KEYS.len(),
        CATALOG.len()
    );
}

#[test]
fn tabella_univoca_e_chiavi_dei_sorgenti_coperte() {
    let mut names = std::collections::BTreeSet::new();
    for (key, _) in KEYS {
        assert!(names.insert(*key), "chiave duplicata: {key}");
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path().join("src");
        if path.is_dir() {
            visit(&path, &names);
        }
    }
}

fn visit(path: &std::path::Path, names: &std::collections::BTreeSet<&str>) {
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            visit(&path, names);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            let production = text.split("#[cfg(test)]").next().unwrap();
            for literal in production.split('"').skip(1).step_by(2) {
                if literal.starts_with("plenora.geometry.")
                    && !literal.ends_with('.')
                    && !literal.contains([' ', '{', '\n'])
                {
                    assert!(
                        names.contains(literal),
                        "chiave non classificata: {literal} ({})",
                        path.display()
                    );
                }
            }
        }
    }
}

#[test]
fn proprieta_crs_native_verificate_anche_senza_geoparquet() {
    use plenora_core::contract::arrow_metadata::{
        geometry_output_field, read_geometry_contract_keys,
    };
    let field = geometry_output_field("geometry", "EPSG:32632").unwrap();
    let mut metadata = field.metadata().clone();
    metadata.insert("plenora.geometry.native.geoparquet.edges", "spherical");
    let spherical = field.clone().with_metadata(metadata);
    read_geometry_contract_keys(&spherical).expect("trasporto sferico");
    assert!(matches!(
        policy::require_planar(&spherical),
        Err(plenora_core::PlenoraError::Unsupported(_))
    ));
    let mut metadata = field.metadata().clone();
    metadata.insert("plenora.geometry.native.geoparquet.epoch", "2023.5");
    metadata.insert(
        "geo",
        r#"{"crs":"EPSG:32632","dimensions":"xy","epoch":2024.5}"#,
    );
    let error = read_geometry_contract_keys(&field.with_metadata(metadata))
        .expect_err("rifiuto con variante attesa verificata sotto");
    assert!(matches!(error, plenora_core::PlenoraError::Crs(_)));
    assert_eq!(error.phase(), plenora_core::ErrorPhase::Validate);
    assert_eq!(error.phase(), plenora_core::ErrorPhase::Validate);
    assert!(!error.to_string().contains("2023"));
    assert!(!error.to_string().contains("2024"));
}

#[test]
fn catalogo_e_ruoli_sono_enumerati_senza_default() {
    use plenora_core::catalog::{Arity, Family};
    use policy::{OperandUse, OPERATION_POLICY};
    let mut seen = std::collections::BTreeSet::new();
    for &(id, _, roles) in OPERATION_POLICY {
        assert!(seen.insert(id));
        let op = CATALOG
            .iter()
            .find(|op| op.id == id)
            .expect("operazione censita");
        assert_eq!(
            roles.len(),
            if op.arity == Arity::BinaryOrdered {
                2
            } else {
                1
            }
        );
        assert_eq!(roles, policy::operand_uses(id).unwrap());
        for role in roles {
            assert_eq!(
                matches!(role, OperandUse::Read | OperandUse::Produce),
                op.family == Family::Geo
            );
        }
    }
    assert_eq!(seen, CATALOG.iter().map(|op| op.id).collect());
    assert!(matches!(
        policy::operand_uses("future.operation"),
        Err(plenora_core::PlenoraError::Internal(_))
    ));
}

#[test]
fn namespace_postgres_chiuso_per_tutti_gli_effetti() {
    use plenora_core::arrow::Metadata;
    let keys = KEYS
        .iter()
        .filter(|(key, _)| key.starts_with("plenora.postgres."))
        .map(|(key, _)| *key)
        .chain(["plenora.postgres.future"]);
    for key in keys {
        for effect in [
            Effect::Unchanged,
            Effect::Rows,
            Effect::Values,
            Effect::Types,
            Effect::Crs,
            Effect::Merge,
        ] {
            let mut md = Metadata::new();
            md.insert(key, "SENTINEL_NATIVE");
            md.insert("source.provenance", "origin");
            policy::apply(&mut md, effect);
            assert_eq!(
                md.contains_key(key),
                matches!(effect, Effect::Unchanged | Effect::Rows)
            );
            assert_eq!(
                md.get("source.provenance").map(String::as_str),
                Some("origin")
            );
            assert_eq!(policy::class(key), Class::Derived);
            assert!(policy::governed(key));
        }
    }
}

#[test]
fn postgres_per_lineage_geometria_attributo_e_primaria() {
    use plenora_core::arrow::{DataType, Field, Metadata, Schema};
    use plenora_core::contract::arrow_metadata::geometry_output_field;
    use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
    let keys: Vec<_> = KEYS
        .iter()
        .filter(|(key, _)| key.starts_with("plenora.postgres."))
        .map(|(key, _)| *key)
        .chain(["plenora.postgres.future"])
        .collect();
    for key in keys {
        for primary in [0, 1] {
            for selected in [0, 1] {
                let mut fields: Vec<_> = ["a", "b"]
                    .map(|name| geometry_output_field(name, "EPSG:32632").unwrap())
                    .to_vec();
                fields.push(Field::new("attribute", DataType::Int64, true));
                for field in &mut fields {
                    let mut md = field.metadata().clone();
                    md.insert(key, "SENTINEL");
                    *field = field.clone().with_metadata(md);
                }
                let mut md = Metadata::new();
                md.insert("plenora.contract.version", "1");
                let mut input = contract_from_arrow_schema(
                    std::sync::Arc::new(Schema::new_with_metadata(fields, md)),
                    plenora_core::crs::resolve_crs,
                )
                .unwrap();
                input.active_geometry = Some(input.geometries[primary].field_id);
                for (op, effect) in [
                    ("geo.area", Effect::Unchanged),
                    ("geo.sjoin", Effect::Rows),
                    ("geo.translate", Effect::Values),
                    ("geo.centroid", Effect::Types),
                    ("geo.reproject", Effect::Crs),
                    ("table.concat", Effect::Merge),
                ] {
                    let descriptor = CATALOG.iter().find(|d| d.id == op).unwrap();
                    let effects = policy::column_effects(
                        descriptor,
                        &input.schema,
                        &[Some(&input.geometries[selected])],
                    )
                    .unwrap();
                    assert_eq!(effects[selected].1, effect);
                    for (position, column_effect) in effects {
                        let mut md = input.schema.field(position).metadata().clone();
                        policy::apply(&mut md, column_effect);
                        assert_eq!(
                            md.contains_key(key),
                            matches!(column_effect, Effect::Unchanged | Effect::Rows),
                            "{op}/{primary}/{selected}/{position}/{key}"
                        );
                    }
                    // La lineage non applica l'effetto geometrico agli attributi.
                    assert!(input.schema.field(2).metadata().contains_key(key));
                }
            }
        }
    }
}

#[test]
fn geometria_nuova_accetta_estensione_crs_vuota() {
    use plenora_core::contract::arrow_metadata::{
        geometry_output_field, read_geometry_contract_keys,
    };
    let field = geometry_output_field("geometry", "EPSG:32632").unwrap();
    for raw in ["", "   ", "{}"] {
        let mut metadata = field.metadata().clone();
        metadata.insert("ARROW:extension:metadata", raw);
        let source = field.clone().with_metadata(metadata);
        let inherited = policy::inherit_crs(&field, &source).unwrap();
        assert_eq!(
            read_geometry_contract_keys(&inherited).expect("geometria ereditata"),
            read_geometry_contract_keys(&field).expect("fixture")
        );
    }
}

#[test]
fn orientamento_per_cella_e_fusione() {
    use plenora_core::arrow::Metadata;
    for key in [
        "orientation",
        "plenora.geometry.native.geoparquet.orientation",
    ] {
        let mut md = Metadata::new();
        md.insert(key, "counterclockwise");
        let original = md.clone();
        policy::apply(&mut md, Effect::Rows);
        assert_eq!(md, original);
        assert_eq!(policy::merge(&md, &md).unwrap(), md);
        assert!(!policy::merge(&md, &Metadata::new())
            .unwrap()
            .contains_key(key));
        policy::apply(&mut md, Effect::Values);
        assert!(!md.contains_key(key));
    }
}

#[test]
fn srid_derivato_solo_con_mapping_wkb_risolto() {
    use plenora_core::arrow::Schema;
    use plenora_core::contract::arrow_metadata::{
        geometry_output_field, require_geometry_compatibility, GeometryComparison,
    };
    use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
    use plenora_core::crs::resolve_crs;
    for encoding in ["wkb", "ewkb"] {
        let base = geometry_output_field("geometry", "EPSG:32632").unwrap();
        let mut md = base.metadata().clone();
        md.remove("geo");
        md.insert("plenora.geometry.encoding", encoding);
        md.insert("plenora.geometry.crs_id", "EPSG:32632");
        md.insert("plenora.geometry.crs_resolution", "resolved");
        let absent = base.with_metadata(md.clone());
        md.insert("plenora.geometry.srid", "32632");
        let present = absent.clone().with_metadata(md);
        let mut metadata = plenora_core::arrow::Metadata::new();
        metadata.insert("plenora.contract.version", "1");
        let schema = Schema::new_with_metadata(vec![absent.clone()], metadata);
        let contract =
            contract_from_arrow_schema(std::sync::Arc::new(schema), resolve_crs).unwrap();
        for (left, right) in [(&present, &absent), (&absent, &present)] {
            let result = require_geometry_compatibility(
                left,
                &contract.geometries,
                right,
                &contract.geometries,
                GeometryComparison::Declaration,
            );
            if encoding == "wkb" {
                assert_successo!(result, ());
            } else {
                assert_rifiuto!(
                    result,
                    plenora_core::ErrorCategory::Crs,
                    plenora_core::ErrorPhase::Validate
                );
            }
        }
    }
}

#[test]
fn effetti_per_operando_passante_e_primaria_assente() {
    use plenora_core::arrow::Schema;
    use plenora_core::catalog::{Family, SourceRowProvenance};
    use plenora_core::contract::arrow_metadata::geometry_output_field;
    use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
    let fields = ["passante", "operando", "altra"]
        .map(|name| geometry_output_field(name, "EPSG:32632").unwrap());
    let contract = contract_from_arrow_schema(
        std::sync::Arc::new(Schema::new(fields.to_vec())),
        plenora_core::crs::resolve_crs,
    )
    .unwrap();
    assert!(contract.active_geometry.is_none());
    let operand = contract
        .geometries
        .iter()
        .find(|g| g.name == "operando")
        .unwrap();
    for op in CATALOG.iter().filter(|op| op.family == Family::Geo) {
        let effects = policy::column_effects(op, &contract.schema, &[Some(operand)]).unwrap();
        let effect = policy::operation_effect(op);
        assert_eq!(effects[1], (1, effect), "{}", op.id);
        let expected = if op.source_row_provenance() == SourceRowProvenance::Preserved {
            Effect::Unchanged
        } else {
            Effect::Rows
        };
        assert_eq!(effects[0], (0, expected), "{}", op.id);
        assert_eq!(effects[2], (2, expected), "{}", op.id);
    }
}

#[test]
fn pubblicazione_non_rielegge_geo_di_schema() {
    use plenora_core::arrow::{Metadata, Schema};
    use plenora_core::contract::arrow_metadata::geometry_output_field;
    use plenora_core::contract::arrow_schema::pubblica_schema_con_primaria;
    let fields = ["a", "b"].map(|name| geometry_output_field(name, "EPSG:32632").unwrap());
    let mut metadata = Metadata::new();
    metadata.insert("geo", r#"{"primary_column":"b","columns":{"a":{},"b":{}}}"#);
    let schema = Schema::new_with_metadata(fields.to_vec(), metadata);
    for primary in [None, Some(0)] {
        let output = pubblica_schema_con_primaria(
            &schema,
            primary,
            &std::collections::BTreeSet::default(),
            &std::collections::BTreeSet::default(),
        )
        .unwrap();
        assert!(!output.metadata().contains_key("geo"));
    }
    let output = pubblica_schema_con_primaria(
        &schema,
        Some(1),
        &std::collections::BTreeSet::default(),
        &std::collections::BTreeSet::default(),
    )
    .unwrap();
    assert!(output.metadata().contains_key("geo"));
    let renamed = Schema::new_with_metadata(
        vec![
            schema.field(0).clone().with_name("renamed"),
            schema.field(1).clone(),
        ],
        schema.metadata().clone(),
    );
    let output = pubblica_schema_con_primaria(
        &renamed,
        Some(1),
        &std::collections::BTreeSet::default(),
        &std::collections::BTreeSet::default(),
    )
    .unwrap();
    assert!(!output.metadata().contains_key("geo"));
    let reordered = schema.project(&[1, 0]).unwrap();
    let output = pubblica_schema_con_primaria(
        &reordered,
        Some(0),
        &std::collections::BTreeSet::default(),
        &std::collections::BTreeSet::default(),
    )
    .unwrap();
    assert!(output.metadata().contains_key("geo"));
    let projected = schema.project(&[0]).unwrap();
    let output = pubblica_schema_con_primaria(
        &projected,
        None,
        &std::collections::BTreeSet::default(),
        &std::collections::BTreeSet::default(),
    )
    .unwrap();
    assert!(!output.metadata().contains_key("geo"));
}

#[test]
fn metadati_derivati_identici_con_e_senza_contratto() {
    use plenora_core::arrow::{Field, Schema};
    use plenora_core::contract::arrow_metadata::{geometry_output_field, merged_geometry_metadata};
    use plenora_core::contract::arrow_schema::{
        arrow_schema_from_contract, contract_from_arrow_schema,
    };
    let field = geometry_output_field("g", "EPSG:32632").unwrap();
    let mut md = field.metadata().clone();
    md.insert("plenora.geometry.types", "point");
    md.insert("plenora.geometry.types_declaration", "mixed");
    let field: Field = field.with_metadata(md);
    let contract = contract_from_arrow_schema(
        std::sync::Arc::new(Schema::new_with_metadata(
            vec![field],
            std::iter::once(("plenora.contract.version", "1"))
                .collect::<plenora_core::arrow::Metadata>(),
        )),
        plenora_core::crs::resolve_crs,
    )
    .unwrap();
    let schema = arrow_schema_from_contract(&contract).unwrap();
    let raw = merged_geometry_metadata(&[schema.field(0)], &[]).unwrap();
    let typed = merged_geometry_metadata(&[schema.field(0)], &contract.geometries).unwrap();
    let atteso: plenora_core::arrow::Metadata = [
        ("ARROW:extension:name", "geoarrow.wkb"),
        ("geo", r#"{"crs":"EPSG:32632","dimensions":"xy"}"#),
        ("plenora.geometry.encoding", "wkb"),
        ("plenora.geometry.dimensions", "xy"),
        ("plenora.geometry.types_declaration", "unresolved"),
        ("plenora.geometry.crs_resolution", "resolved"),
        ("plenora.geometry.crs_id", "EPSG:32632"),
        ("plenora.geometry.srid", "32632"),
        ("plenora.geometry.axis_order", "unknown"),
        ("plenora.geometry.spatial_semantics", "geometry"),
        ("plenora.geometry.precision", "float64"),
    ]
    .into_iter()
    .collect();
    assert_eq!(raw, atteso);
    assert_eq!(typed, atteso);
}

#[test]
fn effetti_operandi_multipli_e_ruoli_assenti() {
    use plenora_core::arrow::Schema;
    use plenora_core::contract::arrow_metadata::geometry_output_field;
    use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
    let fields = ["passante", "primo", "secondo"]
        .map(|name| geometry_output_field(name, "EPSG:32632").unwrap());
    let contract = contract_from_arrow_schema(
        std::sync::Arc::new(Schema::new(fields.to_vec())),
        plenora_core::crs::resolve_crs,
    )
    .unwrap();
    let op = CATALOG.iter().find(|op| op.id == "geo.explode").unwrap();
    let operands = [&contract.geometries[1], &contract.geometries[2]];
    for selected in [
        vec![Some(operands[0]), Some(operands[1])],
        vec![Some(operands[1]), Some(operands[0])],
        vec![None, Some(operands[0]), Some(operands[1])],
    ] {
        assert_eq!(
            policy::column_effects(op, &contract.schema, &selected).unwrap(),
            vec![(0, Effect::Rows), (1, Effect::Types), (2, Effect::Types)]
        );
    }
    let partial = Schema::new(vec![fields[0].clone(), fields[1].clone()]);
    for selected in [
        vec![Some(operands[0]), Some(operands[1])],
        vec![Some(operands[1]), Some(operands[0])],
    ] {
        assert!(matches!(
            policy::column_effects(op, &partial, &selected),
            Err(plenora_core::PlenoraError::Internal(_))
        ));
    }
    let absent = Schema::new(vec![fields[0].clone()]);
    for selected in [vec![], vec![None], vec![Some(operands[0])]] {
        let error = policy::column_effects(op, &absent, &selected)
            .expect_err("rifiuto con variante attesa verificata sotto");
        assert!(matches!(error, plenora_core::PlenoraError::Internal(_)));
        assert_eq!(error.phase(), plenora_core::ErrorPhase::Write);
        assert_eq!(error.phase(), plenora_core::ErrorPhase::Write);
        assert_eq!(error.to_string(),
            "internal error: politica metadati: operando assente dall'uscita con righe non conservate");
    }
    let preserved = CATALOG.iter().find(|op| op.id == "geo.buffer").unwrap();
    assert_eq!(
        policy::column_effects(preserved, &absent, &[None]).unwrap(),
        vec![(0, Effect::Unchanged)]
    );
    for id in [
        "geo.coverage_validate",
        "geo.shared_paths",
        "geo.generate_grid",
    ] {
        let producer = CATALOG.iter().find(|op| op.id == id).unwrap();
        assert_eq!(
            policy::column_effects(producer, &absent, &[None]).unwrap(),
            vec![(0, Effect::Unchanged)]
        );
    }
}
