//! Il perimetro di qualificazione: cio' che solo una macchina vera puo' dire.
//!
//! # Perche' e' un'immagine sola, con tre modi
//!
//! Perche' due dei tre modi devono essere **lo stesso binario** per costruzione,
//! non per convenzione. Lo spawner e' l'immagine del supervisore rieseguita:
//! provarlo con un secondo eseguibile proverebbe che due programmi diversi
//! collaborano, che e' un'altra affermazione. E il worker ostile deve nascere
//! dallo spawner, perche' cio' che si giudica e' l'identita' che lo spawner gli
//! lascia.
//!
//! I modi sono:
//!
//! - **spawner**, riconosciuto perche' `argv[1]` e' la versione della richiesta.
//!   E' il primo ramo di [`principale`], prima di qualunque altra cosa: e' la
//!   sentinella del dispatch;
//! - **supervisore**, che prepara il dominio e avvia lo spawner;
//! - **ostile**, che gira **dentro** il dominio con l'identita' del worker e
//!   tenta cio' che non deve riuscire;
//! - **finestra**, che misura che cosa resta leggibile di `/proc/self` **fra**
//!   il cambio d'identita' e la `exec`. E' un intervallo che nessun altro modo
//!   attraversa, e su cui poggia una scelta della sequenza.
//!
//! # Perche' non e' compilato in produzione
//!
//! `qualificazione_isolamento` non e' una feature di Cargo: e' un `cfg` che si
//! passa a `rustc`. La differenza sta in **come** si accende. Una feature la si
//! abilita dichiarandola fra le dipendenze, e l'unificazione la propaga a chi
//! non l'ha chiesta; un `cfg` non si propaga, e nessun crate dipendente puo'
//! accenderlo.
//!
//! Non e' pero' inaccessibile, e dirlo altrimenti sarebbe falso: chi controlla
//! il comando di build lo puo' mettere in `RUSTFLAGS`. La garanzia e' contro
//! l'incidente, non contro l'intenzione.
//!
//! # Il formato dell'evidenza
//!
//! Ogni riga che il gate legge comincia con `QI ` ed e' una coppia
//! `chiave=valore`. Non e' un dettaglio estetico: il gate deve poter
//! distinguere cio' che il programma **afferma** da cio' che stampa per gli
//! umani, e un formato riconoscibile e' l'unico modo in cui un rapporto
//! troncato o interrotto non si legge come un rapporto completo.
//!
//! # Ogni chiave compare una volta sola, e non e' una convenzione
//!
//! Nello stesso file scrivono **tre processi**: il supervisore, lo spawner che
//! ne eredita lo stdout, e il worker che nasce dallo spawner. Il gate li legge
//! come un flusso unico, e su un flusso unico due righe con la stessa chiave
//! sono ambigue: leggerne una nasconde l'altra, ed e' il modo in cui un
//! rapporto contraddittorio si legge come coerente.
//!
//! Per questo non esiste una chiave `modo` che ogni processo riempie a modo
//! suo: esistono `modo_supervisore`, `modo_spawner`, `modo_ostile`,
//! `modo_finestra`. Vale anche per le due fasi di un'attesa, che sono due
//! chiavi e non due valori della stessa. Il gate rifiuta i duplicati, quindi
//! una chiave riusata non passa inosservata — diventa rossa.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::dominio::Gerarchia;
use super::spawner::{avvia, avvia_con_barriera, dal_confine, DaEseguire};
use super::{prepara_dominio, Controllo, EvidenzaPreflight, IdentitaWorker, VERSIONE_RICHIESTA};

