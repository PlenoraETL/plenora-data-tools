//! I quattro produttori: guardano, e accodano cio' che vedono.
//!
//! # Che cosa hanno in comune, e non e' un caso
//!
//! Tutti e quattro finiscono allo stesso modo: **rendono il proprio resoconto**
//! e lasciano cadere la bocchetta. Non e' disciplina, e' il tipo — `resoconto`
//! consuma la bocchetta — e serve a due cose che il consumatore non potrebbe
//! ottenere altrimenti.
//!
//! La prima e' che il canale si disconnetta: finche' una bocchetta vive, il
//! drenaggio finale non vede `Disconnected`.
//!
//! La seconda e' che un rifiuto torni **fuori dalla coda**. Un produttore che
//! non riesce ad accodare non puo' dirlo accodando: il resoconto passa dal
//! `JoinHandle`, che e' una via che non si riempie.
//!
//! # E che cosa non hanno in comune
//!
//! Nessuno di loro decide niente. Il lettore non dice «il worker ha finito»,
//! l'orologio non dice «e' un timeout», il sorvegliante non dice «e' andata
//! bene»: dicono cosa hanno visto, e il giudizio sta tutto nel consumatore, che
//! e' l'unico ad avere il quadro.

use std::io::Read;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::protocollo::handshake::HandshakeAccettato;
use crate::protocollo::lettore::leggi_frame;
use crate::protocollo::messaggi::{Corpo, Progresso};

use super::coda::{Bocchetta, Esaurita};
use crate::isolamento::sorgente::{Freno, Interruttore, SorgenteTerminabile};
use super::Fatto;

/// Cio' che un produttore rende quando finisce.
///
/// `None` se ha detto tutto quello che aveva da dire.
pub(super) type Resoconto = Option<Esaurita>;

/// A che punto e' la conversazione, dal lato di chi ascolta.
///
/// # Perche' il lettore ha uno stato
///
/// Perche' «fuori sequenza» non e' una proprieta' del messaggio ma del momento
/// in cui arriva: un `Progresso` va benissimo prima dell'esito e non ha senso
/// dopo, e senza sapere dove si e' arrivati non si puo' dire quale delle due
/// cose sia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PuntoDellaConversazione {
    /// Si accettano progressi, e un esito.
    InCorso,
    /// L'esito e' arrivato: non deve arrivare altro.
    Concluso,
}

/// Il progresso, **conservato** invece che inoltrato.
///
/// # Perche' si conserva l'ultimo invece di sommare
///
/// Perche' i contatori del `Progresso` sono **totali**, non incrementi: ogni
/// rapporto dice quanto si e' fatto fin li'. L'ultimo li contiene tutti, e
/// tenere solo lui non perde niente.
///
/// Sommarli sarebbe sbagliato due volte. Darebbe un numero che non significa
/// nulla — la somma di sette letture cumulative non e' un conteggio di niente —
/// e aprirebbe la strada al traboccamento, le cui due uscite sono entrambe
/// cattive: saturare rende `u64::MAX` indistinguibile da un conteggio esatto
/// pari a `u64::MAX`, cioe' una perdita **silenziosa**; andare in panico mette
/// il supervisore in ginocchio per un numero che ha scelto il worker.
///
/// # Perche' coalescere serve comunque
///
/// Perche' quante volte il worker riporta lo sceglie lui, e un fatto per
/// rapporto gli lascerebbe decidere quanto spazio occupare in coda. Conservando
/// l'ultimo, cio' che arriva al consumatore e' **un fatto solo**.
#[derive(Debug, Default, Clone, Copy)]
struct ProgressoOsservato {
    ultimo: Option<Progresso>,
}

