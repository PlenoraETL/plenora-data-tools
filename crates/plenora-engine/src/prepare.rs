//! Preparer del DAG — `prepare(&ValidatedGraph, &RuntimeContext) ->
//! ExecutionPlan`.
//!
//! Tre vincoli: hot path minimale, configurazioni preparate e tipizzate
//! prima dell'esecuzione, osservabilita' per nodo anche nei segmenti fusi
//! (architettura.md#planner-ed-executor).
//!
//! Il [`ValidatedGraph`] contiene solo decisioni semantiche stabili; qui si
//! prendono le decisioni fisiche **per questa esecuzione**:
//!
//! - scomposizione del DAG in [`PhysicalSegment`] con [`SegmentMode`]
//!   esplicita: catene massimali di nodi `Streaming` fusi in un unico
//!   segmento (`LinearStreaming`, oppure `GeoFused` se tutti i nodi sono
//!   geo — nella v1 eseguito come `LinearStreaming`, ma la struttura per
//!   kernel [`GeoRole`] e' il punto di aggancio per una cache di decode, vincolo
//!   decode/encode geo minimizzato); ogni nodo `Blocking`/`BinaryBlocking` e' un segmento a se';
//! - [`PreparedKernel`] per ogni nodo: configurazione deserializzata,
//!   tipizzata e gia' rivalidata, indici di colonna e CRS risolti — niente
//!   JSON ne' ricerche per nome nel loop di esecuzione (configurazioni preparate, hot path minimale);
//! - last consumer di ogni arco (rilascio al last consumer) e punti di materializzazione espliciti
//!   (`materialize_output`, materializzazione minima: fan-out, decisione D9);
//! - configurazione delle metriche (osservabilita' per nodo: per nodo logico anche dentro ai
//!   segmenti fusi, e per segmento).
//!
//! Statistiche di runtime (architettura.md#planner-ed-executor): [`RuntimeStatistic::Unknown`] e' il
//! default e impone scelte conservative. Nella v1 seriale le statistiche
//! `Known`/`Estimated` non cambiano ancora nessuna decisione fisica (il
//! parallelismo adattivo non esiste): sono validate, propagate nel piano per
//! osservabilita' e pronte per chi le usera'.
//!
//! Limitazioni v1 (fail-closed in `prepare`, mai a meta' esecuzione): il
//! dispatch copre le trasformazioni geo 1:1 in place, le misure "add
//! column", le estensioni geo v1.1-v1.3 (`from_wkt`,
//! `geometry_accessors`, `collect`, `line_locate_point`, `generate_grid`,
//! `subdivide`, `snap`, `coverage_validate`, `shared_paths`,
//! `cluster_dbscan`) e i quattro binari geo di architettura.md#geometrie
//! (`geo.sjoin`, `geo.nearest`, `geo.within`,
//! `geo.count_points_in_polygons`); le altre op geo — es. `geo.dissolve`,
//! `geo.explode`, predicati, distanze, i binari geo con ri-encode (clip,
//! overlay, booleane pairwise: il ri-encode di D14.1 non e' implementato) — e le op tabellari
//! N-arie con piu' di due input sono rifiutate con `PlenoraError::Unsupported`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use geo::{Geometry, Point};
use serde::Deserialize;

use plenora_core::catalog::{Arity, ExecutionClass, ExpansionConstraint, Family, OperationId};
use plenora_core::contract::{DataContract, RuntimeStatistic};
use plenora_core::limits::Limits;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::extensions::OnWktError;
use plenora_kernels_geo::extensions2::{GridExtent, GridShape};
use plenora_kernels_geo::spatial_join::JoinPredicate;

use crate::cancellation::CancellationToken;
use crate::geo_transport::pair::{validate_pair_parameters, PairOperation, PairParameterValues};
use crate::geo_transport::transport::{
    ArrowOperation, BufferCap, SimplifyPolicyParam, TransformArrowSchema,
};
use crate::plan::NodeV5;
use crate::planner::ValidatedGraph;
use crate::table_engine;

/// Dimensione di batch obiettivo e tetto duro (architettura.md tetto in byte per batch).
///
/// La v1 non ri-pacchettizza i batch in lettura (conservativo): il target
/// e' consultivo per le scelte fisiche future, `max_batch_bytes` e' un
/// limite duro verificato dall'executor su ogni batch che scorre nel piano.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchTarget {
    /// Obiettivo consultivo di byte per batch (tetto in byte per batch).
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

/// Statistiche di runtime di un input (architettura.md#planner-ed-executor).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputStatistics {
    /// Righe totali dell'input (es. da header Arrow IPC file format).
    pub rows: RuntimeStatistic<u64>,
    /// Numero di batch dell'input.
    pub batches: RuntimeStatistic<u64>,
}

/// Contesto runtime di una singola esecuzione (architettura.md).
///
/// Non contiene nulla di semantico: uno stesso `ValidatedGraph` con due
/// `RuntimeContext` diversi produce due `ExecutionPlan` diversi, ed e' il
/// comportamento voluto (architettura.md#planner-ed-executor). Fanno eccezione `cancellation`,
/// `diagnostics` e `temp_root` (errori-e-limiti.md, cancellazione cooperativa ed errori arricchiti): non sono decisioni fisiche
/// e NON entrano nell'`ExecutionPlan` — li consuma direttamente `execute`.
#[derive(Clone, Debug)]
pub struct RuntimeContext {
    /// Statistiche per nome di input; gli input assenti valgono
    /// [`InputStatistics::default`] (tutto `Unknown` → conservativo).
    pub statistics: BTreeMap<String, InputStatistics>,
    /// Grado massimo di parallelismo offerto dall'ambiente. La v1 esegue
    /// sempre seriale (`SerialFused`, parallelismo solo dove conviene): il valore e' registrato nel piano
    /// e lo useranno le strategie parallele, quando esisteranno.
    pub max_parallelism: u32,
    /// Dimensionamento dei batch (tetto in byte per batch).
    pub batch_target: BatchTarget,
    /// Metriche da raccogliere (osservabilita' per nodo).
    pub metrics: MetricsConfig,
    /// Token di cancellazione cooperativa (errori-e-limiti.md#cancellazione):
    /// il default non e'
    /// mai cancellato. Il chiamante (es. l'handler Ctrl-C della CLI) trattiene
    /// un clone del token e lo cancella dall'esterno; l'executor lo osserva
    /// ai confini cooperativi onorando il `CancellationBehavior` di catalogo.
    pub cancellation: CancellationToken,
    /// Modalita' diagnostica opt-in (errori-e-limiti.md, errori arricchiti), solo per input fidati:
    /// gli errori includono contesto strutturale aggiuntivo (indice di
    /// batch, riga, colonna dove disponibile) — MAI valori. Default `false`:
    /// messaggi invariati (retrocompatibile).
    pub diagnostics: bool,
    /// Radice del `TempStore` dell'esecuzione e dello scavenging all'avvio
    /// (errori-e-limiti.md): `None` = temp di sistema. Configurabile per i test e per
    /// ambienti con una temp dedicata.
    pub temp_root: Option<PathBuf>,
    /// Kill switch della fusione dei segmenti geo (architettura.md#geometrie D12.9): con
    /// `true` (default) `prepare` annota i gruppi di nodi geo 1:1 fondibili
    /// e l'executor li esegue con il runner fuso (un decode/encode per
    /// gruppo); con `false` i gruppi non si formano e l'esecuzione e' quella
    /// nodo-per-nodo. Registrato nel piano come le altre opzioni runtime:
    /// serve alla disattivazione operativa, all'oracolo differenziale e ai
    /// benchmark A/B.
    pub geo_fusion: bool,
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
            geo_fusion: true,
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

/// Quali metriche raccogliere durante l'esecuzione (osservabilita' per nodo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricsConfig {
    /// Metriche per nodo logico (righe in/out, batch, wall time) — restano
    /// per nodo anche quando piu' nodi sono fusi in un segmento (osservabilita' per nodo).
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

/// Modalita' fisica di un segmento (architettura.md modalita' fisiche esplicite).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentMode {
    /// Catena di kernel streaming (almeno uno tabellare): batch-per-batch,
    /// senza materializzazione intermedia (streaming reale, segmenti lineari senza code).
    LinearStreaming,
    /// Catena di sole op geo 1:1. I run di nodi fondibili annotati da
    /// `prepare` (campo `fusion_group`, architettura.md#geometrie) sono eseguiti col runner
    /// fuso — un decode/encode per gruppo su ogni batch; il resto e'
    /// eseguito come `LinearStreaming`.
    GeoFused,
    /// Nodo blocking unario: materializza l'input ed esegue una sola volta.
    Blocking,
    /// Nodo blocking binario: materializza left e right ed esegue una sola
    /// volta.
    BinaryBlocking,
}

