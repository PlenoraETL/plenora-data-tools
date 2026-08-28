//! Esempi end-to-end di `examples/`, eseguiti come li esegue chi legge il
//! README: stessi comandi, stessi file, stesso output atteso.
//!
//! Un esempio che non riproduce il proprio `atteso/` rompe la build. E' il
//! solo modo perche' la documentazione resti vera: un comando copiato da un
//! README che non gira piu' e' peggio di un README assente.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use plenora_core::arrow::array::{Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc::reader::FileReader;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde_json::{json, Value};

const fn executable() -> &'static str {
    env!("CARGO_BIN_EXE_plenora-data-tools")
}

/// Radice del repository: `crates/plenora-cli` risalito di due livelli.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("radice del repository")
        .to_path_buf()
}

fn leggi_json(path: &Path) -> Value {
    let testo = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("lettura di {}: {error}", path.display()));
    serde_json::from_str(&testo).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Scrive le righe JSON dell'esempio come Arrow IPC file format.
///
/// L'input committato e' JSON perche' sia leggibile in una revisione; il file
/// Arrow lo si materializza qui, cosi' il repository non porta binari e
/// l'esempio resta ispezionabile.
fn scrivi_arrow(righe: &Value, destinazione: &Path) {
    let righe = righe.as_array().expect("array di righe");
    let nomi: Vec<Option<&str>> = righe.iter().map(|riga| riga["nome"].as_str()).collect();
    let regioni: Vec<Option<&str>> = righe.iter().map(|riga| riga["regione"].as_str()).collect();
    let abitanti: Vec<Option<i64>> = righe.iter().map(|riga| riga["abitanti"].as_i64()).collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new("nome", DataType::Utf8, false),
        Field::new("regione", DataType::Utf8, false),
        Field::new("abitanti", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(nomi)),
            Arc::new(StringArray::from(regioni)),
            Arc::new(Int64Array::from(abitanti)),
        ],
    )
    .expect("batch dell'esempio");
    let file = std::fs::File::create(destinazione).expect("create arrow");
    let mut writer = FileWriter::try_new(file, &schema).expect("writer");
    writer.write(&batch).expect("write");
    writer.finish().expect("finish");
}

/// Rilegge un Arrow IPC file come righe JSON, per confrontarlo con `atteso/`.
fn arrow_come_json(path: &Path) -> Value {
    let file = std::fs::File::open(path).expect("open output");
    let reader = FileReader::try_new(file, None).expect("reader");
    let mut righe = Vec::new();
    for batch in reader {
        let batch = batch.expect("batch");
        let nome = batch
            .column_by_name("nome")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .expect("colonna nome");
        let regione = batch
            .column_by_name("regione")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .expect("colonna regione");
        let abitanti = batch
            .column_by_name("abitanti")
            .and_then(|column| column.as_any().downcast_ref::<Int64Array>())
            .expect("colonna abitanti");
        for indice in 0..batch.num_rows() {
            righe.push(json!({
                "nome": nome.value(indice),
                "regione": regione.value(indice),
                "abitanti": abitanti.value(indice),
            }));
        }
    }
    Value::Array(righe)
}

#[test]
fn e1_filtro_e_ordinamento_riproduce_l_output_atteso() {
    let esempio = repository().join("examples/e1-filtro-ordinamento");
    let lavoro = tempfile::tempdir().expect("tempdir");
    let input = lavoro.path().join("citta.arrow");
    let output = lavoro.path().join("output.arrow");
    scrivi_arrow(&leggi_json(&esempio.join("dati/citta.json")), &input);
    let piano = esempio.join("piano.json");

    // 1. `describe`: cosa contiene l'input. E' il primo comando del README.
    let descrizione = Command::new(executable())
        .args(["describe", "--input"])
        .arg(&input)
        .output()
        .expect("describe");
    assert!(
        descrizione.status.success(),
        "describe fallito: {}",
        String::from_utf8_lossy(&descrizione.stdout)
    );
    let descrizione: Value =
        serde_json::from_slice(&descrizione.stdout).expect("describe emette JSON");
    assert_eq!(descrizione["status"], "ok");
    let campi: Vec<&str> = descrizione["fields"]
        .as_array()
        .expect("campi")
        .iter()
        .map(|campo| campo["name"].as_str().expect("nome"))
        .collect();
    assert_eq!(campi, ["nome", "regione", "abitanti"]);
    assert!(
        descrizione["contract_fingerprint"]
            .as_str()
            .is_some_and(|hex| hex.len() == 64),
        "il fingerprint del contratto deve essere esadecimale a 32 byte"
    );
    // Nessuna geometria in questo esempio, e il campo lo dice invece di
    // tacere.
    assert_eq!(descrizione["geometries"], json!([]));

    // 2. `validate`: il piano si controlla contro il contratto, senza dati.
    let validazione = Command::new(executable())
        .args(["validate", "--plan"])
        .arg(&piano)
        .arg("--input")
        .arg(format!("citta={}", input.display()))
        .output()
        .expect("validate");
    assert!(
        validazione.status.success(),
        "validate fallito: {}",
        String::from_utf8_lossy(&validazione.stdout)
    );

    // 3. `run`: esecuzione e pubblicazione atomica.
    let esecuzione = Command::new(executable())
        .args(["run", "--plan"])
        .arg(&piano)
        .arg("--input")
        .arg(format!("citta={}", input.display()))
        .arg("--output")
        .arg(&output)
        .output()
        .expect("run");
    assert!(
        esecuzione.status.success(),
        "run fallito: {}",
        String::from_utf8_lossy(&esecuzione.stdout)
    );
    let metriche: Value = serde_json::from_slice(&esecuzione.stdout).expect("run emette JSON");
    assert!(
        metriche.get("metrics").is_some() || metriche.get("output_rows").is_some(),
        "run deve stampare le metriche: {metriche}"
    );

    // 4. L'output e' esattamente quello committato in `atteso/`.
    assert_eq!(
        arrow_come_json(&output),
        leggi_json(&esempio.join("atteso/output.json")),
        "l'esempio non riproduce piu' il proprio output atteso"
    );
}

#[test]
fn e1_rifiuta_un_nome_di_input_che_il_piano_non_dichiara() {
    // Il difetto che la forma nominale chiude: con `--inputs` un percorso
    // puo' finire sull'input sbagliato in silenzio. Qui un nome errato e'
    // un errore prima di leggere qualunque dato.
    let esempio = repository().join("examples/e1-filtro-ordinamento");
    let lavoro = tempfile::tempdir().expect("tempdir");
    let input = lavoro.path().join("citta.arrow");
    scrivi_arrow(&leggi_json(&esempio.join("dati/citta.json")), &input);

    let esito = Command::new(executable())
        .args(["validate", "--plan"])
        .arg(esempio.join("piano.json"))
        .arg("--input")
        .arg(format!("sbagliato={}", input.display()))
        .output()
        .expect("validate");
    assert!(
        !esito.status.success(),
        "un nome non dichiarato deve fallire"
    );
    let messaggio = String::from_utf8_lossy(&esito.stdout);
    assert!(
        messaggio.contains("non dichiarato dal piano") && messaggio.contains("citta"),
        "l'errore deve dire quali nomi il piano dichiara: {messaggio}"
    );
}
