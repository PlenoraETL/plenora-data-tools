//! I tredici vettori di composizione attraversano kernel diretti e pipeline.
use plenora_core::arrow::{
    array::{ArrayRef, BinaryArray, Int64Array, RecordBatch},
    DataType, Field, Metadata, Schema,
};
use plenora_core::contract::arrow_metadata::geometry_output_field;
use plenora_core::contract::arrow_schema::primary_geometry_position;
use plenora_kernels_table::{joins, setops, Limits};
use plenora_pipeline::Pipeline;
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};

fn mapped_input(vector: &Value, side: usize, vertical: bool) -> RecordBatch {
    let mapping = vector["mapping"].as_array().unwrap();
    let names: BTreeSet<_> = mapping
        .iter()
        .filter(|m| vertical || m["input"].as_u64().unwrap() == u64::try_from(side).unwrap())
        .map(|m| m["output"].as_str().unwrap())
        .collect();
    let mut fields = vec![Field::new("key", DataType::Int64, false)
        .with_metadata(Metadata::from_iter([("plenora.field_id", "1000")]))];
    let mut columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(vec![1]))];
    let mut primary = None;
    for (index, name) in names.into_iter().enumerate() {
        let source = mapping.iter().find(|m| {
            m["input"].as_u64().unwrap() == u64::try_from(side).unwrap() && m["output"] == name
        });
        let id = source.map_or_else(
            || (2000 + index).to_string(),
            |m| m["field"].as_str().unwrap().to_owned(),
        );
        if source.is_some_and(|m| m["field"] == vector["inputs"][side]["primary"]) {
            primary = Some(id.clone());
        }
        let field = geometry_output_field(name, "EPSG:32632").unwrap();
        let mut md = field.metadata().clone();
        md.insert("plenora.field_id", id);
        fields.push(field.with_metadata(md));
        columns.push(Arc::new(BinaryArray::from(vec![None::<&[u8]>])));
    }
    let mut metadata = Metadata::from_iter([("plenora.contract.version", "1")]);
    if fields.len() > 1 {
        metadata.insert(
            "plenora.primary_geometry",
            primary.unwrap_or_else(|| "none".into()),
        );
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        columns,
    )
    .unwrap()
}

#[test]
fn tredici_vettori_kernel_diretti_pipeline_e_inversione() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../plenora-kernels-table/tests/riferimenti/primary-composition-v1");
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 13);
    for path in paths {
        let vector: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let vertical = vector["operation"] != "join";
        let inputs = [
            mapped_input(&vector, 0, vertical),
            mapped_input(&vector, 1, vertical),
        ];
        let ops: &[&str] = if vertical {
            &[
                "table.concat",
                "table.concat_by_name",
                "table.union_distinct",
            ]
        } else {
            &["table.join"]
        };
        for reverse in [false, true] {
            let (left, right) = if reverse {
                (&inputs[1], &inputs[0])
            } else {
                (&inputs[0], &inputs[1])
            };
            for &op in ops {
                let config = if vertical {
                    json!({})
                } else {
                    json!({"left_keys":["key"],"right_keys":["key"],"how":"inner"})
                };
                let limits = Limits::default();
                let direct = match op {
                    "table.concat" => joins::concat(
                        left,
                        right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                    "table.concat_by_name" => joins::concat_by_name(
                        &[left, right],
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                    "table.union_distinct" => setops::union_distinct(
                        left,
                        right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                    _ => joins::join(
                        left,
                        right,
                        &serde_json::from_value(config.clone()).unwrap(),
                        &limits,
                    ),
                }
                .unwrap();
                let plan = Pipeline::from_json(&json!({"version":1,"inputs":["l","r"],"steps":[{"op":op,"in":["l","r"],"out":"o","config":config}],"outputs":["o"]}).to_string()).unwrap();
                let result = plan
                    .validate(&[("l", left.schema()), ("r", right.schema())])
                    .unwrap()
                    .run(vec![
                        ("l".into(), left.clone()),
                        ("r".into(), right.clone()),
                    ])
                    .unwrap();
                for output in [&direct, &result.outputs[0].1] {
                    let actual = primary_geometry_position(&output.schema())
                        .unwrap()
                        .map(|p| output.schema().field(p).name().clone());
                    // Il kernel join aggiunge suffissi fissi ai nomi del vettore.
                    let actual = actual.map(|name| {
                        if vertical {
                            name
                        } else {
                            name.strip_suffix("_L")
                                .or_else(|| name.strip_suffix("_R"))
                                .unwrap()
                                .to_owned()
                        }
                    });
                    assert_eq!(
                        actual.as_deref(),
                        vector["expected_primary"].as_str(),
                        "{} {op} reverse={reverse}",
                        path.display()
                    );
                }
            }
        }
    }
}
