//! Un digest SHA-256 sul filo: **un tipo**, non una stringa che sembra un
//! digest.
//!
//! # Perche' non basta un controllo in `codifica`
//!
//! Quattro campi del protocollo portano l'uscita testuale di uno SHA-256 —
//! l'identita' dell'artefatto, il digest dell'insieme delle risorse, il
//! `plan_hash_atteso` e il fingerprint del contratto d'ingresso. Con una
//! `String` la loro forma dipendeva da un controllo scritto altrove: quattro
//! chiamate che qualcuno poteva dimenticare di aggiungere al quinto campo.
//!
//! Qui la forma e' del tipo. Non esiste un `DigestSha256` non canonico, quindi
//! non c'e' un controllo da ricordare: c'e' un costruttore che rifiuta.
//!
//! # Perche' resta distinto da `CommitToken`
//!
//! La rappresentazione e' la stessa — 32 byte, 64 caratteri esadecimali
//! minuscoli — e la tentazione di unificarli e' esattamente il motivo per cui
//! sono due tipi. Differiscono in **cosa dicono** e in **cosa si puo'
//! mostrare**:
//!
//! - un digest e' l'identita' di un contenuto, e va **confrontato**: `Debug` e
//!   `Display` lo mostrano, perche' vedere due digest diversi affiancati e'
//!   il modo in cui si diagnostica un disaccordo;
//! - un `commit_token` identifica un **tentativo**, e non compare mai: ne'
//!   in `Debug`, ne' in `Display`, ne' in un errore.
//!
//! Un tipo solo avrebbe dovuto scegliere una delle due politiche, e
//! qualunque scelta sarebbe stata sbagliata per meta' degli usi. La piu'
//! pericolosa e' la piu' comoda: mostrarli tutti, e scoprire un giorno che il
//! token e' finito in un log perche' condivideva un `Debug` con qualcosa che
//! non era segreto.

use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Byte di un digest SHA-256: **32**, ed e' questa l'autorita'.
///
/// I 64 caratteri della forma sul filo si derivano da qui — `limiti.rs` li
/// calcola come `DIGEST_BYTES * 2` — e non il contrario. Derivare i byte dai
/// caratteri sembrava equivalente e non lo e': `caratteri / 2` su un numero
/// dispari ne perderebbe uno in silenzio, e un 66 continuerebbe a compilare
/// dichiarando 33 byte, che non e' piu' uno SHA-256. Il numero che non puo'
/// cambiare senza cambiare algoritmo e' questo.
pub const DIGEST_BYTES: usize = 32;

/// Caratteri della forma esadecimale, derivati.
const DIGEST_CARATTERI: usize = DIGEST_BYTES * 2;

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
    /// Le maiuscole non sono una variante accettabile: il confronto fra i due
    /// lati e' un confronto di stringhe, e `AA…` e `aa…` sarebbero due digest
    /// diversi dello stesso valore.
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
///
/// I 32 byte non sono raggiungibili: la sola uscita e' la forma canonica,
/// **ricostruita** dai byte e non conservata dal testo d'origine, cosi' non
/// puo' esistere un digest che rende una grafia diversa da quella che
/// viaggera' sul filo.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DigestSha256 {
    byte: [u8; DIGEST_BYTES],
}

impl DigestSha256 {
    /// Interpreta un testo come digest.
    ///
    /// # Errors
    ///
    /// [`FormaDigestNonValida`] se non e' esattamente 64 caratteri esadecimali
    /// minuscoli.
    pub fn da_esadecimale(testo: &str) -> Result<Self, FormaDigestNonValida> {
        // Sui byte e non sui caratteri, e il numero riportato e' la stessa
        // misura che ha deciso: due misure diverse nello stesso errore si
        // contraddicono proprio nei casi in cui l'errore serve.
        if testo.len() != DIGEST_CARATTERI {
            return Err(FormaDigestNonValida::LunghezzaErrata {
                attesi: DIGEST_CARATTERI,
                trovati: testo.len(),
            });
        }
        let (coppie, _) = testo.as_bytes().as_chunks::<2>();
        let mut byte = [0_u8; DIGEST_BYTES];
        for (indice, coppia) in coppie.iter().enumerate() {
            let alto = cifra(coppia[0], indice * 2)?;
            let basso = cifra(coppia[1], indice * 2 + 1)?;
            byte[indice] = (alto << 4) | basso;
        }
        Ok(Self { byte })
    }

