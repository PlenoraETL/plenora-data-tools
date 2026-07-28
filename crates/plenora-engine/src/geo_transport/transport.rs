//! Trasporto Arrow v3: envelope checksummed con payload Arrow IPC e
//! geometrie GeoArrow-WKB.
//!
//! Layout dell'envelope `PLNGEO3`:
//!
//! ```text
//! offset 0   magic "PLNGEO3\0"          (8 byte)
//! offset 8   payload_len uint64 LE      (8 byte)
//! offset 16  payload Arrow IPC stream   (payload_len byte)
//! ...        trailer "GEOEND3\0"        (8 byte)
//! ...        SHA-256(magic || len || payload) (32 byte)
//! ...        EOF: byte residui rifiutati
//! ```
//!
//! Le geometrie viaggiano in una colonna `Binary` con metadati di estensione
//! `GeoArrow` (`ARROW:extension:name` = `geoarrow.wkb`) e metadato `geo` JSON
//! con la chiave `crs`. Ogni cella non-null viene validata con il validatore
//! WKB del kernel; i null sono preservati. Il modulo e' puro I/O su
//! `Read`/`Write`: la verifica semantica del CRS e la pubblicazione atomica
//! restano nel livello comandi.
//!
//! Operazioni 1:1: `centroid`, `convex_hull`, `envelope`, `buffer`,
//! `simplify`, `boundary`, `point_on_surface`, `make_valid` (richiede
//! `geos-backend`) e `reproject` (richiede `proj-backend`) producono una
//! colonna geometria GeoArrow-WKB; `area`, `length`, `perimeter` producono
//! Float64, `vertex_count` `UInt64`, `bounds` quattro colonne Float64
//! `<geometry_column>_minx/miny/maxx/maxy`, `to_wkt` Utf8.

use std::io::{Read, Write};

use plenora_core::arrow::array::{
    Array, BinaryArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::StreamWriter;
use geo::{CoordsIter, Geometry};
use geozero::{wkb::Wkb, CoordDimensions, ToGeo, ToWkb};
use rayon::prelude::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use plenora_kernels_geo::advanced::{voronoi_cells, AdvancedError};
use plenora_kernels_geo::analysis::{
    count_points_in_polygons, minimum_distances, nearest_matches, within_indexes, AnalysisError,
};
use plenora_kernels_geo::construction::{
    line_from_ordered_points, point_from_lon_lat, polygon_from_ordered_points, ConstructionError,
};
use plenora_core::crs::MAX_CRS_DEFINITION_BYTES;
use plenora_core::contract::GeometryDimensions;
use plenora_kernels_geo::extended::{
    affine_transform, concave_hull, geodesic_distance_m, geodesic_line_length_m,
    hausdorff_distance, haversine_distance_m, rotate_about, scale_about, translate, ExtendedError,
};
#[cfg(feature = "geos-backend")]
use plenora_kernels_geo::extended_algorithms::split_line;
use plenora_kernels_geo::extended_algorithms::{
    delaunay, densify, frechet_distance, geodesic_area_m2, geodesic_bearing_degrees,
    geometry_diagnostics, line_interpolate_point, line_merge, line_substring, snap_to_grid,
    ExtendedAlgorithmError,
};
#[cfg(feature = "geos-backend")]
use plenora_kernels_geo::geos_backend::{
    make_valid_wkb, polygonize_linework, split_polygon_by_linework, GeosBackendError, RepairMethod,
};
use plenora_kernels_geo::operations::{
    area, boundary, bounds, buffer_with_cap, explode, length, perimeter, point_on_surface,
    simplify_with_policy, to_wkt, vertex_count, BufferCapStyle, OperationError, SimplifyPolicy,
};
use super::pair_protocol::MAX_PAIRS;
use plenora_kernels_geo::predicates::{evaluate as evaluate_predicate, PredicateError, SpatialPredicate};
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::proj_backend::{ProjBackendError, Reprojector};
use super::protocol::{MAX_ROWS, MAX_STREAM_BYTES};
use plenora_kernels_geo::spatial_join::{spatial_join_nullable, JoinPredicate, SpatialJoinError};
use plenora_kernels_geo::topology::{
    boolean_operation, clean_valid_polygon_topology, clip_to_mask, dissolve, polygon_overlay,
    BooleanOperation, OverlayMode, TopologyError,
};
use plenora_core::PlenoraError;
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, Operation};

pub const ENVELOPE_MAGIC: &[u8; 8] = b"PLNGEO3\0";
pub const ENVELOPE_TRAILER_MAGIC: &[u8; 8] = b"GEOEND3\0";
// Costanti dei metadati GeoArrow: casa unica in `arrow_adapter`
// (unificazione B1.1), qui ri-esportate per compatibilita' di percorso.
pub use plenora_kernels_geo::arrow_adapter::{
    DEFAULT_GEOMETRY_COLUMN, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
};
pub const DEFAULT_X_COLUMN: &str = "x";
pub const DEFAULT_Y_COLUMN: &str = "y";
pub const PARENT_INDEX_COLUMN: &str = "__parent_index";
pub const LEFT_INDEX_COLUMN: &str = "__left_index";
pub const RIGHT_INDEX_COLUMN: &str = "__right_index";
pub const DISTANCE_COLUMN: &str = "distance";
pub const WITHIN_COLUMN: &str = "within";
pub const COUNT_COLUMN: &str = "count";
pub const CLASS_COLUMN: &str = "__class";
/// Lavoro massimo di noding GEOS per `polygonize` e `split` poligonale.
pub const MAX_NODING_WORK: u64 = 100_000_000;
/// Test di intersezione massimi per `split` lineare.
pub const MAX_SPLIT_WORK: u64 = 100_000_000;