impl ProgressoOsservato {
    /// Prende il rapporto, se non fa marcia indietro.
    ///
    /// # Errors
    ///
    /// Il motivo, se uno dei tre assi e' regredito: e' una violazione del
    /// protocollo, non un rapporto strano, perche' i contatori sono totali e un
    /// totale non torna indietro.
    fn osserva(&mut self, quanto: Progresso) -> std::result::Result<(), String> {
        if let Some(prima) = self.ultimo {
            for (asse, vecchio, nuovo) in [
                ("righe", prima.righe, quanto.righe),
                ("batch", prima.batch, quanto.batch),
                (
                    "nodi completati",
                    prima.nodi_completati,
                    quanto.nodi_completati,
                ),
            ] {
                if nuovo < vecchio {
                    return Err(format!(
                        "il contatore «{asse}» e' passato da {vecchio} a {nuovo}, e i contatori \
                         del progresso sono totali: un totale non torna indietro"
                    ));
                }
            }
        }
        self.ultimo = Some(quanto);
        Ok(())
    }

    /// Il fatto da accodare, se c'e' stato almeno un rapporto.
    fn in_fatto(self) -> Option<Fatto> {
        self.ultimo
            .map(|ultimo| Fatto::MessaggioDalWorker(Box::new(Corpo::Progresso(ultimo))))
    }
}

/// Il canale **dopo** che l'handshake e' stato consumato.
///
/// # Perche' il confine sta nel tipo
///
/// Perche' la sequenza che il lettore accetta — progressi, poi l'esito — e'
/// giusta **solo dopo** che la `Risposta` e' stata letta e verificata. Su un
/// canale grezzo il primo messaggio e' la `Risposta`, e un lettore che partisse
/// da li' la troverebbe fuori sequenza: rifiuterebbe una conversazione
/// perfettamente valida, e lo farebbe per un errore di chi lo ha avviato.
///
/// Un commento che dicesse «avviarlo dopo l'handshake» non basterebbe: chi
/// scrive il chiamante lo legge una volta e poi non piu'. Qui la prova e' un
/// valore — [`HandshakeAccettato`] — che **solo** l'handshake produce, e senza
/// il quale questo tipo non si costruisce.
pub(super) struct CanaleOperativo<R: Read> {
    sorgente: R,
    /// La prova, tenuta perche' esista e non perche' si legga.
    ///
    /// Il token che porta appartiene al publish, che qui non c'e': cio' che
    /// serve a questo modulo e' che il valore **sia stato ottenuto**, e per
    /// ottenerlo bisogna essere passati dall'handshake.
    _accordo: HandshakeAccettato,
}

impl<R: Read> CanaleOperativo<R> {
    /// Il canale, dopo l'accordo, su una sorgente qualunque.
    ///
    /// # Perche' vive solo nei casi
    ///
    /// Perche' una sorgente qualunque non si puo' rendere non bloccante, e un
    /// lettore che resta dentro una `read` non si sveglia quando qualcuno frena.
    /// In produzione la sorgente e' un descrittore, e c'e' un costruttore che se
    /// ne occupa; qui non c'e' descrittore, e i casi che leggono da un vettore
    /// di byte non hanno nulla da bloccare.
    ///
    /// Tenerlo disponibile alla produzione vorrebbe dire lasciare aperta la
    /// strada per costruire un lettore che non si puo' fermare — e la si
    /// prenderebbe senza accorgersene, perche' compila.
    #[cfg(test)]
    pub(super) const fn dopo_l_accordo(sorgente: R, accordo: HandshakeAccettato) -> Self {
        Self {
            sorgente,
            _accordo: accordo,
        }
    }
}

#[cfg(target_os = "linux")]
impl CanaleOperativo<std::io::PipeReader> {
    /// Il canale del supervisore, **reso non bloccante**.
    ///
    /// # Perche' qui e non nel lettore
    ///
    /// Perche' il lettore e' generico su cio' che legge, e su una sorgente
    /// generica non c'e' niente da mettere in modalita' non bloccante. Se lo
    /// facesse lui, dovrebbe farlo «quando puo'» — cioe' mai in modo
    /// verificabile.
    ///
    /// Qui invece il tipo e' un descrittore, e questo costruttore e' l'**unico**
    /// modo di ottenere un canale operativo di produzione: chi lo usa non puo'
    /// dimenticarsene, perche' non c'e' un'altra porta.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::IsolationUnavailable`] se i flag non si leggono o non si
    /// riscrivono.
    pub(super) fn dal_supervisore(
        sorgente: std::io::PipeReader,
        accordo: HandshakeAccettato,
    ) -> plenora_core::error::Result<Self> {
        use std::os::fd::AsFd as _;
        crate::isolamento::sorgente::rendi_non_bloccante(sorgente.as_fd())?;
        Ok(Self {
            sorgente,
            _accordo: accordo,
        })
    }
}

