//! Unit test del formato piano v4 (parsing con limiti, validazione
//! strutturale, alias, migrazione legacy, canonicalizzazione).

use serde_json::{json, Value};

use plenora_core::limits::PlanLimits;
use plenora_core::PlenoraError;

use super::*;
use crate::table_engine;

fn limits_with(f: impl Fn(&mut PlanLimits)) -> PlanLimits {
    let mut limits = PlanLimits::default();
    f(&mut limits);
    limits
}

/// Piano v4 minimo valido: main -> a (filter) -> b (sort) -> output b.
fn minimal_plan_json() -> String {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "table.sort", "in": ["a"], "config": {}}
        ],
        "output": "b"
    })
    .to_string()
}

fn parse_err(json_text: &str, limits: &PlanLimits) -> String {
    match PlanV4::parse(json_text, limits) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("il piano doveva essere rifiutato"),
    }
}

#[test]
fn parses_minimal_plan_and_computes_topological_order() {
    let validated = PlanV4::parse_default(&minimal_plan_json()).unwrap();
    assert_eq!(validated.topological_order(), &["a".to_owned(), "b".to_owned()]);
    assert_eq!(validated.plan().schema_version, PLAN_SCHEMA_VERSION_V4);
    assert_eq!(validated.plan().output, "b");
}

#[test]
fn rejects_wrong_schema_version() {
    let plan = json!({
        "schema_version": 3,
        "inputs": ["main"],
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}],
        "output": "a"
    })
    .to_string();
    assert!(parse_err(&plan, &PlanLimits::default()).contains("schema_version 3"));
}

#[test]
fn rejects_unknown_fields_at_every_level() {
    let bad_plan = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}],
        "surprise": true
    })
    .to_string();
    assert!(PlanV4::parse_default(&bad_plan).is_err());

    let bad_node = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}, "surprise": 1}]
    })
    .to_string();
    assert!(PlanV4::parse_default(&bad_node).is_err());

    let bad_limits = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "limits": {"not_a_limit": 3},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(PlanV4::parse_default(&bad_limits).is_err());

    let bad_plan_limits = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "limits": {"plan": {"not_a_plan_limit": 3}},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(PlanV4::parse_default(&bad_plan_limits).is_err());
}

#[test]
fn applies_max_plan_json_bytes_before_parsing() {
    let limits = limits_with(|l| l.max_plan_json_bytes = 16);
    let error = parse_err(&minimal_plan_json(), &limits);
    assert!(error.contains("max_plan_json_bytes"), "{error}");
    // Il limite scatta anche su JSON malformato: è pre-parse.
    let garbage = "questo non e' json per niente".repeat(4);
    assert!(parse_err(&garbage, &limits).contains("max_plan_json_bytes"));
}

#[test]
fn applies_max_plan_nodes_and_edges() {
    let limits = limits_with(|l| l.max_plan_nodes = 1);
    assert!(parse_err(&minimal_plan_json(), &limits).contains("max_plan_nodes"));

    // a -> b, a -> c: 3 archi con limite 2.
    let three_edges = json!({
        "schema_version": 4, "inputs": ["main"], "output": "c",
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "table.filter", "in": ["a"], "config": {}},
            {"id": "c", "op": "table.join", "in": ["a", "b"], "config": {}}
        ]
    })
    .to_string();
    let limits = limits_with(|l| l.max_plan_edges = 2);
    assert!(parse_err(&three_edges, &limits).contains("max_plan_edges"));
}

#[test]
fn applies_max_plan_depth() {
    // Catena main -> a -> b -> c: profondità 3.
    let chain = json!({
        "schema_version": 4, "inputs": ["main"], "output": "c",
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "table.sort", "in": ["a"], "config": {}},
            {"id": "c", "op": "table.distinct", "in": ["b"], "config": {}}
        ]
    })
    .to_string();
    PlanV4::parse(&chain, &limits_with(|l| l.max_plan_depth = 3)).unwrap();
    let error = parse_err(&chain, &limits_with(|l| l.max_plan_depth = 2));
    assert!(error.contains("max_plan_depth"), "{error}");
}

