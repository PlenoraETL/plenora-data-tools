//! Governor della memoria del piano e batch governati (ADR-0002, Fase 2B —
//! milestone M1a/M1b; Architetture.md par. 6.4, Prestazioni.md par. 3).
//!
//! Perimetro di `max_memory_bytes`: la memoria Arrow governata dall'engine —
//! i batch che attraversano gli archi del DAG e le materializzazioni
//! intermedie dei segmenti blocking. Il conteggio avviene **ai confini di
//! batch**, mai per riga, e il governor non percorre mai ricorsivamente i
//! batch: i byte di un lease sono fissati all'acquisizione e nessun nodo li
//! riconta (overhead ADR-0002).
//!
//! Ownership: ogni batch viaggia in un [`GovernedBatch`] con il suo
//! [`MemoryLease`] — reference-counted (`Arc` interno), condiviso al fan-out
//! (tee) e rilasciato al `Drop` dell'ultimo riferimento. La quota di un
//! batch e' contata **una sola volta**, all'ingresso dell'arco; i cloni del
//! tee condividono il lease senza mai duplicare il conteggio.
//!
//! Protocollo di reservation a tre vie ([`ReservationResult`], ADR-0002):
//! in questa milestone l'esecuzione e' seriale e il governor emette solo
//! `Granted` — vedi [`MemoryGovernor::try_reserve`] per la regola v1 e il
//! perche' gli altri due esiti non sono attuabili in seriale.
//!
//! Spill (Fase 2B M2c): `sort`/`distinct`/`aggregate` hanno una variante
//! spilled cablata nell'executor, ma l'attivazione e' **PREVENTIVA** ai punti
//! di dispatch (soglia stimata "byte input > `max_memory_bytes`", ADR-0002
//! "attivazione prima dell'esaurimento"), NON guidata da una reservation
//! fallita: `MustSpill` resta non emesso in v1 — il re-scheduling su
//! reservation fallita richiede il planner che riprova (M3).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use plenora_core::arrow::array::RecordBatch;
use plenora_core::contract::BatchSequence;
use plenora_core::{PlenoraError, Result};

/// Esito di una richiesta di reservation (ADR-0002, protocollo
/// anti-deadlock: niente fail-fast immediato quando la quota potrebbe
/// liberarsi a breve).
#[derive(Debug)]
pub enum ReservationResult {
    /// Quota assegnata: il lease restituisce i byte al governor al `Drop`
    /// dell'ultimo clone.
    Granted(MemoryLease),
    /// La quota potrebbe liberarsi dopo un progresso globale del piano: il
    /// ramo richiedente (che per invariante ADR-0002 non trattiene risorse)
    /// puo' essere sospeso e riprovare, senza busy-waiting. Richiede uno
    /// scheduler con rami sospendibili: esiste nell'API per il runtime
    /// parallelo (M3) ma non e' MAI emesso dalla v1 seriale.
    RetryAfterProgress,
    /// Il richiedente ha una strategia di spill e deve attivarla (preferita
    /// a nuova quota, ADR-0002). Resta MAI emesso in v1: lo spill selettivo
    /// esiste (Fase 2B M2c: sort/distinct/aggregate spilled) ma la sua
    /// attivazione e' PREVENTIVA ai punti di dispatch, su soglia stimata —
    /// non su reservation fallita. Emetterlo richiede il planner che
    /// riprova il nodo con una strategia diversa (re-scheduling, M3).
    MustSpill,
}

/// Stato interno condiviso di un lease: i byte tornano al governor al
/// `Drop` di questa struttura, cioe' al `Drop` dell'ULTIMO clone del lease.
#[derive(Debug)]
struct LeaseInner {
    governor: Arc<GovernorShared>,
    id: u64,
    /// Byte trattenuti. Atomico perche' [`MemoryPermit::ritaglia`] li
    /// riduce: un permesso prenota un maggiorante e restituisce la
    /// differenza appena la dimensione esatta e' nota, senza mai rilasciare e
    /// riprenotare.
    bytes: AtomicU64,
    /// Nodo/arco che ha acquisito la quota (osservabilita' ADR-0002).
    owner: String,
    created: Instant,
}

impl Drop for LeaseInner {
    fn drop(&mut self) {
        // Mai panic in Drop: un mutex avvelenato durante uno unwind non deve
        // abortire il processo — il rilascio della quota resta garantito.
        // Tutte le mutazioni nella STESSA sezione critica di acquisizioni e
        // snapshot: il rilascio e' linearizzabile come l'acquisizione.
        let mut stato = self.governor.blocca();
        let bytes = self.bytes.load(Ordering::Acquire);
        // Decrementi controllati: un underflow qui significherebbe che si sta
        // restituendo quota mai presa, cioe' contabilita' rotta. Non si puo'
        // restituire un errore da `Drop`, ma si puo' MARCARLA: da quel
        // momento ogni richiesta al governor fallisce con `Internal` invece
        // di operare su numeri che non descrivono piu' nulla.
        match (stato.reserved.checked_sub(bytes), stato.live.checked_sub(1)) {
            (Some(reserved), Some(live)) => {
                stato.reserved = reserved;
                stato.live = live;
            }
            _ => {
                stato.corrotta = Some("rilascio di quota mai acquisita");
            }
        }
        // Biiezione: la nascita DEVE esserci. Se manca, la mappa e il
        // contatore non descrivono piu' lo stesso insieme di lease.
        if stato.births.remove(&self.id).is_none() {
            stato.corrotta = Some("nascita mancante al rilascio di un lease");
        }
    }
}

/// Lease di memoria RAII (ADR-0002): `bytes` byte di quota del budget
/// globale di piano.
///
/// Reference-counted: i cloni condividono la STESSA quota (mai doppio
/// conteggio); i byte tornano al governor al `Drop` dell'ultimo clone. Mai
/// per riga: un lease copre un intero batch o una materializzazione
/// intermedia.
#[derive(Clone, Debug)]
pub struct MemoryLease {
    inner: Arc<LeaseInner>,
}

impl MemoryLease {
    /// Byte di quota coperti dal lease.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.inner.bytes.load(Ordering::Acquire)
    }

    /// Nodo/arco proprietario originario (osservabilita' ADR-0002).
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.inner.owner
    }

    /// Eta' del lease (dalla sua creazione).
    #[must_use]
    pub fn age(&self) -> Duration {
        self.inner.created.elapsed()
    }
}

/// Contabilita' del governor, tutta sotto **un solo** lock.
///
/// # Perche' un lock e non atomici separati
///
/// I contatori non sono indipendenti: `reserved`, `live` e `births`
/// descrivono lo stesso fatto — quali lease esistono e quanto trattengono.
/// Tenerli in atomici distinti li rende aggiornabili solo uno alla volta, e
/// uno snapshot che li legge separatamente puo' cadere in mezzo: byte gia'
/// contati e lease non ancora, o viceversa. Non e' un problema in v1 seriale,
/// ma lo snapshot e' osservabilita' e l'osservabilita' incoerente e' peggio
/// di quella assente.
///
/// Sotto un lock unico ogni acquisizione, ogni rilascio e ogni snapshot sono
/// **linearizzabili**: chi legge vede uno stato che e' realmente esistito. Il
/// costo e' un mutex per lease — cioe' per batch, mai per riga — e il
/// percorso lo prendeva gia' per registrare la nascita del lease.
#[derive(Debug)]
struct Contabilita {
    reserved: u64,
    peak: u64,
    live: u64,
    next_id: u64,
    /// Istante di nascita per id: l'eta' del piu' vecchio si ricava dal
    /// **minimo degli istanti**, non dal primo id. Gli id sono assegnati
    /// prima della lettura dell'orologio, quindi in concorso l'ordine degli
    /// id non e' l'ordine dei tempi.
    births: BTreeMap<u64, Instant>,
    /// Motivo per cui la contabilita' non e' piu' attendibile. Una volta
    /// impostato non si torna indietro: ogni richiesta successiva fallisce.
    corrotta: Option<&'static str>,
}

