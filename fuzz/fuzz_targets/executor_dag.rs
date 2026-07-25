#![no_main]

//! Executor DAG (Fase 2A-4): piani v4 arbitrari ma validi (catene unarie,
//! un nodo binario, catena geo buffer->area) su piccoli input generati dal
//! payload. Pipeline completa `planner::validate` -> `execute` -> drenaggio
//! dello stream. Invarianti: mai panic; batch internamente coerenti (ogni
//! colonna lunga quanto `num_rows`); le metriche cumulative dell'output
//! corrispondono alle righe effettivamente emesse; righe totali limitate.

use std::sync::Arc;

use arrow_array::{ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use geo::{Geometry, Point};
use geozero::{CoordDimensions, ToWkb};
use libfuzzer_sys::fuzz_target;
use plenora_core::contract::{
    ContractProperties, DataContract, FieldId, GeometryColumnContract, GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_engine::planner::validate;
use plenora_engine::{execute, Input, Inputs, RuntimeContext};
use plenora_kernels_geo::arrow_adapter::geometry_output_field;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Fixture tabellari e geometriche (<= 48 righe)
// ---------------------------------------------------------------------------

fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("num", DataType::Float64, true),
        Field::new("name", DataType::Utf8, true),
        Field::new("group", DataType::Utf8, true),
    ]))
}

fn table_contract() -> DataContract {
    DataContract::tabular(table_schema())
}

fn table_batch(payload: &[u8], salt: u8) -> RecordBatch {
    let rows = payload.len().min(48);
    let ids = (0..rows)
        .map(|row| i64::from(payload[row].wrapping_add(salt)))
        .collect::<Vec<_>>();
    let nums = (0..rows)
        .map(|row| (row % 4 != 0).then(|| f64::from(payload[row]) * 0.25))
        .collect::<Vec<_>>();
    let names = (0..rows)
        .map(|row| (row % 5 != 0).then(|| format!("n{}", payload[row])))
        .collect::<Vec<_>>();
    let groups = (0..rows)
        .map(|row| Some(format!("g{}", payload[row] % 4)))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        table_schema(),
        vec![
            Arc::new(Int64Array::from(ids)) as ArrayRef,
            Arc::new(Float64Array::from(nums)) as ArrayRef,
            Arc::new(StringArray::from(names)) as ArrayRef,
            Arc::new(StringArray::from(groups)) as ArrayRef,
        ],
    )
    .expect("fixture tabellare")
}

fn geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        geometry_output_field("geom", "EPSG:32632").expect("campo geometria"),
    ]))
}

