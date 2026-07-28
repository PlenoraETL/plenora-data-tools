//! Preparer del DAG — `prepare(&ValidatedGraph, &RuntimeContext) ->
//! ExecutionPlan` (Architetture.md par. 6.3, ADR 5; Prestazioni.md V2, E1-E3)
//! — Fase 2A-4.
//!
//! Il [`ValidatedGraph`] contiene solo decisioni semantiche stabili; qui si
//! prendono le decisioni fisiche **per questa esecuzione**:
//!
//! - scomposizione del DAG in [`PhysicalSegment`] con [`SegmentMode`]
//!   esplicita: catene massimali di nodi `Streaming` fusi in un unico
//!   segmento (`LinearStreaming`, oppure `GeoFused` se tutti i nodi sono
//!   geo — nella v1 eseguito come `LinearStreaming`, ma la struttura per
//!   kernel [`GeoRole`] predispone la cache di decode di Fase 2C, vincolo
//!   V6); ogni nodo `Blocking`/`BinaryBlocking` e' un segmento a se';
//! - [`PreparedKernel`] per ogni nodo: configurazione deserializzata,
//!   tipizzata e gia' rivalidata, indici di colonna e CRS risolti — niente
//!   JSON ne' ricerche per nome nel loop di esecuzione (E1/V2);
//! - last consumer di ogni arco (V10) e punti di materializzazione espliciti
//!   (`materialize_output`, V9: fan-out, decisione D9);
//! - configurazione delle metriche (E3: per nodo logico anche dentro ai
//!   segmenti fusi, e per segmento).
//!
//! Statistiche di runtime (ADR 5): [`RuntimeStatistic::Unknown`] e' il
//! default e impone scelte conservative. Nella v1 seriale le statistiche
//! `Known`/`Estimated` non cambiano ancora nessuna decisione fisica (il
//! parallelismo adattivo e' Fase 2B): sono validate, propagate nel piano per
//! osservabilita' e pronte per le scelte migliorative future.
//!
//! Limitazioni v1 (fail-closed in `prepare`, mai a meta' esecuzione): il
//! dispatch copre le trasformazioni geo 1:1 in place, le misure "add
//! column" e le estensioni geo v1.1-v1.3 (`from_wkt`,
//! `geometry_accessors`, `collect`, `line_locate_point`, `generate_grid`,
//! `subdivide`, `snap`, `coverage_validate`, `shared_paths`,
//! `cluster_dbscan`); le altre op geo — es. `geo.dissolve`, `geo.explode`,
//! predicati, distanze, op binarie geo — e le op tabellari N-arie con piu'
//! di due input sono rifiutate con `PlenoraError::Unsupported`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use geo::{Geometry, Point};
use serde::Deserialize;

use plenora_core::arrow::schema::DataType;
use plenora_core::catalog::{Arity, ExecutionClass, Family};
use plenora_core::contract::{DataContract, RuntimeStatistic};
use plenora_core::limits::Limits;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::extensions::OnWktError;
use plenora_kernels_geo::extensions2::{GridExtent, GridShape};

use crate::cancellation::CancellationToken;
use crate::geo_transport::transport::{
    ArrowOperation, BufferCap, SimplifyPolicyParam, TransformArrowSchema,
};
use crate::plan::NodeV4;
use crate::planner::ValidatedGraph;
use crate::table_engine;

/// Dimensione di batch obiettivo e tetto duro (Prestazioni.md V7).
///
/// La v1 non ri-pacchettizza i batch in lettura (conservativo): il target
/// e' consultivo per le scelte fisiche future, `max_batch_bytes` e' un
/// limite duro verificato dall'executor su ogni batch che scorre nel piano.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchTarget {
    /// Obiettivo consultivo di byte per batch (V7).
    pub target_batch_bytes: usize,
    /// Tetto duro di byte per batch: un batch piu' grande e' un errore.
    pub max_batch_bytes: usize,
}

impl Default for BatchTarget {
    fn default() -> Self {
        Self {
            target_batch_bytes: 8 * 1024 * 1024,
            max_batch_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Statistiche di runtime di un input (ADR 5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputStatistics {
    /// Righe totali dell'input (es. da header Arrow IPC file format).
    pub rows: RuntimeStatistic<u64>,
    /// Numero di batch dell'input.
    pub batches: RuntimeStatistic<u64>,
}

/// Contesto runtime di una singola esecuzione (Architetture.md par. 6.3).
///
/// Non contiene nulla di semantico: uno stesso `ValidatedGraph` con due
/// `RuntimeContext` diversi produce due `ExecutionPlan` diversi, ed e' il
/// comportamento voluto (ADR 5). Fanno eccezione `cancellation`,
/// `diagnostics` e `temp_root` (ADR 3, M1c/M1d): non sono decisioni fisiche
/// e NON entrano nell'`ExecutionPlan` — li consuma direttamente `execute`.
#[derive(Clone, Debug)]
pub struct RuntimeContext {
    /// Statistiche per nome di input; gli input assenti valgono
    /// [`InputStatistics::default`] (tutto `Unknown` → conservativo).
    pub statistics: BTreeMap<String, InputStatistics>,
    /// Grado massimo di parallelismo offerto dall'ambiente. La v1 esegue
    /// sempre seriale (`SerialFused`, V8): il valore e' registrato nel piano
    /// e sara' usato dalle strategie parallele di Fase 2B.
    pub max_parallelism: u32,
    /// Dimensionamento dei batch (V7).
    pub batch_target: BatchTarget,
    /// Metriche da raccogliere (E3).
    pub metrics: MetricsConfig,
    /// Token di cancellazione cooperativa (ADR 3, M1c): il default non e'
    /// mai cancellato. Il chiamante (es. l'handler Ctrl-C della CLI) trattiene
    /// un clone del token e lo cancella dall'esterno; l'executor lo osserva
    /// ai confini cooperativi onorando il `CancellationBehavior` di catalogo.
    pub cancellation: CancellationToken,
    /// Modalita' diagnostica opt-in (ADR 3, M1d), solo per input fidati:
    /// gli errori includono contesto strutturale aggiuntivo (indice di
    /// batch, riga, colonna dove disponibile) — MAI valori. Default `false`:
    /// messaggi invariati (retrocompatibile).
    pub diagnostics: bool,
    /// Radice del `TempStore` dell'esecuzione e dello scavenging all'avvio
    /// (ADR 3): `None` = temp di sistema. Configurabile per i test e per
    /// ambienti con una temp dedicata.
    pub temp_root: Option<PathBuf>,
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self {
            statistics: BTreeMap::new(),
            max_parallelism: 1,
            batch_target: BatchTarget::default(),
            metrics: MetricsConfig::default(),
            cancellation: CancellationToken::new(),
            diagnostics: false,
            temp_root: None,
        }
    }
}

impl RuntimeContext {
    /// Statistiche di un input (default conservativo se non dichiarate).
    #[must_use]
    pub fn input_statistics(&self, input: &str) -> InputStatistics {
        self.statistics.get(input).copied().unwrap_or_default()
    }
}

/// Quali metriche raccogliere durante l'esecuzione (E3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricsConfig {
    /// Metriche per nodo logico (righe in/out, batch, wall time) — restano
    /// per nodo anche quando piu' nodi sono fusi in un segmento (E3).
    pub per_node: bool,
    /// Metriche per segmento fisico.
    pub per_segment: bool,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            per_node: true,
            per_segment: true,
        }
    }
}

