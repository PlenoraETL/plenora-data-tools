//! Prove della rappresentazione condivisa.
//!
//! Qui sta la **conformita' di forma**: cosa si accetta, cosa si rifiuta, e
//! che nessun rifiuto copi il testo che lo ha causato. Vale per entrambi i
//! wrapper perche' e' l'unico codice che entrambi eseguono.
//!
//! Restano nei due wrapper, e non qui, gli oracoli su cio' che li distingue:
//! la politica di visualizzazione e la traduzione del difetto nel proprio
//! errore. Metterli qui li avrebbe uniti, che e' esattamente la cosa che il
//! tipo separato esiste per impedire.

use serde::de::Visitor as _;

use super::{deserializza, Esadecimale32, FormaNonValida, Visitatore, BYTE, CARATTERI};
use crate::commit_token::CommitToken;
use crate::protocollo::digest::DigestSha256;

/// Un valore canonico, scritto a mano.
const CANONICO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// La sentinella: se compare in un errore, il testo rifiutato e' trapelato.
const SENTINELLA: &str = "deadbeefcafebabe1234567890abcdef00112233445566778899aabbccddeeff";

#[test]
fn i_caratteri_si_derivano_dai_byte() {
    assert_eq!(BYTE, 32, "l'autorita' e' il numero di byte di uno SHA-256");
    assert_eq!(CARATTERI, BYTE * 2);
    assert_eq!(CANONICO.len(), CARATTERI);
}

/// Stesso testo valido → stessi byte e stessa forma canonica.
///
/// La forma si **ricostruisce** dai byte e non si conserva dal testo: senza,
/// un valore costruito da una grafia qualsiasi la renderebbe tale e quale, e
/// sul filo finirebbe qualcosa che non e' canonico.
#[test]
fn lo_stesso_testo_da_lo_stesso_valore_e_la_stessa_forma() {
    let uno = Esadecimale32::da_esadecimale(CANONICO).expect("canonico");
    let due = Esadecimale32::da_esadecimale(CANONICO).expect("canonico");
    // `assert!` e non `assert_eq!`: la primitiva non ha `Debug`, ed e' una
    // proprieta' del tipo, non una scomodita' da aggirare — darglielo qui
    // significherebbe darlo anche in produzione, cioe' imporre una politica.
    assert!(uno == due, "lo stesso testo ha dato valori diversi");
    assert_eq!(uno.in_esadecimale(), CANONICO);
    assert_eq!(due.in_esadecimale(), CANONICO);
}

/// E i **due wrapper** costruiti dallo stesso testo rendono la stessa forma.
///
/// E' la proprieta' che la rappresentazione condivisa deve garantire, e non si
/// puo' provare dentro un wrapper solo: li tocca entrambi, quindi vive qui.
#[test]
fn i_due_wrapper_condividono_la_rappresentazione() {
    let token = CommitToken::da_esadecimale(CANONICO).expect("canonico");
    let digest = DigestSha256::da_esadecimale(CANONICO).expect("canonico");
    assert_eq!(token.in_esadecimale(), digest.in_esadecimale());
    assert_eq!(token.in_esadecimale(), CANONICO);
}

/// Maiuscole, lunghezze errate e caratteri fuori alfabeto: tutti rifiuti.
///
/// La maiuscola non si normalizza: accettarla significherebbe ammettere due
/// grafie di un valore che il confronto — fra stringhe — tratta come due.
#[test]
fn le_forme_non_canoniche_si_rifiutano() {
    for testo in ["", "a", &CANONICO[..CARATTERI - 1], &format!("{CANONICO}0")] {
        assert!(
            matches!(
                Esadecimale32::da_esadecimale(testo).err(),
                Some(FormaNonValida::LunghezzaErrata { .. })
            ),
            "accettato un valore di {} byte",
            testo.len()
        );
    }

    // La lunghezza riportata e' quella **che ha deciso**: byte, non caratteri.
    // Su 64 caratteri con due accentate le due misure divergono, ed e'
    // esattamente il caso in cui il messaggio serve.
    let accentato = format!("{}èè", &CANONICO[..62]);
    assert_eq!(accentato.chars().count(), CARATTERI);
    assert_eq!(
        Esadecimale32::da_esadecimale(&accentato).err(),
        Some(FormaNonValida::LunghezzaErrata {
            attesi: CARATTERI,
            trovati: CARATTERI + 2,
        })
    );

    // La maiuscola e' un difetto **suo**, distinto: e' la distinzione che il
    // `commit_token` porta in API, e senza di essa non potrebbe averla.
    let mut byte = CANONICO.as_bytes().to_vec();
    byte[10] = b'A';
    let maiuscolo = String::from_utf8(byte).expect("ASCII");
    assert_eq!(
        Esadecimale32::da_esadecimale(&maiuscolo).err(),
        Some(FormaNonValida::Maiuscolo { posizione: 10 })
    );

    let mut byte = CANONICO.as_bytes().to_vec();
    byte[7] = b'g';
    let fuori = String::from_utf8(byte).expect("ASCII");
    assert_eq!(
        Esadecimale32::da_esadecimale(&fuori).err(),
        Some(FormaNonValida::NonEsadecimale { posizione: 7 })
    );

    // E nessuna forma maiuscola produce un valore, in nessuna posizione.
    for posizione in 0..CARATTERI {
        let mut byte = CANONICO.as_bytes().to_vec();
        byte[posizione] = byte[posizione].to_ascii_uppercase();
        let testo = String::from_utf8(byte).expect("ASCII");
        if testo == CANONICO {
            continue;
        }
        assert!(
            Esadecimale32::da_esadecimale(&testo).is_err(),
            "accettata una maiuscola in posizione {posizione}"
        );
    }
}

