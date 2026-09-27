//! Lo spawner: entra nel dominio, si spoglia dell'autorita', esegue.
//!
//! E' un processo dedicato perche' la sequenza deve girare fra la nascita del
//! processo e la `exec`, e `CommandExt::pre_exec` e' `unsafe`. Il supervisore
//! lo lancia con un `Command::spawn` ordinario, lo spawner esegue i passi con
//! chiamate sicure e finisce con `CommandExt::exec`, che e' safe.
//!
//! `rustix::thread::{set_thread_groups, set_thread_res_gid, set_thread_res_uid}`
//! cambiano le credenziali del **solo thread** chiamante: in un processo
//! multithread gli altri thread resterebbero privilegiati. Per questo il primo
//! passo pretende un task solo, e **queste API valgono solo qui**.
//!
//! La sequenza e' fail-closed: nessun errore intermedio si ignora o si
//! compensa.
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
//! L'ordine: il cgroup prima dell'identita' (dopo la `setresuid` non si scrive
//! piu' nella gerarchia), `no_new_privs` prima del cambio d'identita', i
//! gruppi prima del GID (`setgroups` richiede l'autorita' che il cambio di GID
//! toglie).
//!
//! Il gate ostile (`scripts/verifica_isolamento_linux.sh`, che fallisce se
//! mancano i prerequisiti) prova cio' che i casi qui non possono:
//!
//! 1. **la sentinella sul dispatch**: questa immagine, rieseguita con
//!    `argv[1]` uguale alla versione della richiesta, arriva spawner con **un
//!    task solo**;
//! 2. **l'immagine sostituita**: il binario sostitutivo non parte mai. Gli
//!    esiti ammessi sono due: sostituzione prima del controllo e
//!    [`TransizioneFallita`] (` (deleted)`, rifiutato da [`accerta_immagine`]),
//!    oppure dopo, e parte l'inode iniziale via `/proc/self/exe`. Li separa una
//!    barriera controllata dopo l'accertamento (`dopo_accertamento` di
//!    `tenta`), non una corsa temporizzata. Il controllo ` (deleted)` e' una
//!    fotografia, non una garanzia all'istante della `exec`: regge
//!    l'esecuzione di `/proc/self/exe`;
//! 3. **la separazione di privilegio**: un worker spogliato non riscrive i
//!    quattro controlli ne' il `cgroup.procs` del padre, e dopo una `unshare`
//!    lo stato resta invariato.

use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use plenora_core::error::{PlenoraError, Result};
use rustix::process::{Gid, Uid};

use super::canale;
use super::dominio::Gerarchia;
use super::figlio as figlio_guardia;
use super::figlio::FiglioVivo;
use super::identita::{leggi_identita, namespace_del_padre, rileggi_credenziali, Identita};
use super::lettura::leggi_limitato;
use super::{
    non_disponibile, DominioRivalidato, IdentitaWorker, Montaggio, ProprietaFile, RichiestaSpawner,
    VERSIONE_RICHIESTA,
};
// Cio' che serve al solo avvio.
use super::{
    esito, spawner_ammissibile, DominioPreparato, TentativoFallito, TransizioneFallita,
    TransizioneRiuscita,
};