/// Modalita' fisica di un segmento (Prestazioni.md E2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentMode {
    /// Catena di kernel streaming (almeno uno tabellare): batch-per-batch,
    /// senza materializzazione intermedia (V3/V4).
    LinearStreaming,
    /// Catena di sole op geo 1:1. Nella v1 e' eseguita come
    /// `LinearStreaming`; il modo distinto predispone la cache di decode WKB
    /// per segmento di Fase 2C (V6: al massimo un decode/encode per
    /// segmento fuso).
    GeoFused,
    /// Nodo blocking unario: materializza l'input ed esegue una sola volta.
    Blocking,
    /// Nodo blocking binario: materializza left e right ed esegue una sola
    /// volta.
    BinaryBlocking,
}

/// Strategia di parallelismo scelta dal piano (Prestazioni.md V8).
///
/// La v1 sceglie sempre `SerialFused` per i segmenti streaming e
/// `BlockingSingleTask` per quelli blocking: il parallelismo si attiva solo
/// con benefici misurati (Fase 2B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParallelismStrategy {
    /// Segmento eseguito serialmente, kernel fusi sul singolo batch (V4).
    SerialFused,
    /// Parallelismo per batch (Fase 2B).
    ParallelPerBatch,
    /// Parallelismo per ramo del DAG (Fase 2B).
    ParallelPerBranch,
    /// Operazione blocking come task singolo.
    BlockingSingleTask,
}

/// Ruolo di un kernel geo dentro a un segmento `GeoFused` (V6/E1).
///
/// E' il punto di aggancio della cache di decode di Fase 2C: i kernel
/// `TransformInPlace` possono condividere le geometrie decodificate lungo la
/// catena; nella v1 ognuno decodifica/encoda via `transform_batches`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeoRole {
    /// Trasformazione 1:1 che sostituisce la geometria in place (schema e
    /// `FieldId` invariati): buffer, simplify, centroid, reproject, snap, ...
    TransformInPlace,
    /// Misura che aggiunge una colonna scalare (`area`, `length`,
    /// `perimeter`, `vertex_count`, `to_wkt`): semantica v4 "add column",
    /// diversa dal trasporto legacy (che sostituisce la colonna geometria).
    /// Include gli accessori per riga (`geometry_accessors`,
    /// `line_locate_point`) e l'etichetta di `cluster_dbscan`.
    MeasureAddColumn,
    /// Produzione della colonna geometria da una colonna testuale WKT
    /// (`from_wkt`): l'input non ha geometria, l'output ne guadagna una.
    ProduceFromText,
    /// Espansione 1:N allineata alle righe (`subdivide`): una riga di input
    /// produce una o piu' righe, con `__parent_index` di lineage.
    OneToMany,
    /// Op che consuma l'intero input materializzato (segmento `Blocking`):
    /// `collect`, `generate_grid`, `coverage_validate`, `shared_paths`.
    WholeTable,
}

/// Misura geo v1 con semantica "aggiungi colonna" (Fase 2A-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureKind {
    /// `geo.area` → colonna `Float64`.
    Area,
    /// `geo.length` → colonna `Float64`.
    Length,
    /// `geo.perimeter` → colonna `Float64`.
    Perimeter,
    /// `geo.vertex_count` → colonna `UInt64`.
    VertexCount,
    /// `geo.to_wkt` → colonna `Utf8`.
    ToWkt,
}

impl MeasureKind {
    /// Tipo Arrow della colonna prodotta.
    #[must_use]
    pub const fn data_type(self) -> DataType {
        match self {
            Self::Area | Self::Length | Self::Perimeter => DataType::Float64,
            Self::VertexCount => DataType::UInt64,
            Self::ToWkt => DataType::Utf8,
        }
    }
}

/// Accessore scalare di `geo.geometry_accessors`, in ordine canonico di
/// output (lo stesso di `plenora_kernels_geo::analyze::ACCESSOR_COLUMNS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessorKind {
    /// Nome OGC del tipo (`Utf8`).
    GeometryType,
    /// Parti della geometria (`UInt64`).
    NumGeometries,
    /// Anelli interni di Polygon/MultiPolygon (`UInt64`).
    NumInteriorRings,
    /// WKT del punto iniziale di una linea aperta (`Utf8`, nullable).
    StartPoint,
    /// WKT del punto finale di una linea aperta (`Utf8`, nullable).
    EndPoint,
    /// Linea chiusa / tipo poligonale (`Boolean`).
    IsClosed,
}

impl AccessorKind {
    /// Tutti gli accessori in ordine canonico di output.
    pub const ALL: [Self; 6] = [
        Self::GeometryType,
        Self::NumGeometries,
        Self::NumInteriorRings,
        Self::StartPoint,
        Self::EndPoint,
        Self::IsClosed,
    ];

    /// Nome canonico della colonna (senza `output_prefix`).
    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::GeometryType => "geometry_type",
            Self::NumGeometries => "num_geometries",
            Self::NumInteriorRings => "num_interior_rings",
            Self::StartPoint => "start_point",
            Self::EndPoint => "end_point",
            Self::IsClosed => "is_closed",
        }
    }

    /// Accessore dal nome canonico (difesa in profondita': la selezione e'
    /// gia' validata in analisi).
    fn from_canonical_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.canonical_name() == name)
    }
}

