//! Il dominio di isolamento: si costruisce, si rilegge, e se non regge non si
//! parte.
//!
//! Qui nasce il dominio, non chi lo usa: `PreparaIsolamento` e' il primo nodo
//! della macchina a stati del supervisore (isolamento.md#31-supervisore).
//!
//! - Il limite e' in vigore **prima** che esista un processo da limitare
//!   (`F4-1`, `GA-7`): applicarlo dopo lascia una finestra senza tetto.
//! - Ogni proprieta' si scrive e **si rilegge**, e il profilo isolato non parte
//!   se una sola diverge
//!   (isolamento.md#9-bis-preflight-del-dominio-scrivere-non-e-configurare).
//! - Prima si accerta **dove** si scrive (percorso, control plane, `cgroup2`)
//!   e **chi** potrebbe disfarlo, poi si scrive: un preflight che scopre a
//!   meta' di essere nel posto sbagliato non puo' piu' tornare indietro.
//! - Il possesso si giudica su ogni antenato fino alla radice del control
//!   plane, perche' si evade scrivendo il `cgroup.procs` di un altro cgroup.
//! - Il trait `SuperficieDominio` prova ovunque ordine, riletture, parsing e
//!   giudizio sui permessi; che il kernel onori le scritture si prova solo su
//!   una gerarchia vera.

use std::path::{Path, PathBuf};

use plenora_core::error::{ErrorPhase, PlenoraError, Result};

// La domanda "chi puo' chiedere l'isolamento, e a quale tetto" (`PR-12`) e'
// distinta dal dominio che questo modulo prepara: la piattaforma si verifica
// in validazione, prima di ogni cgroup, e il giudizio e' una regola pura —
// si prova ovunque, come `worker`. Solo la lettura della politica dell'host
// e' di Linux, e lo dichiara sul singolo elemento.
pub mod attivazione;
#[cfg(target_os = "linux")]
mod canale;
#[cfg(target_os = "linux")]
mod dominio;
// Il chiamante di produzione (`PR-12`): collega dominio, spawner,
// protocollo, verifica e pubblicazione per una richiesta reale. Di Linux
// soltanto, come il dominio che prepara.
#[cfg(target_os = "linux")]
pub mod esecuzione_isolata;
// La guardia sul figlio non ha niente di Linux: `std::process::Child` esiste
// ovunque, e cio' che la guardia fissa — le sue porte e la sentinella — e' una
// regola di proprieta', non di sistema. I suoi casi su processi veri restano
// sotto `cfg(unix)`, perche' quelli il sistema lo toccano.
mod figlio;
#[cfg(target_os = "linux")]
mod identita;
// La lettura fermabile serve a chiunque ascolti un canale senza poter restare
// fermo dentro una `read`: il supervisore e anche il worker, che deve sentire
// un `Annulla` mentre lavora.
#[cfg(target_os = "linux")]
mod lettura;
// La macchina a stati del supervisore: fatti in una coda, un solo giudice.
//
// Niente `cfg` sul modulo: il chiamante di produzione e'
// `esecuzione_isolata::esegui_isolato` tramite `macchina::conduci_isolato`, e
// il giudizio su cio' che attraversa il confine si prova ovunque. Cio' che
// tocca descrittori e file del dominio e' di Linux e lo dichiara sui singoli
// elementi.
mod macchina;
#[cfg(all(target_os = "linux", qualificazione_isolamento))]
pub mod qualificazione;
mod sorgente;
// Il percorso che fa percorrere a un worker **reale** la sequenza intera. Il
// chiamante di produzione (`esecuzione_isolata`) ne riusa `supervisore_per`,
// `digest_dell_immagine`, `scrivi`, `chiudi` e `con_la_pulizia`; `dialoga` resta
// della qualificazione. Di Linux soltanto, come il dominio.
#[cfg(target_os = "linux")]
pub mod prova;
#[cfg(target_os = "linux")]
mod spawner;
mod worker;
// Il verificatore, dal confine: mirror di `worker` per il ruolo che rilegge
// invece di eseguire. Di Linux soltanto: riapre i tre descrittori ereditati
// (le due pipe piu' l'artefatto) con `canale::riapri_accertato` e
// `canale::riapri_accertato_artefatto`, che sono entrambi di Linux.
#[cfg(target_os = "linux")]
mod verificatore;

#[cfg(test)]
mod tests;

/// Le quattro proprieta' che il preflight **scrive** e rilegge.
///
/// Sono quattro e non di piu' perche' ciascuna copre un modo distinto in cui
/// l'attribuzione si perde, e nessuna copre quello di un'altra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Controllo {
    /// `memory.max`: il tetto. Senza, il dominio non e' limitato.
    Tetto,
    /// `memory.swap.max` a zero. Con lo swap il tetto misurerebbe un'altra cosa
    /// — la memoria residente invece di quella richiesta — e l'attribuzione
    /// parlerebbe di una grandezza che non e' quella governata.
    Swap,
    /// `memory.oom.group` a uno. Senza, un OOM parziale e' indistinguibile da
    /// un successo (`F4-8`): un figlio ucciso per il limite lascia il capofila
    /// vivo, con uscita zero, cioe' un successo apparente sopra un guasto di
    /// risorse.
    GroupKill,
    /// `cgroup.max.depth` a zero: il sigillo. Senza, il worker puo' creare
    /// discendenti e uscire dall'osservazione (`F4-9`).
    Sigillo,
}

impl Controllo {
    /// Il nome del file nella gerarchia.
    const fn file(self) -> &'static str {
        match self {
            Self::Tetto => "memory.max",
            Self::Swap => "memory.swap.max",
            Self::GroupKill => "memory.oom.group",
            Self::Sigillo => "cgroup.max.depth",
        }
    }

    /// Il valore che il preflight scrive, per i tre che non dipendono dal
    /// piano.
    const fn valore_fisso(self) -> Option<&'static str> {
        match self {
            Self::Tetto => None,
            Self::Swap | Self::Sigillo => Some("0"),
            Self::GroupKill => Some("1"),
        }
    }

    /// L'ordine in cui i quattro si scrivono.
    ///
    /// Il tetto per primo: e' l'unico che dipende dal piano, ed e' quello la
    /// cui assenza lascia il dominio senza limite.
    const ORDINE: [Self; 4] = [Self::Tetto, Self::Swap, Self::GroupKill, Self::Sigillo];
}

/// I file **del dominio** il cui possesso decide se il worker ha autorita'.
const FILE_DEL_DOMINIO: [&str; 5] = [
    "cgroup.procs",
    "memory.max",
    "memory.swap.max",
    "memory.oom.group",
    "cgroup.max.depth",
];

