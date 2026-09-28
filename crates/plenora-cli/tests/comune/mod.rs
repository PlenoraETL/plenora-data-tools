//! Cio' che i test di integrazione della CLI condividono.
//!
//! # Perche' un modulo e non una copia per file
//!
//! Perche' le forme che il parser della CLI usa per rifiutare un comando ignoto
//! sono un **fatto solo**, e due copie sono due occasioni di divergere. Quella
//! che diverge e' sempre la seconda: il giorno che il messaggio cambia, un file
//! viene aggiornato e l'altro continua a cercare una parola che nessuno scrive
//! piu' — restando verde, perche' un'asserzione che cerca cio' che non c'e'
//! passa sempre.
//!
//! Lo stesso vale per l'impalcatura: l'invocazione del binario, la scrittura
//! di un ingresso Arrow IPC, le fixture tabellari e i punti WKB. Qui ce n'e'
//! una copia sola; ogni differenza fra i casi resta un parametro, mai una
//! scelta fatta per tutti.
//!
//! Sta sotto `tests/comune/` e non in un file di primo livello perche' solo i
//! `.rs` direttamente in `tests/` diventano target: un `mod.rs` in una
//! sottodirectory si include e basta.

// Ogni target usa un sottoinsieme di questa impalcatura: cio' che non usa non
// e' codice morto, e' codice di un altro target.
#![allow(dead_code)]

pub mod costanti;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, BinaryArray, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use serde_json::json;

/// Se il messaggio viene dal parser della CLI invece che dal dispatch.
///
/// # Perche' tre forme, e perche' proprio queste
///
/// Perche' il parser rifiuta un comando ignoto con **«comando non valido»**, e
/// una verifica che cercasse solo «sconosciuto» non lo vedrebbe: cercherebbe
/// per sempre una parola che quel messaggio non contiene, e il caso resterebbe
/// verde anche col dispatch tolto. E' un difetto misurato, non temuto: col
/// dispatch tolto dal `main`, l'asserzione su «sconosciuto» passa lo stesso.
///
/// Il caso nel suo insieme se ne accorge comunque — altre sue asserzioni
/// cadono — ma **questa** riga, che esiste apposta per distinguere il rifiuto
/// del dispatch da quello del parser, non distingue niente.
///
/// Le altre due restano perche' la formulazione puo' cambiare, e un elenco piu'
/// largo sbaglia solo per eccesso di allarme — che e' il verso giusto in cui
/// sbagliare, per un'asserzione che deve accorgersi di una ricaduta.
pub fn ricaduta_nel_parser(testo: &str) -> bool {
    ["comando non valido", "sconosciuto", "unknown"]
        .iter()
        .any(|forma| testo.contains(forma))
}

/// Il binario di questo workspace, come lo esegue chi lo usa.
pub const fn eseguibile() -> &'static str {
    env!("CARGO_BIN_EXE_plenora-data-tools")
}

/// Un'invocazione del binario, da completare con gli argomenti.
pub fn cli() -> Command {
    Command::new(eseguibile())
}

/// Esegue il binario e rende exit code e testo unito: l'envelope va su
/// stdout, ma un messaggio che finisse su stderr non deve sfuggire al caso.
///
/// `senza` e' la variabile d'ambiente tolta al figlio, se il caso prova il
/// cammino in cui manca.
pub fn esegui(argomenti: &[&str], senza: Option<&str>) -> (i32, String) {
    let mut comando = cli();
    comando.args(argomenti);
    if let Some(variabile) = senza {
        comando.env_remove(variabile);
    }
    let uscita = comando.output().expect("il binario si esegue");
    let mut testo = String::from_utf8_lossy(&uscita.stdout).into_owned();
    testo.push_str(&String::from_utf8_lossy(&uscita.stderr));
    (uscita.status.code().unwrap_or(-1), testo)
}

