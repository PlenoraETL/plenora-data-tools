//! Un digest SHA-256 sul filo: **un tipo**, non una stringa che sembra un
//! digest.
//!
//! # Perche' non basta un controllo in `codifica`
//!
//! Quattro campi del protocollo portano l'uscita testuale di uno SHA-256 —
//! l'identita' dell'artefatto, il digest dell'insieme delle risorse, il
//! `plan_hash_atteso` e il fingerprint del contratto d'ingresso. Con una
//! `String` la loro forma dipenderebbe da un controllo scritto altrove:
//! quattro chiamate che qualcuno puo' dimenticare di aggiungere al quinto
//! campo.
//!
//! Qui la forma e' del tipo. Non esiste un `DigestSha256` non canonico, quindi
//! non c'e' un controllo da ricordare: c'e' un costruttore che rifiuta.
//!
//! # Perche' resta distinto da `CommitToken`
//!
//! La rappresentazione e' **la stessa**, e infatti e' condivisa:
//! [`crate::esadecimale32`] tiene i 32 byte, il parsing, la forma canonica e
//! la lettura sanificata. Cio' che i due tipi non condividono e' quel che li
//! rende due tipi:
//!
//! - un digest e' l'identita' di un contenuto, e va **confrontato**: `Debug` e
//!   `Display` lo mostrano, perche' vedere due digest diversi affiancati e'
//!   il modo in cui si diagnostica un disaccordo;
//! - un `commit_token` identifica un **tentativo**, e non compare mai: ne'
//!   in `Debug`, ne' in `Display`, ne' in un errore.
//!
//! La primitiva condivisa non ha percio' ne' `Debug` ne' `Display`: darglieli
//! avrebbe imposto una politica a entrambi, e la piu' comoda — mostrare — e'
//! quella che un giorno mette il token in un log.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::esadecimale32::{self, DaEsadecimale32, Esadecimale32, FormaNonValida};

/// Byte di un digest SHA-256: **32**, e l'autorita' e' quella della primitiva.
pub const DIGEST_BYTES: usize = esadecimale32::BYTE;

/// Perche' un testo non e' un digest.
///
/// **Non porta il valore.** Non perche' un digest sia segreto — non lo e' — ma
/// perche' su questo canale il testo che ha fallito la conversione lo sceglie
/// l'altro capo, e un errore che lo copia e' un modo di far scrivere nel log
/// di chi indaga. La posizione c'e', perche' e' struttura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormaDigestNonValida {
    /// La lunghezza non e' quella richiesta, **in byte**.
    LunghezzaErrata {
        /// Byte attesi.
        attesi: usize,
        /// Byte trovati.
        trovati: usize,
    },
    /// Un carattere non e' esadecimale minuscolo.
    ///
    /// Una variante sola dove il `commit_token` ne ha due: qui la maiuscola e
    /// la spazzatura portano alla stessa azione — riscrivere il campo — mentre
    /// per il token la distinzione dice a chi legge che il valore poteva
    /// essere giusto e la grafia no.
    NonEsadecimaleMinuscolo {
        /// Posizione, in byte dall'inizio.
        posizione: usize,
    },
}

impl fmt::Display for FormaDigestNonValida {
    fn fmt(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LunghezzaErrata { attesi, trovati } => write!(
                formattatore,
                "il digest e' lungo {trovati} byte, ne servono esattamente {attesi}"
            ),
            Self::NonEsadecimaleMinuscolo { posizione } => write!(
                formattatore,
                "il digest ha un carattere che non e' esadecimale minuscolo in \
                 posizione {posizione}"
            ),
        }
    }
}

impl std::error::Error for FormaDigestNonValida {}

/// Un digest SHA-256 valido, per costruzione.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DigestSha256(Esadecimale32);

impl DigestSha256 {
    /// Interpreta un testo come digest.
    ///
    /// # Errors
    ///
    /// [`FormaDigestNonValida`] se non e' esattamente 64 caratteri esadecimali
    /// minuscoli.
    pub fn da_esadecimale(testo: &str) -> Result<Self, FormaDigestNonValida> {
        Esadecimale32::da_esadecimale(testo)
            .map(Self)
            .map_err(Self::da_forma_non_valida)
    }

    /// La forma canonica: 64 caratteri esadecimali minuscoli.
    pub fn in_esadecimale(&self) -> String {
        self.0.in_esadecimale()
    }
}

impl DaEsadecimale32 for DigestSha256 {
    type Errore = FormaDigestNonValida;

    const NOME: &'static str = "il digest";

    fn da_esadecimale32(primitiva: Esadecimale32) -> Self {
        Self(primitiva)
    }

    fn da_forma_non_valida(difetto: FormaNonValida) -> Self::Errore {
        match difetto {
            FormaNonValida::LunghezzaErrata { attesi, trovati } => {
                FormaDigestNonValida::LunghezzaErrata { attesi, trovati }
            }
            FormaNonValida::Maiuscolo { posizione }
            | FormaNonValida::NonEsadecimale { posizione } => {
                FormaDigestNonValida::NonEsadecimaleMinuscolo { posizione }
            }
        }
    }
}

/// Mostra il valore, a differenza di `CommitToken`.
///
/// Un digest e' l'identita' di un contenuto e serve **confrontarlo**: due
/// digest diversi affiancati sono la diagnosi di un disaccordo. La scelta e'
/// scritta e non ereditata da un `derive`, perche' e' meta' della ragione per
/// cui questo tipo esiste separato.
impl fmt::Debug for DigestSha256 {
    fn fmt(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formattatore, "DigestSha256({})", self.in_esadecimale())
    }
}

impl fmt::Display for DigestSha256 {
    fn fmt(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        formattatore.write_str(&self.in_esadecimale())
    }
}

impl Serialize for DigestSha256 {
    fn serialize<S: Serializer>(&self, serializzatore: S) -> Result<S::Ok, S::Error> {
        self.0.serializza(serializzatore)
    }
}

/// Accetta **solo** la forma canonica.
impl<'de> Deserialize<'de> for DigestSha256 {
    fn deserialize<D: Deserializer<'de>>(deserializzatore: D) -> Result<Self, D::Error> {
        esadecimale32::deserializza(deserializzatore)
    }
}

#[cfg(test)]
mod tests;
