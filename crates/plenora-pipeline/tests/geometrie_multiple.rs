//! Trasporto plurale, rimappatura degli ID e rifiuti transitori delle operazioni.
use plenora_core::{
    arrow::{
        array::{BinaryArray, RecordBatch},
        schema::{DataType, Field, Schema},
    },
    catalog::{Arity, Family, CATALOG},
    contract::{arrow_schema::contract_from_arrow_schema, FieldAllocator},
    crs::resolve_crs,
    ErrorCategory,
};
use plenora_pipeline::Pipeline;
use serde_json::json;
use std::{collections::HashMap, sync::Arc};

fn batch(primary: Option<&str>) -> RecordBatch {
    let fields: Vec<_> = ["a", "b"]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            Field::new(*name, DataType::Binary, true).with_metadata(HashMap::from([
                ("ARROW:extension:name".into(), "geoarrow.wkb".into()),
                ("plenora.field_id".into(), (10 + i).to_string()),
            ]))
        })
        .collect();
    let mut metadata = HashMap::from([("plenora.contract.version".into(), "1".into())]);
    if let Some(primary) = primary {
        metadata.insert("plenora.primary_geometry".into(), primary.into());
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        vec![
            Arc::new(BinaryArray::from(vec![None::<&[u8]>, Some(&[255][..])])),
            Arc::new(BinaryArray::from(vec![Some(&[254][..]), None])),
        ],
    )
    .expect("fixture")
}

#[test]
fn trasporto_senza_passi_preserva_colonne_byte_primaria_e_identita() {
    // Gli ID interni del secondo ingresso collidono con quelli sorgente: 1 -> 2,
    // 0 -> 1 e riferimenti pubblici indipendenti dai numeri del planner.
    for primary in [None, Some("none"), Some("10"), Some("11")] {
        let input = batch(primary);
        let pipeline = Pipeline::from_json(
            r#"{"version":1,"inputs":["first","second"],"steps":[],"outputs":["first","second"]}"#,
        )
        .expect("piano");
        let valid = pipeline
            .validate(&[("first", input.schema()), ("second", input.schema())])
            .expect("valida");
        let expected = match primary {
            Some("10") => Some("a"),
            Some("11") => Some("b"),
            _ => None,
        };
        for name in ["first", "second"] {
            let contract = valid.contratto(name).expect("contratto");
            assert_eq!(
                contract.active_geometry_column().map(|g| g.name.as_str()),
                expected
            );
            let published = contract_from_arrow_schema(
                valid.schema_uscita(name).expect("schema").clone(),
                resolve_crs,
            )
            .expect("pubblico");
            assert_eq!(
                published.active_geometry_column().map(|g| g.name.as_str()),
                expected
            );
        }
        let output = valid
            .run(
                [
                    ("first".into(), input.clone()),
                    ("second".into(), input.clone()),
                ]
                .into(),
            )
            .expect("trasporto");
        for (_, table) in &output.outputs {
            assert_eq!(table.columns(), input.columns());
            assert_ne!(
                table.schema().field(0).metadata().get("plenora.field_id"),
                table.schema().field(1).metadata().get("plenora.field_id")
            );
        }
    }
}

#[test]
fn tutte_le_analisi_geo_rifiutano_multigeometria_prima_della_config() {
    for primary in [None, Some("11")] {
        let input =
            contract_from_arrow_schema(batch(primary).schema(), resolve_crs).expect("contratto");
        for operation in CATALOG.iter().filter(|op| op.family == Family::Geo) {
            let arity = match operation.arity {
                Arity::Unary => 1,
                _ => 2,
            };
            for side in 0..arity {
                let mut inputs =
                    vec![
                        plenora_core::contract::DataContract::tabular(Arc::new(Schema::empty()));
                        arity
                    ];
                inputs[side] = input.clone();
                let mut allocator = FieldAllocator::default();
                let result = match operation.family {
                    Family::Table => plenora_kernels_table::analyze::analyze_table_contract(
                        operation.id,
                        &inputs,
                        &json!({}),
                        &mut allocator,
                        &plenora_kernels_table::Limits::default(),
                    ),
                    Family::Geo => plenora_kernels_geo::analyze::analyze_geo_contract(
                        operation.id,
                        &inputs,
                        &json!({}),
                        None,
                        &mut allocator,
                    ),
                };
                assert_eq!(
                    result.expect_err(operation.id).category(),
                    ErrorCategory::Unsupported,
                    "{} lato {}",
                    operation.id,
                    side
                );
            }
        }
    }
}

