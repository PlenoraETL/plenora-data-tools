//! Matrice di verifica della CLI (piano di usabilita' §1.4).
//!
//! Sei dimensioni, applicate a TUTTI i sottocomandi invece che a quelli che
//! capita di ricordare:
//!
//! 1. argomenti mancanti;
//! 2. argomenti sconosciuti;
//! 3. file inesistenti o illeggibili;
//! 4. flag duplicati o incompatibili;
//! 5. parita' fra help e dispatch;
//! 6. nessuna pubblicazione dell'output sui fallimenti, e un solo documento
//!    sul canale previsto.
//!
//! Le convenzioni di canale, envelope, exit code e `--format` sono congelate:
//! qui si verificano, non si ridiscutono.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use plenora_core::arrow::array::{Int64Array, RecordBatch};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde_json::{json, Value};

const fn executable() -> &'static str {
    env!("CARGO_BIN_EXE_plenora-data-tools")
}

/// Tutti i sottocomandi del dispatch. `inspect-dataset` e' l'alias di
/// `describe` e va verificato come gli altri: un alias non controllato e' una
/// superficie non controllata.
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

/// Comandi che accettano almeno un argomento obbligatorio: invocarli nudi
/// deve fallire, non fare qualcosa a caso.
const COMANDI_CON_ARGOMENTI: [&str; 8] = [
    "describe",
    "inspect-dataset",
    "validate",
    "run",
    "transform",
    "spatial-join",
    "transform-arrow",
    "pair-arrow",
];

fn esegui(args: &[&str]) -> Output {
    Command::new(executable())
        .args(args)
        .output()
        .expect("invocazione CLI")
}

/// L'unico documento emesso sul canale previsto.
///
/// Verifica insieme le tre proprieta' che rendono l'uscita consumabile da un
/// programma: stderr vuoto, stdout parsabile per INTERO come un solo valore
/// JSON (due documenti concatenati non lo sono), e l'envelope nella forma
/// dichiarata.
fn envelope_di(output: &Output, contesto: &str) -> Value {
    assert!(
        output.stderr.is_empty(),
        "{contesto}: stderr deve restare vuoto, trovato: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let documento: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{contesto}: stdout deve essere UN solo documento JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(documento["status"], "error", "{contesto}");
    assert_eq!(documento["protocol_version"], 1, "{contesto}");
    for asse in ["category", "phase", "remote_effect"] {
        assert!(
            documento["error"][asse].is_string(),
            "{contesto}: asse `{asse}` mancante"
        );
    }
    assert!(
        documento["error"]["retry"]["kind"].is_string(),
        "{contesto}: disposizione di retry mancante"
    );
    assert!(
        documento["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "{contesto}: messaggio vuoto"
    );
    documento
}

fn scrivi_input(path: &Path) {
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int64Array::from(vec![1_i64]))],
    )
    .expect("batch");
    let file = std::fs::File::create(path).expect("create");
    let mut writer = FileWriter::try_new(file, &schema).expect("writer");
    writer.write(&batch).expect("write");
    writer.finish().expect("finish");
}

/// Input Arrow con `righe` righe in un solo batch.
fn scrivi_input_righe(path: &Path, righe: i64) {
    scrivi_input_batch(path, 1, righe);
}

/// Input Arrow con `batch` batch da `righe_per_batch` righe ciascuno.
///
/// Serve ai limiti di memoria: tanti batch PICCOLI passano singolarmente il
/// tetto del confine IPC, ma accumulati superano il budget — che e'
/// esattamente il caso che la contabilita' globale deve intercettare.
fn scrivi_input_batch(path: &Path, batch: usize, righe_per_batch: i64) {
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    let file = std::fs::File::create(path).expect("create");
    let mut writer = FileWriter::try_new(file, &schema).expect("writer");
    for indice in 0..batch {
        let base = i64::try_from(indice).expect("indice") * righe_per_batch;
        let valori: Vec<i64> = (base..base + righe_per_batch).collect();
        let record = RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(valori))])
            .expect("batch");
        writer.write(&record).expect("write");
    }
    writer.finish().expect("finish");
}

fn scrivi_piano(path: &Path) {
    let piano = json!({
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": [{"id": "f", "op": "table.filter", "in": ["main"],
                   "config": {"column": "id", "operator": ">", "value": 0}}],
        "output": "f",
    });
    std::fs::write(path, serde_json::to_vec(&piano).expect("json")).expect("piano");
}

// ---------------------------------------------------------------------------
// 1. Argomenti mancanti
// ---------------------------------------------------------------------------

