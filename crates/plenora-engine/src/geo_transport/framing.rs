//! Le primitive di incorniciatura dei tre stream checksummati.
//!
//! `PLNGEO2`, `PLNGEO3` e `PLNPAIR1` condividono l'ossatura: sedici byte di
//! header (magic piu' un contatore) e una chiusura fatta di trailer, digest e
//! fine dello stream. Sta qui in un esemplare solo, perche' un confine
//! fail-closed copiato si corregge in alcune copie e non in tutte.
//!
//! Gli errori no: ogni stream conserva il proprio vocabolario e queste
//! funzioni dicono solo **che cosa** non torna. Resta fuori il framing di
//! Arrow (`ipc.rs`), che ha altro formato, altri limiti e altri errori.

use std::io::{self, Read, Write};

/// Header di sedici byte: magic e contatore `u64` little-endian.
pub(super) fn header(magic: [u8; 8], contatore: u64) -> [u8; 16] {
    let mut header = [0_u8; 16];
    header[..8].copy_from_slice(&magic);
    header[8..].copy_from_slice(&contatore.to_le_bytes());
    header
}

/// Il contatore dichiarato da un header gia' letto, se il magic corrisponde.
///
/// Rende `None` sul magic sbagliato invece di un errore proprio: quale errore
/// sia lo decide chi legge, e i tre stream lo chiamano in tre modi diversi.
pub(super) fn contatore_dichiarato(header: &[u8; 16], magic: [u8; 8]) -> Option<u64> {
    if header[..8] != magic {
        return None;
    }
    let mut contatore = [0_u8; 8];
    contatore.copy_from_slice(&header[8..]);
    Some(u64::from_le_bytes(contatore))
}

/// Scrive trailer e digest, poi svuota.
///
/// # Errors
///
/// L'errore del writer sottostante, non riclassificato.
pub(super) fn scrivi_chiusura<W: Write>(
    destinazione: &mut W,
    magic: [u8; 8],
    digest: &[u8; 32],
) -> io::Result<()> {
    destinazione.write_all(&magic)?;
    destinazione.write_all(digest)?;
    destinazione.flush()
}

/// Perche' la chiusura di uno stream checksummato non e' valida.
pub(super) enum DifettoChiusura {
    Io(io::Error),
    Trailer,
    Checksum,
    ByteResidui,
}

/// Legge il trailer, confronta il digest e pretende che lo stream finisca li'.
///
/// I byte dopo il trailer sono un difetto, non un dettaglio: uno stream che
/// continua e' uno stream di cui non si e' verificato tutto.
///
/// # Errors
///
/// [`DifettoChiusura`], che il chiamante traduce nel proprio errore.
pub(super) fn verifica_chiusura<R: Read>(
    sorgente: &mut R,
    magic: [u8; 8],
    digest_calcolato: &[u8; 32],
) -> Result<(), DifettoChiusura> {
    let mut trailer = [0_u8; 8];
    sorgente
        .read_exact(&mut trailer)
        .map_err(DifettoChiusura::Io)?;
    if trailer != magic {
        return Err(DifettoChiusura::Trailer);
    }
    let mut atteso = [0_u8; 32];
    sorgente
        .read_exact(&mut atteso)
        .map_err(DifettoChiusura::Io)?;
    if &atteso != digest_calcolato {
        return Err(DifettoChiusura::Checksum);
    }
    let mut residuo = [0_u8; 1];
    if sorgente.read(&mut residuo).map_err(DifettoChiusura::Io)? != 0 {
        return Err(DifettoChiusura::ByteResidui);
    }
    Ok(())
}