#[test]
fn adapter_tabellari_non_promuovono_la_secondaria() {
    use plenora_kernels_table::columns::{
        drop_columns, select_columns, DropColumns, SelectColumns,
    };
    let input = batch(Some("11"));
    for output in [
        drop_columns(
            &input,
            &DropColumns {
                columns: vec!["b".into()],
            },
        )
        .unwrap(),
        select_columns(
            &input,
            &SelectColumns {
                columns: vec!["a".into()],
            },
        )
        .unwrap(),
    ] {
        let contract = contract_from_arrow_schema(output.schema(), resolve_crs).unwrap();
        assert_eq!(contract.geometries.len(), 1);
        assert!(contract.active_geometry.is_none());
        assert_eq!(output.column(0), input.column(0));
    }
}

#[test]
fn planner_non_insegue_un_id_appena_rimappato() {
    let second = batch(Some("10"));
    let first_schema = Arc::new(Schema::new_with_metadata(
        vec![second.schema().field(0).clone()],
        HashMap::from([("plenora.contract.version".into(), "1".into())]),
    ));
    let pipeline = Pipeline::from_json(
        r#"{"version":1,"inputs":["first","second"],"steps":[],"outputs":["second"]}"#,
    )
    .expect("piano");
    let valid = pipeline
        .validate(&[("first", first_schema), ("second", second.schema())])
        .expect("valida");
    let contract = valid.contratto("second").expect("contratto");
    assert_eq!(contract.geometries[0].field_id.0, 1);
    assert_eq!(contract.geometries[1].field_id.0, 2);
    assert_eq!(
        contract.active_geometry_column().expect("primaria").name,
        "a"
    );
}

#[test]
fn output_plurali_da_input_singolo_e_sostituzione_tabellare_coerente() {
    use plenora_kernels_table::{
        quality::{coalesce, Coalesce},
        reshape::{transpose, Transpose},
        Limits,
    };
    let input = batch(None).project(&[0]).expect("una geometria");
    let transpose_config: Transpose = serde_json::from_value(json!({})).expect("config");
    let transposed = transpose(&input, &transpose_config, &Limits::default()).unwrap();
    let contract = contract_from_arrow_schema(transposed.schema(), resolve_crs).unwrap();
    assert_eq!(contract.geometries.len(), 2);
    assert!(contract.active_geometry.is_none());
    let config: Coalesce =
        serde_json::from_value(json!({"columns":["a"],"output_column":"c"})).unwrap();
    let output = coalesce(&input, &config).unwrap();
    let contract = contract_from_arrow_schema(output.schema(), resolve_crs).unwrap();
    assert_eq!(contract.geometries.len(), 2);
    assert_eq!(contract.active_geometry_column().unwrap().name, "a");
    let input = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            vec![
                input.schema().field(0).clone(),
                Field::new("plain", DataType::Binary, true),
            ],
            input.schema().metadata().clone(),
        )),
        vec![input.column(0).clone(), input.column(0).clone()],
    )
    .expect("attributo binario");
    let config = json!({"columns":["plain"],"output_column":"a"});
    let output = coalesce(
        &input,
        &serde_json::from_value(config.clone()).expect("config"),
    )
    .expect("sostituzione esplicita");
    let contract = contract_from_arrow_schema(input.schema(), resolve_crs).expect("contratto");
    let analyzed = plenora_kernels_table::analyze::analyze_table_contract(
        "table.coalesce",
        &[contract],
        &config,
        &mut FieldAllocator::default(),
        &Limits::default(),
    )
    .expect("analisi");
    assert!(analyzed.geometries.is_empty());
    assert!(analyzed.active_geometry.is_none());
    assert_eq!(analyzed.schema.fields(), output.schema().fields());
}

