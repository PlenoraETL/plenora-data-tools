//! La conduzione: un solo consumatore, e una chiusura che non perde niente.
//!
//! # La sequenza causale, e perche' e' quella
//!
//! 1. **Si ascolta.** I fatti arrivano dai produttori e accendono il registro,
//!    finche' i tre fatti terminali non ci sono tutti — oppure finche' non si
//!    decide di chiudere prima.
//! 2. **Si chiudono gli ingressi.** L'estremo su cui il supervisore scrive cade:
//!    da quel momento il worker vede la fine del proprio ingresso, e sa che non
//!    arrivera' altro. Prima di questo passo il canale e' ancora aperto, e
//!    chiuderlo prima vorrebbe dire rinunciare a poter annullare.
//! 3. **Si fermano i produttori, e li si aspetta.** Fermarli senza aspettarli
//!    lascerebbe vive le loro bocchette; aspettarli senza fermarli aspetterebbe
//!    per sempre. Il loro resoconto torna dal `JoinHandle`, che e' una via che
//!    non passa dalla coda.
//! 4. **Si raccoglie il figlio, e si legge l'evidenza.** In quest'ordine, e
//!    dopo la quiescenza: l'evidenza letta prima misurerebbe un dominio ancora
//!    abitato.
//! 5. **Si drena** fino a `Disconnected`.
//! 6. **Si conclude**, una volta sola.
//!
//! # Che cosa garantisce che niente resti fuori
//!
//! Due cose, e nessuna delle due e' una promessa a parole.
//!
//! **Niente resta fuori dal drenaggio.** Un fatto accodato e' nel canale; il
//! drenaggio legge finche' il canale non dice `Disconnected`, e il canale lo
//! dice solo quando **ogni** bocchetta e' caduta — cioe' dopo che ogni
//! produttore e' stato aspettato. Non c'e' istante in cui il drenaggio smetta
//! mentre qualcuno puo' ancora scrivere.
//!
//! **Niente compare dopo lo snapshot.** Perche' dopo non c'e' piu' nessuno a
//! cui comparire: `chiudi_e_drena` **consuma la coda**, e con lei il
//! ricevitore. Non e' che si smetta di guardare: e' che non esiste piu' un
//! posto da cui prendere. La garanzia sta nel tipo, non nella disciplina di chi
//! scrive il seguito.

use std::thread::JoinHandle;
use std::time::Duration;

use plenora_core::error::EvidenzaDiLimite;

use crate::protocollo::codifica::codifica;
use crate::protocollo::messaggi::{Annulla, Corpo, Frame};

use crate::isolamento::figlio::{
    Chiusura, FiglioVivo, OrologioDiSistema, ProcessoFiglio, Uscita, LIMITE_DI_RACCOLTA,
    PASSO_DI_RACCOLTA,
};

use super::coda::{apri, Bocchetta, Presa};
use super::produttori::{
    avvia_lettore, avvia_orologio, avvia_sorvegliante, Annullatore, CanaleOperativo, Difetto,
    Osservatore, Resoconto,
};
use crate::isolamento::sorgente::Freno;
use super::{EsitoDelSupervisore, Fatto, Impedimento, Registro, UscitaOsservata};

/// Ogni quanto il consumatore torna a guardare se e' ora di chiudere.
///
/// Governa **l'attesa che si chiede**, non quella che si ottiene: come per il
/// lettore, `recv_timeout` promette di non tornare prima, non di tornare entro.
const PASSO_DEL_GIRO: Duration = Duration::from_millis(10);

/// Quanto si concede al dominio, dopo aver deciso di chiudere, prima di
/// smettere di aspettare i fatti terminali.
///
/// # Perche' un margine e non zero
///
/// Perche' fra la decisione di chiudere e la quiescenza c'e' del lavoro vero:
/// il segnale arriva, i processi muoiono, il cgroup si svuota. Chiudere a zero
/// riporterebbe «non quiescente» su domini che lo diventano un istante dopo, e
/// quella riga manderebbe a cercare un residuo che non c'e'.
///
/// # Perche' un margine e non un'attesa
///
/// Perche' se il dominio non si svuota, non si svuotera' guardandolo piu' a
/// lungo: c'e' qualcosa che non muore, ed e' un fatto da riportare. Il margine
/// separa «ci ha messo un momento» da «non e' successo».
const MARGINE_DI_CORTESIA: Duration = Duration::from_secs(2);

/// Quanto si aspetta che il dominio si svuoti **dopo** la forzatura.
///
/// # Perche' non e' il margine di cortesia
///
/// Perche' misura una cosa diversa. Il margine e' cortesia verso il worker —
/// «chiudi con calma» — e si concede **prima** di forzare, a un processo che
/// puo' ancora scegliere. Questa e' il tempo che il kernel impiega a svuotare
/// davvero il cgroup **dopo** `cgroup.kill`, e non e' concessa a nessuno: e'
/// solo il ritardo fra il segnale e il suo effetto.
///
/// # Perche' senza di lei l'esito dipenderebbe dall'orologio
///
/// Perche' l'evidenza di un dominio non ancora vuoto e' una fotografia di
/// qualcosa che si muove. Il prototipo lo misura: al ritorno della `wait`
/// l'evidenza di OOM dice zero, e duecento millisecondi dopo dice uno. La
/// stessa esecuzione, letta troppo presto, diventerebbe `Timeout` invece che
/// `LimiteAttribuito` — e la differenza non sarebbe nel worker ma in quando il
/// kernel ha consegnato l'evento.
///
/// Mezzo secondo e' il doppio abbondante di cio' che il prototipo ha visto. Non
/// e' una promessa che basti sempre: se non basta, il dominio resta non
/// quiescente e **si dichiara**, che e' l'esito giusto per un'osservazione che
/// non si e' potuta fare.
const ATTESA_DELLA_QUIESCENZA: Duration = Duration::from_millis(500);

/// Chi legge l'evidenza del dominio.
///
/// Separato dall'osservatore della quiescenza perche' i due si guardano in
/// momenti diversi e per ragioni diverse: la quiescenza mentre si aspetta,
/// l'evidenza **dopo**, quando non cambia piu'.
pub(super) trait LettoreDiEvidenza {
    /// L'evidenza, letta adesso.
    ///
    /// # Errors
    ///
    /// [`Difetto`], che distingue un tentativo da rifare da una lettura mancata.
    fn evidenza(&mut self) -> std::result::Result<EvidenzaDiLimite, Difetto>;
}

