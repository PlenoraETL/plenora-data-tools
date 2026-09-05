//! La macchina a stati del supervisore: fatti in una coda, un solo giudice.
//!
//! # La regola, e perche' e' una sola
//!
//! **I produttori accodano fatti. Nessuno giudica.** Un produttore che
//! concludesse — «questo e' un timeout», «questo e' un successo» — sposterebbe
//! una decisione in un posto dove il quadro non c'e': il lettore non sa se il
//! dominio e' quiescente, l'orologio non sa se il worker ha parlato, e chi
//! osserva il cgroup non sa niente di entrambi. Ognuno concluderebbe da cio'
//! che vede, e le conclusioni si contraddirebbero.
//!
//! Qui i produttori registrano **cio' che e' successo**, e basta. Un solo
//! consumatore raccoglie, e alla fine chiama [`classifica`] **una volta**.
//!
//! # Perche' l'ordine d'arrivo non decide
//!
//! Perche' non e' una proprieta' del sistema, e' una proprieta' della corsa.
//! Che l'EOF arrivi prima o dopo l'uscita del worker dipende dallo scheduler,
//! e un esito che ne dipendesse cambierebbe da un'esecuzione all'altra sugli
//! stessi fatti.
//!
//! La riduzione e' quindi **commutativa** per costruzione: ogni fatto accende
//! un campo, e nessun campo dipende da quando arriva. La precedenza fra le
//! cause e' quella della §10.3, applicata da [`classifica`] e da nessun altro.
//! I casi la provano permutando i fatti e pretendendo lo stesso esito.
//!
//! # I quattro fatti che restano distinti
//!
//! `Esito`, **uscita**, **EOF** e **quiescenza** sono quattro cose diverse, e
//! il successo le vuole tutte:
//!
//! - l'`Esito` e' un'**affermazione del worker**, e un'affermazione non e' una
//!   prova: dice «ho finito», non «e' finito»;
//! - l'**uscita** e' il processo che muore, **e** che qualcuno lo raccoglie:
//!   un figlio non raccolto resta zombie, e uno zombie non e' un lavoro
//!   concluso;
//! - l'**EOF** e' l'unica cosa che dice che nessuno tiene piu' l'altro capo del
//!   canale. Un discendente del worker che se lo fosse portato dietro lo
//!   terrebbe aperto, e l'EOF non arriverebbe: e' precisamente cio' che
//!   trasforma quella non-garanzia in un ritardo dichiarato invece che in un
//!   risultato sbagliato;
//! - la **quiescenza** e' il dominio vuoto, cioe' che nel cgroup non e' rimasto
//!   nulla.
//!
//! Nessuna sostituisce le altre, e cio' che manca non diventa mai un successo:
//! diventa un tempo che finisce.

use plenora_core::error::{
    ErrorCategory, ErrorPhase, EvidenzaDiLimite, PlenoraError, RemoteEffect, ReplayedError,
    RetryDisposition,
};

use crate::classificazione::{classifica, EsitoClassificato, FattiDopoLaQuiescenza};
use crate::protocollo::messaggi::{
    CategoriaSulFilo, ConteggiDichiarati, Corpo, DiagnosticaSulFilo, DigestArtefatto,
    EffettoSulFilo, ErroreSulFilo, EsitoWorkerSulFilo, FaseSulFilo, FormaPanicSulFilo,
    RetrySulFilo,
};

/// Come il worker e' uscito, **osservato** e non interpretato.
///
/// Codice e segnale sono cio' che il sistema operativo riporta; che un codice
/// diverso da zero sia un problema lo decide chi classifica, non chi guarda.
///
/// # Perche' un enum e non due campi facoltativi
///
/// Perche' due `Option` ammettono due stati che non esistono: **entrambi
/// presenti** — un processo non esce contemporaneamente da se' e per un segnale
/// — ed **entrambi assenti**, che non e' un'uscita ma l'assenza di
/// un'osservazione. Con i campi, ogni lettore deve decidere cosa farne, e
/// prima o poi due lettori decidono diversamente.
///
/// Con l'enum quei due stati non si possono scrivere. L'assenza
/// dell'osservazione si dice altrove, con [`Fatto::OsservazioneImpossibile`]:
/// e' un fatto **nostro**, e tenerlo fra le forme dell'uscita lo farebbe
/// sembrare un modo di uscire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UscitaOsservata {
    /// Il processo e' uscito da se', con questo codice.
    Codice(i32),
    /// Il processo e' stato ucciso da questo segnale.
    Segnale(i32),
}

impl UscitaOsservata {
    /// Un'uscita ordinaria a zero.
    ///
    /// **Solo** il codice zero. Un segnale non e' un'uscita pulita nemmeno
    /// quando e' quello che abbiamo mandato noi: significa che il processo non
    /// ha finito, e' stato fermato.
    const fn pulita(self) -> bool {
        matches!(self, Self::Codice(0))
    }

    /// Come si dice, per l'evidenza e per il confronto fra due letture.
    fn detta(self) -> String {
        match self {
            Self::Codice(codice) => format!("codice {codice}"),
            Self::Segnale(segnale) => format!("segnale {segnale}"),
        }
    }
}

/// Cio' che il worker **dichiara** di se', com'e' arrivato.
///
/// # Perche' si conserva la forma del filo
///
/// Perche' la conversione verso l'errore di dominio perde qualcosa, e dove
/// perde va deciso una volta e in un posto solo. Tenendo qui la forma arrivata,
/// la perdita e' visibile e il rapporto puo' riportare cio' che il worker ha
/// detto **davvero**, non la sua approssimazione.
#[derive(Debug)]
pub(super) enum EsitoDichiarato {
    /// «Ho finito.» Non «e' finito»: la verifica e il publish non sono
    /// affermazioni del worker.
    Successo {
        digest: DigestArtefatto,
        conteggi: ConteggiDichiarati,
    },
    Errore(Box<ErroreSulFilo>),
    Panic {
        forma: FormaPanicSulFilo,
    },
}

