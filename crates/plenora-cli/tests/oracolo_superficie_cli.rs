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
//! # Perche' l'oracolo e' un modello e non un'istantanea
//!
//! Una parte della superficie dipende dalle **feature** con cui si compila:
//! `capabilities` e `catalog` stampano l'elenco dei backend, che e' vuoto con
//! le feature predefinite e vale `["geos", "proj"]` con `full-backends`. Un
//! file catturato una volta sola non puo' essere vero in entrambe le build, e
//! rigenerarlo sotto una delle due sposta soltanto il fallimento sull'altra.
//!
//! Il file committato e' quindi un **modello**: nei tre punti in cui compare
//! il valore di `backends` c'e' un marcatore, e il valore atteso viene
//! materializzato a partire da costanti scritte a mano, una per combinazione
//! di feature. Il confronto resta **byte per byte**: cambia solo il modo in
//! cui si ottiene il testo atteso.
//!
//! Le costanti sono deliberatamente **indipendenti** da
//! `backends_compilati()`, la funzione che produce quel valore: derivarle da
//! lei farebbe confrontare il codice con se stesso, e un suo difetto
//! resterebbe invisibile.
//!
//! # Rigenerazione
//!
//! Dopo un cambiamento **intenzionale** della superficie:
//!
//! ```sh
//! PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-cli --test oracolo_superficie_cli
//! ```
//!
//! La rigenerazione riscrive il **modello**, non l'istantanea: qualunque sia
//! la combinazione di feature usata, il file prodotto e' lo stesso. Va
//! committato insieme alla modifica, cosi' il diff mostra in review che cosa
//! vede l'utente di diverso.

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

// ---------------------------------------------------------------------------
// La parte della superficie che dipende dalle feature.
// ---------------------------------------------------------------------------

/// Prende il posto del valore di `backends` nel modello committato.
///
/// Contiene caratteri che il JSON serializzato non produce mai, e c'e' un test
/// che lo verifica invece di darlo per buono.
const MARCATORE_BACKENDS: &str = "@@BACKENDS@@";

/// Il testo che precede il valore, nei byte esatti del file: `stdout` e' una
/// stringa JSON, quindi le virgolette interne sono con la barra rovescia.
/// Ancorare la sostituzione al nome del campo evita di toccare un `[]`
/// qualunque.
const PREFISSO_BACKENDS: &str = r#"\"backends\": "#;

/// Quante volte la superficie CLI stampa `backends`: `capabilities` in due
/// forme e `catalog`.
///
/// Fissarlo e' il punto della faccenda. Senza, una quarta occorrenza
/// resterebbe nel modello come **valore letterale** invece che come
/// marcatore: il file passerebbe con le feature usate per rigenerarlo e solo
/// con quelle, e il difetto tornerebbe identico a quello che questo test
/// esiste per chiudere.
const OCCORRENZE_BACKENDS: usize = 3;

// I quattro valori possibili, nei byte esatti che il file contiene. Sono
// scritti a mano di proposito: vedi la nota sull'indipendenza in testa al
// file.
const BACKENDS_NESSUNO: &str = "[]";
const BACKENDS_GEOS: &str = r#"[\n    \"geos\"\n  ]"#;
const BACKENDS_PROJ: &str = r#"[\n    \"proj\"\n  ]"#;
const BACKENDS_GEOS_PROJ: &str = r#"[\n    \"geos\",\n    \"proj\"\n  ]"#;

/// Il valore atteso per la combinazione di feature con cui si sta compilando.
///
/// Il `match` e' **esaustivo** su una coppia di booleani: aggiungere un terzo
/// backend non compilera' finche' qualcuno non avra' deciso, esplicitamente,
/// che cosa l'oracolo si aspetta in ciascuno dei nuovi casi.
const fn backends_attesi() -> &'static str {
    match (
        cfg!(feature = "geos-backend"),
        cfg!(feature = "proj-backend"),
    ) {
        (false, false) => BACKENDS_NESSUNO,
        (true, false) => BACKENDS_GEOS,
        (false, true) => BACKENDS_PROJ,
        (true, true) => BACKENDS_GEOS_PROJ,
    }
}

