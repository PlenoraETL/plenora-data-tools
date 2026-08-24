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

mod input;
mod metrics;

pub use input::{Input, Inputs};
use metrics::{accumulate, accumulate_time, sum_rows};
pub use metrics::{ExecutionMetrics, NodeMetrics, SegmentMetrics};

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, RecordBatch, StringArray,
    UInt32Array, UInt64Array,
};
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::{FileWriter, StreamWriter};
use plenora_core::arrow::schema::{Schema, SchemaRef};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::arrow::select::take::take;
use plenora_core::catalog::{
    find_operation, CancellationBehavior, ExpansionConstraint, JoinExpansion, CATALOG,
};
use plenora_core::contract::{
    BatchSequence, ContractCrs, DataContract, GeometryDimensions, GeometryEncoding,
};
use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::error::ReplayedError;
use plenora_core::{ErrorCategory, ErrorPhase, PlenoraError, Result, RetryDisposition};
use plenora_kernels_geo::analysis::{
    count_points_in_polygons_validated, nearest_matches_validated, within_indexes_validated,
};
use plenora_kernels_geo::arrow_adapter::{
    batch_geometry_cells, canonical_geometry_encoding, canonical_geometry_metadata,
    canonical_geometry_srid, canonical_schema_version_metadata, decode_geometry_cell,
    strip_decided_crs_declarations, GeometryMetadataDetails, PLENORA_GEOMETRY_AXIS_ORDER_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_SRID_KEY,
};
use plenora_kernels_geo::spatial_join::spatial_join_nullable_validated;
use plenora_kernels_geo::{operations, validate_wkb_transport_for_dimensions_with_depth};
use plenora_kernels_table::spill::SpillMetrics;

use crate::cancellation::CancellationToken;
use crate::geo_transport::pair::{decode_geometry_batches, preflight_decoded_bytes, PairOperation};
use crate::geo_transport::publish::{publish_with_profile, PublishOutcome, PublishProfile};
use crate::geo_transport::transport::{
    one_to_one_batch_prepared, prepare_one_to_one, OneToOnePrepared, TransformArrowSchema,
};
use crate::geo_transport::unary::{
    one_to_one_batch_fused, FusedStepError, FusedTerminal, FusedTerminalMeasure,
};
use crate::governor::{
    GovernedBatch, MemoryGovernor, MemoryLease, MemoryPermit, ReservationResult,
};
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
// Stato condiviso dell'esecuzione (seriale, thread-locale)
// ---------------------------------------------------------------------------

/// Stato mutabile condiviso tra le chiusure dello stream (contatori per i
/// limiti effettivi e metriche). `Rc`/`RefCell`: esecuzione seriale v1 (parallelismo solo dove conviene).
struct ExecState {
    plan: Rc<ExecutionPlan>,
    metrics: RefCell<ExecutionMetrics>,
    /// Governor del budget memoria globale di piano (architettura.md#memoria).
    governor: MemoryGovernor,
    /// Identita' dell'esecuzione (errori-e-limiti.md, errori arricchiti): riportata negli errori
    /// `Execution`/`Cancelled` e nel lock del `TempStore`.
    execution_id: String,
    /// Token di cancellazione cooperativa (errori-e-limiti.md#cancellazione):
    /// osservato solo ai
    /// confini dell'executor, mai dentro ai kernel (M3).
    cancellation: CancellationToken,
    /// Diagnostica opt-in (errori-e-limiti.md, errori arricchiti): arricchisce le motivazioni degli
    /// errori con contesto strutturale, mai valori.
    diagnostics: bool,
    /// Store temporaneo dell'esecuzione (errori-e-limiti.md): heartbeat al punto
    /// centrale, cleanup RAII al `Drop`.
    temp_store: RefCell<TempStore>,
    /// Directory di spill condivisa (architettura.md#memoria, Fase 2B, spill generalizzato): `spill/`
    /// sotto il `TempStore`, risolta UNA volta alla costruzione (hot path minimale) —
    /// il path e' fisso per tutta l'esecuzione.
    spill_directory: PathBuf,
    /// Metriche di spill aggregate (architettura.md#memoria, Fase 2B, spill generalizzato): alimentate dai
    /// percorsi `*_spilled` attivati nei nodi tabellari.
    spill_metrics: RefCell<SpillMetrics>,
    /// Istante dell'ultimo heartbeat scritto (throttle).
    last_heartbeat: Cell<Instant>,
    /// Istante del PRIMO fallimento di una serie consecutiva di heartbeat,
    /// azzerato al primo successo. `None` significa «l'ultimo tentativo e'
    /// andato bene», non «non ci sono stati tentativi»: distinguere i due
    /// casi conta, perche' un'operazione lunga puo' non chiamare l'heartbeat
    /// per minuti senza che nulla sia rotto.
    heartbeat_fallito_da: Cell<Option<Instant>>,
    /// Righe/batch/byte cumulati per input (`max_input_rows`, `max_batches`,
    /// `max_payload_bytes`).
    input_counts: RefCell<HashMap<String, (u64, u64, u64)>>,
    /// Righe/batch cumulati per arco intermedio (`max_rows_per_edge`).
    edge_counts: RefCell<HashMap<String, (u64, u64)>>,
    /// Righe in/out cumulate per nodo (`max_expansion_factor`).
    node_rows: RefCell<HashMap<String, (u64, u64)>>,
    /// Handle prepared delle operazioni geo 1:1, costruito una volta per
    /// nodo (hot path minimale): indice di colonna e schema di output non sono piu'
    /// risolti a ogni batch.
    prepared_one_to_one: RefCell<HashMap<String, Rc<OneToOnePrepared>>>,
}

/// Intervallo minimo tra due heartbeat del `TempStore` (errori-e-limiti.md): il punto
/// naturale e' "ogni batch processato", ma la scrittura del lock file ha un
/// costo — un heartbeat al secondo e' di gran lunga piu' frequente del TTL
/// di scavenging (24 ore di default) anche con batch piccolissimi.
const HEARTBEAT_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Per quanto si tollera che l'heartbeat fallisca **di seguito** prima di
/// interrompere l'esecuzione (errori-e-limiti.md).
///
/// Cinque minuti: lo stesso ordine di grandezza della grazia che lo
/// scavenging concede prima di dare retta al PID, e tre ordini di grandezza
/// sotto il TTL di default. Abbastanza da attraversare un guasto transitorio
/// del filesystem, abbastanza poco da accorgersi di uno permanente molto
/// prima che la directory diventi raccoglibile.
const HEARTBEAT_MAX_FAILURE: Duration = Duration::from_secs(300);

/// Aggiunge il contesto strutturale al testo di un errore.
///
/// Estratta da `ExecState::with_diagnostics` per poter essere verificata
/// **insieme** a `with_execution_id`, che e' il punto in cui il difetto
/// viveva: le due funzioni vanno esercitate in sequenza, e un test che
/// costruisse a mano lo stato gia' corretto non proverebbe nulla.
///
/// Il dettaglio e' contesto STRUTTURALE — indice di batch, riga, colonna —
/// mai un valore.
fn arricchisci_con_dettaglio(error: PlenoraError, detail: &str) -> PlenoraError {
    let suffix = format!(" [{detail}]");
    match error {
        PlenoraError::Execution {
            node,
            operation,
            execution_id,
            reason,
        } => PlenoraError::Execution {
            node,
            operation,
            execution_id,
            reason: format!("{reason}{suffix}"),
        },
        PlenoraError::InvalidPlan(reason) => PlenoraError::InvalidPlan(format!("{reason}{suffix}")),
        // Da quando la propagazione conserva la categoria, il contesto del
        // passo viaggia in `Replayed` invece che in `Execution`: se
        // l'arricchimento non seguisse anche quell'involucro, attivare
        // `diagnostics` non aggiungerebbe piu' nulla.
        //
        // Si scrive in ENTRAMBI i campi. Per le categorie `Execution` e
        // `Cancelled`, `with_execution_id` RIGENERA il messaggio da
        // `execution_reason` per inserirvi l'id: un suffisso che vivesse solo
        // nel messaggio verrebbe cancellato dalla chiamata immediatamente
        // successiva, e la diagnostica risulterebbe attiva senza aggiungere
        // nulla. Scrivendo in entrambi, le due operazioni commutano.
        PlenoraError::Replayed(mut replayed) => {
            replayed.message = format!("{}{suffix}", replayed.message);
            if let Some(reason) = replayed.execution_reason.take() {
                replayed.execution_reason = Some(format!("{reason}{suffix}"));
            }
            PlenoraError::Replayed(replayed)
        }
        // Gli involucri trasparenti non sono l'errore: si arricchisce cio'
        // che contengono.
        PlenoraError::Tagged { source, phase } => PlenoraError::Tagged {
            source: Box::new(arricchisci_con_dettaglio(*source, detail)),
            phase,
        },
        PlenoraError::RowDiagnostics {
            source,
            diagnostics,
        } => PlenoraError::RowDiagnostics {
            source: Box::new(arricchisci_con_dettaglio(*source, detail)),
            diagnostics,
        },
        other => other,
    }
}

impl ExecState {
    fn new(
        plan: &Rc<ExecutionPlan>,
        execution_id: String,
        cancellation: CancellationToken,
        diagnostics: bool,
        temp_store: TempStore,
    ) -> Rc<Self> {
        let mut metrics = ExecutionMetrics::default();
        for segment in plan.segments() {
            metrics.segments.insert(
                segment.id.clone(),
                SegmentMetrics {
                    mode: segment.mode,
                    rows_in: 0,
                    rows_out: 0,
                    batches_in: 0,
                    batches_out: 0,
                    wall_time: Duration::ZERO,
                },
            );
            for kernel in &segment.kernels {
                metrics.nodes.insert(
                    kernel.node_id.clone(),
                    NodeMetrics {
                        operation: kernel.operation.to_owned(),
                        ..NodeMetrics::default()
                    },
                );
            }
        }
        let spill_directory = temp_store.path().join("spill");
        Rc::new(Self {
            plan: Rc::clone(plan),
            metrics: RefCell::new(metrics),
            governor: MemoryGovernor::new(plan.limits().max_governed_memory_bytes),
            execution_id,
            cancellation,
            diagnostics,
            temp_store: RefCell::new(temp_store),
            spill_directory,
            spill_metrics: RefCell::new(SpillMetrics::default()),
            last_heartbeat: Cell::new(Instant::now()),
            heartbeat_fallito_da: Cell::new(None),
            input_counts: RefCell::new(HashMap::new()),
            edge_counts: RefCell::new(HashMap::new()),
            node_rows: RefCell::new(HashMap::new()),
            prepared_one_to_one: RefCell::new(HashMap::new()),
        })
    }

    /// Handle prepared dell'operazione geo 1:1 del nodo, costruito alla
    /// prima occorrenza (hot path minimale): indice di colonna e schema di output sono
    /// risolti UNA volta per kernel, non per batch. La chiave e' l'id del
    /// nodo: lo schema di input di un arco e' fisso per contratto.
    fn one_to_one_prepared(
        &self,
        kernel: &PreparedKernel,
        schema: &SchemaRef,
        params: &TransformArrowSchema,
    ) -> Result<Rc<OneToOnePrepared>> {
        if let Some(prepared) = self.prepared_one_to_one.borrow().get(&kernel.node_id) {
            return Ok(prepared.clone());
        }
        let prepared =
            Rc::new(prepare_one_to_one(schema, params).map_err(|error| {
                step_error(kernel, PlenoraError::InvalidPlan(error.to_string()))
            })?);
        self.prepared_one_to_one
            .borrow_mut()
            .insert(kernel.node_id.clone(), prepared.clone());
        Ok(prepared)
    }

