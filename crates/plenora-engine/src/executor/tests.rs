//! Test dell'executor (Fase 2A-4, Architetture.md par. 6.3, ADR 5;
//! Prestazioni.md V3/V4/V8/V9).

use std::cell::Cell;
use std::fs::File;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::array::{ArrayRef, BinaryArray, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::{
    ContractProperties, DataContract, FieldId, GeometryColumnContract, GeometryDimensions,
    GeometryEncoding,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_core::{PlenoraError, Result};

use geo::{polygon, Geometry, Point};
use geozero::{CoordDimensions, ToWkb};
use plenora_kernels_geo::arrow_adapter::{
    geometry_dimensions_from_metadata, geometry_output_field, geometry_output_field_with_dimensions,
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY, PLENORA_CONTRACT_VERSION_KEY,
    PLENORA_FIELD_ID_KEY, PLENORA_GEOMETRY_AXIS_ORDER_KEY, PLENORA_GEOMETRY_CRS_ID_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_DIMENSIONS_KEY,
    PLENORA_GEOMETRY_ENCODING_KEY, PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
};

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
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
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
            Err(PlenoraError::InvalidPlan("lettura fallita a meta' stream".into())),
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

// ---------------------------------------------------------------------------
// Dimensionalita' (B1.3): gate stride-aware e passthrough tabellare
// ---------------------------------------------------------------------------

/// Schema geo con dimensionalita' XYZ dichiarata nei metadati `geo`.
fn geo_schema_xyz() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        geometry_output_field_with_dimensions("geom", "EPSG:32632", GeometryDimensions::Xyz)
            .expect("campo geometria xyz"),
    ]))
}

/// Come `geo_contract`, con contratto e metadati XYZ.
fn geo_contract_xyz() -> DataContract {
    let mut contract = geo_contract();
    contract.schema = geo_schema_xyz();
    contract.geometries[0].dimensions = GeometryDimensions::Xyz;
    contract
}

/// WKB ISO little-endian di un Point Z (type code 1001).
fn xyz_point_wkb(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut payload = vec![1_u8];
    payload.extend_from_slice(&1001_u32.to_le_bytes());
    for value in [x, y, z] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload
}

fn geo_batch_xyz(ids: &[i64], cells: &[Option<Vec<u8>>]) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|c| c.as_deref()).collect();
    RecordBatch::try_new(
        geo_schema_xyz(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geo xyz fixture valido")
}

#[test]
fn wkb_type_code_incoherent_with_contract_dimensions_fails_at_the_gate() {
    // (c) B1.3: il gate in lettura valida con la dimensionalita' del
    // contratto dell'arco — una cella XY su un contratto XYZ e' l'errore
    // dedicato di mismatch, prima di qualunque output.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch_xyz(&[1], &[Some(point_wkb(1.0, 2.0))])],
    );
    let output =
        run(&plan, inputs, &[("main".to_owned(), geo_contract_xyz())]).expect("execute");
    let error = output
        .collect_batches()
        .expect_err("type code incoerente col contratto");
    assert!(
        error.to_string().contains("incoerente"),
        "errore dedicato di mismatch dimensionale: {error}"
    );
}

#[test]
fn xyz_batch_round_trips_byte_per_byte_through_a_table_filter() {
    // (e) B1.3: batch xyz -> filtro tabellare (passthrough) -> celle xyz
    // intatte byte-per-byte; i metadati di output dichiarano ancora xyz.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 1}},
        ],
        "output": "f",
    });
    let kept_a = xyz_point_wkb(1.0, 2.0, 3.0);
    let kept_b = xyz_point_wkb(4.0, 5.0, 6.0);
    let dropped = xyz_point_wkb(7.0, 8.0, 9.0);
    let inputs = single_input(
        "main",
        vec![geo_batch_xyz(
            &[2, 1, 3],
            &[Some(kept_a.clone()), Some(dropped), Some(kept_b.clone())],
        )],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract_xyz())]).expect("execute"),
    )
    .expect("collect");
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 2, "il filtro scarta solo la riga con id 1");
    let cells = batch
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("colonna geometria binaria");
    assert_eq!(cells.value(0), kept_a.as_slice(), "cella 0 byte-per-byte");
    assert_eq!(cells.value(1), kept_b.as_slice(), "cella 1 byte-per-byte");
    // I metadati di output dichiarano ancora xyz: mai un xy silenzioso.
    assert_eq!(
        geometry_dimensions_from_metadata(batch.schema().field(1)),
        GeometryDimensions::Xyz
    );
}

/// Punto EWKB little-endian con flag SRID (`0x2000_0000`) + valore SRID.
fn ewkb_srid_point_wkb(srid: u32, x: f64, y: f64) -> Vec<u8> {
    let mut payload = vec![1_u8];
    payload.extend_from_slice(&0x2000_0001_u32.to_le_bytes());
    payload.extend_from_slice(&srid.to_le_bytes());
    payload.extend_from_slice(&x.to_le_bytes());
    payload.extend_from_slice(&y.to_le_bytes());
    payload
}

/// Come `geo_contract`, con encoding EWKB dichiarato (fixture B1.4).
fn geo_contract_ewkb() -> DataContract {
    let mut contract = geo_contract();
    contract.geometries[0].encoding = Some(GeometryEncoding::Ewkb);
    contract
}

#[test]
fn ewkb_srid_cell_fails_at_the_gate_even_with_declared_ewkb_encoding() {
    // (f) B1.4: il flag SRID EWKB non e' preservabile — rifiutato dal
    // validatore celle al gate di lettura per qualunque dimensionalita'
    // dichiarata, anche con `encoding: ewkb` nel contratto. L'input fallisce
    // prima di qualunque output.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input(
        "main",
        vec![geo_batch(&[1], &[Some(ewkb_srid_point_wkb(4326, 1.0, 2.0))])],
    );
    let output = run(&plan, inputs, &[("main".to_owned(), geo_contract_ewkb())]).expect("execute");
    let error = output
        .collect_batches()
        .expect_err("flag SRID EWKB rifiutato al gate");
    assert!(
        error.to_string().contains("SRID"),
        "errore esplicito sullo SRID non preservabile: {error}"
    );
}

#[test]
fn flags_free_ewkb_is_byte_identical_to_iso_and_passes_the_xy_gate() {
    // (f) B1.4, comportamento dichiarato: un payload EWKB senza flag Z/M e
    // senza SRID ha type code identici a WKB ISO — indistinguibile sul filo.
    // Con `encoding: ewkb` dichiarato e dimensionalita' `xy` il gate lo
    // accetta e i byte passano invariati (la validazione resta sui type
    // code, mai sulla chiave `encoding` del metadato).
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let cell = point_wkb(1.0, 2.0); // ISO XY == EWKB puro-XY (stessi byte)
    let inputs = single_input("main", vec![geo_batch(&[1], &[Some(cell.clone())])]);
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract_ewkb())]).expect("execute"),
    )
    .expect("EWKB puro-XY passa il gate xy");
    let cells = batches[0]
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("colonna geometria binaria");
    assert_eq!(cells.value(0), cell.as_slice(), "cella byte-per-byte");
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
fn expansion_constraint_max_relative_triggers_on_many_to_many_join() {
    // Stesso join del test precedente ma fattore 1.5: con la base left+right
    // (SumRelative, 6/5 = 1.2) NON scatterebbe; table.join dichiara
    // MaxRelative (max(6/3, 6/2) = 3.0 > 1.5) -> scatta (ADR 6).
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_expansion_factor": 1.5},
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
    let error = output
        .collect_batches()
        .expect_err("MaxRelative oltre il fattore 1.5");
    let message = error.to_string();
    assert!(message.contains("max_expansion_factor"), "{message}");
    // Il messaggio riporta vincolo e metriche (ADR 6).
    assert!(message.contains("MaxRelative"), "{message}");
    assert!(message.contains("output/left"), "{message}");
    assert!(message.contains("output/right"), "{message}");
}