    /// La forma canonica: 64 caratteri esadecimali minuscoli.
    pub fn in_esadecimale(&self) -> String {
        const CIFRE: &[u8; 16] = b"0123456789abcdef";
        let mut fuori = String::with_capacity(DIGEST_CARATTERI);
        for grezzo in self.byte {
            fuori.push(char::from(CIFRE[usize::from(grezzo >> 4)]));
            fuori.push(char::from(CIFRE[usize::from(grezzo & 0x0F)]));
        }
        fuori
    }
}

/// Il valore di una cifra esadecimale **minuscola**.
const fn cifra(grezzo: u8, posizione: usize) -> Result<u8, FormaDigestNonValida> {
    match grezzo {
        b'0'..=b'9' => Ok(grezzo - b'0'),
        b'a'..=b'f' => Ok(grezzo - b'a' + 10),
        _ => Err(FormaDigestNonValida::NonEsadecimaleMinuscolo { posizione }),
    }
}

/// Mostra il valore, a differenza di `CommitToken`.
///
/// Un digest e' l'identita' di un contenuto e serve **confrontarlo**: due
/// digest diversi affiancati sono la diagnosi di un disaccordo. Il motivo per
/// cui questa riga esiste e' che la scelta sia deliberata e scritta, non
/// ereditata da un `derive`.
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
        serializzatore.serialize_str(&self.in_esadecimale())
    }
}

/// Accetta **solo** la forma canonica.
///
/// `deserialize_any` e non `deserialize_str`, per la stessa ragione del
/// `commit_token`: di fronte a un numero `serde_json` non chiamerebbe il
/// visitatore e comporrebbe da se' un messaggio **dal valore letto**.
impl<'de> Deserialize<'de> for DigestSha256 {
    fn deserialize<D: Deserializer<'de>>(deserializzatore: D) -> Result<Self, D::Error> {
        deserializzatore.deserialize_any(VisitatoreDigest)
    }
}

/// Il rifiuto di cio' che non e' testo, **senza** il valore.
fn non_testuale<E: de::Error>() -> E {
    E::custom(format_args!(
        "il digest deve essere una stringa di {DIGEST_CARATTERI} caratteri \
         esadecimali minuscoli, non un valore di un altro tipo"
    ))
}

/// Un visitatore **suo**, non quello del `commit_token`.
///
/// I due tipi hanno la stessa forma e politiche di riservatezza diverse:
/// condividere il visitatore significherebbe condividere anche la scelta su
/// cosa un messaggio d'errore puo' dire, che e' precisamente cio' che li
/// distingue.
struct VisitatoreDigest;

impl de::Visitor<'_> for VisitatoreDigest {
    type Value = DigestSha256;

    fn expecting(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formattatore,
            "una stringa di {DIGEST_CARATTERI} caratteri esadecimali minuscoli"
        )
    }

    fn visit_str<E: de::Error>(self, testo: &str) -> Result<Self::Value, E> {
        DigestSha256::da_esadecimale(testo).map_err(E::custom)
    }

    // Le larghezze minori ricadono per default su questi; i due a 128 bit no,
    // e il loro default di `serde` nomina il tipo invece del valore. Sono
    // coperti lo stesso perche' quel default appartiene a una dipendenza.
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Err(non_testuale())
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Err(non_testuale())
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Err(non_testuale())
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Err(non_testuale())
    }

    fn visit_i128<E: de::Error>(self, _: i128) -> Result<Self::Value, E> {
        Err(non_testuale())
    }

    fn visit_u128<E: de::Error>(self, _: u128) -> Result<Self::Value, E> {
        Err(non_testuale())
    }
}

#[cfg(test)]
mod tests;