    /// Snapshot delle metriche correnti (con l'osservabilita' dei lease
    /// letta dal governor al momento della chiamata, architettura.md#memoria).
    fn metrics(&self) -> ExecutionMetrics {
        let mut metrics = self.metrics.borrow().clone();
        metrics.memory = self.governor.snapshot();
        metrics.spill = *self.spill_metrics.borrow();
        metrics.counters_saturated |= metrics.spill.saturated;
        metrics
    }

    /// Directory di spill condivisa dell'esecuzione (architettura.md#memoria, Fase 2B, spill generalizzato):
    /// sotto-directory `spill/` del `TempStore` — creata dal workspace di
    /// spill al primo uso, ripulita dei file a fine operazione e rimossa
    /// interamente dal `Drop` RAII dello store. Risolta in `new` (hot path minimale).
    fn spill_directory(&self) -> &Path {
        &self.spill_directory
    }

    /// Accumula le metriche di uno spill attivato in un nodo tabellare.
    ///
    /// La saturazione viaggia dentro `SpillMetrics` e riemerge in
    /// `ExecutionMetrics::counters_saturated` quando le metriche vengono
    /// lette: un contatore di spill a fondo scala non deve poter convivere
    /// con un flag che dice «nessuna saturazione».
    fn add_spill_metrics(&self, delta: SpillMetrics) {
        self.spill_metrics.borrow_mut().accumulate(delta);
    }

    /// Accumula le righe prodotte da un nodo senza clonare la chiave dopo il
    /// primo batch. Il conteggio input viene aggiornato da `check_expansion`.
    ///
    /// La somma e' SATURANTE, non avvolgente: questi contatori decidono se un
    /// limite e' superato, e un contatore che avvolge riaprirebbe il limite
    /// (in release) o farebbe abortire l'esecuzione (in debug, con
    /// `overflow-checks`). Saturare tiene il conteggio nel verso giusto — piu'
    /// alto, quindi piu' restrittivo.
    fn add_node_rows_out(&self, node_id: &str, rows_out: u64) {
        let mut rows = self.node_rows.borrow_mut();
        if let Some(entry) = rows.get_mut(node_id) {
            entry.1 = entry.1.saturating_add(rows_out);
        } else {
            let entry = rows.entry(node_id.to_owned()).or_insert((0, 0));
            entry.1 = entry.1.saturating_add(rows_out);
        }
    }

    /// Un batch e' ricaduto dal runner fuso geo al percorso non fuso
    /// (architettura.md#geometrie D12.7): contatore dedicato, mai silenzioso — nessun errore
    /// nuovo, il risultato resta identico.
    fn record_geo_fusion_fallback(&self) {
        let mut metrics = self.metrics.borrow_mut();
        let metrics = &mut *metrics;
        accumulate(
            &mut metrics.geo_fusion_fallbacks,
            1,
            &mut metrics.counters_saturated,
        );
    }

    /// Heartbeat del `TempStore` al punto centrale (ogni batch processato
    /// passa dal conteggio delle metriche), con throttle di
    /// [`HEARTBEAT_MIN_INTERVAL`]. Best-effort: un heartbeat fallito degrada
    /// un segnale diagnostico (errori-e-limiti.md: mai una prova), non deve fermare
    /// l'esecuzione — il cleanup RAII resta la pulizia principale.
    fn heartbeat(&self) {
        if self.last_heartbeat.get().elapsed() < HEARTBEAT_MIN_INTERVAL {
            return;
        }
        // Il throttle avanza SOLO se la scrittura e' riuscita: un heartbeat
        // fallito va ritentato al batch successivo, non fra un intervallo.
        if self.temp_store.borrow_mut().heartbeat().is_ok() {
            self.last_heartbeat.set(Instant::now());
            self.heartbeat_fallito_da.set(None);
        } else if self.heartbeat_fallito_da.get().is_none() {
            self.heartbeat_fallito_da.set(Some(Instant::now()));
        }
    }

    /// Interrompe l'esecuzione se l'heartbeat fallisce da troppo tempo.
    ///
    /// Il singolo fallimento resta tollerato — una `write` puo' fallire per
    /// una ragione transitoria e fermare per questo un'esecuzione lunga
    /// sarebbe sproporzionato. Un fallimento **persistente** e' un'altra
    /// cosa: il timestamp nel lock smette di avanzare, e superato il TTL lo
    /// scavenging di un altro avvio puo' considerare orfana la directory di
    /// questa esecuzione e cancellargliela sotto — con dentro lo spill.
    /// Proseguire in silenzio sarebbe una failure silenziosa, che questo
    /// progetto non ammette: la tolleranza e' limitata, dichiarata e
    /// scaduta la quale l'errore e' esplicito.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Io` se nessun heartbeat riesce da piu' di
    /// [`HEARTBEAT_MAX_FAILURE`].
    fn verifica_heartbeat(&self) -> Result<()> {
        match self.heartbeat_fallito_da.get() {
            Some(dal) if dal.elapsed() > HEARTBEAT_MAX_FAILURE => {
                Err(PlenoraError::Io(std::io::Error::other(format!(
                    "heartbeat del temp store fallito senza interruzione da oltre {} secondi: \
                     la directory temporanea dell'esecuzione puo' essere considerata orfana \
                     e rimossa",
                    HEARTBEAT_MAX_FAILURE.as_secs()
                ))))
            }
            _ => Ok(()),
        }
    }

    /// Errore di cancellazione attribuito a un punto del DAG
    /// (errori-e-limiti.md#cancellazione):
    /// contesto (nodo, operazione, `execution_id`), mai dati.
    fn cancelled(&self, node: &str, operation: &str) -> PlenoraError {
        PlenoraError::Cancelled {
            node: node.to_owned(),
            operation: operation.to_owned(),
            execution_id: self.execution_id.clone(),
            reason: "cancellazione richiesta dal chiamante".to_owned(),
        }
    }

    /// Check di cancellazione al confine di un kernel (errori-e-limiti.md#cancellazione): onora il
    /// `CancellationBehavior` dichiarato in catalogo — `NonInterruptible`
    /// non offre punti di interruzione (mai check).
    fn check_cancellation(&self, kernel: &PreparedKernel) -> Result<()> {
        if cancellation_behavior(kernel) == CancellationBehavior::NonInterruptible {
            return Ok(());
        }
        self.check_cancellation_point(&kernel.node_id, kernel.operation)
    }

    /// Check di cancellazione a un confine di piano (output) o di batch in
    /// ingresso a un segmento: e' lavoro dell'executor, non del kernel —
    /// sempre attivo, anche a valle di op `NonInterruptible` (errori-e-limiti.md: nessuna
    /// nuova attivita' dopo la cancellazione, publish compreso).
    fn check_cancellation_point(&self, node: &str, operation: &str) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled(node, operation));
        }
        Ok(())
    }

    /// Tag di categoria al confine di uscita: ogni errore `Execution` che lascia il DAG
    /// porta l'`execution_id` (riempito qui se il punto di origine, in
    /// profondita' nel dispatch, non lo aveva a disposizione).
    fn tag_execution(&self, error: PlenoraError) -> PlenoraError {
        error.with_execution_id(&self.execution_id)
    }

    /// Arricchimento diagnostico opt-in (errori-e-limiti.md, errori arricchiti): con `diagnostics`
    /// attivo aggiunge alla motivazione contesto strutturale — indice di
    /// batch, riga, colonna dove disponibile, MAI valori. A flag spento (o
    /// dettaglio assente) l'errore passa invariato: messaggi retrocompatibili.
    fn with_diagnostics(&self, error: PlenoraError, detail: Option<&str>) -> PlenoraError {
        if !self.diagnostics {
            return error;
        }
        let Some(detail) = detail else {
            return error;
        };
        arricchisci_con_dettaglio(error, detail)
    }
}

// ---------------------------------------------------------------------------
// Canale d'arco condiviso (fan-out tee, D9: materializzazione minima,
// rilascio al last consumer)
// ---------------------------------------------------------------------------

/// Errore di un arco conservato in forma scomposta per la riproduzione ai
/// consumatori successivi (`PlenoraError` non e' `Clone`): l'attribuzione
/// originale (`Execution`/`Cancelled` con nodo, operazione ed `execution_id`) e'
/// preservata, non declassata a `InvalidPlan`.
struct StoredEdgeError {
    category: ErrorCategory,
    phase: ErrorPhase,
    remote_effect: plenora_core::RemoteEffect,
    retry: RetryDisposition,
    node: Option<String>,
    operation: Option<String>,
    execution_id: Option<String>,
    execution_reason: Option<String>,
    reason: String,
    row_diagnostics: Option<Box<RowDiagnostics>>,
}

impl StoredEdgeError {
    fn from_error(error: &PlenoraError) -> Self {
        let (node, operation, execution_id) = error.execution_location().map_or(
            (None, None, None),
            |(node, operation, execution_id)| {
                (
                    Some(node.to_owned()),
                    Some(operation.to_owned()),
                    execution_id.map(ToOwned::to_owned),
                )
            },
        );
        Self {
            category: error.category(),
            phase: error.phase(),
            remote_effect: error.remote_effect(),
            retry: error.retry_disposition(),
            node,
            operation,
            execution_id,
            execution_reason: error.execution_reason().map(ToOwned::to_owned),
            reason: error.to_string(),
            row_diagnostics: error.row_diagnostics().cloned().map(Box::new),
        }
    }

    fn to_error(&self) -> PlenoraError {
        let replayed = PlenoraError::Replayed(Box::new(ReplayedError {
            category: self.category,
            phase: self.phase,
            remote_effect: self.remote_effect,
            retry: self.retry,
            message: self.reason.clone(),
            node: self.node.clone(),
            operation: self.operation.clone(),
            execution_id: self.execution_id.clone(),
            execution_reason: self.execution_reason.clone(),
        }));
        match &self.row_diagnostics {
            Some(diagnostics) => replayed.with_row_diagnostics((**diagnostics).clone()),
            None => replayed,
        }
    }
}

/// Stato di un arco: upstream lazy, buffer condiviso tra i consumatori e
/// cursore di lettura per ciascuno. Il buffer trattiene [`GovernedBatch`]:
/// il lease e' condiviso (clone `Arc`) tra i consumatori — la quota del
/// batch e' contata UNA volta all'ingresso dell'arco e torna al governor al
/// `Drop` dell'ultimo riferimento (architettura.md#memoria).
struct EdgeShared {
    upstream: RefCell<Option<BatchStream>>,
    buffer: RefCell<Vec<GovernedBatch>>,
    reads: RefCell<Vec<usize>>,
    done: Cell<bool>,
    /// Errore upstream, riprodotto una sola volta a ciascun consumatore.
    error: RefCell<Option<StoredEdgeError>>,
}

impl EdgeShared {
    fn new(upstream: BatchStream) -> Rc<Self> {
        Rc::new(Self {
            upstream: RefCell::new(Some(upstream)),
            buffer: RefCell::new(Vec::new()),
            reads: RefCell::new(Vec::new()),
            done: Cell::new(false),
            error: RefCell::new(None),
        })
    }

    fn register_reader(self: &Rc<Self>) -> EdgeStream {
        let mut reads = self.reads.borrow_mut();
        let id = reads.len();
        reads.push(0);
        EdgeStream {
            shared: Rc::clone(self),
            id,
            error_delivered: false,
        }
    }
}

/// Handle di lettura di un consumatore su un arco condiviso.
struct EdgeStream {
    shared: Rc<EdgeShared>,
    id: usize,
    /// L'errore dell'arco e' consegnato UNA volta per consumatore, poi lo
    /// stream termina (`None`): mai un iteratore infinito di errori.
    error_delivered: bool,
}

