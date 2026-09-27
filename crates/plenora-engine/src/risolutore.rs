//! Quale implementazione risolve i CRS in questa build, **detto in un posto solo**.
//!
//! Ne escono la **funzione** che risolve e l'**identita'** che la nomina,
//! insieme perche' sono la stessa scelta: se divergessero, l'handshake
//! rifiuterebbe un worker corretto o ne accetterebbe uno che risolve
//! diversamente dal supervisore. Ne esce anche se la build sappia
//! inventariare il proprio ambiente.
//!
//! I `cfg` sono due: quello in [`Risolutore::di_questa_build`] e' la
//! decisione; quello in [`risolvi`] sceglie solo quale simbolo esiste
//! (`resolve_crs` c'e' solo con la feature). Un caso li mette a confronto in
//! entrambe le build.

use plenora_core::crs::{CrsError, ResolvedCrs};

/// La versione del componente che implementa il protocollo.
///
/// E' cio' che l'handshake deve separare: la stessa libreria PROJ sotto due
/// versioni di questo programma fa due programmi diversi.
pub const VERSIONE: &str = env!("CARGO_PKG_VERSION");

/// Chi risolve i CRS in questa build.
///
/// Un enum e non un booleano: la domanda e' «quale implementazione», e una
/// variante nuova fa indicare al compilatore ogni posto che deve decidere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risolutore {
    /// Nessun backend: la risoluzione dichiara di non essere disponibile.
    ///
    /// Non e' «non risolve»: `plenora_core::crs::resolve_crs` valida la
    /// definizione **testualmente** e poi rende `BackendUnavailable`. La
    /// differenza conta, perche' una definizione malformata si rifiuta anche
    /// senza backend.
    SenzaBackend,
    /// PROJ, dietro la feature `proj-backend`.
    ///
    /// Il `cfg` dichiara che senza la feature nessuno la puo' costruire. L'arm
    /// `test` c'e' perche' il rifiuto dell'ambiente PROJ va provato anche dalla
    /// build senza PROJ.
    #[cfg(any(test, feature = "proj-backend"))]
    Proj,
}

impl Risolutore {
    /// Quello di questa build.
    ///
    /// **E' l'unico posto del programma che guarda `proj-backend`** per questa
    /// decisione. Un secondo `cfg` altrove sarebbe la divergenza che questo
    /// modulo esiste per impedire.
    pub const fn di_questa_build() -> Self {
        #[cfg(feature = "proj-backend")]
        {
            Self::Proj
        }
        #[cfg(not(feature = "proj-backend"))]
        {
            Self::SenzaBackend
        }
    }

    /// Come si chiama, per chi deve confrontare due lati.
    ///
    /// E' un nome stabile e non una descrizione: attraversa il filo, e due lati
    /// lo confrontano per uguaglianza. Cambiarlo cambia chi si accorda con chi.
    pub const fn identita(self) -> &'static str {
        match self {
            Self::SenzaBackend => "senza-backend",
            #[cfg(any(test, feature = "proj-backend"))]
            Self::Proj => "proj",
        }
    }

    /// Se questa build sa **inventariare** le risorse che ha a disposizione.
    ///
    /// Serve una radice esclusiva, immutabile e nota. Con PROJ non c'e':
    /// l'API dei percorsi li aggiunge invece di sostituirli e la cache delle
    /// griglie e' attiva per default. Un digest del solo searchpath sarebbe
    /// una falsa garanzia. Le condizioni di rientro stanno in
    /// `errori-e-limiti.md`.
    pub const fn ambiente_inventariabile(self) -> bool {
        match self {
            Self::SenzaBackend => true,
            #[cfg(any(test, feature = "proj-backend"))]
            Self::Proj => false,
        }
    }
}

/// Risolve una definizione CRS con l'implementazione di questa build.
///
/// L'identita' che il worker dichiara e la funzione usata devono venire dalla
/// stessa scelta: nessun chiamante importa `resolve_crs` con un `cfg` proprio.
///
/// # Errors
///
/// Come l'implementazione scelta: [`CrsError::Required`] o
/// [`CrsError::InvalidDefinition`] per una definizione testualmente invalida, e
/// [`CrsError::BackendUnavailable`] quando la build non ha un backend.
pub fn risolvi(definizione: &str, nome: &'static str) -> Result<ResolvedCrs, CrsError> {
    #[cfg(feature = "proj-backend")]
    {
        plenora_kernels_geo::crs::resolve_crs(definizione, nome)
    }
    #[cfg(not(feature = "proj-backend"))]
    {
        plenora_core::crs::resolve_crs(definizione, nome)
    }
}

#[cfg(test)]
mod tests;
