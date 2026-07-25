//! Test dell'executor (Fase 2A-4, Architetture.md par. 6.3, ADR 5;
//! Prestazioni.md V3/V4/V8/V9).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::array::{ArrayRef, BinaryArray, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::{
    ContractProperties, DataContract, FieldId, GeometryColumnContract, GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_core::{PlenoraError, Result};

use geo::{Geometry, Point};
use geozero::{CoordDimensions, ToWkb};
use plenora_kernels_geo::arrow_adapter::geometry_output_field;

use super::*;
use crate::planner::validate;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]))
}

fn table_contract() -> DataContract {
    DataContract::tabular(table_schema())
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
    .expect("contratto fixture valido")
}

fn table_batch(ids: &[i64], names: &[&str]) -> RecordBatch {
    RecordBatch::try_new(
        table_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(StringArray::from(
                names.iter().map(|n| Some(*n)).collect::<Vec<_>>(),
            )) as ArrayRef,
        ],
    )
    .expect("batch fixture valido")
}

fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    Geometry::Point(Point::new(x, y))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb fixture")
}

fn geo_batch(ids: &[i64], cells: &[Option<Vec<u8>>]) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|c| c.as_deref()).collect();
    RecordBatch::try_new(
        geo_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geo fixture valido")
}

fn run(plan: &serde_json::Value, inputs: Inputs, contracts: &[(String, DataContract)]) -> Result<Output> {
    let graph = validate(&plan.to_string(), contracts)?;
    execute(&graph, inputs, RuntimeContext::default())
}

fn single_input(name: &str, batches: Vec<RecordBatch>) -> Inputs {
    Inputs::new()
        .with(name, Input::from_batches(batches).expect("input non vuoto"))
        .expect("input unico")
}

fn output_rows(output: Output) -> Result<(Vec<RecordBatch>, ExecutionMetrics)> {
    output.collect_batches()
}

// ---------------------------------------------------------------------------
// Catene e segmenti
// ---------------------------------------------------------------------------

#[test]
fn linear_table_chain_executes_with_coherent_per_node_metrics() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "r", "op": "table.rename", "in": ["f"],
             "config": {"renames": [{"old_name": "id", "new_name": "key"}]}},
            {"id": "g", "op": "table.aggregate", "in": ["r"],
             "config": {"group_by": ["key"], "aggregations": []}},
        ],
        "output": "g",
    });
    let inputs = single_input(
        "main",
        vec![
            table_batch(&[0, 1], &["a", "b"]),
            table_batch(&[2, 3], &["c", "d"]),
        ],
    );
    let (batches, metrics) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(total_rows, 3, "filter id>0 lascia 3 gruppi");
    // La rinomina e' visibile a valle: group_by sulla colonna rinominata.
    assert!(batches[0].schema().field_with_name("key").is_ok());
    assert!(batches[0].schema().field_with_name("count").is_ok());

    // Metriche per nodo logico (E3), anche dentro al segmento fuso.
    assert_eq!(metrics.nodes.len(), 3);
    let filter = &metrics.nodes["f"];
    assert_eq!(filter.operation, "table.filter");
    assert_eq!(filter.rows_in, 4);
    assert_eq!(filter.rows_out, 3);
    assert_eq!(filter.batches_in, 2);
    let rename = &metrics.nodes["r"];
    assert_eq!(rename.rows_in, 3);
    assert_eq!(rename.rows_out, 3);
    let aggregate = &metrics.nodes["g"];
    assert_eq!(aggregate.rows_in, 3);
    assert_eq!(aggregate.rows_out, 3);
    assert_eq!(metrics.output_rows, 3);
    assert_eq!(metrics.segments.len(), 2);
}

#[test]
fn mixed_table_geo_chain_executes_buffer_then_area() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 10.0}},
            {"id": "a", "op": "geo.area", "in": ["b"], "config": {}},
        ],
        "output": "a",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(
            &[0, 1, 2],
            &[
                Some(point_wkb(0.0, 0.0)),
                Some(point_wkb(100.0, 100.0)),
                None,
            ],
        )],
    );
    let (batches, metrics) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("stream ok");

    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 2, "filter id>0 lascia due righe");
    // La geometria e' preservata (buffer in place) e `area` e' aggiunta.
    assert!(batch.schema().field_with_name("geom").is_ok());
    let area_index = batch
        .schema()
        .column_with_name("area")
        .expect("colonna area aggiunta dalla misura")
        .0;
    let areas = batch
        .column(area_index)
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::Float64Array>()
        .expect("area Float64");
    // Buffer di un punto con raggio 10 ~ cerchio di area pi*100 (~306 con
    // l'approssimazione poligonale di `geo`).
    let expected = 100.0 * std::f64::consts::PI;
    assert!(
        (areas.value(0) - expected).abs() < 15.0,
        "area del buffer: {} vs ~{expected}",
        areas.value(0)
    );
    assert!(areas.is_null(1), "null in -> null out");

    assert_eq!(metrics.nodes["b"].rows_out, 2);
    assert_eq!(metrics.nodes["a"].operation, "geo.area");
}

