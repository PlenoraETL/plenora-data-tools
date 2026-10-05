//! `null` nelle config non è mai il testo vuoto: fino alla 1.1.0 il `value`
//! di `table.filter` e delle condizioni di `table.conditional`, i `result` e
//! il `default_value` di `conditional` e i valori di `mapping` di
//! `table.lookup` scritti `null` valevano `""`. `{"operator": "==", "value":
//! null}` teneva così le celle `""` e non le celle nulle, e un `null` scelto
//! come uscita diventava una cella `""`.
//!
//! Dalla 2.0.0:
//! - `value` `null`, o assente con un operatore che lo legge, si rifiuta
//!   con `InvalidPlan` in validazione e nel kernel (le celle nulle si
//!   cercano con `isnull`/`notnull`, il testo vuoto si scrive `""`);
//! - un `result`, `default_value` o valore di `mapping` `null` dà la cella
//!   nulla (come `THEN NULL` di SQL), e `""` resta il testo vuoto; lo schema
//!   validato lo dichiara (`nullable`) e coincide con quello eseguito.
//!
//! Ogni prova attraversa `Pipeline::validate` e `run`, e la chiamata diretta
//! del kernel dove il rifiuto deve valere anche lì.

mod comune_geo;

use std::sync::Arc;

use plenora_core::arrow::array::{Array, ArrayRef, Float64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::PlenoraError;
use plenora_kernels_table::filtering::{self, MESSAGGIO_VALUE_NULL};
use serde_json::{json, Value};

use comune_geo::{passo, piano};

/// Una colonna `s` `utf8` con una cella nulla, una vuota e una piena.
fn tabella() -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, true)]));
    let colonna: ArrayRef = Arc::new(StringArray::from(vec![None, Some(""), Some("a")]));
    RecordBatch::try_new(schema, vec![colonna]).expect("tabella")
}

fn piano_di(op: &str, config: &Value) -> plenora_pipeline::Pipeline {
    piano(&["t"], vec![passo("x", op, &["t"], config.clone())], &["x"])
}

fn valida(op: &str, config: &Value) -> Result<(), PlenoraError> {
    let schemi: Vec<(&str, SchemaRef)> = vec![("t", tabella().schema())];
    piano_di(op, config).validate(&schemi).map(|_| ())
}

/// Esegue il passo: lo schema validato deve essere quello dell'uscita.
fn esegui(op: &str, config: &Value) -> RecordBatch {
    let schemi: Vec<(&str, SchemaRef)> = vec![("t", tabella().schema())];
    let validata = piano_di(op, config).validate(&schemi).expect("validazione");
    let mut esito = validata
        .run(vec![("t".to_owned(), tabella())])
        .expect("esecuzione");
    esito.outputs.remove(0).1
}

fn messaggio(errore: &PlenoraError) -> Option<String> {
    match errore {
        PlenoraError::InvalidPlan(testo) => Some(testo.clone()),
        PlenoraError::Tagged { source, .. } => messaggio(source),
        _ => None,
    }
}

fn rifiutata(op: &str, config: &Value, frammento: &str) {
    let errore = valida(op, config).expect_err("config da rifiutare");
    let testo = messaggio(&errore).unwrap_or_else(|| panic!("{op}: non InvalidPlan: {errore:?}"));
    assert!(testo.contains(frammento), "{op}: {testo}");
}

fn testi(batch: &RecordBatch, colonna: &str) -> Vec<Option<String>> {
    let indice = batch.schema().index_of(colonna).expect("colonna");
    let valori = batch
        .column(indice)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("utf8");
    (0..valori.len())
        .map(|riga| (!valori.is_null(riga)).then(|| valori.value(riga).to_owned()))
        .collect()
}

fn nullable(batch: &RecordBatch, colonna: &str) -> bool {
    batch
        .schema()
        .field_with_name(colonna)
        .expect("colonna")
        .is_nullable()
}

#[test]
fn filter_rifiuta_value_null_e_assente() {
    for operatore in [
        "==",
        "!=",
        "contains",
        "startswith",
        "endswith",
        ">",
        "between",
    ] {
        rifiutata(
            "table.filter",
            &json!({"column": "s", "operator": operatore, "value": null}),
            MESSAGGIO_VALUE_NULL,
        );
        rifiutata(
            "table.filter",
            &json!({"column": "s", "operator": operatore}),
            "value obbligatorio",
        );
    }
    // Lo stesso rifiuto nel kernel, senza passare dalla validazione.
    let config: filtering::Filter =
        serde_json::from_value(json!({"column": "s", "operator": "==", "value": null}))
            .expect("config");
    let errore = filtering::filter(&tabella(), &config).expect_err("kernel");
    assert_eq!(messaggio(&errore).as_deref(), Some(MESSAGGIO_VALUE_NULL));
    let config: filtering::Filter =
        serde_json::from_value(json!({"column": "s", "operator": "!="})).expect("config");
    assert!(filtering::filter(&tabella(), &config).is_err());
}