/// Configurazione preparata di un kernel (E1): tipizzata, rivalidata in
/// `prepare`, senza JSON nel percorso per batch.
#[derive(Debug)]
pub enum PreparedConfig {
    /// Op tabellare unaria: piano legacy di un solo step gia' validato
    /// (config deserializzata e controllata da `ValidatedPlan`); eseguito
    /// per batch da `table_engine::execute_batch`.
    TableUnary(Box<table_engine::ValidatedPlan>),
    /// Op tabellare binaria (o `table.concat` a due input): piano legacy di
    /// un solo step binario, eseguito una sola volta da
    /// `table_engine::execute_binary` su input materializzati.
    TableBinary(Box<table_engine::ValidatedPlan>),
    /// Trasformazione geo 1:1 in place: parametri tipizzati di
    /// [`TransformArrowSchema`] con CRS e colonna geometria risolti,
    /// `validate_parameters` gia' chiamato. Eseguita per batch da
    /// `geo_transport::transport::transform_batches`.
    GeoTransform(Box<TransformArrowSchema>),
    /// Misura geo con semantica v4 "add column" (il trasporto legacy
    /// sostituirebbe la colonna geometria): dispatch dedicato dell'executor
    /// sui kernel `plenora_kernels_geo::operations`.
    GeoMeasure {
        /// Misura da applicare cella per cella.
        measure: MeasureKind,
        /// Nome della colonna prodotta (derivato dal contratto di output).
        output_column: String,
    },
    /// `geo.from_wkt` (streaming 1:1): colonna WKT `Utf8` → nuova colonna
    /// geometria WKB; indice della colonna WKT e politica d'errore risolti
    /// qui. Eseguito per batch su `extensions::from_wkt_column`.
    GeoFromWkt {
        /// Indice risolto della colonna WKT nel batch di input (V2).
        wkt_column_index: usize,
        /// Politica sulle celle WKT invalide (default `null`).
        on_error: OnWktError,
    },
    /// `geo.geometry_accessors` (streaming 1:1): colonne accessorie scelte,
    /// con i nomi di output risolti dal contratto (prefisso applicato).
    /// Eseguito per batch su `extensions::geometry_accessors`.
    GeoAccessors {
        /// Colonne prodotte: (nome di output, accessore) in ordine canonico.
        columns: Box<[(String, AccessorKind)]>,
    },
    /// `geo.line_locate_point` (streaming 1:1 "add column"): punto di
    /// riferimento decodificato una volta qui (E1), frazione per riga.
    GeoLineLocatePoint {
        /// Punto di riferimento (da `point_wkb` hex, convenzione D16).
        point: Point<f64>,
        /// Nome della colonna prodotta (derivato dal contratto di output).
        output_column: String,
    },
    /// `geo.subdivide` (streaming OneToMany): espansione 1:N per batch con
    /// `__parent_index` di lineage, come `explode`.
    GeoSubdivide {
        /// Soglia di vertici per parte (rivalidato >= 4).
        max_vertices: usize,
    },
    /// `geo.snap` (streaming 1:1 in place): riferimento decodificato una
    /// volta qui (E1), tolleranza rivalidata.
    GeoSnap {
        /// Geometria di riferimento (da `reference_wkb` hex, D16).
        reference: Geometry<f64>,
        /// Distanza massima di aggancio (finita, non negativa).
        tolerance: f64,
    },
    /// `geo.collect` (blocking, ManyToOne): raggruppamento per chiavi
    /// (responsabilita' dell'engine, come `dissolve`) e collezione per
    /// gruppo via `extensions::collect_geometries`.
    GeoCollect {
        /// Indici risolti delle colonne chiave nel batch di input (V2).
        group_by_indices: Box<[usize]>,
    },
    /// `geo.generate_grid` (blocking, `WholeToMany`, generativa): l'input
    /// funge da trigger; la griglia e' prodotta da
    /// `extensions2::generate_grid_rows` con parametri rivalidati.
    GeoGenerateGrid {
        /// Extent della griglia (finito, non degenere).
        extent: GridExtent,
        /// Lato cella (finito, > 0).
        cell_size: f64,
        /// Forma delle celle (default `square`).
        shape: GridShape,
    },
    /// `geo.coverage_validate` (blocking, WholeToMany): overlap della
    /// copertura via `extensions3::coverage_validate_rows`.
    GeoCoverageValidate {
        /// Area minima di overlap segnalata (default 0).
        tolerance: f64,
        /// Limite di issue (default `DEFAULT_MAX_ISSUES`).
        max_issues: usize,
    },
    /// `geo.shared_paths` (blocking, WholeToMany): confini condivisi via
    /// `extensions3::shared_paths_rows`.
    GeoSharedPaths {
        /// Lunghezza minima del segmento collineare (default 0).
        tolerance: f64,
        /// Lunghezza minima del tratto condiviso (default 0).
        min_length: f64,
    },
    /// `geo.cluster_dbscan` (blocking, output `OneToOne` allineato alle
    /// righe): etichetta `UInt64` nullable per riga via
    /// `cluster::dbscan_column`.
    GeoClusterDbscan {
        /// Raggio di vicinato (finito, > 0).
        eps: f64,
        /// Punti minimi per un cluster (>= 1).
        min_points: usize,
        /// Nome della colonna prodotta (derivato dal contratto di output).
        output_column: String,
    },
}

/// Kernel fisico di un segmento (E1/E3).
///
/// Mantiene la mappa verso il nodo logico originario (attribuzione errori,
/// metriche per nodo, limiti per arco) e tutto cio' che e' risolvibile prima
/// dell'hot path (V2).
#[derive(Debug)]
pub struct PreparedKernel {
    /// Id del nodo logico del piano.
    pub node_id: String,
    /// Id canonico dell'operazione (`table.*`/`geo.*`).
    pub operation: &'static str,
    /// Famiglia dell'operazione.
    pub family: Family,
    /// Ruolo geo dentro a un segmento `GeoFused` (punto di aggancio della
    /// cache di decode di Fase 2C); `None` per i kernel tabellari.
    pub geo_role: Option<GeoRole>,
    /// Indice risolto della colonna geometria attiva nel batch di input
    /// del nodo (V2: nessuna ricerca per nome a runtime).
    pub geometry_column_index: Option<usize>,
    /// Configurazione preparata.
    pub config: PreparedConfig,
    /// Contratti degli archi di input del nodo (1 o 2).
    pub input_contracts: Vec<DataContract>,
    /// Contratto dell'arco di output del nodo.
    pub output_contract: DataContract,
}

/// Segmento fisico dell'`ExecutionPlan` (E2).
#[derive(Debug)]
pub struct PhysicalSegment {
    /// Id del segmento (`seg0`, `seg1`, ... in ordine topologico).
    pub id: String,
    /// Kernel fusi del segmento (1 per i segmenti blocking).
    pub kernels: Box<[PreparedKernel]>,
    /// Modalita' fisica esplicita.
    pub mode: SegmentMode,
    /// Strategia di parallelismo scelta (v1: seriale ovunque, V8).
    pub parallelism: ParallelismStrategy,
    /// Archi di input del segmento (1 per streaming/blocking, 2 per
    /// binary-blocking): nomi di input del piano o id di nodi produttori.
    pub input_edges: Box<[String]>,
    /// Arco prodotto dal segmento (id dell'ultimo nodo fuso).
    pub output_edge: String,
    /// Contratto dell'arco di output.
    pub output_contract: DataContract,
    /// Materializzazione esplicitamente prevista dal piano (V9/D9): `true`
    /// se l'arco di output ha piu' di un consumatore (fan-out) — in quel
    /// caso l'executor condivide i batch immutabili tra i consumatori
    /// senza copie di buffer.
    pub materialize_output: bool,
}

/// Ultimo consumatore di un arco (V10): dopo di esso le risorse
/// intermedie dell'arco sono rilasciabili.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LastConsumer {
    /// Un nodo del piano (id).
    Node(String),
    /// L'output del piano.
    Output,
}

/// Decisioni fisiche per una singola esecuzione (ADR 5).
///
/// Prodotto da [`prepare`], consumato dall'executor. Uno stesso
/// `ValidatedGraph` puo' produrre piani diversi su contesti runtime diversi.
#[derive(Debug)]
pub struct ExecutionPlan {
    segments: Box<[PhysicalSegment]>,
    /// Nodo → indice del segmento che lo contiene.
    node_segment: HashMap<String, usize>,
    last_consumers: BTreeMap<String, LastConsumer>,
    output_edge: String,
    metrics_config: MetricsConfig,
    batch_target: BatchTarget,
    limits: Limits,
    /// Statistiche per input come dichiarate nel `RuntimeContext`
    /// (osservabilita'; nessuna scelta fisica v1 dipende da esse).
    input_statistics: BTreeMap<String, InputStatistics>,
}

impl ExecutionPlan {
    /// Segmenti in ordine topologico.
    #[must_use]
    pub fn segments(&self) -> &[PhysicalSegment] {
        &self.segments
    }

    /// Indice del segmento che contiene il nodo.
    #[must_use]
    pub fn segment_of(&self, node_id: &str) -> Option<usize> {
        self.node_segment.get(node_id).copied()
    }

    /// Last consumer per arco (V10).
    #[must_use]
    pub const fn last_consumers(&self) -> &BTreeMap<String, LastConsumer> {
        &self.last_consumers
    }

    /// Arco di output del piano.
    #[must_use]
    pub fn output_edge(&self) -> &str {
        &self.output_edge
    }

    /// Configurazione delle metriche da raccogliere (E3).
    #[must_use]
    pub const fn metrics_config(&self) -> MetricsConfig {
        self.metrics_config
    }

