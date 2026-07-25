//! Test end-to-end della CLI sul DAG v4 (Fase 2A): `validate` e `run` di
//! piani `schema_version: 4` attraverso planner/executor di `plenora-engine`,
//! con il piano legacy che continua a funzionare invariato.
//!
//! Le fixture Arrow sono generate nei test (stesso stile di
//! `roundtrip_smoke.rs`). Il test con geometria richiede la risoluzione CRS
//! PROJ (la scoperta del contratto risolve il `geo.crs` dei metadati): e'
//! compilato solo con la feature `proj-backend`.

use std::process::Command;
use std::sync::Arc;

use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc::reader::FileReader;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use serde_json::json;

#[cfg(feature = "proj-backend")]
use plenora_core::arrow::array::{Array, ArrayRef, BinaryArray, Float64Array};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_plenora-data-tools"))
}

fn write_ipc(path: &std::path::Path, schema: &SchemaRef, batches: &[RecordBatch]) {
    let file = std::fs::File::create(path).expect("create input");
    let mut writer = FileWriter::try_new(file, schema).expect("writer");
    for batch in batches {
        writer.write(batch).expect("write batch");
    }
    writer.finish().expect("finish");
}

fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]))
}

fn table_batch(ids: &[i64], names: &[&str]) -> RecordBatch {
    RecordBatch::try_new(
        table_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())),
            Arc::new(StringArray::from(
                names.iter().map(|name| Some(*name)).collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("batch fixture")
}

/// Piano v4 tabellare: filter `id > 0` poi rename `name` -> `label`.
fn table_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "r", "op": "table.rename", "in": ["f"],
             "config": {"renames": [{"old_name": "name", "new_name": "label"}]}},
        ],
        "output": "r",
    })
}

/// Scrive piano e input tabellare standard nella directory data.
fn write_table_fixture(directory: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let plan = directory.join("plan.json");
    let input = directory.join("input.arrow");
    std::fs::write(&plan, serde_json::to_vec(&table_plan()).expect("json")).expect("plan");
    write_ipc(
        &input,
        &table_schema(),
        &[table_batch(&[0, 1, 2], &["a", "b", "c"])],
    );
    (plan, input)
}

#[test]
fn validate_v4_stampa_il_riepilogo_del_dag() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());

    let result = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["schema_version"], 4);
    let plan_hash = summary["plan_hash"].as_str().expect("plan_hash");
    assert_eq!(plan_hash.len(), 64, "plan_hash SHA-256 esadecimale");
    assert_eq!(summary["inputs"], json!(["main"]));
    assert_eq!(summary["topological_order"], json!(["f", "r"]));

    let nodes = summary["nodes"].as_array().expect("nodi");
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["id"], "f");
    assert_eq!(nodes[0]["op"], "table.filter");

    let edges = summary["edges"].as_array().expect("archi");
    assert_eq!(edges.len(), 3, "un arco per input + uno per nodo");
    assert_eq!(edges[0]["edge"], "main");
    assert_eq!(edges[0]["kind"], "input");
    assert_eq!(edges[0]["contract"]["geometry"], serde_json::Value::Null);
    let field_names: Vec<&str> = edges[2]["contract"]["fields"]
        .as_array()
        .expect("campi")
        .iter()
        .filter_map(|field| field["name"].as_str())
        .collect();
    assert_eq!(field_names, ["id", "label"], "la rinomina e' inferita a secco");

    let segments = summary["segments"].as_array().expect("segmenti");
    assert_eq!(segments.len(), 1, "catena streaming fusa in un segmento");
    assert_eq!(segments[0]["mode"], "LinearStreaming");
    assert_eq!(segments[0]["parallelism"], "SerialFused");
    assert_eq!(segments[0]["nodes"], json!(["f", "r"]));

    assert_eq!(
        summary["required_capabilities"],
        json!([]),
        "nessuna capability per un piano solo tabellare"
    );
    assert_eq!(
        summary["input_contract_fingerprints"].as_array().map(Vec::len),
        Some(1)
    );
}

#[test]
fn run_v4_scrive_output_e_metriche_json_su_stdout() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON metriche");
    assert_eq!(metrics["status"], "ok");
    assert_eq!(metrics["output_rows"], 2, "filter id>0 lascia due righe");
    assert_eq!(metrics["nodes"]["f"]["operation"], "table.filter");
    assert_eq!(metrics["nodes"]["f"]["rows_in"], 3);
    assert_eq!(metrics["nodes"]["f"]["rows_out"], 2);
    assert_eq!(metrics["nodes"]["r"]["operation"], "table.rename");
    assert_eq!(metrics["nodes"]["r"]["rows_out"], 2);
    assert!(
        metrics["nodes"]["f"]["wall_time_ms"].as_f64().is_some(),
        "wall time per nodo presente"
    );
    assert!(metrics["segments"].as_object().is_some_and(|s| !s.is_empty()));

    // Output rileggibile: la rinomina e' applicata, le righe sono due.
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    assert!(reader.schema().field_with_name("label").is_ok());
    let rows: usize = reader.map(|batch| batch.expect("batch").num_rows()).sum();
    assert_eq!(rows, 2);
}

