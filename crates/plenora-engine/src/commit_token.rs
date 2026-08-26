//! Il `commit_token`: un tipo **chiuso**, non una stringa che sembra un token.
//!
//! # Perche' e' un tipo e non un `String`
//!
//! Il token attraversa quattro confini — il chiamante che lo fornisce,
//! l'handshake che lo trasmette, il writer che lo sigilla nel footer, il
//! verificatore che lo rilegge — e a ognuno di quei confini una `String`
//! avrebbe richiesto lo **stesso** controllo, scritto da capo. Quattro copie
//! di un controllo sono quattro occasioni di scriverne una sbagliata, o di
//! dimenticarne una.
//!
//! Qui il controllo sta in un punto solo: [`CommitToken::da_esadecimale`]. Chi
//! ha un [`CommitToken`] in mano non ha bisogno di validarlo, perche' non
//! esiste un `CommitToken` non valido.
//!
//! # La forma canonica
//!
//! Esattamente 64 caratteri esadecimali **minuscoli**: la forma testuale di
//! 32 byte. Le maiuscole non sono una variante accettabile ma un rifiuto, e
//! non per pedanteria — il token finisce in un footer che viene confrontato
//! byte per byte, e due grafie dello stesso valore darebbero due artefatti
//! diversi per lo stesso commit.
//!
//! # Il valore non compare mai
//!
//! `Debug` e `Display` non lo mostrano, e nessun errore di questo modulo lo
//! contiene. E' l'unica proprieta' del tipo che vada difesa deliberatamente:
//! un token in un log e' un token che qualcun altro puo' presentare.
//!
//! La serializzazione invece lo emette, e l'asimmetria e' voluta: sul filo
//! serve, in un log no.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Byte del token: 32, cioe' 64 caratteri esadecimali.
///
/// `pub` qui non significa pubblico: il **modulo** e' privato, e dal crate
/// esce solo cio' che `lib.rs` ri-esporta esplicitamente — [`CommitToken`] e
/// [`FormaTokenNonValida`]. Questa costante resta interna.
pub const COMMIT_TOKEN_BYTES: usize = 32;

/// Caratteri della forma canonica.
pub const COMMIT_TOKEN_CARATTERI: usize = COMMIT_TOKEN_BYTES * 2;

/// La chiave con cui il token vive nel footer di un artefatto.
///
/// **Unica.** Un secondo nome per la stessa cosa renderebbe legittima la
/// domanda «quale delle due vale», che non ha una risposta buona.
pub const CHIAVE_FOOTER_COMMIT_TOKEN: &str = "plenora.commit.token";

/// Perche' un testo non e' un `commit_token`.
///
/// **Nessuna variante porta il valore**, e nemmeno un suo frammento: un
/// errore che cita il token e' un token in un log. La posizione del primo
/// carattere sbagliato invece c'e', perche' e' struttura e non contenuto, e
/// senza di essa un rifiuto non si sa dove guardare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormaTokenNonValida {
    /// Il numero di caratteri non e' quello richiesto.
    LunghezzaErrata {
        /// Caratteri attesi.
        attesi: usize,
        /// Caratteri trovati.
        trovati: usize,
    },
    /// Un carattere non e' esadecimale.
    NonEsadecimale {
        /// Posizione, in caratteri dall'inizio.
        posizione: usize,
    },
    /// Un carattere esadecimale e' maiuscolo.
    ///
    /// Distinta da [`Self::NonEsadecimale`] perche' e' un errore diverso, e
    /// dirlo cambia cosa fa chi lo legge: `A` non e' spazzatura, e' la grafia
    /// sbagliata di un valore che potrebbe essere giusto.
    MaiuscoloNonAmmesso {
        /// Posizione, in caratteri dall'inizio.
        posizione: usize,
    },
}

impl fmt::Display for FormaTokenNonValida {
    fn fmt(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LunghezzaErrata { attesi, trovati } => write!(
                formattatore,
                "il commit_token ha {trovati} caratteri, ne servono esattamente {attesi}"
            ),
            Self::NonEsadecimale { posizione } => write!(
                formattatore,
                "il commit_token ha un carattere non esadecimale in posizione {posizione}"
            ),
            Self::MaiuscoloNonAmmesso { posizione } => write!(
                formattatore,
                "il commit_token ha una maiuscola in posizione {posizione}; la forma \
                 canonica e' minuscola"
            ),
        }
    }
}

impl std::error::Error for FormaTokenNonValida {}

/// Un `commit_token` valido, per costruzione.
///
/// La rappresentazione e' **opaca**: i 32 byte non sono raggiungibili, e la
/// sola uscita e' [`Self::in_esadecimale`], che rende sempre la forma
/// canonica. Non c'e' un modo di ottenere una grafia diversa da quella che
/// finira' nel footer.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommitToken {
    byte: [u8; COMMIT_TOKEN_BYTES],
}