/// I bersagli del giudizio sul possesso: il dominio e **ogni antenato** fino
/// alla radice del control plane, ciascuno con la propria directory e il
/// proprio `cgroup.procs`.
///
/// Si evade scrivendo il `cgroup.procs` di un **altro** cgroup (il padre, un
/// fratello), o creando un cgroup nuovo in una directory scrivibile: il solo
/// dominio non basta. La catena si ferma alla radice del control plane,
/// inclusa: sopra c'e' l'amministrazione della macchina.
fn bersagli_del_possesso(dominio: &Path, radice: &Path) -> Vec<PathBuf> {
    let mut bersagli = vec![dominio.to_path_buf()];
    for file in FILE_DEL_DOMINIO {
        bersagli.push(dominio.join(file));
    }
    // Se il dominio **e'** la radice non c'e' nessun antenato da giudicare, e
    // salirne uno significherebbe uscire dal perimetro dichiarato: la catena
    // parte dal padre e si ferma alla radice, ma qui il padre e' gia' sopra di
    // essa. Senza questo ramo il ciclo non incontrerebbe mai la condizione di
    // arresto e salirebbe fino a `/`.
    if dominio == radice {
        return bersagli;
    }
    let mut corrente = dominio.parent();
    while let Some(antenato) = corrente {
        bersagli.push(antenato.to_path_buf());
        bersagli.push(antenato.join("cgroup.procs"));
        if antenato == radice {
            break;
        }
        corrente = antenato.parent();
    }
    bersagli
}

/// L'identita' con cui il worker girera'.
///
/// Serve al preflight **prima** che il worker esista: il giudizio su
/// proprietario e permessi si fa contro l'identita' che avra', non contro
/// quella di chi prepara.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IdentitaWorker {
    uid: u32,
    gid: u32,
}

/// Proprietario e permessi di un percorso, come il filesystem li riporta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProprietaFile {
    uid: u32,
    gid: u32,
    mode: u32,
}

impl ProprietaFile {
    /// Se un worker con quella identita' **potrebbe** scrivere questo file.
    ///
    /// Il bit di scrittura del gruppo si rifiuta sempre, GID a parte: su un
    /// filesystem con ACL e' la **mask della classe ACL**, e una voce
    /// `user:<worker>:rw` varrebbe anche con un GID estraneo. Rifiutare invece
    /// di interpretare le ACL come il kernel costa qualche rifiuto su gerarchie
    /// che non useremmo comunque. La regola si allenta solo con una prova che
    /// `cgroup2`, nell'ambiente qualificato, non supporti ACL nominative.
    ///
    /// Resta ammesso il bit del proprietario, quando il proprietario **non
    /// e'** il worker: il caso normale di una gerarchia del control plane.
    const fn scrivibile_da(self, worker: IdentitaWorker) -> bool {
        if self.mode & 0o022 != 0 {
            return true;
        }
        self.uid == worker.uid && self.mode & 0o200 != 0
    }
}

/// Il montaggio `cgroup2` che contiene il dominio.
///
/// Non «il primo `cgroup2` che si incontra»: con piu' montaggi, o con un bind
/// mount, registrarne uno e calcolare l'appartenenza su un altro significa dire
/// due cose su due filesystem diversi credendo di parlare dello stesso.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Montaggio {
    /// Il punto di mount, che e' il prefisso da togliere per ottenere il
    /// percorso **dentro** la gerarchia.
    punto: PathBuf,
    /// La radice del mount dentro il proprio filesystem: con un bind mount di
    /// un sottoalbero non e' `/`, e ignorarla sposta il percorso calcolato di
    /// tutto il ramo.
    radice: PathBuf,
    /// Le opzioni **del mount**: `rw`, `nosuid`, `relatime`, la propagazione.
    ///
    /// Non sono quelle in cui cercare `memory_localevents`, e la distinzione
    /// non e' accademica: quella opzione governa il **superblocco**, quindi il
    /// kernel la riporta nell'ultimo campo e non qui. Cercarla nel campo
    /// sbagliato darebbe sempre «assente», e l'assenza e' proprio la risposta
    /// che fa proseguire.
    opzioni_mount: String,
    /// Le opzioni **del superblocco**: `nsdelegate`, `memory_recursiveprot`,
    /// `memory_localevents`. E' qui che vive cio' che cambia la semantica di
    /// `memory.events`.
    opzioni_superblocco: String,
    /// L'identita' del filesystem, `major:minor`: e' cio' che permette di
    /// riconoscere lo **stesso** filesystem raggiunto per un altro percorso.
    dispositivo: String,
}

// Nessun `cfg` di perimetro: tutto cio' che sta qui ha un chiamante di
// produzione (dispatch anticipato dello spawner, `esecuzione_isolata`). Cio'
// che resta sotto `cfg` altrove lo dichiara elemento per elemento; si
// verifica togliendo i `cfg` e costruendo senza `internals` con `-D dead-code`.
//
// Registro: errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.

/// Cio' che una superficie puo' non riuscire a fare.
///
/// Non e' un `PlenoraError`: il difetto e' meccanico, e diventa
/// `IsolationUnavailable` solo quando il preflight lo decide. Tenerli separati
/// impedisce a una superficie di decidere al posto del preflight.
///
/// Porta l'`ErrorKind` perche' «il file non c'e'» (un'assenza, che non
/// concede autorita') e «il file non si legge» (un dubbio, che si rifiuta)
/// hanno conseguenze opposte, e ricavarli dal testo dipenderebbe dalla locale.
#[derive(Debug)]
enum DifettoSuperficie {
    /// La scrittura non e' riuscita.
    ///
    /// Solo il supervisore scrive: lo spawner rilegge e basta.
    Scrittura { cosa: String, causa: std::io::Error },
    /// La lettura non e' riuscita.
    Lettura { cosa: String, causa: std::io::Error },
    /// Il contenuto non ha la forma attesa.
    ///
    /// Distinto dagli altri due perche' qui il file **risponde**: manda a
    /// guardare il formato, non i permessi ne' il kernel.
    Forma(String),
}

impl DifettoSuperficie {
    /// Se il difetto dice che l'oggetto **non c'e'**.
    fn e_assenza(&self) -> bool {
        match self {
            Self::Lettura { causa, .. } | Self::Scrittura { causa, .. } => {
                matches!(causa.kind(), std::io::ErrorKind::NotFound)
            }
            Self::Forma(_) => false,
        }
    }

    /// Un difetto di lettura con la sua causa.
    fn lettura(cosa: impl Into<String>, causa: std::io::Error) -> Self {
        Self::Lettura {
            cosa: cosa.into(),
            causa,
        }
    }

    /// Un difetto di scrittura con la sua causa.
    fn scrittura(cosa: impl Into<String>, causa: std::io::Error) -> Self {
        Self::Scrittura {
            cosa: cosa.into(),
            causa,
        }
    }
}

impl std::fmt::Display for DifettoSuperficie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scrittura { cosa, causa } => write!(f, "scrittura fallita: {cosa}: {causa}"),
            Self::Lettura { cosa, causa } => write!(f, "lettura fallita: {cosa}: {causa}"),
            Self::Forma(motivo) => write!(f, "forma inattesa: {motivo}"),
        }
    }
}

type Esito<T> = std::result::Result<T, DifettoSuperficie>;

/// La superficie su cui il preflight agisce.
///
/// E' deliberatamente povera: nessuna delle sue operazioni sa perche' viene
/// chiamata. Una superficie che conoscesse i controlli potrebbe decidere da se'
/// che cosa e' accettabile, e la decisione tornerebbe a essere distribuita fra
/// due posti.
trait SuperficieDominio {
    /// Il dominio, in forma **canonica**.
    ///
    /// Ogni confronto successivo (montaggio, antenati) si fa per prefisso: un
    /// `..` o un link simbolico lo farebbe su un percorso diverso da quello a
    /// cui si scrive.
    ///
    /// # Errors
    ///
    /// [`DifettoSuperficie::Lettura`] se non si risolve: un dominio che non
    /// esiste non e' un dominio da preparare.
    fn dominio(&self) -> Esito<PathBuf>;

