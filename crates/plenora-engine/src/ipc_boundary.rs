//! Confine unico di lettura Arrow IPC per gli ingressi non fidati.
//!
//! `arrow-ipc` 59.2.0 ha due esposizioni su input ostile, raggiungibili dal
//! primo byte: **panico** in `convert::fb_to_schema`, chiamata da ogni
//! lettore anche dalle API `try_*` (apache/arrow-rs#10575), e **allocazione
//! ostile** da conteggi flatbuffer senza tetto, da cui `catch_unwind` non
//! protegge. La difesa e' pre-validare framing e limiti prima che arrow veda
//! i byte, e chiamare arrow dentro una barriera anti-panico. Tutti gli
//! ingressi file/stream/CLI passano di qui.
//!
//! Non copre i file di spill e i temp store, prodotti da noi. Non copre la
//! mutazione in place dell'ingresso durante la lettura: un solo handle
//! ([`validated_handle`] rende il file riavvolto, mai il percorso) esclude la
//! sostituzione del path ma non uno scrittore concorrente sullo stesso inode,
//! e in quel caso decade il tetto sulle allocazioni. Dichiarato in
//! errori-e-limiti.md#limiti-dichiarati.

use std::fs::File;
use std::io::Read as _;
use std::panic::AssertUnwindSafe;
use std::path::Path;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::reader::{FileReader, StreamReader};
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::panic_policy::barriera_di_dipendenza;
use plenora_core::{ErrorPhase, PlenoraError, Result};

use crate::geo_transport::error::ArrowTransportError;
use crate::geo_transport::ipc::{
    descrivi_panico, valida_file_e_rendi_footer, validate_ipc_file_framing,
    validate_ipc_stream_framing, SeekSource,
};
pub use crate::geo_transport::ipc::{IpcLimits, DEFAULT_MAX_BODY_BYTES, MAX_TOTAL_IPC_MESSAGES};

/// Formato del contenitore IPC di un ingresso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcFormat {
    /// File format (`ARROW1` in testa, footer in coda).
    File,
    /// Stream format (sequenza di messaggi incapsulati).
    Stream,
}

/// Traduce un errore del validatore di confine in `PlenoraError`, taggato
/// [`ErrorPhase::Read`]: nasce leggendo la sorgente (BLOCK-03).
///
/// I tetti di risorsa del confine escono come [`PlenoraError::ResourceLimit`],
/// su cui il chiamante decide di rilanciare con piu' budget; il framing
/// malformato resta `data_mapping`, perche' li' il file e' davvero rotto.
pub(crate) fn read_error(error: ArrowTransportError) -> PlenoraError {
    // Il tag di fase si applica **una volta sola**, qui: dentro la
    // traduzione, il ramo ricorsivo della diagnostica produrrebbe tag
    // annidati (`with_phase` non riavvolge un `Tagged`, ma dopo
    // `with_row_diagnostics` l'esterno non lo e' piu').
    traduci_errore_di_lettura(error).with_phase(ErrorPhase::Read)
}

