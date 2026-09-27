//! L'handshake: **macchina a stati pura**, senza processi ne' canali.
//!
//! # Impossibili e rifiutati
//!
//! **Impossibili**, non compilano: riusare uno stato concluso (ogni
//! transizione **consuma** `self`) e chiedere un `Incarico` prima
//! dell'accordo (il metodo sta su [`WorkerAccordato`], che nasce solo dalla
//! verifica riuscita).
//!
//! **Rifiutati esplicitamente**, perche' arrivano dal filo e nessun tipo puo'
//! impedirli: messaggi nella direzione sbagliata o fuori sequenza, compreso
//! un `Incarico` prima dell'accordo.
//!
//! # Il confronto
//!
//! Risorse, backend e capability sono **insiemi**: lo stesso ambiente
//! elencato in ordine diverso si accorda. I duplicati si **rifiutano**, non si
//! riducono: sceglierne uno sarebbe arbitrario.
//!
//! La descrizione locale **arriva** da fuori: qui si verifica che due
//! descrizioni concordino, senza processi, pipe o scoperta dell'host.

use plenora_core::{PlenoraError, Result};

use crate::commit_token::CommitToken;

use super::codifica::MAX_PROTOCOL_FRAME_BYTES;
use super::limiti::{
    MAX_MESSAGGI_VERSO_SUPERVISORE, MAX_MESSAGGI_VERSO_WORKER, MAX_PIANO_CANONICO_BYTES,
};
use super::messaggi::{
    Ambiente, Corpo, Frame, IdentitaArtefatto, IdentitaResolver, Incarico, IncaricoVerifica,
    LimitiDichiarati, Risposta, Saluto, TipoMessaggio,
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

/// Cio' che i due lati devono **rispecchiare identico**.
///
/// Un tipo solo per i due lati: con due, un campo aggiunto a uno smetterebbe
/// silenziosamente di essere confrontato.
///
/// **Non** viene scoperta qui: e' uno snapshot tipizzato che arriva da fuori,
/// cosi' la verifica si prova senza una macchina vera sotto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descrizione {
    /// Identita' dell'eseguibile.
    pub artefatto: IdentitaArtefatto,
    /// Identita' del resolver CRS.
    pub resolver: IdentitaResolver,
    /// Ambiente risolto.
    pub ambiente: Ambiente,
}

/// Cio' che il **worker** sa di se stesso: la descrizione comune, piu' le
/// capability che **offre**.
///
/// Le capability stanno qui e non in [`Descrizione`] perche' sono l'unico asse
/// asimmetrico: il worker le offre, il supervisore le richiede.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescrizioneLocale {
    /// Cio' che il supervisore deve rispecchiare.
    pub comune: Descrizione,
    /// Capability offerte.
    pub capability: Vec<String>,
}

/// Cio' che il supervisore pretende dall'altro lato.
// Lato supervisore: lo raggiunge il chiamante di produzione del profilo isolato
// (`isolamento::esecuzione_isolata`), oltre ai casi e al percorso di
// qualificazione.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtteseSupervisore {
    /// Cio' che il worker deve rispecchiare identico.
    pub comune: Descrizione,
    /// Le capability senza le quali l'incarico non si puo' dare.
    ///
    /// Confronto **asimmetrico**: il worker puo' offrirne di piu', non di
    /// meno. E' l'unico asse su cui i due lati non devono coincidere.
    pub capability_richieste: Vec<String>,
    /// Il token che identifichera' il tentativo nel footer dell'artefatto.
    pub commit_token: CommitToken,
}

// ---------------------------------------------------------------------------
// La forma canonica, prodotta una volta sola
// ---------------------------------------------------------------------------

/// Un ambiente **gia'** ordinato e senza ripetizioni.
///
/// Si entra solo da [`AmbienteCanonico::da`], che ordina e rifiuta: da qui in
/// poi il confronto e' l'uguaglianza. La riduzione avviene **una volta**, alla
/// costruzione dello stato, e la forma canonica e' anche quella spedita.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AmbienteCanonico(Ambiente);

