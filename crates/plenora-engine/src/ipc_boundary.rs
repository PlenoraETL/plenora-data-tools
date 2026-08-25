//! Confine unico di lettura Arrow IPC per gli ingressi non fidati.
//!
//! # Perche' esiste
//!
//! `arrow-ipc` 59.2.0 ha due esposizioni note su input ostile, entrambe
//! raggiungibili dal primo byte letto:
//!
//! 1. **panico** — `convert::fb_to_schema` scarta con `unwrap()` il campo
//!    opzionale `fields` e contiene una ventina fra `panic!` e
//!    `unimplemented!` sui valori di enum che non riconosce. Ogni lettore la
//!    chiama. Le API che la avvolgono si chiamano `try_*` ma sono fallibili
//!    solo sul parsing esterno: appena ottengono lo schema fanno
//!    `.map(fb_to_schema)`. Segnalato a monte: apache/arrow-rs#10575.
//! 2. **allocazione ostile** — i metadati flatbuffer dichiarano conteggi di
//!    vettori e stringhe che arrow alloca con `Vec::with_capacity` senza un
//!    tetto proprio. `catch_unwind` non protegge da un OOM: l'unica difesa e'
//!    pre-validare il framing e i limiti PRIMA che arrow veda i byte.
//!
//! Il trasporto geo aveva gia' entrambe le difese sul proprio payload in
//! memoria ([`crate::geo_transport::ipc::decode_ipc`]), ma le normali API di
//! ingresso — `Input::read_ipc_file` / `read_ipc_stream` dell'executor e la
//! discovery dello schema della CLI — aprivano `FileReader` e `StreamReader`
//! direttamente, scavalcando la barriera. Questo modulo e' l'unico lettore di
//! confine: tutti gli ingressi file/stream/CLI passano di qui.
//!
//! # Cosa NON copre
//!
//! I file di spill e i temp store dell'esecuzione sono prodotti da noi, nella
//! directory isolata dell'`execution_id`, e non sono ingressi non fidati: non
//! passano da questa barriera, che imporrebbe una scansione in piu' su ogni
//! round di merge senza aggiungere garanzie.
//!
//! Non copre nemmeno la MUTAZIONE IN PLACE dell'ingresso durante la lettura.
//! La pre-validazione e' un time-of-check e la lettura di arrow un
//! time-of-use: fra i due c'e' una finestra. Il confine la restringe usando
//! un solo handle aperto una volta ([`validated_handle`] restituisce il file
//! riavvolto, mai il percorso), il che esclude la sostituzione del path —
//! rename, symlink swap, ricreazione — ma non uno scrittore concorrente che
//! riscriva lo stesso inode. In quel caso resta attiva la barriera
//! anti-panico e decade il tetto sulle allocazioni, che vale solo sui byte
//! effettivamente controllati. La condizione e' dichiarata in errori-e-limiti.md#limiti-dichiarati
//! (`docs/errori-e-limiti.md`) con il requisito operativo corrispondente.

use std::fs::File;
use std::io::Read as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::reader::{FileReader, StreamReader};
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::{ErrorPhase, PlenoraError, Result};