#[test]
fn fan_out_fan_in_join_executes() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "base", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">=", "value": 0}},
            {"id": "l", "op": "table.filter", "in": ["base"],
             "config": {"column": "id", "operator": "<", "value": 3}},
            {"id": "r", "op": "table.filter", "in": ["base"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "j", "op": "table.join", "in": ["l", "r"],
             "config": {"left_keys": ["id"], "right_keys": ["id"]}},
        ],
        "output": "j",
    });
    let inputs = single_input(
        "main",
        vec![
            table_batch(&[0, 1], &["a", "b"]),
            table_batch(&[2, 3], &["c", "d"]),
        ],
    );
    let (batches, metrics) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(total_rows, 2, "join interno su id: [0,1,2] x [1,2,3] -> 2 righe");
    assert_eq!(metrics.nodes["j"].rows_out, 2);
    assert_eq!(metrics.nodes["l"].rows_in, 4);
    assert_eq!(metrics.nodes["r"].rows_in, 4);
}

// ---------------------------------------------------------------------------
// Streaming reale (V3): lazy, batch per batch
// ---------------------------------------------------------------------------

/// Input lazy che conta i batch consumati.
struct CountingInput {
    schema: SchemaRef,
    total: usize,
    emitted: usize,
    pulled: Rc<Cell<usize>>,
}

impl Iterator for CountingInput {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.emitted == self.total {
            return None;
        }
        self.emitted += 1;
        self.pulled.set(self.pulled.get() + 1);
        let offset = i64::try_from(self.emitted).expect("pochi batch") * 100;
        Some(Ok(
            RecordBatch::try_new(
                self.schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![offset, offset + 1])) as ArrayRef,
                    Arc::new(StringArray::from(vec![Some("x"), Some("y")])) as ArrayRef,
                ],
            )
            .expect("batch valido"),
        ))
    }
}

#[test]
fn streaming_segment_flows_batch_by_batch_without_full_materialization() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">=", "value": 0}},
            {"id": "r", "op": "table.rename", "in": ["f"],
             "config": {"renames": [{"old_name": "name", "new_name": "label"}]}},
        ],
        "output": "r",
    });
    let pulled = Rc::new(Cell::new(0_usize));
    let input = Input::from_iter(
        table_schema(),
        CountingInput {
            schema: table_schema(),
            total: 10,
            emitted: 0,
            pulled: Rc::clone(&pulled),
        },
    );
    let inputs = Inputs::new().with("main", input).expect("input");
    let mut output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");

    assert_eq!(pulled.get(), 0, "l'esecuzione e' lazy: nessun batch prima della pull");
    let first = output.next().expect("primo batch").expect("batch ok");
    assert_eq!(first.num_rows(), 2);
    assert_eq!(
        pulled.get(),
        1,
        "una pull sull'output consuma esattamente un batch di input (V3)"
    );
    assert_eq!(output.metrics().nodes["f"].batches_out, 1);

    for _ in 0..3 {
        output.next().expect("batch").expect("batch ok");
    }
    assert_eq!(pulled.get(), 4, "i batch fluisco uno alla volta");
    assert_eq!(output.metrics().output_batches, 4);

    let remaining = output.collect_batches().expect("stream ok").0;
    assert_eq!(remaining.len(), 6);
    assert_eq!(pulled.get(), 10);
}

#[test]
fn blocking_segment_materializes_before_emitting() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">=", "value": 0}},
            {"id": "g", "op": "table.aggregate", "in": ["f"],
             "config": {"group_by": ["id"], "aggregations": []}},
        ],
        "output": "g",
    });
    let pulled = Rc::new(Cell::new(0_usize));
    let input = Input::from_iter(
        table_schema(),
        CountingInput {
            schema: table_schema(),
            total: 5,
            emitted: 0,
            pulled: Rc::clone(&pulled),
        },
    );
    let inputs = Inputs::new().with("main", input).expect("input");
    let mut output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");

    let first = output.next().expect("output aggregate").expect("batch ok");
    assert_eq!(
        pulled.get(),
        5,
        "il segmento blocking drena tutto l'input prima di emettere"
    );
    assert_eq!(first.num_rows(), 10);
    assert!(output.next().is_none());
}

// ---------------------------------------------------------------------------
// Errori e limiti
// ---------------------------------------------------------------------------

#[test]
fn error_mid_stream_publishes_nothing() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">=", "value": 0}},
        ],
        "output": "f",
    });
    let failing = Input::from_iter(
        table_schema(),
        vec![
            Ok(table_batch(&[1], &["a"])),
            Err(PlenoraError::Contract("lettura fallita a meta' stream".into())),
        ]
        .into_iter(),
    );
    let inputs = Inputs::new().with("main", failing).expect("input");
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");

    let directory = tempfile::tempdir().expect("tempdir");
    let destination = directory.path().join("output.arrow");
    let result = output.write_ipc_file(&destination);
    assert!(result.is_err());
    assert!(
        !destination.exists(),
        "nessun output parziale visibile (publish atomico)"
    );
}

