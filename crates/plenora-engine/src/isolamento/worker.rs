//! Il worker: che cosa fa appena nasce, prima di dire qualunque cosa.
//!
//! Trova due descrittori **ereditati** e due numeri nell'ambiente, e nessuna
//! delle due cose e' una prova: li **riguarda** uno per uno e rifiuta cio' che
//! non torna. Fra la `exec` e qui il processo ha cambiato identita', e un
//! controllo fatto dall'altra parte descrive un altro processo.
//!
//! I rifiuti sono nominati uno per uno, perche' ciascuno manda a guardare in
//! un posto diverso.

use plenora_core::error::PlenoraError;

#[cfg(target_os = "linux")]
use super::canale::{self, Verso};
#[cfg(target_os = "linux")]
use super::DalConfine;
use super::{non_disponibile, Result};
#[cfg(target_os = "linux")]
use crate::cancellation::CancellationToken;
#[cfg(target_os = "linux")]
use crate::protocollo::{
    assi::{errore_dichiarabile, forma_sul_filo},
    codifica::codifica,
    descrizione,
    handshake::{WorkerAccordato, WorkerInAttesa},
    lettore::leggi_frame,
    messaggi::{Corpo, EsitoWorkerSulFilo, Frame},
};
use crate::protocollo::{limiti::MAX_PROGRESSO, messaggi::Progresso};

#[cfg(target_os = "linux")]
mod ascolto;
#[cfg(target_os = "linux")]
mod esecuzione;

/// Il separatore fra i due numeri nella variabile del canale.
///
/// Una variabile sola, non due: due variabili hanno stati a meta' senza una
/// lettura ovvia, mentre una sola c'e' nella forma attesa oppure no. Il
/// canale attraversa il confine come una cosa sola.
pub(super) const SEPARATORE: char = ':';

/// I due numeri del canale, letti dall'ambiente e **non ancora creduti**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NumeriLetti {
    pub(super) legge: i32,
    pub(super) scrive: i32,
}

/// Quale delle due meta' della variabile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meta {
    Lettura,
    Scrittura,
}

impl Meta {
    const fn detta(self) -> &'static str {
        match self {
            Self::Lettura => "lettura",
            Self::Scrittura => "scrittura",
        }
    }
}

/// Perche' il valore non e' stato accettato.
///
/// Un tipo e non un messaggio: i messaggi interpolano valori e provano la
/// formulazione. Con un tipo a ogni forma storta corrisponde **una** ragione
/// nominata, e i casi la confrontano per identita'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rifiuto {
    /// La variabile non c'e'.
    Assente,
    /// C'e', ma non e' testo.
    NonTesto,
    /// Non ha il separatore.
    SenzaSeparatore,
    /// Ne ha piu' di uno.
    TroppiSeparatori,
    /// Una meta' e' vuota.
    MetaVuota(Meta),
    /// Una meta' non e' un numero.
    NonNumerica(Meta),
    /// E' un numero, ma **non entra** in un descrittore.
    ///
    /// Distinto da [`Self::NonNumerica`] perche' dice una cosa diversa: la
    /// forma e' quella giusta, la grandezza no. Confonderli manderebbe a
    /// cercare un refuso dove c'e' un valore fuori scala.
    TroppoGrande(Meta),
    /// E' un numero, ma non nella forma che il supervisore scrive.
    NonCanonica(Meta),
    /// E' negativo: non e' un descrittore affatto.
    ///
    /// Distinto da [`Self::FlussoStandard`] perche' zero, uno e due **sono**
    /// descrittori — solo non i nostri — mentre un negativo non nomina niente.
    /// Dirli allo stesso modo direbbe il falso su meta' dei casi.
    NonUnDescrittore(Meta),
    /// E' uno dei tre flussi standard.
    FlussoStandard(Meta),
}

