//! Livello comandi del trasporto geo: verifica semantica del CRS e
//! pubblicazione atomica dell'output.
//!
//! Superficie compatibile: i messaggi sono quelli che chi invoca questi
//! comandi si aspetta. Gli errori sono [`PlenoraError`] — `InvalidPlan` per
//! le violazioni di contratto, `Crs` per gli errori CRS, `Io` per l'I/O.
//!
//! Sequenza attesa dal chiamante: parse dello schema
//! JSON → controllo `schema_version` → `validate_parameters` →
//! `validate_transform_arrow_crs`/`validate_pair_arrow_crs` → esecuzione
//! (`transform_arrow`/`pair_arrow`) → `publish_with_profile`.
//!
//! Profili di publish (errori-e-limiti.md#publish-e-cleanup):
//! [`PublishProfile::DurableAtomic`] aggiunge a [`PublishProfile::Atomic`]
//! il `fsync` della directory dopo il persist. L'esito e' tipizzato
//! ([`PublishOutcome`]) e la destinazione passa un riconoscimento
//! fail-closed del filesystem ([`PlenoraError::Unsupported`]).

use std::io::{self, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::fs::File;

use plenora_core::catalog::{find_operation, CrsRequirement};
use plenora_core::crs::{required_definition, validate_requirement};
use plenora_core::{ErrorPhase, PlenoraError, RemoteEffect};

// La scelta dell'implementazione sta in un posto solo: due copie di due
// `cfg` sono due occasioni di divergere, e chi risolve i CRS deve essere chi
// il programma dichiara di essere.
use crate::risolutore::risolvi as resolve_crs;

use super::transport::{ArrowOperation, PairArrowSchema, TransformArrowSchema};

/// Verifica semantica del CRS per `transform_arrow`. Deve essere chiamata
/// DOPO [`TransformArrowSchema::validate_parameters`] e prima di toccare i
/// dati.
///
/// Il requisito CRS e' risolto dal catalogo core tramite il `catalog_name`
/// (gli alias `geo_*` -> `geo.*` sono nel catalogo). Un requisito `None` e'
/// trattato come `CrsRequirement::Known`; nel catalogo non si presenta.
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

/// Verifica semantica del CRS per `pair_arrow`. Deve essere chiamata DOPO
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

/// Profilo di pubblicazione (errori-e-limiti.md#publish-e-cleanup): la garanzia e' precisa, non
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
    /// `required_capabilities` del `ValidatedGraph`
    /// (errori-e-limiti.md#publish-e-cleanup,
    /// piano-v5.md#identita-e-fingerprint): un grafo validato con un profilo
    /// e' riusabile solo in un ambiente che dichiara la capability omonima
    /// (`check_compatibility` del planner).
    #[must_use]
    pub const fn capability_name(self) -> &'static str {
        match self {
            Self::Atomic => "atomic_publish",
            Self::DurableAtomic => "durable_atomic_publish",
        }
    }
}

/// Esito tipizzato del publish (errori-e-limiti.md#publish-e-cleanup).
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

/// Che ne e' stato del temporaneo, dopo il commit point.
///
/// `persist_noclobber` ripiega, dove manca `RENAME_NOREPLACE`, su
/// `hard_link` seguito da `unlink`: il no-clobber regge, ma l'errore
/// dell'`unlink` e' ignorato e il temporaneo puo' restare senza che nessuno
/// lo dica. Questo tipo rende il residuo un esito riportato.
///
/// Tre stati e non due perche' «non c'e'» e «non ho potuto guardare» sono
/// diversi: confonderli sarebbe fail-open. Non porta la causa dell'`unlink`
/// perche' `tempfile` non la espone: si dice solo cio' che si osserva.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PuliziaDelTemporaneo {
    /// Il temporaneo non c'e' piu': niente da bonificare.
    Rimosso,
    /// Il temporaneo c'e' ancora, e queste sono le sue coordinate.
    ///
    /// Non e' un fallimento della pubblicazione: l'output e' visibile e
    /// completo. E' spazzatura da togliere, e chi deve toglierla ha bisogno del
    /// percorso, non del solo peso.
    Presente {
        /// Dove si trova.
        percorso: PathBuf,
        /// Quanto occupa, al momento dell'osservazione.
        byte: u64,
    },
    /// Non si e' potuto accertare se ci sia.
    NonAccertabile {
        /// Dove si sarebbe dovuto guardare.
        percorso: PathBuf,
        /// Perche' non si e' potuto.
        ragione: RagioneNonAccertabile,
    },
}