    /// Dimensionamento dei batch deciso in `prepare` (V7).
    #[must_use]
    pub const fn batch_target(&self) -> BatchTarget {
        self.batch_target
    }

    /// Limiti effettivi del piano (da applicare in esecuzione).
    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Statistiche di input registrate nel piano (ADR 5).
    #[must_use]
    pub const fn input_statistics(&self) -> &BTreeMap<String, InputStatistics> {
        &self.input_statistics
    }
}

/// Vista pubblica di sola lettura sulla strategia fisica (dry-run, ADR 5):
/// restituisce l'[`ExecutionPlan`] che `execute` produrrebbe per questo
/// grafo e contesto, **senza eseguire nulla**.
///
/// L'API operativa resta a due passi (`validate` -> `execute`); `explain`
/// esiste per l'ispezione (es. `validate` della CLI, che mostra segmenti e
/// strategia prima di correre) e condivide con `execute` lo stesso esito di
/// fattibilita': un piano fuori dal dispatch v1 fallisce qui come là.
///
/// # Errors
///
/// Come la `prepare` interna: `PlenoraError::Unsupported` per operazioni
/// fuori dal dispatch v1 (fail-closed a secco, non a meta' esecuzione).
pub fn explain(graph: &ValidatedGraph, runtime: &RuntimeContext) -> Result<ExecutionPlan> {
    prepare(graph, runtime)
}

/// `prepare` (Architetture.md par. 6.3, ADR 5): decisioni fisiche per questa
/// esecuzione a partire dal grafo validato e dal contesto runtime.
///
/// **Interna al crate** (ADR 5): l'API pubblica del motore e' a due passi
/// (`validate` -> `execute`); la strategia fisica e' un dettaglio di
/// implementazione di `execute`. L'unica vista pubblica e' [`explain`],
/// per l'ispezione a secco (dry-run della CLI).
///
/// Funzione pura e a secco: nessuna lettura di dati. Produce sempre un piano
/// valido con statistiche assenti (`Unknown` → conservativo, ADR 5).
///
/// # Errors
///
/// `PlenoraError::Unsupported` per operazioni fuori dal dispatch v1
/// dell'executor (fail-closed qui, non a meta' stream);
/// `PlenoraError::Contract`/`PlenoraError::Schema` se una configurazione gia'
/// validata semanticamente non supera la rivalidazione fisica (difesa in
/// profondita': non dovrebbe accadere su un `ValidatedGraph` genuino).
///
/// # Panics
///
/// Solo su invarianti interne gia' garantite dalla fase 1 `validate` (op
/// risolta, arco inferito, ogni nodo in esattamente un segmento): mai su
/// input esterno.
pub(crate) fn prepare(graph: &ValidatedGraph, runtime: &RuntimeContext) -> Result<ExecutionPlan> {
    let plan = graph.plan().plan();
    let topo = graph.topological_order();
    let limits = graph.effective_limits().clone();

    // Fan-out per arco: numero di nodi consumatori + 1 se l'arco e' l'output.
    let mut consumers: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in &plan.nodes {
        for reference in &node.inputs {
            consumers
                .entry(reference.as_str())
                .or_default()
                .push(node.id.as_str());
        }
    }
    let fan_out: HashMap<&str, usize> = plan
        .inputs
        .iter()
        .chain(plan.nodes.iter().map(|node| &node.id))
        .map(|edge| {
            (
                edge.as_str(),
                consumers.get(edge.as_str()).map_or(0, Vec::len)
                    + usize::from(plan.output == *edge),
            )
        })
        .collect();

    // Kernel preparati per nodo (E1): config tipizzate, indici e CRS risolti.
    let nodes_by_id: HashMap<&str, &NodeV4> = plan
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut kernels_by_id: HashMap<&str, PreparedKernel> = HashMap::with_capacity(plan.nodes.len());
    for node_id in topo {
        let node = nodes_by_id[node_id.as_str()];
        let kernel = prepare_kernel(graph, node, &limits)?;
        kernels_by_id.insert(node.id.as_str(), kernel);
    }

    // Metadati di catalogo per nodo (guidano la scomposizione in segmenti).
    let mut node_meta: HashMap<&str, (ExecutionClass, Arity, Family)> =
        HashMap::with_capacity(plan.nodes.len());
    for node in &plan.nodes {
        let descriptor = plenora_core::catalog::find_operation(&node.op).ok_or_else(|| {
            PlenoraError::Contract(format!(
                "internal error: nodo `{}`: op risolta in validazione",
                node.id
            ))
        })?;
        node_meta.insert(
            node.id.as_str(),
            (
                descriptor.execution_class,
                descriptor.arity,
                descriptor.family,
            ),
        );
    }

    // Scomposizione in segmenti: catene massimali di nodi Streaming unari
    // con arco intermedio a fan-out 1; ogni nodo blocking e' un segmento.
    let chains = build_chains(topo, &node_meta, &consumers, &fan_out)?;
    let (segments, node_segment) =
        build_segments(graph, &nodes_by_id, &node_meta, &fan_out, chains, &mut kernels_by_id)?;
    let last_consumers = compute_last_consumers(plan, topo, &consumers);

    let input_statistics = plan
        .inputs
        .iter()
        .map(|name| (name.clone(), runtime.input_statistics(name)))
        .collect();

    Ok(ExecutionPlan {
        segments: segments.into_boxed_slice(),
        node_segment,
        last_consumers,
        output_edge: plan.output.clone(),
        metrics_config: runtime.metrics,
        batch_target: runtime.batch_target,
        limits,
        input_statistics,
    })
}

/// Catene massimali di nodi `Streaming` unari con arco intermedio a fan-out
/// 1 (nessun consumatore esterno degli archi interni del segmento); ogni
/// nodo blocking e' una catena a se'.
fn build_chains<'a>(
    topo: &'a [String],
    node_meta: &HashMap<&'a str, (ExecutionClass, Arity, Family)>,
    consumers: &HashMap<&'a str, Vec<&'a str>>,
    fan_out: &HashMap<&'a str, usize>,
) -> Result<Vec<Vec<&'a str>>> {
    let mut chains: Vec<Vec<&'a str>> = Vec::new();
    let mut assigned: HashMap<&'a str, bool> = HashMap::with_capacity(topo.len());
    for node_id in topo {
        if assigned.contains_key(node_id.as_str()) {
            continue;
        }
        let mut chain: Vec<&'a str> = vec![node_id.as_str()];
        if node_meta[node_id.as_str()].0 == ExecutionClass::Streaming {
            // Estendi la catena finche' il consumatore unico e' uno
            // streaming unario.
            loop {
                let Some(&last) = chain.last() else {
                    return Err(PlenoraError::Contract(
                        "internal error: catena non vuota".to_owned(),
                    ));
                };
                if fan_out[last] != 1 {
                    break;
                }
                let next = consumers
                    .get(last)
                    .and_then(|list| list.first())
                    .copied();
                let Some(next) = next else { break };
                let (next_class, next_arity, _) = node_meta[next];
                if next_class != ExecutionClass::Streaming || next_arity != Arity::Unary {
                    break;
                }
                chain.push(next);
            }
        }
        for id in &chain {
            assigned.insert(*id, true);
        }
        chains.push(chain);
    }
    Ok(chains)
}

