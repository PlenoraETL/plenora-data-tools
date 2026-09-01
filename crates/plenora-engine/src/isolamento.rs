//! Il dominio di isolamento: si costruisce, si rilegge, e se non regge non si
//! parte.
//!
//! # Che cosa c'e' qui, e che cosa no
//!
//! Qui nasce **il dominio**, non chi lo usa. `PreparaIsolamento` e' il primo
//! nodo della macchina a stati del supervisore
//! (isolamento.md#31-supervisore): questo modulo porta la sola cosa che
//! supervisore e worker presuppongono entrambi — un dominio che il worker non
//! potra' toccare, oppure il rifiuto di partire.
//!
//! # Perche' prima dello spawn, e non dopo
//!
//! `F4-1` e `GA-7`: il limite e' in vigore **prima che esista un processo da
//! limitare**. Applicare un tetto a un processo gia' partito lascia fra la
//! partenza e l'applicazione una finestra in cui il tetto non c'e', e in quella
//! finestra il worker puo' allocare quanto vuole.
//!
//! # Scrivere non e' configurare
//!
//! Ogni proprieta' su cui poggia l'attribuzione e' una **scrittura su un file**
//! che puo' fallire in silenzio, essere ignorata da un kernel diverso, o essere
//! sovrascritta da qualcun altro. Il preflight le scrive e **le rilegge**, e il
//! profilo isolato non parte se una sola diverge
//! (isolamento.md#9-bis-preflight-del-dominio-scrivere-non-e-configurare).
//!
//! # L'ordine dei passi e' esso stesso una garanzia
//!
//! Prima si accerta **dove** si sta per scrivere — che il percorso si risolva,
//! che stia sotto il control plane, che il filesystem sia davvero `cgroup2` —
//! e poi **chi** potrebbe disfarlo. Solo allora si scrive.
//!
//! Il contrario sembra equivalente e non lo e': un preflight che modifica
//! quattro file e scopre alla fine che il percorso non e' quello atteso non
//! puo' piu' riportarli allo stato di partenza, e ha toccato una gerarchia di
//! cui non sa niente.
//!
//! # Possedere non e' solo essere qualcuno
//!
//! Il giudizio sul possesso non guarda il solo dominio. Per **uscire** dal
//! dominio non si scrive il proprio `cgroup.procs` — li' ci si e' gia' — si
//! scrive quello di un altro cgroup: il padre, un fratello. Il possesso si
//! giudica quindi su ogni antenato fino alla radice del control plane.
//!
//! # Perche' un trait invece del filesystem
//!
//! Perche' meta' di cio' che va provato non ha bisogno di privilegi: l'ordine
//! delle operazioni, il comportamento su ogni rilettura divergente, il
//! parsing, e il giudizio su proprietario e permessi. Quella meta' si prova su
//! una superficie controllata, ovunque.
//!
//! L'altra meta' — che le scritture arrivino davvero al kernel, e che un
//! worker senza autorita' non possa disfarle — non e' simulabile, e si prova
//! solo su una gerarchia vera. Le due prove non si sostituiscono: la prima dice
//! che la procedura e' giusta, la seconda che l'ambiente la onora.

use std::path::{Path, PathBuf};

use plenora_core::error::{ErrorPhase, PlenoraError, Result};

