//! Il figlio appena avviato, finche' non e' stato consegnato o raccolto.
//!
//! # Perche' una guardia, e perche' con piu' di un'uscita
//!
//! Perche' fra lo `spawn` e la consegna al chiamante ci sono cammini che
//! falliscono, e su quei cammini **un processo esiste gia'**. Chiudere i
//! descrittori non basta: un figlio non terminato resta orfano quando il
//! supervisore esce, e uno terminato ma non raccolto resta zombie finche' il
//! supervisore vive.
//!
//! Le uscite volute si chiamano per cio' che fanno, non per un numero d'ordine:
//! un'uscita nuova rinumererebbe le altre, e la prosa direbbe il falso mentre il
//! codice regge.
//!
//! - **la consegna** — [`FiglioVivo::consegna`] — passa il processo a chi lo ha
//!   chiesto;
//! - **l'attesa** — [`FiglioVivo::attendi_la_fine`] — lo lascia finire da se',
//!   senza segnalargli niente, e restituisce la guardia a chi non fa in tempo;
//! - **la chiusura** — [`FiglioVivo::termina_e_raccogli`] — lo termina e dice
//!   com'e' andata, e **restituisce la guardia** quando non ci riesce, perche'
//!   un processo che non si lascia raccogliere esiste ancora e qualcuno deve
//!   restarne responsabile;
//! - **l'arresto** — [`FiglioVivo::arrenditi`] — ferma tutto **dicendo
//!   perche'**: si usa quando sopra non c'e' nessuno che possa riprovare.
//!
//! Le prime tre non sono alternative fra loro nello stesso momento: chi ha
//! fretta di chiudere passa dalla chiusura, chi ha motivo di credere che il
//! figlio stia gia' finendo passa prima dall'attesa — e allora la chiusura
//! diventa il secondo tempo, non il primo.
//!
//! # Perche' non esiste una porta che «rinuncia e prosegue»
//!
//! Perche' sarebbe la porta che tutti userebbero. Registrare il pid e un difetto
//! si legge come diligenza, ma non e' proprieta' e non e' raccolta: il processo
//! resta li', nessuno lo aspetta, e il supervisore prosegue con una riga di
//! rapporto al posto di un responsabile. La guardia che torna da
//! `termina_e_raccogli` deve quindi **risalire** a chi puo' ancora riprovare —
//! ed e' cio' che il contorno della conduzione fa, portandola fuori — oppure
//! fermare tutto.
//!
//! Ma in Rust c'e' sempre un'altra uscita, e non e' facoltativa: **il drop
//! implicito**. Un `?` che esce prima, un `return` aggiunto un domani, la
//! semplice fine dello scope — ognuno rilascia il valore senza passare da
//! nessuna delle porte. Senza un `Drop`, `Child` verrebbe lasciato andare e il
//! figlio resterebbe: non terminato, non raccolto, e senza che niente lo dica.
//!
//! Per questo il processo sta in un `Option`, che le uscite **consumano**, e
//! [`Drop`] e' una **sentinella fail-stop**: se trova ancora qualcosa, quel
//! qualcosa e' sfuggito.
//!
//! # Perche' la sentinella abortisce invece di rimediare
//!
//! Perche' non e' la pulizia ordinaria, e fingere che lo sia sarebbe la cosa
//! peggiore. `Drop` non puo' rendere un errore: qualunque cosa faccia — anche
//! terminare e raccogliere correttamente — lo farebbe **in silenzio**, e il
//! supervisore proseguirebbe credendo che il cammino sfuggito non esista.
//! Quella e' la condizione in cui un difetto resta per sempre, perche' nessuno
//! lo vede mai.
//!
//! La sentinella tenta quindi la terminazione **best-effort** — meglio un
//! figlio ucciso che uno orfano — dice su `stderr` che cosa e' successo, e
//! ferma il processo. Non e' un rimedio: e' un rifiuto di proseguire.
//!
//! # Perche' l'attesa e' limitata
//!
//! Perche' `wait` senza limite lega il supervisore al figlio: un processo in
//! stato ininterrompibile non risponde a `SIGKILL` finche' non esce da li', e
//! il supervisore resterebbe fermo a guardarlo. Un figlio che non si lascia
//! raccogliere entro il tempo dato diventa un **difetto riportato**, non un
//! blocco: chi decide che farne e' la macchina dei timeout, che ha il quadro
//! che questa funzione non ha.

use std::time::Duration;

/// Quanto si aspetta che un figlio da chiudere si lasci raccogliere.
///
/// # Perche' questo ordine di grandezza
///
/// Perche' fra `SIGKILL` e l'uscita c'e' solo il tempo che il kernel impiega a
/// smontare il processo, che sono microsecondi — a meno che il figlio non sia
/// fermo in uno stato ininterrompibile, e li' non c'e' attesa ragionevole che
/// basti. Due secondi non sono la stima di quanto ci vuole: sono il punto oltre
/// il quale continuare ad aspettare non e' piu' un'attesa ma un blocco, e il
/// supervisore ha altro da chiudere.
pub(super) const LIMITE_DI_RACCOLTA: Duration = Duration::from_secs(2);

/// Ogni quanto si riguarda, mentre si aspetta.
///
/// Non c'e' modo di farsi svegliare: `try_wait` non blocca e `wait` non ha
/// scadenza, quindi o si guarda a intervalli o si perde il limite. Un passo
/// corto rende l'attesa reattiva, e in un percorso che di norma finisce al primo
/// giro il costo e' nessuno.
pub(super) const PASSO_DI_RACCOLTA: Duration = Duration::from_millis(2);

/// Come un processo e' finito, nella forma che il sistema operativo riporta.
///
/// # Perche' un tipo qui e non `ExitStatus`
///
/// Perche' `ExitStatus` e' opaco e platform-specific, e i suoi accessori
/// rendono due `Option` che ammettono combinazioni che non esistono. Questo tipo
/// le chiude: un processo o esce da se' con un codice, o viene fermato da un
/// segnale, oppure — e capita — il sistema riporta uno stato da cui non si
/// ricava nessuno dei due, e allora non si inventa niente.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Uscita {
    /// Uscito da se'.
    Codice(i32),
    /// Fermato da un segnale.
    Segnale(i32),
    /// Il sistema non riporta ne' l'uno ne' l'altro.
    ///
    /// Non e' un modo di uscire: e' una lettura che non dice niente, e chi la
    /// riceve deve trattarla come un'osservazione mancata invece che come un
    /// codice zero.
    NonRappresentabile,
}

impl Uscita {
    /// L'uscita, da cio' che il sistema riporta.
    fn da_stato(stato: std::process::ExitStatus) -> Self {
        if let Some(codice) = stato.code() {
            return Self::Codice(codice);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            if let Some(segnale) = stato.signal() {
                return Self::Segnale(segnale);
            }
        }
        Self::NonRappresentabile
    }
}

