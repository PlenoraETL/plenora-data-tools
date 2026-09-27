//! Attivazione del profilo isolato (`PR-12`): chi puo' chiederlo, e a quale
//! tetto.
//!
//! Due domande distinte, con due tempi distinti:
//!
//! 1. **La piattaforma lo supporta affatto?** Fatto statico, verificato in
//!    validazione con [`verifica_piattaforma`]: su Windows e macOS il piano e'
//!    respinto li', non ignorato ne' ricaduto sull'esecuzione in-process.
//! 2. **Questo ambiente Linux puo' prepararlo, ora?** Dipende da privilegi,
//!    cgroup delegati e politica del dispiegamento, non dal piano
//!    (isolamento.md, `PreparaIsolamento`, §3.1 e §9-bis). Si verifica dopo,
//!    con [`autorizza_profilo_isolato`], e il rifiuto e'
//!    [`PlenoraError::IsolationUnavailable`], mai `InvalidPlan`.
//!
//! La **richiesta** e' la sola presenza di `max_domain_memory_bytes` in un
//! piano v6; l'assenza mantiene il percorso attuale, senza tetto implicito.
//! Formato e hash del piano restano quelli ratificati (`Plan Budget 1.0`).
//! Una richiesta di isolamento **non ricade mai** sull'esecuzione in-process:
//! o parte isolata, o e' un rifiuto esplicito.
//!
//! Il tetto che l'host concede viene dalla **configurazione del
//! dispiegamento** (una variabile d'ambiente), mai dal piano ne' dal
//! protocollo: un piano non fidato non sceglie il proprio budget (debito
//! dichiarato in stato-e-roadmap.md). Senza, il profilo isolato non e'
//! disponibile.

use plenora_core::error::{PlenoraError, Result};
use serde::Serialize;

/// Nome della variabile d'ambiente che porta la politica dell'host: il tetto
/// massimo, in byte, che il dispiegamento concede a qualunque dominio di
/// isolamento su questa macchina.
pub const VARIABILE_POLITICA_HOST: &str = "PLENORA_ISOLATION_HOST_MAX_MEMORY_BYTES";

/// Rifiuta un profilo isolato richiesto su una piattaforma che non lo
/// supporta.
///
/// Il giudizio e' puro e separato dalla lettura ([`std::env::consts::OS`] al
/// sito di chiamata), quindi si verifica su ogni piattaforma di CI. Nessuna
/// verifica dinamica: `F4-11` e `F4-6` ne fanno un fatto della piattaforma,
/// perche' nessun prototipo dimostra contenimento *e* attribuzione fuori da
/// Linux.
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
/// Linux soltanto: [`verifica_piattaforma`] ha gia' respinto in validazione
/// le altre piattaforme.
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
/// Il tetto effettivo e' `min(richiesto_byte, limite_host_byte)`: l'host
/// **ritaglia**, non sostituisce, e il piano non amplia. Se il tetto
/// ritagliato scende sotto il budget governato effettivo si rifiuta qui,
/// prima di qualunque spawn, invece di modificare implicitamente i budget del
/// piano. E' una decisione pura su tre interi, e il tetto da passare allo
/// spawner si ottiene solo con un `Ok` da qui.
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
/// `executor::execute` non puo' servire.
///
/// Prova prima l'autorizzazione vera ([`prepara_e_autorizza`]): se rifiuta,
/// quel motivo e' quello giusto. Se autorizza, resta un rifiuto: la funzione
/// e' chiamata solo dalla guardia di `executor::execute`, e il chiamante di
/// produzione (`isolamento::esecuzione_isolata::esegui_isolato`) non passa da
/// qui. Arrivarci significa aver scavalcato il percorso isolato.
///
/// Fuori da Linux il ramo non si raggiunge, perche' `planner::validate` ha
/// gia' chiamato [`verifica_piattaforma`]; rifiuta comunque, senza
/// `unreachable!` in produzione (R6).
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
