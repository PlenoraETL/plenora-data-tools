//! PT-Windows — prototipo bloccante della fase 4.
//!
//! Stesse cinque domande di PT-Linux, piu' quelle che solo Windows pone:
//! residuo del loader, job annidati, processo gia' dentro un job, figli del
//! worker, comportamento della completion port.
//!
//! Ogni misura e' una riga `MISURA <chiave> <valore>`, cosi' il
//! registro non dipende da come io riassumo l'output.

#![allow(clippy::too_many_lines)] // e' un registro di misure, non una libreria

use std::io::Write as _;
use std::os::windows::io::AsRawHandle as _;
use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, OpenJobObjectW,
    QueryInformationJobObject, SetInformationJobObject, JobObjectAssociateCompletionPortInformation,
    JobObjectExtendedLimitInformation, JOBOBJECT_ASSOCIATE_COMPLETION_PORT,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_JOB_MEMORY,
};
use windows_sys::Win32::System::SystemServices::{
    JOB_OBJECT_MSG_ABNORMAL_EXIT_PROCESS, JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO,
    JOB_OBJECT_MSG_EXIT_PROCESS, JOB_OBJECT_MSG_JOB_MEMORY_LIMIT, JOB_OBJECT_MSG_NEW_PROCESS,
    JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT,
};

/// `windows-sys` non espone la maschera completa: e'
/// `STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x3F` (`winnt.h`).
const JOB_OBJECT_ALL_ACCESS: u32 = 0x001F_001F;
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED};

const TETTO: usize = 64 * 1024 * 1024;

fn main() {
    let argomenti: Vec<String> = std::env::args().collect();
    match argomenti.get(1).map(String::as_str) {
        Some("spawner") => spawner(&argomenti[2..]),
        Some("carico") => carico(&argomenti[2..]),
        _ => supervisore(),
    }
}

// ---------------------------------------------------------------------------
// Involucri minimi sulla FFI. Ogni `unsafe` qui e' un `unsafe` che il codice
// di produzione dovrebbe assorbire o delegare a una dipendenza.
// ---------------------------------------------------------------------------

fn nome_wide(nome: &str) -> Vec<u16> {
    nome.encode_utf16().chain(std::iter::once(0)).collect()
}

fn crea_job(nome: &str) -> HANDLE {
    let w = nome_wide(nome);
    // SAFETY: prototipo. `w` e' terminato da NUL e vive per tutta la chiamata.
    unsafe { CreateJobObjectW(std::ptr::null(), w.as_ptr()) }
}

fn apri_job(nome: &str) -> HANDLE {
    let w = nome_wide(nome);
    // SAFETY: prototipo. Come sopra.
    unsafe { OpenJobObjectW(JOB_OBJECT_ALL_ACCESS, 0, w.as_ptr()) }
}

fn imposta_tetto(job: HANDLE, byte: usize) -> bool {
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY;
    info.JobMemoryLimit = byte;
    // SAFETY: prototipo. `info` e' della classe dichiarata e della dimensione
    // dichiarata.
    let esito = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&info).cast(),
            u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap(),
        )
    };
    esito != 0
}

fn stato_job(job: HANDLE) -> Option<JOBOBJECT_EXTENDED_LIMIT_INFORMATION> {
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    let mut lunghezza: u32 = 0;
    // SAFETY: prototipo. Buffer della classe e della dimensione dichiarate.
    let esito = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_mut(&mut info).cast(),
            u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap(),
            &raw mut lunghezza,
        )
    };
    (esito != 0).then_some(info)
}

fn associa_porta(job: HANDLE, porta: HANDLE) -> bool {
    let mut info = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
        CompletionKey: std::ptr::null_mut(),
        CompletionPort: porta,
    };
    // SAFETY: prototipo. Struttura della classe e della dimensione dichiarate.
    let esito = unsafe {
        SetInformationJobObject(
            job,
            JobObjectAssociateCompletionPortInformation,
            std::ptr::from_mut(&mut info).cast(),
            u32::try_from(std::mem::size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>()).unwrap(),
        )
    };
    esito != 0
}

fn dentro_un_job(processo: HANDLE) -> bool {
    let mut esito: BOOL = 0;
    // SAFETY: prototipo. `jobhandle` nullo significa «un job qualunque».
    unsafe { IsProcessInJob(processo, std::ptr::null_mut(), &raw mut esito) };
    esito != 0
}

