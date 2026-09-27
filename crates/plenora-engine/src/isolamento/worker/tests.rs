//! I casi del worker: la matrice dei rifiuti, e cio' che passa.
//!
//! Girano ovunque e senza processi: cio' che si prova qui e' il **giudizio** sul
//! valore che attraversa il confine, non l'ambiente in cui arriva. L'ambiente e'
//! globale al processo, e casi che lo scrivessero misurerebbero l'ordine in cui
//! il runner li ha lanciati invece della regola.

use crate::protocollo::limiti::MAX_PROGRESSO;
use crate::protocollo::messaggi::Progresso;

use super::{numeri_da, Meta, NumeriLetti, Rifiuto};

/// Il nome della variabile, per i rifiuti che la nominano.
///
/// I casi non lo prendono dal modulo del canale: quello e' di Linux, e questi
/// casi girano ovunque.
const QUALE: &str = "PLENORA_CANALE";

/// Il valore della variabile, come arriva davvero: bytes, non testo.
fn valore(testo: &str) -> std::ffi::OsString {
    std::ffi::OsString::from(testo)
}

/// La forma attesa passa, ed e' l'unica che passa.
///
/// Sta per prima perche' senza di essa la matrice dei rifiuti proverebbe
/// soltanto che la funzione rifiuta — cosa che farebbe anche se rifiutasse
/// tutto.
#[test]
fn la_forma_attesa_passa() {
    assert_eq!(
        numeri_da(Some(&valore("3:4"))).expect("«3:4» e' la forma attesa"),
        NumeriLetti {
            legge: 3,
            scrive: 4
        }
    );
    // E i numeri non sono limitati a una cifra: il descrittore e' un numero.
    assert_eq!(
        numeri_da(Some(&valore("17:1024"))).expect("due numeri qualunque"),
        NumeriLetti {
            legge: 17,
            scrive: 1024
        }
    );
}

/// **La matrice dei rifiuti**, con l'oracolo esatto.
///
/// Si confronta il tipo, non il messaggio, che interpola valori: a ogni forma
/// storta corrisponde **una** variante. La colonna della meta' conta: dire
/// «lettura» per un guasto nella scrittura manda al descrittore sbagliato.
#[test]
fn ogni_forma_storta_ha_la_sua_ragione() {
    let matrice: [(&str, Option<std::ffi::OsString>, Rifiuto); 20] = [
        ("assente", None, Rifiuto::Assente),
        ("vuota", Some(valore("")), Rifiuto::SenzaSeparatore),
        (
            "senza separatore",
            Some(valore("34")),
            Rifiuto::SenzaSeparatore,
        ),
        (
            "due separatori",
            Some(valore("3:4:5")),
            Rifiuto::TroppiSeparatori,
        ),
        (
            "meta' sinistra vuota",
            Some(valore(":4")),
            Rifiuto::MetaVuota(Meta::Lettura),
        ),
        (
            "meta' destra vuota",
            Some(valore("3:")),
            Rifiuto::MetaVuota(Meta::Scrittura),
        ),
        (
            "sinistra non numerica",
            Some(valore("tre:4")),
            Rifiuto::NonNumerica(Meta::Lettura),
        ),
        (
            "destra non numerica",
            Some(valore("3:quattro")),
            Rifiuto::NonNumerica(Meta::Scrittura),
        ),
        (
            "con spazio davanti",
            Some(valore(" 3:4")),
            Rifiuto::NonNumerica(Meta::Lettura),
        ),
        (
            "con segno",
            Some(valore("+3:4")),
            Rifiuto::NonCanonica(Meta::Lettura),
        ),
        (
            "con zero davanti",
            Some(valore("03:4")),
            Rifiuto::NonCanonica(Meta::Lettura),
        ),
        (
            "negativa",
            Some(valore("-1:4")),
            Rifiuto::NonUnDescrittore(Meta::Lettura),
        ),
        (
            "sui flussi standard",
            Some(valore("1:4")),
            Rifiuto::FlussoStandard(Meta::Lettura),
        ),
        (
            "destra sui flussi standard",
            Some(valore("3:0")),
            Rifiuto::FlussoStandard(Meta::Scrittura),
        ),
        (
            "troppo grande",
            Some(valore("99999999999:4")),
            Rifiuto::TroppoGrande(Meta::Lettura),
        ),
        // **Le forme composte**: scritte male *e* troppo grandi. La forma viene
        // prima, perche' e' la domanda piu' esterna — finche' non si sa se e'
        // scritto bene, che valore denoti non e' ancora una domanda sensata.
        (
            "troppo grande e col segno",
            Some(valore("+99999999999:4")),
            Rifiuto::NonCanonica(Meta::Lettura),
        ),
        (
            "troppo grande e con lo zero davanti",
            Some(valore("09999999999:4")),
            Rifiuto::NonCanonica(Meta::Lettura),
        ),
        (
            "troppo grande e non numerica",
            Some(valore("99999999999x:4")),
            Rifiuto::NonNumerica(Meta::Lettura),
        ),
        (
            "meno zero",
            Some(valore("-0:4")),
            Rifiuto::NonCanonica(Meta::Lettura),
        ),
        (
            "solo il segno",
            Some(valore("-:4")),
            Rifiuto::NonNumerica(Meta::Lettura),
        ),
    ];

    for (nome, dato, atteso) in matrice {
        let avuto = numeri_da(dato.as_deref())
            .expect_err(&format!("«{nome}» non e' la forma attesa, e va rifiutata"));
        assert_eq!(avuto, atteso, "«{nome}»: la ragione non e' quella");
    }
}

