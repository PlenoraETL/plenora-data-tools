//! Lo spawner: entra nel dominio, si spoglia dell'autorita', esegue.
//!
//! # Perche' un processo dedicato
//!
//! Perche' la sequenza deve girare **fra la nascita del processo e la `exec`**,
//! e li' non si va senza `unsafe`: `CommandExt::pre_exec` e' `unsafe`, e questo
//! progetto non ne ammette.
//!
//! La strada senza `unsafe` e' spostare la sequenza in un processo **suo**: il
//! supervisore lancia lo spawner con un `Command::spawn` ordinario, lo spawner
//! — che e' un processo normale, non un intervallo fra `fork` ed `exec` —
//! esegue tutti i passi con chiamate sicure, e finisce con `CommandExt::exec`,
//! che **e' safe** e sostituisce l'immagine senza tornare.
//!
//! # Il vincolo del thread singolo, che non e' un dettaglio
//!
//! `rustix::thread::{set_thread_groups, set_thread_res_gid, set_thread_res_uid}`
//! sono i syscall **per-thread**: cambiano le credenziali del solo thread
//! chiamante, non del processo. Il wrapper di glibc le propaga a tutti i thread
//! con un segnale; queste no.
//!
//! In uno spawner monothread la differenza non esiste — c'e' un thread solo, e
//! la `exec` conserva le credenziali del chiamante uccidendo gli altri. In un
//! processo multithread la differenza e' un buco: gli altri thread restano
//! privilegiati, e uno di essi puo' fare cio' che al thread spogliato e'
//! vietato.
//!
//! Da qui il **primo** passo della sequenza, che verifica `/proc/self/task` e
//! rifiuta se i task non sono esattamente uno. E da qui il divieto: **queste
//! API valgono solo qui**, e un chiamante multithread non deve usarle.
//!
//! # La sequenza e' fail-closed
//!
//! Sette passi, in quest'ordine, e nessun errore intermedio si ignora o si
//! compensa proseguendo:
//!
//! 1. lo spawner e' monothread;
//! 2. si entra nel cgroup, e si rilegge l'appartenenza;
//! 3. si scrive e si rilegge `oom_score_adj = 0`;
//! 4. non restano descrittori scrivibili verso il control plane;
//! 5. si imposta `no_new_privs`;
//! 6. si svuotano i gruppi supplementari, poi GID e UID reali, effettivi e
//!    salvati;
//! 7. si rileggono identita', gruppi, capability e `no_new_privs`, e si esegue.
//!
//! # Che cosa il gate ostile deve provare, e che i casi qui non provano
//!
//! I casi deterministici provano le regole; l'ambiente no. Queste tre cose
//! esistono solo su una macchina vera, e senza di esse resterebbero
//! affermazioni:
//!
//! 1. **la sentinella sul dispatch**: questa stessa immagine, rieseguita con
//!    `argv[1]` uguale alla versione della richiesta, arriva in modalita'
//!    spawner con **un task solo**. E' cio' che un obbligo scritto non
//!    garantisce: un `main` che avvia un pool di thread prima di guardare
//!    `argv` compila, passa ogni caso qui, e rompe il passo 1 solo a runtime;
//! 2. **l'immagine sostituita**. La proprieta' da provare e' che il binario
//!    sostitutivo **non parta mai**, e non che parta sempre quello iniziale:
//!    quest'ultima e' falsa, perche' una `rename` sopra il pathname originario
//!    toglie l'ultimo collegamento all'inode e fa comparire ` (deleted)` nel
//!    bersaglio di `/proc/self/exe`, che [`accerta_immagine`] rifiuta. La stessa
//!    prova non puo' pretendere il rifiuto e la partenza.
//!
//!    Gli esiti ammessi sono quindi **due**, e la prova passa con entrambi:
//!    la sostituzione avviene prima del controllo, e l'esito e'
//!    [`TransizioneFallita`]; oppure avviene dopo, e parte l'inode iniziale
//!    attraverso `/proc/self/exe`.
//!
//!    Il secondo ramo si distingue solo con una **barriera controllata** fra il
//!    controllo e lo `spawn`: una corsa temporizzata — sostituire e sperare di
//!    aver colpito la finestra giusta — non separa «l'inode iniziale e' partito
//!    perche' il codice e' giusto» da «e' partito perche' la sostituzione e'
//!    arrivata tardi». La barriera va **dopo** l'accertamento dell'immagine,
//!    che non e' negoziabile: e' il parametro `dopo_accertamento` di `tenta`.
//!    Cio' che quel parametro non puo' fare e' saltare il controllo e cambiare
//!    l'inode che `/proc/self/exe` raggiunge; **rendere obsolete altre
//!    osservazioni invece si**, ed e' proprio quello che il gate fa — la
//!    `rename` invalida la fotografia ` (deleted)` che il controllo ha appena
//!    scattato.
//!
//!    La produzione ci passa una callback vuota. L'ingresso che ne passa una
//!    vera vive sotto `#[cfg(test)]` oppure in un binario **solo** di
//!    qualificazione, dietro un `cfg` di riga di comando: non dietro una
//!    feature, che l'unificazione propaga a chi non l'ha chiesta. Chi
//!    costruisce puo' comunque accenderlo di proposito, e va detto: il
//!    perimetro protegge dall'incidente, non dall'intenzione.
//!
//!    Va detto anche cio' che il controllo ` (deleted)` **non** e': una
//!    garanzia all'istante della `exec`. E' una fotografia, e fra lo scatto e
//!    la `exec` il pathname puo' cambiare ancora. Cio' che regge non e' quel
//!    controllo ma l'esecuzione di `/proc/self/exe`, che al nome non torna;
//! 3. **la separazione di privilegio**: che un worker spogliato non possa
//!    riscrivere i quattro controlli, non possa scrivere il `cgroup.procs` del
//!    padre, e che dopo una `unshare` — riuscita o rifiutata dalla policy — lo
//!    stato resti invariato.
//!
//! Il gate e' `scripts/verifica_isolamento_linux.sh`, e fallisce quando i
//! prerequisiti mancano invece di saltare verde.
//!
//! L'ordine ha una ragione a ogni giunzione. Il cgroup **prima** dell'identita'
//! perche' entrarci richiede di scrivere nella gerarchia, e dopo la
//! `setresuid` non si potrebbe piu'. `no_new_privs` **prima** del cambio
//! d'identita' perche' e' cio' che impedisce a una `exec` successiva di
//! riguadagnare privilegi via setuid: dopo, sarebbe una porta chiusa quando
//! qualcuno e' gia' passato. I gruppi **prima** del GID perche' `setgroups`
//! richiede autorita' che il cambio di GID toglie.

