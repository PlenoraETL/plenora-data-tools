//! La coda dei fatti: una sola, limitata, con lo spazio dei terminali riservato.
//!
//! Una sola coda perche' due code riaprono due arbitrati: l'ordine fra vie
//! (la quiescenza osservata mentre un `Esito` gia' arrivato aspetta
//! sull'altra) e la chiusura atomica di due ingressi.
//!
//! Limitata perche' il worker sceglie quanti messaggi mandare. Perche' una
//! coda piena di `Progresso` non tenga fuori una cancellazione servono
//! insieme due cose:
//!
//! 1. il progresso si coalesce prima di entrare, in un fatto solo;
//! 2. ogni produttore ha un budget finito e la capacita' e' la somma dei
//!    budget: il posto di un terminale e' suo per costruzione.
//!
//! La chiusura non guarda se la coda e' vuota: lascia cadere tutte le
//! bocchette e drena fino a `Disconnected`, l'unico segnale che nessuno puo'
//! piu' scrivere.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

/// Quanto si aspetta che il canale si disconnetta, prima di dire che qualcuno
/// e' rimasto vivo.
///
/// Non e' una stima: un drenaggio ordinario finisce subito, e il tetto sta
/// molto oltre, cosi' che scattare significhi «qualcuno e' rimasto vivo» e mai
/// «e' stato lento». La scadenza e' assoluta: si misura una volta e non
/// riparte a ogni fatto, altrimenti un produttore che manda qualcosa di
/// continuo terrebbe aperto il drenaggio per sempre.
///
/// Limite registrato in `errori-e-limiti.md`.
const TETTO_DEL_DRENAGGIO: std::time::Duration = std::time::Duration::from_secs(30);

use super::Fatto;

/// Chi accoda, e quanto puo' accodare.
///
/// Il budget sta nel tipo: chiedere una bocchetta dichiara il produttore, e
/// accodare di piu' richiede un produttore nuovo, che cambia la capacita'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Produttore {
    /// Legge il canale del worker.
    ///
    /// Quattro fatti: l'esito, il progresso **coalesciuto** in uno solo, un
    /// eventuale fatto di protocollo, e la fine del canale. Non uno per
    /// messaggio: quanti messaggi manda il worker non deve decidere quanto
    /// spazio occupa in coda.
    Lettore,
    /// Misura il tempo dell'esecuzione.
    ///
    /// Due fatti e non uno: la scadenza, oppure il non essere riuscito a
    /// rappresentarla. Sono esiti alternativi, ma un budget si conta sul
    /// peggiore, e tenerlo a due lascia posto senza toglierlo a nessuno.
    Orologio,
    /// Porta la cancellazione di chi la chiede.
    Annullatore,
    /// Guarda il dominio.
    ///
    /// Due fatti: la quiescenza, oppure il non essere riuscito a guardarla — e
    /// il secondo puo' seguire il primo se il dominio smette di rispondere dopo
    /// aver detto di essere vuoto.
    Sorvegliante,
    /// Raccoglie il figlio e legge l'evidenza, dopo la quiescenza.
    Raccoglitore,
}

impl Produttore {
    /// Quanti fatti puo' accodare, al piu'.
    pub(super) const fn budget(self) -> usize {
        match self {
            Self::Lettore => 4,
            Self::Orologio | Self::Sorvegliante | Self::Raccoglitore => 2,
            Self::Annullatore => 1,
        }
    }

    /// Se i suoi fatti concludono l'attesa.
    ///
    /// Serve a dire quali sono i produttori il cui posto non deve poter essere
    /// occupato da nessun altro, e a fissarlo in un caso invece che in un
    /// commento.
    #[cfg(any(test, feature = "internals"))]
    pub(super) const fn terminale(self) -> bool {
        match self {
            Self::Orologio | Self::Annullatore | Self::Sorvegliante | Self::Raccoglitore => true,
            // Il lettore accoda anche la fine del canale, che e' terminale, ma
            // accoda pure messaggi che non lo sono: e' l'unico che porta fatti
            // ripetibili, ed e' quindi l'unico da cui gli altri vanno protetti.
            Self::Lettore => false,
        }
    }

    /// Tutti, per i casi e per il conto della capacita'.
    pub(super) const TUTTI: [Self; 5] = [
        Self::Lettore,
        Self::Orologio,
        Self::Annullatore,
        Self::Sorvegliante,
        Self::Raccoglitore,
    ];

    /// Il nome, per l'evidenza.
    pub(super) const fn nome(self) -> &'static str {
        match self {
            Self::Lettore => "lettore",
            Self::Orologio => "orologio",
            Self::Annullatore => "annullatore",
            Self::Sorvegliante => "sorvegliante",
            Self::Raccoglitore => "raccoglitore",
        }
    }
}