impl Rifiuto {
    /// Il messaggio, con il nome della variabile che il chiamante conosce.
    fn detto(self, quale: &str) -> PlenoraError {
        let meta = |m: Meta| format!("la meta' «{}» della variabile «{quale}»", m.detta());
        let motivo = match self {
            Self::Assente => format!(
                "la variabile «{quale}» non c'e': il worker non sa dove sono i suoi estremi"
            ),
            Self::NonTesto => format!("la variabile «{quale}» non e' testo valido"),
            Self::SenzaSeparatore => format!(
                "la variabile «{quale}» non ha la forma «lettura{SEPARATORE}scrittura»: manca il \
                 separatore"
            ),
            Self::TroppiSeparatori => format!(
                "la variabile «{quale}» ha piu' di un «{SEPARATORE}»: quale coppia sia non lo \
                 dichiara nessuno"
            ),
            Self::MetaVuota(m) => format!("{} e' vuota", meta(m)),
            Self::NonNumerica(m) => format!("{} non e' un numero", meta(m)),
            Self::TroppoGrande(m) => {
                format!("{} e' un numero troppo grande per un descrittore", meta(m))
            }
            Self::NonCanonica(m) => format!("{} non e' in forma canonica", meta(m)),
            Self::NonUnDescrittore(m) => format!(
                "{} e' negativa: un descrittore non e' mai negativo",
                meta(m)
            ),
            Self::FlussoStandard(m) => format!(
                "{} nomina uno dei flussi standard: quelli non sono il canale",
                meta(m)
            ),
        };
        non_disponibile("canale", &motivo)
    }
}

/// Legge i due numeri dalla variabile del canale.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], con la ragione nominata.
#[cfg(target_os = "linux")]
pub(super) fn numeri_dall_ambiente() -> Result<NumeriLetti> {
    let quale = canale::VARIABILE_DEL_CANALE;
    numeri_da(std::env::var_os(quale).as_deref()).map_err(|rifiuto| rifiuto.detto(quale))
}

/// Il giudizio sul valore, separato dalla lettura dell'ambiente.
///
/// L'ambiente e' **globale al processo**, e casi che lo scrivessero si
/// disturberebbero in parallelo; isolato, il giudizio e' una funzione pura e
/// le forme storte si scrivono. Non normalizza niente: spazi, `+`, zeri
/// davanti sarebbero forme che il supervisore non scrive.
///
/// # Errors
///
/// Il [`Rifiuto`] che nomina **quale** forma non torna.
fn numeri_da(grezzo: Option<&std::ffi::OsStr>) -> std::result::Result<NumeriLetti, Rifiuto> {
    let grezzo = grezzo.ok_or(Rifiuto::Assente)?;
    // Il valore non e' tenuto a essere UTF-8, e non lo si forza: una variabile
    // che non e' testo non e' una variabile che noi abbiamo scritto. Il
    // contenuto non entra da nessuna parte — e' un ingresso arbitrario, e
    // ripeterlo porterebbe nei log byte che nessuno ha scelto.
    let testo = grezzo.to_str().ok_or(Rifiuto::NonTesto)?;

    let mut meta = testo.split(SEPARATORE);
    let (Some(sinistra), Some(destra)) = (meta.next(), meta.next()) else {
        return Err(Rifiuto::SenzaSeparatore);
    };
    if meta.next().is_some() {
        return Err(Rifiuto::TroppiSeparatori);
    }

    Ok(NumeriLetti {
        legge: numero(sinistra, Meta::Lettura)?,
        scrive: numero(destra, Meta::Scrittura)?,
    })
}

