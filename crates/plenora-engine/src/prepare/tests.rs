//! Test del preparer (Fase 2A-4, Architetture.md par. 6.3, ADR 5).

use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions, RuntimeStatistic,
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

/// Come in `planner::tests`: il marcatore `geoarrow.wkb` rende la colonna
/// identificabile dal check di analyze (ADR-0009, decisione 8).
fn wkb_geometry_field(name: &str) -> Field {
    Field::new(name, DataType::Binary, true).with_metadata(std::collections::HashMap::from([(
        plenora_kernels_geo::arrow_adapter::GEOARROW_EXTENSION_KEY.to_owned(),
        plenora_kernels_geo::arrow_adapter::GEOARROW_WKB_EXTENSION.to_owned(),
    )]))
}

fn geo_contract() -> DataContract {
    DataContract::new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            wkb_geometry_field("geom"),
        ])),
        vec![GeometryColumnContract {
            field_id: FieldId(3),
            name: "geom".to_owned(),
            crs: ContractCrs::Resolved(projected_crs()),
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
            "schema_version": 5,
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
    assert_eq!(
        blocking.parallelism,
        ParallelismStrategy::BlockingSingleTask
    );
    assert_eq!(blocking.kernels.len(), 1);
    assert_eq!(plan.output_edge(), "g");
    assert_eq!(
        plan.last_consumers().get("main"),
        Some(&LastConsumer::Node("f".into()))
    );
    assert_eq!(plan.last_consumers().get("g"), Some(&LastConsumer::Output));
}

#[test]
fn pure_geo_chain_is_geo_fused_mixed_chain_is_linear() {
    let geo_graph = validate_plan(
        &json!({
            "schema_version": 5,
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
    assert!(plan.segments()[0]
        .kernels
        .iter()
        .all(|kernel| kernel.geo_role == Some(GeoRole::TransformInPlace)));

    let mixed_graph = validate_plan(
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "r", "op": "table.rename", "in": ["main"],
                 "config": {"renames": [{"old_name": "id", "new_name": "id2"}]}},
                {"id": "b", "op": "geo.buffer", "in": ["r"], "config": {"distance": 10.0}},
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
            "schema_version": 5,
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
    assert_eq!(
        join_segment.input_edges.as_ref(),
        &["l".to_owned(), "r".to_owned()]
    );
    assert!(!plan
        .segments()
        .iter()
        .any(|segment| segment.materialize_output));

    // Fan-out: lo stesso arco alimenta due nodi -> materializzazione prevista.
    let fanout_graph = validate_plan(
        &json!({
            "schema_version": 5,
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
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [],
            "output": "main",
        }),
        table_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");
    assert!(plan.segments().is_empty());
    assert_eq!(plan.output_edge(), "main");
    assert_eq!(
        plan.last_consumers().get("main"),
        Some(&LastConsumer::Output)
    );
}

#[test]
fn unknown_statistics_are_the_conservative_default() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
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
    assert_eq!(
        plan.segments()[0].parallelism,
        ParallelismStrategy::SerialFused
    );
}

#[test]
fn unsupported_geo_op_fails_at_prepare_not_mid_stream() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
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
            "schema_version": 5,
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
    let roles: Vec<Option<GeoRole>> = segment
        .kernels
        .iter()
        .map(|kernel| kernel.geo_role)
        .collect();
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
        PreparedConfig::GeoSnap { tolerance, .. } => {
            assert!((*tolerance - 0.5).abs() < f64::EPSILON);
        }
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
            "schema_version": 5,
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
        PreparedConfig::GeoLineLocatePoint {
            point,
            output_column,
        } => {
            assert_eq!((point.x(), point.y()), (0.0, 0.0));
            assert_eq!(output_column, "fraction");
        }
        other => panic!("config inattesa: {other:?}"),
    }
}

#[test]
fn blocking_geo_extensions_are_single_blocking_segments() {
    let cases = [
        (
            "geo.collect",
            json!({"group_by": ["id"]}),
            GeoRole::WholeTable,
        ),
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
                "schema_version": 5,
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
        assert_eq!(
            segment.parallelism,
            ParallelismStrategy::BlockingSingleTask,
            "{op}"
        );
        assert_eq!(segment.kernels[0].geo_role, Some(role), "{op}");
    }
}

#[test]
fn fuzzy_join_prepares_as_binary_blocking() {
    let graph = validate(
        &json!({
            "schema_version": 5,
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
    assert_eq!(
        segment.input_edges.as_ref(),
        &["left_in".to_owned(), "right_in".to_owned()]
    );
}

#[test]
fn top_n_prepares_as_blocking_table_unary() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
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
        matches!(
            &plan.segments()[0].kernels[0].config,
            PreparedConfig::TableUnary(_)
        ),
        "top_n unaria blocking via execute_batch"
    );
}

#[cfg(feature = "proj-backend")]
#[test]
fn generative_geo_extensions_prepare_with_their_roles() {
    let from_wkt = validate_plan(
        &json!({
            "schema_version": 5,
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
            "schema_version": 5,
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
        matches!(
            &segment.kernels[0].config,
            PreparedConfig::GeoGenerateGrid { .. }
        ),
        "config tipizzata della griglia"
    );
}

// ---------------------------------------------------------------------------
// Gruppi di fusione geo (ADR-0012)
// ---------------------------------------------------------------------------

/// Catena buffer -> simplify -> centroid: tre kernel fondibili consecutivi.
fn fusible_chain_plan() -> serde_json::Value {
    json!({
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 10.0}},
            {"id": "s", "op": "geo.simplify", "in": ["b"], "config": {"tolerance": 0.1}},
            {"id": "c", "op": "geo.centroid", "in": ["s"], "config": {}},
        ],
        "output": "c",
    })
}

fn fusion_groups(plan: &ExecutionPlan) -> Vec<Option<u32>> {
    plan.segments()[0]
        .kernels
        .iter()
        .map(|kernel| kernel.fusion_group)
        .collect()
}

#[test]
fn fusible_geo_runs_form_one_fusion_group() {
    let graph = validate_plan(&fusible_chain_plan(), geo_contract());
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    // Kill switch registrato nel piano (D12.9) e capability risolta per
    // kernel come `cancellation_behavior` (D12.2).
    assert!(plan.geo_fusion());
    assert_eq!(plan.segments().len(), 1);
    for kernel in &plan.segments()[0].kernels {
        assert_eq!(
            kernel.geo_fusion,
            plenora_core::catalog::GeoFusion::TransformInPlace
        );
    }
    // Tre kernel fondibili consecutivi -> UN gruppo, stesso id sui membri.
    assert_eq!(fusion_groups(&plan), vec![Some(0), Some(0), Some(0)]);
}

#[test]
fn non_fusible_kernel_breaks_the_fusion_run() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 10.0}},
                {"id": "ls", "op": "geo.line_substring", "in": ["b"],
                 "config": {"start_ratio": 0.0, "end_ratio": 0.5}},
                {"id": "s", "op": "geo.simplify", "in": ["ls"], "config": {"tolerance": 0.1}},
                {"id": "t", "op": "geo.translate", "in": ["s"],
                 "config": {"x_offset": 1.0, "y_offset": 2.0}},
            ],
            "output": "t",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    // Un kernel non fondibile (`geo.line_substring`, NotFusible) spezza il
    // run: buffer resta solo (run < 2, nessun gruppo), simplify+translate
    // formano un gruppo a valle.
    assert_eq!(fusion_groups(&plan), vec![None, None, Some(0), Some(0)]);
}

