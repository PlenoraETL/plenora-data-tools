//! Che cosa esce davvero da **stderr** quando la validazione OGC panica.
//!
//! # Perche' su un processo e non in memoria
//!
//! Perche' l'hook di `std` stampa **prima** dell'unwinding: nessun
//! `catch_unwind` dentro lo stesso processo puo' osservare se il payload sia
//! stato pubblicato. Un caso che guardasse solo il valore reso proverebbe che
//! l'errore e' pulito, non che l'uscita lo sia — ed e' l'uscita a essere il
//! canale che pubblica.
//!
//! # Perche' un binario a parte
//!
//! Perche' l'hook e' **stato globale del processo**: installarlo nel binario
//! che ospita gli altri casi della barriera li legherebbe a questo, e
//! l'ordine in cui girano deciderebbe che cosa vedono.
//!
//! Il figlio e' questo stesso binario di test, scelto con una variabile
//! d'ambiente: nessuna dipendenza aggiuntiva e nessun percorso fissato.

use std::process::Command;

/// Il reperto del 5 settembre 2026: fa panicare `check_validation` di `geo`
/// con un messaggio che contiene le coordinate.
const REPERTO: &[u8] = &[
    1, 6, 0, 0, 0, 3, 0, 0, 0, 1, 3, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 1, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 5, 46, 254, 255, 255, 253, 15, 0, 0, 16, 64, 64, 64, 64, 0, 0, 1, 3, 0, 0, 0,
    1, 0, 0, 0, 7, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 0, 0, 0, 4, 0, 4, 0, 0, 8, 116,
    116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 0, 1,
    0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 5, 46, 254,
    255, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 0, 0, 0, 0, 0, 4, 0,
    0, 8, 116, 116, 116, 116, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

const VARIABILE: &str = "PLENORA_TEST_BARRIERA_PRIVACY";

/// Frammenti del payload di `geo` che stderr non deve mai portare.
///
/// Le prime due sono le coordinate che quel panico nomina su **questo**
/// ingresso; le altre sono le parole della sua diagnosi. Non e' un elenco
/// esaustivo, e infatti il caso non si ferma qui: pretende anche che la riga
/// pubblicata sia **esattamente** quella sanitizzata, che e' l'affermazione
/// forte.
const FRAMMENTI: [&str; 5] = [
    "e-270",
    "e-312",
    "COORD",
    "topology position conflict",
    "right_location",
];

/// Che cosa il figlio ha stampato **e** com'e' finito.
///
/// Le due cose insieme, perche' separate non dicono niente: una riga corretta
/// seguita da un fallimento lascerebbe verde un caso che guarda solo la riga,
/// e il figlio starebbe fallendo per un motivo che nessuno legge.
struct Uscita {
    stderr: String,
    riuscita: bool,
    stato: String,
}

fn esegui_figlio(politica: &str) -> Uscita {
    let exe = std::env::current_exe().expect("current_exe");
    let uscita = Command::new(exe)
        .arg("--exact")
        .arg("il_ramo_figlio_non_e_un_test_vero")
        // Senza `--nocapture` la libreria di test intercetta lo stderr del
        // thread, e l'hook di `std` scriverebbe nel suo buffer invece che sul
        // canale reale: il figlio non somiglierebbe piu' al processo che si
        // vuole osservare.
        .arg("--nocapture")
        .env(VARIABILE, politica)
        .output()
        .expect("il figlio parte");
    Uscita {
        stderr: String::from_utf8_lossy(&uscita.stderr).into_owned(),
        riuscita: uscita.status.success(),
        stato: format!("{}", uscita.status),
    }
}

impl Uscita {
    /// Pretende che il figlio sia arrivato in fondo, e rende il suo stderr.
    fn stderr_di_una_corsa_riuscita(&self, quale: &str) -> &str {
        assert!(
            self.riuscita,
            "il figlio «{quale}» non e' arrivato in fondo ({}); \
             qualunque cosa abbia stampato non dimostra niente:\n{}",
            self.stato, self.stderr
        );
        &self.stderr
    }
}

/// La riga sanitizzata, verificata per **intero** invece che a frammenti.
///
/// Un elenco di sottostringhe assenti prova solo che quelle mancano. Qui si
/// pretende la forma completa: un'unica riga, il prefisso, la posizione — che
/// nomina il *punto* della dipendenza, non il dato — e la coda. Se l'hook
/// stampasse una riga in piu', o una diversa, il caso lo vedrebbe.
fn pretendi_la_riga_sanitizzata(stderr: &str) {
    let righe: Vec<&str> = stderr.lines().filter(|r| !r.trim().is_empty()).collect();
    assert_eq!(
        righe.len(),
        1,
        "atteso esattamente una riga su stderr, trovate {}:\n{stderr}",
        righe.len()
    );
    let riga = righe[0];
    let Some(resto) = riga.strip_prefix("plenora: panico interno a ") else {
        panic!("prefisso inatteso: {riga}");
    };
    let Some((posizione, coda)) = resto.split_once(" (") else {
        panic!("manca la forma del payload: {riga}");
    };
    // `geo` panica con `format!`, quindi il payload e' una `String`: la forma
    // e' quella dinamica. Pretenderla per intero, e non «contiene payload»,
    // e' cio' che rende il caso capace di vedere un cambio di formato.
    assert_eq!(
        coda,
        "payload dinamico (contenuto non pubblicato)); \
         nessun contenuto del payload viene pubblicato",
        "coda inattesa: {riga}"
    );
    // La posizione nomina il punto in `geo`, con riga e colonna: e' il
    // riferimento al codice, non un dato dell'ingresso.
    assert!(
        posizione.contains("edge_end_bundle_star.rs:"),
        "posizione inattesa: {posizione}"
    );
}

/// **Con la politica sanitizzata, stderr non porta il payload di `geo`.**
///
/// Pretende due cose insieme: che i frammenti noti non ci siano, e che la riga
/// pubblicata sia quella del formato sanitizzato. La seconda e' quella che
/// regge anche se il messaggio della dipendenza cambia.
///
/// Vale dove il panico esiste: e' un `debug_assert!` di `geo`, quindi solo con
/// le asserzioni di debug attive. Il profilo opposto non e' saltato in silenzio
/// — lo copre [`senza_asserzioni_di_debug_il_reperto_non_panica`], che ne
/// verifica la ragione.
#[cfg(debug_assertions)]
#[test]
fn con_la_politica_sanitizzata_stderr_non_porta_il_payload() {
    let uscita = esegui_figlio("sanitized");
    let stderr = uscita.stderr_di_una_corsa_riuscita("sanitized");
    for frammento in FRAMMENTI {
        assert!(
            !stderr.contains(frammento),
            "stderr pubblica «{frammento}»:\n{stderr}"
        );
    }
    pretendi_la_riga_sanitizzata(stderr);
}

/// **Senza politica, il payload esce: il difetto che la politica chiude.**
///
/// E' il caso di controllo. Senza di lui il primo proverebbe che stderr e'
/// pulito, non che sia la politica a pulirlo: un hook che non stampasse nulla
/// lo farebbe passare ugualmente.
#[cfg(debug_assertions)]
#[test]
fn senza_politica_il_payload_esce_davvero() {
    let uscita = esegui_figlio("default");
    let stderr = uscita.stderr_di_una_corsa_riuscita("default");
    assert!(
        FRAMMENTI.iter().any(|frammento| stderr.contains(frammento)),
        "l'hook predefinito deve pubblicare il payload, o il confronto non \
         dimostra niente:\n{stderr}"
    );
}

/// **Senza asserzioni di debug il reperto non panica affatto.**
///
/// E' la ragione per cui i due casi qui sopra non esistono in questo profilo,
/// verificata invece che dichiarata. Un `#[cfg]` senza questo caso lascerebbe
/// credere che la verifica ci sia sempre; qui si pretende il fatto che la
/// rende superflua — `geo` conclude, e il rifiuto e' ordinario.
///
/// Vale anche come sentinella: il giorno che quel panico comparisse in
/// `release`, questo caso diventerebbe rosso invece di tacere.
#[cfg(not(debug_assertions))]
#[test]
fn senza_asserzioni_di_debug_il_reperto_non_panica() {
    let uscita = esegui_figlio("default");
    let stderr = uscita.stderr_di_una_corsa_riuscita("default");
    // Il figlio ha gia' preteso la categoria attesa per questo profilo, e vi e'
    // arrivato: qui resta da verificare che non abbia stampato nulla, cioe'
    // che nessun panico sia avvenuto.
    assert!(
        stderr.trim().is_empty(),
        "senza asserzioni di debug non deve esserci alcun panico, e invece \
         stderr porta:\n{stderr}"
    );
}

/// Il ramo figlio: non e' un caso, e il nome lo dice.
///
/// Gira solo quando la variabile e' presente; altrimenti esce subito, cosi'
/// una corsa normale della batteria non lo esercita.
///
/// # Perche' il figlio verifica invece di limitarsi a stampare
///
/// Perche' altrimenti il padre osserverebbe stderr senza sapere che cosa lo ha
/// prodotto. Se `install` fallisse in silenzio, il figlio userebbe l'hook
/// predefinito e il caso «sanitizzato» leggerebbe la riga sbagliata; se
/// `geometry_from_wkb` non arrivasse alla validazione, non ci sarebbe alcun
/// panico da osservare. Il figlio pretende entrambe le cose, e il suo stato
/// d'uscita le porta al padre.
#[test]
fn il_ramo_figlio_non_e_un_test_vero() {
    use plenora_core::ErrorCategory;

    let Ok(politica) = std::env::var(VARIABILE) else {
        return;
    };
    if politica == "sanitized" {
        assert!(
            plenora_core::panic_policy::install(
                plenora_core::panic_policy::PanicPolicy::Sanitized
            ),
            "la politica sanitizzata non e' stata installata: cio' che segue \
             uscirebbe dall'hook predefinito"
        );
    }
    // Il panico avviene dentro `geo`, raggiunto dalla porta di produzione: e'
    // il percorso vero, non una simulazione con un `panic!` scritto qui.
    let errore = plenora_kernels_geo::geometry_from_wkb(REPERTO)
        .expect_err("il reperto non e' una geometria accettabile");
    // La categoria attesa per il profilo: con le asserzioni di debug attive il
    // panico c'e' ed e' la barriera a renderlo `Internal`; senza, `geo`
    // conclude e giudica l'ingresso.
    let attesa = if cfg!(debug_assertions) {
        ErrorCategory::Internal
    } else {
        ErrorCategory::InvalidPlan
    };
    assert_eq!(
        errore.category(),
        attesa,
        "categoria inattesa per questo profilo — {errore}"
    );
}
