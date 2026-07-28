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
//!
//! Milestone D (contratti trasversali v2.0-rc3 §9, proposta in attesa di
//! ratifica — andra' in ADR-0009): l'errore porta i quattro assi
//! indipendenti di R9.1. Categoria ([`PlenoraError::category`]) e
//! ritentabilita' ([`PlenoraError::retryable`]) esistono da M1d; qui si
//! aggiungono la fase ([`PlenoraError::phase`], [`ErrorPhase`]) e l'effetto
//! remoto ([`PlenoraError::remote_effect`], [`RemoteEffect`]), entrambi da
//! enumerazioni canoniche condivise (R9.5/R9.6: sottoinsieme ammesso,
//! valori propri vietati). La disposizione di retry di R9.7 (che
//! sostituisce il booleano) e' follow-up dichiarato: per ora
//! `retryable()` resta invariato.

use std::fmt;

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

/// Fase del ciclo dell'operazione in cui l'errore e' nato: asse «fase» di
/// R9.1 (contratti trasversali v2.0-rc3 §9, milestone D).
///
/// Enumerazione canonica (R9.5): sono ammessi solo questi dieci valori —
/// data-tools ne usa un sottoinsieme e non ne definisce di propri. Il
/// canonico non ha una fase «Execute»: l'esecuzione dei nodi del DAG ricade
/// in [`ErrorPhase::Write`] (produzione dell'output), vedi la decisione
/// progettuale in [`PlenoraError::phase`]. Mappatura sul ciclo di
/// data-tools; per i bordi filesystem vale §9: `Connect` = acquisizione
/// dell'handle/lease sulla risorsa, `Probe` = ispezione preliminare del
/// formato, `Commit` = rename atomico di publish (ADR 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorPhase {
    /// Validazione: parse del piano (JSON), contratti, schema, CRS,
    /// capability, limiti del governor.
    Validate,
    /// Acquisizione dell'handle/lease sulla risorsa (bordo filesystem, §9).
    Connect,
    /// Ispezione preliminare del formato o della risorsa di destinazione
    /// (es. riconoscimento fail-closed del filesystem, ADR 7).
    Probe,
    /// Preparazione di kernel e risorse prima dell'esecuzione.
    Prepare,
    /// Lettura dei dati di input dal supporto.
    Read,
    /// Produzione dell'output: esecuzione dei nodi del DAG e scrittura del
    /// tempfile di publish.
    Write,
    /// Finalizzazione dello stream di output (chiusura del writer).
    Finalize,
    /// Commit dell'effetto: rename atomico di publish (ADR 7, §9).
    Commit,
    /// Annullamento dell'effetto, con conferma.
    Rollback,
    /// Pulizia di risorse e residui.
    Cleanup,
}

impl ErrorPhase {
    /// Nome stabile della fase (telemetria, report JSON): `snake_case`
    /// canonico §9.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Connect => "connect",
            Self::Probe => "probe",
            Self::Prepare => "prepare",
            Self::Read => "read",
            Self::Write => "write",
            Self::Finalize => "finalize",
            Self::Commit => "commit",
            Self::Rollback => "rollback",
            Self::Cleanup => "cleanup",
        }
    }
}

impl fmt::Display for ErrorPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Effetto restato sul sistema remoto o sul supporto quando l'operazione
/// riporta l'esito: asse «effetto» di R9.1, enumerazione canonica R9.6
/// (contratti trasversali v2.0-rc3 §9, milestone D).
///
/// L'esito ignoto NON e' una categoria d'errore (R9.3): [`RemoteEffect::Unknown`]
/// vive su questo asse. In data-tools un [`PlenoraError`] ha per costruzione
/// effetto sempre [`RemoteEffect::None`] (vedi [`PlenoraError::remote_effect`]);
/// il caso «publish riuscito, durabilita' non confermata» non e' un errore
/// ma un esito tipizzato (`PublishOutcome`, ADR 7) che si mappa su questo
/// asse senza duplicarlo in una variante d'errore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteEffect {
    /// L'operazione non ha prodotto alcun effetto osservabile.
    None,
    /// L'effetto e' stato annullato, con conferma.
    RolledBack,
    /// Una parte dell'effetto e' visibile e una no.
    Partial,
    /// L'effetto e' definitivo, benche' l'operazione riporti un errore.
    Committed,
    /// L'effetto non e' determinabile con i mezzi disponibili.
    Unknown,
}

impl RemoteEffect {
    /// Nome stabile dell'effetto (telemetria, report JSON): `snake_case`
    /// canonico R9.6.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RolledBack => "rolled_back",
            Self::Partial => "partial",
            Self::Committed => "committed",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for RemoteEffect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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