#[test]
fn identita_nuove_solo_al_confine_con_tutti_gli_input() {
    for primary in [None, Some("10"), Some("11")] {
        for ambiguous in [false, true] {
            let base = batch(primary);
            let mut fields: Vec<Field> = base
                .schema()
                .fields()
                .iter()
                .map(|f| f.as_ref().clone())
                .collect();
            fields.push(Field::new("x", DataType::Binary, true));
            let a = RecordBatch::try_new(
                Arc::new(Schema::new_with_metadata(
                    fields,
                    base.schema().metadata().clone(),
                )),
                vec![
                    base.column(0).clone(),
                    base.column(1).clone(),
                    base.column(0).clone(),
                ],
            )
            .expect("a");
            let mut fields = vec![Field::new("y", DataType::Binary, true)
                .with_metadata(HashMap::from([("plenora.field_id".into(), "12".into())]))];
            let mut arrays = vec![base.column(0).clone()];
            if ambiguous {
                fields.push(base.schema().field(0).clone());
                arrays.push(base.column(0).clone());
            }
            let b = RecordBatch::try_new(
                Arc::new(Schema::new_with_metadata(
                    fields,
                    HashMap::from([("plenora.contract.version".into(), "1".into())]),
                )),
                arrays,
            )
            .expect("b");
            let pipeline = Pipeline::from_json(
                r#"{"version":1,"inputs":["a","b"],"steps":[],"outputs":["a","b"]}"#,
            )
            .expect("piano");
            let valid = pipeline
                .validate(&[("a", a.schema()), ("b", b.schema())])
                .expect("valida");
            assert!(!valid
                .contratto("a")
                .expect("interno")
                .schema
                .field(2)
                .metadata()
                .contains_key("plenora.field_id"));
            let expected = if ambiguous { "14" } else { "13" };
            let published = valid.schema_uscita("a").expect("pubblico").clone();
            assert_eq!(published.field(2).metadata()["plenora.field_id"], expected);
            assert_eq!(
                valid.schema_uscita("b").expect("b").field(0).metadata()["plenora.field_id"],
                "12"
            );
            let reread =
                contract_from_arrow_schema(published.clone(), resolve_crs).expect("rilettura");
            assert_eq!(
                reread.active_geometry_column().map(|g| g.name.as_str()),
                match primary {
                    Some("10") => Some("a"),
                    Some("11") => Some("b"),
                    _ => None,
                }
            );
            let output = valid
                .run([("a".into(), a), ("b".into(), b)].into())
                .expect("esecuzione");
            assert_eq!(output.outputs[0].1.schema(), published);
        }
    }
}

#[test]
// Matrice di operazioni, primarie e valori dei tre campi geometrici.
#[allow(clippy::too_many_lines)]
fn derivazioni_e_composizioni_preservano_geometrie_e_primaria() {
    use plenora_core::arrow::array::Int64Array;
    use plenora_core::contract::arrow_metadata::geometry_output_field;
    let fields = vec![
        Field::new("id", DataType::Int64, false),
        geometry_output_field("a", "EPSG:32632").unwrap(),
        geometry_output_field("b", "EPSG:3857").unwrap(),
        geometry_output_field("c", "EPSG:32632").unwrap(),
    ];
    let fields = fields
        .into_iter()
        .enumerate()
        .map(|(i, field)| {
            let mut md = field.metadata().clone();
            md.insert("plenora.field_id", (20 + i).to_string());
            field.with_metadata(md)
        })
        .collect::<Vec<_>>();
    let mut bytes = vec![1, 1, 0, 0, 0];
    bytes.extend_from_slice(&0_f64.to_le_bytes());
    bytes.extend_from_slice(&0_f64.to_le_bytes());
    for primary in ["none", "21", "22"] {
        let input = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                fields.clone(),
                HashMap::from([
                    ("plenora.contract.version".into(), "1".into()),
                    ("plenora.primary_geometry".into(), primary.into()),
                ]),
            )),
            vec![
                Arc::new(Int64Array::from(vec![1, 1])),
                Arc::new(BinaryArray::from(vec![Some(bytes.as_slice()), None])),
                Arc::new(BinaryArray::from(vec![None, Some(bytes.as_slice())])),
                Arc::new(BinaryArray::from(vec![
                    Some(bytes.as_slice()),
                    Some(bytes.as_slice()),
                ])),
            ],
        )
        .unwrap();
        for (op, config, names, expected_primary) in [
            (
                "table.coalesce",
                json!({"columns":["a","c"],"output_column":"d"}),
                vec!["a", "b", "c", "d"],
                match primary {
                    "21" => Some("a"),
                    "22" => Some("b"),
                    _ => None,
                },
            ),
            (
                "table.coalesce",
                json!({"columns":["a","c"],"output_column":"a"}),
                vec!["a", "b", "c"],
                match primary {
                    "21" => Some("a"),
                    "22" => Some("b"),
                    _ => None,
                },
            ),
            (
                "table.melt",
                json!({"id_columns":["b"],"value_columns":["a","c"],"value_name":"d"}),
                vec!["b", "d"],
                if primary == "22" { Some("b") } else { None },
            ),
            (
                "table.aggregate",
                json!({"group_by":["id"],"aggregations":[{"column":"a","function":"first","alias":"ga"},{"column":"b","function":"last","alias":"gb"},{"column":"c","function":"first","alias":"gc"}]}),
                vec!["ga", "gb", "gc"],
                None,
            ),
            (
                "table.select_columns",
                json!({"columns":["c","b"]}),
                vec!["c", "b"],
                if primary == "22" { Some("b") } else { None },
            ),
        ] {
            let plan = json!({"version":1,"inputs":["source"],"steps":[{"op":op,"in":["source"],"out":"result","config":config}],"outputs":["result"]});
            let pipeline = Pipeline::from_json(&plan.to_string()).unwrap();
            let valid = pipeline
                .validate(&[("source", input.schema())])
                .unwrap_or_else(|e| panic!("{op} {primary}: {e}"));
            let output = valid
                .run([("source".into(), input.clone())].into())
                .unwrap_or_else(|e| panic!("{op} {primary}: {e}"));
            let batch = &output.outputs[0].1;
            let contract = contract_from_arrow_schema(batch.schema(), resolve_crs).unwrap();
            assert_eq!(
                contract
                    .geometries
                    .iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>(),
                names,
                "{op}"
            );
            assert_eq!(
                contract.active_geometry_column().map(|g| g.name.as_str()),
                expected_primary,
                "{op}"
            );
            if op == "table.aggregate" {
                assert_eq!(
                    batch.column(1).as_ref(),
                    input.column(1).slice(0, 1).as_ref()
                );
                assert_eq!(
                    batch.column(2).as_ref(),
                    input.column(2).slice(1, 1).as_ref()
                );
            }
        }
    }
}