    /// La radice del control plane, canonica: il livello piu' alto fino a cui
    /// il possesso va giudicato.
    ///
    /// # Errors
    ///
    /// [`DifettoSuperficie::Lettura`].
    fn radice_control_plane(&self) -> Esito<PathBuf>;

    /// Il montaggio `cgroup2` che contiene quel percorso.
    ///
    /// # Errors
    ///
    /// [`DifettoSuperficie::Lettura`] se non ce n'e' uno, se ce n'e' piu' d'uno
    /// che lo contiene ugualmente bene, o se `mountinfo` non si interpreta.
    fn montaggio(&self, dominio: &Path) -> Esito<Montaggio>;

    /// Proprietario e permessi di un percorso.
    ///
    /// # Errors
    ///
    /// [`DifettoSuperficie::Lettura`].
    fn proprieta(&self, percorso: &Path) -> Esito<ProprietaFile>;

    /// I namespace del processo che prepara.
    ///
    /// # Errors
    ///
    /// [`DifettoSuperficie::Lettura`].
    fn namespace(&self) -> Esito<Vec<(String, String)>>;

    /// Scrive un valore nel file del controllo.
    ///
    /// # Errors
    ///
    /// [`DifettoSuperficie::Scrittura`].
    fn scrivi(&mut self, controllo: Controllo, valore: &str) -> Esito<()>;

    /// Rilegge il file del controllo, senza interpretarlo.
    ///
    /// # Errors
    ///
    /// [`DifettoSuperficie::Lettura`].
    fn rileggi(&self, controllo: Controllo) -> Esito<String>;

    /// Il contenuto di `cgroup.events`.
    ///
    /// # Errors
    ///
    /// [`DifettoSuperficie::Lettura`]. Che il file **non** sia leggibile e' gia'
    /// un esito: senza quel segnale la barriera di quiescenza non e'
    /// implementabile.
    fn eventi(&self) -> Esito<String>;
}

/// Cio' che il preflight ha accertato, e **l'unica** via per avviare qualcosa
/// dentro il dominio.
///
/// Il preflight accerta una **combinazione** (quel dominio, sotto quella
/// radice, su quel montaggio, con quei namespace, contro quell'identita'), e
/// campi indipendenti permetterebbero di verificarne una ed eseguirne
/// un'altra. Si costruisce **solo** dentro [`prepara_dominio`], non ha campi
/// ricombinabili, e lo spawner lo consuma. Non e' un booleano: porta cio' che
/// il preflight ha osservato, che serve allo spawner e all'evidenza.
#[derive(Debug, PartialEq, Eq)]
struct DominioPreparato {
    /// Il dominio, **canonico**: e' su questo che si scrive, e non sul
    /// percorso che il chiamante ha nominato.
    dominio: PathBuf,
    /// La radice del control plane, canonica.
    radice: PathBuf,
    /// L'identita' del worker giudicata contro i permessi di questa gerarchia.
    worker: IdentitaWorker,
    /// Il tetto riletto, in byte.
    tetto_byte: u64,
    /// Il montaggio che contiene il dominio. Lo spawner calcola l'appartenenza
    /// **su questo**, non su un percorso convenzionale.
    montaggio: Montaggio,
    /// I namespace del processo che ha preparato il dominio.
    ///
    /// Lo spawner li pretende **identici** ai propri prima della `exec`: li
    /// eredita dal supervisore, quindi una differenza significa una `unshare`
    /// nel mezzo.
    ///
    /// Il controllo vale al momento della `exec`, non dopo: `no_new_privs` non
    /// vieta `unshare`, e un worker puo' creare uno user namespace con
    /// capability piene dove la policy del kernel lo consente. La prova ostile
    /// accetta quindi due esiti: `unshare` rifiutata dall'host, oppure riuscita
    /// ma senza poter riscrivere il control plane ne' uscire dal dominio,
    /// perche' i file della gerarchia appartengono a un UID non mappato nel
    /// namespace nuovo e il dominio e' sigillato.
    namespace_attesi: Vec<(String, String)>,
    /// Se fra le opzioni di superblocco c'e' `memory_localevents`.
    ///
    /// **Registrato, non rifiutato.** L'opzione rende non gerarchico anche
    /// `memory.events`, ma con il dominio sigillato non ci sono discendenti e
    /// locale e gerarchico coincidono. La conclusione dipende dal sigillo ed e'
    /// un ragionamento, non un'osservazione: il caso non e' mai stato misurato,
    /// per questo il valore si registra.
    eventi_locali: bool,
}

/// Dove sta il dominio e chi puo' toccarlo.
///
/// Supervisore e spawner lo accertano con questa sola funzione: stesso
/// ordine, stesse condizioni, stesse ragioni di rifiuto. Due copie
/// divergerebbero.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], col nome di cio' che ha ceduto.
fn accerta_perimetro<S: SuperficieDominio>(
    superficie: &S,
    worker: IdentitaWorker,
) -> Result<(PathBuf, PathBuf, Montaggio)> {
    // --- dove: il percorso, e che sia davvero cgroup2 ----------------------
    let dominio = superficie
        .dominio()
        .map_err(|difetto| non_disponibile("dominio", &difetto.to_string()))?;
    let radice = superficie
        .radice_control_plane()
        .map_err(|difetto| non_disponibile("control plane", &difetto.to_string()))?;
    if !dominio.starts_with(&radice) {
        return Err(non_disponibile(
            "dominio",
            &format!(
                "{} non sta sotto la radice del control plane {}",
                dominio.display(),
                radice.display()
            ),
        ));
    }
    // Che il montaggio esista, sia unico e sia `cgroup2` e' cio' che la
    // selezione stessa accerta: se rende un montaggio, quel montaggio e'
    // cgroup2 e contiene il dominio.
    let montaggio = superficie
        .montaggio(&dominio)
        .map_err(|difetto| non_disponibile("montaggio cgroup2", &difetto.to_string()))?;

    // --- chi: il possesso, sempre prima delle scritture --------------------
    for bersaglio in bersagli_del_possesso(&dominio, &radice) {
        let nome = bersaglio.display().to_string();
        let proprieta = superficie
            .proprieta(&bersaglio)
            .map_err(|difetto| non_disponibile(&nome, &difetto.to_string()))?;
        if proprieta.scrivibile_da(worker) {
            return Err(non_disponibile(
                &nome,
                &format!(
                    "il worker {}:{} potrebbe scriverlo (proprietario {}:{}, mode {:o}): \
                     l'identita' distinta non serve se i permessi la annullano",
                    worker.uid,
                    worker.gid,
                    proprieta.uid,
                    proprieta.gid,
                    proprieta.mode & 0o777
                ),
            ));
        }
    }

    Ok((dominio, radice, montaggio))
}

