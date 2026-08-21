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
    let stderr = String::from_utf8_lossy(&result.stdout);
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
    assert_eq!(parsed.as_array().expect("array").len(), table_operations);

    for argument in ["--help", "-h"] {
        let help = Command::new(executable())
            .arg(argument)
            .output()
            .expect("help");
        assert!(
            help.status.success(),
            "{argument} deve terminare con successo"
        );
        assert!(
            help.stderr.is_empty(),
            "{argument} non deve emettere errori"
        );
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
    assert!(String::from_utf8_lossy(&missing.stdout).contains("--right"));
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
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
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

// Un piano legacy con nodo blocking prima di un'op row-diagnostics
// pubblicherebbe indici post-sort dichiarati `source_row_zero_based`.
// Fail-closed: la CLI rifiuta e richiede un piano DAG, nessun output pubblicato.
#[test]
fn legacy_blocking_plan_with_row_diagnostics_step_requires_dag_v4() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    let output = directory.path().join("output.arrow");
    let plan = directory.path().join("plan.json");
    let schema = Schema::new(vec![Field::new("value", DataType::Utf8, true)]);
    // Ordine tale che il sort riordini davvero: la riga invalida e' la PRIMA
    // sorgente (indice 0) ma la SECONDA post-sort — un indice pubblicato
    // `source_row_zero_based` sarebbe inventato (1 invece di 0).
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![Arc::new(StringArray::from(vec![
            Some("non-una-data"),
            Some("2024-01-01"),
        ]))],
    )
    .expect("fixture");
    write_batches(&input, &[batch], &schema);
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "steps": [
                {"operation": "sort", "config": {"columns": ["value"]}},
                {"operation": "type_cast", "config": {
                    "column": "value", "target_type": "date32", "errors": "coerce"
                }}
            ]
        }))
        .expect("json"),
    )
    .expect("write plan");
    let result = Command::new(executable())
        .arg("run")
        .arg("--plan")
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("run legacy blocking");
    assert!(
        !result.status.success(),
        "piano legacy blocking+diagnostico accettato: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(
        !output.exists(),
        "nessun output pubblicabile da un piano rifiutato"
    );
    // Il rifiuto deve venire dal GATE (provenance non attestabile), non
    // dall'esecuzione: nessun report row-scoped con indici post-sort.
    // Golden di canale: l'envelope di errore vive su STDOUT e stderr
    // resta vuoto (allineamento a `plenora-database-tools`).
    assert!(
        result.stderr.is_empty(),
        "stderr deve restare vuoto: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope_text = String::from_utf8_lossy(&result.stdout).into_owned();
    assert!(
        envelope_text.contains("piano DAG"),
        "atteso rifiuto fail-closed verso il DAG, trovato: {envelope_text}"
    );
    assert!(
        !envelope_text.contains("row_diagnostics"),
        "indici post-sort pubblicati come source_row: {envelope_text}"
    );
}