#[test]
fn expansion_constraint_left_relative_allows_lookup_style_join() {
    // semi_join (lookup-style, output <= left) dichiara LeftRelative: 2
    // righe in uscita su 3 left -> metrica 0.67, sotto il fattore 1.0.
    // Nota: SumRelative e' sempre piu' debole di LeftRelative (left+right >=
    // left), quindi un caso in cui SumRelative scatta e LeftRelative no non
    // esiste; questo test blocca il requisito che un lookup legittimo non
    // venga rifiutato dal vincolo dichiarato in catalogo.
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_expansion_factor": 1.0},
        "inputs": ["left_in", "right_in"],
        "nodes": [
            {"id": "s", "op": "table.semi_join", "in": ["left_in", "right_in"],
             "config": {"left_keys": ["id"], "right_keys": ["id"]}},
        ],
        "output": "s",
    });
    let inputs = Inputs::new()
        .with(
            "left_in",
            Input::from_batches(vec![table_batch(&[1, 2, 3], &["a", "b", "c"])]).expect("input"),
        )
        .and_then(|inputs| {
            inputs.with(
                "right_in",
                Input::from_batches(vec![table_batch(&[1, 2], &["d", "e"])]).expect("input"),
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
    .expect("lookup join entro LeftRelative");
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 2);
}

#[test]
fn expansion_constraint_sum_relative_on_union_distinct() {
    // union_distinct dichiara SumRelative: output 3 righe su base left+right
    // = 4 -> metrica 0.75, sotto il fattore 1.0. Con LeftRelative (3/2 =
    // 1.5) scatterebbe: il vincolo dichiarato in catalogo e' quello
    // applicato (ADR 6).
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_expansion_factor": 1.0},
        "inputs": ["left_in", "right_in"],
        "nodes": [
            {"id": "u", "op": "table.union_distinct", "in": ["left_in", "right_in"],
             "config": {}},
        ],
        "output": "u",
    });
    let inputs = Inputs::new()
        .with("left_in", Input::from_batches(vec![table_batch(&[1, 2], &["a", "b"])]).expect("input"))
        .and_then(|inputs| {
            inputs.with(
                "right_in",
                Input::from_batches(vec![table_batch(&[2, 3], &["b", "c"])]).expect("input"),
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
    .expect("union entro SumRelative");
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 3);
}

#[test]
fn total_rows_processed_counts_rows_through_all_nodes() {
    // Metrica obbligatoria ADR 6 (non limite v1): somma delle righe in
    // ingresso a ogni nodo. 3 righe attraversano filter e rename ->
    // 3 + 3 = 6.
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
    let inputs = single_input("main", vec![table_batch(&[1, 2, 3], &["a", "b", "c"])]);
    let (batches, metrics) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("stream ok");
    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(total_rows, 3);
    assert_eq!(metrics.total_rows_processed, 6);
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
// Milestone C: blocco canonico R2.2/R2.5 nello schema IPC di output
// ---------------------------------------------------------------------------

#[test]
fn canonical_output_schema_without_geometries_is_unchanged() {
    // R2.5: la versione accompagna le chiavi canoniche, non sta su schemi
    // senza — uno schema tabellare resta invariato.
    let schema = canonical_output_schema(&table_contract()).expect("schema tabellare");
    assert_eq!(schema, table_schema());
    assert!(!schema.metadata().contains_key(PLENORA_CONTRACT_VERSION_KEY));
}

#[test]
fn canonical_output_schema_merges_canonical_keys_idempotently() {
    let contract = geo_contract();
    let merged = canonical_output_schema(&contract).expect("fusione canonica");
    assert_eq!(
        merged.metadata().get(PLENORA_CONTRACT_VERSION_KEY).map(String::as_str),
        Some("1"),
        "R2.5: la versione accompagna le chiavi canoniche"
    );
    let metadata = merged
        .field_with_name("geom")
        .expect("geom")
        .metadata();
    assert_eq!(metadata.get(PLENORA_GEOMETRY_DIMENSIONS_KEY).map(String::as_str), Some("xy"));
    assert_eq!(
        metadata.get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY).map(String::as_str),
        Some("resolved")
    );
    assert_eq!(metadata.get(PLENORA_GEOMETRY_CRS_ID_KEY).map(String::as_str), Some("EPSG:32632"));
    // `GeometryMetadataDetails::default()`: axis_order obbligatorio con CRS
    // -> valore canonico `unknown` (mai inventato), encoding e types non
    // dichiarati dal contratto -> chiavi assenti (R5.2).
    assert_eq!(metadata.get(PLENORA_GEOMETRY_AXIS_ORDER_KEY).map(String::as_str), Some("unknown"));
    assert!(!metadata.contains_key(PLENORA_GEOMETRY_ENCODING_KEY));
    assert!(!metadata.contains_key(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY));
    // Le chiavi GeoArrow legacy RESTANO (R2.6: coesistenza coerente).
    assert_eq!(
        metadata.get(GEOARROW_EXTENSION_KEY).map(String::as_str),
        Some(GEOARROW_WKB_EXTENSION)
    );
    assert!(metadata.contains_key(GEO_METADATA_KEY));

    // Idempotenza: una seconda fusione sullo schema arricchito non fallisce
    // (chiavi uguali = coerenti) e non cambia nulla.
    let mut again = geo_contract();
    again.schema = merged.clone();
    let remerged = canonical_output_schema(&again).expect("idempotente");
    assert_eq!(remerged, merged);
}

#[test]
fn canonical_output_schema_rejects_divergent_preexisting_key() {
    // (e) R2.6: chiave canonica gia' presente con valore diverso da quello
    // imposto dal contratto -> errore, mai sovrascrittura silenziosa.
    let mut contract = geo_contract();
    let fields: Vec<Field> = contract
        .schema
        .fields()
        .iter()
        .map(|field| {
            if field.name() == "geom" {
                let mut metadata = field.metadata().clone();
                metadata.insert(PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned());
                field.as_ref().clone().with_metadata(metadata)
            } else {
                field.as_ref().clone()
            }
        })
        .collect();
    contract.schema = Arc::new(Schema::new(fields));
    assert!(matches!(
        canonical_output_schema(&contract),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn canonical_output_schema_rejects_geometry_missing_from_schema() {
    // Fail-closed: colonna geometrica del contratto assente dallo schema.
    let mut contract = geo_contract();
    contract.schema = table_schema();
    assert!(matches!(
        canonical_output_schema(&contract),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn ipc_output_carries_canonical_geometry_keys_and_contract_version() {
    // (d) output v4 con colonna geometria: l'header IPC scritto porta il
    // blocco canonico R2.2 sui campi geometria e la versione R2.5 nei
    // metadati dello schema.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input("main", vec![geo_batch(&[1], &[Some(point_wkb(1.0, 2.0))])]);
    let output = run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute");

    let directory = tempfile::tempdir().expect("tempdir");
    let destination = directory.path().join("output.arrow");
    output.write_ipc_file(&destination).expect("publish");

    let reader = FileReader::try_new(File::open(&destination).expect("open"), None)
        .expect("lettore IPC");
    let schema = reader.schema();
    assert_eq!(
        schema.metadata().get(PLENORA_CONTRACT_VERSION_KEY).map(String::as_str),
        Some("1")
    );
    let metadata = schema.field_with_name("geom").expect("geom").metadata();
    assert_eq!(metadata.get(PLENORA_GEOMETRY_DIMENSIONS_KEY).map(String::as_str), Some("xy"));
    assert_eq!(
        metadata.get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY).map(String::as_str),
        Some("resolved")
    );
    assert_eq!(metadata.get(PLENORA_GEOMETRY_CRS_ID_KEY).map(String::as_str), Some("EPSG:32632"));
    assert_eq!(metadata.get(PLENORA_GEOMETRY_AXIS_ORDER_KEY).map(String::as_str), Some("unknown"));
    // `field_id` non e' emesso (R2.2 opzionale; il FieldId di grafo non ha
    // significato fuori dal processo, ADR-0009 decisione 3).
    assert!(!metadata.contains_key(PLENORA_FIELD_ID_KEY));
    // Coesistenza R2.6: le chiavi GeoArrow legacy non sono rimosse.
    assert_eq!(
        metadata.get(GEOARROW_EXTENSION_KEY).map(String::as_str),
        Some(GEOARROW_WKB_EXTENSION)
    );
    assert!(metadata.contains_key(GEO_METADATA_KEY));
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
    use std::fmt::Write as _;

    // `write!` su String non fallisce mai: il risultato e' scartato per
    // costruzione, non un errore ignorato.
    bytes.iter().fold(String::new(), |mut output, byte| {
        let _ = write!(output, "{byte:02x}");
        output
    })
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

// ---------------------------------------------------------------------------
// Regressioni review engine: limiti cablati, tee errori, blocking fail-closed
// ---------------------------------------------------------------------------

#[test]
fn whole_to_many_ops_are_exempt_from_expansion_factor() {
    // 5 rettangoli identici -> C(5,2) = 10 overlap; con fattore 1.0 e base
    // input (5 righe) il controllo scatterebbe: le op WholeToMany sono
    // esenti (l'input e' un trigger/insieme da analizzare, non una base
    // proporzionale; restano i limiti max_rows_per_edge/max_output_rows).
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_expansion_factor": 1.0},
        "inputs": ["main"],
        "nodes": [
            {"id": "v", "op": "geo.coverage_validate", "in": ["main"], "config": {}},
        ],
        "output": "v",
    });
    let cells: Vec<Option<Vec<u8>>> = (0..5).map(|_| Some(rect_wkb(0.0, 0.0, 4.0, 4.0))).collect();
    let inputs = single_input("main", vec![geo_batch(&[0, 1, 2, 3, 4], &cells)]);
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("op WholeToMany esente dal fattore di espansione");
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 10, "un overlap per coppia");
}

#[cfg(feature = "proj-backend")]
#[test]
fn generate_grid_expansion_exempt_with_small_trigger() {
    // Trigger da 5 righe + griglia 100x100 celle (lato 1.0): 10_000 righe
    // prodotte con fattore 1.0 — esente perche' WholeToMany (ADR 6).
    let plan = json!({
        "schema_version": 4,
        "crs": "EPSG:32632",
        "limits": {"max_expansion_factor": 1.0},
        "inputs": ["main"],
        "nodes": [
            {"id": "g", "op": "geo.generate_grid", "in": ["main"],
             "config": {
                "extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 100.0, "ymax": 100.0},
                "cell_size": 1.0,
             }},
        ],
        "output": "g",
    });
    let inputs = single_input(
        "main",
        vec![table_batch(&[1, 2, 3, 4, 5], &["a", "b", "c", "d", "e"])],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("op generativa esente dal fattore di espansione");
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 10_000);
}

#[test]
fn payload_bytes_limit_triggers_cumulatively_on_input() {
    let first = table_batch(&[1, 2], &["a", "b"]);
    let second = table_batch(&[3], &["c"]);
    let first_bytes = first.get_array_memory_size() as u64;
    let total_bytes = first_bytes + second.get_array_memory_size() as u64;

    // Il primo batch entra nel budget, il cumulato no: fail-closed.
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_payload_bytes": first_bytes},
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input("main", vec![first, second]);
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");
    let error = output.collect_batches().expect_err("payload oltre il limite");
    assert!(error.to_string().contains("max_payload_bytes"), "{error}");

    // Budget sufficiente per l'intero payload: passa.
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_payload_bytes": total_bytes},
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input(
        "main",
        vec![table_batch(&[1, 2], &["a", "b"]), table_batch(&[3], &["c"])],
    );
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute"),
    )
    .expect("payload entro il limite");
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 3);
}

/// GC annidata `depth` volte attorno a un punto: il punto e' a profondita'
/// `depth` nel WKB.
fn nested_collection_wkb(depth: usize) -> Vec<u8> {
    let mut geometry = Geometry::Point(Point::new(1.0, 2.0));
    for _ in 0..depth {
        geometry = Geometry::GeometryCollection(geo::GeometryCollection(vec![geometry]));
    }
    geometry.to_wkb(CoordDimensions::xy()).expect("wkb annidato")
}

#[test]
fn geometry_depth_limit_applies_to_nested_wkb() {
    // Punto a profondita' 2: con max_geometry_depth 1 fallisce in lettura,
    // con 2 (e col default 64) passa.
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_geometry_depth": 1},
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input("main", vec![geo_batch(&[1], &[Some(nested_collection_wkb(2))])]);
    let output = run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute");
    let error = output.collect_batches().expect_err("annidamento oltre il limite");
    assert!(error.to_string().contains("annidamento"), "{error}");

    let plan = json!({
        "schema_version": 4,
        "limits": {"max_geometry_depth": 2},
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input("main", vec![geo_batch(&[1], &[Some(nested_collection_wkb(2))])]);
    let (batches, _) = output_rows(
        run(&plan, inputs, &[("main".to_owned(), geo_contract())]).expect("execute"),
    )
    .expect("annidamento entro il limite");
    assert_eq!(batches[0].num_rows(), 1);
}

#[test]
fn edge_stream_delivers_upstream_error_once_per_reader() {
    let upstream: BatchStream = Box::new(
        vec![
            Ok(GovernedBatch::new(table_batch(&[1], &["a"]), None, None)),
            Err(PlenoraError::Execution {
                node: "n1".to_owned(),
                operation: "table.filter".to_owned(),
                execution_id: "exec-test".to_owned(),
                reason: "boom".to_owned(),
            }),
        ]
        .into_iter(),
    );
    let shared = EdgeShared::new(upstream);
    let mut first = shared.register_reader();
    let mut second = shared.register_reader();

    // Primo consumatore: batch, errore originale, poi chiusura (mai un
    // iteratore infinito di errori).
    assert!(matches!(first.next(), Some(Ok(_))));
    match first.next() {
        Some(Err(PlenoraError::Execution { node, .. })) => assert_eq!(node, "n1"),
        other => panic!("atteso l'errore Step originale: {other:?}"),
    }
    assert!(first.next().is_none(), "errore consegnato una sola volta");
    assert!(first.next().is_none(), "lo stream resta chiuso");

    // Secondo consumatore: batch bufferizzato, errore UNA volta con
    // l'attribuzione Step{node, operation, execution_id} preservata, poi
    // chiusura.
    assert!(matches!(second.next(), Some(Ok(_))));
    match second.next() {
        Some(Err(PlenoraError::Execution {
            node,
            operation,
            execution_id,
            reason,
        })) => {
            assert_eq!(node, "n1");
            assert_eq!(operation, "table.filter");
            assert_eq!(execution_id, "exec-test");
            assert_eq!(reason, "boom");
        }
        other => panic!("atteso Step preservato, non Contract declassato: {other:?}"),
    }
    assert!(second.next().is_none(), "errore consegnato una sola volta");
}

#[test]
fn blocking_segment_fails_closed_when_concatenated_batch_exceeds_byte_cap() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "g", "op": "table.aggregate", "in": ["main"],
             "config": {"group_by": ["id"], "aggregations": []}},
        ],
        "output": "g",
    });
    let ids: Vec<i64> = (0..1_000).collect();
    let names: Vec<String> = ids.iter().map(|id| format!("name{id}")).collect();
    let batches: Vec<RecordBatch> = (0..4)
        .map(|_| table_batch(&ids, &names.iter().map(String::as_str).collect::<Vec<_>>()))
        .collect();
    // Ogni batch singolo e' sotto il tetto; la concatenazione (4x) no.
    let cap = batches[0].get_array_memory_size() * 2;
    let graph =
        validate(&plan.to_string(), &[("main".to_owned(), table_contract())]).expect("validate");
    let runtime = RuntimeContext {
        batch_target: crate::prepare::BatchTarget {
            target_batch_bytes: cap,
            max_batch_bytes: cap,
        },
        ..RuntimeContext::default()
    };
    let inputs = single_input("main", batches);
    let output = execute(&graph, inputs, runtime).expect("execute");
    let error = output
        .collect_batches()
        .expect_err("tetto byte sul batch concatenato");
    assert!(error.to_string().contains("max_batch_bytes"), "{error}");
}

