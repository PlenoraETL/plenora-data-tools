//! Smoke test di Fase 1 per la CLI unificata: round-trip manuali dei comandi
//! `run` (pipeline tabellare) e `transform`/`transform-arrow` (trasporti geo
//! v2/v3), piu' i comandi informativi `catalog` e `validate`.
//!
//! I test geo richiedono la risoluzione CRS PROJ: sono compilati solo con la
//! feature `proj-backend` (es. `cargo test -p plenora-cli --features
//! full-backends`). I test end-to-end avversari arrivano con la fase 1e.

#[cfg(feature = "proj-backend")]
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;

#[cfg(feature = "proj-backend")]
use plenora_core::arrow::array::BinaryArray;
use plenora_core::arrow::array::{RecordBatch, StringArray};
use plenora_core::arrow::ipc::reader::FileReader;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_plenora-data-tools"))
}

fn write_ipc(path: &std::path::Path, schema: &Schema, batches: &[RecordBatch]) {
    let file = std::fs::File::create(path).expect("create input");
    let mut writer = FileWriter::try_new(file, &Arc::new(schema.clone())).expect("writer");
    for batch in batches {
        writer.write(batch).expect("write batch");
    }
    writer.finish().expect("finish");
}

#[test]
fn catalog_unificato_e_filtro_famiglia() {
    let output = cli().arg("catalog").output().expect("catalog");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(parsed.as_array().expect("array").len(), 146);

    let table = cli()
        .args(["catalog", "--family", "table"])
        .output()
        .expect("catalog table");
    assert!(table.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&table.stdout).expect("JSON");
    assert_eq!(parsed.as_array().expect("array").len(), 71);

    let geo = cli()
        .args(["catalog", "--family", "geo"])
        .output()
        .expect("catalog geo");
    assert!(geo.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&geo.stdout).expect("JSON");
    assert_eq!(parsed.as_array().expect("array").len(), 75);

    let invalid = cli()
        .args(["catalog", "--family", "bogus"])
        .output()
        .expect("catalog bogus");
    assert!(!invalid.status.success());
}

#[test]
fn run_roundtrip_rename_streaming_e_fail_closed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    let output_path = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");

    let schema = Schema::new(vec![Field::new("value", DataType::Utf8, true)]);
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![Arc::new(StringArray::from(vec![Some("a"), Some("b")]))],
    )
    .expect("batch");
    write_ipc(&input, &schema, &[batch]);
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"rename","config":{"renames":[{"old_name":"value","new_name":"renamed"}]}}]}"#,
    )
    .expect("plan");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
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

    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    assert_eq!(reader.schema().field(0).name(), "renamed");
    let rows: usize = reader
        .map(|batch| batch.expect("batch").num_rows())
        .sum();
    assert_eq!(rows, 2);

    // Fail-closed: un secondo run sullo stesso output non sovrascrive.
    let again = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run again");
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("esistente"));
}

#[test]
fn validate_stampa_il_riepilogo_del_piano() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"rename","config":{"renames":[{"old_name":"value","new_name":"renamed"}]}}]}"#,
    )
    .expect("plan");

    let result = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .args(["--inputs", "a.arrow", "b.arrow"])
        .output()
        .expect("validate");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["steps"], 1);
}

/// POINT (2 3), little-endian OGC WKB (come `write_self_test` del sorgente).
#[cfg(feature = "proj-backend")]
const POINT_WKB: [u8; 21] = [
    1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 8, 64,
];

#[cfg(feature = "proj-backend")]
#[test]
fn transform_wkb_v2_roundtrip() {
    use plenora_engine::geo_transport::protocol::FrameWriter;

    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.bin");
    let schema = directory.path().join("schema.json");
    let output_path = directory.path().join("output.bin");

    let mut writer = FrameWriter::new(Vec::new(), 1).expect("writer");
    writer.write_frame(Some(&POINT_WKB)).expect("frame");
    let framed = writer.finish().expect("finish").0;
    std::fs::write(&input, framed).expect("input");
    std::fs::write(
        &schema,
        br#"{"schema_version":2,"operation":"centroid","row_count":1,"crs":"EPSG:3857"}"#,
    )
    .expect("schema");

    let result = cli()
        .arg("transform")
        .arg("--input")
        .arg(&input)
        .arg("--schema")
        .arg(&schema)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("transform");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("\"rows\":1"), "stdout: {stdout}");
    assert!(output_path.exists());
}

