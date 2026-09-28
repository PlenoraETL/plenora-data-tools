use std::path::Path;

use plenora_core::error::PressioneDegliAntenati;

use super::super::conduzione::LettoreDiEvidenza;
use super::LeggiEvidenzaDominio;

/// Scrive i file di un dominio finto con i contatori dati.
fn scrivi_dominio(dominio: &Path, oom: u64, uccisi: u64, group_kill: u64, max: u64, picco: u64) {
    std::fs::write(
        dominio.join("memory.events.local"),
        format!(
            "low 0\nhigh 0\nmax {max}\noom {oom}\noom_kill {uccisi}\noom_group_kill {group_kill}\n"
        ),
    )
    .expect("memory.events.local");
    std::fs::write(
        dominio.join("memory.events"),
        format!(
            "low 0\nhigh 0\nmax {max}\noom {oom}\noom_kill {uccisi}\noom_group_kill {group_kill}\n"
        ),
    )
    .expect("memory.events");
    std::fs::write(dominio.join("memory.peak"), format!("{picco}\n")).expect("memory.peak");
}

fn scrivi_antenato(antenato: &Path, oom: u64) {
    std::fs::write(
        antenato.join("memory.events.local"),
        format!("low 0\nhigh 0\nmax 0\noom {oom}\noom_kill 0\noom_group_kill 0\n"),
    )
    .expect("memory.events.local dell'antenato");
}

/// Radice, padre e dominio, in quest'ordine.
fn gerarchia() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let base = tempfile::tempdir().expect("tempdir");
    let radice = base.path().join("radice");
    let padre = radice.join("padre");
    let dominio = padre.join("dominio");
    std::fs::create_dir_all(&dominio).expect("gerarchia");
    // Come in cgroupfs, `cgroup.kill` esiste gia' e vuoto: chi termina lo
    // apre e ci scrive, non lo crea.
    std::fs::write(dominio.join("cgroup.kill"), "").expect("cgroup.kill");
    (base, radice, padre, dominio)
}

/// Il delta si misura dall'istante del costruttore: cio' che il dominio ha
/// registrato prima non entra, cio' che registra dopo si'. E' la proprieta'
/// da cui dipende `EvidenzaDaPrimaDelloSpawn`, che prende quell'istante prima
/// dello spawn.
#[test]
fn il_delta_parte_dall_istantanea_del_costruttore() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 2, 1, 1, 7, 100);
    scrivi_antenato(&padre, 3);
    scrivi_antenato(&radice, 0);

    let mut lettore = LeggiEvidenzaDominio::nuova(dominio.clone(), &radice, 4096);

    scrivi_dominio(&dominio, 3, 3, 2, 9, 500);
    scrivi_antenato(&padre, 3);
    scrivi_antenato(&radice, 1);

    let evidenza = lettore.evidenza().expect("evidenza");
    assert_eq!(evidenza.oom_locali, Some(1));
    assert_eq!(evidenza.uccisi_nel_dominio, Some(2));
    assert_eq!(evidenza.uccisi_nella_gerarchia, Some(2));
    assert_eq!(evidenza.group_kill_locale, Some(1));
    assert_eq!(evidenza.diagnostica.respinte_al_tetto, Some(2));
    assert_eq!(evidenza.diagnostica.picco_byte, Some(500));
    assert_eq!(evidenza.diagnostica.tetto_byte, 4096);
    assert_eq!(
        evidenza.oom_degli_antenati,
        PressioneDegliAntenati::nuova(&[Some(0), Some(1)], Some(2), 0).expect("forma")
    );
}

/// Un OOM **successivo** all'istantanea resta visibile su un dominio che
/// parte da zero: e' il caso di un worker che va in OOM subito dopo
/// l'`Incarico`, che un'istantanea presa tardi assorbirebbe nel «prima».
#[test]
fn un_oom_dopo_l_istantanea_resta_nell_evidenza() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);

    let mut lettore = LeggiEvidenzaDominio::nuova(dominio.clone(), &radice, 4096);
    scrivi_dominio(&dominio, 1, 1, 1, 3, 4096);

    let evidenza = lettore.evidenza().expect("evidenza");
    assert_eq!(evidenza.oom_locali, Some(1));
    assert_eq!(evidenza.group_kill_locale, Some(1));
}

/// Un contatore che scende non e' una diminuzione vera: il delta e' `None`,
/// non zero, perche' zero direbbe «nessuna pressione».
#[test]
fn un_contatore_che_scende_non_diventa_zero() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 5, 5, 5, 5, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);

    let mut lettore = LeggiEvidenzaDominio::nuova(dominio.clone(), &radice, 4096);
    scrivi_dominio(&dominio, 4, 5, 5, 5, 0);

    let evidenza = lettore.evidenza().expect("evidenza");
    assert_eq!(evidenza.oom_locali, None);
    assert_eq!(evidenza.uccisi_nel_dominio, Some(0));
}

