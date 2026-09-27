//! I casi dei produttori: cosa accodano, e quando smettono.
//!
//! Girano senza processi e senza cgroup. Il lettore legge da un vettore di
//! byte, il sorvegliante da un osservatore finto, l'orologio da tempi corti:
//! cio' che si prova e' **la regola**, e provarla con un worker vero la
//! legherebbe allo scheduler.

use std::time::Duration;

use crate::protocollo::codifica::codifica;
use crate::protocollo::messaggi::{
    ConteggiDichiarati, Corpo, DigestArtefatto, EsitoWorkerSulFilo, Frame, Progresso,
};

use super::super::coda::{apri, Coda, Produttore};
use super::super::{Fatto, Ruolo};
use super::{
    avvia_lettore, avvia_orologio, avvia_sorvegliante, Annullatore, CanaleOperativo, Difetto,
    EsitoDellAnnullamento, Osservatore,
};

/// Il canale operativo dei casi: gli stessi byte, dopo un accordo vero.
///
/// L'accordo non si finge: si ottiene facendo l'handshake completo, che e'
/// l'unico modo di avere un [`HandshakeAccettato`]. E' proprio cio' che il tipo
/// impone anche in produzione — i casi non hanno una porta di servizio.
fn canale(byte: Vec<u8>) -> CanaleOperativo<std::io::Cursor<Vec<u8>>> {
    let (accordo, _worker) = crate::protocollo::handshake::tests::giro_nominale();
    CanaleOperativo::dopo_l_accordo(std::io::Cursor::new(byte), accordo)
}

/// I byte di una conversazione.
fn filo(corpi: Vec<Corpo>) -> Vec<u8> {
    let mut byte = Vec::new();
    for corpo in corpi {
        byte.extend(codifica(&Frame::nuovo(corpo)).expect("il frame si codifica"));
    }
    byte
}

fn progresso(righe: u64) -> Corpo {
    Corpo::Progresso(Progresso {
        righe,
        batch: 1,
        nodi_completati: 1,
    })
}

fn esito() -> Corpo {
    Corpo::Esito(Box::new(EsitoWorkerSulFilo::Successo {
        digest_artefatto: DigestArtefatto {
            algoritmo: "sha256".to_owned(),
            valore: "0".repeat(64),
        },
        conteggi: ConteggiDichiarati { righe: 3, batch: 1 },
    }))
}

/// I fatti raccolti dopo che tutti i produttori sono finiti.
fn raccogli(coda: Coda) -> Vec<Fatto> {
    let (fatti, difetto) = coda.chiudi_e_drena();
    assert_eq!(difetto, None, "il canale deve disconnettersi");
    fatti
}

/// Come si chiama un fatto, per contarlo e per cercarlo.
///
/// Un nome per forma invece di un elenco di confronti: cosi' i casi dicono
/// «quanti progressi» e non ripetono ogni volta come si riconosce un progresso.
fn nome_del_fatto(fatto: &Fatto) -> &'static str {
    match fatto {
        Fatto::MessaggioDalWorker(corpo) => match corpo.as_ref() {
            Corpo::Progresso(_) => "progresso",
            Corpo::Esito(_) => "esito",
            _ => "altro messaggio",
        },
        Fatto::FineDelCanale => "fine",
        Fatto::CanaleInterrotto(_) => "interrotto",
        Fatto::UscitaDelWorker(_) => "uscita",
        Fatto::DominioQuiescente => "quiescente",
        Fatto::EvidenzaDelDominio(_) => "evidenza",
        Fatto::TempoScaduto => "tempo",
        Fatto::CancellazioneRichiesta => "cancellazione",
        Fatto::OsservazioneImpossibile { .. } => "impossibile",
    }
}

/// Quante volte un fatto di una certa forma compare.
fn quanti(fatti: &[Fatto], quale: &str) -> usize {
    fatti
        .iter()
        .filter(|fatto| nome_del_fatto(fatto) == quale)
        .count()
}

/// Il primo fatto di una forma, per guardarne dentro.
fn primo<'a>(fatti: &'a [Fatto], quale: &str) -> Option<&'a Fatto> {
    fatti.iter().find(|fatto| nome_del_fatto(fatto) == quale)
}