/// Strategia di parallelismo scelta dal piano (architettura.md parallelismo solo dove conviene).
///
/// La v1 sceglie sempre `SerialFused` per i segmenti streaming e
/// `BlockingSingleTask` per quelli blocking: il parallelismo si attiva solo
/// con benefici misurati.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParallelismStrategy {
    /// Segmento eseguito serialmente, kernel fusi sul singolo batch (segmenti lineari senza code).
    SerialFused,
    /// Parallelismo per batch. Nessun percorso lo sceglie ancora.
    ParallelPerBatch,
    /// Parallelismo per ramo del DAG. Richiede lo scheduler parallelo (M3).
    ParallelPerBranch,
    /// Operazione blocking come task singolo.
    BlockingSingleTask,
}

/// Ruolo di un kernel geo dentro a un segmento `GeoFused`: decode/encode geo
/// minimizzato, con le configurazioni gia' preparate.
///
/// E' il punto di aggancio per una cache di decode: i kernel
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
    /// Op binaria geo che consuma i due input materializzati (segmento
    /// `BinaryBlocking`, architettura.md#geometrie): `sjoin`, `nearest`, `within`,
    /// `count_points_in_polygons`. Mai in un gruppo di fusione.
    BinaryBlocking,
}

/// Misura geo v1 con semantica "aggiungi colonna".
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

/// Piano fisico di un binario geo (architettura.md#geometrie, D14.1/D14.2).
///
/// Perimetro: `geo.sjoin`, `geo.nearest`, `geo.within`,
/// `geo.count_points_in_polygons` — nessuna ri-encode, output via `take`
/// sulle colonne left (sjoin, nearest) o left passthrough + colonna
/// scalare (within, count).
///
/// I parametri sono tipizzati e rivalidati in `prepare` con la stessa
/// tabella per-op del trasporto pair estratta in forma pura
/// (`geo_transport::pair::validate_pair_parameters`, una sola fonte per v3
/// e v4); gli indici delle colonne geometria sono risolti sui due contratti
/// (hot path minimale: nessuna ricerca per nome a runtime); i tetti assoluti D14.6 sono
/// risolti qui dai limiti effettivi del piano — nessuna manopola `max_pairs`
/// da config nodo stile v3.
#[derive(Debug)]
pub struct GeoBinaryPlan {
    /// Operazione binaria (solo le quattro geo del perimetro).
    pub operation: PairOperation,
    /// Predicato del join spaziale (`geo.sjoin`; obbligatorio per tabella).
    pub predicate: Option<JoinPredicate>,
    /// Distanza massima (`geo.nearest`; finita e non negativa se presente).
    pub max_distance: Option<f64>,
    /// Tetto assoluto sulle coppie (D14.6): il tetto righe del piano
    /// applicabile all'arco di output del nodo (`max_output_rows` se il nodo
    /// produce l'output del piano, `max_rows_per_edge` altrimenti).
    pub max_pairs: u64,
    /// Tetto assoluto sui confronti n×m (`geo.nearest`): quadrato del
    /// massimo tra `max_input_rows` e `max_rows_per_edge` — ogni lato e'
    /// coperto da uno dei due limiti, il prodotto per costruzione.
    pub max_comparisons: u64,
    /// Tetto assoluto sui risultati (`geo.nearest`): come `max_pairs`.
    pub max_results: u64,
    /// Indice della colonna geometria nel contratto left.
    pub left_geometry_index: usize,
    /// Indice della colonna geometria nel contratto right.
    pub right_geometry_index: usize,
    /// CRS risolto dell'output (geometria left passthrough; il perimetro non lo
    /// ri-applica — nessuna ri-encode per D14.1 — ma lo registra nel piano
    /// per la diagnostica strutturata, D14.5).
    pub output_crs: String,
}

/// Configurazione preparata di un kernel TABELLARE.
///
/// Due sole forme, entrambe piani legacy di un solo step gia' validati: il
/// dispatch tabellare vive in `table_engine`, non qui.
#[derive(Debug)]
pub enum PreparedTableKernel {
    /// Op tabellare unaria: piano legacy di un solo step gia' validato
    /// (config deserializzata e controllata da `ValidatedPlan`); eseguito
    /// per batch da `table_engine::execute_batch`.
    Unary(Box<table_engine::ValidatedPlan>),
    /// Op tabellare binaria (o `table.concat` a due input): piano legacy di
    /// un solo step binario, eseguito una sola volta da
    /// `table_engine::execute_binary` su input materializzati.
    Binary(Box<table_engine::ValidatedPlan>),
}

/// Configurazione preparata di un kernel GEOMETRICO.
///
/// Tredici forme, perche' le operazioni geo non condividono un dispatch
/// unico: alcune trasformano in place, altre aggiungono colonne, altre
/// riducono. Il `match` che le smista e' esaustivo su QUESTA famiglia — una
/// variante nuova non compila finche' qualcuno non decide che cosa farne.
#[derive(Debug)]
pub enum PreparedGeoKernel {
    /// Op geo binaria di architettura.md#geometrie (senza ri-encode, D14.1):
    /// piano fisico [`GeoBinaryPlan`] con parametri tipizzati rivalidati e
    /// tetti assoluti D14.6 risolti. Eseguita una sola volta dal ramo geo
    /// di `run_binary_blocking` sui due input materializzati.
    Binary(Box<GeoBinaryPlan>),
    /// Trasformazione geo 1:1 in place: parametri tipizzati di
    /// [`TransformArrowSchema`] con CRS e colonna geometria risolti,
    /// `validate_parameters` gia' chiamato. Eseguita per batch da
    /// `geo_transport::transport::transform_batches`.
    Transform(Box<TransformArrowSchema>),
    /// Misura geo con semantica v4 "add column" (il trasporto legacy
    /// sostituirebbe la colonna geometria): dispatch dedicato dell'executor
    /// sui kernel `plenora_kernels_geo::operations`.
    Measure {
        /// Misura da applicare cella per cella.
        measure: MeasureKind,
        /// Nome della colonna prodotta (derivato dal contratto di output).
        output_column: String,
    },
    /// `geo.from_wkt` (streaming 1:1): colonna WKT `Utf8` → nuova colonna
    /// geometria WKB; indice della colonna WKT e politica d'errore risolti
    /// qui. Eseguito per batch su `extensions::from_wkt_column_named`; il
    /// token `on_error=null` resta parsabile ma non autorizza null sintetici.
    FromWkt {
        /// Indice risolto della colonna WKT nel batch di input (hot path minimale).
        wkt_column_index: usize,
        /// Token legacy della politica; ogni cella invalida e' fail-closed.
        on_error: OnWktError,
    },
    /// `geo.geometry_accessors` (streaming 1:1): colonne accessorie scelte,
    /// con i nomi di output risolti dal contratto (prefisso applicato).
    /// Eseguito per batch su `extensions::geometry_accessors`.
    Accessors {
        /// Colonne prodotte: (nome di output, accessore) in ordine canonico.
        columns: Box<[(String, AccessorKind)]>,
    },
    /// `geo.line_locate_point` (streaming 1:1 "add column"): punto di
    /// riferimento decodificato una volta qui (configurazioni preparate), frazione per riga.
    LineLocatePoint {
        /// Punto di riferimento (da `point_wkb` hex, convenzione D16).
        point: Point<f64>,
        /// Nome della colonna prodotta (derivato dal contratto di output).
        output_column: String,
    },
    /// `geo.subdivide` (streaming OneToMany): espansione 1:N per batch con
    /// `__parent_index` di lineage, come `explode`.
    Subdivide {
        /// Soglia di vertici per parte (rivalidato >= 4).
        max_vertices: usize,
    },
    /// `geo.snap` (streaming 1:1 in place): riferimento decodificato una
    /// volta qui (configurazioni preparate), tolleranza rivalidata.
    Snap {
        /// Geometria di riferimento (da `reference_wkb` hex, D16).
        reference: Geometry<f64>,
        /// Distanza massima di aggancio (finita, non negativa).
        tolerance: f64,
    },
    /// `geo.collect` (blocking, ManyToOne): raggruppamento per chiavi
    /// (responsabilita' dell'engine, come `dissolve`) e collezione per
    /// gruppo via `extensions::collect_geometries`.
    Collect {
        /// Indici risolti delle colonne chiave nel batch di input (hot path minimale).
        group_by_indices: Box<[usize]>,
    },
    /// `geo.generate_grid` (blocking, `WholeToMany`, generativa): l'input
    /// funge da trigger; la griglia e' prodotta da
    /// `extensions2::generate_grid_rows` con parametri rivalidati.
    GenerateGrid {
        /// Extent della griglia (finito, non degenere).
        extent: GridExtent,
        /// Lato cella (finito, > 0).
        cell_size: f64,
        /// Forma delle celle (default `square`).
        shape: GridShape,
    },
    /// `geo.coverage_validate` (blocking, WholeToMany): overlap della
    /// copertura via `extensions3::coverage_validate_rows`.
    CoverageValidate {
        /// Area minima di overlap segnalata (default 0).
        tolerance: f64,
        /// Limite di issue (default `DEFAULT_MAX_ISSUES`).
        max_issues: usize,
    },
    /// `geo.shared_paths` (blocking, WholeToMany): confini condivisi via
    /// `extensions3::shared_paths_rows`.
    SharedPaths {
        /// Lunghezza minima del segmento collineare (default 0).
        tolerance: f64,
        /// Lunghezza minima del tratto condiviso (default 0).
        min_length: f64,
    },
    /// `geo.cluster_dbscan` (blocking, output `OneToOne` allineato alle
    /// righe): etichetta `UInt64` nullable per riga via
    /// `cluster::dbscan_column`.
    ClusterDbscan {
        /// Raggio di vicinato (finito, > 0).
        eps: f64,
        /// Punti minimi per un cluster (>= 1).
        min_points: usize,
        /// Nome della colonna prodotta (derivato dal contratto di output).
        output_column: String,
    },
}