// formula ed expression erano omesse dal gate
// legacy pur essendo row-diagnostics per planner/executor: un piano
// sort -> formula/expression bypassava il gate e `execute_complete_batch`
// pubblicava indici post-sort come `source_row_zero_based` Complete.
// Il rifiuto deve avvenire al gate, PRIMA dell'esecuzione: fixture con
// divisore zero — se l'esecuzione partisse, stderr porterebbe un report
// row_diagnostics con indici post-sort inventati.
#[test]
fn legacy_blocking_plan_with_formula_or_expression_requires_dag_v4() {
    let schema = Schema::new(vec![Field::new("id", DataType::Int64, true)]);
    // Il divisore zero e' la riga sorgente 1 ma la riga 0 post-sort: un
    // indice pubblicato `source_row_zero_based` sarebbe inventato.
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![Arc::new(Int64Array::from(vec![Some(1), Some(0), Some(2)]))],
    )
    .expect("fixture");
    let steps = [
        (
            "formula",
            json!({"new_column": "ratio", "formula": "10 / id"}),
        ),
        (
            "expression",
            json!({
                "output_column": "ratio",
                "expression": {
                    "kind": "binary",
                    "op": "divide",
                    "left": {"kind": "literal", "value": 10},
                    "right": {"kind": "column", "name": "id"}
                }
            }),
        ),
    ];
    for (operation, config) in steps {
        let directory = tempfile::tempdir().expect("tempdir");
        let input = directory.path().join("input.arrow");
        let output = directory.path().join("output.arrow");
        let plan = directory.path().join("plan.json");
        write_batches(&input, std::slice::from_ref(&batch), &schema);
        std::fs::write(
            &plan,
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "steps": [
                    {"operation": "sort", "config": {"columns": ["id"]}},
                    {"operation": operation, "config": config}
                ]
            }))
            .expect("json"),
        )
        .expect("write plan");
        let result = Command::new(executable())
            .arg("run")
            .arg("--plan")
            .arg(&plan)
            .arg("--input")
            .arg(&input)
            .arg("--output")
            .arg(&output)
            .output()
            .expect("run legacy formula/expression");
        let stderr = String::from_utf8_lossy(&result.stdout).into_owned();
        assert!(
            !result.status.success(),
            "{operation}: piano legacy blocking+diagnostico accettato: {stderr}"
        );
        assert!(
            !output.exists(),
            "{operation}: nessun output pubblicabile da un piano rifiutato"
        );
        assert!(
            result.stderr.is_empty(),
            "{operation}: stderr deve restare vuoto: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            stderr.contains("piano DAG"),
            "{operation}: atteso rifiuto fail-closed verso il DAG: {stderr}"
        );
        assert!(
            !stderr.contains("row_diagnostics"),
            "{operation}: indici post-sort pubblicati come source_row: {stderr}"
        );
    }
}

// Anti-drift: il gate legacy deve coprire TUTTE le op che
// l'autorita' di catalogo dichiara row-diagnostics ed esprimibili in un
// piano legacy (alias storico). L'universo delle op arriva dal catalogo,
// non da una lista duplicata nel test: un'op diagnostica nuova o
// riclassificata rompe questo test finche' gate e autorita' divergono.
fn row_diagnostics_config_probes() -> Vec<serde_json::Value> {
    let mut probes = vec![json!({})];
    for target in [
        "int",
        "float",
        "bool",
        "uint64",
        "date",
        "datetime",
        "date32",
        "timestamp_millis",
        "decimal128",
        "str",
    ] {
        probes.push(json!({"target_type": target}));
        probes.push(json!({"target_type": target, "errors": "coerce"}));
        probes.push(json!({"target_type": target, "errors": "raise"}));
    }
    for policy in ["error", "empty", "literal"] {
        probes.push(json!({"null_policy": policy}));
    }
    probes
}

#[test]
fn every_legacy_expressible_row_diagnostics_operation_requires_dag_v4() {
    let probes = row_diagnostics_config_probes();

    let schema = Schema::new(vec![Field::new("id", DataType::Int64, true)]);
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![Arc::new(Int64Array::from(vec![Some(1)]))],
    )
    .expect("fixture");
    let mut gated = std::collections::BTreeSet::new();
    for descriptor in CATALOG {
        let aliases: Vec<&str> = plenora_core::catalog::ALIASES
            .iter()
            .filter(|(_, _, canonical)| *canonical == descriptor.id)
            .map(|(_, alias, _)| *alias)
            .collect();
        if aliases.is_empty() {
            continue;
        }
        let Some(config) = probes
            .iter()
            .find(|config| descriptor.emits_row_diagnostics(config))
        else {
            continue;
        };
        for alias in aliases {
            let directory = tempfile::tempdir().expect("tempdir");
            let input = directory.path().join("input.arrow");
            let output = directory.path().join("output.arrow");
            let plan = directory.path().join("plan.json");
            write_batches(&input, std::slice::from_ref(&batch), &schema);
            std::fs::write(
                &plan,
                serde_json::to_vec(&json!({
                    "schema_version": 1,
                    "steps": [
                        {"operation": "sort", "config": {"columns": ["id"]}},
                        {"operation": alias, "config": config}
                    ]
                }))
                .expect("json"),
            )
            .expect("write plan");
            let result = Command::new(executable())
                .arg("run")
                .arg("--plan")
                .arg(&plan)
                .arg("--input")
                .arg(&input)
                .arg("--output")
                .arg(&output)
                .output()
                .expect("run legacy gate probe");
            let stderr = String::from_utf8_lossy(&result.stdout).into_owned();
            assert!(
                !result.status.success(),
                "{} (alias `{alias}`): bypass del gate legacy: {stderr}",
                descriptor.id
            );
            assert!(
                stderr.contains("piano DAG"),
                "{} (alias `{alias}`): atteso rifiuto dal gate verso il DAG: {stderr}",
                descriptor.id
            );
            assert!(
                !stderr.contains("row_diagnostics"),
                "{} (alias `{alias}`): row_diagnostics inventata: {stderr}",
                descriptor.id
            );
            assert!(
                !output.exists(),
                "{} (alias `{alias}`): output pubblicato da piano rifiutato",
                descriptor.id
            );
            gated.insert(descriptor.id);
        }
    }
    // Lock espliciti del perimetro atteso (formula ed expression;
    // md5/sha256 con null_policy=error; type_cast fallibile).
    for expected in [
        "table.formula",
        "table.expression",
        "table.type_cast",
        "table.md5_hash",
        "table.sha256_hash",
        "table.flatten_json",
        "geo.centroid",
    ] {
        assert!(
            gated.contains(expected),
            "{expected}: op diagnostica con alias legacy non coperta dal gate"
        );
    }
}

