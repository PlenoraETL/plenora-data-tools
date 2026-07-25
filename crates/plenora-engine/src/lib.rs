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

pub mod executor;
pub mod geo_transport;
pub mod plan;
pub mod planner;
pub mod prepare;
pub mod table_engine;

pub use executor::{execute, ExecutionMetrics, Input, Inputs, NodeMetrics, Output, SegmentMetrics};
pub use prepare::{
    prepare, AccessorKind, BatchTarget, ExecutionPlan, GeoRole, InputStatistics, LastConsumer,
    MeasureKind, MetricsConfig, ParallelismStrategy, PhysicalSegment, PreparedConfig,
    PreparedKernel, RuntimeContext, SegmentMode,
};
pub use table_engine::{execute_batch, execute_binary, Limits, Plan, Step, ValidatedPlan};
