//! La matrice degli esiti, percorsa da un worker **fittizio**.
//!
//! Copre le righe della §10.1 raggiungibili da questo perimetro. Fuori: la
//! **riga 1** (verifica e publish, `PR-10`: qui il successo dichiarato produce
//! `DaVerificare`), le **righe 9 e 10** (handshake, che questa macchina non
//! guida) e le **righe da 11 a 16** (verifica, publish e cleanup, `PR-10`).
//!
//! Una tabella impone a ogni riga le stesse domande: che cosa si e' concluso,
//! che cosa dice il rapporto, che cosa e' rimasto. Il worker e' fittizio perche'
//! il soggetto e' la **macchina**; il worker vero e' il soggetto di `PR-9`.

use std::time::Duration;

use plenora_core::error::{
    DiagnosticaSupplementare, ErrorCategory, EvidenzaDiLimite, PressioneDegliAntenati,
};

use crate::protocollo::messaggi::{
    CategoriaSulFilo, Corpo, EffettoSulFilo, ErroreSulFilo, EsitoWorkerSulFilo, FaseSulFilo,
    FormaPanicSulFilo, RetrySulFilo,
};

use super::super::produttori::Difetto;
use super::super::Ruolo;
use super::tests::{
    canale, esito_di_successo, filo, nome, valore, Dominio, ForzaIlDominio, GiaUscito,
    GuardaIlDominio, LeggiEvidenza,
};
use super::{conduci, Dintorni, Terminatore};
use crate::isolamento::figlio::{FiglioVivo, Uscita};

/// Che cosa il worker fittizio fa, per una riga della matrice.
struct Copione {
    /// Che cosa manda sul canale prima di finire.
    corpi: Vec<Corpo>,
    /// Che cosa il dominio dice di se', dopo.
    evidenza: std::result::Result<EvidenzaDiLimite, Difetto>,
    /// Il tempo dato. Corto solo dove la riga e' il timeout.
    tempo: Duration,
    /// Se qualcuno annulla.
    annulla: bool,
    /// Come il figlio esce: distingue la riga 2 dalla riga 4.
    uscita: Uscita,
    /// Se il dominio e' gia' vuoto quando si comincia a guardare.
    ///
    /// Falso solo per il timeout: su un dominio gia' quiescente non
    /// scatterebbe mai. Li' il dominio si svuota **dopo la forzatura**.
    si_quieta: bool,
}

/// Che cosa la riga pretende.
struct Attesa {
    /// Il nome dell'esito classificato.
    esito: &'static str,
    /// La categoria d'errore, se ce n'e' una.
    categoria: Option<ErrorCategory>,
    /// Come il rapporto deve dire l'uscita.
    ///
    /// **Il testo esatto**: un controllo piu' lasco lascerebbe passare la
    /// differenza fra un worker che finisce e uno che viene ucciso.
    uscita: &'static str,
}

/// L'evidenza di un dominio in cui non e' successo niente.
fn senza_pressione() -> EvidenzaDiLimite {
    con_contatori(Some(0), Some(0), Some(0), Some(0))
}

/// L'evidenza con i quattro contatori scelti.
fn con_contatori(
    oom_locali: Option<u64>,
    uccisi_nel_dominio: Option<u64>,
    uccisi_nella_gerarchia: Option<u64>,
    group_kill_locale: Option<u64>,
) -> EvidenzaDiLimite {
    EvidenzaDiLimite {
        oom_locali,
        uccisi_nel_dominio,
        uccisi_nella_gerarchia,
        group_kill_locale,
        oom_degli_antenati: PressioneDegliAntenati::alla_radice(),
        diagnostica: DiagnosticaSupplementare {
            tetto_byte: 64 * 1024 * 1024,
            picco_byte: None,
            respinte_al_tetto: None,
        },
    }
}

fn tempo_lungo() -> Duration {
    Duration::from_secs(5)
}

fn errore_dichiarato(categoria: CategoriaSulFilo) -> Corpo {
    Corpo::Esito(Box::new(EsitoWorkerSulFilo::Errore {
        errore: Box::new(ErroreSulFilo {
            categoria,
            fase: FaseSulFilo::Write,
            effetto: EffettoSulFilo::None,
            retry: RetrySulFilo::Never {},
            messaggio: "sanificato".to_owned(),
            nodo: None,
            operazione: None,
            execution_id: None,
            diagnostica: None,
        }),
    }))
}

