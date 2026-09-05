#![no_main]

//! Parser del piano v5 e migrazione di versione contro JSON ostili
//! (errori-e-limiti.md#memoria-governata): byte arbitrari, JSON malformati, strutture
//! enormi o profondamente annidate, identificatori lunghi. Invarianti: mai
//! panic, mai hang; ogni input produce `Ok` o un errore tipizzato; i
//! `PlanLimits` (default o ristretti dal payload) sono applicati durante il
//! parsing. Se il parse riesce, la serializzazione canonica deve ri-parsare
//! (idempotenza, piano-v5.md#identita-e-fingerprint).
//!
//! Il target copre il **dispatch DAG completo** — v4, v5 e v6 — non il solo
//! parser canonico. Il nome storico e' rimasto `plan_v5_parse`: rinominarlo
//! avrebbe scollegato il corpus gia' raccolto, che e' il valore accumulato di
//! una campagna di fuzzing.
//!
//! Tre ingressi, e ciascuno c'e' per un motivo suo:
//!
//! - `valida_per_versione`, che e' **l'ingresso reale del planner**: sceglie
//!   il parser dalla versione dichiarata, quindi e' l'unico punto da cui un
//!   payload puo' raggiungere il parser della v6;
//! - `migrazione_v4::testo_canonico_v5`, che manipola l'albero JSON e le cui
//!   prove di idempotenza sono specifiche della riscrittura v4 -> v5;
//! - `PlanV5::parse`, il parser canonico, raggiunto anche direttamente.
//!
//! Senza il primo la v6 resterebbe **fuori dalla campagna**: il suo parser
//! ostile e il suo canonico non verrebbero mai esercitati, mentre la
//! documentazione dichiara il contrario.

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

    // Il dispatch: l'unico punto da cui un payload raggiunge il parser della
    // v6. Se riesce, la forma canonica deve **ri-attraversare lo stesso
    // dispatch** e rendere la stessa versione canonica e lo stesso canonico.
    //
    // Ripassare dal dispatch e non dal parser di una versione fissata e' il
    // punto: e' cosi' che si scopre un canonico che dichiara una versione che
    // il dispatch non sa piu' riconoscere, o che ne sceglie un'altra.
    if let Ok(validato) = plenora_engine::plan::valida_per_versione(&text, &limits) {
        let versione = validato.schema_version();
        let canonico = validato.canonical_json().to_string();
        let riletto = plenora_engine::plan::valida_per_versione(&canonico, &PlanLimits::default())
            .expect("la forma canonica deve ri-attraversare il dispatch");
        assert_eq!(
            riletto.schema_version(),
            versione,
            "il canonico deve dichiarare la versione da cui proviene"
        );
        assert_eq!(
            riletto.canonical_json().to_string(),
            canonico,
            "canonicalizzazione non idempotente attraverso il dispatch"
        );
        // Il tetto del dominio, che solo un v6 puo' dichiarare, sopravvive al
        // giro: se cadesse, un piano isolato tornerebbe non selezionabile
        // senza che nulla lo dica.
        assert_eq!(
            riletto.max_domain_memory_bytes(),
            validato.max_domain_memory_bytes(),
            "il tetto del dominio non deve perdersi nel giro canonico"
        );
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