/// La capacita' della coda: **la somma dei budget**, non una stima.
///
/// Scritta come somma e non come numero perche' un numero si scollega: chi
/// aggiunge un produttore domani non deve ricordarsi di aggiornare anche
/// questa, e con la somma non puo' dimenticarsene.
pub(super) const CAPACITA: usize = {
    let mut totale = 0;
    let mut indice = 0;
    while indice < Produttore::TUTTI.len() {
        totale += Produttore::TUTTI[indice].budget();
        indice += 1;
    }
    totale
};

/// Prende tutto cio' che e' gia' in coda, senza aspettare niente.
///
/// Serve nei due punti in cui si rinuncia ad aspettare: un tetto non
/// rappresentabile, e una scadenza passata. In entrambi il tempo dell'**attesa**
/// e' finito, ma i fatti gia' arrivati non c'entrano — e lasciarli li' li
/// perderebbe senza dirlo.
fn svuota_senza_bloccare(ricevitore: &Receiver<Fatto>, dentro: &mut Vec<Fatto>) {
    while let Ok(fatto) = ricevitore.try_recv() {
        dentro.push(fatto);
    }
}

/// Un istante oltre il quale non si aspetta piu'.
///
/// E' un tipo e non una somma sul posto perche' la regola — si calcola una
/// volta e non riparte — si provi con istanti scelti, senza far passare il
/// tempo davvero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Scadenza {
    fine: std::time::Instant,
}

impl Scadenza {
    /// La scadenza, se il tetto e' rappresentabile.
    ///
    /// Rende `None` sull'overflow invece di andare in panico: e'
    /// un'impossibilita' che va **osservata**, perche' presumerla e' esattamente
    /// il modo in cui un panico compare nel posto peggiore.
    fn nuova(inizio: std::time::Instant, tetto: std::time::Duration) -> Option<Self> {
        inizio.checked_add(tetto).map(|fine| Self { fine })
    }

    /// Quanto manca, adesso. Zero quando e' passata.
    ///
    /// Non dipende da che cosa e' successo nel frattempo: e' la differenza fra
    /// due istanti, e nessun fatto che arriva la sposta.
    fn rimasto(self, adesso: std::time::Instant) -> std::time::Duration {
        self.fine.saturating_duration_since(adesso)
    }
}

/// Il posto di un produttore in coda, con i suoi gettoni.
///
/// Finito il budget, `manda` rifiuta invece di bloccare: un produttore
/// bloccato non riporta piu' niente e non puo' dirlo. Dentro il budget il
/// rifiuto non arriva, perche' la capacita' e' la somma dei budget.
///
/// Non e' clonabile e nasce solo in [`apri`]: una copia raddoppierebbe i
/// gettoni senza raddoppiare la capacita', e un terminale potrebbe trovare la
/// coda piena.
#[derive(Debug)]
pub(super) struct Bocchetta {
    chi: Produttore,
    rimasti: usize,
    canale: SyncSender<Fatto>,
    /// Il primo rifiuto, se ce n'e' stato uno.
    ///
    /// Non si accoda, perche' nasce quando la coda non accetta piu': il
    /// produttore lo rende dal proprio `JoinHandle`, una via che non passa
    /// dalla coda.
    primo_rifiuto: Option<Esaurita>,
}

