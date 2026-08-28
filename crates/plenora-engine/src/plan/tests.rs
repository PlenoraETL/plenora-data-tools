//! Unit test del formato piano v5 (parsing con limiti, validazione
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
        "schema_version": 5,
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
    match PlanV5::parse(json_text, limits) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("il piano doveva essere rifiutato"),
    }
}

#[test]
fn parses_minimal_plan_and_computes_topological_order() {
    let validated = PlanV5::parse_default(&minimal_plan_json()).unwrap();
    assert_eq!(
        validated.topological_order(),
        &["a".to_owned(), "b".to_owned()]
    );
    assert_eq!(validated.plan().schema_version, PLAN_SCHEMA_VERSION_V5);
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
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}],
        "surprise": true
    })
    .to_string();
    assert!(PlanV5::parse_default(&bad_plan).is_err());

    let bad_node = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}, "surprise": 1}]
    })
    .to_string();
    assert!(PlanV5::parse_default(&bad_node).is_err());

    let bad_limits = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "limits": {"not_a_limit": 3},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(PlanV5::parse_default(&bad_limits).is_err());

    let bad_plan_limits = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "limits": {"plan": {"not_a_plan_limit": 3}},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(PlanV5::parse_default(&bad_plan_limits).is_err());
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
        "schema_version": 5, "inputs": ["main"], "output": "c",
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
        "schema_version": 5, "inputs": ["main"], "output": "c",
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "table.sort", "in": ["a"], "config": {}},
            {"id": "c", "op": "table.distinct", "in": ["b"], "config": {}}
        ]
    })
    .to_string();
    PlanV5::parse(&chain, &limits_with(|l| l.max_plan_depth = 3)).unwrap();
    let error = parse_err(&chain, &limits_with(|l| l.max_plan_depth = 2));
    assert!(error.contains("max_plan_depth"), "{error}");
}

#[test]
fn applies_max_fan_out_and_max_inputs() {
    // "a" consumato da b, c, d: fan-out 3.
    let fan_out = json!({
        "schema_version": 5, "inputs": ["main"], "output": "d",
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "table.sort", "in": ["a"], "config": {}},
            {"id": "c", "op": "table.join", "in": ["a", "b"], "config": {}},
            {"id": "d", "op": "table.join", "in": ["a", "c"], "config": {}}
        ]
    })
    .to_string();
    PlanV5::parse(&fan_out, &limits_with(|l| l.max_fan_out = 3)).unwrap();
    let error = parse_err(&fan_out, &limits_with(|l| l.max_fan_out = 2));
    assert!(error.contains("max_fan_out"), "{error}");

    let two_inputs = json!({
        "schema_version": 5, "inputs": ["main", "other"], "output": "a",
        "nodes": [{"id": "a", "op": "table.join", "in": ["main", "other"], "config": {}}]
    })
    .to_string();
    let error = parse_err(&two_inputs, &limits_with(|l| l.max_inputs = 1));
    assert!(error.contains("max_inputs"), "{error}");
}

#[test]
fn applies_max_config_bytes_per_node_and_identifier_bytes() {
    let big_config = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"],
                   "config": {"expression": "x".repeat(4096)}}]
    })
    .to_string();
    let error = parse_err(
        &big_config,
        &limits_with(|l| l.max_config_bytes_per_node = 128),
    );
    assert!(error.contains("max_config_bytes_per_node"), "{error}");

    let long_id = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a".repeat(64),
        "nodes": [{"id": "a".repeat(64), "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    let error = parse_err(&long_id, &limits_with(|l| l.max_identifier_bytes = 8));
    assert!(error.contains("max_identifier_bytes"), "{error}");
}

#[test]
fn rejects_cycles() {
    let cyclic = json!({
        "schema_version": 5, "inputs": ["main"], "output": "b",
        "nodes": [
            {"id": "a", "op": "table.join", "in": ["main", "b"], "config": {}},
            {"id": "b", "op": "table.filter", "in": ["a"], "config": {}}
        ]
    })
    .to_string();
    assert!(parse_err(&cyclic, &PlanLimits::default()).contains("ciclo"));

    let self_loop = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.join", "in": ["main", "a"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&self_loop, &PlanLimits::default()).contains("ciclo"));
}

#[test]
fn rejects_broken_references_and_unknown_output() {
    let broken_in = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["ghost"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&broken_in, &PlanLimits::default()).contains("ghost"));

    let broken_output = json!({
        "schema_version": 5, "inputs": ["main"], "output": "ghost",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&broken_output, &PlanLimits::default()).contains("output `ghost`"));
}

