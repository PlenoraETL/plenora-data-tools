//! Executor del DAG — fase 2 `execute` (Architetture.md par. 6.3, ADR 5;
//! Prestazioni.md V2-V4, V7-V10, E3) — Fase 2A-4.
//!
//! [`execute`] accetta solo un [`ValidatedGraph`] (type-state: nessun
//! percorso non validato raggiunge l'esecuzione), svolge internamente
//! `prepare` + `execute_physical` (ADR 5) e restituisce un [`Output`] a
//! **pull**: i batch finali sono uno stream lazy, l'input e' consumato
//! batch-per-batch man mano che il chiamante tira l'output (V3: una
//! pipeline streaming non materializza l'intera tabella).
//!
//! Esecuzione v1: **seriale** ovunque (`SerialFused`, V8 — parallelismo e
//! spill restano Fase 2B: M2/M3; il governor della memoria e' entrato con
//! M1a/M1b, la cancellazione cooperativa e gli errori arricchiti con
//! M1c/M1d, vedi sotto). Per questo lo stream usa `Rc`/`RefCell` e
//! [`Output`] non e' `Send`: e' una scelta documentata, non un limite
//! nascosto.
//!
//! Fase 2B M1c — cancellazione cooperativa (ADR 3): il chiamante passa un
//! [`crate::cancellation::CancellationToken`] nel [`RuntimeContext`] e lo
//! cancella dall'esterno (es. handler Ctrl-C della CLI). I check sono solo
//! ai confini dell'executor — tra batch nelle catene streaming, tra kernel,
//! durante il drenaggio dei segmenti blocking e sull'output del piano — e
//! onorano il `CancellationBehavior` dichiarato in catalogo: `Cooperative`
//! (check a ogni batch), `BoundaryOnly` (check tra kernel/a fine kernel e
//! durante il drenaggio), `NonInterruptible` (mai: l'op completa; i confini
//! di piano a valle restano attivi — nessuna nuova attivita' dopo la
//! cancellazione, publish compreso). I kernel NON vedono il token in M1 (il
//! passaggio e' M3). Al cancel l'errore e' `PlenoraError::Cancelled` con
//! `node`/`operation`/`execution_id`; nessun output e' pubblicato (publish
//! atomico) e le metriche parziali restano osservabili iterando [`Output`]
//! manualmente e chiamando [`Output::metrics`] dopo l'errore (i metodi di
//! comodo `collect_batches`/`write_ipc_file*` consumano l'`Output`: con
//! loro le metriche al punto di cancel vanno perse, limite v1 documentato).
//!
//! Fase 2B M1d — errori arricchiti (ADR 3): ogni `execute` genera un
//! `execution_id` (UUID v4 — dipendenza `uuid` gia' pinnata nel workspace,
//! nessuna versione nuova) riportato negli errori `Execution`/`Cancelled` e nel
//! lock del [`crate::temp_store::TempStore`]; `PlenoraError` espone
//! `category()` (poi `phase()`, `remote_effect()` e `retry_disposition()` —
//! gli assi §9; R9.7 ha sostituito il `retryable()` di M1d). La modalita'
//! diagnostica opt-in
//! (`RuntimeContext::diagnostics`, solo per input fidati) aggiunge alla
//! motivazione contesto strutturale — indice di batch, riga, colonna dove
//! disponibile — MAI valori; a flag spento i messaggi sono invariati.
//!
//! Fase 2B — `TempStore` (ADR 3): `execute` esegue lo scavenging
//! best-effort delle directory orfane all'avvio (sulla radice configurata,
//! default temp di sistema) e crea lo store dell'esecuzione **fail-closed**
//! (decisione documentata: niente degrado a tempdir semplice — lo store e'
//! la difesa strutturale contro i crash non intercettabili e lo spill di M2
//! ci scrivera'; se non e' creabile l'esecuzione fallisce prima di toccare
//! i dati). L'heartbeat e' scritto al punto centrale (ogni batch processato
//! passa dal conteggio metriche) con throttle di
//! [`HEARTBEAT_MIN_INTERVAL`]; il cleanup e' RAII al `Drop` dello stato.
//!
//! Fase 2B M1a/M1b — resource accounting (ADR-0002) e sequenza logica
//! (ADR-0001): i batch attraversano gli archi come [`GovernedBatch`]
//! (batch, [`MemoryLease`] e [`BatchSequence`]). La quota `max_memory_bytes`
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
//!   consumatore = pass-through puro; piu' consumatori (fan-out, D9/V9) =
//!   tee che condivide i `RecordBatch` immutabili senza copie di buffer e
//!   rilascia ciascun batch quando tutti i consumatori lo hanno letto
//!   (V10). Il tee bufferizza [`GovernedBatch`]: il lease e' condiviso (clone
//!   `Arc`) tra i consumatori, la quota resta contata una sola volta e torna
//!   al governor con `release_consumed` + `Drop` dell'ultimo riferimento.
//!   In esecuzione seriale i consumatori drenano in sequenza, quindi
//!   il tee coincide con la materializzazione conservativa di D9;
//! - `LinearStreaming`/`GeoFused`: il batch attraversa la catena di kernel
//!   senza code ne' materializzazioni (V4). Nei segmenti `GeoFused` i run di
//!   kernel fondibili annotati da `prepare` (campo `fusion_group`) sono
//!   eseguiti col runner fuso di ADR-0012 — un decode/encode per gruppo su
//!   ogni batch, con errori/metriche/cancellazione per nodo preservati e
//!   fallback strumentato al percorso nodo-per-nodo a reservation governor
//!   fallita (D12.6/D12.7);
//! - `Blocking`/`BinaryBlocking`: alla prima pull drenano gli input del
//!   segmento (materializzazione prevista dal piano, V9), concatenano ed
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
//!   vincolo vincolante dichiarato in catalogo, ADR 6; le op `WholeToMany`
//!   generative/diagnostiche sono esenti — esenzione dichiarata in
//!   catalogo, la base input e' un trigger, insensata come denominatore),
//!   `max_batches` per arco, `max_wkb_cell_bytes` per cella,
//!   `max_payload_bytes` cumulati per input, `max_geometry_depth` per
//!   annidamento WKB, `max_batch_bytes` per batch (V7, tetto duro, applicato
//!   anche al batch concatenato dei segmenti blocking);
//! - nessun output parziale: [`Output::write_ipc_file`] scrive via
//!   [`crate::geo_transport::publish::publish_atomic`] (tempfile + persist
//!   no-clobber solo a stream completato con successo);
//! - metriche per nodo logico e per segmento (E3), prefilled per tutti i
//!   nodi del piano e aggiornate batch per batch.
//!
//! Errore a meta' stream: il batch in errore propaga `Err` nello stream di
//! output; niente viene pubblicato (il tempfile e' eliminato da
//! `publish_atomic`) e le metriche restano consultabili fino al punto di
//! fallimento.
//!
//! Panic dei kernel (ADR 3): intercettati con `catch_unwind` al punto di
//! dispatch — [`run_kernel`] per i kernel unari (streaming e blocking) e la
//! chiamata `execute_binary` per i segmenti binari, il livello piu' interno
//! che conserva l'attribuzione di nodo — e convertiti in
//! `PlenoraError::Execution { node, operation, .. }` con il solo messaggio del
//! panic, mai dati dei batch (regola di error.rs). L'errore propaga come
//! qualunque altro: il publish atomico non e' raggiunto (nessun publish
//! dopo panic) e il cleanup (tempfile, buffer degli archi) avviene comunque
//! via `Drop`. I confini `UnwindSafe` dichiarati per il DAG parallelo
//! (worker, cancellazione globale, spill) restano Fase 2B.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::ipc::reader::{FileReader, StreamReader};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{Schema, SchemaRef};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::arrow::select::take::take;
use plenora_core::catalog::{
    find_operation, CancellationBehavior, ExpansionConstraint, JoinExpansion, CATALOG,
};
use plenora_core::contract::{BatchSequence, ContractCrs, DataContract, GeometryDimensions};
use plenora_core::{ErrorPhase, PlenoraError, Result};
use plenora_kernels_geo::arrow_adapter::{
    batch_geometry_cells, canonical_geometry_metadata, canonical_schema_version_metadata,
    decode_geometry_cell, strip_decided_crs_declarations, GeometryMetadataDetails,
    PLENORA_GEOMETRY_AXIS_ORDER_KEY, PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
};
use plenora_kernels_geo::{operations, validate_wkb_contract_for_dimensions_with_depth};
use plenora_kernels_table::spill::SpillMetrics;

use crate::cancellation::CancellationToken;
use crate::geo_transport::publish::{publish_with_profile, PublishOutcome, PublishProfile};
use crate::geo_transport::transport::{
    OneToOnePrepared, TransformArrowSchema, one_to_one_batch_prepared, prepare_one_to_one,
};
use crate::geo_transport::unary::{
    FusedStepError, FusedTerminal, FusedTerminalMeasure, one_to_one_batch_fused,
};
use crate::governor::{GovernedBatch, MemoryGovernor, MemoryLease, MemoryMetrics, ReservationResult};
use crate::planner::{
    check_compatibility, local_capabilities, ValidatedGraph, ARROW_VERSION, ENGINE_VERSION,
};
use crate::prepare::{
    prepare, AccessorKind, ExecutionPlan, MeasureKind, PhysicalSegment, PreparedConfig,
    PreparedKernel, RuntimeContext, SegmentMode,
};
use crate::table_engine;
use crate::temp_store::{scavenge_stale_temp_dirs, TempStore, DEFAULT_SCAVENGE_TTL};

