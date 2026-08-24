//! Executor del DAG — fase 2 `execute`.
//!
//! Vincoli che governano questo modulo: streaming reale, hot path minimale,
//! materializzazione minima, e osservabilita' per nodo anche dentro ai
//! segmenti fusi (architettura.md#planner-ed-executor).
//!
//! [`execute`] accetta solo un [`ValidatedGraph`] (type-state: nessun
//! percorso non validato raggiunge l'esecuzione), svolge internamente
//! `prepare` + `execute_physical` (architettura.md#planner-ed-executor) e restituisce un [`Output`] a
//! **pull**: i batch finali sono uno stream lazy, l'input e' consumato
//! batch-per-batch man mano che il chiamante tira l'output (streaming reale: una
//! pipeline streaming non materializza l'intera tabella).
//!
//! Esecuzione v1: **seriale** ovunque (`SerialFused`); il parallelismo
//! resta da fare (M3), lo spill c'e' solo per i kernel che lo prevedono.
//! Il governor della memoria, la cancellazione cooperativa e gli errori
//! arricchiti sono invece attivi (vedi sotto). Per questo lo stream usa
//! `Rc`/`RefCell` e
//! [`Output`] non e' `Send`: e' una scelta documentata, non un limite
//! nascosto.
//!
//! Fase 2B, cancellazione cooperativa (errori-e-limiti.md#cancellazione):
//! il chiamante passa un
//! [`crate::cancellation::CancellationToken`] nel [`RuntimeContext`] e lo
//! cancella dall'esterno (es. handler Ctrl-C della CLI). I check sono solo
//! ai confini dell'executor — tra batch nelle catene streaming, tra kernel,
//! durante il drenaggio dei segmenti blocking e sull'output del piano — e
//! onorano il `CancellationBehavior` dichiarato in catalogo: `Cooperative`
//! (check a ogni batch), `BoundaryOnly` (check tra kernel/a fine kernel e
//! durante il drenaggio), `NonInterruptible` (mai: l'op completa; i confini
//! di piano a valle restano attivi — nessuna nuova attivita' dopo la
//! cancellazione, publish compreso). I kernel NON vedono il token (il
//! passaggio e' M3). Al cancel l'errore e' `PlenoraError::Cancelled` con
//! `node`/`operation`/`execution_id`; nessun output e' pubblicato (publish
//! atomico) e le metriche parziali restano osservabili iterando [`Output`]
//! manualmente e chiamando [`Output::metrics`] dopo l'errore (i metodi di
//! comodo `collect_batches`/`write_ipc_file*` consumano l'`Output`: con
//! loro le metriche al punto di cancel vanno perse, limite v1 documentato).
//!
//! Fase 2B, errori arricchiti (errori-e-limiti.md): ogni `execute` genera un
//! `execution_id` (UUID v4 — dipendenza `uuid` gia' pinnata nel workspace,
//! nessuna versione nuova) riportato negli errori `Execution`/`Cancelled` e nel
//! lock del [`crate::temp_store::TempStore`]; `PlenoraError` espone
//! `category()` (poi `phase()`, `remote_effect()` e `retry_disposition()` —
//! gli assi §9; R9.7 ha sostituito il `retryable()` della prima tassonomia). La modalita'
//! diagnostica opt-in
//! (`RuntimeContext::diagnostics`, solo per input fidati) aggiunge alla
//! motivazione contesto strutturale — indice di batch, riga, colonna dove
//! disponibile — MAI valori; a flag spento i messaggi sono invariati.
//!
//! Fase 2B — `TempStore` (errori-e-limiti.md): `execute` esegue lo scavenging
//! best-effort delle directory orfane all'avvio (sulla radice configurata,
//! default temp di sistema) e crea lo store dell'esecuzione **fail-closed**
//! (decisione documentata: niente degrado a tempdir semplice — lo store e'
//! la difesa strutturale contro i crash non intercettabili e lo spill
//! ci scrivera'; se non e' creabile l'esecuzione fallisce prima di toccare
//! i dati). L'heartbeat e' scritto al punto centrale (ogni batch processato
//! passa dal conteggio metriche) con throttle di
//! [`HEARTBEAT_MIN_INTERVAL`]; il cleanup e' RAII al `Drop` dello stato.
//!
//! Fase 2B, governor della memoria — resource accounting (architettura.md#memoria) e sequenza logica
//! (architettura.md#determinismo): i batch attraversano gli archi come [`GovernedBatch`]
//! (batch, [`MemoryLease`] e [`BatchSequence`]). La quota `max_governed_memory_bytes`
//! e' contata UNA volta per batch all'ingresso dell'arco e condivisa
//! reference-counted al fan-out; i kernel restano su `RecordBatch` puro — il
//! wrapper si spacca in ingresso al segmento e si ricompone in uscita. La
//! sequenza e' assegnata sugli input (partizione 0, contatore per input),
//! propagata 1:1 negli streaming e riassegnata deterministicamente nei
//! blocking (regola in [`run_blocking`]). In seriale la reservation e'
//! immediata: quota disponibile o errore `InvalidPlan` fail-fast (regola v1 in
//! [`crate::governor::MemoryGovernor::try_reserve`]).
//!
//! Struttura fisica:
//!
//! - ogni arco del DAG e' un canale condiviso ([`EdgeShared`]): un solo
//!   consumatore = pass-through puro; piu' consumatori (fan-out, D9) =
//!   tee che condivide i `RecordBatch` immutabili senza copie di buffer e
//!   rilascia ciascun batch quando tutti i consumatori lo hanno letto
//!   (rilascio al last consumer). Il tee bufferizza [`GovernedBatch`]: il lease e' condiviso (clone
//!   `Arc`) tra i consumatori, la quota resta contata una sola volta e torna
//!   al governor con `release_consumed` + `Drop` dell'ultimo riferimento.
//!   In esecuzione seriale i consumatori drenano in sequenza, quindi
//!   il tee coincide con la materializzazione conservativa di D9;
//! - `LinearStreaming`/`GeoFused`: il batch attraversa la catena di kernel
//!   senza code ne' materializzazioni (segmenti lineari senza code). Nei segmenti `GeoFused` i run di
//!   kernel fondibili annotati da `prepare` (campo `fusion_group`) sono
//!   eseguiti col runner fuso di architettura.md#geometrie — un decode/encode per gruppo su
//!   ogni batch, con errori/metriche/cancellazione per nodo preservati e
//!   fallback strumentato al percorso nodo-per-nodo a reservation governor
//!   fallita (D12.6/D12.7);
//! - `Blocking`/`BinaryBlocking`: alla prima pull drenano gli input del
//!   segmento (materializzazione prevista dal piano, materializzazione minima), concatenano ed
//!   eseguono il kernel una sola volta;
//! - dispatch nodi: `table.*` via [`crate::table_engine`] (`execute_batch`
//!   per gli unari, `execute_binary` per i binari, con la config gia'
//!   validata in `prepare`); `geo.*` 1:1 in place via
//!   [`crate::geo_transport::transport::transform_batches`]; le misure geo
//!   "add column" (`geo.area` ecc.) via dispatch dedicato sui kernel
//!   `plenora_kernels_geo::operations` (la semantica v4 aggiunge una
//!   colonna, il trasporto legacy la sostituirebbe); le estensioni geo
//!   v1.1-v1.3 via gli adapter Arrow di `plenora_kernels_geo`
//!   (`extensions`/`extensions2`/`extensions3`/`cluster`): streaming per
//!   batch (`from_wkt`, `geometry_accessors`, `line_locate_point`, `snap`,
//!   `subdivide` come espansione 1:N con `__parent_index`), blocking su
//!   input materializzato (`collect` con raggruppamento per chiavi
//!   nell'engine, `generate_grid`, `coverage_validate`, `shared_paths`,
//!   `cluster_dbscan`);
//! - validazione dinamica in lettura (D8): WKB strutturale per cella sugli
//!   input con geometria, tramite
//!   [`plenora_kernels_geo::validate_wkb_contract`], prima che i dati
//!   raggiungano il primo nodo;
//! - limiti effettivi del piano: `max_input_rows` per input,
//!   `max_rows_per_edge` per arco intermedio, `max_output_rows`,
//!   `max_expansion_factor` per nodo (base: input per gli unari; per i
//!   binari calcolate tutte le metriche [`JoinExpansion`] e applicato il
//!   vincolo vincolante dichiarato in catalogo, errori-e-limiti.md; le op `WholeToMany`
//!   generative/diagnostiche sono esenti — esenzione dichiarata in
//!   catalogo, la base input e' un trigger, insensata come denominatore),
//!   `max_batches` per arco, `max_wkb_cell_bytes` per cella,
//!   `max_payload_bytes` cumulati per input, `max_geometry_depth` per
//!   annidamento WKB, `max_batch_bytes` per batch (tetto in byte per batch, tetto duro, applicato
//!   anche al batch concatenato dei segmenti blocking);
//! - nessun output parziale: [`Output::write_ipc_file`] scrive via
//!   [`crate::geo_transport::publish::publish_atomic`] (tempfile + persist
//!   no-clobber solo a stream completato con successo);
//! - metriche per nodo logico e per segmento (osservabilita' per nodo), prefilled per tutti i
//!   nodi del piano e aggiornate batch per batch.
//!
//! Errore a meta' stream: il batch in errore propaga `Err` nello stream di
//! output; niente viene pubblicato (il tempfile e' eliminato da
//! `publish_atomic`) e le metriche restano consultabili fino al punto di
//! fallimento.
//!
//! Panic dei kernel (errori-e-limiti.md#panic-policy): intercettati con `catch_unwind` al punto di
//! dispatch — [`run_kernel`] per i kernel unari (streaming e blocking) e la
//! chiamata `execute_binary` per i segmenti binari, il livello piu' interno
//! che conserva l'attribuzione di nodo — e convertiti in
//! `PlenoraError::Execution { node, operation, .. }` con il solo messaggio del
//! panic, mai dati dei batch (regola di error.rs). L'errore propaga come
//! qualunque altro: il publish atomico non e' raggiunto (nessun publish
//! dopo panic) e il cleanup (tempfile, buffer degli archi) avviene comunque
//! via `Drop`. I confini `UnwindSafe` dichiarati per il DAG parallelo
//! (worker, cancellazione globale, spill) restano Fase 2B.

