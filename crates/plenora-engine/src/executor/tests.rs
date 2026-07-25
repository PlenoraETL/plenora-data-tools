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

use geo::{polygon, Geometry, Point};
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

// ---------------------------------------------------------------------------
// Estensioni geo v1.1-v1.3 e table v1.1-v1.3 (dispatch executor v4)
// ---------------------------------------------------------------------------

fn line_wkb(coords: &[(f64, f64)]) -> Vec<u8> {
    Geometry::LineString(geo::LineString::from(coords.to_vec()))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb linea")
}

fn rect_wkb(xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> Vec<u8> {
    Geometry::Polygon(geo::polygon![
        (x: xmin, y: ymin), (x: xmax, y: ymin),
        (x: xmax, y: ymax), (x: xmin, y: ymax),
        (x: xmin, y: ymin),
    ])
    .to_wkb(CoordDimensions::xy())
    .expect("wkb rettangolo")
}

fn rect_with_hole_wkb() -> Vec<u8> {
    Geometry::Polygon(geo::polygon!(
        exterior: [(x: 0.0, y: 0.0), (x: 8.0, y: 0.0), (x: 8.0, y: 8.0), (x: 0.0, y: 8.0), (x: 0.0, y: 0.0)],
        interiors: [[(x: 2.0, y: 2.0), (x: 4.0, y: 2.0), (x: 4.0, y: 4.0), (x: 2.0, y: 4.0), (x: 2.0, y: 2.0)]],
    ))
    .to_wkb(CoordDimensions::xy())
    .expect("wkb con buco")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// La linea di riferimento della catena snap → subdivide: 7 vertici.
fn reference_line() -> Vec<(f64, f64)> {
    vec![
        (0.0, 0.0),
        (4.0, 0.0),
        (4.0, 4.0),
        (8.0, 4.0),
        (8.0, 8.0),
        (12.0, 8.0),
        (12.0, 12.0),
    ]
}

fn string_column(batch: &RecordBatch, name: &str) -> StringArray {
    let index = batch.schema().column_with_name(name).expect("colonna").0;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("colonna Utf8")
        .clone()
}

fn f64_column(batch: &RecordBatch, name: &str) -> plenora_core::arrow::array::Float64Array {
    let index = batch.schema().column_with_name(name).expect("colonna").0;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::Float64Array>()
        .expect("colonna Float64")
        .clone()
}

fn u64_column(batch: &RecordBatch, name: &str) -> UInt64Array {
    let index = batch.schema().column_with_name(name).expect("colonna").0;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("colonna UInt64")
        .clone()
}

#[test]
fn geo_snap_subdivide_length_chain_expands_rows() {
    let mut perturbed = reference_line();
    perturbed[1] = (4.1, 0.0); // Vertice da agganciare a (4,0) con tolleranza 0.5.
    let reference = line_wkb(&reference_line());
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "s", "op": "geo.snap", "in": ["main"],
             "config": {"reference_wkb": hex(&reference), "tolerance": 0.5}},
            {"id": "d", "op": "geo.subdivide", "in": ["s"],
             "config": {"max_vertices": 4}},
            {"id": "l", "op": "geo.length", "in": ["d"], "config": {}},
        ],
        "output": "l",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(&[0, 1], &[Some(line_wkb(&perturbed)), None])],
    );
    let (batches, metrics) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("stream ok");

    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    // La linea (7 vertici) si spezza in due parti da 4 vertici con vertice
    // condiviso; la riga null resta una riga null.
    assert_eq!(batch.num_rows(), 3);
    let parents = u64_column(batch, "__parent_index");
    assert_eq!(parents.values(), &[0, 0, 1]);
    let lengths = f64_column(batch, "length");
    assert!((lengths.value(0) - 12.0).abs() < 1e-9, "{}", lengths.value(0));
    assert!((lengths.value(1) - 12.0).abs() < 1e-9, "{}", lengths.value(1));
    assert!(lengths.is_null(2), "null in -> null out");
    // Il vertice perturbato e' stato agganciato alla referenza.
    let geom_index = batch.schema().column_with_name("geom").expect("geom").0;
    let cells = batch
        .column(geom_index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("WKB");
    let first = plenora_kernels_geo::geometry_from_wkb(cells.value(0)).expect("decode");
    let Geometry::LineString(line) = first else {
        panic!("attesa LineString");
    };
    assert_eq!((line.0[1].x, line.0[1].y), (4.0, 0.0), "vertice snappato");

    // Metriche per nodo (E3): espansione 2 -> 3 righe su subdivide.
    assert_eq!(metrics.nodes["s"].rows_out, 2);
    assert_eq!(metrics.nodes["d"].rows_in, 2);
    assert_eq!(metrics.nodes["d"].rows_out, 3);
}