/// Le tre operazioni che la guardia fa sul processo.
///
/// # Perche' un trait e non `Child` direttamente
///
/// Perche' i cammini che contano sono quelli che non si producono a comando.
/// «Il figlio si termina ma non si lascia raccogliere» e' esattamente il caso
/// per cui il limite esiste, e su un processo vero servirebbe un programma che
/// ignora `SIGKILL` — cosa che non si scrive, e che se si scrivesse resterebbe
/// in giro dopo il caso.
///
/// Con il trait quel cammino si compone: `prova_a_raccogliere` che dice sempre
/// «ancora vivo», `termina` che riesce. E' la stessa ragione dell'orologio
/// iniettabile: si prova la **regola**, non la macchina.
pub(super) trait ProcessoFiglio {
    /// Il pid, per l'evidenza.
    fn pid(&self) -> u32;
    /// L'uscita, se il processo e' finito; `None` se e' ancora vivo.
    ///
    /// # Perche' rende l'uscita e non un «si'/no»
    ///
    /// Perche' un booleano dice che il processo e' finito e butta via **come**.
    /// Successo, codice diverso da zero e morte per segnale diventano la stessa
    /// cosa, e chi classifica non ha piu' modo di distinguerli: la riga 2 e la
    /// riga 4 della matrice collassano.
    ///
    /// `None` in `Uscita` significa «non rappresentabile»: un `ExitStatus` che
    /// non porta ne' codice ne' segnale non e' un modo di uscire, e' una lettura
    /// che non dice niente — e va detta come tale, non travestita da zero.
    fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>>;
    /// Gli manda il segnale che lo termina.
    fn termina(&mut self) -> std::io::Result<()>;
}

impl ProcessoFiglio for std::process::Child {
    fn pid(&self) -> u32 {
        self.id()
    }

    fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
        self.try_wait().map(|stato| stato.map(Uscita::da_stato))
    }

    fn termina(&mut self) -> std::io::Result<()> {
        self.kill()
    }
}

/// L'orologio del supervisore.
///
/// # Perche' iniettabile
///
/// Perche' i casi deterministici non devono dormire. Un caso che aspettasse
/// davvero misurerebbe la macchina su cui gira — e su una macchina carica
/// misurerebbe un'altra cosa — mentre cio' che va provato e' la **regola**:
/// quante volte si guarda, e che cosa si decide allo scadere.
///
/// Monotono per costruzione: `Instant` non torna indietro nemmeno se l'ora di
/// sistema cambia, e un'attesa misurata sull'ora di sistema potrebbe diventare
/// negativa proprio mentre qualcuno sincronizza l'orologio.
pub(super) trait Orologio {
    /// Quanto tempo e' passato dall'inizio dell'attesa.
    fn trascorso(&self) -> Duration;
    /// Lascia passare un po' di tempo prima di riprovare.
    fn attendi_un_poco(&self);
}

/// L'orologio vero.
pub(super) struct OrologioDiSistema {
    inizio: std::time::Instant,
    passo: Duration,
}

impl OrologioDiSistema {
    /// Un orologio che riprova ogni `passo`.
    pub(super) fn nuovo(passo: Duration) -> Self {
        Self {
            inizio: std::time::Instant::now(),
            passo,
        }
    }
}

impl Orologio for OrologioDiSistema {
    fn trascorso(&self) -> Duration {
        self.inizio.elapsed()
    }

    fn attendi_un_poco(&self) {
        std::thread::sleep(self.passo);
    }
}

/// Come e' finita la chiusura di un figlio.
///
/// # Perche' la guardia torna indietro quando non si raccoglie
///
/// Perche' un processo che non si e' lasciato raccogliere **esiste ancora**, e
/// qualcuno deve restarne responsabile. Rendere solo un messaggio d'errore lo
/// lascerebbe senza padrone: la guardia sarebbe gia' stata consumata, la
/// sentinella non potrebbe intervenire, e il processo resterebbe li' con
/// nessuno che possa nemmeno riprovare.
///
/// Restituendola, la scelta torna a chi ha il quadro: riprovare, farla risalire
/// ancora a chi sta sopra, oppure — quando sopra non c'e' nessuno —
/// [`FiglioVivo::arrenditi`], che ferma tutto dicendo perche'.
#[derive(Debug)]
pub(super) enum Chiusura<P: ProcessoFiglio> {
    /// Raccolto. L'uscita, e i difetti incontrati per strada.
    ///
    /// L'uscita e' `None` soltanto su una guardia gia' vuota, che per
    /// costruzione non arriva fin qui.
    Raccolto {
        uscita: Option<Uscita>,
        difetti: Vec<String>,
    },
    /// Non raccolto. La guardia torna, e i difetti dicono perche'.
    NonRaccolto {
        guardia: FiglioVivo<P>,
        difetti: Vec<String>,
    },
}

/// Un figlio avviato, che deve uscire di scena per una delle porte volute.
#[derive(Debug)]
pub(super) struct FiglioVivo<P: ProcessoFiglio> {
    /// `None` dopo che una delle uscite volute lo ha consumato.
    ///
    /// E' cio' che rende la sentinella capace di distinguere «gia' sistemato»
    /// da «sfuggito»: senza l'`Option` il `Drop` non avrebbe modo di saperlo, e
    /// dovrebbe o non fare niente — lasciando passare i cammini sfuggiti — o
    /// fare qualcosa sempre, anche dopo una consegna riuscita.
    processo: Option<P>,
}

impl<P: ProcessoFiglio> FiglioVivo<P> {
    /// Prende in custodia un processo appena avviato.
    pub(super) const fn nuovo(processo: P) -> Self {
        Self {
            processo: Some(processo),
        }
    }

    /// Il pid, finche' la guardia lo tiene.
    pub(super) fn pid(&self) -> Option<u32> {
        self.processo.as_ref().map(ProcessoFiglio::pid)
    }

    /// La porta della **consegna**: il processo passa a chi lo ha chiesto.
    ///
    /// Da qui in poi la responsabilita' di terminarlo e raccoglierlo e' del
    /// chiamante, e questa guardia non ha piu' niente da custodire.
    ///
    /// Rende `None` solo se la guardia e' gia' vuota, che per costruzione non
    /// puo' accadere — ogni porta prende `self` per valore. Il tipo lo
    /// dice comunque invece di affermarlo con una primitiva di panico, che in
    /// questo progetto non si usa.
    pub(super) fn consegna(mut self) -> Option<P> {
        self.processo.take()
    }

