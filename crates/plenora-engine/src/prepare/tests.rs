//! Test del preparer (Fase 2A-4, Architetture.md par. 6.3, ADR 5).

use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractProperties, DataContract, FieldId, GeometryColumnContract, GeometryDimensions,
    RuntimeStatistic,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_core::PlenoraError;

use super::*;
use crate::planner::validate;

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

fn table_contract() -> DataContract {
    DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ])))
}

fn geo_contract() -> DataContract {
    DataContract::new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geom", DataType::Binary, true),
        ])),
        vec![GeometryColumnContract {
            field_id: FieldId(3),
            name: "geom".to_owned(),
            crs: projected_crs(),
            dimensions: GeometryDimensions::Xy,
            nullable: true,
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto fixture valido")
}

fn validate_plan(plan: &serde_json::Value, contract: DataContract) -> ValidatedGraph {
    validate(&plan.to_string(), &[("main".to_owned(), contract)]).expect("piano valido")
}

// ---------------------------------------------------------------------------
// Scomposizione in segmenti
// ---------------------------------------------------------------------------

#[test]
fn linear_table_chain_fuses_into_one_streaming_segment() {
    let graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "id", "operator": ">", "value": 0}},
                {"id": "r", "op": "table.rename", "in": ["f"],
                 "config": {"renames": [{"old_name": "name", "new_name": "label"}]}},
                {"id": "g", "op": "table.aggregate", "in": ["r"],
                 "config": {"group_by": ["id"], "aggregations": []}},
            ],
            "output": "g",
        }),
        table_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    assert_eq!(plan.segments().len(), 2);
    let streaming = &plan.segments()[0];
    assert_eq!(streaming.mode, SegmentMode::LinearStreaming);
    assert_eq!(streaming.parallelism, ParallelismStrategy::SerialFused);
    assert_eq!(streaming.kernels.len(), 2);
    assert_eq!(streaming.input_edges.as_ref(), &["main".to_owned()]);
    assert_eq!(streaming.output_edge, "r");
    let blocking = &plan.segments()[1];
    assert_eq!(blocking.mode, SegmentMode::Blocking);
    assert_eq!(blocking.parallelism, ParallelismStrategy::BlockingSingleTask);
    assert_eq!(blocking.kernels.len(), 1);
    assert_eq!(plan.output_edge(), "g");
    assert_eq!(plan.last_consumers().get("main"), Some(&LastConsumer::Node("f".into())));
    assert_eq!(plan.last_consumers().get("g"), Some(&LastConsumer::Output));
}

#[test]
fn pure_geo_chain_is_geo_fused_mixed_chain_is_linear() {
    let geo_graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 10.0}},
                {"id": "c", "op": "geo.centroid", "in": ["b"], "config": {}},
            ],
            "output": "c",
        }),
        geo_contract(),
    );
    let plan = prepare(&geo_graph, &RuntimeContext::default()).expect("prepare");
    assert_eq!(plan.segments().len(), 1);
    assert_eq!(plan.segments()[0].mode, SegmentMode::GeoFused);
    assert!(
        plan.segments()[0]
            .kernels
            .iter()
            .all(|kernel| kernel.geo_role == Some(GeoRole::TransformInPlace))
    );

    let mixed_graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "id", "operator": ">", "value": 0}},
                {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 10.0}},
                {"id": "a", "op": "geo.area", "in": ["b"], "config": {}},
            ],
            "output": "a",
        }),
        geo_contract(),
    );
    let plan = prepare(&mixed_graph, &RuntimeContext::default()).expect("prepare");
    assert_eq!(plan.segments().len(), 1);
    assert_eq!(plan.segments()[0].mode, SegmentMode::LinearStreaming);
    let measure = &plan.segments()[0].kernels[2];
    assert_eq!(measure.geo_role, Some(GeoRole::MeasureAddColumn));
    match &measure.config {
        PreparedConfig::GeoMeasure {
            measure,
            output_column,
        } => {
            assert_eq!(*measure, MeasureKind::Area);
            assert_eq!(output_column, "area");
        }
        other => panic!("config inattesa: {other:?}"),
    }
    // Indice della colonna geometria risolto in prepare (E1/V2).
    assert_eq!(measure.geometry_column_index, Some(1));
}