/// Una meta' della variabile, letta come numero di descrittore.
///
/// Si confronta con la forma canonica perche' `parse` accetta `+3`, ` 3`,
/// `03`, che il supervisore non scrive mai.
fn numero(testo: &str, quale: Meta) -> std::result::Result<i32, Rifiuto> {
    if testo.is_empty() {
        return Err(Rifiuto::MetaVuota(quale));
    }
    forma_canonica(testo, quale)?;
    let valore = match testo.parse::<i32>() {
        Ok(valore) => valore,
        // Qui restano **solo** i traboccamenti: la forma e' gia' stata
        // accettata, quindi cio' che `parse` puo' ancora rifiutare e' la
        // grandezza. Il ramo generico c'e' lo stesso, perche' dedurre
        // l'esaustivita' di una libreria non e' verificarla.
        Err(errore) => {
            return Err(match errore.kind() {
                std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
                    Rifiuto::TroppoGrande(quale)
                }
                _ => Rifiuto::NonNumerica(quale),
            })
        }
    };
    // Un negativo non nomina nessun descrittore; zero, uno e due ne nominano
    // tre veri, che pero' sono i flussi standard di questo processo. Un canale
    // che ci finisse sopra parlerebbe con il terminale di chi ha avviato il
    // programma invece che con il supervisore.
    if valore < 0 {
        return Err(Rifiuto::NonUnDescrittore(quale));
    }
    if valore < 3 {
        return Err(Rifiuto::FlussoStandard(quale));
    }
    Ok(valore)
}

/// Che il testo sia scritto **nella forma in cui il supervisore lo scrive**.
///
/// Si giudica prima della grandezza, e sulla sintassi: `+99999999999` e
/// `09999999999` sono scritti male **e** troppo grandi, e vanno chiamati
/// «scritti male». Un confronto con `valore.to_string()` richiederebbe il
/// valore, quindi arriverebbe dopo il traboccamento.
///
/// # Errors
///
/// [`Rifiuto::NonNumerica`] se non e' un intero decimale con segno facoltativo;
/// [`Rifiuto::NonCanonica`] se lo e' ma non nella forma attesa — un `+`, uno
/// zero davanti, o un meno davanti allo zero.
fn forma_canonica(testo: &str, quale: Meta) -> std::result::Result<(), Rifiuto> {
    let (segno, corpo) = testo
        .strip_prefix(['+', '-'])
        .map_or((None, testo), |corpo| {
            (testo.as_bytes().first().copied(), corpo)
        });
    // Un corpo vuoto — il solo segno — non e' un numero scritto male: non e' un
    // numero. E lo stesso vale per qualunque carattere che non sia una cifra
    // decimale, spazi compresi.
    if corpo.is_empty() || !corpo.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Rifiuto::NonNumerica(quale));
    }
    if segno == Some(b'+') {
        return Err(Rifiuto::NonCanonica(quale));
    }
    // Uno zero davanti a un'altra cifra e' una forma che `parse` accetta e che
    // il supervisore non scrive mai.
    if corpo.len() > 1 && corpo.starts_with('0') {
        return Err(Rifiuto::NonCanonica(quale));
    }
    // «meno zero» denota zero, e zero si scrive senza segno.
    if segno == Some(b'-') && corpo == "0" {
        return Err(Rifiuto::NonCanonica(quale));
    }
    Ok(())
}

/// I due estremi del worker, riaperti e verificati.
///
/// Sono **aperture nuove** sulle stesse pipe, possedute e chiuse dal loro
/// `Drop`; gli ereditati restano dove sono (vedi
/// [`canale::riapri_accertato`]). Il consumatore e' l'accordo, che li rende un
/// canale.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(super) struct Estremi {
    /// Da dove arriva cio' che il supervisore dice.
    pub(super) legge: std::fs::File,
    /// Dove va cio' che il worker risponde.
    pub(super) scrive: std::fs::File,
}

