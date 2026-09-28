//! Store temporaneo condiviso per esecuzione e scavenging all'avvio
//! (errori-e-limiti.md, "Crash non intercettabili").
//!
//! `catch_unwind` non copre `panic = "abort"`, crash nei backend nativi, OOM
//! killer e kill esterni: la difesa strutturale e' una directory temporanea
//! isolata per `execution_id`, con un lock file (`lock.json`) il cui
//! heartbeat e' la prova principale di esecuzione viva. Il chiamante decide
//! quando invocare [`heartbeat`](TempStore::heartbeat): lo store non ha timer.
//!
//! [`scavenge_stale_temp_dirs`] tocca solo directory `plenora-*`. Comanda
//! l'heartbeat: un heartbeat fresco non si tocca mai, qualunque cosa dica il
//! PID. PID e hostname sono segnali, non prove (PID riutilizzabili, hostname
//! uguali fra immagini clonate e container): il PID puo' solo accelerare la
//! bonifica di un lock gia' fermo. Decisione e rimozione non sono atomiche; il
//! residuo e' in errori-e-limiti.md.
//!
//! Il PID si interroga solo su Linux (`kill(pid, 0)` con rustix); altrove il
//! fallback considera il processo vivo e decide il solo TTL.

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
/// (errori-e-limiti.md — una macchina sospesa/ibernata puo' congelare l'heartbeat di
/// un'esecuzione ancora valida).
pub const DEFAULT_SCAVENGE_TTL: Duration = Duration::from_hours(24);

/// Lunghezza massima accettata per un `execution_id` (va nel nome della
/// directory, quindi e' validato in modo restrittivo).
const MAX_EXECUTION_ID_LEN: usize = 128;

/// Eta' minima dell'heartbeat perche' il PID registrato conti qualcosa.
///
/// L'executor scrive il lock ogni secondo al piu': la grazia sta molto sopra
/// quella cadenza, per coprire pause di I/O e macchine sotto carico, e molto
/// sotto il TTL di default, per bonificare in fretta dopo un crash.
const GRAZIA_PID: Duration = Duration::from_secs(300);

/// Contenuto del lock file `lock.json` (errori-e-limiti.md): il lock file stesso e' la
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

/// Esito dello scavenging all'avvio (errori-e-limiti.md): telemetria per il chiamante,
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

/// Store temporaneo condiviso per una singola esecuzione (errori-e-limiti.md).
///
/// Creato all'avvio dell'esecuzione, ospita tutti i file temporanei (spill
/// e simili) sotto `plenora-<execution_id>-<random>/`. Il `Drop` rimuove
/// ricorsivamente directory e lock; dopo un crash non intercettabile la
/// directory resta, e [`scavenge_stale_temp_dirs`] la bonifica se chi lo esegue
/// ha il permesso di rimuoverla.
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
    /// e lock file `lock.json`.
    ///
    /// # Errors
    /// Restituisce `PlenoraError::InvalidPlan` se `execution_id` e' vuoto, troppo
    /// lungo o contiene caratteri fuori da `[A-Za-z0-9._-]` (finisce nel nome
    /// della directory); `PlenoraError::Io` per i fallimenti di creazione di
    /// directory e lock.
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

    /// Aggiorna il timestamp di heartbeat nel lock file (errori-e-limiti.md).
    ///
    /// Va invocata a intervalli ben piu' brevi del TTL di scavenging. La
    /// scrittura non e' atomica: un lock corrotto da un crash e' trattato in
    /// modo conservativo da [`scavenge_stale_temp_dirs`].
    ///
    /// # Errors
    /// Restituisce `PlenoraError::Io` se la riscrittura del lock fallisce.
    pub fn heartbeat(&mut self) -> Result<(), PlenoraError> {
        self.lock.heartbeat_unix_secs = now_unix_secs();
        write_lock(&self.directory.path().join(LOCK_FILE_NAME), &self.lock)
    }
}