/// Configurazione preparata, per famiglia.
///
/// # Perche' due enum e non uno
///
/// Un enum solo con quindici varianti costringerebbe l'executor a smistarle
/// tutte, cioe' a conoscere il tipo di configurazione di OGNI operazione
/// delle due famiglie. Separarle tiene quella conoscenza dentro la famiglia
/// che la possiede, e lascia all'orchestrazione le tre cose che la riguardano
/// davvero — classe di esecuzione, contratto, cancellazione.
#[derive(Debug)]
pub enum PreparedConfig {
    /// Kernel tabellare.
    Table(PreparedTableKernel),
    /// Kernel geometrico.
    Geo(PreparedGeoKernel),
}

impl PreparedTableKernel {
    /// Il piano di questo kernel puo' spillare su disco?
    ///
    /// Domanda dell'orchestratore, risposta della famiglia: chi decide dove
    /// materializzare non ha bisogno di sapere quali forme tabellari
    /// esistono, ne' che la capacita' di spill dipende dal piano legacy.
    #[must_use]
    pub fn unary_spill_capable(&self) -> bool {
        match self {
            Self::Unary(piano) => table_engine::unary_spill_capable(piano),
            Self::Binary(_) => false,
        }
    }

    /// Il piano unario, se questa e' la forma unaria.
    #[must_use]
    pub const fn unary_plan(&self) -> Option<&table_engine::ValidatedPlan> {
        match self {
            Self::Unary(piano) => Some(piano),
            Self::Binary(_) => None,
        }
    }

    /// Il piano binario, se questa e' la forma binaria.
    #[must_use]
    pub const fn binary_plan(&self) -> Option<&table_engine::ValidatedPlan> {
        match self {
            Self::Binary(piano) => Some(piano),
            Self::Unary(_) => None,
        }
    }
}

impl PreparedGeoKernel {
    /// I parametri della trasformazione 1:1, se questa e' una trasformazione.
    ///
    /// Serve alla fusione, che compone piu' trasformazioni consecutive: e'
    /// una domanda sulla forma, non un'ispezione delle varianti.
    #[must_use]
    pub const fn transform_params(&self) -> Option<&TransformArrowSchema> {
        match self {
            Self::Transform(parametri) => Some(parametri),
            _ => None,
        }
    }

    /// La misura da applicare, se questo kernel misura.
    #[must_use]
    pub const fn measure_kind(&self) -> Option<MeasureKind> {
        match self {
            Self::Measure { measure, .. } => Some(*measure),
            _ => None,
        }
    }

    /// Il piano geo binario, se questa e' la forma binaria.
    #[must_use]
    pub const fn binary_plan(&self) -> Option<&GeoBinaryPlan> {
        match self {
            Self::Binary(piano) => Some(piano),
            _ => None,
        }
    }
}

impl PreparedConfig {
    /// Il kernel tabellare, se la famiglia e' quella.
    #[must_use]
    pub const fn table(&self) -> Option<&PreparedTableKernel> {
        match self {
            Self::Table(kernel) => Some(kernel),
            Self::Geo(_) => None,
        }
    }

    /// Il kernel geometrico, se la famiglia e' quella.
    #[must_use]
    pub const fn geo(&self) -> Option<&PreparedGeoKernel> {
        match self {
            Self::Geo(kernel) => Some(kernel),
            Self::Table(_) => None,
        }
    }

    /// Puo' spillare su disco? Falso per tutta la famiglia geo.
    #[must_use]
    pub fn unary_spill_capable(&self) -> bool {
        match self {
            Self::Table(kernel) => kernel.unary_spill_capable(),
            Self::Geo(_) => false,
        }
    }
}

/// Kernel fisico di un segmento (configurazioni preparate, osservabilita' per nodo).
///
/// Mantiene la mappa verso il nodo logico originario (attribuzione errori,
/// metriche per nodo, limiti per arco) e tutto cio' che e' risolvibile prima
/// dell'hot path (hot path minimale).
#[derive(Debug)]
pub struct PreparedKernel {
    /// Id del nodo logico del piano.
    pub node_id: String,
    /// Identita' dell'operazione, tipizzata.
    ///
    /// Tipizzata e non `&'static str`: ogni consumatore che debba ragionarci
    /// sopra dovrebbe altrimenti riconvertirla. La stringa si ottiene con
    /// `as_str()` ed e' cio' che va in serializzazione, metriche ed errori —
    /// non nelle decisioni.
    pub operation: OperationId,
    /// Famiglia dell'operazione.
    pub family: Family,
    /// Ruolo geo dentro a un segmento `GeoFused` (punto di aggancio per una
    /// cache di decode); `None` per i kernel tabellari.
    pub geo_role: Option<GeoRole>,
    /// Indice risolto della colonna geometria attiva nel batch di input
    /// del nodo (hot path minimale: nessuna ricerca per nome a runtime).
    pub geometry_column_index: Option<usize>,
    /// Configurazione preparata.
    pub config: PreparedConfig,
    /// Contratti degli archi di input del nodo (1 o 2).
    pub input_contracts: Vec<DataContract>,
    /// Contratto dell'arco di output del nodo.
    pub output_contract: DataContract,
    /// Comportamento alla cancellazione dichiarato in catalogo (errori-e-limiti.md#cancellazione),
    /// risolto in `prepare` (hot path minimale: nessuno scan del catalogo a runtime).
    pub cancellation_behavior: plenora_core::catalog::CancellationBehavior,
    /// Esenzione da `max_expansion_factor` dichiarata in catalogo (errori-e-limiti.md),
    /// risolta in `prepare` (hot path minimale: nessuno scan del catalogo a runtime).
    pub expansion_factor_exempt: bool,
    /// Vincolo di espansione dichiarato dal catalogo, risolto in preparazione.
    ///
    /// Non risolverlo qui costringerebbe l'executor a rileggerlo dal
    /// catalogo a ogni verifica, con una ricerca lineare su tutti i
    /// descrittori per una proprieta' che non cambia mai.
    pub expansion_constraint: ExpansionConstraint,
    /// Fondibilita' dichiarata in catalogo (architettura.md#geometrie D12.2), risolta in
    /// `prepare` come `cancellation_behavior`.
    pub geo_fusion: plenora_core::catalog::GeoFusion,
    /// Emissione di diagnostica row-scoped dichiarata dall'autorita' di
    /// catalogo per la configurazione del nodo (config-sensitive: stessa
    /// `OperationDescriptor::emits_row_diagnostics` del gate provenance del
    /// planner e del gate legacy CLI), risolta in `prepare` — nessuno scan
    /// del catalogo ne' lista duplicata a runtime.
    pub emits_row_diagnostics: bool,
    /// Gruppo di fusione geo del kernel (architettura.md#geometrie): `Some(id)` per i membri
    /// di un run massimale (>= 2) di kernel `GeoTransform` consecutivi
    /// fondibili (capability `TransformInPlace` di entrambi i nodi adiacenti,
    /// stessa colonna geometria, stesso ruolo), piu' UNA misura terminale
    /// opzionale in coda (capability `TerminalMeasure`, config
    /// `GeoMeasure`); l'id e' condiviso dai membri e apre il gruppo sul
    /// primo. `None` se il kernel non e' in un gruppo o se il kill switch
    /// `RuntimeContext::geo_fusion` e' spento (D12.9).
    pub fusion_group: Option<u32>,
}

