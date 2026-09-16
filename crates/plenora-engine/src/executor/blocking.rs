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
    causa_di_riga, collect_measure_failures, esito_kernel, one_to_one_batch_fused, FusedStepError,
    FusedTerminal, FusedTerminalMeasure,
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
use geo::Geometry;
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::catalog::CATALOG;
use plenora_core::contract::{BatchSequence, DataContract};
use plenora_core::{ErrorPhase, PlenoraError, Result};
use plenora_kernels_geo::arrow_adapter::{batch_geometry_cells, decode_geometry_cell};
use plenora_kernels_geo::operations::{self, OperationError};
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
        //
        // L'attribuzione la decide la variante, non il chiamante: un errore
        // gia' interno sotto non diventa una colpa del piano.
        let base = error.errore_del_passo();
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
    let column: std::result::Result<ArrayRef, PlenoraError> = match measure {
        MeasureKind::Area | MeasureKind::Length | MeasureKind::Perimeter => misura_colonna(
            batch.num_rows(),
            |row| cells.is_null(row),
            |row| measure_f64_raw(cells.value(row), measure),
        )
        .map(|values| std::sync::Arc::new(Float64Array::from(values)) as ArrayRef),
        MeasureKind::VertexCount => misura_colonna(
            batch.num_rows(),
            |row| cells.is_null(row),
            |row| misura_riga(cells.value(row), operations::vertex_count),
        )
        .map(|values| std::sync::Arc::new(UInt64Array::from(values)) as ArrayRef),
        MeasureKind::ToWkt => misura_colonna(
            batch.num_rows(),
            |row| cells.is_null(row),
            |row| misura_riga(cells.value(row), operations::to_wkt),
        )
        .map(|values| std::sync::Arc::new(StringArray::from(values)) as ArrayRef),
    };
    let column = column.map_err(|error| step_error(kernel, error))?;
    // Lo schema di output e' quello del contratto (input + colonna misura),
    // un clone di Arc condiviso: nessuna ricostruzione per batch (Arrow come rappresentazione unica),
    // stesso percorso degli altri kernel add-column.
    append_output_column(kernel, batch, column)
}

