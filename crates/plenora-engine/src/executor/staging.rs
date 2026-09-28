//! Staging degli input: materializzazione su disco quando la memoria non basta.
//!
//! Un input che non entra nel budget e' messo da parte su file e riletto;
//! `CountingFile` conta i byte scritti, cosi' il limite di disco e' un tetto
//! vero. `atomic_input_validation_stream` valida TUTTI i batch prima di
//! lasciarne passare uno, perche' validare in streaming pubblicherebbe righe
//! di un input che si scopre invalido piu' avanti.

use std::path::Path;
use std::rc::Rc;

use plenora_core::arrow::array::{RecordBatch, UInt32Array};
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::StreamWriter;
use plenora_core::arrow::select::take::take;
use plenora_core::arrow::ArrowError;
use plenora_core::contract::BatchSequence;
use plenora_core::{PlenoraError, Result};

use crate::governor::GovernedBatch;

use super::diagnostics::{
    attach_partial_row_diagnostics, complete_row_diagnostic_error, merge_row_diagnostics,
};
use super::input::BatchStream;
use super::state::ExecState;

/// Capacita' del buffer di scrittura dello staging.
///
/// Il writer IPC emette ogni messaggio in molti pezzi piccoli (prefisso,
/// metadati, padding, un buffer per colonna): senza buffer ognuno e' una
/// syscall. 64 KiB come lo spill (`SPILL_IO_BUFFER_BYTES` in
/// `plenora-kernels-table`): fissi, uno per staging aperto, fuori dalla
/// contabilita' del governor come il resto del writer IPC
/// (errori-e-limiti.md#memoria-governata). Il replay resta senza buffer: vedi
/// [`apri_replay`].
const STAGING_IO_BUFFER_BYTES: usize = 64 * 1024;

/// Writer con conteggio dei byte e quota dichiarata (`max_temp_bytes` del
/// piano): superata la quota la scrittura fallisce con errore esplicito,
/// mai silenzioso.
///
/// Il conteggio sta **sopra** il buffer: la quota si decide su ogni
/// scrittura logica del writer IPC, allo stesso byte del percorso senza
/// buffer. Gli errori del file arrivano invece allo svuotamento, e l'ordine
/// fra i due si conserva cosi':
///
/// - prima di rifiutare per quota si svuota: i pezzi precedenti, senza
///   buffer, sarebbero gia' stati scritti, e un loro errore del file sarebbe
///   venuto per primo ([`CountingFile::rifiuta`]);
/// - un errore di codifica a meta' messaggio segue la stessa regola in
///   [`stage_one_batch`] ([`prima_il_file`]);
/// - lo svuotamento esplicito dopo l'intestazione e dopo ogni batch, e quello
///   di `finish`, fanno emergere l'errore del file nella stessa chiamata in
///   cui emergeva senza buffer.
pub(super) struct CountingFile {
    /// `None` solo dentro il `Drop`.
    file: Option<std::io::BufWriter<std::fs::File>>,
    written: u64,
    max_bytes: u64,
}

impl CountingFile {
    fn create(path: &Path, max_bytes: u64) -> Result<Self> {
        let file = std::fs::File::create(path).map_err(PlenoraError::Io)?;
        Ok(Self {
            file: Some(std::io::BufWriter::with_capacity(
                STAGING_IO_BUFFER_BYTES,
                file,
            )),
            written: 0,
            max_bytes,
        })
    }

    fn buffer(&mut self) -> std::io::Result<&mut std::io::BufWriter<std::fs::File>> {
        self.file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("staging IPC gia' chiuso"))
    }

    /// Il rifiuto di quota, dopo aver svuotato cio' che lo precede.
    ///
    /// Senza buffer i pezzi gia' accettati sarebbero nel file: se scriverli
    /// fallisce, quell'errore viene prima della quota e si rende lui.
    fn rifiuta(&mut self, messaggio: &'static str) -> std::io::Error {
        match self.buffer().and_then(std::io::Write::flush) {
            Err(del_file) => del_file,
            Ok(()) => std::io::Error::new(std::io::ErrorKind::QuotaExceeded, messaggio),
        }
    }
}

