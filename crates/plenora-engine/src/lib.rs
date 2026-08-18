//! plenora-engine — contratto del piano, planner, preparer ed executor del DAG
//! (Architetture.md par. 3.4).
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
//! [`plan`] — il formato piano v4 (DAG dichiarativo, `PlanLimits` in parsing,
//! migrazione dal piano lineare legacy, canonicalizzazione per il futuro
//! `plan_hash`) — e [`planner`] — la fase 1 `validate` (Architetture.md
//! par. 6.1, ADR 4/5): inferenza dei contratti arco per arco, identita' del
//! `ValidatedGraph` e verifica di compatibilita'. Fase 2A-4 aggiunge
//! [`prepare`] — la fase 2 `prepare` (Architetture.md par. 6.3, ADR 5):
//! `RuntimeContext`/`RuntimeStatistic`, `PreparedKernel` (E1), segmenti
//! fisici con `SegmentMode` (E2), last consumer (V10) — ed [`executor`] —
//! la fase 2 `execute` seriale a pull (V3/V4, V8): dispatch dei nodi sui
//! percorsi esistenti (`table_engine`, `geo_transport`), limiti effettivi,
//! validazione dinamica WKB in lettura (D8), metriche per nodo e per
//! segmento (E3) e scrittura IPC con publish atomico.
//!
//! ADR 5: l'API pubblica del DAG e' a due passi — [`planner::validate`] e
//! [`execute`]; `prepare` e' interna al crate (la strategia fisica e' un
//! dettaglio di `execute`). L'unica vista pubblica sul piano fisico e'
//! [`explain`], a secco, per l'ispezione (dry-run della CLI).
//!
//! Fase 2B aggiunge [`temp_store`] (ADR 3): store temporaneo isolato per
//! `execution_id` con lock file e heartbeat, piu' scavenging all'avvio delle
//! directory orfane — difesa strutturale contro i crash non intercettabili.
//!
//! Fase 2B M1a/M1b aggiunge [`governor`] (ADR-0002/ADR-0001): budget
//! memoria globale di piano `max_memory_bytes`, [`MemoryLease`] RAII
//! reference-counted con reservation a tre vie, e [`GovernedBatch`] con la
//! sequenza logica ai confini degli archi — i kernel restano su
//! `RecordBatch` puro.
//!
//! Fase 2B M1c/M1d aggiunge [`cancellation`] e gli errori arricchiti
//! (ADR 3): [`CancellationToken`] cooperativo osservato ai confini
//! dell'executor (mai dentro ai kernel in M1 — il passaggio e' M3) con
//! errore dedicato `PlenoraError::Cancelled`; `execution_id` per esecuzione
//! negli errori `Execution`/`Cancelled` e nel lock del `TempStore`;
//! `PlenoraError::category()` e gli assi §9 (`phase()`, `remote_effect()`,
//! `retry_disposition()` — R9.7, sostituisce il `retryable()` di M1d);
//! modalita' diagnostica opt-in
//! (`RuntimeContext::diagnostics`, contesto strutturale, mai valori).
//!
//! Fase 2B M2c aggiunge il wiring dello spill generalizzato (ADR-0002):
//! `table.sort`/`distinct`/`aggregate` attivano preventivamente la variante
//! `*_spilled` sopra la soglia stimata "byte input > `max_memory_bytes`";
//! i file di spill vivono nella directory condivisa del [`TempStore`]
//! dell'esecuzione e le [`SpillMetrics`] aggregate sono esposte in
//! [`executor::ExecutionMetrics`].

pub mod cancellation;
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
