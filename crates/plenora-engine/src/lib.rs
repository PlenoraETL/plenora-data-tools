//! plenora-engine — contratto del piano, planner, preparer ed executor del DAG
//! (architettura.md).
//!
//! Fase 1 "coesistenza": due moduli distinti, portati meccanicamente dai
//! progetti di origine senza modifiche di comportamento:
//!
//! - [`table_engine`]: contratto `Plan`/`Step`/`ValidatedPlan`, validazione
//!   fail-closed ed executor della catena tabellare (da
//!   `plenora-nogeo-tools/src/contract.rs` ed `engine.rs`);
//! - [`geo_transport`]: trasporto Arrow v3 (`PLNGEO3`), framing WKB v2
//!   (`PLNGEO2`/`PLNPAIR1`), verifica CRS e pubblicazione atomica (da
//!   `plenora-geo-tools-arrow/src/arrow_transport.rs`, `protocol.rs`,
//!   `pair_protocol.rs` e dal livello comandi di `main.rs`).
//!
//! L'unificazione in un DAG unico e' in corso: Fase 2A introduce
//! [`plan`] — i formati piano v5 e v6 (DAG dichiarativo, `PlanLimits` in
//! parsing, migrazione dal piano lineare legacy e dalla v4,
//! canonicalizzazione per il `plan_hash`, con un dominio per ciascuna delle
//! due versioni DAG) — e [`planner`] — la fase 1 `validate`
//! (architettura.md#planner-ed-executor, piano-v5.md#identita-e-fingerprint):
//! inferenza dei contratti arco per arco, identita' del
//! `ValidatedGraph` e verifica di compatibilita'. Fase 2A-4 aggiunge
//! [`prepare`] — la fase 2 `prepare` (architettura.md, architettura.md#planner-ed-executor):
//! `RuntimeContext`/`RuntimeStatistic`, `PreparedKernel` (configurazioni preparate), segmenti
//! fisici con `SegmentMode` (modalita' fisiche esplicite), last consumer (rilascio al last consumer) — ed [`executor`] —
//! la fase 2 `execute` seriale a pull (streaming reale, segmenti lineari senza code, parallelismo solo dove conviene): dispatch dei nodi sui
//! percorsi esistenti (`table_engine`, `geo_transport`), limiti effettivi,
//! validazione dinamica WKB in lettura (D8), metriche per nodo e per
//! segmento (osservabilita' per nodo) e scrittura IPC con publish atomico.
//!
//! architettura.md#planner-ed-executor: l'API pubblica del DAG e' a due passi — [`planner::validate`] e
//! [`execute`]; `prepare` e' interna al crate (la strategia fisica e' un
//! dettaglio di `execute`). L'unica vista pubblica sul piano fisico e'
//! [`explain`], a secco, per l'ispezione (dry-run della CLI).
//!
//! Fase 2B aggiunge [`temp_store`] (errori-e-limiti.md): store temporaneo isolato per
//! `execution_id` con lock file e heartbeat, piu' scavenging all'avvio delle
//! directory orfane — difesa strutturale contro i crash non intercettabili.
//!
//! Fase 2B, governor della memoria aggiunge [`governor`] (architettura.md#memoria e #determinismo): budget
//! memoria globale di piano `max_governed_memory_bytes`, [`MemoryLease`] RAII
//! reference-counted con reservation a tre vie, e [`GovernedBatch`] con la
//! sequenza logica ai confini degli archi — i kernel restano su
//! `RecordBatch` puro.
//!
//! Fase 2B aggiunge [`cancellation`] e gli errori arricchiti
//! (errori-e-limiti.md#cancellazione): [`CancellationToken`] cooperativo osservato ai confini
//! dell'executor (mai dentro ai kernel — il passaggio e' M3) con
//! errore dedicato `PlenoraError::Cancelled`; `execution_id` per esecuzione
//! negli errori `Execution`/`Cancelled` e nel lock del `TempStore`;
//! `PlenoraError::category()` e gli assi §9 (`phase()`, `remote_effect()`,
//! `retry_disposition()` — R9.7, sostituisce il `retryable()` della prima tassonomia);
//! modalita' diagnostica opt-in
//! (`RuntimeContext::diagnostics`, contesto strutturale, mai valori).
//!
//! Fase 2B aggiunge il wiring dello spill generalizzato (architettura.md#memoria):
//! `table.sort`/`distinct`/`aggregate` attivano preventivamente la variante
//! `*_spilled` sopra la soglia stimata "byte input > `max_governed_memory_bytes`";
//! i file di spill vivono nella directory condivisa del [`TempStore`]
//! dell'esecuzione e le [`SpillMetrics`] aggregate sono esposte in
//! [`executor::ExecutionMetrics`].

pub mod cancellation;
/// Classificazione deterministica dell'esito di un worker isolato (§10 di
/// `isolamento.md`). Logica pura e **interna**: `PR-4` possiede il formato
/// sul filo, quindi questi tipi non escono dal crate.
mod classificazione;
mod error_propagation;
pub mod executor;
pub mod geo_transport;
pub mod governor;
pub mod ipc_boundary;
pub mod parallelism;
pub mod plan;
pub mod planner;
pub mod prepare;
pub mod table_engine;
pub mod temp_store;

pub use cancellation::CancellationToken;
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