/// Nessuna scrittura implicita alla distruzione.
///
/// Il `Drop` di `BufWriter` svuoterebbe il buffer ignorandone l'errore. Qui
/// byte ancora nel buffer esistono solo se lo staging e' gia' fallito: il
/// percorso riuscito svuota a ogni batch e in `finish`, e ne propaga l'errore.
/// Quei byte appartengono a un file che la sua `TempDir` cancella, e si
/// scartano senza scriverli: nessun I/O il cui esito nessuno legge.
impl Drop for CountingFile {
    fn drop(&mut self) {
        if let Some(buffer) = self.file.take() {
            let (_file, _scartati) = buffer.into_parts();
        }
    }
}

impl std::io::Write for CountingFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let Some(written) = self.written.checked_add(buf.len() as u64) else {
            return Err(self.rifiuta("overflow conteggio staging IPC"));
        };
        if written > self.max_bytes {
            return Err(self.rifiuta("staging IPC oltre max_temp_bytes"));
        }
        let n = self.buffer()?.write(buf)?;
        self.written = self.written.checked_add(n as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "overflow conteggio staging IPC",
            )
        })?;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.buffer()?.flush()
    }
}

/// Metadati per-batch dello staging IPC: byte da ri-riservare al replay
/// (gli stessi della riserva originale, rilasciata allo staging) e
/// sequenza logica architettura.md#determinismo catturata allo staging e ripubblicata
/// invariata al replay.
pub(super) struct StagedBatchMeta {
    bytes: u64,
    sequence: Option<BatchSequence>,
}

/// Stato del replay: lettore IPC sul file di staging e metadati per-batch
/// (byte da ri-riservare + sequenza logica da ripubblicare).
pub(super) struct StagedReplay {
    pub(super) reader: StreamReader<std::fs::File>,
    pub(super) staged: std::collections::VecDeque<StagedBatchMeta>,
    // La directory temporanea vive fino alla fine del replay.
    pub(super) _dir: tempfile::TempDir,
}

/// Replay di UN batch dallo staging IPC: compattazione right-sized, lease
/// ri-riservato per batch (memoria bounded) e sequenza logica ripubblicata
/// invariata. Condiviso dal gate input WKB e dallo staging degli output
/// accettati dei segmenti row-diagnostics: nessuna logica duplicata.
pub(super) fn replay_staged_batch(
    state: &ExecState,
    replay: &mut StagedReplay,
    owner: &str,
) -> Option<Result<GovernedBatch>> {
    match replay.reader.next() {
        Some(Ok(batch)) => {
            let Some(meta) = replay.staged.pop_front() else {
                return Some(Err(PlenoraError::Internal(
                    "replay staging IPC: conteggio byte incoerente".into(),
                )));
            };
            // Compattazione: la decodifica IPC condivide un'unica
            // allocazione corpo tra le colonne e ogni buffer la conta
            // interamente (lease e confini di kernel gonfiati ~3x).
            // `take` copia ogni colonna in buffer right-sized: una
            // copia per batch, memoria bounded.
            let batch = match compact_staged_batch(&batch) {
                Ok(compacted) => compacted,
                Err(error) => return Some(Err(error)),
            };
            match state.governor.reserve(meta.bytes, owner) {
                Ok(lease) => Some(Ok(GovernedBatch::new(batch, Some(lease), meta.sequence))),
                Err(error) => Some(Err(error)),
            }
        }
        Some(Err(error)) => Some(Err(PlenoraError::Internal(format!(
            "replay staging IPC: {error}"
        )))),
        None => None,
    }
}

