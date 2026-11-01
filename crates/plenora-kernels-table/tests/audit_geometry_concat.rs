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
        merged_geometry_metadata(&[a.schema().field(0), &b], &[]),
        Err(PlenoraError::Unsupported(_))
    ));
}

// L'oracolo pubblica le dichiarazioni nel campo Arrow prima del kernel:
// l'API di analisi deve decidere allo stesso modo senza tale pubblicazione.
#[test]
#[allow(clippy::too_many_lines)]
fn dichiarazioni_solo_nel_contratto_hanno_lo_stesso_esito_del_kernel() {
    use plenora_core::contract::{
        arrow_schema::{arrow_schema_from_contract, contract_from_arrow_schema},
        ContractProperty, GeometryType, GeometryTypesProperty, PropertyConfidence, PropertyScope,
        TypesDeclaration,
    };
    let batch = geometry("point", "wkb", "xy", true);
    let field = batch.schema().field(0).clone();
    let mut metadata = field.metadata().clone();
    metadata.remove("plenora.geometry.types");
    metadata.remove("plenora.geometry.types_declaration");
    let schema = Arc::new(Schema::new_with_metadata(
        vec![field.with_metadata(metadata)],
        plenora_core::contract::arrow_metadata::canonical_schema_version_metadata(),
    ));
    let base = contract_from_arrow_schema(schema, plenora_core::crs::resolve_crs).unwrap();
    let declarations = [
        None,
        Some((TypesDeclaration::Exact, vec![GeometryType::Point])),
        Some((TypesDeclaration::Exact, vec![GeometryType::LineString])),
        Some((
            TypesDeclaration::Mixed,
            vec![GeometryType::Point, GeometryType::LineString],
        )),
        Some((TypesDeclaration::Mixed, vec![])),
    ];
    for a in &declarations {
        for b in &declarations {
            let contract = |declaration: &Option<(TypesDeclaration, Vec<GeometryType>)>| {
                let mut contract = base.clone();
                if let Some((kind, types)) = declaration {
                    contract.geometries[0].types = ContractProperty::new(
                        PropertyConfidence::Declared(
                            GeometryTypesProperty::new(*kind, types.clone()).unwrap(),
                        ),
                        PropertyScope::Schema,
                    );
                }
                contract.validate().unwrap();
                contract
            };
            let left = contract(a);
            let right = contract(b);
            // Differiscono soltanto le dichiarazioni tipizzate, mai i campi Arrow.
            assert_eq!(left.schema, right.schema);
            let left_batch = RecordBatch::new_empty(arrow_schema_from_contract(&left).unwrap());
            let right_batch = RecordBatch::new_empty(arrow_schema_from_contract(&right).unwrap());
            for (op, config) in [
                ("table.concat", json!({})),
                ("table.concat_by_name", json!({"strict": false})),
                ("table.concat_by_name", json!({"strict": true})),
                ("table.union_distinct", json!({})),
                ("table.intersect", json!({})),
                ("table.except", json!({})),
            ] {
                let limits = Limits::default();
                let analyzed = analyze_table_contract(
                    op,
                    &[left.clone(), right.clone()],
                    &config,
                    &mut FieldAllocator::default(),
                    &limits,
                );
                let kernel = match op {
                    "table.concat" => joins::concat(
                        &left_batch,
                        &right_batch,
                        &serde_json::from_value(json!({})).unwrap(),
                        &limits,
                    ),
                    "table.concat_by_name" => joins::concat_by_name(
                        &[&left_batch, &right_batch],
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                    "table.union_distinct" => setops::union_distinct(
                        &left_batch,
                        &right_batch,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                    _ => setops::validate_schema(&left_batch, &right_batch)
                        .map(|()| left_batch.clone()),
                };
                let accepts = a == b || matches!(op, "table.intersect" | "table.except");
                assert_eq!(
                    analyzed.is_ok(),
                    accepts,
                    "{op}: {a:?}, {b:?}: {analyzed:?}"
                );
                assert_eq!(kernel.is_ok(), accepts, "{op}: {a:?}, {b:?}: {kernel:?}");
                match (analyzed, kernel) {
                    (Err(a), Err(b)) => {
                        assert!(matches!(a, PlenoraError::Schema(_)));
                        assert_eq!(a.to_string(), b.to_string());
                    }
                    (Ok(output), Ok(_)) => {
                        assert_eq!(output.geometries[0].types, left.geometries[0].types);
                    }
                    _ => panic!("analisi e kernel divergono"),
                }
                if op == "table.concat_by_name" {
                    let third = analyze_table_contract(
                        op,
                        &[left.clone(), left.clone(), right.clone()],
                        &config,
                        &mut FieldAllocator::default(),
                        &limits,
                    );
                    assert_eq!(third.is_ok(), accepts);
                }
            }
        }
    }
}

#[test]
fn geometria_solo_nel_contratto_non_si_fonde_con_byte_anonimi() {
    use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
    let batch = geometry("point", "wkb", "xy", true);
    let declared = Arc::new(batch.schema().as_ref().clone().with_metadata(
        plenora_core::contract::arrow_metadata::canonical_schema_version_metadata(),
    ));
    let mut left = contract_from_arrow_schema(declared, plenora_core::crs::resolve_crs).unwrap();
    left.schema = Arc::new(Schema::new(vec![plenora_core::arrow::Field::new(
        "g",
        plenora_core::arrow::DataType::Binary,
        true,
    )]));
    left.validate().unwrap();
    let right = DataContract::tabular(left.schema.clone());
    for (op, config) in [
        ("table.concat", json!({})),
        ("table.concat_by_name", json!({"strict": false})),
        ("table.concat_by_name", json!({"strict": true})),
        ("table.union_distinct", json!({})),
        ("table.intersect", json!({})),
        ("table.except", json!({})),
    ] {
        for inputs in [[left.clone(), right.clone()], [right.clone(), left.clone()]] {
            assert!(
                matches!(
                    analyze_table_contract(
                        op,
                        &inputs,
                        &config,
                        &mut FieldAllocator::default(),
                        &Limits::default()
                    ),
                    Err(PlenoraError::Crs(_))
                ),
                "{op}"
            );
        }
    }
}

#[test]
fn fusione_derivata_conserva_encoding_e_dimensioni_solo_tipizzati() {
    use plenora_core::contract::{
        ContractCrs, ContractProperties, FieldId, GeometryColumnContract, GeometryDimensions,
        GeometryEncoding,
    };
    let field = plenora_core::arrow::Field::new("g", plenora_core::arrow::DataType::Binary, true)
        .with_metadata(std::collections::HashMap::from([(
            "ARROW:extension:name".to_owned(),
            "geoarrow.wkb".to_owned(),
        )]));
    let input = DataContract::from_source(
        Arc::new(Schema::new(vec![field])),
        vec![GeometryColumnContract {
            field_id: FieldId(0),
            name: "g".into(),
            crs: ContractCrs::Missing,
            dimensions: GeometryDimensions::Xyz,
            encoding: Some(GeometryEncoding::Ewkb),
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        Some(FieldId(0)),
        ContractProperties::default(),
    )
    .unwrap();
    for (op, config) in [
        (
            "table.melt",
            json!({"id_columns": [], "value_columns": ["g"], "value_name": "result"}),
        ),
        (
            "table.coalesce",
            json!({"columns": ["g"], "output_column": "g"}),
        ),
    ] {
        let output = analyze_table_contract(
            op,
            std::slice::from_ref(&input),
            &config,
            &mut FieldAllocator::default(),
            &Limits::default(),
        )
        .unwrap();
        let name = if op == "table.coalesce" {
            "g"
        } else {
            "result"
        };
        let geometry = output.geometries.iter().find(|g| g.name == name).unwrap();
        assert_eq!(geometry.encoding, Some(GeometryEncoding::Ewkb), "{op}");
        assert_eq!(geometry.dimensions, GeometryDimensions::Xyz, "{op}");
        let keys = plenora_core::contract::arrow_metadata::read_geometry_contract_keys(
            output.schema.field_with_name(name).unwrap(),
        )
        .unwrap();
        assert_eq!(keys.encoding, geometry.encoding);
        assert_eq!(keys.dimensions, Some(geometry.dimensions));
    }
}

#[test]
fn dichiarazione_tipizzata_e_stessi_metadati_legacy_sono_compatibili() {
    use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
    let schema = Arc::new(Schema::new(vec![
        geometry_output_field("g", "EPSG:3857").unwrap()
    ]));
    let declared =
        contract_from_arrow_schema(schema.clone(), plenora_core::crs::resolve_crs).unwrap();
    let arrow_only = DataContract::tabular(schema);
    for (op, config) in [
        ("table.concat", json!({})),
        ("table.concat_by_name", json!({"strict": false})),
        ("table.concat_by_name", json!({"strict": true})),
        ("table.union_distinct", json!({})),
        ("table.intersect", json!({})),
        ("table.except", json!({})),
    ] {
        for inputs in [
            [declared.clone(), arrow_only.clone()],
            [arrow_only.clone(), declared.clone()],
        ] {
            analyze_table_contract(
                op,
                &inputs,
                &config,
                &mut FieldAllocator::default(),
                &Limits::default(),
            )
            .unwrap();
        }
    }
}

#[test]
fn decisione_crs_sostituisce_la_lineage_anche_nelle_fusioni() {
    use plenora_core::contract::{
        arrow_schema::{arrow_schema_from_contract, contract_from_arrow_schema},
        ContractCrs,
    };
    let field = geometry_output_field("g", "EPSG:4326").unwrap();
    let schema = Arc::new(Schema::new(vec![field.clone()]));
    let mut decided = contract_from_arrow_schema(schema, plenora_core::crs::resolve_crs).unwrap();
    let mut metadata = field.metadata().clone();
    // Conflitto nella sorgente: solo la decisione esplicita lo sostituisce.
    metadata.insert("plenora.geometry.crs_id", "EPSG:3857");
    metadata.insert("plenora.geometry.axis_order", "lat_lon");
    decided.schema = Arc::new(Schema::new_with_metadata(
        vec![field.with_metadata(metadata)],
        plenora_core::contract::arrow_metadata::canonical_schema_version_metadata(),
    ));
    decided.geometries[0].crs = ContractCrs::ResolvedByDecision(
        plenora_core::crs::resolve_crs("EPSG:3857", "crs").unwrap(),
    );
    decided.validate().unwrap();
    let published = arrow_schema_from_contract(&decided).unwrap();
    // L'altro ramo dichiara il CRS deciso, ma non aggiunge nozioni di
    // precisione o semantica spaziale che la decisione non puo' decidere.
    let mut metadata = published.field(0).metadata().clone();
    metadata.remove("plenora.geometry.precision");
    metadata.remove("plenora.geometry.spatial_semantics");
    let published_contract = DataContract::tabular(Arc::new(Schema::new_with_metadata(
        vec![published.field(0).as_ref().clone().with_metadata(metadata)],
        published.metadata().clone(),
    )));
    analyze_table_contract(
        "table.concat",
        &[decided.clone(), published_contract],
        &json!({}),
        &mut FieldAllocator::default(),
        &Limits::default(),
    )
    .unwrap();
    let output = analyze_table_contract(
        "table.melt",
        &[decided],
        &json!({"id_columns": [], "value_columns": ["g"], "value_name": "result"}),
        &mut FieldAllocator::default(),
        &Limits::default(),
    )
    .unwrap();
    let actual = plenora_core::contract::arrow_metadata::read_geometry_contract_keys(
        output.schema.field_with_name("result").unwrap(),
    )
    .unwrap();
    let expected =
        plenora_core::contract::arrow_metadata::read_geometry_contract_keys(published.field(0))
            .unwrap();
    assert_eq!(actual.crs_id, expected.crs_id);
    assert_eq!(actual.axis_order, expected.axis_order);
    assert_eq!(actual.srid, expected.srid);
}

#[test]
fn crs_con_srid_in_conflitto_resta_irrisolto_su_entrambi_i_percorsi() {
    use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
    let field = geometry_output_field("g", "EPSG:3857").unwrap();
    let mut metadata = field.metadata().clone();
    metadata.insert("plenora.geometry.crs_id", "EPSG:3857");
    metadata.insert("plenora.geometry.srid", "4326");
    metadata.insert("plenora.geometry.crs_resolution", "resolved");
    let schema = Arc::new(Schema::new_with_metadata(
        vec![field.with_metadata(metadata)],
        plenora_core::contract::arrow_metadata::canonical_schema_version_metadata(),
    ));
    let contract =
        contract_from_arrow_schema(schema.clone(), plenora_core::crs::resolve_crs).unwrap();
    assert!(matches!(
        contract.geometries[0].crs,
        plenora_core::contract::ContractCrs::DeclaredUnresolved { .. }
    ));
    analyze_table_contract(
        "table.concat",
        &[contract, DataContract::tabular(schema)],
        &json!({}),
        &mut FieldAllocator::default(),
        &Limits::default(),
    )
    .unwrap();
}
