//! Executor del DAG — `execute`.
//!
//! Vincoli: streaming reale, hot path minimale, materializzazione minima,
//! osservabilita' per nodo anche nei segmenti fusi
//! (architettura.md#planner-ed-executor).
//!
//! [`execute`] accetta solo un [`ValidatedGraph`] (type-state), svolge
//! `prepare` + `execute_physical` e restituisce un [`Output`] a **pull**:
//! l'input e' consumato batch per batch man mano che il chiamante tira
//! l'output. L'esecuzione e' seriale (`SerialFused`), per questo lo stream usa
//! `Rc`/`RefCell` e [`Output`] non e' `Send`.
//!
//! - cancellazione cooperativa ai soli confini dell'executor, secondo il
//!   `CancellationBehavior` di catalogo; i kernel non vedono il token
//!   (errori-e-limiti.md#cancellazione);
//! - ogni `execute` ha un `execution_id` riportato negli errori e nel lock del
//!   [`crate::temp_store::TempStore`], creato **fail-closed**; la modalita'
//!   diagnostica aggiunge contesto strutturale, MAI valori (errori-e-limiti.md);
//! - i batch attraversano gli archi come [`GovernedBatch`] (batch, [`MemoryLease`]
//!   e [`BatchSequence`]): quota contata UNA
//!   volta per batch all'ingresso dell'arco e condivisa al fan-out, sequenza
//!   logica assegnata sugli input, propagata 1:1 negli streaming e riassegnata
//!   nei blocking (architettura.md#memoria, architettura.md#determinismo);
//! - ogni arco e' un [`EdgeShared`]: pass-through con un consumatore, tee che
//!   condivide i batch immutabili con piu' consumatori (D9);
//! - validazione WKB per cella sugli input prima del primo nodo (D8) e limiti
//!   effettivi del piano per input, arco, nodo, batch e cella;
//! - dispatch dei nodi `table.*` via [`crate::table_engine`];
//! - nessun output parziale: il publish e' atomico e avviene solo a stream
//!   completato; un errore a meta' stream lascia consultabili le metriche.
//!
//! I panic dei kernel sono intercettati con `catch_unwind` al punto di
//! dispatch ([`run_kernel`], `execute_binary`) e convertiti in
//! `PlenoraError::Execution` con il solo messaggio del panic
//! (errori-e-limiti.md#panic-policy).

mod blocking;
mod diagnostics;
mod fusion;
mod geo;
mod input;
mod metrics;
mod network;
mod output;
mod staging;
mod state;
mod streaming;
mod validation;

use blocking::{
    dispatch_kernel, run_binary_blocking, run_blocking, run_kernel, spill_capable_unary,
};
use diagnostics::{row_diagnostic_stream, segment_emits_row_diagnostics};
#[cfg(test)]
use fusion::PANIC_AT_NODES;
use fusion::{fused_group_terminal, fusion_group_len, try_run_fused_group};
use geo::{
    append_output_column, geo_accessors_batch, geo_cluster_dbscan_batch, geo_collect_batch,
    geo_coverage_validate_batch, geo_from_wkt_batch, geo_generate_grid_batch,
    geo_line_locate_point_batch, geo_shared_paths_batch, geo_snap_batch, geo_subdivide_batch,
};
pub use input::{Input, Inputs};
#[cfg(test)]
use network::StoredEdgeError;
use network::{EdgeShared, EdgeStream};
use output::canonical_output_schema;
pub use output::Output;
use staging::atomic_input_validation_stream;
use state::ExecState;
use streaming::run_streaming_chain;
use validation::{
    cancellation_behavior, check_edge_batch, check_edge_counts, check_expansion,
    geometry_input_requirements, step_error, validate_wkb_cells,
};

// Quello che serve ai soli moduli di test di questo file, che raggiungono
// i nomi del padre con `super::`. Sotto `cfg(test)` invece che fra gli
// import normali: nella build della libreria non servono piu' a nessuno.
#[cfg(test)]
use crate::cancellation::CancellationToken;
#[cfg(test)]
use crate::governor::MemoryGovernor;
#[cfg(test)]
use diagnostics::{attach_partial_row_diagnostics, merge_row_diagnostics};
use metrics::{accumulate, accumulate_time};
pub use metrics::{ExecutionMetrics, NodeMetrics, SegmentMetrics};
#[cfg(test)]
use plenora_core::arrow::array::BooleanArray;
#[cfg(test)]
use plenora_core::arrow::ipc::writer::FileWriter;
#[cfg(test)]
use plenora_core::arrow::ipc::writer::StreamWriter;
#[cfg(test)]
use plenora_core::catalog::CancellationBehavior;
#[cfg(test)]
use plenora_kernels_table::spill::SpillMetrics;
#[cfg(test)]
use state::arricchisci_con_dettaglio;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use validation::CANCEL_BEHAVIOR_OVERRIDES;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};

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

