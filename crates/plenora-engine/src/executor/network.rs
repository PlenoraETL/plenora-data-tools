//! Canale d'arco condiviso: il tee del fan-out.
//!
//! Quando piu' nodi consumano lo stesso arco, i batch vanno consegnati a
//! tutti senza rileggere la sorgente e senza tenerli in memoria piu' del
//! necessario (decisione D9: materializzazione minima, rilascio al last
//! consumer).
//!
//! # L'errore va conservato, non ripetuto
//!
//! Un errore a monte raggiunge N consumatori, ma e' successo **una volta
//! sola**. `StoredEdgeError` lo conserva nella sua forma completa — categoria,
//! fase, effetto remoto, disposizione di retry, diagnostica di riga — e ogni
//! consumatore successivo lo riceve come `Replayed`: sa che sta guardando la
//! ripetizione di un errore gia' accaduto, non un secondo guasto. Senza quella
//! distinzione N consumatori produrrebbero N errori indistinguibili, e chi
//! legge i log conterebbe N guasti dove ce n'era uno.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use plenora_core::diagnostics::RowDiagnostics;
use plenora_core::error::ReplayedError;
use plenora_core::Result;
use plenora_core::{ErrorCategory, ErrorPhase, PlenoraError, RetryDisposition};

use crate::governor::GovernedBatch;

use super::input::BatchStream;

/// Errore di un arco conservato in forma scomposta per la riproduzione ai
/// consumatori successivi (`PlenoraError` non e' `Clone`): l'attribuzione
/// originale (`Execution`/`Cancelled` con nodo, operazione ed `execution_id`) e'
/// preservata, non declassata a `InvalidPlan`.
pub(super) struct StoredEdgeError {
    category: ErrorCategory,
    phase: ErrorPhase,
    remote_effect: plenora_core::RemoteEffect,
    retry: RetryDisposition,
    node: Option<String>,
    operation: Option<String>,
    execution_id: Option<String>,
    execution_reason: Option<String>,
    reason: String,
    row_diagnostics: Option<Box<RowDiagnostics>>,
}

impl StoredEdgeError {
    pub(super) fn from_error(error: &PlenoraError) -> Self {
        let (node, operation, execution_id) = error.execution_location().map_or(
            (None, None, None),
            |(node, operation, execution_id)| {
                (
                    Some(node.to_owned()),
                    Some(operation.to_owned()),
                    execution_id.map(ToOwned::to_owned),
                )
            },
        );
        Self {
            category: error.category(),
            phase: error.phase(),
            remote_effect: error.remote_effect(),
            retry: error.retry_disposition(),
            node,
            operation,
            execution_id,
            execution_reason: error.execution_reason().map(ToOwned::to_owned),
            reason: error.to_string(),
            row_diagnostics: error.row_diagnostics().cloned().map(Box::new),
        }
    }

    pub(super) fn to_error(&self) -> PlenoraError {
        let replayed = PlenoraError::Replayed(Box::new(ReplayedError {
            category: self.category,
            phase: self.phase,
            remote_effect: self.remote_effect,
            retry: self.retry,
            message: self.reason.clone(),
            node: self.node.clone(),
            operation: self.operation.clone(),
            execution_id: self.execution_id.clone(),
            execution_reason: self.execution_reason.clone(),
        }));
        match &self.row_diagnostics {
            Some(diagnostics) => replayed.with_row_diagnostics((**diagnostics).clone()),
            None => replayed,
        }
    }
}

/// Stato di un arco: upstream lazy, buffer condiviso tra i consumatori e
/// cursore di lettura per ciascuno. Il buffer trattiene [`GovernedBatch`]:
/// il lease e' condiviso (clone `Arc`) tra i consumatori — la quota del
/// batch e' contata UNA volta all'ingresso dell'arco e torna al governor al
/// `Drop` dell'ultimo riferimento (architettura.md#memoria).
pub(super) struct EdgeShared {
    upstream: RefCell<Option<BatchStream>>,
    buffer: RefCell<Vec<GovernedBatch>>,
    reads: RefCell<Vec<usize>>,
    done: Cell<bool>,
    /// Errore upstream, riprodotto una sola volta a ciascun consumatore.
    error: RefCell<Option<StoredEdgeError>>,
}

impl EdgeShared {
    pub(super) fn new(upstream: BatchStream) -> Rc<Self> {
        Rc::new(Self {
            upstream: RefCell::new(Some(upstream)),
            buffer: RefCell::new(Vec::new()),
            reads: RefCell::new(Vec::new()),
            done: Cell::new(false),
            error: RefCell::new(None),
        })
    }

    pub(super) fn register_reader(self: &Rc<Self>) -> EdgeStream {
        let mut reads = self.reads.borrow_mut();
        let id = reads.len();
        reads.push(0);
        EdgeStream {
            shared: Rc::clone(self),
            id,
            error_delivered: false,
        }
    }
}

/// Handle di lettura di un consumatore su un arco condiviso.
pub(super) struct EdgeStream {
    shared: Rc<EdgeShared>,
    id: usize,
    /// L'errore dell'arco e' consegnato UNA volta per consumatore, poi lo
    /// stream termina (`None`): mai un iteratore infinito di errori.
    error_delivered: bool,
}

impl EdgeStream {
    /// Rilascia i batch letti da tutti i consumatori (rilascio al last consumer).
    ///
    /// Nel caso a consumatore singolo i batch non sono bufferizzati affatto:
    /// il cursore e' clam-pato alla lunghezza del buffer condiviso.
    pub(super) fn release_consumed(&self) {
        let mut reads = self.shared.reads.borrow_mut();
        let mut buffer = self.shared.buffer.borrow_mut();
        let Some(min_read) = reads.iter().copied().min() else {
            return;
        };
        let min_read = min_read.min(buffer.len());
        if min_read == 0 {
            return;
        }
        buffer.drain(..min_read);
        for cursor in reads.iter_mut() {
            *cursor -= min_read;
        }
    }
}

impl Iterator for EdgeStream {
    type Item = Result<GovernedBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        // 1. Batch gia' bufferizzato per questo consumatore.
        {
            let buffer = self.shared.buffer.borrow();
            let position = self.shared.reads.borrow()[self.id];
            if position < buffer.len() {
                let batch = buffer[position].clone();
                drop(buffer);
                self.shared.reads.borrow_mut()[self.id] += 1;
                self.release_consumed();
                return Some(Ok(batch));
            }
        }
        // 2. Upstream esaurito (o in errore): l'errore e' consegnato una
        // sola volta per consumatore, poi lo stream e' chiuso.
        if self.shared.done.get() {
            if self.error_delivered {
                return None;
            }
            return self.shared.error.borrow().as_ref().map(|stored| {
                self.error_delivered = true;
                Err(stored.to_error())
            });
        }
        // 3. Pull dall'upstream.
        let item = self.shared.upstream.borrow_mut().as_mut()?.next();
        match item {
            Some(Ok(batch)) => {
                let single_consumer = self.shared.reads.borrow().len() == 1;
                if !single_consumer {
                    self.shared.buffer.borrow_mut().push(batch.clone());
                }
                self.shared.reads.borrow_mut()[self.id] += 1;
                self.release_consumed();
                Some(Ok(batch))
            }
            Some(Err(error)) => {
                self.shared.done.set(true);
                *self.shared.error.borrow_mut() = Some(StoredEdgeError::from_error(&error));
                self.error_delivered = true;
                Some(Err(error))
            }
            None => {
                self.shared.done.set(true);
                None
            }
        }
    }
}
