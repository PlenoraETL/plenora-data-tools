//! Prove dell'handshake.
//!
//! Tre strati che non si coprono a vicenda:
//!
//! - il **giro nominale**, che dice che due lati d'accordo si accordano;
//! - un **caso per ogni asse**, perche' un confronto dimenticato lascerebbe
//!   passare esattamente la cosa che quell'asse esiste per fermare;
//! - le **transizioni vietate**, che sono la meta' della macchina a stati:
//!   dire cosa succede quando arriva il messaggio sbagliato e' dire cosa fa
//!   davvero.

use plenora_core::{ErrorCategory, PlenoraError};

use super::{
    limiti_correnti, AtteseSupervisore, Descrizione, DescrizioneLocale, SupervisoreInAttesa,
    WorkerInAttesa,
};
use crate::commit_token::CommitToken;
use crate::protocollo::digest::DigestSha256;
use crate::protocollo::messaggi::{
    Ambiente, Annulla, BackendDinamico, Corpo, DescrittoreIngresso, FormatoIngresso, Frame,
    IdentitaArtefatto, IdentitaResolver, Incarico, LimitiDichiarati, Progresso, RisorsaRisolta,
    Risposta, Saluto,
};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// I digest delle fixture, come costanti: i test li condividono, e ciascuno
/// resta libero di costruirsi il proprio caso.
const ARTEFATTO: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const INSIEME: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PIANO: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const CONTRATTO: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// Un digest dalla forma canonica, per le fixture.
fn digest(testo: &str) -> DigestSha256 {
    DigestSha256::da_esadecimale(testo).expect("canonico")
}

fn token() -> CommitToken {
    CommitToken::da_esadecimale(TOKEN).expect("canonico")
}

fn risorsa(nome: &str) -> RisorsaRisolta {
    RisorsaRisolta {
        nome: nome.to_owned(),
        versione: "1".to_owned(),
        percorso: format!("/r/{nome}"),
    }
}

fn backend(nome: &str) -> BackendDinamico {
    BackendDinamico {
        nome: nome.to_owned(),
        versione: "3.8".to_owned(),
        percorso: format!("/l/lib{nome}.so"),
    }
}

fn ambiente() -> Ambiente {
    Ambiente {
        digest_insieme: digest(INSIEME),
        acquisizione_dinamica: false,
        risorse: vec![risorsa("grid"), risorsa("tin")],
        backend_dinamici: vec![backend("gdal"), backend("proj")],
    }
}

fn comune() -> Descrizione {
    Descrizione {
        artefatto: IdentitaArtefatto {
            digest: digest(ARTEFATTO),
            versione: "1.0".to_owned(),
        },
        resolver: IdentitaResolver {
            identita: "proj".to_owned(),
            versione: "9.4".to_owned(),
        },
        ambiente: ambiente(),
    }
}

fn locale() -> DescrizioneLocale {
    DescrizioneLocale {
        comune: comune(),
        capability: vec!["arrow_ipc".to_owned(), "wkb".to_owned()],
    }
}

fn attese() -> AtteseSupervisore {
    AtteseSupervisore {
        comune: comune(),
        capability_richieste: vec!["arrow_ipc".to_owned()],
        commit_token: token(),
    }
}

/// Il giro completo: `Saluto`, verifica, `Risposta`, verifica, accordo.
///
/// Rende anche i due stati accordati, perche' quasi ogni altro test parte da
/// qui.
pub fn giro_nominale() -> (super::HandshakeAccettato, super::WorkerAccordato) {
    let supervisore = SupervisoreInAttesa::nuovo(attese()).expect("attese coerenti");
    let saluto = Frame::nuovo(Corpo::Saluto(Box::new(supervisore.saluto().clone())));

    let worker = WorkerInAttesa::nuovo(locale()).expect("descrizione coerente");
    let (risposta, accordato) = worker.ricevi(saluto).expect("il worker accetta il saluto");

    let accettato = supervisore
        .ricevi(Frame::nuovo(Corpo::Risposta(Box::new(risposta))))
        .expect("il supervisore accetta la risposta");
    (accettato, accordato)
}

#[test]
fn il_giro_nominale_si_conclude_e_i_due_lati_hanno_lo_stesso_token() {
    let (accettato, accordato) = giro_nominale();
    assert_eq!(accettato.commit_token(), accordato.commit_token());
    assert_eq!(accettato.commit_token().in_esadecimale(), TOKEN);
}