#[test]
fn rejects_dead_nodes_not_reaching_the_output() {
    let dead = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
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
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "a", "op": "table.sort", "in": ["main"], "config": {}}
        ]
    })
    .to_string();
    assert!(parse_err(&duplicate, &PlanLimits::default()).contains("duplicato"));

    let colliding = json!({
        "schema_version": 5, "inputs": ["a"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["a"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&colliding, &PlanLimits::default()).contains("collide"));
}

#[test]
fn checks_arity_against_the_catalog_descriptor() {
    let unary_with_two = json!({
        "schema_version": 5, "inputs": ["main", "other"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main", "other"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&unary_with_two, &PlanLimits::default()).contains("arieta'"));

    let binary_with_one = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.join", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&binary_with_one, &PlanLimits::default()).contains("arieta'"));

    let nary_without_inputs = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.concat", "in": [], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&nary_without_inputs, &PlanLimits::default()).contains("arieta'"));

    // N-aria con 3 input: valida.
    let nary = json!({
        "schema_version": 5, "inputs": ["x", "y", "z"], "output": "a",
        "nodes": [{"id": "a", "op": "table.concat", "in": ["x", "y", "z"], "config": {}}]
    })
    .to_string();
    PlanV5::parse_default(&nary).unwrap();
}

#[test]
fn resolves_legacy_aliases_to_canonical_ids() {
    let aliased = json!({
        "schema_version": 5, "inputs": ["main"], "output": "b",
        "nodes": [
            {"id": "a", "op": "filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "geo_buffer", "in": ["a"], "config": {}}
        ]
    })
    .to_string();
    let validated = PlanV5::parse_default(&aliased).unwrap();
    let ops: Vec<&str> = validated
        .plan()
        .nodes
        .iter()
        .map(|node| node.op.as_str())
        .collect();
    assert_eq!(ops, ["table.filter", "geo.buffer"]);

    let unknown = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "does_not_exist", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&unknown, &PlanLimits::default()).contains("operazione sconosciuta"));
}

#[test]
fn rejects_non_object_config_but_accepts_omitted_config_as_empty() {
    let non_object = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": [1, 2]}]
    })
    .to_string();
    assert!(parse_err(&non_object, &PlanLimits::default()).contains("deve essere un oggetto"));

    let omitted = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"]}]
    })
    .to_string();
    let validated = PlanV5::parse_default(&omitted).unwrap();
    assert_eq!(validated.plan().nodes[0].config, json!({}));
}

#[test]
fn allows_passthrough_output_referencing_an_input() {
    let passthrough = json!({
        "schema_version": 5, "inputs": ["main"], "nodes": [], "output": "main"
    })
    .to_string();
    let validated = PlanV5::parse_default(&passthrough).unwrap();
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
    let migrated = PlanV5::from_legacy(&legacy).unwrap();

    assert_eq!(migrated.schema_version, PLAN_SCHEMA_VERSION_V5);
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
    let migrated = PlanV5::from_legacy(&legacy).unwrap();
    assert_eq!(migrated.inputs, ["left", "right"]);
    assert_eq!(migrated.nodes.len(), 1);
    assert_eq!(migrated.nodes[0].op, "table.join");
    assert_eq!(migrated.nodes[0].inputs, ["left", "right"]);
}

