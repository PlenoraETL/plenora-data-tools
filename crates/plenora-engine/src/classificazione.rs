//! Classificazione deterministica dell'esito di un worker isolato.
//!
//! Logica **pura**: nessun processo, nessun cgroup, nessun protocollo, nessun
//! orologio. Prende i fatti gia' raccolti e rende un esito, e lo fa allo
//! stesso modo ogni volta.
//!
//! Attua la §10 di [`isolamento.md`](../../../../docs/isolamento.md): la
//! matrice degli esiti, la classificazione dell'evidenza di memoria
//! (§10.0-bis) e la precedenza fra eventi concorrenti (§10.3).
//!
//! La **barriera causale** che decide *quando* i fatti sono completi (coda
//! unica, quiescenza, snapshot dell'evidenza) appartiene al supervisore; qui
//! si assume avvenuta, come dice [`FattiDopoLaQuiescenza`]. Niente timestamp
//! ne' ordine d'arrivo: la §10.3 chiude l'insieme degli eventi **prima** di
//! classificare.
//!
//! Niente esce dal crate, e niente `serde`: il formato sul filo appartiene al
//! modulo `protocollo`. A tenere dentro i tipi e' `mod classificazione;` senza
//! `pub` in [`crate`]; qui gli elementi sono `pub` perche' `pub(crate)` in un
//! modulo privato e' ridondante (`clippy::redundant_pub_crate`).

use plenora_core::panic_policy::FormaPayload;
use plenora_core::{ErrorCategory, EvidenzaDiLimite, PlenoraError};

#[cfg(test)]
mod tests;

/// Che cosa il worker ha dichiarato di se'.
///
/// **La morte senza esito non e' una variante.** E' l'assenza dell'esito, e si
/// rappresenta con [`FattiDopoLaQuiescenza::esito_worker`] a `None`: un
/// processo che muore non dichiara nulla, e dargli una variante lo farebbe
/// sembrare un'affermazione.
#[derive(Debug)]
pub enum EsitoWorker {
    /// Il worker dice di aver finito.
    ///
    /// **Non e' il successo finale**: significa «prosegui alla verifica». La
    /// riga 1 della matrice richiede la verifica 1-8 superata *e* il publish
    /// compiuto, e nessuna delle due e' un'affermazione del worker.
    Successo,
    /// Un errore tipizzato, **intero**.
    ///
    /// Non i quattro assi soli: quelli scarterebbero messaggio sanitizzato,
    /// contesto DAG, `execution_id` e diagnostica di riga prima che il
    /// modulo `protocollo` abbia deciso che cosa passa sul filo. Gli assi si
    /// leggono dall'errore, non lo sostituiscono.
    Errore(PlenoraError),
    /// Un panico, con la sola **forma** del payload.
    Panic { forma: FormaDelPayload },
}

/// La forma del payload di un panico, **senza** il contenuto.
///
/// Un tipo e non una `&'static str`, che accetterebbe qualunque letterale: il
/// campo e' privato e porta un [`FormaPayload`], un enum chiuso. Il contenuto
/// non entra **per costruzione**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormaDelPayload(FormaPayload);

impl FormaDelPayload {
    /// Legge la forma di un payload di panico ([`FormaPayload::di`]).
    pub fn di(payload: &(dyn std::any::Any + Send)) -> Self {
        Self(FormaPayload::di(payload))
    }

    /// Da una forma gia' nota, per esempio arrivata dal filo.
    pub const fn da(forma: FormaPayload) -> Self {
        Self(forma)
    }
}

impl std::fmt::Display for FormaDelPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.descrizione())
    }
}

/// Un segnale dell'evidenza, ridotto ai tre livelli su cui la §10.0-bis
/// ragiona.
///
/// [`Self::NonOsservato`] non e' [`Self::Zero`]: il primo dice «non ho
/// letto», il secondo «ho letto e non e' successo». Sono conclusioni opposte,
/// e la matrice le tratta diversamente.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segnale {
    NonOsservato,
    Zero,
    Positivo,
}

impl Segnale {
    /// Riduce un contatore al proprio livello.
    const fn di(contatore: Option<u64>) -> Self {
        match contatore {
            None => Self::NonOsservato,
            Some(0) => Self::Zero,
            Some(_) => Self::Positivo,
        }
    }

    const fn positivo(self) -> bool {
        matches!(self, Self::Positivo)
    }

    const fn osservato(self) -> bool {
        !matches!(self, Self::NonOsservato)
    }
}

