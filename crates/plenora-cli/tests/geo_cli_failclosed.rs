//! Comandi geo legacy e v4 della CLI sotto feature di default (senza backend
//! PROJ/GEOS): i pre-check transazionali, i controlli di versione/limiti e la
//! validazione CRS devono fallire chiusi — mai un output parziale pubblicato.
//! Complementare a `roundtrip_smoke.rs` e `xyzm_roundtrip_cli.rs`, che
//! coprono i round-trip con i backend compilati.

use std::process::Command;
use std::sync::Arc;

use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc::reader::FileReader;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_engine::geo_transport::protocol::{Frame, FrameReader};
use serde_json::json;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_plenora-data-tools"))
}

fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]))
}

fn write_ipc(path: &std::path::Path, schema: &SchemaRef, batches: &[RecordBatch]) {
    let file = std::fs::File::create(path).expect("create input");
    let mut writer = FileWriter::try_new(file, schema).expect("writer");
    for batch in batches {
        writer.write(batch).expect("write batch");
    }
    writer.finish().expect("finish");
}

/// L'envelope d'errore viaggia su STDOUT (stderr resta vuoto), come in
/// `plenora-database-tools`.
fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn self_test_writes_a_valid_control_frame_and_never_overwrites() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("result.bin");

    let result = cli()
        .args(["self-test", "--output"])
        .arg(&output)
        .output()
        .expect("self-test");
    assert!(result.status.success(), "stdout: {}", stderr_of(&result));
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"ok\""));

    // Il frame di controllo e' un WKB v2 leggibile: una riga, POINT (2 3)
    // (centroide del punto di controllo, come nel sorgente).
    let mut reader =
        FrameReader::new(std::fs::File::open(&output).expect("output"), 1).expect("frame reader");
    let frame = reader.next_frame().expect("frame").expect("una riga");
    let Frame::Wkb(payload) = frame else {
        panic!("atteso frame WKB");
    };
    let geometry = plenora_kernels_geo::geometry_from_wkb(&payload).expect("WKB valido");
    match geometry {
        geo::Geometry::Point(point) => {
            assert!((point.x() - 2.0).abs() < f64::EPSILON);
            assert!((point.y() - 3.0).abs() < f64::EPSILON);
        }
        other => panic!("atteso Point, ottenuto {other:?}"),
    }
    assert!(reader.next_frame().expect("fine").is_none());

    // Fail-closed: un secondo self-test sullo stesso percorso non
    // sovrascrive (create_new), il contenuto resta intatto.
    let before = std::fs::read(&output).expect("lettura");
    let second = cli()
        .args(["self-test", "--output"])
        .arg(&output)
        .output()
        .expect("secondo self-test");
    assert!(!second.status.success());
    assert_eq!(std::fs::read(&output).expect("lettura"), before);
}

#[test]
fn transform_rejects_stdout_output_and_unsupported_schema_version() {
    let directory = tempfile::tempdir().expect("tempdir");
    let schema = directory.path().join("schema.json");
    let output = directory.path().join("output.bin");

    // Output `-`: la pubblicazione deve essere transazionale (rifiutato
    // prima ancora di leggere lo schema).
    std::fs::write(
        &schema,
        br#"{"schema_version":2,"operation":"centroid","row_count":0}"#,
    )
    .expect("schema");
    let result = cli()
        .args(["transform", "--input", "input.bin", "--schema"])
        .arg(&schema)
        .args(["--output", "-"])
        .output()
        .expect("transform");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("stdout disabilitato"),
        "stdout: {}",
        stderr_of(&result)
    );

    // schema_version diversa da 2: rifiutata prima di toccare i dati.
    std::fs::write(
        &schema,
        br#"{"schema_version":3,"operation":"centroid","row_count":0}"#,
    )
    .expect("schema");
    let result = cli()
        .args(["transform", "--input", "input.bin", "--schema"])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("transform");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("schema_version"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists(), "nessun output parziale");
}

#[test]
fn transform_requires_a_crs_and_fails_closed_without_backend() {
    let directory = tempfile::tempdir().expect("tempdir");
    let schema = directory.path().join("schema.json");
    let output = directory.path().join("output.bin");

    // CRS assente: la validazione semantica lo richiede sempre.
    std::fs::write(
        &schema,
        br#"{"schema_version":2,"operation":"centroid","row_count":0}"#,
    )
    .expect("schema");
    let result = cli()
        .args(["transform", "--input", "input.bin", "--schema"])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("transform");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("crs"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists());

    // CRS dichiarato ma nessun backend PROJ compilato: la dichiarazione non
    // viene creduta — fail-closed, mai validazione ottimistica.
    std::fs::write(
        &schema,
        br#"{"schema_version":2,"operation":"centroid","row_count":0,"crs":"EPSG:32632"}"#,
    )
    .expect("schema");
    let result = cli()
        .args(["transform", "--input", "input.bin", "--schema"])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("transform");
    assert!(!result.status.success());
    assert!(!output.exists(), "nessun output parziale");
}