// --- il lettore --------------------------------------------------------------

/// **Il progresso si coalesce**: cento rapporti, un fatto solo.
///
/// E' la proprieta' su cui poggia il budget: altrimenti lo spazio in coda lo
/// sceglierebbe il worker, tenendo fuori una cancellazione. Il fatto porta
/// **l'ultimo totale**, non una somma: i contatori sono cumulativi, e
/// sommarli non conterebbe niente e rischierebbe il traboccamento.
#[test]
fn cento_progressi_diventano_un_fatto_solo_con_l_ultimo_totale() {
    let (coda, fascio) = apri();
    let byte = filo((1..=100).map(progresso).collect());
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "progresso"), 1, "un fatto, non cento");
    assert_eq!(quanti(&fatti, "fine"), 1);

    let Some(Fatto::MessaggioDalWorker(corpo)) = primo(&fatti, "progresso") else {
        panic!("il progresso coalesciuto non c'e'");
    };
    let Corpo::Progresso(conservato) = corpo.as_ref() else {
        panic!("non e' un progresso");
    };
    assert_eq!(conservato.righe, 100, "l'ultimo totale, non la somma");
    assert_eq!(conservato.batch, 1);
}

/// **Un contatore che torna indietro e' una violazione del protocollo.**
///
/// I contatori sono totali, e un totale non torna indietro: l'altro lato non
/// segue il contratto. Il lettore accoda **un** fatto e smette, come per ogni
/// altra rottura della sequenza.
#[test]
fn un_contatore_che_torna_indietro_ferma_il_lettore() {
    let (coda, fascio) = apri();
    // Cresce, cresce, e poi arretra.
    let byte = filo(vec![progresso(10), progresso(20), progresso(15)]);
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "interrotto"), 1);
    assert_eq!(quanti(&fatti, "fine"), 0, "una violazione non e' una fine");

    let Some(Fatto::CanaleInterrotto(motivo)) = primo(&fatti, "interrotto") else {
        panic!("manca il fatto di protocollo");
    };
    assert!(motivo.contains("da 20 a 15"), "{motivo}");
    assert!(motivo.contains("non torna indietro"), "{motivo}");

    // E cio' che si e' osservato prima non si perde: il progresso valido
    // arrivato fin li' resta.
    assert_eq!(quanti(&fatti, "progresso"), 1);
}

/// Contatori **fermi** non sono una regressione.
///
/// Due rapporti identici sono leciti: il worker dice «sono ancora qui, e sono
/// allo stesso punto». Un controllo scritto con `<=` invece che con `<` li
/// rifiuterebbe, e rifiuterebbe una conversazione valida.
#[test]
fn contatori_fermi_non_sono_una_regressione() {
    let (coda, fascio) = apri();
    let byte = filo(vec![progresso(10), progresso(10), progresso(10)]);
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "interrotto"), 0);
    assert_eq!(quanti(&fatti, "fine"), 1);
}

/// Un esito arriva, e la fine dopo di lui.
#[test]
fn l_esito_arriva_e_poi_la_fine() {
    let (coda, fascio) = apri();
    let byte = filo(vec![progresso(7), esito()]);
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "esito"), 1);
    assert_eq!(quanti(&fatti, "progresso"), 1);
    assert_eq!(quanti(&fatti, "fine"), 1);
    assert_eq!(quanti(&fatti, "interrotto"), 0);
}