/// Stream di batch del grafo (seriale, thread-locale nella v1): i batch
/// viaggiano governati — quota di memoria (ADR-0002) e sequenza logica
/// (ADR-0001) — e sono spaccati/ricomposti solo ai confini dei kernel.
type BatchStream = Box<dyn Iterator<Item = Result<GovernedBatch>>>;

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// Un input del piano: lettore Arrow IPC o iteratore di `RecordBatch`.
pub enum Input {
    /// Batch gia' in memoria.
    Batches(Vec<RecordBatch>),
    /// Sorgente lazy (lettore IPC o qualunque iteratore di batch).
    Stream {
        /// Schema dichiarato della sorgente (verificato contro il contratto).
        schema: SchemaRef,
        /// Iteratore di batch.
        iter: BatchStream,
    },
}

impl Input {
    /// Input da batch in memoria (schema dal primo batch).
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` se il vettore e' vuoto (per input vuoti usare
    /// [`Input::empty`] con lo schema esplicito).
    pub fn from_batches(batches: Vec<RecordBatch>) -> Result<Self> {
        if batches.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "input da batch: vettore vuoto, usare Input::empty con lo schema".into(),
            ));
        }
        Ok(Self::Batches(batches))
    }

    /// Input vuoto con schema esplicito.
    #[must_use]
    pub fn empty(schema: SchemaRef) -> Self {
        Self::Stream {
            schema,
            iter: Box::new(std::iter::empty()),
        }
    }

    /// Input da un iteratore di batch con schema dichiarato.
    ///
    /// I batch entrano nel perimetro nudi (API pubblica su `RecordBatch`) e
    /// sono avvolti in [`GovernedBatch`] senza lease: quota e sequenza sono
    /// assegnate all'ingresso dell'arco di input (ADR-0002/ADR-0001).
    #[must_use]
    pub fn from_iter<I>(schema: SchemaRef, iter: I) -> Self
    where
        I: Iterator<Item = Result<RecordBatch>> + 'static,
    {
        Self::Stream {
            schema,
            iter: Box::new(
                iter.map(|item| item.map(|batch| GovernedBatch::new(batch, None, None))),
            ),
        }
    }

    /// Lettore Arrow IPC **file format** (lazy: i batch sono letti dal disco
    /// man mano che lo stream di output viene tirato).
    ///
    /// # Errors
    ///
    /// `PlenoraError::Io`/`PlenoraError::DataMapping` se il file non si apre o
    /// l'header IPC non e' valido — taggati [`ErrorPhase::Read`] (BLOCK-03:
    /// nascono leggendo la sorgente).
    pub fn read_ipc_file(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .map_err(|error| PlenoraError::Io(error).with_phase(ErrorPhase::Read))?;
        let reader = FileReader::try_new(file, None)
            .map_err(|error| PlenoraError::from(error).with_phase(ErrorPhase::Read))?;
        let schema = reader.schema();
        Ok(Self::Stream {
            schema,
            iter: Box::new(reader.map(|batch| {
                batch
                    .map_err(PlenoraError::from)
                    .map(|batch| GovernedBatch::new(batch, None, None))
            })),
        })
    }

    /// Lettore Arrow IPC **stream format** (lazy).
    ///
    /// # Errors
    ///
    /// Come [`Input::read_ipc_file`].
    pub fn read_ipc_stream(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .map_err(|error| PlenoraError::Io(error).with_phase(ErrorPhase::Read))?;
        let reader = StreamReader::try_new(file, None)
            .map_err(|error| PlenoraError::from(error).with_phase(ErrorPhase::Read))?;
        let schema = reader.schema();
        Ok(Self::Stream {
            schema,
            iter: Box::new(reader.map(|batch| {
                batch
                    .map_err(PlenoraError::from)
                    .map(|batch| GovernedBatch::new(batch, None, None))
            })),
        })
    }

    /// Schema dichiarato della sorgente.
    fn schema(&self) -> SchemaRef {
        match self {
            Self::Batches(batches) => batches[0].schema(),
            Self::Stream { schema, .. } => schema.clone(),
        }
    }
}

/// Gli input di un'esecuzione, per nome come dichiarati nel piano.
#[derive(Default)]
pub struct Inputs {
    readers: BTreeMap<String, Input>,
}

impl Inputs {
    /// Insieme vuoto.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Aggiunge un input per nome.
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` se il nome e' gia' presente.
    pub fn add(&mut self, name: impl Into<String>, input: Input) -> Result<()> {
        let name = name.into();
        if self.readers.insert(name.clone(), input).is_some() {
            return Err(PlenoraError::InvalidPlan(format!(
                "input duplicato `{name}`"
            )));
        }
        Ok(())
    }

    /// Builder: aggiunge un input e restituisce `self`.
    ///
    /// # Errors
    ///
    /// Come [`Inputs::add`].
    pub fn with(mut self, name: impl Into<String>, input: Input) -> Result<Self> {
        self.add(name, input)?;
        Ok(self)
    }
}

// ---------------------------------------------------------------------------
// Metriche (E3)
// ---------------------------------------------------------------------------

/// Metriche di un nodo logico (presenti anche dentro ai segmenti fusi, E3).
#[derive(Clone, Debug, Default)]
pub struct NodeMetrics {
    /// Id canonico dell'operazione del nodo.
    pub operation: String,
    /// Righe in ingresso al nodo.
    pub rows_in: u64,
    /// Righe prodotte dal nodo.
    pub rows_out: u64,
    /// Batch in ingresso al nodo.
    pub batches_in: u64,
    /// Batch prodotti dal nodo.
    pub batches_out: u64,
    /// Byte Arrow in ingresso al nodo (metadati dei buffer, E3/ADR-0002).
    pub bytes_in: u64,
    /// Byte Arrow prodotti dal nodo (metadati dei buffer, E3/ADR-0002).
    pub bytes_out: u64,
    /// Wall time cumulato del kernel.
    pub wall_time: Duration,
}

/// Metriche di un segmento fisico.
#[derive(Clone, Debug)]
pub struct SegmentMetrics {
    /// Modalita' fisica del segmento.
    pub mode: SegmentMode,
    /// Righe in ingresso al segmento.
    pub rows_in: u64,
    /// Righe prodotte dal segmento.
    pub rows_out: u64,
    /// Batch in ingresso al segmento.
    pub batches_in: u64,
    /// Batch prodotti dal segmento.
    pub batches_out: u64,
    /// Wall time cumulato del segmento.
    pub wall_time: Duration,
}

/// Metriche di un'esecuzione: per nodo logico, per segmento e sull'output.
///
/// Prefilled per tutti i nodi/segmenti del piano all'avvio: un nodo che non
/// ha ancora visto batch compare con contatori a zero. In caso di errore a
/// meta' stream le metriche restano consultabili fino al punto di
/// fallimento.
#[derive(Clone, Debug, Default)]
pub struct ExecutionMetrics {
    /// Per id nodo.
    pub nodes: BTreeMap<String, NodeMetrics>,
    /// Per id segmento.
    pub segments: BTreeMap<String, SegmentMetrics>,
    /// Righe pubblicate sull'output del piano.
    pub output_rows: u64,
    /// Batch pubblicati sull'output del piano.
    pub output_batches: u64,
    /// Righe processate complessivamente: somma delle righe in ingresso a
    /// ogni nodo del piano (ADR 6: metrica obbligatoria, **non** limite v1 —
    /// dipende dal piano fisico, es. segmenti fusi).
    pub total_rows_processed: u64,
    /// Osservabilita' dei lease (ADR-0002): snapshot del governor al momento
    /// della lettura delle metriche — lease vivi, byte trattenuti attuali e
    /// di picco, eta' del lease piu' vecchio.
    pub memory: MemoryMetrics,
    /// Metriche di spill aggregate sull'esecuzione (ADR-0002, Fase 2B M2c):
    /// byte scritti/letti sui file temporanei e numero di file
    /// materializzati dai percorsi `*_spilled` di sort/distinct/aggregate.
    /// Tutte zero se nessun nodo ha spillato.
    pub spill: SpillMetrics,
    /// Batch per cui il runner fuso geo e' ricaduto sul percorso non fuso
    /// per reservation governor fallita (ADR-0012 D12.7): mai silenzioso —
    /// una pressione ricorrente e' osservabile qui invece di manifestarsi
    /// come rallentamento inspiegabile. Nessun errore nuovo: il risultato e'
    /// identico, cambia solo la scelta fisica.
    pub geo_fusion_fallbacks: u64,
}

// ---------------------------------------------------------------------------
// Stato condiviso dell'esecuzione (seriale, thread-locale)
// ---------------------------------------------------------------------------

/// Stato mutabile condiviso tra le chiusure dello stream (contatori per i
/// limiti effettivi e metriche). `Rc`/`RefCell`: esecuzione seriale v1 (V8).
struct ExecState {
    plan: Rc<ExecutionPlan>,
    metrics: RefCell<ExecutionMetrics>,
    /// Governor del budget memoria globale di piano (ADR-0002, M1a).
    governor: MemoryGovernor,
    /// Identita' dell'esecuzione (ADR 3, M1d): riportata negli errori
    /// `Execution`/`Cancelled` e nel lock del `TempStore`.
    execution_id: String,
    /// Token di cancellazione cooperativa (ADR 3, M1c): osservato solo ai
    /// confini dell'executor, mai dentro ai kernel (M3).
    cancellation: CancellationToken,
    /// Diagnostica opt-in (ADR 3, M1d): arricchisce le motivazioni degli
    /// errori con contesto strutturale, mai valori.
    diagnostics: bool,
    /// Store temporaneo dell'esecuzione (ADR 3): heartbeat al punto
    /// centrale, cleanup RAII al `Drop`.
    temp_store: RefCell<TempStore>,
    /// Directory di spill condivisa (ADR-0002, Fase 2B M2c): `spill/`
    /// sotto il `TempStore`, risolta UNA volta alla costruzione (V2) —
    /// il path e' fisso per tutta l'esecuzione.
    spill_directory: PathBuf,
    /// Metriche di spill aggregate (ADR-0002, Fase 2B M2c): alimentate dai
    /// percorsi `*_spilled` attivati nei nodi tabellari.
    spill_metrics: RefCell<SpillMetrics>,
    /// Istante dell'ultimo heartbeat scritto (throttle).
    last_heartbeat: Cell<Instant>,
    /// Righe/batch/byte cumulati per input (`max_input_rows`, `max_batches`,
    /// `max_payload_bytes`).
    input_counts: RefCell<HashMap<String, (u64, u64, u64)>>,
    /// Righe/batch cumulati per arco intermedio (`max_rows_per_edge`).
    edge_counts: RefCell<HashMap<String, (u64, u64)>>,
    /// Righe in/out cumulate per nodo (`max_expansion_factor`).
    node_rows: RefCell<HashMap<String, (u64, u64)>>,
    /// Handle prepared delle operazioni geo 1:1, costruito una volta per
    /// nodo (V2): indice di colonna e schema di output non sono piu'
    /// risolti a ogni batch.
    prepared_one_to_one: RefCell<HashMap<String, Rc<OneToOnePrepared>>>,
}

