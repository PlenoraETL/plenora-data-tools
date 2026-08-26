//! L'handshake: **macchina a stati pura**, senza processi ne' canali.
//!
//! # Che cosa rende impossibile, e cosa rifiuta
//!
//! La distinzione conta, perche' sono due garanzie di forza diversa.
//!
//! **Impossibili** — non compilano:
//!
//! - riusare uno stato concluso: ogni transizione **consuma** `self`, quindi
//!   un supervisore che ha gia' ricevuto la `Risposta` non esiste piu';
//! - chiedere un `Incarico` prima dell'accordo: il metodo sta su
//!   [`WorkerAccordato`], che nasce solo dalla verifica riuscita.
//!
//! **Rifiutati esplicitamente** — perche' arrivano dal filo e nessun tipo puo'
//! impedirli:
//!
//! - una `Risposta` che arriva al posto del `Saluto`, o viceversa;
//! - un messaggio nella direzione sbagliata;
//! - un `Incarico` prima che l'accordo sia concluso;
//! - qualunque altro tipo fuori sequenza.
//!
//! # Il confronto e' esatto, e la rappresentazione e' canonica
//!
//! Risorse, backend e capability sono **insiemi**, non liste: due handshake
//! che elencano le stesse risorse in ordine diverso descrivono lo stesso
//! ambiente e devono accordarsi. Un confronto posizionale li avrebbe fatti
//! divergere per un dettaglio di costruzione, e — peggio — avrebbe reso
//! l'esito dipendente dall'ordine in cui qualcuno ha riempito un `Vec`.
//!
//! I duplicati invece si **rifiutano**, non si riducono: due risorse con lo
//! stesso nome sono un'ambiguita' su quale verra' aperta, e sceglierne una e'
//! sceglierla arbitrariamente.
//!
//! # Che cosa NON c'e'
//!
//! Nessun processo, nessun pipe, nessuna scoperta dell'ambiente della
//! macchina. La descrizione locale **arriva**: qui si verifica che due
//! descrizioni concordino, non si va a vedere com'e' fatto l'host.

use std::collections::BTreeSet;

use plenora_core::{PlenoraError, Result};

use crate::commit_token::CommitToken;

use super::codifica::MAX_PROTOCOL_FRAME_BYTES;
use super::limiti::{
    MAX_MESSAGGI_VERSO_SUPERVISORE, MAX_MESSAGGI_VERSO_WORKER, MAX_PIANO_CANONICO_BYTES,
};
use super::messaggi::{
    Ambiente, BackendDinamico, Corpo, Frame, IdentitaArtefatto, IdentitaResolver, Incarico,
    LimitiDichiarati, RisorsaRisolta, Risposta, Saluto, TipoMessaggio,
};

// ---------------------------------------------------------------------------
// I limiti, da un'autorita' sola
// ---------------------------------------------------------------------------

/// I limiti che questo binario applica davvero.
///
/// Costruiti dalle costanti e non riscritti a mano: un `LimitiDichiarati`
/// compilato con numeri diversi da quelli applicati sarebbe una promessa
/// falsa, e l'altro capo la accetterebbe.
#[must_use]
pub const fn limiti_correnti() -> LimitiDichiarati {
    LimitiDichiarati {
        max_frame_bytes: MAX_PROTOCOL_FRAME_BYTES as u64,
        max_piano_canonico_bytes: MAX_PIANO_CANONICO_BYTES as u64,
        max_messaggi_verso_worker: MAX_MESSAGGI_VERSO_WORKER as u64,
        max_messaggi_verso_supervisore: MAX_MESSAGGI_VERSO_SUPERVISORE as u64,
    }
}

// ---------------------------------------------------------------------------
// Le descrizioni che i due lati portano
// ---------------------------------------------------------------------------

/// Cio' che un lato sa di se stesso.
///
/// **Non** viene scoperto qui: e' uno snapshot tipizzato che arriva da fuori.
/// La scoperta dell'ambiente della macchina e' un'altra cosa e un'altra PR;
/// mescolarla con la verifica avrebbe reso impossibile provare la verifica
/// senza una macchina vera sotto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescrizioneLocale {
    /// Identita' dell'eseguibile.
    pub artefatto: IdentitaArtefatto,
    /// Identita' del resolver CRS.
    pub resolver: IdentitaResolver,
    /// Ambiente risolto.
    pub ambiente: Ambiente,
    /// Capability offerte.
    pub capability: Vec<String>,
}

