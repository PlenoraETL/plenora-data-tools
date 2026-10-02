//! Tabelle fra Python e il runner, per l'Arrow C Stream Interface.
//!
//! In ingresso qualunque oggetto con `__arrow_c_stream__` (l'interfaccia
//! `PyCapsule` di Arrow: `pyarrow.Table`, `pyarrow.RecordBatchReader`, e le
//! tabelle di altre librerie Arrow); il pacchetto Python porta un
//! `pyarrow.RecordBatch` in una `pyarrow.Table` senza copia. I buffer si
//! importano senza copia; lo schema arriva con i metadati di schema e di
//! campo intatti (`plenora.*`, `ARROW:extension:name` di `GeoArrow`). Il
//! runner vuole un solo
//! `RecordBatch` per tabella: uno stream di un blocco passa com'è, uno di
//! più blocchi si unisce con `concat_batches` (una copia, limite dichiarato
//! nel README del crate).
//!
//! In uscita ogni tabella diventa una `pyarrow.Table` di un blocco, senza
//! copia (`arrow_pyarrow::Table`).

use arrow_pyarrow::{FromPyArrow, IntoPyArrow, Table};
use plenora_core::arrow::array::ffi_stream::ArrowArrayStreamReader;
use plenora_core::arrow::array::{RecordBatch, RecordBatchReader};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::arrow::ArrowError;
use plenora_core::{ErrorPhase, PlenoraError};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

use crate::controlli::Controlli;
use crate::errori::Errore;

/// Un errore di lettura dell'input, senza il testo della causa (può
/// contenere valori del produttore).
fn illeggibile(contesto: &str, cosa: &str) -> Errore {
    Errore::Plenora(
        PlenoraError::DataMapping(format!("{contesto}: {cosa}")).with_phase(ErrorPhase::Read),
    )
}

/// L'errore di Arrow sullo stream: un tipo che l'import non conosce è
/// `unsupported`, il resto `data_mapping`. Mai il testo di Arrow, che può
/// riportare il messaggio del produttore.
fn errore_arrow(contesto: &str, errore: &ArrowError) -> Errore {
    match errore {
        ArrowError::NotYetImplemented(_) => Errore::Plenora(
            PlenoraError::Unsupported(format!(
                "{contesto}: tipo Arrow non supportato dall'import (C Data Interface)"
            ))
            .with_phase(ErrorPhase::Read),
        ),
        _ => illeggibile(contesto, "stream Arrow non leggibile (C Data Interface)"),
    }
}

/// L'eccezione Python sollevata dal produttore dello stream. Un'eccezione
/// che non è `Exception` (`KeyboardInterrupt`, `SystemExit`) passa com'è;
/// le altre diventano `data_mapping` senza il loro testo.
fn errore_del_produttore(py: Python<'_>, errore: PyErr, contesto: &str) -> Errore {
    if errore.is_instance_of::<PyException>(py) {
        illeggibile(
            contesto,
            "l'oggetto non da' uno stream Arrow (Arrow PyCapsule Interface, `__arrow_c_stream__`)",
        )
    } else {
        Errore::Python(errore)
    }
}

/// Importa una tabella da un oggetto Python con `__arrow_c_stream__`.
///
/// Fra un blocco e l'altro guarda segnali, gettoni e scadenza (fase
/// `read`): un produttore lento si interrompe come la lettura di un file.
///
/// # Errors
///
/// `data_mapping` o `unsupported` (fase `read`) per uno stream che non si
/// legge o non forma una tabella; `cancelled`, `timeout` o l'eccezione di
/// un segnale a un controllo.
pub fn importa(
    py: Python<'_>,
    oggetto: &Bound<'_, PyAny>,
    contesto: &str,
    controlli: &Controlli,
) -> Result<RecordBatch, Errore> {
    controlli.verifica(
        py,
        &format!("prima di leggere l'{contesto}"),
        ErrorPhase::Read,
    )?;
    let lettore = ArrowArrayStreamReader::from_pyarrow_bound(oggetto)
        .map_err(|errore| errore_del_produttore(py, errore, contesto))?;
    let schema = lettore.schema();
    let mut blocchi = Vec::new();
    for blocco in lettore {
        let blocco = blocco.map_err(|errore| errore_arrow(contesto, &errore))?;
        blocchi.push(blocco);
        controlli.verifica(
            py,
            &format!("durante la lettura dell'{contesto}"),
            ErrorPhase::Read,
        )?;
    }
    match blocchi.len() {
        0 => Ok(RecordBatch::new_empty(schema)),
        1 => blocchi
            .pop()
            .ok_or_else(|| Errore::Plenora(PlenoraError::Internal("blocco assente".to_owned()))),
        _ => concat_batches(&schema, &blocchi).map_err(|_| {
            illeggibile(
                contesto,
                "i blocchi dello stream non formano una tabella (schema diverso o oltre i \
                 limiti di Arrow)",
            )
        }),
    }
}

/// Una tabella d'uscita come `pyarrow.Table`, senza copia.
///
/// # Errors
///
/// `internal` (fase `finalize`) se pyarrow non la riceve.
pub fn esporta(py: Python<'_>, tabella: RecordBatch, nome: &str) -> Result<Py<PyAny>, Errore> {
    let consegna = || {
        Errore::Plenora(
            PlenoraError::Internal(format!("output `{nome}`: consegna a pyarrow non riuscita"))
                .with_phase(ErrorPhase::Finalize),
        )
    };
    let schema = tabella.schema();
    let tavola = Table::try_new(vec![tabella], schema).map_err(|_| consegna())?;
    tavola
        .into_pyarrow(py)
        .map(Bound::unbind)
        .map_err(|_| consegna())
}
