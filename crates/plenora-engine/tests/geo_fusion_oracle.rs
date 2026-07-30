//! Oracolo differenziale della fusione dei segmenti geo, esteso agli errori
//! (ADR-0012, gate di M1): stesso input -> stesso output byte-per-byte O
//! stesso errore (variante, nodo, operazione, categoria, fase, effetto remoto,
//! retry e motivo — con `diagnostics` attivo, quindi con il contesto
//! strutturale `batch_seq` nel motivo).
//!
//! Ogni caso esegue la stessa pipeline due volte — `geo_fusion: true` (runner
//! fuso di `transform_cells_fused`) e `geo_fusion: false` (kill switch D12.9,
//! percorso nodo-per-nodo) — e confronta l'esito. Ogni piano e' prima
//! verificato con `explain`: i nodi attesi DEVONO formare un unico gruppo di
//! fusione a kill switch attivo e nessun gruppo a kill switch spento, senza
//! questa prova l'oracolo confronterebbe due volte il percorso non fuso.
//!
//! Caso (g) (panic iniettato) NON coperto qui: l'hook `PANIC_AT_NODES` di
//! `executor.rs` e' `#[cfg(test)]` e privato del crate, irraggiungibile da un
//! test di integrazione senza modificare il codice di produzione (fuori
//! perimetro — vedi report). Il test governor-oltre-soglia (DER-003, D12.8)
//! esiste gia' in `executor/tests.rs`
//! (`geo_fusion_falls_back_when_the_governor_rejects_the_reservation`) e non
//! e' duplicato.
//!
//! Casi M3 (`geo.reproject`/`geo.make_valid`, backend feature-gated): (m3-a)
//! input OGC-invalido -> `make_valid` in testa al gruppo, riparato senza
//! errori (trappola 1); (m3-b) catena con reproject e cambio di CRS;
//! (m3-c) `make_valid` a meta' catena; (m3-d) cancellazione con `make_valid`
//! `NonInterruptible` nel gruppo. I casi (m3-a/c/d) sono gated su
//! `geos-backend`, (m3-b) su `proj-backend`; il caso "feature spente" e'
//! senza gate e verifica l'esito identico nei due percorsi in OGNI
//! configurazione di build.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use geo::{line_string, polygon, Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{ArrayRef, BinaryArray, Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_core::{ErrorCategory, ErrorPhase, PlenoraError, RemoteEffect, RetryDisposition};
use plenora_engine::planner::{validate, ValidatedGraph};
use plenora_engine::{
    execute, explain, CancellationToken, ExecutionMetrics, Input, Inputs, Output, RuntimeContext,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Fixture (stessa forma dei test di executor: colonna `id` + WKB XY EPSG:32632)
// ---------------------------------------------------------------------------

fn geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        plenora_kernels_geo::arrow_adapter::geometry_output_field("geom", "EPSG:32632")
            .expect("campo geometria"),
    ]))
}

fn geo_contract() -> DataContract {
    DataContract::new(
        geo_schema(),
        vec![GeometryColumnContract {
            field_id: FieldId(3),
            name: "geom".to_owned(),
            crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                "EPSG:32632".to_owned(),
                json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
                CrsKind::Projected,
                Some(1.0),
            )),
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

fn to_wkb(geometry: &Geometry<f64>) -> Vec<u8> {
    geometry.to_wkb(CoordDimensions::xy()).expect("wkb fixture")
}

fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    to_wkb(&Geometry::Point(Point::new(x, y)))
}

fn square_wkb(origin_x: f64, origin_y: f64, side: f64) -> Vec<u8> {
    to_wkb(&Geometry::Polygon(polygon![
        (x: origin_x, y: origin_y),
        (x: origin_x + side, y: origin_y),
        (x: origin_x + side, y: origin_y + side),
        (x: origin_x, y: origin_y + side),
        (x: origin_x, y: origin_y),
    ]))
}

fn geo_batch(ids: &[i64], cells: &[Option<Vec<u8>>]) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    RecordBatch::try_new(
        geo_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geo fixture valido")
}

// ---------------------------------------------------------------------------
// Infrastruttura dell'oracolo
// ---------------------------------------------------------------------------

fn graph(plan: &Value) -> ValidatedGraph {
    validate(&plan.to_string(), &[("main".to_owned(), geo_contract())]).expect("piano valido")
}

fn runtime(geo_fusion: bool) -> RuntimeContext {
    RuntimeContext {
        geo_fusion,
        diagnostics: true,
        ..RuntimeContext::default()
    }
}

fn single_input(batches: Vec<RecordBatch>) -> Inputs {
    Inputs::new()
        .with("main", Input::from_batches(batches).expect("input non vuoto"))
        .expect("input unico")
}

/// Prova che il piano forma DAVVERO il gruppo atteso (vincolo dell'oracolo):
/// con `geo_fusion` attivo i nodi dati condividono un unico `fusion_group`;
/// con il kill switch spento nessun gruppo e' annotato (D12.9, D12.2).
fn assert_group_formation(plan: &Value, nodes: &[&str]) {
    let validated = graph(plan);
    for geo_fusion in [true, false] {
        let execution = explain(&validated, &runtime(geo_fusion)).expect("explain");
        let groups: BTreeMap<&str, Option<u32>> = execution
            .segments()
            .iter()
            .flat_map(|segment| {
                segment
                    .kernels
                    .iter()
                    .map(|kernel| (kernel.node_id.as_str(), kernel.fusion_group))
            })
            .collect();
        if geo_fusion {
            let ids: BTreeSet<u32> = nodes
                .iter()
                .map(|node| {
                    groups[node]
                        .unwrap_or_else(|| panic!("{node}: gruppo di fusione mancante nel piano"))
                })
                .collect();
            assert_eq!(ids.len(), 1, "{nodes:?}: atteso UN solo gruppo di fusione");
        } else {
            for node in nodes {
                assert_eq!(groups[node], None, "{node}: nessun gruppo a kill switch spento");
            }
        }
    }
}

/// Forma dell'errore confrontata dall'oracolo (ADR-0012): variante, nodo,
/// operazione, motivo (con il suffisso `batch_seq` della diagnostica),
/// categoria, fase, effetto remoto e retry. `execution_id` e' escluso PER
/// COSTRUZIONE: e' un UUID nuovo a ogni esecuzione, non un osservabile
/// confrontabile tra due run.
#[derive(Debug, PartialEq, Eq)]
struct ErrorSignature {
    variant: &'static str,
    node: Option<String>,
    operation: Option<String>,
    reason: String,
    category: ErrorCategory,
    phase: ErrorPhase,
    remote_effect: RemoteEffect,
    retry: RetryDisposition,
}

fn error_signature(error: &PlenoraError) -> ErrorSignature {
    let (variant, node, operation, reason) = match error {
        PlenoraError::InvalidPlan(reason)
        | PlenoraError::Unsupported(reason)
        | PlenoraError::Schema(reason)
        | PlenoraError::DataMapping(reason)
        | PlenoraError::Crs(reason)
        | PlenoraError::Internal(reason) => {
            (variant_name(error), None, None, reason.clone())
        }
        PlenoraError::Execution {
            node,
            operation,
            reason,
            ..
        }
        | PlenoraError::Cancelled {
            node,
            operation,
            reason,
            ..
        } => (
            variant_name(error),
            Some(node.clone()),
            Some(operation.clone()),
            reason.clone(),
        ),
        PlenoraError::Io(error) => ("Io", None, None, error.to_string()),
    };
    ErrorSignature {
        variant,
        node,
        operation,
        reason,
        category: error.category(),
        phase: error.phase(),
        remote_effect: error.remote_effect(),
        retry: error.retry_disposition(),
    }
}

const fn variant_name(error: &PlenoraError) -> &'static str {
    match error {
        PlenoraError::InvalidPlan(_) => "InvalidPlan",
        PlenoraError::Unsupported(_) => "Unsupported",
        PlenoraError::Schema(_) => "Schema",
        PlenoraError::DataMapping(_) => "DataMapping",
        PlenoraError::Execution { .. } => "Execution",
        PlenoraError::Crs(_) => "Crs",
        PlenoraError::Cancelled { .. } => "Cancelled",
        PlenoraError::Io(_) => "Io",
        PlenoraError::Internal(_) => "Internal",
    }
}