impl AmbienteCanonico {
    /// **Consuma** l'ambiente: ordina gli insiemi e rifiuta le ambiguita'.
    fn da(mut ambiente: Ambiente, lato: &str) -> Result<Self> {
        // `acquisizione_dinamica` e' una proprieta' della descrizione, non del
        // confronto: un ambiente che puo' cambiare sotto i piedi non ha una
        // forma canonica, quindi non ne esce una.
        if ambiente.acquisizione_dinamica {
            return Err(PlenoraError::InvalidConfiguration(format!(
                "{lato}: `acquisizione_dinamica` e' vera; l'insieme delle risorse \
                 non sarebbe immutabile e il suo digest non descriverebbe piu' \
                 cio' che verra' aperto"
            )));
        }
        ambiente.risorse.sort_by(|sinistra, destra| {
            (&sinistra.nome, &sinistra.versione, &sinistra.percorso).cmp(&(
                &destra.nome,
                &destra.versione,
                &destra.percorso,
            ))
        });
        rifiuta_nomi_ripetuti(&ambiente.risorse, |r| &r.nome, lato, "risorsa")?;

        ambiente.backend_dinamici.sort_by(|sinistra, destra| {
            (&sinistra.nome, &sinistra.versione, &sinistra.percorso).cmp(&(
                &destra.nome,
                &destra.versione,
                &destra.percorso,
            ))
        });
        rifiuta_nomi_ripetuti(
            &ambiente.backend_dinamici,
            |b| &b.nome,
            lato,
            "backend dinamico",
        )?;
        Ok(Self(ambiente))
    }

    /// L'ambiente canonico, per confrontarlo.
    const fn come_ambiente(&self) -> &Ambiente {
        &self.0
    }

    /// L'ambiente canonico, per spedirlo.
    fn in_ambiente(self) -> Ambiente {
        self.0
    }
}

/// Una descrizione validata e ridotta, **una volta sola**.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DescrizioneCanonica {
    artefatto: IdentitaArtefatto,
    resolver: IdentitaResolver,
    ambiente: AmbienteCanonico,
}

impl DescrizioneCanonica {
    /// **Consuma** la descrizione: ne verifica la forma e la riduce.
    ///
    /// La verifica di forma e' quella del decoder, chiamata qui e non
    /// riscritta: due controlli separati per i due versi divergerebbero, e il
    /// verso piu' debole deciderebbe che cosa passa.
    fn da(descrizione: Descrizione, lato: &str) -> Result<Self> {
        super::codifica::verifica_identita(&descrizione.artefatto, &descrizione.resolver)?;
        super::codifica::verifica_ambiente(&descrizione.ambiente)?;
        Ok(Self {
            artefatto: descrizione.artefatto,
            resolver: descrizione.resolver,
            ambiente: AmbienteCanonico::da(descrizione.ambiente, lato)?,
        })
    }
}

/// Le capability in forma canonica: verificate, ordinate, senza ripetizioni.
///
/// **Consuma** l'elenco. Stessa verifica per offerte e richieste: un nome
/// vuoto, o un elenco oltre cio' che una `Risposta` ammette, non si puo'
/// soddisfare.
fn capability_canoniche(mut capability: Vec<String>, lato: &str) -> Result<Vec<String>> {
    super::codifica::verifica_capability(&capability)?;
    capability.sort();
    rifiuta_nomi_ripetuti(&capability, String::as_str, lato, "capability")?;
    Ok(capability)
}

