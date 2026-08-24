//! Store temporaneo condiviso per esecuzione e scavenging all'avvio
//! (errori-e-limiti.md, "Crash non intercettabili").
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
//!   directory `plenora-*` con un heartbeat piu' vecchio del TTL, oppure —
//!   piu' in fretta — con un heartbeat fermo da oltre [`GRAZIA_PID`] il cui
//!   lock viene da questa macchina e nomina un processo che non esiste piu'.
//!   **Un heartbeat fresco non e' MAI toccato**, qualunque cosa dica il PID;
//!   qualunque voce fuori dal pattern `plenora-*` non e' MAI toccata
//!   (fail-safe totale). La garanzia sull'heartbeat vale al momento del
//!   controllo: decisione e rimozione non sono atomiche, e il residuo e'
//!   dichiarato in errori-e-limiti.md.
//!
//! L'ordine dei due segnali non e' un dettaglio. Il PID e' interpretabile
//! solo localmente, e l'hostname registrato non e' una prova d'identita':
//! immagini clonate e container condividono lo stesso nome. Su una
//! `temp_root` condivisa fra host, un PID vivo altrove ma inesistente qui
//! basterebbe a cancellare la directory di un'esecuzione che sta scrivendo.
//! L'heartbeat, che quell'esecuzione aggiorna ogni secondo, e' invece una
//! prova positiva di vita: comanda lui, e il PID puo' solo accelerare la
//! bonifica di un lock gia' fermo.
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
/// (errori-e-limiti.md — una macchina sospesa/ibernata puo' congelare l'heartbeat di
/// un'esecuzione ancora valida).
pub const DEFAULT_SCAVENGE_TTL: Duration = Duration::from_hours(24);

/// Lunghezza massima accettata per un `execution_id` (va nel nome della
/// directory, quindi e' validato in modo restrittivo).
const MAX_EXECUTION_ID_LEN: usize = 128;

/// Eta' minima dell'heartbeat perche' il PID registrato conti qualcosa.
///
/// L'executor scrive il lock con un throttle di un secondo, quindi
/// un'esecuzione viva ha sempre un heartbeat molto piu' giovane di questo
/// valore. Cinque minuti sono tre ordini di grandezza sopra la cadenza e tre
/// sotto il TTL di default: abbastanza da coprire una pausa lunga di I/O o
/// una macchina sotto carico, abbastanza poco da bonificare in fretta dopo
/// un crash senza aspettare le 24 ore.
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
    /// Restituisce `PlenoraError::InvalidPlan` se `execution_id` e' vuoto, troppo
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

    /// Aggiorna il timestamp di heartbeat nel lock file (errori-e-limiti.md).
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