/// Legge il canale del worker, e accoda quello che ne viene.
///
/// # La sequenza che accetta
///
/// Zero o piu' `Progresso`, poi al piu' un `Esito`, poi la fine. Tutto il resto
/// e' **fuori sequenza**: un secondo esito, un progresso dopo l'esito, un
/// messaggio di un tipo che a questo stadio il worker non manda.
///
/// # Che cosa fa quando la sequenza si rompe
///
/// Accoda **un** fatto di protocollo e smette. Non uno per messaggio
/// inaspettato: quanti ne manda lo sceglie il worker, e inoltrarli tutti gli
/// lascerebbe decidere quanto spazio occupare in coda — proprio la cosa da cui
/// il budget protegge. Il primo dice gia' tutto: da li' in poi la conversazione
/// non e' piu' quella che il protocollo descrive.
///
/// # Che cosa rende
///
/// Il filo, e il freno per fermarlo. Il filo rende il resoconto della propria
/// bocchetta.
pub(super) fn avvia_lettore<R: Read + Send + 'static>(
    canale: CanaleOperativo<R>,
    bocchetta: Bocchetta,
) -> std::io::Result<(JoinHandle<Resoconto>, Freno)> {
    let (mut terminabile, freno) = SorgenteTerminabile::nuova(canale.sorgente);
    let filo = nato("plenora-lettore", move || {
        let mut bocchetta = bocchetta;
        let mut progresso = ProgressoOsservato::default();
        let mut punto = PuntoDellaConversazione::InCorso;
        let (fatto_finale, mut rifiuto) =
            fine_del_canale(&mut terminabile, &mut bocchetta, &mut progresso, &mut punto);

        // Il progresso sommato si accoda **prima** della fine: cosi' chi legge
        // la coda incontra il lavoro fatto e poi la sua conclusione, che e'
        // l'ordine in cui sono successi.
        if let Some(fatto) = progresso.in_fatto() {
            if let Err(quale) = bocchetta.manda(fatto) {
                rifiuto = rifiuto.or(Some(quale));
            }
        }
        if let Err(quale) = bocchetta.manda(fatto_finale) {
            rifiuto = rifiuto.or(Some(quale));
        }
        bocchetta.resoconto().or(rifiuto)
    })?;
    Ok((filo, freno))
}

/// Fa nascere un filo, **e ammette che possa non nascere**.
///
/// # Perche' non `std::thread::spawn`
///
/// Perche' `spawn` va in **panico** quando il sistema rifiuta un thread — un
/// limite di processi raggiunto, memoria finita — e un panico li' e' il posto
/// peggiore in cui scoprirlo: succede mentre i produttori stanno nascendo, cioe'
/// quando alcuni sono gia' vivi e il figlio e' gia' avviato. Chi lo subisce non
/// vede un errore, vede un supervisore che sparisce.
///
/// `Builder::spawn` rende invece un `Result`, e un rifiuto diventa una cosa da
/// riportare e da cui tornare indietro.
///
/// # Errors
///
/// Cio' che il sistema dice del rifiuto.
fn nato<T: Send + 'static>(
    nome: &str,
    corpo: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<JoinHandle<T>> {
    #[cfg(test)]
    if inciampo::tocca_a_questa() {
        return Err(std::io::Error::other(
            "nascita rifiutata dalla qualificazione",
        ));
    }
    std::thread::Builder::new()
        .name(nome.to_owned())
        .spawn(corpo)
}