/// Che il dominio sia vuoto, secondo `cgroup.events`.
///
/// Non si scrive niente: si pretende che il segnale **esista** e dica che il
/// dominio e' vuoto. Un dominio gia' popolato prima dell'avvio non e' il
/// nostro, e un `cgroup.events` illeggibile toglie la barriera su cui il
/// supervisore aspetta la fine del worker.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`].
fn accerta_quiescenza<S: SuperficieDominio>(superficie: &S) -> Result<()> {
    let eventi = superficie
        .eventi()
        .map_err(|difetto| non_disponibile("cgroup.events", &difetto.to_string()))?;
    match popolato(&eventi) {
        Ok(false) => Ok(()),
        Ok(true) => Err(non_disponibile(
            "cgroup.events",
            "populated e' 1, atteso 0: il dominio non e' vuoto",
        )),
        Err(motivo) => Err(non_disponibile("cgroup.events", motivo)),
    }
}

/// Prepara il dominio e accerta che il worker non possa disfarlo.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], in fase [`ErrorPhase::Prepare`], con
/// il nome di cio' che ha ceduto e in che modo.
pub(super) fn prepara_dominio<S: SuperficieDominio>(
    superficie: &mut S,
    tetto_byte: u64,
    worker: IdentitaWorker,
) -> Result<DominioPreparato> {
    let (dominio, radice, montaggio) = accerta_perimetro(superficie, worker)?;

    let namespace_attesi = superficie
        .namespace()
        .map_err(|difetto| non_disponibile("namespace", &difetto.to_string()))?;

    // --- che cosa: i quattro controlli, uno per volta ----------------------
    //
    // Ogni controllo si scrive **e si rilegge** prima che il successivo venga
    // toccato. Scriverli tutti e poi rileggerli tutti sarebbe piu' breve e
    // direbbe meno: una scrittura che fallisce a meta' lascerebbe il dominio in
    // uno stato che nessuna rilettura successiva sa distinguere da uno mai
    // toccato.
    let tetto = tetto_byte.to_string();
    for controllo in Controllo::ORDINE {
        let atteso = controllo.valore_fisso().unwrap_or(tetto.as_str());
        superficie
            .scrivi(controllo, atteso)
            .map_err(|difetto| non_disponibile(controllo.file(), &difetto.to_string()))?;
        let riletto = superficie
            .rileggi(controllo)
            .map_err(|difetto| non_disponibile(controllo.file(), &difetto.to_string()))?;
        if riletto.trim() != atteso {
            return Err(non_disponibile(
                controllo.file(),
                &format!("scritto {atteso}, riletto {}", riletto.trim()),
            ));
        }
    }

    // --- quiescenza --------------------------------------------------------
    accerta_quiescenza(superficie)?;

    Ok(DominioPreparato {
        dominio,
        radice,
        worker,
        tetto_byte,
        eventi_locali: opzione_presente(&montaggio.opzioni_superblocco, "memory_localevents"),
        montaggio,
        namespace_attesi,
    })
}

/// Un descrittore letto dalla riga di comando, in **forma canonica**.
///
/// Il solo produttore e' il supervisore, che scrive `i32::to_string()`: ogni
/// altra forma che `str::parse` accetterebbe (`+3`, `03`, `-0`, `-01`) non
/// viene da lui, e si rifiuta. Gli spazi li rifiuta gia' `parse`.
///
/// Non decide se il numero vada bene: `-1` ha forma canonica e lo rifiuta
/// [`canale::numero_ammissibile`], con la ragione giusta («non e' un
/// descrittore», non «non e' un numero»).
///
/// # Errors
///
/// Il motivo, in forma di frase.
fn descrittore_canonico(testo: &str) -> std::result::Result<i32, String> {
    let cifre = testo.strip_prefix('-').unwrap_or(testo);
    if cifre.is_empty() || !cifre.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("«{testo}» non e' un numero decimale"));
    }
    if cifre.len() > 1 && cifre.starts_with('0') {
        return Err(format!(
            "«{testo}» ha uno zero iniziale, che la forma canonica non ha"
        ));
    }
    if testo == "-0" {
        return Err("«-0» non e' la forma canonica di zero".to_owned());
    }
    testo
        .parse()
        .map_err(|_| format!("«{testo}» non entra in un descrittore"))
}

/// Il prefisso che marca il **namespace riservato** dello spawner.
///
/// Chi entra nel namespace non torna indietro: o porta la versione
/// supportata, o riceve un rifiuto che la nomina. Riconoscere solo la
/// versione esatta farebbe cadere una versione diversa nel parser della CLI,
/// con la diagnosi sbagliata («comando sconosciuto»).
const PREFISSO_RISERVATO: &str = "plenora-spawner-";

/// Il prefisso che marca il **namespace riservato** del worker.
///
/// Vale la stessa ragione dello spawner, e non e' una ripetizione: sono due
/// namespace distinti perche' sono due modalita' distinte dello stesso
/// eseguibile, e un processo avviato come worker con una versione che questo
/// binario non conosce deve sentirsi rispondere «versione non supportata», non
/// «comando sconosciuto».
const PREFISSO_WORKER: &str = "plenora-worker-";

/// Il prefisso che marca il **namespace riservato** del verificatore.
///
/// Un terzo namespace, non una variante del worker: il verificatore non
/// esegue un piano, non riceve mai la destinazione finale e non porta
/// capability di pubblicazione. Confondere le due modalita' dietro un solo
/// prefisso avrebbe reso indistinguibile, dal solo `argv[1]`, quale delle
/// due sta per partire — e sono due superfici con autorita' diverse.
const PREFISSO_VERIFICATORE: &str = "plenora-verificatore-";

/// La versione della modalita' worker.
///
/// Cambia quando cambia cio' che il worker si aspetta di trovare al proprio
/// avvio: gli argomenti, la variabile del canale, la forma degli estremi. Non
/// e' la versione del **protocollo** sul filo, che vive nei messaggi e ha una
/// vita sua: qui si dichiara come si entra in modalita' worker, li' che cosa ci
/// si dice dopo.
const VERSIONE_WORKER: &str = "plenora-worker-1";

/// La versione della modalita' verificatore.
///
/// Stessa disciplina di [`VERSIONE_WORKER`], e stesso motivo per cui e' una
/// costante distinta e non un ramo della stessa: il verificatore riapre un
/// terzo descrittore che il worker non riceve mai, quindi «cambia cio' che
/// si aspetta di trovare al proprio avvio» e' gia' oggi una risposta diversa
/// dalle due modalita'.
const VERSIONE_VERIFICATORE: &str = "plenora-verificatore-1";

/// La versione della richiesta che attraversa il confine.
///
/// Cambia quando cambiano i campi o il loro significato; lo spawner rifiuta
/// tutto cio' che non porta **esattamente** questa stringa. Il terzo
/// descrittore, opzionale, e' l'artefatto che il verificatore riapre in sola
/// lettura, con `-1` per «assente».
const VERSIONE_RICHIESTA: &str = "plenora-spawner-3";

/// Che cosa dice il primo argomento, per la modalita' che si sta cercando.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Riconoscimento {
    /// Non e' di **questo** namespace riservato: per quanto ne sa chi ha
    /// chiesto, il programma prosegue per la sua strada.
    ///
    /// Non significa «non e' una modalita' riservata»: significa «non e'
    /// questa». Chi interroga piu' modalita' le interroga in fila, e solo
    /// l'ultima che risponde cosi' lascia davvero passare al parser della CLI.
    AltroComando,
    /// E' la versione supportata.
    Supportata,
    /// E' del namespace riservato ma non e' la versione supportata.
    ///
    /// Non porta la stringa trovata, nemmeno in copia: e' `argv`, un ingresso
    /// arbitrario (ritorni a capo, byte non UTF-8) che non deve finire nei log.
    /// Alla diagnosi serve la versione attesa, non quella trovata.
    VersioneNonSupportata,
}