#[test]
fn geo_geometry_accessors_adds_canonical_and_prefixed_columns() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "a", "op": "geo.geometry_accessors", "in": ["main"], "config": {}},
            {"id": "b", "op": "geo.geometry_accessors", "in": ["a"],
             "config": {"fields": ["is_closed", "geometry_type"], "output_prefix": "g_"}},
        ],
        "output": "b",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(
            &[0, 1, 2],
            &[
                Some(rect_with_hole_wkb()),
                Some(line_wkb(&[(0.0, 0.0), (3.0, 4.0)])),
                None,
            ],
        )],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 3);
    let types = string_column(batch, "geometry_type");
    assert_eq!(types.value(0), "Polygon");
    assert_eq!(types.value(1), "LineString");
    assert!(types.is_null(2), "geometria null -> accessori null");
    let rings = u64_column(batch, "num_interior_rings");
    assert_eq!(rings.value(0), 1);
    assert_eq!(rings.value(1), 0);
    assert!(rings.is_null(2));
    // Selezione con prefisso, ordine canonico indipendente dalla config.
    let prefixed_types = string_column(batch, "g_geometry_type");
    assert_eq!(prefixed_types.value(1), "LineString");
    let closed_index = batch
        .schema()
        .column_with_name("g_is_closed")
        .expect("g_is_closed")
        .0;
    let closed = batch
        .column(closed_index)
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
        .expect("Boolean");
    assert!(closed.value(0), "poligono chiuso");
    assert!(!closed.value(1), "linea aperta");
    let starts = string_column(batch, "start_point");
    assert!(starts.is_null(0));
    assert!(starts.value(1).starts_with("POINT("), "{}", starts.value(1));
}

#[test]
fn geo_line_locate_point_adds_fraction() {
    let point = point_wkb(5.0, 3.0);
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "p", "op": "geo.line_locate_point", "in": ["main"],
             "config": {"point_wkb": hex(&point)}},
        ],
        "output": "p",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(
            &[0, 1, 2],
            &[
                Some(line_wkb(&[(0.0, 0.0), (10.0, 0.0)])),
                Some(rect_wkb(0.0, 0.0, 4.0, 4.0)),
                None,
            ],
        )],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let fractions = f64_column(&batches[0], "fraction");
    assert!((fractions.value(0) - 0.5).abs() < 1e-12, "{}", fractions.value(0));
    assert!(fractions.is_null(1), "non-linea -> null");
    assert!(fractions.is_null(2), "null -> null");
}

#[test]
fn geo_collect_groups_geometries_by_key() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "c", "op": "geo.collect", "in": ["main"],
             "config": {"group_by": ["id"]}},
        ],
        "output": "c",
    });
    let inputs = single_input(
        "main",
        vec![
            geo_batch(&[2, 1], &[Some(point_wkb(9.0, 9.0)), Some(point_wkb(0.0, 0.0))]),
            geo_batch(&[1, 2], &[Some(point_wkb(1.0, 1.0)), None]),
        ],
    );
    let (batches, metrics) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("stream ok");

    assert_eq!(batches.len(), 1, "segmento blocking: un solo batch");
    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 2, "due gruppi (id 1 e 2)");
    // Ordine canonico per chiave: gruppo id=1 prima di id=2.
    let schema = batch.schema();
    assert_eq!(schema.field(0).name(), "geom", "geometria prima colonna");
    assert_eq!(schema.field(1).name(), "id");
    let cells = batch
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("WKB");
    let first = plenora_kernels_geo::geometry_from_wkb(cells.value(0)).expect("decode");
    let Geometry::MultiPoint(points) = first else {
        panic!("gruppo omogeneo di punti -> MultiPoint: {first:?}");
    };
    assert_eq!(points.0.len(), 2, "due punti nel gruppo id=1 (null saltato)");
    let second = plenora_kernels_geo::geometry_from_wkb(cells.value(1)).expect("decode");
    assert_eq!(second, Geometry::Point(Point::new(9.0, 9.0)));
    assert_eq!(metrics.nodes["c"].rows_in, 4);
    assert_eq!(metrics.nodes["c"].rows_out, 2);
}