/// Contabilita' condivisa tra il governor e i lease vivi.
#[derive(Debug)]
struct GovernorShared {
    budget: u64,
    stato: Mutex<Contabilita>,
}

impl GovernorShared {
    /// Accesso alla contabilita', recuperando un mutex avvelenato.
    ///
    /// Un lock avvelenato significa che un thread e' andato in panico
    /// tenendolo. I dati restano leggibili e coerenti — le mutazioni qui sono
    /// sequenze brevi senza punti di panico intermedi — e propagare il
    /// panico dal `Drop` di un lease abortirebbe il processo.
    fn blocca(&self) -> std::sync::MutexGuard<'_, Contabilita> {
        self.stato.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Governor della memoria del piano (ADR-0002): unico budget globale
/// (`max_memory_bytes` dei limiti effettivi).
///
/// Il budget e' condiviso da tutti gli archi e i segmenti — in prospettiva
/// M3 anche dai rami paralleli, che per invariante condivideranno la stessa
/// quota. Clone economico (stato `Arc` condiviso).
#[derive(Clone, Debug)]
pub struct MemoryGovernor {
    shared: Arc<GovernorShared>,
}

impl MemoryGovernor {
    /// Governor con budget `max_memory_bytes`.
    #[must_use]
    pub fn new(max_memory_bytes: u64) -> Self {
        Self {
            shared: Arc::new(GovernorShared {
                budget: max_memory_bytes,
                stato: Mutex::new(Contabilita {
                    reserved: 0,
                    peak: 0,
                    live: 0,
                    next_id: 0,
                    births: BTreeMap::new(),
                    corrotta: None,
                }),
            }),
        }
    }

    /// Budget globale configurato.
    #[must_use]
    pub fn budget_bytes(&self) -> u64 {
        self.shared.budget
    }

    /// Byte attualmente trattenuti dai lease vivi.
    #[must_use]
    pub fn reserved_bytes(&self) -> u64 {
        self.shared.blocca().reserved
    }

    /// Picco storico dei byte trattenuti.
    #[must_use]
    pub fn peak_reserved_bytes(&self) -> u64 {
        self.shared.blocca().peak
    }

    /// Numero di lease vivi.
    #[must_use]
    pub fn live_leases(&self) -> u64 {
        self.shared.blocca().live
    }

    /// Eta' del lease piu' vecchio (`None` se non ci sono lease vivi).
    ///
    /// Il piu' vecchio e' quello con l'**istante minimo**, non quello con
    /// l'id minore. Con la contabilita' sotto un lock unico i due ordini oggi
    /// **non possono divergere**: id e istante sono assegnati nella stessa
    /// sezione critica, nell'ordine. Il minimo degli istanti non corregge
    /// quindi un caso raggiungibile — esprime la semantica giusta, e regge se
    /// un domani l'assegnazione dell'id uscisse dal lock.
    #[must_use]
    pub fn oldest_lease_age(&self) -> Option<Duration> {
        self.shared
            .blocca()
            .births
            .values()
            .min()
            .map(Instant::elapsed)
    }

    /// Snapshot di osservabilita' ADR-0002 per le metriche di esecuzione.
    ///
    /// **Linearizzabile**: tutti i campi sono letti sotto lo stesso lock che
    /// acquisizioni e rilasci usano per mutarli, quindi descrivono uno stato
    /// realmente esistito e non un miscuglio di istanti diversi.
    #[must_use]
    pub fn snapshot(&self) -> MemoryMetrics {
        let stato = self.shared.blocca();
        MemoryMetrics {
            budget_bytes: self.shared.budget,
            reserved_bytes: stato.reserved,
            peak_reserved_bytes: stato.peak,
            live_leases: stato.live,
            oldest_lease_age: stato.births.values().min().map(Instant::elapsed),
            accounting_corrupted: stato.corrotta.is_some(),
        }
    }

    /// Verifica che la contabilita' sia ancora attendibile.
    ///
    /// Va chiamata **prima di dichiarare conclusa con successo**
    /// un'esecuzione: una corruzione rilevata dentro un `Drop` non puo'
    /// propagare un errore da li', quindi senza questo controllo l'ultimo
    /// output verrebbe pubblicato da un governor che ha gia' perso il conto.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Internal` se la contabilita' e' stata marcata
    /// incoerente in qualunque momento dell'esecuzione.
    pub fn verifica_salute(&self, owner: &str) -> Result<()> {
        let stato = self.shared.blocca();
        if let Some(motivo) = stato.corrotta {
            drop(stato);
            return Err(contabilita_corrotta(motivo, owner));
        }
        Ok(())
    }

    /// Reservation a tre vie (ADR-0002).
    ///
    /// Regola v1 (seriale, M1a): l'acquisizione e' **immediata** —
    /// `Granted` se il budget residuo copre `bytes`. Se la quota manca,
    /// l'ADR-0002 prescriverebbe `RetryAfterProgress` (sospensione del ramo
    /// e retry dopo un progresso globale) o `MustSpill` (strategia di spill
    /// preferita): in seriale NESSUNO dei due esiti e' attuabile — non
    /// esiste uno scheduler che sospenda i rami (M3) ne' un planner che
    /// riprovi il nodo con lo spill (M3; lo spill M2c e' attivato
    /// PREVENTIVAMENTE al dispatch, su soglia stimata, non da qui) — quindi
    /// resta l'unico esito residuo dell'ADR-0002, il fail-fast "nessuna
    /// strategia sicura disponibile".
    /// Per questo `RetryAfterProgress` e `MustSpill` esistono nell'API ma
    /// non sono MAI emessi da questa implementazione.
    ///
    /// # Errors
    ///
    /// - `PlenoraError::ResourceLimit` se il budget residuo non copre
    ///   `bytes`: il piano e' corretto, il budget no;
    /// - `PlenoraError::Internal` se la contabilita' del governor e'
    ///   incoerente (vedi [`Self::permesso`]). E' un'invariante nostra rotta,
    ///   non un piano sbagliato.
    pub fn try_reserve(&self, bytes: u64, owner: &str) -> Result<ReservationResult> {
        // Il valore riportato e' quello che ha CAUSATO il rifiuto, letto
        // sotto lo stesso lock che ha deciso: rileggerlo dopo darebbe un
        // numero che in concorso puo' essere gia' un altro, e l'errore
        // racconterebbe uno stato diverso da quello che ha deciso.
        match self.concedi(bytes, owner)? {
            Ok(permesso) => Ok(ReservationResult::Granted(permesso.in_lease()?)),
            Err(riservati) => {
                let budget = self.shared.budget;
                Err(PlenoraError::ResourceLimit(format!(
                    "max_memory_bytes superato: `{owner}` richiede {bytes} byte, \
                     {riservati} gia' riservati su un budget di {budget}"
                )))
            }
        }
    }