use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use plenora_core::error::{PlenoraError, Result};
use rustix::process::{Gid, Uid};

use super::canale;
use super::dominio::Gerarchia;
#[cfg(any(test, feature = "internals"))]
use super::figlio as figlio_guardia;
#[cfg(any(test, feature = "internals"))]
use super::figlio::FiglioVivo;
use super::identita::{leggi_identita, namespace_del_padre, rileggi_credenziali, Identita};
use super::lettura::leggi_limitato;
use super::{
    non_disponibile, DominioRivalidato, IdentitaWorker, Montaggio, ProprietaFile, RichiestaSpawner,
    VERSIONE_RICHIESTA,
};
// Cio' che serve al solo avvio: il supervisore non ha ancora un chiamante di
// produzione, e l'import lo dichiara insieme a cio' che importa.
#[cfg(any(test, feature = "internals"))]
use super::{
    esito, spawner_ammissibile, DominioPreparato, TentativoFallito, TransizioneFallita,
    TransizioneRiuscita,
};

/// Avvia lo spawner sul dominio appena preparato.
///
/// # Perche' consuma il token
///
/// Perche' il preflight prepara **un** dominio e ne avvia **uno** spawner.
/// Prendere il token per riferimento permetterebbe di avviarne due sullo stesso
/// dominio: il secondo troverebbe la quiescenza gia' rotta dal primo, e la
/// troverebbe rotta per una ragione che il rifiuto non sa distinguere da un
/// dominio altrui.
///
/// # Che cosa attraversa il confine
///
/// Solo [`RichiestaSpawner`], che dice su che cosa lavorare e non afferma
/// niente. Il token resta qui e muore qui: non e' trasmissibile, e non c'e'
/// nessuna forma in cui lo spawner possa riceverlo e crederci.
///
/// L'evidenza esce invece **da questa parte**, insieme al figlio: e' cio' che
/// il preflight ha osservato, vale dopo la transizione, e non e' una prova che
/// lo spawner debba ricevere.
///
/// # Quale binario, e perche' non lo sceglie il chiamante
///
/// Lo spawner e' **questa stessa immagine**, rieseguita. Il percorso viene dal
/// kernel — `/proc/self/exe` — e non da un argomento: un percorso scelto dal
/// chiamante renderebbe questa funzione un `Command::spawn` qualunque, capace
/// di rendere un figlio nato fuori dal dominio, con l'identita' del supervisore
/// e senza nessuno dei sette passi, e indistinguibile per il chiamante da una
/// transizione riuscita.
///
/// Il figlio si riconosce come spawner perche' il suo `argv[1]` e'
/// [`VERSIONE_RICHIESTA`]. Da qui un obbligo per il chiamante di produzione, che
/// vale prima di ogni altra cosa che faccia all'avvio — thread compresi, perche'
/// il primo passo della sequenza pretende un processo monothread: se `argv[1]`
/// e' quella stringa, il programma e' uno spawner e passa la mano a
/// [`dal_confine`].
///
/// L'obbligo va **provato**, non dichiarato: un `main` che crea un pool di
/// thread prima di guardare `argv` compila, passa ogni caso deterministico, e
/// rende impossibile il passo 1 solo a runtime e solo sulla macchina vera. La
/// sentinella del gate ostile riesegue quindi questa stessa immagine con
/// `argv[1]` uguale a [`VERSIONE_RICHIESTA`] e pretende che arrivi in modalita'
/// spawner con un task solo.
///
/// # Errors
///
/// [`TransizioneFallita`], che porta la causa **e** l'evidenza. E' in un `Box`
/// perche' porta tutto cio' che il preflight ha osservato — percorsi, montaggio,
/// namespace — ed e' quindi molto piu' grande dell'esito riuscito: senza,
/// **ogni** chiamata pagherebbe in pila la dimensione del ramo raro.
#[cfg(any(test, feature = "internals"))]
pub(super) fn avvia(
    preparato: DominioPreparato,
    da_eseguire: &DaEseguire<'_>,
) -> std::result::Result<TransizioneRiuscita, Box<TransizioneFallita>> {
    // Nessuna giuntura: le callback vuote sono cio' che la produzione passa, e
    // l'unica cosa che un chiamante di qualificazione puo' variare e' **se**
    // fermarsi o fallire in quei due punti, mai che cosa si controlla.
    avvia_interno(preparato, da_eseguire, || Ok(()), || Ok(()))
}