/// Avvia lo spawner sul dominio appena preparato.
///
/// Consuma il token: un dominio, uno spawner. Attraversa il confine solo
/// [`RichiestaSpawner`]; l'evidenza esce da questa parte, insieme al figlio.
///
/// Lo spawner e' **questa stessa immagine**, rieseguita da `/proc/self/exe` e
/// mai da un percorso del chiamante, che avvierebbe un figlio fuori dal
/// dominio indistinguibile da una transizione riuscita. Si riconosce dal suo
/// `argv[1]`, [`VERSIONE_RICHIESTA`]: il chiamante di produzione deve passare
/// la mano a [`dal_confine`] prima di ogni altra cosa all'avvio, thread
/// compresi. L'obbligo lo prova la sentinella del gate ostile.
///
/// # Errors
///
/// [`TransizioneFallita`], che porta la causa **e** l'evidenza. Sta in un
/// `Box` perche' e' molto piu' grande dell'esito riuscito, che altrimenti
/// pagherebbe in pila la dimensione del ramo raro.
pub(super) fn avvia(
    preparato: DominioPreparato,
    da_eseguire: &DaEseguire<'_>,
    artefatto: Option<&std::fs::File>,
) -> std::result::Result<TransizioneRiuscita, Box<TransizioneFallita>> {
    // Nessuna giuntura: le callback vuote sono cio' che la produzione passa, e
    // l'unica cosa che un chiamante di qualificazione puo' variare e' **se**
    // fermarsi o fallire in quei due punti, mai che cosa si controlla.
    avvia_interno(preparato, da_eseguire, artefatto, || Ok(()), || Ok(()))
}

