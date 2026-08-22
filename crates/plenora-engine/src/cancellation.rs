//! Token di cancellazione cooperativa (errori-e-limiti.md#cancellazione, Fase 2B).
//!
//! Decisione di dipendenza: valutata la crate `cancellation-token`
//! (piccola, senza `unsafe`), ma oggi il token e' un semplice flag
//! condiviso — nessuna attesa/notifica, nessuna gerarchia di token: il
//! runtime v1 e' seriale e il check e' un `load` atomico ai confini
//! dell'executor. La politica del workspace (punto unico di versione con
//! pin esatti; cfr. `temp_store`, che preferisce fallback conservativi a
//! nuove dipendenze) scoraggia una dipendenza per una primitiva banale:
//! `Arc<AtomicBool>` dietro un tipo dedicato copre il bisogno e lascia il
//! tipo libero di crescere (attesa, gerarchie) senza cambiare la
//! superficie pubblica.
//!
//! Oggi i kernel NON vedono il token — i check sono solo ai
//! confini dell'executor (tra batch nelle catene streaming, tra kernel,
//! durante il drenaggio dei segmenti blocking, sull'output del piano) e
//! onorano il `CancellationBehavior` dichiarato in catalogo. Il passaggio
//! del token ai kernel (check interni per le op `Cooperative` su batch
//! grandi) e' previsto con il runtime parallelo in M3.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Token di cancellazione cooperativa condiviso tra chiamante ed executor.
///
/// La clonazione condivide il flag (costo di un `Arc`); `Send + Sync`, cosi'
/// un handler di segnale o un thread esterno (es. Ctrl-C della CLI) puo'
/// cancellare mentre l'esecuzione procede.
///
/// Il default e' "mai cancellato": chi non ha interesse a cancellare
/// (API programmatica, test) puo' ignorare il tipo.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Token nuovo, non cancellato.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Richiede la cancellazione: idempotente e visibile subito a tutti i
    /// cloni. L'executor la osserva al prossimo confine cooperativo
    /// (errori-e-limiti.md#cancellazione: nessuna promessa di cancellazione
    /// immediata — un kernel `NonInterruptible` in corso completa prima
    /// dello stop).
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Il token e' stato cancellato?
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_token_is_never_cancelled() {
        assert!(!CancellationToken::new().is_cancelled());
        assert!(!CancellationToken::default().is_cancelled());
    }

    #[test]
    fn cancel_is_shared_across_clones_and_idempotent() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
        clone.cancel();
        assert!(token.is_cancelled());
    }
}