/// Cio' che il supervisore pretende dall'altro lato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtteseSupervisore {
    /// La propria descrizione, che il worker deve rispecchiare.
    pub locale: DescrizioneLocale,
    /// Le capability senza le quali l'incarico non si puo' dare.
    pub capability_richieste: Vec<String>,
    /// Il token che sigillera' l'artefatto.
    pub commit_token: CommitToken,
}

// ---------------------------------------------------------------------------
// Forma canonica di cio' che e' un insieme
// ---------------------------------------------------------------------------

/// Un ambiente ridotto alla propria forma canonica.
///
/// Ordine deterministico e duplicati gia' rifiutati: da qui in poi il
/// confronto e' l'uguaglianza, e non c'e' spazio perche' due descrizioni
/// equivalenti risultino diverse.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AmbienteCanonico {
    digest_insieme: String,
    acquisizione_dinamica: bool,
    risorse: Vec<RisorsaRisolta>,
    backend_dinamici: Vec<BackendDinamico>,
}

fn ambiente_canonico(ambiente: &Ambiente, lato: &str) -> Result<AmbienteCanonico> {
    let mut risorse = ambiente.risorse.clone();
    risorse.sort_by(|sinistra, destra| {
        (&sinistra.nome, &sinistra.versione, &sinistra.percorso).cmp(&(
            &destra.nome,
            &destra.versione,
            &destra.percorso,
        ))
    });
    rifiuta_nomi_ripetuti(risorse.iter().map(|r| r.nome.as_str()), lato, "risorsa")?;

    let mut backend_dinamici = ambiente.backend_dinamici.clone();
    backend_dinamici.sort_by(|sinistra, destra| {
        (&sinistra.nome, &sinistra.versione, &sinistra.percorso).cmp(&(
            &destra.nome,
            &destra.versione,
            &destra.percorso,
        ))
    });
    rifiuta_nomi_ripetuti(
        backend_dinamici.iter().map(|b| b.nome.as_str()),
        lato,
        "backend dinamico",
    )?;

    Ok(AmbienteCanonico {
        digest_insieme: ambiente.digest_insieme.clone(),
        acquisizione_dinamica: ambiente.acquisizione_dinamica,
        risorse,
        backend_dinamici,
    })
}

/// Due voci con lo stesso nome sono un'ambiguita', non una ridondanza.
///
/// Il nome e' l'identita' con cui la risorsa verra' cercata: se ne esistono
/// due, «quale si apre» non ha una risposta, e ridurle a una sceglierebbe al
/// posto di chi ha costruito l'ambiente.
fn rifiuta_nomi_ripetuti<'a>(
    nomi: impl Iterator<Item = &'a str>,
    lato: &str,
    genere: &str,
) -> Result<()> {
    let mut visti: BTreeSet<&str> = BTreeSet::new();
    for nome in nomi {
        if !visti.insert(nome) {
            return Err(PlenoraError::InvalidConfiguration(format!(
                "{lato}: {genere} `{nome}` dichiarato due volte; \
                 quale delle due si apra non ha una risposta"
            )));
        }
    }
    Ok(())
}

/// Le capability in forma canonica: ordinate e senza ripetizioni.
fn capability_canoniche(capability: &[String], lato: &str) -> Result<Vec<String>> {
    let mut ordinate = capability.to_vec();
    ordinate.sort();
    rifiuta_nomi_ripetuti(ordinate.iter().map(String::as_str), lato, "capability")?;
    Ok(ordinate)
}

// ---------------------------------------------------------------------------
// I confronti, uno per asse
// ---------------------------------------------------------------------------

/// L'identita' dell'artefatto e' `Protocol`: se i due binari non sono lo
/// stesso, non stanno parlando la stessa lingua, e non e' una questione di
/// configurazione.
fn confronta_artefatto(atteso: &IdentitaArtefatto, ricevuto: &IdentitaArtefatto) -> Result<()> {
    if atteso.digest != ricevuto.digest {
        return Err(PlenoraError::Protocol(
            "il digest dell'artefatto non coincide: i due lati non sono lo stesso binario"
                .to_owned(),
        ));
    }
    if atteso.versione != ricevuto.versione {
        return Err(PlenoraError::Protocol(format!(
            "la versione dell'artefatto non coincide: attesa `{}`, ricevuta `{}`",
            atteso.versione, ricevuto.versione
        )));
    }
    Ok(())
}

