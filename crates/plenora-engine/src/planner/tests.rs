//! Test del planner (fase 1 `validate`, Architetture.md par. 6.1, ADR 4/5).

use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::catalog::{OperationDescriptor, CATALOG};
use plenora_core::contract::{
    ContractCrs, ContractProperties, ContractProperty, DataContract, FieldId,
    GeometryColumnContract, GeometryDimensions, GeometryEncoding, PropertyConfidence,
    PropertyScope,
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

/// Campo geometria WKB delle fixture: il marcatore di estensione
/// `geoarrow.wkb` rende la colonna identificabile dal trasporto (ADR-0009,
/// decisione 8 — il check vive in analyze, quindi anche le fixture dei
/// contratti devono essere realistiche).
fn wkb_geometry_field(name: &str) -> Field {
    Field::new(name, DataType::Binary, true).with_metadata(std::collections::HashMap::from([(
        plenora_kernels_geo::arrow_adapter::GEOARROW_EXTENSION_KEY.to_owned(),
        plenora_kernels_geo::arrow_adapter::GEOARROW_WKB_EXTENSION.to_owned(),
    )]))
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
            wkb_geometry_field("geom"),
        ])),
        vec![GeometryColumnContract {
            field_id: FieldId(field_id),
            name: "geom".to_owned(),
            crs: ContractCrs::Resolved(crs),
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto fixture valido")
}

fn input(contract: DataContract) -> Vec<(String, DataContract)> {
    vec![("main".to_owned(), contract)]
}

#[test]
fn row_diagnostics_reject_expression_after_cardinality_change() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "selected", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "derived", "op": "table.expression", "in": ["selected"],
             "config": {"output_column": "ratio",
                        "expression": {"kind": "binary", "op": "divide",
                                       "left": {"kind": "column", "name": "id"},
                                       "right": {"kind": "literal", "value": 2}}}}
        ],
        "output": "derived"
    });

    let error = validate(&plan.to_string(), &input(table_contract()))
        .expect_err("expression dopo filter: provenance row-level non dimostrabile");
    assert!(matches!(error, PlenoraError::InvalidPlan(_)));
    assert!(error.to_string().contains("provenance"));
}

#[test]
fn row_diagnostics_reject_formula_after_cardinality_change() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "ordered", "op": "table.sort", "in": ["main"],
             "config": {"columns": ["id"]}},
            {"id": "derived", "op": "table.formula", "in": ["ordered"],
             "config": {"new_column": "ratio", "formula": "id / 2"}}
        ],
        "output": "derived"
    });

    let error = validate(&plan.to_string(), &input(table_contract()))
        .expect_err("formula dopo sort: provenance row-level non dimostrabile");
    assert!(matches!(error, PlenoraError::InvalidPlan(_)));
    assert!(error.to_string().contains("provenance"));
}

#[test]
fn row_diagnostics_keep_observable_provenance_into_expression_and_formula() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "derived", "op": "table.expression", "in": ["main"],
             "config": {"output_column": "ratio",
                        "expression": {"kind": "binary", "op": "divide",
                                       "left": {"kind": "column", "name": "id"},
                                       "right": {"kind": "literal", "value": 2}}}},
            {"id": "again", "op": "table.formula", "in": ["derived"],
             "config": {"new_column": "twice", "formula": "ratio * 2"}}
        ],
        "output": "again"
    });

    validate(&plan.to_string(), &input(table_contract()))
        .expect("expression e formula conservano cardinalita' e ordine");
}

#[test]
fn row_diagnostics_reject_geo_after_cardinality_change() {
    // geo.subdivide espande le righe: una misura geo a valle emetterebbe
    // diagnostica con indici sorgente non osservabili -> piano rifiutato.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "d", "op": "geo.subdivide", "in": ["main"],
             "config": {"max_vertices": 4}},
            {"id": "l", "op": "geo.length", "in": ["d"], "config": {}},
        ],
        "output": "l",
    })
    .to_string();
    match validate(&plan, &input(geo_contract(1))) {
        Err(PlenoraError::InvalidPlan(message)) => {
            assert!(message.contains("geo.length"), "{message}");
            assert!(message.contains("provenance"), "{message}");
        }
        other => panic!("atteso gate provenance, ottenuto {other:?}"),
    }

    // Stesso vincolo con un nodo tabellare cardinality-changing.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    })
    .to_string();
    match validate(&plan, &input(geo_contract(1))) {
        Err(PlenoraError::InvalidPlan(message)) => {
            assert!(message.contains("geo.buffer"), "{message}");
            assert!(message.contains("provenance"), "{message}");
        }
        other => panic!("atteso gate provenance, ottenuto {other:?}"),
    }

    // Catena geo pura: provenance preservata, piano valido.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
            {"id": "a", "op": "geo.area", "in": ["b"], "config": {}},
        ],
        "output": "a",
    })
    .to_string();
    validate(&plan, &input(geo_contract(1))).expect("catena geo pura valida");
}

