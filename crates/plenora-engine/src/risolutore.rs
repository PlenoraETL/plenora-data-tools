//! Quale implementazione risolve i CRS in questa build, **detto in un posto solo**.
//!
//! # Perche' un selettore tipizzato
//!
//! Perche' la scelta e' una, e scriverla due volte la rende due. Senza questo
//! modulo i due chiamanti — `geo_transport::publish` e `planner` — tengono
//! ciascuno la propria coppia di `cfg`, e due copie della stessa scelta sono
//! due occasioni di divergere: il giorno che una cambia, il programma risolve i
//! CRS con un'implementazione e li **descrive** con un'altra.
//!
//! Quel giorno il difetto non si vedrebbe come un errore: si vedrebbe come un
//! handshake che rifiuta un worker corretto, oppure — peggio — che accetta un
//! worker che risolve diversamente dal supervisore. Nessuna delle due cose
//! somiglia a «c'e' un `cfg` di troppo».
//!
//! # Che cosa esce da qui, e perche' insieme
//!
//! La **funzione** che risolve e l'**identita'** che la nomina. Insieme perche'
//! sono la stessa scelta guardata da due lati: se potessero divergere, la
//! descrizione che il worker manda al supervisore non descriverebbe cio' che il
//! worker fa.
//!
//! Ne esce anche se questa build sappia **inventariare** il proprio ambiente,
//! che e' una proprieta' dell'implementazione e non del chiamante.
//!
//! # I due `cfg`, e perche' non sono due decisioni
//!
//! Ce ne sono due, e vale la pena distinguerli invece di dichiarare che ce n'e'
//! uno solo.
//!
//! Il primo, in [`Risolutore::di_questa_build`], e' **la decisione**: quale
//! implementazione questa build usa. E' quello che il resto del programma non
//! deve duplicare.
//!
//! Il secondo, in [`risolvi`], non decide niente: sceglie **quale simbolo
//! esiste**. `plenora_kernels_geo::crs::resolve_crs` c'e' solo con la feature,
//! e nessuna riscrittura puo' nominarlo quando non e' compilato — e' una
//! proprieta' del linking, non una scelta di progetto.
//!
//! I due possono comunque divergere, se qualcuno ne modifica uno solo. Per
//! questo c'e' un caso che li mette a confronto: pretende che l'identita'
//! dichiarata e il comportamento della funzione dicano la stessa cosa, in
//! entrambe le build. Un `cfg` che si sposta senza l'altro lo fa cadere.

use plenora_core::crs::{CrsError, ResolvedCrs};

/// La versione del componente che implementa il protocollo.
///
/// # Perche' quella del componente e non quella dell'implementazione
///
/// Perche' e' cio' che il confronto dell'handshake deve separare: due lati
/// possono avere la stessa libreria PROJ e due versioni diverse di questo
/// programma, e sono due programmi diversi. La versione della libreria, se un
/// giorno servira', e' un campo suo — non questo.
pub const VERSIONE: &str = env!("CARGO_PKG_VERSION");

/// Chi risolve i CRS in questa build.
///
/// # Perche' un enum e non un booleano
///
/// Perche' un booleano dice «con PROJ oppure no», e la domanda vera e' «quale
/// implementazione»: il giorno che ce ne fosse una terza, un booleano
/// costringerebbe a inventare una convenzione, mentre qui si aggiunge una
/// variante e il compilatore indica ogni posto che deve decidere che farne.
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
    /// # Perche' la variante porta un `cfg`
    ///
    /// Perche' senza la feature **nessuno la puo' costruire**: non e' una
    /// variante inutilizzata, e' una variante che quella build non ha. Il
    /// `cfg` lo dichiara, invece di lasciare nel binario un valore che il
    /// programma non sa produrre — che e' cio' che `-D dead-code` chiama col
    /// suo nome.
    ///
    /// L'arm `test` c'e' perche' il rifiuto dell'ambiente PROJ va **provato**
    /// anche dalla build senza PROJ: i casi costruiscono la variante e
    /// pretendono il rifiuto, e senza quell'arm la prova esisterebbe solo
    /// nella build che non ne ha bisogno.
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
    /// # Perche' e' una domanda a se', e perche' oggi PROJ risponde no
    ///
    /// Perche' descrivere un ambiente vuol dire elencarne le risorse e
    /// digerirle, e cio' si puo' fare solo se la radice da cui provengono e'
    /// esclusiva, immutabile e nota. Con PROJ oggi non lo e': l'API che fissa i
    /// percorsi li **aggiunge** a quelli esistenti invece di sostituirli, e la
    /// cache delle griglie e' attiva per default. Non c'e' quindi un insieme di
    /// cui si possa dire «e' tutto, e non cambia».
    ///
    /// Un digest ricavato dal solo searchpath sarebbe una falsa garanzia: due
    /// macchine con lo stesso searchpath e contenuti diversi lo condividerebbero,
    /// e l'handshake direbbe «stesso ambiente» su due ambienti diversi. Meglio
    /// un rifiuto dichiarato.
    ///
    /// Il rientro sta in `errori-e-limiti.md`, e non e' una data: e' un elenco
    /// di condizioni che un provider unico deve soddisfare.
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
/// # Perche' passa da qui
///
/// Perche' l'identita' che il worker dichiara e la funzione che il programma
/// usa devono venire dalla stessa scelta. Un chiamante che importi
/// `resolve_crs` con un `cfg` proprio lascia che le due cose divergano senza
/// che niente lo dica: e' il motivo per cui nessuno lo fa piu'.
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