/// Che cosa l'evidenza di memoria autorizza a dire.
///
/// `Indeterminata` esiste perche' «non attribuito» e' **a sua volta
/// un'affermazione**: con `Ol` non letto e gli altri a zero l'osservazione e'
/// incompleta, non una pressione.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClasseEvidenzaMemoria {
    /// Relazioni impossibili fra i segnali: la misura non e' una misura.
    Incoerente,
    /// Tutti e quattro osservati e positivi: l'unica riga che attribuisce.
    Attribuita,
    /// Tutti e quattro `Some(0)`: letto, e non c'e' nulla.
    Assente,
    /// Almeno una pressione **osservata**, prova insufficiente.
    NonAttribuita,
    /// Letture mancanti e nessun segnale positivo: non lo sappiamo.
    Indeterminata,
}

impl ClasseEvidenzaMemoria {
    /// La categoria d'errore che compete alla classe, quando e' la classe a
    /// decidere l'esito.
    pub const fn categoria(self) -> ErrorCategory {
        match self {
            Self::Attribuita => ErrorCategory::ResourceLimit,
            Self::NonAttribuita => ErrorCategory::UnattributedMemoryPressure,
            // Nessuna conclusione sulla memoria: `Internal` e' l'unica
            // categoria che non afferma nulla sul budget del chiamante.
            Self::Assente | Self::Indeterminata | Self::Incoerente => ErrorCategory::Internal,
        }
    }
}

/// Classifica l'evidenza secondo la §10.0-bis.
///
/// L'ordine e' normativo, perche' le condizioni si **sovrappongono**:
/// incoerente, attribuita, assente, non attribuita, indeterminata.
/// L'incoerenza viene prima perche' le altre, applicate a una lettura rotta,
/// darebbero una classificazione dall'aria normale.
pub fn classifica_evidenza(evidenza: &EvidenzaDiLimite) -> ClasseEvidenzaMemoria {
    // 1. L'incoerenza si valuta sui VALORI GREZZI, prima della riduzione a
    //    livelli. Dopo, `Kl = 2` e `Kh = 1` sono entrambi «positivo» e la
    //    disuguaglianza sparisce: e' l'ordine dei due passi a decidere se il
    //    controllo esiste.
    if let (Some(kl), Some(kh)) = (evidenza.uccisi_nel_dominio, evidenza.uccisi_nella_gerarchia) {
        if kl > kh {
            return ClasseEvidenzaMemoria::Incoerente;
        }
    }
    let ol = Segnale::di(evidenza.oom_locali);
    let kl = Segnale::di(evidenza.uccisi_nel_dominio);
    let kh = Segnale::di(evidenza.uccisi_nella_gerarchia);
    let g = Segnale::di(evidenza.group_kill_locale);

    // Due contraddizioni, constatabili solo su segnali OSSERVATI: `G` senza
    // `Ol` (group kill senza che il tetto del dominio sia stato invocato) e
    // `G` senza `Kh` (group kill che non ha ucciso nulla).
    if g.positivo() && (ol == Segnale::Zero || kh == Segnale::Zero) {
        return ClasseEvidenzaMemoria::Incoerente;
    }

    let tutti_osservati = ol.osservato() && kl.osservato() && kh.osservato() && g.osservato();
    let tutti_positivi = ol.positivo() && kl.positivo() && kh.positivo() && g.positivo();
    let qualche_positivo = ol.positivo() || kl.positivo() || kh.positivo() || g.positivo();

    // 2. Attribuzione forte: la riga INTERA, non il solo `G`.
    if tutti_osservati && tutti_positivi {
        return ClasseEvidenzaMemoria::Attribuita;
    }
    // 3. Assenza: letto tutto, e tutto a zero. Senza `tutti_osservati` un
    //    `None` si leggerebbe come uno zero, e «non ho letto» diventerebbe
    //    «non e' successo».
    if tutti_osservati && !qualche_positivo {
        return ClasseEvidenzaMemoria::Assente;
    }
    // 4. Qualcosa si e' visto, e non basta.
    if qualche_positivo {
        return ClasseEvidenzaMemoria::NonAttribuita;
    }
    // 5. Nessun positivo osservato e qualche lettura mancante.
    ClasseEvidenzaMemoria::Indeterminata
}

/// I fatti su cui si classifica, raccolti **dopo la quiescenza del dominio**.
///
/// Prima della quiescenza `esito_worker: None` sarebbe ambiguo fra «morto
/// senza esito» e «sta ancora lavorando», e l'evidenza una lettura parziale
/// che puo' ancora cambiare. I campi sono privati: si passa da
/// [`Self::dopo_la_quiescenza`], che nomina la condizione.
#[derive(Debug)]
pub struct FattiDopoLaQuiescenza {
    publish_completato: bool,
    evidenza: Option<Box<EvidenzaDiLimite>>,
    timeout_scaduto: bool,
    cancellazione_richiesta: bool,
    /// `None` significa **morto senza esito**, non «non ancora finito»: la
    /// quiescenza e' gia' avvenuta.
    esito_worker: Option<EsitoWorker>,
}

