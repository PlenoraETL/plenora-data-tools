//! Nessun riferimento al progetto d'origine che qui non ha senso.
//!
//! Il repository deriva da `plenora-data-tools@190c493`, dove codici di
//! requisito (`R2.6`, `D16`, `§3.1`) e documenti (`architettura.md`,
//! `errori-e-limiti.md`, `piano-v5.md`, …) spiegavano le regole. Qui quei
//! documenti non esistono: un codice in un messaggio d'errore o in un
//! commento rimanda a un testo che il lettore non può aprire. Il test
//! scandisce il repository (sorgenti, manifesti, README, schede, script,
//! dati, provenienze dei vendor) e fallisce se uno di quei riferimenti
//! ricompare, o se un percorso `docs/…` o `scripts/…` citato non esiste.
//!
//! La scansione è completa o fallisce: un errore nel leggere una cartella o
//! un file di testo (anche UTF-8 non valido) fa fallire il test, e un file
//! con un'estensione non classificata ([`TESTO`], [`BINARI`]) pure.
//!
//! Citare il progetto d'origine per commit (`190c493`) resta lecito: dice da
//! dove viene il codice, non rimanda a un documento.

use std::fs;
use std::path::{Path, PathBuf};

/// Testi che nominano documenti, script o processi del progetto d'origine
/// assenti qui.
const VIETATI: &[&str] = &[
    "\u{a7}",
    "architettura.md",
    "errori-e-limiti",
    "piano-v5",
    "STATO.md",
    "stato-e-roadmap",
    "isolamento.md",
    "verifica_vendor_provenienza",
    "verifica_filtro_sperimentale",
    "verifica_risoluzione_vendor",
    "ICD ",
    "Non adottato",
];

/// Estensioni di testo: ogni file si legge per intero come UTF-8, e un
/// errore di lettura fa fallire il test.
/// `pyi`, `typed` (il marcatore PEP 561, vuoto), `ini` e `txt` sono
/// dell'SDK Python: stub, configurazione di mypy, requisiti fissati.
const TESTO: &[&str] = &[
    "rs", "toml", "md", "py", "pyi", "typed", "ini", "txt", "json", "csv", "lock", "yml",
];
/// Nomi di file di testo senza estensione.
const TESTO_PER_NOME: &[&str] = &[".gitignore", ".gitattributes"];
/// Estensioni binarie: non si leggono, si contano. Un file con
/// un'estensione fuori da [`TESTO`] e da questo elenco fa fallire il test,
/// così un formato nuovo si classifica invece di sparire dalla scansione.
const BINARI: &[&str] = &["parquet", "wkb", "gsb", "pyc"];
/// Voci fuori scansione: prodotti di build, storia git (`.git` è un file
/// in un worktree), patch di codice upstream. Dei vendor si leggono solo le
/// provenienze.
const ESCLUSE: &[&str] = &["target", ".git", "patches"];

fn radice() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// I file di testo da scandire e il numero di binari saltati.
#[derive(Default)]
struct Scansione {
    testi: Vec<PathBuf>,
    binari: usize,
}

fn voci(cartella: &Path) -> Vec<PathBuf> {
    let lettura =
        fs::read_dir(cartella).unwrap_or_else(|errore| panic!("{}: {errore}", cartella.display()));
    let mut voci: Vec<PathBuf> = lettura
        .map(|voce| {
            voce.unwrap_or_else(|errore| panic!("{}: {errore}", cartella.display()))
                .path()
        })
        .collect();
    voci.sort();
    voci
}

fn nome_di(percorso: &Path) -> String {
    percorso
        .file_name()
        .and_then(|nome| nome.to_str())
        .unwrap_or_else(|| panic!("{}: nome non UTF-8", percorso.display()))
        .to_owned()
}