#[cfg(feature = "proj-backend")]
#[test]
fn transform_arrow_v3_roundtrip() {
    use plenora_engine::geo_transport::transport::{
        encode_ipc, EnvelopeWriter, DEFAULT_GEOMETRY_COLUMN, GEOARROW_EXTENSION_KEY,
        GEOARROW_WKB_EXTENSION,
    };

    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.plngeo3");
    let schema_path = directory.path().join("schema.json");
    let output_path = directory.path().join("output.plngeo3");

    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    let schema = Schema::new(vec![
        Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(metadata),
    ]);
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![Arc::new(BinaryArray::from_iter([Some(&POINT_WKB[..])]))],
    )
    .expect("batch");
    let schema_ref = Arc::new(schema);
    let payload = encode_ipc(&schema_ref, std::slice::from_ref(&batch)).expect("encode");
    let mut writer = EnvelopeWriter::new(Vec::new(), payload.len() as u64).expect("writer");
    writer.write_payload(&payload).expect("payload");
    let envelope = writer.finish().expect("finish").0;
    std::fs::write(&input, envelope).expect("input");

    std::fs::write(
        &schema_path,
        br#"{"schema_version":3,"operation":"centroid","row_count":1,"crs":"EPSG:3857"}"#,
    )
    .expect("schema");

    let result = cli()
        .arg("transform-arrow")
        .arg("--input")
        .arg(&input)
        .arg("--schema")
        .arg(&schema_path)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("transform-arrow");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("\"rows\":1"), "stdout: {stdout}");
    assert!(stdout.contains("\"output_rows\":1"), "stdout: {stdout}");
    assert!(output_path.exists());
}

/// BLOCK-06: l'output di `transform-arrow` porta le chiavi canoniche §2 in
/// doppia emissione con quelle GeoArrow (parita' col percorso v4, DER-002
/// estesa), con `plenora.contract.version` nei metadati dello schema (R2.5).
#[cfg(feature = "proj-backend")]
#[test]
fn transform_arrow_v3_emits_canonical_keys() {
    use plenora_engine::geo_transport::transport::{
        EnvelopeReader, decode_ipc, encode_ipc, EnvelopeWriter, DEFAULT_GEOMETRY_COLUMN,
        GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
    };

    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.plngeo3");
    let schema_path = directory.path().join("schema.json");
    let output_path = directory.path().join("output.plngeo3");

    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    let schema = Schema::new(vec![
        Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(metadata),
    ]);
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![Arc::new(BinaryArray::from_iter([Some(&POINT_WKB[..])]))],
    )
    .expect("batch");
    let schema_ref = Arc::new(schema);
    let payload = encode_ipc(&schema_ref, std::slice::from_ref(&batch)).expect("encode");
    let mut writer = EnvelopeWriter::new(Vec::new(), payload.len() as u64).expect("writer");
    writer.write_payload(&payload).expect("payload");
    let envelope = writer.finish().expect("finish").0;
    std::fs::write(&input, envelope).expect("input");

    std::fs::write(
        &schema_path,
        br#"{"schema_version":3,"operation":"centroid","row_count":1,"crs":"EPSG:3857"}"#,
    )
    .expect("schema");

    let result = cli()
        .arg("transform-arrow")
        .arg("--input")
        .arg(&input)
        .arg("--schema")
        .arg(&schema_path)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("transform-arrow");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let envelope = std::fs::read(&output_path).expect("output");
    let payload = EnvelopeReader::new(envelope.as_slice())
        .expect("envelope")
        .read_payload()
        .expect("payload");
    let (out_schema, _) = decode_ipc(&payload).expect("ipc");
    let (_, field) = out_schema
        .column_with_name(DEFAULT_GEOMETRY_COLUMN)
        .expect("geometry column");
    let metadata = field.metadata();
    // Blocco canonico (stessa forma del v4).
    assert_eq!(
        metadata.get("plenora.geometry.dimensions").map(String::as_str),
        Some("xy")
    );
    assert_eq!(
        metadata.get("plenora.geometry.crs_resolution").map(String::as_str),
        Some("resolved")
    );
    assert_eq!(
        metadata.get("plenora.geometry.crs_id").map(String::as_str),
        Some("EPSG:3857")
    );
    assert_eq!(
        metadata.get("plenora.geometry.axis_order").map(String::as_str),
        Some("unknown")
    );
    // Doppia emissione: le chiavi GeoArrow restano.
    assert_eq!(
        metadata.get(GEOARROW_EXTENSION_KEY).map(String::as_str),
        Some(GEOARROW_WKB_EXTENSION)
    );
    // Versione di protocollo nei metadati dello schema (R2.5).
    assert_eq!(
        out_schema
            .metadata()
            .get("plenora.contract.version")
            .map(String::as_str),
        Some("1")
    );
}

