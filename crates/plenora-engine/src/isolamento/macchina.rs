//! La macchina a stati del supervisore: fatti in una coda, un solo giudice.
//!
//! I produttori accodano **fatti**, mai conclusioni: nessuno di loro ha il
//! quadro intero. Un solo consumatore raccoglie e alla fine chiama
//! [`classifica`] **una volta**.
//!
//! La riduzione e' **commutativa** per costruzione: l'ordine d'arrivo dipende
//! dallo scheduler, quindi ogni fatto accende un campo e nessun campo dipende
//! da quando arriva. La precedenza fra le cause e' quella della §10.3,
//! applicata solo da [`classifica`].
//!
//! Il successo vuole quattro fatti distinti: l'`Esito` (un'affermazione del
//! worker, non una prova), l'**uscita** raccolta, l'**EOF** (nessuno tiene piu'
//! l'altro capo del canale) e la **quiescenza** del dominio. Nessuno
//! sostituisce gli altri, e cio' che manca diventa un tempo che finisce, mai un
//! successo.

use std::path::{Path, PathBuf};
use std::time::Duration;

use plenora_core::error::{
    ErrorCategory, ErrorPhase, EvidenzaDiLimite, PlenoraError, RemoteEffect, ReplayedError,
    RetryDisposition,
};

use crate::cancellation::CancellationToken;
use crate::classificazione::{classifica, EsitoClassificato, FattiDopoLaQuiescenza};
use crate::protocollo::handshake::HandshakeAccettato;
use crate::protocollo::messaggi::{
    CategoriaSulFilo, ConteggiDichiarati, Corpo, DiagnosticaSulFilo, DigestArtefatto,
    EffettoSulFilo, ErroreSulFilo, EsitoVerificaSulFilo, EsitoWorkerSulFilo, FaseSulFilo,
    FormaPanicSulFilo, RetrySulFilo,
};

use super::figlio::{FiglioVivo, ProcessoFiglio};
#[cfg(target_os = "linux")]
use super::sorgente::{interruttore, Freno, PASSO_DI_ATTESA};

/// Chi sta dall'altro capo del dominio, e quale corpo del protocollo chiude
/// il suo dialogo.
///
/// E' la fonte unica su **quale** `Corpo` chiude la conversazione:
/// `Corpo::Esito` per il worker, `Corpo::EsitoVerifica` per il verificatore
/// (vedi [`crate::protocollo::messaggi::EsitoVerificaSulFilo`]). L'altro corpo
/// arrivato qui e' un messaggio fuori posto per [`Registro::messaggio`], non un
/// esito. [`Self::nome`] rende il testo dei log dalla stessa fonte, cosi' nome
/// e scelta del corpo non divergono.
///
/// Il `Default` serve solo a `Registro::default()` nel cammino di rinuncia
/// (`conduzione::rinuncia`), che non produce mai un esito classificato; ovunque
/// il ruolo conti lo si passa esplicitamente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Ruolo {
    #[default]
    Worker,
    Verificatore,
}

impl Ruolo {
    /// Il nome per i messaggi di log e di errore.
    pub(super) const fn nome(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Verificatore => "verificatore",
        }
    }
}