#[cfg(feature = "proj-backend")]
std::thread_local! {
    /// Pipeline PROJ per thread: `Reprojector` non e' `Sync`, quindi ogni
    /// thread rayon costruisce la sua una sola volta per coppia CRS.
    static REPROJECTOR: std::cell::RefCell<Option<(String, String, Reprojector)>> =
        const { std::cell::RefCell::new(None) };
}
/// Vertici totali massimi elaborati da `clean_topology` sull'intera tabella.
pub const MAX_CLEAN_VERTICES: u64 = 100_000_000;
/// Metadati massimi di un singolo messaggio Arrow IPC (schema compreso).
pub const MAX_IPC_METADATA_BYTES: usize = 16 * 1024 * 1024;
pub const AREA_COLUMN: &str = "area";
pub const WKT_COLUMN: &str = "wkt";
pub const DEFAULT_MAX_POINTS: u64 = 100_000;
pub const MAX_COLUMNS: usize = 1024;
pub const MAX_BATCHES: usize = 65_536;
pub const MAX_CELL_BYTES: u64 = 64 * 1024 * 1024;
/// Coordinate massime per cella: una cella da 64 MiB contiene al piu' 16 byte
/// per coordinata XY.
///
/// Scelta B1.3 (come in `arrow_adapter::MAX_CELL_COORDINATES`): bound
/// conservativo non stride-aware — con Z/M il reale e' minore, quindi il
/// bound e' permissivo ma sicuro; `Unknown` (R3.4) non ha stride garantito.
pub const MAX_CELL_COORDINATES: u64 = MAX_CELL_BYTES / 16;
const PAYLOAD_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ArrowTransportError {
    #[error("errore I/O trasporto Arrow: {0}")]
    Io(#[from] std::io::Error),
    #[error("magic envelope Arrow v3 non valido")]
    InvalidMagic,
    #[error("trailer envelope Arrow v3 non valido")]
    InvalidTrailer,
    #[error("checksum envelope Arrow v3 non valido")]
    ChecksumMismatch,
    #[error("payload Arrow oltre il limite di {MAX_STREAM_BYTES} byte")]
    StreamTooLarge,
    #[error("byte inattesi dopo il trailer envelope Arrow v3")]
    TrailingBytes,
    #[error("righe {0} oltre il limite {MAX_ROWS}")]
    TooManyRows(u64),
    #[error("colonne {0} oltre il limite {MAX_COLUMNS}")]
    TooManyColumns(usize),
    #[error("batch {0} oltre il limite {MAX_BATCHES}")]
    TooManyBatches(usize),
    #[error("cella WKB da {0} byte oltre il limite {MAX_CELL_BYTES}")]
    CellTooLarge(u64),
    #[error("row_count non coerente: schema={schema}, stream={stream}")]
    RowCountMismatch { schema: u64, stream: u64 },
    #[error("payload scritto {written} byte, dichiarati {declared}")]
    PayloadLengthMismatch { declared: u64, written: u64 },
    #[error("schema_version {0} non supportata dal trasporto Arrow")]
    UnsupportedSchemaVersion(u32),
    #[error("colonna geometria `{0}` assente")]
    MissingGeometryColumn(String),
    #[error("colonna geometria `{0}` senza metadati estensione geoarrow.wkb")]
    MissingGeoArrowMetadata(String),
    #[error("colonna geometria `{name}` di tipo {actual}, atteso Binary")]
    GeometryColumnNotBinary { name: String, actual: String },
    #[error("crs obbligatorio per il trasporto Arrow v3")]
    CrsRequired,
    #[error("crs oltre il limite di {MAX_CRS_DEFINITION_BYTES} byte")]
    CrsTooLarge,
    #[error("parametro {name} obbligatorio per {operation}")]
    MissingParameter {
        operation: &'static str,
        name: &'static str,
    },
    #[error("parametro {name} non applicabile a {operation}")]
    UnexpectedParameter {
        operation: &'static str,
        name: &'static str,
    },
    #[error("parametro {name} non valido per {operation}: {reason}")]
    InvalidParameter {
        operation: &'static str,
        name: &'static str,
        reason: &'static str,
    },
    #[error("operazione {operation} non disponibile senza la feature {feature}")]
    BackendUnavailable {
        operation: &'static str,
        feature: &'static str,
    },
    #[error("metadati messaggio IPC da {0} byte oltre il limite {MAX_IPC_METADATA_BYTES}")]
    IpcMetadataTooLarge(usize),
    #[error("stream IPC troncato o non allineato")]
    IpcTruncated,
    /// Invariante interna violata: parametro gia' validato a monte o caso
    /// gia' ristretto dal dispatch. Indica un difetto del trasporto, non
    /// dell'input; il messaggio nomina solo il parametro o il caso, mai dati.
    #[error("errore interno trasporto Arrow: {0}")]
    Internal(&'static str),
    #[error("decodifica Arrow IPC fallita: {0}")]
    Arrow(String),
    #[error("geometria non valida: {0}")]
    Geometry(String),
    #[error("kernel fallito: {0}")]
    Kernel(#[from] OperationError),
    #[error("righe di output {actual} oltre il limite max_output_rows {limit}")]
    OutputRowsExceeded { actual: u64, limit: u64 },
    #[error("colonna `{0}` assente")]
    MissingColumn(String),
    #[error("colonna `{name}` di tipo {actual}, attesa numerica (Float64 o Int64)")]
    ColumnNotNumeric { name: String, actual: String },
    #[error("colonna `{name}`: coordinata intera oltre 2^53 in valore assoluto, conversione f64 non esatta")]
    IntegerCoordinateTooLarge { name: String },
    #[error("colonna geometria di output `{0}` gia' presente nell'input")]
    OutputColumnExists(String),
    #[error("topologia fallita: {0}")]
    Topology(#[from] TopologyError),
    #[error("costruzione fallita: {0}")]
    Construction(#[from] ConstructionError),
    #[error("kernel avanzato fallito: {0}")]
    Advanced(#[from] AdvancedError),
    #[error("row_count {side} non coerente: schema={schema}, stream={stream}")]
    PairRowCountMismatch {
        side: &'static str,
        schema: u64,
        stream: u64,
    },
    #[error("row_count non allineati: left={left}, right={right}")]
    SideLengthMismatch { left: u64, right: u64 },
    #[error("{operation}: attesa geometria {expected}, ricevuta {actual}")]
    WrongGeometryType {
        operation: &'static str,
        expected: &'static str,
        actual: String,
    },
    #[error("kernel esteso fallito: {0}")]
    Extended(#[from] ExtendedError),
    #[error("algoritmo esteso fallito: {0}")]
    ExtendedAlgorithm(#[from] ExtendedAlgorithmError),
    #[error("predicato fallito: {0}")]
    Predicate(#[from] PredicateError),
    #[error("analisi fallita: {0}")]
    Analysis(#[from] AnalysisError),
    #[error("spatial join fallito: {0}")]
    SpatialJoin(#[from] SpatialJoinError),
    #[cfg(feature = "geos-backend")]
    #[error("make_valid GEOS fallito: {0}")]
    MakeValid(#[from] GeosBackendError),
    #[cfg(feature = "proj-backend")]
    #[error("riproiezione PROJ fallita: {0}")]
    Reproject(#[from] ProjBackendError),
}

/// Conversione dagli errori del kernel WKB (`geometry_from_wkb`,
/// `transform_wkb`, `validate_wkb_contract`): nel sorgente restituivano
/// `GeoEngineError` (variante `Geometry`), ora restituiscono `PlenoraError`.
/// Le varianti `Contract`/`Unsupported`/`Schema` portano nel payload la
/// stringa ESATTA dell'errore originale, quindi vanno in `Geometry`
/// preservando il messaggio. `Io` conserva l'errore I/O incapsulato.
/// `Arrow`, `Json`, `Crs` e `Step` non hanno una variante dedicata in
/// `ArrowTransportError` (nel flusso del trasporto non si presentano mai:
/// il kernel WKB emette solo `Contract`/`Unsupported`): sono mappate su
/// `Arrow` mantenendo il testo completo dell'errore.
impl From<PlenoraError> for ArrowTransportError {
    fn from(error: PlenoraError) -> Self {
        match error {
            PlenoraError::Contract(message)
            | PlenoraError::Unsupported(message)
            | PlenoraError::Schema(message) => Self::Geometry(message),
            PlenoraError::Io(error) => Self::Io(error),
            other => Self::Arrow(other.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ArrowOperation {
    Centroid,
    ConvexHull,
    Envelope,
    Buffer,
    Simplify,
    Boundary,
    PointOnSurface,
    MakeValid,
    Reproject,
    Area,
    Length,
    Perimeter,
    VertexCount,
    Bounds,
    ToWkt,
    Explode,
    Dissolve,
    LineBuilder,
    PolygonBuilder,
    Voronoi,
    FromCoords,
    CleanTopology,
    AffineTransform,
    Translate,
    Scale,
    Rotate,
    ConcaveHull,
    Densify,
    SnapToGrid,
    LineSubstring,
    LineInterpolatePoint,
    GeodesicLineLength,
    GeodesicArea,
    GeometryDiagnostics,
    Delaunay,
    Polygonize,
    LineMerge,
}

/// Cardinalita' input/output dell'operazione sulle righe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrowShape {
    OneToOne,
    OneToMany,
    ManyToOne,
    /// Calcolo sull'intera tabella con output allineato alle righe non-null
    /// (`voronoi`, `clean_topology`).
    Collective,
    /// Tutta la tabella produce un insieme non allineato di righe
    /// (`polygonize`, `line_merge`).
    WholeToMany,
    /// Nessuna colonna geometria in input: Point da due colonne numeriche.
    FromCoords,
    /// Struct diagnostico per riga (`geometry_diagnostics`).
    Diagnostic,
}

impl ArrowOperation {
    pub const ALL: [Self; 37] = [
        Self::Centroid,
        Self::ConvexHull,
        Self::Envelope,
        Self::Buffer,
        Self::Simplify,
        Self::Boundary,
        Self::PointOnSurface,
        Self::MakeValid,
        Self::Reproject,
        Self::Area,
        Self::Length,
        Self::Perimeter,
        Self::VertexCount,
        Self::Bounds,
        Self::ToWkt,
        Self::Explode,
        Self::Dissolve,
        Self::LineBuilder,
        Self::PolygonBuilder,
        Self::Voronoi,
        Self::FromCoords,
        Self::CleanTopology,
        Self::AffineTransform,
        Self::Translate,
        Self::Scale,
        Self::Rotate,
        Self::ConcaveHull,
        Self::Densify,
        Self::SnapToGrid,
        Self::LineSubstring,
        Self::LineInterpolatePoint,
        Self::GeodesicLineLength,
        Self::GeodesicArea,
        Self::GeometryDiagnostics,
        Self::Delaunay,
        Self::Polygonize,
        Self::LineMerge,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Centroid => "centroid",
            Self::ConvexHull => "convex_hull",
            Self::Envelope => "envelope",
            Self::Buffer => "buffer",
            Self::Simplify => "simplify",
            Self::Boundary => "boundary",
            Self::PointOnSurface => "point_on_surface",
            Self::MakeValid => "make_valid",
            Self::Reproject => "reproject",
            Self::Area => "area",
            Self::Length => "length",
            Self::Perimeter => "perimeter",
            Self::VertexCount => "vertex_count",
            Self::Bounds => "bounds",
            Self::ToWkt => "to_wkt",
            Self::Explode => "explode",
            Self::Dissolve => "dissolve",
            Self::LineBuilder => "line_builder",
            Self::PolygonBuilder => "polygon_builder",
            Self::Voronoi => "voronoi",
            Self::FromCoords => "from_coords",
            Self::CleanTopology => "clean_topology",
            Self::AffineTransform => "affine_transform",
            Self::Translate => "translate",
            Self::Scale => "scale",
            Self::Rotate => "rotate",
            Self::ConcaveHull => "concave_hull",
            Self::Densify => "densify",
            Self::SnapToGrid => "snap_to_grid",
            Self::LineSubstring => "line_substring",
            Self::LineInterpolatePoint => "line_interpolate_point",
            Self::GeodesicLineLength => "geodesic_line_length",
            Self::GeodesicArea => "geodesic_area",
            Self::GeometryDiagnostics => "geometry_diagnostics",
            Self::Delaunay => "delaunay",
            Self::Polygonize => "polygonize",
            Self::LineMerge => "line_merge",
        }
    }

    /// Nome della voce di catalogo usata dal livello comandi per il requisito CRS.
    #[must_use]
    pub const fn catalog_name(self) -> &'static str {
        match self {
            Self::Centroid => "geo_centroid",
            Self::ConvexHull => "geo_convex_hull",
            Self::Envelope => "geo_envelope",
            Self::Buffer => "geo_buffer",
            Self::Simplify => "geo_simplify",
            Self::Boundary => "geo_boundary",
            Self::PointOnSurface => "geo_point_on_surface",
            Self::MakeValid => "geo_make_valid",
            Self::Reproject => "geo_reproject",
            Self::Area => "geo_area",
            Self::Length => "geo_length",
            Self::Perimeter => "geo_perimeter",
            Self::VertexCount => "geo_vertex_count",
            Self::Bounds => "geo_bounds_extractor",
            Self::ToWkt => "geo_to_wkt",
            Self::Explode => "geo_explode",
            Self::Dissolve => "geo_dissolve",
            Self::LineBuilder => "geo_line_builder",
            Self::PolygonBuilder => "geo_polygon_builder",
            Self::Voronoi => "geo_voronoi",
            Self::FromCoords => "geo_from_coords",
            Self::CleanTopology => "geo_clean_topology",
            Self::AffineTransform => "affine_transform",
            Self::Translate => "translate",
            Self::Scale => "scale",
            Self::Rotate => "rotate",
            Self::ConcaveHull => "concave_hull",
            Self::Densify => "densify",
            Self::SnapToGrid => "snap_to_grid",
            Self::Delaunay => "delaunay",
            Self::Polygonize => "polygonize",
            Self::LineMerge => "line_merge",
            Self::LineSubstring => "line_substring",
            Self::LineInterpolatePoint => "line_interpolate_point",
            Self::GeodesicLineLength => "geodesic_line_length",
            Self::GeodesicArea => "geodesic_area",
            Self::GeometryDiagnostics => "geometry_diagnostics",
        }
    }

    #[must_use]
    pub const fn shape(self) -> ArrowShape {
        match self {
            Self::Explode | Self::Delaunay => ArrowShape::OneToMany,
            Self::Dissolve
            | Self::LineBuilder
            | Self::PolygonBuilder => ArrowShape::ManyToOne,
            Self::Voronoi | Self::CleanTopology => ArrowShape::Collective,
            Self::Polygonize | Self::LineMerge => ArrowShape::WholeToMany,
            Self::GeometryDiagnostics => ArrowShape::Diagnostic,
            Self::FromCoords => ArrowShape::FromCoords,
            _ => ArrowShape::OneToOne,
        }
    }

    const fn geometry_kernel(self) -> Option<Operation> {
        match self {
            Self::Centroid => Some(Operation::Centroid),
            Self::ConvexHull => Some(Operation::ConvexHull),
            Self::Envelope => Some(Operation::Envelope),
            _ => None,
        }
    }

    /// Vero se l'output sostituisce la colonna geometria con una nuova
    /// colonna GeoArrow-WKB (con metadato CRS), falso per output scalari.
    /// Rilevante solo per le operazioni 1:1.
    const fn produces_geometry(self) -> bool {
        matches!(
            self,
            Self::Centroid
                | Self::ConvexHull
                | Self::Envelope
                | Self::Buffer
                | Self::Simplify
                | Self::Boundary
                | Self::PointOnSurface
                | Self::MakeValid
                | Self::Reproject
                | Self::AffineTransform
                | Self::Translate
                | Self::Scale
                | Self::Rotate
                | Self::ConcaveHull
                | Self::Densify
                | Self::SnapToGrid
                | Self::LineSubstring
                | Self::LineInterpolatePoint
        )
    }
}

/// Stile di cap per `buffer` (default round, come il kernel).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BufferCap {
    Round,
    Flat,
    Square,
}

impl From<BufferCap> for BufferCapStyle {
    fn from(cap: BufferCap) -> Self {
        match cap {
            BufferCap::Round => Self::Round,
            BufferCap::Flat => Self::Flat,
            BufferCap::Square => Self::Square,
        }
    }
}

/// Politica di `simplify`: Douglas-Peucker (default) oppure topology-preserving.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SimplifyPolicyParam {
    DouglasPeucker,
    PreserveTopology,
}

impl From<SimplifyPolicyParam> for SimplifyPolicy {
    fn from(policy: SimplifyPolicyParam) -> Self {
        match policy {
            SimplifyPolicyParam::DouglasPeucker => Self::DouglasPeucker,
            SimplifyPolicyParam::PreserveTopology => Self::PreserveTopology,
        }
    }
}

/// Schema JSON del comando transform-arrow (`schema_version: 3`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformArrowSchema {
    pub schema_version: u32,
    pub operation: ArrowOperation,
    pub row_count: u64,
    pub crs: Option<String>,
    pub geometry_column: Option<String>,
    pub distance: Option<f64>,
    pub cap: Option<BufferCap>,
    pub tolerance: Option<f64>,
    pub simplify_policy: Option<SimplifyPolicyParam>,
    pub target_crs: Option<String>,
    pub max_output_rows: Option<u64>,
    pub max_points: Option<u64>,
    pub x_column: Option<String>,
    pub y_column: Option<String>,
    pub snap_tolerance: Option<f64>,
    pub remove_overlaps: Option<bool>,
    pub fill_gaps: Option<bool>,
    pub coefficients: Option<Vec<f64>>,
    pub x_offset: Option<f64>,
    pub y_offset: Option<f64>,
    pub x_factor: Option<f64>,
    pub y_factor: Option<f64>,
    pub degrees: Option<f64>,
    pub x_origin: Option<f64>,
    pub y_origin: Option<f64>,
    pub concavity: Option<f64>,
    pub length_threshold: Option<f64>,
    pub max_segment_length: Option<f64>,
    pub grid_size: Option<f64>,
    pub start_ratio: Option<f64>,
    pub end_ratio: Option<f64>,
    pub ratio: Option<f64>,
    pub node_input: Option<bool>,
    pub require_complete: Option<bool>,
}

impl TransformArrowSchema {
    pub const VERSION: u32 = 3;

    #[must_use]
    pub fn geometry_column(&self) -> &str {
        self.geometry_column
            .as_deref()
            .unwrap_or(DEFAULT_GEOMETRY_COLUMN)
    }

    #[must_use]
    pub fn x_column(&self) -> &str {
        self.x_column.as_deref().unwrap_or(DEFAULT_X_COLUMN)
    }

    #[must_use]
    pub fn y_column(&self) -> &str {
        self.y_column.as_deref().unwrap_or(DEFAULT_Y_COLUMN)
    }

    /// Limite di espansione righe: obbligatorio per le operazioni 1:N,
    /// default `MAX_ROWS` per le altre.
    #[must_use]
    pub fn max_output_rows_limit(&self) -> u64 {
        self.max_output_rows.unwrap_or(MAX_ROWS)
    }

    fn required_max_output_rows(&self) -> Result<u64, ArrowTransportError> {
        self.max_output_rows
            .ok_or_else(|| ArrowTransportError::MissingParameter {
                operation: self.operation.name(),
                name: "max_output_rows",
            })
    }

    fn required_distance(&self) -> Result<f64, ArrowTransportError> {
        let distance = self.distance.ok_or_else(|| ArrowTransportError::MissingParameter {
            operation: self.operation.name(),
            name: "distance",
        })?;
        if !distance.is_finite() {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name: "distance",
                reason: "deve essere finita",
            });
        }
        Ok(distance)
    }

    fn required_tolerance(&self) -> Result<f64, ArrowTransportError> {
        let tolerance = self
            .tolerance
            .ok_or_else(|| ArrowTransportError::MissingParameter {
                operation: self.operation.name(),
                name: "tolerance",
            })?;
        if !tolerance.is_finite() || tolerance < 0.0 {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name: "tolerance",
                reason: "deve essere finita e non negativa",
            });
        }
        Ok(tolerance)
    }

    fn required_target_crs(&self) -> Result<&str, ArrowTransportError> {
        let target = self
            .target_crs
            .as_deref()
            .ok_or_else(|| ArrowTransportError::MissingParameter {
                operation: self.operation.name(),
                name: "target_crs",
            })?;
        if target.trim().is_empty() {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name: "target_crs",
                reason: "non deve essere vuoto",
            });
        }
        if target.len() > MAX_CRS_DEFINITION_BYTES {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name: "target_crs",
                reason: "oltre il limite di byte per definizione CRS",
            });
        }
        Ok(target)
    }

    fn present_extension_params(&self) -> Vec<&'static str> {
        let mut present = Vec::new();
        if self.coefficients.is_some() {
            present.push("coefficients");
        }
        if self.x_offset.is_some() {
            present.push("x_offset");
        }
        if self.y_offset.is_some() {
            present.push("y_offset");
        }
        if self.x_factor.is_some() {
            present.push("x_factor");
        }
        if self.y_factor.is_some() {
            present.push("y_factor");
        }
        if self.degrees.is_some() {
            present.push("degrees");
        }
        if self.x_origin.is_some() {
            present.push("x_origin");
        }
        if self.y_origin.is_some() {
            present.push("y_origin");
        }
        if self.concavity.is_some() {
            present.push("concavity");
        }
        if self.length_threshold.is_some() {
            present.push("length_threshold");
        }
        if self.max_segment_length.is_some() {
            present.push("max_segment_length");
        }
        if self.grid_size.is_some() {
            present.push("grid_size");
        }
        if self.start_ratio.is_some() {
            present.push("start_ratio");
        }
        if self.end_ratio.is_some() {
            present.push("end_ratio");
        }
        if self.ratio.is_some() {
            present.push("ratio");
        }
        if self.node_input.is_some() {
            present.push("node_input");
        }
        if self.require_complete.is_some() {
            present.push("require_complete");
        }
        present
    }

    fn check_extension_params(&self, allowed: &[&'static str]) -> Result<(), ArrowTransportError> {
        for name in self.present_extension_params() {
            if !allowed.contains(&name) {
                return Err(ArrowTransportError::UnexpectedParameter {
                    operation: self.operation.name(),
                    name,
                });
            }
        }
        Ok(())
    }

    fn required_f64(
        &self,
        name: &'static str,
        value: Option<f64>,
    ) -> Result<f64, ArrowTransportError> {
        value.ok_or_else(|| ArrowTransportError::MissingParameter {
            operation: self.operation.name(),
            name,
        })
    }

    const fn finite_param(&self, name: &'static str, value: f64) -> Result<f64, ArrowTransportError> {
        if !value.is_finite() {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name,
                reason: "deve essere finito",
            });
        }
        Ok(value)
    }

    fn ratio_param(&self, name: &'static str, value: f64) -> Result<f64, ArrowTransportError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name,
                reason: "deve essere finito e compreso tra zero e uno",
            });
        }
        Ok(value)
    }

    /// Verifica che i parametri presenti siano esattamente quelli previsti
    /// dall'operazione e che i valori siano nel dominio del kernel.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::MissingParameter` se manca un parametro
    /// obbligatorio, `ArrowTransportError::UnexpectedParameter` se ne e'
    /// presente uno non previsto dall'operazione,
    /// `ArrowTransportError::InvalidParameter` se un valore e' fuori dominio.
    // Dispatch per operazione intenzionalmente in un'unica funzione: la
    // tabella parametri ammessi/obbligatori resta leggibile come tabella;
    // la scomposizione strutturale e' rimandata a una fase dedicata.
    #[allow(clippy::too_many_lines)]
    pub fn validate_parameters(&self) -> Result<(), ArrowTransportError> {
        let operation = self.operation.name();
        let unexpected = |name: &'static str, present: bool| {
            if present {
                Err(ArrowTransportError::UnexpectedParameter { operation, name })
            } else {
                Ok(())
            }
        };
        // Parametri delle estensioni ammessi per operazione; tutti gli altri
        // sono rifiutati prima di toccare i dati.
        let extension_allowed: &[&'static str] = match self.operation {
            ArrowOperation::AffineTransform => &["coefficients"],
            ArrowOperation::Translate => &["x_offset", "y_offset"],
            ArrowOperation::Scale => &["x_factor", "y_factor", "x_origin", "y_origin"],
            ArrowOperation::Rotate => &["degrees", "x_origin", "y_origin"],
            ArrowOperation::ConcaveHull => &["concavity", "length_threshold"],
            ArrowOperation::Densify => &["max_segment_length"],
            ArrowOperation::SnapToGrid => &["grid_size"],
            ArrowOperation::LineSubstring => &["start_ratio", "end_ratio"],
            ArrowOperation::LineInterpolatePoint => &["ratio"],
            ArrowOperation::Polygonize => &["node_input", "require_complete"],
            _ => &[],
        };
        self.check_extension_params(extension_allowed)?;
        let reject_geometry_params = |schema: &Self| -> Result<(), ArrowTransportError> {
            unexpected("distance", schema.distance.is_some())?;
            unexpected("cap", schema.cap.is_some())?;
            unexpected("tolerance", schema.tolerance.is_some())?;
            unexpected("simplify_policy", schema.simplify_policy.is_some())?;
            unexpected("target_crs", schema.target_crs.is_some())?;
            Ok(())
        };
        let reject_builder_params = |schema: &Self| -> Result<(), ArrowTransportError> {
            unexpected("max_points", schema.max_points.is_some())?;
            unexpected("x_column", schema.x_column.is_some())?;
            unexpected("y_column", schema.y_column.is_some())?;
            Ok(())
        };
        let reject_clean_params = |schema: &Self| -> Result<(), ArrowTransportError> {
            unexpected("snap_tolerance", schema.snap_tolerance.is_some())?;
            unexpected("remove_overlaps", schema.remove_overlaps.is_some())?;
            unexpected("fill_gaps", schema.fill_gaps.is_some())?;
            Ok(())
        };
        match self.operation {
            ArrowOperation::Buffer => {
                self.required_distance()?;
                unexpected("tolerance", self.tolerance.is_some())?;
                unexpected("simplify_policy", self.simplify_policy.is_some())?;
                unexpected("target_crs", self.target_crs.is_some())?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
            ArrowOperation::Simplify => {
                self.required_tolerance()?;
                unexpected("distance", self.distance.is_some())?;
                unexpected("cap", self.cap.is_some())?;
                unexpected("target_crs", self.target_crs.is_some())?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
            ArrowOperation::Reproject => {
                self.required_target_crs()?;
                unexpected("distance", self.distance.is_some())?;
                unexpected("cap", self.cap.is_some())?;
                unexpected("tolerance", self.tolerance.is_some())?;
                unexpected("simplify_policy", self.simplify_policy.is_some())?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
            ArrowOperation::Explode | ArrowOperation::Delaunay => {
                self.required_max_output_rows()?;
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
            ArrowOperation::Voronoi => {
                reject_geometry_params(self)?;
                unexpected("x_column", self.x_column.is_some())?;
                unexpected("y_column", self.y_column.is_some())?;
                reject_clean_params(self)?;
                if let Some(max_points) = self.max_points {
                    if !(2..=MAX_ROWS).contains(&max_points) {
                        return Err(ArrowTransportError::InvalidParameter {
                            operation,
                            name: "max_points",
                            reason: "deve essere tra 2 e il limite righe del trasporto",
                        });
                    }
                }
            }
            ArrowOperation::FromCoords => {
                reject_geometry_params(self)?;
                unexpected("max_points", self.max_points.is_some())?;
                reject_clean_params(self)?;
                for (name, value) in [
                    ("x_column", self.x_column.as_deref()),
                    ("y_column", self.y_column.as_deref()),
                ] {
                    if value.is_some_and(|column| column.trim().is_empty()) {
                        return Err(ArrowTransportError::InvalidParameter {
                            operation,
                            name,
                            reason: "non deve essere vuoto",
                        });
                    }
                }
            }
            ArrowOperation::AffineTransform => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let coefficients =
                    self.coefficients
                        .as_ref()
                        .ok_or(ArrowTransportError::MissingParameter {
                            operation,
                            name: "coefficients",
                        })?;
                if coefficients.len() != 6 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "coefficients",
                        reason: "devono essere esattamente 6 coefficienti",
                    });
                }
                if coefficients.iter().any(|value| !value.is_finite()) {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "coefficients",
                        reason: "devono essere finiti",
                    });
                }
            }
            ArrowOperation::Translate => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                self.finite_param("x_offset", self.required_f64("x_offset", self.x_offset)?)?;
                self.finite_param("y_offset", self.required_f64("y_offset", self.y_offset)?)?;
            }
            ArrowOperation::Scale => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                self.finite_param("x_factor", self.required_f64("x_factor", self.x_factor)?)?;
                self.finite_param("y_factor", self.required_f64("y_factor", self.y_factor)?)?;
                if let Some(value) = self.x_origin {
                    self.finite_param("x_origin", value)?;
                }
                if let Some(value) = self.y_origin {
                    self.finite_param("y_origin", value)?;
                }
            }
            ArrowOperation::Rotate => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                self.finite_param("degrees", self.required_f64("degrees", self.degrees)?)?;
                if let Some(value) = self.x_origin {
                    self.finite_param("x_origin", value)?;
                }
                if let Some(value) = self.y_origin {
                    self.finite_param("y_origin", value)?;
                }
            }
            ArrowOperation::ConcaveHull => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let concavity = self
                    .finite_param("concavity", self.required_f64("concavity", self.concavity)?)?;
                if concavity <= 0.0 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "concavity",
                        reason: "deve essere maggiore di zero",
                    });
                }
                if let Some(value) = self.length_threshold {
                    let value = self.finite_param("length_threshold", value)?;
                    if value < 0.0 {
                        return Err(ArrowTransportError::InvalidParameter {
                            operation,
                            name: "length_threshold",
                            reason: "deve essere non negativa",
                        });
                    }
                }
            }
            ArrowOperation::Densify => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let value = self.finite_param(
                    "max_segment_length",
                    self.required_f64("max_segment_length", self.max_segment_length)?,
                )?;
                if value <= 0.0 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "max_segment_length",
                        reason: "deve essere maggiore di zero",
                    });
                }
            }
            ArrowOperation::SnapToGrid => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let value = self
                    .finite_param("grid_size", self.required_f64("grid_size", self.grid_size)?)?;
                if value <= 0.0 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "grid_size",
                        reason: "deve essere maggiore di zero",
                    });
                }
            }
            ArrowOperation::LineSubstring => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let start = self.ratio_param(
                    "start_ratio",
                    self.required_f64("start_ratio", self.start_ratio)?,
                )?;
                let end =
                    self.ratio_param("end_ratio", self.required_f64("end_ratio", self.end_ratio)?)?;
                if start > end {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "start_ratio/end_ratio",
                        reason: "start_ratio non puo superare end_ratio",
                    });
                }
            }
            ArrowOperation::LineInterpolatePoint => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                self.ratio_param("ratio", self.required_f64("ratio", self.ratio)?)?;
            }
            ArrowOperation::CleanTopology => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                let tolerance =
                    self.snap_tolerance
                        .ok_or(ArrowTransportError::MissingParameter {
                            operation,
                            name: "snap_tolerance",
                        })?;
                if !tolerance.is_finite() || tolerance < 0.0 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "snap_tolerance",
                        reason: "deve essere finita e non negativa",
                    });
                }
            }
            _ => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
        }
        if let Some(limit) = self.max_output_rows {
            if limit > MAX_ROWS {
                return Err(ArrowTransportError::InvalidParameter {
                    operation,
                    name: "max_output_rows",
                    reason: "oltre il limite righe del trasporto",
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct TransformArrowSummary {
    /// Righe di input lette (deve coincidere con `row_count` dello schema).
    pub rows: u64,
    /// Righe di output prodotte: diverge da `rows` per le forme 1:N e N:1.
    pub output_rows: u64,
    pub checksum: [u8; 32],
}

/// Lettore dell'envelope v3 con hasher incrementale, nello stile di
/// `protocol::FrameReader`.
pub struct EnvelopeReader<R> {
    inner: R,
    hasher: Sha256,
    payload_len: u64,
}

impl<R: Read> EnvelopeReader<R> {
    /// Costruisce il lettore e verifica magic e lunghezza dichiarata.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::InvalidMagic` se il magic non corrisponde,
    /// `ArrowTransportError::StreamTooLarge` se il payload dichiarato supera
    /// `MAX_STREAM_BYTES`, `ArrowTransportError::Io` per errori di lettura.
    pub fn new(mut inner: R) -> Result<Self, ArrowTransportError> {
        let mut magic = [0_u8; 8];
        inner.read_exact(&mut magic)?;
        if &magic != ENVELOPE_MAGIC {
            return Err(ArrowTransportError::InvalidMagic);
        }
        let mut payload_len_bytes = [0_u8; 8];
        inner.read_exact(&mut payload_len_bytes)?;
        let payload_len = u64::from_le_bytes(payload_len_bytes);
        if payload_len > MAX_STREAM_BYTES {
            return Err(ArrowTransportError::StreamTooLarge);
        }
        let mut hasher = Sha256::new();
        hasher.update(magic);
        hasher.update(payload_len_bytes);
        Ok(Self {
            inner,
            hasher,
            payload_len,
        })
    }

    /// Legge il payload a chunk, cosi' la memoria cresce solo con i byte che
    /// arrivano davvero, e verifica trailer, checksum e byte residui.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::InvalidTrailer` se il trailer non corrisponde,
    /// `ArrowTransportError::ChecksumMismatch` se il digest non coincide,
    /// `ArrowTransportError::TrailingBytes` se restano byte dopo il trailer,
    /// `ArrowTransportError::Io` per errori di lettura.
    pub fn read_payload(mut self) -> Result<Vec<u8>, ArrowTransportError> {
        let mut payload = Vec::new();
        let mut remaining = self.payload_len;
        while remaining > 0 {
            let take = remaining.min(PAYLOAD_CHUNK_BYTES) as usize;
            let start = payload.len();
            payload.resize(start + take, 0);
            self.inner.read_exact(&mut payload[start..])?;
            remaining -= take as u64;
        }
        self.hasher.update(&payload);

        let mut trailer_magic = [0_u8; 8];
        self.inner.read_exact(&mut trailer_magic)?;
        if &trailer_magic != ENVELOPE_TRAILER_MAGIC {
            return Err(ArrowTransportError::InvalidTrailer);
        }
        let mut expected_digest = [0_u8; 32];
        self.inner.read_exact(&mut expected_digest)?;
        let actual_digest: [u8; 32] = self.hasher.finalize().into();
        if actual_digest != expected_digest {
            return Err(ArrowTransportError::ChecksumMismatch);
        }
        let mut extra = [0_u8; 1];
        if self.inner.read(&mut extra)? != 0 {
            return Err(ArrowTransportError::TrailingBytes);
        }
        Ok(payload)
    }
}

/// Scrittore dell'envelope v3 con lunghezza dichiarata e hasher incrementale.
pub struct EnvelopeWriter<W> {
    inner: W,
    hasher: Sha256,
    payload_len: u64,
    written: u64,
}

impl<W: Write> EnvelopeWriter<W> {
    /// Costruisce lo scrittore e scrive l'header con la lunghezza dichiarata.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::StreamTooLarge` se `payload_len` supera
    /// `MAX_STREAM_BYTES`, `ArrowTransportError::Io` per errori di scrittura.
    pub fn new(mut inner: W, payload_len: u64) -> Result<Self, ArrowTransportError> {
        if payload_len > MAX_STREAM_BYTES {
            return Err(ArrowTransportError::StreamTooLarge);
        }
        let mut header = [0_u8; 16];
        header[..8].copy_from_slice(ENVELOPE_MAGIC);
        header[8..].copy_from_slice(&payload_len.to_le_bytes());
        inner.write_all(&header)?;
        let mut hasher = Sha256::new();
        hasher.update(header);
        Ok(Self {
            inner,
            hasher,
            payload_len,
            written: 0,
        })
    }

    /// Accoda un chunk di payload aggiornando il checksum incrementale.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::StreamTooLarge` se i byte scritti superano la
    /// lunghezza dichiarata, `ArrowTransportError::Io` per errori di
    /// scrittura.
    pub fn write_payload(&mut self, bytes: &[u8]) -> Result<(), ArrowTransportError> {
        let next = self
            .written
            .checked_add(bytes.len() as u64)
            .filter(|next| *next <= self.payload_len)
            .ok_or(ArrowTransportError::StreamTooLarge)?;
        self.inner.write_all(bytes)?;
        self.hasher.update(bytes);
        self.written = next;
        Ok(())
    }

    /// Chiude l'envelope scrivendo trailer e digest; restituisce il writer
    /// sottostante e il checksum.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::PayloadLengthMismatch` se i byte scritti non
    /// coincidono con la lunghezza dichiarata, `ArrowTransportError::Io` per
    /// errori di scrittura o flush.
    pub fn finish(mut self) -> Result<(W, [u8; 32]), ArrowTransportError> {
        if self.written != self.payload_len {
            return Err(ArrowTransportError::PayloadLengthMismatch {
                declared: self.payload_len,
                written: self.written,
            });
        }
        let digest: [u8; 32] = self.hasher.finalize().into();
        self.inner.write_all(ENVELOPE_TRAILER_MAGIC)?;
        self.inner.write_all(&digest)?;
        self.inner.flush()?;
        Ok((self.inner, digest))
    }
}

const fn align8(value: usize) -> usize {
    value.saturating_add(7) & !7
}

// --- Validazione strutturale dei metadati flatbuffer `Message` -------------
//
// arrow-format alloca `Vec::with_capacity(count)` per vettori e stringhe
// dichiarati nei metadati senza un tetto proprio: un payload malevolo puo'
// indurre allocazioni enormi (OOM, trovato via fuzzing). Questo validatore
// percorre la struttura `Message`/`Schema`/`RecordBatch` dello standard IPC
// e verifica che ogni vettore, stringa e buffer stia dentro i byte
// disponibili, prima che arrow-rs veda i metadati. Non e' un parser
// completo: copre solo la struttura che puo' allocare.

const MAX_FLATBUFFER_DEPTH: usize = 64;

fn fb_u16(buf: &[u8], pos: usize) -> Result<u16, ArrowTransportError> {
    buf.get(pos..pos + 2)
        .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
        .map(u16::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)
}

fn fb_u32(buf: &[u8], pos: usize) -> Result<u32, ArrowTransportError> {
    buf.get(pos..pos + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)
}

fn fb_i64(buf: &[u8], pos: usize) -> Result<i64, ArrowTransportError> {
    buf.get(pos..pos + 8)
        .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
        .map(i64::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)
}

/// Tabella flatbuffer in `pos`: ritorna (`vtable_start`, `vtable_len`).
/// A `pos` c'e' l'`soffset` (i32, distanza alla vtable); `vtable_len` e
/// `table_len` stanno nella vtable stessa. L'`soffset` puo' essere NEGATIVO:
/// con vtable deduplicate il writer puo' piazzare la vtable dopo la tabella.
fn fb_table(buf: &[u8], pos: usize) -> Result<(usize, usize), ArrowTransportError> {
    let soffset = buf
        .get(pos..pos + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(i32::from_le_bytes)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if soffset == 0 {
        return Err(ArrowTransportError::IpcTruncated);
    }
    // Conversioni totali: un offset che non entra in i64/usize e' un
    // riferimento malformato, mai un troncamento silenzioso (R5.4).
    let vtable_signed = i64::try_from(pos).map_err(|_| ArrowTransportError::IpcTruncated)?
        - i64::from(soffset);
    let vtable =
        usize::try_from(vtable_signed).map_err(|_| ArrowTransportError::IpcTruncated)?;
    let vtable_len = fb_u16(buf, vtable)? as usize;
    let table_len = fb_u16(buf, vtable + 2)? as usize;
    if vtable_len < 4
        || !vtable_len.is_multiple_of(2)
        || vtable + vtable_len > buf.len()
        || pos + table_len > buf.len()
    {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok((vtable, vtable_len))
}

/// Offset del campo `index` dalla vtable (0 se assente).
fn fb_field(
    buf: &[u8],
    vtable: usize,
    vtable_len: usize,
    index: usize,
) -> Result<usize, ArrowTransportError> {
    let entry = 4 + index * 2;
    if entry + 2 > vtable_len {
        return Ok(0);
    }
    let bytes: [u8; 2] = buf
        .get(vtable + entry..vtable + entry + 2)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ArrowTransportError::IpcTruncated)?;
    Ok(u16::from_le_bytes(bytes) as usize)
}

/// Posizione assoluta di un campo indiretto (tabella, vettore, stringa).
fn fb_indirect(buf: &[u8], table: usize, offset: usize) -> Result<usize, ArrowTransportError> {
    let relative = fb_u32(buf, table + offset)? as usize;
    let target = (table + offset)
        .checked_add(relative)
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if target + 4 > buf.len() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok(target)
}

/// Conteggio di un vettore flatbuffer con elementi da `elem_size` byte:
/// il contenuto deve stare interamente nel buffer.
fn fb_vector(buf: &[u8], pos: usize, elem_size: usize) -> Result<usize, ArrowTransportError> {
    let count = fb_u32(buf, pos)? as usize;
    let bytes = count
        .checked_mul(elem_size)
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or(ArrowTransportError::IpcTruncated)?;
    if pos + bytes > buf.len() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok(count)
}

/// Stringa flatbuffer (vettore di byte con terminatore).
fn fb_string(buf: &[u8], pos: usize) -> Result<(), ArrowTransportError> {
    let count = fb_vector(buf, pos, 1)?;
    if pos + 4 + count >= buf.len() {
        return Err(ArrowTransportError::IpcTruncated);
    }
    Ok(())
}

fn fb_key_value(buf: &[u8], table: usize) -> Result<(), ArrowTransportError> {
    let (vtable, vtable_len) = fb_table(buf, table)?;
    for index in [0, 1] {
        let offset = fb_field(buf, vtable, vtable_len, index)?;
        if offset != 0 {
            fb_string(buf, fb_indirect(buf, table, offset)?)?;
        }
    }
    Ok(())
}

fn fb_custom_metadata(buf: &[u8], table: usize, offset: usize) -> Result<(), ArrowTransportError> {
    if offset == 0 {
        return Ok(());
    }
    let vector = fb_indirect(buf, table, offset)?;
    let count = fb_vector(buf, vector, 4)?;
    for index in 0..count {
        let entry = fb_indirect(buf, vector + 4, index * 4)?;
        fb_key_value(buf, entry)?;
    }
    Ok(())
}

/// Tabella `Field` di uno Schema IPC.
fn fb_field_table(buf: &[u8], table: usize, depth: usize) -> Result<(), ArrowTransportError> {
    if depth > MAX_FLATBUFFER_DEPTH {
        return Err(ArrowTransportError::IpcTruncated);
    }
    let (vtable, vtable_len) = fb_table(buf, table)?;
    // name: stringa.
    let name = fb_field(buf, vtable, vtable_len, 0)?;
    if name != 0 {
        fb_string(buf, fb_indirect(buf, table, name)?)?;
    }
    // type (union): la tabella e' verificata nei limiti; il solo tipo con
    // vettori (Union.typeIds) e' controllato esplicitamente.
    let type_type_offset = fb_field(buf, vtable, vtable_len, 2)?;
    let type_offset = fb_field(buf, vtable, vtable_len, 3)?;
    if type_offset != 0 {
        let union_table = fb_indirect(buf, table, type_offset)?;
        let (type_vtable, type_vtable_len) = fb_table(buf, union_table)?;
        if type_type_offset != 0 {
            let type_type = *buf
                .get(table + type_type_offset)
                .ok_or(ArrowTransportError::IpcTruncated)?;
            if type_type == 14 {
                let type_ids = fb_field(buf, type_vtable, type_vtable_len, 3)?;
                if type_ids != 0 {
                    fb_vector(buf, fb_indirect(buf, union_table, type_ids)?, 4)?;
                }
            }
        }
    }
    // dictionary: DictionaryEncoding (scalari + tabella Int).
    let dictionary = fb_field(buf, vtable, vtable_len, 4)?;
    if dictionary != 0 {
        let dictionary_table = fb_indirect(buf, table, dictionary)?;
        let (dict_vtable, dict_vtable_len) = fb_table(buf, dictionary_table)?;
        let index_type = fb_field(buf, dict_vtable, dict_vtable_len, 1)?;
        if index_type != 0 {
            fb_table(buf, fb_indirect(buf, dictionary_table, index_type)?)?;
        }
    }
    // children: vettore di Field.
    let children = fb_field(buf, vtable, vtable_len, 5)?;
    if children != 0 {
        let vector = fb_indirect(buf, table, children)?;
        let count = fb_vector(buf, vector, 4)?;
        for index in 0..count {
            let child = fb_indirect(buf, vector + 4, index * 4)?;
            fb_field_table(buf, child, depth + 1)?;
        }
    }
    // custom_metadata.
    let custom = fb_field(buf, vtable, vtable_len, 6)?;
    fb_custom_metadata(buf, table, custom)?;
    Ok(())
}

/// Tabella `RecordBatch`: nodi, buffer (entro il body dichiarato), variadic.
fn fb_record_batch(buf: &[u8], table: usize, body_len: usize) -> Result<(), ArrowTransportError> {
    let (vtable, vtable_len) = fb_table(buf, table)?;
    let nodes = fb_field(buf, vtable, vtable_len, 1)?;
    if nodes != 0 {
        fb_vector(buf, fb_indirect(buf, table, nodes)?, 16)?;
    }
    let buffers = fb_field(buf, vtable, vtable_len, 2)?;
    if buffers != 0 {
        let vector = fb_indirect(buf, table, buffers)?;
        let count = fb_vector(buf, vector, 16)?;
        for index in 0..count {
            let entry = vector + 4 + index * 16;
            let buffer_offset = fb_i64(buf, entry)?;
            let length = fb_i64(buf, entry + 8)?;
            // Conversione totale: negativi o oltre usize (target a 32 bit)
            // sono offset malformati, rifiutati invece che troncati.
            let end = usize::try_from(buffer_offset)
                .ok()
                .zip(usize::try_from(length).ok())
                .and_then(|(offset, len)| offset.checked_add(len))
                .ok_or(ArrowTransportError::IpcTruncated)?;
            if end > body_len {
                return Err(ArrowTransportError::IpcTruncated);
            }
        }
    }
    let compression = fb_field(buf, vtable, vtable_len, 3)?;
    if compression != 0 {
        fb_table(buf, fb_indirect(buf, table, compression)?)?;
    }
    let variadic = fb_field(buf, vtable, vtable_len, 4)?;
    if variadic != 0 {
        fb_vector(buf, fb_indirect(buf, table, variadic)?, 8)?;
    }
    Ok(())
}

/// Tabella `Schema`: fields, `custom_metadata` e feature.
fn fb_schema(buf: &[u8], table: usize) -> Result<(), ArrowTransportError> {
    let (vtable, vtable_len) = fb_table(buf, table)?;
    let fields = fb_field(buf, vtable, vtable_len, 1)?;
    if fields != 0 {
        let vector = fb_indirect(buf, table, fields)?;
        let count = fb_vector(buf, vector, 4)?;
        if count > MAX_COLUMNS {
            return Err(ArrowTransportError::TooManyColumns(count));
        }
        for index in 0..count {
            let field = fb_indirect(buf, vector + 4, index * 4)?;
            fb_field_table(buf, field, 0)?;
        }
    }
    let custom = fb_field(buf, vtable, vtable_len, 2)?;
    fb_custom_metadata(buf, table, custom)?;
    let features = fb_field(buf, vtable, vtable_len, 3)?;
    if features != 0 {
        fb_vector(buf, fb_indirect(buf, table, features)?, 8)?;
    }
    Ok(())
}

/// Valida i metadati flatbuffer di un messaggio IPC e ritorna la lunghezza
/// del body dichiarata (`bodyLength`). Header Tensor/SparseTensor sono
/// rifiutati: il trasporto non li usa e nessun produttore onesto li emette.
fn validate_ipc_message_metadata(metadata: &[u8]) -> Result<usize, ArrowTransportError> {
    if metadata.len() < 8 {
        return Err(ArrowTransportError::IpcTruncated);
    }
    let table = fb_u32(metadata, 0)? as usize;
    let (vtable, vtable_len) = fb_table(metadata, table)?;

    // version (0) e header_type (1) sono scalari; header (2) e' la tabella
    // del messaggio; bodyLength (3) uno scalare i64; custom_metadata (4).
    let header_type_offset = fb_field(metadata, vtable, vtable_len, 1)?;
    let header_type = if header_type_offset == 0 {
        0
    } else {
        *metadata
            .get(table + header_type_offset)
            .ok_or(ArrowTransportError::IpcTruncated)?
    };
    let header_offset = fb_field(metadata, vtable, vtable_len, 2)?;
    let header_table = if header_offset == 0 {
        None
    } else {
        Some(fb_indirect(metadata, table, header_offset)?)
    };
    if let Some(header_table) = header_table {
        match header_type {
            1 => fb_schema(metadata, header_table)?,
            2 => {
                // DictionaryBatch: data (RecordBatch) al campo 1.
                let (dict_vtable, dict_vtable_len) = fb_table(metadata, header_table)?;
                let data = fb_field(metadata, dict_vtable, dict_vtable_len, 1)?;
                if data != 0 {
                    let batch = fb_indirect(metadata, header_table, data)?;
                    fb_record_batch(metadata, batch, metadata.len())?;
                }
            }
            3 => {
                // body_len verificato dopo la lettura di bodyLength.
                let _ = fb_table(metadata, header_table)?;
            }
            _ => {
                return Err(ArrowTransportError::Arrow(
                    "header IPC Tensor/SparseTensor non supportato".to_owned(),
                ))
            }
        }
    }

    let body_len_offset = fb_field(metadata, vtable, vtable_len, 3)?;
    let body_len = if body_len_offset == 0 {
        0
    } else {
        let value = fb_i64(metadata, table + body_len_offset)?;
        if value < 0 {
            return Err(ArrowTransportError::IpcTruncated);
        }
        usize::try_from(value).map_err(|_| ArrowTransportError::IpcTruncated)?
    };

    // Con il body noto, i buffer del RecordBatch devono starci dentro.
    if let (Some(header_table), 3) = (header_table, header_type) {
        fb_record_batch(metadata, header_table, body_len)?;
    }

    let custom = fb_field(metadata, vtable, vtable_len, 4)?;
    fb_custom_metadata(metadata, table, custom)?;
    Ok(body_len)
}

/// Pre-validazione del framing IPC prima che arrow-rs allochi: ogni messaggio
/// dichiara la lunghezza dei propri metadati e il flatbuffer dichiara il
/// body; entrambi devono stare dentro il payload, i metadati entro un tetto
/// assoluto e la struttura flatbuffer entro i propri limiti. Senza questo
/// controllo un payload malevolo induce allocazioni enormi dentro arrow-rs
/// (OOM, trovato via fuzzing).
fn validate_ipc_framing(payload: &[u8]) -> Result<(), ArrowTransportError> {
    let mut offset = 0_usize;
    loop {
        let prefix_bytes: [u8; 4] = payload
            .get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(ArrowTransportError::IpcTruncated)?;
        let prefix = u32::from_le_bytes(prefix_bytes);
        let (metadata_len, header) = if prefix == 0xFFFF_FFFF {
            let length_bytes: [u8; 4] = payload
                .get(offset + 4..offset + 8)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(ArrowTransportError::IpcTruncated)?;
            (u32::from_le_bytes(length_bytes) as usize, 8)
        } else {
            (prefix as usize, 4)
        };
        if metadata_len == 0 {
            return Ok(());
        }
        if metadata_len > MAX_IPC_METADATA_BYTES {
            return Err(ArrowTransportError::IpcMetadataTooLarge(metadata_len));
        }
        let metadata_start = offset + header;
        let metadata = payload
            .get(metadata_start..metadata_start + metadata_len)
            .ok_or(ArrowTransportError::IpcTruncated)?;
        let body_len = validate_ipc_message_metadata(metadata)?;
        let end = align8(
            align8(metadata_start + metadata_len)
                .checked_add(body_len)
                .ok_or(ArrowTransportError::IpcTruncated)?,
        );
        if end > payload.len() {
            return Err(ArrowTransportError::IpcTruncated);
        }
        offset = end;
    }
}

/// Decodifica il payload Arrow IPC applicando i limiti di risorse prima di
/// accumulare i batch.
///
/// # Errors
///
/// `ArrowTransportError::IpcTruncated` o `ArrowTransportError::Arrow` per
/// stream malformati, `ArrowTransportError::TooManyColumns` /
/// `TooManyBatches` / `TooManyRows` / `StreamTooLarge` al superamento dei
/// limiti di risorse.
pub fn decode_ipc(payload: &[u8]) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    validate_ipc_framing(payload)?;
    let reader = StreamReader::try_new(payload, None)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    let schema = reader.schema();
    if schema.fields().len() > MAX_COLUMNS {
        return Err(ArrowTransportError::TooManyColumns(schema.fields().len()));
    }
    let mut batches = Vec::new();
    let mut rows = 0_u64;
    for batch in reader {
        let batch = batch.map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
        if batches.len() >= MAX_BATCHES {
            return Err(ArrowTransportError::TooManyBatches(batches.len() + 1));
        }
        rows = rows
            .checked_add(batch.num_rows() as u64)
            .ok_or(ArrowTransportError::StreamTooLarge)?;
        if rows > MAX_ROWS {
            return Err(ArrowTransportError::TooManyRows(rows));
        }
        batches.push(batch);
    }
    Ok((schema, batches))
}

fn geometry_column_index(schema: &Schema, name: &str) -> Result<usize, ArrowTransportError> {
    let (index, field) = schema
        .column_with_name(name)
        .ok_or_else(|| ArrowTransportError::MissingGeometryColumn(name.to_owned()))?;
    if field.data_type() != &DataType::Binary {
        return Err(ArrowTransportError::GeometryColumnNotBinary {
            name: name.to_owned(),
            actual: field.data_type().to_string(),
        });
    }
    let extension = field.metadata().get(GEOARROW_EXTENSION_KEY);
    if extension.map(String::as_str) != Some(GEOARROW_WKB_EXTENSION) {
        return Err(ArrowTransportError::MissingGeoArrowMetadata(
            name.to_owned(),
        ));
    }
    Ok(index)
}

/// Metadato `GeoArrow` `geo` con la chiave `crs`: PROJJSON se la definizione e'
/// gia' un oggetto JSON, altrimenti la forma authority:code come stringa.
///
/// Unificazione B1.1: l'assemblaggio JSON e' unico in
/// [`plenora_kernels_geo::arrow_adapter::geo_metadata_json`] (stesso output
/// byte-per-byte); qui restano solo le validazioni con le varianti
/// d'errore strutturate del trasporto.
fn geo_metadata_json(crs: &str) -> Result<String, ArrowTransportError> {
    if crs.trim().is_empty() {
        return Err(ArrowTransportError::CrsRequired);
    }
    if crs.len() > MAX_CRS_DEFINITION_BYTES {
        return Err(ArrowTransportError::CrsTooLarge);
    }
    plenora_kernels_geo::arrow_adapter::geo_metadata_json(crs)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

fn geometry_output_field(name: &str, crs: &str) -> Result<Field, ArrowTransportError> {
    // Validazione CRS con le varianti strutturate del trasporto; la
    // costruzione del campo (metadati geoarrow.wkb + geo.crs +
    // geo.dimensions) e' unica in `arrow_adapter` (unificazione B1.1).
    geo_metadata_json(crs)?;
    // B1.3: la dimensionalita' dichiarata e' Xy ESPLICITO, non un default
    // silenzioso — ogni output di questo trasporto e' prodotto decodificando
    // in `Geometry<f64>` e ricodificando `to_wkb(CoordDimensions::xy())`,
    // quindi le celle sono sempre WKB 2D; gli input Z/M sono rifiutati a
    // compile-plan (`analyze_geo_contract`) prima di arrivare qui.
    // B1.4: per lo stesso motivo l'encoding e' `None` — le celle ricodificate
    // sono WKB ISO XY e la chiave `encoding` e' omessa (mai ereditata
    // dall'input, fingerprint invariato).
    plenora_kernels_geo::arrow_adapter::geometry_output_field_with_encoding(
        name,
        crs,
        GeometryDimensions::Xy,
        None,
    )
    .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

/// Risultato per cella di una trasformazione 1:1.
enum TransformedColumn {
    Binary(Vec<Option<Vec<u8>>>),
    Float64(Vec<Option<f64>>),
    UInt64(Vec<Option<u64>>),
    Utf8(Vec<Option<String>>),
    Bounds(Vec<Option<[f64; 4]>>),
}

/// Codifica una geometria gia' validata dal kernel in WKB 2D entro il limite
/// per cella.
fn encode_geometry(geometry: &Geometry<f64>) -> Result<Vec<u8>, ArrowTransportError> {
    let payload = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| OperationError::InvalidOutput(error.to_string()))?;
    if payload.len() as u64 > MAX_CELL_BYTES {
        return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
    }
    Ok(payload)
}

/// Applica `f` a ogni cella non-null preservando i null; il limite per cella
/// e' applicato prima di toccare i dati. Le righe sono indipendenti:
/// l'iterazione e' parallela (rayon) con collect indicizzato, quindi
/// l'ordine dell'output resta deterministico. Ogni kernel per-cella e'
/// thread-safe: puri Rust, GEOS con contesto thread-local, PROJ con
/// pipeline thread-local (vedi il ramo `Reproject`).
fn map_nullable<T: Send>(
    cells: &BinaryArray,
    f: impl Fn(&[u8]) -> Result<Option<T>, ArrowTransportError> + Sync,
) -> Result<Vec<Option<T>>, ArrowTransportError> {
    let cell_values: Vec<Option<&[u8]>> = cells.iter().collect();
    // ADR-0001: come la primitiva omonima in arrow_adapter — i `Result`
    // per riga prima (ordine preservato dal collect indicizzato), il primo
    // errore IN ORDINE DI RIGA poi, dal collect sequenziale; mai la
    // selezione non deterministica di rayon.
    let results: Vec<Result<Option<T>, ArrowTransportError>> = cell_values
        .into_par_iter()
        .map(|cell| match cell {
            None => Ok(None),
            Some(payload) => {
                if payload.len() as u64 > MAX_CELL_BYTES {
                    return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
                }
                f(payload)
            }
        })
        .collect();
    results.into_iter().collect()
}

// Dispatch per operazione intenzionalmente monolitico: ogni braccio e' un
// caso della tabella operazione -> kernel; la scomposizione strutturale e'
// rimandata a una fase dedicata.
#[allow(clippy::too_many_lines)]
fn transform_cells(
    params: &TransformArrowSchema,
    cells: &BinaryArray,
) -> Result<TransformedColumn, ArrowTransportError> {
    let operation = params.operation;
    match operation {
        ArrowOperation::Centroid | ArrowOperation::ConvexHull | ArrowOperation::Envelope => {
            let kernel = operation
                .geometry_kernel()
                .ok_or(ArrowTransportError::Internal("operazione geometrica senza kernel"))?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                Ok(transform_wkb(kernel, payload).map(Some)?)
            })?))
        }
        ArrowOperation::Buffer => {
            let distance = params.required_distance()?;
            let cap = BufferCapStyle::from(params.cap.unwrap_or(BufferCap::Round));
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&buffer_with_cap(&geometry, distance, cap)?).map(Some)
            })?))
        }
        ArrowOperation::Simplify => {
            let tolerance = params.required_tolerance()?;
            let policy = SimplifyPolicy::from(
                params
                    .simplify_policy
                    .unwrap_or(SimplifyPolicyParam::DouglasPeucker),
            );
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&simplify_with_policy(&geometry, tolerance, policy)?).map(Some)
            })?))
        }
        ArrowOperation::Boundary => {
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&boundary(&geometry)?).map(Some)
            })?))
        }
        ArrowOperation::PointOnSurface => {
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                point_on_surface(&geometry)?
                    .map(|point| encode_geometry(&point))
                    .transpose()
            })?))
        }
        #[cfg(feature = "geos-backend")]
        ArrowOperation::MakeValid => {
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                // A differenza delle altre operazioni, l'input puo' essere
                // OGC-invalido: e' esattamente cio' che make_valid ripara.
                Ok(make_valid_wkb(payload, RepairMethod::Linework, true).map(Some)?)
            })?))
        }
        #[cfg(not(feature = "geos-backend"))]
        ArrowOperation::MakeValid => Err(ArrowTransportError::BackendUnavailable {
            operation: operation.name(),
            feature: "geos-backend",
        }),
        #[cfg(feature = "proj-backend")]
        ArrowOperation::Reproject => {
            let source = params
                .crs
                .as_deref()
                .ok_or(ArrowTransportError::CrsRequired)?
                .to_owned();
            let target = params.required_target_crs()?.to_owned();
            Ok(TransformedColumn::Binary(map_nullable(
                cells,
                move |payload| {
                    let geometry = geometry_from_wkb(payload)?;
                    // Una pipeline PROJ per thread (PROJ non e' Sync), riusata su
                    // tutte le celle del batch e ricreata solo se cambia coppia.
                    REPROJECTOR.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let stale = slot
                            .as_ref()
                            .is_none_or(|(s, t, _)| s != &source || t != &target);
                        if stale {
                            *slot = Some((
                                source.clone(),
                                target.clone(),
                                Reprojector::new(&source, &target, MAX_CELL_COORDINATES)?,
                            ));
                        }
                        let (_, _, reprojector) = slot
                            .as_ref()
                            .ok_or(ArrowTransportError::Internal("pipeline appena creata assente"))?;
                        let reprojected = reprojector.reproject(&geometry)?;
                        encode_geometry(&reprojected).map(Some)
                    })
                },
            )?))
        }
        #[cfg(not(feature = "proj-backend"))]
        ArrowOperation::Reproject => Err(ArrowTransportError::BackendUnavailable {
            operation: operation.name(),
            feature: "proj-backend",
        }),
        ArrowOperation::Area => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                Ok(area(&geometry).map(Some)?)
            },
        )?)),
        ArrowOperation::Length => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                Ok(length(&geometry).map(Some)?)
            },
        )?)),
        ArrowOperation::Perimeter => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                Ok(perimeter(&geometry).map(Some)?)
            },
        )?)),
        ArrowOperation::VertexCount => {
            Ok(TransformedColumn::UInt64(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                Ok(vertex_count(&geometry).map(Some)?)
            })?))
        }
        ArrowOperation::Bounds => Ok(TransformedColumn::Bounds(map_nullable(cells, |payload| {
            let geometry = geometry_from_wkb(payload)?;
            Ok(bounds(&geometry)?)
        })?)),
        ArrowOperation::ToWkt => Ok(TransformedColumn::Utf8(map_nullable(cells, |payload| {
            let geometry = geometry_from_wkb(payload)?;
            Ok(to_wkt(&geometry).map(Some)?)
        })?)),
        ArrowOperation::AffineTransform => {
            let coefficients: [f64; 6] = params
                .coefficients
                .as_deref()
                .ok_or(ArrowTransportError::Internal("coefficients validato assente"))?
                .try_into()
                .map_err(|_| {
                    ArrowTransportError::Internal("coefficients validato non di 6 elementi")
                })?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&affine_transform(&geometry, coefficients)?).map(Some)
            })?))
        }
        ArrowOperation::Translate => {
            let x = params.required_f64("x_offset", params.x_offset)?;
            let y = params.required_f64("y_offset", params.y_offset)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&translate(&geometry, x, y)?).map(Some)
            })?))
        }
        ArrowOperation::Scale => {
            let x_factor = params.required_f64("x_factor", params.x_factor)?;
            let y_factor = params.required_f64("y_factor", params.y_factor)?;
            let origin = geo::Point::new(
                params.x_origin.unwrap_or(0.0),
                params.y_origin.unwrap_or(0.0),
            );
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&scale_about(&geometry, x_factor, y_factor, origin)?).map(Some)
            })?))
        }
        ArrowOperation::Rotate => {
            let degrees = params.required_f64("degrees", params.degrees)?;
            let origin = geo::Point::new(
                params.x_origin.unwrap_or(0.0),
                params.y_origin.unwrap_or(0.0),
            );
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&rotate_about(&geometry, degrees, origin)?).map(Some)
            })?))
        }
        ArrowOperation::ConcaveHull => {
            let concavity = params.required_f64("concavity", params.concavity)?;
            let length_threshold = params.length_threshold.unwrap_or(0.0);
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&concave_hull(
                    &geometry,
                    concavity,
                    length_threshold,
                    MAX_CELL_COORDINATES,
                )?)
                .map(Some)
            })?))
        }
        ArrowOperation::Densify => {
            let max_segment_length =
                params.required_f64("max_segment_length", params.max_segment_length)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&densify(
                    &geometry,
                    max_segment_length,
                    MAX_CELL_COORDINATES,
                )?)
                .map(Some)
            })?))
        }
        ArrowOperation::SnapToGrid => {
            let grid_size = params.required_f64("grid_size", params.grid_size)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&snap_to_grid(&geometry, grid_size)?).map(Some)
            })?))
        }
        ArrowOperation::LineSubstring => {
            let start = params.required_f64("start_ratio", params.start_ratio)?;
            let end = params.required_f64("end_ratio", params.end_ratio)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                let line = expect_line_string(&geometry, operation.name())?;
                line_substring(&line, start, end)?
                    .map(|output| encode_geometry(&output))
                    .transpose()
            })?))
        }
        ArrowOperation::LineInterpolatePoint => {
            let ratio = params.required_f64("ratio", params.ratio)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                let line = expect_line_string(&geometry, operation.name())?;
                line_interpolate_point(&line, ratio)?
                    .map(|point| encode_geometry(&Geometry::Point(point)))
                    .transpose()
            })?))
        }
        ArrowOperation::GeodesicLineLength => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                let line = expect_line_string(&geometry, operation.name())?;
                Ok(geodesic_line_length_m(&line).map(Some)?)
            },
        )?)),
        ArrowOperation::GeodesicArea => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                match &geometry {
                    Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {}
                    other => {
                        return Err(ArrowTransportError::WrongGeometryType {
                            operation: operation.name(),
                            expected: "Polygon/MultiPolygon",
                            actual: geometry_type_name(other).to_owned(),
                        })
                    }
                }
                Ok(geodesic_area_m2(&geometry).map(Some)?)
            },
        )?)),
        ArrowOperation::Explode
        | ArrowOperation::Dissolve
        | ArrowOperation::LineBuilder
        | ArrowOperation::PolygonBuilder
        | ArrowOperation::Voronoi
        | ArrowOperation::FromCoords
        | ArrowOperation::CleanTopology
        | ArrowOperation::GeometryDiagnostics
        | ArrowOperation::Delaunay
        | ArrowOperation::Polygonize
        | ArrowOperation::LineMerge => Err(ArrowTransportError::Arrow(
            "operazione non 1:1 nel percorso per-cella".to_owned(),
        )),
    }
}

