//! plenora-engine — contratto del piano, planner, preparer ed executor del DAG
//! (architettura.md).
//!
//! # I due percorsi a compatibilita' congelata
//!
//! - [`table_engine`]: contratto `Plan`/`Step`/`ValidatedPlan`, validazione
//!   fail-closed ed executor della catena tabellare;
//! - [`geo_transport`]: trasporto Arrow v3 (`PLNGEO3`), framing WKB v2
//!   (`PLNGEO2`/`PLNPAIR1`), verifica CRS e pubblicazione atomica.
//!
//! Formato sul filo, messaggi ed errori sono superficie compatibile: i piani
//! con `schema_version <= 3` e i comandi geo di trasporto passano di li' e si
//! aspettano esattamente quelli.
//!
//! # Il DAG
//!
//! - [`plan`]: i formati piano v5 e v6 (DAG dichiarativo, `PlanLimits` in
//!   parsing, migrazione dal piano lineare legacy e dalla v4,
//!   canonicalizzazione per il `plan_hash`, con un dominio per ciascuna delle
//!   due versioni DAG);
//! - [`planner`]: `validate` (architettura.md#planner-ed-executor,
//!   piano-v5.md#identita-e-fingerprint) — inferenza dei contratti arco per
//!   arco, identita' del `ValidatedGraph` e verifica di compatibilita';
//! - [`prepare`] (architettura.md, architettura.md#planner-ed-executor):
//!   `RuntimeContext`/`RuntimeStatistic`, `PreparedKernel` (configurazioni
//!   preparate), segmenti fisici con `SegmentMode` (modalita' fisiche
//!   esplicite), rilascio al last consumer;
//! - [`executor`]: `execute` seriale a pull (streaming reale, segmenti
//!   lineari senza code, parallelismo solo dove conviene) — dispatch dei nodi
//!   sui due percorsi sopra, limiti effettivi, validazione dinamica WKB in
//!   lettura (D8), metriche per nodo e per segmento e scrittura IPC con
//!   publish atomico.
//!
//! architettura.md#planner-ed-executor: l'API pubblica del DAG e' a due passi — [`planner::validate`] e
//! [`execute`]; `prepare` e' interna al crate (la strategia fisica e' un
//! dettaglio di `execute`). L'unica vista pubblica sul piano fisico e'
//! [`explain`], a secco, per l'ispezione (dry-run della CLI).
//!
//! # Che cosa sorveglia l'esecuzione
//!
//! - [`temp_store`] (errori-e-limiti.md): store temporaneo isolato per
//!   `execution_id` con lock file e heartbeat, piu' scavenging all'avvio
//!   delle directory orfane — difesa strutturale contro i crash non
//!   intercettabili;
//! - [`governor`] (architettura.md#memoria e #determinismo): budget memoria
//!   globale di piano `max_governed_memory_bytes`, [`MemoryLease`] RAII
//!   reference-counted con reservation a tre vie, e [`GovernedBatch`] con la
//!   sequenza logica ai confini degli archi — i kernel restano su
//!   `RecordBatch` puro;
//! - [`cancellation`] (errori-e-limiti.md#cancellazione):
//!   [`CancellationToken`] cooperativo osservato ai confini dell'executor,
//!   mai dentro ai kernel — portarlo dentro richiede lo scheduler parallelo
//!   (M3) — con errore dedicato `PlenoraError::Cancelled`;
//! - spill generalizzato (architettura.md#memoria):
//!   `table.sort`/`distinct`/`aggregate` attivano preventivamente la variante
//!   `*_spilled` sopra la soglia stimata "byte input > `max_governed_memory_bytes`";
//!   i file di spill vivono nella directory condivisa del [`TempStore`]
//!   dell'esecuzione e le [`SpillMetrics`] aggregate sono esposte in
//!   [`executor::ExecutionMetrics`].
//!
//! Gli errori portano l'`execution_id` dell'esecuzione nelle varianti
//! `Execution`/`Cancelled` e nel lock del [`TempStore`], la categoria stabile
//! (`PlenoraError::category()`) e gli assi §9 (`phase()`,
//! `remote_effect()`, `retry_disposition()` — R9.7: la disposizione al
//! ritentativo non si riduce a un booleano). La modalita' diagnostica e'
//! opt-in (`RuntimeContext::diagnostics`, contesto strutturale, mai valori).