/// Traduce un errore del trasporto **senza** applicare la fase.
///
/// Separare la traduzione dal tagging e' cio' che permette al ramo ricorsivo
/// di comporre senza annidare: la fase la mette [`read_error`], una volta.
fn traduci_errore_di_lettura(error: ArrowTransportError) -> PlenoraError {
    use ArrowTransportError as E;

    /// Testo di un errore che il validatore IPC non puo' produrre.
    ///
    /// Statico e senza dati: nomina il punto, non l'input.
    const IMPOSSIBILE_AL_CONFINE: &str =
        "errore di esecuzione riportato dal lettore di confine IPC";

    match error {
        // --- I/O: la causa si conserva, non si stampa ----------------------
        //
        // Come `DataMapping`, un disco che non risponde passerebbe per un file
        // corrotto. Consumare l'errore conserva lo `std::io::Error` originale.
        E::Io(io) => PlenoraError::Io(io),

        // --- Limiti: il file c'e' ed e' piu' grande di quanto ammettiamo ---
        E::StreamTooLarge
        | E::TooManyRows(_)
        | E::TooManyColumns(_)
        | E::TooManyBatches(_)
        | E::CellTooLarge(_)
        | E::IpcMetadataTooLarge(_, _)
        | E::IpcBodyTooLarge { .. }
        | E::IpcRetainedDictionariesTooLarge { .. }
        | E::IpcTooManyMessages(_, _)
        | E::IpcSchemaTooComplex(_)
        | E::IpcTooManyRecordBatches(_, _)
        | E::IpcTooManyMetadataPairs(_, _)
        | E::IpcMetadataKeyTooLarge(_, _)
        | E::IpcMetadataValueTooLarge(_, _) => PlenoraError::ResourceLimit(error_testo(&error)),

        // --- Difetto nostro -------------------------------------------------
        E::Internal(motivo) => PlenoraError::Internal(motivo.to_owned()),
        // E' gia' interno sotto — per esempio una validazione OGC che non
        // conclude — e resta tale: il testo e' nostro e gia' sanitizzato.
        E::Interno(motivo) => PlenoraError::Internal(motivo),

        // --- Forma non ammessa: qui il file e' davvero rotto ---------------
        //
        // Framing, envelope, schema, custom metadata, contratto: tutto cio'
        // che dice «questi byte non sono cio' che dichiarano di essere».
        E::InvalidMagic
        | E::InvalidTrailer
        | E::ChecksumMismatch
        | E::TrailingBytes
        | E::RowCountMismatch { .. }
        | E::PayloadLengthMismatch { .. }
        | E::UnsupportedSchemaVersion(_)
        | E::MissingGeometryColumn(_)
        | E::MissingGeoArrowMetadata(_)
        | E::GeometryColumnNotBinary { .. }
        | E::CrsRequired
        | E::CrsTooLarge
        | E::IpcTruncated
        | E::IpcTrailingAfterEos
        | E::IpcUnsupportedFeature(_)
        | E::IpcFooterInvalid(_)
        | E::IpcSchemaInvalid(_)
        | E::IpcMetadataInvalid(_)
        | E::Arrow(_)
        | E::ArrowPanic(_)
        | E::Geometry(_)
        | E::MissingColumn(_)
        | E::ColumnNotNumeric { .. }
        | E::IntegerCoordinateTooLarge { .. }
        | E::OutputColumnExists(_)
        | E::WrongGeometryType { .. } => PlenoraError::DataMapping(error_testo(&error)),

        // --- Impossibili dal validatore IPC: difetto NOSTRO ----------------
        //
        // Parametri di operazione, kernel, join, backend nascono eseguendo, e
        // qui si traducono solo errori del lettore di confine: `DataMapping`
        // attribuirebbe ai dati una causa interna. Il testo e' statico e
        // nomina il punto, mai il dato (`errori-e-limiti.md`).
        E::MissingParameter { .. }
        | E::UnexpectedParameter { .. }
        | E::InvalidParameter { .. }
        | E::BackendUnavailable { .. }
        | E::Kernel(_)
        | E::OutputRowsExceeded { .. }
        | E::Topology(_)
        | E::Construction(_)
        | E::Advanced(_)
        | E::PairRowCountMismatch { .. }
        | E::SideLengthMismatch { .. }
        | E::Extended(_)
        | E::ExtendedAlgorithm(_)
        | E::Predicate(_)
        | E::Analysis(_)
        | E::SpatialJoin(_) => PlenoraError::Internal(IMPOSSIBILE_AL_CONFINE.to_owned()),

        #[cfg(feature = "geos-backend")]
        E::MakeValid(_) => PlenoraError::Internal(IMPOSSIBILE_AL_CONFINE.to_owned()),
        #[cfg(feature = "proj-backend")]
        E::Reproject(_) => PlenoraError::Internal(IMPOSSIBILE_AL_CONFINE.to_owned()),

        // --- Diagnostica di riga: si classifica la causa e si RIATTACCA ----
        //
        // La categoria appartiene alla causa; scartare `diagnostics`
        // perderebbe in silenzio il payload piu' specifico.
        E::RowDiagnostics {
            source,
            diagnostics,
        } => traduci_errore_di_lettura(*source).with_row_diagnostics(*diagnostics),
    }
}

/// Testo di un errore del trasporto, gia' sanificato all'origine.
fn error_testo(error: &ArrowTransportError) -> String {
    error.to_string()
}