/// Intervallo minimo tra due heartbeat del `TempStore` (ADR 3): il punto
/// naturale e' "ogni batch processato", ma la scrittura del lock file ha un
/// costo — un heartbeat al secondo e' di gran lunga piu' frequente del TTL
/// di scavenging (24 ore di default) anche con batch piccolissimi.
const HEARTBEAT_MIN_INTERVAL: Duration = Duration::from_secs(1);

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
            governor: MemoryGovernor::new(plan.limits().max_memory_bytes),
            execution_id,
            cancellation,
            diagnostics,
            temp_store: RefCell::new(temp_store),
            spill_directory,
            spill_metrics: RefCell::new(SpillMetrics::default()),
            last_heartbeat: Cell::new(Instant::now()),
            input_counts: RefCell::new(HashMap::new()),
            edge_counts: RefCell::new(HashMap::new()),
            node_rows: RefCell::new(HashMap::new()),
            prepared_one_to_one: RefCell::new(HashMap::new()),
        })
    }

    /// Handle prepared dell'operazione geo 1:1 del nodo, costruito alla
    /// prima occorrenza (V2): indice di colonna e schema di output sono
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
        let prepared = Rc::new(
            prepare_one_to_one(schema, params)
                .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?,
        );
        self.prepared_one_to_one
            .borrow_mut()
            .insert(kernel.node_id.clone(), prepared.clone());
        Ok(prepared)
    }

    /// Snapshot delle metriche correnti (con l'osservabilita' dei lease
    /// letta dal governor al momento della chiamata, ADR-0002).
    fn metrics(&self) -> ExecutionMetrics {
        let mut metrics = self.metrics.borrow().clone();
        metrics.memory = self.governor.snapshot();
        metrics.spill = *self.spill_metrics.borrow();
        metrics
    }

    /// Directory di spill condivisa dell'esecuzione (ADR-0002, Fase 2B M2c):
    /// sotto-directory `spill/` del `TempStore` — creata dal workspace di
    /// spill al primo uso, ripulita dei file a fine operazione e rimossa
    /// interamente dal `Drop` RAII dello store. Risolta in `new` (V2).
    fn spill_directory(&self) -> &Path {
        &self.spill_directory
    }

    /// Accumula le metriche di uno spill attivato in un nodo tabellare.
    fn add_spill_metrics(&self, delta: SpillMetrics) {
        let mut total = self.spill_metrics.borrow_mut();
        total.bytes_written = total.bytes_written.saturating_add(delta.bytes_written);
        total.bytes_read = total.bytes_read.saturating_add(delta.bytes_read);
        total.files = total.files.saturating_add(delta.files);
    }

    /// Un batch e' ricaduto dal runner fuso geo al percorso non fuso
    /// (ADR-0012 D12.7): contatore dedicato, mai silenzioso — nessun errore
    /// nuovo, il risultato resta identico.
    fn record_geo_fusion_fallback(&self) {
        self.metrics.borrow_mut().geo_fusion_fallbacks += 1;
    }

    /// Heartbeat del `TempStore` al punto centrale (ogni batch processato
    /// passa dal conteggio delle metriche), con throttle di
    /// [`HEARTBEAT_MIN_INTERVAL`]. Best-effort: un heartbeat fallito degrada
    /// un segnale diagnostico (ADR 3: mai una prova), non deve fermare
    /// l'esecuzione — il cleanup RAII resta la pulizia principale.
    fn heartbeat(&self) {
        if self.last_heartbeat.get().elapsed() < HEARTBEAT_MIN_INTERVAL {
            return;
        }
        self.last_heartbeat.set(Instant::now());
        let _ = self.temp_store.borrow_mut().heartbeat();
    }

    /// Errore di cancellazione attribuito a un punto del DAG (ADR 3):
    /// contesto (nodo, operazione, `execution_id`), mai dati.
    fn cancelled(&self, node: &str, operation: &str) -> PlenoraError {
        PlenoraError::Cancelled {
            node: node.to_owned(),
            operation: operation.to_owned(),
            execution_id: self.execution_id.clone(),
            reason: "cancellazione richiesta dal chiamante".to_owned(),
        }
    }

    /// Check di cancellazione al confine di un kernel (ADR 3, M1c): onora il
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
    /// sempre attivo, anche a valle di op `NonInterruptible` (ADR 3: nessuna
    /// nuova attivita' dopo la cancellazione, publish compreso).
    fn check_cancellation_point(&self, node: &str, operation: &str) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled(node, operation));
        }
        Ok(())
    }

    /// Tag M1d al confine di uscita: ogni errore `Execution` che lascia il DAG
    /// porta l'`execution_id` (riempito qui se il punto di origine, in
    /// profondita' nel dispatch, non lo aveva a disposizione).
    fn tag_execution(&self, error: PlenoraError) -> PlenoraError {
        match error {
            PlenoraError::Execution {
                node,
                operation,
                mut execution_id,
                reason,
            } => {
                if execution_id.is_empty() {
                    execution_id.clone_from(&self.execution_id);
                }
                PlenoraError::Execution {
                    node,
                    operation,
                    execution_id,
                    reason,
                }
            }
            other => other,
        }
    }

    /// Arricchimento diagnostico opt-in (ADR 3, M1d): con `diagnostics`
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
            PlenoraError::InvalidPlan(reason) => {
                PlenoraError::InvalidPlan(format!("{reason}{suffix}"))
            }
            other => other,
        }
    }
}

// ---------------------------------------------------------------------------
// Canale d'arco condiviso (fan-out tee, D9/V9/V10)
// ---------------------------------------------------------------------------

/// Errore di un arco conservato in forma scomposta per la riproduzione ai
/// consumatori successivi (`PlenoraError` non e' `Clone`): l'attribuzione
/// originale (`Execution`/`Cancelled` con nodo, operazione ed `execution_id`) e'
/// preservata, non declassata a `InvalidPlan`.
struct StoredEdgeError {
    node: Option<String>,
    operation: Option<String>,
    execution_id: Option<String>,
    cancelled: bool,
    reason: String,
}

impl StoredEdgeError {
    fn from_error(error: &PlenoraError) -> Self {
        match error {
            PlenoraError::Execution {
                node,
                operation,
                execution_id,
                reason,
            } => Self {
                node: Some(node.clone()),
                operation: Some(operation.clone()),
                execution_id: Some(execution_id.clone()),
                cancelled: false,
                reason: reason.clone(),
            },
            PlenoraError::Cancelled {
                node,
                operation,
                execution_id,
                reason,
            } => Self {
                node: Some(node.clone()),
                operation: Some(operation.clone()),
                execution_id: Some(execution_id.clone()),
                cancelled: true,
                reason: reason.clone(),
            },
            other => Self {
                node: None,
                operation: None,
                execution_id: None,
                cancelled: false,
                reason: other.to_string(),
            },
        }
    }

    fn to_error(&self) -> PlenoraError {
        match (&self.node, &self.operation) {
            (Some(node), Some(operation)) if self.cancelled => PlenoraError::Cancelled {
                node: node.clone(),
                operation: operation.clone(),
                execution_id: self.execution_id.clone().unwrap_or_default(),
                reason: self.reason.clone(),
            },
            (Some(node), Some(operation)) => PlenoraError::Execution {
                node: node.clone(),
                operation: operation.clone(),
                execution_id: self.execution_id.clone().unwrap_or_default(),
                reason: self.reason.clone(),
            },
            _ => PlenoraError::InvalidPlan(format!("arco interrotto: {}", self.reason)),
        }
    }
}