/// Riapre i due estremi e **accerta** che siano quelli.
///
/// 1. **monothread**, prima di tutto: e' una precondizione del processo,
///    imposta dal supervisore prima della `exec`;
/// 2. **i numeri**, che sono testo e vanno letti prima di poterli usare;
/// 3. **la riapertura**, uno per uno, ciascuno col proprio verso atteso;
/// 4. **la coppia**, che e' una domanda sui due insieme e non su ciascuno: due
///    estremi che guardano la stessa pipe passano ogni controllo individuale e
///    non sono un canale.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], con la ragione precisa: nessuno dei
/// quattro passi rende un rifiuto generico.
#[cfg(target_os = "linux")]
pub(super) fn accerta_gli_estremi() -> Result<Estremi> {
    canale::accerta_monothread()?;
    let numeri = numeri_dall_ambiente()?;
    if numeri.legge == numeri.scrive {
        return Err(non_disponibile(
            "canale",
            &format!(
                "i due estremi portano lo stesso descrittore ({}): non sono due estremi",
                numeri.legge
            ),
        ));
    }
    let legge = canale::riapri_accertato(numeri.legge, Verso::Lettura)?;
    let scrive = canale::riapri_accertato(numeri.scrive, Verso::Scrittura)?;
    accerta_pipe_diverse(&legge, &scrive)?;
    Ok(Estremi { legge, scrive })
}

/// Che i due estremi non guardino la **stessa** pipe.
///
/// Numeri diversi possono nominare la stessa pipe (un descrittore duplicato),
/// e allora il worker parlerebbe con se stesso. Si confronta l'impronta
/// `(dispositivo, inode)`, che i due estremi di **una** pipe condividono.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se le due impronte coincidono, o se
/// una delle due non si legge.
#[cfg(target_os = "linux")]
fn accerta_pipe_diverse(legge: &std::fs::File, scrive: &std::fs::File) -> Result<()> {
    if impronta(legge, "lettura")? == impronta(scrive, "scrittura")? {
        return Err(non_disponibile(
            "canale",
            "i due estremi guardano la stessa pipe: un canale che parla con se stesso non e' un \
             canale",
        ));
    }
    Ok(())
}

/// L'impronta `(dispositivo, inode)` di un estremo.
///
/// E' cio' che identifica la **pipe**, non il descrittore: due aperture della
/// stessa pipe la condividono, ed e' proprio quella coincidenza che il
/// chiamante vuole escludere.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il descrittore non si interroga.
#[cfg(target_os = "linux")]
fn impronta(estremo: &std::fs::File, quale: &str) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let dati = estremo.metadata().map_err(|errore| {
        non_disponibile(
            "canale",
            &format!("l'estremo di {quale} non si lascia interrogare: {errore}"),
        )
    })?;
    Ok((dati.dev(), dati.ino()))
}

/// La quota di emissione del progresso, applicata dove il `Progresso` si conia.
///
/// Finita la quota il worker **smette di emetterlo e continua a lavorare**, e
/// l'`Esito` parte comunque: il progresso e' facoltativo. Sta qui perche' e'
/// una regola del **protocollo**: il supervisore conta i messaggi ricevuti con
/// la stessa nozione, indipendente dal mezzo.
#[derive(Debug)]
struct QuotaDiProgresso {
    emessi: usize,
    quota: usize,
}

impl QuotaDiProgresso {
    /// La quota del protocollo.
    const fn nuova() -> Self {
        Self {
            emessi: 0,
            quota: MAX_PROGRESSO,
        }
    }

    /// Una quota qualunque, per i casi che devono superarla senza produrre
    /// [`MAX_PROGRESSO`] batch veri.
    #[cfg(test)]
    const fn con_quota(quota: usize) -> Self {
        Self { emessi: 0, quota }
    }

    /// Emette, se la quota lo consente.
    ///
    /// # Errors
    ///
    /// Solo quello di chi invia. **Esaurita la quota non c'e' errore**: non
    /// emettere e' il comportamento previsto, e trasformarlo in un errore
    /// fermerebbe il lavoro.
    fn emetti(
        &mut self,
        quanto: Progresso,
        invia: &mut dyn FnMut(Progresso) -> Result<()>,
    ) -> Result<()> {
        if self.emessi >= self.quota {
            return Ok(());
        }
        self.emessi += 1;
        invia(quanto)
    }
}

