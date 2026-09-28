//! plenora-engine — contratto del piano, planner, preparer ed executor del DAG
//! (architettura.md).
//!
//! - [`table_engine`] e [`geo_transport`]: i due percorsi a compatibilita'
//!   congelata (catena tabellare; trasporto `PLNGEO3`/`PLNGEO2`/`PLNPAIR1`).
//!   Formato sul filo, messaggi ed errori sono superficie compatibile.
//! - [`plan`], [`planner`], [`prepare`], [`executor`]: il DAG
//!   (architettura.md#planner-ed-executor, piano-v5.md). L'API pubblica e' a
//!   due passi, [`planner::validate`] e [`execute`]; `prepare` e' interna e
//!   [`explain`] e' l'unica vista, a secco, sul piano fisico.
//! - [`temp_store`], [`governor`], [`cancellation`]: store temporaneo per
//!   `execution_id`, budget memoria globale con [`MemoryLease`] e
//!   [`GovernedBatch`] (architettura.md#memoria), cancellazione cooperativa ai
//!   confini dell'executor (errori-e-limiti.md#cancellazione).
//!
//! Gli errori portano `execution_id`, categoria stabile e gli assi §9 (R9.7);
//! la modalita' diagnostica e' opt-in e non contiene mai valori.

pub mod cancellation;
/// Classificazione deterministica dell'esito di un worker isolato (§10 di
/// `isolamento.md`).
///
/// Logica pura e interna: il formato sul filo appartiene a `protocollo`. Il
/// chiamante di produzione e' `isolamento::macchina::conduci_isolato`.
mod classificazione;
// Privato come modulo: esce solo il tipo, con il `pub use` piu' sotto, cosi'
// non esistono due percorsi per la stessa cosa e le costanti interne restano
// interne.
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
// Compilato sempre: il dispatch anticipato dello spawner ha un chiamante di
// produzione, e il binario spedito deve riconoscere la riga di comando dello
// spawner. Cio' che resta senza chiamante lo dichiara un `cfg` sul singolo
// elemento, mai sul modulo; `cfg(target_os)` sta sui soli sottomoduli che
// toccano il kernel. Registro:
// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
//
// `pub` perche' `isolamento::attivazione` e `isolamento::esecuzione_isolata`
// sono la superficie che `plenora-cli` usa da fuori crate.
pub mod isolamento;