/// Come il worker e' uscito, **osservato** e non interpretato.
///
/// Codice e segnale sono cio' che il sistema operativo riporta; che un codice
/// diverso da zero sia un problema lo decide chi classifica, non chi guarda.
///
/// Un enum e non due `Option`: «entrambi presenti» ed «entrambi assenti» non
/// sono uscite, e cosi' non si possono scrivere. L'assenza dell'osservazione e'
/// un fatto nostro, e si dice con [`Fatto::OsservazioneImpossibile`].
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
/// Si conserva la forma del filo perche' la conversione verso l'errore di
/// dominio perde qualcosa: il rapporto riporta cio' che il worker ha detto
/// davvero, e la perdita avviene in un posto solo.
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

    /// L'esito del **verificatore**, arrivato sul filo, senza giudizio.
    ///
    /// Riduce alla stessa forma di [`Self::dal_filo`]: da qui in poi il codice
    /// non distingue chi ha dichiarato l'esito. Che sia il corpo atteso lo ha
    /// gia' accertato [`Registro::messaggio`].
    fn dal_filo_verifica(esito: EsitoVerificaSulFilo) -> Self {
        match esito {
            EsitoVerificaSulFilo::Successo {
                digest_artefatto,
                conteggi,
            } => Self::Successo {
                digest: digest_artefatto,
                conteggi,
            },
            EsitoVerificaSulFilo::Errore { errore } => Self::Errore(errore),
            EsitoVerificaSulFilo::Panic { forma } => Self::Panic { forma },
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
    /// Distinto da `FineDelCanale`: l'EOF pulito e' uno dei fatti che il
    /// successo richiede, un troncamento no.
    CanaleInterrotto(String),
    /// Il worker e' uscito, ed e' stato raccolto.
    UscitaDelWorker(UscitaOsservata),
    /// Nel dominio non e' rimasto nessuno.
    DominioQuiescente,
    /// Cio' che il dominio dice della memoria, letto **dopo** la quiescenza.
    EvidenzaDelDominio(Box<EvidenzaDiLimite>),
    /// Il tempo dato all'esecuzione e' finito.
    ///
    /// Non dice la fase perche' questa macchina ne misura una sola: il timeout
    /// dell'handshake scade prima che `CanaleOperativo` esista, e lo misura chi
    /// guida l'handshake.
    TempoScaduto,
    /// Qualcuno ha chiesto di annullare.
    ///
    /// Il chiamante di produzione (`isolamento::esecuzione_isolata::esegui_isolato`)
    /// sorveglia lo stesso `CancellationToken` che il percorso in-process
    /// osserva, e chiede l'annullamento tramite [`produttori::Annullatore`]
    /// non appena lo vede cancellato — vedi `macchina::avvia_sorveglianza_esterna`.
    CancellazioneRichiesta,
    /// Un produttore **non e' riuscito a guardare**.
    ///
    /// Non e' un fatto sul worker: e' un fatto su di noi. Tenerlo separato
    /// impedisce che «non ho potuto vedere» si legga come «non c'e' niente».
    OsservazioneImpossibile { chi: &'static str, motivo: String },
}
/// Cio' che i fatti hanno acceso, senza ancora concludere niente.
///
/// Raccoglie invece di sovrascrivere: un campo sovrascritto arbitra in
/// silenzio, e sui fatti contraddittori produce un esito che sembra normale.
/// Due osservazioni uguali non cambiano niente; due diverse sono una
/// contraddizione e diventano un impedimento, mai un esito.
///
/// Cio' che si riporta e' **ordinato e senza ripetizioni**, perche' due
/// rapporti degli stessi fatti coincidano qualunque sia l'ordine d'arrivo.
#[expect(
    clippy::struct_excessive_bools,
    reason = "sono cinque osservazioni indipendenti, ognuna con la sua sorgente e il suo \
              significato: raggrupparle in un tipo comune direbbe che hanno qualcosa in \
              comune, e non ce l'hanno. Il lint protegge dai booleani che sono in realta' \
              uno stato; qui sono fatti"
)]
#[derive(Debug, Default)]
pub(super) struct Registro {
    /// Chi sta dall'altro capo, e quale corpo conta come l'esito che chiude
    /// il dialogo. Vedi [`Ruolo`] per il perche' e per il `Default`.
    ruolo: Ruolo,
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
    /// Un registro vuoto, per il ruolo dato.
    ///
    /// E' il costruttore che ogni dialogo di produzione e ogni caso che ne
    /// simula uno **devono** usare: dice esplicitamente quale corpo chiude
    /// questo dialogo, invece di lasciarlo al `Default` — che esiste solo per
    /// il cammino di rinuncia (vedi [`Ruolo`]).
    pub(super) fn nuovo(ruolo: Ruolo) -> Self {
        Self {
            ruolo,
            ..Self::default()
        }
    }

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

    /// Un messaggio dall'altro capo del dominio.
    ///
    /// Decide il ruolo (vedi [`Ruolo`]): l'esito del tipo non atteso finisce in
    /// `altri_messaggi` come qualunque messaggio fuori posto. E' fail-closed:
    /// senza un esito del tipo atteso `cosa_manca_alla_barriera` vede
    /// `esiti.len() != 1` e la barriera non si chiude su un successo.
    fn messaggio(&mut self, corpo: Corpo) {
        match (self.ruolo, corpo) {
            (Ruolo::Worker, Corpo::Esito(esito)) => {
                self.esiti.push(EsitoDichiarato::dal_filo(*esito));
            }
            (Ruolo::Verificatore, Corpo::EsitoVerifica(esito)) => {
                self.esiti.push(EsitoDichiarato::dal_filo_verifica(*esito));
            }
            _ => self.altri_messaggi += 1,
        }
    }