impl FattiDopoLaQuiescenza {
    /// I fatti, dichiarando che il dominio e' quiescente.
    pub fn dopo_la_quiescenza(
        publish_completato: bool,
        evidenza: Option<EvidenzaDiLimite>,
        timeout_scaduto: bool,
        cancellazione_richiesta: bool,
        esito_worker: Option<EsitoWorker>,
    ) -> Self {
        Self {
            publish_completato,
            evidenza: evidenza.map(Box::new),
            timeout_scaduto,
            cancellazione_richiesta,
            esito_worker,
        }
    }
}

/// L'esito, dopo la precedenza della §10.3.
///
/// Ogni variante che possa coesistere con una lettura la porta con se'; dove
/// la prova autorizza l'esito e' **obbligatoria per tipo**.
#[derive(Debug)]
pub enum EsitoClassificato {
    /// Riga 1: l'output e' visibile. Nessun evento successivo lo rende non
    /// riuscito.
    Pubblicato {
        evidenza: Option<Box<EvidenzaDiLimite>>,
    },
    /// Riga 5: l'unica con attribuzione. La prova e' obbligatoria.
    LimiteAttribuito(Box<EvidenzaDiLimite>),
    /// Riga 7.
    Timeout {
        evidenza: Option<Box<EvidenzaDiLimite>>,
    },
    /// Riga 8.
    Cancellato {
        evidenza: Option<Box<EvidenzaDiLimite>>,
    },
    /// Righe 5-bis e 6b. Anche qui la prova e' obbligatoria: senza, la
    /// categoria direbbe «c'era pressione» senza portare cio' che si e'
    /// visto.
    PressioneNonAttribuita(Box<EvidenzaDiLimite>),
    /// L'evidenza c'e' e **non e' utilizzabile**: incoerente o indeterminata.
    ///
    /// E' terminale e viene prima dell'esito del worker: altrimenti un
    /// successo dichiarato darebbe `DaVerificare` con una lettura rotta, e per
    /// la §10.0-bis queste classi non autorizzano la pubblicazione. La prova e'
    /// obbligatoria.
    EvidenzaNonUtilizzabile {
        classe: ClasseEvidenzaMemoria,
        prova: Box<EvidenzaDiLimite>,
    },
    /// Riga 2.
    ErroreDelWorker {
        errore: PlenoraError,
        evidenza: Option<Box<EvidenzaDiLimite>>,
    },
    /// Riga 3.
    PanicDelWorker {
        forma: FormaDelPayload,
        evidenza: Option<Box<EvidenzaDiLimite>>,
    },
    /// Il worker dice di aver finito e il publish non c'e' stato: **prosegui
    /// alla verifica**, non successo.
    DaVerificare {
        evidenza: Option<Box<EvidenzaDiLimite>>,
    },
    /// Righe 4 e 6a: il processo muore e non c'e' nulla da cui concludere.
    TerminazioneAmbigua {
        evidenza: Option<Box<EvidenzaDiLimite>>,
    },
}

impl EsitoClassificato {
    /// L'evidenza raccolta, se c'e'.
    pub fn evidenza(&self) -> Option<&EvidenzaDiLimite> {
        match self {
            Self::LimiteAttribuito(prova)
            | Self::PressioneNonAttribuita(prova)
            | Self::EvidenzaNonUtilizzabile { prova, .. } => Some(prova),
            Self::Pubblicato { evidenza }
            | Self::Timeout { evidenza }
            | Self::Cancellato { evidenza }
            | Self::ErroreDelWorker { evidenza, .. }
            | Self::PanicDelWorker { evidenza, .. }
            | Self::DaVerificare { evidenza }
            | Self::TerminazioneAmbigua { evidenza } => evidenza.as_deref(),
        }
    }

    /// La categoria d'errore dell'esito. `None` per i due esiti che non sono
    /// errori.
    pub const fn categoria(&self) -> Option<ErrorCategory> {
        match self {
            // Pubblicato e' successo; `DaVerificare` non e' ancora un esito
            // finale, e dargli una categoria lo farebbe sembrare un
            // fallimento.
            Self::Pubblicato { .. } | Self::DaVerificare { .. } => None,
            Self::LimiteAttribuito(_) => Some(ErrorCategory::ResourceLimit),
            Self::Timeout { .. } => Some(ErrorCategory::Timeout),
            Self::Cancellato { .. } => Some(ErrorCategory::Cancelled),
            Self::PressioneNonAttribuita(_) => Some(ErrorCategory::UnattributedMemoryPressure),
            // `Internal`, come le tre classi che non concludono: non afferma
            // nulla sul budget del chiamante.
            Self::EvidenzaNonUtilizzabile { .. } => Some(ErrorCategory::Internal),
            Self::ErroreDelWorker { errore, .. } => Some(errore.category()),
            Self::PanicDelWorker { .. } | Self::TerminazioneAmbigua { .. } => {
                Some(ErrorCategory::Internal)
            }
        }
    }

