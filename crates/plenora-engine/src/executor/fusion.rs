//! Fusione dei gruppi geo: piu' kernel, una decodifica sola.
//!
//! Decodificare il WKB e' il costo dominante di una catena geo. Se piu'
//! operazioni consecutive lavorano sulla stessa geometria, decodificarla una
//! volta e riusarla vale piu' di qualunque altra ottimizzazione del percorso.
//!
//! # Il fallback non e' mai silenzioso
//!
//! Se il governor rifiuta la reservation per la memoria decodificata, il
//! gruppo ricade sul percorso non fuso — stesso risultato, scelta fisica
//! diversa — e il fatto viene CONTATO in `geo_fusion_fallbacks` (decisione
//! D12.7). Una pressione di memoria ricorrente diventa cosi' un numero
//! osservabile invece di un rallentamento inspiegabile.

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

#[cfg(test)]
use super::inject_test_panic;
use super::metrics::{accumulate, accumulate_time};
use super::state::ExecState;
use super::validation::{check_edge_batch, check_edge_counts, check_expansion};
use super::{
    check_batch_bytes, geo_binary_step_error, panic_step_error, record_kernel_metrics, step_error,
};

/// Lunghezza del gruppo di fusione geo che si apre a `position` (0 se il
/// kernel non apre un gruppo, architettura.md#geometrie): i membri condividono l'id assegnato
/// in `prepare` e l'apertura e' il primo membro del run.
pub(super) fn fusion_group_len(kernels: &[PreparedKernel], position: usize) -> usize {
    let Some(group) = kernels[position].fusion_group else {
        return 0;
    };
    if position > 0 && kernels[position - 1].fusion_group == Some(group) {
        return 0;
    }
    let mut len = 1_usize;
    while position + len < kernels.len() && kernels[position + len].fusion_group == Some(group) {
        len += 1;
    }
    len
}

/// Stima iniziale dei byte decodificati del gruppo sul batch (architettura.md#geometrie
/// D12.7): somma ESATTA dei payload WKB non null della colonna geometria —
/// camminata sui payload di input, perche' la forma decodificata transiente
/// non esiste ancora. `None` se la colonna non e' Binary: il percorso non
/// fuso produce l'errore esatto al primo kernel.
pub(super) fn fused_group_decoded_bytes(
    batch: &RecordBatch,
    kernel: &PreparedKernel,
) -> Option<u64> {
    let index = kernel.geometry_column_index?;
    let cells = batch.column(index).as_any().downcast_ref::<BinaryArray>()?;
    let mut total = 0_u64;
    for cell in cells.iter().flatten() {
        total = total.saturating_add(cell.len() as u64);
    }
    Some(total)
}

/// Contesto di un tentativo fuso su un batch (architettura.md#geometrie): argomenti condivisi
/// della contabilita' per kernel tra i percorsi di successo e di fallimento.
pub(super) struct FusedAttempt<'a> {
    state: &'a ExecState,
    segment: &'a PhysicalSegment,
    start: usize,
    group_len: usize,
    rows: u64,
    bytes_in: u64,
    output_is_plan_output: bool,
    /// Istanti di inizio passo (wall time per kernel): il primo copre
    /// decode + kernel 0, come lo `start` di `run_kernel` nel loop non fuso.
    edges: RefCell<Vec<Instant>>,
}