/// I limiti dichiarati sono quelli applicati.
///
/// Se `limiti_correnti` li riscrivesse a mano, il `Saluto` porterebbe una
/// promessa che il codice non mantiene, e l'altro capo l'accetterebbe.
#[test]
fn i_limiti_dichiarati_vengono_dalle_costanti_applicate() {
    use crate::protocollo::codifica::MAX_PROTOCOL_FRAME_BYTES;
    use crate::protocollo::limiti::{
        MAX_MESSAGGI_VERSO_SUPERVISORE, MAX_MESSAGGI_VERSO_WORKER, MAX_PIANO_CANONICO_BYTES,
    };

    let dichiarati = limiti_correnti();
    assert_eq!(dichiarati.max_frame_bytes, MAX_PROTOCOL_FRAME_BYTES as u64);
    assert_eq!(
        dichiarati.max_piano_canonico_bytes,
        MAX_PIANO_CANONICO_BYTES as u64
    );
    assert_eq!(
        dichiarati.max_messaggi_verso_worker,
        MAX_MESSAGGI_VERSO_WORKER as u64
    );
    assert_eq!(
        dichiarati.max_messaggi_verso_supervisore,
        MAX_MESSAGGI_VERSO_SUPERVISORE as u64
    );

    // E il `Saluto` li porta senza che nessuno possa sostituirli.
    let supervisore = SupervisoreInAttesa::nuovo(attese()).expect("coerenti");
    assert_eq!(supervisore.saluto().limiti, dichiarati);
}

// ---------------------------------------------------------------------------
// Un caso per ogni asse
// ---------------------------------------------------------------------------

/// Manda al worker un `Saluto` costruito a mano.
fn worker_riceve(saluto: Saluto) -> Result<(), PlenoraError> {
    let worker = WorkerInAttesa::nuovo(locale()).expect("coerente");
    worker
        .ricevi(Frame::nuovo(Corpo::Saluto(Box::new(saluto))))
        .map(|_| ())
}

/// Manda al supervisore una `Risposta` costruita a mano.
fn supervisore_riceve(risposta: Risposta) -> Result<(), PlenoraError> {
    let supervisore = SupervisoreInAttesa::nuovo(attese()).expect("coerenti");
    supervisore
        .ricevi(Frame::nuovo(Corpo::Risposta(Box::new(risposta))))
        .map(|_| ())
}

fn saluto_nominale() -> Saluto {
    SupervisoreInAttesa::nuovo(attese())
        .expect("coerenti")
        .saluto()
        .clone()
}

fn risposta_nominale() -> Risposta {
    Risposta {
        artefatto: locale().comune.artefatto,
        resolver: locale().comune.resolver,
        ambiente: ambiente(),
        capability: locale().capability,
    }
}

/// L'identita' dell'artefatto e' `Protocol`: i due lati non sono lo stesso
/// binario, e non e' una questione di configurazione.
#[test]
fn un_artefatto_diverso_e_protocol_su_entrambi_i_lati() {
    // Digest **canonici** e diversi: con una forma non canonica il rifiuto
    // arriverebbe dalla verifica di forma, che e' anch'essa `Protocol`, e il
    // test passerebbe senza aver mai raggiunto il confronto che dichiara di
    // provare.
    let mut saluto = saluto_nominale();
    saluto.artefatto.digest = digest(&"f".repeat(64));
    let errore = worker_riceve(saluto).expect_err("digest diverso");
    assert_eq!(errore.category(), ErrorCategory::Protocol, "{errore}");

    let mut saluto = saluto_nominale();
    saluto.artefatto.versione = "2.0".to_owned();
    let errore = worker_riceve(saluto).expect_err("versione diversa");
    assert_eq!(errore.category(), ErrorCategory::Protocol, "{errore}");

    let mut risposta = risposta_nominale();
    risposta.artefatto.digest = digest(&"f".repeat(64));
    let errore = supervisore_riceve(risposta).expect_err("digest diverso");
    assert_eq!(errore.category(), ErrorCategory::Protocol, "{errore}");
}

/// Il resolver e' `InvalidConfiguration`: due binari identici possono
/// risolvere CRS diversi.
#[test]
fn un_resolver_diverso_e_configurazione() {
    let mut saluto = saluto_nominale();
    saluto.resolver.identita = "altro".to_owned();
    let errore = worker_riceve(saluto).expect_err("resolver diverso");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );

    let mut risposta = risposta_nominale();
    risposta.resolver.versione = "9.5".to_owned();
    let errore = supervisore_riceve(risposta).expect_err("versione diversa");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );
}

#[test]
fn un_digest_dell_insieme_diverso_e_configurazione() {
    // Un digest **canonico** e diverso: con una forma non canonica il rifiuto
    // arriverebbe prima, dalla verifica di forma, e questo test proverebbe
    // quella invece del confronto che dichiara di provare.
    let mut saluto = saluto_nominale();
    saluto.ambiente.digest_insieme = digest(&"c".repeat(64));
    let errore = worker_riceve(saluto).expect_err("insieme diverso");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );
}

/// `acquisizione_dinamica` deve essere falsa su **entrambi** i lati.
///
/// Con l'acquisizione dinamica il digest dell'insieme e' una fotografia
/// scaduta, e confrontare i digest non direbbe piu' nulla: per questo si
/// controlla prima di confrontarli.
#[test]
fn l_acquisizione_dinamica_e_rifiutata_da_entrambi_i_lati() {
    let mut saluto = saluto_nominale();
    saluto.ambiente.acquisizione_dinamica = true;
    let errore = worker_riceve(saluto).expect_err("acquisizione dinamica");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );
    assert!(errore.to_string().contains("acquisizione_dinamica"));

    let mut risposta = risposta_nominale();
    risposta.ambiente.acquisizione_dinamica = true;
    let errore = supervisore_riceve(risposta).expect_err("acquisizione dinamica");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );

    // E il supervisore non riesce nemmeno a costruirsi con quella dichiarata:
    // scoprire un difetto proprio dalla risposta altrui sarebbe tardi.
    let mut sue = attese();
    sue.comune.ambiente.acquisizione_dinamica = true;
    assert!(SupervisoreInAttesa::nuovo(sue).is_err());
}