    /// L'esito pubblica?
    ///
    /// Solo la riga 1. Ogni esito terminale ambiguo non pubblica mai
    /// (`GA-1`), e questo vale anche per la pressione non attribuita: che la
    /// categoria dica «non lo sappiamo» non la rende meno terminale.
    pub const fn pubblica(&self) -> bool {
        matches!(self, Self::Pubblicato { .. })
    }
}

/// Classifica i fatti secondo la precedenza **totale** della §10.3.
///
/// L'ordine va dal fatto piu' esterno al meno verificabile: publish
/// completato, OOM attribuito, timeout, cancellazione, pressione non
/// attribuita, evidenza non utilizzabile, esito del worker, terminazione
/// ambigua (mai `ResourceLimit`).
///
/// La pressione non attribuita sta sotto timeout e cancellazione, fatti nostri
/// che concludono, e sopra l'esito del worker: l'errore riportato sotto
/// pressione e' quasi sempre la conseguenza, non la causa.
pub fn classifica(fatti: FattiDopoLaQuiescenza) -> EsitoClassificato {
    let FattiDopoLaQuiescenza {
        publish_completato,
        evidenza,
        timeout_scaduto,
        cancellazione_richiesta,
        esito_worker,
    } = fatti;

    // Classe e prova viaggiano insieme (vedi `Letta`).
    let letta = Letta::da(evidenza);

    if publish_completato {
        // 1. Publish completato.
        return EsitoClassificato::Pubblicato {
            evidenza: letta.in_evidenza(),
        };
    }
    // 2. OOM attribuito.
    if let Letta::Con(ClasseEvidenzaMemoria::Attribuita, prova) = letta {
        return EsitoClassificato::LimiteAttribuito(prova);
    }
    // 3. Timeout.
    if timeout_scaduto {
        return EsitoClassificato::Timeout {
            evidenza: letta.in_evidenza(),
        };
    }
    // 4. Cancellazione.
    if cancellazione_richiesta {
        return EsitoClassificato::Cancellato {
            evidenza: letta.in_evidenza(),
        };
    }
    // 5. Pressione non attribuita.
    if let Letta::Con(ClasseEvidenzaMemoria::NonAttribuita, prova) = letta {
        return EsitoClassificato::PressioneNonAttribuita(prova);
    }
    // 6. Evidenza incoerente o indeterminata: terminale.
    //
    // NON `Assente`: una lettura riuscita e vuota non contraddice l'esito
    // del worker.
    if let Letta::Con(
        classe @ (ClasseEvidenzaMemoria::Incoerente | ClasseEvidenzaMemoria::Indeterminata),
        prova,
    ) = letta
    {
        return EsitoClassificato::EvidenzaNonUtilizzabile { classe, prova };
    }
    // 7. Esito dichiarato dal worker.
    let evidenza = letta.in_evidenza();
    match esito_worker {
        Some(EsitoWorker::Errore(errore)) => {
            EsitoClassificato::ErroreDelWorker { errore, evidenza }
        }
        Some(EsitoWorker::Panic { forma }) => EsitoClassificato::PanicDelWorker { forma, evidenza },
        Some(EsitoWorker::Successo) => EsitoClassificato::DaVerificare { evidenza },
        // 8. Terminazione ambigua: morto senza esito. Ci arriva anche la
        //    classe `Assente`, che non produce un livello proprio, e
        //    l'evidenza la accompagna.
        None => EsitoClassificato::TerminazioneAmbigua { evidenza },
    }
}

/// Evidenza e sua classe, insieme o nessuna delle due.
///
/// Rende impossibile un `Attribuita` senza la prova: con due `Option` separati
/// servirebbe un ripiego che fabbrica un'evidenza vuota, poi riportata come
/// prova.
#[derive(Debug)]
enum Letta {
    Nessuna,
    Con(ClasseEvidenzaMemoria, Box<EvidenzaDiLimite>),
}

impl Letta {
    fn da(evidenza: Option<Box<EvidenzaDiLimite>>) -> Self {
        evidenza.map_or(Self::Nessuna, |prova| {
            Self::Con(classifica_evidenza(&prova), prova)
        })
    }

    // Non `const`: consuma un `Box`, e un distruttore non si valuta in
    // contesto costante. Clippy lo suggerisce guardando il corpo, non il tipo.
    fn in_evidenza(self) -> Option<Box<EvidenzaDiLimite>> {
        match self {
            Self::Nessuna => None,
            Self::Con(_, prova) => Some(prova),
        }
    }
}