/// Far rifiutare una nascita **a comando**.
///
/// # Perche' esiste, e perche' solo nei casi
///
/// Perche' il cammino della rinuncia si percorre soltanto quando il sistema
/// rifiuta un thread, e il sistema lo fa quando e' esaurito. Un caso che
/// volesse arrivarci sul serio dovrebbe portare la macchina in quello stato:
/// non e' una prova, e' un guasto.
///
/// Senza questa giuntura restano provabili le **conseguenze** della rinuncia —
/// chiamandola direttamente — ma non i suoi tre **cablaggi**: che `conduci` la
/// invochi dopo il lettore, dopo l'orologio e dopo il sorvegliante, ogni volta
/// col nome giusto e con cio' che a quel punto esiste. Tre punti di chiamata
/// sono tre occasioni di divergere, e quella che diverge e' sempre l'ultima.
///
/// Sta sotto `cfg(test)` e non dietro una feature. La differenza non e' che
/// `cfg(test)` sia inaccessibile — chi controlla la build puo' selezionare i
/// `cfg` che vuole — ma che **non appartiene a nessun percorso di compilazione
/// previsto**: nessun profilo Cargo ordinario lo include, e nessun consumatore
/// della libreria puo' chiederlo dal proprio `Cargo.toml`. Una feature, invece,
/// e' fatta apposta per essere scelta da chi dipende da noi. Fuori dai casi il
/// modulo non esiste, e `nato` non ha nulla da chiedergli.
///
/// # Perche' un turno **e** il filo che arma
///
/// Perche' i casi girano in parallelo, e ci vogliono tutte e due le difese.
///
/// Il turno serve perche' l'arma e' una: due casi che armassero insieme si
/// sovrascriverebbero, e il secondo farebbe fallire una nascita che il primo
/// stava aspettando — il primo resterebbe verde senza aver misurato niente.
/// Chi arma tiene il turno finche' non ha finito.
///
/// Il filo serve perche' il turno non basta: mentre un caso e' armato, un caso
/// **non armato** puo' far nascere i suoi produttori nello stesso momento e
/// consumare il conteggio. Le nascite si contano quindi per filo, e `nato` viene
/// sempre chiamato dal filo che conduce — che e' il filo del caso.
#[cfg(test)]
pub(super) mod inciampo {
    /// Il turno: uno solo arma per volta.
    static TURNO: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Chi ha armato, quale nascita deve fallire, e quante ne sono state chieste.
    static ARMATO: std::sync::Mutex<Option<(std::thread::ThreadId, usize, usize)>> =
        std::sync::Mutex::new(None);

