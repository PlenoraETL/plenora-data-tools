//! Store temporaneo condiviso per esecuzione e scavenging all'avvio
//! (ADR 3, "Crash non intercettabili").
//!
//! `catch_unwind` non copre `panic = "abort"`, crash nei backend nativi,
//! OOM killer e kill esterni: la difesa strutturale e' una directory
//! temporanea isolata per `execution_id` con un lock file come prova
//! principale di esecuzione viva.
//!
//! - [`TempStore`]: radice configurabile (default: temp di sistema),
//!   sotto-directory `plenora-<execution_id>-<random>/` con lock file
//!   `lock.json` (`execution_id`, PID, hostname, timestamp di creazione e di
//!   heartbeat). RAII: al `Drop` rimuove directory e lock.
//! - [`heartbeat`](TempStore::heartbeat): aggiorna il timestamp nel lock.
//!   Nella v1 seriale e' il chiamante a decidere quando invocarla (es. a
//!   ogni batch o a intervalli regolari): lo store non ha timer interni.
//!   PID, hostname e heartbeat sono segnali diagnostici, mai prove
//!   sufficienti (PID riutilizzabile; una macchina sospesa puo' rendere
//!   vecchio il timestamp senza che l'esecuzione sia orfana) — lo
//!   scavenging applica per questo un TTL conservativo.
//! - [`scavenge_stale_temp_dirs`]: da invocare all'avvio. Elimina solo
//!   directory `plenora-*` il cui lock indica un processo morto **oppure**
//!   un heartbeat piu' vecchio del TTL. Un lock vivo con heartbeat fresco
//!   non e' MAI toccato; qualunque voce fuori dal pattern `plenora-*` non
//!   e' MAI toccata (fail-safe totale).
//!
//! Verifica PID: solo su Linux, via `kill(pid, 0)` con rustix (dipendenza
//! gia' presente per `statfs` in `geo_transport::publish`). Su Windows e
//! sugli altri Unix, senza nuove dipendenze pesanti, il fallback
//! conservativo considera il processo vivo e decide solo il TTL
//! dell'heartbeat.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plenora_core::PlenoraError;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

/// Prefisso delle directory temporanee di esecuzione: e' l'unico pattern
/// che lo scavenging e' autorizzato a considerare (fail-safe).
const DIR_PREFIX: &str = "plenora-";

/// Nome del lock file dentro la directory di esecuzione.
const LOCK_FILE_NAME: &str = "lock.json";

/// TTL di default dello scavenging (24 ore): volutamente conservativo
/// (ADR 3 — una macchina sospesa/ibernata puo' congelare l'heartbeat di
/// un'esecuzione ancora valida).
pub const DEFAULT_SCAVENGE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Lunghezza massima accettata per un `execution_id` (va nel nome della
/// directory, quindi e' validato in modo restrittivo).
const MAX_EXECUTION_ID_LEN: usize = 128;

/// Contenuto del lock file `lock.json` (ADR 3): il lock file stesso e' la
/// prova principale (la directory esiste solo mentre l'esecuzione e' viva,
/// salvo crash); PID, hostname e timestamp sono segnali diagnostici.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockFile {
    execution_id: String,
    pid: u32,
    hostname: String,
    created_unix_secs: u64,
    heartbeat_unix_secs: u64,
}

/// Esito dello scavenging all'avvio (ADR 3): telemetria per il chiamante,
/// nessun valore o payload sensibile.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScavengeReport {
    /// Directory rimosse (processo morto o heartbeat scaduto).
    pub removed: Vec<PathBuf>,
    /// Directory con lock vivo e heartbeat fresco: mai toccate.
    pub kept_alive: usize,
    /// Directory non rimosse per prudenza (lock assente/corrotto ma non
    /// abbastanza vecchio, metadati illeggibili, errore di rimozione).
    pub kept_conservative: usize,
}

/// Store temporaneo condiviso per una singola esecuzione (ADR 3).
///
/// Creato all'avvio dell'esecuzione, ospita tutti i file temporanei (spill
/// e simili) sotto `plenora-<execution_id>-<random>/`. Il `Drop` rimuove
/// ricorsivamente directory e lock; dopo un crash non intercettabile la
/// directory resta e viene bonificata da [`scavenge_stale_temp_dirs`].
#[derive(Debug)]
pub struct TempStore {
    directory: TempDir,
    lock: LockFile,
}