/// L'ingresso dell'immagine di qualificazione.
///
/// # Il dispatch e' la prima cosa, e questo e' il punto
///
/// Il ramo dello spawner sta in cima e non ha niente prima di se'. Il primo
/// passo della sequenza pretende un processo monothread, e un `main` che
/// costruisse un pool di thread prima di guardare `argv` lo renderebbe
/// impossibile — compilando, e passando ogni caso deterministico. Per questo
/// qui si stampa anche il numero di task: e' la sentinella, e il gate pretende
/// che sia uno.
#[must_use]
pub fn principale() -> ExitCode {
    let argomenti: Vec<OsString> = std::env::args_os().collect();
    let Some(primo) = argomenti.get(1) else {
        return lamenta("nessun modo: attesi spawner, supervisore od ostile");
    };

    if primo == VERSIONE_RICHIESTA {
        return modo_spawner(&argomenti);
    }
    match primo.to_str() {
        Some("supervisore") => modo_supervisore(&argomenti),
        Some("ostile") => modo_ostile(&argomenti),
        Some("finestra") => modo_finestra(&argomenti),
        _ => lamenta("modo sconosciuto: attesi spawner, supervisore, ostile o finestra"),
    }
}

/// Il modo spawner: la sentinella, poi il confine.
fn modo_spawner(argomenti: &[OsString]) -> ExitCode {
    // La sentinella. Si stampa **prima** di ogni altra cosa, cosi' il gate la
    // legge anche quando il resto fallisce: un dispatch tardivo non si vede
    // dagli esiti, si vede da qui.
    dichiara("modo_spawner", "avviato");
    match conta_task() {
        Ok(quanti) => dichiara("sentinella_task", &quanti.to_string()),
        Err(motivo) => dichiara("sentinella_task", &format!("illeggibile: {motivo}")),
    }
    match immagine() {
        Ok(nodo) => dichiara("immagine_inode", &nodo),
        Err(motivo) => dichiara("immagine_inode", &format!("illeggibile: {motivo}")),
    }

    // `argomenti[0]` e' il nome del programma: cio' che il confine legge
    // comincia dalla versione.
    match dal_confine(&argomenti[1..]) {
        // `dal_confine` non rende mai `Ok`: se la `exec` riesce, questa riga
        // non esiste piu'.
        Ok(mai) => match mai {},
        Err(errore) => lamenta(&format!("confine: {errore}")),
    }
}

