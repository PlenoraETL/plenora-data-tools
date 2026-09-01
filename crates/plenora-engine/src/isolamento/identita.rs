//! `F4-15`: che il worker **non possieda alcuna autorita' sul control plane**.
//!
//! # La proprieta' non e' «UID distinto»
//!
//! UID/GID distinto e' il **meccanismo**, e per giunta l'unico prototipato
//! (`L19b`, cinque operazioni su cinque respinte). La proprieta' e' un'altra, e
//! guardare solo l'UID la lascerebbe scoperta su cinque assi:
//!
//! | asse | perche' l'UID non basta |
//! |---|---|
//! | identita' reale, effettiva e **salvata** | un salvato privilegiato si riprende con una `setresuid` |
//! | gruppi supplementari | un gruppo che possiede la gerarchia annulla la separazione a parita' di UID |
//! | capability | `CAP_DAC_OVERRIDE` rende irrilevanti proprietario e permessi |
//! | namespace | uno user namespace proprio rida' capability piene **dentro di se'** |
//! | descrittori ereditati | un `fd` gia' aperto in scrittura sopravvive al cambio di UID |
//!
//! L'ultima riga e' la piu' insidiosa: **il cambio d'identita' non revoca
//! l'autorita' gia' acquisita**. Un descrittore aperto sulla gerarchia prima
//! della `setresuid` resta scrivibile dopo, perche' il controllo dei permessi
//! avviene all'apertura e non a ogni scrittura. L'assenza di descrittori
//! scrivibili verso il control plane e' quindi una **proprieta' autonoma**,
//! non una conseguenza.
//!
//! # Non leggibile significa autorita' non esclusa
//!
//! Ogni lettura qui e' fail-closed. Un campo che manca, un numero che non si
//! interpreta, una directory che non si apre: nessuno di questi e' «nessuna
//! autorita'», sono «non lo sappiamo», e non saperlo e' esattamente il caso in
//! cui non si parte. Un `filter_map` che scarta in silenzio trasformerebbe un
//! valore malformato in un lasciapassare.
//!
//! # Perche' si rilegge invece di dedurre
//!
//! Perche' impostare non e' essere. Ogni riga qui sotto e' cio' che il
//! processo **e'**, letto da `/proc/self`, e non cio' che qualcuno ha chiesto
//! che fosse. E' la stessa disciplina del preflight sul dominio, applicata
//! all'identita'.

use std::os::unix::fs::MetadataExt as _;
use std::os::unix::io::AsRawFd as _;
use std::path::Path;

use super::lettura::leggi_limitato;

/// Cio' che un processo e', sui sei assi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Identita {
    /// Reale, effettivo, salvato, filesystem — nell'ordine di
    /// `/proc/self/status`.
    pub(super) uid: [u32; 4],
    /// Idem per il gruppo.
    pub(super) gid: [u32; 4],
    /// I gruppi supplementari.
    pub(super) gruppi: Vec<u32>,
    /// `NoNewPrivs`.
    pub(super) no_new_privs: bool,
    /// Le maschere di capability.
    pub(super) capability: Capability,
    /// I namespace, come `/proc/self/ns` li riporta.
    pub(super) namespace: Vec<(String, String)>,
    /// I descrittori aperti in **scrittura**.
    pub(super) descrittori_scrivibili: Vec<Descrittore>,
}

/// Un descrittore aperto in scrittura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Descrittore {
    /// Dove punta, per poterlo nominare in un errore.
    pub(super) percorso: String,
    /// Il filesystem su cui sta.
    ///
    /// E' questo e non il percorso a dire se il descrittore raggiunge il
    /// control plane: un bind mount, o un secondo punto di mount dello stesso
    /// superblocco, danno percorsi diversi per lo **stesso** filesystem, e un
    /// confronto per prefisso non li vede.
    pub(super) dispositivo: u64,
}

/// Le cinque maschere di capability.
///
/// Cinque e non una: `bounding` dice che cosa il processo **potrebbe**
/// acquisire, gli altri quattro che cosa ha. Confonderli porta a rifiutare un
/// ambiente sano o ad accettarne uno che non lo e'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Capability {
    pub(super) permitted: u64,
    pub(super) effective: u64,
    pub(super) inheritable: u64,
    pub(super) ambient: u64,
    pub(super) bounding: u64,
}

/// Perche' un'identita' non e' priva di autorita'.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Autorita {
    /// Uno degli identificatori e' zero, o non sono tutti uguali.
    Identita(&'static str),
    /// Restano gruppi supplementari.
    Gruppi(Vec<u32>),
    /// Una delle quattro maschere che danno autorita' non e' vuota.
    Capability(&'static str, u64),
    /// `no_new_privs` non e' attivo.
    NoNewPrivs,
    /// Restano descrittori scrivibili sul filesystem del control plane.
    Descrittori(Vec<String>),
    /// Un namespace non e' quello del processo che ha preparato il dominio.
    Namespace(String),
}

impl std::fmt::Display for Autorita {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identita(quale) => write!(f, "identita': {quale}"),
            Self::Gruppi(gruppi) => write!(f, "gruppi supplementari residui: {gruppi:?}"),
            Self::Capability(quale, maschera) => {
                write!(f, "capability {quale} non vuota: {maschera:#x}")
            }
            Self::NoNewPrivs => write!(f, "no_new_privs non attivo"),
            Self::Namespace(motivo) => write!(f, "namespace: {motivo}"),
            Self::Descrittori(percorsi) => write!(
                f,
                "descrittori scrivibili sul filesystem del control plane: {percorsi:?}"
            ),
        }
    }
}

