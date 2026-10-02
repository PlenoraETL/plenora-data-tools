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

use plenora_core::{PlenoraError, RemoteEffect, Result};

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
    let preparato = (|| {
        {
            let mut scrittore = BufWriter::new(temporaneo.as_file_mut());
            scrivi(&mut scrittore)?;
            scrittore.flush()?;
        }
        temporaneo.as_file().sync_all()?;
        verifica(temporaneo.path())
    })();
    if let Err(causa) = preparato {
        return Err(scarta(temporaneo, causa));
    }
    let esito = if sovrascrivi {
        temporaneo.persist(destinazione)
    } else {
        temporaneo.persist_noclobber(destinazione)
    };
    // In caso di errore `PersistError` restituisce il temporaneo.
    esito.map(|_| ()).map_err(|errore| {
        let causa = if errore.error.kind() == std::io::ErrorKind::AlreadyExists {
            esiste_gia()
        } else {
            PlenoraError::Io(errore.error)
        };
        scarta(errore.file, causa)
    })
}

/// Cancella il temporaneo di una scrittura fallita, controllando l'esito
/// (il `Drop` di `tempfile` lo ignora). Se la cancellazione fallisce, un
/// file con i dati resta accanto alla destinazione: l'errore lo dichiara con
/// effetto `unknown`, che **sostituisce** l'effetto che la causa dichiarava
/// (`override_remote_effect`: un `rolled_back` del chiamante non è più
/// vero), e il ritentativo non è più automatico.
fn scarta(temporaneo: tempfile::NamedTempFile, causa: PlenoraError) -> PlenoraError {
    match temporaneo.close() {
        Ok(()) => causa,
        Err(_) => causa.override_remote_effect(RemoteEffect::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use plenora_core::{PlenoraError, RemoteEffect, RetryDisposition};

    use super::scarta;

    fn temporaneo(dir: &tempfile::TempDir) -> tempfile::NamedTempFile {
        tempfile::Builder::new()
            .prefix(".plenora-io-")
            .tempfile_in(dir.path())
            .expect("temporaneo")
    }

    #[test]
    fn un_temporaneo_cancellato_lascia_l_errore_com_e() {
        let dir = tempfile::tempdir().expect("directory");
        let file = temporaneo(&dir);
        let percorso = file.path().to_path_buf();
        let errore = scarta(file, PlenoraError::Conflict("c".to_owned()));
        assert_eq!(errore.remote_effect(), RemoteEffect::None);
        assert!(!percorso.exists());
    }

    #[test]
    fn un_temporaneo_non_cancellabile_rende_l_effetto_ignoto() {
        // Cancellato da fuori prima di `close`: la cancellazione di `scarta`
        // fallisce, e l'errore non puo' piu' dire che non resta nulla.
        let dir = tempfile::tempdir().expect("directory");
        let file = temporaneo(&dir);
        std::fs::remove_file(file.path()).expect("cancellazione esterna");
        let causa = || PlenoraError::Io(std::io::Error::from(std::io::ErrorKind::Interrupted));
        let errore = scarta(file, causa());
        assert_eq!(errore.remote_effect(), RemoteEffect::Unknown);
        // Causa ritentabile: niente piu' ritentativo automatico.
        assert_eq!(causa().retry_disposition(), RetryDisposition::Safe);
        assert_eq!(
            errore.retry_disposition(),
            RetryDisposition::RequiresRecovery
        );
        let pubblico = errore.public_projection();
        assert_eq!(pubblico.remote_effect(), RemoteEffect::Unknown);
        assert_eq!(pubblico.retry(), RetryDisposition::RequiresRecovery);
    }

    #[test]
    fn la_pulizia_fallita_smentisce_l_effetto_del_chiamante() {
        // Il chiamante dichiara `rolled_back` su una causa ritentabile; la
        // verifica cancella il temporaneo da fuori, cosi' la cancellazione
        // di `scarta` fallisce: l'effetto diventa `unknown`.
        let dir = tempfile::tempdir().expect("directory");
        let destinazione = dir.path().join("t.bin");
        let errore = super::scrivi_atomico(
            &destinazione,
            false,
            |scrittore| std::io::Write::write_all(scrittore, b"dati").map_err(PlenoraError::from),
            |percorso| {
                std::fs::remove_file(percorso).expect("cancellazione esterna");
                Err(PlenoraError::Timeout("t".to_owned())
                    .with_remote_effect(RemoteEffect::RolledBack))
            },
        )
        .expect_err("verifica fallita");
        assert_eq!(errore.remote_effect(), RemoteEffect::Unknown);
        assert_eq!(
            errore.retry_disposition(),
            RetryDisposition::RequiresRecovery
        );
        assert_eq!(
            errore.public_projection().remote_effect(),
            RemoteEffect::Unknown
        );
        assert!(!destinazione.exists());
        // Una causa che non si ritenta mai resta `never`.
        let mai = PlenoraError::Conflict("c".to_owned())
            .with_remote_effect(RemoteEffect::RolledBack)
            .override_remote_effect(RemoteEffect::Unknown);
        assert_eq!(mai.remote_effect(), RemoteEffect::Unknown);
        assert_eq!(mai.retry_disposition(), RetryDisposition::Never);
    }
}