/// Scavenging all'avvio delle directory temporanee orfane (errori-e-limiti.md).
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
            ScavengeAction::Remove => {
                // Riclassificazione IMMEDIATA prima di cancellare, con
                // l'orologio riletto.
                //
                // Fra la scansione della directory e la rimozione passa il
                // tempo di esaminare tutte le voci precedenti: in quella
                // finestra l'esecuzione proprietaria puo' aver rinnovato il
                // proprio heartbeat, e `remove_dir_all` avrebbe cancellato
                // una directory tornata viva. La seconda lettura riduce la
                // finestra a quella fra il controllo e la `remove_dir_all`,
                // ma NON la chiude: e' un TOCTOU, dichiarato in
                // errori-e-limiti.md, e chiuderlo richiede una lease
                // interprocesso che la v1 non ha.
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
    let lock = fs::read(&lock_path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<LockFile>(&raw).ok());
    if let Some(lock) = lock {
        // L'HEARTBEAT COMANDA, il PID puo' solo accelerare.
        //
        // `saturating_sub`: un heartbeat nel futuro (clock skew) conta come
        // fresco, mai come scaduto.
        let eta_heartbeat = now.saturating_sub(lock.heartbeat_unix_secs);
        // Un processo LOCALE ancora vivo non si cancella nemmeno per TTL.
        //
        // L'heartbeat non viene da un timer: lo scrive l'executor ai confini
        // di batch. Un'operazione bloccata a lungo su I/O, una macchina
        // ibernata o un salto in avanti dell'orologio possono quindi
        // invecchiarlo oltre il TTL mentre l'esecuzione e' viva e sta
        // scrivendo nella sua directory. Dove il PID e' davvero
        // interrogabile — solo Linux — un processo vivo con lo stesso
        // hostname e' la prova che manca, e vince sulla scadenza. Altrove
        // `process_alive` risponde «vivo» per prudenza e non prova nulla:
        // usarlo qui bloccherebbe ogni bonifica.
        let locale_e_vivo =
            PID_VERIFICABILE && hostname_confrontabile(&lock.hostname) && process_alive(lock.pid);
        if eta_heartbeat > ttl.as_secs() {
            if locale_e_vivo {
                return ScavengeAction::KeepConservative;
            }
            return ScavengeAction::Remove;
        }
        // Da qui in giu' l'heartbeat e' dentro il TTL. Il PID serve solo a
        // bonificare in fretta dopo un crash, senza aspettare le 24 ore, e
        // per farlo servono DUE prove concordi, non una:
        //
        // - il lock dice di venire da questa macchina. Non e' una prova
        //   forte: hostname uguali sono normali fra immagini clonate,
        //   container e host configurati allo stesso modo, quindi da sola
        //   l'uguaglianza non basta;
        // - l'heartbeat e' fermo da piu' di [`GRAZIA_PID`]. Un'esecuzione
        //   viva scrive il proprio lock a intervalli di un secondo, quindi
        //   un heartbeat piu' vecchio della grazia significa che *quel*
        //   processo non sta scrivendo — chiunque sia il PID.
        //
        // Interrogare il PID su un heartbeat FRESCO era il difetto: bastava
        // un hostname omonimo su una `temp_root` condivisa perche' un PID
        // inesistente qui — ma vivo altrove — motivasse la cancellazione di
        // un'esecuzione che stava scrivendo in quel momento.
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
    let mtime = fs::metadata(&lock_path)
        .or_else(|_| fs::metadata(path))
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
/// Su Linux si legge da `/proc/sys/kernel/hostname`, che e' il nome che il
/// kernel conosce: e' l'unica piattaforma dove il PID del lock viene davvero
/// interrogato ([`process_alive`]), quindi e' l'unica dove il confronto fra
/// host deve essere affidabile. Le variabili d'ambiente restano il ripiego —
/// `HOSTNAME` non e' esportata dalla maggior parte delle shell, e un lock
/// scritto con `unknown` non e' confrontabile con nulla.
///
/// Nessuna dipendenza nuova: `std::fs` e le variabili d'ambiente.
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
        let dead_pid = pid_non_esistente();
        // Heartbeat fermo da oltre la grazia ma ben dentro il TTL: e' il PID
        // morto ad accelerare la bonifica, non la scadenza.
        let lock = sample_lock(dead_pid, now_unix_secs() - GRAZIA_PID.as_secs() - 60);
        plant_lock(store.path(), &lock);
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_hours(24)).expect("scavenge");
        assert_eq!(report.removed.len(), 1);
        assert!(!store.path().exists(), "processo morto: directory rimossa");
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
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-foreign", root.path()).expect("store");
        let dead_pid = pid_non_esistente();
        let vecchio = now_unix_secs() - GRAZIA_PID.as_secs() - 60;
        plant_lock(store.path(), &foreign_lock(dead_pid, vecchio));
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_hours(24)).expect("scavenge");
        assert!(
            report.removed.is_empty(),
            "il PID di un altro host non decide: {report:?}"
        );
        assert_eq!(report.kept_alive, 1);
        assert!(store.path().exists());
    }

    /// Il caso che l'uguaglianza di hostname da sola non copre: due macchine
    /// con lo STESSO nome (immagini clonate, container) e una radice
    /// condivisa. Il PID dell'esecuzione remota non esiste qui, ma il suo
    /// heartbeat e' fresco: e' una prova positiva di vita e vince sul PID.
    #[cfg(target_os = "linux")]
    #[test]
    fn un_heartbeat_fresco_batte_sempre_il_pid() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-omonimo", root.path()).expect("store");
        let dead_pid = pid_non_esistente();
        // Stesso hostname (il lock dice di venire da qui) ma PID inesistente
        // e heartbeat appena scritto.
        plant_lock(store.path(), &sample_lock(dead_pid, now_unix_secs()));
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_hours(24)).expect("scavenge");
        assert!(
            report.removed.is_empty(),
            "un heartbeat fresco non puo' essere cancellato da un PID: {report:?}"
        );
        assert_eq!(report.kept_alive, 1);
        assert!(store.path().exists());
    }

    /// Lo stesso lock di un altro host, ma con heartbeat oltre il TTL: qui
    /// decide il TTL, che resta valido fra host, e la directory va rimossa.
    #[test]
    fn foreign_host_still_obeys_the_ttl() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-foreign-stale", root.path()).expect("store");
        plant_lock(store.path(), &foreign_lock(std::process::id(), 1_000_000));
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(60)).expect("scavenge");
        assert_eq!(report.removed.len(), 1);
        assert!(!store.path().exists());
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
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-bloccato", root.path()).expect("store");
        plant_lock(store.path(), &sample_lock(std::process::id(), 1_000_000));
        let report =
            scavenge_stale_temp_dirs(root.path(), Duration::from_secs(60)).expect("scavenge");
        assert!(
            report.removed.is_empty(),
            "un processo locale vivo non e' orfano: {report:?}"
        );
        assert_eq!(report.kept_conservative, 1);
        assert!(store.path().exists());
    }

    #[test]
    fn stale_heartbeat_is_scavenged() {
        let root = tempfile::tempdir().expect("root");
        let store = TempStore::with_root("exec-stale", root.path()).expect("store");
        // Heartbeat antico e processo che non c'e' piu': il TTL decide
        // (errori-e-limiti.md).
        let lock = sample_lock(pid_non_esistente(), 1_000_000);
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
            scavenge_stale_temp_dirs(root.path(), Duration::from_hours(24)).expect("scavenge");
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
