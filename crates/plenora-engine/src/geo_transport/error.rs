//! Errori del trasporto Arrow v3 (`ArrowTransportError`) e conversioni dai
//! kernel.

use thiserror::Error;

use plenora_kernels_geo::advanced::AdvancedError;
use plenora_kernels_geo::analysis::AnalysisError;
use plenora_kernels_geo::construction::ConstructionError;
use plenora_core::crs::MAX_CRS_DEFINITION_BYTES;
use plenora_kernels_geo::extended::ExtendedError;
use plenora_kernels_geo::extended_algorithms::ExtendedAlgorithmError;
#[cfg(feature = "geos-backend")]
use plenora_kernels_geo::geos_backend::GeosBackendError;
use plenora_kernels_geo::operations::OperationError;
use plenora_kernels_geo::predicates::PredicateError;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::proj_backend::ProjBackendError;
use super::protocol::{MAX_ROWS, MAX_STREAM_BYTES};
use plenora_kernels_geo::spatial_join::SpatialJoinError;
use plenora_kernels_geo::topology::TopologyError;
use plenora_core::PlenoraError;

use super::transport::{MAX_BATCHES, MAX_CELL_BYTES, MAX_COLUMNS, MAX_IPC_METADATA_BYTES};

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
/// Le varianti `InvalidPlan`/`Unsupported`/`Schema` di `PlenoraError`
/// portano nel payload la stringa ESATTA dell'errore originale, quindi
/// vanno in `Geometry` preservando il messaggio. `Io` conserva l'errore
/// I/O incapsulato. `DataMapping`, `Crs` e `Execution` non hanno una
/// variante dedicata in `ArrowTransportError` (nel flusso del trasporto
/// non si presentano mai: il kernel WKB emette solo errori di
/// contratto/unsupported): sono mappate su `Arrow` mantenendo il testo
/// completo dell'errore.
impl From<PlenoraError> for ArrowTransportError {
    fn from(error: PlenoraError) -> Self {
        match error {
            PlenoraError::InvalidPlan(message)
            | PlenoraError::Unsupported(message)
            | PlenoraError::Schema(message) => Self::Geometry(message),
            PlenoraError::Io(error) => Self::Io(error),
            other => Self::Arrow(other.to_string()),
        }
    }
}