    /// **Permesso atomico**: verifica e prenota in UNA sola operazione.
    ///
    /// E' il primitivo su cui poggia tutto il resto — `try_reserve` e
    /// `reserve` ne sono involucri — e l'unico punto del crate in cui la
    /// quota viene presa.
    ///
    /// # Perche' esiste
    ///
    /// Leggere un contatore e poi prenotare in base a quella lettura e' due
    /// operazioni: fra le due un altro richiedente puo' inserirsi, e la
    /// decisione risulta presa su uno stato gia' superato. In v1 seriale la
    /// finestra non si apre, ma la forma e' sbagliata e con uno scheduler
    /// parallelo diventerebbe un TOCTOU silenzioso. Qui verifica e
    /// prenotazione avvengono nella stessa sezione critica.
    ///
    /// # I tre esiti sono distinti, e devono restarlo
    ///
    /// - `Ok(Some(permesso))`: quota concessa;
    /// - `Ok(None)`: **il budget non basta**. Non e' un errore, e' una
    ///   decisione: chi ha un'alternativa — per esempio passare al disco —
    ///   non deve costruire e poi scartare un errore;
    /// - `Err(Internal)`: **la contabilita' e' incoerente**. Un diniego di
    ///   budget e un errore interno non vanno confusi: il primo dipende dal
    ///   piano e dai limiti, il secondo e' un'invariante nostra rotta, e
    ///   trattarli allo stesso modo farebbe cercare la causa nel posto
    ///   sbagliato.
    ///
    /// # Proprieta'
    ///
    /// - **aritmetica controllata ovunque**: byte, contatore dei lease vivi e
    ///   generatore di id passano tutti da `checked_add`/`checked_sub`. Un
    ///   superamento non avvolge mai in silenzio: marca la contabilita' come
    ///   corrotta e da quel momento ogni richiesta fallisce (fail-closed);
    /// - **nessuna prenotazione parziale**: le mutazioni avvengono in una
    ///   sola sezione critica, e se una qualsiasi non e' rappresentabile
    ///   nessuna viene pubblicata;
    /// - **rilascio esatto**: la quota torna al governor al `Drop` dell'ultimo
    ///   riferimento, quindi anche lungo un unwind da errore o da
    ///   cancellazione. Il doppio rilascio e' impossibile per costruzione: non
    ///   esiste alcun metodo che rilasci, esiste solo il `Drop`;
    /// - **`bytes == 0`** e' un permesso valido a costo zero: semplifica i
    ///   chiamanti la cui dimensione calcolata puo' risultare nulla.
    ///
    /// Il permesso va **tenuto**: scartarlo lo rilascia immediatamente e la
    /// quota torna disponibile, che e' quasi sempre il contrario di quello
    /// che chi lo ha chiesto voleva. Da qui `#[must_use]`.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Internal` se la contabilita' e' incoerente.
    #[must_use = "un permesso scartato rilascia subito la quota"]
    pub fn permesso(&self, bytes: u64, owner: &str) -> Result<Option<MemoryPermit>> {
        Ok(self.concedi(bytes, owner)?.ok())
    }

    /// Come [`Self::permesso`], ma in caso di diniego riporta i byte
    /// riservati **al momento del rifiuto**: e' il valore che ha deciso, ed e'
    /// l'unico che un messaggio d'errore possa citare senza mentire.
    fn concedi(&self, bytes: u64, owner: &str) -> Result<std::result::Result<MemoryPermit, u64>> {
        let (id, created) = {
            let mut stato = self.shared.blocca();
            if let Some(motivo) = stato.corrotta {
                return Err(contabilita_corrotta(motivo, owner));
            }
            // Verifica e prenotazione nella stessa sezione critica.
            let Some(totale) = stato
                .reserved
                .checked_add(bytes)
                .filter(|totale| *totale <= self.shared.budget)
            else {
                // Somma non rappresentabile o fuori budget: in entrambi i
                // casi si nega, senza pubblicare nulla. La distinzione non
                // serve al chiamante — non c'e' quota — e trattare
                // l'overflow come corruzione bloccherebbe un governor sano.
                return Ok(Err(stato.reserved));
            };
            // I due contatori di servizio sono controllati come i byte: se
            // uno di loro non e' rappresentabile la contabilita' non e' piu'
            // attendibile, e nulla di questa richiesta viene pubblicato.
            let (Some(live), Some(next_id)) =
                (stato.live.checked_add(1), stato.next_id.checked_add(1))
            else {
                stato.corrotta = Some("contatore dei lease vivi o degli id esaurito");
                return Err(contabilita_corrotta(
                    "contatore dei lease vivi o degli id esaurito",
                    owner,
                ));
            };
            let id = stato.next_id;
            // Biiezione `live == births.len()`: un id gia' presente
            // significherebbe due lease con la stessa identita', e la mappa
            // ne perderebbe uno senza che nessun contatore se ne accorga.
            //
            // Il controllo sta PRIMA di ogni mutazione: verificarlo dopo aver
            // scritto `reserved` lascerebbe byte prenotati su una richiesta
            // fallita — una prenotazione parziale, cioe' esattamente cio' che
            // questo primitivo promette di non produrre.
            if stato.births.contains_key(&id) {
                stato.corrotta = Some("id di lease duplicato");
                drop(stato);
                return Err(contabilita_corrotta("id di lease duplicato", owner));
            }
            let created = Instant::now();
            stato.reserved = totale;
            stato.peak = stato.peak.max(totale);
            stato.live = live;
            stato.next_id = next_id;
            stato.births.insert(id, created);
            debug_assert_eq!(
                stato.live,
                stato.births.len() as u64,
                "biiezione fra lease vivi e nascite registrate"
            );
            // Il lock si rilascia QUI, prima di costruire il permesso:
            // l'allocazione dell'`Arc` non ha bisogno della sezione critica e
            // tenercela dentro allungherebbe l'attesa di chi aspetta.
            drop(stato);
            (id, created)
        };
        Ok(Ok(MemoryPermit {
            lease: MemoryLease {
                inner: Arc::new(LeaseInner {
                    governor: Arc::clone(&self.shared),
                    id,
                    bytes: AtomicU64::new(bytes),
                    owner: owner.to_owned(),
                    created,
                }),
            },
        }))
    }

    /// Acquisizione v1: il lease, o l'errore fail-fast (regola in
    /// [`Self::try_reserve`]).
    ///
    /// # Errors
    ///
    /// Come [`Self::try_reserve`].
    pub fn reserve(&self, bytes: u64, owner: &str) -> Result<MemoryLease> {
        match self.try_reserve(bytes, owner)? {
            ReservationResult::Granted(lease) => Ok(lease),
            // Ramo DIFENSIVO: la v1 non emette mai questi esiti (vedi
            // `try_reserve`). Se ci arrivasse, sarebbe un'invariante nostra
            // rotta — non un piano sbagliato e non un budget esaurito.
            // `Internal` lo dice; `InvalidPlan` mandava chi legge a cercare
            // un errore nel proprio piano.
            ReservationResult::RetryAfterProgress | ReservationResult::MustSpill => {
                Err(PlenoraError::Internal(format!(
                    "max_memory_bytes: esito di reservation non attuabile in v1 per `{owner}`"
                )))
            }
        }
    }
}

#[cfg(test)]
impl MemoryGovernor {
    /// Porta il generatore di id vicino al fondo scala. **Solo per i test**:
    /// l'esaurimento e' irraggiungibile in esercizio, ma il comportamento
    /// dev'essere verificato lo stesso — un ramo fail-closed mai eseguito e'
    /// un ramo di cui non si sa nulla.
    fn forza_next_id(&self, valore: u64) {
        self.shared.blocca().next_id = valore;
    }

    /// Come sopra, per il contatore dei lease vivi.
    fn forza_live(&self, valore: u64) {
        self.shared.blocca().live = valore;
    }

    /// `true` se la contabilita' e' stata marcata incoerente.
    pub(crate) fn e_corrotta(&self) -> bool {
        self.shared.blocca().corrotta.is_some()
    }

    /// Marca la contabilita' come incoerente. **Solo per i test**: serve a
    /// verificare che nessuna trasformazione di un permesso riesca dopo, e
    /// che il controllo di salute lo intercetti.
    pub(crate) fn corrompi_per_test(&self, motivo: &'static str) {
        self.shared.blocca().corrotta = Some(motivo);
    }