#[test]
fn rejects_unmigratable_legacy_plans() {
    let empty = legacy_plan(vec![]);
    assert!(PlanV5::from_legacy(&empty).is_err());

    let unknown = legacy_plan(vec![("not_an_operation", json!({}))]);
    assert!(PlanV5::from_legacy(&unknown).is_err());

    // Operazione geo: sconosciuta alla famiglia tabellare, come nel legacy.
    let geo = legacy_plan(vec![("geo_buffer", json!({}))]);
    assert!(PlanV5::from_legacy(&geo).is_err());

    // Catena con step binario: il protocollo legacy non la ammette.
    let mixed = legacy_plan(vec![("join", json!({})), ("filter", json!({}))]);
    assert!(PlanV5::from_legacy(&mixed).is_err());

    let mut wrong_version = legacy_plan(vec![("filter", json!({}))]);
    wrong_version.schema_version = 7;
    assert!(PlanV5::from_legacy(&wrong_version).is_err());
}

#[test]
fn migration_round_trip_is_deterministic_and_idempotent() {
    let legacy = legacy_plan(vec![
        ("filter", json!({"expression": "x > 1"})),
        ("drop_columns", json!({"columns": ["secret"]})),
    ]);
    let migrated = PlanV5::from_legacy(&legacy).unwrap();
    let first = canonical_json(&migrated);

    // Serializza il piano migrato e lo rilegge come v4: stesso canonico.
    let serialized = serde_json::to_string(&migrated).unwrap();
    let reparsed = PlanV5::parse_default(&serialized).unwrap();
    assert_eq!(first, reparsed.canonical_json());

    // La migrazione ripetuta produce lo stesso piano canonico.
    let remigrated = PlanV5::from_legacy(&legacy).unwrap();
    assert_eq!(first, canonical_json(&remigrated));
}

#[test]
fn canonical_json_is_stable_for_equivalent_plans() {
    // Stesso piano con nodi elencati in ordine diverso, campi in ordine
    // diverso, alias vs id canonici, config omessa vs esplicita.
    let plan_a = json!({
        "schema_version": 5,
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
        "schema_version": 5
    })
    .to_string();

    let canonical_a = PlanV5::parse_default(&plan_a).unwrap().canonical_json();
    let canonical_b = PlanV5::parse_default(&plan_b).unwrap().canonical_json();
    assert_eq!(canonical_a, canonical_b);

    // Idempotenza: canonico del canonico (serializzato come piano) invariato.
    let reparsed = PlanV5::parse_default(&canonical_a.to_string()).unwrap();
    assert_eq!(reparsed.canonical_json(), canonical_a);
}

#[test]
fn canonical_json_materializes_limits_and_differs_on_real_differences() {
    let with_limits = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "limits": {"max_rows_per_edge": 5, "max_parallelism": 2},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    let canonical = PlanV5::parse_default(&with_limits)
        .unwrap()
        .canonical_json();
    assert_eq!(canonical["limits"]["max_rows_per_edge"], json!(5));
    assert_eq!(canonical["limits"]["max_parallelism"], json!(2));
    // Default materializzati.
    assert_eq!(canonical["limits"]["max_input_rows"], json!(10_000_000_u64));
    assert!(canonical["limits"]["plan"]["max_plan_nodes"].is_u64());
    assert!(canonical.get("crs").is_none());

    let without_limits = PlanV5::parse_default(&minimal_plan_json())
        .unwrap()
        .canonical_json();
    assert_ne!(canonical, without_limits);

    // Config semanticamente diversa -> canonico diverso.
    let other_config = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {"expression": "y"}}]
    })
    .to_string();
    let other = PlanV5::parse_default(&other_config)
        .unwrap()
        .canonical_json();
    let base = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {"expression": "x"}}]
    })
    .to_string();
    let base = PlanV5::parse_default(&base).unwrap().canonical_json();
    assert_ne!(base, other);
}