#[test]
fn blocking_concat_error_is_attributed_to_the_node() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "g", "op": "table.aggregate", "in": ["main"],
             "config": {"group_by": ["id"], "aggregations": []}},
        ],
        "output": "g",
    });
    let graph =
        validate(&plan.to_string(), &[("main".to_owned(), table_contract())]).expect("validate");
    let physical = Rc::new(prepare(&graph, &RuntimeContext::default()).expect("prepare"));
    let temp_root = tempfile::tempdir().expect("tempdir");
    let store = TempStore::with_root("exec-test", temp_root.path()).expect("temp store");
    let state = ExecState::new(
        &physical,
        "exec-test".to_owned(),
        CancellationToken::new(),
        false,
        store,
    );
    let segment_index = physical
        .segments()
        .iter()
        .position(|segment| segment.mode == SegmentMode::Blocking)
        .expect("segmento blocking");
    // Batch con stesso numero di colonne ma tipo diverso dal contratto di
    // input del kernel: concat fallisce e l'errore deve essere attribuito al
    // nodo (Step), non Arrow nudo.
    let wrong = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Int64, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
            Arc::new(Int64Array::from(vec![2])) as ArrayRef,
        ],
    )
    .expect("batch fixture");
    let error = run_blocking(
        &physical,
        segment_index,
        &state,
        vec![GovernedBatch::new(wrong, None, None)],
    )
    .expect_err("concat con schema incoerente");
    match error {
        PlenoraError::Execution {
            node,
            operation,
            reason,
            ..
        } => {
            assert_eq!(node, "g");
            assert_eq!(operation, "table.aggregate");
            assert!(reason.contains("arrow"), "{reason}");
        }
        other => panic!("atteso Step con attribuzione nodo: {other:?}"),
    }
}

