//! `describe`, `validate` e `run` dal binario: un successo, i rifiuti
//! tipizzati, la scadenza, i formati Arrow file e stream, il determinismo
//! (CLI 2.0, sezioni 4, 8 e 9; Typed Errors 1.0; Row Diagnostics 1.0;
//! Arrow Interchange 1.0).

mod comune;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use comune::{invoca, Esito};
use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_io::{leggi_tabella, scrivi_tabella, OpzioniScrittura};

/// Canarino: un testo che non deve mai comparire nell'uscita pubblica.
const CANARINO: &str = "CANARINO-7f3a91";

fn tabella() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("nome", DataType::Utf8, true),
            Field::new("valore", DataType::Float64, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3, 4])),
            Arc::new(StringArray::from(vec![
                Some("10"),
                Some(CANARINO),
                None,
                Some("40"),
            ])),
            Arc::new(Float64Array::from(vec![
                Some(1.5),
                Some(-2.0),
                None,
                Some(4.0),
            ])),
        ],
    )
    .expect("tabella")
}

struct Cartella {
    _dir: tempfile::TempDir,
    radice: PathBuf,
}

impl Cartella {
    fn nuova() -> Self {
        let dir = tempfile::tempdir().expect("cartella temporanea");
        // Il canarino anche nel percorso: un percorso non entra mai nei
        // messaggi pubblici.
        let radice = dir.path().join(CANARINO);
        std::fs::create_dir(&radice).expect("sottocartella");
        Self { _dir: dir, radice }
    }

    fn percorso(&self, nome: &str) -> String {
        self.radice.join(nome).to_string_lossy().into_owned()
    }

    fn con_tabella(&self, nome: &str) -> String {
        let percorso = self.percorso(nome);
        scrivi_tabella(
            &tabella(),
            Path::new(&percorso),
            &OpzioniScrittura::default(),
        )
        .expect("scrittura");
        percorso
    }

    fn con_piano(&self, nome: &str, testo: &str) -> String {
        let percorso = self.percorso(nome);
        std::fs::write(&percorso, testo).expect("piano");
        percorso
    }
}

const PIANO_FILTRO: &str = r#"{"version": 1, "inputs": ["t"],
  "steps": [{"out": "positivi", "op": "table.filter", "in": ["t"],
             "config": {"column": "valore", "operator": ">", "value": 0}}],
  "outputs": ["positivi"]}"#;

const PIANO_DUE_USCITE: &str = r#"{"version": 1, "inputs": ["t"],
  "steps": [
    {"out": "a", "op": "table.filter", "in": ["t"],
     "config": {"column": "valore", "operator": ">", "value": 0}},
    {"out": "b", "op": "table.limit", "in": ["t"], "config": {"n": 1}}],
  "outputs": ["a", "b"]}"#;

fn senza_canarino(esito: &Esito) {
    assert!(
        !esito.stdout.contains(CANARINO),
        "dato o percorso nell'uscita: {}",
        esito.stdout
    );
}

fn errore(esito: &Esito, categoria: &str, codice: i32) {
    assert_eq!(esito.documento["status"], "error", "{}", esito.stdout);
    assert_eq!(
        esito.documento["error"]["category"], categoria,
        "{}",
        esito.stdout
    );
    assert_eq!(esito.codice, codice, "{}", esito.stdout);
    senza_canarino(esito);
}

#[test]
fn describe_legge_file_stream_e_parquet() {
    let cartella = Cartella::nuova();
    for nome in ["t.arrow", "t.arrows", "t.parquet"] {
        let percorso = cartella.con_tabella(nome);
        let esito = invoca(&["describe", "--input", &percorso, "--format", "json"]);
        assert_eq!(esito.codice, 0, "{}", esito.stdout);
        assert_eq!(esito.documento["command"], "describe");
        assert_eq!(esito.documento["contract"], "plenora-data-description-v1");
        let risultato = &esito.documento["result"];
        assert_eq!(risultato["rows"], 4);
        let colonne = risultato["columns"].as_array().expect("columns");
        assert_eq!(colonne.len(), 3);
        assert_eq!(colonne[0]["name"], "id");
        assert_eq!(colonne[0]["type"], "Int64");
        assert_eq!(colonne[0]["nullable"], false);
        assert!(risultato["active_geometry"].is_null());
        senza_canarino(&esito);
    }
}

