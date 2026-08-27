//! Prove del `commit_token`: **solo cio' che non e' la forma**.
//!
//! La conformita' di forma — accettazione, rifiuti, riservatezza del testo
//! rifiutato — sta nelle prove della rappresentazione condivisa, dove vive il
//! codice che la decide. Qui resta la **riservatezza del valore**, che e' la
//! proprieta' per cui questo tipo esiste separato dal digest: un valore che
//! l'altro capo del canale controlla non si copia nei log, perche' il log lo
//! conserva e lo diffonde insieme al motivo per cui qualcuno lo stava
//! guardando.

use super::{CommitToken, FormaTokenNonValida, CHIAVE_FOOTER_COMMIT_TOKEN};
use crate::esadecimale32::CARATTERI;

/// Un token valido, scritto a mano.
const CANONICO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Un secondo token valido, diverso dal primo in un solo carattere.
const ALTRO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde0";

/// La sentinella: se compare da qualche parte, il valore e' trapelato.
const SENTINELLA: &str = "deadbeefcafebabe1234567890abcdef00112233445566778899aabbccddeeff";

#[test]
fn due_token_diversi_restano_diversi() {
    let uno = CommitToken::da_esadecimale(CANONICO).expect("canonico");
    let diverso = CommitToken::da_esadecimale(ALTRO).expect("canonico");
    assert_ne!(uno, diverso);
    assert_ne!(uno.in_esadecimale(), diverso.in_esadecimale());
}

/// La maiuscola e' un errore **suo**, non spazzatura.
///
/// `A` non e' un carattere qualsiasi: e' la grafia sbagliata di un valore che
/// potrebbe essere giusto, e dirlo cambia cosa fa chi legge il rifiuto. E' la
/// distinzione che il digest invece riunisce, ed e' la ragione per cui il
/// difetto della rappresentazione condivisa la riporta separata.
#[test]
fn la_traduzione_del_difetto_distingue_la_maiuscola() {
    let maiuscolo = CANONICO.to_uppercase();
    assert_eq!(
        CommitToken::da_esadecimale(&maiuscolo),
        Err(FormaTokenNonValida::MaiuscoloNonAmmesso { posizione: 10 }),
        "la prima maiuscola di «{maiuscolo}» e' in posizione 10"
    );

    let mut byte = CANONICO.as_bytes().to_vec();
    byte[7] = b'g';
    let fuori = String::from_utf8(byte).expect("ASCII");
    assert_eq!(
        CommitToken::da_esadecimale(&fuori),
        Err(FormaTokenNonValida::NonEsadecimale { posizione: 7 })
    );

    assert_eq!(
        CommitToken::da_esadecimale("0"),
        Err(FormaTokenNonValida::LunghezzaErrata {
            attesi: CARATTERI,
            trovati: 1,
        })
    );
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
/// Sono i due modi ordinari di non poter prestare una stringa, e con una
/// lettura che pretende il prestito un token perfettamente canonico verrebbe
/// rifiutato con «expected a borrowed string»: un rifiuto per la forma del
/// **trasporto**, indistinguibile da un rifiuto del token. Il token viaggia
/// nel `Saluto`, che il worker legge da un pipe, quindi il primo caso e' il
/// percorso vero.
#[test]
fn un_token_canonico_si_legge_anche_da_sorgenti_non_prestabili() {
    let atteso = CommitToken::da_esadecimale(CANONICO).expect("canonico");

    let json = format!("\"{CANONICO}\"");
    let letto: CommitToken =
        serde_json::from_reader(std::io::Cursor::new(json.as_bytes())).expect("da un lettore");
    assert_eq!(letto, atteso);

    // `\u0030` e' `0`, cioe' il primo carattere di CANONICO: la stringa
    // descrive lo **stesso** token in una grafia JSON diversa, e disfare
    // l'escape produce un testo posseduto.
    let con_escape = format!("\"\\u0030{}\"", &CANONICO[1..]);
    let letto: CommitToken = serde_json::from_str(&con_escape).expect("con escape");
    assert_eq!(letto, atteso);
}

// ---------------------------------------------------------------------------
// Riservatezza: dove il valore NON deve comparire
// ---------------------------------------------------------------------------

/// `Debug` e' cio' che finisce in un log per sbaglio.
///
/// E' la politica **opposta** a quella del digest, ed e' il motivo per cui la
/// rappresentazione condivisa non ha ne' `Debug` ne' `Display`: averli la'
/// avrebbe imposto una scelta sola a due tipi che ne vogliono due.
#[test]
fn debug_e_display_non_mostrano_il_valore() {
    let token = CommitToken::da_esadecimale(SENTINELLA).expect("canonico");
    for reso in [format!("{token:?}"), format!("{token}")] {
        assert!(
            !reso.contains(SENTINELLA) && !reso.contains(&SENTINELLA[..8]),
            "il valore e' trapelato: {reso}"
        );
    }
    // E la via che lo mostra esiste, esplicita e leggibile in review.
    assert_eq!(token.in_esadecimale(), SENTINELLA);
}

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
                !reso.to_lowercase().contains(&SENTINELLA[..8]),
                "l'errore ha copiato il valore: {reso}"
            );
        }

        // E l'errore di `serde`, che nasce dall'errore del wrapper e non dal
        // testo letto.
        let json = serde_json::to_string(&testo).expect("stringa");
        let errore = serde_json::from_str::<CommitToken>(&json).expect_err("non canonico");
        assert!(
            !errore.to_string().to_lowercase().contains(&SENTINELLA[..8]),
            "l'errore di serde ha copiato il valore: {errore}"
        );
    }
}

#[test]
fn la_chiave_del_footer_e_una_sola_e_dichiarata() {
    assert_eq!(CHIAVE_FOOTER_COMMIT_TOKEN, "plenora.commit.token");
    assert_eq!(CANONICO.len(), CARATTERI);
}