impl Identita {
    /// I motivi per cui questa identita' **possiede ancora autorita'**.
    ///
    /// Vuoto significa che nessuno dei sei assi lascia una strada.
    ///
    /// # Il control plane si identifica dal filesystem, non dal percorso
    ///
    /// Un `fd` puo' raggiungere la gerarchia attraverso un bind mount o un
    /// secondo punto di mount dello stesso superblocco: il percorso sarebbe un
    /// altro, il filesystem lo stesso, e la scrittura arriverebbe ugualmente.
    /// Il confronto e' quindi sul **dispositivo**, che e' cio' che il kernel
    /// considera lo stesso oggetto.
    ///
    /// Ne segue che un `fd` scrivibile su un cgroup2 **estraneo** viene
    /// rifiutato insieme agli altri. E' piu' severo del necessario, ed e' il
    /// verso giusto in cui esserlo: scrivere in un cgroup qualunque e'
    /// autorita' che il worker non deve avere.
    ///
    /// # `bounding` non entra nel giudizio, e va detto
    ///
    /// Un bounding set non vuoto **non da' autorita' da solo**: e' il tetto di
    /// cio' che un processo potrebbe acquisire, non cio' che ha. Con
    /// `no_new_privs` attivo e le altre quattro maschere a zero non esiste una
    /// transizione che lo faccia diventare autorita', perche' `no_new_privs` e'
    /// proprio cio' che vieta quelle transizioni.
    ///
    /// Pretenderlo vuoto rifiuterebbe ambienti sani — svuotarlo richiede
    /// `CAP_SETPCAP`, che spesso non c'e' — senza rendere piu' stretta nessuna
    /// garanzia. Si registra e non si giudica.
    pub(super) fn autorita_residua(
        &self,
        dispositivo_control_plane: u64,
        namespace_attesi: &[(String, String)],
    ) -> Vec<Autorita> {
        let mut motivi = Vec::new();

        if self.uid.contains(&0) {
            motivi.push(Autorita::Identita("uno degli UID e' 0"));
        } else if !self.uid.windows(2).all(|coppia| coppia[0] == coppia[1]) {
            motivi.push(Autorita::Identita(
                "i quattro UID non coincidono: un salvato diverso si riprende",
            ));
        }
        if self.gid.contains(&0) {
            motivi.push(Autorita::Identita("uno dei GID e' 0"));
        } else if !self.gid.windows(2).all(|coppia| coppia[0] == coppia[1]) {
            motivi.push(Autorita::Identita("i quattro GID non coincidono"));
        }

        if !self.gruppi.is_empty() {
            motivi.push(Autorita::Gruppi(self.gruppi.clone()));
        }

        for (nome, maschera) in [
            ("permitted", self.capability.permitted),
            ("effective", self.capability.effective),
            ("inheritable", self.capability.inheritable),
            ("ambient", self.capability.ambient),
        ] {
            if maschera != 0 {
                motivi.push(Autorita::Capability(nome, maschera));
            }
        }

        if !self.no_new_privs {
            motivi.push(Autorita::NoNewPrivs);
        }

        let sul_control_plane: Vec<String> = self
            .descrittori_scrivibili
            .iter()
            .filter(|descrittore| descrittore.dispositivo == dispositivo_control_plane)
            .map(|descrittore| descrittore.percorso.clone())
            .collect();
        if !sul_control_plane.is_empty() {
            motivi.push(Autorita::Descrittori(sul_control_plane));
        }

        motivi.extend(self.namespace_divergenti(namespace_attesi));
        motivi
    }