    /// Se i produttori hanno detto tutto quello che possono dire.
    ///
    /// Non pretende l'**uscita**: la osserva il conduttore **dopo** aver smesso
    /// di ascoltare. Aspettarla qui sarebbe aspettare se stessi, e ogni
    /// esecuzione finirebbe per timeout.
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
    /// Non pretende l'`Esito`: un worker che muore non dichiara nulla, e
    /// l'assenza e' un'informazione, non una condizione da attendere. Il canale
    /// conta come finito anche se si e' rotto.
    ///
    /// Introspezione dei casi: la conduzione vera si ferma con
    /// [`Self::si_puo_smettere_di_ascoltare`].
    #[cfg(any(test, feature = "internals"))]
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
    ///
    /// Introspezione dei casi, come [`Self::concluso`]: il giudizio vero e'
    /// [`classifica`], non un booleano.
    #[cfg(any(test, feature = "internals"))]
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
    /// **Piu' di un esito** viola il protocollo anche se i contenuti coincidono:
    /// l'esito chiude la conversazione. **Piu' di un'uscita diversa** e' una
    /// lettura nostra rotta, senza modo di sapere quale. **Piu' di una lettura
    /// dell'evidenza** non dovrebbe esistere: se ne fa una, dopo la quiescenza.
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
    /// Viaggia accanto all'errore e non dentro: [`DiagnosticaSulFilo`] non e'
    /// isomorfa a `RowDiagnostics`, e riempirne i campi mancanti inventerebbe
    /// osservazioni. E' un limite dichiarato: portarla fino a `RowDiagnostics`
    /// richiede un portatore tipizzato, non dei default.
    ///
    /// Solo quando l'esito e' uno: con piu' esiti sceglierne una sarebbe un
    /// arbitrato, e restano tutte nel rapporto.
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
    /// Consuma perche' la classificazione avviene una volta sola.
    ///
    /// `publish_completato` e' sempre falso: il publish non appartiene a questo
    /// perimetro (`PR-10`). La riga 1 della matrice resta irraggiungibile qui, e
    /// un successo dichiarato produce `DaVerificare`, cioe' «prosegui».
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
    /// Le contraddizioni si guardano prima: classificarle produrrebbe un esito
    /// che sembra normale.
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
        // Digest e conteggi non entrano in `classifica`, ma chi riceve
        // `DaVerificare` li confronta con l'artefatto riletto.
        let esito_dichiarato = self.esiti.first().and_then(|dichiarato| match dichiarato {
            EsitoDichiarato::Successo { digest, conteggi } => Some((digest.clone(), *conteggi)),
            EsitoDichiarato::Errore(_) | EsitoDichiarato::Panic { .. } => None,
        });

        // **La barriera precede la classificazione**, per ogni esito: su un
        // dominio ancora abitato l'evidenza si muove, e la stessa esecuzione
        // diventerebbe `Timeout` o `LimiteAttribuito` secondo quando il kernel
        // consegna un OOM.
        if !self.quiescente {
            return Err(Impedimento::BarrieraIncompleta(manca));
        }

        let classificato = classifica(self.in_fatti());

        // **Il resto della barriera governa il proseguire.** Le altre voci di
        // `manca` non impediscono di dire com'e' andata, impediscono di andare
        // avanti: `DaVerificare` e' un permesso verso verifica e publish, e la
        // §10.3 vieta di concederlo su un'esecuzione che non si e' vista
        // finire. Gli altri esiti restano quelli osservati.
        if matches!(classificato, EsitoClassificato::DaVerificare { .. }) && !manca.is_empty() {
            return Err(Impedimento::BarrieraIncompleta(manca));
        }

        Ok(EsitoDelSupervisore {
            classificato,
            rapporto,
            diagnostica_di_riga,
            esito_dichiarato,
        })
    }

    /// Che cosa manca perche' si possa proseguire.
    ///
    /// Un elenco **ordinato** e non un booleano: ogni voce manda chi legge a
    /// cercare in un posto diverso.
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
/// La diagnostica di riga sta accanto a `EsitoClassificato` e non nell'errore:
/// cosi' l'errore resta senza perdite sui quattro assi e la diagnostica resta
/// intera nella forma del filo (vedi `Registro::diagnostica_di_riga`).
#[derive(Debug)]
pub(super) struct EsitoDelSupervisore {
    /// Com'e' andata, secondo la precedenza della §10.3.
    pub(super) classificato: EsitoClassificato,
    /// Che cosa il registro aveva, riga per riga.
    ///
    /// Distingue esecuzioni con lo stesso esito ma fatti diversi, ed e' l'unica
    /// finestra sul registro che `concludi` consuma.
    pub(super) rapporto: Vec<(&'static str, String)>,
    /// Cio' che il worker ha osservato sulle righe, se lo ha detto.
    ///
    /// Nella forma del filo, che e' limitata per costruzione: il protocollo ne
    /// tetta esempi e conteggi, quindi tenerla non apre una via a una
    /// dimensione che il chiamante sceglie.
    pub(super) diagnostica_di_riga: Option<DiagnosticaSulFilo>,
    /// Il digest e i conteggi che il worker ha dichiarato, quando il primo
    /// esito e' un successo — indipendentemente da come `classificato` va a
    /// finire.
    ///
    /// E' il dato grezzo da confrontare con l'artefatto riletto. Non autorizza
    /// nulla da solo: si guarda `classificato` prima di usarlo.
    pub(super) esito_dichiarato: Option<(DigestArtefatto, ConteggiDichiarati)>,
}

/// L'evidenza del dominio con l'istantanea «prima» gia' presa, **prima dello
/// spawn**.
///
/// L'evidenza e' un delta: un «prima» preso dopo lo spawn assorbe un OOM
/// avvenuto nel frattempo, e l'esito diventa `Internal` invece di
/// `ResourceLimit` in silenzio. L'unico costruttore prende in prestito il
/// [`DominioPreparato`](super::DominioPreparato), che lo spawner consuma: il
/// compilatore non lascia prendere l'istantanea tardi. Dominio, radice e tetto
/// sono quelli canonici del preparato.
#[cfg(target_os = "linux")]
pub(super) struct EvidenzaDaPrimaDelloSpawn {
    lettore: adattatori::LeggiEvidenzaDominio,
    dominio: PathBuf,
}

#[cfg(target_os = "linux")]
impl EvidenzaDaPrimaDelloSpawn {
    /// Legge i contatori del dominio e dei suoi antenati adesso.
    pub(super) fn prendi(preparato: &super::DominioPreparato) -> Self {
        Self {
            lettore: adattatori::LeggiEvidenzaDominio::nuova(
                preparato.dominio.clone(),
                &preparato.radice,
                preparato.tetto_byte,
            ),
            dominio: preparato.dominio.clone(),
        }
    }

