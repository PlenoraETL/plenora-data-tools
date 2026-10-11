//! Gli esiti di riferimento della lettura Parquet su file reali: i 106 file
//! di prova di `apache/parquet-testing` e `apache/arrow-testing` e la matrice
//! degli scrittori (pyarrow, fastparquet, `DuckDB`, polars). Per ogni file,
//! in `tests/riferimenti/parquet_esiti.tsv`: insieme, percorso, sha256,
//! scrittore (`created_by`), esito della lettura diretta con il fork
//! (`ok <righe>` o `err <testo>`) ed esito dal confine di `plenora-io`
//! (`ok <righe>` o `err <categoria>`).
//!
//! I file non sono nel repository (licenze diverse, circa 100 MB): la prova
//! è ignorata per default e si esegue con le loro cartelle, ciascuna
//! obbligatoria (una cartella mancante è un errore, non un insieme saltato):
//!
//! ```sh
//! PLENORA_PARQUET_TESTING=<clone di apache/parquet-testing> \
//! PLENORA_ARROW_TESTING=<clone di apache/arrow-testing> \
//! PLENORA_SCRITTORI=<cartella della matrice degli scrittori> \
//! cargo test --release -p plenora-io --test esiti_di_riferimento -- --ignored
//! ```
//!
//! Un file diverso (sha256), mancante o in più, o un esito diverso fa
//! fallire la prova. Con `PLENORA_PARQUET_ESITI_SCRIVI=1` la prova riscrive il
//! riferimento invece di confrontarlo: il cambio si rilegge in PR.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};
use sha2::{Digest, Sha256};

/// L'intestazione del riferimento: da dove vengono i file.
const INTESTAZIONE: &str = "\
# Esiti di riferimento della lettura Parquet (tests/esiti_di_riferimento.rs).
# parquet-testing: apache/parquet-testing al commit 56653c437c8092f704a092d0d1d4e600124cd49f (data/, bad_data/).
# arrow-testing: apache/arrow-testing al commit 9ff285c88565f0f6abc855918c6a342e70e4909c (data/parquet/).
# scrittori: la matrice dei giri di revisione della PR #125, non versionata (circa 87 MB);
#   lo scrittore e la versione sono in created_by, i parametri nel nome del file
#   (pyarrow 25.0.1 e 26.0.0, fastparquet 2026.9.0, DuckDB 1.5.6, polars 2.0.0).
# Colonne: insieme, percorso, sha256, created_by, lettura diretta con il fork, confine di plenora-io.
";

/// Gli insiemi: nome, variabile d'ambiente della cartella, sottocartelle lette.
const INSIEMI: &[(&str, &str, &[&str])] = &[
    (
        "parquet-testing",
        "PLENORA_PARQUET_TESTING",
        &["data", "bad_data"],
    ),
    ("arrow-testing", "PLENORA_ARROW_TESTING", &["data/parquet"]),
    (
        "scrittori",
        "PLENORA_SCRITTORI",
        &[
            "scritti",
            "scritti-piatte",
            "scritti-fpv2",
            "scritti-nulli",
            "scritti-flba",
        ],
    ),
];

fn riferimento() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/riferimenti/parquet_esiti.tsv")
}

fn parquet_sotto(cartella: &Path, file: &mut Vec<PathBuf>) {
    for voce in std::fs::read_dir(cartella).unwrap() {
        let percorso = voce.unwrap().path();
        if percorso.is_dir() {
            parquet_sotto(&percorso, file);
        } else if percorso.extension().is_some_and(|e| e == "parquet") {
            file.push(percorso);
        }
    }
}

/// Una riga di testo: i tab e gli a capo di un messaggio diventano spazi.
fn una_riga(testo: &str) -> String {
    testo.replace(['\t', '\n', '\r'], " ")
}

fn sha256(byte: &[u8]) -> String {
    Sha256::digest(byte)
        .iter()
        .fold(String::new(), |mut testo, b| {
            let _ = write!(testo, "{b:02x}");
            testo
        })
}

/// La lettura diretta con il fork: tutte le righe, o il primo errore.
fn diretta(percorso: &Path) -> String {
    let esito = std::panic::catch_unwind(|| -> Result<usize, String> {
        let file = std::fs::File::open(percorso).map_err(|e| e.to_string())?;
        let lettore = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| e.to_string())?
            .build()
            .map_err(|e| e.to_string())?;
        let mut righe = 0;
        for batch in lettore {
            righe += batch.map_err(|e| e.to_string())?.num_rows();
        }
        Ok(righe)
    });
    match esito {
        Ok(Ok(righe)) => format!("ok {righe}"),
        Ok(Err(errore)) => format!("err {}", una_riga(&errore)),
        Err(_) => "PANICO".to_owned(),
    }
}

fn dal_confine(percorso: &Path) -> String {
    match plenora_io::leggi_tabella(percorso, Some(plenora_io::Formato::Parquet), u64::MAX) {
        Ok(tabella) => format!("ok {}", tabella.num_rows()),
        Err(errore) => format!("err {:?}", errore.category()),
    }
}