#[cfg(target_os = "linux")]
mod dominio;
#[cfg(target_os = "linux")]
mod identita;
#[cfg(target_os = "linux")]
mod lettura;
#[cfg(all(target_os = "linux", qualificazione_isolamento))]
pub mod qualificazione;
#[cfg(target_os = "linux")]
mod spawner;

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
/// # Perche' non basta il `cgroup.procs` del dominio
///
/// Perche' non e' quello con cui si evade. Scrivere il proprio pid nel
/// `cgroup.procs` **del dominio corrente** non porta da nessuna parte: ci si e'
/// gia'. Per uscire si scrive nel `cgroup.procs` di un **altro** cgroup — il
/// padre, un fratello — e quella scrittura non tocca nessuno dei file del
/// dominio.
///
/// La directory di un antenato conta per la stessa ragione: chi la puo'
/// scrivere ci crea dentro un cgroup nuovo e ci si sposta.
///
/// La catena si ferma alla radice del control plane, inclusa: sopra c'e'
/// l'amministrazione della macchina, che non e' cosa nostra da giudicare.
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
    /// # Perche' il bit di gruppo si rifiuta sempre, GID a parte
    ///
    /// Su un filesystem con ACL i bit di gruppo non sono «il gruppo
    /// proprietario»: sono la **mask della classe ACL**, cioe' il tetto dei
    /// permessi di ogni voce nominativa. Un file con `g+w` e un GID che col
    /// worker non c'entra puo' portare una ACL `user:<worker>:rw`, e quella
    /// ACL vale.
    ///
    /// Leggerle per escluderlo significherebbe interrogarle su ogni file e
    /// fidarsi di averle interpretate come il kernel; guardare la sola mask e
    /// rifiutare costa qualche rifiuto in piu' su gerarchie che non useremmo
    /// comunque.
    ///
    /// Resta un'alternativa, e va nominata perche' e' quella che allenterebbe
    /// la regola: una prova esplicita che nell'ambiente qualificato `cgroup2`
    /// non supporti ACL nominative. Finche' quella prova non c'e', vale questa.
    ///
    /// # Che cosa resta ammesso
    ///
    /// Il bit del proprietario, e solo quando il proprietario **non e'** il
    /// worker: e' il caso normale di una gerarchia che il control plane
    /// possiede e amministra.
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

