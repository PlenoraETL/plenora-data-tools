//! Stato mutabile condiviso dalle chiusure dello stream.
//!
//! Contatori dei limiti effettivi, metriche per nodo e per segmento, governor
//! della memoria, store temporaneo, heartbeat.
//!
//! # Perche' `Rc`/`RefCell` e non `Arc`/`Mutex`
//!
//! L'esecuzione fra i nodi del DAG e' **seriale**: il parallelismo vive
//! dentro i kernel, non fra loro. Uno stato thread-locale e' quindi corretto
//! per costruzione, e costa meno di una sincronizzazione che nessuno userebbe.
//! Quando M3 introdurra' lo scheduler parallelo questo e' il primo punto da
//! rivedere — ed e' scritto qui perche' si veda subito, invece di scoprirlo
//! quando il compilatore si lamentera' di `Send`.
//!
//! # L'heartbeat
//!
//! Un fallimento isolato non ferma l'esecuzione: il disco puo' avere un
//! singhiozzo. Un fallimento che dura cinque minuti si', perche' a quel punto
//! non stiamo piu' sorvegliando nulla e proseguire significherebbe promettere
//! una garanzia che non abbiamo piu'.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use plenora_core::arrow::schema::SchemaRef;
use plenora_core::catalog::CancellationBehavior;
use plenora_core::PlenoraError;
use plenora_core::Result;
use plenora_kernels_table::spill::SpillMetrics;

use crate::geo_transport::transport::{prepare_one_to_one, OneToOnePrepared, TransformArrowSchema};
use crate::governor::MemoryGovernor;
use crate::prepare::{ExecutionPlan, PreparedKernel};
use crate::temp_store::TempStore;
use crate::CancellationToken;

use super::metrics::{accumulate, ExecutionMetrics, NodeMetrics, SegmentMetrics};
use super::{cancellation_behavior, step_error};

/// Stato mutabile condiviso tra le chiusure dello stream (contatori per i
/// limiti effettivi e metriche). `Rc`/`RefCell`: esecuzione seriale v1 (parallelismo solo dove conviene).
pub(super) struct ExecState {
    pub(super) plan: Rc<ExecutionPlan>,
    pub(super) metrics: RefCell<ExecutionMetrics>,
    /// Governor del budget memoria globale di piano (architettura.md#memoria).
    pub(super) governor: MemoryGovernor,
    /// Identita' dell'esecuzione (errori-e-limiti.md, errori arricchiti): riportata negli errori
    /// `Execution`/`Cancelled` e nel lock del `TempStore`.
    pub(super) execution_id: String,
    /// Token di cancellazione cooperativa (errori-e-limiti.md#cancellazione):
    /// osservato solo ai
    /// confini dell'executor, mai dentro ai kernel (M3).
    pub(super) cancellation: CancellationToken,
    /// Diagnostica opt-in (errori-e-limiti.md, errori arricchiti): arricchisce le motivazioni degli
    /// errori con contesto strutturale, mai valori.
    pub(super) diagnostics: bool,
    /// Store temporaneo dell'esecuzione (errori-e-limiti.md): heartbeat al punto
    /// centrale, cleanup RAII al `Drop`.
    pub(super) temp_store: RefCell<TempStore>,
    /// Directory di spill condivisa (architettura.md#memoria, Fase 2B, spill generalizzato): `spill/`
    /// sotto il `TempStore`, risolta UNA volta alla costruzione (hot path minimale) —
    /// il path e' fisso per tutta l'esecuzione.
    pub(super) spill_directory: PathBuf,
    /// Metriche di spill aggregate (architettura.md#memoria, Fase 2B, spill generalizzato): alimentate dai
    /// percorsi `*_spilled` attivati nei nodi tabellari.
    pub(super) spill_metrics: RefCell<SpillMetrics>,
    /// Istante dell'ultimo heartbeat scritto (throttle).
    pub(super) last_heartbeat: Cell<Instant>,
    /// Istante del PRIMO fallimento di una serie consecutiva di heartbeat,
    /// azzerato al primo successo. `None` significa «l'ultimo tentativo e'
    /// andato bene», non «non ci sono stati tentativi»: distinguere i due
    /// casi conta, perche' un'operazione lunga puo' non chiamare l'heartbeat
    /// per minuti senza che nulla sia rotto.
    pub(super) heartbeat_fallito_da: Cell<Option<Instant>>,
    /// Righe/batch/byte cumulati per input (`max_input_rows`, `max_batches`,
    /// `max_payload_bytes`).
    pub(super) input_counts: RefCell<HashMap<String, (u64, u64, u64)>>,
    /// Righe/batch cumulati per arco intermedio (`max_rows_per_edge`).
    pub(super) edge_counts: RefCell<HashMap<String, (u64, u64)>>,
    /// Righe in/out cumulate per nodo (`max_expansion_factor`).
    pub(super) node_rows: RefCell<HashMap<String, (u64, u64)>>,
    /// Handle prepared delle operazioni geo 1:1, costruito una volta per
    /// nodo (hot path minimale): indice di colonna e schema di output non sono piu'
    /// risolti a ogni batch.
    pub(super) prepared_one_to_one: RefCell<HashMap<String, Rc<OneToOnePrepared>>>,
}

