//! Applicazione effettiva di `max_parallelism`.
//!
//! I kernel paralleli usano il pool Rayon **globale**, che si dimensiona sui
//! core logici: senza configurarlo, `max_parallelism` non sarebbe un tetto. Il
//! tetto si applica quindi dimensionando il pool globale, una volta, prima
//! dell'esecuzione. Un pool dedicato con `ThreadPool::install` non e'
//! praticabile: richiede una chiusura `Send`, e lo stato dell'executor e'
//! thread-locale (`Rc<ExecutionPlan>`).
//!
//! - La configurazione e' **di processo**; la applicano sia la CLI sia
//!   `execute`, per chi incorpora l'engine come libreria.
//! - E' idempotente sullo stesso valore e fallisce su un valore diverso.
//! - `0` significa «numero di core logici» e si verifica sul pool vero, che
//!   chi incorpora l'engine puo' aver gia' costruito; se i core logici non
//!   sono conoscibili lo zero si rifiuta.

use std::sync::{Mutex, PoisonError};

use plenora_core::{PlenoraError, Result};

/// Grado di parallelismo con cui il pool globale e' stato configurato.
///
/// Un `Mutex` e non un `OnceLock`: controllo e impostazione sono una sezione
/// critica sola, o due chiamate concorrenti con lo stesso grado darebbero un
/// errore spurio su `build_global`.
static CONFIGURED: Mutex<Option<u32>> = Mutex::new(None);

/// Applica `max_parallelism` al pool Rayon del processo.
///
/// Va chiamata prima di qualunque uso di Rayon; dopo il primo uso il pool
/// globale e' gia' costruito e `build_global` fallisce.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se il pool e' gia' configurato con un grado
/// diverso, oppure se Rayon rifiuta la configurazione (pool globale gia'
/// costruito).
pub fn configure(max_parallelism: u32) -> Result<()> {
    // Sezione critica unica: controllo e impostazione non si separano.
    let mut configured = CONFIGURED.lock().unwrap_or_else(PoisonError::into_inner);
    if max_parallelism == 0 {
        // `CONFIGURED` puo' essere `None` mentre il pool globale e' gia' stato
        // costruito da chi incorpora l'engine. I core logici si leggono prima
        // di toccare Rayon: se non sono conoscibili, `0` si rifiuta senza aver
        // costruito un pool che non si sa validare.
        let logici = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .map_err(|error| {
                PlenoraError::InvalidPlan(format!(
                    "max_parallelism 0 non verificabile: il numero di core logici non e' \
                     determinabile su questo sistema ({error}); dichiarare un grado esplicito"
                ))
            })?;
        // Solo ora si interroga Rayon: `current_num_threads` costruisce il
        // pool di default se non esiste ancora — che e' esattamente cio' che
        // `0` chiede — e altrimenti riporta quello vero, comunque sia nato.
        let effettivi = rayon::current_num_threads();
        if effettivi != logici {
            return Err(PlenoraError::InvalidPlan(format!(
                "max_parallelism 0 (core logici: {logici}) non applicabile: il pool di \
                 questo processo ha {effettivi} thread"
            )));
        }
        // Registra il grado EFFETTIVO: da qui in poi il pool esiste e non e'
        // ridimensionabile, quindi un piano successivo con un tetto diverso
        // va rifiutato come qualunque altro cambio di grado.
        let grado = u32::try_from(effettivi).unwrap_or(u32::MAX);
        return match *configured {
            Some(already) if already != grado => Err(PlenoraError::InvalidPlan(format!(
                "max_parallelism 0 non applicabile: il pool di questo processo e' gia' \
                 dimensionato a {already}"
            ))),
            Some(_) => Ok(()),
            None => {
                *configured = Some(grado);
                Ok(())
            }
        };
    }
    if let Some(already) = *configured {
        return if already == max_parallelism {
            Ok(())
        } else {
            Err(PlenoraError::InvalidPlan(format!(
                "max_parallelism {max_parallelism} non applicabile: il pool di questo processo \
                 e' gia' dimensionato a {already} e Rayon non lo ridimensiona"
            )))
        };
    }
    let threads = usize::try_from(max_parallelism).unwrap_or(usize::MAX);
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .map_err(|error| {
            PlenoraError::InvalidPlan(format!(
                "max_parallelism {max_parallelism} non applicabile: {error}"
            ))
        })?;
    // Il valore si registra solo dopo il successo: un fallimento non deve
    // far credere alle chiamate successive che il tetto sia in vigore.
    *configured = Some(max_parallelism);
    drop(configured);
    Ok(())
}

