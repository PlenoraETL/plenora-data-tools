#![no_main]

//! Piano: testo arbitrario in `Pipeline::from_json`, poi la validazione
//! contro uno schema fisso per ogni ingresso dichiarato.
//!
//! Invarianti: mai panico; un rifiuto della lettura è `InvalidPlan`; un piano
//! letto sopravvive al proprio round-trip (serializzato e riletto è uguale),
//! o, se la forma serializzata supera `max_plan_json_bytes` (una `config`
//! omessa si scrive `{}`), è rifiutato come `InvalidPlan`; la validazione è
//! deterministica e non produce `Internal`, salvo quello documentato di una
//! dipendenza in barriera (un WKB di config la cui validazione OGC non
//! conclude).

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::limits::PlanLimits;
use plenora_core::ErrorCategory;
use plenora_pipeline::Pipeline;

#[path = "comune/aggancio.rs"]
mod aggancio;
#[path = "comune/esiti.rs"]
mod esiti;

/// Lo schema di ogni ingresso dichiarato: abbastanza colonne di tipi diversi
/// perché le config dei passi trovino quello che nominano.
fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("value", DataType::Float64, true),
        Field::new("flag", DataType::Boolean, true),
        Field::new("date", DataType::Utf8, true),
        Field::new("wkt", DataType::Utf8, true),
    ]))
}

fuzz_target!(init: aggancio::installa(), |dati: &[u8]| {
    let barriere = aggancio::panici_in_barriera();
    let Ok(testo) = std::str::from_utf8(dati) else {
        return;
    };
    let piano = match Pipeline::from_json(testo) {
        Ok(piano) => piano,
        Err(errore) => {
            assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
            return;
        }
    };

    let serializzato = serde_json::to_string(&piano).expect("piano serializzabile");
    match Pipeline::from_json(&serializzato) {
        Ok(riletto) => assert_eq!(riletto, piano),
        Err(errore) => {
            assert!(serializzato.len() > PlanLimits::default().max_plan_json_bytes);
            assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
        }
    }

    let schema = schema();
    let schemi: Vec<(&str, SchemaRef)> = piano
        .inputs
        .iter()
        .map(|nome| (nome.as_str(), Arc::clone(&schema)))
        .collect();
    let primo = piano.validate(&schemi);
    let secondo = piano.validate(&schemi);
    match (&primo, &secondo) {
        (Ok(primo), Ok(secondo)) => {
            for uscita in &piano.outputs {
                assert_eq!(primo.schema_uscita(uscita), secondo.schema_uscita(uscita));
                assert!(primo.schema_uscita(uscita).is_some(), "uscita senza schema");
            }
        }
        (Err(primo), Err(secondo)) => {
            esiti::stesso_errore("validazione", primo, secondo);
            esiti::errore_ammesso("validazione", primo, barriere);
        }
        _ => panic!("validazione non deterministica"),
    }
});
