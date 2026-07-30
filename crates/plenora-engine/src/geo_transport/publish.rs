//! Livello comandi del trasporto geo: verifica semantica del CRS e
//! pubblicazione atomica dell'output.
//!
//! Port Fase 1 ("coesistenza") da `main.rs` di plenora-geo-tools-arrow:
//! stessa logica, stessi messaggi; gli errori `Box<dyn Error>` del sorgente
//! sono mappati su [`PlenoraError`] (`InvalidPlan` per le violazioni di
//! contratto, `Crs` per gli errori CRS, `Io` per gli errori I/O).
//!
//! Sequenza attesa dal chiamante (come nel sorgente): parse dello schema
//! JSON → controllo `schema_version` → `validate_parameters` →
//! `validate_transform_arrow_crs`/`validate_pair_arrow_crs` → esecuzione
//! (`transform_arrow`/`pair_arrow`) → `publish_atomic`.
//!
//! Profili di publish (ADR 7): [`PublishProfile::Atomic`] e' il comportamento
//! storico; [`PublishProfile::DurableAtomic`] aggiunge il `fsync` della
//! directory dopo il persist. L'esito e' tipizzato ([`PublishOutcome`]) e la
//! destinazione passa un riconoscimento fail-closed del filesystem
//! ([`PlenoraError::Unsupported`]).

use std::io::{self, BufWriter, ErrorKind, Write};
use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
use std::fs::File;

use plenora_core::catalog::{find_operation, CrsRequirement};
use plenora_core::crs::{required_definition, validate_requirement};
use plenora_core::{PlenoraError, RemoteEffect};

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

use super::transport::{ArrowOperation, PairArrowSchema, TransformArrowSchema};

/// Verifica semantica del CRS per `transform_arrow` (righe 249-265 del
/// `main.rs` sorgente). Deve essere chiamata DOPO
/// [`TransformArrowSchema::validate_parameters`] e prima di toccare i dati.
///
/// Il requisito CRS dell'operazione e' risolto dal catalogo core tramite il
/// `catalog_name` legacy (gli alias `geo_*` -> `geo.*` sono gia' nel
/// catalogo). Un requisito `None` e' trattato come `CrsRequirement::Known`:
/// nel catalogo core tutte le operazioni geo hanno `Some(_)`, quindi il ramo
/// e' irraggiungibile e il comportamento resta identico al sorgente.
///
/// # Errors
/// Restituisce `PlenoraError::Crs` se il CRS manca, e' invalido, non e'
/// risolvibile senza backend PROJ o non soddisfa il requisito
/// dell'operazione; `PlenoraError::InvalidPlan` se l'operazione e' assente dal
/// catalogo.
pub fn validate_transform_arrow_crs(schema: &TransformArrowSchema) -> Result<(), PlenoraError> {
    let definition = required_definition(schema.crs.as_deref(), "crs")?;
    let crs = resolve_crs(definition, "crs")?;
    if schema.operation == ArrowOperation::Reproject {
        let target_definition = required_definition(schema.target_crs.as_deref(), "target_crs")?;
        let target = resolve_crs(target_definition, "target_crs")?;
        validate_requirement(CrsRequirement::Reprojection, &[&crs, &target])?;
        return Ok(());
    }
    let catalog_name = schema.operation.catalog_name();
    let descriptor = find_operation(catalog_name).ok_or_else(|| {
        PlenoraError::InvalidPlan(format!("operazione {catalog_name} assente dal catalogo"))
    })?;
    validate_requirement(
        descriptor.crs_requirement.unwrap_or(CrsRequirement::Known),
        &[&crs],
    )?;
    Ok(())
}

