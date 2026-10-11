//! Gli esiti di riferimento della lettura Parquet su file reali: i 106 file
//! di prova di `apache/parquet-testing` e `apache/arrow-testing` e la matrice
//! degli scrittori (pyarrow, fastparquet, `DuckDB`, polars). Per ogni file,
//! in `tests/riferimenti/parquet_esiti.tsv`: insieme, percorso, sha256,
//! scrittore (`created_by`), esito della lettura diretta con il fork ed esito
//! dal confine di `plenora-io`. Un esito è `ok <righe> <digest>` (il digest
//! canonico dello schema e dei valori letti, [`digest`]) o `err <testo>` (il
//! messaggio del fork; dal confine la categoria).
//!
//! I file non sono nel repository (licenze diverse, circa 100 MB). I due
//! corpus Apache sono ai commit dell'intestazione del riferimento; la
//! matrice degli scrittori si rigenera con
//! `tests/riferimenti/scrittori/genera.py`, nei due ambienti dichiarati con
//! le versioni e gli hash esatti (`requisiti-*.txt`): 366 dei 367 file
//! tornano byte per byte, e [`NON_RIPRODUCIBILI`] ha l'esito confrontato
//! senza sha256 né digest. La prova è ignorata per default e si esegue con
//! le cartelle:
//!
//! ```sh
//! PLENORA_PARQUET_TESTING=<clone di apache/parquet-testing> \
//! PLENORA_ARROW_TESTING=<clone di apache/arrow-testing> \
//! PLENORA_SCRITTORI=<cartella prodotta da genera.py> \
//! cargo test --release -p plenora-io --test esiti_di_riferimento -- --ignored
//! ```
//!
//! La qualificazione è l'insieme completo: una cartella non data è un
//! errore. `PLENORA_PARQUET_ESITI_INSIEMI=parquet-testing,arrow-testing`
//! limita la prova agli insiemi nominati e dichiara gli altri come non
//! verificati. Un file diverso (sha256), mancante o in più, o un esito
//! diverso fa fallire la prova. Con `PLENORA_PARQUET_ESITI_SCRIVI=1` la prova
//! riscrive il riferimento invece di confrontarlo (solo sull'insieme
//! completo): il cambio si rilegge in PR.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};
use plenora_core::arrow::array::cast::AsArray;
use plenora_core::arrow::array::{Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Schema};
use sha2::{Digest, Sha256};

/// L'intestazione del riferimento: da dove vengono i file.
const INTESTAZIONE: &str = "\
# Esiti di riferimento della lettura Parquet (tests/esiti_di_riferimento.rs).
# parquet-testing: apache/parquet-testing al commit 56653c437c8092f704a092d0d1d4e600124cd49f (data/, bad_data/).
# arrow-testing: apache/arrow-testing al commit 9ff285c88565f0f6abc855918c6a342e70e4909c (data/parquet/).
# scrittori: tests/riferimenti/scrittori/genera.py, ambienti requisiti-pyarrow25.txt e
#   requisiti-principale.txt (pyarrow 25.0.1 e 26.0.0, fastparquet 2026.9.0, DuckDB 1.5.6,
#   polars 2.0.0); i parametri sono nel nome del file.
# Colonne: insieme, percorso, sha256, created_by, lettura diretta con il fork, confine di plenora-io.
# Un esito: `ok <righe> <digest dello schema e dei valori>` o `err <testo>` (dal confine la categoria).
";

/// Gli insiemi: nome, variabile d'ambiente della cartella, sottocartelle
/// lette, numero di file.
const INSIEMI: &[(&str, &str, &[&str], usize)] = &[
    (
        "parquet-testing",
        "PLENORA_PARQUET_TESTING",
        &["data", "bad_data"],
        103,
    ),
    (
        "arrow-testing",
        "PLENORA_ARROW_TESTING",
        &["data/parquet"],
        3,
    ),
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
        367,
    ),
];

