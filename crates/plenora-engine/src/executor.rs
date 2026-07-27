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
//! Esecuzione v1: **seriale** ovunque (`SerialFused`, V8 — parallelismo,
//! governor, spill e cancellazione sono Fase 2B). Per questo lo stream usa
//! `Rc`/`RefCell` e [`Output`] non e' `Send`: e' una scelta documentata,
//! non un limite nascosto.
//!
//! Struttura fisica:
//!
//! - ogni arco del DAG e' un canale condiviso ([`EdgeShared`]): un solo
//!   consumatore = pass-through puro; piu' consumatori (fan-out, D9/V9) =
//!   tee che condivide i `RecordBatch` immutabili senza copie di buffer e
//!   rilascia ciascun batch quando tutti i consumatori lo hanno letto
//!   (V10). In esecuzione seriale i consumatori drenano in sequenza, quindi
//!   il tee coincide con la materializzazione conservativa di D9;
//! - `LinearStreaming`/`GeoFused`: il batch attraversa la catena di kernel
//!   senza code ne' materializzazioni (V4). `GeoFused` nella v1 esegue le
//!   op geo 1:1 come `LinearStreaming` (la cache di decode e' Fase 2C, la
//!   struttura [`crate::prepare::GeoRole`] la predispone);
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
//! `PlenoraError::Step { node, operation, .. }` con il solo messaggio del
//! panic, mai dati dei batch (regola di error.rs). L'errore propaga come
//! qualunque altro: il publish atomico non e' raggiunto (nessun publish
//! dopo panic) e il cleanup (tempfile, buffer degli archi) avviene comunque
//! via `Drop`. I confini `UnwindSafe` dichiarati per il DAG parallelo
//! (worker, cancellazione globale, spill) restano Fase 2B.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::ipc::reader::{FileReader, StreamReader};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{Field, Schema, SchemaRef};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::arrow::select::take::take;
use plenora_core::catalog::{find_operation, ExpansionConstraint, JoinExpansion, CATALOG};
use plenora_core::contract::DataContract;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::arrow_adapter::{batch_geometry_cells, decode_geometry_cell};
use plenora_kernels_geo::{operations, validate_wkb_contract_with_depth};

use crate::geo_transport::publish::{publish_with_profile, PublishOutcome, PublishProfile};
use crate::geo_transport::transport::{transform_batches, TransformArrowSchema};
use crate::planner::{
    check_compatibility, local_capabilities, ValidatedGraph, ARROW_VERSION, ENGINE_VERSION,
};
use crate::prepare::{
    prepare, AccessorKind, ExecutionPlan, MeasureKind, PhysicalSegment, PreparedConfig,
    PreparedKernel, RuntimeContext, SegmentMode,
};
use crate::table_engine;