fn panico_dichiarato() -> Corpo {
    Corpo::Esito(Box::new(EsitoWorkerSulFilo::Panic {
        forma: FormaPanicSulFilo::Dinamico,
    }))
}

/// Percorre una riga e pretende cio' che la riga dice.
///
/// Le domande sono **le stesse per tutte**: l'esito, la categoria, l'uscita,
/// la disconnessione del canale e l'assenza di difetti di conduzione.
fn percorri(riga: &str, copione: Copione, attesa: &Attesa) {
    // Chi non si quieta resta **abitato** finche' `cgroup.kill` non lo svuota:
    // cosi' una riga di timeout arriva a un esito invece che a un impedimento.
    let dominio = if copione.si_quieta {
        Dominio::gia_vuoto()
    } else {
        Dominio::abitato()
    };
    let (esito, difetti) = conduci(
        canale(filo(copione.corpi)),
        std::io::sink(),
        copione.tempo,
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::che_rende(copione.evidenza),
            figlio: FiglioVivo::nuovo(GiaUscito(copione.uscita)),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );
    assert!(!copione.annulla, "la cancellazione ha un percorso suo");

    let esito = esito.unwrap_or_else(|impedimento| {
        panic!("riga {riga}: i fatti non si lasciano ridurre: {impedimento}")
    });
    assert_eq!(
        nome(&esito.classificato),
        attesa.esito,
        "riga {riga}: l'esito"
    );
    assert_eq!(
        esito.classificato.categoria(),
        attesa.categoria,
        "riga {riga}: la categoria"
    );

    // Le tre domande che valgono per ogni riga.
    assert_eq!(
        valore(&difetti, "uscite"),
        attesa.uscita,
        "riga {riga}: l'uscita del worker"
    );
    assert_eq!(
        difetti.drenaggio, None,
        "riga {riga}: il canale deve disconnettersi"
    );
    assert!(
        difetti.righe().is_empty(),
        "riga {riga}: nessun difetto di conduzione, e invece {:?}",
        difetti.righe()
    );
}

/// **Riga 2**: errore tipizzato del worker, con la categoria che ha dichiarato.
#[test]
fn riga_2_errore_tipizzato_del_worker() {
    percorri(
        "2",
        Copione {
            corpi: vec![errore_dichiarato(CategoriaSulFilo::Schema)],
            evidenza: Ok(senza_pressione()),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "errore_del_worker",
            categoria: Some(ErrorCategory::Schema),
            uscita: "codice 0",
        },
    );
}

/// **Riga 3**: panico nel worker, con la sola forma del payload.
#[test]
fn riga_3_panic_nel_worker() {
    percorri(
        "3",
        Copione {
            corpi: vec![panico_dichiarato()],
            evidenza: Ok(senza_pressione()),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "panic_del_worker",
            categoria: Some(ErrorCategory::Internal),
            uscita: "codice 0",
        },
    );
}

/// **Riga 4**: il processo muore senza dichiarare niente, e senza evidenza di
/// limite.
///
/// L'assenza dell'esito non e' una variante: e' un'assenza, e produce una
/// terminazione ambigua — **mai** dedotta come OOM.
///
/// L'uscita e' un segnale, ed e' il caso che rende **discriminante** la colonna
/// dell'uscita: ridotta a «raccolto si'/no», questa riga direbbe «codice 0»
/// come le altre.
#[test]
fn riga_4_crash_senza_esito() {
    percorri(
        "4",
        Copione {
            corpi: vec![],
            evidenza: Ok(senza_pressione()),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Segnale(9),
        },
        &Attesa {
            esito: "terminazione_ambigua",
            categoria: Some(ErrorCategory::Internal),
            uscita: "segnale 9",
        },
    );
}

/// **Riga 5**: OOM attribuito. E' l'unica riga con attribuzione, e la vuole
/// **intera**.
#[test]
fn riga_5_oom_attribuito() {
    percorri(
        "5",
        Copione {
            corpi: vec![],
            evidenza: Ok(con_contatori(Some(1), Some(1), Some(1), Some(1))),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "limite_attribuito",
            categoria: Some(ErrorCategory::ResourceLimit),
            uscita: "codice 0",
        },
    );
}