fn dentro_questo_job(processo: HANDLE, job: HANDLE) -> bool {
    let mut esito: BOOL = 0;
    // SAFETY: prototipo.
    unsafe { IsProcessInJob(processo, job, &raw mut esito) };
    esito != 0
}

/// Commit privato del processo: e' la grandezza che il limite del job governa,
/// quindi e' anche la misura giusta del residuo non coperto.
fn commit_privato(processo: HANDLE) -> Option<usize> {
    let mut contatori: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    let dimensione = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap();
    contatori.cb = dimensione;
    // SAFETY: prototipo. `GetProcessMemoryInfo` accetta la forma estesa purche'
    // `cb` la dichiari.
    let esito = unsafe {
        GetProcessMemoryInfo(processo, std::ptr::from_mut(&mut contatori).cast(), dimensione)
    };
    (esito != 0).then_some(contatori.PrivateUsage)
}

/// Riprende il thread primario di un processo creato sospeso.
///
/// `std` non espone l'handle del thread, quindi tocca ritrovarlo per
/// enumerazione. E' la ragione principale per cui la strada `CREATE_SUSPENDED`
/// costa piu' della strada dello spawner.
fn riprendi_processo(pid: u32) -> u32 {
    // SAFETY: prototipo. Lo snapshot viene chiuso alla fine.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    let mut voce: THREADENTRY32 = unsafe { std::mem::zeroed() };
    voce.dwSize = u32::try_from(std::mem::size_of::<THREADENTRY32>()).unwrap();
    let mut ripresi = 0_u32;
    // SAFETY: prototipo. `voce.dwSize` e' impostato come richiesto.
    let mut ok = unsafe { Thread32First(snapshot, &raw mut voce) };
    while ok != 0 {
        if voce.th32OwnerProcessID == pid {
            // SAFETY: prototipo. L'handle viene chiuso subito dopo.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, voce.th32ThreadID) };
            if !thread.is_null() {
                // SAFETY: prototipo.
                unsafe {
                    ResumeThread(thread);
                    CloseHandle(thread);
                }
                ripresi += 1;
            }
        }
        // SAFETY: prototipo.
        ok = unsafe { Thread32Next(snapshot, &raw mut voce) };
    }
    // SAFETY: prototipo.
    unsafe { CloseHandle(snapshot) };
    ripresi
}

fn nome_messaggio(codice: u32) -> &'static str {
    match codice {
        JOB_OBJECT_MSG_NEW_PROCESS => "NEW_PROCESS",
        JOB_OBJECT_MSG_EXIT_PROCESS => "EXIT_PROCESS",
        JOB_OBJECT_MSG_ABNORMAL_EXIT_PROCESS => "ABNORMAL_EXIT_PROCESS",
        JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT => "PROCESS_MEMORY_LIMIT",
        JOB_OBJECT_MSG_JOB_MEMORY_LIMIT => "JOB_MEMORY_LIMIT",
        JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO => "ACTIVE_PROCESS_ZERO",
        _ => "altro",
    }
}

/// Svuota la coda della porta senza bloccare oltre il timeout indicato.
fn raccogli_messaggi(porta: HANDLE, attesa_ms: u32) -> Vec<(u32, usize)> {
    let mut messaggi = Vec::new();
    loop {
        let mut codice: u32 = 0;
        let mut chiave: usize = 0;
        let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
        // SAFETY: prototipo. I tre puntatori sono a variabili locali vive.
        let esito = unsafe {
            GetQueuedCompletionStatus(
                porta,
                &raw mut codice,
                &raw mut chiave,
                &raw mut overlapped,
                attesa_ms,
            )
        };
        if esito == 0 {
            break;
        }
        messaggi.push((codice, overlapped as usize));
        if messaggi.len() > 64 {
            break;
        }
    }
    messaggi
}

// ---------------------------------------------------------------------------
// Modalita' `spawner`: entra nel job per nome, poi genera il worker.
//
// I figli ereditano l'appartenenza al job, quindi il worker nasce dentro.
// A differenza di Linux non c'e' `exec`: lo spawner resta vivo e la sua
// memoria pesa sul job. Quanto, e' una delle misure.
// ---------------------------------------------------------------------------

