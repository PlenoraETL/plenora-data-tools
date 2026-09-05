//! PT-Linux — prototipo bloccante dell'esecuzione isolata.
//!
//! Non e' codice di produzione e non e' coperto dai gate del workspace.
//! Deve rispondere a domande, non fornire un'implementazione:
//!
//! 1. **nascita gia' vincolata** — il worker esiste solo dentro il dominio?
//! 2. **contenimento** — la memoria e' davvero trattenuta sotto il tetto?
//! 3. **attribuzione** — l'evento e' leggibile e riferito al nostro cgroup?
//! 4. **terminazione vs allocazione negata** — quale dei due, e che cosa fa il
//!    worker dopo un'allocazione negata?
//! 5. **integrazione** — clone3, delega, `memory.oom.group`, discendenti,
//!    fattibilita' senza `unsafe`.
//!
//! Ogni misura viene stampata come riga `MISURA <chiave> <valore>`, cosi' il
//! registro non dipende da come io riassumo l'output.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RADICE: &str = "/sys/fs/cgroup";
const PREFISSO: &str = "pt-linux";

fn main() {
    let argomenti: Vec<String> = std::env::args().collect();
    match argomenti.get(1).map(String::as_str) {
        Some("spawner") => spawner(&argomenti[2..]),
        Some("carico") => carico(&argomenti[2..]),
        _ => supervisore(),
    }
}

// ---------------------------------------------------------------------------
// Utilita' sul cgroup: nient'altro che lettura e scrittura di file.
// ---------------------------------------------------------------------------

fn leggi(percorso: &Path) -> String {
    fs::read_to_string(percorso).map_or_else(|e| format!("<illeggibile: {e}>"), |t| t.trim().to_owned())
}

fn leggi_intero(dominio: &Path, file: &str) -> Option<u64> {
    fs::read_to_string(dominio.join(file)).ok()?.trim().parse().ok()
}

/// `memory.events.local` e' `chiave valore` per riga.
fn eventi_locali(dominio: &Path) -> BTreeMap<String, u64> {
    let mut mappa = BTreeMap::new();
    if let Ok(testo) = fs::read_to_string(dominio.join("memory.events.local")) {
        for riga in testo.lines() {
            let mut parti = riga.split_whitespace();
            if let (Some(chiave), Some(valore)) = (parti.next(), parti.next()) {
                if let Ok(n) = valore.parse() {
                    mappa.insert(chiave.to_owned(), n);
                }
            }
        }
    }
    mappa
}

/// `memory.events` aggrega i discendenti; `memory.events.local` no. La
/// differenza fra i due e' l'oggetto dello scenario L8.
fn eventi_gerarchici(dominio: &Path) -> BTreeMap<String, u64> {
    let mut mappa = BTreeMap::new();
    if let Ok(testo) = fs::read_to_string(dominio.join("memory.events")) {
        for riga in testo.lines() {
            let mut parti = riga.split_whitespace();
            if let (Some(chiave), Some(valore)) = (parti.next(), parti.next()) {
                if let Ok(n) = valore.parse() {
                    mappa.insert(chiave.to_owned(), n);
                }
            }
        }
    }
    mappa
}

/// `cgroup.events`: `populated` vale 1 se il cgroup **o un suo discendente**
/// contiene processi vivi. E' il kernel a garantirlo, quindi non serve
/// camminare la gerarchia — che e' una lettura non atomica di molti file
/// mentre i processi si spostano.
fn popolato(dominio: &Path) -> Option<bool> {
    let testo = fs::read_to_string(dominio.join("cgroup.events")).ok()?;
    for riga in testo.lines() {
        let mut parti = riga.split_whitespace();
        if parti.next() == Some("populated") {
            return parti.next().map(|v| v == "1");
        }
    }
    None
}

/// Quanti processi vivono adesso nel dominio, discendenti compresi.
fn processi_vivi(dominio: &Path) -> usize {
    let mut totale = fs::read_to_string(dominio.join("cgroup.procs"))
        .map(|t| t.split_whitespace().count())
        .unwrap_or(0);
    if let Ok(voci) = fs::read_dir(dominio) {
        for voce in voci.flatten() {
            if voce.path().is_dir() {
                totale += processi_vivi(&voce.path());
            }
        }
    }
    totale
}

fn abilita_memoria(cartella: &Path) -> Result<(), String> {
    let percorso = cartella.join("cgroup.subtree_control");
    let disponibili = leggi(&cartella.join("cgroup.controllers"));
    if !disponibili.split_whitespace().any(|c| c == "memory") {
        return Err(format!("il controller memory non e' disponibile in {}", cartella.display()));
    }
    fs::write(&percorso, "+memory").map_err(|e| format!("{}: {e}", percorso.display()))
}

/// Svuota il cgroup corrente e abilita il controller `memory` sui due livelli
/// che servono.
///
/// **Regola dei processi interni (cgroup v2).** Un cgroup non-radice non puo'
/// avere insieme processi propri e `subtree_control` popolato. Dentro un
/// container con namespace di cgroup privato, `/sys/fs/cgroup` **non e' la
/// radice reale**: e' un cgroup normale che contiene i nostri processi, quindi
/// la regola si applica e la delega fallisce con `EBUSY` finche' non lo si
/// svuota. E' esattamente il vincolo che il supervisore incontrera' in
/// produzione ogni volta che gira dentro un container.
fn prepara_gerarchia() -> Result<PathBuf, String> {
    let radice = PathBuf::from(RADICE);
    let base = radice.join(PREFISSO);
    let riparo = base.join("supervisore");
    for cartella in [&base, &riparo] {
        if !cartella.exists() {
            fs::create_dir(cartella).map_err(|e| format!("mkdir {}: {e}", cartella.display()))?;
        }
    }

    // Sposta ogni processo del cgroup corrente nel riparo, cosi' il cgroup
    // corrente resta senza processi e puo' delegare.
    let procs = radice.join("cgroup.procs");
    let elenco = fs::read_to_string(&procs).map_err(|e| format!("{}: {e}", procs.display()))?;
    let mut spostati = 0_u32;
    for pid in elenco.split_whitespace() {
        if fs::write(riparo.join("cgroup.procs"), pid).is_ok() {
            spostati += 1;
        }
    }
    println!("MISURA gerarchia.processi_spostati {spostati}");

    abilita_memoria(&radice)?;
    abilita_memoria(&base)?;
    Ok(base)
}

/// Crea un dominio vuoto con il tetto richiesto. Nessun ripiego: se qualcosa
/// non si puo' fare, il prototipo lo dice e si ferma.
/// Come il dominio deve difendersi dai sottogruppi.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Foglia {
    /// Nessuna difesa: il worker puo' creare sottogruppi.
    Aperta,
    /// `cgroup.max.depth = 0`: e' il kernel a rifiutare i discendenti.
    ProfonditaZero,
}

fn crea_dominio(
    base: &Path,
    nome: &str,
    tetto: u64,
    oom_group: bool,
    foglia: Foglia,
) -> Result<PathBuf, String> {
    let dominio = base.join(nome);
    if dominio.exists() {
        let _ = fs::remove_dir(&dominio);
    }
    fs::create_dir(&dominio).map_err(|e| format!("mkdir {}: {e}", dominio.display()))?;
    fs::write(dominio.join("memory.max"), tetto.to_string())
        .map_err(|e| format!("memory.max: {e}"))?;
    // Lo swap va spento, altrimenti il tetto misura la cosa sbagliata. Se il
    // file non c'e', la misura resta valida ma va detto.
    if fs::write(dominio.join("memory.swap.max"), "0").is_err() {
        println!("MISURA {nome}.swap_non_governabile si");
    }
    if oom_group {
        fs::write(dominio.join("memory.oom.group"), "1")
            .map_err(|e| format!("memory.oom.group: {e}"))?;
    }
    if foglia == Foglia::ProfonditaZero {
        // Sigilla la foglia: nessun discendente, deciso dal kernel e non dai
        // permessi. E' l'alternativa da misurare all'evidenza gerarchica.
        match fs::write(dominio.join("cgroup.max.depth"), "0") {
            Ok(()) => println!("MISURA {nome}.max_depth_impostato si"),
            Err(e) => println!("MISURA {nome}.max_depth_impostato no ({e})"),
        }
    }
    Ok(dominio)
}