#[test]
fn geo_coverage_validate_reports_known_overlap() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "v", "op": "geo.coverage_validate", "in": ["main"], "config": {}},
        ],
        "output": "v",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(
            &[0, 1, 2],
            &[
                Some(rect_wkb(0.0, 0.0, 4.0, 4.0)),
                Some(rect_wkb(2.0, 2.0, 6.0, 6.0)),
                Some(rect_wkb(100.0, 100.0, 104.0, 104.0)),
            ],
        )],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 1, "un solo overlap (0,1)");
    let types = string_column(batch, "issue_type");
    assert_eq!(types.value(0), "overlap");
    assert_eq!(u64_column(batch, "index_a").value(0), 0);
    assert_eq!(u64_column(batch, "index_b").value(0), 1);
    let areas = f64_column(batch, "area");
    assert!((areas.value(0) - 4.0).abs() < 1e-9, "{}", areas.value(0));
}

#[test]
fn geo_shared_paths_finds_shared_border() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "s", "op": "geo.shared_paths", "in": ["main"], "config": {}},
        ],
        "output": "s",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(
            &[0, 1, 2],
            &[
                Some(rect_wkb(0.0, 0.0, 4.0, 4.0)),
                Some(rect_wkb(4.0, 0.0, 8.0, 4.0)),
                Some(rect_wkb(100.0, 100.0, 104.0, 104.0)),
            ],
        )],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 1, "un solo confine condiviso (0,1)");
    assert_eq!(u64_column(batch, "index_a").value(0), 0);
    assert_eq!(u64_column(batch, "index_b").value(0), 1);
    let lengths = f64_column(batch, "shared_length");
    assert!((lengths.value(0) - 4.0).abs() < 1e-9, "{}", lengths.value(0));
}

#[test]
fn geo_cluster_dbscan_labels_known_points() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "k", "op": "geo.cluster_dbscan", "in": ["main"],
             "config": {"eps": 1.0, "min_points": 2}},
        ],
        "output": "k",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(
            &[0, 1, 2, 3, 4, 5, 6],
            &[
                Some(point_wkb(0.0, 0.0)),
                Some(point_wkb(0.1, 0.0)),
                Some(point_wkb(0.0, 0.1)),
                Some(point_wkb(100.0, 100.0)),
                Some(point_wkb(100.1, 100.0)),
                Some(point_wkb(500.0, 500.0)),
                None,
            ],
        )],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 7, "output allineato alle righe");
    let labels = u64_column(batch, "cluster_id");
    let first_cluster = labels.value(0);
    assert_eq!(labels.value(1), first_cluster);
    assert_eq!(labels.value(2), first_cluster);
    let second_cluster = labels.value(3);
    assert_ne!(second_cluster, first_cluster);
    assert_eq!(labels.value(4), second_cluster);
    assert!(labels.is_null(5), "outlier -> noise -> null");
    assert!(labels.is_null(6), "geometria null -> null");
}

#[test]
fn table_select_limit_fingerprint_chain_executes() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "s", "op": "table.select_columns", "in": ["main"],
             "config": {"columns": ["id", "name"]}},
            {"id": "l", "op": "table.limit", "in": ["s"], "config": {"n": 2}},
            {"id": "f", "op": "table.stable_fingerprint", "in": ["l"], "config": {}},
        ],
        "output": "f",
    });
    let inputs = single_input(
        "main",
        vec![table_batch(&[1, 2, 3], &["a", "b", "c"])],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 2, "limit a 2 righe");
    let fingerprints = string_column(batch, "fingerprint");
    assert_eq!(fingerprints.value(0).len(), 64, "sha-256 esadecimale");
    assert_ne!(fingerprints.value(0), fingerprints.value(1));
}

#[test]
fn table_top_n_selects_highest_rows() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "table.top_n", "in": ["main"],
             "config": {"columns": ["id"], "n": 2, "descending": true}},
        ],
        "output": "t",
    });
    let inputs = single_input(
        "main",
        vec![
            table_batch(&[1, 5], &["a", "b"]),
            table_batch(&[3], &["c"]),
        ],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 2);
    let index = batch.schema().column_with_name("id").expect("id").0;
    let ids = batch
        .column(index)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("Int64");
    assert_eq!(ids.values(), &[5, 3], "blocking: top_n sull'intero input");
}