impl FusedAttempt<'_> {
    /// Contabilita' dei kernel COMPLETATI del gruppo (architettura.md#geometrie D12.6):
    /// stessa sequenza del loop non fuso — righe per nodo, espansione,
    /// limiti d'arco, metriche per kernel — per i primi `completed` kernel.
    /// Sugli archi interni fusi il batch non e' materializzato: conteggi
    /// righe/batch esatti (1:1), niente tetto byte (D12.8, deroga errori-e-limiti.md#limiti-dichiarati);
    /// il batch materiale esiste solo a gruppo completato (ultimo kernel).
    /// I byte ai confini interni fusi sono zero: i buffer Arrow intermedi
    /// non esistono (metriche per nodo, non reservation).
    ///
    /// # Errors
    ///
    /// Come i check del loop non fuso (`max_expansion_factor`, limiti
    /// d'arco): un loro fallimento precede l'errore del kernel in corso,
    /// esattamente come nel percorso non fuso.
    fn account(
        &self,
        completed: usize,
        output: Option<(&RecordBatch, u64)>,
        finished: Instant,
    ) -> Result<()> {
        let edges = self.edges.borrow();
        for index in 0..completed {
            let position = self.start + index;
            let kernel = &self.segment.kernels[position];
            let is_last = position + 1 == self.segment.kernels.len();
            let kernel_bytes_in = if index == 0 { self.bytes_in } else { 0 };
            let (kernel_bytes_out, materialized) = if index + 1 == self.group_len {
                match output {
                    Some((batch, bytes)) => (bytes, Some(batch)),
                    None => (0, None),
                }
            } else {
                (0, None)
            };
            let elapsed = edges.get(index + 1).map_or_else(
                || finished.saturating_duration_since(edges[index]),
                |next| next.saturating_duration_since(edges[index]),
            );
            self.state.add_node_rows_out(&kernel.node_id, self.rows);
            check_expansion(self.state, kernel, self.rows)?;
            match materialized {
                Some(batch) => {
                    if !(is_last && self.output_is_plan_output) {
                        check_edge_batch(self.state, &kernel.node_id, batch)?;
                    }
                }
                None => {
                    // Arco interno fuso (D12.8/errori-e-limiti.md#limiti-dichiarati): conteggi esatti 1:1,
                    // il tetto byte e' coperto dal governor (D12.7).
                    check_edge_counts(self.state, &kernel.node_id, self.rows)?;
                }
            }
            record_kernel_metrics(
                self.state,
                self.segment,
                kernel,
                self.rows,
                self.rows,
                kernel_bytes_in,
                kernel_bytes_out,
                elapsed,
                position == 0,
                is_last,
            );
        }
        Ok(())
    }
}

