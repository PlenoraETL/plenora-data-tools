//! SDK Python `plenora-data`: il modulo nativo `plenora_data._native`.
//!
//! Il pacchetto Python (`python/plenora_data`) è la superficie pubblica; questo
//! modulo è privato e chiama le stesse funzioni della CLI
//! (`plenora_cli::api`), nelle forme con input in memoria: un'operazione ha
//! una sola implementazione, con la stessa validazione, lo stesso budget,
//! gli stessi documenti e gli stessi errori su ogni superficie (Python SDK
//! 1.0, sezione 12).
//!
//! - Le tabelle passano per l'Arrow C Stream Interface ([`arrow_py`]).
//! - Il lavoro gira in un thread suo, senza il GIL; il chiamante aspetta
//!   sorvegliando `CancellationToken` e segnali Python, e la scadenza è
//!   quella del runner ([`controlli`]).
//! - Gli errori escono come la gerarchia `PlenoraError` di
//!   `plenora_data.errors`, dal documento `plenora-error-v1` ([`errori`]).
//! - Un panico non attraversa il confine: l'hook di panico del modulo è
//!   silenzioso (`plenora_core::panic_policy`, niente su stderr) e ogni
//!   funzione lo intercetta e lo rende `internal` senza payload.
//!
//! I documenti JSON escono come testo JSON compatto, con le chiavi in
//! ordine: il pacchetto Python li legge con `json.loads`.

// Il linker MSVC annuncia su stdout la libreria d'import che crea per ogni
// DLL (`plenora_data_native.dll.lib`): un messaggio informativo, non un
// difetto, che il lint `linker_messages` riporterebbe a ogni build.
#![allow(linker_messages)]

mod arrow_py;
mod controlli;
mod errori;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use plenora_cli::api;
use plenora_cli::capacita::{documento_della, Superficie};
use plenora_core::memoria::byte_vivi;
use plenora_core::panic_policy::{install, PanicPolicy};
use plenora_core::{ErrorPhase, PlenoraError, DEFAULT_MAX_GOVERNED_MEMORY_BYTES};
use plenora_io::{FileIngresso, FileUscita, Ingresso, OpzioniScrittura};
use plenora_pipeline::Pipeline;
use pyo3::prelude::*;

use crate::controlli::Controlli;
use crate::errori::{converti, in_python, panico, Errore};

/// Segnale di annullamento cooperativo, condivisibile fra thread: chi lo
/// alza (`cancel()`) ferma l'operazione al suo controllo successivo.
#[pyclass(frozen, module = "plenora_data._native", name = "CancellationToken")]
pub struct Gettone {
    segnale: Arc<AtomicBool>,
}

