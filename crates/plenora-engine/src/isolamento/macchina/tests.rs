//! I casi della macchina: la riduzione non dipende dalla corsa.
//!
//! Girano ovunque e senza processi: cio' che si prova qui e' la **regola** —
//! quali fatti accendono cosa, e che l'ordine non conti — e provarla con
//! processi veri la legherebbe allo scheduler, cioe' proprio alla cosa da cui
//! deve essere indipendente.

use plenora_core::error::{ErrorCategory, ErrorPhase, RemoteEffect, RetryDisposition};

use crate::classificazione::EsitoClassificato;
use crate::protocollo::messaggi::{
    CategoriaSulFilo, ConteggiDichiarati, Corpo, DiagnosticaSulFilo, DigestArtefatto,
    EffettoSulFilo, ErroreSulFilo, EsitoWorkerSulFilo, FaseSulFilo, FormaPanicSulFilo,
    RetrySulFilo,
};

use super::{Fatto, Impedimento, Registro, UscitaOsservata};

/// Un esito di successo sul filo, con i conteggi obbligatori.
fn esito_di_successo() -> Fatto {
    Fatto::MessaggioDalWorker(Box::new(Corpo::Esito(Box::new(
        EsitoWorkerSulFilo::Successo {
            digest_artefatto: DigestArtefatto {
                algoritmo: "sha256".to_owned(),
                valore: "0".repeat(64),
            },
            conteggi: ConteggiDichiarati { righe: 0, batch: 0 },
        },
    ))))
}

/// Un panico dichiarato dal worker.
fn esito_di_panico(forma: FormaPanicSulFilo) -> Fatto {
    Fatto::MessaggioDalWorker(Box::new(Corpo::Esito(Box::new(
        EsitoWorkerSulFilo::Panic { forma },
    ))))
}

/// L'uscita ordinaria.
const fn uscita_pulita() -> Fatto {
    Fatto::UscitaDelWorker(UscitaOsservata::Codice(0))
}

/// Il registro costruito applicando i fatti nell'ordine dato.
fn registro(fatti: Vec<Fatto>) -> Registro {
    let mut registro = Registro::default();
    for fatto in fatti {
        registro.applica(fatto);
    }
    registro
}

/// Il nome della variante d'esito, per confrontare due classificazioni senza
/// pretendere che `EsitoClassificato` sia confrontabile.
fn nome(esito: &super::EsitoDelSupervisore) -> &'static str {
    match &esito.classificato {
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

/// Le permutazioni di un elenco, per provare che l'ordine non conta.
///
/// Scritte a mano invece che prese da una dipendenza: sono sei righe, e una
/// dipendenza nuova per sei righe non si aggiunge.
fn permutazioni<T>(elementi: Vec<T>) -> Vec<Vec<T>>
where
    T: Clone,
{
    if elementi.len() <= 1 {
        return vec![elementi];
    }
    let mut tutte = Vec::new();
    for indice in 0..elementi.len() {
        let mut resto = elementi.clone();
        let primo = resto.remove(indice);
        for mut coda in permutazioni(resto) {
            coda.insert(0, primo.clone());
            tutte.push(coda);
        }
    }
    tutte
}

/// Una ricetta di fatti, clonabile per generarne le permutazioni.
///
/// `Fatto` non e' clonabile — porta un errore e un'evidenza, che non si
/// duplicano a cuor leggero — quindi le permutazioni si fanno sulle **ricette**
/// e i fatti si costruiscono dopo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ricetta {
    EsitoSuccesso,
    EsitoPanico,
    Fine,
    Uscita,
    Quiescenza,
    TempoScadutoInEsecuzione,
    Cancellazione,
}

impl Ricetta {
    fn in_fatto(self) -> Fatto {
        match self {
            Self::EsitoSuccesso => esito_di_successo(),
            Self::EsitoPanico => esito_di_panico(FormaPanicSulFilo::Statico),
            Self::Fine => Fatto::FineDelCanale,
            Self::Uscita => uscita_pulita(),
            Self::Quiescenza => Fatto::DominioQuiescente,
            Self::TempoScadutoInEsecuzione => Fatto::TempoScaduto,
            Self::Cancellazione => Fatto::CancellazioneRichiesta,
        }
    }
}

/// Che tutte le permutazioni di una ricetta diano lo stesso esito.
fn stesso_esito_in_ogni_ordine(ricette: Vec<Ricetta>, atteso: &str) {
    let ordini = permutazioni(ricette);
    assert!(
        ordini.len() > 1,
        "una ricetta con un ordine solo non prova niente"
    );
    for ordine in ordini {
        let fatti = ordine.iter().map(|r| r.in_fatto()).collect();
        let esito = registro(fatti)
            .concludi(&[])
            .expect("questa ricetta non tocca la decisione aperta");
        assert_eq!(
            nome(&esito),
            atteso,
            "l'ordine {ordine:?} ha prodotto un esito diverso"
        );
    }
}

/// **La proprieta' centrale**: l'ordine d'arrivo non decide.
///
/// Quattro fatti positivi, in tutti i ventiquattro ordini possibili. Se la
/// riduzione dipendesse dalla corsa, almeno uno di questi ordini darebbe un
/// esito diverso — ed e' esattamente la corsa che su una macchina vera non si
/// riesce a riprodurre a comando.
#[test]
fn i_quattro_fatti_positivi_danno_lo_stesso_esito_in_ogni_ordine() {
    stesso_esito_in_ogni_ordine(
        vec![
            Ricetta::EsitoSuccesso,
            Ricetta::Fine,
            Ricetta::Uscita,
            Ricetta::Quiescenza,
        ],
        // Non «pubblicato»: il publish non appartiene a questo perimetro, e un
        // worker che dichiara successo dice «prosegui», non «riuscito».
        "da_verificare",
    );
}

/// Il timeout non si lascia scavalcare da un esito che arriva dopo, ne' da uno
/// che arriva prima.
#[test]
fn il_tempo_scaduto_non_dipende_da_quando_arriva() {
    stesso_esito_in_ogni_ordine(
        vec![
            Ricetta::EsitoSuccesso,
            Ricetta::Fine,
            Ricetta::Uscita,
            Ricetta::Quiescenza,
            Ricetta::TempoScadutoInEsecuzione,
        ],
        "timeout",
    );
}

