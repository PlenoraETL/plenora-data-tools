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
///
/// # Perche' sotto `cfg`, e quando ne esce
///
/// Perche' non ha ancora un chiamante di produzione: il supervisore che la
/// chiama esiste, ma non e' attivato da nessuna policy. Lasciarla compilata in
/// produzione la farebbe risultare codice morto, e l'unico modo di zittire
/// quell'avviso senza toglierla sarebbe renderla pubblica — cioe' fingere
/// un'API che nessuno usa, che e' la scorciatoia che il registro vieta.
///
/// **Condizione di rientro:** il `cfg` cade quando il supervisore viene
/// **davvero attivato**, non quando qualcosa diventa `pub`. Perimetro e regola
/// stanno in
/// `errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals`.
#[cfg(any(test, feature = "internals"))]
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
// Il dominio di isolamento **e' compilato sempre**, perche' un pezzo di esso ha
// un chiamante di produzione: il dispatch anticipato dello spawner, qui sotto.
// Il binario spedito deve riconoscere la riga di comando dello spawner, o un
// worker avviato eseguirebbe il parser degli argomenti ordinario.
//
// Non tutto il modulo pero' e' raggiungibile da li'. Cio' che serve solo al
// **supervisore** — preparazione del dominio, token, transizione, avvio —
// resta sotto `cfg(any(test, feature = "internals"))` con la sua condizione di
// rientro scritta sui singoli elementi: cade quando esiste un supervisore che
// li chiama in produzione.
//
// Il frazionamento non e' pedanteria. Un `cfg` sul modulo intero dichiarerebbe
// una condizione falsa in un verso o nell'altro: o «niente ha un chiamante»,
// che il dispatch smentisce, o «tutto ce l'ha», che il preflight smentisce. E
// un `cfg` che dichiara il falso e' peggio di nessun `cfg`, perche' chi legge
// smette di controllare.
//
// Il `cfg` di piattaforma resta finche' non esiste un secondo dominio
// supportato, ed e' indipendente dall'altro: se fossero una condizione sola, la
// caduta della prima porterebbe via anche la seconda. L'orchestrazione e i suoi
// casi sono **multipiattaforma** — provano la procedura, non l'ambiente —
// quindi `cfg(target_os)` sta sui soli sottomoduli che toccano il kernel.
//
// Regola, perimetro e condizioni di rientro sono registrati in
// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
mod isolamento;