impl TempStore {
    /// Crea lo store nella temp di sistema (`std::env::temp_dir`).
    ///
    /// # Errors
    /// Come [`TempStore::with_root`]; piu' `PlenoraError::Io` se la radice
    /// di default non e' scrivibile.
    pub fn new(execution_id: &str) -> Result<Self, PlenoraError> {
        Self::with_root(execution_id, &std::env::temp_dir())
    }

    /// Crea lo store sotto `root`: directory `plenora-<execution_id>-<random>/`
    /// e lock file `lock.json` con `execution_id`, PID, hostname e timestamp di
    /// creazione/heartbeat.
    ///
    /// # Errors
    /// Restituisce `PlenoraError::Contract` se `execution_id` e' vuoto, troppo
    /// lungo o contiene caratteri fuori da `[A-Za-z0-9._-]` (finisce nel nome
    /// della directory: validazione restrittiva fail-closed);
    /// `PlenoraError::Io` per i fallimenti di creazione di directory e lock.
    pub fn with_root(execution_id: &str, root: &Path) -> Result<Self, PlenoraError> {
        validate_execution_id(execution_id)?;
        let directory = tempfile::Builder::new()
            .prefix(&format!("{DIR_PREFIX}{execution_id}-"))
            .tempdir_in(root)?;
        let lock = LockFile {
            execution_id: execution_id.to_owned(),
            pid: std::process::id(),
            hostname: hostname(),
            created_unix_secs: now_unix_secs(),
            heartbeat_unix_secs: now_unix_secs(),
        };
        write_lock(&directory.path().join(LOCK_FILE_NAME), &lock)?;
        Ok(Self { directory, lock })
    }

    /// Percorso della directory temporanea dell'esecuzione.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    /// `execution_id` associato allo store.
    #[must_use]
    pub fn execution_id(&self) -> &str {
        &self.lock.execution_id
    }

    /// Aggiorna il timestamp di heartbeat nel lock file (ADR 3).
    ///
    /// Nella v1 seriale la periodicita' e' decisa dal chiamante: invocarla a
    /// intervalli ben piu' brevi del TTL di scavenging (es. a ogni batch) —
    /// lo store non ha timer interni. La scrittura non e' atomica: un crash
    /// a meta' scrittura produce un lock corrotto, che lo scavenging tratta
    /// in modo conservativo (vedi [`scavenge_stale_temp_dirs`]).
    ///
    /// # Errors
    /// Restituisce `PlenoraError::Io` se la riscrittura del lock fallisce.
    pub fn heartbeat(&mut self) -> Result<(), PlenoraError> {
        self.lock.heartbeat_unix_secs = now_unix_secs();
        write_lock(&self.directory.path().join(LOCK_FILE_NAME), &self.lock)
    }
}

/// Scavenging all'avvio delle directory temporanee orfane (ADR 3).
///
/// Elenca le voci `plenora-*` in `root` (SOLO directory, SOLO con quel
/// prefisso: qualunque altra voce e' ignorata e mai toccata) e per ognuna
/// legge il lock:
///
/// - PID non piu' esistente sulla macchina (verifica `kill(pid, 0)` solo su
///   Linux) **oppure** heartbeat piu' vecchio di `ttl` → directory e lock
///   cancellati;
/// - lock vivo con heartbeat fresco → mai toccato;
/// - lock assente o corrotto → conservativo: cancellato solo se piu' vecchio
///   di `ttl * 2` (mtime del lock file, o della directory se il lock manca),
///   altrimenti lasciato in pace. Razionale: un crash a meta' scrittura del
///   lock non deve rendere la directory immortale, ma la cancellazione resta
///   subordinata a un margine doppio del TTL.
///
/// Gli errori sulle singole voci non interrompono il giro: sono conteggiati
/// in [`ScavengeReport::kept_conservative`].
///
/// # Errors
/// Restituisce `PlenoraError::Io` se `root` non e' elencabile.
pub fn scavenge_stale_temp_dirs(
    root: &Path,
    ttl: Duration,
) -> Result<ScavengeReport, PlenoraError> {
    let mut report = ScavengeReport::default();
    let now = now_unix_secs();
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else {
            report.kept_conservative += 1;
            continue;
        };
        // Fail-safe totale: solo directory il cui nome inizia con `plenora-`.
        let Ok(file_type) = entry.file_type() else {
            report.kept_conservative += 1;
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(DIR_PREFIX) {
            continue;
        }
        match classify_temp_dir(&entry.path(), ttl, now) {
            ScavengeAction::Remove => match fs::remove_dir_all(entry.path()) {
                Ok(()) => report.removed.push(entry.path()),
                Err(_) => report.kept_conservative += 1,
            },
            ScavengeAction::KeepAlive => report.kept_alive += 1,
            ScavengeAction::KeepConservative => report.kept_conservative += 1,
        }
    }
    Ok(report)
}

