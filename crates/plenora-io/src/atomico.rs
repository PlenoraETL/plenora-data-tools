//! Scrittura atomica: file temporaneo nella stessa directory, poi rinomina.
//!
//! Il contenuto si scrive in un file temporaneo accanto alla destinazione
//! (`tempfile`, nome casuale con prefisso `.plenora-io-`), si porta su disco
//! con `sync_all`, si verifica, e solo allora si rinomina sul percorso
//! finale. Un errore in qualunque punto prima della rinomina cancella il
//! temporaneo: la destinazione non vede mai un file parziale. Senza
//! `sovrascrivi` la rinomina è `persist_noclobber`, che fallisce se nel
//! frattempo qualcuno ha creato la destinazione; con `sovrascrivi` è una
//! rinomina che sostituisce.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use plenora_core::{PlenoraError, Result};

fn esiste_gia() -> PlenoraError {
    PlenoraError::Conflict(
        "la destinazione esiste gia': serve la sovrascrittura esplicita".to_owned(),
    )
}

/// Controllo anticipato: la destinazione non esiste (o si sovrascrive) e non
/// è una directory, e la directory che la conterrà esiste.
///
/// # Errors
///
/// `Conflict` se la destinazione esiste senza sovrascrittura; `Io` se è una
/// directory o la directory genitrice manca.
pub fn verifica_destinazione(destinazione: &Path, sovrascrivi: bool) -> Result<()> {
    match std::fs::symlink_metadata(destinazione) {
        Ok(metadati) if metadati.is_dir() => {
            return Err(PlenoraError::Io(std::io::Error::new(
                std::io::ErrorKind::IsADirectory,
                "la destinazione e' una directory",
            )));
        }
        Ok(_) if !sovrascrivi => return Err(esiste_gia()),
        Ok(_) => {}
        Err(errore) if errore.kind() == std::io::ErrorKind::NotFound => {}
        Err(errore) => return Err(errore.into()),
    }
    let genitrice = directory_di(destinazione);
    if !genitrice.is_dir() {
        return Err(PlenoraError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "la directory della destinazione non esiste",
        )));
    }
    Ok(())
}

fn directory_di(destinazione: &Path) -> &Path {
    match destinazione.parent() {
        Some(genitrice) if !genitrice.as_os_str().is_empty() => genitrice,
        _ => Path::new("."),
    }
}

/// Scrive `destinazione` in modo atomico.
///
/// `scrivi` riceve lo scrittore del temporaneo; `verifica` riceve il
/// percorso del temporaneo già completo e su disco, e può rileggerlo. Solo
/// se entrambi riescono il temporaneo diventa `destinazione`.
///
/// # Errors
///
/// Quelli di [`verifica_destinazione`], di `scrivi` e di `verifica`; `Io`
/// dalla creazione, dalla sincronizzazione e dalla rinomina; `Conflict` se
/// la destinazione è comparsa durante la scrittura senza sovrascrittura.
pub fn scrivi_atomico(
    destinazione: &Path,
    sovrascrivi: bool,
    scrivi: impl FnOnce(&mut BufWriter<&mut File>) -> Result<()>,
    verifica: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    verifica_destinazione(destinazione, sovrascrivi)?;
    let mut temporaneo = tempfile::Builder::new()
        .prefix(".plenora-io-")
        .suffix(".tmp")
        .tempfile_in(directory_di(destinazione))?;
    {
        let mut scrittore = BufWriter::new(temporaneo.as_file_mut());
        scrivi(&mut scrittore)?;
        scrittore.flush()?;
    }
    temporaneo.as_file().sync_all()?;
    verifica(temporaneo.path())?;
    let esito = if sovrascrivi {
        temporaneo.persist(destinazione)
    } else {
        temporaneo.persist_noclobber(destinazione)
    };
    // In caso di errore `PersistError` restituisce il temporaneo, che si
    // cancella quando cade.
    esito.map(|_| ()).map_err(|errore| {
        if errore.error.kind() == std::io::ErrorKind::AlreadyExists {
            esiste_gia()
        } else {
            PlenoraError::Io(errore.error)
        }
    })
}
