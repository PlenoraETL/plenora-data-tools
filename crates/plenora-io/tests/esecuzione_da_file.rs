//! Da file a file: Parquet e `GeoParquet` in ingresso, piano, Parquet e Arrow
//! in uscita; scrittura atomica.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod comune;

use std::fs::File;
use std::path::{Path, PathBuf};

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use plenora_core::arrow::array::{Array, BinaryArray, Int64Array};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::crs::resolve_crs;
use plenora_core::ErrorCategory;
use plenora_io::{
    esegui_da_file, leggi_tabella, scrivi_tabella, FileIngresso, FileUscita, OpzioniScrittura,
};
use plenora_pipeline::Pipeline;

use comune::{cartella, identiche, ordini};

const PIANO: &str = r#"{
  "version": 1,
  "inputs": ["ordini"],
  "steps": [
    {"out": "validi", "op": "table.filter", "in": ["ordini"],
     "config": {"column": "importo", "operator": ">", "value": 0}},
    {"out": "totali", "op": "table.aggregate", "in": ["validi"],
     "config": {"group_by": ["cliente"],
                "aggregations": [{"column": "importo", "function": "sum"}]}}
  ],
  "outputs": ["validi", "totali"]
}"#;

fn ingresso(nome: &str, percorso: &Path) -> FileIngresso {
    FileIngresso {
        nome: nome.to_owned(),
        percorso: percorso.to_path_buf(),
        formato: None,
    }
}

fn uscita(nome: &str, percorso: &Path) -> FileUscita {
    FileUscita {
        nome: nome.to_owned(),
        percorso: percorso.to_path_buf(),
        formato: None,
    }
}

fn fixture(nome: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("dati")
        .join(nome)
}

#[test]
fn parquet_in_piano_parquet_e_arrow_out() {
    let dir = cartella();
    let sorgente = dir.path().join("ordini.parquet");
    scrivi_tabella(&ordini(), &sorgente, &OpzioniScrittura::default()).unwrap();
    let piano = Pipeline::from_json(PIANO).unwrap();
    let validi = dir.path().join("validi.arrow");
    let totali = dir.path().join("totali.parquet");
    let report = esegui_da_file(
        &piano,
        &[ingresso("ordini", &sorgente)],
        &[uscita("validi", &validi), uscita("totali", &totali)],
        &OpzioniScrittura::default(),
    )
    .expect("esecuzione");
    assert_eq!(report.passi.len(), 2);

    // Stesso risultato del runner in memoria.
    let tabella = ordini();
    let validata = piano.validate(&[("ordini", tabella.schema())]).unwrap();
    let atteso = validata.run(vec![("ordini".to_owned(), tabella)]).unwrap();
    identiche(
        &atteso.outputs[0].1,
        &leggi_tabella(&validi, None, u64::MAX).unwrap(),
    );
    identiche(
        &atteso.outputs[1].1,
        &leggi_tabella(&totali, None, u64::MAX).unwrap(),
    );

    // Due esecuzioni, stessi byte.
    let dir2 = cartella();
    let validi2 = dir2.path().join("validi.arrow");
    let totali2 = dir2.path().join("totali.parquet");
    esegui_da_file(
        &piano,
        &[ingresso("ordini", &sorgente)],
        &[uscita("validi", &validi2), uscita("totali", &totali2)],
        &OpzioniScrittura::default(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&validi).unwrap(),
        std::fs::read(&validi2).unwrap()
    );
    assert_eq!(
        std::fs::read(&totali).unwrap(),
        std::fs::read(&totali2).unwrap()
    );
}

#[test]
fn geoparquet_in_filtro_geoparquet_out() {
    let piano = Pipeline::from_json(
        r#"{"version": 1, "inputs": ["luoghi"],
            "steps": [{"out": "scelti", "op": "table.filter", "in": ["luoghi"],
                       "config": {"column": "id", "operator": ">", "value": 1}}],
            "outputs": ["scelti"]}"#,
    )
    .unwrap();
    let dir = cartella();
    let destinazione = dir.path().join("scelti.parquet");
    esegui_da_file(
        &piano,
        &[ingresso("luoghi", &fixture("pyarrow_utm32.parquet"))],
        &[uscita("scelti", &destinazione)],
        &OpzioniScrittura::default(),
    )
    .expect("esecuzione geo");
    let letta = leggi_tabella(&destinazione, None, u64::MAX).unwrap();
    assert_eq!(letta.num_rows(), 3);
    let ids = letta
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    assert_eq!(ids.values().to_vec(), vec![2, 3, 4]);
    let celle = letta
        .column(2)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    assert!(celle.is_null(1));
    let contratto = contract_from_arrow_schema(letta.schema(), resolve_crs).unwrap();
    assert_eq!(
        plenora_io::crs_projjson::identificativo_di(
            contratto.geometries[0].crs.as_resolved().unwrap()
        )
        .unwrap(),
        "EPSG:32632"
    );
    // Il file scritto e' GeoParquet con i tipi dichiarati.
    let costruttore =
        ParquetRecordBatchReaderBuilder::try_new(File::open(&destinazione).unwrap()).unwrap();
    let geo = costruttore
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .unwrap()
        .iter()
        .find(|voce| voce.key == "geo")
        .and_then(|voce| voce.value.clone())
        .unwrap();
    let geo: serde_json::Value = serde_json::from_str(&geo).unwrap();
    assert_eq!(
        geo["columns"]["geometry"]["geometry_types"],
        serde_json::json!(["Point", "Polygon"])
    );
}

