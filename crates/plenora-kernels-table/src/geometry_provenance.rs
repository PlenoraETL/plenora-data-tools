//! Provenienza geometrica degli schemi prodotti dagli adapter diretti.
use plenora_core::arrow::schema::{Field, Schema};
use plenora_core::contract::arrow_metadata::{
    field_declares_wkb_geometry, schema_has_canonical_keys, PLENORA_CONTRACT_VERSION,
    PLENORA_CONTRACT_VERSION_KEY,
};
use plenora_core::contract::arrow_schema::{
    emit_primary_geometry, primary_geometry_position, PRIMARY_GEOMETRY_KEY,
};
use plenora_core::{PlenoraError, Result};
use std::collections::{BTreeSet, HashMap};

/// Corrispondenza sorgente/uscita per posizione, senza uguaglianza di ID fra ingressi.
pub fn compose(
    inputs: &[&Schema],
    fields: Vec<Field>,
    mapping: &[(usize, usize, usize)],
    vertical: bool,
    metadata: HashMap<String, String>,
) -> Result<Schema> {
    let primaries = inputs
        .iter()
        .map(|s| primary_geometry_position(s))
        .collect::<Result<Vec<_>>>()?;
    let mappings: Vec<_> = mapping
        .iter()
        .copied()
        .filter(|(input, source, output)| {
            field_declares_wkb_geometry(inputs[*input].field(*source))
                && field_declares_wkb_geometry(&fields[*output])
        })
        .collect();
    let primary = if vertical {
        let targets = primaries
            .iter()
            .enumerate()
            .map(|(i, primary)| {
                primary.and_then(|p| {
                    mappings
                        .iter()
                        .find(|(input, source, _)| *input == i && *source == p)
                        .map(|m| m.2)
                })
            })
            .collect::<Vec<_>>();
        targets
            .first()
            .copied()
            .flatten()
            .filter(|p| targets.iter().all(|t| *t == Some(*p)))
    } else {
        let sources: BTreeSet<_> = mappings.iter().map(|m| m.0).collect();
        if sources.len() == 1 {
            let input = *sources
                .first()
                .ok_or_else(|| PlenoraError::Schema("provenienza geometrica assente".into()))?;
            primaries[input].and_then(|p| {
                mappings
                    .iter()
                    .find(|m| m.0 == input && m.1 == p)
                    .map(|m| m.2)
            })
        } else {
            None
        }
    };
    emit(fields, primary, metadata)
}

/// Emette lo stato della primaria e disambigua le identita' pubbliche dell'uscita.
pub fn emit(
    fields: Vec<Field>,
    primary: Option<usize>,
    metadata: HashMap<String, String>,
) -> Result<Schema> {
    let schema = prepare(fields, metadata)?;
    Ok(emit_primary_geometry(&schema, primary)?.as_ref().clone())
}