/// Il tentativo vero e proprio, condiviso fra [`avvia`] e la sua variante con
/// barriera.
///
/// Una copia sola, perche' la variante vive nel perimetro di qualificazione e
/// due sequenze potrebbero divergere proprio dove il gate misura.
///
/// `accerta_immagine` viene **prima** e non passa da un parametro: cederla al
/// chiamante la renderebbe facoltativa. `dopo_accertamento` viene **dopo**: e'
/// la barriera con cui il gate sostituisce il binario fra controllo e `spawn`.
/// Non puo' saltare il controllo ne' cambiare l'inode che `/proc/self/exe`
/// raggiunge; rende obsoleta la fotografia ` (deleted)`, e basta perche'
/// l'inode e' l'unica cosa che si esegue.
///
/// La produzione passa una callback vuota. La funzione e' privata con un solo
/// chiamante: l'ingresso del gate vive sotto `#[cfg(test)]` o in un binario
/// di qualificazione, mai dietro una feature, che l'unificazione propaga.
fn tenta(
    richiesta: &RichiestaSpawner,
    worker: IdentitaWorker,
    da_eseguire: &DaEseguire<'_>,
    dopo_accertamento: impl FnOnce() -> Result<()>,
    estremi: &canale::EstremiDelWorker,
    artefatto: Option<&std::fs::File>,
) -> std::result::Result<FiglioVivo<std::process::Child>, Box<TentativoFallito>> {
    accerta_immagine(worker)?;
    dopo_accertamento()?;

    // 1. Il comando si costruisce **mentre tutto e' ancora `CLOEXEC`**, per non
    // allungare la finestra con allocazioni e fallimenti.
    //
    // Si esegue `/proc/self/exe`, non il nome che risolve: il nome si puo'
    // sostituire con una `rename`, il collegamento resta legato all'immagine.
    let mut comando = std::process::Command::new(IMMAGINE);
    comando
        .args(richiesta.in_argomenti())
        .arg("--")
        .arg(da_eseguire.eseguibile)
        .args(da_eseguire.argomenti);

    // 2. Il monothread si accerta **adesso**, immediatamente prima della prima
    //    modifica: fra l'avvio e questo punto un thread puo' essere nato.
    canale::accerta_monothread()?;

    // 3. `CLOEXEC` via ai due estremi del worker, e a nient'altro — piu'
    //    l'artefatto, quando questa transizione avvia un verificatore.
    //    Stessa finestra, stessa ragione: nessun descrittore in meno di
    //    rigore solo perche' e' opzionale.
    estremi.rendi_ereditabili()?;
    if let Some(file) = artefatto {
        canale::rendi_ereditabile_artefatto(file)?;
    }

    // Il braccio «spawn»: il fallimento arriva **con tutti i descrittori
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
const IMMAGINE: &str = "/proc/self/exe";

/// Che l'immagine in esecuzione sia rieseguibile.
///
/// Il **nome** si legge con `read_link`, suffisso ` (deleted)` compreso;
/// l'**inode** si interroga attraverso `/proc/self/exe`. Interrogare il nome
/// fallirebbe su un'immagine rimossa con la ragione sbagliata, e su una
/// sostituita descriverebbe il file nuovo.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se `/proc/self/exe` non si legge o
/// non si interroga, o se una delle tre condizioni manca.
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
/// Rivalida tutto (ambiente, percorsi, montaggio, permessi, namespace e i
/// quattro controlli) in un [`DominioRivalidato`] **locale**: un processo che
/// crede al mittente non aggiunge garanzie. Non scrive i controlli, li
/// rilegge: il tetto e' gia' in vigore quando lo spawner nasce (`F4-1`,
/// `GA-7`).
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] per una richiesta malformata, per una
/// rivalidazione che non regge, o per un passo della sequenza che cede.
pub(super) fn dal_confine(argomenti: &[std::ffi::OsString]) -> Result<std::convert::Infallible> {
    let (grezza, da_eseguire) = spacca(argomenti)?;
    let richiesta =
        RichiestaSpawner::da_argomenti(grezza).map_err(|motivo| passo("richiesta", &motivo))?;

    // Stadio 2: i due descrittori ereditati si **riguardano** qui, prima di
    // entrare nel dominio e di spogliarsi dell'autorita': rifiutare dopo
    // costerebbe un rimedio. La verifica del supervisore riguardava i suoi
    // descrittori, non questi numeri.
    //
    // Il worker riceve la coppia **rivalidata qui**, non i numeri della
    // richiesta, e il valore si ottiene solo da chi lo ha verificato.
    let canale_del_worker =
        canale::accerta_coppia(richiesta.worker_legge, richiesta.worker_scrive)?;
    // Il terzo descrittore, quando c'e': stesso principio dei due estremi del
    // canale, riguardato e non creduto. `-1` (la forma canonica di «assente»
    // gia' accertata da `descrittore_canonico` in `RichiestaSpawner::da_argomenti`)
    // e' l'unico valore che salta la rivalidazione — e' l'assenza stessa, non
    // un descrittore da controllare.
    let artefatto_lettura = if richiesta.artefatto_lettura < 0 {
        None
    } else {
        Some(canale::accerta_artefatto(richiesta.artefatto_lettura)?.numero)
    };
    // La superficie si costruisce **dalla richiesta**, e non arriva da un
    // chiamante: e' l'unica forma in cui «lo spawner rivalida da se'» non
    // dipende da chi lo ha invocato. `Gerarchia::nuova` canonicalizza, e
    // `rivalida` pretende poi che il canonico coincida col nome ricevuto — che
    // e' il confronto che smaschera un percorso indiretto.
    let gerarchia = Gerarchia::nuova(&richiesta.dominio, &richiesta.radice)
        .map_err(|difetto| passo("gerarchia", &difetto.to_string()))?;
    let padre = namespace_del_padre().map_err(|errore| passo("namespace del padre", &errore))?;
    let rivalidato = super::rivalida(&gerarchia, &richiesta, padre)?;
    entra_ed_esegui(
        rivalidato,
        canale_del_worker,
        artefatto_lettura,
        &da_eseguire,
    )
}

/// [`avvia`] con una barriera fra l'accertamento dell'immagine e lo `spawn`.
///
/// Serve a **una** prova: che si esegua l'inode e non il nome, sostituendo il
/// binario mentre il processo e' fermo fra controllo e `spawn`. Sta sotto
/// `qualificazione_isolamento`, un `cfg` di `rustc` e non una feature, che
/// l'unificazione propagherebbe; chi lo mette in `RUSTFLAGS` lo accende di
/// proposito, e la garanzia e' contro l'incidente, non l'intenzione.
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
    avvia_interno(
        preparato,
        da_eseguire,
        None,
        prima_dello_spawn,
        dopo_lo_spawn,
    )
}