#[test]
fn budget_contato_sugli_input() {
    let dir = cartella();
    let sorgente = dir.path().join("ordini.parquet");
    scrivi_tabella(&ordini(), &sorgente, &OpzioniScrittura::default()).unwrap();
    let piano = Pipeline::from_json(
        r#"{"version": 1, "inputs": ["ordini"], "limits": {"max_governed_memory_bytes": 64},
            "steps": [], "outputs": ["ordini"]}"#,
    )
    .unwrap();
    let destinazione = dir.path().join("out.parquet");
    let errore = esegui_da_file(
        &piano,
        &[ingresso("ordini", &sorgente)],
        &[uscita("ordini", &destinazione)],
        &OpzioniScrittura::default(),
    )
    .expect_err("oltre il budget");
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit);
    assert!(errore.to_string().contains("input `ordini`"), "{errore}");
    assert!(!destinazione.exists());
}

#[test]
fn percorsi_controllati_prima_di_leggere() {
    let dir = cartella();
    let sorgente = dir.path().join("ordini.parquet");
    scrivi_tabella(&ordini(), &sorgente, &OpzioniScrittura::default()).unwrap();
    let piano = Pipeline::from_json(PIANO).unwrap();
    let validi = dir.path().join("validi.arrow");
    let totali = dir.path().join("totali.parquet");
    let esegui = |uscite: &[FileUscita], opzioni: &OpzioniScrittura| {
        esegui_da_file(&piano, &[ingresso("ordini", &sorgente)], uscite, opzioni)
    };
    let default = OpzioniScrittura::default();
    // Output senza percorso, nome estraneo, nome ripetuto.
    for uscite in [
        vec![uscita("validi", &validi)],
        vec![
            uscita("validi", &validi),
            uscita("totali", &totali),
            uscita("altro", &totali),
        ],
        vec![uscita("validi", &validi), uscita("validi", &totali)],
    ] {
        let errore = esegui(&uscite, &default).expect_err("uscite sbagliate");
        assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
    }
    // Uscita uguale all'ingresso, o a un'altra uscita.
    let errore = esegui(
        &[uscita("validi", &validi), uscita("totali", &sorgente)],
        &default,
    )
    .expect_err("uscita = ingresso");
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan);
    let errore = esegui(
        &[uscita("validi", &validi), uscita("totali", &validi)],
        &default,
    )
    .expect_err("uscite uguali");
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan);
    // Destinazione esistente: Conflict prima di eseguire, file intatto.
    std::fs::write(&totali, b"precedente").unwrap();
    let errore = esegui(
        &[uscita("validi", &validi), uscita("totali", &totali)],
        &default,
    )
    .expect_err("esistente");
    assert_eq!(errore.category(), ErrorCategory::Conflict, "{errore}");
    assert_eq!(std::fs::read(&totali).unwrap(), b"precedente");
    assert!(
        !validi.exists(),
        "nessun output scritto prima del controllo"
    );
    // Con la sovrascrittura esplicita si sostituisce.
    let sovrascrivi = OpzioniScrittura {
        sovrascrivi: true,
        ..OpzioniScrittura::default()
    };
    esegui(
        &[uscita("validi", &validi), uscita("totali", &totali)],
        &sovrascrivi,
    )
    .unwrap();
    assert!(leggi_tabella(&totali, None, u64::MAX).is_ok());
}

fn file_nella(dir: &Path) -> Vec<String> {
    let mut nomi: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|voce| voce.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    nomi.sort();
    nomi
}