impl CommitToken {
    /// Interpreta un testo come `commit_token`.
    ///
    /// # Errors
    ///
    /// [`FormaTokenNonValida`] se il testo non e' esattamente
    /// [`COMMIT_TOKEN_CARATTERI`] caratteri esadecimali minuscoli.
    pub fn da_esadecimale(testo: &str) -> Result<Self, FormaTokenNonValida> {
        // Sui **byte** e non sui caratteri: un testo UTF-8 multibyte non e'
        // esadecimale, quindi contare i byte non e' un'approssimazione — e
        // contare i caratteri costerebbe una scansione in piu' per rifiutare
        // le stesse cose.
        if testo.len() != COMMIT_TOKEN_CARATTERI {
            return Err(FormaTokenNonValida::LunghezzaErrata {
                attesi: COMMIT_TOKEN_CARATTERI,
                trovati: testo.chars().count(),
            });
        }
        // `as_chunks` e non `chunks_exact`: la lunghezza e' gia' stata
        // verificata sopra, quindi la coda e' vuota per costruzione e il
        // compilatore lo sa.
        let (coppie, _) = testo.as_bytes().as_chunks::<2>();
        let mut byte = [0_u8; COMMIT_TOKEN_BYTES];
        for (indice, coppia) in coppie.iter().enumerate() {
            let alto = cifra(coppia[0], indice * 2)?;
            let basso = cifra(coppia[1], indice * 2 + 1)?;
            byte[indice] = (alto << 4) | basso;
        }
        Ok(Self { byte })
    }

    /// La forma canonica: 64 caratteri esadecimali minuscoli.
    ///
    /// Ricostruita dai byte e non conservata dal testo d'origine, cosi' non
    /// puo' esistere un `CommitToken` che rende una grafia diversa da quella
    /// che il footer riceverebbe.
    #[must_use]
    pub fn in_esadecimale(&self) -> String {
        const CIFRE: &[u8; 16] = b"0123456789abcdef";
        let mut fuori = String::with_capacity(COMMIT_TOKEN_CARATTERI);
        for grezzo in self.byte {
            fuori.push(char::from(CIFRE[usize::from(grezzo >> 4)]));
            fuori.push(char::from(CIFRE[usize::from(grezzo & 0x0F)]));
        }
        fuori
    }
}

/// Il valore di una cifra esadecimale minuscola.
const fn cifra(grezzo: u8, posizione: usize) -> Result<u8, FormaTokenNonValida> {
    match grezzo {
        b'0'..=b'9' => Ok(grezzo - b'0'),
        b'a'..=b'f' => Ok(grezzo - b'a' + 10),
        b'A'..=b'F' => Err(FormaTokenNonValida::MaiuscoloNonAmmesso { posizione }),
        _ => Err(FormaTokenNonValida::NonEsadecimale { posizione }),
    }
}

/// Non mostra il valore.
///
/// `Debug` e' cio' che finisce in un log per sbaglio — dentro un `{:?}` di una
/// struttura piu' grande, in un `unwrap` che stampa il contesto, in un
/// messaggio d'errore di una dipendenza. E' esattamente il posto in cui un
/// token non deve poter arrivare, quindi non ci arriva.
impl fmt::Debug for CommitToken {
    fn fmt(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        formattatore.write_str("CommitToken(<non mostrato>)")
    }
}

/// Non mostra il valore, per lo stesso motivo di [`Debug`](fmt::Debug).
///
/// Chi lo vuole davvero chiama [`CommitToken::in_esadecimale`], che e' una
/// riga che si legge in review.
impl fmt::Display for CommitToken {
    fn fmt(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        formattatore.write_str("<commit_token non mostrato>")
    }
}

/// Emette la forma canonica.
///
/// L'asimmetria con `Display` e' voluta e non una svista: sul filo il valore
/// serve, in un log no.
impl Serialize for CommitToken {
    fn serialize<S: Serializer>(&self, serializzatore: S) -> Result<S::Ok, S::Error> {
        serializzatore.serialize_str(&self.in_esadecimale())
    }
}

/// Accetta **solo** la forma canonica.
///
/// Un token maiuscolo o di lunghezza sbagliata non viene normalizzato: viene
/// rifiutato. Normalizzarlo significherebbe accettare due grafie per lo stesso
/// valore, e il footer le distinguerebbe comunque.
impl<'de> Deserialize<'de> for CommitToken {
    fn deserialize<D: Deserializer<'de>>(deserializzatore: D) -> Result<Self, D::Error> {
        let testo = <&str>::deserialize(deserializzatore)?;
        // Il messaggio d'errore di `serde` **non** cita il testo: `custom`
        // riceve solo cio' che scriviamo noi, e `FormaTokenNonValida` non
        // porta il valore.
        Self::da_esadecimale(testo).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