#[test]
fn row_diagnostics_reject_unobservable_provenance_after_filter() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "selected", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "cast", "op": "table.type_cast", "in": ["selected"],
             "config": {"column": "name", "target_type": "date32", "errors": "coerce"}}
        ],
        "output": "cast"
    });

    let error = validate(&plan.to_string(), &[("main".to_owned(), table_contract())])
        .expect_err("provenance row-level non dimostrabile");
    assert!(matches!(error, PlenoraError::InvalidPlan(_)));
    assert!(error.to_string().contains("provenance"));
}

#[test]
fn row_diagnostics_hash_gate_follows_null_policy() {
    // P1-3/P2: md5/sha256 rifiutano row-scoped SOLO con null_policy=error —
    // il gate provenance segue la stessa autorita' config-sensitive del
    // catalogo, non una lista statica.
    let plan_with = |operation: &str, config: serde_json::Value| {
        json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "ordered", "op": "table.sort", "in": ["main"],
                 "config": {"columns": ["id"]}},
                {"id": "hash", "op": operation, "in": ["ordered"], "config": config}
            ],
            "output": "hash"
        })
        .to_string()
    };
    for operation in ["table.md5_hash", "table.sha256_hash"] {
        let failure_message =
            format!("{operation} con null_policy=error dopo sort: indici inventabili");
        let error = validate(
            &plan_with(
                operation,
                json!({"columns": ["id"], "null_policy": "error"}),
            ),
            &input(table_contract()),
        )
        .expect_err(&failure_message);
        assert!(matches!(error, PlenoraError::InvalidPlan(_)));
        assert!(error.to_string().contains("provenance"));
        for config in [
            json!({"columns": ["id"]}),
            json!({"columns": ["id"], "null_policy": "empty"}),
            json!({"columns": ["id"], "null_policy": "literal", "null_literal": "<null>"}),
        ] {
            let success_message =
                format!("{operation} senza null_policy=error: nessun rifiuto row-scoped");
            validate(&plan_with(operation, config), &input(table_contract()))
                .expect(&success_message);
        }
    }
}

#[test]
fn row_diagnostics_gate_ignores_hmac_sha256() {
    // P2: hmac_sha256 non emette MAI diagnostica row-scoped (le null_policy
    // legacy producono output dichiarato): nessun gate provenance.
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "ordered", "op": "table.sort", "in": ["main"],
             "config": {"columns": ["id"]}},
            {"id": "mac", "op": "table.hmac_sha256", "in": ["ordered"],
             "config": {"columns": ["id"], "key_env": "PLENORA_PLANNER_TEST_KEY"}}
        ],
        "output": "mac"
    });
    validate(&plan.to_string(), &input(table_contract()))
        .expect("hmac_sha256 dopo sort: non emettendo, non richiede provenance");
}

#[test]
fn row_diagnostics_type_cast_gate_follows_target_type() {
    // Il gate segue l'autorita' anche per type_cast: solo i target con
    // conversione fallibile row-scoped richiedono provenance.
    let plan_with = |config: serde_json::Value| {
        json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "ordered", "op": "table.sort", "in": ["main"],
                 "config": {"columns": ["id"]}},
                {"id": "cast", "op": "table.type_cast", "in": ["ordered"], "config": config}
            ],
            "output": "cast"
        })
        .to_string()
    };
    validate(
        &plan_with(json!({"column": "id", "target_type": "str"})),
        &input(table_contract()),
    )
    .expect("type_cast verso str: totale, nessun rifiuto row-scoped");
    let error = validate(
        &plan_with(json!({"column": "name", "target_type": "date32"})),
        &input(table_contract()),
    )
    .expect_err("type_cast date32 dopo sort: indici inventabili");
    assert!(error.to_string().contains("provenance"));
}