/// Il modo supervisore: prepara il dominio, avvia lo spawner, riporta.
///
/// La riga di comando e'
/// `supervisore <dominio> <radice> <tetto> <uid> <gid> [--attendi <pronto> <via>] [--barriera <pronto> <via>] -- <worker> [argomenti]`.
///
/// # Le due attese, che servono a due cose opposte
///
/// `--attendi` ferma il processo **prima di tutto**, preflight compreso. Serve
/// al braccio in cui la sostituzione dell'immagine avviene *prima* del
/// controllo: il gate rinomina mentre il processo e' fermo qui, e
/// l'accertamento trova poi un'immagine cancellata. Non tocca nessuna giuntura
/// della libreria, perche' non ce n'e' bisogno: aspettare all'inizio lo sa fare
/// il programma da se'.
///
/// `--barriera` ferma invece il processo **fra l'accertamento e lo `spawn`**, e
/// li' una giuntura serve: e' il braccio in cui la sostituzione arriva dopo il
/// controllo, e in cui deve partire lo stesso l'inode iniziale.
fn modo_supervisore(argomenti: &[OsString]) -> ExitCode {
    dichiara("modo_supervisore", "avviato");
    let Some(taglio) = argomenti.iter().position(|pezzo| pezzo == "--") else {
        return lamenta("manca il separatore -- fra il supervisore e il worker");
    };
    let (testa, coda) = argomenti.split_at(taglio);
    let [_, eseguibile, argomenti_worker @ ..] = coda else {
        return lamenta("dopo -- manca l'eseguibile del worker");
    };

    let coppia = |nome: &str| {
        testa
            .iter()
            .position(|pezzo| pezzo == nome)
            .map_or(Ok(None), |dove| {
                match (testa.get(dove + 1), testa.get(dove + 2)) {
                    (Some(pronto), Some(via)) => {
                        Ok(Some((PathBuf::from(pronto), PathBuf::from(via))))
                    }
                    _ => Err(format!("{nome} vuole due fifo: pronto e via")),
                }
            })
    };
    let (Ok(iniziale), Ok(barriera)) = (coppia("--attendi"), coppia("--barriera")) else {
        return lamenta("--attendi e --barriera vogliono due fifo ciascuna: pronto e via");
    };

    // L'attesa iniziale, prima del preflight e prima di qualunque lettura.
    if let Some((pronto, via)) = iniziale {
        dichiara("attesa_iniziale_in_corso", "si");
        if let Err(errore) = attendi(&pronto, &via) {
            return lamenta(&format!("attesa iniziale: {errore}"));
        }
        dichiara("attesa_iniziale_conclusa", "si");
    }

    let [_, _, dominio, radice, tetto, uid, gid, ..] = testa else {
        return lamenta("supervisore vuole dominio, radice, tetto, uid e gid");
    };
    let (dominio, radice) = (PathBuf::from(dominio), PathBuf::from(radice));
    let (Some(tetto), Some(uid), Some(gid)) = (numero(tetto), numero(uid), numero(gid)) else {
        return lamenta("tetto, uid e gid vogliono essere numeri");
    };
    let (Ok(uid), Ok(gid)) = (u32::try_from(uid), u32::try_from(gid)) else {
        return lamenta("uid e gid non entrano in u32");
    };
    let worker = IdentitaWorker { uid, gid };

    let mut gerarchia = match Gerarchia::nuova(&dominio, &radice) {
        Ok(gerarchia) => gerarchia,
        Err(difetto) => return lamenta(&format!("gerarchia: {difetto}")),
    };
    let preparato = match prepara_dominio(&mut gerarchia, tetto, worker) {
        Ok(preparato) => preparato,
        Err(errore) => return lamenta(&format!("preflight: {errore}")),
    };
    dichiara("preflight", "riuscito");

    let da_eseguire = DaEseguire {
        eseguibile: Path::new(eseguibile),
        argomenti: argomenti_worker,
    };
    let esito = match barriera {
        None => avvia(preparato, &da_eseguire),
        Some((pronto, via)) => avvia_con_barriera(preparato, &da_eseguire, || {
            dichiara("barriera_in_attesa", "si");
            attendi(&pronto, &via)
        }),
    };

    match esito {
        Ok(mut riuscita) => {
            riporta_evidenza(&riuscita.evidenza);
            dichiara("avvio", "riuscito");
            dichiara("figlio_pid", &riuscita.figlio.id().to_string());
            match riuscita.figlio.wait() {
                Ok(stato) => {
                    dichiara("figlio_uscita", &stato.code().map_or_else(
                        || "ucciso da un segnale".to_owned(),
                        |codice| codice.to_string(),
                    ));
                    ExitCode::SUCCESS
                }
                Err(errore) => lamenta(&format!("attesa del figlio: {errore}")),
            }
        }
        Err(fallita) => {
            // L'evidenza esce **anche qui**: il dominio e' gia' configurato, e
            // chi lo smonta deve sapere quale sia.
            riporta_evidenza(&fallita.evidenza);
            dichiara("avvio", "fallito");
            lamenta(&format!("avvio: {}", fallita.causa))
        }
    }
}

