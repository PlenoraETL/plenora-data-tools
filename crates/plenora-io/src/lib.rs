//! plenora-io — tabelle da e verso file.
//!
//! Formati (README, «File»):
//!
//! - **Arrow IPC**: lettura di file (Feather v2) e stream, scrittura di
//!   file; schema e metadati di schema e di campo intatti ([`ipc`]);
//! - **Parquet**: lettura con lo schema Arrow incorporato verificato,
//!   scrittura deterministica ([`parquet_io`]);
//! - **`GeoParquet` 1.1**: il metadato di file `geo` si mappa sul contratto
//!   geometrico del workspace (`GeoArrow`-WKB, CRS della tabella integrata) e
//!   ritorno ([`geoparquet`]).
//!
//! Ogni tabella è un solo `RecordBatch` in memoria: niente streaming.
//! Ogni scrittura è atomica ([`atomico`]): file temporaneo nella stessa
//! directory, verifica, rinomina; un percorso esistente non si sovrascrive
//! senza [`OpzioniScrittura::sovrascrivi`].
//!
//! [`esegui_da_file`] carica gli input nominati di un piano, lo valida e lo
//! esegue con il budget del piano, e scrive gli output nominati.

pub mod atomico;
pub mod crs_projjson;
mod esecuzione;
mod formato;
pub mod geoparquet;
pub mod ipc;
mod memoria;
pub mod parquet_io;
pub mod wkb;

use std::cell::RefCell;
use std::path::Path;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::{PlenoraError, Result};

pub use esecuzione::{esegui_da_file, FileIngresso, FileUscita};
pub use formato::{CompressioneParquet, Formato, OpzioniScrittura};

/// Legge una tabella da file.
///
/// `formato` esplicito o dall'estensione; `residuo` è il budget in byte che
/// la tabella può occupare (`u64::MAX` per nessun limite): oltre, la lettura
/// si ferma con `ResourceLimit` (README, «File», per ciò che si verifica
/// prima di decodificare e ciò che si verifica dopo).
///
/// # Errors
///
/// Quelli di [`Formato::risolvi`], [`ipc::leggi`] e [`parquet_io::leggi`].
pub fn leggi_tabella(
    percorso: &Path,
    formato: Option<Formato>,
    residuo: u64,
) -> Result<RecordBatch> {
    match Formato::risolvi(formato, percorso)? {
        Formato::ArrowIpc => ipc::leggi(percorso, residuo),
        Formato::Parquet => parquet_io::leggi(percorso, residuo),
    }
}

/// Scrive una tabella su file, in modo atomico.
///
/// # Errors
///
/// Quelli di [`Formato::risolvi`], [`atomico::scrivi_atomico`],
/// [`ipc::scrivi`], [`parquet_io::scrivi`] e [`parquet_io::verifica_schema`].
pub fn scrivi_tabella(
    tabella: &RecordBatch,
    percorso: &Path,
    opzioni: &OpzioniScrittura,
) -> Result<()> {
    // Run-end e union non si scrivono: la rilettura li rifiuterebbe
    // (README, «Run-end e union rifiutati al confine»).
    plenora_core::contract::arrow_schema::verifica_tipi_supportati(&tabella.schema())?;
    match Formato::risolvi(opzioni.formato, percorso)? {
        Formato::ArrowIpc => atomico::scrivi_atomico(
            percorso,
            opzioni.sovrascrivi,
            |uscita| ipc::scrivi(tabella, uscita),
            |temporaneo| ipc::verifica_schema(temporaneo, &tabella.schema()),
        ),
        Formato::Parquet => {
            let scritto = RefCell::new(None);
            atomico::scrivi_atomico(
                percorso,
                opzioni.sovrascrivi,
                |uscita| {
                    let schema = parquet_io::scrivi(tabella, uscita, opzioni.compressione)?;
                    scritto.replace(Some(schema));
                    Ok(())
                },
                |temporaneo| {
                    let schema = scritto.borrow_mut().take().ok_or_else(|| {
                        PlenoraError::Internal("schema scritto assente".to_owned())
                    })?;
                    parquet_io::verifica_schema(temporaneo, &schema)
                },
            )
        }
    }
}