/// Scorciatoia per gli scenari che non sigillano nulla.
fn crea_dominio_aperto(
    base: &Path,
    nome: &str,
    tetto: u64,
    oom_group: bool,
) -> Result<PathBuf, String> {
    crea_dominio(base, nome, tetto, oom_group, Foglia::Aperta)
}

fn rimuovi_dominio(dominio: &Path) {
    let _ = fs::remove_dir(dominio);
}

// ---------------------------------------------------------------------------
// Modalita' `spawner`: entra nel dominio e poi si sostituisce col carico.
//
// E' la strada SAFE. `Command::exec` e la scrittura su `cgroup.procs` sono
// entrambe API sicure: dopo la exec l'immagine del worker viene mappata
// *dentro* il cgroup, quindi anche il caricamento e' contabilizzato.
// ---------------------------------------------------------------------------

fn spawner(resto: &[String]) -> ! {
    let dominio = PathBuf::from(&resto[0]);
    let utente: u32 = resto[1].parse().unwrap_or(0);
    let pid = std::process::id();
    if let Err(e) = fs::write(dominio.join("cgroup.procs"), pid.to_string()) {
        eprintln!("SPAWNER errore cgroup.procs: {e}");
        std::process::exit(101);
    }
    // Verifica di essere entrato prima di sostituire l'immagine.
    let mio = leggi(Path::new("/proc/self/cgroup"));
    eprintln!("SPAWNER cgroup-prima-di-exec {mio}");

    // `oom_score_adj` si eredita. A -1000 il kernel NON uccide il task,
    // nemmeno con `memory.oom.group=1`: un worker che lo eredita da un
    // chiamante protetto sopravvive al group kill.
    let ereditato = leggi(Path::new("/proc/self/oom_score_adj"));
    eprintln!("SPAWNER oom_score_adj-ereditato {ereditato}");
    if std::env::var_os("PT_NON_NORMALIZZARE").is_none() {
        if let Err(e) = fs::write("/proc/self/oom_score_adj", "0") {
            eprintln!("SPAWNER oom_score_adj-normalizzazione-fallita {e}");
            std::process::exit(103);
        }
        // Rilettura: scrivere senza verificare non e' normalizzare, e' sperare.
        let riletto = leggi(Path::new("/proc/self/oom_score_adj"));
        eprintln!("SPAWNER oom_score_adj-riletto {riletto}");
        if riletto != "0" {
            eprintln!("SPAWNER oom_score_adj-readback-incoerente: fallisco chiuso");
            std::process::exit(104);
        }
    } else {
        eprintln!("SPAWNER oom_score_adj-normalizzazione DISATTIVATA (braccio di controllo)");
    }

    let mut comando = Command::new(std::env::current_exe().expect("current_exe"));
    comando.arg("carico").args(&resto[2..]);
    if utente != 0 {
        // `uid`/`gid` sono API sicure e valgono anche per `exec`: il worker
        // nasce dentro il dominio E senza i privilegi per manometterlo.
        comando.uid(utente).gid(utente);
    }
    let errore = comando.exec();
    eprintln!("SPAWNER exec fallita: {errore}");
    std::process::exit(102);
}

// ---------------------------------------------------------------------------
// Modalita' `carico`: il finto worker.
// ---------------------------------------------------------------------------

fn carico(resto: &[String]) {
    let scenario = resto.first().map_or("tocca", String::as_str);

    // Primo atto: dire dove si e' nati. Se il cgroup non e' il nostro, la
    // proprieta' "nascita gia' vincolata" e' falsa e tutto il resto non conta.
    println!("CARICO cgroup {}", leggi(Path::new("/proc/self/cgroup")));
    println!("CARICO scenario {scenario}");
    println!("CARICO oom_score_adj {}", leggi(Path::new("/proc/self/oom_score_adj")));
    let _ = std::io::stdout().flush();

    match scenario {
        "chiedi" => chiedi_senza_toccare(),
        "figlio" => genera_figlio(&resto[1..]),
        "sottogruppo" => prova_sottogruppo(),
        "sottogruppo_oom" => sottogruppo_oom(),
        "evasione" => evasione_in_tre_passi(),
        "orfano" => genera_orfano(),
        "dorme" => {
            println!("CARICO dorme oom_score_adj={}", leggi(Path::new("/proc/self/oom_score_adj")));
            let _ = std::io::stdout().flush();
            std::thread::sleep(std::time::Duration::from_millis(1500));
            println!("CARICO dorme fine");
        }
        // Serve a misurare il solo costo d'avvio. Senza questo ramo si cade nel
        // caso predefinito e si alloca fino all'OOM: `l11` riporterebbe come
        // «picco d'avvio» il tetto del dominio, cioe' un numero vero che
        // misura un'altra cosa.
        "inerte" => println!("CARICO inerte fine"),
        "sottogruppo_vivo" => sottogruppo_vivo(),
        "riscrivi" => riscrivi_il_preflight(),
        _ => tocca_finche_muore(),
    }
}

/// Il cgroup del processo corrente, come percorso sotto la radice.
fn mio_dominio() -> Option<PathBuf> {
    let mio = leggi(Path::new("/proc/self/cgroup"));
    let relativo = mio.rsplit("::").next()?.trim();
    let relativo = relativo.strip_prefix('/').unwrap_or(relativo);
    Some(PathBuf::from(RADICE).join(relativo))
}

/// Il cammino che `memory.events.local` non vede: il worker si crea un
/// sottogruppo, gli da' il controller della memoria, ci mette dentro un figlio
/// e lo lascia sfondare li'.
fn sottogruppo_oom() {
    let Some(mio_dominio) = mio_dominio() else {
        println!("CARICO sottogruppo_oom cgroup-non-risolto");
        return;
    };
    let interno = mio_dominio.join("interno");
    if let Err(e) = fs::create_dir(&interno) {
        println!("CARICO sottogruppo_oom mkdir-rifiutato {e}");
        return;
    }
    println!("CARICO sottogruppo_oom mkdir-riuscito");
    match fs::write(mio_dominio.join("cgroup.subtree_control"), "+memory") {
        Ok(()) => println!("CARICO sottogruppo_oom controller-delegato si"),
        Err(e) => println!("CARICO sottogruppo_oom controller-delegato no ({e})"),
    }
    let _ = std::io::stdout().flush();

    // Il figlio si sposta da solo nel sottogruppo e poi sfonda.
    let mio = std::env::current_exe().expect("current_exe");
    let figlio = Command::new(mio)
        .arg("spawner")
        .arg(&interno)
        .arg("0")
        .arg("tocca")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    match figlio {
        Ok(mut figlio) => {
            let stato = figlio.wait().expect("wait");
            println!(
                "CARICO sottogruppo_oom figlio codice={:?} segnale={:?}",
                stato.code(),
                stato.signal()
            );
        }
        Err(e) => println!("CARICO sottogruppo_oom spawn-fallito {e}"),
    }
    println!("CARICO padre-sopravvissuto si");
    let _ = std::io::stdout().flush();
}

/// La sequenza completa: sposta se stesso, poi delega, poi sfonda.
///
/// Se riesce, l'evidenza dell'OOM non appare piu' in `memory.events.local`
/// del dominio, perche' il processo ucciso non gli appartiene piu'.
fn evasione_in_tre_passi() {
    let Some(dominio) = mio_dominio() else {
        println!("CARICO evasione cgroup-non-risolto");
        return;
    };
    let interno = dominio.join("evaso");

    // 1. Crea il sottogruppo.
    if let Err(e) = fs::create_dir(&interno) {
        println!("CARICO evasione passo1-mkdir rifiutato ({e})");
        return;
    }
    println!("CARICO evasione passo1-mkdir riuscito");

    // 2. Si sposta dentro: da qui il dominio non ha piu' processi propri.
    let pid = std::process::id();
    if let Err(e) = fs::write(interno.join("cgroup.procs"), pid.to_string()) {
        println!("CARICO evasione passo2-spostamento rifiutato ({e})");
        return;
    }
    println!("CARICO evasione passo2-spostamento riuscito");
    println!("CARICO evasione cgroup-ora {}", leggi(Path::new("/proc/self/cgroup")));

    // 3. Ora la delega del controller dovrebbe passare.
    match fs::write(dominio.join("cgroup.subtree_control"), "+memory") {
        Ok(()) => println!("CARICO evasione passo3-delega riuscita"),
        Err(e) => {
            println!("CARICO evasione passo3-delega rifiutata ({e})");
            return;
        }
    }
    let _ = std::io::stdout().flush();

    // 4. E adesso sfonda, da dentro il sottogruppo.
    tocca_finche_muore();
}

