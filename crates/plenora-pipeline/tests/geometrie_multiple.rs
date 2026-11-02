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
fn tutte_le_analisi_non_migrate_rifiutano_multigeometria_prima_della_config() {
    for primary in [None, Some("11")] {
        let input =
            contract_from_arrow_schema(batch(primary).schema(), resolve_crs).expect("contratto");
        for operation in CATALOG {
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
fn adapter_tabellari_rifiutano_prima_di_perdere_colonne_o_riferimenti() {
    use plenora_kernels_table::{
        analysis::{sample, Sample},
        columns::{drop_columns, select_columns, DropColumns, SelectColumns},
    };
    let input = batch(Some("11"));
    assert_eq!(
        drop_columns(
            &input,
            &DropColumns {
                columns: vec!["b".into()]
            }
        )
        .expect_err("drop")
        .category(),
        ErrorCategory::Unsupported
    );
    assert_eq!(
        select_columns(
            &input,
            &SelectColumns {
                columns: vec!["a".into()]
            }
        )
        .expect_err("select")
        .category(),
        ErrorCategory::Unsupported
    );
    let config: Sample = serde_json::from_value(json!({"n":1})).expect("config");
    assert_eq!(
        sample(&input, &config).expect_err("sample").category(),
        ErrorCategory::Unsupported
    );
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
fn output_plurali_da_input_singolo_rifiutati_e_sostituzione_tabellare_coerente() {
    use plenora_kernels_table::{
        quality::{coalesce, Coalesce},
        reshape::{transpose, Transpose},
        Limits,
    };
    let input = batch(None).project(&[0]).expect("una geometria");
    let transpose_config: Transpose = serde_json::from_value(json!({})).expect("config");
    assert_eq!(
        transpose(&input, &transpose_config, &Limits::default())
            .expect_err("output plurale")
            .category(),
        ErrorCategory::Unsupported
    );
    let config: Coalesce =
        serde_json::from_value(json!({"columns":["a"],"output_column":"c"})).expect("config");
    assert_eq!(
        coalesce(&input, &config)
            .expect_err("seconda geometria")
            .category(),
        ErrorCategory::Unsupported
    );
    let contract = contract_from_arrow_schema(input.schema(), resolve_crs).expect("contratto");
    let result = plenora_kernels_table::analyze::analyze_table_contract(
        "table.coalesce",
        &[contract],
        &json!({"columns":["a"],"output_column":"c"}),
        &mut FieldAllocator::default(),
        &Limits::default(),
    );
    assert_eq!(
        result.expect_err("seconda geometria in analisi").category(),
        ErrorCategory::Unsupported
    );
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
