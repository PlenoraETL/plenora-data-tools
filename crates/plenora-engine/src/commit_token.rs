//! Il `commit_token`: un tipo **chiuso**, non una stringa che sembra un token.
//!
//! # Perche' e' un tipo e non un `String`
//!
//! Il token attraversa quattro confini — il chiamante che lo fornisce,
//! l'handshake che lo trasmette, il writer che lo scrive nel footer, il
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
//! La rappresentazione e la sua lettura vengono da [`crate::esadecimale32`],
//! condivise col digest del protocollo, che ha la stessa forma. **Cio' che non
//! e' condiviso e' tutto il resto**: il significato, l'errore, e la regola qui
//! sotto.
//!
//! # Il valore non compare mai
//!
//! `Debug` e `Display` non lo mostrano, e nessun errore di questo modulo lo
//! contiene. La regola e' generale e non riguarda un potere del token: un
//! valore che l'altro capo del canale controlla non si copia nei log, perche'
//! il log lo conserva e lo diffonde insieme al motivo per cui qualcuno lo
//! stava guardando. E' anche la ragione per cui la primitiva condivisa non ha
//! ne' `Debug` ne' `Display`: averli la' avrebbe imposto una politica a chi
//! non la vuole.
//!
//! La serializzazione invece lo emette, e l'asimmetria e' voluta: sul filo
//! serve, in un log no.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::esadecimale32::{self, DaEsadecimale32, Esadecimale32, FormaNonValida};

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
    /// **In byte**, cioe' nella stessa misura che decide il rifiuto. Contarli
    /// in caratteri per il solo messaggio produce un assurdo: su 64 caratteri
    /// di cui alcuni multibyte uscirebbe «attesi 64, trovati 64», un rifiuto
    /// che si smentisce da solo e manda a cercare il difetto altrove. Sulla
    /// forma canonica, che e' ASCII, le due misure coincidono; divergono
    /// esattamente nel caso in cui il messaggio serve.
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
pub struct CommitToken(Esadecimale32);

impl CommitToken {
    /// Interpreta un testo come `commit_token`.
    ///
    /// # Errors
    ///
    /// [`FormaTokenNonValida`] se il testo non e' esattamente **64** caratteri
    /// esadecimali minuscoli.
    ///
    /// Il numero e' scritto e non collegato: la costante che lo definisce sta
    /// nella rappresentazione condivisa, che e' privata, e un link da una doc
    /// pubblica a un simbolo privato non si risolve.
    pub fn da_esadecimale(testo: &str) -> Result<Self, FormaTokenNonValida> {
        Esadecimale32::da_esadecimale(testo)
            .map(Self)
            .map_err(Self::da_forma_non_valida)
    }

    /// La forma canonica: 64 caratteri esadecimali minuscoli.
    ///
    /// Ricostruita dai byte e non conservata dal testo d'origine, cosi' non
    /// puo' esistere un `CommitToken` che rende una grafia diversa da quella
    /// che il footer riceverebbe.
    #[must_use]
    pub fn in_esadecimale(&self) -> String {
        self.0.in_esadecimale()
    }
}

impl DaEsadecimale32 for CommitToken {
    type Errore = FormaTokenNonValida;

    const NOME: &'static str = "il commit_token";

    fn da_esadecimale32(primitiva: Esadecimale32) -> Self {
        Self(primitiva)
    }

    /// La maiuscola resta un errore **suo**, e non si riunisce agli altri
    /// caratteri: e' la distinzione che questo tipo ha in API, ed e' la
    /// ragione per cui il difetto della primitiva la riporta separata.
    fn da_forma_non_valida(difetto: FormaNonValida) -> Self::Errore {
        match difetto {
            FormaNonValida::LunghezzaErrata { attesi, trovati } => {
                FormaTokenNonValida::LunghezzaErrata { attesi, trovati }
            }
            FormaNonValida::Maiuscolo { posizione } => {
                FormaTokenNonValida::MaiuscoloNonAmmesso { posizione }
            }
            FormaNonValida::NonEsadecimale { posizione } => {
                FormaTokenNonValida::NonEsadecimale { posizione }
            }
        }
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
        self.0.serializza(serializzatore)
    }
}

/// Accetta **solo** la forma canonica.
///
/// Un token maiuscolo o di lunghezza sbagliata non viene normalizzato: viene
/// rifiutato. Normalizzarlo significherebbe accettare due grafie per lo stesso
/// valore, e il footer le distinguerebbe comunque.
impl<'de> Deserialize<'de> for CommitToken {
    fn deserialize<D: Deserializer<'de>>(deserializzatore: D) -> Result<Self, D::Error> {
        esadecimale32::deserializza(deserializzatore)
    }
}

#[cfg(test)]
mod tests;