#[test]
fn binary_blocking_metrics_count_real_input_batches() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["left", "right"],
        "nodes": [
            {"id": "j", "op": "table.join", "in": ["left", "right"],
             "config": {"left_keys": ["id"], "right_keys": ["id"]}},
        ],
        "output": "j",
    });
    let inputs = Inputs::new()
        .with(
            "left",
            Input::from_batches(vec![
                table_batch(&[1], &["a"]),
                table_batch(&[2], &["b"]),
                table_batch(&[3], &["c"]),
            ])
            .expect("left"),
        )
        .expect("inputs")
        .with(
            "right",
            Input::from_batches(vec![table_batch(&[2], &["bb"])]).expect("right"),
        )
        .expect("inputs");
    let contracts = vec![
        ("left".to_owned(), table_contract()),
        ("right".to_owned(), table_contract()),
    ];
    let (_, metrics) =
        output_rows(run(&plan, inputs, &contracts).expect("execute")).expect("stream ok");
    assert_eq!(
        metrics.nodes["j"].batches_in, 4,
        "3 batch left + 1 batch right drenati davvero"
    );
    assert_eq!(metrics.nodes["j"].batches_out, 1);
    let segment = metrics.segments.values().next().expect("segmento binario");
    assert_eq!(segment.batches_in, 4);
}

// ---------------------------------------------------------------------------
// Identita' ADR 4: l'executor rifiuta i grafi incompatibili
// ---------------------------------------------------------------------------

#[test]
fn execute_rejects_a_graph_incompatible_with_the_environment() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    });
    let contracts = vec![("main".to_owned(), table_contract())];
    let mut graph = validate(&plan.to_string(), &contracts).expect("validate");
    // Grafo la cui identita' non combacia con l'ambiente corrente (es. riuso
    // di un grafo validato da un'altra build): l'executor rifiuta (ADR 4).
    graph.set_engine_version_for_test("0.0.0-altra");
    let inputs = single_input("main", vec![table_batch(&[1], &["a"])]);
    match execute(&graph, inputs, RuntimeContext::default()) {
        Err(PlenoraError::InvalidPlan(message)) => {
            assert!(message.contains("GRAPH_MISMATCH"), "{message}");
        }
        Err(other) => panic!("atteso Contract GRAPH_MISMATCH, ottenuto {other}"),
        Ok(_) => panic!("atteso il rifiuto del grafo incompatibile"),
    }

    // Controllo positivo: a identita' coerente lo stesso piano esegue.
    let graph = validate(&plan.to_string(), &contracts).expect("validate");
    let inputs = single_input("main", vec![table_batch(&[1], &["a"])]);
    execute(&graph, inputs, RuntimeContext::default()).expect("grafo compatibile accettato");
}

// ---------------------------------------------------------------------------
// Panic dei kernel al confine dell'executor (ADR 3)
// ---------------------------------------------------------------------------

/// Guard che deregistra il proprio nodo dall'hook di iniezione panic anche
/// se il test fallisce. Ogni test usa un id di nodo distinto: i test girano
/// in parallelo nello stesso processo e l'hook e' globale.
struct PanicHookGuard {
    node: String,
}

impl PanicHookGuard {
    fn set(node: &str) -> Self {
        super::PANIC_AT_NODES
            .lock()
            .expect("hook panic")
            .push(node.to_owned());
        Self {
            node: node.to_owned(),
        }
    }
}

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        if let Ok(mut hook) = super::PANIC_AT_NODES.lock() {
            hook.retain(|node| node != &self.node);
        }
    }
}