    /// La porta dell'**attesa**: si lascia finire da se', senza segnalargli
    /// niente.
    ///
    /// # Perche' esiste, accanto a `termina_e_raccogli`
    ///
    /// Perche' su un cammino riuscito il figlio sta gia' uscendo, e fra «ha
    /// chiuso il canale» e «il kernel ha registrato la sua uscita» c'e' una
    /// finestra che non e' un guasto: e' il tempo che ci mette a finire.
    /// `termina_e_raccogli` in quella finestra **manda un segnale**, perche' il
    /// suo primo sguardo lo trova ancora vivo — e un worker ucciso per aver
    /// tardato un millisecondo e' un worker ucciso senza motivo.
    ///
    /// Qui invece non si segnala mai: si guarda, si aspetta un poco, si
    /// riguarda, finche' il tempo dato non e' finito. Chi non fa in tempo torna
    /// **dentro la guardia**, e il chiamante decide se concedergli altro tempo o
    /// passare alla porta della chiusura.
    ///
    /// # Che cosa rende
    ///
    /// [`Chiusura::Raccolto`] con l'uscita, se ha finito; altrimenti
    /// [`Chiusura::NonRaccolto`] con la guardia intatta. I difetti sono quelli
    /// dell'interrogazione: qui non c'e' nessuna terminazione che possa
    /// fallire.
    #[cfg(any(test, feature = "internals"))]
    pub(super) fn attendi_la_fine(
        mut self,
        limite: Duration,
        orologio: &impl Orologio,
    ) -> Chiusura<P> {
        let mut difetti = Vec::new();
        let Some(processo) = self.processo.as_mut() else {
            return Chiusura::Raccolto {
                uscita: None,
                difetti,
            };
        };

        loop {
            match processo.prova_a_raccogliere() {
                Ok(Some(uscita)) => {
                    self.processo = None;
                    return Chiusura::Raccolto {
                        uscita: Some(uscita),
                        difetti,
                    };
                }
                Ok(None) => {}
                Err(errore) => {
                    // Non si sa in che stato sia. Non lo si abbandona e non lo
                    // si uccide qui: torna al chiamante, che ha la porta della
                    // chiusura per farlo.
                    difetti.push(format!("non si riesce a interrogare il figlio: {errore}"));
                    return Chiusura::NonRaccolto {
                        guardia: self,
                        difetti,
                    };
                }
            }
            if orologio.trascorso() >= limite {
                return Chiusura::NonRaccolto {
                    guardia: self,
                    difetti,
                };
            }
            orologio.attendi_un_poco();
        }
    }

    /// La porta della **chiusura**: si termina il figlio, e si dice se qualcosa
    /// e' andato storto.
    ///
    /// # L'ordine, e la corsa che sta in mezzo
    ///
    /// Prima si prova a raccogliere: se il figlio e' gia' uscito da solo non
    /// c'e' niente da terminare.
    ///
    /// Poi la terminazione, se e' ancora vivo. Fra le due c'e' una corsa — il
    /// figlio puo' uscire proprio li' — e allora la terminazione puo'
    /// rispondere che quel processo non e' piu' terminabile. Non e' un difetto:
    /// e' il caso in cui il lavoro e' gia' fatto.
    ///
    /// **Il pid non viene riciclato in quella finestra**, e la ragione e' che
    /// un figlio uscito e non ancora raccolto resta *zombie*: il suo pid
    /// appartiene ancora a lui, e nessun altro processo puo' riceverlo finche'
    /// il padre non lo raccoglie. La corsa e' quindi innocua non perche' sia
    /// stretta, ma perche' il kernel tiene il posto.
    ///
    /// Per questo dopo la terminazione — riuscita o rifiutata — **la raccolta
    /// segue comunque**: uscire li' lascerebbe proprio lo zombie che questa
    /// funzione esiste per chiudere.
    ///
    /// Infine la raccolta, **limitata nel tempo**: si guarda, si aspetta un
    /// poco, si riguarda, finche' il tempo dato non e' finito.
    ///
    /// # Che cosa rende
    ///
    /// **L'uscita**, se il figlio e' stato raccolto, e il motivo se qualcosa e'
    /// andato storto. Le due cose sono indipendenti: un figlio puo' essere
    /// raccolto **e** aver dato problemi nel terminarlo, e un figlio che non si
    /// raccoglie non ha un'uscita da mostrare.
    ///
    /// Il difetto e' di **pulizia**, e il chiamante lo conserva insieme a quello
    /// che lo ha portato qui — non al suo posto.
    pub(super) fn termina_e_raccogli(
        mut self,
        limite: Duration,
        orologio: &impl Orologio,
    ) -> Chiusura<P> {
        let mut difetti = Vec::new();

        // Il processo resta **dentro la guardia** per tutta la funzione. Le
        // operazioni passano da `as_mut`, e `take` avviene solo quando la
        // raccolta e' riuscita: cosi' ogni ritorno anticipato lascia la guardia
        // piena, e la guardia piena e' cio' che il chiamante deve sistemare.
        //
        // Estrarlo subito significherebbe che su ogni cammino di fallimento il
        // figlio sparisce: la sentinella non puo' intervenire su una guardia
        // gia' vuota, e nessuno resta responsabile di un processo che puo'
        // essere ancora vivo.
        let Some(processo) = self.processo.as_mut() else {
            return Chiusura::Raccolto {
                uscita: None,
                difetti,
            };
        };

        // 1. Gia' uscito?
        match processo.prova_a_raccogliere() {
            Ok(Some(uscita)) => {
                self.processo = None;
                return Chiusura::Raccolto {
                    uscita: Some(uscita),
                    difetti,
                };
            }
            Ok(None) => {}
            Err(errore) => {
                // Non si sa in che stato sia, e non lo si abbandona: si prova a
                // terminarlo lo stesso. Tornare qui lascerebbe un processo
                // ignoto e nessuno che ci abbia provato.
                difetti.push(format!("non si riesce a interrogare il figlio: {errore}"));
            }
        }

        // 2. Terminazione. Un rifiuto che dice «non c'e' piu'» e' la corsa, non
        //    un guasto — e in ogni caso si passa al punto 3, perche' un figlio
        //    uscito e non raccolto e' esattamente uno zombie.
        //
        //    **Il pid non viene riciclato in quella finestra**: un figlio uscito
        //    e non ancora raccolto resta *zombie*, e il suo pid appartiene
        //    ancora a lui.
        match processo.termina() {
            Ok(()) => (),
            Err(errore) if errore.kind() == std::io::ErrorKind::InvalidInput => (),
            Err(errore) => difetti.push(format!("non si riesce a terminare il figlio: {errore}")),
        }

        // 3. Raccolta, con un tetto. **I difetti si sommano**: una terminazione
        //    rifiutata e una raccolta che non arriva sono due cose, e la seconda
        //    non spiega la prima.
        loop {
            match processo.prova_a_raccogliere() {
                Ok(Some(uscita)) => {
                    self.processo = None;
                    return Chiusura::Raccolto {
                        uscita: Some(uscita),
                        difetti,
                    };
                }
                Ok(None) => {}
                Err(errore) => {
                    difetti.push(format!("non si riesce a raccogliere il figlio: {errore}"));
                    return Chiusura::NonRaccolto {
                        guardia: self,
                        difetti,
                    };
                }
            }
            if orologio.trascorso() >= limite {
                difetti.push(format!(
                    "il figlio non si lascia raccogliere entro {} ms: resta da chiudere",
                    limite.as_millis()
                ));
                return Chiusura::NonRaccolto {
                    guardia: self,
                    difetti,
                };
            }
            orologio.attendi_un_poco();
        }
    }

