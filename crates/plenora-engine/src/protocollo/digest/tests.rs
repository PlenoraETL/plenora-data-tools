//! Prove del digest sul filo.
//!
//! Due gruppi che non si coprono a vicenda: la **forma** — cosa si accetta e
//! cosa no — e la **distinzione dal `commit_token`**, che ha la stessa
//! rappresentazione e politiche opposte su cosa si puo' mostrare.

use serde::de::Visitor as _;

use super::{DigestSha256, FormaDigestNonValida, VisitatoreDigest};
use crate::protocollo::limiti::MAX_DIGEST_BYTES;

/// Un digest canonico, scritto a mano.
const CANONICO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// La sentinella: se compare in un errore, il valore e' trapelato.
const SENTINELLA: &str = "deadbeefcafebabe1234567890abcdef00112233445566778899aabbccddeeff";

#[test]
fn la_forma_canonica_si_accetta_e_torna_identica() {
    let digest = DigestSha256::da_esadecimale(CANONICO).expect("canonico");
    assert_eq!(digest.in_esadecimale(), CANONICO);
}

/// La forma si **ricostruisce** dai byte, non si conserva dal testo.
///
/// Conservando il testo, un digest costruito da una grafia qualsiasi la
/// renderebbe tale e quale, e sul filo finirebbe qualcosa che non e' canonico.
#[test]
fn due_digest_uguali_rendono_lo_stesso_testo() {
    let uno = DigestSha256::da_esadecimale(CANONICO).expect("canonico");
    let due = DigestSha256::da_esadecimale(CANONICO).expect("canonico");
    assert_eq!(uno, due);
    assert_eq!(uno.in_esadecimale(), due.in_esadecimale());
}

/// Ogni forma non canonica e' un rifiuto, e nessuna si normalizza.
#[test]
fn le_forme_non_canoniche_si_rifiutano() {
    let corti = ["", "a", &CANONICO[..MAX_DIGEST_BYTES - 1]];
    for testo in corti {
        assert!(
            matches!(
                DigestSha256::da_esadecimale(testo),
                Err(FormaDigestNonValida::LunghezzaErrata { .. })
            ),
            "accettato un digest di {} byte",
            testo.len()
        );
    }
    let lungo = format!("{CANONICO}0");
    assert!(matches!(
        DigestSha256::da_esadecimale(&lungo),
        Err(FormaDigestNonValida::LunghezzaErrata { .. })
    ));

    // La maiuscola: stessa lunghezza, valore che potrebbe essere giusto, e
    // rifiutata lo stesso — il confronto fra i due lati e' fra stringhe.
    let maiuscolo = CANONICO.to_uppercase();
    assert!(matches!(
        DigestSha256::da_esadecimale(&maiuscolo),
        Err(FormaDigestNonValida::NonEsadecimaleMinuscolo { .. })
    ));

    // E un carattere fuori alfabeto, in una posizione nota.
    let mut byte = CANONICO.as_bytes().to_vec();
    byte[7] = b'g';
    let fuori = String::from_utf8(byte).expect("ASCII");
    assert_eq!(
        DigestSha256::da_esadecimale(&fuori),
        Err(FormaDigestNonValida::NonEsadecimaleMinuscolo { posizione: 7 })
    );
}