/// Tentativo di esecuzione FUSA di un gruppo geo su un batch (architettura.md#geometrie):
/// reservation dei byte decodificati sul governor (D12.7) e, a concessione
/// avvenuta, runner fuso con un solo decode/encode. Restituisce `true` se il
/// gruppo e' stato eseguito (batch e byte al confine aggiornati); `false` se
/// si e' ricaduti sul percorso non fuso per QUESTO batch — reservation
/// fallita, metrica dedicata registrata, batch invariato: nessun errore
/// nuovo, il loop standard produce l'esito identico.
///
/// Un gruppo e' un run di trasformazioni `TransformInPlace` (>= 1) piu' UNA
/// misura terminale opzionale in coda (capability `TerminalMeasure`):
/// la misura consuma la forma decodificata dell'ultimo passo e appende la
/// colonna scalare (semantica v4 "add column" — la colonna geometria
/// sopravvive e viene ri-encodata una sola volta al confine). La reservation
/// D12.7 non cambia: copre i byte decodificati della colonna geometria e
/// l'output scalare e' nel lease di uscita del segmento, come senza misura.
///
/// Errori e osservabilita' per nodo (D12.6): righe 1:1 e metriche per ogni
/// kernel del gruppo, `check_cancellation` tra un kernel e l'altro (errore
/// `Cancelled` attribuito al kernel in corso, come il check del loop),
/// errori di cella via `step_error` al kernel che li ha prodotti (tabella di
/// attribuzione del runner fuso), `catch_unwind` sul gruppo con attribuzione
/// al kernel in corso (stesso pattern di `run_kernel`).
///
/// # Errors
///
/// Come il loop non fuso per i kernel del gruppo (limiti, errori di cella,
/// cancellazione, panic convertito), piu' `PlenoraError::Internal` se un
/// kernel non misura del gruppo non e' `GeoTransform` (invariante di
/// `prepare`).
#[allow(clippy::too_many_arguments)]
// Il dispatcher mantiene nello stesso confine transazionale validazione,
// budget e publish del segmento; estrarne frammenti separerebbe invarianti che
// devono fallire insieme. L'eccezione resta locale e verificata dalla CI.
#[allow(clippy::too_many_lines)]
pub(super) fn try_run_fused_group(
    segment: &PhysicalSegment,
    state: &ExecState,
    batch: &mut RecordBatch,
    start: usize,
    group_len: usize,
    bytes_at_boundary: &mut u64,
    batch_detail: Option<&str>,
    output_is_plan_output: bool,
) -> Result<bool> {
    let kernels = &segment.kernels[start..start + group_len];
    let Some(decoded_bytes) = fused_group_decoded_bytes(batch, &kernels[0]) else {
        return Ok(false);
    };
    let lease = match state
        .governor
        .try_reserve(decoded_bytes, &kernels[0].node_id)
    {
        Ok(ReservationResult::Granted(lease)) => lease,
        // Reservation fallita -> fallback strumentato (D12.7). Gli esiti
        // `RetryAfterProgress`/`MustSpill` non sono mai emessi dalla v1
        // seriale: per difesa stesso fallback, mai una primitiva di panic.
        Ok(ReservationResult::RetryAfterProgress | ReservationResult::MustSpill) | Err(_) => {
            state.record_geo_fusion_fallback();
            return Ok(false);
        }
    };
    // Misura terminale (architettura.md#geometrie): se l'ultimo membro del gruppo e' una
    // misura "add column" (`GeoMeasure`), il gruppo eseguito dal runner fuso
    // e' il run di trasformazioni che la precede + la misura in coda, che
    // consuma la forma decodificata dell'ultimo passo e appende la colonna
    // scalare. La colonna geometria SOPRAVVIVE (semantica v4): viene
    // ri-encodata una sola volta al confine, come senza misura.
    let (transforms, terminal) = fused_group_terminal(kernels);
    // Parametri tipizzati delle trasformazioni del gruppo (config
    // `GeoTransform` garantita da `prepare`: la condizione di fondibilita'
    // la richiede).
    let mut params: Vec<&TransformArrowSchema> = Vec::with_capacity(transforms.len());
    for kernel in transforms {
        match kernel
            .config
            .geo()
            .and_then(PreparedGeoKernel::transform_params)
        {
            Some(kernel_params) => {
                params.push(kernel_params);
            }
            None => {
                return Err(PlenoraError::Internal(format!(
                    "nodo `{}`: kernel non GeoTransform in un gruppo fuso",
                    kernel.node_id
                )));
            }
        }
    }
    // Handle prepared del PRIMO kernel del gruppo: valida la colonna di
    // input attribuendo l'errore al primo nodo (come il percorso non fuso).
    let prepared = state
        .one_to_one_prepared(&kernels[0], &batch.schema(), params[0])
        .map_err(|error| state.with_diagnostics(error, batch_detail))?;
    // architettura.md#geometrie D12.5: lo schema di output del gruppo e' quello dell'ULTIMA
    // trasformazione — con `reproject` nel gruppo il CRS del campo geometria
    // cambia a meta' catena. La ricostruzione canonica del campo dipende
    // solo da (nome, CRS di output) e gli altri campi passano invariati,
    // quindi l'handle risolto sullo schema del batch e' IDENTICO a quello
    // che il percorso non fuso risolverebbe sullo schema intermedio; per le
    // op di trasformazione o di misura (CRS invariato) coincide con
    // l'handle del primo kernel.
    let last_transform = transforms.len() - 1;
    let output_prepared = state
        .one_to_one_prepared(
            &kernels[last_transform],
            &batch.schema(),
            params[last_transform],
        )
        .map_err(|error| state.with_diagnostics(error, batch_detail))?;
    // Marker del kernel in corso (attribuzione dei panic, D12.6).
    let current = Cell::new(0_usize);
    let attempt = FusedAttempt {
        state,
        segment,
        start,
        group_len,
        rows: batch.num_rows() as u64,
        bytes_in: *bytes_at_boundary,
        output_is_plan_output,
        edges: RefCell::new(vec![Instant::now()]),
    };
    // Da qui il runner fuso viene eseguito: si conta **prima** della
    // chiamata, non dopo, perche' un gruppo che termina con errore o con un
    // panic convertito lo ha comunque raggiunto — ed e' proprio negli oracoli
    // degli errori che «il percorso fuso e' stato eseguito» va dimostrato.
    // Tutto cio' che puo' far rinunciare alla fusione e' gia' stato deciso
    // prima di questo punto: kill switch e formazione del gruppo dal
    // chiamante, byte decodificati ignoti e reservation fallita qui sopra.
    state.record_geo_fusion_group_started();
    // `AssertUnwindSafe`: stessa giustificazione di `run_kernel` — esecuzione
    // seriale, batch e config proprieta' esclusiva della chiamata, l'errore
    // ferma lo stream e nessuno stato del kernel e' riusato dopo un panic.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        one_to_one_batch_fused(
            batch,
            &params,
            terminal,
            &prepared,
            &output_prepared,
            &mut |index| {
                current.set(index);
                if index > 0 {
                    attempt.edges.borrow_mut().push(Instant::now());
                }
                let kernel = &kernels[index];
                #[cfg(test)]
                inject_test_panic(&kernel.node_id);
                state.check_cancellation(kernel)
            },
        )
    }));
    let finished = Instant::now();
    let result = match outcome {
        Ok(result) => result,
        Err(payload) => {
            attempt.account(current.get(), None, finished)?;
            let error = panic_step_error(&kernels[current.get()], &*payload);
            return Err(state.with_diagnostics(error, batch_detail));
        }
    };
    match result {
        Ok(output) => {
            let out_bytes = output.get_array_memory_size() as u64;
            attempt.account(group_len, Some((&output, out_bytes)), finished)?;
            // D12.7: la quota decodificata e' rilasciata PRIMA della
            // reservation del lease di uscita del segmento.
            drop(lease);
            *batch = output;
            *bytes_at_boundary = out_bytes;
            Ok(true)
        }
        Err(FusedStepError::Control(error)) => {
            // Cancellazione al confine del kernel in corso: forma finale,
            // niente `step_error` ne' diagnostica (come il check del loop).
            attempt.account(current.get(), None, finished)?;
            Err(error)
        }
        Err(FusedStepError::Kernel { index, error }) => {
            attempt.account(index, None, finished)?;
            let base = PlenoraError::InvalidPlan(error.to_string());
            let base = match error.row_diagnostics() {
                Some(diagnostics) => base.with_row_diagnostics(diagnostics.clone()),
                None => base,
            };
            let error = step_error(&kernels[index], base);
            Err(state.with_diagnostics(error, batch_detail))
        }
        Err(FusedStepError::Measure { index, error }) => {
            // Misura terminale: l'errore e' gia' nella forma del
            // percorso non fuso (`geo_measure_batch` chiude il `PlenoraError`
            // grezzo con `step_error`, senza wrap in `InvalidPlan`).
            attempt.account(index, None, finished)?;
            let error = step_error(&kernels[index], error);
            Err(state.with_diagnostics(error, batch_detail))
        }
    }
}

