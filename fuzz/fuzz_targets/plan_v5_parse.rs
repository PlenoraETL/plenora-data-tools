#![no_main]

//! Parser del piano v5 e migrazione di versione contro JSON ostili
//! (Fase 2A, errori-e-limiti.md#memoria-governata): byte arbitrari, JSON malformati, strutture
//! enormi o profondamente annidate, identificatori lunghi. Invarianti: mai
//! panic, mai hang; ogni input produce `Ok` o un errore tipizzato; i
//! `PlanLimits` (default o ristretti dal payload) sono applicati durante il
//! parsing. Se il parse riesce, la serializzazione canonica deve ri-parsare
//! (idempotenza, piano-v5.md#identita-e-fingerprint).
//!
//! Il target copre **due** ingressi, non uno: il parser canonico e il
//! dispatcher di versione, che e' l'ingresso reale del planner. Fuzzare solo
//! il parser lascerebbe fuori la riscrittura v4 -> v5, che e' il pezzo nuovo
//! e quello che manipola l'albero JSON.

use libfuzzer_sys::fuzz_target;
use plenora_core::limits::PlanLimits;
use plenora_engine::plan::{migrazione_v4, PlanV5};

fn limits_from(payload: &[u8]) -> PlanLimits {
    let mut limits = PlanLimits::default();
    // Con il primo byte dispari si restringono i limiti usando i byte
    // successivi: il parser deve applicarli comunque senza panicare.
    if payload.first().copied().unwrap_or_default() % 2 == 1 {
        let pick = |index: usize, base: usize| -> usize {
            1 + payload.get(index).copied().unwrap_or_default() as usize % base
        };
        limits.max_plan_json_bytes = pick(1, 1 << 20);
        limits.max_plan_nodes = pick(2, 64);
        limits.max_plan_edges = pick(3, 128);
        limits.max_plan_depth = pick(4, 16);
        limits.max_fan_out = pick(5, 8);
        limits.max_inputs = pick(6, 4);
        limits.max_config_bytes_per_node = pick(7, 4_096);
        limits.max_identifier_bytes = pick(8, 64);
    }
    limits
}

fuzz_target!(|payload: &[u8]| {
    let limits = limits_from(payload);
    let text = String::from_utf8_lossy(payload);

    // Dispatcher di versione: un v4 arriva al parser solo da qui. Se la
    // migrazione riesce, ripeterla sul risultato deve riuscire DI NUOVO e
    // dare lo stesso testo.
    //
    // La seconda passata non e' condizionata al suo successo: un `if let Ok`
    // qui renderebbe il controllo vacuo proprio nel caso interessante — un
    // migrato che supera `max_plan_json_bytes` sarebbe un `Err` silenzioso
    // invece che l'idempotenza rotta che e'. E' esattamente il difetto che
    // questo target ha nascosto finche' e' stato scritto cosi'.
    if let Ok(canonico) = migrazione_v4::testo_canonico_v5(&text, &limits) {
        let due_volte = migrazione_v4::testo_canonico_v5(canonico.as_ref(), &limits)
            .expect("se la prima passata riesce, la seconda deve riuscire");
        assert_eq!(
            canonico.as_ref(),
            due_volte.as_ref(),
            "migrazione non idempotente"
        );
        assert!(
            canonico.len() <= limits.max_plan_json_bytes,
            "il testo canonico restituito supera max_plan_json_bytes"
        );
        let _ = PlanV5::parse(canonico.as_ref(), &limits);
    }

    if let Ok(plan) = PlanV5::parse(&text, &limits) {
        // Limiti strutturali rispettati sul piano validato.
        assert!(plan.plan().nodes.len() <= limits.max_plan_nodes);
        assert!(plan.plan().inputs.len() <= limits.max_inputs);
        assert!(plan.topological_order().len() == plan.plan().nodes.len());
        // Idempotenza della canonicalizzazione: il JSON canonico ri-parsa
        // con i limiti di default e produce la stessa forma canonica.
        let canonical = plan.canonical_json().to_string();
        let reparsed = PlanV5::parse_default(&canonical).expect("canonical re-parse");
        assert_eq!(
            reparsed.canonical_json().to_string(),
            canonical,
            "canonicalizzazione non idempotente"
        );
    }
});
