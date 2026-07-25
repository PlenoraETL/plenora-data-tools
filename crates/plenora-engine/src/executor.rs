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
//!   colonna, il trasporto legacy la sostituirebbe);
//! - validazione dinamica in lettura (D8): WKB strutturale per cella sugli
//!   input con geometria, tramite
//!   [`plenora_kernels_geo::validate_wkb_contract`], prima che i dati
//!   raggiungano il primo nodo;
//! - limiti effettivi del piano: `max_input_rows` per input,
//!   `max_rows_per_edge` per arco intermedio, `max_output_rows`,
//!   `max_expansion_factor` per nodo (base: input per gli unari,
//!   left+right per i binari), `max_batches` per arco, `max_wkb_cell_bytes`
//!   per cella, `max_batch_bytes` per batch (V7, tetto duro);
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

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use plenora_core::arrow::array::{
    Array, ArrayRef, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::ipc::reader::{FileReader, StreamReader};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{Field, Schema, SchemaRef};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::contract::DataContract;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::arrow_adapter::{batch_geometry_cells, decode_geometry_cell};
use plenora_kernels_geo::{operations, validate_wkb_contract};

use crate::geo_transport::publish::publish_atomic;
use crate::geo_transport::transport::{transform_batches, TransformArrowSchema};
use crate::planner::ValidatedGraph;
use crate::prepare::{
    prepare, ExecutionPlan, MeasureKind, PhysicalSegment, PreparedConfig, PreparedKernel,
    RuntimeContext, SegmentMode,
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
}

// ---------------------------------------------------------------------------
// Stato condiviso dell'esecuzione (seriale, thread-locale)
// ---------------------------------------------------------------------------

/// Stato mutabile condiviso tra le chiusure dello stream (contatori per i
/// limiti effettivi e metriche). `Rc`/`RefCell`: esecuzione seriale v1 (V8).
struct ExecState {
    plan: Rc<ExecutionPlan>,
    metrics: RefCell<ExecutionMetrics>,
    /// Righe/batch cumulati per input (`max_input_rows`, `max_batches`).
    input_counts: RefCell<HashMap<String, (u64, u64)>>,
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

/// Stato di un arco: upstream lazy, buffer condiviso tra i consumatori e
/// cursore di lettura per ciascuno.
struct EdgeShared {
    upstream: RefCell<Option<BatchStream>>,
    buffer: RefCell<Vec<RecordBatch>>,
    reads: RefCell<Vec<usize>>,
    done: Cell<bool>,
    /// Messaggio dell'errore upstream (riprodotto ai consumatori successivi:
    /// `PlenoraError` non e' `Clone`).
    error: RefCell<Option<String>>,
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
        }
    }
}