fn spawner(resto: &[String]) {
    let nome = &resto[0];
    let job = apri_job(nome);
    if job.is_null() {
        // SAFETY: prototipo.
        eprintln!("SPAWNER OpenJobObject fallita: {}", unsafe { GetLastError() });
        std::process::exit(101);
    }
    // SAFETY: prototipo. `GetCurrentProcess` torna una pseudo-handle valida.
    let io_stesso = unsafe { GetCurrentProcess() };
    // SAFETY: prototipo.
    let assegnato = unsafe { AssignProcessToJobObject(job, io_stesso) };
    if assegnato == 0 {
        // SAFETY: prototipo.
        eprintln!("SPAWNER AssignProcessToJobObject fallita: {}", unsafe { GetLastError() });
        std::process::exit(102);
    }
    let mio_commit = commit_privato(io_stesso).unwrap_or(0);
    eprintln!("SPAWNER commit-proprio {mio_commit}");

    let figlio = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("carico")
        .args(&resto[1..])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    match figlio {
        Ok(mut figlio) => {
            let stato = figlio.wait().expect("wait del worker");
            eprintln!("SPAWNER worker-codice {:?}", stato.code());
            std::process::exit(stato.code().unwrap_or(-1));
        }
        Err(e) => {
            eprintln!("SPAWNER spawn del worker fallito: {e}");
            std::process::exit(103);
        }
    }
}

// ---------------------------------------------------------------------------
// Modalita' `carico`: il finto worker.
// ---------------------------------------------------------------------------

fn carico(resto: &[String]) {
    let scenario = resto.first().map_or("tocca", String::as_str);
    // SAFETY: prototipo.
    let io_stesso = unsafe { GetCurrentProcess() };
    println!("CARICO in-job {}", if dentro_un_job(io_stesso) { "si" } else { "no" });
    println!("CARICO commit-iniziale {}", commit_privato(io_stesso).unwrap_or(0));
    println!("CARICO scenario {scenario}");
    let _ = std::io::stdout().flush();

    match scenario {
        "chiedi" => chiedi_senza_toccare(),
        "figlio" => genera_figlio(),
        "annidato" => prova_job_annidato(io_stesso),
        "inerte" => println!("CARICO inerte fine"),
        _ => tocca_finche_puoi(io_stesso),
    }
    let _ = std::io::stdout().flush();
}

