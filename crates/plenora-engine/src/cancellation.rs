//! Token di cancellazione cooperativa (errori-e-limiti.md#cancellazione).
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

    /// Il flag atomico condiviso, per un installatore che deve scrivere
    /// **lo stesso bit** che [`is_cancelled`](Self::is_cancelled) legge,
    /// senza passare da [`cancel`](Self::cancel) — il caso d'uso e' un
    /// gestore di segnale esterno (`signal_hook::flag::register`, sul
    /// percorso isolato di `isolamento::esecuzione_isolata`) che riceve un
    /// `Arc<AtomicBool>` da condividere con una libreria, non un
    /// `&CancellationToken` da chiamare.
    ///
    /// Il clone condivide l'`Arc`: scrivere nel flag restituito e' visibile
    /// a questo token e a tutti i suoi cloni, esattamente come
    /// [`cancel`](Self::cancel). `signal_hook::flag::register` scrive con
    /// `Ordering::SeqCst` — piu' forte di `Ordering::Release` che `cancel`
    /// usa, e compatibile con l'`Ordering::Acquire` di
    /// [`is_cancelled`](Self::is_cancelled).
    #[must_use]
    pub fn condividi_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.flag)
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

    /// Il flag condiviso e' lo **stesso** `Arc`: scriverci direttamente
    /// (come farebbe `signal_hook::flag::register`, senza passare da
    /// `cancel`) e' visibile a `is_cancelled` — il caso d'uso reale di
    /// `condividi_flag`.
    #[test]
    fn condividi_flag_scrive_lo_stesso_bit_che_is_cancelled_legge() {
        let token = CancellationToken::new();
        let bit = token.condividi_flag();
        assert!(!token.is_cancelled());

        bit.store(true, Ordering::SeqCst);
        assert!(
            token.is_cancelled(),
            "una scrittura diretta sul flag condiviso deve essere visibile a is_cancelled"
        );
    }

    /// Il flag condiviso resta lo stesso `Arc` anche attraverso un clone del
    /// token — non una copia indipendente per ciascun clone.
    #[test]
    fn condividi_flag_e_lo_stesso_arc_su_ogni_clone() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(Arc::ptr_eq(
            &token.condividi_flag(),
            &clone.condividi_flag()
        ));
    }
}
