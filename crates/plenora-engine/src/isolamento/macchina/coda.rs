//! La coda dei fatti: una sola, limitata, con lo spazio dei terminali riservato.
//!
//! # Perche' una sola coda
//!
//! Perche' due code rimettono in piedi due arbitrati che la coda unica esiste
//! per togliere.
//!
//! Il primo e' l'ordine fra vie diverse: la quiescenza puo' essere osservata
//! sulla via dei terminali mentre un `Esito` gia' arrivato aspetta ancora
//! sull'altra, e il consumatore concluderebbe che il worker e' morto senza dire
//! niente — mentre lo ha detto, e il messaggio e' li'.
//!
//! Il secondo e' la chiusura: chiudere e drenare **due** ingressi in modo
//! atomico e' un problema nuovo, che si risolve con un altro arbitrato. Una
//! coda sola non ha ordine fra vie, perche' non ha vie.
//!
//! # Perche' limitata, e perche' questo non blocca i terminali
//!
//! Limitata perche' il worker sceglie quanti messaggi mandare, e una coda che
//! cresce con quella scelta e' una via alla memoria che il chiamante non
//! governa.
//!
//! Ma una coda limitata usata indistintamente da tutti ricrea proprio il blocco
//! che i thread devono evitare: piena di `Progresso`, terrebbe fuori una
//! cancellazione. Due cose lo impediscono insieme, e nessuna delle due basta da
//! sola:
//!
//! 1. il **progresso si coalesce prima** di entrare: non e' un fatto per batch,
//!    e' un fatto solo che dice quanto si e' fatto. Cio' che il worker sceglie
//!    non decide quante volte si accoda;
//! 2. ogni produttore ha un **budget finito**, e la capacita' e' la **somma**
//!    dei budget. Non e' una stima con margine: e' un'identita', e la si
//!    verifica. Un produttore terminale non puo' quindi trovare il suo posto
//!    occupato, perche' quel posto e' suo per costruzione — nessun altro ha i
//!    gettoni per prenderglielo.
//!
//! # Perche' la chiusura non guarda se e' vuota
//!
//! Perche' «vuota adesso» non e' «non arrivera' altro»: fra la domanda e la
//! risposta un produttore vivo puo' accodare, e un drenaggio che si fermasse li'
//! perderebbe proprio i fatti dell'ultimo istante — che sono quelli che
//! contano. Si lasciano cadere **tutte** le bocchette e si drena finche' il
//! canale non dice `Disconnected`, che vuol dire «nessuno puo' piu' scrivere» ed
//! e' l'unica affermazione utile.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

/// Quanto si aspetta che il canale si disconnetta, prima di dire che qualcuno
/// e' rimasto vivo.
///
/// # Che cosa e' questo numero
///
/// Non la stima di quanto ci vuole: un drenaggio ordinario finisce appena i
/// thread muoiono, cioe' subito. E' il punto oltre il quale continuare ad
/// aspettare non e' piu' un'attesa ma un blocco, e trenta secondi lo mettono
/// molto oltre qualunque drenaggio vero — cosi' che scattare significhi sempre
/// «qualcuno e' rimasto vivo» e mai «e' stato lento».
///
/// # E' una scadenza **assoluta**
///
/// Si misura una volta, all'inizio, e non riparte quando arriva un fatto. E'
/// la differenza fra un tetto e nessun tetto: con una scadenza che si rinnova a
/// ogni consegna, un produttore che manda qualcosa ogni ventinove secondi
/// terrebbe aperto il drenaggio per sempre, e il tetto ci sarebbe solo sulla
/// carta.
///
/// # E' un limite governato
///
/// Nominato qui, motivato qui, e registrato in `errori-e-limiti.md` con regola,
/// perimetro e condizione di rientro.
const TETTO_DEL_DRENAGGIO: std::time::Duration = std::time::Duration::from_secs(30);

use super::Fatto;