/// Perche' l'accertamento del temporaneo non e' riuscito.
///
/// Sanitizzata come ogni altra ragione del progetto: dice il **genere** del
/// guasto, non che cosa ci fosse nel file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RagioneNonAccertabile {
    /// Guardare quel percorso richiede un permesso che non c'e'.
    PermessoNegato,
    /// Un guasto di I/O che non e' un permesso.
    GuastoDiLettura,
}

/// Com'e' andata una pubblicazione, sui **due** assi che la descrivono.
///
/// La durabilita' riguarda la **destinazione**, la pulizia il
/// **temporaneo**: sono indipendenti, e un enum solo dovrebbe enumerarne le
/// combinazioni.
///
/// Dopo il commit point l'output e' visibile e nessuno dei due diventa un
/// errore: sono **avvertenze**, perche' rifare la pubblicazione troverebbe
/// la destinazione occupata. Prima del commit un errore resta un errore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsitoDellaPubblicazione {
    /// Se le garanzie di durabilita' del profilo sono state soddisfatte.
    pub durabilita: PublishOutcome,
    /// Che ne e' stato del temporaneo.
    pub pulizia: PuliziaDelTemporaneo,
}

/// Guarda se il temporaneo e' sopravvissuto al commit.
///
/// Si chiama **dopo** un commit riuscito, e per questo non rende mai un errore:
/// cio' che non si riesce a sapere diventa [`PuliziaDelTemporaneo::NonAccertabile`],
/// che e' un'osservazione e non un guasto della pubblicazione.
fn accerta_il_temporaneo(percorso: PathBuf) -> PuliziaDelTemporaneo {
    match std::fs::symlink_metadata(&percorso) {
        Ok(metadati) => PuliziaDelTemporaneo::Presente {
            percorso,
            byte: metadati.len(),
        },
        Err(errore) => match errore.kind() {
            io::ErrorKind::NotFound => PuliziaDelTemporaneo::Rimosso,
            io::ErrorKind::PermissionDenied => PuliziaDelTemporaneo::NonAccertabile {
                percorso,
                ragione: RagioneNonAccertabile::PermessoNegato,
            },
            _ => PuliziaDelTemporaneo::NonAccertabile {
                percorso,
                ragione: RagioneNonAccertabile::GuastoDiLettura,
            },
        },
    }
}

impl PublishOutcome {
    /// Effetto dell'esito sull'asse canonico «effetto remoto» (R9.6,
    /// contratti trasversali v2.0-rc10 §9), senza duplicare l'esito in una
    /// variante d'errore: l'esito ignoto non e' una categoria d'errore (R9.3).
    ///
    /// Entrambi gli esiti mappano su [`RemoteEffect::Committed`]: l'output e'
    /// completo e visibile. In
    /// [`PublishOutcome::PublishedButDurabilityUnconfirmed`] non e' confermata
    /// la DURABILITA' (errori-e-limiti.md#publish-e-cleanup), non l'esistenza
    /// dell'effetto, quindi [`RemoteEffect::Unknown`] sarebbe scorretto.
    #[must_use]
    pub const fn remote_effect(self) -> RemoteEffect {
        match self {
            Self::Published | Self::PublishedButDurabilityUnconfirmed => RemoteEffect::Committed,
        }
    }
}

/// Tentativi totali di persist (1 iniziale + retry sui soli errori
/// transitori; errori-e-limiti.md#publish-e-cleanup: share lock e antivirus su Windows).
const MAX_PERSIST_ATTEMPTS: u32 = 4;

/// Backoff iniziale tra un tentativo di persist e il successivo; raddoppia a
/// ogni retry (20, 40, 80 ms).
const PERSIST_INITIAL_BACKOFF: Duration = Duration::from_millis(20);

