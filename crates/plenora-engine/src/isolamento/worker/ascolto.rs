//! Ascoltare l'annullamento **mentre** si lavora, e smettere quando si vuole.
//!
//! # Perche' serve un secondo lettore
//!
//! Perche' l'`Annulla` arriva quando il worker sta gia' eseguendo, e chi
//! esegue non e' in ascolto: e' dentro l'executor, che consuma batch. Un
//! `Annulla` letto solo alla fine sarebbe letto quando non serve piu'.
//!
//! # Perche' e' un thread e non un giro nel mezzo del lavoro
//!
//! Perche' il lavoro non ha un punto in cui passare: l'esecuzione e' uno
//! stream tirato dalla scrittura dell'artefatto, e infilarci un controllo del
//! canale vorrebbe dire far conoscere il protocollo a chi scrive i byte. La
//! cancellazione, che invece l'executor **gia'** osserva ai propri confini
//! cooperativi, e' l'unico canale che i due condividono — e questo lettore non
//! fa altro che tirare quella leva.
//!
//! # Perche' si puo' fermare, e perche' e' obbligatorio
//!
//! Un lettore fermo dentro una `read` non si sveglia perche' altrove il lavoro
//! e' finito: aspetta byte che non arrivano. Se non lo si ferma, il processo
//! non esce — e un worker che non esce e' esattamente cio' che il supervisore
//! deve poi uccidere, cioe' il caso peggiore.
//!
//! Per questo il tipo non ha un cammino in cui il thread resti indietro: si
//! ferma e si raccoglie con una funzione sola, che consuma [`Ascolto`].
//!
//! # Che cosa non fa
//!
//! **Non decide.** Rende un [`Ascoltato`] e basta: che un fatto sia un guasto
//! o un accadimento previsto lo stabilisce chi ha in mano anche l'esito del
//! lavoro, perche' e' l'unico che li puo' mettere insieme.

use std::io::Read;
use std::os::fd::AsFd;

use plenora_core::error::PlenoraError;

use crate::cancellation::CancellationToken;
use crate::isolamento::sorgente::{
    interruttore, rendi_non_bloccante, Freno, SorgenteTerminabile, PASSO_DI_ATTESA,
};
use crate::protocollo::assi::forma_sul_filo;
use crate::protocollo::lettore::leggi_frame;
use crate::protocollo::messaggi::{Corpo, FormaPanicSulFilo, TipoMessaggio};

use super::Result;

/// Che cosa l'ascolto ha visto, prima di essere fermato.
///
/// # Perche' un enum e non un `Result`
///
/// Perche' non sono due casi ma sei, e tre di essi non sono ne' successi ne'
/// fallimenti: sono **accadimenti**. Un `Result` costringerebbe a scegliere da
/// che parte metterli, e chi legge si troverebbe un EOF classificato come
/// errore o un guasto classificato come esito normale.
///
/// Essendo un enum chiuso, chi lo riceve deve nominarli tutti: e' cio' che
/// impedisce a uno di essi di sparire in un `_ => {}`.
#[derive(Debug)]
pub(super) enum Ascoltato {
    /// E' arrivato un `Annulla`, e il token e' stato cancellato.
    Annullamento,
    /// Il supervisore ha chiuso la propria direzione.
    ///
    /// **Non e' un guasto.** Un supervisore che non intende annullare puo'
    /// chiudere dopo l'`Incarico`, e allora l'EOF dice soltanto che nessun
    /// annullamento potra' piu' arrivare.
    FineDelCanale,
    /// Si e' fermato su richiesta, senza aver visto niente: il lavoro e'
    /// finito prima.
    Fermato,
    /// E' arrivato un messaggio che non e' un `Annulla`.
    ///
    /// Dopo l'`Incarico` non c'e' nient'altro da dire, quindi qualunque altro
    /// tipo e' una violazione della sequenza.
    FuoriSequenza(TipoMessaggio),
    /// Il canale non regge, oppure il frame non e' un frame.
    Guasto(PlenoraError),
    /// Il lettore stesso e' andato in panico.
    ///
    /// La forma del payload, non il payload: il contenuto non esce di qui, e
    /// nemmeno da chi lo riceve.
    Panico(FormaPanicSulFilo),
}

