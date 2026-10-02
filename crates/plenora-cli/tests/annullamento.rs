//! Annullamento cooperativo (CLI 2.0, sezione 9): con il segnale alzato i
//! comandi che lo dichiarano finiscono con `cancelled` ed exit 130, senza
//! output; senza gestore installato non partono.
//!
//! Il segnale si alza qui direttamente: è lo stesso `Arc<AtomicBool>` che
//! `main.rs` passa al gestore di Ctrl-C e SIGTERM (`ctrlc`). Che il sistema
//! operativo consegni il segnale al gestore non è provato da questi test
//! (servirebbe inviare Ctrl-C a un sottoprocesso, cioè FFI): è un limite
//! dichiarato in docs/cli.md, «Annullamento dal sistema operativo non
//! provato da un test».

mod comune;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use comune::valida_inviluppo;
use plenora_cli::esegui_invocazione;
use plenora_core::arrow::array::{Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_io::{scrivi_tabella, OpzioniScrittura};
use serde_json::Value;

fn documento(stdout: &str) -> Value {
    let documento: Value = serde_json::from_str(stdout).expect("JSON");
    valida_inviluppo(&documento).expect("inviluppo");
    documento
}

#[test]
fn segnale_alzato_annulla_senza_output() {
    let dir = tempfile::tempdir().expect("cartella");
    let ingresso = dir.path().join("t.arrow");
    let tabella = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("a", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1, 2]))],
    )
    .expect("tabella");
    scrivi_tabella(&tabella, &ingresso, &OpzioniScrittura::default()).expect("ingresso");
    let piano = dir.path().join("p.json");
    std::fs::write(
        &piano,
        r#"{"version": 1, "inputs": ["t"], "steps": [
            {"out": "x", "op": "table.limit", "in": ["t"], "config": {"n": 1}}],
            "outputs": ["x"]}"#,
    )
    .expect("piano");
    let uscita = dir.path().join("x.arrow");
    let testo = |percorso: &Path| percorso.to_string_lossy().into_owned();
    let alzato: plenora_cli::Segnale = Some(Arc::new(AtomicBool::new(true)));
    let casi: Vec<Vec<String>> = vec![
        vec!["describe".into(), "--input".into(), testo(&ingresso)],
        vec![
            "validate".into(),
            "--plan".into(),
            testo(&piano),
            "--input".into(),
            format!("t={}", testo(&ingresso)),
        ],
        vec![
            "run".into(),
            "--plan".into(),
            testo(&piano),
            "--input".into(),
            format!("t={}", testo(&ingresso)),
            "--output".into(),
            testo(&uscita),
            "--timeout-ms".into(),
            "0".into(),
        ],
    ];
    for argomenti in &casi {
        let esito = esegui_invocazione(argomenti, &alzato);
        assert_eq!(esito.codice, 130, "{}", esito.stdout);
        let documento = documento(&esito.stdout);
        let errore = &documento["error"];
        assert_eq!(errore["category"], "cancelled");
        assert_eq!(errore["code"], "EXECUTION_CANCELLED");
        assert_eq!(errore["remote_effect"], "none");
        assert!(!uscita.exists());
    }

    // Senza gestore installato i comandi che dichiarano l'annullamento non
    // partono; la scoperta sì.
    for argomenti in &casi {
        let esito = esegui_invocazione(argomenti, &None);
        assert_eq!(esito.codice, 70, "{}", esito.stdout);
        assert_eq!(documento(&esito.stdout)["error"]["category"], "internal");
    }
    let esito = esegui_invocazione(&["catalog".to_owned()], &None);
    assert_eq!(esito.codice, 0);
}