/// Il corpo condiviso fra [`avvia`] e la sua variante con barriera.
///
/// Sta qui e non dentro `avvia` perche' la variante con barriera vive nel
/// perimetro di qualificazione, e due copie della sequenza sarebbero due
/// sequenze che possono divergere — proprio quella che il gate misura.
/// Il tentativo vero e proprio.
///
/// # L'ordine dei due, e perche' non e' invertibile
///
/// `accerta_immagine` viene **prima** e non passa da nessun parametro. E' la
/// condizione che rende lo spawner uno spawner — immagine non cancellata,
/// regolare, non riscrivibile dal worker — e cederla al chiamante la
/// renderebbe facoltativa: chi passasse un accertamento vuoto avrebbe lo
/// `spawn` senza nessun controllo. Il binario resterebbe `/proc/self/exe`, ma
/// l'invariante non varrebbe piu' per costruzione, e varrebbe solo finche'
/// tutti i chiamanti si comportano bene.
///
/// `dopo_accertamento` viene **dopo**. E' la barriera che il gate ostile ha
/// bisogno di inserire per sostituire il binario **fra** il controllo e lo
/// `spawn`, che e' l'unico modo di distinguere «l'inode iniziale e' partito
/// perche' il codice e' giusto» da «e' partito perche' la sostituzione e'
/// arrivata tardi».
///
/// # Che cosa la barriera puo' e non puo' fare
///
/// Non puo' **saltare** il controllo, perche' non lo sostituisce: quando viene
/// chiamata, l'accertamento e' gia' avvenuto. E non puo' cambiare **l'inode
/// che `/proc/self/exe` raggiunge**, che e' quello di questo processo e non
/// dipende da nessun nome.
///
/// Puo' invece rendere obsolete le altre osservazioni, ed e' precisamente cio'
/// che il gate fa: rinominando il pathname invalida la fotografia
/// ` (deleted)` appena scattata. Dire che «non puo' disfare cio' che il
/// controllo ha stabilito» sarebbe quindi falso — di quel controllo restano
/// vere solo le conclusioni che riguardano l'inode, e la ragione per cui basta
/// e' che l'inode e' anche l'unica cosa che si esegue.
///
/// La produzione passa una callback vuota. Che nessun altro possa passarne una
/// diversa e' garantito dal fatto che questa funzione e' **privata** e ha un
/// solo chiamante: l'ingresso che serve al gate vive sotto `#[cfg(test)]` o in
/// un binario solo di qualificazione, mai dietro una feature — perche' una
/// feature l'unificazione la propaga, e ci si arriverebbe senza averlo
/// chiesto.
#[cfg(any(test, feature = "internals"))]
fn tenta(
    richiesta: &RichiestaSpawner,
    worker: IdentitaWorker,
    da_eseguire: &DaEseguire<'_>,
    dopo_accertamento: impl FnOnce() -> Result<()>,
    estremi: &canale::EstremiDelWorker,
) -> std::result::Result<FiglioVivo<std::process::Child>, Box<TentativoFallito>> {
    accerta_immagine(worker)?;
    dopo_accertamento()?;

    // 1. Il comando si costruisce **mentre tutto e' ancora `CLOEXEC`**.
    //
    // Non e' un ordine di comodo: costruire gli argomenti dopo aver reso
    // ereditabili i descrittori allungherebbe la finestra di tutto cio' che
    // serve a costruirli — allocazioni, formattazioni, e ogni loro possibile
    // fallimento.
    //
    // Si esegue `/proc/self/exe`, non il nome che quel collegamento risolve: il
    // nome puo' essere sostituito fra il giudizio e questa riga — una `rename`
    // e' atomica — mentre il collegamento resta legato all'immagine di questo
    // processo.
    let mut comando = std::process::Command::new(IMMAGINE);
    comando
        .args(richiesta.in_argomenti())
        .arg("--")
        .arg(da_eseguire.eseguibile)
        .args(da_eseguire.argomenti);

    // 2. Il monothread si accerta **adesso**, immediatamente prima della prima
    //    modifica: fra l'avvio e questo punto un thread puo' essere nato.
    canale::accerta_monothread()?;

    // 3. `CLOEXEC` via ai due estremi del worker, e a nient'altro.
    estremi.rendi_ereditabili()?;

    // Il braccio «spawn»: il fallimento arriva **con entrambi gli estremi
    // ereditabili**, che e' lo stato piu' esposto della sequenza.
    #[cfg(qualificazione_isolamento)]
    canale::guasto_richiesto("spawn").map_err(Box::<TentativoFallito>::from)?;

    // 4. Lo `spawn`, subito. Fra il passo 3 e questa riga non c'e' niente.
    //
    //    Il figlio nasce **dentro la guardia**, nella stessa espressione che lo
    //    crea. Non e' uno stile: fra lo `spawn` e un incapsulamento fatto una
    //    riga dopo ci sarebbe un tratto in cui il processo esiste e nessuno lo
    //    custodisce, ed e' precisamente il tratto in cui un `?` aggiunto un
    //    domani lo lascerebbe andare.
    comando
        .spawn()
        .map(FiglioVivo::nuovo)
        .map_err(|errore| Box::<TentativoFallito>::from(passo("avvio", &errore.to_string())))

    // 5. Il rilascio degli estremi non sta qui ma nel chiamante, che li lascia
    //    cadere su **entrambi** i cammini. Farlo qui vorrebbe dire prendere la
    //    guardia per valore, e allora un ritorno anticipato prima del passo 3
    //    la consumerebbe senza che nessuno l'abbia ancora resa ereditabile:
    //    corretto, ma per un motivo diverso da quello che serve. Cosi' invece
    //    la regola e' una sola, e vale per tutti i cammini.
}

/// Il collegamento che il kernel tiene legato all'immagine di questo processo.
#[cfg(any(test, feature = "internals"))]
const IMMAGINE: &str = "/proc/self/exe";

/// Che l'immagine in esecuzione sia rieseguibile.
///
/// # Le due letture, e perche' in quest'ordine
///
/// Il **nome** si legge con `read_link`, che rende il bersaglio cosi' com'e',
/// suffisso ` (deleted)` compreso. L'**inode** si interroga invece attraverso
/// `/proc/self/exe`, che si risolve all'immagine anche quando quel nome non
/// esiste piu'.
///
/// Interrogare il nome sarebbe sbagliato due volte. Su un'immagine rimossa
/// fallirebbe con `NotFound` prima ancora che qualcuno guardi il suffisso, e il
/// rifiuto arriverebbe con la ragione sbagliata. E su un'immagine sostituita
/// riuscirebbe, descrivendo pero' il file **nuovo**: proprietario e permessi
/// giudicati sarebbero di un binario che non e' questo.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se `/proc/self/exe` non si legge o
/// non si interroga, o se una delle tre condizioni manca.
#[cfg(any(test, feature = "internals"))]
fn accerta_immagine(worker: IdentitaWorker) -> Result<()> {
    let percorso = Path::new(IMMAGINE);
    let bersaglio = std::fs::read_link(percorso)
        .map_err(|errore| passo("immagine in esecuzione", &format!("{IMMAGINE}: {errore}")))?;
    let dati = std::fs::metadata(percorso)
        .map_err(|errore| passo("immagine in esecuzione", &format!("{IMMAGINE}: {errore}")))?;
    spawner_ammissibile(
        &bersaglio,
        dati.is_file(),
        ProprietaFile {
            uid: dati.uid(),
            gid: dati.gid(),
            mode: dati.mode(),
        },
        worker,
    )
}