impl EsitoDichiarato {
    /// L'esito arrivato sul filo, senza giudizio.
    fn dal_filo(esito: EsitoWorkerSulFilo) -> Self {
        match esito {
            EsitoWorkerSulFilo::Successo {
                digest_artefatto,
                conteggi,
            } => Self::Successo {
                digest: digest_artefatto,
                conteggi,
            },
            EsitoWorkerSulFilo::Errore { errore } => Self::Errore(errore),
            EsitoWorkerSulFilo::Panic { forma } => Self::Panic { forma },
        }
    }

    /// Il nome della forma, per l'evidenza.
    const fn nome(&self) -> &'static str {
        match self {
            Self::Successo { .. } => "successo",
            Self::Errore(_) => "errore",
            Self::Panic { .. } => "panic",
        }
    }
}

/// Un fatto: qualcosa che e' successo.
///
/// **Nessuna variante e' una conclusione.** «Il tempo e' finito» e' un fatto;
/// «e' un timeout» sarebbe un giudizio, e non appartiene a chi accoda.
#[derive(Debug)]
pub(super) enum Fatto {
    /// Un messaggio del worker, decodificato e non interpretato.
    MessaggioDalWorker(Box<Corpo>),
    /// Il canale e' finito **in modo pulito**, al confine fra due messaggi.
    FineDelCanale,
    /// Il canale e' finito male: il filo si e' rotto, o il protocollo e' stato
    /// violato.
    ///
    /// # Perche' non e' `FineDelCanale`
    ///
    /// Perche' l'EOF pulito e' uno dei quattro fatti che il successo richiede, e
    /// un troncamento non lo e'. Fonderli direbbe che un interlocutore
    /// interrotto a meta' parola ha finito di parlare.
    CanaleInterrotto(String),
    /// Il worker e' uscito, ed e' stato raccolto.
    UscitaDelWorker(UscitaOsservata),
    /// Nel dominio non e' rimasto nessuno.
    DominioQuiescente,
    /// Cio' che il dominio dice della memoria, letto **dopo** la quiescenza.
    EvidenzaDelDominio(Box<EvidenzaDiLimite>),
    /// Il tempo dato all'esecuzione e' finito.
    ///
    /// # Perche' non dice quale fase
    ///
    /// Perche' questa macchina ne misura **una sola**. I timeout documentati
    /// sono due — handshake e esecuzione — ma il primo misura dall'avvio alla
    /// `Risposta`, cioe' un intervallo che si chiude **prima** che questa
    /// macchina esista: `CanaleOperativo` si costruisce solo da un accordo gia'
    /// concluso. Chi guida l'handshake misura quel tempo, e se scade non arriva
    /// mai qui.
    ///
    /// Portare qui una fase che questa macchina non puo' emettere darebbe un
    /// campo che vale sempre lo stesso, e prima o poi qualcuno lo leggerebbe
    /// come se potesse valere altro.
    TempoScaduto,
    /// Qualcuno ha chiesto di annullare.
    CancellazioneRichiesta,
    /// Un produttore **non e' riuscito a guardare**.
    ///
    /// Non e' un fatto sul worker: e' un fatto su di noi. Tenerlo separato
    /// impedisce che «non ho potuto vedere» si legga come «non c'e' niente».
    OsservazioneImpossibile { chi: &'static str, motivo: String },
}
/// Cio' che i fatti hanno acceso, senza ancora concludere niente.
///
/// # Perche' raccoglie invece di sovrascrivere
///
/// Perche' un campo che si sovrascrive sceglie in silenzio. «Vince il primo» e
/// «vince l'ultimo» sono due arbitrati diversi, entrambi plausibili, e
/// entrambi nascondono che c'e' qualcosa da arbitrare: sui fatti duplicati la
/// scelta e' invisibile, e su quelli **contraddittori** produce un esito che
/// sembra normale.
///
/// Qui si raccoglie tutto e si decide alla fine. Due osservazioni uguali sono
/// la stessa cosa detta due volte, e non cambiano niente; due diverse sono una
/// contraddizione, e diventano un impedimento — mai un esito.
///
/// # Perche' gli elenchi si riordinano
///
/// Perche' l'ordine d'arrivo e' una proprieta' della corsa, non
/// dell'esecuzione. Due rapporti degli stessi fatti devono coincidere riga per
/// riga anche se lo scheduler li ha consegnati in ordini diversi, quindi cio'
/// che si riporta e' **ordinato e senza ripetizioni**.
#[expect(
    clippy::struct_excessive_bools,
    reason = "sono cinque osservazioni indipendenti, ognuna con la sua sorgente e il suo \
              significato: raggrupparle in un tipo comune direbbe che hanno qualcosa in \
              comune, e non ce l'hanno. Il lint protegge dai booleani che sono in realta' \
              uno stato; qui sono fatti"
)]
#[derive(Debug, Default)]
pub(super) struct Registro {
    /// Gli esiti dichiarati, tutti.
    ///
    /// Il protocollo ne prevede uno: l'esito chiude la conversazione. Tenerli
    /// tutti serve a poterlo dire, invece di scegliere quale conta.
    esiti: Vec<EsitoDichiarato>,
    /// Quanti messaggi che non sono un esito sono arrivati, per l'evidenza.
    altri_messaggi: usize,
    fine_pulita: bool,
    /// I motivi per cui il canale si e' rotto, se si e' rotto.
    interruzioni: Vec<String>,
    /// Le uscite osservate. Piu' d'una **diversa** e' una contraddizione
    /// nostra, non del worker.
    uscite: Vec<UscitaOsservata>,
    quiescente: bool,
    /// Le letture dell'evidenza. Se ne fa una, dopo la quiescenza.
    evidenze: Vec<EvidenzaDiLimite>,
    /// Se il tempo dell'esecuzione e' finito.
    tempo_scaduto: bool,
    cancellazione: bool,
    /// Le osservazioni mancate.
    osservazioni_mancate: Vec<String>,
}