/// Il modo ostile: tre tentativi, e nessuno deve riuscire.
///
/// La riga di comando e' `ostile <dominio> <radice>`.
///
/// # Perche' esce sempre a zero
///
/// Perche' cio' che si giudica non e' se questo programma e' contento, ma che
/// cosa ha osservato: un codice d'uscita che riassumesse tre tentativi
/// perderebbe quale dei tre riesce quando non deve. Il gate legge le righe e
/// decide; questo modo riporta e basta. Un'uscita non a zero resta riservata a
/// cio' che gli impedisce di riportare.
fn modo_ostile(argomenti: &[OsString]) -> ExitCode {
    dichiara("modo_ostile", "avviato");
    let [_, _, dominio, radice] = argomenti else {
        return lamenta("ostile vuole dominio e radice");
    };
    let (dominio, radice) = (PathBuf::from(dominio), PathBuf::from(radice));
    riporta_identita("prima_unshare");
    riporta_leggibilita_di_proc("nel_worker");

    // Primo tentativo: i quattro controlli del dominio.
    for controllo in Controllo::ORDINE {
        let bersaglio = dominio.join(controllo.file());
        let prima = leggi(&bersaglio);
        let esito = scrivi(&bersaglio, "1");
        let dopo = leggi(&bersaglio);
        dichiara(
            &format!("tentativo_controllo_{}", controllo.file()),
            &format!("esito={esito} prima={prima} dopo={dopo}"),
        );
    }

    // Secondo tentativo: uscire dal dominio, che si fa scrivendo il
    // `cgroup.procs` di **un altro** cgroup. Il proprio non porta da nessuna
    // parte: ci si e' gia'.
    //
    // I bersagli sono due, e non sono equivalenti. Il padre e' quello ovvio, ma
    // in cgroup v2 nessun processo puo' abitare un cgroup che ha figli e
    // controllori delegati — nemmeno il control plane — quindi un rifiuto li'
    // non dice niente sul worker: dice solo com'e' fatta la gerarchia. Un
    // **fratello foglia** e' invece scrivibile dal control plane, ed e' il
    // bersaglio su cui il rifiuto discrimina.
    let padre = dominio
        .parent()
        .map_or_else(|| radice.clone(), Path::to_path_buf);
    let fuga = padre.join("cgroup.procs");
    dichiara(
        "tentativo_fuga_padre",
        &format!("bersaglio={} {}", fuga.display(), tentativo(&fuga)),
    );
    let vicini = match fratelli(&padre, &dominio) {
        Ok(vicini) => vicini,
        Err(motivo) => return lamenta(&format!("fratelli del dominio: {motivo}")),
    };
    for vicino in &vicini {
        let bersaglio = vicino.join("cgroup.procs");
        dichiara(
            "tentativo_fuga_vicino",
            &format!("bersaglio={} {}", bersaglio.display(), tentativo(&bersaglio)),
        );
    }

    // Terzo tentativo: `unshare` di uno user namespace, e poi il control plane.
    // Non si pretende che la `unshare` fallisca — `no_new_privs` non la
    // impedisce — ma che dopo di essa il control plane resti fuori portata.
    let unshare = rustix::thread::unshare(rustix::thread::UnshareFlags::NEWUSER);
    dichiara(
        "tentativo_unshare",
        &unshare.as_ref().map_or_else(
            |errore| format!("rifiutata: {errore}"),
            |()| "riuscita".to_owned(),
        ),
    );
    if unshare.is_ok() {
        riporta_identita("dopo_unshare");
    }
    let tetto = dominio.join(Controllo::Tetto.file());
    let prima = leggi(&tetto);
    let esito = scrivi(&tetto, "1");
    let dopo = leggi(&tetto);
    dichiara(
        "tentativo_dopo_unshare",
        &format!("esito={esito} prima={prima} dopo={dopo}"),
    );
    dichiara(
        "tentativo_dopo_unshare_fuga",
        &format!("bersaglio={} {}", fuga.display(), tentativo(&fuga)),
    );
    for vicino in &vicini {
        let bersaglio = vicino.join("cgroup.procs");
        dichiara(
            "tentativo_dopo_unshare_fuga_vicino",
            &format!("bersaglio={} {}", bersaglio.display(), tentativo(&bersaglio)),
        );
    }

    dichiara("ostile", "concluso");
    ExitCode::SUCCESS
}

/// I cgroup fratelli del dominio: le vie d'uscita che esistono davvero.
///
/// # Perche' fallisce invece di rendere una lista corta
///
/// Perche' una lista vuota e una lista che non si e' potuta leggere sono
/// indistinguibili dall'esterno, e il gate le legge allo stesso modo: «il
/// worker non ha tentato nessuna fuga». Sarebbe un verde che dice «non e'
/// riuscito» quando il vero significato e' «non ha provato».
///
/// # Errors
///
/// Il motivo, in forma di frase: la directory che non si apre, o la voce che
/// non si legge.
fn fratelli(padre: &Path, dominio: &Path) -> std::result::Result<Vec<PathBuf>, String> {
    let elenco =
        std::fs::read_dir(padre).map_err(|errore| format!("{}: {errore}", padre.display()))?;
    let mut trovati = Vec::new();
    for esito in elenco {
        let figlio = esito.map_err(|errore| format!("{}: {errore}", padre.display()))?;
        let percorso = figlio.path();
        let tipo = figlio
            .file_type()
            .map_err(|errore| format!("{}: {errore}", percorso.display()))?;
        if tipo.is_dir() && percorso != dominio {
            trovati.push(percorso);
        }
    }
    trovati.sort();
    Ok(trovati)
}

