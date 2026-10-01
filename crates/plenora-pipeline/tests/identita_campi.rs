//! Identità dei campi attraverso il runner (`plenora.field_id`, ARROW-003 e
//! ARROW-004): l'uscita di un piano porta versione e identità; una colonna
//! che il piano propaga o rinomina tiene quella dell'ingresso, una nuova ne
//! riceve una mai usata dagli ingressi, e un'identità che due ingressi
//! dichiarano si perde invece di passare all'altro campo.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_pipeline::Pipeline;

const ID: &str = "plenora.field_id";

fn con_id(campo: Field, id: &str) -> Field {
    campo.with_metadata(HashMap::from([(ID.to_owned(), id.to_owned())]))
}

/// `ordini(id, cliente, importo)` con le identità date (o senza).
fn ordini(identita: Option<[&str; 3]>) -> RecordBatch {
    let mut campi = vec![
        Field::new("id", DataType::Int64, false),
        Field::new("cliente", DataType::Utf8, false),
        Field::new("importo", DataType::Float64, true),
    ];
    let mut metadati = HashMap::new();
    if let Some(identita) = identita {
        campi = campi
            .into_iter()
            .zip(identita)
            .map(|(campo, id)| con_id(campo, id))
            .collect();
        metadati.insert("plenora.contract.version".to_owned(), "1".to_owned());
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(campi, metadati)),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3])),
            Arc::new(StringArray::from(vec!["a", "b", "a"])),
            Arc::new(Float64Array::from(vec![Some(10.0), None, Some(2.5)])),
        ],
    )
    .unwrap()
}

/// `clienti(cliente, nome)` con le identità date.
fn clienti(identita: [&str; 2]) -> RecordBatch {
    let campi = vec![
        con_id(Field::new("cliente", DataType::Utf8, false), identita[0]),
        con_id(Field::new("nome", DataType::Utf8, false), identita[1]),
    ];
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            campi,
            HashMap::from([("plenora.contract.version".to_owned(), "1".to_owned())]),
        )),
        vec![
            Arc::new(StringArray::from(vec!["a", "b"])),
            Arc::new(StringArray::from(vec!["Anna", "Bruno"])),
        ],
    )
    .unwrap()
}

fn esegui(piano: &str, tabelle: Vec<(&str, RecordBatch)>) -> BTreeMap<String, RecordBatch> {
    let pipeline = Pipeline::from_json(piano).expect("piano");
    let schemi: Vec<(&str, _)> = tabelle
        .iter()
        .map(|(nome, tabella)| (*nome, tabella.schema()))
        .collect();
    let validata = pipeline.validate(&schemi).expect("validazione");
    let pubblicati: BTreeMap<String, _> = pipeline
        .outputs
        .iter()
        .map(|nome| {
            (
                nome.clone(),
                validata.schema_uscita(nome).expect("schema").clone(),
            )
        })
        .collect();
    let esito = validata
        .run(
            tabelle
                .into_iter()
                .map(|(nome, tabella)| (nome.to_owned(), tabella))
                .collect(),
        )
        .expect("esecuzione");
    esito
        .outputs
        .into_iter()
        .map(|(nome, tabella)| {
            // Lo schema dell'uscita e' quello pubblicato in validazione.
            assert_eq!(&tabella.schema(), &pubblicati[&nome], "{nome}");
            (nome, tabella)
        })
        .collect()
}

/// Identità per nome di colonna.
fn identita(tabella: &RecordBatch) -> BTreeMap<String, String> {
    tabella
        .schema()
        .fields()
        .iter()
        .map(|campo| {
            (
                campo.name().clone(),
                campo
                    .metadata()
                    .get(ID)
                    .cloned()
                    .unwrap_or_else(|| panic!("`{}` senza identita'", campo.name())),
            )
        })
        .collect()
}

fn attese(coppie: &[(&str, &str)]) -> BTreeMap<String, String> {
    coppie
        .iter()
        .map(|(nome, id)| ((*nome).to_owned(), (*id).to_owned()))
        .collect()
}