/// Il resolver e' `InvalidConfiguration`: due binari identici possono
/// risolvere CRS diversi, ed e' una scelta di configurazione sbagliata, non un
/// protocollo violato.
fn confronta_resolver(atteso: &IdentitaResolver, ricevuto: &IdentitaResolver) -> Result<()> {
    if atteso.identita != ricevuto.identita {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "il resolver CRS non coincide: atteso `{}`, ricevuto `{}`",
            atteso.identita, ricevuto.identita
        )));
    }
    if atteso.versione != ricevuto.versione {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "la versione del resolver non coincide: attesa `{}`, ricevuta `{}`",
            atteso.versione, ricevuto.versione
        )));
    }
    Ok(())
}

fn confronta_ambiente(atteso: &Ambiente, ricevuto: &Ambiente) -> Result<()> {
    // L'acquisizione dinamica si controlla su **entrambi** e prima di tutto:
    // un backend che scarica una griglia a meta' esecuzione rende il digest
    // una fotografia scaduta, e allora confrontare i digest non dice piu'
    // nulla.
    for (lato, ambiente) in [("supervisore", atteso), ("worker", ricevuto)] {
        if ambiente.acquisizione_dinamica {
            return Err(PlenoraError::InvalidConfiguration(format!(
                "{lato}: `acquisizione_dinamica` e' vera; l'insieme delle risorse \
                 non sarebbe immutabile e il suo digest non descriverebbe piu' \
                 cio' che verra' aperto"
            )));
        }
    }

    let atteso = ambiente_canonico(atteso, "supervisore")?;
    let ricevuto = ambiente_canonico(ricevuto, "worker")?;

    if atteso.digest_insieme != ricevuto.digest_insieme {
        return Err(PlenoraError::InvalidConfiguration(
            "il digest dell'insieme delle risorse non coincide: i due lati non \
             vedono lo stesso insieme immutabile"
                .to_owned(),
        ));
    }
    if atteso.risorse != ricevuto.risorse {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "le risorse risolte non coincidono: {} attese, {} ricevute",
            atteso.risorse.len(),
            ricevuto.risorse.len()
        )));
    }
    if atteso.backend_dinamici != ricevuto.backend_dinamici {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "i backend dinamici non coincidono: {} attesi, {} ricevuti",
            atteso.backend_dinamici.len(),
            ricevuto.backend_dinamici.len()
        )));
    }
    Ok(())
}

/// Le capability richieste devono esserci **tutte**.
///
/// Non uguaglianza: un worker che ne offre di piu' va benissimo. Cio' che non
/// va bene e' che ne manchi una, perche' l'incarico la userebbe.
fn confronta_capability(richieste: &[String], offerte: &[String]) -> Result<()> {
    let richieste = capability_canoniche(richieste, "supervisore")?;
    let offerte = capability_canoniche(offerte, "worker")?;
    let disponibili: BTreeSet<&str> = offerte.iter().map(String::as_str).collect();
    let mancanti: Vec<&str> = richieste
        .iter()
        .map(String::as_str)
        .filter(|nome| !disponibili.contains(nome))
        .collect();
    if !mancanti.is_empty() {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "il worker non offre {} capability richieste: {}",
            mancanti.len(),
            mancanti.join(", ")
        )));
    }
    Ok(())
}

/// I limiti sono `Protocol`: un disaccordo qui significa che i due lati
/// applicano regole diverse allo stesso canale.
fn confronta_limiti(dichiarati: &LimitiDichiarati) -> Result<()> {
    let miei = limiti_correnti();
    if *dichiarati == miei {
        return Ok(());
    }
    let mut divergenti: Vec<String> = Vec::new();
    for (nome, mio, suo) in [
        (
            "max_frame_bytes",
            miei.max_frame_bytes,
            dichiarati.max_frame_bytes,
        ),
        (
            "max_piano_canonico_bytes",
            miei.max_piano_canonico_bytes,
            dichiarati.max_piano_canonico_bytes,
        ),
        (
            "max_messaggi_verso_worker",
            miei.max_messaggi_verso_worker,
            dichiarati.max_messaggi_verso_worker,
        ),
        (
            "max_messaggi_verso_supervisore",
            miei.max_messaggi_verso_supervisore,
            dichiarati.max_messaggi_verso_supervisore,
        ),
    ] {
        if mio != suo {
            divergenti.push(format!("`{nome}`: applico {mio}, dichiarato {suo}"));
        }
    }
    Err(PlenoraError::Protocol(format!(
        "i limiti del protocollo non coincidono ({})",
        divergenti.join("; ")
    )))
}

// ---------------------------------------------------------------------------
// Estrazione dei corpi, con la direzione controllata
// ---------------------------------------------------------------------------

/// Il messaggio arrivato non e' quello che questo stato aspetta.
fn fuori_sequenza(atteso: TipoMessaggio, arrivato: TipoMessaggio) -> PlenoraError {
    PlenoraError::Protocol(format!("atteso `{atteso:?}`, ricevuto `{arrivato:?}`"))
}