/// Piano con un nodo `table.filter` (streaming) con id dato.
fn panic_plan(node: &str) -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": node, "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">=", "value": 0}},
        ],
        "output": node,
    })
}

#[test]
fn kernel_panic_becomes_step_error_attributed_to_node() {
    let _guard = PanicHookGuard::set("boom_stream");
    let inputs = single_input("main", vec![table_batch(&[1], &["a"])]);
    let output = run(
        &panic_plan("boom_stream"),
        inputs,
        &[("main".to_owned(), table_contract())],
    )
    .expect("execute");
    let error = output.collect_batches().expect_err("panic convertito in errore");
    match error {
        PlenoraError::Execution {
            node,
            operation,
            reason,
            ..
        } => {
            assert_eq!(node, "boom_stream", "attribuzione al nodo che e' andato in panic");
            assert_eq!(operation, "table.filter");
            assert!(
                reason.contains("panic di test iniettato"),
                "il motivo riporta il messaggio del panic: {reason}"
            );
        }
        other => panic!("atteso Step, ottenuto {other}"),
    }
}

#[test]
fn blocking_kernel_panic_becomes_step_error_attributed_to_node() {
    let _guard = PanicHookGuard::set("boom_block");
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "boom_block", "op": "table.aggregate", "in": ["main"],
             "config": {"group_by": ["id"], "aggregations": []}},
        ],
        "output": "boom_block",
    });
    let inputs = single_input("main", vec![table_batch(&[1, 2], &["a", "b"])]);
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");
    let error = output.collect_batches().expect_err("panic convertito in errore");
    match error {
        PlenoraError::Execution {
            node,
            operation,
            reason,
            ..
        } => {
            assert_eq!(node, "boom_block");
            assert_eq!(operation, "table.aggregate");
            assert!(reason.contains("panic di test iniettato"), "{reason}");
        }
        other => panic!("atteso Step, ottenuto {other}"),
    }
}

#[test]
fn binary_kernel_panic_becomes_step_error_attributed_to_node() {
    let _guard = PanicHookGuard::set("boom_join");
    let plan = json!({
        "schema_version": 4,
        "inputs": ["left", "right"],
        "nodes": [
            {"id": "boom_join", "op": "table.join", "in": ["left", "right"],
             "config": {"left_keys": ["id"], "right_keys": ["id"]}},
        ],
        "output": "boom_join",
    });
    let inputs = Inputs::new()
        .with("left", Input::from_batches(vec![table_batch(&[1], &["a"])]).expect("left"))
        .expect("inputs")
        .with(
            "right",
            Input::from_batches(vec![table_batch(&[1], &["b"])]).expect("right"),
        )
        .expect("inputs");
    let contracts = vec![
        ("left".to_owned(), table_contract()),
        ("right".to_owned(), table_contract()),
    ];
    let output = run(&plan, inputs, &contracts).expect("execute");
    let error = output.collect_batches().expect_err("panic convertito in errore");
    match error {
        PlenoraError::Execution {
            node,
            operation,
            reason,
            ..
        } => {
            assert_eq!(node, "boom_join", "attribuzione anche per i segmenti BinaryBlocking");
            assert_eq!(operation, "table.join");
            assert!(reason.contains("panic di test iniettato"), "{reason}");
        }
        other => panic!("atteso Step, ottenuto {other}"),
    }
}

#[test]
fn kernel_panic_publishes_nothing() {
    let _guard = PanicHookGuard::set("boom_pub");
    let inputs = single_input("main", vec![table_batch(&[1], &["a"])]);
    let output = run(
        &panic_plan("boom_pub"),
        inputs,
        &[("main".to_owned(), table_contract())],
    )
    .expect("execute");

    let directory = tempfile::tempdir().expect("tempdir");
    let destination = directory.path().join("output.arrow");
    let result = output.write_ipc_file(&destination);
    assert!(result.is_err());
    assert!(
        !destination.exists(),
        "nessun publish dopo panic (ADR 3): il tempfile e' eliminato"
    );
}

// ---------------------------------------------------------------------------
// Cancellazione cooperativa (ADR 3, M1c), errori arricchiti (M1d), TempStore
// ---------------------------------------------------------------------------

/// Input lazy che cancella il token dopo `cancel_after` batch emessi: simula
/// un Ctrl-C che arriva mentre lo stream scorre (esecuzione seriale: il
/// cancel e' osservato al confine dell'executor successivo alla pull che lo
/// ha prodotto).
struct CancellingInput {
    schema: SchemaRef,
    total: usize,
    emitted: usize,
    cancel_after: usize,
    token: CancellationToken,
}

impl Iterator for CancellingInput {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.emitted == self.total {
            return None;
        }
        self.emitted += 1;
        if self.emitted > self.cancel_after {
            self.token.cancel();
        }
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

/// Guard dell'hook di override del `CancellationBehavior` (stesso pattern di
/// `PanicHookGuard`): registra alla creazione, deregistra al Drop. Gli id
/// nodo registrati devono essere UNIVOCI per test: l'hook e' globale al
/// processo e i test girano in parallelo.
struct CancelBehaviorGuard {
    node: String,
}

impl CancelBehaviorGuard {
    fn set(node: &str, behavior: CancellationBehavior) -> Self {
        super::CANCEL_BEHAVIOR_OVERRIDES
            .lock()
            .expect("hook behavior")
            .push((node.to_owned(), behavior));
        Self {
            node: node.to_owned(),
        }
    }
}

impl Drop for CancelBehaviorGuard {
    fn drop(&mut self) {
        if let Ok(mut hook) = super::CANCEL_BEHAVIOR_OVERRIDES.lock() {
            hook.retain(|(node, _)| node != &self.node);
        }
    }
}

/// Catena streaming di due op `Cooperative` (`table.filter`, `table.rename`).
fn streaming_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">=", "value": 0}},
            {"id": "r", "op": "table.rename", "in": ["f"],
             "config": {"renames": [{"old_name": "name", "new_name": "label"}]}},
        ],
        "output": "r",
    })
}

fn cancelling_input(total: usize, cancel_after: usize, token: &CancellationToken) -> Input {
    Input::from_iter(
        table_schema(),
        CancellingInput {
            schema: table_schema(),
            total,
            emitted: 0,
            cancel_after,
            token: token.clone(),
        },
    )
}

fn execute_with_token(
    plan: &serde_json::Value,
    input: Input,
    token: CancellationToken,
) -> Result<Output> {
    let graph = validate(&plan.to_string(), &[("main".to_owned(), table_contract())])?;
    let inputs = Inputs::new().with("main", input).expect("input");
    let runtime = RuntimeContext {
        cancellation: token,
        ..RuntimeContext::default()
    };
    execute(&graph, inputs, runtime)
}

#[test]
fn cancel_between_batches_in_streaming_chain() {
    let token = CancellationToken::new();
    let mut output = execute_with_token(&streaming_plan(), cancelling_input(5, 1, &token), token)
        .expect("execute");

    // Primo batch: consegnato. Il cancel scatta alla produzione del secondo.
    output.next().expect("primo batch").expect("batch ok");
    match output.next() {
        Some(Err(PlenoraError::Cancelled {
            node,
            operation,
            execution_id,
            ..
        })) => {
            assert_eq!(
                node, "f",
                "attribuzione al primo kernel della catena (Cooperative: check a ogni batch)"
            );
            assert_eq!(operation, "table.filter");
            assert_eq!(execution_id, output.execution_id());
        }
        other => panic!("atteso Cancelled: {other:?}"),
    }
    // Metriche parziali osservabili al punto di cancel (ADR 3): solo il
    // primo batch e' entrato nella catena.
    let metrics = output.metrics();
    assert_eq!(metrics.nodes["f"].batches_in, 1);
    assert_eq!(metrics.output_batches, 1);
}

