//! Scadenza, annullamento e lavoro fuori dal thread del chiamante.
//!
//! Il lavoro (lettura dei file, validazione, runner, scrittura) gira in un
//! thread suo, senza il GIL. Il thread del chiamante aspetta a intervalli di
//! [`INTERVALLO`] senza il GIL e a ogni intervallo:
//!
//! - porta nel segnale del runner i `CancellationToken` del chiamante;
//! - chiama `PyErr_CheckSignals` (`Python::check_signals`): sul thread
//!   principale un Ctrl-C diventa lì `KeyboardInterrupt`; il segnale del
//!   runner si alza, si aspetta che il lavoro si fermi al suo controllo
//!   successivo, e il `KeyboardInterrupt` esce con l'esito del lavoro come
//!   `__cause__` (gli assi veri, anche `partial` o `committed`).
//!
//! I gettoni si portano nel segnale anche prima di far partire il lavoro (un
//! gettone già alzato non lascia leggere né scrivere nulla) e alla consegna:
//! un annullamento arrivato mentre il lavoro finiva diventa `cancelled` in
//! fase `finalize`, con effetto `committed` se gli output sono stati
//! scritti, mai un successo.
//!
//! La scadenza è l'`Interruzione` del runner, la stessa della CLI: fissata
//! all'ingresso della chiamata e controllata fra le letture, i passi e le
//! scritture.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use plenora_core::{ErrorPhase, PlenoraError, RemoteEffect};
use plenora_pipeline::Interruzione;
use pyo3::prelude::*;

use crate::errori::{in_python, panico, Errore};

/// Ogni quanto il thread del chiamante guarda gettoni e segnali.
const INTERVALLO: Duration = Duration::from_millis(10);

/// Scadenza e segnali di una chiamata.
pub struct Controlli {
    /// Scadenza e segnale che il lavoro controlla.
    pub interruzione: Interruzione,
    /// Il segnale del runner, alzato da un gettone o da un'eccezione.
    segnale: Arc<AtomicBool>,
    /// I `CancellationToken` del chiamante.
    gettoni: Vec<Arc<AtomicBool>>,
}

/// Un argomento rifiutato: fase `prepare`, quella che `PlenoraError`
/// deriva per la configurazione e che la CLI dà ai suoi argomenti.
fn configurazione(motivo: &str) -> PlenoraError {
    PlenoraError::InvalidConfiguration(motivo.to_owned())
}

/// Una durata in secondi da un `float` di Python: finita e non negativa.
fn durata(secondi: f64, nome: &str) -> Result<Duration, PlenoraError> {
    Duration::try_from_secs_f64(secondi).map_err(|_| {
        configurazione(&format!(
            "`{nome}`: atteso un numero finito di secondi, non negativo e \
             rappresentabile"
        ))
    })
}

