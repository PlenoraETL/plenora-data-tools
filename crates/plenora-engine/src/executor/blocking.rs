//! Il percorso bloccante: materializza, esegue, misura.
//!
//! Un kernel blocking non puo' emettere finche' non ha visto tutto l'input:
//! un ordinamento, un'aggregazione, un join. Qui l'input viene materializzato,
//! il kernel eseguito e l'esito misurato.
//!
//! # Perche' il dispatch e' un `match` grande
//!
//! `dispatch_kernel` conosce il tipo di configurazione di ogni singola
//! operazione, ed e' una duplicazione: all'engine basterebbero la classe di
//! esecuzione, il contratto e la cancellazione. Toglierla richiede una
//! facciata per famiglia che oggi non esiste, e finche' non esiste il `match`
//! resta — ed e' bene che stia in un file proprio, dove si vede quanto e'
//! grande, invece che sepolto in mezzo ad altre cinquemila righe.

use crate::geo_transport::pair::preflight_decoded_bytes;
use crate::geo_transport::transport::{one_to_one_batch_prepared, TransformArrowSchema};
use crate::geo_transport::unary::{
    one_to_one_batch_fused, FusedStepError, FusedTerminal, FusedTerminalMeasure,
};
use crate::governor::{GovernedBatch, MemoryLease, MemoryPermit, ReservationResult};
use crate::planner::{
    check_compatibility, check_declared_input_contracts, local_capabilities, ValidatedGraph,
    ARROW_VERSION, ENGINE_VERSION,
};
use crate::prepare::{
    prepare, ExecutionPlan, MeasureKind, PhysicalSegment, PreparedConfig, PreparedGeoKernel,
    PreparedKernel, PreparedTableKernel, RuntimeContext, SegmentMode,
};
use crate::table_engine;
use crate::temp_store::{scavenge_stale_temp_dirs, TempStore, DEFAULT_SCAVENGE_TTL};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::catalog::CATALOG;
use plenora_core::contract::{BatchSequence, DataContract};
use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::{ErrorPhase, PlenoraError, Result};
use plenora_kernels_geo::arrow_adapter::{batch_geometry_cells, decode_geometry_cell};
use plenora_kernels_geo::operations;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::geo::run_geo_binary_blocking;
use super::geo::{
    append_output_column, geo_accessors_batch, geo_cluster_dbscan_batch, geo_collect_batch,
    geo_coverage_validate_batch, geo_from_wkt_batch, geo_generate_grid_batch,
    geo_line_locate_point_batch, geo_shared_paths_batch, geo_snap_batch, geo_subdivide_batch,
};
#[cfg(test)]
use super::inject_test_panic;
use super::metrics::{accumulate, accumulate_time, sum_rows};
use super::state::ExecState;
use super::validation::{
    check_edge_batch, check_edge_counts, check_expansion, check_join_expansion,
};
use plenora_core::arrow::select::concat::concat_batches;

use super::{
    blocking_output_sequence, check_batch_bytes, geo_binary_step_error, panic_step_error,
    record_kernel_metrics, step_error, GeoBinarySide,
};

/// Un kernel su un batch: confine di panic policy dell'executor
/// (errori-e-limiti.md#panic-policy). Un panic del kernel
/// e' intercettato qui — il livello piu' interno che conserva l'attribuzione
/// di nodo — e convertito in errore `Execution` con il solo messaggio del panic
/// ([`panic_step_error`]); l'errore propaga nello stream, quindi il publish
/// atomico non e' mai raggiunto dopo un panic.
///
/// `AssertUnwindSafe` e' legittimo in questo punto: l'esecuzione v1 e'
/// seriale, batch e config sono proprieta' esclusiva della chiamata (nessuno
/// stato condiviso mutabile attraversa il confine) e l'errore ferma lo
/// stream, quindi un eventuale stato interno del kernel lasciato incoerente
/// dal panic non e' mai riusato. I confini `UnwindSafe` dichiarati per il
/// DAG parallelo valgono soltanto quando esistera' uno scheduler che li
/// attraversi (M3).
pub(super) fn run_kernel(
    kernel: &PreparedKernel,
    batch: RecordBatch,
    state: &ExecState,
) -> Result<RecordBatch> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        inject_test_panic(&kernel.node_id);
        dispatch_kernel(kernel, batch, state)
    })) {
        Ok(result) => result,
        Err(payload) => Err(panic_step_error(kernel, &*payload)),
    }
}