/// I file della matrice senza byte riproducibili (UUID casuali di
/// `DuckDB`): per loro la prova confronta le righe e gli esiti, non lo
/// sha256 né i digest dei valori.
const NON_RIPRODUCIBILI: &[(&str, &str)] =
    &[("scrittori", "scritti-flba/duckdb/duckdb-uuid-dec.parquet")];

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

fn esadecimale(byte: &[u8]) -> String {
    byte.iter().fold(String::new(), |mut testo, b| {
        let _ = write!(testo, "{b:02x}");
        testo
    })
}

// --- Il digest canonico ------------------------------------------------------

/// Un pezzo del digest con la sua lunghezza: nessuna concatenazione è
/// ambigua.
fn pezzo(h: &mut Sha256, byte: &[u8]) {
    h.update((byte.len() as u64).to_le_bytes());
    h.update(byte);
}

/// Il valore `i` di `array` nel digest: un segno per il nullo, poi una forma
/// che dipende solo dal valore, non da come i buffer lo tengono (offset,
/// slice, viste, dizionari). Un tipo senza forma canonica ferma la prova.
fn valore(h: &mut Sha256, array: &dyn Array, i: usize) {
    if array.logical_nulls().is_some_and(|nulli| nulli.is_null(i)) {
        h.update([0]);
        return;
    }
    h.update([1]);
    match array.data_type() {
        DataType::Null => {}
        DataType::Boolean => h.update([u8::from(array.as_boolean().value(i))]),
        DataType::Utf8 => pezzo(h, array.as_string::<i32>().value(i).as_bytes()),
        DataType::LargeUtf8 => pezzo(h, array.as_string::<i64>().value(i).as_bytes()),
        DataType::Utf8View => pezzo(h, array.as_string_view().value(i).as_bytes()),
        DataType::Binary => pezzo(h, array.as_binary::<i32>().value(i)),
        DataType::LargeBinary => pezzo(h, array.as_binary::<i64>().value(i)),
        DataType::BinaryView => pezzo(h, array.as_binary_view().value(i)),
        DataType::FixedSizeBinary(_) => pezzo(h, array.as_fixed_size_binary().value(i)),
        DataType::List(_) => elementi(h, array.as_list::<i32>().value(i).as_ref()),
        DataType::LargeList(_) => elementi(h, array.as_list::<i64>().value(i).as_ref()),
        DataType::FixedSizeList(_, _) => {
            elementi(h, array.as_fixed_size_list().value(i).as_ref());
        }
        DataType::Map(_, _) => elementi(h, &array.as_map().value(i)),
        DataType::Struct(_) => {
            for figlio in array.as_struct().columns() {
                valore(h, figlio.as_ref(), i);
            }
        }
        DataType::Dictionary(_, _) => {
            let dizionario = array.as_any_dictionary();
            let chiave = dizionario.normalized_keys()[i];
            valore(h, dizionario.values().as_ref(), chiave);
        }
        tipo => match tipo.primitive_width() {
            // Interi, decimali, virgola mobile, date, ore, istanti, durate,
            // intervalli: i byte little-endian del valore.
            Some(larghezza) => {
                let dati = array.to_data();
                let inizio = (dati.offset() + i) * larghezza;
                pezzo(h, &dati.buffers()[0].as_slice()[inizio..inizio + larghezza]);
            }
            None => panic!("tipo senza forma canonica nel digest: {tipo}"),
        },
    }
}

fn elementi(h: &mut Sha256, figli: &dyn Array) {
    h.update((figli.len() as u64).to_le_bytes());
    for j in 0..figli.len() {
        valore(h, figli, j);
    }
}

