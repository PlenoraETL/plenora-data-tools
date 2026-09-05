//! I casi degli assi, e in particolare del ritardo che sul filo non entra.

use std::time::Duration;

use plenora_core::error::RetryDisposition;
use plenora_core::PlenoraError;

use super::super::messaggi::{CategoriaSulFilo, RetrySulFilo};
use super::{dichiarabile_dal_rifiuto, errore_dichiarabile, errore_sul_filo, ritentativo_sul_filo};

/// **Il ritardo massimo esatto passa, e passa intero.**
///
/// # Che cosa esclude
///
/// Che il controllo rifiuti un valore rappresentabile. Un confronto con `>=`
/// invece di `>`, o un tipo intermedio piu' stretto, toglierebbe il valore
/// all'estremo — e all'estremo non guarda quasi nessuno.
#[test]
fn il_ritardo_massimo_passa_intero() {
    // `PlenoraError` non e' confrontabile: non si confrontano due `Result`, si
    // guarda che l'esito sia riuscito **e** che porti il valore atteso.
    let al_limite = RetryDisposition::After(Duration::from_millis(u64::MAX));
    assert!(
        ritentativo_sul_filo(al_limite).is_ok_and(|resa| resa
            == RetrySulFilo::After {
                delay_ms: u64::MAX
            }),
        "il ritardo massimo rappresentabile deve passare, e passare intero"
    );
}

/// **Un millisecondo oltre il massimo viene rifiutato.**
///
/// # Che cosa esclude
///
/// La saturazione. Saturando, questo valore e quello del caso precedente
/// arriverebbero sul filo **identici**: due domini diversi, un messaggio solo, e
/// chi legge senza modo di sapere quale sia passato. E' la perdita che non lascia
/// traccia, cioe' quella che nessun controllo a valle puo' riprendere.
///
/// I due casi vanno letti insieme: il primo da solo passerebbe anche saturando,
/// il secondo da solo passerebbe anche rifiutando tutto.
#[test]
fn un_millisecondo_oltre_il_massimo_viene_rifiutato() {
    let oltre = RetryDisposition::After(Duration::from_millis(u64::MAX) + Duration::from_millis(1));
    match ritentativo_sul_filo(oltre) {
        Err(PlenoraError::Internal(detto)) => assert!(
            detto.contains("non entra nel filo"),
            "il rifiuto deve dire perche': «{detto}»"
        ),
        altro => panic!("oltre il massimo si rifiuta come Internal, non si satura: {altro:?}"),
    }
}

/// **Il massimo e l'oltre non arrivano sul filo con lo stesso valore.**
///
/// # Che cosa esclude
///
/// Che i due casi qui sopra restino veri mentre la proprieta' che li lega cade.
/// E' la proprieta' vera — due domini distinti restano distinti — e la si scrive
/// perche' un giorno qualcuno potrebbe «aggiustare» il rifiuto in un valore
/// sentinella, superando entrambi i casi precedenti e riaprendo la perdita.
#[test]
fn il_massimo_e_l_oltre_restano_distinguibili() {
    let massimo = ritentativo_sul_filo(RetryDisposition::After(Duration::from_millis(u64::MAX)));
    let oltre = ritentativo_sul_filo(RetryDisposition::After(
        Duration::from_millis(u64::MAX) + Duration::from_millis(1),
    ));
    assert_ne!(
        massimo.ok(),
        oltre.ok(),
        "il massimo e cio' che lo supera non possono avere lo stesso esito"
    );
}

/// **Le disposizioni senza ritardo passano tutte.**
///
/// # Che cosa esclude
///
/// Che rendere fallibile la conversione abbia reso fallibile anche cio' che non
/// puo' fallire: un rifiuto che scattasse sulle quattro varianti senza ritardo
/// zittirebbe il worker su ogni errore ordinario.
#[test]
fn le_disposizioni_senza_ritardo_non_rifiutano_mai() {
    for (nel_dominio, sul_filo) in [
        (RetryDisposition::Never, RetrySulFilo::Never {}),
        (RetryDisposition::Safe, RetrySulFilo::Safe {}),
        (
            RetryDisposition::RequiresIdempotencyKey,
            RetrySulFilo::RequiresIdempotencyKey {},
        ),
        (
            RetryDisposition::RequiresRecovery,
            RetrySulFilo::RequiresRecovery {},
        ),
    ] {
        assert!(ritentativo_sul_filo(nel_dominio).is_ok_and(|resa| resa == sul_filo));
    }
}

/// **Senza rifiuto, il dichiarabile e' la conversione e nient'altro.**
///
/// # Che cosa esclude
///
/// Che `errore_dichiarabile` dichiari sempre la guardia. Un rifiuto che scattasse
/// su tutto renderebbe verde il caso qui sotto senza distinguere niente, e ogni
/// errore del worker arriverebbe sul filo come `Internal`.
#[test]
fn un_errore_ordinario_conserva_i_propri_assi() {
    let ordinario = PlenoraError::Internal("un difetto qualunque".to_owned());
    assert_eq!(
        errore_sul_filo(&ordinario).ok(),
        Some(errore_dichiarabile(&ordinario)),
        "senza rifiuto il dichiarabile non aggiunge e non toglie niente"
    );
}

/// **La guardia porta entrambi i fatti, e dice i propri assi.**
///
/// # Che cosa esclude
///
/// Che il rifiuto zittisca il worker, o che lo faccia mentire. Il messaggio deve
/// nominare **sia** la ragione del rifiuto **sia** l'errore che si stava
/// riportando; categoria e ritentativo devono parlare di *questo* errore —
/// `Internal`, `Never` — e non di quello arrivato, che porta una categoria
/// diversa apposta.
///
/// # Perche' si chiama la composizione e non `errore_dichiarabile`
///
/// Perche' oggi nessun `PlenoraError` produce `After`, quindi il ramo della
/// guardia non si raggiunge da li'. Cio' che si puo' provare — e che conta — e'
/// **che cosa dice** la guardia quando tocca a lei, e quella e' una funzione con
/// due parametri.
#[test]
fn la_guardia_porta_il_rifiuto_e_l_originale() {
    let originale = PlenoraError::Protocol("il lavoro non e' andato".to_owned());
    let rifiuto = PlenoraError::Internal("il ritardo non entra nel filo".to_owned());
    assert_ne!(
        super::categoria_sul_filo(originale.category()),
        CategoriaSulFilo::Internal,
        "l'originale deve avere una categoria diversa, o il caso non distingue niente"
    );

    let sul_filo = dichiarabile_dal_rifiuto(&originale, &rifiuto);

    assert_eq!(sul_filo.categoria, CategoriaSulFilo::Internal);
    assert_eq!(sul_filo.retry, RetrySulFilo::Never {});
    assert!(
        sul_filo.messaggio.contains(&rifiuto.to_string()),
        "manca la ragione del rifiuto: «{}»",
        sul_filo.messaggio
    );
    assert!(
        sul_filo.messaggio.contains(&originale.to_string()),
        "manca l'errore che si stava riportando: «{}»",
        sul_filo.messaggio
    );
    assert_eq!(
        sul_filo.fase,
        super::fase_sul_filo(originale.phase()),
        "la fase e' un fatto osservato: il rifiuto non la tocca"
    );
}
