//! I produttori: guardano, e accodano cio' che vedono.
//!
//! Ognuno finisce **rendendo il proprio resoconto**, che consuma la bocchetta:
//! cosi' il canale si disconnette (finche' una bocchetta vive il drenaggio non
//! vede `Disconnected`) e un rifiuto ad accodare torna fuori dalla coda,
//! attraverso il `JoinHandle`.
//!
//! Nessun produttore giudica: dice che cosa ha visto, e il giudizio sta nel
//! consumatore, l'unico ad avere il quadro.

use std::io::Read;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::protocollo::handshake::HandshakeAccettato;
use crate::protocollo::lettore::leggi_frame;
use crate::protocollo::messaggi::{Corpo, Progresso};

use super::coda::{Bocchetta, Esaurita};
use super::{Fatto, Ruolo};
use crate::isolamento::sorgente::{Freno, Interruttore, SorgenteTerminabile};

/// Cio' che un produttore rende quando finisce.
///
/// `None` se ha detto tutto quello che aveva da dire.
pub(super) type Resoconto = Option<Esaurita>;

/// A che punto e' la conversazione, dal lato di chi ascolta.
///
/// «Fuori sequenza» dipende dal momento, non dal messaggio: un `Progresso` va
/// bene prima dell'esito e non dopo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PuntoDellaConversazione {
    /// Si accettano progressi, e un esito.
    InCorso,
    /// L'esito e' arrivato: non deve arrivare altro.
    Concluso,
}

/// Il progresso, **conservato** invece che inoltrato.
///
/// I contatori del `Progresso` sono **totali**, non incrementi: l'ultimo
/// rapporto li contiene tutti. Sommarli darebbe un numero senza significato e
/// aprirebbe al traboccamento (saturare e' una perdita silenziosa, il panico lo
/// sceglierebbe il worker). Tenere solo l'ultimo fa arrivare al consumatore
/// **un fatto solo**, qualunque sia il numero di rapporti scelto dal worker.
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
/// La sequenza che il lettore accetta vale solo dopo la `Risposta`: su un
/// canale grezzo la `Risposta` risulterebbe fuori sequenza. Il tipo lo impone
/// chiedendo [`HandshakeAccettato`], che **solo** l'handshake produce.
pub(super) struct CanaleOperativo<R: Read> {
    sorgente: R,
    /// La prova, tenuta perche' esista e non perche' si legga: il token che
    /// porta appartiene al publish.
    _accordo: HandshakeAccettato,
}

