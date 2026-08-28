//! Il lettore limitato: **legge il prefisso, decide, poi alloca**.
//!
//! # Perche' esiste, dato che `decodifica` esiste gia'
//!
//! `decodifica` riceve una slice: quando viene chiamata i byte del payload
//! qualcuno li ha gia' letti. Il suo rifiuto e' sincero su di se' — non alloca
//! e non guarda oltre i quattro byte — ma da solo non impedisce che un frame
//! ostile faccia leggere un gigabyte da un canale.
//!
//! Quella difesa e' qui, ed e' l'unico posto in cui puo' stare: solo chi legge
//! puo' decidere di **non** leggere.
//!
//! # L'ordine, che e' tutto
//!
//! 1. si leggono esattamente [`BYTE_PREFISSO`] byte;
//! 2. si chiama [`lunghezza_dichiarata`], l'autorita' gia' provata — non una
//!    copia del confronto, che potrebbe divergere;
//! 3. **solo se accetta** si alloca — una volta sola, e in modo fallibile —
//!    e si legge.
//!
//! Un prefisso che dichiara `MAX + 1` fa consumare quattro byte e nient'altro.
//! Non e' una conseguenza da dedurre leggendo il codice: e' provata con un
//! lettore-spia che conta i byte consumati.
//!
//! # Generico su [`Read`], deliberatamente
//!
//! Nessun pipe concreto: il canale reale arriva col worker e col supervisore.
//! Qui c'e' la regola di lettura, e la si prova su sorgenti costruite apposta
//! per essere ostili — cosa che con un pipe vero sarebbe molto piu' difficile.

use std::io::{ErrorKind, Read};

use plenora_core::{PlenoraError, Result};

use super::codifica::{decodifica, lunghezza_dichiarata, BYTE_PREFISSO};
use super::messaggi::Frame;

/// I byte totali del frame: prefisso piu' payload dichiarato.
///
/// Sta in una funzione sua per poterla **chiamare** in un test invece di
/// riscrivere `checked_add` accanto all'asserzione: un test che rifa' il
/// calcolo prova il calcolo del test, e resterebbe verde anche se la somma di
/// produzione sparisse.
///
/// Il ramo di traboccamento e' irraggiungibile per il chiamante vero —
/// `lunghezza_dichiarata` ha gia' respinto tutto cio' che supera il tetto —
/// ma esiste perche' la garanzia sta nel tetto, non qui, e un giorno il tetto
/// potrebbe cambiare senza che nessuno ripassi da questa riga.
fn totale_frame(dichiarata: usize) -> Result<usize> {
    BYTE_PREFISSO.checked_add(dichiarata).ok_or_else(|| {
        PlenoraError::Protocol(format!(
            "lunghezza del frame fuori intervallo: {dichiarata} byte piu' il prefisso"
        ))
    })
}

/// Legge un frame da una sorgente qualsiasi.
///
/// Rende `Ok(None)` **solo** se la sorgente e' finita in modo pulito prima del
/// primo byte del prefisso, cioe' al confine fra un messaggio e il successivo.
/// E' una condizione diversa da un prefisso troncato, e tenerle separate
/// conta: la prima e' la fine normale di una conversazione, la seconda e' un
/// interlocutore che si e' interrotto a meta' parola.
///
/// # Errors
///
/// - [`PlenoraError::Io`] se la sorgente fallisce, **conservato**: un guasto
///   del canale non e' una violazione del protocollo, e riclassificarlo come
///   tale direbbe che ha sbagliato l'altro capo quando invece si e' rotto il
///   filo;
/// - [`PlenoraError::Protocol`] per prefisso o payload troncato, lunghezza
///   oltre il tetto, e per tutto cio' che `decodifica` rifiuta.
pub fn leggi_frame<R: Read + ?Sized>(sorgente: &mut R) -> Result<Option<Frame>> {
    let Some(prefisso) = leggi_prefisso(sorgente)? else {
        return Ok(None);
    };

    // Il tetto **prima** di allocare: da qui in poi si sa quanto si legge, e
    // si sa che e' un numero che abbiamo accettato.
    let dichiarata = lunghezza_dichiarata(prefisso)?;

    // **Un** buffer, non due.
    //
    // Allocare il payload e poi un secondo `Vec` per rimetterci davanti il
    // prefisso costerebbe, al limite, due volte ~64 MiB: il doppio di cio' che
    // il tetto concede. Un tetto smette di essere un tetto se chi lo rispetta
    // alloca due volte.
    //
    // E l'allocazione e' **fallibile**. `vec![0; n]` aborta il processo se il
    // sistema non ha memoria: su un numero che arriva dall'altro capo del
    // canale, un abort e' la risposta sbagliata — e non e' nemmeno un errore
    // che qualcuno possa classificare, perche' il processo non c'e' piu'.
    //
    // Questa proprieta' **non e' provata dalla suite**, e va detto invece di
    // lasciarlo credere: la differenza fra `try_reserve_exact` e
    // `reserve_exact` si manifesta solo a memoria esaurita, e un test non puo'
    // esaurirla in modo portabile. Cio' che la sorregge e' la firma —
    // `try_reserve_exact` rende un `Result`, quindi il fallimento non si puo'
    // ignorare senza scriverlo.
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
/// Le due cause vanno tenute separate fin qui: piu' in su diventerebbero lo
/// stesso errore, e «il canale si e' rotto» finirebbe scritto come «l'altro
/// capo ha violato il protocollo».
enum ErroreLettura {
    Io(std::io::Error),
    Troncato { letti: usize },
}

/// Riempie `destinazione` per intero, o dice quanti byte ha fatto in tempo a
/// leggere.
///
/// Scritta a mano invece di `Read::read_exact` per una ragione sola: quella
/// rende un `io::Error` di `UnexpectedEof` che **non dice quanti byte sono
/// arrivati**. Su un prefisso quel numero e' la differenza fra «la
/// conversazione e' finita» e «si e' interrotta a meta'».
fn leggi_esatti<R: Read + ?Sized>(
    sorgente: &mut R,
    destinazione: &mut [u8],
) -> std::result::Result<(), ErroreLettura> {
    let mut letti = 0_usize;
    while letti < destinazione.len() {
        match sorgente.read(&mut destinazione[letti..]) {
            Ok(0) => return Err(ErroreLettura::Troncato { letti }),
            Ok(quanti) => letti += quanti,
            // `Interrupted` non e' un guasto: e' un segnale arrivato durante
            // la syscall. Trattarlo da errore farebbe fallire una lettura
            // legittima ogni volta che il processo riceve un segnale.
            Err(errore) if errore.kind() == ErrorKind::Interrupted => {}
            Err(errore) => return Err(ErroreLettura::Io(errore)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