/// Drena lo stream fino al primo errore (gli eventuali batch precedenti sono
/// gia' stati pubblicati nel canale, come nel consumo reale).
fn first_stream_error(output: &mut Output) -> Option<PlenoraError> {
    for item in output.by_ref() {
        if let Err(error) = item {
            return Some(error);
        }
    }
    None
}

/// Esegue il piano fino al primo errore dello stream; restituisce anche le
/// metriche parziali: `geo_fusion_fallbacks == 0` sul percorso fuso dimostra
/// che l'errore viene dal runner fuso e non da un fallback silenzioso (D12.7).
fn run_until_error(
    plan: &Value,
    batches: Vec<RecordBatch>,
    geo_fusion: bool,
) -> (PlenoraError, ExecutionMetrics) {
    let mut output =
        execute(&graph(plan), single_input(batches), runtime(geo_fusion)).expect("execute");
    let error = first_stream_error(&mut output)
        .expect("atteso un errore, lo stream e' terminato con successo");
    (error, output.metrics())
}

/// Oracolo sugli errori: stessa firma nei due percorsi, attribuzione al nodo
/// atteso (`None` per errori senza nodo, es. validazione dell'arco di
/// input), nessun fallback. Restituisce la firma per asserzioni ulteriori.
fn assert_oracle_error(
    case: &str,
    plan: &Value,
    fixture: &dyn Fn() -> Vec<RecordBatch>,
    expected_node: Option<&str>,
) -> ErrorSignature {
    let (fused_error, fused_metrics) = run_until_error(plan, fixture(), true);
    let (plain_error, plain_metrics) = run_until_error(plan, fixture(), false);
    assert_eq!(
        fused_metrics.geo_fusion_fallbacks, 0,
        "{case}: fallback nel percorso fuso — l'oracolo non starebbe confrontando il runner fuso"
    );
    assert_eq!(
        plain_metrics.geo_fusion_fallbacks, 0,
        "{case}: fallback inatteso nel percorso non fuso"
    );
    let signature = error_signature(&fused_error);
    assert_eq!(
        signature.node.as_deref(),
        expected_node,
        "{case}: attribuzione attesa nel percorso fuso"
    );
    assert_eq!(
        signature,
        error_signature(&plain_error),
        "{case}: errore diverso tra i percorsi\n  fuso:     {fused_error}\n  non fuso: {plain_error}"
    );
    signature
}

fn run_ok(plan: &Value, batches: Vec<RecordBatch>, geo_fusion: bool) -> (Vec<RecordBatch>, ExecutionMetrics) {
    execute(&graph(plan), single_input(batches), runtime(geo_fusion))
        .expect("execute")
        .collect_batches()
        .expect("stream ok")
}

// ---------------------------------------------------------------------------
// (a) Percorso felice multi-tipo
// ---------------------------------------------------------------------------

/// Sei op fondibili (`TransformInPlace`, perimetro M1): translate -> densify
/// -> rotate -> simplify -> scale -> envelope, un solo gruppo.
fn happy_path_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 3.0, "y_offset": -2.0}},
            {"id": "d", "op": "geo.densify", "in": ["t"],
             "config": {"max_segment_length": 7.5}},
            {"id": "r", "op": "geo.rotate", "in": ["d"], "config": {"degrees": 15.0}},
            {"id": "s", "op": "geo.simplify", "in": ["r"], "config": {"tolerance": 0.01}},
            {"id": "k", "op": "geo.scale", "in": ["s"],
             "config": {"x_factor": 1.5, "y_factor": 0.5}},
            {"id": "e", "op": "geo.envelope", "in": ["k"], "config": {}},
        ],
        "output": "e",
    })
}

fn holed_polygon_wkb() -> Vec<u8> {
    to_wkb(&Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (40.0, 0.0),
            (40.0, 40.0),
            (0.0, 40.0),
            (0.0, 0.0),
        ]),
        vec![LineString::from(vec![
            (10.0, 10.0),
            (20.0, 10.0),
            (20.0, 20.0),
            (10.0, 20.0),
            (10.0, 10.0),
        ])],
    )))
}

/// Punti, linestring, poligoni con e senza buchi, multi-*, null — due batch.
fn multi_type_batches() -> Vec<RecordBatch> {
    let linestring = to_wkb(&Geometry::LineString(line_string![
        (x: 0.0, y: 0.0),
        (x: 12.5, y: 3.5),
        (x: 25.0, y: 1.0),
        (x: 30.0, y: 20.0),
        (x: 44.0, y: 8.0),
    ]));
    let multi_point = to_wkb(&Geometry::MultiPoint(MultiPoint::from(vec![
        Point::new(1.0, 1.0),
        Point::new(5.0, 7.0),
        Point::new(-2.0, 3.0),
    ])));
    let multi_linestring = to_wkb(&Geometry::MultiLineString(MultiLineString::new(vec![
        LineString::from(vec![(0.0, 0.0), (9.0, 2.0), (4.0, 11.0)]),
        LineString::from(vec![(20.0, 20.0), (25.0, 30.0)]),
    ])));
    let multi_polygon = to_wkb(&Geometry::MultiPolygon(MultiPolygon(vec![
        Polygon::new(
            LineString::from(vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 5.0), (0.0, 0.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (10.0, 10.0),
                (17.0, 10.0),
                (17.0, 17.0),
                (10.0, 17.0),
                (10.0, 10.0),
            ]),
            vec![],
        ),
    ])));
    vec![
        geo_batch(
            &[0, 1, 2, 3, 4],
            &[
                Some(point_wkb(1.0, 2.0)),
                Some(linestring),
                Some(square_wkb(10.0, 10.0, 20.0)),
                Some(holed_polygon_wkb()),
                None,
            ],
        ),
        geo_batch(
            &[5, 6, 7, 8],
            &[
                Some(multi_point),
                Some(multi_linestring),
                Some(multi_polygon),
                Some(point_wkb(-3.0, 4.0)),
            ],
        ),
    ]
}