    /// La porta dell'**arresto**: ferma tutto, dicendo perche'.
    ///
    /// # Quando si usa
    ///
    /// Quando il figlio non si e' lasciato raccogliere e sopra non c'e' nessuno
    /// che possa riprovare. Non e' il caso ordinario: dove un responsabile
    /// esiste, la guardia gli risale e la scelta resta sua.
    ///
    /// # Perche' fermare e non proseguire riportando
    ///
    /// Perche' proseguire vorrebbe dire lasciare un processo che nessuno
    /// aspetta, in un processo che intanto dichiara di aver finito. Una riga di
    /// rapporto non lo raccoglie: descrive la perdita, e poi la lascia
    /// accadere. Fra le due cose sbagliate — fermarsi troppo presto e perdere un
    /// processo in silenzio — la seconda e' quella che non si scopre mai.
    ///
    /// # Perche' `abort` e non un panico
    ///
    /// Per la stessa ragione della sentinella: un panico si puo' catturare, e un
    /// difetto catturato torna a essere invisibile.
    pub(super) fn arrenditi(mut self, contesto: &str) -> ! {
        // **Prima si prova a ucciderlo, poi ci si ferma.** `abort` ferma noi,
        // non lui: senza questo tentativo il figlio passa al reaper del sistema e
        // sopravvive al processo che dichiara di fermarsi per non lasciarlo
        // vivo.
        //
        // Cio' che si puo' dire e' che il segnale e' partito, non che il figlio
        // sia morto: `termina` manda, non osserva. La riga riporta quindi il
        // tentativo, non un esito che nessuno ha visto.
        let (numero, colpo) = self.processo.take().map_or_else(
            || {
                (
                    "sconosciuto".to_owned(),
                    "nessun processo da terminare".to_owned(),
                )
            },
            |mut processo| {
                let pid = processo.pid().to_string();
                (pid, ultimo_tentativo_di_terminazione(&mut processo))
            },
        );
        eprintln!(
            "plenora: il figlio {numero} non si e' lasciato raccogliere ({contesto}), e non c'e' \
             nessuno che possa riprovare. Ultimo tentativo di terminazione: {colpo}. Il processo \
             si ferma qui."
        );
        std::process::abort()
    }

    /// Smonta la guardia senza chiudere niente. **Solo nei casi.**
    ///
    /// # Perche' esiste, e perche' non e' una porta
    ///
    /// Perche' nei casi il processo non e' un processo: e' una finzione che non
    /// tiene nessuna risorsa, e farle attraversare una vera chiusura
    /// misurerebbe la finzione invece della regola.
    ///
    /// La garanzia non e' che nessuno possa accendere `cfg(test)` — chi controlla
    /// la build puo' selezionare i `cfg` che vuole. E' che questa via **non
    /// appartiene alla build ordinaria di Cargo** e non e' raggiungibile
    /// dall'API: nessun percorso di compilazione previsto la include, e nessun
    /// chiamante puo' nominarla.
    #[cfg(test)]
    pub(super) fn smonta(mut self) -> Option<u32> {
        self.processo.take().map(|processo| processo.pid())
    }
}

impl<P: ProcessoFiglio> Drop for FiglioVivo<P> {
    /// La sentinella: se qui c'e' ancora un processo, e' sfuggito.
    ///
    /// Non e' la pulizia ordinaria — quella passa dalle porte che raccolgono,
    /// l'attesa e la chiusura, e che possono dire com'e' andata proprio perche'
    /// raccolgono. Questa e' l'ultima riga: tenta
    /// la terminazione perche' un figlio a cui e' arrivato un segnale e' meglio
    /// di uno a cui non e' arrivato niente, **riporta che cosa si e' potuto
    /// fare**, e ferma il processo.
    ///
    /// La risposta entra nel messaggio e non si scarta: «segnale inviato» e
    /// «segnale non inviato» sono due situazioni diverse per chi legge il log, e
    /// scriverne una sola le fa sembrare la stessa. Nessuna delle due dice che il
    /// figlio sia morto — qui nessuno lo raccoglie, e quindi nessuno lo sa.
    ///
    /// # Perche' `abort` e non un rimedio silenzioso
    ///
    /// Perche' un rimedio silenzioso **funzionerebbe**, e sarebbe il problema:
    /// il cammino sfuggito continuerebbe a esistere, il supervisore
    /// proseguirebbe come se niente fosse, e nessuno saprebbe mai che c'e' un
    /// `?` che salta le porte. Un difetto che si auto-ripara e' un difetto che
    /// non si corregge.
    ///
    /// # Perche' una riga su `stderr`
    ///
    /// Perche' un `abort` muto e' indiagnosticabile: chi lo trova nei log vede
    /// un processo sparito e nient'altro. Il contratto «stderr vuoto» vale per
    /// il funzionamento ordinario, e questo non lo e' — e' il momento in cui il
    /// programma dichiara di non potersi fidare di se stesso.
    fn drop(&mut self) {
        let Some(mut processo) = self.processo.take() else {
            return;
        };
        let pid = processo.pid();
        let colpo = ultimo_tentativo_di_terminazione(&mut processo);
        eprintln!(
            "plenora: il figlio {pid} e' sfuggito alla guardia — non raccolto. Ultimo tentativo \
             di terminazione: {colpo}. Il supervisore si ferma qui invece di proseguire con un \
             processo di cui non sa niente."
        );
        std::process::abort();
    }
}

/// L'ultimo tentativo di terminazione, e **che cosa si e' potuto fare**.
///
/// # Perche' provare, prima di fermarsi
///
/// Perche' `abort` ferma **noi**, non il figlio. Un processo lasciato cadere non
/// muore: passa al reaper del sistema e sopravvive al supervisore. Fermarsi senza
/// aver provato a ucciderlo sarebbe la meta' del lavoro, e la meta' che non si
/// vede.
///
/// # Perche' nessuna delle tre risposte dice «terminato»
///
/// Perche' nessuna delle tre lo sa. [`ProcessoFiglio::termina`] **manda** la
/// terminazione; non osserva l'uscita — nel cammino ordinario e' seguita da
/// `prova_a_raccogliere` proprio per questo. Un `SIGKILL` accettato dice che il
/// segnale e' partito, non che il processo sia finito: un processo in attesa
/// ininterrompibile lo riceve e resta li' finche' la chiamata di sistema non
/// ritorna.
///
/// Scrivere «terminato» sarebbe quindi un'affermazione piu' forte di quella che
/// il tipo autorizza, e in un log e' peggio del silenzio: chi legge smetterebbe
/// di cercare un processo che c'e' ancora. Le tre risposte dicono cio' che si e'
/// fatto, e dichiarano ogni volta che **l'uscita non e' stata osservata**.
///
/// Nemmeno `InvalidInput` autorizza «gia' uscito»: il contratto del tratto dice
/// che quel genere significa «non piu' terminabile», che e' compatibile con un
/// figlio gia' finito ma non lo prova.
///
/// # Perche' non si raccoglie
///
/// Perche' dopo non c'e' piu' nessuno che possa aspettare. Un figlio non raccolto
/// passa al reaper del sistema, che se ne occupa; un figlio a cui **non** e'
/// arrivato niente e' l'altro esito, ed e' il solo che valga la pena leggere in
/// un log — per questo la risposta torna indietro invece di essere scartata con
/// un `let _`.
fn ultimo_tentativo_di_terminazione<P: ProcessoFiglio>(processo: &mut P) -> String {
    match processo.termina() {
        Ok(()) => "segnale di terminazione inviato; uscita non osservata".to_owned(),
        Err(errore) if errore.kind() == std::io::ErrorKind::InvalidInput => {
            "processo non piu' terminabile; uscita non osservata".to_owned()
        }
        Err(errore) => format!("segnale non inviato ({errore}); puo' restare vivo"),
    }
}

