//! Errori unificati (Architetture.md par. 3.1).
//!
//! Regola: nessun dato sensibile negli errori — contesto (nodo, operazione,
//! motivo), mai valori. La modalità diagnostica opt-in (ADR 3) è aggiunta
//! dall'executor, non da queste varianti.
//!
//! Fase 2B M1d (ADR 3): ogni errore espone una [`ErrorCategory`] stabile
//! ([`PlenoraError::category`]) e l'indicazione [`PlenoraError::retryable`];
//! `Step` e `Cancelled` portano l'`execution_id` dell'esecuzione che li ha
//! prodotti (vuoto se costruiti fuori da un'esecuzione DAG, es. percorso
//! legacy `table_engine` — il Display lo omette in quel caso).

use thiserror::Error;

/// Errore unico del workspace, fusione di `EngineError` (nogeo-tools) e
/// `GeoEngineError` (geo-tools-arrow).
#[derive(Debug, Error)]
pub enum PlenoraError {
    /// Violazione del contratto del piano o di un nodo.
    #[error("contract violation: {0}")]
    Contract(String),

    /// Operazione non supportata (id sconosciuto, maturity insufficiente,
    /// capability mancante).
    #[error("unsupported operation: {0}")]
    Unsupported(String),

    /// Violazione di schema Arrow o di `DataContract`.
    #[error("schema violation: {0}")]
    Schema(String),

    /// Fallimento di un nodo durante l'esecuzione.
    ///
    /// `execution_id` (ADR 3, M1d) identifica l'esecuzione DAG che ha
    /// prodotto l'errore: e' riempito dall'executor al confine di
    /// dispatch/uscita; resta vuoto per errori costruiti fuori da
    /// un'esecuzione DAG (percorso legacy `table_engine`).
    #[error("step failed at node `{node}` (operation `{operation}`{}): {reason}", execution_suffix(execution_id))]
    Step {
        node: String,
        operation: String,
        execution_id: String,
        reason: String,
    },

    /// Errore CRS (irrisolvibile, requisito non soddisfatto, dominio violato).
    #[error("CRS error: {0}")]
    Crs(String),

    /// Destinazione di publish non supportata (ADR 7): filesystem di rete o
    /// non identificabile — riconoscimento fail-closed del filesystem.
    #[error("unsupported publish target: {0}")]
    UnsupportedPublishTarget(String),

    /// Esecuzione annullata dal chiamante (ADR 3, M1c): il token di
    /// cancellazione e' stato osservato a un confine cooperativo
    /// dell'executor e nessun output e' stato pubblicato (invariante I8).
    /// Contesto come `Step` — nodo, operazione, `execution_id` — mai dati.
    #[error("cancelled at node `{node}` (operation `{operation}`{}): {reason}", execution_suffix(execution_id))]
    Cancelled {
        node: String,
        operation: String,
        execution_id: String,
        reason: String,
    },

    /// Errore Arrow.
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    /// Errore di deserializzazione JSON (piano o config).
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Errore di I/O.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Categoria stabile di un [`PlenoraError`] (ADR 3).
///
/// L'errore primario conserva la categoria; pensata per telemetria e report
/// machine-readable, non per il matching di controllo di flusso (per quello
/// ci sono le varianti).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// Violazione del contratto del piano o di un nodo.
    Contract,
    /// Operazione non supportata.
    Unsupported,
    /// Violazione di schema Arrow o di `DataContract`.
    Schema,
    /// Fallimento di un nodo durante l'esecuzione.
    Step,
    /// Errore CRS.
    Crs,
    /// Destinazione di publish non supportata (ADR 7).
    UnsupportedPublishTarget,
    /// Esecuzione annullata dal chiamante (ADR 3).
    Cancelled,
    /// Errore Arrow.
    Arrow,
    /// Errore di deserializzazione JSON.
    Json,
    /// Errore di I/O.
    Io,
}

impl ErrorCategory {
    /// Nome stabile della categoria (telemetria, report JSON).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contract => "contract",
            Self::Unsupported => "unsupported",
            Self::Schema => "schema",
            Self::Step => "step",
            Self::Crs => "crs",
            Self::UnsupportedPublishTarget => "unsupported_publish_target",
            Self::Cancelled => "cancelled",
            Self::Arrow => "arrow",
            Self::Json => "json",
            Self::Io => "io",
        }
    }
}