/// Un file che manca al primo istante lascia `None`: non si inventa uno zero
/// di partenza.
#[test]
fn un_file_assente_all_istantanea_lascia_none() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);

    let mut lettore = LeggiEvidenzaDominio::nuova(dominio.clone(), &radice, 4096);
    scrivi_dominio(&dominio, 1, 1, 1, 1, 1);

    let evidenza = lettore.evidenza().expect("evidenza");
    assert_eq!(evidenza.oom_locali, None);
    assert_eq!(evidenza.group_kill_locale, None);
}

/// L'evidenza da prima dello spawn su un dominio finto: `cgroup.events` dice
/// se e' popolato.
fn evidenza_su(radice: &Path, dominio: &Path) -> super::super::EvidenzaDaPrimaDelloSpawn {
    super::super::EvidenzaDaPrimaDelloSpawn {
        lettore: LeggiEvidenzaDominio::nuova(dominio.to_path_buf(), radice, 4096),
        dominio: dominio.to_path_buf(),
    }
}

fn popolato(dominio: &Path, si: bool) {
    std::fs::write(
        dominio.join("cgroup.events"),
        format!("populated {}\nfrozen 0\n", u8::from(si)),
    )
    .expect("cgroup.events");
}

fn causa_del_dialogo() -> plenora_core::PlenoraError {
    plenora_core::PlenoraError::IsolationUnavailable("il worker ha chiuso il canale".to_owned())
}

/// Un OOM attribuito fra lo spawn e la conduzione non si presenta come
/// «isolamento non disponibile»: l'evidenza del dominio precede la causa del
/// canale.
#[test]
fn un_oom_prima_della_conduzione_diventa_resource_limit() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, false);
    let evidenza = evidenza_su(&radice, &dominio);
    scrivi_dominio(&dominio, 1, 1, 1, 3, 4096);

    let errore = evidenza.rileggi_il_fallimento("worker", causa_del_dialogo());
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{errore}"
    );
}

/// Pressione osservata senza il gruppo ucciso: non attribuita, ma non taciuta.
#[test]
fn una_pressione_non_attribuita_prima_della_conduzione_resta_visibile() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, false);
    let evidenza = evidenza_su(&radice, &dominio);
    scrivi_dominio(&dominio, 1, 0, 0, 3, 4096);

    let errore = evidenza.rileggi_il_fallimento("worker", causa_del_dialogo());
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::UnattributedMemoryPressure,
        "{errore}"
    );
}

/// Senza eventi di memoria la causa resta quella del dialogo.
#[test]
fn senza_eventi_resta_la_causa_del_dialogo() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, false);
    let evidenza = evidenza_su(&radice, &dominio);

    let errore = evidenza.rileggi_il_fallimento("worker", causa_del_dialogo());
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::IsolationUnavailable,
        "{errore}"
    );
}

/// Un dominio che non si svuota nemmeno con `cgroup.kill` non si legge
/// (`F4-10`), e l'errore lo dice: processi possono esserci rimasti.
#[test]
fn un_dominio_che_non_si_svuota_nemmeno_col_kill_e_un_errore_interno() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, true);
    let evidenza = evidenza_su(&radice, &dominio);
    scrivi_dominio(&dominio, 1, 1, 1, 3, 4096);

    let errore = evidenza.rileggi_il_fallimento("worker", causa_del_dialogo());
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::Internal,
        "{errore}"
    );
    assert_eq!(
        std::fs::read_to_string(dominio.join("cgroup.kill")).expect("cgroup.kill scritto"),
        "1"
    );
}

/// Il kernel finto: svuota il dominio quando compare `cgroup.kill`.
fn svuota_al_kill(dominio: &Path) -> std::thread::JoinHandle<()> {
    let dominio = dominio.to_path_buf();
    // Una scadenza ampia, non un numero di giri: il codice sotto test aspetta
    // la quiescenza prima di scrivere `cgroup.kill`, e su una macchina carica
    // fra l'avvio di questo thread e la scrittura passano secondi. Un kernel
    // finto che si arrende prima lascia il dominio popolato, e il test
    // fallisce per una corsa, non per un difetto.
    let scadenza = std::time::Instant::now() + std::time::Duration::from_secs(60);
    std::thread::spawn(move || {
        while std::time::Instant::now() < scadenza {
            let kill = std::fs::read_to_string(dominio.join("cgroup.kill")).expect("cgroup.kill");
            if kill == "1" {
                popolato(&dominio, false);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    })
}

/// Svuotato solo da `cgroup.kill`, senza evidenza che attribuisca: la §10.3
/// chiama ambiguo l'esito, e la causa del dialogo da sola non basta.
#[test]
fn terminato_col_kill_senza_evidenza_e_ambiguo() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, true);
    let evidenza = evidenza_su(&radice, &dominio);
    let kernel = svuota_al_kill(&dominio);

    let errore = evidenza.rileggi_il_fallimento("worker", causa_del_dialogo());
    kernel.join().expect("kernel finto");
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::Internal,
        "{errore}"
    );
    assert!(errore.to_string().contains("ambiguo"), "{errore}");
}