/// `true` se il file inizia con il magic dell'Arrow IPC **file format**
/// (`ARROW1`); altrimenti e' trattato come stream format.
///
/// # Errors
///
/// `PlenoraError::Io` taggato [`ErrorPhase::Read`] se il file non si apre o
/// non si legge.
pub fn sniff_format(path: &Path) -> Result<IpcFormat> {
    let sniffed = (|| -> Result<IpcFormat> {
        const MAGIC: &[u8; 6] = b"ARROW1";
        let mut file = File::open(path)?;
        let mut buffer = [0_u8; 6];
        let mut read = 0_usize;
        while read < buffer.len() {
            let count = file.read(&mut buffer[read..])?;
            if count == 0 {
                break;
            }
            read += count;
        }
        if read == MAGIC.len() && &buffer == MAGIC {
            Ok(IpcFormat::File)
        } else {
            Ok(IpcFormat::Stream)
        }
    })();
    sniffed.map_err(|error| error.with_phase(ErrorPhase::Read))
}

/// Apre il file, ne pre-valida il framing secondo il formato e restituisce
/// l'handle riportato all'inizio, pronto per arrow.
fn validated_handle(path: &Path, format: IpcFormat, limits: &IpcLimits) -> Result<File> {
    let file =
        File::open(path).map_err(|error| PlenoraError::Io(error).with_phase(ErrorPhase::Read))?;
    let total_len = file
        .metadata()
        .map_err(|error| PlenoraError::Io(error).with_phase(ErrorPhase::Read))?
        .len();
    let mut source = SeekSource::new(file, total_len);
    match format {
        IpcFormat::File => validate_ipc_file_framing(&mut source, limits),
        IpcFormat::Stream => validate_ipc_stream_framing(&mut source, limits),
    }
    .map_err(read_error)?;
    source.rewind().map_err(read_error)
}

/// Esegue `build` dentro la barriera anti-panico, convertendo un eventuale
/// panico di arrow in errore.
fn guarded<T, F: FnOnce() -> Result<T>>(build: F) -> Result<T> {
    match barriera_di_dipendenza(AssertUnwindSafe(build)) {
        Ok(esito) => esito,
        Err(panico) => Err(PlenoraError::DataMapping(format!(
            "arrow-ipc in panico sullo schema della sorgente: {}",
            descrivi_panico(&panico)
        ))
        .with_phase(ErrorPhase::Read)),
    }
}

/// Lettore lazy di confine: itera i `RecordBatch` della sorgente tenendo ogni
/// chiamata ad arrow dentro la barriera anti-panico.
///
/// Dopo un panico l'iteratore si spegne (`None` da li' in poi): lo stato
/// interno del lettore arrow non e' piu' affidabile e continuare a tirarlo
/// significherebbe leggere da una struttura lasciata a meta'.
pub struct BoundaryBatches {
    reader: BoundaryReader,
    poisoned: bool,
}

impl std::fmt::Debug for BoundaryBatches {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // I lettori arrow non sono `Debug`: si espone la forma, non lo stato.
        formatter
            .debug_struct("BoundaryBatches")
            .field(
                "format",
                match &self.reader {
                    BoundaryReader::File(_) => &"file",
                    BoundaryReader::Stream(_) => &"stream",
                },
            )
            .field("poisoned", &self.poisoned)
            .finish()
    }
}

enum BoundaryReader {
    File(Box<FileReader<File>>),
    Stream(Box<StreamReader<File>>),
}

impl Iterator for BoundaryBatches {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.poisoned {
            return None;
        }
        let reader = &mut self.reader;
        let esito = barriera_di_dipendenza(AssertUnwindSafe(|| match reader {
            BoundaryReader::File(reader) => reader.next(),
            BoundaryReader::Stream(reader) => reader.next(),
        }));
        match esito {
            Ok(item) => item.map(|batch| {
                batch.map_err(|error| PlenoraError::from(error).with_phase(ErrorPhase::Read))
            }),
            Err(panico) => {
                self.poisoned = true;
                Some(Err(PlenoraError::DataMapping(format!(
                    "arrow-ipc in panico leggendo un batch della sorgente: {}",
                    descrivi_panico(&panico)
                ))
                .with_phase(ErrorPhase::Read)))
            }
        }
    }
}