impl Registro {
    /// Accende cio' che il fatto dice, e niente altro.
    pub(super) fn applica(&mut self, fatto: Fatto) {
        match fatto {
            Fatto::MessaggioDalWorker(corpo) => self.messaggio(*corpo),
            Fatto::FineDelCanale => self.fine_pulita = true,
            Fatto::CanaleInterrotto(motivo) => self.interruzioni.push(motivo),
            Fatto::UscitaDelWorker(uscita) => self.uscite.push(uscita),
            Fatto::DominioQuiescente => self.quiescente = true,
            Fatto::EvidenzaDelDominio(evidenza) => self.evidenze.push(*evidenza),
            Fatto::TempoScaduto => self.tempo_scaduto = true,
            Fatto::CancellazioneRichiesta => self.cancellazione = true,
            Fatto::OsservazioneImpossibile { chi, motivo } => {
                self.osservazioni_mancate.push(format!("{chi}: {motivo}"));
            }
        }
    }

    /// Un messaggio del worker.
    fn messaggio(&mut self, corpo: Corpo) {
        match corpo {
            Corpo::Esito(esito) => self.esiti.push(EsitoDichiarato::dal_filo(*esito)),
            _ => self.altri_messaggi += 1,
        }
    }

    /// Se i produttori hanno detto tutto quello che possono dire.
    ///
    /// # Perche' due fatti e non tre
    ///
    /// Perche' l'**uscita** non arriva dai produttori: la osserva il conduttore,
    /// e la osserva **dopo** aver smesso di ascoltare — raccogliere un figlio
    /// mentre si aspetta che parli vorrebbe dire ucciderlo per sapere se aveva
    /// altro da dire.
    ///
    /// Aspettarla dentro il giro sarebbe quindi aspettare se stessi: il giro non
    /// finirebbe mai, e a chiuderlo resterebbe solo il timeout — cioe' ogni
    /// esecuzione, anche quella riuscita, finirebbe fuori tempo massimo. E' un
    /// difetto che questo codice ha avuto, e che un caso ha trovato appendendosi.
    pub(super) const fn si_puo_smettere_di_ascoltare(&self) -> bool {
        (self.fine_pulita || !self.interruzioni.is_empty()) && self.quiescente
    }

    /// Se il dominio si e' svuotato.
    pub(super) const fn dominio_quiescente(&self) -> bool {
        self.quiescente
    }

    /// Se qualcuno ha chiesto di annullare.
    ///
    /// Distinto da [`Self::si_deve_chiudere`] perche' le due chiusure non si
    /// comportano allo stesso modo: a una cancellazione si risponde chiedendo
    /// al worker di smettere, a un tempo scaduto no — il tempo che ha e'
    /// quello.
    pub(super) const fn cancellazione_richiesta(&self) -> bool {
        self.cancellazione
    }

    /// Se e' arrivato qualcosa che dice di chiudere.
    ///
    /// Un tempo finito o una cancellazione: due fatti che non concludono da
    /// soli — i tre terminali servono comunque — ma che dicono di smettere di
    /// aspettare che il lavoro finisca da se'.
    pub(super) const fn si_deve_chiudere(&self) -> bool {
        self.tempo_scaduto || self.cancellazione
    }

    /// Se i tre fatti terminali sono tutti arrivati.
    ///
    /// # Perche' tre e non quattro
    ///
    /// Perche' l'`Esito` **puo' mancare**: un worker che muore non dichiara
    /// nulla, e aspettarlo vorrebbe dire aspettare per sempre un processo che
    /// non c'e' piu'. La sua assenza e' un'informazione — «morto senza esito» —
    /// non una condizione da attendere.
    ///
    /// Il canale conta come finito anche se si e' rotto: un filo troncato non
    /// portera' altro. Che sia finito bene o male lo dice un campo diverso.
    pub(super) const fn concluso(&self) -> bool {
        (self.fine_pulita || !self.interruzioni.is_empty())
            && !self.uscite.is_empty()
            && self.quiescente
    }

    /// Se i quattro fatti positivi ci sono **tutti**.
    ///
    /// E' la condizione che il successo richiede, ed e' separata da
    /// [`Self::concluso`] perche' le due domande sono diverse: «si puo' smettere
    /// di aspettare» e «e' andato tutto bene» hanno risposte indipendenti.
    pub(super) fn quattro_fatti_positivi(&self) -> bool {
        self.esiti.len() == 1
            && self.fine_pulita
            && self.interruzioni.is_empty()
            && self.uscita_sola().is_some_and(UscitaOsservata::pulita)
            && self.quiescente
    }

    /// L'unica uscita osservata, se ce n'e' esattamente una **distinta**.
    fn uscita_sola(&self) -> Option<UscitaOsservata> {
        let prima = *self.uscite.first()?;
        self.uscite
            .iter()
            .all(|altra| *altra == prima)
            .then_some(prima)
    }

