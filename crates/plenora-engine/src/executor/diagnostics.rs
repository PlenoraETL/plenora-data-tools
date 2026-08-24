//! Diagnostica di riga: dire QUALE riga, senza dire che cosa conteneva.
//!
//! Quando un'operazione scarta righe, il chiamante ha diritto di sapere quante e
//! quali — per indice, mai per contenuto. E' la stessa disciplina degli errori
//! senza dati applicata a una superficie piu' insidiosa: un esempio di riga
//! scartata e' utilissimo da avere nei log, ed e' esattamente cio' che non deve
//! finirci.
//!
//! # Completezza dichiarata
//!
//! `RowDiagnosticsCompleteness` esiste perche' un elenco troncato che non dice
//! di esserlo e' peggio di nessun elenco: chi lo legge conclude che le righe
//! scartate erano quelle e basta.

use std::rc::Rc;

use plenora_core::arrow::ipc::reader::StreamReader;
use plenora_core::diagnostics::RowDiagnostics;
use plenora_core::error::ReplayedError;
use plenora_core::{ErrorCategory, ErrorPhase, PlenoraError, Result, RetryDisposition};

use crate::governor::{GovernedBatch, MemoryPermit};
use crate::prepare::ExecutionPlan;

use super::input::BatchStream;
use super::network::EdgeStream;
use super::run_streaming_chain;
use super::staging::{replay_staged_batch, StagedReplay, StagingAccepted, StagingOutcome};
use super::state::ExecState;

/// Fusione dei report row-scoped: la procedura vive in
/// [`RowDiagnostics::merge_into`], condivisa con il runner fuso del trasporto
/// geo. Qui resta solo la traduzione dell'invariante violata nell'errore di
/// questo perimetro.
pub(super) fn merge_row_diagnostics(
    aggregate: &mut Option<RowDiagnostics>,
    incoming: RowDiagnostics,
    source_offset: u64,
) -> Result<()> {
    RowDiagnostics::merge_into(aggregate, incoming, source_offset)
        .map_err(|error| PlenoraError::Internal(error.message().to_owned()))
}

pub(super) fn attach_partial_row_diagnostics(
    error: PlenoraError,
    aggregate: &mut Option<RowDiagnostics>,
    knowledge_limit: &str,
) -> PlenoraError {
    let Some(report) = aggregate.take() else {
        return error;
    };
    error.with_row_diagnostics(report.into_partial(knowledge_limit))
}

pub(super) fn complete_row_diagnostic_error(
    report: RowDiagnostics,
    context: Option<(String, String, String)>,
) -> PlenoraError {
    let source = match context {
        Some((node, operation, execution_id)) => PlenoraError::Replayed(Box::new(ReplayedError {
            category: ErrorCategory::DataMapping,
            phase: ErrorPhase::Read,
            remote_effect: plenora_core::RemoteEffect::None,
            retry: RetryDisposition::Never,
            message: "righe non conformi al contratto di trasformazione".into(),
            node: Some(node),
            operation: Some(operation),
            execution_id: Some(execution_id),
            execution_reason: None,
        })),
        None => {
            PlenoraError::DataMapping("righe non conformi al contratto di trasformazione".into())
                .with_phase(ErrorPhase::Read)
        }
    };
    source.with_row_diagnostics(report)
}

/// Selezione del machinery row-diagnostics per segmento (R9.9): un kernel
/// vi partecipa se e solo se l'autorita' di catalogo
/// (`OperationDescriptor::emits_row_diagnostics`, risolta in `prepare`
/// sulla config del nodo) lo dichiara emittente — stessa classificazione
/// del gate provenance del planner e del gate legacy CLI, nessuna lista
/// locale duplicata.
pub(super) fn segment_emits_row_diagnostics(plan: &ExecutionPlan, segment_index: usize) -> bool {
    plan.segments()[segment_index]
        .kernels
        .iter()
        .any(|kernel| kernel.emits_row_diagnostics)
}