/// Dispatch per famiglia di un kernel su un batch.
///
/// I kernel tabellari unari ricevono la directory di spill condivisa
/// dell'esecuzione (architettura.md#memoria, spill generalizzato): `sort`/`distinct`/`aggregate`
/// sopra la soglia di spill scrivono nel `TempStore` e le loro metriche sono
/// accumulate in [`ExecState`].
///
/// Smista per FAMIGLIA, non per operazione: le quindici forme vivono dentro
/// la famiglia che le possiede, e all'orchestrazione restano le tre cose che
/// la riguardano davvero — classe di esecuzione, contratto, cancellazione.
pub(super) fn dispatch_kernel(
    kernel: &PreparedKernel,
    batch: RecordBatch,
    state: &ExecState,
) -> Result<RecordBatch> {
    match &kernel.config {
        PreparedConfig::Table(tabellare) => tabellare.esegui_batch(kernel, batch, state),
        PreparedConfig::Geo(geometrico) => geometrico.esegui_batch(kernel, &batch, state),
    }
}

impl PreparedTableKernel {
    /// Esegue un batch. Facciata della famiglia tabellare: chi orchestra non
    /// ha bisogno di sapere quali forme esistono qui dentro.
    ///
    /// # Errors
    ///
    /// L'errore del kernel, attribuito al nodo logico.
    fn esegui_batch(
        &self,
        kernel: &PreparedKernel,
        batch: RecordBatch,
        state: &ExecState,
    ) -> Result<RecordBatch> {
        match self {
            Self::Unary(plan) => {
                let (output, spill_metrics) =
                    table_engine::execute_batch_with_spill_row_diagnostics(
                        batch,
                        plan,
                        Some(state.spill_directory()),
                    )
                    .map_err(|error| step_error(kernel, error))?;
                state.add_spill_metrics(spill_metrics);
                Ok(output)
            }
            Self::Binary(_) => Err(PlenoraError::Internal(format!(
                "nodo `{}`: kernel binario in una catena streaming",
                kernel.node_id
            ))),
        }
    }
}

impl PreparedGeoKernel {
    /// Esegue un batch. Facciata della famiglia geometrica.
    ///
    /// Il `match` e' esaustivo su QUESTA famiglia: una variante nuova non
    /// compila finche' qualcuno non decide che cosa farne. E' la garanzia
    /// che un dispatch unico non puo' dare: un enum di quindici varianti
    /// condivise fra due famiglie non dice a quale delle due manca un caso.
    ///
    /// # Errors
    ///
    /// L'errore del kernel, attribuito al nodo logico.
    fn esegui_batch(
        &self,
        kernel: &PreparedKernel,
        batch: &RecordBatch,
        state: &ExecState,
    ) -> Result<RecordBatch> {
        match self {
            Self::Binary(_) => Err(PlenoraError::Internal(format!(
                "nodo `{}`: kernel binario geo in una catena streaming",
                kernel.node_id
            ))),
            Self::Transform(params) => geo_transform_batch(kernel, batch, params, state),
            Self::Measure { measure, .. } => geo_measure_batch(kernel, batch, *measure),
            Self::FromWkt {
                wkt_column_index,
                on_error,
            } => geo_from_wkt_batch(kernel, batch, *wkt_column_index, *on_error),
            Self::Accessors { columns } => geo_accessors_batch(kernel, batch, columns),
            Self::LineLocatePoint {
                point,
                output_column,
            } => geo_line_locate_point_batch(kernel, batch, point, output_column),
            Self::Subdivide { max_vertices } => geo_subdivide_batch(kernel, batch, *max_vertices),
            Self::Snap {
                reference,
                tolerance,
            } => geo_snap_batch(kernel, batch, reference, *tolerance),
            Self::Collect { group_by_indices } => {
                geo_collect_batch(kernel, batch, group_by_indices)
            }
            Self::GenerateGrid {
                extent,
                cell_size,
                shape,
            } => geo_generate_grid_batch(kernel, extent, *cell_size, *shape),
            Self::CoverageValidate {
                tolerance,
                max_issues,
            } => geo_coverage_validate_batch(kernel, batch, *tolerance, *max_issues),
            Self::SharedPaths {
                tolerance,
                min_length,
            } => geo_shared_paths_batch(kernel, batch, *tolerance, *min_length),
            Self::ClusterDbscan {
                eps,
                min_points,
                output_column,
            } => geo_cluster_dbscan_batch(kernel, batch, *eps, *min_points, output_column),
        }
    }
}