fn tocca_finche_puoi(io_stesso: HANDLE) {
    let blocco = 4 * 1024 * 1024;
    let mut trattenuto: Vec<Vec<u8>> = Vec::new();
    let mut totale: usize = 0;
    loop {
        let mut pezzo: Vec<u8> = Vec::new();
        if let Err(e) = pezzo.try_reserve_exact(blocco) {
            // Allocazione NEGATA: il worker e' vivo e puo' reagire.
            println!("CARICO negata-a {totale} errore {e:?}");
            println!("CARICO commit-al-rifiuto {}", commit_privato(io_stesso).unwrap_or(0));
            // La prova che sia vivo: fa qualcosa dopo il rifiuto e lo dice.
            let mut piccolo: Vec<u8> = Vec::new();
            let riserva_piccola = piccolo.try_reserve_exact(4096).is_ok();
            println!(
                "CARICO vivo-dopo-il-rifiuto si (riserva-piccola={})",
                if riserva_piccola { "ok" } else { "negata" }
            );
            let _ = std::io::stdout().flush();
            std::process::exit(42);
        }
        pezzo.resize(blocco, 0xA5);
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

fn chiedi_senza_toccare() {
    for potenza in [30_u32, 34, 38, 42, 46] {
        let dimensione = 1_usize << potenza;
        let mut v: Vec<u8> = Vec::new();
        let esito = if v.try_reserve_exact(dimensione).is_ok() { "accettata" } else { "negata" };
        println!("CARICO richiesta 2^{potenza} {esito}");
        let _ = std::io::stdout().flush();
        drop(v);
    }
    println!("CARICO fine-richieste");
}

fn genera_figlio() {
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
    println!("CARICO figlio-codice {:?}", stato.code());
    println!("CARICO padre-sopravvissuto si");
}

/// Il worker prova a crearsi un job proprio con un tetto piu' alto. Se il
/// tetto esterno smettesse di valere, l'isolamento sarebbe aggirabile
/// dall'interno.
fn prova_job_annidato(io_stesso: HANDLE) {
    let interno = crea_job("");
    if interno.is_null() {
        // SAFETY: prototipo.
        println!("CARICO annidato creazione-fallita {}", unsafe { GetLastError() });
        return;
    }
    let alto = imposta_tetto(interno, 4 * 1024 * 1024 * 1024);
    println!("CARICO annidato tetto-alto-impostato {}", if alto { "si" } else { "no" });
    // SAFETY: prototipo.
    let assegnato = unsafe { AssignProcessToJobObject(interno, io_stesso) };
    println!("CARICO annidato assegnato {}", if assegnato != 0 { "si" } else { "no" });
    if assegnato == 0 {
        // SAFETY: prototipo.
        println!("CARICO annidato errore {}", unsafe { GetLastError() });
    }
    // Ora prova a superare il tetto ESTERNO restando sotto quello interno.
    tocca_finche_puoi(io_stesso);
}

// ---------------------------------------------------------------------------
// Supervisore.
// ---------------------------------------------------------------------------

struct Scenario {
    nome: &'static str,
    argomento: &'static str,
    tetto: usize,
}

fn supervisore() {
    println!("PT-Windows — prototipo bloccante fase 4");
    // SAFETY: prototipo.
    let io_stesso = unsafe { GetCurrentProcess() };
    println!(
        "MISURA supervisore.gia_in_un_job {}",
        if dentro_un_job(io_stesso) { "si" } else { "no" }
    );
    println!("MISURA supervisore.commit {}", commit_privato(io_stesso).unwrap_or(0));

    // ---- S1: residuo del loader sulla strada CREATE_SUSPENDED --------------
    println!("\n=== S1 — residuo del loader (CREATE_SUSPENDED + assegnazione) ===");
    misura_residuo_loader();

    // ---- S2..S6: strada dello spawner --------------------------------------
    let scenari = [
        Scenario { nome: "s2-nascita", argomento: "inerte", tetto: TETTO },
        Scenario { nome: "s3-chiedi", argomento: "chiedi", tetto: TETTO },
        Scenario { nome: "s4-tocca", argomento: "tocca", tetto: TETTO },
        Scenario { nome: "s5-figlio", argomento: "figlio", tetto: TETTO },
        Scenario { nome: "s6-annidato", argomento: "annidato", tetto: TETTO },
        // Tetto sotto il costo d'avvio di un processo: il gemello di S1-bis
        // su Linux. Serve a vedere se il dominio impedisce la nascita o si
        // limita a negare le allocazioni successive.
        Scenario { nome: "s7-infimo", argomento: "inerte", tetto: 256 * 1024 },
    ];
    for (indice, scenario) in scenari.iter().enumerate() {
        println!("\n=== {} — {} ===", scenario.nome, scenario.argomento);
        esegui_scenario(scenario, indice);
    }

    println!("\n=== FINE PT-Windows ===");
}

fn misura_residuo_loader() {
    let nome = format!("pt-windows-loader-{}", std::process::id());
    let job = crea_job(&nome);
    if job.is_null() {
        // SAFETY: prototipo.
        println!("MISURA s1.errore CreateJobObject {}", unsafe { GetLastError() });
        return;
    }
    if !imposta_tetto(job, TETTO) {
        // SAFETY: prototipo.
        println!("MISURA s1.errore SetInformationJobObject {}", unsafe { GetLastError() });
        return;
    }

    let figlio = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("carico")
        .arg("inerte")
        .creation_flags(CREATE_SUSPENDED)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut figlio) = figlio else {
        println!("MISURA s1.errore spawn");
        return;
    };
    let handle = figlio.as_raw_handle() as HANDLE;

    // Il thread primario non ha ancora eseguito un'istruzione: quello che si
    // misura qui e' cio' che il sistema ha gia' impegnato per conto del
    // processo PRIMA che il job possa dire qualcosa.
    let residuo = commit_privato(handle).unwrap_or(0);
    println!("MISURA s1.commit_prima_dell_associazione {residuo}");

    // SAFETY: prototipo.
    let assegnato = unsafe { AssignProcessToJobObject(job, handle) };
    println!("MISURA s1.assegnazione {}", if assegnato != 0 { "riuscita" } else { "fallita" });
    if assegnato == 0 {
        // SAFETY: prototipo.
        println!("MISURA s1.errore_assegnazione {}", unsafe { GetLastError() });
    }
    println!("MISURA s1.in_questo_job {}", if dentro_questo_job(handle, job) { "si" } else { "no" });
    if let Some(stato) = stato_job(job) {
        println!("MISURA s1.job_peak_dopo_assegnazione {}", stato.PeakJobMemoryUsed);
        println!("MISURA s1.job_peak_processo {}", stato.PeakProcessMemoryUsed);
    }

    let ripresi = riprendi_processo(figlio.id());
    println!("MISURA s1.thread_ripresi {ripresi}");
    let stato = figlio.wait().expect("wait");
    println!("MISURA s1.codice {:?}", stato.code());
    if let Some(finale) = stato_job(job) {
        println!("MISURA s1.job_peak_finale {}", finale.PeakJobMemoryUsed);
    }
    // SAFETY: prototipo.
    unsafe { CloseHandle(job) };
}

fn esegui_scenario(scenario: &Scenario, indice: usize) {
    let nome_job = format!("pt-windows-{}-{indice}", std::process::id());
    let job = crea_job(&nome_job);
    if job.is_null() {
        // SAFETY: prototipo.
        println!("MISURA {}.errore CreateJobObject {}", scenario.nome, unsafe { GetLastError() });
        return;
    }
    if !imposta_tetto(job, scenario.tetto) {
        // SAFETY: prototipo.
        println!("MISURA {}.errore tetto {}", scenario.nome, unsafe { GetLastError() });
        return;
    }
    // SAFETY: prototipo. Porta nuova, nessun file associato.
    let porta = unsafe { CreateIoCompletionPort(!0_usize as HANDLE, std::ptr::null_mut(), 0, 1) };
    let associata = associa_porta(job, porta);
    println!("MISURA {}.porta_associata {}", scenario.nome, if associata { "si" } else { "no" });

    let uscita = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("spawner")
        .arg(&nome_job)
        .arg(scenario.argomento)
        .output()
        .expect("esecuzione dello spawner");

    for riga in String::from_utf8_lossy(&uscita.stderr).lines() {
        println!("    | {riga}");
    }
    let stdout = String::from_utf8_lossy(&uscita.stdout).into_owned();
    for riga in stdout.lines() {
        println!("    > {riga}");
    }

    println!("MISURA {}.codice {:?}", scenario.nome, uscita.status.code());
    println!(
        "MISURA {}.worker_in_job {}",
        scenario.nome,
        if stdout.contains("CARICO in-job si") { "si" } else { "no" }
    );
    println!(
        "MISURA {}.allocazione_negata {}",
        scenario.nome,
        if stdout.contains("negata-a") { "si" } else { "no" }
    );
    println!(
        "MISURA {}.vivo_dopo_il_rifiuto {}",
        scenario.nome,
        if stdout.contains("vivo-dopo-il-rifiuto si") { "si" } else { "no" }
    );

    if let Some(stato) = stato_job(job) {
        println!("MISURA {}.job_peak {}", scenario.nome, stato.PeakJobMemoryUsed);
        println!("MISURA {}.processo_peak {}", scenario.nome, stato.PeakProcessMemoryUsed);
        println!("MISURA {}.tetto {}", scenario.nome, stato.JobMemoryLimit);
    }

    let messaggi = raccogli_messaggi(porta, 500);
    let mut riassunto: Vec<String> = Vec::new();
    for (codice, chiave) in &messaggi {
        riassunto.push(format!("{}({chiave})", nome_messaggio(*codice)));
    }
    println!("MISURA {}.messaggi {}", scenario.nome, riassunto.join(" "));
    println!(
        "MISURA {}.notifica_limite {}",
        scenario.nome,
        if messaggi.iter().any(|(c, _)| *c == JOB_OBJECT_MSG_JOB_MEMORY_LIMIT
            || *c == JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT)
        {
            "si"
        } else {
            "no"
        }
    );

    // SAFETY: prototipo.
    unsafe {
        CloseHandle(porta);
        CloseHandle(job);
    }
}
