use std::fs::File;
use std::process::Command;
use std::sync::Arc;

use plenora_core::arrow::array::{Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc::reader::FileReader;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::catalog::{Family, CATALOG};
use serde_json::json;

const fn executable() -> &'static str {
    env!("CARGO_BIN_EXE_plenora-data-tools")
}

fn write_input(path: &std::path::Path) {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(vec![Some("Été"), None]))],
    )
    .expect("fixture");
    let mut file = File::create(path).expect("create input");
    let mut writer = FileWriter::try_new(&mut file, &batch.schema()).expect("writer");
    writer.write(&batch).expect("write batch");
    writer.finish().expect("finish input");
}

fn write_batches(path: &std::path::Path, batches: &[RecordBatch], schema: &Schema) {
    let mut file = File::create(path).expect("create input");
    let mut writer = FileWriter::try_new(&mut file, schema).expect("writer");
    for batch in batches {
        writer.write(batch).expect("write batch");
    }
    writer.finish().expect("finish input");
}

#[test]
fn cli_round_trip_is_atomic_and_refuses_overwrite() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");
    write_input(&input);
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "steps": [
                {"operation": "text_normalize", "config": {
                    "columns": ["value"], "operations": "full", "overwrite": true
                }},
                {"operation": "string_length", "config": {
                    "column": "value", "output_column": "length"
                }}
            ]
        }))
        .expect("json"),
    )
    .expect("write plan");

    let status = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("run CLI");
    assert!(status.success());

    let mut reader = FileReader::try_new(File::open(&output).expect("output"), None)
        .expect("valid Arrow output");
    let result = reader.next().expect("one batch").expect("batch");
    let values = result
        .column_by_name("value")
        .expect("value")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("utf8");
    assert_eq!(values.value(0), "ete");
    assert!(values.is_null(1));

    let second = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("second CLI run");
    assert!(!second.success());
}

#[test]
fn invalid_plan_is_rejected_before_input_is_opened() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("invalid.json");
    let output = directory.path().join("must-not-exist.arrow");
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "steps": [{"operation": "string_pad", "config": {
                "column": "value", "width": 3, "side": "left",
                "fill_char": "xx", "output_column": null
            }}]
        }))
        .expect("json"),
    )
    .expect("write plan");

    let result = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(directory.path().join("missing.arrow"))
        .arg("--output")
        .arg(&output)
        .output()
        .expect("run CLI");
    assert!(!result.status.success());
    assert!(!output.exists());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("fill_char"));
    assert!(!stderr.contains("missing.arrow"));
}

#[test]
fn informational_commands_and_argument_errors_are_stable() {
    // Il catalogo delle 66 operazioni tabellari si ottiene ora filtrando
    // il catalogo unificato per `Family::Table` (CLI: `catalog --family table`).
    let catalog = Command::new(executable())
        .args(["catalog", "--family", "table"])
        .output()
        .expect("catalog");
    assert!(catalog.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&catalog.stdout).expect("catalog JSON");
    let table_operations = CATALOG
        .iter()
        .filter(|operation| operation.family == Family::Table)
        .count();
    assert_eq!(
        parsed.as_array().expect("array").len(),
        table_operations
    );

    for argument in ["--help", "-h"] {
        let help = Command::new(executable())
            .arg(argument)
            .output()
            .expect("help");
        assert!(help.status.success(), "{argument} deve terminare con successo");
        assert!(help.stderr.is_empty(), "{argument} non deve emettere errori");
        assert!(String::from_utf8_lossy(&help.stdout).contains("plenora-data-tools catalog"));
    }

    for argument in ["self-test", "--version", "-V"] {
        assert!(Command::new(executable())
            .arg(argument)
            .status()
            .expect("informational command")
            .success());
    }
    assert!(!Command::new(executable())
        .status()
        .expect("usage")
        .success());
    assert!(!Command::new(executable())
        .args(["run", "--plan"])
        .status()
        .expect("missing value")
        .success());
    assert!(!Command::new(executable())
        .args(["run", "--input", "x", "--output", "y"])
        .status()
        .expect("missing flag")
        .success());
}

#[test]
fn empty_ipc_file_is_transformed_and_published() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("empty.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");
    let schema = Schema::new(vec![Field::new("value", DataType::Utf8, true)]);
    write_batches(&input, &[], &schema);
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"rename","config":{"renames":[{"old_name":"value","new_name":"renamed"}]}}]}"#,
    )
    .expect("plan");
    let status = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("CLI");
    assert!(status.success());
    let reader = FileReader::try_new(File::open(output).expect("output"), None).expect("IPC");
    assert_eq!(reader.schema().field(0).name(), "renamed");
}

#[test]
fn empty_ipc_file_is_safe_for_a_blocking_plan() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("empty.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");
    let schema = Schema::new(vec![Field::new("id", DataType::Int64, true)]);
    write_batches(&input, &[], &schema);
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"sort","config":{"columns":["id"],"ascending":true}}]}"#,
    )
    .expect("plan");

    let status = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("CLI");
    assert!(status.success());

    let reader = FileReader::try_new(File::open(output).expect("output"), None).expect("IPC");
    assert_eq!(reader.schema().as_ref(), &schema);
}

#[test]
fn total_row_limit_across_batches_leaves_no_output() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("two.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");
    let schema = Arc::new(Schema::new(vec![Field::new("value", DataType::Utf8, true)]));
    let first = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(StringArray::from(vec![Some("a")]))],
    )
    .expect("first");
    let second = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(StringArray::from(vec![Some("b")]))],
    )
    .expect("second");
    write_batches(&input, &[first, second], schema.as_ref());
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"limits":{"max_rows":1},"steps":[{"operation":"drop_columns","config":{"columns":[]}}]}"#,
    )
    .expect("plan");
    let status = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("CLI");
    assert!(!status.success());
    assert!(!output.exists());
}