#[test]
fn crs_decisions_are_validated_and_enter_the_canonical_form() {
    // R4.6.3: la decisione del centro e' esplicita nel piano — fa parte
    // dell'identita' piano-v5.md#identita-e-fingerprint (una decisione diversa e' un piano diverso) e
    // le chiavi devono essere input dichiarati, con definizione non vuota.
    let with_decision = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "crs_decisions": {"main": "EPSG:32632"},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    let validated = PlanV5::parse_default(&with_decision).unwrap();
    assert_eq!(
        validated
            .plan()
            .crs_decisions
            .get("main")
            .map(String::as_str),
        Some("EPSG:32632")
    );
    let canonical = validated.canonical_json();
    assert_eq!(canonical["crs_decisions"]["main"], json!("EPSG:32632"));

    // Piano senza decisioni: il campo non compare nella forma canonica
    // (piani e fingerprint esistenti invariati).
    let without = PlanV5::parse_default(&minimal_plan_json())
        .unwrap()
        .canonical_json();
    assert!(without.get("crs_decisions").is_none());

    // Una decisione diversa e' un piano diverso.
    let other_decision = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "crs_decisions": {"main": "EPSG:4326"},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert_ne!(
        canonical,
        PlanV5::parse_default(&other_decision)
            .unwrap()
            .canonical_json()
    );

    // Decisione per un input non dichiarato -> errore esplicito.
    let unknown_input = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "crs_decisions": {"other": "EPSG:32632"},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&unknown_input, &PlanLimits::default()).contains("crs_decisions"));

    // Definizione vuota -> errore esplicito.
    let empty_definition = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "crs_decisions": {"main": "   "},
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(parse_err(&empty_definition, &PlanLimits::default()).contains("crs_decisions"));
}

#[test]
fn canonical_json_normalizes_numbers() {
    // `100` e `100.0` denotano lo stesso valore: stessa forma canonica
    // (piano-v5.md#identita-e-fingerprint), anche annidati in array e oggetti dentro la config.
    let plan_with = |value: serde_json::Value| {
        json!({
            "schema_version": 5, "inputs": ["main"], "output": "a",
            "nodes": [{"id": "a", "op": "table.filter", "in": ["main"],
                       "config": {"column": "id", "operator": ">",
                                  "value": value, "nested": [{"n": value}, [-2.0]]}}]
        })
        .to_string()
    };
    let int_plan = PlanV5::parse_default(&plan_with(json!(100)))
        .unwrap()
        .canonical_json();
    let float_plan = PlanV5::parse_default(&plan_with(json!(100.0)))
        .unwrap()
        .canonical_json();
    assert_eq!(int_plan, float_plan);
    // La forma canonica e' l'intero (float a valore intero entro 2^53).
    assert_eq!(int_plan["nodes"][0]["config"]["value"], json!(100));
    assert_eq!(int_plan["nodes"][0]["config"]["nested"][1][0], json!(-2));

    // I float con frazione restano float.
    let frac = PlanV5::parse_default(&plan_with(json!(2.5)))
        .unwrap()
        .canonical_json();
    assert_eq!(frac["nodes"][0]["config"]["value"], json!(2.5));

    // Oltre 2^53 un intero puo' non avere un f64 esatto: le forme non sono
    // unificate (fail-closed, valori distinti non collassano mai).
    let big_int = PlanV5::parse_default(&plan_with(json!(9_007_199_254_740_994_u64)))
        .unwrap()
        .canonical_json();
    let big_float = PlanV5::parse_default(&plan_with(json!(9_007_199_254_740_994.0)))
        .unwrap()
        .canonical_json();
    assert_ne!(
        big_int["nodes"][0]["config"]["value"],
        big_float["nodes"][0]["config"]["value"]
    );

    // Regressione (collisione plan_hash): 2^53+1 non ha un f64 esatto e
    // arrotonda su 2^53. Canonicalizzare passando per f64 collasserebbe i due
    // interi sulla stessa forma canonica; gli interi devono restare distinti
    // e conservare le cifre esatte.
    let odd_int = PlanV5::parse_default(&plan_with(json!(9_007_199_254_740_993_u64)))
        .unwrap()
        .canonical_json();
    let exact_int = PlanV5::parse_default(&plan_with(json!(9_007_199_254_740_992_u64)))
        .unwrap()
        .canonical_json();
    assert_eq!(
        odd_int["nodes"][0]["config"]["value"],
        json!(9_007_199_254_740_993_u64)
    );
    assert_ne!(
        odd_int["nodes"][0]["config"]["value"],
        exact_int["nodes"][0]["config"]["value"]
    );

    // Gli interi restano invariati fino a u64::MAX / i64::MIN: nessun
    // passaggio per f64, nessuna perdita di cifre.
    let umax = PlanV5::parse_default(&plan_with(json!(u64::MAX)))
        .unwrap()
        .canonical_json();
    assert_eq!(umax["nodes"][0]["config"]["value"], json!(u64::MAX));
    let imin = PlanV5::parse_default(&plan_with(json!(i64::MIN)))
        .unwrap()
        .canonical_json();
    assert_eq!(imin["nodes"][0]["config"]["value"], json!(i64::MIN));
}

