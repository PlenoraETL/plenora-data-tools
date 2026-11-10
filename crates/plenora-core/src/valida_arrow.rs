//! Confine comune per dati Arrow forniti dal chiamante.

use crate::arrow::array::{Array, RecordBatch};
use crate::{ErrorPhase, PlenoraError, Result};

fn malformato() -> PlenoraError {
    PlenoraError::DataMapping("dati Arrow non validi".to_owned()).with_phase(ErrorPhase::Read)
}

/// Valida ricorsivamente buffer, offset, dizionari e figli prima dell'uso.
///
/// # Errors
///
/// `DataMapping/read` senza valori del chiamante per un array malformato.
pub fn array(array: &dyn Array) -> Result<()> {
    array.to_data().validate_full().map_err(|_| malformato())
}

/// Valida tutte le colonne e la loro coerenza con schema e righe.
///
/// # Errors
///
/// `DataMapping/read` senza valori del chiamante per una tabella malformata.
pub fn batch(batch: &RecordBatch) -> Result<()> {
    let schema = batch.schema();
    if schema.fields().len() != batch.num_columns() {
        return Err(malformato());
    }
    let rows = batch.num_rows();
    for (field, column) in schema.fields().iter().zip(batch.columns()) {
        array(column.as_ref())?;
        if column.len() != rows
            || column.data_type() != field.data_type()
            || (!field.is_nullable() && column.null_count() > 0)
        {
            return Err(malformato());
        }
    }
    Ok(())
}