#[cfg(test)]
mod tests {
    /// L'uscita e i difetti di una chiusura **riuscita**.
    ///
    /// I casi che la usano dichiarano cosi' di aspettarsi che il figlio venga
    /// raccolto: se la guardia tornasse indietro, il caso fallirebbe con il
    /// motivo giusto invece di confrontare valori che non ci sono.
    fn raccolto<P: super::ProcessoFiglio>(
        chiusura: super::Chiusura<P>,
    ) -> (Option<super::Uscita>, Option<String>) {
        match chiusura {
            super::Chiusura::Raccolto { uscita, difetti } => {
                (uscita, (!difetti.is_empty()).then(|| difetti.join("; ")))
            }
            super::Chiusura::NonRaccolto { guardia, difetti } => {
                guardia.smonta();
                panic!("il figlio doveva essere raccolto: {difetti:?}")
            }
        }
    }

    /// I difetti di una chiusura **non riuscita**, con la guardia consumata.
    fn non_raccolto<P: super::ProcessoFiglio>(chiusura: super::Chiusura<P>) -> Vec<String> {
        match chiusura {
            super::Chiusura::Raccolto { uscita, .. } => {
                panic!("il figlio non doveva essere raccolto, e invece: {uscita:?}")
            }
            super::Chiusura::NonRaccolto { guardia, difetti } => {
                guardia.smonta();
                difetti
            }
        }
    }

    use std::cell::{Cell, RefCell};
    use std::time::Duration;

    use super::{FiglioVivo, Orologio, ProcessoFiglio, Uscita};

    /// Un orologio che non dorme: ogni attesa lo fa avanzare di un passo.
    struct OrologioFinto {
        trascorso: Cell<Duration>,
        passo: Duration,
        attese: Cell<u32>,
    }

    impl OrologioFinto {
        fn nuovo(passo: Duration) -> Self {
            Self {
                trascorso: Cell::new(Duration::ZERO),
                passo,
                attese: Cell::new(0),
            }
        }
    }

    impl Orologio for OrologioFinto {
        fn trascorso(&self) -> Duration {
            self.trascorso.get()
        }

        fn attendi_un_poco(&self) {
            self.trascorso.set(self.trascorso.get() + self.passo);
            self.attese.set(self.attese.get() + 1);
        }
    }

    /// Un processo che risponde come gli si dice.
    ///
    /// Serve per i cammini che su un processo vero non si producono a comando —
    /// «si termina ma non si lascia raccogliere» in testa a tutti.
    struct FigliFinto {
        /// Cio' che `prova_a_raccogliere` risponde, in ordine; l'ultimo si
        /// ripete.
        raccolte: RefCell<Vec<std::io::Result<Option<Uscita>>>>,
        terminazione: Cell<Option<std::io::ErrorKind>>,
        terminato: Cell<bool>,
        interrogazioni: Cell<u32>,
    }

    impl FigliFinto {
        fn nuovo(raccolte: Vec<std::io::Result<Option<Uscita>>>) -> Self {
            Self {
                raccolte: RefCell::new(raccolte),
                terminazione: Cell::new(None),
                terminato: Cell::new(false),
                interrogazioni: Cell::new(0),
            }
        }
    }

    impl ProcessoFiglio for &FigliFinto {
        fn pid(&self) -> u32 {
            4242
        }

        fn prova_a_raccogliere(&mut self) -> std::io::Result<Option<Uscita>> {
            self.interrogazioni.set(self.interrogazioni.get() + 1);
            let mut coda = self.raccolte.borrow_mut();
            if coda.len() > 1 {
                return coda.remove(0);
            }
            match coda.first() {
                Some(Ok(valore)) => Ok(*valore),
                Some(Err(errore)) => Err(std::io::Error::from(errore.kind())),
                None => Ok(Some(Uscita::Codice(0))),
            }
        }

        fn termina(&mut self) -> std::io::Result<()> {
            self.terminato.set(true);
            self.terminazione
                .get()
                .map_or(Ok(()), |genere| Err(std::io::Error::from(genere)))
        }
    }

    /// Un figlio gia' uscito si raccoglie senza che nessuno lo termini, e **la
    /// sua uscita si vede**.
    #[test]
    fn un_figlio_gia_uscito_non_si_termina_e_rende_la_sua_uscita() {
        let finto = FigliFinto::nuovo(vec![Ok(Some(Uscita::Codice(3)))]);
        let guardia = FiglioVivo::nuovo(&finto);
        let orologio = OrologioFinto::nuovo(Duration::from_millis(10));
        let (uscita, difetto) =
            raccolto(guardia.termina_e_raccogli(Duration::from_secs(5), &orologio));
        assert_eq!(difetto, None);
        assert_eq!(
            uscita,
            Some(Uscita::Codice(3)),
            "il codice non si perde: senza, la riga 2 e la riga 4 della matrice collassano"
        );
        assert!(
            !finto.terminato.get(),
            "a un figlio gia' uscito non si manda nessun segnale"
        );
    }

    /// E se e' morto per un segnale, si vede il segnale.
    #[test]
    fn un_figlio_ucciso_rende_il_segnale() {
        let finto = FigliFinto::nuovo(vec![Ok(Some(Uscita::Segnale(9)))]);
        let guardia = FiglioVivo::nuovo(&finto);
        let orologio = OrologioFinto::nuovo(Duration::from_millis(10));
        let (uscita, _difetto) =
            raccolto(guardia.termina_e_raccogli(Duration::from_secs(5), &orologio));
        assert_eq!(uscita, Some(Uscita::Segnale(9)));
    }

    /// Un figlio vivo si termina e poi si raccoglie.
    #[test]
    fn un_figlio_vivo_si_termina_e_poi_si_raccoglie() {
        let finto = FigliFinto::nuovo(vec![Ok(None), Ok(Some(Uscita::Segnale(9)))]);
        let guardia = FiglioVivo::nuovo(&finto);
        let orologio = OrologioFinto::nuovo(Duration::from_millis(10));
        let (uscita, difetto) =
            raccolto(guardia.termina_e_raccogli(Duration::from_secs(5), &orologio));
        assert_eq!(difetto, None);
        assert_eq!(
            uscita,
            Some(Uscita::Segnale(9)),
            "un figlio terminato da noi muore per segnale, e si vede"
        );
        assert!(finto.terminato.get(), "un figlio vivo va terminato");
    }