/// Dal testo osservato al modello: il valore diventa il marcatore.
///
/// Fallisce se le occorrenze non sono esattamente quelle attese, cosi' una
/// rigenerazione non puo' produrre un modello parziale.
fn in_modello(osservato: &str) -> String {
    let concreto = format!("{PREFISSO_BACKENDS}{}", backends_attesi());
    let astratto = format!("{PREFISSO_BACKENDS}{MARCATORE_BACKENDS}");
    let trovate = osservato.matches(&concreto).count();
    assert_eq!(
        trovate, OCCORRENZE_BACKENDS,
        "la superficie stampa {trovate} volte il valore di `backends`, non {OCCORRENZE_BACKENDS}. \
         Se l'occorrenza in piu' e' voluta, aggiornare OCCORRENZE_BACKENDS; se e' in meno, \
         qualcosa ha smesso di dichiarare i backend"
    );
    osservato.replace(&concreto, &astratto)
}

/// Dal modello al testo atteso: il marcatore diventa il valore.
fn atteso_da(modello: &str) -> String {
    let trovate = modello.matches(MARCATORE_BACKENDS).count();
    assert_eq!(
        trovate, OCCORRENZE_BACKENDS,
        "il modello committato contiene {trovate} marcatori invece di {OCCORRENZE_BACKENDS}. \
         Con meno marcatori il file e' un'istantanea di una sola combinazione di feature, \
         e passerebbe soltanto con quella: rigenerarlo con PLENORA_UPDATE_SNAPSHOT=1"
    );
    modello.replace(MARCATORE_BACKENDS, backends_attesi())
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
        std::fs::write(path, in_modello(&attuale))
            .expect("rigenerazione dell'oracolo della superficie");
        eprintln!("modello della superficie CLI rigenerato in {ORACOLO_PATH}");
        return;
    }
    let modello = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("oracolo non leggibile ({error}): generarlo con PLENORA_UPDATE_SNAPSHOT=1")
    });
    let atteso = atteso_da(&modello.replace("\r\n", "\n"));
    assert!(
        attuale == atteso,
        "la superficie CLI diverge dall'oracolo committato {ORACOLO_PATH}. \
         Uno spostamento meccanico di codice NON deve produrre questo diff. \
         Se il cambiamento e' intenzionale, rigenerare con \
         `PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-cli --test oracolo_superficie_cli` \
         e committare l'oracolo insieme alla modifica"
    );
}

/// Il marcatore deve essere impossibile da confondere con l'output vero.
///
/// Se un giorno la CLI stampasse quella sequenza, la sostituzione
/// introdurrebbe una differenza inventata e il fallimento non si spiegherebbe.
#[test]
fn il_marcatore_non_compare_nella_superficie_reale() {
    assert!(
        !oracolo_content().contains(MARCATORE_BACKENDS),
        "la superficie CLI contiene {MARCATORE_BACKENDS}, che l'oracolo usa come marcatore: \
         sceglierne un altro"
    );
}

/// I quattro valori attesi devono essere distinti fra loro.
///
/// Se due combinazioni di feature avessero lo stesso testo atteso, l'oracolo
/// passerebbe con la feature sbagliata attiva e non lo direbbe.
#[test]
fn i_valori_attesi_sono_distinti() {
    let valori = [
        BACKENDS_NESSUNO,
        BACKENDS_GEOS,
        BACKENDS_PROJ,
        BACKENDS_GEOS_PROJ,
    ];
    for (i, primo) in valori.iter().enumerate() {
        for secondo in valori.iter().skip(i + 1) {
            assert!(
                primo != secondo,
                "due combinazioni di feature attendono lo stesso testo"
            );
        }
    }
}

/// Andata e ritorno: dal testo osservato al modello e viceversa.
///
/// E' la proprieta' che rende la rigenerazione indipendente dalle feature —
/// quella la cui assenza aveva reso rosso il CI.
#[test]
fn il_modello_non_dipende_dalle_feature_usate_per_generarlo() {
    let osservato = oracolo_content();
    let modello = in_modello(&osservato);
    assert!(
        !modello.contains(&format!("{PREFISSO_BACKENDS}{}", backends_attesi())),
        "il modello conserva ancora il valore concreto dei backend"
    );
    assert!(
        atteso_da(&modello) == osservato,
        "il giro modello -> atteso non torna al punto di partenza"
    );
}