/// Fra timeout e cancellazione decide la precedenza della §10.3, non l'ordine.
///
/// Il timeout viene prima: e' il nostro orologio, misurato, mentre la
/// cancellazione e' una decisione. Che l'una arrivi prima dell'altro non
/// sposta nulla.
#[test]
fn fra_timeout_e_cancellazione_decide_la_precedenza() {
    stesso_esito_in_ogni_ordine(
        vec![
            Ricetta::Cancellazione,
            Ricetta::TempoScadutoInEsecuzione,
            Ricetta::Fine,
            Ricetta::Uscita,
            Ricetta::Quiescenza,
        ],
        "timeout",
    );
}

/// La cancellazione da sola conclude come cancellazione.
#[test]
fn la_cancellazione_da_sola_e_una_cancellazione() {
    stesso_esito_in_ogni_ordine(
        vec![
            Ricetta::Cancellazione,
            Ricetta::Fine,
            Ricetta::Uscita,
            Ricetta::Quiescenza,
        ],
        "cancellato",
    );
}

/// Un panico dichiarato resta un panico, in qualunque ordine arrivi.
#[test]
fn il_panico_dichiarato_non_dipende_dall_ordine() {
    stesso_esito_in_ogni_ordine(
        vec![
            Ricetta::EsitoPanico,
            Ricetta::Fine,
            Ricetta::Uscita,
            Ricetta::Quiescenza,
        ],
        "panic_del_worker",
    );
}

/// Morire senza dichiarare niente e' una terminazione ambigua, non un errore
/// del worker.
///
/// L'assenza dell'esito e' un'informazione, e va distinta da un esito che dice
/// «ho fallito»: nel primo caso non sappiamo perche', nel secondo si'.
#[test]
fn morire_senza_esito_e_una_terminazione_ambigua() {
    stesso_esito_in_ogni_ordine(
        vec![Ricetta::Fine, Ricetta::Uscita, Ricetta::Quiescenza],
        "terminazione_ambigua",
    );
}

// --- i quattro fatti distinti ------------------------------------------------

/// I quattro fatti positivi sono **quattro**, e togliendone uno solo la
/// condizione cade.
///
/// E' la prova che nessuno dei quattro e' decorativo. Un controllo che
/// guardasse solo l'esito, o solo la quiescenza, passerebbe tre di questi
/// quattro sotto-casi.
#[test]
fn togliere_uno_qualunque_dei_quattro_fatti_toglie_il_successo() {
    let completi = vec![
        Ricetta::EsitoSuccesso,
        Ricetta::Fine,
        Ricetta::Uscita,
        Ricetta::Quiescenza,
    ];
    assert!(
        registro(completi.iter().map(|r| r.in_fatto()).collect()).quattro_fatti_positivi(),
        "i quattro fatti insieme sono il successo"
    );

    for mancante in 0..completi.len() {
        let mut ridotti = completi.clone();
        let tolto = ridotti.remove(mancante);
        assert!(
            !registro(ridotti.iter().map(|r| r.in_fatto()).collect()).quattro_fatti_positivi(),
            "senza {tolto:?} i quattro fatti positivi non ci sono"
        );
    }
}

/// Un'uscita per segnale non e' un'uscita pulita, anche con tutto il resto a
/// posto.
#[test]
fn un_uscita_per_segnale_non_e_un_successo() {
    let mut registro = registro(vec![
        esito_di_successo(),
        Fatto::FineDelCanale,
        Fatto::DominioQuiescente,
    ]);
    registro.applica(Fatto::UscitaDelWorker(UscitaOsservata::Segnale(9)));
    assert!(!registro.quattro_fatti_positivi());
    assert!(
        registro.concluso(),
        "si puo' smettere di aspettare anche quando non e' andata bene"
    );
}

/// Un canale troncato non e' un EOF, e non conta come fatto positivo — ma
/// conclude comunque l'attesa.
///
/// Sono due domande diverse: «si puo' smettere di aspettare» e «e' andato tutto
/// bene». Fonderle terrebbe il supervisore fermo davanti a un filo che non
/// portera' piu' niente.
#[test]
fn un_canale_troncato_conclude_l_attesa_ma_non_e_un_fatto_positivo() {
    let registro = registro(vec![
        esito_di_successo(),
        Fatto::CanaleInterrotto("prefisso troncato".to_owned()),
        uscita_pulita(),
        Fatto::DominioQuiescente,
    ]);
    assert!(registro.concluso());
    assert!(!registro.quattro_fatti_positivi());
}

/// Finche' i tre fatti terminali non ci sono tutti, non si conclude.
#[test]
fn si_aspetta_finche_i_tre_fatti_terminali_non_ci_sono() {
    for parziale in [
        vec![Fatto::FineDelCanale, uscita_pulita()],
        vec![Fatto::FineDelCanale, Fatto::DominioQuiescente],
        vec![uscita_pulita(), Fatto::DominioQuiescente],
    ] {
        assert!(
            !registro(parziale).concluso(),
            "due fatti su tre non concludono"
        );
    }
}

// --- il protocollo -----------------------------------------------------------

/// I messaggi che non sono un esito si contano e non decidono.
#[test]
fn i_messaggi_di_progresso_non_decidono() {
    let registro = registro(vec![
        Fatto::MessaggioDalWorker(Box::new(Corpo::Progresso(
            crate::protocollo::messaggi::Progresso {
                righe: 1,
                batch: 1,
                nodi_completati: 1,
            },
        ))),
        Fatto::FineDelCanale,
        uscita_pulita(),
        Fatto::DominioQuiescente,
    ]);
    let esito = registro.concludi(&[]).expect("nessuna decisione aperta");
    assert_eq!(
        nome(&esito),
        "terminazione_ambigua",
        "il progresso non e' una dichiarazione d'esito"
    );
}

// --- la forma del panico -----------------------------------------------------