/// Le capability che questo worker offre, **derivate** da chi le conosce.
///
/// Vengono da [`crate::planner::compiled_capabilities`], la stessa autorita'
/// che l'executor confronta col grafo validato: un elenco a mano divergerebbe.
/// Non ci sono `arrow_ipc` ne' `wkb` (formato obbligatorio e codifica del
/// contratto, non negoziabili), ne' i profili di publish: il worker non
/// pubblica, ed e' la differenza con `local_capabilities`.
///
/// `proj` non compare perche' con `proj-backend` la descrizione locale
/// rifiuta l'ambiente prima dell'handshake. `geos` compare quando e'
/// compilato, ed e' attraversabile: il worker usa gli stessi kernel del
/// percorso in-process.
///
/// Il confronto e' **asimmetrico**: il supervisore chiede un sottoinsieme, e
/// offrirne di piu' non e' un disaccordo. I nomi arrivano gia' in ordine
/// lessicografico, che e' la forma che l'handshake pretende.
#[cfg(target_os = "linux")]
fn capability_offerte() -> Vec<String> {
    crate::planner::compiled_capabilities()
        .names()
        .map(str::to_owned)
        .collect()
}

/// Conclude l'accordo con il supervisore.
///
/// 1. **ci si descrive**, prima di leggere: una misura fatta dopo il `Saluto`
///    potrebbe farsi influenzare da cio' che si e' letto;
/// 2. si legge **un** frame. Il lettore guarda il prefisso, decide, e solo
///    allora alloca;
/// 3. l'accordo lo giudica [`WorkerInAttesa::ricevi`], che confronta le due
///    descrizioni e rifiuta al primo disaccordo;
/// 4. si risponde con la **nostra** descrizione, non un'eco, perche' il
///    supervisore faccia lo stesso confronto.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il canale non regge; quello che
/// rende l'handshake se le due descrizioni non concordano.
#[cfg(target_os = "linux")]
fn accordati(estremi: &mut Estremi) -> Result<WorkerAccordato> {
    let locale = descrizione::di_questa_build(capability_offerte())?;
    let attesa = WorkerInAttesa::nuovo(locale)?;

    let Some(frame) = leggi_frame(&mut estremi.legge)? else {
        return Err(non_disponibile(
            "accordo",
            "il canale e' finito prima del saluto: il supervisore non ha detto niente",
        ));
    };
    let (risposta, accordato) = attesa.ricevi(frame)?;

    manda(&mut estremi.scrive, Corpo::Risposta(Box::new(risposta)))?;
    Ok(accordato)
}

/// Scrive tutti i byte, e si assicura che partano.
///
/// Il `flush` conta: il supervisore aspetta, e byte fermi in un buffer gli
/// darebbero un timeout al posto della diagnosi vera.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se la scrittura non riesce.
#[cfg(target_os = "linux")]
fn scrivi_tutto(dove: &mut std::fs::File, byte: &[u8]) -> Result<()> {
    use std::io::Write as _;
    dove.write_all(byte)
        .map_err(|causa| non_disponibile("accordo", &format!("la risposta non parte: {causa}")))?;
    dove.flush()
        .map_err(|causa| non_disponibile("accordo", &format!("la risposta non arriva: {causa}")))
}