/// Costruisce i [`PhysicalSegment`] dalle catene, muovendo i kernel
/// preparati (ogni nodo appartiene a esattamente un segmento).
#[allow(clippy::too_many_arguments)]
fn build_segments<'a>(
    graph: &ValidatedGraph,
    nodes_by_id: &HashMap<&'a str, &'a NodeV4>,
    node_meta: &HashMap<&'a str, (ExecutionClass, Arity, Family)>,
    fan_out: &HashMap<&'a str, usize>,
    chains: Vec<Vec<&'a str>>,
    kernels_by_id: &mut HashMap<&'a str, PreparedKernel>,
) -> Result<(Vec<PhysicalSegment>, HashMap<String, usize>)> {
    let mut segments: Vec<PhysicalSegment> = Vec::with_capacity(chains.len());
    let mut node_segment: HashMap<String, usize> = HashMap::with_capacity(nodes_by_id.len());
    for chain in chains {
        let first = chain[0];
        let last = chain[chain.len() - 1];
        let (class, _, _) = node_meta[first];
        let input_count = nodes_by_id[first].inputs.len();
        let mode = match class {
            ExecutionClass::Streaming => {
                if chain.iter().all(|id| node_meta[id].2 == Family::Geo) {
                    SegmentMode::GeoFused
                } else {
                    SegmentMode::LinearStreaming
                }
            }
            // Il numero di input decide la forma blocking: `BinaryOrdered`
            // e le N-arie a due input (`table.concat`) sono BinaryBlocking.
            ExecutionClass::Blocking | ExecutionClass::BinaryBlocking => {
                if input_count == 2 {
                    SegmentMode::BinaryBlocking
                } else {
                    SegmentMode::Blocking
                }
            }
        };
        let parallelism = match mode {
            SegmentMode::LinearStreaming | SegmentMode::GeoFused => ParallelismStrategy::SerialFused,
            SegmentMode::Blocking | SegmentMode::BinaryBlocking => {
                ParallelismStrategy::BlockingSingleTask
            }
        };

        let index = segments.len();
        for id in &chain {
            node_segment.insert((*id).to_owned(), index);
        }
        let kernels: Vec<PreparedKernel> = chain
            .iter()
            .map(|id| {
                kernels_by_id.remove(id).ok_or_else(|| {
                    PlenoraError::Contract(format!(
                        "internal error: nodo `{id}`: ogni nodo in esattamente un segmento"
                    ))
                })
            })
            .collect::<Result<_>>()?;
        segments.push(PhysicalSegment {
            id: format!("seg{index}"),
            kernels: kernels.into_boxed_slice(),
            mode,
            parallelism,
            input_edges: nodes_by_id[first].inputs.clone().into_boxed_slice(),
            output_edge: last.to_owned(),
            output_contract: graph
                .edge_contract(last)
                .ok_or_else(|| {
                    PlenoraError::Contract(format!(
                        "internal error: arco `{last}` inferito in validazione"
                    ))
                })?
                .clone(),
            materialize_output: fan_out[last] > 1,
        });
    }
    Ok((segments, node_segment))
}

/// Last consumer per arco (V10): l'ultimo nodo consumatore in ordine
/// topologico; l'output del piano e' il consumatore finale del suo arco.
fn compute_last_consumers<'a>(
    plan: &'a crate::plan::PlanV4,
    topo: &[String],
    consumers: &HashMap<&'a str, Vec<&'a str>>,
) -> BTreeMap<String, LastConsumer> {
    let topo_position: HashMap<&str, usize> = topo
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let mut last_consumers: BTreeMap<String, LastConsumer> = BTreeMap::new();
    for edge in plan
        .inputs
        .iter()
        .chain(plan.nodes.iter().map(|node| &node.id))
    {
        let consumer = if plan.output == *edge {
            LastConsumer::Output
        } else {
            consumers
                .get(edge.as_str())
                .and_then(|list| {
                    list.iter()
                        .max_by_key(|id| topo_position.get(*id).copied().unwrap_or(0))
                })
                .map_or(LastConsumer::Output, |id| {
                    LastConsumer::Node((*id).to_owned())
                })
        };
        last_consumers.insert(edge.clone(), consumer);
    }
    last_consumers
}

/// Prepara il kernel di un nodo: config tipizzata e rivalidata, indice della
/// colonna geometria e CRS risolti (E1/V2).
fn prepare_kernel(
    graph: &ValidatedGraph,
    node: &NodeV4,
    limits: &Limits,
) -> Result<PreparedKernel> {
    let descriptor = plenora_core::catalog::find_operation(&node.op).ok_or_else(|| {
        PlenoraError::Contract(format!(
            "internal error: nodo `{}`: op risolta in validazione",
            node.id
        ))
    })?;
    let input_contracts: Vec<DataContract> = node
        .inputs
        .iter()
        .map(|edge| {
            graph
                .edge_contract(edge)
                .ok_or_else(|| {
                    PlenoraError::Contract(format!(
                        "internal error: arco `{edge}` inferito in validazione"
                    ))
                })
                .cloned()
        })
        .collect::<Result<_>>()?;
    let output_contract = graph
        .edge_contract(&node.id)
        .ok_or_else(|| {
            PlenoraError::Contract(format!(
                "internal error: arco `{}` inferito in validazione",
                node.id
            ))
        })?
        .clone();
    let geometry_column_index = match input_contracts
        .first()
        .and_then(|contract| contract.active_geometry_column())
    {
        Some(geometry) => Some(
            input_contracts[0]
                .schema
                .column_with_name(&geometry.name)
                .ok_or_else(|| {
                    PlenoraError::Contract(
                        "internal error: colonna geometria nel contratto".to_owned(),
                    )
                })?
                .0,
        ),
        None => None,
    };

    let legacy_limits = legacy_limits(limits);
    let (config, geo_role) = match descriptor.family {
        Family::Table => (prepare_table(node, descriptor, &legacy_limits)?, None),
        Family::Geo => {
            let (config, role) = prepare_geo(node, descriptor, &input_contracts[0], &output_contract)?;
            (config, Some(role))
        }
    };

    Ok(PreparedKernel {
        node_id: node.id.clone(),
        operation: descriptor.id,
        family: descriptor.family,
        geo_role,
        geometry_column_index,
        config,
        input_contracts,
        output_contract,
    })
}

/// Limiti del motore tabellare legacy derivati dai limiti effettivi del
/// piano (i campi senza corrispettivo restano ai default legacy).
///
/// Mapping documentato (semantica ADR 6): il `max_rows` legacy e' un limite
/// **per batch/tabella** del motore a tabella intera; qui lo si ancora a
/// `max_input_rows` (tetto conservativo sulla tabella materializzata dai
/// nodi blocking). I limiti per arco (`max_rows_per_edge`) e di espansione
/// restano applicati dall'executor con la semantica cumulativa corretta —
/// mapparli sul `max_rows` legacy li farebbe scattare per batch, non per
/// arco.
fn legacy_limits(limits: &Limits) -> table_engine::Limits {
    let defaults = table_engine::Limits::default();
    table_engine::Limits {
        max_rows: usize::try_from(limits.rows.max_input_rows).unwrap_or(usize::MAX),
        max_columns: defaults.max_columns,
        max_string_bytes: limits.max_string_bytes,
        max_regex_bytes: limits.max_regex_bytes,
        max_split_columns: defaults.max_split_columns,
        max_memory_bytes: usize::try_from(limits.max_memory_bytes).unwrap_or(usize::MAX),
        max_temp_bytes: limits.max_temp_bytes,
        spill_partitions: usize::try_from(limits.spill_partitions.max(2)).unwrap_or(usize::MAX),
    }
}

