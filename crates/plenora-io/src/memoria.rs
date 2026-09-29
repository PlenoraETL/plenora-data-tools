//! Conti di memoria comuni a lettura e scrittura.

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

/// Byte della fetta più grande della tabella tagliata ogni
/// `righe_per_fetta` righe: `get_slice_memory_size` di ogni colonna (per
/// eccesso: valori dei dizionari e figli delle liste interi), o le viste
/// intere per un tipo che Arrow non sa misurare. Misura le fette vere, così
/// una riga molto più grande della media non sfugge alla stima.
pub fn fetta_massima(tabella: &RecordBatch, righe_per_fetta: usize) -> u64 {
    let righe = tabella.num_rows();
    let passo = righe_per_fetta.max(1);
    let mut massimo = 0_u64;
    let mut inizio = 0;
    loop {
        let lunghezza = passo.min(righe - inizio);
        let fetta = tabella.slice(inizio, lunghezza);
        let byte = fetta.columns().iter().fold(0_u64, |totale, colonna| {
            let byte = colonna
                .to_data()
                .get_slice_memory_size()
                .unwrap_or_else(|_| plenora_core::memoria::byte_viste(colonna.as_ref()));
            totale.saturating_add(u64::try_from(byte).unwrap_or(u64::MAX))
        });
        massimo = massimo.max(byte);
        inizio += lunghezza;
        if inizio >= righe {
            return massimo;
        }
    }
}