#[test]
fn scrittura_atomica() {
    let dir = cartella();
    let destinazione = dir.path().join("t.parquet");
    // Esistente senza sovrascrittura: Conflict, contenuto intatto.
    std::fs::write(&destinazione, b"vecchio").unwrap();
    let errore = scrivi_tabella(&ordini(), &destinazione, &OpzioniScrittura::default())
        .expect_err("esistente");
    assert_eq!(errore.category(), ErrorCategory::Conflict);
    assert_eq!(std::fs::read(&destinazione).unwrap(), b"vecchio");
    // Errore durante la scrittura: la destinazione resta quella di prima e
    // nessun temporaneo resta nella directory.
    let senza_colonne = plenora_core::batch_with_rows(
        std::sync::Arc::new(plenora_core::arrow::schema::Schema::empty()),
        vec![],
        3,
    )
    .unwrap();
    let sovrascrivi = OpzioniScrittura {
        sovrascrivi: true,
        ..OpzioniScrittura::default()
    };
    assert!(scrivi_tabella(&senza_colonne, &destinazione, &sovrascrivi).is_err());
    assert_eq!(std::fs::read(&destinazione).unwrap(), b"vecchio");
    assert_eq!(file_nella(dir.path()), vec!["t.parquet".to_owned()]);
    // Sovrascrittura riuscita.
    scrivi_tabella(&ordini(), &destinazione, &sovrascrivi).unwrap();
    identiche(
        &ordini(),
        &leggi_tabella(&destinazione, None, u64::MAX).unwrap(),
    );
    assert_eq!(file_nella(dir.path()), vec!["t.parquet".to_owned()]);
    // Directory mancante o destinazione directory: errore, niente creato.
    let manca = dir.path().join("manca").join("t.arrow");
    assert_eq!(
        scrivi_tabella(&ordini(), &manca, &OpzioniScrittura::default())
            .expect_err("directory mancante")
            .category(),
        // `Io` con `ErrorKind::NotFound`: categoria `not_found`.
        ErrorCategory::NotFound
    );
    let sottodirectory = dir.path().join("d.arrow");
    std::fs::create_dir(&sottodirectory).unwrap();
    assert!(scrivi_tabella(&ordini(), &sottodirectory, &sovrascrivi).is_err());
    assert!(sottodirectory.is_dir());
}

#[test]
fn budget_contato_sulla_scrittura() {
    // Il budget basta per leggere ed eseguire, non per il transitorio fisso
    // della scrittura Parquet (MARGINE_SCRITTURA): errore prima di scrivere,
    // con il nome dell'output; lo stesso piano verso Arrow IPC passa.
    let dir = cartella();
    let sorgente = dir.path().join("ordini.arrow");
    scrivi_tabella(&ordini(), &sorgente, &OpzioniScrittura::default()).unwrap();
    let budget = plenora_io::parquet_io::MARGINE_SCRITTURA / 2;
    let piano = Pipeline::from_json(&format!(
        r#"{{"version": 1, "inputs": ["ordini"], "limits": {{"max_governed_memory_bytes": {budget}}},
            "steps": [{{"out": "copia", "op": "table.filter", "in": ["ordini"],
                        "config": {{"column": "id", "operator": ">", "value": 0}}}}],
            "outputs": ["copia"]}}"#
    ))
    .unwrap();
    let parquet = dir.path().join("copia.parquet");
    let errore = esegui_da_file(
        &piano,
        &[ingresso("ordini", &sorgente)],
        &[uscita("copia", &parquet)],
        &OpzioniScrittura::default(),
    )
    .expect_err("scrittura oltre il budget");
    assert_eq!(errore.category(), ErrorCategory::ResourceLimit, "{errore}");
    assert!(errore.to_string().contains("output `copia`"), "{errore}");
    assert!(!parquet.exists());
    let arrow = dir.path().join("copia.arrow");
    esegui_da_file(
        &piano,
        &[ingresso("ordini", &sorgente)],
        &[uscita("copia", &arrow)],
        &OpzioniScrittura::default(),
    )
    .expect("verso Arrow IPC");
    assert!(arrow.exists());
}

#[cfg(windows)]
#[test]
fn stesso_file_con_altre_maiuscole() {
    let dir = cartella();
    let sorgente = dir.path().join("ordini.parquet");
    scrivi_tabella(&ordini(), &sorgente, &OpzioniScrittura::default()).unwrap();
    let piano = Pipeline::from_json(PIANO).unwrap();
    let sovrascrivi = OpzioniScrittura {
        sovrascrivi: true,
        ..OpzioniScrittura::default()
    };
    // Uscita che e' l'ingresso con altre maiuscole.
    let errore = esegui_da_file(
        &piano,
        &[ingresso("ordini", &sorgente)],
        &[
            uscita("validi", &dir.path().join("v.arrow")),
            uscita("totali", &dir.path().join("ORDINI.parquet")),
        ],
        &sovrascrivi,
    )
    .expect_err("uscita = ingresso");
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
    // Due uscite sullo stesso file: la seconda non sostituisce la prima.
    let errore = esegui_da_file(
        &piano,
        &[ingresso("ordini", &sorgente)],
        &[
            uscita("validi", &dir.path().join("x.parquet")),
            uscita("totali", &dir.path().join("X.parquet")),
        ],
        &sovrascrivi,
    )
    .expect_err("uscite sullo stesso file");
    assert_eq!(errore.category(), ErrorCategory::Conflict, "{errore}");
    let rimasta = leggi_tabella(&dir.path().join("x.parquet"), None, u64::MAX).unwrap();
    assert_eq!(rimasta.num_columns(), 3, "resta l'output `validi`");
}