#[test]
fn applies_max_fan_out_and_max_inputs() {
    // "a" consumato da b, c, d: fan-out 3.
    let fan_out = json!({
        "schema_version": 4, "inputs": ["main"], "output": "d",
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "table.sort", "in": ["a"], "config": {}},
            {"id": "c", "op": "table.join", "in": ["a", "b"], "config": {}},
            {"id": "d", "op": "table.join", "in": ["a", "c"], "config": {}}
        ]
    })
    .to_string();
    PlanV4::parse(&fan_out, &limits_with(|l| l.max_fan_out = 3)).unwrap();
    let error = parse_err(&fan_out, &limits_with(|l| l.max_fan_out = 2));
    assert!(error.contains("max_fan_out"), "{error}");

    let two_inputs = json!({
        "schema_version": 4, "inputs": ["main", "other"], "output": "a",
        "nodes": [{"id": "a", "op": "table.join", "in": ["main", "other"], "config": {}}]
    })
    .to_string();
    let error = parse_err(&two_inputs, &limits_with(|l| l.max_inputs = 1));
    assert!(error.contains("max_inputs"), "{error}");
}

#[test]
fn applies_max_config_bytes_per_node_and_identifier_bytes() {
    let big_config = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"],
                   "config": {"expression": "x".repeat(4096)}}]
    })
    .to_string();
    let error = parse_err(&big_config, &limits_with(|l| l.max_config_bytes_per_node = 128));
    assert!(error.contains("max_config_bytes_per_node"), "{error}");

    let long_id = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a".repeat(64),
        "nodes": [{"id": "a".repeat(64), "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    let error = parse_err(&long_id, &limits_with(|l| l.max_identifier_bytes = 8));
    assert!(error.contains("max_identifier_bytes"), "{error}");
}

#[test]
fn rejects_cycles() {
    let cyclic = json!({
        "schema_version": 4, "inputs": ["main"], "output": "b",
        "nodes": [
            {"id": "a", "op": "table.join", "in": ["main", "b"], "config": {}},
            {"id": "b", "op": "table.filter", "in": ["a"], "config": {}}
        ]
    })
    .to_string();
    assert!(parse_err(&cyclic, &PlanLimits::default()).contains("ciclo"));

    let self_loop = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.join", "in": ["main", "a"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&self_loop, &PlanLimits::default()).contains("ciclo"));
}

#[test]
fn rejects_broken_references_and_unknown_output() {
    let broken_in = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["ghost"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&broken_in, &PlanLimits::default()).contains("ghost"));

    let broken_output = json!({
        "schema_version": 4, "inputs": ["main"], "output": "ghost",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&broken_output, &PlanLimits::default()).contains("output `ghost`"));
}

#[test]
fn rejects_dead_nodes_not_reaching_the_output() {
    let dead = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "dead", "op": "table.sort", "in": ["main"], "config": {}}
        ]
    })
    .to_string();
    let error = parse_err(&dead, &PlanLimits::default());
    assert!(error.contains("non raggiunge l'output"), "{error}");
}