#[test]
fn run_v4_senza_inputs_fallisce() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, _input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("input"), "stderr: {stderr}");
    assert!(!output_path.exists(), "nessun output parziale");
}

#[test]
fn run_v4_schema_mismatch_fallisce_in_validazione() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");
    // Il piano filtra su una colonna assente dallo schema dell'input.
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "mancante", "operator": ">", "value": 0}},
            ],
            "output": "f",
        }))
        .expect("json"),
    )
    .expect("plan");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(!result.status.success());
    assert!(!result.stderr.is_empty(), "errore diagnostico presente");
    assert!(!output_path.exists(), "nessun output parziale");
}

#[test]
fn run_v4_non_sovrascrive_un_output_esistente() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");
    std::fs::write(&output_path, b"contenuto precedente").expect("output preesistente");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("esistente"), "stderr: {stderr}");
    assert_eq!(
        std::fs::read(&output_path).expect("output intatto"),
        b"contenuto precedente",
        "no-clobber: il file esistente non e' toccato"
    );
}

#[test]
fn piano_legacy_continua_a_funzionare_invariato() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    let input = directory.path().join("input.arrow");
    let output_path = directory.path().join("output.arrow");
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"rename","config":{"renames":[{"old_name":"name","new_name":"label"}]}}]}"#,
    )
    .expect("plan");
    write_ipc(&input, &table_schema(), &[table_batch(&[1, 2], &["a", "b"])]);

    // validate legacy: riepilogo di Fase 1, senza i campi del DAG v4.
    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate legacy");
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["steps"], 1);
    assert!(summary.get("plan_hash").is_none());

    // run legacy: stessa invocazione di sempre (--input singolo).
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run legacy");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    assert!(reader.schema().field_with_name("label").is_ok());
}

// ---------------------------------------------------------------------------
// Piano v4 misto (tabellare + geo): richiede il backend PROJ per la
// risoluzione del CRS nella scoperta del contratto di input.
// ---------------------------------------------------------------------------

/// Fixture geo: `id` Int64 + colonna `geometry` GeoArrow-WKB (EPSG:32632).
#[cfg(feature = "proj-backend")]
fn geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        plenora_kernels_geo::arrow_adapter::geometry_output_field("geometry", "EPSG:32632")
            .expect("campo geometria"),
    ]))
}

#[cfg(feature = "proj-backend")]
fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    use geo::{Geometry, Point};
    use geozero::{CoordDimensions, ToWkb};

    Geometry::Point(Point::new(x, y))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb fixture")
}

#[cfg(feature = "proj-backend")]
fn geo_batch(ids: &[i64], cells: &[Option<Vec<u8>>]) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    RecordBatch::try_new(
        geo_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geo fixture")
}

/// Piano v4 misto: filter `id > 0` -> buffer(10) -> area.
#[cfg(feature = "proj-backend")]
fn mixed_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 10.0}},
            {"id": "a", "op": "geo.area", "in": ["b"], "config": {}},
        ],
        "output": "a",
    })
}

#[cfg(feature = "proj-backend")]
#[test]
fn dag_v4_misto_geo_end_to_end() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    let input = directory.path().join("input.arrow");
    let output_path = directory.path().join("output.arrow");
    std::fs::write(&plan, serde_json::to_vec(&mixed_plan()).expect("json")).expect("plan");
    write_ipc(
        &input,
        &geo_schema(),
        &[geo_batch(
            &[0, 1, 2],
            &[Some(point_wkb(0.0, 0.0)), Some(point_wkb(100.0, 100.0)), None],
        )],
    );

    // validate-only: il contratto di input e' scoperto dai metadati GeoArrow.
    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    let edges = summary["edges"].as_array().expect("archi");
    let geometry = &edges[0]["contract"]["geometry"];
    assert_eq!(geometry["name"], "geometry");
    assert_eq!(geometry["crs"], "EPSG:32632");
    assert_eq!(geometry["crs_kind"], "Projected");

    // run: esecuzione del DAG e metriche per nodo.
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&run.stdout).expect("JSON metriche");
    assert_eq!(metrics["status"], "ok");
    assert_eq!(metrics["output_rows"], 2);
    assert_eq!(metrics["nodes"]["b"]["operation"], "geo.buffer");
    assert_eq!(metrics["nodes"]["b"]["rows_out"], 2);
    assert_eq!(metrics["nodes"]["a"]["operation"], "geo.area");

    // Output: geometria preservata dal buffer, colonna `area` aggiunta.
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let batches: Vec<RecordBatch> = reader.map(|batch| batch.expect("batch")).collect();
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 2);
    let batch = &batches[0];
    assert!(batch.schema().field_with_name("geometry").is_ok());
    let (area_index, _) = batch.schema().column_with_name("area").expect("colonna area");
    let areas = batch
        .column(area_index)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("area Float64");
    // Buffer di un punto con raggio 10 ~ cerchio di area pi*100 (con
    // l'approssimazione poligonale di `geo`, ~306).
    let expected = 100.0 * std::f64::consts::PI;
    assert!(
        (areas.value(0) - expected).abs() < 15.0,
        "area del buffer: {} vs ~{expected}",
        areas.value(0)
    );
    assert!(areas.is_null(1), "null in -> null out");
}