/// Segmento fisico dell'`ExecutionPlan` (modalita' fisiche esplicite).
#[derive(Debug)]
pub struct PhysicalSegment {
    /// Id del segmento (`seg0`, `seg1`, ... in ordine topologico).
    pub id: String,
    /// Kernel fusi del segmento (1 per i segmenti blocking).
    pub kernels: Box<[PreparedKernel]>,
    /// Modalita' fisica esplicita.
    pub mode: SegmentMode,
    /// Strategia di parallelismo scelta (v1: seriale ovunque, parallelismo solo dove conviene).
    pub parallelism: ParallelismStrategy,
    /// Archi di input del segmento (1 per streaming/blocking, 2 per
    /// binary-blocking): nomi di input del piano o id di nodi produttori.
    pub input_edges: Box<[String]>,
    /// Arco prodotto dal segmento (id dell'ultimo nodo fuso).
    pub output_edge: String,
    /// Contratto dell'arco di output.
    pub output_contract: DataContract,
    /// Materializzazione esplicitamente prevista dal piano (materializzazione minima, D9): `true`
    /// se l'arco di output ha piu' di un consumatore (fan-out) — in quel
    /// caso l'executor condivide i batch immutabili tra i consumatori
    /// senza copie di buffer.
    pub materialize_output: bool,
}

/// Ultimo consumatore di un arco (rilascio al last consumer): dopo di esso le risorse
/// intermedie dell'arco sono rilasciabili.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LastConsumer {
    /// Un nodo del piano (id).
    Node(String),
    /// L'output del piano.
    Output,
}

/// Decisioni fisiche per una singola esecuzione (architettura.md#planner-ed-executor).
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
    /// Kill switch della fusione geo registrato nel piano (architettura.md#geometrie D12.9):
    /// copia di `RuntimeContext::geo_fusion` per questa esecuzione.
    geo_fusion: bool,
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

    /// Last consumer per arco (rilascio al last consumer).
    #[must_use]
    pub const fn last_consumers(&self) -> &BTreeMap<String, LastConsumer> {
        &self.last_consumers
    }

    /// Arco di output del piano.
    #[must_use]
    pub fn output_edge(&self) -> &str {
        &self.output_edge
    }

    /// Configurazione delle metriche da raccogliere (osservabilita' per nodo).
    #[must_use]
    pub const fn metrics_config(&self) -> MetricsConfig {
        self.metrics_config
    }

    /// Dimensionamento dei batch deciso in `prepare` (tetto in byte per batch).
    #[must_use]
    pub const fn batch_target(&self) -> BatchTarget {
        self.batch_target
    }

    /// Limiti effettivi del piano (da applicare in esecuzione).
    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Statistiche di input registrate nel piano (architettura.md#planner-ed-executor).
    #[must_use]
    pub const fn input_statistics(&self) -> &BTreeMap<String, InputStatistics> {
        &self.input_statistics
    }

    /// Kill switch della fusione geo registrato nel piano (architettura.md#geometrie D12.9).
    #[must_use]
    pub const fn geo_fusion(&self) -> bool {
        self.geo_fusion
    }
}

/// Vista pubblica di sola lettura sulla strategia fisica (dry-run).
///
/// Restituisce l'[`ExecutionPlan`] che `execute` produrrebbe per questo
/// grafo e contesto, **senza eseguire nulla**
/// (architettura.md#planner-ed-executor).
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

/// `prepare` (architettura.md, architettura.md#planner-ed-executor): decisioni fisiche per questa
/// esecuzione a partire dal grafo validato e dal contesto runtime.
///
/// **Interna al crate** (architettura.md#planner-ed-executor): l'API pubblica del motore e' a due passi
/// (`validate` -> `execute`); la strategia fisica e' un dettaglio di
/// implementazione di `execute`. L'unica vista pubblica e' [`explain`],
/// per l'ispezione a secco (dry-run della CLI).
///
/// Funzione pura e a secco: nessuna lettura di dati. Produce sempre un piano
/// valido con statistiche assenti (`Unknown` → conservativo, architettura.md#planner-ed-executor).
///
/// # Errors
///
/// `PlenoraError::Unsupported` per operazioni fuori dal dispatch v1
/// dell'executor (fail-closed qui, non a meta' stream);
/// `PlenoraError::InvalidPlan`/`PlenoraError::Schema` se una configurazione gia'
/// validata semanticamente non supera la rivalidazione fisica (difesa in
/// profondita': non dovrebbe accadere su un `ValidatedGraph` genuino).
///
/// # Panics
///
/// Solo su invarianti interne gia' garantite da `validate` (op
/// risolta, arco inferito, ogni nodo in esattamente un segmento): mai su
/// input esterno.
pub(crate) fn prepare(graph: &ValidatedGraph, runtime: &RuntimeContext) -> Result<ExecutionPlan> {
    let plan = graph.plan().struttura_condivisa();
    let plan = plan.as_ref();
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

    // Kernel preparati per nodo (configurazioni preparate): config tipizzate, indici e CRS risolti.
    let nodes_by_id: HashMap<&str, &NodeV5> = plan
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut kernels_by_id: HashMap<&str, PreparedKernel> = HashMap::with_capacity(plan.nodes.len());
    for node_id in topo {
        let node = nodes_by_id[node_id.as_str()];
        let kernel = prepare_kernel(graph, node, &limits, plan.output == node.id)?;
        kernels_by_id.insert(node.id.as_str(), kernel);
    }

    // Metadati di catalogo per nodo (guidano la scomposizione in segmenti).
    let mut node_meta: HashMap<&str, (ExecutionClass, Arity, Family)> =
        HashMap::with_capacity(plan.nodes.len());
    for node in &plan.nodes {
        let descriptor = plenora_core::catalog::find_operation(&node.op).ok_or_else(|| {
            PlenoraError::Internal(format!("nodo `{}`: op risolta in validazione", node.id))
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
    let (segments, node_segment) = build_segments(
        graph,
        &nodes_by_id,
        &node_meta,
        &fan_out,
        chains,
        &mut kernels_by_id,
        runtime.geo_fusion,
    )?;
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
        geo_fusion: runtime.geo_fusion,
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
                    return Err(PlenoraError::Internal("catena non vuota".to_owned()));
                };
                if fan_out[last] != 1 {
                    break;
                }
                let next = consumers.get(last).and_then(|list| list.first()).copied();
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
    nodes_by_id: &HashMap<&'a str, &'a NodeV5>,
    node_meta: &HashMap<&'a str, (ExecutionClass, Arity, Family)>,
    fan_out: &HashMap<&'a str, usize>,
    chains: Vec<Vec<&'a str>>,
    kernels_by_id: &mut HashMap<&'a str, PreparedKernel>,
    geo_fusion_enabled: bool,
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
            SegmentMode::LinearStreaming | SegmentMode::GeoFused => {
                ParallelismStrategy::SerialFused
            }
            SegmentMode::Blocking | SegmentMode::BinaryBlocking => {
                ParallelismStrategy::BlockingSingleTask
            }
        };

        let index = segments.len();
        for id in &chain {
            node_segment.insert((*id).to_owned(), index);
        }
        let mut kernels: Vec<PreparedKernel> = chain
            .iter()
            .map(|id| {
                kernels_by_id.remove(id).ok_or_else(|| {
                    PlenoraError::Internal(format!(
                        "nodo `{id}`: ogni nodo in esattamente un segmento"
                    ))
                })
            })
            .collect::<Result<_>>()?;
        // Gruppi di fusione geo (architettura.md#geometrie D12.2): solo a kill switch attivo
        // (D12.9); i segmenti blocking hanno un solo kernel, quindi il
        // run massimale e' sempre < 2 e l'annotazione resta vuota.
        if geo_fusion_enabled {
            annotate_fusion_groups(&mut kernels);
        }
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
                    PlenoraError::Internal(format!("arco `{last}` inferito in validazione"))
                })?
                .clone(),
            materialize_output: fan_out[last] > 1,
        });
    }
    Ok((segments, node_segment))
}