/// Kernel tabellare: piano legacy di un solo step, validato in `prepare`
/// (config deserializzata e controllata qui, non nel loop per batch).
fn prepare_table(
    node: &NodeV4,
    descriptor: &plenora_core::catalog::OperationDescriptor,
    legacy_limits: &table_engine::Limits,
) -> Result<PreparedConfig> {
    let step = table_engine::Step {
        operation: descriptor.id.to_owned(),
        config: node.config.clone(),
    };
    let plan = table_engine::Plan {
        schema_version: table_engine::SCHEMA_VERSION,
        limits: legacy_limits.clone(),
        steps: vec![step],
    };
    let validated = plan.validate().map_err(|error| {
        PlenoraError::Contract(format!(
            "nodo `{}`: rivalidazione fisica della config fallita: {error}",
            node.id
        ))
    })?;
    match descriptor.arity {
        plenora_core::catalog::Arity::Unary => Ok(PreparedConfig::TableUnary(Box::new(validated))),
        plenora_core::catalog::Arity::BinaryOrdered => {
            Ok(PreparedConfig::TableBinary(Box::new(validated)))
        }
        plenora_core::catalog::Arity::NAry => {
            if node.inputs.len() == 2 {
                // `table.concat` a due input usa il dispatch binario legacy.
                Ok(PreparedConfig::TableBinary(Box::new(validated)))
            } else {
                Err(PlenoraError::Unsupported(format!(
                    "nodo `{}`: {} con {} input: l'executor v1 supporta solo 2 \
                     (N-aria e' Fase 2B)",
                    node.id,
                    descriptor.id,
                    node.inputs.len()
                )))
            }
        }
    }
}

/// Config serde della trasformazione geo 1:1 (nomi e domini come
/// `analyze.rs`; la validazione stretta e' gia' avvenuta nel planner).
#[derive(Debug, Default, Deserialize)]
struct GeoTransformConfig {
    distance: Option<f64>,
    cap: Option<BufferCap>,
    tolerance: Option<f64>,
    policy: Option<SimplifyPolicyParam>,
    target_crs: Option<String>,
    coefficients: Option<Vec<f64>>,
    x_offset: Option<f64>,
    y_offset: Option<f64>,
    x_factor: Option<f64>,
    y_factor: Option<f64>,
    x_origin: Option<f64>,
    y_origin: Option<f64>,
    degrees: Option<f64>,
    concavity: Option<f64>,
    length_threshold: Option<f64>,
    max_segment_length: Option<f64>,
    grid_size: Option<f64>,
    start_ratio: Option<f64>,
    end_ratio: Option<f64>,
    ratio: Option<f64>,
}

/// Config serde delle misure "add column" (`output_column` opzionale).
#[derive(Debug, Deserialize)]
struct GeoMeasureConfig {
    output_column: Option<String>,
}

/// Config serde di `geo.from_wkt` (nomi e domini come `analyze.rs`; la
/// validazione stretta e' gia' avvenuta nel planner).
#[derive(Debug, Deserialize)]
struct GeoFromWktConfig {
    wkt_column: String,
    output_column: Option<String>,
    on_error: Option<OnWktError>,
    crs: Option<String>,
}

/// Config serde di `geo.geometry_accessors` (selezione per nome canonico).
#[derive(Debug, Deserialize)]
struct GeoAccessorsConfig {
    fields: Option<Vec<String>>,
    output_prefix: Option<String>,
}

/// Config serde di `geo.line_locate_point` (punto WKB hex, D16).
#[derive(Debug, Deserialize)]
struct GeoLineLocatePointConfig {
    point_wkb: String,
    output_column: Option<String>,
}

/// Config serde di `geo.subdivide`.
#[derive(Debug, Deserialize)]
struct GeoSubdivideConfig {
    max_vertices: usize,
    output_column: Option<String>,
}

/// Config serde di `geo.snap` (riferimento WKB hex, D16).
#[derive(Debug, Deserialize)]
struct GeoSnapConfig {
    reference_wkb: String,
    tolerance: f64,
}

/// Config serde di `geo.collect`.
#[derive(Debug, Deserialize)]
struct GeoCollectConfig {
    group_by: Vec<String>,
}

/// Extent serde di `geo.generate_grid`.
#[derive(Debug, Deserialize)]
struct GeoGridExtentConfig {
    xmin: f64,
    ymin: f64,
    xmax: f64,
    ymax: f64,
}

/// Config serde di `geo.generate_grid`.
#[derive(Debug, Deserialize)]
struct GeoGenerateGridConfig {
    extent: GeoGridExtentConfig,
    cell_size: f64,
    shape: Option<GridShape>,
    crs: Option<String>,
    include_centroid: Option<bool>,
}

/// Config serde di `geo.coverage_validate` (default kernel).
#[derive(Debug, Deserialize)]
struct GeoCoverageValidateConfig {
    tolerance: Option<f64>,
    max_issues: Option<usize>,
}

/// Config serde di `geo.shared_paths` (default kernel).
#[derive(Debug, Deserialize)]
struct GeoSharedPathsConfig {
    tolerance: Option<f64>,
    min_length: Option<f64>,
}

/// Config serde di `geo.cluster_dbscan`.
#[derive(Debug, Deserialize)]
struct GeoClusterDbscanConfig {
    eps: f64,
    min_points: usize,
    output_column: Option<String>,
}

/// Mapping op geo v4 → [`ArrowOperation`] del trasporto (trasformazioni 1:1
/// in place coperte dal dispatch v1).
fn geo_transform_operation(id: &str) -> Option<ArrowOperation> {
    match id {
        "geo.centroid" => Some(ArrowOperation::Centroid),
        "geo.convex_hull" => Some(ArrowOperation::ConvexHull),
        "geo.envelope" => Some(ArrowOperation::Envelope),
        "geo.buffer" => Some(ArrowOperation::Buffer),
        "geo.simplify" => Some(ArrowOperation::Simplify),
        "geo.boundary" => Some(ArrowOperation::Boundary),
        "geo.point_on_surface" => Some(ArrowOperation::PointOnSurface),
        "geo.make_valid" => Some(ArrowOperation::MakeValid),
        "geo.reproject" => Some(ArrowOperation::Reproject),
        "geo.affine_transform" => Some(ArrowOperation::AffineTransform),
        "geo.translate" => Some(ArrowOperation::Translate),
        "geo.scale" => Some(ArrowOperation::Scale),
        "geo.rotate" => Some(ArrowOperation::Rotate),
        "geo.concave_hull" => Some(ArrowOperation::ConcaveHull),
        "geo.densify" => Some(ArrowOperation::Densify),
        "geo.snap_to_grid" => Some(ArrowOperation::SnapToGrid),
        "geo.line_substring" => Some(ArrowOperation::LineSubstring),
        "geo.line_interpolate_point" => Some(ArrowOperation::LineInterpolatePoint),
        _ => None,
    }
}

