//! Arrow IPC: lettura di file (Feather v2) e stream, scrittura di file.
//!
//! La lettura riconosce file e stream dal contenuto (il file comincia con
//! `ARROW1`), legge tutti i blocchi e li ricompone in un solo `RecordBatch`
//! con lo schema del file, metadati di schema e di campo compresi. Con un
//! solo blocco le colonne restano viste del buffer letto (nessuna copia);
//! con più blocchi `concat_batches` li copia.
//!
//! Memoria: i buffer letti sono al più i byte del file (i dati IPC senza
//! compressione si leggono per viste), e la ricomposizione di più blocchi
//! ne tiene insieme due copie. Prima di leggere si verifica che la
//! dimensione del file (il doppio, se i blocchi sono più di uno) stia nel
//! budget residuo; dopo, i byte vivi esatti.
//!
//! La scrittura procede a blocchi di righe di circa [`BYTE_PER_BLOCCO`]
//! byte, come lo sfratto del runner: `FileWriter` codifica ogni blocco in un
//! vettore prima di scriverlo, e blocchi limitati limitano quel transitorio.
//! Nessuna compressione: i crate Arrow del workspace non la abilitano, e un
//! file IPC compresso si rifiuta in lettura con l'errore di Arrow.

use plenora_core::contract::arrow_schema::verifica_tipi_supportati;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::reader::{FileReader, StreamReader};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::memoria::byte_vivi;
use plenora_core::{PlenoraError, Result};

use crate::memoria::{oltre_il_budget, stima_byte};

/// Byte di dati per blocco scritto.
pub const BYTE_PER_BLOCCO: u64 = 8 * 1024 * 1024;

const MAGIA_FILE: &[u8; 6] = b"ARROW1";
const MAGIA_FEATHER_V1: &[u8; 4] = b"FEA1";

/// Ricompone i blocchi letti in un batch solo, con lo schema del file.
fn ricomponi(schema: &SchemaRef, blocchi: Vec<RecordBatch>) -> Result<RecordBatch> {
    match blocchi.len() {
        0 => Ok(RecordBatch::new_empty(schema.clone())),
        1 => blocchi
            .into_iter()
            .next()
            .ok_or_else(|| PlenoraError::Internal("blocco atteso e assente".to_owned())),
        _ => Ok(concat_batches(schema, &blocchi)?),
    }
}

/// Legge un file Arrow IPC (file o stream) in un solo `RecordBatch`.
///
/// `residuo` è il budget che la tabella può occupare.
///
/// # Errors
///
/// `ResourceLimit` se il file o la tabella letta non stanno in `residuo`;
/// `Unsupported` per Feather v1; `Io`, `DataMapping` (Arrow) dalla lettura.
pub fn leggi(percorso: &Path, residuo: u64) -> Result<RecordBatch> {
    let mut file = File::open(percorso)?;
    let lunghezza = file.metadata()?.len();
    let mut inizio = Vec::with_capacity(MAGIA_FILE.len());
    Read::by_ref(&mut file)
        .take(MAGIA_FILE.len() as u64)
        .read_to_end(&mut inizio)?;
    file.seek(SeekFrom::Start(0))?;
    if inizio.starts_with(MAGIA_FEATHER_V1) {
        return Err(PlenoraError::Unsupported(
            "Feather v1 non supportato: solo Feather v2 (Arrow IPC)".to_owned(),
        ));
    }
    let (schema, blocchi) = if inizio == MAGIA_FILE {
        let lettore = FileReader::try_new(BufReader::new(file), None)?;
        let quanti = lettore.num_batches();
        let fattore = if quanti > 1 { 2 } else { 1 };
        if lunghezza.saturating_mul(fattore) > residuo {
            return Err(oltre_il_budget(lunghezza.saturating_mul(fattore), residuo));
        }
        let schema = lettore.schema();
        verifica_tipi_supportati(&schema)?;
        let blocchi = lettore.collect::<std::result::Result<Vec<_>, _>>()?;
        (schema, blocchi)
    } else {
        if lunghezza > residuo {
            return Err(oltre_il_budget(lunghezza, residuo));
        }
        let lettore = StreamReader::try_new(BufReader::new(file), None)?;
        let schema = lettore.schema();
        verifica_tipi_supportati(&schema)?;
        let blocchi = lettore.collect::<std::result::Result<Vec<_>, _>>()?;
        if blocchi.len() > 1 {
            let letti = byte_vivi(blocchi.iter())?;
            let stima = letti.saturating_add(stima_byte(&blocchi));
            if stima > residuo {
                return Err(oltre_il_budget(stima, residuo));
            }
        }
        (schema, blocchi)
    };
    let tabella = ricomponi(&schema, blocchi)?;
    let vivi = byte_vivi(std::iter::once(&tabella))?;
    if vivi > residuo {
        return Err(oltre_il_budget(vivi, residuo));
    }
    Ok(tabella)
}

/// Righe per blocco: circa [`BYTE_PER_BLOCCO`] byte di dati ciascuno.
fn righe_per_blocco(tabella: &RecordBatch) -> usize {
    let righe = tabella.num_rows();
    let dati = u64::try_from(plenora_core::memoria::byte_dati(tabella))
        .unwrap_or(u64::MAX)
        .max(1);
    let per_blocco = u128::from(BYTE_PER_BLOCCO)
        * u128::from(u64::try_from(righe).unwrap_or(u64::MAX).max(1))
        / u128::from(dati);
    usize::try_from(per_blocco).unwrap_or(usize::MAX).max(1)
}

/// Scrive la tabella come file Arrow IPC.
///
/// # Errors
///
/// `DataMapping` (Arrow) e `Io` dalla codifica e dalla scrittura.
pub fn scrivi(tabella: &RecordBatch, uscita: impl Write) -> Result<()> {
    let mut scrittore = FileWriter::try_new(uscita, &tabella.schema())?;
    let righe = tabella.num_rows();
    if tabella.num_columns() == 0 {
        // Senza colonne il blocco porta solo il numero di righe.
        if righe > 0 {
            scrittore.write(tabella)?;
        }
    } else {
        let per_blocco = righe_per_blocco(tabella);
        let mut inizio = 0;
        while inizio < righe {
            let lunghezza = per_blocco.min(righe - inizio);
            scrittore.write(&tabella.slice(inizio, lunghezza))?;
            inizio += lunghezza;
        }
    }
    scrittore.finish()?;
    Ok(())
}

/// Transitorio previsto della scrittura: il doppio del blocco più grande
/// (il vettore di codifica cresce per raddoppi), misurato sui blocchi veri
/// con i valori dei dizionari interi, più un margine.
#[must_use]
pub fn transitorio_scrittura(tabella: &RecordBatch) -> u64 {
    crate::memoria::fetta_massima(tabella, righe_per_blocco(tabella))
        .saturating_mul(2)
        .saturating_add(crate::memoria::MARGINE)
}

/// Rilegge il footer di un file appena scritto: lo schema deve essere quello
/// scritto.
///
/// # Errors
///
/// `Schema` se lo schema del file è diverso; `Io`, `DataMapping` dalla
/// lettura del footer.
pub fn verifica_schema(percorso: &Path, scritto: &SchemaRef) -> Result<()> {
    let lettore = FileReader::try_new(BufReader::new(File::open(percorso)?), None)?;
    if lettore.schema() != *scritto {
        return Err(PlenoraError::Schema(
            "schema del file IPC scritto diverso da quello della tabella".to_owned(),
        ));
    }
    Ok(())
}