    /// I namespace che non coincidono con quelli attesi.
    ///
    /// # Perche' sono un asse e non un dato
    ///
    /// Non perche' una `unshare` di `user` nasconda le capability: dentro il
    /// namespace nuovo ci sono, e la maschera le mostra. Il punto e' un altro,
    /// ed e' che cambia **rispetto a quale namespace** capability e proprieta'
    /// dei file hanno significato: un UID che li' non ha autorita' puo' averla
    /// su file mappati diversamente, e le stesse maschere lette prima e dopo
    /// non parlano piu' della stessa cosa. Lo stesso vale, con effetti diversi,
    /// per `mnt` — che cambia quali percorsi esistono — per `pid` e per
    /// `cgroup`.
    ///
    /// Lo spawner nasce dal processo che prepara e li **eredita**: una
    /// differenza significa che nel mezzo qualcuno ha fatto una `unshare`.
    ///
    /// # Che cosa questo controllo non copre, e va detto
    ///
    /// Dice che nel momento della `exec` il namespace e' quello giusto. Non impedisce al worker di fare `unshare` **dopo**, e sarebbe falso
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
    ///
    /// # I quattro che si pretendono
    ///
    /// `user`, `pid`, `cgroup`, `mnt`. Gli altri si registrano ma non si
    /// giudicano: `net`, `uts` e `ipc` non danno autorita' sul control plane, e
    /// pretenderli identici rifiuterebbe ambienti sani.
    ///
    /// Mancante, duplicato o diverso sono tutti e tre un motivo: di un
    /// namespace che non si sa nominare non si sa nemmeno dire che sia quello
    /// giusto.
    fn namespace_divergenti(&self, attesi: &[(String, String)]) -> Vec<Autorita> {
        /// I namespace che danno autorita' sul control plane.
        const PRETESI: [&str; 4] = ["user", "pid", "cgroup", "mnt"];

        let mut motivi = Vec::new();
        for nome in PRETESI {
            match (unico(&self.namespace, nome), unico(attesi, nome)) {
                (Some(proprio), Some(preflight)) if proprio == preflight => {}
                (Some(proprio), Some(preflight)) => motivi.push(Autorita::Namespace(format!(
                    "{nome} e' {proprio}, il preflight ha osservato {preflight}"
                ))),
                (None, _) => motivi.push(Autorita::Namespace(format!(
                    "{nome} manca o compare piu' di una volta fra i propri"
                ))),
                (_, None) => motivi.push(Autorita::Namespace(format!(
                    "{nome} manca o compare piu' di una volta fra gli attesi"
                ))),
            }
        }
        motivi
    }
}

/// Il valore di un namespace, se compare **esattamente una volta**.
///
/// Un duplicato non ha una lettura ovvia, e sceglierne uno significherebbe
/// affidarsi all'ordine di una lista che nessuno ha ordinato per quello.
fn unico<'a>(namespace: &'a [(String, String)], cercato: &str) -> Option<&'a str> {
    let mut trovato = None;
    for (nome, valore) in namespace {
        if nome != cercato {
            continue;
        }
        if trovato.is_some() {
            return None;
        }
        trovato = Some(valore.as_str());
    }
    trovato
}

/// Legge da `/proc/self` cio' che il processo **e'**.
///
/// # Quando si puo' chiamare, e quando no
///
/// **Prima** del cambio d'identita'. Nella finestra che va dal cambio alla
/// `exec` non funziona, e non e' un difetto di questo codice: quando un
/// processo cambia le proprie credenziali il kernel azzera il suo flag
/// *dumpable*, e con esso passa `/proc/<pid>` a `root`.
///
/// Che cosa si perde davvero non e' pero' materia di deduzione, ed e' il
/// motivo per cui il gate lo misura invece di darlo per scontato: sulla
/// gerarchia qualificata `status` resta leggibile — e' un file, e i permessi
/// della directory concedono l'attraversamento a tutti — e resta leggibile
/// anche `fd`, perche' il kernel fa un'eccezione esplicita per il processo che
/// guarda i propri descrittori. Cade `ns`, che quell'eccezione non ce l'ha, ed
/// e' abbastanza: senza i namespace la rilettura completa non e' possibile.
///
/// La finestra si chiude con la `exec`, che rimette *dumpable* e restituisce
/// `/proc/<pid>` al nuovo proprietario: il worker, dopo, si legge senza
/// problemi. Chi vive **dentro** la finestra e' il settimo passo dello
/// spawner, e per quello c'e' [`rileggi_credenziali`].
///
/// La misura sta nel modo `finestra` di `scripts/verifica_isolamento_linux.sh`,
/// che riporta la leggibilita' prima e dopo il cambio nello stesso processo.
/// Un kernel che concedesse tutto anche li' renderebbe [`rileggi_credenziali`]
/// non obbligatoria, non sbagliata: la sua correttezza poggia su un'altra
/// ragione, ed e' scritta li'.
///
/// # Errors
///
/// Il motivo, in forma di frase. Ogni campo mancante o malformato e' un
/// errore: su questi assi «non leggibile» significa «autorita' non esclusa».
pub(super) fn leggi_identita() -> std::result::Result<Identita, String> {
    let status = leggi_limitato(Path::new("/proc/self/status")).map_err(|e| e.to_string())?;

    Ok(Identita {
        uid: quaterna(&status, "Uid")?,
        gid: quaterna(&status, "Gid")?,
        gruppi: gruppi(&status)?,
        no_new_privs: booleano(&status, "NoNewPrivs")?,
        capability: Capability {
            permitted: maschera(&status, "CapPrm")?,
            effective: maschera(&status, "CapEff")?,
            inheritable: maschera(&status, "CapInh")?,
            ambient: maschera(&status, "CapAmb")?,
            bounding: maschera(&status, "CapBnd")?,
        },
        namespace: namespace_di_self()?,
        descrittori_scrivibili: descrittori_scrivibili()?,
    })
}

