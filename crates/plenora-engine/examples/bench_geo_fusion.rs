//! Misura A/B engine-level della fusione dei segmenti geo (ADR-0012 M1+M2,
//! kill switch D12.9): due scenari, ciascuno eseguito con
//! `RuntimeContext.geo_fusion` true/false, N run alternate per modalita'.
//! Scenario `chain_transforms`: `geo.buffer -> geo.simplify -> geo.centroid`
//! (M1). Scenario `chain_terminal_measure`: la stessa catena + `geo.area` in
//! coda (M2 — la catena completa del baseline kernel-level, `chain_wkb` vs
//! `chain_fused` in `benchmarks/baseline/baseline.json`). A livello engine il
//! percorso include il framing dei `RecordBatch` tra un nodo e l'altro,
//! quindi il delta atteso e' minore del kernel-level ma deve restare
//! significativo. Con la feature `proj-backend` e' attivo anche lo scenario
//! `chain_reproject` (M3): `geo.reproject` (EPSG:32632 -> EPSG:3857) ->
//! `geo.translate` -> `geo.rotate` — la riproiezione domina il costo del
//! gruppo, quindi il delta atteso e' minore che negli altri scenari.
//!
//! Uso: `bench_geo_fusion` — una riga JSON per scenario+modalita' su stdout,
//! piu' una riga di sintesi con il delta percentuale. Fallisce (exit != 0)
//! se il percorso fuso registra fallback governor o se gli output delle due
//! modalita' non sono identici (stesso oracolo dei test di ADR-0012).

// Benchmark: usano il percorso permissivo di `Inputs`, deprecato ma
// ancora supportato. Non e' codice di produzione.
#![allow(deprecated)]

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use plenora_core::arrow::array::{ArrayRef, BinaryArray, Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};

use geo::{polygon, Geometry, Point};
use geozero::{CoordDimensions, ToWkb};

use plenora_engine::executor::{execute, ExecutionMetrics};
use plenora_engine::planner::{validate, ValidatedGraph};
use plenora_engine::prepare::RuntimeContext;
use plenora_engine::{Input, Inputs};
use plenora_kernels_geo::arrow_adapter::geometry_output_field;

/// Righe totali della fixture (dimensione dichiarata: ogni run resta entro
/// pochi secondi nel container rust:1.92, molto sotto il tetto di ~60s).
const ROWS: usize = 200_000;
/// Righe per batch (~10k, come da dimensionamento della misura).
const BATCH_ROWS: usize = 10_000;
/// Run per modalita', alternate A,B,A,B... per non legare il risultato
/// all'ordine di esecuzione.
const RUNS: usize = 5;

/// Piano v4: la catena del baseline kernel-level (buffer distanza 5.0,
/// simplify tolleranza 0.01, centroid) — tre kernel fondibili consecutivi
/// (perimetro M1, capability `TransformInPlace`).
fn chain_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 5.0}},
            {"id": "s", "op": "geo.simplify", "in": ["b"], "config": {"tolerance": 0.01}},
            {"id": "c", "op": "geo.centroid", "in": ["s"], "config": {}},
        ],
        "output": "c",
    })
}

/// Piano v4: la catena completa del baseline (buffer -> simplify -> centroid
/// -> area): tre `TransformInPlace` + la misura terminale (M2).
fn chain_plan_with_area() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 5.0}},
            {"id": "s", "op": "geo.simplify", "in": ["b"], "config": {"tolerance": 0.01}},
            {"id": "c", "op": "geo.centroid", "in": ["s"], "config": {}},
            {"id": "a", "op": "geo.area", "in": ["c"], "config": {}},
        ],
        "output": "a",
    })
}