/// **Un secondo esito e' fuori sequenza**: un fatto di protocollo, e si smette.
///
/// Non uno per messaggio inaspettato. Se il worker ne mandasse mille, mille
/// fatti riempirebbero la coda — cioe' il worker deciderebbe quanto spazio
/// occupare, che e' precisamente cio' da cui il budget protegge.
#[test]
fn un_secondo_esito_ferma_il_lettore_con_un_fatto_solo() {
    let (coda, fascio) = apri();
    // Dieci esiti in fila: il primo e' buono, gli altri nove sono di troppo.
    let mut corpi = vec![esito()];
    for _ in 0..9 {
        corpi.push(esito());
    }
    let byte = filo(corpi);
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "esito"), 1, "il primo passa");
    assert_eq!(
        quanti(&fatti, "interrotto"),
        1,
        "e per gli altri nove **un** fatto solo"
    );
    assert_eq!(
        quanti(&fatti, "fine"),
        0,
        "una sequenza rotta non e' una fine"
    );

    let Some(Fatto::CanaleInterrotto(motivo)) = primo(&fatti, "interrotto") else {
        panic!("manca il fatto di protocollo");
    };
    assert!(motivo.contains("fuori sequenza"), "{motivo}");
    assert!(motivo.contains("gia' conclusa"), "{motivo}");
}

/// Un progresso **dopo** l'esito e' fuori sequenza quanto un secondo esito.
///
/// «Fuori sequenza» non e' una proprieta' del messaggio ma del momento: lo
/// stesso progresso, prima, va benissimo.
#[test]
fn un_progresso_dopo_l_esito_e_fuori_sequenza() {
    let (coda, fascio) = apri();
    let byte = filo(vec![esito(), progresso(1)]);
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "interrotto"), 1);
    assert_eq!(quanti(&fatti, "fine"), 0);
}

/// Byte a meta' frame: il canale e' **interrotto**, non finito.
#[test]
fn un_frame_troncato_interrompe_e_non_finisce() {
    let (coda, fascio) = apri();
    let mut byte = filo(vec![esito()]);
    byte.truncate(byte.len() / 2);
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "interrotto"), 1);
    assert_eq!(quanti(&fatti, "fine"), 0, "un troncamento non e' un EOF");
    assert_eq!(quanti(&fatti, "esito"), 0);
}

/// **Il lettore non supera mai il proprio budget.**
///
/// Cento progressi e dieci esiti: qualunque cosa il worker mandi, i fatti che
/// ne escono stanno nel budget del lettore.
#[test]
fn il_lettore_non_supera_mai_il_budget() {
    let (coda, fascio) = apri();
    let mut corpi: Vec<Corpo> = (1..=100).map(progresso).collect();
    for _ in 0..10 {
        corpi.push(esito());
    }
    let byte = filo(corpi);
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert!(
        fatti.len() <= Produttore::Lettore.budget(),
        "il lettore ha accodato {} fatti, oltre i {} del budget",
        fatti.len(),
        Produttore::Lettore.budget()
    );
}

/// **L'esito non puo' essere sorpassato dalla fine.**
///
/// Li accoda lo stesso produttore da un solo `SyncSender`, e la coda FIFO
/// conserva l'ordine. Senza, un consumatore potrebbe concludere «morto senza
/// esito» con l'esito in coda: e' cio' che distingue la riga 2 dalla riga 4
/// della matrice. Il caso guarda le posizioni nel drenaggio, non i tempi.
#[test]
fn la_fine_non_sorpassa_l_esito() {
    let (coda, fascio) = apri();
    let byte = filo(vec![progresso(1), esito()]);
    let (filo_lettore, _freno) =
        avvia_lettore(canale(byte), fascio.lettore, Ruolo::Worker).expect("il filo nasce");
    assert_eq!(filo_lettore.join().expect("il lettore finisce"), None);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    let dove = |quale: &str| {
        fatti
            .iter()
            .position(|fatto| nome_del_fatto(fatto) == quale)
            .unwrap_or_else(|| panic!("manca il fatto «{quale}»: {fatti:?}"))
    };
    assert!(
        dove("esito") < dove("fine"),
        "la fine ha sorpassato l'esito: {fatti:?}"
    );
    assert!(
        dove("progresso") < dove("fine"),
        "la fine ha sorpassato il progresso: {fatti:?}"
    );
}