/// Un campo `Chiave:<tab>valore` di `/proc/self/status`.
fn campo<'a>(status: &'a str, chiave: &str) -> std::result::Result<&'a str, String> {
    let mut trovato = None;
    for riga in status.lines() {
        let Some((nome, valore)) = riga.split_once(':') else {
            continue;
        };
        if nome != chiave {
            continue;
        }
        if trovato.is_some() {
            return Err(format!(
                "/proc/self/status ha piu' di un campo {chiave}: quale valga non lo dichiara                  nessuno, e sceglierne uno sarebbe inventare meta' del giudizio"
            ));
        }
        trovato = Some(valore.trim());
    }
    trovato.ok_or_else(|| format!("/proc/self/status non ha il campo {chiave}"))
}

/// I quattro identificatori di `Uid` o `Gid`: reale, effettivo, salvato,
/// filesystem.
///
/// Devono essere **esattamente quattro** e tutti interpretabili: tre
/// significherebbe un formato diverso da quello atteso, e completarlo con un
/// valore scelto da noi sarebbe inventare meta' del giudizio.
fn quaterna(status: &str, chiave: &str) -> std::result::Result<[u32; 4], String> {
    let valore = campo(status, chiave)?;
    let numeri = valore
        .split_whitespace()
        .map(|n| {
            n.parse::<u32>()
                .map_err(|_| format!("{chiave} contiene «{n}», che non e' un identificatore"))
        })
        .collect::<std::result::Result<Vec<u32>, String>>()?;
    let quanti = numeri.len();
    <[u32; 4]>::try_from(numeri.as_slice())
        .map_err(|_| format!("{chiave} ha {quanti} valori invece di 4"))
}

/// I gruppi supplementari.
///
/// Il campo **deve esserci**. Assente non e' «nessun gruppo»: e' un
/// `/proc/self/status` che non e' quello atteso, e su un asse di `F4-15` questo
/// basta a non partire. Vuoto invece e' legittimo, ed e' esattamente cio' che
/// lo spawner produce.
fn gruppi(status: &str) -> std::result::Result<Vec<u32>, String> {
    campo(status, "Groups")?
        .split_whitespace()
        .map(|n| {
            n.parse::<u32>()
                .map_err(|_| format!("Groups contiene «{n}», che non e' un gruppo"))
        })
        .collect()
}

fn booleano(status: &str, chiave: &str) -> std::result::Result<bool, String> {
    match campo(status, chiave)? {
        "0" => Ok(false),
        "1" => Ok(true),
        altro => Err(format!("{chiave} vale «{altro}», che non e' 0 ne' 1")),
    }
}

fn maschera(status: &str, chiave: &str) -> std::result::Result<u64, String> {
    let valore = campo(status, chiave)?;
    u64::from_str_radix(valore, 16)
        .map_err(|_| format!("{chiave} vale «{valore}», che non e' una maschera esadecimale"))
}

/// Rilegge cio' che il cambio d'identita' puo' aver cambiato.
///
/// # Perche' namespace e descrittori si portano avanti invece di rileggerli
///
/// Non per comodita': **dopo** il cambio non sono leggibili, per la ragione
/// spiegata su [`leggi_identita`]. La domanda vera e' se portarli avanti sia
/// lecito, e la risposta dipende da che cosa sta in mezzo fra le due letture.
///
/// In mezzo ci sono tre cose sole: `no_new_privs`, `setgroups` e le tre
/// `setres*id`. Nessuna apre o chiude un descrittore, e nessuna cambia un
/// namespace — cambiarli richiede `unshare`, `setns` o `clone`, che qui non
/// compaiono. Le due osservazioni portate avanti sono quindi ancora vere, e
/// dirlo non e' una concessione: e' l'unica ragione per cui la verifica finale
/// resta completa.
///
/// Cio' che invece **cambia** — uid, gid, gruppi supplementari, le cinque
/// maschere di capability, `no_new_privs` — sta tutto in `/proc/self/status`,
/// che resta leggibile. Nessun asse di `F4-15` esce dalla verifica: quelli che
/// non si rileggono sono quelli che non si possono essere mossi.
///
/// # Perche' non si riapre l'accesso
///
/// Perche' l'unico modo sarebbe rimettere il flag *dumpable*, e quel flag e'
/// anche cio' che permette a un altro processo dello stesso uid di fare
/// `ptrace` su questo. Riaprire `/proc` per potersi guardare allo specchio
/// significherebbe aprire una porta molto piu' grande di cio' che si guadagna.
///
/// # Errors
///
/// Il motivo, in forma di frase.
pub(super) fn rileggi_credenziali(prima: &Identita) -> std::result::Result<Identita, String> {
    let status = leggi_limitato(Path::new("/proc/self/status")).map_err(|e| e.to_string())?;
    Ok(Identita {
        uid: quaterna(&status, "Uid")?,
        gid: quaterna(&status, "Gid")?,
        gruppi: gruppi(&status)?,
        no_new_privs: booleano(&status, "NoNewPrivs")?,
        capability: Capability {
            permitted: maschera(&status, "CapPrm")?,
            effective: maschera(&status, "CapEff")?,
            inheritable: maschera(&status, "CapInh")?,
            ambient: maschera(&status, "CapAmb")?,
            bounding: maschera(&status, "CapBnd")?,
        },
        namespace: prima.namespace.clone(),
        descrittori_scrivibili: prima.descrittori_scrivibili.clone(),
    })
}