/// Piano minimo con un blocco `limits` arbitrario.
fn plan_with_limits(limits: &Value) -> String {
    json!({
        "schema_version": 5,
        "limits": limits,
        "inputs": ["main"],
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"], "config": {}},
            {"id": "b", "op": "table.sort", "in": ["a"], "config": {}}
        ],
        "output": "b"
    })
    .to_string()
}

/// I limiti dati/runtime del piano sono **configurazione**: il piano puo'
/// dichiararli sopra o sotto il default, ed e' il modo previsto per
/// dimensionare l'esecuzione. Il dominio resta chiuso da `Limits::validate`.
#[test]
fn i_limiti_dati_runtime_si_dichiarano_in_entrambi_i_versi() {
    for (campo, valore) in [
        ("max_input_rows", json!(10)),
        ("max_output_rows", json!(20_000_000)),
        ("max_rows_per_edge", json!(10)),
        ("max_expansion_factor", json!(2.0)),
        ("max_governed_memory_bytes", json!(1_099_511_627_776_u64)),
        ("max_temp_bytes", json!(1_048_576)),
        ("max_wkb_cell_bytes", json!(1024)),
        ("max_payload_bytes", json!(1_048_576)),
        ("max_batches", json!(8)),
        ("max_geometry_depth", json!(4)),
        ("max_string_bytes", json!(1024)),
        ("max_regex_bytes", json!(256)),
        ("max_parallelism", json!(2)),
        ("spill_partitions", json!(128)),
    ] {
        let piano = plan_with_limits(&json!({ campo: valore }));
        assert!(
            PlanV5::parse_default(&piano).is_ok(),
            "dichiarazione rifiutata su {campo}"
        );
    }
    // Fuori dominio resta rifiutato, dove il controllo e' sempre stato.
    let fuori = plan_with_limits(&json!({"spill_partitions": 1}));
    let validato = PlanV5::parse_default(&fuori).expect("il parse non giudica il dominio");
    assert!(validato.effective_limits().validate().is_err());
}

/// I limiti di piano dichiarati governano il piano che li dichiara: senza
/// questo, finirebbero nel `plan_hash` senza che nulla li applichi.
#[test]
fn i_limiti_di_piano_dichiarati_governano_il_piano_che_li_dichiara() {
    // Due nodi, tetto dichiarato a uno.
    let piano = plan_with_limits(&json!({"plan": {"max_plan_nodes": 1}}));
    let errore = parse_err(&piano, &PlanLimits::default());
    assert!(errore.contains("max_plan_nodes"), "{errore}");

    // `max_plan_json_bytes` e' l'eccezione dichiarata: un tetto sul testo va
    // applicato prima di leggerlo, quindi non puo' venire dal testo. Il
    // piano lo dichiara, resta soggetto alla sola restrizione, ma non
    // governa se stesso — altrimenti la forma canonica, che materializza
    // tutti i limiti ed e' sempre piu' grande, non rientrerebbe piu'.
    let piano = plan_with_limits(&json!({"plan": {"max_plan_json_bytes": 16}}));
    let validato = PlanV5::parse_default(&piano).expect("il tetto sui byte e' del chiamante");
    let canonico = validato.canonical_json().to_string();
    assert!(
        canonico.len() > 16,
        "la forma canonica materializza i limiti: e' piu' grande del documento"
    );
    // ...e la forma canonica rientra: e' la proprieta' che il ricontrollo
    // avrebbe rotto.
    let riletto = PlanV5::parse_default(&canonico).expect("round-trip canonico");
    assert_eq!(riletto.canonical_json(), validato.canonical_json());

    // Profondita' due, tetto dichiarato a uno.
    let piano = plan_with_limits(&json!({"plan": {"max_plan_depth": 1}}));
    let errore = parse_err(&piano, &PlanLimits::default());
    assert!(errore.contains("max_plan_depth"), "{errore}");

    // Un tetto dichiarato ma rispettato non cambia nulla.
    let piano = plan_with_limits(&json!({"plan": {"max_plan_nodes": 2}}));
    assert!(PlanV5::parse_default(&piano).is_ok());
}