/// Verifica semantica del CRS per `pair_arrow` (righe 300-313 del `main.rs`
/// sorgente). Deve essere chiamata DOPO
/// [`PairArrowSchema::validate_parameters`] e prima di toccare i dati.
///
/// # Errors
/// Restituisce `PlenoraError::Crs` se uno dei CRS manca, e' invalido, non
/// e' risolvibile senza backend PROJ o non soddisfa il requisito
/// dell'operazione; `PlenoraError::InvalidPlan` se l'operazione e' assente dal
/// catalogo.
pub fn validate_pair_arrow_crs(schema: &PairArrowSchema) -> Result<(), PlenoraError> {
    let left_definition = required_definition(schema.left_crs.as_deref(), "left_crs")?;
    let right_definition = required_definition(schema.right_crs.as_deref(), "right_crs")?;
    let left_crs = resolve_crs(left_definition, "left_crs")?;
    let right_crs = resolve_crs(right_definition, "right_crs")?;
    let catalog_name = schema.operation.catalog_name();
    let descriptor = find_operation(catalog_name).ok_or_else(|| {
        PlenoraError::InvalidPlan(format!("operazione {catalog_name} assente dal catalogo"))
    })?;
    validate_requirement(
        descriptor.crs_requirement.unwrap_or(CrsRequirement::Known),
        &[&left_crs, &right_crs],
    )?;
    Ok(())
}

/// Profilo di pubblicazione (ADR 7): la garanzia e' precisa, non
/// un'assunzione. Il profilo fa parte delle capability del piano: la scelta
/// e' esplicita, mai silenziosa.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PublishProfile {
    /// `AtomicPublish` (default): nessun output parziale mai visibile.
    /// Tempfile nella stessa directory/filesystem della destinazione,
    /// `persist` no-clobber (rename atomico, mai sovrascrittura), `sync_all`
    /// del file prima del rename.
    #[default]
    Atomic,
    /// `DurableAtomicPublish`: in aggiunta, `fsync` della directory dopo il
    /// persist. Offre le piu' forti garanzie di durabilita' della piattaforma
    /// supportata — non una garanzia universale: controller, cache disco e
    /// impostazioni di sistema restano fuori dal controllo dell'engine. Su
    /// Windows il `fsync` di directory non e' disponibile e l'esito e'
    /// [`PublishOutcome::PublishedButDurabilityUnconfirmed`].
    DurableAtomic,
}

impl PublishProfile {
    /// Nome della capability corrispondente al profilo nelle
    /// `required_capabilities` del `ValidatedGraph` (ADR 7/ADR 4): un grafo
    /// validato con un profilo e' riusabile solo in un ambiente che dichiara
    /// la capability omonima (`check_compatibility` del planner).
    #[must_use]
    pub const fn capability_name(self) -> &'static str {
        match self {
            Self::Atomic => "atomic_publish",
            Self::DurableAtomic => "durable_atomic_publish",
        }
    }
}

/// Esito tipizzato del publish (ADR 7).
///
/// Il fallimento del `fsync` di directory dopo il rename non e' un errore
/// generico — l'output e' completo e gia' visibile, ma la durabilita' non e'
/// confermata. Il chiamante (CLI, adapter) decide come presentarlo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Tutte le garanzie del profilo richiesto sono state soddisfatte.
    Published,
    /// Publish riuscito, durabilita' non confermata (`fsync` della directory
    /// non supportato dalla piattaforma o fallito dopo il rename).
    PublishedButDurabilityUnconfirmed,
}

impl PublishOutcome {
    /// Effetto dell'esito sull'asse canonico «effetto remoto» (R9.6,
    /// contratti trasversali v2.0-rc10 §9, milestone D): collegamento
    /// esplicito tra ADR 7 e il modello a quattro assi (R9.1), SENZA
    /// duplicare l'esito in una variante d'errore — l'esito ignoto non e'
    /// una categoria d'errore (R9.3).
    ///
    /// Entrambi gli esiti mappano su [`RemoteEffect::Committed`]: a publish
    /// terminato l'output e' completo e visibile alla destinazione, quindi
    /// l'effetto e' determinato e definitivo dal punto di vista del
    /// chiamante. In [`PublishOutcome::PublishedButDurabilityUnconfirmed`]
    /// cio' che non e' confermato e' la DURABILITA' (sopravvivenza a un
    /// crash della macchina, ADR 7), non l'esistenza dell'effetto:
    /// [`RemoteEffect::Unknown`] («effetto non determinabile con i mezzi
    /// disponibili», R9.6) sarebbe scorretto — l'output e' osservabile.
    #[must_use]
    pub const fn remote_effect(self) -> RemoteEffect {
        match self {
            Self::Published | Self::PublishedButDurabilityUnconfirmed => RemoteEffect::Committed,
        }
    }
}