/// Stream di batch del grafo (seriale, thread-locale nella v1).
type BatchStream = Box<dyn Iterator<Item = Result<RecordBatch>>>;

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
    /// `PlenoraError::Contract` se il vettore e' vuoto (per input vuoti usare
    /// [`Input::empty`] con lo schema esplicito).
    pub fn from_batches(batches: Vec<RecordBatch>) -> Result<Self> {
        if batches.is_empty() {
            return Err(PlenoraError::Contract(
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
    #[must_use]
    pub fn from_iter<I>(schema: SchemaRef, iter: I) -> Self
    where
        I: Iterator<Item = Result<RecordBatch>> + 'static,
    {
        Self::Stream {
            schema,
            iter: Box::new(iter),
        }
    }

    /// Lettore Arrow IPC **file format** (lazy: i batch sono letti dal disco
    /// man mano che lo stream di output viene tirato).
    ///
    /// # Errors
    ///
    /// `PlenoraError::Io`/`PlenoraError::Arrow` se il file non si apre o
    /// l'header IPC non e' valido.
    pub fn read_ipc_file(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let reader = FileReader::try_new(file, None)?;
        let schema = reader.schema();
        Ok(Self::Stream {
            schema,
            iter: Box::new(reader.map(|batch| batch.map_err(PlenoraError::from))),
        })
    }

    /// Lettore Arrow IPC **stream format** (lazy).
    ///
    /// # Errors
    ///
    /// Come [`Input::read_ipc_file`].
    pub fn read_ipc_stream(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let reader = StreamReader::try_new(file, None)?;
        let schema = reader.schema();
        Ok(Self::Stream {
            schema,
            iter: Box::new(reader.map(|batch| batch.map_err(PlenoraError::from))),
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
    /// `PlenoraError::Contract` se il nome e' gia' presente.
    pub fn add(&mut self, name: impl Into<String>, input: Input) -> Result<()> {
        let name = name.into();
        if self.readers.insert(name.clone(), input).is_some() {
            return Err(PlenoraError::Contract(format!(
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
}

// ---------------------------------------------------------------------------
// Stato condiviso dell'esecuzione (seriale, thread-locale)
// ---------------------------------------------------------------------------

/// Stato mutabile condiviso tra le chiusure dello stream (contatori per i
/// limiti effettivi e metriche). `Rc`/`RefCell`: esecuzione seriale v1 (V8).
struct ExecState {
    plan: Rc<ExecutionPlan>,
    metrics: RefCell<ExecutionMetrics>,
    /// Righe/batch/byte cumulati per input (`max_input_rows`, `max_batches`,
    /// `max_payload_bytes`).
    input_counts: RefCell<HashMap<String, (u64, u64, u64)>>,
    /// Righe/batch cumulati per arco intermedio (`max_rows_per_edge`).
    edge_counts: RefCell<HashMap<String, (u64, u64)>>,
    /// Righe in/out cumulate per nodo (`max_expansion_factor`).
    node_rows: RefCell<HashMap<String, (u64, u64)>>,
}

impl ExecState {
    fn new(plan: &Rc<ExecutionPlan>) -> Rc<Self> {
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
        Rc::new(Self {
            plan: Rc::clone(plan),
            metrics: RefCell::new(metrics),
            input_counts: RefCell::new(HashMap::new()),
            edge_counts: RefCell::new(HashMap::new()),
            node_rows: RefCell::new(HashMap::new()),
        })
    }

    /// Snapshot delle metriche correnti.
    fn metrics(&self) -> ExecutionMetrics {
        self.metrics.borrow().clone()
    }
}

// ---------------------------------------------------------------------------
// Canale d'arco condiviso (fan-out tee, D9/V9/V10)
// ---------------------------------------------------------------------------

/// Errore di un arco conservato in forma scomposta per la riproduzione ai
/// consumatori successivi (`PlenoraError` non e' `Clone`): l'attribuzione
/// `Step{nodo, operazione}` originale e' preservata, non declassata a
/// `Contract`.
struct StoredEdgeError {
    node: Option<String>,
    operation: Option<String>,
    reason: String,
}

impl StoredEdgeError {
    fn from_error(error: &PlenoraError) -> Self {
        match error {
            PlenoraError::Step {
                node,
                operation,
                reason,
            } => Self {
                node: Some(node.clone()),
                operation: Some(operation.clone()),
                reason: reason.clone(),
            },
            other => Self {
                node: None,
                operation: None,
                reason: other.to_string(),
            },
        }
    }

    fn to_error(&self) -> PlenoraError {
        match (&self.node, &self.operation) {
            (Some(node), Some(operation)) => PlenoraError::Step {
                node: node.clone(),
                operation: operation.clone(),
                reason: self.reason.clone(),
            },
            _ => PlenoraError::Contract(format!("arco interrotto: {}", self.reason)),
        }
    }
}

/// Stato di un arco: upstream lazy, buffer condiviso tra i consumatori e
/// cursore di lettura per ciascuno.
struct EdgeShared {
    upstream: RefCell<Option<BatchStream>>,
    buffer: RefCell<Vec<RecordBatch>>,
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
    type Item = Result<RecordBatch>;

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

/// Output di un'esecuzione: stream lazy dei batch finali + metriche.
///
/// Iterare l'`Output` guida l'esecuzione: l'input e' consumato
/// batch-per-batch (V3). Non e' `Send` nella v1 seriale (V8).
pub struct Output {
    contract: DataContract,
    stream: BatchStream,
    state: Rc<ExecState>,
}

impl Output {
    /// Schema Arrow dell'output (dal contratto inferito in validazione).
    #[must_use]
    pub fn schema(&self) -> SchemaRef {
        self.contract.schema.clone()
    }

    /// Contratto dell'arco di output del piano.
    #[must_use]
    pub const fn output_contract(&self) -> &DataContract {
        &self.contract
    }

    /// Snapshot delle metriche correnti (parziali finche' lo stream non e'
    /// esaurito).
    #[must_use]
    pub fn metrics(&self) -> ExecutionMetrics {
        self.state.metrics()
    }

    /// Drena lo stream raccogliendo tutti i batch finali.
    ///
    /// # Errors
    ///
    /// Propaga il primo errore dello stream (nessun output parziale viene
    /// restituito).
    pub fn collect_batches(self) -> Result<(Vec<RecordBatch>, ExecutionMetrics)> {
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
    /// # Errors
    ///
    /// Propaga errori di stream e di I/O; `PlenoraError::Contract` se la
    /// destinazione esiste gia' o la directory non esiste;
    /// `PlenoraError::UnsupportedPublishTarget` se il filesystem di
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
        let schema = self.contract.schema.clone();
        let mut stream = self.stream;
        let ((), outcome) = publish_with_profile(path, profile, move |writer| {
            let mut ipc = FileWriter::try_new(writer, &schema)?;
            for item in &mut stream {
                ipc.write(&item?)?;
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
        self.stream.next()
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
/// - `PlenoraError::Contract`: `GRAPH_MISMATCH` sull'identita' del grafo,
///   input mancanti/extra/duplicati, op fuori dal dispatch v1 (da `prepare`);
/// - `PlenoraError::Schema`: schema di un input diverso dal contratto
///   validato.
#[allow(clippy::needless_pass_by_value)] // Firma per valore voluta da ADR 5.
pub fn execute(
    graph: &ValidatedGraph,
    inputs: Inputs,
    runtime: RuntimeContext,
) -> Result<Output> {
    check_compatibility(graph, CATALOG, ENGINE_VERSION, ARROW_VERSION, &local_capabilities())?;
    let plan = Rc::new(prepare(graph, &runtime)?);
    execute_physical(&plan, graph, inputs)
}

/// `execute_physical` (ADR 5, interno): verifica degli input contro i
/// contratti validati e costruzione della rete di stream.
fn execute_physical(
    plan: &Rc<ExecutionPlan>,
    graph: &ValidatedGraph,
    inputs: Inputs,
) -> Result<Output> {
    let declared: Vec<&String> = graph.plan().plan().inputs.iter().collect();
    for name in declared.iter().map(|s| (*s).as_str()) {
        if !inputs.readers.contains_key(name) {
            return Err(PlenoraError::Contract(format!(
                "manca l'input `{name}`"
            )));
        }
    }
    if let Some(extra) = inputs
        .readers
        .keys()
        .find(|name| !declared.iter().any(|d| d.as_str() == name.as_str()))
    {
        return Err(PlenoraError::Contract(format!(
            "input `{extra}` non dichiarato nel piano"
        )));
    }

    // Schemi degli input contro i contratti validati (fail-closed, prima di
    // toccare i dati).
    for (name, input) in &inputs.readers {
        let contract = graph
            .edge_contract(name)
            .expect("input dichiarato ha un contratto");
        let provided = input.schema();
        if provided.fields() != contract.schema.fields() {
            return Err(PlenoraError::Schema(format!(
                "l'input `{name}` ha uno schema diverso dal contratto validato"
            )));
        }
    }

    let state = ExecState::new(plan);
    let mut input_contracts = BTreeMap::new();
    for name in &graph.plan().plan().inputs {
        input_contracts.insert(
            name.clone(),
            graph.edge_contract(name).expect("input dichiarato").clone(),
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
    // metriche di pubblicazione.
    let output_state = Rc::clone(&state);
    let mut output_counts = (0_u64, 0_u64);
    let stream = Box::new(stream.map(move |item| {
        let batch = item?;
        check_batch_bytes(&output_state, &batch, "output")?;
        let limits = &output_state.plan.limits();
        output_counts.0 += batch.num_rows() as u64;
        output_counts.1 += 1;
        if output_counts.0 > limits.rows.max_output_rows {
            return Err(PlenoraError::Contract(format!(
                "max_output_rows superato: {} righe di output > {}",
                output_counts.0, limits.rows.max_output_rows
            )));
        }
        if output_counts.1 > limits.max_batches {
            return Err(PlenoraError::Contract(format!(
                "max_batches superato sull'output: {} batch > {}",
                output_counts.1, limits.max_batches
            )));
        }
        let mut metrics = output_state.metrics.borrow_mut();
        metrics.output_rows = output_counts.0;
        metrics.output_batches = output_counts.1;
        Ok(batch)
    })) as BatchStream;

    let contract = graph.output_contract().clone();
    Ok(Output {
        contract,
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
            self.input_stream(edge)
        } else {
            let index = self.plan.segment_of(edge).ok_or_else(|| {
                PlenoraError::Contract(format!("arco `{edge}` senza produttore"))
            })?;
            self.segment_stream(index)?
        };
        let shared = EdgeShared::new(upstream);
        self.edges.insert(edge.to_owned(), Rc::clone(&shared));
        Ok(shared.register_reader())
    }

    /// Stream di un input del piano: limiti per input, byte per batch e
    /// validazione dinamica WKB per cella (D8) prima del primo nodo.
    fn input_stream(&mut self, edge: &str) -> BatchStream {
        let input = self
            .inputs
            .remove(edge)
            .expect("presenza verificata dal chiamante");
        let contract = self
            .input_contracts
            .get(edge)
            .expect("contratto dell'input")
            .clone();
        let raw: BatchStream = match input {
            Input::Batches(batches) => Box::new(batches.into_iter().map(Ok)),
            Input::Stream { iter, .. } => iter,
        };

        let state = Rc::clone(&self.state);
        let edge_name = edge.to_owned();
        let expected_schema = contract.schema.clone();
        let geometry_index = contract
            .active_geometry_column()
            .map(|geometry| {
                contract
                    .schema
                    .column_with_name(&geometry.name)
                    .expect("colonna geometria nel contratto")
                    .0
            });
        Box::new(raw.map(move |item| {
            let batch = item?;
            if batch.schema().as_ref() != expected_schema.as_ref() {
                return Err(PlenoraError::Schema(format!(
                    "batch dell'input `{edge_name}` con schema diverso dal contratto"
                )));
            }
            check_batch_bytes(&state, &batch, &edge_name)?;
            if let Some(index) = geometry_index {
                validate_wkb_cells(&state, &batch, index, &edge_name)?;
            }
            let mut counts = state.input_counts.borrow_mut();
            let entry = counts.entry(edge_name.clone()).or_insert((0, 0, 0));
            entry.0 += batch.num_rows() as u64;
            entry.1 += 1;
            entry.2 += batch.get_array_memory_size() as u64;
            let limits = &state.plan.limits();
            if entry.0 > limits.rows.max_input_rows {
                return Err(PlenoraError::Contract(format!(
                    "max_input_rows superato sull'input `{edge_name}`: {} righe > {}",
                    entry.0, limits.rows.max_input_rows
                )));
            }
            if entry.1 > limits.max_batches {
                return Err(PlenoraError::Contract(format!(
                    "max_batches superato sull'input `{edge_name}`: {} batch > {}",
                    entry.1, limits.max_batches
                )));
            }
            if entry.2 > limits.max_payload_bytes {
                return Err(PlenoraError::Contract(format!(
                    "max_payload_bytes superato sull'input `{edge_name}`: {} byte > {}",
                    entry.2, limits.max_payload_bytes
                )));
            }
            Ok(batch)
        }))
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
                    item.and_then(|batch| run_streaming_chain(&plan, index, &state, batch))
                })))
            }
            SegmentMode::Blocking => {
                let mut input = self.edge_stream(&input_edges[0])?;
                let plan = Rc::clone(&self.plan);
                let state = Rc::clone(&self.state);
                let mut once = Some(move || {
                    let batches = (&mut input).collect::<Result<Vec<_>>>()?;
                    run_blocking(&plan, index, &state, &batches)
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
                    let left_batches = (&mut left).collect::<Result<Vec<_>>>()?;
                    let right_batches = (&mut right).collect::<Result<Vec<_>>>()?;
                    run_binary_blocking(&plan, index, &state, &left_batches, &right_batches)
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

/// Tetto duro sui byte di un batch (V7: `max_batch_bytes`).
fn check_batch_bytes(state: &ExecState, batch: &RecordBatch, where_: &str) -> Result<()> {
    let bytes = batch.get_array_memory_size();
    let max = state.plan.batch_target().max_batch_bytes;
    if bytes > max {
        return Err(PlenoraError::Contract(format!(
            "max_batch_bytes superato su `{where_}`: {bytes} byte > {max}"
        )));
    }
    Ok(())
}

/// Validazione dinamica in lettura (D8): struttura WKB di ogni cella non
/// null, con i limiti effettivi del piano applicati prima del validatore
/// strutturale (`max_wkb_cell_bytes` per cella, `max_geometry_depth` per
/// l'annidamento; il tetto componenti resta quello del validatore).
fn validate_wkb_cells(
    state: &ExecState,
    batch: &RecordBatch,
    geometry_index: usize,
    edge: &str,
) -> Result<()> {
    let cells = batch_geometry_cells(batch, geometry_index, "geometry")?;
    let limits = state.plan.limits();
    let max_cell = limits.max_wkb_cell_bytes;
    let max_depth = limits.max_geometry_depth as usize;
    for row in 0..batch.num_rows() {
        if cells.is_null(row) {
            continue;
        }
        let payload = cells.value(row);
        if payload.len() as u64 > max_cell {
            return Err(PlenoraError::Contract(format!(
                "cella WKB oltre max_wkb_cell_bytes sull'arco `{edge}` (riga {row})"
            )));
        }
        validate_wkb_contract_with_depth(payload, max_depth).map_err(|error| {
            PlenoraError::Contract(format!(
                "WKB non valido sull'arco `{edge}` (riga {row}): {error}"
            ))
        })?;
    }
    Ok(())
}

/// Contatori e limiti dell'arco intermedio prodotto da un kernel
/// (`max_rows_per_edge`, `max_batches`, byte per batch).
fn check_edge_batch(state: &ExecState, edge: &str, batch: &RecordBatch) -> Result<()> {
    check_batch_bytes(state, batch, edge)?;
    let mut counts = state.edge_counts.borrow_mut();
    let entry = counts.entry(edge.to_owned()).or_insert((0, 0));
    entry.0 += batch.num_rows() as u64;
    entry.1 += 1;
    let limits = &state.plan.limits();
    if entry.0 > limits.rows.max_rows_per_edge {
        return Err(PlenoraError::Contract(format!(
            "max_rows_per_edge superato sull'arco `{edge}`: {} righe > {}",
            entry.0, limits.rows.max_rows_per_edge
        )));
    }
    if entry.1 > limits.max_batches {
        return Err(PlenoraError::Contract(format!(
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
/// dell'output.
fn expansion_exempt(kernel: &PreparedKernel) -> bool {
    find_operation(kernel.operation)
        .is_some_and(|descriptor| descriptor.expansion_factor_exempt)
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
    let entry = rows.entry(kernel.node_id.clone()).or_insert((0, 0));
    entry.0 += base_rows;
    let factor = state.plan.limits().rows.max_expansion_factor;
    if (entry.1 as f64) > (entry.0 as f64) * factor {
        return Err(PlenoraError::Contract(format!(
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
        return Err(PlenoraError::Contract(format!(
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
/// diagnosi senza dati sensibili.
fn step_error(kernel: &PreparedKernel, error: PlenoraError) -> PlenoraError {
    let reason = match error {
        PlenoraError::Step { reason, .. } => reason,
        other => other.to_string(),
    };
    PlenoraError::Step {
        node: kernel.node_id.clone(),
        operation: kernel.operation.to_owned(),
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
        PlenoraError::Contract(format!("panic nel kernel: {message}")),
    )
}

/// Metriche di un'esecuzione di kernel (per nodo e per segmento, E3).
/// `first`/`last` indicano la posizione del kernel nel segmento (righe e
/// batch di ingresso contati solo sul primo, di uscita solo sull'ultimo).
#[allow(clippy::too_many_arguments)]
fn record_kernel_metrics(
    state: &ExecState,
    segment: &PhysicalSegment,
    kernel: &PreparedKernel,
    rows_in: u64,
    rows_out: u64,
    elapsed: Duration,
    first: bool,
    last: bool,
) {
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

/// Catena streaming (V4): il batch attraversa i kernel in sequenza senza
/// materializzazione; limiti per arco ed espansione dopo ogni kernel.
fn run_streaming_chain(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    batch: RecordBatch,
) -> Result<RecordBatch> {
    let segment = &plan.segments()[segment_index];
    let output_is_plan_output = segment.output_edge == plan.output_edge();
    let mut batch = batch;
    let kernels = &segment.kernels;
    for (position, kernel) in kernels.iter().enumerate() {
        let rows_in = batch.num_rows() as u64;
        let start = Instant::now();
        batch = run_kernel(kernel, batch)?;
        let elapsed = start.elapsed();
        let rows_out = batch.num_rows() as u64;
        {
            let mut rows = state.node_rows.borrow_mut();
            rows.entry(kernel.node_id.clone()).or_insert((0, 0)).1 += rows_out;
        }
        check_expansion(state, kernel, rows_in)?;
        // Limiti d'arco sugli archi interni e sull'arco di uscita del
        // segmento, a meno che non sia l'output del piano (li valgono
        // max_output_rows e il wrapper di output).
        let is_last = position + 1 == kernels.len();
        if !(is_last && output_is_plan_output) {
            check_edge_batch(state, &kernel.node_id, &batch)?;
        }
        record_kernel_metrics(
            state,
            segment,
            kernel,
            rows_in,
            rows_out,
            elapsed,
            position == 0,
            is_last,
        );
    }
    Ok(batch)
}

/// Hook di test (ADR 3): id dei nodi in cui iniettare un panic, per
/// verificare la conversione panic → errore `Step` al confine dell'executor.
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
/// di nodo — e convertito in errore `Step` con il solo messaggio del panic
/// ([`panic_step_error`]); l'errore propaga nello stream, quindi il publish
/// atomico non e' mai raggiunto dopo un panic.
///
/// `AssertUnwindSafe` e' legittimo in questo punto: l'esecuzione v1 e'
/// seriale, batch e config sono proprieta' esclusiva della chiamata (nessuno
/// stato condiviso mutabile attraversa il confine) e l'errore ferma lo
/// stream, quindi un eventuale stato interno del kernel lasciato incoerente
/// dal panic non e' mai riusato. I confini `UnwindSafe` dichiarati per il
/// DAG parallelo restano Fase 2B.
fn run_kernel(kernel: &PreparedKernel, batch: RecordBatch) -> Result<RecordBatch> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        inject_test_panic(&kernel.node_id);
        dispatch_kernel(kernel, batch)
    })) {
        Ok(result) => result,
        Err(payload) => Err(panic_step_error(kernel, &*payload)),
    }
}

/// Dispatch per famiglia di un kernel su un batch.
fn dispatch_kernel(kernel: &PreparedKernel, batch: RecordBatch) -> Result<RecordBatch> {
    match &kernel.config {
        PreparedConfig::TableUnary(plan) => {
            table_engine::execute_batch(batch, plan).map_err(|error| step_error(kernel, error))
        }
        PreparedConfig::TableBinary(_) => Err(PlenoraError::Contract(format!(
            "nodo `{}`: kernel binario in una catena streaming (errore interno)",
            kernel.node_id
        ))),
        PreparedConfig::GeoTransform(params) => geo_transform_batch(kernel, &batch, params),
        PreparedConfig::GeoMeasure {
            measure,
            output_column,
        } => geo_measure_batch(kernel, &batch, *measure, output_column),
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
/// envelope): i parametri sono tipizzati e risolti da `prepare` (E1).
fn geo_transform_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    params: &TransformArrowSchema,
) -> Result<RecordBatch> {
    let schema = batch.schema();
    let (_, mut out) = transform_batches(&schema, std::slice::from_ref(batch), params)
        .map_err(|error| step_error(kernel, PlenoraError::Contract(error.to_string())))?;
    if out.len() != 1 {
        return Err(step_error(
            kernel,
            PlenoraError::Contract(format!(
                "trasformazione 1:1 ha prodotto {} batch (errore interno)",
                out.len()
            )),
        ));
    }
    Ok(out.remove(0))
}

/// Misura geo "add column" (semantica v4): decodifica le celle WKB non null,
/// applica il kernel scalare e aggiunge la colonna in coda allo schema (il
/// nome e' quello inferito dal planner, risolto in `prepare`).
fn geo_measure_batch(
    kernel: &PreparedKernel,
    batch: &RecordBatch,
    measure: MeasureKind,
    output_column: &str,
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
                    .map_err(|error| step_error(kernel, PlenoraError::Contract(error.to_string())))?;
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
                    .map_err(|error| step_error(kernel, PlenoraError::Contract(error.to_string())))?;
                values.push(Some(value));
            }
            std::sync::Arc::new(StringArray::from(values))
        }
    };
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    fields.push(Field::new(output_column, measure.data_type(), true));
    let schema = Schema::new_with_metadata(fields, batch.schema().metadata().clone());
    let mut columns = batch.columns().to_vec();
    columns.push(column);
    RecordBatch::try_new(std::sync::Arc::new(schema), columns)
        .map_err(|error| step_error(kernel, PlenoraError::from(error)))
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
        MeasureKind::VertexCount | MeasureKind::ToWkt => unreachable!("misura non f64"),
    }
    .map_err(|error| step_error(kernel, PlenoraError::Contract(error.to_string())))?;
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
            .map_err(|error| step_error(kernel, PlenoraError::Contract(error.to_string())))?;
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
            .map_err(|error| step_error(kernel, PlenoraError::Contract(error.to_string())))?;
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
            .map_err(|error| step_error(kernel, PlenoraError::Contract(error.to_string())))?;
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
/// UInt64 nullable per riga (noise → null).
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

/// Segmento blocking unario: input materializzato (previsto dal piano, V9),
/// concatenato ed eseguito una sola volta.
fn run_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    batches: &[RecordBatch],
) -> Result<RecordBatch> {
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().expect("segmento blocking: 1 kernel");
    let rows_in = batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64;
    let schema = kernel.input_contracts[0].schema.clone();
    let full = if batches.is_empty() {
        RecordBatch::new_empty(schema)
    } else {
        concat_batches(&schema, batches).map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    // Il batch concatenato non ha un produttore a monte che ne abbia
    // verificato i byte: il tetto duro V7 si applica anche qui (fail-closed).
    check_batch_bytes(state, &full, &kernel.node_id)?;
    let start = Instant::now();
    let output = run_kernel(kernel, full)?;
    let elapsed = start.elapsed();
    let rows_out = output.num_rows() as u64;
    {
        let mut rows = state.node_rows.borrow_mut();
        rows.entry(kernel.node_id.clone()).or_insert((0, 0)).1 += rows_out;
    }
    check_expansion(state, kernel, rows_in)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    record_kernel_metrics(state, segment, kernel, rows_in, rows_out, elapsed, true, true);
    Ok(output)
}

/// Segmento blocking binario: left e right materializzati, concatenati ed
/// eseguiti una sola volta via `execute_binary`.
fn run_binary_blocking(
    plan: &Rc<ExecutionPlan>,
    segment_index: usize,
    state: &ExecState,
    left_batches: &[RecordBatch],
    right_batches: &[RecordBatch],
) -> Result<RecordBatch> {
    let segment = &plan.segments()[segment_index];
    let kernel = segment.kernels.first().expect("segmento binario: 1 kernel");
    let PreparedConfig::TableBinary(binary_plan) = &kernel.config else {
        return Err(PlenoraError::Contract(format!(
            "nodo `{}`: config non binaria in un segmento BinaryBlocking (errore interno)",
            kernel.node_id
        )));
    };
    let left_rows = left_batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64;
    let right_rows = right_batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64;
    let batches_in = (left_batches.len() + right_batches.len()) as u64;
    let left_schema = kernel.input_contracts[0].schema.clone();
    let right_schema = kernel.input_contracts[1].schema.clone();
    let left = if left_batches.is_empty() {
        RecordBatch::new_empty(left_schema)
    } else {
        concat_batches(&left_schema, left_batches)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    let right = if right_batches.is_empty() {
        RecordBatch::new_empty(right_schema)
    } else {
        concat_batches(&right_schema, right_batches)
            .map_err(|error| step_error(kernel, PlenoraError::from(error)))?
    };
    // Come per il blocking unario: tetto duro V7 sui batch concatenati.
    check_batch_bytes(state, &left, &kernel.node_id)?;
    check_batch_bytes(state, &right, &kernel.node_id)?;
    let start = Instant::now();
    // Confine ADR 3 come `run_kernel`: panic del kernel binario convertito
    // in errore `Step` attribuito al nodo, mai publish dopo panic.
    let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        inject_test_panic(&kernel.node_id);
        table_engine::execute_binary(&left, &right, binary_plan)
    }))
    .unwrap_or_else(|payload| Err(panic_step_error(kernel, &*payload)))
    .map_err(|error| step_error(kernel, error))?;
    let elapsed = start.elapsed();
    let rows_out = output.num_rows() as u64;
    {
        let mut rows = state.node_rows.borrow_mut();
        rows.entry(kernel.node_id.clone()).or_insert((0, 0)).1 += rows_out;
    }
    // ADR 6: per le operazioni binarie il runtime calcola tutte le metriche
    // di espansione e applica il vincolo vincolante dichiarato in catalogo.
    check_join_expansion(state, kernel, left_rows, right_rows, rows_out)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    // Metriche per nodo: righe in = left + right, batch in = quelli reali
    // drenati dai due rami.
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
    Ok(output)
}

#[cfg(test)]
mod tests;