#[test]
fn row_diagnostics_keep_observable_provenance_through_schema_only_nodes() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "rename", "op": "table.rename", "in": ["main"],
             "config": {"renames": [{"old_name": "name", "new_name": "effective_date"}]}},
            {"id": "cast", "op": "table.type_cast", "in": ["rename"],
             "config": {"column": "effective_date", "target_type": "date32", "errors": "coerce"}}
        ],
        "output": "cast"
    });

    validate(&plan.to_string(), &input(table_contract()))
        .expect("rename 1:1 non perde la posizione sorgente osservabile");
}

#[test]
fn contract_canonical_serializes_dimensions_as_icd_strings() {
    // B1.1: il fingerprint dei contratti Xy non cambia — "dimensions" resta
    // la stringa "xy" prodotta anche dalla serializzazione precedente.
    let canonical = contract_canonical(&geo_contract(0));
    assert_eq!(canonical["geometries"][0]["dimensions"], json!("xy"));

    // Tutte le 5 varianti entrano nel fingerprint in forma ICD minuscola.
    for (dimensions, text) in [
        (GeometryDimensions::Xy, "xy"),
        (GeometryDimensions::Xyz, "xyz"),
        (GeometryDimensions::Xym, "xym"),
        (GeometryDimensions::Xyzm, "xyzm"),
        (GeometryDimensions::Unknown, "unknown"),
    ] {
        let mut contract = geo_contract(0);
        contract.geometries[0].dimensions = dimensions;
        let canonical = contract_canonical(&contract);
        assert_eq!(canonical["geometries"][0]["dimensions"], json!(text));
    }
}

#[test]
fn contract_canonical_omits_encoding_unless_declared() {
    // B1.3: un contratto Xy senza encoding produce ESATTAMENTE lo stesso
    // JSON di prima (chiave assente, non null) — fingerprint invariato.
    let without = contract_canonical(&geo_contract(0));
    let geometry = &without["geometries"][0];
    assert!(geometry.get("encoding").is_none());
    assert_eq!(
        geometry,
        &json!({
            "name": "geom",
            "crs": {
                "definition": "EPSG:32632",
                "kind": "projected",
                "horizontal_unit_to_metre": 1.0_f64.to_bits(),
            },
            "dimensions": "xy",
            "nullable": true,
        })
    );

    // Encoding dichiarato: entra nel fingerprint in forma ICD minuscola.
    for (encoding, text) in [
        (GeometryEncoding::Wkb, "wkb"),
        (GeometryEncoding::Ewkb, "ewkb"),
    ] {
        let mut contract = geo_contract(0);
        contract.geometries[0].encoding = Some(encoding);
        let canonical = contract_canonical(&contract);
        assert_eq!(canonical["geometries"][0]["encoding"], json!(text));
    }
    // E cambia il fingerprint rispetto al contratto senza encoding.
    let mut contract = geo_contract(0);
    contract.geometries[0].encoding = Some(GeometryEncoding::Wkb);
    assert_ne!(
        contract_fingerprint(&contract).unwrap(),
        contract_fingerprint(&geo_contract(0)).unwrap()
    );
}

