//! Le concatenazioni non estendono implicitamente dichiarazioni geometriche.
use plenora_core::arrow::array::{BinaryArray, RecordBatch};
use plenora_core::arrow::schema::Schema;
use plenora_core::contract::{arrow_metadata::geometry_output_field, DataContract, FieldAllocator};
use plenora_core::PlenoraError;
use plenora_kernels_table::{analyze::analyze_table_contract, joins, setops, Limits};
use serde_json::json;
use std::sync::Arc;

fn geometry(kind: &str, encoding: &str, dimensions: &str, empty: bool) -> RecordBatch {
    let field = geometry_output_field("g", "EPSG:3857").unwrap();
    let mut metadata = field.metadata().clone();
    metadata.remove("geo");
    metadata.insert("plenora.geometry.encoding", encoding);
    metadata.insert("plenora.geometry.dimensions", dimensions);
    metadata.insert("plenora.geometry.types_declaration", "exact");
    metadata.insert("plenora.geometry.types", kind);
    let code: u32 = if kind == "point" { 1 } else { 2 };
    let code = code + if dimensions == "xyz" { 1000 } else { 0 };
    let mut bytes = vec![1];
    bytes.extend_from_slice(&code.to_le_bytes());
    if kind != "point" {
        bytes.extend_from_slice(&2_u32.to_le_bytes());
    }
    for index in 0..if kind == "point" { 1 } else { 2 } {
        for _ in 0..if dimensions == "xyz" { 3 } else { 2 } {
            bytes.extend_from_slice(&f64::from(index).to_le_bytes());
        }
    }
    let cells = if empty {
        vec![]
    } else {
        vec![Some(bytes.as_slice())]
    };
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![field.with_metadata(metadata)])),
        vec![Arc::new(BinaryArray::from(cells))],
    )
    .unwrap()
}

#[test]
fn ogni_concatenazione_rifiuta_dichiarazioni_incompatibili() {
    for empty in [false, true] {
        let left = geometry("point", "wkb", "xy", empty);
        for (kind, encoding, dimensions) in [
            ("linestring", "wkb", "xy"),
            ("point", "ewkb", "xy"),
            ("point", "wkb", "xyz"),
        ] {
            let right = geometry(kind, encoding, dimensions, empty);
            for (op, config) in [
                ("table.concat", json!({})),
                ("table.concat_by_name", json!({"strict":false})),
                ("table.concat_by_name", json!({"strict":true})),
                ("table.union_distinct", json!({})),
            ] {
                let limits = Limits::default();
                let result = match op {
                    "table.concat" => joins::concat(
                        &left,
                        &right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                    "table.concat_by_name" => joins::concat_by_name(
                        &[&left, &right],
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                    _ => setops::union_distinct(
                        &left,
                        &right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                };
                let error = result.unwrap_err();
                assert!(matches!(error, PlenoraError::Schema(_)), "{error}");
                let analyzed = analyze_table_contract(
                    op,
                    &[
                        DataContract::tabular(left.schema()),
                        DataContract::tabular(right.schema()),
                    ],
                    &config,
                    &mut FieldAllocator::default(),
                    &limits,
                )
                .unwrap_err();
                assert_eq!(error.to_string(), analyzed.to_string());
            }
        }
        assert!(joins::concat_by_name(
            &[&left, &left],
            &joins::ConcatByName { strict: false },
            &Limits::default()
        )
        .is_ok());
    }
}

#[test]
fn confrontare_tipi_diversi_non_impila_le_dichiarazioni() {
    let left = geometry("point", "wkb", "xy", false);
    let right = geometry("linestring", "wkb", "xy", false);
    assert!(setops::validate_schema(&left, &right).is_ok());
    for op in ["table.intersect", "table.except"] {
        assert!(analyze_table_contract(
            op,
            &[
                DataContract::tabular(left.schema()),
                DataContract::tabular(right.schema())
            ],
            &json!({}),
            &mut FieldAllocator::default(),
            &Limits::default()
        )
        .is_ok());
    }
}

#[test]
fn fusione_non_eredita_precisione_discordante() {
    use plenora_core::contract::arrow_metadata::merged_geometry_metadata;
    let a = geometry("point", "wkb", "xy", true);
    let field = a.schema().field(0).clone();
    let mut metadata = field.metadata().clone();
    metadata.insert("plenora.geometry.precision", "float32");
    let b = field.with_metadata(metadata);
    assert!(matches!(
        merged_geometry_metadata(&[a.schema().field(0), &b]),
        Err(PlenoraError::Unsupported(_))
    ));
}