/// Il worker, dal confine: la sequenza intera.
///
/// **Prima che il canale esista** non c'e' nessuno a cui dire niente, e
/// l'unica uscita e' un rifiuto che il supervisore vede come stato terminale.
/// **Dopo l'accordo** ogni fallimento diventa un `Esito` dichiarato, anche
/// quando il lavoro fallisce: da fuori un guasto non si distingue da un
/// ritardo.
///
/// La riuscita di questa funzione **non e'** la riuscita dell'esecuzione
/// isolata: verifica e pubblicazione appartengono a chi ha osservato.
#[cfg(target_os = "linux")]
pub(super) fn dal_confine() -> DalConfine {
    let mut estremi = match accerta_gli_estremi() {
        Ok(estremi) => estremi,
        Err(errore) => return DalConfine::Fallita(errore),
    };
    let accordato = match accordati(&mut estremi) {
        Ok(accordato) => accordato,
        Err(errore) => return DalConfine::Fallita(errore),
    };
    match lavora(estremi, accordato) {
        Ok(()) => DalConfine::Conclusa,
        Err(errore) => DalConfine::Fallita(errore),
    }
}

/// Riceve l'incarico, lo esegue, e dichiara com'e' andata.
///
/// L'errore del lavoro esce **sul filo**, come esito dichiarato. Di qui esce
/// solo il fallimento del canale: se l'esito non parte, il supervisore non ha
/// niente da leggere.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se l'incarico non arriva o l'esito
/// non parte; gli errori del protocollo se il frame non e' un incarico.
#[cfg(target_os = "linux")]
fn lavora(estremi: Estremi, accordato: WorkerAccordato) -> Result<()> {
    let Estremi { legge, mut scrive } = estremi;
    let dichiarazione = ricevi_ed_esegui(legge, accordato, &mut scrive);
    let inviato = manda(&mut scrive, Corpo::Esito(Box::new(dichiarazione.esito)));

    quel_che_resta(inviato, dichiarazione.anche)
}

/// Che cosa risale, dopo aver provato a dichiarare.
///
/// Il filo porta **un** esito e lo stato terminale un altro; il supervisore li
/// guarda entrambi (passi 2 e 1 della sequenza), e nessuno va perso per far
/// posto all'altro. E' una funzione a se' perche' un `?` di troppo scarterebbe
/// il secondo fatto in silenzio; isolata, un caso la attraversa in tutti i
/// modi. Registro: errori-e-limiti.md#il-filo-porta-un-esito-solo.
///
/// # Errors
///
/// Cio' che il filo non ha potuto dire: il secondo fatto, il fallimento
/// dell'invio, o entrambi.
#[cfg(target_os = "linux")]
fn quel_che_resta(inviato: Result<()>, anche: Option<PlenoraError>) -> Result<()> {
    match (inviato, anche) {
        (Ok(()), None) => Ok(()),
        // L'esito e' partito, e resta un secondo fatto che il filo non porta.
        // Risale: il processo esce non-zero, e di quel fatto sopravvive
        // **l'esistenza, non l'identita'** — il supervisore osserva il codice
        // d'uscita, non questo valore ne' il suo motivo. E' poco, ed e' piu' di
        // niente: un `Panic` dichiarato con uscita zero direbbe che per il
        // resto e' andato tutto bene. Il limite sta in
        // errori-e-limiti.md#il-filo-porta-un-esito-solo.
        (Ok(()), Some(secondo)) => Err(secondo),
        (Err(invio), None) => Err(invio),
        // Nemmeno l'esito e' partito: sopravvivono entrambi i difetti, prima
        // il canale e poi cio' che si sarebbe dovuto dire. Non passa da
        // `con_contesto`, che lascerebbe intatte le varianti dell'invio.
        (Err(invio), Some(secondo)) => Err(non_disponibile(
            "esito",
            &format!(
                "l'esito non e' partito ({invio}), e non si e' potuto dichiarare nemmeno \
                 questo: {secondo}"
            ),
        )),
    }
}

/// Cio' che il worker ha da dichiarare, e cio' che il filo non puo' portare.
///
/// L'`Esito` ha **un** posto, e certi cammini producono due fatti (un panico
/// e una violazione del canale di controllo). `Panic` porta solo il payload,
/// quindi il secondo fatto esce di qui e fa uscire il processo **non-zero**:
/// ne sopravvive l'esistenza, non l'identita'. Limite del protocollo,
/// registrato in errori-e-limiti.md#il-filo-porta-un-esito-solo.
#[cfg(target_os = "linux")]
struct Dichiarazione {
    /// Cio' che parte sul filo.
    esito: EsitoWorkerSulFilo,
    /// Il secondo fatto, se ce n'e' uno.
    anche: Option<PlenoraError>,
}

