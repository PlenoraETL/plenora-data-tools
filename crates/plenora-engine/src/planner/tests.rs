//! Test del planner (fase 1 `validate`, Architetture.md par. 6.1, ADR 4/5).

use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::catalog::{OperationDescriptor, CATALOG};
use plenora_core::contract::{
    ContractProperties, ContractProperty, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions, PropertyConfidence, PropertyScope,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_core::PlenoraError;

use crate::plan::PlanV4;
use crate::table_engine;

use super::*;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

fn projected_crs() -> ResolvedCrs {
    ResolvedCrs::from_resolved_parts(
        "EPSG:32632".to_owned(),
        json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
        CrsKind::Projected,
        Some(1.0),
    )
}

fn geographic_crs() -> ResolvedCrs {
    ResolvedCrs::from_resolved_parts(
        "EPSG:4326".to_owned(),
        json!({"type": "GeographicCRS", "name": "WGS 84"}),
        CrsKind::Geographic,
        None,
    )
}

fn table_contract() -> DataContract {
    DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ])))
}

/// Contratto con geometria: `id` Int64 + colonna WKB `geom` con il `FieldId`
/// dato (i contratti di input arrivano con id assegnati dal lettore: il
/// planner li rimappa nel namespace del grafo, D16).
fn geo_contract(field_id: u32) -> DataContract {
    geo_contract_with_crs(field_id, projected_crs())
}

fn geo_contract_with_crs(field_id: u32, crs: ResolvedCrs) -> DataContract {
    DataContract::new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geom", DataType::Binary, true),
        ])),
        vec![GeometryColumnContract {
            field_id: FieldId(field_id),
            name: "geom".to_owned(),
            crs,
            dimensions: GeometryDimensions::Xy,
            nullable: true,
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto fixture valido")
}

fn input(contract: DataContract) -> Vec<(String, DataContract)> {
    vec![("main".to_owned(), contract)]
}

/// Pipeline mista table+geo: filter -> buffer -> aggregate.
fn mixed_plan_json() -> String {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 100.0}},
            {"id": "g", "op": "table.aggregate", "in": ["b"],
             "config": {"group_by": ["id"], "aggregations": []}},
        ],
        "output": "g",
    })
    .to_string()
}

fn validate_mixed() -> ValidatedGraph {
    validate(&mixed_plan_json(), &input(geo_contract(7))).expect("pipeline mista valida")
}

// ---------------------------------------------------------------------------
// Happy path
// ---------------------------------------------------------------------------

#[test]
fn mixed_table_geo_pipeline_validates_end_to_end() {
    let graph = validate_mixed();

    assert_eq!(graph.plan_format_version(), 4);
    assert_eq!(graph.engine_version().0, ENGINE_VERSION);
    assert_eq!(graph.topological_order(), &["f", "b", "g"]);
    assert_eq!(
        graph.used_operations(),
        &["geo.buffer", "table.aggregate", "table.filter"]
    );
    // ADR 7: il profilo di publish di default (`AtomicPublish`) e' sempre
    // richiesto, finche' il formato piano non dichiara un profilo.
    assert_eq!(
        graph.required_capabilities().names().collect::<Vec<_>>(),
        vec![PublishProfile::Atomic.capability_name()]
    );
    assert_eq!(graph.input_contract_fingerprints().len(), 1);

    // La geometria segue la colonna attraverso table.filter e geo.buffer con
    // lo stesso FieldId (rimappato, non quello del lettore).
    let input_geometry = graph.edge_contract("main").unwrap().geometries[0].field_id;
    assert_ne!(input_geometry, FieldId(7), "FieldId di input rimappato");
    let filtered = graph.edge_contract("f").unwrap();
    assert_eq!(filtered.geometries.len(), 1);
    assert_eq!(filtered.geometries[0].field_id, input_geometry);
    let buffered = graph.edge_contract("b").unwrap();
    assert_eq!(buffered.geometries[0].field_id, input_geometry);

    // table.aggregate senza la geometria in group_by la perde: output tabellare.
    let output = graph.output_contract();
    assert!(output.geometries.is_empty());
    assert!(output.schema.field_with_name("count").is_ok());
}