#[test]
fn rejects_duplicate_and_colliding_identifiers() {
    let duplicate = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "a", "op": "table.sort", "in": ["main"], "config": {}}
        ]
    })
    .to_string();
    assert!(parse_err(&duplicate, &PlanLimits::default()).contains("duplicato"));

    let colliding = json!({
        "schema_version": 4, "inputs": ["a"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["a"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&colliding, &PlanLimits::default()).contains("collide"));
}

#[test]
fn checks_arity_against_the_catalog_descriptor() {
    let unary_with_two = json!({
        "schema_version": 4, "inputs": ["main", "other"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main", "other"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&unary_with_two, &PlanLimits::default()).contains("arieta'"));

    let binary_with_one = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.join", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&binary_with_one, &PlanLimits::default()).contains("arieta'"));

    let nary_without_inputs = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.concat", "in": [], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&nary_without_inputs, &PlanLimits::default()).contains("arieta'"));

    // N-aria con 3 input: valida.
    let nary = json!({
        "schema_version": 4, "inputs": ["x", "y", "z"], "output": "a",
        "nodes": [{"id": "a", "op": "table.concat", "in": ["x", "y", "z"], "config": {}}]
    })
    .to_string();
    PlanV4::parse_default(&nary).unwrap();
}

#[test]
fn resolves_legacy_aliases_to_canonical_ids() {
    let aliased = json!({
        "schema_version": 4, "inputs": ["main"], "output": "b",
        "nodes": [
            {"id": "a", "op": "filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "geo_buffer", "in": ["a"], "config": {}}
        ]
    })
    .to_string();
    let validated = PlanV4::parse_default(&aliased).unwrap();
    let ops: Vec<&str> = validated
        .plan()
        .nodes
        .iter()
        .map(|node| node.op.as_str())
        .collect();
    assert_eq!(ops, ["table.filter", "geo.buffer"]);

    let unknown = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "does_not_exist", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&unknown, &PlanLimits::default()).contains("operazione sconosciuta"));
}

#[test]
fn rejects_non_object_config_but_accepts_omitted_config_as_empty() {
    let non_object = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": [1, 2]}]
    })
    .to_string();
    assert!(parse_err(&non_object, &PlanLimits::default()).contains("deve essere un oggetto"));

    let omitted = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"]}]
    })
    .to_string();
    let validated = PlanV4::parse_default(&omitted).unwrap();
    assert_eq!(validated.plan().nodes[0].config, json!({}));
}

#[test]
fn allows_passthrough_output_referencing_an_input() {
    let passthrough = json!({
        "schema_version": 4, "inputs": ["main"], "nodes": [], "output": "main"
    })
    .to_string();
    let validated = PlanV4::parse_default(&passthrough).unwrap();
    assert!(validated.topological_order().is_empty());
}

fn legacy_plan(steps: Vec<(&str, Value)>) -> table_engine::Plan {
    table_engine::Plan {
        schema_version: table_engine::SCHEMA_VERSION,
        limits: table_engine::Limits::default(),
        steps: steps
            .into_iter()
            .map(|(operation, config)| table_engine::Step {
                operation: operation.into(),
                config,
            })
            .collect(),
    }
}

#[test]
fn migrates_linear_legacy_plan_to_degenerate_dag() {
    let legacy = legacy_plan(vec![
        ("filter", json!({"expression": "x > 1"})),
        ("rename", json!({"columns": {"a": "b"}})),
        ("sort", json!({"by": ["b"]})),
    ]);
    let migrated = PlanV4::from_legacy(&legacy).unwrap();

    assert_eq!(migrated.schema_version, PLAN_SCHEMA_VERSION_V4);
    assert_eq!(migrated.inputs, ["main"]);
    assert_eq!(migrated.output, "n2");
    let nodes = &migrated.nodes;
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0].id, "n0");
    assert_eq!(nodes[0].op, "table.filter");
    assert_eq!(nodes[0].inputs, ["main"]);
    assert_eq!(nodes[1].inputs, ["n0"]);
    assert_eq!(nodes[2].inputs, ["n1"]);
    assert_eq!(nodes[1].op, "table.rename");

    // Limiti legacy mappati sui limiti v4.
    let effective = migrated.limits.effective();
    assert_eq!(effective.rows.max_input_rows, 10_000_000);
    assert_eq!(effective.rows.max_rows_per_edge, 10_000_000);
}

