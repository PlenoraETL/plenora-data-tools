//! Trentadue byte in forma esadecimale minuscola: la **rappresentazione**,
//! senza semantica.
//!
//! Una rappresentazione condivisa, due semantiche distinte: il `commit_token`
//! identifica un tentativo, il digest SHA-256 identifica un contenuto. La
//! forma che hanno in comune vive qui, e la regola che la definisce esiste in
//! un punto solo — due copie sarebbero due occasioni di correggerne una sola.
//!
//! I due tipi restano distinti perche' cio' che li distingue **non e' la
//! forma**.
//!
//! # Che cosa questo modulo deliberatamente non ha
//!
//! Nessun `Debug` e nessun `Display`. Sarebbero una politica, e la politica e'
//! esattamente il punto in cui i due tipi divergono: un digest si mostra,
//! perche' vedere due digest affiancati e' come si diagnostica un disaccordo;
//! un token non compare mai, in nessuna forma. Dare qui una politica
//! significherebbe imporla a entrambi, e la piu' comoda — mostrare — e' quella
//! che un giorno mette il token in un log.
//!
//! Per la stessa ragione l'errore di forma di questo modulo **non esce**: ogni
//! wrapper lo traduce nel proprio, che e' il tipo che i suoi chiamanti
//! conoscono.

use std::fmt;

use serde::{de, Deserializer, Serializer};

/// Byte della rappresentazione: **32**, ed e' questa l'autorita'.
///
/// L'invariante e' che la lunghezza testuale **derivi** dai byte. Nel verso
/// opposto la derivazione ammette valori che non sono uno SHA-256 e li lascia
/// compilare.
pub const BYTE: usize = 32;

/// Caratteri della forma esadecimale, derivati.
pub const CARATTERI: usize = BYTE * 2;

/// Perche' un testo non e' trentadue byte esadecimali minuscoli.
///
/// **Non porta il valore**, e nemmeno un frammento: su un canale ostile il
/// testo che ha fallito la conversione lo sceglie l'altro capo, e un errore
/// che lo copia e' un modo di far scrivere nel log di chi indaga. La posizione
/// c'e', perche' e' struttura e non contenuto.
///
/// Le tre varianti sono quelle che i wrapper devono poter distinguere: il
/// `commit_token` tratta la maiuscola come un errore **suo** — `A` non e'
/// spazzatura, e' la grafia sbagliata di un valore che potrebbe essere giusto
/// — mentre il digest le riunisce. Un difetto meno dettagliato di cosi'
/// costringerebbe il primo a rinunciare a una distinzione che ha in API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormaNonValida {
    /// La lunghezza non e' quella richiesta, **in byte**.
    LunghezzaErrata {
        /// Byte attesi.
        attesi: usize,
        /// Byte trovati.
        trovati: usize,
    },
    /// Un carattere esadecimale e' maiuscolo.
    Maiuscolo {
        /// Posizione, in byte dall'inizio.
        posizione: usize,
    },
    /// Un carattere non e' esadecimale.
    NonEsadecimale {
        /// Posizione, in byte dall'inizio.
        posizione: usize,
    },
}

/// Trentadue byte, per costruzione in forma canonica.
///
/// I byte non sono raggiungibili e la forma testuale si **ricostruisce** da
/// essi: non puo' esistere un valore che rende una grafia diversa da quella
/// che finira' sul filo.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Esadecimale32 {
    byte: [u8; BYTE],
}

impl Esadecimale32 {
    /// Interpreta un testo.
    ///
    /// # Errors
    ///
    /// [`FormaNonValida`] se non e' esattamente [`CARATTERI`] caratteri
    /// esadecimali minuscoli.
    pub fn da_esadecimale(testo: &str) -> Result<Self, FormaNonValida> {
        // Sui byte e non sui caratteri: un testo UTF-8 multibyte non e'
        // esadecimale, quindi contarli non e' un'approssimazione. E il numero
        // riportato e' la **stessa** misura che ha deciso: due misure diverse
        // nello stesso errore si contraddicono proprio nei casi in cui
        // l'errore serve.
        if testo.len() != CARATTERI {
            return Err(FormaNonValida::LunghezzaErrata {
                attesi: CARATTERI,
                trovati: testo.len(),
            });
        }
        // `as_chunks` e non `chunks_exact`: la lunghezza e' gia' verificata,
        // quindi la coda e' vuota per costruzione e il compilatore lo sa.
        let (coppie, _) = testo.as_bytes().as_chunks::<2>();
        let mut byte = [0_u8; BYTE];
        for (indice, coppia) in coppie.iter().enumerate() {
            let alto = cifra(coppia[0], indice * 2)?;
            let basso = cifra(coppia[1], indice * 2 + 1)?;
            byte[indice] = (alto << 4) | basso;
        }
        Ok(Self { byte })
    }

    /// Costruisce dai byte grezzi.
    ///
    /// Non valida nulla, e non ha nulla da validare: ogni sequenza di
    /// [`BYTE`] byte ha una e una sola forma canonica, che
    /// [`in_esadecimale`](Self::in_esadecimale) ricostruisce. Serve a chi il
    /// valore lo **calcola** invece di riceverlo come testo — l'uscita di uno
    /// SHA-256, per esempio — e che altrimenti dovrebbe formattarlo a mano per
    /// poi rifarlo interpretare, con due grafie possibili al posto di una.
    pub const fn dai_byte(byte: [u8; BYTE]) -> Self {
        Self { byte }
    }