#[test]
fn fan_out_fan_in_validates() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "a", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": "<", "value": 1000}},
            {"id": "j", "op": "table.join", "in": ["a", "b"],
             "config": {"left_keys": ["id"], "right_keys": ["id"]}},
        ],
        "output": "j",
    })
    .to_string();
    let graph = validate(&plan, &input(table_contract())).expect("fan-out/fan-in valido");
    assert_eq!(graph.topological_order(), &["a", "b", "j"]);
    assert!(graph.output_contract().schema.field_with_name("id").is_ok());
}

#[test]
fn binary_geo_fan_in_remaps_input_field_ids() {
    // Due input con lo stesso FieldId(4) assegnato dal lettore: il planner
    // rimappa nel namespace globale, nessuna collisione (D16).
    let plan = json!({
        "schema_version": 4,
        "inputs": ["left", "right"],
        "nodes": [
            {"id": "j", "op": "geo.sjoin", "in": ["left", "right"],
             "config": {"predicate": "intersects"}},
        ],
        "output": "j",
    })
    .to_string();
    let contracts = vec![
        ("left".to_owned(), geo_contract(4)),
        ("right".to_owned(), geo_contract(4)),
    ];
    let graph = validate(&plan, &contracts).expect("sjoin valida");
    let left_id = graph.edge_contract("left").unwrap().geometries[0].field_id;
    let right_id = graph.edge_contract("right").unwrap().geometries[0].field_id;
    assert_ne!(left_id, right_id, "FieldId rimappati in namespace globale");
    let joined = graph.output_contract();
    assert!(joined
        .schema
        .field_with_name(plenora_kernels_geo::analyze::RIGHT_INDEX_COLUMN)
        .is_ok());
}

// ---------------------------------------------------------------------------
// Errori di validazione (fail-closed, prima della lettura dei dati)
// ---------------------------------------------------------------------------

#[test]
fn geo_op_after_dropped_geometry_fails_in_validation() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "d", "op": "table.drop_columns", "in": ["main"],
             "config": {"columns": ["geom"]}},
            {"id": "b", "op": "geo.buffer", "in": ["d"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    })
    .to_string();
    let result = validate(&plan, &input(geo_contract(1)));
    match result {
        Err(PlenoraError::Schema(message)) => {
            assert!(message.contains("nodo `b`"), "contesto nodo: {message}");
            assert!(message.contains("geometria"), "{message}");
        }
        other => panic!("atteso Schema error in validazione, ottenuto {other:?}"),
    }
}

#[test]
fn incompatible_crs_fails_in_validation() {
    // geo.buffer richiede CRS proiettato: input geografico -> errore CRS.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    })
    .to_string();
    let result = validate(&plan, &input(geo_contract_with_crs(1, geographic_crs())));
    assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
}

#[cfg(not(feature = "proj-backend"))]
#[test]
fn plan_crs_fails_closed_without_proj_backend() {
    let plan = json!({
        "schema_version": 4,
        "crs": "EPSG:32632",
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
    .to_string();
    let result = validate(&plan, &input(table_contract()));
    match result {
        Err(PlenoraError::Crs(message)) => {
            assert!(message.contains("CRS_BACKEND_UNAVAILABLE"), "{message}");
        }
        other => panic!("atteso CRS_BACKEND_UNAVAILABLE, ottenuto {other:?}"),
    }
}

#[cfg(feature = "proj-backend")]
#[test]
fn plan_crs_resolves_with_proj_backend() {
    let plan = json!({
        "schema_version": 4,
        "crs": "EPSG:32632",
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
    .to_string();
    let graph = validate(&plan, &input(table_contract())).expect("CRS risolto da PROJ");
    assert_eq!(graph.plan_crs().unwrap().kind(), CrsKind::Projected);
}

#[cfg(not(feature = "geos-backend"))]
#[test]
fn missing_capability_fails_in_validation() {
    // geo.make_valid richiede il backend geos: senza la feature compilata il
    // piano fallisce in validazione, non a meta' esecuzione (par. 6.1 passo 5).
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "m", "op": "geo.make_valid", "in": ["main"]},
        ],
        "output": "m",
    })
    .to_string();
    let result = validate(&plan, &input(geo_contract(1)));
    match result {
        Err(PlenoraError::Unsupported(message)) => {
            assert!(message.contains("geos"), "{message}");
            assert!(message.contains("nodo `m`"), "{message}");
        }
        other => panic!("atteso Unsupported per capability mancante, ottenuto {other:?}"),
    }
}