/// Trasformazione geo 1:1 in place via `geo_transport` (per batch, senza
/// envelope): i parametri sono tipizzati e risolti da `prepare` (configurazioni preparate);
/// indice di colonna e schema di output arrivano dall'handle prepared del
/// nodo, costruito una volta per esecuzione (hot path minimale).
pub(super) fn geo_transform_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    params: &TransformArrowSchema,
    state: &ExecState,
) -> Result<RecordBatch> {
    let prepared = state.one_to_one_prepared(kernel, &batch.schema(), params)?;
    one_to_one_batch_prepared(batch, params, &prepared).map_err(|error| {
        // R9.9: il trasporto allega la diagnostica row-scoped completa dei
        // fallimenti di cella (indici batch-locali); qui si preserva e il
        // wrapper di segmento la traduce in indici assoluti.
        let base = PlenoraError::InvalidPlan(error.to_string());
        let base = match error.row_diagnostics() {
            Some(diagnostics) => base.with_row_diagnostics(diagnostics.clone()),
            None => base,
        };
        step_error(kernel, base)
    })
}

/// Misura geo "add column" (semantica v4): decodifica le celle WKB non null,
/// applica il kernel scalare e aggiunge la colonna in coda allo schema (il
/// nome e' quello inferito dal planner, risolto in `prepare`).
pub(super) fn geo_measure_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    measure: MeasureKind,
) -> Result<RecordBatch> {
    let geometry_index = kernel.geometry_column_index.ok_or_else(|| {
        step_error(
            kernel,
            PlenoraError::Schema("misura senza colonna geometria".into()),
        )
    })?;
    let geometry_name = kernel.input_contracts[0]
        .active_geometry_column()
        .map_or("geometry", |geometry| geometry.name.as_str());
    let cells = batch_geometry_cells(batch, geometry_index, geometry_name)
        .map_err(|error| step_error(kernel, error))?;
    // R9.9: i fallimenti per riga (decode di un intermedio non conforme o
    // kernel scalare) sono raccolti COMPLETI prima di chiudere — stessa
    // semantica del ramo fuso (`measure_cells`): mai il solo primo errore.
    let mut failures: Vec<(u64, &'static str)> = Vec::new();
    let mut first_error: Option<PlenoraError> = None;
    let mut record = |row: usize, cause: &'static str, error: PlenoraError| {
        failures.push((row as u64, cause));
        if first_error.is_none() {
            first_error = Some(error);
        }
    };
    let column: ArrayRef = match measure {
        MeasureKind::Area | MeasureKind::Length | MeasureKind::Perimeter => {
            let mut values: Vec<Option<f64>> = Vec::with_capacity(batch.num_rows());
            for row in 0..batch.num_rows() {
                if cells.is_null(row) {
                    values.push(None);
                    continue;
                }
                match measure_f64_raw(cells.value(row), measure) {
                    Ok(value) => values.push(Some(value)),
                    Err((cause, error)) => {
                        record(row, cause, error);
                        values.push(None);
                    }
                }
            }
            std::sync::Arc::new(Float64Array::from(values))
        }
        MeasureKind::VertexCount => {
            let mut values: Vec<Option<u64>> = Vec::with_capacity(batch.num_rows());
            for row in 0..batch.num_rows() {
                if cells.is_null(row) {
                    values.push(None);
                    continue;
                }
                let decoded = decode_geometry_cell(cells.value(row))
                    .map_err(|error| ("geometry.invalid_wkb", error))
                    .and_then(|geometry| {
                        operations::vertex_count(&geometry).map_err(|error| {
                            (
                                "geometry.kernel_failed",
                                PlenoraError::InvalidPlan(error.to_string()),
                            )
                        })
                    });
                match decoded {
                    Ok(value) => values.push(Some(value)),
                    Err((cause, error)) => {
                        record(row, cause, error);
                        values.push(None);
                    }
                }
            }
            std::sync::Arc::new(UInt64Array::from(values))
        }
        MeasureKind::ToWkt => {
            let mut values: Vec<Option<String>> = Vec::with_capacity(batch.num_rows());
            for row in 0..batch.num_rows() {
                if cells.is_null(row) {
                    values.push(None);
                    continue;
                }
                let decoded = decode_geometry_cell(cells.value(row))
                    .map_err(|error| ("geometry.invalid_wkb", error))
                    .and_then(|geometry| {
                        operations::to_wkt(&geometry).map_err(|error| {
                            (
                                "geometry.kernel_failed",
                                PlenoraError::InvalidPlan(error.to_string()),
                            )
                        })
                    });
                match decoded {
                    Ok(value) => values.push(Some(value)),
                    Err((cause, error)) => {
                        record(row, cause, error);
                        values.push(None);
                    }
                }
            }
            std::sync::Arc::new(StringArray::from(values))
        }
    };
    if let Some(first) = first_error {
        return Err(step_error(
            kernel,
            first.with_row_diagnostics(measure_row_diagnostics(&failures)),
        ));
    }
    // Lo schema di output e' quello del contratto (input + colonna misura),
    // un clone di Arc condiviso: nessuna ricostruzione per batch (Arrow come rappresentazione unica),
    // stesso percorso degli altri kernel add-column.
    append_output_column(kernel, batch, column)
}

