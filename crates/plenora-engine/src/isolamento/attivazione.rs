//! Attivazione del profilo isolato (`PR-12`): chi puo' chiederlo, e a quale
//! tetto.
//!
//! Due domande distinte, con due tempi distinti:
//!
//! 1. **La piattaforma lo supporta affatto?** E' un fatto statico — dipende
//!    da quale binario e' stato compilato e su quale sistema gira, non
//!    dall'ambiente di una singola esecuzione. Si verifica in validazione,
//!    prima di ogni altra cosa, con [`verifica_piattaforma`]: su Windows e
//!    macOS un piano che richiede l'isolamento e' respinto li', non ignorato
//!    ne' lasciato ricadere sull'esecuzione in-process.
//! 2. **Questo ambiente Linux puo' prepararlo, ora?** E' dinamico — dipende
//!    da privilegi, cgroup delegati e dalla politica che il dispiegamento ha
//!    configurato — e non e' una proprieta' del piano (isolamento.md,
//!    `PreparaIsolamento`, §3.1 e §9-bis): lo stesso piano puo' riuscire su
//!    una macchina e fallire su un'altra. Si verifica dopo, con
//!    [`autorizza_profilo_isolato`], e il rifiuto e'
//!    [`PlenoraError::IsolationUnavailable`], mai `InvalidPlan`.
//!
//! # Selezione: la presenza del campo, non un campo a parte
//!
//! Questa e' una decisione semantica nuova di `PR-12`, non una conseguenza
//! gia' dimostrata della ratifica di `PR-2`: la sola presenza di
//! `max_domain_memory_bytes` in un piano v6 costituisce la **richiesta** del
//! profilo isolato. La sua assenza mantiene il percorso attuale, senza tetto
//! implicito. Non esiste un campo separato "seleziona l'isolamento": il
//! formato e l'hash del piano restano quelli gia' ratificati (`Plan Budget
//! 1.0`) — questo modulo non li tocca, decide solo che cosa fare di un
//! valore che il parser gia' produce.
//!
//! **Una richiesta di isolamento non puo' mai ricadere sull'esecuzione
//! in-process.** Se la piattaforma non lo supporta, o l'host non puo'
//! prepararlo, l'esito e' un rifiuto esplicito — mai un'esecuzione silenziosa
//! col percorso ordinario, che negherebbe la garanzia che il piano ha chiesto.
//!
//! # Politica dell'host: fidata, separata dal piano
//!
//! Il tetto che l'host concede non viaggia nel piano ne' nel protocollo: un
//! piano non fidato non deve poter scegliere il proprio budget (debito
//! dichiarato in stato-e-roadmap.md). Viene da **configurazione del
//! dispiegamento** — qui, una variabile d'ambiente del processo — che il
//! piano non scrive e non legge. Senza di essa il profilo isolato non e'
//! disponibile: non si ripiega su «allora vale ciò che chiede il piano»,
//! che sarebbe l'host piu' esposto per omissione, non per scelta.

use plenora_core::error::{PlenoraError, Result};
use serde::Serialize;

/// Nome della variabile d'ambiente che porta la politica dell'host: il tetto
/// massimo, in byte, che il dispiegamento concede a qualunque dominio di
/// isolamento su questa macchina.
pub const VARIABILE_POLITICA_HOST: &str = "PLENORA_ISOLATION_HOST_MAX_MEMORY_BYTES";

/// Rifiuta un profilo isolato richiesto su una piattaforma che non lo
/// supporta.
///
/// Presa dei fatti (`sistema_operativo`) separata dal giudizio, per la stessa
/// ragione delle altre osservazioni di questo confine: la lettura puo'
/// variare per motivi che il giudizio non deve conoscere — qui la lettura e'
/// [`std::env::consts::OS`] al sito di chiamata reale — mentre il giudizio e'
/// una funzione pura, verificabile su ogni piattaforma di CI senza dipendere
/// da quella su cui il test gira davvero.
///
/// Non serve alcuna verifica dinamica — non un cgroup, non un privilegio —
/// perche' la domanda a cui risponde non e' «questo ambiente lo offre ora»
/// ma «questo binario, su questo sistema, potrebbe mai offrirlo». `F4-11` e
/// `F4-6` la rendono un fatto della piattaforma: nessun prototipo dimostra
/// oggi contenimento *e* attribuzione fuori da Linux.
///
/// # Errors
///
/// [`PlenoraError::Unsupported`] se `richiede_isolamento` e
/// `sistema_operativo` non e' `"linux"`.
pub fn verifica_piattaforma(richiede_isolamento: bool, sistema_operativo: &str) -> Result<()> {
    if richiede_isolamento && sistema_operativo != "linux" {
        return Err(PlenoraError::Unsupported(format!(
            "il piano dichiara max_domain_memory_bytes, cioe' richiede il profilo isolato: non \
             supportato su `{sistema_operativo}`, disponibile solo su Linux (F4-6, F4-11)"
        )));
    }
    Ok(())
}

