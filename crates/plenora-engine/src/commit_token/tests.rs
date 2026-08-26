//! Prove del `commit_token`.
//!
//! Due gruppi che non si coprono a vicenda: la **forma** (cosa si accetta e
//! cosa no) e la **riservatezza** (dove il valore non deve comparire). Il
//! secondo non discende dal primo: un tipo puo' validare benissimo e poi
//! stampare il valore in ogni messaggio.

use super::{
    CommitToken, FormaTokenNonValida, CHIAVE_FOOTER_COMMIT_TOKEN, COMMIT_TOKEN_BYTES,
    COMMIT_TOKEN_CARATTERI,
};

/// Un token valido, scritto a mano.
const CANONICO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Un secondo token valido, diverso dal primo in un solo carattere.
const ALTRO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde0";

/// La sentinella: se compare da qualche parte, il valore e' trapelato.
const SENTINELLA: &str = "deadbeefcafebabe1234567890abcdef00112233445566778899aabbccddeeff";

#[test]
fn la_forma_canonica_si_accetta_e_torna_identica() {
    let token = CommitToken::da_esadecimale(CANONICO).expect("canonico");
    assert_eq!(token.in_esadecimale(), CANONICO);
}

/// La forma canonica si ricostruisce dai byte, non si conserva dal testo.
///
/// Sono due cose diverse: conservando il testo, un token costruito da una
/// grafia qualsiasi la renderebbe tale e quale, e il footer riceverebbe
/// qualcosa che non e' canonico.
#[test]
fn due_token_uguali_rendono_lo_stesso_testo() {
    let uno = CommitToken::da_esadecimale(CANONICO).expect("canonico");
    let due = CommitToken::da_esadecimale(CANONICO).expect("canonico");
    assert_eq!(uno, due);
    assert_eq!(uno.in_esadecimale(), due.in_esadecimale());

    let diverso = CommitToken::da_esadecimale(ALTRO).expect("canonico");
    assert_ne!(uno, diverso);
    assert_ne!(uno.in_esadecimale(), diverso.in_esadecimale());
}

#[test]
fn la_lunghezza_sbagliata_e_un_errore() {
    for testo in [
        "",
        "0",
        &CANONICO[..COMMIT_TOKEN_CARATTERI - 1],
        &format!("{CANONICO}0"),
        &CANONICO.repeat(2),
    ] {
        let errore = CommitToken::da_esadecimale(testo).expect_err("lunghezza sbagliata");
        assert!(
            matches!(errore, FormaTokenNonValida::LunghezzaErrata { .. }),
            "motivo inatteso per {} byte: {errore:?}",
            testo.len()
        );
    }
}

/// La lunghezza riportata e' quella **che ha deciso**: byte, non caratteri.
///
/// Il numero nel messaggio veniva da una seconda misura, presa in caratteri
/// perche' sembrava piu' leggibile. Su un testo di 64 caratteri con due
/// accentate il rifiuto era giusto e la spiegazione diceva «attesi 64, trovati
/// 64»: un errore che si smentisce da solo, e che manda chi legge a cercare il
/// difetto altrove. Sulla forma canonica, che e' ASCII, le due misure
/// coincidono; divergono esattamente nel caso in cui il messaggio serve.
#[test]
fn la_lunghezza_riportata_e_quella_che_ha_deciso() {
    let accentato = format!("{}èè", &CANONICO[..62]);
    assert_eq!(accentato.chars().count(), COMMIT_TOKEN_CARATTERI);
    assert_eq!(accentato.len(), COMMIT_TOKEN_CARATTERI + 2);
    let errore = CommitToken::da_esadecimale(&accentato).expect_err("lunghezza sbagliata");
    assert_eq!(
        errore,
        FormaTokenNonValida::LunghezzaErrata {
            attesi: COMMIT_TOKEN_CARATTERI,
            trovati: COMMIT_TOKEN_CARATTERI + 2,
        }
    );
    // Cio' che il difetto produceva: un rifiuto che nomina due volte lo stesso
    // numero. Che non accada e' la proprieta', non un dettaglio del testo.
    let detto = errore.to_string();
    assert!(
        !detto.contains("64 byte, ne servono esattamente 64"),
        "il rifiuto si smentisce da solo: {detto}"
    );
}

#[test]
fn un_carattere_non_esadecimale_e_un_errore() {
    let mut testo: Vec<u8> = CANONICO.as_bytes().to_vec();
    testo[7] = b'g';
    let testo = String::from_utf8(testo).expect("ASCII");
    assert_eq!(
        CommitToken::da_esadecimale(&testo).expect_err("non esadecimale"),
        FormaTokenNonValida::NonEsadecimale { posizione: 7 }
    );
}

