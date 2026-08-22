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

#[cfg(feature = "proj-backend")]
fn assert_public_io_admission_metadata(schema: &Schema) {
    let geometry = schema
        .field_with_name("geometry")
        .expect("campo geometria pubblico");
    let metadata = geometry.metadata();
    for (key, expected) in [
        ("plenora.geometry.encoding", "wkb"),
        ("plenora.geometry.dimensions", "xy"),
        ("plenora.geometry.crs_resolution", "resolved"),
        ("plenora.geometry.types_declaration", "exact"),
        ("plenora.geometry.types", "point"),
    ] {
        assert_eq!(
            metadata.get(key).map(String::as_str),
            Some(expected),
            "metadato pubblico {key}"
        );
    }
    assert_eq!(
        schema
            .metadata()
            .get("plenora.contract.version")
            .map(String::as_str),
        Some("1")
    );
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
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );

    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    assert_eq!(reader.schema().field(0).name(), "renamed");
    let rows: usize = reader.map(|batch| batch.expect("batch").num_rows()).sum();
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
    assert!(String::from_utf8_lossy(&again.stdout).contains("esistente"));
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
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
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
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
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
    let schema = Schema::new(vec![Field::new(
        DEFAULT_GEOMETRY_COLUMN,
        DataType::Binary,
        true,
    )
    .with_metadata(metadata)]);
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
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("\"rows\":1"), "stdout: {stdout}");
    assert!(stdout.contains("\"output_rows\":1"), "stdout: {stdout}");
    assert!(output_path.exists());

    let ipc_output = directory.path().join("output.arrow");
    let result = cli()
        .arg("transform-arrow")
        .arg("--input")
        .arg(&input)
        .arg("--schema")
        .arg(&schema_path)
        .arg("--output")
        .arg(&ipc_output)
        .args(["--output-format", "ipc-file"])
        .output()
        .expect("transform-arrow ipc-file");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    let reader = FileReader::try_new(std::fs::File::open(&ipc_output).expect("ipc output"), None)
        .expect("Arrow IPC file pubblico");
    assert_public_io_admission_metadata(reader.schema().as_ref());
    let batches = reader.collect::<Result<Vec<_>, _>>().expect("batch IPC");
    assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
}