/// Disambigua gli ID senza materializzare geometrie dichiarate solo dal contratto.
pub fn prepare(mut fields: Vec<Field>, mut metadata: HashMap<String, String>) -> Result<Schema> {
    // La primaria resta nel contratto logico finche' lo schema e' completo.
    metadata.remove(PRIMARY_GEOMETRY_KEY);
    // Identita' pubbliche duplicate da ingressi distinti diventano nuove identita'.
    let mut reserved = BTreeSet::new();
    for field in &fields {
        if let Some(id) = field
            .metadata()
            .get(plenora_core::contract::arrow_metadata::PLENORA_FIELD_ID_KEY)
        {
            let id = id
                .parse::<u32>()
                .map_err(|_| PlenoraError::Schema("identita' pubblica non valida".into()))?;
            reserved.insert(id);
        }
    }
    let mut seen = BTreeSet::new();
    for field in &mut fields {
        let old = field
            .metadata()
            .get(plenora_core::contract::arrow_metadata::PLENORA_FIELD_ID_KEY)
            .cloned();
        let duplicate = old.as_ref().is_some_and(|id| !seen.insert(id.clone()));
        if duplicate {
            let id = reserved
                .last()
                .copied()
                .map_or(Some(0), |id| id.checked_add(1))
                .ok_or_else(|| PlenoraError::Schema("identita' pubbliche esaurite".into()))?;
            reserved.insert(id);
            seen.insert(id.to_string());
            let mut md = field.metadata().clone();
            md.insert(
                plenora_core::contract::arrow_metadata::PLENORA_FIELD_ID_KEY,
                id.to_string(),
            );
            *field = field.clone().with_metadata(md);
            metadata.insert("plenora.contract.version".into(), "1".into());
        }
    }
    // Anche una dichiarazione derivata nuova richiede la versione del trasporto,
    // senza inventare identita pubbliche per i campi.
    let schema = Schema::new_with_metadata(fields, metadata);
    if schema_has_canonical_keys(&schema)
        && !schema.metadata().contains_key(PLENORA_CONTRACT_VERSION_KEY)
    {
        let mut metadata = schema.metadata().clone();
        metadata.insert(
            PLENORA_CONTRACT_VERSION_KEY,
            PLENORA_CONTRACT_VERSION.to_string(),
        );
        return Ok(schema.with_metadata(metadata));
    }
    Ok(schema)
}

/// Colonne conservate per nome; le derivate con un nome nuovo non sono promosse.
pub fn unary(input: &Schema, fields: Vec<Field>) -> Result<Schema> {
    let mapping = fields
        .iter()
        .enumerate()
        .filter_map(|(output, field)| {
            input
                .index_of(field.name())
                .ok()
                .map(|source| (0, source, output))
        })
        .collect::<Vec<_>>();
    compose(
        &[input],
        fields,
        &mapping,
        false,
        input.metadata().clone().into_iter().collect(),
    )
}