/// Gli identificatori dei namespace.
///
/// Si registrano e non si giudicano: quale namespace sia «giusto» dipende da
/// come il supervisore e' stato avviato, e deciderlo qui imporrebbe una
/// topologia che il documento non fissa. Chi confronta e' il gate, che sa in
/// quale ambiente sta girando.
///
/// Ma **illeggibili e' un errore**: un `/proc/self/ns` che non si apre e' un
/// ambiente su cui non si puo' dire nulla, e su un asse di `F4-15` non poter
/// dire nulla vale come non poter escludere.
pub(super) fn namespace_di_self() -> std::result::Result<Vec<(String, String)>, String> {
    namespace_in(Path::new("/proc/self/ns"))
}

/// I namespace del processo che ci ha generato.
///
/// # Perche' il padre e non un argomento
///
/// Perche' lo spawner deve poter dire che i namespace in cui gira sono quelli
/// del supervisore, e un valore che gli arriva **dal** supervisore non lo dice:
/// direbbe soltanto che chi ha scritto l'argomento e chi lo legge sono
/// d'accordo, il che e' vero anche quando entrambi si sbagliano e vero per
/// costruzione quando l'argomento e' stato inventato.
///
/// Il PPID e i link sotto `/proc/<ppid>/ns` sono invece un fatto del kernel:
/// nessuna riga di comando li cambia.
///
/// # Che cosa questo **non** dice: che il supervisore sia vivo
///
/// Sarebbe comodo leggerlo come una rilevazione della morte del padre, e non lo
/// e'. Se il supervisore muore fra lo `spawn` e questa lettura, lo spawner viene
/// adottato da `init`, e i namespace di `init` sono quelli dell'host. Un
/// supervisore che sta anch'esso nei namespace dell'host — il caso ordinario —
/// ha quindi gli stessi identificatori di `init`: **coincidono**, il confronto
/// passa, e il worker parte orfano.
///
/// C'e' inoltre una corsa fra le due letture: il PPID si legge da
/// `/proc/self/status`, i namespace da `/proc/<ppid>/ns`, e in mezzo il padre
/// puo' morire e il pid essere riciclato da un altro processo. Il risultato
/// sarebbe allora il namespace di un estraneo, senza che niente lo segnali.
///
/// Nessuna delle due cose si chiude qui, e nominarle serve a non credere di
/// avere una garanzia che non si ha. Rilevare la morte del supervisore vuol
/// dire legare la sua identita' con pid **piu'** start-time e riverificarla, o
/// piu' semplicemente tenere aperto un canale la cui chiusura si osserva: sono
/// entrambe proprieta' del ciclo di vita del supervisore, e un supervisore
/// questo modulo non lo contiene. Rientrano quando esiste il primo chiamante di
/// produzione, insieme a chi sorveglia il worker e ne raccoglie l'esito.
///
/// Cio' che questa lettura da' e' comunque piu' di un argomento, ed e' il
/// motivo per cui resta: i namespace con cui il confronto avviene sono quelli
/// del processo che il kernel indica come padre, non quelli che una riga di
/// comando afferma. Chiude la `unshare` fra lo `spawn` e la `exec`, che e' cio'
/// per cui esiste; non chiude l'orfananza.
///
/// # Errors
///
/// Un `/proc/self/status` senza `PPid`, un PPID che non e' un numero, o un
/// `/proc/<ppid>/ns` che non si legge.
pub(super) fn namespace_del_padre() -> std::result::Result<Vec<(String, String)>, String> {
    let status = leggi_limitato(Path::new("/proc/self/status")).map_err(|e| e.to_string())?;
    let ppid: u32 = campo(&status, "PPid")?
        .parse()
        .map_err(|_| "PPid non e' un numero".to_owned())?;
    namespace_in(Path::new(&format!("/proc/{ppid}/ns")))
}

/// I namespace elencati sotto una directory `ns`.
fn namespace_in(directory: &Path) -> std::result::Result<Vec<(String, String)>, String> {
    let dove = directory.display();
    let voci = std::fs::read_dir(directory).map_err(|errore| format!("{dove}: {errore}"))?;
    let mut trovati = Vec::new();
    for esito in voci {
        let namespace = esito.map_err(|errore| format!("{dove}: {errore}"))?;
        let nome = namespace.file_name().to_string_lossy().into_owned();
        let bersaglio = std::fs::read_link(namespace.path())
            .map_err(|errore| format!("{dove}/{nome}: {errore}"))?;
        trovati.push((nome, bersaglio.to_string_lossy().into_owned()));
    }
    trovati.sort();
    Ok(trovati)
}