/// Se questo processo e' uno spawner, lo esegue e non torna.
///
/// Va chiamata **per prima** nel `main`, prima di creare qualunque thread: lo
/// spawner pretende un processo monothread, perche' le credenziali si cambiano
/// per thread.
///
/// Rende [`DalConfine::AltroComando`] se `argv[1]` non e' la versione della
/// richiesta, [`DalConfine::Fallita`] se la sequenza non regge; mai
/// [`DalConfine::Conclusa`], perche' la riuscita e' una `exec`.
#[cfg(target_os = "linux")]
#[must_use]
pub fn spawner_dal_confine(argomenti: &[std::ffi::OsString]) -> DalConfine {
    isolamento::dal_confine_se_spawner(argomenti)
}
/// Se questo processo e' un **worker**, lo porta fin dove il worker arriva.
///
/// Va chiamata subito dopo [`spawner_dal_confine`], prima del parser della
/// CLI: una riga del namespace riservato non deve arrivare al parser.
///
/// Rende [`DalConfine::AltroComando`] se `argv[1]` non e' del namespace del
/// worker; [`DalConfine::Conclusa`] quando il worker ha dichiarato l'esito (non
/// che l'esecuzione sia riuscita: giudica il supervisore);
/// [`DalConfine::Fallita`] quando non c'e' stato modo di dichiararlo.
#[cfg(target_os = "linux")]
#[must_use]
pub fn worker_dal_confine(argomenti: &[std::ffi::OsString]) -> DalConfine {
    isolamento::dal_confine_se_worker(argomenti)
}
/// Se questo processo e' un **verificatore**, lo porta fin dove arriva.
///
/// Va chiamata nello stesso punto di [`worker_dal_confine`]: e' una terza
/// modalita' dello stesso eseguibile, che non esegue un piano e non riceve mai
/// la destinazione finale.
///
/// Rende [`DalConfine::AltroComando`] se `argv[1]` non e' del namespace del
/// verificatore; [`DalConfine::Conclusa`] quando ha dichiarato l'esito del
/// confronto (non che l'artefatto sia valido: giudica il coordinatore);
/// [`DalConfine::Fallita`] quando non c'e' stato modo di dichiararlo.
#[cfg(target_os = "linux")]
#[must_use]
pub fn verificatore_dal_confine(argomenti: &[std::ffi::OsString]) -> DalConfine {
    isolamento::dal_confine_se_verificatore(argomenti)
}
// Il perimetro di qualificazione, presente solo con `--cfg
// qualificazione_isolamento`: un `cfg` non si propaga ai dipendenti come una
// feature, quindi non ci si arriva per sbaglio (chi controlla `RUSTFLAGS` ci
// arriva lo stesso). Espone l'immagine che il gate ostile riesegue e la
// giuntura fra accertamento dell'immagine e `spawn`, solo per quel gate.
#[cfg(all(target_os = "linux", qualificazione_isolamento, feature = "internals"))]
pub use isolamento::qualificazione;
#[cfg(target_os = "linux")]
pub use isolamento::DalConfine;
// Il percorso di qualificazione end-to-end: un worker reale, un canale reale,
// e un referto che dice **quale immagine** ha percorso la sequenza.
#[cfg(all(target_os = "linux", any(test, feature = "internals")))]
pub use isolamento::prova;
pub mod parallelism;
pub mod plan;
pub mod planner;
pub mod prepare;
// Sempre privato: e' un canale interno fra due processi spediti insieme, e
// renderlo pubblico, anche sotto feature, prometterebbe di non cambiarlo. Da
// fuori si passa da [`interni`]. Il chiamante esterno al modulo e' il worker;
// cio' che resta senza chiamante lo dichiara sui singoli elementi, senza
// `allow(dead_code)`.
mod protocollo;
// Le osservazioni su una destinazione, e il passo 9.
//
// Pubblico per `risolvi_commit`: chi non riceve risposta dal processo che
// pubblica deve poter guardare il disco senza passare da noi.
pub mod pubblicazione;
// Quale implementazione risolve i CRS in questa build, detto in un posto solo.
// Privato: e' una decisione interna, e la superficie pubblica non deve
// dipendere da quale backend c'e' sotto.
mod risolutore;
pub mod table_engine;
pub mod temp_store;
// Le fixture condivise dai test interni. L'`allow` copre `Inputs::with`, il
// percorso permissivo deprecato che le prove dell'executor esercitano ancora.
#[cfg(test)]
#[allow(deprecated)]
mod test_support;
// I passi da 3 a 8-bis e il passo 9 che ne consuma la prova. Chiamante di
// produzione: `isolamento::esecuzione_isolata::esegui_isolato`.
mod verifica;

pub use cancellation::CancellationToken;
pub use commit_token::{CommitToken, FormaTokenNonValida};
pub use executor::{execute, ExecutionMetrics, Input, Inputs, NodeMetrics, Output, SegmentMetrics};
pub use governor::{GovernedBatch, MemoryGovernor, MemoryLease, MemoryMetrics, ReservationResult};
pub use ipc_boundary::{BoundaryBatches, IpcFormat, IpcLimits};
pub use plenora_kernels_table::spill::SpillMetrics;
pub use prepare::{
    explain, AccessorKind, BatchTarget, Confinamento, ExecutionPlan, GeoRole, InputStatistics,
    LastConsumer, MeasureKind, MetricsConfig, ParallelismStrategy, PhysicalSegment, PreparedConfig,
    PreparedKernel, RuntimeContext, SegmentMode,
};
pub use table_engine::{
    execute_batch, execute_batch_with_spill, execute_binary, execute_complete_batch, Limits, Plan,
    Step, ValidatedPlan,
};
pub use temp_store::{scavenge_stale_temp_dirs, ScavengeReport, TempStore, DEFAULT_SCAVENGE_TTL};