    /// **Il caso che conta**: terminazione riuscita, raccolta che non arriva
    /// mai, e il limite che scade.
    ///
    /// Su un processo vero questo cammino richiederebbe un programma che
    /// ignora `SIGKILL`, che non si scrive. Qui si compone: il figlio dice
    /// sempre «ancora vivo», la terminazione riesce, e la regola da provare e'
    /// che allo scadere si **riporti** invece di restare fermi.
    #[test]
    fn terminato_ma_non_raccoglibile_scade_e_si_riporta() {
        let finto = FigliFinto::nuovo(vec![Ok(None)]);
        let guardia = FiglioVivo::nuovo(&finto);
        let orologio = OrologioFinto::nuovo(Duration::from_millis(10));
        let difetti =
            non_raccolto(guardia.termina_e_raccogli(Duration::from_millis(50), &orologio));
        let riuniti = difetti.join("; ");
        assert!(
            riuniti.contains("non si lascia raccogliere"),
            "il difetto non e' quello atteso: {riuniti}"
        );
        assert!(
            finto.terminato.get(),
            "la terminazione e' comunque avvenuta"
        );
        // Cinque attese da 10 ms per arrivare a 50: la regola e' «si riprova
        // finche' il tempo dato non e' finito», e il conteggio la fissa.
        assert_eq!(orologio.attese.get(), 5);
        // Sette interrogazioni, non sei: una prima della terminazione — quella
        // che chiede «e' gia' uscito da solo?» — e sei nel giro, perche' allo
        // scadere si guarda un'ultima volta **prima** di rinunciare.
        assert_eq!(finto.interrogazioni.get(), 7);
    }

    /// Una terminazione che dice «non c'e' piu'» e' la corsa, non un difetto —
    /// e la raccolta **segue comunque**.
    #[test]
    fn la_corsa_sulla_terminazione_non_e_un_difetto_e_la_raccolta_segue() {
        let finto = FigliFinto::nuovo(vec![Ok(None), Ok(Some(Uscita::Codice(0)))]);
        finto
            .terminazione
            .set(Some(std::io::ErrorKind::InvalidInput));
        let guardia = FiglioVivo::nuovo(&finto);
        let orologio = OrologioFinto::nuovo(Duration::from_millis(10));
        let (uscita, difetto) =
            raccolto(guardia.termina_e_raccogli(Duration::from_secs(5), &orologio));
        assert_eq!(
            difetto, None,
            "il figlio uscito fra le due chiamate non e' un guasto"
        );
        assert_eq!(uscita, Some(Uscita::Codice(0)));
        assert_eq!(
            finto.interrogazioni.get(),
            2,
            "dopo la corsa la raccolta segue comunque: senza, resterebbe uno zombie"
        );
    }

    /// Una terminazione che fallisce davvero e' un difetto — e anche li' la
    /// raccolta segue.
    #[test]
    fn una_terminazione_fallita_e_un_difetto_ma_non_ferma_la_raccolta() {
        let finto = FigliFinto::nuovo(vec![Ok(None), Ok(Some(Uscita::Codice(0)))]);
        finto
            .terminazione
            .set(Some(std::io::ErrorKind::PermissionDenied));
        let guardia = FiglioVivo::nuovo(&finto);
        let orologio = OrologioFinto::nuovo(Duration::from_millis(10));
        let (uscita, difetto) =
            raccolto(guardia.termina_e_raccogli(Duration::from_secs(5), &orologio));
        assert_eq!(
            uscita,
            Some(Uscita::Codice(0)),
            "raccolto lo stesso: la terminazione rifiutata non impedisce la raccolta"
        );
        let difetto = difetto.expect("una terminazione rifiutata e' un difetto");
        assert!(difetto.contains("non si riesce a terminare"), "{difetto}");
        assert_eq!(finto.interrogazioni.get(), 2);
    }

    /// **Due guasti sono due difetti, e la guardia torna piena.**
    ///
    /// # Che cosa esclude
    ///
    /// Due cose insieme.
    ///
    /// La prima: che il secondo difetto **sostituisca** il primo. Una
    /// terminazione rifiutata e una raccolta impossibile sono due cose, e la
    /// seconda non spiega la prima. Chi legge un solo difetto cerchera' un
    /// problema di raccolta su un processo a cui nessuno e' riuscito nemmeno a
    /// mandare il segnale.
    ///
    /// La seconda, piu' grave: che il figlio **sparisca** sul cammino di
    /// fallimento. Estrarlo dalla guardia all'inizio lascia ogni ritorno
    /// anticipato senza padrone — guardia consumata, sentinella senza niente da
    /// sorvegliare, e un processo che puo' essere ancora vivo. Chiedere il `pid`
    /// alla guardia che torna e' il modo di vederlo: una guardia svuotata
    /// risponde `None`.
    #[test]
    fn una_terminazione_rifiutata_e_una_raccolta_impossibile_sono_due_difetti() {
        let finto = FigliFinto::nuovo(vec![
            Ok(None),
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        ]);
        finto
            .terminazione
            .set(Some(std::io::ErrorKind::PermissionDenied));
        let guardia = FiglioVivo::nuovo(&finto);
        let orologio = OrologioFinto::nuovo(Duration::from_millis(10));

        let super::Chiusura::NonRaccolto { guardia, difetti } =
            guardia.termina_e_raccogli(Duration::from_secs(5), &orologio)
        else {
            panic!("una raccolta che fallisce non produce un figlio raccolto");
        };

        assert_eq!(difetti.len(), 2, "i due guasti sono due: {difetti:?}");
        assert!(
            difetti[0].contains("non si riesce a terminare"),
            "il primo difetto e' quello della terminazione: {difetti:?}"
        );
        assert!(
            difetti[1].contains("non si riesce a raccogliere"),
            "e il secondo quello della raccolta: {difetti:?}"
        );

        // La guardia torna **con dentro il figlio**.
        assert_eq!(
            guardia.pid(),
            Some(4242),
            "il processo esiste ancora, e qualcuno deve restarne responsabile"
        );
        // E si esce dalla via dei casi: senza, il `Drop` di questa riga
        // abortirebbe il processo che li esegue.
        assert_eq!(guardia.smonta(), Some(4242));
    }

    /// La consegna svuota la guardia, cosi' la sentinella non scatta.
    #[test]
    fn la_consegna_svuota_la_guardia() {
        let finto = FigliFinto::nuovo(vec![Ok(Some(Uscita::Codice(0)))]);
        let guardia = FiglioVivo::nuovo(&finto);
        assert_eq!(guardia.pid(), Some(4242));
        assert!(guardia.consegna().is_some());
        // Se la consegna non avesse svuotato, il `Drop` di questa riga
        // abortirebbe il processo dei casi: il caso passa **perche'** la
        // sentinella non scatta.
    }