/// Pipeline mista table+geo: buffer -> filter -> aggregate.
///
/// L'op geo precede il nodo cardinality-changing: `geo.buffer` emette
/// diagnostica row-scoped e richiede provenance originale osservabile
/// (contratto trasversale), quindi non puo' stare a valle di `table.filter`.
fn mixed_plan_json() -> String {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 100.0}},
            {"id": "f", "op": "table.filter", "in": ["b"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "g", "op": "table.aggregate", "in": ["f"],
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
    assert_eq!(graph.topological_order(), &["b", "f", "g"]);
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

    // La geometria segue la colonna attraverso geo.buffer e table.filter con
    // lo stesso FieldId (rimappato, non quello del lettore).
    let input_geometry = graph.edge_contract("main").unwrap().geometries[0].field_id;
    assert_ne!(input_geometry, FieldId(7), "FieldId di input rimappato");
    let buffered = graph.edge_contract("b").unwrap();
    assert_eq!(buffered.geometries.len(), 1);
    assert_eq!(buffered.geometries[0].field_id, input_geometry);
    let filtered = graph.edge_contract("f").unwrap();
    assert_eq!(filtered.geometries[0].field_id, input_geometry);

    // table.aggregate senza la geometria in group_by la perde: output tabellare.
    let output = graph.output_contract().unwrap();
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
    assert!(graph
        .output_contract()
        .unwrap()
        .schema
        .field_with_name("id")
        .is_ok());
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
    let joined = graph.output_contract().unwrap();
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

// ---------------------------------------------------------------------------
// R4.6.3 (contratti trasversali v2.0-rc9/rc10): il requisito di CRS
// risolvibile e' condizionato alle operazioni che lo usano.
// ---------------------------------------------------------------------------

/// Contratto con geometria SENZA CRS dichiarato (`ContractCrs::Missing`):
/// lo stato che la discovery produce per una colonna `GeoArrow` senza alcuna
/// rappresentazione CRS accettata (R4.4: mai un CRS inventato).
fn geo_contract_missing_crs(field_id: u32) -> DataContract {
    DataContract::new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            wkb_geometry_field("geom"),
        ])),
        vec![GeometryColumnContract {
            field_id: FieldId(field_id),
            name: "geom".to_owned(),
            crs: ContractCrs::Missing,
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto fixture valido")
}

#[test]
fn missing_crs_enters_the_contract_fingerprint() {
    // ADR 4 + R4.6.3: un contratto con CRS risolto e uno con CRS mancante
    // NON hanno lo stesso fingerprint — altrimenti un piano validato su
    // input risolto accetterebbe in riesecuzione un input senza CRS senza
    // rivalidazione, spostando il fallimento a runtime. La forma risolta
    // resta byte-identica (test `contract_canonical_omits_encoding_*`).
    let resolved = contract_fingerprint(&geo_contract(0)).expect("fingerprint risolto");
    let missing = contract_fingerprint(&geo_contract_missing_crs(0)).expect("fingerprint missing");
    assert_ne!(resolved, missing);
    assert_eq!(
        contract_fingerprint(&geo_contract_missing_crs(0)).expect("fingerprint missing"),
        missing,
        "fingerprint deterministico (ADR-0001)"
    );
    // Forma canonica: lo stato entra col valore R2.2.
    let canonical = contract_canonical(&geo_contract_missing_crs(0));
    assert_eq!(canonical["geometries"][0]["crs"], json!("missing"));
}

/// Contratto con geometria a CRS dichiarato non risolto
/// (`ContractCrs::DeclaredUnresolved`, R4.6.3): lo stato che la discovery
/// produce per un'incoerenza dichiarata o un conflitto decidibile.
fn geo_contract_declared_unresolved_crs(field_id: u32) -> DataContract {
    DataContract::new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            wkb_geometry_field("geom"),
        ])),
        vec![GeometryColumnContract {
            field_id: FieldId(field_id),
            name: "geom".to_owned(),
            crs: ContractCrs::DeclaredUnresolved {
                crs_id: Some("EPSG:99999".to_owned()),
                definition: None,
                definition_format: None,
            },
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto fixture valido")
}

#[test]
fn declared_unresolved_enters_the_contract_fingerprint_with_declarations() {
    // ADR 4 + R4.1: i tre stati NON collassano — resolved, missing e
    // declared_unresolved producono tre fingerprint distinti, e due
    // incoerenze con dichiarazioni diverse non sono lo stesso contratto.
    let declared = contract_fingerprint(&geo_contract_declared_unresolved_crs(0))
        .expect("fingerprint declared_unresolved");
    assert_ne!(
        declared,
        contract_fingerprint(&geo_contract(0)).expect("risolto")
    );
    assert_ne!(
        declared,
        contract_fingerprint(&geo_contract_missing_crs(0)).expect("missing")
    );
    assert_eq!(
        contract_fingerprint(&geo_contract_declared_unresolved_crs(0))
            .expect("fingerprint declared_unresolved"),
        declared,
        "fingerprint deterministico (ADR-0001)"
    );
    // Forma canonica: lo stato entra con le dichiarazioni.
    let canonical = contract_canonical(&geo_contract_declared_unresolved_crs(0));
    let crs = &canonical["geometries"][0]["crs"];
    assert_eq!(crs["resolution"], json!("declared_unresolved"));
    assert_eq!(crs["crs_id"], json!("EPSG:99999"));
}

#[test]
fn table_ops_propagate_declared_unresolved_unchanged() {
    // R4.6.3: il centro non pretende un CRS risolvibile per operazioni che
    // non lo richiedono — un filtro tabellare valida e propaga l'incoerenza
    // dichiarata INVARIATA (R4.6.4: arriva al bordo di scrittura con le
    // dichiarazioni originali).
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
    .to_string();
    let graph =
        validate(&plan, &input(geo_contract_declared_unresolved_crs(1))).expect("validazione");
    let output = graph.output_contract().expect("contratto di output");
    assert_eq!(output.geometries.len(), 1);
    let ContractCrs::DeclaredUnresolved {
        crs_id, definition, ..
    } = &output.geometries[0].crs
    else {
        panic!(
            "l'incoerenza dichiarata si propaga invariata: {:?}",
            output.geometries[0].crs
        );
    };
    assert_eq!(crs_id.as_deref(), Some("EPSG:99999"));
    assert_eq!(definition, &None);
}

#[test]
fn table_ops_validate_on_missing_crs_geometry_and_propagate_it() {
    // R4.6.3: un filtro tabellare su una colonna non geometrica non ha
    // bisogno di alcun CRS — rifiutarlo sarebbe piu' restrittivo del ruolo.
    // R4.6.4: lo stato `missing` attraversa invariato il contratto di
    // output (propagare non e' tollerare: le chiavi §2 lo dichiarano).
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
    .to_string();
    let graph = validate(&plan, &input(geo_contract_missing_crs(1))).expect("validazione");
    let output = graph.output_contract().expect("contratto di output");
    assert_eq!(output.geometries.len(), 1);
    assert!(
        matches!(output.geometries[0].crs, ContractCrs::Missing),
        "lo stato mancante si propaga invariato"
    );
}

#[test]
fn geo_op_on_missing_crs_fails_with_the_declared_cause() {
    // R4.6.3: il fallimento si sposta al punto in cui un'op con
    // `CrsRequirement` tocca la colonna (analyze, a compile-plan) — la
    // categoria e' `Crs` come ogni requisito CRS non soddisfatto e il
    // messaggio dichiara la causa, non l'ultimo tentativo di lettura.
    let buffer = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    })
    .to_string();
    let result = validate(&buffer, &input(geo_contract_missing_crs(1)));
    match result {
        Err(PlenoraError::Crs(message)) => {
            assert!(
                message.contains("nessun CRS dichiarato in alcuna rappresentazione accettata"),
                "{message}"
            );
        }
        other => panic!("atteso errore Crs per CRS mancante, ottenuto {other:?}"),
    }

    // Anche a valle di op tabellari: lo stato mancante si propaga nel
    // contratto e ferma l'op geo quando la raggiunge. `table.rename`
    // preserva la provenance (a differenza di `table.filter`, che la
    // invaliderebbe e verrebbe rifiutata dal gate row-diagnostics).
    let chained = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "r", "op": "table.rename", "in": ["main"],
             "config": {"renames": [{"old_name": "id", "new_name": "id2"}]}},
            {"id": "b", "op": "geo.buffer", "in": ["r"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    })
    .to_string();
    let result = validate(&chained, &input(geo_contract_missing_crs(1)));
    match result {
        Err(PlenoraError::Crs(message)) => {
            assert!(message.contains("nodo `b`"), "contesto nodo: {message}");
            assert!(
                message.contains("nessun CRS dichiarato in alcuna rappresentazione accettata"),
                "{message}"
            );
        }
        other => panic!("atteso errore Crs per CRS mancante, ottenuto {other:?}"),
    }
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
    check_compatibility(
        &graph,
        CATALOG,
        ENGINE_VERSION,
        ARROW_VERSION,
        &local_capabilities(),
    )
    .expect("ambiente coerente");
    // Un ambiente senza geos rifiuta il grafo (ADR 4).
    let result = check_compatibility(
        &graph,
        CATALOG,
        ENGINE_VERSION,
        ARROW_VERSION,
        &CapabilitySet::default(),
    );
    assert!(
        matches!(result, Err(PlenoraError::InvalidPlan(_))),
        "{result:?}"
    );
}

