//! Oracolo della superficie CLI: i **byte esatti** di stdout, stderr ed exit
//! code per una matrice di invocazioni deterministiche.
//!
//! # Perche' esiste
//!
//! [`matrice_cli`](../matrice_cli.rs) verifica le **proprieta'** della CLI —
//! exit code, forma dell'envelope, canale, parita' fra help e dispatch — ed e'
//! la difesa giusta per quelle. Nessuna di esse pero' guarda il testo: l'help
//! puo' cambiare ordine delle voci, un envelope puo' perdere un campo che
//! nessun test nomina, una tabella puo' cambiare allineamento, e la matrice
//! resta verde.
//!
//! Questo oracolo serve al refactor strutturale, il cui criterio di uscita per
//! la CLI e' «output byte-identico e stessi exit code»
//! ([`stato-e-roadmap.md`](../../../../docs/stato-e-roadmap.md)). Uno
//! spostamento meccanico di codice non deve cambiare un solo byte di cio' che
//! l'utente vede: qui quel criterio smette di essere un'affermazione e diventa
//! una verifica.
//!
//! # Che cosa NON copre
//!
//! Solo le invocazioni **deterministiche e senza stato**: help, versione,
//! capability, catalogo, e gli errori di invocazione. Tutto cio' che dipende
//! da file, tempi o percorsi assoluti resta fuori — quelle superfici sono
//! coperte dai test che le sanno normalizzare.
//!
//! # Rigenerazione
//!
//! Dopo un cambiamento **intenzionale** della superficie:
//!
//! ```sh
//! PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-cli --test oracolo_superficie_cli
//! ```
//!
//! e commit dell'oracolo insieme alla modifica, cosi' il diff mostra in review
//! che cosa vede l'utente di diverso.

use std::process::Command;

use serde_json::{json, Value};

const fn eseguibile() -> &'static str {
    env!("CARGO_BIN_EXE_plenora-data-tools")
}

const ORACOLO_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/oracolo_superficie_cli.snap"
);

/// Le invocazioni catturate.
///
/// Coprono le tre superfici che un refactor puo' scalfire senza accorgersene:
/// il testo di help (generale e per comando), i documenti informativi
/// (versione, capability, catalogo) e gli envelope di errore di invocazione,
/// che sono il contratto verso chi automatizza.
const INVOCAZIONI: &[&[&str]] = &[
    // Superficie di aiuto. La forma per comando e' `<comando> --help`, non
    // `help <comando>`: la prima stesura di questo oracolo usava la seconda e
    // catturava dodici volte lo stesso envelope di comando sconosciuto, cioe'
    // sorvegliava il nulla credendo di sorvegliare l'aiuto.
    &[],
    &["--help"],
    &["-h"],
    &["catalog", "--help"],
    &["describe", "--help"],
    &["describe", "-h"],
    &["inspect-dataset", "--help"],
    &["validate", "--help"],
    &["run", "--help"],
    &["capabilities", "--help"],
    &["transform", "--help"],
    &["spatial-join", "--help"],
    &["transform-arrow", "--help"],
    &["pair-arrow", "--help"],
    // Forme che NON sono comandi: restano fissate perche' cambiarle sarebbe
    // una rottura per chi le ha gia' incontrate.
    &["help"],
    &["version"],
    // Documenti informativi: deterministici, senza filesystem.
    &["--version"],
    &["-V"],
    &["capabilities"],
    &["catalog"],
    // Errori di invocazione: l'envelope e' il contratto verso chi automatizza.
    &["comando-inesistente"],
    &["catalog", "--flag-che-non-esiste"],
    &["describe"],
    &["validate"],
    &["run"],
    &["transform"],
    &["spatial-join"],
    &["transform-arrow"],
    &["pair-arrow"],
];

/// Rende il testo indipendente dalla macchina.
///
/// L'unica variabilita' attesa e' il percorso dell'eseguibile, che cargo mette
/// in una directory di build diversa a ogni configurazione. Se un giorno
/// comparisse dell'altra variabilita', l'oracolo fallirebbe in modo instabile
/// invece di nasconderla — ed e' il comportamento voluto: un'instabilita' e'
/// una notizia, non un fastidio da sopprimere.
fn normalizza(testo: &str) -> String {
    testo.replace(eseguibile(), "<eseguibile>")
}

fn cattura(args: &[&str]) -> Value {
    let esito = Command::new(eseguibile())
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("invocazione CLI {args:?}: {error}"));
    json!({
        "argomenti": args,
        "exit_code": esito.status.code(),
        "stdout": normalizza(&String::from_utf8_lossy(&esito.stdout)),
        "stderr": normalizza(&String::from_utf8_lossy(&esito.stderr)),
    })
}

fn oracolo_content() -> String {
    let voci: Vec<Value> = INVOCAZIONI.iter().map(|args| cattura(args)).collect();
    let mut content = serde_json::to_string_pretty(&voci).expect("la serializzazione non fallisce");
    content.push('\n');
    content
}

/// La superficie CLI deve coincidere con l'oracolo committato, byte per byte.
#[test]
fn la_superficie_cli_coincide_con_l_oracolo_committato() {
    let attuale = oracolo_content();
    let path = std::path::Path::new(ORACOLO_PATH);
    if std::env::var_os("PLENORA_UPDATE_SNAPSHOT").is_some() {
        std::fs::write(path, &attuale).expect("rigenerazione dell'oracolo della superficie");
        eprintln!("oracolo della superficie CLI rigenerato in {ORACOLO_PATH}");
        return;
    }
    let atteso = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("oracolo non leggibile ({error}): generarlo con PLENORA_UPDATE_SNAPSHOT=1")
    });
    let atteso = atteso.replace("\r\n", "\n");
    assert!(
        attuale == atteso,
        "la superficie CLI diverge dall'oracolo committato {ORACOLO_PATH}. \
         Uno spostamento meccanico di codice NON deve produrre questo diff. \
         Se il cambiamento e' intenzionale, rigenerare con \
         `PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-cli --test oracolo_superficie_cli` \
         e committare l'oracolo insieme alla modifica"
    );
}
