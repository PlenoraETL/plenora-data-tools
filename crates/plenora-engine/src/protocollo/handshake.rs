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

use plenora_core::{PlenoraError, Result};

use crate::commit_token::CommitToken;

use super::codifica::MAX_PROTOCOL_FRAME_BYTES;
use super::limiti::{
    MAX_MESSAGGI_VERSO_SUPERVISORE, MAX_MESSAGGI_VERSO_WORKER, MAX_PIANO_CANONICO_BYTES,
};
use super::messaggi::{
    Ambiente, Corpo, Frame, IdentitaArtefatto, IdentitaResolver, Incarico, LimitiDichiarati,
    Risposta, Saluto, TipoMessaggio,
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
/// Una sola, condivisa: un tipo per lato avrebbe significato due elenchi di
/// campi da tenere allineati a mano, e il giorno che uno dei due ne acquista
/// uno il disallineamento non lo dice nessuno — l'handshake confronta cio' che
/// i due tipi hanno in comune, e cio' che non ce l'hanno smette
/// silenziosamente di essere confrontato.
///
/// **Non** viene scoperta qui: e' uno snapshot tipizzato che arriva da fuori.
/// La scoperta dell'ambiente della macchina e' un'altra cosa e un'altra PR;
/// mescolarla con la verifica avrebbe reso impossibile provare la verifica
/// senza una macchina vera sotto.
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
/// asimmetrico: il worker le offre, il supervisore le richiede, e i due
/// insiemi non hanno lo stesso significato. Metterle nel tipo comune avrebbe
/// dato al supervisore un campo da riempire che nessuno guarda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescrizioneLocale {
    /// Cio' che il supervisore deve rispecchiare.
    pub comune: Descrizione,
    /// Capability offerte.
    pub capability: Vec<String>,
}

/// Cio' che il supervisore pretende dall'altro lato.
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
/// L'invariante e' del tipo, non di chi lo usa: si entra solo da
/// [`AmbienteCanonico::da`], che ordina e rifiuta. Da qui in poi il confronto
/// e' l'uguaglianza, e non c'e' spazio perche' due descrizioni equivalenti
/// risultino diverse.
///
/// La riduzione avviene **una volta**, alla costruzione dello stato, e non
/// dentro ogni confronto su un clone: farla li' costerebbe due ordinamenti e
/// due copie per handshake, butterebbe via la forma ridotta subito dopo, e
/// lascerebbe viaggiare quella d'origine. Cosi' invece la forma canonica e'
/// anche quella che viene spedita.
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
/// **Consuma** l'elenco. La verifica e' la stessa per quelle offerte e per
/// quelle richieste: un nome vuoto non e' offribile ne' richiedibile, e un
/// elenco piu' lungo di quanto una `Risposta` ne ammetta e' un'attesa che
/// nessun worker conforme puo' soddisfare.
fn capability_canoniche(mut capability: Vec<String>, lato: &str) -> Result<Vec<String>> {
    super::codifica::verifica_capability(&capability)?;
    capability.sort();
    rifiuta_nomi_ripetuti(&capability, String::as_str, lato, "capability")?;
    Ok(capability)
}

/// Due voci con lo stesso nome sono un'ambiguita', non una ridondanza.
///
/// Il nome e' l'identita' con cui la risorsa verra' cercata: se ne esistono
/// due, «quale si apre» non ha una risposta, e ridurle a una sceglierebbe al
/// posto di chi ha costruito l'ambiente.
///
/// Pretende l'elenco **gia' ordinato**, e li' i ripetuti sono adiacenti: un
/// secondo insieme costruito apposta direbbe la stessa cosa allocando. Il
/// conteggio dei distinti resta esatto proprio perche' l'ordine lo garantisce
/// — `voci - ripetuti` e' vero solo su un elenco ordinato, ed e' il motivo per
/// cui questa funzione non si puo' chiamare prima di aver ordinato.
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
    // Nessun ordinamento e nessun clone: le due forme sono gia' canoniche,
    // e ridurle qui significherebbe rifarlo a ogni confronto su copie che
    // vengono buttate via subito dopo. `acquisizione_dinamica` non si
    // ricontrolla per la stessa ragione: un ambiente che la dichiara non
    // diventa mai un `AmbienteCanonico`.
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
/// Non uguaglianza: un worker che ne offre di piu' va benissimo. Cio' che non
/// va bene e' che ne manchi una, perche' l'incarico la userebbe.
///
/// Entrambi gli elenchi arrivano **gia' ordinati**, quindi l'appartenenza si
/// decide con una scansione parallela invece che costruendo un insieme:
/// l'insieme sarebbe una terza copia dei nomi, e i nomi vengono dal filo.
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
#[derive(Debug)]
pub struct SupervisoreInAttesa {
    /// Gia' ridotta: il confronto con la `Risposta` non deve rifare nulla.
    da_rispecchiare: DescrizioneCanonica,
    /// Gia' verificate e ordinate.
    capability_richieste: Vec<String>,
    commit_token: CommitToken,
    saluto: Saluto,
}

