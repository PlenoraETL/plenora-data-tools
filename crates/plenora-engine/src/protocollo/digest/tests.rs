//! Prove del digest sul filo: **solo cio' che non e' la forma**.
//!
//! La conformita' di forma — accettazione, rifiuti, riservatezza del testo
//! rifiutato — sta nelle prove della rappresentazione condivisa, dove vive il
//! codice che la decide. Qui restano la politica di visualizzazione e la
//! traduzione del difetto, che sono cio' per cui questo tipo esiste separato.

use super::{DigestSha256, FormaDigestNonValida, DIGEST_BYTES};
use crate::protocollo::limiti::MAX_DIGEST_BYTES;

/// Un digest canonico, scritto a mano.
const CANONICO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn il_tetto_del_campo_si_deriva_dai_byte_del_digest() {
    assert_eq!(MAX_DIGEST_BYTES, DIGEST_BYTES * 2);
    assert_eq!(CANONICO.len(), MAX_DIGEST_BYTES);
}

/// Il digest **mostra** il valore, e la scelta e' sua.
///
/// E' la differenza per cui i due tipi restano distinti pur condividendo la
/// rappresentazione: un digest si confronta, e vedere due digest affiancati e'
/// come si diagnostica un disaccordo.
#[test]
fn il_digest_mostra_il_valore_in_debug_e_display() {
    let digest = DigestSha256::da_esadecimale(CANONICO).expect("canonico");
    assert_eq!(format!("{digest}"), CANONICO);
    assert!(format!("{digest:?}").contains(CANONICO));
}

/// La maiuscola e la spazzatura portano allo **stesso** difetto.
///
/// Dove il `commit_token` distingue, qui si riunisce: per un digest l'azione e'
/// la stessa — riscrivere il campo — e una distinzione che non cambia cosa fa
/// chi legge e' una distinzione che invecchia.
#[test]
fn la_traduzione_del_difetto_riunisce_maiuscola_e_spazzatura() {
    let mut byte = CANONICO.as_bytes().to_vec();
    byte[10] = b'A';
    let maiuscolo = String::from_utf8(byte).expect("ASCII");
    assert_eq!(
        DigestSha256::da_esadecimale(&maiuscolo),
        Err(FormaDigestNonValida::NonEsadecimaleMinuscolo { posizione: 10 })
    );

    let mut byte = CANONICO.as_bytes().to_vec();
    byte[7] = b'g';
    let fuori = String::from_utf8(byte).expect("ASCII");
    assert_eq!(
        DigestSha256::da_esadecimale(&fuori),
        Err(FormaDigestNonValida::NonEsadecimaleMinuscolo { posizione: 7 })
    );

    assert_eq!(
        DigestSha256::da_esadecimale("aa"),
        Err(FormaDigestNonValida::LunghezzaErrata {
            attesi: MAX_DIGEST_BYTES,
            trovati: 2,
        })
    );
}

#[test]
fn il_giro_serde_conserva_la_forma_canonica() {
    let digest = DigestSha256::da_esadecimale(CANONICO).expect("canonico");
    let reso = serde_json::to_string(&digest).expect("serializzabile");
    assert_eq!(reso, format!("\"{CANONICO}\""));
    let riletto: DigestSha256 = serde_json::from_str(&reso).expect("rileggibile");
    assert_eq!(riletto, digest);
}