// ---------------------------------------------------------------------------
// Convenzioni condivise con plenora-database: canale dell'envelope, exit code
// stabili, formato di output, identita' leggibile da un programma
// ---------------------------------------------------------------------------

#[test]
fn l_envelope_vive_su_stdout_e_stderr_resta_vuoto() {
    // Convenzione di famiglia: chi orchestra i due componenti cerca gli
    // errori in un posto solo. Vale anche per gli errori di INVOCAZIONE,
    // che nascono prima di qualunque comando.
    let result = Command::new(executable())
        .args(["run", "--plan"])
        .output()
        .expect("invocazione CLI");
    assert!(!result.status.success());
    assert!(
        result.stderr.is_empty(),
        "stderr deve restare vuoto: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("stdout deve essere l'envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["protocol_version"], 1);
    assert!(envelope["error"]["category"].is_string());
    assert!(envelope["error"]["phase"].is_string());
    assert!(envelope["error"]["remote_effect"].is_string());
    assert!(envelope["error"]["retry"]["kind"].is_string());
}

#[test]
fn gli_exit_code_seguono_la_categoria_dell_envelope() {
    // Il codice e' una proiezione della categoria, non un numero a parte:
    // uno script che non vuole parsare JSON deve poter distinguere almeno le
    // classi. Ogni caso verifica ENTRAMBI — categoria e codice — cosi' una
    // divergenza fra i due non passa.
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    write_input(&input);

    // Piano malformato -> invalid_plan -> 2.
    let plan = directory.path().join("plan.json");
    std::fs::write(&plan, "{ non json").expect("plan");
    let result = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(directory.path().join("out.arrow"))
        .output()
        .expect("run");
    let envelope: serde_json::Value = serde_json::from_slice(&result.stdout).expect("envelope");
    assert_eq!(envelope["error"]["category"], "data_mapping");
    assert_eq!(result.status.code(), Some(3), "data_mapping -> 3");

    // File inesistente -> io -> 5.
    let result = Command::new(executable())
        .args(["describe", "--input"])
        .arg(directory.path().join("assente.arrow"))
        .output()
        .expect("describe");
    let envelope: serde_json::Value = serde_json::from_slice(&result.stdout).expect("envelope");
    assert_eq!(envelope["error"]["category"], "io");
    assert_eq!(result.status.code(), Some(5), "io -> 5");

    // Argomento mancante -> invalid_plan -> 2.
    let result = Command::new(executable())
        .args(["describe"])
        .output()
        .expect("describe");
    let envelope: serde_json::Value = serde_json::from_slice(&result.stdout).expect("envelope");
    assert_eq!(envelope["error"]["category"], "invalid_plan");
    assert_eq!(result.status.code(), Some(2), "invalid_plan -> 2");
}

#[test]
fn il_formato_globale_e_json_per_default_e_rifiuta_cio_che_non_rende() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    write_input(&input);

    // Default: JSON, senza dover chiedere nulla.
    let result = Command::new(executable())
        .args(["describe", "--input"])
        .arg(&input)
        .output()
        .expect("describe");
    assert!(result.status.success());
    serde_json::from_slice::<serde_json::Value>(&result.stdout).expect("JSON per default");

    // Markdown dove esiste una resa leggibile, e il flag vale anche PRIMA
    // del sottocomando.
    for args in [vec!["describe"], vec!["--format", "markdown", "describe"]] {
        let mut command = Command::new(executable());
        command.args(&args);
        if args.len() == 1 {
            command.args(["--format", "markdown"]);
        }
        let result = command
            .args(["--input"])
            .arg(&input)
            .output()
            .expect("describe markdown");
        assert!(result.status.success(), "{args:?}");
        let testo = String::from_utf8_lossy(&result.stdout);
        assert!(testo.contains("## Campi"), "{args:?}: {testo}");
        assert!(
            serde_json::from_slice::<serde_json::Value>(&result.stdout).is_err(),
            "in markdown l'output non deve essere JSON"
        );
    }

    // Dove una resa leggibile non c'e', il flag e' RIFIUTATO invece di
    // essere ignorato: un flag accettato e disatteso e' peggio.
    let result = Command::new(executable())
        .args(["--format", "markdown", "run", "--plan", "x.json"])
        .output()
        .expect("run markdown");
    assert!(!result.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&result.stdout).expect("envelope");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("non e' disponibile per `run`")),
        "{envelope}"
    );

    // Un formato sconosciuto e' un errore, non un ripiego silenzioso su json.
    let result = Command::new(executable())
        .args(["--format", "yaml", "catalog"])
        .output()
        .expect("catalog yaml");
    assert!(!result.status.success());
    assert_eq!(result.status.code(), Some(2));
}

