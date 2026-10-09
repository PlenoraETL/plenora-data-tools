//! Sonde delle prove dei controlli, solo con la feature `sonde-di-prova`
//! (disattivata per default): le build di rilascio non hanno né questo
//! modulo né `_native._sonda_consegna` e `_native._sonda_lavoro`.
//!
//! - `_sonda_consegna(callable | None)`: un callable chiamato fra la fine
//!   del lavoro e il controllo della consegna, sul thread del chiamante;
//!   la sua eccezione passa com'è.
//! - `_sonda_lavoro(callable | None)`: un callable chiamato con il GIL, nel
//!   thread del lavoro, ai punti di `plenora_pipeline::sonda` (`prima`,
//!   `fra_passi`, `fra_scritture`), con il nome del punto. Risponde `None`
//!   (il lavoro prosegue), un numero di secondi (il lavoro aspetta al più
//!   per quel tempo che la sua interruzione scatti, poi prosegue: così una
//!   prova sa che il segnale alzato dalla sonda è arrivato al lavoro mentre
//!   girava, senza contare su quanto dura) o `"scadenza"` (solo a `prima`:
//!   la scadenza del lavoro, se c'è, diventa adesso).
//!
//! Sono gli istanti che una prova non sa raggiungere dall'esterno se non a
//! tempo.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use plenora_core::PlenoraError;
use plenora_pipeline::sonda::{Azione, Punto};
use plenora_pipeline::Interruzione;
use pyo3::prelude::*;

/// La sonda della consegna.
static CONSEGNA: Mutex<Option<Py<PyAny>>> = Mutex::new(None);

/// Registra o toglie la sonda della consegna.
///
/// La sonda precedente esce dal lucchetto e si rilascia dopo averlo
/// liberato: il suo `__del__` può richiamare questa funzione, e rilasciarla
/// con il lucchetto preso sarebbe un deadlock.
#[pyfunction]
#[pyo3(name = "_sonda_consegna")]
#[allow(clippy::needless_pass_by_value)] // PyO3 estrae l'argomento per valore.
fn sonda_consegna(sonda: Option<Py<PyAny>>) {
    let precedente = std::mem::replace(
        &mut *CONSEGNA.lock().unwrap_or_else(PoisonError::into_inner),
        sonda,
    );
    drop(precedente);
}

/// Chiama la sonda della consegna, se c'è; la sua eccezione passa com'è.
pub fn chiama_consegna(py: Python<'_>) -> PyResult<()> {
    let sonda = CONSEGNA
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|sonda| sonda.clone_ref(py));
    sonda.map_or(Ok(()), |sonda| sonda.call0(py).map(|_| ()))
}

/// Registra o toglie la sonda del lavoro (in `plenora_pipeline::sonda`,
/// che rilascia la precedente fuori dal lucchetto).
#[pyfunction]
#[pyo3(name = "_sonda_lavoro")]
#[allow(clippy::needless_pass_by_value)] // PyO3 estrae l'argomento per valore.
fn sonda_lavoro(sonda: Option<Py<PyAny>>) {
    plenora_pipeline::sonda::registra(sonda.map(|sonda| {
        let sonda: plenora_pipeline::sonda::Sonda =
            Arc::new(move |punto, interruzione| chiama_lavoro(&sonda, punto, interruzione));
        sonda
    }));
}

/// Che cosa chiede la sonda del lavoro.
enum Risposta {
    Prosegui,
    Attendi(Duration),
    Scadenza,
}

/// Chiama la sonda del lavoro a `punto` e fa ciò che chiede.
///
/// # Errors
///
/// `Internal` se la sonda solleva o restituisce qualcosa che non è `None`,
/// un numero di secondi valido o `"scadenza"`.
fn chiama_lavoro(
    sonda: &Py<PyAny>,
    punto: Punto,
    interruzione: &Interruzione,
) -> Result<Azione, PlenoraError> {
    let fallita = || PlenoraError::Internal("sonda del lavoro fallita".to_owned());
    // L'eccezione della sonda si scarta con il GIL preso.
    let risposta = Python::attach(|py| {
        let valore = sonda.call1(py, (punto.nome(),)).map_err(|_| fallita())?;
        let valore = valore.bind(py);
        if valore.is_none() {
            return Ok(Risposta::Prosegui);
        }
        if let Ok(testo) = valore.extract::<String>() {
            return if testo == "scadenza" {
                Ok(Risposta::Scadenza)
            } else {
                Err(fallita())
            };
        }
        let secondi = valore.extract::<f64>().map_err(|_| fallita())?;
        Duration::try_from_secs_f64(secondi)
            .map(Risposta::Attendi)
            .map_err(|_| fallita())
    })?;
    match risposta {
        Risposta::Prosegui => Ok(Azione::Prosegui),
        Risposta::Scadenza => Ok(Azione::AnticipaScadenza),
        Risposta::Attendi(tetto) => {
            let inizio = Instant::now();
            while interruzione.verifica("").is_ok() && inizio.elapsed() < tetto {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(Azione::Prosegui)
        }
    }
}

/// Aggiunge le due sonde al modulo nativo.
pub fn aggiungi(modulo: &Bound<'_, PyModule>) -> PyResult<()> {
    modulo.add_function(wrap_pyfunction!(sonda_consegna, modulo)?)?;
    modulo.add_function(wrap_pyfunction!(sonda_lavoro, modulo)?)?;
    Ok(())
}