#[test]
fn transform_arrow_rejects_unsupported_version_and_missing_crs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let schema = directory.path().join("schema.json");
    let output = directory.path().join("output.plngeo3");

    // Output `-`: pubblicazione transazionale obbligatoria.
    std::fs::write(
        &schema,
        br#"{"schema_version":3,"operation":"centroid","row_count":0}"#,
    )
    .expect("schema");
    let result = cli()
        .args(["transform-arrow", "--input", "input.plngeo3", "--schema"])
        .arg(&schema)
        .args(["--output", "-"])
        .output()
        .expect("transform-arrow");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("stdout disabilitato"),
        "stdout: {}",
        stderr_of(&result)
    );

    // schema_version diversa da 3.
    std::fs::write(
        &schema,
        br#"{"schema_version":2,"operation":"centroid","row_count":0}"#,
    )
    .expect("schema");
    let result = cli()
        .args(["transform-arrow", "--input", "input.plngeo3", "--schema"])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("transform-arrow");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("schema_version"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists());

    // CRS assente.
    std::fs::write(
        &schema,
        br#"{"schema_version":3,"operation":"centroid","row_count":0}"#,
    )
    .expect("schema");
    let result = cli()
        .args(["transform-arrow", "--input", "input.plngeo3", "--schema"])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("transform-arrow");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("crs"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists());

    // CRS dichiarato senza backend: fail-closed.
    std::fs::write(
        &schema,
        br#"{"schema_version":3,"operation":"centroid","row_count":0,"crs":"EPSG:32632"}"#,
    )
    .expect("schema");
    let result = cli()
        .args(["transform-arrow", "--input", "input.plngeo3", "--schema"])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("transform-arrow");
    assert!(!result.status.success());
    assert!(!output.exists());
}

#[test]
// Batteria CLI fail-closed sequenziale: la lunghezza e' nel numero di casi.
#[allow(clippy::too_many_lines)]
fn pair_arrow_requires_file_paths_valid_version_and_crs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let schema = directory.path().join("schema.json");
    let output = directory.path().join("pairs.bin");

    // `-` non e' ammesso: due input da file, output transazionale.
    std::fs::write(
        &schema,
        br#"{"schema_version":3,"operation":"sjoin","left_row_count":0,"right_row_count":0,"predicate":"intersects","max_pairs":10}"#,
    )
    .expect("schema");
    let result = cli()
        .args([
            "pair-arrow",
            "--left",
            "-",
            "--right",
            "right.bin",
            "--schema",
        ])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("pair-arrow");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("percorsi file"),
        "stdout: {}",
        stderr_of(&result)
    );

    // schema_version diversa da 3.
    std::fs::write(
        &schema,
        br#"{"schema_version":2,"operation":"sjoin","left_row_count":0,"right_row_count":0,"predicate":"intersects","max_pairs":10}"#,
    )
    .expect("schema");
    let result = cli()
        .args([
            "pair-arrow",
            "--left",
            "left.bin",
            "--right",
            "right.bin",
            "--schema",
        ])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("pair-arrow");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("schema_version"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists());

    // Entrambi i CRS sono obbligatori.
    std::fs::write(
        &schema,
        br#"{"schema_version":3,"operation":"sjoin","left_row_count":0,"right_row_count":0,"predicate":"intersects","max_pairs":10}"#,
    )
    .expect("schema");
    let result = cli()
        .args([
            "pair-arrow",
            "--left",
            "left.bin",
            "--right",
            "right.bin",
            "--schema",
        ])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("pair-arrow");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("crs"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists());

    // CRS dichiarati senza backend: fail-closed.
    std::fs::write(
        &schema,
        br#"{"schema_version":3,"operation":"sjoin","left_row_count":0,"right_row_count":0,"predicate":"intersects","max_pairs":10,"left_crs":"EPSG:32632","right_crs":"EPSG:32632"}"#,
    )
    .expect("schema");
    let result = cli()
        .args([
            "pair-arrow",
            "--left",
            "left.bin",
            "--right",
            "right.bin",
            "--schema",
        ])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("pair-arrow");
    assert!(!result.status.success());
    assert!(!output.exists());
}