/// Misura scalare su una cella (null gia' gestito dal chiamante): errore
/// grezzo con la causa row-scoped, nella forma del ramo fuso.
pub(super) fn measure_f64_raw(
    payload: &[u8],
    measure: MeasureKind,
) -> std::result::Result<f64, (&'static str, PlenoraError)> {
    let geometry =
        decode_geometry_cell(payload).map_err(|error| ("geometry.invalid_wkb", error))?;
    match measure {
        MeasureKind::Area => operations::area(&geometry),
        MeasureKind::Length => operations::length(&geometry),
        MeasureKind::Perimeter => operations::perimeter(&geometry),
        MeasureKind::VertexCount | MeasureKind::ToWkt => {
            return Err((
                "geometry.kernel_failed",
                PlenoraError::Internal(
                    "misura non f64 nel percorso scalare f64: invariante di dispatch violata"
                        .into(),
                ),
            ));
        }
    }
    .map_err(|error| {
        (
            "geometry.kernel_failed",
            PlenoraError::InvalidPlan(error.to_string()),
        )
    })
}

/// Report `plenora-row-diagnostics-v1` completo (batch-locale) per i
/// fallimenti per riga di una misura geo: stessa forma del ramo fuso e del
/// trasporto (scope Read, esempi bounded, nessun valore).
pub(super) fn measure_row_diagnostics(rows: &[(u64, &'static str)]) -> RowDiagnostics {
    const EXAMPLES_LIMIT: u64 = 10;
    let mut by_row = std::collections::BTreeMap::new();
    for (row, cause) in rows {
        by_row.entry(*row).or_insert(*cause);
    }
    let observed_total = by_row.len() as u64;
    let mut counts = std::collections::BTreeMap::new();
    let mut examples = Vec::new();
    for (row, cause) in &by_row {
        *counts.entry((*cause).to_owned()).or_insert(0_u64) += 1;
        if u64::try_from(examples.len()).unwrap_or(u64::MAX) < EXAMPLES_LIMIT {
            examples.push(RowDiagnosticExample {
                source_index: *row,
                cause: (*cause).to_owned(),
                column: None,
                key: None,
                write_state: None,
            });
        }
    }
    RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness: RowDiagnosticsCompleteness::Complete,
        knowledge_limits: None,
        observed_total,
        total: Some(observed_total),
        input_total: None,
        counts,
        examples_limit: EXAMPLES_LIMIT,
        examples_truncated: observed_total > EXAMPLES_LIMIT,
        examples,
        diagnostic_state_counts: None,
        write_outcome: None,
    }
}

// ---------------------------------------------------------------------------
// Orchestrazione dei segmenti bloccanti
//
// Materializzazione, reservation, smistamento per famiglia. Sono generiche:
// vivono qui e non in `geo.rs` perche' la decisione di dove materializzare
// non e' una questione geometrica: il ramo geo binario e' solo quello con il
// percorso dedicato.
// ---------------------------------------------------------------------------

/// Il kernel di un segmento blocking unario e' spill-capable (architettura.md#memoria,
/// spill generalizzato): `table.sort`/`distinct`/`aggregate` hanno la variante
/// `*_spilled` in kernels-table (cfr. `table_engine::unary_spill_capable`).
pub(super) fn spill_capable_unary(kernel: &PreparedKernel) -> bool {
    kernel.config.unary_spill_capable()
}

/// Segmento blocking unario: input materializzato (previsto dal piano, materializzazione minima),
/// concatenato ed eseguito una sola volta.
///
/// Confine architettura.md#memoria: i lease degli input sono trattenuti durante la
/// concatenazione (i buffer sorgente sono vivi), poi la materializzazione
/// concatenata riceve il suo lease — reservation completa prima di iniziare
/// (categoria "memoria stimabile"), acquisita PRIMA di rilasciare gli input
/// (mai sotto-conteggio al confine) — e l'output riceve a sua volta un
/// lease nuovo. Sequenza riassegnata con la regola documentata in
/// [`blocking_output_sequence`].
pub(super) fn run_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    batches: Vec<GovernedBatch>,
) -> Result<GovernedBatch> {
    // Come nel percorso streaming: heartbeat persistentemente fallito =
    // errore esplicito, mai silenzio (errori-e-limiti.md).
    state.verifica_heartbeat()?;
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().ok_or_else(|| {
        PlenoraError::Internal(
            "segmento blocking senza kernel: invariante del planner violata".into(),
        )
    })?;
    let rows_in = batches.iter().map(|g| g.batch.num_rows()).sum::<usize>() as u64;
    let bytes_in = batches
        .iter()
        .map(GovernedBatch::accounted_bytes)
        .sum::<u64>();
    let schema = kernel.input_contracts[0].schema.clone();
    let full = if batches.is_empty() {
        RecordBatch::new_empty(schema)
    } else {
        let unwrapped: Vec<RecordBatch> = batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    // Il batch concatenato non ha un produttore a monte che ne abbia
    // verificato i byte: il tetto duro in byte per batch si applica anche qui (fail-closed).
    // I byte restituiti alimentano la reservation (hot path minimale: un solo conteggio).
    let full_bytes = check_batch_bytes(state, &full, &kernel.node_id)?;
    // architettura.md#memoria (spill generalizzato): se il kernel spillera' — stessa soglia
    // deterministica valutata al dispatch tabellare (`should_spill_unary`
    // sui byte stimati dell'input), stessi limiti — l'intermedio
    // concatenato NON consuma quota governor: la memoria di lavoro
    // dell'operatore e' auto-limitata dallo spill su disco e la
    // reservation fallirebbe per costruzione (la soglia ha la stessa
    // grandezza del budget). Altrimenti reservation completa
    // dell'intermedio prima di rilasciare i lease degli input (architettura.md#memoria:
    // mai attesa con reservation parziale).
    let spill_path = kernel
        .config
        .table()
        .filter(|tabellare| tabellare.unary_spill_capable())
        .and_then(PreparedTableKernel::unary_plan)
        .is_some_and(|table_plan| {
            plenora_kernels_table::spill::should_spill_unary(&full, table_plan.limits())
        });
    let full_lease = if spill_path {
        None
    } else {
        Some(state.governor.reserve(full_bytes, &kernel.node_id)?)
    };
    drop(batches);
    // errori-e-limiti.md#cancellazione: a fine drenaggio, prima del kernel monolitico
    // (`BoundaryOnly`: check tra kernel/a fine kernel; `NonInterruptible`:
    // mai).
    state.check_cancellation(kernel)?;
    let start = Instant::now();
    let output = run_kernel(kernel, full, state)?;
    let elapsed = start.elapsed();
    // Lease dell'output acquisito prima di rilasciare l'intermedio.
    let output_lease = state
        .governor
        .reserve(output.get_array_memory_size() as u64, &kernel.node_id)?;
    drop(full_lease);
    let rows_out = output.num_rows() as u64;
    state.add_node_rows_out(&kernel.node_id, rows_out);
    check_expansion(state, kernel, rows_in)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    let bytes_out = output_lease.bytes();
    record_kernel_metrics(
        state, segment, kernel, rows_in, rows_out, bytes_in, bytes_out, elapsed, true, true,
    );
    Ok(GovernedBatch::new(
        output,
        Some(output_lease),
        Some(blocking_output_sequence(kernel)),
    ))
}

/// Segmento blocking binario: left e right materializzati, concatenati ed
/// eseguiti una sola volta via `execute_binary`.
///
/// Confine architettura.md#memoria come [`run_blocking`], con reservation multiple in
/// ORDINE GLOBALE FISSO — left prima di right — completa prima di iniziare
/// (mai attesa con reservation parziale; in v1 fail-fast non c'e' attesa,
/// ma l'ordine e' gia' quello richiesto al runtime parallelo M3 per evitare
/// deadlock). Sequenza riassegnata con la regola documentata in
/// [`blocking_output_sequence`] (scansione seriale left-then-right).
// La lunghezza e' data dal guscio architettura.md#memoria completo (concat, reservation,
// metriche) piu' lo smistamento D14.2: sequenza lineare, non complessita'
// logica (stesso criterio di `pair_arrow`).
#[allow(clippy::too_many_lines)]
pub(super) fn run_binary_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    left_batches: Vec<GovernedBatch>,
    right_batches: Vec<GovernedBatch>,
) -> Result<GovernedBatch> {
    // Terzo percorso di esecuzione, e va guardato come gli altri due: un
    // segmento binario emette un solo batch, quindi non esiste
    // necessariamente un confine successivo dove un heartbeat fermo da
    // troppo tempo diventerebbe un errore. Senza questo controllo l'output
    // potrebbe essere pubblicato con il lock ormai stantio.
    state.verifica_heartbeat()?;
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().ok_or_else(|| {
        PlenoraError::Internal(
            "segmento binario senza kernel: invariante del planner violata".into(),
        )
    })?;
    // Smistamento architettura.md#geometrie D14.2 sul `PreparedConfig`: il ramo geo ha il
    // percorso dedicato [`run_geo_binary_blocking`] (stesso guscio, cuore
    // decode → kernel validated → output v4); il ramo tabellare prosegue
    // qui sotto, invariato.
    if let Some(geo_plan) = kernel.config.geo().and_then(PreparedGeoKernel::binary_plan) {
        return run_geo_binary_blocking(
            plan,
            segment_index,
            state,
            geo_plan,
            left_batches,
            right_batches,
        );
    }
    let Some(binary_plan) = kernel
        .config
        .table()
        .and_then(PreparedTableKernel::binary_plan)
    else {
        return Err(PlenoraError::Internal(format!(
            "nodo `{}`: config non binaria in un segmento BinaryBlocking",
            kernel.node_id
        )));
    };
    let left_rows = left_batches
        .iter()
        .map(|g| g.batch.num_rows())
        .sum::<usize>() as u64;
    let right_rows = right_batches
        .iter()
        .map(|g| g.batch.num_rows())
        .sum::<usize>() as u64;
    let bytes_in = left_batches
        .iter()
        .chain(right_batches.iter())
        .map(GovernedBatch::accounted_bytes)
        .sum::<u64>();
    let batches_in = (left_batches.len() + right_batches.len()) as u64;
    let left_schema = kernel.input_contracts[0].schema.clone();
    let right_schema = kernel.input_contracts[1].schema.clone();
    let left = if left_batches.is_empty() {
        RecordBatch::new_empty(left_schema)
    } else {
        let unwrapped: Vec<RecordBatch> = left_batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&left_schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    let right = if right_batches.is_empty() {
        RecordBatch::new_empty(right_schema)
    } else {
        let unwrapped: Vec<RecordBatch> = right_batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&right_schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    // Come per il blocking unario: tetto duro in byte per batch sui batch concatenati; i
    // byte restituiti alimentano le reservation (hot path minimale: un solo conteggio).
    let left_bytes = check_batch_bytes(state, &left, &kernel.node_id)?;
    let right_bytes = check_batch_bytes(state, &right, &kernel.node_id)?;
    // Reservation complete degli intermedi in ordine globale fisso (left,
    // poi right), poi rilascio dei lease degli input.
    let left_lease = state.governor.reserve(left_bytes, &kernel.node_id)?;
    let right_lease = state.governor.reserve(right_bytes, &kernel.node_id)?;
    drop(left_batches);
    drop(right_batches);
    // errori-e-limiti.md#cancellazione: a fine drenaggio, prima del kernel binario monolitico
    // (come `run_blocking`).
    state.check_cancellation(kernel)?;
    let start = Instant::now();
    // Confine di panic policy (errori-e-limiti.md#panic-policy) come
    // `run_kernel`: panic del kernel binario convertito
    // in errore `Execution` attribuito al nodo, mai publish dopo panic.
    let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        inject_test_panic(&kernel.node_id);
        table_engine::execute_binary(&left, &right, binary_plan)
    }))
    .unwrap_or_else(|payload| Err(panic_step_error(kernel, &*payload)))
    .map_err(|error| step_error(kernel, error))?;
    let elapsed = start.elapsed();
    let output_lease = state
        .governor
        .reserve(output.get_array_memory_size() as u64, &kernel.node_id)?;
    drop(left_lease);
    drop(right_lease);
    let rows_out = output.num_rows() as u64;
    state.add_node_rows_out(&kernel.node_id, rows_out);
    // errori-e-limiti.md: per le operazioni binarie il runtime calcola tutte le metriche
    // di espansione e applica il vincolo vincolante dichiarato in catalogo.
    check_join_expansion(state, kernel, left_rows, right_rows, rows_out)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    // Metriche per nodo: righe in = left + right, batch in = quelli reali
    // drenati dai due rami.
    // errori-e-limiti.md: heartbeat del TempStore al punto centrale (come
    // `record_kernel_metrics`, non riusata qui per i conteggi doppi input).
    state.heartbeat();
    let config = state.plan.metrics_config();
    let mut borrowed = state.metrics.borrow_mut();
    let metrics = &mut *borrowed;
    let saturated = &mut metrics.counters_saturated;
    // Metrica obbligatoria errori-e-limiti.md (come in `record_kernel_metrics`). Le righe
    // in ingresso di un nodo binario sono la somma dei due lati: con limiti
    // configurabili fino a `u64::MAX` la somma non e' rappresentabile per
    // costruzione, e non e' una metrica che possa abortire l'esecuzione.
    let rows_in = sum_rows(left_rows, right_rows, saturated);
    accumulate(&mut metrics.total_rows_processed, rows_in, saturated);
    if config.per_node {
        if let Some(node) = metrics.nodes.get_mut(&kernel.node_id) {
            accumulate(&mut node.rows_in, rows_in, saturated);
            accumulate(&mut node.rows_out, rows_out, saturated);
            accumulate(&mut node.batches_in, batches_in, saturated);
            accumulate(&mut node.batches_out, 1, saturated);
            accumulate(&mut node.bytes_in, bytes_in, saturated);
            accumulate(&mut node.bytes_out, output_lease.bytes(), saturated);
            accumulate_time(&mut node.wall_time, elapsed, saturated);
        }
    }
    if config.per_segment {
        if let Some(seg) = metrics.segments.get_mut(&segment.id) {
            accumulate(&mut seg.rows_in, rows_in, saturated);
            accumulate(&mut seg.rows_out, rows_out, saturated);
            accumulate(&mut seg.batches_in, batches_in, saturated);
            accumulate(&mut seg.batches_out, 1, saturated);
            accumulate_time(&mut seg.wall_time, elapsed, saturated);
        }
    }
    Ok(GovernedBatch::new(
        output,
        Some(output_lease),
        Some(blocking_output_sequence(kernel)),
    ))
}