#[cfg(target_os = "linux")]
impl Dichiarazione {
    /// Un fatto solo, che sta tutto nell'esito.
    const fn sola(esito: EsitoWorkerSulFilo) -> Self {
        Self { esito, anche: None }
    }

    /// Un errore da dichiarare, e nient'altro da far risalire.
    fn errore(causa: &PlenoraError) -> Self {
        Self::sola(EsitoWorkerSulFilo::Errore {
            errore: Box::new(errore_dichiarabile(causa)),
        })
    }
}

/// Aggiunge un secondo fatto al messaggio di un errore del filo.
///
/// Gli assi di `ErroreSulFilo` (categoria, fase, effetto, ritentativo)
/// descrivono il primo errore; il messaggio e' l'unico campo che porta
/// entrambi senza mentire.
#[cfg(target_os = "linux")]
fn con_anche(
    mut errore: crate::protocollo::messaggi::ErroreSulFilo,
    anche: Option<&PlenoraError>,
) -> crate::protocollo::messaggi::ErroreSulFilo {
    if let Some(secondo) = anche {
        errore.messaggio = format!("{}; mentre si ascoltava: {secondo}", errore.messaggio);
    }
    errore
}

/// Riceve l'incarico, lo esegue, e rende cio' che c'e' da dichiarare.
///
/// Non rende un `Result`: dopo l'accordo ogni fallimento si dichiara, e un
/// `?` farebbe uscire il worker senza che il supervisore riceva niente.
#[cfg(target_os = "linux")]
fn ricevi_ed_esegui(
    mut legge: std::fs::File,
    accordato: WorkerAccordato,
    scrive: &mut std::fs::File,
) -> Dichiarazione {
    let ricevuto = match leggi_frame(&mut legge) {
        Err(causa) => return Dichiarazione::errore(&causa),
        Ok(None) => {
            return Dichiarazione::errore(&non_disponibile(
                "incarico",
                "il canale e' finito prima dell'incarico: l'accordo c'e', il lavoro no",
            ))
        }
        Ok(Some(frame)) => accordato.ricevi_incarico(frame),
    };
    let (incarico, token) = match ricevuto {
        Ok(coppia) => coppia,
        Err(causa) => return Dichiarazione::errore(&causa),
    };

    // Da qui in poi l'estremo di lettura appartiene all'ascolto: dopo
    // l'`Incarico` l'unica cosa che puo' ancora arrivare e' un `Annulla`, e chi
    // esegue non e' in ascolto. Il token nasce **prima** dell'esecuzione perche'
    // e' lo stesso che entra nel `RuntimeContext`: due token sarebbero due leve,
    // e l'annullamento tirerebbe quella che l'executor non guarda.
    let annullamento = CancellationToken::new();
    let ascolto = match ascolto::Ascolto::comincia(legge, annullamento.clone()) {
        Ok(ascolto) => ascolto,
        Err(causa) => return Dichiarazione::errore(&causa),
    };

    let mut quota = QuotaDiProgresso::nuova();
    // Il panico si prende qui e non nel `main`: il dispatch delle modalita'
    // avviene **prima** del suo `catch_unwind`, quindi un panico del worker
    // uscirebbe da questo processo senza che il supervisore riceva niente — e
    // dovrebbe dedurne la causa da uno stato terminale.
    let lavoro = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        esecuzione::esegui(&incarico, &token, &annullamento, &mut |quanto| {
            quota.emetti(quanto, &mut |uno| manda(scrive, Corpo::Progresso(uno)))
        })
    }));
    // Su **ogni** cammino, compreso quello del panico: un lettore non fermato
    // resta dentro la propria attesa, e il processo non esce.
    let guasto = guasto_dell_ascolto(ascolto.ferma_e_raccogli());

    match lavoro {
        // Il lavoro e' riuscito, ma se il canale di controllo si e' comportato
        // male non lo si tace: un messaggio fuori sequenza o un canale che cede
        // dicono che lo scambio non e' quello concordato — e l'artefatto resta
        // sul temporaneo, dove nessuno lo pubblica.
        Ok(Ok((digest_artefatto, conteggi))) => guasto.map_or_else(
            || {
                Dichiarazione::sola(EsitoWorkerSulFilo::Successo {
                    digest_artefatto,
                    conteggi,
                })
            },
            |guasto| Dichiarazione::errore(&guasto),
        ),
        // Due errori, un posto: quello del lavoro tiene i propri assi, l'altro
        // entra nel **messaggio**. Non passa da `con_contesto`, che lascia
        // intatte `Io`, `Protocol`, `IsolationUnavailable`, `Internal`.
        Ok(Err(causa)) => Dichiarazione::sola(EsitoWorkerSulFilo::Errore {
            errore: Box::new(con_anche(errore_dichiarabile(&causa), guasto.as_ref())),
        }),
        // Un panico e un guasto del canale sono due fatti, e `Panic` porta la
        // sola forma del payload: non c'e' dove metterci l'altro. Il panico va
        // sul filo — e' cio' che il supervisore non potrebbe ricostruire — e il
        // guasto risale allo stato terminale.
        Err(payload) => Dichiarazione {
            // La **forma**, mai il contenuto. Il payload si lascia cadere qui
            // senza che nessuno lo legga.
            esito: EsitoWorkerSulFilo::Panic {
                forma: forma_sul_filo(payload.as_ref()),
            },
            anche: guasto,
        },
    }
}