/// (a) ADR-0012: catena di sei op fondibili su fixture multi-tipo — output
/// byte-per-byte identico (confronto diretto dei `RecordBatch`, schema e
/// dati) e metriche per nodo preservate (D12.6).
#[test]
fn a_happy_path_multi_type_byte_per_byte() {
    let plan = happy_path_plan();
    assert_group_formation(&plan, &["t", "d", "r", "s", "k", "e"]);
    let (fused_batches, fused_metrics) = run_ok(&plan, multi_type_batches(), true);
    let (plain_batches, plain_metrics) = run_ok(&plan, multi_type_batches(), false);
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "percorso fuso eseguito");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
    assert_eq!(fused_batches, plain_batches, "output fuso diverso dal non fuso");
    for node in ["t", "d", "r", "s", "k", "e"] {
        let fused_node = &fused_metrics.nodes[node];
        let plain_node = &plain_metrics.nodes[node];
        assert_eq!(
            (fused_node.rows_in, fused_node.rows_out),
            (plain_node.rows_in, plain_node.rows_out),
            "{node}: righe 1:1 in A/B"
        );
    }
}

/// Identita' contratto/schema con fusione on/off (D12.5): la fusione non puo'
/// cambiare nessun contratto d'arco — schema IPC (metadati `FieldId` e blocco
/// canonico R2.2 inclusi) e contratto d'uscita identici.
#[test]
fn contract_and_schema_identical_with_fusion_on_and_off() {
    let oracle_plan = happy_path_plan();
    let validated = graph(&oracle_plan);
    let fused = execute(&validated, single_input(multi_type_batches()), runtime(true)).expect("fuso");
    let plain = execute(&validated, single_input(multi_type_batches()), runtime(false)).expect("non fuso");
    assert_eq!(
        fused.schema(),
        plain.schema(),
        "schema IPC identico (FieldId e blocco canonico nei metadati)"
    );
    // `DataContract` non implementa `PartialEq`: il confronto e' sulla forma
    // `Debug`, deterministica a parita' di build.
    assert_eq!(
        format!("{:?}", fused.output_contract()),
        format!("{:?}", plain.output_contract()),
        "contratto d'uscita identico"
    );
}

// ---------------------------------------------------------------------------
// (b) Cella oltre MAX_CELL_BYTES al nodo 1 di 3 (D12.3)
// ---------------------------------------------------------------------------

fn cell_too_large_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "d1", "op": "geo.densify", "in": ["main"],
             "config": {"max_segment_length": 1.0}},
            {"id": "t", "op": "geo.translate", "in": ["d1"],
             "config": {"x_offset": 1.0, "y_offset": 1.0}},
            {"id": "s", "op": "geo.simplify", "in": ["t"], "config": {"tolerance": 0.5}},
        ],
        "output": "s",
    })
}

/// `MultiLineString` di 1000 segmenti lunghi 4193: la densify con passo 1.0
/// produce 4194 coordinate per parte, stima totale `4_194_000` — SOTTO il
/// tetto del kernel (2^22) — ma il WKB e' ~67,1 MiB, OLTRE `MAX_CELL_BYTES`:
/// l'errore scatta nel check di cella del primo nodo (encode non fuso,
/// `wkb_size_xy` fuso) con la stessa misura esatta e la stessa attribuzione
/// (D12.3). `MultiLineString` e non `MultiPolygon`: la validazione OGC dei
/// poligoni e' appaiata (O(n^2)) e renderebbe la fixture troppo lenta in
/// debug; le linee si validano in O(n).
fn oversized_cell_batches() -> Vec<RecordBatch> {
    let lines: Vec<LineString<f64>> = (0..1_000_u32)
        .map(|index| {
            let y = f64::from(index) * 10.0;
            LineString::from(vec![(0.0, y), (4_193.0, y)])
        })
        .collect();
    vec![geo_batch(
        &[0],
        &[Some(to_wkb(&Geometry::MultiLineString(MultiLineString::new(
            lines,
        ))))],
    )]
}

/// (b) ADR-0012: la prima op produce una cella oltre 64 MiB -> `CellTooLarge`
/// al PRIMO nodo in entrambi i percorsi (stessa variante, categoria e motivo
/// con la stessa misura esatta in byte).
#[test]
fn b_cell_over_max_cell_bytes_attributed_to_first_node() {
    let plan = cell_too_large_plan();
    assert_group_formation(&plan, &["d1", "t", "s"]);
    let signature = assert_oracle_error("b", &plan, &oversized_cell_batches, Some("d1"));
    assert_eq!(signature.variant, "Execution", "b: errore di esecuzione");
    assert!(
        signature.reason.contains("cella WKB da"),
        "b: motivo `CellTooLarge`: {}",
        signature.reason
    );
    assert!(
        signature.reason.contains("batch_seq"),
        "b: contesto diagnostico presente: {}",
        signature.reason
    );
}

// ---------------------------------------------------------------------------
// (c) Input malformato al primo nodo
// ---------------------------------------------------------------------------

fn malformed_input_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 1.0, "y_offset": 1.0}},
            {"id": "s", "op": "geo.simplify", "in": ["t"], "config": {"tolerance": 0.01}},
            {"id": "e", "op": "geo.envelope", "in": ["s"], "config": {}},
        ],
        "output": "e",
    })
}

/// WKB di punto troncato a meta' delle coordinate.
fn truncated_point_wkb() -> Vec<u8> {
    point_wkb(1.0, 2.0)[..10].to_vec()
}

/// WKB di punto con la coordinata X sovrascritta a NaN (layout little-endian:
/// endianness, type code, poi X ai byte 5..13).
fn nan_point_wkb() -> Vec<u8> {
    let mut payload = point_wkb(1.0, 2.0);
    payload[5..13].copy_from_slice(&f64::NAN.to_le_bytes());
    payload
}