const fn geometry_type_name(geometry: &Geometry<f64>) -> &'static str {
    match geometry {
        Geometry::Point(_) => "Point",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::Line(_) => "Line",
        Geometry::LineString(_) => "LineString",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::Polygon(_) => "Polygon",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
        Geometry::Rect(_) => "Rect",
        Geometry::Triangle(_) => "Triangle",
    }
}

fn expect_line_string(
    geometry: &Geometry<f64>,
    operation: &'static str,
) -> Result<geo::LineString<f64>, ArrowTransportError> {
    match geometry {
        Geometry::LineString(line) => Ok(line.clone()),
        other => Err(ArrowTransportError::WrongGeometryType {
            operation,
            expected: "LineString",
            actual: geometry_type_name(other).to_owned(),
        }),
    }
}

const fn spatial_predicate_name(predicate: SpatialPredicate) -> &'static str {
    match predicate {
        SpatialPredicate::Intersects => "intersects",
        SpatialPredicate::Disjoint => "disjoint",
        SpatialPredicate::Contains => "contains",
        SpatialPredicate::Within => "within",
        SpatialPredicate::EqualsTopo => "equals_topo",
        SpatialPredicate::Covers => "covers",
        SpatialPredicate::CoveredBy => "covered_by",
        SpatialPredicate::ContainsProperly => "contains_properly",
        SpatialPredicate::Touches => "touches",
        SpatialPredicate::Crosses => "crosses",
        SpatialPredicate::Overlaps => "overlaps",
    }
}

fn expect_point(
    geometry: &Geometry<f64>,
    operation: &'static str,
) -> Result<geo::Point<f64>, ArrowTransportError> {
    match geometry {
        Geometry::Point(point) => Ok(*point),
        other => Err(ArrowTransportError::WrongGeometryType {
            operation,
            expected: "Point",
            actual: geometry_type_name(other).to_owned(),
        }),
    }
}

fn bounds_column_names(geometry_column: &str) -> [String; 4] {
    [
        format!("{geometry_column}_minx"),
        format!("{geometry_column}_miny"),
        format!("{geometry_column}_maxx"),
        format!("{geometry_column}_maxy"),
    ]
}

/// Applica l'operazione ai batch in input secondo la sua forma
/// (1:1, 1:N, N:1, collettiva, costruzione da coordinate).
///
/// # Errors
///
/// Propaga gli errori del percorso specifico della forma (validazione
/// parametri, limiti di risorse, kernel); `ArrowTransportError::Internal` se
/// una forma non e' coperta dal dispatch (difetto del trasporto).
pub fn transform_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    match params.operation.shape() {
        ArrowShape::OneToOne => one_to_one_batches(schema, batches, params),
        ArrowShape::OneToMany => explode_batches(schema, batches, params),
        ArrowShape::ManyToOne => collect_batches(schema, batches, params),
        ArrowShape::Collective => match params.operation {
            ArrowOperation::Voronoi => voronoi_batches(schema, batches, params),
            ArrowOperation::CleanTopology => clean_topology_batches(schema, batches, params),
            _ => Err(ArrowTransportError::Internal("shape Collective non coperta")),
        },
        ArrowShape::WholeToMany => match params.operation {
            ArrowOperation::Polygonize => polygonize_batches(schema, batches, params),
            ArrowOperation::LineMerge => line_merge_batches(schema, batches, params),
            _ => Err(ArrowTransportError::Internal("shape WholeToMany non coperta")),
        },
        ArrowShape::FromCoords => from_coords_batches(schema, batches, params),
        ArrowShape::Diagnostic => diagnostics_batches(schema, batches, params),
    }
}

/// Handle prepared delle operazioni 1:1 (V2).
///
/// Indice di colonna e schema di output sono risolti UNA volta per kernel,
/// non per batch — il lavoro che `one_to_one_batches` rifaceva a ogni
/// chiamata (clone dei `Field` con le mappe metadata, serializzazione JSON
/// del metadato `geo`, ricerca per nome).
pub struct OneToOnePrepared {
    geometry_index: usize,
    output_schema: SchemaRef,
}

/// Risolve l'handle prepared di un'operazione 1:1 (V2).
///
/// # Errors
///
/// Come `one_to_one_batches` per la parte di risoluzione (colonna
/// geometria assente, CRS richiesto assente, operazione non coperta).
pub fn prepare_one_to_one(
    schema: &SchemaRef,
    params: &TransformArrowSchema,
) -> Result<OneToOnePrepared, ArrowTransportError> {
    let operation = params.operation;
    let geometry_column = params.geometry_column();
    let output_crs = match operation {
        ArrowOperation::Reproject => params.required_target_crs()?,
        _ => params
            .crs
            .as_deref()
            .ok_or(ArrowTransportError::CrsRequired)?,
    };
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    if operation.produces_geometry() {
        output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    } else {
        match operation {
            ArrowOperation::Area
            | ArrowOperation::Length
            | ArrowOperation::Perimeter
            | ArrowOperation::GeodesicLineLength
            | ArrowOperation::GeodesicArea => {
                output_fields[geometry_index] =
                    Field::new(operation.name(), DataType::Float64, true);
            }
            ArrowOperation::VertexCount => {
                output_fields[geometry_index] =
                    Field::new(operation.name(), DataType::UInt64, true);
            }
            ArrowOperation::ToWkt => {
                output_fields[geometry_index] = Field::new(WKT_COLUMN, DataType::Utf8, true);
            }
            ArrowOperation::Bounds => {
                let bounds_fields: Vec<Field> = bounds_column_names(geometry_column)
                    .into_iter()
                    .map(|name| Field::new(name, DataType::Float64, true))
                    .collect();
                output_fields.splice(geometry_index..=geometry_index, bounds_fields);
            }
            _ => {
                return Err(ArrowTransportError::Internal(
                    "operazione non geometrica non coperta",
                ))
            }
        }
    }
    let output_schema = std::sync::Arc::new(Schema::new_with_metadata(
        output_fields,
        schema.metadata().clone(),
    ));
    Ok(OneToOnePrepared {
        geometry_index,
        output_schema,
    })
}

/// Batch trasformato con l'handle prepared: nessuna ricostruzione di
/// schema per batch (`try_new` rivalida le colonne — fail-closed).
///
/// # Errors
///
/// Come `one_to_one_batches` per la parte dati (colonna non Binary,
/// errori del kernel di cella, schema incoerente con le colonne).
pub fn one_to_one_batch_prepared(
    batch: &RecordBatch,
    params: &TransformArrowSchema,
    prepared: &OneToOnePrepared,
) -> Result<RecordBatch, ArrowTransportError> {
    let geometry_index = prepared.geometry_index;
    let cells = batch
        .column(geometry_index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| ArrowTransportError::GeometryColumnNotBinary {
            name: params.geometry_column().to_owned(),
            actual: batch.column(geometry_index).data_type().to_string(),
        })?;
    let transformed = transform_cells(params, cells)?;
    let mut columns = batch.columns().to_vec();
    match transformed {
        TransformedColumn::Binary(values) => {
            columns[geometry_index] = std::sync::Arc::new(
                values.iter().map(|cell| cell.as_deref()).collect::<BinaryArray>(),
            );
        }
        TransformedColumn::Float64(values) => {
            columns[geometry_index] = std::sync::Arc::new(Float64Array::from(values));
        }
        TransformedColumn::UInt64(values) => {
            columns[geometry_index] = std::sync::Arc::new(UInt64Array::from(values));
        }
        TransformedColumn::Utf8(values) => {
            columns[geometry_index] = std::sync::Arc::new(StringArray::from(values));
        }
        TransformedColumn::Bounds(values) => {
            let arrays: Vec<plenora_core::arrow::array::ArrayRef> = (0..4)
                .map(|axis| {
                    std::sync::Arc::new(Float64Array::from(
                        values
                            .iter()
                            .map(|value| value.map(|bounds| bounds[axis]))
                            .collect::<Vec<Option<f64>>>(),
                    )) as plenora_core::arrow::array::ArrayRef
                })
                .collect();
            columns.splice(geometry_index..=geometry_index, arrays);
        }
    }
    RecordBatch::try_new(prepared.output_schema.clone(), columns)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

/// Operazioni 1:1: la colonna geometria e' sostituita dal risultato (Binary
/// GeoArrow-WKB, Float64, `UInt64`, Utf8 oppure quattro colonne Float64 per
/// `bounds`); tutte le altre colonne passano invariate; i null sono preservati.
fn one_to_one_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let prepared = prepare_one_to_one(schema, params)?;
    let mut output_batches = Vec::with_capacity(batches.len());
    for batch in batches {
        output_batches.push(one_to_one_batch_prepared(batch, params, &prepared)?);
    }
    Ok((prepared.output_schema, output_batches))
}

fn batch_geometry_cells<'a>(
    batch: &'a RecordBatch,
    geometry_index: usize,
    geometry_column: &str,
) -> Result<&'a BinaryArray, ArrowTransportError> {
    batch
        .column(geometry_index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| ArrowTransportError::GeometryColumnNotBinary {
            name: geometry_column.to_owned(),
            actual: batch.column(geometry_index).data_type().to_string(),
        })
}

/// `explode` (1:N): ogni geometria non-null produce le sue componenti
/// nell'ordine del kernel; i null non producono righe figlie. Gli attributi
/// sono replicati sulle righe figlie e `__parent_index` (`UInt64`) riporta
/// l'indice globale della riga di input. L'espansione e' controllata
/// incrementalmente contro `max_output_rows` prima di codificare i figli.
fn explode_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let limit = params.required_max_output_rows()?;
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    output_fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let mut output_batches = Vec::with_capacity(batches.len());
    let mut total_rows = 0_u64;
    let mut row_offset = 0_u64;
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        let mut encoded: Vec<Vec<u8>> = Vec::new();
        let mut take_indices: Vec<u64> = Vec::new();
        let mut parents: Vec<u64> = Vec::new();
        for (row, cell) in cells.iter().enumerate() {
            let Some(payload) = cell else {
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            let geometry = geometry_from_wkb(payload)?;
            let parts: Vec<Geometry<f64>> = match params.operation {
                ArrowOperation::Explode => explode(&geometry)?,
                ArrowOperation::Delaunay => delaunay(&geometry, MAX_CELL_COORDINATES, limit)?
                    .into_iter()
                    .map(Geometry::Polygon)
                    .collect(),
                _ => {
                    return Err(ArrowTransportError::Internal(
                        "explode_batches: operazione non 1:N",
                    ))
                }
            };
            let next = total_rows
                .checked_add(parts.len() as u64)
                .ok_or(ArrowTransportError::StreamTooLarge)?;
            if next > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: next,
                    limit,
                });
            }
            total_rows = next;
            for part in &parts {
                encoded.push(encode_geometry(part)?);
                take_indices.push(row as u64);
                parents.push(row_offset + row as u64);
            }
        }
        let take_indices = UInt64Array::from(take_indices);
        let mut columns: Vec<plenora_core::arrow::array::ArrayRef> = Vec::with_capacity(batch.num_columns() + 1);
        for (index, column) in batch.columns().iter().enumerate() {
            if index == geometry_index {
                columns.push(std::sync::Arc::new(
                    encoded
                        .iter()
                        .map(|part| Some(part.as_slice()))
                        .collect::<BinaryArray>(),
                ));
            } else {
                columns.push(
                    plenora_core::arrow::select::take::take(column, &take_indices, None)
                        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
                );
            }
        }
        columns.push(std::sync::Arc::new(UInt64Array::from(parents)));
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
        row_offset += batch.num_rows() as u64;
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// Operazioni N:1 (`dissolve`, `line_builder`, `polygon_builder`): tutto
/// l'input produce una sola riga; il grouping resta nell'adapter. Le colonne
/// attributo non sono propagate (l'aggregazione e' un compito dell'adapter):
/// l'output contiene solo la colonna geometria. Input senza geometrie non
/// null (o punti insufficienti per i builder) produce una riga con geometria
/// null. I null sono ignorati dai builder, che mantengono l'ordine righe.
fn collect_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let operation = params.operation;
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut geometries: Vec<Option<Geometry<f64>>> = Vec::new();
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for cell in cells {
            match cell {
                None => geometries.push(None),
                Some(payload) => {
                    if payload.len() as u64 > MAX_CELL_BYTES {
                        return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
                    }
                    geometries.push(Some(geometry_from_wkb(payload)?));
                }
            }
        }
    }
    let result: Option<Geometry<f64>> = match operation {
        ArrowOperation::Dissolve => {
            let polygons: Vec<Geometry<f64>> = geometries.into_iter().flatten().collect();
            if polygons.is_empty() {
                None
            } else {
                Some(dissolve(&polygons)?)
            }
        }
        ArrowOperation::LineBuilder => line_from_ordered_points(&geometries)?,
        ArrowOperation::PolygonBuilder => polygon_from_ordered_points(&geometries)?,
        _ => {
            return Err(ArrowTransportError::Internal(
                "collect_batches: operazione non N:1",
            ))
        }
    };
    let limit = params.max_output_rows_limit();
    if limit == 0 {
        return Err(ArrowTransportError::OutputRowsExceeded { actual: 1, limit });
    }
    let value = result
        .map(|geometry| encode_geometry(&geometry))
        .transpose()?;
    let output_schema = std::sync::Arc::new(Schema::new_with_metadata(
        vec![geometry_output_field(geometry_column, output_crs)?],
        schema.metadata().clone(),
    ));
    let batch = RecordBatch::try_new(
        output_schema.clone(),
        vec![std::sync::Arc::new(BinaryArray::from_iter([
            value.as_deref()
        ]))],
    )
    .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    Ok((output_schema, vec![batch]))
}

