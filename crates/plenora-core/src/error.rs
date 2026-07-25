//! Errori unificati (Architetture.md par. 3.1).
//!
//! Regola: nessun dato sensibile negli errori — contesto (nodo, operazione,
//! motivo), mai valori. La modalità diagnostica opt-in (ADR 3) è aggiunta
//! dall'executor, non da queste varianti.

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
    #[error("step failed at node `{node}` (operation `{operation}`): {reason}")]
    Step {
        node: String,
        operation: String,
        reason: String,
    },

    /// Errore CRS (irrisolvibile, requisito non soddisfatto, dominio violato).
    #[error("CRS error: {0}")]
    Crs(String),

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

pub type Result<T> = std::result::Result<T, PlenoraError>;