/// WKB di poligono con un anello di quattro punti NON chiuso.
fn unclosed_ring_wkb() -> Vec<u8> {
    let mut payload = vec![1_u8]; // little endian
    payload.extend_from_slice(&3_u32.to_le_bytes()); // Polygon
    payload.extend_from_slice(&1_u32.to_le_bytes()); // un anello
    payload.extend_from_slice(&4_u32.to_le_bytes()); // quattro punti
    for (x, y) in [(0.0_f64, 0.0_f64), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)] {
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
    }
    payload
}

/// WKB di poligono a farfalla (anello auto-intersecante): strutturalmente
/// ben formato (anello chiuso, coordinate finite) ma OGC-invalido — supera la
/// validazione strutturale dell'arco di input e fallisce al check OGC del
/// decode del primo nodo (`geometry_from_wkb`, ADR 11).
fn bowtie_wkb() -> Vec<u8> {
    to_wkb(&Geometry::Polygon(polygon![
        (x: 0.0, y: 0.0),
        (x: 10.0, y: 10.0),
        (x: 10.0, y: 0.0),
        (x: 0.0, y: 10.0),
        (x: 0.0, y: 0.0),
    ]))
}

/// (c) ADR-0012: WKB malformato in input -> stesso errore nei due percorsi.
///
/// Esito rilevato dall'oracolo (diverso dalla lettera del caso ADR, che
/// attende l'attribuzione al primo nodo): il WKB STRUTTURALMENTE invalido
/// (byte troncati, NaN, anello non chiuso) e' rifiutato dalla validazione
/// dell'ARCO DI INPUT (`validate_wkb_contract_for_dimensions_with_depth`)
/// prima che qualunque nodo sia eseguito — variante `InvalidPlan`, nessun
/// nodo, riga nel motivo — identicamente nei due percorsi (la fusione non
/// e' ancora in gioco). L'attribuzione al primo nodo vale per cio' che
/// supera l'arco: il sottocaso "bowtie" (OGC-invalido ma strutturalmente
/// valido) fallisce al decode del primo nodo `t` in entrambi i percorsi.
/// Il sottocaso "prima riga" verifica la selezione del primo errore in
/// ordine di riga: con due celle malformate l'errore e' quello della riga 0.
#[test]
fn c_malformed_wkb_attributed_to_first_node() {
    let plan = malformed_input_plan();
    assert_group_formation(&plan, &["t", "s", "e"]);
    let cases: [(&str, Vec<u8>); 3] = [
        ("c-troncati", truncated_point_wkb()),
        ("c-nan", nan_point_wkb()),
        ("c-anello-aperto", unclosed_ring_wkb()),
    ];
    let mut first_reason = String::new();
    for (label, cell) in cases {
        let signature = assert_oracle_error(
            label,
            &plan,
            &|| vec![geo_batch(&[0, 1], &[Some(cell.clone()), Some(point_wkb(3.0, 4.0))])],
            None,
        );
        assert_eq!(
            signature.variant, "InvalidPlan",
            "{label}: rifiuto strutturale all'arco di input"
        );
        assert!(
            signature.reason.contains("(riga 0)"),
            "{label}: la riga e' nel motivo: {}",
            signature.reason
        );
        if label == "c-troncati" {
            first_reason = signature.reason;
        }
    }
    // Due righe malformate (riga 0 troncata, riga 1 NaN): l'errore selezionato
    // e' quello della riga 0 — stessa riga nei due percorsi.
    let signature = assert_oracle_error(
        "c-prima-riga",
        &plan,
        &|| vec![geo_batch(&[0, 1], &[Some(truncated_point_wkb()), Some(nan_point_wkb())])],
        None,
    );
    assert_eq!(
        signature.reason, first_reason,
        "c-prima-riga: selezione del primo errore in ordine di riga"
    );
    // OGC-invalido strutturalmente valido: supera l'arco di input e fallisce
    // al decode del PRIMO NODO in entrambi i percorsi (nel runner fuso il
    // decode iniziale e' attribuito al primo kernel del gruppo).
    let signature = assert_oracle_error(
        "c-bowtie",
        &plan,
        &|| vec![geo_batch(&[0], &[Some(bowtie_wkb())])],
        Some("t"),
    );
    assert_eq!(signature.variant, "Execution", "c-bowtie: errore di esecuzione");
}

// ---------------------------------------------------------------------------
// (d) Non-finiti prodotti da kernel (D12.4)
// ---------------------------------------------------------------------------

fn centroid_overflow_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 1.0, "y_offset": 1.0}},
            {"id": "c", "op": "geo.centroid", "in": ["t"], "config": {}},
            {"id": "s", "op": "geo.simplify", "in": ["c"], "config": {"tolerance": 0.01}},
        ],
        "output": "s",
    })
}

/// (d1) centroide su coordinate ~1e308: i prodotti della formula traboccano a
/// `inf`/`NaN`. Profilo A (D12.4): l'output e' rivalidato al nodo che lo ha
/// prodotto (`transform_geometry_canonical`) — errore al nodo `c` in entrambi
/// i percorsi, stessa variante.
#[test]
fn d1_centroid_overflow_attributed_to_producer() {
    let plan = centroid_overflow_plan();
    assert_group_formation(&plan, &["t", "c", "s"]);
    let fixture = || vec![geo_batch(&[0], &[Some(square_wkb(1e308, 1e308, 1e307))])];
    let signature = assert_oracle_error("d1", &plan, &fixture, Some("c"));
    assert_eq!(signature.variant, "Execution", "d1: errore di esecuzione");
}

fn scale_inf_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 1.0, "y_offset": 1.0}},
            {"id": "k", "op": "geo.scale", "in": ["t"],
             "config": {"x_factor": 1e308, "y_factor": 1.0}},
            {"id": "r", "op": "geo.rotate", "in": ["k"], "config": {"degrees": 10.0}},
        ],
        "output": "r",
    })
}

/// (d2) scale x1e308 a meta' catena: 1e30 * 1e308 = `inf`. L'ADR cita questo
/// scenario come profilo B (errore al nodo k+1), ma con gli op di M1 NON e'
/// realizzabile: `scale` (`affine_transform` -> `validate_output`) valida il
/// proprio output e rifiuta i non-finiti al produttore, in entrambi i
/// percorsi — l'errore e' quindi attribuito al nodo `k` (stesso kernel,
/// stessa chiamata, stessa variante). Parita' confermata nella sola forma
/// raggiungibile; il braccio k+1 del runner fuso per i non-finiti resta
/// difesa in profondita' senza trigger dagli op M1 (vedi report).
#[test]
fn d2_scale_to_infinite_attributed_to_producer() {
    let plan = scale_inf_plan();
    assert_group_formation(&plan, &["t", "k", "r"]);
    let fixture = || vec![geo_batch(&[0], &[Some(square_wkb(1e30, 1e30, 1e30))])];
    let signature = assert_oracle_error("d2", &plan, &fixture, Some("k"));
    assert_eq!(signature.variant, "Execution", "d2: errore di esecuzione");
}