#[test]
fn senza_identita_in_ingresso_l_uscita_le_riceve_in_ordine() {
    let uscite = esegui(
        r#"{"version": 1, "inputs": ["o"],
            "steps": [{"out": "f", "op": "table.filter", "in": ["o"],
                       "config": {"column": "importo", "operator": ">", "value": 0}}],
            "outputs": ["f"]}"#,
        vec![("o", ordini(None))],
    );
    let f = &uscite["f"];
    assert_eq!(
        f.schema()
            .metadata()
            .get("plenora.contract.version")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        identita(f),
        attese(&[("id", "0"), ("cliente", "1"), ("importo", "2")])
    );
}

#[test]
fn propagate_e_rinominate_tengono_l_identita_le_nuove_no() {
    let uscite = esegui(
        r#"{"version": 1, "inputs": ["o"],
            "steps": [
              {"out": "r", "op": "table.rename", "in": ["o"],
               "config": {"renames": [{"old_name": "importo", "new_name": "euro"}]}},
              {"out": "s", "op": "table.select_columns", "in": ["r"],
               "config": {"columns": ["euro", "id"]}},
              {"out": "c", "op": "table.formula", "in": ["s"],
               "config": {"new_column": "doppio", "formula": "euro * 2"}}
            ],
            "outputs": ["c"]}"#,
        vec![("o", ordini(Some(["10", "11", "12"])))],
    );
    // `euro` e' `importo` rinominato, `id` propagata; `doppio` e' nuova e
    // parte sopra la massima identita' degli ingressi (12), mai da una
    // gia' usata (11 era `cliente`, uscita dal piano).
    assert_eq!(
        identita(&uscite["c"]),
        attese(&[("euro", "12"), ("id", "10"), ("doppio", "13")])
    );
}

#[test]
fn un_identita_dichiarata_da_due_ingressi_si_perde() {
    // `ordini` e `clienti` vengono da namespace diversi e dichiarano
    // entrambi l'identita' 1: nel join la stessa cifra direbbe due campi,
    // quindi chi la porta ne riceve una nuova. Le altre restano.
    let uscite = esegui(
        r#"{"version": 1, "inputs": ["o", "k"],
            "steps": [{"out": "j", "op": "table.join", "in": ["o", "k"],
                       "config": {"left_keys": ["cliente"], "right_keys": ["cliente"],
                                  "how": "inner"}}],
            "outputs": ["j"]}"#,
        vec![
            ("o", ordini(Some(["0", "1", "2"]))),
            ("k", clienti(["1", "5"])),
        ],
    );
    // Il join rinomina le colonne non chiave (`_L`, `_R`): la rinomina
    // tiene l'identita'.
    let j = identita(&uscite["j"]);
    assert_eq!(j["id_L"], "0");
    assert_eq!(j["importo_L"], "2");
    assert_eq!(j["nome_R"], "5");
    for (nome, id) in &j {
        if nome.starts_with("cliente") {
            assert!(
                id.parse::<u32>().unwrap() > 5,
                "`{nome}` porta l'identita' ambigua o una gia' usata: {id}"
            );
        }
    }
    let mut viste: Vec<&String> = j.values().collect();
    viste.sort();
    viste.dedup();
    assert_eq!(viste.len(), j.len(), "identita' uniche nello schema: {j:?}");
}

#[test]
fn un_piano_in_catena_sull_uscita_di_un_altro_conserva_le_identita() {
    // Il secondo piano legge l'uscita del primo (che porta versione e
    // identita') e ne conserva le identita' sulle colonne che propaga.
    let primo = esegui(
        r#"{"version": 1, "inputs": ["o"], "steps": [], "outputs": ["o"]}"#,
        vec![("o", ordini(None))],
    );
    let intermedia = primo["o"].clone();
    let secondo = esegui(
        r#"{"version": 1, "inputs": ["t"],
            "steps": [{"out": "f", "op": "table.filter", "in": ["t"],
                       "config": {"column": "importo", "operator": ">", "value": 0}}],
            "outputs": ["f"]}"#,
        vec![("t", intermedia.clone())],
    );
    assert_eq!(identita(&secondo["f"]), identita(&intermedia));
}