/// Kernel geo: trasformazioni 1:1 in place via `transform_batches`, misure
/// "add column" via dispatch dedicato; il resto e' fuori dal dispatch v1.
fn prepare_geo(
    node: &NodeV4,
    descriptor: &plenora_core::catalog::OperationDescriptor,
    input_contract: &DataContract,
    output_contract: &DataContract,
) -> Result<(PreparedConfig, GeoRole)> {
    if let Some(operation) = geo_transform_operation(descriptor.id) {
        let parsed: GeoTransformConfig = serde_json::from_value(node.config.clone())?;
        let geometry = input_contract
            .active_geometry_column()
            .ok_or_else(|| {
                PlenoraError::Contract(
                    "internal error: geometria attiva verificata in validazione".to_owned(),
                )
            })?;
        let params = TransformArrowSchema {
            schema_version: TransformArrowSchema::VERSION,
            operation,
            row_count: 0,
            crs: Some(geometry.crs.definition().to_owned()),
            geometry_column: Some(geometry.name.clone()),
            distance: parsed.distance,
            cap: parsed.cap,
            tolerance: parsed.tolerance,
            simplify_policy: parsed.policy,
            target_crs: parsed.target_crs,
            max_output_rows: None,
            max_points: None,
            x_column: None,
            y_column: None,
            snap_tolerance: None,
            remove_overlaps: None,
            fill_gaps: None,
            coefficients: parsed.coefficients,
            x_offset: parsed.x_offset,
            y_offset: parsed.y_offset,
            x_factor: parsed.x_factor,
            y_factor: parsed.y_factor,
            degrees: parsed.degrees,
            x_origin: parsed.x_origin,
            y_origin: parsed.y_origin,
            concavity: parsed.concavity,
            length_threshold: parsed.length_threshold,
            max_segment_length: parsed.max_segment_length,
            grid_size: parsed.grid_size,
            start_ratio: parsed.start_ratio,
            end_ratio: parsed.end_ratio,
            ratio: parsed.ratio,
            node_input: None,
            require_complete: None,
        };
        params.validate_parameters().map_err(|error| {
            PlenoraError::Contract(format!(
                "nodo `{}`: rivalidazione fisica dei parametri fallita: {error}",
                node.id
            ))
        })?;
        return Ok((
            PreparedConfig::GeoTransform(Box::new(params)),
            GeoRole::TransformInPlace,
        ));
    }

    let measure = match descriptor.id {
        "geo.area" => Some(MeasureKind::Area),
        "geo.length" => Some(MeasureKind::Length),
        "geo.perimeter" => Some(MeasureKind::Perimeter),
        "geo.vertex_count" => Some(MeasureKind::VertexCount),
        "geo.to_wkt" => Some(MeasureKind::ToWkt),
        _ => None,
    };
    if let Some(measure) = measure {
        let parsed: GeoMeasureConfig = serde_json::from_value(node.config.clone())?;
        let output_column = measure_output_column(
            &node.id,
            input_contract,
            output_contract,
            parsed.output_column.as_deref(),
        )?;
        return Ok((
            PreparedConfig::GeoMeasure {
                measure,
                output_column,
            },
            GeoRole::MeasureAddColumn,
        ));
    }

    if let Some(prepared) = prepare_geo_extension(node, descriptor, input_contract, output_contract)? {
        return Ok(prepared);
    }

    Err(PlenoraError::Unsupported(format!(
        "nodo `{}`: {} non e' nel dispatch v1 dell'executor (Fase 2A-4): \
         coperte le trasformazioni geo 1:1 in place, le misure area/length/\
         perimeter/vertex_count/to_wkt e le estensioni v1.1-v1.3 (from_wkt, \
         geometry_accessors, collect, line_locate_point, generate_grid, \
         subdivide, snap, coverage_validate, shared_paths, cluster_dbscan); \
         il resto e' Fase 2B/2C",
        node.id, descriptor.id
    )))
}