/// Se questo processo e' uno spawner, lo esegue e non torna.
///
/// # Dove va chiamata, e perche' proprio li'
///
/// **Per prima**, nel `main` del programma, prima di qualunque cosa crei un
/// thread. Il primo passo della sequenza dello spawner pretende un processo
/// monothread — le credenziali si cambiano per thread, e gli altri
/// resterebbero privilegiati — quindi un pool costruito prima di questa
/// chiamata renderebbe lo spawner impossibile. Compilando, e passando ogni
/// caso deterministico.
///
/// # Che cosa rende
///
/// [`DalConfine::AltroComando`] se `argv[1]` non e' la versione della
/// richiesta: il processo non e' uno spawner e il chiamante prosegue
/// normalmente.
///
/// [`DalConfine::Fallita`] se lo e' ma la sequenza non regge.
/// [`DalConfine::Conclusa`] non lo rende mai: la riuscita dello spawner e' una
/// `exec`, e dopo quella questo processo non esiste piu'.
#[cfg(target_os = "linux")]
#[must_use]
pub fn spawner_dal_confine(argomenti: &[std::ffi::OsString]) -> DalConfine {
    isolamento::dal_confine_se_spawner(argomenti)
}
/// Se questo processo e' un **worker**, lo porta fin dove il worker arriva.
///
/// # Dove va chiamata
///
/// Subito dopo [`spawner_dal_confine`], e come quella **prima di tutto il
/// resto**: sono due modalita' dello stesso eseguibile, scelte dal primo
/// argomento, e una riga del namespace riservato che arrivasse al parser della
/// CLI si sentirebbe rispondere «comando sconosciuto» invece della diagnosi
/// vera.
///
/// # Che cosa rende
///
/// [`DalConfine::AltroComando`] se `argv[1]` non e' del namespace del worker:
/// il processo non e' un worker e il chiamante prosegue normalmente.
///
/// [`DalConfine::Conclusa`] quando il worker ha percorso la sequenza fino
/// all'esito dichiarato — che **non** significa che l'esecuzione isolata sia
/// riuscita: significa che il worker ha detto com'e' andata, e chi giudica e'
/// il supervisore.
///
/// [`DalConfine::Fallita`] quando non c'e' stato modo di dirlo: il canale non
/// regge, oppure un canale non c'e' ancora.
#[cfg(target_os = "linux")]
#[must_use]
pub fn worker_dal_confine(argomenti: &[std::ffi::OsString]) -> DalConfine {
    isolamento::dal_confine_se_worker(argomenti)
}
// Il perimetro di qualificazione, che esiste solo quando `rustc` riceve
// `--cfg qualificazione_isolamento`.
//
// Non e' una feature, e la differenza sta in come si accende: una feature la si
// abilita dichiarandola fra le dipendenze, e l'unificazione la propaga anche a
// chi non l'ha chiesta, quindi una build di produzione potrebbe ritrovarsela
// addosso perche' un'altra cosa nell'albero l'ha voluta. Un `cfg` non si
// propaga: nessun crate dipendente puo' accenderlo.
//
// Cio' che non garantisce: chi controlla il comando di build lo puo' mettere in
// `RUSTFLAGS`. La garanzia e' che non ci si arrivi **per sbaglio**, non che non
// ci si possa arrivare.
//
// Che cosa espone: l'immagine che il gate ostile riesegue, e la giuntura con la
// barriera fra l'accertamento dell'immagine e lo `spawn`. Nessuna delle due
// deve poter essere raggiunta da codice che non sia quel gate.
#[cfg(all(target_os = "linux", qualificazione_isolamento, feature = "internals"))]
pub use isolamento::qualificazione;
#[cfg(target_os = "linux")]
pub use isolamento::DalConfine;
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
// Il `cfg` di perimetro non c'e' piu', e la condizione che lo regge e'
// scritta: chiede un chiamante **esterno** al modulo, e l'handshake che vive
// dentro non lo e'. Quel chiamante e' il worker, che si descrive, legge il
// saluto, giudica l'accordo e risponde — da codice di produzione, raggiunto dal
// dispatch della riga di comando.
//
// Cio' che dentro il modulo resta senza chiamante lo dichiara sui singoli
// elementi: un `cfg` sul modulo intero direbbe che nessuno lo usa, e non e'
// vero. Nessun `allow(dead_code)` in nessuno dei due casi — l'assenza di
// chiamante si dichiara, non si nasconde.
mod protocollo;
// Quale implementazione risolve i CRS in questa build, detto in un posto solo.
// Privato: e' una decisione interna, e la superficie pubblica non deve
// dipendere da quale backend c'e' sotto.
mod risolutore;
pub mod table_engine;
pub mod temp_store;
// Il verificatore dell'artefatto ha lo stesso perimetro del protocollo, e per
// la stessa ragione: **non ha ancora un chiamante di produzione**. Chi lo
// chiamera' e' la sequenza di verifica e publish, con `PR-10`; finche' non
// esiste, il modulo si
// compila dove qualcuno lo usa davvero — i test e la facciata `interni`, da
// cui il fuzzer lo raggiunge.
//
// Privato senza eccezioni: e' il verificatore di un percorso interno, e
// renderlo pubblico prima che il percorso esista sarebbe la promessa di non
// cambiarlo.
//
// Regola, perimetro e condizione di rientro sono registrati in
// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
#[cfg(any(test, feature = "internals"))]
mod verifica;

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