#[test]
fn filter_distingue_testo_vuoto_e_cella_nulla() {
    let vuote = esegui(
        "table.filter",
        &json!({"column": "s", "operator": "==", "value": ""}),
    );
    assert_eq!(testi(&vuote, "s"), vec![Some(String::new())]);
    let nulle = esegui(
        "table.filter",
        &json!({"column": "s", "operator": "isnull"}),
    );
    assert_eq!(testi(&nulle, "s"), vec![None]);
}

#[test]
fn conditional_rifiuta_value_null_e_assente() {
    rifiutata(
        "table.conditional",
        &json!({"column": "s", "conditions": [{"value": null, "result": "x"}]}),
        MESSAGGIO_VALUE_NULL,
    );
    rifiutata(
        "table.conditional",
        &json!({"column": "s", "conditions": [{"operator": "==", "result": "x"}]}),
        "value obbligatorio",
    );
    let config: filtering::Conditional = serde_json::from_value(
        json!({"column": "s", "conditions": [{"value": null, "result": "x"}]}),
    )
    .expect("config");
    let errore = filtering::conditional(&tabella(), &config).expect_err("kernel");
    assert_eq!(messaggio(&errore).as_deref(), Some(MESSAGGIO_VALUE_NULL));
}

#[test]
fn conditional_null_da_la_cella_nulla_anche_in_uscita_testuale() {
    // `default_value` assente: `null`, la cella nulla (CASE senza ELSE).
    let uscita = esegui(
        "table.conditional",
        &json!({"column": "s", "conditions": [{"value": "a", "result": "uno"}]}),
    );
    assert_eq!(
        testi(&uscita, "result"),
        vec![None, None, Some("uno".into())]
    );
    assert!(nullable(&uscita, "result"));
    // `result` null scritto, `""` come default: restano distinti.
    let uscita = esegui(
        "table.conditional",
        &json!({
            "column": "s",
            "conditions": [{"value": "a", "result": null}, {"value": "", "result": "vuota"}],
            "default_value": ""
        }),
    );
    assert_eq!(
        testi(&uscita, "result"),
        vec![Some(String::new()), Some("vuota".into()), None]
    );
    assert!(nullable(&uscita, "result"));
    // Nessun null fra i risultati: la colonna resta non nullable.
    let uscita = esegui(
        "table.conditional",
        &json!({"column": "s", "conditions": [{"value": "a", "result": "x"}], "default_value": "y"}),
    );
    assert_eq!(
        testi(&uscita, "result"),
        vec![Some("y".into()), Some("y".into()), Some("x".into())]
    );
    assert!(!nullable(&uscita, "result"));
}

#[test]
fn conditional_testo_vuoto_non_e_un_numero_nullo() {
    // Fino alla 1.1.0 `""` accanto a un numero dava un Float64 con null.
    let uscita = esegui(
        "table.conditional",
        &json!({"column": "s", "conditions": [{"value": "a", "result": 1}], "default_value": ""}),
    );
    assert_eq!(
        testi(&uscita, "result"),
        vec![Some(String::new()), Some(String::new()), Some("1".into())]
    );
    // `null` accanto a un numero resta il Float64 nullable.
    let uscita = esegui(
        "table.conditional",
        &json!({"column": "s", "conditions": [{"value": "a", "result": 1}], "default_value": null}),
    );
    let indice = uscita.schema().index_of("result").expect("colonna");
    let numeri = uscita
        .column(indice)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("float64");
    assert_eq!(
        (0..numeri.len())
            .map(|riga| (!numeri.is_null(riga)).then(|| numeri.value(riga)))
            .collect::<Vec<_>>(),
        vec![None, None, Some(1.0)]
    );
}

#[test]
fn lookup_valore_null_da_la_cella_nulla() {
    let uscita = esegui(
        "table.lookup",
        &json!({"column": "s", "mapping": {"a": null, "": "vuota"}, "output_column": "o"}),
    );
    assert_eq!(testi(&uscita, "o"), vec![None, Some("vuota".into()), None]);
    let uscita = esegui(
        "table.lookup",
        &json!({"column": "s", "mapping": {"a": ""}, "default": "altro", "output_column": "o"}),
    );
    assert_eq!(
        testi(&uscita, "o"),
        vec![None, Some("altro".into()), Some(String::new())]
    );
}

/// Lo schema del kernel, senza il runner (che rimette sul batch quello del
/// contratto): la colonna `utf8` è nullable solo se un risultato è `null`.
#[test]
fn conditional_nullable_nel_kernel() {
    let kernel = |config: Value| {
        let config: filtering::Conditional = serde_json::from_value(config).expect("config");
        let uscita = filtering::conditional(&tabella(), &config).expect("kernel");
        nullable(&uscita, "result")
    };
    assert!(kernel(
        json!({"column": "s", "conditions": [{"value": "a", "result": "x"}]})
    ));
    assert!(kernel(
        json!({"column": "s", "conditions": [{"value": "a", "result": null}], "default_value": "y"})
    ));
    assert!(!kernel(
        json!({"column": "s", "conditions": [{"value": "a", "result": "x"}], "default_value": "y"})
    ));
}

#[test]
fn il_messaggio_del_rifiuto_e_leggibile() {
    assert!(!MESSAGGIO_VALUE_NULL.contains("  "));
    assert!(MESSAGGIO_VALUE_NULL.contains("le celle nulle si cercano con isnull e notnull"));
}