/// Intervallo minimo tra due heartbeat del `TempStore` (errori-e-limiti.md): il punto
/// naturale e' "ogni batch processato", ma la scrittura del lock file ha un
/// costo — un heartbeat al secondo e' di gran lunga piu' frequente del TTL
/// di scavenging (24 ore di default) anche con batch piccolissimi.
pub(super) const HEARTBEAT_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Per quanto si tollera che l'heartbeat fallisca **di seguito** prima di
/// interrompere l'esecuzione (errori-e-limiti.md).
///
/// Cinque minuti: lo stesso ordine di grandezza della grazia che lo
/// scavenging concede prima di dare retta al PID, e tre ordini di grandezza
/// sotto il TTL di default. Abbastanza da attraversare un guasto transitorio
/// del filesystem, abbastanza poco da accorgersi di uno permanente molto
/// prima che la directory diventi raccoglibile.
pub(super) const HEARTBEAT_MAX_FAILURE: Duration = Duration::from_secs(300);

/// Aggiunge il contesto strutturale al testo di un errore.
///
/// Estratta da `ExecState::with_diagnostics` per poter essere verificata
/// **insieme** a `with_execution_id`, che e' il punto in cui il difetto
/// viveva: le due funzioni vanno esercitate in sequenza, e un test che
/// costruisse a mano lo stato gia' corretto non proverebbe nulla.
///
/// Il dettaglio e' contesto STRUTTURALE — indice di batch, riga, colonna —
/// mai un valore.
pub(super) fn arricchisci_con_dettaglio(error: PlenoraError, detail: &str) -> PlenoraError {
    let suffix = format!(" [{detail}]");
    match error {
        PlenoraError::Execution {
            node,
            operation,
            execution_id,
            reason,
        } => PlenoraError::Execution {
            node,
            operation,
            execution_id,
            reason: format!("{reason}{suffix}"),
        },
        PlenoraError::InvalidPlan(reason) => PlenoraError::InvalidPlan(format!("{reason}{suffix}")),
        // Da quando la propagazione conserva la categoria, il contesto del
        // passo viaggia in `Replayed` invece che in `Execution`: se
        // l'arricchimento non seguisse anche quell'involucro, attivare
        // `diagnostics` non aggiungerebbe piu' nulla.
        //
        // Si scrive in ENTRAMBI i campi. Per le categorie `Execution` e
        // `Cancelled`, `with_execution_id` RIGENERA il messaggio da
        // `execution_reason` per inserirvi l'id: un suffisso che vivesse solo
        // nel messaggio verrebbe cancellato dalla chiamata immediatamente
        // successiva, e la diagnostica risulterebbe attiva senza aggiungere
        // nulla. Scrivendo in entrambi, le due operazioni commutano.
        PlenoraError::Replayed(mut replayed) => {
            replayed.message = format!("{}{suffix}", replayed.message);
            if let Some(reason) = replayed.execution_reason.take() {
                replayed.execution_reason = Some(format!("{reason}{suffix}"));
            }
            PlenoraError::Replayed(replayed)
        }
        // Gli involucri trasparenti non sono l'errore: si arricchisce cio'
        // che contengono.
        PlenoraError::Tagged { source, phase } => PlenoraError::Tagged {
            source: Box::new(arricchisci_con_dettaglio(*source, detail)),
            phase,
        },
        PlenoraError::RowDiagnostics {
            source,
            diagnostics,
        } => PlenoraError::RowDiagnostics {
            source: Box::new(arricchisci_con_dettaglio(*source, detail)),
            diagnostics,
        },
        other => other,
    }
}

