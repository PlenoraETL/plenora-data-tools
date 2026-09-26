//! Gli ingressi Arrow IPC di `scripts/qualifica_profilo_isolato.sh`.
//!
//! Scrive un file format con tre colonne — `id` (`Int64`), `nome` (`Utf8`),
//! `valore` (`Float64`) — e il numero di righe e di righe per batch che si
//! chiede. I valori dipendono solo dall'indice di riga, quindi due giri con gli
//! stessi argomenti scrivono gli stessi byte.
//!
//! # Perche' un esempio e non uno script
//!
//! Perche' la macchina di qualifica non ha `pyarrow`, e il formato lo scrive
//! gia' la dipendenza del prodotto: un generatore in un altro linguaggio
//! sarebbe una seconda implementazione del formato da tenere allineata.
//!
//! # Uso
//!
//! ```sh
//! cargo run --release -p plenora-engine --example ingressi_qualifica -- \
//!     USCITA.arrow RIGHE RIGHE_PER_BATCH
//! ```

use std::error::Error;
use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};

fn main() -> Result<(), Box<dyn Error>> {
    let argomenti: Vec<String> = std::env::args().skip(1).collect();
    let [uscita, righe, per_batch] = argomenti.as_slice() else {
        return Err("uso: ingressi_qualifica USCITA.arrow RIGHE RIGHE_PER_BATCH".into());
    };
    let righe: u64 = righe.parse()?;
    let per_batch: u64 = per_batch.parse()?;
    if per_batch == 0 {
        return Err("RIGHE_PER_BATCH deve essere maggiore di zero".into());
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("nome", DataType::Utf8, false),
        Field::new("valore", DataType::Float64, false),
    ]));
    let file = std::fs::File::create(uscita)?;
    let mut scrittore = FileWriter::try_new(file, &schema)?;
    let mut inizio = 0_u64;
    while inizio < righe {
        let limite = righe.min(inizio.saturating_add(per_batch));
        let indici = inizio..limite;
        let id: Int64Array = indici
            .clone()
            .map(|indice| i64::try_from(indice).unwrap_or(i64::MAX))
            .collect();
        let nome: StringArray = indici
            .clone()
            .map(|indice| Some(format!("riga-{indice}")))
            .collect();
        let valore: Float64Array = indici
            .map(|indice| u32::try_from(indice % 1_000_003).map_or(0.0, f64::from))
            .collect();
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(id), Arc::new(nome), Arc::new(valore)],
        )?;
        scrittore.write(&batch)?;
        inizio = limite;
    }
    scrittore.finish()?;
    Ok(())
}