// ---------------------------------------------------------------------------
// (e) OGC-invalido prodotto a meta' catena (D12.4, profilo B)
// ---------------------------------------------------------------------------

fn ogc_invalid_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 1.0, "y_offset": 1.0}},
            {"id": "k", "op": "geo.scale", "in": ["t"],
             "config": {"x_factor": 0.0, "y_factor": 1.0}},
            {"id": "r", "op": "geo.rotate", "in": ["k"], "config": {"degrees": 10.0}},
        ],
        "output": "r",
    })
}

/// (e) scale con fattore X nullo collasserebbe l'anello su un segmento
/// verticale (punti ripetuti, OGC-invalido). Come per (d2), lo scenario
/// «errore al nodo successivo» dell'ADR non e' realizzabile con gli op di
/// M1: OGNI kernel fondibile valida il proprio output (affine/snap/densify/
/// buffer/simplify -> `validate_output` con `check_validation` OGC) e
/// rifiuta l'OGC-invalido al produttore — qui `scale` fallisce al nodo `k`
/// con `InvalidOutput` in entrambi i percorsi, stessa variante e motivo.
/// Il check OGC inter-passo del runner fuso (D12.4 profilo B, attribuzione
/// k+1) resta difesa in profondita' non raggiungibile via output dei kernel
/// M1; la divergenza di precedenza al validatore (caso -0.0/NaN di D12.4)
/// NON si manifesta perche' nessun intermedio invalido arriva mai al
/// round-trip WKB (vedi report).
#[test]
fn e_ogc_invalid_mid_chain_attributed_to_producer() {
    let plan = ogc_invalid_plan();
    assert_group_formation(&plan, &["t", "k", "r"]);
    let fixture = || vec![geo_batch(&[0], &[Some(square_wkb(0.0, 0.0, 10.0))])];
    let signature = assert_oracle_error("e", &plan, &fixture, Some("k"));
    assert_eq!(signature.variant, "Execution", "e: errore di esecuzione");
}

// ---------------------------------------------------------------------------
// (f) Cancellazione a meta' gruppo
// ---------------------------------------------------------------------------

/// Input geo lazy che cancella il token quando l'executor tira il batch
/// successivo a `cancel_after` (pattern dei test di executor per la
/// cancellazione): il Ctrl-C arriva a meta' stream ed e' osservato al
/// confine del kernel in corso — nel runner fuso via `control` tra un kernel
/// e l'altro, nel percorso non fuso al confine di kernel del loop.
struct CancellingGeoInput {
    batches: std::vec::IntoIter<RecordBatch>,
    pulled: usize,
    cancel_after: usize,
    token: CancellationToken,
}

impl Iterator for CancellingGeoInput {
    type Item = plenora_core::Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        let batch = self.batches.next()?;
        self.pulled += 1;
        if self.pulled > self.cancel_after {
            self.token.cancel();
        }
        Some(Ok(batch))
    }
}

fn run_cancellation(
    plan: &Value,
    batches: Vec<RecordBatch>,
    geo_fusion: bool,
) -> (PlenoraError, ExecutionMetrics) {
    let token = CancellationToken::new();
    let input = Input::from_iter(
        geo_schema(),
        CancellingGeoInput {
            batches: batches.into_iter(),
            pulled: 0,
            cancel_after: 1,
            token: token.clone(),
        },
    );
    let inputs = Inputs::new().with("main", input).expect("input unico");
    let runtime = RuntimeContext {
        cancellation: token,
        ..runtime(geo_fusion)
    };
    let mut output = execute(&graph(plan), inputs, runtime).expect("execute");
    let error =
        first_stream_error(&mut output).expect("atteso Cancelled, lo stream e' terminato");
    (error, output.metrics())
}

fn cancellation_batches() -> Vec<RecordBatch> {
    (0..3_u32)
        .map(|batch_index| {
            let base = f64::from(batch_index) * 100.0;
            geo_batch(
                &[i64::from(batch_index) * 2, i64::from(batch_index) * 2 + 1],
                &[
                    Some(square_wkb(base, base, 10.0)),
                    Some(point_wkb(base + 1.0, base + 2.0)),
                ],
            )
        })
        .collect()
}

/// (f) ADR-0012: token attivato mentre lo stream scorre (dal secondo batch in
/// poi) -> `Cancelled` con la stessa attribuzione nei due percorsi. Il check
/// cooperativo osserva il token al confine del primo kernel del gruppo sul
/// batch in corso: il nodo e' `t` in entrambi i percorsi. La cancellazione
/// osservabile esattamente TRA due kernel dello stesso batch non e'
/// iniettabile dall'esterno senza un hook dedicato (vedi report, caso (g)).
#[test]
fn f_cancellation_mid_group_same_node() {
    let plan = malformed_input_plan();
    assert_group_formation(&plan, &["t", "s", "e"]);
    let (fused_error, fused_metrics) = run_cancellation(&plan, cancellation_batches(), true);
    let (plain_error, plain_metrics) = run_cancellation(&plan, cancellation_batches(), false);
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "percorso fuso eseguito");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
    let signature = error_signature(&fused_error);
    assert_eq!(signature.variant, "Cancelled", "f: variante Cancelled");
    assert_eq!(signature.node.as_deref(), Some("t"), "f: stesso nodo osservante");
    assert_eq!(signature.category, ErrorCategory::Cancelled, "f: categoria");
    assert_eq!(
        signature,
        error_signature(&plain_error),
        "f: errore diverso tra i percorsi\n  fuso:     {fused_error}\n  non fuso: {plain_error}"
    );
}

// ---------------------------------------------------------------------------
// M2 — misura terminale in coda al gruppo fuso
// ---------------------------------------------------------------------------
//
// Nota sul caso errore «misura su geometria invalida prodotta a meta'
// catena»: NON realizzabile con gli op del perimetro M1+M2, per la stessa
// ragione dei casi (d2)/(e) — OGNI kernel fondibile valida il proprio
// output (validate_output / pipeline canonica di profilo A), quindi nessun
// intermedio OGC-invalido raggiunge mai il decode del nodo misura. La
// validazione pre-misura del runner fuso (D12.4 profilo B -> nodo misura,
// variante `FusedStepError::Measure`) resta difesa in profondita' ed e'
// verificata direttamente a livello runner
// (`unary.rs::tests::measure_validation_error_is_attributed_to_the_measure_node`).
// I casi errore raggiungibili con misura in coda sono coperti qui sotto:
// limiti di cella e input invalido, che devono mantenere l'attribuzione ai
// nodi trasformazione (la misura non li sposta).