/// Che cosa di `/proc/self` e' leggibile in questo momento.
///
/// # Perche' e' evidenza e non un dettaglio
///
/// Perche' la verifica finale dello spawner porta avanti namespace e
/// descrittori invece di rileggerli, e la necessita' di quella scelta dipende da
/// che cosa `/proc/self` concede in quel preciso momento. Un'affermazione del
/// genere va **misurata** sulla macchina che si qualifica.
///
/// Il prefisso conta: la stessa misura presa in due momenti diversi dice due
/// cose diverse, e senza il momento non si sa quale delle due si sta leggendo.
fn riporta_leggibilita_di_proc(quando: &str) {
    for nome in ["status", "ns", "fd", "fdinfo"] {
        let percorso = format!("/proc/self/{nome}");
        let esito = if nome == "status" {
            std::fs::read_to_string(&percorso).map(|_| ())
        } else {
            std::fs::read_dir(&percorso).map(|_| ())
        };
        dichiara(
            &format!("proc_leggibile_{quando}_{nome}"),
            &esito.map_or_else(|errore| format!("no: {errore}"), |()| "si".to_owned()),
        );
    }
}

/// Il modo finestra: che cosa resta leggibile fra il cambio d'identita' e la
/// `exec`.
///
/// La riga di comando e' `finestra <uid> <gid>`.
///
/// # Perche' e' un modo a se'
///
/// Perche' quell'intervallo non lo attraversa nessun altro modo, e le due
/// misure che gli somigliano dicono un'altra cosa. Prima del cambio il processo
/// e' ancora se stesso; dopo la `exec` il kernel **rimette** il flag *dumpable*
/// e restituisce `/proc/<pid>` al nuovo proprietario, quindi il worker si legge
/// senza problemi. Solo in mezzo — credenziali cambiate, immagine ancora la
/// stessa — `/proc/<pid>` appartiene a root e le sue directory non si
/// attraversano.
///
/// E' esattamente li' che vive il settimo passo della sequenza, ed e' la ragione
/// per cui rilegge le credenziali invece dell'identita' intera. Misurarlo qui
/// trasforma quella ragione da argomento in osservazione.
///
/// # Perche' non esegue niente dopo
///
/// Perche' non deve: le credenziali cambiate valgono per questo thread e il
/// processo muore subito dopo aver riportato. Non c'e' nessun worker da
/// avviare, e avviarlo confonderebbe due misure.
fn modo_finestra(argomenti: &[OsString]) -> ExitCode {
    dichiara("modo_finestra", "avviato");
    let [_, _, uid, gid] = argomenti else {
        return lamenta("finestra vuole uid e gid");
    };
    let (Some(uid), Some(gid)) = (numero(uid), numero(gid)) else {
        return lamenta("uid e gid vogliono essere numeri");
    };
    let (Ok(uid), Ok(gid)) = (u32::try_from(uid), u32::try_from(gid)) else {
        return lamenta("uid e gid non entrano in u32");
    };

    riporta_leggibilita_di_proc("prima_del_cambio");

    if let Err(errore) = rustix::thread::set_no_new_privs(true) {
        return lamenta(&format!("no_new_privs: {errore}"));
    }
    if let Err(errore) = rustix::thread::set_thread_groups(&[]) {
        return lamenta(&format!("gruppi: {errore}"));
    }
    let gid = rustix::process::Gid::from_raw(gid);
    if let Err(errore) = rustix::thread::set_thread_res_gid(gid, gid, gid) {
        return lamenta(&format!("GID: {errore}"));
    }
    let uid = rustix::process::Uid::from_raw(uid);
    if let Err(errore) = rustix::thread::set_thread_res_uid(uid, uid, uid) {
        return lamenta(&format!("UID: {errore}"));
    }

    riporta_leggibilita_di_proc("dopo_il_cambio");
    dichiara("finestra", "conclusa");
    ExitCode::SUCCESS
}

