//! Staging degli input: materializzazione su disco quando la memoria non basta.
//!
//! Un input che non entra nel budget non viene rifiutato: viene messo da parte
//! su file e riletto. `CountingFile` conta i byte mentre li scrive, cosi' il
//! limite di disco e' un tetto vero e non una stima.
//!
//! # La validazione e' atomica per un motivo
//!
//! `atomic_input_validation_stream` valida TUTTI i batch prima di lasciarne
//! passare uno. Validare in streaming sarebbe piu' economico, ma pubblicherebbe
//! righe valide di un input che si scoprira' invalido tre batch dopo — e a quel
//! punto l'output parziale e' gia' uscito.

use std::path::Path;
use std::rc::Rc;

use plenora_core::arrow::array::{RecordBatch, UInt32Array};
use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::arrow::ipc::writer::StreamWriter;
use plenora_core::arrow::select::take::take;
use plenora_core::contract::BatchSequence;
use plenora_core::{PlenoraError, Result};

use crate::governor::GovernedBatch;

use super::diagnostics::{
    attach_partial_row_diagnostics, complete_row_diagnostic_error, merge_row_diagnostics,
};
use super::input::BatchStream;
use super::state::ExecState;

/// Writer con conteggio dei byte e quota dichiarata (`max_temp_bytes` del
/// piano): superata la quota la scrittura fallisce con errore esplicito,
/// mai silenzioso.
pub(super) struct CountingFile {
    file: std::fs::File,
    written: u64,
    max_bytes: u64,
}

impl CountingFile {
    fn create(path: &Path, max_bytes: u64) -> Result<Self> {
        let file = std::fs::File::create(path).map_err(PlenoraError::Io)?;
        Ok(Self {
            file,
            written: 0,
            max_bytes,
        })
    }
}

impl std::io::Write for CountingFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.written.checked_add(buf.len() as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "overflow conteggio staging IPC",
            )
        })?;
        if written > self.max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "staging IPC oltre max_temp_bytes",
            ));
        }
        let n = self.file.write(buf)?;
        self.written = self.written.checked_add(n as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "overflow conteggio staging IPC",
            )
        })?;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
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

/// Validazione atomica dell'input geometrico (D8/B1.3) con memoria BOUNDED:
/// i batch accettati sono staged su IPC entro la quota `max_temp_bytes`
/// dichiarata dal piano e il lease governor e' rilasciato subito; solo a
/// validazione completata senza rifiuti i batch sono riletti uno alla
/// volta, con lease ri-riservato per batch e stessa sequenza logica.
/// Invarianti R9.9 preservate: nessun accepted esce prima della validazione
/// completa; un rifiuto row-scoped (anche tardivo) produce il report
/// completo mergiato e zero accepted; un errore non row-scoped propaga
/// fail-closed con la diagnostica parziale dichiarata. Un errore di I/O in
/// replay e' una failure infrastrutturale (accepted parziali possibili,
/// come ogni failure mid-stream): non e' un rifiuto di righe.
///
/// Nota quota: lo staging degli input, lo staging degli output accettati
/// dei segmenti row-diagnostics e gli spill degli operatori misurano
/// ciascuno la propria scrittura contro `max_temp_bytes`; la somma su
/// disco puo' superare la quota (v1, contabilita' separate).
/// Esito della fase di staging dell'input gate: errore terminale (eventuale
/// assenza di batch staged -> stream vuoto) oppure replay dal file staged.
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
/// # Perche' esiste
///
/// La barriera R9.9 — nessun accepted pubblicato prima che la scansione sia
/// completa — non richiede il disco: richiede solo che nulla esca prima della
/// fine. Trattenere i batch gia' governati la soddisfa allo stesso modo, e
/// risparmia per ogni riga una serializzazione IPC, una scrittura, una
/// rilettura, una decodifica e una copia `take`.
///
/// # Perche' non puo' trasformare un input eseguibile in un `ResourceLimit`
///
/// Durante una passata della catena i lease vivi sono al piu' due: quello
/// del batch d'ingresso e quello dell'uscita (`run_streaming_chain` acquisisce
/// il secondo prima di rilasciare il primo). Quindi:
///
/// - **su disco** il picco della passata `k` e' `input_k + output_k`;
/// - **in memoria** e' `trattenuti + input_k + output_k`.
///
/// Si entra nella passata `k` in modalita' memoria **solo se**
/// `trattenuti + input_k + max_batch_bytes <= budget`, dove `input_k` e' la
/// dimensione REALE del batch gia' prelevato e `max_batch_bytes` e' il tetto
/// duro del piano (tetto in byte per batch). Ogni batch di output attraversa il wrapper d'uscita,
/// che applica lo stesso tetto: `output_k > max_batch_bytes` fa fallire il
/// piano **in entrambe le modalita'**. Per un piano che prima riusciva vale
/// quindi `output_k <= max_batch_bytes`, e il picco in memoria non supera il
/// budget.
///
/// La soglia e' **derivata dai limiti del piano e dai lease effettivamente
/// vivi**: nessuna percentuale scelta a mano, nessuna decisione temporale,
/// nessuna dipendenza dall'ordine di arrivo.
// La variante `Disco` porta writer e handle del file: piu' grande di una
// `VecDeque`, ma esiste al massimo una volta per segmento e boxarla
// aggiungerebbe un'indirezione sul percorso caldo dello staging.
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
        let stream = StreamWriter::try_new(counting, &batch.schema())
            .map_err(|error| PlenoraError::Internal(format!("staging {what}: {error}")))?;
        *writer = Some(stream);
        *staging = Some((dir, path));
    }
    let active = writer
        .as_mut()
        .ok_or_else(|| PlenoraError::Internal(format!("staging {what} non inizializzato")))?;
    active.write(batch).map_err(|error| {
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
    match std::fs::File::open(&path)
        .map_err(PlenoraError::Io)
        .and_then(|file| {
            StreamReader::try_new(file, None)
                .map_err(|error| PlenoraError::Internal(format!("replay staging IPC: {error}")))
        }) {
        Ok(reader) => StagingOutcome::Replay(StagedReplay {
            reader,
            staged: staged_meta,
            _dir: dir,
        }),
        Err(error) => StagingOutcome::Terminal(Some(error)),
    }
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