    /// Le contraddizioni fra i fatti, **ordinate** e senza ripetizioni.
    ///
    /// # Le tre, e perche' sono diverse fra loro
    ///
    /// **Piu' di un esito** e' una violazione del protocollo, e lo e' anche
    /// quando i due dicono la stessa cosa: l'esito *chiude* la conversazione, e
    /// un secondo messaggio dopo la chiusura dice che l'altro capo non sta
    /// seguendo il protocollo — che i contenuti coincidano non lo rimette in
    /// piedi. Qui non c'e' niente da arbitrare, e per questo non si arbitra.
    ///
    /// **Piu' di un'uscita diversa** e' una contraddizione **nostra**: l'uscita
    /// la osserviamo noi, e osservarla due volte in modo uguale e' la stessa
    /// lettura fatta due volte. In modo diverso significa che una delle due
    /// letture e' rotta, e non c'e' modo di sapere quale.
    ///
    /// **Piu' di una lettura dell'evidenza** e' la stessa cosa: se ne fa una,
    /// dopo la quiescenza, e due letture della stessa cosa in un momento in cui
    /// nulla cambia piu' non dovrebbero esistere.
    fn contraddizioni(&self) -> Vec<String> {
        let mut trovate = Vec::new();
        if self.esiti.len() > 1 {
            let mut forme: Vec<&str> = self.esiti.iter().map(EsitoDichiarato::nome).collect();
            forme.sort_unstable();
            trovate.push(format!(
                "il worker ha dichiarato {} esiti ({}), e l'esito chiude la conversazione",
                self.esiti.len(),
                forme.join(", ")
            ));
        }
        if self.uscite.len() > 1 && self.uscita_sola().is_none() {
            let mut viste: Vec<String> = self
                .uscite
                .iter()
                .copied()
                .map(UscitaOsservata::detta)
                .collect();
            viste.sort_unstable();
            viste.dedup();
            trovate.push(format!(
                "l'uscita del worker e' stata osservata in modi diversi ({}), e non c'e' modo di sapere quale lettura regge",
                viste.join(", ")
            ));
        }
        if self.evidenze.len() > 1 {
            trovate.push(format!(
                "l'evidenza del dominio e' stata letta {} volte dopo la quiescenza, quando nulla cambia piu'",
                self.evidenze.len()
            ));
        }
        trovate.sort();
        trovate
    }

    /// Cio' che si e' osservato, in forma leggibile, per l'evidenza.
    ///
    /// L'ordine delle righe e' quello **dichiarato qui**, e il contenuto di
    /// ogni riga e' ordinato: chi legge due rapporti degli stessi fatti deve
    /// poterli confrontare riga per riga.
    pub(super) fn evidenza_dei_fatti(&self) -> Vec<(&'static str, String)> {
        let elenco = |mut voci: Vec<String>| {
            if voci.is_empty() {
                return "nessuna".to_owned();
            }
            voci.sort();
            voci.dedup();
            voci.join(" | ")
        };
        vec![
            (
                "esiti_dichiarati",
                elenco(
                    self.esiti
                        .iter()
                        .map(|e| e.nome().to_owned())
                        .collect::<Vec<_>>(),
                ),
            ),
            ("altri_messaggi", self.altri_messaggi.to_string()),
            ("fine_pulita", self.fine_pulita.to_string()),
            ("interruzioni", elenco(self.interruzioni.clone())),
            (
                "uscite",
                elenco(
                    self.uscite
                        .iter()
                        .copied()
                        .map(UscitaOsservata::detta)
                        .collect(),
                ),
            ),
            ("quiescente", self.quiescente.to_string()),
            ("tempo_scaduto", self.tempo_scaduto.to_string()),
            ("cancellazione", self.cancellazione.to_string()),
            ("evidenze_lette", self.evidenze.len().to_string()),
            (
                "diagnostica_di_riga",
                elenco(self.diagnostiche_nel_rapporto()),
            ),
            (
                "osservazioni_mancate",
                elenco(self.osservazioni_mancate.clone()),
            ),
            ("contraddizioni", elenco(self.contraddizioni())),
        ]
    }

    /// La diagnostica di riga arrivata con l'esito, **intera**.
    ///
    /// # Perche' si possiede invece di contarla
    ///
    /// Perche' un conteggio conserva l'esistenza, non il contenuto: dire «ce
    /// n'e' una» e buttarla e' un modo piu' educato di buttarla. Se il dato non
    /// va perso, va **tenuto**.
    ///
    /// # Perche' fuori dall'errore
    ///
    /// Perche' [`DiagnosticaSulFilo`] non e' isomorfa a `RowDiagnostics`: le
    /// mancano campi, e riempirli con valori inventati direbbe di aver osservato
    /// cose che nessuno ha osservato. L'errore di dominio porta quindi i quattro
    /// assi — quelli si', senza perdite — e la diagnostica viaggia **accanto**,
    /// nella forma in cui e' arrivata.
    ///
    /// E' un limite dichiarato, non una perdita: chi legge l'esito del
    /// supervisore ha tutto. Cio' che resta aperto e' se debba arrivare fino a
    /// `RowDiagnostics`, e per farlo serve un portatore tipizzato — non dei
    /// default.
    ///
    /// # Perche' solo quando l'esito e' uno
    ///
    /// Perche' con due esiti non esiste **la** diagnostica: ce ne sono due, e
    /// prendere quella del primo arrivato sarebbe di nuovo un arbitrato — lo
    /// stesso che il registro rifiuta di fare sull'esito, fatto di nascosto su
    /// cio' che l'esito si porta dietro. Due rapporti degli stessi due esiti,
    /// consegnati in ordine opposto, direbbero cose diverse.
    ///
    /// Quando gli esiti sono piu' d'uno il protocollo e' gia' contraddittorio e
    /// non si conclude: le diagnostiche restano tutte nel rapporto, ordinate, e
    /// nessuna viene eletta.
    fn diagnostica_di_riga(&self) -> Option<DiagnosticaSulFilo> {
        let [solo] = &self.esiti[..] else {
            return None;
        };
        match solo {
            EsitoDichiarato::Errore(errore) => errore.diagnostica.clone(),
            EsitoDichiarato::Successo { .. } | EsitoDichiarato::Panic { .. } => None,
        }
    }