/// BLOCK-06: anche `pair-arrow` emette le chiavi canoniche; su un'operazione
/// pass-through (`within`) il blocco e' derivato dal metadato `geo` del
/// campo propagato (stesso CRS, stessa dimensionalita').
#[cfg(feature = "proj-backend")]
#[test]
fn pair_arrow_v3_emits_canonical_keys() {
    use plenora_engine::geo_transport::transport::{
        EnvelopeReader, decode_ipc, encode_ipc, EnvelopeWriter, DEFAULT_GEOMETRY_COLUMN,
        GEO_METADATA_KEY, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
    };

    let directory = tempfile::tempdir().expect("tempdir");
    let left_path = directory.path().join("left.plngeo3");
    let right_path = directory.path().join("right.plngeo3");
    let schema_path = directory.path().join("schema.json");
    let output_path = directory.path().join("output.plngeo3");

    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    metadata.insert(
        GEO_METADATA_KEY.to_owned(),
        r#"{"crs":"EPSG:3857","dimensions":"xy"}"#.to_owned(),
    );
    let schema = Schema::new(vec![
        Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(metadata),
    ]);
    let schema_ref = Arc::new(schema);
    let batch = RecordBatch::try_new(
        schema_ref.clone(),
        vec![Arc::new(BinaryArray::from_iter([Some(&POINT_WKB[..])]))],
    )
    .expect("batch");
    let payload = encode_ipc(&schema_ref, std::slice::from_ref(&batch)).expect("encode");
    let mut writer = EnvelopeWriter::new(Vec::new(), payload.len() as u64).expect("writer");
    writer.write_payload(&payload).expect("payload");
    let envelope = writer.finish().expect("finish").0;
    std::fs::write(&left_path, &envelope).expect("left");
    std::fs::write(&right_path, &envelope).expect("right");

    std::fs::write(
        &schema_path,
        br#"{"schema_version":3,"operation":"within","left_row_count":1,"right_row_count":1,"left_crs":"EPSG:3857","right_crs":"EPSG:3857","max_pairs":10}"#,
    )
    .expect("schema");

    let result = cli()
        .arg("pair-arrow")
        .arg("--left")
        .arg(&left_path)
        .arg("--right")
        .arg(&right_path)
        .arg("--schema")
        .arg(&schema_path)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("pair-arrow");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let envelope = std::fs::read(&output_path).expect("output");
    let payload = EnvelopeReader::new(envelope.as_slice())
        .expect("envelope")
        .read_payload()
        .expect("payload");
    let (out_schema, _) = decode_ipc(&payload).expect("ipc");
    let (_, field) = out_schema
        .column_with_name(DEFAULT_GEOMETRY_COLUMN)
        .expect("geometry column");
    let metadata = field.metadata();
    assert_eq!(
        metadata.get("plenora.geometry.dimensions").map(String::as_str),
        Some("xy")
    );
    assert_eq!(
        metadata.get("plenora.geometry.crs_resolution").map(String::as_str),
        Some("resolved")
    );
    assert_eq!(
        metadata.get("plenora.geometry.crs_id").map(String::as_str),
        Some("EPSG:3857")
    );
    assert_eq!(
        out_schema
            .metadata()
            .get("plenora.contract.version")
            .map(String::as_str),
        Some("1")
    );
}
