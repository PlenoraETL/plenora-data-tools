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
/// Questo enum e' `pub`, riesportato da `plenora-engine` e
/// `#[non_exhaustive]`: un consumatore esterno deve prevedere un ramo
/// generico, e aggiungere una variante non e' una rottura.
///
/// Non lo e' sempre stato. Le cinque diagnosi sui tetti dei custom metadata
/// hanno rotto i `match` esaustivi scritti fuori dal workspace — rottura
/// accettata formalmente, ed e' il prezzo di distinguerle invece di
/// comprimerle in una variante generica: i tre tetti devono essere superabili
/// separatamente, altrimenti un test non puo' dire quale abbia parato.
/// `#[non_exhaustive]` e' cio' che rende quella l'ultima volta.
///
/// Dentro il workspace nessun `match` su questo enum e' esaustivo, e la
/// disciplina dei mapping esaustivi resta dove serve: sulla corrispondenza
/// variante -> categoria.
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
    /// di memoria. Riportare la costante invece del valore applicato farebbe
    /// dire al messaggio «168 oltre il limite 16777216».
    #[error("metadati messaggio IPC da {0} byte oltre il limite {1}")]
    IpcMetadataTooLarge(usize, usize),
    #[error("stream IPC troncato o non allineato")]
    IpcTruncated,
    /// Body di un messaggio IPC oltre il tetto applicato dal confine PRIMA
    /// che arrow allochi: e' il controllo che `max_batch_bytes` non puo'
    /// fare, perche' misura un `RecordBatch` gia' materializzato.
    #[error("body del messaggio IPC da {declared} byte oltre il limite {limit}")]
    IpcBodyTooLarge { declared: u64, limit: u64 },
    /// Somma dei body dei blocchi dizionario oltre il tetto del verificatore.
    ///
    /// E' un tetto **cumulativo**, e non lo copre `IpcBodyTooLarge`: mille
    /// dizionari da un megabyte stanno ciascuno sotto `max_body_bytes` e
    /// insieme trattengono un gigabyte, perche' `FileReader` li decodifica
    /// tutti all'apertura e li tiene vivi per l'intera scansione. Il
    /// controllo e' sui `bodyLength` DICHIARATI nel footer, quindi avviene
    /// prima che arrow ne decodifichi uno.
    #[error("body dei dizionari IPC da {declared} byte oltre il limite {limit}")]
    IpcRetainedDictionariesTooLarge { declared: u64, limit: u64 },
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
    /// Un errore che **sotto** e' gia' `Internal`, e resta tale.
    ///
    /// Senza questa variante un `PlenoraError::Internal` cade nel ramo
    /// generico e diventa `Arrow(String)`, cioe' un errore della libreria
    /// arrow; poi il passo dell'executor lo riscrive `InvalidPlan`. Un difetto
    /// nostro diventerebbe cosi' una colpa del piano, che e' l'attribuzione
    /// sbagliata e manda chi legge a correggere un ingresso sano.
    ///
    /// Il caso che la rende necessaria e' la validazione OGC che **non
    /// conclude**: vedi `errori-e-limiti.md`.
    #[error("errore interno propagato: {0}")]
    Interno(String),
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
    /// L'errore di un passo del kernel, **attribuito a chi ne ha colpa**.
    ///
    /// # Perche' non basta `InvalidPlan`
    ///
    /// Perche' non ogni fallimento di un passo e' colpa del piano. Un difetto
    /// nostro — o una validazione che non ha concluso, che non ha giudicato
    /// nulla — chiamato `InvalidPlan` manda chi legge a correggere un ingresso
    /// che nessuno ha dimostrato sbagliato, e ne cambia l'exit code.
    ///
    /// # Perche' qui e non nei due chiamanti
    ///
    /// Perche' i chiamanti sono due — il passo fuso e quello non fuso — e due
    /// copie della stessa decisione divergono: e' gia' successo che un ramo
    /// conservasse la diagnostica di riga e l'altro no.
    pub(crate) fn errore_del_passo(&self) -> PlenoraError {
        if self.source_error().e_interna() {
            return PlenoraError::Internal(self.to_string());
        }
        PlenoraError::InvalidPlan(self.to_string())
    }

    /// **La colpa non e' di chi ha scritto il piano.**
    ///
    /// Due famiglie: gli errori gia' interni qui — o una libreria abortita — e
    /// gli esiti tipizzati dei kernel che dicono «la validazione OGC non ha
    /// concluso». I secondi non sono un giudizio sull'ingresso: nessuno lo ha
    /// dato, e attribuirlo al piano manda chi legge a correggere una geometria
    /// che nessuno ha dimostrato sbagliata.
    ///
    /// Va chiamata sulla **causa**, non sull'involucro: `RowDiagnostics`
    /// avvolge senza cambiare cio' che e' andato storto, e classificare
    /// l'involucro farebbe cadere nel ramo generico ogni errore interno che
    /// porti una diagnostica di riga. La traversata e' quella di
    /// [`Self::source_error`], che esiste gia' per lo stesso motivo.
    // NON `const`: il ramo `geos-backend` chiama `category()`, che non lo e'.
    // Clippy suggerisce `const fn` perche' nella configurazione predefinita
    // quel ramo non e' compilato — e il suggerimento, preso alla lettera,
    // rompe la build con la feature attiva. Guardare la categoria invece
    // della variante e' cio' che permette di riconoscere un `Internal`
    // avvolto in `Tagged` o in una diagnostica di riga, e vale piu' della
    // constness.
    #[allow(clippy::missing_const_for_fn)]
    pub(crate) fn e_interna(&self) -> bool {
        use plenora_kernels_geo::advanced::AdvancedError as A;
        use plenora_kernels_geo::analysis::AnalysisError as An;
        use plenora_kernels_geo::construction::ConstructionError as C;
        use plenora_kernels_geo::extended::ExtendedError as E;
        use plenora_kernels_geo::extended_algorithms::ExtendedAlgorithmError as Ea;
        use plenora_kernels_geo::operations::OperationError as O;
        use plenora_kernels_geo::predicates::PredicateError as P;
        use plenora_kernels_geo::spatial_join::SpatialJoinError as S;
        use plenora_kernels_geo::topology::TopologyError as T;

        match self {
            Self::Interno(_) | Self::Internal(_) | Self::ArrowPanic(_) => true,
            Self::Kernel(O::ValidazioneNonConclusa(_) | O::Internal(_))
            | Self::Topology(T::ValidazioneNonConclusa(_))
            | Self::Construction(C::ValidazioneNonConclusa(_))
            | Self::Advanced(A::ValidazioneNonConclusa(_))
            | Self::Extended(E::ValidazioneNonConclusa(_))
            | Self::ExtendedAlgorithm(Ea::ValidazioneNonConclusa(_) | Ea::Internal(_))
            | Self::Predicate(P::ValidazioneNonConclusa(_))
            | Self::Analysis(An::ValidazioneNonConclusa(_))
            | Self::SpatialJoin(S::ValidazioneNonConclusa(_) | S::Internal(_)) => true,
            #[cfg(feature = "proj-backend")]
            Self::Reproject(
                plenora_kernels_geo::proj_backend::ProjBackendError::ValidazioneNonConclusa(_),
            ) => true,
            // Il backend GEOS non ha una variante propria: incapsula il
            // `PlenoraError` del contratto, quindi si guarda quello.
            #[cfg(feature = "geos-backend")]
            Self::MakeValid(GeosBackendError::InputContract(errore)) => {
                errore.category() == plenora_core::ErrorCategory::Internal
            }
            _ => false,
        }
    }

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
/// `transform_wkb`, `validate_wkb_contract`), che rendono `PlenoraError`.
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
            // Un `Internal` resta interno. Nel ramo generico diventerebbe
            // `Arrow`, e il passo dell'executor lo riscriverebbe `InvalidPlan`:
            // un difetto nostro finirebbe attribuito a chi ha scritto il piano.
            PlenoraError::Internal(message) => Self::Interno(message),
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

    /// **La causa si classifica attraverso gli involucri.**
    ///
    /// `RowDiagnostics` avvolge senza cambiare la causa. Classificare
    /// l'involucro invece del contenuto farebbe cadere nel ramo generico —
    /// cioe' su `InvalidPlan` — ogni errore interno che porti una diagnostica
    /// di riga, e l'attribuzione tornerebbe a chi ha scritto il piano.
    #[test]
    fn un_errore_interno_avvolto_resta_interno() {
        use plenora_core::diagnostics::{
            RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
            ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
        };

        let mut counts = std::collections::BTreeMap::new();
        counts.insert("geometry.invalid_wkb".to_owned(), 1_u64);
        let diagnostics = RowDiagnostics {
            contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
            scope: RowDiagnosticScope::Read,
            index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
            completeness: RowDiagnosticsCompleteness::Complete,
            knowledge_limits: None,
            observed_total: 1,
            total: Some(1),
            input_total: None,
            counts,
            examples_limit: 10,
            examples_truncated: false,
            diagnostic_state_counts: None,
            write_outcome: None,
            examples: vec![RowDiagnosticExample {
                source_index: 0,
                cause: "geometry.invalid_wkb".to_owned(),
                column: None,
                key: None,
                write_state: None,
            }],
        };

        let avvolto = ArrowTransportError::Interno("difetto nostro".to_owned())
            .with_row_diagnostics(diagnostics);
        assert!(
            matches!(avvolto, ArrowTransportError::RowDiagnostics { .. }),
            "l'involucro deve esserci, o il caso non prova niente"
        );
        assert_eq!(
            avvolto.errore_del_passo().category(),
            plenora_core::ErrorCategory::Internal,
            "un interno avvolto resta interno: {avvolto}"
        );
    }

    /// **Gli esiti tipizzati dei kernel sono riconosciuti.**
    ///
    /// La variante `ValidazioneNonConclusa` esiste in quattordici enum, e il
    /// classificatore deve vederla in tutti: riconoscerne uno solo lascerebbe
    /// gli altri tredici ad attribuire al piano un difetto che non e' suo.
    #[test]
    fn la_validazione_non_conclusa_dei_kernel_e_interna() {
        use plenora_kernels_geo::advanced::AdvancedError;
        use plenora_kernels_geo::analysis::AnalysisError;
        use plenora_kernels_geo::construction::ConstructionError;
        use plenora_kernels_geo::extended::ExtendedError;
        use plenora_kernels_geo::extended_algorithms::ExtendedAlgorithmError;
        use plenora_kernels_geo::operations::OperationError;
        use plenora_kernels_geo::predicates::PredicateError;
        use plenora_kernels_geo::spatial_join::SpatialJoinError;
        use plenora_kernels_geo::topology::TopologyError;

        let mut casi = vec![
            ArrowTransportError::Kernel(OperationError::ValidazioneNonConclusa("forma")),
            ArrowTransportError::Topology(TopologyError::ValidazioneNonConclusa("forma")),
            ArrowTransportError::Construction(ConstructionError::ValidazioneNonConclusa("forma")),
            ArrowTransportError::Advanced(AdvancedError::ValidazioneNonConclusa("forma")),
            ArrowTransportError::Extended(ExtendedError::ValidazioneNonConclusa("forma")),
            ArrowTransportError::ExtendedAlgorithm(ExtendedAlgorithmError::ValidazioneNonConclusa(
                "forma",
            )),
            ArrowTransportError::Predicate(PredicateError::ValidazioneNonConclusa("forma")),
            ArrowTransportError::Analysis(AnalysisError::ValidazioneNonConclusa("forma")),
            ArrowTransportError::SpatialJoin(SpatialJoinError::ValidazioneNonConclusa("forma")),
        ];
        // I due backend opzionali: senza la feature il ramo non e' compilato,
        // e il caso non puo' pretenderlo. Con la feature, si', ed e' li' che
        // un mapping dimenticato si vedrebbe.
        #[cfg(feature = "proj-backend")]
        casi.push(ArrowTransportError::Reproject(
            plenora_kernels_geo::proj_backend::ProjBackendError::ValidazioneNonConclusa("forma"),
        ));
        #[cfg(feature = "geos-backend")]
        casi.push(ArrowTransportError::MakeValid(
            plenora_kernels_geo::geos_backend::GeosBackendError::InputContract(
                plenora_core::PlenoraError::Internal("difetto nostro".to_owned()),
            ),
        ));

        for caso in &casi {
            assert_eq!(
                caso.errore_del_passo().category(),
                plenora_core::ErrorCategory::Internal,
                "esito non riconosciuto: {caso}"
            );
        }

        // Il conteggio fissa la tabella **scritta qui**: se un caso venisse
        // tolto, o se un `cfg` ne facesse cadere uno senza che nessuno se ne
        // accorga, il numero non torna.
        //
        // Non scopre un kernel NUOVO che acquisti la variante e non venga
        // aggiunto: in quel caso non cambierebbero ne' il vettore ne' questo
        // numero, e il caso resterebbe verde. La completezza rispetto ai rami
        // che `e_interna` riconosce OGGI e' stata verificata confrontando i
        // due elenchi a mano — undici rami di kernel qui, i tre non-kernel in
        // `i_rami_interni_non_kernel_sono_riconosciuti` — non da questa
        // asserzione.
        let attesi = 9
            + usize::from(cfg!(feature = "proj-backend"))
            + usize::from(cfg!(feature = "geos-backend"));
        assert_eq!(
            casi.len(),
            attesi,
            "la tabella e' cambiata di dimensione rispetto a quella dichiarata"
        );
    }

    /// **Anche i tre rami interni non-kernel sono riconosciuti.**
    ///
    #[test]
    fn gli_internal_dei_kernel_non_diventano_invalidplan() {
        let cases = [
            ArrowTransportError::Kernel(OperationError::Internal("forma")),
            ArrowTransportError::ExtendedAlgorithm(ExtendedAlgorithmError::Internal("forma")),
            ArrowTransportError::SpatialJoin(SpatialJoinError::Internal("forma")),
        ];
        for case in cases {
            assert!(case.e_interna());
            let error = case.errore_del_passo();
            assert_eq!(error.category(), plenora_core::ErrorCategory::Internal);
            assert!(error.row_diagnostics().is_none());
        }
    }

    /// `e_interna` ha due famiglie: gli esiti tipizzati dei kernel, coperti dai
    /// casi qui sopra, e questi. Verificarli separatamente e' cio' che rende
    /// completo il confronto a mano fra la tabella e i rami della funzione.
    #[test]
    fn i_rami_interni_non_kernel_sono_riconosciuti() {
        let casi = [
            ArrowTransportError::Internal("difetto nostro"),
            ArrowTransportError::Interno("gia' interno sotto".to_owned()),
            ArrowTransportError::ArrowPanic("arrow abortita".to_owned()),
        ];
        for caso in &casi {
            assert_eq!(
                caso.errore_del_passo().category(),
                plenora_core::ErrorCategory::Internal,
                "ramo interno non riconosciuto: {caso}"
            );
        }
    }

    /// **Una geometria davvero invalida resta colpa del piano.**
    ///
    /// La meta' che rende la distinzione una distinzione: se ogni esito
    /// diventasse `Internal`, i casi qui sopra passerebbero senza dire nulla.
    #[test]
    fn una_geometria_invalida_resta_colpa_del_piano() {
        use plenora_kernels_geo::operations::OperationError;

        let caso = ArrowTransportError::Kernel(OperationError::InvalidInput(
            "anello con auto-intersezione".to_owned(),
        ));
        assert_eq!(
            caso.errore_del_passo().category(),
            plenora_core::ErrorCategory::InvalidPlan,
            "un ingresso giudicato invalido resta attribuito al piano: {caso}"
        );
    }

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