#[cfg(feature = "geos-backend")]
#[test]
fn compiled_capability_validates_and_is_required_by_graph() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "m", "op": "geo.make_valid", "in": ["main"]},
        ],
        "output": "m",
    })
    .to_string();
    let graph = validate(&plan, &input(geo_contract(1))).expect("geos compilato");
    assert!(graph.required_capabilities().contains("geos"));
    check_compatibility(&graph, CATALOG, ENGINE_VERSION, ARROW_VERSION, &local_capabilities())
        .expect("ambiente coerente");
    // Un ambiente senza geos rifiuta il grafo (ADR 4).
    let result = check_compatibility(&graph, CATALOG, ENGINE_VERSION, ARROW_VERSION, &CapabilitySet::default());
    assert!(matches!(result, Err(PlenoraError::Contract(_))), "{result:?}");
}

#[test]
fn input_contracts_must_match_declared_inputs() {
    let plan = mixed_plan_json();

    let missing = validate(&plan, &[]);
    assert!(matches!(missing, Err(PlenoraError::Contract(_))), "{missing:?}");

    let extra = validate(
        &plan,
        &[
            ("main".to_owned(), geo_contract(1)),
            ("other".to_owned(), table_contract()),
        ],
    );
    assert!(matches!(extra, Err(PlenoraError::Contract(_))), "{extra:?}");

    let duplicate = validate(
        &plan,
        &[
            ("main".to_owned(), geo_contract(1)),
            ("main".to_owned(), geo_contract(2)),
        ],
    );
    assert!(matches!(duplicate, Err(PlenoraError::Contract(_))), "{duplicate:?}");
}

#[test]
fn sorted_by_keys_on_input_are_rejected_fail_closed() {
    let mut contract = table_contract();
    contract.properties.sorted_by = Some(ContractProperty::new(
        PropertyConfidence::Proven(vec![FieldId(0)]),
        PropertyScope::Stream,
    ));
    let result = validate(&mixed_plan_json(), &input(contract));
    match result {
        Err(PlenoraError::Contract(message)) => {
            assert!(message.contains("sorted_by"), "{message}");
        }
        other => panic!("atteso rifiuto sorted_by su input, ottenuto {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Alias, migrazione, canonicalizzazione (ADR 4)
// ---------------------------------------------------------------------------

#[test]
fn legacy_aliases_in_v4_plan_validate_and_share_plan_hash() {
    let aliased = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "geo_buffer", "in": ["f"], "config": {"distance": 100.0}},
        ],
        "output": "b",
    })
    .to_string();
    let canonical = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 100.0}},
        ],
        "output": "b",
    })
    .to_string();
    let from_alias = validate(&aliased, &input(geo_contract(1))).expect("alias risolti");
    let from_canonical = validate(&canonical, &input(geo_contract(1))).expect("canonico");
    assert_eq!(from_alias.plan_hash(), from_canonical.plan_hash());
    assert_eq!(from_alias.catalog_fingerprint(), from_canonical.catalog_fingerprint());
}

#[test]
fn equivalent_plans_share_plan_hash() {
    // Ordine dei nodi nel JSON e config omessa vs `{}` esplicita sono
    // irrilevanti: il piano canonico e' lo stesso (ADR 4).
    let sparse = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 100.0}},
            {"id": "c", "op": "geo.centroid", "in": ["b"]},
        ],
        "output": "c",
    })
    .to_string();
    let explicit = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "c", "op": "geo.centroid", "in": ["b"], "config": {}},
            {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 100.0}},
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "c",
    })
    .to_string();
    let first = validate(&sparse, &input(geo_contract(1))).expect("primo piano");
    let second = validate(&explicit, &input(geo_contract(1))).expect("secondo piano");
    assert_eq!(first.plan_hash(), second.plan_hash());

    let different = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 200.0}},
        ],
        "output": "b",
    })
    .to_string();
    let other = validate(&different, &input(geo_contract(1))).expect("terzo piano");
    assert_ne!(first.plan_hash(), other.plan_hash());
}