/// Copia un batch decodificato dallo staging in buffer right-sized (vedi
/// replay): `take` con tutti gli indici, per colonna.
///
/// # Errors
/// - `ResourceLimit`: righe oltre `u32::MAX` (gia' escluso dai limiti di
///   piano, difesa);
/// - `Schema`: errore Arrow nella `take` o nella ricostruzione.
pub(super) fn compact_staged_batch(batch: &RecordBatch) -> Result<RecordBatch> {
    let indices: UInt32Array = (0..u32::try_from(batch.num_rows())
        .map_err(|_| PlenoraError::ResourceLimit("batch staging oltre u32 righe".into()))?)
        .collect::<Vec<_>>()
        .into();
    let columns = batch
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &indices, None).map_err(PlenoraError::from))
        .collect::<Result<Vec<_>>>()?;
    plenora_core::batch_with_rows(batch.schema(), columns, batch.num_rows())
}

/// Esito della fase di staging dell'input gate: errore terminale (eventuale
/// assenza di batch staged -> stream vuoto) oppure replay dal file staged.
///
/// La validazione atomica dell'input geometrico (D8) mette su IPC i batch
/// accettati entro `max_temp_bytes` e li rilegge solo a validazione completa
/// senza rifiuti (R9.9): un rifiuto row-scoped produce il report completo e
/// zero accepted; un errore di I/O in replay e' una failure infrastrutturale,
/// non un rifiuto di righe. Staging e spill misurano ciascuno la propria
/// scrittura contro `max_temp_bytes`, quindi la somma su disco puo' superarla.
pub(super) enum StagingOutcome {
    Terminal(Option<PlenoraError>),
    Replay(StagedReplay),
    /// Coda ordinata degli accepted trattenuti in memoria, con i lease
    /// originali ancora vivi: consegnata direttamente, senza IPC ne' copie
    /// (architettura.md#memoria, staging memory-first). Prodotta SOLO dai segmenti row-diagnostics; il gate
    /// WKB dell'input resta su disco.
    Memoria(std::collections::VecDeque<GovernedBatch>),
}

/// Staging degli accepted di un segmento row-diagnostics: **prima in
/// memoria**, con passaggio definitivo su disco quando il budget non basta
/// piu' (architettura.md#memoria, staging memory-first).
///
/// La barriera R9.9 chiede solo che nessun accepted esca prima della fine
/// della scansione; trattenere i batch governati la soddisfa senza il giro
/// IPC su disco.
///
/// In una passata i lease vivi sono al piu' input e output, quindi il picco in
/// memoria e' `trattenuti + input_k + output_k`. Si resta in memoria solo se
/// `trattenuti + input_k + max_batch_bytes <= budget`; il wrapper d'uscita
/// fa fallire in entrambe le modalita' un `output_k > max_batch_bytes`, quindi
/// un piano eseguibile non supera il budget e non diventa un `ResourceLimit`.
/// La soglia deriva dai limiti del piano e dai lease vivi, non dal tempo.
// `Disco` e' piu' grande di una `VecDeque`, ma esiste al massimo una volta per
// segmento e boxarla aggiungerebbe un'indirezione sul percorso caldo.
#[allow(clippy::large_enum_variant)]
pub(super) enum StagingAccepted {
    /// Batch trattenuti in ordine, lease vivi.
    ///
    /// Nessun totale locale dei byte: i lease sono gia' contati dal governor,
    /// che e' la fonte unica della soglia (vedi `accedibile_in_memoria`).
    /// Tenerne una copia qui sarebbe un duplicato — e un duplicato PARZIALE,
    /// perche' non vedrebbe le prenotazioni degli altri rami.
    Memoria(std::collections::VecDeque<GovernedBatch>),
    /// Modalita' disco: definitiva, non si torna indietro.
    Disco {
        writer: Option<StreamWriter<CountingFile>>,
        staging: Option<(tempfile::TempDir, std::path::PathBuf)>,
        meta: std::collections::VecDeque<StagedBatchMeta>,
    },
}