impl Controlli {
    /// I controlli di una chiamata. `scadenza` è un istante in secondi
    /// dall'epoca Unix (`datetime.timestamp()`), `timeout` i secondi che
    /// restano da adesso alla scadenza fissata quando la chiamata pubblica è
    /// cominciata (il pacchetto Python la fissa sul clock monotono prima di
    /// mettere in coda una chiamata asincrona): uno solo dei due, come
    /// `--deadline` e `--timeout-ms` della CLI. Una scadenza già passata
    /// scade al primo controllo.
    ///
    /// # Errors
    ///
    /// `InvalidConfiguration` (fase `prepare`) per entrambi, per un valore
    /// non finito o negativo, o per un istante che il clock monotono non
    /// rappresenta.
    pub fn nuovi(
        scadenza: Option<f64>,
        timeout: Option<f64>,
        gettoni: Vec<Arc<AtomicBool>>,
    ) -> Result<Self, PlenoraError> {
        let adesso = Instant::now();
        let fuori_scala = |nome: &str| {
            configurazione(&format!(
                "`{nome}`: oltre quanto il clock monotono rappresenta"
            ))
        };
        let scadenza = match (scadenza, timeout) {
            (Some(_), Some(_)) => {
                return Err(configurazione("una sola fra `deadline` e `timeout`"));
            }
            (None, None) => None,
            (None, Some(secondi)) => Some(
                adesso
                    .checked_add(durata(secondi, "timeout")?)
                    .ok_or_else(|| fuori_scala("timeout"))?,
            ),
            (Some(epoca), None) => {
                if !epoca.is_finite() {
                    return Err(configurazione(
                        "`deadline`: atteso un istante finito (datetime con fuso)",
                    ));
                }
                let istante = if epoca >= 0.0 {
                    SystemTime::UNIX_EPOCH.checked_add(durata(epoca, "deadline")?)
                } else {
                    SystemTime::UNIX_EPOCH.checked_sub(durata(-epoca, "deadline")?)
                }
                .ok_or_else(|| fuori_scala("deadline"))?;
                // Già passata: scade al primo controllo, senza aritmetica
                // all'indietro sull'`Instant`.
                let restante = istante
                    .duration_since(SystemTime::now())
                    .unwrap_or(Duration::ZERO);
                Some(
                    adesso
                        .checked_add(restante)
                        .ok_or_else(|| fuori_scala("deadline"))?,
                )
            }
        };
        let segnale = Arc::new(AtomicBool::new(false));
        Ok(Self {
            interruzione: Interruzione {
                scadenza,
                annullamento: Some(Arc::clone(&segnale)),
            },
            segnale,
            gettoni,
        })
    }

    /// Porta i gettoni nel segnale e guarda i segnali Python. Va chiamata
    /// con il GIL, dal thread del chiamante.
    ///
    /// # Errors
    ///
    /// L'eccezione di un gestore di segnale Python (`KeyboardInterrupt`),
    /// dopo aver alzato il segnale del runner.
    fn sorveglia(&self, py: Python<'_>) -> PyResult<()> {
        self.trasferisci();
        py.check_signals().inspect_err(|_| {
            self.segnale.store(true, Ordering::Release);
        })
    }

    /// Porta nel segnale del runner un gettone già alzato. Si chiama prima
    /// di far partire il lavoro (un gettone alzato prima della chiamata non
    /// lascia leggere né scrivere nulla) e a ogni intervallo.
    fn trasferisci(&self) {
        if self
            .gettoni
            .iter()
            .any(|gettone| gettone.load(Ordering::Acquire))
        {
            self.segnale.store(true, Ordering::Release);
        }
    }

    /// L'annullamento visto alla consegna, dopo che il lavoro è finito con
    /// successo: `cancelled` in fase `finalize`, con l'effetto vero
    /// (`committed` se il lavoro ha scritto i suoi output), mai un successo
    /// taciuto né un effetto perso.
    fn annullato_alla_consegna(con_effetti: bool) -> PlenoraError {
        let errore = PlenoraError::Cancelled(
            "esecuzione annullata alla consegna: il lavoro era finito, il risultato \
             non si consegna"
                .to_owned(),
        )
        .with_phase(ErrorPhase::Finalize);
        if con_effetti {
            errore.with_remote_effect(RemoteEffect::Committed)
        } else {
            errore
        }
    }

    /// Un punto di controllo sul thread del chiamante (fra un blocco Arrow e
    /// l'altro): segnali Python, gettoni, scadenza.
    ///
    /// # Errors
    ///
    /// [`Errore::Python`] per un'eccezione di segnale; `Cancelled` o
    /// `Timeout` nella `fase` data.
    pub fn verifica(&self, py: Python<'_>, dove: &str, fase: ErrorPhase) -> Result<(), Errore> {
        self.sorveglia(py).map_err(Errore::Python)?;
        self.interruzione
            .verifica(dove)
            .map_err(|errore| Errore::Plenora(errore.with_phase(fase)))
    }