/// Handle di lettura di un consumatore su un arco condiviso.
struct EdgeStream {
    shared: Rc<EdgeShared>,
    id: usize,
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
        // 2. Upstream esaurito (o in errore).
        if self.shared.done.get() {
            return self
                .shared
                .error
                .borrow()
                .as_ref()
                .map(|message| Err(PlenoraError::Contract(format!("arco interrotto: {message}"))));
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
                *self.shared.error.borrow_mut() = Some(error.to_string());
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
    /// output parziale e' mai visibile.
    ///
    /// # Errors
    ///
    /// Propaga errori di stream e di I/O; `PlenoraError::Contract` se la
    /// destinazione esiste gia' o la directory non esiste.
    pub fn write_ipc_file(self, path: &Path) -> Result<ExecutionMetrics> {
        let schema = self.contract.schema.clone();
        let mut stream = self.stream;
        publish_atomic(path, move |writer| {
            let mut ipc = FileWriter::try_new(writer, &schema)?;
            for item in &mut stream {
                ipc.write(&item?)?;
            }
            ipc.finish()?;
            Ok(())
        })?;
        Ok(self.state.metrics())
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
/// I nomi e gli schemi degli input sono verificati contro i contratti
/// validati prima di costruire lo stream (fail-closed); l'esecuzione vera e
/// propria resta lazy: parte alla prima pull dell'[`Output`].
///
/// # Errors
///
/// - `PlenoraError::Contract`: input mancanti/extra/duplicati, op fuori dal
///   dispatch v1 (da `prepare`);
/// - `PlenoraError::Schema`: schema di un input diverso dal contratto
///   validato.
#[allow(clippy::needless_pass_by_value)] // Firma per valore voluta da ADR 5.
pub fn execute(
    graph: &ValidatedGraph,
    inputs: Inputs,
    runtime: RuntimeContext,
) -> Result<Output> {
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
            let entry = counts.entry(edge_name.clone()).or_insert((0, 0));
            entry.0 += batch.num_rows() as u64;
            entry.1 += 1;
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
/// null, con il limite per cella dei limiti effettivi applicato prima del
/// validatore strutturale (64 MiB, 100k componenti, profondita' 64).
fn validate_wkb_cells(
    state: &ExecState,
    batch: &RecordBatch,
    geometry_index: usize,
    edge: &str,
) -> Result<()> {
    let cells = batch_geometry_cells(batch, geometry_index, "geometry")?;
    let max_cell = state.plan.limits().max_wkb_cell_bytes;
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
        validate_wkb_contract(payload).map_err(|error| {
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

/// Fattore di espansione per nodo (ADR 6: base input per gli unari,
/// left+right per i binari).
#[allow(clippy::cast_precision_loss)] // Il fattore e' f64 per contratto (ADR 6); sotto 2^53 righe il confronto e' esatto.
fn check_expansion(state: &ExecState, kernel: &PreparedKernel, base_rows: u64) -> Result<()> {
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

/// Un kernel streaming su un batch (dispatch per famiglia).
fn run_kernel(kernel: &PreparedKernel, batch: RecordBatch) -> Result<RecordBatch> {
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
        concat_batches(&schema, batches)?
    };
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
    let left_schema = kernel.input_contracts[0].schema.clone();
    let right_schema = kernel.input_contracts[1].schema.clone();
    let left = if left_batches.is_empty() {
        RecordBatch::new_empty(left_schema)
    } else {
        concat_batches(&left_schema, left_batches)?
    };
    let right = if right_batches.is_empty() {
        RecordBatch::new_empty(right_schema)
    } else {
        concat_batches(&right_schema, right_batches)?
    };
    let start = Instant::now();
    let output = table_engine::execute_binary(&left, &right, binary_plan)
        .map_err(|error| step_error(kernel, error))?;
    let elapsed = start.elapsed();
    let rows_out = output.num_rows() as u64;
    {
        let mut rows = state.node_rows.borrow_mut();
        rows.entry(kernel.node_id.clone()).or_insert((0, 0)).1 += rows_out;
    }
    // ADR 6: per le operazioni binarie la base dell'espansione e' left+right.
    check_expansion(state, kernel, left_rows + right_rows)?;
    if segment.output_edge != plan.output_edge() {
        check_edge_batch(state, &kernel.node_id, &output)?;
    }
    // Metriche per nodo: righe in = left + right.
    let config = state.plan.metrics_config();
    let mut metrics = state.metrics.borrow_mut();
    if config.per_node {
        if let Some(node) = metrics.nodes.get_mut(&kernel.node_id) {
            node.rows_in += left_rows + right_rows;
            node.rows_out += rows_out;
            node.batches_in += 2;
            node.batches_out += 1;
            node.wall_time += elapsed;
        }
    }
    if config.per_segment {
        if let Some(seg) = metrics.segments.get_mut(&segment.id) {
            seg.rows_in += left_rows + right_rows;
            seg.rows_out += rows_out;
            seg.batches_in += 2;
            seg.batches_out += 1;
            seg.wall_time += elapsed;
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