#[test]
// Batteria CLI fail-closed sequenziale: la lunghezza e' nel numero di casi.
#[allow(clippy::too_many_lines)]
fn spatial_join_enforces_version_max_pairs_and_crs_before_touching_data() {
    let directory = tempfile::tempdir().expect("tempdir");
    let schema = directory.path().join("schema.json");
    let output = directory.path().join("pairs.bin");

    // `-` non e' ammesso.
    std::fs::write(
        &schema,
        br#"{"schema_version":2,"predicate":"intersects","left_row_count":0,"right_row_count":0,"max_pairs":10}"#,
    )
    .expect("schema");
    let result = cli()
        .args([
            "spatial-join",
            "--left",
            "left.bin",
            "--right",
            "right.bin",
            "--schema",
        ])
        .arg(&schema)
        .args(["--output", "-"])
        .output()
        .expect("spatial-join");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("percorsi file"),
        "stdout: {}",
        stderr_of(&result)
    );

    // schema_version diversa da 2.
    std::fs::write(
        &schema,
        br#"{"schema_version":3,"predicate":"intersects","left_row_count":0,"right_row_count":0,"max_pairs":10}"#,
    )
    .expect("schema");
    let result = cli()
        .args([
            "spatial-join",
            "--left",
            "left.bin",
            "--right",
            "right.bin",
            "--schema",
        ])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("spatial-join");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("schema_version"),
        "stdout: {}",
        stderr_of(&result)
    );

    // max_pairs fuori dominio (0 e oltre il limite del protocollo).
    for max_pairs in [0_u64, 10_000_001] {
        let document = json!({
            "schema_version": 2,
            "predicate": "intersects",
            "left_row_count": 0,
            "right_row_count": 0,
            "max_pairs": max_pairs
        });
        std::fs::write(&schema, serde_json::to_vec(&document).expect("json")).expect("schema");
        let result = cli()
            .args([
                "spatial-join",
                "--left",
                "left.bin",
                "--right",
                "right.bin",
                "--schema",
            ])
            .arg(&schema)
            .arg("--output")
            .arg(&output)
            .output()
            .expect("spatial-join");
        assert!(!result.status.success(), "max_pairs={max_pairs}");
        assert!(
            stderr_of(&result).contains("max_pairs"),
            "max_pairs={max_pairs}, stdout: {}",
            stderr_of(&result)
        );
        assert!(!output.exists());
    }

    // CRS obbligatori su entrambi i lati.
    std::fs::write(
        &schema,
        br#"{"schema_version":2,"predicate":"intersects","left_row_count":0,"right_row_count":0,"max_pairs":10}"#,
    )
    .expect("schema");
    let result = cli()
        .args([
            "spatial-join",
            "--left",
            "left.bin",
            "--right",
            "right.bin",
            "--schema",
        ])
        .arg(&schema)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("spatial-join");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("crs"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists());
}

#[test]
fn run_v4_rejects_the_right_flag_and_accepts_the_single_input_flag() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    let input = directory.path().join("input.arrow");
    let output = directory.path().join("output.arrow");

    // `--right` non ha senso per i piani v4 (accoppiamento posizionale via
    // `--inputs`): rifiutato prima di toccare qualunque file.
    std::fs::write(&plan, br#"{"schema_version":5}"#).expect("plan");
    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .args(["--right", "r.arrow", "--output"])
        .arg(&output)
        .output()
        .expect("run");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("--right"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists());

    // `--input` singolo: equivalente a `--inputs` per un piano a un input.
    let document = json!({
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "r", "op": "table.rename", "in": ["f"],
             "config": {"renames": [{"old_name": "name", "new_name": "label"}]}},
        ],
        "output": "r",
    });
    std::fs::write(&plan, serde_json::to_vec(&document).expect("json")).expect("plan");
    let batch = RecordBatch::try_new(
        table_schema(),
        vec![
            Arc::new(Int64Array::from(vec![0, 1, 2])),
            Arc::new(StringArray::from(vec![Some("a"), Some("b"), Some("c")])),
        ],
    )
    .expect("batch");
    write_ipc(&input, &table_schema(), &[batch]);
    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("run");
    assert!(result.status.success(), "stdout: {}", stderr_of(&result));
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("\"output_rows\": 2"), "stdout: {stdout}");
    let mut reader =
        FileReader::try_new(std::fs::File::open(&output).expect("output"), None).expect("IPC");
    let result_batch = reader.next().expect("batch").expect("valid");
    assert_eq!(result_batch.num_rows(), 2);
    assert!(result_batch.schema().field_with_name("label").is_ok());
}

#[test]
fn blocking_plan_over_max_rows_fails_before_any_publication() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");

    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(Int64Array::from(vec![2, 1]))],
    )
    .expect("batch");
    write_ipc(&input, &schema, &[batch]);
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"limits":{"max_rows":1},"steps":[{"operation":"sort","config":{"columns":["id"],"ascending":true}}]}"#,
    )
    .expect("plan");

    // Il piano blocking materializza l'input: il limite righe scatta in
    // lettura, prima di qualunque scrittura.
    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("run");
    assert!(!result.status.success());
    assert!(
        stderr_of(&result).contains("oltre"),
        "stdout: {}",
        stderr_of(&result)
    );
    assert!(!output.exists(), "nessun output parziale");
}