impl<R: Read> CanaleOperativo<R> {
    /// Il canale, dopo l'accordo, su una sorgente qualunque.
    ///
    /// Solo nei casi: una sorgente qualunque non si rende non bloccante, e in
    /// produzione darebbe un lettore che non si puo' fermare. I casi leggono da
    /// un vettore di byte, che non blocca.
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
    /// Sta qui e non nel lettore, che e' generico sulla sorgente: questo
    /// costruttore e' l'**unico** modo di ottenere un canale operativo di
    /// produzione, quindi la modalita' non bloccante non si puo' dimenticare.
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
/// Accetta zero o piu' `Progresso`, poi al piu' un `Esito`, poi la fine; tutto
/// il resto e' **fuori sequenza**. Alla prima rottura accoda **un** fatto di
/// protocollo e smette: inoltrare ogni messaggio inaspettato lascerebbe al
/// worker decidere quanto spazio occupare in coda.
///
/// Rende il filo, che rende il resoconto della bocchetta, e il freno.
pub(super) fn avvia_lettore<R: Read + Send + 'static>(
    canale: CanaleOperativo<R>,
    bocchetta: Bocchetta,
    ruolo: Ruolo,
) -> std::io::Result<(JoinHandle<Resoconto>, Freno)> {
    let (mut terminabile, freno) = SorgenteTerminabile::nuova(canale.sorgente);
    let filo = nato("plenora-lettore", move || {
        let mut bocchetta = bocchetta;
        let mut progresso = ProgressoOsservato::default();
        let mut punto = PuntoDellaConversazione::InCorso;
        let (fatto_finale, mut rifiuto) = fine_del_canale(
            &mut terminabile,
            &mut bocchetta,
            &mut progresso,
            &mut punto,
            ruolo,
        );

        // Il progresso conservato si accoda **prima** della fine: cosi' chi legge
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
/// `std::thread::spawn` va in panico se il sistema rifiuta il thread, mentre
/// alcuni produttori sono vivi e il figlio e' avviato; `Builder::spawn` rende
/// un `Result`, e il rifiuto diventa un errore da cui tornare indietro.
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
/// Il sistema rifiuta un thread solo quando e' esaurito: senza questa giuntura
/// non si provano i cablaggi della rinuncia in `conduci` (dopo il lettore,
/// l'orologio e il sorvegliante). Sta sotto `cfg(test)` e non dietro una
/// feature, che un consumatore della libreria potrebbe scegliere.
///
/// I casi girano in parallelo: il turno impedisce a due casi di armare
/// insieme, e le nascite si contano per filo perche' un caso non armato non
/// consumi il conteggio. `nato` e' sempre chiamato dal filo del caso.
#[cfg(test)]
pub(super) mod inciampo {
    /// Il turno: uno solo arma per volta.
    static TURNO: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Chi ha armato, quale nascita deve fallire, e quante ne sono state chieste.
    static ARMATO: std::sync::Mutex<Option<(std::thread::ThreadId, usize, usize)>> =
        std::sync::Mutex::new(None);

    /// L'arma, che si disinnesca da sola.
    ///
    /// Il guasto resta armato finche' l'arma vive; lasciarla cadere lo spegne.
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
        // `Builder::spawn`, e un lucchetto tenuto attraverso una nascita
        // espone a un'attesa reciproca fra casi.
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
///
/// Il ruolo decide anche qui, oltre che in `Registro::messaggio`, perche' qui
/// la conversazione diventa «conclusa»: un esito del ruolo sbagliato e' fuori
/// sequenza subito, invece di chiudere il dialogo e far leggere come fuori
/// sequenza cio' che arriva dopo.
fn fine_del_canale<R: Read>(
    sorgente: &mut SorgenteTerminabile<R>,
    bocchetta: &mut Bocchetta,
    progresso: &mut ProgressoOsservato,
    punto: &mut PuntoDellaConversazione,
    ruolo: Ruolo,
) -> (Fatto, Resoconto) {
    loop {
        match leggi_frame(sorgente) {
            Ok(None) => return (Fatto::FineDelCanale, None),
            Ok(Some(frame)) => match (*punto, ruolo, frame.in_corpo()) {
                (PuntoDellaConversazione::InCorso, _, Corpo::Progresso(quanto)) => {
                    if let Err(motivo) = progresso.osserva(quanto) {
                        // Una regressione e' una violazione del protocollo, e si
                        // tratta come le altre: **un** fatto, e si smette.
                        return (Fatto::CanaleInterrotto(motivo), None);
                    }
                }
                (PuntoDellaConversazione::InCorso, Ruolo::Worker, Corpo::Esito(esito)) => {
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
                (
                    PuntoDellaConversazione::InCorso,
                    Ruolo::Verificatore,
                    Corpo::EsitoVerifica(esito),
                ) => {
                    *punto = PuntoDellaConversazione::Concluso;
                    if let Err(rifiuto) = bocchetta.manda(Fatto::MessaggioDalWorker(Box::new(
                        Corpo::EsitoVerifica(esito),
                    ))) {
                        return (
                            Fatto::CanaleInterrotto(
                                "l'esito di verifica non si e' potuto accodare".to_owned(),
                            ),
                            Some(rifiuto),
                        );
                    }
                }
                (_, _, altro) => {
                    return (
                        Fatto::CanaleInterrotto(format!(
                        "messaggio fuori sequenza: «{}» dopo che la conversazione e' {} (ruolo {})",
                        nome_del_corpo(&altro),
                        se_conclusa(*punto),
                        ruolo.nome(),
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
        Corpo::IncaricoVerifica(_) => "incarico_verifica",
        Corpo::Annulla(_) => "annulla",
        Corpo::Risposta(_) => "risposta",
        Corpo::Progresso(_) => "progresso",
        Corpo::Esito(_) => "esito",
        Corpo::EsitoVerifica(_) => "esito_verifica",
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
/// Un tempo solo: il timeout dell'handshake si chiude **prima** che il canale
/// operativo esista, e lo misura chi guida l'handshake.
///
/// Aspetta a passi perche' un'attesa sola non si interrompe: il filo, e con
/// lui la bocchetta, resterebbe vivo fino alla scadenza e il canale non si
/// disconnetterebbe.
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
/// La quiescenza si osserva: l'uscita del figlio non dice niente dei suoi
/// discendenti, che possono abitare ancora il dominio.
pub(super) fn avvia_sorvegliante<O>(
    osservatore: O,
    bocchetta: Bocchetta,
    passo: Duration,
) -> std::result::Result<(JoinHandle<Resoconto>, Freno), (std::io::Error, Option<O>)>
where
    O: Osservatore + Send + 'static,
{
    // L'osservatore viaggia in una cella condivisa perche' deve poter tornare
    // indietro: se il sistema rifiuta il thread, `Builder::spawn` lascia cadere
    // la chiusura con cio' che ha catturato, e chi rinuncia ha bisogno
    // dell'osservatore per sapere se il dominio forzato si e' svuotato.
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
                    // smette: l'evidenza sulla quiescenza e' gia' incompleta, e
                    // una lettura riuscita dopo non cancella il buco.
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
/// Un'interruzione si rifa' senza lasciare buchi nell'evidenza; un'osservazione
/// impossibile lascia un buco che resta. Due varianti evitano di distinguerle
/// dal testo.
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
/// Tre esiti e non un `Option<Esaurita>`, il cui `None` confonderebbe «fatto»
/// e «troppo tardi»: solo `NonAccodata` chiede a chi annulla di fare
/// qualcos'altro, perche' il supervisore non vedra' mai la richiesta.
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
/// Non e' un filo: non aspetta un evento, lo **porta**, e basta una chiamata.
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
        // La presa si rilascia **prima** di accodare: la bocchetta e' gia'
        // nostra, e chi annulla e chi depone non devono aspettarsi.
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
    /// Il dato e' un `Option<Bocchetta>`, che non si corrompe a meta'.
    /// Rinunciare con `ok()?` farebbe sparire senza dirlo una richiesta di
    /// annullamento, o lascerebbe viva la bocchetta fino al tetto del drenaggio.
    fn prendi(&self) -> Option<Bocchetta> {
        self.bocchetta
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

#[cfg(test)]
mod tests;