#[test]
fn input_contracts_must_match_declared_inputs() {
    let plan = mixed_plan_json();

    let missing = validate(&plan, &[]);
    assert!(
        matches!(missing, Err(PlenoraError::InvalidPlan(_))),
        "{missing:?}"
    );

    let extra = validate(
        &plan,
        &[
            ("main".to_owned(), geo_contract(1)),
            ("other".to_owned(), table_contract()),
        ],
    );
    assert!(
        matches!(extra, Err(PlenoraError::InvalidPlan(_))),
        "{extra:?}"
    );

    let duplicate = validate(
        &plan,
        &[
            ("main".to_owned(), geo_contract(1)),
            ("main".to_owned(), geo_contract(2)),
        ],
    );
    assert!(
        matches!(duplicate, Err(PlenoraError::InvalidPlan(_))),
        "{duplicate:?}"
    );
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
        Err(PlenoraError::InvalidPlan(message)) => {
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
    // L'op geo precede `table.filter` (cardinality-changing): il gate
    // provenance row-diagnostics rifiuterebbe l'ordine inverso.
    let aliased = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo_buffer", "in": ["main"], "config": {"distance": 100.0}},
            {"id": "f", "op": "filter", "in": ["b"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
    .to_string();
    let canonical = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 100.0}},
            {"id": "f", "op": "table.filter", "in": ["b"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
    .to_string();
    let from_alias = validate(&aliased, &input(geo_contract(1))).expect("alias risolti");
    let from_canonical = validate(&canonical, &input(geo_contract(1))).expect("canonico");
    assert_eq!(from_alias.plan_hash(), from_canonical.plan_hash());
    assert_eq!(
        from_alias.catalog_fingerprint(),
        from_canonical.catalog_fingerprint()
    );
}

#[test]
fn equivalent_plans_share_plan_hash() {
    // Ordine dei nodi nel JSON e config omessa vs `{}` esplicita sono
    // irrilevanti: il piano canonico e' lo stesso (ADR 4).
    let sparse = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 100.0}},
            {"id": "c", "op": "geo.centroid", "in": ["b"]},
            {"id": "f", "op": "table.filter", "in": ["c"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
    .to_string();
    let explicit = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["c"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "c", "op": "geo.centroid", "in": ["b"], "config": {}},
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 100.0}},
        ],
        "output": "f",
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
    assert!(graph
        .output_contract()
        .unwrap()
        .schema
        .field_with_name("count")
        .is_ok());
}

