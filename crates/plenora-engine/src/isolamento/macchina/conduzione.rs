//! La conduzione: un solo consumatore, e una chiusura che non perde niente.
//!
//! Sequenza: si ascolta finche' i fatti terminali non ci sono tutti (o si
//! decide di chiudere); si chiude l'ingresso del worker; si fermano **e** si
//! aspettano i produttori; si raccoglie il figlio e poi si legge l'evidenza; si
//! drena fino a `Disconnected`; si conclude una volta sola.
//!
//! Niente resta fuori dal drenaggio: il canale dice `Disconnected` solo quando
//! ogni bocchetta e' caduta, cioe' dopo che ogni produttore e' stato aspettato.
//! Niente compare dopo lo snapshot: `chiudi_e_drena` consuma la coda e il
//! ricevitore, e la garanzia sta nel tipo.

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
use super::{EsitoDelSupervisore, Fatto, Impedimento, Registro, Ruolo, UscitaOsservata};
use crate::isolamento::sorgente::Freno;

/// Ogni quanto il consumatore torna a guardare se e' ora di chiudere.
///
/// Governa **l'attesa che si chiede**, non quella che si ottiene: come per il
/// lettore, `recv_timeout` promette di non tornare prima, non di tornare entro.
const PASSO_DEL_GIRO: Duration = Duration::from_millis(10);

/// Quanto si concede al dominio, dopo aver deciso di chiudere, prima di
/// smettere di aspettare i fatti terminali.
///
/// Non zero, perche' fra la decisione e la quiescenza c'e' lavoro vero, e un
/// «non quiescente» spurio manderebbe a cercare un residuo che non c'e'. Non
/// illimitato, perche' un dominio che non si svuota non si svuota guardandolo
/// di piu': e' un fatto da riportare.
pub(super) const MARGINE_DI_CORTESIA: Duration = Duration::from_secs(2);

/// Quanto si aspetta che il dominio si svuoti **dopo** la forzatura.
///
/// Non e' il margine di cortesia: e' il ritardo fra `cgroup.kill` e il suo
/// effetto. Senza, l'evidenza fotograferebbe un dominio che si muove: il
/// prototipo vede l'evidenza di OOM a zero al ritorno della `wait` e a uno
/// duecento millisecondi dopo, e la stessa esecuzione diventerebbe `Timeout`
/// invece di `LimiteAttribuito`. Il valore e' il doppio abbondante di quella
/// misura; se non basta, il dominio resta non quiescente e **si dichiara**.
pub(super) const ATTESA_DELLA_QUIESCENZA: Duration = Duration::from_millis(500);

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
/// Un fascio con nomi e non parametri in fila: due dello stesso tipo si
/// scambierebbero senza che il compilatore se ne accorga.
pub(super) struct Dintorni<O, T, E, P>
where
    O: Osservatore + Send + 'static,
    T: Terminatore,
    E: LettoreDiEvidenza,
    P: ProcessoFiglio,
{
    /// Chi sta dall'altro capo del dominio: decide quale corpo del protocollo
    /// chiude il dialogo (vedi [`Ruolo`] e [`Registro::messaggio`]).
    pub(super) ruolo: Ruolo,
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
    /// La produzione passa [`Coda::tetto_di_produzione`]; i casi che provano
    /// una chiusura sbagliata lo accorciano per restare veloci. Un chiamante di
    /// prova varia **quanto** si aspetta, mai che cosa si conclude.
    pub(super) tetto_del_drenaggio: Duration,
    /// Quanto si concede al dominio dopo aver deciso di chiudere.
    ///
    /// Iniettabile per la stessa ragione del tetto. La produzione passa
    /// [`MARGINE_DI_CORTESIA`].
    pub(super) margine_di_cortesia: Duration,
    /// Quanto si aspetta che il dominio si svuoti **dopo** la forzatura.
    ///
    /// La produzione passa [`ATTESA_DELLA_QUIESCENZA`], che spiega perche' e'
    /// un'attesa distinta dal margine; i casi la accorciano.
    pub(super) attesa_della_quiescenza: Duration,
}