    /// Un fallimento del dialogo **prima** della conduzione — il
    /// supervisore che non si costruisce, l'handshake, l'incarico che non si
    /// scrive, il canale operativo che non si apre — riletto attraverso
    /// l'evidenza del dominio.
    ///
    /// Fra lo spawn e [`conduci_isolato`] un OOM si presenta come un canale
    /// chiuso; la §10.3 mette l'evidenza del dominio davanti all'esito del
    /// canale.
    ///
    /// Va chiamata dopo che il figlio e' stato raccolto, e legge a dominio
    /// **quiescente** (`F4-10`). Se la quiescenza non arriva entro
    /// [`conduzione::ATTESA_DELLA_QUIESCENZA`], termina con `cgroup.kill` e
    /// riattende: l'esito e' allora **ambiguo**, attribuito solo se l'evidenza
    /// lo attribuisce. Un dominio che non si svuota nemmeno cosi' non si legge.
    ///
    /// Rende la causa invariata quando l'evidenza non dice niente
    /// (`Assente`, `Indeterminata`) e il dominio si e' svuotato da se'.
    pub(super) fn rileggi_il_fallimento(self, soggetto: &str, causa: PlenoraError) -> PlenoraError {
        self.rileggi(soggetto, causa, Precedenza::DelDialogo)
    }

    /// Come [`Self::rileggi_il_fallimento`], per una **cancellazione**
    /// osservata prima della conduzione.
    ///
    /// Nella §10.3 la cancellazione cede soltanto all'OOM attribuito: una
    /// pressione non attribuita, o un'evidenza incoerente, lasciano
    /// `Cancelled`, perche' la cancellazione e' un fatto e loro no.
    pub(super) fn rileggi_la_cancellazione(
        self,
        soggetto: &str,
        causa: PlenoraError,
    ) -> PlenoraError {
        self.rileggi(soggetto, causa, Precedenza::DellaCancellazione)
    }

    /// Come [`Self::rileggi_il_fallimento`], per il **giudizio** del
    /// supervisore su una `Risposta` arrivata per intero: un'incompatibilita'
    /// accertata (righe 9 e 10). L'evidenza che dice qualcosa decide come per
    /// il dialogo; un dominio svuotato solo con `cgroup.kill` non rende invece
    /// ambiguo un fatto gia' accertato.
    pub(super) fn rileggi_il_giudizio(self, soggetto: &str, causa: PlenoraError) -> PlenoraError {
        self.rileggi(soggetto, causa, Precedenza::DelGiudizio)
    }