/// Annota i gruppi di fusione geo dentro a un segmento (architettura.md#geometrie D12.2):
/// run massimali di almeno due kernel consecutivi fondibili — capability
/// `GeoFusion::TransformInPlace` di ENTRAMBI i nodi adiacenti, ruolo
/// [`GeoRole::TransformInPlace`], config `GeoTransform` e stessa colonna
/// geometria — piu' UNA misura terminale opzionale in coda (capability
/// `GeoFusion::TerminalMeasure`, ruolo [`GeoRole::MeasureAddColumn`], config
/// `GeoMeasure`, stessa colonna). Con la misura in coda basta UN solo
/// transform (gruppo di due nodi); una misura da sola non forma mai gruppo
/// (non c'e' nulla da fondere: resta sul percorso nodo-per-nodo). Ogni
/// gruppo riceve un id progressivo (per segmento) condiviso dai membri;
/// l'executor riconosce l'apertura sul primo membro. I run di un solo
/// transform senza misura non sono annotati: il runner fuso non avrebbe
/// vantaggio e il percorso nodo-per-nodo resta il riferimento.
fn annotate_fusion_groups(kernels: &mut [PreparedKernel]) {
    let fusible_transform = |kernel: &PreparedKernel| {
        kernel.geo_fusion == plenora_core::catalog::GeoFusion::TransformInPlace
            && kernel.geo_role == Some(GeoRole::TransformInPlace)
            && matches!(
                kernel.config,
                PreparedConfig::Geo(PreparedGeoKernel::Transform(_))
            )
            && kernel.geometry_column_index.is_some()
    };
    // Misura terminale: puo' solo CHIUDERE un run di transform, mai
    // aprirlo o proseguirlo (una sola misura per gruppo, D12.2).
    let terminal_measure = |kernel: &PreparedKernel| {
        kernel.geo_fusion == plenora_core::catalog::GeoFusion::TerminalMeasure
            && kernel.geo_role == Some(GeoRole::MeasureAddColumn)
            && matches!(
                kernel.config,
                PreparedConfig::Geo(PreparedGeoKernel::Measure { .. })
            )
            && kernel.geometry_column_index.is_some()
    };
    let mut next_group = 0_u32;
    let mut start = 0_usize;
    while start < kernels.len() {
        if !fusible_transform(&kernels[start]) {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < kernels.len()
            && fusible_transform(&kernels[end])
            && kernels[end].geometry_column_index == kernels[end - 1].geometry_column_index
        {
            end += 1;
        }
        // Una misura `TerminalMeasure` in coda al run, sulla stessa
        // colonna geometria, entra nel gruppo come ultimo membro.
        let terminal = end < kernels.len()
            && terminal_measure(&kernels[end])
            && kernels[end].geometry_column_index == kernels[end - 1].geometry_column_index;
        let group_end = if terminal { end + 1 } else { end };
        if group_end - start >= 2 {
            for kernel in &mut kernels[start..group_end] {
                kernel.fusion_group = Some(next_group);
            }
            next_group += 1;
        }
        start = group_end;
    }
}

/// Last consumer per arco (rilascio al last consumer): l'ultimo nodo consumatore in ordine
/// topologico; l'output del piano e' il consumatore finale del suo arco.
fn compute_last_consumers<'a>(
    plan: &'a crate::plan::PlanV5,
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
/// colonna geometria e CRS risolti (configurazioni preparate, hot path minimale). `is_plan_output` indica se
/// l'arco di output del nodo e' l'output del piano (serve ai tetti assoluti
/// D14.6 dei binari geo: `max_output_rows` vs `max_rows_per_edge`).
fn prepare_kernel(
    graph: &ValidatedGraph,
    node: &NodeV5,
    limits: &Limits,
    is_plan_output: bool,
) -> Result<PreparedKernel> {
    let descriptor = plenora_core::catalog::find_operation(&node.op).ok_or_else(|| {
        PlenoraError::Internal(format!("nodo `{}`: op risolta in validazione", node.id))
    })?;
    let input_contracts: Vec<DataContract> = node
        .inputs
        .iter()
        .map(|edge| {
            graph
                .edge_contract(edge)
                .ok_or_else(|| {
                    PlenoraError::Internal(format!("arco `{edge}` inferito in validazione"))
                })
                .cloned()
        })
        .collect::<Result<_>>()?;
    let output_contract = graph
        .edge_contract(&node.id)
        .ok_or_else(|| {
            PlenoraError::Internal(format!("arco `{}` inferito in validazione", node.id))
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
                    PlenoraError::Internal("colonna geometria nel contratto".to_owned())
                })?
                .0,
        ),
        None => None,
    };

    let limiti_tabellari = limiti_dei_kernel_tabellari(limits)?;
    let (config, geo_role) = match descriptor.family {
        Family::Table => (prepare_table(node, descriptor, &limiti_tabellari)?, None),
        Family::Geo => {
            let (config, role) = prepare_geo(
                node,
                descriptor,
                &input_contracts,
                &output_contract,
                limits,
                is_plan_output,
            )?;
            (config, Some(role))
        }
    };

    Ok(PreparedKernel {
        node_id: node.id.clone(),
        operation: descrittore_tipizzato(descriptor)?,
        family: descriptor.family,
        geo_role,
        geometry_column_index,
        config,
        input_contracts,
        output_contract,
        cancellation_behavior: descriptor.cancellation_behavior,
        expansion_factor_exempt: descriptor.expansion_factor_exempt,
        expansion_constraint: descriptor.expansion_constraint,
        geo_fusion: descriptor.geo_fusion,
        emits_row_diagnostics: descriptor.emits_row_diagnostics(&node.config),
        fusion_group: None,
    })
}

/// Limiti del motore tabellare legacy derivati dai limiti effettivi del
/// piano (i campi senza corrispettivo restano ai default legacy).
///
/// Mapping documentato (semantica errori-e-limiti.md): il `max_rows` legacy e' un limite
/// **per batch/tabella** del motore a tabella intera; qui lo si ancora a
/// `max_input_rows` (tetto conservativo sulla tabella materializzata dai
/// nodi blocking). I limiti per arco (`max_rows_per_edge`) e di espansione
/// restano applicati dall'executor con la semantica cumulativa corretta —
/// mapparli sul `max_rows` legacy li farebbe scattare per batch, non per
/// arco.
fn limiti_dei_kernel_tabellari(limits: &Limits) -> Result<table_engine::Limits> {
    // Le conversioni verso `usize` sono fail-closed. Con `unwrap_or(usize::MAX)`,
    // su una piattaforma dove `usize` e' piu' stretto di `u64`, un budget che
    // non ci sta diventerebbe il massimo rappresentabile — cioe' un tetto di
    // sicurezza ALLARGATO in silenzio, nella direzione sbagliata. Un limite che non si puo' onorare e' una configurazione da
    // rifiutare, non da arrotondare.
    let stretto = |valore: u64, nome: &str| -> Result<usize> {
        usize::try_from(valore).map_err(|_| {
            PlenoraError::ResourceLimit(format!(
                "{nome} dichiarato oltre quanto questa piattaforma sa rappresentare"
            ))
        })
    };
    Ok(table_engine::Limits {
        max_rows: stretto(limits.rows.max_input_rows, "max_input_rows")?,
        // NON derivati dal piano: sono limiti interni dei kernel, dichiarati
        // dove sono imposti (`plenora_kernels_table::limiti_interni`).
        // Prendendoli da `Limits::default()` qui dentro sembrerebbero
        // ereditati dal piano.
        max_columns: plenora_kernels_table::limiti_interni::MAX_COLUMNS,
        max_split_columns: plenora_kernels_table::limiti_interni::MAX_SPLIT_COLUMNS,
        max_string_bytes: limits.max_string_bytes,
        max_regex_bytes: limits.max_regex_bytes,
        max_governed_memory_bytes: stretto(
            limits.max_governed_memory_bytes,
            "max_governed_memory_bytes",
        )?,
        max_temp_bytes: limits.max_temp_bytes,
        // Nessuna correzione qui: il minimo e' imposto da `Limits::validate`
        // all'ingresso del planner, che RIFIUTA un valore fuori dominio invece
        // di modificarlo alle spalle di chi ha scritto il piano.
        spill_partitions: stretto(u64::from(limits.spill_partitions), "spill_partitions")?,
    })
}