/// Chi sa svuotare il dominio.
pub(super) trait Terminatore {
    /// Chiede al dominio di terminare chi lo abita.
    ///
    /// # Errors
    ///
    /// Il motivo, in forma di frase.
    fn termina(&mut self) -> std::result::Result<(), String>;
}

/// Cio' che sta intorno alla conduzione, e che i casi sostituiscono.
///
/// # Perche' un fascio e non cinque parametri
///
/// Perche' cinque parametri in fila si scambiano fra loro senza che il
/// compilatore se ne accorga, quando due hanno lo stesso tipo. Con i nomi, uno
/// scambio non compila.
pub(super) struct Dintorni<O, T, E, P>
where
    O: Osservatore + Send + 'static,
    T: Terminatore,
    E: LettoreDiEvidenza,
    P: ProcessoFiglio,
{
    /// Guarda se il dominio si e' svuotato.
    pub(super) osservatore: O,
    /// Sa svuotarlo, quando si decide di chiudere.
    pub(super) terminatore: T,
    /// Legge l'evidenza, dopo.
    pub(super) evidenza: E,
    /// Il figlio da raccogliere.
    pub(super) figlio: FiglioVivo<P>,
    /// Quanto si aspetta che il canale si disconnetta, alla chiusura.
    ///
    /// # Perche' e' qui e non solo nella coda
    ///
    /// Perche' i casi che provano **cosa succede quando la chiusura e'
    /// sbagliata** — un produttore non fermato, una bocchetta dimenticata —
    /// devono poterlo fare in fretta. Col tetto di produzione ciascuno di
    /// quei casi durerebbe mezzo minuto, e una batteria che dura mezz'ora non
    /// la esegue nessuno: e' il modo in cui un controllo smette di esistere.
    ///
    /// La produzione passa [`Coda::tetto_di_produzione`]. Cio' che un
    /// chiamante di prova puo' variare e' **quanto** si aspetta, mai che cosa
    /// si conclude.
    pub(super) tetto_del_drenaggio: Duration,
    /// Quanto si concede al dominio dopo aver deciso di chiudere.
    ///
    /// Iniettabile per la stessa ragione del tetto: i casi che percorrono le
    /// righe in cui si chiude prima — timeout, cancellazione — pagherebbero due
    /// secondi ciascuno, e una batteria lenta e' una batteria che non si
    /// esegue. La produzione passa [`MARGINE_DI_CORTESIA`].
    pub(super) margine_di_cortesia: Duration,
    /// Quanto si aspetta che il dominio si svuoti **dopo** la forzatura.
    ///
    /// # Perche' e' un'attesa a se'
    ///
    /// Perche' misura una cosa diversa dal margine. Il margine e' cortesia verso
    /// il worker — «chiudi con calma» — e si concede **prima** di forzare.
    /// Questa e' il tempo che il kernel impiega a svuotare davvero il cgroup
    /// **dopo** il segnale, e senza di lei si leggerebbe l'evidenza di un
    /// dominio in cui qualcosa sta ancora morendo.
    ///
    /// La produzione passa [`ATTESA_DELLA_QUIESCENZA`], dove sta anche la
    /// misura del prototipo che ne fissa l'ordine di grandezza. I casi la
    /// accorciano, perche' un caso che aspettasse mezzo secondo per ogni riga
    /// misurerebbe soprattutto la pazienza di chi lo esegue.
    pub(super) attesa_della_quiescenza: Duration,
}

/// Cio' che sta **intorno** all'esito.
///
/// # Perche' il rapporto sta qui e non dentro l'esito
///
/// Perche' l'esito puo' non esserci. Quando la conclusione rifiuta — fatti che
/// si contraddicono, barriera incompleta, un produttore che non e' nato — chi
/// legge ha **piu'** bisogno del rapporto, non meno: e' l'unica cosa che dice
/// quali fatti ci sono. Tenerlo dentro l'esito lo farebbe sparire proprio nei
/// casi in cui serve.
///
/// # Che cosa fanno i difetti
///
/// I difetti sono cose **nostre** — un produttore che non ha potuto dire tutto,
/// un dominio che non si e' lasciato terminare — e non sono un esito: nessuno di
/// loro dice come sia andata l'esecuzione, e nessuno di loro la classifica.
///
/// Entrano pero' nel **giudizio sulla barriera**. Non e' una contraddizione: la
/// barriera non chiede «com'e' andata», chiede «l'ho visto abbastanza da poterlo
/// dire». Un produttore che non ha accodato e un drenaggio che ha rinunciato
/// sono esattamente le ragioni per cui la risposta puo' essere no. Tenerli fuori
/// significherebbe concludere su un'osservazione che sappiamo incompleta —
/// sapendolo, e tacendolo.
///
/// Sui cammini in cui la barriera e' completa restano dove stanno: accanto
/// all'esito, non al suo posto.
/// # Perche' porta il figlio non raccolto
///
/// Perche' un processo che non si e' lasciato raccogliere **esiste ancora**, e
/// il difetto che lo dice non lo raccoglie. Lasciarlo cadere qui dentro
/// significherebbe scaricarne la proprieta' su una riga di testo: la riga
/// descrive la perdita, e poi la perdita accade.
///
/// La guardia risale quindi a chi ha chiamato la conduzione, che e' l'unico
/// sopra di lei a poter ancora riprovare o a decidere di fermarsi. E se anche
/// lui la lascia cadere, la sentinella e' ancora armata: la proprieta' non si
/// perde mai in silenzio, si perde con un abort.
pub(super) struct Contorno<P: ProcessoFiglio> {
    /// Che cosa il registro aveva, riga per riga.
    pub(super) rapporto: Vec<(&'static str, String)>,
    /// Cio' che i produttori non hanno potuto accodare.
    pub(super) resoconti: Vec<String>,
    /// Il drenaggio che non ha visto la disconnessione.
    pub(super) drenaggio: Option<String>,
    /// Il figlio che non si e' lasciato chiudere.
    pub(super) raccolta: Option<String>,
    /// Il dominio che non si e' lasciato terminare.
    pub(super) terminazione: Option<String>,
    /// L'`Annulla` che non si e' potuto mandare.
    ///
    /// Separato dagli altri perche' dice una cosa sua: il worker **non ha
    /// saputo** che qualcuno vuole fermarlo, e quindi non ha modo di
    /// chiudere ordinatamente. Il margine gli e' stato concesso lo stesso, ma su
    /// una richiesta che non gli e' arrivata.
    pub(super) annulla: Option<String>,
    /// Il figlio che non si e' lasciato raccogliere, **ancora sotto guardia**.
    ///
    /// `None` su ogni cammino che l'ha raccolto, che sono quasi tutti. Quando
    /// c'e', chi legge ha in mano un processo vivo e la responsabilita' che ne
    /// consegue: la sentinella scatta se lo lascia cadere.
    pub(super) figlio_non_raccolto: Option<FiglioVivo<P>>,
}

impl<P: ProcessoFiglio> Default for Contorno<P> {
    /// Scritto a mano: quello derivato pretenderebbe `P: Default`, e un processo
    /// non ha un valore predefinito.
    fn default() -> Self {
        Self {
            rapporto: Vec::new(),
            resoconti: Vec::new(),
            drenaggio: None,
            raccolta: None,
            terminazione: None,
            annulla: None,
            figlio_non_raccolto: None,
        }
    }
}

impl<P: ProcessoFiglio> Contorno<P> {
    /// Aggiunge il resoconto di un produttore, se ha qualcosa da dire.
    fn dal_produttore(&mut self, resoconto: Resoconto) {
        if let Some(quale) = resoconto {
            self.resoconti.push(quale.to_string());
        }
    }