/// Classifica l'errore di `persist`: `AlreadyExists` non e' MAI ritentato
/// (il no-clobber e' sacro); i codici Windows di share-lock/antivirus
/// (`ERROR_ACCESS_DENIED` = 5, `ERROR_SHARING_VIOLATION` = 32,
/// `ERROR_LOCK_VIOLATION` = 33) sono ritentati con backoff breve.
///
/// Indipendente dalla piattaforma per restare testabile ovunque: su Unix
/// `rename(2)` non produce questi raw OS error, quindi il retry non scatta.
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
fn persist_with_retry(mut persist: impl FnMut() -> Result<(), io::Error>) -> Result<(), io::Error> {
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
/// (errori-e-limiti.md#publish-e-cleanup). Riconoscimento fail-closed: qualunque magic fuori da questa
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
/// dedicato (filesystem di rete fuori scope v1, errori-e-limiti.md#publish-e-cleanup).
#[cfg(any(target_os = "linux", test))]
const NETWORK_FS_MAGICS: &[(u64, &str)] = &[
    (0x6969, "NFS"),
    (0x517B, "SMB"),
    (0xFF53_4D42, "CIFS"),
    (0xFE53_4D42, "SMB2"),
];

/// Classe del filesystem di destinazione (errori-e-limiti.md#publish-e-cleanup, fail-closed).
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

/// Riconoscimento fail-closed del filesystem (errori-e-limiti.md#publish-e-cleanup) su Linux: `statfs`
/// della directory di destinazione e whitelist dei magic locali. Fase
/// [`ErrorPhase::Probe`] (BLOCK-03): ispezione preliminare della
/// destinazione — la fase in cui la si scopre, non quella derivata dalla
/// variante `Unsupported` in cui e' confluita.
#[cfg(target_os = "linux")]
fn ensure_supported_publish_target(parent: &Path) -> Result<(), PlenoraError> {
    let stat = rustix::fs::statfs(parent).map_err(|errno| {
        io_at(
            ErrorPhase::Probe,
            io::Error::from_raw_os_error(errno.raw_os_error()),
        )
    })?;
    #[allow(clippy::cast_sign_loss)] // I magic f_type dei filesystem sono positivi.
    let magic = stat.f_type as u64;
    match classify_filesystem(magic) {
        FilesystemClass::Local => Ok(()),
        FilesystemClass::Network(name) => Err(PlenoraError::Unsupported(format!(
            "filesystem di rete {name} (fuori scope v1): {}",
            parent.display()
        ))
        .with_phase(ErrorPhase::Probe)),
        FilesystemClass::Unknown => Err(PlenoraError::Unsupported(format!(
            "filesystem non identificabile (f_type={magic:#x}), rifiuto fail-closed: {}",
            parent.display()
        ))
        .with_phase(ErrorPhase::Probe)),
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
        ))
        .with_phase(ErrorPhase::Probe));
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
    ))
    .with_phase(ErrorPhase::Probe))
}

/// `fsync` della directory dopo il persist (profilo
/// [`PublishProfile::DurableAtomic`], errori-e-limiti.md#publish-e-cleanup). Su Unix apre la directory e la
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
/// profilo [`PublishProfile::DurableAtomic`] non e' confermabile (errori-e-limiti.md#publish-e-cleanup) e
/// l'esito e' sempre [`PublishOutcome::PublishedButDurabilityUnconfirmed`].
#[cfg(not(unix))]
const fn sync_directory_outcome(_parent: &Path) -> PublishOutcome {
    PublishOutcome::PublishedButDurabilityUnconfirmed
}