    /// L'arma, che si disinnesca da sola.
    ///
    /// Tenerla viva e' cio' che tiene armato il guasto; lasciarla cadere lo
    /// spegne. Un ripristino da scrivere a mano si dimentica, e un caso
    /// successivo troverebbe una nascita che fallisce senza averlo chiesto.
    pub(in crate::isolamento::macchina) struct Armato {
        /// Il turno, tenuto finche' l'arma vive.
        _turno: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for Armato {
        fn drop(&mut self) {
            *ARMATO
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }

    /// Fa fallire la `quale`-esima nascita chiesta da **questo** filo.
    ///
    /// Si conta da uno: `1` e' il lettore, `2` l'orologio, `3` il sorvegliante.
    pub(in crate::isolamento::macchina) fn fai_fallire_la_nascita(quale: usize) -> Armato {
        let turno = TURNO
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *ARMATO
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((std::thread::current().id(), quale, 0));
        Armato { _turno: turno }
    }

    /// Se la nascita che si sta chiedendo adesso e' quella da far fallire.
    pub(super) fn tocca_a_questa() -> bool {
        // La presa si rilascia **prima** di rendere: `nato` sta per chiamare
        // `Builder::spawn`, e tenere un lucchetto attraverso una nascita e' il
        // modo di scoprire un giorno che due casi si aspettano a vicenda.
        let tocca = {
            let mut armato = ARMATO
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match armato.as_mut() {
                Some((chi, quale, chieste)) if *chi == std::thread::current().id() => {
                    *chieste += 1;
                    *chieste == *quale
                }
                _ => false,
            }
        };
        tocca
    }
}

/// Il giro di lettura: rende il fatto con cui il canale finisce, e l'eventuale
/// rifiuto incontrato per strada.
fn fine_del_canale<R: Read>(
    sorgente: &mut SorgenteTerminabile<R>,
    bocchetta: &mut Bocchetta,
    progresso: &mut ProgressoOsservato,
    punto: &mut PuntoDellaConversazione,
) -> (Fatto, Resoconto) {
    loop {
        match leggi_frame(sorgente) {
            Ok(None) => return (Fatto::FineDelCanale, None),
            Ok(Some(frame)) => match (*punto, frame.in_corpo()) {
                (PuntoDellaConversazione::InCorso, Corpo::Progresso(quanto)) => {
                    if let Err(motivo) = progresso.osserva(quanto) {
                        // Una regressione e' una violazione del protocollo, e si
                        // tratta come le altre: **un** fatto, e si smette.
                        return (Fatto::CanaleInterrotto(motivo), None);
                    }
                }
                (PuntoDellaConversazione::InCorso, Corpo::Esito(esito)) => {
                    *punto = PuntoDellaConversazione::Concluso;
                    if let Err(rifiuto) =
                        bocchetta.manda(Fatto::MessaggioDalWorker(Box::new(Corpo::Esito(esito))))
                    {
                        return (
                            Fatto::CanaleInterrotto("l'esito non si e' potuto accodare".to_owned()),
                            Some(rifiuto),
                        );
                    }
                }
                (_, altro) => {
                    return (
                        Fatto::CanaleInterrotto(format!(
                            "messaggio fuori sequenza: «{}» dopo che la conversazione e' {}",
                            nome_del_corpo(&altro),
                            se_conclusa(*punto)
                        )),
                        None,
                    )
                }
            },
            Err(errore) => {
                return (
                    Fatto::CanaleInterrotto(format!("il canale non si legge: {errore}")),
                    None,
                )
            }
        }
    }
}

/// Il nome del tipo di messaggio, per l'evidenza.
const fn nome_del_corpo(corpo: &Corpo) -> &'static str {
    match corpo {
        Corpo::Saluto(_) => "saluto",
        Corpo::Incarico(_) => "incarico",
        Corpo::Annulla(_) => "annulla",
        Corpo::Risposta(_) => "risposta",
        Corpo::Progresso(_) => "progresso",
        Corpo::Esito(_) => "esito",
    }
}

/// Come si dice il punto della conversazione, nel messaggio di rifiuto.
const fn se_conclusa(punto: PuntoDellaConversazione) -> &'static str {
    match punto {
        PuntoDellaConversazione::InCorso => "ancora in corso",
        PuntoDellaConversazione::Concluso => "gia' conclusa dall'esito",
    }
}

/// Misura il tempo dell'esecuzione, e dice quando e' finito.
///
/// # Perche' un tempo solo
///
/// Perche' l'altro non appartiene a questa macchina. Il timeout dell'handshake
/// misura dall'avvio alla `Risposta`, e quell'intervallo si chiude **prima** che
/// il canale operativo esista — chi lo misura e' chi guida l'handshake. Averne
/// due qui, percorsi in sequenza, farebbe partire il secondo solo dopo che il
/// primo e' scaduto: il tempo dell'esecuzione comincerebbe a contare quando
/// quello del saluto e' gia' finito, e un'esecuzione valida si vedrebbe
/// scadere addosso un tempo che non e' il suo.
///
/// # Perche' aspetta a piccoli passi invece che tutto insieme
///
/// Perche' un'attesa sola non si interrompe: se il lavoro finisce prima, il filo
/// resterebbe fermo fino alla scadenza, e con lui la sua bocchetta — che e'
/// esattamente cio' che impedisce al canale di disconnettersi.
pub(super) fn avvia_orologio(
    tempo_di_esecuzione: Duration,
    bocchetta: Bocchetta,
    passo: Duration,
) -> std::io::Result<(JoinHandle<Resoconto>, Freno)> {
    let (interruttore, freno) = crate::isolamento::sorgente::interruttore();
    let filo = nato("plenora-orologio", move || {
        let mut bocchetta = bocchetta;
        match attendi_o_fermati(&interruttore, tempo_di_esecuzione, passo) {
            Attesa::Compiuta => {
                let _ = bocchetta.manda(Fatto::TempoScaduto);
            }
            Attesa::Fermata => (),
            Attesa::NonRappresentabile => {
                // Non e' un arresto: e' un guasto nostro, e va accodato come
                // tale. Tacerlo lascerebbe un tempo che non e' mai scaduto e
                // nessuno che sappia perche'.
                let _ = bocchetta.manda(Fatto::OsservazioneImpossibile {
                    chi: "orologio",
                    motivo: format!(
                        "il tempo di {} ms non e' rappresentabile come scadenza",
                        tempo_di_esecuzione.as_millis()
                    ),
                });
            }
        }
        bocchetta.resoconto()
    })?;
    Ok((filo, freno))
}