/// I descrittori aperti in scrittura, col filesystem su cui stanno.
///
/// # L'enumerazione usa **un** descrittore, e lo esclude
///
/// Aprire la directory dei descrittori ne crea uno, che comparirebbe
/// nell'elenco. Si apre con `rustix`, che rende un `OwnedFd`, e lo si consegna
/// a `Dir::new`, che ne prende **possesso** invece di aprirne un altro:
/// `Dir::fd()` rende il numero interrogabile, e si esclude.
///
/// `Dir::read_from` no: quella ne apre un **secondo**, che nessuno
/// escluderebbe. Quel secondo si chiude insieme all'iteratore, e la lettura del
/// suo `fdinfo` fallirebbe — facendo cadere l'intera scansione per un
/// descrittore che e' nostro.
///
/// Con `std::fs::read_dir` il numero non e' interrogabile affatto: `AsRawFd`
/// non e' implementato per `ReadDir`, e l'unica alternativa sarebbe
/// indovinarlo.
///
/// # Il modo di accesso si legge da `fdinfo`
///
/// Il campo `flags` e' in **ottale**, e i due bit bassi sono `O_RDONLY`,
/// `O_WRONLY`, `O_RDWR`. Si guarda li' e non nei permessi del file, perche'
/// cio' che conta e' come il descrittore e' stato aperto — ed e' precisamente
/// il punto: un `fd` aperto in scrittura resta scrivibile dopo il cambio
/// d'identita'.
///
/// # La precondizione: un thread solo
///
/// Fra lo scatto dell'elenco e la lettura di ogni `fdinfo` passa del tempo, e
/// in quel tempo **un altro thread potrebbe chiudere un descrittore**. In un
/// processo monothread non accade, e lo spawner quella condizione la accerta
/// come primo passo, prima di chiamare qui.
///
/// Un descrittore **sparito** fra lo scatto e la lettura non e' pero' un
/// difetto in nessuno dei due casi, ed e' l'unica forma di fallimento che si
/// tollera: un `fd` che non esiste non da' autorita' a nessuno, e rifiutare
/// per la sua assenza sarebbe fail-closed su un pericolo che non c'e'. La
/// distinzione e' fra «non esiste» e «non si legge»: la prima si salta, la
/// seconda resta un rifiuto.
///
/// # Errors
///
/// Qualunque voce che non si riesca a leggere per una ragione diversa
/// dall'assenza. Un descrittore su cui non sappiamo dire se e' scrivibile e'
/// un descrittore che non possiamo escludere.
fn descrittori_scrivibili() -> std::result::Result<Vec<Descrittore>, String> {
    let cartella = rustix::fs::open(
        "/proc/self/fd",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|errore| format!("/proc/self/fd: {errore}"))?;

    let mut numeri = Vec::new();
    let mut elenco =
        rustix::fs::Dir::new(cartella).map_err(|errore| format!("/proc/self/fd: {errore}"))?;
    let proprio = elenco
        .fd()
        .map_err(|errore| format!("/proc/self/fd: {errore}"))?
        .as_raw_fd();
    while let Some(voce) = elenco.read() {
        let voce = voce.map_err(|errore| format!("/proc/self/fd: {errore}"))?;
        let nome = voce.file_name().to_string_lossy().into_owned();
        if nome == "." || nome == ".." {
            continue;
        }
        let descrittore: i32 = nome
            .parse()
            .map_err(|_| format!("/proc/self/fd contiene «{nome}», che non e' un descrittore"))?;
        if descrittore != proprio {
            numeri.push(descrittore);
        }
    }
    // L'enumerazione e' finita: il proprio descrittore si chiude qui, prima di
    // guardare gli altri, cosi' non resta aperto nemmeno per il tempo delle
    // letture.
    drop(elenco);

    let mut scrivibili = Vec::new();
    for numero in numeri {
        let percorso_info = format!("/proc/self/fdinfo/{numero}");
        let info = match leggi_limitato(Path::new(&percorso_info)) {
            Ok(info) => info,
            // Sparito fra lo scatto e la lettura: non esiste, quindi non da'
            // autorita' a nessuno.
            Err(errore) if errore.e_assenza() => continue,
            Err(errore) => return Err(errore.to_string()),
        };
        let flags = info
            .lines()
            .find_map(|riga| riga.strip_prefix("flags:"))
            .ok_or_else(|| format!("/proc/self/fdinfo/{numero} non ha il campo flags"))?;
        let flags = u32::from_str_radix(flags.trim(), 8).map_err(|_| {
            format!(
                "/proc/self/fdinfo/{numero}: flags «{}» non e' ottale",
                flags.trim()
            )
        })?;
        // `O_WRONLY` vale 1 e `O_RDWR` 2; `O_RDONLY` e' 0, quindi il modo sta
        // nei due bit bassi e non in un singolo bit. `trailing_zeros() >= 2`
        // dice la stessa cosa e la dice peggio: nasconde che si stanno
        // guardando due bit noti dietro una proprieta' aritmetica.
        #[allow(
            clippy::verbose_bit_mask,
            reason = "i due bit bassi sono il modo di accesso"
        )]
        if flags & 0b11 == 0 {
            continue;
        }
        let collegamento = format!("/proc/self/fd/{numero}");
        let bersaglio = match std::fs::read_link(&collegamento) {
            Ok(bersaglio) => bersaglio,
            Err(errore) if errore.kind() == std::io::ErrorKind::NotFound => continue,
            Err(errore) => return Err(format!("{collegamento}: {errore}")),
        };
        // `metadata` segue il link, ed e' cio' che serve: il filesystem che
        // conta e' quello dell'oggetto aperto.
        let dati = match std::fs::metadata(&collegamento) {
            Ok(dati) => dati,
            Err(errore) if errore.kind() == std::io::ErrorKind::NotFound => continue,
            Err(errore) => return Err(format!("{collegamento}: {errore}")),
        };
        scrivibili.push(Descrittore {
            percorso: bersaglio.to_string_lossy().into_owned(),
            dispositivo: dati.dev(),
        });
    }
    scrivibili.sort_by(|a, b| a.percorso.cmp(&b.percorso));
    Ok(scrivibili)
}