/// Prova a disfare, dall'interno, tutto cio' che il preflight ha stabilito.
///
/// Non e' un worker ostile: e' un worker che chiama le stesse API del
/// supervisore perche' vive nello stesso processo-famiglia e con gli stessi
/// privilegi. `NG-7` dice che il modello di minaccia e' il guasto — ma un
/// guasto che riscrive `memory.max` produce lo stesso effetto di un attacco.
fn riscrivi_il_preflight() {
    let Some(dominio) = mio_dominio() else {
        println!("CARICO riscrivi cgroup-non-risolto");
        return;
    };
    // `loginuid` non e' l'identita' effettiva: leggere quello stampa
    // 4294967295, che non dice niente.
    let identita = leggi(Path::new("/proc/self/status"))
        .lines()
        .find(|r| r.starts_with("Uid:"))
        .unwrap_or("Uid: ignoto")
        .to_owned();
    println!("CARICO riscrivi identita {identita}");

    for (file, valore) in [
        ("cgroup.max.depth", "10"),
        ("memory.max", "1073741824"),
        ("memory.oom.group", "0"),
        ("memory.swap.max", "max"),
    ] {
        match fs::write(dominio.join(file), valore) {
            Ok(()) => {
                let riletto = leggi(&dominio.join(file));
                println!("CARICO riscrivi {file} RIUSCITA nuovo={riletto}");
            }
            Err(e) => println!("CARICO riscrivi {file} rifiutata ({e})"),
        }
    }

    // E la fuga completa: uscire dal dominio scrivendo altrove.
    let fuori = dominio.parent().map(|p| p.join("supervisore"));
    if let Some(fuori) = fuori {
        match fs::write(fuori.join("cgroup.procs"), std::process::id().to_string()) {
            Ok(()) => println!(
                "CARICO riscrivi fuga RIUSCITA ora-sono {}",
                leggi(Path::new("/proc/self/cgroup"))
            ),
            Err(e) => println!("CARICO riscrivi fuga rifiutata ({e})"),
        }
    }
    let _ = std::io::stdout().flush();
}

/// Mette un figlio in un sottogruppo e lo lascia vivo, poi esce.
///
/// Serve a L15: al ritorno della `wait` il `cgroup.procs` del dominio e'
/// vuoto — il processo sta nel figlio — mentre `populated` deve valere 1.
fn sottogruppo_vivo() {
    let Some(dominio) = mio_dominio() else {
        println!("CARICO sottogruppo_vivo cgroup-non-risolto");
        return;
    };
    let interno = dominio.join("vivo");
    if let Err(e) = fs::create_dir(&interno) {
        println!("CARICO sottogruppo_vivo mkdir-rifiutato {e}");
        return;
    }
    let mio = std::env::current_exe().expect("current_exe");
    let figlio = Command::new(mio)
        .arg("spawner")
        .arg(&interno)
        .arg("0")
        .arg("dorme")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match figlio {
        Ok(figlio) => {
            println!("CARICO sottogruppo_vivo figlio-pid {}", figlio.id());
            // Aspetta che il nipote sia INSEDIATO nel sottogruppo. Uscire
            // prima lascerebbe la domanda senza risposta: `cgroup.procs` del
            // dominio lo vedrebbe ancora, e non perche' funzioni.
            let mut giri = 0;
            let insediato = loop {
                let dentro = fs::read_to_string(interno.join("cgroup.procs"))
                    .map(|t| t.split_whitespace().count())
                    .unwrap_or(0);
                if dentro > 0 {
                    break true;
                }
                if giri > 200 {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
                giri += 1;
            };
            println!(
                "CARICO sottogruppo_vivo nipote-insediato {} dopo {} ms",
                if insediato { "si" } else { "NO" },
                giri * 5
            );
        }
        Err(e) => println!("CARICO sottogruppo_vivo spawn-fallito {e}"),
    }
    println!("CARICO capofila-esce-subito con-uscita-0");
    let _ = std::io::stdout().flush();
}

/// Un discendente che sopravvive al capofila: serve a vedere se al ritorno
/// della `wait` il dominio e' davvero quiescente.
fn genera_orfano() {
    let mio = std::env::current_exe().expect("current_exe");
    let figlio = Command::new(mio)
        .arg("carico")
        .arg("tocca")
        // Se ereditasse stdout, il supervisore resterebbe in attesa anche di
        // lui e la domanda sulla quiescenza non si porrebbe.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match figlio {
        Ok(figlio) => println!("CARICO orfano-pid {}", figlio.id()),
        Err(e) => println!("CARICO orfano-spawn-fallito {e}"),
    }
    println!("CARICO capofila-esce-subito con-uscita-0");
    let _ = std::io::stdout().flush();
    // Nessuna `wait`: il capofila esce e lascia il figlio vivo.
}

/// Allocazione **richiesta e toccata**: e' il caso che il tetto deve fermare.
fn tocca_finche_muore() {
    let blocco = 4 * 1024 * 1024; // 4 MiB per giro
    let mut trattenuto: Vec<Vec<u8>> = Vec::new();
    let mut totale: usize = 0;
    loop {
        let mut pezzo: Vec<u8> = Vec::new();
        if let Err(e) = pezzo.try_reserve_exact(blocco) {
            // Allocazione NEGATA: il worker e' vivo e puo' reagire.
            println!("CARICO negata-a {totale} errore {e:?}");
            println!("CARICO vivo-dopo-il-rifiuto si");
            let _ = std::io::stdout().flush();
            std::process::exit(42);
        }
        pezzo.resize(blocco, 0xA5);
        // Tocca ogni pagina: e' qui che il kernel addebita davvero.
        for i in (0..blocco).step_by(4096) {
            pezzo[i] = 1;
        }
        trattenuto.push(pezzo);
        totale += blocco;
        if totale % (32 * 1024 * 1024) == 0 {
            println!("CARICO toccati {totale}");
            let _ = std::io::stdout().flush();
        }
        if totale > 8 * 1024 * 1024 * 1024 {
            println!("CARICO nessun-contenimento {totale}");
            let _ = std::io::stdout().flush();
            std::process::exit(43);
        }
    }
}

/// Allocazione **richiesta e non toccata**: mostra che il tetto del cgroup non
/// si manifesta come rifiuto di `malloc`.
fn chiedi_senza_toccare() {
    for potenza in [30_u32, 34, 38, 42, 46] {
        let dimensione = 1_usize << potenza;
        let mut v: Vec<u8> = Vec::new();
        let esito = match v.try_reserve_exact(dimensione) {
            Ok(()) => "accettata",
            Err(_) => "negata",
        };
        println!("CARICO richiesta 2^{potenza} {esito}");
        let _ = std::io::stdout().flush();
        drop(v);
    }
    println!("CARICO fine-richieste");
    let _ = std::io::stdout().flush();
}

/// Discendente: il figlio eredita il cgroup del padre.
fn genera_figlio(_resto: &[String]) {
    let mio = std::env::current_exe().expect("current_exe");
    let mut figlio = Command::new(mio)
        .arg("carico")
        .arg("tocca")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn del figlio");
    println!("CARICO figlio-pid {}", figlio.id());
    let _ = std::io::stdout().flush();
    let stato = figlio.wait().expect("wait del figlio");
    println!("CARICO figlio-stato codice={:?} segnale={:?}", stato.code(), stato.signal());
    println!("CARICO padre-sopravvissuto si");
    let _ = std::io::stdout().flush();
}

/// Delega: il worker riesce a creare un sotto-cgroup dentro il proprio?
fn prova_sottogruppo() {
    let Some(mio_dominio) = mio_dominio() else {
        println!("CARICO sottogruppo cgroup-non-risolto");
        return;
    };
    let figlio = mio_dominio.join("interno");
    match fs::create_dir(&figlio) {
        Ok(()) => {
            println!("CARICO sottogruppo creato {}", figlio.display());
            let _ = fs::remove_dir(&figlio);
        }
        Err(e) => println!("CARICO sottogruppo rifiutato {e}"),
    }
    let _ = std::io::stdout().flush();
}