/// Apre un ingresso IPC di formato noto: framing pre-validato, schema letto
/// dentro la barriera anti-panico, batch lazy.
///
/// # Errors
///
/// Tutti taggati [`ErrorPhase::Read`], distinti per categoria:
///
/// | categoria | quando |
/// |---|---|
/// | `PlenoraError::Io` | il file non si apre, non si legge o non si riavvolge. La causa `std::io::Error` e' conservata |
/// | `PlenoraError::ResourceLimit` | l'ingresso supera un tetto del confine: byte, messaggi, batch, righe, colonne, complessita' dello schema, custom metadata |
/// | `PlenoraError::DataMapping` | il framing e' malformato, lo schema o il contratto non sono quelli attesi, oppure arrow va in panico sullo schema |
pub fn open_with_format(
    path: &Path,
    format: IpcFormat,
    limits: &IpcLimits,
) -> Result<(SchemaRef, BoundaryBatches)> {
    let file = validated_handle(path, format, limits)?;
    guarded(move || match format {
        IpcFormat::File => {
            let reader = FileReader::try_new(file, None)
                .map_err(|error| PlenoraError::from(error).with_phase(ErrorPhase::Read))?;
            let schema = reader.schema();
            Ok((
                schema,
                BoundaryBatches {
                    reader: BoundaryReader::File(Box::new(reader)),
                    poisoned: false,
                },
            ))
        }
        IpcFormat::Stream => {
            let reader = StreamReader::try_new(file, None)
                .map_err(|error| PlenoraError::from(error).with_phase(ErrorPhase::Read))?;
            let schema = reader.schema();
            Ok((
                schema,
                BoundaryBatches {
                    reader: BoundaryReader::Stream(Box::new(reader)),
                    poisoned: false,
                },
            ))
        }
    })
}

/// Un artefatto Arrow IPC **file format** il cui framing e' stato convalidato,
/// ancora aperto sullo stesso handle.
///
/// Campo privato e nessun costruttore: l'unica funzione che ne rende uno e'
/// [`convalida_artefatto`] (con le sue varianti), quindi digest e consegna ad
/// arrow riferiscono per costruzione al file convalidato. Difende dalla
/// sostituzione del percorso, non dalla mutazione in place dei byte.
// Senza `cfg`: lo usano `pubblicazione::risolvi_commit` e il verificatore del
// profilo isolato.
pub(crate) struct ArtefattoConvalidato {
    // Un campo solo, e non anche i byte totali: quelli la sorgente li conosce
    // gia' — glieli si passa costruendola — e tenerne una seconda copia qui
    // vorrebbe dire due numeri che possono divergere per la stessa grandezza.
    // Chi li vuole li chiede a lei.
    sorgente: crate::geo_transport::ipc::SeekSource<File>,
}

impl ArtefattoConvalidato {
    /// Un secondo handle sullo **stesso** file gia' aperto.
    ///
    /// `try_clone` duplica il descrittore, non il nome: riaprire per percorso
    /// potrebbe trovare un file diverso. Serve perche' [`Self::in_batches`]
    /// consuma l'artefatto.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::Io`] se il descrittore non si duplica.
    pub(crate) fn duplica(&self) -> Result<Self> {
        let copia = self
            .sorgente
            .lettore()
            .try_clone()
            .map_err(|errore| PlenoraError::Io(errore).with_phase(ErrorPhase::Read))?;
        Ok(Self {
            sorgente: crate::geo_transport::ipc::SeekSource::new(copia, self.byte_totali()),
        })
    }

    /// Quanti byte ha il file **adesso**, chiesti al descrittore aperto.
    ///
    /// [`Self::byte_totali`] e' la misura dell'apertura: confrontarla con
    /// un'altra derivata dalla stessa apertura non puo' fallire. Questa scopre
    /// la mutazione in place avvenuta dopo, senza risolvere il percorso.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::Io`] se il descrittore non si lascia interrogare.
    pub(crate) fn misura_ora(&self) -> Result<u64> {
        self.sorgente
            .lettore()
            .metadata()
            .map(|metadati| metadati.len())
            .map_err(|errore| PlenoraError::Io(errore).with_phase(ErrorPhase::Read))
    }

    /// I byte del file, misurati all'apertura.
    pub(crate) fn byte_totali(&self) -> u64 {
        use crate::geo_transport::ipc::IpcSource as _;
        self.sorgente.total_len()
    }

    /// Legge una finestra per offset, senza spostare cio' che arrow leggera'.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Io` taggato [`ErrorPhase::Read`].
    pub(crate) fn leggi_a(&mut self, offset: u64, len: usize, out: &mut Vec<u8>) -> Result<()> {
        use crate::geo_transport::ipc::IpcSource as _;
        self.sorgente.read_at(offset, len, out).map_err(read_error)
    }