impl StagingAccepted {
    pub(super) const fn nuovo() -> Self {
        Self::Memoria(std::collections::VecDeque::new())
    }

    /// Modalita' disco definitiva, partendo da una coda gia' trattenuta.
    ///
    /// I batch sono travasati **nell'ordine** in cui sono stati prodotti e i
    /// lease rilasciati uno a uno: il picco durante il travaso non cresce
    /// mai sopra quello gia' concesso.
    pub(super) fn passa_a_disco(&mut self, state: &Rc<ExecState>, edge: &str) -> Result<()> {
        let Self::Memoria(coda) = self else {
            return Ok(());
        };
        let coda = std::mem::take(coda);
        let mut writer = None;
        let mut staging = None;
        let mut meta = std::collections::VecDeque::new();
        for governed in coda {
            stage_one_batch(
                &mut writer,
                &mut staging,
                state,
                "output",
                edge,
                &governed.batch,
            )?;
            meta.push_back(StagedBatchMeta {
                bytes: governed.accounted_bytes(),
                sequence: governed.seq.clone(),
            });
            // Rilascio esplicito: il lease muore qui, non a fine ciclo.
            drop(governed);
        }
        *self = Self::Disco {
            writer,
            staging,
            meta,
        };
        Ok(())
    }

    /// Accoglie un accepted, gia' governato.
    pub(super) fn accogli(
        &mut self,
        state: &Rc<ExecState>,
        edge: &str,
        governed: GovernedBatch,
    ) -> Result<()> {
        match self {
            Self::Memoria(coda) => {
                coda.push_back(governed);
                Ok(())
            }
            Self::Disco {
                writer,
                staging,
                meta,
            } => {
                stage_one_batch(writer, staging, state, "output", edge, &governed.batch)?;
                meta.push_back(StagedBatchMeta {
                    bytes: governed.accounted_bytes(),
                    sequence: governed.seq.clone(),
                });
                Ok(())
            }
        }
    }
}

/// Quale errore rendere quando una scrittura IPC fallisce con byte ancora nel
/// buffer.
///
/// Il writer IPC puo' fallire nella codifica **dopo** aver scritto parte del
/// batch (i dizionari, in `arrow-ipc` 59). Senza buffer quei pezzi sarebbero
/// gia' nel file, e un loro errore del file sarebbe venuto per primo: si
/// svuota adesso, e se lo svuotamento fallisce vince il suo errore, come
/// avrebbe vinto prima. Un errore che viene gia' dal file (`IoError`, quota
/// compresa: [`CountingFile::rifiuta`] svuota prima) e' per costruzione il
/// primo, e resta.
pub(super) fn prima_il_file(
    errore: ArrowError,
    svuota: impl FnOnce() -> std::result::Result<(), ArrowError>,
) -> ArrowError {
    if matches!(errore, ArrowError::IoError(..)) {
        return errore;
    }
    match svuota() {
        Err(del_file) => del_file,
        Ok(()) => errore,
    }
}