impl ExecState {
    pub(super) fn new(
        plan: &Rc<ExecutionPlan>,
        execution_id: String,
        cancellation: CancellationToken,
        diagnostics: bool,
        temp_store: TempStore,
    ) -> Rc<Self> {
        let mut metrics = ExecutionMetrics::default();
        for segment in plan.segments() {
            metrics.segments.insert(
                segment.id.clone(),
                SegmentMetrics {
                    mode: segment.mode,
                    rows_in: 0,
                    rows_out: 0,
                    batches_in: 0,
                    batches_out: 0,
                    wall_time: Duration::ZERO,
                },
            );
            for kernel in &segment.kernels {
                metrics.nodes.insert(
                    kernel.node_id.clone(),
                    NodeMetrics {
                        operation: kernel.operation.as_str().to_owned(),
                        ..NodeMetrics::default()
                    },
                );
            }
        }
        let spill_directory = temp_store.path().join("spill");
        Rc::new(Self {
            plan: Rc::clone(plan),
            metrics: RefCell::new(metrics),
            governor: MemoryGovernor::new(plan.limits().max_governed_memory_bytes),
            execution_id,
            cancellation,
            diagnostics,
            temp_store: RefCell::new(temp_store),
            spill_directory,
            spill_metrics: RefCell::new(SpillMetrics::default()),
            last_heartbeat: Cell::new(Instant::now()),
            heartbeat_fallito_da: Cell::new(None),
            input_counts: RefCell::new(HashMap::new()),
            edge_counts: RefCell::new(HashMap::new()),
            node_rows: RefCell::new(HashMap::new()),
            prepared_one_to_one: RefCell::new(HashMap::new()),
        })
    }

    /// Handle prepared dell'operazione geo 1:1 del nodo, costruito alla
    /// prima occorrenza (hot path minimale): indice di colonna e schema di output sono
    /// risolti UNA volta per kernel, non per batch. La chiave e' l'id del
    /// nodo: lo schema di input di un arco e' fisso per contratto.
    pub(super) fn one_to_one_prepared(
        &self,
        kernel: &PreparedKernel,
        schema: &SchemaRef,
        params: &TransformArrowSchema,
    ) -> Result<Rc<OneToOnePrepared>> {
        if let Some(prepared) = self.prepared_one_to_one.borrow().get(&kernel.node_id) {
            return Ok(prepared.clone());
        }
        let prepared =
            Rc::new(prepare_one_to_one(schema, params).map_err(|error| {
                step_error(kernel, PlenoraError::InvalidPlan(error.to_string()))
            })?);
        self.prepared_one_to_one
            .borrow_mut()
            .insert(kernel.node_id.clone(), prepared.clone());
        Ok(prepared)
    }

    /// Snapshot delle metriche correnti (con l'osservabilita' dei lease
    /// letta dal governor al momento della chiamata, architettura.md#memoria).
    pub(super) fn metrics(&self) -> ExecutionMetrics {
        let mut metrics = self.metrics.borrow().clone();
        metrics.memory = self.governor.snapshot();
        metrics.spill = *self.spill_metrics.borrow();
        metrics.counters_saturated |= metrics.spill.saturated;
        metrics
    }