fn scrittore(percorso: &Path) -> String {
    let esito = std::panic::catch_unwind(|| {
        let file = std::fs::File::open(percorso).ok()?;
        let lettore = SerializedFileReader::new(file).ok()?;
        lettore
            .metadata()
            .file_metadata()
            .created_by()
            .map(una_riga)
    });
    esito.ok().flatten().unwrap_or_else(|| "-".to_owned())
}

/// Le righe del riferimento, calcolate sui file: chiave (insieme, percorso).
fn calcola() -> BTreeMap<(String, String), String> {
    let mut mancanti = Vec::new();
    let mut righe = BTreeMap::new();
    for (insieme, variabile, sottocartelle) in INSIEMI {
        let Ok(radice) = std::env::var(variabile) else {
            mancanti.push(*variabile);
            continue;
        };
        let radice = PathBuf::from(radice);
        for sotto in *sottocartelle {
            let cartella = radice.join(sotto);
            assert!(
                cartella.is_dir(),
                "{}: cartella assente",
                cartella.display()
            );
            let mut file = Vec::new();
            parquet_sotto(&cartella, &mut file);
            for percorso in file {
                let relativo = percorso
                    .strip_prefix(&radice)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let byte = std::fs::read(&percorso).unwrap();
                let riga = format!(
                    "{}\t{}\t{}\t{}",
                    sha256(&byte),
                    scrittore(&percorso),
                    diretta(&percorso),
                    dal_confine(&percorso)
                );
                righe.insert(((*insieme).to_owned(), relativo), riga);
            }
        }
    }
    assert!(
        mancanti.is_empty(),
        "cartelle dei file non date: {mancanti:?} (un insieme non verificato non è un insieme passato)"
    );
    righe
}

fn leggi_riferimento() -> BTreeMap<(String, String), String> {
    let testo = std::fs::read_to_string(riferimento()).unwrap();
    testo
        .lines()
        .filter(|riga| !riga.starts_with('#') && !riga.is_empty())
        .map(|riga| {
            let mut parti = riga.splitn(3, '\t');
            let insieme = parti.next().unwrap().to_owned();
            let percorso = parti.next().unwrap().to_owned();
            let resto = parti.next().unwrap().to_owned();
            ((insieme, percorso), resto)
        })
        .collect()
}

#[test]
#[ignore = "chiede i file di prova e la matrice degli scrittori (vedi la documentazione del modulo)"]
fn gli_esiti_sui_file_reali_sono_quelli_di_riferimento() {
    let dai_file = calcola();
    if std::env::var("PLENORA_PARQUET_ESITI_SCRIVI").as_deref() == Ok("1") {
        let mut testo = INTESTAZIONE.to_owned();
        for ((insieme, percorso), resto) in &dai_file {
            let _ = writeln!(testo, "{insieme}\t{percorso}\t{resto}");
        }
        std::fs::write(riferimento(), testo).unwrap();
        return;
    }
    let attesi = leggi_riferimento();
    let mut differenze = Vec::new();
    for (chiave, atteso) in &attesi {
        match dai_file.get(chiave) {
            None => differenze.push(format!("mancante: {chiave:?}")),
            Some(calcolato) if calcolato != atteso => differenze.push(format!(
                "{chiave:?}\n    atteso    {atteso}\n    calcolato {calcolato}"
            )),
            Some(_) => {}
        }
    }
    for chiave in dai_file.keys() {
        if !attesi.contains_key(chiave) {
            differenze.push(format!("in più: {chiave:?}"));
        }
    }
    assert!(
        differenze.is_empty(),
        "{} differenze dal riferimento:\n{}",
        differenze.len(),
        differenze.join("\n")
    );
}

/// Il riferimento è ben formato: sei colonne, sha256 di 64 cifre, esiti
/// nella forma attesa, e i conteggi degli insiemi (103 + 3 file di prova).
#[test]
fn il_riferimento_e_ben_formato() {
    let testo = std::fs::read_to_string(riferimento()).unwrap();
    let mut per_insieme: BTreeMap<String, usize> = BTreeMap::new();
    for riga in testo
        .lines()
        .filter(|r| !r.starts_with('#') && !r.is_empty())
    {
        let colonne: Vec<&str> = riga.split('\t').collect();
        assert_eq!(colonne.len(), 6, "{riga}");
        assert!(
            colonne[2].len() == 64 && colonne[2].bytes().all(|b| b.is_ascii_hexdigit()),
            "{riga}"
        );
        for esito in &colonne[4..] {
            assert!(
                esito.starts_with("ok ") || esito.starts_with("err ") || *esito == "PANICO",
                "{riga}"
            );
        }
        assert_ne!(colonne[4], "PANICO", "{riga}");
        *per_insieme.entry(colonne[0].to_owned()).or_default() += 1;
    }
    assert_eq!(per_insieme.get("parquet-testing"), Some(&103));
    assert_eq!(per_insieme.get("arrow-testing"), Some(&3));
    assert!(per_insieme.get("scrittori").is_some_and(|n| *n > 0));
}