/// Piano M2: due transform fondibili + misura terminale `geo.area`.
fn terminal_area_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 3.0, "y_offset": -2.0}},
            {"id": "r", "op": "geo.rotate", "in": ["t"], "config": {"degrees": 15.0}},
            {"id": "a", "op": "geo.area", "in": ["r"], "config": {}},
        ],
        "output": "a",
    })
}

/// Piano M2: UN transform + misura terminale `geo.to_wkt` (gruppo di due).
fn terminal_to_wkt_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 3.0, "y_offset": -2.0}},
            {"id": "w", "op": "geo.to_wkt", "in": ["t"], "config": {}},
        ],
        "output": "w",
    })
}

/// (M2) Percorso felice con misura terminale: il gruppo include il nodo
/// misura (prova di formazione, D12.2), l'output e' byte-per-byte identico
/// al percorso non fuso su fixture multi-tipo con null, le metriche per nodo
/// sono preservate (D12.6) e la colonna geometria SOPRAVVIVE (semantica v4
/// "add column": misura appesa in coda, null-in -> null-out).
#[test]
fn m2_happy_path_terminal_measure_byte_per_byte() {
    use plenora_core::arrow::array::{Array, Float64Array, StringArray};

    for (case, plan, nodes) in [
        ("m2-area", terminal_area_plan(), vec!["t", "r", "a"]),
        ("m2-to_wkt", terminal_to_wkt_plan(), vec!["t", "w"]),
    ] {
        assert_group_formation(&plan, &nodes);
        let (fused_batches, fused_metrics) = run_ok(&plan, multi_type_batches(), true);
        let (plain_batches, plain_metrics) = run_ok(&plan, multi_type_batches(), false);
        assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "{case}: percorso fuso eseguito");
        assert_eq!(plain_metrics.geo_fusion_fallbacks, 0, "{case}: nessun fallback atteso");
        assert_eq!(
            fused_batches, plain_batches,
            "{case}: output fuso diverso dal non fuso"
        );
        for node in &nodes {
            let fused_node = &fused_metrics.nodes[*node];
            let plain_node = &plain_metrics.nodes[*node];
            assert_eq!(
                (fused_node.rows_in, fused_node.rows_out),
                (plain_node.rows_in, plain_node.rows_out),
                "{case}: {node}: righe 1:1 in A/B"
            );
        }
        // La colonna geometria sopravvive (indice 1, Binary) e la misura
        // (indice 2) e' null esattamente dove la geometria e' null (riga 4
        // del batch 0 della fixture multi-tipo).
        let batch = &fused_batches[0];
        assert_eq!(
            batch.column(1).data_type(),
            &DataType::Binary,
            "{case}: la colonna geometria sopravvive alla misura"
        );
        let measure_null = match batch.column(2).data_type() {
            DataType::Float64 => batch
                .column(2)
                .as_any()
                .downcast_ref::<Float64Array>()
                .map(|column| column.is_null(4)),
            DataType::Utf8 => batch
                .column(2)
                .as_any()
                .downcast_ref::<StringArray>()
                .map(|column| column.is_null(4)),
            other => panic!("{case}: tipo misura inatteso {other}"),
        };
        assert_eq!(measure_null, Some(true), "{case}: null-in -> null-out");
    }
}

/// (M2) Cella oltre `MAX_CELL_BYTES` al primo transform con misura in coda:
/// la misura NON sposta l'attribuzione — `CellTooLarge` scatta al nodo che
/// ha prodotto la cella (D12.3) in entrambi i percorsi, prima che il nodo
/// misura sia eseguito (nel non fuso il nodo misura parte solo dopo che il
/// nodo trasformazione ha completato tutte le righe).
#[test]
fn m2_oversize_cell_with_terminal_measure_attributed_to_transform() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "d1", "op": "geo.densify", "in": ["main"],
             "config": {"max_segment_length": 1.0}},
            {"id": "t", "op": "geo.translate", "in": ["d1"],
             "config": {"x_offset": 1.0, "y_offset": 1.0}},
            {"id": "vc", "op": "geo.vertex_count", "in": ["t"], "config": {}},
        ],
        "output": "vc",
    });
    assert_group_formation(&plan, &["d1", "t", "vc"]);
    let signature = assert_oracle_error("m2-b", &plan, &oversized_cell_batches, Some("d1"));
    assert_eq!(signature.variant, "Execution", "m2-b: errore di esecuzione");
    assert!(
        signature.reason.contains("cella WKB da"),
        "m2-b: motivo `CellTooLarge`: {}",
        signature.reason
    );
}

/// (M2) Input OGC-invalido (bowtie, strutturalmente valido: supera l'arco di
/// input) con misura in coda: fallisce al decode del PRIMO nodo in entrambi
/// i percorsi — la misura in coda non cambia l'attribuzione degli errori di
/// input.
#[test]
fn m2_ogc_invalid_input_with_terminal_measure_attributed_to_first_node() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 1.0, "y_offset": 1.0}},
            {"id": "a", "op": "geo.area", "in": ["t"], "config": {}},
        ],
        "output": "a",
    });
    assert_group_formation(&plan, &["t", "a"]);
    let signature = assert_oracle_error(
        "m2-c",
        &plan,
        &|| vec![geo_batch(&[0], &[Some(bowtie_wkb())])],
        Some("t"),
    );
    assert_eq!(signature.variant, "Execution", "m2-c: errore di esecuzione");
}

// ---------------------------------------------------------------------------
// M3 — reproject / make_valid (backend feature-gated)
// ---------------------------------------------------------------------------

/// Piano M3: `make_valid` in TESTA al gruppo (trappola 1): l'input
/// OGC-invalido arriva dall'arco (gate strutturale) direttamente al nodo
/// che esiste per ripararlo.
fn make_valid_first_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "mv", "op": "geo.make_valid", "in": ["main"], "config": {}},
            {"id": "t", "op": "geo.translate", "in": ["mv"],
             "config": {"x_offset": 1.0, "y_offset": 2.0}},
        ],
        "output": "t",
    })
}

/// Piano M3: `make_valid` a meta' catena, con successore.
#[cfg(feature = "geos-backend")]
fn make_valid_mid_chain_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "t", "op": "geo.translate", "in": ["main"],
             "config": {"x_offset": 1.0, "y_offset": 2.0}},
            {"id": "mv", "op": "geo.make_valid", "in": ["t"], "config": {}},
            {"id": "r", "op": "geo.rotate", "in": ["mv"], "config": {"degrees": 10.0}},
        ],
        "output": "r",
    })
}