/// L'ingresso dello spawner: legge la richiesta, rivalida, esegue.
///
/// # Perche' rivalida invece di ricevere una prova
///
/// Perche' una prova sarebbe qualcosa che questo processo accetta per buona, e
/// un processo che crede a cio' che gli viene detto non aggiunge nessuna
/// garanzia a quella del mittente. Qui si riguarda tutto: ambiente, percorsi,
/// montaggio, permessi, namespace e i quattro controlli — e l'esito e' un
/// [`DominioRivalidato`] **locale**, che nessuno ha spedito.
///
/// # Che cosa **non** fa, e perche' non e' una svista
///
/// Non scrive i quattro controlli: li rilegge. Il tetto deve essere gia' in
/// vigore quando questo processo nasce (`F4-1`, `GA-7`), e scriverlo qui
/// vorrebbe dire che fra la nascita e il limite c'e' una finestra. Spostare
/// l'intero preflight qui cambierebbe la macchina a stati documentata, e non e'
/// una cosa che si fa di straforo.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] per una richiesta malformata, per una
/// rivalidazione che non regge, o per un passo della sequenza che cede.
pub(super) fn dal_confine(argomenti: &[std::ffi::OsString]) -> Result<std::convert::Infallible> {
    let (grezza, da_eseguire) = spacca(argomenti)?;
    let richiesta =
        RichiestaSpawner::da_argomenti(grezza).map_err(|motivo| passo("richiesta", &motivo))?;

    // Stadio 2: i due descrittori ereditati si riguardano **qui**, prima di
    // entrare nel dominio e prima di spogliarsi dell'autorita'.
    //
    // Prima, perche' un canale che non regge e' un motivo per non proseguire, e
    // proseguire vorrebbe dire configurare un dominio e cambiare identita' per
    // poi accorgersene dopo — quando rifiutare costa un rimedio invece di un
    // ritorno.
    //
    // E si **riguardano**, non si ricevono: cio' che attraversa il confine sono
    // due numeri, che non affermano niente. Il supervisore li ha verificati nel
    // proprio processo, ma quella verifica riguarda i **suoi** descrittori; qui
    // sono altri numeri in un'altra tabella, e l'unica cosa che li lega e'
    // un'affermazione del chiamante.
    // La coppia **rivalidata qui** e' cio' che il worker ricevera', e non i due
    // numeri della richiesta: sono gli stessi valori solo finche' nessuno mente,
    // e il senso di questo passo e' proprio non doverlo dare per scontato. Il
    // valore non si costruisce, si **riceve** da chi lo ha verificato: non c'e'
    // un modo di ottenerne uno senza passare di li'.
    let canale_del_worker =
        canale::accerta_coppia(richiesta.worker_legge, richiesta.worker_scrive)?;
    // La superficie si costruisce **dalla richiesta**, e non arriva da un
    // chiamante: e' l'unica forma in cui «lo spawner rivalida da se'» non
    // dipende da chi lo ha invocato. `Gerarchia::nuova` canonicalizza, e
    // `rivalida` pretende poi che il canonico coincida col nome ricevuto — che
    // e' il confronto che smaschera un percorso indiretto.
    let gerarchia = Gerarchia::nuova(&richiesta.dominio, &richiesta.radice)
        .map_err(|difetto| passo("gerarchia", &difetto.to_string()))?;
    let padre = namespace_del_padre().map_err(|errore| passo("namespace del padre", &errore))?;
    let rivalidato = super::rivalida(&gerarchia, &richiesta, padre)?;
    entra_ed_esegui(rivalidato, canale_del_worker, &da_eseguire)
}

/// [`avvia`] con una barriera fra l'accertamento dell'immagine e lo `spawn`.
///
/// # Perche' esiste, e perche' non esiste in produzione
///
/// Esiste per **una** prova: che a essere eseguito sia l'inode e non il nome.
/// Dimostrarlo richiede di sostituire il binario mentre il processo e' fermo
/// fra il controllo e lo `spawn`, e senza un punto in cui fermarlo resterebbe
/// una corsa temporizzata — sostituire e sperare di aver colpito la finestra,
/// che non separa «e' partito l'inode giusto perche' il codice e' giusto» da
/// «perche' la sostituzione e' arrivata tardi».
///
/// Non entra in produzione per incidente perche' `qualificazione_isolamento`
/// non e' una feature: e' un `cfg` che si passa a `rustc`. Una feature la si
/// abilita dichiarandola fra le dipendenze, e l'unificazione la propaga anche a
/// chi non l'ha chiesta; un `cfg` non si propaga. Chi costruisce puo'
/// comunque metterlo in `RUSTFLAGS`: la garanzia e' contro l'incidente, non
/// contro l'intenzione.
///
/// La barriera non puo' saltare l'accertamento — quando corre, quello e' gia'
/// avvenuto — ne' cambiare l'inode che `/proc/self/exe` raggiunge. Rende invece
/// obsolete le osservazioni sul nome, ed e' esattamente cio' che il gate le
/// chiede di fare.
///
/// # Errors
///
/// [`TransizioneFallita`], che porta la causa e l'evidenza.
#[cfg(qualificazione_isolamento)]
pub(super) fn avvia_con_giunture(
    preparato: DominioPreparato,
    da_eseguire: &DaEseguire<'_>,
    prima_dello_spawn: impl FnOnce() -> Result<()>,
    dopo_lo_spawn: impl FnOnce() -> Result<()>,
) -> std::result::Result<TransizioneRiuscita, Box<TransizioneFallita>> {
    avvia_interno(preparato, da_eseguire, prima_dello_spawn, dopo_lo_spawn)
}

