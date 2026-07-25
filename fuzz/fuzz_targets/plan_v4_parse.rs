#![no_main]

//! Parser del piano v4 contro JSON ostili (Fase 2A, ADR 6): byte arbitrari,
//! JSON malformati, strutture enormi o profondamente annidate, identificatori
//! lunghi. Invarianti: mai panic, mai hang; ogni input produce `Ok` o un
//! errore tipizzato; i `PlanLimits` (default o ristretti dal payload) sono
//! applicati durante il parsing. Se il parse riesce, la serializzazione
//! canonica deve ri-parsare (idempotenza, ADR 4).

use libfuzzer_sys::fuzz_target;
use plenora_core::limits::PlanLimits;
use plenora_engine::plan::PlanV4;

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
    if let Ok(plan) = PlanV4::parse(&text, &limits) {
        // Limiti strutturali rispettati sul piano validato.
        assert!(plan.plan().nodes.len() <= limits.max_plan_nodes);
        assert!(plan.plan().inputs.len() <= limits.max_inputs);
        assert!(plan.topological_order().len() == plan.plan().nodes.len());
        // Idempotenza della canonicalizzazione: il JSON canonico ri-parsa
        // con i limiti di default e produce la stessa forma canonica.
        let canonical = plan.canonical_json().to_string();
        let reparsed = PlanV4::parse_default(&canonical).expect("canonical re-parse");
        assert_eq!(
            reparsed.canonical_json().to_string(),
            canonical,
            "canonicalizzazione non idempotente"
        );
    }
});