/// Che cosa dice `argv[1]` rispetto a una modalita'.
///
/// Funzione pura e **multipiattaforma**: il riconoscimento e' una regola e si
/// prova ovunque. Una sola per tutte le modalita' (namespace riservato,
/// versione esatta, rifiuto che nomina la versione attesa), perche' due copie
/// divergerebbero.
fn riconosci_modalita(
    primo: Option<&std::ffi::OsString>,
    prefisso: &str,
    versione: &str,
) -> Riconoscimento {
    let Some(primo) = primo else {
        return Riconoscimento::AltroComando;
    };
    if primo == versione {
        return Riconoscimento::Supportata;
    }
    // Il confronto sul prefisso e' sui **byte**: un `argv` non e' tenuto a
    // essere UTF-8, e passare per `to_str()` farebbe cadere nel parser della
    // CLI una riga che nel namespace riservato ci sta eccome.
    if primo.as_encoded_bytes().starts_with(prefisso.as_bytes()) {
        return Riconoscimento::VersioneNonSupportata;
    }
    Riconoscimento::AltroComando
}

/// Quello che attraversa il confine fra supervisore e spawner.
///
/// E' una **richiesta**, non una prova: [`DominioPreparato`] non attraversa
/// un confine di processo, e uno spawner che credesse al mittente non
/// aggiungerebbe garanzie. Dice solo **su che cosa** lavorare (dominio,
/// radice, identita', tetto); lo spawner rivalida tutto da se'.
///
/// E' limitata perche' ogni campo e' una cosa di cui lo spawner potrebbe
/// fidarsi: il montaggio lo ritrova, i namespace li confronta con quelli del
/// proprio padre.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RichiestaSpawner {
    dominio: PathBuf,
    radice: PathBuf,
    uid: u32,
    gid: u32,
    tetto_byte: u64,
    /// Il descrittore da cui il worker **legge** cio' che il supervisore gli
    /// manda.
    ///
    /// Passa come numero perche' un descrittore non ha altra forma sul filo:
    /// attraversa le due `exec` in quanto tale, e cio' che il supervisore puo'
    /// dire e' quale numero abbia. Che quel numero sia davvero l'estremo giusto
    /// non e' affermato qui — e' **riverificato** a ogni stadio.
    worker_legge: i32,
    /// Il descrittore su cui il worker **scrive** cio' che manda al
    /// supervisore.
    worker_scrive: i32,
    /// Il descrittore dell'artefatto, aperto in sola lettura dal
    /// coordinatore, che il **verificatore** riapre.
    ///
    /// `-1` quando non c'e' artefatto, cioe' per ogni worker ordinario: la
    /// stessa forma canonica che [`descrittore_canonico`] accetta per gli
    /// altri due, e che lo spawner rivalida.
    artefatto_lettura: i32,
}

/// Nessun artefatto da cedere: la forma con cui una richiesta per il
/// **worker** dichiara il terzo descrittore assente.
const ARTEFATTO_ASSENTE: i32 = -1;

impl RichiestaSpawner {
    /// Gli argomenti con cui lo spawner viene avviato.
    fn in_argomenti(&self) -> Vec<std::ffi::OsString> {
        vec![
            std::ffi::OsString::from(VERSIONE_RICHIESTA),
            self.dominio.clone().into_os_string(),
            self.radice.clone().into_os_string(),
            std::ffi::OsString::from(self.uid.to_string()),
            std::ffi::OsString::from(self.gid.to_string()),
            std::ffi::OsString::from(self.tetto_byte.to_string()),
            std::ffi::OsString::from(self.worker_legge.to_string()),
            std::ffi::OsString::from(self.worker_scrive.to_string()),
            std::ffi::OsString::from(self.artefatto_lettura.to_string()),
        ]
    }

    /// La richiesta letta dagli argomenti, fail-closed.
    ///
    /// Si rifiutano un numero di argomenti diverso, una versione diversa, un
    /// numero che non si interpreta, un percorso relativo: nessuna forma ha
    /// una lettura di ripiego.
    ///
    /// # Errors
    ///
    /// Il motivo, in forma di frase.
    fn da_argomenti(argomenti: &[std::ffi::OsString]) -> std::result::Result<Self, String> {
        let [versione, dominio, radice, uid, gid, tetto, legge, scrive, artefatto] = argomenti
        else {
            return Err(format!(
                "la richiesta ha {} argomenti invece di 9",
                argomenti.len()
            ));
        };
        if versione != VERSIONE_RICHIESTA {
            return Err(format!(
                "versione della richiesta non riconosciuta: attesa {VERSIONE_RICHIESTA}"
            ));
        }
        // Qui `to_str` e' invece esatto, e non e' una svista che sia diverso
        // dai percorsi: un uid, un gid e un numero di byte sono cifre ASCII,
        // e un argomento che non si decodifica non e' un numero scritto male,
        // e' qualcosa che non e' un numero. Rifiutarlo e' la risposta giusta.
        let numero = |campo: &std::ffi::OsString, nome: &str| -> std::result::Result<u64, String> {
            campo
                .to_str()
                .and_then(|testo| testo.parse().ok())
                .ok_or_else(|| format!("{nome} non e' un numero"))
        };
        // Assoluto **secondo POSIX** (slash iniziale), non secondo
        // `Path::is_absolute`, che su Windows direbbe non assoluto
        // `/sys/fs/cgroup`: il percorso e' sempre di una gerarchia `cgroup2`.
        //
        // Lo slash si cerca nei **byte**: un percorso Linux non e' tenuto a
        // essere UTF-8, e `as_encoded_bytes` rappresenta i byte ASCII come se
        // stessi, quindi il controllo e' esatto su ogni piattaforma.
        let dominio = PathBuf::from(dominio);
        let radice = PathBuf::from(radice);
        for (nome, percorso) in [("il dominio", &dominio), ("la radice", &radice)] {
            if percorso.as_os_str().as_encoded_bytes().first() != Some(&b'/') {
                return Err(format!("{nome} non e' un percorso assoluto"));
            }
        }
        // I due descrittori si leggono come **interi con segno**, e non come
        // numeri naturali: un `-1` sulla riga di comando e' una cosa che puo'
        // arrivare, e leggerlo come «non e' un numero» manderebbe a cercare un
        // errore di sintassi invece di un descrittore che non esiste. Il
        // giudizio su quel valore sta in `canale::numero_ammissibile`, che ha
        // un messaggio per ciascuna ragione.
        let descrittore =
            |campo: &std::ffi::OsString, nome: &str| -> std::result::Result<i32, String> {
                let scritto = campo
                    .to_str()
                    .ok_or_else(|| format!("{nome} non e' testo decodificabile"))?;
                descrittore_canonico(scritto).map_err(|motivo| format!("{nome}: {motivo}"))
            };
        Ok(Self {
            dominio,
            radice,
            uid: u32::try_from(numero(uid, "l'uid")?).map_err(|_| "l'uid non entra in u32")?,
            gid: u32::try_from(numero(gid, "il gid")?).map_err(|_| "il gid non entra in u32")?,
            tetto_byte: numero(tetto, "il tetto")?,
            worker_legge: descrittore(legge, "il descrittore di lettura del worker")?,
            worker_scrive: descrittore(scrive, "il descrittore di scrittura del worker")?,
            // `-1` e' ammesso qui a differenza degli altri due: e' la forma
            // canonica di «nessun artefatto», non un descrittore da
            // rivalidare. `descrittore_canonico` accerta comunque la forma —
            // niente zeri iniziali, niente `-0` — prima che questa funzione
            // decida che cosa il numero significa.
            artefatto_lettura: descrittore_canonico(
                artefatto
                    .to_str()
                    .ok_or("il descrittore dell'artefatto non e' testo decodificabile")?,
            )
            .map_err(|motivo| format!("il descrittore dell'artefatto: {motivo}"))?,
        })
    }
}