/// Le tre forme del filo diventano le tre forme del dominio, e restano
/// distinte.
///
/// La distinzione conta: se due forme collassassero, un rapporto direbbe
/// «payload statico» dove il panico e' dinamico, e nessuno se ne accorgerebbe
/// perche' il contenuto non si pubblica.
#[test]
fn le_tre_forme_del_panico_restano_tre() {
    let statico = super::forma_di_dominio(FormaPanicSulFilo::Statico).to_string();
    let dinamico = super::forma_di_dominio(FormaPanicSulFilo::Dinamico).to_string();
    let altro = super::forma_di_dominio(FormaPanicSulFilo::NonTestuale).to_string();
    assert_ne!(statico, dinamico);
    assert_ne!(dinamico, altro);
    assert_ne!(statico, altro);
}

/// E coincidono con quelle che l'autorita' produce su payload veri.
///
/// E' cio' che impedisce alle due nozioni di forma di divergere: se
/// `panic_policy` cambiasse una stringa, questo caso lo direbbe invece di
/// lasciare che il filo e il dominio raccontino due cose diverse.
#[test]
fn le_forme_coincidono_con_quelle_dell_autorita() {
    use crate::classificazione::FormaDelPayload;
    let statico: &'static str = "un payload vero";
    let coppie = [
        (
            FormaPanicSulFilo::Statico,
            FormaDelPayload::di(&statico).to_string(),
        ),
        (
            FormaPanicSulFilo::Dinamico,
            FormaDelPayload::di(&"un payload vero".to_owned()).to_string(),
        ),
        (
            FormaPanicSulFilo::NonTestuale,
            FormaDelPayload::di(&42_u32).to_string(),
        ),
    ];
    for (dal_filo, dall_autorita) in coppie {
        assert_eq!(
            super::forma_di_dominio(dal_filo).to_string(),
            dall_autorita,
            "la forma {dal_filo:?} non coincide con quella dell'autorita'"
        );
    }
}

// --- l'errore del worker, senza perdere assi --------------------------------

/// Un errore sul filo con assi qualunque.
fn errore_sul_filo(
    categoria: CategoriaSulFilo,
    fase: FaseSulFilo,
    effetto: EffettoSulFilo,
    retry: RetrySulFilo,
) -> ErroreSulFilo {
    ErroreSulFilo {
        categoria,
        fase,
        effetto,
        retry,
        messaggio: "sanificato".to_owned(),
        nodo: Some("nodo-7".to_owned()),
        operazione: Some("scrivi".to_owned()),
        execution_id: Some("esecuzione-3".to_owned()),
        diagnostica: None,
    }
}

fn esito_di_errore(errore: ErroreSulFilo) -> Fatto {
    Fatto::MessaggioDalWorker(Box::new(Corpo::Esito(Box::new(
        EsitoWorkerSulFilo::Errore {
            errore: Box::new(errore),
        },
    ))))
}

/// **I quattro assi attraversano tutti**, e l'errore di dominio li rende senza
/// ricalcolarli.
///
/// E' cio' che distingue un portatore da una variante scelta per somiglianza:
/// una variante scelta produrrebbe la categoria che le compete, non quella
/// dichiarata, e la disposizione al ritentativo verrebbe **ricalcolata** da
/// fase ed effetto invece di essere quella che il worker ha mandato.
#[test]
fn i_quattro_assi_dell_errore_attraversano_invariati() {
    use plenora_core::error::{ErrorCategory, ErrorPhase, RemoteEffect, RetryDisposition};

    let errore = super::errore_di_dominio(&errore_sul_filo(
        CategoriaSulFilo::UnattributedMemoryPressure,
        FaseSulFilo::Commit,
        EffettoSulFilo::Partial,
        RetrySulFilo::RequiresRecovery {},
    ));

    assert_eq!(errore.category(), ErrorCategory::UnattributedMemoryPressure);
    assert_eq!(errore.phase(), ErrorPhase::Commit);
    assert_eq!(errore.remote_effect(), RemoteEffect::Partial);
    assert_eq!(
        errore.retry_disposition(),
        RetryDisposition::RequiresRecovery
    );
    assert!(
        errore.to_string().contains("sanificato"),
        "il messaggio sanificato viaggia: {errore}"
    );
}

/// Il ritardo del ritentativo e' un valore, e viaggia come tale.
#[test]
fn il_ritardo_del_ritentativo_attraversa() {
    use plenora_core::error::RetryDisposition;
    let errore = super::errore_di_dominio(&errore_sul_filo(
        CategoriaSulFilo::Transient,
        FaseSulFilo::Read,
        EffettoSulFilo::None,
        RetrySulFilo::After { delay_ms: 1500 },
    ));
    assert_eq!(
        errore.retry_disposition(),
        RetryDisposition::After(std::time::Duration::from_millis(1500))
    );
}

/// Un errore dichiarato si classifica come errore del worker, e conserva la
/// categoria dichiarata.
#[test]
fn un_errore_dichiarato_si_classifica_con_la_sua_categoria() {
    let registro = registro(vec![
        esito_di_errore(errore_sul_filo(
            CategoriaSulFilo::ResourceLimit,
            FaseSulFilo::Write,
            EffettoSulFilo::None,
            RetrySulFilo::Never {},
        )),
        Fatto::FineDelCanale,
        uscita_pulita(),
        Fatto::DominioQuiescente,
    ]);
    let esito = registro.concludi(&[]).expect("nessun impedimento");
    assert_eq!(nome(&esito), "errore_del_worker");
    assert_eq!(
        esito.classificato.categoria(),
        Some(plenora_core::error::ErrorCategory::ResourceLimit),
        "la categoria e' quella dichiarata dal worker, non quella di una variante scelta"
    );
}

