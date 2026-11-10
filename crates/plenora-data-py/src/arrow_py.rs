//! Tabelle fra Python e il runner, per l'Arrow C Stream Interface.
//!
//! In ingresso qualunque oggetto con `__arrow_c_stream__` (l'interfaccia
//! `PyCapsule` di Arrow: `pyarrow.Table`, `pyarrow.RecordBatchReader`, e le
//! tabelle di altre librerie Arrow) o `__arrow_c_array__`. Gli originali
//! `PyArrow` sono validati prima del passaggio C, anche nei reader lazy. I buffer si
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
use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::Schema;
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::memoria::{byte_dati, picco_unione};
use plenora_core::{ErrorPhase, PlenoraError};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

use crate::controlli::Controlli;
use crate::errori::{e_pubblica, Errore};

/// Un errore di lettura dell'input, senza il testo della causa (può
/// contenere valori del produttore).
fn illeggibile(contesto: &str, cosa: &str) -> Errore {
    Errore::Plenora(
        PlenoraError::DataMapping(format!("{contesto}: {cosa}")).with_phase(ErrorPhase::Read),
    )
}

/// L'eccezione Python sollevata dal produttore dello stream. Un'eccezione
/// che non è `Exception` (`KeyboardInterrupt`, `SystemExit`) o che è già
/// una `PlenoraError` pubblica passa com'è; le altre diventano
/// `data_mapping` senza il loro testo.
fn errore_del_produttore(py: Python<'_>, errore: PyErr, contesto: &str) -> Errore {
    if errore.is_instance_of::<PyException>(py) && !e_pubblica(py, &errore) {
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
    let preparato = py
        .import("plenora_data._arrow")
        .and_then(|modulo| modulo.getattr("prepara"))
        .and_then(|prepara| prepara.call1((oggetto,)))
        .map_err(|errore| errore_del_produttore(py, errore, contesto))?;
    // PyArrow mantiene lo stream lazy; ogni batch passa poi dall'import
    // fallibile di arrow-pyarrow, che valida ArrayData PRIMA di StructArray.
    let lettore = py
        .import("plenora_data._arrow")
        .and_then(|modulo| modulo.getattr("lettore"))
        .and_then(|funzione| funzione.call1((&preparato,)))
        .map_err(|errore| errore_del_produttore(py, errore, contesto))?;
    let schema = std::sync::Arc::new(
        lettore
            .getattr("schema")
            .and_then(|schema| Schema::from_pyarrow_bound(&schema))
            .map_err(|errore| errore_del_produttore(py, errore, contesto))?,
    );
    // L'import C Stream si fida dei buffer del produttore. Il contratto
    // esclude run-end e union: rifiutarli prima di importare o concatenare
    // blocchi impedisce che dati malformati arrivino ai kernel Arrow.
    plenora_core::contract::arrow_schema::verifica_tipi_supportati(&schema)
        .map_err(|errore| Errore::Plenora(errore.with_phase(ErrorPhase::Read)))?;
    let mut blocchi = Vec::new();
    let mut letti = 0_u64;
    for blocco in lettore
        .try_iter()
        .map_err(|errore| errore_del_produttore(py, errore, contesto))?
    {
        let blocco = blocco
            .and_then(|blocco| RecordBatch::from_pyarrow_bound(&blocco))
            .map_err(|errore| {
                if errore.is_instance_of::<PyException>(py) && !e_pubblica(py, &errore) {
                    illeggibile(contesto, "stream Arrow non leggibile (C Data Interface)")
                } else {
                    Errore::Python(errore)
                }
            })?;
        plenora_core::valida_arrow::batch(&blocco).map_err(Errore::Plenora)?;
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
        0 => plenora_core::batch_vuoto(schema)
            .map_err(|_| illeggibile(contesto, "lo schema non ammette una tabella vuota")),
        1 => blocchi
            .pop()
            .ok_or_else(|| Errore::Plenora(PlenoraError::Internal("blocco assente".to_owned()))),
        _ if picco_unione(&blocchi) > residuo => {
            Err(oltre_il_budget(contesto, picco_unione(&blocchi), residuo))
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
    plenora_core::valida_arrow::batch(&tabella).map_err(Errore::Plenora)?;
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