/// Cio' che il preflight ha **osservato**, e che vale dopo la transizione.
///
/// E' separata dal token perche' le vite sono opposte: il token e'
/// **lineare** (si consuma una volta, e impedisce due spawner sullo stesso
/// dominio), l'evidenza e' duplicabile e persistente e deve sopravvivere
/// proprio al caso riuscito. Fra le osservazioni c'e' `memory_localevents`,
/// che il contratto promette registrato insieme al sigillo.
///
/// Non attraversa il confine: lo spawner rivalida da se' e non deve crederci.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EvidenzaPreflight {
    /// Il dominio canonico su cui il preflight ha scritto.
    dominio: PathBuf,
    /// La radice del control plane, canonica.
    radice: PathBuf,
    /// L'identita' contro cui il possesso e' stato giudicato.
    worker: IdentitaWorker,
    /// Il tetto riletto, in byte.
    tetto_byte: u64,
    /// Il montaggio `cgroup2` che contiene il dominio.
    montaggio: Montaggio,
    /// I namespace del processo che ha preparato il dominio.
    namespace_attesi: Vec<(String, String)>,
    /// Se fra le opzioni di superblocco c'e' `memory_localevents`.
    eventi_locali: bool,
}

/// I numeri dei due estremi destinati al worker, **gia' verificati**.
///
/// I due campi hanno un nome perche' due `i32` si scambiano senza che niente
/// protesti. Non c'e' costruttore aperto: l'unico modo di ottenerne uno e'
/// [`canale::accerta_coppia`], che guarda che siano due estremi di due pipe
/// distinte, nei versi giusti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NumeriDelCanale {
    legge: i32,
    scrive: i32,
}

impl NumeriDelCanale {
    /// I due numeri nella forma che la variabile del canale porta.
    ///
    /// Sta qui perche' e' la meta' che scrive cio' che il worker legge, e le
    /// due meta' devono conoscere **una** forma. E' quella canonica che il
    /// worker pretende (decimale, senza segno, senza zeri davanti), garantita
    /// da `Display` di `i32` su descrittori validi.
    fn in_variabile(self) -> std::ffi::OsString {
        std::ffi::OsString::from(format!(
            "{}{}{}",
            self.legge,
            worker::SEPARATORE,
            self.scrive
        ))
    }
}

impl DominioPreparato {
    /// Solo l'evidenza, per il cammino in cui il canale **non si e' aperto**.
    ///
    /// Senza canale non c'e' richiesta da fare, e inventarne i descrittori la
    /// renderebbe rappresentabile. Serve l'evidenza, perche' il dominio e' gia'
    /// configurato e qualcuno deve smontarlo.
    fn solo_evidenza(self) -> EvidenzaPreflight {
        EvidenzaPreflight {
            dominio: self.dominio,
            radice: self.radice,
            worker: self.worker,
            tetto_byte: self.tetto_byte,
            montaggio: self.montaggio,
            namespace_attesi: self.namespace_attesi,
            eventi_locali: self.eventi_locali,
        }
    }

    /// Smonta il token nelle sue due meta': la richiesta e l'evidenza.
    ///
    /// Consuma: un token riusabile permetterebbe due spawner sullo stesso
    /// dominio. L'evidenza esce qui perche' dopo la transizione non si
    /// ricostruisce. Vuole il canale perche' la richiesta ne porta i numeri, e
    /// una richiesta senza canale non deve essere rappresentabile.
    ///
    /// `artefatto_lettura` e' l'handle del coordinatore sull'artefatto, aperto
    /// in sola lettura, quando si avvia un **verificatore**; `None` per ogni
    /// worker ordinario. Attraversa il confine il numero grezzo, che lo
    /// spawner rivalida come i due estremi del canale.
    fn consuma(
        self,
        canale: NumeriDelCanale,
        artefatto_lettura: Option<i32>,
    ) -> (RichiestaSpawner, EvidenzaPreflight) {
        let richiesta = RichiestaSpawner {
            dominio: self.dominio.clone(),
            radice: self.radice.clone(),
            uid: self.worker.uid,
            gid: self.worker.gid,
            tetto_byte: self.tetto_byte,
            worker_legge: canale.legge,
            worker_scrive: canale.scrive,
            artefatto_lettura: artefatto_lettura.unwrap_or(ARTEFATTO_ASSENTE),
        };
        let evidenza = EvidenzaPreflight {
            dominio: self.dominio,
            radice: self.radice,
            worker: self.worker,
            tetto_byte: self.tetto_byte,
            montaggio: self.montaggio,
            namespace_attesi: self.namespace_attesi,
            eventi_locali: self.eventi_locali,
        };
        (richiesta, evidenza)
    }
}

/// Se il binario che lo spawner sta per rieseguire e' ammissibile.
///
/// Il chiamante non sceglie il binario: un percorso esterno avvierebbe
/// qualunque programma fuori dal dominio, con un `Child` indistinguibile da
/// quello di una transizione riuscita. Si riesegue il binario **in
/// esecuzione**; qui sta la regola, che si prova ovunque, separata dalla
/// lettura.
///
/// `percorso` e' il bersaglio di `/proc/self/exe`, cioe' un nome: serve a
/// nominare e a riconoscere l'immagine rimossa. `regolare` e `proprieta`
/// descrivono l'**inode in esecuzione**. L'avvio usa `/proc/self/exe`, che il
/// kernel lega all'immagine del processo: fra giudizio e `exec` non c'e'
/// risoluzione da rifare, quindi nessuna `rename` si infila nel mezzo.
///
/// Le tre condizioni:
///
/// - **non e' stata rimossa** (suffisso ` (deleted)`): l'immagine eseguita
///   sarebbe comunque giusta, ma control plane in esecuzione e su disco
///   sarebbero due programmi diversi, uno stato che nessuno ha dichiarato;
/// - **e' un file regolare**;
/// - **il worker non la puo' riscrivere**: altrimenti il prossimo avvio
///   eseguirebbe cio' che il worker ci ha messo. Il giudizio e' quello
///   conservativo dei file della gerarchia, sull'inode.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], col nome del percorso e la
/// condizione che manca.
fn spawner_ammissibile(
    percorso: &Path,
    regolare: bool,
    proprieta: ProprietaFile,
    worker: IdentitaWorker,
) -> Result<()> {
    let nome = percorso.display().to_string();
    if percorso
        .as_os_str()
        .as_encoded_bytes()
        .ends_with(b" (deleted)")
    {
        return Err(non_disponibile(
            &nome,
            "l'immagine in esecuzione e' stata rimossa o sostituita: il control plane \
             che gira e quello su disco non sono lo stesso programma",
        ));
    }
    if !regolare {
        return Err(non_disponibile(&nome, "non e' un file regolare"));
    }
    if proprieta.scrivibile_da(worker) {
        return Err(non_disponibile(
            &nome,
            &format!(
                "il worker {}:{} potrebbe riscriverlo (proprietario {}:{}, mode {:o}): \
                 uno spawner che il worker modifica non separa nessun privilegio",
                worker.uid,
                worker.gid,
                proprieta.uid,
                proprieta.gid,
                proprieta.mode & 0o777
            ),
        ));
    }
    Ok(())
}