/// **Ogni ragione dice una cosa diversa.**
///
/// Il tipo garantisce ragioni distinte; il caso garantisce che restino
/// distinte **una volta dette**, meta' comprese, perche' chi legge un log
/// possa separarle.
#[test]
fn ragioni_distinte_danno_messaggi_distinti() {
    let tutte = [
        Rifiuto::Assente,
        Rifiuto::NonTesto,
        Rifiuto::SenzaSeparatore,
        Rifiuto::TroppiSeparatori,
        Rifiuto::MetaVuota(Meta::Lettura),
        Rifiuto::MetaVuota(Meta::Scrittura),
        Rifiuto::NonNumerica(Meta::Lettura),
        Rifiuto::NonNumerica(Meta::Scrittura),
        Rifiuto::TroppoGrande(Meta::Lettura),
        Rifiuto::TroppoGrande(Meta::Scrittura),
        Rifiuto::NonCanonica(Meta::Lettura),
        Rifiuto::NonCanonica(Meta::Scrittura),
        Rifiuto::NonUnDescrittore(Meta::Lettura),
        Rifiuto::NonUnDescrittore(Meta::Scrittura),
        Rifiuto::FlussoStandard(Meta::Lettura),
        Rifiuto::FlussoStandard(Meta::Scrittura),
    ];

    let mut detti: Vec<String> = tutte.iter().map(|r| r.detto(QUALE).to_string()).collect();
    let quante = detti.len();
    detti.sort();
    detti.dedup();
    assert_eq!(
        detti.len(),
        quante,
        "due ragioni diverse dicono la stessa cosa: chi legge non puo' separarle"
    );
}

/// **La forma viene prima della grandezza.**
///
/// Un valore scritto male e troppo grande e' **scritto male**: e' la domanda
/// piu' esterna, e rispondere «troppo grande» manderebbe a cercare un valore
/// fuori scala dove c'e' un segno di troppo. Il caso lo fissa su entrambe le
/// forme composte, e su quella che non e' nemmeno un numero.
#[test]
fn la_forma_precede_la_grandezza() {
    for (testo, atteso) in [
        ("+99999999999:4", Rifiuto::NonCanonica(Meta::Lettura)),
        ("09999999999:4", Rifiuto::NonCanonica(Meta::Lettura)),
        ("-09999999999:4", Rifiuto::NonCanonica(Meta::Lettura)),
        ("99999999999x:4", Rifiuto::NonNumerica(Meta::Lettura)),
        ("3:+99999999999", Rifiuto::NonCanonica(Meta::Scrittura)),
    ] {
        assert_eq!(
            numeri_da(Some(&valore(testo))).expect_err("va rifiutato"),
            atteso,
            "«{testo}»: la forma va giudicata prima della grandezza"
        );
    }
}