/// Stato di un arco: upstream lazy, buffer condiviso tra i consumatori e
/// cursore di lettura per ciascuno. Il buffer trattiene [`GovernedBatch`]:
/// il lease e' condiviso (clone `Arc`) tra i consumatori — la quota del
/// batch e' contata UNA volta all'ingresso dell'arco e torna al governor al
/// `Drop` dell'ultimo riferimento (ADR-0002).
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
    /// Rilascia i batch letti da tutti i consumatori (V10).
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
            return self
                .shared
                .error
                .borrow()
                .as_ref()
                .map(|stored| {
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
///   opzionale modellato dal contratto) implica `axis_order = unknown`: la
///   chiave e' obbligatoria quando un CRS e' presente e `unknown` e' il
///   valore canonico dichiarato dalla tabella §2 — `ResolvedCrs` non porta
///   l'ordine degli assi e dichiararlo sarebbe inventarlo (R5.2 riguarda le
///   chiavi opzionali, che restano assenti);
/// - R2.6: una chiave canonica gia' presente sul campo (o la versione sullo
///   schema) con valore DIVERSO da quello imposto dal contratto e' un
///   errore, mai una sovrascrittura silenziosa; valore uguale e'
///   idempotente. Le chiavi che l'operazione RISCRIVE di mestiere (ADR-0009,
///   decisione 8 — il blocco CRS per `reproject`, `types`/
///   `types_declaration` per le trasformazioni che cambiano il tipo
///   geometrico) non passano MAI di qui come divergenze: la sostituzione
///   avviene a monte, nel contratto prodotto dall'analisi
///   (`analyze_reproject` / `with_geometry_types` rimuovono le chiavi
///   ereditate), e qui sono ri-emesse dal contratto come ogni altra. Per
///   tutte le chiavi non riscritte il guard resta intatto. Eccezioni
///   dichiarate: `axis_order = unknown` non e' una
///   dichiarazione ma l'assenza di una (il `DataContract` non modella
///   l'ordine degli assi, ADR-0009) — una chiave `axis_order` gia'
///   presente e' informazione della lineage (R2.4/R2.7) e resta;
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
                    // `axis_order = unknown` non e' una dichiarazione ma
                    // l'ASSENZA di una (il DataContract non modella
                    // l'ordine degli assi, ADR-0009): una chiave gia'
                    // presente e' informazione della lineage (R2.4), non
                    // una divergenza — R2.7 completa solo l'assente, mai
                    // arbitra. Resta il valore dichiarato dall'ingresso.
                    if key == PLENORA_GEOMETRY_AXIS_ORDER_KEY && value == "unknown" {
                        continue;
                    }
                    // R4.6.4: un centro che ha rilevato un'incoerenza CRS la
                    // DICHIARA (`declared_unresolved`) invece di propagare la
                    // dichiarazione `resolved` del produttore, che
                    // l'incoerenza stessa smentisce — silenziarla e'
                    // vietato. E' l'unica sovrascrittura ammessa su una
                    // chiave canonica: una sola chiave, una sola direzione
                    // (`resolved` -> `declared_unresolved`), mai il
                    // contrario (ADR-0009, decisione 7). La direzione
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
/// batch-per-batch (V3). Non e' `Send` nella v1 seriale (V8).
pub struct Output {
    contract: DataContract,
    /// Schema IPC emesso: quello del contratto piu' il blocco canonico
    /// R2.2/R2.5 (milestone C), calcolato una sola volta alla costruzione
    /// ([`canonical_output_schema`], fail-fast su divergenze R2.6).
    schema: SchemaRef,
    stream: BatchStream,
    state: Rc<ExecState>,
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

    /// Identita' dell'esecuzione (ADR 3, M1d): la stessa riportata negli
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
        Ok((batches, self.state.metrics()))
    }

    /// Drena lo stream conservando i wrapper governati (lease + sequenza).
    ///
    /// Seam interno per i test M1a/M1b (ADR-0001/ADR-0002): in questa
    /// milestone nessun consumatore pubblico riordina per `BatchSequence`.
    #[cfg(test)]
    pub(crate) fn collect_governed(self) -> Result<(Vec<GovernedBatch>, ExecutionMetrics)> {
        let batches = self.stream.collect::<Result<Vec<_>>>()?;
        Ok((batches, self.state.metrics()))
    }

    /// Scrive l'output in Arrow IPC file format con publish atomico
    /// (decisione D22/ADR 7): tempfile nella directory di destinazione,
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
    /// destinazione e' di rete o non identificabile (ADR 7).
    pub fn write_ipc_file(self, path: &Path) -> Result<ExecutionMetrics> {
        let (metrics, _outcome) = self.write_ipc_file_with_profile(path, PublishProfile::Atomic)?;
        Ok(metrics)
    }

    /// Come [`Output::write_ipc_file`], ma con profilo di publish
    /// selezionabile (ADR 7) ed esito tipizzato restituito al chiamante:
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
        let mut stream = self.stream;
        let ((), outcome) = publish_with_profile(path, profile, move |writer| {
            let mut ipc = FileWriter::try_new(writer, &schema)?;
            // Cache della decisione di rivestimento per Arc di schema: i
            // batch di uno stream condividono lo stesso Arc (lo schema del
            // contratto del kernel), quindi il confronto profondo di
            // `Schema` (campi + mappe metadata) si esegue solo al primo
            // batch di ogni schema distinto (V2: lavoro hoistable fuori
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
                    RecordBatch::try_new(schema.clone(), batch.columns().to_vec())?
                } else {
                    batch
                };
                ipc.write(&batch)?;
            }
            ipc.finish()?;
            Ok(())
        })?;
        Ok((self.state.metrics(), outcome))
    }
}

impl Iterator for Output {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        self.stream
            .next()
            .map(|item| item.map(GovernedBatch::into_batch))
    }
}

// ---------------------------------------------------------------------------
// execute
// ---------------------------------------------------------------------------

/// Fase 2 `execute` (Architetture.md par. 6.3, ADR 5): accetta solo il
/// prodotto di [`crate::planner::validate`] (type-state), esegue
/// internamente `prepare` + `execute_physical`.
///
/// All'ingresso l'identita' ADR 4 del grafo e' riverificata contro l'ambiente
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
///   (fail-closed ADR 3, vedi l'header del modulo).
#[allow(clippy::needless_pass_by_value)] // Firma per valore voluta da ADR 5.
pub fn execute(
    graph: &ValidatedGraph,
    inputs: Inputs,
    runtime: RuntimeContext,
) -> Result<Output> {
    check_compatibility(graph, CATALOG, ENGINE_VERSION, ARROW_VERSION, &local_capabilities())?;
    let plan = Rc::new(prepare(graph, &runtime)?);
    execute_physical(&plan, graph, inputs, &runtime)
}