/// Fusioni verticali per nome: ogni sorgente deve contribuire la stessa primaria.
pub fn vertical(
    inputs: &[&Schema],
    fields: Vec<Field>,
    metadata: HashMap<String, String>,
) -> Result<Schema> {
    let mut mapping = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        for (output, field) in fields.iter().enumerate() {
            if let Ok(source) = input.index_of(field.name()) {
                mapping.push((i, source, output));
            }
        }
    }
    compose(inputs, fields, &mapping, true, metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plenora_core::arrow::schema::DataType;
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct Input {
        geometry_ids: Vec<String>,
        primary: Option<String>,
    }
    #[derive(Deserialize)]
    struct Mapping {
        input: usize,
        field: String,
        output: String,
    }
    #[derive(Deserialize)]
    struct Vector {
        operation: String,
        inputs: Vec<Input>,
        mapping: Vec<Mapping>,
        expected_primary: Option<String>,
        expected_key: Option<String>,
    }
    fn field(id: &str) -> Field {
        Field::new(id, DataType::Binary, true).with_metadata(HashMap::from([
            ("ARROW:extension:name".into(), "geoarrow.wkb".into()),
            (
                plenora_core::contract::arrow_metadata::PLENORA_FIELD_ID_KEY.into(),
                id.into(),
            ),
        ]))
    }
    #[test]
    fn contratti_parziali_emettono_primaria_solo_dopo_materializzazione() {
        use plenora_core::contract::arrow_schema::{
            arrow_schema_from_contract, contract_from_arrow_schema,
        };
        use plenora_core::contract::FieldAllocator;
        use plenora_core::crs::resolve_crs;
        use std::sync::Arc;
        for primary in ["none", "10", "20"] {
            for materialized in 0..4 {
                let raw = Arc::new(Schema::new_with_metadata(
                    vec![field("10"), field("20")],
                    HashMap::from([
                        ("plenora.contract.version".into(), "1".into()),
                        (PRIMARY_GEOMETRY_KEY.into(), primary.into()),
                    ]),
                ));
                let mut input = contract_from_arrow_schema(raw, resolve_crs).unwrap();
                let fields = input
                    .schema
                    .fields()
                    .iter()
                    .enumerate()
                    .map(|(i, field)| {
                        let mut md = field.metadata().clone();
                        if materialized & (1 << i) == 0 {
                            md.remove("ARROW:extension:name");
                        }
                        field.as_ref().clone().with_metadata(md)
                    })
                    .collect::<Vec<_>>();
                input.schema = Arc::new(Schema::new_with_metadata(
                    fields,
                    input.schema.metadata().clone(),
                ));
                let output = crate::analyze::analyze_table_contract(
                    "table.reorder_columns",
                    std::slice::from_ref(&input),
                    &serde_json::json!({"columns":["20","10"]}),
                    &mut FieldAllocator::default(),
                    &crate::Limits::default(),
                )
                .unwrap();
                let expected = (primary != "none").then_some(primary);
                assert_eq!(
                    output.active_geometry_column().map(|g| g.name.as_str()),
                    expected
                );
                for field in output.schema.fields() {
                    assert_eq!(
                        field.metadata(),
                        input
                            .schema
                            .field_with_name(field.name())
                            .unwrap()
                            .metadata()
                    );
                }
                let emitted = arrow_schema_from_contract(&output).unwrap();
                assert_eq!(
                    primary_geometry_position(&emitted)
                        .unwrap()
                        .map(|p| emitted.field(p).name().as_str()),
                    expected
                );
                let reread = contract_from_arrow_schema(emitted, resolve_crs).unwrap();
                assert_eq!(reread.geometries.len(), 2);
                assert_eq!(
                    reread.active_geometry_column().map(|g| g.name.as_str()),
                    expected
                );
            }
        }
    }

    #[test]
    fn vettori_contratti_primary_composition_v1() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/riferimenti/primary-composition-v1");
        let mut paths = std::fs::read_dir(directory)
            .unwrap()
            .map(|p| p.unwrap().path())
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths.len(), 13);
        for path in paths {
            let vector: Vector = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let inputs = vector
                .inputs
                .iter()
                .map(|input| {
                    let metadata = if input.geometry_ids.is_empty() {
                        HashMap::new()
                    } else {
                        HashMap::from([(
                            PRIMARY_GEOMETRY_KEY.into(),
                            input.primary.clone().unwrap_or_else(|| "none".into()),
                        )])
                    };
                    Schema::new_with_metadata(
                        input
                            .geometry_ids
                            .iter()
                            .map(|id| field(id))
                            .collect::<Vec<_>>(),
                        metadata,
                    )
                })
                .collect::<Vec<_>>();
            let outputs = vector
                .mapping
                .iter()
                .map(|m| m.output.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let mapping = vector
                .mapping
                .iter()
                .map(|m| {
                    (
                        m.input,
                        inputs[m.input].index_of(&m.field).unwrap(),
                        outputs.iter().position(|id| id == &m.output).unwrap(),
                    )
                })
                .collect::<Vec<_>>();
            for reverse in [false, true] {
                let mut inputs = inputs.iter().collect::<Vec<_>>();
                let mut mapping = mapping.clone();
                if reverse {
                    inputs.reverse();
                    for m in &mut mapping {
                        m.0 = inputs.len() - 1 - m.0;
                    }
                }
                let schema = compose(
                    &inputs,
                    outputs.iter().map(|id| field(id)).collect(),
                    &mapping,
                    vector.operation != "join",
                    HashMap::new(),
                )
                .unwrap();
                let actual = primary_geometry_position(&schema)
                    .unwrap()
                    .map(|p| schema.field(p).name().clone());
                assert_eq!(actual, vector.expected_primary, "{}", path.display());
                assert_eq!(
                    schema.metadata().get(PRIMARY_GEOMETRY_KEY),
                    vector.expected_key.as_ref(),
                    "{}",
                    path.display()
                );
                assert_eq!(
                    schema,
                    compose(
                        &inputs,
                        outputs.iter().map(|id| field(id)).collect(),
                        &mapping,
                        vector.operation != "join",
                        HashMap::new()
                    )
                    .unwrap()
                );
            }
        }
    }
}