/// La direzione e' una proprieta' del tipo, e va controllata **prima** del
/// contenuto.
///
/// Un `Saluto` che arriva al supervisore non e' un `Saluto` malformato: e' un
/// messaggio che viaggia dalla parte sbagliata, e dirlo cosi' manda chi indaga
/// a guardare il posto giusto.
fn verifica_direzione(frame: &Frame, attesa: super::messaggi::Direzione) -> Result<()> {
    let sua = frame.tipo().direzione();
    if sua == attesa {
        return Ok(());
    }
    Err(PlenoraError::Protocol(format!(
        "`{:?}` viaggia verso {:?}, ma qui si ricevono solo messaggi verso {attesa:?}",
        frame.tipo(),
        sua
    )))
}

// ---------------------------------------------------------------------------
// Il supervisore
// ---------------------------------------------------------------------------

/// Il supervisore che ha emesso il `Saluto` e aspetta la `Risposta`.
///
/// Non ha `Clone`, e ogni transizione consuma `self`: uno stato concluso non
/// e' riusabile perche' non esiste piu'.
#[derive(Debug)]
pub struct SupervisoreInAttesa {
    attese: AtteseSupervisore,
    saluto: Saluto,
}

impl SupervisoreInAttesa {
    /// Costruisce il `Saluto` e si mette in attesa.
    ///
    /// I limiti non sono un parametro: vengono da [`limiti_correnti`], cosi'
    /// nessuno puo' dichiararne di diversi da quelli che applica.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::InvalidConfiguration`] se la propria descrizione non e'
    /// coerente — duplicati fra risorse, backend o capability, oppure
    /// acquisizione dinamica dichiarata.
    pub fn nuovo(attese: AtteseSupervisore) -> Result<Self> {
        // La propria descrizione si valida **prima** di spedirla: dichiarare
        // un ambiente ambiguo e scoprirlo dalla risposta dell'altro sarebbe
        // scoprire dall'esterno un difetto proprio.
        let _ = ambiente_canonico(&attese.locale.ambiente, "supervisore")?;
        let _ = capability_canoniche(&attese.capability_richieste, "supervisore")?;
        if attese.locale.ambiente.acquisizione_dinamica {
            return Err(PlenoraError::InvalidConfiguration(
                "supervisore: `acquisizione_dinamica` e' vera; l'insieme delle \
                 risorse non sarebbe immutabile"
                    .to_owned(),
            ));
        }
        let saluto = Saluto {
            artefatto: attese.locale.artefatto.clone(),
            resolver: attese.locale.resolver.clone(),
            ambiente: attese.locale.ambiente.clone(),
            commit_token: attese.commit_token,
            limiti: limiti_correnti(),
        };
        Ok(Self { attese, saluto })
    }

    /// Il `Saluto` da spedire.
    #[must_use]
    pub const fn saluto(&self) -> &Saluto {
        &self.saluto
    }

    /// Riceve un frame e, se e' la `Risposta` attesa e concorda, conclude
    /// l'accordo.
    ///
    /// Consuma `self`: un secondo messaggio non ha piu' uno stato in cui
    /// arrivare.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::Protocol`] per direzione sbagliata, tipo fuori
    /// sequenza, identita' dell'artefatto o limiti incompatibili;
    /// [`PlenoraError::InvalidConfiguration`] per resolver, ambiente o
    /// capability incompatibili.
    pub fn ricevi(self, frame: Frame) -> Result<HandshakeAccettato> {
        verifica_direzione(&frame, super::messaggi::Direzione::VersoSupervisore)?;
        let tipo = frame.tipo();
        // Il frame si **consuma**: chi lo ha ricevuto lo ha esaurito, e
        // tenerlo in vita dopo inviterebbe a rileggerlo.
        let Corpo::Risposta(risposta) = frame.in_corpo() else {
            return Err(fuori_sequenza(TipoMessaggio::Risposta, tipo));
        };

        confronta_artefatto(&self.attese.locale.artefatto, &risposta.artefatto)?;
        confronta_resolver(&self.attese.locale.resolver, &risposta.resolver)?;
        confronta_ambiente(&self.attese.locale.ambiente, &risposta.ambiente)?;
        confronta_capability(&self.attese.capability_richieste, &risposta.capability)?;

        Ok(HandshakeAccettato {
            commit_token: self.attese.commit_token,
        })
    }
}