#[test]
fn id_provvisori_non_si_appropriano_degli_id_ricevuti() {
    use plenora_core::arrow::array::Int64Array;
    let input = batch(Some("10"));
    let other = Arc::new(Schema::new_with_metadata(
        vec![Field::new("other", DataType::Int64, false)
            .with_metadata(HashMap::from([("plenora.field_id".into(), "12".into())]))],
        HashMap::from([("plenora.contract.version".into(), "1".into())]),
    ));
    let config = json!({"version":1,"inputs":["source","other"],"steps":[
        {"op":"table.coalesce","in":["source"],"out":"result","config":{"columns":["a"],"output_column":"a"}}
    ],"outputs":["result","other"]});
    let pipeline = Pipeline::from_json(&config.to_string()).unwrap();
    let valid = pipeline
        .validate(&[("source", input.schema()), ("other", other.clone())])
        .unwrap();
    let result = valid.schema_uscita("result").unwrap();
    let id = result
        .field_with_name("a")
        .unwrap()
        .metadata()
        .get("plenora.field_id")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert!(id > 12);
    assert_eq!(
        valid
            .schema_uscita("other")
            .unwrap()
            .field(0)
            .metadata()
            .get("plenora.field_id")
            .unwrap(),
        "12"
    );
    valid
        .run(
            [
                ("source".into(), input.clone()),
                (
                    "other".into(),
                    RecordBatch::try_new(other, vec![Arc::new(Int64Array::from(vec![1]))]).unwrap(),
                ),
            ]
            .into(),
        )
        .unwrap();
    let plan = json!({"version":1,"inputs":["source"],"steps":[
        {"op":"table.cross_join","in":["source","source"],"out":"result","config":{}}
    ],"outputs":["source","result"]});
    let pipeline = Pipeline::from_json(&plan.to_string()).unwrap();
    let valid = pipeline.validate(&[("source", input.schema())]).unwrap();
    assert_eq!(
        valid
            .schema_uscita("source")
            .unwrap()
            .field(0)
            .metadata()
            .get("plenora.field_id")
            .unwrap(),
        "10"
    );
    for field in valid.schema_uscita("result").unwrap().fields() {
        let id = field
            .metadata()
            .get("plenora.field_id")
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(id > 11, "nessuna copia conserva l'identita' ambigua");
    }
    valid.run([("source".into(), input)].into()).unwrap();
}
