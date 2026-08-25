//! Errori del trasporto Arrow v3 (`ArrowTransportError`) e conversioni dai
//! kernel.

use thiserror::Error;

use super::protocol::{MAX_ROWS, MAX_STREAM_BYTES};
use plenora_core::crs::MAX_CRS_DEFINITION_BYTES;
use plenora_core::diagnostics::RowDiagnostics;
use plenora_core::PlenoraError;
use plenora_kernels_geo::advanced::AdvancedError;
use plenora_kernels_geo::analysis::AnalysisError;
use plenora_kernels_geo::construction::ConstructionError;
use plenora_kernels_geo::extended::ExtendedError;
use plenora_kernels_geo::extended_algorithms::ExtendedAlgorithmError;
#[cfg(feature = "geos-backend")]
use plenora_kernels_geo::geos_backend::GeosBackendError;
use plenora_kernels_geo::operations::OperationError;
use plenora_kernels_geo::predicates::PredicateError;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::proj_backend::ProjBackendError;
use plenora_kernels_geo::spatial_join::SpatialJoinError;
use plenora_kernels_geo::topology::TopologyError;

use super::transport::{MAX_BATCHES, MAX_CELL_BYTES, MAX_COLUMNS};

/// # Compatibilita' della superficie pubblica
///
/// Questo enum e' `pub` e riesportato da `plenora-engine`. `PR-0` gli aggiunge
/// cinque varianti, il che **rompe** ogni `match` esaustivo scritto fuori dal
/// workspace: la rottura e' accettata formalmente, ed e' il prezzo di
/// distinguere le nuove diagnosi invece di comprimerle in una variante
/// generica — i tre tetti sui custom metadata devono essere superabili
/// separatamente, altrimenti un test non puo' dire quale abbia parato.
///
/// Da qui in avanti l'enum e' `#[non_exhaustive]`, cosi' e' l'ultima volta:
/// un consumatore esterno deve prevedere un ramo generico, e le varianti
/// future smettono di essere una rottura. Dentro il workspace non cambia
/// nulla — nessun `match` era esaustivo, verificato compilando — e la
/// disciplina dei mapping esaustivi resta dove serve, cioe' sulla
/// corrispondenza variante -> categoria.
#[derive(Debug, Error)]
#[non_exhaustive]
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
    /// Metadati oltre il tetto EFFETTIVO (`{1}`), che non e' sempre il
    /// default: quando i limiti derivano da un piano, il tetto e' il budget
    /// di memoria. Il messaggio riportava la costante invece del valore
    /// applicato, e diceva quindi «168 oltre il limite 16777216».
    #[error("metadati messaggio IPC da {0} byte oltre il limite {1}")]
    IpcMetadataTooLarge(usize, usize),
    #[error("stream IPC troncato o non allineato")]
    IpcTruncated,
    /// Body di un messaggio IPC oltre il tetto applicato dal confine PRIMA
    /// che arrow allochi: e' il controllo che `max_batch_bytes` non puo'
    /// fare, perche' misura un `RecordBatch` gia' materializzato.
    #[error("body del messaggio IPC da {declared} byte oltre il limite {limit}")]
    IpcBodyTooLarge { declared: u64, limit: u64 },
    /// Messaggi (o blocchi del footer) oltre il numero ammesso.
    #[error("messaggi IPC {0} oltre il limite {1}")]
    IpcTooManyMessages(usize, usize),
    /// Schema IPC oltre il budget di nodi, oppure con sottoalberi condivisi:
    /// entrambi fanno esplodere l'espansione, qui e dentro arrow.
    #[error("schema IPC oltre il budget di {0} nodi, o con sottoalberi condivisi")]
    IpcSchemaTooComplex(usize),
    /// Record batch oltre il limite semantico del piano (`max_batches`).
    #[error("record batch IPC {0} oltre il limite {1}")]
    IpcTooManyRecordBatches(usize, usize),
    /// Byte o messaggi dopo il marcatore di fine stream: il reader li
    /// ignorerebbe, il validatore non li ha visti.
    #[error("byte dopo il marcatore di fine stream IPC")]
    IpcTrailingAfterEos,
    /// Costrutto IPC che il confine non sa limitare, e che quindi rifiuta
    /// invece di lasciar passare non misurato. Il messaggio nomina il
    /// costrutto, mai i dati.
    #[error("costrutto IPC non ammesso dal confine: {0}")]
    IpcUnsupportedFeature(&'static str),
    /// Footer del file format incoerente: blocchi fuori dalla regione dati,
    /// sovrapposti o non allineati.
    #[error("footer IPC incoerente: {0}")]
    IpcFooterInvalid(&'static str),
    /// Schema IPC di forma non ammessa: un campo che `arrow-ipc` dereferenzia
    /// senza controllarlo — `fields` dello schema, `indexType` di una codifica
    /// a dizionario — e che il confine pretende invece di lasciar passare.
    ///
    /// Distinta da [`ArrowTransportError::IpcFooterInvalid`], che riguarda il
    /// footer del file format: uno schema di stream non ha footer.
    #[error("schema IPC di forma non ammessa: {0}")]
    IpcSchemaInvalid(&'static str),
    /// Custom metadata IPC di forma non ammessa: chiave o valore assenti,
    /// chiave vuota, UTF-8 non valido, chiave duplicata.
    ///
    /// Il messaggio nomina la violazione, **mai** la chiave o il valore: sono
    /// dati di chi ha prodotto il file.
    #[error("custom metadata IPC non validi: {0}")]
    IpcMetadataInvalid(&'static str),
    /// Coppie di custom metadata oltre il tetto in UNA collezione.
    ///
    /// Distinta dalle due che seguono di proposito: i tre tetti vanno
    /// superabili separatamente, altrimenti un test non puo' dire quale abbia
    /// parato.
    #[error("custom metadata IPC: {0} coppie oltre il limite {1}")]
    IpcTooManyMetadataPairs(usize, usize),
    /// Chiave di custom metadata oltre il tetto in byte.
    #[error("custom metadata IPC: chiave da {0} byte oltre il limite {1}")]
    IpcMetadataKeyTooLarge(usize, usize),
    /// Valore di custom metadata oltre il tetto in byte.
    #[error("custom metadata IPC: valore da {0} byte oltre il limite {1}")]
    IpcMetadataValueTooLarge(usize, usize),
    /// Invariante interna violata: parametro gia' validato a monte o caso
    /// gia' ristretto dal dispatch. Indica un difetto del trasporto, non
    /// dell'input; il messaggio nomina solo il parametro o il caso, mai dati.
    #[error("errore interno trasporto Arrow: {0}")]
    Internal(&'static str),
    #[error("decodifica Arrow IPC fallita: {0}")]
    Arrow(String),
    /// `arrow-ipc` e' andato in panico decodificando lo schema del payload.
    ///
    /// Non e' un errore nostro ne' un difetto del chiamante: `fb_to_schema`
    /// contiene venti `panic!`/`unimplemented!` raggiungibili da un `FlatBuffer`
    /// non fidato, e i reader la chiamano sempre. Le API che la avvolgono si
    /// chiamano `try_*` ma sono fallibili solo sul parsing esterno: appena
    /// ottengono lo schema fanno `.map(fb_to_schema)`.
    ///
    /// La variante esiste per distinguerlo da `Arrow(String)`, che rappresenta
    /// un errore che la libreria ha *restituito*. Qui la libreria e' abortita,
    /// e la differenza va resa visibile invece che appiattita.
    #[error("arrow-ipc in panico sullo schema del payload: {0}")]
    ArrowPanic(String),
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
    /// Errore con diagnostica row-scoped conforme al contratto trasversale
    /// `plenora-row-diagnostics-v1` (R9.9-R9.12): testo e variante della
    /// causa primaria restano invariati; il payload e' bounded, senza valori.
    #[error("{source}")]
    RowDiagnostics {
        /// Causa primaria.
        source: Box<Self>,
        /// Payload machine-readable validato all'emissione.
        diagnostics: Box<RowDiagnostics>,
    },
}

impl ArrowTransportError {
    /// Errore restituito da arrow-rs, **sanificato**.
    ///
    /// Il testo di arrow-rs cita regolarmente il valore che ha causato il
    /// difetto: farlo attraversare il confine cosi' com'e' violerebbe la
    /// regola «errori senza dati» (errori-e-limiti.md#privacy-dei-messaggi)
    /// e legherebbe la privacy dei nostri errori al comportamento di una
    /// dipendenza. Passa quindi il solo codice della variante
    /// ([`plenora_core::error::arrow_error_code`]), che dice che genere di
    /// difetto e' senza dire su quale dato.
    #[must_use]
    pub fn arrow(error: &plenora_core::arrow::ArrowError) -> Self {
        Self::Arrow(format!(
            "arrow error: {}",
            plenora_core::error::arrow_error_code(error)
        ))
    }

    /// Associa un payload row-scoped senza alterare testo o variante
    /// dell'errore; un payload non valido degrada a `Internal` (mai
    /// pubblicare diagnostica non conforme).
    #[must_use]
    pub fn with_row_diagnostics(self, diagnostics: RowDiagnostics) -> Self {
        if diagnostics.validate_for_emission().is_err() {
            return Self::Internal("row diagnostics interne non valide");
        }
        Self::RowDiagnostics {
            source: Box::new(self),
            diagnostics: Box::new(diagnostics),
        }
    }

    /// Restituisce il payload row-scoped, anche attraverso wrapper annidati.
    #[must_use]
    pub fn row_diagnostics(&self) -> Option<&RowDiagnostics> {
        match self {
            Self::RowDiagnostics { diagnostics, .. } => Some(diagnostics),
            _ => None,
        }
    }

    /// L'errore causale sotto eventuali wrapper `RowDiagnostics` (per i
    /// confronti di variante/messaggio, che la diagnostica non altera).
    #[must_use]
    pub fn source_error(&self) -> &Self {
        match self {
            Self::RowDiagnostics { source, .. } => source.source_error(),
            _ => self,
        }
    }

    /// Come `source_error`, per consumo: scarta i wrapper `RowDiagnostics`
    /// e restituisce l'errore causale (usato prima di ri-allegare un report
    /// aggregato, mai per duplicare diagnostica).
    #[must_use]
    pub fn into_source(self) -> Self {
        match self {
            Self::RowDiagnostics { source, .. } => source.into_source(),
            _ => self,
        }
    }
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
/// completo dell'errore. Il wrapper di fase `Tagged` (BLOCK-03) e'
/// attraversato: il tag riguarda l'asse fase, che il trasporto non porta —
/// la conversione vede la variante interna, esattamente come senza tag.
impl From<PlenoraError> for ArrowTransportError {
    fn from(error: PlenoraError) -> Self {
        match error {
            PlenoraError::InvalidPlan(message)
            | PlenoraError::Unsupported(message)
            | PlenoraError::Schema(message) => Self::Geometry(message),
            PlenoraError::Io(error) => Self::Io(error),
            PlenoraError::Tagged { source, .. } => Self::from(*source),
            PlenoraError::RowDiagnostics {
                source,
                diagnostics,
            } => Self::from(*source).with_row_diagnostics(*diagnostics),
            other => Self::Arrow(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use plenora_core::ErrorPhase;

    use super::*;

    #[test]
    fn le_varianti_di_contratto_diventano_geometry_con_il_payload_esatto() {
        // InvalidPlan/Unsupported/Schema portano nel payload la stringa
        // ESATTA dell'errore originale: Geometry la preserva verbatim, senza
        // il prefisso di Display di PlenoraError.
        for source in [
            PlenoraError::InvalidPlan("contratto violato".into()),
            PlenoraError::Unsupported("operazione assente".into()),
            PlenoraError::Schema("schema incoerente".into()),
        ] {
            let payload = match &source {
                PlenoraError::InvalidPlan(message)
                | PlenoraError::Unsupported(message)
                | PlenoraError::Schema(message) => message.clone(),
                other => panic!("variante inattesa: {other:?}"),
            };
            let converted = ArrowTransportError::from(source);
            let ArrowTransportError::Geometry(message) = &converted else {
                panic!("atteso Geometry, ottenuto {converted:?}");
            };
            assert_eq!(message, &payload);
        }
    }

    #[test]
    fn io_e_preservato_come_errore_io_tipizzato() {
        let source = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe chiusa");
        let converted = ArrowTransportError::from(PlenoraError::Io(source));
        let ArrowTransportError::Io(error) = &converted else {
            panic!("atteso Io, ottenuto {converted:?}");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn il_wrapper_di_fase_e_attraversato_fino_alla_variante_interna() {
        // Tagged su una variante di contratto: la conversione vede la
        // variante interna (Geometry), esattamente come senza tag.
        let source = PlenoraError::Schema("schema incoerente".into()).with_phase(ErrorPhase::Read);
        let converted = ArrowTransportError::from(source);
        let ArrowTransportError::Geometry(message) = &converted else {
            panic!("atteso Geometry, ottenuto {converted:?}");
        };
        assert_eq!(message, "schema incoerente");
        // Tagged su una variante senza controparte: stessa traversata, Arrow
        // con il testo Display completo della variante interna.
        let source = PlenoraError::Crs("crs irrisolvibile".into()).with_phase(ErrorPhase::Validate);
        let converted = ArrowTransportError::from(source);
        let ArrowTransportError::Arrow(message) = &converted else {
            panic!("atteso Arrow, ottenuto {converted:?}");
        };
        assert_eq!(message, "CRS error: crs irrisolvibile");
    }

    #[test]
    fn le_varianti_senza_controparte_diventano_arrow_con_testo_completo() {
        // DataMapping/Crs/Execution non si presentano nel flusso del
        // trasporto: mappate su Arrow mantenendo il testo completo.
        for source in [
            PlenoraError::DataMapping("valore fuori dominio".into()),
            PlenoraError::Crs("crs irrisolvibile".into()),
            PlenoraError::Execution {
                node: "n1".into(),
                operation: "geo.buffer".into(),
                execution_id: String::new(),
                reason: "kernel fallito".into(),
            },
        ] {
            let expected = source.to_string();
            let converted = ArrowTransportError::from(source);
            let ArrowTransportError::Arrow(message) = &converted else {
                panic!("atteso Arrow, ottenuto {converted:?}");
            };
            assert_eq!(message, &expected);
        }
    }
}