/// L'identita' di un piano NON dipende dalla policy di chi lo valida.
///
/// Lo stesso documento, accettato sotto due tetti diversi, deve avere lo
/// stesso canonico e quindi lo stesso `plan_hash`: l'esecuzione e' la
/// stessa, e un grafo persistito dev'essere confrontabile con se stesso.
#[test]
fn il_canonico_non_dipende_dalla_policy_di_chi_valida() {
    // Catena profonda 300: OLTRE il `max_plan_depth` di default (256), quindi
    // accettabile solo con una policy piu' larga. E' il caso che mette alla
    // prova sia l'identita' sia la rileggibilita' del canonico.
    let mut nodes = Vec::new();
    let mut previous = "main".to_owned();
    for index in 0..300 {
        let id = format!("n{index:03}");
        nodes.push(json!({"id": id, "op": "table.filter", "in": [previous], "config": {}}));
        previous = format!("n{index:03}");
    }
    let profondo = json!({
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": nodes,
        "output": previous,
    })
    .to_string();

    // Due policy diverse, entrambe abbastanza larghe da accettare il piano.
    // Si allarga SOLO la profondita': allargare anche il fan-out
    // confonderebbe il rifiuto per profondita' con quello per restrizione,
    // dato che il canonico dichiara il fan-out di default.
    let larghi = limits_with(|l| l.max_plan_depth = 512);
    let larghissimi = limits_with(|l| l.max_plan_depth = 4096);
    let sotto_larghi = PlanV5::parse(&profondo, &larghi).expect("valido con policy larga");
    let sotto_larghissimi =
        PlanV5::parse(&profondo, &larghissimi).expect("valido con policy larghissima");
    assert_eq!(
        sotto_larghi.canonical_json(),
        sotto_larghissimi.canonical_json(),
        "l'identita' e' una proprieta' del piano, non di chi lo valida"
    );
    // ...e coincide con quella prodotta senza conoscere il chiamante: sono i
    // default della libreria a essere materializzati.
    assert_eq!(
        sotto_larghi.canonical_json(),
        canonical_json(sotto_larghi.plan())
    );
    assert_eq!(
        sotto_larghi.canonical_json()["limits"]["plan"]["max_plan_depth"],
        json!(256),
        "il canonico materializza il default, non la policy del chiamante"
    );
    // Il prezzo, verificato invece che solo dichiarato: quel canonico NON e'
    // rileggibile con nessuna policy, perche' dichiara i default e li viola.
    // E' un input di hash, non un documento riproponibile.
    let testo_canonico = sotto_larghi.canonical_json().to_string();
    let con_policy_larga = parse_err(&testo_canonico, &larghi);
    assert!(
        con_policy_larga.contains("max_plan_depth"),
        "il canonico dichiara 256 e la catena e' profonda 300: non rientra          nemmeno con la policy che l'aveva accettato — {con_policy_larga}"
    );
    let con_default = parse_err(&testo_canonico, &PlanLimits::default());
    assert!(con_default.contains("max_plan_depth"), "{con_default}");

    // Per un piano che sta DENTRO i default il canonico si rilegge, ed e' la
    // condizione di ogni piano prodotto dalla CLI.
    let dentro = PlanV5::parse_default(&minimal_plan_json()).expect("valido");
    let riletto = PlanV5::parse_default(&dentro.canonical_json().to_string())
        .expect("il canonico di un piano dentro i default rientra");
    assert_eq!(riletto.canonical_json(), dentro.canonical_json());

    // Con la policy di DEFAULT il canonico e' quello di sempre: i
    // `plan_hash` gia' emessi non cambiano.
    let semplice = minimal_plan_json();
    let validato_default = PlanV5::parse_default(&semplice).expect("valido");
    assert_eq!(
        validato_default.canonical_json()["limits"]["plan"],
        canonical_json(validato_default.plan())["limits"]["plan"],
        "con la policy di default la forma canonica non cambia"
    );

    // ...compreso il caso in cui il piano DICHIARA un limite di piano: il
    // valore dichiarato dev'essere quello che entra nel canonico, non
    // quello del chiamante. Sostituirlo cambierebbe il `plan_hash` di ogni
    // piano che lo dichiara, senza cambio di dominio.
    for (campo, valore) in [
        ("max_plan_json_bytes", 4096_u64),
        ("max_plan_nodes", 8),
        ("max_identifier_bytes", 32),
    ] {
        let dichiarato = plan_with_limits(&json!({"plan": { campo: valore }}));
        let validato = PlanV5::parse_default(&dichiarato).expect("valido");
        assert_eq!(
            validato.canonical_json()["limits"]["plan"][campo],
            json!(valore),
            "il canonico deve portare il valore dichiarato di {campo}"
        );
        assert_eq!(
            validato.canonical_json(),
            canonical_json(validato.plan()),
            "con la policy di default il canonico coincide con quello libero"
        );
    }
}