/// L'avvio riuscito: il figlio, e cio' che il preflight ha osservato.
#[derive(Debug)]
struct TransizioneRiuscita {
    figlio: std::process::Child,
    evidenza: EvidenzaPreflight,
    /// L'estremo da cui il supervisore **legge** cio' che il worker manda.
    ///
    /// Sta nell'esito riuscito e non altrove perche' e' li' che serve, e
    /// perche' un canale senza un figlio a cui appartenga non ha niente da
    /// portare: sul cammino fallito i due estremi cadono, e la pipe muore con
    /// loro invece di restare aperta a non leggere niente.
    supervisore_legge: std::io::PipeReader,
    /// L'estremo su cui il supervisore **scrive** cio' che manda al worker.
    supervisore_scrive: std::io::PipeWriter,
}

/// L'avvio fallito: la causa, e cio' che il preflight ha osservato.
///
/// Porta l'evidenza perche' il dominio e' **gia' configurato** e resta li':
/// chi lo smonta deve sapere quale sia. Nel caso riuscito l'evidenza serve al
/// rapporto, qui alla pulizia.
///
/// Non c'e' conversione verso `PlenoraError`: ogni `?` butterebbe via
/// l'evidenza in silenzio. Chi vuole l'errore lo prende da `causa`.
#[derive(Debug)]
struct TransizioneFallita {
    causa: PlenoraError,
    evidenza: EvidenzaPreflight,
    /// Che cosa va storto **mentre si chiude**, se qualcosa.
    ///
    /// Sta accanto alla causa, non al suo posto: la causa serve a capire, il
    /// difetto di pulizia a rimediare. `None` dice che non c'e' niente da
    /// rimediare.
    difetto_di_pulizia: Option<String>,
}

/// Un tentativo che non e' riuscito: perche', e che cosa resta.
///
/// Un fallimento **dopo** lo `spawn` ha due facce: la ragione per cui la
/// transizione non avviene, e l'esito della chiusura del figlio che esiste
/// gia'. Nessuna delle due si puo' sacrificare.
#[derive(Debug)]
struct TentativoFallito {
    causa: PlenoraError,
    /// `None` quando non c'e' niente da chiudere, o la chiusura e' andata.
    difetto_di_pulizia: Option<String>,
}

/// Un errore che arriva **prima** che esista un figlio diventa un tentativo
/// fallito senza niente da rimediare.
///
/// La conversione sta su `Box` e non su `TentativoFallito` perche' e' la forma
/// che l'operatore `?` usa: il tipo e' grande — porta un `PlenoraError` e una
/// stringa — e restituirlo per valore farebbe pagare a **ogni** chiamata la
/// dimensione del ramo raro.
impl From<PlenoraError> for Box<TentativoFallito> {
    fn from(causa: PlenoraError) -> Self {
        Self::new(TentativoFallito {
            causa,
            difetto_di_pulizia: None,
        })
    }
}

/// I due esiti dell'avvio, costruiti dallo stesso posto.
///
/// Non tocca ne' filesystem ne' processi: il ramo fallito si prova ovunque,
/// senza ambiente. I controlli stanno in `tenta`, che questa funzione non
/// chiama e che nessun parametro sostituisce. E' generica sull'esito riuscito
/// perche' non lo guarda.
///
/// # Errors
///
/// [`TransizioneFallita`], che porta la causa **e** l'evidenza. Sta in un
/// `Box` perche' e' molto piu' grande dell'esito riuscito, che altrimenti
/// pagherebbe in pila la dimensione del ramo raro.
fn esito<T>(
    tentativo: std::result::Result<T, Box<TentativoFallito>>,
    evidenza: EvidenzaPreflight,
) -> std::result::Result<(T, EvidenzaPreflight), Box<TransizioneFallita>> {
    match tentativo {
        Ok(figlio) => Ok((figlio, evidenza)),
        Err(fallito) => Err(Box::new(TransizioneFallita {
            causa: fallito.causa,
            evidenza,
            difetto_di_pulizia: fallito.difetto_di_pulizia,
        })),
    }
}

/// Cio' che lo spawner ha **rivalidato da se'**.
///
/// E' il risultato di aver riguardato percorsi, montaggio, permessi,
/// namespace e i quattro controlli dentro il processo che poi eseguira'.
/// Niente `Clone`: si consuma entrando nel dominio, una volta sola.
#[derive(Debug, PartialEq, Eq)]
struct DominioRivalidato {
    dominio: PathBuf,
    radice: PathBuf,
    worker: IdentitaWorker,
    montaggio: Montaggio,
    namespace_del_padre: Vec<(String, String)>,
}

/// Rivalida il dominio dentro lo spawner, senza scrivere niente.
///
/// Rilegge perche' fra preflight e `spawn` la gerarchia puo' essere cambiata.
/// Non riscrive perche' il limite deve essere **gia'** in vigore quando lo
/// spawner nasce (`F4-1`, `GA-7`): il supervisore scrive, lo spawner
/// controlla. I namespace si confrontano col **padre**, letto da `/proc`, non
/// con un valore della richiesta.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], come il preflight.
fn rivalida<S: SuperficieDominio>(
    superficie: &S,
    richiesta: &RichiestaSpawner,
    namespace_del_padre: Vec<(String, String)>,
) -> Result<DominioRivalidato> {
    let worker = IdentitaWorker {
        uid: richiesta.uid,
        gid: richiesta.gid,
    };
    let (dominio, radice, montaggio) = accerta_perimetro(superficie, worker)?;
    if dominio != richiesta.dominio {
        return Err(non_disponibile(
            "dominio",
            &format!(
                "la richiesta nomina {} ma il percorso risolto e' {}",
                richiesta.dominio.display(),
                dominio.display()
            ),
        ));
    }
    if radice != richiesta.radice {
        return Err(non_disponibile(
            "control plane",
            &format!(
                "la richiesta nomina {} ma il percorso risolto e' {}",
                richiesta.radice.display(),
                radice.display()
            ),
        ));
    }

    // I quattro controlli si **rileggono**, non si riscrivono.
    let tetto = richiesta.tetto_byte.to_string();
    for controllo in Controllo::ORDINE {
        let atteso = controllo.valore_fisso().unwrap_or(tetto.as_str());
        let riletto = superficie
            .rileggi(controllo)
            .map_err(|difetto| non_disponibile(controllo.file(), &difetto.to_string()))?;
        if riletto.trim() != atteso {
            return Err(non_disponibile(
                controllo.file(),
                &format!("atteso {atteso}, riletto {}", riletto.trim()),
            ));
        }
    }

    accerta_quiescenza(superficie)?;

    Ok(DominioRivalidato {
        dominio,
        radice,
        worker,
        montaggio,
        namespace_del_padre,
    })
}