/// **Il traboccamento non e' «non e' un numero».**
///
/// La forma e' quella giusta; la grandezza no. Sono due diagnosi che mandano a
/// guardare due cose diverse — un refuso, oppure un valore fuori scala — e il
/// ramo che le raccoglie e' lo stesso `Err` di `parse`: distinguerle richiede di
/// guardare il **genere** dell'errore, non solo la sua esistenza.
#[test]
fn il_traboccamento_e_una_ragione_sua() {
    for testo in [
        format!("{}0:4", i32::MAX),
        format!("{}0:4", i32::MIN),
        "99999999999999999999:4".to_owned(),
    ] {
        assert_eq!(
            numeri_da(Some(&valore(&testo))).expect_err("non entra in un i32"),
            Rifiuto::TroppoGrande(Meta::Lettura),
            "«{testo}» trabocca, e il rifiuto deve dirlo"
        );
    }
}

/// **Un negativo non e' un flusso standard.**
///
/// Zero, uno e due **sono** descrittori — solo non i nostri — mentre un
/// negativo non nomina niente. Dirli allo stesso modo direbbe il falso su una
/// delle due meta' dei casi, e manderebbe a cercare un terminale dove c'e' un
/// valore che non e' un descrittore.
#[test]
fn un_negativo_e_un_flusso_standard_non_sono_la_stessa_cosa() {
    let negativo = numeri_da(Some(&valore("-1:4"))).expect_err("non e' un descrittore");
    let standard = numeri_da(Some(&valore("2:4"))).expect_err("e' un flusso standard");
    assert_eq!(negativo, Rifiuto::NonUnDescrittore(Meta::Lettura));
    assert_eq!(standard, Rifiuto::FlussoStandard(Meta::Lettura));
    assert_ne!(
        negativo.detto(QUALE).to_string(),
        standard.detto(QUALE).to_string()
    );
}

/// **La variabile che non e' testo si rifiuta senza finire nel messaggio.**
///
/// Due cose insieme. Che si rifiuti: una variabile che non e' testo non e' una
/// variabile che il supervisore ha scritto. E che il **contenuto non compaia**:
/// e' un ingresso arbitrario, e ripeterlo porterebbe nei log byte che nessuno
/// ha scelto — ritorni a capo che spezzano una riga in due, o sequenze che un
/// terminale interpreta.
#[test]
#[cfg(unix)]
fn una_variabile_che_non_e_testo_si_rifiuta_senza_ripeterla() {
    use std::os::unix::ffi::OsStringExt as _;

    let velenoso = std::ffi::OsString::from_vec(vec![0x33, 0xff, 0x3a, 0x34]);
    assert_eq!(
        numeri_da(Some(&velenoso)).expect_err("non e' UTF-8"),
        Rifiuto::NonTesto
    );
    let detto = Rifiuto::NonTesto.detto(QUALE).to_string();
    assert!(
        !detto.contains('\u{fffd}') && !detto.contains('3'),
        "il contenuto non deve finire nel messaggio: {detto}"
    );
}

/// **Due estremi con lo stesso numero non sono due estremi.**
///
/// La lettura non lo rifiuta — sono due numeri ben formati — e non deve: e' una
/// domanda sulla **coppia**, e la coppia si giudica dove si usa. Il caso fissa
/// che la lettura li renda cosi' come sono, perche' il rifiuto arrivi dal posto
/// che sa che cosa sono.
#[test]
fn due_numeri_uguali_passano_la_lettura_e_cadono_sulla_coppia() {
    assert_eq!(
        numeri_da(Some(&valore("7:7"))).expect("«7:7» sono due numeri ben formati"),
        NumeriLetti {
            legge: 7,
            scrive: 7
        }
    );
}