#[cfg(any(test, feature = "internals"))]
fn avvia_interno(
    preparato: DominioPreparato,
    da_eseguire: &DaEseguire<'_>,
    prima_dello_spawn: impl FnOnce() -> Result<()>,
    dopo_lo_spawn: impl FnOnce() -> Result<()>,
) -> std::result::Result<TransizioneRiuscita, Box<TransizioneFallita>> {
    // Il canale nasce **prima** della richiesta, perche' la richiesta ne porta
    // i numeri. Se non si apre, non c'e' niente da chiedere: resta l'evidenza,
    // perche' il dominio e' gia' configurato.
    let (sup_legge, sup_scrive, estremi) = match canale::apri() {
        Ok(canale) => canale,
        Err(causa) => {
            return Err(Box::new(TransizioneFallita {
                causa,
                evidenza: preparato.solo_evidenza(),
                // Nessun figlio e' mai esistito: non c'e' niente da rimediare,
                // e dirlo e' un'informazione, non un'assenza.
                difetto_di_pulizia: None,
            }));
        }
    };
    // I numeri si riguardano prima di metterli nella richiesta: `apri` li ha
    // appena verificati, e riguardarli costa due letture — ma e' l'unico modo in
    // cui il tipo che li porta significa «verificati» invece di «passati di
    // qui».
    let numeri = match estremi.numeri() {
        Ok(numeri) => numeri,
        Err(causa) => {
            return Err(Box::new(TransizioneFallita {
                causa,
                evidenza: preparato.solo_evidenza(),
                difetto_di_pulizia: None,
            }));
        }
    };
    let (richiesta, evidenza) = preparato.consuma(numeri);
    let tentativo = tenta(
        &richiesta,
        evidenza.worker,
        da_eseguire,
        prima_dello_spawn,
        &estremi,
    );
    // Gli estremi del worker cadono **qui**, su entrambi i cammini: la guardia
    // esce di scena prima che l'esito venga costruito, quindi non c'e' ritorno
    // che li lasci vivi nel supervisore.
    drop(estremi);
    // `esito` resta puro — non conosce ne' le pipe ne' il figlio, ed e' generico
    // proprio per questo: la sua regola e' come si compone un fallimento, e non
    // cambia con cio' che il tentativo rende.
    //
    // Il `?` qui e' sicuro perche' su questo cammino **nessun figlio esiste**:
    // `tenta` fallisce prima dello `spawn` o sullo `spawn` stesso.
    let (figlio, evidenza) = esito(tentativo, evidenza)?;

    // Da qui in poi un fallimento ha un figlio da chiudere, e ci si passa da
    // **un punto solo**. Non e' eleganza: due punti di rimedio sono due
    // occasioni di divergere, e quella che diverge e' sempre la seconda.
    if let Err(causa) = dopo_lo_spawn() {
        // Si conservano **entrambi** i difetti. La causa dice perche' la
        // transizione non e' riuscita; il difetto di pulizia dice che cosa e'
        // rimasto — e sono due fatti diversi, che il supervisore usa in due
        // momenti diversi. Sostituire il primo col secondo direbbe che il
        // problema e' la pulizia, che e' la diagnosi sbagliata.
        // L'uscita non serve qui: su questo cammino la transizione non e'
        // avvenuta, e **come** il figlio e' morto non aggiunge niente a
        // «l'avvio e' fallito». Chi la usa e' il supervisore, che raccoglie un
        // figlio che ha lavorato.
        let difetto_di_pulizia = match figlio.termina_e_raccogli(
            figlio_guardia::LIMITE_DI_RACCOLTA,
            &figlio_guardia::OrologioDiSistema::nuovo(figlio_guardia::PASSO_DI_RACCOLTA),
        ) {
            figlio_guardia::Chiusura::Raccolto { difetti, .. } => {
                (!difetti.is_empty()).then(|| difetti.join("; "))
            }
            figlio_guardia::Chiusura::NonRaccolto { guardia, difetti } => {
                // Qui **non** c'e' nessuno a cui la guardia possa risalire.
                //
                // Cio' che questa funzione rende e' un errore tipizzato, e un
                // errore non tiene un processo: attraversa i confini, viene
                // convertito, e finisce in una superficie pubblica dove un
                // `FiglioVivo` non ha posto. Farlo scendere in una riga di
                // rapporto lascerebbe un processo che nessuno aspetta mentre
                // l'avvio dichiara di essere fallito ordinatamente.
                //
                // Ci si ferma, dicendo perche'. E' l'esito peggiore tranne uno:
                // proseguire.
                guardia.arrenditi(&format!(
                    "l'avvio non e' riuscito e la chiusura del figlio nemmeno: {}",
                    difetti.join("; ")
                ))
            }
        };
        return Err(Box::new(TransizioneFallita {
            causa,
            evidenza,
            difetto_di_pulizia,
        }));
    }

    // La consegna: da qui la responsabilita' del figlio e' del chiamante.
    let Some(figlio) = figlio.consegna() else {
        // Irraggiungibile per costruzione — la guardia e' appena stata creata e
        // nessuna porta e' stata attraversata — ma lo si dice col tipo
        // invece che con una primitiva di panico, che qui non si usa.
        return Err(Box::new(TransizioneFallita {
            causa: passo("avvio", "la guardia del figlio era gia' vuota"),
            evidenza,
            difetto_di_pulizia: None,
        }));
    };
    Ok(TransizioneRiuscita {
        figlio,
        evidenza,
        supervisore_legge: sup_legge,
        supervisore_scrive: sup_scrive,
    })
}

/// La riga di comando divisa sul `--`.
///
/// Il separatore serve perche' gli argomenti del worker sono arbitrari: senza,
/// un worker chiamato con sei argomenti che cominciano con la stringa di
/// versione sarebbe indistinguibile da una richiesta.
///
/// # Perche' presta invece di copiare
///
/// Perche' questo codice gira **prima** del passo 2, cioe' mentre il processo
/// e' ancora fuori dal cgroup e nessun tetto lo governa. Copiare la richiesta e
/// tutti gli argomenti del worker raddoppierebbe li' una quantita' che il
/// chiamante sceglie e che `ARG_MAX` limita a qualche megabyte: piccola in
/// assoluto, ma non governata, e allocata esattamente dove il limite non c'e'
/// ancora.
///
/// Le fette vivono quanto gli argomenti da cui vengono, che sono quelli del
/// processo e durano fino alla `exec`: non c'e' niente da possedere.
fn spacca(argomenti: &[std::ffi::OsString]) -> Result<(&[std::ffi::OsString], DaEseguire<'_>)> {
    let taglio = argomenti
        .iter()
        .position(|pezzo| pezzo == "--")
        .ok_or_else(|| passo("richiesta", "manca il separatore -- fra richiesta e worker"))?;
    let (richiesta, resto) = argomenti.split_at(taglio);
    let [_, eseguibile, argomenti_worker @ ..] = resto else {
        return Err(passo("richiesta", "dopo -- manca l'eseguibile del worker"));
    };
    Ok((
        richiesta,
        DaEseguire {
            eseguibile: Path::new(eseguibile),
            argomenti: argomenti_worker,
        },
    ))
}

/// Che cosa lo spawner deve **eseguire**.
///
/// Dominio, montaggio, radice, namespace e identita' non stanno qui: arrivano
/// tutti insieme dal token che il preflight rende, e arrivarci come campi
/// indipendenti permetterebbe di verificare una combinazione ed eseguirne
/// un'altra.
///
/// I campi sono **prestiti**: cio' che va eseguito e' gia' in memoria — negli
/// argomenti del processo, o presso il supervisore — e riprodurlo qui vorrebbe
/// dire allocare fuori dal dominio una quantita' che il chiamante sceglie.
pub(super) struct DaEseguire<'a> {
    pub(super) eseguibile: &'a Path,
    pub(super) argomenti: &'a [std::ffi::OsString],
}