impl SupervisoreInAttesa {
    /// Costruisce il `Saluto` e si mette in attesa.
    ///
    /// I limiti non sono un parametro: vengono da [`limiti_correnti`], cosi'
    /// nessuno puo' dichiararne di diversi da quelli che applica.
    ///
    /// Il `Saluto` porta la descrizione **canonica**, non quella d'origine:
    /// due supervisori che dichiarano lo stesso ambiente con gli insiemi in
    /// ordine diverso emettono percio' lo stesso frame, byte per byte.
    /// Spedire la forma d'origine e confrontare quella ridotta avrebbe reso il
    /// frame dipendente dall'ordine in cui qualcuno ha riempito un `Vec`.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::InvalidConfiguration`] se cio' che il supervisore
    /// dichiara non e' coerente — campi vuoti, digest non canonici, duplicati
    /// fra risorse, backend o capability **richieste**, oppure acquisizione
    /// dinamica dichiarata.
    ///
    /// Le capability offerte non compaiono, e non per omissione: il
    /// supervisore non ne offre, e [`AtteseSupervisore`] non gliele fa
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

        // La **forma** prima del confronto, e non perche' il decoder l'abbia
        // gia' applicata: un `Frame` si costruisce anche in processo, senza
        // passare da `decodifica`. Un campo malformato confrontato per primo
        // esce come «non coincide», che manda a cercare un disaccordo dove
        // c'e' un valore che non e' un valore.
        //
        // La riduzione consuma cio' che e' arrivato: il frame e' gia' stato
        // esaurito, quindi non c'e' niente da clonare.
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
            commit_token: self.commit_token,
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
    /// Un `Incarico` che arrivi qui e' rifiutato **esplicitamente**: e' il
    /// caso «incarico prima dell'accordo», e va detto con quel nome invece di
    /// somigliare a un messaggio malformato.
    ///
    /// La `Risposta` porta la forma **canonica**, come il `Saluto`: due worker
    /// che descrivono lo stesso ambiente in ordine diverso rispondono con lo
    /// stesso frame.
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
    /// sequenza. L'handshake **non** verifica il contenuto dell'incarico: il
    /// piano, il suo hash e i contratti d'ingresso li verifica chi lo esegue,
    /// perche' verificarli qui vorrebbe dire farlo due volte e in due modi che
    /// possono divergere.
    pub fn ricevi_incarico(self, frame: Frame) -> Result<(Incarico, CommitToken)> {
        verifica_direzione(&frame, super::messaggi::Direzione::VersoWorker)?;
        let tipo = frame.tipo();
        let Corpo::Incarico(incarico) = frame.in_corpo() else {
            return Err(fuori_sequenza(TipoMessaggio::Incarico, tipo));
        };
        // La forma, per la stessa ragione del `Saluto` e della `Risposta`: il
        // frame puo' non essere passato dal decoder. Senza, un incarico con un
        // `plan_hash_atteso` che non e' un digest usciva di qui intatto, e a
        // rifiutarlo sarebbe stato chi lo esegue — cioe' dopo.
        super::codifica::verifica_incarico(&incarico)?;
        Ok((*incarico, self.commit_token))
    }
}

#[cfg(test)]
mod tests;