#[test]
fn cancel_during_blocking_drain() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "g", "op": "table.aggregate", "in": ["main"],
             "config": {"group_by": ["id"], "aggregations": []}},
        ],
        "output": "g",
    });
    let token = CancellationToken::new();
    let mut output =
        execute_with_token(&plan, cancelling_input(5, 1, &token), token).expect("execute");

    // La prima pull guida il drenaggio: il secondo batch cancella il token
    // e il check tra batch (BoundaryOnly: durante il drenaggio) ferma il
    // segmento prima del kernel monolitico.
    match output.next() {
        Some(Err(PlenoraError::Cancelled { node, operation, .. })) => {
            assert_eq!(node, "g");
            assert_eq!(operation, "table.aggregate");
        }
        other => panic!("atteso Cancelled durante il drenaggio: {other:?}"),
    }
    // Il kernel non e' mai partito.
    assert_eq!(output.metrics().nodes["g"].rows_in, 0);
}

#[test]
fn non_interruptible_op_is_never_interrupted() {
    // Le sole op `NonInterruptible` del catalogo v1 richiedono i backend
    // opzionali (geos/proj): l'hook marca i kernel del piano con il behavior
    // da verificare (il gating e' quello di catalogo, l'op e' irrilevante).
    // Id nodo UNIVOCI: l'hook e' globale al processo e i test girano in
    // parallelo (stessa disciplina dei nomi di `PanicHookGuard`).
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "ni_f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">=", "value": 0}},
            {"id": "ni_r", "op": "table.rename", "in": ["ni_f"],
             "config": {"renames": [{"old_name": "name", "new_name": "label"}]}},
        ],
        "output": "ni_r",
    });
    let _guard_f = CancelBehaviorGuard::set("ni_f", CancellationBehavior::NonInterruptible);
    let _guard_r = CancelBehaviorGuard::set("ni_r", CancellationBehavior::NonInterruptible);
    let token = CancellationToken::new();
    let mut output = execute_with_token(&plan, cancelling_input(5, 1, &token), token)
        .expect("execute");

    output.next().expect("primo batch").expect("batch ok");
    // Il secondo batch attraversa comunque entrambi i kernel
    // (`NonInterruptible`: nessun check); il cancel e' osservato al confine
    // di piano, prima della consegna (ADR 3: nessuna nuova attivita' dopo
    // la cancellazione).
    match output.next() {
        Some(Err(PlenoraError::Cancelled { node, operation, .. })) => {
            assert_eq!(node, "ni_r", "confine di output del piano");
            assert_eq!(operation, "output");
        }
        other => panic!("atteso Cancelled al confine di piano: {other:?}"),
    }
    // L'op non e' stata interrotta: ha processato anche il batch successivo
    // alla cancellazione (latenza osservabile nelle metriche, ADR 3).
    assert_eq!(output.metrics().nodes["ni_f"].batches_in, 2);
}

#[test]
fn cancelled_run_publishes_nothing_and_reports_execution_id() {
    let token = CancellationToken::new();
    let output = execute_with_token(&panic_plan("f"), cancelling_input(5, 1, &token), token)
        .expect("execute");
    // Formato documentato: prefisso leggibile + UUID v4 semplice (32 hex) —
    // charset compatibile con la validazione del TempStore.
    let execution_id = output.execution_id().to_owned();
    assert!(execution_id.starts_with("exec-"), "{execution_id}");
    assert_eq!(execution_id.len(), 5 + 32, "{execution_id}");
    assert!(
        execution_id[5..].chars().all(|c| c.is_ascii_hexdigit()),
        "{execution_id}"
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let destination = directory.path().join("output.arrow");
    match output.write_ipc_file(&destination) {
        Err(PlenoraError::Cancelled { .. }) => {}
        other => panic!("atteso Cancelled: {other:?}"),
    }
    assert!(
        !destination.exists(),
        "nessun publish dopo cancel (invariante I8)"
    );
    assert!(
        std::fs::read_dir(directory.path())
            .expect("lettura directory")
            .next()
            .is_none(),
        "nessun tempfile parziale residuo"
    );
}

#[test]
fn execution_ids_are_unique_per_execute() {
    let contracts = [("main".to_owned(), table_contract())];
    let first = run(
        &panic_plan("f"),
        single_input("main", vec![table_batch(&[1], &["a"])]),
        &contracts,
    )
    .expect("execute");
    let second = run(
        &panic_plan("f"),
        single_input("main", vec![table_batch(&[1], &["a"])]),
        &contracts,
    )
    .expect("execute");
    assert_ne!(first.execution_id(), second.execution_id());
}

#[test]
fn execute_creates_temp_store_and_cleans_it_up() {
    let root = tempfile::tempdir().expect("root");
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let graph =
        validate(&plan.to_string(), &[("main".to_owned(), table_contract())]).expect("validate");
    let runtime = RuntimeContext {
        temp_root: Some(root.path().to_owned()),
        ..RuntimeContext::default()
    };
    let output = execute(&graph, single_input("main", vec![table_batch(&[1], &["a"])]), runtime)
        .expect("execute");
    let store_dir = root
        .path()
        .read_dir()
        .expect("lettura root")
        .map(|entry| entry.expect("entry").path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("plenora-{}-", output.execution_id()))
                })
        })
        .expect("store dell'esecuzione presente mentre l'Output e' vivo");
    assert!(store_dir.join("lock.json").is_file(), "lock ADR 3 scritto");

    let (batches, _) = output.collect_batches().expect("stream ok");
    assert_eq!(batches.len(), 1);
    assert!(
        !store_dir.exists(),
        "cleanup RAII: il Drop dello stato rimuove directory e lock"
    );
}

#[test]
fn execute_scavenges_stale_temp_dirs_at_startup() {
    let root = tempfile::tempdir().expect("root");
    // Directory orfana: lock con PID vivo ma heartbeat antico (TTL scaduto).
    let stale = root.path().join("plenora-exec-stale-xyz");
    std::fs::create_dir(&stale).expect("mkdir stale");
    std::fs::write(
        stale.join("lock.json"),
        serde_json::json!({
            "execution_id": "exec-stale",
            "pid": std::process::id(),
            "hostname": "test-host",
            "created_unix_secs": 1,
            "heartbeat_unix_secs": 1,
        })
        .to_string(),
    )
    .expect("lock stale");
    // Voce fuori pattern `plenora-*`: mai toccata.
    let other = root.path().join("altro");
    std::fs::create_dir(&other).expect("mkdir other");

    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let graph =
        validate(&plan.to_string(), &[("main".to_owned(), table_contract())]).expect("validate");
    let runtime = RuntimeContext {
        temp_root: Some(root.path().to_owned()),
        ..RuntimeContext::default()
    };
    let output = execute(&graph, single_input("main", vec![table_batch(&[1], &["a"])]), runtime)
        .expect("execute");
    assert!(!stale.exists(), "scavenging all'avvio: orfana rimossa");
    assert!(other.exists(), "fuori pattern: mai toccata");
    drop(output.collect_batches().expect("stream ok"));
}