/// `execute_physical` (ADR 5, interno): verifica degli input contro i
/// contratti validati e costruzione della rete di stream.
fn execute_physical(
    plan: &Rc<ExecutionPlan>,
    graph: &ValidatedGraph,
    inputs: Inputs,
    runtime: &RuntimeContext,
) -> Result<Output> {
    let declared: Vec<&String> = graph.plan().plan().inputs.iter().collect();
    for name in declared.iter().map(|s| (*s).as_str()) {
        if !inputs.readers.contains_key(name) {
            return Err(PlenoraError::InvalidPlan(format!(
                "manca l'input `{name}`"
            )));
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

    // Schemi degli input contro i contratti validati (fail-closed, prima di
    // toccare i dati).
    for (name, input) in &inputs.readers {
        let contract = graph.edge_contract(name).ok_or_else(|| {
            PlenoraError::Internal(format!(
                "l'input `{name}` non ha un contratto nel grafo validato"
            ))
        })?;
        let provided = input.schema();
        if provided.fields() != contract.schema.fields() {
            return Err(PlenoraError::Schema(format!(
                "l'input `{name}` ha uno schema diverso dal contratto validato"
            )));
        }
    }

    // ADR 3, M1d: identita' dell'esecuzione — UUID v4 (dipendenza `uuid`
    // gia' pinnata nel workspace, nessuna versione nuova) con prefisso
    // leggibile; il charset rispetta la validazione restrittiva del
    // `TempStore` ([A-Za-z0-9._-]).
    let execution_id = format!("exec-{}", uuid::Uuid::new_v4().simple());
    // ADR 3: scavenging all'avvio delle directory temporanee orfane, sulla
    // radice dello store (default: temp di sistema; configurabile via
    // `RuntimeContext::temp_root`). Best-effort: un fallimento dello
    // scavenging non deve mai impedire un'esecuzione valida — le directory
    // orfane restano e saranno raccolte al giro successivo.
    let temp_root = runtime.temp_root.clone().unwrap_or_else(std::env::temp_dir);
    let _ = scavenge_stale_temp_dirs(&temp_root, DEFAULT_SCAVENGE_TTL);
    // Fail-closed, decisione documentata: niente degrado a tempdir semplice.
    // Il `TempStore` e' la difesa strutturale ADR 3 contro i crash non
    // intercettabili (lo spill di M2 ci scrivera'): eseguire senza
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
    // `Output`), cosi' la coda d'uscita resta dentro il perimetro ADR-0002.
    let output_state = Rc::clone(&state);
    let output_edge = plan.output_edge().to_owned();
    let mut output_counts = (0_u64, 0_u64);
    let stream = Box::new(stream.map(move |item| {
        // Tag M1d: ogni errore `Execution` che esce dal DAG porta l'execution_id.
        let governed = item.map_err(|error| output_state.tag_execution(error))?;
        // ADR 3, M1c: confine di piano — sempre attivo, anche a valle di op
        // `NonInterruptible` (nessuna nuova attivita' dopo la cancellazione:
        // consegnare/pubblicare e' nuova attivita').
        output_state.check_cancellation_point(&output_edge, "output")?;
        let batch = &governed.batch;
        let _ = check_batch_bytes(&output_state, batch, "output")?;
        let limits = &output_state.plan.limits();
        output_counts.0 += batch.num_rows() as u64;
        output_counts.1 += 1;
        if output_counts.0 > limits.rows.max_output_rows {
            return Err(PlenoraError::InvalidPlan(format!(
                "max_output_rows superato: {} righe di output > {}",
                output_counts.0, limits.rows.max_output_rows
            )));
        }
        if output_counts.1 > limits.max_batches {
            return Err(PlenoraError::InvalidPlan(format!(
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
    /// piu' consumatori (fan-out) sono condivisi via tee (D9/V9).
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
    /// lease di memoria (ADR-0002, quota contata UNA volta per arco) e la
    /// sequenza logica (ADR-0001: `source_node` = nome dell'input,
    /// `input_partition` = 0 — nessun ramo parallelo della sorgente in v1 —
    /// `sequence_number` = contatore seriale per input).
    ///
    /// Errori `Internal` sulle invarianti di costruzione della rete
    /// (reader/contratto dell'input presenti per il dispatch del chiamante,
    /// colonna geometria nello schema del contratto): il caso "impossibile"
    /// e' un errore esplicito, mai un panic (R6).
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
            Input::Batches(batches) => Box::new(
                batches
                    .into_iter()
                    .map(|batch| Ok(GovernedBatch::new(batch, None, None))),
            ),
            Input::Stream { iter, .. } => iter,
        };

        let state = Rc::clone(&self.state);
        let edge_name = edge.to_owned();
        let expected_schema = contract.schema.clone();
        let geometry_column = contract.active_geometry_column();
        let geometry_index = geometry_column
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
        // B1.3: la dimensionalita' attesa dal gate WKB e' quella del
        // contratto risolto dell'input (stessa fonte di `geometry_index`):
        // type code incoerente col contratto -> errore dedicato; `Unknown`
        // (R3.4) accetta e deriva lo stride dal type code di ogni cella.
        let geometry_dimensions =
            geometry_column.map_or(GeometryDimensions::Xy, |geometry| geometry.dimensions);
        let mut sequence_number = 0_u64;
        Ok(Box::new(raw.map(move |item| {
            // Confine di lettura (BLOCK-03): gli errori della sorgente e la
            // coerenza per-batch dello schema nascono leggendo l'input —
            // fase Read. Gli errori di governor e di validazione WKB qui
            // sotto restano validazione, derivata per variante.
            let batch = item.map_err(|error| error.with_phase(ErrorPhase::Read))?.batch;
            if batch.schema().as_ref() != expected_schema.as_ref() {
                return Err(PlenoraError::Schema(format!(
                    "batch dell'input `{edge_name}` con schema diverso dal contratto"
                ))
                .with_phase(ErrorPhase::Read));
            }
            let bytes = check_batch_bytes(&state, &batch, &edge_name)?;
            if let Some(index) = geometry_index {
                validate_wkb_cells(&state, &batch, index, &edge_name, geometry_dimensions)?;
            }
            {
                let mut counts = state.input_counts.borrow_mut();
                // Chiave clonata solo al primo batch dell'input (V2): i
                // batch successivi entrano dal `get_mut` sul nome esistente.
                if let Some(entry) = counts.get_mut(&edge_name) {
                    entry.0 += batch.num_rows() as u64;
                    entry.1 += 1;
                    entry.2 += bytes;
                } else {
                    let entry = counts.entry(edge_name.clone()).or_insert((0, 0, 0));
                    entry.0 += batch.num_rows() as u64;
                    entry.1 += 1;
                    entry.2 += bytes;
                }
                let entry = &counts[&edge_name];
                let limits = &state.plan.limits();
                if entry.0 > limits.rows.max_input_rows {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "max_input_rows superato sull'input `{edge_name}`: {} righe > {}",
                        entry.0, limits.rows.max_input_rows
                    )));
                }
                if entry.1 > limits.max_batches {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "max_batches superato sull'input `{edge_name}`: {} batch > {}",
                        entry.1, limits.max_batches
                    )));
                }
                if entry.2 > limits.max_payload_bytes {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "max_payload_bytes superato sull'input `{edge_name}`: {} byte > {}",
                        entry.2, limits.max_payload_bytes
                    )));
                }
            }
            // ADR-0002: reservation immediata (v1 seriale — regola in
            // `MemoryGovernor::try_reserve`); i limiti per input sopra sono
            // gia' passati, quindi qui il fallimento e' solo per budget
            // globale esaurito.
            let lease = state.governor.reserve(bytes, &edge_name)?;
            // ADR-0001: sequenza logica d'ingresso (contatore seriale).
            let seq = BatchSequence {
                source_node: edge_name.clone(),
                input_partition: 0,
                sequence_number,
            };
            sequence_number += 1;
            Ok(GovernedBatch::new(batch, Some(lease), Some(seq)))
        })))
    }

    /// Stream prodotto da un segmento, secondo la sua modalita' (E2).
    fn segment_stream(&mut self, index: usize) -> Result<BatchStream> {
        let (mode, input_edges) = {
            let segment = &self.plan.segments()[index];
            (segment.mode, segment.input_edges.to_vec())
        };
        match mode {
            SegmentMode::LinearStreaming | SegmentMode::GeoFused => {
                let input = self.edge_stream(&input_edges[0])?;
                let plan = Rc::clone(&self.plan);
                let state = Rc::clone(&self.state);
                Ok(Box::new(input.map(move |item| {
                    item.and_then(|governed| run_streaming_chain(&plan, index, &state, governed))
                })))
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
                    // ADR-0002 (Fase 2B M2c): verso un kernel spill-capable
                    // la quota governor dei batch drenati e' rilasciata
                    // subito. La soglia di attivazione dello spill ha la
                    // stessa grandezza del budget (byte stimati dell'input vs
                    // `max_memory_bytes`): trattenere i lease renderebbe lo
                    // spill irraggiungibile (fail-fast al drenaggio, prima
                    // del dispatch). La memoria di lavoro dell'operatore e'
                    // auto-limitata dallo spill su disco; approssimazione v1:
                    // la materializzazione dell'input resta in RAM (lo spill
                    // in streaming durante il drenaggio e' M3).
                    let spill_capable = spill_capable_unary(kernel);
                    // ADR 3, M1c: drenaggio dell'input — check a ogni
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
                    // ADR 3, M1c: come per il blocking unario — check a ogni
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

/// Tetto duro sui byte di un batch (V7: `max_batch_bytes`). Restituisce i
/// byte del batch (V2: un solo conteggio, riusato da lease e contatori).
fn check_batch_bytes(state: &ExecState, batch: &RecordBatch, where_: &str) -> Result<u64> {
    let bytes = batch.get_array_memory_size();
    let max = state.plan.batch_target().max_batch_bytes;
    if bytes > max {
        return Err(PlenoraError::InvalidPlan(format!(
            "max_batch_bytes superato su `{where_}`: {bytes} byte > {max}"
        )));
    }
    Ok(bytes as u64)
}

/// Validazione dinamica in lettura (D8): struttura WKB di ogni cella non
/// null, con i limiti effettivi del piano applicati prima del validatore
/// strutturale (`max_wkb_cell_bytes` per cella, `max_geometry_depth` per
/// l'annidamento; il tetto componenti resta quello del validatore).
///
/// B1.3: il validatore e' stride-aware — la dimensionalita' attesa
/// (`dimensions`, dal contratto risolto dell'arco) fissa lo stride delle
/// coordinate e ogni type code incoerente col contratto e' un errore
/// dedicato; con `Unknown` (R3.4) lo stride e' derivato dal type code di
/// ogni geometria.
fn validate_wkb_cells(
    state: &ExecState,
    batch: &RecordBatch,
    geometry_index: usize,
    edge: &str,
    dimensions: GeometryDimensions,
) -> Result<()> {
    let cells = batch_geometry_cells(batch, geometry_index, "geometry")?;
    let limits = state.plan.limits();
    let max_cell = limits.max_wkb_cell_bytes;
    let max_depth = limits.max_geometry_depth as usize;
    // Diagnostica opt-in (M1d): il nome della colonna e' contesto
    // strutturale, non un valore — risolto solo nel ramo d'errore (V2),
    // mai allocato sul percorso felice.
    let column_detail = || {
        format!(
            "colonna `{}`",
            batch.schema().field(geometry_index).name()
        )
    };
    for row in 0..batch.num_rows() {
        if cells.is_null(row) {
            continue;
        }
        let payload = cells.value(row);
        if payload.len() as u64 > max_cell {
            return Err(state.with_diagnostics(
                PlenoraError::InvalidPlan(format!(
                    "cella WKB oltre max_wkb_cell_bytes sull'arco `{edge}` (riga {row})"
                )),
                Some(&column_detail()),
            ));
        }
        validate_wkb_contract_for_dimensions_with_depth(payload, dimensions, max_depth)
            .map_err(|error| {
                PlenoraError::InvalidPlan(format!(
                    "WKB non valido sull'arco `{edge}` (riga {row}): {error}"
                ))
            })
            .map_err(|error| state.with_diagnostics(error, Some(&column_detail())))?;
    }
    Ok(())
}

/// Contatori e limiti dell'arco intermedio prodotto da un kernel
/// (`max_rows_per_edge`, `max_batches`, byte per batch).
fn check_edge_batch(state: &ExecState, edge: &str, batch: &RecordBatch) -> Result<()> {
    let _ = check_batch_bytes(state, batch, edge)?;
    check_edge_counts(state, edge, batch.num_rows() as u64)
}

/// Conteggi righe/batch di un arco intermedio e limiti corrispondenti
/// (`max_rows_per_edge`, `max_batches`) SENZA il tetto byte del batch:
/// archi interni dei gruppi fusi geo (ADR-0012 D12.8, deroga DER-003 di
/// `docs/deroghe.md` — il batch non e' materializzato e H-03 e' coperto dal
/// governor, reservation D12.7). Righe e batch restano esatti (1:1).
fn check_edge_counts(state: &ExecState, edge: &str, rows: u64) -> Result<()> {
    let mut counts = state.edge_counts.borrow_mut();
    // Chiave clonata solo al primo batch dell'arco (V2): i batch successivi
    // entrano dal `get_mut` sul nome esistente.
    if let Some(entry) = counts.get_mut(edge) {
        entry.0 += rows;
        entry.1 += 1;
    } else {
        let entry = counts.entry(edge.to_owned()).or_insert((0, 0));
        entry.0 += rows;
        entry.1 += 1;
    }
    let entry = &counts[edge];
    let limits = &state.plan.limits();
    if entry.0 > limits.rows.max_rows_per_edge {
        return Err(PlenoraError::InvalidPlan(format!(
            "max_rows_per_edge superato sull'arco `{edge}`: {} righe > {}",
            entry.0, limits.rows.max_rows_per_edge
        )));
    }
    if entry.1 > limits.max_batches {
        return Err(PlenoraError::InvalidPlan(format!(
            "max_batches superato sull'arco `{edge}`: {} batch > {}",
            entry.1, limits.max_batches
        )));
    }
    Ok(())
}

/// Esenzione da `max_expansion_factor` dichiarata in catalogo (ADR 6):
/// le op `WholeToMany` (`geo.generate_grid`, `geo.coverage_validate`,
/// `geo.shared_paths`) sono generative/diagnostiche — l'input funge da
/// trigger o da insieme da analizzare, non da base proporzionale
/// dell'output. Risolta in `prepare` (V2: nessuno scan del catalogo nel
/// loop per batch).
const fn expansion_exempt(kernel: &PreparedKernel) -> bool {
    kernel.expansion_factor_exempt
}

/// Comportamento alla cancellazione del kernel (ADR 3, M1c): dichiarato in
/// catalogo dal descriptor dell'operazione e risolto in `prepare` (V2:
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

/// Hook di test (ADR 3): override del `CancellationBehavior` di catalogo
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

/// Fattore di espansione per nodo unario (ADR 6: base = righe di input;
/// per i binari si veda [`check_join_expansion`]).
///
/// Per le op esenti (dichiarato in catalogo, [`expansion_exempt`]) il
/// controllo sul fattore output/input non si applica: la produzione resta
/// limitata da `max_rows_per_edge` / `max_output_rows` / `max_batches`.
#[allow(clippy::cast_precision_loss)] // Il fattore e' f64 per contratto (ADR 6); sotto 2^53 righe il confronto e' esatto.
fn check_expansion(state: &ExecState, kernel: &PreparedKernel, base_rows: u64) -> Result<()> {
    if expansion_exempt(kernel) {
        return Ok(());
    }
    let mut rows = state.node_rows.borrow_mut();
    // Chiave clonata solo al primo batch del nodo (V2): i batch successivi
    // entrano dal `get_mut` sull'id esistente.
    if let Some(entry) = rows.get_mut(&kernel.node_id) {
        entry.0 += base_rows;
    } else {
        rows.entry(kernel.node_id.clone()).or_insert((0, 0)).0 += base_rows;
    }
    let entry = &rows[&kernel.node_id];
    let factor = state.plan.limits().rows.max_expansion_factor;
    if (entry.1 as f64) > (entry.0 as f64) * factor {
        return Err(PlenoraError::InvalidPlan(format!(
            "max_expansion_factor superato al nodo `{}`: {} righe output > {} x {} righe input",
            kernel.node_id, entry.1, factor, entry.0
        )));
    }
    Ok(())
}

/// Fattore di espansione per nodo binario (ADR 6): calcola tutte le
/// metriche [`JoinExpansion`] e applica il vincolo vincolante dichiarato in
/// catalogo per l'operazione (default `SumRelative` se l'op non e' in
/// catalogo — non dovrebbe accadere: il piano e' validato sul catalogo).
/// La soglia e' `max_expansion_factor` dei limiti effettivi, tranne per il
/// vincolo `Custom(fattore)`: il fattore dichiarato in catalogo la
/// sovrascrive per la singola operazione (stima a priori, ADR 5/6).
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
    let constraint = descriptor.map_or(ExpansionConstraint::SumRelative, |d| d.expansion_constraint);
    let expansion = JoinExpansion::compute(output_rows, left_rows, right_rows);
    let binding = expansion.binding_metric(constraint);
    let factor = constraint.binding_threshold(state.plan.limits().rows.max_expansion_factor);
    if binding > factor {
        return Err(PlenoraError::InvalidPlan(format!(
            "max_expansion_factor superato al nodo `{}` (vincolo {constraint:?}): \
             metrica vincolante {binding} > {factor}; \
             output/(left+right)={}, output/left={}, output/right={} \
             (output={output_rows}, left={left_rows}, right={right_rows})",
            kernel.node_id,
            expansion.output_over_sum_inputs,
            expansion.output_over_left,
            expansion.output_over_right,
        )));
    }
    Ok(())
}

/// Errore di un kernel attribuito al nodo logico (E3), preservando la
/// diagnosi senza dati sensibili. L'`execution_id` e' vuoto qui (il dispatch
/// in profondita' non lo ha a disposizione): lo riempie il tag M1d al
/// confine di uscita (`ExecState::tag_execution`).
fn step_error(kernel: &PreparedKernel, error: PlenoraError) -> PlenoraError {
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

/// Conversione di un panic di kernel in errore di nodo (ADR 3): il payload
/// testuale (`&str`/`String`) diventa il motivo, mai dati dei batch (regola
/// di error.rs: contesto, non valori). Payload non testuale: motivo generico.
fn panic_step_error(kernel: &PreparedKernel, payload: &(dyn std::any::Any + Send)) -> PlenoraError {
    let message = payload
        .downcast_ref::<&'static str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "payload non testuale".to_owned());
    step_error(
        kernel,
        PlenoraError::InvalidPlan(format!("panic nel kernel: {message}")),
    )
}

/// Metriche di un'esecuzione di kernel (per nodo e per segmento, E3).
/// `first`/`last` indicano la posizione del kernel nel segmento (righe e
/// batch di ingresso contati solo sul primo, di uscita solo sull'ultimo).
/// I byte sono i metadati dei buffer Arrow ai confini del kernel
/// (E3/ADR-0002: il governor non riconta — questi conteggi sono metriche,
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
    // ADR 3: heartbeat del TempStore al punto centrale — ogni batch
    // processato passa di qui (throttled, vedi `ExecState::heartbeat`).
    state.heartbeat();
    let config = state.plan.metrics_config();
    let mut metrics = state.metrics.borrow_mut();
    // Metrica obbligatoria ADR 6: sempre aggiornata, indipendente dalla
    // configurazione per-nodo/per-segmento.
    metrics.total_rows_processed += rows_in;
    if config.per_node {
        if let Some(node) = metrics.nodes.get_mut(&kernel.node_id) {
            node.rows_in += rows_in;
            node.rows_out += rows_out;
            node.batches_in += 1;
            node.batches_out += 1;
            node.bytes_in += bytes_in;
            node.bytes_out += bytes_out;
            node.wall_time += elapsed;
        }
    }
    if config.per_segment {
        if let Some(seg) = metrics.segments.get_mut(&segment.id) {
            if first {
                seg.rows_in += rows_in;
                seg.batches_in += 1;
            }
            if last {
                seg.rows_out += rows_out;
                seg.batches_out += 1;
            }
            seg.wall_time += elapsed;
        }
    }
}

// ---------------------------------------------------------------------------
// Esecuzione dei kernel
// ---------------------------------------------------------------------------

/// Sequenza logica riassegnata all'output di un segmento blocking
/// (ADR-0001): la cardinalita' cambia (concatenazione + kernel una tantum),
/// quindi la sequenza degli input non e' propagabile 1:1.
///
/// Regola v1 — deterministica, per ordine di scansione seriale: il segmento
/// blocking emette UN batch per esecuzione (per i binari il contenuto
/// dipende dalla scansione left-then-right, anch'essa seriale), quindi la
/// sequenza e' sempre `source_node` = nodo del kernel, `input_partition` =
/// 0, `sequence_number` = 0. In M1 nessun consumatore riordina: la sequenza
/// e' osservabilita' e predisposizione per il collect indicizzato di M3.
fn blocking_output_sequence(kernel: &PreparedKernel) -> BatchSequence {
    BatchSequence {
        source_node: kernel.node_id.clone(),
        input_partition: 0,
        sequence_number: 0,
    }
}

/// Catena streaming (V4): il batch attraversa i kernel in sequenza senza
/// materializzazione; limiti per arco ed espansione dopo ogni kernel.
///
/// Confine ADR-0002/ADR-0001: il wrapper si spacca in ingresso (i kernel
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
) -> Result<GovernedBatch> {
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
    // Diagnostica opt-in (M1d): la sequenza logica e' contesto strutturale
    // (indice di batch), mai un valore. Formattata solo a flag attivo (V2):
    // `with_diagnostics` la ignorerebbe comunque a diagnostica spenta.
    let batch_detail = if state.diagnostics {
        seq.as_ref()
            .map(|seq| format!("batch_seq={}", seq.sequence_number))
    } else {
        None
    };
    let mut position = 0_usize;
    while position < kernels.len() {
        // ADR-0012: se il kernel apre un gruppo di fusione geo (>= 2 membri)
        // il gruppo e' eseguito col runner fuso su QUESTO batch; a
        // reservation governor fallita si ricade sul percorso non fuso per
        // il batch (D12.7, fallback strumentato) e il loop standard
        // processa i kernel uno a uno.
        let group_len = fusion_group_len(kernels, position);
        if group_len > 1
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
        // ADR 3, M1c: check cooperativo al confine di kernel — per il primo
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
        {
            let mut rows = state.node_rows.borrow_mut();
            if let Some(entry) = rows.get_mut(&kernel.node_id) {
                entry.1 += rows_out;
            } else {
                rows.entry(kernel.node_id.clone()).or_insert((0, 0)).1 += rows_out;
            }
        }
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
        position += 1;
    }
    // Ricomposizione: quota dell'output acquisita prima di rilasciare
    // l'input (mai sotto-conteggio al confine, ADR-0002).
    let output_lease = state
        .governor
        .reserve(bytes_at_boundary, &segment.output_edge)?;
    drop(input_lease);
    Ok(GovernedBatch::new(batch, Some(output_lease), seq))
}

/// Lunghezza del gruppo di fusione geo che si apre a `position` (0 se il
/// kernel non apre un gruppo, ADR-0012): i membri condividono l'id assegnato
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

/// Stima iniziale dei byte decodificati del gruppo sul batch (ADR-0012
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

/// Contesto di un tentativo fuso su un batch (ADR-0012): argomenti condivisi
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
    /// Contabilita' dei kernel COMPLETATI del gruppo (ADR-0012 D12.6):
    /// stessa sequenza del loop non fuso — righe per nodo, espansione,
    /// limiti d'arco, metriche per kernel — per i primi `completed` kernel.
    /// Sugli archi interni fusi il batch non e' materializzato: conteggi
    /// righe/batch esatti (1:1), niente tetto byte (D12.8, deroga DER-003);
    /// il batch materiale esiste solo a gruppo completato (ultimo kernel).
    /// I byte ai confini interni fusi sono zero: i buffer Arrow intermedi
    /// non esistono (metriche E3, non reservation).
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
            {
                let mut node_rows = self.state.node_rows.borrow_mut();
                if let Some(entry) = node_rows.get_mut(&kernel.node_id) {
                    entry.1 += self.rows;
                } else {
                    node_rows
                        .entry(kernel.node_id.clone())
                        .or_insert((0, 0))
                        .1 += self.rows;
                }
            }
            check_expansion(self.state, kernel, self.rows)?;
            match materialized {
                Some(batch) => {
                    if !(is_last && self.output_is_plan_output) {
                        check_edge_batch(self.state, &kernel.node_id, batch)?;
                    }
                }
                None => {
                    // Arco interno fuso (D12.8/DER-003): conteggi esatti 1:1,
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

/// Tentativo di esecuzione FUSA di un gruppo geo su un batch (ADR-0012):
/// reservation dei byte decodificati sul governor (D12.7) e, a concessione
/// avvenuta, runner fuso con un solo decode/encode. Restituisce `true` se il
/// gruppo e' stato eseguito (batch e byte al confine aggiornati); `false` se
/// si e' ricaduti sul percorso non fuso per QUESTO batch — reservation
/// fallita, metrica dedicata registrata, batch invariato: nessun errore
/// nuovo, il loop standard produce l'esito identico.
///
/// Un gruppo e' un run di trasformazioni `TransformInPlace` (>= 1) piu' UNA
/// misura terminale opzionale in coda (M2, capability `TerminalMeasure`):
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
    let lease = match state.governor.try_reserve(decoded_bytes, &kernels[0].node_id) {
        Ok(ReservationResult::Granted(lease)) => lease,
        // Reservation fallita -> fallback strumentato (D12.7). Gli esiti
        // `RetryAfterProgress`/`MustSpill` non sono mai emessi dalla v1
        // seriale: per difesa stesso fallback, mai una primitiva di panic.
        Ok(ReservationResult::RetryAfterProgress | ReservationResult::MustSpill) | Err(_) => {
            state.record_geo_fusion_fallback();
            return Ok(false);
        }
    };
    // Misura terminale (ADR-0012 M2): se l'ultimo membro del gruppo e' una
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
    // ADR-0012 M3: lo schema di output del gruppo e' quello dell'ULTIMA
    // trasformazione — con `reproject` nel gruppo il CRS del campo geometria
    // cambia a meta' catena. La ricostruzione canonica del campo dipende
    // solo da (nome, CRS di output) e gli altri campi passano invariati,
    // quindi l'handle risolto sullo schema del batch e' IDENTICO a quello
    // che il percorso non fuso risolverebbe sullo schema intermedio; per le
    // op M1/M2 (CRS invariato) coincide con l'handle del primo kernel.
    let last_transform = transforms.len() - 1;
    let output_prepared = state
        .one_to_one_prepared(&kernels[last_transform], &batch.schema(), params[last_transform])
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
        one_to_one_batch_fused(batch, &params, terminal, &prepared, &output_prepared, &mut |index| {
            current.set(index);
            if index > 0 {
                attempt.edges.borrow_mut().push(Instant::now());
            }
            let kernel = &kernels[index];
            #[cfg(test)]
            inject_test_panic(&kernel.node_id);
            state.check_cancellation(kernel)
        })
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
            let error = step_error(&kernels[index], PlenoraError::InvalidPlan(error.to_string()));
            Err(state.with_diagnostics(error, batch_detail))
        }
        Err(FusedStepError::Measure { index, error }) => {
            // Misura terminale (M2): l'errore e' gia' nella forma del
            // percorso non fuso (`geo_measure_batch` chiude il `PlenoraError`
            // grezzo con `step_error`, senza wrap in `InvalidPlan`).
            attempt.account(index, None, finished)?;
            let error = step_error(&kernels[index], error);
            Err(state.with_diagnostics(error, batch_detail))
        }
    }
}