#[test]
fn table_align_schema_then_concat_by_name_executes() {
    let right_schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let right_contract = DataContract::tabular(right_schema.clone());
    let right_batch = RecordBatch::try_new(
        right_schema,
        vec![Arc::new(Int64Array::from(vec![3, 4])) as ArrayRef],
    )
    .expect("batch destro");
    let plan = json!({
        "schema_version": 4,
        "inputs": ["left_in", "right_in"],
        "nodes": [
            {"id": "a", "op": "table.align_schema", "in": ["right_in"],
             "config": {"columns": [
                {"name": "id", "type": "Int64"},
                {"name": "name", "type": "Utf8", "default": "anon"},
             ]}},
            {"id": "c", "op": "table.concat_by_name", "in": ["left_in", "a"],
             "config": {}},
        ],
        "output": "c",
    });
    let inputs = Inputs::new()
        .with(
            "left_in",
            Input::from_batches(vec![table_batch(&[1, 2], &["a", "b"])]).expect("input"),
        )
        .and_then(|inputs| {
            inputs.with("right_in", Input::from_batches(vec![right_batch]).expect("input"))
        })
        .expect("inputs");
    let (batches, _) = output_rows(
        run(
            &plan,
            inputs,
            &[
                ("left_in".to_owned(), table_contract()),
                ("right_in".to_owned(), right_contract),
            ],
        )
        .expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 4);
    let names = string_column(batch, "name");
    assert_eq!(names.value(2), "anon", "default della colonna allineata");
    assert_eq!(names.value(3), "anon");
}

#[test]
fn table_validate_rules_annotates_rows() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "v", "op": "table.validate_rules", "in": ["main"],
             "config": {"rules": [
                {"name": "positive", "operator": "gt", "column": "id", "value": 0},
             ]}},
        ],
        "output": "v",
    });
    let inputs = single_input(
        "main",
        vec![table_batch(&[1, 2], &["a", "b"])],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    let valid_index = batch.schema().column_with_name("_valid").expect("_valid").0;
    let valid = batch
        .column(valid_index)
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
        .expect("Boolean");
    assert!(valid.value(0) && valid.value(1), "tutte le righe valide");
}

#[test]
fn table_hmac_sha256_uses_key_from_env() {
    // La chiave resta fuori dal piano: nel piano c'e' solo il nome della
    // variabile d'ambiente.
    std::env::set_var("PLENORA_TEST_HMAC_KEY", "chiave-di-test");
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "h", "op": "table.hmac_sha256", "in": ["main"],
             "config": {"columns": ["name"], "key_env": "PLENORA_TEST_HMAC_KEY"}},
        ],
        "output": "h",
    });
    let inputs = single_input(
        "main",
        vec![table_batch(&[1, 2], &["a", "b"])],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let hmacs = string_column(&batches[0], "hmac");
    assert_eq!(hmacs.value(0).len(), 64, "hmac sha-256 esadecimale");
    assert_ne!(hmacs.value(0), hmacs.value(1));
}

#[test]
fn table_fuzzy_join_matches_similar_names() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["left_in", "right_in"],
        "nodes": [
            {"id": "j", "op": "table.fuzzy_join", "in": ["left_in", "right_in"],
             "config": {
                "left_key": "name", "right_key": "name",
                "metric": "jaro_winkler", "threshold": 0.8,
                "blocking": "prefix",
             }},
        ],
        "output": "j",
    });
    let inputs = Inputs::new()
        .with(
            "left_in",
            Input::from_batches(vec![table_batch(&[1, 2], &["milano", "roma"])]).expect("input"),
        )
        .and_then(|inputs| {
            inputs.with(
                "right_in",
                Input::from_batches(vec![table_batch(&[1, 2], &["milano", "torino"])]).expect("input"),
            )
        })
        .expect("inputs");
    let (batches, _) = output_rows(
        run(
            &plan,
            inputs,
            &[
                ("left_in".to_owned(), table_contract()),
                ("right_in".to_owned(), table_contract()),
            ],
        )
        .expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 1, "solo milano~milano supera la soglia");
    let scores = f64_column(batch, "score");
    assert!((scores.value(0) - 1.0).abs() < 1e-12, "{}", scores.value(0));
}

// --- Estensioni generative: richiedono la risoluzione CRS (proj-backend) ----