/// Chi accoda, e quanto puo' accodare.
///
/// # Perche' il budget sta nel tipo
///
/// Perche' un numero scritto in una costante lontana si scollega da chi lo
/// spende. Qui il produttore **e'** il suo budget: chiedere una bocchetta
/// significa dichiarare chi si e', e da quella dichiarazione discende quanti
/// fatti si possono accodare. Non c'e' modo di chiederne di piu' senza
/// aggiungere un produttore, e aggiungerne uno cambia la capacita'.
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
/// # Perche' un tipo, e non una somma sul posto
///
/// Perche' cosi' la regola — **si calcola una volta e non riparte** — si puo'
/// provare senza aspettare. Con la somma scritta dentro il giro, l'unico modo
/// di verificarla sarebbe far passare il tempo davvero, e un caso che aspetta
/// misura la macchina su cui gira invece della regola.
///
/// Qui la regola e' una funzione di due istanti, e un caso la interroga con gli
/// istanti che vuole.
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
/// # Perche' rifiuta invece di bloccare
///
/// Perche' un produttore bloccato e' un produttore che non riporta piu' niente,
/// e non ha modo di dirlo. Finito il budget, `manda` rende un rifiuto: il
/// produttore lo vede, smette, e chi legge il rapporto sa che resta altro da
/// dire. Il blocco invece non si vede da nessuna parte.
///
/// Dentro il budget il rifiuto non arriva mai, perche' la capacita' e' la somma
/// dei budget: il posto c'e' per costruzione.
/// # Perche' non e' clonabile
///
/// Perche' una copia raddoppierebbe i gettoni senza raddoppiare la capacita', e
/// il conto su cui poggia lo spazio riservato smetterebbe di valere: due
/// lettori spenderebbero otto posti su undici, e un terminale troverebbe la
/// coda piena. Non c'e' `Clone`, e non c'e' modo di costruirne una fuori da
/// [`apri`].
#[derive(Debug)]
pub(super) struct Bocchetta {
    chi: Produttore,
    rimasti: usize,
    canale: SyncSender<Fatto>,
    /// Il primo rifiuto, se ce n'e' stato uno.
    ///
    /// # Perche' si conserva qui e non si accoda
    ///
    /// Perche' un rifiuto nasce quando la coda non accetta piu': accodarlo
    /// vorrebbe dire riportare che la coda e' piena **attraverso la coda
    /// piena**, che e' la definizione di un messaggio che non arriva. Resta
    /// invece nella bocchetta, e il produttore lo rende dal proprio
    /// `JoinHandle` — una via che non passa dalla coda e non puo' riempirsi.
    primo_rifiuto: Option<Esaurita>,
}

impl Bocchetta {
    /// Accoda un fatto, se restano gettoni.
    ///
    /// # Errors
    ///
    /// [`Esaurita`] quando il budget e' finito — o, per costruzione mai, quando
    /// la coda e' piena. I due casi si distinguono perche' mandano a guardare
    /// due cose diverse: il primo un produttore troppo loquace, il secondo un
    /// invariante rotto.
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
    /// # Perche' il primo, e perche' oggi non fa differenza
    ///
    /// Il primo e non l'ultimo, perche' i successivi ne sono la conseguenza.
    /// Va pero' detto per intero: da una bocchetta sola i rifiuti hanno tutti
    /// **lo stesso valore** — `Esaurita` porta il produttore, che non cambia — e
    /// una volta finito il budget ogni tentativo successivo rifiuta per quella
    /// ragione, senza mai arrivare alle altre due.
    ///
    /// Nessuna esecuzione raggiungibile distingue quindi il primo dall'ultimo:
    /// questa e' stabilita' dell'ordine per un domani in cui i rifiuti possano
    /// differire, non un controllo che qualche caso mette alla prova. Un caso
    /// che dicesse di provarlo sarebbe vacuo, e ce n'e' stato uno: e' stato tolto.
    const fn annota(&mut self, quale: Esaurita) -> Esaurita {
        if self.primo_rifiuto.is_none() {
            self.primo_rifiuto = Some(quale);
        }
        quale
    }