#[test]
fn fan_out_breaks_fusion_and_marks_materialization() {
    let graph = validate(
        &json!({
            "schema_version": 4,
            "inputs": ["left_in", "right_in"],
            "nodes": [
                {"id": "l", "op": "table.filter", "in": ["left_in"],
                 "config": {"column": "id", "operator": ">", "value": 0}},
                {"id": "r", "op": "table.filter", "in": ["right_in"],
                 "config": {"column": "id", "operator": "<", "value": 100}},
                {"id": "j", "op": "table.join", "in": ["l", "r"],
                 "config": {"left_keys": ["id"], "right_keys": ["id"]}},
            ],
            "output": "j",
        })
        .to_string(),
        &[
            ("left_in".to_owned(), table_contract()),
            ("right_in".to_owned(), table_contract()),
        ],
    )
    .expect("piano valido");
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    assert_eq!(plan.segments().len(), 3);
    let join_segment = plan
        .segments()
        .iter()
        .find(|segment| segment.mode == SegmentMode::BinaryBlocking)
        .expect("segmento binario");
    assert_eq!(join_segment.input_edges.as_ref(), &["l".to_owned(), "r".to_owned()]);
    assert!(!plan
        .segments()
        .iter()
        .any(|segment| segment.materialize_output));

    // Fan-out: lo stesso arco alimenta due nodi -> materializzazione prevista.
    let fanout_graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "a", "op": "table.filter", "in": ["main"],
                 "config": {"column": "id", "operator": ">", "value": 0}},
                {"id": "b", "op": "table.filter", "in": ["a"],
                 "config": {"column": "id", "operator": "<", "value": 10}},
                {"id": "c", "op": "table.filter", "in": ["a"],
                 "config": {"column": "id", "operator": ">", "value": 1}},
                {"id": "j", "op": "table.concat", "in": ["b", "c"], "config": {}},
            ],
            "output": "j",
        }),
        table_contract(),
    );
    let plan = prepare(&fanout_graph, &RuntimeContext::default()).expect("prepare");
    let producer = plan
        .segments()
        .iter()
        .find(|segment| segment.output_edge == "a")
        .expect("segmento produttore");
    assert!(
        producer.materialize_output,
        "il fan-out deve essere una materializzazione esplicita del piano"
    );
}

#[test]
fn pass_through_plan_prepares_without_segments() {
    let graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [],
            "output": "main",
        }),
        table_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");
    assert!(plan.segments().is_empty());
    assert_eq!(plan.output_edge(), "main");
    assert_eq!(plan.last_consumers().get("main"), Some(&LastConsumer::Output));
}

#[test]
fn unknown_statistics_are_the_conservative_default() {
    let graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "id", "operator": ">", "value": 0}},
            ],
            "output": "f",
        }),
        table_contract(),
    );
    let runtime = RuntimeContext::default();
    assert_eq!(
        runtime.input_statistics("main"),
        InputStatistics {
            rows: RuntimeStatistic::Unknown,
            batches: RuntimeStatistic::Unknown,
        }
    );
    let plan = prepare(&graph, &runtime).expect("prepare con statistiche assenti");
    assert_eq!(
        plan.input_statistics()["main"].rows,
        RuntimeStatistic::Unknown
    );

    // Statistiche Known: registrate nel piano, nessuna scelta fisica v1
    // dipende da esse (ADR 5).
    let mut runtime = RuntimeContext::default();
    runtime.statistics.insert(
        "main".to_owned(),
        InputStatistics {
            rows: RuntimeStatistic::Known(1_000),
            batches: RuntimeStatistic::Known(4),
        },
    );
    let plan = prepare(&graph, &runtime).expect("prepare con statistiche note");
    assert_eq!(
        plan.input_statistics()["main"].rows,
        RuntimeStatistic::Known(1_000)
    );
    assert_eq!(plan.segments()[0].parallelism, ParallelismStrategy::SerialFused);
}