/// Il tetto richiesto dal piano e quello concesso al dominio, in forma
/// machine-readable — cosi' come isolamento.md richiede per un tetto
/// ritagliato dalla politica dell'host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConcessioneDominio {
    /// Il tetto che il piano ha dichiarato in `max_domain_memory_bytes`.
    pub richiesto_byte: u64,
    /// `min(richiesto_byte, limite_host_byte)`: il tetto che il dominio
    /// riceve davvero.
    pub concesso_byte: u64,
}

/// Perche' la politica dell'host non e' leggibile o applicabile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RifiutoPolitica {
    /// La variabile d'ambiente non c'e'.
    Assente,
    /// C'e', ma non e' un numero di byte.
    NonNumerica,
    /// E' un numero, ma zero: un tetto nullo non e' una politica.
    Zero,
}

impl RifiutoPolitica {
    fn detto(self) -> PlenoraError {
        let motivo = match self {
            Self::Assente => format!(
                "la variabile «{VARIABILE_POLITICA_HOST}» non c'e': nessuna politica dell'host \
                 e' configurata, e senza di essa il profilo isolato non e' disponibile — non si \
                 ripiega sul tetto che il piano chiede"
            ),
            Self::NonNumerica => {
                format!("la variabile «{VARIABILE_POLITICA_HOST}» non e' un numero di byte valido")
            }
            Self::Zero => format!(
                "la variabile «{VARIABILE_POLITICA_HOST}» e' zero: un tetto nullo non e' una \
                 politica, e' un dominio che non potrebbe allocare nulla"
            ),
        };
        PlenoraError::IsolationUnavailable(motivo)
    }
}

/// Il giudizio sulla politica letta, separato dalla lettura dell'ambiente per
/// la stessa ragione di [`super::worker::numeri_dall_ambiente`]: l'ambiente e'
/// globale al processo, e casi che lo scrivessero si darebbero fastidio a
/// vicenda girando in parallelo; il giudizio, isolato, e' una funzione pura.
fn politica_da(grezzo: Option<&str>) -> std::result::Result<u64, RifiutoPolitica> {
    let grezzo = grezzo.ok_or(RifiutoPolitica::Assente)?;
    let valore: u64 = grezzo
        .trim()
        .parse()
        .map_err(|_| RifiutoPolitica::NonNumerica)?;
    if valore == 0 {
        return Err(RifiutoPolitica::Zero);
    }
    Ok(valore)
}

/// Legge la politica dell'host dall'ambiente del processo.
///
/// Linux soltanto: [`verifica_piattaforma`] ha gia' respinto, in validazione,
/// ogni richiesta di isolamento su un'altra piattaforma — chi arriva qui e'
/// gia' su Linux per costruzione.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], con il motivo esatto: assente,
/// non numerica, o zero. Mai un default permissivo.
#[cfg(target_os = "linux")]
pub fn leggi_politica_dell_host() -> Result<u64> {
    politica_da(std::env::var(VARIABILE_POLITICA_HOST).ok().as_deref())
        .map_err(RifiutoPolitica::detto)
}

