//! Unit test della migrazione v4 -> v5 (ADR 15).
//!
//! Le prove sono organizzate per promessa, non per funzione: il contratto che
//! il blocco introduce e' «nessun alias, migrazione esplicita, canonico
//! unico», e ogni test dice quale meta' di quella frase difende.

use serde_json::{json, Value};

use plenora_core::limits::PlanLimits;

use super::*;
use crate::plan::{PlanV5, PLAN_SCHEMA_VERSION_V5};

/// Piano nella forma dichiarata, con il blocco `limits` fornito dal chiamante.
fn piano(versione: u16, limiti: &Value) -> String {
    json!({
        "schema_version": versione,
        "inputs": ["main"],
        "limits": limiti,
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}}
        ],
        "output": "a"
    })
    .to_string()
}

fn errore_di(testo: &str) -> String {
    match testo_canonico_v5(testo, &PlanLimits::default()) {
        Err(errore) => errore.to_string(),
        Ok(_) => panic!("il piano doveva essere rifiutato"),
    }
}

fn canonico(testo: &str) -> Value {
    let migrato = testo_canonico_v5(testo, &PlanLimits::default()).expect("migrazione");
    PlanV5::parse_default(migrato.as_ref())
        .expect("piano valido")
        .canonical_json()
}

// ---------------------------------------------------------------------------
// Nessun alias: i tre modi di sbagliare nome falliscono tutti
// ---------------------------------------------------------------------------

#[test]
fn un_piano_v5_col_nome_della_v4_e_rifiutato() {
    let testo = piano(5, &json!({"max_memory_bytes": 4096}));
    // Il rifiuto arriva dal parser v5, non dalla migrazione: un piano v5 non
    // passa MAI dalla migrazione, altrimenti l'alias esisterebbe di fatto.
    let migrato = testo_canonico_v5(&testo, &PlanLimits::default()).expect("dispatch");
    assert_eq!(migrato.as_ref(), testo, "un v5 attraversa senza copia");
    let errore = PlanV5::parse_default(migrato.as_ref())
        .expect_err("il nome della v4 non esiste nella v5")
        .to_string();
    assert!(errore.contains("max_memory_bytes"), "{errore}");
}

#[test]
fn un_piano_v4_col_nome_della_v5_e_rifiutato() {
    let errore = errore_di(&piano(4, &json!({"max_governed_memory_bytes": 4096})));
    assert!(errore.contains("max_governed_memory_bytes"), "{errore}");
    assert!(errore.contains("non ha alias"), "{errore}");
}

#[test]
fn entrambe_le_chiavi_insieme_sono_rifiutate_in_ogni_versione() {
    let limiti = json!({"max_memory_bytes": 4096, "max_governed_memory_bytes": 4096});

    // v4: la struttura v4 non conosce il nome nuovo.
    let errore = errore_di(&piano(4, &limiti));
    assert!(errore.contains("max_governed_memory_bytes"), "{errore}");

    // v5: la struttura v5 non conosce il nome vecchio. Non e' una seconda
    // regola, e' la stessa: qualunque versione si dichiari, una delle due
    // chiavi e' sconosciuta a chi deserializza.
    let testo = piano(5, &limiti);
    let errore = PlanV5::parse_default(&testo)
        .expect_err("le due chiavi non convivono")
        .to_string();
    assert!(errore.contains("max_memory_bytes"), "{errore}");
}

// ---------------------------------------------------------------------------
// Fail-closed PRIMA della migrazione
// ---------------------------------------------------------------------------

#[test]
fn chiavi_duplicate_rifiutate_prima_della_migrazione() {
    // Due volte lo stesso nome: `serde_json` sceglierebbe l'ultima, e la
    // scelta avverrebbe prima della migrazione — due testi diversi
    // produrrebbero lo stesso piano migrato.
    let testo = r#"{"schema_version":4,"inputs":["main"],
        "limits":{"max_memory_bytes":1024,"max_memory_bytes":2048},
        "nodes":[{"id":"a","op":"table.filter","in":["main"],"config":{}}],
        "output":"a"}"#;
    let errore = errore_di(testo);
    assert!(errore.contains("duplicat"), "{errore}");
}