/// Il digest canonico di una tabella letta: lo schema (nome, tipo, nullabilità
/// di ogni campo, senza metadati) e i valori, riga per riga, colonna per
/// colonna, nell'ordine letto; 32 cifre esadecimali (128 bit).
fn digest(schema: &Schema, blocchi: &[RecordBatch]) -> String {
    let mut h = Sha256::new();
    for campo in schema.fields() {
        pezzo(&mut h, campo.name().as_bytes());
        pezzo(&mut h, campo.data_type().to_string().as_bytes());
        h.update([u8::from(campo.is_nullable())]);
    }
    for blocco in blocchi {
        for riga in 0..blocco.num_rows() {
            for colonna in blocco.columns() {
                valore(&mut h, colonna.as_ref(), riga);
            }
        }
    }
    esadecimale(&h.finalize()[..16])
}

// --- Le letture ---------------------------------------------------------------

/// La lettura diretta con il fork: tutte le righe e il loro digest, o il
/// primo errore.
fn diretta(percorso: &Path) -> String {
    let esito = std::panic::catch_unwind(|| -> Result<String, String> {
        let file = std::fs::File::open(percorso).map_err(|e| e.to_string())?;
        let costruttore =
            ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| e.to_string())?;
        let schema = costruttore.schema().clone();
        let lettore = costruttore.build().map_err(|e| e.to_string())?;
        let mut blocchi = Vec::new();
        for batch in lettore {
            blocchi.push(batch.map_err(|e| e.to_string())?);
        }
        let righe: usize = blocchi.iter().map(RecordBatch::num_rows).sum();
        Ok(format!("{righe} {}", digest(&schema, &blocchi)))
    });
    match esito {
        Ok(Ok(letto)) => format!("ok {letto}"),
        Ok(Err(errore)) => format!("err {}", una_riga(&errore)),
        Err(_) => "PANICO".to_owned(),
    }
}