/// **Riga 5-bis**: il tetto locale e' stato invocato e nessuna uccisione e'
/// stata contata.
///
/// Non e' `ResourceLimit`, e non dichiara una causa: e' compatibile con un
/// limite raggiunto e recuperato, e attribuire sarebbe un passo di troppo.
#[test]
fn riga_5bis_pressione_locale_senza_uccisione() {
    percorri(
        "5-bis",
        Copione {
            corpi: vec![],
            evidenza: Ok(con_contatori(Some(1), Some(0), Some(0), Some(0))),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "pressione_non_attribuita",
            categoria: Some(ErrorCategory::UnattributedMemoryPressure),
            uscita: "codice 0",
        },
    );
}

/// **Riga 6a**: il processo muore, e di pressione non c'e' **nessuna** traccia.
#[test]
fn riga_6a_terminazione_senza_evidenza() {
    percorri(
        "6a",
        Copione {
            corpi: vec![],
            evidenza: Ok(senza_pressione()),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "terminazione_ambigua",
            categoria: Some(ErrorCategory::Internal),
            uscita: "codice 0",
        },
    );
}

/// **Riga 6b**: il processo muore, c'e' pressione, ma non autorizza
/// l'attribuzione.
#[test]
fn riga_6b_terminazione_con_pressione_non_attribuibile() {
    percorri(
        "6b",
        Copione {
            corpi: vec![],
            // Uccisioni nella gerarchia senza group kill locale: qualcuno e'
            // morto, ma non per il tetto di **questo** dominio.
            evidenza: Ok(con_contatori(Some(0), Some(0), Some(1), Some(0))),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "pressione_non_attribuita",
            categoria: Some(ErrorCategory::UnattributedMemoryPressure),
            uscita: "codice 0",
        },
    );
}

/// **Riga 7**: il tempo dato finisce.
#[test]
fn riga_7_timeout() {
    percorri(
        "7",
        Copione {
            corpi: vec![],
            evidenza: Ok(senza_pressione()),
            tempo: Duration::from_millis(5),
            annulla: false,
            // Il dominio **non** si svuota: e' cio' che lascia scattare il
            // tempo.
            si_quieta: false,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "timeout",
            categoria: Some(ErrorCategory::Timeout),
            uscita: "codice 0",
        },
    );
}

/// **Riga 8**: la cancellazione chiesta **prima che il giro cominci ad
/// ascoltare** resta agganciata.
///
/// Si annulla dentro `consegna_annullatore`, che la conduzione chiama **prima**
/// del giro: l'ordine e' deterministico, senza `sleep` da tarare.
#[test]
fn riga_8_cancellazione_chiesta_prima_del_giro_resta_agganciata() {
    // Il dominio e' abitato: cosi' a chiudere il giro e' la cancellazione, non
    // un worker che finisce per conto suo.
    let dominio = Dominio::abitato();
    let (esito, difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::che_rende(Ok(senza_pressione())),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        // Annulla **qui**: la conduzione non ha ancora ricevuto un solo fatto.
        |annullatore| {
            annullatore.annulla();
        },
    );

    let esito = esito.expect("nessun impedimento");
    assert_eq!(nome(&esito.classificato), "cancellato");
    assert_eq!(
        esito.classificato.categoria(),
        Some(ErrorCategory::Cancelled)
    );
    assert_eq!(valore(&difetti, "cancellazione"), "true");
    assert!(difetti.righe().is_empty(), "{:?}", difetti.righe());
}

/// E la stessa cancellazione, chiesta **mentre** il giro ascolta, arriva
/// ugualmente.
///
/// Il `sleep` e' innocuo: l'esito atteso non dipende da quando arriva.
#[test]
fn la_cancellazione_arriva_anche_a_giro_iniziato() {
    let dominio = Dominio::abitato();
    let (esito, _difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::che_rende(Ok(senza_pressione())),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |annullatore| {
            let mio = std::sync::Arc::clone(annullatore);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(10));
                mio.annulla();
            });
        },
    );
    assert_eq!(nome(&esito.expect("concluso").classificato), "cancellato");
}

/// Gli eventi osservati, ognuno col momento in cui e' avvenuto.
type Eventi = std::sync::Arc<std::sync::Mutex<Vec<(String, std::time::Instant)>>>;

/// Un margine abbastanza lungo da potersi misurare.
///
/// Sta sopra la risoluzione di `Instant`, cosi' che l'asserzione sul minimo
/// trascorso dipenda dal comportamento e non dalla macchina.
const MARGINE_MISURABILE: Duration = Duration::from_millis(80);