/// Tentativi totali di persist (1 iniziale + retry sui soli errori
/// transitori; ADR 7: share lock e antivirus su Windows).
const MAX_PERSIST_ATTEMPTS: u32 = 4;

/// Backoff iniziale tra un tentativo di persist e il successivo; raddoppia a
/// ogni retry (20, 40, 80 ms).
const PERSIST_INITIAL_BACKOFF: Duration = Duration::from_millis(20);

/// Classifica l'errore di `persist`: `AlreadyExists` non e' MAI ritentato
/// (il no-clobber e' sacro); i codici Windows di share-lock/antivirus
/// (`ERROR_ACCESS_DENIED` = 5, `ERROR_SHARING_VIOLATION` = 32,
/// `ERROR_LOCK_VIOLATION` = 33) sono ritentati con backoff breve.
///
/// La funzione e' volutamente indipendente dalla piattaforma per restare
/// testabile ovunque: su Unix `rename(2)` non produce mai questi raw OS
/// error (5 = `EIO`, 32 = `EPIPE`, 33 = `EDOM`), quindi il retry non scatta.
fn retryable_persist_error(error: &io::Error) -> bool {
    if error.kind() == ErrorKind::AlreadyExists {
        return false;
    }
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

/// Esegue `persist` con retry a backoff esponenziale breve sui soli errori
/// transitori ([`retryable_persist_error`]). La closure e' il punto di
/// iniezione dei test: in produzione avvolge `persist_noclobber`,
/// recuperando il tempfile da `PersistError` a ogni tentativo fallito.
fn persist_with_retry(
    mut persist: impl FnMut() -> Result<(), io::Error>,
) -> Result<(), io::Error> {
    let mut backoff = PERSIST_INITIAL_BACKOFF;
    let mut attempt = 1;
    loop {
        match persist() {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt < MAX_PERSIST_ATTEMPTS && retryable_persist_error(&error) {
                    std::thread::sleep(backoff);
                    backoff = backoff.saturating_mul(2);
                    attempt += 1;
                } else {
                    return Err(error);
                }
            }
        }
    }
}

/// Magic `f_type` (vedi `statfs(2)`) dei filesystem locali supportati
/// (ADR 7). Riconoscimento fail-closed: qualunque magic fuori da questa
/// whitelist e' rifiutato come `UnsupportedPublishTarget` — in dubbio,
/// rifiutare.
#[cfg(any(target_os = "linux", test))]
const LOCAL_FS_MAGICS: &[u64] = &[
    0xEF53,      // ext2/ext3/ext4
    0x5846_5342, // xfs ("XFSB")
    0x9123_683E, // btrfs
    0x0102_1994, // tmpfs
    0x794C_7630, // overlayfs (rootfs dei container)
];

/// Magic `f_type` dei filesystem di rete noti: rifiutati con messaggio
/// dedicato (filesystem di rete fuori scope v1, ADR 7).
#[cfg(any(target_os = "linux", test))]
const NETWORK_FS_MAGICS: &[(u64, &str)] = &[
    (0x6969, "NFS"),
    (0x517B, "SMB"),
    (0xFF53_4D42, "CIFS"),
    (0xFE53_4D42, "SMB2"),
];