/// Scomposizione di un gruppo fuso (architettura.md#geometrie): il run di trasformazioni
/// e la misura terminale opzionale in coda (presente se l'ultimo membro e'
/// un `GeoMeasure` — invariante di `prepare`: la misura puo' solo chiudere
/// un gruppo, mai aprirlo o proseguirlo).
pub(super) fn fused_group_terminal(
    kernels: &[PreparedKernel],
) -> (&[PreparedKernel], Option<FusedTerminal<'_>>) {
    let Some(measure) = kernels[kernels.len() - 1]
        .config
        .geo()
        .and_then(PreparedGeoKernel::measure_kind)
    else {
        return (kernels, None);
    };
    let measure = match measure {
        MeasureKind::Area => FusedTerminalMeasure::Area,
        MeasureKind::Length => FusedTerminalMeasure::Length,
        MeasureKind::Perimeter => FusedTerminalMeasure::Perimeter,
        MeasureKind::VertexCount => FusedTerminalMeasure::VertexCount,
        MeasureKind::ToWkt => FusedTerminalMeasure::ToWkt,
    };
    (
        &kernels[..kernels.len() - 1],
        Some(FusedTerminal {
            measure,
            output_schema: &kernels[kernels.len() - 1].output_contract.schema,
        }),
    )
}

/// Hook di test (errori-e-limiti.md#panic-policy): id dei nodi in cui
/// iniettare un panic, per
/// verificare la conversione panic → errore `Execution` al confine dell'executor.
/// Solo `cfg(test)`: i kernel non usano panic per errori attesi. Insieme
/// (non singolo id): i test girano in parallelo nello stesso processo e
/// ciascuno registra/deregistra il proprio nodo senza interferire.
#[cfg(test)]
pub(super) static PANIC_AT_NODES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