    /// Riavvolge e consegna i batch ad arrow, dentro la barriera anti-panico.
    ///
    /// Il riavvolgimento sta qui, non a carico del chiamante.
    ///
    /// # Errors
    ///
    /// Come [`open_with_format`], meno gli errori di apertura.
    pub(crate) fn in_batches(self) -> Result<(SchemaRef, BoundaryBatches)> {
        let file = self.sorgente.rewind().map_err(read_error)?;
        guarded(move || {
            let reader = FileReader::try_new(file, None)
                .map_err(|error| PlenoraError::from(error).with_phase(ErrorPhase::Read))?;
            let schema = reader.schema();
            Ok((
                schema,
                BoundaryBatches {
                    reader: BoundaryReader::File(Box::new(reader)),
                    poisoned: false,
                },
            ))
        })
    }
}

/// Apre un artefatto, ne convalida il framing e ne estrae la chiave richiesta
/// dai custom metadata del footer, **in una traversata sola**.
///
/// E' il costruttore di [`ArtefattoConvalidato`]. A differenza di [`open`],
/// che riapre per percorso a ogni passo, tiene framing, token, digest e
/// consegna ad arrow sullo stesso handle.
///
/// # Errors
///
/// Gli errori del confine, taggati [`ErrorPhase::Read`]: `Io` sull'apertura,
/// `ResourceLimit` sui tetti — compreso quello cumulativo sui dizionari —
/// `DataMapping` sul framing malformato.
// La chiama il verificatore, che vuole un errore del progetto invece della
// causa. Chi osserva una destinazione usa la forma con la causa.
pub(crate) fn convalida_artefatto(
    percorso: &Path,
    limits: &IpcLimits,
    chiave: &str,
) -> Result<(Option<String>, ArtefattoConvalidato)> {
    convalida_artefatto_con_causa(percorso, limits, chiave).map_err(|causa| match causa {
        CausaDiApertura::Io(errore) => PlenoraError::Io(errore).with_phase(ErrorPhase::Read),
        CausaDiApertura::Confine(causa) => read_error(causa),
    })
}

/// Perche' l'apertura convalidata non e' riuscita, **prima** che qualcuno la
/// appiattisca in un [`PlenoraError`].
///
/// Chi osserva una destinazione deve distinguere sigillo assente, sigillo non
/// corrispondente, tetto superato e footer rifiutato, che `read_error`
/// riunisce in una categoria sola.
pub(crate) enum CausaDiApertura {
    /// Il file non si e' aperto o non si e' lasciato misurare.
    Io(std::io::Error),
    /// Il confine ostile ha rifiutato il contenuto.
    Confine(crate::geo_transport::error::ArrowTransportError),
}

/// Come [`convalida_artefatto`], ma conserva la causa invece di tradurla.
///
/// Rende l'artefatto gia' convalidato, cosi' chi legge oltre il footer non
/// riapre il percorso.
///
/// # Errors
///
/// [`CausaDiApertura`], che il chiamante traduce come gli serve.
pub(crate) fn convalida_artefatto_con_causa(
    percorso: &Path,
    limits: &IpcLimits,
    chiave: &str,
) -> std::result::Result<(Option<String>, ArtefattoConvalidato), CausaDiApertura> {
    let file = File::open(percorso).map_err(CausaDiApertura::Io)?;
    convalida_handle_con_causa(file, limits, chiave)
}

/// Come [`convalida_artefatto_con_causa`], ma da un handle **gia' aperto**
/// invece che da un percorso.
///
/// Il verificatore (`isolamento.md#2-quater-topologia-chi-osserva-chi`) riceve
/// l'artefatto come descrittore aperto in sola lettura, mai come percorso
/// ([`NG-9`](../../../docs/isolamento.md)). Tutto cio' che segue l'apertura e'
/// condiviso con [`convalida_artefatto_con_causa`].
///
/// # Errors
///
/// [`CausaDiApertura`], come [`convalida_artefatto_con_causa`] meno gli
/// errori di apertura per percorso.
pub(crate) fn convalida_handle_con_causa(
    file: File,
    limits: &IpcLimits,
    chiave: &str,
) -> std::result::Result<(Option<String>, ArtefattoConvalidato), CausaDiApertura> {
    use crate::geo_transport::ipc::{valida_file_ed_estrai, SeekSource};

    let byte_totali = file.metadata().map_err(CausaDiApertura::Io)?.len();
    let mut sorgente = SeekSource::new(file, byte_totali);
    let trovato = valida_file_ed_estrai(&mut sorgente, limits, Some(chiave))
        .map_err(CausaDiApertura::Confine)?;
    Ok((trovato, ArtefattoConvalidato { sorgente }))
}