#[test]
fn legacy_plan_migrates_and_validates_end_to_end() {
    let legacy = table_engine::Plan {
        schema_version: 3,
        limits: table_engine::Limits::default(),
        steps: vec![
            table_engine::Step {
                operation: "filter".to_owned(),
                config: json!({"column": "id", "operator": ">", "value": 0}),
            },
            table_engine::Step {
                operation: "aggregate".to_owned(),
                config: json!({"group_by": ["id"], "aggregations": []}),
            },
        ],
    };
    let migrated = PlanV4::from_legacy(&legacy).expect("migrazione");
    let plan_json = serde_json::to_string(&migrated).expect("serializzazione piano migrato");
    let graph = validate(&plan_json, &input(table_contract())).expect("piano migrato valido");
    assert_eq!(graph.plan_format_version(), 4);
    assert_eq!(graph.topological_order(), &["n0", "n1"]);
    assert!(graph.output_contract().schema.field_with_name("count").is_ok());
}

// ---------------------------------------------------------------------------
// Identita' e compatibilita' (ADR 4)
// ---------------------------------------------------------------------------

#[test]
fn check_compatibility_accepts_the_current_environment() {
    let graph = validate_mixed();
    check_compatibility(&graph, CATALOG, ENGINE_VERSION, ARROW_VERSION, &local_capabilities())
        .expect("grafo compatibile con l'ambiente corrente");
    // Un superset di capability resta compatibile.
    let mut superset = local_capabilities();
    superset.insert("capability_futura");
    check_compatibility(&graph, CATALOG, ENGINE_VERSION, ARROW_VERSION, &superset).expect("superset compatibile");
}

#[test]
fn publish_profile_is_required_and_checked() {
    // ADR 7: il profilo di publish e' una capability del grafo.
    let graph = validate_mixed();
    // Default `AtomicPublish` finche' il formato piano non dichiara un profilo.
    assert!(
        graph
            .required_capabilities()
            .contains(PublishProfile::Atomic.capability_name())
    );
    // Un ambiente senza il profilo di publish richiesto rifiuta il grafo.
    let result = check_compatibility(&graph, CATALOG, ENGINE_VERSION, ARROW_VERSION, &compiled_capabilities());
    match result {
        Err(PlenoraError::Contract(message)) => {
            assert!(message.contains("GRAPH_MISMATCH"), "{message}");
            assert!(message.contains("atomic_publish"), "{message}");
        }
        other => panic!("atteso mismatch capability publish, ottenuto {other:?}"),
    }
    // L'ambiente locale dichiara entrambi i profili implementati (ADR 7).
    assert!(local_capabilities().contains(PublishProfile::Atomic.capability_name()));
    assert!(local_capabilities().contains(PublishProfile::DurableAtomic.capability_name()));
}

#[test]
fn engine_version_mismatch_is_rejected() {
    let graph = validate_mixed();
    let result = check_compatibility(&graph, CATALOG, "0.0.0-altra", ARROW_VERSION, &local_capabilities());
    match result {
        Err(PlenoraError::Contract(message)) => {
            assert!(message.contains("GRAPH_MISMATCH"), "{message}");
            assert!(message.contains("engine_version"), "{message}");
        }
        other => panic!("atteso mismatch engine_version, ottenuto {other:?}"),
    }
}

#[test]
fn arrow_version_mismatch_is_rejected() {
    let graph = validate_mixed();
    // Identita' coerente: la versione Arrow registrata e' quella della build.
    assert_eq!(graph.arrow_version().0, ARROW_VERSION);
    let result = check_compatibility(&graph, CATALOG, ENGINE_VERSION, "0.0.0-altra", &local_capabilities());
    match result {
        Err(PlenoraError::Contract(message)) => {
            assert!(message.contains("GRAPH_MISMATCH"), "{message}");
            assert!(message.contains("arrow_version"), "{message}");
        }
        other => panic!("atteso mismatch arrow_version, ottenuto {other:?}"),
    }
}