/// Kernel tabellare: piano legacy di un solo step, validato in `prepare`
/// (config deserializzata e controllata qui, non nel loop per batch).
fn prepare_table(
    node: &NodeV5,
    descriptor: &plenora_core::catalog::OperationDescriptor,
    limiti_tabellari: &table_engine::Limits,
) -> Result<PreparedConfig> {
    let step = table_engine::Step {
        operation: descriptor.id.to_owned(),
        config: node.config.clone(),
    };
    let plan = table_engine::Plan {
        schema_version: table_engine::SCHEMA_VERSION,
        limits: limiti_tabellari.clone(),
        steps: vec![step],
    };
    let validated = plan.validate().map_err(|error| {
        PlenoraError::InvalidPlan(format!(
            "nodo `{}`: rivalidazione fisica della config fallita: {error}",
            node.id
        ))
    })?;
    match descriptor.arity {
        plenora_core::catalog::Arity::Unary => Ok(PreparedConfig::Table(
            PreparedTableKernel::Unary(Box::new(validated)),
        )),
        plenora_core::catalog::Arity::BinaryOrdered => Ok(PreparedConfig::Table(
            PreparedTableKernel::Binary(Box::new(validated)),
        )),
        plenora_core::catalog::Arity::NAry => {
            if node.inputs.len() == 2 {
                // `table.concat` a due input usa il dispatch binario legacy.
                Ok(PreparedConfig::Table(PreparedTableKernel::Binary(
                    Box::new(validated),
                )))
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

/// Config serde di `geo.sjoin` (v4): solo il predicato — i tetti sono D14.6
/// (limiti del piano), mai `max_pairs` da config nodo stile v3.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeoSJoinConfig {
    predicate: JoinPredicate,
}

/// Config serde di `geo.nearest` (v4): solo la distanza massima opzionale.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeoNearestConfig {
    max_distance: Option<f64>,
}

/// Config serde di `geo.within` / `geo.count_points_in_polygons` (v4): il
/// nome della colonna prodotta e' semantica di contratto (gia' applicata
/// dal planner nell'inferenza).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeoBinaryOutputColumnConfig {
    output_column: Option<String>,
}

/// Binari geo di architettura.md#geometrie (D14.1: nessuna ri-encode).
/// `None` se l'op non e' nel perimetro (clip, overlay, booleane pairwise:
/// richiedono il ri-encode, e restano `Unsupported`).
///
/// D14.6: il tetto assoluto per-op NON e' una manopola
/// di nodo ne' un campo di catalogo — e' il tetto righe del piano gia' in
/// vigore sull'arco di output del nodo (`max_output_rows` se il nodo produce
/// l'output del piano, `max_rows_per_edge` altrimenti), passato al kernel
/// come `max_pairs`/`max_results` (rifiuto durante il calcolo, prima della
/// materializzazione completa delle coppie) e riverificato post-hoc dai
/// check esistenti (`check_join_expansion` errori-e-limiti.md + conteggi per arco/output).
/// Una sola fonte: i limiti effettivi del piano. Per i confronti n×m di
/// `nearest` (lavoro, non espansione) il tetto e' il quadrato del massimo
/// tra `max_input_rows` e `max_rows_per_edge`: ogni lato e' coperto da uno
/// dei due, il prodotto per costruzione.
///
/// La rivalidazione passa per la tabella per-op del trasporto pair estratta
/// in forma pura (D14.2): i parametri del nodo (predicato, `max_distance`) e
/// i tetti risolti sono verificati insieme, con le stesse regole di dominio
/// del v3 — incluso il tetto del protocollo coppie (`MAX_PAIRS`): un piano
/// con limiti di righe oltre quel tetto e' rifiutato qui, fail-closed.
// La lunghezza e' data dalla sequenza lineare dei casi per op (config
// tipizzata + vista parametri) e dalla risoluzione documentata dei tetti
// D14.6, non da complessita' logica.
#[allow(clippy::too_many_lines)]
fn prepare_geo_binary(
    node: &NodeV5,
    descriptor: &plenora_core::catalog::OperationDescriptor,
    input_contracts: &[DataContract],
    output_contract: &DataContract,
    limits: &Limits,
    is_plan_output: bool,
) -> Result<Option<(PreparedConfig, GeoRole)>> {
    let operation = match descrittore_tipizzato(descriptor)? {
        OperationId::GeoSjoin => PairOperation::SJoin,
        OperationId::GeoNearest => PairOperation::Nearest,
        OperationId::GeoWithin => PairOperation::Within,
        OperationId::GeoCountPointsInPolygons => PairOperation::CountPointsInPolygons,
        _ => return Ok(None),
    };
    let row_cap = if is_plan_output {
        limits.rows.max_output_rows
    } else {
        limits.rows.max_rows_per_edge
    };
    let input_ceiling = limits
        .rows
        .max_input_rows
        .max(limits.rows.max_rows_per_edge);
    let max_comparisons = input_ceiling.saturating_mul(input_ceiling);
    let (predicate, max_distance, values) = match operation {
        PairOperation::SJoin => {
            let parsed: GeoSJoinConfig = serde_json::from_value(node.config.clone())?;
            (
                Some(parsed.predicate),
                None,
                PairParameterValues {
                    predicate: Some(parsed.predicate),
                    max_pairs: Some(row_cap),
                    ..PairParameterValues::default()
                },
            )
        }
        PairOperation::Nearest => {
            let parsed: GeoNearestConfig = serde_json::from_value(node.config.clone())?;
            (
                None,
                parsed.max_distance,
                PairParameterValues {
                    max_comparisons: Some(max_comparisons),
                    max_results: Some(row_cap),
                    max_distance: parsed.max_distance,
                    ..PairParameterValues::default()
                },
            )
        }
        PairOperation::Within | PairOperation::CountPointsInPolygons => {
            let parsed: GeoBinaryOutputColumnConfig = serde_json::from_value(node.config.clone())?;
            // `output_column` e' semantica di contratto: gia' applicata dal
            // planner, niente da risolvere a runtime (lo schema di output
            // e' quello del contratto, fonte unica di verita').
            let _ = &parsed.output_column;
            (
                None,
                None,
                PairParameterValues {
                    max_pairs: Some(row_cap),
                    ..PairParameterValues::default()
                },
            )
        }
        _ => {
            return Err(PlenoraError::Internal(format!(
                "nodo `{}`: op binaria geo fuori perimetro nel dispatch M1",
                node.id
            )))
        }
    };
    validate_pair_parameters(operation, &values).map_err(|error| {
        PlenoraError::InvalidPlan(format!(
            "nodo `{}`: rivalidazione fisica dei parametri fallita: {error}",
            node.id
        ))
    })?;
    // Indici delle colonne geometria sui due contratti (hot path minimale): risolti qui,
    // mai per nome a runtime. L'identificabilita' e' garantita da analyze
    // (piano-v5.md#contratti-di-input decisione 8, entrambi gli operandi).
    let geometry_index = |contract: &DataContract, side: &'static str| -> Result<usize> {
        let geometry = contract.active_geometry_column().ok_or_else(|| {
            PlenoraError::Internal(format!(
                "geometria attiva lato {side} verificata in validazione"
            ))
        })?;
        Ok(contract
            .schema
            .column_with_name(&geometry.name)
            .ok_or_else(|| PlenoraError::Internal("colonna geometria nel contratto".to_owned()))?
            .0)
    };
    let left_geometry_index = geometry_index(&input_contracts[0], "left")?;
    let right_contract = input_contracts.get(1).ok_or_else(|| {
        PlenoraError::Internal(format!(
            "nodo `{}`: binario geo con un solo contratto di input",
            node.id
        ))
    })?;
    let right_geometry_index = geometry_index(right_contract, "right")?;
    // CRS di output = CRS left (geometria passthrough, requisito di
    // catalogo `SameProjected` gia' verificato da analyze su entrambi).
    let output_geometry = output_contract.active_geometry_column().ok_or_else(|| {
        PlenoraError::Internal("binario geo senza geometria attiva in output".to_owned())
    })?;
    let output_crs = output_geometry
        .crs
        .as_resolved()
        .ok_or_else(|| {
            PlenoraError::Internal("binario geo senza CRS risolto dopo la validazione".to_owned())
        })?
        .definition()
        .to_owned();
    Ok(Some((
        PreparedConfig::Geo(PreparedGeoKernel::Binary(Box::new(GeoBinaryPlan {
            operation,
            predicate,
            max_distance,
            max_pairs: row_cap,
            max_comparisons,
            max_results: row_cap,
            left_geometry_index,
            right_geometry_index,
            output_crs,
        }))),
        GeoRole::BinaryBlocking,
    )))
}