/// Esegue la sequenza e poi il worker.
///
/// # Che cosa rende
///
/// Non rende mai `Ok`: se la `exec` riesce, ha sostituito l'immagine e questa
/// funzione non esiste piu'. Il tipo lo dice — `Infallible` nel ramo riuscito —
/// perche' una firma che ammettesse un ritorno normale inviterebbe a
/// scriverci del codice dopo, e quel codice non girerebbe mai.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] per ogni passo che non regge, col
/// nome del passo e il modo. Nessun passo si compensa proseguendo: un ambiente
/// che non concede uno dei sette non e' un ambiente in cui il profilo isolato
/// vale meno, e' uno in cui non vale.
fn entra_ed_esegui(
    rivalidato: DominioRivalidato,
    canale_del_worker: super::NumeriDelCanale,
    da_eseguire: &DaEseguire<'_>,
) -> Result<std::convert::Infallible> {
    // Il token si **smonta** qui, e da qui in poi esistono solo i suoi pezzi.
    // Prenderlo per riferimento sarebbe piu' economico e direbbe un'altra cosa:
    // che dopo questa chiamata il chiamante ne ha ancora uno, cioe' che puo'
    // entrare due volte in cio' che ha verificato una volta.
    let DominioRivalidato {
        dominio,
        radice,
        worker,
        montaggio,
        namespace_del_padre,
    } = rivalidato;

    // --- 1. monothread -----------------------------------------------------
    let task = conta_task().map_err(|errore| passo("thread singolo", &errore))?;
    if task != 1 {
        return Err(passo(
            "thread singolo",
            &format!(
                "lo spawner ha {task} task: le credenziali si cambiano per thread, e gli altri \
                 resterebbero privilegiati"
            ),
        ));
    }

    // --- 2. dentro il dominio, e riletto -----------------------------------
    //
    // Lo `0` e' il processo corrente: scriverlo evita di doversi procurare il
    // proprio pid e di fidarsi che sia ancora valido quando la scrittura
    // arriva.
    scrivi(&dominio.join("cgroup.procs"), "0")
        .map_err(|errore| passo("ingresso nel dominio", &errore))?;
    let appartenenza = leggi_limitato(Path::new("/proc/self/cgroup"))
        .map_err(|errore| passo("appartenenza", &errore.to_string()))?;
    let letto = percorso_cgroup(&appartenenza).map_err(|errore| passo("appartenenza", errore))?;
    let atteso = dentro_la_gerarchia(&dominio, &montaggio)
        .map_err(|errore| passo("appartenenza", &errore))?;
    if letto != atteso {
        return Err(passo(
            "appartenenza",
            &format!("atteso {atteso}, letto {letto}"),
        ));
    }

    // --- 3. uccidibilita' --------------------------------------------------
    //
    // A `-1000` il kernel non uccide il task **nemmeno con
    // `memory.oom.group = 1`**: un worker che eredita quel valore da un
    // chiamante protetto sopravvive al group kill e riproduce `F4-8`. E l'esito
    // e' peggiore della sopravvivenza: un dominio che raggiunge il limite senza
    // uccidere nessuno non avanza piu', e richiede un `cgroup.kill`
    // dall'esterno.
    //
    // Scrivere senza verificare non e' normalizzare: e' sperare.
    scrivi(Path::new("/proc/self/oom_score_adj"), "0")
        .map_err(|errore| passo("oom_score_adj", &errore))?;
    let riletto = leggi_limitato(Path::new("/proc/self/oom_score_adj"))
        .map_err(|errore| passo("oom_score_adj", &errore.to_string()))?;
    if riletto.trim() != "0" {
        return Err(passo(
            "oom_score_adj",
            &format!("scritto 0, riletto {}", riletto.trim()),
        ));
    }

    // --- 4. nessun descrittore scrivibile verso il control plane -----------
    //
    // E' una proprieta' **autonoma**, non una conseguenza dei passi 5 e 6: il
    // cambio d'identita' non revoca l'autorita' gia' acquisita, perche' il
    // controllo dei permessi avviene all'apertura e non a ogni scrittura. Un
    // `fd` aperto sulla gerarchia prima della `setresuid` resta scrivibile
    // dopo.
    //
    // Qui si **verifica e si rifiuta**, non si chiude. Chiudere un descrittore
    // ereditato per numero richiede di costruirne un proprietario da un intero
    // grezzo, e ogni via per farlo e' `unsafe`. Rifiutare e' fail-closed e non
    // richiede niente: un ambiente che ci passa un `fd` sul control plane non
    // e' un ambiente in cui possiamo isolare, e chiuderlo di nascosto
    // nasconderebbe che qualcuno ce lo ha dato.
    let dispositivo = dispositivo_di(&radice).map_err(|errore| passo("descrittori", &errore))?;
    let prima = leggi_identita().map_err(|errore| passo("descrittori", &errore))?;
    let aperti: Vec<&str> = prima
        .descrittori_scrivibili
        .iter()
        .filter(|descrittore| descrittore.dispositivo == dispositivo)
        .map(|descrittore| descrittore.percorso.as_str())
        .collect();
    if !aperti.is_empty() {
        return Err(passo(
            "descrittori",
            &format!(
                "restano descrittori scrivibili sul filesystem del control plane: {aperti:?}. \
                 Il cambio d'identita' non li revoca"
            ),
        ));
    }

    // --- 4-bis. il canale passa al worker anche come proprieta' ------------
    //
    // # Perche' serve
    //
    // Perche' il worker **riapre** i propri estremi da `/proc/self/fd`, e quella
    // riapertura controlla i permessi sull'inode della pipe. Le pipe le ha
    // create il supervisore, quindi appartengono a lui: dopo il passo 6 il
    // worker ha un'altra identita' e la riapertura gli risponde
    // `Permission denied`. Il canale ci sarebbe — i descrittori sono ereditati e
    // validi — e il worker non potrebbe prenderne possesso.
    //
    // Non lo si evita smettendo di riaprire: prendere possesso di un descrittore
    // ereditato **per numero** richiede di costruirne un proprietario da un
    // intero grezzo, e ogni via per farlo e' `unsafe`, che questo crate vieta.
    // La riapertura e' la sola forma sicura, ed e' anche quella che permette di
    // **accertare** che l'estremo sia quello dichiarato invece di crederci.
    //
    // # Perche' proprio qui
    //
    // Non prima: il passo 4 pretende che non resti nessun descrittore scrivibile
    // verso il control plane, e un cambio di proprieta' fatto sopra sarebbe
    // un'autorita' esercitata nel mezzo di quella verifica. Non dopo: il passo 6
    // toglie proprio i privilegi che servono a cedere la proprieta'.
    //
    // # Che cosa si cede, e che cosa no
    //
    // I **due oggetti pipe**, che sono due e non quattro: ogni pipe ha un inode
    // solo, e i due lati lo condividono. Cedere l'estremo del worker cede quindi
    // anche l'inode su cui il supervisore ha il proprio estremo — e va detto,
    // perche' e' facile leggerlo come «gli altri due restano miei».
    //
    // Cio' che il supervisore conserva sono i propri **handle gia' aperti**, non
    // la proprieta' degli inode: il permesso si controlla all'apertura, e i suoi
    // descrittori sono aperti da prima. Continua a leggere e scrivere come
    // sempre; cio' che non potrebbe piu' fare e' **riaprirli** da
    // `/proc/self/fd`, che e' un'operazione che non compie.
    //
    // Al worker questo non da' niente che non abbia gia' — i descrittori li ha
    // ereditati — gli da' il modo di riaprirli, che e' come il protocollo
    // pretende che li prenda.
    //
    // Il cambio passa dal percorso e non dal numero: `chown` su
    // `/proc/self/fd/N` segue il collegamento fino all'inode della pipe, e non
    // richiede di costruire un prestito da un intero — che sarebbe di nuovo
    // `unsafe`.
    for (numero, quale) in [
        (canale_del_worker.legge, "lettura"),
        (canale_del_worker.scrive, "scrittura"),
    ] {
        rustix::fs::chown(
            format!("/proc/self/fd/{numero}").as_str(),
            Some(Uid::from_raw(worker.uid)),
            Some(Gid::from_raw(worker.gid)),
        )
        .map_err(|errore| {
            passo(
                "canale",
                &format!(
                    "l'estremo di {quale} ({numero}) non passa al worker {}:{}: {errore}",
                    worker.uid, worker.gid
                ),
            )
        })?;
    }

    // --- 5. no_new_privs ---------------------------------------------------
    rustix::thread::set_no_new_privs(true)
        .map_err(|errore| passo("no_new_privs", &errore.to_string()))?;

    // --- 6. gruppi, poi GID, poi UID ---------------------------------------
    rustix::thread::set_thread_groups(&[])
        .map_err(|errore| passo("gruppi supplementari", &errore.to_string()))?;
    let gid = Gid::from_raw(worker.gid);
    rustix::thread::set_thread_res_gid(gid, gid, gid)
        .map_err(|errore| passo("GID", &errore.to_string()))?;
    let uid = Uid::from_raw(worker.uid);
    rustix::thread::set_thread_res_uid(uid, uid, uid)
        .map_err(|errore| passo("UID", &errore.to_string()))?;

    // --- 7. rilettura, e solo allora la exec -------------------------------
    //
    // Impostare non e' essere. Cio' che segue e' cio' che il processo **e'**,
    // letto da `/proc/self`, e se non coincide con l'incarico la `exec` non
    // parte.
    // `rileggi_credenziali` e non `leggi_identita`: dopo la `setresuid` il
    // kernel azzera il flag *dumpable* e rende `/proc/self/{ns,fd,fdinfo}`
    // inattraversabili al processo stesso. Cio' che si rilegge e' cio' che il
    // cambio puo' aver toccato; cio' che si porta avanti e' cio' che non puo'
    // essersi mosso, e la ragione sta sulla funzione.
    let dopo = rileggi_credenziali(&prima).map_err(|errore| passo("rilettura", &errore))?;
    verifica_spogliato(&dopo, &namespace_del_padre, worker, dispositivo)?;

    // La variabile del canale si **impone**, e non si aggiunge: `env` sostituisce
    // qualunque valore ereditato. Un `PLENORA_CANALE` gia' presente
    // nell'ambiente — messo da chi ha avviato il supervisore, o rimasto da un
    // tentativo precedente — direbbe al worker due numeri che non sono i suoi, e
    // il worker li rivaliderebbe trovandoli buoni: sarebbero descrittori veri,
    // solo di un altro canale.
    //
    // La forma la decide `in_variabile`, che e' la meta' scrivente della stessa
    // convenzione che il worker legge. E il worker **rivalida comunque**: quello
    // che arriva di qui e' un'affermazione, come tutto il resto che attraversa
    // una `exec`.
    let errore = std::os::unix::process::CommandExt::exec(
        std::process::Command::new(da_eseguire.eseguibile)
            .args(da_eseguire.argomenti)
            .env(
                canale::VARIABILE_DEL_CANALE,
                canale_del_worker.in_variabile(),
            ),
    );
    Err(passo("exec", &errore.to_string()))
}

