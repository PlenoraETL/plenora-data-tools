//! Che cosa succede alla CATEGORIA di un errore quando gli si aggiunge il
//! contesto del passo.
//!
//! Quando un passo fallisce, chi propaga l'errore vi aggiunge nodo e
//! operazione. Il modo storico era avvolgere tutto in
//! [`PlenoraError::Execution`], che pero' **sostituisce** la categoria: ogni
//! errore diventava `execution`, exit 6, retry `Never`.
//!
//! ## Perche' una lista non poteva funzionare
//!
//! La prima versione di questo modulo aveva un elenco di categorie
//! «preservate»: tre su diciotto. Era la forma sbagliata del problema, per
//! due ragioni.
//!
//! **Cancellava decisioni.** [`ErrorCategory::Io`] ha disposizione di
//! ritentativo `Safe`: un errore di I/O durante lo spill e' ritentabile, e
//! trasformarlo in `execution` lo rendeva `Never` — cioe' diceva al
//! chiamante di non riprovare una cosa che si poteva riprovare. Lo stesso
//! vale per `Timeout`, `Transient`, `Authentication`, `Authorization`,
//! `NotFound`, `Conflict`: sono tutte categorie su cui un chiamante fa
//! qualcosa di specifico, e la lista le buttava via.
//!
//! **Era destinata a restare indietro.** Una lista di eccezioni va aggiornata
//! ogni volta che si aggiunge una categoria, e nessuno se ne ricorda: quella
//! di tre elementi ne aveva gia' dimenticate quindici.
//!
//! ## La regola
//!
//! Si inverte il default. Un errore che porta gia' una classificazione la
//! **conserva**, e il contesto del passo gli viene aggiunto tramite
//! [`PlenoraError::Replayed`], che porta categoria e attribuzione insieme.
//! `Execution` viene costruito solo per un fallimento che classificazione
//! propria non ne ha — cioe' per un errore che e' gia' `Execution`.
//!
//! Cosi' non c'e' piu' una lista da tenere allineata: la regola vale per
//! costruzione anche per le categorie che verranno.

use plenora_core::ErrorCategory;

/// `true` se la categoria va **preservata** invece di essere sostituita da
/// `execution`.
///
/// Vero per tutte le categorie tranne [`ErrorCategory::Execution`], che e'
/// per definizione «il passo e' fallito e non so dire altro»: li' non c'e'
/// nulla da preservare, e riavvolgerla aggiunge solo il nodo corrente.
#[must_use]
pub const fn categoria_preservata(categoria: ErrorCategory) -> bool {
    !matches!(categoria, ErrorCategory::Execution)
}

#[cfg(test)]
mod tests {
    use super::categoria_preservata;
    use plenora_core::ErrorCategory;

    #[test]
    fn l_elenco_delle_categorie_viene_dalla_fonte_canonica() {
        // Nessuna copia locale: l'elenco e' `ErrorCategory::ALL`, e la sua
        // completezza e' presidiata in `plenora-core` dalla coppia
        // `ALL` + `index` (match esaustivo). La versione precedente di
        // questo test aveva un array scritto a mano e verificava
        // `len() == 18` sul PROPRIO array: una condizione che sarebbe
        // rimasta vera aggiungendo una diciannovesima variante all'enum.
        // Prometteva un controllo che non faceva.
        assert!(
            !ErrorCategory::ALL.is_empty(),
            "l'elenco canonico non e' vuoto"
        );
    }

    #[test]
    fn solo_execution_viene_sostituita() {
        for &categoria in ErrorCategory::ALL {
            let atteso = categoria != ErrorCategory::Execution;
            assert_eq!(
                categoria_preservata(categoria),
                atteso,
                "{categoria:?}: una classificazione gia' presente non si butta via; \
                 solo `Execution` non ne ha una da preservare"
            );
        }
    }

    #[test]
    fn le_categorie_ritentabili_non_diventano_definitive() {
        // La conseguenza piu' concreta della regola vecchia: un errore
        // ritentabile che diventa `execution` diventa anche `Never`. Il test
        // nomina le categorie per cui questo era un danno diretto.
        for categoria in [
            ErrorCategory::Io,
            ErrorCategory::Timeout,
            ErrorCategory::Transient,
            ErrorCategory::Authentication,
            ErrorCategory::Authorization,
        ] {
            assert!(
                categoria_preservata(categoria),
                "{categoria:?} porta una disposizione di ritentativo propria: \
                 sostituirla la cancella"
            );
        }
    }
}