/// `run --plan <piano> <flag> <ingresso> --output <uscita>`, con il flag
/// d'ingresso (`--input` o `--inputs`) scritto da chi chiama.
///
/// Rende il comando e non l'esito: chi chiama sceglie fra `output()` e
/// `status()`, e puo' aggiungere argomenti in coda.
pub fn comando_run(piano: &Path, flag: &str, ingresso: &Path, uscita: &Path) -> Command {
    let mut comando = cli();
    comando
        .args(["run", "--plan"])
        .arg(piano)
        .arg(flag)
        .arg(ingresso)
        .arg("--output")
        .arg(uscita);
    comando
}

/// Invoca `run --plan <piano> --inputs <ingresso> --output <uscita>`.
///
/// Rende l'uscita del processo **grezza**: codice, stdout e stderr restano da
/// esaminare a chi chiama, perche' sono gli oracoli e non possono stare qui.
pub fn cli_run(piano: &Path, ingresso: &Path, uscita: &Path) -> Output {
    comando_run(piano, "--inputs", ingresso, uscita)
        .output()
        .expect("il processo si avvia")
}

/// Scrive `batches` in `path` come Arrow IPC file format, con lo schema dato.
pub fn scrivi_ipc(path: &Path, schema: &Schema, batches: &[RecordBatch]) {
    let file = std::fs::File::create(path).expect("create input");
    let mut writer = FileWriter::try_new(file, schema).expect("writer");
    for batch in batches {
        writer.write(batch).expect("write batch");
    }
    writer.finish().expect("finish");
}

/// Tutti i sottocomandi del dispatch. `inspect-dataset` e' l'alias di
/// `describe` e va verificato come gli altri: un alias non controllato e' una
/// superficie non controllata.
pub const COMANDI: [&str; 10] = [
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

/// Schema tabellare: `id` Int64 non nullo + `name` Utf8.
pub fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]))
}

/// Un batch di [`table_schema`], senza null.
pub fn table_batch(ids: &[i64], names: &[&str]) -> RecordBatch {
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
pub fn table_plan() -> serde_json::Value {
    json!({
        "schema_version": 5,
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

/// Piano v4: solo `table.filter` `id > 0` (colonna non geometrica).
pub fn filter_only_plan() -> serde_json::Value {
    json!({
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
}

/// Scrive `piano` come JSON in `path`.
pub fn scrivi_piano(path: &Path, piano: &serde_json::Value) {
    std::fs::write(path, serde_json::to_vec(piano).expect("json")).expect("plan");
}

/// Scrive piano e input tabellare standard nella directory data:
/// `plan.json` con [`table_plan`], `input.arrow` con tre righe (id 0, 1, 2).
pub fn write_table_fixture(directory: &Path) -> (PathBuf, PathBuf) {
    let plan = directory.join("plan.json");
    let input = directory.join("input.arrow");
    scrivi_piano(&plan, &table_plan());
    scrivi_ipc(
        &input,
        &table_schema(),
        &[table_batch(&[0, 1, 2], &["a", "b", "c"])],
    );
    (plan, input)
}

/// WKB ISO little-endian di un Point XY (type code 1).
pub fn point_wkb_le(x: f64, y: f64) -> Vec<u8> {
    let mut wkb = Vec::with_capacity(21);
    wkb.push(1_u8);
    wkb.extend_from_slice(&1_u32.to_le_bytes());
    wkb.extend_from_slice(&x.to_le_bytes());
    wkb.extend_from_slice(&y.to_le_bytes());
    wkb
}

/// WKB ISO little-endian di un Point Z (type code 1001).
pub fn point_z_wkb(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut payload = vec![1_u8];
    payload.extend_from_slice(&1001_u32.to_le_bytes());
    for value in [x, y, z] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload
}

/// Batch `id` Int64 + colonna geometria Binary, nell'ordine dello schema dato.
pub fn batch_id_geometria(
    schema: SchemaRef,
    ids: &[i64],
    cells: &[Option<Vec<u8>>],
) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geometria fixture")
}