/// Il guasto che l'ascolto ha visto, se ne ha visto uno.
///
/// Sono guasti la violazione della sequenza, il cedimento del canale e il
/// panico del lettore. Non lo sono l'annullamento (chiesto dal supervisore),
/// la fine del canale (legittima) e l'arresto (una nostra decisione).
#[cfg(target_os = "linux")]
fn guasto_dell_ascolto(ascoltato: ascolto::Ascoltato) -> Option<PlenoraError> {
    use ascolto::Ascoltato;

    match ascoltato {
        Ascoltato::Annullamento | Ascoltato::FineDelCanale | Ascoltato::Fermato => None,
        // I due enum del filo non espongono il proprio nome alla produzione, e
        // non gliene si aggiunge uno: `TUTTE` esiste per i casi, e un accessore
        // generato per tutti gli enum sarebbe codice morto su quelli che nessuno
        // nomina. Qui il nome serve a un lettore umano, e la forma `Debug` di un
        // enum chiuso lo e' — senza portare con se' nessun valore.
        Ascoltato::FuoriSequenza(tipo) => Some(PlenoraError::Protocol(format!(
            "dopo l'incarico e' arrivato un messaggio di tipo «{tipo:?}», e dopo l'incarico il \
             protocollo ammette solo un annullamento"
        ))),
        Ascoltato::Guasto(causa) => Some(causa),
        Ascoltato::Panico(forma) => Some(non_disponibile(
            "annullamento",
            &format!(
                "il lettore dell'annullamento e' andato in panico (payload {forma:?}); nessun \
                 contenuto del payload viene pubblicato"
            ),
        )),
    }
}

/// Manda un corpo, e si assicura che parta.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se la codifica o la scrittura non
/// riescono.
#[cfg(target_os = "linux")]
fn manda(dove: &mut std::fs::File, corpo: Corpo) -> Result<()> {
    let byte = codifica(&Frame::nuovo(corpo))?;
    scrivi_tutto(dove, &byte)
}

#[cfg(test)]
mod tests;
