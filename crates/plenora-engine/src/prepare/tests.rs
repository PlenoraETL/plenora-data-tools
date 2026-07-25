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
