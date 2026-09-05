//! I casi della conduzione: la sequenza, e cio' che non si perde.

use std::time::Duration;

use plenora_core::error::{DiagnosticaSupplementare, EvidenzaDiLimite, PressioneDegliAntenati};

use crate::protocollo::codifica::codifica;
use crate::protocollo::messaggi::{
    ConteggiDichiarati, Corpo, DigestArtefatto, EsitoWorkerSulFilo, Frame, Progresso,
};

use super::super::produttori::{CanaleOperativo, Difetto, Osservatore};
use super::super::{EsitoClassificato, Impedimento};
use super::{conduci, Dintorni, LettoreDiEvidenza, Terminatore};
use crate::isolamento::figlio::{FiglioVivo, ProcessoFiglio, Uscita};

// --- i dintorni finti --------------------------------------------------------

/// Un osservatore che risponde secondo un copione, e poi resta sulla risposta
/// finale.
pub(super) struct Guarda {
    risposte: std::sync::Mutex<Vec<std::result::Result<bool, Difetto>>>,
    ultima: std::result::Result<bool, Difetto>,
}

impl Guarda {
    pub(super) fn che_dice(
        risposte: Vec<std::result::Result<bool, Difetto>>,
        ultima: bool,
    ) -> Self {
        Self {
            risposte: std::sync::Mutex::new(risposte),
            ultima: Ok(ultima),
        }
    }
}

impl Osservatore for Guarda {
    fn quiescente(&mut self) -> std::result::Result<bool, Difetto> {
        let mut coda = self.risposte.lock().expect("nessun altro la tiene");
        if coda.is_empty() {
            return self.ultima.clone();
        }
        coda.remove(0)
    }
}

/// Un dominio finto in cui la quiescenza **segue** la forzatura.
///
/// # Perche' i due lati sono legati
///
/// Perche' nel dominio vero lo sono: `cgroup.kill` e' cio' che svuota il cgroup,
/// e la quiescenza arriva un momento dopo. Un osservatore che dicesse «vuoto»
/// indipendentemente dalla forzatura farebbe finire il giro **prima** del
/// margine, il terminatore non verrebbe chiamato affatto, e i casi della
/// cancellazione e del timeout resterebbero verdi misurando un cammino diverso
/// da quello che dichiarano.
///
/// Un osservatore che dicesse «pieno» per sempre farebbe l'errore opposto:
/// nessuna esecuzione arriverebbe a un esito, e ogni riga diventerebbe un
/// impedimento.
#[derive(Debug, Default)]
pub(super) struct Dominio {
    /// Se il dominio si e' svuotato.
    vuoto: std::sync::atomic::AtomicBool,
    /// Quante volte lo si e' forzato.
    forzature: std::sync::atomic::AtomicUsize,
    /// Cio' che la forzatura risponde, quando non riesce.
    difetto: Option<String>,
    /// Quante interrogazioni il dominio regge **dopo** la forzatura prima di
    /// risultare vuoto.
    ///
    /// Zero e' il caso pronto; un numero e' il cgroup che ci mette un momento a
    /// svuotarsi, che e' cio' che fa il vero.
    resistenze: std::sync::atomic::AtomicUsize,
}