#[test]
fn chiavi_duplicate_al_livello_alto_rifiutate_prima_della_migrazione() {
    let testo = r#"{"schema_version":4,"schema_version":5,"inputs":["main"],
        "nodes":[{"id":"a","op":"table.filter","in":["main"],"config":{}}],
        "output":"a"}"#;
    let errore = errore_di(testo);
    assert!(errore.contains("duplicat"), "{errore}");
}

#[test]
fn chiave_sconosciuta_nei_limiti_rifiutata_prima_della_migrazione() {
    let errore = errore_di(&piano(4, &json!({"max_memoria_bytes": 4096})));
    assert!(errore.contains("max_memoria_bytes"), "{errore}");
}

#[test]
fn chiave_sconosciuta_al_livello_alto_rifiutata_dal_parser_v5() {
    // La migrazione non guarda il resto del piano: il rifiuto arriva dal
    // parser v5, che e' l'unico posto in cui la struttura del piano vive.
    let testo = r#"{"schema_version":4,"inputs":["main"],"sconosciuto":1,
        "nodes":[{"id":"a","op":"table.filter","in":["main"],"config":{}}],
        "output":"a"}"#;
    let migrato = testo_canonico_v5(testo, &PlanLimits::default()).expect("migrazione");
    let errore = PlanV5::parse_default(migrato.as_ref())
        .expect_err("chiave sconosciuta")
        .to_string();
    assert!(errore.contains("sconosciuto"), "{errore}");
}

#[test]
fn il_tetto_sui_byte_precede_la_migrazione() {
    let testo = piano(4, &json!({"max_memory_bytes": 4096}));
    let stretti = PlanLimits {
        max_plan_json_bytes: testo.len() - 1,
        ..PlanLimits::default()
    };
    let errore = testo_canonico_v5(&testo, &stretti)
        .expect_err("sopra il tetto")
        .to_string();
    assert!(errore.contains("max_plan_json_bytes"), "{errore}");
}

// ---------------------------------------------------------------------------
// Versioni fuori dalla migrazione
// ---------------------------------------------------------------------------

#[test]
fn le_versioni_legacy_non_hanno_un_percorso_dag_diretto() {
    for versione in [1_u16, 2, 3] {
        let errore = errore_di(&piano(versione, &json!({})));
        assert!(errore.contains("legacy"), "v{versione}: {errore}");
    }
}

#[test]
fn una_versione_futura_e_rifiutata() {
    let errore = errore_di(&piano(6, &json!({})));
    assert!(errore.contains("non e' supportata"), "{errore}");
}