/// **La cancellazione arriva al worker, e prima che il dominio venga forzato.**
///
/// La §8.1: `Annulla` sul filo, poi il margine, poi la terminazione forzata.
/// Si guarda **cosa e' finito sul filo** e **quando** e' stato chiamato il
/// terminatore, su una traccia condivisa che conserva l'ordine.
#[test]
fn la_cancellazione_arriva_sul_filo_prima_della_terminazione() {
    /// Un ingresso che registra **che cosa** ha ricevuto e **quando**.
    struct Traccia {
        eventi: Eventi,
        byte: Vec<u8>,
    }
    impl std::io::Write for Traccia {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.byte.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.eventi.lock().expect("nessun altro la tiene").push((
                format!("annulla:{}", self.byte.len()),
                std::time::Instant::now(),
            ));
            Ok(())
        }
    }

    /// Un terminatore che registra quando e' stato chiamato — e che **svuota
    /// davvero** il dominio, altrimenti il giro non finirebbe.
    struct TerminaTracciato {
        eventi: Eventi,
        dominio: std::sync::Arc<Dominio>,
    }
    impl Terminatore for TerminaTracciato {
        fn termina(&mut self) -> std::result::Result<(), String> {
            self.eventi
                .lock()
                .expect("nessun altro la tiene")
                .push(("termina".to_owned(), std::time::Instant::now()));
            self.dominio.forza()
        }
    }

    let eventi: Eventi = std::sync::Arc::default();
    // Il dominio e' abitato: senza la forzatura il giro non finirebbe, e
    // l'ordine e' osservabile invece che accidentale.
    let dominio = Dominio::abitato();
    let (_esito, _difetti) = conduci(
        canale(filo(vec![])),
        Traccia {
            eventi: std::sync::Arc::clone(&eventi),
            byte: Vec::new(),
        },
        tempo_lungo(),
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: TerminaTracciato {
                eventi: std::sync::Arc::clone(&eventi),
                dominio: std::sync::Arc::clone(&dominio),
            },
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: MARGINE_MISURABILE,
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |annullatore| {
            annullatore.annulla();
        },
    );

    let visti = eventi.lock().expect("nessun altro la tiene").clone();
    let nomi: Vec<&str> = visti.iter().map(|(nome, _)| nome.as_str()).collect();
    assert_eq!(visti.len(), 2, "un Annulla e una terminazione: {nomi:?}");
    assert!(
        nomi[0].starts_with("annulla:"),
        "l'Annulla deve arrivare per primo: {nomi:?}"
    );
    assert_eq!(nomi[1], "termina", "e la forzatura per ultima: {nomi:?}");

    // E i byte scritti sono un frame vero, non un segnaposto.
    let quanti: usize = nomi[0]
        .trim_start_matches("annulla:")
        .parse()
        .expect("il conteggio e' un numero");
    assert!(quanti > 0, "sul filo non e' finito niente");

    // **E fra i due e' passato il margine**: l'ordine da solo non lo dice. La
    // misura e' un **minimo**, quindi una macchina carica non la rompe.
    let atteso = visti[1].1.duration_since(visti[0].1);
    assert!(
        atteso >= MARGINE_MISURABILE,
        "fra l'Annulla e la forzatura sono passati {atteso:?}, meno del margine \
         di {MARGINE_MISURABILE:?}: il margine non e' stato concesso"
    );
}