/// Piano M3: `reproject` (EPSG:32632 -> EPSG:3857) seguito da un altro
/// transform — cambio di CRS a meta' catena. Il target e' proiettato:
/// i transform successivi richiedono `CrsRequirement::Projected` (verso un
/// target geografico come EPSG:4326 la catena prosegue solo con op
/// `Known`, vedi il piano con misura terminale sotto).
fn reproject_chain_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "p", "op": "geo.reproject", "in": ["main"],
             "config": {"target_crs": "EPSG:3857"}},
            {"id": "t", "op": "geo.translate", "in": ["p"],
             "config": {"x_offset": 1000.0, "y_offset": -2000.0}},
        ],
        "output": "t",
    })
}

/// Piano M3: `reproject` verso un CRS GEOGRAFICO (EPSG:4326) con misura
/// terminale `to_wkt` (requisito CRS `Known`) in coda al gruppo.
#[cfg(feature = "proj-backend")]
fn reproject_geographic_measure_plan() -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "p", "op": "geo.reproject", "in": ["main"],
             "config": {"target_crs": "EPSG:4326"}},
            {"id": "w", "op": "geo.to_wkt", "in": ["p"], "config": {}},
        ],
        "output": "w",
    })
}

/// Bowtie (OGC-invalido ma strutturalmente valido: supera l'arco di input),
/// geometrie valide multi-tipo e null, su due batch.
#[cfg(feature = "geos-backend")]
fn invalid_then_valid_batches() -> Vec<RecordBatch> {
    vec![
        geo_batch(
            &[0, 1, 2, 3],
            &[
                Some(bowtie_wkb()),
                Some(square_wkb(0.0, 0.0, 10.0)),
                Some(point_wkb(5.0, 6.0)),
                None,
            ],
        ),
        geo_batch(
            &[4, 5],
            &[Some(holed_polygon_wkb()), Some(bowtie_wkb())],
        ),
    ]
}

/// (m3-a) ADR-0012 M3, trappola 1 — il caso centrale: input OGC-INVALIDO ->
/// `make_valid` in testa al gruppo. Nel percorso non fuso il nodo legge col
/// SOLO gate strutturale di `make_valid_wkb` e ripara; il percorso fuso NON
/// deve rifiutare l'intermedio/ingresso OGC-invalido (decode iniziale solo
/// strutturale): riparato identico nei due percorsi, NESSUN errore, output
/// OGC-valido.
///
/// Nota di copertura (come i casi (d2)/(e)): «transform che produce
/// OGC-invalido -> `make_valid`» NON e' realizzabile con gli op del perimetro
/// — ogni kernel fondibile valida il proprio output — quindi la meta'
/// inter-passo dell'eccezione (check OGC omesso davanti a `make_valid`) resta
/// difesa in profondita'; la meta' raggiungibile (`make_valid` in testa,
/// input OGC-invalido dall'arco) e' verificata qui.
#[cfg(feature = "geos-backend")]
#[test]
fn m3a_ogc_invalid_input_to_make_valid_repaired_identically() {
    use plenora_core::arrow::array::Array;

    let plan = make_valid_first_plan();
    assert_group_formation(&plan, &["mv", "t"]);
    let (fused_batches, fused_metrics) = run_ok(&plan, invalid_then_valid_batches(), true);
    let (plain_batches, plain_metrics) = run_ok(&plan, invalid_then_valid_batches(), false);
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "percorso fuso eseguito");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
    assert_eq!(
        fused_batches, plain_batches,
        "m3-a: riparazione diversa tra i percorsi"
    );
    for node in ["mv", "t"] {
        let fused_node = &fused_metrics.nodes[node];
        let plain_node = &plain_metrics.nodes[node];
        assert_eq!(
            (fused_node.rows_in, fused_node.rows_out),
            (plain_node.rows_in, plain_node.rows_out),
            "m3-a: {node}: righe 1:1 in A/B"
        );
    }
    // Nessuna cella null persa/guadagnata (riga 3 del primo batch) e output
    // riparato OGC-valido su tutte le celle.
    let first = fused_batches[0]
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("colonna geometria");
    assert!(first.is_null(3), "m3-a: null-in -> null-out");
    for batch in &fused_batches {
        let cells = batch
            .column(1)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .expect("colonna geometria");
        for cell in cells.iter().flatten() {
            plenora_kernels_geo::geometry_from_wkb(cell)
                .expect("m3-a: output riparato OGC-valido");
        }
    }
}

/// (m3-a2) Confine del gruppo subito dopo `make_valid` (gruppo con sola
/// misura a valle): nel percorso non fuso il nodo emette i byte WKB di GEOS
/// (o il passthrough dell'input valido); il runner fuso ri-encoda la forma
/// decodificata — i byte di confine e la colonna misura devono coincidere
/// (parita' GEOS/geozero sulla stessa geometria riparata).
#[cfg(feature = "geos-backend")]
#[test]
fn m3a2_make_valid_then_measure_boundary_bytes_match() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "mv", "op": "geo.make_valid", "in": ["main"], "config": {}},
            {"id": "a", "op": "geo.area", "in": ["mv"], "config": {}},
        ],
        "output": "a",
    });
    assert_group_formation(&plan, &["mv", "a"]);
    let (fused_batches, fused_metrics) = run_ok(&plan, invalid_then_valid_batches(), true);
    let (plain_batches, plain_metrics) = run_ok(&plan, invalid_then_valid_batches(), false);
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "percorso fuso eseguito");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
    assert_eq!(
        fused_batches, plain_batches,
        "m3-a2: byte di confine o misura diversi tra i percorsi"
    );
}

/// (m3-b) ADR-0012 M3: catena con `reproject` — EPSG:32632 -> EPSG:3857 ->
/// translate. Output byte-per-byte identico nei due percorsi e schema di
/// confine col CRS TARGET (il runner fuso costruisce il batch sullo schema
/// dell'ultima trasformazione: il cambio di CRS a meta' gruppo e' fisico,
/// non contrattuale).
#[cfg(feature = "proj-backend")]
#[test]
fn m3b_reproject_chain_byte_per_byte_with_target_crs_schema() {
    let plan = reproject_chain_plan();
    assert_group_formation(&plan, &["p", "t"]);
    let (fused_batches, fused_metrics) = run_ok(&plan, multi_type_batches(), true);
    let (plain_batches, plain_metrics) = run_ok(&plan, multi_type_batches(), false);
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "percorso fuso eseguito");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
    assert_eq!(
        fused_batches, plain_batches,
        "m3-b: output fuso diverso dal non fuso"
    );
    for node in ["p", "t"] {
        let fused_node = &fused_metrics.nodes[node];
        let plain_node = &plain_metrics.nodes[node];
        assert_eq!(
            (fused_node.rows_in, fused_node.rows_out),
            (plain_node.rows_in, plain_node.rows_out),
            "m3-b: {node}: righe 1:1 in A/B"
        );
    }
    let metadata = format!("{:?}", fused_batches[0].schema().field(1).metadata());
    assert!(
        metadata.contains("EPSG:3857"),
        "m3-b: il campo geometria di confine porta il CRS target: {metadata}"
    );
}

