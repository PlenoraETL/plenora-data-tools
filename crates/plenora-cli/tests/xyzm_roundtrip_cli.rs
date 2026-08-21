//! Test end-to-end CLI del round-trip XYZM (milestone B1.4): un input IPC
//! con colonna geometria Point Z (`geo.dimensions = "xyz"`) attraversa un
//! piano tabellare (passthrough) preservando celle byte-per-byte e metadato
//! dimensionale; un'op geo elaborante sullo stesso input e' rifiutata a
//! compile-plan (nessun output pubblicato); un metadato `xyz` con celle XY
//! (incoerenza) e' un errore al gate di lettura, mai un passthrough
//! silenzioso.
//!
//! Come i test geo di `dag_v4_cli.rs`, richiedono la risoluzione CRS PROJ
//! (la scoperta del contratto risolve il `geo.crs` dei metadati): l'intero
//! file e' compilato solo con la feature `proj-backend`.
#![cfg(feature = "proj-backend")]

use std::process::Command;
use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, BinaryArray, Int64Array, RecordBatch};
use plenora_core::arrow::ipc::reader::FileReader;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::GeometryDimensions;
use serde_json::json;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_plenora-data-tools"))
}

/// Schema `id` Int64 + colonna `geometry` GeoArrow-WKB con dimensionalita'
/// XYZ dichiarata nel metadato `geo` (EPSG:32632).
fn xyz_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        plenora_kernels_geo::arrow_adapter::geometry_output_field_with_dimensions(
            "geometry",
            "EPSG:32632",
            GeometryDimensions::Xyz,
        )
        .expect("campo geometria xyz"),
    ]))
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

/// WKB ISO little-endian di un Point XY (type code 1): celle incoerenti con
/// un metadato `dimensions = "xyz"` (fixture adversarial).
fn xy_point_wkb(x: f64, y: f64) -> Vec<u8> {
    let mut payload = vec![1_u8];
    payload.extend_from_slice(&1_u32.to_le_bytes());
    payload.extend_from_slice(&x.to_le_bytes());
    payload.extend_from_slice(&y.to_le_bytes());
    payload
}

fn xyz_batch(ids: &[i64], cells: &[Option<Vec<u8>>]) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    RecordBatch::try_new(
        xyz_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geo xyz fixture")
}

fn write_input(directory: &std::path::Path, batch: &RecordBatch) -> std::path::PathBuf {
    let input = directory.join("input.arrow");
    let file = std::fs::File::create(&input).expect("create input");
    let mut writer = FileWriter::try_new(file, &xyz_schema()).expect("writer");
    writer.write(batch).expect("write batch");
    writer.finish().expect("finish");
    input
}

fn write_plan(directory: &std::path::Path, plan: &serde_json::Value) -> std::path::PathBuf {
    let plan_path = directory.join("plan.json");
    std::fs::write(&plan_path, serde_json::to_vec(plan).expect("json")).expect("plan");
    plan_path
}

fn run_cli(
    plan: &std::path::Path,
    input: &std::path::Path,
    output: &std::path::Path,
) -> std::process::Output {
    cli()
        .args(["run", "--plan"])
        .arg(plan)
        .arg("--inputs")
        .arg(input)
        .arg("--output")
        .arg(output)
        .output()
        .expect("run")
}

/// Dimensionalita' dichiarata nel metadato `geo` della colonna `geometry`
/// dell'output pubblicato.
fn output_dimensions(output_path: &std::path::Path) -> GeometryDimensions {
    let reader = FileReader::try_new(std::fs::File::open(output_path).expect("output"), None)
        .expect("reader");
    let schema = reader.schema();
    let field = schema.field_with_name("geometry").expect("campo geometry");
    plenora_kernels_geo::arrow_adapter::geometry_dimensions_from_metadata(field)
}