    /// Quanti gettoni restano.
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

/// Le cinque bocchette, consegnate **una volta sola**.
///
/// # Perche' un fascio con i nomi, e non una funzione che le distribuisce
///
/// Perche' una funzione si chiama due volte. Con `bocchetta(Produttore::Lettore)`
/// nessuno impedisce di chiederne due, e due lettori hanno otto gettoni su
/// undici: il conto su cui poggia lo spazio riservato ai terminali smette di
/// valere, e la somma dei budget non dimostra piu' la capacita' necessaria.
///
/// Qui le bocchette **esistono in cinque esemplari** perche' i campi sono
/// cinque, non clonabili, e nascono tutti insieme in [`apri`]. Non c'e' modo di
/// averne una sesta: chi la volesse dovrebbe aprire un'altra coda, che e' un
/// altro consumatore e un altro conto.
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
    /// # Perche' rende tre cose e non due
    ///
    /// Perche' «non e' arrivato niente» e «non arrivera' piu' niente» sono
    /// risposte diverse, e chi ascolta ne fa due cose diverse: sulla prima
    /// riprova, sulla seconda smette.
    ///
    /// Distinguerle **qui** e non con una seconda domanda non e' comodita': una
    /// domanda separata dovrebbe interrogare il canale, e interrogarlo con
    /// `try_recv` **mangia un fatto** se ce n'e' uno. Il fatto sparirebbe
    /// dentro una domanda che ne chiede un'altra, e sparirebbe in
    /// silenzio. `recv_timeout` invece distingue le tre risposte senza
    /// consumare niente che non sia il fatto reso.
    pub(super) fn prossimo(&self, limite: std::time::Duration) -> Presa {
        match self.ricevitore.recv_timeout(limite) {
            Ok(fatto) => Presa::Fatto(fatto),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Presa::Scaduto,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Presa::Disconnessa,
        }
    }

    /// Prende cio' che e' **gia'** in coda, senza aspettare.
    ///
    /// # Perche' non e' il drenaggio, e quando e' lecita
    ///
    /// Perche' il drenaggio serve a non perdere cio' che *sta per* arrivare, e
    /// per quello lascia cadere le bocchette e aspetta la disconnessione.
    /// Questa fa una cosa piu' piccola: raccoglie cio' che c'e' **adesso**, e
    /// non chiude niente.
    ///
    /// Non e' quindi il `is_empty()` vietato altrove, che sarebbe una domanda
    /// sul futuro travestita da domanda sul presente. E' lecita a una sola
    /// condizione: che ogni produttore in grado di accodare sia gia' stato
    /// **fermato e aspettato**. Il `join` di un thread stabilisce che tutto cio'
    /// che quel thread ha accodato e' gia' visibile qui — non c'e' un fatto in
    /// volo che una seconda lettura troverebbe.
    ///
    /// # Perche' serve
    ///
    /// Perche' fra la fine dell'ascolto e la lettura dell'evidenza c'e' una
    /// decisione che dipende dai fatti: **il dominio e' quiescente?** Se la
    /// quiescenza e' stata accodata dopo l'ultima interrogazione del giro, e la
    /// si scoprisse soltanto al drenaggio finale, l'evidenza verrebbe saltata su
    /// un dominio che nel frattempo si e' svuotato — e l'esito direbbe «tempo
    /// scaduto» su un'esecuzione uccisa dall'OOM.
    pub(super) fn raccogli_i_fermi(&self) -> Vec<Fatto> {
        let mut fermi = Vec::new();
        svuota_senza_bloccare(&self.ricevitore, &mut fermi);
        fermi
    }