/// Quanti task ha questo processo.
///
/// Una voce che non si legge e' un errore e non uno scarto: sottocontare i
/// task significherebbe dichiarare monothread un processo che non lo e', che e'
/// esattamente la condizione che questo passo esiste per escludere.
///
/// # Errors
///
/// L'errore di lettura di `/proc/self/task`.
fn conta_task() -> std::result::Result<usize, String> {
    let voci = std::fs::read_dir("/proc/self/task")
        .map_err(|errore| format!("/proc/self/task: {errore}"))?;
    let mut quanti = 0_usize;
    for voce in voci {
        voce.map_err(|errore| format!("/proc/self/task: {errore}"))?;
        quanti += 1;
    }
    Ok(quanti)
}

/// L'identita' del filesystem su cui sta un percorso.
fn dispositivo_di(percorso: &Path) -> std::result::Result<u64, String> {
    std::fs::metadata(percorso)
        .map(|dati| dati.dev())
        .map_err(|errore| format!("{}: {errore}", percorso.display()))
}

/// Che l'identita' riletta non porti piu' autorita', e sia quella chiesta.
fn verifica_spogliato(
    identita: &Identita,
    namespace_attesi: &[(String, String)],
    worker: super::IdentitaWorker,
    dispositivo_control_plane: u64,
) -> Result<()> {
    let motivi = identita.autorita_residua(dispositivo_control_plane, namespace_attesi);
    if !motivi.is_empty() {
        let elenco: Vec<String> = motivi.iter().map(ToString::to_string).collect();
        return Err(passo("rilettura", &elenco.join("; ")));
    }
    if identita.uid != [worker.uid; 4] || identita.gid != [worker.gid; 4] {
        return Err(passo(
            "rilettura",
            &format!(
                "identita' non quella chiesta: uid {:?}, gid {:?}",
                identita.uid, identita.gid
            ),
        ));
    }
    Ok(())
}