#[test]
fn unsupported_geo_op_fails_at_prepare_not_mid_stream() {
    let graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "d", "op": "geo.dissolve", "in": ["main"], "config": {}},
            ],
            "output": "d",
        }),
        geo_contract(),
    );
    let result = prepare(&graph, &RuntimeContext::default());
    assert!(matches!(result, Err(PlenoraError::Unsupported(_))));
}

// ---------------------------------------------------------------------------
// Estensioni geo v1.1-v1.3 e table v1.1-v1.3 (classificazione segmenti)
// ---------------------------------------------------------------------------

/// WKB little-endian di `POINT(0 0)` (riferimento/secondo operando da config).
const POINT_WKB_HEX: &str = "010100000000000000000000000000000000000000";

#[test]
fn streaming_geo_extensions_fuse_with_their_roles() {
    let graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "s", "op": "geo.snap", "in": ["main"],
                 "config": {"reference_wkb": POINT_WKB_HEX, "tolerance": 0.5}},
                {"id": "d", "op": "geo.subdivide", "in": ["s"],
                 "config": {"max_vertices": 8}},
                {"id": "a", "op": "geo.geometry_accessors", "in": ["d"], "config": {}},
            ],
            "output": "a",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    assert_eq!(plan.segments().len(), 1);
    let segment = &plan.segments()[0];
    assert_eq!(segment.mode, SegmentMode::GeoFused);
    let roles: Vec<Option<GeoRole>> = segment.kernels.iter().map(|kernel| kernel.geo_role).collect();
    assert_eq!(
        roles,
        vec![
            Some(GeoRole::TransformInPlace),
            Some(GeoRole::OneToMany),
            Some(GeoRole::MeasureAddColumn),
        ]
    );
    // Config tipizzate risolte in prepare (E1): riferimento decodificato,
    // soglia e colonne accessorie gia' pronte per il loop per batch.
    match &segment.kernels[0].config {
        PreparedConfig::GeoSnap { tolerance, .. } => assert!((*tolerance - 0.5).abs() < f64::EPSILON),
        other => panic!("config inattesa: {other:?}"),
    }
    match &segment.kernels[1].config {
        PreparedConfig::GeoSubdivide { max_vertices } => assert_eq!(*max_vertices, 8),
        other => panic!("config inattesa: {other:?}"),
    }
    match &segment.kernels[2].config {
        PreparedConfig::GeoAccessors { columns } => assert_eq!(columns.len(), 6),
        other => panic!("config inattesa: {other:?}"),
    }
}

#[test]
fn line_locate_point_prepares_typed_point_and_output_column() {
    let graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "p", "op": "geo.line_locate_point", "in": ["main"],
                 "config": {"point_wkb": POINT_WKB_HEX}},
            ],
            "output": "p",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    let segment = &plan.segments()[0];
    assert_eq!(segment.mode, SegmentMode::GeoFused);
    match &segment.kernels[0].config {
        PreparedConfig::GeoLineLocatePoint { point, output_column } => {
            assert_eq!((point.x(), point.y()), (0.0, 0.0));
            assert_eq!(output_column, "fraction");
        }
        other => panic!("config inattesa: {other:?}"),
    }
}

#[test]
fn blocking_geo_extensions_are_single_blocking_segments() {
    let cases = [
        ("geo.collect", json!({"group_by": ["id"]}), GeoRole::WholeTable),
        ("geo.coverage_validate", json!({}), GeoRole::WholeTable),
        ("geo.shared_paths", json!({}), GeoRole::WholeTable),
        (
            "geo.cluster_dbscan",
            json!({"eps": 1.0, "min_points": 2}),
            GeoRole::MeasureAddColumn,
        ),
    ];
    for (op, config, role) in cases {
        let graph = validate_plan(
            &json!({
                "schema_version": 4,
                "inputs": ["main"],
                "nodes": [
                    {"id": "n", "op": op, "in": ["main"], "config": config},
                ],
                "output": "n",
            }),
            geo_contract(),
        );
        let plan = prepare(&graph, &RuntimeContext::default())
            .unwrap_or_else(|error| panic!("prepare di {op}: {error}"));
        assert_eq!(plan.segments().len(), 1, "{op}");
        let segment = &plan.segments()[0];
        assert_eq!(segment.mode, SegmentMode::Blocking, "{op}");
        assert_eq!(segment.parallelism, ParallelismStrategy::BlockingSingleTask, "{op}");
        assert_eq!(segment.kernels[0].geo_role, Some(role), "{op}");
    }
}