/// Decisione dello scavenging su una singola directory `plenora-*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScavengeAction {
    /// Processo morto o heartbeat scaduto: cancellare.
    Remove,
    /// Lock vivo e heartbeat fresco: mai toccare.
    KeepAlive,
    /// Lock assente/corrotto non abbastanza vecchio: prudenza.
    KeepConservative,
}

/// Classificazione pura di una directory `plenora-*` (ADR 3): la prova
/// principale e' il lock file; PID e heartbeat sono segnali diagnostici.
fn classify_temp_dir(path: &Path, ttl: Duration, now: u64) -> ScavengeAction {
    let lock_path = path.join(LOCK_FILE_NAME);
    let lock = fs::read(&lock_path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<LockFile>(&raw).ok());
    if let Some(lock) = lock {
        if !process_alive(lock.pid) {
            return ScavengeAction::Remove;
        }
        // `saturating_sub`: un heartbeat nel futuro (clock skew) conta
        // come fresco, mai come scaduto.
        if now.saturating_sub(lock.heartbeat_unix_secs) > ttl.as_secs() {
            return ScavengeAction::Remove;
        }
        return ScavengeAction::KeepAlive;
    }
    // Lock assente o corrotto: conservativo, cancella solo oltre
    // TTL*2 misurato sul mtime (del lock se esiste, della directory
    // altrimenti). Metadati illeggibili → mai cancellare.
    let mtime = fs::metadata(&lock_path)
        .or_else(|_| fs::metadata(path))
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok());
    match mtime {
        Some(mtime_secs) if now.saturating_sub(mtime_secs.as_secs()) > ttl.as_secs() * 2 => {
            ScavengeAction::Remove
        }
        _ => ScavengeAction::KeepConservative,
    }
}

/// Verifica `kill(pid, 0)` (solo Linux, via rustix — dipendenza gia'
/// presente per `statfs`): `ESRCH` = processo inesistente; `EPERM` = esiste
/// ma non e' nostro, quindi vivo. Il PID resta un segnale diagnostico
/// (riutilizzabile), mai una prova sufficiente (ADR 3).
#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(errno) => errno != rustix::io::Errno::SRCH,
    }
}

/// Windows e altri Unix: nessuna verifica PID portabile senza nuove
/// dipendenze. Fallback conservativo: il processo e' considerato vivo e
/// decide solo il TTL dell'heartbeat (ADR 3).
#[cfg(not(target_os = "linux"))]
const fn process_alive(_pid: u32) -> bool {
    true
}

/// Validazione fail-closed dell'`execution_id`: finisce nel nome della
/// directory, quindi solo `[A-Za-z0-9._-]` entro una lunghezza massima;
/// i soli punti (`.`/`..`/...) sono rifiutati (segmenti di percorso).
fn validate_execution_id(execution_id: &str) -> Result<(), PlenoraError> {
    let valid = !execution_id.is_empty()
        && execution_id.len() <= MAX_EXECUTION_ID_LEN
        && execution_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && execution_id.bytes().any(|byte| byte != b'.');
    if valid {
        return Ok(());
    }
    Err(PlenoraError::Contract(format!(
        "execution_id non valido per la directory temporanea (solo [A-Za-z0-9._-], \
         max {MAX_EXECUTION_ID_LEN} caratteri): {execution_id:?}"
    )))
}