/// Permesso a eseguire la prossima passata **trattenendo** cio' che c'e' gia'.
///
/// Non e' una verifica seguita da una prenotazione: e' **una sola
/// operazione**. Si chiede al governor un permesso per `max_batch_bytes` —
/// il tetto duro per batch (tetto in byte per batch), che il wrapper d'uscita applica a ogni
/// batch di output ed e' quindi un maggiorante valido dell'unica prenotazione
/// che la passata aggiunge. Se il permesso e' concesso, quella quota e'
/// **gia' nostra**: la passata puo' ritagliarne l'output senza che nessun
/// altro possa infilarsi nel mezzo, oggi che l'esecuzione e' seriale come
/// domani che non lo sara'.
///
/// `None` significa "non c'e' spazio per un'altra passata trattenendo": si
/// passa al disco. Non e' un errore ed e' fail-closed — un permesso negato
/// sceglie sempre la modalita' col picco piu' basso.
///
/// In modalita' disco non si chiede nulla: il passaggio e' definitivo.
///
/// Un ingresso **senza lease** non e' contabilizzato dal governor: la
/// decisione poggerebbe su un totale che non comprende i byte in arrivo, e si
/// va su disco.
///
/// # Errors
///
/// Propaga l'errore interno del governor se la sua contabilita' e'
/// incoerente: un diniego di budget (`Ok(None)`) e una contabilita' rotta
/// (`Err`) restano distinti fino in cima, perche' il primo si gestisce
/// passando al disco e il secondo no.
pub(super) fn permesso_di_trattenere(
    state: &ExecState,
    accepted: &StagingAccepted,
    ingresso: &GovernedBatch,
    edge: &str,
) -> Result<Option<MemoryPermit>> {
    if matches!(accepted, StagingAccepted::Disco { .. }) {
        return Ok(None);
    }
    // Un ingresso senza lease non e' contabilizzato dal governor.
    if ingresso.lease.is_none() {
        return Ok(None);
    }
    let Ok(tetto_batch) = u64::try_from(state.plan.batch_target().max_batch_bytes) else {
        return Ok(None);
    };
    // L'owner e' l'arco, non una costante: architettura.md#memoria vuole che un lease vivo
    // sia attribuibile, e `oldest_lease_age` con `owner` e' l'unico modo di
    // sapere CHI sta trattenendo quota. Un nome generico renderebbe la
    // diagnosi impossibile proprio sul lease piu' grande del piano.
    state.governor.permesso(tetto_batch, edge)
}