/// Estensioni geo v1.1-v1.3: config tipizzate e rivalidate (E1), secondo
/// operando da config decodificato una volta qui (mai nel loop per batch).
/// `None` se l'op non e' un'estensione coperta.
// Dispatcher esaustivo sulle estensioni v1.1-v1.3: la lunghezza e' data
// dalla sequenza lineare dei casi (config tipizzata + validazione per op),
// non da complessita' logica (fase di pulizia: niente refactor strutturali).
#[allow(clippy::too_many_lines)]
fn prepare_geo_extension(
    node: &NodeV4,
    descriptor: &plenora_core::catalog::OperationDescriptor,
    input_contract: &DataContract,
    output_contract: &DataContract,
) -> Result<Option<(PreparedConfig, GeoRole)>> {
    let prepared = match descriptor.id {
        "geo.from_wkt" => {
            let parsed: GeoFromWktConfig = serde_json::from_value(node.config.clone())?;
            // `output_column` e `crs` sono semantica di contratto (nome e CRS
            // della colonna prodotta): gia' applicati dal planner.
            let _ = (&parsed.output_column, &parsed.crs);
            let (wkt_column_index, _) = input_contract
                .schema
                .column_with_name(&parsed.wkt_column)
                .ok_or_else(|| {
                    PlenoraError::Schema(format!(
                        "nodo `{}`: colonna WKT `{}` assente dal contratto di input",
                        node.id, parsed.wkt_column
                    ))
                })?;
            (
                PreparedConfig::GeoFromWkt {
                    wkt_column_index,
                    on_error: parsed.on_error.unwrap_or(OnWktError::Null),
                },
                GeoRole::ProduceFromText,
            )
        }
        "geo.geometry_accessors" => {
            let parsed: GeoAccessorsConfig = serde_json::from_value(node.config.clone())?;
            let prefix = parsed.output_prefix.as_deref().unwrap_or("");
            let selected: Vec<AccessorKind> = match &parsed.fields {
                None => AccessorKind::ALL.to_vec(),
                Some(names) => names
                    .iter()
                    .map(|name| {
                        AccessorKind::from_canonical_name(name).ok_or_else(|| {
                            PlenoraError::Contract(format!(
                                "nodo `{}`: accessorio `{name}` sconosciuto",
                                node.id
                            ))
                        })
                    })
                    .collect::<Result<_>>()?,
            };
            // Ordine canonico e deduplicazione per costruzione: si iterano
            // gli accessori del canone e si tengono quelli selezionati
            // (ogni `AccessorKind` e' in `ALL`, quindi la posizione esiste
            // sempre per costruzione).
            let selected: Vec<AccessorKind> = AccessorKind::ALL
                .into_iter()
                .filter(|kind| selected.contains(kind))
                .collect();
            // I nomi di output (prefisso applicato) devono esistere nel
            // contratto inferito dal planner (difesa in profondita').
            let mut columns: Vec<(String, AccessorKind)> = Vec::with_capacity(selected.len());
            for kind in selected {
                let name = format!("{prefix}{}", kind.canonical_name());
                if output_contract.schema.field_with_name(&name).is_err() {
                    return Err(PlenoraError::Schema(format!(
                        "nodo `{}`: colonna accessoria `{name}` assente dal contratto \
                         di output inferito",
                        node.id
                    )));
                }
                columns.push((name, kind));
            }
            (
                PreparedConfig::GeoAccessors {
                    columns: columns.into_boxed_slice(),
                },
                GeoRole::MeasureAddColumn,
            )
        }
        "geo.line_locate_point" => {
            let parsed: GeoLineLocatePointConfig = serde_json::from_value(node.config.clone())?;
            let Geometry::Point(point) = decode_wkb_hex(&node.id, "point_wkb", &parsed.point_wkb)?
            else {
                return Err(PlenoraError::Contract(format!(
                    "nodo `{}`: point_wkb deve essere un Point",
                    node.id
                )));
            };
            let output_column = measure_output_column(
                &node.id,
                input_contract,
                output_contract,
                parsed.output_column.as_deref(),
            )?;
            (
                PreparedConfig::GeoLineLocatePoint { point, output_column },
                GeoRole::MeasureAddColumn,
            )
        }
        "geo.subdivide" => {
            let parsed: GeoSubdivideConfig = serde_json::from_value(node.config.clone())?;
            // `output_column` e' semantica di contratto (rinomina in place):
            // gia' applicata dal planner, niente da fare a runtime.
            let _ = &parsed.output_column;
            if parsed.max_vertices < plenora_kernels_geo::extensions2::MIN_SUBDIVIDE_VERTICES {
                return Err(PlenoraError::Contract(format!(
                    "nodo `{}`: max_vertices deve essere almeno 4 (anello chiuso minimo)",
                    node.id
                )));
            }
            (
                PreparedConfig::GeoSubdivide {
                    max_vertices: parsed.max_vertices,
                },
                GeoRole::OneToMany,
            )
        }
        "geo.snap" => {
            let parsed: GeoSnapConfig = serde_json::from_value(node.config.clone())?;
            let reference = decode_wkb_hex(&node.id, "reference_wkb", &parsed.reference_wkb)?;
            if !parsed.tolerance.is_finite() || parsed.tolerance < 0.0 {
                return Err(PlenoraError::Contract(format!(
                    "nodo `{}`: tolerance deve essere finita e non negativa",
                    node.id
                )));
            }
            (
                PreparedConfig::GeoSnap {
                    reference,
                    tolerance: parsed.tolerance,
                },
                GeoRole::TransformInPlace,
            )
        }
        "geo.collect" => {
            let parsed: GeoCollectConfig = serde_json::from_value(node.config.clone())?;
            let mut indices: Vec<usize> = Vec::with_capacity(parsed.group_by.len());
            for name in &parsed.group_by {
                let (index, _) = input_contract.schema.column_with_name(name).ok_or_else(|| {
                    PlenoraError::Schema(format!(
                        "nodo `{}`: colonna chiave `{name}` assente dal contratto di input",
                        node.id
                    ))
                })?;
                indices.push(index);
            }
            (
                PreparedConfig::GeoCollect {
                    group_by_indices: indices.into_boxed_slice(),
                },
                GeoRole::WholeTable,
            )
        }
        "geo.generate_grid" => {
            let parsed: GeoGenerateGridConfig = serde_json::from_value(node.config.clone())?;
            // `crs` e `include_centroid` sono semantica di contratto (CRS e
            // colonne dell'output): gia' applicati dal planner.
            let _ = (&parsed.crs, &parsed.include_centroid);
            let extent = GridExtent::new(
                parsed.extent.xmin,
                parsed.extent.ymin,
                parsed.extent.xmax,
                parsed.extent.ymax,
            )
            .map_err(|error| {
                PlenoraError::Contract(format!("nodo `{}`: {error}", node.id))
            })?;
            let shape = parsed.shape.unwrap_or(GridShape::Square);
            // Rivalidazione fisica: cell_size e limite celle (E1).
            plenora_kernels_geo::extensions2::grid_cell_count(&extent, parsed.cell_size, shape)
                .map_err(|error| {
                    PlenoraError::Contract(format!("nodo `{}`: {error}", node.id))
                })?;
            (
                PreparedConfig::GeoGenerateGrid {
                    extent,
                    cell_size: parsed.cell_size,
                    shape,
                },
                GeoRole::WholeTable,
            )
        }
        "geo.coverage_validate" => {
            let parsed: GeoCoverageValidateConfig = serde_json::from_value(node.config.clone())?;
            let tolerance = parsed.tolerance.unwrap_or(0.0);
            if !tolerance.is_finite() || tolerance < 0.0 {
                return Err(PlenoraError::Contract(format!(
                    "nodo `{}`: tolerance deve essere finita e non negativa",
                    node.id
                )));
            }
            (
                PreparedConfig::GeoCoverageValidate {
                    tolerance,
                    max_issues: parsed
                        .max_issues
                        .unwrap_or(plenora_kernels_geo::extensions3::DEFAULT_MAX_ISSUES),
                },
                GeoRole::WholeTable,
            )
        }
        "geo.shared_paths" => {
            let parsed: GeoSharedPathsConfig = serde_json::from_value(node.config.clone())?;
            let tolerance = parsed.tolerance.unwrap_or(0.0);
            let min_length = parsed.min_length.unwrap_or(0.0);
            for (name, value) in [("tolerance", tolerance), ("min_length", min_length)] {
                if !value.is_finite() || value < 0.0 {
                    return Err(PlenoraError::Contract(format!(
                        "nodo `{}`: {name} deve essere finita e non negativa",
                        node.id
                    )));
                }
            }
            (
                PreparedConfig::GeoSharedPaths {
                    tolerance,
                    min_length,
                },
                GeoRole::WholeTable,
            )
        }
        "geo.cluster_dbscan" => {
            let parsed: GeoClusterDbscanConfig = serde_json::from_value(node.config.clone())?;
            if !parsed.eps.is_finite() || parsed.eps <= 0.0 {
                return Err(PlenoraError::Contract(format!(
                    "nodo `{}`: eps deve essere finito e maggiore di zero",
                    node.id
                )));
            }
            if parsed.min_points < 1 {
                return Err(PlenoraError::Contract(format!(
                    "nodo `{}`: min_points deve essere almeno 1",
                    node.id
                )));
            }
            let output_column = measure_output_column(
                &node.id,
                input_contract,
                output_contract,
                parsed.output_column.as_deref(),
            )?;
            (
                PreparedConfig::GeoClusterDbscan {
                    eps: parsed.eps,
                    min_points: parsed.min_points,
                    output_column,
                },
                GeoRole::MeasureAddColumn,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(prepared))
}

/// Decodifica e valida strutturalmente un WKB esadecimale da config
/// (secondo operando "unario", convenzione D16): una sola volta in
/// `prepare`, mai nel loop per batch (E1).
fn decode_wkb_hex(node_id: &str, name: &str, hex: &str) -> Result<Geometry<f64>> {
    let invalid = || {
        PlenoraError::Contract(format!(
            "nodo `{node_id}`: {name} non e' WKB esadecimale valido"
        ))
    };
    if !hex.len().is_multiple_of(2) || hex.is_empty() {
        return Err(invalid());
    }
    let bytes: std::result::Result<Vec<u8>, _> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16))
        .collect();
    let bytes = bytes.map_err(|_| invalid())?;
    plenora_kernels_geo::geometry_from_wkb(&bytes).map_err(|error| {
        PlenoraError::Contract(format!(
            "nodo `{node_id}`: {name} non decodificabile: {error}"
        ))
    })
}

/// Nome della colonna prodotta da una misura: la colonna presente nel
/// contratto di output e assente in quello di input (fonte unica di verita':
/// l'inferenza del planner). Se `output_column` e' dichiarato in config deve
/// coincidere (difesa in profondita').
fn measure_output_column(
    node_id: &str,
    input_contract: &DataContract,
    output_contract: &DataContract,
    declared: Option<&str>,
) -> Result<String> {
    let added: Vec<&str> = output_contract
        .schema
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .filter(|name| input_contract.schema.field_with_name(name).is_err())
        .collect();
    let name = match added.as_slice() {
        [single] => *single,
        _ => {
            return Err(PlenoraError::Schema(format!(
                "nodo `{node_id}`: attesa una sola colonna aggiunta dalla misura, \
                 trovate {}",
                added.len()
            )))
        }
    };
    if let Some(declared) = declared {
        if declared != name {
            return Err(PlenoraError::Schema(format!(
                "nodo `{node_id}`: output_column `{declared}` diversa dalla colonna \
                 inferita `{name}`"
            )));
        }
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests;