/// Aspetta il gate: si annuncia pronto, e poi aspetta il via.
///
/// # Perche' due fifo e non un'attesa a tempo
///
/// Perche' un'attesa a tempo non e' una barriera: il gate non saprebbe se la
/// sostituzione e' arrivata dentro la finestra, e un esito giusto sarebbe
/// indistinguibile da un esito fortunato. Aprire una fifo in scrittura blocca
/// finche' qualcuno non la apre in lettura, e viceversa: sono due appuntamenti,
/// e nessuno dei due passa per l'orologio.
fn attendi(pronto: &Path, via: &Path) -> plenora_core::error::Result<()> {
    std::fs::write(pronto, b"pronto\n").map_err(|errore| {
        super::non_disponibile("barriera", &format!("{}: {errore}", pronto.display()))
    })?;
    std::fs::read(via)
        .map(|_| ())
        .map_err(|errore| super::non_disponibile("barriera", &format!("{}: {errore}", via.display())))
}

/// Il numero di task del processo.
fn conta_task() -> std::result::Result<usize, String> {
    let voci =
        std::fs::read_dir("/proc/self/task").map_err(|errore| format!("/proc/self/task: {errore}"))?;
    let mut quanti = 0_usize;
    for voce in voci {
        voce.map_err(|errore| format!("/proc/self/task: {errore}"))?;
        quanti += 1;
    }
    Ok(quanti)
}

/// `dispositivo:inode` dell'immagine in esecuzione.
///
/// E' cio' con cui il gate distingue l'immagine iniziale dalla sostitutiva
/// senza doverle rendere diverse: due copie dello stesso binario hanno lo
/// stesso contenuto e inode diversi, e l'inode e' quello che conta.
fn immagine() -> std::result::Result<String, String> {
    use std::os::unix::fs::MetadataExt as _;
    let dati = std::fs::metadata("/proc/self/exe")
        .map_err(|errore| format!("/proc/self/exe: {errore}"))?;
    Ok(format!("{}:{}", dati.dev(), dati.ino()))
}

/// Le sette osservazioni del preflight, una riga ciascuna.
fn riporta_evidenza(evidenza: &EvidenzaPreflight) {
    dichiara("evidenza_dominio", &evidenza.dominio.display().to_string());
    dichiara("evidenza_radice", &evidenza.radice.display().to_string());
    dichiara(
        "evidenza_worker",
        &format!("{}:{}", evidenza.worker.uid, evidenza.worker.gid),
    );
    dichiara("evidenza_tetto", &evidenza.tetto_byte.to_string());
    dichiara(
        "evidenza_montaggio",
        &format!(
            "punto={} radice={} dispositivo={} superblocco={}",
            evidenza.montaggio.punto.display(),
            evidenza.montaggio.radice.display(),
            evidenza.montaggio.dispositivo,
            evidenza.montaggio.opzioni_superblocco
        ),
    );
    dichiara(
        "evidenza_namespace",
        &evidenza
            .namespace_attesi
            .iter()
            .map(|(nome, valore)| format!("{nome}={valore}"))
            .collect::<Vec<_>>()
            .join(","),
    );
    dichiara(
        "evidenza_eventi_locali",
        &evidenza.eventi_locali.to_string(),
    );
}

