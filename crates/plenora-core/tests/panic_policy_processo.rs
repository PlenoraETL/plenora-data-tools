//! La politica dei panici va verificata su un PROCESSO, non in memoria:
//! l'hook di `std` stampa su stderr prima dell'unwinding, quindi nessun
//! `catch_unwind` dentro lo stesso processo puo' osservare se il payload sia
//! stato pubblicato o no. Il test si ri-esegue quindi come figlio.
//!
//! Il figlio e' questo stesso binario di test: `current_exe` piu' una
//! variabile d'ambiente che seleziona il ramo. Nessuna dipendenza aggiuntiva,
//! nessun percorso hardcoded, funziona su tutte le piattaforme che il
//! workspace supporta.

use std::process::Command;

/// Testo che il payload del panico deve contenere e che stderr non deve
/// contenere mai. Sta per un valore di riga finito in un `assert_eq!` di una
/// dipendenza.
const SEGRETO: &str = "valore-riservato-che-non-deve-comparire";

const VARIABILE: &str = "PLENORA_TEST_PANIC_POLICY";

/// Ramo figlio: installa la politica indicata dalla variabile e va in panico
/// con un payload che contiene il segreto.
fn ramo_figlio(politica: &str) -> ! {
    use plenora_core::panic_policy::{install, PanicPolicy};
    match politica {
        "silent" => {
            install(PanicPolicy::Silent);
        }
        "sanitized" => {
            install(PanicPolicy::Sanitized);
        }
        // Qualcuno installa la NOSTRA politica e poi un terzo componente
        // chiama `std::panic::set_hook` per conto suo. E' il limite reale del
        // modulo: `Once` governa le chiamate a `install`, non l'API di `std`.
        "scavalcata" => {
            install(PanicPolicy::Silent);
            std::panic::set_hook(Box::new(|info| {
                // Un hook ostile o semplicemente ignaro: ripubblica tutto.
                eprintln!("hook di terze parti: {info}");
            }));
        }
        // "default": nessuna installazione, hook di `std`. E' il caso di
        // controllo che dimostra che il difetto esiste davvero.
        _ => {}
    }
    panic!("panico simulato con {SEGRETO} nel messaggio");
}

fn esegui_figlio(politica: &str) -> (String, String) {
    let exe = std::env::current_exe().expect("current_exe");
    let output = Command::new(exe)
        .arg("--exact")
        .arg("il_ramo_figlio_non_e_un_test_vero")
        // Senza `--nocapture` la libreria di test intercetta lo stderr del
        // thread e l'hook di `std` scrive nel suo buffer invece che sul
        // canale reale: il figlio non somiglierebbe piu' al processo che
        // vogliamo osservare. Il nostro hook scrive su `stderr()` diretto e
        // sfuggirebbe alla cattura comunque — motivo in piu' per toglierla,
        // cosi' i tre rami sono confrontabili.
        .arg("--nocapture")
        .env(VARIABILE, politica)
        .output()
        .expect("il figlio deve partire");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Punto d'ingresso del figlio. Non e' un test: se la variabile non c'e',
/// esce subito senza fare nulla, cosi' il padre puo' invocarlo per nome.
#[test]
fn il_ramo_figlio_non_e_un_test_vero() {
    if let Ok(politica) = std::env::var(VARIABILE) {
        ramo_figlio(&politica);
    }
}

#[test]
fn l_hook_di_default_pubblica_il_payload_del_panico() {
    // Premessa del difetto. Se un giorno `std` smettesse di stampare il
    // payload, questo test fallirebbe e ci direbbe che la politica non serve
    // piu': e' informazione utile, non un falso allarme.
    let (_, stderr) = esegui_figlio("default");
    assert!(
        stderr.contains(SEGRETO),
        "premessa: senza politica il payload finisce su stderr; stderr: {stderr}"
    );
}

#[test]
fn la_politica_silent_non_pubblica_nulla() {
    let (stdout, stderr) = esegui_figlio("silent");
    assert!(
        !stderr.contains(SEGRETO),
        "il payload non deve comparire su stderr; stderr: {stderr}"
    );
    assert!(!stdout.contains(SEGRETO), "ne' su stdout; stdout: {stdout}");
}

#[test]
fn la_politica_sanitized_pubblica_la_forma_ma_non_il_contenuto() {
    let (stdout, stderr) = esegui_figlio("sanitized");
    assert!(
        !stderr.contains(SEGRETO) && !stdout.contains(SEGRETO),
        "il payload non deve comparire su nessun canale; stderr: {stderr}"
    );
    assert!(
        stderr.contains("payload statico") || stderr.contains("payload dinamico"),
        "l'embedder deve pero' sapere che un panico c'e' stato; stderr: {stderr}"
    );
    assert!(
        stderr.contains("panic_policy_processo.rs"),
        "e deve sapere dove: la posizione e' proprieta' del programma, non del dato; \
         stderr: {stderr}"
    );
}

#[test]
fn la_politica_non_sopravvive_a_un_set_hook_di_terze_parti() {
    // La politica NON e' inamovibile: `Once` impedisce solo una seconda
    // chiamata a `install`, e `std::panic::set_hook` resta pubblico per
    // chiunque, quindi un hook di terze parti installato dopo il nostro
    // puo' ripubblicare il payload.
    //
    // Il test rende il limite ESEGUIBILE invece che dichiarato: se un domani
    // qualcuno credesse di aver reso l'hook inamovibile, questo test glielo
    // smentirebbe — o, se davvero lo diventasse, fallirebbe e chiederebbe di
    // aggiornare la documentazione. In entrambi i casi il documento e il
    // codice restano allineati.
    let (_, stderr) = esegui_figlio("scavalcata");
    assert!(
        stderr.contains(SEGRETO),
        "il limite dichiarato e' reale: un hook di terze parti installato dopo il nostro \
         ripubblica il payload; stderr: {stderr}"
    );
}