#[test]
fn plan_hash_normalizes_integer_and_float_forms() {
    // `100` e `100.0` denotano lo stesso valore: la canonicalizzazione
    // normalizza i numeri (ADR 4) e i due piani producono lo stesso hash.
    let plan_with = |value: serde_json::Value| {
        json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "id", "operator": ">", "value": value}},
            ],
            "output": "f",
        })
        .to_string()
    };
    let int_graph = validate(&plan_with(json!(100)), &input(table_contract())).expect("config intera");
    let float_graph =
        validate(&plan_with(json!(100.0)), &input(table_contract())).expect("config float");
    assert_eq!(int_graph.plan_hash(), float_graph.plan_hash());

    // Oltre 2^53 un intero puo' non avere un f64 esatto: le forme NON sono
    // unificate e gli hash restano distinti (fail-closed, nessun collasso).
    let big_int =
        validate(&plan_with(json!(9_007_199_254_740_994_u64)), &input(table_contract())).expect("int oltre 2^53");
    let big_float =
        validate(&plan_with(json!(9_007_199_254_740_994.0)), &input(table_contract())).expect("float oltre 2^53");
    assert_ne!(big_int.plan_hash(), big_float.plan_hash());
}

#[test]
fn catalog_fingerprint_mismatch_is_rejected() {
    let graph = validate_mixed();

    // Stessa op, semantica cambiata: fingerprint diverso (ADR 4).
    let bumped: Vec<OperationDescriptor> = CATALOG
        .iter()
        .map(|descriptor| {
            let mut clone = descriptor.clone();
            if clone.id == "geo.buffer" {
                clone.semantic_version += 1;
            }
            clone
        })
        .collect();
    let result = check_compatibility(&graph, &bumped, ENGINE_VERSION, ARROW_VERSION, &local_capabilities());
    match result {
        Err(PlenoraError::Contract(message)) => {
            assert!(message.contains("catalog_fingerprint"), "{message}");
        }
        other => panic!("atteso mismatch fingerprint, ottenuto {other:?}"),
    }

    // Op usata rimossa dal catalogo: rifiutata anche a parita' di versioni.
    let without_buffer: Vec<OperationDescriptor> = CATALOG
        .iter()
        .filter(|descriptor| descriptor.id != "geo.buffer")
        .cloned()
        .collect();
    let result = check_compatibility(&graph, &without_buffer, ENGINE_VERSION, ARROW_VERSION, &local_capabilities());
    assert!(matches!(result, Err(PlenoraError::Contract(_))), "{result:?}");

    // Un'op NON usata che cambia non invalida il grafo.
    let untouched: Vec<OperationDescriptor> = CATALOG
        .iter()
        .map(|descriptor| {
            let mut clone = descriptor.clone();
            if clone.id == "table.pivot" {
                clone.kernel_version += 1;
            }
            clone
        })
        .collect();
    check_compatibility(&graph, &untouched, ENGINE_VERSION, ARROW_VERSION, &local_capabilities())
        .expect("op non usata fuori dal fingerprint");
}

#[test]
fn input_contract_mismatch_is_rejected() {
    let graph = validate_mixed();

    // Stesso contratto (anche con FieldId diverso: e' identita' interna del
    // grafo, non dell'input) -> compatibile.
    check_input_compatibility(&graph, &input(geo_contract(99))).expect("stesso contratto");

    // Schema diverso -> mismatch.
    let mut wider = geo_contract(1);
    wider.schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("geom", DataType::Binary, true),
        Field::new("extra", DataType::Utf8, true),
    ]));
    let result = check_input_compatibility(&graph, &input(wider));
    assert!(matches!(result, Err(PlenoraError::Contract(_))), "{result:?}");

    // Geometria con CRS diverso -> mismatch.
    let other_crs = geo_contract_with_crs(1, geographic_crs());
    let result = check_input_compatibility(&graph, &input(other_crs));
    assert!(matches!(result, Err(PlenoraError::Contract(_))), "{result:?}");

    // Input mancante -> mismatch.
    let result = check_input_compatibility(&graph, &[]);
    assert!(matches!(result, Err(PlenoraError::Contract(_))), "{result:?}");
}