/// La maiuscola e' un errore **suo**, non spazzatura.
///
/// `A` non e' un carattere qualsiasi: e' la grafia sbagliata di un valore che
/// potrebbe essere giusto, e dirlo cambia cosa fa chi legge il rifiuto.
#[test]
fn una_maiuscola_e_un_errore_distinto_e_non_si_normalizza() {
    let maiuscolo = CANONICO.to_uppercase();
    assert_eq!(maiuscolo.len(), COMMIT_TOKEN_CARATTERI);
    let errore = CommitToken::da_esadecimale(&maiuscolo).expect_err("maiuscolo");
    assert_eq!(
        errore,
        FormaTokenNonValida::MaiuscoloNonAmmesso { posizione: 10 },
        "la prima maiuscola di «{maiuscolo}» e' in posizione 10"
    );

    // E non si normalizza: nessuna forma maiuscola produce un token.
    for posizione in 0..COMMIT_TOKEN_CARATTERI {
        let mut byte = CANONICO.as_bytes().to_vec();
        byte[posizione] = byte[posizione].to_ascii_uppercase();
        let testo = String::from_utf8(byte).expect("ASCII");
        if testo == CANONICO {
            continue;
        }
        assert!(
            CommitToken::da_esadecimale(&testo).is_err(),
            "accettata una maiuscola in posizione {posizione}"
        );
    }
}

#[test]
fn il_giro_serde_conserva_la_forma_canonica() {
    let token = CommitToken::da_esadecimale(CANONICO).expect("canonico");
    let reso = serde_json::to_string(&token).expect("serializzabile");
    assert_eq!(reso, format!("\"{CANONICO}\""));
    let riletto: CommitToken = serde_json::from_str(&reso).expect("rileggibile");
    assert_eq!(riletto, token);
}

/// Un token canonico si legge **comunque arrivi**, non solo da un buffer in
/// memoria.
///
/// Sono i due modi ordinari di non poter prestare una stringa, e prima
/// rifiutavano un token perfettamente canonico con «expected a borrowed
/// string»: un rifiuto per la forma del trasporto travestito da rifiuto del
/// token. Il token viaggia nel `Saluto`, che il **worker** legge da un pipe,
/// quindi il primo caso non e' un'ipotesi di laboratorio: e' il percorso vero.
#[test]
fn un_token_canonico_si_legge_anche_da_sorgenti_non_prestabili() {
    let atteso = CommitToken::da_esadecimale(CANONICO).expect("canonico");

    // 1. Una sorgente che scorre: non c'e' un buffer da cui prestare.
    let json = format!("\"{CANONICO}\"");
    let letto: CommitToken =
        serde_json::from_reader(std::io::Cursor::new(json.as_bytes())).expect("da un lettore");
    assert_eq!(letto, atteso);

    // 2. Una stringa JSON con escape: disfarli produce un testo posseduto.
    //    `\u0030` e' `0`, cioe' il primo carattere di CANONICO — la stringa
    //    descrive lo **stesso** token, in una grafia JSON diversa.
    let con_escape = format!("\"\\u0030{}\"", &CANONICO[1..]);
    let letto: CommitToken = serde_json::from_str(&con_escape).expect("con escape");
    assert_eq!(letto, atteso);
}

/// Anche in lettura la forma non canonica si **rifiuta**, non si aggiusta.
#[test]
fn serde_rifiuta_le_forme_non_canoniche() {
    for testo in [
        CANONICO.to_uppercase(),
        CANONICO[..63].to_owned(),
        format!("{CANONICO}0"),
        "non un token".to_owned(),
        String::new(),
    ] {
        let json = serde_json::to_string(&testo).expect("stringa serializzabile");
        assert!(
            serde_json::from_str::<CommitToken>(&json).is_err(),
            "accettata la forma non canonica «{testo}»"
        );
    }

    // E i tipi sbagliati: un numero non e' un token.
    for json in ["1234", "null", "[]", "{}", "true"] {
        assert!(
            serde_json::from_str::<CommitToken>(json).is_err(),
            "accettato `{json}` come token"
        );
    }
}

/// Nemmeno un valore **non testuale** finisce nell'errore.
///
/// `visit_str` chiude la porta delle stringhe e lascia aperte le altre: i
/// default del `Visitor` costruiscono `Unexpected` **dal valore ricevuto**, e
/// `Bool`, `Signed`/`Unsigned` e `Float` lo stampano. Un token scritto per
/// sbaglio senza virgolette e' un numero, e usciva in chiaro: «invalid type:
/// integer `8675309124816324`».
///
/// Le sentinelle qui sono cifre che non compaiono altrove nel file: se una si
/// affaccia nel messaggio, e' arrivata dal valore.
#[test]
fn nessun_errore_porta_il_valore_nemmeno_quando_non_e_una_stringa() {
    const RIFIUTO: &str = "non un valore di un altro tipo";
    let casi = [
        ("8675309124816324", "8675309124816324"),
        ("-8675309124816324", "8675309124816324"),
        ("8675309.124816324", "8675309"),
        ("true", "true"),
        ("false", "false"),
    ];
    for (json, sentinella) in casi {
        let errore = serde_json::from_str::<CommitToken>(json).expect_err("non e' un token");
        let reso = errore.to_string();
        assert!(
            !reso.contains(sentinella),
            "l'errore ha copiato il valore ricevuto: {reso}"
        );
        // E non per caso: il messaggio e' **quello costante**, uguale per tutti.
        assert!(
            reso.contains(RIFIUTO),
            "rifiuto inatteso per `{json}`: {reso}"
        );
    }

    // Gli altri tipi non scalari non portano il valore nemmeno per default —
    // «null», «sequence», «map» — ma passano dal rifiuto di `serde`, non dal
    // nostro: qui si prova che restano un errore, non che siano nostri.
    for json in ["null", "[]", "{}"] {
        assert!(
            serde_json::from_str::<CommitToken>(json).is_err(),
            "accettato `{json}` come token"
        );
    }
}