/// Cio' che sta **intorno** all'esito.
///
/// Il rapporto sta qui perche' l'esito puo' mancare, e quando la conclusione
/// rifiuta il rapporto e' l'unica cosa che dice quali fatti ci sono.
///
/// I difetti sono cose **nostre** e non classificano l'esecuzione, ma entrano
/// nel giudizio sulla barriera: dicono se si e' visto abbastanza per
/// concludere. Con la barriera completa restano accanto all'esito.
///
/// Il figlio non raccolto esiste ancora, e la sua guardia risale a chi ha
/// chiamato la conduzione, l'unico che puo' riprovare o fermarsi. Se anche lui
/// la lascia cadere, la sentinella abortisce: la proprieta' non si perde in
/// silenzio.
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
    /// Il dominio che, terminato, non si e' svuotato entro l'attesa: processi
    /// possono esservi rimasti anche se `cgroup.kill` si e' scritto.
    pub(super) abitato: Option<String>,
    /// L'`Annulla` che non si e' potuto mandare.
    ///
    /// Separato perche' dice che il worker **non ha saputo** di essere fermato:
    /// il margine gli e' stato concesso su una richiesta che non gli e' arrivata.
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
            abitato: None,
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
        tutte.extend(self.abitato.clone());
        tutte.extend(self.annulla.clone());
        tutte.sort();
        tutte.dedup();
        tutte
    }
}