#[test]
fn transforms_and_terminal_measure_form_one_fusion_group() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 10.0}},
                {"id": "s", "op": "geo.simplify", "in": ["b"], "config": {"tolerance": 0.1}},
                {"id": "a", "op": "geo.area", "in": ["s"], "config": {}},
            ],
            "output": "a",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    // M2: la misura terminale (`geo.area`, capability TerminalMeasure)
    // chiude il run di transform ed entra nel gruppo come ultimo membro.
    let kernels = &plan.segments()[0].kernels;
    assert_eq!(
        kernels[2].geo_fusion,
        plenora_core::catalog::GeoFusion::TerminalMeasure
    );
    assert!(matches!(
        kernels[2].config,
        PreparedConfig::GeoMeasure { .. }
    ));
    assert_eq!(fusion_groups(&plan), vec![Some(0), Some(0), Some(0)]);
}

#[test]
fn single_transform_plus_terminal_measure_forms_a_group_of_two() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "t", "op": "geo.translate", "in": ["main"],
                 "config": {"x_offset": 1.0, "y_offset": 2.0}},
                {"id": "w", "op": "geo.to_wkt", "in": ["t"], "config": {}},
            ],
            "output": "w",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    // M2: con la misura in coda basta UN transform (gruppo di due nodi).
    assert_eq!(fusion_groups(&plan), vec![Some(0), Some(0)]);
}