#[test]
fn diagnostics_off_leaves_step_error_unchanged() {
    let _guard = PanicHookGuard::set("diag_off");
    let inputs = single_input("main", vec![table_batch(&[1], &["a"])]);
    let output = run(
        &panic_plan("diag_off"),
        inputs,
        &[("main".to_owned(), table_contract())],
    )
    .expect("execute");
    let error = output.collect_batches().expect_err("panic convertito");
    match error {
        PlenoraError::Execution {
            reason,
            execution_id,
            ..
        } => {
            assert_eq!(
                reason,
                "contract violation: panic nel kernel: panic di test iniettato al nodo `diag_off`",
                "diagnostics spento: motivo invariato (retrocompatibile)"
            );
            assert!(
                !execution_id.is_empty(),
                "execution_id sempre presente negli errori DAG (M1d)"
            );
        }
        other => panic!("atteso Step: {other:?}"),
    }
}

#[test]
fn diagnostics_on_enriches_step_error_with_batch_index() {
    let _guard = PanicHookGuard::set("diag_on");
    let graph = validate(
        &panic_plan("diag_on").to_string(),
        &[("main".to_owned(), table_contract())],
    )
    .expect("validate");
    let runtime = RuntimeContext {
        diagnostics: true,
        ..RuntimeContext::default()
    };
    let output = execute(&graph, single_input("main", vec![table_batch(&[1], &["a"])]), runtime)
        .expect("execute");
    let error = output.collect_batches().expect_err("panic convertito");
    match error {
        PlenoraError::Execution { reason, .. } => {
            assert!(reason.contains("panic di test iniettato"), "{reason}");
            assert!(
                reason.contains("[batch_seq=0]"),
                "contesto strutturale aggiunto (indice di batch, mai valori): {reason}"
            );
        }
        other => panic!("atteso Step: {other:?}"),
    }
}

#[test]
fn diagnostics_on_wkb_error_adds_column_context_without_values() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let run_with = |diagnostics: bool| {
        let graph =
            validate(&plan.to_string(), &[("main".to_owned(), geo_contract())]).expect("validate");
        let runtime = RuntimeContext {
            diagnostics,
            ..RuntimeContext::default()
        };
        execute(
            &graph,
            single_input("main", vec![geo_batch(&[1], &[Some(b"non-e-wkb".to_vec())])]),
            runtime,
        )
        .expect("execute")
        .collect_batches()
        .expect_err("WKB invalido")
        .to_string()
    };
    let off = run_with(false);
    assert!(off.contains("(riga 0)"), "{off}");
    assert!(
        !off.contains("colonna"),
        "diagnostics spento: messaggio invariato: {off}"
    );
    let on = run_with(true);
    assert!(
        on.contains("colonna `geom`"),
        "contesto colonna a flag attivo: {on}"
    );
    assert!(
        !on.contains("non-e-wkb"),
        "mai valori: il payload della cella non compare: {on}"
    );
}

// ---------------------------------------------------------------------------
// Spill generalizzato (ADR-0002, Fase 2B M2c): attivazione preventiva al
// dispatch, TempStore condiviso, metriche e quota temp.
// ---------------------------------------------------------------------------

/// Piano con un `table.distinct` e limiti dati espliciti: con
/// `max_memory_bytes` sotto i byte stimati dell'input il dispatch attiva
/// `distinct_spilled` (soglia deterministica ADR-0002).
fn distinct_plan(max_memory_bytes: u64, max_temp_bytes: u64) -> serde_json::Value {
    json!({
        "schema_version": 4,
        "limits": {"max_memory_bytes": max_memory_bytes, "max_temp_bytes": max_temp_bytes},
        "inputs": ["main"],
        "nodes": [
            {"id": "d", "op": "table.distinct", "in": ["main"],
             "config": {"subset": ["id"], "keep": "first"}},
        ],
        "output": "d",
    })
}

/// 2048 righe con sole 8 chiavi distinte, in batch da 64: ogni batch singolo
/// entra in un budget di `2 * batch_bytes`, il totale no — lo scenario di
/// spill (lo stream eccede cumulativamente il budget).
fn skewed_batches() -> (Vec<RecordBatch>, u64) {
    let ids: Vec<i64> = (0..2048).map(|i| i % 8).collect();
    let names: Vec<String> = ids.iter().map(|id| format!("name{id}")).collect();
    let names_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let batches: Vec<RecordBatch> = ids
        .chunks(64)
        .zip(names_refs.chunks(64))
        .map(|(chunk_ids, chunk_names)| table_batch(chunk_ids, chunk_names))
        .collect();
    let batch_bytes = batches[0].get_array_memory_size() as u64;
    let total_bytes: u64 = batches
        .iter()
        .map(|batch| batch.get_array_memory_size() as u64)
        .sum();
    let memory_budget = batch_bytes * 2;
    assert!(
        total_bytes > memory_budget,
        "lo spill deve essere inevitabile: {total_bytes} <= {memory_budget}"
    );
    (batches, memory_budget)
}

/// Percorso dello store temporaneo dell'esecuzione dentro `root`.
fn store_dir_of(root: &std::path::Path, execution_id: &str) -> Option<std::path::PathBuf> {
    root.read_dir()
        .expect("lettura root")
        .map(|entry| entry.expect("entry").path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&format!("plenora-{execution_id}-")))
        })
}

#[test]
fn distinct_spills_end_to_end_into_shared_temp_store() {
    let root = tempfile::tempdir().expect("root");
    let (batches, memory_budget) = skewed_batches();
    let graph = validate(
        &distinct_plan(memory_budget, 1 << 30).to_string(),
        &[("main".to_owned(), table_contract())],
    )
    .expect("validate");
    let runtime = RuntimeContext {
        temp_root: Some(root.path().to_owned()),
        ..RuntimeContext::default()
    };
    let output = execute(&graph, single_input("main", batches), runtime).expect("execute");

    // Mentre l'Output e' vivo lo store esiste e la sotto-directory di spill
    // non contiene residui: i file sono ripuliti a fine operazione
    // (cleanup/Drop del workspace di kernels-table).
    let store_dir = store_dir_of(root.path(), output.execution_id())
        .expect("store dell'esecuzione presente mentre l'Output e' vivo");
    let spill_dir = store_dir.join("spill");
    if spill_dir.exists() {
        assert!(
            spill_dir.read_dir().expect("read_dir spill").next().is_none(),
            "nessun file di spill residuo durante l'esecuzione"
        );
    }

    let (spilled, metrics) = output.collect_batches().expect("stream ok");
    let spilled_rows: usize = spilled.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(spilled_rows, 8, "8 chiavi distinte");
    // Metriche di spill aggregate (ADR-0002): lo spill e' avvenuto.
    assert!(metrics.spill.bytes_written > 0, "{:?}", metrics.spill);
    assert!(metrics.spill.bytes_read > 0, "{:?}", metrics.spill);
    assert!(metrics.spill.files > 0, "{:?}", metrics.spill);
    // Cleanup RAII completo: nessun residuo nella temp root iniettata.
    assert!(
        root.path().read_dir().expect("read_dir root").next().is_none(),
        "TempStore rimosso al Drop: nessun residuo"
    );

    // Oracolo: stesso piano con budget ampio (percorso in memoria) — output
    // identico e nessuna metrica di spill.
    let (batches, _) = skewed_batches();
    let output = run(
        &distinct_plan(1 << 40, 1 << 30),
        single_input("main", batches),
        &[("main".to_owned(), table_contract())],
    )
    .expect("execute");
    let (expected, oracle_metrics) = output.collect_batches().expect("stream ok");
    assert_eq!(oracle_metrics.spill, SpillMetrics::default());
    assert_eq!(spilled, expected, "output spilled identico al percorso in memoria");
}