fn avvia_interno(
    preparato: DominioPreparato,
    da_eseguire: &DaEseguire<'_>,
    artefatto: Option<&std::fs::File>,
    prima_dello_spawn: impl FnOnce() -> Result<()>,
    dopo_lo_spawn: impl FnOnce() -> Result<()>,
) -> std::result::Result<TransizioneRiuscita, Box<TransizioneFallita>> {
    // Solo per `artefatto.map(...)` piu' sotto: in cima alla funzione, non
    // dopo le prime istruzioni — un `use` a meta' corpo e' facile da
    // scambiare per un'importazione con effetto locale, che non e'. Il nome
    // (non `as _`) serve a riferire `AsRawFd::as_raw_fd` come funzione,
    // senza una chiusura che lo richiami soltanto.
    use rustix::fd::AsRawFd;

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
    // Il numero grezzo dell'artefatto, non l'handle: e' cio' che entra nella
    // richiesta, e lo spawner lo rivalidera' da se' con `canale::accerta_artefatto`
    // invece di crederci — lo stesso principio dei due estremi del canale.
    let (richiesta, evidenza) = preparato.consuma(numeri, artefatto.map(AsRawFd::as_raw_fd));
    let tentativo = tenta(
        &richiesta,
        evidenza.worker,
        da_eseguire,
        prima_dello_spawn,
        &estremi,
        artefatto,
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
        // Si conservano **entrambi** i difetti: la causa dice perche' la
        // transizione non e' riuscita, il difetto di pulizia che cosa e'
        // rimasto. L'uscita del figlio non aggiunge niente a «l'avvio e'
        // fallito».
        let difetto_di_pulizia = match figlio.termina_e_raccogli(
            figlio_guardia::LIMITE_DI_RACCOLTA,
            &figlio_guardia::OrologioDiSistema::nuovo(figlio_guardia::PASSO_DI_RACCOLTA),
        ) {
            figlio_guardia::Chiusura::Raccolto { difetti, .. } => {
                (!difetti.is_empty()).then(|| difetti.join("; "))
            }
            figlio_guardia::Chiusura::NonRaccolto { guardia, difetti } => {
                // Qui la guardia non puo' risalire a nessuno: un errore
                // tipizzato non tiene un processo. Ci si ferma, dicendo
                // perche'; proseguire sarebbe peggio.
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
/// Il separatore distingue gli argomenti arbitrari del worker dalla
/// richiesta. Presta invece di copiare: il codice gira **prima** del passo 2,
/// fuori dal cgroup, dove una copia di argomenti scelti dal chiamante non
/// sarebbe governata. Le fette durano quanto gli argomenti del processo, cioe'
/// fino alla `exec`.
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
/// insieme dal token, perche' campi indipendenti permetterebbero di
/// verificare una combinazione ed eseguirne un'altra. I campi sono
/// **prestiti**, per non allocare fuori dal dominio.
pub(super) struct DaEseguire<'a> {
    pub(super) eseguibile: &'a Path,
    pub(super) argomenti: &'a [std::ffi::OsString],
}

/// Esegue la sequenza e poi il worker.
///
/// Non rende mai `Ok`: se la `exec` riesce, l'immagine e' sostituita.
/// `Infallible` nel ramo riuscito impedisce di scrivere codice che non
/// girerebbe mai.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] per ogni passo che non regge, col
/// nome del passo e il modo. Nessun passo si compensa proseguendo: senza uno
/// dei sette il profilo isolato non vale.
fn entra_ed_esegui(
    rivalidato: DominioRivalidato,
    canale_del_worker: super::NumeriDelCanale,
    artefatto_lettura: Option<i32>,
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

    let (dispositivo, prima) = accerta_prima_del_cambio_identita(&dominio, &radice, &montaggio)?;

    // --- 4-bis. il canale passa al worker anche come proprieta' ------------
    //
    // Il worker **riapre** i propri estremi da `/proc/self/fd` (l'adozione per
    // numero e' `unsafe`), e la riapertura controlla i permessi sull'inode
    // della pipe, che appartiene al supervisore: dopo il passo 6 risponderebbe
    // `Permission denied`.
    //
    // Qui e non altrove: prima, sarebbe un'autorita' esercitata dentro la
    // verifica del passo 4; dopo, il passo 6 ha tolto i privilegi per farlo.
    //
    // Si cedono i **due oggetti pipe**: i due lati condividono l'inode, quindi
    // si cede anche quello dell'estremo del supervisore. Il supervisore
    // conserva gli handle gia' aperti (il permesso si controlla all'apertura),
    // e non li riapre. Il `chown` passa da `/proc/self/fd/N`, che segue il
    // collegamento fino all'inode senza costruire un prestito da un intero.
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

    // Il terzo descrittore, quando c'e': stesso passo, stesso rigore. Il
    // verificatore lo riapre da `/proc/self/fd` esattamente come il worker
    // riapre le sue pipe, e senza il cambio di proprieta' la riapertura
    // risponderebbe `Permission denied` dopo il passo 6.
    if let Some(numero) = artefatto_lettura {
        rustix::fs::chown(
            format!("/proc/self/fd/{numero}").as_str(),
            Some(Uid::from_raw(worker.uid)),
            Some(Gid::from_raw(worker.gid)),
        )
        .map_err(|errore| {
            passo(
                "artefatto",
                &format!(
                    "il descrittore dell'artefatto ({numero}) non passa al verificatore \
                     {}:{}: {errore}",
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
    // Impostare non e' essere: se cio' che il processo **e'** non coincide con
    // l'incarico, la `exec` non parte. `rileggi_credenziali` e non
    // `leggi_identita`: dopo la `setresuid` il flag *dumpable* rende
    // `/proc/self/{ns,fd,fdinfo}` inattraversabili, e la ragione sta sulla
    // funzione.
    let dopo = rileggi_credenziali(&prima).map_err(|errore| passo("rilettura", &errore))?;
    verifica_spogliato(&dopo, &namespace_del_padre, worker, dispositivo)?;

    // La variabile del canale si **impone**: `env` sostituisce un
    // `PLENORA_CANALE` ereditato, che indicherebbe descrittori veri di un
    // altro canale. La forma la decide `in_variabile`, e il worker rivalida
    // comunque.
    //
    // `PLENORA_ARTEFATTO_LETTURA` si aggiunge **solo** quando c'e' un numero:
    // per un worker ordinario affermerebbe un descrittore che non esiste, e
    // solo il verificatore la legge.
    let mut comando = std::process::Command::new(da_eseguire.eseguibile);
    comando.args(da_eseguire.argomenti).env(
        canale::VARIABILE_DEL_CANALE,
        canale_del_worker.in_variabile(),
    );
    if let Some(numero) = artefatto_lettura {
        comando.env(canale::VARIABILE_ARTEFATTO, numero.to_string());
    }
    let errore = std::os::unix::process::CommandExt::exec(&mut comando);
    Err(passo("exec", &errore.to_string()))
}

/// I passi 1-4 di [`entra_ed_esegui`]: quelli che accertano l'ambiente
/// **prima** che qualunque autorita' venga ceduta o cambiata — monothread,
/// appartenenza al dominio, uccidibilita', nessun descrittore scrivibile
/// verso il control plane.
///
/// Separata per il tetto di righe per funzione (R6), sul confine «prima del
/// cambio d'identita'». Rende `dispositivo` e `prima`, che servono anche al
/// passo 7.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] al primo dei quattro passi che non
/// regge, col nome del passo.
fn accerta_prima_del_cambio_identita(
    dominio: &Path,
    radice: &Path,
    montaggio: &Montaggio,
) -> Result<(u64, Identita)> {
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
    let atteso =
        dentro_la_gerarchia(dominio, montaggio).map_err(|errore| passo("appartenenza", &errore))?;
    if letto != atteso {
        return Err(passo(
            "appartenenza",
            &format!("atteso {atteso}, letto {letto}"),
        ));
    }

    // --- 3. uccidibilita' --------------------------------------------------
    //
    // A `-1000` il kernel non uccide il task nemmeno con
    // `memory.oom.group = 1`: il worker sopravvive al group kill (`F4-8`), e
    // il dominio al limite non avanza piu' senza un `cgroup.kill` esterno.
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
    // Proprieta' **autonoma**: il permesso si controlla all'apertura, e un
    // `fd` aperto sulla gerarchia prima della `setresuid` resta scrivibile.
    // Si **rifiuta**, non si chiude: chiudere per numero e' `unsafe`, e
    // chiuderlo di nascosto nasconderebbe chi ce lo ha dato.
    let dispositivo = dispositivo_di(radice).map_err(|errore| passo("descrittori", &errore))?;
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

    Ok((dispositivo, prima))
}

/// Quanti task ha questo processo.
///
/// Una voce che non si legge e' un errore: sottocontare dichiarerebbe
/// monothread un processo che non lo e'.
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
/// Fail-closed: in cgroup v2 c'e' **una sola** riga con ID gerarchia zero, e
/// si rifiutano due righe `0::` o un percorso relativo o vuoto. Le righe v1
/// di un sistema ibrido si saltano.
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
/// gerarchia: si toglie il punto di mount e si rimette la radice del mount,
/// che con un bind mount di sottoalbero non e' `/`.
///
/// # Errors
///
/// Se il dominio non sta sotto quel punto di mount: il risultato non avrebbe
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

    use super::{conta_task, dentro_la_gerarchia, dispositivo_di, percorso_cgroup};
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

    // --- conta_task / dispositivo_di: letture reali, nessun privilegio ------
    //
    // Entrambe leggono solo `/proc/self` o `stat` un percorso: nessuno
    // spawn, nessun cambio di identita'. Il processo di test stesso e' un
    // soggetto valido — non serve un worker vero per accertare che la
    // lettura sia quella giusta.

    /// Il processo di test, monothread per costruzione (nessun altro test
    /// gira concorrentemente in QUESTO processo, dato che i test Rust
    /// condividono il processo ma non lo stato di `/proc/self/task`, che
    /// riflette i thread veri del processo, non i test logici): `conta_task`
    /// deve trovarne almeno uno, se stesso.
    #[test]
    #[cfg(target_os = "linux")]
    fn conta_task_trova_almeno_il_thread_che_chiama() {
        let quanti = conta_task().expect("/proc/self/task deve leggersi in un ambiente Linux");
        assert!(
            quanti >= 1,
            "il processo che chiama e' gia' un task: {quanti}"
        );
    }

    /// L'identita' del filesystem letta per il percorso di prova coincide con
    /// quella che `std::fs::metadata` osserva indipendentemente sullo stesso
    /// percorso — la stessa `stat`, non una ricostruita.
    #[test]
    #[cfg(target_os = "linux")]
    fn dispositivo_di_coincide_con_una_stat_indipendente() {
        use std::os::unix::fs::MetadataExt as _;
        let percorso = std::env::temp_dir();
        let atteso = std::fs::metadata(&percorso)
            .expect("il percorso temporaneo deve essere leggibile")
            .dev();
        assert_eq!(
            dispositivo_di(&percorso).expect("il dispositivo deve leggersi"),
            atteso
        );
    }

    /// Un percorso che non esiste non ha un dispositivo da leggere: rifiuto
    /// nominato, non un panico ne' un valore inventato.
    #[test]
    #[cfg(target_os = "linux")]
    fn dispositivo_di_rifiuta_un_percorso_inesistente() {
        let percorso = Path::new("/percorso/che/di-proposito/non-esiste-mai");
        assert!(dispositivo_di(percorso).is_err());
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