/// Classe del filesystem di destinazione (ADR 7, fail-closed).
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilesystemClass {
    /// Filesystem locale in whitelist: publish consentito.
    Local,
    /// Filesystem di rete riconosciuto: rifiutato con messaggio dedicato.
    Network(&'static str),
    /// Magic non riconosciuto: rifiutato (in dubbio, rifiutare).
    Unknown,
}

/// Classificazione pura del magic `f_type`: punto di iniezione dei test per
/// il rifiuto dei filesystem di rete senza bisogno di un mount NFS reale.
#[cfg(any(target_os = "linux", test))]
fn classify_filesystem(magic: u64) -> FilesystemClass {
    if LOCAL_FS_MAGICS.contains(&magic) {
        return FilesystemClass::Local;
    }
    for &(network_magic, name) in NETWORK_FS_MAGICS {
        if magic == network_magic {
            return FilesystemClass::Network(name);
        }
    }
    FilesystemClass::Unknown
}

/// Riconoscimento fail-closed del filesystem (ADR 7) su Linux: `statfs`
/// della directory di destinazione e whitelist dei magic locali.
#[cfg(target_os = "linux")]
fn ensure_supported_publish_target(parent: &Path) -> Result<(), PlenoraError> {
    let stat = rustix::fs::statfs(parent)
        .map_err(|errno| io::Error::from_raw_os_error(errno.raw_os_error()))?;
    #[allow(clippy::cast_sign_loss)] // I magic f_type dei filesystem sono positivi.
    let magic = stat.f_type as u64;
    match classify_filesystem(magic) {
        FilesystemClass::Local => Ok(()),
        FilesystemClass::Network(name) => Err(PlenoraError::Unsupported(format!(
            "filesystem di rete {name} (fuori scope v1): {}",
            parent.display()
        ))),
        FilesystemClass::Unknown => Err(PlenoraError::Unsupported(format!(
            "filesystem non identificabile (f_type={magic:#x}), rifiuto fail-closed: {}",
            parent.display()
        ))),
    }
}

/// Riconoscimento fail-closed su Windows, solo `std`: i percorsi UNC
/// (`\\server\share`) sono sicuramente di rete e vengono rifiutati. Limite
/// documentato: senza `windows-sys` (`GetDriveTypeW`) i drive di rete
/// mappati su lettera non sono rilevabili e sono accettati come locali.
#[cfg(windows)]
fn ensure_supported_publish_target(parent: &Path) -> Result<(), PlenoraError> {
    let text = parent.as_os_str().to_string_lossy();
    let verbatim = text.starts_with("\\\\?\\") || text.starts_with("\\\\.\\");
    if (text.starts_with("\\\\") && !verbatim) || text.starts_with("//") {
        return Err(PlenoraError::Unsupported(format!(
            "percorso UNC (filesystem di rete): {}",
            parent.display()
        )));
    }
    Ok(())
}

/// Altri Unix (macOS compreso): il riconoscimento via `statfs` non e' ancora
/// implementato; per la regola fail-closed la destinazione non
/// identificabile e' rifiutata.
#[cfg(all(unix, not(target_os = "linux")))]
fn ensure_supported_publish_target(parent: &Path) -> Result<(), PlenoraError> {
    Err(PlenoraError::Unsupported(format!(
        "riconoscimento del filesystem non implementato su questa piattaforma, \
         rifiuto fail-closed: {}",
        parent.display()
    )))
}

/// `fsync` della directory dopo il persist (profilo
/// [`PublishProfile::DurableAtomic`], ADR 7). Su Unix apre la directory e la
/// sincronizza; il fallimento non e' un errore — il file e' gia' pubblicato —
/// ma declassa l'esito a [`PublishOutcome::PublishedButDurabilityUnconfirmed`].
#[cfg(unix)]
fn sync_directory_outcome(parent: &Path) -> PublishOutcome {
    match File::open(parent).and_then(|directory| directory.sync_all()) {
        Ok(()) => PublishOutcome::Published,
        Err(_) => PublishOutcome::PublishedButDurabilityUnconfirmed,
    }
}

/// Windows non espone il `fsync` di directory: la durabilita' richiesta dal
/// profilo [`PublishProfile::DurableAtomic`] non e' confermabile (ADR 7) e
/// l'esito e' sempre [`PublishOutcome::PublishedButDurabilityUnconfirmed`].
#[cfg(not(unix))]
const fn sync_directory_outcome(_parent: &Path) -> PublishOutcome {
    PublishOutcome::PublishedButDurabilityUnconfirmed
}

// Hook di iniezione dei fallimenti (solo test, ADR 7): simula un crash
// dopo scrittura + sync del tempfile e prima del persist. Thread-local per
// non interferire con i test paralleli.
#[cfg(test)]
thread_local! {
    static FAIL_BEFORE_PERSIST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Pubblicazione atomica dell'output con profilo selezionabile (ADR 7).
///
/// Rifiuta un output esistente, verifica il filesystem di destinazione
/// (fail-closed), scrive su un tempfile `.plenora-geo-*.partial` nella
/// directory di destinazione, flush + sync e `persist_noclobber` con retry a
/// backoff sui soli errori transitori di condivisione (Windows). Con
/// [`PublishProfile::DurableAtomic`] sincronizza anche la directory dopo il
/// persist e l'esito riflette la conferma della durabilita'.
///
/// # Errors
/// Restituisce `PlenoraError::InvalidPlan` se l'output esiste gia' o la
/// directory di destinazione non esiste;
/// `PlenoraError::Unsupported` se il filesystem di destinazione
/// e' di rete o non identificabile; `PlenoraError::Io` per i fallimenti di
/// scrittura, sync o persist; propaga l'errore della closure `write`.
///
/// # Panics
///
/// Mai su input esterno: il solo `expect` copre l'invariante interna per cui
/// il tempfile resta disponibile finche' il persist non e' riuscito — a ogni
/// tentativo fallito `PersistError` restituisce il file al retry.
pub fn publish_with_profile<T>(
    output_path: &Path,
    profile: PublishProfile,
    write: impl FnOnce(&mut dyn Write) -> Result<T, PlenoraError>,
) -> Result<(T, PublishOutcome), PlenoraError> {
    if output_path.exists() {
        return Err(PlenoraError::InvalidPlan(format!(
            "output gia' esistente: {}",
            output_path.display()
        )));
    }
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(PlenoraError::InvalidPlan(format!(
            "directory output inesistente: {}",
            parent.display()
        )));
    }
    ensure_supported_publish_target(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".plenora-geo-")
        .suffix(".partial")
        .tempfile_in(parent)?;
    let result = {
        let mut output_writer = BufWriter::with_capacity(1024 * 1024, temporary.as_file_mut());
        let result = write(&mut output_writer)?;
        output_writer.flush()?;
        result
    };
    temporary.as_file().sync_all()?;
    // Hook di iniezione (solo test, ADR 7): fallimento tra scrittura e
    // persist — il tempfile deve essere ripulito dal Drop, nessun output
    // parziale visibile.
    #[cfg(test)]
    if FAIL_BEFORE_PERSIST.with(|flag| flag.replace(false)) {
        return Err(PlenoraError::Io(io::Error::other(
            "fallimento iniettato dal test prima del persist",
        )));
    }
    let mut temporary = Some(temporary);
    persist_with_retry(|| {
        // Il tempfile resta disponibile finche' il persist fallisce (ogni
        // errore lo restituisce via `PersistError`): un `None` qui e' una
        // invariante interna violata, errore esplicito e non ritentabile
        // (`retryable_persist_error`: kind diverso da `AlreadyExists` e
        // nessun codice OS transitorio), mai un panic (R6).
        let Some(file) = temporary.take() else {
            return Err(io::Error::other(
                "tempfile di publish assente al retry del persist: invariante interna violata",
            ));
        };
        match file.persist_noclobber(output_path) {
            Ok(_persisted) => Ok(()),
            Err(persist_error) => {
                temporary = Some(persist_error.file);
                Err(persist_error.error)
            }
        }
    })?;
    let outcome = match profile {
        PublishProfile::Atomic => PublishOutcome::Published,
        PublishProfile::DurableAtomic => sync_directory_outcome(parent),
    };
    Ok((result, outcome))
}

/// Pubblicazione atomica dell'output (righe 315-349 del `main.rs` sorgente).
///
/// Wrapper di compatibilita' su [`publish_with_profile`] con profilo
/// [`PublishProfile::Atomic`]: comportamento identico al publish storico,
/// l'esito tipizzato (sempre [`PublishOutcome::Published`] a publish
/// riuscito) e' scartato.
///
/// # Errors
/// Come [`publish_with_profile`].
pub fn publish_atomic<T>(
    output_path: &Path,
    write: impl FnOnce(&mut dyn Write) -> Result<T, PlenoraError>,
) -> Result<T, PlenoraError> {
    let (result, _outcome) = publish_with_profile(output_path, PublishProfile::Atomic, write)?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // -- Riconoscimento fail-closed del filesystem (ADR 7) ------------------

    #[test]
    fn classify_local_magics_as_supported() {
        for &magic in LOCAL_FS_MAGICS {
            assert_eq!(
                classify_filesystem(magic),
                FilesystemClass::Local,
                "magic {magic:#x} atteso come locale"
            );
        }
    }

    #[test]
    fn classify_network_magics_as_rejected() {
        assert_eq!(classify_filesystem(0x6969), FilesystemClass::Network("NFS"));
        assert_eq!(classify_filesystem(0x517B), FilesystemClass::Network("SMB"));
        assert_eq!(
            classify_filesystem(0xFF53_4D42),
            FilesystemClass::Network("CIFS")
        );
        assert_eq!(
            classify_filesystem(0xFE53_4D42),
            FilesystemClass::Network("SMB2")
        );
    }

    #[test]
    fn classify_unknown_magic_is_fail_closed() {
        assert_eq!(classify_filesystem(0xDEAD_BEEF), FilesystemClass::Unknown);
        // FUSE non e' identificabile come locale: in dubbio, rifiutare.
        assert_eq!(classify_filesystem(0x6573_5546), FilesystemClass::Unknown);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tempdir_is_recognized_as_local() {
        let directory = tempfile::tempdir().expect("tempdir");
        ensure_supported_publish_target(directory.path()).expect("filesystem locale");
    }

    // -- Classificazione degli errori di persist ----------------------------

    #[test]
    fn already_exists_is_never_retryable() {
        let error = io::Error::from(ErrorKind::AlreadyExists);
        assert!(!retryable_persist_error(&error));
    }

    #[test]
    fn windows_share_lock_codes_are_retryable() {
        // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        for code in [5, 32, 33] {
            assert!(retryable_persist_error(&io::Error::from_raw_os_error(code)));
        }
    }

    #[test]
    fn other_errors_are_not_retryable() {
        // EACCES (13) su Unix: accesso negato permanente, non transitorio.
        assert!(!retryable_persist_error(&io::Error::from_raw_os_error(13)));
        assert!(!retryable_persist_error(&io::Error::other("generico")));
    }

    // -- Retry con backoff (punto di iniezione) -----------------------------

    #[test]
    fn persist_retries_transient_errors_then_succeeds() {
        let attempts = AtomicU32::new(0);
        let result = persist_with_retry(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            if attempts.load(Ordering::SeqCst) < 3 {
                Err(io::Error::from_raw_os_error(32)) // ERROR_SHARING_VIOLATION
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn persist_gives_up_after_max_attempts() {
        let attempts = AtomicU32::new(0);
        let result = persist_with_retry(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::from_raw_os_error(33)) // ERROR_LOCK_VIOLATION
        });
        let error = result.expect_err("i tentativi si esauriscono");
        assert_eq!(error.raw_os_error(), Some(33));
        assert_eq!(attempts.load(Ordering::SeqCst), MAX_PERSIST_ATTEMPTS);
    }

    #[test]
    fn persist_does_not_retry_already_exists() {
        let attempts = AtomicU32::new(0);
        let result = persist_with_retry(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::from(ErrorKind::AlreadyExists))
        });
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn persist_does_not_retry_non_transient_errors() {
        let attempts = AtomicU32::new(0);
        let result = persist_with_retry(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::from_raw_os_error(13)) // EACCES: non transitorio
        });
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    // -- Profili di publish ---------------------------------------------------

    #[test]
    fn publish_outcome_maps_on_the_remote_effect_axis() {
        // R9.1/R9.6 (milestone D): l'esito tipizzato di ADR 7 vive
        // sull'asse effetto, non in una variante d'errore (R9.3). Committed
        // in entrambi i casi: l'output e' completo e visibile; in
        // `PublishedButDurabilityUnconfirmed` e' la durabilita' a non
        // essere confermata, non l'esistenza dell'effetto.
        assert_eq!(
            PublishOutcome::Published.remote_effect(),
            RemoteEffect::Committed
        );
        assert_eq!(
            PublishOutcome::PublishedButDurabilityUnconfirmed.remote_effect(),
            RemoteEffect::Committed
        );
    }

    #[test]
    fn atomic_profile_reports_published() {
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("output.bin");
        let ((), outcome) = publish_with_profile(&destination, PublishProfile::Atomic, |writer| {
            writer.write_all(b"x")?;
            Ok(())
        })
        .expect("publish atomico");
        assert_eq!(outcome, PublishOutcome::Published);
        assert_eq!(std::fs::read(&destination).expect("lettura"), b"x");
    }

    #[test]
    fn durable_profile_publishes_and_syncs_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("output.bin");
        let (value, outcome) =
            publish_with_profile(&destination, PublishProfile::DurableAtomic, |writer| {
                writer.write_all(b"dati")?;
                Ok(42)
            })
            .expect("publish durevole");
        assert_eq!(value, 42);
        // Su Unix il fsync di directory e' supportato; su Windows no (ADR 7).
        #[cfg(unix)]
        assert_eq!(outcome, PublishOutcome::Published);
        #[cfg(not(unix))]
        assert_eq!(outcome, PublishOutcome::PublishedButDurabilityUnconfirmed);
        assert_eq!(std::fs::read(&destination).expect("lettura"), b"dati");
    }

    #[test]
    fn publish_atomic_wrapper_keeps_legacy_behavior() {
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("output.bin");
        let value = publish_atomic(&destination, |writer| {
            writer.write_all(b"legacy")?;
            Ok(7)
        })
        .expect("publish legacy");
        assert_eq!(value, 7);
        assert_eq!(std::fs::read(&destination).expect("lettura"), b"legacy");

        // No-clobber: la seconda pubblicazione sullo stesso path fallisce.
        let result = publish_atomic(&destination, |writer| {
            writer.write_all(b"altro")?;
            Ok(())
        });
        assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))));
        assert_eq!(std::fs::read(&destination).expect("lettura"), b"legacy");
    }

    // -- Crash simulato tra scrittura e persist (ADR 7) ----------------------

    #[test]
    fn crash_between_write_and_persist_leaves_nothing_visible() {
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("output.bin");
        FAIL_BEFORE_PERSIST.with(|flag| flag.set(true));
        let result = publish_with_profile(&destination, PublishProfile::Atomic, |writer| {
            writer.write_all(b"dati mai pubblicati")?;
            Ok(())
        });
        assert!(matches!(result, Err(PlenoraError::Io(_))));
        assert!(
            !destination.exists(),
            "nessun file visibile alla destinazione"
        );
        // Il tempfile `.partial` e' ripulito dal Drop: directory vuota.
        let leftovers = std::fs::read_dir(directory.path())
            .expect("read_dir")
            .count();
        assert_eq!(leftovers, 0, "il tempfile e' ripulito");
    }
}