    /// **Le tre risposte dell'ultimo tentativo, e nessuna dice «terminato».**
    ///
    /// # Perche' le stringhe esatte e non solo la distinzione
    ///
    /// Perche' tre risposte distinte provano l'iniettivita', non la
    /// correttezza: sarebbero distinte anche se dicessero tutte una cosa piu'
    /// forte del vero. Cio' che conta e' **che cosa affermano**, e l'affermazione
    /// da escludere e' «il processo e' finito» — che nessuna delle tre e'
    /// autorizzata a fare, perche' [`ProcessoFiglio::termina`] manda la
    /// terminazione e non osserva l'uscita.
    ///
    /// Il caso su `/bin/sleep` mostra che nel caso qualificato il pid sparisce;
    /// questo fissa che non lo si trasformi in una garanzia universale.
    #[test]
    fn l_ultimo_tentativo_dice_cio_che_ha_fatto_e_non_di_piu() {
        // 1. Il segnale parte.
        let inviato = FigliFinto::nuovo(vec![Ok(None)]);
        let inviato = super::ultimo_tentativo_di_terminazione(&mut &inviato);
        assert_eq!(
            inviato,
            "segnale di terminazione inviato; uscita non osservata"
        );

        // 2. Il sistema dice che non c'e' piu' niente da terminare. Non e' «e'
        //    uscito»: e' compatibile con un figlio finito, e non lo prova.
        let non_terminabile = FigliFinto::nuovo(vec![Ok(None)]);
        non_terminabile
            .terminazione
            .set(Some(std::io::ErrorKind::InvalidInput));
        let non_terminabile = super::ultimo_tentativo_di_terminazione(&mut &non_terminabile);
        assert_eq!(
            non_terminabile,
            "processo non piu' terminabile; uscita non osservata"
        );

        // 3. Il segnale non parte affatto: e' l'unico dei tre in cui si sa
        //    qualcosa di brutto, ed e' quello che deve saltare all'occhio.
        let rifiutato = FigliFinto::nuovo(vec![Ok(None)]);
        rifiutato
            .terminazione
            .set(Some(std::io::ErrorKind::PermissionDenied));
        let rifiutato = super::ultimo_tentativo_di_terminazione(&mut &rifiutato);
        assert!(
            rifiutato.starts_with("segnale non inviato ("),
            "{rifiutato}"
        );
        assert!(
            rifiutato.ends_with("); puo' restare vivo"),
            "il motivo del sistema sta in mezzo, e la conseguenza in fondo: {rifiutato}"
        );

        // Nessuna afferma che il figlio sia morto.
        for risposta in [&inviato, &non_terminabile, &rifiutato] {
            assert!(
                !risposta.contains("terminato;") && !risposta.contains("gia' uscito"),
                "nessuna delle tre puo' dire che il processo e' finito: {risposta}"
            );
        }
        // E restano tre, non due: una tabella che collassasse due righe
        // renderebbe indistinguibili un segnale partito e uno rifiutato.
        assert_ne!(inviato, non_terminabile);
        assert_ne!(inviato, rifiutato);
        assert_ne!(non_terminabile, rifiutato);
    }

