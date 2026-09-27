//! Il lettore limitato: **legge il prefisso, decide, poi alloca**.
//!
//! `decodifica` riceve una slice, cioe' byte gia' letti: da sola non impedisce
//! che un frame ostile faccia leggere un gigabyte da un canale. Solo chi legge
//! puo' decidere di **non** leggere.
//!
//! # L'ordine
//!
//! 1. si leggono esattamente [`BYTE_PREFISSO`] byte;
//! 2. si chiama [`lunghezza_dichiarata`], l'autorita' gia' provata — non una
//!    copia del confronto, che potrebbe divergere;
//! 3. **solo se accetta** si alloca — una volta sola, e in modo fallibile —
//!    e si legge.
//!
//! Un prefisso che dichiara `MAX + 1` fa consumare quattro byte e nient'altro;
//! lo prova un lettore-spia che conta i byte consumati.
//!
//! Generico su [`Read`] per provare la regola su sorgenti costruite ostili; il
//! canale reale lo portano worker e supervisore.

use std::io::{ErrorKind, Read};

use plenora_core::{PlenoraError, Result};

use super::codifica::{decodifica, lunghezza_dichiarata, BYTE_PREFISSO};
use super::messaggi::Frame;

/// I byte totali del frame: prefisso piu' payload dichiarato.
///
/// Funzione a se' perche' il test chiami la somma di produzione invece di
/// rifarla. Il traboccamento e' irraggiungibile finche' `lunghezza_dichiarata`
/// applica il tetto, ma la garanzia sta nel tetto, non qui.
fn totale_frame(dichiarata: usize) -> Result<usize> {
    BYTE_PREFISSO.checked_add(dichiarata).ok_or_else(|| {
        PlenoraError::Protocol(format!(
            "lunghezza del frame fuori intervallo: {dichiarata} byte piu' il prefisso"
        ))
    })
}

/// Legge un frame da una sorgente qualsiasi.
///
/// Rende `Ok(None)` **solo** se la sorgente finisce prima del primo byte del
/// prefisso, cioe' al confine fra due messaggi: la fine normale di una
/// conversazione, distinta da un prefisso troncato.
///
/// # Errors
///
/// - [`PlenoraError::Io`] se la sorgente fallisce, **conservato**: un guasto
///   del canale non e' una violazione del protocollo dell'altro capo;
/// - [`PlenoraError::Protocol`] per prefisso o payload troncato, lunghezza
///   oltre il tetto, e per tutto cio' che `decodifica` rifiuta.
pub fn leggi_frame<R: Read + ?Sized>(sorgente: &mut R) -> Result<Option<Frame>> {
    let Some(prefisso) = leggi_prefisso(sorgente)? else {
        return Ok(None);
    };

    // Il tetto **prima** di allocare: da qui in poi si sa quanto si legge, e
    // si sa che e' un numero che abbiamo accettato.
    let dichiarata = lunghezza_dichiarata(prefisso)?;

    // **Un** buffer, non due: un secondo `Vec` per il prefisso raddoppierebbe
    // cio' che il tetto concede. L'allocazione e' **fallibile**: su un numero
    // che arriva dall'altro capo, l'abort di `vec![0; n]` a memoria esaurita
    // e' la risposta sbagliata. Proprieta' non provata dalla suite (servirebbe
    // esaurire la memoria in modo portabile): la sorregge la firma di
    // `try_reserve_exact`, che rende un `Result`.
    let totale = totale_frame(dichiarata)?;
    let mut frame: Vec<u8> = Vec::new();
    frame.try_reserve_exact(totale).map_err(|_| {
        PlenoraError::ResourceLimit(format!(
            "memoria insufficiente per un frame di {totale} byte"
        ))
    })?;
    frame.extend_from_slice(&prefisso);
    // `resize` non rialloca: la capacita' e' gia' quella definitiva.
    frame.resize(totale, 0);

    leggi_esatti(sorgente, &mut frame[BYTE_PREFISSO..]).map_err(|origine| match origine {
        ErroreLettura::Io(errore) => PlenoraError::Io(errore),
        ErroreLettura::Troncato { letti } => PlenoraError::Protocol(format!(
            "payload troncato: dichiarati {dichiarata} byte, letti {letti}"
        )),
    })?;

    decodifica(&frame).map(Some)
}

/// I quattro byte del prefisso, o `None` se la sorgente e' gia' finita.
fn leggi_prefisso<R: Read + ?Sized>(sorgente: &mut R) -> Result<Option<[u8; BYTE_PREFISSO]>> {
    let mut prefisso = [0_u8; BYTE_PREFISSO];
    match leggi_esatti(sorgente, &mut prefisso) {
        Ok(()) => Ok(Some(prefisso)),
        Err(ErroreLettura::Io(errore)) => Err(PlenoraError::Io(errore)),
        // Zero byte prima del prefisso: la conversazione e' finita al confine
        // giusto.
        Err(ErroreLettura::Troncato { letti: 0 }) => Ok(None),
        Err(ErroreLettura::Troncato { letti }) => Err(PlenoraError::Protocol(format!(
            "prefisso troncato: letti {letti} byte, ne servono {BYTE_PREFISSO}"
        ))),
    }
}

/// Perche' una lettura esatta non e' riuscita.
///
/// Le due cause restano separate: «il canale si e' rotto» non e' «l'altro
/// capo ha violato il protocollo».
enum ErroreLettura {
    Io(std::io::Error),
    Troncato { letti: usize },
}

/// Riempie `destinazione` per intero, o dice quanti byte ha fatto in tempo a
/// leggere.
///
/// Non usa `Read::read_exact` perche' il suo `UnexpectedEof` non dice quanti
/// byte sono arrivati, e su un prefisso quel numero distingue la fine della
/// conversazione da un'interruzione.
fn leggi_esatti<R: Read + ?Sized>(
    sorgente: &mut R,
    destinazione: &mut [u8],
) -> std::result::Result<(), ErroreLettura> {
    let mut letti = 0_usize;
    while letti < destinazione.len() {
        match sorgente.read(&mut destinazione[letti..]) {
            Ok(0) => return Err(ErroreLettura::Troncato { letti }),
            Ok(quanti) => letti += quanti,
            // `Interrupted` e' un segnale arrivato durante la syscall, non un
            // guasto.
            Err(errore) if errore.kind() == ErrorKind::Interrupted => {}
            Err(errore) => return Err(ErroreLettura::Io(errore)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