    /// Le diagnostiche arrivate, **tutte**, per il rapporto.
    ///
    /// Ordinate e senza ripetizioni: gli stessi esiti, in qualunque ordine,
    /// danno la stessa riga.
    fn diagnostiche_nel_rapporto(&self) -> Vec<String> {
        self.esiti
            .iter()
            .filter_map(|esito| match esito {
                EsitoDichiarato::Errore(errore) => errore.diagnostica.as_ref().map(|d| {
                    format!(
                        "{} su {} osservate, esempi troncati: {}",
                        d.scope, d.observed_total, d.esempi_troncati
                    )
                }),
                EsitoDichiarato::Successo { .. } | EsitoDichiarato::Panic { .. } => None,
            })
            .collect()
    }

    /// I fatti per la classificazione, **consumando il registro**.
    ///
    /// # Perche' consuma
    ///
    /// Perche' la classificazione avviene una volta sola, e prendere `self` per
    /// valore e' il modo di dirlo con il tipo invece che con un commento: dopo
    /// questa chiamata il registro non esiste piu', quindi non c'e' una seconda
    /// chiamata da evitare per disciplina.
    ///
    /// # Perche' `publish_completato` e' sempre falso
    ///
    /// Perche' il publish non appartiene a questo perimetro: e' la sequenza di
    /// `PR-10`, e finche' non c'e' nessuno che la compie, dichiararla compiuta
    /// sarebbe una bugia. La riga 1 della matrice resta quindi irraggiungibile
    /// qui, e un worker che dichiara successo produce `DaVerificare` — che e'
    /// «prosegui», non «riuscito».
    fn in_fatti(self) -> FattiDopoLaQuiescenza {
        let esito = self.esiti.first().map(|dichiarato| match dichiarato {
            EsitoDichiarato::Successo { .. } => crate::classificazione::EsitoWorker::Successo,
            EsitoDichiarato::Panic { forma } => crate::classificazione::EsitoWorker::Panic {
                forma: forma_di_dominio(*forma),
            },
            EsitoDichiarato::Errore(errore) => {
                crate::classificazione::EsitoWorker::Errore(errore_di_dominio(errore))
            }
        });

        FattiDopoLaQuiescenza::dopo_la_quiescenza(
            false,
            self.evidenze.into_iter().next(),
            self.tempo_scaduto,
            self.cancellazione,
            esito,
        )
    }

    /// L'esito, **classificato una volta sola**.
    ///
    /// # Perche' le contraddizioni si guardano prima
    ///
    /// Perche' classificare fatti che si contraddicono produce un esito che
    /// sembra normale. Chi lo legge non ha modo di sapere che sotto ci sono due
    /// osservazioni incompatibili, e la prima cosa che farebbe con un esito
    /// normale e' crederci.
    ///
    /// # Errors
    ///
    /// [`Impedimento`] quando i fatti non si lasciano ridurre a un esito.
    pub(super) fn concludi(
        self,
        difetti_della_conduzione: &[String],
    ) -> std::result::Result<EsitoDelSupervisore, Impedimento> {
        let contraddizioni = self.contraddizioni();
        if !contraddizioni.is_empty() {
            return Err(Impedimento::FattiContraddittori(contraddizioni));
        }
        // Cio' che serve dopo si prende **prima**: `in_fatti` consuma il
        // registro, e dopo non c'e' piu' niente da cui prenderlo.
        let diagnostica_di_riga = self.diagnostica_di_riga();
        let rapporto = self.evidenza_dei_fatti();
        let manca = self.cosa_manca_alla_barriera(difetti_della_conduzione);

        // **La barriera precede la classificazione.** `FattiDopoLaQuiescenza` si
        // chiama cosi' perche' quello e' il momento in cui i suoi campi
        // significano qualcosa: prima, l'evidenza e' una fotografia di qualcosa
        // che si muove, e l'esito del worker puo' non essere ancora arrivato.
        // Costruirli su un dominio ancora abitato sarebbe una bugia dichiarata
        // nel nome del tipo.
        //
        // Non e' una cautela in piu' su `DaVerificare`: e' la condizione perche'
        // **qualunque** classificazione abbia senso. Senza, la stessa esecuzione
        // diventa `Timeout` o `LimiteAttribuito` secondo quando il kernel
        // consegna un OOM.
        if !self.quiescente {
            return Err(Impedimento::BarrieraIncompleta(manca));
        }

        let classificato = classifica(self.in_fatti());

        // **Il resto della barriera governa il proseguire.**
        //
        // La quiescenza e' gia' stata pretesa sopra, per ogni esito: qui non se
        // ne parla piu'. Cio' che resta in `manca` sono le altre voci — l'EOF
        // che non e' arrivato, un'uscita che non e' pulita, un'osservazione
        // saltata, un difetto della conduzione — e quelle non impediscono di
        // **dire** com'e' andata: impediscono di **andare avanti**.
        //
        // `DaVerificare` non e' un esito, e' un permesso: dice «vai verso la
        // verifica e il publish». Concederlo su un'esecuzione che non si e'
        // vista finire vorrebbe dire pubblicare su una speranza, ed e'
        // esattamente cio' che la §10.3 vieta.
        //
        // Gli altri esiti non si toccano: un errore dichiarato dal worker resta
        // un errore anche se l'EOF non e' arrivato, e trasformarlo direbbe una
        // cosa falsa su una cosa che si e' vista.
        if matches!(classificato, EsitoClassificato::DaVerificare { .. }) && !manca.is_empty() {
            return Err(Impedimento::BarrieraIncompleta(manca));
        }

        Ok(EsitoDelSupervisore {
            classificato,
            rapporto,
            diagnostica_di_riga,
        })
    }