impl EdgeStream {
    /// Rilascia i batch letti da tutti i consumatori (rilascio al last consumer).
    ///
    /// Nel caso a consumatore singolo i batch non sono bufferizzati affatto:
    /// il cursore e' clam-pato alla lunghezza del buffer condiviso.
    fn release_consumed(&self) {
        let mut reads = self.shared.reads.borrow_mut();
        let mut buffer = self.shared.buffer.borrow_mut();
        let Some(min_read) = reads.iter().copied().min() else {
            return;
        };
        let min_read = min_read.min(buffer.len());
        if min_read == 0 {
            return;
        }
        buffer.drain(..min_read);
        for cursor in reads.iter_mut() {
            *cursor -= min_read;
        }
    }
}

impl Iterator for EdgeStream {
    type Item = Result<GovernedBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        // 1. Batch gia' bufferizzato per questo consumatore.
        {
            let buffer = self.shared.buffer.borrow();
            let position = self.shared.reads.borrow()[self.id];
            if position < buffer.len() {
                let batch = buffer[position].clone();
                drop(buffer);
                self.shared.reads.borrow_mut()[self.id] += 1;
                self.release_consumed();
                return Some(Ok(batch));
            }
        }
        // 2. Upstream esaurito (o in errore): l'errore e' consegnato una
        // sola volta per consumatore, poi lo stream e' chiuso.
        if self.shared.done.get() {
            if self.error_delivered {
                return None;
            }
            return self.shared.error.borrow().as_ref().map(|stored| {
                self.error_delivered = true;
                Err(stored.to_error())
            });
        }
        // 3. Pull dall'upstream.
        let item = self.shared.upstream.borrow_mut().as_mut()?.next();
        match item {
            Some(Ok(batch)) => {
                let single_consumer = self.shared.reads.borrow().len() == 1;
                if !single_consumer {
                    self.shared.buffer.borrow_mut().push(batch.clone());
                }
                self.shared.reads.borrow_mut()[self.id] += 1;
                self.release_consumed();
                Some(Ok(batch))
            }
            Some(Err(error)) => {
                self.shared.done.set(true);
                *self.shared.error.borrow_mut() = Some(StoredEdgeError::from_error(&error));
                self.error_delivered = true;
                Some(Err(error))
            }
            None => {
                self.shared.done.set(true);
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Schema IPC dell'output: lo schema del contratto arricchito del blocco
/// canonico R2.2 per ogni colonna geometrica e della versione di protocollo
/// R2.5 nei metadati dello schema (milestone C — post-processo CENTRALE: i
/// campi continuano a essere costruiti dagli `analyze_contract` con le sole
/// chiavi `GeoArrow` legacy, che RESTANO — R2.6 ammette la coesistenza se
/// coerente; il cablaggio dei singoli analyze e' milestone successiva).
///
/// Regole:
///
/// - per ogni `GeometryColumnContract` del contratto, le chiavi di
///   [`canonical_geometry_metadata`] sono fuse nel metadata del campo
///   omonimo. `GeometryMetadataDetails::default()` (nessun dettaglio
///   opzionale modellato dal contratto) attiva la cascata di completamento
///   DELL'ASSENTE (R2.7, piano-v5.md#contratti-di-input emendamento 2026-07-31): normalmente
///   `axis_order` e `srid` sono DEDOTTI dalla definizione canonica d'autorita'
///   ([`ResolvedCrs::authority_axis_order`]/[`ResolvedCrs::authority_srid`]
///   — lo stesso oggetto con cui il kernel ha operato; deduzione da
///   autorita', non invenzione) e `axis_order` vale `unknown` solo quando
///   neanche la definizione determina gli assi — `unknown` resta l'onesta',
///   non il default pigro (R5.2 riguarda le chiavi opzionali, che restano
///   assenti). `geo.reproject` fa eccezione esplicita per `axis_order`: lo
///   inserisce gia' nell'output dell'analisi con l'ordine GIS normalizzato
///   realmente prodotto dal backend (`lon_lat`/`easting_northing`), distinto
///   dall'ordine nativo dell'autorita'; lo `srid` resta d'autorita';
/// - R2.6: una chiave canonica gia' presente sul campo (o la versione sullo
///   schema) con valore DIVERSO da quello imposto dal contratto e' un
///   errore, mai una sovrascrittura silenziosa; valore uguale e'
///   idempotente. Le chiavi che l'operazione RISCRIVE di mestiere (piano-v5.md#contratti-di-input,
///   decisione 8 — il blocco CRS per `reproject`, `types`/
///   `types_declaration` per le trasformazioni che cambiano il tipo
///   geometrico) non passano MAI di qui come divergenze: la sostituzione
///   avviene a monte, nel contratto prodotto dall'analisi
///   (`analyze_reproject` / `with_geometry_types` rimuovono le chiavi
///   ereditate), e qui sono ri-emesse dal contratto come ogni altra. Per
///   tutte le chiavi non riscritte il guard resta intatto. Eccezioni
///   dichiarate: `axis_order` e `srid` sono
///   per completamento dell'assente (R2.7, mai arbitrato) — una chiave
///   di lineage PRESENTE vince sempre, qualunque sia il valore emesso dal
///   contratto (anche un valore dedotto dall'autorita': la deduzione non
///   deve mai trasformarsi in conflitto R2.6 su un passthrough; prima
///   dell'emendamento 2026-07-31 lo skip copriva solo `axis_order =
///   unknown`, l'unico valore emesso possibile allora);
///   `crs_resolution = resolved` preesistente e' corretta in
///   `declared_unresolved` quando il contratto porta un'incoerenza rilevata
///   (R4.6.4: mai silenziarla propagando la dichiarazione `resolved` che
///   l'incoerenza smentisce — unica sovrascrittura ammessa, in una sola
///   direzione);
/// - R2.5: `plenora.contract.version` e' aggiunta ai metadati dello schema
///   SOLO se almeno un campo porta chiavi canoniche (sempre vero quando il
///   contratto dichiara geometrie: [`canonical_geometry_metadata`] emette
///   comunque `dimensions`/`crs_resolution`/`field_id`); uno schema senza
///   geometrie e' restituito invariato;
/// - una colonna geometrica del contratto assente dallo schema e' un errore
///   (invariante violata a monte: fail-closed, mai silenziosa).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per chiave canonica preesistente divergente
/// (R2.6) o colonna geometrica del contratto assente nello schema.
fn canonical_output_schema(contract: &DataContract) -> Result<SchemaRef> {
    if contract.geometries.is_empty() {
        return Ok(contract.schema.clone());
    }
    let mut matched = 0_usize;
    let mut fields = Vec::with_capacity(contract.schema.fields().len());
    for field in contract.schema.fields() {
        let Some(geometry) = contract
            .geometries
            .iter()
            .find(|geometry| geometry.name.as_str() == field.name().as_str())
        else {
            fields.push(field.as_ref().clone());
            continue;
        };
        matched += 1;
        let canonical = canonical_geometry_metadata(geometry, &GeometryMetadataDetails::default());
        let mut metadata = field.metadata().clone();
        // R4.6.3: con un CRS deciso dal piano (`ResolvedByDecision`) le
        // dichiarazioni della sorgente sono SOSTITUITE, non fuse — il
        // blocco canonico ri-emette il CRS deciso e la lineage non deve
        // riproporre il conflitto a valle. Lo schema del contratto di
        // input resta intatto (il check fail-closed input/contratto
        // confronta i campi, metadati inclusi): la sostituzione vive solo
        // qui, all'emissione.
        if matches!(geometry.crs, ContractCrs::ResolvedByDecision(_)) {
            strip_decided_crs_declarations(&mut metadata);
        }
        for (key, value) in &canonical {
            match metadata.get(key) {
                Some(existing) if existing != value => {
                    // `axis_order` e `srid` sono per completamento DELL'ASSENTE
                    // (R2.7), mai per arbitrato: una chiave di lineage
                    // PRESENTE vince sempre, qualunque sia il valore emesso —
                    // anche un valore dedotto dalla definizione d'autorita'
                    // (piano-v5.md#contratti-di-input, emendamento 2026-07-31): la deduzione riempie
                    // solo le chiavi assenti e non deve mai trasformarsi in
                    // un falso conflitto R2.6 su un passthrough (R2.4: la
                    // dichiarazione del produttore resta). Prima
                    // dell'emendamento lo skip copriva solo
                    // `axis_order = unknown`, allora unico valore possibile.
                    if key == PLENORA_GEOMETRY_AXIS_ORDER_KEY || key == PLENORA_GEOMETRY_SRID_KEY {
                        continue;
                    }
                    // R4.6.4: un centro che ha rilevato un'incoerenza CRS la
                    // DICHIARA (`declared_unresolved`) invece di propagare la
                    // dichiarazione `resolved` del produttore, che
                    // l'incoerenza stessa smentisce — silenziarla e'
                    // vietato. E' l'unica sovrascrittura ammessa su una
                    // chiave canonica: una sola chiave, una sola direzione
                    // (`resolved` -> `declared_unresolved`), mai il
                    // contrario (piano-v5.md#contratti-di-input, decisione 7). La direzione
                    // opposta (`declared_unresolved` -> `resolved`) non
                    // passa di qui: con una decisione del piano le
                    // dichiarazioni della sorgente sono gia' state rimosse
                    // sopra (`strip_decided_crs_declarations`).
                    if key == PLENORA_GEOMETRY_CRS_RESOLUTION_KEY
                        && existing == "resolved"
                        && value == "declared_unresolved"
                    {
                        metadata.insert(key.clone(), value.clone());
                        continue;
                    }
                    return Err(PlenoraError::InvalidPlan(format!(
                        "campo geometria `{}`: chiave `{key}` gia' presente con un valore \
                         diverso da quello del contratto (R2.6: il componente fallisce, \
                         non sovrascrive)",
                        geometry.name
                    )));
                }
                Some(_) => {}
                None => {
                    metadata.insert(key.clone(), value.clone());
                }
            }
        }
        fields.push(field.as_ref().clone().with_metadata(metadata));
    }
    if matched != contract.geometries.len() {
        return Err(PlenoraError::InvalidPlan(
            "colonna geometrica del contratto assente nello schema di output".to_owned(),
        ));
    }
    // R2.5: la versione accompagna le chiavi canoniche; qui almeno un campo
    // le porta (guardia in testa e conteggio sopra).
    let mut metadata = contract.schema.metadata().clone();
    for (key, value) in canonical_schema_version_metadata() {
        match metadata.get(&key) {
            Some(existing) if existing != &value => {
                return Err(PlenoraError::InvalidPlan(format!(
                    "chiave `{key}` dello schema gia' presente con un valore diverso \
                     (R2.6: il componente fallisce, non sovrascrive)"
                )));
            }
            Some(_) => {}
            None => {
                metadata.insert(key, value);
            }
        }
    }
    Ok(Arc::new(Schema::new_with_metadata(fields, metadata)))
}

/// Output di un'esecuzione: stream lazy dei batch finali + metriche.
///
/// Iterare l'`Output` guida l'esecuzione: l'input e' consumato
/// batch-per-batch (streaming reale). Non e' `Send` nella v1 seriale (parallelismo solo dove conviene).
pub struct Output {
    contract: DataContract,
    /// Schema IPC emesso: quello del contratto piu' il blocco canonico
    /// R2.2/R2.5 (milestone C), calcolato una sola volta alla costruzione
    /// ([`canonical_output_schema`], fail-fast su divergenze R2.6).
    schema: SchemaRef,
    stream: BatchStream,
    state: Rc<ExecState>,
    /// Stato terminale del consumo per iteratore: dopo che lo stream si e'
    /// esaurito, il controllo di salute finale corre **una sola volta** e il
    /// suo esito non si ripete. Senza, un iteratore riavviato produrrebbe lo
    /// stesso errore all'infinito, e chi lo consuma in un `for` lo vedrebbe
    /// come un ciclo che non finisce.
    esaurito: bool,
}

impl Output {
    /// Schema Arrow dell'output: quello del contratto inferito in
    /// validazione arricchito del blocco canonico R2.2 e della versione
    /// R2.5 (milestone C) — lo stesso schema scritto nell'header IPC da
    /// [`Output::write_ipc_file`].
    #[must_use]
    pub fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    /// Contratto dell'arco di output del piano.
    ///
    /// Il suo `schema` e' quello inferito in validazione, SENZA il blocco
    /// canonico R2.2/R2.5: lo schema effettivamente emesso in IPC e'
    /// [`Output::schema`].
    #[must_use]
    pub const fn output_contract(&self) -> &DataContract {
        &self.contract
    }

    /// Identita' dell'esecuzione (errori-e-limiti.md, errori arricchiti): la stessa riportata negli
    /// errori `Execution`/`Cancelled` e nel lock del `TempStore`.
    #[must_use]
    pub fn execution_id(&self) -> &str {
        &self.state.execution_id
    }

    /// Snapshot delle metriche correnti (parziali finche' lo stream non e'
    /// esaurito).
    #[must_use]
    pub fn metrics(&self) -> ExecutionMetrics {
        self.state.metrics()
    }

    /// Drena lo stream raccogliendo tutti i batch finali.
    ///
    /// Il wrapper governato si spacca al confine pubblico: il lease di ogni
    /// batch e' rilasciato alla consegna (la memoria passa al chiamante).
    ///
    /// # Errors
    ///
    /// Propaga il primo errore dello stream (nessun output parziale viene
    /// restituito).
    pub fn collect_batches(self) -> Result<(Vec<RecordBatch>, ExecutionMetrics)> {
        let batches = self
            .stream
            .map(|item| item.map(GovernedBatch::into_batch))
            .collect::<Result<Vec<_>>>()?;
        // Controllo di salute PRIMA di dichiarare conclusa l'esecuzione: una
        // corruzione della contabilita' rilevata dentro un `Drop` non puo'
        // propagare un errore da li', e senza questo l'ultimo output verrebbe
        // consegnato da un governor che ha gia' perso il conto.
        self.state.governor.verifica_salute("output")?;
        // Stesso criterio per il temp store: consegnare l'output di
        // un'esecuzione il cui lock e' fermo da oltre la tolleranza
        // significherebbe dichiararla riuscita mentre la sua directory e'
        // gia' raccoglibile da un altro avvio.
        self.state.verifica_heartbeat()?;
        Ok((batches, self.state.metrics()))
    }

    /// Drena lo stream conservando i wrapper governati (lease + sequenza).
    ///
    /// Seam interno per i test del governor (architettura.md#determinismo e #memoria): in questa
    /// milestone nessun consumatore pubblico riordina per `BatchSequence`.
    #[cfg(test)]
    pub(crate) fn collect_governed(self) -> Result<(Vec<GovernedBatch>, ExecutionMetrics)> {
        let batches = self.stream.collect::<Result<Vec<_>>>()?;
        Ok((batches, self.state.metrics()))
    }

    /// Scrive l'output in Arrow IPC file format con publish atomico
    /// (decisione D22/errori-e-limiti.md#publish-e-cleanup): tempfile nella directory di destinazione,
    /// persist no-clobber solo a stream completato con successo — nessun
    /// output parziale e' mai visibile. Profilo [`PublishProfile::Atomic`]:
    /// wrapper su [`Output::write_ipc_file_with_profile`], l'esito tipizzato
    /// (sempre `Published` a publish riuscito) e' scartato.
    ///
    /// L'header IPC porta lo schema di [`Output::schema`]: quello del
    /// contratto piu' il blocco canonico R2.2 per ogni colonna geometrica e
    /// la versione R2.5 nei metadati dello schema (milestone C); le chiavi
    /// `GeoArrow` legacy restano (coesistenza coerente, R2.6).
    ///
    /// # Errors
    ///
    /// Propaga errori di stream e di I/O; `PlenoraError::InvalidPlan` se la
    /// destinazione esiste gia' o la directory non esiste;
    /// `PlenoraError::Unsupported` se il filesystem di
    /// destinazione e' di rete o non identificabile (errori-e-limiti.md#publish-e-cleanup).
    pub fn write_ipc_file(self, path: &Path) -> Result<ExecutionMetrics> {
        let (metrics, _outcome) = self.write_ipc_file_with_profile(path, PublishProfile::Atomic)?;
        Ok(metrics)
    }

    /// Come [`Output::write_ipc_file`], ma con profilo di publish
    /// selezionabile (errori-e-limiti.md#publish-e-cleanup) ed esito tipizzato restituito al chiamante:
    /// [`PublishOutcome::PublishedButDurabilityUnconfirmed`] se il publish e'
    /// riuscito ma la durabilita' non e' confermata (es. `fsync` di directory
    /// non supportato dalla piattaforma).
    ///
    /// # Errors
    ///
    /// Come [`Output::write_ipc_file`].
    pub fn write_ipc_file_with_profile(
        self,
        path: &Path,
        profile: PublishProfile,
    ) -> Result<(ExecutionMetrics, PublishOutcome)> {
        let schema = self.schema.clone();
        let governor = self.state.governor.clone();
        let stato_publish = Rc::clone(&self.state);
        let mut stream = self.stream;
        let ((), outcome) = publish_with_profile(path, profile, move |writer| {
            let mut ipc = FileWriter::try_new(writer, &schema)?;
            // Cache della decisione di rivestimento per Arc di schema: i
            // batch di uno stream condividono lo stesso Arc (lo schema del
            // contratto del kernel), quindi il confronto profondo di
            // `Schema` (campi + mappe metadata) si esegue solo al primo
            // batch di ogni schema distinto (hot path minimale: lavoro hoistable fuori
            // dal loop). Il rivestimento resta fail-closed: `try_new`
            // rivalida ogni batch rivestito.
            let mut schema_decision: Option<(SchemaRef, bool)> = None;
            for item in &mut stream {
                let batch = item?.into_batch();
                // Lo schema emesso (blocco canonico R2.2/R2.5 fuso dal
                // contratto) puo' differire da quello del batch solo nei
                // metadati: rivestimento a costo zero sui buffer (colonne
                // condivise via Arc), fail-closed su qualunque altra
                // divergenza (tipo, numero di colonne).
                let batch_schema = batch.schema();
                let rewrap = match &schema_decision {
                    Some((seen, decision)) if Arc::ptr_eq(seen, &batch_schema) => *decision,
                    _ => {
                        let decision = batch_schema != schema;
                        schema_decision = Some((batch_schema, decision));
                        decision
                    }
                };
                let batch = if rewrap {
                    // Rivestimento dello schema prima della pubblicazione: le
                    // colonne sono quelle dell'input, quindi possono essere
                    // zero e la cardinalita' va dichiarata.
                    let righe = batch.num_rows();
                    plenora_core::batch_with_rows(schema.clone(), batch.columns().to_vec(), righe)?
                } else {
                    batch
                };
                ipc.write(&batch)?;
            }
            ipc.finish()?;
            // Controllo di salute PRIMA del publish atomico: una corruzione
            // della contabilita' rilevata dentro un `Drop` non puo' propagare
            // un errore da li', e il publish e' irreversibile. Qui il file
            // temporaneo non e' ancora stato reso visibile (errori-e-limiti.md#publish-e-cleanup), quindi
            // fallire ora significa non pubblicare nulla.
            governor.verifica_salute("output")?;
            // Stesso cancello per il temp store: il publish e' irreversibile,
            // e pubblicare mentre il lock e' fermo da oltre la tolleranza
            // significherebbe dichiarare riuscita un'esecuzione la cui
            // directory e' gia' raccoglibile.
            stato_publish.verifica_heartbeat()?;
            Ok(())
        })?;
        Ok((self.state.metrics(), outcome))
    }
}

impl Iterator for Output {
    type Item = Result<RecordBatch>;

    /// Consumo batch per batch dell'output.
    ///
    /// A stream esaurito corre il **controllo di salute terminale**: se la
    /// contabilita' del governor e' stata marcata incoerente — cosa che puo'
    /// accadere dentro il `Drop` dell'ultimo lease, dove un errore non puo'
    /// essere propagato — l'iteratore produce **una volta** `Some(Err(...))` e
    /// poi `None`.
    ///
    /// Senza questo controllo chi consuma con `for batch in output` non
    /// passerebbe ne' da [`Output::collect_batches`] ne' dal publish atomico,
    /// e una corruzione rilevata all'ultimo rilascio diventerebbe un successo
    /// silenzioso: lo stream finirebbe e basta.
    ///
    /// L'errore e' emesso una sola volta (`esaurito`): ripeterlo a ogni
    /// chiamata trasformerebbe un `for` in un ciclo che non termina.
    fn next(&mut self) -> Option<Self::Item> {
        if self.esaurito {
            return None;
        }
        if let Some(item) = self.stream.next() {
            return Some(item.map(GovernedBatch::into_batch));
        }
        self.esaurito = true;
        // Terminale dell'iteratore: come il cancello di consegna e quello di
        // publish, riporta sia una contabilita' corrotta sia un heartbeat
        // fermo da troppo tempo. Chiudere lo stream in silenzio su un lock
        // stantio sarebbe dichiarare riuscita un'esecuzione la cui directory
        // e' gia' raccoglibile.
        self.state
            .governor
            .verifica_salute("output")
            .err()
            .or_else(|| self.state.verifica_heartbeat().err())
            .map(Err)
    }
}

// ---------------------------------------------------------------------------
// execute
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

/// Writer con conteggio dei byte e quota dichiarata (`max_temp_bytes` del
/// piano): superata la quota la scrittura fallisce con errore esplicito,
/// mai silenzioso.
struct CountingFile {
    file: std::fs::File,
    written: u64,
    max_bytes: u64,
}

impl CountingFile {
    fn create(path: &Path, max_bytes: u64) -> Result<Self> {
        let file = std::fs::File::create(path).map_err(PlenoraError::Io)?;
        Ok(Self {
            file,
            written: 0,
            max_bytes,
        })
    }
}

impl std::io::Write for CountingFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.written.checked_add(buf.len() as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "overflow conteggio staging IPC",
            )
        })?;
        if written > self.max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "staging IPC oltre max_temp_bytes",
            ));
        }
        let n = self.file.write(buf)?;
        self.written = self.written.checked_add(n as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "overflow conteggio staging IPC",
            )
        })?;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Metadati per-batch dello staging IPC: byte da ri-riservare al replay