fn geo_contract() -> DataContract {
    DataContract::new(
        geo_schema(),
        vec![GeometryColumnContract {
            field_id: FieldId(3),
            name: "geom".to_owned(),
            crs: ResolvedCrs::from_resolved_parts(
                "EPSG:32632".to_owned(),
                json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
                CrsKind::Projected,
                Some(1.0),
            ),
            dimensions: GeometryDimensions::Xy,
            nullable: true,
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto geometrico")
}

fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    Geometry::Point(Point::new(x, y))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb fixture")
}

fn geo_batch(payload: &[u8]) -> RecordBatch {
    let rows = payload.len().min(24);
    let ids = (0..rows).map(|row| row as i64).collect::<Vec<_>>();
    let cells = (0..rows)
        .map(|row| {
            (row % 6 != 0).then(|| {
                point_wkb(
                    f64::from(payload[row]) - 128.0,
                    f64::from(payload[(row * 7 + 1) % payload.len().max(1)]) - 128.0,
                )
            })
        })
        .collect::<Vec<_>>();
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|c| c.as_deref()).collect();
    RecordBatch::try_new(
        geo_schema(),
        vec![
            Arc::new(Int64Array::from(ids)) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("fixture geometrica")
}

// ---------------------------------------------------------------------------
// Esecuzione e invarianti
// ---------------------------------------------------------------------------

fn run(plan: &Value, inputs: Inputs, contracts: &[(String, DataContract)]) {
    let Ok(graph) = validate(&plan.to_string(), contracts) else {
        return; // piano rifiutato in validazione: esito lecito
    };
    let output = match execute(&graph, inputs, RuntimeContext::default()) {
        Ok(output) => output,
        Err(_) => return,
    };
    let Ok((batches, metrics)) = output.collect_batches() else {
        return;
    };
    let mut total = 0_usize;
    for batch in &batches {
        assert!(batch
            .columns()
            .iter()
            .all(|column| column.len() == batch.num_rows()));
        total += batch.num_rows();
    }
    assert!(total <= 10_000, "righe output non limitate: {total}");
    assert_eq!(
        total,
        metrics.output_rows as usize,
        "metriche output_rows incoerenti"
    );
}

fuzz_target!(|payload: &[u8]| {
    if payload.len() < 8 {
        return;
    }
    match payload[0] % 3 {
        0 => {
            // Catena unaria di 1..=3 nodi che preservano le colonne,
            // terminata opzionalmente da `table.aggregate`.
            let preserving = [
                json!({"op": "table.filter", "config": {"column": "id", "operator": ">", "value": i64::from(payload[1])}}),
                json!({"op": "table.sort", "config": {"columns": ["id"], "ascending": payload[2] % 2 == 0}}),
                json!({"op": "table.add_row_number", "config": {"output_column": "row"}}),
                json!({"op": "table.string_length", "config": {"column": "name", "output_column": "len"}}),
                json!({"op": "table.distinct", "config": {"subset": ["group"], "keep": "first"}}),
            ];
            let length = 1 + payload[1] as usize % 3;
            let mut nodes = Vec::new();
            for index in 0..length {
                let chosen = &preserving[payload[2 + index] as usize % preserving.len()];
                let input = if index == 0 {
                    "main".to_owned()
                } else {
                    format!("n{}", index - 1)
                };
                nodes.push(json!({
                    "id": format!("n{index}"),
                    "op": chosen["op"],
                    "in": [input],
                    "config": chosen["config"],
                }));
            }
            if payload[3] % 2 == 0 {
                nodes.push(json!({
                    "id": format!("n{length}"),
                    "op": "table.aggregate",
                    "in": [format!("n{}", length - 1)],
                    "config": {"group_by": ["group"], "aggregations": [{"column": "num", "function": "sum", "alias": "total"}]},
                }));
            }
            let output = nodes.len() - 1;
            let plan = json!({
                "schema_version": 4,
                "inputs": ["main"],
                "nodes": nodes,
                "output": format!("n{output}"),
            });
            let inputs = Inputs::new()
                .with(
                    "main",
                    Input::from_batches(vec![table_batch(&payload[4..], 0)]).expect("input"),
                )
                .expect("inputs");
            run(&plan, inputs, &[("main".to_owned(), table_contract())]);
        }
        1 => {
            // Un nodo binario (join / semi / anti) su due input con lo stesso
            // contratto, opzionalmente seguito da un filtro sulla chiave.
            let how = ["inner", "left", "outer"][payload[2] as usize % 3];
            let (op, config) = match payload[1] % 3 {
                0 => (
                    "table.join",
                    json!({"left_keys": ["id"], "right_keys": ["id"], "how": how}),
                ),
                1 => (
                    "table.semi_join",
                    json!({"left_keys": ["id"], "right_keys": ["id"]}),
                ),
                _ => (
                    "table.anti_join",
                    json!({"left_keys": ["id"], "right_keys": ["id"]}),
                ),
            };
            let mut nodes = vec![json!({
                "id": "b",
                "op": op,
                "in": ["main", "right"],
                "config": config,
            })];
            let mut output = "b";
            if payload[3] % 2 == 0 && op == "table.join" {
                nodes.push(json!({
                    "id": "f",
                    "op": "table.filter",
                    "in": ["b"],
                    "config": {"column": "id", "operator": ">=", "value": 0},
                }));
                output = "f";
            }
            let plan = json!({
                "schema_version": 4,
                "inputs": ["main", "right"],
                "nodes": nodes,
                "output": output,
            });
            let inputs = Inputs::new()
                .with(
                    "main",
                    Input::from_batches(vec![table_batch(&payload[4..], 0)]).expect("input"),
                )
                .expect("inputs")
                .with(
                    "right",
                    Input::from_batches(vec![table_batch(&payload[4..], 37)]).expect("input"),
                )
                .expect("inputs");
            run(
                &plan,
                inputs,
                &[
                    ("main".to_owned(), table_contract()),
                    ("right".to_owned(), table_contract()),
                ],
            );
        }
        _ => {
            // Catena geo: buffer (distanza dal payload) -> area.
            let distance = f64::from(payload[1] % 64) * 0.5;
            let plan = json!({
                "schema_version": 4,
                "inputs": ["geomain"],
                "nodes": [
                    {"id": "b", "op": "geo.buffer", "in": ["geomain"], "config": {"distance": distance}},
                    {"id": "a", "op": "geo.area", "in": ["b"], "config": {}},
                ],
                "output": "a",
            });
            let inputs = Inputs::new()
                .with(
                    "geomain",
                    Input::from_batches(vec![geo_batch(&payload[2..])]).expect("input"),
                )
                .expect("inputs");
            run(&plan, inputs, &[("geomain".to_owned(), geo_contract())]);
        }
    }
});
