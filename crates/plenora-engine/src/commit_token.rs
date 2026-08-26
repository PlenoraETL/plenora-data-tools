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

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

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
    /// La lunghezza non e' quella richiesta.
    ///
    /// **In byte**, come la misura che decide il rifiuto. Contarli in
    /// caratteri per il messaggio sembrava piu' gentile e produceva un
    /// assurdo: su 64 caratteri di cui alcuni multibyte usciva «attesi 64,
    /// trovati 64», cioe' un rifiuto che si smentisce da solo. Sulla forma
    /// canonica, che e' ASCII, byte e caratteri coincidono e la gentilezza non
    /// serviva a nessuno.
    LunghezzaErrata {
        /// Byte attesi.
        attesi: usize,
        /// Byte trovati.
        trovati: usize,
    },
    /// Un carattere non e' esadecimale.
    NonEsadecimale {
        /// Posizione, in byte dall'inizio.
        posizione: usize,
    },
    /// Un carattere esadecimale e' maiuscolo.
    ///
    /// Distinta da [`Self::NonEsadecimale`] perche' e' un errore diverso, e
    /// dirlo cambia cosa fa chi lo legge: `A` non e' spazzatura, e' la grafia
    /// sbagliata di un valore che potrebbe essere giusto.
    MaiuscoloNonAmmesso {
        /// Posizione, in byte dall'inizio.
        posizione: usize,
    },
}