/// Il percorso v2 in `/proc/self/cgroup`.
///
/// # Perche' fail-closed
///
/// Il file ha, in cgroup v2, **una sola** riga con ID gerarchia zero. Un
/// parser che prende la prima che incontra accetta:
///
/// - un file con **due** righe `0::`, dove quale valga non lo dichiara
///   nessuno;
/// - una riga `0::` con un percorso **relativo** o vuoto, che non e' un
///   percorso di cgroup e non si puo' confrontare con niente.
///
/// Su un sistema ibrido il file porta anche righe v1: quelle si saltano, ed e'
/// il motivo per cui non basta prendere la prima riga qualunque essa sia.
///
/// # Errors
///
/// Il motivo, in forma di frase.
fn percorso_cgroup(contenuto: &str) -> std::result::Result<&str, &'static str> {
    let mut trovato: Option<&str> = None;
    for riga in contenuto.lines() {
        let Some(percorso) = riga.strip_prefix("0::") else {
            continue;
        };
        if trovato.is_some() {
            return Err(
                "/proc/self/cgroup ha piu' di una riga v2: quale valga non lo dichiara \
                        nessuno",
            );
        }
        // Niente `trim`: un percorso di cgroup puo' contenere spazi, anche in
        // coda, e toglierli renderebbe un percorso **diverso** da quello a cui
        // il processo appartiene. `lines()` ha gia' tolto il fine riga, che e'
        // l'unica cosa che non fa parte del nome.
        trovato = Some(percorso);
    }
    match trovato {
        None => Err("/proc/self/cgroup non ha una riga v2"),
        Some(percorso) if percorso.starts_with('/') => Ok(percorso),
        Some(_) => Err("la riga v2 di /proc/self/cgroup non porta un percorso assoluto"),
    }
}

/// Il percorso del dominio **dentro** la gerarchia.
///
/// `/proc/self/cgroup` riporta il percorso relativo alla radice della
/// gerarchia, non quello nel filesystem. La conversione toglie il punto di
/// mount e rimette la radice del mount: con un bind mount di sottoalbero la
/// radice non e' `/`, e ignorarla sposta il percorso calcolato di tutto il
/// ramo.
///
/// # Errors
///
/// Se il dominio non sta sotto quel punto di mount: e' un montaggio che non lo
/// contiene, e calcolarci sopra un percorso darebbe un risultato senza
/// significato.
fn dentro_la_gerarchia(
    dominio: &Path,
    montaggio: &Montaggio,
) -> std::result::Result<String, String> {
    // `Path::strip_prefix` e non un confronto di testo: il primo lavora per
    // **componenti**, e `/sys/fs/cgroup2` non e' dentro `/sys/fs/cgroup` per
    // quanto lo sia il suo testo.
    let resto = dominio.strip_prefix(&montaggio.punto).map_err(|_| {
        format!(
            "il dominio {} non sta sotto il punto di mount {}",
            dominio.display(),
            montaggio.punto.display()
        )
    })?;
    let composto = montaggio.radice.join(resto);
    composto
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("il percorso {} non e' UTF-8", composto.display()))
}

fn scrivi(percorso: &Path, valore: &str) -> std::result::Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(percorso)
        .map_err(|errore| format!("{}: {errore}", percorso.display()))?;
    file.write_all(valore.as_bytes())
        .map_err(|errore| format!("{}: {errore}", percorso.display()))
}

fn passo(quale: &str, motivo: &str) -> PlenoraError {
    non_disponibile(&format!("spawner, passo «{quale}»"), motivo)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{dentro_la_gerarchia, percorso_cgroup};
    use crate::isolamento::Montaggio;

    fn montaggio(punto: &str, radice: &str) -> Montaggio {
        Montaggio {
            punto: PathBuf::from(punto),
            radice: PathBuf::from(radice),
            opzioni_mount: "rw".to_owned(),
            opzioni_superblocco: "rw,nsdelegate".to_owned(),
            dispositivo: "0:27".to_owned(),
        }
    }

    /// Si legge la riga v2, e le righe v1 di un sistema ibrido si saltano.
    #[test]
    fn si_legge_la_riga_v2_non_la_prima() {
        let contenuto = "1:name=systemd:/user.slice\n0::/plenora/dominio-7\n";
        assert_eq!(percorso_cgroup(contenuto), Ok("/plenora/dominio-7"));
    }

    /// Due righe v2 sono ambigue, e l'ambiguita' e' un rifiuto.
    ///
    /// Un parser che prende la prima sceglierebbe secondo l'ordine delle
    /// righe, che qui non ha quel significato.
    #[test]
    fn due_righe_v2_sono_un_rifiuto() {
        let contenuto = "0::/uno\n0::/due\n";
        assert!(percorso_cgroup(contenuto)
            .expect_err("due righe")
            .contains("piu' di una riga v2"));
    }

    /// Una riga v2 senza percorso assoluto non e' confrontabile con niente.
    #[test]
    fn un_percorso_non_assoluto_e_un_rifiuto() {
        assert!(percorso_cgroup("0::relativo\n").is_err());
        assert!(percorso_cgroup("0::\n").is_err());
    }

    /// Senza riga v2 non c'e' appartenenza da confermare.
    #[test]
    fn senza_riga_v2_non_c_e_appartenenza() {
        assert!(percorso_cgroup("1:name=systemd:/user.slice\n").is_err());
    }

    /// Il percorso si calcola sul montaggio scelto dal preflight.
    #[test]
    fn il_percorso_si_calcola_sul_montaggio() {
        assert_eq!(
            dentro_la_gerarchia(
                Path::new("/sys/fs/cgroup/plenora/dominio-7"),
                &montaggio("/sys/fs/cgroup", "/")
            ),
            Ok("/plenora/dominio-7".to_owned())
        );
    }

    /// Con un bind mount di sottoalbero la radice non e' `/`, e ignorarla
    /// sposterebbe il percorso di tutto il ramo.
    ///
    /// E' il caso che un prefisso convenzionale sbaglia in silenzio: il
    /// dominio raggiunto da `/mnt/dominio` sta, dentro la gerarchia, in
    /// `/plenora/...`, non in `/...`.
    #[test]
    fn con_un_bind_mount_la_radice_rientra_nel_percorso() {
        assert_eq!(
            dentro_la_gerarchia(
                Path::new("/mnt/dominio/lavoro"),
                &montaggio("/mnt/dominio", "/plenora")
            ),
            Ok("/plenora/lavoro".to_owned())
        );
    }

    /// Il dominio che coincide col punto di mount sta alla radice.
    #[test]
    fn il_dominio_al_punto_di_mount_e_la_radice() {
        assert_eq!(
            dentro_la_gerarchia(
                Path::new("/sys/fs/cgroup"),
                &montaggio("/sys/fs/cgroup", "/")
            ),
            Ok("/".to_owned())
        );
    }

    /// Un dominio fuori dal punto di mount non ha un percorso da calcolare.
    #[test]
    fn un_dominio_fuori_dal_montaggio_e_un_errore() {
        assert!(dentro_la_gerarchia(
            Path::new("/altrove/dominio"),
            &montaggio("/sys/fs/cgroup", "/")
        )
        .is_err());
    }
}