    /// Cancella la nascita del primo lease vivo, rompendo la biiezione
    /// `live == births.len()`. **Solo per i test**: nessun percorso reale
    /// puo' farlo, ma il rilascio deve accorgersene lo stesso.
    fn rimuovi_nascita_per_test(&self) {
        let mut stato = self.shared.blocca();
        if let Some(id) = stato.births.keys().next().copied() {
            stato.births.remove(&id);
        }
    }
}

/// Errore di contabilita' incoerente: e' un'invariante nostra rotta, non un
/// budget esaurito, e il messaggio deve dirlo a chi legge.
fn contabilita_corrotta(motivo: &str, owner: &str) -> PlenoraError {
    PlenoraError::Internal(format!(
        "contabilita' del governor incoerente ({motivo}): richiesta di `{owner}` rifiutata"
    ))
}

/// Quota ottenuta con [`MemoryGovernor::permesso`]: verificata e prenotata in
/// una sola operazione.
///
/// Un permesso **trattiene davvero** la quota finche' vive. E' questa la
/// differenza rispetto a leggere un contatore e decidere: fra la decisione e
/// l'uso nessun altro puo' prendere quei byte, perche' sono gia' presi.
///
/// Tre destini, tutti espliciti:
///
/// - [`MemoryPermit::in_lease`] lo converte nel lease definitivo, **senza
///   nuova prenotazione**: la quota e' la stessa, cambia solo il nome di chi
///   la tiene;
/// - [`MemoryPermit::ritaglia`] ne ricava un lease piu' piccolo restituendo
///   subito la differenza — per chi prenota un maggiorante prima di conoscere
///   la dimensione esatta;
/// - il `Drop` restituisce tutto, anche lungo un unwind.
///
/// Non e' `Clone`: un permesso e' un diritto esclusivo su una quota, e
/// duplicarlo significherebbe raddoppiarne l'uso senza raddoppiarne il
/// conteggio.
#[derive(Debug)]
pub struct MemoryPermit {
    lease: MemoryLease,
}

impl MemoryPermit {
    /// Byte trattenuti dal permesso.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.lease.bytes()
    }

    /// Converte il permesso nel lease definitivo, senza nuova prenotazione.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Internal` se la contabilita' e' gia' corrotta: **nessuna
    /// trasformazione di un permesso** puo' riuscire su un governor che non sa
    /// piu' quanto trattiene. Il permesso viene distrutto e la quota
    /// restituita.
    pub fn in_lease(self) -> Result<MemoryLease> {
        {
            let stato = self.lease.inner.governor.blocca();
            if let Some(motivo) = stato.corrotta {
                let owner = self.lease.owner().to_owned();
                drop(stato);
                return Err(contabilita_corrotta(motivo, &owner));
            }
        }
        Ok(self.lease)
    }

    /// Riduce il permesso a `bytes` e lo consegna come lease, restituendo
    /// subito la differenza al governor.
    ///
    /// Serve a chi deve prenotare un **maggiorante** prima di conoscere la
    /// dimensione esatta — perche' la conoscera' solo dopo aver eseguito il
    /// lavoro — e non puo' rilasciare e riprenotare: fra i due momenti la
    /// quota potrebbe sparire, ed e' esattamente la finestra che il permesso
    /// esiste per chiudere.
    ///
    /// La quota non viene mai ri-prenotata: si abbassa il conteggio dello
    /// stesso lease e si restituisce solo il resto. L'ordine — prima
    /// abbassare cio' che il lease dichiara, poi restituire il resto — fa si'
    /// che il contatore del governor sia in ogni istante **maggiore o uguale**
    /// a quanto i lease vivi dichiarano: mai il contrario.
    ///
    /// # Errors
    ///
    /// `PlenoraError::Internal` in due casi, entrambi invarianti nostre rotte
    /// e non condizioni del piano:
    ///
    /// - `bytes` **eccede il permesso**. Significa che il maggiorante con cui
    ///   il chiamante ha prenotato era sbagliato. Non esiste un ripiego
    ///   corretto: rilasciare e riprenotare riaprirebbe esattamente la
    ///   finestra che il permesso esiste per chiudere, quindi si fallisce;
    /// - la **contabilita' e' gia' corrotta**. Nessuna trasformazione di un
    ///   permesso puo' riuscire su un governor che non sa piu' quanto
    ///   trattiene.
    ///
    /// In entrambi i casi il permesso — preso **per valore** — viene
    /// distrutto e la quota torna subito al governor: nulla resta appeso a un
    /// permesso che nessuno tiene piu'.
    pub fn ritaglia(self, bytes: u64) -> Result<MemoryLease> {
        let trattenuti = self.lease.bytes();
        let owner = self.lease.owner().to_owned();
        {
            let mut stato = self.lease.inner.governor.blocca();
            if let Some(motivo) = stato.corrotta {
                drop(stato);
                return Err(contabilita_corrotta(motivo, &owner));
            }
            let Some(resto) = trattenuti.checked_sub(bytes) else {
                drop(stato);
                return Err(PlenoraError::Internal(format!(
                    "ritaglio oltre il permesso di `{owner}`: richiesti {bytes} byte                      su {trattenuti} trattenuti. Il maggiorante con cui e' stato                      prenotato era sbagliato, e non esiste un ripiego corretto:                      rilasciare e riprenotare riaprirebbe la finestra che il                      permesso chiude"
                )));
            };
            let Some(reserved) = stato.reserved.checked_sub(resto) else {
                stato.corrotta = Some("ritaglio oltre la quota contabilizzata");
                drop(stato);
                return Err(contabilita_corrotta(
                    "ritaglio oltre la quota contabilizzata",
                    &owner,
                ));
            };
            // Stessa sezione critica di acquisizioni, rilasci e snapshot: la
            // riduzione del lease e la restituzione del resto avvengono
            // insieme, quindi nessuno puo' osservare un lease che dichiara
            // meno di quanto il governor gli imputa, ne' il contrario.
            self.lease.inner.bytes.store(bytes, Ordering::Release);
            stato.reserved = reserved;
        }
        Ok(self.lease)
    }
}

/// Osservabilita' dei lease (ADR-0002): snapshot del governor nelle metriche
/// di esecuzione. Un riferimento trattenuto e' quota occupata e deve essere
/// diagnosticabile.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct MemoryMetrics {
    /// Budget globale del piano (`max_memory_bytes`).
    pub budget_bytes: u64,
    /// Byte attualmente trattenuti dai lease vivi.
    pub reserved_bytes: u64,
    /// Picco storico dei byte trattenuti.
    pub peak_reserved_bytes: u64,
    /// Lease vivi al momento dello snapshot.
    pub live_leases: u64,
    /// Eta' del lease piu' vecchio (`None` se nessun lease vivo).
    pub oldest_lease_age: Option<Duration>,
    /// `true` se la contabilita' del governor e' stata marcata **incoerente**
    /// in qualunque momento dell'esecuzione.
    ///
    /// Quando e' `true` gli altri campi di questa struttura **non sono
    /// attendibili**: descrivono contatori che hanno smesso di corrispondere
    /// ai lease vivi. `Output::metrics()` e' pubblica e puo' essere letta a
    /// meta' stream, quando l'errore non e' ancora stato consegnato al
    /// chiamante: senza questo campo mostrerebbe numeri apparentemente sani.
    ///
    /// Le cause sono tutte invarianti interne rotte — mai condizioni del
    /// piano: rilascio di quota mai acquisita, id di lease duplicato, nascita
    /// mancante al rilascio, esaurimento dei contatori di servizio.
    pub accounting_corrupted: bool,
}

/// Batch che attraversa il DAG con la sua quota di memoria e la sua sequenza
/// logica (ownership ADR-0002, ordine logico ADR-0001).
///
/// Il wrapper esiste solo AI CONFINI dell'engine (archi, tee,
/// materializzazioni blocking): i kernel restano su `RecordBatch` puro — il
/// batch si spacca in ingresso al segmento e si ricompone in uscita con un
/// lease nuovo (byte dell'output) e la sequenza propagata o riassegnata.
///
/// `Clone` condivide i buffer Arrow e il lease (entrambi reference-counted):
/// il tee di fan-out clona il `GovernedBatch` e la quota resta contata UNA
/// volta fino al rilascio dell'ultimo riferimento.
#[derive(Clone, Debug)]
pub struct GovernedBatch {
    /// Il batch Arrow vero e proprio.
    pub batch: RecordBatch,
    /// Quota di memoria del batch (`None` solo per batch nati fuori dal
    /// perimetro del governor, es. sorgenti di test o wrapper di comodo).
    pub lease: Option<MemoryLease>,
    /// Sequenza logica ADR-0001 (`None` solo fuori dal perimetro).
    pub seq: Option<BatchSequence>,
}