#[pymethods]
impl Gettone {
    #[new]
    fn nuovo() -> Self {
        Self {
            segnale: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Alza il segnale. Idempotente; non si abbassa più.
    fn cancel(&self) {
        self.segnale.store(true, Ordering::Release);
    }

    /// Se il segnale è alzato.
    #[getter]
    fn cancelled(&self) -> bool {
        self.segnale.load(Ordering::Acquire)
    }

    fn __repr__(&self) -> &'static str {
        if self.cancelled() {
            "CancellationToken(cancelled=True)"
        } else {
            "CancellationToken(cancelled=False)"
        }
    }
}

/// Esegue il corpo di una funzione del modulo: un errore diventa
/// l'eccezione pubblica, un panico `internal` senza payload (con effetto
/// `unknown` se `con_effetti`).
fn proteggi<T>(
    py: Python<'_>,
    con_effetti: bool,
    corpo: impl FnOnce() -> Result<T, Errore>,
) -> PyResult<T> {
    match catch_unwind(AssertUnwindSafe(corpo)) {
        Ok(Ok(valore)) => Ok(valore),
        Ok(Err(errore)) => Err(converti(py, errore)),
        Err(payload) => Err(in_python(py, &panico(payload.as_ref(), con_effetti))),
    }
}

/// Un argomento rifiutato: fase `prepare`, derivata, come per gli argomenti
/// della CLI.
fn configurazione(motivo: &str) -> Errore {
    Errore::Plenora(PlenoraError::InvalidConfiguration(motivo.to_owned()))
}

fn interno(motivo: &str) -> Errore {
    Errore::Plenora(PlenoraError::Internal(motivo.to_owned()).with_phase(ErrorPhase::Validate))
}

fn percorso(oggetto: &Bound<'_, PyAny>, voce: &str) -> Result<PathBuf, Errore> {
    oggetto
        .extract::<PathBuf>()
        .map_err(|_| configurazione(&format!("`{voce}`: atteso un percorso (str o os.PathLike)")))
}

/// I controlli della chiamata, con un primo punto di controllo prima di
/// qualunque lavoro: una scadenza già passata (per esempio mentre la
/// chiamata asincrona aspettava un thread dell'executor) o un gettone già
/// alzato fermano la chiamata qui, in fase `prepare`, senza leggere nulla.
///
/// `scadenza_monotona` è l'istante di `time.monotonic()` fissato dal
/// pacchetto Python all'ingresso della chiamata pubblica (`timeout`); qui
/// diventa il tempo che resta.
fn controlli(
    py: Python<'_>,
    scadenza: Option<f64>,
    scadenza_monotona: Option<f64>,
    gettoni: Vec<Bound<'_, Gettone>>,
) -> Result<Controlli, Errore> {
    let gettoni = gettoni
        .into_iter()
        .map(|gettone| Arc::clone(&gettone.get().segnale))
        .collect();
    let restante = match scadenza_monotona {
        None => None,
        Some(istante) if istante.is_finite() => {
            let adesso = py
                .import("time")
                .and_then(|tempo| tempo.call_method0("monotonic"))
                .and_then(|valore| valore.extract::<f64>())
                .map_err(|_| interno("clock monotono di Python non leggibile"))?;
            Some((istante - adesso).max(0.0))
        }
        Some(_) => {
            return Err(configurazione(
                "`timeout`: atteso un numero finito di secondi, non negativo e rappresentabile",
            ))
        }
    };
    let controlli = Controlli::nuovi(scadenza, restante, gettoni).map_err(Errore::Plenora)?;
    controlli.verifica(py, "prima di cominciare", ErrorPhase::Prepare)?;
    Ok(controlli)
}

/// Il piano: un file (`path`, letto come la CLI lo legge) o il testo JSON
/// (`json`), al più `max_plan_json_bytes` in entrambi i casi.
fn piano(py: Python<'_>, tipo: &str, oggetto: &Bound<'_, PyAny>) -> Result<Pipeline, Errore> {
    match tipo {
        "path" => {
            let percorso = percorso(oggetto, "plan")?;
            py.detach(move || api::leggi_piano(&percorso))
                .map_err(Errore::Plenora)
        }
        "json" => {
            let testo = oggetto
                .extract::<String>()
                .map_err(|_| configurazione("`plan`: atteso testo JSON UTF-8"))?;
            py.detach(move || Pipeline::from_json(&testo))
                .map_err(Errore::Plenora)
        }
        _ => Err(interno("forma del piano sconosciuta")),
    }
}

/// Gli input nell'ordine dato: i file restano percorsi (li legge il lavoro,
/// con il budget del piano), gli oggetti Arrow si importano qui, con il
/// GIL, prima di partire. I nomi si verificano contro il piano prima di
/// importare qualunque dato. Ogni stream si importa con il budget del piano
/// meno le tabelle già importate, e si ferma appena lo supera.
fn ingressi(
    py: Python<'_>,
    piano: &Pipeline,
    voci: Vec<(String, String, Bound<'_, PyAny>)>,
    controlli: &Controlli,
) -> Result<Vec<Ingresso>, Errore> {
    api::verifica_nomi(
        &piano.inputs,
        voci.iter().map(|(nome, _, _)| nome.as_str()),
        "inputs",
    )?;
    let budget = plenora_io::budget_del_piano(piano)?;
    let mut ingressi = Vec::with_capacity(voci.len());
    for (nome, tipo, oggetto) in voci {
        let preso = match tipo.as_str() {
            "path" => Ingresso::File(FileIngresso {
                percorso: percorso(&oggetto, &format!("inputs[{nome}]"))?,
                nome,
                formato: None,
            }),
            "arrow" => {
                let importate = ingressi.iter().filter_map(|ingresso| match ingresso {
                    Ingresso::Tabella { tabella, .. } => Some(tabella),
                    Ingresso::File(_) => None,
                });
                let residuo = budget.saturating_sub(byte_vivi(importate)?);
                let tabella = arrow_py::importa(
                    py,
                    &oggetto,
                    &format!("input `{nome}`"),
                    controlli,
                    residuo,
                )?;
                Ingresso::Tabella { nome, tabella }
            }
            _ => return Err(interno("forma dell'input sconosciuta")),
        };
        ingressi.push(preso);
    }
    Ok(ingressi)
}

/// La versione del pacchetto: quella del crate, che è anche quella dei
/// metadati del wheel (pyproject.toml la legge dal crate).
#[pyfunction]
#[must_use]
const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Il documento `capabilities-v2` dell'SDK, come testo JSON.
#[pyfunction]
fn capabilities(py: Python<'_>) -> PyResult<String> {
    proteggi(py, false, || {
        Ok(documento_della(Superficie::Python).to_string())
    })
}

/// `data.catalog`, come testo JSON.
#[pyfunction]
fn catalog(py: Python<'_>) -> PyResult<String> {
    proteggi(py, false, || Ok(api::catalogo().to_string()))
}

/// `data.describe` di un file (`path`) o di un oggetto Arrow (`arrow`).
#[pyfunction]
fn describe(
    py: Python<'_>,
    tipo: &str,
    sorgente: &Bound<'_, PyAny>,
    scadenza: Option<f64>,
    scadenza_monotona: Option<f64>,
    gettoni: Vec<Bound<'_, Gettone>>,
) -> PyResult<String> {
    proteggi(py, false, || {
        let controlli = controlli(py, scadenza, scadenza_monotona, gettoni)?;
        let documento = match tipo {
            "path" => {
                let percorso = percorso(sorgente, "data")?;
                controlli.esegui(py, false, move |interruzione| {
                    api::descrivi(&percorso, interruzione)
                })?
            }
            "arrow" => {
                // Lo stesso budget di default della lettura da file.
                let tabella = arrow_py::importa(
                    py,
                    sorgente,
                    "input",
                    &controlli,
                    DEFAULT_MAX_GOVERNED_MEMORY_BYTES,
                )?;
                controlli.esegui(py, false, move |interruzione| {
                    api::descrivi_tabella(&tabella, interruzione)
                })?
            }
            _ => return Err(interno("forma della sorgente sconosciuta")),
        };
        Ok(documento.to_string())
    })
}

/// `data.validate`.
#[pyfunction]
fn validate(
    py: Python<'_>,
    tipo_piano: &str,
    piano_dato: &Bound<'_, PyAny>,
    voci: Vec<(String, String, Bound<'_, PyAny>)>,
    scadenza: Option<f64>,
    scadenza_monotona: Option<f64>,
    gettoni: Vec<Bound<'_, Gettone>>,
) -> PyResult<String> {
    proteggi(py, false, || {
        let controlli = controlli(py, scadenza, scadenza_monotona, gettoni)?;
        let piano = piano(py, tipo_piano, piano_dato)?;
        let ingressi = ingressi(py, &piano, voci, &controlli)?;
        let documento = controlli.esegui(py, false, move |interruzione| {
            api::valida_ingressi(&piano, ingressi, interruzione)
        })?;
        Ok(documento.to_string())
    })
}

/// Il risultato nativo di `data.run`: il documento JSON e le tabelle rese
/// (nessuna se gli output sono andati su file).
type Esecuzione = (String, Vec<(String, Py<PyAny>)>);

/// `data.run`: con `uscite` scrive gli output su file (effetto `local`),
/// senza li rende come `pyarrow.Table`.
#[pyfunction]
#[allow(clippy::too_many_arguments)] // La firma privata che il pacchetto Python chiama.
fn run(
    py: Python<'_>,
    tipo_piano: &str,
    piano_dato: &Bound<'_, PyAny>,
    voci: Vec<(String, String, Bound<'_, PyAny>)>,
    uscite: Option<Vec<(String, Bound<'_, PyAny>)>>,
    sovrascrivi: bool,
    scadenza: Option<f64>,
    scadenza_monotona: Option<f64>,
    gettoni: Vec<Bound<'_, Gettone>>,
) -> PyResult<Esecuzione> {
    let con_effetti = uscite.is_some();
    proteggi(py, con_effetti, || {
        let controlli = controlli(py, scadenza, scadenza_monotona, gettoni)?;
        if uscite.is_none() && sovrascrivi {
            return Err(configurazione(
                "`overwrite` vale solo con `outputs`: senza, nessun file si scrive",
            ));
        }
        let piano = piano(py, tipo_piano, piano_dato)?;
        let uscite = uscite
            .map(|uscite| {
                api::verifica_nomi(
                    &piano.outputs,
                    uscite.iter().map(|(nome, _)| nome.as_str()),
                    "outputs",
                )?;
                uscite
                    .into_iter()
                    .map(|(nome, oggetto)| {
                        Ok(FileUscita {
                            percorso: percorso(&oggetto, &format!("outputs[{nome}]"))?,
                            nome,
                            formato: None,
                        })
                    })
                    .collect::<Result<Vec<_>, Errore>>()
            })
            .transpose()?;
        let ingressi = ingressi(py, &piano, voci, &controlli)?;
        if let Some(uscite) = uscite {
            let opzioni = OpzioniScrittura {
                sovrascrivi,
                ..OpzioniScrittura::default()
            };
            let documento = controlli.esegui(py, true, move |interruzione| {
                api::esegui_ingressi(&piano, ingressi, &uscite, &opzioni, interruzione)
            })?;
            return Ok((documento.to_string(), Vec::new()));
        }
        let (documento, tabelle) = controlli.esegui(py, false, move |interruzione| {
            api::esegui_in_memoria(&piano, ingressi, interruzione)
        })?;
        let tabelle = tabelle
            .into_iter()
            .map(|(nome, tabella)| {
                let oggetto = arrow_py::esporta(py, tabella, &nome)?;
                Ok((nome, oggetto))
            })
            .collect::<Result<Vec<_>, Errore>>()?;
        Ok((documento.to_string(), tabelle))
    })
}

#[pymodule]
fn _native(modulo: &Bound<'_, PyModule>) -> PyResult<()> {
    // Nessun testo di panico su stderr: il payload può contenere valori di
    // riga, e ogni panico diventa comunque l'errore `internal` (sopra).
    // L'hook è quello della libreria standard di questo modulo nativo, non
    // dell'interprete né di altri moduli.
    let _ = install(PanicPolicy::Silent);
    modulo.add_class::<Gettone>()?;
    modulo.add_function(wrap_pyfunction!(version, modulo)?)?;
    modulo.add_function(wrap_pyfunction!(capabilities, modulo)?)?;
    modulo.add_function(wrap_pyfunction!(catalog, modulo)?)?;
    modulo.add_function(wrap_pyfunction!(describe, modulo)?)?;
    modulo.add_function(wrap_pyfunction!(validate, modulo)?)?;
    modulo.add_function(wrap_pyfunction!(run, modulo)?)?;
    Ok(())
}