#[test]
fn la_versione_e_leggibile_da_un_programma() {
    let result = Command::new(executable())
        .args(["--version"])
        .output()
        .expect("version");
    assert!(result.status.success());
    let documento: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("--version emette JSON");
    assert_eq!(documento["component"], "plenora-data-tools");
    assert_eq!(documento["component_version"], env!("CARGO_PKG_VERSION"));
    assert!(documento["arrow_version"].is_string());
    assert!(documento["backends"].is_array());
    assert_eq!(documento["operations"], CATALOG.len());

    // `capabilities` porta la stessa identita' accanto al documento
    // dichiarativo: chi interroga le capability non deve chiedere altrove
    // con quale binario sta parlando.
    let result = Command::new(executable())
        .args(["capabilities"])
        .output()
        .expect("capabilities");
    assert!(result.status.success());
    let documento: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("capabilities JSON");
    assert_eq!(documento["component_version"], env!("CARGO_PKG_VERSION"));
    assert!(documento["backends"].is_array());
}

#[test]
fn ogni_sottocomando_del_dispatch_ha_un_help_che_lo_nomina() {
    // Un help che non elenca un comando, o che ne elenca uno inesistente, e'
    // un difetto: e' la prima cosa che legge chi non conosce il tool. La
    // lista e' quella del dispatch, non una copia — se il dispatch cambia e
    // l'help no, questo test cade.
    const COMANDI: [&str; 10] = [
        "catalog",
        "describe",
        "inspect-dataset",
        "validate",
        "run",
        "capabilities",
        "transform",
        "spatial-join",
        "transform-arrow",
        "pair-arrow",
    ];
    let generale = Command::new(executable())
        .args(["--help"])
        .output()
        .expect("help");
    assert!(generale.status.success());
    let generale = String::from_utf8_lossy(&generale.stdout).into_owned();
    for comando in COMANDI {
        // `inspect-dataset` compare come alias sulla riga di `describe`.
        assert!(
            generale.contains(comando),
            "l'help generale non nomina `{comando}`"
        );
        let specifico = Command::new(executable())
            .args([comando, "--help"])
            .output()
            .expect("help del sottocomando");
        assert!(
            specifico.status.success(),
            "`{comando} --help` deve funzionare"
        );
        let testo = String::from_utf8_lossy(&specifico.stdout);
        assert!(
            testo.contains(comando) || testo.contains("describe"),
            "`{comando} --help` non nomina il comando: {testo}"
        );
    }
    // Ogni comando accetta `--format` senza che l'help debba ripeterlo: e'
    // globale, e viene tolto prima del dispatch.
    let result = Command::new(executable())
        .args(["--format", "json", "--help"])
        .output()
        .expect("help con formato");
    assert!(result.status.success());
}