/// Anche i limiti di piano possono solo restringere quelli del chiamante.
#[test]
fn i_limiti_di_piano_non_possono_allargare_quelli_del_chiamante() {
    let stretti = limits_with(|l| l.max_plan_nodes = 2);
    let piano = plan_with_limits(&json!({"plan": {"max_plan_nodes": 1024}}));
    let errore = parse_err(&piano, &stretti);
    assert!(
        errore.contains("solo restringere") && errore.contains("plan.max_plan_nodes"),
        "{errore}"
    );
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
    assert_eq!(effective.max_governed_memory_bytes, 512 * 1024 * 1024);
}

#[test]
fn unsupported_and_error_variants_are_puntuali() {
    // Operazione sconosciuta -> Contract; JSON malformato -> Json.
    let malformed = "{ not json".to_owned() + &"x".repeat(8);
    assert!(matches!(
        PlanV5::parse_default(&malformed),
        Err(PlenoraError::DataMapping(_))
    ));
    let unknown = json!({
        "schema_version": 5, "inputs": ["main"], "output": "a",
        "nodes": [{"id": "a", "op": "nope", "in": ["main"], "config": {}}]
    })
    .to_string();
    assert!(matches!(
        PlanV5::parse_default(&unknown),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn rejects_declared_inputs_never_referenced() {
    let plan = json!({
        "schema_version": 5, "inputs": ["main", "ghost"], "output": "a",
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}]
    })
    .to_string();
    let error = parse_err(&plan, &PlanLimits::default());
    assert!(error.contains("ghost"), "{error}");
    assert!(error.contains("non e' referenziato"), "{error}");

    // Pass-through: l'input riferito direttamente dall'output non ha
    // consumatori ma resta valido.
    let passthrough = json!({
        "schema_version": 5, "inputs": ["main"], "nodes": [], "output": "main"
    })
    .to_string();
    PlanV5::parse(&passthrough, &PlanLimits::default()).expect("pass-through valido");
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
        "schema_version": 5, "inputs": ["main"], "output": previous, "nodes": nodes,
    })
    .to_string();
    let limits = limits_with(|l| l.max_plan_depth = 512);
    let parsed = PlanV5::parse(&plan, &limits).expect("piano valido con limiti custom");
    let canonical = parsed.canonical_json();
    assert_eq!(
        canonical["nodes"][0]["id"], "zz_head",
        "ordine topologico anche oltre i PlanLimits di default"
    );
    assert_eq!(canonical["nodes"][299]["id"], "n299");
}
