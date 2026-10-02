//! Tabelle fra Python e il runner, per l'Arrow C Stream Interface.
//!
//! In ingresso qualunque oggetto con `__arrow_c_stream__` (l'interfaccia
//! `PyCapsule` di Arrow: `pyarrow.Table`, `pyarrow.RecordBatchReader`, e le
//! tabelle di altre librerie Arrow); il pacchetto Python porta un
//! `pyarrow.RecordBatch` in una `pyarrow.Table` senza copia. I buffer si
//! importano senza copia; lo schema arriva con i metadati di schema e di
//! campo intatti (`plenora.*`, `ARROW:extension:name` di `GeoArrow`). Un
//! buffer che non rispetta l'allineamento di Arrow si riallinea con una
//! copia (`arrow-array`, all'import): «senza copia» vale per i buffer
//! allineati, come quelli di pyarrow. Il runner vuole un solo `RecordBatch`
//! per tabella: uno stream di un blocco passa com'è, uno di più blocchi si
//! unisce con `concat_batches` (una copia, contata nel budget).
//!
//! In uscita ogni tabella diventa una `pyarrow.Table` di un blocco, senza
//! copia (`arrow_pyarrow::Table`).

use arrow_pyarrow::{FromPyArrow, IntoPyArrow, Table};
use plenora_core::arrow::array::ffi_stream::ArrowArrayStreamReader;
use plenora_core::arrow::array::{RecordBatch, RecordBatchReader};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::arrow::ArrowError;
use plenora_core::memoria::byte_dati;
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

/// Il budget superato durante l'import: fase `read`, come la lettura di un
/// file oltre il suo residuo.
fn oltre_il_budget(contesto: &str, servono: u64, residuo: u64) -> Errore {
    Errore::Plenora(
        PlenoraError::ResourceLimit(format!(
            "{contesto}: {servono} byte oltre il budget residuo di {residuo} \
             (max_governed_memory_bytes)"
        ))
        .with_phase(ErrorPhase::Read),
    )
}

/// Importa una tabella da un oggetto Python con `__arrow_c_stream__`.
///
/// Fra un blocco e l'altro guarda segnali, gettoni e scadenza (fase
/// `read`): un produttore lento si interrompe come la lettura di un file.
///
/// `residuo` è il budget che la tabella può occupare: i byte di dati dei
/// blocchi (`byte_dati`) si sommano mentre arrivano, e l'import si ferma al
/// primo blocco che lo supera, senza chiedere il successivo al produttore.
/// Con più blocchi l'unione ne fa una copia: blocchi e copia devono stare
/// insieme nel residuo prima di unirli.
///
/// # Errors
///
/// `data_mapping` o `unsupported` (fase `read`) per uno stream che non si
/// legge o non forma una tabella; `resource_limit` (fase `read`) oltre il
/// residuo; `cancelled`, `timeout` o l'eccezione di un segnale a un
/// controllo.
pub fn importa(
    py: Python<'_>,
    oggetto: &Bound<'_, PyAny>,
    contesto: &str,
    controlli: &Controlli,
    residuo: u64,
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
    let mut letti = 0_u64;
    for blocco in lettore {
        let blocco = blocco.map_err(|errore| errore_arrow(contesto, &errore))?;
        letti = letti.saturating_add(u64::try_from(byte_dati(&blocco)).unwrap_or(u64::MAX));
        if letti > residuo {
            return Err(oltre_il_budget(contesto, letti, residuo));
        }
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
        _ if letti.saturating_mul(2) > residuo => {
            Err(oltre_il_budget(contesto, letti.saturating_mul(2), residuo))
        }
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