/// **Le capability offerte sono quelle compilate, e nient'altro.**
///
/// Fissa la **derivazione** da `compiled_capabilities` e l'**assenza dei
/// profili di publish**, che stanno in `local_capabilities` perche' il worker
/// non pubblica. Il confronto e' per uguaglianza sull'insieme intero, cosi'
/// nessuna voce inventata passa.
#[test]
#[cfg(target_os = "linux")]
fn le_capability_offerte_sono_i_backend_compilati() {
    let offerte = super::capability_offerte();
    let compilate: Vec<String> = crate::planner::compiled_capabilities()
        .names()
        .map(str::to_owned)
        .collect();
    assert_eq!(offerte, compilate, "l'elenco non e' quello derivato");

    let locali = crate::planner::local_capabilities();
    for profilo in [
        crate::geo_transport::publish::PublishProfile::Atomic,
        crate::geo_transport::publish::PublishProfile::DurableAtomic,
    ] {
        let nome = profilo.capability_name();
        assert!(
            locali.contains(nome),
            "«{nome}» deve stare fra le capability locali, o il caso non prova niente"
        );
        assert!(
            !offerte.iter().any(|offerta| offerta == nome),
            "«{nome}» e' un profilo di publish: il worker non pubblica"
        );
    }

    assert!(
        offerte.iter().all(|nome| nome == "geos" || nome == "proj"),
        "il namespace dei backend e' geos/proj: {offerte:?}"
    );
    assert!(
        offerte.windows(2).all(|coppia| coppia[0] < coppia[1]),
        "l'handshake pretende nomi ordinati e distinti: {offerte:?}"
    );
}

/// **Esaurita la quota il progresso smette, il lavoro no.**
///
/// Oltre [`MAX_PROGRESSO`] **non parte piu' niente** (il supervisore lo
/// classificherebbe come violazione), e l'emissione oltre quota **non e' un
/// errore** (fermerebbe l'artefatto per un messaggio facoltativo). I giri
/// superano la quota vera: una quota finta proverebbe il contatore, non la
/// regola.
#[test]
fn oltre_la_quota_il_progresso_tace_e_il_lavoro_continua() {
    let mut quota = super::QuotaDiProgresso::nuova();
    let mut emessi = 0_usize;
    let giri = MAX_PROGRESSO + 37;

    for numero in 0..giri {
        let esito = quota.emetti(
            Progresso {
                righe: numero as u64,
                batch: numero as u64,
                nodi_completati: 0,
            },
            &mut |_| {
                emessi += 1;
                Ok(())
            },
        );
        assert!(
            esito.is_ok(),
            "al giro {numero} l'emissione ha reso un errore: fermerebbe il lavoro"
        );
    }

    assert_eq!(
        emessi, MAX_PROGRESSO,
        "sono partiti {emessi} progressi su una quota di {MAX_PROGRESSO}"
    );
}

/// L'errore di chi invia risale, perche' e' del canale e non della quota.
///
/// Distinguere i due casi conta: il silenzio oltre quota è previsto, un canale
/// rotto no — e confonderli farebbe proseguire un worker che non ha piu' modo
/// di dichiarare l'esito.
#[test]
fn l_errore_di_chi_invia_non_viene_ingoiato() {
    let mut quota = super::QuotaDiProgresso::con_quota(4);
    let esito = quota.emetti(
        Progresso {
            righe: 0,
            batch: 0,
            nodi_completati: 0,
        },
        &mut |_| {
            Err(plenora_core::PlenoraError::Internal(
                "canale rotto".to_owned(),
            ))
        },
    );
    assert!(
        esito.is_err(),
        "un canale rotto non e' un silenzio previsto"
    );
}