impl Dominio {
    /// Un dominio abitato, che si svuota quando lo si forza.
    pub(super) fn abitato() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    /// Un dominio che si svuota **qualche interrogazione dopo** la forzatura.
    ///
    /// E' il caso vero: `cgroup.kill` torna, e i processi muoiono dopo. Chi
    /// forzasse senza guardare lo troverebbe ancora abitato e lo direbbe.
    pub(super) fn che_si_svuota_dopo(interrogazioni: usize) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            vuoto: std::sync::atomic::AtomicBool::new(false),
            forzature: std::sync::atomic::AtomicUsize::new(0),
            difetto: None,
            resistenze: std::sync::atomic::AtomicUsize::new(interrogazioni),
        })
    }

    /// Un dominio gia' vuoto: il worker ha chiuso da se'.
    pub(super) fn gia_vuoto() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            vuoto: std::sync::atomic::AtomicBool::new(true),
            forzature: std::sync::atomic::AtomicUsize::new(0),
            difetto: None,
            resistenze: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Un dominio che **non** si lascia forzare, e lo dice.
    ///
    /// Non si svuota nemmeno: e' esattamente cio' che il difetto significa.
    pub(super) fn che_non_si_forza(motivo: &str) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            vuoto: std::sync::atomic::AtomicBool::new(false),
            forzature: std::sync::atomic::AtomicUsize::new(0),
            difetto: Some(motivo.to_owned()),
            resistenze: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Cio' che `cgroup.kill` fa: si conta, e il dominio si svuota.
    pub(super) fn forza(&self) -> std::result::Result<(), String> {
        self.forzature
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(motivo) = &self.difetto {
            return Err(motivo.clone());
        }
        self.vuoto.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Quante volte e' stato forzato.
    pub(super) fn quante_forzature(&self) -> usize {
        self.forzature.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// L'osservatore di quel dominio.
#[derive(Debug)]
pub(super) struct GuardaIlDominio(pub(super) std::sync::Arc<Dominio>);

impl Osservatore for GuardaIlDominio {
    fn quiescente(&mut self) -> std::result::Result<bool, Difetto> {
        if !self.0.vuoto.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(false);
        }
        // Forzato, ma non ancora vuoto: il cgroup si svuota in un momento, non
        // nell'istante della scrittura.
        let rimaste = self.0.resistenze.load(std::sync::atomic::Ordering::Relaxed);
        if rimaste > 0 {
            self.0
                .resistenze
                .store(rimaste - 1, std::sync::atomic::Ordering::Relaxed);
            return Ok(false);
        }
        Ok(true)
    }
}

/// Il terminatore di quel dominio.
#[derive(Debug)]
pub(super) struct ForzaIlDominio(pub(super) std::sync::Arc<Dominio>);

impl Terminatore for ForzaIlDominio {
    fn termina(&mut self) -> std::result::Result<(), String> {
        self.0.forza()
    }
}

/// Un terminatore che ricorda se e' stato chiamato, e quante volte.
#[derive(Debug, Default, Clone)]
pub(super) struct Termina {
    pub(super) chiamate: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(super) difetto: Option<String>,
}

impl Terminatore for Termina {
    fn termina(&mut self) -> std::result::Result<(), String> {
        self.chiamate
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.difetto.clone().map_or(Ok(()), Err)
    }
}

/// Un lettore d'evidenza che risponde come gli si dice.
pub(super) struct LeggiEvidenza {
    risposta: std::result::Result<EvidenzaDiLimite, Difetto>,
}

impl LeggiEvidenza {
    /// L'evidenza di un dominio in cui non e' successo niente.
    ///
    /// I contatori sono **zero**, non assenti: sono due cose diverse, e la
    /// classificazione le distingue. Zero significa «l'ho letto, e non e'
    /// successo niente»; assente significa «non l'ho letto», che e'
    /// un'evidenza indeterminata e quindi inutilizzabile. Con tutti `None` ogni
    /// caso finisce in `EvidenzaNonUtilizzabile`, e li' a sbagliare e' la
    /// fixture, non la classificazione.
    pub(super) fn senza_pressione() -> Self {
        Self {
            risposta: Ok(EvidenzaDiLimite {
                oom_locali: Some(0),
                uccisi_nel_dominio: Some(0),
                uccisi_nella_gerarchia: Some(0),
                group_kill_locale: Some(0),
                oom_degli_antenati: PressioneDegliAntenati::alla_radice(),
                diagnostica: DiagnosticaSupplementare {
                    tetto_byte: 64 * 1024 * 1024,
                    picco_byte: None,
                    respinte_al_tetto: None,
                },
            }),
        }
    }

    /// L'evidenza che si vuole, per la matrice.
    pub(super) const fn che_rende(
        risposta: std::result::Result<EvidenzaDiLimite, Difetto>,
    ) -> Self {
        Self { risposta }
    }

    fn impossibile() -> Self {
        Self {
            risposta: Err(Difetto::Impossibile("memory.events illeggibile".to_owned())),
        }
    }
}

impl LettoreDiEvidenza for LeggiEvidenza {
    fn evidenza(&mut self) -> std::result::Result<EvidenzaDiLimite, Difetto> {
        match &self.risposta {
            Ok(letta) => Ok(EvidenzaDiLimite {
                oom_locali: letta.oom_locali,
                uccisi_nel_dominio: letta.uccisi_nel_dominio,
                uccisi_nella_gerarchia: letta.uccisi_nella_gerarchia,
                group_kill_locale: letta.group_kill_locale,
                oom_degli_antenati: letta.oom_degli_antenati,
                diagnostica: letta.diagnostica,
            }),
            Err(difetto) => Err(difetto.clone()),
        }
    }
}

/// Un figlio finto, gia' uscito.
pub(super) struct GiaUscito(pub(super) Uscita);

impl ProcessoFiglio for GiaUscito {
    fn pid(&self) -> u32 {
        4242
    }

    fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
        Ok(Some(self.0))
    }

    fn termina(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// --- il canale ---------------------------------------------------------------

pub(super) fn filo(corpi: Vec<Corpo>) -> Vec<u8> {
    let mut byte = Vec::new();
    for corpo in corpi {
        byte.extend(codifica(&Frame::nuovo(corpo)).expect("il frame si codifica"));
    }
    byte
}

pub(super) fn canale(byte: Vec<u8>) -> CanaleOperativo<std::io::Cursor<Vec<u8>>> {
    let (accordo, _worker) = crate::protocollo::handshake::tests::giro_nominale();
    CanaleOperativo::dopo_l_accordo(std::io::Cursor::new(byte), accordo)
}

pub(super) fn esito_di_successo() -> Corpo {
    Corpo::Esito(Box::new(EsitoWorkerSulFilo::Successo {
        digest_artefatto: DigestArtefatto {
            algoritmo: "sha256".to_owned(),
            valore: "0".repeat(64),
        },
        conteggi: ConteggiDichiarati { righe: 3, batch: 1 },
    }))
}

pub(super) fn nome(esito: &EsitoClassificato) -> &'static str {
    match esito {
        EsitoClassificato::Pubblicato { .. } => "pubblicato",
        EsitoClassificato::LimiteAttribuito(_) => "limite_attribuito",
        EsitoClassificato::Timeout { .. } => "timeout",
        EsitoClassificato::Cancellato { .. } => "cancellato",
        EsitoClassificato::PressioneNonAttribuita(_) => "pressione_non_attribuita",
        EsitoClassificato::EvidenzaNonUtilizzabile { .. } => "evidenza_non_utilizzabile",
        EsitoClassificato::ErroreDelWorker { .. } => "errore_del_worker",
        EsitoClassificato::PanicDelWorker { .. } => "panic_del_worker",
        EsitoClassificato::DaVerificare { .. } => "da_verificare",
        EsitoClassificato::TerminazioneAmbigua { .. } => "terminazione_ambigua",
    }
}

/// Il valore di una riga del rapporto.
///
/// Si legge dal **contorno** e non dall'esito, perche' il contorno c'e' sempre:
/// quando la conclusione rifiuta, l'esito non esiste e il rapporto e' l'unica
/// cosa che dice quali fatti ci sono.
pub(super) fn valore<P: ProcessoFiglio>(contorno: &super::Contorno<P>, chiave: &str) -> String {
    contorno
        .rapporto
        .iter()
        .find(|(nome, _)| *nome == chiave)
        .map(|(_, valore)| valore.clone())
        .unwrap_or_default()
}

/// Il tempo lungo: usato dove il timeout non deve scattare.
///
/// # Perche' cinque secondi e non un'ora
///
/// Perche' «lungo» qui vuol dire «piu' lungo del caso», e il caso dura
/// millisecondi. Un'ora sarebbe lo stesso rispetto a cio' che si prova, ma non
/// rispetto a cio' che succede **quando il codice e' rotto**: se i produttori
/// non venissero fermati, l'attesa dell'orologio durerebbe fino alla scadenza,
/// e una batteria che si appende per un'ora non dice niente a nessuno.
fn tempo_lungo() -> Duration {
    Duration::from_secs(5)
}

// --- il cammino nominale -----------------------------------------------------

/// Il giro completo: progresso, esito, fine, dominio vuoto, figlio raccolto.
///
/// L'esito e' `DaVerificare` e non un successo, perche' il publish non
/// appartiene a questo perimetro.
#[test]
fn il_giro_nominale_arriva_a_da_verificare() {
    let (esito, difetti) = conduci(
        canale(filo(vec![
            Corpo::Progresso(Progresso {
                righe: 3,
                batch: 1,
                nodi_completati: 1,
            }),
            esito_di_successo(),
        ])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            osservatore: Guarda::che_dice(vec![Ok(false)], true),
            terminatore: Termina::default(),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(200),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );

    let esito = esito.expect("nessun impedimento");
    assert_eq!(nome(&esito.classificato), "da_verificare");
    assert!(
        difetti.righe().is_empty(),
        "nessun difetto atteso: {:?}",
        difetti.righe()
    );
}

/// **Il dominio non si termina quando non serve.**
///
/// Se il lavoro finisce da solo, nessuno manda segnali a nessuno. Un
/// terminatore chiamato sul cammino nominale ucciderebbe un worker che aveva
/// gia' finito, e la differenza si vedrebbe solo nei log di chi la subisce.
#[test]
fn sul_cammino_nominale_il_dominio_non_si_termina() {
    let terminatore = Termina::default();
    let chiamate = std::sync::Arc::clone(&terminatore.chiamate);
    let (esito, _difetti) = conduci(
        canale(filo(vec![esito_di_successo()])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            osservatore: Guarda::che_dice(vec![], true),
            terminatore,
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(200),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );
    assert!(esito.is_ok());
    assert_eq!(chiamate.load(std::sync::atomic::Ordering::Relaxed), 0);
}

// --- il timeout --------------------------------------------------------------

/// Un tempo che scade porta a `Timeout`, e **il dominio si termina una volta
/// sola**.
///
/// Chiederlo a ogni giro manderebbe segnali a un dominio che sta gia' morendo,
/// senza accelerarlo: il conteggio lo fissa.
///
/// Il dominio e' **abitato** e si svuota solo dopo `cgroup.kill`: e' cio' che
/// rende il caso una prova della forzatura invece che del giro che finisce da
/// se'.
#[test]
fn un_tempo_scaduto_termina_il_dominio_una_volta_sola() {
    let dominio = Dominio::abitato();
    // Il canale non finisce mai da solo: senza il timeout, il giro resterebbe
    // ad ascoltare.
    let (esito, _difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(200),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(200),
        },
        |_| (),
    );

    let esito = esito.expect("nessun impedimento");
    assert_eq!(nome(&esito.classificato), "timeout");
    assert_eq!(
        dominio.quante_forzature(),
        1,
        "il dominio si termina una volta, non a ogni giro"
    );
}

/// **Un dominio che non si lascia forzare non produce un esito.**
///
/// La forzatura che fallisce non e' un difetto di contorno: e' la ragione per
/// cui il dominio resta abitato, e su un dominio abitato i contatori non sono
/// un'osservazione. Cio' che si puo' dire non e' «timeout» — sarebbe
/// un'attribuzione fatta su una fotografia in movimento — ma **che la barriera
/// non e' completa**, e perche'.
///
/// Il difetto resta accanto all'impedimento: i due si leggono insieme, e da soli
/// direbbero meta' della storia.
#[test]
fn un_dominio_che_non_si_termina_non_produce_un_esito() {
    let dominio = Dominio::che_non_si_forza("cgroup.kill non si scrive");
    let (esito, difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(200),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );

    let Err(Impedimento::BarrieraIncompleta(manca)) = esito else {
        panic!("senza quiescenza non c'e' un esito da dare");
    };
    assert!(
        manca.iter().any(|riga| riga.contains("non e' quiescente")),
        "{manca:?}"
    );
    assert!(
        difetti
            .righe()
            .iter()
            .any(|riga| riga.contains("cgroup.kill")),
        "{:?}",
        difetti.righe()
    );
    // E si e' provato: rinunciare in silenzio darebbe lo stesso impedimento
    // senza aver chiesto niente a nessuno.
    assert_eq!(dominio.quante_forzature(), 1);
}

// --- cio' che il drenaggio non lascia indietro --------------------------------

/// **Nessun fatto arrivato prima della chiusura resta fuori.**
///
/// Il caso costruisce la corsa peggiore: un canale che porta un esito **lento**,
/// che arriva mentre il consumatore ha gia' smesso di aspettare perche' il
/// tempo e' scaduto. Il fatto entra in coda dopo che il consumatore ha guardato
/// per l'ultima volta e prima che il lettore venga fermato — cioe' proprio nella
/// finestra in cui un drenaggio che si fermasse su un istante vuoto lo
/// perderebbe.
///
/// L'esito deve comparire nel registro finale: si vede perche' la
/// classificazione dice `timeout` **con** un esito dichiarato alle spalle, e il
/// rapporto lo nomina.
#[test]
fn un_esito_tardivo_non_si_perde() {
    /// Una sorgente che consegna i byte solo dopo un ritardo.
    struct Lenta {
        byte: Vec<u8>,
        consegnati: bool,
        ritardo: Duration,
    }
    impl std::io::Read for Lenta {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.consegnati {
                std::thread::sleep(self.ritardo);
                self.consegnati = true;
                let quanti = self.byte.len().min(buffer.len());
                buffer[..quanti].copy_from_slice(&self.byte[..quanti]);
                self.byte.drain(..quanti);
                return Ok(quanti);
            }
            if self.byte.is_empty() {
                return Ok(0);
            }
            let quanti = self.byte.len().min(buffer.len());
            buffer[..quanti].copy_from_slice(&self.byte[..quanti]);
            self.byte.drain(..quanti);
            Ok(quanti)
        }
    }

    let (accordo, _worker) = crate::protocollo::handshake::tests::giro_nominale();
    let lenta = Lenta {
        byte: filo(vec![esito_di_successo()]),
        consegnati: false,
        // Piu' lungo del tempo dato: il consumatore smette di aspettare prima.
        ritardo: Duration::from_millis(60),
    };

    let (esito, difetti) = conduci(
        CanaleOperativo::dopo_l_accordo(lenta, accordo),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            osservatore: Guarda::che_dice(vec![], true),
            terminatore: Termina::default(),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(200),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );

    let esito = esito.expect("nessun impedimento");
    // Il tempo scaduto vince sulla precedenza, ed e' giusto: e' un fatto
    // nostro, misurato.
    assert_eq!(nome(&esito.classificato), "timeout");
    assert_eq!(
        difetti.drenaggio, None,
        "il drenaggio deve arrivare alla disconnessione"
    );
}

/// **Il drenaggio non lascia indietro niente**: tutti i fatti sono nel registro
/// finale.
///
/// # Perche' questo caso prova il travaso, e per costruzione
///
/// Perche' **uscita ed evidenza sono accodate dopo che il giro e' finito**: la
/// conduzione le osserva al passo 4, quando non ascolta piu'. Non possono
/// quindi essere entrate dal giro principale, e l'unica via che resta e' il
/// drenaggio. Se cio' che il drenaggio rende non venisse travasato nel
/// registro, quelle due righe direbbero «non osservata» e «0».
///
/// Non dipende da nessuna corsa: un caso costruito su un fatto che arriva
/// **tardi** dipenderebbe invece dal colpire una finestra di frazioni di
/// millisecondo, e su una macchina diversa passerebbe o fallirebbe per ragioni
/// che non riguardano il codice. Un caso cosi' e' peggio di nessun caso, e ce
/// n'e' stato uno, e non c'e' piu'.
#[test]
fn tutti_i_fatti_arrivano_al_registro_finale() {
    let (esito, difetti) = conduci(
        canale(filo(vec![esito_di_successo()])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            osservatore: Guarda::che_dice(vec![Ok(false), Ok(false)], true),
            terminatore: Termina::default(),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(200),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );

    let esito = esito.expect("nessun impedimento");
    // `DaVerificare` richiede l'esito dichiarato, e la conclusione avviene solo
    // dopo i tre fatti terminali: se l'esito e' questo, ci sono tutti.
    assert_eq!(nome(&esito.classificato), "da_verificare");
    assert!(difetti.righe().is_empty(), "{:?}", difetti.righe());

    // E si guarda **quali**. Uscita ed evidenza sono gli unici due fatti che la
    // conduzione accoda **dopo** aver smesso di ascoltare: passano quindi per
    // forza dal drenaggio, e se cio' che il drenaggio rende non entrasse nel
    // registro, queste due righe direbbero «non osservata» e «0».
    assert_ne!(
        valore(&difetti, "uscite"),
        "nessuna",
        "l'uscita e' accodata dopo il giro: se e' qui, il drenaggio l'ha travasata"
    );
    assert_eq!(
        valore(&difetti, "evidenze_lette"),
        "1",
        "e con lei l'evidenza"
    );
    assert_eq!(valore(&difetti, "fine_pulita"), "true");
    assert_eq!(valore(&difetti, "quiescente"), "true");
    assert_eq!(valore(&difetti, "esiti_dichiarati"), "successo");
}

/// **Un'evidenza illeggibile impedisce di proseguire.**
///
/// `DaVerificare` significa «vai avanti verso la verifica e il publish», e
/// darlo con una lettura del dominio che non c'e' stata autorizzerebbe a
/// pubblicare senza sapere che cosa e' successo nel dominio.
///
/// E' la forma di difetto piu' insidiosa: un caso che *registra* l'osservazione
/// mancata e insieme permette di ignorarla sembra una prova, e non lo e'.
#[test]
fn un_evidenza_illeggibile_impedisce_di_proseguire() {
    let (esito, _difetti) = conduci(
        canale(filo(vec![esito_di_successo()])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            osservatore: Guarda::che_dice(vec![], true),
            terminatore: Termina::default(),
            evidenza: LeggiEvidenza::impossibile(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(200),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );
    let Err(Impedimento::BarrieraIncompleta(manca)) = esito else {
        panic!("un'evidenza non letta non autorizza a proseguire");
    };
    assert!(
        manca
            .iter()
            .any(|riga| riga.contains("osservazioni mancate")),
        "{manca:?}"
    );
}

/// **Un guasto nella chiusura non nasconde i passi successivi.**
///
/// # Perche' la chiusura non ha `?`
///
/// Perche' ogni passo dopo il giro produce **evidenza**, e un `?` la
/// sopprimerebbe da li' in poi. Un figlio che non si lascia raccogliere e' un
/// difetto; ma se per quel difetto non si leggesse piu' l'evidenza del dominio,
/// il rapporto direbbe «evidenza non letta» — che manda a cercare un problema
/// di lettura dove il problema e' un figlio.
///
/// Qui il figlio non si raccoglie **e** l'evidenza si legge lo stesso: si
/// vedono tutti e due i marcatori, non solo il primo.
///
/// # E la guardia risale
///
/// Il figlio che non si e' lasciato raccogliere esiste ancora, e la conduzione
/// non se ne libera con una riga di rapporto: lo consegna a chi l'ha chiamata,
/// dentro il contorno. Questo caso e' quel chiamante, e infatti deve
/// **rivendicarlo** — se lo lasciasse cadere, la sentinella fermerebbe il
/// processo che esegue i casi. Non e' un inconveniente del caso: e' la prova
/// che la proprieta' e' arrivata fin qui.
#[test]
fn un_guasto_nella_chiusura_non_nasconde_i_passi_successivi() {
    /// Un figlio che non muore e non si lascia raccogliere.
    struct NonSiRaccoglie;
    impl ProcessoFiglio for NonSiRaccoglie {
        fn pid(&self) -> u32 {
            7
        }
        fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
            Ok(None)
        }
        fn termina(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let (esito, mut difetti) = conduci(
        canale(filo(vec![esito_di_successo()])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            osservatore: Guarda::che_dice(vec![], true),
            terminatore: Termina::default(),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(NonSiRaccoglie),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );

    // **La guardia e' qui.** Prima di ogni altra cosa: se la conduzione se ne
    // fosse liberata — con una porta che rinuncia e prosegue — questo campo
    // sarebbe `None`, il caso passerebbe lo stesso il resto delle domande, e
    // nessuno saprebbe che un processo e' rimasto vivo.
    let guardia = difetti
        .figlio_non_raccolto
        .take()
        .expect("il figlio non raccolto risale a chi ha chiamato");
    assert_eq!(
        guardia.pid(),
        Some(7),
        "ed e' quello, non un pid nel rapporto"
    );
    // Rivendicata: qui il processo e' finto, e i casi hanno una via loro.
    assert_eq!(guardia.smonta(), Some(7));

    // Il primo marcatore: il figlio non si e' lasciato chiudere.
    let raccolta = difetti
        .raccolta
        .clone()
        .expect("un figlio che non si raccoglie e' un difetto");
    assert!(raccolta.contains("non si lascia raccogliere"), "{raccolta}");
    assert!(
        raccolta.contains("risale a chi ha chiamato"),
        "il rapporto dice anche dov'e' finito: {raccolta}"
    );

    // I marcatori dei passi **successivi**, che un `?` avrebbe soppresso. Si
    // leggono dal contorno, che esce sempre — anche quando la conclusione
    // rifiuta, ed e' proprio il caso qui.
    assert_eq!(
        valore(&difetti, "evidenze_lette"),
        "1",
        "l'evidenza si legge lo stesso: il guasto e' del figlio, non della lettura"
    );
    assert_eq!(
        difetti.drenaggio, None,
        "e il drenaggio arriva comunque alla disconnessione"
    );
    // E l'uscita **non** risulta osservata, perche' davvero non lo e': il
    // rapporto dice il vero, invece di riempire il buco.
    assert_eq!(valore(&difetti, "uscite"), "nessuna");

    // E non si prosegue: un figlio che non si e' lasciato chiudere e' una
    // ragione per non pubblicare, non un dettaglio accanto a un permesso.
    let Err(Impedimento::BarrieraIncompleta(manca)) = esito else {
        panic!("un figlio non raccolto non autorizza a proseguire");
    };
    assert!(
        manca
            .iter()
            .any(|riga| riga.contains("uscita del worker non e' stata osservata")),
        "{manca:?}"
    );
}

/// **Un produttore che muore male si riporta**, e lo si sa dal `join`.
///
/// E' il caso che tiene l'attesa dei produttori. Un filo che finisce male non
/// accoda niente e non ha un resoconto da rendere: l'unica traccia che lascia e'
/// il valore d'errore del suo `JoinHandle`. Senza aspettarlo, un sorvegliante
/// morto sarebbe indistinguibile da uno che non ha visto niente — e la
/// differenza e' fra «non e' quiescente» e «non lo sappiamo».
#[test]
fn un_produttore_che_muore_male_si_riporta() {
    /// Un osservatore che si rompe invece di rispondere.
    struct SiRompe;
    impl Osservatore for SiRompe {
        fn quiescente(&mut self) -> std::result::Result<bool, Difetto> {
            panic!("l'osservatore si rompe di proposito");
        }
    }

    let (_esito, difetti) = conduci(
        canale(filo(vec![esito_di_successo()])),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            osservatore: SiRompe,
            terminatore: Termina::default(),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );

    assert!(
        difetti
            .righe()
            .iter()
            .any(|riga| riga.contains("sorvegliante") && riga.contains("finito male")),
        "il filo rotto non e' stato riportato: {:?}",
        difetti.righe()
    );
}

/// **Un produttore che resta senza gettoni lo dice**, e il difetto torna dal
/// resoconto.
///
/// # Perche' si prova sulla bocchetta e non sulla conduzione
///
/// Perche' con i budget di oggi **nessun produttore puo' esaurirli**: il lettore
/// ne ha quattro e ne spende al piu' tre, l'orologio due e ne spende due,
/// sorvegliante e raccoglitore lo stesso. E' una proprieta' voluta — i budget
/// sono tarati sul peggio — ma vuol dire che quel cammino, in una conduzione
/// vera, non si raggiunge.
///
/// Provarlo qui e' quindi onesto per quello che e': la via del resoconto
/// funziona, e servira' al primo produttore che avra' piu' da dire dei suoi
/// gettoni. Cio' che tiene l'attesa dei produttori nella conduzione e' il caso
/// del filo che muore male, che invece si raggiunge.
#[test]
fn un_produttore_senza_gettoni_lo_riporta() {
    use super::super::coda::{apri, Produttore};
    let (coda, fascio) = apri();
    let mut raccoglitore = fascio.raccoglitore;
    for _ in 0..Produttore::Raccoglitore.budget() {
        raccoglitore
            .manda(super::super::Fatto::DominioQuiescente)
            .expect("dentro il budget");
    }
    let rifiuto = raccoglitore
        .manda(super::super::Fatto::DominioQuiescente)
        .expect_err("finiti i gettoni");
    assert_eq!(
        raccoglitore.resoconto(),
        Some(rifiuto),
        "cio' che non si e' potuto dire torna dal resoconto, non dalla coda"
    );
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.annullatore);
    drop(fascio.sorvegliante);
    let (_fatti, difetto) = coda.chiudi_e_drena_entro(Duration::from_millis(200));
    assert_eq!(difetto, None);
}

// --- il protocollo rotto -----------------------------------------------------

/// Due esiti arrivano al registro e diventano un **impedimento**, non un esito.
///
/// E' la prova che la conduzione non arbitra: il lettore fa passare il primo e
/// riporta la violazione, il registro ci trova due fatti incompatibili, e la
/// conclusione si rifiuta.
#[test]
fn due_esiti_diventano_un_impedimento_anche_end_to_end() {
    // Il lettore ne fa passare **uno** e poi riporta la violazione, quindi il
    // registro vede un esito e un canale interrotto: non due esiti. Cio' che si
    // prova qui e' che la violazione arrivi fino in fondo e che il canale
    // interrotto **non** valga come fine pulita.
    let (esito, _difetti) = conduci(
        canale(filo(vec![esito_di_successo(), esito_di_successo()])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            osservatore: Guarda::che_dice(vec![], true),
            terminatore: Termina::default(),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(200),
            margine_di_cortesia: Duration::from_millis(50),
            attesa_della_quiescenza: Duration::from_millis(50),
        },
        |_| (),
    );
    // L'esito passato e' uno, ma il canale si e' **interrotto**: la barriera non
    // e' completa, e proseguire vorrebbe dire pubblicare sulla base di una
    // conversazione che non si e' vista finire.
    let Err(Impedimento::BarrieraIncompleta(manca)) = esito else {
        panic!("un canale interrotto non autorizza a proseguire");
    };
    assert!(
        manca.iter().any(|riga| riga.contains("interrotto")),
        "{manca:?}"
    );
    assert!(
        manca
            .iter()
            .any(|riga| riga.contains("non e' finito in modo pulito")),
        "{manca:?}"
    );
}

/// Un impedimento vero: due uscite diverse.
///
/// Non si costruisce dal canale — l'uscita la osserva il conduttore — quindi si
/// prova sul registro. Il caso sta qui per dire che la conduzione **rende** un
/// impedimento quando ce n'e' uno, invece di inghiottirlo.
#[test]
fn la_conduzione_rende_l_impedimento_invece_di_inghiottirlo() {
    let mut registro = super::super::Registro::default();
    registro.applica(super::super::Fatto::UscitaDelWorker(
        super::super::UscitaOsservata::Codice(0),
    ));
    registro.applica(super::super::Fatto::UscitaDelWorker(
        super::super::UscitaOsservata::Codice(3),
    ));
    assert!(matches!(
        registro.concludi(&[]),
        Err(Impedimento::FattiContraddittori(_))
    ));
}

// --- la rinuncia a una nascita parziale ---------------------------------------

/// **Chi rinuncia chiude comunque il dominio, raccoglie e drena.**
///
/// # Che cosa esclude
///
/// L'idea che una nascita parziale sia «un tentativo non cominciato». Quando il
/// secondo produttore non nasce, il worker **esiste gia'**: lo `spawn` e'
/// avvenuto prima, puo' avere discendenti, e il primo produttore puo' avere gia'
/// accodato. Tornare indietro senza chiudere il dominio lascerebbe processi vivi
/// in un cgroup che nessuno guarda piu'; tornare senza drenare renderebbe una
/// pagina bianca su un'esecuzione che qualcosa aveva gia' detto.
///
/// Il caso mette in coda un fatto **prima** di rinunciare, e lo cerca nel
/// rapporto dopo.
///
/// # Che cosa non esclude
///
/// Che `conduci` chiami questa funzione nei tre punti giusti: qui la si chiama
/// direttamente, perche' far fallire `Builder::spawn` a comando richiederebbe
/// una giuntura nel codice di produzione per una condizione che il sistema
/// concede solo quando e' esaurito. I tre punti di chiamata restano una lettura,
/// non una misura.
#[test]
fn rinunciare_chiude_il_dominio_raccoglie_e_non_perde_i_fatti() {
    let (coda, fascio) = super::super::coda::apri();

    // Il lettore e' gia' nato, e ha gia' parlato.
    let mut gia_nato = fascio.lettore;
    assert!(gia_nato
        .manda(super::super::Fatto::MessaggioDalWorker(Box::new(
            esito_di_successo(),
        )))
        .is_ok());
    drop(gia_nato);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);

    let annullatore = super::Annullatore::nuovo(fascio.annullatore);
    let dominio = Dominio::abitato();

    let (esito, difetti) = super::rinuncia(
        "orologio",
        &std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        Vec::new(),
        &annullatore,
        super::PerChiudere {
            raccoglitore: fascio.raccoglitore,
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(7))),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            coda,
            tetto_del_drenaggio: Duration::from_millis(500),
            attesa_della_quiescenza: Duration::from_millis(200),
        },
        Some(GuardaIlDominio(std::sync::Arc::clone(&dominio))),
        super::Contorno::default(),
    );

    let Err(Impedimento::ProduttoreNonNato { chi, motivo }) = esito else {
        panic!("una nascita mancata non e' un esito");
    };
    assert_eq!(chi, "orologio");
    assert!(!motivo.is_empty(), "il motivo del sistema si riporta");

    // 1. Il dominio si e' chiuso — e **si e' svuotato davvero**. Contare le
    //    forzature non basta: `cgroup.kill` e' asincrono, e un caso che si
    //    fermasse al conteggio resterebbe verde su un dominio che non muore.
    assert_eq!(
        dominio.quante_forzature(),
        1,
        "il worker esiste gia': rinunciare senza chiuderlo lascerebbe processi vivi"
    );
    assert_eq!(
        valore(&difetti, "quiescente"),
        "true",
        "chiedere la terminazione non e' osservarla"
    );
    // 2. Il figlio si e' raccolto, e la sua uscita e' nel rapporto.
    assert_eq!(
        valore(&difetti, "uscite"),
        "codice 7",
        "senza, non si saprebbe se era gia' morto o se lo abbiamo ucciso noi"
    );
    assert_eq!(difetti.raccolta, None);
    // 3. Il fatto arrivato prima della rinuncia c'e' ancora.
    assert_ne!(
        valore(&difetti, "esiti_dichiarati"),
        "nessuna",
        "cio' che il worker aveva gia' detto non si butta via: {:?}",
        difetti.rapporto
    );
    // 4. E il drenaggio ha visto la disconnessione.
    assert_eq!(difetti.drenaggio, None);
}

/// **Un dominio che ci mette un momento a svuotarsi viene aspettato.**
///
/// `cgroup.kill` torna, e i processi muoiono dopo. Una rinuncia che guardasse
/// una volta sola troverebbe il dominio ancora abitato e lo direbbe — riportando
/// un residuo che un istante dopo non c'e' piu'. Qui il dominio regge tre
/// interrogazioni: la quiescenza deve comparire lo stesso.
#[test]
fn rinunciare_aspetta_un_dominio_che_si_svuota_dopo_qualche_giro() {
    let (coda, fascio) = super::super::coda::apri();
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    let annullatore = super::Annullatore::nuovo(fascio.annullatore);
    let dominio = Dominio::che_si_svuota_dopo(3);

    let (_esito, difetti) = super::rinuncia(
        "orologio",
        &std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        Vec::new(),
        &annullatore,
        super::PerChiudere {
            raccoglitore: fascio.raccoglitore,
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            coda,
            tetto_del_drenaggio: Duration::from_millis(500),
            attesa_della_quiescenza: Duration::from_millis(500),
        },
        Some(GuardaIlDominio(std::sync::Arc::clone(&dominio))),
        super::Contorno::default(),
    );

    assert_eq!(
        valore(&difetti, "quiescente"),
        "true",
        "il dominio si e' svuotato, e chi guarda una volta sola non lo vede"
    );
    assert!(difetti.righe().is_empty(), "{:?}", difetti.righe());
}

/// **Un dominio che non si svuota resta non quiescente, e si dice perche'.**
///
/// E' l'altra meta': senza di essa, un osservatore che rispondesse sempre
/// «vuoto» renderebbe verde anche il caso precedente. Qui la forzatura
/// fallisce, il dominio resta abitato, e il rapporto deve dire **entrambe** le
/// cose — che non e' quiescente, e che `cgroup.kill` non si e' scritto.
#[test]
fn rinunciare_su_un_dominio_che_non_si_svuota_lo_dichiara() {
    let (coda, fascio) = super::super::coda::apri();
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    let annullatore = super::Annullatore::nuovo(fascio.annullatore);
    let dominio = Dominio::che_non_si_forza("cgroup.kill non si scrive");

    let (esito, difetti) = super::rinuncia(
        "sorvegliante",
        &std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        Vec::new(),
        &annullatore,
        super::PerChiudere {
            raccoglitore: fascio.raccoglitore,
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            coda,
            tetto_del_drenaggio: Duration::from_millis(500),
            attesa_della_quiescenza: Duration::from_millis(30),
        },
        Some(GuardaIlDominio(std::sync::Arc::clone(&dominio))),
        super::Contorno::default(),
    );

    assert!(matches!(esito, Err(Impedimento::ProduttoreNonNato { .. })));
    assert_eq!(valore(&difetti, "quiescente"), "false");
    let righe = difetti.righe();
    assert!(
        righe.iter().any(|riga| riga.contains("cgroup.kill")),
        "manca il motivo per cui non si e' svuotato: {righe:?}"
    );
    assert!(
        righe
            .iter()
            .any(|riga| riga.contains("non si e' svuotato entro")),
        "manca il fatto che non si e' svuotato: {righe:?}"
    );
}

/// **Senza nessuno che possa guardare, la rinuncia lo dichiara.**
///
/// L'osservatore torna in un `Option` perche' la cella condivisa potrebbe, in
/// linea di principio, essere vuota. Tacere li' lascerebbe un dominio non
/// quiescente senza nessuna riga che spieghi perche' nessuno lo ha guardato.
#[test]
fn rinunciare_senza_osservatore_dichiara_di_non_aver_guardato() {
    let (coda, fascio) = super::super::coda::apri();
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    let annullatore = super::Annullatore::nuovo(fascio.annullatore);
    let dominio = Dominio::abitato();

    let (_esito, difetti) = super::rinuncia(
        "lettore",
        &std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        Vec::new(),
        &annullatore,
        super::PerChiudere {
            raccoglitore: fascio.raccoglitore,
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            coda,
            tetto_del_drenaggio: Duration::from_millis(500),
            attesa_della_quiescenza: Duration::from_millis(30),
        },
        None::<GuardaIlDominio>,
        super::Contorno::default(),
    );

    assert_eq!(valore(&difetti, "quiescente"), "false");
    assert!(
        valore(&difetti, "osservazioni_mancate").contains("quiescenza"),
        "{}",
        valore(&difetti, "osservazioni_mancate")
    );
}

// --- i tre cablaggi della rinuncia -------------------------------------------

/// Percorre `conduci` con la `quale`-esima nascita che fallisce, e pretende cio'
/// che ogni rinuncia deve fare.
///
/// Le domande sono **le stesse per tutte e tre**: l'impedimento nomina chi non e'
/// nato; il dominio e' stato chiuso **e si e' svuotato**; il figlio e' stato
/// raccolto e la sua uscita e' nel rapporto; il drenaggio ha visto la
/// disconnessione. Appartengono a ogni cammino, ed e' per questo che si chiedono
/// qui invece che in tre posti che possono divergere.
fn la_nascita_che_fallisce(quale: usize, chi_atteso: &str) {
    let dominio = Dominio::che_si_svuota_dopo(2);
    let _armato = super::super::produttori::inciampo::fai_fallire_la_nascita(quale);

    let (esito, difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        Duration::from_secs(30),
        Dintorni {
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(5))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(20),
            attesa_della_quiescenza: Duration::from_millis(500),
        },
        |_| (),
    );

    let Err(Impedimento::ProduttoreNonNato { chi, motivo }) = esito else {
        panic!("la nascita {quale} doveva fallire, e la conduzione rinunciare");
    };
    assert_eq!(chi, chi_atteso, "la rinuncia nomina chi non e' nato");
    assert!(
        motivo.contains("qualificazione"),
        "il motivo del sistema si riporta: {motivo}"
    );

    assert_eq!(
        dominio.quante_forzature(),
        1,
        "il worker esiste gia': chi rinuncia chiude il dominio"
    );
    assert_eq!(
        valore(&difetti, "quiescente"),
        "true",
        "e ne osserva la quiescenza, invece di fermarsi alla richiesta"
    );
    assert_eq!(
        valore(&difetti, "uscite"),
        "codice 5",
        "il figlio si raccoglie, e la sua uscita entra nel rapporto"
    );
    assert_eq!(difetti.raccolta, None);
    assert_eq!(
        difetti.drenaggio, None,
        "e il canale si disconnette: nessuna bocchetta resta viva"
    );
}

/// **Il lettore che non nasce porta a una rinuncia che lo nomina.**
#[test]
fn se_il_lettore_non_nasce_si_rinuncia() {
    la_nascita_che_fallisce(1, "lettore");
}

/// **L'orologio che non nasce porta alla stessa rinuncia**, con un lettore gia'
/// vivo da fermare.
///
/// E' il cammino che il primo non tocca: qui il rollback ha qualcuno da
/// aspettare, e una bocchetta in piu' da lasciar cadere.
#[test]
fn se_l_orologio_non_nasce_si_rinuncia() {
    la_nascita_che_fallisce(2, "orologio");
}

/// **Il sorvegliante che non nasce porta alla stessa rinuncia**, e l'osservatore
/// torna indietro.
///
/// E' il cammino piu' esposto: l'osservatore viene consegnato alla nascita, e
/// senza il suo ritorno la rinuncia non potrebbe guardare il dominio.
/// Che `quiescente` sia `true` e' la prova che e' tornato.
#[test]
fn se_il_sorvegliante_non_nasce_si_rinuncia_e_l_osservatore_torna() {
    la_nascita_che_fallisce(3, "sorvegliante");
}

// --- le scadenze che non si possono rappresentare -----------------------------

/// **Un margine di cortesia non rappresentabile si dice, e si forza subito.**
///
/// # Perche' non si lascia `None`
///
/// Perche' sarebbe il peggio dei due mondi: senza scadenza non ci sarebbe niente
/// a fermare l'attesa, e la terminazione forzata non arriverebbe mai. Una
/// scadenza che non si puo' calcolare vale quindi **gia' passata** — e lo si
/// dice, perche' un'attesa saltata in silenzio si legge come un'attesa scaduta.
#[test]
fn un_margine_non_rappresentabile_si_osserva() {
    let dominio = Dominio::abitato();
    let (_esito, difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::MAX,
            attesa_della_quiescenza: Duration::from_millis(200),
        },
        |_| (),
    );

    let righe = difetti.righe();
    assert!(
        righe
            .iter()
            .any(|riga| riga.contains("margine di cortesia") && riga.contains("rappresentabili")),
        "{righe:?}"
    );
    assert_eq!(
        dominio.quante_forzature(),
        1,
        "e si forza comunque: senza, l'attesa non finirebbe mai"
    );
}