// ---------------------------------------------------------------------------
// Probe di clone3 — l'unico `unsafe` del prototipo, ed e' il punto.
//
// Non forka: chiede al kernel due domande con argomenti volutamente invalidi,
// e distingue le risposte. ENOSYS = non c'e' (o e' filtrato da seccomp),
// EINVAL = c'e'. Per il flag: EBADF = il flag e' riconosciuto e il kernel e'
// arrivato a validare il descrittore; EINVAL = flag sconosciuto.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Default)]
struct ArgomentiClone3 {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
    set_tid: u64,
    set_tid_size: u64,
    cgroup: u64,
}

/// `include/uapi/linux/sched.h`: `CLONE_INTO_CGROUP 0x200000000ULL`.
/// E' un flag esclusivo di `clone3`, sopra i 32 bit dei flag storici.
const CLONE_INTO_CGROUP: u64 = 0x2_0000_0000;

fn probe_clone3() {
    // Domanda 1: la syscall esiste?
    let mut vuoti = ArgomentiClone3::default();
    // SAFETY: prototipo. size=0 e' invalido di proposito: il kernel torna
    // EINVAL prima di leggere la struttura, quindi nessun processo nasce.
    let esito = unsafe {
        libc::syscall(libc::SYS_clone3, std::ptr::from_mut(&mut vuoti), 0_usize)
    };
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    let disponibile = esito == -1 && errno == libc::EINVAL;
    println!(
        "MISURA clone3.presente {} (ritorno={esito} errno={errno})",
        if disponibile { "si" } else { "no" }
    );

    // Domanda 2: il flag CLONE_INTO_CGROUP e' riconosciuto?
    let mut con_flag = ArgomentiClone3 {
        flags: CLONE_INTO_CGROUP,
        exit_signal: libc::SIGCHLD as u64,
        // Descrittore mai aperto, ma <= INT_MAX: `copy_clone_args_from_user`
        // respinge con EINVAL qualunque `cgroup > INT_MAX` *prima* di provare
        // ad aprirlo, e un valore troppo grande maschererebbe la risposta.
        cgroup: 1_000_000,
        ..ArgomentiClone3::default()
    };
    let dimensione = std::mem::size_of::<ArgomentiClone3>();
    // SAFETY: prototipo. Il descrittore di cgroup e' invalido, quindi il
    // kernel fallisce con EBADF senza creare alcun processo.
    let esito = unsafe {
        libc::syscall(libc::SYS_clone3, std::ptr::from_mut(&mut con_flag), dimensione)
    };
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    let riconosciuto = esito == -1 && errno == libc::EBADF;
    println!(
        "MISURA clone3.into_cgroup {} (ritorno={esito} errno={errno})",
        if riconosciuto { "si" } else { "no" }
    );
    if esito >= 0 {
        println!("MISURA clone3.into_cgroup ANOMALIA: il probe ha creato un processo");
    }
}

// ---------------------------------------------------------------------------
// Supervisore: esegue gli scenari e stampa le misure.
// ---------------------------------------------------------------------------

struct Esito {
    codice: Option<i32>,
    segnale: Option<i32>,
    stdout: String,
}

/// Come `esegui_come`, ma con una scadenza: se il dominio non si spegne entro
/// il tempo dato, lo si uccide con `cgroup.kill` e lo si dichiara.
///
/// `cgroup.kill` manda `SIGKILL` a tutti i processi del cgroup e **non**
/// consulta `oom_score_adj`: e' la sola via d'uscita quando il worker e'
/// protetto dall'OOM killer.
fn esegui_con_scadenza(dominio: &Path, argomenti: &[&str], scadenza_ms: u64) -> (Esito, bool) {
    let mio = std::env::current_exe().expect("current_exe");
    let mut comando = Command::new(mio);
    comando
        .arg("spawner")
        .arg(dominio)
        .arg("0")
        .args(argomenti)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut figlio = comando.spawn().expect("spawn dello spawner");

    let mut atteso = 0;
    let mut ucciso = false;
    loop {
        match figlio.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => break,
        }
        if atteso >= scadenza_ms {
            let _ = fs::write(dominio.join("cgroup.kill"), "1");
            ucciso = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
        atteso += 25;
    }
    let uscita = figlio.wait_with_output().expect("raccolta dell'uscita");
    let stderr = String::from_utf8_lossy(&uscita.stderr);
    for riga in stderr.lines() {
        println!("    | {riga}");
    }
    let stdout = String::from_utf8_lossy(&uscita.stdout).into_owned();
    for riga in stdout.lines() {
        println!("    > {riga}");
    }
    (
        Esito { codice: uscita.status.code(), segnale: uscita.status.signal(), stdout },
        ucciso,
    )
}

fn esegui_nel_dominio(dominio: &Path, argomenti: &[&str]) -> Esito {
    esegui_come(dominio, 0, argomenti)
}

fn esegui_come(dominio: &Path, utente: u32, argomenti: &[&str]) -> Esito {
    let mio = std::env::current_exe().expect("current_exe");
    let mut comando = Command::new(mio);
    comando.arg("spawner").arg(dominio).arg(utente.to_string()).args(argomenti);
    let uscita = comando.output().expect("esecuzione dello spawner");
    let stderr = String::from_utf8_lossy(&uscita.stderr);
    for riga in stderr.lines() {
        println!("    | {riga}");
    }
    let stdout = String::from_utf8_lossy(&uscita.stdout).into_owned();
    for riga in stdout.lines() {
        println!("    > {riga}");
    }
    Esito { codice: uscita.status.code(), segnale: uscita.status.signal(), stdout }
}

fn stampa_stato(dominio: &Path, etichetta: &str) {
    for file in ["memory.max", "memory.current", "memory.peak"] {
        if let Some(v) = leggi_intero(dominio, file) {
            println!("MISURA {etichetta}.{file} {v}");
        }
    }
    for (chiave, valore) in eventi_locali(dominio) {
        println!("MISURA {etichetta}.events.local.{chiave} {valore}");
    }
}


// ---------------------------------------------------------------------------
// Secondo ciclo: una funzione per ciascuna domanda del prototipo.
// ---------------------------------------------------------------------------