/// Come e' finita un'attesa.
///
/// Tre esiti e non due: «l'ho aspettata tutta», «mi hanno fermato» e «non si
/// poteva nemmeno rappresentare» sono cose diverse, e la terza e' un guasto che
/// va detto invece di travestirsi da seconda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attesa {
    /// Il tempo e' passato tutto.
    Compiuta,
    /// Qualcuno ha chiesto di fermarsi.
    Fermata,
    /// La scadenza non e' rappresentabile.
    NonRappresentabile,
}

/// Aspetta `quanto`, a passi, guardando il freno.
///
/// La scadenza e' **assoluta**: si calcola una volta, e i passi non la spostano.
/// Sommare i passi invece di guardare l'orologio farebbe scivolare l'attesa di
/// tutto cio' che ogni passo dura in piu' di quanto ha chiesto.
fn attendi_o_fermati(interruttore: &Interruttore, quanto: Duration, passo: Duration) -> Attesa {
    let Some(scadenza) = std::time::Instant::now().checked_add(quanto) else {
        // Un'attesa non rappresentabile. Si rinuncia ad aspettarla invece di
        // andare in panico sommando, ma **non** si finge che qualcuno abbia
        // frenato: sono due cose diverse, e confonderle farebbe sparire un
        // guasto dentro una decisione.
        return Attesa::NonRappresentabile;
    };
    loop {
        if interruttore.fermato() {
            return Attesa::Fermata;
        }
        let adesso = std::time::Instant::now();
        if adesso >= scadenza {
            return Attesa::Compiuta;
        }
        std::thread::sleep(passo.min(scadenza.saturating_duration_since(adesso)));
    }
}