/// Decide se il profilo isolato puo' partire con questa politica dell'host,
/// e a quale tetto.
///
/// Il tetto effettivo e' `min(richiesto_byte, limite_host_byte)` — la
/// politica dell'host **ritaglia**, non sostituisce, e non e' mai
/// ampliabile dal piano. Se il tetto cosi' ritagliato scende sotto il
/// budget governato effettivo, l'incoerenza si rifiuta qui, PRIMA di
/// qualunque spawn: il dominio non modifica implicitamente i budget che il
/// piano ha gia' dichiarato coerenti fra loro, li rifiuta apertamente.
///
/// Nessun codice di questa funzione avvia un processo o tocca dati: e' una
/// decisione pura su tre interi, e chi in futuro costruira' il chiamante di
/// produzione non potra' ottenere un tetto da passare allo spawner senza
/// prima ottenere `Ok` da qui.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il tetto concesso e' sotto il
/// budget governato effettivo, nominando entrambi i valori.
pub fn autorizza_profilo_isolato(
    richiesto_byte: u64,
    governato_effettivo_byte: u64,
    limite_host_byte: u64,
) -> Result<ConcessioneDominio> {
    let concesso_byte = richiesto_byte.min(limite_host_byte);
    if concesso_byte < governato_effettivo_byte {
        return Err(PlenoraError::IsolationUnavailable(format!(
            "la politica dell'host concede {concesso_byte} byte al dominio, sotto il budget \
             governato effettivo di {governato_effettivo_byte} byte: il dominio non potrebbe \
             contenere l'esecuzione che il piano dichiara, e non si avvia con un tetto diverso \
             da quello dichiarato"
        )));
    }
    Ok(ConcessioneDominio {
        richiesto_byte,
        concesso_byte,
    })
}

/// Legge la politica dell'host e autorizza il profilo isolato in un solo
/// passo — la composizione che un chiamante di produzione dovra' fare.
///
/// # Errors
///
/// Come [`leggi_politica_dell_host`] e [`autorizza_profilo_isolato`].
#[cfg(target_os = "linux")]
pub fn prepara_e_autorizza(
    richiesto_byte: u64,
    governato_effettivo_byte: u64,
) -> Result<ConcessioneDominio> {
    let limite_host_byte = leggi_politica_dell_host()?;
    autorizza_profilo_isolato(richiesto_byte, governato_effettivo_byte, limite_host_byte)
}

/// Costruisce il rifiuto per una richiesta di isolamento che
/// `executor::execute` non puo' ancora servire.
///
/// Prova prima l'autorizzazione vera ([`prepara_e_autorizza`]): se rifiuta
/// gia' — piattaforma, politica assente, ritaglio incoerente — quel motivo
/// e' quello giusto. Se invece autorizza, resta comunque un rifiuto, perche'
/// questa funzione e' chiamata solo dalla guardia di `executor::execute`
/// (`PR-12`): il chiamante di produzione vero e'
/// `isolamento::esecuzione_isolata::esegui_isolato`, che non passa da qui —
/// se l'autorizzazione riesce ma si arriva comunque a questo punto, e'
/// perche' qualcuno ha invocato `execute` direttamente su un piano che
/// richiede l'isolamento, scavalcando il percorso isolato.
///
/// Su una piattaforma diversa da Linux questo ramo non dovrebbe mai essere
/// raggiunto: [`verifica_piattaforma`], chiamata da `planner::validate`
/// prima che un `ValidatedGraph` possa esistere, ha gia' respinto la
/// richiesta la'. Il ramo rifiuta comunque, invece di presupporre
/// l'irraggiungibile: nessun `unreachable!` in codice di produzione (R6).
#[must_use]
pub fn richiesta_isolamento_non_ancora_servibile(
    richiesto_byte: u64,
    governato_effettivo_byte: u64,
) -> PlenoraError {
    #[cfg(target_os = "linux")]
    {
        match prepara_e_autorizza(richiesto_byte, governato_effettivo_byte) {
            Ok(concessione) => PlenoraError::IsolationUnavailable(format!(
                "profilo isolato autorizzato (concessi {} byte su {} richiesti) ma questo e' \
                 `execute` chiamato direttamente, non il percorso isolato: il chiamante di \
                 produzione e' `isolamento::esecuzione_isolata::esegui_isolato`, e l'esecuzione \
                 non ricade mai sul percorso in-process",
                concessione.concesso_byte, concessione.richiesto_byte
            )),
            Err(errore) => errore,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        PlenoraError::Unsupported(format!(
            "il piano richiede il profilo isolato ({richiesto_byte} byte, budget governato \
             effettivo {governato_effettivo_byte} byte), non supportato su questa piattaforma"
        ))
    }
}

#[cfg(test)]
mod tests;