/// L7 — il dominio puo' essere sigillato dal kernel come foglia?
fn l7_foglia_sigillata(base: &Path) {
    titolo("L7 — dominio sigillato con cgroup.max.depth=0");
    match crea_dominio(base, "l7-sigillato", 256 * 1024 * 1024, false, Foglia::ProfonditaZero) {
        Ok(dominio) => {
            println!("MISURA l7.max_depth {}", leggi(&dominio.join("cgroup.max.depth")));
            let esito = esegui_nel_dominio(&dominio, &["sottogruppo"]);
            let creato = esito.stdout.contains("sottogruppo creato");
            println!(
                "MISURA l7.worker_puo_creare_sottogruppo {}",
                if creato { "si — LA FOGLIA NON TIENE" } else { "no" }
            );
            for riga in esito.stdout.lines().filter(|r| r.contains("sottogruppo rifiutato")) {
                println!("MISURA l7.rifiuto {}", riga.trim_start_matches("CARICO sottogruppo rifiutato "));
            }
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l7.errore {e}"),
    }
}

/// L8 — la domanda centrale: un OOM dentro un sottogruppo e' visibile?
fn l8_oom_nel_sottogruppo(base: &Path) {
    titolo("L8 — OOM dentro un sottogruppo: local contro gerarchico");
    match crea_dominio(base, "l8-sottogruppo", 64 * 1024 * 1024, false, Foglia::Aperta) {
        Ok(dominio) => {
            let locali_prima = eventi_locali(&dominio);
            let gerarchici_prima = eventi_gerarchici(&dominio);
            let esito = esegui_nel_dominio(&dominio, &["sottogruppo_oom"]);

            println!("MISURA l8.codice {:?}", esito.codice);
            println!("MISURA l8.segnale {:?}", esito.segnale);
            for chiave in ["mkdir-riuscito", "mkdir-rifiutato", "controller-delegato"] {
                for riga in esito.stdout.lines().filter(|r| r.contains(chiave)) {
                    println!("MISURA l8.{}", riga.trim_start_matches("CARICO sottogruppo_oom "));
                }
            }
            let figlio_ucciso = esito.stdout.contains("segnale=Some(9)");
            println!("MISURA l8.figlio_ucciso_nel_sottogruppo {}", if figlio_ucciso { "si" } else { "no" });

            let locali_dopo = eventi_locali(&dominio);
            let gerarchici_dopo = eventi_gerarchici(&dominio);
            for chiave in ["max", "oom", "oom_kill"] {
                let dl = locali_dopo.get(chiave).copied().unwrap_or(0)
                    - locali_prima.get(chiave).copied().unwrap_or(0);
                let dg = gerarchici_dopo.get(chiave).copied().unwrap_or(0)
                    - gerarchici_prima.get(chiave).copied().unwrap_or(0);
                println!("MISURA l8.delta.local.{chiave} {dl}");
                println!("MISURA l8.delta.gerarchico.{chiave} {dg}");
            }
            let cieco = locali_dopo.get("oom_kill").copied().unwrap_or(0)
                == locali_prima.get("oom_kill").copied().unwrap_or(0)
                && gerarchici_dopo.get("oom_kill").copied().unwrap_or(0)
                    > gerarchici_prima.get("oom_kill").copied().unwrap_or(0);
            println!(
                "MISURA l8.local_e_cieco {}",
                if cieco { "SI — evidenza persa se si legge solo local" } else { "no" }
            );

            let _ = fs::remove_dir(dominio.join("interno"));
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l8.errore {e}"),
    }
}

/// L12 — la via non ingenua: spostarsi prima, delegare poi.
fn l12_evasione(base: &Path) {
    titolo("L12 — evasione in tre passi dal cgroup osservato");
    match crea_dominio(base, "l12-evasione", 64 * 1024 * 1024, false, Foglia::Aperta) {
        Ok(dominio) => {
            let locali_prima = eventi_locali(&dominio);
            let gerarchici_prima = eventi_gerarchici(&dominio);
            let esito = esegui_nel_dominio(&dominio, &["evasione"]);
            println!("MISURA l12.codice {:?}", esito.codice);
            println!("MISURA l12.segnale {:?}", esito.segnale);
            for passo in ["passo1-mkdir", "passo2-spostamento", "passo3-delega"] {
                for riga in esito.stdout.lines().filter(|r| r.contains(passo)) {
                    println!("MISURA l12.{}", riga.trim_start_matches("CARICO evasione "));
                }
            }
            let evaso = esito.stdout.contains("passo3-delega riuscita");
            println!("MISURA l12.evasione_riuscita {}", if evaso { "SI" } else { "no" });

            let locali_dopo = eventi_locali(&dominio);
            let gerarchici_dopo = eventi_gerarchici(&dominio);
            for chiave in ["max", "oom", "oom_kill"] {
                let dl = locali_dopo.get(chiave).copied().unwrap_or(0)
                    - locali_prima.get(chiave).copied().unwrap_or(0);
                let dg = gerarchici_dopo.get(chiave).copied().unwrap_or(0)
                    - gerarchici_prima.get(chiave).copied().unwrap_or(0);
                println!("MISURA l12.delta.local.{chiave} {dl}");
                println!("MISURA l12.delta.gerarchico.{chiave} {dg}");
            }
            let perso = locali_dopo.get("oom_kill").copied().unwrap_or(0)
                == locali_prima.get("oom_kill").copied().unwrap_or(0)
                && gerarchici_dopo.get("oom_kill").copied().unwrap_or(0)
                    > gerarchici_prima.get("oom_kill").copied().unwrap_or(0);
            println!(
                "MISURA l12.local_e_cieco {}",
                if perso { "SI — leggere solo local perde l'evidenza" } else { "no" }
            );

            let _ = fs::remove_dir(dominio.join("evaso"));
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l12.errore {e}"),
    }
}

/// L13 — la stessa evasione, ma contro un dominio sigillato.
fn l13_evasione_contro_foglia(base: &Path) {
    titolo("L13 — la stessa evasione contro cgroup.max.depth=0");
    match crea_dominio(base, "l13-sigillato", 64 * 1024 * 1024, true, Foglia::ProfonditaZero) {
        Ok(dominio) => {
            let locali_prima = eventi_locali(&dominio);
            let esito = esegui_nel_dominio(&dominio, &["evasione"]);
            println!("MISURA l13.codice {:?}", esito.codice);
            println!("MISURA l13.segnale {:?}", esito.segnale);
            for riga in esito.stdout.lines().filter(|r| r.contains("evasione passo")) {
                println!("MISURA l13.{}", riga.trim_start_matches("CARICO evasione "));
            }
            let fermata = esito.stdout.contains("passo1-mkdir rifiutato");
            println!(
                "MISURA l13.evasione_fermata_al_primo_passo {}",
                if fermata { "si" } else { "NO" }
            );
            let locali_dopo = eventi_locali(&dominio);
            for chiave in ["oom", "oom_kill", "oom_group_kill"] {
                let dl = locali_dopo.get(chiave).copied().unwrap_or(0)
                    - locali_prima.get(chiave).copied().unwrap_or(0);
                println!("MISURA l13.delta.local.{chiave} {dl}");
            }
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l13.errore {e}"),
    }
}

/// L9 — con i privilegi ceduti, il worker riesce ancora a toccare il dominio?
fn l9_senza_privilegi(base: &Path) {
    titolo("L9 — worker non privilegiato (uid 1000)");
    match crea_dominio(base, "l9-nonpriv", 256 * 1024 * 1024, false, Foglia::Aperta) {
        Ok(dominio) => {
            let esito = esegui_come(&dominio, 1000, &["sottogruppo"]);
            println!("MISURA l9.codice {:?}", esito.codice);
            let nato = esito.stdout.contains("CARICO cgroup");
            println!("MISURA l9.worker_e_partito {}", if nato { "si" } else { "no" });
            let creato = esito.stdout.contains("sottogruppo creato");
            println!(
                "MISURA l9.puo_creare_sottogruppo {}",
                if creato { "si — I PERMESSI NON BASTANO" } else { "no" }
            );
            for riga in esito.stdout.lines().filter(|r| r.contains("sottogruppo rifiutato")) {
                println!("MISURA l9.rifiuto {}", riga.trim_start_matches("CARICO sottogruppo rifiutato "));
            }
            let _ = fs::remove_dir(dominio.join("interno"));
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l9.errore {e}"),
    }
}

/// L10 — al ritorno della wait il dominio e' quiescente? E un OOM puo'
/// arrivare dopo?
fn l10_quiescenza(base: &Path) {
    titolo("L10 — quiescenza: il capofila esce 0 e lascia un orfano vivo");
    match crea_dominio(base, "l10-orfano", 64 * 1024 * 1024, false, Foglia::Aperta) {
        Ok(dominio) => {
            let prima = eventi_gerarchici(&dominio);
            let esito = esegui_nel_dominio(&dominio, &["orfano"]);
            // Subito dopo che il capofila e' uscito: che cosa vede il
            // supervisore se pubblicasse adesso?
            let vivi_subito = processi_vivi(&dominio);
            let eventi_subito = eventi_gerarchici(&dominio);
            println!("MISURA l10.capofila_codice {:?}", esito.codice);
            println!("MISURA l10.processi_vivi_alla_wait {vivi_subito}");
            let oom_subito = eventi_subito.get("oom_kill").copied().unwrap_or(0)
                - prima.get("oom_kill").copied().unwrap_or(0);
            println!("MISURA l10.oom_kill_visibile_alla_wait {oom_subito}");

            // Ora si aspetta la quiescenza vera, come dovrebbe fare la
            // barriera prima della pubblicazione.
            let mut giri = 0;
            while processi_vivi(&dominio) > 0 && giri < 600 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                giri += 1;
            }
            let dopo = eventi_gerarchici(&dominio);
            let oom_dopo = dopo.get("oom_kill").copied().unwrap_or(0)
                - prima.get("oom_kill").copied().unwrap_or(0);
            println!("MISURA l10.attesa_quiescenza_ms {}", giri * 50);
            println!("MISURA l10.processi_vivi_dopo {}", processi_vivi(&dominio));
            println!("MISURA l10.oom_kill_dopo_la_quiescenza {oom_dopo}");
            println!(
                "MISURA l10.evidenza_arrivata_in_ritardo {}",
                if oom_dopo > oom_subito { "SI — pubblicare alla wait sarebbe stato un errore" } else { "no" }
            );
            stampa_stato(&dominio, "l10");
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l10.errore {e}"),
    }
}

/// L11 — ripetizioni: un campione non e' una misura.
fn l11_ripetizioni(base: &Path, quante: u32) {
    titolo("L11 — ripetizioni del costo d'avvio e del contenimento");
    let mut picchi_avvio = Vec::new();
    let mut picchi_tetto = Vec::new();
    for giro in 0..quante {
        if let Ok(dominio) = crea_dominio(
            base,
            &format!("l11-avvio-{giro}"),
            256 * 1024 * 1024,
            false,
            Foglia::Aperta,
        ) {
            let _ = esegui_nel_dominio(&dominio, &["inerte"]);
            if let Some(v) = leggi_intero(&dominio, "memory.peak") {
                picchi_avvio.push(v);
            }
            rimuovi_dominio(&dominio);
        }
        if let Ok(dominio) = crea_dominio(
            base,
            &format!("l11-tetto-{giro}"),
            64 * 1024 * 1024,
            true,
            Foglia::Aperta,
        ) {
            let _ = esegui_nel_dominio(&dominio, &["tocca"]);
            if let Some(v) = leggi_intero(&dominio, "memory.peak") {
                picchi_tetto.push(v);
            }
            rimuovi_dominio(&dominio);
        }
    }
    riporta_intervallo("l11.picco_avvio", &picchi_avvio);
    riporta_intervallo("l11.picco_sotto_tetto", &picchi_tetto);
}

fn riporta_intervallo(chiave: &str, valori: &[u64]) {
    if valori.is_empty() {
        println!("MISURA {chiave} nessuna misura");
        return;
    }
    let minimo = valori.iter().copied().min().unwrap_or(0);
    let massimo = valori.iter().copied().max().unwrap_or(0);
    println!(
        "MISURA {chiave} n={} min={minimo} max={massimo} valori={valori:?}",
        valori.len()
    );
}


/// L14 — quiescenza con un orfano nello **stesso** cgroup.
///
/// Qui `cgroup.procs` e `populated` devono concordare: e' il caso facile, e
/// serve come controllo per L15.
fn l14_quiescenza_stesso_cgroup(base: &Path) {
    titolo("L14 — quiescenza letta da cgroup.events, orfano nello stesso cgroup");
    match crea_dominio(base, "l14-orfano", 64 * 1024 * 1024, false, Foglia::Aperta) {
        Ok(dominio) => {
            let esito = esegui_nel_dominio(&dominio, &["orfano"]);
            println!("MISURA l14.capofila_codice {:?}", esito.codice);

            let procs = fs::read_to_string(dominio.join("cgroup.procs"))
                .map(|t| t.split_whitespace().count())
                .unwrap_or(0);
            let pop = popolato(&dominio);
            println!("MISURA l14.cgroup_procs_alla_wait {procs}");
            println!("MISURA l14.populated_alla_wait {pop:?}");
            println!(
                "MISURA l14.concordano {}",
                if (procs > 0) == pop.unwrap_or(false) { "si" } else { "NO" }
            );

            let mut giri = 0;
            while popolato(&dominio) == Some(true) && giri < 600 {
                std::thread::sleep(std::time::Duration::from_millis(25));
                giri += 1;
            }
            println!("MISURA l14.attesa_populated_zero_ms {}", giri * 25);
            println!("MISURA l14.populated_finale {:?}", popolato(&dominio));
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l14.errore {e}"),
    }
}

/// L15 — quiescenza con un processo vivo in un **sottogruppo**.
///
/// E' il caso che smaschera `cgroup.procs`: il file elenca solo il cgroup
/// corrente, quindi al ritorno della `wait` risulta vuoto mentre un processo
/// del dominio e' vivo un livello piu' sotto.
fn l15_quiescenza_sottogruppo(base: &Path) {
    titolo("L15 — quiescenza con un processo vivo in un sottogruppo");
    match crea_dominio(base, "l15-figlio", 64 * 1024 * 1024, false, Foglia::Aperta) {
        Ok(dominio) => {
            let esito = esegui_nel_dominio(&dominio, &["sottogruppo_vivo"]);
            println!("MISURA l15.capofila_codice {:?}", esito.codice);

            let procs = fs::read_to_string(dominio.join("cgroup.procs"))
                .map(|t| t.split_whitespace().count())
                .unwrap_or(0);
            let ricorsivi = processi_vivi(&dominio);
            let pop = popolato(&dominio);
            for riga in esito.stdout.lines().filter(|r| r.contains("nipote-insediato")) {
                println!(
                    "MISURA l15.{}",
                    riga.trim_start_matches("CARICO sottogruppo_vivo ")
                );
            }
            let valida = esito.stdout.contains("nipote-insediato si");
            println!(
                "MISURA l15.misura_valida {}",
                if valida { "si" } else { "NO — il nipote non si e' insediato" }
            );
            println!("MISURA l15.cgroup_procs_alla_wait {procs}");
            println!("MISURA l15.scansione_ricorsiva_alla_wait {ricorsivi}");
            println!("MISURA l15.populated_alla_wait {pop:?}");
            println!(
                "MISURA l15.cgroup_procs_e_cieco {}",
                if procs == 0 && pop == Some(true) {
                    "SI — dominio vuoto secondo cgroup.procs, popolato secondo il kernel"
                } else {
                    "no"
                }
            );

            let mut giri = 0;
            while popolato(&dominio) == Some(true) && giri < 600 {
                std::thread::sleep(std::time::Duration::from_millis(25));
                giri += 1;
            }
            println!("MISURA l15.attesa_populated_zero_ms {}", giri * 25);
            println!("MISURA l15.populated_finale {:?}", popolato(&dominio));
            let _ = fs::remove_dir(dominio.join("vivo"));
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l15.errore {e}"),
    }
}

/// L16 — `oom_score_adj = -1000` ereditato batte `memory.oom.group=1`?
///
/// Due bracci: senza normalizzazione (controllo) e con.
fn l16_oom_score_adj(base: &Path) {
    titolo("L16 — eredita' di oom_score_adj contro memory.oom.group");

    // Il supervisore si protegge: rappresenta un chiamante che gira sotto un
    // servizio protetto dall'OOM killer.
    match fs::write("/proc/self/oom_score_adj", "-1000") {
        Ok(()) => println!("MISURA l16.supervisore_protetto si"),
        Err(e) => {
            println!("MISURA l16.supervisore_protetto no ({e}) — scenario non eseguibile");
            return;
        }
    }
    println!(
        "MISURA l16.supervisore_oom_score_adj {}",
        leggi(Path::new("/proc/self/oom_score_adj"))
    );

    for (nome, normalizza) in [("l16a-senza", false), ("l16b-con", true)] {
        let etichetta = if normalizza { "con_normalizzazione" } else { "senza_normalizzazione" };
        match crea_dominio(base, nome, 64 * 1024 * 1024, true, Foglia::ProfonditaZero) {
            Ok(dominio) => {
                if normalizza {
                    std::env::remove_var("PT_NON_NORMALIZZARE");
                } else {
                    std::env::set_var("PT_NON_NORMALIZZARE", "1");
                }
                let prima = eventi_gerarchici(&dominio);
                let (esito, ucciso) = esegui_con_scadenza(&dominio, &["tocca"], 4_000);
                println!(
                    "MISURA l16.{etichetta}.ha_richiesto_cgroup_kill {}",
                    if ucciso { "SI — il dominio non si e' spento da solo" } else { "no" }
                );
                println!("MISURA l16.{etichetta}.codice {:?}", esito.codice);
                println!("MISURA l16.{etichetta}.segnale {:?}", esito.segnale);
                for riga in esito.stdout.lines().filter(|r| r.starts_with("CARICO oom_score_adj")) {
                    println!(
                        "MISURA l16.{etichetta}.worker_oom_score_adj {}",
                        riga.trim_start_matches("CARICO oom_score_adj ")
                    );
                }
                println!(
                    "MISURA l16.{etichetta}.verdetto {}",
                    match (normalizza, ucciso) {
                        (false, true) => "il worker protetto ha resistito al group kill",
                        (false, false) => "il worker protetto e' stato fermato comunque",
                        (true, true) => "ANOMALIA: normalizzato e non fermato dal group kill",
                        (true, false) => "normalizzato e fermato dal group kill, come atteso",
                    }
                );
                let dopo = eventi_gerarchici(&dominio);
                for chiave in ["oom", "oom_kill", "oom_group_kill"] {
                    let d = dopo.get(chiave).copied().unwrap_or(0)
                        - prima.get(chiave).copied().unwrap_or(0);
                    println!("MISURA l16.{etichetta}.delta.{chiave} {d}");
                }
                if let Some(v) = leggi_intero(&dominio, "memory.peak") {
                    println!("MISURA l16.{etichetta}.memory.peak {v}");
                }
                rimuovi_dominio(&dominio);
            }
            Err(e) => println!("MISURA l16.{etichetta}.errore {e}"),
        }
    }
    std::env::remove_var("PT_NON_NORMALIZZARE");
    let _ = fs::write("/proc/self/oom_score_adj", "0");
}

/// L17 — il preflight: cio' che il supervisore deve verificare prima di
/// dichiarare utilizzabile un dominio.
fn l17_preflight(base: &Path) {
    titolo("L17 — preflight del dominio: scrivi, poi rileggi");
    match crea_dominio(base, "l17-preflight", 64 * 1024 * 1024, true, Foglia::ProfonditaZero) {
        Ok(dominio) => {
            for (chiave, atteso) in [
                ("memory.max", "67108864"),
                ("memory.oom.group", "1"),
                ("cgroup.max.depth", "0"),
                ("memory.swap.max", "0"),
            ] {
                let letto = leggi(&dominio.join(chiave));
                println!(
                    "MISURA l17.{chiave} letto={letto} atteso={atteso} {}",
                    if letto == atteso { "OK" } else { "DIVERGE" }
                );
            }
            println!("MISURA l17.cgroup.events {:?}", popolato(&dominio));
            let opzioni = leggi(Path::new("/proc/self/mounts"));
            let localevents = opzioni
                .lines()
                .filter(|r| r.contains(" cgroup2 "))
                .any(|r| r.contains("memory_localevents"));
            println!(
                "MISURA l17.memory_localevents {}",
                if localevents { "PRESENTE — memory.events non e' gerarchico" } else { "assente" }
            );
            for riga in opzioni.lines().filter(|r| r.contains(" cgroup2 ")) {
                println!("MISURA l17.mount {riga}");
            }
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA l17.errore {e}"),
    }
}


/// L18 — un antenato con il tetto piu' basso uccide il worker.
///
/// Il dominio non raggiunge mai il PROPRIO limite: quello che si esaurisce e'
/// il tetto del padre. `Ol` resta a zero, `Kl` sale.
fn l18_oom_dell_antenato(base: &Path) {
    titolo("L18 — OOM di un antenato: Ol=0 con Kl>=1");

    let padre = base.join("l18-padre");
    if padre.exists() {
        let _ = fs::remove_dir_all(&padre);
    }
    if let Err(e) = fs::create_dir(&padre) {
        println!("MISURA l18.errore mkdir-padre {e}");
        return;
    }
    // Il padre stringe, il figlio no.
    if let Err(e) = fs::write(padre.join("memory.max"), (32 * 1024 * 1024).to_string()) {
        println!("MISURA l18.errore memory.max-padre {e}");
        return;
    }
    let _ = fs::write(padre.join("memory.swap.max"), "0");
    if let Err(e) = abilita_memoria(&padre) {
        println!("MISURA l18.errore delega-padre {e}");
        return;
    }
    let dominio = padre.join("dominio");
    if let Err(e) = fs::create_dir(&dominio) {
        println!("MISURA l18.errore mkdir-dominio {e}");
        return;
    }
    // Tetto del dominio DELIBERATAMENTE piu' alto: non sara' lui a scattare.
    let _ = fs::write(dominio.join("memory.max"), (256 * 1024 * 1024).to_string());
    let _ = fs::write(dominio.join("memory.swap.max"), "0");
    println!("MISURA l18.tetto_padre {}", leggi(&padre.join("memory.max")));
    println!("MISURA l18.tetto_dominio {}", leggi(&dominio.join("memory.max")));

    let locali_prima = eventi_locali(&dominio);
    let gerarchici_prima = eventi_gerarchici(&dominio);
    let padre_prima = eventi_locali(&padre);

    let esito = esegui_nel_dominio(&dominio, &["tocca"]);
    println!("MISURA l18.codice {:?}", esito.codice);
    println!("MISURA l18.segnale {:?}", esito.segnale);

    let locali_dopo = eventi_locali(&dominio);
    let gerarchici_dopo = eventi_gerarchici(&dominio);
    let padre_dopo = eventi_locali(&padre);

    let delta = |prima: &BTreeMap<String, u64>, dopo: &BTreeMap<String, u64>, k: &str| {
        dopo.get(k).copied().unwrap_or(0) - prima.get(k).copied().unwrap_or(0)
    };
    let ol = delta(&locali_prima, &locali_dopo, "oom");
    let kl = delta(&locali_prima, &locali_dopo, "oom_kill");
    let kh = delta(&gerarchici_prima, &gerarchici_dopo, "oom_kill");
    println!("MISURA l18.dominio.Ol {ol}");
    println!("MISURA l18.dominio.Kl {kl}");
    println!("MISURA l18.dominio.Kh {kh}");
    println!("MISURA l18.padre.oom {}", delta(&padre_prima, &padre_dopo, "oom"));
    println!("MISURA l18.padre.oom_kill {}", delta(&padre_prima, &padre_dopo, "oom_kill"));
    println!(
        "MISURA l18.combinazione_scoperta {}",
        if ol == 0 && kl >= 1 {
            "SI — Ol=0 con Kl>=1: la matrice non la copriva"
        } else {
            "no"
        }
    );
    // Il picco DEL PADRE e' la misura diretta del superamento temporaneo: il
    // dominio e' contenuto in lui, quindi dedurlo dal picco del figlio
    // sarebbe un'inferenza dove si puo' avere un'osservazione.
    let tetto_padre = leggi_intero(&padre, "memory.max").unwrap_or(0);
    for (chi, dove) in [("dominio", &dominio), ("padre", &padre)] {
        if let Some(v) = leggi_intero(dove, "memory.peak") {
            println!("MISURA l18.{chi}.memory.peak {v}");
            if chi == "padre" && tetto_padre > 0 {
                println!(
                    "MISURA l18.padre.superamento_del_tetto {} byte",
                    i128::from(v) - i128::from(tetto_padre)
                );
                println!(
                    "MISURA l18.superamento_temporaneo_osservato {}",
                    if v > tetto_padre { "SI" } else { "no" }
                );
            }
        }
    }

    let _ = fs::remove_dir(&dominio);
    let _ = fs::remove_dir(&padre);
}

/// L19 — il worker disfa il preflight, con gli stessi privilegi.
fn l19_riscrittura_post_preflight(base: &Path) {
    titolo("L19 — il worker riscrive cio' che il preflight ha stabilito");

    for (nome, utente) in [("l19a-stesso-uid", 0_u32), ("l19b-uid-diverso", 1000)] {
        let etichetta = if utente == 0 { "stesso_uid" } else { "uid_diverso" };
        match crea_dominio(base, nome, 64 * 1024 * 1024, true, Foglia::ProfonditaZero) {
            Ok(dominio) => {
                let esito = esegui_come(&dominio, utente, &["riscrivi"]);
                println!("MISURA l19.{etichetta}.codice {:?}", esito.codice);
                for riga in esito.stdout.lines().filter(|r| r.starts_with("CARICO riscrivi ")) {
                    println!("MISURA l19.{etichetta}.{}", riga.trim_start_matches("CARICO riscrivi "));
                }
                let riuscite = esito.stdout.matches("RIUSCITA").count();
                println!("MISURA l19.{etichetta}.riscritture_riuscite {riuscite}");
                println!(
                    "MISURA l19.{etichetta}.preflight_resiste {}",
                    if riuscite == 0 { "si" } else { "NO — il sigillo si disfa dall'interno" }
                );
                // Che cosa vale ADESSO, dopo il passaggio del worker.
                for chiave in ["cgroup.max.depth", "memory.max", "memory.oom.group"] {
                    println!(
                        "MISURA l19.{etichetta}.dopo.{chiave} {}",
                        leggi(&dominio.join(chiave))
                    );
                }
                rimuovi_dominio(&dominio);
            }
            Err(e) => println!("MISURA l19.{etichetta}.errore {e}"),
        }
    }
}

fn titolo(testo: &str) {
    println!("\n=== {testo} ===");
}

#[allow(clippy::too_many_lines)] // e' un registro di misure, non una funzione di libreria
fn supervisore() {
    println!("PT-Linux — prototipo bloccante fase 4");
    println!("MISURA kernel {}", leggi(Path::new("/proc/sys/kernel/osrelease")));
    println!("MISURA cgroup.controllers {}", leggi(&PathBuf::from(RADICE).join("cgroup.controllers")));
    println!("MISURA uid {}", leggi(Path::new("/proc/self/uid_map")).replace('\n', " | "));

    titolo("S0 — disponibilita' di clone3 e del flag");
    probe_clone3();

    titolo("S0-bis — delega della gerarchia");
    let base = match prepara_gerarchia() {
        Ok(base) => {
            println!("MISURA gerarchia.delegabile si");
            base
        }
        Err(e) => {
            println!("MISURA gerarchia.delegabile no ({e})");
            println!("
Nessuno scenario e' eseguibile senza delega. Nessun ripiego.");
            return;
        }
    };

    titolo("S1 — nascita gia' vincolata (tetto molto basso)");
    // Se anche il caricamento dell'immagine e' addebitato al dominio, un tetto
    // sotto la dimensione del binario deve impedire persino l'avvio.
    match crea_dominio_aperto(&base, "s1-stretto", 2 * 1024 * 1024, false) {
        Ok(dominio) => {
            let esito = esegui_nel_dominio(&dominio, &["chiedi"]);
            println!("MISURA s1.codice {:?}", esito.codice);
            println!("MISURA s1.segnale {:?}", esito.segnale);
            println!(
                "MISURA s1.worker_ha_parlato {}",
                if esito.stdout.contains("CARICO") { "si" } else { "no" }
            );
            stampa_stato(&dominio, "s1");
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA s1.errore {e}"),
    }

    titolo("S1-bis — tetto sotto il costo minimo di un processo");
    match crea_dominio_aperto(&base, "s1bis-infimo", 128 * 1024, false) {
        Ok(dominio) => {
            let esito = esegui_nel_dominio(&dominio, &["chiedi"]);
            println!("MISURA s1bis.codice {:?}", esito.codice);
            println!("MISURA s1bis.segnale {:?}", esito.segnale);
            println!(
                "MISURA s1bis.worker_ha_parlato {}",
                if esito.stdout.contains("CARICO") { "si" } else { "no" }
            );
            stampa_stato(&dominio, "s1bis");
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA s1bis.errore {e}"),
    }

    titolo("S2 — il worker nasce dentro il dominio (tetto ampio)");
    match crea_dominio_aperto(&base, "s2-nascita", 256 * 1024 * 1024, false) {
        Ok(dominio) => {
            let atteso = format!("{PREFISSO}/s2-nascita");
            let esito = esegui_nel_dominio(&dominio, &["sottogruppo"]);
            let dentro = esito.stdout.lines().any(|r| r.starts_with("CARICO cgroup") && r.contains(&atteso));
            println!("MISURA s2.nato_dentro {}", if dentro { "si" } else { "no" });
            let delega = esito.stdout.contains("sottogruppo creato");
            println!("MISURA s2.puo_creare_sottogruppo {}", if delega { "si" } else { "no" });
            stampa_stato(&dominio, "s2");
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA s2.errore {e}"),
    }

    titolo("S3 — allocazione richiesta ma non toccata");
    match crea_dominio_aperto(&base, "s3-chiedi", 64 * 1024 * 1024, false) {
        Ok(dominio) => {
            let esito = esegui_nel_dominio(&dominio, &["chiedi"]);
            println!("MISURA s3.codice {:?}", esito.codice);
            println!("MISURA s3.segnale {:?}", esito.segnale);
            for riga in esito.stdout.lines().filter(|r| r.starts_with("CARICO richiesta")) {
                println!("MISURA s3.{}", riga.trim_start_matches("CARICO richiesta ").replace(' ', " -> "));
            }
            stampa_stato(&dominio, "s3");
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA s3.errore {e}"),
    }

    titolo("S4 — contenimento: allocazione toccata oltre il tetto");
    match crea_dominio_aperto(&base, "s4-tocca", 64 * 1024 * 1024, false) {
        Ok(dominio) => {
            let prima = eventi_locali(&dominio);
            let esito = esegui_nel_dominio(&dominio, &["tocca"]);
            println!("MISURA s4.codice {:?}", esito.codice);
            println!("MISURA s4.segnale {:?}", esito.segnale);
            let negata = esito.stdout.contains("negata-a");
            println!("MISURA s4.allocazione_negata {}", if negata { "si" } else { "no" });
            println!(
                "MISURA s4.terminato_da_segnale {}",
                if esito.segnale.is_some() { "si" } else { "no" }
            );
            let dopo = eventi_locali(&dominio);
            for chiave in ["max", "oom", "oom_kill", "oom_group_kill"] {
                let d = dopo.get(chiave).copied().unwrap_or(0)
                    - prima.get(chiave).copied().unwrap_or(0);
                println!("MISURA s4.delta.{chiave} {d}");
            }
            stampa_stato(&dominio, "s4");
            rimuovi_dominio(&dominio);
        }
        Err(e) => println!("MISURA s4.errore {e}"),
    }

    for (nome, oom_group) in [("s5-discendenti", false), ("s6-oom-group", true)] {
        titolo(&format!("{nome} — discendenti, memory.oom.group={}", u8::from(oom_group)));
        match crea_dominio_aperto(&base, nome, 64 * 1024 * 1024, oom_group) {
            Ok(dominio) => {
                let prima = eventi_locali(&dominio);
                let esito = esegui_nel_dominio(&dominio, &["figlio"]);
                println!("MISURA {nome}.codice {:?}", esito.codice);
                println!("MISURA {nome}.segnale {:?}", esito.segnale);
                let padre_vivo = esito.stdout.contains("padre-sopravvissuto si");
                println!(
                    "MISURA {nome}.padre_sopravvissuto {}",
                    if padre_vivo { "si" } else { "no" }
                );
                for riga in esito.stdout.lines().filter(|r| r.starts_with("CARICO figlio-stato")) {
                    println!("MISURA {nome}.figlio {}", riga.trim_start_matches("CARICO figlio-stato "));
                }
                let dopo = eventi_locali(&dominio);
                for chiave in ["max", "oom", "oom_kill", "oom_group_kill"] {
                    let d = dopo.get(chiave).copied().unwrap_or(0)
                        - prima.get(chiave).copied().unwrap_or(0);
                    println!("MISURA {nome}.delta.{chiave} {d}");
                }
                stampa_stato(&dominio, nome);
                rimuovi_dominio(&dominio);
            }
            Err(e) => println!("MISURA {nome}.errore {e}"),
        }
    }

    l7_foglia_sigillata(&base);
    l8_oom_nel_sottogruppo(&base);
    l9_senza_privilegi(&base);
    l12_evasione(&base);
    l13_evasione_contro_foglia(&base);
    l10_quiescenza(&base);
    l11_ripetizioni(&base, 5);
    l14_quiescenza_stesso_cgroup(&base);
    l15_quiescenza_sottogruppo(&base);
    l16_oom_score_adj(&base);
    l17_preflight(&base);
    l18_oom_dell_antenato(&base);
    l19_riscrittura_post_preflight(&base);

    let _ = fs::remove_dir(base.join("supervisore"));
    let _ = fs::remove_dir(&base);
    println!("\n=== FINE PT-Linux ===");
}