/// `voronoi` (collettiva): i punti non-null producono una cella a testa
/// nell'ordine input; le righe null restano null e le posizioni riga sono
/// preservate. Il kernel impone il cap punti (`max_points`) e rifiuta input
/// non puntuali o meno di due punti.
fn voronoi_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let max_points =
        usize::try_from(params.max_points.unwrap_or(DEFAULT_MAX_POINTS)).map_err(|_| {
            ArrowTransportError::InvalidParameter {
                operation: params.operation.name(),
                name: "max_points",
                reason: "non rappresentabile su questa piattaforma",
            }
        })?;
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut points: Vec<Geometry<f64>> = Vec::new();
    let mut positions: Vec<u64> = Vec::new();
    let mut row_offset = 0_u64;
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for (row, cell) in cells.iter().enumerate() {
            let Some(payload) = cell else {
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            points.push(geometry_from_wkb(payload)?);
            positions.push(row_offset + row as u64);
        }
        row_offset += batch.num_rows() as u64;
    }
    let limit = params.max_output_rows_limit();
    if row_offset > limit {
        return Err(ArrowTransportError::OutputRowsExceeded {
            actual: row_offset,
            limit,
        });
    }
    let cells = voronoi_cells(&points, max_points)?;
    let mut encoded: Vec<Vec<u8>> = Vec::with_capacity(cells.len());
    for cell in &cells {
        encoded.push(encode_geometry(cell)?);
    }

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let mut output_batches = Vec::with_capacity(batches.len());
    let mut cursor = 0_usize;
    let mut row_offset = 0_u64;
    for batch in batches {
        let mut values: Vec<Option<&[u8]>> = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() as u64 {
            if cursor < positions.len() && positions[cursor] == row_offset + row {
                values.push(Some(encoded[cursor].as_slice()));
                cursor += 1;
            } else {
                values.push(None);
            }
        }
        row_offset += batch.num_rows() as u64;
        let mut columns = batch.columns().to_vec();
        columns[geometry_index] = std::sync::Arc::new(BinaryArray::from_iter(values));
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// `clean_topology` (collettiva): cleanup ordinato dell'intera tabella
/// (gap close morfologico e sovrapposizioni first-row-wins). Output allineato
/// all'input: attributi invariati, geometria ripulita; i null restano null e
/// le righe assorbite da una riga precedente diventano null. Input non
/// poligonali o invalidi sono rifiutati (la riparazione spetta a `make_valid`).
fn clean_topology_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let snap_tolerance = params
        .snap_tolerance
        .ok_or_else(|| ArrowTransportError::MissingParameter {
            operation: params.operation.name(),
            name: "snap_tolerance",
        })?;
    let remove_overlaps = params.remove_overlaps.unwrap_or(true);
    let fill_gaps = params.fill_gaps.unwrap_or(true);
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut geometries: Vec<Geometry<f64>> = Vec::new();
    let mut positions: Vec<u64> = Vec::new();
    let mut row_offset = 0_u64;
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for (row, cell) in cells.iter().enumerate() {
            let Some(payload) = cell else {
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            geometries.push(geometry_from_wkb(payload)?);
            positions.push(row_offset + row as u64);
        }
        row_offset += batch.num_rows() as u64;
    }
    let cleaned = clean_valid_polygon_topology(
        &geometries,
        snap_tolerance,
        remove_overlaps,
        fill_gaps,
        MAX_ROWS,
        MAX_CLEAN_VERTICES,
    )?;
    let mut encoded: Vec<Option<Vec<u8>>> = Vec::with_capacity(cleaned.len());
    for geometry in &cleaned {
        encoded.push(
            geometry
                .as_ref()
                .map(encode_geometry)
                .transpose()?,
        );
    }

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let mut output_batches = Vec::with_capacity(batches.len());
    let mut cursor = 0_usize;
    let mut row_offset = 0_u64;
    for batch in batches {
        let mut values: Vec<Option<&[u8]>> = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() as u64 {
            if cursor < positions.len() && positions[cursor] == row_offset + row {
                values.push(encoded[cursor].as_deref());
                cursor += 1;
            } else {
                values.push(None);
            }
        }
        row_offset += batch.num_rows() as u64;
        let mut columns = batch.columns().to_vec();
        columns[geometry_index] = std::sync::Arc::new(BinaryArray::from_iter(values));
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// `geometry_diagnostics` (1:1 struct): la colonna geometria e' sostituita
/// da colonne esplicative (`geometry_type`, `coordinate_count`, `is_empty`,
/// `is_finite`, `is_valid`, `validity_reason`, `bounds_minx/miny/maxx/maxy`).
/// A differenza delle altre operazioni l'input OGC-invalido e' ACCETTATO
/// (solo il contratto strutturale WKB e' verificato): diagnosticare geometrie
/// invalide e' lo scopo dell'operazione.
fn diagnostics_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_column = params.geometry_column();
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let diagnostic_fields = vec![
        Field::new("geometry_type", DataType::Utf8, true),
        Field::new("coordinate_count", DataType::UInt64, true),
        Field::new("is_empty", DataType::Boolean, true),
        Field::new("is_finite", DataType::Boolean, true),
        Field::new("is_valid", DataType::Boolean, true),
        Field::new("validity_reason", DataType::Utf8, true),
        Field::new("bounds_minx", DataType::Float64, true),
        Field::new("bounds_miny", DataType::Float64, true),
        Field::new("bounds_maxx", DataType::Float64, true),
        Field::new("bounds_maxy", DataType::Float64, true),
    ];
    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields.splice(
        geometry_index..=geometry_index,
        diagnostic_fields.iter().cloned(),
    );
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let mut output_batches = Vec::with_capacity(batches.len());
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        let mut geometry_type: Vec<Option<String>> = Vec::with_capacity(batch.num_rows());
        let mut coordinate_count: Vec<Option<u64>> = Vec::with_capacity(batch.num_rows());
        let mut is_empty: Vec<Option<bool>> = Vec::with_capacity(batch.num_rows());
        let mut is_finite: Vec<Option<bool>> = Vec::with_capacity(batch.num_rows());
        let mut is_valid: Vec<Option<bool>> = Vec::with_capacity(batch.num_rows());
        let mut validity_reason: Vec<Option<String>> = Vec::with_capacity(batch.num_rows());
        let mut bounds: Vec<Option<[f64; 4]>> = Vec::with_capacity(batch.num_rows());
        for cell in cells {
            let Some(payload) = cell else {
                geometry_type.push(None);
                coordinate_count.push(None);
                is_empty.push(None);
                is_finite.push(None);
                is_valid.push(None);
                validity_reason.push(None);
                bounds.push(None);
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            plenora_kernels_geo::validate_wkb_contract(payload)?;
            let geometry = Wkb(payload)
                .to_geo()
                .map_err(|error| ArrowTransportError::Geometry(format!("WKB non valido: {error}")))?;
            let diagnostics = geometry_diagnostics(&geometry)?;
            geometry_type.push(Some(diagnostics.geometry_type.to_owned()));
            coordinate_count.push(Some(diagnostics.coordinate_count));
            is_empty.push(Some(diagnostics.is_empty));
            is_finite.push(Some(diagnostics.is_finite));
            is_valid.push(Some(diagnostics.is_valid));
            validity_reason.push(diagnostics.validity_reason);
            bounds.push(diagnostics.bounds);
        }
        let mut columns = batch.columns().to_vec();
        let diagnostic_columns: Vec<plenora_core::arrow::array::ArrayRef> = vec![
            std::sync::Arc::new(StringArray::from(geometry_type)),
            std::sync::Arc::new(UInt64Array::from(coordinate_count)),
            std::sync::Arc::new(plenora_core::arrow::array::BooleanArray::from(is_empty)),
            std::sync::Arc::new(plenora_core::arrow::array::BooleanArray::from(is_finite)),
            std::sync::Arc::new(plenora_core::arrow::array::BooleanArray::from(is_valid)),
            std::sync::Arc::new(StringArray::from(validity_reason)),
            std::sync::Arc::new(Float64Array::from(
                bounds.iter().map(|b| b.map(|b| b[0])).collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                bounds.iter().map(|b| b.map(|b| b[1])).collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                bounds.iter().map(|b| b.map(|b| b[2])).collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                bounds.iter().map(|b| b.map(|b| b[3])).collect::<Vec<_>>(),
            )),
        ];
        columns.splice(geometry_index..=geometry_index, diagnostic_columns);
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// Raccoglie tutte le celle non-null dell'intera tabella in una
/// `GeometryCollection` per i kernel collettivi (`polygonize`, `line_merge`).
fn collect_linework(
    batches: &[RecordBatch],
    geometry_index: usize,
    geometry_column: &str,
) -> Result<Geometry<f64>, ArrowTransportError> {
    let mut lines = Vec::new();
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for cell in cells {
            let Some(payload) = cell else {
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            lines.push(geometry_from_wkb(payload)?);
        }
    }
    Ok(Geometry::GeometryCollection(lines.into()))
}

/// Output di sole geometrie (una riga per pezzo) per i kernel collettivi,
/// con colonna di classificazione opzionale.
fn geometry_rows_output(
    schema: &SchemaRef,
    geometry_column: &str,
    output_crs: &str,
    rows: &[(Option<Vec<u8>>, Option<&'static str>)],
    with_class: bool,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let mut fields = vec![geometry_output_field(geometry_column, output_crs)?];
    if with_class {
        fields.push(Field::new(CLASS_COLUMN, DataType::Utf8, false));
    }
    let output_schema =
        std::sync::Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));
    let mut columns: Vec<plenora_core::arrow::array::ArrayRef> = vec![std::sync::Arc::new(
        rows.iter().map(|row| row.0.as_deref()).collect::<BinaryArray>(),
    )];
    if with_class {
        let classes: Vec<&'static str> = rows
            .iter()
            .map(|row| row.1.ok_or(ArrowTransportError::Internal("classe mancante")))
            .collect::<Result<_, _>>()?;
        columns.push(std::sync::Arc::new(StringArray::from(classes)));
    }
    let batch = RecordBatch::try_new(output_schema.clone(), columns)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    Ok((output_schema, vec![batch]))
}

/// `polygonize` (collettiva, richiede `geos-backend`): tutte le linee
/// non-null dell'input sono nodate e poligonizzate; l'output contiene una
/// riga per poligono e per residuo, classificati in `__class`
/// (`polygon`/`cut_edge`/`dangle`/`invalid_ring`). Nessun attributo
/// propagato, come per `dissolve`.
#[cfg(feature = "geos-backend")]
fn polygonize_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;
    let linework = collect_linework(batches, geometry_index, geometry_column)?;
    let result = polygonize_linework(
        &linework,
        params.node_input.unwrap_or(true),
        params.require_complete.unwrap_or(false),
        MAX_CLEAN_VERTICES,
        MAX_NODING_WORK,
        params.max_output_rows_limit(),
        MAX_CLEAN_VERTICES,
    )?;
    let mut rows: Vec<(Option<Vec<u8>>, Option<&'static str>)> = Vec::new();
    for polygon in &result.polygons {
        rows.push((
            Some(encode_geometry(&Geometry::Polygon(polygon.clone()))?),
            Some("polygon"),
        ));
    }
    for (class, lines) in [
        ("cut_edge", &result.cut_edges),
        ("dangle", &result.dangles),
        ("invalid_ring", &result.invalid_ring_lines),
    ] {
        for line in lines {
            rows.push((
                Some(encode_geometry(&Geometry::LineString(line.clone()))?),
                Some(class),
            ));
        }
    }
    geometry_rows_output(schema, geometry_column, output_crs, &rows, true)
}

#[cfg(not(feature = "geos-backend"))]
const fn polygonize_batches(
    _schema: &SchemaRef,
    _batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    Err(ArrowTransportError::BackendUnavailable {
        operation: params.operation.name(),
        feature: "geos-backend",
    })
}

/// `line_merge` (collettiva): le linee non-null dell'intera tabella sono
/// mergiate nei cammini massimali (barriera ai nodi di grado diverso da 2);
/// output di sole geometrie, una riga per linea mergiata.
fn line_merge_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;
    let linework = collect_linework(batches, geometry_index, geometry_column)?;
    let merged = line_merge(
        &linework,
        MAX_CLEAN_VERTICES,
        params.max_output_rows_limit(),
    )?;
    let mut rows: Vec<(Option<Vec<u8>>, Option<&'static str>)> = Vec::new();
    for line in &merged {
        rows.push((
            Some(encode_geometry(&Geometry::LineString(line.clone()))?),
            None,
        ));
    }
    geometry_rows_output(schema, geometry_column, output_crs, &rows, false)
}

/// Estrae i valori di una colonna numerica come float64 opzionali.
fn numeric_values(
    batch: &RecordBatch,
    index: usize,
    name: &str,
) -> Result<Vec<Option<f64>>, ArrowTransportError> {
    let column = batch.column(index);
    match column.data_type() {
        DataType::Float64 => Ok(column
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or(ArrowTransportError::Internal("tipo verificato Float64"))?
            .iter()
            .collect()),
        DataType::Int64 => {
            // Guardia di range (R5.4): oltre 2^53 in valore assoluto la
            // conversione i64 -> f64 non e' esatta e sposterebbe la
            // coordinata in silenzio; si rifiuta la riga con errore
            // tipizzato invece di produrre una geometria imprecisa.
            const MAX_EXACT: u64 = 1_u64 << 53;
            // Esattezza garantita dalla guardia: qui |x| <= 2^53, quindi
            // ogni i64 ammesso ha un f64 esattamente uguale.
            #[allow(clippy::cast_precision_loss)]
            let exact = |x: i64| -> Result<f64, ArrowTransportError> {
                if x.unsigned_abs() > MAX_EXACT {
                    Err(ArrowTransportError::IntegerCoordinateTooLarge {
                        name: name.to_owned(),
                    })
                } else {
                    Ok(x as f64)
                }
            };
            column
                .as_any()
                .downcast_ref::<plenora_core::arrow::array::Int64Array>()
                .ok_or(ArrowTransportError::Internal("tipo verificato Int64"))?
                .iter()
                .map(|value| value.map(exact).transpose())
                .collect()
        }
        other => Err(ArrowTransportError::ColumnNotNumeric {
            name: name.to_owned(),
            actual: other.to_string(),
        }),
    }
}

/// `from_coords` (1:1 senza colonna geometria in input): due colonne
/// numeriche (default `x`/`y`) producono una colonna geometria Point
/// aggiunta in coda; null in x o y -> geometria null; coordinate non finite
/// sono rifiutate dal kernel (fail-closed). Tutte le colonne di input
/// passano invariate.
fn from_coords_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    if schema.column_with_name(geometry_column).is_some() {
        return Err(ArrowTransportError::OutputColumnExists(
            geometry_column.to_owned(),
        ));
    }
    let (x_index, x_field) = schema
        .column_with_name(params.x_column())
        .ok_or_else(|| ArrowTransportError::MissingColumn(params.x_column().to_owned()))?;
    let (y_index, y_field) = schema
        .column_with_name(params.y_column())
        .ok_or_else(|| ArrowTransportError::MissingColumn(params.y_column().to_owned()))?;
    for (name, field) in [(params.x_column(), x_field), (params.y_column(), y_field)] {
        if !matches!(field.data_type(), DataType::Float64 | DataType::Int64) {
            return Err(ArrowTransportError::ColumnNotNumeric {
                name: name.to_owned(),
                actual: field.data_type().to_string(),
            });
        }
    }

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields.push(geometry_output_field(geometry_column, output_crs)?);
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let limit = params.max_output_rows_limit();
    let mut output_batches = Vec::with_capacity(batches.len());
    for batch in batches {
        if batch.num_rows() as u64 > limit {
            return Err(ArrowTransportError::OutputRowsExceeded {
                actual: batch.num_rows() as u64,
                limit,
            });
        }
        let xs = numeric_values(batch, x_index, params.x_column())?;
        let ys = numeric_values(batch, y_index, params.y_column())?;
        let mut points: Vec<Option<Vec<u8>>> = Vec::with_capacity(batch.num_rows());
        for (x, y) in xs.into_iter().zip(ys) {
            match (x, y) {
                (Some(x), Some(y)) => {
                    let point = point_from_lon_lat(x, y)?;
                    points.push(Some(encode_geometry(&point)?));
                }
                _ => points.push(None),
            }
        }
        let mut columns = batch.columns().to_vec();
        columns.push(std::sync::Arc::new(
            points
                .iter()
                .map(|point| point.as_deref())
                .collect::<BinaryArray>(),
        ));
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// Codifica i batch in un payload Arrow IPC stream entro i limiti di risorse.
///
/// # Errors
///
/// `ArrowTransportError::TooManyBatches` se i batch superano il limite,
/// `ArrowTransportError::Arrow` per errori di codifica IPC,
/// `ArrowTransportError::StreamTooLarge` se il payload supera
/// `MAX_STREAM_BYTES`.
pub fn encode_ipc(
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>, ArrowTransportError> {
    if batches.len() > MAX_BATCHES {
        return Err(ArrowTransportError::TooManyBatches(batches.len()));
    }
    let mut payload = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut payload, schema)
            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
        }
        writer
            .finish()
            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    }
    if payload.len() as u64 > MAX_STREAM_BYTES {
        return Err(ArrowTransportError::StreamTooLarge);
    }
    Ok(payload)
}

/// Pipeline completa envelope -> Arrow -> kernel -> Arrow -> envelope.
/// Il CRS e' trattato come metadato opaco: la verifica semantica spetta al
/// livello comandi.
///
/// # Errors
///
/// `ArrowTransportError::UnsupportedSchemaVersion` per versioni non
/// supportate, `ArrowTransportError::TooManyRows` / `CrsRequired` /
/// `RowCountMismatch` per violazioni del contratto di schema; propaga gli
/// errori di envelope (`EnvelopeReader`/`EnvelopeWriter`), di decodifica
/// (`decode_ipc`), di trasformazione (`transform_batches`) e di codifica
/// (`encode_ipc`).
pub fn transform_arrow(
    reader: impl Read,
    writer: impl Write,
    schema: &TransformArrowSchema,
) -> Result<TransformArrowSummary, ArrowTransportError> {
    if schema.schema_version != TransformArrowSchema::VERSION {
        return Err(ArrowTransportError::UnsupportedSchemaVersion(
            schema.schema_version,
        ));
    }
    if schema.row_count > MAX_ROWS {
        return Err(ArrowTransportError::TooManyRows(schema.row_count));
    }
    schema.validate_parameters()?;
    if schema.crs.is_none() {
        return Err(ArrowTransportError::CrsRequired);
    }

    let payload = EnvelopeReader::new(reader)?.read_payload()?;
    let (input_schema, batches) = decode_ipc(&payload)?;
    let rows: u64 = batches.iter().map(|batch| batch.num_rows() as u64).sum();
    if rows != schema.row_count {
        return Err(ArrowTransportError::RowCountMismatch {
            schema: schema.row_count,
            stream: rows,
        });
    }

    let (output_schema, output_batches) = transform_batches(&input_schema, &batches, schema)?;
    let output_rows: u64 = output_batches
        .iter()
        .map(|batch| batch.num_rows() as u64)
        .sum();
    let output_payload = encode_ipc(&output_schema, &output_batches)?;
    let mut envelope = EnvelopeWriter::new(writer, output_payload.len() as u64)?;
    envelope.write_payload(&output_payload)?;
    let (_, checksum) = envelope.finish()?;
    Ok(TransformArrowSummary {
        rows,
        output_rows,
        checksum,
    })
}

// --- Forma binary + lineage (Fase C) ---------------------------------------

/// Operazioni binarie su due envelope v3 (left/right).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PairOperation {
    #[serde(rename = "sjoin")]
    SJoin,
    Distance,
    Nearest,
    Clip,
    Overlay,
    Within,
    CountPointsInPolygons,
    Intersection,
    Union,
    Difference,
    SymmetricDifference,
    Predicate,
    HausdorffDistance,
    FrechetDistance,
    HaversineDistance,
    GeodesicDistance,
    Bearing,
    Split,
}

impl PairOperation {
    pub const ALL: [Self; 18] = [
        Self::SJoin,
        Self::Distance,
        Self::Nearest,
        Self::Clip,
        Self::Overlay,
        Self::Within,
        Self::CountPointsInPolygons,
        Self::Intersection,
        Self::Union,
        Self::Difference,
        Self::SymmetricDifference,
        Self::Predicate,
        Self::HausdorffDistance,
        Self::FrechetDistance,
        Self::HaversineDistance,
        Self::GeodesicDistance,
        Self::Bearing,
        Self::Split,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SJoin => "sjoin",
            Self::Distance => "distance",
            Self::Nearest => "nearest",
            Self::Clip => "clip",
            Self::Overlay => "overlay",
            Self::Within => "within",
            Self::CountPointsInPolygons => "count_points_in_polygons",
            Self::Intersection => "intersection",
            Self::Union => "union",
            Self::Difference => "difference",
            Self::SymmetricDifference => "symmetric_difference",
            Self::Predicate => "predicate",
            Self::HausdorffDistance => "hausdorff_distance",
            Self::FrechetDistance => "frechet_distance",
            Self::HaversineDistance => "haversine_distance",
            Self::GeodesicDistance => "geodesic_distance",
            Self::Bearing => "bearing",
            Self::Split => "split",
        }
    }

    /// Nome della voce di catalogo usata dal livello comandi per il requisito CRS.
    #[must_use]
    pub const fn catalog_name(self) -> &'static str {
        match self {
            Self::SJoin => "sjoin",
            Self::Distance => "geo_distance",
            Self::Nearest => "geo_nearest",
            Self::Clip => "geo_clip",
            Self::Overlay => "geo_overlay",
            Self::Within => "geo_within",
            Self::CountPointsInPolygons => "geo_count_points_in_polygons",
            Self::Intersection => "geo_intersection",
            Self::Union => "geo_union",
            Self::Difference => "geo_difference",
            Self::SymmetricDifference => "geo_symmetric_difference",
            // Tutti i predicati DE-9IM condividono famiglia e requisito CRS.
            Self::Predicate => "predicate_intersects",
            Self::HausdorffDistance => "hausdorff_distance",
            Self::FrechetDistance => "frechet_distance",
            Self::HaversineDistance => "haversine_distance",
            Self::GeodesicDistance => "geodesic_distance",
            Self::Bearing => "bearing",
            Self::Split => "split",
        }
    }

    const fn boolean_kernel(self) -> Option<BooleanOperation> {
        match self {
            Self::Intersection => Some(BooleanOperation::Intersection),
            Self::Union => Some(BooleanOperation::Union),
            Self::Difference => Some(BooleanOperation::Difference),
            Self::SymmetricDifference => Some(BooleanOperation::SymmetricDifference),
            _ => None,
        }
    }
}

/// Schema JSON del comando pair-arrow (`schema_version: 3`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairArrowSchema {
    pub schema_version: u32,
    pub operation: PairOperation,
    pub left_row_count: u64,
    pub right_row_count: u64,
    pub left_crs: Option<String>,
    pub right_crs: Option<String>,
    pub geometry_column: Option<String>,
    pub predicate: Option<JoinPredicate>,
    pub overlay_mode: Option<OverlayMode>,
    pub max_pairs: Option<u64>,
    pub max_comparisons: Option<u64>,
    pub max_results: Option<u64>,
    pub max_distance: Option<f64>,
    pub max_output_rows: Option<u64>,
    pub spatial_predicate: Option<SpatialPredicate>,
    pub max_coordinate_pairs: Option<u64>,
    pub tolerance: Option<f64>,
}

impl PairArrowSchema {
    pub const VERSION: u32 = 3;

    #[must_use]
    pub fn geometry_column(&self) -> &str {
        self.geometry_column
            .as_deref()
            .unwrap_or(DEFAULT_GEOMETRY_COLUMN)
    }

    #[must_use]
    pub fn max_output_rows_limit(&self) -> u64 {
        self.max_output_rows.unwrap_or(MAX_ROWS)
    }