/// I due interi a 128 bit, che dal giro JSON non si raggiungono.
///
/// `serde_json` non li produce — un numero diventa `i64`, `u64` o `f64` — e
/// quindi il test qui sopra non li tocca. Il modo di esercitarli senza
/// inventare un formato e' chiamare il visitatore, che sta in questo crate:
/// meglio due righe di test che due metodi mai eseguiti a guardia di una
/// proprieta' di riservatezza.
///
/// Oggi il default di `serde` per questi due direbbe «i128» e non il valore,
/// quindi il rifiuto sarebbe riservato comunque. Il punto e' che sarebbe
/// riservato **per come una dipendenza formatta un messaggio**, e non per una
/// scelta nostra.
#[test]
fn anche_gli_interi_a_128_bit_rifiutano_senza_portare_il_valore() {
    use serde::de::Visitor;

    const SENTINELLA: i128 = 8_675_309_124_816_324;
    let attesa = SENTINELLA.to_string();

    let errore = super::VisitatoreToken
        .visit_i128::<serde_json::Error>(SENTINELLA)
        .expect_err("un intero non e' un token");
    assert!(
        !errore.to_string().contains(&attesa),
        "l'errore ha copiato il valore: {errore}"
    );

    let errore = super::VisitatoreToken
        .visit_u128::<serde_json::Error>(SENTINELLA.unsigned_abs())
        .expect_err("un intero non e' un token");
    assert!(
        !errore.to_string().contains(&attesa),
        "l'errore ha copiato il valore: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Riservatezza: dove il valore NON deve comparire
// ---------------------------------------------------------------------------

/// `Debug` e' cio' che finisce in un log per sbaglio.
#[test]
fn debug_e_display_non_mostrano_il_valore() {
    let token = CommitToken::da_esadecimale(SENTINELLA).expect("canonico");

    let reso = format!("{token:?}");
    assert!(!reso.contains(SENTINELLA), "`Debug` ha mostrato il valore");
    assert!(
        !reso.contains(&SENTINELLA[..8]),
        "`Debug` ha mostrato un frammento"
    );

    let mostrato = format!("{token}");
    assert!(
        !mostrato.contains(SENTINELLA),
        "`Display` ha mostrato il valore"
    );
    assert!(
        !mostrato.contains(&SENTINELLA[..8]),
        "`Display` ha mostrato un frammento"
    );

    // E dentro una struttura piu' grande, che e' il caso reale.
    let dentro = format!("{:?}", (1_u8, token, "coda"));
    assert!(!dentro.contains(SENTINELLA));
}

/// Nessun errore di questo modulo porta il valore.
///
/// Il rifiuto e' proprio il momento in cui il token e' sospetto e qualcuno
/// vorrebbe vederlo per capire: e' anche il momento in cui, se lo mostri,
/// finisce in un log insieme al motivo per cui qualcuno lo stava guardando.
#[test]
fn nessun_errore_porta_il_valore() {
    let sospetti = [
        SENTINELLA.to_uppercase(),
        SENTINELLA[..63].to_owned(),
        format!("{SENTINELLA}0"),
        format!("{}g{}", &SENTINELLA[..10], &SENTINELLA[11..]),
    ];
    for testo in sospetti {
        let errore = CommitToken::da_esadecimale(&testo).expect_err("non canonico");
        for reso in [format!("{errore}"), format!("{errore:?}")] {
            assert!(
                !reso.contains(SENTINELLA) && !reso.contains(&SENTINELLA[..8]),
                "l'errore ha copiato il valore: {reso}"
            );
            assert!(
                !reso.to_lowercase().contains(&SENTINELLA[..8]),
                "l'errore ha copiato il valore in altra grafia: {reso}"
            );
        }

        // E l'errore di `serde`, che nasce da `custom` e non dal testo.
        let json = serde_json::to_string(&testo).expect("stringa");
        let errore = serde_json::from_str::<CommitToken>(&json).expect_err("non canonico");
        let reso = errore.to_string();
        assert!(
            !reso.to_lowercase().contains(&SENTINELLA[..8]),
            "l'errore di serde ha copiato il valore: {reso}"
        );
    }
}

#[test]
fn la_chiave_del_footer_e_una_sola_e_dichiarata() {
    assert_eq!(CHIAVE_FOOTER_COMMIT_TOKEN, "plenora.commit.token");
    // I byte del token e i caratteri della forma canonica stanno in relazione
    // fissa: due caratteri per byte.
    assert_eq!(COMMIT_TOKEN_CARATTERI, COMMIT_TOKEN_BYTES * 2);
    assert_eq!(CANONICO.len(), COMMIT_TOKEN_CARATTERI);
}