/// (gli stessi della riserva originale, rilasciata allo staging) e
/// sequenza logica architettura.md#determinismo catturata allo staging e ripubblicata
/// invariata al replay.
struct StagedBatchMeta {
    bytes: u64,
    sequence: Option<BatchSequence>,
}

/// Stato del replay: lettore IPC sul file di staging e metadati per-batch
/// (byte da ri-riservare + sequenza logica da ripubblicare).
struct StagedReplay {
    reader: StreamReader<std::fs::File>,
    staged: std::collections::VecDeque<StagedBatchMeta>,
    // La directory temporanea vive fino alla fine del replay.
    _dir: tempfile::TempDir,
}

/// Replay di UN batch dallo staging IPC: compattazione right-sized, lease
/// ri-riservato per batch (memoria bounded) e sequenza logica ripubblicata
/// invariata. Condiviso dal gate input WKB e dallo staging degli output
/// accettati dei segmenti row-diagnostics: nessuna logica duplicata.
fn replay_staged_batch(
    state: &ExecState,
    replay: &mut StagedReplay,
    owner: &str,
) -> Option<Result<GovernedBatch>> {
    match replay.reader.next() {
        Some(Ok(batch)) => {
            let Some(meta) = replay.staged.pop_front() else {
                return Some(Err(PlenoraError::Internal(
                    "replay staging IPC: conteggio byte incoerente".into(),
                )));
            };
            // Compattazione: la decodifica IPC condivide un'unica
            // allocazione corpo tra le colonne e ogni buffer la conta
            // interamente (lease e confini di kernel gonfiati ~3x).
            // `take` copia ogni colonna in buffer right-sized: una
            // copia per batch, memoria bounded.
            let batch = match compact_staged_batch(&batch) {
                Ok(compacted) => compacted,
                Err(error) => return Some(Err(error)),
            };
            match state.governor.reserve(meta.bytes, owner) {
                Ok(lease) => Some(Ok(GovernedBatch::new(batch, Some(lease), meta.sequence))),
                Err(error) => Some(Err(error)),
            }
        }
        Some(Err(error)) => Some(Err(PlenoraError::Internal(format!(
            "replay staging IPC: {error}"
        )))),
        None => None,
    }
}

