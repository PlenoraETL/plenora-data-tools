//! Invarianti plurali e primaria: casi limite e proprieta' deterministiche.
use plenora_core::{
    arrow::schema::{DataType, Field, Schema, SchemaRef},
    contract::{
        arrow_schema::{arrow_schema_from_contract, contract_from_arrow_schema, pubblica_schema},
        DataContract, FieldAllocator, FieldId, FieldOrigin,
    },
    crs::resolve_crs,
    PlenoraError,
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

const PRIMARY: &str = "plenora.primary_geometry";
const ID: &str = "plenora.field_id";

fn geometry(name: &str, id: Option<u32>) -> Field {
    let mut metadata = HashMap::from([("ARROW:extension:name".into(), "geoarrow.wkb".into())]);
    if let Some(id) = id {
        metadata.insert(ID.into(), id.to_string());
    }
    Field::new(name, DataType::Binary, true).with_metadata(metadata)
}
fn schema(fields: Vec<Field>, primary: Option<&str>) -> SchemaRef {
    let mut metadata = HashMap::from([("plenora.contract.version".into(), "1".into())]);
    if let Some(primary) = primary {
        metadata.insert(PRIMARY.into(), primary.into());
    }
    Arc::new(Schema::new_with_metadata(fields, metadata))
}
fn read(schema: SchemaRef) -> DataContract {
    contract_from_arrow_schema(schema, resolve_crs).expect("contratto")
}
fn primary_name(contract: &DataContract) -> Option<&str> {
    contract.active_geometry_column().map(|g| g.name.as_str())
}

#[test]
fn cardinalita_e_forma_canonica_della_primaria() {
    for count in 0..5 {
        for selected in 0..=count {
            let fields: Vec<_> = (0..count)
                .map(|i| geometry(&format!("g{i}"), Some(10 + i)))
                .collect();
            let key = if selected < count {
                Some((10 + selected).to_string())
            } else if count > 0 {
                Some("none".into())
            } else {
                None
            };
            let contract = read(schema(fields.clone(), key.as_deref()));
            let expected = (selected < count).then(|| format!("g{selected}"));
            assert_eq!(primary_name(&contract), expected.as_deref());
            let out = arrow_schema_from_contract(&contract).expect("emissione");
            let expected_key = match (count, selected < count) {
                (1, false) => Some("none".to_owned()),
                (2.., true) => key.clone(),
                _ => None,
            };
            assert_eq!(out.metadata().get(PRIMARY), expected_key.as_ref());
            let again = read(out.clone());
            assert_eq!(primary_name(&again), expected.as_deref());
            assert_eq!(
                arrow_schema_from_contract(&again).expect("idempotenza"),
                out
            );
            assert_eq!(
                arrow_schema_from_contract(&contract).expect("determinismo"),
                out
            );
            assert_eq!(
                contract
                    .geometries
                    .iter()
                    .map(|g| g.field_id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                count as usize
            );
            let absent = read(schema(fields, None));
            assert_eq!(absent.active_geometry.is_some(), count == 1);
        }
    }
}

#[test]
fn selettore_risolve_solo_identita_ricevute_e_non_promuove() {
    let raw = schema(
        vec![geometry("a", Some(10)), geometry("b", None)],
        Some("10"),
    );
    let contract = read(raw.clone());
    assert_eq!(
        contract
            .select_geometry(Some((&raw, FieldId(10))))
            .expect("selettore")
            .name,
        "a"
    );
    assert!(matches!(
        contract.select_geometry(Some((&raw, FieldId(1)))),
        Err(PlenoraError::Schema(_))
    ));
    let published = arrow_schema_from_contract(&contract).expect("completamento");
    let completed = read(published);
    assert!(matches!(
        completed.select_geometry(Some((&raw, FieldId(11)))),
        Err(PlenoraError::Schema(_))
    ));
    let no_primary = read(schema(vec![geometry("a", Some(10))], Some("none")));
    assert!(no_primary.active_geometry_column().is_none());
    assert!(matches!(
        no_primary.select_geometry(None),
        Err(PlenoraError::Schema(_))
    ));
    assert_eq!(
        no_primary
            .select_geometry(Some((&no_primary.schema, FieldId(10))))
            .expect("esplicita")
            .name,
        "a"
    );
}

#[test]
fn primaria_non_iniziale_segue_rinomina_riordino_e_identita_pubblicate() {
    let contract = read(schema(
        vec![geometry("a", Some(10)), geometry("b", Some(20))],
        Some("20"),
    ));
    let output = pubblica_schema(
        &contract.schema,
        &BTreeSet::from([10, 20]),
        &BTreeSet::from([20]),
    )
    .expect("rimappa pubblico");
    assert_eq!(
        output.metadata().get(PRIMARY).map(String::as_str),
        Some("21")
    );
    assert_eq!(primary_name(&read(output)), Some("b"));
    let mut changed = contract;
    changed.geometries.reverse();
    changed.geometries[0].name = "renamed".into();
    changed.field_ids.remove("b");
    changed.field_ids.insert("renamed".into(), FieldId(1));
    changed.schema = schema(
        vec![geometry("renamed", Some(20)), geometry("a", Some(10))],
        Some("20"),
    );
    assert_eq!(
        primary_name(&read(
            arrow_schema_from_contract(&changed).expect("rinomina")
        )),
        Some("renamed")
    );
}

#[test]
fn ricostruzione_non_rimappa_due_volte_con_id_incrocati() {
    for first in 0..5 {
        for second in 0..5 {
            if first == second {
                continue;
            }
            for primary in [first, second] {
                let mut contract =
                    read(schema(vec![geometry("a", None), geometry("b", None)], None));
                contract.geometries[0].field_id = FieldId(first);
                contract.geometries[1].field_id = FieldId(second);
                contract.field_ids =
                    [("a".into(), FieldId(first)), ("b".into(), FieldId(second))].into();
                contract.active_geometry = Some(FieldId(primary));
                let output = DataContract::new(
                    contract.schema.clone(),
                    vec![FieldOrigin::New; 2],
                    contract.geometries.clone(),
                    contract.active_geometry,
                    contract.properties.clone(),
                    &mut FieldAllocator::default(),
                )
                .expect("mappa completa");
                assert_eq!(primary_name(&output), primary_name(&contract));
                assert_eq!(output.geometries[0].field_id, FieldId(0));
                assert_eq!(output.geometries[1].field_id, FieldId(1));
            }
        }
    }
}

#[test]
fn riferimenti_non_conformi_senza_dati_negli_errori() {
    for key in [
        "",
        "01",
        "+1",
        "-1",
        "1.0",
        " 1",
        "1\n",
        "sentinella",
        "999999999999999999999999999999",
    ] {
        let error = contract_from_arrow_schema(
            schema(vec![geometry("a", Some(1))], Some(key)),
            resolve_crs,
        )
        .expect_err("rifiuto");
        assert!(matches!(error, PlenoraError::Schema(_)));
        assert!(!error.to_string().contains("sentinella"));
    }
    for key in ["none", "1"] {
        assert!(matches!(
            contract_from_arrow_schema(schema(vec![], Some(key)), resolve_crs),
            Err(PlenoraError::Schema(_))
        ));
    }
    let attr = Field::new("attr", DataType::Int64, true)
        .with_metadata(HashMap::from([(ID.into(), "1".into())]));
    assert!(matches!(
        contract_from_arrow_schema(
            schema(vec![attr, geometry("a", Some(2))], Some("1")),
            resolve_crs
        ),
        Err(PlenoraError::Schema(_))
    ));
    let mut wrong = geometry("a", Some(1));
    let mut metadata = wrong.metadata().clone();
    metadata.insert(PRIMARY, "none");
    wrong = wrong.with_metadata(metadata);
    assert!(matches!(
        contract_from_arrow_schema(schema(vec![wrong], None), resolve_crs),
        Err(PlenoraError::Schema(_))
    ));
    // Il riferimento non puo' usare un ID che il completamento assegnerebbe.
    assert!(matches!(
        contract_from_arrow_schema(schema(vec![geometry("a", None)], Some("0")), resolve_crs),
        Err(PlenoraError::Schema(_))
    ));
}

#[test]
fn corrispondenza_completa_e_collisioni_rifiutate() {
    let base = read(schema(
        vec![geometry("a", Some(10)), geometry("b", Some(20))],
        Some("20"),
    ));
    let mut omitted = base.clone();
    omitted.geometries.remove(0);
    assert!(matches!(omitted.validate(), Err(PlenoraError::Schema(_))));
    let mut duplicate = base.clone();
    duplicate.geometries.push(duplicate.geometries[0].clone());
    assert!(matches!(duplicate.validate(), Err(PlenoraError::Schema(_))));
    let mut collision = base.clone();
    collision.geometries[1].field_id = FieldId(0);
    collision.field_ids.insert("b".into(), FieldId(0));
    assert!(matches!(collision.validate(), Err(PlenoraError::Schema(_))));
    let mut reordered = base.clone();
    reordered.geometries.reverse();
    assert!(matches!(reordered.validate(), Err(PlenoraError::Schema(_))));
    let mut dangling = base;
    dangling.active_geometry = Some(FieldId(99));
    assert!(matches!(dangling.validate(), Err(PlenoraError::Schema(_))));
    for fields in [
        vec![geometry("same", Some(1)), geometry("same", Some(2))],
        vec![geometry("a", Some(1)), geometry("b", Some(1))],
    ] {
        assert!(matches!(
            contract_from_arrow_schema(schema(fields, None), resolve_crs),
            Err(PlenoraError::Schema(_))
        ));
    }
}

#[test]
fn precedenza_identita_e_vocabolario_prima_della_primaria_prima_del_crs() {
    let repeated = schema(
        vec![geometry("a", Some(1)), geometry("b", Some(1))],
        Some("bad"),
    );
    let error = contract_from_arrow_schema(repeated, resolve_crs).expect_err("id ripetuti");
    assert!(error.to_string().contains("plenora.field_id"));
    let wrong_type = geometry("a", Some(1)).with_data_type(DataType::Utf8);
    let error = contract_from_arrow_schema(schema(vec![wrong_type], Some("bad")), resolve_crs)
        .expect_err("tipo");
    assert!(!error.to_string().contains(PRIMARY));
    let mut field = geometry("a", Some(1));
    let mut metadata = field.metadata().clone();
    metadata.insert("geo", r#"{"crs":"EPSG:999999"}"#);
    field = field.with_metadata(metadata);
    let error = contract_from_arrow_schema(schema(vec![field], Some("bad")), resolve_crs)
        .expect_err("primaria");
    assert!(error.to_string().contains(PRIMARY));
}

#[test]
fn vocabolario_del_campo_precede_primaria_anche_con_crs_malformato() {
    let mut field = geometry("a", Some(1));
    let mut metadata = field.metadata().clone();
    metadata.insert("plenora.geometry.crs_definition", "sentinella-crs");
    metadata.insert("plenora.geometry.encoding", "bad");
    field = field.with_metadata(metadata);
    let error = contract_from_arrow_schema(schema(vec![field], Some("bad")), resolve_crs)
        .expect_err("encoding");
    assert!(matches!(error, PlenoraError::Schema(_)));
    assert!(error.to_string().contains("encoding"));
    assert!(!error.to_string().contains("sentinella-crs"));
    let mut field = geometry("a", Some(1));
    let mut metadata = field.metadata().clone();
    metadata.insert("ARROW:extension:metadata", "[");
    field = field.with_metadata(metadata);
    let error = contract_from_arrow_schema(schema(vec![field], Some("bad")), resolve_crs)
        .expect_err("estensione");
    assert!(matches!(error, PlenoraError::Schema(_)));
    assert!(!error.to_string().contains(PRIMARY));
}

#[test]
fn eliminazione_della_primaria_non_promuove_la_secondaria() {
    let mut contract = read(schema(
        vec![geometry("a", Some(1)), geometry("b", Some(2))],
        Some("1"),
    ));
    contract.schema = Arc::new(Schema::new_with_metadata(
        vec![contract.schema.field(1).clone()],
        contract.schema.metadata().clone(),
    ));
    contract.geometries.remove(0);
    contract.field_ids.remove("a");
    contract.active_geometry = None;
    let output = arrow_schema_from_contract(&contract).expect("projection");
    assert_eq!(
        output.metadata().get(PRIMARY).map(String::as_str),
        Some("none")
    );
    assert!(read(output).active_geometry_column().is_none());
}

#[test]
fn canone_interno_assegna_solo_identita_necessaria_alla_primaria() {
    for primary in [None, Some("10")] {
        let raw = schema(
            vec![
                geometry("a", Some(10)),
                geometry("b", None),
                Field::new("x", DataType::Int64, true),
            ],
            primary,
        );
        let contract = read(raw.clone());
        let internal = arrow_schema_from_contract(&contract).expect("canone interno");
        for (before, after) in raw.fields().iter().zip(internal.fields()) {
            assert_eq!(before.metadata().get(ID), after.metadata().get(ID));
        }
        let published = pubblica_schema(&internal, &BTreeSet::from([10, 12]), &BTreeSet::new())
            .expect("confine");
        assert_eq!(published.field(1).metadata()[ID], "13");
        assert_eq!(published.field(2).metadata()[ID], "14");
    }
    let mut synthetic = read(schema(vec![geometry("a", None), geometry("b", None)], None));
    synthetic.active_geometry = Some(synthetic.geometries[0].field_id);
    let output = arrow_schema_from_contract(&synthetic).unwrap();
    assert_eq!(output.field(0).metadata()[ID], "0");
    assert!(!output.field(1).metadata().contains_key(ID));
    assert_eq!(primary_name(&read(output)), Some("a"));
}

#[test]
fn emissione_primaria_senza_id_canonica_e_fail_closed() {
    use plenora_core::contract::arrow_schema::{emit_primary_geometry, primary_geometry_position};
    for position in 0..3 {
        for existing in [None, Some(17), Some(u32::MAX - 1)] {
            let mut fields = vec![
                geometry("secondary", existing),
                Field::new("value", DataType::Int64, true),
            ];
            fields.insert(position, geometry("primary", None));
            let input = schema(fields, None);
            let output = emit_primary_geometry(&input, Some(position)).unwrap();
            let expected_id = existing.map_or(0, |id| id + 1).to_string();
            assert_eq!(output.field(position).metadata()[ID], expected_id);
            assert_eq!(output.metadata()[PRIMARY], expected_id);
            assert_eq!(primary_geometry_position(&output).unwrap(), Some(position));
            for index in 0..3 {
                if index != position {
                    assert_eq!(input.field(index), output.field(index));
                }
            }
            assert_eq!(
                emit_primary_geometry(&output, Some(position)).unwrap(),
                output
            );
            assert_eq!(
                emit_primary_geometry(&input, Some(position)).unwrap(),
                output
            );
            // Nessuna elezione quando la provenienza non indica una primaria.
            assert_eq!(emit_primary_geometry(&input, None).unwrap(), input);
        }
    }
    let exhausted = schema(
        vec![geometry("other", Some(u32::MAX)), geometry("primary", None)],
        None,
    );
    assert!(matches!(
        emit_primary_geometry(&exhausted, Some(1)),
        Err(PlenoraError::Unsupported(_))
    ));
    for position in [2, usize::MAX] {
        assert!(matches!(
            emit_primary_geometry(&exhausted, Some(position)),
            Err(PlenoraError::Schema(_))
        ));
    }
    let single = schema(vec![geometry("primary", None)], None);
    assert_eq!(emit_primary_geometry(&single, Some(0)).unwrap(), single);
    // Il riferimento ricevuto si risolve prima del completamento.
    let dangling = schema(vec![geometry("a", None), geometry("b", None)], Some("0"));
    assert!(matches!(
        pubblica_schema(&dangling, &BTreeSet::new(), &BTreeSet::new()),
        Err(PlenoraError::Schema(_))
    ));
}

#[test]
fn emissione_none_extension_only_aggiunge_versione() {
    use plenora_core::contract::arrow_schema::emit_primary_geometry;
    let input = Schema::new(vec![geometry("secondary", None)]);
    let output = emit_primary_geometry(&input, None).unwrap();
    assert_eq!(output.metadata()[PRIMARY], "none");
    assert_eq!(output.metadata()["plenora.contract.version"], "1");
    assert!(primary_name(&read(output)).is_none());
}

#[test]
fn emissione_rifiuta_primaria_senza_estensione_o_non_binaria() {
    use plenora_core::contract::arrow_schema::emit_primary_geometry;
    for field in [
        Field::new("invalid", DataType::Binary, true).with_metadata(HashMap::from([(
            "plenora.geometry.encoding".into(),
            "wkb".into(),
        )])),
        Field::new("invalid", DataType::Utf8, true).with_metadata(HashMap::from([(
            "ARROW:extension:name".into(),
            "geoarrow.wkb".into(),
        )])),
    ] {
        let input = Schema::new(vec![field, geometry("other", None)]);
        assert!(matches!(
            emit_primary_geometry(&input, Some(0)),
            Err(PlenoraError::Schema(_))
        ));
    }
}