/// La diagnostica di riga **non attraversa**, e il rapporto lo dice.
///
/// Non e' un difetto nascosto: e' una decisione aperta — `DiagnosticaSulFilo`
/// non e' isomorfa a `RowDiagnostics` — e finche' resta aperta il conteggio
/// impedisce che l'assenza si legga come «non ce n'e' nessuna».
#[test]
fn la_diagnostica_di_riga_non_attraversa_e_si_dichiara() {
    let mut errore = errore_sul_filo(
        CategoriaSulFilo::Execution,
        FaseSulFilo::Write,
        EffettoSulFilo::None,
        RetrySulFilo::Never {},
    );
    errore.diagnostica = Some(DiagnosticaSulFilo {
        contract: "contratto".to_owned(),
        scope: "colonna".to_owned(),
        completeness: "completa".to_owned(),
        observed_total: 3,
        conteggi: Vec::new(),
        esempi: Vec::new(),
        esempi_troncati: false,
    });
    let registro = registro(vec![
        esito_di_errore(errore),
        Fatto::FineDelCanale,
        uscita_pulita(),
        Fatto::DominioQuiescente,
    ]);
    // Non un conteggio: il contenuto. Il rapporto la nomina, e l'esito la
    // possiede intera.
    assert!(
        valore(&registro, "diagnostica_di_riga").contains("colonna"),
        "il rapporto non nomina la diagnostica: {}",
        valore(&registro, "diagnostica_di_riga")
    );
    let esito = registro.concludi(&[]).expect("nessun impedimento");
    let portata = esito
        .diagnostica_di_riga
        .expect("la diagnostica arriva fino all'esito");
    assert_eq!(portata.observed_total, 3);
    assert_eq!(portata.contract, "contratto");
    assert_eq!(portata.scope, "colonna");
    assert_eq!(portata.completeness, "completa");
    assert!(!portata.esempi_troncati);
}

// --- l'oracolo delle conversioni --------------------------------------------
//
// # Perche' una tabella e non «i risultati sono tutti distinti»
//
// Perche' la distinzione prova l'**iniettivita'**, non la correttezza. Uno
// scambio completo — `Schema` che diventa `Io` e `Io` che diventa `Schema` —
// lascia venti risultati distinti, e un caso che conta le distinzioni lo trova
// perfetto. Cio' che serve e' dire, per ogni variante, **quale** deve uscire.
//
// # Perche' la tabella prova anche di essere completa
//
// Perche' una tabella incompleta e' un oracolo che tace proprio dove serve: le
// varianti che non elenca sono quelle di cui nessuno ha detto niente. I casi
// confrontano quindi la tabella con `TUTTE` del filo e con l'elenco dichiarato
// del cuore, in **entrambi** i versi — nessuna variante del filo senza riga,
// nessuna variante del cuore che non sia il bersaglio di qualcuna.

/// Ogni categoria del filo, e la categoria del cuore che deve produrre.
const ORACOLO_CATEGORIE: &[(CategoriaSulFilo, ErrorCategory)] = &[
    (CategoriaSulFilo::InvalidPlan, ErrorCategory::InvalidPlan),
    (
        CategoriaSulFilo::InvalidConfiguration,
        ErrorCategory::InvalidConfiguration,
    ),
    (CategoriaSulFilo::Schema, ErrorCategory::Schema),
    (CategoriaSulFilo::DataMapping, ErrorCategory::DataMapping),
    (CategoriaSulFilo::Crs, ErrorCategory::Crs),
    (CategoriaSulFilo::Unsupported, ErrorCategory::Unsupported),
    (CategoriaSulFilo::NotFound, ErrorCategory::NotFound),
    (CategoriaSulFilo::Conflict, ErrorCategory::Conflict),
    (
        CategoriaSulFilo::Authentication,
        ErrorCategory::Authentication,
    ),
    (
        CategoriaSulFilo::Authorization,
        ErrorCategory::Authorization,
    ),
    (CategoriaSulFilo::Timeout, ErrorCategory::Timeout),
    (CategoriaSulFilo::Cancelled, ErrorCategory::Cancelled),
    (
        CategoriaSulFilo::ResourceLimit,
        ErrorCategory::ResourceLimit,
    ),
    (CategoriaSulFilo::Io, ErrorCategory::Io),
    (CategoriaSulFilo::Protocol, ErrorCategory::Protocol),
    (CategoriaSulFilo::Transient, ErrorCategory::Transient),
    (CategoriaSulFilo::Execution, ErrorCategory::Execution),
    (
        CategoriaSulFilo::IsolationUnavailable,
        ErrorCategory::IsolationUnavailable,
    ),
    (
        CategoriaSulFilo::UnattributedMemoryPressure,
        ErrorCategory::UnattributedMemoryPressure,
    ),
    (CategoriaSulFilo::Internal, ErrorCategory::Internal),
];

/// Ogni fase del filo, e la fase del cuore che deve produrre.
const ORACOLO_FASI: &[(FaseSulFilo, ErrorPhase)] = &[
    (FaseSulFilo::Validate, ErrorPhase::Validate),
    (FaseSulFilo::Connect, ErrorPhase::Connect),
    (FaseSulFilo::Probe, ErrorPhase::Probe),
    (FaseSulFilo::Prepare, ErrorPhase::Prepare),
    (FaseSulFilo::Read, ErrorPhase::Read),
    (FaseSulFilo::Write, ErrorPhase::Write),
    (FaseSulFilo::Finalize, ErrorPhase::Finalize),
    (FaseSulFilo::Commit, ErrorPhase::Commit),
    (FaseSulFilo::Rollback, ErrorPhase::Rollback),
    (FaseSulFilo::Cleanup, ErrorPhase::Cleanup),
];

/// Ogni effetto del filo, e l'effetto del cuore che deve produrre.
const ORACOLO_EFFETTI: &[(EffettoSulFilo, RemoteEffect)] = &[
    (EffettoSulFilo::None, RemoteEffect::None),
    (EffettoSulFilo::RolledBack, RemoteEffect::RolledBack),
    (EffettoSulFilo::Partial, RemoteEffect::Partial),
    (EffettoSulFilo::Committed, RemoteEffect::Committed),
    (EffettoSulFilo::Unknown, RemoteEffect::Unknown),
];

/// L'errore di dominio prodotto da assi qualunque, per interrogarne uno.
fn con_assi(
    categoria: CategoriaSulFilo,
    fase: FaseSulFilo,
    effetto: EffettoSulFilo,
    retry: RetrySulFilo,
) -> plenora_core::error::PlenoraError {
    super::errore_di_dominio(&errore_sul_filo(categoria, fase, effetto, retry))
}