/// Copia un batch decodificato dallo staging in buffer right-sized (vedi
/// replay): `take` con tutti gli indici, per colonna.
///
/// # Errors
/// - `ResourceLimit`: righe oltre `u32::MAX` (gia' escluso dai limiti di
///   piano, difesa);
/// - `Schema`: errore Arrow nella `take` o nella ricostruzione.
fn compact_staged_batch(batch: &RecordBatch) -> Result<RecordBatch> {
    let indices: UInt32Array = (0..u32::try_from(batch.num_rows())
        .map_err(|_| PlenoraError::ResourceLimit("batch staging oltre u32 righe".into()))?)
        .collect::<Vec<_>>()
        .into();
    let columns = batch
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &indices, None).map_err(PlenoraError::from))
        .collect::<Result<Vec<_>>>()?;
    plenora_core::batch_with_rows(batch.schema(), columns, batch.num_rows())
}

/// Validazione atomica dell'input geometrico (D8/B1.3) con memoria BOUNDED:
/// i batch accettati sono staged su IPC entro la quota `max_temp_bytes`
/// dichiarata dal piano e il lease governor e' rilasciato subito; solo a
/// validazione completata senza rifiuti i batch sono riletti uno alla
/// volta, con lease ri-riservato per batch e stessa sequenza logica.
/// Invarianti R9.9 preservate: nessun accepted esce prima della validazione
/// completa; un rifiuto row-scoped (anche tardivo) produce il report
/// completo mergiato e zero accepted; un errore non row-scoped propaga
/// fail-closed con la diagnostica parziale dichiarata. Un errore di I/O in
/// replay e' una failure infrastrutturale (accepted parziali possibili,
/// come ogni failure mid-stream): non e' un rifiuto di righe.
///
/// Nota quota: lo staging degli input, lo staging degli output accettati
/// dei segmenti row-diagnostics e gli spill degli operatori misurano
/// ciascuno la propria scrittura contro `max_temp_bytes`; la somma su
/// disco puo' superare la quota (v1, contabilita' separate).
/// Esito della fase di staging dell'input gate: errore terminale (eventuale
/// assenza di batch staged -> stream vuoto) oppure replay dal file staged.
enum StagingOutcome {
    Terminal(Option<PlenoraError>),
    Replay(StagedReplay),
    /// Coda ordinata degli accepted trattenuti in memoria, con i lease
    /// originali ancora vivi: consegnata direttamente, senza IPC ne' copie
    /// (architettura.md#memoria, staging memory-first). Prodotta SOLO dai segmenti row-diagnostics; il gate
    /// WKB dell'input resta su disco.
    Memoria(std::collections::VecDeque<GovernedBatch>),
}

/// Staging degli accepted di un segmento row-diagnostics: **prima in
/// memoria**, con passaggio definitivo su disco quando il budget non basta
/// piu' (architettura.md#memoria, staging memory-first).
///
/// # Perche' esiste
///
/// La barriera R9.9 — nessun accepted pubblicato prima che la scansione sia
/// completa — non richiede il disco: richiede solo che nulla esca prima della
/// fine. Trattenere i batch gia' governati la soddisfa allo stesso modo, e
/// risparmia per ogni riga una serializzazione IPC, una scrittura, una
/// rilettura, una decodifica e una copia `take`.
///
/// # Perche' non puo' trasformare un input eseguibile in un `ResourceLimit`
///
/// Durante una passata della catena i lease vivi sono al piu' due: quello
/// del batch d'ingresso e quello dell'uscita (`run_streaming_chain` acquisisce
/// il secondo prima di rilasciare il primo). Quindi:
///
/// - **su disco** il picco della passata `k` e' `input_k + output_k`;
/// - **in memoria** e' `trattenuti + input_k + output_k`.
///
/// Si entra nella passata `k` in modalita' memoria **solo se**
/// `trattenuti + input_k + max_batch_bytes <= budget`, dove `input_k` e' la
/// dimensione REALE del batch gia' prelevato e `max_batch_bytes` e' il tetto
/// duro del piano (tetto in byte per batch). Ogni batch di output attraversa il wrapper d'uscita,
/// che applica lo stesso tetto: `output_k > max_batch_bytes` fa fallire il
/// piano **in entrambe le modalita'**. Per un piano che prima riusciva vale
/// quindi `output_k <= max_batch_bytes`, e il picco in memoria non supera il
/// budget.
///
/// La soglia e' **derivata dai limiti del piano e dai lease effettivamente
/// vivi**: nessuna percentuale scelta a mano, nessuna decisione temporale,
/// nessuna dipendenza dall'ordine di arrivo.
// La variante `Disco` porta writer e handle del file: piu' grande di una
// `VecDeque`, ma esiste al massimo una volta per segmento e boxarla
// aggiungerebbe un'indirezione sul percorso caldo dello staging.
#[allow(clippy::large_enum_variant)]
enum StagingAccepted {
    /// Batch trattenuti in ordine, lease vivi.
    ///
    /// Nessun totale locale dei byte: i lease sono gia' contati dal governor,
    /// che e' la fonte unica della soglia (vedi `accedibile_in_memoria`).
    /// Tenerne una copia qui sarebbe un duplicato — e un duplicato PARZIALE,
    /// perche' non vedrebbe le prenotazioni degli altri rami.
    Memoria(std::collections::VecDeque<GovernedBatch>),
    /// Modalita' disco: definitiva, non si torna indietro.
    Disco {
        writer: Option<StreamWriter<CountingFile>>,
        staging: Option<(tempfile::TempDir, std::path::PathBuf)>,
        meta: std::collections::VecDeque<StagedBatchMeta>,
    },
}

impl StagingAccepted {
    const fn nuovo() -> Self {
        Self::Memoria(std::collections::VecDeque::new())
    }

    /// Modalita' disco definitiva, partendo da una coda gia' trattenuta.
    ///
    /// I batch sono travasati **nell'ordine** in cui sono stati prodotti e i
    /// lease rilasciati uno a uno: il picco durante il travaso non cresce
    /// mai sopra quello gia' concesso.
    fn passa_a_disco(&mut self, state: &Rc<ExecState>, edge: &str) -> Result<()> {
        let Self::Memoria(coda) = self else {
            return Ok(());
        };
        let coda = std::mem::take(coda);
        let mut writer = None;
        let mut staging = None;
        let mut meta = std::collections::VecDeque::new();
        for governed in coda {
            stage_one_batch(
                &mut writer,
                &mut staging,
                state,
                "output",
                edge,
                &governed.batch,
            )?;
            meta.push_back(StagedBatchMeta {
                bytes: governed.accounted_bytes(),
                sequence: governed.seq.clone(),
            });
            // Rilascio esplicito: il lease muore qui, non a fine ciclo.
            drop(governed);
        }
        *self = Self::Disco {
            writer,
            staging,
            meta,
        };
        Ok(())
    }

    /// Accoglie un accepted, gia' governato.
    fn accogli(
        &mut self,
        state: &Rc<ExecState>,
        edge: &str,
        governed: GovernedBatch,
    ) -> Result<()> {
        match self {
            Self::Memoria(coda) => {
                coda.push_back(governed);
                Ok(())
            }
            Self::Disco {
                writer,
                staging,
                meta,
            } => {
                stage_one_batch(writer, staging, state, "output", edge, &governed.batch)?;
                meta.push_back(StagedBatchMeta {
                    bytes: governed.accounted_bytes(),
                    sequence: governed.seq.clone(),
                });
                Ok(())
            }
        }
    }
}

/// Scrive un batch nello staging IPC (inizializzando file e writer al primo
/// batch); la quota `max_temp_bytes` e' fatta rispettare da `CountingFile`.
/// `what` qualifica il contesto nei messaggi (`input` gate WKB, `output`
/// segmenti row-diagnostics): stessa logica, nessuna duplicazione.
fn stage_one_batch(
    writer: &mut Option<StreamWriter<CountingFile>>,
    staging: &mut Option<(tempfile::TempDir, std::path::PathBuf)>,
    state: &Rc<ExecState>,
    what: &str,
    edge: &str,
    batch: &RecordBatch,
) -> Result<()> {
    if writer.is_none() {
        let dir = tempfile::Builder::new()
            .prefix(&format!("plenora-staging-{what}-"))
            .tempdir()
            .map_err(PlenoraError::Io)?;
        let path = dir.path().join("staged.arrow");
        let counting = CountingFile::create(&path, state.plan.limits().max_temp_bytes)?;
        let stream = StreamWriter::try_new(counting, &batch.schema())
            .map_err(|error| PlenoraError::Internal(format!("staging {what}: {error}")))?;
        *writer = Some(stream);
        *staging = Some((dir, path));
    }
    let active = writer
        .as_mut()
        .ok_or_else(|| PlenoraError::Internal(format!("staging {what} non inizializzato")))?;
    active.write(batch).map_err(|error| {
        PlenoraError::InvalidPlan(format!(
            "staging {what} `{edge}` fallito oltre la quota o per I/O: {error}"
        ))
    })?;
    Ok(())
}