#[test]
fn risorse_o_backend_diversi_sono_configurazione() {
    let mut saluto = saluto_nominale();
    saluto.ambiente.risorse = vec![risorsa("grid")];
    let errore = worker_riceve(saluto).expect_err("risorse diverse");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );

    let mut saluto = saluto_nominale();
    saluto.ambiente.backend_dinamici = vec![backend("gdal")];
    let errore = worker_riceve(saluto).expect_err("backend diversi");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );

    // Anche un solo campo dentro una risorsa: la versione e' identita'.
    let mut saluto = saluto_nominale();
    saluto.ambiente.risorse[0].versione = "2".to_owned();
    assert!(
        worker_riceve(saluto).is_err(),
        "versione di risorsa ignorata"
    );

    let mut saluto = saluto_nominale();
    saluto.ambiente.risorse[0].percorso = "/altrove".to_owned();
    assert!(
        worker_riceve(saluto).is_err(),
        "percorso di risorsa ignorato"
    );
}

/// Una capability richiesta e assente e' configurazione.
///
/// Il verso conta: al worker e' concesso offrirne **di piu'**, non di meno.
#[test]
fn una_capability_mancante_e_configurazione_ma_una_in_piu_va_bene() {
    let mut risposta = risposta_nominale();
    risposta.capability = vec!["wkb".to_owned()];
    let errore = supervisore_riceve(risposta).expect_err("capability mancante");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );
    // Il conteggio, non il nome: la capability mancante si deduce per
    // differenza da quelle offerte, che arrivano dal filo.
    assert!(
        errore.to_string().contains("1 delle 1 capability"),
        "{errore}"
    );

    let mut risposta = risposta_nominale();
    risposta.capability.push("extra".to_owned());
    supervisore_riceve(risposta).expect("una capability in piu' non e' un problema");
}