/// Scomposizione di un gruppo fuso (ADR-0012 M2): il run di trasformazioni
/// e la misura terminale opzionale in coda (presente se l'ultimo membro e'
/// un `GeoMeasure` — invariante di `prepare`: la misura puo' solo chiudere
/// un gruppo, mai aprirlo o proseguirlo).
fn fused_group_terminal(kernels: &[PreparedKernel]) -> (&[PreparedKernel], Option<FusedTerminal<'_>>) {
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

/// Hook di test (ADR 3): id dei nodi in cui iniettare un panic, per
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

/// Un kernel su un batch: confine ADR 3 dell'executor. Un panic del kernel
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
fn run_kernel(kernel: &PreparedKernel, batch: RecordBatch, state: &ExecState) -> Result<RecordBatch> {
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
/// dell'esecuzione (ADR-0002, Fase 2B M2c): `sort`/`distinct`/`aggregate`
/// sopra la soglia di spill scrivono nel `TempStore` e le loro metriche sono
/// accumulate in [`ExecState`].
fn dispatch_kernel(kernel: &PreparedKernel, batch: RecordBatch, state: &ExecState) -> Result<RecordBatch> {
    match &kernel.config {
        PreparedConfig::TableUnary(plan) => {
            let (output, spill_metrics) =
                table_engine::execute_batch_with_spill(batch, plan, Some(state.spill_directory()))
                    .map_err(|error| step_error(kernel, error))?;
            state.add_spill_metrics(spill_metrics);
            Ok(output)
        }
        PreparedConfig::TableBinary(_) => Err(PlenoraError::Internal(format!(
            "nodo `{}`: kernel binario in una catena streaming",
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
/// envelope): i parametri sono tipizzati e risolti da `prepare` (E1);
/// indice di colonna e schema di output arrivano dall'handle prepared del
/// nodo, costruito una volta per esecuzione (V2).
fn geo_transform_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    params: &TransformArrowSchema,
    state: &ExecState,
) -> Result<RecordBatch> {
    let prepared = state.one_to_one_prepared(kernel, &batch.schema(), params)?;
    one_to_one_batch_prepared(batch, params, &prepared)
        .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))
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
    let column: ArrayRef = match measure {
        MeasureKind::Area | MeasureKind::Length | MeasureKind::Perimeter => {
            let mut values: Vec<Option<f64>> = Vec::with_capacity(batch.num_rows());
            for row in 0..batch.num_rows() {
                values.push(measure_f64(kernel, cells.value(row), measure, cells.is_null(row))?);
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
                let geometry = decode_geometry_cell(cells.value(row))
                    .map_err(|error| step_error(kernel, error))?;
                let value = operations::vertex_count(&geometry)
                    .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
                values.push(Some(value));
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
                let geometry = decode_geometry_cell(cells.value(row))
                    .map_err(|error| step_error(kernel, error))?;
                let value = operations::to_wkt(&geometry)
                    .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
                values.push(Some(value));
            }
            std::sync::Arc::new(StringArray::from(values))
        }
    };
    // Lo schema di output e' quello del contratto (input + colonna misura),
    // un clone di Arc condiviso: nessuna ricostruzione per batch (V1),
    // stesso percorso degli altri kernel add-column.
    append_output_column(kernel, batch, column)
}

/// Misura scalare su una cella (null-in → null-out).
fn measure_f64(
    kernel: &PreparedKernel,
    payload: &[u8],
    measure: MeasureKind,
    is_null: bool,
) -> Result<Option<f64>> {
    if is_null {
        return Ok(None);
    }
    let geometry = decode_geometry_cell(payload).map_err(|error| step_error(kernel, error))?;
    let value = match measure {
        MeasureKind::Area => operations::area(&geometry),
        MeasureKind::Length => operations::length(&geometry),
        MeasureKind::Perimeter => operations::perimeter(&geometry),
        MeasureKind::VertexCount | MeasureKind::ToWkt => {
            return Err(step_error(
                kernel,
                PlenoraError::Internal(
                    "misura non f64 nel percorso scalare f64: invariante di dispatch violata"
                        .into(),
                ),
            ));
        }
    }
    .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
    Ok(Some(value))
}

// ---------------------------------------------------------------------------
// Estensioni geo v1.1-v1.3 (dispatch dedicato sugli adapter Arrow dei kernel)
// ---------------------------------------------------------------------------

/// Celle WKB della colonna geometria attiva del batch (indice risolto in
/// `prepare`, V2), con errore attribuito al nodo logico.
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

/// Indice risolto della colonna geometria attiva (V2), con errore di nodo.
fn kernel_geometry_index(kernel: &PreparedKernel) -> Result<usize> {
    kernel.geometry_column_index.ok_or_else(|| {
        step_error(
            kernel,
            PlenoraError::Schema("op geo senza colonna geometria".into()),
        )
    })
}

/// Batch con una colonna aggiunta in coda: lo schema e' quello del contratto
/// di output inferito dal planner (fonte unica di verita', E1).
fn append_output_column(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    column: ArrayRef,
) -> Result<RecordBatch> {
    let mut columns = batch.columns().to_vec();
    columns.push(column);
    RecordBatch::try_new(kernel.output_contract.schema.clone(), columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
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
        .ok_or_else(|| {
            step_error(
                kernel,
                PlenoraError::Schema("colonna WKT non Utf8".into()),
            )
        })?;
    let cells = plenora_kernels_geo::extensions::from_wkt_column(values, on_error)
        .map_err(|error| step_error(kernel, error))?;
    let geometry = BinaryArray::from(
        cells
            .iter()
            .map(|cell| cell.as_deref())
            .collect::<Vec<_>>(),
    );
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
        let geometry = decode_geometry_cell(cells.value(row))
            .map_err(|error| step_error(kernel, error))?;
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
                    .map(|access| access.as_ref().and_then(|access| access.start_point.as_deref()))
                    .collect::<Vec<_>>(),
            )),
            AccessorKind::EndPoint => std::sync::Arc::new(StringArray::from(
                accessors
                    .iter()
                    .map(|access| access.as_ref().and_then(|access| access.end_point.as_deref()))
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
    RecordBatch::try_new(kernel.output_contract.schema.clone(), all_columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
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
        let geometry = decode_geometry_cell(cells.value(row))
            .map_err(|error| step_error(kernel, error))?;
        let fraction = plenora_kernels_geo::extensions::line_locate_point(&geometry, point)
            .map_err(|error| step_error(kernel, PlenoraError::InvalidPlan(error.to_string())))?;
        values.push(fraction);
    }
    append_output_column(kernel, batch, std::sync::Arc::new(Float64Array::from(values)))
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
        let pieces = plenora_kernels_geo::extensions2::subdivide_wkb(cells.value(row), max_vertices)
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
                parts
                    .iter()
                    .map(|part| part.as_deref())
                    .collect::<Vec<_>>(),
            )));
        } else {
            columns.push(
                take(column.as_ref(), &indices, None)
                    .map_err(|error| step_error(kernel, PlenoraError::from(error)))?,
            );
        }
    }
    columns.push(std::sync::Arc::new(indices));
    RecordBatch::try_new(kernel.output_contract.schema.clone(), columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
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
    RecordBatch::try_new(kernel.output_contract.schema.clone(), columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
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
        let group: Vec<Option<geo::Geometry<f64>>> = rows
            .iter()
            .map(|&row| geometries[row].clone())
            .collect();
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
    RecordBatch::try_new(kernel.output_contract.schema.clone(), columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
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
        rows.iter().map(|row| row.wkb.as_slice()).collect::<Vec<_>>(),
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
    RecordBatch::try_new(kernel.output_contract.schema.clone(), columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
}

/// `geo.coverage_validate` (blocking, WholeToMany): una riga per overlap.
fn geo_coverage_validate_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    tolerance: f64,
    max_issues: usize,
) -> Result<RecordBatch> {
    let cells = kernel_geometry_cells(kernel, batch)?;
    let rows = plenora_kernels_geo::extensions3::coverage_validate_rows(cells, tolerance, max_issues)
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
            rows.iter().map(|row| row.wkb.as_slice()).collect::<Vec<_>>(),
        )),
    ];
    RecordBatch::try_new(kernel.output_contract.schema.clone(), columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
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
            rows.iter().map(|row| row.wkb.as_slice()).collect::<Vec<_>>(),
        )),
    ];
    RecordBatch::try_new(kernel.output_contract.schema.clone(), columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
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
    append_output_column(kernel, batch, std::sync::Arc::new(UInt64Array::from(labels)))
}