/// **L'oracolo delle categorie**: ognuna produce quella e non un'altra.
#[test]
fn ogni_categoria_del_filo_produce_quella_giusta() {
    for (dal_filo, attesa) in ORACOLO_CATEGORIE {
        let errore = con_assi(
            *dal_filo,
            FaseSulFilo::Write,
            EffettoSulFilo::None,
            RetrySulFilo::Never {},
        );
        assert_eq!(
            errore.category(),
            *attesa,
            "«{dal_filo:?}» non produce «{attesa:?}»"
        );
    }
}

/// E la tabella **copre tutto**, nei due versi.
///
/// Senza il primo confronto una variante nuova sul filo resterebbe senza
/// oracolo; senza il secondo, una categoria del cuore potrebbe non essere il
/// bersaglio di nessuna e nessuno se ne accorgerebbe.
#[test]
fn l_oracolo_delle_categorie_copre_i_due_elenchi() {
    assert_eq!(
        ORACOLO_CATEGORIE.len(),
        CategoriaSulFilo::TUTTE.len(),
        "la tabella e l'elenco del filo non hanno la stessa lunghezza"
    );
    for (variante, nome_sul_filo) in CategoriaSulFilo::TUTTE {
        assert!(
            ORACOLO_CATEGORIE
                .iter()
                .any(|(dal_filo, _)| dal_filo == variante),
            "«{nome_sul_filo}» non ha una riga nell'oracolo"
        );
    }
    for attesa in ErrorCategory::ALL {
        assert!(
            ORACOLO_CATEGORIE.iter().any(|(_, core)| core == attesa),
            "«{attesa:?}» non e' il bersaglio di nessuna categoria del filo"
        );
    }
}

/// **L'oracolo delle fasi.**
#[test]
fn ogni_fase_del_filo_produce_quella_giusta() {
    for (dal_filo, attesa) in ORACOLO_FASI {
        let errore = con_assi(
            CategoriaSulFilo::Execution,
            *dal_filo,
            EffettoSulFilo::None,
            RetrySulFilo::Never {},
        );
        assert_eq!(
            errore.phase(),
            *attesa,
            "«{dal_filo:?}» non produce «{attesa:?}»"
        );
    }
}

#[test]
fn l_oracolo_delle_fasi_copre_i_due_elenchi() {
    assert_eq!(ORACOLO_FASI.len(), FaseSulFilo::TUTTE.len());
    for (variante, nome_sul_filo) in FaseSulFilo::TUTTE {
        assert!(
            ORACOLO_FASI
                .iter()
                .any(|(dal_filo, _)| dal_filo == variante),
            "«{nome_sul_filo}» non ha una riga nell'oracolo"
        );
    }
    // Il verso opposto — «nessuna fase del cuore resta senza sorgente» — non si
    // puo' interrogare da qui: l'elenco dichiarato di `ErrorPhase` vive sotto
    // `cfg(test)` ed e' privato a `plenora-core`, dove un caso suo lo confronta
    // con le varianti. Cio' che si prova qui e' che le fasi prodotte siano
    // **tutte diverse**: senza, due fasi del filo potrebbero finire sulla stessa
    // e la tabella sembrerebbe comunque completa.
    let distinte: std::collections::BTreeSet<_> =
        ORACOLO_FASI.iter().map(|(_, core)| core.as_str()).collect();
    assert_eq!(distinte.len(), ORACOLO_FASI.len());
}

/// **L'oracolo degli effetti.**
#[test]
fn ogni_effetto_del_filo_produce_quello_giusto() {
    for (dal_filo, atteso) in ORACOLO_EFFETTI {
        let errore = con_assi(
            CategoriaSulFilo::Execution,
            FaseSulFilo::Write,
            *dal_filo,
            RetrySulFilo::Never {},
        );
        assert_eq!(
            errore.remote_effect(),
            *atteso,
            "«{dal_filo:?}» non produce «{atteso:?}»"
        );
    }
}

#[test]
fn l_oracolo_degli_effetti_copre_i_due_elenchi() {
    assert_eq!(ORACOLO_EFFETTI.len(), EffettoSulFilo::TUTTE.len());
    for (variante, nome_sul_filo) in EffettoSulFilo::TUTTE {
        assert!(
            ORACOLO_EFFETTI
                .iter()
                .any(|(dal_filo, _)| dal_filo == variante),
            "«{nome_sul_filo}» non ha una riga nell'oracolo"
        );
    }
    // Come per le fasi: l'elenco dichiarato di `RemoteEffect` non e' visibile da
    // qui, e cio' che si prova e' che nessun effetto del filo ne raggiunga uno
    // gia' raggiunto da un altro.
    let distinti: std::collections::BTreeSet<_> = ORACOLO_EFFETTI
        .iter()
        .map(|(_, core)| core.as_str())
        .collect();
    assert_eq!(distinti.len(), ORACOLO_EFFETTI.len());
}

/// **L'oracolo del ritentativo**, con il ritardo che si conserva.
///
/// `After` non e' una variante come le altre: porta un valore, e una
/// conversione che lo perdesse — o lo scambiasse di unita' — produrrebbe una
/// disposizione giusta con un ritardo sbagliato. Qui i millisecondi sono tre,
/// scelti per non coincidere con nessun default plausibile.
#[test]
fn ogni_disposizione_del_filo_produce_quella_giusta() {
    let oracolo: &[(RetrySulFilo, RetryDisposition)] = &[
        (RetrySulFilo::Never {}, RetryDisposition::Never),
        (RetrySulFilo::Safe {}, RetryDisposition::Safe),
        (
            RetrySulFilo::RequiresIdempotencyKey {},
            RetryDisposition::RequiresIdempotencyKey,
        ),
        (
            RetrySulFilo::RequiresRecovery {},
            RetryDisposition::RequiresRecovery,
        ),
        (
            RetrySulFilo::After { delay_ms: 3 },
            RetryDisposition::After(std::time::Duration::from_millis(3)),
        ),
    ];
    for (dal_filo, attesa) in oracolo {
        let errore = con_assi(
            CategoriaSulFilo::Execution,
            FaseSulFilo::Write,
            EffettoSulFilo::None,
            *dal_filo,
        );
        assert_eq!(
            errore.retry_disposition(),
            *attesa,
            "«{dal_filo:?}» non produce «{attesa:?}»"
        );
    }
    assert_eq!(
        oracolo.len(),
        RetrySulFilo::NOMI.len(),
        "la tabella e l'elenco del filo non hanno la stessa lunghezza"
    );
    // Le cinque disposizioni prodotte sono tutte diverse: `After` si confronta
    // per nome perche' il suo valore e' un ritardo, e due ritardi diversi
    // restano la stessa disposizione.
    let distinte: std::collections::BTreeSet<_> =
        oracolo.iter().map(|(_, core)| core.as_str()).collect();
    assert_eq!(distinte.len(), oracolo.len());
}