// Macchina a stati lineare: scansione, decisione memoria/disco, diagnostica e
// chiusura restano nello stesso scope per rendere evidente il cleanup
// fail-closed.
#[allow(clippy::too_many_lines)]
pub(super) fn scan_row_diagnostic_segment(
    input: &mut EdgeStream,
    plan: &Rc<ExecutionPlan>,
    state: &Rc<ExecState>,
    segment_index: usize,
) -> StagingOutcome {
    let mut diagnostics = None;
    let mut diagnostic_context = None;
    let mut input_rejected = false;
    let mut source_offset = 0_u64;
    let mut terminal_error = None;
    let mut accepted = StagingAccepted::nuovo();
    let output_edge = &plan.segments()[segment_index].output_edge;
    for item in input {
        let governed = match item {
            Ok(governed) => governed,
            Err(error) => {
                if let Some(report) = error.row_diagnostics().cloned() {
                    if diagnostic_context.is_none() {
                        diagnostic_context = error.execution_location().map_or_else(
                            || {
                                plan.segments()[segment_index]
                                    .kernels
                                    .first()
                                    .map(|kernel| {
                                        (
                                            kernel.node_id.clone(),
                                            kernel.operation.as_str().to_owned(),
                                            state.execution_id.clone(),
                                        )
                                    })
                            },
                            |(node, operation, execution_id)| {
                                Some((
                                    node.to_owned(),
                                    operation.to_owned(),
                                    execution_id.map_or_else(
                                        || state.execution_id.clone(),
                                        ToOwned::to_owned,
                                    ),
                                ))
                            },
                        );
                    }
                    input_rejected = true;
                    if let Err(error) = merge_row_diagnostics(&mut diagnostics, report, 0) {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            error,
                            &mut diagnostics,
                            "data_tools.diagnostic_merge_failed",
                        ));
                        break;
                    }
                    continue;
                }
                terminal_error = Some(attach_partial_row_diagnostics(
                    error,
                    &mut diagnostics,
                    "data_tools.input_stream_interrupted",
                ));
                break;
            }
        };
        let batch_offset = source_offset;
        let Ok(batch_rows) = u64::try_from(governed.batch.num_rows()) else {
            terminal_error = Some(attach_partial_row_diagnostics(
                PlenoraError::Internal("cardinalita batch fuori intervallo".into()),
                &mut diagnostics,
                "data_tools.source_offset_unrepresentable",
            ));
            break;
        };
        let Some(offset) = source_offset.checked_add(batch_rows) else {
            terminal_error = Some(attach_partial_row_diagnostics(
                PlenoraError::Internal("indice sorgente stream fuori intervallo".into()),
                &mut diagnostics,
                "data_tools.source_offset_overflow",
            ));
            break;
        };
        source_offset = offset;
        // Una rejection di validazione input (WKB) e' attribuita al primo
        // kernel consumatore del segmento, ma continua a drenare/validare
        // l'input per completare i conteggi senza eseguire kernel downstream.
        if diagnostics.is_some() && input_rejected {
            continue;
        }
        // architettura.md#memoria, staging memory-first: la decisione memoria/disco si prende QUI, con il
        // batch d'ingresso gia' prelevato e quindi di dimensione NOTA, e
        // PRIMA di eseguire la catena su di esso. Cosi' la passata successiva
        // non puo' superare il budget: se non ci sta, i trattenuti vanno su
        // disco adesso, non dopo il fallimento.
        let permesso = match permesso_di_trattenere(state, &accepted, &governed, output_edge) {
            Ok(permesso) => permesso,
            Err(error) => {
                // Contabilita' del governor incoerente: e' un'invariante
                // nostra rotta, non un budget esaurito. Termina la scansione
                // senza pubblicare accepted, come ogni altro errore.
                terminal_error = Some(attach_partial_row_diagnostics(
                    error,
                    &mut diagnostics,
                    "data_tools.governor_accounting_broken",
                ));
                break;
            }
        };
        if permesso.is_none() {
            if let Err(error) = accepted.passa_a_disco(state, output_edge) {
                terminal_error = Some(attach_partial_row_diagnostics(
                    error,
                    &mut diagnostics,
                    "data_tools.output_staging_failed",
                ));
                break;
            }
        }
        let diagnostic_node = diagnostic_context
            .as_ref()
            .map(|(node, _, _): &(String, String, String)| node.as_str());
        match run_streaming_chain(
            plan,
            segment_index,
            state,
            governed,
            diagnostic_node,
            permesso,
        ) {
            Ok(output) => {
                if diagnostics.is_none() {
                    // La barriera R9.9 non richiede il disco: richiede che
                    // nulla esca prima della fine della scansione. In
                    // memoria il lease resta vivo e il batch e' consegnato
                    // tale e quale; su disco il lease e' rilasciato qui e
                    // ri-riservato al replay. Se una rejection tardiva
                    // arriva dopo, in entrambi i casi non si pubblica nulla.
                    if let Err(error) = accepted.accogli(state, output_edge, output) {
                        terminal_error = Some(attach_partial_row_diagnostics(
                            error,
                            &mut diagnostics,
                            "data_tools.output_staging_failed",
                        ));
                        break;
                    }
                }
            }
            Err(error) => {
                let Some(report) = error.row_diagnostics().cloned() else {
                    terminal_error = Some(attach_partial_row_diagnostics(
                        error,
                        &mut diagnostics,
                        "data_tools.processing_interrupted",
                    ));
                    break;
                };
                if diagnostic_context.is_none() {
                    if let Some((node, operation, execution_id)) = error.execution_location() {
                        diagnostic_context = Some((
                            node.to_owned(),
                            operation.to_owned(),
                            execution_id
                                .map_or_else(|| state.execution_id.clone(), ToOwned::to_owned),
                        ));
                    }
                }
                if let Err(error) = merge_row_diagnostics(&mut diagnostics, report, batch_offset) {
                    terminal_error = Some(attach_partial_row_diagnostics(
                        error,
                        &mut diagnostics,
                        "data_tools.diagnostic_merge_failed",
                    ));
                    break;
                }
            }
        }
    }
    if terminal_error.is_none() {
        if let StagingAccepted::Disco { writer, .. } = &mut accepted {
            if let Some(active) = writer.as_mut() {
                if let Err(error) = active.finish() {
                    terminal_error = Some(attach_partial_row_diagnostics(
                        PlenoraError::Internal(format!("chiusura staging output: {error}")),
                        &mut diagnostics,
                        "data_tools.output_staging_failed",
                    ));
                }
            }
        }
    }
    if terminal_error.is_none() {
        if let Err(error) = state.check_cancellation_point("output", "row_diagnostics") {
            terminal_error = Some(attach_partial_row_diagnostics(
                error,
                &mut diagnostics,
                "data_tools.cancelled_after_rejection",
            ));
        } else if let Some(report) = diagnostics.take() {
            terminal_error = Some(complete_row_diagnostic_error(
                report,
                diagnostic_context.take(),
            ));
        }
    }
    if let Some(error) = terminal_error {
        // Errore, rejection tardiva o cancellazione: `accepted` e' distrutto
        // qui. In memoria i lease muoiono con la coda, su disco il `TempDir`
        // cancella il file. In nessuno dei due casi esce un batch.
        return StagingOutcome::Terminal(Some(error));
    }
    let (writer, staging, staged_meta) = match accepted {
        // Modalita' memoria: la coda si consegna com'e', in ordine, con i
        // lease gia' vivi. Nessun IPC, nessuna decodifica, nessuna copia.
        StagingAccepted::Memoria(coda) => {
            if coda.is_empty() {
                return StagingOutcome::Terminal(None);
            }
            return StagingOutcome::Memoria(coda);
        }
        StagingAccepted::Disco {
            writer,
            staging,
            meta,
        } => (writer, staging, meta),
    };
    drop(writer);
    let Some((dir, path)) = staging else {
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

/// Machinery R9.9 per i segmenti che emettono diagnostica: scansione
/// completa (staging bounded degli accepted) seguita da replay lazy con
/// ri-riserva per batch — nessun accepted esce prima dello scan completo,
/// nessun lease trattenuto oltre il singolo batch.
pub(super) fn row_diagnostic_stream(
    mut input: EdgeStream,
    plan: Rc<ExecutionPlan>,
    state: Rc<ExecState>,
    segment_index: usize,
) -> BatchStream {
    let mut terminal: Option<std::vec::IntoIter<Result<GovernedBatch>>> = None;
    let mut replay: Option<StagedReplay> = None;
    let mut memoria: Option<std::collections::VecDeque<GovernedBatch>> = None;
    let mut scansione_fatta = false;
    Box::new(std::iter::from_fn(move || {
        if !scansione_fatta {
            scansione_fatta = true;
            match scan_row_diagnostic_segment(&mut input, &plan, &state, segment_index) {
                StagingOutcome::Terminal(error) => {
                    terminal = Some(
                        error
                            .map_or_else(Vec::new, |error| vec![Err(error)])
                            .into_iter(),
                    );
                }
                StagingOutcome::Replay(staged) => replay = Some(staged),
                StagingOutcome::Memoria(coda) => memoria = Some(coda),
            }
        }
        if let Some(active) = terminal.as_mut() {
            return active.next();
        }
        // Modalita' memoria: consegna in ordine di produzione, con il lease
        // e la `BatchSequence` originali. Il batch e' lo stesso oggetto
        // prodotto dalla catena — nessun round-trip IPC puo' alterarlo.
        if let Some(coda) = memoria.as_mut() {
            return coda.pop_front().map(Ok);
        }
        let active = replay.as_mut()?;
        let output_edge = &plan.segments()[segment_index].output_edge;
        replay_staged_batch(&state, active, output_edge)
    }))
}