use crate::geo_transport::error::ArrowTransportError;
use crate::geo_transport::ipc::{
    descrivi_panico, validate_ipc_file_framing, validate_ipc_stream_framing, SeekSource,
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
/// I tetti di RISORSA del confine — metadati, body, numero di messaggi e di
/// record batch, complessita' dello schema, dimensione dello stream — sono
/// limiti, non dati malformati: escono come [`PlenoraError::ResourceLimit`],
/// che e' la categoria su cui il chiamante decide di rilanciare con piu'
/// budget. Uscivano tutti come `data_mapping`, indistinguibili da un file
/// corrotto. Il framing malformato resta `data_mapping`: li' il file e'
/// davvero rotto.
///
/// La fase resta `Read` per entrambi: il tag del confine vince sulla
/// derivazione per variante, e questi errori nascono leggendo.
fn read_error(error: ArrowTransportError) -> PlenoraError {
    use ArrowTransportError as E;

    /// Testo di un errore che il validatore IPC non puo' produrre.
    ///
    /// Statico e senza dati: nomina il punto, non l'input.
    const IMPOSSIBILE_AL_CONFINE: &str =
        "errore di esecuzione riportato dal lettore di confine IPC";

    let errore = match error {
        // --- I/O: la causa si conserva, non si stampa ----------------------
        //
        // La versione precedente lo classificava `DataMapping`, cioe' «il file
        // e' rotto», e ne teneva solo il testo. Un disco che non risponde
        // durante `read_at` o `rewind` diventava cosi' un file corrotto, e
        // mandava chi legge a cercare un difetto nei dati. Consumare l'errore
        // invece di prenderlo a prestito e' cio' che permette di preservare
        // lo `std::io::Error` originale.
        E::Io(io) => PlenoraError::Io(io),

        // --- Limiti: il file c'e' ed e' piu' grande di quanto ammettiamo ---
        E::StreamTooLarge
        | E::TooManyRows(_)
        | E::TooManyColumns(_)
        | E::TooManyBatches(_)
        | E::CellTooLarge(_)
        | E::IpcMetadataTooLarge(_, _)
        | E::IpcBodyTooLarge { .. }
        | E::IpcTooManyMessages(_, _)
        | E::IpcSchemaTooComplex(_)
        | E::IpcTooManyRecordBatches(_, _)
        | E::IpcTooManyMetadataPairs(_, _)
        | E::IpcMetadataKeyTooLarge(_, _)
        | E::IpcMetadataValueTooLarge(_, _) => PlenoraError::ResourceLimit(error_testo(&error)),

        // --- Difetto nostro -------------------------------------------------
        E::Internal(motivo) => PlenoraError::Internal(motivo.to_owned()),

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
        // Parametri di operazione, kernel, join, backend: nascono ESEGUENDO, e
        // questa funzione traduce solo gli errori del lettore di confine.
        //
        // La stesura precedente le lasciava in `DataMapping` «per non cambiare
        // comportamento su un percorso che non dovrebbe esistere». Era la
        // stessa classe del difetto su `Io`, appena corretto: una causa
        // interna attribuita ai dati. Conservare il vecchio fallback non e'
        // compatibilita' — e' conservare una diagnosi falsa proprio dove il
        // codice dichiara che non puo' succedere.
        //
        // Il testo e' **statico**: non porta nulla dell'errore originale,
        // perche' `Internal` dice «difetto nostro» e il messaggio nomina il
        // punto, mai il dato (`errori-e-limiti.md`).
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
        // La categoria appartiene alla causa, non all'involucro. Ma la
        // stesura precedente ricorreva **scartando** `diagnostics`: la
        // categoria sopravviveva e il payload no, cioe' si perdeva
        // silenziosamente l'informazione piu' specifica che l'errore avesse.
        //
        // Riattaccarla costa una riga ed e' corretto comunque, che questo ramo
        // sia raggiungibile o no dal confine — e non obbliga a decidere una
        // raggiungibilita' che nessuno puo' osservare da qui.
        E::RowDiagnostics {
            source,
            diagnostics,
        } => {
            return read_error(*source)
                .with_row_diagnostics(*diagnostics)
                .with_phase(ErrorPhase::Read)
        }
    };
    errore.with_phase(ErrorPhase::Read)
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
    match catch_unwind(AssertUnwindSafe(build)) {
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
        let esito = catch_unwind(AssertUnwindSafe(|| match reader {
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
/// Tutti taggati [`ErrorPhase::Read`], e distinti per categoria — la
/// distinzione conta, perche' dice a chi legge dove guardare:
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

/// Apre un ingresso IPC riconoscendone il formato dal magic.
///
/// # Errors
///
/// Come [`open_with_format`].
pub fn open(path: &Path, limits: &IpcLimits) -> Result<(SchemaRef, BoundaryBatches)> {
    let format = sniff_format(path)?;
    open_with_format(path, format, limits)
}

/// Schema dell'header IPC di un ingresso (file o stream format): nessuna riga
/// di dati letta, ma framing pre-validato e schema letto dentro la barriera.
///
/// # Errors
///
/// Come [`open_with_format`].
pub fn header_schema(path: &Path, limits: &IpcLimits) -> Result<SchemaRef> {
    let (schema, _) = open(path, limits)?;
    Ok(schema)
}

/// Limiti del confine derivati dai limiti effettivi del piano.
///
/// Il tetto sul body e' il PIU' STRETTO fra i limiti che il batch dovra'
/// rispettare comunque: `max_batch_bytes` (con cui l'executor misura il batch
/// risultante), `max_governed_memory_bytes` (il budget del governor) e
/// `max_payload_bytes`. Derivarlo dal solo `max_batch_bytes` lasciava arrow
/// allocare fino al tetto di default del body (64 MiB) anche con un budget di
/// memoria di 1 MiB, e il
/// governor interveniva solo dopo la materializzazione — cioe' dopo
/// l'allocazione che il tetto deve impedire.
///
/// `max_batches` e' il limite semantico dei RECORD BATCH, non dei messaggi:
/// lo schema e i `DictionaryBatch` hanno il proprio tetto, altrimenti un
/// piano con `max_batches = 1` rifiuterebbe qualunque stream non vuoto.
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
        // Anche i METADATI sono un'allocazione, e arrow li alloca prima di
        // qualunque batch: lasciarli al default significava permettere 16 MiB
        // di metadati sotto un budget di memoria di 1 MiB — cioe' sforare il
        // budget prima ancora di leggere una riga. Il tetto e' il piu'
        // stretto fra il default e il budget effettivo.
        max_metadata_bytes: usize::try_from(
            u64::try_from(default.max_metadata_bytes)
                .unwrap_or(u64::MAX)
                .min(tetto),
        )
        .unwrap_or(default.max_metadata_bytes),
        max_body_bytes: tetto,
        max_record_batches: usize::try_from(limits.max_batches).unwrap_or(usize::MAX),
        max_messages: default.max_messages,
    }
}

/// Limiti del confine per i percorsi che hanno un solo budget di memoria e
/// nessun piano DAG alle spalle (piani legacy, `schema_version <= 3`).
///
/// Il percorso legacy usava `IpcLimits::default()`: 64 MiB di body e 16 MiB di
/// metadati indipendentemente da `max_governed_memory_bytes`, cioe' un confine che non
/// vincolava il budget dichiarato dal piano. Qui il budget e' l'unico dato
/// disponibile e diventa il tetto di entrambe le allocazioni.
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
    }
}

#[cfg(test)]
mod tests;