#[cfg(feature = "proj-backend")]
#[test]
fn geo_from_wkt_snap_subdivide_length_chain_executes() {
    let reference = line_wkb(&reference_line());
    let plan = json!({
        "schema_version": 4,
        "crs": "EPSG:32632",
        "inputs": ["main"],
        "nodes": [
            {"id": "w", "op": "geo.from_wkt", "in": ["main"],
             "config": {"wkt_column": "name", "on_error": "null"}},
            {"id": "s", "op": "geo.snap", "in": ["w"],
             "config": {"reference_wkb": hex(&reference), "tolerance": 0.5}},
            {"id": "d", "op": "geo.subdivide", "in": ["s"],
             "config": {"max_vertices": 4}},
            {"id": "l", "op": "geo.length", "in": ["d"], "config": {}},
        ],
        "output": "l",
    });
    let wkt = "LINESTRING(0 0, 4.1 0, 4 4, 8 4, 8 8, 12 8, 12 12)";
    let inputs = single_input(
        "main",
        vec![table_batch(&[0, 1, 2], &[wkt, "NON E WKT", "POINT(1 1)"])],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    // Riga 0: 2 parti; riga 1 (WKT invalido): null; riga 2 (punto): 1 parte.
    assert_eq!(batch.num_rows(), 4);
    let parents = u64_column(batch, "__parent_index");
    assert_eq!(parents.values(), &[0, 0, 1, 2]);
    let lengths = f64_column(batch, "length");
    assert!((lengths.value(0) - 12.0).abs() < 1e-9);
    assert!((lengths.value(1) - 12.0).abs() < 1e-9);
    assert!(lengths.is_null(2), "WKT invalido con on_error null -> null");
    assert!((lengths.value(3) - 0.0).abs() < 1e-12, "lunghezza di un punto");
    // La colonna geometria prodotta ha i metadati GeoArrow del contratto.
    let schema = batch.schema();
    let field = schema.field_with_name("geometry").expect("geometry");
    assert_eq!(
        field.metadata().get("ARROW:extension:name").map(String::as_str),
        Some("geoarrow.wkb")
    );
}

#[cfg(feature = "proj-backend")]
#[test]
fn geo_generate_grid_then_collect_executes() {
    let plan = json!({
        "schema_version": 4,
        "crs": "EPSG:32632",
        "inputs": ["main"],
        "nodes": [
            {"id": "g", "op": "geo.generate_grid", "in": ["main"],
             "config": {
                "extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 10.0, "ymax": 10.0},
                "cell_size": 5.0, "include_centroid": true,
             }},
            {"id": "c", "op": "geo.collect", "in": ["g"],
             "config": {"group_by": ["cell_i", "cell_j"]}},
        ],
        "output": "c",
    });
    let inputs = single_input("main", vec![table_batch(&[1], &["trigger"])]);
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");

    let batch = &batches[0];
    // Un gruppo per cella: 4 righe, geometria singola per gruppo (le celle
    // adiacenti di una griglia si toccano su un lato e una MultiPolygon OGC
    // non lo ammette: il kernel e' fail-closed su output non valido).
    assert_eq!(batch.num_rows(), 4, "una riga per cella della griglia 2x2");
    let cells = batch
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("WKB");
    for row in 0..4 {
        let geometry = plenora_kernels_geo::geometry_from_wkb(cells.value(row)).expect("decode");
        assert!(matches!(geometry, Geometry::Polygon(_)), "{geometry:?}");
    }
    let columns = u64_column(batch, "cell_i");
    assert_eq!(columns.values(), &[0, 0, 1, 1]);
    let rows = u64_column(batch, "cell_j");
    assert_eq!(rows.values(), &[0, 1, 0, 1]);
}

#[cfg(not(feature = "proj-backend"))]
#[test]
fn from_wkt_and_generate_grid_fail_closed_on_crs_without_proj_backend() {
    // Le op generative richiedono un CRS risolto (config o piano): senza
    // proj-backend la risoluzione fallisce chiusa gia' in validate.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "w", "op": "geo.from_wkt", "in": ["main"],
             "config": {"wkt_column": "name", "crs": "EPSG:32632"}},
        ],
        "output": "w",
    });
    let result = validate(&plan.to_string(), &[("main".to_owned(), table_contract())]);
    assert!(
        matches!(result, Err(PlenoraError::Crs(_))),
        "atteso errore CRS fail-closed: {result:?}"
    );
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "g", "op": "geo.generate_grid", "in": ["main"],
             "config": {
                "extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 10.0, "ymax": 10.0},
                "cell_size": 5.0, "crs": "EPSG:32632",
             }},
        ],
        "output": "g",
    });
    let result = validate(&plan.to_string(), &[("main".to_owned(), table_contract())]);
    assert!(
        matches!(result, Err(PlenoraError::Crs(_))),
        "atteso errore CRS fail-closed: {result:?}"
    );
}