impl Bocchetta {
    /// Accoda un fatto, se restano gettoni.
    ///
    /// # Errors
    ///
    /// [`Esaurita`] quando il budget e' finito, o quando la coda e' piena, che
    /// per costruzione non accade: il primo caso indica un produttore troppo
    /// loquace, il secondo un invariante rotto.
    pub(super) fn manda(&mut self, fatto: Fatto) -> std::result::Result<(), Esaurita> {
        if self.rimasti == 0 {
            return Err(self.annota(Esaurita::Budget(self.chi)));
        }
        match self.canale.try_send(fatto) {
            Ok(()) => {
                self.rimasti -= 1;
                Ok(())
            }
            // La coda piena e' irraggiungibile finche' la capacita' e' la somma
            // dei budget. Se accade, l'invariante e' rotto e va detto con la sua
            // parola invece che confuso con un produttore esaurito.
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                Err(self.annota(Esaurita::CodaPiena(self.chi)))
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                Err(self.annota(Esaurita::NessunAscoltatore(self.chi)))
            }
        }
    }

    /// Registra il primo rifiuto e lo rende.
    ///
    /// Il primo, perche' i successivi ne sono la conseguenza. Da una bocchetta
    /// sola i rifiuti hanno tutti lo stesso valore, quindi nessuna esecuzione
    /// raggiungibile distingue il primo dall'ultimo: e' stabilita' dell'ordine,
    /// non un controllo che un caso possa provare.
    const fn annota(&mut self, quale: Esaurita) -> Esaurita {
        if self.primo_rifiuto.is_none() {
            self.primo_rifiuto = Some(quale);
        }
        quale
    }

    /// Quanti gettoni restano.
    #[cfg(any(test, feature = "internals"))]
    pub(super) const fn rimasti(&self) -> usize {
        self.rimasti
    }

    /// Che cosa la bocchetta non ha potuto dire, **consumandola**.
    ///
    /// E' cio' che il produttore rende dal proprio `JoinHandle`: una via che non
    /// passa dalla coda, e che quindi funziona anche quando la coda e' il
    /// problema. Consumare la bocchetta e' anche il modo di garantire che il
    /// produttore la lasci cadere — e finche' non cade, `recv` non dira' mai
    /// `Disconnected`.
    pub(super) fn resoconto(self) -> Option<Esaurita> {
        self.primo_rifiuto
    }
}

/// Perche' un fatto non entra in coda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Esaurita {
    /// Il produttore ha finito i suoi gettoni.
    Budget(Produttore),
    /// La coda e' piena: un invariante rotto, non un produttore loquace.
    CodaPiena(Produttore),
    /// Il consumatore non c'e' piu'.
    NessunAscoltatore(Produttore),
}

impl std::fmt::Display for Esaurita {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Budget(chi) => write!(
                f,
                "{}: budget di fatti esaurito, il resto non si accoda",
                chi.nome()
            ),
            Self::CodaPiena(chi) => write!(
                f,
                "{}: coda piena, che con la capacita' pari alla somma dei budget non puo' accadere",
                chi.nome()
            ),
            Self::NessunAscoltatore(chi) => {
                write!(f, "{}: il consumatore non c'e' piu'", chi.nome())
            }
        }
    }
}

/// Le bocchette dei produttori, consegnate una volta sola.
///
/// Un fascio con i nomi e non una funzione che le distribuisce, perche' una
/// funzione si chiama due volte: due lettori spenderebbero lo spazio
/// riservato ai terminali. I campi, non clonabili, nascono tutti insieme in
/// [`apri`].
#[derive(Debug)]
pub(super) struct Fascio {
    pub(super) lettore: Bocchetta,
    pub(super) orologio: Bocchetta,
    pub(super) annullatore: Bocchetta,
    pub(super) sorvegliante: Bocchetta,
    pub(super) raccoglitore: Bocchetta,
}

/// Che cosa si e' preso dalla coda.
///
/// Tre risposte e non due: senza la terza, chi ascolta non puo' distinguere una
/// pausa dalla fine, e o smette troppo presto o aspetta per sempre.
#[derive(Debug)]
pub(super) enum Presa {
    /// Un fatto.
    Fatto(Fatto),
    /// Il tempo dato e' finito, e non e' arrivato niente. Puo' ancora arrivare.
    Scaduto,
    /// Nessuno puo' piu' scrivere: non arrivera' piu' niente.
    Disconnessa,
}

/// La coda, dal lato di chi consuma.
///
/// **Non tiene nessun mandante.** Non e' una dimenticanza: e' la condizione per
/// cui `recv` puo' dire `Disconnected`. Un modello conservato qui — anche solo
/// per poterne clonare altre — terrebbe il canale vivo per sempre, e il
/// drenaggio finale non finirebbe mai.
#[derive(Debug)]
pub(super) struct Coda {
    ricevitore: Receiver<Fatto>,
}

/// Apre la coda e consegna il fascio.
///
/// E' l'unico costruttore di entrambi, e li rende **insieme**: non esiste una
/// coda senza il suo fascio, ne' un fascio senza la sua coda.
pub(super) fn apri() -> (Coda, Fascio) {
    let (mandante, ricevitore) = sync_channel(CAPACITA);
    let bocchetta = |chi: Produttore| Bocchetta {
        chi,
        rimasti: chi.budget(),
        canale: mandante.clone(),
        primo_rifiuto: None,
    };
    let fascio = Fascio {
        lettore: bocchetta(Produttore::Lettore),
        orologio: bocchetta(Produttore::Orologio),
        annullatore: bocchetta(Produttore::Annullatore),
        sorvegliante: bocchetta(Produttore::Sorvegliante),
        raccoglitore: bocchetta(Produttore::Raccoglitore),
    };
    // Il modello **muore qui**. Ogni copia sopravvissuta impedirebbe al canale
    // di disconnettersi, e il drenaggio finale aspetterebbe un produttore che
    // non esiste.
    drop(mandante);
    (Coda { ricevitore }, fascio)
}