/// Identita', gruppi, capability, `no_new_privs` e namespace, un campo per
/// cosa.
///
/// # Perche' campi e non un `Debug`
///
/// Perche' il gate deve poterci fare **asserzioni esatte**, non una ricerca di
/// sottostringhe. Un `Debug` e' una riga sola in cui `uid: [65534, 65534, …]`
/// e `bounding: 2199023255551` stanno mescolati a tutto il resto: per
/// pretendere che le capability effettive siano zero bisognerebbe cercare un
/// pezzo di testo, e una ricerca di testo passa anche quando il campo cambia
/// nome o quando due campi si somigliano.
///
/// Con un campo per cosa il gate confronta valori. E' la differenza fra un
/// gate che legge e uno che sembra leggere.
fn riporta_identita(quando: &str) {
    let identita = match super::identita::leggi_identita() {
        Ok(identita) => identita,
        Err(motivo) => {
            dichiara(&format!("id_{quando}_leggibile"), &format!("no: {motivo}"));
            return;
        }
    };
    let quaterna = |valori: [u32; 4]| {
        valori
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    dichiara(&format!("id_{quando}_leggibile"), "si");
    dichiara(&format!("id_{quando}_uid"), &quaterna(identita.uid));
    dichiara(&format!("id_{quando}_gid"), &quaterna(identita.gid));
    dichiara(
        &format!("id_{quando}_gruppi"),
        &identita
            .gruppi
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
    );
    dichiara(
        &format!("id_{quando}_no_new_privs"),
        if identita.no_new_privs { "1" } else { "0" },
    );
    for (nome, valore) in [
        ("cap_effective", identita.capability.effective),
        ("cap_permitted", identita.capability.permitted),
        ("cap_inheritable", identita.capability.inheritable),
        ("cap_ambient", identita.capability.ambient),
        ("cap_bounding", identita.capability.bounding),
    ] {
        dichiara(&format!("id_{quando}_{nome}"), &valore.to_string());
    }
    for (nome, valore) in &identita.namespace {
        dichiara(&format!("id_{quando}_ns_{nome}"), valore);
    }
    dichiara(
        &format!("id_{quando}_descrittori_scrivibili"),
        &identita.descrittori_scrivibili.len().to_string(),
    );
}

/// Il contenuto di un file, o il motivo per cui non si legge.
///
/// Serve al **prima/dopo** di ogni tentativo: un tentativo rifiutato che
/// lasciasse il valore cambiato non sarebbe rifiutato, e senza le due letture
/// nessuno se ne accorgerebbe.
fn leggi(percorso: &Path) -> String {
    std::fs::read_to_string(percorso).map_or_else(
        |errore| format!("«{errore}»"),
        |contenuto| format!("«{}»", contenuto.trim()),
    )
}

/// Un tentativo, col contenuto del bersaglio **prima e dopo**.
///
/// # Perche' ogni tentativo porta le due letture
///
/// Perche' «non e' riuscito» e «non ha cambiato niente» sono due cose diverse,
/// e la prima non implica la seconda: una scrittura puo' essere rifiutata dopo
/// aver troncato il file, e un rifiuto che arriva a meta' lascia un valore
/// diverso da quello di partenza. Senza le due letture il gate potrebbe solo
/// dedurre l'invarianza dall'esito, che e' esattamente la deduzione sbagliata.
///
/// Vale anche per le vie d'uscita: `cgroup.procs` prima e dopo dice se il
/// worker ci si e' spostato, e lo dice meglio dell'esito della `write` —
/// perche' una `write` accettata che non sposta niente si legge come una
/// riuscita.
fn tentativo(percorso: &Path) -> String {
    let prima = leggi(percorso);
    let esito = scrivi(percorso, "0");
    let dopo = leggi(percorso);
    format!("esito={esito} prima={prima} dopo={dopo}")
}

/// Prova a scrivere, e dice com'e' andata.
fn scrivi(percorso: &Path, valore: &str) -> String {
    let aperto = std::fs::OpenOptions::new().write(true).open(percorso);
    match aperto {
        Err(errore) => format!("rifiutato in apertura: {errore}"),
        Ok(mut file) => match file.write_all(valore.as_bytes()) {
            Err(errore) => format!("rifiutato in scrittura: {errore}"),
            Ok(()) => "RIUSCITO".to_owned(),
        },
    }
}

/// Un numero, o niente.
fn numero(campo: &OsString) -> Option<u64> {
    campo.to_str().and_then(|testo| testo.parse().ok())
}

/// Una riga di evidenza.
fn dichiara(chiave: &str, valore: &str) {
    println!("QI {chiave}={valore}");
}

/// Il motivo, e l'uscita non a zero.
fn lamenta(motivo: &str) -> ExitCode {
    dichiara("errore", motivo);
    ExitCode::FAILURE
}