/// Scrive un batch nello staging IPC (inizializzando file e writer al primo
/// batch); la quota `max_temp_bytes` e' fatta rispettare da `CountingFile`.
/// `what` qualifica il contesto nei messaggi (`input` gate WKB, `output`
/// segmenti row-diagnostics): stessa logica, nessuna duplicazione.
pub(super) fn stage_one_batch(
    writer: &mut Option<StreamWriter<CountingFile>>,
    staging: &mut Option<(tempfile::TempDir, std::path::PathBuf)>,
    state: &Rc<ExecState>,
    what: &str,
    edge: &str,
    batch: &RecordBatch,
) -> Result<()> {
    if writer.is_none() {
        let dir = tempfile::Builder::new()
            .prefix(&format!("plenora-staging-{what}-"))
            .tempdir()
            .map_err(PlenoraError::Io)?;
        let path = dir.path().join("staged.arrow");
        let counting = CountingFile::create(&path, state.plan.limits().max_temp_bytes)?;
        // Lo svuotamento dopo l'intestazione riporta un errore di I/O dello
        // schema qui, con l'errore di `try_new`, e non al primo batch.
        let stream = StreamWriter::try_new(counting, &batch.schema())
            .and_then(|mut stream| stream.flush().map(|()| stream))
            .map_err(|error| PlenoraError::Internal(format!("staging {what}: {error}")))?;
        *writer = Some(stream);
        *staging = Some((dir, path));
    }
    let active = writer
        .as_mut()
        .ok_or_else(|| PlenoraError::Internal(format!("staging {what} non inizializzato")))?;
    // Scrittura e svuotamento del batch con lo stesso errore: un fallimento
    // del file emerge nel batch che lo ha causato, come senza buffer.
    active
        .write(batch)
        .map_err(|error| prima_il_file(error, || active.flush()))
        .and_then(|()| active.flush())
        .map_err(|error| {
            PlenoraError::InvalidPlan(format!(
                "staging {what} `{edge}` fallito oltre la quota o per I/O: {error}"
            ))
        })?;
    Ok(())
}

/// Drena lo stream di input validando ogni batch (gate WKB con diagnostica
/// completa) e facendo staging IPC bounded su `max_temp_bytes`; il lease del
/// governor e' rilasciato dopo lo staging di ciascun batch.
// Macchina a stati lineare: staging, diagnostica, chiusura e apertura replay
// restano nello stesso scope per rendere evidente il cleanup fail-closed.
#[allow(clippy::too_many_lines)]
pub(super) fn stage_input_batches(
    input: &mut BatchStream,
    state: &Rc<ExecState>,
    edge: &str,
) -> StagingOutcome {
    let mut diagnostics = None;
    let mut terminal_error = None;
    let mut staged_meta: std::collections::VecDeque<StagedBatchMeta> =
        std::collections::VecDeque::new();
    let mut next_sequence: u64 = 0;
    let mut staging: Option<(tempfile::TempDir, std::path::PathBuf)> = None;
    let mut writer: Option<StreamWriter<CountingFile>> = None;
    for item in input {
        match item {
            Ok(batch) => {
                if diagnostics.is_none() && terminal_error.is_none() {
                    let staged = stage_one_batch(
                        &mut writer,
                        &mut staging,
                        state,
                        "input",
                        edge,
                        &batch.batch,
                    );
                    if let Err(error) = staged {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            error,
                            &mut diagnostics,
                            "data_tools.input_staging_failed",
                        ));
                        break;
                    }
                    let sequence_number = next_sequence;
                    let Some(next) = next_sequence.checked_add(1) else {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            PlenoraError::Internal("overflow sequenza staging input".into()),
                            &mut diagnostics,
                            "data_tools.input_staging_failed",
                        ));
                        break;
                    };
                    next_sequence = next;
                    staged_meta.push_back(StagedBatchMeta {
                        bytes: batch.accounted_bytes(),
                        sequence: Some(BatchSequence {
                            source_node: edge.to_owned(),
                            input_partition: 0,
                            sequence_number,
                        }),
                    });
                    // Il lease del batch e' rilasciato con il drop:
                    // durante il drenaggio resta riservato al piu'
                    // un batch alla volta.
                }
            }
            Err(error) => {
                if let Some(report) = error.row_diagnostics().cloned() {
                    if let Err(error) = merge_row_diagnostics(&mut diagnostics, report, 0) {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            error,
                            &mut diagnostics,
                            "data_tools.diagnostic_merge_failed",
                        ));
                        break;
                    }
                } else {
                    terminal_error = Some(attach_partial_row_diagnostics(
                        error,
                        &mut diagnostics,
                        "data_tools.input_stream_interrupted",
                    ));
                    break;
                }
            }
        }
    }
    if terminal_error.is_none() {
        if let Some(active) = writer.as_mut() {
            if let Err(error) = active.finish() {
                terminal_error = Some(attach_partial_row_diagnostics(
                    PlenoraError::Internal(format!("chiusura staging input: {error}")),
                    &mut diagnostics,
                    "data_tools.input_staging_failed",
                ));
            }
        }
    }
    drop(writer);
    if terminal_error.is_none() {
        if let Err(error) = state.check_cancellation_point(edge, "input_validation") {
            terminal_error = Some(attach_partial_row_diagnostics(
                error,
                &mut diagnostics,
                "data_tools.cancelled_after_rejection",
            ));
        } else if let Some(report) = diagnostics.take() {
            terminal_error = Some(complete_row_diagnostic_error(report, None));
        }
    }
    if let Some(error) = terminal_error {
        return StagingOutcome::Terminal(Some(error));
    }
    let Some((dir, path)) = staging.take() else {
        return StagingOutcome::Terminal(None);
    };
    match apri_replay(&path) {
        Ok(reader) => StagingOutcome::Replay(StagedReplay {
            reader,
            staged: staged_meta,
            _dir: dir,
        }),
        Err(error) => StagingOutcome::Terminal(Some(error)),
    }
}