impl Coda {
    /// Il prossimo fatto, aspettando al piu' `limite`.
    ///
    /// Distingue nella stessa chiamata «non e' arrivato niente» (si riprova) da
    /// «non arrivera' piu' niente» (si smette): una domanda separata con
    /// `try_recv` consumerebbe in silenzio un fatto presente.
    pub(super) fn prossimo(&self, limite: std::time::Duration) -> Presa {
        match self.ricevitore.recv_timeout(limite) {
            Ok(fatto) => Presa::Fatto(fatto),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Presa::Scaduto,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Presa::Disconnessa,
        }
    }

    /// Prende cio' che e' gia' in coda, senza aspettare e senza chiudere.
    ///
    /// Non e' il drenaggio, ne' un `is_empty()` sul futuro: e' lecita solo
    /// quando ogni produttore in grado di accodare e' gia' stato fermato e
    /// aspettato, perche' il `join` rende visibile qui tutto cio' che ha
    /// accodato.
    ///
    /// Serve a sapere, prima di leggere l'evidenza, se il dominio e'
    /// quiescente: una quiescenza scoperta solo al drenaggio finale farebbe
    /// saltare l'evidenza, e l'esito direbbe «tempo scaduto» su un'esecuzione
    /// uccisa dall'OOM.
    pub(super) fn raccogli_i_fermi(&self) -> Vec<Fatto> {
        let mut fermi = Vec::new();
        svuota_senza_bloccare(&self.ricevitore, &mut fermi);
        fermi
    }

    /// Chiude e drena fino a `Disconnected`, senza fermarsi su un istante vuoto.
    ///
    /// # Che cosa il chiamante deve aver gia' fatto
    ///
    /// 1. rendere terminabili le sorgenti bloccanti dei produttori: un lettore
    ///    fermo in una `read` tiene viva la sua bocchetta;
    /// 2. aspettare i produttori e prenderne il resoconto;
    /// 3. lasciar cadere ogni bocchetta, anche quelle del consumatore.
    ///
    /// Se ne manca una l'attesa non finisce: il tetto la trasforma in un
    /// difetto detto, ed e' lungo abbastanza da non interrompere un drenaggio
    /// vero.
    ///
    /// Rende i fatti raccolti, e il motivo se il canale non si e' disconnesso.
    /// La produzione passa da [`Self::chiudi_e_drena_entro`] con
    /// [`Self::tetto_di_produzione`]; questa serve ai casi.
    #[cfg(any(test, feature = "internals"))]
    pub(super) fn chiudi_e_drena(self) -> (Vec<Fatto>, Option<String>) {
        self.chiudi_e_drena_entro(TETTO_DEL_DRENAGGIO)
    }

    /// Il tetto di produzione, per chi lo deve passare a [`Self::chiudi_e_drena_entro`].
    pub(super) const fn tetto_di_produzione() -> std::time::Duration {
        TETTO_DEL_DRENAGGIO
    }