    /// La resa, in un processo che puo' permettersi di morire.
    ///
    /// Soggetto del caso che viene dopo, come `sentinella_soggetto`. Qui pero'
    /// il figlio **non** e' sfuggito: qualcuno ha provato a raccoglierlo, non ci
    /// e' riuscito, e sopra non c'e' nessuno che possa riprovare.
    ///
    /// # Perche' un `/bin/sleep` e non un finto
    ///
    /// Perche' cio' che il caso deve misurare e' che il figlio **muore**, e un
    /// finto non muore: risponderebbe `Ok(())` alla terminazione e il caso
    /// resterebbe verde anche se la resa non uccidesse nessuno. E' esattamente
    /// il difetto da escludere — `abort` ferma noi, non lui, e un processo
    /// lasciato cadere passa al reaper del sistema e sopravvive.
    ///
    /// La resa si chiama qui direttamente. Portare un `/bin/sleep` a non
    /// lasciarsi raccogliere richiederebbe un programma che ignora `SIGKILL`,
    /// che non si scrive: il cammino che porta alla resa e' provato altrove, e
    /// questo caso prova che cosa la resa **fa**.
    #[test]
    #[ignore = "abortisce di proposito: lo esegue il caso che lo osserva"]
    #[cfg(target_os = "linux")]
    fn resa_soggetto() {
        let figlio = std::process::Command::new("/bin/sleep")
            .arg("600")
            // I flussi vanno recisi: chi guarda legge con `output()`, che aspetta
            // l'EOF delle pipe e non l'uscita del processo. Un nipote che le
            // ereditasse le terrebbe aperte anche dopo la morte dell'osservato, e
            // proprio nel caso mutato l'osservatore resterebbe fermo invece di
            // diventare rosso.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("/bin/sleep si avvia");
        println!("PID={}", figlio.id());
        {
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
        let guardia = FiglioVivo::nuovo(figlio);
        guardia.arrenditi("il limite di raccolta e' scaduto")
    }

    /// **La prova della resa**: il processo si ferma, dice com'e' andata, e il
    /// figlio non resta.
    ///
    /// # Perche' da fuori, e perche' distinta dalla sentinella
    ///
    /// Da fuori per la stessa ragione: un caso non puo' osservare la propria
    /// fine. Distinta perche' le due righe devono restare **diverse**. La
    /// sentinella dice «sfuggito», e manda a cercare un cammino che non passa da
    /// nessuna porta; la resa dice «non si e' lasciato raccogliere, e nessuno
    /// puo' riprovare», e manda a guardare il figlio. Se un giorno collassassero
    /// nella stessa riga, questo caso lo direbbe.
    ///
    /// # La misura che conta, e fin dove arriva
    ///
    /// Il pid che sparisce. Fermarsi dichiarando di non voler lasciare un
    /// processo vivo, e lasciarlo vivo, e' peggio del difetto da evitare.
    ///
    /// Vale pero' **per questo caso**, non in generale: prova che su un
    /// `/bin/sleep` ordinario la resa manda il segnale prima di fermarsi e il
    /// processo se ne va. Non prova che un qualunque figlio muoia — un processo
    /// in attesa ininterrompibile riceve il segnale e resta finche' la chiamata
    /// di sistema non ritorna — ed e' per questo che il messaggio dice «segnale
    /// inviato» e non «terminato».
    #[test]
    #[cfg(target_os = "linux")]
    fn la_resa_ferma_il_processo_e_non_lascia_il_figlio() {
        use std::os::unix::process::ExitStatusExt as _;

        let eseguibile = std::env::current_exe().expect("l'eseguibile dei casi ha un percorso");
        let uscita = std::process::Command::new(eseguibile)
            .args([
                "isolamento::figlio::tests::resa_soggetto",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .output()
            .expect("il processo dedicato si avvia");

        let fuori = String::from_utf8_lossy(&uscita.stdout);
        let errori = String::from_utf8_lossy(&uscita.stderr);

        // 1. Si e' fermato, e per abort.
        assert_eq!(
            uscita.status.signal(),
            Some(6),
            "una resa deve fermare il processo, e per abort — stdout: {fuori} / stderr: {errori}"
        );

        // 2. E ha detto che cosa e' successo, compreso l'esito del colpo. Senza
        //    quest'ultimo, «terminato» e «non terminato» si leggerebbero uguali.
        assert!(
            errori.contains("non si e' lasciato raccogliere"),
            "la resa deve dire che cosa e' successo: {errori}"
        );
        assert!(
            errori.contains("nessuno che possa riprovare"),
            "e perche' ci si ferma qui invece di riportare e proseguire: {errori}"
        );
        assert!(
            errori.contains("Ultimo tentativo di terminazione: segnale di terminazione inviato"),
            "e che cosa ha potuto fare l'ultimo tentativo: {errori}"
        );
        assert!(
            !errori.contains("e' sfuggito alla guardia"),
            "la resa non e' la sentinella, e le due righe non devono collassare: {errori}"
        );

        // 3. **Il figlio non e' rimasto.** E' la meta' che non si vede: `abort`
        //    ferma noi, non lui.
        let pid: u32 = fuori
            .lines()
            .find_map(|riga| riga.strip_prefix("PID="))
            .and_then(|numero| numero.trim().parse().ok())
            .unwrap_or_else(|| panic!("il soggetto non ha dichiarato il pid: {fuori}"));

        // Se ne occupa il reaper del sistema, non noi: l'attesa e' limitata, e si
        // guarda `comm` invece della sola esistenza perche' un pid libero puo'
        // tornare in uso.
        let comando = format!("/proc/{pid}/comm");
        let mut resta = true;
        for _ in 0..200_u32 {
            match std::fs::read_to_string(&comando) {
                Err(_) => {
                    resta = false;
                    break;
                }
                Ok(nome) if nome.trim() != "sleep" => {
                    resta = false;
                    break;
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(
            !resta,
            "il figlio {pid} e' sopravvissuto alla resa: fermarsi dicendo di non volerlo \
             lasciare vivo, e lasciarlo vivo, e' peggio del difetto da evitare"
        );
    }

    /// Il cammino sfuggito, in un processo che puo' permettersi di morire.
    ///
    /// Non e' un caso: e' il **soggetto** del caso che viene dopo. Lascia
    /// cadere la guardia senza passare da nessuna delle porte — cioe' fa
    /// esattamente cio' che un `?` distratto farebbe — e si aspetta di essere
    /// fermato. Marcato `ignore` perche' eseguirlo insieme agli altri
    /// abbatterebbe l'intera batteria: lo esegue solo chi lo sta guardando.
    #[test]
    #[ignore = "abortisce di proposito: lo esegue il caso che lo osserva"]
    #[cfg(target_os = "linux")]
    fn sentinella_soggetto() {
        let figlio = std::process::Command::new("/bin/sleep")
            .arg("600")
            // **I flussi vanno recisi**, e non e' igiene: chi guarda legge con
            // `output()`, che aspetta l'EOF delle pipe e non l'uscita del
            // processo. Un nipote che le ereditasse le terrebbe aperte anche
            // dopo la morte dell'osservato — e proprio nel caso mutato, quello
            // in cui il nipote sopravvive, l'osservatore resterebbe fermo per
            // dieci minuti invece di diventare rosso. Un caso che si appende al
            // posto di fallire e' un caso che non misura.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("/bin/sleep si avvia");
        // Il pid esce **prima** della caduta: dopo non c'e' piu' nessuno che
        // possa scriverlo, ed e' cio' che serve a chi guarda per verificare che
        // il figlio non sia rimasto.
        println!("PID={}", figlio.id());
        {
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
        let _guardia = FiglioVivo::nuovo(figlio);
        // Fine dello scope: nessuna consegna, nessuna raccolta. L'uscita che
        // nessuno vuole.
    }

    /// **La prova della sentinella**: il processo si ferma, lo dice, e il figlio
    /// sfuggito non resta.
    ///
    /// # Perche' da fuori
    ///
    /// Perche' l'effetto della sentinella e' fermare il processo, e un caso non
    /// puo' osservare la propria fine. Chi guarda e' un processo diverso: rilegge
    /// il proprio eseguibile chiedendogli il solo caso `sentinella_soggetto`, e
    /// misura tre cose che insieme distinguono la sentinella da qualunque altra
    /// morte — il segnale, la riga che la spiega, e l'assenza del nipote.
    #[test]
    #[cfg(target_os = "linux")]
    fn la_sentinella_ferma_il_processo_e_non_lascia_il_figlio() {
        use std::os::unix::process::ExitStatusExt as _;

        let eseguibile = std::env::current_exe().expect("l'eseguibile dei casi ha un percorso");
        let uscita = std::process::Command::new(eseguibile)
            .args([
                "isolamento::figlio::tests::sentinella_soggetto",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .output()
            .expect("il processo dedicato si avvia");

        let fuori = String::from_utf8_lossy(&uscita.stdout);
        let errori = String::from_utf8_lossy(&uscita.stderr);

        // 1. Si e' fermato, e per abort: non un'uscita ordinaria, non un panico
        //    con codice, non un successo silenzioso.
        assert_eq!(
            uscita.status.signal(),
            Some(6),
            "il processo non e' stato fermato da abort — stdout: {fuori} / stderr: {errori}"
        );

        // 2. E lo ha detto. Un abort muto sarebbe indiagnosticabile.
        assert!(
            errori.contains("sfuggito alla guardia"),
            "la sentinella non ha lasciato traccia: {errori}"
        );
        // Compresa la risposta dell'ultimo tentativo. Una riga che dicesse
        // sempre la stessa cosa non distinguerebbe un segnale partito da uno
        // rifiutato, cioe' proprio la differenza per cui il messaggio esiste.
        assert!(
            errori.contains("Ultimo tentativo di terminazione: segnale di terminazione inviato"),
            "la sentinella deve dire che cosa ha potuto fare: {errori}"
        );

        // 3. Il nipote non e' rimasto: la terminazione best-effort e' avvenuta
        //    prima dell'abort. Senza questa misura la sentinella potrebbe
        //    limitarsi a morire, che e' meta' del lavoro.
        let pid: u32 = fuori
            .lines()
            .find_map(|riga| riga.strip_prefix("PID="))
            .and_then(|numero| numero.trim().parse().ok())
            .unwrap_or_else(|| panic!("il soggetto non ha dichiarato il pid: {fuori}"));

        // Del nipote si occupa il reaper del sistema, non noi: l'attesa e'
        // limitata, e si guarda `comm` invece della sola esistenza perche' un pid
        // libero puo' sempre tornare in uso.
        let comando = format!("/proc/{pid}/comm");
        let mut resta = true;
        for _ in 0..200_u32 {
            match std::fs::read_to_string(&comando) {
                Err(_) => {
                    resta = false;
                    break;
                }
                Ok(nome) if nome.trim() != "sleep" => {
                    resta = false;
                    break;
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(
            !resta,
            "il figlio {pid} e' sopravvissuto alla sentinella: terminarlo e' la meta' che non si vede"
        );
    }

    /// Un processo vero, per non provare la regola solo su un finto.
    #[test]
    #[cfg(unix)]
    fn un_processo_vero_si_termina_e_si_raccoglie() {
        let figlio = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("/bin/sleep si avvia");
        let pid = figlio.id();
        let guardia = FiglioVivo::nuovo(figlio);
        let orologio = super::OrologioDiSistema::nuovo(Duration::from_millis(5));
        let (uscita, difetto) =
            raccolto(guardia.termina_e_raccogli(Duration::from_secs(5), &orologio));
        assert_eq!(difetto, None);
        // Su un processo vero, ucciso da noi: il sistema riporta il segnale, non
        // un codice. E' la prova che l'uscita che arriva e' quella del sistema e
        // non una che ci siamo inventati.
        assert_eq!(uscita, Some(Uscita::Segnale(9)), "ucciso da SIGKILL");
        // Raccolto vuol dire raccolto: il pid non porta piu' uno zombie nostro.
        let stato = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(
            !stato.contains(" Z "),
            "il figlio e' rimasto zombie: {stato}"
        );
    }
}