/// Scrittura del lock file (non atomica per scelta, vedi
/// [`TempStore::heartbeat`]: un lock corrotto e' gestito in modo
/// conservativo dallo scavenging).
fn write_lock(path: &Path, lock: &LockFile) -> Result<(), PlenoraError> {
    let raw = serde_json::to_vec_pretty(lock)?;
    fs::write(path, raw)?;
    Ok(())
}

/// Timestamp corrente in secondi Unix; 0 se l'orologio e' prima dell'epoca
/// (orologio rotto: i confronti altrove usano `saturating_sub`, quindi un
/// timestamp anomalo non puo' rendere "scaduto" un heartbeat fresco).
fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Hostname diagnostico (ADR 3: segnale, mai prova). Solo variabili
/// d'ambiente (`HOSTNAME` su Unix, `COMPUTERNAME` su Windows): nessuna
/// nuova dipendenza; se assenti, `unknown`.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Legge e parsa il lock file di uno store.
    fn read_lock(store: &TempStore) -> LockFile {
        let raw = fs::read(store.path().join(LOCK_FILE_NAME)).expect("lock presente");
        serde_json::from_slice(&raw).expect("lock valido")
    }

    /// Scrive un lock arbitrario in una directory temporanea esistente.
    fn plant_lock(dir: &Path, lock: &LockFile) {
        fs::write(
            dir.join(LOCK_FILE_NAME),
            serde_json::to_vec(lock).expect("serializzazione"),
        )
        .expect("scrittura lock");
    }

    fn sample_lock(pid: u32, heartbeat_unix_secs: u64) -> LockFile {
        LockFile {
            execution_id: "exec-test".to_owned(),
            pid,
            hostname: "test-host".to_owned(),
            created_unix_secs: heartbeat_unix_secs,
            heartbeat_unix_secs,
        }
    }

    // -- Creazione / heartbeat / Drop ---------------------------------------

    #[test]
    fn creation_writes_lock_with_expected_fields() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-42.A_b", root.path()).expect("store");
        // Directory isolata sotto la radice con il pattern atteso.
        assert_eq!(store.path().parent(), Some(root.path()));
        let name = store
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .expect("nome directory");
        assert!(name.starts_with("plenora-exec-42.A_b-"), "nome: {name}");
        // Lock con execution_id, PID e timestamp coerenti.
        let lock = read_lock(&store);
        assert_eq!(lock.execution_id, "exec-42.A_b");
        assert_eq!(lock.pid, std::process::id());
        assert_eq!(store.execution_id(), "exec-42.A_b");
        let now = now_unix_secs();
        assert!(now.saturating_sub(lock.created_unix_secs) <= 5);
        assert!(now.saturating_sub(lock.heartbeat_unix_secs) <= 5);
    }

    #[test]
    fn invalid_execution_ids_are_rejected() {
        let root = tempfile::tempdir().expect("root");
        for bad in ["", "a/b", "a\\b", "..", "a b", "a\0b"] {
            assert!(
                matches!(
                    TempStore::with_root(bad, root.path()),
                    Err(PlenoraError::Contract(_))
                ),
                "id atteso come rifiutato: {bad:?}"
            );
        }
        let too_long = "x".repeat(MAX_EXECUTION_ID_LEN + 1);
        assert!(TempStore::with_root(&too_long, root.path()).is_err());
    }

    #[test]
    fn heartbeat_updates_lock_timestamp() {
        let root = tempfile::tempdir().expect("root");
        let mut store = TempStore::with_root("exec-hb", root.path()).expect("store");
        // Invecchia artificialmente il lock, poi heartbeat: il timestamp
        // torna fresco.
        let mut lock = read_lock(&store);
        lock.heartbeat_unix_secs = 1;
        plant_lock(store.path(), &lock);
        store.heartbeat().expect("heartbeat");
        let lock = read_lock(&store);
        assert!(now_unix_secs().saturating_sub(lock.heartbeat_unix_secs) <= 5);
    }

    #[test]
    fn drop_removes_directory_and_lock() {
        let root = tempfile::tempdir().expect("root");
        let path;
        {
            let store = TempStore::with_root("exec-drop", root.path()).expect("store");
            path = store.path().to_owned();
            assert!(path.join(LOCK_FILE_NAME).is_file());
        }
        assert!(!path.exists(), "il Drop rimuove directory e lock");
    }

    // -- Scavenging -----------------------------------------------------------

    #[test]
    fn live_lock_is_never_scavenged() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-alive", root.path()).expect("store");
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(1)).expect("scavenge");
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_alive, 1);
        assert!(store.path().exists(), "lock vivo: mai toccare");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dead_pid_is_scavenged() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-dead", root.path()).expect("store");
        // Trova un PID inesistente scansionando all'indietro dal massimo.
        let dead_pid = (2..=4_000_000_u32)
            .rev()
            .find(|&pid| !process_alive(pid))
            .expect("un pid libero esiste");
        let lock = sample_lock(dead_pid, now_unix_secs());
        plant_lock(store.path(), &lock);
        // TTL generoso: e' il PID morto a decidere, non l'heartbeat.
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(86_400)).expect("scavenge");
        assert_eq!(report.removed.len(), 1);
        assert!(!store.path().exists(), "processo morto: directory rimossa");
        std::mem::forget(store); // la directory e' gia' stata rimossa
    }

    #[test]
    fn stale_heartbeat_is_scavenged() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-stale", root.path()).expect("store");
        // PID vivo (noi stessi) ma heartbeat antico: il TTL decide (ADR 3).
        let lock = sample_lock(std::process::id(), 1_000_000);
        plant_lock(store.path(), &lock);
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(60)).expect("scavenge");
        assert_eq!(report.removed.len(), 1);
        assert!(!store.path().exists());
        std::mem::forget(store);
    }

    #[test]
    fn fresh_heartbeat_with_live_pid_survives() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-fresh", root.path()).expect("store");
        let lock = sample_lock(std::process::id(), now_unix_secs());
        plant_lock(store.path(), &lock);
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(86_400)).expect("scavenge");
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_alive, 1);
        assert!(store.path().exists());
    }

    #[test]
    fn corrupt_lock_is_conservative_until_double_ttl() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-corrupt", root.path()).expect("store");
        let lock_path = store.path().join(LOCK_FILE_NAME);
        fs::write(&lock_path, b"{ non-json !!!").expect("lock corrotto");
        // Corrotto e recente: mai cancellare.
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(60)).expect("scavenge");
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_conservative, 1);
        assert!(store.path().exists());
        // Corrotto e piu' vecchio di TTL*2 (mtime del lock): cancellare.
        let old = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&lock_path)
            .expect("apertura lock")
            .set_modified(old)
            .expect("set mtime");
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(60)).expect("scavenge");
        assert_eq!(report.removed.len(), 1);
        assert!(!store.path().exists());
        std::mem::forget(store);
    }

    #[test]
    fn non_plenora_entries_are_never_touched() {
        let root = tempfile::tempdir().expect("root");
        // Directory fuori pattern: mai considerata.
        let other_dir = root.path().join("altra-roba");
        fs::create_dir(&other_dir).expect("mkdir");
        // File con nome nel pattern: non e' una directory, mai toccato.
        let stray_file = root.path().join("plenora-file-strano");
        fs::write(&stray_file, b"x").expect("file");
        // Directory nel pattern ma senza lock e recente: conservativa.
        let no_lock_dir = root.path().join("plenora-senza-lock-xyz");
        fs::create_dir(&no_lock_dir).expect("mkdir");
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(1)).expect("scavenge");
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_alive, 0);
        assert_eq!(report.kept_conservative, 1);
        assert!(other_dir.exists());
        assert!(stray_file.exists());
        assert!(no_lock_dir.exists());
    }

    #[test]
    fn concurrent_stores_do_not_disturb_each_other() {
        let root = tempfile::tempdir().expect("root");
        let mut first = TempStore::with_root("exec-a", root.path()).expect("primo store");
        let second = TempStore::with_root("exec-a", root.path()).expect("secondo store");
        assert_ne!(first.path(), second.path(), "suffisso random distinto");
        // Heartbeat e scavenge sull'uno non toccano l'altro.
        first.heartbeat().expect("heartbeat");
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(60)).expect("scavenge");
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_alive, 2);
        assert!(first.path().exists());
        assert!(second.path().exists());
        // Il Drop dell'uno lascia intatta la directory dell'altro.
        let second_path = second.path().to_owned();
        drop(first);
        assert!(second_path.exists());
        assert!(second_path.join(LOCK_FILE_NAME).is_file());
    }
}