    /// [`Self::chiudi_e_drena`] con il tetto in mano al chiamante.
    ///
    /// Permette di provare in fretta che un produttore rimasto vivo diventi un
    /// difetto detto. L'unico chiamante di produzione passa la costante: una
    /// prova varia quanto si aspetta, mai che cosa si conclude.
    pub(super) fn chiudi_e_drena_entro(
        self,
        tetto: std::time::Duration,
    ) -> (Vec<Fatto>, Option<String>) {
        // La scadenza si calcola una volta: rinnovarla a ogni fatto darebbe a
        // un produttore lento il potere di prolungare il drenaggio.
        // `checked_add` e non `+`: l'overflow di `Instant` va in panico, e
        // un'impossibilita' osservata va detta, non presunta.
        let Some(scadenza) = Scadenza::nuova(std::time::Instant::now(), tetto) else {
            let mut subito = Vec::new();
            svuota_senza_bloccare(&self.ricevitore, &mut subito);
            return (
                subito,
                Some(format!(
                    "il tetto di {} ms non e' rappresentabile come scadenza: si e' drenato \
                     soltanto cio' che c'e' gia'",
                    tetto.as_millis()
                )),
            );
        };
        let mut raccolti = Vec::new();
        loop {
            let rimasto = scadenza.rimasto(std::time::Instant::now());
            if rimasto.is_zero() {
                // Prima di rinunciare si prende cio' che c'e' gia': il tempo e'
                // finito per l'attesa, non per i fatti arrivati. `try_recv` non
                // blocca.
                svuota_senza_bloccare(&self.ricevitore, &mut raccolti);
                return (
                    raccolti,
                    Some(format!(
                        "il canale non si e' disconnesso entro {} ms: un produttore e' ancora vivo, e la sua sorgente non e' stata resa terminabile",
                        tetto.as_millis()
                    )),
                );
            }
            // `recv_timeout` rende `Disconnected` **solo** quando nessuno puo'
            // piu' scrivere: e' la condizione voluta, e non si confonde con una
            // pausa, che rende `Timeout` e fa riprovare.
            match self.ricevitore.recv_timeout(rimasto) {
                Ok(fatto) => raccolti.push(fatto),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return (raccolti, None),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apri, Esaurita, Presa, Produttore, CAPACITA};
    use crate::isolamento::macchina::{Fatto, UscitaOsservata};

    /// La somma dei budget, ricalcolata qui.
    ///
    /// I casi confrontano con questa e non con [`CAPACITA`]: un caso misurato
    /// sulla costante che giudica resterebbe verde anche abbassandola.
    fn somma_dei_budget() -> usize {
        Produttore::TUTTI.iter().map(|chi| chi.budget()).sum()
    }

    /// **La capacita' e' la somma dei budget**, e non una stima con margine.
    #[test]
    fn la_capacita_e_la_somma_dei_budget() {
        assert_eq!(CAPACITA, somma_dei_budget());
    }

    /// Ogni produttore ha un budget **finito e non nullo**.
    #[test]
    fn ogni_produttore_ha_un_budget_finito() {
        for chi in Produttore::TUTTI {
            assert!(chi.budget() > 0, "{} non puo' accodare niente", chi.nome());
            assert!(
                chi.budget() <= 4,
                "{} ha un budget troppo largo",
                chi.nome()
            );
        }
    }

    /// I nomi sono distinti: due produttori con lo stesso nome renderebbero
    /// illeggibile il rapporto.
    #[test]
    fn i_nomi_dei_produttori_sono_distinti() {
        let nomi: std::collections::BTreeSet<_> =
            Produttore::TUTTI.iter().map(|chi| chi.nome()).collect();
        assert_eq!(nomi.len(), Produttore::TUTTI.len());
    }

    /// I terminali hanno, tutti insieme, uno spazio loro.
    #[test]
    fn lo_spazio_dei_terminali_e_riservato() {
        let ripetibili: usize = Produttore::TUTTI
            .iter()
            .filter(|chi| !chi.terminale())
            .map(|chi| chi.budget())
            .sum();
        let terminali: usize = Produttore::TUTTI
            .iter()
            .filter(|chi| chi.terminale())
            .map(|chi| chi.budget())
            .sum();
        assert!(terminali > 0, "senza spazio riservato non c'e' riserva");
        assert_eq!(
            somma_dei_budget(),
            ripetibili + terminali,
            "ogni produttore e' o ripetibile o terminale, e nessuno e' entrambi"
        );
    }

    /// **Il fascio consegna cinque bocchette, e i loro gettoni sono la
    /// capacita'.**
    ///
    /// La garanzia strutturale sta nel tipo (campi fissi, nessun `Clone`, un
    /// solo costruttore); qui si misura che i campi spendano esattamente la
    /// capacita'.
    #[test]
    fn il_fascio_ha_cinque_bocchette_che_valgono_la_capacita() {
        let (coda, fascio) = apri();
        let mut bocchette = [
            fascio.lettore,
            fascio.orologio,
            fascio.annullatore,
            fascio.sorvegliante,
            fascio.raccoglitore,
        ];
        let gettoni: usize = bocchette.iter().map(super::Bocchetta::rimasti).sum();
        assert_eq!(
            gettoni,
            somma_dei_budget(),
            "cinque bocchette, tutti i gettoni"
        );

        let mut accodati = 0;
        for bocchetta in &mut bocchette {
            while bocchetta.manda(Fatto::FineDelCanale).is_ok() {
                accodati += 1;
            }
        }
        assert_eq!(
            accodati,
            somma_dei_budget(),
            "ogni gettone ha trovato il suo posto: se la capacita' fosse minore \
             della somma, qui ne mancherebbe almeno uno"
        );

        drop(bocchette);
        let (raccolti, difetto) = coda.chiudi_e_drena();
        assert_eq!(difetto, None, "il canale si disconnette");
        assert_eq!(
            raccolti.len(),
            somma_dei_budget(),
            "e il drenaggio li rende tutti"
        );
    }

    /// **Lasciato cadere il fascio, la coda si disconnette.**
    ///
    /// E' la prova che **nessun mandante sopravvive** fuori dal fascio: se ne
    /// restasse uno — il modello tenuto dalla coda, una copia dimenticata in una
    /// chiusura — il canale resterebbe vivo e `try_recv` direbbe «vuota», non
    /// «disconnessa». E un drenaggio che aspetta `Disconnected` non finirebbe
    /// mai.
    #[test]
    fn caduto_il_fascio_la_coda_si_disconnette() {
        let (coda, fascio) = apri();
        assert!(
            matches!(
                coda.prossimo(std::time::Duration::from_millis(1)),
                Presa::Scaduto
            ),
            "finche' il fascio vive, qualcuno puo' ancora scrivere"
        );
        drop(fascio);
        assert!(
            matches!(
                coda.prossimo(std::time::Duration::from_millis(1)),
                Presa::Disconnessa
            ),
            "caduto il fascio non resta nessun mandante"
        );
    }

    /// **Chiedere alla coda se e' finita non mangia un fatto.**
    ///
    /// E' il difetto che una domanda separata avrebbe: interrogare il canale per
    /// sapere se e' disconnesso significa provare a ricevere, e provare a
    /// ricevere consuma. Il fatto sparirebbe dentro una domanda che chiede
    /// un'altra cosa — e sparirebbe in silenzio.
    #[test]
    fn interrogare_la_coda_non_consuma_i_fatti() {
        let (coda, fascio) = apri();
        let mut sorvegliante = fascio.sorvegliante;
        sorvegliante
            .manda(Fatto::DominioQuiescente)
            .expect("dentro il budget");

        // Con un fatto dentro, la coda non e' ne' scaduta ne' disconnessa: lo
        // rende.
        assert!(matches!(
            coda.prossimo(std::time::Duration::from_millis(1)),
            Presa::Fatto(Fatto::DominioQuiescente)
        ));

        // E il fatto non e' stato consumato due volte: adesso non c'e' piu'
        // niente, ma qualcuno puo' ancora scrivere.
        assert!(matches!(
            coda.prossimo(std::time::Duration::from_millis(1)),
            Presa::Scaduto
        ));

        drop(sorvegliante);
        drop(fascio.lettore);
        drop(fascio.orologio);
        drop(fascio.annullatore);
        drop(fascio.raccoglitore);
        let (raccolti, difetto) = coda.chiudi_e_drena();
        assert_eq!(difetto, None);
        assert!(raccolti.is_empty(), "il fatto e' gia' stato reso una volta");
    }

    /// Finiti i gettoni, la bocchetta **rifiuta** invece di bloccare, e se lo
    /// ricorda.
    #[test]
    fn finito_il_budget_la_bocchetta_rifiuta_e_lo_ricorda() {
        let (_coda, fascio) = apri();
        let mut lettore = fascio.lettore;
        for _ in 0..Produttore::Lettore.budget() {
            assert!(lettore.manda(Fatto::FineDelCanale).is_ok());
        }
        assert_eq!(lettore.rimasti(), 0);
        assert_eq!(
            lettore.manda(Fatto::FineDelCanale),
            Err(Esaurita::Budget(Produttore::Lettore))
        );
        assert_eq!(
            lettore.resoconto(),
            Some(Esaurita::Budget(Produttore::Lettore)),
            "il rifiuto torna dal resoconto, non dalla coda che lo ha causato"
        );
    }

    /// **La prova che conta**: saturato il produttore ripetibile, tutti i
    /// terminali entrano lo stesso, subito.
    #[test]
    fn saturato_il_lettore_i_terminali_entrano_lo_stesso() {
        let (_coda, fascio) = apri();
        let mut lettore = fascio.lettore;
        for _ in 0..Produttore::Lettore.budget() {
            lettore
                .manda(Fatto::FineDelCanale)
                .expect("dentro il budget");
        }

        let mut orologio = fascio.orologio;
        assert!(orologio.manda(Fatto::TempoScaduto).is_ok());
        assert!(orologio.manda(Fatto::TempoScaduto).is_ok());

        let mut annullatore = fascio.annullatore;
        assert!(annullatore.manda(Fatto::CancellazioneRichiesta).is_ok());

        let mut sorvegliante = fascio.sorvegliante;
        assert!(sorvegliante.manda(Fatto::DominioQuiescente).is_ok());
        assert!(sorvegliante
            .manda(Fatto::OsservazioneImpossibile {
                chi: "quiescenza",
                motivo: "il dominio non risponde piu'".to_owned(),
            })
            .is_ok());

        let mut raccoglitore = fascio.raccoglitore;
        assert!(raccoglitore
            .manda(Fatto::UscitaDelWorker(UscitaOsservata::Codice(0)))
            .is_ok());
    }

    /// Il drenaggio arriva fino a `Disconnected`, e non si ferma su un istante
    /// vuoto.
    #[test]
    fn il_drenaggio_non_si_ferma_su_un_istante_vuoto() {
        let (coda, fascio) = apri();
        let mut lento = fascio.sorvegliante;
        drop(fascio.lettore);
        drop(fascio.orologio);
        drop(fascio.annullatore);
        drop(fascio.raccoglitore);
        let filo = std::thread::spawn(move || {
            // Il ritardo e' la cosa che si vuole provare: il consumatore
            // troverebbe la coda vuota se guardasse adesso.
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = lento.manda(Fatto::DominioQuiescente);
        });

        assert!(
            matches!(
                coda.prossimo(std::time::Duration::from_millis(1)),
                Presa::Scaduto
            ),
            "adesso la coda e' vuota, ed e' il punto"
        );
        let (raccolti, difetto) = coda.chiudi_e_drena();
        filo.join().expect("il produttore lento finisce");
        assert_eq!(difetto, None, "il canale si disconnette");
        assert_eq!(raccolti.len(), 1, "il fatto tardivo non si perde");
    }

    /// **Un produttore rimasto vivo diventa un difetto detto**, non un'attesa
    /// infinita.
    ///
    /// E' il caso che distingue un supervisore che riporta da uno appeso. Qui la
    /// bocchetta viene dimenticata di proposito — e' cio' che accadrebbe con un
    /// lettore fermo dentro una `read` che nessuno ha reso terminabile — e il
    /// drenaggio, invece di aspettare per sempre, dice chi manca.
    #[test]
    fn un_produttore_rimasto_vivo_diventa_un_difetto() {
        let (coda, fascio) = apri();
        let vivo = fascio.sorvegliante;
        drop(fascio.lettore);
        drop(fascio.orologio);
        drop(fascio.annullatore);
        drop(fascio.raccoglitore);

        let (raccolti, difetto) = coda.chiudi_e_drena_entro(std::time::Duration::from_millis(30));
        assert!(raccolti.is_empty());
        let detto = difetto.expect("un produttore vivo va detto, non aspettato");
        assert!(detto.contains("non si e' disconnesso"), "{detto}");
        drop(vivo);
    }

    /// **Il drenaggio vero ha un tetto, e conserva i fatti gia' accodati.**
    ///
    /// La scadenza assoluta la prova
    /// `la_regola_della_scadenza_non_dipende_dall_orologio`, senza misurare
    /// durate. Due canali laterali sostituiscono un `sleep`, che su una
    /// macchina carica perde la scommessa sullo scheduler: il primo dice «il
    /// fatto e' in coda» prima che si cominci a drenare, il secondo tiene viva
    /// la bocchetta finche' il drenaggio non e' scaduto.
    #[test]
    fn il_drenaggio_vero_ha_un_tetto_e_conserva_i_fatti() {
        let (coda, fascio) = apri();
        let mut insistente = fascio.sorvegliante;
        drop(fascio.lettore);
        drop(fascio.orologio);
        drop(fascio.annullatore);
        drop(fascio.raccoglitore);

        let (accodato, accodato_visto) = std::sync::mpsc::channel::<()>();
        let (libera, liberato) = std::sync::mpsc::channel::<()>();

        let filo = std::thread::spawn(move || {
            insistente
                .manda(Fatto::DominioQuiescente)
                .expect("la coda accetta il fatto");
            // Il segnale parte **dopo** l'invio: e' cio' che lo rende una
            // garanzia invece di una speranza.
            accodato.send(()).expect("il test ascolta");
            // E qui si resta, tenendo la bocchetta: il drenaggio deve trovare
            // un canale ancora connesso, e rinunciare per scadenza.
            let _ = liberato.recv();
            drop(insistente);
        });

        accodato_visto
            .recv()
            .expect("il produttore accoda prima di segnalare");

        let (raccolti, difetto) = coda.chiudi_e_drena_entro(std::time::Duration::from_millis(80));

        // Il produttore si libera **dopo** il drenaggio: liberarlo prima gli
        // farebbe lasciare la bocchetta, e il drenaggio finirebbe per
        // disconnessione invece che per scadenza — cioe' proverebbe un'altra
        // cosa.
        libera.send(()).expect("il produttore aspetta");
        filo.join().expect("il produttore insistente finisce");

        let detto = difetto.expect("il canale non si e' disconnesso, e va detto");
        assert!(detto.contains("non si e' disconnesso"), "{detto}");
        assert!(
            !raccolti.is_empty(),
            "i fatti gia' accodati restano nel rapporto insieme al difetto"
        );
    }

    /// **La regola della scadenza, senza orologio.**
    ///
    /// Il residuo cala e basta, e interrogarla non la rinnova. Gli istanti li
    /// sceglie il caso, quindi non si aspetta e non si misura la macchina; che
    /// il drenaggio vero finisca lo prova il caso col tempo vero.
    #[test]
    fn la_regola_della_scadenza_non_dipende_dall_orologio() {
        let inizio = std::time::Instant::now();
        let scadenza = super::Scadenza::nuova(inizio, std::time::Duration::from_millis(100))
            .expect("cento millisecondi sono rappresentabili");

        // Interrogata a istanti crescenti, il residuo cala.
        let a_zero = scadenza.rimasto(inizio);
        let a_meta = scadenza.rimasto(inizio + std::time::Duration::from_millis(40));
        let a_fine = scadenza.rimasto(inizio + std::time::Duration::from_millis(100));
        let oltre = scadenza.rimasto(inizio + std::time::Duration::from_millis(500));

        assert_eq!(a_zero, std::time::Duration::from_millis(100));
        assert_eq!(a_meta, std::time::Duration::from_millis(60));
        assert!(a_fine.is_zero());
        assert!(oltre.is_zero(), "passata resta passata, non torna negativa");

        // E **rinterrogarla non la sposta**: e' la differenza fra una scadenza
        // assoluta e una che riparte. Chiedere due volte allo stesso istante,
        // con in mezzo una domanda a un istante piu' avanti, deve dare lo stesso
        // residuo.
        assert_eq!(scadenza.rimasto(inizio), a_zero);
    }

    /// Un tetto non rappresentabile e' un **errore osservato**, non un panico.
    ///
    /// `Instant + Duration` va in panico sull'overflow, e un panico dentro la
    /// chiusura del supervisore sarebbe il posto peggiore in cui scoprirlo.
    #[test]
    fn un_tetto_non_rappresentabile_si_osserva() {
        let inizio = std::time::Instant::now();
        assert!(
            super::Scadenza::nuova(inizio, std::time::Duration::MAX).is_none(),
            "un tetto impossibile si dice, non si presume"
        );
        assert!(super::Scadenza::nuova(inizio, super::TETTO_DEL_DRENAGGIO).is_some());
    }

    /// **Allo scadere non si perde cio' che e' gia' in coda.**
    ///
    /// Il difetto si vede solo con piu' di un fatto pronto allo scadere. Il
    /// caso e' deterministico: i fatti si accodano prima, il lettore resta vivo
    /// e il tetto e' zero, quindi l'atteso e' tutti i fatti insieme al difetto
    /// di mancata disconnessione.
    #[test]
    fn allo_scadere_i_fatti_gia_accodati_tornano_tutti() {
        let (coda, fascio) = apri();
        let mut sorvegliante = fascio.sorvegliante;
        let mut orologio = fascio.orologio;
        sorvegliante
            .manda(Fatto::DominioQuiescente)
            .expect("dentro il budget");
        orologio
            .manda(Fatto::TempoScaduto)
            .expect("dentro il budget");
        orologio
            .manda(Fatto::TempoScaduto)
            .expect("dentro il budget");

        // Il lettore resta **vivo**: il canale non si disconnettera' mai, e il
        // drenaggio deve arrivare al tetto.
        let vivo = fascio.lettore;
        drop(fascio.annullatore);
        drop(fascio.raccoglitore);
        drop(sorvegliante);
        drop(orologio);

        let (raccolti, difetto) = coda.chiudi_e_drena_entro(std::time::Duration::ZERO);
        assert_eq!(
            raccolti.len(),
            3,
            "i tre fatti gia' accodati devono tornare tutti, e invece {raccolti:?}"
        );
        let detto = difetto.expect("il canale non si e' disconnesso, e va detto");
        assert!(detto.contains("non si e' disconnesso"), "{detto}");
        drop(vivo);
    }

    /// Senza ascoltatore, la bocchetta lo dice invece di bloccarsi.
    #[test]
    fn senza_ascoltatore_la_bocchetta_lo_dice() {
        let (coda, fascio) = apri();
        let mut bocchetta = fascio.annullatore;
        drop(coda);
        assert_eq!(
            bocchetta.manda(Fatto::CancellazioneRichiesta),
            Err(Esaurita::NessunAscoltatore(Produttore::Annullatore))
        );
    }
}
