//! plenora-io — tabelle da e verso file.
//!
//! Formati (docs/file.md, «File»):
//!
//! - **Arrow IPC**: lettura di file (Feather v2) e stream, scrittura di
//!   file e stream ([`Formato::ArrowIpcStream`], estensione `.arrows`);
//!   schema e metadati di schema e di campo intatti ([`ipc`]);
//! - **Parquet**: lettura con lo schema Arrow incorporato verificato,
//!   scrittura deterministica ([`parquet_io`]);
//! - **`GeoParquet` 1.1**: il metadato di file `geo` si mappa sul contratto
//!   geometrico del workspace (`GeoArrow`-WKB, CRS della tabella integrata) e
//!   ritorno ([`geoparquet`]).
//!
//! Ogni file letto è input ostile: il confine di lettura ([`confine`],
//! docs/file.md, «Confine di lettura») verifica lunghezze e limiti prima che
//! `arrow-ipc` o `parquet` allochino, e chiama le dipendenze dentro una
//! barriera anti-panico; un file malformato è un errore, mai un panico.
//!
//! Ogni tabella è un solo `RecordBatch` in memoria: niente streaming.
//! Ogni scrittura è atomica ([`atomico`]): file temporaneo nella stessa
//! directory, verifica, rinomina; un percorso esistente non si sovrascrive
//! senza [`OpzioniScrittura::sovrascrivi`].
//!
//! [`esegui_da_file`] carica gli input nominati di un piano, lo valida e lo
//! esegue con il budget del piano, e scrive gli output nominati;
//! [`esegui_ingressi`], [`valida_ingressi`] ed [`esegui_in_memoria`]
//! accettano anche tabelle già in memoria ([`Ingresso`]).

pub mod atomico;
pub mod confine;
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

pub use confine::LimitiLettura;
pub use esecuzione::{
    budget_del_piano, esegui_da_file, esegui_da_file_interrompibile, esegui_in_memoria,
    esegui_ingressi, valida_da_file, valida_ingressi, EsitoFile, FileIngresso, FileUscita,
    Ingresso, UscitaScritta,
};
pub use formato::{CompressioneParquet, Formato, OpzioniScrittura};

/// Legge una tabella da file.
///
/// `formato` esplicito o dall'estensione; `residuo` è il budget in byte che
/// la tabella può occupare (`u64::MAX` per nessun limite): oltre, la lettura
/// si ferma con `ResourceLimit` (docs/file.md, «File», per ciò che si verifica
/// prima di decodificare e ciò che si verifica dopo).
///
/// # Errors
///
/// Quelli di [`leggi_tabella_con_limiti`].
pub fn leggi_tabella(
    percorso: &Path,
    formato: Option<Formato>,
    residuo: u64,
) -> Result<RecordBatch> {
    leggi_tabella_con_limiti(percorso, formato, residuo, &LimitiLettura::default())
}

/// Come [`leggi_tabella`], con i limiti del confine di lettura espliciti
/// (docs/file.md, «Confine di lettura»).
///
/// # Errors
///
/// Quelli di [`Formato::risolvi`], [`ipc::leggi`] e [`parquet_io::leggi`].
pub fn leggi_tabella_con_limiti(
    percorso: &Path,
    formato: Option<Formato>,
    residuo: u64,
    limiti: &LimitiLettura,
) -> Result<RecordBatch> {
    match Formato::risolvi(formato, percorso)? {
        // File o stream si riconoscono dal contenuto, con ogni estensione.
        Formato::ArrowIpc | Formato::ArrowIpcStream => ipc::leggi(percorso, residuo, limiti),
        Formato::Parquet => parquet_io::leggi(percorso, residuo, limiti),
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
    // (docs/runner.md, «Run-end e union rifiutati al confine»).
    plenora_core::contract::arrow_schema::verifica_tipi_supportati(&tabella.schema())?;
    // Uno schema con chiavi `plenora.*` si scrive solo se conforme al
    // confine: versione `1` e identità dei campi valide e uniche (docs/metadati-arrow.md,
    // «Metadati Arrow»). Uno schema senza chiavi `plenora.*` passa intatto.
    plenora_core::contract::arrow_schema::verifica_metadati_di_confine(&tabella.schema())?;
    match Formato::risolvi(opzioni.formato, percorso)? {
        Formato::ArrowIpc => atomico::scrivi_atomico(
            percorso,
            opzioni.sovrascrivi,
            |uscita| ipc::scrivi(tabella, uscita),
            |temporaneo| ipc::verifica_schema(temporaneo, &tabella.schema()),
        ),
        Formato::ArrowIpcStream => atomico::scrivi_atomico(
            percorso,
            opzioni.sovrascrivi,
            |uscita| ipc::scrivi_stream(tabella, uscita),
            |temporaneo| ipc::verifica_schema_stream(temporaneo, &tabella.schema()),
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
