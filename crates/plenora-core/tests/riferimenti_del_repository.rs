//! Nessun riferimento al progetto d'origine che qui non ha senso.
//!
//! Il repository deriva da `plenora-data-tools@190c493`, dove codici di
//! requisito (`R2.6`, `D16`, `§3.1`) e documenti (`architettura.md`,
//! `errori-e-limiti.md`, `piano-v5.md`, …) spiegavano le regole. Qui quei
//! documenti non esistono: un codice in un messaggio d'errore o in un
//! commento rimanda a un testo che il lettore non può aprire. Il test
//! scandisce il testo del repository (sorgenti, manifesti, README, schede,
//! script, provenienze dei vendor) e fallisce se uno di quei riferimenti
//! ricompare, o se un percorso `docs/…` o `scripts/…` citato non esiste.
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
    ".github/workflows",
    "ICD ",
    "Non adottato",
];

fn radice() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn da_scandire(cartella: &Path, file: &mut Vec<PathBuf>) {
    let Ok(voci) = fs::read_dir(cartella) else {
        return;
    };
    let mut voci: Vec<PathBuf> = voci
        .filter_map(|voce| voce.ok().map(|v| v.path()))
        .collect();
    voci.sort();
    for percorso in voci {
        let nome = percorso
            .file_name()
            .and_then(|nome| nome.to_str())
            .unwrap_or_default()
            .to_owned();
        if percorso.is_dir() {
            if matches!(nome.as_str(), "target" | ".git" | "patches" | "data") {
                continue;
            }
            if nome == "vendor" {
                // Dei vendor solo le provenienze: il resto è codice upstream.
                for vendor in fs::read_dir(&percorso).expect("vendor").flatten() {
                    for voce in fs::read_dir(vendor.path()).into_iter().flatten().flatten() {
                        let p = voce.path();
                        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                        if n.starts_with("PROVENANCE") && estensione(&p, &["md"]) {
                            file.push(p);
                        }
                    }
                }
                continue;
            }
            da_scandire(&percorso, file);
        } else if estensione(&percorso, &["rs", "toml", "md", "py"]) {
            file.push(percorso);
        }
    }
}

fn estensione(percorso: &Path, ammesse: &[&str]) -> bool {
    percorso
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ammesse.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

const fn parola(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Codici di requisito del progetto d'origine: `R2.6`, `R4.x`, `D16`,
/// `D14.6`, a inizio parola.
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
        if punto_e_cifra {
            trovati.push(riga[i..dopo + 2].to_owned());
        } else if b == b'D' && fine_parola {
            trovati.push(riga[i..dopo].to_owned());
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
    let mut file = Vec::new();
    da_scandire(&radice, &mut file);
    let questo = Path::new(file!())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let mut difetti = Vec::new();
    let mut scanditi = 0;
    for percorso in &file {
        if percorso.file_name().and_then(|n| n.to_str()) == Some(questo.as_str()) {
            continue;
        }
        let Ok(testo) = fs::read_to_string(percorso) else {
            continue;
        };
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
    assert!(codici("soundex R163, ID1, 2D, D1a, Robert").is_empty());
    assert_eq!(percorsi("vedi `docs/manca.md`."), ["docs/manca.md"]);
    assert!(percorsi("[x](../docs/operazioni.md) o crates/docs/x.md").is_empty());
}