/// Come [`convalida_artefatto`], ma da un handle gia' aperto: la forma
/// pubblica (nel crate) di [`convalida_handle_con_causa`], con la causa gia'
/// tradotta in [`PlenoraError`].
///
/// # Errors
///
/// Gli errori del confine, taggati [`ErrorPhase::Read`]: `Io` sulla misura
/// dell'handle, `ResourceLimit` sui tetti, `DataMapping` sul framing
/// malformato — esattamente come [`convalida_artefatto`].
pub(crate) fn convalida_handle_artefatto(
    file: File,
    limits: &IpcLimits,
    chiave: &str,
) -> Result<(Option<String>, ArtefattoConvalidato)> {
    convalida_handle_con_causa(file, limits, chiave).map_err(|causa| match causa {
        CausaDiApertura::Io(errore) => PlenoraError::Io(errore).with_phase(ErrorPhase::Read),
        CausaDiApertura::Confine(causa) => read_error(causa),
    })
}

/// Avvolge un handle **gia' accertato altrove** in un [`ArtefattoConvalidato`],
/// senza rifare la traversata del framing.
///
/// Nella topologia a due domini la verifica vera avviene nel dominio del
/// verificatore; il coordinatore non riparsa Arrow (§2-ter). Non e' una porta
/// verso la pubblicazione di un file non verificato: `ArtefattoVerificato`
/// nasce solo dopo la barriera di successo col verificatore, e il passo 9
/// rimisura il file e ricalcola il digest sui byte copiati. `pub(crate)`, con
/// il solo chiamante di produzione a due domini.
pub(crate) const fn artefatto_gia_accertato(file: File, byte_totali: u64) -> ArtefattoConvalidato {
    ArtefattoConvalidato {
        sorgente: crate::geo_transport::ipc::SeekSource::new(file, byte_totali),
    }
}

/// Apre un ingresso IPC riconoscendone il formato dal magic.
///
/// # Errors
///
/// Come [`open_with_format`].
pub fn open(path: &Path, limits: &IpcLimits) -> Result<(SchemaRef, BoundaryBatches)> {
    let format = sniff_format(path)?;
    open_with_format(path, format, limits)
}

/// Schema dell'header IPC di un ingresso (file o stream format): framing
/// pre-validato, schema letto dentro la barriera, e **nessun dato decodificato**
/// — ne' righe ne' dizionari.
///
/// Il file format non passa da `FileReader`, che decodifica tutti i
/// dizionari prima di rendere lo schema: sul profilo isolato sarebbero
/// allocazioni del coordinatore che precedono l'autorizzazione (`F4-5`).
/// [`schema_dal_footer`] lo ricava dal footer gia' convalidato. Lo stream
/// format legge il solo messaggio di schema.
///
/// # Errors
///
/// Come [`open_with_format`].
pub fn header_schema(path: &Path, limits: &IpcLimits) -> Result<SchemaRef> {
    match sniff_format(path)? {
        IpcFormat::File => {
            let file = File::open(path)
                .map_err(|error| PlenoraError::Io(error).with_phase(ErrorPhase::Read))?;
            let total_len = file
                .metadata()
                .map_err(|error| PlenoraError::Io(error).with_phase(ErrorPhase::Read))?
                .len();
            let mut source = SeekSource::new(file, total_len);
            let footer = valida_file_e_rendi_footer(&mut source, limits).map_err(read_error)?;
            guarded(|| schema_dal_footer(&footer))
        }
        IpcFormat::Stream => {
            let (schema, _) = open_with_format(path, IpcFormat::Stream, limits)?;
            Ok(schema)
        }
    }
}