/// Due voci con lo stesso nome sono un'ambiguita', non una ridondanza.
///
/// Ridurle a una sceglierebbe quale aprire al posto di chi ha costruito
/// l'ambiente. Pretende l'elenco **gia' ordinato**: i ripetuti sono adiacenti,
/// e il conteggio dei distinti (`voci - ripetuti`) e' esatto solo cosi'.
fn rifiuta_nomi_ripetuti<T>(
    ordinati: &[T],
    nome: impl Fn(&T) -> &str,
    lato: &str,
    genere: &str,
) -> Result<()> {
    let ripetuti = ordinati
        .windows(2)
        .filter(|coppia| nome(&coppia[0]) == nome(&coppia[1]))
        .count();
    if ripetuti != 0 {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "{lato}: {genere} dichiarata piu' volte con lo stesso nome \
             ({} voci, {} nomi distinti); quale si apra non ha una risposta",
            ordinati.len(),
            ordinati.len() - ripetuti
        )));
    }
    Ok(())
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
        return Err(PlenoraError::Protocol(
            "la versione dell'artefatto non coincide: i due lati non sono la \
             stessa build"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Il resolver e' `InvalidConfiguration`: due binari identici possono
/// risolvere CRS diversi, ed e' una scelta di configurazione sbagliata, non un
/// protocollo violato.
fn confronta_resolver(atteso: &IdentitaResolver, ricevuto: &IdentitaResolver) -> Result<()> {
    if atteso.identita != ricevuto.identita {
        return Err(PlenoraError::InvalidConfiguration(
            "il resolver CRS non coincide sull'identita'".to_owned(),
        ));
    }
    if atteso.versione != ricevuto.versione {
        return Err(PlenoraError::InvalidConfiguration(
            "il resolver CRS non coincide sulla versione".to_owned(),
        ));
    }
    Ok(())
}

fn confronta_ambiente(atteso: &AmbienteCanonico, ricevuto: &AmbienteCanonico) -> Result<()> {
    // Le due forme sono gia' canoniche: niente ordinamento ne' clone.
    // `acquisizione_dinamica` non si ricontrolla: un ambiente che la dichiara
    // non diventa mai un `AmbienteCanonico`.
    let atteso = atteso.come_ambiente();
    let ricevuto = ricevuto.come_ambiente();

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
/// Non uguaglianza: un worker puo' offrirne di piu'. Gli elenchi arrivano
/// **gia' ordinati**, quindi basta una scansione parallela, senza una terza
/// copia dei nomi che vengono dal filo.
// Lato supervisore: lo raggiunge il chiamante di produzione del profilo isolato
// (`isolamento::esecuzione_isolata`), oltre ai casi e al percorso di
// qualificazione.
fn confronta_capability(richieste: &[String], offerte: &[String]) -> Result<()> {
    let mut scorre = offerte.iter();
    let mut corrente = scorre.next();
    let mut mancanti = 0_usize;
    for richiesta in richieste {
        while corrente.is_some_and(|offerta| offerta < richiesta) {
            corrente = scorre.next();
        }
        if corrente == Some(richiesta) {
            corrente = scorre.next();
        } else {
            mancanti += 1;
        }
    }
    if mancanti != 0 {
        // Il conteggio e non i nomi: le richieste sono configurazione nostra,
        // ma le offerte arrivano dal filo, e un elenco di «mancanti» dice per
        // differenza che cosa l'altro capo ha dichiarato.
        return Err(PlenoraError::InvalidConfiguration(format!(
            "il worker non offre {mancanti} delle {} capability richieste",
            richieste.len()
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
            // Il **nome** del limite, non i due valori: quello dichiarato
            // arriva dal filo. Chi indaga ha i propri limiti a portata di
            // costante, e il frame ricevuto in mano.
            divergenti.push(format!("`{nome}`"));
        }
    }
    Err(PlenoraError::Protocol(format!(
        "i limiti del protocollo non coincidono su: {}",
        divergenti.join(", ")
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
// Lato supervisore: lo raggiunge il chiamante di produzione
// (`isolamento::esecuzione_isolata`), oltre ai casi e al percorso di
// qualificazione. La copia del `commit_token` resta sotto `internals`: la
// produzione legge la propria — vedi
// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
#[derive(Debug)]
pub struct SupervisoreInAttesa {
    /// Gia' ridotta: il confronto con la `Risposta` non deve rifare nulla.
    da_rispecchiare: DescrizioneCanonica,
    /// Gia' verificate e ordinate.
    capability_richieste: Vec<String>,
    /// Sopravvive solo per raggiungere [`HandshakeAccettato::commit_token`]:
    /// la copia che il protocollo usa davvero e' in `saluto`.
    #[cfg(any(test, feature = "internals"))]
    commit_token: CommitToken,
    saluto: Saluto,
}

impl SupervisoreInAttesa {
    /// Costruisce il `Saluto` e si mette in attesa.
    ///
    /// I limiti non sono un parametro: vengono da [`limiti_correnti`], cosi'
    /// nessuno puo' dichiararne di diversi da quelli che applica.
    ///
    /// Il `Saluto` porta la descrizione **canonica**: lo stesso ambiente
    /// dichiarato in ordine diverso produce lo stesso frame, byte per byte.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::InvalidConfiguration`] se cio' che il supervisore
    /// dichiara non e' coerente — campi vuoti, digest non canonici, duplicati
    /// fra risorse, backend o capability **richieste**, oppure acquisizione
    /// dinamica dichiarata.
    ///
    /// Le capability offerte non compaiono: [`AtteseSupervisore`] non le fa
    /// dichiarare.
    pub fn nuovo(attese: AtteseSupervisore) -> Result<Self> {
        // La propria descrizione si valida e si riduce **prima** di spedirla,
        // una volta sola: dichiarare un ambiente ambiguo e scoprirlo dalla
        // risposta dell'altro sarebbe scoprire dall'esterno un difetto
        // proprio.
        let da_rispecchiare = DescrizioneCanonica::da(attese.comune, "supervisore")?;
        let capability_richieste =
            capability_canoniche(attese.capability_richieste, "supervisore")?;
        let saluto = Saluto {
            artefatto: da_rispecchiare.artefatto.clone(),
            resolver: da_rispecchiare.resolver.clone(),
            ambiente: da_rispecchiare.ambiente.clone().in_ambiente(),
            commit_token: attese.commit_token,
            limiti: limiti_correnti(),
        };
        Ok(Self {
            da_rispecchiare,
            capability_richieste,
            #[cfg(any(test, feature = "internals"))]
            commit_token: attese.commit_token,
            saluto,
        })
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
        let Risposta {
            artefatto,
            resolver,
            ambiente,
            capability,
        } = *risposta;

        // La **forma** prima del confronto: un `Frame` si costruisce anche in
        // processo, senza `decodifica`, e un campo malformato confrontato per
        // primo uscirebbe come «non coincide». La riduzione consuma cio' che
        // e' arrivato, senza clonare.
        let ricevuta = DescrizioneCanonica::da(
            Descrizione {
                artefatto,
                resolver,
                ambiente,
            },
            "worker",
        )?;
        let offerte = capability_canoniche(capability, "worker")?;

        confronta_artefatto(&self.da_rispecchiare.artefatto, &ricevuta.artefatto)?;
        confronta_resolver(&self.da_rispecchiare.resolver, &ricevuta.resolver)?;
        confronta_ambiente(&self.da_rispecchiare.ambiente, &ricevuta.ambiente)?;
        confronta_capability(&self.capability_richieste, &offerte)?;

        Ok(HandshakeAccettato {
            #[cfg(any(test, feature = "internals"))]
            commit_token: self.commit_token,
        })
    }
}

/// L'accordo concluso.
///
/// Esiste **solo** come risultato di una verifica riuscita: averlo in mano e'
/// la prova che le due descrizioni concordano.
///
/// Lo produce `isolamento::esecuzione_isolata`, oltre ai casi e al percorso
/// di qualificazione. Il campo `commit_token` resta sotto `internals`: la
/// produzione consegna al verificatore la propria copia del token, e questa
/// la leggono solo i casi — vedi
/// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
#[derive(Debug)]
pub struct HandshakeAccettato {
    #[cfg(any(test, feature = "internals"))]
    commit_token: CommitToken,
}

impl HandshakeAccettato {
    /// Il token su cui i due lati si sono accordati.
    #[cfg(any(test, feature = "internals"))]
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
    /// Gia' ridotta, come dal lato supervisore e per la stessa ragione.
    locale: DescrizioneCanonica,
    /// Gia' verificate e ordinate.
    capability: Vec<String>,
}

impl WorkerInAttesa {
    /// Un worker che si descrive cosi'.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::InvalidConfiguration`] se la propria descrizione e'
    /// ambigua, incompleta o con digest non canonici.
    pub fn nuovo(locale: DescrizioneLocale) -> Result<Self> {
        Ok(Self {
            locale: DescrizioneCanonica::da(locale.comune, "worker")?,
            capability: capability_canoniche(locale.capability, "worker")?,
        })
    }

    /// Riceve un frame; se e' il `Saluto` e concorda, produce la `Risposta` e
    /// lo stato accordato.
    ///
    /// Un `Incarico` che arrivi qui e' rifiutato **esplicitamente** come
    /// «incarico prima dell'accordo». La `Risposta` porta la forma
    /// **canonica**, come il `Saluto`.
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
        let Saluto {
            artefatto,
            resolver,
            ambiente,
            commit_token,
            limiti,
        } = *saluto;

        // I limiti per primi: se i due lati non applicano le stesse regole al
        // canale, il resto del confronto avviene su un canale su cui non si e'
        // d'accordo.
        confronta_limiti(&limiti)?;
        // Poi la forma di cio' che e' arrivato, e solo dopo il confronto: la
        // ragione e' la stessa del lato supervisore, e vale anche qui perche'
        // un `Frame` puo' non essere passato dal decoder.
        let ricevuta = DescrizioneCanonica::da(
            Descrizione {
                artefatto,
                resolver,
                ambiente,
            },
            "supervisore",
        )?;
        confronta_artefatto(&self.locale.artefatto, &ricevuta.artefatto)?;
        confronta_resolver(&self.locale.resolver, &ricevuta.resolver)?;
        confronta_ambiente(&ricevuta.ambiente, &self.locale.ambiente)?;

        // `self` e' gia' consumato: la descrizione locale si **sposta** nella
        // risposta invece di essere clonata.
        let risposta = Risposta {
            artefatto: self.locale.artefatto,
            resolver: self.locale.resolver,
            ambiente: self.locale.ambiente.in_ambiente(),
            capability: self.capability,
        };
        let accordato = WorkerAccordato { commit_token };
        Ok((risposta, accordato))
    }
}

/// Il worker dopo l'accordo: da qui, e solo da qui, puo' ricevere l'`Incarico`.
#[derive(Debug)]
pub struct WorkerAccordato {
    /// Lo legge [`WorkerAccordato::ricevi_incarico`], che lo consegna a chi
    /// esegue: e' l'unica strada, e per questo il token non puo' arrivare
    /// all'artefatto senza passare dall'incarico.
    commit_token: CommitToken,
}

impl WorkerAccordato {
    /// Il token accettato nel `Saluto`.
    ///
    /// Chi esegue un piano riceve il token da [`Self::ricevi_incarico`],
    /// insieme all'incarico. Questa seconda porta serve ai casi che
    /// confrontano i due lati prima che un incarico esista, e al verificatore
    /// (`isolamento::verificatore`), che non riceve mai un `Incarico`: la sua
    /// fase e' [`Self::ricevi_incarico_verifica`], che porta il token nella
    /// propria firma.
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
    /// sequenza. L'handshake **non** verifica il contenuto dell'incarico:
    /// piano, hash e contratti d'ingresso li verifica chi lo esegue, una volta
    /// sola.
    pub fn ricevi_incarico(self, frame: Frame) -> Result<(Incarico, CommitToken)> {
        verifica_direzione(&frame, super::messaggi::Direzione::VersoWorker)?;
        let tipo = frame.tipo();
        let Corpo::Incarico(incarico) = frame.in_corpo() else {
            return Err(fuori_sequenza(TipoMessaggio::Incarico, tipo));
        };
        // La forma, per la stessa ragione del `Saluto` e della `Risposta`: il
        // frame puo' non essere passato dal decoder. Senza, un incarico con un
        // `plan_hash_atteso` che non e' un digest uscirebbe di qui intatto, e
        // a rifiutarlo sarebbe chi lo esegue — cioe' dopo.
        super::codifica::verifica_incarico(&incarico)?;
        Ok((*incarico, self.commit_token))
    }

    /// Riceve l'`IncaricoVerifica`: l'equivalente di [`Self::ricevi_incarico`]
    /// per il ruolo che rilegge invece di eseguire.
    ///
    /// Consuma `self`, per la stessa ragione: un secondo incarico non ha
    /// uno stato in cui arrivare.
    ///
    /// # Errors
    ///
    /// Come [`Self::ricevi_incarico`].
    pub fn ricevi_incarico_verifica(self, frame: Frame) -> Result<(IncaricoVerifica, CommitToken)> {
        verifica_direzione(&frame, super::messaggi::Direzione::VersoWorker)?;
        let tipo = frame.tipo();
        let Corpo::IncaricoVerifica(incarico) = frame.in_corpo() else {
            return Err(fuori_sequenza(TipoMessaggio::IncaricoVerifica, tipo));
        };
        // Stessa ragione di `ricevi_incarico`: il frame puo' non essere
        // passato dal decoder.
        super::codifica::verifica_incarico_verifica(&incarico)?;
        Ok((*incarico, self.commit_token))
    }
}

#[cfg(test)]
pub mod tests;