    /// Directory di spill condivisa dell'esecuzione (architettura.md#memoria, Fase 2B, spill generalizzato):
    /// sotto-directory `spill/` del `TempStore` — creata dal workspace di
    /// spill al primo uso, ripulita dei file a fine operazione e rimossa
    /// interamente dal `Drop` RAII dello store. Risolta in `new` (hot path minimale).
    pub(super) fn spill_directory(&self) -> &Path {
        &self.spill_directory
    }

    /// Accumula le metriche di uno spill attivato in un nodo tabellare.
    ///
    /// La saturazione viaggia dentro `SpillMetrics` e riemerge in
    /// `ExecutionMetrics::counters_saturated` quando le metriche vengono
    /// lette: un contatore di spill a fondo scala non deve poter convivere
    /// con un flag che dice «nessuna saturazione».
    pub(super) fn add_spill_metrics(&self, delta: SpillMetrics) {
        self.spill_metrics.borrow_mut().accumulate(delta);
    }

    /// Accumula le righe prodotte da un nodo senza clonare la chiave dopo il
    /// primo batch. Il conteggio input viene aggiornato da `check_expansion`.
    ///
    /// La somma e' SATURANTE, non avvolgente: questi contatori decidono se un
    /// limite e' superato, e un contatore che avvolge riaprirebbe il limite
    /// (in release) o farebbe abortire l'esecuzione (in debug, con
    /// `overflow-checks`). Saturare tiene il conteggio nel verso giusto — piu'
    /// alto, quindi piu' restrittivo.
    pub(super) fn add_node_rows_out(&self, node_id: &str, rows_out: u64) {
        let mut rows = self.node_rows.borrow_mut();
        if let Some(entry) = rows.get_mut(node_id) {
            entry.1 = entry.1.saturating_add(rows_out);
        } else {
            let entry = rows.entry(node_id.to_owned()).or_insert((0, 0));
            entry.1 = entry.1.saturating_add(rows_out);
        }
    }

    /// Un batch e' ricaduto dal runner fuso geo al percorso non fuso
    /// (architettura.md#geometrie D12.7): contatore dedicato, mai silenzioso — nessun errore
    /// nuovo, il risultato resta identico.
    pub(super) fn record_geo_fusion_fallback(&self) {
        let mut metrics = self.metrics.borrow_mut();
        let metrics = &mut *metrics;
        accumulate(
            &mut metrics.geo_fusion_fallbacks,
            1,
            &mut metrics.counters_saturated,
        );
    }

    /// Heartbeat del `TempStore` al punto centrale (ogni batch processato
    /// passa dal conteggio delle metriche), con throttle di
    /// [`HEARTBEAT_MIN_INTERVAL`]. Best-effort: un heartbeat fallito degrada
    /// un segnale diagnostico (errori-e-limiti.md: mai una prova), non deve fermare
    /// l'esecuzione — il cleanup RAII resta la pulizia principale.
    pub(super) fn heartbeat(&self) {
        if self.last_heartbeat.get().elapsed() < HEARTBEAT_MIN_INTERVAL {
            return;
        }
        // Il throttle avanza SOLO se la scrittura e' riuscita: un heartbeat
        // fallito va ritentato al batch successivo, non fra un intervallo.
        if self.temp_store.borrow_mut().heartbeat().is_ok() {
            self.last_heartbeat.set(Instant::now());
            self.heartbeat_fallito_da.set(None);
        } else if self.heartbeat_fallito_da.get().is_none() {
            self.heartbeat_fallito_da.set(Some(Instant::now()));
        }
    }