/// **Un'attesa della quiescenza non rappresentabile si dice, e non si aspetta.**
///
/// E' la stessa classe della precedente, sulla seconda scadenza — quella che
/// nasce **dopo** la forzatura. Un `or_else` muto la farebbe valere «gia'
/// passata» senza una riga: si smetterebbe di aspettare al giro dopo, e chi
/// legge un dominio non quiescente andrebbe a cercare un residuo invece di una
/// somma che non si e' potuta fare.
#[test]
fn un_attesa_della_quiescenza_non_rappresentabile_si_osserva() {
    let dominio = Dominio::che_non_si_forza("cgroup.kill non si scrive");
    let (_esito, difetti) = conduci(
        canale(filo(vec![])),
        std::io::sink(),
        Duration::from_millis(5),
        Dintorni {
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(20),
            attesa_della_quiescenza: Duration::MAX,
        },
        |_| (),
    );

    let righe = difetti.righe();
    assert!(
        righe.iter().any(|riga| {
            riga.contains("attesa della quiescenza") && riga.contains("rappresentabil")
        }),
        "{righe:?}"
    );
}

/// **E la stessa scadenza, dentro la rinuncia, si comporta allo stesso modo.**
///
/// Due punti che calcolano la stessa cosa sono due occasioni di divergere. Qui
/// la rinuncia non aspetta, e lo dichiara: senza la riga, un dominio non
/// quiescente si leggerebbe come «forzato e non morto» invece che «non
/// guardato».
#[test]
fn nella_rinuncia_un_attesa_non_rappresentabile_si_osserva() {
    let (coda, fascio) = super::super::coda::apri();
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    let annullatore = super::Annullatore::nuovo(fascio.annullatore);
    let dominio = Dominio::abitato();

    let (_esito, difetti) = super::rinuncia(
        "lettore",
        &std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        Vec::new(),
        &annullatore,
        super::PerChiudere {
            raccoglitore: fascio.raccoglitore,
            figlio: FiglioVivo::nuovo(GiaUscito(Uscita::Codice(0))),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            coda,
            tetto_del_drenaggio: Duration::from_millis(500),
            attesa_della_quiescenza: Duration::MAX,
        },
        Some(GuardaIlDominio(std::sync::Arc::clone(&dominio))),
        super::Contorno::default(),
    );

    let righe = difetti.righe();
    assert!(
        righe.iter().any(|riga| {
            riga.contains("attesa della quiescenza") && riga.contains("non si guarda")
        }),
        "{righe:?}"
    );
    assert_eq!(
        valore(&difetti, "quiescente"),
        "false",
        "non si e' guardato, e quindi non si e' visto"
    );
}

