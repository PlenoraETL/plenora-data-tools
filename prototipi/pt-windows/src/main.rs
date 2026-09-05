//! PT-Windows — prototipo bloccante dell'esecuzione isolata.
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
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    JobObjectLimitViolationInformation, JobObjectNotificationLimitInformation,
    JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECT_LIMIT_VIOLATION_INFORMATION,
    JOBOBJECT_NOTIFICATION_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_JOB_MEMORY,
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

/// Il *notification limit* sta sotto il tetto duro: il superamento va
/// registrato prima che il contenimento entri in gioco, altrimenti la prova
/// arriverebbe insieme al danno invece che prima.
const SOGLIA_NOTIFICA: u64 = 48 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Secondo ciclo: la prova interrogabile.
// ---------------------------------------------------------------------------

/// Registra un limite di notifica sulla memoria del job.
///
/// E' distinto dal tetto duro: quello contiene, questo **fa registrare la
/// violazione** in una struttura che si puo' interrogare in un momento
/// qualunque, senza dipendere dalla consegna di un messaggio.
fn imposta_notifica(job: HANDLE, byte: u64) -> bool {
    let mut info: JOBOBJECT_NOTIFICATION_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY;
    info.JobMemoryLimit = byte;
    // SAFETY: prototipo. Classe e dimensione dichiarate coerenti.
    let esito = unsafe {
        SetInformationJobObject(
            job,
            JobObjectNotificationLimitInformation,
            std::ptr::from_ref(&info).cast(),
            u32::try_from(std::mem::size_of::<JOBOBJECT_NOTIFICATION_LIMIT_INFORMATION>()).unwrap(),
        )
    };
    esito != 0
}

/// La prova interrogabile: quali limiti risultano violati, adesso.
fn violazioni(job: HANDLE) -> Option<JOBOBJECT_LIMIT_VIOLATION_INFORMATION> {
    let mut info: JOBOBJECT_LIMIT_VIOLATION_INFORMATION = unsafe { std::mem::zeroed() };
    let mut lunghezza: u32 = 0;
    // SAFETY: prototipo. Classe e dimensione dichiarate coerenti.
    let esito = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectLimitViolationInformation,
            std::ptr::from_mut(&mut info).cast(),
            u32::try_from(std::mem::size_of::<JOBOBJECT_LIMIT_VIOLATION_INFORMATION>()).unwrap(),
            &raw mut lunghezza,
        )
    };
    (esito != 0).then_some(info)
}

/// Quanti processi vivono ancora nel job: la quiescenza, interrogata.
fn processi_attivi(job: HANDLE) -> Option<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
    let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
    let mut lunghezza: u32 = 0;
    // SAFETY: prototipo.
    let esito = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicAccountingInformation,
            std::ptr::from_mut(&mut info).cast(),
            u32::try_from(std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>()).unwrap(),
            &raw mut lunghezza,
        )
    };
    (esito != 0).then_some(info)
}

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
        "orfano" => genera_orfano(),
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

/// Un discendente che sopravvive al capofila, per la domanda sulla quiescenza.
fn genera_orfano() {
    let mio = std::env::current_exe().expect("current_exe");
    let figlio = Command::new(mio)
        .arg("carico")
        .arg("tocca")
        // Senza questo il supervisore aspetterebbe anche lui, e la domanda
        // sulla quiescenza non si porrebbe.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match figlio {
        Ok(figlio) => println!("CARICO orfano-pid {}", figlio.id()),
        Err(e) => println!("CARICO orfano-spawn-fallito {e}"),
    }
    println!("CARICO capofila-esce-subito con-uscita-0");
    let _ = std::io::stdout().flush();
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


// ---------------------------------------------------------------------------
// Secondo ciclo: gli scenari avversari.
// ---------------------------------------------------------------------------

