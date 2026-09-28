//! Presidio STRUTTURALE dell'invariante «un batch a zero colonne puo' avere
//! righe».
//!
//! Arrow non deduce la cardinalita' quando non ci sono colonne da cui
//! dedurla: `RecordBatch::try_new` — e `try_new_with_options` senza row
//! count — **rifiutano** di costruire il batch
//! (`must either specify a row count or at least one column`). Il difetto
//! non e' quindi una perdita silenziosa di righe: e' un'operazione legittima
//! che FALLISCE. Un difetto di disponibilita', non di correttezza.
//!
//! Ogni ricostruzione le cui colonne DERIVANO dall'input deve percio' passare
//! da [`plenora_core::batch_with_rows`], che dichiara la cardinalita' per il
//! caso vuoto e lascia decidere alle colonne quando ci sono.
//!
//! Questo controllo e' sul SORGENTE e non sul comportamento, per una ragione
//! precisa: la classe ricompare ogni volta in un punto nuovo — i kernel
//! tabellari, l'engine — e un test per sito non impedisce al prossimo sito
//! di nascere sbagliato. Un controllo sulla forma del codice si', perche'
//! fallisce quando qualcuno lo scrive.
//!
//! # Che cosa questo presidio NON e'
//!
//! Un filtro sintattico, non una dimostrazione. Guarda una finestra di
//! contesto attorno alla costruzione: un sito che deriva le colonne piu'
//! lontano gli sfugge. Genera anche falsi positivi, e la cura e' passare da
//! `batch_with_rows`, che e' corretto anche quando non serve.
//!
//! E' pero' **fail-closed**: qualunque errore di scansione o di lettura fa
//! fallire il test invece di ridurre silenziosamente il perimetro. Un gate
//! che ignora un `read_dir` fallito e' un gate che puo' diventare verde
//! smettendo di guardare.

use std::path::{Path, PathBuf};

/// Derivazioni che indicano colonne provenienti dall'input.
const DERIVAZIONI: [&str; 4] = [
    ".columns().to_vec()",
    ".columns()\n",
    "batch.schema()",
    "output.schema()",
];

/// Costruttori che possono non dichiarare la cardinalita'.
///
/// `try_new_with_options` e' incluso: le opzioni possono NON contenere il row
/// count, e in quel caso si comporta come `try_new`. Chi lo usa con
/// `with_row_count` sta gia' dichiarando la cardinalita', ma sta anche
/// duplicando `batch_with_rows`: passare da li' resta la forma giusta.
const COSTRUTTORI: [&str; 2] = [
    "RecordBatch::try_new(",
    "RecordBatch::try_new_with_options(",
];

/// Righe di contesto guardate all'indietro dalla costruzione.
const FINESTRA: usize = 14;

fn radice_workspace() -> PathBuf {
    // `CARGO_MANIFEST_DIR` e' `crates/plenora-core`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("radice del workspace")
        .to_path_buf()
}

/// Elenca i sorgenti di produzione. Fallisce su qualunque errore di
/// scansione: un perimetro ridotto in silenzio renderebbe il gate inutile.
fn sorgenti(radice: &Path) -> Vec<PathBuf> {
    let mut trovati = Vec::new();
    let mut da_visitare = vec![radice.join("crates")];
    while let Some(directory) = da_visitare.pop() {
        let voci = std::fs::read_dir(&directory)
            .unwrap_or_else(|errore| panic!("scansione di {}: {errore}", directory.display()));
        for elemento in voci {
            let elemento = elemento
                .unwrap_or_else(|errore| panic!("voce in {}: {errore}", directory.display()));
            let percorso = elemento.path();
            let tipo = elemento
                .file_type()
                .unwrap_or_else(|errore| panic!("tipo di {}: {errore}", percorso.display()));
            if tipo.is_dir() {
                // I `target*` contengono sorgenti generati e dipendenze.
                if percorso
                    .file_name()
                    .and_then(|nome| nome.to_str())
                    .is_some_and(|nome| nome.starts_with("target"))
                {
                    continue;
                }
                da_visitare.push(percorso);
            } else if tipo.is_file()
                && percorso.extension().and_then(|e| e.to_str()) == Some("rs")
                // I file di test — sia i moduli `*/tests.rs` sia i test
                // d'integrazione sotto `tests/` — costruiscono fixture con
                // colonne letterali: li' i costruttori nudi sono legittimi.
                && !percorso.components().any(|componente| {
                    componente.as_os_str() == "tests" || componente.as_os_str() == "benches"
                })
                && !percorso.ends_with("tests.rs")
            {
                trovati.push(percorso);
            }
        }
    }
    assert!(
        !trovati.is_empty(),
        "nessun sorgente trovato sotto {}: il gate non sta guardando nulla",
        radice.join("crates").display()
    );
    trovati
}

