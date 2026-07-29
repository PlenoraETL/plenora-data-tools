use std::cmp::Ordering;

use plenora_core::arrow::array::{
    Array, ArrayRef, Float64Array, Int64Array, RecordBatch, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use plenora_core::Result;

use crate::scalar_as_string;

#[cfg(test)] // Solo i test-oracolo usano il percorso testuale originale.
pub(in crate::aggregation) fn row_key(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<String> {
    let mut key = String::new();
    for index in indices {
        let value = scalar_as_string(batch.column(*index).as_ref(), row)?;
        key.push_str(batch.column(*index).data_type().to_string().as_str());
        key.push('\u{1e}');
        match value {
            Some(value) => {
                key.push('1');
                key.push_str(&value.len().to_string());
                key.push(':');
                key.push_str(&value);
            }
            None => key.push('0'),
        }
        key.push('\u{1f}');
    }
    Ok(key)
}

/// Confronto tipizzato tra due celle, condiviso dai tre siti di confronto di
/// `table.sort`.
///
/// I tre siti sono `compare_at` (stesso batch), `ColumnComparator` (fast
/// path tipizzato) e il merge k-way dello spill (`spill::compare_cells`,
/// batch diversi — da qui la forma a due array).
///
/// Semantica: null dopo i valori (uguaglianza tra null); confronto nativo
/// esatto per Int64 (`i64::cmp`: la conversione a f64 collasserebbe valori
/// distinti oltre 2^53 sullo stesso double) e per `UInt64` (`u64::cmp`: il
/// fallback testuale ordinerebbe "10" prima di "9"); `total_cmp` per
/// Float64 (invariato); fallback `scalar_as_string` per gli altri tipi
/// (invariato, stesso ordine storico del kernel). Le colonne confrontate
/// appartengono allo stesso schema; se i tipi non coincidono con quello
/// atteso si ricade comunque sul fallback testuale.
pub fn compare_cells_typed(
    left: &ArrayRef,
    left_row: usize,
    right: &ArrayRef,
    right_row: usize,
) -> Result<Ordering> {
    let left_null = left.is_null(left_row);
    let right_null = right.is_null(right_row);
    // Match esaustivo sulle quattro combinazioni: il caso (false, false)
    // prosegue con il confronto tipizzato, nessun braccio impossibile.
    match (left_null, right_null) {
        (true, true) => return Ok(Ordering::Equal),
        (true, false) => return Ok(Ordering::Greater),
        (false, true) => return Ok(Ordering::Less),
        (false, false) => {}
    }
    match left.data_type() {
        DataType::Int64 => {
            if let (Some(left_values), Some(right_values)) = (
                left.as_any().downcast_ref::<Int64Array>(),
                right.as_any().downcast_ref::<Int64Array>(),
            ) {
                return Ok(left_values.value(left_row).cmp(&right_values.value(right_row)));
            }
        }
        DataType::UInt64 => {
            if let (Some(left_values), Some(right_values)) = (
                left.as_any().downcast_ref::<UInt64Array>(),
                right.as_any().downcast_ref::<UInt64Array>(),
            ) {
                return Ok(left_values.value(left_row).cmp(&right_values.value(right_row)));
            }
        }
        DataType::Float64 => {
            if let (Some(left_values), Some(right_values)) = (
                left.as_any().downcast_ref::<Float64Array>(),
                right.as_any().downcast_ref::<Float64Array>(),
            ) {
                return Ok(left_values
                    .value(left_row)
                    .total_cmp(&right_values.value(right_row)));
            }
        }
        _ => {}
    }
    Ok(scalar_as_string(left.as_ref(), left_row)?
        .cmp(&scalar_as_string(right.as_ref(), right_row)?))
}

pub(in crate::aggregation) fn compare_at(batch: &RecordBatch, index: usize, left: usize, right: usize) -> Result<Ordering> {
    let array = batch.column(index);
    compare_cells_typed(array, left, array, right)
}
