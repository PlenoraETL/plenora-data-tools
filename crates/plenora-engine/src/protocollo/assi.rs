//! Gli assi di un errore, **dal dominio al filo**.
//!
//! # Perche' un modulo e non una `From`
//!
//! Perche' la conversione non e' totale, e una `From` suggerirebbe che lo sia.
//! Tre assi su quattro passano interi; il ritentativo puo' portare un ritardo
//! che sul filo non entra, e allora **rifiuta** invece di saturare; la
//! diagnostica di riga non passa affatto. Il posto dove dire tutto questo e' la
//! documentazione di funzioni con un nome, non un'implementazione di tratto che
//! si applica in silenzio.
//!
//! # Il verso opposto sta altrove, e c'e' un caso che li tiene insieme
//!
//! Dal filo al dominio converte `isolamento::macchina`, che e' chi legge cio'
//! che il worker manda. Le due direzioni sono in due posti perche' hanno due
//! chiamanti — il worker produce, il supervisore consuma — e restano allineate
//! per due ragioni indipendenti: ogni `match` e' **esaustivo**, quindi una
//! variante nuova non compila finche' non le si dice dove va da entrambe le
//! parti; e un caso di andata e ritorno pretende che ogni valore torni se
//! stesso. Il caso vive accanto al verso opposto, dove entrambi sono
//! raggiungibili.
//!
//! # Il messaggio
//!
//! Attraversa cosi' com'e'. Non c'e' un ripulitore, e non e' una dimenticanza:
//! i messaggi di questo programma non portano contenuto — nominano fasi,
//! grandezze e percorsi, mai valori di cella o frammenti di riga — e la regola
//! e' registrata in `errori-e-limiti.md`. Aggiungere qui un filtro darebbe
//! l'impressione che quella regola valga per averlo scritto, mentre vale
//! perche' ogni errore la rispetta dove nasce.

use plenora_core::error::{ErrorCategory, ErrorPhase, RemoteEffect, Result, RetryDisposition};
use plenora_core::PlenoraError;

use super::messaggi::{
    CategoriaSulFilo, EffettoSulFilo, ErroreSulFilo, FaseSulFilo, FormaPanicSulFilo, RetrySulFilo,
};

/// Dove l'errore e' successo, nella forma che il filo porta.
///
/// Sta in una funzione perche' la usano in due — la conversione e il rifiuto — e
/// scriverla due volte darebbe due nozioni di «posizione» libere di divergere
/// proprio nel caso in cui una delle due si legge di rado.
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
/// # Che cosa entra, e che cosa no
///
/// I quattro assi, il messaggio, e la posizione nel DAG quando c'e'. Non entra
/// la diagnostica di riga: [`super::messaggi::DiagnosticaSulFilo`] non e'
/// isomorfa a `RowDiagnostics` — le mancano campi — e riempirli con valori
/// inventati direbbe di aver osservato cose che nessuno ha osservato. Un campo
/// vuoto e' quindi `None`, non un segnaposto.
///
/// Il motivo semantico dell'esecuzione non entra per la stessa ragione per cui
/// non entra dall'altra parte: il protocollo non lo trasporta.
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
/// # Perche' esiste, e perche' non e' un ripiego
///
/// Perche' chi la chiama sta gia' riportando un guasto, e non ha un secondo
/// canale su cui riportare il guasto del riporto. Restare senza messaggio
/// lascerebbe il supervisore ad aspettare un `Esito` che non arriva.
///
/// # Che cosa manda quando rifiuta
///
/// Un errore **proprio**, non quello di partenza truccato. La categoria e'
/// `Internal` perche' e' un difetto nostro; il ritentativo e' `Never` perche' un
/// errore che non si sa descrivere non si ritenta; il messaggio porta **tutti e
/// due** i fatti — il testo dell'originale e la ragione del rifiuto — perche' e'
/// l'unico campo che li regge entrambi senza mentire su nessuno.
///
/// Non si riusa qui la conversione: categoria e ritentativo si scrivono, perche'
/// sono affermazioni su **questo** errore e non su quello arrivato. Fase,
/// effetto e posizione restano invece quelli osservati: il rifiuto riguarda un
/// asse solo, e buttare via gli altri sarebbe una seconda perdita per aver
/// scoperto la prima.
///
/// # Che oggi il rifiuto non si raggiunga non lo rende inutile
///
/// Nessuna variante di `PlenoraError` produce [`RetryDisposition::After`]:
/// `plenora-core` lo dichiara, perche' non ci sono sorgenti di backoff
/// tipizzate. Il ramo del rifiuto e' quindi una **guardia**, non un cammino — e
/// resta scritto per la stessa ragione per cui il controllo e' fallibile: il
/// giorno in cui una sorgente di backoff nascesse, il codice che la incontra
/// deve gia' sapere che farne, invece di scoprirlo saturando.
///
/// Cio' che la guardia **dice** e' provato: il testo lo compone
/// [`dichiarabile_dal_rifiuto`], che prende i due errori e si guarda da sola.
#[must_use]
pub fn errore_dichiarabile(errore: &PlenoraError) -> ErroreSulFilo {
    match errore_sul_filo(errore) {
        Ok(sul_filo) => sul_filo,
        Err(rifiuto) => dichiarabile_dal_rifiuto(errore, &rifiuto),
    }
}