/// Piano v4 (M3, richiede `proj-backend`): reproject EPSG:32632 ->
/// EPSG:3857, poi translate e rotate in metri — tre kernel fondibili
/// consecutivi, il primo backend-gated e `NonInterruptible`. (Il target
/// dev'essere proiettato: i transform a valle richiedono
/// `CrsRequirement::Projected`.)
#[cfg(feature = "proj-backend")]
fn chain_plan_reproject() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "p", "op": "geo.reproject", "in": ["main"],
             "config": {"target_crs": "EPSG:3857"}},
            {"id": "t", "op": "geo.translate", "in": ["p"],
             "config": {"x_offset": 100.0, "y_offset": -200.0}},
            {"id": "r", "op": "geo.rotate", "in": ["t"], "config": {"degrees": 1.0}},
        ],
        "output": "r",
    })
}

fn geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        geometry_output_field("geom", "EPSG:32632").expect("campo geometria"),
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

fn square_wkb(origin_x: f64, origin_y: f64, side: f64) -> Vec<u8> {
    Geometry::Polygon(polygon![
        (x: origin_x, y: origin_y),
        (x: origin_x + side, y: origin_y),
        (x: origin_x + side, y: origin_y + side),
        (x: origin_x, y: origin_y + side),
        (x: origin_x, y: origin_y),
    ])
    .to_wkb(CoordDimensions::xy())
    .expect("wkb fixture")
}

fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    Geometry::Point(Point::new(x, y))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb fixture")
}

/// Fixture: quadrati 20x20 su griglia da 500 colonne (passo 100) alternati
/// a punti al centro della cella, con l'1 per mille di celle null per
/// coprire il percorso nullable. Coordinate intere piccole: esatte in f64
/// per costruzione, nessun RNG (determinismo ADR-0001).
fn fixture_batches() -> Vec<RecordBatch> {
    let mut batches = Vec::with_capacity(ROWS.div_ceil(BATCH_ROWS));
    for first in (0..ROWS).step_by(BATCH_ROWS) {
        let len = BATCH_ROWS.min(ROWS - first);
        let ids: Vec<i64> = (first..first + len)
            .map(|row| i64::try_from(row).expect("righe < i64::MAX"))
            .collect();
        let cells: Vec<Option<Vec<u8>>> = (first..first + len)
            .map(|row| {
                if row % 1_000 == 999 {
                    None
                } else {
                    let col = u32::try_from(row / 2 % 500).expect("colonna < u32::MAX");
                    let line = u32::try_from(row / 2 / 500).expect("linea < u32::MAX");
                    let x = f64::from(col) * 100.0;
                    let y = f64::from(line) * 100.0;
                    if row % 2 == 0 {
                        Some(square_wkb(x, y, 20.0))
                    } else {
                        Some(point_wkb(x + 50.0, y + 50.0))
                    }
                }
            })
            .collect();
        let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
        batches.push(
            RecordBatch::try_new(
                geo_schema(),
                vec![
                    Arc::new(Int64Array::from(ids)) as ArrayRef,
                    Arc::new(BinaryArray::from(refs)) as ArrayRef,
                ],
            )
            .expect("batch geo fixture valido"),
        );
    }
    batches
}

/// Una run: preparazione (`geo_fusion` agisce in `prepare`) + esecuzione +
/// raccolta degli output. Ritorna batch, metriche e durata in secondi.
fn run_once(
    graph: &ValidatedGraph,
    fixture: &[RecordBatch],
    geo_fusion: bool,
) -> (Vec<RecordBatch>, ExecutionMetrics, f64) {
    let inputs = Inputs::new()
        .with(
            "main",
            Input::from_batches(fixture.to_vec()).expect("input non vuoto"),
        )
        .expect("input unico");
    let runtime = RuntimeContext {
        geo_fusion,
        ..RuntimeContext::default()
    };
    let start = Instant::now();
    let (batches, metrics) = execute(graph, inputs, runtime)
        .expect("execute")
        .collect_batches()
        .expect("stream ok");
    (batches, metrics, start.elapsed().as_secs_f64())
}