#[test]
fn il_formato_stream_si_scrive_stream_e_il_file_si_scrive_file() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.arrow");
    let piano = cartella.con_piano("p.json", PIANO_FILTRO);
    for (nome, tipo, inizio) in [
        (
            "o.arrow",
            "application/vnd.apache.arrow.file",
            &b"ARROW1"[..],
        ),
        (
            "o.arrows",
            "application/vnd.apache.arrow.stream",
            &[0xFF, 0xFF, 0xFF, 0xFF][..],
        ),
    ] {
        let uscita = cartella.percorso(nome);
        let esito = invoca(&[
            "run",
            "--plan",
            &piano,
            "--input",
            &format!("t={ingresso}"),
            "--output",
            &uscita,
        ]);
        assert_eq!(esito.codice, 0, "{}", esito.stdout);
        let risultato = &esito.documento["result"];
        assert_eq!(risultato["outputs"][0]["name"], "positivi");
        assert_eq!(risultato["outputs"][0]["content_type"], tipo);
        assert_eq!(risultato["outputs"][0]["rows"], 2);
        assert_eq!(risultato["steps"][0]["op"], "table.filter");
        let byte = std::fs::read(&uscita).expect("uscita");
        assert!(byte.starts_with(inizio), "{nome}");
        let letta = leggi_tabella(Path::new(&uscita), None, u64::MAX).expect("rilettura");
        assert_eq!(letta.num_rows(), 2);
        // Lo schema d'uscita porta la versione del contratto Arrow.
        assert_eq!(
            letta
                .schema()
                .metadata()
                .get("plenora.contract.version")
                .map(String::as_str),
            Some("1")
        );
        senza_canarino(&esito);
    }
}

/// I NAME di `--input` e `--output` si confrontano con il piano prima di
/// leggere: un percorso con `=` (diviso al primo `=`) non finisce nel
/// messaggio, e un input in più non si legge.
#[test]
fn nomi_degli_argomenti_contro_il_piano() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.arrow");
    let piano = cartella.con_piano("p.json", PIANO_FILTRO);
    let con_uguale = cartella.percorso("x=o.arrow");
    let mancante = cartella.percorso("manca.arrow");
    let casi: Vec<Vec<String>> = vec![
        // `--output` con `=` nel percorso: il pezzo prima non è un output.
        vec![
            "--input".into(),
            format!("t={ingresso}"),
            "--output".into(),
            con_uguale,
        ],
        // Un input non dichiarato (file assente): `invalid_configuration`,
        // non `not_found`, perché nessun file si legge.
        vec![
            "--input".into(),
            format!("t={ingresso}"),
            "--input".into(),
            format!("{CANARINO}={mancante}"),
            "--output".into(),
            cartella.percorso("o.arrow"),
        ],
        // Lo stesso input due volte, e un input mancante.
        vec![
            "--input".into(),
            format!("t={ingresso}"),
            "--input".into(),
            format!("t={ingresso}"),
            "--output".into(),
            cartella.percorso("o.arrow"),
        ],
        vec!["--output".into(), cartella.percorso("o.arrow")],
    ];
    for coda in casi {
        let mut argomenti = vec!["run".to_owned(), "--plan".to_owned(), piano.clone()];
        argomenti.extend(coda);
        let riferimenti: Vec<&str> = argomenti.iter().map(String::as_str).collect();
        errore(&invoca(&riferimenti), "invalid_configuration", 2);
    }
}