    /// Verifica che i parametri presenti siano esattamente quelli previsti
    /// dall'operazione e che i valori siano nel dominio del kernel.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::UnexpectedParameter` se e' presente un parametro
    /// non previsto dall'operazione, `ArrowTransportError::MissingParameter`
    /// se ne manca uno obbligatorio, `ArrowTransportError::InvalidParameter`
    /// se un valore e' fuori dominio.
    // Tabella parametri per operazione intenzionalmente in un'unica funzione;
    // la scomposizione strutturale e' rimandata a una fase dedicata.
    #[allow(clippy::too_many_lines)]
    pub fn validate_parameters(&self) -> Result<(), ArrowTransportError> {
        let operation = self.operation.name();
        // Parametri ammessi per operazione: tutto il resto e' rifiutato
        // prima di toccare i dati.
        let allowed: &[&'static str] = match self.operation {
            PairOperation::SJoin => &["predicate", "max_pairs"],
            PairOperation::Distance => &["max_comparisons"],
            PairOperation::Nearest => &["max_comparisons", "max_results", "max_distance"],
            PairOperation::Overlay => &["overlay_mode", "max_pairs"],
            PairOperation::Within | PairOperation::CountPointsInPolygons => &["max_pairs"],
            PairOperation::Predicate => &["spatial_predicate"],
            PairOperation::HausdorffDistance | PairOperation::FrechetDistance => {
                &["max_coordinate_pairs"]
            }
            PairOperation::Split => &["tolerance"],
            // Operazioni senza parametri.
            PairOperation::Clip
            | PairOperation::Intersection
            | PairOperation::Union
            | PairOperation::Difference
            | PairOperation::SymmetricDifference
            | PairOperation::HaversineDistance
            | PairOperation::GeodesicDistance
            | PairOperation::Bearing => &[],
        };
        let mut present: Vec<(&'static str, bool)> = vec![
            ("predicate", self.predicate.is_some()),
            ("overlay_mode", self.overlay_mode.is_some()),
            ("max_pairs", self.max_pairs.is_some()),
            ("max_comparisons", self.max_comparisons.is_some()),
            ("max_results", self.max_results.is_some()),
            ("max_distance", self.max_distance.is_some()),
            ("spatial_predicate", self.spatial_predicate.is_some()),
            ("max_coordinate_pairs", self.max_coordinate_pairs.is_some()),
            ("tolerance", self.tolerance.is_some()),
        ];
        present.retain(|(_, is_present)| *is_present);
        for (name, _) in &present {
            if !allowed.contains(name) {
                return Err(ArrowTransportError::UnexpectedParameter { operation, name });
            }
        }
        let required = |name: &'static str, value: Option<u64>| {
            value.ok_or(ArrowTransportError::MissingParameter { operation, name })
        };
        let positive = |name: &'static str, value: u64| {
            if value == 0 {
                Err(ArrowTransportError::InvalidParameter {
                    operation,
                    name,
                    reason: "deve essere maggiore di zero",
                })
            } else {
                Ok(value)
            }
        };
        match self.operation {
            PairOperation::SJoin => {
                if self.predicate.is_none() {
                    return Err(ArrowTransportError::MissingParameter {
                        operation,
                        name: "predicate",
                    });
                }
                let max_pairs = positive("max_pairs", required("max_pairs", self.max_pairs)?)?;
                if max_pairs > MAX_PAIRS {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "max_pairs",
                        reason: "oltre il limite del protocollo coppie",
                    });
                }
            }
            PairOperation::Distance => {
                positive(
                    "max_comparisons",
                    required("max_comparisons", self.max_comparisons)?,
                )?;
            }
            PairOperation::Nearest => {
                positive(
                    "max_comparisons",
                    required("max_comparisons", self.max_comparisons)?,
                )?;
                positive("max_results", required("max_results", self.max_results)?)?;
                if self
                    .max_distance
                    .is_some_and(|value| !value.is_finite() || value < 0.0)
                {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "max_distance",
                        reason: "deve essere finita e non negativa",
                    });
                }
            }
            PairOperation::Overlay => {
                if self.overlay_mode.is_none() {
                    return Err(ArrowTransportError::MissingParameter {
                        operation,
                        name: "overlay_mode",
                    });
                }
                positive("max_pairs", required("max_pairs", self.max_pairs)?)?;
            }
            PairOperation::Within | PairOperation::CountPointsInPolygons => {
                let max_pairs = positive("max_pairs", required("max_pairs", self.max_pairs)?)?;
                if max_pairs > MAX_PAIRS {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "max_pairs",
                        reason: "oltre il limite del protocollo coppie",
                    });
                }
            }
            PairOperation::Predicate => {
                if self.spatial_predicate.is_none() {
                    return Err(ArrowTransportError::MissingParameter {
                        operation,
                        name: "spatial_predicate",
                    });
                }
            }
            PairOperation::HausdorffDistance | PairOperation::FrechetDistance => {
                positive(
                    "max_coordinate_pairs",
                    required("max_coordinate_pairs", self.max_coordinate_pairs)?,
                )?;
            }
            PairOperation::Split => {
                if self
                    .tolerance
                    .is_some_and(|value| !value.is_finite() || value < 0.0)
                {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "tolerance",
                        reason: "deve essere finita e non negativa",
                    });
                }
            }
            _ => {}
        }
        if let Some(limit) = self.max_output_rows {
            if limit > MAX_ROWS {
                return Err(ArrowTransportError::InvalidParameter {
                    operation,
                    name: "max_output_rows",
                    reason: "oltre il limite righe del trasporto",
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct PairArrowSummary {
    pub left_rows: u64,
    pub right_rows: u64,
    pub output_rows: u64,
    pub checksum: [u8; 32],
}

/// Lato di una coppia decodificato: schema, batch IPC e geometrie validate.
type DecodedSide = (SchemaRef, Vec<RecordBatch>, Vec<Option<Geometry<f64>>>);

/// Decodifica un lato (envelope + IPC + colonna geometria) e materializza le
/// geometrie validate: entrambi i lati sono verificati prima del calcolo.
fn decode_geometry_side(
    reader: impl Read,
    expected_rows: u64,
    geometry_column: &str,
    side: &'static str,
) -> Result<DecodedSide, ArrowTransportError> {
    if expected_rows > MAX_ROWS {
        return Err(ArrowTransportError::TooManyRows(expected_rows));
    }
    let payload = EnvelopeReader::new(reader)?.read_payload()?;
    let (schema, batches) = decode_ipc(&payload)?;
    let rows: u64 = batches.iter().map(|batch| batch.num_rows() as u64).sum();
    if rows != expected_rows {
        return Err(ArrowTransportError::PairRowCountMismatch {
            side,
            schema: expected_rows,
            stream: rows,
        });
    }
    let geometry_index = geometry_column_index(&schema, geometry_column)?;
    let mut geometries = Vec::with_capacity(
        usize::try_from(rows).map_err(|_| ArrowTransportError::TooManyRows(rows))?,
    );
    for batch in &batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for cell in cells {
            match cell {
                None => geometries.push(None),
                Some(payload) => {
                    if payload.len() as u64 > MAX_CELL_BYTES {
                        return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
                    }
                    geometries.push(Some(geometry_from_wkb(payload)?));
                }
            }
        }
    }
    Ok((schema, batches, geometries))
}

fn lineage_schema(nullable: bool, with_distance: bool) -> Schema {
    let mut fields = vec![
        Field::new(LEFT_INDEX_COLUMN, DataType::UInt64, nullable),
        Field::new(RIGHT_INDEX_COLUMN, DataType::UInt64, nullable),
    ];
    if with_distance {
        fields.push(Field::new(DISTANCE_COLUMN, DataType::Float64, false));
    }
    Schema::new(fields)
}

fn pairs_batch(
    pairs: &[(u64, u64, Option<f64>)],
    schema: &Schema,
) -> Result<RecordBatch, ArrowTransportError> {
    let left = UInt64Array::from_iter_values(pairs.iter().map(|pair| pair.0));
    let right = UInt64Array::from_iter_values(pairs.iter().map(|pair| pair.1));
    let mut columns: Vec<plenora_core::arrow::array::ArrayRef> =
        vec![std::sync::Arc::new(left), std::sync::Arc::new(right)];
    if schema.fields().len() == 3 {
        let distances: Vec<f64> = pairs
            .iter()
            .map(|pair| {
                pair.2
                    .ok_or(ArrowTransportError::Internal("distance mancante"))
            })
            .collect::<Result<_, _>>()?;
        columns.push(std::sync::Arc::new(Float64Array::from_iter_values(
            distances,
        )));
    }
    RecordBatch::try_new(std::sync::Arc::new(schema.clone()), columns)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

/// Colonna scalare allineata alle righe left da accodare agli attributi.
enum AppendedColumn {
    Boolean(Vec<Option<bool>>),
    UInt64(Vec<Option<u64>>),
    Float64(Vec<Option<f64>>),
}

/// Accoda una colonna scalare alle colonne left preservando i batch.
fn append_column_batches(
    left_schema: &SchemaRef,
    left_batches: &[RecordBatch],
    field: Field,
    values: &AppendedColumn,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let mut output_fields: Vec<Field> = left_schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields.push(field);
    let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
        output_fields,
        left_schema.metadata().clone(),
    ));
    let mut out_batches = Vec::with_capacity(left_batches.len());
    let mut offset = 0_usize;
    for batch in left_batches {
        let end = offset + batch.num_rows();
        let column: plenora_core::arrow::array::ArrayRef = match values {
            AppendedColumn::Boolean(values) => std::sync::Arc::new(
                plenora_core::arrow::array::BooleanArray::from(values[offset..end].to_vec()),
            ),
            AppendedColumn::UInt64(values) => {
                std::sync::Arc::new(UInt64Array::from(values[offset..end].to_vec()))
            }
            AppendedColumn::Float64(values) => {
                std::sync::Arc::new(Float64Array::from(values[offset..end].to_vec()))
            }
        };
        offset = end;
        let mut columns = batch.columns().to_vec();
        columns.push(column);
        out_batches.push(
            RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((out_schema, out_batches))
}

/// Sostituisce la colonna geometria left con i valori dati (null inclusi).
fn replace_geometry_batches(
    left_schema: &SchemaRef,
    left_batches: &[RecordBatch],
    geometry_column: &str,
    output_crs: &str,
    values: &[Option<Vec<u8>>],
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_index = geometry_column_index(left_schema, geometry_column)?;
    let mut output_fields: Vec<Field> = left_schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
        output_fields,
        left_schema.metadata().clone(),
    ));
    let mut out_batches = Vec::with_capacity(left_batches.len());
    let mut offset = 0_usize;
    for batch in left_batches {
        let end = offset + batch.num_rows();
        let mut columns = batch.columns().to_vec();
        columns[geometry_index] = std::sync::Arc::new(
            values[offset..end]
                .iter()
                .map(|value| value.as_deref())
                .collect::<BinaryArray>(),
        );
        offset = end;
        out_batches.push(
            RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((out_schema, out_batches))
}

/// Pipeline pair-arrow: due envelope v3 -> kernel binario -> envelope v3.
/// I CRS sono trattati come metadati opachi; l'uguaglianza semantica e'
/// verificata dal livello comandi.
///
/// # Errors
///
/// - `ArrowTransportError::UnsupportedSchemaVersion`: `schema_version` non
///   supportata;
/// - `ArrowTransportError::CrsRequired`: `left_crs` o `right_crs` assente;
/// - come `PairArrowSchema::validate_parameters` per i parametri;
/// - `ArrowTransportError::OutputRowsExceeded`: righe di output oltre
///   `max_output_rows`;
/// - `ArrowTransportError::SideLengthMismatch`: `row_count` left/right diversi
///   per le operazioni allineate per riga;
/// - `ArrowTransportError::Internal`: invariante interna violata (parametro
///   gia' validato assente — difetto del trasporto, non dell'input);
/// - propaga gli errori di decodifica dei due lati (envelope, IPC, limiti,
///   validazione WKB), dei kernel binari e di codifica dell'output
///   (`encode_ipc`, `EnvelopeWriter`).
// Pipeline unica su tutte le PairOperation: la lunghezza e' data dalla
// sequenza lineare dei casi del dispatcher sul contratto v3, non da
// complessita' logica (fase di pulizia: niente refactor strutturali).
#[allow(clippy::too_many_lines)]
pub fn pair_arrow(
    left_reader: impl Read,
    right_reader: impl Read,
    writer: impl Write,
    schema: &PairArrowSchema,
) -> Result<PairArrowSummary, ArrowTransportError> {
    if schema.schema_version != PairArrowSchema::VERSION {
        return Err(ArrowTransportError::UnsupportedSchemaVersion(
            schema.schema_version,
        ));
    }
    schema.validate_parameters()?;
    if schema.left_crs.is_none() || schema.right_crs.is_none() {
        return Err(ArrowTransportError::CrsRequired);
    }
    let geometry_column = schema.geometry_column();
    let (left_schema, left_batches, left) =
        decode_geometry_side(left_reader, schema.left_row_count, geometry_column, "left")?;
    let (_right_schema, _right_batches, right) = decode_geometry_side(
        right_reader,
        schema.right_row_count,
        geometry_column,
        "right",
    )?;
    let left_rows = schema.left_row_count;
    let right_rows = schema.right_row_count;
    let limit = schema.max_output_rows_limit();

    let (output_schema, output_batches): (SchemaRef, Vec<RecordBatch>) = match schema.operation {
        PairOperation::SJoin => {
            let pairs = spatial_join_nullable(
                &left,
                &right,
                schema
                    .predicate
                    .ok_or(ArrowTransportError::Internal("predicate validato assente"))?,
                schema
                    .max_pairs
                    .ok_or(ArrowTransportError::Internal("max_pairs validato assente"))?,
            )?;
            if pairs.len() as u64 > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: pairs.len() as u64,
                    limit,
                });
            }
            let out_schema = std::sync::Arc::new(lineage_schema(false, false));
            let flat: Vec<(u64, u64, Option<f64>)> = pairs
                .iter()
                .map(|pair| (pair.left, pair.right, None))
                .collect();
            let batch = pairs_batch(&flat, &out_schema)?;
            (out_schema, vec![batch])
        }
        PairOperation::Distance => {
            let distances = minimum_distances(
                &left,
                &right,
                schema
                    .max_comparisons
                    .ok_or(ArrowTransportError::Internal("max_comparisons validato assente"))?,
            )?;
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            // Output allineato a left: colonne invariate, `distance` in coda.
            let mut output_fields: Vec<Field> = left_schema
                .fields()
                .iter()
                .map(|field| field.as_ref().clone())
                .collect();
            output_fields.push(Field::new(DISTANCE_COLUMN, DataType::Float64, true));
            let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
                output_fields,
                left_schema.metadata().clone(),
            ));
            let mut out_batches = Vec::with_capacity(left_batches.len());
            let mut offset = 0_usize;
            for batch in &left_batches {
                let values: Vec<Option<f64>> =
                    distances[offset..offset + batch.num_rows()].to_vec();
                offset += batch.num_rows();
                let mut columns = batch.columns().to_vec();
                columns.push(std::sync::Arc::new(Float64Array::from(values)));
                out_batches.push(
                    RecordBatch::try_new(out_schema.clone(), columns)
                        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
                );
            }
            (out_schema, out_batches)
        }
        PairOperation::Nearest => {
            let matches = nearest_matches(
                &left,
                &right,
                schema.max_distance,
                schema
                    .max_comparisons
                    .ok_or(ArrowTransportError::Internal("max_comparisons validato assente"))?,
                schema
                    .max_results
                    .ok_or(ArrowTransportError::Internal("max_results validato assente"))?,
            )?;
            if matches.len() as u64 > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: matches.len() as u64,
                    limit,
                });
            }
            let out_schema = std::sync::Arc::new(lineage_schema(false, true));
            let flat: Vec<(u64, u64, Option<f64>)> = matches
                .iter()
                .map(|m| (m.left, m.right, Some(m.distance)))
                .collect();
            let batch = pairs_batch(&flat, &out_schema)?;
            (out_schema, vec![batch])
        }
        PairOperation::Clip => {
            let masks: Vec<Geometry<f64>> = right.into_iter().flatten().collect();
            let mut left_values: Vec<Geometry<f64>> = Vec::new();
            let mut positions: Vec<u64> = Vec::new();
            for (index, geometry) in left.iter().enumerate() {
                if let Some(geometry) = geometry {
                    left_values.push(geometry.clone());
                    positions.push(index as u64);
                }
            }
            let clipped = clip_to_mask(&left_values, &masks)?;
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let output_crs = schema
                .left_crs
                .as_deref()
                .ok_or(ArrowTransportError::Internal("left_crs validato assente"))?;
            let mut output_fields: Vec<Field> = left_schema
                .fields()
                .iter()
                .map(|field| field.as_ref().clone())
                .collect();
            let geometry_index = geometry_column_index(&left_schema, geometry_column)?;
            output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
            let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
                output_fields,
                left_schema.metadata().clone(),
            ));
            let mut encoded: Vec<Option<Vec<u8>>> = Vec::with_capacity(clipped.len());
            for geometry in &clipped {
                encoded.push(
                    geometry
                        .as_ref()
                        .map(encode_geometry)
                        .transpose()?,
                );
            }
            let mut out_batches = Vec::with_capacity(left_batches.len());
            let mut cursor = 0_usize;
            let mut row_offset = 0_u64;
            for batch in &left_batches {
                let mut values: Vec<Option<&[u8]>> = Vec::with_capacity(batch.num_rows());
                for row in 0..batch.num_rows() as u64 {
                    if cursor < positions.len() && positions[cursor] == row_offset + row {
                        values.push(encoded[cursor].as_deref());
                        cursor += 1;
                    } else {
                        values.push(None);
                    }
                }
                row_offset += batch.num_rows() as u64;
                let mut columns = batch.columns().to_vec();
                columns[geometry_index] = std::sync::Arc::new(BinaryArray::from_iter(values));
                out_batches.push(
                    RecordBatch::try_new(out_schema.clone(), columns)
                        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
                );
            }
            (out_schema, out_batches)
        }
        PairOperation::Overlay => {
            let filter = |geometries: &[Option<Geometry<f64>>]| {
                let mut values = Vec::new();
                let mut positions = Vec::new();
                for (index, geometry) in geometries.iter().enumerate() {
                    if let Some(geometry) = geometry {
                        values.push(geometry.clone());
                        positions.push(index as u64);
                    }
                }
                (values, positions)
            };
            let (left_values, left_positions) = filter(&left);
            let (right_values, right_positions) = filter(&right);
            let pieces = polygon_overlay(
                &left_values,
                &right_values,
                schema
                    .overlay_mode
                    .ok_or(ArrowTransportError::Internal("overlay_mode validato assente"))?,
                schema
                    .max_pairs
                    .ok_or(ArrowTransportError::Internal("max_pairs validato assente"))?,
                limit,
            )?;
            let output_crs = schema
                .left_crs
                .as_deref()
                .ok_or(ArrowTransportError::Internal("left_crs validato assente"))?;
            let out_schema = std::sync::Arc::new(Schema::new(vec![
                geometry_output_field(geometry_column, output_crs)?,
                Field::new(LEFT_INDEX_COLUMN, DataType::UInt64, true),
                Field::new(RIGHT_INDEX_COLUMN, DataType::UInt64, true),
            ]));
            let mut encoded: Vec<Vec<u8>> = Vec::with_capacity(pieces.len());
            let mut left_index: Vec<Option<u64>> = Vec::with_capacity(pieces.len());
            let mut right_index: Vec<Option<u64>> = Vec::with_capacity(pieces.len());
            for piece in &pieces {
                encoded.push(encode_geometry(&piece.geometry)?);
                // Indici prodotti dal kernel overlay: la conversione e'
                // totale; un u64 che non entra in usize (target a 32 bit)
                // e' un difetto del kernel, non un valore da troncare.
                left_index.push(match piece.left {
                    Some(index) => Some(left_positions[usize::try_from(index).map_err(
                        |_| ArrowTransportError::Internal("indice overlay left oltre usize"),
                    )?]),
                    None => None,
                });
                right_index.push(match piece.right {
                    Some(index) => Some(right_positions[usize::try_from(index).map_err(
                        |_| ArrowTransportError::Internal("indice overlay right oltre usize"),
                    )?]),
                    None => None,
                });
            }
            let batch = RecordBatch::try_new(
                out_schema.clone(),
                vec![
                    std::sync::Arc::new(
                        encoded
                            .iter()
                            .map(|value| Some(value.as_slice()))
                            .collect::<BinaryArray>(),
                    ),
                    std::sync::Arc::new(UInt64Array::from(left_index)),
                    std::sync::Arc::new(UInt64Array::from(right_index)),
                ],
            )
            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
            (out_schema, vec![batch])
        }
        PairOperation::Within => {
            let indexes = within_indexes(
                &left,
                &right,
                schema
                    .max_pairs
                    .ok_or(ArrowTransportError::Internal("max_pairs validato assente"))?,
            )?;
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let matched: std::collections::HashSet<u64> = indexes.into_iter().collect();
            let flags: Vec<Option<bool>> = left
                .iter()
                .enumerate()
                .map(|(index, geometry)| {
                    geometry.as_ref().map(|_| matched.contains(&(index as u64)))
                })
                .collect();
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(WITHIN_COLUMN, DataType::Boolean, true),
                &AppendedColumn::Boolean(flags),
            )?
        }
        PairOperation::CountPointsInPolygons => {
            // Contratto: left = poligoni (output allineato), right = punti.
            let counts = count_points_in_polygons(
                &left,
                &right,
                schema
                    .max_pairs
                    .ok_or(ArrowTransportError::Internal("max_pairs validato assente"))?,
            )?;
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let values: Vec<Option<u64>> = counts
                .iter()
                .enumerate()
                .map(|(index, count)| left[index].as_ref().map(|_| *count))
                .collect();
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(COUNT_COLUMN, DataType::UInt64, true),
                &AppendedColumn::UInt64(values),
            )?
        }
        operation @ (PairOperation::Intersection
        | PairOperation::Union
        | PairOperation::Difference
        | PairOperation::SymmetricDifference) => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let kernel = operation
                .boolean_kernel()
                .ok_or(ArrowTransportError::Internal("booleana pairwise senza kernel"))?;
            // righe indipendenti: parallelo con ordine deterministico.
            // ADR-0001: primo errore in ordine di riga (collect
            // sequenziale dopo quello parallelo indicizzato), mai la
            // selezione non deterministica di rayon.
            let results: Vec<Result<Option<Vec<u8>>, ArrowTransportError>> = left
                .par_iter()
                .zip(right.par_iter())
                .map(|(left_geometry, right_geometry)| {
                    match (left_geometry, right_geometry) {
                        (Some(left_geometry), Some(right_geometry)) => {
                            let result = boolean_operation(left_geometry, right_geometry, kernel)?;
                            // EMPTY -> null, convenzione coerente con `clip`.
                            if result.coords_count() == 0 {
                                Ok(None)
                            } else {
                                Ok(Some(encode_geometry(&result)?))
                            }
                        }
                        _ => Ok(None),
                    }
                })
                .collect();
            let values: Vec<Option<Vec<u8>>> =
                results.into_iter().collect::<Result<_, ArrowTransportError>>()?;
            let output_crs = schema
                .left_crs
                .as_deref()
                .ok_or(ArrowTransportError::Internal("left_crs validato assente"))?;
            replace_geometry_batches(
                &left_schema,
                &left_batches,
                geometry_column,
                output_crs,
                &values,
            )?
        }
        PairOperation::Predicate => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let predicate = schema
                .spatial_predicate
                .ok_or(ArrowTransportError::Internal("spatial_predicate validato assente"))?;
            // ADR-0001: primo errore in ordine di riga (collect
            // sequenziale dopo quello parallelo indicizzato).
            let results: Vec<Result<Option<bool>, ArrowTransportError>> = left
                .par_iter()
                .zip(right.par_iter())
                .map(|(left_geometry, right_geometry)| {
                    Ok(match (left_geometry, right_geometry) {
                        (Some(left_geometry), Some(right_geometry)) => Some(
                            evaluate_predicate(left_geometry, right_geometry, predicate)?,
                        ),
                        _ => None,
                    })
                })
                .collect();
            let flags: Vec<Option<bool>> =
                results.into_iter().collect::<Result<_, ArrowTransportError>>()?;
            let column_name = format!("predicate_{}", spatial_predicate_name(predicate));
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(column_name, DataType::Boolean, true),
                &AppendedColumn::Boolean(flags),
            )?
        }
        PairOperation::HausdorffDistance | PairOperation::FrechetDistance => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let max_pairs = schema
                .max_coordinate_pairs
                .ok_or(ArrowTransportError::Internal("max_coordinate_pairs validato assente"))?;
            // ADR-0001: primo errore in ordine di riga (collect
            // sequenziale dopo quello parallelo indicizzato).
            let results: Vec<Result<Option<f64>, ArrowTransportError>> = left
                .par_iter()
                .zip(right.par_iter())
                .map(|(left_geometry, right_geometry)| {
                    Ok(match (left_geometry, right_geometry) {
                        (Some(left_geometry), Some(right_geometry)) => {
                            if schema.operation == PairOperation::HausdorffDistance {
                                hausdorff_distance(left_geometry, right_geometry, max_pairs)?
                            } else {
                                let left_line =
                                    expect_line_string(left_geometry, schema.operation.name())?;
                                let right_line =
                                    expect_line_string(right_geometry, schema.operation.name())?;
                                frechet_distance(&left_line, &right_line, max_pairs)?
                            }
                        }
                        _ => None,
                    })
                })
                .collect();
            let values: Vec<Option<f64>> =
                results.into_iter().collect::<Result<_, ArrowTransportError>>()?;
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(schema.operation.name(), DataType::Float64, true),
                &AppendedColumn::Float64(values),
            )?
        }
        PairOperation::HaversineDistance
        | PairOperation::GeodesicDistance
        | PairOperation::Bearing => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            // ADR-0001: primo errore in ordine di riga (collect
            // sequenziale dopo quello parallelo indicizzato).
            let results: Vec<Result<Option<f64>, ArrowTransportError>> = left
                .par_iter()
                .zip(right.par_iter())
                .map(|(left_geometry, right_geometry)| {
                    Ok(match (left_geometry, right_geometry) {
                        (Some(left_geometry), Some(right_geometry)) => {
                            let left_point = expect_point(left_geometry, schema.operation.name())?;
                            let right_point =
                                expect_point(right_geometry, schema.operation.name())?;
                            Some(match schema.operation {
                                PairOperation::HaversineDistance => {
                                    haversine_distance_m(left_point, right_point)?
                                }
                                PairOperation::GeodesicDistance => {
                                    geodesic_distance_m(left_point, right_point)?
                                }
                                _ => geodesic_bearing_degrees(left_point, right_point)?,
                            })
                        }
                        _ => None,
                    })
                })
                .collect();
            let values: Vec<Option<f64>> =
                results.into_iter().collect::<Result<_, ArrowTransportError>>()?;
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(schema.operation.name(), DataType::Float64, true),
                &AppendedColumn::Float64(values),
            )?
        }
        #[cfg(feature = "geos-backend")]
        PairOperation::Split => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            let tolerance = schema.tolerance.unwrap_or(0.0);
            let output_crs = schema
                .left_crs
                .as_deref()
                .ok_or(ArrowTransportError::Internal("left_crs validato assente"))?;
            let geometry_index = geometry_column_index(&left_schema, geometry_column)?;
            let mut encoded: Vec<Vec<u8>> = Vec::new();
            let mut take_indices: Vec<u64> = Vec::new();
            let mut parents: Vec<u64> = Vec::new();
            let mut total = 0_u64;
            for (row, (left_geometry, right_geometry)) in left.iter().zip(&right).enumerate() {
                let (Some(left_geometry), Some(right_geometry)) = (left_geometry, right_geometry)
                else {
                    continue;
                };
                let pieces: Vec<Geometry<f64>> = match left_geometry {
                    Geometry::LineString(line) => split_line(
                        line,
                        right_geometry,
                        tolerance,
                        MAX_CELL_COORDINATES,
                        MAX_SPLIT_WORK,
                        limit,
                        MAX_CELL_COORDINATES,
                    )?
                    .into_iter()
                    .map(Geometry::LineString)
                    .collect(),
                    Geometry::Polygon(_) | Geometry::MultiPolygon(_) => split_polygon_by_linework(
                        left_geometry,
                        right_geometry,
                        MAX_CELL_COORDINATES,
                        MAX_NODING_WORK,
                        limit,
                        MAX_CELL_COORDINATES,
                    )?
                    .into_iter()
                    .map(Geometry::Polygon)
                    .collect(),
                    other => {
                        return Err(ArrowTransportError::WrongGeometryType {
                            operation: schema.operation.name(),
                            expected: "LineString o Polygon/MultiPolygon",
                            actual: geometry_type_name(other).to_owned(),
                        })
                    }
                };
                let next = total
                    .checked_add(pieces.len() as u64)
                    .ok_or(ArrowTransportError::StreamTooLarge)?;
                if next > limit {
                    return Err(ArrowTransportError::OutputRowsExceeded {
                        actual: next,
                        limit,
                    });
                }
                total = next;
                for piece in &pieces {
                    encoded.push(encode_geometry(piece)?);
                    take_indices.push(row as u64);
                    parents.push(row as u64);
                }
            }
            let mut output_fields: Vec<Field> = left_schema
                .fields()
                .iter()
                .map(|field| field.as_ref().clone())
                .collect();
            output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
            output_fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
            let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
                output_fields,
                left_schema.metadata().clone(),
            ));
            let take_indices = UInt64Array::from(take_indices);
            let mut columns: Vec<plenora_core::arrow::array::ArrayRef> =
                Vec::with_capacity(left_schema.fields().len() + 1);
            for index in 0..left_schema.fields().len() {
                if index == geometry_index {
                    columns.push(std::sync::Arc::new(BinaryArray::from_iter(
                        encoded.iter().map(|piece| Some(piece.as_slice())),
                    )));
                } else {
                    let column = plenora_core::arrow::select::concat::concat(
                        &left_batches
                            .iter()
                            .map(|batch| batch.column(index).as_ref())
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
                    columns.push(
                        plenora_core::arrow::select::take::take(&column, &take_indices, None)
                            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
                    );
                }
            }
            columns.push(std::sync::Arc::new(UInt64Array::from(parents)));
            let batch = RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
            (out_schema, vec![batch])
        }
        #[cfg(not(feature = "geos-backend"))]
        PairOperation::Split => {
            return Err(ArrowTransportError::BackendUnavailable {
                operation: schema.operation.name(),
                feature: "geos-backend",
            });
        }
    };

    let output_rows: u64 = output_batches
        .iter()
        .map(|batch| batch.num_rows() as u64)
        .sum();
    let output_payload = encode_ipc(&output_schema, &output_batches)?;
    let mut envelope = EnvelopeWriter::new(writer, output_payload.len() as u64)?;
    envelope.write_payload(&output_payload)?;
    let (_, checksum) = envelope.finish()?;
    Ok(PairArrowSummary {
        left_rows,
        right_rows,
        output_rows,
        checksum,
    })
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use plenora_core::arrow::array::Int64Array;
    use geo::{line_string, polygon, Area, CoordsIter, Geometry, Point};
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::sync::Arc;

    const CRS: &str = "EPSG:3857";

    fn square_wkb(size: f64) -> Vec<u8> {
        Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: size, y: 0.0),
            (x: size, y: size), (x: 0.0, y: size),
            (x: 0.0, y: 0.0),
        ])
        .to_wkb(CoordDimensions::xy())
        .expect("fixture WKB")
    }

    fn line_wkb() -> Vec<u8> {
        Geometry::LineString(line_string![
            (x: 0.0, y: 0.0),
            (x: 3.0, y: 0.0),
            (x: 3.0, y: 4.0),
        ])
        .to_wkb(CoordDimensions::xy())
        .expect("fixture WKB")
    }

    fn geometry_field() -> Field {
        let mut metadata = HashMap::new();
        metadata.insert(
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        );
        Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(metadata)
    }

    fn fixture_batch(geometries: &[Option<&[u8]>]) -> (SchemaRef, RecordBatch) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("label", DataType::Utf8, true),
            Field::new("weight", DataType::Float64, true),
            geometry_field(),
        ]));
        let rows = geometries.len();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                // righe fixture: poche per costruzione, entro i64.
                Arc::new(Int64Array::from(
                    (0..i64::try_from(rows).expect("righe fixture entro i64"))
                        .collect::<Vec<i64>>(),
                )),
                Arc::new(StringArray::from(
                    (0..rows)
                        .map(|index| Some(format!("riga-{index}")))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    (0..rows)
                        .map(|index| {
                            // fixture di test: `rows` e' il numero di
                            // geometrie passate alla fixture (poche unita'),
                            // ampiamente entro 2^53: conversione esatta.
                            #[allow(clippy::cast_precision_loss)]
                            let half = index as f64 * 0.5;
                            Some(half)
                        })
                        .collect::<Vec<_>>(),
                )),
                Arc::new(geometries.iter().copied().collect::<BinaryArray>()),
            ],
        )
        .expect("fixture batch");
        (schema, batch)
    }

    fn envelope_bytes(schema: &SchemaRef, batches: &[RecordBatch]) -> Vec<u8> {
        let payload = encode_ipc(schema, batches).expect("encode");
        let mut writer = EnvelopeWriter::new(Vec::new(), payload.len() as u64).expect("writer");
        writer.write_payload(&payload).expect("payload");
        writer.finish().expect("finish").0
    }

    fn arrow_schema(row_count: u64, operation: ArrowOperation) -> TransformArrowSchema {
        TransformArrowSchema {
            schema_version: TransformArrowSchema::VERSION,
            operation,
            row_count,
            crs: Some(CRS.to_owned()),
            geometry_column: None,
            distance: None,
            cap: None,
            tolerance: None,
            simplify_policy: None,
            target_crs: None,
            max_output_rows: None,
            max_points: None,
            x_column: None,
            y_column: None,
            snap_tolerance: None,
            remove_overlaps: None,
            fill_gaps: None,
            coefficients: None,
            x_offset: None,
            y_offset: None,
            x_factor: None,
            y_factor: None,
            degrees: None,
            x_origin: None,
            y_origin: None,
            concavity: None,
            length_threshold: None,
            max_segment_length: None,
            grid_size: None,
            start_ratio: None,
            end_ratio: None,
            ratio: None,
            node_input: None,
            require_complete: None,
        }
    }

    fn run(schema: &TransformArrowSchema, input: &[u8]) -> Result<Vec<u8>, ArrowTransportError> {
        let mut output = Vec::new();
        transform_arrow(input, &mut output, schema)?;
        Ok(output)
    }

    fn decode_output(output: &[u8]) -> (SchemaRef, Vec<RecordBatch>) {
        let payload = EnvelopeReader::new(output)
            .expect("envelope")
            .read_payload()
            .expect("payload");
        decode_ipc(&payload).expect("ipc")
    }

    fn single_cell_output(output: &[u8], column: &str) -> (SchemaRef, RecordBatch, usize) {
        let (schema, batches) = decode_output(output);
        let index = schema.index_of(column).expect("colonna output");
        (schema, batches.into_iter().next().expect("batch"), index)
    }

    #[test]
    fn geometry_roundtrip_preserves_nulls_attributes_and_crs_metadata() {
        let square = square_wkb(4.0);
        let (schema, batch) = fixture_batch(&[Some(&square), None, Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(3, ArrowOperation::Centroid), &input).expect("transform");

        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_batches.len(), 1);
        let out_batch = &out_batches[0];
        assert_eq!(out_batch.num_rows(), 3);

        let geometry_index = out_schema
            .index_of(DEFAULT_GEOMETRY_COLUMN)
            .expect("geometry column");
        let field = out_schema.field(geometry_index);
        assert_eq!(field.data_type(), &DataType::Binary);
        assert_eq!(
            field
                .metadata()
                .get(GEOARROW_EXTENSION_KEY)
                .map(String::as_str),
            Some(GEOARROW_WKB_EXTENSION)
        );
        let geo: serde_json::Value = serde_json::from_str(
            field
                .metadata()
                .get(GEO_METADATA_KEY)
                .expect("geo metadata"),
        )
        .expect("geo JSON");
        assert_eq!(
            geo.get("crs").and_then(serde_json::Value::as_str),
            Some(CRS)
        );

        let cells = out_batch
            .column(geometry_index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .expect("Binary column");
        let expected = transform_wkb(Operation::Centroid, &square).expect("kernel");
        assert_eq!(cells.value(0), expected.as_slice());
        assert!(cells.is_null(1));
        assert_eq!(cells.value(2), expected.as_slice());
        let centroid = geometry_from_wkb(cells.value(0)).expect("decode centroid");
        assert_eq!(centroid, Geometry::Point(Point::new(2.0, 2.0)));

        let ids = out_batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64");
        assert_eq!(ids.values(), &[0, 1, 2]);
        let labels = out_batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Utf8");
        assert_eq!(labels.value(2), "riga-2");
        let weights = out_batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("Float64");
        assert_eq!(weights.value(1), 0.5);
    }

    #[test]
    fn backend_free_operations_roundtrip_with_null_preservation() {
        let square = square_wkb(2.0);
        let cases: [(ArrowOperation, TransformArrowSchema); 12] = [
            (
                ArrowOperation::Centroid,
                arrow_schema(2, ArrowOperation::Centroid),
            ),
            (
                ArrowOperation::ConvexHull,
                arrow_schema(2, ArrowOperation::ConvexHull),
            ),
            (
                ArrowOperation::Envelope,
                arrow_schema(2, ArrowOperation::Envelope),
            ),
            (
                ArrowOperation::Buffer,
                TransformArrowSchema {
                    distance: Some(0.5),
                    ..arrow_schema(2, ArrowOperation::Buffer)
                },
            ),
            (
                ArrowOperation::Simplify,
                TransformArrowSchema {
                    tolerance: Some(0.1),
                    ..arrow_schema(2, ArrowOperation::Simplify)
                },
            ),
            (
                ArrowOperation::Boundary,
                arrow_schema(2, ArrowOperation::Boundary),
            ),
            (
                ArrowOperation::PointOnSurface,
                arrow_schema(2, ArrowOperation::PointOnSurface),
            ),
            (ArrowOperation::Area, arrow_schema(2, ArrowOperation::Area)),
            (
                ArrowOperation::Length,
                arrow_schema(2, ArrowOperation::Length),
            ),
            (
                ArrowOperation::Perimeter,
                arrow_schema(2, ArrowOperation::Perimeter),
            ),
            (
                ArrowOperation::VertexCount,
                arrow_schema(2, ArrowOperation::VertexCount),
            ),
            (
                ArrowOperation::ToWkt,
                arrow_schema(2, ArrowOperation::ToWkt),
            ),
        ];
        for (operation, schema) in cases {
            let (fixture_schema, batch) = fixture_batch(&[Some(&square), None]);
            let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
            let output = run(&schema, &input)
                .unwrap_or_else(|error| panic!("{} fallita: {error}", operation.name()));
            let (out_schema, out_batches) = decode_output(&output);
            let batch = &out_batches[0];
            assert_eq!(batch.num_rows(), 2, "{}", operation.name());

            if operation.produces_geometry() {
                let index = out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
                let cells = batch
                    .column(index)
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .expect("Binary");
                assert!(!cells.is_null(0), "{}", operation.name());
                assert!(cells.is_null(1), "{}", operation.name());
                geometry_from_wkb(cells.value(0)).unwrap_or_else(|error| {
                    panic!("{} output non valido: {error}", operation.name())
                });
            } else {
                let column_name = match operation {
                    ArrowOperation::ToWkt => WKT_COLUMN,
                    _ => operation.name(),
                };
                let index = out_schema.index_of(column_name).unwrap();
                let column = batch.column(index);
                assert!(!column.is_null(0), "{}", operation.name());
                assert!(column.is_null(1), "{}", operation.name());
            }
        }
    }

    #[test]
    fn buffer_honours_distance_cap_and_validates_parameters() {
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        for cap in [BufferCap::Round, BufferCap::Flat, BufferCap::Square] {
            let schema = TransformArrowSchema {
                distance: Some(1.0),
                cap: Some(cap),
                ..arrow_schema(1, ArrowOperation::Buffer)
            };
            let output = run(&schema, &input).expect("buffer");
            let (_, batch, index) = single_cell_output(&output, DEFAULT_GEOMETRY_COLUMN);
            let cells = batch
                .column(index)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let buffered = geometry_from_wkb(cells.value(0)).expect("decode");
            let area = buffered.unsigned_area();
            // buffer(1) di un quadrato 2x2: fra quadrato espanso (16) e cerchio.
            assert!(area > 8.0 && area <= 16.0, "cap {cap:?}: area {area}");
        }

        let missing = arrow_schema(1, ArrowOperation::Buffer);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "distance",
                ..
            })
        ));

        let nan = TransformArrowSchema {
            distance: Some(f64::NAN),
            ..arrow_schema(1, ArrowOperation::Buffer)
        };
        assert!(matches!(
            run(&nan, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "distance",
                ..
            })
        ));

        let unexpected = TransformArrowSchema {
            distance: Some(1.0),
            tolerance: Some(0.1),
            ..arrow_schema(1, ArrowOperation::Buffer)
        };
        assert!(matches!(
            run(&unexpected, &input),
            Err(ArrowTransportError::UnexpectedParameter {
                name: "tolerance",
                ..
            })
        ));
    }

    #[test]
    fn simplify_honours_tolerance_policy_and_validates_parameters() {
        let mut jittered = vec![1_u8];
        jittered.extend_from_slice(&2_u32.to_le_bytes());
        jittered.extend_from_slice(&6_u32.to_le_bytes());
        for (x, y) in [
            (0.0_f64, 0.0_f64),
            (1.0, 0.01),
            (2.0, -0.01),
            (3.0, 0.01),
            (4.0, -0.01),
            (5.0, 0.0),
        ] {
            jittered.extend_from_slice(&x.to_le_bytes());
            jittered.extend_from_slice(&y.to_le_bytes());
        }
        let (fixture_schema, batch) = fixture_batch(&[Some(&jittered)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        for policy in [
            SimplifyPolicyParam::DouglasPeucker,
            SimplifyPolicyParam::PreserveTopology,
        ] {
            let schema = TransformArrowSchema {
                tolerance: Some(0.5),
                simplify_policy: Some(policy),
                ..arrow_schema(1, ArrowOperation::Simplify)
            };
            let output = run(&schema, &input).expect("simplify");
            let (_, batch, index) = single_cell_output(&output, DEFAULT_GEOMETRY_COLUMN);
            let cells = batch
                .column(index)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let simplified = geometry_from_wkb(cells.value(0)).expect("decode");
            assert_eq!(simplified.coords_count(), 2, "policy {policy:?}");
        }

        let missing = arrow_schema(1, ArrowOperation::Simplify);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "tolerance",
                ..
            })
        ));

        let negative = TransformArrowSchema {
            tolerance: Some(-1.0),
            ..arrow_schema(1, ArrowOperation::Simplify)
        };
        assert!(matches!(
            run(&negative, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "tolerance",
                ..
            })
        ));
    }

    #[test]
    fn boundary_and_point_on_surface_produce_expected_geometry_types() {
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        let output = run(&arrow_schema(1, ArrowOperation::Boundary), &input).expect("boundary");
        let (_, batch, index) = single_cell_output(&output, DEFAULT_GEOMETRY_COLUMN);
        let cells = batch
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(matches!(
            geometry_from_wkb(cells.value(0)).unwrap(),
            Geometry::MultiLineString(_)
        ));

        let output = run(&arrow_schema(1, ArrowOperation::PointOnSurface), &input)
            .expect("point_on_surface");
        let (_, batch, index) = single_cell_output(&output, DEFAULT_GEOMETRY_COLUMN);
        let cells = batch
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let point = geometry_from_wkb(cells.value(0)).unwrap();
        let Geometry::Point(point) = point else {
            panic!("point_on_surface deve produrre un Point: {point:?}")
        };
        assert!(point.x() > 0.0 && point.x() < 2.0);
        assert!(point.y() > 0.0 && point.y() < 2.0);
    }

    #[test]
    fn length_perimeter_vertex_count_bounds_and_wkt_are_exact() {
        let line = line_wkb();
        let (fixture_schema, batch) = fixture_batch(&[Some(&line), None]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        for (operation, expected) in [
            (ArrowOperation::Length, 7.0),
            (ArrowOperation::Perimeter, 7.0),
        ] {
            let output =
                run(&arrow_schema(2, operation), &input).unwrap_or_else(|_| panic!("{}", operation.name()));
            let (_, batch, index) = single_cell_output(&output, operation.name());
            let values = batch
                .column(index)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            assert_eq!(values.value(0), expected, "{}", operation.name());
            assert!(values.is_null(1));
        }

        let output =
            run(&arrow_schema(2, ArrowOperation::VertexCount), &input).expect("vertex_count");
        let (out_schema, batch, index) = single_cell_output(&output, "vertex_count");
        assert_eq!(out_schema.field(index).data_type(), &DataType::UInt64);
        let values = batch
            .column(index)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(values.value(0), 3);
        assert!(values.is_null(1));

        let output = run(&arrow_schema(2, ArrowOperation::Bounds), &input).expect("bounds");
        let (out_schema, batches) = decode_output(&output);
        let expected_bounds = [
            ("geometry_minx", 0.0),
            ("geometry_miny", 0.0),
            ("geometry_maxx", 3.0),
            ("geometry_maxy", 4.0),
        ];
        for (name, expected) in expected_bounds {
            let index = out_schema.index_of(name).expect("colonna bounds");
            let values = batches[0]
                .column(index)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            assert_eq!(values.value(0), expected, "{name}");
            assert!(values.is_null(1));
        }

        let output = run(&arrow_schema(2, ArrowOperation::ToWkt), &input).expect("to_wkt");
        let (out_schema, batch, index) = single_cell_output(&output, WKT_COLUMN);
        assert_eq!(out_schema.field(index).data_type(), &DataType::Utf8);
        let values = batch
            .column(index)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        // Adattamento Fase 1: il crate `wkt` non e' una dipendenza di
        // plenora-engine (nel sorgente il WKT veniva ri-parsato con
        // `wkt::TryFromWkt` e confrontato con la geometria attesa); qui il
        // confronto usa il kernel `to_wkt` come riferimento canonico.
        let expected_wkt = to_wkt(&geometry_from_wkb(&line).unwrap()).expect("wkt atteso");
        assert_eq!(values.value(0), expected_wkt);
        assert!(values.is_null(1));
    }

    #[cfg(feature = "geos-backend")]
    #[test]
    fn make_valid_repairs_bowtie_and_preserves_valid_geometries() {
        let mut bowtie = vec![1_u8];
        bowtie.extend_from_slice(&3_u32.to_le_bytes());
        bowtie.extend_from_slice(&1_u32.to_le_bytes());
        bowtie.extend_from_slice(&5_u32.to_le_bytes());
        for (x, y) in [
            (0.0_f64, 0.0_f64),
            (2.0, 2.0),
            (0.0, 2.0),
            (2.0, 0.0),
            (0.0, 0.0),
        ] {
            bowtie.extend_from_slice(&x.to_le_bytes());
            bowtie.extend_from_slice(&y.to_le_bytes());
        }
        assert!(geometry_from_wkb(&bowtie).is_err());

        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&bowtie), None, Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(3, ArrowOperation::MakeValid), &input).expect("make_valid");
        let (out_schema, out_batches) = decode_output(&output);
        let index = out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
        let cells = out_batches[0]
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let repaired = geometry_from_wkb(cells.value(0)).expect("riparata");
        assert!((repaired.unsigned_area() - 2.0).abs() < 1e-12);
        assert!(cells.is_null(1));
        assert_eq!(cells.value(2), square.as_slice());
    }

    #[cfg(not(feature = "geos-backend"))]
    #[test]
    fn make_valid_without_geos_fails_closed() {
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::MakeValid), &input),
            Err(ArrowTransportError::BackendUnavailable {
                feature: "geos-backend",
                ..
            })
        ));
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn reproject_transforms_coordinates_and_stamps_target_crs() {
        let mut point = vec![1_u8, 1, 0, 0, 0];
        point.extend_from_slice(&12.0_f64.to_le_bytes());
        point.extend_from_slice(&41.0_f64.to_le_bytes());
        let (fixture_schema, batch) = fixture_batch(&[Some(&point), None]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        let schema = TransformArrowSchema {
            crs: Some("EPSG:4326".to_owned()),
            target_crs: Some(CRS.to_owned()),
            ..arrow_schema(2, ArrowOperation::Reproject)
        };
        let output = run(&schema, &input).expect("reproject");
        let (out_schema, out_batches) = decode_output(&output);
        let index = out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
        let geo: serde_json::Value = serde_json::from_str(
            out_schema
                .field(index)
                .metadata()
                .get(GEO_METADATA_KEY)
                .expect("geo metadata"),
        )
        .unwrap();
        assert_eq!(
            geo.get("crs").and_then(serde_json::Value::as_str),
            Some(CRS)
        );
        let cells = out_batches[0]
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let reprojected = geometry_from_wkb(cells.value(0)).unwrap();
        let Geometry::Point(point) = reprojected else {
            panic!("atteso Point: {reprojected:?}")
        };
        // EPSG:3857 di (12E, 41N) calcolato con PROJ.
        assert!((point.x() - 1_335_833.8895).abs() < 0.01);
        assert!((point.y() - 5_012_341.6638).abs() < 0.01);
        assert!(cells.is_null(1));

        let missing_target = TransformArrowSchema {
            crs: Some("EPSG:4326".to_owned()),
            ..arrow_schema(2, ArrowOperation::Reproject)
        };
        assert!(matches!(
            run(&missing_target, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "target_crs",
                ..
            })
        ));
    }

    #[cfg(not(feature = "proj-backend"))]
    #[test]
    fn reproject_without_proj_fails_closed() {
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let schema = TransformArrowSchema {
            target_crs: Some("EPSG:32632".to_owned()),
            ..arrow_schema(1, ArrowOperation::Reproject)
        };
        assert!(matches!(
            run(&schema, &input),
            Err(ArrowTransportError::BackendUnavailable {
                feature: "proj-backend",
                ..
            })
        ));
    }

    #[test]
    fn multiple_batches_are_preserved_in_output() {
        let square = square_wkb(1.0);
        let (schema, first) = fixture_batch(&[Some(&square)]);
        let (_, second) = fixture_batch(&[None]);
        let input = envelope_bytes(&schema, &[first, second]);
        let output = run(&arrow_schema(2, ArrowOperation::Envelope), &input).expect("transform");
        let (_, batches) = decode_output(&output);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].num_rows(), 1);
        assert_eq!(batches[1].num_rows(), 1);
    }

    #[test]
    fn single_byte_corruption_is_detected_by_checksum() {
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let mut input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let flip = input.len() / 2;
        input[flip] ^= 0x01;
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &input),
            Err(ArrowTransportError::ChecksumMismatch)
        ));
    }

    #[test]
    fn truncation_trailing_bytes_and_bad_magic_fail_closed() {
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        for cut in [1_usize, 8, 20, 40] {
            let truncated = &input[..input.len() - cut];
            assert!(
                run(&arrow_schema(1, ArrowOperation::Centroid), truncated).is_err(),
                "cut={cut}"
            );
        }

        let mut extra = input.clone();
        extra.push(0);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &extra),
            Err(ArrowTransportError::TrailingBytes)
        ));

        let mut bad_magic = input;
        bad_magic[0] = b'X';
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &bad_magic),
            Err(ArrowTransportError::InvalidMagic)
        ));

        let mut bad_trailer =
            envelope_bytes(&schema, std::slice::from_ref(&fixture_batch(&[None]).1));
        let trailer_start = bad_trailer.len() - 40;
        bad_trailer[trailer_start] ^= 0x01;
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &bad_trailer),
            Err(ArrowTransportError::InvalidTrailer)
        ));
    }

    #[test]
    fn row_count_schema_version_and_resource_limits_fail_closed() {
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));

        assert!(matches!(
            run(&arrow_schema(2, ArrowOperation::Centroid), &input),
            Err(ArrowTransportError::RowCountMismatch {
                schema: 2,
                stream: 1
            })
        ));

        let mut wrong_version = arrow_schema(1, ArrowOperation::Centroid);
        wrong_version.schema_version = 2;
        assert!(matches!(
            run(&wrong_version, &input),
            Err(ArrowTransportError::UnsupportedSchemaVersion(2))
        ));

        let too_many = arrow_schema(MAX_ROWS + 1, ArrowOperation::Centroid);
        assert!(matches!(
            run(&too_many, &input),
            Err(ArrowTransportError::TooManyRows(_))
        ));

        let missing_crs = TransformArrowSchema {
            crs: None,
            ..arrow_schema(1, ArrowOperation::Centroid)
        };
        assert!(matches!(
            run(&missing_crs, &input),
            Err(ArrowTransportError::CrsRequired)
        ));
    }

    #[test]
    fn column_and_batch_limits_fail_closed() {
        let wide_fields: Vec<Field> = (0..=MAX_COLUMNS)
            .map(|index| Field::new(format!("col{index}"), DataType::Int64, true))
            .collect();
        let wide_schema = Arc::new(Schema::new(wide_fields));
        let payload = encode_ipc(&wide_schema, &[]).expect("encode wide");
        assert!(matches!(
            decode_ipc(&payload),
            Err(ArrowTransportError::TooManyColumns(_))
        ));

        let (schema, batch) = fixture_batch(&[None]);
        let batches = vec![batch; MAX_BATCHES + 1];
        assert!(matches!(
            encode_ipc(&schema, &batches),
            Err(ArrowTransportError::TooManyBatches(_))
        ));
    }

    #[test]
    fn oversized_wkb_cell_fails_before_validation() {
        // MAX_CELL_BYTES e' una costante da 64 MiB: entra in usize su
        // ogni target supportato; la conversione e' totale per contratto.
        let oversized =
            vec![0_u8; usize::try_from(MAX_CELL_BYTES).expect("limite celle entro usize") + 1];
        let (schema, batch) = fixture_batch(&[Some(&oversized)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &input),
            Err(ArrowTransportError::CellTooLarge(_))
        ));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Area), &input),
            Err(ArrowTransportError::CellTooLarge(_))
        ));
    }

    #[test]
    fn geometry_column_contract_is_fail_closed() {
        let (schema, batch) = fixture_batch(&[None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));

        let renamed = TransformArrowSchema {
            geometry_column: Some("assente".to_owned()),
            ..arrow_schema(1, ArrowOperation::Centroid)
        };
        assert!(matches!(
            run(&renamed, &input),
            Err(ArrowTransportError::MissingGeometryColumn(_))
        ));

        let wrong_type_schema = Arc::new(Schema::new(vec![Field::new(
            DEFAULT_GEOMETRY_COLUMN,
            DataType::Int64,
            true,
        )]));
        let wrong_type_batch = RecordBatch::try_new(
            wrong_type_schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1_i64]))],
        )
        .unwrap();
        let wrong_type = envelope_bytes(&wrong_type_schema, &[wrong_type_batch]);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &wrong_type),
            Err(ArrowTransportError::GeometryColumnNotBinary { .. })
        ));

        let no_metadata_schema = Arc::new(Schema::new(vec![Field::new(
            DEFAULT_GEOMETRY_COLUMN,
            DataType::Binary,
            true,
        )]));
        let no_metadata_batch = RecordBatch::try_new(
            no_metadata_schema.clone(),
            vec![Arc::new(BinaryArray::from_iter([None::<&[u8]>]))],
        )
        .unwrap();
        let no_metadata = envelope_bytes(&no_metadata_schema, &[no_metadata_batch]);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &no_metadata),
            Err(ArrowTransportError::MissingGeoArrowMetadata(_))
        ));

        let invalid_wkb = vec![0xde_u8, 0xad];
        let (schema, batch) = fixture_batch(&[Some(&invalid_wkb)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Area), &input),
            Err(ArrowTransportError::Geometry(_))
        ));
    }

    #[test]
    fn ipc_decode_rejects_oversized_metadata_and_truncation_without_oom() {
        // Regressione fuzz (OOM): 4 byte che dichiarano ~709 MiB di metadati
        // in formato legacy; prima della pre-validazione arrow-rs allocava
        // quanto dichiarato.
        let oom_input = [0x5b, 0x74, 0x32, 0x2a];
        assert!(matches!(
            decode_ipc(&oom_input),
            Err(ArrowTransportError::IpcMetadataTooLarge(707_949_659))
        ));
        // continuazione valida ma metadati troncati.
        let truncated = [0xff, 0xff, 0xff, 0xff, 0x10, 0x00];
        assert!(matches!(
            decode_ipc(&truncated),
            Err(ArrowTransportError::IpcTruncated)
        ));
        assert!(matches!(
            decode_ipc(&[]),
            Err(ArrowTransportError::IpcTruncated)
        ));
        // metadati oltre il tetto assoluto anche con continuazione moderna.
        // MAX_IPC_METADATA_BYTES e' una costante da 16 MiB: entra in u32;
        // la conversione e' totale per contratto.
        let declared = u32::try_from(MAX_IPC_METADATA_BYTES).expect("tetto metadati entro u32") + 8;
        let mut oversized = vec![0xff, 0xff, 0xff, 0xff];
        oversized.extend_from_slice(&declared.to_le_bytes());
        oversized.extend_from_slice(&[0; 16]);
        assert!(matches!(
            decode_ipc(&oversized),
            Err(ArrowTransportError::IpcMetadataTooLarge(_))
        ));
        // un batch valido continua a decodificare (framing reale coperto dai
        // roundtrip degli altri test).
        let square = square_wkb(1.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let payload = encode_ipc(&schema, std::slice::from_ref(&batch)).expect("encode");
        assert!(decode_ipc(&payload).is_ok());
    }

    #[test]
    fn unknown_schema_field_is_rejected_and_operation_params_parse() {
        let body = br#"{"schema_version":3,"operation":"centroid","row_count":1,"crs":"EPSG:3857","sconosciuto":true}"#;
        assert!(serde_json::from_slice::<TransformArrowSchema>(body).is_err());

        let minimal = br#"{"schema_version":3,"operation":"area","row_count":1,"crs":"EPSG:3857"}"#;
        let schema: TransformArrowSchema = serde_json::from_slice(minimal).expect("schema");
        assert_eq!(schema.geometry_column(), DEFAULT_GEOMETRY_COLUMN);
        assert_eq!(schema.operation, ArrowOperation::Area);

        let buffer = br#"{"schema_version":3,"operation":"buffer","row_count":1,"crs":"EPSG:3857","distance":2.5,"cap":"flat"}"#;
        let schema: TransformArrowSchema = serde_json::from_slice(buffer).expect("buffer schema");
        assert_eq!(schema.distance, Some(2.5));
        assert_eq!(schema.cap, Some(BufferCap::Flat));
        schema.validate_parameters().expect("parametri validi");

        let simplify = br#"{"schema_version":3,"operation":"simplify","row_count":1,"crs":"EPSG:3857","tolerance":0.1,"simplify_policy":"preserve_topology"}"#;
        let schema: TransformArrowSchema = serde_json::from_slice(simplify).expect("simplify");
        assert_eq!(
            schema.simplify_policy,
            Some(SimplifyPolicyParam::PreserveTopology)
        );

        let reproject = br#"{"schema_version":3,"operation":"reproject","row_count":1,"crs":"EPSG:4326","target_crs":"EPSG:3857"}"#;
        let schema: TransformArrowSchema = serde_json::from_slice(reproject).expect("reproject");
        schema.validate_parameters().expect("parametri validi");
    }

    #[test]
    fn geo_metadata_embeds_projjson_objects_and_plain_codes() {
        let code = geo_metadata_json(CRS).expect("code");
        let parsed: serde_json::Value = serde_json::from_str(&code).unwrap();
        assert_eq!(parsed["crs"], serde_json::Value::String(CRS.to_owned()));

        let projjson = r#"{"type":"ProjectedCRS","name":"demo"}"#;
        let embedded = geo_metadata_json(projjson).expect("projjson");
        let parsed: serde_json::Value = serde_json::from_str(&embedded).unwrap();
        assert_eq!(parsed["crs"]["type"], "ProjectedCRS");

        assert!(matches!(
            geo_metadata_json("  "),
            Err(ArrowTransportError::CrsRequired)
        ));
        let oversized = "X".repeat(MAX_CRS_DEFINITION_BYTES + 1);
        assert!(matches!(
            geo_metadata_json(&oversized),
            Err(ArrowTransportError::CrsTooLarge)
        ));
    }

    #[test]
    fn geo_metadata_json_is_byte_identical_to_arrow_adapter() {
        // Unificazione B1.1: il trasporto delega l'assemblaggio JSON ad
        // `arrow_adapter`; l'output deve essere identico byte-per-byte.
        for crs in [CRS, r#"{"type":"ProjectedCRS","name":"demo"}"#] {
            assert_eq!(
                geo_metadata_json(crs).expect("transport"),
                plenora_kernels_geo::arrow_adapter::geo_metadata_json(crs).expect("adapter")
            );
        }
        // Il campo di output dichiara anche la dimensionalita' (B1.1).
        let field = geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("field");
        let geo: serde_json::Value = serde_json::from_str(
            field.metadata().get(GEO_METADATA_KEY).expect("geo metadata"),
        )
        .expect("geo JSON");
        assert_eq!(
            geo.get("dimensions").and_then(serde_json::Value::as_str),
            Some("xy")
        );
        assert_eq!(
            field.metadata().get(GEO_METADATA_KEY).map(String::as_str),
            plenora_kernels_geo::arrow_adapter::geometry_output_field(
                DEFAULT_GEOMETRY_COLUMN,
                CRS
            )
            .expect("adapter field")
            .metadata()
            .get(GEO_METADATA_KEY)
            .map(String::as_str)
        );
    }

    #[test]
    fn writer_rejects_payload_beyond_declared_length() {
        let mut writer = EnvelopeWriter::new(Vec::new(), 4).expect("writer");
        writer.write_payload(b"ab").expect("chunk");
        assert!(matches!(
            writer.write_payload(b"cde"),
            Err(ArrowTransportError::StreamTooLarge)
        ));
        assert!(matches!(
            writer.finish(),
            Err(ArrowTransportError::PayloadLengthMismatch {
                declared: 4,
                written: 2
            })
        ));
    }

    #[test]
    fn ipc_roundtrip_through_cursor_io() {
        let square = square_wkb(3.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let mut output = Vec::new();
        transform_arrow(
            Cursor::new(input),
            Cursor::new(&mut output),
            &arrow_schema(1, ArrowOperation::ConvexHull),
        )
        .expect("transform");
        let (_, batches) = decode_output(&output);
        let geometry = geometry_from_wkb(
            batches[0]
                .column(3)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap()
                .value(0),
        )
        .unwrap();
        assert!(geometry.unsigned_area() > 0.0);
    }

    fn multipoint_wkb() -> Vec<u8> {
        Geometry::MultiPoint(geo::MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(2.0, 2.0),
        ]))
        .to_wkb(CoordDimensions::xy())
        .expect("fixture WKB")
    }

    #[test]
    fn explode_expands_rows_with_lineage_and_replicated_attributes() {
        let multi = multipoint_wkb();
        let square = square_wkb(1.0);
        let (schema, batch) = fixture_batch(&[Some(&multi), None, Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let explode_schema = TransformArrowSchema {
            max_output_rows: Some(16),
            ..arrow_schema(3, ArrowOperation::Explode)
        };
        let output = run(&explode_schema, &input).expect("explode");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_batches.len(), 1);
        let batch = &out_batches[0];
        // 3 punti dal MultiPoint + 1 riga dal Polygon semplice; null senza figli.
        assert_eq!(batch.num_rows(), 4);

        let parents = batch
            .column(out_schema.index_of(PARENT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("UInt64");
        assert_eq!(parents.values(), &[0, 0, 0, 2]);
        assert!(!parents.is_nullable());

        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64");
        assert_eq!(ids.values(), &[0, 0, 0, 2]);
        let labels = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Utf8");
        assert_eq!(labels.value(3), "riga-2");

        let cells = batch
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .expect("Binary");
        for row in 0..3 {
            let Geometry::Point(point) = geometry_from_wkb(cells.value(row)).unwrap() else {
                panic!("componente MultiPoint deve essere Point")
            };
            // `row` e' l'indice del loop 0..3: esatto in f64.
            #[allow(clippy::cast_precision_loss)]
            let coordinate = row as f64;
            assert_eq!(point, Point::new(coordinate, coordinate));
        }
        assert!(matches!(
            geometry_from_wkb(cells.value(3)).unwrap(),
            Geometry::Polygon(_)
        ));
    }

    #[test]
    fn explode_enforces_max_output_rows_incrementally() {
        let multi = multipoint_wkb();
        let (schema, batch) = fixture_batch(&[Some(&multi)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));

        let missing = arrow_schema(1, ArrowOperation::Explode);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "max_output_rows",
                ..
            })
        ));

        let too_small = TransformArrowSchema {
            max_output_rows: Some(2),
            ..arrow_schema(1, ArrowOperation::Explode)
        };
        assert!(matches!(
            run(&too_small, &input),
            Err(ArrowTransportError::OutputRowsExceeded {
                actual: 3,
                limit: 2
            })
        ));
    }

    #[test]
    fn dissolve_merges_polygons_and_rejects_other_types() {
        let mut shifted = vec![1_u8];
        shifted.extend_from_slice(&3_u32.to_le_bytes());
        shifted.extend_from_slice(&1_u32.to_le_bytes());
        shifted.extend_from_slice(&5_u32.to_le_bytes());
        for (x, y) in [
            (1.0_f64, 0.0_f64),
            (3.0, 0.0),
            (3.0, 2.0),
            (1.0, 2.0),
            (1.0, 0.0),
        ] {
            shifted.extend_from_slice(&x.to_le_bytes());
            shifted.extend_from_slice(&y.to_le_bytes());
        }
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&square), None, Some(&shifted)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(3, ArrowOperation::Dissolve), &input).expect("dissolve");
        let (out_schema, out_batches) = decode_output(&output);
        // una sola riga, solo colonna geometria, attributi non propagati.
        assert_eq!(out_schema.fields().len(), 1);
        assert_eq!(out_batches[0].num_rows(), 1);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let dissolved = geometry_from_wkb(cells.value(0)).expect("decode");
        assert!((dissolved.unsigned_area() - 6.0).abs() < 1e-12);

        // solo null: una riga con geometria null.
        let (schema, batch) = fixture_batch(&[None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(1, ArrowOperation::Dissolve), &input).expect("dissolve");
        let (_, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(cells.is_null(0));

        // input non poligonale: rifiutato dal kernel.
        let line = line_wkb();
        let (schema, batch) = fixture_batch(&[Some(&line)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Dissolve), &input),
            Err(ArrowTransportError::Topology(_))
        ));
    }

    #[test]
    fn line_and_polygon_builder_use_input_order_and_skip_nulls() {
        let points: Vec<Vec<u8>> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .into_iter()
            .map(|(x, y)| {
                Geometry::Point(Point::new(x, y))
                    .to_wkb(CoordDimensions::xy())
                    .unwrap()
            })
            .collect();
        let refs: Vec<Option<&[u8]>> = vec![
            Some(&points[0]),
            None,
            Some(&points[1]),
            Some(&points[2]),
            Some(&points[3]),
        ];
        let (schema, batch) = fixture_batch(&refs);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));

        let output = run(&arrow_schema(5, ArrowOperation::LineBuilder), &input).expect("line");
        let (_, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let Geometry::LineString(line) = geometry_from_wkb(cells.value(0)).unwrap() else {
            panic!("line_builder deve produrre LineString")
        };
        assert_eq!(line.coords_count(), 4);

        let output =
            run(&arrow_schema(5, ArrowOperation::PolygonBuilder), &input).expect("polygon");
        let (_, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let polygon = geometry_from_wkb(cells.value(0)).unwrap();
        assert!((polygon.unsigned_area() - 1.0).abs() < 1e-12);

        // punti insufficienti: riga null, non errore.
        let (schema, batch) = fixture_batch(&[Some(&points[0]), None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(2, ArrowOperation::LineBuilder), &input).expect("line");
        let (_, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(cells.is_null(0));

        // input non puntuale: fail-closed.
        let square = square_wkb(1.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::LineBuilder), &input),
            Err(ArrowTransportError::Construction(_))
        ));

        // ordine auto-intersecante: il kernel rifiuta il poligono invalido.
        let bowtie_points: Vec<Vec<u8>> = [(0.0, 0.0), (1.0, 1.0), (0.0, 1.0), (1.0, 0.0)]
            .into_iter()
            .map(|(x, y)| {
                Geometry::Point(Point::new(x, y))
                    .to_wkb(CoordDimensions::xy())
                    .unwrap()
            })
            .collect();
        let refs: Vec<Option<&[u8]>> = bowtie_points.iter().map(|p| Some(p.as_slice())).collect();
        let (schema, batch) = fixture_batch(&refs);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(4, ArrowOperation::PolygonBuilder), &input),
            Err(ArrowTransportError::Construction(_))
        ));
    }

    #[test]
    fn voronoi_preserves_positions_and_enforces_point_cap() {
        use geo::Intersects;

        let points: Vec<Vec<u8>> = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]
            .into_iter()
            .map(|(x, y)| {
                Geometry::Point(Point::new(x, y))
                    .to_wkb(CoordDimensions::xy())
                    .unwrap()
            })
            .collect();
        let refs: Vec<Option<&[u8]>> = vec![
            Some(&points[0]),
            None,
            Some(&points[1]),
            Some(&points[2]),
            Some(&points[3]),
        ];
        let (schema, batch) = fixture_batch(&refs);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(5, ArrowOperation::Voronoi), &input).expect("voronoi");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_batches[0].num_rows(), 5);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(cells.is_null(1));
        for (row, point) in [
            (0, &points[0]),
            (2, &points[1]),
            (3, &points[2]),
            (4, &points[3]),
        ] {
            let cell = geometry_from_wkb(cells.value(row)).expect("cella");
            let expected_point = geometry_from_wkb(point).unwrap();
            assert!(cell.intersects(&expected_point), "cella riga {row}");
        }
        // attributi preservati sulle stesse righe.
        let ids = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2, 3, 4]);

        // cap punti dal kernel.
        let capped = TransformArrowSchema {
            max_points: Some(3),
            ..arrow_schema(5, ArrowOperation::Voronoi)
        };
        assert!(matches!(
            run(&capped, &input),
            Err(ArrowTransportError::Advanced(_))
        ));

        // max_points non valido nello schema.
        let invalid = TransformArrowSchema {
            max_points: Some(1),
            ..arrow_schema(5, ArrowOperation::Voronoi)
        };
        assert!(matches!(
            run(&invalid, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "max_points",
                ..
            })
        ));

        // input non puntuale: fail-closed.
        let square = square_wkb(1.0);
        let (schema, batch) = fixture_batch(&[Some(&square), Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(2, ArrowOperation::Voronoi), &input),
            Err(ArrowTransportError::Advanced(_))
        ));
    }

    fn coords_batch(xs: Vec<Option<f64>>, ys: Vec<Option<f64>>) -> (SchemaRef, RecordBatch) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("x", DataType::Float64, true),
            Field::new("y", DataType::Float64, true),
        ]));
        // righe fixture: poche per costruzione, entro i64.
        let rows = i64::try_from(xs.len()).expect("righe fixture entro i64");
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from((0..rows).collect::<Vec<i64>>())),
                Arc::new(Float64Array::from(xs)),
                Arc::new(Float64Array::from(ys)),
            ],
        )
        .expect("coords batch");
        (schema, batch)
    }

    #[test]
    fn from_coords_builds_points_without_geometry_input() {
        let (schema, batch) = coords_batch(
            vec![Some(12.0), None, Some(7.5)],
            vec![Some(41.0), Some(1.0), None],
        );
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output =
            run(&arrow_schema(3, ArrowOperation::FromCoords), &input).expect("from_coords");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_schema.fields().len(), 4);
        let index = out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
        let field = out_schema.field(index);
        assert_eq!(
            field
                .metadata()
                .get(GEOARROW_EXTENSION_KEY)
                .map(String::as_str),
            Some(GEOARROW_WKB_EXTENSION)
        );
        let cells = out_batches[0]
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert_eq!(
            geometry_from_wkb(cells.value(0)).unwrap(),
            Geometry::Point(Point::new(12.0, 41.0))
        );
        assert!(cells.is_null(1));
        assert!(cells.is_null(2));
        let ids = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);

        // coordinate non finite: rifiutate (fail-closed).
        let (schema, batch) = coords_batch(vec![Some(f64::NAN)], vec![Some(0.0)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input),
            Err(ArrowTransportError::Construction(_))
        ));

        // colonna assente.
        let (schema, batch) = coords_batch(vec![Some(1.0)], vec![Some(2.0)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let renamed = TransformArrowSchema {
            x_column: Some("lon".to_owned()),
            ..arrow_schema(1, ArrowOperation::FromCoords)
        };
        assert!(matches!(
            run(&renamed, &input),
            Err(ArrowTransportError::MissingColumn(_))
        ));

        // colonna non numerica.
        let bad_schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Utf8, true),
            Field::new("y", DataType::Float64, true),
        ]));
        let bad_batch = RecordBatch::try_new(
            bad_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![Some("testo")])),
                Arc::new(Float64Array::from(vec![Some(2.0)])),
            ],
        )
        .unwrap();
        let input = envelope_bytes(&bad_schema, &[bad_batch]);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input),
            Err(ArrowTransportError::ColumnNotNumeric { .. })
        ));

        // collisione col nome geometria di output.
        let (schema, batch) = fixture_batch(&[None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input),
            Err(ArrowTransportError::OutputColumnExists(_))
        ));
    }

    #[test]
    fn from_coords_accepts_int64_coordinates() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, true),
            Field::new("y", DataType::Int64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(3_i64)])),
                Arc::new(Int64Array::from(vec![Some(4_i64)])),
            ],
        )
        .unwrap();
        let input = envelope_bytes(&schema, &[batch]);
        let output =
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input).expect("from_coords");
        let (out_schema, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert_eq!(
            geometry_from_wkb(cells.value(0)).unwrap(),
            Geometry::Point(Point::new(3.0, 4.0))
        );
    }

    #[test]
    fn from_coords_rejects_int64_beyond_f64_exact_range() {
        // Oltre 2^53 in valore assoluto la conversione i64 -> f64 non e'
        // esatta: la coordinata va rifiutata, mai spostata in silenzio.
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, true),
            Field::new("y", DataType::Int64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some((1_i64 << 53) + 1)])),
                Arc::new(Int64Array::from(vec![Some(4_i64)])),
            ],
        )
        .unwrap();
        let input = envelope_bytes(&schema, &[batch]);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input),
            Err(ArrowTransportError::IntegerCoordinateTooLarge { .. })
        ));

        // Il confine 2^53 e' esattamente rappresentabile: resta accettato.
        let boundary = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(1_i64 << 53)])),
                Arc::new(Int64Array::from(vec![Some(-(1_i64 << 53))])),
            ],
        )
        .unwrap();
        let input = envelope_bytes(&schema, &[boundary]);
        let output =
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input).expect("from_coords");
        let (out_schema, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert_eq!(
            geometry_from_wkb(cells.value(0)).unwrap(),
            Geometry::Point(Point::new(2_f64.powi(53), -(2_f64.powi(53))))
        );
    }

    // --- Forma binary + lineage ---------------------------------------------

    fn pair_schema(operation: PairOperation, left_rows: u64, right_rows: u64) -> PairArrowSchema {
        PairArrowSchema {
            schema_version: PairArrowSchema::VERSION,
            operation,
            left_row_count: left_rows,
            right_row_count: right_rows,
            left_crs: Some(CRS.to_owned()),
            right_crs: Some(CRS.to_owned()),
            geometry_column: None,
            predicate: None,
            overlay_mode: None,
            max_pairs: None,
            max_comparisons: None,
            max_results: None,
            max_distance: None,
            max_output_rows: None,
            spatial_predicate: None,
            max_coordinate_pairs: None,
            tolerance: None,
        }
    }

    fn run_pair(
        schema: &PairArrowSchema,
        left: &[u8],
        right: &[u8],
    ) -> Result<Vec<u8>, ArrowTransportError> {
        let mut output = Vec::new();
        pair_arrow(left, right, &mut output, schema)?;
        Ok(output)
    }

    fn side_envelope(geometries: &[Option<&[u8]>]) -> Vec<u8> {
        let (schema, batch) = fixture_batch(geometries);
        envelope_bytes(&schema, std::slice::from_ref(&batch))
    }

    fn point_wkb(x: f64, y: f64) -> Vec<u8> {
        Geometry::Point(Point::new(x, y))
            .to_wkb(CoordDimensions::xy())
            .expect("point")
    }

    fn shifted_square_wkb(dx: f64, dy: f64, size: f64) -> Vec<u8> {
        Geometry::Polygon(polygon![
            (x: dx, y: dy), (x: dx + size, y: dy),
            (x: dx + size, y: dy + size), (x: dx, y: dy + size),
            (x: dx, y: dy),
        ])
        .to_wkb(CoordDimensions::xy())
        .expect("polygon")
    }

    #[test]
    fn sjoin_emits_deterministic_pairs_and_skips_nulls() {
        let left = side_envelope(&[
            Some(&shifted_square_wkb(0.0, 0.0, 2.0)),
            None,
            Some(&shifted_square_wkb(10.0, 10.0, 2.0)),
        ]);
        let right = side_envelope(&[
            None,
            Some(&shifted_square_wkb(1.0, 1.0, 2.0)),
            Some(&shifted_square_wkb(20.0, 20.0, 1.0)),
        ]);
        let schema = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(10),
            ..pair_schema(PairOperation::SJoin, 3, 3)
        };
        let output = run_pair(&schema, &left, &right).expect("sjoin");
        let (out_schema, batches) = decode_output(&output);
        assert_eq!(batches.len(), 1);
        let left_index = batches[0]
            .column(out_schema.index_of(LEFT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let right_index = batches[0]
            .column(out_schema.index_of(RIGHT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(left_index.values(), &[0]);
        assert_eq!(right_index.values(), &[1]);
        assert_eq!(out_schema.fields().len(), 2);

        // max_pairs obbligatorio e zero rifiutato.
        let missing = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            ..pair_schema(PairOperation::SJoin, 3, 3)
        };
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "max_pairs",
                ..
            })
        ));
        let zero = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(0),
            ..pair_schema(PairOperation::SJoin, 3, 3)
        };
        assert!(matches!(
            run_pair(&zero, &left, &right),
            Err(ArrowTransportError::InvalidParameter {
                name: "max_pairs",
                ..
            })
        ));

        // row_count lato right non coerente.
        let mismatch = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(10),
            ..pair_schema(PairOperation::SJoin, 3, 2)
        };
        assert!(matches!(
            run_pair(&mismatch, &left, &right),
            Err(ArrowTransportError::PairRowCountMismatch { side: "right", .. })
        ));
    }

    #[test]
    fn distance_is_aligned_to_left_with_nulls_and_limit() {
        let left = side_envelope(&[
            Some(&point_wkb(0.0, 0.0)),
            None,
            Some(&point_wkb(10.0, 0.0)),
        ]);
        let right = side_envelope(&[Some(&point_wkb(3.0, 4.0)), Some(&point_wkb(10.0, 6.0))]);
        let schema = PairArrowSchema {
            max_comparisons: Some(100),
            ..pair_schema(PairOperation::Distance, 3, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("distance");
        let (out_schema, batches) = decode_output(&output);
        // colonne left invariate + distance in coda.
        assert_eq!(out_schema.fields().len(), 5);
        assert!(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).is_ok());
        let values = batches[0]
            .column(out_schema.index_of(DISTANCE_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(values.value(0), 5.0);
        assert!(values.is_null(1));
        assert_eq!(values.value(2), 6.0);
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);

        let limited = PairArrowSchema {
            max_comparisons: Some(1),
            ..pair_schema(PairOperation::Distance, 3, 2)
        };
        assert!(matches!(
            run_pair(&limited, &left, &right),
            Err(ArrowTransportError::Analysis(_))
        ));
    }

    #[test]
    fn nearest_emits_all_ties_with_distance() {
        let left = side_envelope(&[Some(&point_wkb(0.0, 0.0)), None]);
        let right = side_envelope(&[
            Some(&point_wkb(-1.0, 0.0)),
            None,
            Some(&point_wkb(1.0, 0.0)),
            Some(&point_wkb(5.0, 0.0)),
        ]);
        let schema = PairArrowSchema {
            max_comparisons: Some(100),
            max_results: Some(10),
            ..pair_schema(PairOperation::Nearest, 2, 4)
        };
        let output = run_pair(&schema, &left, &right).expect("nearest");
        let (out_schema, batches) = decode_output(&output);
        let left_index = batches[0]
            .column(out_schema.index_of(LEFT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let right_index = batches[0]
            .column(out_schema.index_of(RIGHT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let distances = batches[0]
            .column(out_schema.index_of(DISTANCE_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        // entrambi i pareggi a distanza 1, ordinati per indice right.
        assert_eq!(left_index.values(), &[0, 0]);
        assert_eq!(right_index.values(), &[0, 2]);
        assert_eq!(distances.values(), &[1.0, 1.0]);

        // max_results sotto i pareggi: errore dal kernel.
        let limited = PairArrowSchema {
            max_comparisons: Some(100),
            max_results: Some(1),
            ..pair_schema(PairOperation::Nearest, 2, 4)
        };
        assert!(matches!(
            run_pair(&limited, &left, &right),
            Err(ArrowTransportError::Analysis(_))
        ));

        // max_distance non finita: rifiutata dallo schema.
        let invalid = PairArrowSchema {
            max_comparisons: Some(100),
            max_results: Some(10),
            max_distance: Some(f64::NAN),
            ..pair_schema(PairOperation::Nearest, 2, 4)
        };
        assert!(matches!(
            run_pair(&invalid, &left, &right),
            Err(ArrowTransportError::InvalidParameter {
                name: "max_distance",
                ..
            })
        ));
    }

    #[test]
    fn clip_marks_rows_outside_mask_as_null() {
        let inside = shifted_square_wkb(0.0, 0.0, 2.0);
        let outside = shifted_square_wkb(10.0, 10.0, 2.0);
        let left = side_envelope(&[Some(&inside), None, Some(&outside)]);
        let right = side_envelope(&[None, Some(&shifted_square_wkb(1.0, 1.0, 2.0))]);
        let schema = pair_schema(PairOperation::Clip, 3, 2);
        let output = run_pair(&schema, &left, &right).expect("clip");
        let (out_schema, batches) = decode_output(&output);
        assert_eq!(batches[0].num_rows(), 3);
        let cells = batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        // riga 0: intersezione 1x1; riga 1: null in input; riga 2: fuori maschera -> null.
        let clipped = geometry_from_wkb(cells.value(0)).unwrap();
        assert!((clipped.unsigned_area() - 1.0).abs() < 1e-12);
        assert!(cells.is_null(1));
        assert!(cells.is_null(2));
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);
    }

    #[test]
    fn overlay_emits_pieces_with_nullable_lineage() {
        let left = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0)), None]);
        let right = side_envelope(&[Some(&shifted_square_wkb(1.0, 0.0, 2.0)), None]);

        for (mode, expected_pieces) in [
            (OverlayMode::Intersection, 1_usize),
            (OverlayMode::Union, 3),
            (OverlayMode::SymmetricDifference, 2),
            (OverlayMode::Identity, 2),
        ] {
            let schema = PairArrowSchema {
                overlay_mode: Some(mode),
                max_pairs: Some(10),
                ..pair_schema(PairOperation::Overlay, 2, 2)
            };
            let output = run_pair(&schema, &left, &right).expect("overlay");
            let (out_schema, batches) = decode_output(&output);
            assert_eq!(batches[0].num_rows(), expected_pieces, "mode {mode:?}");
            let left_index = batches[0]
                .column(out_schema.index_of(LEFT_INDEX_COLUMN).unwrap())
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap();
            let right_index = batches[0]
                .column(out_schema.index_of(RIGHT_INDEX_COLUMN).unwrap())
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap();
            match mode {
                OverlayMode::Intersection => {
                    assert!(!left_index.is_null(0) && !right_index.is_null(0));
                }
                OverlayMode::Union | OverlayMode::SymmetricDifference => {
                    // pezzi con un solo lato: l'altro indice e' null.
                    let left_nulls = (0..expected_pieces)
                        .filter(|&i| left_index.is_null(i))
                        .count();
                    let right_nulls = (0..expected_pieces)
                        .filter(|&i| right_index.is_null(i))
                        .count();
                    assert_eq!(left_nulls, 1, "mode {mode:?}");
                    assert_eq!(right_nulls, 1, "mode {mode:?}");
                }
                OverlayMode::Identity => {
                    let right_nulls = (0..expected_pieces)
                        .filter(|&i| right_index.is_null(i))
                        .count();
                    assert_eq!(right_nulls, 1);
                }
            }
            let cells = batches[0]
                .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let total: f64 = (0..expected_pieces)
                .map(|i| geometry_from_wkb(cells.value(i)).unwrap().unsigned_area())
                .sum();
            // Bracci separati per costruzione: ogni OverlayMode ha una
            // semantica diversa; 4.0 coincide per SymmetricDifference e
            // Identity solo su questa fixture, non per lo stesso caso.
            #[allow(clippy::match_same_arms)]
            let expected_area = match mode {
                OverlayMode::Intersection => 2.0,
                OverlayMode::Union => 6.0,
                OverlayMode::SymmetricDifference => 4.0,
                OverlayMode::Identity => 4.0,
            };
            assert!(
                (total - expected_area).abs() < 1e-9,
                "mode {mode:?}: {total}"
            );
        }

        // overlay_mode obbligatorio.
        let missing = PairArrowSchema {
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Overlay, 2, 2)
        };
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "overlay_mode",
                ..
            })
        ));

        // input non poligonale: rifiutato dal kernel.
        let bad_left = side_envelope(&[Some(&point_wkb(0.0, 0.0)), None]);
        let schema = PairArrowSchema {
            overlay_mode: Some(OverlayMode::Intersection),
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Overlay, 2, 2)
        };
        assert!(matches!(
            run_pair(&schema, &bad_left, &right),
            Err(ArrowTransportError::Topology(_))
        ));
    }

    #[test]
    fn pair_requires_both_crs_declarations() {
        let left = side_envelope(&[None]);
        let right = side_envelope(&[None]);
        let schema = PairArrowSchema {
            right_crs: None,
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(1),
            ..pair_schema(PairOperation::SJoin, 1, 1)
        };
        assert!(matches!(
            run_pair(&schema, &left, &right),
            Err(ArrowTransportError::CrsRequired)
        ));
    }

    #[test]
    fn within_outputs_strict_boolean_aligned_to_left() {
        let left = side_envelope(&[
            Some(&point_wkb(0.5, 0.5)), // dentro
            Some(&point_wkb(0.0, 1.0)), // sul bordo: strict-within -> false
            Some(&point_wkb(5.0, 5.0)), // fuori
            None,
        ]);
        let right = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0)), None]);
        let schema = PairArrowSchema {
            max_pairs: Some(100),
            ..pair_schema(PairOperation::Within, 4, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("within");
        let (out_schema, batches) = decode_output(&output);
        // colonne left invariate + `within` Boolean in coda.
        assert_eq!(out_schema.fields().len(), 5);
        let flags = batches[0]
            .column(out_schema.index_of(WITHIN_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
            .unwrap();
        assert!(flags.value(0));
        assert!(!flags.value(1));
        assert!(!flags.value(2));
        assert!(flags.is_null(3));

        let missing = pair_schema(PairOperation::Within, 4, 2);
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "max_pairs",
                ..
            })
        ));
    }

    #[test]
    fn count_points_in_polygons_counts_strict_within_only() {
        let left = side_envelope(&[
            Some(&shifted_square_wkb(0.0, 0.0, 2.0)),
            Some(&shifted_square_wkb(10.0, 10.0, 2.0)),
            None,
        ]);
        let right = side_envelope(&[
            Some(&point_wkb(0.5, 0.5)),
            Some(&point_wkb(1.5, 1.5)),
            Some(&point_wkb(0.0, 1.0)), // bordo: non contato
            Some(&point_wkb(10.5, 10.5)),
            Some(&point_wkb(50.0, 50.0)),
            None,
        ]);
        let schema = PairArrowSchema {
            max_pairs: Some(100),
            ..pair_schema(PairOperation::CountPointsInPolygons, 3, 6)
        };
        let output = run_pair(&schema, &left, &right).expect("count");
        let (out_schema, batches) = decode_output(&output);
        let counts = batches[0]
            .column(out_schema.index_of(COUNT_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(counts.value(0), 2);
        assert_eq!(counts.value(1), 1);
        assert!(counts.is_null(2));
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);
    }

    #[test]
    fn pairwise_booleans_align_rows_and_map_empty_to_null() {
        let overlapping_a = shifted_square_wkb(0.0, 0.0, 2.0);
        let overlapping_b = shifted_square_wkb(1.0, 0.0, 2.0);
        let far = shifted_square_wkb(10.0, 10.0, 1.0);
        let left = side_envelope(&[
            Some(&overlapping_a),
            Some(&overlapping_a),
            Some(&overlapping_a),
            None,
        ]);
        let right = side_envelope(&[
            Some(&overlapping_b),
            Some(&far),
            Some(&overlapping_a),
            Some(&overlapping_b),
        ]);

        let run_op = |operation: PairOperation| {
            let schema = pair_schema(operation, 4, 4);
            let output = run_pair(&schema, &left, &right)
                .unwrap_or_else(|_| panic!("{}", operation.name()));
            let (out_schema, batches) = decode_output(&output);
            let cells = batches[0]
                .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let areas: Vec<Option<f64>> = (0..4)
                .map(|row| {
                    (!cells.is_null(row))
                        .then(|| geometry_from_wkb(cells.value(row)).unwrap().unsigned_area())
                })
                .collect();
            areas
        };

        // intersection: overlap 1x2=2, disgiunti -> null, uguali -> 4, left null -> null.
        let areas = run_op(PairOperation::Intersection);
        assert!((areas[0].unwrap() - 2.0).abs() < 1e-12);
        assert!(areas[1].is_none());
        assert!((areas[2].unwrap() - 4.0).abs() < 1e-12);
        assert!(areas[3].is_none());

        // union: 4+4-2=6; disgiunti 4+1=5; uguali 4; null -> null.
        let areas = run_op(PairOperation::Union);
        assert!((areas[0].unwrap() - 6.0).abs() < 1e-12);
        assert!((areas[1].unwrap() - 5.0).abs() < 1e-12);
        assert!((areas[2].unwrap() - 4.0).abs() < 1e-12);
        assert!(areas[3].is_none());

        // difference: A\B=2; A\far=4; A\A -> null (EMPTY).
        let areas = run_op(PairOperation::Difference);
        assert!((areas[0].unwrap() - 2.0).abs() < 1e-12);
        assert!((areas[1].unwrap() - 4.0).abs() < 1e-12);
        assert!(areas[2].is_none());
        assert!(areas[3].is_none());

        // symmetric_difference: 4; 5; uguale -> null.
        let areas = run_op(PairOperation::SymmetricDifference);
        assert!((areas[0].unwrap() - 4.0).abs() < 1e-12);
        assert!((areas[1].unwrap() - 5.0).abs() < 1e-12);
        assert!(areas[2].is_none());

        // row_count non allineati: fail-closed.
        let mismatched_right = side_envelope(&[Some(&overlapping_b)]);
        let schema = pair_schema(PairOperation::Intersection, 4, 1);
        assert!(matches!(
            run_pair(&schema, &left, &mismatched_right),
            Err(ArrowTransportError::SideLengthMismatch { left: 4, right: 1 })
        ));

        // input non poligonale: rifiutato dal kernel.
        let bad_left = side_envelope(&[Some(&point_wkb(0.0, 0.0))]);
        let bad_right = side_envelope(&[Some(&point_wkb(1.0, 1.0))]);
        let schema = pair_schema(PairOperation::Union, 1, 1);
        assert!(matches!(
            run_pair(&schema, &bad_left, &bad_right),
            Err(ArrowTransportError::Topology(_))
        ));
    }

    #[test]
    fn clean_topology_applies_first_row_wins_and_preserves_positions() {
        let square = shifted_square_wkb(0.0, 0.0, 2.0);
        let duplicate = shifted_square_wkb(0.0, 0.0, 2.0);
        let separate = shifted_square_wkb(10.0, 10.0, 2.0);
        let (schema, batch) =
            fixture_batch(&[Some(&square), Some(&duplicate), Some(&separate), None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let clean_schema = TransformArrowSchema {
            snap_tolerance: Some(0.0),
            ..arrow_schema(4, ArrowOperation::CleanTopology)
        };
        let output = run(&clean_schema, &input).expect("clean_topology");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_batches[0].num_rows(), 4);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        // riga 0 conservata; la duplicata e' assorbita (first-row-wins) -> null;
        // la separata conservata; null in input -> null.
        assert!((geometry_from_wkb(cells.value(0)).unwrap().unsigned_area() - 4.0).abs() < 1e-12);
        assert!(cells.is_null(1));
        assert!((geometry_from_wkb(cells.value(2)).unwrap().unsigned_area() - 4.0).abs() < 1e-12);
        assert!(cells.is_null(3));
        let ids = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2, 3]);

        // snap_tolerance obbligatoria e non negativa.
        let missing = arrow_schema(4, ArrowOperation::CleanTopology);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "snap_tolerance",
                ..
            })
        ));
        let negative = TransformArrowSchema {
            snap_tolerance: Some(-0.5),
            ..arrow_schema(4, ArrowOperation::CleanTopology)
        };
        assert!(matches!(
            run(&negative, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "snap_tolerance",
                ..
            })
        ));

        // input non poligonale: rifiutato dal kernel.
        let line = line_wkb();
        let (schema, batch) = fixture_batch(&[Some(&line)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let single = TransformArrowSchema {
            snap_tolerance: Some(0.0),
            ..arrow_schema(1, ArrowOperation::CleanTopology)
        };
        assert!(matches!(
            run(&single, &input),
            Err(ArrowTransportError::Topology(_))
        ));
    }

    // --- Estensioni di catalogo ----------------------------------------------

    fn single_geometry_output(output: &[u8]) -> Geometry<f64> {
        let (out_schema, out_batches) = decode_output(output);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        geometry_from_wkb(cells.value(0)).expect("decode")
    }

    fn run_single(
        schema: &TransformArrowSchema,
        wkb: &[u8],
    ) -> Result<Vec<u8>, ArrowTransportError> {
        let (fixture_schema, batch) = fixture_batch(&[Some(wkb)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        run(schema, &input)
    }

    #[test]
    fn affine_family_transforms_geometry_and_validates_params() {
        let square = square_wkb(2.0);

        let schema = TransformArrowSchema {
            x_offset: Some(10.0),
            y_offset: Some(-5.0),
            ..arrow_schema(1, ArrowOperation::Translate)
        };
        let output = run_single(&schema, &square).expect("translate");
        let Geometry::Polygon(translated) = single_geometry_output(&output) else {
            panic!("atteso Polygon")
        };
        assert_eq!(translated.exterior().0[0], geo::Coord { x: 10.0, y: -5.0 });

        let schema = TransformArrowSchema {
            x_factor: Some(2.0),
            y_factor: Some(2.0),
            ..arrow_schema(1, ArrowOperation::Scale)
        };
        let output = run_single(&schema, &square).expect("scale");
        assert!((single_geometry_output(&output).unsigned_area() - 16.0).abs() < 1e-12);

        let schema = TransformArrowSchema {
            degrees: Some(90.0),
            ..arrow_schema(1, ArrowOperation::Rotate)
        };
        let output = run_single(&schema, &square).expect("rotate");
        assert!((single_geometry_output(&output).unsigned_area() - 4.0).abs() < 1e-12);

        let schema = TransformArrowSchema {
            coefficients: Some(vec![1.0, 0.0, 5.0, 0.0, 1.0, 5.0]),
            ..arrow_schema(1, ArrowOperation::AffineTransform)
        };
        let output = run_single(&schema, &square).expect("affine");
        let Geometry::Polygon(shifted) = single_geometry_output(&output) else {
            panic!("atteso Polygon")
        };
        assert_eq!(shifted.exterior().0[0], geo::Coord { x: 5.0, y: 5.0 });

        let scattered = Geometry::MultiPoint(geo::MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(4.0, 0.5),
            Point::new(3.5, 3.0),
            Point::new(1.0, 4.0),
            Point::new(-0.5, 2.0),
            Point::new(2.0, 2.0),
        ]))
        .to_wkb(CoordDimensions::xy())
        .expect("scattered");
        let schema = TransformArrowSchema {
            concavity: Some(2.0),
            ..arrow_schema(1, ArrowOperation::ConcaveHull)
        };
        let output = run_single(&schema, &scattered).expect("concave_hull");
        assert!(matches!(
            single_geometry_output(&output),
            Geometry::Polygon(_)
        ));

        // parametri invalidi e non applicabili.
        let missing = arrow_schema(1, ArrowOperation::Translate);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "x_offset",
                ..
            })
        ));
        let bad_coefficients = TransformArrowSchema {
            coefficients: Some(vec![1.0, 0.0, 0.0, 0.0, 1.0]),
            ..arrow_schema(1, ArrowOperation::AffineTransform)
        };
        assert!(matches!(
            run(&bad_coefficients, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "coefficients",
                ..
            })
        ));
        let zero_concavity = TransformArrowSchema {
            concavity: Some(0.0),
            ..arrow_schema(1, ArrowOperation::ConcaveHull)
        };
        assert!(matches!(
            run(&zero_concavity, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "concavity",
                ..
            })
        ));
        let unexpected = TransformArrowSchema {
            x_offset: Some(1.0),
            y_offset: Some(1.0),
            degrees: Some(90.0),
            ..arrow_schema(1, ArrowOperation::Translate)
        };
        assert!(matches!(
            run(&unexpected, &input),
            Err(ArrowTransportError::UnexpectedParameter {
                name: "degrees",
                ..
            })
        ));
    }

    #[test]
    fn densify_and_snap_to_grid_transform_cells() {
        let line = line_wkb();
        let schema = TransformArrowSchema {
            max_segment_length: Some(1.0),
            ..arrow_schema(1, ArrowOperation::Densify)
        };
        let output = run_single(&schema, &line).expect("densify");
        let densified = single_geometry_output(&output);
        assert!(densified.coords_count() > 3);

        let schema = TransformArrowSchema {
            grid_size: Some(0.5),
            ..arrow_schema(1, ArrowOperation::SnapToGrid)
        };
        let output = run_single(&schema, &line).expect("snap");
        let snapped = single_geometry_output(&output);
        assert!(snapped
            .coords_iter()
            .all(|c| (c.x * 2.0).fract() == 0.0 && (c.y * 2.0).fract() == 0.0));

        let (fixture_schema, batch) = fixture_batch(&[Some(&line)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let invalid = TransformArrowSchema {
            max_segment_length: Some(0.0),
            ..arrow_schema(1, ArrowOperation::Densify)
        };
        assert!(matches!(
            run(&invalid, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "max_segment_length",
                ..
            })
        ));
        let invalid_grid = TransformArrowSchema {
            grid_size: Some(-1.0),
            ..arrow_schema(1, ArrowOperation::SnapToGrid)
        };
        assert!(matches!(
            run(&invalid_grid, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "grid_size",
                ..
            })
        ));
    }

    #[test]
    fn line_reference_ops_require_lines_and_valid_ratios() {
        let mut line = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&10.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());

        let schema = TransformArrowSchema {
            start_ratio: Some(0.25),
            end_ratio: Some(0.75),
            ..arrow_schema(1, ArrowOperation::LineSubstring)
        };
        let output = run_single(&schema, &line).expect("substring");
        let Geometry::LineString(piece) = single_geometry_output(&output) else {
            panic!("atteso LineString")
        };
        assert_eq!(piece.0.first().unwrap().x, 2.5);
        assert_eq!(piece.0.last().unwrap().x, 7.5);

        let schema = TransformArrowSchema {
            ratio: Some(0.5),
            ..arrow_schema(1, ArrowOperation::LineInterpolatePoint)
        };
        let output = run_single(&schema, &line).expect("interpolate");
        assert_eq!(
            single_geometry_output(&output),
            Geometry::Point(Point::new(5.0, 0.0))
        );

        let square = square_wkb(1.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let schema = TransformArrowSchema {
            ratio: Some(0.5),
            ..arrow_schema(1, ArrowOperation::LineInterpolatePoint)
        };
        assert!(matches!(
            run(&schema, &input),
            Err(ArrowTransportError::WrongGeometryType { .. })
        ));

        let (fixture_schema, batch) = fixture_batch(&[Some(&line)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let invalid = TransformArrowSchema {
            ratio: Some(1.5),
            ..arrow_schema(1, ArrowOperation::LineInterpolatePoint)
        };
        assert!(matches!(
            run(&invalid, &input),
            Err(ArrowTransportError::InvalidParameter { name: "ratio", .. })
        ));
        let inverted = TransformArrowSchema {
            start_ratio: Some(0.8),
            end_ratio: Some(0.2),
            ..arrow_schema(1, ArrowOperation::LineSubstring)
        };
        assert!(matches!(
            run(&inverted, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "start_ratio/end_ratio",
                ..
            })
        ));
    }

    #[test]
    fn geodesic_unary_ops_measure_lines_and_areas() {
        let mut line = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&1.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        let output = run_single(&arrow_schema(1, ArrowOperation::GeodesicLineLength), &line)
            .expect("geodesic_line_length");
        let (out_schema, out_batches) = decode_output(&output);
        let values = out_batches[0]
            .column(out_schema.index_of("geodesic_line_length").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((values.value(0) - 111_319.5).abs() / 111_319.5 < 1e-3);

        let square = square_wkb(1.0);
        let output = run_single(&arrow_schema(1, ArrowOperation::GeodesicArea), &square)
            .expect("geodesic_area");
        let (out_schema, out_batches) = decode_output(&output);
        let values = out_batches[0]
            .column(out_schema.index_of("geodesic_area").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((values.value(0) - 1.2309e10).abs() / 1.2309e10 < 1e-3);

        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::GeodesicLineLength), &input),
            Err(ArrowTransportError::WrongGeometryType { .. })
        ));
    }

    #[test]
    fn geometry_diagnostics_accepts_invalid_input_and_reports_structure() {
        let mut bowtie = vec![1_u8];
        bowtie.extend_from_slice(&3_u32.to_le_bytes());
        bowtie.extend_from_slice(&1_u32.to_le_bytes());
        bowtie.extend_from_slice(&5_u32.to_le_bytes());
        for (x, y) in [
            (0.0_f64, 0.0_f64),
            (2.0, 2.0),
            (0.0, 2.0),
            (2.0, 0.0),
            (0.0, 0.0),
        ] {
            bowtie.extend_from_slice(&x.to_le_bytes());
            bowtie.extend_from_slice(&y.to_le_bytes());
        }
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&bowtie), Some(&square), None]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let output = run(
            &arrow_schema(3, ArrowOperation::GeometryDiagnostics),
            &input,
        )
        .expect("diagnostics");
        let (out_schema, out_batches) = decode_output(&output);
        let batch = &out_batches[0];
        let column = |name: &str| batch.column(out_schema.index_of(name).unwrap()).clone();

        let is_valid = column("is_valid");
        let is_valid = is_valid
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
            .unwrap();
        assert!(!is_valid.value(0));
        assert!(is_valid.value(1));
        assert!(is_valid.is_null(2));

        let reasons = column("validity_reason");
        let reasons = reasons.as_any().downcast_ref::<StringArray>().unwrap();
        assert!(!reasons.is_null(0));
        assert!(reasons.is_null(1));

        let types = column("geometry_type");
        let types = types.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(types.value(0), "Polygon");
        let counts = column("coordinate_count");
        let counts = counts.as_any().downcast_ref::<UInt64Array>().unwrap();
        assert_eq!(counts.value(1), 5);
        let bounds_maxx = column("bounds_maxx");
        let bounds_maxx = bounds_maxx.as_any().downcast_ref::<Float64Array>().unwrap();
        assert_eq!(bounds_maxx.value(1), 2.0);
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);
    }

    #[test]
    fn delaunay_expands_triangles_with_lineage_and_limit() {
        let multi = Geometry::MultiPoint(geo::MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(3.0, 0.5),
            Point::new(1.0, 2.5),
        ]))
        .to_wkb(CoordDimensions::xy())
        .expect("multi");
        let square = square_wkb(1.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&multi), None, Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let schema = TransformArrowSchema {
            max_output_rows: Some(64),
            ..arrow_schema(3, ArrowOperation::Delaunay)
        };
        let output = run(&schema, &input).expect("delaunay");
        let (out_schema, out_batches) = decode_output(&output);
        let parents = out_batches[0]
            .column(out_schema.index_of(PARENT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let rows = out_batches[0].num_rows();
        assert!(rows >= 2);
        assert!(parents.values().iter().all(|&p| p == 0 || p == 2));
        assert!(parents.values().contains(&0));
        assert!(parents.values().contains(&2));
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let first = geometry_from_wkb(cells.value(0)).unwrap();
        assert!(matches!(first, Geometry::Polygon(_)));
        assert_eq!(first.coords_count(), 4);

        let missing = arrow_schema(3, ArrowOperation::Delaunay);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "max_output_rows",
                ..
            })
        ));
    }

    #[test]
    fn line_merge_merges_maximal_paths_only() {
        let mut lines_wkb = vec![1_u8, 7, 0, 0, 0, 3, 0, 0, 0];
        let segments: [[(f64, f64); 2]; 3] = [
            [(0.0, 0.0), (1.0, 0.0)],
            [(1.0, 0.0), (2.0, 0.0)],
            [(5.0, 5.0), (6.0, 6.0)],
        ];
        for segment in segments {
            lines_wkb.push(1);
            lines_wkb.extend_from_slice(&2_u32.to_le_bytes());
            lines_wkb.extend_from_slice(&2_u32.to_le_bytes());
            for (x, y) in segment {
                lines_wkb.extend_from_slice(&x.to_le_bytes());
                lines_wkb.extend_from_slice(&y.to_le_bytes());
            }
        }
        let (fixture_schema, batch) = fixture_batch(&[Some(&lines_wkb), None]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(2, ArrowOperation::LineMerge), &input).expect("line_merge");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_schema.fields().len(), 1);
        assert_eq!(out_batches[0].num_rows(), 2);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let merged: Vec<usize> = (0..2)
            .map(|row| geometry_from_wkb(cells.value(row)).unwrap().coords_count())
            .collect();
        assert!(merged.contains(&3));
        assert!(merged.contains(&2));
    }

    #[cfg(feature = "geos-backend")]
    #[test]
    fn polygonize_classifies_faces_and_residuals() {
        // quadrato chiuso + dangle: attesi un poligono e un dangle.
        let mut collection = vec![1_u8, 7, 0, 0, 0, 2, 0, 0, 0];
        let ring_groups: [&[(f64, f64)]; 2] = [
            [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)].as_slice(),
            [(1.0, 1.0), (3.0, 3.0)].as_slice(),
        ];
        for ring in ring_groups {
            collection.push(1);
            collection.extend_from_slice(&2_u32.to_le_bytes());
            collection.extend_from_slice(&(ring.len() as u32).to_le_bytes());
            for (x, y) in ring {
                collection.extend_from_slice(&x.to_le_bytes());
                collection.extend_from_slice(&y.to_le_bytes());
            }
        }
        let (fixture_schema, batch) = fixture_batch(&[Some(&collection)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(1, ArrowOperation::Polygonize), &input).expect("polygonize");
        let (out_schema, out_batches) = decode_output(&output);
        let classes = out_batches[0]
            .column(out_schema.index_of(CLASS_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let classes: Vec<&str> = (0..out_batches[0].num_rows())
            .map(|row| classes.value(row))
            .collect();
        assert!(classes.contains(&"polygon"));
        assert!(classes.contains(&"dangle"));
        assert_eq!(classes.len(), 2);
    }

    #[cfg(not(feature = "geos-backend"))]
    #[test]
    fn polygonize_without_geos_fails_closed() {
        let square = square_wkb(1.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Polygonize), &input),
            Err(ArrowTransportError::BackendUnavailable {
                feature: "geos-backend",
                ..
            })
        ));
    }

    #[test]
    fn predicate_pair_op_aligns_rows_and_names_column() {
        let square = shifted_square_wkb(0.0, 0.0, 2.0);
        let inside = point_wkb(1.0, 1.0);
        let boundary = point_wkb(0.0, 1.0);
        let left = side_envelope(&[Some(&square), Some(&square), None]);
        let right = side_envelope(&[Some(&inside), Some(&boundary), Some(&inside)]);
        let schema = PairArrowSchema {
            spatial_predicate: Some(SpatialPredicate::Covers),
            ..pair_schema(PairOperation::Predicate, 3, 3)
        };
        let output = run_pair(&schema, &left, &right).expect("predicate");
        let (out_schema, batches) = decode_output(&output);
        let flags = batches[0]
            .column(out_schema.index_of("predicate_covers").unwrap())
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
            .unwrap();
        assert!(flags.value(0));
        assert!(flags.value(1));
        assert!(flags.is_null(2));

        let missing = pair_schema(PairOperation::Predicate, 3, 3);
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "spatial_predicate",
                ..
            })
        ));

        let short_right = side_envelope(&[Some(&inside)]);
        let schema = PairArrowSchema {
            spatial_predicate: Some(SpatialPredicate::Intersects),
            ..pair_schema(PairOperation::Predicate, 3, 1)
        };
        assert!(matches!(
            run_pair(&schema, &left, &short_right),
            Err(ArrowTransportError::SideLengthMismatch { .. })
        ));
    }

    #[test]
    fn hausdorff_and_frechet_are_pairwise_with_limits() {
        let line_a = line_wkb();
        let line_b = {
            let mut wkb = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
            wkb.extend_from_slice(&0.0_f64.to_le_bytes());
            wkb.extend_from_slice(&1.0_f64.to_le_bytes());
            wkb.extend_from_slice(&3.0_f64.to_le_bytes());
            wkb.extend_from_slice(&5.0_f64.to_le_bytes());
            wkb
        };
        let left = side_envelope(&[Some(&line_a), None]);
        let right = side_envelope(&[Some(&line_b), Some(&line_b)]);

        let schema = PairArrowSchema {
            max_coordinate_pairs: Some(1000),
            ..pair_schema(PairOperation::HausdorffDistance, 2, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("hausdorff");
        let (out_schema, batches) = decode_output(&output);
        let values = batches[0]
            .column(out_schema.index_of("hausdorff_distance").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(values.value(0) > 0.0);
        assert!(values.is_null(1));

        let schema = PairArrowSchema {
            max_coordinate_pairs: Some(1000),
            ..pair_schema(PairOperation::FrechetDistance, 2, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("frechet");
        let (out_schema, batches) = decode_output(&output);
        let values = batches[0]
            .column(out_schema.index_of("frechet_distance").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(values.value(0) > 0.0);

        // tipo sbagliato per frechet e limite di lavoro.
        let square = shifted_square_wkb(0.0, 0.0, 1.0);
        let bad = side_envelope(&[Some(&square), None]);
        let schema = PairArrowSchema {
            max_coordinate_pairs: Some(1000),
            ..pair_schema(PairOperation::FrechetDistance, 2, 2)
        };
        assert!(matches!(
            run_pair(&schema, &bad, &right),
            Err(ArrowTransportError::WrongGeometryType { .. })
        ));
        let schema = PairArrowSchema {
            max_coordinate_pairs: Some(1),
            ..pair_schema(PairOperation::HausdorffDistance, 2, 2)
        };
        assert!(matches!(
            run_pair(&schema, &left, &right),
            Err(ArrowTransportError::Extended(_))
        ));
        let missing = pair_schema(PairOperation::HausdorffDistance, 2, 2);
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "max_coordinate_pairs",
                ..
            })
        ));
    }

    #[test]
    fn geodesic_pair_ops_measure_between_points() {
        let rome = point_wkb(12.0, 41.0);
        let milan = point_wkb(9.0, 45.0);
        let left = side_envelope(&[Some(&rome), None]);
        let right = side_envelope(&[Some(&milan), Some(&milan)]);
        for (operation, column, expected, tolerance) in [
            (
                PairOperation::HaversineDistance,
                "haversine_distance",
                507_205.0,
                0.01,
            ),
            (
                PairOperation::GeodesicDistance,
                "geodesic_distance",
                507_161.0,
                0.01,
            ),
            (PairOperation::Bearing, "bearing", 332.2, 0.05),
        ] {
            let schema = pair_schema(operation, 2, 2);
            let output = run_pair(&schema, &left, &right)
                .unwrap_or_else(|_| panic!("{}", operation.name()));
            let (out_schema, batches) = decode_output(&output);
            let values = batches[0]
                .column(out_schema.index_of(column).unwrap())
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            let actual = values.value(0);
            assert!(
                (actual - expected).abs() / expected < tolerance,
                "{}: {actual} vs {expected}",
                operation.name()
            );
            assert!(values.is_null(1));
        }

        // tipo sbagliato: non Point.
        let square = shifted_square_wkb(0.0, 0.0, 1.0);
        let bad = side_envelope(&[Some(&square), None]);
        let schema = pair_schema(PairOperation::HaversineDistance, 2, 2);
        assert!(matches!(
            run_pair(&schema, &bad, &right),
            Err(ArrowTransportError::WrongGeometryType { .. })
        ));
    }

    #[cfg(feature = "geos-backend")]
    #[test]
    fn split_produces_pieces_with_lineage_and_conserves_measures() {
        let mut line = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&10.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        let cutter = point_wkb(5.0, 0.0);
        let left = side_envelope(&[Some(&line), None]);
        let right = side_envelope(&[Some(&cutter), Some(&cutter)]);
        let schema = PairArrowSchema {
            max_output_rows: Some(16),
            ..pair_schema(PairOperation::Split, 2, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("split lineare");
        let (out_schema, batches) = decode_output(&output);
        assert_eq!(batches[0].num_rows(), 2);
        let parents = batches[0]
            .column(out_schema.index_of(PARENT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(parents.values(), &[0, 0]);
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 0]);
        let cells = batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let total: f64 = (0..2)
            .map(|row| match geometry_from_wkb(cells.value(row)).unwrap() {
                Geometry::LineString(piece) => geo::algorithm::line_measures::Length::length(
                    &geo::algorithm::line_measures::Euclidean,
                    &piece,
                ),
                other => panic!("atteso LineString: {other:?}"),
            })
            .sum();
        assert!((total - 10.0).abs() < 1e-9);

        // split poligonale: quadrato tagliato da una retta verticale.
        let square = shifted_square_wkb(0.0, 0.0, 2.0);
        let mut blade = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
        blade.extend_from_slice(&1.0_f64.to_le_bytes());
        blade.extend_from_slice(&(-1.0_f64).to_le_bytes());
        blade.extend_from_slice(&1.0_f64.to_le_bytes());
        blade.extend_from_slice(&3.0_f64.to_le_bytes());
        let left = side_envelope(&[Some(&square)]);
        let right = side_envelope(&[Some(&blade)]);
        let schema = PairArrowSchema {
            max_output_rows: Some(16),
            ..pair_schema(PairOperation::Split, 1, 1)
        };
        let output = run_pair(&schema, &left, &right).expect("split poligonale");
        let (out_schema, batches) = decode_output(&output);
        assert_eq!(batches[0].num_rows(), 2);
        let cells = batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let total: f64 = (0..2)
            .map(|row| geometry_from_wkb(cells.value(row)).unwrap().unsigned_area())
            .sum();
        assert!((total - 4.0).abs() < 1e-9);

        // tipo sorgente non supportato.
        let point = point_wkb(0.0, 0.0);
        let bad = side_envelope(&[Some(&point)]);
        assert!(matches!(
            run_pair(&schema, &bad, &right),
            Err(ArrowTransportError::WrongGeometryType { .. })
        ));
    }

    #[cfg(not(feature = "geos-backend"))]
    #[test]
    fn split_without_geos_fails_closed() {
        let left = side_envelope(&[None]);
        let right = side_envelope(&[None]);
        let schema = pair_schema(PairOperation::Split, 1, 1);
        assert!(matches!(
            run_pair(&schema, &left, &right),
            Err(ArrowTransportError::BackendUnavailable {
                feature: "geos-backend",
                ..
            })
        ));
    }
}