/// Cio' che una superficie puo' non riuscire a fare.
///
/// Non e' un `PlenoraError`: il difetto qui e' meccanico, e diventa
/// `IsolationUnavailable` solo quando il preflight decide che rende il dominio
/// inservibile. Tenerli separati impedisce a una superficie di decidere al
/// posto del preflight.
///
/// # Perche' porta l'`ErrorKind` e non solo un testo
///
/// Perche' un chiamante deve poter distinguere «il file non c'e'» da «il file
/// non si legge», e quelle due cose hanno conseguenze opposte: la prima e'
/// un'assenza, che non concede autorita' a nessuno; la seconda e' un dubbio, e
/// un dubbio si rifiuta.
///
/// Ricavare la distinzione dal **testo** — cercare `os error 2` nel `Display`
/// — la farebbe dipendere da come il sistema formatta i propri errori, cioe'
/// da qualcosa che nessuno ha promesso e che una locale diversa cambia. Il
/// tipo la porta invece per costruzione.
#[derive(Debug)]
enum DifettoSuperficie {
    /// La scrittura non e' riuscita.
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
    /// Canonica perche' ogni confronto successivo — il montaggio che lo
    /// contiene, gli antenati fino alla radice — si fa per prefisso, e un `..`
    /// o un link simbolico nel mezzo renderebbe quel confronto una domanda su
    /// un percorso diverso da quello a cui si scrive.
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
/// # Perche' un token e non un insieme di dati
///
/// Perche' il preflight accerta una **combinazione**: che *quel* dominio
/// canonico stia sotto *quella* radice, su *quel* montaggio, con *quei*
/// namespace, e che *quella* identita' del worker non possa disfarlo. Nessuna
/// di quelle affermazioni vale da sola.
///
/// Se lo spawner ricevesse gli stessi dati come campi indipendenti, un
/// chiamante potrebbe verificarne una combinazione ed eseguirne un'altra —
/// preparare il dominio A e avviare il worker in B, o con un UID che nessuno
/// ha giudicato contro i permessi di A. Il tipo lo impedisce: si costruisce
/// **solo** dentro [`prepara_dominio`], non ha costruttore ne' campi
/// ricombinabili, e lo spawner lo consuma invece di ricevere le parti.
///
/// Non e' un booleano: un preflight riuscito ha **osservato** delle cose, e
/// alcune servono allo spawner o a chi legge l'evidenza dopo. Ridurle a «e'
/// andata bene» le butterebbe via nel momento in cui costano meno.
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
    /// Lo spawner li pretende **identici** ai propri prima della `exec`: nasce
    /// dal supervisore e li eredita, quindi una differenza significa che nel
    /// mezzo qualcuno ha fatto una `unshare`.
    ///
    /// Che cosa cambia una `unshare` di `user`: non che le capability
    /// spariscano dalla maschera — dentro il namespace nuovo ci sono, e la
    /// maschera le mostra — ma **rispetto a quale namespace** capability e
    /// proprieta' dei file hanno significato. Un UID che li' non ha autorita'
    /// puo' averla su file mappati diversamente.
    ///
    /// Questo controllo dice che nel momento della `exec` il namespace e'
    /// quello giusto. Non impedisce al worker di fare `unshare` **dopo**, e sarebbe falso
    /// dire che `no_new_privs` lo impedisca: `no_new_privs` vieta di acquisire
    /// privilegi attraverso una `execve` — setuid, capability di file — e non
    /// tocca `unshare`. Un processo non privilegiato che crea uno user
    /// namespace, dove la policy del kernel lo consente, lo crea anche con
    /// `no_new_privs = 1`, e dentro quel namespace ha capability piene.
    ///
    /// La prova ostile non deve quindi pretendere che la `unshare` fallisca.
    /// Deve accettare **due** esiti, ed entrambi sono un successo:
    ///
    /// - la `unshare` e' rifiutata dalla policy dell'host;
    /// - la `unshare` riesce, il namespace cambia e nel figlio ci sono
    ///   capability, ma riscrivere il control plane e uscire dal dominio
    ///   restano impossibili, e lo stato resta invariato.
    ///
    /// Il secondo e' quello che conta, perche' e' quello che dice **perche'**
    /// regge: non un flag, ma il fatto che i file della gerarchia appartengono
    /// a un UID che nel namespace nuovo non e' mappato, e che il dominio e'
    /// sigillato. Le capability di uno user namespace valgono sugli oggetti di
    /// quel namespace, non su quelli del padre.
    namespace_attesi: Vec<(String, String)>,
    /// Se fra le opzioni di superblocco c'e' `memory_localevents`.
    ///
    /// **Registrato, non rifiutato.** L'opzione rende non gerarchico anche
    /// `memory.events`, cioe' toglie una delle tre fonti di evidenza. Ma con il
    /// dominio sigillato non ci sono discendenti, quindi locale e gerarchico
    /// coincidono — e la conclusione **dipende dal sigillo**, che e' a sua volta
    /// una scrittura riletta. Se il sigillo non si stabilisce il profilo non
    /// parte comunque, e il caso non si presenta.
    ///
    /// Quel caso non e' pero' mai stato misurato: la gerarchia su cui il
    /// prototipo gira non ha `memory_localevents`, quindi la conclusione e' un
    /// ragionamento e non un'osservazione. Per questo il valore si registra.
    eventi_locali: bool,
}