/// (m3-b2) ADR-0012 M3+M2: `reproject` verso un CRS GEOGRAFICO con misura
/// terminale in coda al gruppo — la colonna geometria sopravvive col CRS
/// target e la misura e' byte-per-byte identica.
#[cfg(feature = "proj-backend")]
#[test]
fn m3b2_reproject_to_geographic_with_terminal_measure() {
    let plan = reproject_geographic_measure_plan();
    assert_group_formation(&plan, &["p", "w"]);
    let (fused_batches, fused_metrics) = run_ok(&plan, multi_type_batches(), true);
    let (plain_batches, plain_metrics) = run_ok(&plan, multi_type_batches(), false);
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "percorso fuso eseguito");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
    assert_eq!(
        fused_batches, plain_batches,
        "m3-b2: output fuso diverso dal non fuso"
    );
    let metadata = format!("{:?}", fused_batches[0].schema().field(1).metadata());
    assert!(
        metadata.contains("EPSG:4326"),
        "m3-b2: il campo geometria di confine porta il CRS geografico: {metadata}"
    );
}

/// (m3-c) ADR-0012 M3: `make_valid` a meta' catena seguito da altro op —
/// su input valido e' un passthrough byte-identico e la validazione
/// inter-passo standard (strutturale + OGC) resta in vigore davanti al
/// successore (l'output riparato e' valido per contratto del kernel).
#[cfg(feature = "geos-backend")]
#[test]
fn m3c_make_valid_mid_chain_byte_per_byte() {
    let plan = make_valid_mid_chain_plan();
    assert_group_formation(&plan, &["t", "mv", "r"]);
    let (fused_batches, fused_metrics) = run_ok(&plan, multi_type_batches(), true);
    let (plain_batches, plain_metrics) = run_ok(&plan, multi_type_batches(), false);
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "percorso fuso eseguito");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
    assert_eq!(
        fused_batches, plain_batches,
        "m3-c: output fuso diverso dal non fuso"
    );
    for node in ["t", "mv", "r"] {
        let fused_node = &fused_metrics.nodes[node];
        let plain_node = &plain_metrics.nodes[node];
        assert_eq!(
            (fused_node.rows_in, fused_node.rows_out),
            (plain_node.rows_in, plain_node.rows_out),
            "m3-c: {node}: righe 1:1 in A/B"
        );
    }
}

/// (m3-d) ADR-0012 M3: cancellazione con `make_valid` `NonInterruptible`
/// nel gruppo — MAI dentro il kernel: il check al confine di `make_valid`
/// onora il behavior di catalogo (saltato) in entrambi i percorsi e il
/// `Cancelled` e' osservato al PRIMO nodo cooperativo successivo (`t`),
/// con la stessa attribuzione.
#[cfg(feature = "geos-backend")]
#[test]
fn m3d_cancellation_with_non_interruptible_make_valid_same_node() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "mv", "op": "geo.make_valid", "in": ["main"], "config": {}},
            {"id": "t", "op": "geo.translate", "in": ["mv"],
             "config": {"x_offset": 1.0, "y_offset": 1.0}},
            {"id": "r", "op": "geo.rotate", "in": ["t"], "config": {"degrees": 10.0}},
        ],
        "output": "r",
    });
    assert_group_formation(&plan, &["mv", "t", "r"]);
    let (fused_error, fused_metrics) = run_cancellation(&plan, cancellation_batches(), true);
    let (plain_error, plain_metrics) = run_cancellation(&plan, cancellation_batches(), false);
    assert_eq!(fused_metrics.geo_fusion_fallbacks, 0, "percorso fuso eseguito");
    assert_eq!(plain_metrics.geo_fusion_fallbacks, 0);
    let signature = error_signature(&fused_error);
    assert_eq!(signature.variant, "Cancelled", "m3-d: variante Cancelled");
    assert_eq!(
        signature.node.as_deref(),
        Some("t"),
        "m3-d: osservata al primo nodo cooperativo dopo make_valid (mai dentro)"
    );
    assert_eq!(
        signature,
        error_signature(&plain_error),
        "m3-d: errore diverso tra i percorsi\n  fuso:     {fused_error}\n  non fuso: {plain_error}"
    );
}

/// M3 a feature spente, SENZA cfg gate: un piano con `make_valid` o
/// `reproject` ha lo STESSO esito con fusione attiva e kill switch spento
/// in qualunque configurazione di build — a feature spente il rifiuto
/// fail-closed in validazione (capability `geos`/`proj` mancante, mai un
/// gruppo formato), a feature attive l'output byte-per-byte. Il
/// `BackendUnavailable` del trasporto e' coperto a livello runner
/// (`unary.rs::tests`) perche' i piani non lo raggiungono mai: la
/// validazione scatta prima.
#[test]
fn m3_backend_ops_identical_outcome_with_fusion_on_and_off() {
    /// Esito dell'esecuzione come `Result`: batch raccolti oppure il testo
    /// integrale del primo errore (validazione, esecuzione o stream).
    fn outcome(plan: &Value, geo_fusion: bool) -> Result<Vec<RecordBatch>, String> {
        let validated = validate(&plan.to_string(), &[("main".to_owned(), geo_contract())])
            .map_err(|error| error.to_string())?;
        let mut output = execute(&validated, single_input(multi_type_batches()), runtime(geo_fusion))
            .map_err(|error| error.to_string())?;
        let mut batches = Vec::new();
        for item in output.by_ref() {
            batches.push(item.map_err(|error| error.to_string())?);
        }
        Ok(batches)
    }

    for (case, plan) in [
        ("m3-off-make_valid", make_valid_first_plan()),
        ("m3-off-reproject", reproject_chain_plan()),
    ] {
        match (outcome(&plan, true), outcome(&plan, false)) {
            (Ok(fused), Ok(plain)) => assert_eq!(
                fused, plain,
                "{case}: output diverso tra i percorsi a feature attive"
            ),
            (Err(fused), Err(plain)) => assert_eq!(
                fused, plain,
                "{case}: rifiuto diverso tra i percorsi a feature spente"
            ),
            (fused, plain) => panic!(
                "{case}: esito divergente tra i percorsi (fuso: {}, non fuso: {})",
                fused.is_ok(),
                plain.is_ok()
            ),
        }
    }
}