fn estensione(percorso: &Path, ammesse: &[&str]) -> bool {
    percorso
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ammesse.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

fn classifica(percorso: PathBuf, scansione: &mut Scansione) {
    if estensione(&percorso, TESTO) || TESTO_PER_NOME.contains(&nome_di(&percorso).as_str()) {
        scansione.testi.push(percorso);
    } else if estensione(&percorso, BINARI) {
        scansione.binari += 1;
    } else {
        panic!(
            "{}: estensione non classificata (TESTO o BINARI)",
            percorso.display()
        );
    }
}

fn da_scandire(cartella: &Path, scansione: &mut Scansione) {
    for percorso in voci(cartella) {
        let nome = nome_di(&percorso);
        if ESCLUSE.contains(&nome.as_str()) {
            continue;
        }
        if !percorso.is_dir() {
            classifica(percorso, scansione);
            continue;
        }
        if nome == "vendor" {
            // Dei vendor solo le provenienze: il resto è codice upstream.
            for vendor in voci(&percorso) {
                for voce in voci(&vendor) {
                    if nome_di(&voce).starts_with("PROVENANCE") && estensione(&voce, &["md"]) {
                        scansione.testi.push(voce);
                    }
                }
            }
            continue;
        }
        da_scandire(&percorso, scansione);
    }
}

const fn parola(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

const fn virgolette(byte: Option<&u8>) -> bool {
    matches!(byte, Some(b'"' | b'\''))
}

/// Codici di requisito del progetto d'origine: `R2.6`, `R4.x`, `R3.4.1`,
/// `D16`, `D14.6`, a inizio parola, citati nel testo.
///
/// Scelta contestuale: un codice che è da solo l'intero contenuto di una
/// stringa (`"D16"`, `'R2.6'`: il valore di una fixture o di un esempio
/// JSON) è un dato, non una citazione, e non si segnala. Le citazioni del
/// progetto d'origine stavano sempre nel testo: fra parentesi, prima di
/// `:`, dopo una parola (`come D14.6`, `tabella R2.2`), dentro una frase.
fn codici(riga: &str) -> Vec<String> {
    let byte = riga.as_bytes();
    let mut trovati = Vec::new();
    for (i, &b) in byte.iter().enumerate() {
        if !(b == b'R' || b == b'D') || (i > 0 && parola(byte[i - 1])) {
            continue;
        }
        let cifre = byte[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if cifre == 0 || cifre > 2 {
            continue;
        }
        let dopo = i + 1 + cifre;
        let punto_e_cifra = byte.get(dopo) == Some(&b'.')
            && byte
                .get(dopo + 1)
                .is_some_and(|c| c.is_ascii_digit() || *c == b'x');
        let fine_parola = byte.get(dopo).is_none_or(|c| !parola(*c) && *c != b'.');
        let mut fine = if punto_e_cifra {
            dopo + 1
                + byte[dopo + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit() || matches!(c, b'.' | b'x'))
                    .count()
        } else if b == b'D' && fine_parola {
            dopo
        } else {
            continue;
        };
        if byte[..fine].ends_with(b".") {
            fine -= 1;
        }
        let dato = i > 0 && virgolette(byte.get(i - 1)) && virgolette(byte.get(fine));
        if !dato {
            trovati.push(riga[i..fine].to_owned());
        }
    }
    trovati
}

/// Percorsi `docs/…md` e `scripts/…py` citati nella riga.
fn percorsi(riga: &str) -> Vec<String> {
    let mut trovati = Vec::new();
    for prefisso in ["docs/", "scripts/"] {
        let mut da = 0;
        while let Some(pos) = riga[da..].find(prefisso) {
            let inizio = da + pos;
            da = inizio + prefisso.len();
            if inizio > 0 {
                let prima = riga.as_bytes()[inizio - 1];
                if parola(prima) || matches!(prima, b'/' | b'.' | b'-') {
                    continue;
                }
            }
            let fine = riga[inizio..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || "_./-".contains(c)))
                .map_or(riga.len(), |n| inizio + n);
            let percorso = riga[inizio..fine].trim_end_matches('.');
            if estensione(Path::new(percorso), &["md", "py"]) {
                trovati.push(percorso.to_owned());
            }
        }
    }
    trovati
}

#[test]
fn nessun_riferimento_obsoleto_nel_repository() {
    let radice = radice();
    let mut scansione = Scansione::default();
    da_scandire(&radice, &mut scansione);
    let questo = nome_di(Path::new(file!()));
    let mut difetti = Vec::new();
    let mut scanditi = 0;
    for percorso in &scansione.testi {
        if nome_di(percorso) == questo {
            continue;
        }
        let testo = fs::read_to_string(percorso)
            .unwrap_or_else(|errore| panic!("{}: {errore}", percorso.display()));
        scanditi += 1;
        let relativo = percorso.strip_prefix(&radice).unwrap_or(percorso).display();
        for (numero, riga) in testo.lines().enumerate() {
            for vietato in VIETATI {
                if riga.contains(vietato) {
                    difetti.push(format!("{relativo}:{}: `{vietato}`", numero + 1));
                }
            }
            for codice in codici(riga) {
                difetti.push(format!("{relativo}:{}: codice `{codice}`", numero + 1));
            }
            for citato in percorsi(riga) {
                if !radice.join(&citato).is_file() {
                    difetti.push(format!("{relativo}:{}: `{citato}` non esiste", numero + 1));
                }
            }
        }
    }
    assert!(scanditi > 100, "scansione troppo corta: {scanditi} file");
    assert!(scansione.binari > 0, "nessun binario contato");
    assert!(
        difetti.is_empty(),
        "riferimenti obsoleti:\n{}",
        difetti.join("\n")
    );
}

#[test]
fn i_riconoscitori_riconoscono() {
    assert_eq!(codici("vale (R2.6) e R4.x"), ["R2.6", "R4.x"]);
    assert_eq!(codici("una sola (D16), come D14.6."), ["D16", "D14.6"]);
    assert_eq!(
        codici("tipi (R3.4.1, ordine) e R2.5 impone"),
        ["R3.4.1", "R2.5"]
    );
    assert!(codici("soundex R163, ID1, 2D, D1a, Robert").is_empty());
    // Un codice che è l'intera stringa è un dato di fixture, non una
    // citazione; dentro una frase fra virgolette resta una citazione.
    assert!(codici(r#"{"valori": ["D16", "R2.6"]}, 'D14.6'"#).is_empty());
    assert_eq!(codici(r#""vedi D16""#), ["D16"]);
    assert_eq!(percorsi("vedi `docs/manca.md`."), ["docs/manca.md"]);
    assert!(percorsi("[x](../docs/operazioni.md) o crates/docs/x.md").is_empty());
}

#[test]
fn un_estensione_non_classificata_fa_fallire() {
    let esito = std::panic::catch_unwind(|| {
        classifica(
            PathBuf::from("x/sconosciuto.bin"),
            &mut Scansione::default(),
        );
    });
    assert!(esito.is_err());
    let mut scansione = Scansione::default();
    classifica(PathBuf::from("x/y.parquet"), &mut scansione);
    classifica(PathBuf::from("x/y.rs"), &mut scansione);
    assert_eq!((scansione.binari, scansione.testi.len()), (1, 1));
}
