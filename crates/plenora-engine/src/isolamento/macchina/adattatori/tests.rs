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