// ---------------------------------------------------------------------------
// Identita' e compatibilita' (ADR 4)
// ---------------------------------------------------------------------------

#[test]
fn check_compatibility_accepts_the_current_environment() {
    let graph = validate_mixed();
    check_compatibility(
        &graph,
        CATALOG,
        ENGINE_VERSION,
        ARROW_VERSION,
        &local_capabilities(),
    )
    .expect("grafo compatibile con l'ambiente corrente");
    // Un superset di capability resta compatibile.
    let mut superset = local_capabilities();
    superset.insert("capability_futura");
    check_compatibility(&graph, CATALOG, ENGINE_VERSION, ARROW_VERSION, &superset)
        .expect("superset compatibile");
}

#[test]
fn publish_profile_is_required_and_checked() {
    // ADR 7: il profilo di publish e' una capability del grafo.
    let graph = validate_mixed();
    // Default `AtomicPublish` finche' il formato piano non dichiara un profilo.
    assert!(graph
        .required_capabilities()
        .contains(PublishProfile::Atomic.capability_name()));
    // Un ambiente senza il profilo di publish richiesto rifiuta il grafo.
    let result = check_compatibility(
        &graph,
        CATALOG,
        ENGINE_VERSION,
        ARROW_VERSION,
        &compiled_capabilities(),
    );
    match result {
        Err(PlenoraError::InvalidPlan(message)) => {
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
    let result = check_compatibility(
        &graph,
        CATALOG,
        "0.0.0-altra",
        ARROW_VERSION,
        &local_capabilities(),
    );
    match result {
        Err(PlenoraError::InvalidPlan(message)) => {
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
    let result = check_compatibility(
        &graph,
        CATALOG,
        ENGINE_VERSION,
        "0.0.0-altra",
        &local_capabilities(),
    );
    match result {
        Err(PlenoraError::InvalidPlan(message)) => {
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
    let int_graph =
        validate(&plan_with(json!(100)), &input(table_contract())).expect("config intera");
    let float_graph =
        validate(&plan_with(json!(100.0)), &input(table_contract())).expect("config float");
    assert_eq!(int_graph.plan_hash(), float_graph.plan_hash());

    // Oltre 2^53 un intero puo' non avere un f64 esatto: le forme NON sono
    // unificate e gli hash restano distinti (fail-closed, nessun collasso).
    let big_int = validate(
        &plan_with(json!(9_007_199_254_740_994_u64)),
        &input(table_contract()),
    )
    .expect("int oltre 2^53");
    let big_float = validate(
        &plan_with(json!(9_007_199_254_740_994.0)),
        &input(table_contract()),
    )
    .expect("float oltre 2^53");
    assert_ne!(big_int.plan_hash(), big_float.plan_hash());

    // Regressione (collisione plan_hash): 2^53 e 2^53+1 sono lo stesso
    // double. Canonicalizzare gli interi passando per f64 dava a due config
    // distinte lo stesso hash, rendendo insicuro il riuso del piano.
    let exact = validate(
        &plan_with(json!(9_007_199_254_740_992_u64)),
        &input(table_contract()),
    )
    .expect("2^53");
    let odd = validate(
        &plan_with(json!(9_007_199_254_740_993_u64)),
        &input(table_contract()),
    )
    .expect("2^53+1");
    assert_ne!(exact.plan_hash(), odd.plan_hash());
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
    let result = check_compatibility(
        &graph,
        &bumped,
        ENGINE_VERSION,
        ARROW_VERSION,
        &local_capabilities(),
    );
    match result {
        Err(PlenoraError::InvalidPlan(message)) => {
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
    let result = check_compatibility(
        &graph,
        &without_buffer,
        ENGINE_VERSION,
        ARROW_VERSION,
        &local_capabilities(),
    );
    assert!(
        matches!(result, Err(PlenoraError::InvalidPlan(_))),
        "{result:?}"
    );

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
    check_compatibility(
        &graph,
        &untouched,
        ENGINE_VERSION,
        ARROW_VERSION,
        &local_capabilities(),
    )
    .expect("op non usata fuori dal fingerprint");
}

#[test]
fn row_diagnostics_version_bumps_move_the_plan_fingerprint() {
    // ADR-0004 (delta row-diagnostics 2026-08-03): i bump di versione devono
    // invalidare i grafi validati contro la baseline af812aa. Per ogni op
    // rappresentativa: si valida un piano che la usa col catalogo corrente e
    // si verifica che un catalogo riportato ALLE VERSIONI DI BASELINE
    // produca mismatch di `catalog_fingerprint` — prova che il bump e' nel
    // perimetro del fingerprint per-op, non solo nel descrittore.
    let assert_baseline_mismatch =
        |graph: &ValidatedGraph, op_id: &str, baseline: &dyn Fn(&mut OperationDescriptor)| {
            let reverted: Vec<OperationDescriptor> = CATALOG
                .iter()
                .map(|descriptor| {
                    let mut clone = descriptor.clone();
                    if clone.id == op_id {
                        baseline(&mut clone);
                    }
                    clone
                })
                .collect();
            let result = check_compatibility(
                graph,
                &reverted,
                ENGINE_VERSION,
                ARROW_VERSION,
                &local_capabilities(),
            );
            match result {
                Err(PlenoraError::InvalidPlan(message)) => {
                    assert!(
                        message.contains("catalog_fingerprint"),
                        "{op_id}: {message}"
                    );
                }
                other => panic!("{op_id}: atteso mismatch fingerprint sulla baseline, {other:?}"),
            }
        };

    // table.formula: baseline semantic 1 / kernel 2 (nuovo reject_rows).
    let formula_plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "derived", "op": "table.formula", "in": ["main"],
             "config": {"new_column": "ratio", "formula": "id / 2"}}
        ],
        "output": "derived"
    })
    .to_string();
    let graph = validate(&formula_plan, &input(table_contract())).expect("piano formula");
    assert_baseline_mismatch(&graph, "table.formula", &|descriptor| {
        descriptor.semantic_version = 1;
        descriptor.kernel_version = 2;
    });

    // table.expression: baseline semantic 2 / kernel 3 (bump preesistente
    // expression-v2; il delta row-diagnostics alza semantic 3 / kernel 4).
    let expression_plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "derived", "op": "table.expression", "in": ["main"],
             "config": {"output_column": "ratio",
                        "expression": {"kind": "binary", "op": "divide",
                                       "left": {"kind": "column", "name": "id"},
                                       "right": {"kind": "literal", "value": 2}}}}
        ],
        "output": "derived"
    })
    .to_string();
    let graph = validate(&expression_plan, &input(table_contract())).expect("piano expression");
    assert_baseline_mismatch(&graph, "table.expression", &|descriptor| {
        descriptor.semantic_version = 2;
        descriptor.kernel_version = 3;
    });

    // table.type_cast: solo kernel_version bumpata (2 -> 3, nuova
    // implementazione diagnostica) — il fingerprint deve vedere ANCHE il
    // bump kernel-only.
    let cast_plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "cast", "op": "table.type_cast", "in": ["main"],
             "config": {"column": "name", "target_type": "date32"}}
        ],
        "output": "cast"
    })
    .to_string();
    let graph = validate(&cast_plan, &input(table_contract())).expect("piano type_cast");
    assert_baseline_mismatch(&graph, "table.type_cast", &|descriptor| {
        descriptor.kernel_version = 2;
    });

    // geo.buffer (diag-transport): baseline semantic 1 — l'errore row-scoped
    // ora porta il payload di diagnostica (bump semantico, kernel invariato).
    let graph = validate_mixed();
    assert_baseline_mismatch(&graph, "geo.buffer", &|descriptor| {
        descriptor.semantic_version = 1;
    });
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
    assert!(
        matches!(result, Err(PlenoraError::InvalidPlan(_))),
        "{result:?}"
    );

    // Geometria con CRS diverso -> mismatch.
    let other_crs = geo_contract_with_crs(1, geographic_crs());
    let result = check_input_compatibility(&graph, &input(other_crs));
    assert!(
        matches!(result, Err(PlenoraError::InvalidPlan(_))),
        "{result:?}"
    );

    // Input mancante -> mismatch.
    let result = check_input_compatibility(&graph, &[]);
    assert!(
        matches!(result, Err(PlenoraError::InvalidPlan(_))),
        "{result:?}"
    );
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
        plenora_core::limits::Limits::default()
            .rows
            .max_rows_per_edge
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