#[test]
fn run_con_piu_uscite_e_parquet() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.parquet");
    let piano = cartella.con_piano("p.json", PIANO_DUE_USCITE);
    let a = cartella.percorso("a.parquet");
    let b = cartella.percorso("b.arrows");
    let esito = invoca(&[
        "run",
        "--plan",
        &piano,
        "--input",
        &format!("t={ingresso}"),
        "--output",
        &format!("a={a}"),
        "--output",
        &format!("b={b}"),
        "--format",
        "json",
    ]);
    assert_eq!(esito.codice, 0, "{}", esito.stdout);
    let uscite = esito.documento["result"]["outputs"]
        .as_array()
        .expect("outputs");
    assert_eq!(uscite.len(), 2);
    assert_eq!(uscite[0]["content_type"], "application/vnd.apache.parquet");
    assert_eq!(
        uscite[1]["content_type"],
        "application/vnd.apache.arrow.stream"
    );
    assert_eq!(uscite[1]["rows"], 1);

    // Senza nome, `--output` vale solo per un piano con un solo output.
    let esito = invoca(&[
        "run",
        "--plan",
        &piano,
        "--input",
        &format!("t={ingresso}"),
        "--output",
        &cartella.percorso("x.arrow"),
    ]);
    errore(&esito, "invalid_configuration", 2);
}

#[test]
fn validate_rende_gli_schemi_d_uscita_senza_eseguire() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.arrow");
    let piano = cartella.con_piano("p.json", PIANO_DUE_USCITE);
    let esito = invoca(&[
        "validate",
        "--plan",
        &piano,
        "--input",
        &format!("t={ingresso}"),
    ]);
    assert_eq!(esito.codice, 0, "{}", esito.stdout);
    let risultato = &esito.documento["result"];
    assert_eq!(risultato["plan_version"], 1);
    assert_eq!(risultato["plan_format"], "plenora-data-plan-v1");
    assert_eq!(risultato["outputs"][0]["name"], "a");
    assert_eq!(risultato["outputs"][0]["columns"][0]["name"], "id");
    // L'identità del campo pubblicata con lo schema d'uscita.
    assert!(risultato["outputs"][0]["columns"][0]["field_id"].is_u64());
}

#[test]
fn piano_non_valido_e_operazione_non_eseguibile() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.arrow");
    let rotto = cartella.con_piano("rotto.json", r#"{"version": 1, "inputs": ["t"]}"#);
    let esito = invoca(&[
        "validate",
        "--plan",
        &rotto,
        "--input",
        &format!("t={ingresso}"),
    ]);
    errore(&esito, "invalid_plan", 2);

    let versione = cartella.con_piano(
        "v2.json",
        r#"{"version": 2, "inputs": ["t"], "steps": [], "outputs": ["t"]}"#,
    );
    let esito = invoca(&[
        "validate",
        "--plan",
        &versione,
        "--input",
        &format!("t={ingresso}"),
    ]);
    errore(&esito, "invalid_plan", 2);

    let trasposta = cartella.con_piano(
        "tr.json",
        r#"{"version": 1, "inputs": ["t"], "steps": [
            {"out": "x", "op": "table.transpose", "in": ["t"], "config": {"id_column": "nome"}}],
            "outputs": ["x"]}"#,
    );
    let esito = invoca(&[
        "validate",
        "--plan",
        &trasposta,
        "--input",
        &format!("t={ingresso}"),
    ]);
    errore(&esito, "unsupported", 3);
}

#[test]
fn diagnostica_per_riga_nell_errore_senza_valori() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.arrow");
    let piano = cartella.con_piano(
        "p.json",
        r#"{"version": 1, "inputs": ["t"], "steps": [
            {"out": "n", "op": "table.type_cast", "in": ["t"],
             "config": {"column": "nome", "target_type": "int", "errors": "raise"}}],
            "outputs": ["n"]}"#,
    );
    let uscita = cartella.percorso("n.arrow");
    let esito = invoca(&[
        "run",
        "--plan",
        &piano,
        "--input",
        &format!("t={ingresso}"),
        "--output",
        &uscita,
    ]);
    errore(&esito, "data_mapping", 3);
    let diagnostica = &esito.documento["error"]["details"]["row_diagnostics"];
    assert_eq!(diagnostica["contract"], "plenora-row-diagnostics-v1");
    assert_eq!(diagnostica["examples"][0]["source_index"], 1);
    assert!(!Path::new(&uscita).exists(), "nessun output dopo un errore");
}