/// Lo schema dai byte di un footer **gia' convalidato**.
///
/// Ripete, nello stesso ordine, i controlli di `FileReaderBuilder::build` di
/// `arrow-ipc` 59.2.0 che precedono i dizionari; dove arrow fa `unwrap()` sullo
/// schema assente, qui c'e' un errore. `fb_to_schema` gira dentro la barriera
/// di chi chiama. I messaggi non riportano byte del footer.
fn schema_dal_footer(footer: &[u8]) -> Result<SchemaRef> {
    let illeggibile = |motivo: &str| {
        PlenoraError::DataMapping(format!("footer IPC non utilizzabile: {motivo}"))
            .with_phase(ErrorPhase::Read)
    };
    let footer = plenora_core::arrow::ipc::root_as_footer(footer)
        .map_err(|_| illeggibile("il FlatBuffer non supera la verifica"))?;
    // Assente e vuoto non sono la stessa cosa: arrow rifiuta il primo e
    // accetta il secondo, e cosi' qui.
    if footer.recordBatches().is_none() {
        return Err(illeggibile("il vettore dei record batch manca"));
    }
    let schema = footer
        .schema()
        .ok_or_else(|| illeggibile("lo schema manca"))?;
    if !schema.endianness().equals_to_target_endianness() {
        return Err(illeggibile(
            "l'endianness della sorgente non e' quella di questo sistema",
        ));
    }
    Ok(std::sync::Arc::new(
        plenora_core::arrow::ipc::convert::fb_to_schema(schema),
    ))
}

/// Limiti del confine derivati dai limiti effettivi del piano.
///
/// Il tetto sul body e' il piu' stretto fra `max_batch_bytes`,
/// `max_governed_memory_bytes` e `max_payload_bytes`: il governor interviene
/// solo dopo la materializzazione, cioe' dopo l'allocazione da impedire.
/// `max_batches` limita i record batch, non i messaggi: schema e
/// `DictionaryBatch` hanno un tetto proprio.
#[must_use]
pub fn limits_from_plan(
    limits: &plenora_core::limits::Limits,
    max_batch_bytes: usize,
) -> IpcLimits {
    let default = IpcLimits::default();
    let tetto = u64::try_from(max_batch_bytes)
        .unwrap_or(u64::MAX)
        .min(limits.max_governed_memory_bytes)
        .min(limits.max_payload_bytes);
    IpcLimits {
        // Anche i metadati sono un'allocazione, precedente a qualunque batch:
        // il tetto e' il piu' stretto fra il default e il budget effettivo.
        max_metadata_bytes: usize::try_from(
            u64::try_from(default.max_metadata_bytes)
                .unwrap_or(u64::MAX)
                .min(tetto),
        )
        .unwrap_or(default.max_metadata_bytes),
        max_body_bytes: tetto,
        max_record_batches: usize::try_from(limits.max_batches).unwrap_or(usize::MAX),
        max_messages: default.max_messages,
        // Anche i dizionari sono un'allocazione dentro il budget, e a
        // differenza dei batch restano vivi tutti insieme: lasciarli al
        // massimale ammetterebbe 64 MiB di dizionari sotto un budget di 1 MiB.
        max_retained_dictionary_body_bytes: default.max_retained_dictionary_body_bytes.min(tetto),
        ..IpcLimits::default()
    }
}

/// Limiti del confine per i percorsi che hanno un solo budget di memoria e
/// nessun piano DAG alle spalle (piani legacy, `schema_version <= 3`).
///
/// Con `IpcLimits::default()` il confine ammetterebbe 64 MiB di body e 16
/// MiB di metadati indipendentemente da `max_governed_memory_bytes`, cioe'
/// non vincolerebbe il budget dichiarato dal piano. Qui il budget e' l'unico
/// dato disponibile e diventa il tetto di entrambe le allocazioni.
#[must_use]
pub fn limits_from_memory_budget(max_governed_memory_bytes: usize) -> IpcLimits {
    let default = IpcLimits::default();
    let tetto = u64::try_from(max_governed_memory_bytes).unwrap_or(u64::MAX);
    IpcLimits {
        max_metadata_bytes: usize::try_from(
            u64::try_from(default.max_metadata_bytes)
                .unwrap_or(u64::MAX)
                .min(tetto),
        )
        .unwrap_or(default.max_metadata_bytes),
        max_body_bytes: default.max_body_bytes.min(tetto),
        max_record_batches: default.max_record_batches,
        max_messages: default.max_messages,
        max_retained_dictionary_body_bytes: default.max_retained_dictionary_body_bytes.min(tetto),
        ..IpcLimits::default()
    }
}

#[cfg(test)]
mod tests;