    /// Esegue `lavoro` in un thread suo e aspetta l'esito senza il GIL,
    /// sorvegliando gettoni e segnali (doc del modulo).
    ///
    /// Un panico del lavoro diventa `internal` senza payload, con effetto
    /// `unknown` se `con_effetti` (l'operazione può aver scritto file).
    ///
    /// # Errors
    ///
    /// L'errore del lavoro; [`Errore::Python`] per un'eccezione di segnale
    /// arrivata mentre si aspettava, con l'esito del lavoro come causa;
    /// `internal` se il thread non parte.
    pub fn esegui<T: Send + 'static>(
        &self,
        py: Python<'_>,
        con_effetti: bool,
        lavoro: impl FnOnce(&Interruzione) -> Result<T, PlenoraError> + Send + 'static,
    ) -> Result<T, Errore> {
        // Un gettone alzato prima della chiamata si vede al primo controllo
        // del lavoro, prima di qualunque lettura.
        self.trasferisci();
        let interruzione = self.interruzione.clone();
        let (invio, ricezione) = mpsc::channel();
        let maniglia = std::thread::Builder::new()
            .name("plenora-data".to_owned())
            .spawn(move || {
                // Solo nelle build delle prove (feature `sonde-di-prova`).
                #[cfg(feature = "sonde-di-prova")]
                let mut interruzione = interruzione;
                let esito = catch_unwind(AssertUnwindSafe(|| {
                    #[cfg(feature = "sonde-di-prova")]
                    plenora_pipeline::sonda::chiama_prima(&mut interruzione)?;
                    lavoro(&interruzione)
                }))
                .unwrap_or_else(|payload| Err(panico(payload.as_ref(), con_effetti)));
                // Il chiamante aspetta sempre l'esito: un invio fallito vuol
                // dire solo che non c'è più nessuno ad aspettarlo.
                let _ = invio.send(esito);
            })
            .map_err(|_| {
                Errore::Plenora(
                    PlenoraError::Internal("thread di lavoro non avviabile".to_owned())
                        .with_phase(ErrorPhase::Validate),
                )
            })?;
        let mut ricezione = ricezione;
        let esito = loop {
            let (indietro, esito) = py.detach(move || {
                let esito = ricezione.recv_timeout(INTERVALLO);
                (ricezione, esito)
            });
            ricezione = indietro;
            match esito {
                Ok(Err(errore)) => break Err(Errore::Plenora(errore)),
                Ok(Ok(valore)) => {
                    #[cfg(feature = "sonde-di-prova")]
                    if let Err(eccezione) = crate::sonde::chiama_consegna(py) {
                        break Err(Errore::Python(eccezione));
                    }
                    // Consegna: un annullamento arrivato nell'ultimo
                    // intervallo (gettone o segnale) non diventa un successo.
                    if let Err(eccezione) = self.sorveglia(py) {
                        let errore = Self::annullato_alla_consegna(con_effetti);
                        eccezione.set_cause(py, Some(in_python(py, &errore)));
                        break Err(Errore::Python(eccezione));
                    }
                    if self.segnale.load(Ordering::Acquire) {
                        break Err(Errore::Plenora(Self::annullato_alla_consegna(con_effetti)));
                    }
                    break Ok(valore);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Err(eccezione) = self.sorveglia(py) {
                        // Il segnale è alzato: il lavoro si ferma al suo
                        // controllo successivo, e il suo esito dice che cosa
                        // ha già fatto (finito comunque: annullato alla
                        // consegna, con l'effetto vero).
                        let (_, esito) = py.detach(move || {
                            let esito = ricezione.recv();
                            (ricezione, esito)
                        });
                        let errore = match esito {
                            Ok(Err(errore)) => Some(errore),
                            Ok(Ok(_)) => Some(Self::annullato_alla_consegna(con_effetti)),
                            Err(_) => None,
                        };
                        if let Some(errore) = errore {
                            eccezione.set_cause(py, Some(in_python(py, &errore)));
                        }
                        break Err(Errore::Python(eccezione));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let errore = PlenoraError::Internal(
                        "il thread di lavoro e' finito senza esito".to_owned(),
                    );
                    break Err(Errore::Plenora(if con_effetti {
                        errore.with_remote_effect(RemoteEffect::Unknown)
                    } else {
                        errore
                    }));
                }
            }
        };
        // L'esito c'è: il thread ha finito o sta finendo. Riunirlo evita un
        // thread vivo oltre la chiamata (e oltre lo spegnimento
        // dell'interprete).
        let _ = py.detach(move || maniglia.join());
        esito
    }
}
