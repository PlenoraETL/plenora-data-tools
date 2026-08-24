//! La catena streaming: batch dentro, batch fuori, senza materializzare.
//!
//! E' il percorso normale. Ogni kernel 1:1 riceve un batch e ne produce uno,
//! e la catena si compone come una pipeline di iteratori pigri: nessun nodo
//! trattiene piu' di un batch alla volta, e la memoria non cresce con la
//! dimensione dell'input.
//!
//! Quando un nodo della catena e' blocking, la catena si interrompe li': il
//! percorso bloccante e' in [`super::blocking`].

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
    prepare, ExecutionPlan, MeasureKind, PhysicalSegment, PreparedConfig, PreparedKernel,
    RuntimeContext, SegmentMode,
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

use super::state::ExecState;
use super::validation::{
    cancellation_behavior, check_edge_batch, check_edge_counts, check_expansion,
};
use super::{
    check_batch_bytes, fused_group_terminal, fusion_group_len, record_kernel_metrics, run_kernel,
    step_error, try_run_fused_group,
};

/// Catena streaming (segmenti lineari senza code): il batch attraversa i kernel in sequenza senza
/// materializzazione; limiti per arco ed espansione dopo ogni kernel.
///
/// Confine architettura.md#memoria e #determinismo: il wrapper si spacca in ingresso (i kernel
/// restano su `RecordBatch` puro) e si ricompone in uscita — lease NUOVO
/// sui byte dell'output, acquisito PRIMA di rilasciare quello di input (mai
/// sotto-conteggio al confine: il picco reale del kernel e' input+output),
/// e sequenza propagata 1:1. Ogni kernel streaming della v1 e'
/// batch-in/batch-out — anche le espansioni 1:N per batch come
/// `geo.subdivide` — quindi la propagazione 1:1 e' esatta a granularita' di
/// batch.
pub(super) fn run_streaming_chain(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    governed: GovernedBatch,
    stop_after_node: Option<&str>,
    permesso: Option<MemoryPermit>,
) -> Result<GovernedBatch> {
    // Ogni batch passa da qui: e' il punto in cui un heartbeat che fallisce
    // da troppo tempo smette di essere tollerato (errori-e-limiti.md).
    state.verifica_heartbeat()?;
    let segment = &plan.segments()[segment_index];
    let output_is_plan_output = segment.output_edge == plan.output_edge();
    let (mut batch, input_lease, seq) = governed.into_parts();
    let per_node = state.plan.metrics_config().per_node;
    // Byte al confine di kernel: il lease di ingresso li fornisce gratis
    // (nessun riconteggio); i confini interni sono stimati sui metadati dei
    // buffer solo se le metriche per nodo sono attive — piu' il confine
    // finale, che serve comunque al lease dell'output.
    let mut bytes_at_boundary = input_lease.as_ref().map_or_else(
        || {
            if per_node {
                batch.get_array_memory_size() as u64
            } else {
                0
            }
        },
        MemoryLease::bytes,
    );
    let kernels = &segment.kernels;
    // Diagnostica opt-in (errori arricchiti): la sequenza logica e' contesto strutturale
    // (indice di batch), mai un valore. Formattata solo a flag attivo (hot path minimale):
    // `with_diagnostics` la ignorerebbe comunque a diagnostica spenta.
    let batch_detail = if state.diagnostics {
        seq.as_ref()
            .map(|seq| format!("batch_seq={}", seq.sequence_number))
    } else {
        None
    };
    let mut position = 0_usize;
    let mut stopped_early = false;
    while position < kernels.len() {
        // architettura.md#geometrie: se il kernel apre un gruppo di fusione geo (>= 2 membri)
        // il gruppo e' eseguito col runner fuso su QUESTO batch; a
        // reservation governor fallita si ricade sul percorso non fuso per
        // il batch (D12.7, fallback strumentato) e il loop standard
        // processa i kernel uno a uno.
        let group_len = fusion_group_len(kernels, position);
        if stop_after_node.is_none()
            && group_len > 1
            && try_run_fused_group(
                segment,
                state,
                &mut batch,
                position,
                group_len,
                &mut bytes_at_boundary,
                batch_detail.as_deref(),
                output_is_plan_output,
            )?
        {
            position += group_len;
            continue;
        }
        let kernel = &kernels[position];
        // errori-e-limiti.md#cancellazione: check cooperativo al confine di kernel — per il primo
        // kernel della catena e' anche il check "tra batch"; onora il
        // `CancellationBehavior` di catalogo (`NonInterruptible`: mai).
        state.check_cancellation(kernel)?;
        let rows_in = batch.num_rows() as u64;
        let bytes_in = bytes_at_boundary;
        let start = Instant::now();
        batch = run_kernel(kernel, batch, state)
            .map_err(|error| state.with_diagnostics(error, batch_detail.as_deref()))?;
        let elapsed = start.elapsed();
        let rows_out = batch.num_rows() as u64;
        let is_last = position + 1 == kernels.len();
        bytes_at_boundary = if per_node || is_last {
            batch.get_array_memory_size() as u64
        } else {
            0
        };
        state.add_node_rows_out(&kernel.node_id, rows_out);
        check_expansion(state, kernel, rows_in)?;
        // Limiti d'arco sugli archi interni e sull'arco di uscita del
        // segmento, a meno che non sia l'output del piano (li valgono
        // max_output_rows e il wrapper di output).
        if !(is_last && output_is_plan_output) {
            check_edge_batch(state, &kernel.node_id, &batch)?;
        }
        record_kernel_metrics(
            state,
            segment,
            kernel,
            rows_in,
            rows_out,
            bytes_in,
            bytes_at_boundary,
            elapsed,
            position == 0,
            is_last,
        );
        if stop_after_node == Some(kernel.node_id.as_str()) {
            stopped_early = true;
            break;
        }
        position += 1;
    }
    if stopped_early {
        drop(input_lease);
        return Ok(GovernedBatch::new(batch, None, seq));
    }
    // Ricomposizione: quota dell'output acquisita prima di rilasciare
    // l'input (mai sotto-conteggio al confine, architettura.md#memoria).
    //
    // Se il chiamante ha gia' un permesso, l'output si RITAGLIA da quello:
    // la quota e' gia' sua, e riprenotarla aprirebbe la finestra che il
    // permesso esiste per chiudere. Il permesso e' un maggiorante
    // (`max_batch_bytes`, che il wrapper d'uscita applica a ogni batch di
    // output), quindi il ritaglio riesce per ogni output che il piano
    // potrebbe pubblicare.
    //
    // **Nessun ripiego su una nuova prenotazione.** Un ritaglio fallito
    // significa che il maggiorante era sbagliato, cioe' un'invariante nostra
    // rotta: rilasciare e riprenotare la nasconderebbe e reintrodurrebbe
    // proprio la finestra che il permesso esiste per chiudere. Si propaga
    // l'errore.
    let output_lease = match permesso {
        Some(permesso) => permesso.ritaglia(bytes_at_boundary)?,
        None => state
            .governor
            .reserve(bytes_at_boundary, &segment.output_edge)?,
    };
    drop(input_lease);
    Ok(GovernedBatch::new(batch, Some(output_lease), seq))
}
