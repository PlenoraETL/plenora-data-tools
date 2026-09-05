//! Che cosa succede alla CATEGORIA di un errore quando gli si aggiunge il
//! contesto del passo.
//!
//! Quando un passo fallisce, chi propaga l'errore vi aggiunge nodo e
//! operazione. Avvolgere tutto in [`PlenoraError::Execution`] e' il modo
//! ovvio, e **sostituisce** la categoria: ogni errore diventerebbe
//! `execution`, exit 6, retry `Never`.
//!
//! ## Perche' una lista di eccezioni non puo' funzionare
//!
//! Un elenco di categorie «preservate» e' la forma sbagliata del problema,
//! per due ragioni.
//!
//! **Cancella decisioni.** [`ErrorCategory::Io`] ha disposizione di
//! ritentativo `Safe`: un errore di I/O durante lo spill e' ritentabile, e
//! trasformarlo in `execution` lo renderebbe `Never` — cioe' direbbe al
//! chiamante di non riprovare una cosa che si puo' riprovare. Lo stesso vale
//! per `Timeout`, `Transient`, `Authentication`, `Authorization`,
//! `NotFound`, `Conflict`: sono tutte categorie su cui un chiamante fa
//! qualcosa di specifico, e una lista corta le butta via.
//!
//! **E' destinata a restare indietro.** Una lista di eccezioni va aggiornata
//! ogni volta che si aggiunge una categoria, e nessuno se ne ricorda.
//!
//! ## La regola
//!
//! Si inverte il default. Un errore che porta gia' una classificazione la
//! **conserva**, e il contesto del passo gli viene aggiunto tramite
//! [`PlenoraError::Replayed`], che porta categoria e attribuzione insieme.
//! `Execution` viene costruito solo per un fallimento che classificazione
//! propria non ne ha — cioe' per un errore che e' gia' `Execution`.
//!
//! Cosi' non c'e' nessuna lista da tenere allineata: la regola vale per
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
    fn l_elenco_delle_categorie_viene_da_una_fonte_sola() {
        // Nessuna copia locale: l'elenco e' `ErrorCategory::ALL`, e la sua
        // completezza e' presidiata in `plenora-core` dalla coppia
        // `ALL` + `index` (match esaustivo). Un array scritto a mano qui,
        // verificato con `len() == 18`, resterebbe vero aggiungendo una
        // diciannovesima variante all'enum: prometterebbe un controllo che
        // non fa.
        //
        // «Canonica» sarebbe la parola sbagliata per la fonte: diciotto
        // categorie vengono dal canone congelato, due sono estensioni locali.
        // Cio' che conta qui e' che la fonte sia UNA.
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