// --- l'uscita che il sistema non sa riportare ---------------------------------

/// Un figlio che finisce senza che il sistema dica come.
///
/// Non e' un'invenzione: `ExitStatus` puo' non portare ne' codice ne' segnale, e
/// la guardia lo rende come `Uscita::NonRappresentabile` invece di scegliere uno
/// zero.
struct FinitoSenzaDirlo;

impl ProcessoFiglio for FinitoSenzaDirlo {
    fn pid(&self) -> u32 {
        99
    }
    fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
        Ok(Some(Uscita::NonRappresentabile))
    }
    fn termina(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// **Sul cammino ordinario, un'uscita non rappresentabile e' un'osservazione
/// mancata.**
///
/// Non un'uscita a zero — sarebbe un successo inventato — e non il silenzio, che
/// si leggerebbe come «il worker non e' ancora uscito». Il figlio **e' finito**,
/// e cio' che manca e' il modo.
#[test]
fn un_uscita_non_rappresentabile_e_un_osservazione_mancata() {
    let dominio = Dominio::gia_vuoto();
    let (esito, difetti) = conduci(
        canale(filo(vec![esito_di_successo()])),
        std::io::sink(),
        tempo_lungo(),
        Dintorni {
            osservatore: GuardaIlDominio(std::sync::Arc::clone(&dominio)),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            evidenza: LeggiEvidenza::senza_pressione(),
            figlio: FiglioVivo::nuovo(FinitoSenzaDirlo),
            tetto_del_drenaggio: Duration::from_millis(500),
            margine_di_cortesia: Duration::from_millis(20),
            attesa_della_quiescenza: Duration::from_millis(200),
        },
        |_| (),
    );

    assert_eq!(
        valore(&difetti, "uscite"),
        "nessuna",
        "non si inventa uno zero su un'uscita che nessuno ha letto"
    );
    let mancate = valore(&difetti, "osservazioni_mancate");
    assert!(mancate.contains("uscita"), "{mancate}");
    assert!(esito.is_err(), "senza l'uscita osservata non si prosegue");
}

/// **E nel rollback vale lo stesso.**
///
/// # Che cosa esclude
///
/// Che le due conversioni divergano. Nel rollback basta scartare
/// `NonRappresentabile` insieme a `None` perche' «il sistema non sa dirmi come e'
/// finito» valga «non e' ancora finito», e la differenza sparisca senza una
/// riga. Due copie della stessa regola sono due occasioni di divergere, e la
/// seconda e' quella che nessuno rilegge: la conversione e' quindi una sola, e
/// questo caso e' l'altra meta' che la tiene vera su tutti e due i cammini.
#[test]
fn nel_rollback_un_uscita_non_rappresentabile_non_sparisce() {
    let (coda, fascio) = super::super::coda::apri();
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    let annullatore = super::Annullatore::nuovo(fascio.annullatore);
    let dominio = Dominio::abitato();

    let (_esito, difetti) = super::rinuncia(
        "orologio",
        &std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        Vec::new(),
        &annullatore,
        super::PerChiudere {
            raccoglitore: fascio.raccoglitore,
            figlio: FiglioVivo::nuovo(FinitoSenzaDirlo),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            coda,
            tetto_del_drenaggio: Duration::from_millis(500),
            attesa_della_quiescenza: Duration::from_millis(200),
        },
        Some(GuardaIlDominio(std::sync::Arc::clone(&dominio))),
        super::Contorno::default(),
    );

    assert_eq!(valore(&difetti, "uscite"), "nessuna");
    let mancate = valore(&difetti, "osservazioni_mancate");
    assert!(
        mancate.contains("uscita"),
        "l'uscita che il sistema non sa dire non sparisce nemmeno qui: {mancate}"
    );
    assert!(
        mancate.contains("99"),
        "e porta il pid, perche' e' quello il figlio di cui non si sa: {mancate}"
    );
}

/// **Anche dalla rinuncia la guardia risale, e non si scarica.**
///
/// # Perche' serve, visto che il cammino ordinario ha gia' il suo caso
///
/// Perche' sono due punti di codice, e due punti che fanno la stessa cosa sono
/// due occasioni di divergere. Il cammino ordinario ha un caso che rivendica la
/// guardia; senza questo, una mutazione che nella rinuncia sostituisce la
/// risalita con la via dei casi **non fa fallire niente**.
///
/// Qui il figlio non si lascia raccogliere mentre la conduzione sta gia'
/// tornando indietro da una nascita mancata: il difetto e' doppio, e la guardia
/// deve arrivare comunque a chi ha chiamato.
#[test]
fn anche_dalla_rinuncia_la_guardia_risale() {
    /// Un figlio che non muore e non si lascia raccogliere.
    struct NonSiRaccoglie;
    impl ProcessoFiglio for NonSiRaccoglie {
        fn pid(&self) -> u32 {
            11
        }
        fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
            Ok(None)
        }
        fn termina(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let (coda, fascio) = super::super::coda::apri();
    drop(fascio.lettore);
    drop(fascio.orologio);
    drop(fascio.sorvegliante);
    let annullatore = super::Annullatore::nuovo(fascio.annullatore);
    let dominio = Dominio::abitato();

    let (_esito, mut difetti) = super::rinuncia(
        "orologio",
        &std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        Vec::new(),
        &annullatore,
        super::PerChiudere {
            raccoglitore: fascio.raccoglitore,
            figlio: FiglioVivo::nuovo(NonSiRaccoglie),
            terminatore: ForzaIlDominio(std::sync::Arc::clone(&dominio)),
            coda,
            tetto_del_drenaggio: Duration::from_millis(500),
            attesa_della_quiescenza: Duration::from_millis(200),
        },
        Some(GuardaIlDominio(std::sync::Arc::clone(&dominio))),
        super::Contorno::default(),
    );

    let guardia = difetti
        .figlio_non_raccolto
        .take()
        .expect("il figlio non raccolto risale anche da qui");
    assert_eq!(
        guardia.pid(),
        Some(11),
        "ed e' quello, non un pid nel rapporto"
    );
    assert_eq!(guardia.smonta(), Some(11));

    let raccolta = difetti
        .raccolta
        .clone()
        .expect("un figlio che non si raccoglie e' un difetto");
    assert!(
        raccolta.contains("risale a chi ha chiamato"),
        "e il rapporto dice dov'e' finito: {raccolta}"
    );
    // E il resto della rinuncia e' avvenuto lo stesso: il dominio si e' chiuso.
    assert_eq!(dominio.quante_forzature(), 1);
    assert_eq!(valore(&difetti, "quiescente"), "true");
}
