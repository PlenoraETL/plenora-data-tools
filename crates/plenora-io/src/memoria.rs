//! Conti di memoria comuni a lettura e scrittura.

use plenora_core::arrow::array::cast::AsArray;
use plenora_core::arrow::array::RecordBatch;
use plenora_core::PlenoraError;

/// Margine fisso dei transitori di scrittura: metadati, buffer di I/O,
/// pagine in corso.
pub const MARGINE: u64 = 1024 * 1024;

pub fn oltre_il_budget(servono: u64, residuo: u64) -> PlenoraError {
    PlenoraError::ResourceLimit(format!(
        "{servono} byte oltre il budget residuo di {residuo} (max_governed_memory_bytes)"
    ))
}

/// Byte di una copia dei batch (`plenora_core::memoria::byte_dati`), la
/// stima di ciò che `concat_batches` alloca.
pub fn stima_byte(blocchi: &[RecordBatch]) -> u64 {
    blocchi.iter().fold(0_u64, |totale, blocco| {
        totale.saturating_add(
            u64::try_from(plenora_core::memoria::byte_dati(blocco)).unwrap_or(u64::MAX),
        )
    })
}

/// Valori dei dizionari di primo livello, interi.
pub fn byte_dizionari(tabella: &RecordBatch) -> u64 {
    tabella.columns().iter().fold(0_u64, |totale, colonna| {
        let byte = colonna.as_any_dictionary_opt().map_or(0, |dizionario| {
            plenora_core::memoria::byte_viste(dizionario.values().as_ref())
        });
        totale.saturating_add(u64::try_from(byte).unwrap_or(u64::MAX))
    })
}