/// Il nome di un limite e il modo di guastarlo.
type CampoDeiLimiti = (&'static str, fn(&mut LimitiDichiarati));

/// I limiti sono `Protocol`: i due lati applicherebbero regole diverse allo
/// stesso canale.
#[test]
fn limiti_diversi_sono_protocol_e_il_motivo_nomina_il_campo() {
    let campi: [CampoDeiLimiti; 4] = [
        ("max_frame_bytes", |l| l.max_frame_bytes += 1),
        ("max_piano_canonico_bytes", |l| {
            l.max_piano_canonico_bytes -= 1;
        }),
        ("max_messaggi_verso_worker", |l| {
            l.max_messaggi_verso_worker = 99;
        }),
        ("max_messaggi_verso_supervisore", |l| {
            l.max_messaggi_verso_supervisore = 0;
        }),
    ];
    for (nome, guasta) in campi {
        let mut saluto = saluto_nominale();
        guasta(&mut saluto.limiti);
        let errore = worker_riceve(saluto).expect_err("limiti diversi");
        assert_eq!(errore.category(), ErrorCategory::Protocol, "{errore}");
        assert!(
            errore.to_string().contains(nome),
            "il motivo non nomina `{nome}`: {errore}"
        );
    }
}

// ---------------------------------------------------------------------------
// La rappresentazione canonica
// ---------------------------------------------------------------------------

/// Riordinare risorse, backend o capability non cambia l'esito.
///
/// Sono insiemi: un confronto posizionale li avrebbe fatti divergere per un
/// dettaglio di costruzione, e l'esito sarebbe dipeso dall'ordine in cui
/// qualcuno ha riempito un `Vec`.
#[test]
fn l_ordine_di_risorse_backend_e_capability_non_conta() {
    let mut saluto = saluto_nominale();
    saluto.ambiente.risorse.reverse();
    saluto.ambiente.backend_dinamici.reverse();
    worker_riceve(saluto).expect("l'ordine non deve contare");

    let mut risposta = risposta_nominale();
    risposta.ambiente.risorse.reverse();
    risposta.ambiente.backend_dinamici.reverse();
    risposta.capability.reverse();
    supervisore_riceve(risposta).expect("l'ordine non deve contare");

    // E anche dal lato di chi le dichiara: un supervisore che le elenca al
    // contrario produce un `Saluto` che il worker accetta lo stesso.
    let mut sue = attese();
    sue.comune.ambiente.risorse.reverse();
    sue.capability_richieste.reverse();
    let supervisore = SupervisoreInAttesa::nuovo(sue).expect("coerenti");
    let saluto = Frame::nuovo(Corpo::Saluto(Box::new(supervisore.saluto().clone())));
    let worker = WorkerInAttesa::nuovo(locale()).expect("coerente");
    let (risposta, _) = worker.ricevi(saluto).expect("l'ordine non deve contare");
    supervisore
        .ricevi(Frame::nuovo(Corpo::Risposta(Box::new(risposta))))
        .expect("l'ordine non deve contare");
}

/// I duplicati si **rifiutano**, non si riducono.
///
/// Due risorse con lo stesso nome sono un'ambiguita' su quale verra' aperta, e
/// ridurle a una sceglierebbe al posto di chi ha costruito l'ambiente.
#[test]
fn i_duplicati_sono_rifiutati_ovunque() {
    // Nella descrizione propria, prima ancora di spedire.
    let mut sue = attese();
    sue.comune.ambiente.risorse.push(risorsa("grid"));
    let errore = SupervisoreInAttesa::nuovo(sue).expect_err("risorsa duplicata");
    assert!(
        errore.to_string().contains("3 voci, 2 nomi distinti"),
        "{errore}"
    );
    assert_eq!(errore.category(), ErrorCategory::InvalidConfiguration);

    let mut sue = attese();
    sue.comune.ambiente.backend_dinamici.push(backend("gdal"));
    assert!(
        SupervisoreInAttesa::nuovo(sue).is_err(),
        "backend duplicato"
    );

    let mut sue = attese();
    sue.capability_richieste.push("arrow_ipc".to_owned());
    assert!(
        SupervisoreInAttesa::nuovo(sue).is_err(),
        "capability richiesta due volte"
    );

    let mut sua = locale();
    sua.capability.push("wkb".to_owned());
    assert!(
        WorkerInAttesa::nuovo(sua).is_err(),
        "capability offerta due volte"
    );

    // E in cio' che arriva dal filo.
    let mut saluto = saluto_nominale();
    saluto.ambiente.risorse.push(risorsa("grid"));
    assert!(
        worker_riceve(saluto).is_err(),
        "risorsa duplicata accettata"
    );

    let mut risposta = risposta_nominale();
    risposta.capability.push("wkb".to_owned());
    assert!(
        supervisore_riceve(risposta).is_err(),
        "capability duplicata accettata"
    );
}

/// Un duplicato con lo stesso nome ma contenuto diverso e' comunque un
/// duplicato.
///
/// E' il caso che un dedup ingenuo lascerebbe passare: due voci distinte come
/// valori, ma con la stessa identita'.
#[test]
fn due_risorse_con_lo_stesso_nome_e_versioni_diverse_sono_un_duplicato() {
    let mut sue = attese();
    let mut gemella = risorsa("grid");
    gemella.versione = "2".to_owned();
    sue.comune.ambiente.risorse.push(gemella);
    let errore = SupervisoreInAttesa::nuovo(sue).expect_err("nome ripetuto");
    assert!(
        errore.to_string().contains("3 voci, 2 nomi distinti"),
        "{errore}"
    );
}

// ---------------------------------------------------------------------------
// Le transizioni: quelle ammesse e quelle vietate
// ---------------------------------------------------------------------------

fn incarico() -> Frame {
    Frame::nuovo(Corpo::Incarico(Box::new(Incarico {
        piano_canonico: serde_json::value::RawValue::from_string(
            r#"{"schema_version":6}"#.to_owned(),
        )
        .expect("JSON valido"),
        plan_hash_atteso: digest(PIANO),
        ingressi: vec![DescrittoreIngresso {
            nome: "in".to_owned(),
            percorso: "/d/a.arrow".to_owned(),
            formato: FormatoIngresso::File,
            contract_fingerprint_atteso: digest(CONTRATTO),
        }],
        artefatto_temporaneo: "/t/out.arrow".to_owned(),
    })))
}

/// Un `Incarico` prima dell'accordo e' rifiutato **col suo nome**.
///
/// Non «messaggio inatteso»: e' il caso che il perimetro nomina, e chiamarlo
/// cosi' dice a chi legge che l'handshake non e' concluso, non che il
/// messaggio sia malformato.
#[test]
fn un_incarico_prima_dell_accordo_e_rifiutato() {
    let worker = WorkerInAttesa::nuovo(locale()).expect("coerente");
    let errore = worker.ricevi(incarico()).expect_err("incarico anticipato");
    assert_eq!(errore.category(), ErrorCategory::Protocol, "{errore}");
    assert!(
        errore.to_string().contains("prima dell'accordo"),
        "il motivo non nomina il caso: {errore}"
    );
}

/// Dopo l'accordo, l'`Incarico` si riceve.
#[test]
fn dopo_l_accordo_l_incarico_si_riceve_col_token_concordato() {
    let (_, accordato) = giro_nominale();
    let (ricevuto, token_concordato) = accordato
        .ricevi_incarico(incarico())
        .expect("l'incarico si riceve");
    assert_eq!(ricevuto.plan_hash_atteso, digest(PIANO));
    assert_eq!(token_concordato.in_esadecimale(), TOKEN);
}

/// Il worker non accetta una `Risposta`: viaggia dalla parte sbagliata.
#[test]
fn un_messaggio_nella_direzione_sbagliata_e_rifiutato_come_tale() {
    let worker = WorkerInAttesa::nuovo(locale()).expect("coerente");
    let errore = worker
        .ricevi(Frame::nuovo(Corpo::Risposta(Box::new(risposta_nominale()))))
        .expect_err("direzione sbagliata");
    assert_eq!(errore.category(), ErrorCategory::Protocol, "{errore}");
    assert!(
        errore.to_string().contains("viaggia verso"),
        "il motivo non parla di direzione: {errore}"
    );

    // E il supervisore non accetta un `Saluto`.
    let supervisore = SupervisoreInAttesa::nuovo(attese()).expect("coerenti");
    let errore = supervisore
        .ricevi(Frame::nuovo(Corpo::Saluto(Box::new(saluto_nominale()))))
        .expect_err("direzione sbagliata");
    assert!(errore.to_string().contains("viaggia verso"), "{errore}");
}

/// Un tipo giusto per direzione ma sbagliato per lo stato e' fuori sequenza.
#[test]
fn un_tipo_fuori_sequenza_e_rifiutato() {
    // Al worker, che aspetta il `Saluto`, arriva un `Annulla`: direzione
    // giusta, momento sbagliato.
    let worker = WorkerInAttesa::nuovo(locale()).expect("coerente");
    let errore = worker
        .ricevi(Frame::nuovo(Corpo::Annulla(Annulla {
            motivo: "x".to_owned(),
        })))
        .expect_err("fuori sequenza");
    assert!(errore.to_string().contains("atteso"), "{errore}");

    // Al supervisore, che aspetta la `Risposta`, arriva un `Progresso`.
    let supervisore = SupervisoreInAttesa::nuovo(attese()).expect("coerenti");
    let errore = supervisore
        .ricevi(Frame::nuovo(Corpo::Progresso(Progresso {
            righe: 0,
            batch: 0,
            nodi_completati: 0,
        })))
        .expect_err("fuori sequenza");
    assert!(errore.to_string().contains("atteso"), "{errore}");

    // E dopo l'accordo, un `Annulla` al posto dell'`Incarico`.
    let (_, accordato) = giro_nominale();
    let errore = accordato
        .ricevi_incarico(Frame::nuovo(Corpo::Annulla(Annulla {
            motivo: "x".to_owned(),
        })))
        .expect_err("fuori sequenza");
    assert!(errore.to_string().contains("atteso"), "{errore}");
}

/// Il riuso di uno stato concluso **non compila**, e qui non c'e' un oracolo
/// che lo provi.
///
/// Ogni transizione consuma `self`, quindi un secondo `ricevi` sullo stesso
/// supervisore non e' un errore da gestire: e' codice che il compilatore
/// rifiuta. La garanzia sta nella firma — `fn ricevi(self, ..)` su un tipo
/// senza `Clone` — e il compilatore la fa rispettare a ogni chiamata del
/// crate.
///
/// **Un `compile_fail` qui sarebbe peggio di niente.** Un blocco
/// ```` ```compile_fail ```` su questa doc non verrebbe **mai eseguito**:
/// `rustdoc` non raccoglie i doctest dai moduli `#[cfg(test)]`, e `cargo test
/// --doc` risponde «0 tests». Raccolto sarebbe peggio ancora: `protocollo` e'
/// un modulo privato, quindi quel codice fallirebbe per **privacy**, non per
/// il valore mosso — un `compile_fail` che passa per la ragione sbagliata e'
/// un oracolo che dichiara una proprieta' senza sorvegliarla.
///
/// Provarla davvero richiede `trybuild` o `compiletest`, cioe' una dipendenza
/// nuova, e una superficie da cui il tipo sia raggiungibile. Fino ad allora e'
/// dichiarata e non provata, ed e' scritto qui.
///
/// Il test qui sotto prova un'altra cosa, vera e verificabile: due handshake
/// distinti non condividono stato.
#[test]
fn due_handshake_distinti_non_si_influenzano() {
    let (primo, _) = giro_nominale();
    let (secondo, _) = giro_nominale();
    assert_eq!(primo.commit_token(), secondo.commit_token());
}

/// La versione del protocollo non e' un asse da confrontare qui, ed e' giusto
/// cosi'.
///
/// Non c'e' un campo da impostare: il frame la deriva dalla costante e il
/// decoder rifiuta qualunque altra. Un confronto in questo modulo sarebbe una
/// guardia che non puo' fallire — cioe' una guardia che sembra proteggere e
/// non protegge. Quello che si puo' provare, e che qui si prova, e' che un
/// frame di versione diversa **non arriva mai** all'handshake.
#[test]
fn un_frame_di_versione_diversa_non_arriva_all_handshake() {
    use crate::protocollo::codifica::decodifica;

    let corpo = r#"{"motivo":"x"}"#;
    let testo = format!(r#"{{"protocol_version":2,"tipo":"annulla","corpo":{corpo}}}"#);
    let mut byte = u32::try_from(testo.len())
        .expect("corto")
        .to_be_bytes()
        .to_vec();
    byte.extend_from_slice(testo.as_bytes());

    let errore = decodifica(&byte).expect_err("versione ignota");
    assert!(errore.to_string().contains("non riconosciuta"), "{errore}");
}

// ---------------------------------------------------------------------------
// Riservatezza: nessun errore riporta cio' che e' arrivato dal filo
// ---------------------------------------------------------------------------

/// La sentinella: se compare in un messaggio d'errore, e' arrivata dal filo.
const SENTINELLA: &str = "SEGRETO-8675309124816324";

/// Un errore che cita il valore ricevuto e' quel valore in un log.
///
/// Il rifiuto e' il momento in cui qualcuno guarda, ed e' anche il momento in
/// cui cio' che si mostra viene copiato nel log insieme al motivo per cui lo
/// si stava guardando. Chi indaga ha il frame in mano: il messaggio deve dirgli
/// **quale campo** non torna, non che cosa c'e' scritto.
///
/// Ogni caso passa da un asse diverso del confronto, perche' la riservatezza
/// non e' una proprieta' del modulo ma di ciascun percorso d'errore: basta uno
/// che copi il valore.
#[test]
fn nessun_errore_dell_handshake_porta_cio_che_arriva_dal_filo() {
    /// La sentinella numerica, per i campi che non sono testo.
    const SENTINELLA_NUMERICA: u64 = 8_675_309_124_816_324;

    let mut casi: Vec<(&str, PlenoraError)> = Vec::new();

    // Versione dell'artefatto.
    let mut risposta = risposta_nominale();
    risposta.artefatto.versione = SENTINELLA.to_owned();
    casi.push((
        "versione artefatto",
        supervisore_riceve(risposta).expect_err("versione diversa"),
    ));

    // Identita' e versione del resolver.
    let mut risposta = risposta_nominale();
    risposta.resolver.identita = SENTINELLA.to_owned();
    casi.push((
        "identita' resolver",
        supervisore_riceve(risposta).expect_err("resolver diverso"),
    ));
    let mut risposta = risposta_nominale();
    risposta.resolver.versione = SENTINELLA.to_owned();
    casi.push((
        "versione resolver",
        supervisore_riceve(risposta).expect_err("versione diversa"),
    ));

    // Nome ripetuto fra le risorse: il nome lo sceglie chi ha scritto il frame.
    let mut risposta = risposta_nominale();
    let mut gemella = risorsa(SENTINELLA);
    gemella.versione = "9".to_owned();
    risposta.ambiente.risorse.push(risorsa(SENTINELLA));
    risposta.ambiente.risorse.push(gemella);
    casi.push((
        "risorsa duplicata",
        supervisore_riceve(risposta).expect_err("nome ripetuto"),
    ));

    // Capability offerta.
    let mut risposta = risposta_nominale();
    risposta.capability = vec![SENTINELLA.to_owned()];
    casi.push((
        "capability",
        supervisore_riceve(risposta).expect_err("capability mancante"),
    ));

    // Limiti dichiarati: qui la sentinella e' un numero.
    let mut saluto = saluto_nominale();
    saluto.limiti.max_frame_bytes = SENTINELLA_NUMERICA;
    casi.push(("limiti", worker_riceve(saluto).expect_err("limiti diversi")));

    let numerica = SENTINELLA_NUMERICA.to_string();
    for (asse, errore) in casi {
        for reso in [format!("{errore}"), format!("{errore:?}")] {
            assert!(
                !reso.contains(SENTINELLA) && !reso.contains(&numerica),
                "l'errore su «{asse}» ha copiato il valore ricevuto: {reso}"
            );
        }
    }
}

/// Due descrizioni **vuote** si accorderebbero, e non e' un accordo.
///
/// L'handshake confronta per uguaglianza, e la stringa vuota e' uguale alla
/// stringa vuota: due lati che non dichiarano nulla concludono l'accordo
/// avendo confrontato il nulla col nulla. Il tetto non lo impedisce — una
/// stringa vuota sta sotto qualunque tetto — quindi serve una regola che
/// dica non solo «quanto grande», ma «che ci sia».
///
/// Si prova su **entrambi i lati**: il rifiuto della propria descrizione
/// arriva alla costruzione, quello della descrizione ricevuta alla verifica.
#[test]
fn una_descrizione_vuota_non_si_costruisce_ne_si_accetta() {
    // Il proprio lato, prima di spedire.
    let mut sue = attese();
    sue.comune.resolver.identita = String::new();
    let errore = SupervisoreInAttesa::nuovo(sue).expect_err("identita' vuota");
    assert!(errore.to_string().contains("resolver.identita"), "{errore}");

    let mut sua = locale();
    sua.comune.artefatto.versione = String::new();
    let errore = WorkerInAttesa::nuovo(sua).expect_err("versione vuota");
    assert!(
        errore.to_string().contains("artefatto.versione"),
        "{errore}"
    );

    // Il lato ricevuto, alla verifica.
    let mut risposta = risposta_nominale();
    risposta.resolver.versione = String::new();
    let errore = supervisore_riceve(risposta).expect_err("versione vuota");
    assert!(errore.to_string().contains("resolver.versione"), "{errore}");

    // E il caso che la sua assenza renderebbe possibile: due lati che
    // dichiarano il vuoto sullo stesso asse **coinciderebbero**.
    let mut sue = attese();
    sue.comune.resolver.identita = String::new();
    let mut sua = locale();
    sua.comune.resolver.identita = String::new();
    assert!(
        SupervisoreInAttesa::nuovo(sue).is_err() && WorkerInAttesa::nuovo(sua).is_err(),
        "due descrizioni vuote sono arrivate al confronto, dove si sarebbero accordate"
    );
}

/// Un digest non canonico **non si costruisce**, quindi non arriva qui.
///
/// Questo test non prova piu' un rifiuto: prova che il rifiuto non serve. I
/// quattro digest del filo sono `DigestSha256`, e non esiste un valore di quel
/// tipo che non sia canonico — le forme rifiutate stanno nelle prove del tipo,
/// dove sono esercitate una per una.
///
/// Cio' che resta da provare **qui** e' l'altra meta': che due digest canonici
/// e diversi siano un disaccordo, e che il disaccordo sia dell'asse giusto.
#[test]
fn due_digest_canonici_e_diversi_sono_un_disaccordo() {
    let mut risposta = risposta_nominale();
    risposta.ambiente.digest_insieme = digest(&"c".repeat(64));
    let errore = supervisore_riceve(risposta).expect_err("insieme diverso");
    assert_eq!(
        errore.category(),
        ErrorCategory::InvalidConfiguration,
        "{errore}"
    );
    assert!(
        errore.to_string().contains("digest dell'insieme"),
        "{errore}"
    );
}

/// Attese che **nessuna** risposta valida potrebbe soddisfare.
///
/// Facendo passare le capability richieste dalla sola riduzione a forma
/// canonica — ordine e duplicati — e non dalla verifica che vale per quelle
/// offerte, il supervisore potrebbe chiedere un nome vuoto, che nessuna
/// `Risposta` valida puo' portare, o piu' capability di quante una `Risposta`
/// ne ammetta: in entrambi i casi l'accordo sarebbe impossibile per
/// costruzione, e lo si scoprirebbe al confronto invece che alla costruzione.
#[test]
fn le_capability_richieste_passano_dalla_stessa_verifica_delle_offerte() {
    use crate::protocollo::limiti::MAX_CAPABILITY;

    let mut sue = attese();
    sue.capability_richieste = vec![String::new()];
    let errore = SupervisoreInAttesa::nuovo(sue).expect_err("nome vuoto");
    assert!(errore.to_string().contains("capability"), "{errore}");

    let mut sue = attese();
    sue.capability_richieste = (0..=MAX_CAPABILITY).map(|i| format!("c{i}")).collect();
    let errore = SupervisoreInAttesa::nuovo(sue).expect_err("cardinalita' eccessiva");
    assert!(
        errore.to_string().contains("capability") && errore.to_string().contains("oltre il tetto"),
        "{errore}"
    );
}

/// Un `Incarico` malformato non passa perche' arriva da un `Frame` diretto.
///
/// Il frame consegnato all'accordato puo' non essere passato dal decoder: nel
/// crate si costruisce con `Frame::nuovo`. Senza una verifica qui, un incarico
/// con un `plan_hash_atteso` che non e' un digest uscirebbe intatto, e a
/// rifiutarlo sarebbe chi lo esegue — cioe' piu' tardi, e altrove.
#[test]
fn un_incarico_malformato_e_rifiutato_anche_da_un_frame_diretto() {
    // I guasti sui digest non compaiono, e non per dimenticanza: dopo che i
    // quattro digest sono un tipo, `plan_hash_atteso = "dd"` non si scrive
    // piu'. Restano i campi che una `String` puo' ancora sbagliare.
    for guasto in [
        |i: &mut Incarico| i.artefatto_temporaneo = String::new(),
        |i: &mut Incarico| i.ingressi[0].nome = "x".repeat(300),
        |i: &mut Incarico| i.ingressi[0].nome = String::new(),
        |i: &mut Incarico| i.ingressi[0].percorso = String::new(),
    ] {
        let mut frame = incarico();
        if let Corpo::Incarico(dentro) = frame.corpo_mutabile() {
            guasto(dentro);
        }
        let (_, accordato) = giro_nominale();
        let errore = accordato
            .ricevi_incarico(frame)
            .expect_err("incarico malformato accettato da un frame diretto");
        assert_eq!(errore.category(), ErrorCategory::Protocol, "{errore}");
    }
}

// ---------------------------------------------------------------------------
// La forma canonica e' anche quella spedita
// ---------------------------------------------------------------------------

/// Due descrizioni **logicamente uguali** producono lo stesso frame.
///
/// Risorse, backend e capability sono insiemi: elencarli in ordine diverso
/// descrive lo stesso ambiente. Se il frame spedito conservasse l'ordine
/// d'origine, due supervisori d'accordo emetterebbero byte diversi, e il
/// determinismo del filo dipenderebbe dall'ordine in cui qualcuno ha riempito
/// un `Vec`.
///
/// L'oracolo sono i **byte**, non la struttura: confrontare due `Saluto` con
/// `==` proverebbe che i campi coincidono, non che coincida cio' che viaggia.
#[test]
fn due_descrizioni_equivalenti_producono_lo_stesso_saluto() {
    let dritto = SupervisoreInAttesa::nuovo(attese()).expect("coerenti");

    let mut al_contrario = attese();
    al_contrario.comune.ambiente.risorse.reverse();
    al_contrario.comune.ambiente.backend_dinamici.reverse();
    al_contrario.capability_richieste.reverse();
    let rovescio = SupervisoreInAttesa::nuovo(al_contrario).expect("coerenti");

    let byte = |s: &SupervisoreInAttesa| {
        crate::protocollo::codifica::codifica(&Frame::nuovo(Corpo::Saluto(Box::new(
            s.saluto().clone(),
        ))))
        .expect("il saluto si codifica")
    };
    assert_eq!(
        byte(&dritto),
        byte(&rovescio),
        "due descrizioni equivalenti hanno prodotto frame diversi"
    );
}

/// Lo stesso per la `Risposta`, che il worker costruisce.
///
/// Non e' lo stesso test scritto due volte: i due frame li costruiscono due
/// percorsi diversi — il supervisore alla costruzione, il worker alla
/// ricezione — e una sola delle due forme canonicalizzate direbbe che il giro
/// funziona solo in un verso.
#[test]
fn due_worker_equivalenti_producono_la_stessa_risposta() {
    let byte = |mut descrizione: DescrizioneLocale, rovescia: bool| {
        if rovescia {
            descrizione.comune.ambiente.risorse.reverse();
            descrizione.comune.ambiente.backend_dinamici.reverse();
            descrizione.capability.reverse();
        }
        let worker = WorkerInAttesa::nuovo(descrizione).expect("coerente");
        let saluto = Frame::nuovo(Corpo::Saluto(Box::new(saluto_nominale())));
        let (risposta, _) = worker.ricevi(saluto).expect("accordo");
        crate::protocollo::codifica::codifica(&Frame::nuovo(Corpo::Risposta(Box::new(risposta))))
            .expect("la risposta si codifica")
    };
    assert_eq!(
        byte(locale(), false),
        byte(locale(), true),
        "due worker equivalenti hanno prodotto risposte diverse"
    );
}

/// La riduzione avviene **una volta**, e cio' che si confronta e' gia' ridotto.
///
/// Si osserva dall'esterno cosi': l'ordine d'origine non sopravvive in nessuno
/// dei due lati, quindi un `Saluto` costruito da un ordine e una `Risposta`
/// costruita dall'altro si accordano — e il giro nominale, che parte da
/// descrizioni gia' ordinate, resta identico.
#[test]
fn i_due_lati_si_accordano_qualunque_sia_l_ordine_d_origine() {
    let mut sue = attese();
    sue.comune.ambiente.risorse.reverse();
    let supervisore = SupervisoreInAttesa::nuovo(sue).expect("coerenti");
    let saluto = Frame::nuovo(Corpo::Saluto(Box::new(supervisore.saluto().clone())));

    let mut sua = locale();
    sua.comune.ambiente.backend_dinamici.reverse();
    sua.capability.reverse();
    let worker = WorkerInAttesa::nuovo(sua).expect("coerente");
    let (risposta, _) = worker.ricevi(saluto).expect("il worker accetta");

    supervisore
        .ricevi(Frame::nuovo(Corpo::Risposta(Box::new(risposta))))
        .expect("il supervisore accetta");
}

/// Cio' che si spedisce e' **ordinato**, e lo si dice senza confrontarlo con
/// se stesso.
///
/// I due oracoli qui sopra confrontano due esiti dello **stesso** percorso:
/// provano che l'ordine d'origine non conta, e infatti una deviazione
/// applicata a entrambi i lati sfugge a tutti e due — verificato iniettandola.
/// Questo test dice invece una proprieta' assoluta del frame emesso, che non
/// ha bisogno di un secondo frame per essere vera.
#[test]
fn cio_che_si_spedisce_porta_gli_insiemi_ordinati() {
    let mut sue = attese();
    sue.comune.ambiente.risorse.reverse();
    sue.comune.ambiente.backend_dinamici.reverse();
    let supervisore = SupervisoreInAttesa::nuovo(sue).expect("coerenti");
    let ambiente = &supervisore.saluto().ambiente;
    assert!(
        ambiente.risorse.is_sorted_by_key(|r| &r.nome),
        "il `Saluto` porta risorse non ordinate"
    );
    assert!(
        ambiente.backend_dinamici.is_sorted_by_key(|b| &b.nome),
        "il `Saluto` porta backend non ordinati"
    );

    let mut sua = locale();
    sua.comune.ambiente.risorse.reverse();
    sua.capability.reverse();
    let worker = WorkerInAttesa::nuovo(sua).expect("coerente");
    let (risposta, _) = worker
        .ricevi(Frame::nuovo(Corpo::Saluto(Box::new(saluto_nominale()))))
        .expect("accordo");
    assert!(
        risposta.ambiente.risorse.is_sorted_by_key(|r| &r.nome),
        "la `Risposta` porta risorse non ordinate"
    );
    assert!(
        risposta.capability.is_sorted(),
        "la `Risposta` porta capability non ordinate"
    );
}