/// Il kernel di un segmento blocking unario e' spill-capable (ADR-0002,
/// Fase 2B M2c): `table.sort`/`distinct`/`aggregate` hanno la variante
/// `*_spilled` in kernels-table (cfr. `table_engine::unary_spill_capable`).
fn spill_capable_unary(kernel: &PreparedKernel) -> bool {
    matches!(
        &kernel.config,
        PreparedConfig::TableUnary(table_plan) if table_engine::unary_spill_capable(table_plan)
    )
}

/// Segmento blocking unario: input materializzato (previsto dal piano, V9),
/// concatenato ed eseguito una sola volta.
///
/// Confine ADR-0002: i lease degli input sono trattenuti durante la
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
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().ok_or_else(|| {
        PlenoraError::Internal("segmento blocking senza kernel: invariante del planner violata".into())
    })?;
    let rows_in = batches.iter().map(|g| g.batch.num_rows()).sum::<usize>() as u64;
    let bytes_in = batches.iter().map(GovernedBatch::accounted_bytes).sum::<u64>();
    let schema = kernel.input_contracts[0].schema.clone();
    let full = if batches.is_empty() {
        RecordBatch::new_empty(schema)
    } else {
        let unwrapped: Vec<RecordBatch> = batches.iter().map(|g| g.batch.clone()).collect();
        concat_batches(&schema, &unwrapped)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    // Il batch concatenato non ha un produttore a monte che ne abbia
    // verificato i byte: il tetto duro V7 si applica anche qui (fail-closed).
    // I byte restituiti alimentano la reservation (V2: un solo conteggio).
    let full_bytes = check_batch_bytes(state, &full, &kernel.node_id)?;
    // ADR-0002 (Fase 2B M2c): se il kernel spillera' — stessa soglia
    // deterministica valutata al dispatch tabellare (`should_spill_unary`
    // sui byte stimati dell'input), stessi limiti — l'intermedio
    // concatenato NON consuma quota governor: la memoria di lavoro
    // dell'operatore e' auto-limitata dallo spill su disco e la
    // reservation fallirebbe per costruzione (la soglia ha la stessa
    // grandezza del budget). Altrimenti reservation completa
    // dell'intermedio prima di rilasciare i lease degli input (ADR-0002:
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
    // ADR 3, M1c: a fine drenaggio, prima del kernel monolitico
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
    {
        let mut rows = state.node_rows.borrow_mut();
        if let Some(entry) = rows.get_mut(&kernel.node_id) {
            entry.1 += rows_out;
        } else {
            rows.entry(kernel.node_id.clone()).or_insert((0, 0)).1 += rows_out;
        }
    }
    check_expansion(state, kernel, rows_in)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    let bytes_out = output_lease.bytes();
    record_kernel_metrics(state, segment, kernel, rows_in, rows_out, bytes_in, bytes_out, elapsed, true, true);
    Ok(GovernedBatch::new(
        output,
        Some(output_lease),
        Some(blocking_output_sequence(kernel)),
    ))
}

