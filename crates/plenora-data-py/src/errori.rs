//! Errori al confine Python.
//!
//! Un [`PlenoraError`] esce come istanza della gerarchia pubblica
//! `plenora_data.errors` (radice `PlenoraError`, una sottoclasse per
//! categoria), costruita in Python dal documento `plenora-error-v1` della
//! proiezione pubblica (`PlenoraError::public_projection`): gli stessi assi,
//! lo stesso codice, lo stesso messaggio senza dati e la stessa diagnostica
//! per riga della CLI, letti come campi e mai dal testo.
//!
//! Un'eccezione Python che non è un errore dell'operazione (Ctrl-C mentre
//! si aspetta il lavoro, un'eccezione del segnale) passa intatta
//! ([`Errore::Python`]): è del chiamante, non del componente.

use std::any::Any;

use plenora_core::panic_policy::forma_payload;
use plenora_core::{PlenoraError, RemoteEffect};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyModule;

/// Il modulo Python che costruisce le eccezioni pubbliche.
const MODULO_ERRORI: &str = "plenora_data.errors";
/// La funzione di [`MODULO_ERRORI`] che costruisce l'eccezione dal
/// documento `plenora-error-v1`.
const COSTRUTTORE: &str = "_da_documento";

/// Documento di riserva, se la proiezione non si serializzasse: la
/// serializzazione fallisce solo per un ritardo `after` oltre il massimo,
/// che la proiezione porta già a `never`. Assi conservativi.
const DOCUMENTO_DI_RISERVA: &str = r#"{"category":"internal","phase":"finalize","remote_effect":"unknown","retry":{"kind":"never"},"message":"internal: errore non serializzabile"}"#;

/// L'esito d'errore di una funzione del modulo nativo.
pub enum Errore {
    /// Un errore dell'operazione: diventa un'eccezione `PlenoraError`.
    Plenora(PlenoraError),
    /// Un'eccezione Python da propagare com'è (`KeyboardInterrupt` e simili).
    Python(PyErr),
}

impl From<PlenoraError> for Errore {
    fn from(errore: PlenoraError) -> Self {
        Self::Plenora(errore)
    }
}

/// L'eccezione pubblica di un [`PlenoraError`].
pub fn in_python(py: Python<'_>, errore: &PlenoraError) -> PyErr {
    let documento = serde_json::to_string(&errore.public_projection())
        .unwrap_or_else(|_| DOCUMENTO_DI_RISERVA.to_owned());
    costruisci(py, &documento)
}

/// L'eccezione Python di un [`Errore`].
pub fn converti(py: Python<'_>, errore: Errore) -> PyErr {
    match errore {
        Errore::Plenora(errore) => in_python(py, &errore),
        Errore::Python(errore) => errore,
    }
}

/// Costruisce l'eccezione con `plenora_data.errors._da_documento`. Se il
/// modulo non risponde (un'installazione rotta) l'eccezione che ne esce è
/// quella dell'import: il pacchetto Python la trasforma in
/// `PlenoraInternalError`, così nessuna eccezione nativa supera il confine.
fn costruisci(py: Python<'_>, documento: &str) -> PyErr {
    match modulo(py).and_then(|modulo| modulo.call_method1(COSTRUTTORE, (documento,))) {
        Ok(eccezione) => PyErr::from_value(eccezione),
        Err(errore) => errore,
    }
}

fn modulo(py: Python<'_>) -> PyResult<&Bound<'_, PyModule>> {
    static MODULO: PyOnceLock<Py<PyModule>> = PyOnceLock::new();
    MODULO
        .get_or_try_init(py, || py.import(MODULO_ERRORI).map(Bound::unbind))
        .map(|modulo| modulo.bind(py))
}

/// Se un'eccezione Python è già una `PlenoraError` pubblica (per esempio
/// un annullamento sollevato dal produttore di uno stream): allora passa
/// com'è, con la sua categoria, invece di essere riclassificata.
pub fn e_pubblica(py: Python<'_>, errore: &PyErr) -> bool {
    modulo(py)
        .and_then(|modulo| modulo.getattr("PlenoraError"))
        .and_then(|radice| errore.value(py).is_instance(&radice))
        .unwrap_or(false)
}

/// L'errore di un panico intercettato: `internal`, senza il testo del
/// payload (può contenere valori di riga), con effetto `unknown` quando
/// l'operazione può aver scritto file.
pub fn panico(payload: &(dyn Any + Send), con_effetti: bool) -> PlenoraError {
    let errore = PlenoraError::Internal(format!("panico interno ({})", forma_payload(payload)));
    if con_effetti {
        errore.with_remote_effect(RemoteEffect::Unknown)
    } else {
        errore
    }
}