#[test]
fn fuzzy_join_prepares_as_binary_blocking() {
    let graph = validate(
        &json!({
            "schema_version": 4,
            "inputs": ["left_in", "right_in"],
            "nodes": [
                {"id": "j", "op": "table.fuzzy_join", "in": ["left_in", "right_in"],
                 "config": {
                    "left_key": "name", "right_key": "name",
                    "metric": "jaro_winkler", "threshold": 0.8,
                    "blocking": "prefix",
                 }},
            ],
            "output": "j",
        })
        .to_string(),
        &[
            ("left_in".to_owned(), table_contract()),
            ("right_in".to_owned(), table_contract()),
        ],
    )
    .expect("piano valido");
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    assert_eq!(plan.segments().len(), 1);
    let segment = &plan.segments()[0];
    assert_eq!(segment.mode, SegmentMode::BinaryBlocking);
    assert!(
        matches!(&segment.kernels[0].config, PreparedConfig::TableBinary(_)),
        "fuzzy_join usa il dispatch binario tabellare"
    );
    assert_eq!(segment.input_edges.as_ref(), &["left_in".to_owned(), "right_in".to_owned()]);
}

#[test]
fn top_n_prepares_as_blocking_table_unary() {
    let graph = validate_plan(
        &json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "t", "op": "table.top_n", "in": ["main"],
                 "config": {"columns": ["id"], "n": 2}},
            ],
            "output": "t",
        }),
        table_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");
    assert_eq!(plan.segments()[0].mode, SegmentMode::Blocking);
    assert!(
        matches!(&plan.segments()[0].kernels[0].config, PreparedConfig::TableUnary(_)),
        "top_n unaria blocking via execute_batch"
    );
}

#[cfg(feature = "proj-backend")]
#[test]
fn generative_geo_extensions_prepare_with_their_roles() {
    let from_wkt = validate_plan(
        &json!({
            "schema_version": 4,
            "crs": "EPSG:32632",
            "inputs": ["main"],
            "nodes": [
                {"id": "w", "op": "geo.from_wkt", "in": ["main"],
                 "config": {"wkt_column": "name"}},
            ],
            "output": "w",
        }),
        table_contract(),
    );
    let plan = prepare(&from_wkt, &RuntimeContext::default()).expect("prepare from_wkt");
    let kernel = &plan.segments()[0].kernels[0];
    assert_eq!(kernel.geo_role, Some(GeoRole::ProduceFromText));
    match &kernel.config {
        PreparedConfig::GeoFromWkt {
            wkt_column_index,
            on_error,
        } => {
            assert_eq!(*wkt_column_index, 1);
            assert_eq!(*on_error, plenora_kernels_geo::extensions::OnWktError::Null);
        }
        other => panic!("config inattesa: {other:?}"),
    }

    let grid = validate_plan(
        &json!({
            "schema_version": 4,
            "crs": "EPSG:32632",
            "inputs": ["main"],
            "nodes": [
                {"id": "g", "op": "geo.generate_grid", "in": ["main"],
                 "config": {
                    "extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 10.0, "ymax": 10.0},
                    "cell_size": 5.0,
                 }},
            ],
            "output": "g",
        }),
        table_contract(),
    );
    let plan = prepare(&grid, &RuntimeContext::default()).expect("prepare generate_grid");
    let segment = &plan.segments()[0];
    assert_eq!(segment.mode, SegmentMode::Blocking);
    assert_eq!(segment.kernels[0].geo_role, Some(GeoRole::WholeTable));
    assert!(
        matches!(&segment.kernels[0].config, PreparedConfig::GeoGenerateGrid { .. }),
        "config tipizzata della griglia"
    );
}