// ---------------------------------------------------------------------------
// Snapshot canonico del catalogo (disciplina ADR 4)
// ---------------------------------------------------------------------------

/// Percorso dello snapshot committato (`tests/catalog_snapshot.snap`).
const CATALOG_SNAPSHOT_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/catalog_snapshot.snap");

/// Contenuto canonico dello snapshot: i descrittori di TUTTE le op del
/// catalogo in ordine stabile (per id, lo stesso ordine che
/// [`catalog_fingerprint`] richiede al chiamante), JSON pretty-printed
/// a chiavi ordinate per un diff leggibile in review.
///
/// La forma e' un SUPERINSIEME di [`descriptor_canonical`]: ADR-0012 D12.2
/// (decisione deliberata) tiene `geo_fusion` FUORI dal fingerprint
/// (capability fisica, non semantica) ma DENTRO lo snapshot — ogni cambio di
/// fondibilita' resta un diff esplicito in PR.
fn catalog_snapshot_content() -> String {
    let mut descriptors: Vec<&OperationDescriptor> = CATALOG.iter().collect();
    descriptors.sort_by(|left, right| left.id.cmp(right.id));
    let canonical: Vec<Value> = descriptors
        .iter()
        .map(|descriptor| {
            let mut value = descriptor_canonical(descriptor);
            if let Value::Object(map) = &mut value {
                map.insert(
                    "geo_fusion".to_owned(),
                    Value::String(descriptor.geo_fusion.as_str().to_owned()),
                );
            }
            value
        })
        .collect();
    let mut content =
        serde_json::to_string_pretty(&canonical).expect("la serializzazione non fallisce");
    content.push('\n');
    content
}