/// Scavenging all'avvio delle directory temporanee orfane (errori-e-limiti.md).
///
/// Esamina solo le directory `plenora-*` in `root` e ne legge il lock, con i
/// criteri del doc di modulo. Un lock assente o corrotto (crash a meta'
/// scrittura) si cancella solo oltre `ttl * 2`, misurato sull'mtime del lock
/// o della directory. Gli errori sulle singole voci non interrompono il giro:
/// sono conteggiati in [`ScavengeReport::kept_conservative`].
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
            ScavengeAction::Remove => {
                // Riclassificazione immediata, con l'orologio riletto:
                // durante la scansione l'esecuzione proprietaria puo' aver
                // rinnovato l'heartbeat. Resta la finestra fra controllo e
                // `remove_dir_all`, un TOCTOU dichiarato in errori-e-limiti.md;
                // chiuderla richiede una lease interprocesso.
                if classify_temp_dir(&entry.path(), ttl, now_unix_secs()) == ScavengeAction::Remove
                {
                    match fs::remove_dir_all(entry.path()) {
                        Ok(()) => report.removed.push(entry.path()),
                        Err(_) => report.kept_conservative += 1,
                    }
                } else {
                    report.kept_conservative += 1;
                }
            }
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

/// Classificazione pura di una directory `plenora-*` (errori-e-limiti.md): la prova
/// principale e' il lock file; PID e heartbeat sono segnali diagnostici.
fn classify_temp_dir(path: &Path, ttl: Duration, now: u64) -> ScavengeAction {
    let lock_path = path.join(LOCK_FILE_NAME);
    // Solo un lock che **non esiste** e' assente. Uno che c'e' e non si lascia
    // leggere non dice niente sull'esecuzione che lo possiede, e la regola del
    // lock assente — cancellare oltre TTL*2 — cancellerebbe forse uno store
    // vivo: si tiene, sempre.
    let lock = match fs::read(&lock_path) {
        Ok(raw) => serde_json::from_slice::<LockFile>(&raw).ok(),
        Err(errore) if errore.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return ScavengeAction::KeepConservative,
    };
    if let Some(lock) = lock {
        // L'HEARTBEAT COMANDA, il PID puo' solo accelerare.
        //
        // `saturating_sub`: un heartbeat nel futuro (clock skew) conta come
        // fresco, mai come scaduto.
        let eta_heartbeat = now.saturating_sub(lock.heartbeat_unix_secs);
        // Un processo locale ancora vivo non si cancella nemmeno per TTL:
        // l'heartbeat si scrive ai confini di batch, e I/O bloccato,
        // ibernazione o salti d'orologio possono invecchiarlo mentre
        // l'esecuzione e' viva. Vale solo dove il PID e' interrogabile (Linux):
        // altrove `process_alive` risponde «vivo» per prudenza e bloccherebbe
        // ogni bonifica.
        let locale_e_vivo =
            PID_VERIFICABILE && hostname_confrontabile(&lock.hostname) && process_alive(lock.pid);
        if eta_heartbeat > ttl.as_secs() {
            if locale_e_vivo {
                return ScavengeAction::KeepConservative;
            }
            return ScavengeAction::Remove;
        }
        // Da qui l'heartbeat e' dentro il TTL. Il PID accelera la bonifica
        // solo con due prove concordi: il lock viene da questa macchina
        // (prova debole da sola) e l'heartbeat e' fermo da piu' di
        // [`GRAZIA_PID`]. Su un heartbeat fresco il PID non si interroga: un
        // hostname omonimo su una `temp_root` condivisa basterebbe a
        // cancellare un'esecuzione viva altrove.
        if eta_heartbeat > GRAZIA_PID.as_secs()
            && hostname_confrontabile(&lock.hostname)
            && !process_alive(lock.pid)
        {
            return ScavengeAction::Remove;
        }
        return ScavengeAction::KeepAlive;
    }
    // Lock assente o corrotto: conservativo, cancella solo oltre
    // TTL*2 misurato sul mtime (del lock se esiste, della directory
    // altrimenti). Metadati illeggibili → mai cancellare.
    // Il mtime della directory sostituisce quello del lock solo se il lock
    // **non esiste**: un lock che c'e' e non si lascia misurare non si
    // rimpiazza con un altro orologio, e la directory si tiene.
    let mtime = match fs::metadata(&lock_path) {
        Ok(metadata) => Ok(metadata),
        Err(errore) if errore.kind() == std::io::ErrorKind::NotFound => fs::metadata(path),
        Err(errore) => Err(errore),
    }
    .and_then(|metadata| metadata.modified())
    .ok()
    .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok());
    match mtime {
        // `saturating_mul`: un TTL enorme non deve avvolgere la soglia e
        // trasformare «non abbastanza vecchio» in «da cancellare». Saturando,
        // la soglia resta il massimo esprimibile e la directory viene tenuta —
        // la direzione prudente per uno scavenger che cancella file.
        Some(mtime_secs)
            if now.saturating_sub(mtime_secs.as_secs()) > ttl.as_secs().saturating_mul(2) =>
        {
            ScavengeAction::Remove
        }
        _ => ScavengeAction::KeepConservative,
    }
}

