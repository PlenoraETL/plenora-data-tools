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
    bytes: u64,
    /// Nodo/arco che ha acquisito la quota (osservabilita' ADR-0002).
    owner: String,
    created: Instant,
}

impl Drop for LeaseInner {
    fn drop(&mut self) {
        // Mai panic in Drop: un mutex avvelenato durante uno unwind non deve
        // abortire il processo — il rilascio della quota resta garantito.
        self.governor.reserved_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        self.governor.live_leases.fetch_sub(1, Ordering::AcqRel);
        let mut births = self
            .governor
            .lease_births
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        births.remove(&self.id);
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
        self.inner.bytes
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

/// Contabilita' condivisa tra il governor e i lease vivi. `Arc` + atomici:
/// la stessa contabilita' servira' i worker paralleli (M3) senza cambiare
/// l'API; in v1 seriale la contesa e' nulla e il costo e' una manciata di
/// operazioni atomiche per batch, mai per riga.
#[derive(Debug)]
struct GovernorShared {
    budget: u64,
    reserved_bytes: AtomicU64,
    peak_reserved_bytes: AtomicU64,
    live_leases: AtomicU64,
    next_lease_id: AtomicU64,
    /// Istanti di creazione dei lease vivi per id (monotono come il tempo):
    /// la prima entry e' il lease piu' vecchio (osservabilita' ADR-0002).
    lease_births: Mutex<BTreeMap<u64, Instant>>,
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
                reserved_bytes: AtomicU64::new(0),
                peak_reserved_bytes: AtomicU64::new(0),
                live_leases: AtomicU64::new(0),
                next_lease_id: AtomicU64::new(0),
                lease_births: Mutex::new(BTreeMap::new()),
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
        self.shared.reserved_bytes.load(Ordering::Acquire)
    }

    /// Picco storico dei byte trattenuti.
    #[must_use]
    pub fn peak_reserved_bytes(&self) -> u64 {
        self.shared.peak_reserved_bytes.load(Ordering::Acquire)
    }

    /// Numero di lease vivi.
    #[must_use]
    pub fn live_leases(&self) -> u64 {
        self.shared.live_leases.load(Ordering::Acquire)
    }

    /// Eta' del lease piu' vecchio (`None` se non ci sono lease vivi).
    #[must_use]
    pub fn oldest_lease_age(&self) -> Option<Duration> {
        let births = self
            .shared
            .lease_births
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        births.values().next().map(Instant::elapsed)
    }

    /// Snapshot di osservabilita' ADR-0002 per le metriche di esecuzione.
    #[must_use]
    pub fn snapshot(&self) -> MemoryMetrics {
        MemoryMetrics {
            budget_bytes: self.shared.budget,
            reserved_bytes: self.reserved_bytes(),
            peak_reserved_bytes: self.peak_reserved_bytes(),
            live_leases: self.live_leases(),
            oldest_lease_age: self.oldest_lease_age(),
        }
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
    /// strategia sicura disponibile": errore `Contract` `max_memory_bytes`.
    /// Per questo `RetryAfterProgress` e `MustSpill` esistono nell'API ma
    /// non sono MAI emessi da questa implementazione.
    ///
    /// Costo: poche operazioni atomiche per batch, mai per riga, nessun
    /// riconteggio ricorsivo dei buffer (i byte li fissa il chiamante al
    /// confine di batch).
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` se il budget residuo non copre `bytes`
    /// (fail-fast v1, vedi sopra).
    pub fn try_reserve(&self, bytes: u64, owner: &str) -> Result<ReservationResult> {
        // Prenotazione atomica add-e-controlla con rollback immediato: in v1
        // seriale un solo produttore alla volta tira lo stream, ma la forma
        // resta corretta anche in concorso (la quota in eccesso e'
        // restituita subito) — pronta per M3 senza cambi di API.
        let reserved = self.shared.reserved_bytes.fetch_add(bytes, Ordering::AcqRel) + bytes;
        if reserved > self.shared.budget {
            self.shared.reserved_bytes.fetch_sub(bytes, Ordering::AcqRel);
            return Err(PlenoraError::InvalidPlan(format!(
                "max_memory_bytes superato: `{owner}` richiede {bytes} byte, \
                 {} gia' riservati su un budget di {}",
                reserved - bytes,
                self.shared.budget
            )));
        }
        self.shared
            .peak_reserved_bytes
            .fetch_max(reserved, Ordering::AcqRel);
        self.shared.live_leases.fetch_add(1, Ordering::AcqRel);
        let id = self.shared.next_lease_id.fetch_add(1, Ordering::AcqRel);
        let created = Instant::now();
        {
            let mut births = self
                .shared
                .lease_births
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            births.insert(id, created);
        }
        Ok(ReservationResult::Granted(MemoryLease {
            inner: Arc::new(LeaseInner {
                governor: Arc::clone(&self.shared),
                id,
                bytes,
                owner: owner.to_owned(),
                created,
            }),
        }))
    }

    /// Acquisizione v1: il lease, o l'errore `Contract` fail-fast se il
    /// budget e' esaurito (regola in [`Self::try_reserve`]).
    ///
    /// # Errors
    ///
    /// Come [`Self::try_reserve`].
    pub fn reserve(&self, bytes: u64, owner: &str) -> Result<MemoryLease> {
        match self.try_reserve(bytes, owner)? {
            ReservationResult::Granted(lease) => Ok(lease),
            // Mai emessi dalla v1 (vedi `try_reserve`); mappati comunque a
            // fail-fast per difesa — mai `unreachable!` su esiti futuri.
            ReservationResult::RetryAfterProgress | ReservationResult::MustSpill => Err(
                PlenoraError::InvalidPlan(format!(
                    "max_memory_bytes: esito di reservation non attuabile in v1 per `{owner}`"
                )),
            ),
        }
    }
}

/// Osservabilita' dei lease (ADR-0002): snapshot del governor nelle metriche
/// di esecuzione. Un riferimento trattenuto e' quota occupata e deve essere
/// diagnosticabile.
#[derive(Clone, Debug, Default)]
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
        assert!(
            matches!(error, PlenoraError::InvalidPlan(ref reason) if reason.contains("max_memory_bytes")),
            "errore Contract max_memory_bytes: {error}"
        );
        // Il tentativo fallito non trattiene quota (rollback immediato).
        assert_eq!(governor.reserved_bytes(), 60);

        drop(lease);
        governor.reserve(60, "nodo_b").expect("quota liberata dal Drop");
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
}
