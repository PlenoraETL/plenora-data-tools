//! Che il ramo **legacy** di `run` stampi davvero l'esito della pubblicazione.
//!
//! # Perche' esiste, separato dai casi del serializzatore
//!
//! Perche' i casi che vivono accanto a `campi_della_pubblicazione` provano che
//! quella funzione costruisce i campi giusti, e **nient'altro**: togliendo la
//! `println!` dal comando resterebbero tutti verdi, e l'avvertenza di pulizia
//! tornerebbe a non uscire da nessuna parte senza che un solo caso se ne
//! accorga. Provare il serializzatore non prova l'emissione.
//!
//! Questo caso guarda invece **stdout del processo**, cioe' l'unica cosa che
//! chi usa il comando vede. E' la controprova mirata: esegue il binario come lo
//! esegue un utente, legge cio' che stampa e pretende che sia il documento.
//!
//! # Che cosa fissa, e in che ordine
//!
//! Prima che stdout sia un documento JSON — non testo che gli somiglia — poi
//! che porti `status`, `durability_confirmed` e `temp_cleanup`, e infine che
//! `temp_cleanup` sia un oggetto con uno `state`. Le tre affermazioni sono
//! separate perche' si rompono separatamente: si puo' perdere la stampa, si puo'
//! perdere un campo, si puo' cambiare la forma di un campo.
//!
//! # Perche' non pretende un valore preciso di `temp_cleanup`
//!
//! Perche' dipende dalla piattaforma: dove il commit e' un rename atomico il
//! temporaneo non c'e' piu', dove si ripiega su `hard_link` + `unlink` puo'
//! restare. Pretendere `removed` renderebbe il caso rosso su un sistema in cui
//! il codice si comporta **correttamente**. Cio' che deve valere ovunque e' che
//! il campo ci sia e dica il proprio stato.

use std::process::Command;

/// Il piano legacy piu' piccolo che produca un output: nessun passo geo,
/// nessuna feature, cosi' il caso gira ovunque gira la CLI.
const PIANO_LEGACY: &[u8] = br#"{"schema_version":1,"steps":[{"operation":"rename","config":{"renames":[{"old_name":"valore","new_name":"rinominato"}]}}]}"#;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_plenora-data-tools"))
}

/// Scrive un ingresso Arrow IPC minimo.
fn scrivi_ingresso(percorso: &std::path::Path) {
    use std::sync::Arc;

    use plenora_core::arrow::array::{RecordBatch, StringArray};
    use plenora_core::arrow::ipc::writer::FileWriter;
    use plenora_core::arrow::schema::{DataType, Field, Schema};

    let schema = Schema::new(vec![Field::new("valore", DataType::Utf8, true)]);
    let batch = RecordBatch::try_new(
        Arc::new(schema.clone()),
        vec![Arc::new(StringArray::from(vec![Some("a"), Some("b")]))],
    )
    .expect("il batch di prova si costruisce");
    let file = std::fs::File::create(percorso).expect("l'ingresso si crea");
    let mut scrittore =
        FileWriter::try_new(file, &Arc::new(schema)).expect("il writer si costruisce");
    scrittore.write(&batch).expect("scrittura");
    scrittore.finish().expect("chiusura");
}

/// **Il ramo legacy di `run` stampa il documento con l'esito.**
#[test]
fn il_run_legacy_stampa_l_esito_della_pubblicazione() {
    let stanza = tempfile::tempdir().expect("cartella");
    let piano = stanza.path().join("piano.json");
    let ingresso = stanza.path().join("ingresso.arrow");
    let uscita = stanza.path().join("uscita.arrow");
    std::fs::write(&piano, PIANO_LEGACY).expect("il piano si scrive");
    scrivi_ingresso(&ingresso);

    let esito = cli()
        .args(["run", "--plan"])
        .arg(&piano)
        .arg("--input")
        .arg(&ingresso)
        .arg("--output")
        .arg(&uscita)
        .output()
        .expect("il comando parte");

    let testo = String::from_utf8_lossy(&esito.stdout).into_owned();
    assert!(
        esito.status.success(),
        "il run legacy deve riuscire — stdout: {testo} — stderr: {}",
        String::from_utf8_lossy(&esito.stderr)
    );
    assert!(uscita.is_file(), "e l'output deve esserci");

    // 1. Stdout e' un documento, non testo che gli somiglia.
    let documento: serde_json::Value = serde_json::from_str(testo.trim()).unwrap_or_else(|causa| {
        panic!("il ramo legacy deve stampare un documento JSON, e non lo fa: «{testo}» ({causa})")
    });

    // 2. Porta i campi che chi legge deve trovare.
    assert_eq!(
        documento.get("status").and_then(serde_json::Value::as_str),
        Some("ok"),
        "manca lo stato: {documento}"
    );
    assert!(
        documento
            .get("durability_confirmed")
            .is_some_and(serde_json::Value::is_boolean),
        "manca la durabilita', o non e' un booleano: {documento}"
    );

    // 3. E la pulizia del temporaneo dice il proprio stato.
    let pulizia = documento
        .get("temp_cleanup")
        .unwrap_or_else(|| panic!("manca l'avvertenza di pulizia: {documento}"));
    assert!(
        pulizia.is_object(),
        "la pulizia e' sempre un oggetto: {pulizia}"
    );
    assert!(
        pulizia
            .get("state")
            .is_some_and(serde_json::Value::is_string),
        "e porta sempre uno stato: {pulizia}"
    );
}
