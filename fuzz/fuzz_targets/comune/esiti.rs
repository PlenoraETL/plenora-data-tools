#![allow(dead_code)] // Ogni target usa solo una parte dei confronti.

//! Confronti sugli errori comuni ai target.
//!
//! Il testo di un errore non basta: `RowDiagnostics` delega il `Display` alla
//! causa e nasconde il payload strutturato, quindi due esecuzioni con
//! conteggi o esempi diversi avrebbero lo stesso testo.

use plenora_core::{ErrorCategory, PlenoraError};

/// Valore sentinella che i target mettono nelle celle: per la regola
/// «errori senza dati» non deve mai comparire in un errore, né nel testo né
/// nella diagnostica per riga.
pub const SENTINELLA: &str = "SENTINELLA-7f3a9c";

/// Due errori uguali su ogni asse osservabile.
pub fn stesso_errore(contesto: &str, a: &PlenoraError, b: &PlenoraError) {
    assert_eq!(a.to_string(), b.to_string(), "{contesto}");
    assert_eq!(a.category(), b.category(), "{contesto}");
    assert_eq!(a.phase(), b.phase(), "{contesto}");
    assert_eq!(a.row_diagnostics(), b.row_diagnostics(), "{contesto}");
}

/// Un errore ammesso: mai `Internal`, salvo quello documentato di una
/// dipendenza che va in panico dentro una barriera durante questo ingresso
/// (`barriere_prima` è [`crate::aggancio::panici_in_barriera`] letto
/// all'inizio); mai la sentinella.
pub fn errore_ammesso(contesto: &str, errore: &PlenoraError, barriere_prima: u64) {
    if errore.category() == ErrorCategory::Internal {
        assert!(
            crate::aggancio::panici_in_barriera() > barriere_prima,
            "{contesto}: Internal senza un panico di dipendenza in barriera: {errore}"
        );
    }
    senza_sentinella(contesto, errore);
}

/// Né il testo né la diagnostica per riga contengono [`SENTINELLA`].
pub fn senza_sentinella(contesto: &str, errore: &PlenoraError) {
    assert!(
        !errore.to_string().contains(SENTINELLA),
        "{contesto}: dato nel testo dell'errore"
    );
    if let Some(diagnostica) = errore.row_diagnostics() {
        assert!(
            !format!("{diagnostica:?}").contains(SENTINELLA),
            "{contesto}: dato nella diagnostica per riga"
        );
    }
}