    /// Che cosa manca perche' si possa proseguire.
    ///
    /// # Perche' l'elenco e non un «si'/no»
    ///
    /// Perche' chi legge deve sapere **che cosa** e' mancato: «non si e' visto
    /// l'EOF» manda a cercare un discendente che tiene il canale, «il dominio
    /// non e' quiescente» manda a guardare il cgroup, e un difetto della
    /// conduzione manda a guardare noi. Un booleano li manderebbe tutti e tre
    /// nello stesso posto sbagliato.
    ///
    /// L'elenco e' **ordinato**, come tutto cio' che il registro riporta.
    fn cosa_manca_alla_barriera(&self, difetti_della_conduzione: &[String]) -> Vec<String> {
        let mut manca = Vec::new();
        if self.esiti.len() != 1 {
            manca.push(format!(
                "esiti dichiarati: {} invece di uno",
                self.esiti.len()
            ));
        }
        if !self.fine_pulita {
            manca.push("il canale non e' finito in modo pulito".to_owned());
        }
        if !self.interruzioni.is_empty() {
            manca.push("il canale si e' interrotto".to_owned());
        }
        match self.uscita_sola() {
            None => manca.push("l'uscita del worker non e' stata osservata".to_owned()),
            Some(uscita) if !uscita.pulita() => {
                manca.push(format!("il worker e' uscito con {}", uscita.detta()));
            }
            Some(_) => (),
        }
        if !self.quiescente {
            manca.push("il dominio non e' quiescente".to_owned());
        }
        if !self.osservazioni_mancate.is_empty() {
            manca.push(format!(
                "osservazioni mancate: {}",
                self.osservazioni_mancate.len()
            ));
        }
        for difetto in difetti_della_conduzione {
            manca.push(format!("difetto della conduzione: {difetto}"));
        }
        manca.sort();
        manca.dedup();
        manca
    }
}

/// L'esito del supervisore: la classificazione, **e** cio' che non entra nella
/// classificazione.
///
/// # Perche' due campi
///
/// Perche' `EsitoClassificato` risponde a «com'e' andata», e la diagnostica di
/// riga non e' una risposta a quella domanda: e' cio' che il worker osserva
/// mentre lavora. Infilarla nell'errore richiederebbe di convertirla
/// in `RowDiagnostics`, che ha campi che il filo non porta — e inventarli
/// sarebbe peggio che tenerla da parte.
///
/// Tenendola qui, la conversione dell'errore resta **senza perdite sui quattro
/// assi** e la diagnostica resta **intera nella sua forma**. Nessuna delle due
/// affermazioni copre l'altra, e per questo sono due campi e non uno.
#[derive(Debug)]
pub(super) struct EsitoDelSupervisore {
    /// Com'e' andata, secondo la precedenza della §10.3.
    pub(super) classificato: EsitoClassificato,
    /// Che cosa il registro aveva, riga per riga.
    ///
    /// # Perche' esce insieme all'esito
    ///
    /// Perche' l'esito e' una conclusione, e una conclusione non dice da quali
    /// fatti viene. Due esecuzioni possono finire entrambe in `Timeout` — una
    /// perche' il worker non ha mai parlato, l'altra perche' ha parlato tardi —
    /// e chi legge deve poterle distinguere senza rieseguirle.
    ///
    /// E' anche l'unica finestra sul registro, che `concludi` consuma: senza,
    /// nessun caso potrebbe dire **quali** fatti sono arrivati, solo che
    /// l'esito e' quello.
    pub(super) rapporto: Vec<(&'static str, String)>,
    /// Cio' che il worker ha osservato sulle righe, se lo ha detto.
    ///
    /// Nella forma del filo, che e' limitata per costruzione: il protocollo ne
    /// tetta esempi e conteggi, quindi tenerla non apre una via a una
    /// dimensione che il chiamante sceglie.
    pub(super) diagnostica_di_riga: Option<DiagnosticaSulFilo>,
}

/// Perche' dai fatti non esce un esito.
///
/// # Perche' non e' una variante dell'esito
///
/// Perche' non e' un modo in cui l'esecuzione puo' andare: e' un modo in cui la
/// **nostra osservazione** puo' rompersi. Metterla fra gli esiti classificati
/// la renderebbe una risposta alla domanda «com'e' andata», e non lo e' — la
/// risposta e' che non lo sappiamo, e perche'.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Impedimento {
    /// Un produttore non e' nato: l'esecuzione non e' stata **osservata**.
    ///
    /// # Perche' e' un impedimento e non un esito
    ///
    /// Perche' non dice niente su come e' andata. Il sistema ha rifiutato un
    /// thread — un limite raggiunto, memoria finita — e cio' che segue non e'
    /// un'esecuzione osservata male: e' un'esecuzione di cui manca l'osservatore.
    /// Darle un esito la farebbe sembrare qualcosa che e' andato in un modo.
    ///
    /// # Che cosa **non** significa
    ///
    /// Non significa «il tentativo non e' cominciato». Il worker a quel punto
    /// **esiste gia'**, puo' avere discendenti, e puo' aver gia' scritto sul
    /// canale. Dire che non e' cominciato lascerebbe credere che non ci sia
    /// niente da chiudere — ed e' il contrario: chi rinuncia deve chiudere il
    /// dominio, raccogliere il figlio e drenare cio' che e' gia' arrivato,
    /// esattamente come su ogni altro cammino.
    ProduttoreNonNato { chi: &'static str, motivo: String },
    /// La barriera causale non e' completa.
    ///
    /// # Perche' un impedimento e non un esito piu' cauto
    ///
    /// Perche' cio' che manca non e' un'informazione in meno sullo stesso esito:
    /// e' la ragione per cui **nessun esito significa quello che dice**.
    ///
    /// Il caso piu' netto e' la **quiescenza**. Finche' nel dominio c'e'
    /// qualcuno vivo, i contatori della memoria non sono un'osservazione ma una
    /// fotografia di qualcosa che si muove: il prototipo misura domini in cui
    /// l'evidenza dice zero al ritorno della `wait` e uno duecento millisecondi
    /// dopo. Classificare li' vorrebbe dire far dipendere l'esito
    /// da quando il kernel consegna un evento — la stessa esecuzione
    /// diventerebbe `Timeout` o `LimiteAttribuito` secondo il momento.
    ///
    /// Gli altri pezzi — l'EOF, l'uscita pulita, le osservazioni mancate —
    /// impediscono invece di **proseguire**: `DaVerificare` e' un permesso, e
    /// concederlo su un'esecuzione che non si e' vista finire vorrebbe dire
    /// pubblicare su una speranza.
    ///
    /// Declassare silenziosamente a «terminazione ambigua» sarebbe peggio: chi
    /// legge cercherebbe un worker morto male, invece della barriera che manca.
    BarrieraIncompleta(Vec<String>),
    /// Due o piu' fatti che non possono essere veri insieme.
    ///
    /// L'elenco e' **ordinato**: gli stessi fatti danno lo stesso impedimento,
    /// qualunque sia l'ordine in cui sono arrivati.
    FattiContraddittori(Vec<String>),
}

impl std::fmt::Display for Impedimento {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FattiContraddittori(quali) => {
                write!(
                    f,
                    "i fatti si contraddicono e non se ne ricava un esito: {}",
                    quali.join("; ")
                )
            }
            Self::ProduttoreNonNato { chi, motivo } => {
                write!(
                    f,
                    "il produttore «{chi}» non e' nato ({motivo}): l'esecuzione non e' \
                     stata osservata, e non c'e' niente da classificare"
                )
            }
            Self::BarrieraIncompleta(manca) => {
                write!(
                    f,
                    "la barriera causale non e' completa, e non se ne ricava un esito: {}",
                    manca.join("; ")
                )
            }
        }
    }
}