/// **Su una pipe vera il lettore si ferma quando qualcuno frena.**
///
/// Un `Cursor` non blocca mai, quindi i casi su vettore non provano l'arresto.
/// Qui la pipe e' vera e l'altro capo resta aperto, cosi' l'EOF non arriva: il
/// filo deve finire comunque, e finisce solo se il canale e' non bloccante.
#[test]
#[cfg(target_os = "linux")]
fn su_una_pipe_vera_il_lettore_si_ferma() {
    let (legge, scrive) = std::io::pipe().expect("la pipe si crea");
    let (accordo, _worker) = crate::protocollo::handshake::tests::giro_nominale();
    let canale = super::CanaleOperativo::dal_supervisore(legge, accordo)
        .expect("il canale si rende non bloccante");

    let (coda, fascio) = apri();
    let (filo_lettore, freno) =
        avvia_lettore(canale, fascio.lettore, Ruolo::Worker).expect("il filo nasce");

    // Nessuno scrive, e l'altro capo resta aperto: senza il freno, il lettore
    // resterebbe dentro `read` per sempre.
    std::thread::sleep(Duration::from_millis(20));
    freno.ferma();
    let resoconto = filo_lettore.join().expect("il lettore finisce");
    assert_eq!(resoconto, None);

    drop(scrive);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    // Fermarsi non e' finire: cio' che si accoda e' un'interruzione, non un EOF.
    assert_eq!(quanti(&fatti, "interrotto"), 1, "{fatti:?}");
    assert_eq!(
        quanti(&fatti, "fine"),
        0,
        "un arresto non e' una fine pulita"
    );
}

// --- l'orologio --------------------------------------------------------------

/// La scadenza arriva, **una sola**.
///
/// Una e non due: questa macchina misura il tempo dell'esecuzione, e quello
/// dell'handshake si chiude prima che il canale operativo esista.
#[test]
fn la_scadenza_arriva_una_sola_volta() {
    let (coda, fascio) = apri();
    let (filo_orologio, _freno) = avvia_orologio(
        Duration::from_millis(1),
        fascio.orologio,
        Duration::from_millis(1),
    )
    .expect("il filo nasce");
    assert_eq!(filo_orologio.join().expect("l'orologio finisce"), None);
    drop(fascio.lettore);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "tempo"), 1);
}

/// **L'orologio fermato non accoda niente, e non aspetta la scadenza.**
///
/// E' cio' che permette di aspettarlo alla chiusura: senza il freno, un
/// orologio con una scadenza lontana terrebbe viva la sua bocchetta fino ad
/// allora, e il canale non si disconnetterebbe.
#[test]
fn un_orologio_fermato_non_accoda_e_non_aspetta() {
    let (coda, fascio) = apri();
    let (filo_orologio, freno) = avvia_orologio(
        Duration::from_secs(3600),
        fascio.orologio,
        Duration::from_millis(2),
    )
    .expect("il filo nasce");
    let inizio = std::time::Instant::now();
    freno.ferma();
    assert_eq!(filo_orologio.join().expect("l'orologio finisce"), None);
    let trascorso = inizio.elapsed();
    drop(fascio.lettore);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "tempo"), 0);
    assert!(
        trascorso < Duration::from_secs(60),
        "l'orologio ha aspettato la scadenza invece del freno: {trascorso:?}"
    );
}

// --- il sorvegliante ---------------------------------------------------------

/// Un osservatore che risponde come gli si dice.
///
/// `Debug` perche' la nascita mancata lo rende indietro, e l'`expect` sulla
/// nascita lo richiede.
#[derive(Debug)]
struct Copione {
    risposte: Vec<std::result::Result<bool, Difetto>>,
    interrogazioni: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Osservatore for Copione {
    fn quiescente(&mut self) -> std::result::Result<bool, Difetto> {
        self.interrogazioni
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.risposte.is_empty() {
            return Ok(false);
        }
        self.risposte.remove(0)
    }
}

/// Il dominio si svuota dopo qualche giro, e la quiescenza si accoda una volta.
#[test]
fn la_quiescenza_si_accoda_quando_il_dominio_si_svuota() {
    let (coda, fascio) = apri();
    let interrogazioni = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (filo_sorvegliante, _freno) = avvia_sorvegliante(
        Copione {
            risposte: vec![Ok(false), Ok(false), Ok(true)],
            interrogazioni: std::sync::Arc::clone(&interrogazioni),
        },
        fascio.sorvegliante,
        Duration::from_millis(1),
    )
    .expect("il filo nasce");
    assert_eq!(filo_sorvegliante.join().expect("finisce"), None);
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "quiescente"), 1);
    assert_eq!(
        interrogazioni.load(std::sync::atomic::Ordering::Relaxed),
        3,
        "si guarda finche' non e' vuoto, e poi si smette"
    );
}