/// Apre il file di staging per il replay.
///
/// Il lettore resta **senza buffer**, per scelta. Il reader IPC legge ogni
/// messaggio con quattro `read_exact` (prefisso, lunghezza, metadati, corpo):
/// un buffer ne risparmierebbe tre per batch, sotto il rumore della misura
/// dello staging (qualche centinaio di syscall su 49 batch, contro una
/// variazione di circa 1 ms su 31). In cambio leggerebbe oltre il messaggio
/// corrente, e un errore del
/// sistema operativo sui byte del batch successivo emergerebbe mentre si
/// rilegge quello corrente, anticipando un errore di kernel su quel batch: un
/// cambio nell'ordine degli errori da dichiarare, per un guadagno che non si
/// misura.
///
/// # Errors
///
/// `Io` se il file non si apre; `Internal` se l'intestazione IPC non si legge.
pub(super) fn apri_replay(path: &Path) -> Result<StreamReader<std::fs::File>> {
    let file = std::fs::File::open(path).map_err(PlenoraError::Io)?;
    StreamReader::try_new(file, None)
        .map_err(|error| PlenoraError::Internal(format!("replay staging IPC: {error}")))
}

/// Validazione atomica dell'input geometrico: staging bounded + replay.
pub(super) fn atomic_input_validation_stream(
    mut input: BatchStream,
    state: Rc<ExecState>,
    edge: String,
) -> BatchStream {
    let mut terminal: Option<std::vec::IntoIter<Result<GovernedBatch>>> = None;
    let mut replay: Option<StagedReplay> = None;
    Box::new(std::iter::from_fn(move || {
        if terminal.is_none() && replay.is_none() {
            match stage_input_batches(&mut input, &state, &edge) {
                StagingOutcome::Terminal(error) => {
                    terminal = Some(
                        error
                            .map_or_else(Vec::new, |error| vec![Err(error)])
                            .into_iter(),
                    );
                }
                StagingOutcome::Replay(staged) => replay = Some(staged),
                // Il gate WKB dell'input resta su disco: `stage_input_batches`
                // non produce mai la variante in memoria. Braccio
                // fail-closed, non silenzioso.
                StagingOutcome::Memoria(_) => {
                    terminal = Some(
                        vec![Err(PlenoraError::Internal(
                            "staging input: modalita' memoria non prevista dal gate WKB".into(),
                        ))]
                        .into_iter(),
                    );
                }
            }
        }
        if let Some(active) = terminal.as_mut() {
            return active.next();
        }
        let active = replay.as_mut()?;
        replay_staged_batch(&state, active, &edge)
    }))
}