/// La categoria, dal filo al dominio.
///
/// # Perche' esaustiva e scritta a mano
///
/// Perche' le due enumerazioni sono **due**, e restano allineate solo se
/// qualcuno se ne accorge quando smettono di esserlo. Un `match` esaustivo lo
/// fa fare al compilatore: una variante nuova da una parte non compila finche'
/// non le si dice dove va. Una conversione per nome — passando dalle stringhe
/// stabili — compilerebbe sempre e fallirebbe a runtime, cioe' nel posto
/// sbagliato.
const fn categoria(dal_filo: CategoriaSulFilo) -> ErrorCategory {
    match dal_filo {
        CategoriaSulFilo::InvalidPlan => ErrorCategory::InvalidPlan,
        CategoriaSulFilo::InvalidConfiguration => ErrorCategory::InvalidConfiguration,
        CategoriaSulFilo::Schema => ErrorCategory::Schema,
        CategoriaSulFilo::DataMapping => ErrorCategory::DataMapping,
        CategoriaSulFilo::Crs => ErrorCategory::Crs,
        CategoriaSulFilo::Unsupported => ErrorCategory::Unsupported,
        CategoriaSulFilo::NotFound => ErrorCategory::NotFound,
        CategoriaSulFilo::Conflict => ErrorCategory::Conflict,
        CategoriaSulFilo::Authentication => ErrorCategory::Authentication,
        CategoriaSulFilo::Authorization => ErrorCategory::Authorization,
        CategoriaSulFilo::Timeout => ErrorCategory::Timeout,
        CategoriaSulFilo::Cancelled => ErrorCategory::Cancelled,
        CategoriaSulFilo::ResourceLimit => ErrorCategory::ResourceLimit,
        CategoriaSulFilo::Io => ErrorCategory::Io,
        CategoriaSulFilo::Protocol => ErrorCategory::Protocol,
        CategoriaSulFilo::Transient => ErrorCategory::Transient,
        CategoriaSulFilo::Execution => ErrorCategory::Execution,
        CategoriaSulFilo::IsolationUnavailable => ErrorCategory::IsolationUnavailable,
        CategoriaSulFilo::UnattributedMemoryPressure => ErrorCategory::UnattributedMemoryPressure,
        CategoriaSulFilo::Internal => ErrorCategory::Internal,
    }
}

/// La fase, dal filo al dominio.
const fn fase_di_errore(dal_filo: FaseSulFilo) -> ErrorPhase {
    match dal_filo {
        FaseSulFilo::Validate => ErrorPhase::Validate,
        FaseSulFilo::Connect => ErrorPhase::Connect,
        FaseSulFilo::Probe => ErrorPhase::Probe,
        FaseSulFilo::Prepare => ErrorPhase::Prepare,
        FaseSulFilo::Read => ErrorPhase::Read,
        FaseSulFilo::Write => ErrorPhase::Write,
        FaseSulFilo::Finalize => ErrorPhase::Finalize,
        FaseSulFilo::Commit => ErrorPhase::Commit,
        FaseSulFilo::Rollback => ErrorPhase::Rollback,
        FaseSulFilo::Cleanup => ErrorPhase::Cleanup,
    }
}