/// Applica un kernel di riga a tutte le celle del batch (le nulle passano
/// come `None`), raccogliendo i fallimenti COMPLETI prima di chiudere —
/// stessa semantica del ramo fuso (`measure_cells`): mai il solo primo
/// errore. Chiude con [`collect_measure_failures`], **condivisa** col ramo
/// fuso invece di duplicata: precedenza (un'interruzione vince su qualunque
/// ordinario e propaga bare, senza diagnostica) e raccolta sono la stessa
/// funzione sui due percorsi, non due copie da tenere allineate a mano.
fn misura_colonna<T>(
    num_rows: usize,
    is_null: impl Fn(usize) -> bool,
    per_row: impl Fn(usize) -> std::result::Result<T, (Option<&'static str>, PlenoraError)>,
) -> std::result::Result<Vec<Option<T>>, PlenoraError> {
    let mut failures = Vec::new();
    let mut values = Vec::with_capacity(num_rows);
    for row in 0..num_rows {
        if is_null(row) {
            values.push(None);
            continue;
        }
        match per_row(row) {
            Ok(value) => values.push(Some(value)),
            Err((cause, error)) => {
                failures.push((row as u64, cause, error));
                values.push(None);
            }
        }
    }
    if failures.is_empty() {
        return Ok(values);
    }
    Err(collect_measure_failures(failures))
}

/// Decodifica una cella e applica un kernel scalare, con la stessa
/// classificazione di causa del ramo fuso (`measure_cells`): `None` per una
/// validazione interrotta, sia alla decodifica sia dentro il kernel — mai
/// attribuibile alla riga, indipendentemente dal sito.
fn misura_riga<T>(
    payload: &[u8],
    kernel: impl FnOnce(&Geometry<f64>) -> std::result::Result<T, OperationError>,
) -> std::result::Result<T, (Option<&'static str>, PlenoraError)> {
    let geometry = decode_geometry_cell(payload)
        .map_err(|error| (causa_di_riga(&error, "geometry.invalid_wkb"), error))?;
    kernel(&geometry).map_err(|error| {
        let error = esito_kernel(error);
        (causa_di_riga(&error, "geometry.kernel_failed"), error)
    })
}

pub(super) fn measure_f64_raw(
    payload: &[u8],
    measure: MeasureKind,
) -> std::result::Result<f64, (Option<&'static str>, PlenoraError)> {
    match measure {
        MeasureKind::Area => misura_riga(payload, operations::area),
        MeasureKind::Length => misura_riga(payload, operations::length),
        MeasureKind::Perimeter => misura_riga(payload, operations::perimeter),
        MeasureKind::VertexCount | MeasureKind::ToWkt => {
            let error = PlenoraError::Internal(
                "misura non f64 nel percorso scalare f64: invariante di dispatch violata".into(),
            );
            Err((causa_di_riga(&error, "geometry.kernel_failed"), error))
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{LineString, Point, Polygon};
    use geozero::{CoordDimensions, ToWkb};

    fn wkb_valido() -> Vec<u8> {
        Geometry::Point(Point::new(1.0, 2.0))
            .to_wkb(CoordDimensions::xy())
            .expect("fixture wkb")
    }

    /// **Prova del componente `misura_riga`**: non della raccolta
    /// (`misura_colonna`/`collect_measure_failures`), non dell'executor —
    /// una sola cella, un solo kernel sintetico.
    ///
    /// WKB ordinario e valido: la decodifica riesce, il kernel sintetico e'
    /// l'unico a fallire. Restituisce direttamente
    /// `OperationError::ValidazioneNonConclusa`: nessun panico qui, la
    /// barriera e la sua conversione sono provate separatamente (prove
    /// 1/2/3 sul contenimento, tuttora in parte da progettare).
    #[test]
    fn misura_riga_valida_non_conclusa_diventa_internal_senza_causa() {
        let payload = wkb_valido();
        let risultato = misura_riga(
            &payload,
            |_g| -> std::result::Result<String, OperationError> {
                Err(OperationError::ValidazioneNonConclusa("prova"))
            },
        );
        let Err((cause, error)) = risultato else {
            panic!("atteso un fallimento")
        };
        assert_eq!(
            cause, None,
            "una validazione interrotta non e' attribuibile alla riga"
        );
        assert_eq!(error.category(), plenora_core::ErrorCategory::Internal);
        assert_eq!(
            error.to_string(),
            "internal error: validazione OGC non conclusa: prova (contenuto non pubblicato)"
        );
    }

    /// Prova del componente: un fallimento ordinario del kernel resta nella
    /// forma di prima della correzione — categoria e testo invariati,
    /// causa `geometry.kernel_failed`. Non e' cambiato da questa
    /// correzione, ed e' il caso che dimostra che non lo e'.
    #[test]
    fn misura_riga_errore_ordinario_resta_invalidplan_con_causa_kernel() {
        let payload = wkb_valido();
        let risultato = misura_riga(
            &payload,
            |_g| -> std::result::Result<String, OperationError> {
                Err(OperationError::InvalidInput(
                    "anello con auto-intersezione".to_owned(),
                ))
            },
        );
        let Err((cause, error)) = risultato else {
            panic!("atteso un fallimento")
        };
        assert_eq!(cause, Some("geometry.kernel_failed"));
        assert_eq!(error.category(), plenora_core::ErrorCategory::InvalidPlan);
        assert_eq!(
            error.to_string(),
            "contract violation: geometria di input non valida: anello con auto-intersezione"
        );
    }

    /// Prova del componente: se `to_wkt` rende `WktSerialization` (l'anello
    /// interno senza esterno della migrazione WKT v2, diff 3/4 del
    /// candidato memory-lab), la classificazione resta quella ordinaria —
    /// stessa forma della prova sopra con `InvalidInput`. Sintetico di
    /// proposito: il decoder WKB di questo prodotto rifiuta QUALUNQUE
    /// anello (compreso l'esterno) sotto le quattro coordinate —
    /// `geometry_contract.rs::check_ring` — quindi un payload che arrivi da
    /// WKB non puo' mai portare l'esterno vuoto che innesca il guardiano di
    /// `wkt`: la prova sotto (`decode_geometry_cell_rifiuta_...`) lo
    /// verifica. Qui si prova solo che, SE il kernel rendesse comunque
    /// quell'errore — da un ingresso costruito altrove, non da WKB — la
    /// classificazione sarebbe corretta.
    #[test]
    fn misura_riga_wkt_serialization_resta_invalidplan_con_causa_kernel() {
        let payload = wkb_valido();
        let risultato = misura_riga(
            &payload,
            |_g| -> std::result::Result<String, OperationError> {
                Err(OperationError::WktSerialization(
                    "geometria non serializzabile".to_owned(),
                ))
            },
        );
        let Err((cause, error)) = risultato else {
            panic!("atteso un fallimento")
        };
        assert_eq!(cause, Some("geometry.kernel_failed"));
        assert_eq!(error.category(), plenora_core::ErrorCategory::InvalidPlan);
        assert_eq!(
            error.to_string(),
            "contract violation: serializzazione WKT fallita: geometria non serializzabile"
        );
    }

    /// La ragione per cui la prova sopra e' sintetica: un poligono vuoto
    /// (con o senza interni) non decodifica affatto in questo prodotto —
    /// `check_polygon` pretende l'esterno sempre presente e con almeno
    /// quattro coordinate, un vincolo piu' stretto della sola validita' OGC
    /// (che accetterebbe il vuoto) e indipendente dalla migrazione WKT: il
    /// reperto storico del laboratorio non attraversa il confine WKB di
    /// QUESTO prodotto, a prescindere da `to_wkt`.
    #[test]
    fn decode_geometry_cell_rifiuta_poligono_vuoto_prima_che_un_kernel_lo_veda() {
        let vuoto = Polygon::new(LineString::from(Vec::<(f64, f64)>::new()), Vec::new());
        let geometria = Geometry::MultiPolygon(geo::MultiPolygon::new(vec![vuoto]));
        let payload = geometria
            .to_wkb(CoordDimensions::xy())
            .expect("fixture wkb per il componente vuoto");

        let errore = decode_geometry_cell(&payload).expect_err("il vuoto non decodifica");
        assert!(
            errore.to_string().contains("meno di quattro coordinate"),
            "atteso il rifiuto del decoder sull'anello vuoto, ottenuto: {errore}"
        );
    }

    /// Prova del componente: se la decodifica fallisce, il kernel non deve
    /// mai essere invocato — verificato per fatto osservato (un contatore),
    /// non per assunzione sul cortocircuito di `?`.
    #[test]
    fn misura_riga_errore_di_decodifica_non_invoca_il_kernel() {
        let chiamato = std::cell::Cell::new(false);
        let risultato = misura_riga(
            b"non e' un wkb valido",
            |_g| -> std::result::Result<String, OperationError> {
                chiamato.set(true);
                Ok("non deve arrivare qui".to_owned())
            },
        );
        assert!(
            !chiamato.get(),
            "il kernel non deve essere invocato se la decodifica fallisce"
        );
        let Err((cause, error)) = risultato else {
            panic!("atteso un fallimento di decodifica")
        };
        assert_eq!(
            cause,
            Some("geometry.invalid_wkb"),
            "un errore di decodifica ordinario resta attribuibile alla riga"
        );
        assert_eq!(error.category(), plenora_core::ErrorCategory::InvalidPlan);
    }

    // Prove di `misura_colonna`: la precedenza nella raccolta non fusa va
    // sorvegliata qui, con gli stessi scenari misti gia' usati per
    // `measure_cells` sul percorso fuso — un test su `misura_riga` non
    // attraversa `collect_measure_failures`, e questi si'. La selezione
    // della riga qui e' per indice esplicito (parametro di `per_row`), non
    // per identita' di indirizzo: `misura_colonna` non manipola geometrie,
    // solo indici di riga. La raccolta e' condivisa col percorso fuso
    // (`collect_measure_failures`): una divergenza tra le due la vedrebbero
    // entrambe le batterie di test, non una sola.

    /// Un fallimento ordinario a riga 0, uno interrotto a riga 1: vince
    /// comunque l'interruzione.
    #[test]
    fn misura_colonna_ordinario_poi_interrotta_non_vince_l_ordinario() {
        let risultato = misura_colonna::<()>(
            2,
            |_row| false,
            |row| {
                if row == 0 {
                    Err((
                        Some("geometry.kernel_failed"),
                        PlenoraError::InvalidPlan("anello con auto-intersezione".to_owned()),
                    ))
                } else {
                    Err((None, PlenoraError::Internal("prova".to_owned())))
                }
            },
        );
        let error = risultato.expect_err("deve fallire");
        assert_eq!(error.category(), plenora_core::ErrorCategory::Internal);
        assert!(error.row_diagnostics().is_none());
    }

    /// Meta' simmetrica: interrotto a riga 0, ordinario a riga 1 — stesso
    /// esito, per escludere una dipendenza dall'ordine di riga.
    #[test]
    fn misura_colonna_interrotta_poi_ordinario_non_vince_l_ordinario() {
        let risultato = misura_colonna::<()>(
            2,
            |_row| false,
            |row| {
                if row == 0 {
                    Err((None, PlenoraError::Internal("prova".to_owned())))
                } else {
                    Err((
                        Some("geometry.kernel_failed"),
                        PlenoraError::InvalidPlan("anello con auto-intersezione".to_owned()),
                    ))
                }
            },
        );
        let error = risultato.expect_err("deve fallire");
        assert_eq!(error.category(), plenora_core::ErrorCategory::Internal);
        assert!(error.row_diagnostics().is_none());
    }

    /// Due interruzioni distinguibili: vince quella di riga minore.
    #[test]
    fn misura_colonna_due_interruzioni_sceglie_quella_in_ordine_logico_minore() {
        let risultato = misura_colonna::<()>(
            2,
            |_row| false,
            |row| {
                Err((
                    None,
                    PlenoraError::Internal(format!("marcatore-riga-{row}")),
                ))
            },
        );
        let error = risultato.expect_err("deve fallire");
        assert_eq!(error.category(), plenora_core::ErrorCategory::Internal);
        assert!(error.row_diagnostics().is_none());
        let testo = error.to_string();
        assert!(testo.contains("marcatore-riga-0"), "trovato: {testo}");
        assert!(!testo.contains("marcatore-riga-1"), "trovato: {testo}");
    }

    /// Soli errori ordinari: diagnostica di riga completa e ordinata,
    /// comportamento invariato rispetto a prima della correzione.
    #[test]
    fn misura_colonna_soli_errori_ordinari_mantiene_diagnostica_completa_e_ordinata() {
        let risultato = misura_colonna::<()>(
            3,
            |row| row == 0,
            |_row| {
                Err((
                    Some("geometry.kernel_failed"),
                    PlenoraError::InvalidPlan("anello con auto-intersezione".to_owned()),
                ))
            },
        );
        let error = risultato.expect_err("deve fallire");
        assert_eq!(error.category(), plenora_core::ErrorCategory::InvalidPlan);
        let diagnostics = error
            .row_diagnostics()
            .expect("errore ordinario, diagnostica di riga attesa");
        assert_eq!(diagnostics.observed_total, 2);
        assert_eq!(diagnostics.counts.get("geometry.kernel_failed"), Some(&2));
        let indici: Vec<u64> = diagnostics
            .examples
            .iter()
            .map(|esempio| esempio.source_index)
            .collect();
        assert_eq!(indici, vec![1, 2]);
    }

    /// **Riga valida seguita da errore, sul ciclo di raccolta REALE
    /// (`misura_colonna`), non su una sola cella.** Riga 0 calcola un WKT
    /// vero (non un segnaposto: una `assert_eq!` su un valore diverso da
    /// quello reale del kernel la vedrebbe), riga 1 fallisce con
    /// `WktSerialization`. Sintetico di proposito, come le prove gemelle su
    /// `misura_riga`/`measure_cells`: nessun payload WKB porta l'esterno
    /// vuoto che innesca quell'errore (vedi
    /// `decode_geometry_cell_rifiuta_...`), tenuta separata dal rifiuto
    /// strutturale reale — qui si prova la raccolta multi-riga, non la
    /// raggiungibilita'.
    #[test]
    fn misura_colonna_riga_valida_poi_wkt_serialization_non_pubblica_nulla() {
        let valore_riga_0 = operations::to_wkt(&Geometry::Point(Point::new(1.0, 2.0)))
            .expect("il kernel reale calcola un WKT vero per la riga 0");
        let risultato = misura_colonna(
            2,
            |_row| false,
            |row| {
                if row == 0 {
                    Ok(valore_riga_0.clone())
                } else {
                    Err((
                        Some("geometry.kernel_failed"),
                        PlenoraError::InvalidPlan(
                            "serializzazione WKT fallita: geometria non serializzabile".to_owned(),
                        ),
                    ))
                }
            },
        );
        let errore = risultato
            .expect_err("una sola riga fallita deve rendere Err, mai un vettore con la riga 0");
        assert_eq!(errore.category(), plenora_core::ErrorCategory::InvalidPlan);
        assert_eq!(
            errore.to_string(),
            "contract violation: serializzazione WKT fallita: geometria non serializzabile"
        );
        let diagnostics = errore
            .row_diagnostics()
            .expect("errore ordinario, diagnostica di riga attesa");
        assert_eq!(diagnostics.observed_total, 1);
        assert_eq!(diagnostics.counts.get("geometry.kernel_failed"), Some(&1));
        let indici: Vec<u64> = diagnostics
            .examples
            .iter()
            .map(|esempio| esempio.source_index)
            .collect();
        assert_eq!(indici, vec![1], "solo la riga 1 e' fallita, non la 0");
    }
}
