//! Il contratto del dispatch del worker, visto dal binario.
//!
//! Questo eseguibile e' anche il worker del profilo isolato: si riconosce
//! perche' `argv[1]` sta nel proprio namespace riservato. I casi puri provano la
//! **regola** del riconoscimento; questi provano che il binario la **usi**.
//!
//! # Perche' non basta la prova pura
//!
//! Perche' fra la regola e il binario c'e' il cablaggio, e il cablaggio si puo'
//! togliere senza che niente diventi rosso. Togliendo il worker dall'elenco
//! delle modalita' interrogate nel `main`, i casi puri resterebbero verdi — la
//! regola c'e' ancora — e anche `-D dead-code` tacerebbe, perche' la facciata e'
//! pubblica e una funzione pubblica non e' mai codice morto.
//!
//! Il difetto si vedrebbe solo a runtime, come un worker che si sente dire
//! «comando sconosciuto» da un supervisore che aspetta un saluto.

use std::process::Command;

mod comune;
use comune::{eseguibile, ricaduta_nel_parser};

/// La variabile del canale, che questi casi tolgono di mezzo.
///
/// Sta scritta qui e non presa dal motore: e' un contratto fra due processi, e
/// un caso che la leggesse dalla stessa costante del codice non si accorgerebbe
/// se quel nome cambiasse.
const VARIABILE: &str = "PLENORA_CANALE";

/// Che cosa il binario ha scritto, unito: l'envelope va su stdout, ma un
/// messaggio che finisse su stderr non deve sfuggire al caso.
///
/// La variabile del canale viene **tolta** dall'ambiente del figlio: questi
/// casi provano il cammino in cui non c'e', e l'ambiente del runner potrebbe
/// averla per conto suo.
fn esegui(argomenti: &[&str]) -> (i32, String) {
    let uscita = Command::new(eseguibile())
        .args(argomenti)
        .env_remove(VARIABILE)
        .output()
        .expect("il binario si esegue");
    let mut testo = String::from_utf8_lossy(&uscita.stdout).into_owned();
    testo.push_str(&String::from_utf8_lossy(&uscita.stderr));
    (uscita.status.code().unwrap_or(-1), testo)
}

/// **La modalita' worker raggiunge il worker**, e fallisce dove deve.
///
/// Senza la variabile del canale il worker non sa dove sono i suoi estremi, e
/// lo dice. E' la prova che il cablaggio esiste: se il `main` non interrogasse
/// il worker, questa riga cadrebbe nel parser della CLI e il messaggio parlerebbe
/// di un comando sconosciuto invece che della variabile.
///
/// La distinzione e' fra due diagnosi. «La variabile non c'e'» dice a un
/// supervisore che cosa ha dimenticato; «comando sconosciuto» lo manda a
/// cercare un errore di digitazione che non c'e'.
#[test]
#[cfg(target_os = "linux")]
fn la_modalita_worker_raggiunge_il_worker() {
    let (codice, testo) = esegui(&["plenora-worker-1"]);
    assert_ne!(codice, 0, "senza canale il worker non puo' riuscire");
    assert!(
        testo.contains(VARIABILE),
        "il rifiuto non nomina la variabile del canale: {testo}"
    );
    assert!(
        !ricaduta_nel_parser(&testo),
        "la riga e' ricaduta nel parser della CLI: {testo}"
    );
}

/// Ogni versione del namespace del worker che non sia quella supportata e' un
/// rifiuto che **nomina** la versione attesa — e non ripete l'argomento.
///
/// # Perche' si guarda anche cio' che il messaggio non dice
///
/// Perche' `argv` e' un ingresso che il chiamante sceglie, e un errore che lo
/// ripetesse porterebbe nei log contenuto arbitrario: ritorni a capo che
/// spezzano una riga in due, o sequenze che un terminale interpreta. Chi ha
/// scritto quella riga sa che cosa ha scritto; cio' che non sa e' **quale
/// versione serve**.
#[test]
#[cfg(target_os = "linux")]
fn le_altre_versioni_del_worker_si_nominano_senza_ripetersi() {
    let ostili = [
        "plenora-worker-0",
        "plenora-worker-2",
        "plenora-worker-",
        "plenora-worker-99-ostile",
    ];
    for versione in ostili {
        let (codice, testo) = esegui(&[versione]);
        assert_ne!(codice, 0, "«{versione}» non puo' riuscire: {testo}");
        assert!(
            testo.contains("plenora-worker-1"),
            "«{versione}»: il rifiuto non nomina la versione attesa: {testo}"
        );
        assert!(
            !ricaduta_nel_parser(&testo),
            "«{versione}» e' ricaduta nel parser della CLI: {testo}"
        );
        // La coda che distingue l'argomento dalla versione attesa non deve
        // comparire: e' la parte che il chiamante ha scelto.
        if let Some(coda) = versione.strip_prefix("plenora-worker-") {
            assert!(
                coda.is_empty() || !testo.contains(coda),
                "«{versione}»: il rifiuto ripete l'argomento («{coda}»): {testo}"
            );
        }
    }
}

/// Il namespace del worker cattura cio' che gli appartiene, e **non** cio' che
/// gli somiglia per caso.
#[test]
#[cfg(target_os = "linux")]
fn il_confine_del_namespace_del_worker_e_quello_dichiarato() {
    for fuori in ["plenora-worker", "worker-1", "plenora_worker-1"] {
        let (codice, testo) = esegui(&[fuori]);
        assert!(
            !testo.contains("modalita' worker non supportata"),
            "«{fuori}» non appartiene al namespace del worker: {testo}"
        );
        assert_ne!(
            codice, 0,
            "«{fuori}» resta un comando sconosciuto per la CLI"
        );
    }
}

/// **Le due modalita' non si rubano le righe**, viste dal binario.
///
/// I casi puri lo provano sulla regola; qui si prova che l'ordine in cui il
/// `main` le interroga non cambi l'esito. Una modalita' che rivendicasse le
/// righe dell'altra nominerebbe la propria versione attesa, mandando chi legge
/// a correggere la cosa sbagliata.
#[test]
#[cfg(target_os = "linux")]
fn ogni_modalita_rifiuta_con_la_propria_versione() {
    let (_, dal_worker) = esegui(&["plenora-worker-9"]);
    assert!(
        dal_worker.contains("plenora-worker-1") && !dal_worker.contains("plenora-spawner"),
        "il rifiuto del worker nomina lo spawner: {dal_worker}"
    );

    let (_, dallo_spawner) = esegui(&["plenora-spawner-9"]);
    assert!(
        dallo_spawner.contains("plenora-spawner-2") && !dallo_spawner.contains("plenora-worker"),
        "il rifiuto dello spawner nomina il worker: {dallo_spawner}"
    );
}