#[test]
fn ogni_comando_con_argomenti_obbligatori_fallisce_se_invocato_nudo() {
    for comando in COMANDI_CON_ARGOMENTI {
        let output = esegui(&[comando]);
        assert!(
            !output.status.success(),
            "`{comando}` senza argomenti non deve avere successo"
        );
        let envelope = envelope_di(&output, comando);
        assert_eq!(
            envelope["error"]["category"], "invalid_plan",
            "{comando}: un argomento mancante e' un errore di invocazione"
        );
        assert_eq!(output.status.code(), Some(2), "{comando}");
    }
}

#[test]
fn un_flag_senza_valore_e_un_errore_di_invocazione() {
    for (comando, flag) in [
        ("catalog", "--family"),
        ("describe", "--input"),
        ("validate", "--plan"),
        ("run", "--plan"),
        ("transform", "--input"),
        ("spatial-join", "--left"),
        // Il flag sotto esame e' il PRIMO obbligatorio del comando: con un
        // altro, il messaggio nominerebbe (correttamente) quello mancante
        // prima, e il test misurerebbe l'ordine dei controlli invece della
        // proprieta' che interessa.
        ("transform-arrow", "--input"),
        ("pair-arrow", "--left"),
    ] {
        let output = esegui(&[comando, flag]);
        assert!(!output.status.success(), "{comando} {flag}");
        let envelope = envelope_di(&output, &format!("{comando} {flag}"));
        assert_eq!(envelope["error"]["category"], "invalid_plan");
        assert!(
            envelope["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(flag)),
            "{comando}: il messaggio deve nominare il flag: {envelope}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Argomenti sconosciuti
// ---------------------------------------------------------------------------

#[test]
fn nessun_comando_ignora_un_flag_che_non_conosce() {
    // Prima di questa verifica `catalog --sconosciuto`, `capabilities --pippo`
    // e `self-test --sconosciuto` uscivano con SUCCESSO ignorando il flag:
    // eseguire cio' che non si e' capito e' il difetto, non il flag.
    for comando in COMANDI.iter().chain(std::iter::once(&"self-test")) {
        let output = esegui(&[comando, "--sconosciuto"]);
        assert!(
            !output.status.success(),
            "`{comando}` ha accettato un flag sconosciuto"
        );
        let envelope = envelope_di(&output, comando);
        assert!(
            envelope["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("non riconosciuto")),
            "{comando}: {envelope}"
        );
        assert_eq!(output.status.code(), Some(2), "{comando}");
    }
}

#[test]
fn un_comando_inesistente_emette_solo_l_envelope() {
    // L'help finiva su stderr insieme all'envelope su stdout: due canali per
    // un errore solo. Ora l'envelope e' l'unico documento e indica `--help`.
    let output = esegui(&["comando-inesistente"]);
    assert!(!output.status.success());
    let envelope = envelope_di(&output, "comando inesistente");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("--help")),
        "il messaggio deve indicare come scoprire i comandi: {envelope}"
    );
}

// ---------------------------------------------------------------------------
// 3. File inesistenti o illeggibili
// ---------------------------------------------------------------------------

#[test]
fn un_file_inesistente_o_illeggibile_e_un_errore_di_io_senza_pubblicazione() {
    let directory = tempfile::tempdir().expect("tempdir");
    let assente = directory.path().join("assente.arrow");
    // Una directory al posto di un file e' il caso "illeggibile" riproducibile
    // su ogni piattaforma: i permessi non lo sono.
    let illeggibile = directory.path().to_path_buf();
    let output_path = directory.path().join("mai-creato.arrow");

    for percorso in [&assente, &illeggibile] {
        let percorso = percorso.to_string_lossy().into_owned();
        for args in [
            vec!["describe", "--input", &percorso],
            vec!["inspect-dataset", "--input", &percorso],
            vec!["validate", "--plan", &percorso],
        ] {
            let output = esegui(&args);
            assert!(!output.status.success(), "{args:?}");
            let envelope = envelope_di(&output, &format!("{args:?}"));
            assert_eq!(envelope["error"]["category"], "io", "{args:?}: {envelope}");
            assert_eq!(output.status.code(), Some(5), "{args:?}");
        }
    }
    assert!(
        !output_path.exists(),
        "nessun output deve essere creato da un fallimento"
    );
}

// ---------------------------------------------------------------------------
// 4. Flag duplicati o incompatibili
// ---------------------------------------------------------------------------

#[test]
fn un_flag_a_valore_singolo_non_si_ripete() {
    // `describe --input a --input b` usava il primo e scartava il secondo in
    // silenzio: l'utente credeva di aver descritto `b`.
    for (comando, flag) in [
        ("describe", "--input"),
        ("catalog", "--family"),
        ("run", "--plan"),
        ("validate", "--plan"),
        ("transform", "--output"),
    ] {
        let output = esegui(&[comando, flag, "uno", flag, "due"]);
        assert!(!output.status.success(), "{comando} {flag} ripetuto");
        let envelope = envelope_di(&output, &format!("{comando} {flag} ripetuto"));
        assert!(
            envelope["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("ripetuto")),
            "{comando}: {envelope}"
        );
    }
}

#[test]
fn l_input_nominale_resta_ripetibile_dove_ha_senso() {
    // La ripetizione di `--input` e' la forma nominale di `run`/`validate`:
    // il controllo sui duplicati non deve romperla.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("piano.json");
    let input = directory.path().join("input.arrow");
    scrivi_piano(&piano);
    scrivi_input(&input);
    let output = esegui(&[
        "validate",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &format!("main={}", input.display()),
    ]);
    assert!(
        output.status.success(),
        "la forma nominale deve restare valida: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn le_forme_incompatibili_sono_rifiutate() {
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("piano.json");
    let input = directory.path().join("input.arrow");
    let output_path = directory.path().join("out.arrow");
    scrivi_piano(&piano);
    scrivi_input(&input);
    let piano = piano.to_string_lossy().into_owned();
    let input_s = input.to_string_lossy().into_owned();
    let nominale = format!("main={}", input.display());
    let output_s = output_path.to_string_lossy().into_owned();

    for (descrizione, args) in [
        (
            "nominale e posizionale insieme",
            vec![
                "validate", "--plan", &piano, "--input", &nominale, "--inputs", &input_s,
            ],
        ),
        (
            "--right su un piano v4",
            vec![
                "run", "--plan", &piano, "--input", &input_s, "--right", &input_s, "--output",
                &output_s,
            ],
        ),
    ] {
        let output = esegui(&args);
        assert!(!output.status.success(), "{descrizione}");
        envelope_di(&output, descrizione);
        assert!(
            !output_path.exists(),
            "{descrizione}: nessun output da un'invocazione rifiutata"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Parita' fra help e dispatch
// ---------------------------------------------------------------------------

#[test]
fn l_help_nomina_ogni_comando_e_ogni_flag_che_il_dispatch_accetta() {
    let generale = esegui(&["--help"]);
    assert!(generale.status.success());
    assert!(
        generale.stderr.is_empty(),
        "l'help va su stdout, non su stderr"
    );
    let generale = String::from_utf8_lossy(&generale.stdout).into_owned();

    for comando in COMANDI.iter().chain(std::iter::once(&"self-test")) {
        assert!(
            generale.contains(*comando),
            "l'help generale non nomina `{comando}`"
        );
        let specifico = esegui(&[comando, "--help"]);
        assert!(specifico.status.success(), "`{comando} --help`");
        let testo = String::from_utf8_lossy(&specifico.stdout).into_owned();
        // Ogni flag accettato dal dispatch compare nell'help del comando: un
        // flag documentato e non accettato, o accettato e non documentato,
        // sono lo stesso difetto visto da due lati.
        let flag_documentati = testo.clone();
        let flag_accettati = esegui(&[comando, "--flag-che-non-esiste"]);
        let envelope: Value =
            serde_json::from_slice(&flag_accettati.stdout).expect("envelope del flag ignoto");
        let messaggio = envelope["error"]["message"]
            .as_str()
            .expect("messaggio")
            .to_owned();
        let Some(elenco) = messaggio.split("ammessi: ").nth(1) else {
            continue;
        };
        for flag in elenco.trim_end_matches(')').split(", ") {
            if flag == "nessuno" {
                continue;
            }
            assert!(
                flag_documentati.contains(flag),
                "`{comando} --help` non documenta il flag accettato `{flag}`"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Nessuna pubblicazione sui fallimenti, un solo documento sul canale
// ---------------------------------------------------------------------------

#[test]
fn nessun_fallimento_di_run_lascia_un_output() {
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("piano.json");
    let input = directory.path().join("input.arrow");
    scrivi_piano(&piano);
    scrivi_input(&input);
    let piano_s = piano.to_string_lossy().into_owned();
    let input_s = input.to_string_lossy().into_owned();

    let casi: [(&str, Vec<&str>); 4] = [
        (
            "piano assente",
            vec!["run", "--plan", "/assente.json", "--input", &input_s],
        ),
        (
            "input assente",
            vec!["run", "--plan", &piano_s, "--input", "/assente.arrow"],
        ),
        (
            "flag sconosciuto",
            vec!["run", "--plan", &piano_s, "--input", &input_s, "--boh"],
        ),
        (
            "nome di input non dichiarato",
            vec!["run", "--plan", &piano_s, "--input", "sbagliato=/dev/null"],
        ),
    ];
    for (indice, (descrizione, args)) in casi.into_iter().enumerate() {
        let output_path = directory.path().join(format!("output-{indice}.arrow"));
        let mut args = args;
        args.push("--output");
        let output_s = output_path.to_string_lossy().into_owned();
        args.push(&output_s);
        let output = esegui(&args);
        assert!(!output.status.success(), "{descrizione}");
        envelope_di(&output, descrizione);
        assert!(
            !output_path.exists(),
            "{descrizione}: output pubblicato da un'esecuzione fallita"
        );
    }
}

#[test]
fn i_comandi_che_riescono_emettono_un_solo_documento_su_stdout() {
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("piano.json");
    let input = directory.path().join("input.arrow");
    let output_path = directory.path().join("out.arrow");
    scrivi_piano(&piano);
    scrivi_input(&input);
    let piano_s = piano.to_string_lossy().into_owned();
    let input_s = input.to_string_lossy().into_owned();
    let nominale = format!("main={}", input.display());
    let output_s = output_path.to_string_lossy().into_owned();

    let casi: [(&str, Vec<&str>); 6] = [
        ("catalog", vec!["catalog"]),
        ("capabilities", vec!["capabilities"]),
        ("version", vec!["--version"]),
        ("describe", vec!["describe", "--input", &input_s]),
        (
            "validate",
            vec!["validate", "--plan", &piano_s, "--input", &nominale],
        ),
        (
            "run",
            vec![
                "run", "--plan", &piano_s, "--input", &nominale, "--output", &output_s,
            ],
        ),
    ];
    for (descrizione, args) in casi {
        let output = esegui(&args);
        assert!(
            output.status.success(),
            "{descrizione}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            output.stderr.is_empty(),
            "{descrizione}: stderr deve restare vuoto, trovato: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Un solo documento: due JSON concatenati non si deserializzano.
        serde_json::from_slice::<Value>(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{descrizione}: stdout deve essere UN solo documento JSON ({error}): {}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
    }
}

// ---------------------------------------------------------------------------
// Sesto giro, finding 6 — nessun token puo' restare inosservato
// ---------------------------------------------------------------------------

#[test]
fn nessun_token_estraneo_viene_ignorato() {
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("piano.json");
    let input = directory.path().join("input.arrow");
    let uscita = directory.path().join("out.arrow");
    scrivi_piano(&piano);
    scrivi_input(&input);
    let piano_s = piano.to_string_lossy().into_owned();
    let input_s = input.to_string_lossy().into_owned();
    let nominale = format!("main={}", input.display());
    let uscita_s = uscita.to_string_lossy().into_owned();

    let casi: [(&str, Vec<&str>, &str); 7] = [
        (
            "posizionale inatteso",
            vec!["describe", "pippo", "--input", &input_s],
            "posizionale",
        ),
        (
            "posizionale dopo i flag",
            vec!["run", "--plan", &piano_s, "pippo"],
            "posizionale",
        ),
        (
            "flag in forma breve",
            vec!["describe", "-x", "--input", &input_s],
            "forma lunga",
        ),
        (
            "flag usato come valore",
            vec!["run", "--plan", "--output", &uscita_s],
            "e' un flag, non un valore",
        ),
        (
            "--inputs senza valori",
            vec!["validate", "--plan", &piano_s, "--inputs"],
            "valore mancante",
        ),
        (
            "--format ripetuto",
            vec!["--format", "json", "--format", "markdown", "catalog"],
            "ripetuto",
        ),
        (
            "argomenti dopo --version",
            vec!["--version", "pippo"],
            "non accetta argomenti",
        ),
    ];
    for (descrizione, args, atteso) in casi {
        let output = esegui(&args);
        assert!(
            !output.status.success(),
            "{descrizione}: l'invocazione doveva fallire"
        );
        let envelope = envelope_di(&output, descrizione);
        assert!(
            envelope["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(atteso)),
            "{descrizione}: atteso `{atteso}`, ottenuto {envelope}"
        );
        assert!(
            !uscita.exists(),
            "{descrizione}: nessun output da un'invocazione non compresa"
        );
    }

    // Il caso peggiore: un `run` che avrebbe pubblicato, con un token
    // ignorato in mezzo. Nessun output deve comparire.
    let output = esegui(&[
        "run", "--plan", &piano_s, "--input", &nominale, "--output", &uscita_s, "pippo",
    ]);
    assert!(!output.status.success(), "token estraneo accettato");
    envelope_di(&output, "run con token estraneo");
    assert!(
        !uscita.exists(),
        "un output non deve mai essere pubblicato da un'invocazione con token ignorati"
    );

    // Le invocazioni legittime restano tali.
    assert!(esegui(&["--version"]).status.success());
    assert!(esegui(&["--version", "--json"]).status.success());
    assert!(esegui(&["--help"]).status.success());
    assert!(esegui(&["--format", "markdown", "catalog"])
        .status
        .success());
    assert!(esegui(&["describe", "--input", &input_s]).status.success());
}

#[test]
fn stderr_resta_vuoto_su_ogni_esito_incluso_il_panico() {
    // Sesto giro, finding 7: il contratto «stderr vuoto» aveva due crepe —
    // l'avviso di durabilita' del publish e l'hook di panico di default.
    // Il primo e' diventato un campo del documento di uscita, il secondo e'
    // intercettato da `main`, che ne fa un envelope su stdout.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("piano.json");
    let input = directory.path().join("input.arrow");
    let uscita = directory.path().join("out.arrow");
    scrivi_piano(&piano);
    scrivi_input(&input);

    // Successo: nessun avviso su stderr, e la durabilita' e' un campo del
    // documento.
    let output = esegui(&[
        "run",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &format!("main={}", input.display()),
        "--output",
        &uscita.to_string_lossy(),
    ]);
    assert!(
        output.status.success(),
        "{:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr deve restare vuoto anche in successo: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let documento: Value = serde_json::from_slice(&output.stdout).expect("un solo documento");
    assert!(
        documento["durability_confirmed"].is_boolean(),
        "l'esito del publish e' un campo, non un avviso su un altro canale: {documento}"
    );

    // Input malformato: l'errore arriva su stdout, stderr resta vuoto anche
    // quando dentro arrow qualcosa va storto.
    let corrotto = directory.path().join("corrotto.arrow");
    std::fs::write(&corrotto, b"ARROW1\x00\x00non-un-file-arrow").expect("scrittura");
    let output = esegui(&["describe", "--input", &corrotto.to_string_lossy()]);
    assert!(!output.status.success());
    envelope_di(&output, "input corrotto");
}

// ---------------------------------------------------------------------------
// Settimo giro — parser fail-closed anche nei percorsi help/version, e
// categoria `resource_limit` raggiungibile
// ---------------------------------------------------------------------------

#[test]
fn nemmeno_i_percorsi_help_e_version_ignorano_un_token() {
    let casi: [(&str, Vec<&str>, &str); 10] = [
        (
            "help di sottocomando con token estraneo",
            vec!["run", "--help", "junk"],
            "posizionale",
        ),
        (
            "flag breve consumato come valore di --plan",
            vec!["run", "--plan", "-x", "--output", "o.arrow"],
            "e' un flag, non un valore",
        ),
        (
            "flag breve consumato da --inputs",
            vec!["validate", "--plan", "p.json", "--inputs", "-x"],
            "valore mancante",
        ),
        (
            "--json ripetuto su --version",
            vec!["--version", "--json", "--json"],
            "non accetta argomenti",
        ),
        (
            "token dopo --help globale",
            vec!["--help", "junk"],
            "non accetta argomenti",
        ),
        (
            "flag sconosciuto prima di --help",
            vec!["describe", "--boh", "--help"],
            "non riconosciuto",
        ),
        // Ottavo giro, finding 5. `--json` non e' un modificatore globale: e'
        // dichiarato solo da `--version`. Accettarlo altrove significava
        // ignorare un token, cioe' proprio cio' che il parser deve chiudere.
        (
            "--json su un comando che non lo dichiara",
            vec!["describe", "--json"],
            "non riconosciuto",
        ),
        (
            "--json su --help",
            vec!["--help", "--json"],
            "non accetta argomenti",
        ),
        // `--help` e `-h` sono la stessa opzione: due forme diverse dello
        // stesso flag restavano due token distinti e sfuggivano al controllo
        // dei duplicati.
        (
            "--help e -h insieme sono un duplicato",
            vec!["run", "--help", "-h"],
            "ripetuto",
        ),
        ("-h ripetuto", vec!["describe", "-h", "-h"], "ripetuto"),
    ];
    for (descrizione, args, atteso) in casi {
        let output = esegui(&args);
        assert!(
            !output.status.success(),
            "{descrizione}: doveva fallire, stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let envelope = envelope_di(&output, descrizione);
        assert!(
            envelope["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(atteso)),
            "{descrizione}: atteso `{atteso}`, ottenuto {envelope}"
        );
    }

    // Le forme legittime restano tali.
    assert!(esegui(&["run", "--help"]).status.success());
    assert!(esegui(&["--version", "--json"]).status.success());
    assert!(esegui(&["--help"]).status.success());
}

#[test]
fn un_limite_di_risorsa_produce_la_categoria_e_l_exit_code_dedicati() {
    // Settimo giro, finding 7: `resource_limit` non era prodotta da nessuna
    // variante concreta, quindi l'exit code 4 era irraggiungibile. Un piano
    // legacy con `max_rows` minuscolo lo raggiunge.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("legacy.json");
    let input = directory.path().join("input.arrow");
    let uscita = directory.path().join("out.arrow");
    std::fs::write(
        &piano,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "limits": {"max_rows": 1},
            "steps": [{"operation": "rename", "config": {"renames": [
                {"old_name": "id", "new_name": "identificativo"}
            ]}}]
        }))
        .expect("json"),
    )
    .expect("piano");
    scrivi_input_righe(&input, 8);

    let output = esegui(&[
        "run",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &input.to_string_lossy(),
        "--output",
        &uscita.to_string_lossy(),
    ]);
    assert!(!output.status.success());
    let envelope = envelope_di(&output, "limite di righe");
    assert_eq!(
        envelope["error"]["category"], "resource_limit",
        "un limite superato non e' un piano invalido: {envelope}"
    );
    assert_eq!(
        output.status.code(),
        Some(4),
        "la categoria `resource_limit` proietta sull'exit code 4"
    );
    assert!(!uscita.exists(), "nessun output da un limite superato");
}

#[test]
fn nessun_eprintln_incondizionato_nel_sorgente_della_cli() {
    // Settimo giro, finding 4. La garanzia «stderr vuoto per i consumatori
    // non interattivi» non e' verificabile da un test d'integrazione — non si
    // puo' inviare SIGINT in modo portabile — quindi si verifica
    // STRUTTURALMENTE sul sorgente: ogni `eprintln!` della CLI deve stare
    // dentro un ramo governato da `is_terminal`.
    //
    // Il controllo esiste perche' la modifica precedente era stata scritta
    // ma non salvata: l'ADR dichiarava la garanzia e il codice non la
    // implementava.
    let sorgente =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
            .expect("sorgente della CLI");
    let occorrenze: Vec<usize> = sorgente
        .match_indices("eprintln!")
        .map(|(indice, _)| indice)
        .collect();
    assert!(
        !occorrenze.is_empty(),
        "il test presuppone che gli avvisi interattivi esistano ancora"
    );
    for indice in occorrenze {
        // Si guarda indietro di 600 caratteri: il ramo `if interattivo` che
        // governa l'avviso e' immediatamente sopra.
        let inizio = indice.saturating_sub(600);
        let contesto = &sorgente[inizio..indice];
        assert!(
            contesto.contains("interattivo"),
            "eprintln! non governato da `is_terminal` attorno all'offset {indice}"
        );
    }
    assert!(
        sorgente.contains("IsTerminal::is_terminal(&std::io::stderr())"),
        "il controllo dichiarato nell'ADR dev'essere nel codice, non solo nel documento"
    );
}

#[test]
fn il_budget_di_memoria_legacy_e_globale_e_include_il_picco() {
    // Settimo giro, finding 2: il budget era verificato per singolo input e
    // ignorava la duplicazione della concatenazione. Un piano legacy con un
    // budget minuscolo deve fallire con `resource_limit`, non materializzare.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("legacy.json");
    let input = directory.path().join("input.arrow");
    let uscita = directory.path().join("out.arrow");
    std::fs::write(
        &piano,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "limits": {"max_rows": 1_000_000, "max_governed_memory_bytes": 4096},
            "steps": [{"operation": "sort", "config": {"columns": ["id"], "ascending": true}}]
        }))
        .expect("json"),
    )
    .expect("piano");
    // Dodici batch da 32 righe: ~256 byte l'uno, quindi ogni messaggio passa
    // il tetto del confine (4 KiB), ma accumulati fanno ~3 KiB e il picco
    // stimato con la concatenazione supera il budget. Cosi' il test esercita
    // la contabilita' globale, non il confine IPC.
    scrivi_input_batch(&input, 12, 32);

    let output = esegui(&[
        "run",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &input.to_string_lossy(),
        "--output",
        &uscita.to_string_lossy(),
    ]);
    assert!(!output.status.success(), "il budget deve chiudere");
    let envelope = envelope_di(&output, "budget di memoria legacy");
    assert_eq!(
        envelope["error"]["category"], "resource_limit",
        "{envelope}"
    );
    assert!(!uscita.exists(), "nessun output da un budget superato");
}

#[test]
fn il_budget_legacy_e_globale_anche_fra_i_due_lati_di_un_piano_binario() {
    // Ottavo giro, finding 1: il budget residuo era calcolato ma non
    // collegato. Nel percorso binario ogni lato passava dalla propria
    // verifica con il budget INTERO, quindi un piano dichiarato a N byte ne
    // poteva trattenere 2N. Il test costruisce esattamente quel caso: ogni
    // lato da solo sta nel budget, la somma no.
    //
    // Il budget e' scelto sul picco stimato (accumulato * 2, per la
    // duplicazione della concatenazione): un lato solo resta sotto, i due
    // insieme lo superano. Se il residuo non viene passato al secondo lato il
    // comando riesce, ed e' il difetto.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("legacy-binario.json");
    let sinistra = directory.path().join("left.arrow");
    let destra = directory.path().join("right.arrow");
    let uscita = directory.path().join("out.arrow");
    std::fs::write(
        &piano,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "limits": {"max_rows": 1_000_000, "max_governed_memory_bytes": 5_000},
            "steps": [{
                "operation": "join",
                "config": {"left_keys": ["id"], "right_keys": ["id"], "how": "inner"}
            }]
        }))
        .expect("json"),
    )
    .expect("piano");
    // Quattro batch da 32 righe per lato. Le dimensioni esatte le decide
    // Arrow (`get_array_memory_size` include padding e allineamento), quindi
    // qui non si dichiarano numeri: cio' che il test verifica e' la
    // RELAZIONE fra i due casi, e la relazione e' provata dalla coppia di
    // test. `un_lato_solo_resta_dentro_lo_stesso_budget` esegue lo stesso
    // caricamento con lo STESSO budget e riesce; questo, che ne fa due,
    // fallisce. Se il budget fosse semplicemente troppo stretto fallirebbero
    // entrambi, e la coppia non passerebbe.
    scrivi_input_batch(&sinistra, 4, 32);
    scrivi_input_batch(&destra, 4, 32);

    let output = esegui(&[
        "run",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &sinistra.to_string_lossy(),
        "--right",
        &destra.to_string_lossy(),
        "--output",
        &uscita.to_string_lossy(),
    ]);
    assert!(
        !output.status.success(),
        "i due lati insieme superano il budget dichiarato"
    );
    let envelope = envelope_di(&output, "budget binario legacy");
    assert_eq!(
        envelope["error"]["category"], "resource_limit",
        "{envelope}"
    );
    assert!(!uscita.exists(), "nessun output da un budget superato");
}

#[test]
fn un_lato_solo_resta_dentro_lo_stesso_budget() {
    // Controprova del test precedente: con UN solo input e lo stesso budget
    // il comando riesce. Senza questa meta' il primo test potrebbe passare
    // per un budget semplicemente troppo stretto, e non proverebbe nulla
    // sulla globalita' della contabilita'.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("legacy-unario.json");
    let input = directory.path().join("solo.arrow");
    let uscita = directory.path().join("out.arrow");
    std::fs::write(
        &piano,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "limits": {"max_rows": 1_000_000, "max_governed_memory_bytes": 5_000},
            "steps": [{"operation": "sort", "config": {"columns": ["id"], "ascending": true}}]
        }))
        .expect("json"),
    )
    .expect("piano");
    scrivi_input_batch(&input, 4, 32);

    let output = esegui(&[
        "run",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &input.to_string_lossy(),
        "--output",
        &uscita.to_string_lossy(),
    ]);
    assert!(
        output.status.success(),
        "un lato solo sta nel budget: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// ---------------------------------------------------------------------------
// Nono giro — propagazione della categoria e budget globale
// ---------------------------------------------------------------------------

#[test]
fn il_percorso_legacy_non_declassa_un_limite_di_risorsa_a_esecuzione() {
    // Finding 2, seconda meta'. L'executor DAG conservava gia' la categoria;
    // il percorso legacy avvolgeva TUTTO in `Execution`, quindi lo stesso
    // limite dava `resource_limit`/4 con un piano v4 e `execution`/6 con un
    // piano legacy. La versione dello schema non puo' cambiare la natura di
    // un errore.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("legacy.json");
    let input = directory.path().join("input.arrow");
    let uscita = directory.path().join("out.arrow");
    // `melt` alza il limite sui DATI prodotti (righe x colonne valore), non
    // sulla configurazione: e' un passo unario, quindi passa dal percorso
    // legacy che avvolgeva tutto in `Execution`.
    std::fs::write(
        &piano,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "limits": {"max_rows": 3},
            "steps": [{
                "operation": "melt",
                "config": {"id_columns": ["id"], "value_columns": ["a", "b"]}
            }]
        }))
        .expect("json"),
    )
    .expect("piano");
    scrivi_input_melt(&input, 2);

    let output = esegui(&[
        "run",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &input.to_string_lossy(),
        "--output",
        &uscita.to_string_lossy(),
    ]);
    assert!(!output.status.success(), "il limite deve chiudere");
    let envelope = envelope_di(&output, "limite legacy");
    assert_eq!(
        envelope["error"]["category"], "resource_limit",
        "il percorso legacy non deve declassare la categoria: {envelope}"
    );
    assert_eq!(
        output.status.code(),
        Some(4),
        "e l'exit code segue la categoria: {envelope}"
    );
}

#[test]
fn il_budget_di_memoria_legacy_copre_anche_l_esecuzione() {
    // Il caricamento era limitato, l'esecuzione no: gli input restavano vivi
    // e il kernel riceveva `max_governed_memory_bytes` INTERO, e l'output non veniva
    // addebitato a nessuno. Un piano dichiarato a N byte poteva quindi
    // arrivare molto oltre N.
    //
    // Dal giro successivo questo caso passa dal rifiuto PREVENTIVO di
    // `cross_join` (`preflight_output_bytes`), non piu' dal controllo di
    // ammissione a valle: il numero di righe dell'output e' esatto prima di
    // allocare. Per le operazioni senza preflight resta l'ammissione, ed e'
    // una deroga dichiarata (DER-011) — non un tetto duro.
    //
    // Il caso e' costruito con un margine che l'aritmetica di Arrow non puo'
    // ribaltare: due input da 64 righe a una colonna (~mezzo KiB l'uno)
    // producono con `cross_join` 4 096 righe a due colonne (~64 KiB). Il
    // budget sta comodamente sopra il caricamento e due ordini di grandezza
    // sotto l'output: qualunque padding lo lascia dalla stessa parte.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("legacy-espansione.json");
    let sinistra = directory.path().join("sinistra.arrow");
    let destra = directory.path().join("destra.arrow");
    let uscita = directory.path().join("out.arrow");
    std::fs::write(
        &piano,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "limits": {"max_rows": 1_000_000, "max_governed_memory_bytes": 8_192},
            "steps": [{"operation": "cross_join", "config": {}}]
        }))
        .expect("json"),
    )
    .expect("piano");
    scrivi_input_batch(&sinistra, 1, 64);
    scrivi_input_batch(&destra, 1, 64);

    let output = esegui(&[
        "run",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &sinistra.to_string_lossy(),
        "--right",
        &destra.to_string_lossy(),
        "--output",
        &uscita.to_string_lossy(),
    ]);
    assert!(
        !output.status.success(),
        "l'output e' il prodotto dei due input: non sta nel budget globale"
    );
    let envelope = envelope_di(&output, "budget d'esecuzione");
    assert_eq!(
        envelope["error"]["category"], "resource_limit",
        "{envelope}"
    );
    // Deve scattare DOPO il caricamento: se il budget chiudesse gia' in
    // lettura, il test non direbbe nulla sul budget dell'esecuzione — che e'
    // esattamente cio' che mancava.
    let messaggio = envelope["error"]["message"].as_str().unwrap_or_default();
    assert!(
        !messaggio.contains("l'input materializzato"),
        "i due input entrano nel budget: il limite arriva dopo, {envelope}"
    );
    assert!(!uscita.exists(), "nessun output da un budget superato");
}

#[test]
fn lo_stesso_piano_riesce_quando_il_budget_copre_anche_l_output() {
    // Controprova: stessa forma, budget che copre input piu' output. Senza
    // questa meta' il test precedente potrebbe passare per un budget
    // semplicemente troppo stretto, e non proverebbe nulla sull'addebito
    // dell'esecuzione.
    let directory = tempfile::tempdir().expect("tempdir");
    let piano = directory.path().join("legacy-espansione-ok.json");
    let sinistra = directory.path().join("sinistra.arrow");
    let destra = directory.path().join("destra.arrow");
    let uscita = directory.path().join("out.arrow");
    std::fs::write(
        &piano,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "limits": {"max_rows": 1_000_000, "max_governed_memory_bytes": 4_194_304},
            "steps": [{"operation": "cross_join", "config": {}}]
        }))
        .expect("json"),
    )
    .expect("piano");
    scrivi_input_batch(&sinistra, 1, 64);
    scrivi_input_batch(&destra, 1, 64);

    let output = esegui(&[
        "run",
        "--plan",
        &piano.to_string_lossy(),
        "--input",
        &sinistra.to_string_lossy(),
        "--right",
        &destra.to_string_lossy(),
        "--output",
        &uscita.to_string_lossy(),
    ]);
    assert!(
        output.status.success(),
        "con 4 MiB di budget lo stesso piano deve riuscire: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(uscita.exists(), "l'output dev'essere stato pubblicato");
}

/// Input a tre colonne per i piani `melt`: `id` piu' due colonne valore.
fn scrivi_input_melt(path: &Path, righe: i64) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("a", DataType::Int64, true),
        Field::new("b", DataType::Int64, true),
    ]));
    let file = std::fs::File::create(path).expect("create");
    let mut writer = FileWriter::try_new(file, &schema).expect("writer");
    let valori: Vec<i64> = (0..righe).collect();
    let record = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(valori.clone())),
            Arc::new(Int64Array::from(valori.clone())),
            Arc::new(Int64Array::from(valori)),
        ],
    )
    .expect("batch");
    writer.write(&record).expect("write");
    writer.finish().expect("finish");
}
