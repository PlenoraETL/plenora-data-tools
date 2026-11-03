//! Enumerazione della politica: catalogo e vocabolario sono le uniche liste.
#![allow(clippy::unwrap_used, clippy::expect_used)]
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
    assert!(matches!(
        read_geometry_contract_keys(&field.clone().with_metadata(metadata)),
        Err(plenora_core::PlenoraError::Unsupported(_))
    ));
    let mut metadata = field.metadata().clone();
    metadata.insert("plenora.geometry.native.geoparquet.epoch", "2023.5");
    metadata.insert(
        "geo",
        r#"{"crs":"EPSG:32632","dimensions":"xy","epoch":2024.5}"#,
    );
    let error = read_geometry_contract_keys(&field.with_metadata(metadata)).unwrap_err();
    assert!(matches!(error, plenora_core::PlenoraError::Crs(_)));
    assert!(!error.to_string().contains("2023"));
    assert!(!error.to_string().contains("2024"));
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
        assert!(read_geometry_contract_keys(&inherited).is_ok());
    }
}
