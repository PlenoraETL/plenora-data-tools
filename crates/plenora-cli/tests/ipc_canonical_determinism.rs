//! Test del livello 2 di determinismo (ADR-0001, "Determinismo IPC
//! canonico"): stesso piano validato, stesso input, stessa configurazione
//! del writer → i due output IPC devono essere binariamente identici.
//!
//! Il livello 2 e' garantito solo a parita' di versione dell'engine ed e'
//! testato separatamente dal livello 1 (determinismo semantico, verificato
//! con confronto geometrico/semantico). Se questo test fallisce segnala un
//! NON-determinismo binario reale del motore: non va "corretto" il test.

use std::process::Command;
use std::sync::Arc;

use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
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
fn stesso_piano_produce_output_ipc_binariamente_identici() {
    let directory = tempfile::tempdir().expect("tempdir");
    let input = directory.path().join("input.arrow");
    let output_first = directory.path().join("output_first.arrow");
    let output_second = directory.path().join("output_second.arrow");
    let plan = directory.path().join("plan.json");

    let schema = Schema::new(vec![
        Field::new("name", DataType::Utf8, true),
        Field::new("amount", DataType::Int64, true),
    ]);
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![
            Arc::new(StringArray::from(vec![
                Some("alpha"),
                Some("beta"),
                Some("gamma"),
                None,
            ])),
            Arc::new(Int64Array::from(vec![Some(10), Some(-4), None, Some(7)])),
        ],
    )
    .expect("batch");
    write_ipc(&input, &schema, &[batch]);

    // Piano a due passi (alias legacy del catalogo), identico per entrambe
    // le esecuzioni.
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[
            {"operation":"rename","config":{"renames":[{"old_name":"name","new_name":"label"}]}},
            {"operation":"sort","config":{"columns":["label","amount"]}}
        ]}"#,
    )
    .expect("plan");

    for output in [&output_first, &output_second] {
        let result = cli()
            .args(["run", "--plan"])
            .arg(&plan)
            .arg("--input")
            .arg(&input)
            .arg("--output")
            .arg(output)
            .output()
            .expect("run");
        assert!(
            result.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&result.stdout)
        );
    }

    let first = std::fs::read(&output_first).expect("read first output");
    let second = std::fs::read(&output_second).expect("read second output");
    assert_eq!(
        first,
        second,
        "livello 2 violato: stessi piano/input/writer ma output IPC non \
         binariamente identici ({} vs {} byte)",
        first.len(),
        second.len()
    );
}