#[test]
fn corrupt_ipc_and_missing_output_directory_fail_without_publication() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("corrupt.arrow");
    let plan = directory.path().join("plan.json");
    std::fs::write(&input, b"not arrow").expect("corrupt input");
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"drop_columns","config":{"columns":[]}}]}"#,
    )
    .expect("plan");
    let output = directory.path().join("missing").join("output.arrow");
    let status = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("CLI");
    assert!(!status.success());
    assert!(!output.exists());
}

#[test]
fn blocking_plan_combines_batches_before_sorting() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let batches = [
        RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![3, 1]))],
        )
        .expect("first"),
        RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![4, 2]))],
        )
        .expect("second"),
    ];
    write_batches(&input, &batches, schema.as_ref());
    std::fs::write(&plan, br#"{"schema_version":1,"steps":[{"operation":"sort","config":{"columns":["id"],"ascending":true}}]}"#).expect("plan");
    assert!(Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("CLI")
        .success());
    let mut reader = FileReader::try_new(File::open(output).expect("output"), None).expect("IPC");
    let result = reader.next().expect("batch").expect("valid");
    assert_eq!(
        result
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64")
            .values(),
        &[1, 2, 3, 4]
    );
}

#[test]
fn binary_plan_requires_right_and_publishes_join_atomically() {
    let directory = tempfile::tempdir().expect("tempdir");
    let left = directory.path().join("left.arrow");
    let right = directory.path().join("right.arrow");
    let missing_output = directory.path().join("missing.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let left_batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(Int64Array::from(vec![1, 2]))],
    )
    .expect("left");
    let right_batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(Int64Array::from(vec![2, 3]))],
    )
    .expect("right");
    write_batches(&left, &[left_batch], schema.as_ref());
    write_batches(&right, &[right_batch], schema.as_ref());
    std::fs::write(&plan, br#"{"schema_version":1,"steps":[{"operation":"join","config":{"left_keys":["id"],"right_keys":["id"],"how":"outer"}}]}"#).expect("plan");
    let missing = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&left)
        .arg("--output")
        .arg(&missing_output)
        .output()
        .expect("missing right");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--right"));
    assert!(!missing_output.exists());
    assert!(Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&left)
        .arg("--right")
        .arg(&right)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("CLI")
        .success());
    let mut reader = FileReader::try_new(File::open(output).expect("output"), None).expect("IPC");
    assert_eq!(reader.next().expect("batch").expect("valid").num_rows(), 3);
}

#[test]
fn streaming_multiple_batches_reuses_writer_and_preserves_schema() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");
    let schema = Arc::new(Schema::new(vec![Field::new(
        "value",
        DataType::Utf8,
        false,
    )]));
    let batches = [
        RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(StringArray::from(vec!["A"]))],
        )
        .expect("first"),
        RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(StringArray::from(vec!["B"]))],
        )
        .expect("second"),
    ];
    write_batches(&input, &batches, schema.as_ref());
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"string_length","config":{"column":"value","output_column":"length"}}]}"#,
    )
    .expect("plan");
    assert!(Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .expect("CLI")
        .success());
    let reader = FileReader::try_new(File::open(output).expect("output"), None).expect("IPC");
    let rows = reader
        .map(|batch| batch.expect("batch").num_rows())
        .sum::<usize>();
    assert_eq!(rows, 2);

    let missing_right_value = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(directory.path().join("unused.arrow"))
        .arg("--right")
        .status()
        .expect("missing right value");
    assert!(!missing_right_value.success());
}

#[test]
fn valid_blocking_plan_reports_missing_input_without_publication() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    let output = directory.path().join("output.arrow");
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"sort","config":{"columns":["id"]}}]}"#,
    )
    .expect("plan");
    let status = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(directory.path().join("missing.arrow"))
        .arg("--output")
        .arg(&output)
        .status()
        .expect("CLI");
    assert!(!status.success());
    assert!(!output.exists());
}

#[test]
fn capabilities_emette_il_documento_dichiarativo_icd10() {
    // ICD §10 R10.2: capability dichiarative interrogabili prima
    // dell'esecuzione, in forma leggibile da un programma.
    let output = Command::new(executable())
        .arg("capabilities")
        .output()
        .expect("capabilities");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(document["protocol_version"], 1);
    assert_eq!(document["arrow_version"], "59.1.0");
    // Modello geometrico: tutte e cinque le dimensioni propagate (R3.3),
    // elaborazione solo XY (R3.3.1), encoding chiusi (R3.5).
    assert_eq!(
        document["geometry"]["dimensions_propagated"],
        json!(["xy", "xyz", "xym", "xyzm", "unknown"])
    );
    assert_eq!(document["geometry"]["dimensions_elaborated"], json!(["xy"]));
    assert_eq!(document["geometry"]["encodings"], json!(["wkb", "ewkb"]));
    // Una capability per ogni op del catalogo (fonte unica).
    let operations = document["operations"].as_array().expect("operations");
    assert_eq!(operations.len(), CATALOG.len());
    let reproject = operations
        .iter()
        .find(|op| op["id"] == "geo.reproject")
        .expect("geo.reproject presente");
    assert_eq!(reproject["required_capabilities"], json!(["proj"]));
    assert_eq!(reproject["cancellation_behavior"], "non_interruptible");
}