#[test]
fn spill_temp_quota_exceeded_fails_with_dedicated_error() {
    let root = tempfile::tempdir().expect("root");
    let (batches, memory_budget) = skewed_batches();
    let graph = validate(
        &distinct_plan(memory_budget, 1).to_string(),
        &[("main".to_owned(), table_contract())],
    )
    .expect("validate");
    let runtime = RuntimeContext {
        temp_root: Some(root.path().to_owned()),
        ..RuntimeContext::default()
    };
    let error = execute(&graph, single_input("main", batches), runtime)
        .expect("execute")
        .collect_batches()
        .expect_err("quota temp superata: errore dedicato");
    assert!(
        error.to_string().contains("max_temp_bytes"),
        "errore dedicato max_temp_bytes: {error}"
    );
    // Nessun residuo anche sul percorso di errore (Drop del workspace +
    // Drop RAII del TempStore).
    assert!(
        root.path().read_dir().expect("read_dir root").next().is_none(),
        "nessun residuo nella temp root dopo l'errore"
    );
}

// ---------------------------------------------------------------------------
// Fusione dei segmenti geo (ADR-0012)
// ---------------------------------------------------------------------------

/// Catena buffer -> simplify -> centroid: tre kernel fondibili consecutivi
/// (perimetro M1, capability `TransformInPlace`).
fn geo_fusion_chain_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 5.0}},
            {"id": "s", "op": "geo.simplify", "in": ["b"], "config": {"tolerance": 0.01}},
            {"id": "c", "op": "geo.centroid", "in": ["s"], "config": {}},
        ],
        "output": "c",
    })
}

fn square_wkb(origin_x: f64, origin_y: f64, side: f64) -> Vec<u8> {
    Geometry::Polygon(polygon![
        (x: origin_x, y: origin_y),
        (x: origin_x + side, y: origin_y),
        (x: origin_x + side, y: origin_y + side),
        (x: origin_x, y: origin_y + side),
        (x: origin_x, y: origin_y),
    ])
    .to_wkb(CoordDimensions::xy())
    .expect("wkb fixture")
}

fn run_geo_fusion(
    plan: &serde_json::Value,
    batches: Vec<RecordBatch>,
    geo_fusion: bool,
) -> Result<(Vec<RecordBatch>, ExecutionMetrics)> {
    let graph = validate(&plan.to_string(), &[("main".to_owned(), geo_contract())])?;
    let runtime = RuntimeContext {
        geo_fusion,
        ..RuntimeContext::default()
    };
    output_rows(execute(&graph, single_input("main", batches), runtime)?)
}

fn fusion_fixture_batches() -> Vec<RecordBatch> {
    vec![
        geo_batch(
            &[0, 1, 2],
            &[
                Some(point_wkb(0.0, 0.0)),
                Some(square_wkb(10.0, 10.0, 20.0)),
                None,
            ],
        ),
        geo_batch(
            &[3, 4],
            &[
                Some(point_wkb(-5.0, 7.0)),
                Some(square_wkb(-100.0, 0.0, 5.0)),
            ],
        ),
    ]
}

/// A/B via kill switch (D12.9): la pipeline di tre geo transform fusi
/// produce output identico al percorso non fuso, con metriche per nodo
/// preservate (D12.6).
#[test]
fn fused_geo_group_matches_unfused_output_and_keeps_per_node_metrics() {
    let plan = geo_fusion_chain_plan();
    let (fused_batches, fused_metrics) =
        run_geo_fusion(&plan, fusion_fixture_batches(), true).expect("fuso");
    let (plain_batches, plain_metrics) =
        run_geo_fusion(&plan, fusion_fixture_batches(), false).expect("non fuso");

    assert_eq!(fused_batches, plain_batches, "output fuso diverso dal non fuso");

    for node in ["b", "s", "c"] {
        let fused_node = &fused_metrics.nodes[node];
        let plain_node = &plain_metrics.nodes[node];
        assert_eq!(fused_node.rows_in, 5, "{node}: righe in ingresso 1:1");
        assert_eq!(fused_node.rows_out, 5, "{node}: righe prodotte 1:1");
        assert_eq!(fused_node.batches_in, 2, "{node}: batch in ingresso");
        assert_eq!(fused_node.rows_in, plain_node.rows_in, "{node}: righe in A/B");
        assert_eq!(fused_node.rows_out, plain_node.rows_out, "{node}: righe out A/B");
    }
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0);
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
}

/// Poligono semplice valido (anello da `coords` coordinate su un cerchio):
/// gonfia i byte decodificati stimati per il test di fallback governor.
// Angoli della fixture: conteggi piccoli ed esatti anche in f64.
#[allow(clippy::cast_precision_loss)]
fn circle_polygon_wkb(coords: usize) -> Vec<u8> {
    let mut ring: Vec<(f64, f64)> = (0..coords - 1)
        .map(|index| {
            let angle = index as f64 / (coords - 1) as f64 * std::f64::consts::TAU;
            (1_000.0 * angle.cos(), 1_000.0 * angle.sin())
        })
        .collect();
    ring.push((1_000.0, 0.0)); // chiusura esatta sull'angolo 0
    Geometry::Polygon(geo::Polygon::new(geo::LineString::from(ring), vec![]))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb fixture")
}

/// Reservation governor fallita (D12.7): il budget copre il lease dell'input
/// e quello dell'output (centroid -> punti, pochi byte) ma NON input + byte
/// decodificati stimati (~64 KiB): il batch ricade sul percorso non fuso con
/// metrica dedicata — mai silenzioso, mai un errore nuovo. E' anche la prova
/// che il governor scatta davvero su un batch oltre soglia (condizione di
/// entrata in vigore della deroga DER-003, D12.8).
#[test]
fn geo_fusion_falls_back_when_the_governor_rejects_the_reservation() {
    let cells = || {
        vec![
            Some(circle_polygon_wkb(2_000)),
            Some(circle_polygon_wkb(2_000)),
        ]
    };
    let batch = geo_batch(&[0, 1], &cells());
    let budget = batch.get_array_memory_size() as u64 + 4_096;
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "limits": {"max_memory_bytes": budget},
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 5.0}},
            {"id": "s", "op": "geo.simplify", "in": ["b"], "config": {"tolerance": 0.01}},
            {"id": "c", "op": "geo.centroid", "in": ["s"], "config": {}},
        ],
        "output": "c",
    });

    let (fused_batches, fused_metrics) =
        run_geo_fusion(&plan, vec![batch], true).expect("fallback, non errore");
    let (plain_batches, plain_metrics) =
        run_geo_fusion(&plan, vec![geo_batch(&[0, 1], &cells())], false).expect("non fuso");

    assert_eq!(fused_metrics.geo_fusion_fallbacks, 1, "un batch -> un fallback");
    assert_eq!(fused_batches, plain_batches, "output diverso dal non fuso");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
}