/// Dove sta il dominio e chi puo' toccarlo.
///
/// E' la parte che il supervisore e lo spawner accertano **allo stesso modo**:
/// stesso ordine, stesse condizioni, stesse ragioni di rifiuto. Tenerla in un
/// posto solo e' l'unica forma in cui «lo spawner rivalida» significa davvero
/// che rivalida *quello*: due copie divergono, e la seconda a divergere e'
/// sempre quella che non si sta guardando.
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
fn prepara_dominio<S: SuperficieDominio>(
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

/// La versione della richiesta che attraversa il confine.
///
/// Cambia quando cambiano i campi o il loro significato. Lo spawner rifiuta
/// tutto cio' che non porta **esattamente** questa stringa: un supervisore e
/// uno spawner di versioni diverse non sono lo stesso programma, e
/// interpretare gli argomenti dell'altro significherebbe indovinare.
const VERSIONE_RICHIESTA: &str = "plenora-spawner-1";

/// Quello che attraversa il confine fra supervisore e spawner.
///
/// # Perche' una richiesta e non una prova
///
/// [`DominioPreparato`] e' un valore Rust in memoria: non attraversa un
/// confine di processo, e non c'e' modo di trasmetterlo. Cio' che passa e'
/// **una richiesta**, ed e' una differenza di sostanza, non di forma: una
/// prova sarebbe qualcosa che lo spawner accetta per buona, e uno spawner che
/// crede a cio' che gli viene detto non aggiunge nessuna garanzia a quella del
/// mittente.
///
/// La richiesta dice quindi soltanto **su che cosa** lavorare — quale dominio,
/// quale radice, quale identita', quale tetto — e non contiene nessuna
/// affermazione del tipo «e' gia' stato verificato». Lo spawner rivalida tutto
/// da se': ambiente, percorsi, montaggio, permessi, namespace e i quattro
/// controlli.
///
/// # Perche' e' limitata
///
/// Perche' ogni campo in piu' e' una cosa in piu' di cui lo spawner potrebbe
/// fidarsi. Il montaggio non passa: lo spawner lo ritrova. I namespace non
/// passano: lo spawner li confronta con quelli del **proprio padre**, che e'
/// un fatto che nessun argomento puo' falsificare.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RichiestaSpawner {
    dominio: PathBuf,
    radice: PathBuf,
    uid: u32,
    gid: u32,
    tetto_byte: u64,
}

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
        ]
    }

    /// La richiesta letta dagli argomenti, fail-closed.
    ///
    /// # Che cosa si rifiuta
    ///
    /// Un numero di argomenti diverso, una versione diversa, un numero che non
    /// si interpreta, un percorso relativo. Nessuna di queste forme ha una
    /// lettura di ripiego: un argomento in piu' o in meno significa che chi
    /// scrive e chi legge non sono d'accordo su che cosa sia una richiesta.
    ///
    /// # Errors
    ///
    /// Il motivo, in forma di frase.
    fn da_argomenti(argomenti: &[std::ffi::OsString]) -> std::result::Result<Self, String> {
        let [versione, dominio, radice, uid, gid, tetto] = argomenti else {
            return Err(format!(
                "la richiesta ha {} argomenti invece di 6",
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
        // Assoluto **secondo POSIX**, cioe' con lo slash iniziale, e non
        // secondo `Path::is_absolute`: quest'ultimo risponde secondo la
        // piattaforma su cui il codice gira, e su Windows direbbe che
        // `/sys/fs/cgroup` non e' assoluto. Il percorso di cui si parla qui e'
        // sempre e solo un percorso di gerarchia `cgroup2`, che e' un oggetto
        // Linux: chiedere alla piattaforma ospite come si scrivono i percorsi
        // farebbe dipendere il giudizio da dove il caso viene compilato invece
        // che da che cosa il percorso e'.
        //
        // Lo slash si cerca nei **byte**. Un percorso Linux e' una sequenza di
        // byte senza `/` e senza `NUL`, e non e' tenuto a essere UTF-8:
        // passare per `to_str()` rifiuterebbe un dominio valido solo perche'
        // il suo nome non si decodifica, che e' una restrizione che nessuno ha
        // dichiarato e che il parser di `mountinfo` — che i byte li conserva —
        // non applica. `as_encoded_bytes` e' definito come un sovrainsieme di
        // UTF-8 in cui i byte ASCII rappresentano se stessi, quindi cercarci
        // uno `/` iniziale e' esatto su ogni piattaforma.
        let dominio = PathBuf::from(dominio);
        let radice = PathBuf::from(radice);
        for (nome, percorso) in [("il dominio", &dominio), ("la radice", &radice)] {
            if percorso.as_os_str().as_encoded_bytes().first() != Some(&b'/') {
                return Err(format!("{nome} non e' un percorso assoluto"));
            }
        }
        Ok(Self {
            dominio,
            radice,
            uid: u32::try_from(numero(uid, "l'uid")?).map_err(|_| "l'uid non entra in u32")?,
            gid: u32::try_from(numero(gid, "il gid")?).map_err(|_| "il gid non entra in u32")?,
            tetto_byte: numero(tetto, "il tetto")?,
        })
    }
}

/// Cio' che il preflight ha **osservato**, e che vale dopo la transizione.
///
/// # Perche' e' separata dal token
///
/// Perche' le due cose hanno vite opposte, e tenerle in un oggetto solo obbliga
/// a scegliere quale delle due sacrificare.
///
/// Il token e' **lineare**: esiste una volta, si consuma una volta, e la sua
/// unicita' e' cio' che impedisce di avviare due spawner sullo stesso dominio.
/// Se portasse anche l'evidenza, consumarlo la butterebbe via — ed e'
/// esattamente nel caso riuscito, cioe' quando c'e' qualcosa da riportare, che
/// andrebbe persa.
///
/// L'evidenza e' invece **duplicabile e persistente**: si registra, si scrive
/// in un rapporto, si confronta con quella di un'altra macchina. Fra le sue
/// osservazioni c'e' `memory_localevents`, che il contratto promette registrato
/// insieme al sigillo: una promessa che non sopravvivesse alla transizione
/// riuscita sarebbe mantenuta solo quando non serve.
///
/// # Perche' non attraversa il confine
///
/// Perche' non e' una prova. Lo spawner rivalida tutto da se' e non guarda
/// niente di quanto sta qui: trasmetterla lo inviterebbe a crederci, e uno
/// spawner che crede a cio' che gli viene detto non aggiunge nessuna garanzia a
/// quella del mittente.
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

impl DominioPreparato {
    /// Smonta il token nelle sue due meta': la richiesta e l'evidenza.
    ///
    /// Consuma: il preflight prepara **un** dominio e ne avvia **uno** spawner,
    /// e un token riusabile permetterebbe di avviarne due sullo stesso dominio
    /// — il secondo dei quali troverebbe la quiescenza gia' rotta dal primo, ma
    /// solo per caso.
    ///
    /// L'evidenza esce di qui perche' il chiamante la tenga: e' l'unico momento
    /// in cui esiste, e dopo la transizione non c'e' piu' modo di ricostruirla.
    fn consuma(self) -> (RichiestaSpawner, EvidenzaPreflight) {
        let richiesta = RichiestaSpawner {
            dominio: self.dominio.clone(),
            radice: self.radice.clone(),
            uid: self.worker.uid,
            gid: self.worker.gid,
            tetto_byte: self.tetto_byte,
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
/// # Perche' il chiamante non lo sceglie
///
/// Perche' un percorso che arriva dall'esterno rende falso tutto cio' che il
/// preflight promette: `/bin/true`, o direttamente il worker, verrebbero
/// avviati come figli ordinari — **fuori** dal dominio, con l'identita' del
/// supervisore e senza nessuno dei sette passi — e il chiamante avrebbe in mano
/// un `Child` indistinguibile da quello di una transizione riuscita.
///
/// Il binario e' quindi quello **in esecuzione**, letto dal kernel e non da un
/// argomento. Questa funzione decide se quel binario si puo' rieseguire; e' qui,
/// separata dalla lettura, perche' la regola si prova ovunque mentre la lettura
/// no.
///
/// # Si giudica un nome, ma non si esegue un nome
///
/// I due argomenti vengono da due posti diversi, e non e' un caso. `percorso` e'
/// il **bersaglio** di `/proc/self/exe`, cioe' un nome, e serve solo a dire di
/// che cosa si sta parlando e a riconoscere l'immagine rimossa. `regolare` e
/// `proprieta` descrivono invece l'**inode in esecuzione**, interrogato
/// attraverso `/proc/self/exe` e non attraverso quel nome.
///
/// La distinzione decide l'esito. Un nome si puo' sostituire fra il momento in
/// cui lo si guarda e quello in cui lo si esegue — e' una `rename`, ed e'
/// atomica — e un controllo fatto sul nome direbbe allora una cosa vera su un
/// file e ne eseguirebbe un altro. Per questo l'avvio non usa il nome: usa
/// `/proc/self/exe`, che il kernel tiene legato all'immagine di questo
/// processo. Fra il giudizio e la `exec` non c'e' nessuna finestra perche' non
/// c'e' nessuna risoluzione da rifare.
///
/// # Le tre condizioni, e perche' ciascuna
///
/// - **non e' stata rimossa**: su Linux il bersaglio di `/proc/self/exe` porta
///   il suffisso ` (deleted)` quando il file e' stato rimosso o sostituito
///   sotto il processo. Eseguire `/proc/self/exe` darebbe comunque l'immagine
///   giusta — e' proprio cio' che quel collegamento garantisce — quindi il
///   rifiuto non serve a evitare di eseguire un binario altrui. Serve a non
///   proseguire quando il control plane in esecuzione e quello su disco sono
///   due programmi diversi, che e' uno stato che nessuno ha dichiarato;
/// - **e' un file regolare**: una directory, un socket o un dispositivo non si
///   eseguono, e trattarli come eseguibili significa non aver guardato;
/// - **il worker non lo puo' riscrivere**: e' la condizione che conta. Un
///   binario dello spawner che il worker puo' modificare rende l'intera
///   separazione di privilegio una formalita', perche' il prossimo avvio
///   eseguirebbe cio' che il worker ci ha messo dentro. Il giudizio e' lo
///   stesso, conservativo, che vale sui file della gerarchia, e vale
///   sull'inode: un `mode` letto dal nome descriverebbe di nuovo un file che
///   potrebbe non essere questo.
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
    if percorso.as_os_str().as_encoded_bytes().ends_with(b" (deleted)") {
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
}

/// L'avvio fallito: la causa, e cio' che il preflight ha osservato.
///
/// # Perche' l'evidenza sta anche qui
///
/// Perche' quando l'avvio fallisce il dominio e' **gia' configurato**: il
/// preflight ha scritto e riletto i quattro controlli su una gerarchia vera, e
/// quel dominio resta li'. Chi deve smontarlo ha bisogno di sapere quale sia, e
/// un errore che dicesse solo «non e' partito» lascerebbe dietro di se' un
/// cgroup con un tetto, un sigillo e nessuno che lo rimuova.
///
/// E' l'esatto contrario del caso riuscito, dove l'evidenza serve al rapporto:
/// qui serve alla pulizia. Sono due usi diversi della stessa osservazione, e
/// nessuno dei due sopravvive se l'evidenza vive in un ramo solo.
///
/// # Perche' non c'e' una conversione verso `PlenoraError`
///
/// Perche' esisterebbe per essere usata con `?`, e ogni `?` su questo tipo
/// butterebbe via l'evidenza in silenzio — cioe' rifarebbe esattamente il
/// difetto che questo tipo esiste per chiudere. Chi vuole l'errore lo prende
/// da `causa`, e in quel momento ha l'evidenza in mano.
#[derive(Debug)]
struct TransizioneFallita {
    causa: PlenoraError,
    evidenza: EvidenzaPreflight,
}

/// I due esiti dell'avvio, costruiti dallo stesso posto.
///
/// # Perche' e' una funzione a se', e sta qui
///
/// Perche' e' l'unica parte dell'avvio che non tocca ne' il filesystem ne' un
/// processo: prende cio' che il tentativo ha reso e cio' che il preflight ha
/// osservato, e li mette insieme. Separarla rende il ramo fallito **provabile
/// senza ambiente** — un caso deterministico le passa un errore e guarda che
/// l'evidenza esca intera — e sta nell'orchestrazione, non nello spawner,
/// perche' non ha niente di Linux e i casi che la esercitano girano ovunque.
///
/// Che il ramo fallito si raggiunga in un caso non vuol dire che si possa
/// raggiungere saltando i controlli: quelli stanno in `tenta`, che questa
/// funzione non chiama e che nessun parametro sostituisce.
///
/// # Errors
///
/// [`TransizioneFallita`], che porta la causa **e** l'evidenza. E' in un `Box`
/// perche' porta tutto cio' che il preflight ha osservato — percorsi, montaggio,
/// namespace — ed e' quindi molto piu' grande dell'esito riuscito: senza,
/// **ogni** chiamata pagherebbe in pila la dimensione del ramo raro.
fn esito(
    tentativo: Result<std::process::Child>,
    evidenza: EvidenzaPreflight,
) -> std::result::Result<TransizioneRiuscita, Box<TransizioneFallita>> {
    match tentativo {
        Ok(figlio) => Ok(TransizioneRiuscita { figlio, evidenza }),
        Err(causa) => Err(Box::new(TransizioneFallita { causa, evidenza })),
    }
}

/// Cio' che lo spawner ha **rivalidato da se'**.
///
/// Non e' [`DominioPreparato`] letto da un argomento: e' il risultato di aver
/// riguardato tutto — percorsi, montaggio, permessi, namespace e i quattro
/// controlli — dentro il processo che poi eseguira'. La richiesta dice su che
/// cosa guardare; questo tipo dice che si e' guardato.
///
/// Niente `Clone`: si consuma nel momento in cui lo spawner entra nel dominio,
/// e averne due copie vorrebbe dire poter entrare due volte in cio' che e'
/// stato verificato una volta sola.
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
/// # Perche' rilegge invece di fidarsi
///
/// Perche' fra il preflight del supervisore e questo momento e' passato uno
/// `spawn`, e in quel tempo la gerarchia puo' essere cambiata: qualcuno puo'
/// aver riscritto un controllo, cambiato i permessi, spostato un mount. Un
/// programma che eseguisse sulla parola del proprio chiamante non
/// aggiungerebbe nessuna garanzia a quelle del chiamante.
///
/// # Perche' non riscrive
///
/// Perche' il limite deve essere **gia'** in vigore quando lo spawner nasce
/// (`F4-1`, `GA-7`): scriverlo qui significherebbe che fra la nascita del
/// processo e l'applicazione del tetto c'e' una finestra. Il supervisore
/// scrive, lo spawner controlla. Spostare le scritture qui cambierebbe la
/// macchina a stati, ed e' una decisione che non si prende di straforo.
///
/// # I namespace si confrontano col **padre**
///
/// Non con un valore che arriva dalla richiesta, che sarebbe di nuovo fidarsi:
/// col processo che ci ha generato, letto da `/proc`. Se differiscono, fra lo
/// `spawn` e questo momento qualcuno ha fatto una `unshare`.
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

/// Se il dominio e' popolato, secondo `cgroup.events`.
///
/// # Perche' non basta trovare il campo
///
/// Il file ha una forma semplice — `chiave valore`, una coppia per riga — e
/// proprio per questo un parser indulgente ci passa sopra senza accorgersi di
/// niente. Le forme ambigue vanno rifiutate tutte, perche' nessuna ha una
/// lettura ovvia e sceglierne una significa **inventare** il valore su cui poi
/// si decide se partire:
///
/// - **assente**: senza il segnale la barriera di quiescenza non e'
///   implementabile;
/// - **duplicato**: due righe, due valori possibili. Prendere la prima o
///   l'ultima e' una convenzione che nessuno ha dichiarato, e su un file di
///   kernel una duplicazione dice che quel file non e' quello che crediamo;
/// - **non numerico** o **fuori da `{0, 1}`**: il campo e' un booleano, e un
///   terzo valore significa che il formato e' cambiato sotto di noi.
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