/// Stream di batch del grafo (seriale, thread-locale nella v1): i batch
/// viaggiano governati — quota di memoria (architettura.md#memoria) e sequenza logica
/// (architettura.md#determinismo) — e sono spaccati/ricomposti solo ai confini dei kernel.
use input::BatchStream;

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Esegue un grafo validato (architettura.md#planner-ed-executor).
///
/// Accetta solo il prodotto di [`crate::planner::validate`] (type-state) e
/// riverifica all'ingresso l'identita' del grafo contro l'ambiente corrente
/// ([`check_compatibility`], piano-v5.md#identita-e-fingerprint): su qualunque
/// mismatch rifiuta.
///
/// Nomi e schemi degli input sono verificati contro i contratti validati prima
/// di costruire lo stream (fail-closed); l'esecuzione parte alla prima pull
/// dell'[`Output`]. Il fingerprint completo dei contratti di input
/// ([`crate::planner::check_input_compatibility`], geometria e CRS) resta al
/// chiamante, che dispone dei `DataContract` letti dagli header IPC.
///
/// # Errors
///
/// - `PlenoraError::InvalidPlan`: `GRAPH_MISMATCH` sull'identita' del grafo,
///   input mancanti/extra/duplicati, op fuori dal dispatch v1 (da `prepare`);
/// - `PlenoraError::Schema`: schema di un input diverso dal contratto
///   validato;
/// - `PlenoraError::Io`/`PlenoraError::InvalidPlan`: `TempStore` non creabile
///   (fail-closed errori-e-limiti.md, vedi l'header del modulo).
#[allow(clippy::needless_pass_by_value)] // Firma per valore voluta da architettura.md#planner-ed-executor.
pub fn execute(graph: &ValidatedGraph, inputs: Inputs, runtime: RuntimeContext) -> Result<Output> {
    // Un piano che richiede il profilo isolato non ricade MAI sul percorso
    // in-process. Lo serve `isolamento::esecuzione_isolata::esegui_isolato`,
    // il cui worker confinato chiama `execute` con `runtime.gia_confinato`
    // valorizzato (tipo non costruibile fuori dal crate); ogni altro arrivo
    // qui e' rifiutato prima di `check_compatibility` e di ogni tocco ai dati.
    if let Some(richiesto_byte) = graph
        .plan()
        .max_domain_memory_bytes()
        .filter(|_| runtime.gia_confinato.is_none())
    {
        return Err(
            crate::isolamento::attivazione::richiesta_isolamento_non_ancora_servibile(
                richiesto_byte,
                graph.effective_limits().max_governed_memory_bytes,
            ),
        );
    }
    check_compatibility(
        graph,
        CATALOG,
        ENGINE_VERSION,
        ARROW_VERSION,
        &local_capabilities(),
    )?;
    // `max_parallelism` si applica QUI, prima di qualunque uso di Rayon:
    // dimensiona il pool del processo, l'unica leva che vincola davvero tutti
    // i percorsi paralleli dei kernel (errori-e-limiti.md, errori-e-limiti.md#limiti-dichiarati). Farlo solo nella
    // CLI lascerebbe il limite inapplicato per chi incorpora l'engine come
    // libreria — cioe' proprio dove nessuno lo noterebbe.
    crate::parallelism::configure(graph.effective_limits().max_parallelism)?;
    let plan = Rc::new(prepare(graph, &runtime)?);
    execute_physical(&plan, graph, inputs, &runtime)
}

/// `execute_physical` (architettura.md#planner-ed-executor, interno): verifica degli input contro i
/// contratti validati e costruzione della rete di stream.
// Orchestrazione lineare della rete di stream: lunga per costruzione.
#[allow(clippy::too_many_lines)]
fn execute_physical(
    plan: &Rc<ExecutionPlan>,
    graph: &ValidatedGraph,
    inputs: Inputs,
    runtime: &RuntimeContext,
) -> Result<Output> {
    let declared: Vec<&String> = graph.plan().inputs().iter().collect();
    for name in declared.iter().map(|s| (*s).as_str()) {
        if !inputs.readers.contains_key(name) {
            return Err(PlenoraError::InvalidPlan(format!("manca l'input `{name}`")));
        }
    }
    if let Some(extra) = inputs
        .readers
        .keys()
        .find(|name| !declared.iter().any(|d| d.as_str() == name.as_str()))
    {
        return Err(PlenoraError::InvalidPlan(format!(
            "input `{extra}` non dichiarato nel piano"
        )));
    }

    // Contratti dichiarati dal chiamante: verifica del fingerprint completo,
    // la stessa di `check_input_compatibility`. E' il confine forte, ed e'
    // disponibile a chi lo chiude passando il contratto insieme all'input.
    if !inputs.contracts.is_empty() {
        let declared: Vec<(String, DataContract)> = inputs
            .contracts
            .iter()
            .map(|(name, contract)| (name.clone(), contract.clone()))
            .collect();
        check_declared_input_contracts(graph, &declared)?;
    }

    // Schemi degli input contro i contratti validati (fail-closed, prima di
    // toccare i dati).
    for (name, input) in &inputs.readers {
        let contract = graph.edge_contract(name).ok_or_else(|| {
            PlenoraError::Internal(format!(
                "l'input `{name}` non ha un contratto nel grafo validato"
            ))
        })?;
        let provided = input.schema()?;
        // Schema COMPLETO, metadati compresi: i metadati di campo portano le
        // chiavi canoniche della geometria (encoding, dimensioni, CRS), che
        // confrontando i soli `fields()` resterebbero fuori — due sorgenti
        // con gli stessi campi e metadati geometrici diversi passerebbero per
        // identiche.
        if provided.fields() != contract.schema.fields()
            || provided.metadata() != contract.schema.metadata()
        {
            return Err(PlenoraError::Schema(format!(
                "l'input `{name}` ha uno schema diverso dal contratto validato"
            )));
        }
    }

    // errori-e-limiti.md, errori arricchiti: identita' dell'esecuzione — UUID v4 (dipendenza `uuid`
    // gia' pinnata nel workspace, nessuna versione nuova) con prefisso
    // leggibile; il charset rispetta la validazione restrittiva del
    // `TempStore` ([A-Za-z0-9._-]).
    let execution_id = format!("exec-{}", uuid::Uuid::new_v4().simple());
    // errori-e-limiti.md: scavenging all'avvio delle directory temporanee orfane, sulla
    // radice dello store (default: temp di sistema; configurabile via
    // `RuntimeContext::temp_root`). Best-effort: un fallimento dello
    // scavenging non deve mai impedire un'esecuzione valida — le directory
    // orfane restano, e un giro successivo le raccoglie solo se ha i permessi
    // per farlo (errori-e-limiti.md#la-pulizia-dei-domini-non-esce-su-un-canale-machine-readable).
    let temp_root = runtime.temp_root.clone().unwrap_or_else(std::env::temp_dir);
    let _ = scavenge_stale_temp_dirs(&temp_root, DEFAULT_SCAVENGE_TTL);
    // Fail-closed, decisione documentata: niente degrado a tempdir semplice.
    // Il `TempStore` e' la difesa strutturale errori-e-limiti.md contro i crash non
    // intercettabili (lo spill ci scrivera'): eseguire senza
    // nasconderebbe la perdita di protezione. Se non e' creabile,
    // l'esecuzione fallisce qui, prima di toccare i dati.
    let temp_store = TempStore::with_root(&execution_id, &temp_root)?;

    let state = ExecState::new(
        plan,
        execution_id,
        runtime.cancellation.clone(),
        runtime.diagnostics,
        temp_store,
    );
    let mut input_contracts = BTreeMap::new();
    for name in graph.plan().inputs() {
        input_contracts.insert(
            name.clone(),
            graph
                .edge_contract(name)
                .ok_or_else(|| {
                    PlenoraError::Internal(format!(
                        "l'input `{name}` non ha un contratto nel grafo validato"
                    ))
                })?
                .clone(),
        );
    }
    let mut network = Network {
        plan: Rc::clone(plan),
        state: Rc::clone(&state),
        inputs: inputs.readers,
        input_contracts,
        edges: HashMap::new(),
    };
    let stream = network.edge_stream(plan.output_edge())?;

    // Wrapper dell'output: max_output_rows, max_batches, byte per batch e
    // metriche di pubblicazione. Il batch resta governato fino alla consegna
    // al chiamante (il lease e' rilasciato dallo spacchettamento in
    // `Output`), cosi' la coda d'uscita resta dentro il perimetro architettura.md#memoria.
    let output_state = Rc::clone(&state);
    let output_edge = plan.output_edge().to_owned();
    let mut output_counts = (0_u64, 0_u64);
    let stream = Box::new(stream.map(move |item| {
        // Tag di categoria: ogni errore `Execution` che esce dal DAG porta l'execution_id.
        let governed = item.map_err(|error| output_state.tag_execution(error))?;
        // errori-e-limiti.md#cancellazione: confine di piano — sempre attivo, anche a valle di op
        // `NonInterruptible` (nessuna nuova attivita' dopo la cancellazione:
        // consegnare/pubblicare e' nuova attivita').
        output_state.check_cancellation_point(&output_edge, "output")?;
        // Heartbeat al confine comune a ogni percorso: un piano pass-through
        // (`nodes: []`) non attraversa nessun `run_*`, e senza questo punto
        // il suo lock non sarebbe mai rinnovato e lo staging di un
        // pass-through geometrico lungo verrebbe classificato orfano.
        output_state.heartbeat();
        output_state.verifica_heartbeat()?;
        let batch = &governed.batch;
        let _ = check_batch_bytes(&output_state, batch, "output")?;
        let limits = &output_state.plan.limits();
        output_counts.0 = output_counts
            .0
            .checked_add(batch.num_rows() as u64)
            .ok_or_else(|| PlenoraError::Internal("overflow conteggio righe output".into()))?;
        output_counts.1 = output_counts
            .1
            .checked_add(1)
            .ok_or_else(|| PlenoraError::Internal("overflow conteggio batch output".into()))?;
        if output_counts.0 > limits.rows.max_output_rows {
            return Err(PlenoraError::ResourceLimit(format!(
                "max_output_rows superato: {} righe di output > {}",
                output_counts.0, limits.rows.max_output_rows
            )));
        }
        if output_counts.1 > limits.max_batches {
            return Err(PlenoraError::ResourceLimit(format!(
                "max_batches superato sull'output: {} batch > {}",
                output_counts.1, limits.max_batches
            )));
        }
        let mut metrics = output_state.metrics.borrow_mut();
        metrics.output_rows = output_counts.0;
        metrics.output_batches = output_counts.1;
        Ok(governed)
    })) as BatchStream;

    let contract = graph.output_contract()?.clone();
    // Lo schema IPC (blocco canonico R2.2 + versione R2.5) e'
    // calcolato una sola volta qui — fail-fast su divergenze R2.6, prima di
    // toccare i dati.
    let schema = canonical_output_schema(&contract)?;
    Ok(Output {
        contract,
        schema,
        stream,
        state,
        esaurito: false,
    })
}

/// La rete di stream del DAG: costruzione lazy e memoizzata degli archi.
struct Network {
    plan: Rc<ExecutionPlan>,
    state: Rc<ExecState>,
    inputs: BTreeMap<String, Input>,
    input_contracts: BTreeMap<String, DataContract>,
    edges: HashMap<String, Rc<EdgeShared>>,
}

impl Network {
    /// Stream di lettura di un arco (nome di input o id nodo). Archi con
    /// piu' consumatori (fan-out) sono condivisi via tee (D9).
    fn edge_stream(&mut self, edge: &str) -> Result<EdgeStream> {
        if let Some(shared) = self.edges.get(edge) {
            return Ok(shared.register_reader());
        }
        let upstream: BatchStream = if self.inputs.contains_key(edge) {
            self.input_stream(edge)?
        } else {
            let index = self.plan.segment_of(edge).ok_or_else(|| {
                PlenoraError::InvalidPlan(format!("arco `{edge}` senza produttore"))
            })?;
            self.segment_stream(index)?
        };
        let shared = EdgeShared::new(upstream);
        self.edges.insert(edge.to_owned(), Rc::clone(&shared));
        Ok(shared.register_reader())
    }

    /// Stream di un input del piano: limiti per input, byte per batch e
    /// validazione dinamica WKB per cella (D8) prima del primo nodo.
    ///
    /// Confine di lettura (BLOCK-03): gli errori della sorgente e della
    /// coerenza per-batch dello schema sono taggati [`ErrorPhase::Read`].
    /// Qui ogni batch riceve il lease di memoria (architettura.md#memoria) e
    /// la sequenza logica (architettura.md#determinismo: nome dell'input,
    /// partizione 0, contatore seriale per input).
    ///
    /// Le invarianti di costruzione della rete violate danno errore
    /// `Internal`, mai un panic (R6).
    // Costruzione lineare della rete per arco: lunga per costruzione.
    #[allow(clippy::too_many_lines)]
    fn input_stream(&mut self, edge: &str) -> Result<BatchStream> {
        let input = self.inputs.remove(edge).ok_or_else(|| {
            PlenoraError::Internal(format!(
                "reader dell'input `{edge}` assente: invariante di dispatch violata"
            ))
        })?;
        let contract = self
            .input_contracts
            .get(edge)
            .ok_or_else(|| {
                PlenoraError::Internal(format!(
                    "contratto dell'input `{edge}` assente: invariante di dispatch violata"
                ))
            })?
            .clone();
        let raw: BatchStream = match input {
            Input::Batches { batches } => Box::new(
                batches
                    .into_iter()
                    .map(|batch| Ok(GovernedBatch::new(batch, None, None))),
            ),
            Input::Stream { iter, .. } => iter,
        };

        let state = Rc::clone(&self.state);
        let edge_name = edge.to_owned();
        let expected_schema = contract.schema.clone();
        let (geometry_index, geometry_dimensions, geometry_encoding, geometry_srid) =
            geometry_input_requirements(&contract)?;
        let requires_atomic_wkb_validation = geometry_index.is_some();
        let mut sequence_number = 0_u64;
        let mut source_row_offset = 0_u64;
        let mapped = Box::new(raw.map(move |item| {
            // Confine di lettura (BLOCK-03): gli errori della sorgente e la
            // coerenza per-batch dello schema nascono leggendo l'input —
            // fase Read. Gli errori di governor e di validazione WKB qui
            // sotto restano validazione, derivata per variante.
            let batch = item
                .map_err(|error| error.with_phase(ErrorPhase::Read))?
                .batch;
            if batch.schema().as_ref() != expected_schema.as_ref() {
                return Err(PlenoraError::Schema(format!(
                    "batch dell'input `{edge_name}` con schema diverso dal contratto"
                ))
                .with_phase(ErrorPhase::Read));
            }
            let bytes = check_batch_bytes(&state, &batch, &edge_name)?;
            let batch_offset = source_row_offset;
            source_row_offset = source_row_offset
                .checked_add(batch.num_rows() as u64)
                .ok_or_else(|| PlenoraError::Internal("overflow indice sorgente WKB".into()))?;
            if let Some(index) = geometry_index {
                validate_wkb_cells(
                    &state,
                    &batch,
                    index,
                    &edge_name,
                    geometry_dimensions,
                    geometry_encoding,
                    geometry_srid,
                    batch_offset,
                )?;
            }
            {
                let mut counts = state.input_counts.borrow_mut();
                // Chiave clonata solo al primo batch dell'input (hot path minimale): i
                // batch successivi entrano dal `get_mut` sul nome esistente.
                if let Some(entry) = counts.get_mut(&edge_name) {
                    entry.0 = entry
                        .0
                        .checked_add(batch.num_rows() as u64)
                        .ok_or_else(|| {
                            PlenoraError::Internal("overflow conteggio righe input".into())
                        })?;
                    entry.1 = entry.1.checked_add(1).ok_or_else(|| {
                        PlenoraError::Internal("overflow conteggio batch input".into())
                    })?;
                    entry.2 = entry.2.checked_add(bytes).ok_or_else(|| {
                        PlenoraError::Internal("overflow conteggio byte input".into())
                    })?;
                } else {
                    counts.insert(edge_name.clone(), (batch.num_rows() as u64, 1, bytes));
                }
                let entry = &counts[&edge_name];
                let limits = &state.plan.limits();
                // Fase `Read` esplicita per tutti e tre: scattano mentre si
                // legge la sorgente, allo stesso confine dei tetti del
                // trasporto, e la derivazione per variante darebbe `Validate`
                // (piano-v5.md#contratti-di-input).
                if entry.0 > limits.rows.max_input_rows {
                    return Err(PlenoraError::ResourceLimit(format!(
                        "max_input_rows superato sull'input `{edge_name}`: {} righe > {}",
                        entry.0, limits.rows.max_input_rows
                    ))
                    .with_phase(ErrorPhase::Read));
                }
                if entry.1 > limits.max_batches {
                    return Err(PlenoraError::ResourceLimit(format!(
                        "max_batches superato sull'input `{edge_name}`: {} batch > {}",
                        entry.1, limits.max_batches
                    ))
                    .with_phase(ErrorPhase::Read));
                }
                if entry.2 > limits.max_payload_bytes {
                    return Err(PlenoraError::ResourceLimit(format!(
                        "max_payload_bytes superato sull'input `{edge_name}`: {} byte > {}",
                        entry.2, limits.max_payload_bytes
                    ))
                    .with_phase(ErrorPhase::Read));
                }
            }
            // architettura.md#memoria: reservation immediata; i limiti per
            // input sono gia' passati, quindi qui il fallimento e' solo per
            // budget globale esaurito. Fase `Read` esplicita, come i tetti
            // qui sopra (la derivazione direbbe `Write`).
            let lease = state
                .governor
                .reserve(bytes, &edge_name)
                .map_err(|error| error.with_phase(ErrorPhase::Read))?;
            // architettura.md#determinismo: sequenza logica d'ingresso (contatore seriale).
            let seq = BatchSequence {
                source_node: edge_name.clone(),
                input_partition: 0,
                sequence_number,
            };
            sequence_number = sequence_number
                .checked_add(1)
                .ok_or_else(|| PlenoraError::Internal("overflow sequenza batch input".into()))?;
            Ok(GovernedBatch::new(batch, Some(lease), Some(seq)))
        })) as BatchStream;
        if requires_atomic_wkb_validation {
            Ok(atomic_input_validation_stream(
                mapped,
                Rc::clone(&self.state),
                edge.to_owned(),
            ))
        } else {
            Ok(mapped)
        }
    }

    /// Stream prodotto da un segmento, secondo la sua modalita' (modalita' fisiche esplicite).
    fn segment_stream(&mut self, index: usize) -> Result<BatchStream> {
        let (mode, input_edges) = {
            let segment = &self.plan.segments()[index];
            (segment.mode, segment.input_edges.to_vec())
        };
        match mode {
            SegmentMode::LinearStreaming | SegmentMode::GeoFused => {
                let input = self.edge_stream(&input_edges[0])?;
                if segment_emits_row_diagnostics(&self.plan, index) {
                    Ok(row_diagnostic_stream(
                        input,
                        Rc::clone(&self.plan),
                        Rc::clone(&self.state),
                        index,
                    ))
                } else {
                    let plan = Rc::clone(&self.plan);
                    let state = Rc::clone(&self.state);
                    Ok(Box::new(input.map(move |item| {
                        run_streaming_chain(&plan, index, &state, item?, None, None)
                    })))
                }
            }
            SegmentMode::Blocking => {
                let mut input = self.edge_stream(&input_edges[0])?;
                let plan = Rc::clone(&self.plan);
                let state = Rc::clone(&self.state);
                let mut once = Some(move || {
                    let kernel = plan.segments()[index].kernels.first().ok_or_else(|| {
                        PlenoraError::Internal(
                            "segmento blocking senza kernel: invariante del planner violata".into(),
                        )
                    })?;
                    // architettura.md#memoria: verso un kernel spill-capable la
                    // quota dei batch drenati e' rilasciata subito, perche' la
                    // soglia dello spill si misura sullo stesso budget e
                    // trattenere i lease la renderebbe irraggiungibile. La
                    // materializzazione dell'input resta in RAM.
                    let spill_capable = spill_capable_unary(kernel);
                    // errori-e-limiti.md#cancellazione: drenaggio dell'input — check a ogni
                    // confine di batch, onorando il behavior del kernel che
                    // ricevera' i dati (`NonInterruptible`: mai).
                    let mut batches = Vec::new();
                    for item in &mut input {
                        state.check_cancellation(kernel)?;
                        let mut governed = item?;
                        if spill_capable {
                            governed.lease = None;
                        }
                        batches.push(governed);
                    }
                    run_blocking(&plan, index, &state, batches)
                });
                Ok(Box::new(std::iter::from_fn(move || {
                    once.take().map(|mut run| run())
                })))
            }
            SegmentMode::BinaryBlocking => {
                let mut left = self.edge_stream(&input_edges[0])?;
                let mut right = self.edge_stream(&input_edges[1])?;
                let plan = Rc::clone(&self.plan);
                let state = Rc::clone(&self.state);
                let mut once = Some(move || {
                    let kernel = plan.segments()[index].kernels.first().ok_or_else(|| {
                        PlenoraError::Internal(
                            "segmento binario senza kernel: invariante del planner violata".into(),
                        )
                    })?;
                    // errori-e-limiti.md#cancellazione: come per il blocking unario — check a ogni
                    // confine di batch durante il drenaggio dei due rami.
                    let mut left_batches = Vec::new();
                    for item in &mut left {
                        state.check_cancellation(kernel)?;
                        left_batches.push(item?);
                    }
                    let mut right_batches = Vec::new();
                    for item in &mut right {
                        state.check_cancellation(kernel)?;
                        right_batches.push(item?);
                    }
                    run_binary_blocking(&plan, index, &state, left_batches, right_batches)
                });
                Ok(Box::new(std::iter::from_fn(move || {
                    once.take().map(|mut run| run())
                })))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Limiti e validazione dinamica
// ---------------------------------------------------------------------------

/// Tetto duro sui byte di un batch (tetto in byte per batch: `max_batch_bytes`). Restituisce i
/// byte del batch (hot path minimale: un solo conteggio, riusato da lease e contatori).
fn check_batch_bytes(state: &ExecState, batch: &RecordBatch, where_: &str) -> Result<u64> {
    let bytes = batch.get_array_memory_size();
    let max = state.plan.batch_target().max_batch_bytes;
    if bytes > max {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_batch_bytes superato su `{where_}`: {bytes} byte > {max}"
        )));
    }
    Ok(bytes as u64)
}

/// Conversione di un panic di kernel in errore di nodo
/// (errori-e-limiti.md#panic-policy): il payload
/// testuale (`&str`/`String`) diventa il motivo, mai dati dei batch (regola
/// di error.rs: contesto, non valori). Payload non testuale: motivo generico.
pub(super) fn panic_step_error(
    kernel: &PreparedKernel,
    payload: &(dyn std::any::Any + Send),
) -> PlenoraError {
    // Il testo del panico NON viene pubblicato: non e' scritto da noi e puo'
    // contenere i valori che un `assert` di una dipendenza ha confrontato,
    // cioe' dati della riga. Si riporta solo la forma del payload; il nodo e
    // l'operazione, che sono la vera informazione diagnostica, li aggiunge
    // `step_error`.
    let forma = plenora_core::panic_policy::forma_payload(payload);
    // Categoria `Internal`, non `InvalidPlan`. Un panico dentro un kernel e'
    // un difetto NOSTRO — o di una dipendenza che usiamo — e il chiamante non
    // ha nulla da correggere nel proprio piano. La classificazione conta
    // perche' `step_error` conserva le categorie invece di avvolgere tutto in
    // `Execution`: dire `invalid_plan` manderebbe chi legge a cercare un
    // errore che non ha commesso.
    step_error(
        kernel,
        PlenoraError::Internal(format!("panic nel kernel: {forma}")),
    )
}

/// Lato di un'operazione binaria geo (campo strutturato del carrier D14.5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GeoBinarySide {
    Left,
    Right,
}

impl GeoBinarySide {
    /// Nome stabile del lato (dettaglio diagnostico, mai nel testo base).
    const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// Carrier degli errori di uno step binario geo prima dell'attribuzione al
/// nodo (architettura.md#geometrie D14.5.2): sorgente grezza + fase + side + indice di riga
/// come campi strutturati. La posizione NON entra MAI nel testo base del
/// messaggio (regola 8): e' pubblicata solo nel dettaglio diagnostico
/// opt-in (`side=… row=…`, stesso canale di `batch_seq`, errori-e-limiti.md, errori arricchiti).
pub(super) struct GeoBinaryStepError {
    /// Fase del ciclo (D14.5.4): `Read` per drenaggio/decode, `Write` per
    /// kernel e costruzione dell'output.
    phase: ErrorPhase,
    /// Lato che ha prodotto l'errore (decode), se applicabile.
    side: Option<GeoBinarySide>,
    /// Riga della cella nella sequenza decodificata, se applicabile.
    row_index: Option<u64>,
    /// Sorgente grezza, invariata nel testo.
    source: PlenoraError,
}

/// Conversione del carrier D14.5 in errore di nodo: `step_error` aggiunge il
/// contesto **preservando la categoria** (D14.5.1); la fase `Read`
/// del decode e' taggata al confine (`Write` e' gia' la fase derivata di
/// `Execution`, D14.5.4); side/riga solo come dettaglio diagnostico opt-in.
pub(super) fn geo_binary_step_error(
    state: &ExecState,
    kernel: &PreparedKernel,
    carrier: GeoBinaryStepError,
) -> PlenoraError {
    let detail = match (carrier.side, carrier.row_index) {
        (Some(side), Some(row)) => Some(format!("side={} row={row}", side.as_str())),
        (Some(side), None) => Some(format!("side={}", side.as_str())),
        (None, Some(row)) => Some(format!("row={row}")),
        (None, None) => None,
    };
    let error = state.with_diagnostics(step_error(kernel, carrier.source), detail.as_deref());
    // `tag_execution` prima del tag di fase: il wrapper `Tagged` non
    // attraversa il riempimento dell'`execution_id` al confine di uscita.
    let error = state.tag_execution(error);
    if carrier.phase == ErrorPhase::Write {
        error
    } else {
        error.with_phase(carrier.phase)
    }
}

/// Metriche di un'esecuzione di kernel (per nodo e per segmento, osservabilita' per nodo).
/// `first`/`last` indicano la posizione del kernel nel segmento (righe e
/// batch di ingresso contati solo sul primo, di uscita solo sull'ultimo).
/// I byte sono i metadati dei buffer Arrow ai confini del kernel
/// (osservabilita' per nodo; architettura.md#memoria: il governor non riconta — questi conteggi sono metriche,
/// non reservation).
#[allow(clippy::too_many_arguments)]
pub(super) fn record_kernel_metrics(
    state: &ExecState,
    segment: &PhysicalSegment,
    kernel: &PreparedKernel,
    rows_in: u64,
    rows_out: u64,
    bytes_in: u64,
    bytes_out: u64,
    elapsed: Duration,
    first: bool,
    last: bool,
) {
    // errori-e-limiti.md: heartbeat del TempStore al punto centrale — ogni batch
    // processato passa di qui (throttled, vedi `ExecState::heartbeat`).
    state.heartbeat();
    let config = state.plan.metrics_config();
    let mut borrowed = state.metrics.borrow_mut();
    let metrics = &mut *borrowed;
    let saturated = &mut metrics.counters_saturated;
    // Metrica obbligatoria errori-e-limiti.md: sempre aggiornata, indipendente dalla
    // configurazione per-nodo/per-segmento.
    accumulate(&mut metrics.total_rows_processed, rows_in, saturated);
    if config.per_node {
        if let Some(node) = metrics.nodes.get_mut(&kernel.node_id) {
            accumulate(&mut node.rows_in, rows_in, saturated);
            accumulate(&mut node.rows_out, rows_out, saturated);
            accumulate(&mut node.batches_in, 1, saturated);
            accumulate(&mut node.batches_out, 1, saturated);
            accumulate(&mut node.bytes_in, bytes_in, saturated);
            accumulate(&mut node.bytes_out, bytes_out, saturated);
            accumulate_time(&mut node.wall_time, elapsed, saturated);
        }
    }
    if config.per_segment {
        if let Some(seg) = metrics.segments.get_mut(&segment.id) {
            if first {
                accumulate(&mut seg.rows_in, rows_in, saturated);
                accumulate(&mut seg.batches_in, 1, saturated);
            }
            if last {
                accumulate(&mut seg.rows_out, rows_out, saturated);
                accumulate(&mut seg.batches_out, 1, saturated);
            }
            accumulate_time(&mut seg.wall_time, elapsed, saturated);
        }
    }
}

// ---------------------------------------------------------------------------
// Esecuzione dei kernel
// ---------------------------------------------------------------------------

/// Sequenza logica dell'output di un segmento blocking
/// (architettura.md#determinismo).
///
/// La cardinalita' cambia, quindi la sequenza degli input non si propaga 1:1.
/// Il segmento emette UN batch per esecuzione, in ordine di scansione seriale
/// (left-then-right per i binari): la sequenza e' sempre nodo del kernel,
/// partizione 0, numero 0.
pub(super) fn blocking_output_sequence(kernel: &PreparedKernel) -> BatchSequence {
    BatchSequence {
        source_node: kernel.node_id.clone(),
        input_partition: 0,
        sequence_number: 0,
    }
}

/// Iniezione del panic di test: scatta solo ai nodi registrati nell'hook.
#[cfg(test)]
pub(super) fn inject_test_panic(node_id: &str) {
    if PANIC_AT_NODES
        .lock()
        .expect("hook panic non avvelenato")
        .iter()
        .any(|node| node == node_id)
    {
        panic!("panic di test iniettato al nodo `{node_id}`");
    }
}

#[cfg(test)]
// Come il modulo `tests`: copre anche il percorso permissivo deprecato.
#[allow(deprecated)]
mod governor_tests;
#[cfg(test)]
// I test coprono anche il percorso permissivo di `Inputs` (`add`/`with`),
// deprecato ma ancora supportato: finche' esiste, va testato. L'`allow` sta
// qui, sulla dichiarazione del modulo, cosi' e' un punto solo da cancellare
// quando la deprecazione diventera' rimozione.
#[allow(deprecated)]
mod tests;
