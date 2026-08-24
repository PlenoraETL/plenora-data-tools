//! PT-Linux — prototipo bloccante della fase 4.
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
fn crea_dominio(base: &Path, nome: &str, tetto: u64, oom_group: bool) -> Result<PathBuf, String> {
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
    Ok(dominio)
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
    let pid = std::process::id();
    if let Err(e) = fs::write(dominio.join("cgroup.procs"), pid.to_string()) {
        eprintln!("SPAWNER errore cgroup.procs: {e}");
        std::process::exit(101);
    }
    // Verifica di essere entrato prima di sostituire l'immagine.
    let mio = leggi(Path::new("/proc/self/cgroup"));
    eprintln!("SPAWNER cgroup-prima-di-exec {mio}");

    let errore = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("carico")
        .args(&resto[1..])
        .exec();
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
    let _ = std::io::stdout().flush();

    match scenario {
        "chiedi" => chiedi_senza_toccare(),
        "figlio" => genera_figlio(&resto[1..]),
        "sottogruppo" => prova_sottogruppo(),
        _ => tocca_finche_muore(),
    }
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
    let mio = leggi(Path::new("/proc/self/cgroup"));
    let relativo = mio.rsplit("::").next().unwrap_or("").trim();
    let relativo = relativo.strip_prefix('/').unwrap_or(relativo);
    let mio_dominio = PathBuf::from(RADICE).join(relativo);
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

fn esegui_nel_dominio(dominio: &Path, argomenti: &[&str]) -> Esito {
    let mio = std::env::current_exe().expect("current_exe");
    let mut comando = Command::new(mio);
    comando.arg("spawner").arg(dominio).args(argomenti);
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
    match crea_dominio(&base, "s1-stretto", 2 * 1024 * 1024, false) {
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
    match crea_dominio(&base, "s1bis-infimo", 128 * 1024, false) {
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
    match crea_dominio(&base, "s2-nascita", 256 * 1024 * 1024, false) {
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
    match crea_dominio(&base, "s3-chiedi", 64 * 1024 * 1024, false) {
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
    match crea_dominio(&base, "s4-tocca", 64 * 1024 * 1024, false) {
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
        match crea_dominio(&base, nome, 64 * 1024 * 1024, oom_group) {
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

    let _ = fs::remove_dir(base.join("supervisore"));
    let _ = fs::remove_dir(&base);
    println!("\n=== FINE PT-Linux ===");
}