/// **Un tempo scaduto non chiede: forza e basta.**
///
/// Chiedere gli darebbe un'attesa in piu' che nessuno gli ha concesso.
#[test]
fn un_tempo_scaduto_non_manda_annulla() {
    struct Raccoglie(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Raccoglie {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("nessun altro la tiene")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let scritti = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let dominio = Dominio::abitato();

    let (_esito, _difetti) = conduci(
        canale(filo(vec![])),
        Raccoglie(std::sync::Arc::clone(&scritti)),
        Duration::from_millis(5),
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );

    assert!(
        scritti.lock().expect("nessun altro la tiene").is_empty(),
        "su un tempo scaduto non si chiede niente"
    );
}

/// **La cancellazione termina il dominio.**
///
/// Altrimenti la cancellazione sarebbe un'etichetta sull'esito, con il worker
/// che continua a girare.
#[test]
fn la_cancellazione_termina_il_dominio() {
    // Il worker **non** chiude da se': altrimenti il caso resterebbe verde
    // senza che nessuno abbia terminato niente.
    let dominio = Dominio::abitato();
    let (_esito, _difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::che_rende(Ok(senza_pressione())),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |annullatore| {
            annullatore.annulla();
        },
    );
    assert_eq!(
        dominio.quante_forzature(),
        1,
        "chi annulla si aspetta che il lavoro smetta, non solo che si chiami cosi'"
    );
}

/// **La riga 1 non e' raggiungibile qui, e il caso lo fissa.**
///
/// Un worker che dichiara successo produce `DaVerificare` («prosegui») e non
/// `Pubblicato`: «il worker dice di aver finito» non e' «l'output e' visibile».
#[test]
fn la_riga_1_non_e_raggiungibile_senza_publish() {
    percorri(
        "1 (irraggiungibile)",
        Copione {
            corpi: vec![esito_di_successo()],
            evidenza: Ok(senza_pressione()),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "da_verificare",
            // `DaVerificare` non ha categoria: non e' ancora un esito finale, e
            // dargliela lo farebbe sembrare un fallimento.
            categoria: None,
            uscita: "codice 0",
        },
    );
}

/// Un'evidenza che non si e' potuta leggere non diventa «niente e' successo».
///
/// E' la differenza fra zero e assente, e la classificazione la fa: con i
/// contatori a zero l'esito e' una terminazione ambigua, con i contatori
/// assenti e' un'evidenza inutilizzabile — che e' terminale e viene **prima**
/// dell'esito dichiarato dal worker.
#[test]
fn un_evidenza_non_letta_non_e_un_dominio_tranquillo() {
    percorri(
        "evidenza assente",
        Copione {
            corpi: vec![esito_di_successo()],
            evidenza: Ok(con_contatori(None, None, None, None)),
            tempo: tempo_lungo(),
            annulla: false,
            si_quieta: true,
            uscita: Uscita::Codice(0),
        },
        &Attesa {
            esito: "evidenza_non_utilizzabile",
            categoria: Some(ErrorCategory::Internal),
            uscita: "codice 0",
        },
    );
}

// --- l'OOM tardivo, e l'evidenza che non si legge troppo presto ---------------

/// Un lettore d'evidenza che risponde **secondo lo stato del dominio**.
///
/// Finche' il dominio non e' stato forzato dice zero, dopo dice OOM: e' cio'
/// che il prototipo misura (vedi `ATTESA_DELLA_QUIESCENZA`).
struct EvidenzaCheSegueIlDominio {
    dominio: std::sync::Arc<Dominio>,
    letture: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl super::LettoreDiEvidenza for EvidenzaCheSegueIlDominio {
    fn evidenza(
        &mut self,
    ) -> std::result::Result<EvidenzaDiLimite, super::super::produttori::Difetto> {
        self.letture
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.dominio.quante_forzature() == 0 {
            return Ok(con_contatori(Some(0), Some(0), Some(0), Some(0)));
        }
        Ok(con_contatori(Some(1), Some(1), Some(1), Some(1)))
    }
}

/// **L'OOM che arriva dopo la forzatura si vede.**
///
/// Esclude che la conduzione smetta di ascoltare nell'istante in cui forza:
/// l'evidenza direbbe zero e l'esito sarebbe un timeout. La riga 5 prova
/// l'evidenza immediata; questo caso quella che arriva **tardi**.
#[test]
fn l_oom_tardivo_si_vede_perche_si_continua_ad_ascoltare_dopo_la_forzatura() {
    let dominio = Dominio::abitato();
    let letture = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (esito, difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: EvidenzaCheSegueIlDominio {
                dominio: std::sync::Arc::clone(&dominio),
                letture: std::sync::Arc::clone(&letture),
            },
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(20),
            attesa_della_quiescenza: Duration::from_millis(500),
        },
        |_| (),
    );

    let esito = esito.expect("dopo la quiescenza l'esito c'e'");
    assert_eq!(
        nome(&esito.classificato),
        "limite_attribuito",
        "letta troppo presto, l'evidenza avrebbe detto zero"
    );
    assert_eq!(valore(&difetti, "quiescente"), "true");
    assert_eq!(
        letture.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "l'evidenza si legge una volta, e dopo"
    );
}

/// **Su un dominio ancora abitato l'evidenza non si legge affatto.**
///
/// Non basta rifiutare l'esito: `evidenze_lette: 1` direbbe il falso nel
/// rapporto. Il dominio non si lascia forzare, cosi' resta abitato nonostante
/// il tentativo.
#[test]
fn su_un_dominio_abitato_l_evidenza_non_si_legge() {
    let dominio = Dominio::che_non_si_forza("cgroup.kill non si scrive");
    let letture = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (esito, difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: EvidenzaCheSegueIlDominio {
                dominio: std::sync::Arc::clone(&dominio),
                letture: std::sync::Arc::clone(&letture),
            },
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(20),
            attesa_della_quiescenza: Duration::from_millis(20),
        },
        |_| (),
    );

    assert!(esito.is_err(), "senza quiescenza non c'e' un esito da dare");
    assert_eq!(
        letture.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "sui contatori di un dominio abitato non si guarda nemmeno"
    );
    assert_eq!(valore(&difetti, "evidenze_lette"), "0");
    assert!(
        valore(&difetti, "osservazioni_mancate").contains("evidenza"),
        "la rinuncia si dichiara: {}",
        valore(&difetti, "osservazioni_mancate")
    );
}

/// **La quiescenza accodata dopo l'ultima interrogazione del giro si vede.**
///
/// La finestra: il sorvegliante accoda la quiescenza dopo l'ultima
/// interrogazione del giro, e il registro non lo sa ancora quando decide se
/// leggere l'evidenza (vedi il passo 4 di `conduci`).
///
/// Deterministico: il terminatore non torna finche' l'osservatore, cadendo dopo
/// la `manda`, non annuncia l'accodamento; con l'attesa della quiescenza a zero
/// il giro esce **senza interrogare** la coda un'altra volta.
#[test]
fn la_quiescenza_accodata_all_ultimo_istante_si_vede() {
    /// L'osservatore, che annuncia cadendo di aver finito il suo lavoro.
    struct GuardaEAnnuncia {
        vuoto: std::sync::Arc<std::sync::atomic::AtomicBool>,
        annuncia: std::sync::mpsc::Sender<()>,
    }
    impl super::super::produttori::Osservatore for GuardaEAnnuncia {
        fn quiescente(&mut self) -> std::result::Result<bool, super::super::produttori::Difetto> {
            Ok(self.vuoto.load(std::sync::atomic::Ordering::Acquire))
        }
    }
    impl Drop for GuardaEAnnuncia {
        fn drop(&mut self) {
            // Il filo del sorvegliante ha gia' accodato: la `manda` viene prima
            // del ritorno, e questo cade dopo.
            let _ = self.annuncia.send(());
        }
    }

    /// Il terminatore, che non torna finche' la quiescenza non e' **in coda**.
    struct ForzaEAspettaLAccodamento {
        vuoto: std::sync::Arc<std::sync::atomic::AtomicBool>,
        saputo: std::sync::mpsc::Receiver<()>,
    }
    impl Terminatore for ForzaEAspettaLAccodamento {
        fn termina(&mut self) -> std::result::Result<(), String> {
            self.vuoto.store(true, std::sync::atomic::Ordering::Release);
            self.saputo
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| "il sorvegliante non ha accodato la quiescenza".to_owned())
        }
    }

    let vuoto = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (annuncia, saputo) = std::sync::mpsc::channel();

    let (esito, difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            ruolo: Ruolo::Worker,
            osservatore: GuardaEAnnuncia {
                vuoto: std::sync::Arc::clone(&vuoto),
                annuncia,
            },
            terminatore: ForzaEAspettaLAccodamento {
                vuoto: std::sync::Arc::clone(&vuoto),
                saputo,
            },
            evidenza: LeggiEvidenza::che_rende(Ok(con_contatori(
                Some(1),
                Some(1),
                Some(1),
                Some(1),
            ))),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(20),
            // Zero: dopo la forzatura il giro esce senza interrogare ancora la
            // coda, e la quiescenza resta li' dentro. E' la finestra.
            attesa_della_quiescenza: Duration::ZERO,
        },
        |_| (),
    );

    let esito = esito.expect("la quiescenza c'e', e l'esito con lei");
    assert_eq!(valore(&difetti, "quiescente"), "true");
    assert_eq!(
        valore(&difetti, "evidenze_lette"),
        "1",
        "l'evidenza si legge: saltarla e' il difetto"
    );
    assert_eq!(
        nome(&esito.classificato),
        "limite_attribuito",
        "saltando l'evidenza, la stessa esecuzione si chiamerebbe «timeout»"
    );
    assert_eq!(
        difetti.abitato, None,
        "la quiescenza accodata vale anche per il contorno: il dominio non e' abitato"
    );
}