    /// La forma canonica: [`CARATTERI`] caratteri esadecimali minuscoli.
    pub fn in_esadecimale(&self) -> String {
        const CIFRE: &[u8; 16] = b"0123456789abcdef";
        let mut fuori = String::with_capacity(CARATTERI);
        for grezzo in self.byte {
            fuori.push(char::from(CIFRE[usize::from(grezzo >> 4)]));
            fuori.push(char::from(CIFRE[usize::from(grezzo & 0x0F)]));
        }
        fuori
    }

    /// Emette la forma canonica.
    ///
    /// Sta qui e non nei wrapper perche' e' l'unico verso in cui i due non
    /// divergono: sul filo il valore serve a entrambi, ed e' lo stesso.
    pub fn serializza<S: Serializer>(&self, serializzatore: S) -> Result<S::Ok, S::Error> {
        serializzatore.serialize_str(&self.in_esadecimale())
    }
}

/// Il valore di una cifra esadecimale **minuscola**.
///
/// La maiuscola si distingue e non si accetta: normalizzarla significherebbe
/// ammettere due grafie di un valore che il confronto — che e' fra stringhe —
/// tratterebbe come due valori diversi.
const fn cifra(grezzo: u8, posizione: usize) -> Result<u8, FormaNonValida> {
    match grezzo {
        b'0'..=b'9' => Ok(grezzo - b'0'),
        b'a'..=b'f' => Ok(grezzo - b'a' + 10),
        b'A'..=b'F' => Err(FormaNonValida::Maiuscolo { posizione }),
        _ => Err(FormaNonValida::NonEsadecimale { posizione }),
    }
}

/// Cio' che un wrapper deve dire di se' per farsi deserializzare da qui.
///
/// Tre cose, e nessuna riguarda la forma: come si costruisce, come si chiama
/// nel messaggio d'attesa, e **qual e' il suo errore**. La traduzione del
/// difetto e' del wrapper perche' e' l'unica parte in cui i due divergono:
/// l'uno distingue la maiuscola, l'altro la riunisce agli altri caratteri.
pub trait DaEsadecimale32: Sized {
    /// L'errore che i chiamanti del wrapper conoscono.
    type Errore: fmt::Display;

    /// Come chiamarlo in «attesa una stringa di N caratteri…».
    const NOME: &'static str;

    /// Dal valore canonico al wrapper.
    fn da_esadecimale32(primitiva: Esadecimale32) -> Self;

    /// Dal difetto di forma all'errore del wrapper.
    fn da_forma_non_valida(difetto: FormaNonValida) -> Self::Errore;
}

/// La deserializzazione, **una sola**, sanificata.
///
/// # Errors
///
/// L'errore del formato, che non porta mai il testo rifiutato.
pub fn deserializza<'de, D, T>(deserializzatore: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DaEsadecimale32,
{
    // `deserialize_any` e non `deserialize_str`: di fronte a un numero
    // `serde_json` non chiamerebbe il visitatore e comporrebbe da se' un
    // messaggio **dal valore letto** (`peek_invalid_type`). Il prezzo, che va
    // detto, e' che questa lettura vuole un formato autodescrittivo — sul filo
    // c'e' JSON. Registrato in
    // errori-e-limiti.md#il-commit-token-si-deserializza-solo-da-formati-autodescrittivi.
    deserializzatore.deserialize_any(Visitatore::<T>(std::marker::PhantomData))
}

struct Visitatore<T>(std::marker::PhantomData<T>);

/// Il rifiuto di cio' che non e' testo, **senza** il valore.
fn non_testuale<T: DaEsadecimale32, E: de::Error>() -> E {
    E::custom(format_args!(
        "{} deve essere una stringa di {CARATTERI} caratteri esadecimali \
         minuscoli, non un valore di un altro tipo",
        T::NOME
    ))
}

/// I metodi che rifiutano e basta esistono perche' i default del `Visitor`
/// costruiscono `Unexpected` **dal valore ricevuto**, e tre delle sue varianti
/// lo stampano: `Bool`, `Signed`/`Unsigned`, `Float`. Verificato una variante
/// alla volta: `null`, sequenze e mappe non portano nulla, `Bytes` dice «byte
/// array», e `char` ricade su `visit_str`, dove il testo entra nella
/// validazione vera, che non lo copia.
///
/// I due a 128 bit non ricadono sui precedenti e il loro default nomina il
/// tipo invece del valore. Sono coperti lo stesso: quel default appartiene a
/// una dipendenza, e la riservatezza non puo' dipendere da come una libreria
/// formatta un messaggio.
impl<T: DaEsadecimale32> de::Visitor<'_> for Visitatore<T> {
    type Value = T;

    fn expecting(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formattatore,
            "{} come stringa di {CARATTERI} caratteri esadecimali minuscoli",
            T::NOME
        )
    }

    fn visit_str<E: de::Error>(self, testo: &str) -> Result<Self::Value, E> {
        // Il messaggio non cita il testo: `custom` riceve solo l'errore del
        // wrapper, che per costruzione non porta il valore.
        Esadecimale32::da_esadecimale(testo)
            .map(T::da_esadecimale32)
            .map_err(|difetto| E::custom(T::da_forma_non_valida(difetto)))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Err(non_testuale::<T, E>())
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Err(non_testuale::<T, E>())
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Err(non_testuale::<T, E>())
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Err(non_testuale::<T, E>())
    }

    fn visit_i128<E: de::Error>(self, _: i128) -> Result<Self::Value, E> {
        Err(non_testuale::<T, E>())
    }

    fn visit_u128<E: de::Error>(self, _: u128) -> Result<Self::Value, E> {
        Err(non_testuale::<T, E>())
    }
}

#[cfg(test)]
mod tests;