/// L'identita' tipizzata di un descrittore del catalogo.
///
/// Il descrittore VIENE dal catalogo, quindi la conversione non puo' fallire:
/// il test di bijezione in `plenora-core` lo garantisce. `Result` invece di
/// `expect` perche' il gate R6 vieta le primitive di panico nel codice di
/// produzione, e perche' un'invariante violata deve diventare un errore
/// diagnosticabile, non un abort.
fn descrittore_tipizzato(
    descriptor: &plenora_core::catalog::OperationDescriptor,
) -> Result<OperationId> {
    OperationId::from_canonical(descriptor.id).ok_or_else(|| {
        PlenoraError::Internal(format!(
            "operazione `{}` a catalogo senza variante in OperationId",
            descriptor.id
        ))
    })
}

/// Mapping op geo v4 → [`ArrowOperation`] del trasporto (trasformazioni 1:1
/// in place coperte dal dispatch v1).
fn geo_transform_operation(id: &str) -> Option<ArrowOperation> {
    match OperationId::from_canonical(id)? {
        OperationId::GeoCentroid => Some(ArrowOperation::Centroid),
        OperationId::GeoConvexHull => Some(ArrowOperation::ConvexHull),
        OperationId::GeoEnvelope => Some(ArrowOperation::Envelope),
        OperationId::GeoBuffer => Some(ArrowOperation::Buffer),
        OperationId::GeoSimplify => Some(ArrowOperation::Simplify),
        OperationId::GeoBoundary => Some(ArrowOperation::Boundary),
        OperationId::GeoPointOnSurface => Some(ArrowOperation::PointOnSurface),
        OperationId::GeoMakeValid => Some(ArrowOperation::MakeValid),
        OperationId::GeoReproject => Some(ArrowOperation::Reproject),
        OperationId::GeoAffineTransform => Some(ArrowOperation::AffineTransform),
        OperationId::GeoTranslate => Some(ArrowOperation::Translate),
        OperationId::GeoScale => Some(ArrowOperation::Scale),
        OperationId::GeoRotate => Some(ArrowOperation::Rotate),
        OperationId::GeoConcaveHull => Some(ArrowOperation::ConcaveHull),
        OperationId::GeoDensify => Some(ArrowOperation::Densify),
        OperationId::GeoSnapToGrid => Some(ArrowOperation::SnapToGrid),
        OperationId::GeoLineSubstring => Some(ArrowOperation::LineSubstring),
        OperationId::GeoLineInterpolatePoint => Some(ArrowOperation::LineInterpolatePoint),
        _ => None,
    }
}

/// Kernel geo: trasformazioni 1:1 in place via `transform_batches`, misure
/// "add column" via dispatch dedicato, binari geo di architettura.md#geometrie via
/// [`prepare_geo_binary`]; il resto e' fuori dal dispatch v1.
///
/// `input_contracts` sono i contratti degli archi di input del nodo (1 per
/// le unarie, 2 per i binari); `limits` e `is_plan_output` servono solo al
/// braccio binario (tetti assoluti D14.6).
// La lunghezza e' data dalla sequenza lineare dei bracci di dispatch
// (trasformazioni, misure, binari geo, estensioni), non da complessita'
// logica.
#[allow(clippy::too_many_lines)]
fn prepare_geo(
    node: &NodeV5,
    descriptor: &plenora_core::catalog::OperationDescriptor,
    input_contracts: &[DataContract],
    output_contract: &DataContract,
    limits: &Limits,
    is_plan_output: bool,
) -> Result<(PreparedConfig, GeoRole)> {
    let input_contract = &input_contracts[0];
    if let Some(operation) = geo_transform_operation(descriptor.id) {
        let parsed: GeoTransformConfig = serde_json::from_value(node.config.clone())?;
        let geometry = input_contract.active_geometry_column().ok_or_else(|| {
            PlenoraError::Internal("geometria attiva verificata in validazione".to_owned())
        })?;
        // Invariante di validazione: ogni trasformazione geo dichiara un
        // `CrsRequirement` e il gate R4.6.3 dell'analyze ferma un CRS non
        // risolto (`Missing` o `DeclaredUnresolved`) a compile-plan — qui
        // il CRS e' sempre risolto.
        let crs = geometry.crs.as_resolved().ok_or_else(|| {
            PlenoraError::Internal(
                "trasformazione geo senza CRS risolto dopo la validazione".to_owned(),
            )
        })?;
        let params = TransformArrowSchema {
            schema_version: TransformArrowSchema::VERSION,
            operation,
            row_count: 0,
            crs: Some(crs.definition().to_owned()),
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
            PlenoraError::InvalidPlan(format!(
                "nodo `{}`: rivalidazione fisica dei parametri fallita: {error}",
                node.id
            ))
        })?;
        return Ok((
            PreparedConfig::Geo(PreparedGeoKernel::Transform(Box::new(params))),
            GeoRole::TransformInPlace,
        ));
    }

    let measure = match descrittore_tipizzato(descriptor)? {
        OperationId::GeoArea => Some(MeasureKind::Area),
        OperationId::GeoLength => Some(MeasureKind::Length),
        OperationId::GeoPerimeter => Some(MeasureKind::Perimeter),
        OperationId::GeoVertexCount => Some(MeasureKind::VertexCount),
        OperationId::GeoToWkt => Some(MeasureKind::ToWkt),
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
            PreparedConfig::Geo(PreparedGeoKernel::Measure {
                measure,
                output_column,
            }),
            GeoRole::MeasureAddColumn,
        ));
    }

    if let Some(prepared) = prepare_geo_binary(
        node,
        descriptor,
        input_contracts,
        output_contract,
        limits,
        is_plan_output,
    )? {
        return Ok(prepared);
    }

    if let Some(prepared) =
        prepare_geo_extension(node, descriptor, input_contract, output_contract)?
    {
        return Ok(prepared);
    }

    Err(PlenoraError::Unsupported(format!(
        "nodo `{}`: {} non e' nel dispatch v1 dell'executor (Fase 2A-4): \
         coperte le trasformazioni geo 1:1 in place, le misure area/length/\
         perimeter/vertex_count/to_wkt, le estensioni v1.1-v1.3 (from_wkt, \
         geometry_accessors, collect, line_locate_point, generate_grid, \
         subdivide, snap, coverage_validate, shared_paths, cluster_dbscan) e \
         i binari del perimetro architettura.md#geometrie M1 (sjoin, nearest, within, \
         count_points_in_polygons); il resto e' Fase 2B/2C (clip, overlay e \
         booleane pairwise al secondo cantiere D14.1)",
        node.id, descriptor.id
    )))
}