/// L'accordo concluso.
///
/// Esiste **solo** come risultato di una verifica riuscita: non c'e' un
/// costruttore che lo produca altrimenti, quindi non si puo' avere in mano
/// senza aver fatto l'handshake.
#[derive(Debug)]
pub struct HandshakeAccettato {
    commit_token: CommitToken,
}

impl HandshakeAccettato {
    /// Il token su cui i due lati si sono accordati.
    #[must_use]
    pub const fn commit_token(&self) -> &CommitToken {
        &self.commit_token
    }
}

// ---------------------------------------------------------------------------
// Il worker
// ---------------------------------------------------------------------------

/// Il worker che aspetta il `Saluto`.
#[derive(Debug)]
pub struct WorkerInAttesa {
    locale: DescrizioneLocale,
}

impl WorkerInAttesa {
    /// Un worker che si descrive cosi'.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::InvalidConfiguration`] se la propria descrizione e'
    /// ambigua.
    pub fn nuovo(locale: DescrizioneLocale) -> Result<Self> {
        let _ = ambiente_canonico(&locale.ambiente, "worker")?;
        let _ = capability_canoniche(&locale.capability, "worker")?;
        Ok(Self { locale })
    }

    /// Riceve un frame; se e' il `Saluto` e concorda, produce la `Risposta` e
    /// lo stato accordato.
    ///
    /// Un `Incarico` che arrivi qui e' rifiutato **esplicitamente**: e' il
    /// caso «incarico prima dell'accordo», e va detto con quel nome invece di
    /// somigliare a un messaggio malformato.
    ///
    /// # Errors
    ///
    /// Come [`SupervisoreInAttesa::ricevi`], piu' il caso dell'`Incarico`
    /// anticipato.
    pub fn ricevi(self, frame: Frame) -> Result<(Risposta, WorkerAccordato)> {
        verifica_direzione(&frame, super::messaggi::Direzione::VersoWorker)?;
        let tipo = frame.tipo();
        if tipo == TipoMessaggio::Incarico {
            return Err(PlenoraError::Protocol(
                "`Incarico` ricevuto prima dell'accordo: l'handshake non e' \
                 concluso, e nulla di cio' che l'incarico dichiara e' ancora \
                 stato verificato"
                    .to_owned(),
            ));
        }
        let Corpo::Saluto(saluto) = frame.in_corpo() else {
            return Err(fuori_sequenza(TipoMessaggio::Saluto, tipo));
        };

        // I limiti per primi: se i due lati non applicano le stesse regole al
        // canale, il resto del confronto avviene su un canale su cui non si e'
        // d'accordo.
        confronta_limiti(&saluto.limiti)?;
        confronta_artefatto(&self.locale.artefatto, &saluto.artefatto)?;
        confronta_resolver(&self.locale.resolver, &saluto.resolver)?;
        confronta_ambiente(&saluto.ambiente, &self.locale.ambiente)?;

        // `self` e' gia' consumato: la descrizione locale si **sposta** nella
        // risposta invece di essere clonata.
        let DescrizioneLocale {
            artefatto,
            resolver,
            ambiente,
            capability,
        } = self.locale;
        let risposta = Risposta {
            artefatto,
            resolver,
            ambiente,
            capability,
        };
        let accordato = WorkerAccordato {
            commit_token: saluto.commit_token,
        };
        Ok((risposta, accordato))
    }
}

/// Il worker dopo l'accordo: da qui, e solo da qui, puo' ricevere l'`Incarico`.
#[derive(Debug)]
pub struct WorkerAccordato {
    commit_token: CommitToken,
}

impl WorkerAccordato {
    /// Il token accettato nel `Saluto`.
    #[must_use]
    pub const fn commit_token(&self) -> &CommitToken {
        &self.commit_token
    }

    /// Riceve l'`Incarico`.
    ///
    /// Consuma `self`: un secondo `Incarico` non ha uno stato in cui arrivare.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::Protocol`] per direzione sbagliata o tipo fuori
    /// sequenza. `PR-5` non verifica il contenuto dell'incarico: il piano, il
    /// suo hash e i contratti d'ingresso appartengono all'esecuzione.
    pub fn ricevi_incarico(self, frame: Frame) -> Result<(Incarico, CommitToken)> {
        verifica_direzione(&frame, super::messaggi::Direzione::VersoWorker)?;
        let tipo = frame.tipo();
        let Corpo::Incarico(incarico) = frame.in_corpo() else {
            return Err(fuori_sequenza(TipoMessaggio::Incarico, tipo));
        };
        Ok((*incarico, self.commit_token))
    }
}

#[cfg(test)]
mod tests;