impl GovernedBatch {
    /// Avvolge un batch con lease e sequenza (entrambi opzionali fuori dal
    /// perimetro governato).
    #[must_use]
    pub const fn new(
        batch: RecordBatch,
        lease: Option<MemoryLease>,
        seq: Option<BatchSequence>,
    ) -> Self {
        Self { batch, lease, seq }
    }

    /// Byte contabilizzati: quelli del lease se presente, altrimenti la
    /// stima Arrow puntuale (metadati dei buffer, non le celle).
    #[must_use]
    pub fn accounted_bytes(&self) -> u64 {
        self.lease.as_ref().map_or_else(
            || self.batch.get_array_memory_size() as u64,
            MemoryLease::bytes,
        )
    }

    /// Spacca il wrapper: il kernel lavora sul `RecordBatch` puro, lease e
    /// sequenza restano al confine.
    #[must_use]
    pub fn into_parts(self) -> (RecordBatch, Option<MemoryLease>, Option<BatchSequence>) {
        (self.batch, self.lease, self.seq)
    }

    /// Consegna il batch al chiamante esterno, uscendo dal perimetro del
    /// governor (il lease e' rilasciato qui: la memoria passa al chiamante).
    #[must_use]
    pub fn into_batch(self) -> RecordBatch {
        self.batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Permesso atteso da un governor con contabilita' SANA: `None` significa
    /// budget negato, e un `Err` qui e' un difetto del test o del governor —
    /// non un caso da gestire in linea.
    fn concesso(governor: &MemoryGovernor, bytes: u64, owner: &str) -> Option<MemoryPermit> {
        governor
            .permesso(bytes, owner)
            .expect("contabilita' del governor sana")
    }

    /// Il lease definitivo di un permesso su governor sano.
    fn in_lease(permesso: MemoryPermit) -> MemoryLease {
        permesso.in_lease().expect("contabilita' del governor sana")
    }

    #[test]
    fn lease_releases_bytes_on_last_clone_drop() {
        let governor = MemoryGovernor::new(1_000);
        let lease = governor.reserve(400, "nodo_a").expect("quota disponibile");
        assert_eq!(governor.reserved_bytes(), 400);
        assert_eq!(governor.live_leases(), 1);
        assert_eq!(lease.bytes(), 400);
        assert_eq!(lease.owner(), "nodo_a");

        // Il clone condivide la STESSA quota: mai doppio conteggio.
        let clone = lease.clone();
        assert_eq!(governor.reserved_bytes(), 400);
        assert_eq!(governor.live_leases(), 1);

        drop(lease);
        assert_eq!(governor.reserved_bytes(), 400, "un clone e' ancora vivo");
        assert_eq!(governor.live_leases(), 1);

        drop(clone);
        assert_eq!(governor.reserved_bytes(), 0, "rilascio all'ultimo Drop");
        assert_eq!(governor.live_leases(), 0);
        assert_eq!(governor.peak_reserved_bytes(), 400);
        assert!(governor.oldest_lease_age().is_none());
    }

    #[test]
    fn budget_exhaustion_is_fail_fast_contract() {
        let governor = MemoryGovernor::new(100);
        let lease = governor.reserve(60, "nodo_a").expect("entro budget");
        let error = governor
            .reserve(60, "nodo_b")
            .expect_err("budget esaurito: fail-fast");
        // Nono giro: il budget esaurito e' un limite di RISORSA, non un piano
        // sbagliato. Il piano dichiara un tetto e i dati non ci stanno; la
        // decisione del chiamante e' «rilancia con piu' budget», che e'
        // esattamente cio' che `resource_limit` significa.
        assert!(
            matches!(error, PlenoraError::ResourceLimit(ref reason) if reason.contains("max_memory_bytes")),
            "errore ResourceLimit max_memory_bytes: {error}"
        );
        // Il tentativo fallito non trattiene quota (rollback immediato).
        assert_eq!(governor.reserved_bytes(), 60);

        drop(lease);
        governor
            .reserve(60, "nodo_b")
            .expect("quota liberata dal Drop");
    }

    #[test]
    fn try_reserve_emits_only_granted_in_serial_v1() {
        // Regola v1 documentata su `try_reserve`: l'unico esito emesso e'
        // `Granted`; RetryAfterProgress/MustSpill esistono nell'API per M3+.
        let governor = MemoryGovernor::new(10);
        match governor.try_reserve(10, "nodo").expect("quota piena") {
            ReservationResult::Granted(lease) => {
                assert_eq!(lease.bytes(), 10);
            }
            ReservationResult::RetryAfterProgress | ReservationResult::MustSpill => {
                panic!("la v1 seriale non emette mai RetryAfterProgress/MustSpill");
            }
        }
    }

    #[test]
    fn oldest_lease_age_tracks_live_leases() {
        let governor = MemoryGovernor::new(1_000);
        assert!(governor.oldest_lease_age().is_none());
        let first = governor.reserve(100, "primo").expect("quota");
        let second = governor.reserve(100, "secondo").expect("quota");
        let oldest = governor.oldest_lease_age().expect("due lease vivi");
        assert!(oldest <= first.age(), "il piu' vecchio e' il primo");
        drop(first);
        let remaining = governor.oldest_lease_age().expect("un lease vivo");
        assert!(remaining <= second.age());
        drop(second);
        assert!(governor.oldest_lease_age().is_none());
    }

    #[test]
    fn snapshot_reports_observability_fields() {
        let governor = MemoryGovernor::new(512);
        let lease = governor.reserve(128, "nodo").expect("quota");
        let snapshot = governor.snapshot();
        assert_eq!(snapshot.budget_bytes, 512);
        assert_eq!(snapshot.reserved_bytes, 128);
        assert_eq!(snapshot.peak_reserved_bytes, 128);
        assert_eq!(snapshot.live_leases, 1);
        assert!(snapshot.oldest_lease_age.is_some());
        drop(lease);
        let snapshot = governor.snapshot();
        assert_eq!(snapshot.reserved_bytes, 0);
        assert_eq!(snapshot.live_leases, 0);
        assert_eq!(snapshot.peak_reserved_bytes, 128, "il picco resta storico");
    }

    // -----------------------------------------------------------------
    // Permesso atomico (prerequisito M3)
    // -----------------------------------------------------------------

    #[test]
    fn permesso_al_limite_esatto_e_appena_oltre() {
        let governor = MemoryGovernor::new(1_000);
        let tutto = concesso(&governor, 1_000, "esatto").expect("limite esatto");
        assert_eq!(governor.reserved_bytes(), 1_000);
        assert!(
            concesso(&governor, 1, "oltre").is_none(),
            "un byte oltre il limite non deve essere concesso"
        );
        drop(tutto);
        assert_eq!(governor.reserved_bytes(), 0, "rilascio esatto");
        assert!(concesso(&governor, 1_000, "di nuovo").is_some());
    }

    #[test]
    fn permesso_di_zero_byte_e_valido() {
        let governor = MemoryGovernor::new(0);
        let permesso = concesso(&governor, 0, "nulla").expect("zero byte");
        assert_eq!(permesso.bytes(), 0);
        assert_eq!(governor.reserved_bytes(), 0);
    }

    #[test]
    fn permesso_fail_closed_su_overflow() {
        // Budget massimo: la somma con un secondo permesso enorme non e'
        // rappresentabile in u64. Deve essere negata, non avvolta.
        let governor = MemoryGovernor::new(u64::MAX);
        let primo = concesso(&governor, u64::MAX - 10, "primo").expect("primo");
        assert!(
            concesso(&governor, u64::MAX, "overflow").is_none(),
            "somma non rappresentabile: permesso negato"
        );
        assert_eq!(
            governor.reserved_bytes(),
            u64::MAX - 10,
            "nessuna prenotazione parziale dopo il rifiuto"
        );
        assert!(concesso(&governor, 10, "il resto esatto").is_some());
        drop(primo);
    }

    #[test]
    fn permesso_negato_non_lascia_prenotazioni_parziali() {
        let governor = MemoryGovernor::new(100);
        let tenuto = concesso(&governor, 60, "tenuto").expect("primo");
        for _ in 0..50 {
            assert!(concesso(&governor, 41, "troppo").is_none());
        }
        assert_eq!(
            governor.reserved_bytes(),
            60,
            "cinquanta rifiuti non devono spostare il contatore di un byte"
        );
        assert_eq!(governor.live_leases(), 1, "nessun lease dai rifiuti");
        drop(tenuto);
        assert_eq!(governor.reserved_bytes(), 0);
        assert_eq!(governor.live_leases(), 0);
    }

    #[test]
    fn ritaglio_restituisce_solo_la_differenza() {
        let governor = MemoryGovernor::new(1_000);
        let permesso = concesso(&governor, 800, "maggiorante").expect("permesso");
        assert_eq!(governor.reserved_bytes(), 800);
        let lease = permesso.ritaglia(120).expect("ritaglio entro il permesso");
        assert_eq!(lease.bytes(), 120, "il lease dichiara la dimensione reale");
        assert_eq!(
            governor.reserved_bytes(),
            120,
            "la differenza torna subito al governor"
        );
        assert_eq!(governor.live_leases(), 1, "resta UN solo lease");
        drop(lease);
        assert_eq!(governor.reserved_bytes(), 0);
        assert_eq!(governor.live_leases(), 0);
    }

    #[test]
    fn ritaglio_oltre_il_permesso_e_negato() {
        let governor = MemoryGovernor::new(1_000);
        let permesso = concesso(&governor, 100, "piccolo").expect("permesso");
        let errore = permesso
            .ritaglia(101)
            .expect_err("non si ritaglia piu' di quanto si trattiene");
        assert!(
            matches!(errore, PlenoraError::Internal(_)),
            "un ritaglio impossibile e' un'invariante rotta, non un limite del              piano: {errore:?}"
        );
        // Il tentativo consuma il permesso: la quota torna al governor
        // invece di restare appesa a un permesso che nessuno tiene piu'.
        assert_eq!(governor.reserved_bytes(), 0);
        assert_eq!(governor.live_leases(), 0);
    }

    #[test]
    fn ritaglio_a_zero_e_valido() {
        let governor = MemoryGovernor::new(1_000);
        let permesso = concesso(&governor, 500, "maggiorante").expect("permesso");
        let lease = permesso.ritaglia(0).expect("ritaglio nullo");
        assert_eq!(lease.bytes(), 0);
        assert_eq!(governor.reserved_bytes(), 0);
        assert_eq!(governor.live_leases(), 1, "il lease vive, pur a zero byte");
        drop(lease);
        assert_eq!(governor.live_leases(), 0);
    }

    #[test]
    fn permesso_rilasciato_lungo_un_unwind() {
        // Il rilascio non dipende da un cammino di ritorno: il `Drop` corre
        // anche mentre un panico risale. E' il caso di errore e di
        // cancellazione.
        let governor = MemoryGovernor::new(1_000);
        let clone = governor.clone();
        let esito = std::panic::catch_unwind(move || {
            let _permesso = clone.permesso(700, "panico").expect("permesso");
            assert_eq!(clone.reserved_bytes(), 700);
            panic!("errore simulato");
        });
        assert!(esito.is_err(), "il panico deve propagarsi");
        assert_eq!(
            governor.reserved_bytes(),
            0,
            "la quota torna al governor anche lungo un unwind"
        );
        assert_eq!(governor.live_leases(), 0);
    }

    #[test]
    fn permessi_concorrenti_non_superano_mai_il_limite() {
        // Partenza sincronizzata: tutti i thread attendono la stessa
        // barriera, poi chiedono insieme. I permessi concessi restano VIVI
        // fino alla join, quindi la somma osservata e' davvero simultanea: se
        // verifica e prenotazione fossero due operazioni, qui si vedrebbe.
        use std::sync::Barrier;

        const THREAD: usize = 16;
        const PER_PERMESSO: u64 = 64;
        const CONCEDIBILI: usize = 10;
        let governor = MemoryGovernor::new(PER_PERMESSO * CONCEDIBILI as u64);
        let barriera = Arc::new(Barrier::new(THREAD));
        let massimo = Arc::new(AtomicU64::new(0));

        let permessi: Vec<Option<MemoryPermit>> = std::thread::scope(|scope| {
            // Il `collect` e' NECESSARIO: senza, i thread verrebbero avviati e
            // uniti uno alla volta e la concorrenza — cioe' l'oggetto del
            // test — sparirebbe. Tutti devono essere in volo insieme.
            #[allow(clippy::needless_collect)]
            let mani: Vec<_> = (0..THREAD)
                .map(|_| {
                    let governor = governor.clone();
                    let barriera = Arc::clone(&barriera);
                    let massimo = Arc::clone(&massimo);
                    scope.spawn(move || {
                        barriera.wait();
                        let permesso = concesso(&governor, PER_PERMESSO, "concorrente");
                        // Osservato MENTRE i permessi sono vivi: e' qui che un
                        // eccesso si manifesterebbe.
                        massimo.fetch_max(governor.reserved_bytes(), Ordering::AcqRel);
                        permesso
                    })
                })
                .collect();
            mani.into_iter()
                .map(|mano| mano.join().expect("thread senza panico"))
                .collect()
        });

        let concessi = permessi.iter().filter(|p| p.is_some()).count();
        assert_eq!(
            concessi, CONCEDIBILI,
            "concessi esattamente quelli che il budget copre, tutti vivi insieme"
        );
        assert_eq!(
            governor.reserved_bytes(),
            PER_PERMESSO * CONCEDIBILI as u64,
            "il budget e' esattamente saturo"
        );
        assert!(
            massimo.load(Ordering::Acquire) <= governor.budget_bytes(),
            "il totale riservato ha superato il budget: {} > {}",
            massimo.load(Ordering::Acquire),
            governor.budget_bytes()
        );
        drop(permessi);
        assert_eq!(governor.reserved_bytes(), 0, "tutto rilasciato");
        assert_eq!(governor.live_leases(), 0);
    }

    #[test]
    fn permessi_concorrenti_con_dimensioni_diverse() {
        // Stessa barriera, taglie diverse: il budget non deve essere superato
        // qualunque combinazione riesca a passare.
        use std::sync::Barrier;

        const THREAD: usize = 24;
        let governor = MemoryGovernor::new(1_000);
        let barriera = Arc::new(Barrier::new(THREAD));
        let massimo = Arc::new(AtomicU64::new(0));
        std::thread::scope(|scope| {
            for indice in 0..THREAD {
                let governor = governor.clone();
                let barriera = Arc::clone(&barriera);
                let massimo = Arc::clone(&massimo);
                scope.spawn(move || {
                    let taglia = 17 * (indice as u64 % 7 + 1);
                    barriera.wait();
                    for _ in 0..200 {
                        if let Some(permesso) = concesso(&governor, taglia, "misto") {
                            massimo.fetch_max(governor.reserved_bytes(), Ordering::AcqRel);
                            drop(permesso);
                        }
                    }
                });
            }
        });
        assert!(
            massimo.load(Ordering::Acquire) <= 1_000,
            "budget superato in concorso: {}",
            massimo.load(Ordering::Acquire)
        );
        assert_eq!(governor.reserved_bytes(), 0);
        assert_eq!(governor.live_leases(), 0);
    }

    #[test]
    fn ritaglio_concorrente_non_supera_il_limite() {
        // Il ritaglio abbassa il conteggio: se lo facesse ri-prenotando, qui
        // si aprirebbe una finestra in cui due thread tengono insieme piu'
        // del budget.
        use std::sync::Barrier;

        const THREAD: usize = 12;
        let governor = MemoryGovernor::new(THREAD as u64 * 100);
        let barriera = Arc::new(Barrier::new(THREAD));
        let massimo = Arc::new(AtomicU64::new(0));
        std::thread::scope(|scope| {
            for _ in 0..THREAD {
                let governor = governor.clone();
                let barriera = Arc::clone(&barriera);
                let massimo = Arc::clone(&massimo);
                scope.spawn(move || {
                    barriera.wait();
                    for _ in 0..100 {
                        if let Some(permesso) = concesso(&governor, 100, "maggiorante") {
                            massimo.fetch_max(governor.reserved_bytes(), Ordering::AcqRel);
                            let lease = permesso.ritaglia(10).expect("ritaglio valido");
                            massimo.fetch_max(governor.reserved_bytes(), Ordering::AcqRel);
                            drop(lease);
                        }
                    }
                });
            }
        });
        assert!(
            massimo.load(Ordering::Acquire) <= governor.budget_bytes(),
            "budget superato durante i ritagli: {}",
            massimo.load(Ordering::Acquire)
        );
        assert_eq!(governor.reserved_bytes(), 0);
        assert_eq!(governor.live_leases(), 0);
    }

    #[test]
    fn try_reserve_e_reserve_passano_dal_permesso() {
        // Non due contabilita': `reserve` e' un involucro, e i suoi effetti
        // sui contatori sono quelli del permesso.
        let governor = MemoryGovernor::new(100);
        let lease = governor.reserve(100, "tutto").expect("limite esatto");
        assert_eq!(governor.reserved_bytes(), 100);
        let errore = governor.reserve(1, "oltre").expect_err("budget esaurito");
        assert!(
            matches!(errore, PlenoraError::ResourceLimit(_)),
            "errore atteso ResourceLimit: {errore:?}"
        );
        assert_eq!(
            governor.reserved_bytes(),
            100,
            "il rifiuto non sposta il contatore"
        );
        drop(lease);
        assert_eq!(governor.reserved_bytes(), 0);
    }

    // -----------------------------------------------------------------
    // Residui della seconda lettura indipendente
    // -----------------------------------------------------------------

    #[test]
    fn eta_del_piu_vecchio_e_il_massimo_delle_eta_vive() {
        // Deterministico e senza `sleep`: le eta' crescono monotonamente, quindi
        // il valore riportato dal governor deve stare FRA il massimo letto
        // prima e il massimo letto dopo. E' il modo di fissare "il piu'
        // vecchio" senza dipendere da un ritardo che su una macchina carica
        // puo' non essere quello che il test crede.
        let governor = MemoryGovernor::new(1_000);
        let lease: Vec<MemoryLease> = (0..8)
            .map(|indice| {
                in_lease(
                    concesso(&governor, 10, &format!("lease-{indice}")).expect("quota disponibile"),
                )
            })
            .collect();

        let prima = lease
            .iter()
            .map(MemoryLease::age)
            .max()
            .expect("almeno un lease");
        let riportata = governor.oldest_lease_age().expect("lease vivi");
        let dopo = lease
            .iter()
            .map(MemoryLease::age)
            .max()
            .expect("almeno un lease");

        assert!(
            riportata >= prima,
            "il governor riporta un'eta' minore del massimo gia' osservato: \
             {riportata:?} < {prima:?}"
        );
        assert!(
            riportata <= dopo,
            "il governor riporta un'eta' maggiore del massimo osservabile: \
             {riportata:?} > {dopo:?}"
        );

        // Caduto il piu' vecchio, il riportato deve seguire il secondo. Si usa
        // di nuovo il sandwich, e NON un confronto con l'eta' del caduto letta
        // prima: fra le due letture passa del tempo, e se supera la differenza
        // di nascita fra primo e secondo — che e' di microsecondi — il
        // confronto fallisce senza che nulla sia rotto. E' un difetto che
        // questo test ha avuto davvero, trovato martellandolo quaranta volte.
        let mut lease = lease;
        drop(lease.remove(0));
        let prima = lease
            .iter()
            .map(MemoryLease::age)
            .max()
            .expect("almeno un lease");
        let riportata = governor.oldest_lease_age().expect("lease vivi");
        let dopo = lease
            .iter()
            .map(MemoryLease::age)
            .max()
            .expect("almeno un lease");
        assert!(
            riportata >= prima && riportata <= dopo,
            "dopo la caduta del piu' vecchio l'eta' riportata non segue il              secondo: {riportata:?} fuori da [{prima:?}, {dopo:?}]"
        );
        drop(lease);
        assert!(governor.oldest_lease_age().is_none());
    }

    #[test]
    fn esaurimento_degli_id_e_errore_interno_non_diniego_di_budget() {
        // Un diniego di budget e un errore di contabilita' devono restare
        // distinti: il primo si gestisce (per esempio passando al disco), il
        // secondo no. Qui il budget e' ampio e la richiesta piccola: se il
        // governor rispondesse `Ok(None)` manderebbe il chiamante a cercare
        // spazio che c'e'.
        let governor = MemoryGovernor::new(1_000_000);
        governor.forza_next_id(u64::MAX);
        let errore = governor
            .permesso(10, "dopo l'esaurimento")
            .expect_err("id esauriti: errore interno");
        assert!(
            matches!(errore, PlenoraError::Internal(_)),
            "atteso Internal, non un diniego di budget: {errore:?}"
        );
        assert_eq!(
            governor.reserved_bytes(),
            0,
            "nulla deve essere prenotato quando la richiesta fallisce"
        );
        assert_eq!(governor.live_leases(), 0);
        assert!(governor.e_corrotta(), "la contabilita' resta marcata");
        // Fail-closed e irreversibile: nessuna richiesta successiva passa.
        assert!(governor.permesso(1, "ancora").is_err());
        assert!(governor.reserve(1, "ancora").is_err());
    }

    #[test]
    fn esaurimento_del_contatore_dei_lease_vivi_e_errore_interno() {
        let governor = MemoryGovernor::new(1_000_000);
        governor.forza_live(u64::MAX);
        let errore = governor
            .permesso(10, "lease esauriti")
            .expect_err("contatore dei lease esaurito");
        assert!(matches!(errore, PlenoraError::Internal(_)), "{errore:?}");
        assert_eq!(
            governor.reserved_bytes(),
            0,
            "nessuna prenotazione parziale"
        );
    }

    #[test]
    fn overflow_dei_byte_resta_un_diniego_non_una_corruzione() {
        // La somma dei byte non rappresentabile e' indistinguibile, per il
        // chiamante, da un budget insufficiente: in entrambi i casi non c'e'
        // quota. Marcarla come corruzione bloccherebbe un governor sano.
        let governor = MemoryGovernor::new(u64::MAX);
        let primo = concesso(&governor, u64::MAX - 10, "primo").expect("primo");
        assert!(
            concesso(&governor, u64::MAX, "overflow").is_none(),
            "atteso diniego"
        );
        assert!(
            !governor.e_corrotta(),
            "un overflow di byte non e' una contabilita' rotta"
        );
        assert!(concesso(&governor, 10, "il resto esatto").is_some());
        drop(primo);
    }

    #[test]
    fn snapshot_coerente_sotto_acquisizioni_concorrenti() {
        // Linearizzabilita': ogni snapshot deve descrivere uno stato
        // realmente esistito. Con contatori separati si potrebbe osservare
        // `live_leases > 0` e nessuna nascita registrata — o il contrario —
        // perche' i due aggiornamenti non sarebbero simultanei.
        //
        // Deterministico nel senso che conta: nessun `sleep`, nessuna
        // dipendenza da un ordine. Le invarianti valgono a OGNI lettura,
        // qualunque intreccio si realizzi.
        use std::sync::atomic::AtomicBool;
        use std::sync::Barrier;

        const THREAD: usize = 8;
        const GIRI: usize = 2_000;
        let governor = MemoryGovernor::new(THREAD as u64 * 64);
        let barriera = Arc::new(Barrier::new(THREAD + 1));
        let finito = Arc::new(AtomicBool::new(false));

        std::thread::scope(|scope| {
            for _ in 0..THREAD {
                let governor = governor.clone();
                let barriera = Arc::clone(&barriera);
                scope.spawn(move || {
                    barriera.wait();
                    for _ in 0..GIRI {
                        if let Ok(Some(permesso)) = governor.permesso(64, "concorrente") {
                            drop(permesso);
                        }
                    }
                });
            }
            barriera.wait();
            let mut letture = 0_u64;
            while !finito.load(Ordering::Acquire) && letture < 50_000 {
                let s = governor.snapshot();
                letture += 1;
                assert!(
                    s.reserved_bytes <= s.budget_bytes,
                    "snapshot fuori budget: {} > {}",
                    s.reserved_bytes,
                    s.budget_bytes
                );
                assert!(
                    s.peak_reserved_bytes >= s.reserved_bytes,
                    "picco minore del riservato: {} < {}",
                    s.peak_reserved_bytes,
                    s.reserved_bytes
                );
                // L'invariante che con contatori separati si romperebbe: o ci
                // sono lease vivi E una nascita registrata, o nessuno dei due.
                assert_eq!(
                    s.live_leases > 0,
                    s.oldest_lease_age.is_some(),
                    "snapshot incoerente: {} lease vivi ma nascita registrata = {}",
                    s.live_leases,
                    s.oldest_lease_age.is_some()
                );
            }
            finito.store(true, Ordering::Release);
        });

        assert_eq!(governor.reserved_bytes(), 0);
        assert_eq!(governor.live_leases(), 0);
        assert!(governor.oldest_lease_age().is_none());
        assert!(!governor.e_corrotta(), "nessuna corruzione sotto contesa");
    }

    #[test]
    fn rilascio_concorrente_non_marca_mai_la_contabilita_corrotta() {
        // I decrementi sono controllati: se il rilascio restituisse piu' di
        // quanto preso, la contabilita' verrebbe marcata. Sotto migliaia di
        // acquisizioni e rilasci intrecciati non deve succedere mai.
        use std::sync::Barrier;

        const THREAD: usize = 8;
        let governor = MemoryGovernor::new(4_096);
        let barriera = Arc::new(Barrier::new(THREAD));
        std::thread::scope(|scope| {
            for indice in 0..THREAD {
                let governor = governor.clone();
                let barriera = Arc::clone(&barriera);
                scope.spawn(move || {
                    let taglia = 8 * (indice as u64 % 5 + 1);
                    barriera.wait();
                    for _ in 0..1_000 {
                        if let Ok(Some(permesso)) = governor.permesso(taglia, "misto") {
                            // Meta' consegnati come lease, meta' ritagliati:
                            // entrambe le strade passano dagli stessi
                            // decrementi controllati.
                            if taglia.is_multiple_of(16) {
                                drop(permesso.in_lease().expect("governor sano"));
                            } else if let Ok(lease) = permesso.ritaglia(taglia / 2) {
                                drop(lease);
                            }
                        }
                    }
                });
            }
        });
        assert!(!governor.e_corrotta(), "contabilita' marcata sotto contesa");
        assert_eq!(governor.reserved_bytes(), 0, "tutto restituito");
        assert_eq!(governor.live_leases(), 0);
    }

    // -----------------------------------------------------------------
    // Il fallback eliminato, e la corruzione che non deve passare
    // -----------------------------------------------------------------

    #[test]
    fn il_vecchio_fallback_del_ritaglio_non_esiste_piu() {
        // Prima, un ritaglio impossibile ripiegava su una nuova `reserve`:
        // rilascio e riprenotazione, cioe' proprio la finestra che il permesso
        // esiste per chiudere. Ora fallisce, e il messaggio dice perche'.
        let governor = MemoryGovernor::new(1_000_000);
        let permesso = concesso(&governor, 100, "maggiorante sbagliato").expect("permesso");
        let errore = permesso
            .ritaglia(101)
            .expect_err("il ritaglio oltre il permesso deve fallire");
        let testo = errore.to_string();
        assert!(
            matches!(errore, PlenoraError::Internal(_)),
            "atteso Internal: {errore:?}"
        );
        assert!(
            testo.contains("ritaglio oltre il permesso"),
            "il messaggio deve dire che il maggiorante era sbagliato: {testo}"
        );
        // E soprattutto: il budget e' tornato libero, non e' stato ri-preso.
        assert_eq!(governor.reserved_bytes(), 0);
        assert_eq!(governor.live_leases(), 0);
        assert!(
            !governor.e_corrotta(),
            "una stima sbagliata del chiamante non corrompe la contabilita'"
        );
    }

    #[test]
    fn governor_corrotto_prima_del_ritaglio_non_concede_nulla() {
        // La corruzione puo' emergere fra l'acquisizione del permesso e il
        // suo uso — per esempio dal `Drop` di un altro lease. Da quel momento
        // NESSUNA trasformazione del permesso deve riuscire.
        let governor = MemoryGovernor::new(1_000);
        let permesso = concesso(&governor, 500, "prima").expect("permesso");
        governor.corrompi_per_test("corruzione simulata");
        let errore = permesso
            .ritaglia(100)
            .expect_err("ritaglio su governor corrotto");
        assert!(matches!(errore, PlenoraError::Internal(_)), "{errore:?}");

        // Anche `in_lease`, che non tocca i contatori, deve rifiutare.
        let governor = MemoryGovernor::new(1_000);
        let permesso = concesso(&governor, 500, "prima").expect("permesso");
        governor.corrompi_per_test("corruzione simulata");
        let errore = permesso
            .in_lease()
            .expect_err("in_lease su governor corrotto");
        assert!(matches!(errore, PlenoraError::Internal(_)), "{errore:?}");
    }

    #[test]
    fn nascita_mancante_al_rilascio_marca_la_contabilita() {
        // Biiezione `live == births.len()`: se la nascita sparisce, il
        // rilascio se ne accorge. Il `Drop` non puo' restituire un errore, ma
        // marca — e il controllo di salute lo intercetta prima di dichiarare
        // conclusa l'esecuzione.
        let governor = MemoryGovernor::new(1_000);
        let lease = in_lease(concesso(&governor, 100, "vittima").expect("permesso"));
        governor.rimuovi_nascita_per_test();
        assert!(
            !governor.e_corrotta(),
            "finche' il lease vive non c'e' ancora nulla di rotto"
        );
        drop(lease);
        assert!(
            governor.e_corrotta(),
            "il rilascio di un lease senza nascita registrata deve marcare"
        );
        assert!(governor.verifica_salute("test").is_err());
    }

    #[test]
    fn id_duplicato_marca_la_contabilita_e_non_prenota() {
        // L'altro verso della biiezione: due lease con la stessa identita'
        // farebbero perdere una voce alla mappa senza che alcun contatore se
        // ne accorga.
        let governor = MemoryGovernor::new(1_000);
        let primo = in_lease(concesso(&governor, 100, "primo").expect("permesso"));
        let riservati = governor.reserved_bytes();
        // Il prossimo id sara' quello gia' vivo.
        governor.forza_next_id(0);
        let errore = governor
            .permesso(100, "collisione")
            .expect_err("id gia' presente");
        assert!(matches!(errore, PlenoraError::Internal(_)), "{errore:?}");
        assert!(governor.e_corrotta());
        assert_eq!(
            governor.reserved_bytes(),
            riservati,
            "una collisione non deve lasciare byte prenotati"
        );
        drop(primo);
    }

    #[test]
    fn verifica_salute_e_il_cancello_prima_di_dichiarare_successo() {
        let governor = MemoryGovernor::new(1_000);
        assert!(governor.verifica_salute("output").is_ok());
        governor.corrompi_per_test("corruzione simulata");
        let errore = governor
            .verifica_salute("output")
            .expect_err("governor corrotto");
        assert!(matches!(errore, PlenoraError::Internal(_)), "{errore:?}");
        assert!(
            errore.to_string().contains("contabilita'"),
            "il messaggio deve dire che e' un'invariante interna: {errore}"
        );
    }
}