/// Il ritardo si conserva **come valore**, non come presenza.
///
/// Tre durate diverse devono arrivare diverse: una conversione che le
/// appiattisse su un default passerebbe il caso precedente se il default fosse
/// tre millisecondi.
#[test]
fn il_ritardo_si_conserva_come_valore() {
    use plenora_core::error::RetryDisposition;
    for millisecondi in [0_u64, 3, 1500, 86_400_000] {
        let errore = con_assi(
            CategoriaSulFilo::Transient,
            FaseSulFilo::Read,
            EffettoSulFilo::None,
            RetrySulFilo::After {
                delay_ms: millisecondi,
            },
        );
        assert_eq!(
            errore.retry_disposition(),
            RetryDisposition::After(std::time::Duration::from_millis(millisecondi)),
            "il ritardo di {millisecondi} ms non si conserva"
        );
    }
}

// --- fatti duplicati e contraddittori ----------------------------------------

/// Il valore di una chiave del rapporto.
fn valore(registro: &Registro, chiave: &str) -> String {
    registro
        .evidenza_dei_fatti()
        .into_iter()
        .find(|(k, _)| *k == chiave)
        .map(|(_, v)| v)
        .unwrap_or_default()
}

/// **Due esiti diversi sono un impedimento, non un arbitrato.**
///
/// Scegliere il primo o l'ultimo sarebbe decidere al posto di chi legge, e
/// senza dirglielo.
#[test]
fn due_esiti_diversi_impediscono_la_conclusione() {
    let registro = registro(vec![
        esito_di_panico(FormaPanicSulFilo::Statico),
        esito_di_successo(),
        Fatto::FineDelCanale,
        uscita_pulita(),
        Fatto::DominioQuiescente,
    ]);
    let Err(Impedimento::FattiContraddittori(quali)) = registro.concludi(&[]) else {
        panic!("due esiti non si lasciano ridurre a un esito");
    };
    assert_eq!(quali.len(), 1);
    assert!(quali[0].contains("2 esiti"), "{}", quali[0]);
}

/// E **l'ordine non cambia l'impedimento**: gli stessi due esiti, invertiti,
/// danno la stessa cosa.
///
/// E' la corsa a ordine invertito, fatta senza processi: se il difetto
/// dipendesse dall'ordine sarebbe di nuovo un arbitrato, solo mascherato da
/// errore.
#[test]
fn l_impedimento_di_due_esiti_non_dipende_dall_ordine() {
    let uno = registro(vec![
        esito_di_panico(FormaPanicSulFilo::Statico),
        esito_di_successo(),
    ])
    .concludi(&[])
    .expect_err("due esiti impediscono");
    let altro = registro(vec![
        esito_di_successo(),
        esito_di_panico(FormaPanicSulFilo::Statico),
    ])
    .concludi(&[])
    .expect_err("due esiti impediscono");
    assert_eq!(uno, altro, "lo stesso insieme di fatti, lo stesso difetto");
}

/// Due esiti **uguali** sono comunque un impedimento.
///
/// Che i contenuti coincidano non rimette in piedi il protocollo: l'esito
/// chiude la conversazione, e un messaggio dopo la chiusura dice che l'altro
/// capo non la sta seguendo. Non c'e' niente da arbitrare, e per questo non si
/// arbitra: si rifiuta.
#[test]
fn anche_due_esiti_uguali_sono_un_impedimento() {
    let registro = registro(vec![esito_di_successo(), esito_di_successo()]);
    assert!(registro.concludi(&[]).is_err());
}

/// **Due uscite diverse sono un impedimento**: una delle due letture e' rotta,
/// e non c'e' modo di sapere quale.
#[test]
fn due_uscite_diverse_impediscono_la_conclusione() {
    let registro = registro(vec![
        uscita_pulita(),
        Fatto::UscitaDelWorker(UscitaOsservata::Segnale(9)),
        Fatto::FineDelCanale,
        Fatto::DominioQuiescente,
    ]);
    let Err(Impedimento::FattiContraddittori(quali)) = registro.concludi(&[]) else {
        panic!("due uscite diverse non si lasciano ridurre");
    };
    assert!(quali[0].contains("modi diversi"), "{}", quali[0]);
}

/// Due uscite **uguali** sono la stessa lettura fatta due volte, e non
/// impediscono niente.
///
/// E' la differenza con l'esito: l'uscita la osserviamo noi, e osservarla due
/// volte allo stesso modo non e' una contraddizione. Senza questo caso, un
/// codice che rifiutasse ogni duplicato passerebbe i tre casi precedenti e
/// romperebbe un produttore che riporta due volte.
#[test]
fn due_uscite_uguali_non_impediscono_niente() {
    let registro = registro(vec![
        esito_di_successo(),
        uscita_pulita(),
        uscita_pulita(),
        Fatto::FineDelCanale,
        Fatto::DominioQuiescente,
    ]);
    assert!(registro.quattro_fatti_positivi());
    let esito = registro.concludi(&[]).expect("nessun impedimento");
    assert_eq!(nome(&esito), "da_verificare");
}

/// Due letture dell'evidenza dopo la quiescenza sono un impedimento: dopo la
/// quiescenza non cambia piu' niente, quindi non c'e' motivo di leggere due
/// volte.
#[test]
fn due_letture_dell_evidenza_impediscono_la_conclusione() {
    let registro = registro(vec![
        Fatto::EvidenzaDelDominio(evidenza_vuota()),
        Fatto::EvidenzaDelDominio(evidenza_vuota()),
    ]);
    assert!(registro.concludi(&[]).is_err());
}