/// Segmento blocking binario: left e right materializzati, concatenati ed
/// eseguiti una sola volta via `execute_binary`.
///
/// Confine ADR-0002 come [`run_blocking`], con reservation multiple in
/// ORDINE GLOBALE FISSO — left prima di right — completa prima di iniziare
/// (mai attesa con reservation parziale; in v1 fail-fast non c'e' attesa,
/// ma l'ordine e' gia' quello richiesto al runtime parallelo M3 per evitare
/// deadlock). Sequenza riassegnata con la regola documentata in
/// [`blocking_output_sequence`] (scansione seriale left-then-right).
fn run_binary_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    left_batches: Vec<GovernedBatch>,
    right_batches: Vec<GovernedBatch>,
) -> Result<GovernedBatch> {
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().ok_or_else(|| {
        PlenoraError::Internal("segmento binario senza kernel: invariante del planner violata".into())
    })?;
    let PreparedConfig::TableBinary(binary_plan) = &kernel.config else {
        return Err(PlenoraError::Internal(format!(
            "nodo `{}`: config non binaria in un segmento BinaryBlocking",
            kernel.node_id
        )));
    };
    let left_rows = left_batches.iter().map(|g| g.batch.num_rows()).sum::<usize>() as u64;
    let right_rows = right_batches.iter().map(|g| g.batch.num_rows()).sum::<usize>() as u64;
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
    // Come per il blocking unario: tetto duro V7 sui batch concatenati; i
    // byte restituiti alimentano le reservation (V2: un solo conteggio).
    let left_bytes = check_batch_bytes(state, &left, &kernel.node_id)?;
    let right_bytes = check_batch_bytes(state, &right, &kernel.node_id)?;
    // Reservation complete degli intermedi in ordine globale fisso (left,
    // poi right), poi rilascio dei lease degli input.
    let left_lease = state.governor.reserve(left_bytes, &kernel.node_id)?;
    let right_lease = state.governor.reserve(right_bytes, &kernel.node_id)?;
    drop(left_batches);
    drop(right_batches);
    // ADR 3, M1c: a fine drenaggio, prima del kernel binario monolitico
    // (come `run_blocking`).
    state.check_cancellation(kernel)?;
    let start = Instant::now();
    // Confine ADR 3 come `run_kernel`: panic del kernel binario convertito
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
    {
        let mut rows = state.node_rows.borrow_mut();
        if let Some(entry) = rows.get_mut(&kernel.node_id) {
            entry.1 += rows_out;
        } else {
            rows.entry(kernel.node_id.clone()).or_insert((0, 0)).1 += rows_out;
        }
    }
    // ADR 6: per le operazioni binarie il runtime calcola tutte le metriche
    // di espansione e applica il vincolo vincolante dichiarato in catalogo.
    check_join_expansion(state, kernel, left_rows, right_rows, rows_out)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    // Metriche per nodo: righe in = left + right, batch in = quelli reali
    // drenati dai due rami.
    // ADR 3: heartbeat del TempStore al punto centrale (come
    // `record_kernel_metrics`, non riusata qui per i conteggi doppi input).
    state.heartbeat();
    let config = state.plan.metrics_config();
    let mut metrics = state.metrics.borrow_mut();
    // Metrica obbligatoria ADR 6 (come in `record_kernel_metrics`).
    metrics.total_rows_processed += left_rows + right_rows;
    if config.per_node {
        if let Some(node) = metrics.nodes.get_mut(&kernel.node_id) {
            node.rows_in += left_rows + right_rows;
            node.rows_out += rows_out;
            node.batches_in += batches_in;
            node.batches_out += 1;
            node.bytes_in += bytes_in;
            node.bytes_out += output_lease.bytes();
            node.wall_time += elapsed;
        }
    }
    if config.per_segment {
        if let Some(seg) = metrics.segments.get_mut(&segment.id) {
            seg.rows_in += left_rows + right_rows;
            seg.rows_out += rows_out;
            seg.batches_in += batches_in;
            seg.batches_out += 1;
            seg.wall_time += elapsed;
        }
    }
    Ok(GovernedBatch::new(
        output,
        Some(output_lease),
        Some(blocking_output_sequence(kernel)),
    ))
}

#[cfg(test)]
mod governor_tests;
#[cfg(test)]
mod tests;