/// Un lettore che ascolta l'annullamento finche' non lo si ferma.
#[derive(Debug)]
pub(super) struct Ascolto {
    freno: Freno,
    mano: std::thread::JoinHandle<Ascoltato>,
}

impl Ascolto {
    /// Comincia ad ascoltare.
    ///
    /// # Un frame solo, e perche' basta
    ///
    /// Il lettore legge **un** frame e finisce. Dopo l'`Incarico` il
    /// protocollo ammette al piu' un `Annulla`, e un secondo messaggio non
    /// avrebbe uno stato in cui arrivare: leggerne altri vorrebbe dire tenere
    /// vivo un lettore per un caso che non esiste.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::IsolationUnavailable`] se il descrittore non si mette
    /// in modalita' non bloccante, o se il thread non nasce. Senza modalita'
    /// non bloccante il lettore non sarebbe fermabile, e cominciare comunque
    /// darebbe un ascolto che nessuno puo' concludere.
    pub(super) fn comincia<R: Read + AsFd + Send + 'static>(
        canale: R,
        annullamento: CancellationToken,
    ) -> Result<Self> {
        rendi_non_bloccante(canale.as_fd())?;
        let (interruttore, freno) = interruttore();
        let mano = std::thread::Builder::new()
            .name("plenora-ascolto-annulla".to_owned())
            .spawn(move || {
                let mut sorgente =
                    SorgenteTerminabile::con_interruttore(canale, PASSO_DI_ATTESA, interruttore);
                ascolta(&mut sorgente, &annullamento)
            })
            .map_err(|causa| {
                super::non_disponibile(
                    "annullamento",
                    &format!("il lettore dell'annullamento non nasce: {causa}"),
                )
            })?;
        Ok(Self { freno, mano })
    }

    /// Ferma il lettore e ne raccoglie l'esito.
    ///
    /// Consuma `self`: non esiste un cammino in cui si fermi senza essere
    /// raccolto, ne' uno in cui si raccolga due volte.
    ///
    /// # Perche' non rende un `Result`
    ///
    /// Perche' non c'e' niente che possa fallire: un `join` su un thread che
    /// e' andato in panico non e' un fallimento di questa funzione, e' un
    /// fatto sul lettore — e ha la sua variante.
    pub(super) fn ferma_e_raccogli(self) -> Ascoltato {
        self.freno.ferma();
        self.mano.join().unwrap_or_else(|payload| {
            // Il payload non si legge: se ne prende la **forma**, che e' un
            // enum chiuso di tre valori e non porta con se' nessun byte del
            // messaggio del panico.
            Ascoltato::Panico(forma_sul_filo(payload.as_ref()))
        })
    }
}

/// Legge un frame, e dice che cos'e'.
///
/// # Perche' l'arresto si riconosce dall'interruttore e non dal messaggio
///
/// Perche' la sorgente rende un errore di I/O ordinario quando la si ferma, e
/// distinguerlo dal testo vorrebbe dire confrontare stringhe: chi un giorno
/// riscrive quel messaggio trasforma silenziosamente un nostro arresto in un
/// guasto del canale. Chi ha chiesto l'arresto lo sa, e glielo si chiede.
fn ascolta<R: Read>(
    sorgente: &mut SorgenteTerminabile<R>,
    annullamento: &CancellationToken,
) -> Ascoltato {
    match leggi_frame(sorgente) {
        Ok(Some(frame)) => {
            // Il tipo si prende dal frame, che lo **deriva** dal corpo:
            // nominare le varianti qui darebbe due risposte alla stessa
            // domanda.
            let tipo = frame.tipo();
            if matches!(frame.corpo(), Corpo::Annulla(_)) {
                // La leva, e l'unica cosa che questo lettore fa al lavoro.
                // L'executor la osserva ai propri confini cooperativi: da qui
                // in poi il lavoro finisce da se', senza che il supervisore
                // debba forzare niente.
                annullamento.cancel();
                Ascoltato::Annullamento
            } else {
                Ascoltato::FuoriSequenza(tipo)
            }
        }
        Ok(None) => Ascoltato::FineDelCanale,
        Err(causa) => {
            if sorgente.fermato() {
                Ascoltato::Fermato
            } else {
                Ascoltato::Guasto(causa)
            }
        }
    }
}

#[cfg(test)]
mod tests;