    /// Interrompe l'esecuzione se l'heartbeat fallisce da troppo tempo.
    ///
    /// Il singolo fallimento resta tollerato — una `write` puo' fallire per
    /// una ragione transitoria e fermare per questo un'esecuzione lunga
    /// sarebbe sproporzionato. Un fallimento **persistente** e' un'altra
    /// cosa: il timestamp nel lock smette di avanzare, e superato il TTL lo
    /// scavenging di un altro avvio puo' considerare orfana la directory di
    /// questa esecuzione e cancellargliela sotto — con dentro lo spill.
    /// Proseguire in silenzio sarebbe una failure silenziosa, che questo
    /// progetto non ammette: la tolleranza e' limitata, dichiarata e
    /// scaduta la quale l'errore e' esplicito.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Io` se nessun heartbeat riesce da piu' di
    /// [`HEARTBEAT_MAX_FAILURE`].
    pub(super) fn verifica_heartbeat(&self) -> Result<()> {
        match self.heartbeat_fallito_da.get() {
            Some(dal) if dal.elapsed() > HEARTBEAT_MAX_FAILURE => {
                Err(PlenoraError::Io(std::io::Error::other(format!(
                    "heartbeat del temp store fallito senza interruzione da oltre {} secondi: \
                     la directory temporanea dell'esecuzione puo' essere considerata orfana \
                     e rimossa",
                    HEARTBEAT_MAX_FAILURE.as_secs()
                ))))
            }
            _ => Ok(()),
        }
    }

    /// Errore di cancellazione attribuito a un punto del DAG
    /// (errori-e-limiti.md#cancellazione):
    /// contesto (nodo, operazione, `execution_id`), mai dati.
    pub(super) fn cancelled(&self, node: &str, operation: &str) -> PlenoraError {
        PlenoraError::Cancelled {
            node: node.to_owned(),
            operation: operation.to_owned(),
            execution_id: self.execution_id.clone(),
            reason: "cancellazione richiesta dal chiamante".to_owned(),
        }
    }

    /// Check di cancellazione al confine di un kernel (errori-e-limiti.md#cancellazione): onora il
    /// `CancellationBehavior` dichiarato in catalogo — `NonInterruptible`
    /// non offre punti di interruzione (mai check).
    pub(super) fn check_cancellation(&self, kernel: &PreparedKernel) -> Result<()> {
        if cancellation_behavior(kernel) == CancellationBehavior::NonInterruptible {
            return Ok(());
        }
        self.check_cancellation_point(&kernel.node_id, kernel.operation.as_str())
    }

    /// Check di cancellazione a un confine di piano (output) o di batch in
    /// ingresso a un segmento: e' lavoro dell'executor, non del kernel —
    /// sempre attivo, anche a valle di op `NonInterruptible` (errori-e-limiti.md: nessuna
    /// nuova attivita' dopo la cancellazione, publish compreso).
    pub(super) fn check_cancellation_point(&self, node: &str, operation: &str) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled(node, operation));
        }
        Ok(())
    }

    /// Tag di categoria al confine di uscita: ogni errore `Execution` che lascia il DAG
    /// porta l'`execution_id` (riempito qui se il punto di origine, in
    /// profondita' nel dispatch, non lo aveva a disposizione).
    pub(super) fn tag_execution(&self, error: PlenoraError) -> PlenoraError {
        error.with_execution_id(&self.execution_id)
    }

    /// Arricchimento diagnostico opt-in (errori-e-limiti.md, errori arricchiti): con `diagnostics`
    /// attivo aggiunge alla motivazione contesto strutturale — indice di
    /// batch, riga, colonna dove disponibile, MAI valori. A flag spento (o
    /// dettaglio assente) l'errore passa invariato: messaggi retrocompatibili.
    pub(super) fn with_diagnostics(
        &self,
        error: PlenoraError,
        detail: Option<&str>,
    ) -> PlenoraError {
        if !self.diagnostics {
            return error;
        }
        let Some(detail) = detail else {
            return error;
        };
        arricchisci_con_dettaglio(error, detail)
    }
}