    /// Chiude e drena.
    ///
    /// # Perche' non guarda se e' vuota
    ///
    /// Perche' «vuota adesso» non dice niente su cio' che sta per arrivare: fra
    /// la domanda e la risposta un produttore vivo puo' accodare, e fermarsi li'
    /// perderebbe i fatti dell'ultimo istante — quelli che raccontano com'e'
    /// finita. Qui si lascia cadere il modello, si aspetta che le bocchette
    /// cadano con i loro thread, e si legge finche' il canale non dice
    /// `Disconnected`: l'unica affermazione che significa «nessuno puo' piu'
    /// scrivere».
    ///
    /// # Che cosa il chiamante deve aver gia' fatto
    ///
    /// Tre cose, e nessuna delle tre e' facoltativa.
    ///
    /// 1. **Rendere terminabili le sorgenti bloccanti dei produttori.** Un
    ///    lettore fermo dentro una `read` non si sveglia perche' qualcuno
    ///    altrove lascia cadere un mandante: continuerebbe ad aspettare byte che
    ///    non arrivano, e la sua bocchetta resterebbe viva insieme a lui.
    /// 2. **Aspettare i produttori**, e prendersi il loro resoconto. Finche' un
    ///    thread vive, la sua bocchetta vive.
    /// 3. **Lasciar cadere ogni bocchetta**, comprese quelle che il consumatore
    ///    tiene per se'.
    ///
    /// Fatte le tre, questa funzione finisce sul `Disconnected`, che e' la
    /// condizione voluta.
    ///
    /// # Perche' c'e' comunque un tetto
    ///
    /// Perche' se una delle tre non e' stata fatta — un lettore fermo dentro una
    /// `read`, una bocchetta dimenticata in una chiusura — l'attesa non finisce
    /// mai, e un supervisore appeso e' il peggiore degli esiti: non conclude,
    /// non riporta, e non si distingue da uno che sta lavorando.
    ///
    /// Il tetto non e' quindi una scorciatoia sul drenaggio: e' lungo abbastanza
    /// da non poter interrompere un drenaggio vero, e serve a **trasformare
    /// un'attesa infinita in un difetto detto**. Chi lo legge sa che qualcuno e'
    /// rimasto vivo, ed e' l'unica informazione utile in quel momento.
    ///
    /// # Che cosa rende
    ///
    /// I fatti raccolti, e il motivo se il canale non si e' mai disconnesso.
    pub(super) fn chiudi_e_drena(self) -> (Vec<Fatto>, Option<String>) {
        self.chiudi_e_drena_entro(TETTO_DEL_DRENAGGIO)
    }

    /// Il tetto di produzione, per chi lo deve passare a [`Self::chiudi_e_drena_entro`].
    pub(super) const fn tetto_di_produzione() -> std::time::Duration {
        TETTO_DEL_DRENAGGIO
    }