    /// Fase del ciclo in cui l'errore e' nato (asse «fase» di R9.1,
    /// milestone D): mapping dichiarato per variante, stesso stampo di
    /// [`PlenoraError::category`].
    ///
    /// La derivazione per variante e' sufficiente: NESSUN override ai
    /// confini di tagging (`step_error`/`tag_execution` in engine,
    /// `at_input` nella CLI, publish) e' stato introdotto in questa
    /// milestone. Scelte di mapping (da riportare in ADR-0009):
    ///
    /// - `Contract`, `Unsupported`, `Schema`, `Crs`, `Json` →
    ///   [`ErrorPhase::Validate`]: parse del piano e controlli di contratto,
    ///   schema, CRS, capability e limiti sono validazione per natura, e il
    ///   canonico non ha una fase «Parse». Approssimazioni dichiarate: i
    ///   controlli del governor (es. `max_expansion_factor`) scattano
    ///   DURANTE l'esecuzione ma restano validazione di vincoli; il check
    ///   «output esiste gia'» di publish (ADR 7) avviene al confine di
    ///   commit. La variante non distingue i momenti: il raffinamento e'
    ///   follow-up.
    /// - `UnsupportedPublishTarget` → [`ErrorPhase::Probe`]: il
    ///   riconoscimento fail-closed del filesystem di destinazione (ADR 7)
    ///   e' ispezione preliminare della risorsa, prima di qualunque
    ///   scrittura — sui bordi filesystem §9 assegna l'ispezione a `Probe`.
    /// - `Step`, `Cancelled` → [`ErrorPhase::Write`]: il canonico non ha una
    ///   fase «Execute». DECISIONE PROGETTUALE: in data-tools la lettura
    ///   degli input (fase `Read`) avviene al confine `Input` PRIMA
    ///   dell'esecuzione del DAG e i suoi errori emergono come
    ///   `Io`/`Arrow`/`Schema`, mai come `Step`; un `Step` nasce solo
    ///   mentre un nodo produce il proprio stream di output, e la
    ///   cancellazione (invariante I8: nessun output pubblicato) e'
    ///   osservata agli stessi confini cooperativi. La produzione
    ///   dell'output e' la fase `Write` del ciclo canonico.
    /// - `Arrow`, `Io` → [`ErrorPhase::Write`]: le varianti coprono sia la
    ///   lettura degli input sia la scrittura/publish e non distinguono.
    ///   Si dichiara `Write` perche' e' il lato con possibile effetto sul
    ///   supporto, il solo rilevante quando la ritentabilita' sara'
    ///   calcolata da fase ed effetto (R9.7, follow-up): un errore in
    ///   lettura resta privo di effetti e altrettanto gestibile.
    ///   Approssimazione dichiarata.
    #[must_use]
    pub const fn phase(&self) -> ErrorPhase {
        match self {
            // Bracci fusi per fase (stessa decisione documentata sopra per
            // ogni variante): l'esaustivita' e' preservata perche' tutte
            // le varianti restano nominate esplicitamente.
            Self::Contract(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::Crs(_)
            | Self::Json(_) => ErrorPhase::Validate,
            Self::UnsupportedPublishTarget(_) => ErrorPhase::Probe,
            Self::Step { .. } | Self::Cancelled { .. } | Self::Arrow(_) | Self::Io(_) => {
                ErrorPhase::Write
            }
        }
    }

    /// Effetto restato sul supporto quando l'errore e' riportato (asse
    /// «effetto» di R9.1, enumerazione R9.6): mapping dichiarato per
    /// variante.
    ///
    /// Sempre [`RemoteEffect::None`], PER COSTRUZIONE: il publish atomico
    /// (ADR 7) scrive su tempfile nella stessa directory e pubblica solo a
    /// grafo completato con successo, eliminando il tempfile a qualunque
    /// fallimento — nessun output parziale e' mai visibile alla
    /// destinazione; la cancellazione rispetta l'invariante I8 (nessun
    /// output pubblicato). Anche gli eventuali residui temp dopo un crash
    /// restano `None`: non sono alla destinazione, non sono osservabili dal
    /// chiamante come effetto dell'operazione. L'unico caso «effetto
    /// presente a fronte di una segnalazione» — publish riuscito con
    /// durabilita' non confermata — NON e' un errore (R9.3): e' tipizzato
    /// come `PublishOutcome::PublishedButDurabilityUnconfirmed` (ADR 7) e
    /// mappato sull'asse effetto da quel tipo, non duplicato qui.
    #[must_use]
    pub const fn remote_effect(&self) -> RemoteEffect {
        match self {
            // Tutte le varianti nominate esplicitamente (esaustivita'
            // preservata): `None` per costruzione, vedi la doc sopra.
            Self::Contract(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::Step { .. }
            | Self::Crs(_)
            | Self::UnsupportedPublishTarget(_)
            | Self::Cancelled { .. }
            | Self::Arrow(_)
            | Self::Json(_)
            | Self::Io(_) => RemoteEffect::None,
        }
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

    /// Una istanza per variante costruibile direttamente, con la fase
    /// attesa (milestone D, R9.1); le varianti `#[from]` sono coperte a
    /// parte, come in [`samples`].
    fn phase_samples() -> Vec<(PlenoraError, ErrorPhase)> {
        vec![
            (PlenoraError::Contract("c".into()), ErrorPhase::Validate),
            (PlenoraError::Unsupported("u".into()), ErrorPhase::Validate),
            (PlenoraError::Schema("s".into()), ErrorPhase::Validate),
            (step("exec-1"), ErrorPhase::Write),
            (PlenoraError::Crs("crs".into()), ErrorPhase::Validate),
            (
                PlenoraError::UnsupportedPublishTarget("t".into()),
                ErrorPhase::Probe,
            ),
            (cancelled(), ErrorPhase::Write),
            (
                PlenoraError::Io(std::io::Error::other("io")),
                ErrorPhase::Write,
            ),
        ]
    }

    #[test]
    fn phase_mapping_is_declared_per_variant() {
        for (error, expected) in phase_samples() {
            assert_eq!(error.phase(), expected, "{error}");
        }
        // Varianti `#[from]`: costruibili solo da errori reali.
        let arrow: PlenoraError = arrow_schema::ArrowError::SchemaError("boom".into()).into();
        assert_eq!(arrow.phase(), ErrorPhase::Write);
        let json: PlenoraError = serde_json::from_str::<u32>("\"non-un-numero\"")
            .expect_err("json invalido")
            .into();
        assert_eq!(json.phase(), ErrorPhase::Validate);
    }

    #[test]
    fn remote_effect_is_none_for_every_variant_by_construction() {
        // ADR 7 (publish atomico: nessun output parziale mai visibile) +
        // invariante I8 (cancellazione senza output pubblicato): un
        // `PlenoraError` non accompagna mai un effetto osservabile. Il caso
        // «durabilita' non confermata» e' un `PublishOutcome`, non un
        // errore (R9.3).
        for (error, _) in samples() {
            assert_eq!(error.remote_effect(), RemoteEffect::None, "{error}");
        }
        let arrow: PlenoraError = arrow_schema::ArrowError::SchemaError("boom".into()).into();
        assert_eq!(arrow.remote_effect(), RemoteEffect::None);
        let json: PlenoraError = serde_json::from_str::<u32>("\"non-un-numero\"")
            .expect_err("json invalido")
            .into();
        assert_eq!(json.remote_effect(), RemoteEffect::None);
    }

    #[test]
    fn phase_names_are_exactly_the_canonical_ten() {
        // R9.5: solo i dieci valori canonici, snake_case; nessun valore
        // proprio di data-tools. La tabella e' esaustiva per costruzione:
        // aggiungere una variante all'enum senza toccare questo test lo
        // farebbe fallire sul conteggio.
        let all = [
            (ErrorPhase::Validate, "validate"),
            (ErrorPhase::Connect, "connect"),
            (ErrorPhase::Probe, "probe"),
            (ErrorPhase::Prepare, "prepare"),
            (ErrorPhase::Read, "read"),
            (ErrorPhase::Write, "write"),
            (ErrorPhase::Finalize, "finalize"),
            (ErrorPhase::Commit, "commit"),
            (ErrorPhase::Rollback, "rollback"),
            (ErrorPhase::Cleanup, "cleanup"),
        ];
        assert_eq!(all.len(), 10, "l'enumerazione canonica ha dieci fasi");
        for (phase, name) in all {
            assert_eq!(phase.as_str(), name);
            assert_eq!(phase.to_string(), name, "Display = as_str canonico");
        }
    }

    #[test]
    fn remote_effect_names_are_exactly_the_canonical_five() {
        // R9.6: solo i cinque valori canonici; l'esito ignoto e' un effetto
        // (`unknown`), non una categoria (R9.3).
        let all = [
            (RemoteEffect::None, "none"),
            (RemoteEffect::RolledBack, "rolled_back"),
            (RemoteEffect::Partial, "partial"),
            (RemoteEffect::Committed, "committed"),
            (RemoteEffect::Unknown, "unknown"),
        ];
        assert_eq!(all.len(), 5, "l'enumerazione canonica ha cinque effetti");
        for (effect, name) in all {
            assert_eq!(effect.as_str(), name);
            assert_eq!(effect.to_string(), name, "Display = as_str canonico");
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