#[cfg(test)]
mod tests {
    use super::{
        booleano, gruppi, maschera, quaterna, Autorita, Capability, Descrittore, Identita,
    };

    const CGROUP2: u64 = 0x1234;

    /// I namespace che il preflight ha osservato: gli stessi che [`spogliata`]
    /// dichiara, cosi' l'asse non interferisce con i casi che provano gli
    /// altri.
    fn attesi() -> Vec<(String, String)> {
        ["user", "pid", "cgroup", "mnt"]
            .into_iter()
            .map(|nome| (nome.to_owned(), format!("{nome}:[4026531837]")))
            .collect()
    }

    /// Un'identita' senza autorita': il caso che deve passare.
    fn spogliata() -> Identita {
        Identita {
            uid: [1000; 4],
            gid: [1000; 4],
            gruppi: Vec::new(),
            no_new_privs: true,
            capability: Capability {
                // Un bounding set pieno e' la norma, e non e' autorita'.
                bounding: 0x0000_003f_ffff_ffff,
                ..Capability::default()
            },
            namespace: attesi(),
            descrittori_scrivibili: vec![Descrittore {
                percorso: "/tmp/output.arrow".to_owned(),
                dispositivo: 0x9999,
            }],
        }
    }

    #[test]
    fn un_identita_spogliata_non_ha_autorita_residua() {
        assert!(
            spogliata().autorita_residua(CGROUP2, &attesi()).is_empty(),
            "un bounding pieno e un fd su un altro filesystem non sono autorita'"
        );
    }

    /// Ciascuno degli assi, da solo, e' autorita'.
    ///
    /// Un caso per asse e non un caso solo: un giudizio che si fermasse al
    /// primo motivo passerebbe un test che ne prova uno, e lascerebbe gli
    /// altri senza copertura.
    #[test]
    fn ogni_asse_da_solo_e_autorita() {
        let mut root = spogliata();
        root.uid = [0; 4];
        assert!(matches!(
            root.autorita_residua(CGROUP2, &attesi()).first(),
            Some(Autorita::Identita(_))
        ));

        // Nessuno degli UID e' zero, eppure l'autorita' c'e': `setresuid` puo'
        // tornare al salvato senza alcun permesso.
        let mut salvato = spogliata();
        salvato.uid = [1000, 1000, 0, 1000];
        assert!(!salvato.autorita_residua(CGROUP2, &attesi()).is_empty());

        let mut con_gruppi = spogliata();
        con_gruppi.gruppi = vec![27];
        assert!(matches!(
            con_gruppi.autorita_residua(CGROUP2, &attesi()).first(),
            Some(Autorita::Gruppi(_))
        ));

        for regola in [
            |c: &mut Capability| c.permitted = 1,
            |c: &mut Capability| c.effective = 1,
            |c: &mut Capability| c.inheritable = 1,
            |c: &mut Capability| c.ambient = 1,
        ] {
            let mut identita = spogliata();
            regola(&mut identita.capability);
            assert!(matches!(
                identita.autorita_residua(CGROUP2, &attesi()).first(),
                Some(Autorita::Capability(_, _))
            ));
        }

        let mut nnp = spogliata();
        nnp.no_new_privs = false;
        assert!(matches!(
            nnp.autorita_residua(CGROUP2, &attesi()).first(),
            Some(Autorita::NoNewPrivs)
        ));
    }