/// L'effetto remoto, dal filo al dominio.
const fn effetto(dal_filo: EffettoSulFilo) -> RemoteEffect {
    match dal_filo {
        EffettoSulFilo::None => RemoteEffect::None,
        EffettoSulFilo::RolledBack => RemoteEffect::RolledBack,
        EffettoSulFilo::Partial => RemoteEffect::Partial,
        EffettoSulFilo::Committed => RemoteEffect::Committed,
        EffettoSulFilo::Unknown => RemoteEffect::Unknown,
    }
}

/// La disposizione al ritentativo, dal filo al dominio.
///
/// L'unica variante con un valore e' `After`, e il valore e' un ritardo in
/// millisecondi: si converte, non si reinterpreta.
const fn ritentativo(dal_filo: &RetrySulFilo) -> RetryDisposition {
    match dal_filo {
        RetrySulFilo::Never {} => RetryDisposition::Never,
        RetrySulFilo::Safe {} => RetryDisposition::Safe,
        RetrySulFilo::RequiresIdempotencyKey {} => RetryDisposition::RequiresIdempotencyKey,
        RetrySulFilo::RequiresRecovery {} => RetryDisposition::RequiresRecovery,
        RetrySulFilo::After { delay_ms } => {
            RetryDisposition::After(std::time::Duration::from_millis(*delay_ms))
        }
    }
}

/// L'errore del worker, portato **senza perdere gli assi**.
///
/// # Perche' `Replayed` e non una variante scelta per categoria
///
/// Perche' la categoria di `PlenoraError` discende dalla variante, tranne che
/// qui: [`PlenoraError::Replayed`] porta gli assi **cosi' come arrivano**, e
/// `category`, `phase`, `remote_effect` e `retry_disposition` li rendono senza
/// ricalcolarli. Sceglierne invece una che «produca» la categoria dichiarata
/// avrebbe funzionato per alcune e per altre no — e per quelle no la categoria
/// riportata sarebbe stata inventata.
///
/// # Perche' `execution_reason` e' `None`
///
/// Perche' non viaggia. Quel campo serve a rigenerare il testo canonico quando
/// l'execution id viene assegnato **dopo** lo snapshot, e il protocollo non lo
/// trasporta: metterci il messaggio sanitizzato lo farebbe passare per un
/// motivo semantico che nessuno ha mandato.
///
/// # Senza perdite **sugli assi**, e non oltre
///
/// La diagnostica di riga non entra qui, e dirlo per intero conta: questa
/// conversione e' senza perdite sui quattro assi, non senza perdite in
/// assoluto. [`DiagnosticaSulFilo`] non e' isomorfa a `RowDiagnostics` — le
/// mancano campi — e completarli con valori inventati direbbe di aver osservato
/// cose che nessuno ha osservato.
///
/// Non viene pero' nemmeno scartata: la possiede
/// [`EsitoDelSupervisore::diagnostica_di_riga`], **intera** e nella forma in
/// cui e' arrivata. Cio' che resta aperto e' se debba arrivare fino a
/// `RowDiagnostics`, e per farlo serve un portatore tipizzato. La decisione e'
/// registrata in `errori-e-limiti.md`.
fn errore_di_dominio(dal_filo: &ErroreSulFilo) -> PlenoraError {
    PlenoraError::Replayed(Box::new(ReplayedError {
        category: categoria(dal_filo.categoria),
        phase: fase_di_errore(dal_filo.fase),
        remote_effect: effetto(dal_filo.effetto),
        retry: ritentativo(&dal_filo.retry),
        message: dal_filo.messaggio.clone(),
        node: dal_filo.nodo.clone(),
        operation: dal_filo.operazione.clone(),
        execution_id: dal_filo.execution_id.clone(),
        execution_reason: None,
    }))
}

/// La forma del panico, dal filo al dominio.
///
/// # Perche' passa dall'autorita' invece di riscrivere le tre stringhe
///
/// Perche' due elenchi della stessa cosa divergono. `FormaDelPayload` esiste
/// per garantire che il **contenuto** di un panico non entri mai dove il
/// progetto dichiara che non entra, e la garanzia sta nel fatto che il suo
/// unico costruttore chiama `plenora_core::panic_policy::forma_payload`.
/// Aggiungere un costruttore che prende una stringa la toglierebbe.
///
/// Qui si costruisce invece un **rappresentante** di ciascuna delle tre forme —
/// un `&'static str`, una `String`, un intero — e si chiede all'autorita' che
/// forma abbia. I rappresentanti sono vuoti o nulli: non portano niente da
/// pubblicare, e l'unica cosa che si estrae da loro e' il loro **tipo**.
///
/// Cosi' la nozione di forma resta una sola. Se un giorno l'autorita' ne
/// distinguesse una quarta, questa funzione la seguirebbe senza che nessuno se
/// ne ricordi.
fn forma_di_dominio(forma: FormaPanicSulFilo) -> crate::classificazione::FormaDelPayload {
    use crate::classificazione::FormaDelPayload;
    match forma {
        FormaPanicSulFilo::Statico => {
            let rappresentante: &'static str = "";
            FormaDelPayload::di(&rappresentante)
        }
        FormaPanicSulFilo::Dinamico => FormaDelPayload::di(&String::new()),
        FormaPanicSulFilo::NonTestuale => FormaDelPayload::di(&0_u8),
    }
}

mod coda;
mod conduzione;
mod produttori;
mod sorgente;

#[cfg(test)]
mod tests;
