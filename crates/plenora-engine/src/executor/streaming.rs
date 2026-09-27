//! La catena streaming: batch dentro, batch fuori, senza materializzare.
//!
//! Ogni kernel 1:1 riceve un batch e ne produce uno, come una pipeline di
//! iteratori pigri: la memoria non cresce con l'input. Un nodo blocking
//! interrompe la catena ([`super::blocking`]).

use crate::governor::{GovernedBatch, MemoryLease, MemoryPermit};
use crate::prepare::ExecutionPlan;
use plenora_core::Result;
use std::rc::Rc;
use std::time::Instant;

use super::state::ExecState;
use super::validation::{check_edge_batch, check_expansion};
use super::{fusion_group_len, record_kernel_metrics, run_kernel, try_run_fused_group};

/// Catena streaming: il batch attraversa i kernel in sequenza senza
/// materializzazione; limiti per arco ed espansione dopo ogni kernel.
///
/// Confine architettura.md#memoria e #determinismo: il lease NUOVO
/// dell'output e' acquisito PRIMA di rilasciare quello di input (il picco
/// reale del kernel e' input+output), e la sequenza si propaga 1:1, esatta
/// perche' ogni kernel streaming e' batch-in/batch-out, espansioni 1:N come
/// `geo.subdivide` comprese.
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
    // Quota dell'output acquisita prima di rilasciare l'input
    // (architettura.md#memoria). Con un permesso l'output si RITAGLIA da
    // quello, che e' un maggiorante (`max_batch_bytes`); un ritaglio fallito
    // e' un'invariante rotta e si propaga, senza ripiegare su una nuova
    // prenotazione che riaprirebbe la finestra chiusa dal permesso.
    let output_lease = match permesso {
        Some(permesso) => permesso.ritaglia(bytes_at_boundary)?,
        None => state
            .governor
            .reserve(bytes_at_boundary, &segment.output_edge)?,
    };
    drop(input_lease);
    Ok(GovernedBatch::new(batch, Some(output_lease), seq))
}