#[test]
fn invalid_wkb_cell_fails_at_read_before_any_output() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(&[1], &[Some(b"non-e-wkb".to_vec())])],
    );
    let output = run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute");
    let error = output.collect_batches().expect_err("WKB invalido");
    assert!(
        error.to_string().contains("WKB"),
        "validazione dinamica in lettura (D8): {error}"
    );
}

#[test]
fn pass_through_plan_streams_input_to_output() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input(
        "main",
        vec![
            table_batch(&[1, 2], &["a", "b"]),
            table_batch(&[3], &["c"]),
        ],
    );
    let (batches, metrics) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");
    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(total_rows, 3);
    assert_eq!(metrics.output_rows, 3);
    assert!(metrics.nodes.is_empty());
}

#[test]
fn row_limits_trigger_on_input_edge_and_output() {
    // max_input_rows
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_input_rows": 2},
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input("main", vec![table_batch(&[1, 2, 3], &["a", "b", "c"])]);
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");
    let error = output.collect_batches().expect_err("limite input");
    assert!(error.to_string().contains("max_input_rows"), "{error}");

    // max_rows_per_edge su un arco intermedio
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_rows_per_edge": 2},
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">=", "value": 0}},
            {"id": "r", "op": "table.rename", "in": ["f"],
             "config": {"renames": [{"old_name": "name", "new_name": "label"}]}},
        ],
        "output": "r",
    });
    let inputs = single_input("main", vec![table_batch(&[1, 2, 3], &["a", "b", "c"])]);
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");
    let error = output.collect_batches().expect_err("limite arco");
    assert!(error.to_string().contains("max_rows_per_edge"), "{error}");

    // max_output_rows sull'arco finale
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_output_rows": 2},
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input("main", vec![table_batch(&[1, 2, 3], &["a", "b", "c"])]);
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");
    let error = output.collect_batches().expect_err("limite output");
    assert!(error.to_string().contains("max_output_rows"), "{error}");
}

#[test]
fn expansion_factor_triggers_on_join() {
    // 3 righe left x 2 righe right con la stessa chiave: 6 righe in uscita,
    // base ADR 6 = left + right = 5 -> 6 > 5 x 1.0 scatta il limite.
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_expansion_factor": 1.0},
        "inputs": ["left_in", "right_in"],
        "nodes": [
            {"id": "j", "op": "table.join", "in": ["left_in", "right_in"],
             "config": {"left_keys": ["id"], "right_keys": ["id"]}},
        ],
        "output": "j",
    });
    let inputs = Inputs::new()
        .with("left_in", Input::from_batches(vec![table_batch(&[1, 1, 1], &["a", "b", "c"])]).expect("input"))
        .and_then(|inputs| {
            inputs.with(
                "right_in",
                Input::from_batches(vec![table_batch(&[1, 1], &["d", "e"])]).expect("input"),
            )
        })
        .expect("inputs");
    let output = run(
        &plan,
        inputs,
        &[
            ("left_in".to_owned(), table_contract()),
            ("right_in".to_owned(), table_contract()),
        ],
    )
    .expect("execute");
    let error = output.collect_batches().expect_err("espansione oltre il fattore 1.0");
    assert!(
        error.to_string().contains("max_expansion_factor"),
        "{error}"
    );
}

#[test]
fn input_schema_mismatch_is_rejected_before_execution() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let wrong_schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false)]));
    let batch = RecordBatch::try_new(
        wrong_schema,
        vec![Arc::new(StringArray::from(vec![Some("a")])) as ArrayRef],
    )
    .expect("batch");
    let inputs = Inputs::new()
        .with("main", Input::from_batches(vec![batch]).expect("input"))
        .expect("inputs");
    let result = run(&plan, inputs, &[("main".to_owned(), table_contract())]);
    assert!(matches!(result, Err(PlenoraError::Schema(_))));
}

// ---------------------------------------------------------------------------
// Scrittura IPC con publish atomico
// ---------------------------------------------------------------------------

#[test]
fn ipc_roundtrip_through_publish_atomic() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input(
        "main",
        vec![
            table_batch(&[1, 2], &["a", "b"]),
            table_batch(&[3], &["c"]),
        ],
    );
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");

    let directory = tempfile::tempdir().expect("tempdir");
    let destination = directory.path().join("output.arrow");
    let metrics = output.write_ipc_file(&destination).expect("publish");
    assert!(destination.exists());
    assert_eq!(metrics.output_rows, 3);

    // Rilettura: lo stesso piano pass-through sul file pubblicato.
    let input = Input::read_ipc_file(&destination).expect("lettore IPC");
    let inputs = Inputs::new().with("main", input).expect("input");
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");
    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(total_rows, 3);
}