    /// [`Self::chiudi_e_drena`] con il tetto in mano al chiamante.
    ///
    /// Esiste per **una** prova: che un produttore rimasto vivo diventi un
    /// difetto detto invece di un'attesa infinita. Con il tetto vero quel caso
    /// durerebbe mezzo minuto, e un caso che dura mezzo minuto non lo esegue
    /// nessuno — che e' il modo in cui un controllo smette di esistere.
    ///
    /// E' privata e ha un solo chiamante di produzione, che le passa la
    /// costante. Cio' che un chiamante di prova puo' variare e' **quanto** si
    /// aspetta, mai che cosa si conclude.
    pub(super) fn chiudi_e_drena_entro(
        self,
        tetto: std::time::Duration,
    ) -> (Vec<Fatto>, Option<String>) {
        // La scadenza si calcola **una volta**. Ricalcolarla dentro il giro, o
        // farla ripartire quando arriva un fatto, darebbe a un produttore lento
        // il potere di prolungare il drenaggio all'infinito.
        //
        // `checked_add` e non `+`: la somma di un `Instant` e una durata **puo'
        // andare in overflow**, e l'operatore in quel caso va in panico. Con
        // trenta secondi non accade su nessuna macchina reale, ma un panico
        // dentro la chiusura del supervisore sarebbe il posto peggiore in cui
        // scoprirlo — e un'impossibilita' osservata va detta, non presunta.
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
                // **Prima di rinunciare, si svuota cio' che c'e' gia'.** Il
                // tempo e' finito per l'*attesa*, non per i fatti arrivati
                // mentre si aspetta: tornare senza prenderli perderebbe
                // proprio quelli dell'ultimo istante, che sono quelli che
                // raccontano com'e' finita. Non e' un'attesa in piu':
                // `try_recv` non blocca, e si ferma appena la coda e' vuota.
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

    /// La somma dei budget, **ricalcolata qui**.
    ///
    /// I casi operativi confrontano con questa e non con [`CAPACITA`]: un caso
    /// che si misurasse sulla costante che deve giudicare si guarirebbe da solo
    /// — abbassare la costante abbasserebbe anche il valore atteso, e il caso
    /// resterebbe verde con la coda piu' piccola del necessario. E' un difetto
    /// che questa batteria ha avuto davvero, e che una mutazione ha scoperto.
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
    /// E' la proprieta' strutturale su cui poggia lo spazio riservato: le
    /// bocchette che esistono sono **quelle** e non altre, quindi la somma dei
    /// loro budget e' la somma dei budget. Una funzione che le distribuisse a
    /// richiesta non lo garantirebbe — due lettori avrebbero otto gettoni su
    /// undici — e nessun caso a runtime potrebbe accorgersene, perche' il
    /// difetto sarebbe nella possibilita', non in una chiamata.
    ///
    /// Qui la garanzia sta nel tipo: cinque campi, nessun `Clone`, un solo
    /// costruttore. Questo caso misura che i cinque campi spendano esattamente
    /// la capacita'.
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
    /// # Che cosa prova, e che cosa lascia all'altro caso
    ///
    /// Che il drenaggio **vero** si fermi quando il produttore resta vivo, e che
    /// nel fermarsi non butti via cio' che aveva gia' sentito: quello che si e'
    /// raccolto prima di rinunciare e' evidenza quanto la rinuncia.
    ///
    /// Che la scadenza sia **assoluta** — calcolata una volta, e non rinnovata
    /// da cio' che arriva — lo prova
    /// `la_regola_della_scadenza_non_dipende_dall_orologio`, che sceglie gli
    /// istanti e non aspetta. Qui non si finge di distinguere il rinnovo
    /// misurando durate: una misura del genere direbbe qualcosa sulla macchina.
    ///
    /// # Perche' due canali laterali e nessun `sleep`
    ///
    /// Perche' un `sleep` che deve garantire che un evento **sia gia'
    /// accaduto** e' una scommessa sullo scheduler, e su una macchina carica la
    /// si perde: il produttore non arriva a mandare, il drenaggio scade a mani
    /// vuote, e il caso diventa rosso senza che nulla sia rotto. E' successo.
    ///
    /// I due canali tolgono la scommessa. Il primo porta «il fatto e' in coda»,
    /// e il test non comincia a drenare prima di averlo ricevuto: cosi' «gia'
    /// accodato» e' un fatto osservato, non un'attesa sperata. Il secondo tiene
    /// vivo il produttore — cioe' gli fa trattenere la bocchetta — finche' il
    /// test non lo libera, che e' l'unico modo di garantire che il canale
    /// **non** si disconnetta mentre si drena.
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
    /// Una scadenza si calcola da un istante e non si sposta: interrogata a
    /// istanti successivi rende un residuo che **cala e basta**, e nessun fatto
    /// che arriva nel frattempo la rinnova. Qui gli istanti li sceglie il caso,
    /// quindi la regola si prova senza aspettare — e senza misurare la macchina
    /// su cui gira invece del codice.
    ///
    /// Il caso col tempo vero resta, ed e' un'altra cosa: quello prova che il
    /// drenaggio vero finisca davvero, con il margine che un orologio da parete
    /// richiede.
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
    /// # Perche' e' un caso a se', e perche' e' deterministico
    ///
    /// Perche' il tempo finisce per l'**attesa**, non per i fatti gia'
    /// arrivati: tornare senza prenderli perderebbe proprio quelli dell'ultimo
    /// istante, che sono quelli che raccontano com'e' finita. E' un difetto che
    /// si vede solo quando **piu' di un fatto** e' pronto allo scadere, quindi
    /// un caso con un fatto solo lo lascerebbe passare.
    ///
    /// Non c'e' niente da tarare: i fatti si accodano **prima**, il mandante
    /// resta vivo — cosi' il canale non si disconnette — e il tetto e' zero, che
    /// vuol dire «gia' scaduto» al primo giro. L'esito atteso e' quindi: tutti i
    /// fatti, **insieme** al difetto di mancata disconnessione.
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