pub mod cancellation;
/// Classificazione deterministica dell'esito di un worker isolato (§10 di
/// `isolamento.md`). Logica pura e **interna**: il formato sul filo
/// appartiene al modulo `protocollo`, quindi questi tipi non escono dal
/// crate.
mod classificazione;
// Il `commit_token` e' **privato come modulo**: esce solo il tipo, tramite
// un `pub use` piu' sotto.
//
// Un `pub mod` piu' il re-export avrebbe dato due percorsi per la stessa cosa
// — `plenora_engine::commit_token::CommitToken` e
// `plenora_engine::CommitToken` — e con essi le costanti del modulo, che a un
// consumatore non servono: `CHIAVE_FOOTER_COMMIT_TOKEN` e' il nome di una
// chiave che scriviamo noi.
// Cio' che il chiamante deve poter fare e' costruire un token e riceverne il
// rifiuto motivato: due nomi, non sei.
/// Il `commit_token` nel footer di un artefatto: scrittura prima di `finish`,
/// lettura dalla stessa traversata rinforzata che convalida il file.
pub(crate) mod commit_footer;
mod commit_token;
mod error_propagation;
// La rappresentazione condivisa dal `commit_token` e dal digest del
// protocollo: 32 byte in esadecimale minuscolo. Privata alla radice e **mai**
// ri-esportata — cio' che esce dal crate sono i due tipi che la usano, non la
// forma che hanno in comune.
mod esadecimale32;
pub mod executor;
pub mod geo_transport;
pub mod governor;
/// Facciata **instabile e non-production** per il crate `fuzz/` e per la sonda
/// di calibrazione, che stanno fuori dal crate. Non e' nel `default`.
///
/// Compilata anche sotto `test`, cosi' le invarianti che il fuzzer applica
/// hanno **una definizione sola** e le esercita gia' la suite ordinaria,
/// invece di aspettare la campagna notturna.
#[cfg(any(test, feature = "internals"))]
#[doc(hidden)]
pub mod interni;
pub mod ipc_boundary;
pub mod parallelism;
pub mod plan;
pub mod planner;
pub mod prepare;
// Il protocollo e' **sempre privato**, senza eccezioni: e' un canale interno
// fra due processi che spediamo insieme, e renderlo pubblico — anche solo
// sotto una feature — sarebbe la promessa di non cambiarlo. Chi sta fuori dal
// crate passa da [`interni`], che espone un verdetto e una costante, non i
// tipi.
//
// Il `cfg` dice una cosa vera e non la zittisce: il protocollo non ha ancora
// un chiamante **fuori da se stesso**. Finche' non ce l'ha, il modulo si
// compila dove qualcuno lo usa davvero — i test e la facciata. Cosi' non serve
// nessun `allow(dead_code)`: l'assenza di chiamante e' dichiarata, non
// nascosta.
//
// L'handshake sta **dentro** `protocollo`: consuma i suoi messaggi ma non e'
// un chiamante del modulo, quindi non soddisfa la condizione. Toglierlo prima
// che un chiamante esterno esista rimetterebbe in piedi le decine di
// `dead_code` che il `cfg` evita — cioe' l'esatta situazione per cui esiste.
//
// Regola, perimetro e condizione di rientro sono registrati in
// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
#[cfg(any(test, feature = "internals"))]
mod protocollo;
pub mod table_engine;
pub mod temp_store;

pub use cancellation::CancellationToken;
pub use commit_token::{CommitToken, FormaTokenNonValida};
pub use executor::{execute, ExecutionMetrics, Input, Inputs, NodeMetrics, Output, SegmentMetrics};
pub use governor::{GovernedBatch, MemoryGovernor, MemoryLease, MemoryMetrics, ReservationResult};
pub use ipc_boundary::{BoundaryBatches, IpcFormat, IpcLimits};
pub use plenora_kernels_table::spill::SpillMetrics;
pub use prepare::{
    explain, AccessorKind, BatchTarget, ExecutionPlan, GeoRole, InputStatistics, LastConsumer,
    MeasureKind, MetricsConfig, ParallelismStrategy, PhysicalSegment, PreparedConfig,
    PreparedKernel, RuntimeContext, SegmentMode,
};
pub use table_engine::{
    execute_batch, execute_batch_with_spill, execute_binary, execute_complete_batch, Limits, Plan,
    Step, ValidatedPlan,
};
pub use temp_store::{scavenge_stale_temp_dirs, ScavengeReport, TempStore, DEFAULT_SCAVENGE_TTL};