/// Snapshot test del catalogo (ADR 4): il catalogo reale deve coincidere
/// con lo snapshot committato `crates/plenora-engine/tests/catalog_snapshot.snap`.
///
/// Qualunque PR che cambi un descrittore (campi, versioni per-componente,
/// vincoli di espansione, maturity, ...) mostra il diff dello snapshot in
/// review; un cambiamento NON intenzionale fallisce qui.
///
/// Rigenerazione dopo un cambiamento intenzionale del catalogo:
///
/// ```sh
/// PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-engine catalog_matches_committed_snapshot
/// ```
///
/// e commit dello snapshot aggiornato insieme alla modifica del catalogo.
#[test]
fn catalog_matches_committed_snapshot() {
    let actual = catalog_snapshot_content();
    let path = std::path::Path::new(CATALOG_SNAPSHOT_PATH);
    if std::env::var_os("PLENORA_UPDATE_SNAPSHOT").is_some() {
        std::fs::write(path, &actual).expect("rigenerazione dello snapshot del catalogo");
        eprintln!("snapshot del catalogo rigenerato in {CATALOG_SNAPSHOT_PATH}");
        return;
    }
    let expected = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "snapshot del catalogo non leggibile ({error}): generarlo con PLENORA_UPDATE_SNAPSHOT=1"
        )
    });
    // Il confronto e' insensibile agli a-capo: su Windows il checkout puo'
    // produrre CRLF (nessun .gitattributes imponeva LF fino al 2026-07-27).
    let expected = expected.replace("\r\n", "\n");
    assert!(
        actual == expected,
        "il catalogo diverge dallo snapshot committato {CATALOG_SNAPSHOT_PATH}: \
         se il cambiamento e' intenzionale, rigenerare con \
         `PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-engine catalog_matches_committed_snapshot` \
         e committare lo snapshot aggiornato"
    );
}