    /// Un `fd` sullo **stesso filesystem** del control plane e' autorita',
    /// anche se il percorso non ne e' un prefisso.
    ///
    /// E' il caso del bind mount: percorso diverso, superblocco lo stesso, e la
    /// scrittura arriva ugualmente. Un confronto per prefisso lo lascerebbe
    /// passare.
    #[test]
    fn un_fd_sullo_stesso_filesystem_e_autorita_anche_per_un_altro_percorso() {
        let mut identita = spogliata();
        identita.descrittori_scrivibili.push(Descrittore {
            percorso: "/mnt/alias/plenora/memory.max".to_owned(),
            dispositivo: CGROUP2,
        });
        assert!(
            matches!(
                identita.autorita_residua(CGROUP2, &attesi()).first(),
                Some(Autorita::Descrittori(_))
            ),
            "il cambio d'identita' non revoca un fd gia' aperto, e un alias e' lo stesso fs"
        );
    }

    /// Il bounding set pieno, da solo, **non** e' autorita'.
    #[test]
    fn un_bounding_set_pieno_non_e_autorita() {
        let mut identita = spogliata();
        identita.capability.bounding = u64::MAX;
        assert!(identita.autorita_residua(CGROUP2, &attesi()).is_empty());
    }

    // # Perche' la scansione reale dei descrittori non si prova qui
    //
    // Perche' cio' che la scansione fa dipende dalla **tabella dei descrittori
    // del processo**, e un binario di test la condivide fra tutti i casi che
    // girano in parallelo. Fra lo scatto dell'elenco e la lettura di ogni
    // `fdinfo` un altro thread apre e chiude file, e un caso che dipende da
    // quel contenuto non e' deterministico: sarebbe verde o rosso a seconda di
    // che cosa stanno facendo gli altri, e un caso che fallisce a caso smette
    // di essere letto.
    //
    // La cosa e' peggiore di un semplice colore instabile. Quello che il caso
    // dovrebbe sorvegliare — che la scansione non apra un secondo descrittore
    // che poi nessuno esclude — si manifesta come un `fdinfo` che non si legge.
    // Ma la tolleranza dell'assenza, che qui e' necessaria e giusta, salta
    // proprio quei descrittori: in un runner condiviso il difetto che il caso
    // esiste per vedere passerebbe **inosservato**, e il verde direbbe che non
    // c'e' invece che dire che non e' stato guardato.
    //
    // Dichiararlo deterministico sarebbe quindi due volte falso, e un caso che
    // afferma piu' di quanto misura e' peggio di nessun caso: qui restano le
    // sole prove di parsing puro, che non toccano `/proc`.
    //
    // Dove si prova davvero: nel processo dello spawner, che e' monothread per
    // costruzione — il primo passo della sequenza lo accerta — e che nessun
    // altro perturba. Il gate `scripts/verifica_isolamento_linux.sh` lo esegue
    // sulla VM dedicata, ed e' l'unico posto in cui l'affermazione «non colano
    // descrittori» e' misurabile.
    //
    // (Nota di modulo, non doc di un elemento: il caso non esiste, e questa e'
    // la ragione per cui non esiste.)

    /// Ogni forma malformata di `/proc/self/status` e' un errore, non un valore
    /// di ripiego.
    ///
    /// E' il caso che distingue «non c'e' autorita'» da «non lo sappiamo»: un
    /// parser indulgente rende i due indistinguibili, e il secondo diventa un
    /// lasciapassare.
    #[test]
    fn ogni_campo_malformato_e_un_errore() {
        assert!(quaterna("Uid:\t1000 1000 1000 1000\n", "Uid").is_ok());
        assert!(
            quaterna("Uid:\t1000 1000 1000\n", "Uid").is_err(),
            "tre valori invece di quattro"
        );
        assert!(
            quaterna("Uid:\t1000 x 1000 1000\n", "Uid").is_err(),
            "un valore che non e' un numero"
        );
        assert!(quaterna("Gid:\t0 0 0 0\n", "Uid").is_err(), "campo assente");

        assert!(gruppi("Groups:\t\n").is_ok(), "vuoto e' legittimo");
        assert!(gruppi("Groups:\t4 24\n").is_ok());
        assert!(gruppi("Uid:\t0\n").is_err(), "assente non e' vuoto");
        assert!(gruppi("Groups:\tx\n").is_err());

        assert!(booleano("NoNewPrivs:\t1\n", "NoNewPrivs").expect("uno"));
        assert!(!booleano("NoNewPrivs:\t0\n", "NoNewPrivs").expect("zero"));
        assert!(booleano("NoNewPrivs:\t2\n", "NoNewPrivs").is_err());
        assert!(booleano("Altro:\t1\n", "NoNewPrivs").is_err());

        assert_eq!(
            maschera("CapPrm:\t000001ffffffffff\n", "CapPrm").expect("maschera"),
            0x1ff_ffff_ffff
        );
        assert!(maschera("CapPrm:\tzzz\n", "CapPrm").is_err());
        assert!(maschera("CapEff:\t0\n", "CapPrm").is_err());
    }
}