/// Le contraddizioni si **accumulano** e non dipendono dall'ordine.
///
/// Riportarne una sola manderebbe a correggere quella e a ritrovare le altre
/// una per volta; riportarle in ordine d'arrivo renderebbe due rapporti degli
/// stessi fatti diversi fra loro.
#[test]
fn le_contraddizioni_si_accumulano_e_non_dipendono_dall_ordine() {
    let costruisci = |invertito: bool| {
        let mut fatti = vec![
            esito_di_successo(),
            esito_di_panico(FormaPanicSulFilo::Statico),
            uscita_pulita(),
            Fatto::UscitaDelWorker(UscitaOsservata::Codice(3)),
        ];
        if invertito {
            fatti.reverse();
        }
        registro(fatti)
            .concludi(&[])
            .expect_err("due contraddizioni")
    };
    let dritto = costruisci(false);
    let rovescio = costruisci(true);
    let Impedimento::FattiContraddittori(quali) = &dritto else {
        panic!("questi fatti si contraddicono, non impediscono la nascita di un filo");
    };
    assert_eq!(quali.len(), 2, "entrambe si riportano: {quali:?}");
    assert_eq!(dritto, rovescio, "l'ordine non cambia l'impedimento");
}

/// Le osservazioni mancate si conservano tutte.
#[test]
fn le_osservazioni_mancate_si_conservano_tutte() {
    let registro = registro(vec![
        Fatto::OsservazioneImpossibile {
            chi: "quiescenza",
            motivo: "cgroup.events illeggibile".to_owned(),
        },
        Fatto::OsservazioneImpossibile {
            chi: "evidenza",
            motivo: "memory.events illeggibile".to_owned(),
        },
    ]);
    let mancate = valore(&registro, "osservazioni_mancate");
    assert!(mancate.contains("quiescenza"), "{mancate}");
    assert!(mancate.contains("evidenza"), "{mancate}");
}

/// **La sentinella sulla diagnostica**: due esiti diversi con diagnostiche
/// diverse, in ordine opposto, danno lo **stesso** rapporto.
///
/// # Perche' e' un caso a se'
///
/// Perche' il registro rifiuta di arbitrare fra due esiti, ma la diagnostica
/// viaggia **dentro** l'esito, e prendere quella del primo arrivato sarebbe lo
/// stesso arbitrato fatto un passo piu' in basso — dove non si vede. Quando il
/// protocollo e' gia' contraddittorio, non si elegge niente: restano tutte, e
/// il rapporto le riporta ordinate.
#[test]
fn due_esiti_con_diagnostiche_diverse_danno_lo_stesso_rapporto() {
    let con_diagnostica = |scope: &str, osservate: u64| {
        let mut errore = errore_sul_filo(
            CategoriaSulFilo::Execution,
            FaseSulFilo::Write,
            EffettoSulFilo::None,
            RetrySulFilo::Never {},
        );
        errore.diagnostica = Some(DiagnosticaSulFilo {
            contract: "contratto".to_owned(),
            scope: scope.to_owned(),
            completeness: "completa".to_owned(),
            observed_total: osservate,
            conteggi: Vec::new(),
            esempi: Vec::new(),
            esempi_troncati: false,
        });
        esito_di_errore(errore)
    };

    let rapporto = |invertito: bool| {
        let mut fatti = vec![
            con_diagnostica("colonna-a", 1),
            con_diagnostica("colonna-b", 2),
        ];
        if invertito {
            fatti.reverse();
        }
        let registro = registro(fatti);
        let righe = registro.evidenza_dei_fatti();
        let difetto = registro.concludi(&[]).expect_err("due esiti impediscono");
        (righe, difetto)
    };

    let (dritto, difetto_dritto) = rapporto(false);
    let (rovescio, difetto_rovescio) = rapporto(true);

    assert_eq!(dritto, rovescio, "il rapporto non dipende dall'ordine");
    assert_eq!(difetto_dritto, difetto_rovescio);

    // E le diagnostiche ci sono **entrambe**: nessuna eletta, nessuna persa.
    let diagnostiche = dritto
        .iter()
        .find(|(chiave, _)| *chiave == "diagnostica_di_riga")
        .map(|(_, valore)| valore.clone())
        .unwrap_or_default();
    assert!(diagnostiche.contains("colonna-a"), "{diagnostiche}");
    assert!(diagnostiche.contains("colonna-b"), "{diagnostiche}");
}

/// Con **un solo** esito la diagnostica arriva invece fino all'esito del
/// supervisore: non c'e' niente da arbitrare, e tenerla fuori sarebbe una
/// perdita gratuita.
#[test]
fn con_un_solo_esito_la_diagnostica_arriva_all_esito() {
    let mut errore = errore_sul_filo(
        CategoriaSulFilo::Execution,
        FaseSulFilo::Write,
        EffettoSulFilo::None,
        RetrySulFilo::Never {},
    );
    errore.diagnostica = Some(DiagnosticaSulFilo {
        contract: "contratto".to_owned(),
        scope: "colonna-sola".to_owned(),
        completeness: "completa".to_owned(),
        observed_total: 7,
        conteggi: Vec::new(),
        esempi: Vec::new(),
        esempi_troncati: false,
    });
    let esito = registro(vec![
        esito_di_errore(errore),
        Fatto::FineDelCanale,
        uscita_pulita(),
        Fatto::DominioQuiescente,
    ])
    .concludi(&[])
    .expect("nessun impedimento");
    let portata = esito.diagnostica_di_riga.expect("la diagnostica arriva");
    assert_eq!(portata.scope, "colonna-sola");
    assert_eq!(portata.observed_total, 7);
}

