//! I casi dell'ascolto: cio' che vede, e che si lascia sempre raccogliere.
//!
//! Girano su una pipe vera, perche' cio' che si prova qui e' proprio il
//! comportamento su un descrittore: una sorgente finta non si mette in
//! modalita' non bloccante, e la fermabilita' e' esattamente quella proprieta'.

use std::io::Write as _;
use std::time::{Duration, Instant};

use crate::cancellation::CancellationToken;
use crate::protocollo::codifica::codifica;
use crate::protocollo::messaggi::{Annulla, Corpo, Frame, TipoMessaggio};

use super::{Ascoltato, Ascolto};

/// Oltre questa soglia un annullamento non visto e' un difetto, non un momento
/// sfortunato.
///
/// Non e' una garanzia e non appartiene a nessun contratto — nessuno scheduler
/// promette di rimettere un processo sulla CPU entro un tempo — ma su una
/// macchina che non e' in ginocchio due secondi sono tre ordini di grandezza
/// sopra il passo d'attesa del lettore.
const SOGLIA: Duration = Duration::from_secs(2);

/// Il frame di un corpo, pronto da scrivere sulla pipe.
fn byte(corpo: Corpo) -> Vec<u8> {
    codifica(&Frame::nuovo(corpo)).expect("il frame si codifica")
}

/// Un annullamento come il supervisore lo manderebbe.
fn annulla() -> Vec<u8> {
    byte(Corpo::Annulla(Annulla {
        motivo: "richiesto dal caso".to_owned(),
    }))
}

/// **Un `Annulla` cancella il token, e il lavoro cooperativo finisce da se'.**
///
/// # Che cosa prova, e perche' e' il caso che conta
///
/// Che l'annullamento arrivi **al lavoro** e non solo al lettore. La leva e'
/// una sola — lo stesso `CancellationToken` che entra nel `RuntimeContext` — e
/// un lettore che la tirasse su un token diverso lascerebbe il lavoro correre
/// fino in fondo: il supervisore dovrebbe allora forzare la terminazione, cioe'
/// uccidere un processo che avrebbe potuto fermarsi da solo.
///
/// Il «lavoro» qui e' un ciclo che guarda il token, com'e' un confine
/// cooperativo dell'executor. Nessuno lo interrompe da fuori: finisce perche' il
/// token e' cancellato, e il caso fallisce se non finisce entro la soglia.
#[test]
fn un_annullamento_ferma_il_lavoro_cooperativo_senza_forzature() {
    let (lettore, mut scrittore) = std::io::pipe().expect("la pipe si apre");
    let annullamento = CancellationToken::new();
    let ascolto = Ascolto::comincia(lettore, annullamento.clone()).expect("l'ascolto comincia");

    scrittore.write_all(&annulla()).expect("l'annulla parte");
    scrittore.flush().expect("e arriva");

    // Il lavoro cooperativo: gira finche' il token non dice basta.
    let cominciato = Instant::now();
    let mut giri: u64 = 0;
    while !annullamento.is_cancelled() {
        assert!(
            cominciato.elapsed() < SOGLIA,
            "dopo {giri} giri il lavoro non ha visto l'annullamento: la leva non e' la stessa"
        );
        giri += 1;
        std::thread::sleep(Duration::from_millis(1));
    }

    assert!(
        matches!(ascolto.ferma_e_raccogli(), Ascoltato::Annullamento),
        "l'ascolto deve dichiarare di aver visto l'annullamento"
    );
}

/// **Un messaggio che non e' un `Annulla` non sparisce.**
///
/// Dopo l'incarico non c'e' nient'altro da dire: qualunque altro tipo e' una
/// violazione della sequenza, e il caso pretende che il tipo arrivi a chi
/// decide invece di essere scartato.
#[test]
fn un_messaggio_fuori_sequenza_si_nomina() {
    let (lettore, mut scrittore) = std::io::pipe().expect("la pipe si apre");
    let annullamento = CancellationToken::new();
    let ascolto = Ascolto::comincia(lettore, annullamento.clone()).expect("l'ascolto comincia");

    let progresso = byte(Corpo::Progresso(crate::protocollo::messaggi::Progresso {
        righe: 1,
        batch: 1,
        nodi_completati: 0,
    }));
    scrittore.write_all(&progresso).expect("il frame parte");
    scrittore.flush().expect("e arriva");

    // Si da' al lettore il tempo di leggere il frame che gli e' stato scritto,
    // poi si raccoglie: `ferma_e_raccogli` consuma, quindi non si riprova.
    // L'attesa e' due ordini di grandezza sopra il passo del lettore, e la
    // soglia dice quando smettere di considerarla sfortuna.
    let cominciato = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    assert!(cominciato.elapsed() < SOGLIA, "il lettore non ha concluso");
    let visto = ascolto.ferma_e_raccogli();

    let Ascoltato::FuoriSequenza(tipo) = visto else {
        panic!("un progresso dopo l'incarico e' fuori sequenza: {visto:?}");
    };
    assert_eq!(tipo, TipoMessaggio::Progresso);
    assert!(
        !annullamento.is_cancelled(),
        "solo un annulla annulla: un altro messaggio non deve tirare la leva"
    );
}

/// **La chiusura del supervisore e' una fine, non un guasto.**
///
/// Un supervisore che non intende annullare puo' chiudere la propria direzione
/// dopo l'incarico. Trasformarlo in un errore renderebbe fallito un lavoro
/// riuscito.
#[test]
fn la_chiusura_del_canale_non_e_un_guasto() {
    let (lettore, scrittore) = std::io::pipe().expect("la pipe si apre");
    let annullamento = CancellationToken::new();
    let ascolto = Ascolto::comincia(lettore, annullamento.clone()).expect("l'ascolto comincia");

    drop(scrittore);

    let cominciato = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    assert!(cominciato.elapsed() < SOGLIA);
    assert!(
        matches!(ascolto.ferma_e_raccogli(), Ascoltato::FineDelCanale),
        "l'EOF si dichiara come fine, non come errore"
    );
    assert!(!annullamento.is_cancelled());
}

/// **Un lettore che non ha visto niente si lascia comunque raccogliere.**
///
/// # Perche' e' il caso che tiene in piedi tutti gli altri
///
/// Perche' e' il cammino normale: il lavoro finisce, nessun annullamento
/// arriva, e il lettore e' fermo dentro la propria attesa. Se non si fermasse,
/// il processo non uscirebbe — e un worker che non esce e' proprio cio' che il
/// supervisore deve poi uccidere.
///
/// Il caso non ha bisogno di una soglia: se l'arresto non funzionasse,
/// `ferma_e_raccogli` non tornerebbe affatto e sarebbe il runner a dirlo.
#[test]
fn un_ascolto_muto_si_ferma_e_si_raccoglie() {
    let (lettore, scrittore) = std::io::pipe().expect("la pipe si apre");
    let annullamento = CancellationToken::new();
    let ascolto = Ascolto::comincia(lettore, annullamento.clone()).expect("l'ascolto comincia");

    // Lo scrittore resta **vivo**: senza EOF, un lettore non fermabile
    // aspetterebbe per sempre. E' esattamente la condizione che il caso
    // vuole.
    assert!(
        matches!(ascolto.ferma_e_raccogli(), Ascoltato::Fermato),
        "l'arresto e' una nostra decisione, e si dichiara come tale"
    );
    drop(scrittore);
    assert!(!annullamento.is_cancelled());
}