#[test]
fn identity_accessors_are_consistent() {
    let graph = validate_mixed();
    let repeat = validate(&mixed_plan_json(), &input(geo_contract(7))).expect("stessa pipeline");
    // Stessa validazione, stessa identita' (determinismo della fase 1).
    assert_eq!(graph.plan_hash(), repeat.plan_hash());
    assert_eq!(graph.catalog_fingerprint(), repeat.catalog_fingerprint());
    assert_eq!(
        graph.input_contract_fingerprints(),
        repeat.input_contract_fingerprints()
    );
    // L'hash e' esadecimale a 64 caratteri (SHA-256).
    assert_eq!(graph.plan_hash().to_hex().len(), 64);
    assert_eq!(format!("{}", graph.catalog_fingerprint()).len(), 64);
    // Limiti effettivi: default del piano non dichiarato.
    assert_eq!(
        graph.effective_limits().rows.max_rows_per_edge,
        plenora_core::limits::Limits::default().rows.max_rows_per_edge
    );
}

// ---------------------------------------------------------------------------
// Regressioni review engine (planner)
// ---------------------------------------------------------------------------

#[test]
fn extra_input_contract_error_is_deterministic() {
    // Piu' contratti extra: il messaggio segnala sempre il primo in ordine
    // lessicografico, non un nome dipendente dall'hash della mappa.
    let contracts = vec![
        ("main".to_owned(), table_contract()),
        ("zeta".to_owned(), table_contract()),
        ("alfa".to_owned(), table_contract()),
    ];
    for _ in 0..8 {
        let error = validate(&mixed_plan_json(), &contracts).expect_err("input extra");
        assert!(
            error.to_string().contains("`alfa`"),
            "segnalato il primo extra in ordine: {error}"
        );
    }
}

#[test]
fn compatibility_extra_contract_error_is_deterministic() {
    let graph = validate_mixed();
    let contracts = vec![
        ("zeta".to_owned(), geo_contract(7)),
        ("main".to_owned(), geo_contract(7)),
        ("alfa".to_owned(), geo_contract(7)),
    ];
    for _ in 0..8 {
        let error = check_input_compatibility(&graph, &contracts).expect_err("input extra");
        assert!(
            error.to_string().contains("`alfa`"),
            "segnalato il primo extra in ordine: {error}"
        );
    }
}

#[test]
fn input_geometry_names_are_not_bound_in_the_field_allocator() {
    // Due input con colonna geometrica omonima: il nome NON e' legato al
    // FieldId rimappato nell'allocatore (altrimenti l'ultimo input
    // vincerebbe). Le chiavi interned dagli analyze non devono collidere con
    // i FieldId delle geometrie di input.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["left", "right"],
        "nodes": [
            {"id": "sl", "op": "table.sort", "in": ["left"], "config": {"columns": ["geom"]}},
            {"id": "ag", "op": "table.aggregate", "in": ["right"],
             "config": {"group_by": ["id"], "aggregations": []}},
            {"id": "j", "op": "table.join", "in": ["sl", "ag"],
             "config": {"left_keys": ["id"], "right_keys": ["id"]}},
        ],
        "output": "j",
    })
    .to_string();
    let contracts = vec![
        ("left".to_owned(), geo_contract(4)),
        ("right".to_owned(), geo_contract(4)),
    ];
    let graph = validate(&plan, &contracts).expect("piano valido");
    let left_geometry = graph.edge_contract("left").unwrap().geometries[0].field_id;
    let right_geometry = graph.edge_contract("right").unwrap().geometries[0].field_id;
    assert_ne!(left_geometry, right_geometry, "namespace globale (D16)");
    let sorted = graph
        .edge_contract("sl")
        .unwrap()
        .properties
        .sorted_by
        .as_ref()
        .and_then(|sorted| sorted.confidence.value().cloned())
        .expect("sorted_by inferita su sl");
    assert!(
        !sorted.contains(&left_geometry) && !sorted.contains(&right_geometry),
        "la chiave interned non collide coi FieldId delle geometrie di input"
    );
}