#[test]
fn scadenza_passata_e_timeout_zero() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.arrow");
    let piano = cartella.con_piano("p.json", PIANO_FILTRO);
    let uscita = cartella.percorso("o.arrow");
    for controllo in [
        ["--deadline", "2000-01-01T00:00:00Z"],
        ["--timeout-ms", "0"],
    ] {
        let esito = invoca(&[
            "run",
            "--plan",
            &piano,
            "--input",
            &format!("t={ingresso}"),
            "--output",
            &uscita,
            controllo[0],
            controllo[1],
        ]);
        errore(&esito, "timeout", 5);
        assert_eq!(
            esito.documento["error"]["code"],
            "EXECUTION_DEADLINE_EXCEEDED"
        );
        assert_eq!(esito.documento["error"]["remote_effect"], "none");
        assert_eq!(esito.documento["error"]["phase"], "read");
        assert!(!Path::new(&uscita).exists());
        let esito = invoca(&["describe", "--input", &ingresso, controllo[0], controllo[1]]);
        errore(&esito, "timeout", 5);
    }
    // Una scadenza lontana non interrompe.
    let esito = invoca(&[
        "run",
        "--plan",
        &piano,
        "--input",
        &format!("t={ingresso}"),
        "--output",
        &uscita,
        "--deadline",
        "2999-01-01T00:00:00+01:00",
    ]);
    assert_eq!(esito.codice, 0, "{}", esito.stdout);
    let esito = invoca(&["describe", "--input", &ingresso, "--deadline", "domani"]);
    errore(&esito, "invalid_configuration", 2);
}

#[test]
fn destinazione_esistente_e_sovrascrittura_esplicita() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.arrow");
    let piano = cartella.con_piano("p.json", PIANO_FILTRO);
    let uscita = cartella.con_tabella("o.arrow");
    let argomenti = [
        "run",
        "--plan",
        &piano,
        "--input",
        &format!("t={ingresso}"),
        "--output",
        &uscita,
    ];
    let esito = invoca(&argomenti);
    errore(&esito, "conflict", 5);
    let mut con_sovrascrittura = argomenti.to_vec();
    con_sovrascrittura.push("--overwrite");
    assert_eq!(invoca(&con_sovrascrittura).codice, 0);
}

#[test]
fn file_mancanti_senza_percorsi_nei_messaggi() {
    let cartella = Cartella::nuova();
    let mancante = cartella.percorso("manca.arrow");
    let esito = invoca(&["describe", "--input", &mancante]);
    errore(&esito, "not_found", 5);
    assert_eq!(esito.documento["error"]["phase"], "read");
    let piano = cartella.percorso("manca.json");
    let esito = invoca(&["validate", "--plan", &piano]);
    errore(&esito, "not_found", 5);
    let piano = cartella.con_piano("p.json", PIANO_FILTRO);
    let esito = invoca(&[
        "validate",
        "--plan",
        &piano,
        "--input",
        &format!("t={mancante}"),
    ]);
    errore(&esito, "not_found", 5);
    let esito = invoca(&["describe", "--input", &cartella.con_piano("x.csv", "a,b\n")]);
    errore(&esito, "unsupported", 3);
}

#[test]
fn stesso_ingresso_stessi_byte() {
    let cartella = Cartella::nuova();
    let ingresso = cartella.con_tabella("t.arrow");
    let piano = cartella.con_piano("p.json", PIANO_DUE_USCITE);
    let mut raccolti = Vec::new();
    for giro in 0..2 {
        let a = cartella.percorso(&format!("a{giro}.arrow"));
        let b = cartella.percorso(&format!("b{giro}.parquet"));
        let esito = invoca(&[
            "run",
            "--plan",
            &piano,
            "--input",
            &format!("t={ingresso}"),
            "--output",
            &format!("a={a}"),
            "--output",
            &format!("b={b}"),
        ]);
        assert_eq!(esito.codice, 0, "{}", esito.stdout);
        raccolti.push((
            esito.stdout,
            std::fs::read(&a).expect("a"),
            std::fs::read(&b).expect("b"),
        ));
    }
    assert_eq!(raccolti[0], raccolti[1]);
}