/// `transform-arrow` con `from_coords`: le coordinate non finite sono un
/// difetto row-scoped — l'envelope d'errore porta la diagnostica completa
/// (conteggi, indice assoluto, colonna) e nessun output e' pubblicato.
#[cfg(feature = "proj-backend")]
#[test]
fn transform_arrow_from_coords_reports_row_diagnostics() {
    use plenora_core::arrow::array::Float64Array;
    use plenora_engine::geo_transport::transport::{encode_ipc, EnvelopeWriter};

    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.plngeo3");
    let schema_path = directory.path().join("schema.json");
    let output_path = directory.path().join("output.plngeo3");

    let schema = Schema::new(vec![
        Field::new("x", DataType::Float64, true),
        Field::new("y", DataType::Float64, true),
    ]);
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![
            Arc::new(Float64Array::from(vec![Some(1.0), Some(f64::NAN)])),
            Arc::new(Float64Array::from(vec![Some(2.0), Some(4.0)])),
        ],
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
        br#"{"schema_version":3,"operation":"from_coords","row_count":2,"crs":"EPSG:3857"}"#,
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
    assert!(!result.status.success(), "coordinata NaN accettata");
    // L'envelope vive su stdout e stderr resta vuoto
    // (errori-e-limiti.md#envelope-e-canali).
    assert!(
        result.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("envelope row diagnostics");
    let diagnostics = &envelope["error"]["row_diagnostics"];
    assert_eq!(
        diagnostics["contract"], "plenora-row-diagnostics-v1",
        "envelope: {envelope}"
    );
    assert_eq!(diagnostics["counts"]["geometry.non_finite_coordinate"], 1);
    assert_eq!(diagnostics["examples"][0]["source_index"], 1);
    assert_eq!(diagnostics["examples"][0]["column"], "x");
    assert!(
        !output_path.exists(),
        "publish atomico: nessun output parziale"
    );
}

/// Un rifiuto row-scoped di `transform-arrow` e' un difetto del DATO
/// letto, non del piano: l'envelope porta gli assi `data_mapping`/`read`,
/// `remote_effect` none, retry never (mai riclassificato `invalid_plan`) e la
/// diagnostica, con zero output pubblicato. Un errore NON row-scoped
/// mantiene la classificazione storica.
#[cfg(feature = "proj-backend")]
#[test]
fn transform_arrow_row_diagnostics_error_axes_are_data_mapping() {
    use plenora_core::arrow::array::Float64Array;
    use plenora_engine::geo_transport::transport::{encode_ipc, EnvelopeWriter};

    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.plngeo3");
    let schema_path = directory.path().join("schema.json");
    let output_path = directory.path().join("output.plngeo3");

    let schema = Schema::new(vec![
        Field::new("x", DataType::Float64, true),
        Field::new("y", DataType::Float64, true),
    ]);
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![
            Arc::new(Float64Array::from(vec![Some(1.0), Some(f64::NAN)])),
            Arc::new(Float64Array::from(vec![Some(2.0), Some(4.0)])),
        ],
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
        br#"{"schema_version":3,"operation":"from_coords","row_count":2,"crs":"EPSG:3857"}"#,
    )
    .expect("schema");

    let run_transform = || {
        cli()
            .arg("transform-arrow")
            .arg("--input")
            .arg(&input)
            .arg("--schema")
            .arg(&schema_path)
            .arg("--output")
            .arg(&output_path)
            .output()
            .expect("transform-arrow")
    };

    let result = run_transform();
    assert!(!result.status.success(), "coordinata NaN accettata");
    assert!(
        result.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("envelope row diagnostics");
    let error = &envelope["error"];
    assert_eq!(error["category"], "data_mapping", "envelope: {envelope}");
    assert_eq!(error["phase"], "read", "envelope: {envelope}");
    assert_eq!(error["remote_effect"], "none", "envelope: {envelope}");
    assert_eq!(error["retry"]["kind"], "never", "envelope: {envelope}");
    assert_eq!(
        error["row_diagnostics"]["counts"]["geometry.non_finite_coordinate"], 1,
        "envelope: {envelope}"
    );
    assert!(
        !output_path.exists(),
        "publish atomico: nessun output parziale"
    );

    // Controllo: errore NON row-scoped (envelope malformato) -> la
    // classificazione storica resta invariata, nessuna diagnostica.
    std::fs::write(&input, b"non-un-envelope-plngeo3").expect("input malformato");
    let result = run_transform();
    assert!(!result.status.success(), "envelope malformato accettato");
    assert!(result.stderr.is_empty());
    let envelope: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("envelope errore non row-scoped");
    let error = &envelope["error"];
    assert_eq!(error["category"], "invalid_plan", "envelope: {envelope}");
    assert_eq!(error["phase"], "validate", "envelope: {envelope}");
    assert!(error["row_diagnostics"].is_null(), "envelope: {envelope}");
    assert!(!output_path.exists());
}

/// BLOCK-06: l'output di `transform-arrow` porta le chiavi canoniche §2 in
/// doppia emissione con quelle `GeoArrow` (parita' col percorso v4, errori-e-limiti.md#limiti-dichiarati
/// estesa), con `plenora.contract.version` nei metadati dello schema (R2.5).
#[cfg(feature = "proj-backend")]
#[test]
fn transform_arrow_v3_emits_canonical_keys() {
    use plenora_engine::geo_transport::transport::{
        decode_ipc, encode_ipc, EnvelopeReader, EnvelopeWriter, DEFAULT_GEOMETRY_COLUMN,
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
    let schema = Schema::new(vec![Field::new(
        DEFAULT_GEOMETRY_COLUMN,
        DataType::Binary,
        true,
    )
    .with_metadata(metadata)]);
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
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
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
        metadata
            .get("plenora.geometry.dimensions")
            .map(String::as_str),
        Some("xy")
    );
    assert_eq!(
        metadata
            .get("plenora.geometry.crs_resolution")
            .map(String::as_str),
        Some("resolved")
    );
    assert_eq!(
        metadata.get("plenora.geometry.crs_id").map(String::as_str),
        Some("EPSG:3857")
    );
    assert_eq!(
        metadata
            .get("plenora.geometry.axis_order")
            .map(String::as_str),
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
fn assert_pair_transport_metadata(schema: &Schema) {
    use plenora_engine::geo_transport::transport::DEFAULT_GEOMETRY_COLUMN;

    let (_, field) = schema
        .column_with_name(DEFAULT_GEOMETRY_COLUMN)
        .expect("geometry column");
    let metadata = field.metadata();
    assert_eq!(
        metadata
            .get("plenora.geometry.dimensions")
            .map(String::as_str),
        Some("xy")
    );
    assert_eq!(
        metadata
            .get("plenora.geometry.crs_resolution")
            .map(String::as_str),
        Some("resolved")
    );
    assert_eq!(
        metadata.get("plenora.geometry.crs_id").map(String::as_str),
        Some("EPSG:3857")
    );
    assert_eq!(
        schema
            .metadata()
            .get("plenora.contract.version")
            .map(String::as_str),
        Some("1")
    );
}

#[cfg(feature = "proj-backend")]
#[test]
fn pair_arrow_v3_emits_canonical_keys() {
    use plenora_engine::geo_transport::transport::{
        decode_ipc, encode_ipc, EnvelopeReader, EnvelopeWriter, DEFAULT_GEOMETRY_COLUMN,
        GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
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
    let schema = Schema::new(vec![Field::new(
        DEFAULT_GEOMETRY_COLUMN,
        DataType::Binary,
        true,
    )
    .with_metadata(metadata)]);
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
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );

    let envelope = std::fs::read(&output_path).expect("output");
    let payload = EnvelopeReader::new(envelope.as_slice())
        .expect("envelope")
        .read_payload()
        .expect("payload");
    let (out_schema, _) = decode_ipc(&payload).expect("ipc");
    assert_pair_transport_metadata(out_schema.as_ref());

    let ipc_output = directory.path().join("pair-output.arrow");
    let result = cli()
        .arg("pair-arrow")
        .arg("--left")
        .arg(&left_path)
        .arg("--right")
        .arg(&right_path)
        .arg("--schema")
        .arg(&schema_path)
        .arg("--output")
        .arg(&ipc_output)
        .args(["--output-format", "ipc-file"])
        .output()
        .expect("pair-arrow ipc-file");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    let reader = FileReader::try_new(std::fs::File::open(&ipc_output).expect("ipc output"), None)
        .expect("Arrow IPC file pubblico");
    assert_public_io_admission_metadata(reader.schema().as_ref());
    let batches = reader.collect::<Result<Vec<_>, _>>().expect("batch IPC");
    assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
}

#[test]
fn arrow_output_format_rejects_unknown_values_before_publication() {
    for command in ["transform-arrow", "pair-arrow"] {
        let result = cli()
            .args([command, "--output-format", "stream"])
            .output()
            .expect("invocazione CLI");
        assert_eq!(result.status.code(), Some(2), "{command}");
        assert!(
            result.stderr.is_empty(),
            "{command}: stderr deve restare vuoto"
        );
        let error = String::from_utf8_lossy(&result.stdout);
        assert!(
            error.contains("--output-format non valido"),
            "{command}: {error}"
        );
    }
}
