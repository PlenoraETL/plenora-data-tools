//! Il contratto del dispatch dello spawner, visto dal binario.
//!
//! Questo eseguibile e' anche lo spawner del profilo isolato: si riconosce
//! perche' `argv[1]` sta nel namespace riservato. I casi qui sotto fissano il
//! confine fra le due nature — quando il programma e' uno spawner e quando e'
//! la CLI di sempre — perche' e' un confine che si sposta in silenzio.
//!
//! # Che cosa **non** si prova qui, e dove si prova
//!
//! Che lo spawner non costruisca il pool di rayon. Il primo passo della
//! sequenza conta `/proc/self/task` e rifiuta se i task non sono uno, quindi
//! la prova esiste — ma per arrivarci serve un dominio `cgroup2` vero, che un
//! test di integrazione non ha. La prova sta nel gate ostile
//! (`scripts/verifica_isolamento_linux.sh`), che riporta `sentinella_task`.
//!
//! Qui si prova la meta' che si puo' provare ovunque: che una riga del
//! namespace riservato **non torni mai** al parser degli argomenti.

use std::process::Command;

const fn eseguibile() -> &'static str {
    env!("CARGO_BIN_EXE_plenora-data-tools")
}

/// Che cosa il binario ha scritto, unito: l'envelope va su stdout, ma un
/// messaggio che finisse su stderr non deve sfuggire al caso.
fn esegui(argomenti: &[&str]) -> (i32, String) {
    let uscita = Command::new(eseguibile())
        .args(argomenti)
        .output()
        .expect("il binario si esegue");
    let mut testo = String::from_utf8_lossy(&uscita.stdout).into_owned();
    testo.push_str(&String::from_utf8_lossy(&uscita.stderr));
    (uscita.status.code().unwrap_or(-1), testo)
}

/// Un comando ordinario resta quello di sempre.
///
/// E' la meta' che si dimentica di provare: un dispatch che catturasse troppo
/// trasformerebbe ogni invocazione in un tentativo di spawner, e il difetto si
/// vedrebbe solo dal lato di chi usa la CLI.
#[test]
fn un_comando_ordinario_prosegue_invariato() {
    let (codice, testo) = esegui(&["catalog"]);
    assert_eq!(codice, 0, "«catalog» non e' riuscito: {testo}");
    assert!(
        !testo.contains("spawner"),
        "un comando ordinario non deve nominare lo spawner: {testo}"
    );
}

/// Una richiesta malformata fallisce **nello spawner**, e non ricade nel
/// parser della CLI.
///
/// La distinzione e' fra due diagnosi. «Manca il separatore» dice a un
/// supervisore che cosa correggere; «comando sconosciuto» lo manda a cercare
/// un errore di digitazione che non c'e'.
#[test]
#[cfg(target_os = "linux")]
fn una_richiesta_malformata_fallisce_nello_spawner() {
    let (codice, testo) = esegui(&["plenora-spawner-2"]);
    assert_ne!(codice, 0, "una richiesta malformata non puo' riuscire");
    assert!(
        testo.contains("spawner"),
        "l'errore non viene dallo spawner: {testo}"
    );
    assert!(
        !testo.contains("sconosciuto") && !testo.contains("unknown"),
        "la richiesta e' ricaduta nel parser della CLI: {testo}"
    );
}

/// Ogni versione del namespace riservato che non sia quella supportata e' un
/// rifiuto che **nomina** la versione attesa.
#[test]
#[cfg(target_os = "linux")]
fn le_altre_versioni_del_namespace_sono_rifiuti_che_si_nominano() {
    for versione in [
        "plenora-spawner-1",
        "plenora-spawner-3",
        "plenora-spawner-99",
        "plenora-spawner-",
    ] {
        let (codice, testo) = esegui(&[versione]);
        assert_ne!(codice, 0, "«{versione}» non puo' riuscire: {testo}");
        assert!(
            testo.contains("plenora-spawner-2"),
            "«{versione}»: il rifiuto non nomina la versione attesa: {testo}"
        );
        assert!(
            !testo.contains("sconosciuto") && !testo.contains("unknown"),
            "«{versione}» e' ricaduta nel parser della CLI: {testo}"
        );
    }
}

/// Il namespace riservato cattura anche cio' che gli somiglia soltanto in
/// parte, e **non** cio' che gli somiglia per caso.
#[test]
#[cfg(target_os = "linux")]
fn il_confine_del_namespace_e_quello_dichiarato() {
    // Dentro: prefisso esatto, versione qualunque.
    let (dentro, testo_dentro) = esegui(&["plenora-spawner-xyz"]);
    assert_ne!(dentro, 0);
    assert!(
        testo_dentro.contains("plenora-spawner-2"),
        "dovrebbe essere un rifiuto di versione: {testo_dentro}"
    );

    // Fuori: somiglia, ma il prefisso non c'e'.
    for fuori in ["plenora-spawner", "spawner-2", "plenora_spawner-2"] {
        let (codice, testo) = esegui(&[fuori]);
        assert!(
            !testo.contains("versione della richiesta non supportata"),
            "«{fuori}» non appartiene al namespace riservato: {testo}"
        );
        assert_ne!(
            codice, 0,
            "«{fuori}» resta un comando sconosciuto per la CLI"
        );
    }
}