#[test]
fn lone_terminal_measure_forms_no_group() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "a", "op": "geo.area", "in": ["main"], "config": {}},
            ],
            "output": "a",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    // Una misura da sola non forma mai gruppo: non c'e' nulla da fondere,
    // resta sul percorso nodo-per-nodo.
    assert_eq!(fusion_groups(&plan), vec![None]);
}

#[test]
fn terminal_measure_closes_but_does_not_extend_the_run() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "t", "op": "geo.translate", "in": ["main"],
                 "config": {"x_offset": 1.0, "y_offset": 2.0}},
                {"id": "a", "op": "geo.area", "in": ["t"], "config": {}},
                {"id": "vc", "op": "geo.vertex_count", "in": ["a"], "config": {}},
                {"id": "s", "op": "geo.simplify", "in": ["vc"], "config": {"tolerance": 0.1}},
                {"id": "r", "op": "geo.rotate", "in": ["s"], "config": {"degrees": 10.0}},
            ],
            "output": "r",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    // UNA sola misura per gruppo e solo in coda: la seconda misura non
    // entra nel gruppo di translate+area; simplify+rotate formano un nuovo
    // gruppo a valle.
    assert_eq!(
        fusion_groups(&plan),
        vec![Some(0), Some(0), None, Some(1), Some(1)]
    );
}

#[test]
fn geo_fusion_kill_switch_disables_groups() {
    let graph = validate_plan(&fusible_chain_plan(), geo_contract());
    let runtime = RuntimeContext {
        geo_fusion: false,
        ..RuntimeContext::default()
    };
    let plan = prepare(&graph, &runtime).expect("prepare");

    assert!(
        !plan.geo_fusion(),
        "kill switch spento registrato nel piano"
    );
    assert_eq!(fusion_groups(&plan), vec![None, None, None]);
}

/// M3: `make_valid` (capability `TransformInPlace`) forma gruppi come le
/// altre op del perimetro. La validazione fail-closed (capability `geos`)
/// rende irraggiungibile questo piano a feature spenta, quindi il caso
/// "nessun gruppo senza backend" non ha bisogno di un gate in `prepare`.
#[cfg(feature = "geos-backend")]
#[test]
fn make_valid_joins_fusion_groups() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "t", "op": "geo.translate", "in": ["main"],
                 "config": {"x_offset": 1.0, "y_offset": 2.0}},
                {"id": "m", "op": "geo.make_valid", "in": ["t"], "config": {}},
                {"id": "r", "op": "geo.rotate", "in": ["m"], "config": {"degrees": 10.0}},
            ],
            "output": "r",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    let kernels = &plan.segments()[0].kernels;
    assert_eq!(
        kernels[1].geo_fusion,
        plenora_core::catalog::GeoFusion::TransformInPlace
    );
    assert_eq!(fusion_groups(&plan), vec![Some(0), Some(0), Some(0)]);
}

/// M3: `reproject` forma gruppi e puo' stare in qualunque posizione del run
/// (qui in testa, con cambio di CRS a meta' catena gestito dal runner). Il
/// target e' EPSG:3857 (proiettato): i transform a valle richiedono un CRS
/// proiettato (`CrsRequirement::Projected`).
#[cfg(feature = "proj-backend")]
#[test]
fn reproject_joins_fusion_groups() {
    let graph = validate_plan(
        &json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [
                {"id": "p", "op": "geo.reproject", "in": ["main"],
                 "config": {"target_crs": "EPSG:3857"}},
                {"id": "t", "op": "geo.translate", "in": ["p"],
                 "config": {"x_offset": 1000.0, "y_offset": 1000.0}},
            ],
            "output": "t",
        }),
        geo_contract(),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");

    let kernels = &plan.segments()[0].kernels;
    assert_eq!(
        kernels[0].geo_fusion,
        plenora_core::catalog::GeoFusion::TransformInPlace
    );
    assert_eq!(fusion_groups(&plan), vec![Some(0), Some(0)]);
}