/// Drena lo stream di input validando ogni batch (gate WKB con diagnostica
/// completa) e facendo staging IPC bounded su `max_temp_bytes`; il lease del
/// governor e' rilasciato dopo lo staging di ciascun batch.
// Macchina a stati lineare: staging, diagnostica, chiusura e apertura replay
// restano nello stesso scope per rendere evidente il cleanup fail-closed.
#[allow(clippy::too_many_lines)]
fn stage_input_batches(
    input: &mut BatchStream,
    state: &Rc<ExecState>,
    edge: &str,
) -> StagingOutcome {
    let mut diagnostics = None;
    let mut terminal_error = None;
    let mut staged_meta: std::collections::VecDeque<StagedBatchMeta> =
        std::collections::VecDeque::new();
    let mut next_sequence: u64 = 0;
    let mut staging: Option<(tempfile::TempDir, std::path::PathBuf)> = None;
    let mut writer: Option<StreamWriter<CountingFile>> = None;
    for item in input {
        match item {
            Ok(batch) => {
                if diagnostics.is_none() && terminal_error.is_none() {
                    let staged = stage_one_batch(
                        &mut writer,
                        &mut staging,
                        state,
                        "input",
                        edge,
                        &batch.batch,
                    );
                    if let Err(error) = staged {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            error,
                            &mut diagnostics,
                            "data_tools.input_staging_failed",
                        ));
                        break;
                    }
                    let sequence_number = next_sequence;
                    let Some(next) = next_sequence.checked_add(1) else {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            PlenoraError::Internal("overflow sequenza staging input".into()),
                            &mut diagnostics,
                            "data_tools.input_staging_failed",
                        ));
                        break;
                    };
                    next_sequence = next;
                    staged_meta.push_back(StagedBatchMeta {
                        bytes: batch.accounted_bytes(),
                        sequence: Some(BatchSequence {
                            source_node: edge.to_owned(),
                            input_partition: 0,
                            sequence_number,
                        }),
                    });
                    // Il lease del batch e' rilasciato con il drop:
                    // durante il drenaggio resta riservato al piu'
                    // un batch alla volta.
                }
            }
            Err(error) => {
                if let Some(report) = error.row_diagnostics().cloned() {
                    if let Err(error) = merge_row_diagnostics(&mut diagnostics, report, 0) {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            error,
                            &mut diagnostics,
                            "data_tools.diagnostic_merge_failed",
                        ));
                        break;
                    }
                } else {
                    terminal_error = Some(attach_partial_row_diagnostics(
                        error,
                        &mut diagnostics,
                        "data_tools.input_stream_interrupted",
                    ));
                    break;
                }
            }
        }
    }
    if terminal_error.is_none() {
        if let Some(active) = writer.as_mut() {
            if let Err(error) = active.finish() {
                terminal_error = Some(attach_partial_row_diagnostics(
                    PlenoraError::Internal(format!("chiusura staging input: {error}")),
                    &mut diagnostics,
                    "data_tools.input_staging_failed",
                ));
            }
        }
    }
    drop(writer);
    if terminal_error.is_none() {
        if let Err(error) = state.check_cancellation_point(edge, "input_validation") {
            terminal_error = Some(attach_partial_row_diagnostics(
                error,
                &mut diagnostics,
                "data_tools.cancelled_after_rejection",
            ));
        } else if let Some(report) = diagnostics.take() {
            terminal_error = Some(complete_row_diagnostic_error(report, None));
        }
    }
    if let Some(error) = terminal_error {
        return StagingOutcome::Terminal(Some(error));
    }
    let Some((dir, path)) = staging.take() else {
        return StagingOutcome::Terminal(None);
    };
    match std::fs::File::open(&path)
        .map_err(PlenoraError::Io)
        .and_then(|file| {
            StreamReader::try_new(file, None)
                .map_err(|error| PlenoraError::Internal(format!("replay staging IPC: {error}")))
        }) {
        Ok(reader) => StagingOutcome::Replay(StagedReplay {
            reader,
            staged: staged_meta,
            _dir: dir,
        }),
        Err(error) => StagingOutcome::Terminal(Some(error)),
    }
}

/// Validazione atomica dell'input geometrico: staging bounded + replay.
fn atomic_input_validation_stream(
    mut input: BatchStream,
    state: Rc<ExecState>,
    edge: String,
) -> BatchStream {
    let mut terminal: Option<std::vec::IntoIter<Result<GovernedBatch>>> = None;
    let mut replay: Option<StagedReplay> = None;
    Box::new(std::iter::from_fn(move || {
        if terminal.is_none() && replay.is_none() {
            match stage_input_batches(&mut input, &state, &edge) {
                StagingOutcome::Terminal(error) => {
                    terminal = Some(
                        error
                            .map_or_else(Vec::new, |error| vec![Err(error)])
                            .into_iter(),
                    );
                }
                StagingOutcome::Replay(staged) => replay = Some(staged),
                // Il gate WKB dell'input resta su disco: `stage_input_batches`
                // non produce mai la variante in memoria. Braccio
                // fail-closed, non silenzioso.
                StagingOutcome::Memoria(_) => {
                    terminal = Some(
                        vec![Err(PlenoraError::Internal(
                            "staging input: modalita' memoria non prevista dal gate WKB".into(),
                        ))]
                        .into_iter(),
                    );
                }
            }
        }
        if let Some(active) = terminal.as_mut() {
            return active.next();
        }
        let active = replay.as_mut()?;
        replay_staged_batch(&state, active, &edge)
    }))
}

// Il contesto di validazione e' un gruppo di parametri coeso (celle, limiti,
// framing, offset): estrarlo in una struct dedicata non aggiungerebbe
// informazione in questo choke point interno.
#[allow(clippy::too_many_arguments)]
fn validate_wkb_cells(
    state: &ExecState,
    batch: &RecordBatch,
    geometry_index: usize,
    edge: &str,
    dimensions: GeometryDimensions,
    encoding: GeometryEncoding,
    expected_srid: Option<u32>,
    source_offset: u64,
) -> Result<()> {
    const EXAMPLES_LIMIT: u64 = 10;
    let cells = batch_geometry_cells(batch, geometry_index, "geometry")?;
    let limits = state.plan.limits();
    let max_cell = limits.max_wkb_cell_bytes;
    let max_depth = limits.max_geometry_depth as usize;
    // Diagnostica opt-in (errori arricchiti): il nome della colonna e' contesto
    // strutturale, non un valore — risolto solo nel ramo d'errore (hot path minimale),
    // mai allocato sul percorso felice.
    let column = batch.schema().field(geometry_index).name().clone();
    let mut rejected = Vec::new();
    for row in 0..batch.num_rows() {
        if cells.is_null(row) {
            continue;
        }
        let payload = cells.value(row);
        if payload.len() as u64 > max_cell {
            rejected.push((row, "geometry.cell_too_large"));
            continue;
        }
        if let Err(error) = validate_wkb_transport_for_dimensions_with_depth(
            payload,
            dimensions,
            encoding,
            expected_srid,
            max_depth,
        ) {
            rejected.push((
                row,
                if matches!(error, PlenoraError::Crs(_)) {
                    "geometry.crs_mismatch"
                } else {
                    "geometry.invalid_wkb"
                },
            ));
        }
    }
    if rejected.is_empty() {
        return Ok(());
    }
    let observed_total = u64::try_from(rejected.len())
        .map_err(|_| PlenoraError::Internal("troppe rejection WKB".into()))?;
    let mut counts = BTreeMap::new();
    let mut examples = Vec::new();
    for (row, cause) in rejected {
        let count = counts.entry(cause.to_owned()).or_insert(0_u64);
        *count = count
            .checked_add(1)
            .ok_or_else(|| PlenoraError::Internal("overflow conteggio rejection WKB".into()))?;
        if u64::try_from(examples.len())
            .map_err(|_| PlenoraError::Internal("troppi esempi WKB".into()))?
            < EXAMPLES_LIMIT
        {
            examples.push(RowDiagnosticExample {
                source_index: source_offset
                    .checked_add(u64::try_from(row).map_err(|_| {
                        PlenoraError::Internal("indice WKB non rappresentabile".into())
                    })?)
                    .ok_or_else(|| PlenoraError::Internal("overflow indice WKB".into()))?,
                cause: cause.to_owned(),
                column: Some(column.clone()),
                key: None,
                write_state: None,
            });
        }
    }
    let report = RowDiagnostics {
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
    };
    Err(PlenoraError::DataMapping(format!(
        "geometrie non conformi sull'arco `{edge}`; consultare row_diagnostics"
    ))
    .with_phase(ErrorPhase::Read)
    .with_row_diagnostics(report))
}

fn geometry_input_requirements(
    contract: &DataContract,
) -> Result<(
    Option<usize>,
    GeometryDimensions,
    GeometryEncoding,
    Option<u32>,
)> {
    let geometry = contract.active_geometry_column();
    let index = geometry
        .map(|geometry| {
            contract
                .schema
                .column_with_name(&geometry.name)
                .map(|resolved| resolved.0)
                .ok_or_else(|| {
                    PlenoraError::Internal(format!(
                        "colonna geometria `{}` assente dallo schema del contratto",
                        geometry.name
                    ))
                })
        })
        .transpose()?;
    let dimensions = geometry.map_or(GeometryDimensions::Xy, |geometry| geometry.dimensions);
    let declared_encoding = index
        .map(|index| canonical_geometry_encoding(contract.schema.field(index)))
        .transpose()?
        .flatten();
    let contract_encoding = geometry.and_then(|geometry| geometry.encoding);
    if let Some((declared, governed)) = declared_encoding
        .zip(contract_encoding)
        .filter(|(declared, governed)| declared != governed)
    {
        let column = geometry.map_or("<assente>", |geometry| geometry.name.as_str());
        return Err(PlenoraError::Schema(format!(
            "colonna geometria `{column}`: encoding field `{}` incoerente con encoding contratto `{}`",
            declared.as_str(),
            governed.as_str()
        )));
    }
    let encoding = contract_encoding
        .or(declared_encoding)
        .unwrap_or(GeometryEncoding::Wkb);
    let declared_srid = index
        .map(|index| canonical_geometry_srid(contract.schema.field(index)))
        .transpose()?
        .flatten();
    let resolved_srid = geometry
        .and_then(|geometry| geometry.crs.as_resolved())
        .and_then(|crs| {
            crs.authority_srid()
                .or_else(|| plenora_core::crs::authority_code_srid(crs.definition()))
        });
    if resolved_srid
        .zip(declared_srid)
        .is_some_and(|(resolved, declared)| resolved != declared)
    {
        return Err(PlenoraError::Crs(
            "SRID dichiarato incoerente con il CRS risolto".to_owned(),
        ));
    }
    let srid = resolved_srid.or(declared_srid);
    Ok((index, dimensions, encoding, srid))
}

/// Contatori e limiti dell'arco intermedio prodotto da un kernel
/// (`max_rows_per_edge`, `max_batches`, byte per batch).
fn check_edge_batch(state: &ExecState, edge: &str, batch: &RecordBatch) -> Result<()> {
    let _ = check_batch_bytes(state, batch, edge)?;
    check_edge_counts(state, edge, batch.num_rows() as u64)
}

/// Conteggi righe/batch di un arco intermedio e limiti corrispondenti
/// (`max_rows_per_edge`, `max_batches`) SENZA il tetto byte del batch:
/// archi interni dei gruppi fusi geo (architettura.md#geometrie D12.8, deroga errori-e-limiti.md#limiti-dichiarati di
/// `docs/errori-e-limiti.md` — il batch non e' materializzato e H-03 e' coperto dal
/// governor, reservation D12.7). Righe e batch restano esatti (1:1).
fn check_edge_counts(state: &ExecState, edge: &str, rows: u64) -> Result<()> {
    let mut counts = state.edge_counts.borrow_mut();
    // Chiave clonata solo al primo batch dell'arco (hot path minimale): i batch successivi
    // entrano dal `get_mut` sul nome esistente.
    if let Some(entry) = counts.get_mut(edge) {
        entry.0 = entry
            .0
            .checked_add(rows)
            .ok_or_else(|| PlenoraError::Internal("overflow conteggio righe arco".into()))?;
        entry.1 = entry
            .1
            .checked_add(1)
            .ok_or_else(|| PlenoraError::Internal("overflow conteggio batch arco".into()))?;
    } else {
        counts.insert(edge.to_owned(), (rows, 1));
    }
    let entry = &counts[edge];
    let limits = &state.plan.limits();
    if entry.0 > limits.rows.max_rows_per_edge {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_rows_per_edge superato sull'arco `{edge}`: {} righe > {}",
            entry.0, limits.rows.max_rows_per_edge
        )));
    }
    if entry.1 > limits.max_batches {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_batches superato sull'arco `{edge}`: {} batch > {}",
            entry.1, limits.max_batches
        )));
    }
    Ok(())
}

/// Esenzione da `max_expansion_factor` dichiarata in catalogo (errori-e-limiti.md):
/// le op `WholeToMany` (`geo.generate_grid`, `geo.coverage_validate`,
/// `geo.shared_paths`) sono generative/diagnostiche — l'input funge da
/// trigger o da insieme da analizzare, non da base proporzionale
/// dell'output. Risolta in `prepare` (hot path minimale: nessuno scan del catalogo nel
/// loop per batch).
const fn expansion_exempt(kernel: &PreparedKernel) -> bool {
    kernel.expansion_factor_exempt
}