#[test]
fn una_schema_version_assente_o_non_intera_e_rifiutata() {
    let errore = errore_di(r#"{"inputs":["main"],"output":"a"}"#);
    assert!(errore.contains("non dichiara"), "{errore}");
    let errore = errore_di(r#"{"schema_version":"4","inputs":["main"],"output":"a"}"#);
    assert!(errore.contains("intero"), "{errore}");
    let errore = errore_di(r#"{"schema_version":-1,"inputs":["main"],"output":"a"}"#);
    assert!(errore.contains("intero"), "{errore}");
    let errore = errore_di(r#"{"schema_version":70000,"inputs":["main"],"output":"a"}"#);
    assert!(errore.contains("fuori intervallo"), "{errore}");
}

// ---------------------------------------------------------------------------
// Determinismo e idempotenza
// ---------------------------------------------------------------------------

#[test]
fn la_migrazione_e_deterministica() {
    let testo = piano(
        4,
        &json!({"max_memory_bytes": 4096, "max_temp_bytes": 8192}),
    );
    let primo = migra_v4_a_v5(&testo).expect("migrazione");
    for _ in 0..8 {
        assert_eq!(migra_v4_a_v5(&testo).expect("migrazione"), primo);
    }
}

#[test]
fn il_dispatch_e_idempotente() {
    let testo = piano(4, &json!({"max_memory_bytes": 4096}));
    let limiti = PlanLimits::default();
    let una = testo_canonico_v5(&testo, &limiti).expect("prima passata");
    let due = testo_canonico_v5(una.as_ref(), &limiti).expect("seconda passata");
    assert_eq!(una.as_ref(), due.as_ref());
}

#[test]
fn migrare_un_piano_gia_migrato_e_un_errore_esplicito() {
    // `migra_v4_a_v5` da sola non deve «riuscire» su un piano v5: un successo
    // silenzioso qui renderebbe indistinguibile un piano migrato una volta da
    // uno migrato due, e nasconderebbe l'errore di chi chiama.
    let migrato = migra_v4_a_v5(&piano(4, &json!({"max_memory_bytes": 4096}))).expect("migrazione");
    let errore = migra_v4_a_v5(&migrato)
        .expect_err("gia' migrato")
        .to_string();
    assert!(errore.contains("accetta solo"), "{errore}");
}

// ---------------------------------------------------------------------------
// Equivalenza col v5 scritto a mano
// ---------------------------------------------------------------------------

#[test]
fn un_v4_e_il_v5_equivalente_hanno_lo_stesso_piano_canonico() {
    let da_v4 = canonico(&piano(4, &json!({"max_memory_bytes": 4096})));
    let da_v5 = canonico(&piano(5, &json!({"max_governed_memory_bytes": 4096})));
    assert_eq!(da_v4, da_v5);
    assert_eq!(da_v4["schema_version"], json!(PLAN_SCHEMA_VERSION_V5));
    assert_eq!(da_v4["limits"]["max_governed_memory_bytes"], json!(4096));
}

#[test]
fn un_v4_senza_limiti_prende_i_default_canonici() {
    let da_v4 = canonico(&piano(4, &json!({})));
    let da_v5 = canonico(&piano(5, &json!({})));
    assert_eq!(da_v4, da_v5);
    assert_eq!(
        da_v4["limits"]["max_governed_memory_bytes"],
        json!(512_u64 * 1024 * 1024),
        "il default canonico e' quello di Limits::default"
    );
}

#[test]
fn la_migrazione_non_perde_gli_altri_override() {
    // Se un campo cadesse durante la traduzione, il piano girerebbe sotto un
    // limite che non ha chiesto. La forma canonica dei due deve coincidere
    // campo per campo, non solo sul budget di memoria.
    let limiti_v4 = json!({
        "max_input_rows": 10,
        "max_output_rows": 11,
        "max_rows_per_edge": 12,
        "max_expansion_factor": 1.5,
        "plan": {"max_plan_nodes": 7},
        "max_memory_bytes": 4096,
        "max_temp_bytes": 8192,
        "spill_partitions": 3,
        "max_parallelism": 2,
        "max_wkb_cell_bytes": 1024,
        "max_payload_bytes": 2048,
        "max_batches": 5,
        "max_geometry_depth": 4,
        "max_string_bytes": 256,
        "max_regex_bytes": 128,
    });
    let mut limiti_v5 = limiti_v4.clone();
    let oggetto = limiti_v5.as_object_mut().expect("oggetto");
    let budget = oggetto.remove("max_memory_bytes").expect("budget");
    oggetto.insert("max_governed_memory_bytes".to_owned(), budget);

    let da_v4 = canonico(&piano(4, &limiti_v4));
    let da_v5 = canonico(&piano(5, &limiti_v5));
    assert_eq!(da_v4, da_v5);
    assert_eq!(da_v4["limits"]["max_input_rows"], json!(10));
    assert_eq!(da_v4["limits"]["plan"]["max_plan_nodes"], json!(7));
    assert_eq!(da_v4["limits"]["max_regex_bytes"], json!(128));
}

#[test]
fn la_migrazione_non_riordina_ne_normalizza_il_resto_del_piano() {
    // Normalizzare e' compito della canonicalizzazione: farlo anche qui
    // significherebbe poterlo fare in due modi. Il testo migrato conserva i
    // nodi come sono stati scritti.
    let testo = r#"{"schema_version":4,"output":"a","inputs":["main"],
        "nodes":[{"id":"a","op":"table.filter","in":["main"],"config":{"z":1,"y":2}}]}"#;
    let migrato = migra_v4_a_v5(testo).expect("migrazione");
    let valore: Value = serde_json::from_str(&migrato).expect("JSON");
    assert_eq!(valore["nodes"][0]["config"], json!({"z": 1, "y": 2}));
    assert_eq!(valore["schema_version"], json!(PLAN_SCHEMA_VERSION_V5));
}