/// Guarda il dominio finche' non e' vuoto.
///
/// # Perche' la quiescenza si osserva invece di dedurla
///
/// Perche' «il figlio e' uscito» non dice niente sui suoi discendenti: il
/// dominio puo' essere ancora abitato da qualcuno che il figlio ha avviato, e
/// concludere sulla sua sola uscita direbbe che il lavoro e' finito mentre
/// qualcosa gira ancora.
pub(super) fn avvia_sorvegliante<O>(
    osservatore: O,
    bocchetta: Bocchetta,
    passo: Duration,
) -> std::result::Result<(JoinHandle<Resoconto>, Freno), (std::io::Error, Option<O>)>
where
    O: Osservatore + Send + 'static,
{
    // L'osservatore viaggia in una cella condivisa, e non catturato di peso.
    //
    // # Perche'
    //
    // Perche' deve poter **tornare indietro**. `Builder::spawn` prende la
    // chiusura e, quando il sistema rifiuta il thread, la lascia cadere con
    // tutto cio' che ha catturato: l'osservatore sparirebbe li'. Chi rinuncia a
    // una nascita parziale ne ha bisogno — forza il dominio, e poi deve sapere
    // se si e' svuotato. Senza, la rinuncia potrebbe dire «ho chiesto» e mai
    // «e' successo».
    //
    // La chiusura lo prende al primo giro; chi resta fuori lo ritrova nella
    // cella se la chiusura non e' mai partita.
    let cella = std::sync::Arc::new(std::sync::Mutex::new(Some(osservatore)));
    let sua = std::sync::Arc::clone(&cella);
    let (interruttore, freno) = crate::isolamento::sorgente::interruttore();
    let nascita = nato("plenora-sorvegliante", move || {
        let mut bocchetta = bocchetta;
        let preso = sua
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(mut osservatore) = preso else {
            // Non accade: la cella si svuota qui e in nessun altro posto. Se
            // accadesse, tacere lascerebbe la quiescenza senza nessuno che la
            // guardi e senza nessuno che lo dica.
            let _ = bocchetta.manda(Fatto::OsservazioneImpossibile {
                chi: "quiescenza",
                motivo: "l'osservatore non e' arrivato al suo filo".to_owned(),
            });
            return bocchetta.resoconto();
        };
        loop {
            if interruttore.fermato() {
                return bocchetta.resoconto();
            }
            match osservatore.quiescente() {
                Ok(true) => {
                    let _ = bocchetta.manda(Fatto::DominioQuiescente);
                    return bocchetta.resoconto();
                }
                Ok(false) => std::thread::sleep(passo),
                Err(Difetto::Interrotta) => {
                    // Un'interruzione non e' una mancanza di osservazione: la
                    // lettura non e' avvenuta, e riprovarla la fa avvenire. Non
                    // sporca l'evidenza, quindi non c'e' niente da riportare.
                }
                Err(Difetto::Impossibile(motivo)) => {
                    // Non aver potuto guardare e' un fatto **nostro**, e si
                    // smette.
                    //
                    // Non perche' un tentativo successivo fallirebbe: potrebbe
                    // benissimo riuscire. Si smette perche' l'osservazione
                    // mancata rende **gia' incompleta** l'evidenza sulla
                    // quiescenza, e una lettura riuscita dopo non cancella il
                    // buco che c'e' stato: direbbe «adesso e' vuoto», non «lo e'
                    // sempre stato». Insistere aggiungerebbe righe senza
                    // aggiungere certezza.
                    let _ = bocchetta.manda(Fatto::OsservazioneImpossibile {
                        chi: "quiescenza",
                        motivo,
                    });
                    return bocchetta.resoconto();
                }
            }
        }
    });
    match nascita {
        Ok(filo) => Ok((filo, freno)),
        // L'osservatore torna in un `Option` e non nudo: la cella e' piena per
        // costruzione quando la chiusura non e' partita, ma «per costruzione»
        // non e' una garanzia del tipo. Un `Option` dice a chi rinuncia che
        // guardare il dominio potrebbe non essere possibile — e chi rinuncia
        // sa gia' come dirlo.
        Err(errore) => Err((
            errore,
            cella
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        )),
    }
}

/// Perche' un'osservazione non e' avvenuta.
///
/// # Perche' due varianti e non un messaggio
///
/// Perche' rispondono a domande diverse. «La lettura e' stata interrotta»
/// significa che non e' avvenuta e che rifarla la fa avvenire: non manca niente
/// all'evidenza, manca solo un tentativo. «Non si e' potuto guardare» significa
/// che l'evidenza ha un buco, e quel buco resta anche se il tentativo dopo
/// riesce.
///
/// Un messaggio solo obbligherebbe chi legge a indovinare quale delle due, e
/// indovinerebbe leggendo il testo — cioe' male.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Difetto {
    /// La lettura e' stata interrotta: si puo' rifare.
    Interrotta,
    /// Non si e' potuto guardare, e l'evidenza ne resta incompleta.
    Impossibile(String),
}

/// Chi sa dire se il dominio e' vuoto.
///
/// Un trait e non la superficie del dominio: cosi' i casi compongono le tre
/// risposte che contano — «ancora abitato», «interrotta» e «non si legge» —
/// senza un cgroup vero, e il sorvegliante resta provabile ovunque.
pub(super) trait Osservatore {
    /// `Ok(true)` se nel dominio non e' rimasto nessuno.
    ///
    /// # Errors
    ///
    /// [`Difetto`], che distingue un tentativo da rifare da un'osservazione
    /// mancata.
    fn quiescente(&mut self) -> std::result::Result<bool, Difetto>;
}