/// Comportamento alla cancellazione del kernel (errori-e-limiti.md#cancellazione): dichiarato in
/// catalogo dal descriptor dell'operazione e risolto in `prepare` (hot path minimale:
/// nessuno scan del catalogo nel loop per batch).
// Non const fn: sotto cfg(test) chiama l'hook `test_behavior_override`
// (non const) — nel build normale il corpo e' la sola lettura di campo.
#[allow(clippy::missing_const_for_fn)]
fn cancellation_behavior(kernel: &PreparedKernel) -> CancellationBehavior {
    #[cfg(test)]
    if let Some(behavior) = test_behavior_override(&kernel.node_id) {
        return behavior;
    }
    kernel.cancellation_behavior
}

/// Hook di test (errori-e-limiti.md#cancellazione): override del
/// `CancellationBehavior` di catalogo
/// per id nodo. Serve a verificare il rispetto dei behavior senza i backend
/// opzionali: le sole op `NonInterruptible` del catalogo v1
/// (`geo.make_valid`, `geo.reproject`, `geo.polygonize`, `geo.split`)
/// richiedono le capability `geos`/`proj`. Stesso pattern di
/// [`PANIC_AT_NODES`]: insieme, registrazione/deregistrazione per test.
#[cfg(test)]
static CANCEL_BEHAVIOR_OVERRIDES: std::sync::Mutex<Vec<(String, CancellationBehavior)>> =
    std::sync::Mutex::new(Vec::new());

/// Lettura dell'override di behavior di test (scatta solo ai nodi
/// registrati nell'hook).
#[cfg(test)]
fn test_behavior_override(node_id: &str) -> Option<CancellationBehavior> {
    CANCEL_BEHAVIOR_OVERRIDES
        .lock()
        .expect("hook behavior non avvelenato")
        .iter()
        .find_map(|(node, behavior)| (node == node_id).then_some(*behavior))
}