/// **Non aver potuto guardare si accoda una volta sola, e poi si smette.**
///
/// Insistere riempirebbe il budget di ripetizioni della stessa cosa: se il
/// dominio non si legge adesso, non si leggera' nemmeno fra un millisecondo, e
/// due righe uguali non dicono piu' di una.
#[test]
fn un_osservazione_impossibile_si_dice_una_volta_sola() {
    let (coda, fascio) = apri();
    let interrogazioni = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (filo_sorvegliante, _freno) = avvia_sorvegliante(
        Copione {
            risposte: vec![Err(Difetto::Impossibile(
                "cgroup.events illeggibile".to_owned(),
            ))],
            interrogazioni: std::sync::Arc::clone(&interrogazioni),
        },
        fascio.sorvegliante,
        Duration::from_millis(1),
    )
    .expect("il filo nasce");
    assert_eq!(filo_sorvegliante.join().expect("finisce"), None);
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "impossibile"), 1);
    assert_eq!(
        quanti(&fatti, "quiescente"),
        0,
        "non aver visto non e' vuoto"
    );
    assert_eq!(interrogazioni.load(std::sync::atomic::Ordering::Relaxed), 1);

    let Some(Fatto::OsservazioneImpossibile { chi, motivo }) = primo(&fatti, "impossibile") else {
        panic!("manca il fatto");
    };
    assert_eq!(*chi, "quiescenza");
    assert!(motivo.contains("illeggibile"), "{motivo}");
}

/// **Un'osservazione interrotta si rifa'**, e non sporca l'evidenza.
///
/// Un'interruzione non e' una mancanza: la lettura non e' avvenuta, e rifarla la
/// fa avvenire. Trattarla come un'osservazione mancata rinuncerebbe alla
/// quiescenza per un segnale — e il segnale, a differenza di un cgroup
/// illeggibile, non dice niente sul dominio.
#[test]
fn un_osservazione_interrotta_si_rifa() {
    let (coda, fascio) = apri();
    let interrogazioni = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (filo_sorvegliante, _freno) = avvia_sorvegliante(
        Copione {
            risposte: vec![
                Err(Difetto::Interrotta),
                Err(Difetto::Interrotta),
                Ok(false),
                Ok(true),
            ],
            interrogazioni: std::sync::Arc::clone(&interrogazioni),
        },
        fascio.sorvegliante,
        Duration::from_millis(1),
    )
    .expect("il filo nasce");
    assert_eq!(filo_sorvegliante.join().expect("finisce"), None);
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(
        quanti(&fatti, "quiescente"),
        1,
        "l'interruzione non impedisce di arrivarci"
    );
    assert_eq!(
        quanti(&fatti, "impossibile"),
        0,
        "un'interruzione non e' un'osservazione mancata"
    );
    assert_eq!(interrogazioni.load(std::sync::atomic::Ordering::Relaxed), 4);
}

/// Un sorvegliante fermato smette senza accodare.
#[test]
fn un_sorvegliante_fermato_smette_senza_accodare() {
    let (coda, fascio) = apri();
    let interrogazioni = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (filo_sorvegliante, freno) = avvia_sorvegliante(
        Copione {
            // Non diventa mai vuoto: senza il freno, questo filo non finirebbe.
            risposte: vec![Ok(false)],
            interrogazioni: std::sync::Arc::clone(&interrogazioni),
        },
        fascio.sorvegliante,
        Duration::from_millis(1),
    )
    .expect("il filo nasce");
    std::thread::sleep(Duration::from_millis(10));
    freno.ferma();
    assert_eq!(filo_sorvegliante.join().expect("finisce"), None);
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "quiescente"), 0);
}

// --- l'annullatore -----------------------------------------------------------

