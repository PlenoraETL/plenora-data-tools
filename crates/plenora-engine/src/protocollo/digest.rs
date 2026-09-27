//! Un digest SHA-256 sul filo: **un tipo**, non una stringa che sembra un
//! digest.
//!
//! Con una `String` la forma di ogni campo digest dipenderebbe da un
//! controllo scritto altrove, da ricordare a ogni campo nuovo. Qui la forma e'
//! del tipo: non esiste un `DigestSha256` non canonico, c'e' un costruttore
//! che rifiuta.
//!
//! # Perche' resta distinto da `CommitToken`
//!
//! La rappresentazione e' condivisa in [`crate::esadecimale32`]. La politica
//! di visualizzazione no: un digest e' l'identita' di un contenuto e si
//! **mostra**, per confrontarlo; un `commit_token` identifica un tentativo e
//! non compare mai. Per questo la primitiva condivisa non ha ne' `Debug` ne'
//! `Display`.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// L'unico algoritmo di digest ammesso in v1.
///
/// Sta col tipo, non col verificatore, perche' la usano entrambi i lati: chi
/// dichiara l'algoritmo scrivendo l'artefatto e chi lo pretende rileggendolo. Il campo `algoritmo`
/// di [`super::messaggi::DigestArtefatto`] resta una stringa sul filo; la
/// coerenza si impone qui. Un secondo algoritmo cambia insieme valore
/// canonico, lunghezza e tipo.
pub const ALGORITMO_DIGEST: &str = "sha256";

use crate::esadecimale32::{self, DaEsadecimale32, Esadecimale32, FormaNonValida};

/// Lo SHA-256 di tutto cio' che `lettore` rende, a blocchi di 64 KiB.
///
/// La memoria non cresce col file. Aprire la sorgente e dare un nome agli
/// errori resta al chiamante, che sa che cosa sta leggendo.
///
/// # Errors
///
/// L'errore di `read`, invariato.
pub fn sha256_da_lettore(lettore: &mut impl std::io::Read) -> std::io::Result<Esadecimale32> {
    use sha2::{Digest as _, Sha256};

    // Mai zero: con un blocco vuoto `read` renderebbe 0, cioe' «fine», e il
    // digest sarebbe quello del file vuoto.
    let mut digestore = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let quanti = lettore.read(&mut buffer)?;
        if quanti == 0 {
            return Ok(Esadecimale32::dai_byte(digestore.finalize().into()));
        }
        digestore.update(&buffer[..quanti]);
    }
}

/// Byte di un digest SHA-256: **32**, e l'autorita' e' quella della primitiva.
pub const DIGEST_BYTES: usize = esadecimale32::BYTE;

/// Perche' un testo non e' un digest.
///
/// **Non porta il valore**: il testo rifiutato lo sceglie l'altro capo, e
/// copiarlo gli farebbe scrivere nel log di chi indaga. La posizione c'e',
/// perche' e' struttura.
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
    /// Una variante sola, dove il `commit_token` ne ha due: qui maiuscola e
    /// spazzatura portano alla stessa azione, riscrivere il campo.
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
/// Due digest diversi affiancati sono la diagnosi di un disaccordo. E' scritto
/// a mano, non con `derive`, perche' e' la politica che distingue i due tipi.
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
