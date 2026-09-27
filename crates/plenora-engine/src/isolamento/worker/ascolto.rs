//! Ascoltare l'annullamento **mentre** si lavora, e smettere quando si vuole.
//!
//! L'`Annulla` arriva mentre il worker e' dentro l'executor, che non ascolta
//! il canale e non deve conoscere il protocollo. L'unica leva comune e' la
//! cancellazione, che l'executor gia' osserva ai propri confini cooperativi:
//! questo lettore, su un thread suo, la tira.
//!
//! Il lettore si ferma obbligatoriamente: fermo in una `read`, impedirebbe al
//! processo di uscire. Si ferma e si raccoglie con una funzione sola, che
//! consuma [`Ascolto`].
//!
//! **Non decide**: rende un [`Ascoltato`], e se sia un guasto lo stabilisce
//! chi ha in mano anche l'esito del lavoro.

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
/// Un enum chiuso e non un `Result`: alcuni casi sono **accadimenti**, ne'
/// successi ne' fallimenti, e chi lo riceve deve nominarli tutti.
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
    /// Il lettore legge **un** frame e finisce: dopo l'`Incarico` il
    /// protocollo ammette al piu' un `Annulla`.
    ///
    /// # Errors
    ///
    /// [`PlenoraError::IsolationUnavailable`] se il descrittore non si mette
    /// in modalita' non bloccante, o se il thread non nasce: senza, il lettore
    /// non sarebbe fermabile.
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
    /// Consuma `self`: non si ferma senza raccogliere, ne' si raccoglie due
    /// volte. Non rende un `Result`: un lettore andato in panico e' un fatto
    /// con la sua variante, non un fallimento di questa funzione.
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
/// L'arresto si riconosce dall'interruttore, non dal testo dell'errore di
/// I/O: riscrivere quel messaggio trasformerebbe in silenzio un nostro
/// arresto in un guasto del canale.
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