/// **La cancellazione si accoda una volta sola**, anche se la si chiede dieci.
///
/// Il fatto e' gia' li'; ripeterlo spenderebbe gettoni per dire una cosa che il
/// registro ha gia'.
#[test]
fn la_cancellazione_si_accoda_una_volta_sola() {
    let (coda, fascio) = apri();
    let annullatore = Annullatore::nuovo(fascio.annullatore);
    // La prima entra; le altre nove trovano la bocchetta gia' presa, e lo
    // **dicono** invece di confondersi con un successo.
    assert_eq!(annullatore.annulla(), EsitoDellAnnullamento::Accodata);
    for _ in 0..9 {
        assert_eq!(annullatore.annulla(), EsitoDellAnnullamento::GiaDeposta);
    }
    drop(annullatore);
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "cancellazione"), 1);
}

/// Deposta senza annullare, la bocchetta cade e il canale si disconnette.
#[test]
fn deporre_l_annullatore_non_accoda_niente() {
    let (coda, fascio) = apri();
    let annullatore = Annullatore::nuovo(fascio.annullatore);
    assert_eq!(annullatore.deponi(), None);
    drop(annullatore);
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(quanti(&fatti, "cancellazione"), 0);
    assert!(fatti.is_empty());
}

/// **Un lucchetto avvelenato non fa sparire la cancellazione.**
///
/// Con un `ok()?` l'avvelenamento renderebbe lo stesso `None` di «bocchetta
/// gia' deposta»: la cancellazione sparirebbe in silenzio e il worker
/// continuerebbe. Recuperare e' corretto perche' il dato e' un
/// `Option<Bocchetta>`, che non si corrompe a meta'. Il panico su stderr e'
/// rumore voluto: il lucchetto si avvelena davvero.
#[test]
fn un_lucchetto_avvelenato_non_fa_sparire_la_cancellazione() {
    let (coda, fascio) = apri();
    let annullatore = std::sync::Arc::new(Annullatore::nuovo(fascio.annullatore));

    // Si avvelena per davvero: un filo prende il lucchetto e muore tenendolo.
    let mandante = std::sync::Arc::clone(&annullatore);
    let morto = std::thread::spawn(move || {
        let _presa = mandante
            .bocchetta
            .lock()
            .expect("il lucchetto e' ancora sano");
        panic!("un filo muore tenendo il lucchetto");
    })
    .join();
    assert!(morto.is_err(), "il filo doveva morire");
    assert!(
        annullatore.bocchetta.is_poisoned(),
        "senza avvelenamento il caso non misura niente"
    );

    // E la cancellazione entra lo stesso.
    assert_eq!(annullatore.annulla(), EsitoDellAnnullamento::Accodata);

    drop(annullatore);
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    let fatti = raccogli(coda);
    assert_eq!(
        quanti(&fatti, "cancellazione"),
        1,
        "il fatto e' in coda, non solo l'esito che dice di si'"
    );
}

/// E anche `deponi` recupera: la bocchetta cade, e il canale si disconnette.
///
/// Con un `ok()?` la bocchetta resterebbe viva, il drenaggio aspetterebbe fino
/// al tetto, e il difetto riportato non nominerebbe la vera ragione.
#[test]
fn un_lucchetto_avvelenato_non_trattiene_la_bocchetta() {
    let (coda, fascio) = apri();
    let annullatore = std::sync::Arc::new(Annullatore::nuovo(fascio.annullatore));

    let mandante = std::sync::Arc::clone(&annullatore);
    let _ = std::thread::spawn(move || {
        let _presa = mandante
            .bocchetta
            .lock()
            .expect("il lucchetto e' ancora sano");
        panic!("un filo muore tenendo il lucchetto");
    })
    .join();
    assert!(annullatore.bocchetta.is_poisoned());

    assert_eq!(annullatore.deponi(), None);
    drop(annullatore);
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    drop(fascio.raccoglitore);

    // Se la bocchetta fosse rimasta viva, questa raccolta non finirebbe se non
    // al tetto del drenaggio.
    let fatti = raccogli(coda);
    assert!(fatti.is_empty());
}