/// Terminato con `cgroup.kill`, ma l'evidenza attribuisce: resta l'OOM.
#[test]
fn terminato_col_kill_con_oom_attribuito_resta_resource_limit() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, true);
    let evidenza = evidenza_su(&radice, &dominio);
    scrivi_dominio(&dominio, 1, 1, 1, 3, 4096);
    let kernel = svuota_al_kill(&dominio);

    let errore = evidenza.rileggi_il_fallimento("worker", causa_del_dialogo());
    kernel.join().expect("kernel finto");
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{errore}"
    );
}

fn cancellazione() -> plenora_core::PlenoraError {
    plenora_core::PlenoraError::Cancelled {
        node: "verificatore".to_owned(),
        operation: "dominio isolato".to_owned(),
        execution_id: String::new(),
        reason: "cancellato appena nato".to_owned(),
    }
}

/// La cancellazione cede al solo OOM attribuito (§10.3)...
#[test]
fn la_cancellazione_cede_all_oom_attribuito() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, false);
    let evidenza = evidenza_su(&radice, &dominio);
    scrivi_dominio(&dominio, 1, 1, 1, 3, 4096);

    let errore = evidenza.rileggi_la_cancellazione("verificatore", cancellazione());
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{errore}"
    );
}

/// ...e non a una pressione che non attribuisce.
#[test]
fn la_cancellazione_resta_davanti_alla_pressione_non_attribuita() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, false);
    let evidenza = evidenza_su(&radice, &dominio);
    scrivi_dominio(&dominio, 1, 0, 0, 3, 4096);

    let errore = evidenza.rileggi_la_cancellazione("verificatore", cancellazione());
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::Cancelled,
        "{errore}"
    );
}

/// Il giudizio su una `Risposta` intera resta tale anche se il dominio si
/// svuota solo con `cgroup.kill`: nessuna quiescenza tardiva rende ambiguo un
/// fatto (righe 9 e 10 della matrice).
#[test]
fn il_giudizio_dell_handshake_non_diventa_ambiguo_col_kill() {
    for causa in [
        plenora_core::PlenoraError::InvalidConfiguration("resolver diverso".to_owned()),
        plenora_core::PlenoraError::Protocol("artefatto diverso".to_owned()),
    ] {
        let (_base, radice, padre, dominio) = gerarchia();
        scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
        scrivi_antenato(&padre, 0);
        scrivi_antenato(&radice, 0);
        popolato(&dominio, true);
        let evidenza = evidenza_su(&radice, &dominio);
        let kernel = svuota_al_kill(&dominio);
        let attesa = causa.category();

        let errore = evidenza.rileggi_il_giudizio("worker", causa);
        kernel.join().expect("kernel finto");
        assert_eq!(errore.category(), attesa, "{errore}");
    }
}

/// La stessa categoria dal **canale** — un payload troncato e' `Protocol` —
/// non e' un giudizio: col dominio svuotato solo da `cgroup.kill` l'esito e'
/// ambiguo. La provenienza decide, non la categoria.
#[test]
fn un_protocol_dal_canale_col_kill_e_ambiguo() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, true);
    let evidenza = evidenza_su(&radice, &dominio);
    let kernel = svuota_al_kill(&dominio);

    let errore = evidenza.rileggi_il_fallimento(
        "worker",
        plenora_core::PlenoraError::Protocol("payload troncato".to_owned()),
    );
    kernel.join().expect("kernel finto");
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::Internal,
        "{errore}"
    );
    assert!(errore.to_string().contains("ambiguo"), "{errore}");
}

/// Il giudizio cede comunque all'evidenza che attribuisce.
#[test]
fn il_giudizio_cede_all_oom_attribuito() {
    let (_base, radice, padre, dominio) = gerarchia();
    scrivi_dominio(&dominio, 0, 0, 0, 0, 0);
    scrivi_antenato(&padre, 0);
    scrivi_antenato(&radice, 0);
    popolato(&dominio, false);
    let evidenza = evidenza_su(&radice, &dominio);
    scrivi_dominio(&dominio, 1, 1, 1, 3, 4096);

    let errore = evidenza.rileggi_il_giudizio(
        "worker",
        plenora_core::PlenoraError::Protocol("artefatto diverso".to_owned()),
    );
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{errore}"
    );
}

/// Su una directory che non e' un cgroup `cgroup.kill` non c'e': terminare
/// e' un errore, non la creazione di un file qualunque che fa sembrare
/// riuscita una terminazione mai avvenuta.
#[test]
fn senza_cgroup_kill_terminare_e_un_errore_e_non_crea_file() {
    use super::super::conduzione::Terminatore as _;
    let base = tempfile::tempdir().expect("tempdir");
    let mut terminatore = super::TerminaDominio::nuova(base.path().to_path_buf());
    let errore = terminatore
        .termina()
        .expect_err("senza cgroup.kill non si termina");
    assert!(errore.starts_with("cgroup.kill: "), "{errore}");
    assert!(!base.path().join("cgroup.kill").try_exists().expect("stat"));
}