/// Nessun errore riporta il testo che ha rifiutato.
///
/// Su questo canale il testo che fallisce la conversione lo sceglie l'altro
/// capo: copiarlo nell'errore e' un modo di far scrivere testo arbitrario nel
/// log di chi indaga, insieme al motivo per cui lo stava guardando.
#[test]
fn nessun_errore_riporta_il_testo_rifiutato() {
    let sospetti = [
        SENTINELLA.to_uppercase(),
        SENTINELLA[..63].to_owned(),
        format!("{SENTINELLA}0"),
        format!("{}g{}", &SENTINELLA[..10], &SENTINELLA[11..]),
    ];
    for testo in sospetti {
        let difetto = Esadecimale32::da_esadecimale(&testo)
            .err()
            .expect("non canonico");
        assert!(
            !format!("{difetto:?}")
                .to_lowercase()
                .contains(&SENTINELLA[..8]),
            "il difetto ha copiato il testo: {difetto:?}"
        );
    }
}

/// Input **non testuali** rifiutati, e senza il valore ricevuto.
///
/// E' l'asse che `visit_str` da solo lascia aperto: i default del `Visitor`
/// costruiscono `Unexpected` dal valore ricevuto, e `Bool`, `Signed`,
/// `Unsigned` e `Float` lo stampano. La sentinella qui e' un numero.
#[test]
fn i_valori_non_testuali_si_rifiutano_senza_il_valore() {
    /// La sentinella testuale, per il giro JSON.
    const NUMERICA: &str = "8675309124816324";
    /// La sentinella per i due a 128 bit, che dal giro JSON non si raggiungono.
    const SENTINELLA_NUMERICA: i128 = 8_675_309_124_816_324;

    for json in [NUMERICA, "-8675309124816324", "8675309.124816324", "true"] {
        for reso in [
            serde_json::from_str::<CommitToken>(json)
                .expect_err("non e' un token")
                .to_string(),
            serde_json::from_str::<DigestSha256>(json)
                .expect_err("non e' un digest")
                .to_string(),
        ] {
            assert!(
                !reso.contains(NUMERICA) && !reso.contains("8675309"),
                "l'errore ha copiato il valore ricevuto: {reso}"
            );
            assert!(
                reso.contains("non un valore di un altro tipo"),
                "rifiuto inatteso: {reso}"
            );
        }
    }

    // I due a 128 bit non li produce `serde_json`, quindi il giro JSON non li
    // tocca: si esercitano chiamando il visitatore, che e' l'unico modo di
    // provarli senza inventare un formato.
    let attesa = SENTINELLA_NUMERICA.to_string();
    for reso in [
        Visitatore::<CommitToken>(std::marker::PhantomData)
            .visit_i128::<serde_json::Error>(SENTINELLA_NUMERICA)
            .expect_err("non e' un token")
            .to_string(),
        Visitatore::<DigestSha256>(std::marker::PhantomData)
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

/// La lettura sanificata e' **una sola**, e la usano entrambi.
///
/// Si osserva cosi': lo stesso testo canonico attraversa `deserializza` per i
/// due tipi e rende in entrambi i casi la forma d'origine.
#[test]
fn la_deserializzazione_e_una_sola_per_entrambi() {
    let json = format!("\"{CANONICO}\"");
    let token: CommitToken =
        deserializza(&mut serde_json::Deserializer::from_str(&json)).expect("canonico");
    let digest: DigestSha256 =
        deserializza(&mut serde_json::Deserializer::from_str(&json)).expect("canonico");
    assert_eq!(token.in_esadecimale(), CANONICO);
    assert_eq!(digest.in_esadecimale(), CANONICO);
}