/// Stampa la riga JSON di una modalita' e ritorna la mediana (secondi).
fn report(mode: &str, durations: &mut [f64]) -> f64 {
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    let min = durations[0];
    let max = durations[durations.len() - 1];
    // ROWS << 2^53: la conversione e' esatta per costruzione.
    #[allow(clippy::cast_precision_loss)]
    let rate = ROWS as f64 / median;
    let batches = ROWS.div_ceil(BATCH_ROWS);
    println!(
        "{{\"mode\":\"{mode}\",\"runs\":{RUNS},\"rows\":{ROWS},\"batches\":{batches},\"median_seconds\":{median},\"min_seconds\":{min},\"max_seconds\":{max},\"rows_per_second\":{rate}}}"
    );
    median
}

/// Uno scenario A/B completo: 2 x RUNS run alternate, controlli bloccanti
/// (nessun fallback, output identici), una riga JSON per modalita' e la
/// sintesi col delta.
fn run_scenario(label: &str, plan: &serde_json::Value) {
    run_scenario_with_warmup(label, plan, false);
}

/// Come [`run_scenario`], con un warmup opzionale (una run per modalita',
/// non misurata): serve agli scenari con backend che pagano un'init per
/// thread al primo uso (la pipeline PROJ thread-local di `reproject`,
/// condivisa tra le modalita' perche' il pool rayon e' lo stesso del
/// processo) — senza warmup la prima run misurata includerebbe l'init e
/// sbilancerebbe la mediana della modalita' eseguita per prima.
fn run_scenario_with_warmup(label: &str, plan: &serde_json::Value, warmup: bool) {
    let graph =
        validate(&plan.to_string(), &[("main".to_owned(), geo_contract())]).expect("validate");
    let fixture = fixture_batches();

    if warmup {
        let _ = run_once(&graph, &fixture, true);
        let _ = run_once(&graph, &fixture, false);
    }

    // Run alternate A,B,A,B...: la mediana di 5 per modalita' non dipende
    // dall'ordine e un eventuale drift termico/di cache pesa su entrambe.
    let mut durations_fused = Vec::with_capacity(RUNS);
    let mut durations_plain = Vec::with_capacity(RUNS);
    let mut fused_reference: Option<Vec<RecordBatch>> = None;
    let mut plain_reference: Option<Vec<RecordBatch>> = None;
    for run in 0..RUNS * 2 {
        let fused = run % 2 == 0;
        let (batches, metrics, seconds) = run_once(&graph, &fixture, fused);
        if fused {
            assert_eq!(
                metrics.geo_fusion_fallbacks, 0,
                "{label}: nessun fallback governor atteso sulla fixture"
            );
            durations_fused.push(seconds);
            fused_reference.get_or_insert(batches);
        } else {
            durations_plain.push(seconds);
            plain_reference.get_or_insert(batches);
        }
    }

    // Oracolo A/B (D12.9): output identico tra le due modalita', come nei
    // test di ADR-0012 (confronto per valore dei RecordBatch di una run).
    let outputs_identical =
        fused_reference.expect("run fusa") == plain_reference.expect("run non fusa");
    assert!(
        outputs_identical,
        "{label}: output fuso diverso dal non fuso"
    );

    let median_fused = report("fused", &mut durations_fused);
    let median_plain = report("unfused", &mut durations_plain);
    let delta = (median_fused - median_plain) / median_plain * 100.0;
    println!(
        "{{\"scenario\":\"{label}\",\"delta_percent\":{delta},\"outputs_identical\":{outputs_identical},\"geo_fusion_fallbacks\":0}}"
    );
}

fn main() {
    run_scenario("chain_transforms", &chain_plan());
    run_scenario("chain_terminal_measure", &chain_plan_with_area());
    #[cfg(feature = "proj-backend")]
    run_scenario_with_warmup("chain_reproject", &chain_plan_reproject(), true);
}