/// Hostname sconosciuto: `hostname()` non ha potuto leggerlo dall'ambiente.
/// Due lock con questo valore non dicono di essere sulla stessa macchina,
/// dicono che nessuno dei due sa dove si trova.
const HOSTNAME_SCONOSCIUTO: &str = "unknown";

/// `true` se il lock e' stato scritto su questa stessa macchina, cioe' se il
/// suo PID e' interpretabile localmente.
///
/// Fail-safe: con un hostname sconosciuto da una delle due parti la risposta
/// e' `false`, e la decisione ricade sul solo TTL dell'heartbeat.
fn hostname_confrontabile(registrato: &str) -> bool {
    let locale = hostname();
    registrato != HOSTNAME_SCONOSCIUTO && locale != HOSTNAME_SCONOSCIUTO && registrato == locale
}

/// Verifica `kill(pid, 0)` (solo Linux, via rustix — dipendenza gia'
/// presente per `statfs`): `ESRCH` = processo inesistente; `EPERM` = esiste
/// ma non e' nostro, quindi vivo. Il PID resta un segnale diagnostico
/// (riutilizzabile), mai una prova sufficiente (errori-e-limiti.md).
#[cfg(target_os = "linux")]
pub(crate) fn process_alive(pid: u32) -> bool {
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
/// decide solo il TTL dell'heartbeat (errori-e-limiti.md).
#[cfg(not(target_os = "linux"))]
const fn process_alive(_pid: u32) -> bool {
    true
}

/// `true` dove [`process_alive`] interroga davvero il sistema operativo.
///
/// Serve a distinguere «il processo risulta vivo» da «non sappiamo dirlo».
/// Solo la prima e' una prova, e solo la prima puo' impedire una rimozione
/// per TTL: altrove il fallback conservativo risponde sempre «vivo» e
/// bloccherebbe ogni bonifica.
#[cfg(target_os = "linux")]
const PID_VERIFICABILE: bool = true;

/// Vedi [`PID_VERIFICABILE`].
#[cfg(not(target_os = "linux"))]
const PID_VERIFICABILE: bool = false;

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
    Err(PlenoraError::InvalidPlan(format!(
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

/// Hostname della macchina (errori-e-limiti.md: segnale, mai prova).
///
/// Su Linux, l'unica piattaforma dove [`process_alive`] interroga il PID, si
/// legge `/proc/sys/kernel/hostname`; altrove si ripiega sulle variabili
/// d'ambiente.
fn hostname() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(nome) = fs::read_to_string("/proc/sys/kernel/hostname") {
        let nome = nome.trim();
        if !nome.is_empty() {
            return nome.to_owned();
        }
    }
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|nome| !nome.is_empty())
        .unwrap_or_else(|| HOSTNAME_SCONOSCIUTO.to_owned())
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

    /// Lock scritto da QUESTA macchina: il PID e' quindi interpretabile.
    fn sample_lock(pid: u32, heartbeat_unix_secs: u64) -> LockFile {
        LockFile {
            execution_id: "exec-test".to_owned(),
            pid,
            hostname: hostname(),
            created_unix_secs: heartbeat_unix_secs,
            heartbeat_unix_secs,
        }
    }

    /// Lock scritto da un'ALTRA macchina (radice temporanea condivisa): il
    /// PID non e' interpretabile qui.
    fn foreign_lock(pid: u32, heartbeat_unix_secs: u64) -> LockFile {
        LockFile {
            hostname: format!("{}-altro-host", hostname()),
            ..sample_lock(pid, heartbeat_unix_secs)
        }
    }

    /// Una radice temporanea e uno store con l'id dato. Il chiamante le lega
    /// come `(root, store)`: lo store si rilascia prima della radice.
    fn radice_e_store(execution_id: &str) -> (tempfile::TempDir, TempStore) {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root(execution_id, root.path()).expect("store");
        (root, store)
    }

    /// Lo scavenging della radice con il TTL dato.
    fn scava(root: &tempfile::TempDir, ttl: Duration) -> ScavengeReport {
        scavenge_stale_temp_dirs(root.path(), ttl).expect("scavenge")
    }

    // -- Creazione / heartbeat / Drop ---------------------------------------

    #[test]
    fn creation_writes_lock_with_expected_fields() {
        let (root, store) = radice_e_store("exec-42.A_b");
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
                    Err(PlenoraError::InvalidPlan(_))
                ),
                "id atteso come rifiutato: {bad:?}"
            );
        }
        let too_long = "x".repeat(MAX_EXECUTION_ID_LEN + 1);
        assert!(TempStore::with_root(&too_long, root.path()).is_err());
    }

    #[test]
    fn heartbeat_updates_lock_timestamp() {
        let (_root, mut store) = radice_e_store("exec-hb");
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
        assert!(
            !path.try_exists().expect("stat"),
            "il Drop rimuove directory e lock"
        );
    }

    // -- Scavenging -----------------------------------------------------------

    #[test]
    fn live_lock_is_never_scavenged() {
        let (root, store) = radice_e_store("exec-alive");
        let report = scava(&root, Duration::from_secs(1));
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_alive, 1);
        assert!(
            store.path().try_exists().expect("stat"),
            "lock vivo: mai toccare"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dead_pid_is_scavenged() {
        let (root, store) = radice_e_store("exec-dead");
        // Trova un PID inesistente scansionando all'indietro dal massimo.
        let dead_pid = pid_non_esistente();
        // Heartbeat fermo da oltre la grazia ma ben dentro il TTL: e' il PID
        // morto ad accelerare la bonifica, non la scadenza.
        let lock = sample_lock(dead_pid, now_unix_secs() - GRAZIA_PID.as_secs() - 60);
        plant_lock(store.path(), &lock);
        let report = scava(&root, Duration::from_hours(24));
        assert_eq!(report.removed.len(), 1);
        assert!(
            !store.path().try_exists().expect("stat"),
            "processo morto: directory rimossa"
        );
        std::mem::forget(store); // la directory e' gia' stata rimossa
    }

    /// Radice temporanea condivisa fra host: il PID registrato appartiene a
    /// un'altra macchina e qui non esiste. Interrogarlo localmente
    /// significherebbe cancellare la directory di un'esecuzione viva.
    /// L'heartbeat e' oltre la grazia — cioe' l'unica cosa che trattiene la
    /// rimozione e' il confronto fra host.
    #[cfg(target_os = "linux")]
    #[test]
    fn foreign_host_pid_is_never_trusted() {
        let (root, store) = radice_e_store("exec-foreign");
        let dead_pid = pid_non_esistente();
        let vecchio = now_unix_secs() - GRAZIA_PID.as_secs() - 60;
        plant_lock(store.path(), &foreign_lock(dead_pid, vecchio));
        let report = scava(&root, Duration::from_hours(24));
        assert!(
            report.removed.is_empty(),
            "il PID di un altro host non decide: {report:?}"
        );
        assert_eq!(report.kept_alive, 1);
        assert!(store.path().try_exists().expect("stat"));
    }

    /// Il caso che l'uguaglianza di hostname da sola non copre: due macchine
    /// con lo STESSO nome (immagini clonate, container) e una radice
    /// condivisa. Il PID dell'esecuzione remota non esiste qui, ma il suo
    /// heartbeat e' fresco: e' una prova positiva di vita e vince sul PID.
    #[cfg(target_os = "linux")]
    #[test]
    fn un_heartbeat_fresco_batte_sempre_il_pid() {
        let (root, store) = radice_e_store("exec-omonimo");
        let dead_pid = pid_non_esistente();
        // Stesso hostname (il lock dice di venire da qui) ma PID inesistente
        // e heartbeat appena scritto.
        plant_lock(store.path(), &sample_lock(dead_pid, now_unix_secs()));
        let report = scava(&root, Duration::from_hours(24));
        assert!(
            report.removed.is_empty(),
            "un heartbeat fresco non puo' essere cancellato da un PID: {report:?}"
        );
        assert_eq!(report.kept_alive, 1);
        assert!(store.path().try_exists().expect("stat"));
    }

    /// Lo stesso lock di un altro host, ma con heartbeat oltre il TTL: qui
    /// decide il TTL, che resta valido fra host, e la directory va rimossa.
    #[test]
    fn foreign_host_still_obeys_the_ttl() {
        let (root, store) = radice_e_store("exec-foreign-stale");
        plant_lock(store.path(), &foreign_lock(std::process::id(), 1_000_000));
        let report = scava(&root, Duration::from_secs(60));
        assert_eq!(report.removed.len(), 1);
        assert!(!store.path().try_exists().expect("stat"));
        std::mem::forget(store);
    }

    /// L'hostname deve essere una risposta utile: se fosse sempre
    /// `unknown` il confronto fra host non distinguerebbe nulla e il PID
    /// tornerebbe a decidere ovunque.
    #[cfg(target_os = "linux")]
    #[test]
    fn hostname_is_resolvable_on_linux() {
        assert_ne!(hostname(), HOSTNAME_SCONOSCIUTO);
        assert!(hostname_confrontabile(&hostname()));
        assert!(!hostname_confrontabile(&format!("{}-altro", hostname())));
        assert!(!hostname_confrontabile(HOSTNAME_SCONOSCIUTO));
    }

    /// PID che sulla piattaforma corrente non appartiene a nessuno.
    ///
    /// Dove il PID non e' verificabile (`PID_VERIFICABILE == false`) qualunque
    /// valore va bene: il fallback conservativo non lo consulta per decidere
    /// una rimozione per TTL.
    fn pid_non_esistente() -> u32 {
        #[cfg(target_os = "linux")]
        {
            (2..=4_000_000_u32)
                .rev()
                .find(|&pid| !process_alive(pid))
                .expect("un pid libero esiste")
        }
        #[cfg(not(target_os = "linux"))]
        {
            std::process::id()
        }
    }

    /// Un PID LOCALE vivo non si cancella nemmeno oltre il TTL: l'heartbeat
    /// non viene da un timer, e un blocco lungo o un'ibernazione possono
    /// invecchiarlo mentre l'esecuzione sta ancora scrivendo.
    #[cfg(target_os = "linux")]
    #[test]
    fn un_pid_locale_vivo_non_si_cancella_nemmeno_oltre_il_ttl() {
        let (root, store) = radice_e_store("exec-bloccato");
        plant_lock(store.path(), &sample_lock(std::process::id(), 1_000_000));
        let report = scava(&root, Duration::from_secs(60));
        assert!(
            report.removed.is_empty(),
            "un processo locale vivo non e' orfano: {report:?}"
        );
        assert_eq!(report.kept_conservative, 1);
        assert!(store.path().try_exists().expect("stat"));
    }

    #[test]
    fn stale_heartbeat_is_scavenged() {
        let (root, store) = radice_e_store("exec-stale");
        // Heartbeat antico e processo che non c'e' piu': il TTL decide
        // (errori-e-limiti.md).
        let lock = sample_lock(pid_non_esistente(), 1_000_000);
        plant_lock(store.path(), &lock);
        let report = scava(&root, Duration::from_secs(60));
        assert_eq!(report.removed.len(), 1);
        assert!(!store.path().try_exists().expect("stat"));
        std::mem::forget(store);
    }

    #[test]
    fn fresh_heartbeat_with_live_pid_survives() {
        let (root, store) = radice_e_store("exec-fresh");
        let lock = sample_lock(std::process::id(), now_unix_secs());
        plant_lock(store.path(), &lock);
        let report = scava(&root, Duration::from_hours(24));
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_alive, 1);
        assert!(store.path().try_exists().expect("stat"));
    }

    #[test]
    fn corrupt_lock_is_conservative_until_double_ttl() {
        let (root, store) = radice_e_store("exec-corrupt");
        let lock_path = store.path().join(LOCK_FILE_NAME);
        fs::write(&lock_path, b"{ non-json !!!").expect("lock corrotto");
        // Corrotto e recente: mai cancellare.
        let report = scava(&root, Duration::from_secs(60));
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_conservative, 1);
        assert!(store.path().try_exists().expect("stat"));
        // Corrotto e piu' vecchio di TTL*2 (mtime del lock): cancellare.
        let old = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&lock_path)
            .expect("apertura lock")
            .set_modified(old)
            .expect("set mtime");
        let report = scava(&root, Duration::from_secs(60));
        assert_eq!(report.removed.len(), 1);
        assert!(!store.path().try_exists().expect("stat"));
        std::mem::forget(store);
    }

    /// Un lock che c'e' e non si legge non e' un lock assente: nemmeno oltre
    /// TTL*2 lo store si cancella. La lettura fallisce qui perche' al posto del
    /// file c'e' una directory — un permesso negato non basterebbe, i casi
    /// girano anche come root. Solo Unix: su Windows una directory non si apre
    /// come file per cambiarle il mtime.
    #[test]
    #[cfg(unix)]
    fn un_lock_illeggibile_non_autorizza_la_cancellazione() {
        let (root, store) = radice_e_store("exec-illeggibile");
        let lock_path = store.path().join(LOCK_FILE_NAME);
        fs::remove_file(&lock_path).expect("via il lock");
        fs::create_dir(&lock_path).expect("un lock che non si legge");
        let old = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::open(&lock_path)
            .expect("apertura della directory")
            .set_modified(old)
            .expect("set mtime");
        let report = scava(&root, Duration::from_secs(60));
        assert!(report.removed.is_empty(), "{report:?}");
        assert_eq!(report.kept_conservative, 1);
        assert!(store.path().try_exists().expect("stat"));
        fs::remove_dir(&lock_path).expect("via la directory");
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
        let report = scava(&root, Duration::from_secs(1));
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_alive, 0);
        assert_eq!(report.kept_conservative, 1);
        assert!(other_dir.try_exists().expect("stat"));
        assert!(stray_file.try_exists().expect("stat"));
        assert!(no_lock_dir.try_exists().expect("stat"));
    }

    #[test]
    fn concurrent_stores_do_not_disturb_each_other() {
        let root = tempfile::tempdir().expect("root");
        let mut first = TempStore::with_root("exec-a", root.path()).expect("primo store");
        let second = TempStore::with_root("exec-a", root.path()).expect("secondo store");
        assert_ne!(first.path(), second.path(), "suffisso random distinto");
        // Heartbeat e scavenge sull'uno non toccano l'altro.
        first.heartbeat().expect("heartbeat");
        let report = scava(&root, Duration::from_secs(60));
        assert!(report.removed.is_empty());
        assert_eq!(report.kept_alive, 2);
        assert!(first.path().try_exists().expect("stat"));
        assert!(second.path().try_exists().expect("stat"));
        // Il Drop dell'uno lascia intatta la directory dell'altro.
        let second_path = second.path().to_owned();
        drop(first);
        assert!(second_path.try_exists().expect("stat"));
        assert!(second_path.join(LOCK_FILE_NAME).is_file());
    }
}