/// La lunghezza riportata e' quella **che ha deciso**: byte, non caratteri.
#[test]
fn la_lunghezza_riportata_e_quella_che_ha_deciso() {
    let accentato = format!("{}èè", &CANONICO[..62]);
    assert_eq!(accentato.chars().count(), MAX_DIGEST_BYTES);
    assert_eq!(
        DigestSha256::da_esadecimale(&accentato),
        Err(FormaDigestNonValida::LunghezzaErrata {
            attesi: MAX_DIGEST_BYTES,
            trovati: MAX_DIGEST_BYTES + 2,
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

/// Anche in lettura la forma non canonica si rifiuta, e i tipi sbagliati pure.
#[test]
fn serde_rifiuta_le_forme_non_canoniche_e_i_tipi_sbagliati() {
    for testo in [
        CANONICO.to_uppercase(),
        CANONICO[..63].to_owned(),
        "non un digest".to_owned(),
        String::new(),
    ] {
        let json = serde_json::to_string(&testo).expect("stringa");
        assert!(
            serde_json::from_str::<DigestSha256>(&json).is_err(),
            "accettata la forma non canonica «{testo}»"
        );
    }
    for json in ["1234", "null", "[]", "{}", "true"] {
        assert!(
            serde_json::from_str::<DigestSha256>(json).is_err(),
            "accettato `{json}` come digest"
        );
    }
}

/// Nessun errore riporta il testo che ha rifiutato.
///
/// Un digest non e' segreto, ma su questo canale il testo che ha fallito la
/// conversione lo sceglie l'altro capo: copiarlo nell'errore e' un modo di far
/// scrivere testo arbitrario nel log di chi indaga.
#[test]
fn nessun_errore_riporta_il_testo_rifiutato() {
    /// La sentinella numerica, per il campo che non e' testo.
    const NUMERICA: &str = "8675309124816324";

    let sospetti = [
        SENTINELLA.to_uppercase(),
        SENTINELLA[..63].to_owned(),
        format!("{SENTINELLA}0"),
        format!("{}g{}", &SENTINELLA[..10], &SENTINELLA[11..]),
    ];
    for testo in sospetti {
        let errore = DigestSha256::da_esadecimale(&testo).expect_err("non canonico");
        for reso in [format!("{errore}"), format!("{errore:?}")] {
            assert!(
                !reso.to_lowercase().contains(&SENTINELLA[..8]),
                "l'errore ha copiato il testo: {reso}"
            );
        }

        // E l'errore di `serde`, che nasce da `custom` e non dal testo.
        let json = serde_json::to_string(&testo).expect("stringa");
        let errore = serde_json::from_str::<DigestSha256>(&json).expect_err("non canonico");
        assert!(
            !errore.to_string().to_lowercase().contains(&SENTINELLA[..8]),
            "l'errore di serde ha copiato il testo: {errore}"
        );
    }

    // Il valore non testuale, che `serde_json` sbrigherebbe da se' con
    // `deserialize_str`.
    let errore = serde_json::from_str::<DigestSha256>(NUMERICA).expect_err("non e' un digest");
    assert!(
        !errore.to_string().contains(NUMERICA),
        "l'errore ha copiato il valore ricevuto: {errore}"
    );
}

/// I due a 128 bit, che dal giro JSON non si raggiungono.
#[test]
fn anche_gli_interi_a_128_bit_rifiutano_senza_portare_il_valore() {
    const SENTINELLA_NUMERICA: i128 = 8_675_309_124_816_324;
    let attesa = SENTINELLA_NUMERICA.to_string();

    for reso in [
        VisitatoreDigest
            .visit_i128::<serde_json::Error>(SENTINELLA_NUMERICA)
            .expect_err("non e' un digest")
            .to_string(),
        VisitatoreDigest
            .visit_u128::<serde_json::Error>(SENTINELLA_NUMERICA.unsigned_abs())
            .expect_err("non e' un digest")
            .to_string(),
    ] {
        assert!(
            !reso.contains(&attesa),
            "l'errore ha copiato il valore: {reso}"
        );
    }
}

/// Il digest **mostra** il valore, il `commit_token` no.
///
/// E' la differenza per cui i due tipi restano distinti pur avendo la stessa
/// rappresentazione. Un tipo solo avrebbe dovuto scegliere una politica sola,
/// e la piu' comoda — mostrarli entrambi — e' quella che un giorno mette il
/// token in un log.
#[test]
fn il_digest_si_mostra_mentre_il_token_no() {
    let digest = DigestSha256::da_esadecimale(SENTINELLA).expect("canonico");
    assert!(format!("{digest}").contains(SENTINELLA));
    assert!(format!("{digest:?}").contains(SENTINELLA));

    let token = crate::commit_token::CommitToken::da_esadecimale(SENTINELLA).expect("canonico");
    assert!(!format!("{token}").contains(SENTINELLA));
    assert!(!format!("{token:?}").contains(SENTINELLA));
}