impl fmt::Display for FormaTokenNonValida {
    fn fmt(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LunghezzaErrata { attesi, trovati } => write!(
                formattatore,
                "il commit_token e' lungo {trovati} byte, ne servono esattamente {attesi}"
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
        //
        // Il numero riportato e' la **stessa** misura che ha deciso, non una
        // seconda presa in caratteri: due misure diverse nello stesso errore
        // possono contraddirsi, e lo facevano.
        if testo.len() != COMMIT_TOKEN_CARATTERI {
            return Err(FormaTokenNonValida::LunghezzaErrata {
                attesi: COMMIT_TOKEN_CARATTERI,
                trovati: testo.len(),
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
///
/// # Perche' un visitatore e non `<&str>::deserialize`
///
/// Quello pretende una stringa **presa in prestito** dal buffer d'ingresso, e
/// ci sono due modi ordinari di non poterla dare: una sorgente che scorre
/// (`from_reader` non ha un buffer da cui prestare) e una stringa JSON con
/// escape (`"a..."` va disfatta, e il risultato e' posseduto). In
/// entrambi i casi un token **perfettamente canonico** veniva rifiutato con
/// «expected a borrowed string»: un rifiuto per la forma del trasporto,
/// spacciato per un rifiuto del token.
///
/// Il visitatore accetta il testo comunque arrivi. `visit_string` e
/// `visit_borrowed_str` ricadono per default su `visit_str`, quindi la
/// validazione resta in un punto solo.
///
/// # Perche' `deserialize_any` e non `deserialize_str`
///
/// Sembra il contrario di quel che si vuole — qui si accetta **solo** una
/// stringa — e invece e' l'unico dei due che tiene il valore fuori
/// dall'errore. Chiedendo `deserialize_str`, di fronte a un numero
/// `serde_json` **non chiama il visitatore**: sbriga il disaccordo da se' con
/// `peek_invalid_type`, che costruisce il messaggio dal valore letto. Il
/// rifiuto e' giusto e dice «invalid type: integer `1234…`», cioe' il token in
/// chiaro se qualcuno lo ha scritto senza virgolette.
///
/// Con `deserialize_any` il formato si limita a **dire cosa ha trovato**,
/// chiamando il `visit_*` corrispondente, e la decisione — con il messaggio —
/// torna qui. Il vincolo che ne segue e' registrato in
/// errori-e-limiti.md#il-commit-token-si-deserializza-solo-da-formati-autodescrittivi,
/// con ambito e condizione di rientro. Il prezzo e' che questa
/// `Deserialize` vuole un formato
/// autodescrittivo: sul filo c'e' JSON, e il protocollo non prevede altro.
impl<'de> Deserialize<'de> for CommitToken {
    fn deserialize<D: Deserializer<'de>>(deserializzatore: D) -> Result<Self, D::Error> {
        deserializzatore.deserialize_any(VisitatoreToken)
    }
}

/// Il rifiuto di cio' che non e' testo, **senza** il valore.
///
/// Un `&'static str` costruito qui e nient'altro: non c'e' un parametro in cui
/// il valore ricevuto possa entrare, quindi non e' una regola da rispettare
/// nei sei metodi che lo usano — e' il tipo che non lo consente.
fn non_testuale<E: de::Error>() -> E {
    E::custom(format_args!(
        "il commit_token deve essere una stringa di {COMMIT_TOKEN_CARATTERI} caratteri \
         esadecimali minuscoli, non un valore di un altro tipo"
    ))
}

/// Il visitatore che porta la validazione dove il testo arriva.
struct VisitatoreToken;

/// # Perche' ci sono sei metodi che rifiutano e basta
///
/// I default del `Visitor` rendono `Err(invalid_type(Unexpected::…))`, e
/// `Unexpected` **e' costruito dal valore ricevuto**. Tre delle sue varianti lo
/// stampano — `Bool`, `Signed`/`Unsigned`, `Float` — quindi `{"commit_token":
/// 1234}` usciva come «invalid type: integer `1234`». Un token non e' un
/// numero, ma un token scritto per sbaglio senza virgolette **sì**, e quello e'
/// il valore vero in chiaro in un log.
///
/// Verificato una variante alla volta, non dedotto: `null`, le sequenze e le
/// mappe rendono «null», «sequence», «map» e non portano nulla; `Bytes` rende
/// «byte array»; `char` e le stringhe ricadono su [`VisitatoreToken::visit_str`]
/// e finiscono nella validazione vera, che non copia il testo. Restano i sei
/// qui sotto, e li' il valore non arriva perche' [`non_testuale`] non ha dove
/// metterlo.
impl de::Visitor<'_> for VisitatoreToken {
    type Value = CommitToken;

    /// Descrive la **forma attesa**, non quella ricevuta.
    ///
    /// E' l'unico messaggio che `serde` compone da solo, e questa e' la
    /// ragione per cui `visit_str` c'e' anche se il default lo genererebbe:
    /// senza, un token arriverebbe a `invalid_type` e finirebbe stampato
    /// dentro `Unexpected::Str`.
    fn expecting(&self, formattatore: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formattatore,
            "una stringa di {COMMIT_TOKEN_CARATTERI} caratteri esadecimali minuscoli"
        )
    }

    fn visit_str<E: de::Error>(self, testo: &str) -> Result<Self::Value, E> {
        // Il messaggio d'errore di `serde` **non** cita il testo: `custom`
        // riceve solo cio' che scriviamo noi, e `FormaTokenNonValida` non
        // porta il valore.
        CommitToken::da_esadecimale(testo).map_err(E::custom)
    }

    // Le larghezze minori ricadono per default su questi: `i8`/`i16`/`i32` su
    // `visit_i64`, `u8`/`u16`/`u32` su `visit_u64`, `f32` su `visit_f64`.
    // Coprirli qui copre l'intera famiglia.
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

    // I due a 128 bit non ricadono sui precedenti e oggi non stampano il
    // valore: lo dicono a parole («i128»). Sono qui perche' quel default e'
    // di `serde`, non nostro, e la riservatezza del token non deve dipendere
    // da come una dipendenza formatta un messaggio.
    fn visit_i128<E: de::Error>(self, _: i128) -> Result<Self::Value, E> {
        Err(non_testuale())
    }

    fn visit_u128<E: de::Error>(self, _: u128) -> Result<Self::Value, E> {
        Err(non_testuale())
    }
}

#[cfg(test)]
mod tests;