/// Fattore di espansione per nodo unario (errori-e-limiti.md: base = righe di input;
/// per i binari si veda [`check_join_expansion`]).
///
/// Per le op esenti (dichiarato in catalogo, [`expansion_exempt`]) il
/// controllo sul fattore output/input non si applica: la produzione resta
/// limitata da `max_rows_per_edge` / `max_output_rows` / `max_batches`.
fn check_expansion(state: &ExecState, kernel: &PreparedKernel, base_rows: u64) -> Result<()> {
    if expansion_exempt(kernel) {
        return Ok(());
    }
    let mut rows = state.node_rows.borrow_mut();
    // Chiave clonata solo al primo batch del nodo (hot path minimale): i batch successivi
    // entrano dal `get_mut` sull'id esistente.
    if let Some(entry) = rows.get_mut(&kernel.node_id) {
        entry.0 = entry.0.saturating_add(base_rows);
    } else {
        let entry = rows.entry(kernel.node_id.clone()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(base_rows);
    }
    let entry = &rows[&kernel.node_id];
    let factor = state.plan.limits().rows.max_expansion_factor;
    if plenora_core::limits::expansion_exceeded(entry.1, entry.0, factor) {
        return Err(PlenoraError::ResourceLimit(format!(
            "max_expansion_factor superato al nodo `{}`: {} righe output > {} x {} righe input",
            kernel.node_id, entry.1, factor, entry.0
        )));
    }
    Ok(())
}

/// Fattore di espansione per nodo binario (errori-e-limiti.md): calcola tutte le
/// metriche [`JoinExpansion`] e applica il vincolo vincolante dichiarato in
/// catalogo per l'operazione (default `SumRelative` se l'op non e' in
/// catalogo — non dovrebbe accadere: il piano e' validato sul catalogo).
/// La soglia e' `max_expansion_factor` dei limiti effettivi, tranne per il
/// vincolo `Custom(fattore)`: il fattore dichiarato in catalogo la
/// sovrascrive per la singola operazione (stima a priori,
/// architettura.md#planner-ed-executor, errori-e-limiti.md).
fn check_join_expansion(
    state: &ExecState,
    kernel: &PreparedKernel,
    left_rows: u64,
    right_rows: u64,
    output_rows: u64,
) -> Result<()> {
    let descriptor = find_operation(kernel.operation);
    if descriptor.is_some_and(|d| d.expansion_factor_exempt) {
        return Ok(());
    }
    let constraint =
        descriptor.map_or(ExpansionConstraint::SumRelative, |d| d.expansion_constraint);
    let max_expansion_factor = state.plan.limits().rows.max_expansion_factor;
    if !constraint.exceeded(output_rows, left_rows, right_rows, max_expansion_factor) {
        return Ok(());
    }
    // Le metriche si calcolano solo per RACCONTARE l'esito: la decisione e'
    // gia' presa sui conteggi interi, e i rapporti `f64` che seguono sono
    // arrotondati sopra 2^53 righe — cioe' proprio dove il difetto stava.
    let expansion = JoinExpansion::compute(output_rows, left_rows, right_rows);
    let factor = constraint.binding_threshold(max_expansion_factor);
    Err(PlenoraError::ResourceLimit(format!(
        "max_expansion_factor superato al nodo `{}` (vincolo {constraint:?}): \
         soglia {factor}, output={output_rows}, left={left_rows}, right={right_rows}; \
         metriche osservate (approssimate): output/(left+right)={}, \
         output/left={}, output/right={}",
        kernel.node_id,
        expansion.output_over_sum_inputs,
        expansion.output_over_left,
        expansion.output_over_right,
    )))
}

/// Errore di un kernel attribuito al nodo logico (osservabilita' per nodo), preservando la
/// diagnosi senza dati sensibili. L'`execution_id` e' vuoto qui (il dispatch
/// in profondita' non lo ha a disposizione): lo riempie il tag di categoria al
/// confine di uscita (`ExecState::tag_execution`).
fn step_error(kernel: &PreparedKernel, error: PlenoraError) -> PlenoraError {
    if let Some(diagnostics) = error.row_diagnostics().cloned() {
        let replayed = PlenoraError::Replayed(Box::new(ReplayedError {
            category: error.category(),
            phase: error.phase(),
            remote_effect: error.remote_effect(),
            retry: error.retry_disposition(),
            message: error.to_string(),
            node: Some(kernel.node_id.clone()),
            operation: Some(kernel.operation.to_owned()),
            execution_id: None,
            execution_reason: error.execution_reason().map(ToOwned::to_owned),
        }));
        return replayed.with_row_diagnostics(diagnostics);
    }
    // Un limite di RISORSA non diventa `Execution`: la categoria e' cio' su
    // cui il chiamante decide (rilanciare con piu' budget, non correggere il
    // piano), e avvolgerla in `Execution` la faceva sparire — l'errore usciva
    // come `execution`/exit 6 invece di `resource_limit`/exit 4. Si conserva
    // la categoria e si aggiunge il contesto del nodo tramite `Replayed`, che
    // e' il portatore tipizzato di categoria + attribuzione.
    // Il riconoscimento passa da `category()`, non da un `matches!` sulla
    // variante ESTERNA: un `ResourceLimit` puo' arrivare dentro un involucro
    // trasparente — `Tagged` (fase dichiarata da un confine) o un `Replayed`
    // gia' costruito da un livello piu' interno — e in quel caso il match
    // sulla variante non lo vedeva, quindi la categoria si perdeva
    // esattamente nei casi in cui era stata dichiarata con piu' cura.
    // `category()` attraversa gli involucri per costruzione.
    //
    // QUALI categorie si preservano lo decide `error_propagation`, non questa
    // funzione: il gemello legacy fa la stessa scelta, e due elenchi scritti
    // a mano in due file erano gia' divergenti.
    if crate::error_propagation::categoria_preservata(error.category()) {
        return PlenoraError::Replayed(Box::new(ReplayedError {
            category: error.category(),
            phase: error.phase(),
            remote_effect: error.remote_effect(),
            retry: error.retry_disposition(),
            message: error.to_string(),
            node: Some(kernel.node_id.clone()),
            operation: Some(kernel.operation.to_owned()),
            execution_id: None,
            execution_reason: error.execution_reason().map(ToOwned::to_owned),
        }));
    }
    let reason = match error {
        PlenoraError::Execution { reason, .. } => reason,
        other => other.to_string(),
    };
    PlenoraError::Execution {
        node: kernel.node_id.clone(),
        operation: kernel.operation.to_owned(),
        execution_id: String::new(),
        reason,
    }
}

/// Fusione dei report row-scoped: la procedura vive in
/// [`RowDiagnostics::merge_into`], condivisa con il runner fuso del trasporto
/// geo. Qui resta solo la traduzione dell'invariante violata nell'errore di
/// questo perimetro.
fn merge_row_diagnostics(
    aggregate: &mut Option<RowDiagnostics>,
    incoming: RowDiagnostics,
    source_offset: u64,
) -> Result<()> {
    RowDiagnostics::merge_into(aggregate, incoming, source_offset)
        .map_err(|error| PlenoraError::Internal(error.message().to_owned()))
}

fn attach_partial_row_diagnostics(
    error: PlenoraError,
    aggregate: &mut Option<RowDiagnostics>,
    knowledge_limit: &str,
) -> PlenoraError {
    let Some(report) = aggregate.take() else {
        return error;
    };
    error.with_row_diagnostics(report.into_partial(knowledge_limit))
}

fn complete_row_diagnostic_error(
    report: RowDiagnostics,
    context: Option<(String, String, String)>,
) -> PlenoraError {
    let source = match context {
        Some((node, operation, execution_id)) => PlenoraError::Replayed(Box::new(ReplayedError {
            category: ErrorCategory::DataMapping,
            phase: ErrorPhase::Read,
            remote_effect: plenora_core::RemoteEffect::None,
            retry: RetryDisposition::Never,
            message: "righe non conformi al contratto di trasformazione".into(),
            node: Some(node),
            operation: Some(operation),
            execution_id: Some(execution_id),
            execution_reason: None,
        })),
        None => {
            PlenoraError::DataMapping("righe non conformi al contratto di trasformazione".into())
                .with_phase(ErrorPhase::Read)
        }
    };
    source.with_row_diagnostics(report)
}

/// Selezione del machinery row-diagnostics per segmento (R9.9): un kernel
/// vi partecipa se e solo se l'autorita' di catalogo
/// (`OperationDescriptor::emits_row_diagnostics`, risolta in `prepare`
/// sulla config del nodo) lo dichiara emittente — stessa classificazione
/// del gate provenance del planner e del gate legacy CLI, nessuna lista
/// locale duplicata.
fn segment_emits_row_diagnostics(plan: &ExecutionPlan, segment_index: usize) -> bool {
    plan.segments()[segment_index]
        .kernels
        .iter()
        .any(|kernel| kernel.emits_row_diagnostics)
}

/// Permesso a eseguire la prossima passata **trattenendo** cio' che c'e' gia'.
///
/// Non e' una verifica seguita da una prenotazione: e' **una sola
/// operazione**. Si chiede al governor un permesso per `max_batch_bytes` —
/// il tetto duro per batch (tetto in byte per batch), che il wrapper d'uscita applica a ogni
/// batch di output ed e' quindi un maggiorante valido dell'unica prenotazione
/// che la passata aggiunge. Se il permesso e' concesso, quella quota e'
/// **gia' nostra**: la passata puo' ritagliarne l'output senza che nessun
/// altro possa infilarsi nel mezzo, oggi che l'esecuzione e' seriale come
/// domani che non lo sara'.
///
/// `None` significa "non c'e' spazio per un'altra passata trattenendo": si
/// passa al disco. Non e' un errore ed e' fail-closed — un permesso negato
/// sceglie sempre la modalita' col picco piu' basso.
///
/// In modalita' disco non si chiede nulla: il passaggio e' definitivo.
///
/// Un ingresso **senza lease** non e' contabilizzato dal governor: la
/// decisione poggerebbe su un totale che non comprende i byte in arrivo, e si
/// va su disco.
///
/// # Errors
///
/// Propaga l'errore interno del governor se la sua contabilita' e'
/// incoerente: un diniego di budget (`Ok(None)`) e una contabilita' rotta
/// (`Err`) restano distinti fino in cima, perche' il primo si gestisce
/// passando al disco e il secondo no.
fn permesso_di_trattenere(
    state: &ExecState,
    accepted: &StagingAccepted,
    ingresso: &GovernedBatch,
    edge: &str,
) -> Result<Option<MemoryPermit>> {
    if matches!(accepted, StagingAccepted::Disco { .. }) {
        return Ok(None);
    }
    // Un ingresso senza lease non e' contabilizzato dal governor.
    if ingresso.lease.is_none() {
        return Ok(None);
    }
    let Ok(tetto_batch) = u64::try_from(state.plan.batch_target().max_batch_bytes) else {
        return Ok(None);
    };
    // L'owner e' l'arco, non una costante: architettura.md#memoria vuole che un lease vivo
    // sia attribuibile, e `oldest_lease_age` con `owner` e' l'unico modo di
    // sapere CHI sta trattenendo quota. Un nome generico renderebbe la
    // diagnosi impossibile proprio sul lease piu' grande del piano.
    state.governor.permesso(tetto_batch, edge)
}

// Macchina a stati lineare: scansione, decisione memoria/disco, diagnostica e
// chiusura restano nello stesso scope per rendere evidente il cleanup
// fail-closed.
#[allow(clippy::too_many_lines)]
fn scan_row_diagnostic_segment(
    input: &mut EdgeStream,
    plan: &Rc<ExecutionPlan>,
    state: &Rc<ExecState>,
    segment_index: usize,
) -> StagingOutcome {
    let mut diagnostics = None;
    let mut diagnostic_context = None;
    let mut input_rejected = false;
    let mut source_offset = 0_u64;
    let mut terminal_error = None;
    let mut accepted = StagingAccepted::nuovo();
    let output_edge = &plan.segments()[segment_index].output_edge;
    for item in input {
        let governed = match item {
            Ok(governed) => governed,
            Err(error) => {
                if let Some(report) = error.row_diagnostics().cloned() {
                    if diagnostic_context.is_none() {
                        diagnostic_context = error.execution_location().map_or_else(
                            || {
                                plan.segments()[segment_index]
                                    .kernels
                                    .first()
                                    .map(|kernel| {
                                        (
                                            kernel.node_id.clone(),
                                            kernel.operation.to_owned(),
                                            state.execution_id.clone(),
                                        )
                                    })
                            },
                            |(node, operation, execution_id)| {
                                Some((
                                    node.to_owned(),
                                    operation.to_owned(),
                                    execution_id.map_or_else(
                                        || state.execution_id.clone(),
                                        ToOwned::to_owned,
                                    ),
                                ))
                            },
                        );
                    }
                    input_rejected = true;
                    if let Err(error) = merge_row_diagnostics(&mut diagnostics, report, 0) {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            error,
                            &mut diagnostics,
                            "data_tools.diagnostic_merge_failed",
                        ));
                        break;
                    }
                    continue;
                }
                terminal_error = Some(attach_partial_row_diagnostics(
                    error,
                    &mut diagnostics,
                    "data_tools.input_stream_interrupted",
                ));
                break;
            }
        };
        let batch_offset = source_offset;
        let Ok(batch_rows) = u64::try_from(governed.batch.num_rows()) else {
            terminal_error = Some(attach_partial_row_diagnostics(
                PlenoraError::Internal("cardinalita batch fuori intervallo".into()),
                &mut diagnostics,
                "data_tools.source_offset_unrepresentable",
            ));
            break;
        };
        let Some(offset) = source_offset.checked_add(batch_rows) else {
            terminal_error = Some(attach_partial_row_diagnostics(
                PlenoraError::Internal("indice sorgente stream fuori intervallo".into()),
                &mut diagnostics,
                "data_tools.source_offset_overflow",
            ));
            break;
        };
        source_offset = offset;
        // Una rejection di validazione input (WKB) e' attribuita al primo
        // kernel consumatore del segmento, ma continua a drenare/validare
        // l'input per completare i conteggi senza eseguire kernel downstream.
        if diagnostics.is_some() && input_rejected {
            continue;
        }
        // architettura.md#memoria, staging memory-first: la decisione memoria/disco si prende QUI, con il
        // batch d'ingresso gia' prelevato e quindi di dimensione NOTA, e
        // PRIMA di eseguire la catena su di esso. Cosi' la passata successiva
        // non puo' superare il budget: se non ci sta, i trattenuti vanno su
        // disco adesso, non dopo il fallimento.
        let permesso = match permesso_di_trattenere(state, &accepted, &governed, output_edge) {
            Ok(permesso) => permesso,
            Err(error) => {
                // Contabilita' del governor incoerente: e' un'invariante
                // nostra rotta, non un budget esaurito. Termina la scansione
                // senza pubblicare accepted, come ogni altro errore.
                terminal_error = Some(attach_partial_row_diagnostics(
                    error,
                    &mut diagnostics,
                    "data_tools.governor_accounting_broken",
                ));
                break;
            }
        };
        if permesso.is_none() {
            if let Err(error) = accepted.passa_a_disco(state, output_edge) {
                terminal_error = Some(attach_partial_row_diagnostics(
                    error,
                    &mut diagnostics,
                    "data_tools.output_staging_failed",
                ));
                break;
            }
        }
        let diagnostic_node = diagnostic_context
            .as_ref()
            .map(|(node, _, _): &(String, String, String)| node.as_str());
        match run_streaming_chain(
            plan,
            segment_index,
            state,
            governed,
            diagnostic_node,
            permesso,
        ) {
            Ok(output) => {
                if diagnostics.is_none() {
                    // La barriera R9.9 non richiede il disco: richiede che
                    // nulla esca prima della fine della scansione. In
                    // memoria il lease resta vivo e il batch e' consegnato
                    // tale e quale; su disco il lease e' rilasciato qui e
                    // ri-riservato al replay. Se una rejection tardiva
                    // arriva dopo, in entrambi i casi non si pubblica nulla.
                    if let Err(error) = accepted.accogli(state, output_edge, output) {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            error,
                            &mut diagnostics,
                            "data_tools.output_staging_failed",
                        ));
                        break;
                    }
                }
            }
            Err(error) => {
                let Some(report) = error.row_diagnostics().cloned() else {
                    terminal_error = Some(attach_partial_row_diagnostics(
                        error,
                        &mut diagnostics,
                        "data_tools.processing_interrupted",
                    ));
                    break;
                };
                if diagnostic_context.is_none() {
                    if let Some((node, operation, execution_id)) = error.execution_location() {
                        diagnostic_context = Some((
                            node.to_owned(),
                            operation.to_owned(),
                            execution_id
                                .map_or_else(|| state.execution_id.clone(), ToOwned::to_owned),
                        ));
                    }
                }
                if let Err(error) = merge_row_diagnostics(&mut diagnostics, report, batch_offset) {
                    terminal_error = Some(attach_partial_row_diagnostics(
                        error,
                        &mut diagnostics,
                        "data_tools.diagnostic_merge_failed",
                    ));
                    break;
                }
            }
        }
    }
    if terminal_error.is_none() {
        if let StagingAccepted::Disco { writer, .. } = &mut accepted {
            if let Some(active) = writer.as_mut() {
                if let Err(error) = active.finish() {
                    terminal_error = Some(attach_partial_row_diagnostics(
                        PlenoraError::Internal(format!("chiusura staging output: {error}")),
                        &mut diagnostics,
                        "data_tools.output_staging_failed",
                    ));
                }
            }
        }
    }
    if terminal_error.is_none() {
        if let Err(error) = state.check_cancellation_point("output", "row_diagnostics") {
            terminal_error = Some(attach_partial_row_diagnostics(
                error,
                &mut diagnostics,
                "data_tools.cancelled_after_rejection",
            ));
        } else if let Some(report) = diagnostics.take() {
            terminal_error = Some(complete_row_diagnostic_error(
                report,
                diagnostic_context.take(),
            ));
        }
    }
    if let Some(error) = terminal_error {
        // Errore, rejection tardiva o cancellazione: `accepted` e' distrutto
        // qui. In memoria i lease muoiono con la coda, su disco il `TempDir`
        // cancella il file. In nessuno dei due casi esce un batch.
        return StagingOutcome::Terminal(Some(error));
    }
    let (writer, staging, staged_meta) = match accepted {
        // Modalita' memoria: la coda si consegna com'e', in ordine, con i
        // lease gia' vivi. Nessun IPC, nessuna decodifica, nessuna copia.
        StagingAccepted::Memoria(coda) => {
            if coda.is_empty() {
                return StagingOutcome::Terminal(None);
            }
            return StagingOutcome::Memoria(coda);
        }
        StagingAccepted::Disco {
            writer,
            staging,
            meta,
        } => (writer, staging, meta),
    };
    drop(writer);
    let Some((dir, path)) = staging else {
        return StagingOutcome::Terminal(None);
    };
    match std::fs::File::open(&path)
        .map_err(PlenoraError::Io)
        .and_then(|file| {
            StreamReader::try_new(file, None)
                .map_err(|error| PlenoraError::Internal(format!("replay staging IPC: {error}")))
        }) {
        Ok(reader) => StagingOutcome::Replay(StagedReplay {
            reader,
            staged: staged_meta,
            _dir: dir,
        }),
        Err(error) => StagingOutcome::Terminal(Some(error)),
    }
}

/// Machinery R9.9 per i segmenti che emettono diagnostica: scansione
/// completa (staging bounded degli accepted) seguita da replay lazy con
/// ri-riserva per batch — nessun accepted esce prima dello scan completo,
/// nessun lease trattenuto oltre il singolo batch.
fn row_diagnostic_stream(
    mut input: EdgeStream,
    plan: Rc<ExecutionPlan>,
    state: Rc<ExecState>,
    segment_index: usize,
) -> BatchStream {
    let mut terminal: Option<std::vec::IntoIter<Result<GovernedBatch>>> = None;
    let mut replay: Option<StagedReplay> = None;
    let mut memoria: Option<std::collections::VecDeque<GovernedBatch>> = None;
    let mut scansione_fatta = false;
    Box::new(std::iter::from_fn(move || {
        if !scansione_fatta {
            scansione_fatta = true;
            match scan_row_diagnostic_segment(&mut input, &plan, &state, segment_index) {
                StagingOutcome::Terminal(error) => {
                    terminal = Some(
                        error
                            .map_or_else(Vec::new, |error| vec![Err(error)])
                            .into_iter(),
                    );
                }
                StagingOutcome::Replay(staged) => replay = Some(staged),
                StagingOutcome::Memoria(coda) => memoria = Some(coda),
            }
        }
        if let Some(active) = terminal.as_mut() {
            return active.next();
        }
        // Modalita' memoria: consegna in ordine di produzione, con il lease
        // e la `BatchSequence` originali. Il batch e' lo stesso oggetto
        // prodotto dalla catena — nessun round-trip IPC puo' alterarlo.
        if let Some(coda) = memoria.as_mut() {
            return coda.pop_front().map(Ok);
        }
        let active = replay.as_mut()?;
        let output_edge = &plan.segments()[segment_index].output_edge;
        replay_staged_batch(&state, active, output_edge)
    }))
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