// Hook di iniezione dei fallimenti (solo test, errori-e-limiti.md#publish-e-cleanup): simula un crash
// dopo scrittura + sync del tempfile e prima del persist. Thread-local per
// non interferire con i test paralleli.
#[cfg(test)]
thread_local! {
    static FAIL_BEFORE_PERSIST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Errore I/O del confine di publish con la fase esplicita (BLOCK-03,
/// assi §9): la variante `Io` non distingue il momento, il confine si' —
/// testo, categoria e disposizione di retry invariati per delega.
fn io_at(phase: ErrorPhase, error: io::Error) -> PlenoraError {
    PlenoraError::Io(error).with_phase(phase)
}

/// La destinazione e' occupata: una sola forma, per ogni strada che la
/// scopre.
///
/// Il testo non nomina chi l'ha scoperta, perche' a chi legge non cambia
/// niente: la destinazione c'e', e la prima esecuzione che pubblica vince.
#[must_use]
pub fn conflitto_sulla_destinazione(output_path: &Path) -> PlenoraError {
    PlenoraError::Conflict(format!("output gia' esistente: {}", output_path.display()))
        .with_phase(ErrorPhase::Commit)
}

/// Il controllo no-clobber anticipato: la destinazione non deve esistere
/// (errori-e-limiti.md#publish-e-cleanup).
///
/// E' un'anticipazione: l'autorita' e' l'`AlreadyExists` al persist, e le due
/// strade danno la stessa classe. Chi controlla prima di cominciare il lavoro,
/// come la CLI, usa questa funzione e non una copia.
///
/// # Errors
///
/// `PlenoraError::Conflict`, fase `Commit`, se la destinazione esiste;
/// `PlenoraError::Io`, fase `Probe`, se non si lascia osservare: una
/// destinazione che non si lascia osservare non e' libera.
pub fn verifica_destinazione_libera(output_path: &Path) -> Result<(), PlenoraError> {
    if output_path
        .try_exists()
        .map_err(|error| io_at(ErrorPhase::Probe, error))?
    {
        return Err(conflitto_sulla_destinazione(output_path));
    }
    Ok(())
}

/// Pubblicazione atomica dell'output con profilo selezionabile (errori-e-limiti.md#publish-e-cleanup).
///
/// Rifiuta un output esistente, verifica il filesystem di destinazione
/// (fail-closed), scrive su un tempfile `.plenora-geo-*.partial` nella
/// directory di destinazione, flush + sync e `persist_noclobber` con retry a
/// backoff sui soli errori transitori di condivisione (Windows). Con
/// [`PublishProfile::DurableAtomic`] sincronizza anche la directory dopo il
/// persist e l'esito riflette la conferma della durabilita'.
///
/// Tagging di fase (BLOCK-03, piano-v5.md#contratti-di-input): destinazione
/// [`ErrorPhase::Probe`], tempfile [`ErrorPhase::Write`], flush/sync
/// [`ErrorPhase::Finalize`], no-clobber e rename [`ErrorPhase::Commit`]. Gli
/// errori della closure `write` NON sono taggati; la pulizia del tempfile e'
/// via `Drop`, quindi nessun errore [`ErrorPhase::Cleanup`].
///
/// # Errors
/// Restituisce `PlenoraError::Conflict` se l'output esiste gia' (tag
/// `Commit`); `PlenoraError::InvalidPlan` se la directory di destinazione non
/// esiste (tag `Probe`); `PlenoraError::Unsupported` se il filesystem di
/// destinazione e' di rete o non identificabile (tag `Probe`);
/// `PlenoraError::Io` se la destinazione non si lascia osservare (tag
/// `Probe`) e per i fallimenti di scrittura, sync o persist (tag
/// `Write`/`Finalize`/`Commit`);
/// propaga invariato l'errore della closure `write`.
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
) -> Result<(T, EsitoDellaPubblicazione), PlenoraError> {
    verifica_destinazione_libera(output_path)?;
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    // Riconoscimento preliminare della destinazione: fase Probe. Solo «non
    // esiste» e «non e' una directory» sono del piano; una directory che non si
    // lascia osservare e' un errore di I/O, non una directory inesistente.
    let e_una_directory = match std::fs::metadata(parent) {
        Ok(metadata) => metadata.is_dir(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(io_at(ErrorPhase::Probe, error)),
    };
    if !e_una_directory {
        return Err(PlenoraError::InvalidPlan(format!(
            "directory output inesistente: {}",
            parent.display()
        ))
        .with_phase(ErrorPhase::Probe));
    }
    // Destinazione non supportata: la fase e' `Probe`, quella in cui la si
    // scopre, e il tag dentro `ensure_supported_publish_target` la impone
    // sulla derivazione per variante.
    ensure_supported_publish_target(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".plenora-geo-")
        .suffix(".partial")
        .tempfile_in(parent)
        .map_err(|error| io_at(ErrorPhase::Write, error))?;
    let result = {
        let mut output_writer = BufWriter::with_capacity(1024 * 1024, temporary.as_file_mut());
        // Errori della closure: propagati invariati (nascono nel chiamante,
        // restano derivati per variante — `Write` per Io/DataMapping).
        let result = write(&mut output_writer)?;
        output_writer
            .flush()
            .map_err(|error| io_at(ErrorPhase::Finalize, error))?;
        result
    };
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| io_at(ErrorPhase::Finalize, error))?;
    // Hook di iniezione (solo test, errori-e-limiti.md#publish-e-cleanup): fallimento tra scrittura e
    // persist — il tempfile deve essere ripulito dal Drop, nessun output
    // parziale visibile.
    #[cfg(test)]
    if FAIL_BEFORE_PERSIST.with(|flag| flag.replace(false)) {
        return Err(PlenoraError::Io(io::Error::other(
            "fallimento iniettato dal test prima del persist",
        )));
    }
    // Il percorso si prende **adesso**: `persist_noclobber` consuma il
    // `NamedTempFile`, e dopo non c'e' piu' nessuno che sappia dove fosse.
    let percorso_temporaneo = temporary.path().to_path_buf();
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
    })
    // Rename atomico: fase Commit (§9). L'`AlreadyExists` osservato qui e'
    // **l'autorita'**, e ha la stessa classe del controllo preliminare.
    .map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            conflitto_sulla_destinazione(output_path)
        } else {
            io_at(ErrorPhase::Commit, error)
        }
    })?;
    let durabilita = match profile {
        PublishProfile::Atomic => PublishOutcome::Published,
        PublishProfile::DurableAtomic => sync_directory_outcome(parent),
    };
    // Da qui in poi l'output e' visibile: cio' che si scopre sul temporaneo e'
    // un'avvertenza, mai un fallimento.
    let pulizia = accerta_il_temporaneo(percorso_temporaneo);
    Ok((
        result,
        EsitoDellaPubblicazione {
            durabilita,
            pulizia,
        },
    ))
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Pubblica col profilo atomico **ignorando l'esito**.
    ///
    /// Esiste solo nei test: in produzione scartare le avvertenze e' la classe
    /// di difetto da evitare. Qui serve ai casi che giudicano altro.
    fn pubblica_ignorando_l_esito<T>(
        output_path: &Path,
        write: impl FnOnce(&mut dyn Write) -> Result<T, PlenoraError>,
    ) -> Result<T, PlenoraError> {
        publish_with_profile(output_path, PublishProfile::Atomic, write).map(|(valore, _)| valore)
    }

    // -- Riconoscimento fail-closed del filesystem (errori-e-limiti.md#publish-e-cleanup) ------------------

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
    fn windows_share_lock_codes_are_retryable() {
        // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        for code in [5, 32, 33] {
            assert!(retryable_persist_error(&io::Error::from_raw_os_error(code)));
        }
    }

    #[test]
    fn already_exists_and_other_errors_are_not_retryable() {
        for error in [
            // Mai: la destinazione esiste, riprovare non la fa sparire.
            io::Error::from(ErrorKind::AlreadyExists),
            // EACCES (13) su Unix: accesso negato permanente, non transitorio.
            io::Error::from_raw_os_error(13),
            io::Error::other("generico"),
        ] {
            assert!(!retryable_persist_error(&error), "{error:?}");
        }
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
    fn persist_does_not_retry_already_exists_nor_non_transient_errors() {
        let casi: [fn() -> io::Error; 2] = [
            || io::Error::from(ErrorKind::AlreadyExists),
            || io::Error::from_raw_os_error(13), // EACCES: non transitorio
        ];
        for errore in casi {
            let attempts = AtomicU32::new(0);
            let result = persist_with_retry(|| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(errore())
            });
            assert!(result.is_err());
            assert_eq!(attempts.load(Ordering::SeqCst), 1, "{:?}", errore());
        }
    }

    // -- Profili di publish ---------------------------------------------------

    #[test]
    fn publish_outcome_maps_on_the_remote_effect_axis() {
        // R9.1/R9.6: l'esito tipizzato di errori-e-limiti.md#publish-e-cleanup vive
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
        assert_eq!(outcome.durabilita, PublishOutcome::Published);
        assert_eq!(
            outcome.pulizia,
            PuliziaDelTemporaneo::Rimosso,
            "il rename atomico non lascia niente"
        );
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
        // Su Unix il fsync di directory e' supportato; su Windows no (errori-e-limiti.md#publish-e-cleanup).
        #[cfg(unix)]
        assert_eq!(outcome.durabilita, PublishOutcome::Published);
        assert_eq!(
            outcome.pulizia,
            PuliziaDelTemporaneo::Rimosso,
            "il rename atomico non lascia niente"
        );
        #[cfg(not(unix))]
        assert_eq!(
            outcome.durabilita,
            PublishOutcome::PublishedButDurabilityUnconfirmed
        );
        assert_eq!(std::fs::read(&destination).expect("lettura"), b"dati");
    }

    #[test]
    fn il_valore_della_closure_torna_e_la_prima_pubblicazione_vince() {
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("output.bin");
        let value = pubblica_ignorando_l_esito(&destination, |writer| {
            writer.write_all(b"legacy")?;
            Ok(7)
        })
        .expect("publish legacy");
        assert_eq!(value, 7);
        assert_eq!(std::fs::read(&destination).expect("lettura"), b"legacy");

        // No-clobber: la seconda pubblicazione sullo stesso path fallisce,
        // e la prima resta.
        let result = pubblica_ignorando_l_esito(&destination, |writer| {
            writer.write_all(b"altro")?;
            Ok(())
        });
        let error = result.expect_err("no-clobber");
        assert_eq!(error.phase(), ErrorPhase::Commit);
        assert!(matches!(error.untag(), PlenoraError::Conflict(_)));
        assert_eq!(std::fs::read(&destination).expect("lettura"), b"legacy");
    }

    // -- Crash simulato tra scrittura e persist (errori-e-limiti.md#publish-e-cleanup) ----------------------

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
            !destination.try_exists().expect("stat"),
            "nessun file visibile alla destinazione"
        );
        // Il tempfile `.partial` e' ripulito dal Drop: directory vuota.
        let leftovers = std::fs::read_dir(directory.path())
            .expect("read_dir")
            .count();
        assert_eq!(leftovers, 0, "il tempfile e' ripulito");
    }

    // -- Tagging di fase al confine di publish (BLOCK-03, piano-v5.md#contratti-di-input) --------

    /// Una destinazione che non si lascia osservare non e' libera: `Io`, fase
    /// `Probe`. Sotto un file regolare `stat` fallisce con `ENOTDIR`, che non
    /// e' `NotFound`; su Windows lo stesso percorso risponde «non trovato».
    #[cfg(unix)]
    #[test]
    fn una_destinazione_non_osservabile_e_io_in_fase_probe() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("file");
        std::fs::write(&file, b"x").expect("fixture");
        let error = verifica_destinazione_libera(&file.join("output.bin"))
            .expect_err("destinazione non osservabile");
        assert_eq!(error.category(), plenora_core::ErrorCategory::Io, "{error}");
        assert_eq!(error.phase_tag(), Some(ErrorPhase::Probe), "{error}");
    }

    #[test]
    fn io_at_tags_each_publish_phase_without_changing_text_or_axes() {
        // I punti I/O del confine (creazione tempfile, flush/sync, persist)
        // portano la fase esatta; testo, categoria e retry attraversano il
        // wrapper invariati (Io: categoria Io, disposizione Safe).
        for phase in [ErrorPhase::Write, ErrorPhase::Finalize, ErrorPhase::Commit] {
            let error = io_at(phase, io::Error::other("disco pieno"));
            assert_eq!(error.phase(), phase, "{error}");
            assert_eq!(error.to_string(), "io error: disco pieno");
            assert_eq!(error.category(), plenora_core::ErrorCategory::Io);
            assert_eq!(
                error.retry_disposition(),
                plenora_core::RetryDisposition::Safe
            );
        }
    }

    /// **Una destinazione occupata e' un conflitto, non un piano invalido.**
    ///
    /// La variante e' documentata per questo caso — «destinazione gia'
    /// esistente o conflitto di scrittura» — e il piano non ha niente di
    /// sbagliato: e' il posto a essere occupato.
    #[test]
    fn existing_output_is_a_commit_phase_conflict() {
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("output.bin");
        std::fs::write(&destination, b"vecchio").expect("fixture");
        let error = publish_with_profile(&destination, PublishProfile::Atomic, |writer| {
            writer.write_all(b"nuovo")?;
            Ok(())
        })
        .expect_err("no-clobber");
        assert_eq!(error.phase(), ErrorPhase::Commit);
        assert_eq!(error.phase_tag(), Some(ErrorPhase::Commit));
        assert_eq!(
            error.to_string(),
            format!("conflict: output gia' esistente: {}", destination.display())
        );
        assert_eq!(error.category(), plenora_core::ErrorCategory::Conflict);
        assert_eq!(
            error.retry_disposition(),
            plenora_core::RetryDisposition::Never
        );
        assert!(matches!(error.untag(), PlenoraError::Conflict(_)));
        assert_eq!(
            std::fs::read(&destination).expect("lettura"),
            b"vecchio",
            "e cio' che c'era non e' stato toccato"
        );
    }

    /// **La stessa classe se la collisione si scopre al commit.**
    ///
    /// La destinazione compare **mentre** si scrive: il controllo preliminare
    /// non la vede, e la classe deve essere la stessa.
    #[test]
    fn a_destination_appearing_during_the_write_is_the_same_conflict() {
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("output.bin");
        // La destinazione non c'e' quando si entra: il controllo preliminare
        // passa. Compare dentro la closure, cioe' dopo.
        let error = publish_with_profile(&destination, PublishProfile::Atomic, |writer| {
            writer.write_all(b"nuovo")?;
            std::fs::write(&destination, b"arrivato prima").map_err(PlenoraError::Io)?;
            Ok(())
        })
        .expect_err("il persist trova il nome occupato");
        assert_eq!(
            error.category(),
            plenora_core::ErrorCategory::Conflict,
            "scoperta al commit, stessa classe del controllo preliminare"
        );
        assert_eq!(error.phase(), ErrorPhase::Commit);
        assert_eq!(
            std::fs::read(&destination).expect("lettura"),
            b"arrivato prima",
            "chi e' arrivato prima resta"
        );
    }

    #[test]
    fn missing_output_directory_is_a_probe_phase_error() {
        // Riconoscimento preliminare della destinazione: fase Probe.
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("assente").join("output.bin");
        let error = pubblica_ignorando_l_esito(&destination, |writer| {
            writer.write_all(b"x")?;
            Ok(())
        })
        .expect_err("directory inesistente");
        assert_eq!(error.phase(), ErrorPhase::Probe);
        assert_eq!(error.phase_tag(), Some(ErrorPhase::Probe));
        assert!(matches!(error.untag(), PlenoraError::InvalidPlan(_)));
    }

    #[cfg(windows)]
    #[test]
    fn unc_target_is_a_probe_phase_error() {
        // Windows: il riconoscimento fail-closed rifiuta i percorsi UNC —
        // la destinazione non supportata cade su Probe, la fase in cui la si
        // scopre.
        let error = ensure_supported_publish_target(Path::new("//server/share"))
            .expect_err("UNC rifiutato");
        assert_eq!(error.phase(), ErrorPhase::Probe);
        assert_eq!(error.category(), plenora_core::ErrorCategory::Unsupported);
        assert!(matches!(error.untag(), PlenoraError::Unsupported(_)));
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    #[test]
    fn unimplemented_target_probe_is_a_probe_phase_error() {
        // Unix non-Linux: riconoscimento non implementato, rifiuto
        // fail-closed — stessa fase Probe della destinazione non supportata.
        let directory = tempfile::tempdir().expect("tempdir");
        let error = ensure_supported_publish_target(directory.path())
            .expect_err("riconoscimento non implementato");
        assert_eq!(error.phase(), ErrorPhase::Probe);
        assert!(matches!(error.untag(), PlenoraError::Unsupported(_)));
    }

    #[test]
    fn write_closure_errors_are_propagated_untagged() {
        // Regressione: gli errori della closure nascono nel chiamante e
        // restano derivati per variante (DataMapping -> Write), senza tag.
        let directory = tempfile::tempdir().expect("tempdir");
        let destination = directory.path().join("output.bin");
        let error =
            pubblica_ignorando_l_esito(&destination, |_writer| -> Result<(), PlenoraError> {
                Err(PlenoraError::DataMapping("errore del chiamante".into()))
            })
            .expect_err("closure fallita");
        assert_eq!(error.phase(), ErrorPhase::Write);
        assert_eq!(error.phase_tag(), None, "nessun tag: fase derivata");
        assert_eq!(error.to_string(), "errore del chiamante");
    }

    // -- Verifica CRS dei comandi (fail-closed senza backend PROJ) --------

    #[cfg(not(feature = "proj-backend"))]
    #[test]
    fn transform_arrow_crs_is_required_and_never_trusted_without_backend() {
        // CRS assente: errore prima ancora della risoluzione.
        let missing: TransformArrowSchema = serde_json::from_value(serde_json::json!({
            "schema_version": TransformArrowSchema::VERSION,
            "operation": "centroid",
            "row_count": 0
        }))
        .expect("schema");
        let result = validate_transform_arrow_crs(&missing);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");

        // CRS dichiarato ma nessun backend compilato: la dichiarazione non
        // viene mai creduta — fail-closed, non validazione ottimistica.
        let declared: TransformArrowSchema = serde_json::from_value(serde_json::json!({
            "schema_version": TransformArrowSchema::VERSION,
            "operation": "centroid",
            "row_count": 0,
            "crs": "EPSG:32632"
        }))
        .expect("schema");
        let result = validate_transform_arrow_crs(&declared);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
    }

    #[cfg(not(feature = "proj-backend"))]
    #[test]
    fn pair_arrow_crs_are_both_required_and_never_trusted_without_backend() {
        let base = serde_json::json!({
            "schema_version": PairArrowSchema::VERSION,
            "operation": "sjoin",
            "left_row_count": 0,
            "right_row_count": 0
        });
        // left_crs assente.
        let schema: PairArrowSchema = serde_json::from_value(base.clone()).expect("schema");
        let result = validate_pair_arrow_crs(&schema);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
        // right_crs assente (left presente).
        let mut with_left = base.clone();
        with_left["left_crs"] = serde_json::json!("EPSG:32632");
        let schema: PairArrowSchema = serde_json::from_value(with_left).expect("schema");
        let result = validate_pair_arrow_crs(&schema);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
        // Entrambi dichiarati, nessun backend: fail-closed.
        let mut both = base;
        both["left_crs"] = serde_json::json!("EPSG:32632");
        both["right_crs"] = serde_json::json!("EPSG:32632");
        let schema: PairArrowSchema = serde_json::from_value(both).expect("schema");
        let result = validate_pair_arrow_crs(&schema);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
    }

    // -- Filesystem di destinazione non identificabile (errori-e-limiti.md#publish-e-cleanup) -------------

    #[cfg(target_os = "linux")]
    #[test]
    fn unidentifiable_filesystem_is_rejected_fail_closed() {
        // procfs (f_type 0x9fa0) non e' nella whitelist dei filesystem
        // locali: in dubbio, rifiutare — prima di creare qualunque tempfile.
        let destination = Path::new("/proc/plenora-publish-non-deve-esistere.bin");
        let error = pubblica_ignorando_l_esito(destination, |writer| {
            writer.write_all(b"x")?;
            Ok(())
        })
        .expect_err("procfs fuori whitelist");
        assert_eq!(error.phase(), ErrorPhase::Probe);
        assert_eq!(error.category(), plenora_core::ErrorCategory::Unsupported);
        assert!(
            error.to_string().contains("filesystem non identificabile"),
            "{error}"
        );
        assert!(matches!(error.untag(), PlenoraError::Unsupported(_)));
        assert!(
            !destination.try_exists().expect("stat"),
            "nessuna pubblicazione parziale"
        );
    }
}