/// Prepara un job con tetto duro e limite di notifica, e facoltativamente una
/// porta. Torna `(job, porta)`; la porta e' nulla se non richiesta.
fn job_del_secondo_ciclo(nome: &str, con_porta: bool) -> (HANDLE, HANDLE) {
    let job = crea_job(nome);
    if job.is_null() {
        return (job, std::ptr::null_mut());
    }
    imposta_tetto(job, TETTO);
    let porta = if con_porta {
        // SAFETY: prototipo. Porta nuova, nessun file associato.
        let porta =
            unsafe { CreateIoCompletionPort(!0_usize as HANDLE, std::ptr::null_mut(), 0, 1) };
        associa_porta(job, porta);
        porta
    } else {
        std::ptr::null_mut()
    };
    // Il limite di notifica va dopo la porta: e' li' che il sistema decide se
    // ha qualcuno a cui riferire.
    let notifica = imposta_notifica(job, SOGLIA_NOTIFICA);
    println!(
        "MISURA {nome}.notifica_impostata {}",
        if notifica { "si" } else { "no" }
    );
    (job, porta)
}

fn stampa_violazioni(etichetta: &str, job: HANDLE) -> bool {
    match violazioni(job) {
        Some(v) => {
            let memoria_violata = (v.ViolationLimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY) != 0;
            println!("MISURA {etichetta}.violazione_interrogabile si");
            println!("MISURA {etichetta}.violation_limit_flags {:#x}", v.ViolationLimitFlags);
            println!("MISURA {etichetta}.job_memory {}", v.JobMemory);
            println!("MISURA {etichetta}.job_memory_limit {}", v.JobMemoryLimit);
            println!(
                "MISURA {etichetta}.memoria_risulta_violata {}",
                if memoria_violata { "si" } else { "no" }
            );
            memoria_violata
        }
        None => {
            // SAFETY: prototipo.
            println!("MISURA {etichetta}.violazione_interrogabile no (errore {})", unsafe {
                GetLastError()
            });
            false
        }
    }
}

/// W1 — il caso peggiore: il capofila esce 0, il figlio sfonda, e la porta
/// non viene MAI drenata. La prova regge lo stesso?
fn w1_prova_senza_drenare(indice: usize) {
    println!("\n=== W1 — capofila a 0, porta mai drenata ===");
    let nome = format!("pt-windows-w1-{}-{indice}", std::process::id());
    let (job, porta) = job_del_secondo_ciclo(&nome, true);
    if job.is_null() {
        println!("MISURA w1.errore creazione");
        return;
    }

    let uscita = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("spawner")
        .arg(&nome)
        .arg("figlio")
        .output()
        .expect("spawner");
    let stdout = String::from_utf8_lossy(&uscita.stdout).into_owned();

    println!("MISURA w1.codice_capofila {:?}", uscita.status.code());
    println!(
        "MISURA w1.figlio_ha_sfondato {}",
        if stdout.contains("negata-a") { "si" } else { "no" }
    );
    let violata = stampa_violazioni("w1", job);
    println!(
        "MISURA w1.prova_disponibile_senza_messaggi {}",
        if violata { "SI" } else { "NO — la prova dipende dai messaggi" }
    );

    // Solo ORA si guarda la porta, per confrontare le due fonti.
    let messaggi = raccogli_messaggi(porta, 300);
    let riassunto: Vec<String> = messaggi
        .iter()
        .map(|(c, k)| format!("{}({k})", nome_messaggio(*c)))
        .collect();
    println!("MISURA w1.messaggi_in_coda {}", riassunto.join(" "));

    // SAFETY: prototipo.
    unsafe {
        CloseHandle(porta);
        CloseHandle(job);
    }
}