#[test]
fn migrates_single_binary_legacy_step_to_two_inputs() {
    let legacy = legacy_plan(vec![("join", json!({"on": "id"}))]);
    let migrated = PlanV4::from_legacy(&legacy).unwrap();
    assert_eq!(migrated.inputs, ["left", "right"]);
    assert_eq!(migrated.nodes.len(), 1);
    assert_eq!(migrated.nodes[0].op, "table.join");
    assert_eq!(migrated.nodes[0].inputs, ["left", "right"]);
}

#[test]
fn rejects_unmigratable_legacy_plans() {
    let empty = legacy_plan(vec![]);
    assert!(PlanV4::from_legacy(&empty).is_err());

    let unknown = legacy_plan(vec![("not_an_operation", json!({}))]);
    assert!(PlanV4::from_legacy(&unknown).is_err());

    // Operazione geo: sconosciuta alla famiglia tabellare, come nel legacy.
    let geo = legacy_plan(vec![("geo_buffer", json!({}))]);
    assert!(PlanV4::from_legacy(&geo).is_err());

    // Catena con step binario: il protocollo legacy non la ammette.
    let mixed = legacy_plan(vec![("join", json!({})), ("filter", json!({}))]);
    assert!(PlanV4::from_legacy(&mixed).is_err());

    let mut wrong_version = legacy_plan(vec![("filter", json!({}))]);
    wrong_version.schema_version = 7;
    assert!(PlanV4::from_legacy(&wrong_version).is_err());
}

#[test]
fn migration_round_trip_is_deterministic_and_idempotent() {
    let legacy = legacy_plan(vec![
        ("filter", json!({"expression": "x > 1"})),
        ("drop_columns", json!({"columns": ["secret"]})),
    ]);
    let migrated = PlanV4::from_legacy(&legacy).unwrap();
    let first = canonical_json(&migrated);

    // Serializza il piano migrato e lo rilegge come v4: stesso canonico.
    let serialized = serde_json::to_string(&migrated).unwrap();
    let reparsed = PlanV4::parse_default(&serialized).unwrap();
    assert_eq!(first, reparsed.canonical_json());

    // La migrazione ripetuta produce lo stesso piano canonico.
    let remigrated = PlanV4::from_legacy(&legacy).unwrap();
    assert_eq!(first, canonical_json(&remigrated));
}

#[test]
fn canonical_json_is_stable_for_equivalent_plans() {
    // Stesso piano con nodi elencati in ordine diverso, campi in ordine
    // diverso, alias vs id canonici, config omessa vs esplicita.
    let plan_a = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "output": "b",
        "nodes": [
            {"id": "a", "op": "filter", "in": ["main"]},
            {"id": "b", "op": "table.sort", "in": ["a"], "config": {}}
        ]
    })
    .to_string();

    let plan_b = json!({
        "output": "b",
        "nodes": [
            {"config": {}, "in": ["a"], "op": "table.sort", "id": "b"},
            {"config": {}, "in": ["main"], "op": "table.filter", "id": "a"}
        ],
        "inputs": ["main"],
        "schema_version": 4
    })
    .to_string();

    let canonical_a = PlanV4::parse_default(&plan_a).unwrap().canonical_json();
    let canonical_b = PlanV4::parse_default(&plan_b).unwrap().canonical_json();
    assert_eq!(canonical_a, canonical_b);

    // Idempotenza: canonico del canonico (serializzato come piano) invariato.
    let reparsed = PlanV4::parse_default(&canonical_a.to_string()).unwrap();
    assert_eq!(reparsed.canonical_json(), canonical_a);
}