/// Estensioni geo v1.1-v1.3: config tipizzate e rivalidate (configurazioni preparate), secondo
/// operando da config decodificato una volta qui (mai nel loop per batch).
/// `None` se l'op non e' un'estensione coperta.
// Dispatcher esaustivo sulle estensioni v1.1-v1.3: la lunghezza e' data
// dalla sequenza lineare dei casi (config tipizzata + validazione per op),
// non da complessita' logica, e spezzarla in funzioni artificiali
// peggiorerebbe solo la leggibilita'.
#[allow(clippy::too_many_lines)]
fn prepare_geo_extension(
    node: &NodeV5,
    descriptor: &plenora_core::catalog::OperationDescriptor,
    input_contract: &DataContract,
    output_contract: &DataContract,
) -> Result<Option<(PreparedConfig, GeoRole)>> {
    let prepared = match descrittore_tipizzato(descriptor)? {
        OperationId::GeoFromWkt => {
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
                PreparedConfig::Geo(PreparedGeoKernel::FromWkt {
                    wkt_column_index,
                    on_error: parsed.on_error.unwrap_or(OnWktError::Null),
                }),
                GeoRole::ProduceFromText,
            )
        }
        OperationId::GeoGeometryAccessors => {
            let parsed: GeoAccessorsConfig = serde_json::from_value(node.config.clone())?;
            let prefix = parsed.output_prefix.as_deref().unwrap_or("");
            let selected: Vec<AccessorKind> = match &parsed.fields {
                None => AccessorKind::ALL.to_vec(),
                Some(names) => names
                    .iter()
                    .map(|name| {
                        AccessorKind::from_canonical_name(name).ok_or_else(|| {
                            PlenoraError::InvalidPlan(format!(
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
                PreparedConfig::Geo(PreparedGeoKernel::Accessors {
                    columns: columns.into_boxed_slice(),
                }),
                GeoRole::MeasureAddColumn,
            )
        }
        OperationId::GeoLineLocatePoint => {
            let parsed: GeoLineLocatePointConfig = serde_json::from_value(node.config.clone())?;
            let Geometry::Point(point) = decode_wkb_hex(&node.id, "point_wkb", &parsed.point_wkb)?
            else {
                return Err(PlenoraError::InvalidPlan(format!(
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
                PreparedConfig::Geo(PreparedGeoKernel::LineLocatePoint {
                    point,
                    output_column,
                }),
                GeoRole::MeasureAddColumn,
            )
        }
        OperationId::GeoSubdivide => {
            let parsed: GeoSubdivideConfig = serde_json::from_value(node.config.clone())?;
            // `output_column` e' semantica di contratto (rinomina in place):
            // gia' applicata dal planner, niente da fare a runtime.
            let _ = &parsed.output_column;
            if parsed.max_vertices < plenora_kernels_geo::extensions2::MIN_SUBDIVIDE_VERTICES {
                return Err(PlenoraError::InvalidPlan(format!(
                    "nodo `{}`: max_vertices deve essere almeno 4 (anello chiuso minimo)",
                    node.id
                )));
            }
            (
                PreparedConfig::Geo(PreparedGeoKernel::Subdivide {
                    max_vertices: parsed.max_vertices,
                }),
                GeoRole::OneToMany,
            )
        }
        OperationId::GeoSnap => {
            let parsed: GeoSnapConfig = serde_json::from_value(node.config.clone())?;
            let reference = decode_wkb_hex(&node.id, "reference_wkb", &parsed.reference_wkb)?;
            if !parsed.tolerance.is_finite() || parsed.tolerance < 0.0 {
                return Err(PlenoraError::InvalidPlan(format!(
                    "nodo `{}`: tolerance deve essere finita e non negativa",
                    node.id
                )));
            }
            (
                PreparedConfig::Geo(PreparedGeoKernel::Snap {
                    reference,
                    tolerance: parsed.tolerance,
                }),
                GeoRole::TransformInPlace,
            )
        }
        OperationId::GeoCollect => {
            let parsed: GeoCollectConfig = serde_json::from_value(node.config.clone())?;
            let mut indices: Vec<usize> = Vec::with_capacity(parsed.group_by.len());
            for name in &parsed.group_by {
                let (index, _) = input_contract
                    .schema
                    .column_with_name(name)
                    .ok_or_else(|| {
                        PlenoraError::Schema(format!(
                            "nodo `{}`: colonna chiave `{name}` assente dal contratto di input",
                            node.id
                        ))
                    })?;
                indices.push(index);
            }
            (
                PreparedConfig::Geo(PreparedGeoKernel::Collect {
                    group_by_indices: indices.into_boxed_slice(),
                }),
                GeoRole::WholeTable,
            )
        }
        OperationId::GeoGenerateGrid => {
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
            .map_err(|error| PlenoraError::InvalidPlan(format!("nodo `{}`: {error}", node.id)))?;
            let shape = parsed.shape.unwrap_or(GridShape::Square);
            // Rivalidazione fisica: cell_size e limite celle (configurazioni preparate).
            plenora_kernels_geo::extensions2::grid_cell_count(&extent, parsed.cell_size, shape)
                .map_err(|error| {
                    PlenoraError::InvalidPlan(format!("nodo `{}`: {error}", node.id))
                })?;
            (
                PreparedConfig::Geo(PreparedGeoKernel::GenerateGrid {
                    extent,
                    cell_size: parsed.cell_size,
                    shape,
                }),
                GeoRole::WholeTable,
            )
        }
        OperationId::GeoCoverageValidate => {
            let parsed: GeoCoverageValidateConfig = serde_json::from_value(node.config.clone())?;
            let tolerance = parsed.tolerance.unwrap_or(0.0);
            if !tolerance.is_finite() || tolerance < 0.0 {
                return Err(PlenoraError::InvalidPlan(format!(
                    "nodo `{}`: tolerance deve essere finita e non negativa",
                    node.id
                )));
            }
            (
                PreparedConfig::Geo(PreparedGeoKernel::CoverageValidate {
                    tolerance,
                    max_issues: parsed
                        .max_issues
                        .unwrap_or(plenora_kernels_geo::extensions3::DEFAULT_MAX_ISSUES),
                }),
                GeoRole::WholeTable,
            )
        }
        OperationId::GeoSharedPaths => {
            let parsed: GeoSharedPathsConfig = serde_json::from_value(node.config.clone())?;
            let tolerance = parsed.tolerance.unwrap_or(0.0);
            let min_length = parsed.min_length.unwrap_or(0.0);
            for (name, value) in [("tolerance", tolerance), ("min_length", min_length)] {
                if !value.is_finite() || value < 0.0 {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "nodo `{}`: {name} deve essere finita e non negativa",
                        node.id
                    )));
                }
            }
            (
                PreparedConfig::Geo(PreparedGeoKernel::SharedPaths {
                    tolerance,
                    min_length,
                }),
                GeoRole::WholeTable,
            )
        }
        OperationId::GeoClusterDbscan => {
            let parsed: GeoClusterDbscanConfig = serde_json::from_value(node.config.clone())?;
            if !parsed.eps.is_finite() || parsed.eps <= 0.0 {
                return Err(PlenoraError::InvalidPlan(format!(
                    "nodo `{}`: eps deve essere finito e maggiore di zero",
                    node.id
                )));
            }
            if parsed.min_points < 1 {
                return Err(PlenoraError::InvalidPlan(format!(
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
                PreparedConfig::Geo(PreparedGeoKernel::ClusterDbscan {
                    eps: parsed.eps,
                    min_points: parsed.min_points,
                    output_column,
                }),
                GeoRole::MeasureAddColumn,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(prepared))
}

/// Decodifica e valida strutturalmente un WKB esadecimale da config
/// (secondo operando "unario", convenzione D16): una sola volta in
/// `prepare`, mai nel loop per batch (configurazioni preparate).
fn decode_wkb_hex(node_id: &str, name: &str, hex: &str) -> Result<Geometry<f64>> {
    let invalid = || {
        PlenoraError::InvalidPlan(format!(
            "nodo `{node_id}`: {name} non e' WKB esadecimale valido"
        ))
    };
    // Stesso panic UTF-8 dei kernel geo, stessa cura: la decodifica e' una
    // sola (`wkb_hex_to_bytes`), sui byte, e qui si traduce solo l'esito.
    let bytes = plenora_kernels_geo::wkb_hex_to_bytes(hex).ok_or_else(invalid)?;
    plenora_kernels_geo::geometry_from_wkb(&bytes).map_err(|error| {
        PlenoraError::InvalidPlan(format!(
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
