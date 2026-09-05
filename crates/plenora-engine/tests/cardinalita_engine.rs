//! La cardinalita' dei batch senza colonne nei percorsi dell'ENGINE.
//!
//! Il costruttore che dichiara le righe vive in `plenora-core`: un
//! invariante del workspace non puo' abitare in una foglia, perche' chi non
//! dipende da quella foglia non lo vede. I tre punti dell'engine che lo
//! esercitano sono il rivestimento dello schema in pubblicazione, la
//! compattazione dello staging e la normalizzazione `LargeUtf8`.
//!
//! Questi test esercitano i percorsi dal loro ingresso pubblico. Il presidio
//! strutturale che impedisce a nuovi siti di nascere sbagliati sta in
//! `plenora-core/tests/cardinalita_zero_colonne.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::array::{RecordBatch, RecordBatchOptions};
use plenora_core::arrow::schema::Schema;
use plenora_engine::table_engine::{Plan, ValidatedPlan};

/// Batch con zero colonne e `righe` righe: legittimo in Arrow.
fn senza_colonne(righe: usize, metadata: HashMap<String, String>) -> RecordBatch {
    let opzioni = RecordBatchOptions::new().with_row_count(Some(righe));
    RecordBatch::try_new_with_options(
        Arc::new(Schema::empty().with_metadata(metadata)),
        Vec::new(),
        &opzioni,
    )
    .expect("batch senza colonne")
}

/// Piano con un passo che non tocca le colonne: `limit` che tiene tutto.
///
/// Un piano senza passi non e' valido, e serve comunque attraversare la
/// normalizzazione — che avviene PRIMA dei passi — e la ricostruzione finale.
fn piano_identita() -> ValidatedPlan {
    let plan: Plan = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "steps": [{"operation": "table.limit", "config": {"n": 1_000_000, "offset": 0}}]
    }))
    .expect("piano");
    plan.validate().expect("piano valido")
}

#[test]
fn la_normalizzazione_conserva_le_righe_senza_colonne_con_metadata_pandas() {
    // `normalize_large_utf8` ricostruisce il batch per rimuovere il metadata
    // `pandas`. Le colonne derivano dall'input: con zero colonne una
    // ricostruzione che non dichiari la cardinalita' fallisce, perche' arrow
    // rifiuta un batch senza colonne e senza cardinalita'. Il metadata `pandas` e' la
    // condizione che fa scattare la ricostruzione — senza, la funzione
    // restituisce l'input inalterato e il difetto non si vede.
    let mut metadata = HashMap::new();
    metadata.insert("pandas".to_owned(), "{}".to_owned());
    let ingresso = senza_colonne(5, metadata);
    assert_eq!(ingresso.num_rows(), 5, "premessa");

    let uscita = plenora_engine::table_engine::execute_batch(ingresso, &piano_identita())
        .expect("esecuzione del piano identita'");
    assert_eq!(
        uscita.num_rows(),
        5,
        "la normalizzazione conserva la cardinalita' invece di fallire"
    );
    assert!(
        !uscita.schema().metadata().contains_key("pandas"),
        "il metadata pandas viene comunque rimosso"
    );
}

#[test]
fn l_esecuzione_completa_conserva_le_righe_senza_colonne() {
    // Stesso invariante sul percorso blocking, che attraversa anche la
    // validazione del batch e la ricostruzione finale.
    let mut metadata = HashMap::new();
    metadata.insert("pandas".to_owned(), "{}".to_owned());
    let ingresso = senza_colonne(3, metadata);

    let uscita = plenora_engine::table_engine::execute_complete_batch(ingresso, &piano_identita())
        .expect("esecuzione completa");
    assert_eq!(uscita.num_rows(), 3, "righe conservate anche in blocking");
}
