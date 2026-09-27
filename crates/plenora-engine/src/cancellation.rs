//! Token di cancellazione cooperativa (errori-e-limiti.md#cancellazione).
//!
//! Un `Arc<AtomicBool>` dietro un tipo dedicato: basta un flag condiviso, e
//! il tipo puo' crescere (attesa, gerarchie) senza cambiare la superficie
//! pubblica. Nessuna dipendenza esterna per una primitiva banale.
//!
//! I kernel non vedono il token: i check stanno ai confini dell'executor (fra
//! batch, fra kernel, nel drenaggio dei segmenti blocking, sull'output) e
//! onorano il `CancellationBehavior` del catalogo. I check interni ai kernel
//! arrivano con il runtime parallelo (M3).

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

    /// Il flag atomico condiviso, per chi deve scrivere lo stesso bit che
    /// [`is_cancelled`](Self::is_cancelled) legge senza chiamare
    /// [`cancel`](Self::cancel).
    ///
    /// Serve a un gestore di segnale esterno (`signal_hook::flag::register`)
    /// che vuole un `Arc<AtomicBool>`. Scrivere nel flag equivale a `cancel`
    /// per questo token e tutti i suoi cloni; `SeqCst` e' compatibile con
    /// l'`Acquire` di `is_cancelled`.
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