/// Che cosa il confine ha deciso di questo processo.
///
/// Tre esiti e non due: il worker esegue, dichiara com'e' andata e ha finito.
/// Con un `Option` quel caso si confonderebbe con «non e' la mia modalita'»,
/// e il processo arriverebbe al parser della CLI dopo un'esecuzione riuscita.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub enum DalConfine {
    /// Non e' questa modalita': si prova la prossima, e poi il parser.
    AltroComando,
    /// E' questa modalita', ed e' arrivata in fondo.
    ///
    /// Lo spawner non la produce mai, e non e' una lacuna: la sua riuscita e'
    /// una `exec`, dopo la quale questo processo non esiste piu' e non c'e'
    /// nessuno a cui rendere un valore.
    Conclusa,
    /// E' questa modalita', e non e' andata.
    Fallita(PlenoraError),
}

/// L'ingresso dello spawner, quando la riga di comando dice che lo e'.
///
/// Il riconoscimento sta qui perche' il chiamante non deve conoscere la
/// stringa della versione.
///
/// # Errors
///
/// [`DalConfine::Fallita`] col motivo se questo processo e' uno spawner e la
/// sequenza non regge; [`DalConfine::AltroComando`] se non lo e'.
#[cfg(target_os = "linux")]
pub(crate) fn dal_confine_se_spawner(argomenti: &[std::ffi::OsString]) -> DalConfine {
    match riconosci_modalita(argomenti.get(1), PREFISSO_RISERVATO, VERSIONE_RICHIESTA) {
        Riconoscimento::AltroComando => DalConfine::AltroComando,
        // Il messaggio e' **costante**: non riporta cio' che ha trovato, e non
        // per reticenza — la versione trovata e' `argv`, e un errore che la
        // ripetesse porterebbe nei log un ingresso arbitrario.
        Riconoscimento::VersioneNonSupportata => DalConfine::Fallita(non_disponibile(
            "spawner",
            &format!("versione della richiesta non supportata: serve «{VERSIONE_RICHIESTA}»"),
        )),
        Riconoscimento::Supportata => match spawner::dal_confine(&argomenti[1..]) {
            // `dal_confine` non rende mai `Ok`: se la `exec` riesce, questo
            // processo non esiste piu'.
            Ok(mai) => match mai {},
            Err(errore) => DalConfine::Fallita(errore),
        },
    }
}

/// Se questo processo e' un **worker**, lo porta fin dove il worker arriva.
///
/// Va chiamata nello stesso punto del dispatch dello spawner, e comunque
/// prima del parser della CLI: altrimenti una riga del namespace riservato
/// riceverebbe «comando sconosciuto». Rende [`DalConfine::AltroComando`] se
/// `argv[1]` non e' del namespace del worker.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] con la versione attesa, quando il
/// namespace e' quello e la versione no.
#[cfg(target_os = "linux")]
pub(crate) fn dal_confine_se_worker(argomenti: &[std::ffi::OsString]) -> DalConfine {
    match riconosci_modalita(argomenti.get(1), PREFISSO_WORKER, VERSIONE_WORKER) {
        Riconoscimento::AltroComando => DalConfine::AltroComando,
        // Il messaggio e' **costante**, per la stessa ragione dello spawner: la
        // versione trovata e' `argv`, e un errore che la ripetesse porterebbe
        // nei log un ingresso arbitrario.
        Riconoscimento::VersioneNonSupportata => DalConfine::Fallita(non_disponibile(
            "worker",
            &format!("versione della modalita' worker non supportata: serve «{VERSIONE_WORKER}»"),
        )),
        Riconoscimento::Supportata => worker::dal_confine(),
    }
}

/// Se questo processo e' un **verificatore**, lo porta fin dove arriva.
///
/// Stessa disciplina di [`dal_confine_se_worker`], su un namespace riservato
/// distinto ([`PREFISSO_VERIFICATORE`]): le due autorita' («rilegge un
/// artefatto», «esegue un piano») restano visibili gia' dal dispatch.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] con la versione attesa, quando il
/// namespace e' quello e la versione no.
#[cfg(target_os = "linux")]
pub(crate) fn dal_confine_se_verificatore(argomenti: &[std::ffi::OsString]) -> DalConfine {
    match riconosci_modalita(
        argomenti.get(1),
        PREFISSO_VERIFICATORE,
        VERSIONE_VERIFICATORE,
    ) {
        Riconoscimento::AltroComando => DalConfine::AltroComando,
        Riconoscimento::VersioneNonSupportata => DalConfine::Fallita(non_disponibile(
            "verificatore",
            &format!(
                "versione della modalita' verificatore non supportata: serve \
                 «{VERSIONE_VERIFICATORE}»"
            ),
        )),
        Riconoscimento::Supportata => verificatore::dal_confine(),
    }
}

/// Se il dominio e' popolato, secondo `cgroup.events`.
///
/// Le forme ambigue si rifiutano tutte, perche' sceglierne una lettura
/// inventerebbe il valore su cui si decide se partire: campo **assente**
/// (senza segnale la barriera di quiescenza non si implementa),
/// **duplicato**, **non numerico** o **fuori da `{0, 1}`** (il formato e'
/// cambiato sotto di noi).
///
/// # Errors
///
/// Il motivo, gia' in forma di frase: e' il testo che finisce nell'esito.
fn popolato(eventi: &str) -> std::result::Result<bool, &'static str> {
    let mut trovato: Option<&str> = None;
    for riga in eventi.lines() {
        let Some((nome, valore)) = riga.split_once(' ') else {
            continue;
        };
        if nome.trim() != "populated" {
            continue;
        }
        if trovato.is_some() {
            return Err(
                "populated compare piu' di una volta: quale riga valga non lo dichiara nessuno",
            );
        }
        trovato = Some(valore.trim());
    }
    match trovato {
        None => Err("manca il campo populated: la barriera di quiescenza non e' implementabile"),
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err("populated non e' 0 ne' 1: il formato del file non e' quello atteso"),
    }
}

/// Se un'opzione compare fra quelle di montaggio.
///
/// Confronto per **elemento** e non per sottostringa: `memory_localevents`
/// comparirebbe dentro un'ipotetica `no_memory_localevents`, e una difesa che
/// si lascia ingannare da un prefisso non e' una difesa.
fn opzione_presente(opzioni: &str, cercata: &str) -> bool {
    opzioni.split(',').any(|opzione| opzione.trim() == cercata)
}

/// L'unico costruttore dell'esito negativo.
///
/// Uno solo perche' il testo ha una forma pretesa — che cosa ha ceduto, in che
/// modo — e sparpagliare la costruzione la farebbe divergere alla terza
/// occorrenza.
fn non_disponibile(cosa: &str, motivo: &str) -> PlenoraError {
    PlenoraError::IsolationUnavailable(format!(
        "dominio di isolamento non stabilito su {cosa}: {motivo}"
    ))
    .with_phase(ErrorPhase::Prepare)
}