/// **La variabile del canale si scrive e si rilegge uguale.**
///
/// Chi scrive sta col tipo dei numeri, chi legge qui: una forma cambiata da
/// una parte sola non romperebbe la compilazione, ma il worker rifiuterebbe a
/// runtime. Copre anche il tre, primo descrittore non standard, che il parser
/// tratta a parte.
#[test]
#[cfg(target_os = "linux")]
fn la_variabile_del_canale_va_e_torna() {
    for (legge, scrive) in [(3, 4), (7, 11), (1_000, 1_001), (i32::MAX - 1, i32::MAX)] {
        let scritta = crate::isolamento::NumeriDelCanale { legge, scrive }.in_variabile();
        assert_eq!(
            numeri_da(Some(scritta.as_os_str())),
            Ok(NumeriLetti { legge, scrive }),
            "«{scritta:?}» non torna la coppia da cui e' nata"
        );
    }
}

/// **I due fatti non collassano mai l'uno sull'altro.**
///
/// Si prova ognuna delle quattro combinazioni. Conta di piu' l'ultima: quando
/// **nemmeno l'esito parte**, devono restare sia il canale che non ha retto
/// sia il messaggio che non ha potuto portare.
#[test]
#[cfg(target_os = "linux")]
fn i_due_fatti_sopravvivono_in_tutte_e_quattro_le_combinazioni() {
    use plenora_core::PlenoraError;

    let secondo = || PlenoraError::Protocol("il secondo fatto".to_owned());
    let invio = || PlenoraError::Internal("il canale non ha retto".to_owned());

    assert!(
        super::quel_che_resta(Ok(()), None).is_ok(),
        "un esito partito e nient'altro da dire non lascia niente indietro"
    );

    let Err(solo_secondo) = super::quel_che_resta(Ok(()), Some(secondo())) else {
        panic!("il secondo fatto deve risalire allo stato terminale");
    };
    assert!(solo_secondo.to_string().contains("il secondo fatto"));

    let Err(solo_invio) = super::quel_che_resta(Err(invio()), None) else {
        panic!("un esito che non parte e' un fallimento del worker");
    };
    assert!(solo_invio.to_string().contains("il canale non ha retto"));

    let Err(entrambi) = super::quel_che_resta(Err(invio()), Some(secondo())) else {
        panic!("due fatti non fanno un successo");
    };
    let detto = entrambi.to_string();
    assert!(
        detto.contains("il canale non ha retto") && detto.contains("il secondo fatto"),
        "nessuno dei due deve sparire: {detto}"
    );
}

/// **Il secondo fatto entra nel messaggio dell'errore, senza toccarne gli assi.**
///
/// Esclude che il guasto del canale di controllo sparisca, e che entri
/// sovrascrivendo categoria o fase, che descrivono il **primo** errore.
#[test]
#[cfg(target_os = "linux")]
fn il_secondo_fatto_entra_nel_messaggio_e_non_negli_assi() {
    use plenora_core::PlenoraError;

    let del_lavoro = PlenoraError::Schema("lo schema non regge".to_owned());
    let nudo = crate::protocollo::assi::errore_dichiarabile(&del_lavoro);
    let composto = super::con_anche(
        crate::protocollo::assi::errore_dichiarabile(&del_lavoro),
        Some(&PlenoraError::Protocol("il canale ha ceduto".to_owned())),
    );

    assert!(
        composto.messaggio.contains("lo schema non regge")
            && composto.messaggio.contains("il canale ha ceduto"),
        "nessuno dei due fatti deve sparire: {}",
        composto.messaggio
    );
    assert_eq!(
        composto.categoria, nudo.categoria,
        "gli assi restano del lavoro"
    );
    assert_eq!(composto.fase, nudo.fase);
    assert_eq!(composto.effetto, nudo.effetto);
    assert_eq!(composto.retry, nudo.retry);

    // Senza secondo fatto il messaggio non viene toccato: e' cio' che fissa che
    // il caso misuri la composizione e non `con_anche` in generale.
    let solo = super::con_anche(
        crate::protocollo::assi::errore_dichiarabile(&del_lavoro),
        None,
    );
    assert_eq!(solo.messaggio, nudo.messaggio);
}
