//! Gli assi di un errore, **dal dominio al filo**.
//!
//! Funzioni con un nome e non una `From`, perche' la conversione non e'
//! totale: il ritentativo puo' portare un ritardo che sul filo non entra, e
//! allora **rifiuta** invece di saturare; la diagnostica di riga non passa.
//!
//! Il verso opposto lo converte `isolamento::macchina`, lato supervisore. Le
//! due direzioni restano allineate perche' ogni `match` e' esaustivo e un
//! caso di andata e ritorno, accanto al verso opposto, pretende che ogni
//! valore torni se stesso.
//!
//! Il messaggio attraversa com'e', senza ripulitore: i messaggi non portano
//! contenuto per regola (`errori-e-limiti.md`), rispettata dove l'errore
//! nasce.

use plenora_core::error::{ErrorCategory, ErrorPhase, RemoteEffect, Result, RetryDisposition};
use plenora_core::PlenoraError;

use super::messaggi::{
    CategoriaSulFilo, EffettoSulFilo, ErroreSulFilo, FaseSulFilo, FormaPanicSulFilo, RetrySulFilo,
};

/// Dove l'errore e' successo, nella forma che il filo porta.
///
/// Condivisa dalla conversione e dal rifiuto, per avere una sola nozione di
/// «posizione».
fn posizione(errore: &PlenoraError) -> (Option<String>, Option<String>, Option<String>) {
    match errore.execution_location() {
        Some((nodo, operazione, id)) => (
            Some(nodo.to_owned()),
            Some(operazione.to_owned()),
            id.map(str::to_owned),
        ),
        None => (None, None, None),
    }
}

/// L'errore del worker, portato sul filo **senza perdere gli assi**.
///
/// Entrano i quattro assi, il messaggio e la posizione nel DAG. Non entra la
/// diagnostica di riga: [`super::messaggi::DiagnosticaSulFilo`] non e'
/// isomorfa a `RowDiagnostics`, e riempirne i campi mancanti sarebbe
/// inventarli. Non entra il motivo semantico, che il protocollo non trasporta.
///
/// # Errors
///
/// [`PlenoraError::Internal`] se l'asse del ritentativo non entra sul filo: vedi
/// [`ritentativo_sul_filo`]. Chi deve dichiarare comunque qualcosa usa
/// [`errore_dichiarabile`], che quel rifiuto lo porta invece di propagarlo.
pub fn errore_sul_filo(errore: &PlenoraError) -> Result<ErroreSulFilo> {
    let (nodo, operazione, execution_id) = posizione(errore);
    Ok(ErroreSulFilo {
        categoria: categoria_sul_filo(errore.category()),
        fase: fase_sul_filo(errore.phase()),
        effetto: effetto_sul_filo(errore.remote_effect()),
        retry: ritentativo_sul_filo(errore.retry_disposition())?,
        messaggio: errore.to_string(),
        nodo,
        operazione,
        execution_id,
        diagnostica: None,
    })
}

/// L'errore da mandare sul filo, **sempre**: se un asse non ci entra, si
/// dichiara il rifiuto.
///
/// Chi la chiama sta gia' riportando un guasto e non ha un secondo canale:
/// senza messaggio il supervisore aspetterebbe un `Esito` che non arriva.
///
/// Quando rifiuta manda un errore **proprio**: `Internal` perche' e' un
/// difetto nostro, `Never` perche' un errore che non si sa descrivere non si
/// ritenta, e il messaggio porta sia l'originale sia la ragione del rifiuto.
/// Fase, effetto e posizione restano quelli osservati: il rifiuto riguarda un
/// asse solo.
///
/// Nessuna variante di `PlenoraError` produce oggi [`RetryDisposition::After`],
/// quindi il ramo del rifiuto e' una **guardia**; il suo testo lo compone
/// [`dichiarabile_dal_rifiuto`], provata a parte.
#[must_use]
pub fn errore_dichiarabile(errore: &PlenoraError) -> ErroreSulFilo {
    match errore_sul_filo(errore) {
        Ok(sul_filo) => sul_filo,
        Err(rifiuto) => dichiarabile_dal_rifiuto(errore, &rifiuto),
    }
}

/// L'errore sul filo che dichiara un rifiuto di conversione.
///
/// Funzione a se' perche' il ramo che la chiama oggi non si raggiunge (vedi
/// [`errore_dichiarabile`]): cosi' i test la provano passando i due errori.
fn dichiarabile_dal_rifiuto(errore: &PlenoraError, rifiuto: &PlenoraError) -> ErroreSulFilo {
    let (nodo, operazione, execution_id) = posizione(errore);
    ErroreSulFilo {
        categoria: CategoriaSulFilo::Internal,
        fase: fase_sul_filo(errore.phase()),
        effetto: effetto_sul_filo(errore.remote_effect()),
        retry: RetrySulFilo::Never {},
        messaggio: format!("{rifiuto}; l'errore da riportare era: {errore}"),
        nodo,
        operazione,
        execution_id,
        diagnostica: None,
    }
}

/// La forma di un payload di panico, dal dominio al filo.
///
/// L'autorita' della classificazione e' `plenora_core::panic_policy::forma_payload`:
/// invece di riscriverla con tre `is::<T>()`, si confronta cio' che dice del
/// payload vero con cio' che dice di un rappresentante di ciascuna forma, come
/// fa `isolamento::macchina` nel verso opposto.
///
/// Non passa da `FormaDelPayload`, che si compila solo sotto `test` e
/// `internals`. Esce solo **una variante di un enum chiuso**, senza byte del
/// payload.
#[must_use]
pub fn forma_sul_filo(payload: &(dyn std::any::Any + Send)) -> FormaPanicSulFilo {
    use plenora_core::panic_policy::forma_payload;

    let letta = forma_payload(payload);
    let statico: &'static str = "";
    if letta == forma_payload(&statico) {
        FormaPanicSulFilo::Statico
    } else if letta == forma_payload(&String::new()) {
        FormaPanicSulFilo::Dinamico
    } else {
        FormaPanicSulFilo::NonTestuale
    }
}

