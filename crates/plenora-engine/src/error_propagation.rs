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

use plenora_core::error::ReplayedError;
use plenora_core::{ErrorCategory, PlenoraError};

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

/// Aggiunge a un errore il contesto del passo (nodo e operazione), con la
/// regola di [`categoria_preservata`].
///
/// L'unica costruzione, per l'executor DAG e per il percorso legacy: due copie
/// divergerebbero, e la stessa esecuzione darebbe errori diversi a seconda
/// della versione del piano. Le row diagnostics restano sull'errore esterno.
/// Un errore gia' `Execution` conserva la propria `reason`, senza annidare il
/// testo del contesto precedente. L'`execution_id` resta vuoto: lo riempie il
/// confine di uscita dell'executor, e il percorso legacy non ne ha.
pub(crate) fn con_contesto_del_passo(
    error: PlenoraError,
    node: String,
    operation: String,
) -> PlenoraError {
    let categoria = error.category();
    let diagnostics = error.row_diagnostics().cloned();
    if diagnostics.is_some() || categoria_preservata(categoria) {
        let replayed = PlenoraError::Replayed(Box::new(ReplayedError {
            category: categoria,
            phase: error.phase(),
            remote_effect: error.remote_effect(),
            retry: error.retry_disposition(),
            message: error.to_string(),
            node: Some(node),
            operation: Some(operation),
            execution_id: None,
            execution_reason: error.execution_reason().map(ToOwned::to_owned),
        }));
        return match diagnostics {
            Some(diagnostics) => replayed.with_row_diagnostics(diagnostics),
            None => replayed,
        };
    }
    let reason = match error {
        PlenoraError::Execution { reason, .. } => reason,
        other => other.to_string(),
    };
    PlenoraError::Execution {
        node,
        operation,
        execution_id: String::new(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::{categoria_preservata, con_contesto_del_passo};
    use plenora_core::{ErrorCategory, PlenoraError};

    #[test]
    fn una_categoria_preservata_riceve_il_contesto_senza_cambiare() {
        let errore = con_contesto_del_passo(
            PlenoraError::ResourceLimit("tetto".to_owned()),
            "3".to_owned(),
            "sort".to_owned(),
        );
        assert_eq!(errore.category(), ErrorCategory::ResourceLimit);
        assert_eq!(errore.execution_location(), Some(("3", "sort", None)));
    }

    #[test]
    fn un_execution_riavvolto_non_annida_il_testo() {
        // Il contesto piu' interno si sostituisce: la `reason` resta quella
        // originale, non il testo intero dell'errore precedente.
        let interno = PlenoraError::Execution {
            node: "1".to_owned(),
            operation: "filter".to_owned(),
            execution_id: String::new(),
            reason: "motivo".to_owned(),
        };
        let errore = con_contesto_del_passo(interno, "2".to_owned(), "sort".to_owned());
        let PlenoraError::Execution { node, operation, reason, .. } = &errore else {
            panic!("atteso Execution, ottenuto {errore:?}");
        };
        assert_eq!((node.as_str(), operation.as_str()), ("2", "sort"));
        assert_eq!(reason, "motivo");
    }

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