/// W2 — e senza alcuna porta associata? Se la prova sopravvive anche qui,
/// non dipende dal meccanismo di consegna.
fn w2_prova_senza_porta(indice: usize) {
    println!("\n=== W2 — nessuna completion port associata ===");
    let nome = format!("pt-windows-w2-{}-{indice}", std::process::id());
    let (job, _) = job_del_secondo_ciclo(&nome, false);
    if job.is_null() {
        println!("MISURA w2.errore creazione");
        return;
    }

    let uscita = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("spawner")
        .arg(&nome)
        .arg("tocca")
        .output()
        .expect("spawner");
    let stdout = String::from_utf8_lossy(&uscita.stdout).into_owned();
    println!("MISURA w2.codice {:?}", uscita.status.code());
    println!(
        "MISURA w2.ha_sfondato {}",
        if stdout.contains("negata-a") { "si" } else { "no" }
    );
    let violata = stampa_violazioni("w2", job);
    println!(
        "MISURA w2.prova_indipendente_dalla_porta {}",
        if violata { "SI" } else { "NO" }
    );
    // SAFETY: prototipo.
    unsafe { CloseHandle(job) };
}

/// W3 — quiescenza: al ritorno della `wait` il job e' vuoto?
fn w3_quiescenza(indice: usize) {
    println!("\n=== W3 — quiescenza: capofila a 0 con un orfano vivo ===");
    let nome = format!("pt-windows-w3-{}-{indice}", std::process::id());
    let (job, porta) = job_del_secondo_ciclo(&nome, true);
    if job.is_null() {
        println!("MISURA w3.errore creazione");
        return;
    }

    let uscita = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("spawner")
        .arg(&nome)
        .arg("orfano")
        .output()
        .expect("spawner");
    println!("MISURA w3.codice_capofila {:?}", uscita.status.code());

    let subito = processi_attivi(job).map_or(u32::MAX, |a| a.ActiveProcesses);
    println!("MISURA w3.processi_attivi_alla_wait {subito}");
    let violata_subito = violazioni(job)
        .is_some_and(|v| (v.ViolationLimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY) != 0);
    println!(
        "MISURA w3.violazione_visibile_alla_wait {}",
        if violata_subito { "si" } else { "no" }
    );

    // Attesa della quiescenza, come dovrebbe fare la barriera.
    let mut giri = 0;
    while processi_attivi(job).map_or(0, |a| a.ActiveProcesses) > 0 && giri < 600 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        giri += 1;
    }
    println!("MISURA w3.attesa_quiescenza_ms {}", giri * 50);
    println!(
        "MISURA w3.processi_attivi_dopo {}",
        processi_attivi(job).map_or(u32::MAX, |a| a.ActiveProcesses)
    );
    let violata_dopo = stampa_violazioni("w3", job);
    println!(
        "MISURA w3.evidenza_arrivata_in_ritardo {}",
        if violata_dopo && !violata_subito {
            "SI — pubblicare alla wait sarebbe stato un errore"
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

/// W4 — con il supervisore gia' dentro un job esterno.
fn w4_job_ereditato(indice: usize) {
    println!("\n=== W4 — supervisore gia' dentro un job esterno ===");
    let esterno = crea_job(&format!("pt-windows-esterno-{}-{indice}", std::process::id()));
    if esterno.is_null() {
        println!("MISURA w4.errore job-esterno");
        return;
    }
    // Tetto molto piu' alto: non deve essere lui a fermare nulla.
    imposta_tetto(esterno, 4 * 1024 * 1024 * 1024);
    // SAFETY: prototipo.
    let dentro = unsafe { AssignProcessToJobObject(esterno, GetCurrentProcess()) };
    println!(
        "MISURA w4.supervisore_nel_job_esterno {}",
        if dentro != 0 { "si" } else { "no" }
    );
    // SAFETY: prototipo.
    println!("MISURA w4.gia_in_un_job {}", if dentro_un_job(unsafe { GetCurrentProcess() }) {
        "si"
    } else {
        "no"
    });

    let nome = format!("pt-windows-w4-{}-{indice}", std::process::id());
    let (job, porta) = job_del_secondo_ciclo(&nome, true);
    if job.is_null() {
        println!("MISURA w4.errore job-interno");
        return;
    }
    let uscita = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("spawner")
        .arg(&nome)
        .arg("tocca")
        .output()
        .expect("spawner");
    let stdout = String::from_utf8_lossy(&uscita.stdout).into_owned();
    println!("MISURA w4.codice {:?}", uscita.status.code());
    println!(
        "MISURA w4.contenuto_dal_job_interno {}",
        if stdout.contains("negata-a") { "si" } else { "no" }
    );
    let violata = stampa_violazioni("w4", job);
    println!(
        "MISURA w4.prova_regge_con_job_ereditato {}",
        if violata { "SI" } else { "NO" }
    );
    // SAFETY: prototipo. Il job esterno non si chiude: il supervisore ci vive
    // dentro fino alla fine, ed e' cio' che lo scenario vuole rappresentare.
    unsafe {
        CloseHandle(porta);
        CloseHandle(job);
    }
}

/// W5 — ripetizioni: intervalli invece di campioni singoli.
fn w5_ripetizioni(quante: usize) {
    println!("\n=== W5 — ripetizioni del residuo del loader ===");
    let mut residui = Vec::new();
    let mut spawner = Vec::new();
    for giro in 0..quante {
        let nome = format!("pt-windows-w5-{}-{giro}", std::process::id());
        let job = crea_job(&nome);
        if job.is_null() || !imposta_tetto(job, TETTO) {
            continue;
        }
        let figlio = Command::new(std::env::current_exe().expect("current_exe"))
            .arg("carico")
            .arg("inerte")
            .creation_flags(CREATE_SUSPENDED)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        if let Ok(mut figlio) = figlio {
            let handle = figlio.as_raw_handle() as HANDLE;
            if let Some(r) = commit_privato(handle) {
                residui.push(r);
            }
            // SAFETY: prototipo.
            unsafe { AssignProcessToJobObject(job, handle) };
            riprendi_processo(figlio.id());
            let _ = figlio.wait();
        }
        // SAFETY: prototipo.
        unsafe { CloseHandle(job) };

        // E quanto costa lo spawner al dominio.
        let nome2 = format!("pt-windows-w5s-{}-{giro}", std::process::id());
        let job2 = crea_job(&nome2);
        if !job2.is_null() && imposta_tetto(job2, TETTO) {
            let uscita = Command::new(std::env::current_exe().expect("current_exe"))
                .arg("spawner")
                .arg(&nome2)
                .arg("inerte")
                .output();
            if let Ok(uscita) = uscita {
                let testo = String::from_utf8_lossy(&uscita.stderr).into_owned();
                for riga in testo.lines().filter(|r| r.contains("commit-proprio")) {
                    if let Some(v) = riga.split_whitespace().last().and_then(|v| v.parse().ok()) {
                        spawner.push(v);
                    }
                }
            }
            // SAFETY: prototipo.
            unsafe { CloseHandle(job2) };
        }
    }
    riporta_intervallo("w5.residuo_loader", &residui);
    riporta_intervallo("w5.commit_spawner", &spawner);
}

/// W6 — la prova e' un livello o un fermo? Campionamento fitto dopo la fine.
///
/// Se sparisce, ogni disegno che interroghi il sistema DOPO aver atteso la
/// quiescenza sta correndo contro un orologio che non controlla.
fn w6_durata_della_prova(indice: usize) {
    println!("\n=== W6 — per quanto resta interrogabile la violazione ===");
    let nome = format!("pt-windows-w6-{}-{indice}", std::process::id());
    let (job, porta) = job_del_secondo_ciclo(&nome, true);
    if job.is_null() {
        println!("MISURA w6.errore creazione");
        return;
    }

    let uscita = Command::new(std::env::current_exe().expect("current_exe"))
        .arg("spawner")
        .arg(&nome)
        .arg("tocca")
        .output()
        .expect("spawner");
    println!("MISURA w6.codice {:?}", uscita.status.code());

    // Campiona subito e poi ogni 25 ms, fino a un secondo.
    let mut visibile_a = Vec::new();
    let mut sparita_a: Option<usize> = None;
    for giro in 0..40_usize {
        let attiva = violazioni(job)
            .is_some_and(|v| (v.ViolationLimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY) != 0);
        let ms = giro * 25;
        if attiva {
            visibile_a.push(ms);
        } else if sparita_a.is_none() && !visibile_a.is_empty() {
            sparita_a = Some(ms);
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    println!(
        "MISURA w6.visibile_al_primo_campione {}",
        if visibile_a.first() == Some(&0) { "si" } else { "no" }
    );
    println!("MISURA w6.campioni_con_violazione {}", visibile_a.len());
    match sparita_a {
        Some(ms) => {
            println!("MISURA w6.sparita_dopo_ms {ms}");
            println!("MISURA w6.prova_durevole NO — e' un livello, non un fermo");
        }
        None if visibile_a.is_empty() => {
            println!("MISURA w6.sparita_dopo_ms mai-vista");
            println!("MISURA w6.prova_durevole NO — non e' stata vista affatto");
        }
        None => {
            println!("MISURA w6.sparita_dopo_ms non-sparita-entro-1000");
            println!("MISURA w6.prova_durevole forse — non sparita entro un secondo");
        }
    }
    // SAFETY: prototipo.
    unsafe {
        CloseHandle(porta);
        CloseHandle(job);
    }
}

/// W7 — il picco del job come prova durevole, e il suo costo in falsi
/// positivi.
fn w7_prova_durevole(indice: usize) {
    println!("\n=== W7 — il picco del job sopravvive alla quiescenza? ===");

    for (etichetta, scenario, sfonda) in [
        ("sfonda", "tocca", true),
        ("sotto_tetto", "inerte", false),
        ("chiede_e_basta", "chiedi", false),
        // Il caso peggiore, esatto: sfonda un figlio, il capofila esce 0.
        ("figlio_sfonda", "figlio", true),
    ] {
        let nome = format!("pt-windows-w7{etichetta}-{}-{indice}", std::process::id());
        let job = crea_job(&nome);
        if job.is_null() || !imposta_tetto(job, TETTO) {
            println!("MISURA w7.{etichetta}.errore creazione");
            continue;
        }
        let uscita = Command::new(std::env::current_exe().expect("current_exe"))
            .arg("spawner")
            .arg(&nome)
            .arg(scenario)
            .output()
            .expect("spawner");
        println!("MISURA w7.{etichetta}.codice {:?}", uscita.status.code());

        // Subito, e poi dopo mezzo secondo: il picco deve essere lo stesso.
        let subito = stato_job(job).map_or(0, |s| s.PeakJobMemoryUsed);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let dopo = stato_job(job).map_or(0, |s| s.PeakJobMemoryUsed);
        let tetto = stato_job(job).map_or(0, |s| s.JobMemoryLimit);

        println!("MISURA w7.{etichetta}.picco_subito {subito}");
        println!("MISURA w7.{etichetta}.picco_dopo_500ms {dopo}");
        println!("MISURA w7.{etichetta}.tetto {tetto}");
        println!(
            "MISURA w7.{etichetta}.picco_stabile {}",
            if subito == dopo { "si" } else { "NO" }
        );
        let raggiunto = dopo >= tetto && tetto > 0;
        println!(
            "MISURA w7.{etichetta}.picco_almeno_il_tetto {}",
            if raggiunto { "si" } else { "no" }
        );
        println!(
            "MISURA w7.{etichetta}.verdetto {}",
            match (sfonda, raggiunto) {
                (true, true) => "corretto — ha sfondato ed e' rilevato",
                (true, false) => "FALSO NEGATIVO — ha sfondato e non si vede",
                (false, true) => "FALSO POSITIVO — non ha sfondato ma risulta",
                (false, false) => "corretto — non ha sfondato e non risulta",
            }
        );
        // SAFETY: prototipo.
        unsafe { CloseHandle(job) };
    }
}

fn riporta_intervallo(chiave: &str, valori: &[usize]) {
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

    w1_prova_senza_drenare(1);
    w2_prova_senza_porta(2);
    w3_quiescenza(3);
    w5_ripetizioni(5);
    w6_durata_della_prova(6);
    w7_prova_durevole(7);
    // W4 per ultimo: mette il supervisore dentro un job da cui non esce.
    w4_job_ereditato(4);

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