#[test]
fn canonical_json_materializes_limits_and_differs_on_real_differences() {
    let with_limits = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "limits": {"max_rows_per_edge": 5, "max_parallelism": 2},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    let canonical = PlanV4::parse_default(&with_limits).unwrap().canonical_json();
    assert_eq!(canonical["limits"]["max_rows_per_edge"], json!(5));
    assert_eq!(canonical["limits"]["max_parallelism"], json!(2));
    // Default materializzati.
    assert_eq!(
        canonical["limits"]["max_input_rows"],
        json!(10_000_000_u64)
    );
    assert!(canonical["limits"]["plan"]["max_plan_nodes"].is_u64());
    assert!(canonical.get("crs").is_none());

    let without_limits = PlanV4::parse_default(&minimal_plan_json())
        .unwrap()
        .canonical_json();
    assert_ne!(canonical, without_limits);

    // Config semanticamente diversa -> canonico diverso.
    let other_config = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {"expression": "y"}}]
    })
    .to_string();
    let other = PlanV4::parse_default(&other_config).unwrap().canonical_json();
    let base = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {"expression": "x"}}]
    })
    .to_string();
    let base = PlanV4::parse_default(&base).unwrap().canonical_json();
    assert_ne!(base, other);
}

#[test]
fn effective_limits_combine_plan_overrides_and_defaults() {
    let overrides = LimitsOverride {
        max_output_rows: Some(42),
        ..LimitsOverride::default()
    };
    let effective = overrides.effective();
    assert_eq!(effective.rows.max_output_rows, 42);
    assert_eq!(effective.rows.max_input_rows, 10_000_000);
    assert_eq!(effective.max_memory_bytes, 512 * 1024 * 1024);
}

#[test]
fn unsupported_and_error_variants_are_puntuali() {
    // Operazione sconosciuta -> Contract; JSON malformato -> Json.
    let malformed = "{ not json".to_owned() + &"x".repeat(8);
    assert!(matches!(
        PlanV4::parse_default(&malformed),
        Err(PlenoraError::Json(_))
    ));
    let unknown = json!({
        "schema_version": 4, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "nope", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(matches!(
        PlanV4::parse_default(&unknown),
        Err(PlenoraError::Contract(_))
    ));
}

#[test]
fn rejects_declared_inputs_never_referenced() {
    let plan = json!({
        "schema_version": 4, "inputs": ["main", "ghost"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    let error = parse_err(&plan, &PlanLimits::default());
    assert!(error.contains("ghost"), "{error}");
    assert!(error.contains("non e' referenziato"), "{error}");

    // Pass-through: l'input riferito direttamente dall'output non ha
    // consumatori ma resta valido.
    let passthrough = json!({
        "schema_version": 4, "inputs": ["main"], "nodes": [], "output": "main"
    })
    .to_string();
    PlanV4::parse(&passthrough, &PlanLimits::default()).expect("pass-through valido");
}

#[test]
fn canonical_order_stays_topological_beyond_default_plan_depth() {
    // Catena di 300 nodi: oltre il max_plan_depth di default (256) ma valida
    // con limiti custom. La canonicalizzazione deve restare topologica, non
    // ricadere sull'ordine lessicografico degli id.
    let mut nodes = Vec::new();
    let mut previous = "main".to_owned();
    for index in 0..300 {
        // La testa ha id lessicograficamente DOPO gli altri: se l'ordine
        // fosse lessicografico non comparirebbe per prima.
        let id = if index == 0 {
            "zz_head".to_owned()
        } else {
            format!("n{index:03}")
        };
        nodes.push(json!({"id": id, "op": "table.filter", "in": [previous], "config": {}}));
        previous = id;
    }
    let plan = json!({
        "schema_version": 4, "inputs": ["main"], "output": previous, "nodes": nodes,
    })
    .to_string();
    let limits = limits_with(|l| l.max_plan_depth = 512);
    let parsed = PlanV4::parse(&plan, &limits).expect("piano valido con limiti custom");
    let canonical = parsed.canonical_json();
    assert_eq!(
        canonical["nodes"][0]["id"], "zz_head",
        "ordine topologico anche oltre i PlanLimits di default"
    );
    assert_eq!(canonical["nodes"][299]["id"], "n299");
}