/// Ascolta i fatti finche' c'e' qualcosa da ascoltare.
///
/// Decisa la chiusura, nell'ordine della §8.1 e **una volta sola**: si chiede
/// (solo su cancellazione), si aspetta il margine di cortesia, e solo dopo si
/// forza.
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
    // Distinto dalla scadenza: la decisione si prende una volta anche quando la
    // scadenza non si e' potuta calcolare.
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

            // 1. Si **chiede** al worker di smettere, solo su cancellazione: su
            //    un tempo scaduto chiedere gli darebbe un'attesa in piu' che
            //    nessuno gli ha concesso.
            if registro.cancellazione_richiesta() {
                if let Err(motivo) = manda_annulla(ingresso_verso_il_worker) {
                    difetti.annulla = Some(motivo);
                }
            }

            // 2. Il **margine di cortesia**: una preferenza con una scadenza,
            //    non una garanzia di reazione. Una scadenza non rappresentabile
            //    vale **gia' passata**, e lo si dice: senza scadenza la
            //    forzatura non arriverebbe mai.
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
        //    prima: forzare subito vorrebbe dire non aver chiesto niente.
        if let Some(scadenza) = scadenza_di_cortesia {
            if !gia_forzato && std::time::Instant::now() >= scadenza {
                gia_forzato = true;
                if let Err(motivo) = terminatore.termina() {
                    difetti.terminazione = Some(motivo);
                }
                // 4. E **si continua ad ascoltare**: la quiescenza arriva un
                //    momento dopo la forzatura (vedi `ATTESA_DELLA_QUIESCENZA`).
                //    Una seconda scadenza non rappresentabile si dichiara, come
                //    la prima, invece di valere «gia' passata» in silenzio.
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
                // guardandolo piu' a lungo. E' un fatto dell'esito, come nella
                // rinuncia: processi possono essere rimasti.
                if !registro.dominio_quiescente() {
                    difetti.abitato = Some(format!(
                        "il dominio non si e' svuotato entro {} ms dalla forzatura",
                        attesa_della_quiescenza.as_millis()
                    ));
                }
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
/// In quest'ordine, perche' l'evidenza letta prima della raccolta misurerebbe un
/// dominio ancora in movimento. Senza ritorni anticipati, perche' ogni passo
/// produce evidenza: un figlio non raccolto non deve sopprimere la lettura del
/// dominio.
fn chiudi_il_figlio_e_leggi<P: ProcessoFiglio, E: LettoreDiEvidenza>(
    figlio: FiglioVivo<P>,
    evidenza: &mut E,
    raccoglitore: &mut Bocchetta,
    difetti: &mut Contorno<P>,
    dominio_quiescente: bool,
) {
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
            // La guardia **risale** a chi ha chiamato (vedi `Contorno`), e non
            // si scarica su una riga di rapporto.
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

    // L'evidenza si legge **solo** su un dominio quiescente: su uno abitato i
    // contatori si muovono ancora, e l'esito dipenderebbe da quando il kernel
    // consegna un evento.
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
/// Un gesto solo: fermare senza aspettare lascia vive le bocchette, aspettare
/// senza fermare aspetta per sempre.
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
/// Uno per volta, perche' ognuno puo' non nascere (`Builder::spawn` puo'
/// rifiutare): cosi' si sa **chi** ha rifiutato e **quali** sono gia' vivi.
///
/// # Errors
///
/// Il nome di chi non e' nato, il motivo del sistema, l'elenco di quelli gia'
/// vivi (da fermare, o il canale non si disconnetterebbe mai) e l'osservatore,
/// che serve a chi si ritira per guardare se il dominio si e' svuotato. Il
/// sorvegliante nasce per ultimo, quindi su ogni cammino d'errore l'osservatore
/// c'e' ancora.
fn fai_nascere_i_produttori<R, O>(
    canale: CanaleOperativo<R>,
    osservatore: O,
    tempo_di_esecuzione: Duration,
    per_il_lettore: Bocchetta,
    per_l_orologio: Bocchetta,
    per_il_sorvegliante: Bocchetta,
    ruolo: Ruolo,
) -> std::result::Result<[Filo; 3], (&'static str, std::io::Error, Avviati, Option<O>)>
where
    R: std::io::Read + Send + 'static,
    O: Osservatore + Send + 'static,
{
    let mut avviati: Avviati = Vec::new();

    let (filo_lettore, freno_lettore) = match avvia_lettore(canale, per_il_lettore, ruolo) {
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
/// Le stesse cose servono sia se i produttori nascono sia se no: la chiusura e'
/// **una**, e la rinuncia non ne fa una versione ridotta.
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
/// Una funzione sola per la chiusura ordinaria e per la rinuncia, perche' la
/// conversione non diverga fra le due.
///
/// `None` e' «non raccolto», e il difetto della raccolta lo dice gia': non si
/// accoda niente. `NonRappresentabile` e' un'uscita **avvenuta** che il sistema
/// non sa descrivere, e si accoda come osservazione impossibile: trattarla come
/// `None` la farebbe leggere come un worker ancora vivo. Codice e segnale
/// restano distinti perche' sono righe diverse della matrice.
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
/// Serve perche' `cgroup.kill` e' **asincrono**: la scrittura torna, e i
/// processi muoiono dopo.
///
/// Accoda la quiescenza se arriva, l'impossibilita' di guardare se
/// l'osservatore manca o rifiuta, e **niente** se il tempo finisce: e' l'assenza
/// del fatto atteso, e il motivo va fra i difetti.
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

    // La scadenza e' **assoluta**, come quella del drenaggio. Se non e'
    // rappresentabile non si aspetta, e lo si dice.
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
            // Ancora abitato, oppure interrotto: finche' c'e' tempo si riprova.
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
            difetti.abitato = Some(format!(
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
/// Non si classifica, ma si fa tutto il resto: si chiude il dominio, si guarda
/// che si sia svuotato, si fermano i fili gia' vivi, si **raccoglie il figlio**
/// (un `FiglioVivo` lasciato cadere fa abortire il processo) e si drena.
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

    // 1. Il dominio si chiude: il worker **esiste gia'** e puo' avere
    //    discendenti.
    if let Err(motivo) = terminatore.termina() {
        difetti.terminazione = Some(motivo);
    }

    // 2. E si **guarda** che si sia svuotato, da soli: sui cammini della
    //    rinuncia il sorvegliante non e' mai nato.
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

    // 5. Si drena: il lettore e l'annullatore possono aver gia' accodato, e
    //    quei fatti vanno nel rapporto.
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
/// Fisso: un testo variabile porterebbe nel worker qualcosa del supervisore.
const MOTIVO_DELL_ANNULLA: &str = "annullato dal supervisore";

/// Manda l'`Annulla` sul filo, e si assicura che parta.
///
/// Serve `flush`: con uno scrittore bufferizzato la richiesta partirebbe solo
/// alla chiusura dell'ingresso, dopo il margine, quando non serve piu'.
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
/// Puo' annullare chi riceve l'annullatore da `consegna_annullatore`, chiamata
/// **prima** di mettersi ad ascoltare: dopo, la funzione non torna finche' non
/// ha concluso.
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
        ruolo,
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
    // L'annullatore si consegna **prima** di mettersi ad ascoltare.
    let annullatore = std::sync::Arc::new(Annullatore::nuovo(fascio.annullatore));
    consegna_annullatore(&annullatore);
    // Se un filo non nasce, `rinuncia` torna indietro su quelli gia' vivi e
    // raccoglie il figlio.
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
            ruolo,
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

    // `Registro::nuovo`, non `default()`: questo e' il registro che decide
    // se pubblicare, e il ruolo che sceglie fra `Corpo::Esito` e
    // `Corpo::EsitoVerifica` deve essere quello vero, mai quello di comodo.
    let mut registro = Registro::nuovo(ruolo);

    // --- 1. si ascolta ------------------------------------------------------
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
    ferma_e_aspetta(
        [
            ("lettore", filo_lettore, freno_lettore),
            ("orologio", filo_orologio, freno_orologio),
            ("sorvegliante", filo_sorvegliante, freno_sorvegliante),
        ],
        &mut difetti,
    );
    // La bocchetta dell'annullatore cade qui: `deponi` la svuota anche se chi
    // lo ha ricevuto ne tiene una copia.
    difetti.dal_produttore(annullatore.deponi());

    // --- 4. si prende cio' che i produttori hanno gia' detto ----------------
    //
    // **Prima** di decidere se leggere l'evidenza. Una quiescenza accodata dopo
    // l'ultima interrogazione resterebbe invisibile fino al drenaggio: l'evidenza verrebbe
    // saltata e la conclusione classificherebbe **senza evidenza** («tempo
    // scaduto» su un'esecuzione uccisa dall'OOM).
    for fatto in coda.raccogli_i_fermi() {
        registro.applica(fatto);
    }
    riconcilia_l_abitato(&registro, &mut difetti);

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
    // Restano l'uscita del figlio e l'evidenza; il drenaggio stabilisce anche
    // che il canale si e' davvero disconnesso.
    let (tardivi, difetto_di_drenaggio) = coda.chiudi_e_drena_entro(tetto_del_drenaggio);
    for fatto in tardivi {
        registro.applica(fatto);
    }
    riconcilia_l_abitato(&registro, &mut difetti);
    difetti.drenaggio = difetto_di_drenaggio;

    // --- 7. si conclude, una volta sola ------------------------------------
    // La conclusione riceve i difetti della conduzione, che sono ragioni per
    // **non proseguire**. Il rapporto si prende prima: `concludi` consuma il
    // registro, e il rapporto deve uscire anche quando la conclusione rifiuta.
    difetti.rapporto = registro.evidenza_dei_fatti();
    let verdetto = registro.concludi(&difetti.righe());
    (verdetto, difetti)
}

/// Il dominio «abitato» lo decide il giro al momento in cui smette di
/// aspettare, e il registro puo' ancora non aver visto la quiescenza gia'
/// accodata. Applicati i fatti in coda, un dominio quiescente non e' abitato.
fn riconcilia_l_abitato<P: ProcessoFiglio>(registro: &Registro, difetti: &mut Contorno<P>) {
    if registro.dominio_quiescente() {
        difetti.abitato = None;
    }
}

#[cfg(test)]
mod matrice;
#[cfg(test)]
mod tests;