    /// Le righe, ordinate, per il rapporto.
    pub(super) fn righe(&self) -> Vec<String> {
        let mut tutte = self.resoconti.clone();
        tutte.extend(self.drenaggio.clone());
        tutte.extend(self.raccolta.clone());
        tutte.extend(self.terminazione.clone());
        tutte.extend(self.annulla.clone());
        tutte.sort();
        tutte.dedup();
        tutte
    }
}

/// Ascolta i fatti finche' c'e' qualcosa da ascoltare.
///
/// # La chiusura, quando si decide di chiuderla
///
/// Nell'ordine della §8.1, e **una volta sola**:
///
/// 1. si **chiede** al worker di smettere, ma solo se qualcuno ha annullato: su
///    un tempo scaduto non si chiede, perche' il tempo che ha e' quello e
///    riaprire una conversazione finita gli darebbe un'attesa in piu' che
///    nessuno gli ha concesso;
/// 2. si aspetta il **margine di cortesia** — un worker che chiude un file
///    ordinatamente lascia meno lavoro al cleanup di uno terminato a meta';
/// 3. scaduto il margine, si **forza**. Non prima: forzare nello stesso istante
///    in cui si chiede vorrebbe dire non aver chiesto niente, e il margine
///    sarebbe una riga di documento senza un comportamento sotto.
fn ascolta<T: Terminatore>(
    coda: &super::coda::Coda,
    registro: &mut Registro,
    ingresso_verso_il_worker: &mut impl std::io::Write,
    terminatore: &mut T,
    difetti: &mut Contorno<impl ProcessoFiglio>,
    margine_di_cortesia: Duration,
    attesa_della_quiescenza: Duration,
) {
    let mut scadenza_di_cortesia = None;
    // Distinto dalla scadenza: e' **la decisione**, e la decisione si prende una
    // volta anche quando la scadenza non si e' potuta calcolare. Legarla alla
    // scadenza fa richiamare il terminatore a ogni giro nel caso in cui la
    // scadenza manchi.
    let mut gia_deciso_di_chiudere = false;
    // Distinto ancora: dopo la forzatura si torna ad ascoltare, e senza questo
    // la condizione del margine forzerebbe a ogni giro.
    let mut gia_forzato = false;
    loop {
        if registro.si_puo_smettere_di_ascoltare() {
            break;
        }
        // Deciso di chiudere, **una volta sola**, e nell'ordine della §8.1:
        // prima si chiede, poi si aspetta, e solo alla fine si forza.
        if registro.si_deve_chiudere() && !gia_deciso_di_chiudere {
            gia_deciso_di_chiudere = true;

            // 1. Si **chiede** al worker di smettere, se e' lui che qualcuno
            //    vuole annullare. Su un tempo scaduto non si chiede: il tempo
            //    che ha e' quello, e riaprire una conversazione il cui
            //    tempo e' finito darebbe al worker un'attesa in piu' che
            //    nessuno gli ha concesso.
            if registro.cancellazione_richiesta() {
                if let Err(motivo) = manda_annulla(ingresso_verso_il_worker) {
                    difetti.annulla = Some(motivo);
                }
            }

            // 2. Il **margine di cortesia**. Un worker che sta chiudendo un file
            //    ordinatamente lascia meno lavoro al cleanup di uno terminato a
            //    meta'. Non e' una garanzia di reazione: e' una preferenza con
            //    una scadenza.
            //
            //    `checked_add` puo' rendere `None`. Lasciare `None` sarebbe il
            //    peggio dei due mondi: non ci sarebbe nessuna scadenza a fermare
            //    l'attesa, e la terminazione forzata non arriverebbe mai. Una
            //    scadenza non rappresentabile vale quindi **gia' passata**, e lo
            //    si dice.
            if let Some(quando) = std::time::Instant::now().checked_add(margine_di_cortesia) {
                scadenza_di_cortesia = Some(quando);
            } else {
                difetti.resoconti.push(format!(
                    "margine di cortesia: {} ms non sono rappresentabili come scadenza, e si \
                     forza subito",
                    margine_di_cortesia.as_millis()
                ));
                scadenza_di_cortesia = Some(std::time::Instant::now());
            }
        }

        // 3. Scaduto il margine, la **terminazione forzata** del dominio. Non
        //    prima: forzare nello stesso istante in cui si chiede vorrebbe dire
        //    non aver chiesto niente, e il margine sarebbe una riga di
        //    documento senza un comportamento sotto.
        if let Some(scadenza) = scadenza_di_cortesia {
            if !gia_forzato && std::time::Instant::now() >= scadenza {
                gia_forzato = true;
                if let Err(motivo) = terminatore.termina() {
                    difetti.terminazione = Some(motivo);
                }
                // 4. E **si continua ad ascoltare**, perche' la forzatura non e'
                //    la fine: e' cio' che *rende* quiescente il dominio, e la
                //    quiescenza arriva un momento dopo. Smettere qui
                //    vorrebbe dire leggere l'evidenza di un dominio in cui
                //    qualcosa puo' ancora morire, e un OOM consegnato dopo
                //    non e' un OOM avvenuto dopo: la stessa esecuzione
                //    diventerebbe `Timeout` o `LimiteAttribuito` secondo quando
                //    arriva.
                //    La seconda scadenza si calcola come la prima, e come la
                //    prima puo' non essere rappresentabile. Un `or_else` muto la
                //    farebbe valere «gia' passata» **senza dirlo**: si
                //    smetterebbe di aspettare la quiescenza al giro dopo, e chi
                //    legge un dominio non quiescente cercherebbe un residuo
                //    invece di una somma che non si e' potuta fare.
                if let Some(quando) = std::time::Instant::now().checked_add(attesa_della_quiescenza)
                {
                    scadenza_di_cortesia = Some(quando);
                } else {
                    difetti.resoconti.push(format!(
                        "attesa della quiescenza: {} ms non sono rappresentabili come scadenza, e non si aspetta",
                        attesa_della_quiescenza.as_millis()
                    ));
                    scadenza_di_cortesia = Some(std::time::Instant::now());
                }
                continue;
            }
            if gia_forzato && std::time::Instant::now() >= scadenza {
                // Forzato, e il dominio non si e' svuotato lo stesso. Non c'e'
                // altro da aspettare: chi non muore con `cgroup.kill` non muore
                // guardandolo piu' a lungo.
                break;
            }
        }
        match coda.prossimo(PASSO_DEL_GIRO) {
            Presa::Fatto(fatto) => registro.applica(fatto),
            Presa::Scaduto => (),
            // Nessuno puo' piu' scrivere: continuare ad aspettare i fatti
            // terminali aspetterebbe qualcuno che non c'e'.
            Presa::Disconnessa => break,
        }
    }
}

/// Raccoglie il figlio, poi legge l'evidenza, e accoda cio' che ha visto.
///
/// # Perche' in quest'ordine, e perche' senza ritorni anticipati
///
/// L'ordine: l'evidenza letta prima della raccolta misurerebbe un dominio in cui
/// qualcosa puo' ancora succedere, e riporterebbe numeri che un istante dopo
/// sono altri.
///
/// L'assenza di ritorni anticipati: ogni passo qui **produce evidenza**, e
/// tornare indietro la sopprimerebbe da li' in poi. Un figlio che non si lascia
/// raccogliere e' un difetto; ma se per quel difetto non si leggesse piu'
/// l'evidenza del dominio, il rapporto direbbe «evidenza non letta» — e
/// manderebbe a cercare un problema di lettura dove il problema e' un figlio.
fn chiudi_il_figlio_e_leggi<P: ProcessoFiglio, E: LettoreDiEvidenza>(
    figlio: FiglioVivo<P>,
    evidenza: &mut E,
    raccoglitore: &mut Bocchetta,
    difetti: &mut Contorno<P>,
    dominio_quiescente: bool,
) {
    //
    // In quest'ordine. L'evidenza letta prima della raccolta misurerebbe un
    // dominio in cui qualcosa puo' ancora succedere, e riporterebbe numeri che
    // un istante dopo sono altri.
    let pid = figlio.pid();
    let uscita = match figlio.termina_e_raccogli(
        LIMITE_DI_RACCOLTA,
        &OrologioDiSistema::nuovo(PASSO_DI_RACCOLTA),
    ) {
        Chiusura::Raccolto {
            uscita,
            difetti: quali,
        } => {
            difetti.raccolta = riunisci(&quali);
            uscita
        }
        Chiusura::NonRaccolto {
            guardia,
            difetti: quali,
        } => {
            // La guardia **risale**, e non si scarica qui.
            //
            // Rinunciare dichiarandolo — pid nel rapporto, difetto accanto —
            // sembra diligenza ed e' una perdita: la riga descrive un processo
            // vivo, e poi lo lascia vivo. Sopra c'e' chi ha chiamato la
            // conduzione, che puo' ancora riprovare o decidere di fermarsi, e la
            // scelta e' sua.
            let mut quali = quali;
            if let Some(numero) = guardia.pid() {
                quali.push(format!(
                    "il figlio {numero} non si e' lasciato raccogliere: la guardia risale a chi \
                     ha chiamato"
                ));
            }
            difetti.raccolta = riunisci(&quali);
            difetti.figlio_non_raccolto = Some(guardia);
            None
        }
    };
    accoda_l_uscita(uscita, pid, raccoglitore);

    // L'evidenza si legge **solo** su un dominio quiescente. Su un dominio
    // ancora abitato i contatori non sono un'osservazione: sono una fotografia
    // di qualcosa che si sta muovendo, e attribuire su quella firma
    // significherebbe far dipendere l'esito da quando il kernel ha consegnato un
    // evento. Non leggerla non e' rinunciare a un'informazione: e' rifiutarne
    // una che non ha significato.
    if !dominio_quiescente {
        let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {
            chi: "evidenza",
            motivo: "il dominio non e' quiescente: i contatori non sono ancora fermi".to_owned(),
        });
        return;
    }