impl PlenoraError {
    /// Categoria dell'errore (ADR 3, M1d): mapping dichiarato per variante.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::Contract(_) => ErrorCategory::Contract,
            Self::Unsupported(_) => ErrorCategory::Unsupported,
            Self::Schema(_) => ErrorCategory::Schema,
            Self::Step { .. } => ErrorCategory::Step,
            Self::Crs(_) => ErrorCategory::Crs,
            Self::UnsupportedPublishTarget(_) => ErrorCategory::UnsupportedPublishTarget,
            Self::Cancelled { .. } => ErrorCategory::Cancelled,
            Self::Arrow(_) => ErrorCategory::Arrow,
            Self::Json(_) => ErrorCategory::Json,
            Self::Io(_) => ErrorCategory::Io,
        }
    }

    /// L'errore ammette un retry da parte del chiamante?
    ///
    /// Default `false`: quasi tutti gli errori del workspace sono
    /// deterministici (contratto, schema, configurazione, limiti) o
    /// volontari (cancellazione) — ritentare a parita' di input fallirebbe
    /// allo stesso modo. `true` SOLO per [`ErrorCategory::Io`]: errori di
    /// I/O potenzialmente transitori (filesystem temporaneo o di rete
    /// momentaneamente indisponibile, esaurimento momentaneo di risorse di
    /// sistema) possono riuscire a un tentativo successivo. Backoff e numero
    /// di tentativi restano responsabilita' del chiamante.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Io(_))
    }
}

/// Suffisso del Display con l'`execution_id` (ADR 3): omesso quando
/// l'errore e' nato fuori da un'esecuzione DAG (id vuoto), cosi' i messaggi
/// del percorso legacy restano invariati.
fn execution_suffix(execution_id: &str) -> String {
    if execution_id.is_empty() {
        String::new()
    } else {
        format!(", execution `{execution_id}`")
    }
}

pub type Result<T> = std::result::Result<T, PlenoraError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn step(execution_id: &str) -> PlenoraError {
        PlenoraError::Step {
            node: "n".to_owned(),
            operation: "table.filter".to_owned(),
            execution_id: execution_id.to_owned(),
            reason: "boom".to_owned(),
        }
    }

    fn cancelled() -> PlenoraError {
        PlenoraError::Cancelled {
            node: "n".to_owned(),
            operation: "table.filter".to_owned(),
            execution_id: "exec-1".to_owned(),
            reason: "cancellazione richiesta dal chiamante".to_owned(),
        }
    }

    /// Una istanza per variante costruibile direttamente, con la categoria
    /// attesa (le varianti `#[from]` sono coperte a parte).
    fn samples() -> Vec<(PlenoraError, ErrorCategory)> {
        vec![
            (PlenoraError::Contract("c".into()), ErrorCategory::Contract),
            (PlenoraError::Unsupported("u".into()), ErrorCategory::Unsupported),
            (PlenoraError::Schema("s".into()), ErrorCategory::Schema),
            (step("exec-1"), ErrorCategory::Step),
            (PlenoraError::Crs("crs".into()), ErrorCategory::Crs),
            (
                PlenoraError::UnsupportedPublishTarget("t".into()),
                ErrorCategory::UnsupportedPublishTarget,
            ),
            (cancelled(), ErrorCategory::Cancelled),
            (
                PlenoraError::Io(std::io::Error::other("io")),
                ErrorCategory::Io,
            ),
        ]
    }

    #[test]
    fn category_mapping_is_declared_per_variant() {
        for (error, expected) in samples() {
            assert_eq!(error.category(), expected, "{error}");
            assert!(!error.category().as_str().is_empty());
        }
        // Varianti `#[from]`: costruibili solo da errori reali.
        let arrow: PlenoraError = arrow_schema::ArrowError::SchemaError("boom".into()).into();
        assert_eq!(arrow.category(), ErrorCategory::Arrow);
        let json: PlenoraError = serde_json::from_str::<u32>("\"non-un-numero\"")
            .expect_err("json invalido")
            .into();
        assert_eq!(json.category(), ErrorCategory::Json);
    }

    #[test]
    fn retryable_is_true_only_for_transient_io() {
        for (error, _) in samples() {
            let expected = matches!(error, PlenoraError::Io(_));
            assert_eq!(error.retryable(), expected, "{error}");
        }
    }

    #[test]
    fn step_display_omits_the_execution_id_when_empty() {
        // Percorso legacy (nessuna esecuzione DAG): messaggio invariato.
        assert_eq!(
            step("").to_string(),
            "step failed at node `n` (operation `table.filter`): boom"
        );
        assert_eq!(
            step("exec-42").to_string(),
            "step failed at node `n` (operation `table.filter`, execution `exec-42`): boom"
        );
    }

    #[test]
    fn cancelled_display_carries_context_without_values() {
        let text = cancelled().to_string();
        assert_eq!(
            text,
            "cancelled at node `n` (operation `table.filter`, execution `exec-1`): \
             cancellazione richiesta dal chiamante"
        );
        assert_eq!(cancelled().category(), ErrorCategory::Cancelled);
        assert!(!cancelled().retryable(), "la cancellazione e' volontaria");
    }
}