// ---------------------------------------------------------------------------
// Dispatch fail-closed e rivalidazione fisica delle estensioni geo (E1)
// ---------------------------------------------------------------------------
//
// La fase 1 (`planner::validate` via `analyze`) pre-valida le config; le
// rivalidazioni di `prepare` sono difesa in profondita': qui sono esercitate
// chiamando direttamente le funzioni interne con contratti da fixture, per
// verificare che il secondo livello resti fail-closed anche se il primo si
// allenta. I comportamenti verificati sono quelli del perimetro documentato
// (dispatch v1, limiti del kernel, coerenza dei contratti).

/// POINT (2 3), little-endian OGC WKB, in esadecimale (convenzione D16).
const POINT_HEX: &str = "010100000000000000000000400000000000000840";

/// LINESTRING (0 0, 1 1), little-endian OGC WKB, in esadecimale.
const LINESTRING_HEX: &str =
    "01020000000200000000000000000000000000000000000000000000000000f03f000000000000f03f";

fn geo_node(op: &str, config: serde_json::Value) -> NodeV5 {
    NodeV5 {
        id: "n".to_owned(),
        op: op.to_owned(),
        inputs: vec!["main".to_owned()],
        config,
    }
}

fn descriptor_of(op: &str) -> &'static plenora_core::catalog::OperationDescriptor {
    plenora_core::catalog::find_operation(op).expect("op in catalogo")
}

#[test]
fn nary_concat_over_two_inputs_is_rejected_fail_closed() {
    // `table.concat` e' NAry: il planner accetta piu' di due input, ma
    // l'executor v1 ne supporta solo due — il rifiuto arriva in `prepare`
    // (fail-closed a secco), mai a meta' esecuzione.
    let graph = validate(
        &json!({
            "schema_version": 5,
            "inputs": ["a", "b", "c"],
            "nodes": [
                {"id": "cat", "op": "table.concat", "in": ["a", "b", "c"], "config": {}}
            ],
            "output": "cat",
        })
        .to_string(),
        &[
            ("a".to_owned(), table_contract()),
            ("b".to_owned(), table_contract()),
            ("c".to_owned(), table_contract()),
        ],
    )
    .expect("il planner accetta concat N-aria");
    let result = prepare(&graph, &RuntimeContext::default());
    match result {
        Err(PlenoraError::Unsupported(message)) => {
            assert!(message.contains("N-aria"), "{message}");
            assert!(message.contains("3 input"), "{message}");
        }
        other => panic!("atteso Unsupported, ottenuto {other:?}"),
    }
}

#[test]
fn from_wkt_extension_resolves_column_index_and_error_policy() {
    let input = DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("wkt", DataType::Utf8, true),
    ])));
    // Default `on_error: null`; indice risolto qui, mai nel loop per batch.
    let node = geo_node("geo.from_wkt", json!({"wkt_column": "wkt"}));
    let (config, role) =
        prepare_geo_extension(&node, descriptor_of("geo.from_wkt"), &input, &input)
            .expect("prepare")
            .expect("estensione coperta");
    match config {
        PreparedConfig::GeoFromWkt {
            wkt_column_index,
            on_error,
        } => {
            assert_eq!(wkt_column_index, 1);
            assert_eq!(on_error, OnWktError::Null);
        }
        other => panic!("config inattesa: {other:?}"),
    }
    assert_eq!(role, GeoRole::ProduceFromText);

    // Politica esplicita `fail`.
    let node = geo_node(
        "geo.from_wkt",
        json!({"wkt_column": "wkt", "on_error": "fail"}),
    );
    let (config, _) = prepare_geo_extension(&node, descriptor_of("geo.from_wkt"), &input, &input)
        .expect("prepare")
        .expect("estensione coperta");
    match config {
        PreparedConfig::GeoFromWkt { on_error, .. } => assert_eq!(on_error, OnWktError::Fail),
        other => panic!("config inattesa: {other:?}"),
    }

    // Colonna WKT assente dal contratto di input: errore Schema, mai indice
    // inventato (difesa in profondita': analyze la pre-valida).
    let node = geo_node("geo.from_wkt", json!({"wkt_column": "assente"}));
    let result = prepare_geo_extension(&node, descriptor_of("geo.from_wkt"), &input, &input);
    match result {
        Err(PlenoraError::Schema(message)) => {
            assert!(message.contains("assente"), "{message}");
        }
        other => panic!("atteso Schema, ottenuto {other:?}"),
    }
}