/// La categoria, dal dominio al filo.
///
/// Esaustiva e scritta a mano, come il verso opposto: una conversione per nome
/// compilerebbe sempre e fallirebbe a runtime.
#[must_use]
pub const fn categoria_sul_filo(nel_dominio: ErrorCategory) -> CategoriaSulFilo {
    match nel_dominio {
        ErrorCategory::InvalidPlan => CategoriaSulFilo::InvalidPlan,
        ErrorCategory::InvalidConfiguration => CategoriaSulFilo::InvalidConfiguration,
        ErrorCategory::Schema => CategoriaSulFilo::Schema,
        ErrorCategory::DataMapping => CategoriaSulFilo::DataMapping,
        ErrorCategory::Crs => CategoriaSulFilo::Crs,
        ErrorCategory::Unsupported => CategoriaSulFilo::Unsupported,
        ErrorCategory::NotFound => CategoriaSulFilo::NotFound,
        ErrorCategory::Conflict => CategoriaSulFilo::Conflict,
        ErrorCategory::Authentication => CategoriaSulFilo::Authentication,
        ErrorCategory::Authorization => CategoriaSulFilo::Authorization,
        ErrorCategory::Timeout => CategoriaSulFilo::Timeout,
        ErrorCategory::Cancelled => CategoriaSulFilo::Cancelled,
        ErrorCategory::ResourceLimit => CategoriaSulFilo::ResourceLimit,
        ErrorCategory::Io => CategoriaSulFilo::Io,
        ErrorCategory::Protocol => CategoriaSulFilo::Protocol,
        ErrorCategory::Transient => CategoriaSulFilo::Transient,
        ErrorCategory::Execution => CategoriaSulFilo::Execution,
        ErrorCategory::IsolationUnavailable => CategoriaSulFilo::IsolationUnavailable,
        ErrorCategory::UnattributedMemoryPressure => CategoriaSulFilo::UnattributedMemoryPressure,
        ErrorCategory::Internal => CategoriaSulFilo::Internal,
    }
}

/// La fase, dal dominio al filo.
#[must_use]
pub const fn fase_sul_filo(nel_dominio: ErrorPhase) -> FaseSulFilo {
    match nel_dominio {
        ErrorPhase::Validate => FaseSulFilo::Validate,
        ErrorPhase::Connect => FaseSulFilo::Connect,
        ErrorPhase::Probe => FaseSulFilo::Probe,
        ErrorPhase::Prepare => FaseSulFilo::Prepare,
        ErrorPhase::Read => FaseSulFilo::Read,
        ErrorPhase::Write => FaseSulFilo::Write,
        ErrorPhase::Finalize => FaseSulFilo::Finalize,
        ErrorPhase::Commit => FaseSulFilo::Commit,
        ErrorPhase::Rollback => FaseSulFilo::Rollback,
        ErrorPhase::Cleanup => FaseSulFilo::Cleanup,
    }
}

/// L'effetto remoto, dal dominio al filo.
#[must_use]
pub const fn effetto_sul_filo(nel_dominio: RemoteEffect) -> EffettoSulFilo {
    match nel_dominio {
        RemoteEffect::None => EffettoSulFilo::None,
        RemoteEffect::RolledBack => EffettoSulFilo::RolledBack,
        RemoteEffect::Partial => EffettoSulFilo::Partial,
        RemoteEffect::Committed => EffettoSulFilo::Committed,
        RemoteEffect::Unknown => EffettoSulFilo::Unknown,
    }
}

/// La disposizione al ritentativo, dal dominio al filo.
///
/// # Errors
///
/// [`PlenoraError::Internal`] se il ritardo di [`RetryDisposition::After`] non
/// entra in un `u64` di millisecondi.
///
/// # Perche' rifiuta invece di saturare
///
/// Sul filo il ritardo e' un `u64` di millisecondi, la `Duration` ne conta in
/// `u128`. Saturare manderebbe `u64::MAX` per durate diverse, una perdita che
/// non lascia traccia. Che una durata simile non nasca da nessuna politica
/// reale non basta: si controlla, e lo si riporta come `Internal`.
pub fn ritentativo_sul_filo(nel_dominio: RetryDisposition) -> Result<RetrySulFilo> {
    Ok(match nel_dominio {
        RetryDisposition::Never => RetrySulFilo::Never {},
        RetryDisposition::Safe => RetrySulFilo::Safe {},
        RetryDisposition::RequiresIdempotencyKey => RetrySulFilo::RequiresIdempotencyKey {},
        RetryDisposition::RequiresRecovery => RetrySulFilo::RequiresRecovery {},
        RetryDisposition::After(quanto) => {
            let millisecondi = quanto.as_millis();
            let delay_ms = u64::try_from(millisecondi).map_err(|_| {
                PlenoraError::Internal(format!(
                    "il ritardo di ritentativo e' {millisecondi} ms e non entra nel filo, \
                     che lo porta come u64"
                ))
            })?;
            RetrySulFilo::After { delay_ms }
        }
    })
}

#[cfg(test)]
mod tests;