    fn rileggi(self, soggetto: &str, causa: PlenoraError, precedenza: Precedenza) -> PlenoraError {
        use crate::classificazione::{classifica_evidenza, ClasseEvidenzaMemoria};
        use conduzione::{LettoreDiEvidenza as _, Terminatore as _};
        use produttori::Osservatore as _;

        let mut osservatore = adattatori::SorvegliaDominio::nuova(self.dominio.clone());
        let terminato = if attendi_la_quiescenza(&mut osservatore) {
            false
        } else {
            let mut terminatore = adattatori::TerminaDominio::nuova(self.dominio.clone());
            if let Err(difetto) = terminatore.termina() {
                return PlenoraError::Internal(format!(
                    "il dominio isolato del {soggetto} non si e' svuotato dopo un fallimento \
                     precoce, e cgroup.kill non si scrive ({difetto}): processi possono restare \
                     nel dominio; causa del dialogo: {causa}"
                ));
            }
            if !attendi_la_quiescenza(&mut osservatore) {
                return PlenoraError::Internal(format!(
                    "il dominio isolato del {soggetto} non si e' svuotato nemmeno dopo \
                     cgroup.kill: processi possono restare nel dominio; causa del dialogo: \
                     {causa}"
                ));
            }
            true
        };
        let mut lettore = self.lettore;
        let classe = lettore
            .evidenza()
            .map(|prova| (classifica_evidenza(&prova), prova));
        match (classe, precedenza) {
            (Ok((ClasseEvidenzaMemoria::Attribuita, prova)), _) => {
                PlenoraError::ResourceLimit(format!(
                    "il dominio isolato del {soggetto} ha raggiunto il proprio tetto prima della \
                     conduzione: {prova:?}"
                ))
            }
            (
                Ok((ClasseEvidenzaMemoria::NonAttribuita, prova)),
                Precedenza::DelDialogo | Precedenza::DelGiudizio,
            ) => PlenoraError::UnattributedMemoryPressure {
                contesto: format!("dominio isolato del {soggetto}, prima della conduzione"),
                evidenza: Box::new(prova),
            },
            (
                Ok((ClasseEvidenzaMemoria::Incoerente, _)),
                Precedenza::DelDialogo | Precedenza::DelGiudizio,
            ) => PlenoraError::Internal(format!(
                "l'evidenza del dominio isolato del {soggetto} non e' utilizzabile: Incoerente"
            )),
            // Il dominio si e' svuotato solo con `cgroup.kill`, e l'evidenza
            // non attribuisce: la §10.3 chiama ambiguo questo esito, e la
            // causa del dialogo non lo spiega da sola. Il giudizio su una
            // `Risposta` intera non passa di qui: e' gia' un fatto.
            (_, Precedenza::DelDialogo) if terminato => PlenoraError::Internal(format!(
                "il dominio isolato del {soggetto} e' stato terminato con cgroup.kill dopo un \
                 fallimento precoce: esito ambiguo; causa del dialogo: {causa}"
            )),
            // Il resto lascia la causa: la cancellazione cede al solo OOM
            // attribuito, il dialogo a un'evidenza che dica qualcosa.
            _ => causa,
        }
    }
}

/// Quale causa cede all'evidenza, nella rilettura di un fallimento precoce.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
enum Precedenza {
    /// La causa del dialogo cede a ogni evidenza che dica qualcosa.
    DelDialogo,
    /// La cancellazione cede al solo OOM attribuito.
    DellaCancellazione,
    /// Il giudizio su una `Risposta` intera cede all'evidenza come il
    /// dialogo, ma non all'ambiguita' di `cgroup.kill`.
    DelGiudizio,
}

/// Attende la quiescenza del dominio fino a
/// [`conduzione::ATTESA_DELLA_QUIESCENZA`]. Una lettura interrotta si ripete;
/// ogni altro difetto di lettura vale come «non quiescente», che e' il verso
/// prudente: porta a `cgroup.kill`, non a leggere un'evidenza a meta'.
#[cfg(target_os = "linux")]
fn attendi_la_quiescenza(osservatore: &mut adattatori::SorvegliaDominio) -> bool {
    use produttori::Osservatore as _;

    let scadenza = std::time::Instant::now() + conduzione::ATTESA_DELLA_QUIESCENZA;
    loop {
        match osservatore.quiescente() {
            Ok(true) => return true,
            Ok(false) | Err(produttori::Difetto::Interrotta)
                if std::time::Instant::now() < scadenza =>
            {
                std::thread::sleep(PASSO_DI_ATTESA);
            }
            _ => return false,
        }
    }
}

/// Conduce un tentativo reale: dominio vero, worker vero, evidenza vera —
/// non i finti di `conduzione::tests`.
///
/// Costruisce il [`produttori::CanaleOperativo`] dall'accordo gia' concluso
/// (l'handshake lo compie chi chiama), gli adattatori reali del dominio e i
/// [`conduzione::Dintorni`], poi chiama [`conduzione::conduci`] e traduce il
/// suo esito.
///
/// Rende digest e conteggi dichiarati **solo** quando la conduzione raggiunge
/// `DaVerificare`: il permesso di procedere alla verifica indipendente, non la
/// pubblicazione (`PR-10`). Ogni altro esito diventa un errore classificato.
///
/// `annullamento_esterno` (per esempio l'handler Ctrl-C della CLI) e'
/// sorvegliato da un filo che nasce in `consegna_annullatore` e chiede
/// l'annullamento tramite [`produttori::Annullatore`]. Se il filo non nasce la
/// conduzione prosegue: la cancellazione e' cooperativa e senza promessa di
/// immediatezza (`errori-e-limiti.md#cancellazione`).
///
/// `ruolo` nomina chi sta dall'altro capo nei messaggi e sceglie il corpo che
/// chiude il dialogo (vedi [`Registro::messaggio`]); la macchina resta
/// generica, e un parametro evita due copie che divergano proprio sulla
/// classificazione. `evidenza` si ottiene solo da
/// [`EvidenzaDaPrimaDelloSpawn::prendi`].
///
/// # Errors
///
/// L'errore classificato, con la categoria che [`EsitoClassificato::categoria`]
/// gia' assegna; oppure [`PlenoraError::Internal`] se i fatti stessi non si
/// sono lasciati ridurre a un esito ([`Impedimento`]) — un difetto
/// dell'osservazione, non del worker.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
pub(super) fn conduci_isolato<P: ProcessoFiglio>(
    ruolo: Ruolo,
    lettore: std::io::PipeReader,
    accordo: HandshakeAccettato,
    scrittore: std::io::PipeWriter,
    tempo_di_esecuzione: Duration,
    guardia: FiglioVivo<P>,
    dominio: PathBuf,
    evidenza: EvidenzaDaPrimaDelloSpawn,
    annullamento_esterno: CancellationToken,
) -> std::result::Result<(DigestArtefatto, ConteggiDichiarati), PlenoraError> {
    let soggetto = ruolo.nome();
    // Un errore qui non puo' uscire con `?`: lascerebbe cadere `guardia` con
    // il figlio vivo, e la sentinella di `FiglioVivo` interromperebbe il
    // processo. Il figlio si chiude, e il fallimento passa dall'evidenza.
    let canale = match produttori::CanaleOperativo::dal_supervisore(lettore, accordo) {
        Ok(canale) => canale,
        Err(causa) => {
            let (_uscita, difetti) = super::prova::chiudi(guardia, Some(&causa));
            for difetto in &difetti {
                eprintln!("plenora: pulizia del {soggetto} isolato: {difetto}");
            }
            return Err(evidenza.rileggi_il_fallimento(soggetto, causa));
        }
    };
    let osservatore = adattatori::SorvegliaDominio::nuova(dominio.clone());
    let terminatore = adattatori::TerminaDominio::nuova(dominio);
    let evidenza = evidenza.lettore;
    let dintorni = conduzione::Dintorni {
        ruolo,
        osservatore,
        terminatore,
        evidenza,
        figlio: guardia,
        tetto_del_drenaggio: coda::Coda::tetto_di_produzione(),
        margine_di_cortesia: conduzione::MARGINE_DI_CORTESIA,
        attesa_della_quiescenza: conduzione::ATTESA_DELLA_QUIESCENZA,
    };
    let mut sorveglianza = None;
    let (esito, contorno) = conduzione::conduci(
        canale,
        scrittore,
        tempo_di_esecuzione,
        dintorni,
        |annullatore| {
            sorveglianza = avvia_sorveglianza_esterna(annullamento_esterno, annullatore);
        },
    );
    if let Some((freno, filo)) = sorveglianza {
        freno.ferma();
        if filo.join().is_err() {
            eprintln!("plenora: il filo di sorveglianza dell'annullamento e' andato in panico");
        }
    }
    segnala_pulizia_della_conduzione(&contorno);

    // Un dominio che non si e' lasciato terminare, o che terminato non si e'
    // svuotato, e' un fatto dell'esito, non solo del log: processi possono
    // esservi rimasti, e l'errore lo dice.
    let residuo = contorno
        .terminazione
        .as_ref()
        .or(contorno.abitato.as_ref())
        .map_or_else(String::new, |motivo| {
            format!("; il dominio non si e' lasciato svuotare ({motivo}): processi possono esservi rimasti")
        });
    let supervisore = esito.map_err(|impedimento| {
        PlenoraError::Internal(format!(
            "la conduzione del profilo isolato non si e' lasciata ridurre a un esito: {}{residuo}",
            messaggio_di_impedimento(impedimento)
        ))
    })?;

    for (chiave, valore) in &supervisore.rapporto {
        eprintln!("plenora: conduzione del profilo isolato ({soggetto}), {chiave}: {valore}");
    }
    if let Some(diagnostica) = &supervisore.diagnostica_di_riga {
        eprintln!("plenora: diagnostica di riga del {soggetto} isolato: {diagnostica:?}");
    }
    // Un solo punto legge evidenza e categoria, qualunque sia l'esito.
    if !supervisore.classificato.pubblica() {
        if let Some(prova) = supervisore.classificato.evidenza() {
            eprintln!(
                "plenora: evidenza del dominio isolato ({soggetto}, categoria {:?}): {prova:?}",
                supervisore.classificato.categoria()
            );
        }
    }

    interpreta_classificato(
        soggetto,
        supervisore.classificato,
        supervisore.esito_dichiarato,
        tempo_di_esecuzione,
    )
}

/// Avvia il filo che sorveglia `annullamento_esterno`, e chiede
/// l'annullamento tramite `annullatore` appena lo vede cancellato.
///
/// Rende `None` se il filo non nasce: un guasto del sistema, riportato
/// (`eprintln!`) invece che fatto fallire l'intero tentativo, per la ragione
/// gia' detta su [`conduci_isolato`].
#[cfg(target_os = "linux")]
fn avvia_sorveglianza_esterna(
    annullamento_esterno: CancellationToken,
    annullatore: &std::sync::Arc<produttori::Annullatore>,
) -> Option<(Freno, std::thread::JoinHandle<()>)> {
    let (spia, freno) = interruttore();
    let annullatore = std::sync::Arc::clone(annullatore);
    match std::thread::Builder::new()
        .name("plenora-sorveglianza-annullamento".to_owned())
        .spawn(move || {
            while !spia.fermato() {
                if annullamento_esterno.is_cancelled() {
                    match annullatore.annulla() {
                        produttori::EsitoDellAnnullamento::Accodata
                        | produttori::EsitoDellAnnullamento::GiaDeposta => {}
                        produttori::EsitoDellAnnullamento::NonAccodata(motivo) => {
                            eprintln!(
                                "plenora: la richiesta di annullamento non e' entrata in coda: \
                                 {motivo}"
                            );
                        }
                    }
                    return;
                }
                std::thread::sleep(PASSO_DI_ATTESA);
            }
        }) {
        Ok(filo) => Some((freno, filo)),
        Err(causa) => {
            eprintln!(
                "plenora: il filo di sorveglianza dell'annullamento non nasce: {causa}; questo \
                 tentativo procede senza cancellazione esterna"
            );
            None
        }
    }
}

/// Riporta cio' che la conduzione ha lasciato da pulire — mai un secondo
/// motivo di rifiuto, solo diagnostica accanto all'esito.
#[cfg(target_os = "linux")]
fn segnala_pulizia_della_conduzione<P: ProcessoFiglio>(contorno: &conduzione::Contorno<P>) {
    for resoconto in &contorno.resoconti {
        eprintln!("plenora: conduzione del profilo isolato: {resoconto}");
    }
    for motivo in [
        &contorno.drenaggio,
        &contorno.raccolta,
        &contorno.terminazione,
        &contorno.abitato,
        &contorno.annulla,
    ]
    .into_iter()
    .flatten()
    {
        eprintln!("plenora: pulizia della conduzione isolata: {motivo}");
    }
    // Un figlio non raccolto e' ancora sotto guardia: lasciarlo cadere fa
    // scattare la sentinella di `FiglioVivo` con un abort, e il log lo
    // annuncia prima.
    if contorno.figlio_non_raccolto.is_some() {
        eprintln!(
            "plenora: il worker isolato non si e' lasciato raccogliere: il processo abortira'"
        );
    }
}

/// Il motivo per cui i fatti non si sono lasciati ridurre a un esito.
#[cfg(target_os = "linux")]
fn messaggio_di_impedimento(impedimento: Impedimento) -> String {
    match impedimento {
        Impedimento::ProduttoreNonNato { chi, motivo } => {
            format!("il produttore «{chi}» non e' nato: {motivo}")
        }
        Impedimento::BarrieraIncompleta(manca) => {
            format!("la barriera prima della classificazione non e' completa: {manca:?}")
        }
        Impedimento::FattiContraddittori(contraddizioni) => {
            format!("i fatti osservati si contraddicono: {contraddizioni:?}")
        }
    }
}

/// Traduce la classificazione della §10 nell'esito che il chiamante di
/// produzione rende: il digest dichiarato solo per `DaVerificare`, un
/// errore classificato per ogni altro esito.
#[cfg(target_os = "linux")]
fn interpreta_classificato(
    soggetto: &str,
    classificato: EsitoClassificato,
    esito_dichiarato: Option<(DigestArtefatto, ConteggiDichiarati)>,
    tempo_di_esecuzione: Duration,
) -> std::result::Result<(DigestArtefatto, ConteggiDichiarati), PlenoraError> {
    match classificato {
        EsitoClassificato::DaVerificare { .. } => esito_dichiarato.ok_or_else(|| {
            PlenoraError::Internal(format!(
                "la conduzione concede DaVerificare ma non porta il digest dichiarato ({soggetto})"
            ))
        }),
        EsitoClassificato::LimiteAttribuito(evidenza) => Err(PlenoraError::ResourceLimit(format!(
            "il dominio isolato del {soggetto} ha raggiunto il proprio tetto: {evidenza:?}"
        ))),
        EsitoClassificato::Timeout { .. } => Err(PlenoraError::Timeout(format!(
            "il {soggetto} isolato non ha concluso entro {} secondi",
            tempo_di_esecuzione.as_secs()
        ))),
        // `Cancelled`, non `Internal`: stessa categoria ed exit code del
        // percorso non isolato. `node`/`operation` portano il soggetto isolato
        // e la fase, non un nodo del DAG; `execution_id` resta vuoto (vedi
        // `PlenoraError::execution_location`).
        EsitoClassificato::Cancellato { .. } => Err(PlenoraError::Cancelled {
            node: soggetto.to_owned(),
            operation: "dominio isolato".to_owned(),
            execution_id: String::new(),
            reason: format!("l'esecuzione isolata del {soggetto} e' stata cancellata"),
        }),
        EsitoClassificato::PressioneNonAttribuita(evidenza) => {
            Err(PlenoraError::UnattributedMemoryPressure {
                contesto: format!("dominio isolato del {soggetto}, dopo la quiescenza"),
                evidenza,
            })
        }
        EsitoClassificato::EvidenzaNonUtilizzabile { classe, .. } => {
            Err(PlenoraError::Internal(format!(
                "l'evidenza del dominio isolato del {soggetto} non e' utilizzabile: {classe:?} \
                 (categoria {:?})",
                classe.categoria()
            )))
        }
        EsitoClassificato::ErroreDelWorker { errore, evidenza } => {
            if let Some(prova) = evidenza {
                eprintln!(
                    "plenora: evidenza del dominio isolato ({soggetto}) con errore dichiarato: \
                     {prova:?}"
                );
            }
            Err(errore)
        }
        EsitoClassificato::PanicDelWorker { forma, evidenza } => {
            if let Some(prova) = evidenza {
                eprintln!(
                    "plenora: evidenza del dominio isolato ({soggetto}) con panico dichiarato: \
                     {prova:?}"
                );
            }
            Err(PlenoraError::Internal(format!(
                "il {soggetto} isolato e' andato in panico: {forma:?}"
            )))
        }
        EsitoClassificato::TerminazioneAmbigua { evidenza } => {
            if let Some(prova) = evidenza {
                eprintln!(
                    "plenora: evidenza del dominio isolato ({soggetto}) con terminazione \
                     ambigua: {prova:?}"
                );
            }
            Err(PlenoraError::Internal(format!(
                "il {soggetto} isolato e' terminato in modo ambiguo"
            )))
        }
        EsitoClassificato::Pubblicato { evidenza } => {
            if let Some(prova) = evidenza {
                eprintln!(
                    "plenora: evidenza del dominio isolato ({soggetto}) su Pubblicato: {prova:?}"
                );
            }
            Err(PlenoraError::Internal(format!(
                "la conduzione ({soggetto}) ha reso Pubblicato, che questo percorso non produce \
                 mai"
            )))
        }
    }
}

/// Perche' dai fatti non esce un esito.
///
/// Non e' una variante dell'esito: non dice com'e' andata l'esecuzione, dice
/// che la **nostra osservazione** si e' rotta, e perche'.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Impedimento {
    /// Un produttore non e' nato: l'esecuzione non e' stata **osservata**.
    ///
    /// Non significa «il tentativo non e' cominciato»: il worker esiste gia',
    /// e chi rinuncia deve chiudere il dominio, raccogliere il figlio e drenare
    /// cio' che e' arrivato, come su ogni altro cammino.
    ProduttoreNonNato { chi: &'static str, motivo: String },
    /// La barriera causale non e' completa.
    ///
    /// Senza **quiescenza** i contatori della memoria si muovono ancora (il
    /// prototipo li misura cambiare dopo il ritorno della `wait`), e nessun
    /// esito significa quello che dice. Le altre voci impediscono di
    /// proseguire verso `DaVerificare`. Declassare a «terminazione ambigua»
    /// manderebbe chi legge a cercare un worker morto male.
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
/// Un `match` esaustivo scritto a mano: una variante nuova da una parte non
/// compila finche' non la si mappa, dove una conversione per nome fallirebbe a
/// runtime.
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
/// [`PlenoraError::Replayed`] porta i quattro assi cosi' come arrivano, senza
/// ricalcolarli da una variante scelta per categoria. `execution_reason` e'
/// `None` perche' il protocollo non lo trasporta.
///
/// Senza perdite sugli assi, non in assoluto: la diagnostica di riga la
/// possiede [`EsitoDelSupervisore::diagnostica_di_riga`], e il limite e'
/// registrato in `errori-e-limiti.md`.
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

/// La forma del panico, dal filo al dominio: un `match` esaustivo su due enum
/// chiusi, senza contenuto da portare.
const fn forma_di_dominio(forma: FormaPanicSulFilo) -> crate::classificazione::FormaDelPayload {
    use crate::classificazione::FormaDelPayload;
    use plenora_core::panic_policy::FormaPayload;
    FormaDelPayload::da(match forma {
        FormaPanicSulFilo::Statico => FormaPayload::Statico,
        FormaPanicSulFilo::Dinamico => FormaPayload::Dinamico,
        FormaPanicSulFilo::NonTestuale => FormaPayload::NonTestuale,
    })
}

#[cfg(target_os = "linux")]
mod adattatori;
mod coda;
mod conduzione;
mod produttori;

#[cfg(test)]
mod tests;