#[cfg(test)]
mod column_reference_tests {
    use crate::Limits;
    use plenora_core::arrow::{
        array::{BinaryArray, Int64Array, RecordBatch},
        schema::{DataType, Field, Schema},
    };
    use plenora_core::contract::{
        arrow_metadata::geometry_output_field, arrow_schema::contract_from_arrow_schema,
        FieldAllocator,
    };
    use plenora_core::{crs::resolve_crs, ErrorCategory};
    use serde_json::json;
    use std::sync::Arc;

    fn input(name: &str, crs: &str) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                geometry_output_field(name, crs).unwrap(),
                Field::new("t", DataType::Int64, false),
            ])),
            vec![
                Arc::new(BinaryArray::from(vec![Some(b"x".as_slice())])),
                Arc::new(Int64Array::from(vec![1])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn chiavi_con_nomi_diversi_verificano_il_riferimento_anche_senza_righe() {
        for empty in [false, true] {
            let mut left = input("g", "EPSG:32632");
            let mut right = input("h", "EPSG:3857");
            if empty {
                left = left.slice(0, 0);
                right = right.slice(0, 0);
            }
            for op in [
                "table.join",
                "table.semi_join",
                "table.anti_join",
                "table.assert_foreign_key",
                "table.reconcile",
                "table.asof_join",
            ] {
                let config = if op == "table.asof_join" {
                    json!({"left_on":"t","right_on":"t","left_by":["g"],"right_by":["h"]})
                } else {
                    json!({"left_keys":["g"],"right_keys":["h"]})
                };
                let result = match op {
                    "table.join" => crate::joins::join(
                        &left,
                        &right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &Limits::default(),
                    ),
                    "table.semi_join" => crate::joins::semi_join(
                        &left,
                        &right,
                        &serde_json::from_value(config.clone()).unwrap(),
                    ),
                    "table.anti_join" => crate::joins::anti_join(
                        &left,
                        &right,
                        &serde_json::from_value(config.clone()).unwrap(),
                    ),
                    "table.asof_join" => crate::joins::asof_join(
                        &left,
                        &right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &Limits::default(),
                    ),
                    "table.assert_foreign_key" => crate::governance::assert_foreign_key(
                        &left,
                        &right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &Limits::default(),
                    ),
                    _ => crate::governance::reconcile(
                        &left,
                        &right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &Limits::default(),
                    ),
                };
                assert_eq!(result.unwrap_err().category(), ErrorCategory::Crs, "{op}");
                let inputs = [
                    contract_from_arrow_schema(left.schema(), resolve_crs).unwrap(),
                    contract_from_arrow_schema(right.schema(), resolve_crs).unwrap(),
                ];
                assert_eq!(
                    crate::analyze::analyze_table_contract(
                        op,
                        &inputs,
                        &config,
                        &mut FieldAllocator::default(),
                        &Limits::default()
                    )
                    .unwrap_err()
                    .category(),
                    ErrorCategory::Crs,
                    "{op}"
                );
            }
            let config =
                serde_json::from_value(json!({"left_keys":["t"],"right_keys":["t"]})).unwrap();
            assert!(
                crate::joins::semi_join(&left, &right, &config).is_ok(),
                "geometrie passanti indipendenti"
            );
        }
    }

    #[test]
    fn concat_per_nome_disambigua_id_logici_di_sorgenti_distinte() {
        let inputs = [input("g", "EPSG:32632"), input("h", "EPSG:3857")]
            .map(|b| contract_from_arrow_schema(b.schema(), resolve_crs).unwrap());
        assert_eq!(
            inputs[0].geometries[0].field_id,
            inputs[1].geometries[0].field_id
        );
        let output = crate::analyze::analyze_table_contract(
            "table.concat_by_name",
            &inputs,
            &json!({"strict":false}),
            &mut FieldAllocator::default(),
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(output.geometries.len(), 2);
        assert_ne!(output.geometries[0].field_id, output.geometries[1].field_id);
        assert!(output.active_geometry.is_none());
        assert!(output.geometries.iter().all(|g| g.nullable));
    }
}