/// Grado di parallelismo del pool di questo processo, se gia' accertato.
///
/// Lo accerta un tetto applicato, oppure lo zero che ha constatato il pool di
/// default. `None` significa «nessuna configurazione ancora attraversata»,
/// non «pool di default»: senza una chiamata a [`configure`] questo modulo
/// non ha guardato il pool.
#[must_use]
pub fn configured() -> Option<u32> {
    *CONFIGURED.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Numero di core logici, o `None` se il sistema non lo dichiara.
    fn core_logici() -> Option<usize> {
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .ok()
    }

    #[test]
    fn zero_esige_che_il_pool_del_processo_sia_quello_di_default() {
        // Lo zero verifica il pool vero, e l'ordine rispetto agli altri test
        // del binario non e' noto: si asserisce l'invariante (`0` passa se e
        // solo se il pool ha esattamente i core logici), non un esito fisso.
        let esito = configure(0);
        // Dopo la chiamata il pool esiste di sicuro: `current_num_threads`
        // lo costruisce se manca.
        let effettivi = rayon::current_num_threads();
        match core_logici() {
            Some(logici) => assert_eq!(
                esito.is_ok(),
                effettivi == logici,
                "0 deve passare esattamente quando il pool ha i core logici                  (effettivi {effettivi}, logici {logici})"
            ),
            // Senza core logici dichiarati il grado non e' dimostrabile e lo
            // zero si rifiuta: fail-closed, non «passa comunque».
            None => assert!(
                esito.is_err(),
                "senza core logici noti, 0 non e' verificabile e va rifiutato"
            ),
        }
        if esito.is_ok() {
            // Il grado accertato viene registrato, ed e' quello vero.
            assert_eq!(
                configured(),
                Some(u32::try_from(effettivi).unwrap_or(u32::MAX)),
                "il grado accertato va registrato"
            );
            assert!(configure(0).is_ok(), "0 resta idempotente");
        }
    }

    #[test]
    fn la_configurazione_e_idempotente_e_rifiuta_un_grado_diverso() {
        // Un solo grado per processo, e il pool globale puo' essere gia'
        // costruito da un altro test del binario: entrambi i rami asseriscono,
        // nessuno esce verde senza verificare.
        if configure(2).is_ok() {
            assert_eq!(configured(), Some(2), "il grado applicato va registrato");
            // Idempotenza sullo stesso grado.
            assert!(configure(2).is_ok());
            // Un grado diverso non e' onorabile e viene rifiutato.
            assert!(
                configure(3).is_err(),
                "un grado diverso da quello in vigore deve essere rifiutato"
            );
        } else {
            // Fallimento: nessun grado deve risultare registrato DA QUESTA
            // chiamata — non si dichiara in vigore un tetto non applicato.
            assert_ne!(
                configured(),
                Some(2),
                "un tetto non applicato non deve risultare in vigore"
            );
            // E le chiamate successive continuano a fallire, invece di
            // passare in silenzio lasciando credere che il tetto valga.
            assert!(configure(2).is_err());
            assert!(configure(3).is_err());
        }
        // `0` passa solo se il pool del processo e' quello di default: dopo
        // un tetto a 2 su una macchina con piu' di due core deve fallire.
        let effettivi = rayon::current_num_threads();
        if let Some(logici) = core_logici() {
            assert_eq!(
                configure(0).is_ok(),
                effettivi == logici,
                "0 non puo' passare su un pool diverso dai core logici"
            );
        }
    }
}