mod diagnostics;
mod input;
mod metrics;
mod network;
mod output;
mod staging;
mod state;
mod validation;

use diagnostics::{row_diagnostic_stream, segment_emits_row_diagnostics};
pub use input::{Input, Inputs};
#[cfg(test)]
use network::StoredEdgeError;
use network::{EdgeShared, EdgeStream};
use output::canonical_output_schema;
pub use output::Output;
use staging::atomic_input_validation_stream;
use state::ExecState;
use validation::{
    cancellation_behavior, check_edge_batch, check_edge_counts, check_expansion,
    check_join_expansion, geometry_input_requirements, step_error, validate_wkb_cells,
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
use metrics::{accumulate, accumulate_time, sum_rows};
pub use metrics::{ExecutionMetrics, NodeMetrics, SegmentMetrics};
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
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::arrow::select::take::take;
use plenora_core::catalog::CATALOG;
use plenora_core::contract::{BatchSequence, DataContract};
use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::{ErrorPhase, PlenoraError, Result};
use plenora_kernels_geo::analysis::{
    count_points_in_polygons_validated, nearest_matches_validated, within_indexes_validated,
};
use plenora_kernels_geo::arrow_adapter::{batch_geometry_cells, decode_geometry_cell};
use plenora_kernels_geo::operations;
use plenora_kernels_geo::spatial_join::spatial_join_nullable_validated;

use crate::geo_transport::pair::{decode_geometry_batches, preflight_decoded_bytes, PairOperation};
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
    prepare, AccessorKind, ExecutionPlan, GeoBinaryPlan, MeasureKind, PhysicalSegment,
    PreparedConfig, PreparedKernel, RuntimeContext, SegmentMode,
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

/// Fase 2 `execute` (architettura.md, architettura.md#planner-ed-executor): accetta solo il
/// prodotto di [`crate::planner::validate`] (type-state), esegue
/// internamente `prepare` + `execute_physical`.
///
/// All'ingresso l'identita' piano-v5.md#identita-e-fingerprint del grafo e' riverificata contro l'ambiente
/// corrente ([`check_compatibility`]: catalogo, versioni engine/Arrow,
/// capability): l'executor rifiuta su qualunque mismatch, mai procedere alla
/// cieca. Nella v1 il grafo e' validato e usato nello stesso processo, quindi
/// il check e' parzialmente ridondante, ma il costo e' irrilevante (grafo in
/// memoria) e la porta resta chiusa per il riuso futuro.
///
/// I nomi e gli schemi degli input sono verificati contro i contratti
/// validati prima di costruire lo stream (fail-closed); l'esecuzione vera e
/// propria resta lazy: parte alla prima pull dell'[`Output`]. Il fingerprint
/// completo dei contratti di input ([`crate::planner::check_input_compatibility`],
/// che copre geometria e CRS) resta al chiamante, che dispone dei
/// `DataContract` letti dagli header IPC: qui gli input arrivano come soli
/// schemi Arrow.
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
    // CLI lasciava il limite inapplicato per chi incorpora l'engine come
    // libreria — cioe' proprio dove nessuno lo avrebbe notato.
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
    let declared: Vec<&String> = graph.plan().plan().inputs.iter().collect();
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
        // confrontando i soli `fields()` restavano fuori — due sorgenti con
        // gli stessi campi e metadati geometrici diversi passavano identiche.
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
    // orfane restano e saranno raccolte al giro successivo.
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
    for name in &graph.plan().plan().inputs {
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
        // Heartbeat al confine COMUNE a ogni percorso.
        //
        // I tre `run_*` non bastano: un piano pass-through (`nodes: []`) non
        // ne attraversa nessuno — l'output e' direttamente lo stream
        // dell'input — quindi non rinnovava mai il lock e non inizializzava
        // nemmeno il conteggio dei fallimenti, rendendo inefficace qualunque
        // controllo finale. Un pass-through geometrico usa staging
        // temporaneo, e su un'esecuzione lunga se lo vedeva classificare
        // orfano. Qui passa ogni batch di ogni piano.
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
    // Milestone C: lo schema IPC (blocco canonico R2.2 + versione R2.5) e'
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
    ///
    /// Punto di ingresso nel perimetro governato: qui ogni batch riceve il
    /// lease di memoria (architettura.md#memoria, quota contata UNA volta per arco) e la
    /// sequenza logica (architettura.md#determinismo: `source_node` = nome dell'input,
    /// `input_partition` = 0 — nessun ramo parallelo della sorgente in v1 —
    /// `sequence_number` = contatore seriale per input).
    ///
    /// Errori `Internal` sulle invarianti di costruzione della rete
    /// (reader/contratto dell'input presenti per il dispatch del chiamante,
    /// colonna geometria nello schema del contratto): il caso "impossibile"
    /// e' un errore esplicito, mai un panic (R6).
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
                // Fase `Read` per tutti e tre: questi tetti scattano mentre
                // si LEGGE la sorgente, allo stesso confine dei tetti del
                // trasporto (`ipc_boundary::read_error`). Prima uscivano come
                // `Validate` per derivazione di variante, e al medesimo
                // confine due limiti sulla stessa lettura dichiaravano fasi
                // diverse: un tetto di byte diceva «lettura», un tetto di
                // righe diceva «validazione». Il tag esplicito vince sulla
                // derivazione, come stabilito in piano-v5.md#contratti-di-input.
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
            // architettura.md#memoria: reservation immediata (v1 seriale — regola in
            // `MemoryGovernor::try_reserve`); i limiti per input sopra sono
            // gia' passati, quindi qui il fallimento e' solo per budget
            // globale esaurito.
            //
            // Fase `Read` come i tre tetti qui sopra: e' lo stesso confine e
            // lo stesso istante. Senza il tag la derivazione della variante
            // direbbe `Write`, e due limiti sulla stessa lettura
            // dichiarerebbero di nuovo fasi diverse.
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
                    // architettura.md#memoria (Fase 2B, spill generalizzato): verso un kernel spill-capable
                    // la quota governor dei batch drenati e' rilasciata
                    // subito. La soglia di attivazione dello spill ha la
                    // stessa grandezza del budget (byte stimati dell'input vs
                    // `max_governed_memory_bytes`): trattenere i lease renderebbe lo
                    // spill irraggiungibile (fail-fast al drenaggio, prima
                    // del dispatch). La memoria di lavoro dell'operatore e'
                    // auto-limitata dallo spill su disco; approssimazione v1:
                    // la materializzazione dell'input resta in RAM (lo spill
                    // in streaming durante il drenaggio e' M3).
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
fn panic_step_error(kernel: &PreparedKernel, payload: &(dyn std::any::Any + Send)) -> PlenoraError {
    // Il testo del panico NON viene pubblicato: non e' scritto da noi e puo'
    // contenere i valori che un `assert` di una dipendenza ha confrontato,
    // cioe' dati della riga. Si riporta solo la forma del payload; il nodo e
    // l'operazione, che sono la vera informazione diagnostica, li aggiunge
    // `step_error`.
    let forma = plenora_core::panic_policy::forma_payload(payload);
    // Categoria `Internal`, non `InvalidPlan`. Un panico dentro un kernel e'
    // un difetto NOSTRO — o di una dipendenza che usiamo — e il chiamante non
    // ha nulla da correggere nel proprio piano. Finche' `step_error`
    // avvolgeva tutto in `Execution` la classificazione qui sotto era
    // invisibile; da quando le categorie si conservano, dire `invalid_plan`
    // manderebbe chi legge a cercare un errore che non ha commesso.
    step_error(
        kernel,
        PlenoraError::Internal(format!("panic nel kernel: {forma}")),
    )
}

/// Lato di un'operazione binaria geo (campo strutturato del carrier D14.5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeoBinarySide {
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
struct GeoBinaryStepError {
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
fn geo_binary_step_error(
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
fn record_kernel_metrics(
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

/// Sequenza logica riassegnata all'output di un segmento blocking
/// (architettura.md#determinismo): la cardinalita' cambia (concatenazione + kernel una tantum),
/// quindi la sequenza degli input non e' propagabile 1:1.
///
/// Regola v1 — deterministica, per ordine di scansione seriale: il segmento
/// blocking emette UN batch per esecuzione (per i binari il contenuto
/// dipende dalla scansione left-then-right, anch'essa seriale), quindi la
/// sequenza e' sempre `source_node` = nodo del kernel, `input_partition` =
/// 0, `sequence_number` = 0. Oggi nessun consumatore riordina: la sequenza
/// e' osservabilita' e predisposizione per il collect indicizzato di M3.
fn blocking_output_sequence(kernel: &PreparedKernel) -> BatchSequence {
    BatchSequence {
        source_node: kernel.node_id.clone(),
        input_partition: 0,
        sequence_number: 0,
    }
}

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
fn run_streaming_chain(
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

/// Lunghezza del gruppo di fusione geo che si apre a `position` (0 se il
/// kernel non apre un gruppo, architettura.md#geometrie): i membri condividono l'id assegnato
/// in `prepare` e l'apertura e' il primo membro del run.
fn fusion_group_len(kernels: &[PreparedKernel], position: usize) -> usize {
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
fn fused_group_decoded_bytes(batch: &RecordBatch, kernel: &PreparedKernel) -> Option<u64> {
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
struct FusedAttempt<'a> {
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
fn try_run_fused_group(
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
        match &kernel.config {
            PreparedConfig::GeoTransform(kernel_params) => params.push(kernel_params),
            _ => {
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
    // architettura.md#geometrie M3: lo schema di output del gruppo e' quello dell'ULTIMA
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
fn fused_group_terminal(
    kernels: &[PreparedKernel],
) -> (&[PreparedKernel], Option<FusedTerminal<'_>>) {
    let PreparedConfig::GeoMeasure { measure, .. } = &kernels[kernels.len() - 1].config else {
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
static PANIC_AT_NODES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Iniezione del panic di test: scatta solo ai nodi registrati nell'hook.
#[cfg(test)]
fn inject_test_panic(node_id: &str) {
    if PANIC_AT_NODES
        .lock()
        .expect("hook panic non avvelenato")
        .iter()
        .any(|node| node == node_id)
    {
        panic!("panic di test iniettato al nodo `{node_id}`");
    }
}

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
/// DAG parallelo restano Fase 2B.
fn run_kernel(
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
/// dell'esecuzione (architettura.md#memoria, Fase 2B, spill generalizzato): `sort`/`distinct`/`aggregate`
/// sopra la soglia di spill scrivono nel `TempStore` e le loro metriche sono
/// accumulate in [`ExecState`].
fn dispatch_kernel(
    kernel: &PreparedKernel,
    batch: RecordBatch,
    state: &ExecState,
) -> Result<RecordBatch> {
    match &kernel.config {
        PreparedConfig::TableUnary(plan) => {
            let (output, spill_metrics) = table_engine::execute_batch_with_spill_row_diagnostics(
                batch,
                plan,
                Some(state.spill_directory()),
            )
            .map_err(|error| step_error(kernel, error))?;
            state.add_spill_metrics(spill_metrics);
            Ok(output)
        }
        PreparedConfig::TableBinary(_) => Err(PlenoraError::Internal(format!(
            "nodo `{}`: kernel binario in una catena streaming",
            kernel.node_id
        ))),
        PreparedConfig::GeoBinary(_) => Err(PlenoraError::Internal(format!(
            "nodo `{}`: kernel binario geo in una catena streaming",
            kernel.node_id
        ))),
        PreparedConfig::GeoTransform(params) => geo_transform_batch(kernel, &batch, params, state),
        PreparedConfig::GeoMeasure { measure, .. } => geo_measure_batch(kernel, &batch, *measure),
        PreparedConfig::GeoFromWkt {
            wkt_column_index,
            on_error,
        } => geo_from_wkt_batch(kernel, &batch, *wkt_column_index, *on_error),
        PreparedConfig::GeoAccessors { columns } => geo_accessors_batch(kernel, &batch, columns),
        PreparedConfig::GeoLineLocatePoint {
            point,
            output_column,
        } => geo_line_locate_point_batch(kernel, &batch, point, output_column),
        PreparedConfig::GeoSubdivide { max_vertices } => {
            geo_subdivide_batch(kernel, &batch, *max_vertices)
        }
        PreparedConfig::GeoSnap {
            reference,
            tolerance,
        } => geo_snap_batch(kernel, &batch, reference, *tolerance),
        PreparedConfig::GeoCollect { group_by_indices } => {
            geo_collect_batch(kernel, &batch, group_by_indices)
        }
        PreparedConfig::GeoGenerateGrid {
            extent,
            cell_size,
            shape,
        } => geo_generate_grid_batch(kernel, extent, *cell_size, *shape),
        PreparedConfig::GeoCoverageValidate {
            tolerance,
            max_issues,
        } => geo_coverage_validate_batch(kernel, &batch, *tolerance, *max_issues),
        PreparedConfig::GeoSharedPaths {
            tolerance,
            min_length,
        } => geo_shared_paths_batch(kernel, &batch, *tolerance, *min_length),
        PreparedConfig::GeoClusterDbscan {
            eps,
            min_points,
            output_column,
        } => geo_cluster_dbscan_batch(kernel, &batch, *eps, *min_points, output_column),
    }
}

/// Trasformazione geo 1:1 in place via `geo_transport` (per batch, senza
/// envelope): i parametri sono tipizzati e risolti da `prepare` (configurazioni preparate);
/// indice di colonna e schema di output arrivano dall'handle prepared del
/// nodo, costruito una volta per esecuzione (hot path minimale).
fn geo_transform_batch(
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
fn geo_measure_batch(
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
fn measure_f64_raw(
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
fn measure_row_diagnostics(rows: &[(u64, &'static str)]) -> RowDiagnostics {
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
// Estensioni geo v1.1-v1.3 (dispatch dedicato sugli adapter Arrow dei kernel)
// ---------------------------------------------------------------------------

/// Celle WKB della colonna geometria attiva del batch (indice risolto in
/// `prepare`, hot path minimale), con errore attribuito al nodo logico.
fn kernel_geometry_cells<'a>(
    kernel: &PreparedKernel,
    batch: &'a RecordBatch,
) -> Result<&'a BinaryArray> {
    let geometry_index = kernel.geometry_column_index.ok_or_else(|| {
        step_error(
            kernel,
            PlenoraError::Schema("op geo senza colonna geometria".into()),
        )
    })?;
    let geometry_name = kernel.input_contracts[0]
        .active_geometry_column()
        .map_or("geometry", |geometry| geometry.name.as_str());
    batch_geometry_cells(batch, geometry_index, geometry_name)
        .map_err(|error| step_error(kernel, error))
}

/// Indice risolto della colonna geometria attiva (hot path minimale), con errore di nodo.
fn kernel_geometry_index(kernel: &PreparedKernel) -> Result<usize> {
    kernel.geometry_column_index.ok_or_else(|| {
        step_error(
            kernel,
            PlenoraError::Schema("op geo senza colonna geometria".into()),
        )
    })
}

/// Batch con una colonna aggiunta in coda: lo schema e' quello del contratto
/// di output inferito dal planner (fonte unica di verita', configurazioni preparate).
fn append_output_column(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    column: ArrayRef,
) -> Result<RecordBatch> {
    let mut columns = batch.columns().to_vec();
    columns.push(column);
    let righe = batch.num_rows();
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.from_wkt` (streaming 1:1): colonna WKT → nuova colonna geometria.
fn geo_from_wkt_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    wkt_column_index: usize,
    on_error: plenora_kernels_geo::extensions::OnWktError,
) -> Result<RecordBatch> {
    let values = batch
        .column(wkt_column_index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| step_error(kernel, PlenoraError::Schema("colonna WKT non Utf8".into())))?;
    let column_name = batch.schema().field(wkt_column_index).name().clone();
    let cells = plenora_kernels_geo::extensions::from_wkt_column_named(
        values,
        on_error,
        Some(&column_name),
    )
    .map_err(|error| step_error(kernel, error))?;
    let geometry = BinaryArray::from(cells.iter().map(|cell| cell.as_deref()).collect::<Vec<_>>());
    append_output_column(kernel, batch, std::sync::Arc::new(geometry))
}

/// `geo.geometry_accessors` (streaming 1:1): colonne accessorie per riga;
/// geometria null → tutti gli accessori null.
fn geo_accessors_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    columns: &[(String, AccessorKind)],
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let mut accessors: Vec<Option<plenora_kernels_geo::extensions::GeometryAccessors>> =
        Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        if cells.is_null(row) {
            accessors.push(None);
            continue;
        }
        let geometry =
            decode_geometry_cell(cells.value(row)).map_err(|error| step_error(kernel, error))?;
        let values = plenora_kernels_geo::extensions::geometry_accessors(&geometry)
            .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
        accessors.push(Some(values));
    }
    let mut produced: Vec<ArrayRef> = Vec::with_capacity(columns.len());
    for (_, kind) in columns {
        let column: ArrayRef = match kind {
            AccessorKind::GeometryType => std::sync::Arc::new(StringArray::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().map(|access| access.geometry_type))
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::NumGeometries => std::sync::Arc::new(UInt64Array::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().map(|access| access.num_geometries))
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::NumInteriorRings => std::sync::Arc::new(UInt64Array::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().map(|access| access.num_interior_rings))
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::StartPoint => std::sync::Arc::new(StringArray::from(
                accessors
                    .iter()
                    .map(|access| {
                        access
                            .as_ref()
                            .and_then(|access| access.start_point.as_deref())
                    })
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::EndPoint => std::sync::Arc::new(StringArray::from(
                accessors
                    .iter()
                    .map(|access| {
                        access
                            .as_ref()
                            .and_then(|access| access.end_point.as_deref())
                    })
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::IsClosed => std::sync::Arc::new(BooleanArray::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().map(|access| access.is_closed))
                    .collect::<Vec<_>>(),
            )),
        };
        produced.push(column);
    }
    let mut all_columns = batch.columns().to_vec();
    all_columns.extend(produced);
    let righe = batch.num_rows();
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), all_columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.line_locate_point` (streaming 1:1 "add column"): frazione [0,1] del
/// punto di riferimento sulla linea; null per geometrie null o non-linee.
fn geo_line_locate_point_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    point: &geo::Point<f64>,
    output_column: &str,
) -> Result<RecordBatch> {
    let _ = output_column; // Il nome e' gia' nel contratto di output usato per lo schema.
    let cells = kernel_geometry_cells(kernel, batch)?;
    let mut values: Vec<Option<f64>> = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        if cells.is_null(row) {
            values.push(None);
            continue;
        }
        let geometry =
            decode_geometry_cell(cells.value(row)).map_err(|error| step_error(kernel, error))?;
        let fraction = plenora_kernels_geo::extensions::line_locate_point(&geometry, point)
            .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
        values.push(fraction);
    }
    append_output_column(
        kernel,
        batch,
        std::sync::Arc::new(Float64Array::from(values)),
    )
}

/// `geo.subdivide` (streaming OneToMany): espansione 1:N per batch con
/// `__parent_index` di lineage (come `explode`); riga con geometria null
/// produce una riga con geometria null.
fn geo_subdivide_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    max_vertices: usize,
) -> Result<RecordBatch> {
    let geometry_index = kernel_geometry_index(kernel)?;
    let cells = kernel_geometry_cells(kernel, batch)?;
    let mut parent_index: Vec<u64> = Vec::new();
    let mut parts: Vec<Option<Vec<u8>>> = Vec::new();
    for row in 0..batch.num_rows() {
        let row_index = row as u64;
        if cells.is_null(row) {
            parent_index.push(row_index);
            parts.push(None);
            continue;
        }
        let pieces =
            plenora_kernels_geo::extensions2::subdivide_wkb(cells.value(row), max_vertices)
                .map_err(|error| step_error(kernel, error))?;
        for piece in pieces {
            parent_index.push(row_index);
            parts.push(Some(piece));
        }
    }
    let indices = UInt64Array::from(parent_index);
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(batch.num_columns() + 1);
    for (index, column) in batch.columns().iter().enumerate() {
        if index == geometry_index {
            columns.push(std::sync::Arc::new(BinaryArray::from(
                parts.iter().map(|part| part.as_deref()).collect::<Vec<_>>(),
            )));
        } else {
            columns.push(
                take(column.as_ref(), &indices, None)
                    .map_err(|error| step_error(kernel, PlenoraError::from(error)))?,
            );
        }
    }
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    columns.push(std::sync::Arc::new(indices));
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.snap` (streaming 1:1 in place): vertici agganciati al riferimento
/// entro tolleranza; schema invariato.
fn geo_snap_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    reference: &geo::Geometry<f64>,
    tolerance: f64,
) -> Result<RecordBatch> {
    let geometry_index = kernel_geometry_index(kernel)?;
    let cells = kernel_geometry_cells(kernel, batch)?;
    let snapped = plenora_kernels_geo::extensions2::snap_column(cells, reference, tolerance)
        .map_err(|error| step_error(kernel, error))?;
    let mut columns = batch.columns().to_vec();
    columns[geometry_index] = std::sync::Arc::new(BinaryArray::from(
        snapped
            .iter()
            .map(|cell| cell.as_deref())
            .collect::<Vec<_>>(),
    ));
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.collect` (blocking, ManyToOne): raggruppamento canonico per chiavi
/// (ordine lessicografico della chiave testuale, come `table.aggregate`) e
/// collezione delle geometrie del gruppo in ordine di input.
fn geo_collect_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    group_by_indices: &[usize],
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let geometries = plenora_kernels_geo::arrow_adapter::map_nullable(cells, |payload| {
        decode_geometry_cell(payload).map(Some)
    })
    .map_err(|error| step_error(kernel, error))?;
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for row in 0..batch.num_rows() {
        let mut key = String::new();
        for &index in group_by_indices {
            let column = batch.column(index);
            key.push_str(&column.data_type().to_string());
            key.push('\u{1e}');
            match plenora_kernels_table::scalar_as_string(column.as_ref(), row)
                .map_err(|error| step_error(kernel, error))?
            {
                Some(value) => {
                    key.push('1');
                    key.push_str(&value.len().to_string());
                    key.push(':');
                    key.push_str(&value);
                }
                None => key.push('0'),
            }
            key.push('\u{1f}');
        }
        groups.entry(key).or_default().push(row);
    }
    let mut collected: Vec<Option<Vec<u8>>> = Vec::with_capacity(groups.len());
    let mut representatives: Vec<u64> = Vec::with_capacity(groups.len());
    for rows in groups.values() {
        let group: Vec<Option<geo::Geometry<f64>>> =
            rows.iter().map(|&row| geometries[row].clone()).collect();
        let geometry = plenora_kernels_geo::extensions::collect_geometries(&group)
            .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
        collected.push(match &geometry {
            Some(geometry) => Some(
                plenora_kernels_geo::arrow_adapter::encode_geometry(geometry)
                    .map_err(|error| step_error(kernel, error))?,
            ),
            None => None,
        });
        representatives.push(rows[0] as u64);
    }
    let indices = UInt64Array::from(representatives);
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(group_by_indices.len() + 1);
    columns.push(std::sync::Arc::new(BinaryArray::from(
        collected
            .iter()
            .map(|cell| cell.as_deref())
            .collect::<Vec<_>>(),
    )));
    for &index in group_by_indices {
        columns.push(
            take(batch.column(index).as_ref(), &indices, None)
                .map_err(|error| step_error(kernel, PlenoraError::from(error)))?,
        );
    }
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.generate_grid` (blocking, generativa): l'input funge da trigger; lo
/// schema di output (con o senza centroidi) decide le colonne prodotte.
fn geo_generate_grid_batch(
    kernel: &PreparedKernel,
    extent: &plenora_kernels_geo::extensions2::GridExtent,
    cell_size: f64,
    shape: plenora_kernels_geo::extensions2::GridShape,
) -> Result<RecordBatch> {
    let rows = plenora_kernels_geo::extensions2::generate_grid_rows(extent, cell_size, shape)
        .map_err(|error| step_error(kernel, error))?;
    let include_centroid = kernel
        .output_contract
        .schema
        .field_with_name("centroid_x")
        .is_ok();
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(5);
    columns.push(std::sync::Arc::new(BinaryArray::from(
        rows.iter()
            .map(|row| row.wkb.as_slice())
            .collect::<Vec<_>>(),
    )));
    columns.push(std::sync::Arc::new(UInt64Array::from_iter_values(
        rows.iter().map(|row| row.cell_i),
    )));
    columns.push(std::sync::Arc::new(UInt64Array::from_iter_values(
        rows.iter().map(|row| row.cell_j),
    )));
    if include_centroid {
        columns.push(std::sync::Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|row| row.centroid_x),
        )));
        columns.push(std::sync::Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|row| row.centroid_y),
        )));
    }
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.coverage_validate` (blocking, WholeToMany): una riga per overlap.
fn geo_coverage_validate_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    tolerance: f64,
    max_issues: usize,
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let rows =
        plenora_kernels_geo::extensions3::coverage_validate_rows(cells, tolerance, max_issues)
            .map_err(|error| step_error(kernel, error))?;
    let columns: Vec<ArrayRef> = vec![
        std::sync::Arc::new(StringArray::from(
            rows.iter().map(|row| row.issue_type).collect::<Vec<_>>(),
        )),
        std::sync::Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.index_a),
        )),
        std::sync::Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.index_b),
        )),
        std::sync::Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|row| row.area),
        )),
        std::sync::Arc::new(BinaryArray::from(
            rows.iter()
                .map(|row| row.wkb.as_slice())
                .collect::<Vec<_>>(),
        )),
    ];
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.shared_paths` (blocking, WholeToMany): una riga per coppia con
/// confine condiviso.
fn geo_shared_paths_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    tolerance: f64,
    min_length: f64,
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let rows = plenora_kernels_geo::extensions3::shared_paths_rows(cells, tolerance, min_length)
        .map_err(|error| step_error(kernel, error))?;
    let columns: Vec<ArrayRef> = vec![
        std::sync::Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.index_a),
        )),
        std::sync::Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.index_b),
        )),
        std::sync::Arc::new(Float64Array::from_iter_values(
            rows.iter().map(|row| row.shared_length),
        )),
        std::sync::Arc::new(BinaryArray::from(
            rows.iter()
                .map(|row| row.wkb.as_slice())
                .collect::<Vec<_>>(),
        )),
    ];
    let righe = columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        .map_err(|error| step_error(kernel, error))
}

/// `geo.cluster_dbscan` (blocking, output allineato alle righe): etichetta
/// `UInt64` nullable per riga (noise → null).
fn geo_cluster_dbscan_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    eps: f64,
    min_points: usize,
    output_column: &str,
) -> Result<RecordBatch> {
    let _ = output_column; // Il nome e' gia' nel contratto di output usato per lo schema.
    let cells = kernel_geometry_cells(kernel, batch)?;
    let labels = plenora_kernels_geo::cluster::dbscan_column(cells, eps, min_points)
        .map_err(|error| step_error(kernel, error))?;
    append_output_column(
        kernel,
        batch,
        std::sync::Arc::new(UInt64Array::from(labels)),
    )
}

/// Il kernel di un segmento blocking unario e' spill-capable (architettura.md#memoria,
/// Fase 2B, spill generalizzato): `table.sort`/`distinct`/`aggregate` hanno la variante
/// `*_spilled` in kernels-table (cfr. `table_engine::unary_spill_capable`).
fn spill_capable_unary(kernel: &PreparedKernel) -> bool {
    matches!(
        &kernel.config,
        PreparedConfig::TableUnary(table_plan) if table_engine::unary_spill_capable(table_plan)
    )
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
fn run_blocking(
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
    // architettura.md#memoria (Fase 2B, spill generalizzato): se il kernel spillera' — stessa soglia
    // deterministica valutata al dispatch tabellare (`should_spill_unary`
    // sui byte stimati dell'input), stessi limiti — l'intermedio
    // concatenato NON consuma quota governor: la memoria di lavoro
    // dell'operatore e' auto-limitata dallo spill su disco e la
    // reservation fallirebbe per costruzione (la soglia ha la stessa
    // grandezza del budget). Altrimenti reservation completa
    // dell'intermedio prima di rilasciare i lease degli input (architettura.md#memoria:
    // mai attesa con reservation parziale).
    let spill_path = match &kernel.config {
        PreparedConfig::TableUnary(table_plan) if table_engine::unary_spill_capable(table_plan) => {
            plenora_kernels_table::spill::should_spill_unary(&full, table_plan.limits())
        }
        _ => false,
    };
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
fn run_binary_blocking(
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
    // poteva essere pubblicato con il lock ormai stantio.
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
    if let PreparedConfig::GeoBinary(geo_plan) = &kernel.config {
        return run_geo_binary_blocking(
            plan,
            segment_index,
            state,
            geo_plan,
            left_batches,
            right_batches,
        );
    }
    let PreparedConfig::TableBinary(binary_plan) = &kernel.config else {
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

/// Ramo geo di [`run_binary_blocking`] (architettura.md#geometrie, D14.2): stesso
/// guscio del ramo tabellare — drenaggio dei due rami a monte,
/// `concat_batches`, tetti in byte per batch, reservation in ordine globale fisso
/// left→right, cancellazione post-drenaggio, `catch_unwind`,
/// `check_join_expansion`, `blocking_output_sequence`, metriche — con il
/// cuore sostituito da decode totale D14.3 → kernel `*_validated` → output
/// secondo il contratto v4 ([`execute_geo_binary`]).
///
/// Cancellazione `BoundaryOnly` (catalogo, D14.5.5): i confini sono quelli
/// esistenti (batch in drenaggio + post-drenaggio pre-kernel), nessun check
/// dentro il kernel — comportamento voluto, non lacuna.
///
/// Contabilita' D14.4: per ciascun lato, nell'ordine globale fisso
/// left→right — reservation dei byte Arrow, preflight della forma
/// decodificata ([`preflight_decoded_bytes`]), reservation della forma
/// decodificata, decode. Rifiuto fail-closed PRIMA dell'allocazione: una
/// reservation rifiutata ferma il nodo senza partial state (i lease sono
/// RAII). Il lease Arrow right e' rilasciato prima del kernel (il left
/// resta per take/passthrough); i lease decodificati dopo il lease
/// dell'output.
///
/// Errori D14.5: decode → fase `Read` con side/riga strutturati,
/// kernel e costruzione output → fase `Write` (carrier
/// [`GeoBinaryStepError`], conversione [`geo_binary_step_error`]); il primo
/// errore e' in ordine (side, riga) per costruzione della sequenza.
// La lunghezza e' data dal guscio architettura.md#memoria completo (concat, reservation,
// metriche) piu' la sequenza D14.4 per lato: sequenza lineare, non
// complessita' logica (stesso criterio di `pair_arrow`).
#[allow(clippy::too_many_lines)]
fn run_geo_binary_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    geo_plan: &GeoBinaryPlan,
    left_batches: Vec<GovernedBatch>,
    right_batches: Vec<GovernedBatch>,
) -> Result<GovernedBatch> {
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().ok_or_else(|| {
        PlenoraError::Internal(
            "segmento binario senza kernel: invariante del planner violata".into(),
        )
    })?;
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
    // Come nel ramo tabellare: tetto duro in byte per batch sui batch concatenati; i byte
    // restituiti alimentano le reservation (hot path minimale: un solo conteggio).
    let left_bytes = check_batch_bytes(state, &left, &kernel.node_id)?;
    let right_bytes = check_batch_bytes(state, &right, &kernel.node_id)?;
    // I batch d'ingresso non servono piu' (il concat possiede i nuovi
    // buffer): i loro lease tornano al governor prima delle reservation.
    drop(left_batches);
    drop(right_batches);
    // D14.4: per ciascun lato, in ordine globale fisso left→right —
    // reservation Arrow, preflight della forma decodificata, reservation
    // decodificata, decode. Il decode left completo precede qualunque
    // contabilita' del lato right: il primo errore e' in ordine (side,
    // riga) per costruzione (D14.5.3).
    let left_lease = state.governor.reserve(left_bytes, &kernel.node_id)?;
    let left_decoded_bytes = preflight_decoded_bytes(
        &left.schema(),
        std::slice::from_ref(&left),
        geo_plan.left_geometry_index,
    );
    let left_decoded_lease = state
        .governor
        .reserve(left_decoded_bytes, &kernel.node_id)?;
    let left_geometries = decode_geometry_batches(
        &left.schema(),
        std::slice::from_ref(&left),
        geo_plan.left_geometry_index,
    )
    .map_err(|error| {
        geo_binary_step_error(
            state,
            kernel,
            GeoBinaryStepError {
                phase: ErrorPhase::Read,
                side: Some(GeoBinarySide::Left),
                row_index: error.row_index,
                source: PlenoraError::InvalidPlan(error.source.to_string()),
            },
        )
    })?;
    let right_lease = state.governor.reserve(right_bytes, &kernel.node_id)?;
    let right_decoded_bytes = preflight_decoded_bytes(
        &right.schema(),
        std::slice::from_ref(&right),
        geo_plan.right_geometry_index,
    );
    let right_decoded_lease = state
        .governor
        .reserve(right_decoded_bytes, &kernel.node_id)?;
    let right_geometries = decode_geometry_batches(
        &right.schema(),
        std::slice::from_ref(&right),
        geo_plan.right_geometry_index,
    )
    .map_err(|error| {
        geo_binary_step_error(
            state,
            kernel,
            GeoBinaryStepError {
                phase: ErrorPhase::Read,
                side: Some(GeoBinarySide::Right),
                row_index: error.row_index,
                source: PlenoraError::InvalidPlan(error.source.to_string()),
            },
        )
    })?;
    // D14.4: lease Arrow RIGHT rilasciato prima del kernel (il left resta
    // per take/passthrough); il batch right non serve piu'.
    drop(right_lease);
    drop(right);
    // errori-e-limiti.md / D14.5.5: a fine drenaggio, prima del kernel binario
    // monolitico (come `run_blocking` e il ramo tabellare).
    state.check_cancellation(kernel)?;
    let start = Instant::now();
    // Confine di panic policy (errori-e-limiti.md#panic-policy, D14.5.6)
    // come il ramo tabellare: panic del kernel
    // convertito in errore `Execution` attribuito al nodo, mai publish
    // dopo panic; hook `PANIC_AT_NODES` esteso al ramo.
    let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        inject_test_panic(&kernel.node_id);
        execute_geo_binary(kernel, geo_plan, &left, &left_geometries, &right_geometries)
    }))
    .unwrap_or_else(|payload| Err(panic_step_error(kernel, &*payload)))
    .map_err(|source| {
        geo_binary_step_error(
            state,
            kernel,
            GeoBinaryStepError {
                phase: ErrorPhase::Write,
                side: None,
                row_index: None,
                source,
            },
        )
    })?;
    let elapsed = start.elapsed();
    let output_lease = state
        .governor
        .reserve(output.get_array_memory_size() as u64, &kernel.node_id)?;
    // D14.4: lease decodificati rilasciati dopo il lease dell'output; anche
    // il lease Arrow left (take/passthrough completati nel kernel).
    drop(left_decoded_lease);
    drop(right_decoded_lease);
    drop(left_lease);
    let rows_out = output.num_rows() as u64;
    state.add_node_rows_out(&kernel.node_id, rows_out);
    // errori-e-limiti.md: il vincolo relativo vincolante di catalogo si applica come nel
    // ramo tabellare; il tetto assoluto D14.6 e' gia' stato fatto rispettare
    // dal kernel (`max_pairs`/`max_results` risolti in prepare dai limiti
    // effettivi del piano, prima della materializzazione completa).
    check_join_expansion(state, kernel, left_rows, right_rows, rows_out)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    // Metriche per nodo: righe in = left + right, batch in = quelli reali
    // drenati dai due rami (stesso blocco del ramo tabellare).
    // errori-e-limiti.md: heartbeat del TempStore al punto centrale.
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

/// Cuore del ramo geo (D14.2): kernel `*_validated` (architettura.md#geometrie:
/// precondizione soddisfatta per costruzione dal decode totale D14.3,
/// eseguito dal chiamante) e costruzione dell'output secondo il contratto
/// v4:
///
/// - `sjoin`: `take` delle colonne left sugli indici di coppia + colonna
///   `right_index` (non-null per inner join);
/// - `nearest`: come sjoin + colonna `distance` (una riga per match, gia'
///   filtrato su `max_distance` dal kernel);
/// - `within`/`count_points_in_polygons`: left passthrough + colonna
///   flag/conteggio allineata alle righe left (null dove la geometria left
///   e' null).
///
/// Lo schema di output e' quello del contratto inferito dal planner
/// (fonte unica di verita', configurazioni preparate) — identico per costruzione allo schema del
/// contratto analyze v4, verificato dai test di identita'.
///
/// Gli errori propagano come sorgente grezza: fase (`Write`), side e riga
/// sono aggiunti dal chiamante nel carrier [`GeoBinaryStepError`] (D14.5.2).
// La lunghezza e' data dalla sequenza lineare dei quattro casi del
// perimetro della fusione (kernel e costruzione output per op), non da
// complessita'
// logica (stesso criterio di `pair_arrow`).
#[allow(clippy::too_many_lines)]
fn execute_geo_binary(
    kernel: &PreparedKernel,
    geo_plan: &GeoBinaryPlan,
    left: &RecordBatch,
    left_geometries: &[Option<geo::Geometry<f64>>],
    right_geometries: &[Option<geo::Geometry<f64>>],
) -> Result<RecordBatch> {
    match geo_plan.operation {
        PairOperation::SJoin => {
            let pairs = spatial_join_nullable_validated(
                left_geometries,
                right_geometries,
                geo_plan.predicate.ok_or_else(|| {
                    PlenoraError::Internal(
                        "sjoin senza predicato: invariante di prepare violata".into(),
                    )
                })?,
                geo_plan.max_pairs,
            )
            .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))?;
            let left_indices = UInt64Array::from_iter_values(pairs.iter().map(|pair| pair.left));
            let mut columns: Vec<ArrayRef> = Vec::with_capacity(left.num_columns() + 1);
            for column in left.columns() {
                columns
                    .push(take(column.as_ref(), &left_indices, None).map_err(PlenoraError::from)?);
            }
            columns.push(std::sync::Arc::new(UInt64Array::from_iter_values(
                pairs.iter().map(|pair| pair.right),
            )));
            let righe = columns
                .first()
                .map_or(0, plenora_core::arrow::array::Array::len);
            plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        }
        PairOperation::Nearest => {
            let matches = nearest_matches_validated(
                left_geometries,
                right_geometries,
                geo_plan.max_distance,
                geo_plan.max_comparisons,
                geo_plan.max_results,
            )
            .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))?;
            let left_indices = UInt64Array::from_iter_values(matches.iter().map(|m| m.left));
            let mut columns: Vec<ArrayRef> = Vec::with_capacity(left.num_columns() + 2);
            for column in left.columns() {
                columns
                    .push(take(column.as_ref(), &left_indices, None).map_err(PlenoraError::from)?);
            }
            columns.push(std::sync::Arc::new(UInt64Array::from_iter_values(
                matches.iter().map(|m| m.right),
            )));
            columns.push(std::sync::Arc::new(Float64Array::from_iter_values(
                matches.iter().map(|m| m.distance),
            )));
            let righe = columns
                .first()
                .map_or(0, plenora_core::arrow::array::Array::len);
            plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        }
        PairOperation::Within => {
            let indexes =
                within_indexes_validated(left_geometries, right_geometries, geo_plan.max_pairs)
                    .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))?;
            let matched: std::collections::HashSet<u64> = indexes.into_iter().collect();
            let flags: Vec<Option<bool>> = left_geometries
                .iter()
                .enumerate()
                .map(|(index, geometry)| {
                    geometry.as_ref().map(|_| matched.contains(&(index as u64)))
                })
                .collect();
            let mut columns = left.columns().to_vec();
            columns.push(std::sync::Arc::new(BooleanArray::from(flags)));
            let righe = columns
                .first()
                .map_or(0, plenora_core::arrow::array::Array::len);
            plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        }
        PairOperation::CountPointsInPolygons => {
            // Contratto: left = poligoni (output allineato), right = punti.
            let counts = count_points_in_polygons_validated(
                left_geometries,
                right_geometries,
                geo_plan.max_pairs,
            )
            .map_err(|error| PlenoraError::InvalidPlan(error.to_string()))?;
            let values: Vec<Option<u64>> = counts
                .iter()
                .enumerate()
                .map(|(index, count)| left_geometries[index].as_ref().map(|_| *count))
                .collect();
            let mut columns = left.columns().to_vec();
            columns.push(std::sync::Arc::new(UInt64Array::from(values)));
            let righe = columns
                .first()
                .map_or(0, plenora_core::arrow::array::Array::len);
            plenora_core::batch_with_rows(kernel.output_contract.schema.clone(), columns, righe)
        }
        _ => Err(PlenoraError::Internal(
            "op binaria geo fuori perimetro M1: invariante di prepare violata".into(),
        )),
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