fn dal_confine(percorso: &Path) -> String {
    match plenora_io::leggi_tabella(percorso, Some(plenora_io::Formato::Parquet), u64::MAX) {
        Ok(tabella) => format!(
            "ok {} {}",
            tabella.num_rows(),
            digest(tabella.schema().as_ref(), std::slice::from_ref(&tabella))
        ),
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

/// Gli insiemi da verificare: tutti, salvo `PLENORA_PARQUET_ESITI_INSIEMI`.
fn insiemi_scelti() -> BTreeSet<&'static str> {
    let tutti: BTreeSet<&str> = INSIEMI.iter().map(|(nome, ..)| *nome).collect();
    let Ok(variabile) = std::env::var("PLENORA_PARQUET_ESITI_INSIEMI") else {
        return tutti;
    };
    let scelti: BTreeSet<&str> = variabile.split(',').map(str::trim).collect();
    let sconosciuti: Vec<_> = scelti.difference(&tutti).collect();
    assert!(
        sconosciuti.is_empty(),
        "insiemi sconosciuti: {sconosciuti:?}"
    );
    for escluso in tutti.difference(&scelti) {
        eprintln!(
            "INSIEME NON VERIFICATO: {escluso} (escluso da PLENORA_PARQUET_ESITI_INSIEMI); \
             la qualificazione chiede l'insieme completo"
        );
    }
    tutti
        .into_iter()
        .filter(|nome| scelti.contains(nome))
        .collect()
}

/// Le righe del riferimento calcolate sui file degli insiemi scelti:
/// chiave (insieme, percorso).
fn calcola(scelti: &BTreeSet<&str>) -> BTreeMap<(String, String), String> {
    let mut mancanti = Vec::new();
    let mut righe = BTreeMap::new();
    for (insieme, variabile, sottocartelle, _) in INSIEMI {
        if !scelti.contains(insieme) {
            continue;
        }
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
                    esadecimale(&Sha256::digest(&byte)),
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
        "cartelle dei file non date: {mancanti:?} (un insieme non verificato non è un insieme \
         passato; PLENORA_PARQUET_ESITI_INSIEMI ne sceglie un sottoinsieme dichiarato)"
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

/// Per un file non riproducibile: la riga senza sha256 e senza i digest.
fn senza_byte(resto: &str) -> String {
    resto
        .split('\t')
        .skip(1)
        .map(|colonna| {
            let parti: Vec<&str> = colonna.split(' ').collect();
            if parti.len() == 3 && parti[0] == "ok" {
                format!("ok {}", parti[1])
            } else {
                colonna.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\t")
}

#[test]
#[ignore = "chiede i file di prova e la matrice degli scrittori (vedi la documentazione del modulo)"]
fn gli_esiti_sui_file_reali_sono_quelli_di_riferimento() {
    let scelti = insiemi_scelti();
    let dai_file = calcola(&scelti);
    if std::env::var("PLENORA_PARQUET_ESITI_SCRIVI").as_deref() == Ok("1") {
        assert_eq!(
            scelti.len(),
            INSIEMI.len(),
            "si riscrive solo sull'insieme completo"
        );
        let mut testo = INTESTAZIONE.to_owned();
        for ((insieme, percorso), resto) in &dai_file {
            let _ = writeln!(testo, "{insieme}\t{percorso}\t{resto}");
        }
        std::fs::write(riferimento(), testo).unwrap();
        return;
    }
    let attesi: BTreeMap<_, _> = leggi_riferimento()
        .into_iter()
        .filter(|((insieme, _), _)| scelti.contains(insieme.as_str()))
        .collect();
    let mut differenze = Vec::new();
    for (chiave, atteso) in &attesi {
        let non_riproducibile = NON_RIPRODUCIBILI
            .iter()
            .any(|(i, p)| chiave.0 == *i && chiave.1 == *p);
        match dai_file.get(chiave) {
            None => differenze.push(format!("mancante: {chiave:?}")),
            Some(trovato) if non_riproducibile && senza_byte(trovato) != senza_byte(atteso) => {
                differenze.push(format!(
                    "{chiave:?} (non riproducibile)\n    atteso  {atteso}\n    trovato {trovato}"
                ));
            }
            Some(trovato) if !non_riproducibile && trovato != atteso => differenze.push(format!(
                "{chiave:?}\n    atteso  {atteso}\n    trovato {trovato}"
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

/// Il riferimento è ben formato: sei colonne, chiavi uniche, sha256 di 64
/// cifre, esiti nella forma attesa (un `ok` con righe e digest), nessun
/// panico, e i conteggi esatti degli insiemi (103 + 3 + 367).
#[test]
fn il_riferimento_e_ben_formato() {
    let testo = std::fs::read_to_string(riferimento()).unwrap();
    let mut per_insieme: BTreeMap<String, usize> = BTreeMap::new();
    let mut chiavi = BTreeSet::new();
    for riga in testo
        .lines()
        .filter(|r| !r.starts_with('#') && !r.is_empty())
    {
        let colonne: Vec<&str> = riga.split('\t').collect();
        assert_eq!(colonne.len(), 6, "{riga}");
        assert!(
            chiavi.insert((colonne[0], colonne[1])),
            "chiave ripetuta: {riga}"
        );
        assert!(
            colonne[2].len() == 64 && colonne[2].bytes().all(|b| b.is_ascii_hexdigit()),
            "{riga}"
        );
        for esito in &colonne[4..] {
            let parti: Vec<&str> = esito.split(' ').collect();
            let ok_ben_formato = parti.len() == 3
                && parti[0] == "ok"
                && parti[1].parse::<usize>().is_ok()
                && parti[2].len() == 32
                && parti[2].bytes().all(|b| b.is_ascii_hexdigit());
            assert!(
                ok_ben_formato || esito.starts_with("err "),
                "esito mal formato: {riga}"
            );
        }
        *per_insieme.entry(colonne[0].to_owned()).or_default() += 1;
    }
    let attesi: BTreeMap<String, usize> = INSIEMI
        .iter()
        .map(|(nome, _, _, quanti)| ((*nome).to_owned(), *quanti))
        .collect();
    assert_eq!(per_insieme, attesi);
}