#[test]
fn xyz_input_round_trips_byte_per_byte_through_a_table_filter() {
    // Round-trip B1.4: Point Z (metadato geo.dimensions = "xyz") -> filtro
    // tabellare (passthrough) -> publish -> output IPC riletto: celle
    // byte-identiche e metadato `xyz` preservato (mai un xy silenzioso).
    let directory = tempfile::tempdir().expect("tempdir");
    let kept_a = xyz_point_wkb(1.0, 2.0, 3.0);
    let kept_b = xyz_point_wkb(4.0, 5.0, 6.0);
    let dropped = xyz_point_wkb(7.0, 8.0, 9.0);
    let input = write_input(
        directory.path(),
        &xyz_batch(
            &[2, 1, 3],
            &[Some(kept_a.clone()), Some(dropped), Some(kept_b.clone())],
        ),
    );
    let plan = write_plan(
        directory.path(),
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "id", "operator": ">", "value": 1}},
            ],
            "output": "f",
        }),
    );
    let output_path = directory.path().join("output.arrow");

    let run = run_cli(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&run.stdout)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&run.stdout).expect("JSON metriche");
    assert_eq!(metrics["status"], "ok");
    assert_eq!(metrics["output_rows"], 2);

    // Metadato dimensionale preservato nell'output pubblicato.
    assert_eq!(output_dimensions(&output_path), GeometryDimensions::Xyz);

    // Celle rilette byte-identiche alle sole righe filtrate.
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let batches: Vec<RecordBatch> = reader.map(|batch| batch.expect("batch")).collect();
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 2);
    let (geometry_index, _) = batches[0]
        .schema()
        .column_with_name("geometry")
        .expect("colonna geometry");
    let cells = batches[0]
        .column(geometry_index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("colonna geometria binaria");
    assert_eq!(cells.value(0), kept_a.as_slice(), "cella 0 byte-per-byte");
    assert_eq!(cells.value(1), kept_b.as_slice(), "cella 1 byte-per-byte");
}

#[test]
fn geo_op_on_xyz_input_is_rejected_at_compile_plan_without_output() {
    // B1.4: op geo elaborante (`geo.buffer`) su input XYZ -> rifiuto a
    // compile-plan con messaggio che cita op e dimensionalita'; publish
    // atomico: nessun file di output.
    let directory = tempfile::tempdir().expect("tempdir");
    let input = write_input(
        directory.path(),
        &xyz_batch(&[1, 2], &[Some(xyz_point_wkb(1.0, 2.0, 3.0)), None]),
    );
    let plan = write_plan(
        directory.path(),
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "b", "op": "geo.buffer", "in": ["main"],
                 "config": {"distance": 10.0}},
            ],
            "output": "b",
        }),
    );
    let output_path = directory.path().join("output.arrow");

    let run = run_cli(&plan, &input, &output_path);
    assert!(!run.status.success(), "input XYZ accettato da geo.buffer");
    let stderr = String::from_utf8_lossy(&run.stdout);
    assert!(
        stderr.contains("geo.buffer"),
        "l'errore cita l'op: {stderr}"
    );
    assert!(
        stderr.contains("xyz"),
        "l'errore cita la dimensionalita': {stderr}"
    );
    assert!(
        !output_path.exists(),
        "publish atomico: nessun output parziale"
    );
}

#[test]
fn xyz_metadata_with_xy_cells_fails_at_the_gate_never_silent_passthrough() {
    // Adversarial B1.4: metadato `dimensions = "xyz"` ma celle XY (type code
    // 1) -> errore dedicato di mismatch al gate di lettura (percorso CLI),
    // mai un passthrough silenzioso; nessun output pubblicato.
    let directory = tempfile::tempdir().expect("tempdir");
    let input = write_input(
        directory.path(),
        &xyz_batch(&[1], &[Some(xy_point_wkb(1.0, 2.0))]),
    );
    let plan = write_plan(
        directory.path(),
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "id", "operator": ">", "value": 0}},
            ],
            "output": "f",
        }),
    );
    let output_path = directory.path().join("output.arrow");

    let run = run_cli(&plan, &input, &output_path);
    assert!(
        !run.status.success(),
        "incoerenza metadato/celle passata silenziosamente"
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&run.stdout).expect("envelope row diagnostics");
    assert_eq!(envelope["error"]["category"], "data_mapping");
    assert_eq!(
        envelope["error"]["row_diagnostics"]["counts"]["geometry.invalid_wkb"],
        1
    );
    assert!(
        !output_path.exists(),
        "publish atomico: nessun output parziale"
    );
}