/// L'errore sul filo che dichiara un rifiuto di conversione.
///
/// Sta in una funzione propria perche' il ramo che la chiama oggi non si
/// raggiunge — vedi [`errore_dichiarabile`] — e cio' che non si raggiunge non si
/// prova: separandola, i due errori diventano parametri e i casi la guardano
/// direttamente, senza dover costruire uno stato che non esiste.
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
/// # Perche' passa dai rappresentanti e non da tre `is::<T>()`
///
/// Perche' la classificazione ha gia' un'autorita' —
/// `plenora_core::panic_policy::forma_payload`, che distingue i tre casi che
/// `std` puo' produrre senza leggere il contenuto di nessuno — e riscriverla
/// qui darebbe due nozioni di «forma» libere di divergere. Una delle due,
/// prima o poi, finirebbe per pubblicare qualcosa.
///
/// Si chiede quindi all'autorita' che cosa dice del payload vero, e che cosa
/// direbbe di un rappresentante di ciascuna delle tre forme: la variante e'
/// quella che combacia. E' lo stesso giro che `isolamento::macchina` fa nel
/// verso opposto, dove i rappresentanti stanno dall'altra parte.
///
/// # Perche' non passa da `FormaDelPayload`
///
/// Perche' quel tipo vive in `classificazione`, che si compila solo sotto
/// `test` e `internals`: il worker e' produzione, e non lo raggiunge. La
/// garanzia che serve qui e' comunque intera — l'unica cosa che esce da questa
/// funzione e' **una variante di un enum chiuso**, e nessuna delle tre porta
/// con se' un byte del payload.
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
/// — passando dalle stringhe stabili — compilerebbe sempre e fallirebbe a
/// runtime, cioe' nel posto sbagliato.
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
/// `After` porta una `Duration`, che conta i millisecondi in `u128`; sul filo
/// il ritardo e' un `u64`. Saturare all'estremo manderebbe `u64::MAX` sia per
/// `Duration::from_millis(u64::MAX)` sia per qualunque durata piu' lunga: due
/// domini diversi, un solo valore sul filo, e nessun modo — per chi legge — di
/// sapere quale dei due abbia attraversato. E' una perdita che non lascia
/// traccia, cioe' la sola specie che nessun controllo a valle puo' riprendere.
///
/// Che una durata simile non nasca da nessuna politica reale non cambia niente:
/// «non puo' accadere» e' un ragionamento, non un controllo. Qui si controlla, e
/// se accade lo si dice — fallibile e riportato come `Internal`, che e' la forma
/// con cui questo progetto verifica le proprie invarianti.
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