/// `true` se la riga cade dentro un modulo `#[cfg(test)] mod ...`.
fn dentro_i_test(righe: &[&str], indice: usize) -> bool {
    righe[..=indice].iter().enumerate().any(|(i, riga)| {
        riga.trim() == "#[cfg(test)]"
            && righe
                .get(i + 1)
                .is_some_and(|successiva| successiva.trim_start().starts_with("mod "))
    })
}

#[test]
fn nessuna_ricostruzione_da_input_usa_un_costruttore_nudo() {
    let radice = radice_workspace();
    let sorgenti = sorgenti(&radice);
    let mut sospetti = Vec::new();
    let mut esaminati = 0_usize;

    for percorso in sorgenti {
        // Il presidio non si applica a se stesso.
        if percorso.ends_with("cardinalita_zero_colonne.rs") {
            continue;
        }
        let contenuto = std::fs::read_to_string(&percorso)
            .unwrap_or_else(|errore| panic!("lettura di {}: {errore}", percorso.display()));
        esaminati += 1;
        let righe: Vec<&str> = contenuto.split('\n').collect();
        for (indice, riga) in righe.iter().enumerate() {
            if !COSTRUTTORI.iter().any(|nudo| riga.contains(nudo)) {
                continue;
            }
            if dentro_i_test(&righe, indice) {
                continue;
            }
            let inizio = indice.saturating_sub(FINESTRA);
            let finestra = righe[inizio..=indice].join("\n");
            if DERIVAZIONI
                .iter()
                .any(|derivazione| finestra.contains(derivazione))
            {
                sospetti.push(format!(
                    "{}:{}",
                    percorso
                        .strip_prefix(&radice)
                        .unwrap_or(&percorso)
                        .display(),
                    indice + 1
                ));
            }
        }
    }

    assert!(
        esaminati > 20,
        "solo {esaminati} sorgenti esaminati: il perimetro si e' ristretto, \
         il gate non sta piu' presidiando"
    );
    assert!(
        sospetti.is_empty(),
        "queste ricostruzioni derivano le colonne dall'input e usano un \
         costruttore che non dichiara la cardinalita': con zero colonne arrow \
         RIFIUTA di costruire il batch, e un'operazione legittima fallisce. \
         Usare `plenora_core::batch_with_rows`, che e' corretto anche quando \
         le colonne ci sono.\n  {}",
        sospetti.join("\n  ")
    );
}

#[test]
fn il_costruttore_condiviso_conserva_le_righe_senza_colonne() {
    use plenora_core::arrow::array::{Int64Array, RecordBatch, RecordBatchOptions};
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use std::sync::Arc;

    // Il caso che i costruttori nudi non sanno costruire.
    let vuoto = plenora_core::batch_with_rows(Arc::new(Schema::empty()), Vec::new(), 7)
        .expect("batch senza colonne");
    assert_eq!(vuoto.num_rows(), 7, "la cardinalita' dichiarata vale");

    // PREMESSA VERIFICATA, non assunta. Arrow non restituisce zero righe in
    // silenzio: senza colonne e senza una cardinalita' dichiarata RIFIUTA di
    // costruire il batch. La classe di difetto e' quindi un'operazione
    // legittima che FALLISCE, non un numero sbagliato reso senza errore.
    let errore = RecordBatch::try_new_with_options(
        Arc::new(Schema::empty()),
        Vec::new(),
        &RecordBatchOptions::new(),
    )
    .expect_err("arrow rifiuta un batch senza colonne e senza row count");
    assert!(
        errore
            .to_string()
            .contains("must either specify a row count or at least one column"),
        "premessa: arrow lo dice esplicitamente, non tace: {errore}"
    );

    // E il caso normale resta identico: quando le colonne ci sono, decidono
    // loro, e il parametro viene ignorato.
    let con_colonne = plenora_core::batch_with_rows(
        Arc::new(Schema::new(vec![Field::new("a", DataType::Int64, false)])),
        vec![Arc::new(Int64Array::from(vec![1_i64, 2, 3]))],
        999,
    )
    .expect("batch con colonne");
    assert_eq!(
        con_colonne.num_rows(),
        3,
        "le colonne, quando ci sono, decidono: il parametro non puo' \
         introdurre un numero sbagliato"
    );
}
