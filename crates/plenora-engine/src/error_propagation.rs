//! Che cosa succede alla categoria di un errore quando gli si aggiunge il
//! contesto del passo.
//!
//! Avvolgere tutto in [`PlenoraError::Execution`] sostituirebbe la categoria
//! (exit 6, retry `Never`) e cancellerebbe decisioni: un errore
//! [`ErrorCategory::Io`] durante lo spill e' ritentabile. Una lista di
//! categorie da preservare resterebbe indietro alla prima categoria nuova.
//!
//! La regola inverte il default: un errore gia' classificato conserva la
//! categoria e riceve il contesto tramite [`PlenoraError::Replayed`];
//! `Execution` si costruisce solo per un errore che e' gia' `Execution`. Vale
//! per costruzione anche per le categorie future.

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
    fn l_elenco_delle_categorie_viene_da_una_fonte_sola() {
        // L'elenco e' `ErrorCategory::ALL`, la cui completezza e' presidiata
        // in `plenora-core` da un match esaustivo: una copia locale con la
        // lunghezza scritta a mano resterebbe verde anche con una variante in
        // piu'.
        assert!(
            !ErrorCategory::ALL.is_empty(),
            "l'elenco delle categorie supportate non e' vuoto"
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
        // La conseguenza piu' concreta della sostituzione: un errore
        // ritentabile che diventasse `execution` diventerebbe anche `Never`.
        // Il test nomina le categorie per cui il danno sarebbe diretto.
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