#[test]
fn generate_grid_extension_revalidates_extent_and_cell_size() {
    let trigger = table_contract();
    let node = geo_node(
        "geo.generate_grid",
        json!({
            "extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 10.0, "ymax": 10.0},
            "cell_size": 2.5
        }),
    );
    let (config, role) = prepare_geo_extension(
        &node,
        descriptor_of("geo.generate_grid"),
        &trigger,
        &trigger,
    )
    .expect("prepare")
    .expect("estensione coperta");
    match config {
        PreparedConfig::GeoGenerateGrid {
            extent,
            cell_size,
            shape,
        } => {
            assert_eq!(
                extent,
                GridExtent::new(0.0, 0.0, 10.0, 10.0).expect("extent")
            );
            assert!((cell_size - 2.5).abs() < f64::EPSILON);
            assert_eq!(shape, GridShape::Square, "shape di default");
        }
        other => panic!("config inattesa: {other:?}"),
    }
    assert_eq!(role, GeoRole::WholeTable);

    // Extent degenere e cell_size nulla: rifiutati alla rivalidazione fisica
    // (analyze li pre-valida; il secondo livello resta fail-closed).
    let degenerate = geo_node(
        "geo.generate_grid",
        json!({
            "extent": {"xmin": 1.0, "ymin": 0.0, "xmax": 1.0, "ymax": 5.0},
            "cell_size": 1.0
        }),
    );
    assert!(matches!(
        prepare_geo_extension(
            &degenerate,
            descriptor_of("geo.generate_grid"),
            &trigger,
            &trigger
        ),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let zero_cell = geo_node(
        "geo.generate_grid",
        json!({
            "extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 10.0, "ymax": 10.0},
            "cell_size": 0.0
        }),
    );
    assert!(matches!(
        prepare_geo_extension(
            &zero_cell,
            descriptor_of("geo.generate_grid"),
            &trigger,
            &trigger
        ),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn wkb_hex_operands_are_decoded_once_and_fail_closed() {
    // Decodifica una tantum in `prepare` (E1): hex valido -> geometria.
    let point = decode_wkb_hex("n", "point_wkb", POINT_HEX).expect("point valido");
    match point {
        Geometry::Point(point) => {
            assert!((point.x() - 2.0).abs() < f64::EPSILON);
            assert!((point.y() - 3.0).abs() < f64::EPSILON);
        }
        other => panic!("atteso Point, ottenuto {other:?}"),
    }
    // Lunghezza dispari, vuoto, caratteri non hex, WKB non decodificabile:
    // errore esplicito, mai bytes inventati.
    for bad in ["", "0", "zz", "0102"] {
        assert!(
            decode_wkb_hex("n", "point_wkb", bad).is_err(),
            "input malformato accettato: {bad:?}"
        );
    }
    // `line_locate_point`: una geometria che non e' un Point e' rifiutata.
    let node = geo_node(
        "geo.line_locate_point",
        json!({"point_wkb": LINESTRING_HEX}),
    );
    let result = prepare_geo_extension(
        &node,
        descriptor_of("geo.line_locate_point"),
        &geo_contract(),
        &geo_contract(),
    );
    match result {
        Err(PlenoraError::InvalidPlan(message)) => {
            assert!(message.contains("Point"), "{message}");
        }
        other => panic!("atteso InvalidPlan, ottenuto {other:?}"),
    }
}

#[test]
fn extension_revalidations_stay_fail_closed() {
    let geo = geo_contract();
    // `subdivide`: sotto il minimo di vertici di un anello chiuso.
    let node = geo_node("geo.subdivide", json!({"max_vertices": 3}));
    assert!(matches!(
        prepare_geo_extension(&node, descriptor_of("geo.subdivide"), &geo, &geo),
        Err(PlenoraError::InvalidPlan(_))
    ));
    // `snap`: tolleranza negativa.
    let node = geo_node(
        "geo.snap",
        json!({"reference_wkb": POINT_HEX, "tolerance": -1.0}),
    );
    assert!(matches!(
        prepare_geo_extension(&node, descriptor_of("geo.snap"), &geo, &geo),
        Err(PlenoraError::InvalidPlan(_))
    ));
    // `collect`: colonna chiave assente dal contratto di input.
    let node = geo_node("geo.collect", json!({"group_by": ["assente"]}));
    assert!(matches!(
        prepare_geo_extension(&node, descriptor_of("geo.collect"), &geo, &geo),
        Err(PlenoraError::Schema(_))
    ));
    // `coverage_validate`: tolleranza negativa.
    let node = geo_node("geo.coverage_validate", json!({"tolerance": -1.0}));
    assert!(matches!(
        prepare_geo_extension(&node, descriptor_of("geo.coverage_validate"), &geo, &geo),
        Err(PlenoraError::InvalidPlan(_))
    ));
    // `shared_paths`: min_length negativa.
    let node = geo_node("geo.shared_paths", json!({"min_length": -1.0}));
    assert!(matches!(
        prepare_geo_extension(&node, descriptor_of("geo.shared_paths"), &geo, &geo),
        Err(PlenoraError::InvalidPlan(_))
    ));
    // `cluster_dbscan`: eps nullo e min_points nullo.
    let node = geo_node("geo.cluster_dbscan", json!({"eps": 0.0, "min_points": 1}));
    assert!(matches!(
        prepare_geo_extension(&node, descriptor_of("geo.cluster_dbscan"), &geo, &geo),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let node = geo_node("geo.cluster_dbscan", json!({"eps": 1.0, "min_points": 0}));
    assert!(matches!(
        prepare_geo_extension(&node, descriptor_of("geo.cluster_dbscan"), &geo, &geo),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn accessors_extension_revalidates_selection_and_output_contract() {
    let geo = geo_contract();
    // Accessorio sconosciuto: rifiuto esplicito (analyze lo pre-valida con
    // un enum chiuso; il secondo livello non si fida).
    let node = geo_node("geo.geometry_accessors", json!({"fields": ["bogus"]}));
    match prepare_geo_extension(&node, descriptor_of("geo.geometry_accessors"), &geo, &geo) {
        Err(PlenoraError::InvalidPlan(message)) => {
            assert!(message.contains("bogus"), "{message}");
        }
        other => panic!("atteso InvalidPlan, ottenuto {other:?}"),
    }
    // Il contratto di output non contiene la colonna accessoria inferita:
    // incoerenza segnalata, mai accesso per nome a runtime.
    let node = geo_node(
        "geo.geometry_accessors",
        json!({"fields": ["geometry_type"]}),
    );
    match prepare_geo_extension(&node, descriptor_of("geo.geometry_accessors"), &geo, &geo) {
        Err(PlenoraError::Schema(message)) => {
            assert!(message.contains("geometry_type"), "{message}");
        }
        other => panic!("atteso Schema, ottenuto {other:?}"),
    }
}

#[test]
fn measure_output_column_requires_exactly_one_added_column() {
    let input = table_contract();
    // Nessuna colonna aggiunta: il contratto di output non puo' essere
    // quello di una misura.
    match measure_output_column("n", &input, &input, None) {
        Err(PlenoraError::Schema(message)) => {
            assert!(message.contains("trovate 0"), "{message}");
        }
        other => panic!("atteso Schema, ottenuto {other:?}"),
    }
    // Piu' di una colonna aggiunta: ambiguita' rifiutata.
    let wider = DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("a", DataType::Float64, true),
        Field::new("b", DataType::Float64, true),
    ])));
    match measure_output_column("n", &input, &wider, None) {
        Err(PlenoraError::Schema(message)) => {
            assert!(message.contains("trovate 2"), "{message}");
        }
        other => panic!("atteso Schema, ottenuto {other:?}"),
    }
    // `output_column` dichiarata diversa dalla colonna inferita: la
    // divergenza e' un errore, la fonte unica resta il contratto.
    let one = DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("area", DataType::Float64, true),
    ])));
    match measure_output_column("n", &input, &one, Some("length")) {
        Err(PlenoraError::Schema(message)) => {
            assert!(message.contains("diversa"), "{message}");
        }
        other => panic!("atteso Schema, ottenuto {other:?}"),
    }
    assert_eq!(
        measure_output_column("n", &input, &one, Some("area")).expect("coerente"),
        "area"
    );
    assert_eq!(
        measure_output_column("n", &input, &one, None).expect("inferita"),
        "area"
    );
}

// ---------------------------------------------------------------------------
// Binari geo nel piano (ADR-0014 M1)
// ---------------------------------------------------------------------------

/// Piano a due input geo con un nodo binario come output del piano.
fn geo_binary_graph(op: &str, config: &serde_json::Value) -> ValidatedGraph {
    geo_binary_graph_with_limits(op, config, &json!({}))
}

/// Come [`geo_binary_graph`], con limiti di piano espliciti (D14.6).
fn geo_binary_graph_with_limits(
    op: &str,
    config: &serde_json::Value,
    limits: &serde_json::Value,
) -> ValidatedGraph {
    validate(
        &json!({
            "schema_version": 5,
            "limits": limits,
            "inputs": ["left_in", "right_in"],
            "nodes": [
                {"id": "j", "op": op, "in": ["left_in", "right_in"], "config": config},
            ],
            "output": "j",
        })
        .to_string(),
        &[
            ("left_in".to_owned(), geo_contract()),
            ("right_in".to_owned(), geo_contract()),
        ],
    )
    .expect("piano valido")
}

/// Config `GeoBinary` del primo kernel del segmento (panico altrimenti).
fn geo_binary_config(segment: &PhysicalSegment) -> &GeoBinaryPlan {
    let PreparedConfig::GeoBinary(geo_plan) = &segment.kernels[0].config else {
        panic!("atteso GeoBinary, ottenuto {:?}", segment.kernels[0].config);
    };
    geo_plan
}

#[test]
fn geo_binary_m1_ops_prepare_as_geo_binary_with_resolved_plan() {
    let cases: [(&str, serde_json::Value, PairOperation); 4] = [
        (
            "geo.sjoin",
            json!({"predicate": "intersects"}),
            PairOperation::SJoin,
        ),
        (
            "geo.nearest",
            json!({"max_distance": 1.5}),
            PairOperation::Nearest,
        ),
        ("geo.within", json!({}), PairOperation::Within),
        (
            "geo.count_points_in_polygons",
            json!({}),
            PairOperation::CountPointsInPolygons,
        ),
    ];
    let defaults = Limits::default();
    let ceiling = defaults
        .rows
        .max_input_rows
        .max(defaults.rows.max_rows_per_edge);
    for (op, config, expected) in cases {
        let graph = geo_binary_graph(op, &config);
        let plan = prepare(&graph, &RuntimeContext::default())
            .unwrap_or_else(|error| panic!("prepare di {op}: {error}"));
        assert_eq!(plan.segments().len(), 1, "{op}");
        let segment = &plan.segments()[0];
        assert_eq!(segment.mode, SegmentMode::BinaryBlocking, "{op}");
        assert_eq!(
            segment.input_edges.as_ref(),
            &["left_in".to_owned(), "right_in".to_owned()],
            "{op}"
        );
        let kernel = &segment.kernels[0];
        assert_eq!(kernel.geo_role, Some(GeoRole::BinaryBlocking), "{op}");
        let geo_plan = geo_binary_config(segment);
        assert_eq!(geo_plan.operation, expected, "{op}");
        // Indici risolti sui due contratti fixture (`geom` e' la seconda colonna).
        assert_eq!(geo_plan.left_geometry_index, 1, "{op}");
        assert_eq!(geo_plan.right_geometry_index, 1, "{op}");
        assert_eq!(geo_plan.output_crs, "EPSG:32632", "{op}");
        // D14.6: il nodo e' l'output del piano -> tetto = max_output_rows.
        assert_eq!(geo_plan.max_pairs, defaults.rows.max_output_rows, "{op}");
        assert_eq!(geo_plan.max_results, defaults.rows.max_output_rows, "{op}");
        assert_eq!(geo_plan.max_comparisons, ceiling * ceiling, "{op}");
    }
    // Parametri tipizzati rivalidati: predicato del sjoin e max_distance del
    // nearest arrivano al piano come valori tipizzati (E1, niente JSON a runtime).
    let graph = geo_binary_graph("geo.sjoin", &json!({"predicate": "within"}));
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");
    assert_eq!(
        geo_binary_config(&plan.segments()[0]).predicate,
        Some(JoinPredicate::Within)
    );
    let graph = geo_binary_graph("geo.nearest", &json!({"max_distance": 2.5}));
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");
    assert_eq!(
        geo_binary_config(&plan.segments()[0]).max_distance,
        Some(2.5)
    );
}

#[test]
fn geo_binary_caps_follow_edge_position_and_plan_limits() {
    // Nodo terminale: tetto = max_output_rows del piano (override esplicito).
    let graph = geo_binary_graph_with_limits(
        "geo.sjoin",
        &json!({"predicate": "intersects"}),
        &json!({"max_output_rows": 42}),
    );
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");
    assert_eq!(geo_binary_config(&plan.segments()[0]).max_pairs, 42);

    // Nodo intermedio (a valle un filter): tetto = max_rows_per_edge.
    let graph = validate(
        &json!({
            "schema_version": 5,
            "limits": {"max_rows_per_edge": 77},
            "inputs": ["left_in", "right_in"],
            "nodes": [
                {"id": "j", "op": "geo.sjoin", "in": ["left_in", "right_in"],
                 "config": {"predicate": "intersects"}},
                {"id": "f", "op": "table.filter", "in": ["j"],
                 "config": {"column": "id", "operator": ">", "value": 0}},
            ],
            "output": "f",
        })
        .to_string(),
        &[
            ("left_in".to_owned(), geo_contract()),
            ("right_in".to_owned(), geo_contract()),
        ],
    )
    .expect("piano valido");
    let plan = prepare(&graph, &RuntimeContext::default()).expect("prepare");
    let segment = plan
        .segments()
        .iter()
        .find(|segment| segment.output_edge == "j")
        .expect("segmento del sjoin");
    let geo_plan = geo_binary_config(segment);
    assert_eq!(geo_plan.max_pairs, 77);
    // Il tetto confronti resta il quadrato del tetto d'ingresso (default 10M).
    assert_eq!(geo_plan.max_comparisons, 10_000_000_u64 * 10_000_000);
}

#[test]
fn limiti_fuori_dominio_sono_rifiutati_prima_del_prepare() {
    // `max_output_rows = 0` descrive un piano che non puo' emettere nulla.
    // Prima veniva accettato in validazione e intercettato solo in `prepare`,
    // dalla rivalidazione fisica dei parametri della coppia (`max_pairs > 0`),
    // con un messaggio che parlava del kernel invece che del limite.
    //
    // Ora `Limits::validate` lo rifiuta all'ingresso del planner, per TUTTI i
    // piani — geo compresi, che il preparer tabellare non attraversano. La
    // rivalidazione in `prepare` resta come difesa in profondita': non e' piu'
    // raggiungibile da un piano, ed e' il verso giusto.
    let error = validate(
        &json!({
            "schema_version": 5,
            "limits": {"max_output_rows": 0},
            "inputs": ["left_in", "right_in"],
            "nodes": [
                {"id": "j", "op": "geo.sjoin", "in": ["left_in", "right_in"],
                 "config": {"predicate": "intersects"}},
            ],
            "output": "j",
        })
        .to_string(),
        &[
            ("left_in".to_owned(), geo_contract()),
            ("right_in".to_owned(), geo_contract()),
        ],
    )
    .expect_err("limite fuori dominio");
    let PlenoraError::InvalidPlan(message) = &error else {
        panic!("atteso InvalidPlan, ottenuto {error:?}");
    };
    assert!(message.contains("max_output_rows"), "{message}");
}

#[test]
fn geo_binary_outside_m1_perimeter_stays_unsupported() {
    // Secondo cantiere D14.1 (ri-encode): il rifiuto e' invariato.
    let cases: [(&str, serde_json::Value); 6] = [
        ("geo.clip", json!({})),
        ("geo.overlay", json!({"mode": "union"})),
        ("geo.intersection", json!({})),
        ("geo.union", json!({})),
        ("geo.difference", json!({})),
        ("geo.symmetric_difference", json!({})),
    ];
    for (op, config) in cases {
        let graph = geo_binary_graph(op, &config);
        match prepare(&graph, &RuntimeContext::default()) {
            Err(PlenoraError::Unsupported(message)) => {
                assert!(
                    message.contains("non e' nel dispatch v1"),
                    "{op}: {message}"
                );
            }
            other => panic!("{op}: atteso Unsupported, ottenuto {other:?}"),
        }
    }
}