/// Che cosa e' successo alla richiesta di annullamento.
///
/// # Perche' tre esiti e non un `Option`
///
/// Perche' chi annulla deve poter distinguere tre cose che portano a decisioni
/// diverse: la richiesta e' **in coda** e il supervisore la vedra'; la
/// conduzione ha gia' **deposto** la bocchetta, quindi non ascolta piu' e
/// annullare non serve; oppure la richiesta **non e' entrata**, e allora il
/// supervisore non la vedra' mai — che e' l'unico caso in cui chi ha annullato
/// deve fare qualcos'altro.
///
/// Un `Option<Esaurita>` le confonde: `None` direbbe insieme «fatto», «troppo
/// tardi» e «lucchetto avvelenato».
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EsitoDellAnnullamento {
    /// La richiesta e' in coda.
    Accodata,
    /// La conduzione ha gia' deposto la bocchetta: non ascolta piu'.
    GiaDeposta,
    /// La richiesta non e' entrata, e il motivo.
    NonAccodata(String),
}

/// Chi puo' chiedere l'annullamento.
///
/// # Perche' non e' un filo
///
/// Perche' non ha niente da guardare: non aspetta un evento, lo **porta**. Un
/// filo che dormisse in attesa di essere svegliato sarebbe un filo in piu' da
/// fermare e da aspettare, per fare cio' che una chiamata fa da sola.
#[derive(Debug)]
pub(super) struct Annullatore {
    bocchetta: std::sync::Mutex<Option<Bocchetta>>,
}

impl Annullatore {
    /// Prende in carico la bocchetta della cancellazione.
    pub(super) const fn nuovo(bocchetta: Bocchetta) -> Self {
        Self {
            bocchetta: std::sync::Mutex::new(Some(bocchetta)),
        }
    }

    /// Chiede l'annullamento, **una volta sola**.
    ///
    /// La seconda chiamata non accoda niente: il fatto e' gia' li', e ripeterlo
    /// spenderebbe un gettone per dire una cosa che il registro ha gia'.
    ///
    /// Rende cio' che la bocchetta non ha potuto dire, se qualcosa.
    pub(super) fn annulla(&self) -> EsitoDellAnnullamento {
        // La presa si rilascia **prima** di accodare: tenere un lucchetto
        // mentre si parla con la coda vorrebbe dire che chi annulla e chi
        // depone si aspettano a vicenda per una cosa che riguarda solo la
        // bocchetta, e la bocchetta a quel punto e' gia' nostra.
        let Some(mut bocchetta) = self.prendi() else {
            return EsitoDellAnnullamento::GiaDeposta;
        };
        let _ = bocchetta.manda(Fatto::CancellazioneRichiesta);
        bocchetta
            .resoconto()
            .map_or(EsitoDellAnnullamento::Accodata, |quale| {
                EsitoDellAnnullamento::NonAccodata(quale.to_string())
            })
    }

    /// Lascia cadere la bocchetta senza annullare.
    ///
    /// Serve alla chiusura: finche' l'annullatore tiene la sua bocchetta, il
    /// canale non si disconnette.
    pub(super) fn deponi(&self) -> Resoconto {
        self.prendi().and_then(Bocchetta::resoconto)
    }

    /// Prende la bocchetta, **recuperando da un lucchetto avvelenato**.
    ///
    /// # Perche' si recupera invece di rinunciare
    ///
    /// Perche' un lucchetto avvelenato dice che un altro thread e' morto con la
    /// presa in mano, non che il dato sotto sia rotto: qui il dato e' un
    /// `Option<Bocchetta>`, e le due sole cose che gli succedono sono «c'e'» e
    /// «e' stata presa». Nessuna delle due si corrompe a meta'.
    ///
    /// Rinunciare con un `ok()?` collassa tre cose diverse nello stesso `None`:
    /// lucchetto avvelenato, bocchetta gia' deposta, e successo. Su `annulla`
    /// fa **sparire la richiesta** senza dirlo; su `deponi` lascia viva la
    /// bocchetta fino al tetto del drenaggio, e chi legge quel difetto non ha
    /// modo di risalire alla ragione.
    fn prendi(&self) -> Option<Bocchetta> {
        self.bocchetta
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

#[cfg(test)]
mod tests;