/// Un'evidenza senza nessuna lettura, per i casi che non la misurano.
fn evidenza_vuota() -> Box<plenora_core::error::EvidenzaDiLimite> {
    Box::new(plenora_core::error::EvidenzaDiLimite {
        oom_locali: None,
        uccisi_nel_dominio: None,
        uccisi_nella_gerarchia: None,
        group_kill_locale: None,
        oom_degli_antenati: plenora_core::error::PressioneDegliAntenati::alla_radice(),
        diagnostica: plenora_core::error::DiagnosticaSupplementare {
            tetto_byte: 64 * 1024 * 1024,
            picco_byte: None,
            respinte_al_tetto: None,
        },
    })
}

// --- la barriera vale per ogni esito, non per uno solo ------------------------

/// Il nome di una famiglia di fatti, e come costruirla.
///
/// Un alias e non il tipo scritto per esteso: la firma senza nome non si legge,
/// e il nome dice cio' che la coppia e' — un'etichetta per i messaggi, e una
/// ricetta.
type Famiglia = (&'static str, fn() -> Vec<Fatto>);

/// Cinque famiglie di fatti che danno cinque esiti diversi, senza la quiescenza.
///
/// La quiescenza si aggiunge fuori, cosi' che le stesse cinque righe si possano
/// percorrere due volte: con e senza.
fn famiglie_senza_quiescenza() -> Vec<Famiglia> {
    vec![
        ("successo dichiarato", || {
            vec![esito_di_successo(), Fatto::FineDelCanale, uscita_pulita()]
        }),
        ("errore dichiarato", || {
            vec![
                esito_di_errore(errore_sul_filo(
                    CategoriaSulFilo::ResourceLimit,
                    FaseSulFilo::Write,
                    EffettoSulFilo::None,
                    RetrySulFilo::Never {},
                )),
                Fatto::FineDelCanale,
                uscita_pulita(),
            ]
        }),
        ("panico dichiarato", || {
            vec![
                esito_di_panico(FormaPanicSulFilo::Statico),
                Fatto::FineDelCanale,
                uscita_pulita(),
            ]
        }),
        ("tempo scaduto", || {
            vec![Fatto::TempoScaduto, Fatto::FineDelCanale, uscita_pulita()]
        }),
        ("cancellazione", || {
            vec![
                Fatto::CancellazioneRichiesta,
                Fatto::FineDelCanale,
                uscita_pulita(),
            ]
        }),
    ]
}

/// **Con la quiescenza le cinque famiglie danno cinque esiti diversi.**
///
/// Non prova la barriera: prova che la tabella che la prova **non e' vuota**.
/// Cinque righe che producessero tutte lo stesso esito renderebbero il caso
/// successivo una sola misura ripetuta cinque volte, e la falla da escludere —
/// «la barriera vale solo per `DaVerificare`» — passerebbe lo stesso.
#[test]
fn le_cinque_famiglie_danno_cinque_esiti_distinti() {
    let mut visti = Vec::new();
    for (chi, fatti) in famiglie_senza_quiescenza() {
        let mut tutti = fatti();
        tutti.push(Fatto::DominioQuiescente);
        let esito = registro(tutti)
            .concludi(&[])
            .unwrap_or_else(|impedimento| panic!("{chi}: doveva concludere: {impedimento}"));
        visti.push(nome(&esito));
    }
    let mut ordinati = visti.clone();
    ordinati.sort_unstable();
    ordinati.dedup();
    assert_eq!(
        ordinati.len(),
        5,
        "le famiglie devono dare esiti diversi, e invece: {visti:?}"
    );
}

/// **Senza la quiescenza nessuna delle cinque produce un esito.**
///
/// # Perche' vale per tutte e non solo per il permesso di proseguire
///
/// Perche' la quiescenza non e' cio' che autorizza a **proseguire**: e' cio' che
/// rende i contatori un'osservazione invece di una fotografia in movimento.
/// Finche' nel dominio c'e' qualcuno vivo, un OOM puo' ancora arrivare — e la
/// stessa esecuzione diventerebbe `Timeout` oppure `LimiteAttribuito` secondo
/// quando il kernel lo consegna. Un errore dichiarato e un panico non sono meno
/// esposti: sono classificazioni fatte sugli stessi contatori.
///
/// Restringere il controllo a `DaVerificare` lascerebbe verdi quattro di queste
/// cinque righe. E' esattamente cio' che il caso esclude.
#[test]
fn senza_quiescenza_nessuna_famiglia_produce_un_esito() {
    for (chi, fatti) in famiglie_senza_quiescenza() {
        let Err(Impedimento::BarrieraIncompleta(manca)) = registro(fatti()).concludi(&[]) else {
            panic!("{chi}: su un dominio ancora abitato non c'e' un esito da dare");
        };
        assert!(
            manca
                .iter()
                .any(|riga| riga == "il dominio non e' quiescente"),
            "{chi}: la ragione deve essere la quiescenza, e invece: {manca:?}"
        );
    }
}

/// **Un difetto della conduzione basta a fermare la barriera.**
///
/// Non e' in contraddizione con «i difetti non sono un esito»: la barriera non
/// chiede com'e' andata, chiede se lo si e' visto abbastanza da poterlo dire. Un
/// produttore che non ha accodato e un drenaggio che ha rinunciato sono
/// esattamente le ragioni per cui la risposta puo' essere no.
///
/// Il caso parte da fatti **completi**, cosi' che a fare la differenza sia solo
/// il difetto: senza, sarebbe la solita barriera incompleta con un'altra
/// etichetta.
#[test]
fn un_difetto_della_conduzione_ferma_la_barriera() {
    let completi = || {
        vec![
            esito_di_successo(),
            Fatto::FineDelCanale,
            uscita_pulita(),
            Fatto::DominioQuiescente,
        ]
    };
    // Senza difetti si conclude: e' la meta' che rende l'altra significativa.
    assert!(registro(completi()).concludi(&[]).is_ok());

    let Err(Impedimento::BarrieraIncompleta(manca)) =
        registro(completi()).concludi(&["il drenaggio ha rinunciato dopo 30 s".to_owned()])
    else {
        panic!("un'osservazione dichiarata incompleta non autorizza a concludere");
    };
    assert!(
        manca
            .iter()
            .any(|riga| riga.contains("il drenaggio ha rinunciato")),
        "{manca:?}"
    );
}