    match evidenza.evidenza() {
        Ok(letta) => {
            let _ = raccoglitore.manda(Fatto::EvidenzaDelDominio(Box::new(letta)));
        }
        Err(Difetto::Interrotta) => {
            let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {
                chi: "evidenza",
                motivo: "la lettura e' stata interrotta e non si e' rifatta".to_owned(),
            });
        }
        Err(Difetto::Impossibile(motivo)) => {
            let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {
                chi: "evidenza",
                motivo,
            });
        }
    }
}

/// Ferma i produttori, li aspetta, e prende il loro resoconto.
///
/// # Perche' le due cose insieme
///
/// Perche' separarle e' il modo di sbagliarle. Fermare senza aspettare lascia
/// vive le bocchette e il canale non si disconnette; aspettare senza fermare
/// aspetta per sempre. Sono un gesto solo, e stanno in una funzione sola.
fn ferma_e_aspetta(
    fili: [(&'static str, JoinHandle<Resoconto>, Freno); 3],
    difetti: &mut Contorno<impl ProcessoFiglio>,
) {
    // Prima **tutti** i freni, poi tutte le attese: fermare e aspettare uno per
    // volta farebbe aspettare il primo mentre il secondo sta ancora lavorando,
    // e la chiusura durerebbe la somma invece del massimo.
    for (_, _, freno) in &fili {
        freno.ferma();
    }
    for (chi, filo, _) in fili {
        match filo.join() {
            Ok(resoconto) => difetti.dal_produttore(resoconto),
            Err(_) => difetti
                .resoconti
                .push(format!("{chi}: il filo e' finito male")),
        }
    }
}

/// Un produttore vivo: la sua maniglia e il suo freno.
type Filo = (JoinHandle<Resoconto>, Freno);

/// I produttori gia' nati, con il nome che serve a riportarli.
type Avviati = Vec<(&'static str, JoinHandle<Resoconto>, Freno)>;

/// Fa nascere i tre produttori, in ordine.
///
/// # Perche' uno per volta e non tutti insieme
///
/// Perche' ognuno puo' non nascere: `Builder::spawn` rende un errore quando il
/// sistema rifiuta un thread, e quel rifiuto arriva al secondo o al terzo tanto
/// quanto al primo. Farli nascere uno per volta significa sapere **chi** ha
/// rifiutato e **quali** sono gia' vivi — cioe' avere in mano cio' che serve per
/// tornare indietro.
///
/// # Errors
///
/// Il nome di chi non e' nato, il motivo che il sistema ha dato, l'elenco di
/// quelli gia' vivi, e **l'osservatore**.
///
/// L'elenco non e' un dettaglio: senza, chi si ritira non saprebbe chi fermare,
/// e resterebbero produttori vivi con la loro bocchetta — il canale non si
/// disconnetterebbe mai.
///
/// L'osservatore nemmeno: chi si ritira forza il dominio, e poi deve
/// **guardare** se si e' svuotato. Su tutti e tre i cammini il sorvegliante non
/// e' mai nato — e' l'ultimo — quindi l'osservatore c'e' ancora: nei primi due
/// perche' non e' stato usato, nel terzo perche' la nascita mancata lo rende
/// indietro.
fn fai_nascere_i_produttori<R, O>(
    canale: CanaleOperativo<R>,
    osservatore: O,
    tempo_di_esecuzione: Duration,
    per_il_lettore: Bocchetta,
    per_l_orologio: Bocchetta,
    per_il_sorvegliante: Bocchetta,
) -> std::result::Result<[Filo; 3], (&'static str, std::io::Error, Avviati, Option<O>)>
where
    R: std::io::Read + Send + 'static,
    O: Osservatore + Send + 'static,
{
    let mut avviati: Avviati = Vec::new();

    let (filo_lettore, freno_lettore) = match avvia_lettore(canale, per_il_lettore) {
        Ok(coppia) => coppia,
        Err(errore) => return Err(("lettore", errore, avviati, Some(osservatore))),
    };
    avviati.push(("lettore", filo_lettore, freno_lettore.clone()));

    let (filo_orologio, freno_orologio) =
        match avvia_orologio(tempo_di_esecuzione, per_l_orologio, PASSO_DEL_GIRO) {
            Ok(coppia) => coppia,
            Err(errore) => return Err(("orologio", errore, avviati, Some(osservatore))),
        };
    avviati.push(("orologio", filo_orologio, freno_orologio.clone()));

    let (filo_sorvegliante, freno_sorvegliante) =
        match avvia_sorvegliante(osservatore, per_il_sorvegliante, PASSO_DEL_GIRO) {
            Ok(coppia) => coppia,
            Err((errore, indietro)) => return Err(("sorvegliante", errore, avviati, indietro)),
        };

    // Ci sono tutti: l'elenco del rollback non serve piu', e le maniglie tornano
    // a essere quelle nominate — la forma in cui il seguito le usa.
    let (_, filo_lettore, _) = avviati.remove(0);
    let (_, filo_orologio, _) = avviati.remove(0);
    Ok([
        (filo_lettore, freno_lettore),
        (filo_orologio, freno_orologio),
        (filo_sorvegliante, freno_sorvegliante),
    ])
}

/// Cio' che serve per chiudere, comunque vada.
///
/// # Perche' un fascio e non sei argomenti
///
/// Perche' sono le stesse sei cose su due cammini opposti — i produttori nascono
/// oppure no — e tenerle insieme dice che la chiusura e' **una**: chi si ritira
/// da una nascita parziale fa cio' che fa la conduzione completa, non una
/// versione ridotta.
struct PerChiudere<T: Terminatore, P: ProcessoFiglio> {
    /// La bocchetta con cui si accoda l'uscita del figlio.
    raccoglitore: Bocchetta,
    /// Il figlio, che va chiuso su ogni cammino.
    figlio: FiglioVivo<P>,
    /// Chi sa svuotare il dominio.
    terminatore: T,
    /// La coda, che va drenata su ogni cammino.
    coda: super::coda::Coda,
    /// Fin dove si drena prima di dichiarare la rinuncia.
    tetto_del_drenaggio: Duration,
    /// Quanto si guarda il dominio dopo averlo forzato.
    attesa_della_quiescenza: Duration,
}

/// Accoda l'uscita del figlio, **com'e'**.
///
/// # Perche' una funzione sola per due cammini
///
/// Perche' i cammini sono due — la chiusura ordinaria e la rinuncia a una
/// nascita parziale — e la conversione e' una. Due copie sono due occasioni di
/// divergere, e quella che diverge e' sempre la seconda: basta che la rinuncia
/// scarti `NonRappresentabile` insieme a `None` perche' «il sistema non sa dirmi
/// come e' finito» valga «non e' ancora finito».
///
/// # Perche' `NonRappresentabile` non e' `None`
///
/// Perche' dicono due cose opposte. `None` e' «non l'ho raccolto», e il difetto
/// della raccolta lo dice gia'; accodare qualcosa li' inventerebbe un'uscita che
/// nessuno ha visto. `NonRappresentabile` e' «l'ho raccolto, ed **e' finito**, ma
/// il sistema non riporta ne' un codice ne' un segnale»: e' un'osservazione
/// mancata su un'uscita avvenuta. Trattarla come niente la farebbe leggere come
/// un worker ancora vivo, e la barriera direbbe «l'uscita non e' stata osservata»
/// senza dire che qualcuno ha guardato e non ha capito.
///
/// # Perche' successo, codice e segnale restano tre cose
///
/// Perche' chi classifica deve poterle distinguere: un'uscita a zero, una
/// diversa da zero e una morte per segnale sono tre righe diverse della matrice.
fn accoda_l_uscita(uscita: Option<Uscita>, pid: Option<u32>, raccoglitore: &mut Bocchetta) {
    match uscita {
        Some(Uscita::Codice(codice)) => {
            let _ = raccoglitore.manda(Fatto::UscitaDelWorker(UscitaOsservata::Codice(codice)));
        }
        Some(Uscita::Segnale(segnale)) => {
            let _ = raccoglitore.manda(Fatto::UscitaDelWorker(UscitaOsservata::Segnale(segnale)));
        }
        Some(Uscita::NonRappresentabile) => {
            let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {
                chi: "uscita",
                motivo: format!(
                    "il figlio {} e' finito, ma il sistema non riporta ne' un codice ne' un segnale",
                    pid.map_or_else(|| "sconosciuto".to_owned(), |numero| numero.to_string())
                ),
            });
        }
        None => (),
    }
}

/// Guarda il dominio finche' non si e' svuotato, o finche' non e' troppo tardi.
///
/// # Perche' guardare, quando si e' gia' chiesto
///
/// Perche' `cgroup.kill` e' **asincrono**: la scrittura torna, e i processi
/// muoiono dopo. Fermarsi alla chiamata vorrebbe dire riportare «ho chiesto»
/// lasciando credere «e' successo», e un dominio ancora abitato ha contatori che
/// non sono un'osservazione.
///
/// # Che cosa accoda
///
/// La quiescenza, se arriva. L'impossibilita' di guardare, se l'osservatore
/// manca o rifiuta. **Niente**, se il tempo finisce: «non si e' svuotato entro»
/// non e' un fatto sul dominio, e' l'assenza del fatto atteso — e la
/// barriera la tratta come tale. Il motivo va accanto, fra i difetti, perche'
/// senza chi legge vedrebbe un dominio abitato e nessuna spiegazione.
fn guarda_che_si_sia_svuotato<O: Osservatore>(
    osservatore: Option<O>,
    entro: Duration,
    raccoglitore: &mut Bocchetta,
    difetti: &mut Contorno<impl ProcessoFiglio>,
) {
    let Some(mut osservatore) = osservatore else {
        let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {
            chi: "quiescenza",
            motivo: "non c'e' nessuno che possa guardare il dominio".to_owned(),
        });
        return;
    };

    // La scadenza e' **assoluta**, come quella del drenaggio: una che si
    // rinnovasse a ogni giro non finirebbe mai su un dominio che risponde.
    // `checked_add` puo' non rappresentarla, e allora non si aspetta — ma lo si
    // dice, perche' un'attesa saltata in silenzio si legge come un'attesa
    // scaduta.
    let Some(fine) = std::time::Instant::now().checked_add(entro) else {
        difetti.resoconti.push(format!(
            "attesa della quiescenza: {} ms non sono rappresentabili come scadenza, e non si guarda",
            entro.as_millis()
        ));
        return;
    };

    loop {
        match osservatore.quiescente() {
            Ok(true) => {
                let _ = raccoglitore.manda(Fatto::DominioQuiescente);
                return;
            }
            // Ancora abitato, oppure interrotto: in tutti e due i casi si
            // riprova. Un'interruzione non e' una mancanza — la lettura non e'
            // avvenuta, e riprovarla la fa avvenire — e un dominio pieno non e'
            // una risposta definitiva finche' c'e' tempo.
            Ok(false) | Err(Difetto::Interrotta) => (),
            Err(Difetto::Impossibile(motivo)) => {
                let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {
                    chi: "quiescenza",
                    motivo,
                });
                return;
            }
        }
        if std::time::Instant::now() >= fine {
            difetti.resoconti.push(format!(
                "il dominio non si e' svuotato entro {} ms dalla forzatura",
                entro.as_millis()
            ));
            return;
        }
        std::thread::sleep(PASSO_DEL_GIRO);
    }
}

/// Torna indietro da una partenza parziale.
///
/// # Perche' l'ordine e' questo
///
/// Prima si fermano e si aspettano i fili gia' vivi, perche' finche' vivono
/// vivono le loro bocchette. Poi cadono le bocchette che il conduttore tiene per
/// se'. Poi **si raccoglie il figlio**: e' l'unica cosa che non si puo'
/// saltare, perche' `FiglioVivo` lasciato cadere fa abortire il processo — la
/// sentinella non distingue un cammino sfortunato da un `?` dimenticato, ed e'
/// giusto cosi'.
///
/// Non si classifica — non c'e' un'esecuzione osservata da classificare — ma si
/// fa tutto il resto: si chiude il dominio, si **guarda che si sia svuotato**,
/// si raccoglie il figlio e si drena.
fn rinuncia<T: Terminatore, P: ProcessoFiglio, O: Osservatore>(
    chi: &'static str,
    errore: &std::io::Error,
    avviati: Avviati,
    annullatore: &Annullatore,
    per_chiudere: PerChiudere<T, P>,
    osservatore: Option<O>,
    mut difetti: Contorno<P>,
) -> (
    std::result::Result<EsitoDelSupervisore, Impedimento>,
    Contorno<P>,
) {
    let PerChiudere {
        raccoglitore,
        figlio,
        mut terminatore,
        coda,
        tetto_del_drenaggio,
        attesa_della_quiescenza,
    } = per_chiudere;
    let mut raccoglitore = raccoglitore;

    // 1. Il dominio si chiude. Il worker **esiste gia'** — lo `spawn` e'
    //    avvenuto prima di arrivare qui — e puo' avere discendenti: rinunciare
    //    senza chiuderlo lascerebbe processi vivi in un dominio che nessuno
    //    guarda piu'.
    if let Err(motivo) = terminatore.termina() {
        difetti.terminazione = Some(motivo);
    }

    // 2. E si **guarda** che si sia svuotato.
    //
    //    Chiedere non e' osservare: `cgroup.kill` e' asincrono, e una rinuncia
    //    che si fermasse alla chiamata direbbe «ho chiesto» lasciando credere
    //    «e' successo». Il sorvegliante qui non c'e' — su tutti i cammini della
    //    rinuncia non e' mai nato — quindi si guarda da soli.
    guarda_che_si_sia_svuotato(
        osservatore,
        attesa_della_quiescenza,
        &mut raccoglitore,
        &mut difetti,
    );

    // 3. I fili gia' vivi si fermano e si aspettano, come su ogni altro cammino.
    for (nome, filo, freno) in avviati {
        freno.ferma();
        match filo.join() {
            Ok(resoconto) => difetti.dal_produttore(resoconto),
            Err(_) => difetti
                .resoconti
                .push(format!("{nome}: il filo e' finito male")),
        }
    }
    difetti.dal_produttore(annullatore.deponi());

    // 4. Il figlio si raccoglie, e **la sua uscita si accoda**: buttarla qui
    //    vorrebbe dire non distinguere un worker gia' morto da un worker che
    //    fermiamo noi.
    let pid = figlio.pid();
    match figlio.termina_e_raccogli(
        LIMITE_DI_RACCOLTA,
        &OrologioDiSistema::nuovo(PASSO_DI_RACCOLTA),
    ) {
        Chiusura::Raccolto {
            uscita,
            difetti: quali,
        } => {
            difetti.raccolta = riunisci(&quali);
            accoda_l_uscita(uscita, pid, &mut raccoglitore);
        }
        Chiusura::NonRaccolto {
            guardia,
            difetti: mut quali,
        } => {
            // Come sul cammino ordinario: la guardia risale, e non si scarica su
            // una riga di rapporto.
            if let Some(numero) = guardia.pid() {
                quali.push(format!(
                    "il figlio {numero} non si e' lasciato raccogliere: la guardia risale a chi \
                     ha chiamato"
                ));
            }
            difetti.raccolta = riunisci(&quali);
            difetti.figlio_non_raccolto = Some(guardia);
        }
    }
    difetti.dal_produttore(raccoglitore.resoconto());

    // 5. Si drena. Il lettore puo' aver gia' accodato, e chi ha ricevuto
    //    l'annullatore puo' aver gia' annullato: quei fatti esistono, e buttarli
    //    renderebbe il rapporto una pagina bianca su un'esecuzione che qualcosa
    //    aveva gia' detto.
    let (tardivi, difetto_di_drenaggio) = coda.chiudi_e_drena_entro(tetto_del_drenaggio);
    difetti.drenaggio = difetto_di_drenaggio;
    let mut registro = Registro::default();
    for fatto in tardivi {
        registro.applica(fatto);
    }
    difetti.rapporto = registro.evidenza_dei_fatti();

    (
        Err(Impedimento::ProduttoreNonNato {
            chi,
            motivo: errore.to_string(),
        }),
        difetti,
    )
}

/// Piu' difetti in una riga sola, o niente se non ce n'e' nessuno.
///
/// I difetti della chiusura di un figlio si **sommano** — una terminazione
/// rifiutata e una raccolta che non arriva sono due cose, e la seconda non
/// spiega la prima — ma il contorno li porta in un campo solo. Qui si uniscono
/// senza perderne nessuno.
fn riunisci(difetti: &[String]) -> Option<String> {
    (!difetti.is_empty()).then(|| difetti.join("; "))
}

/// Il motivo che accompagna l'`Annulla`.
///
/// Fisso e non descrittivo di chi ha annullato: il motivo viaggia sul filo
/// verso un processo che non deve sapere niente di chi sta dall'altra parte, e
/// un testo variabile sarebbe una via per cui qualcosa del supervisore finisce
/// nel worker.
const MOTIVO_DELL_ANNULLA: &str = "annullato dal supervisore";

/// Manda l'`Annulla` sul filo, e si assicura che parta.
///
/// # Perche' `flush` e non solo `write_all`
///
/// Perche' `write_all` mette i byte dove il tipo li accetta, che non e'
/// necessariamente il descrittore: con uno scrittore bufferizzato la richiesta
/// resterebbe in memoria fino alla prossima occasione, e la prossima occasione
/// qui e' la **chiusura dell'ingresso** — cioe' dopo il margine, quando ormai
/// non serve piu'.
///
/// # Errors
///
/// Il motivo, in forma di frase, se il frame non si codifica o non si scrive.
fn manda_annulla(ingresso: &mut impl std::io::Write) -> std::result::Result<(), String> {
    let frame = Frame::nuovo(Corpo::Annulla(Annulla {
        motivo: MOTIVO_DELL_ANNULLA.to_owned(),
    }));
    let byte = codifica(&frame).map_err(|errore| format!("l'Annulla non si codifica: {errore}"))?;
    ingresso
        .write_all(&byte)
        .map_err(|errore| format!("l'Annulla non si scrive: {errore}"))?;
    ingresso
        .flush()
        .map_err(|errore| format!("l'Annulla resta nel buffer: {errore}"))
}

/// Conduce un tentativo dall'inizio alla classificazione.
///
/// # Chi puo' annullare
///
/// Chi riceve l'annullatore da `consegna_annullatore`, che viene chiamata
/// **prima** che la conduzione si metta ad ascoltare. E' l'unico momento utile:
/// dopo, questa funzione non torna finche' non ha concluso, e un annullatore
/// consegnato alla fine potrebbe annullare solo cio' che e' gia' finito.
///
/// # Errors
///
/// [`Impedimento`] quando i fatti non si lasciano ridurre a un esito.
pub(super) fn conduci<R, W, O, T, E, P>(
    canale: CanaleOperativo<R>,
    mut ingresso_verso_il_worker: W,
    tempo_di_esecuzione: Duration,
    dintorni: Dintorni<O, T, E, P>,
    consegna_annullatore: impl FnOnce(&std::sync::Arc<Annullatore>),
) -> (
    std::result::Result<EsitoDelSupervisore, Impedimento>,
    Contorno<P>,
)
where
    R: std::io::Read + Send + 'static,
    W: std::io::Write,
    O: Osservatore + Send + 'static,
    T: Terminatore,
    E: LettoreDiEvidenza,
    P: ProcessoFiglio,
{
    let Dintorni {
        osservatore,
        terminatore,
        mut evidenza,
        figlio,
        tetto_del_drenaggio,
        margine_di_cortesia,
        attesa_della_quiescenza,
    } = dintorni;

    let mut difetti = Contorno::default();
    let (coda, fascio) = apri();
    // L'annullatore si consegna **prima** di mettersi ad ascoltare. Tenerlo per
    // se' vorrebbe dire che nessuno puo' annullare mentre si ascolta, cioe' in
    // ogni momento in cui annullare serve: la cancellazione diventerebbe una
    // parola del documento senza un modo di dirla.
    let annullatore = std::sync::Arc::new(Annullatore::nuovo(fascio.annullatore));
    consegna_annullatore(&annullatore);
    // I tre fili nascono **uno per volta**, e ognuno puo' non nascere. Quando
    // uno rifiuta, si torna indietro su quelli gia' vivi: fermarli, aspettarli,
    // e lasciar cadere le bocchette. Senza il rollback resterebbero produttori
    // vivi con la loro bocchetta, il canale non si disconnetterebbe mai, e —
    // peggio — `FiglioVivo` cadrebbe senza passare da nessuna delle sue porte,
    // facendo scattare la sentinella su un cammino che e' solo sfortunato.
    let per_chiudere = PerChiudere {
        raccoglitore: fascio.raccoglitore,
        figlio,
        terminatore,
        coda,
        tetto_del_drenaggio,
        attesa_della_quiescenza,
    };
    let [(filo_lettore, freno_lettore), (filo_orologio, freno_orologio), (filo_sorvegliante, freno_sorvegliante)] =
        match fai_nascere_i_produttori(
            canale,
            osservatore,
            tempo_di_esecuzione,
            fascio.lettore,
            fascio.orologio,
            fascio.sorvegliante,
        ) {
            Ok(tre) => tre,
            Err((chi, errore, avviati, indietro)) => {
                return rinuncia(
                    chi,
                    &errore,
                    avviati,
                    &annullatore,
                    per_chiudere,
                    indietro,
                    difetti,
                )
            }
        };

    // Da qui in poi ci sono tutti, e il fascio si riapre: la chiusura ordinaria
    // usa le stesse sei cose, una per volta.
    let PerChiudere {
        mut raccoglitore,
        figlio,
        mut terminatore,
        coda,
        tetto_del_drenaggio,
        attesa_della_quiescenza,
    } = per_chiudere;

    let mut registro = Registro::default();

    // --- 1. si ascolta ------------------------------------------------------
    //
    // In una funzione sua perche' e' una regola intera: si ascolta finche' i
    // produttori hanno da dire, e quando si decide di chiudere si segue la
    // §8.1 — si chiede, si aspetta, si forza. Leggerla in mezzo al resto la
    // farebbe sembrare una serie di condizioni invece di una sequenza.
    ascolta(
        &coda,
        &mut registro,
        &mut ingresso_verso_il_worker,
        &mut terminatore,
        &mut difetti,
        margine_di_cortesia,
        attesa_della_quiescenza,
    );

    // --- 2. si chiudono gli ingressi ---------------------------------------
    //
    // Da qui il worker vede la fine del proprio ingresso. Prima di questo punto
    // il canale resta aperto, perche' finche' si ascolta si puo' ancora
    // annullare.
    drop(ingresso_verso_il_worker);

    // --- 3. si fermano i produttori, e li si aspetta ------------------------
    //
    // In una funzione sua: non per lunghezza, ma perche' e' una sequenza con
    // una regola propria — fermare, aspettare, raccogliere il resoconto — che si
    // legge tutta insieme o non si legge.
    ferma_e_aspetta(
        [
            ("lettore", filo_lettore, freno_lettore),
            ("orologio", filo_orologio, freno_orologio),
            ("sorvegliante", filo_sorvegliante, freno_sorvegliante),
        ],
        &mut difetti,
    );
    // La bocchetta dell'annullatore cade qui. Chi lo ha ricevuto puo' tenerne
    // ancora una copia: `deponi` la svuota comunque, e da quel momento la sua
    // richiesta non accoda piu' niente — il che e' giusto, perche' la coda non
    // ascolta piu'.
    difetti.dal_produttore(annullatore.deponi());

    // --- 4. si prende cio' che i produttori hanno gia' detto ----------------
    //
    // **Prima** di decidere se leggere l'evidenza, e non dopo. I tre fili sono
    // stati fermati e aspettati: cio' che hanno accodato e' tutto qui, e
    // nient'altro puo' arrivare da loro.
    //
    // Senza questo passo la quiescenza accodata **dopo l'ultima interrogazione
    // del giro** resterebbe invisibile fino al drenaggio finale: l'evidenza
    // verrebbe saltata perche' il registro dice ancora «abitato», e il drenaggio
    // subito dopo renderebbe il registro quiescente. La conclusione troverebbe
    // la barriera completa e classificherebbe **senza evidenza** — «tempo
    // scaduto» su un'esecuzione uccisa dall'OOM. E' una finestra piccola, ed e'
    // esattamente la finestra in cui i fatti terminali arrivano.
    for fatto in coda.raccogli_i_fermi() {
        registro.applica(fatto);
    }

    // --- 5. si raccoglie il figlio, e poi si legge l'evidenza ---------------
    chiudi_il_figlio_e_leggi(
        figlio,
        &mut evidenza,
        &mut raccoglitore,
        &mut difetti,
        registro.dominio_quiescente(),
    );
    difetti.dal_produttore(raccoglitore.resoconto());

    // --- 6. si drena --------------------------------------------------------
    //
    // Restano i fatti che questa funzione stessa ha appena accodato — l'uscita
    // del figlio, l'evidenza — e nient'altro: qui il drenaggio serve a vederli
    // e a stabilire che il canale si e' davvero disconnesso.
    let (tardivi, difetto_di_drenaggio) = coda.chiudi_e_drena_entro(tetto_del_drenaggio);
    for fatto in tardivi {
        registro.applica(fatto);
    }
    difetti.drenaggio = difetto_di_drenaggio;

    // --- 7. si conclude, una volta sola ------------------------------------
    // La conclusione riceve i difetti della conduzione: un figlio non
    // raccolto, un'evidenza illeggibile, un produttore morto o un drenaggio
    // incompleto sono ragioni per **non proseguire**, e tenerli in una seconda
    // meta' della tupla li lascerebbe accanto a un permesso di andare avanti.
    // Il rapporto si prende **prima** di concludere, perche' `concludi` consuma
    // il registro e perche' deve uscire anche quando la conclusione rifiuta.
    difetti.rapporto = registro.evidenza_dei_fatti();
    let verdetto = registro.concludi(&difetti.righe());
    (verdetto, difetti)
}

#[cfg(test)]
mod matrice;
#[cfg(test)]
mod tests;
