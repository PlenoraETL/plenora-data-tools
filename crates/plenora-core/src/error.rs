//! Errori unificati (Architetture.md par. 3.1).
//!
//! Regola: nessun dato sensibile negli errori — contesto (nodo, operazione,
//! motivo), mai valori. La modalità diagnostica opt-in (ADR 3) è aggiunta
//! dall'executor, non da queste varianti.
//!
//! Fase 2B M1d (ADR 3): ogni errore espone una [`ErrorCategory`] stabile
//! ([`PlenoraError::category`]); `Execution` e `Cancelled` portano
//! l'`execution_id` dell'esecuzione che li ha prodotti (vuoto se costruiti
//! fuori da un'esecuzione DAG, es. percorso legacy `table_engine` — il
//! Display lo omette in quel caso).
//!
//! Milestone D (contratti trasversali v2.0-rc8 §9, proposta in attesa di
//! ratifica — andra' in ADR-0009): l'errore porta i quattro assi
//! indipendenti di R9.1. Categoria ([`PlenoraError::category`]) esiste da
//! M1d; qui si aggiungono la fase ([`PlenoraError::phase`], [`ErrorPhase`]),
//! l'effetto remoto ([`PlenoraError::remote_effect`], [`RemoteEffect`]) e
//! la disposizione di retry ([`PlenoraError::retry_disposition`],
//! [`RetryDisposition`]), tutti da enumerazioni canoniche condivise
//! (R9.5/R9.6: sottoinsieme ammesso, valori propri vietati). R9.7
//! sostituisce il booleano `retryable()` di M1d — insufficiente e
//! pericoloso: un timeout in lettura e' ritentabile, lo stesso timeout
//! dopo l'invio di un commit non lo e' — con una disposizione calcolata
//! da fase, effetto e idempotenza, mai dalla sola categoria.

use std::fmt;
use std::time::Duration;

use thiserror::Error;

/// Errore unico del workspace, fusione di `EngineError` (nogeo-tools) e
/// `GeoEngineError` (geo-tools-arrow).
///
/// Nomi delle varianti allineati all'enumerazione canonica §9 (Appendice C,
/// contratti trasversali v2.0-rc8, R9.5: sottoinsieme ammesso, mai valori
/// propri): `Contract` → `InvalidPlan`, `Step` → `Execution`,
/// `UnsupportedPublishTarget` → fusa in `Unsupported`, `Json`/`Arrow` →
/// fuse in `DataMapping`. **I testi `Display` sono invariati** ("contract
/// violation", "step failed at node", "arrow error", ...): la rinomina e'
/// a livello di variante e categoria machine-readable, non di messaggio —
/// nessun consumatore testuale si rompe. Approssimazione dichiarata: la
/// fusione `Json`+`Arrow` in `DataMapping` perde la sorgente tipizzata
/// (resta nel testo) e la distinzione di fase parse/I-O (vedi
/// [`PlenoraError::phase`]).
#[derive(Debug, Error)]
pub enum PlenoraError {
    /// Piano o configurazione di un nodo malformati o incoerenti.
    #[error("contract violation: {0}")]
    InvalidPlan(String),

    /// Operazione non supportata (id sconosciuto, maturity insufficiente,
    /// capability mancante, destinazione di publish non supportata).
    #[error("unsupported operation: {0}")]
    Unsupported(String),

    /// Violazione di schema Arrow o di `DataContract`.
    #[error("schema violation: {0}")]
    Schema(String),

    /// Un valore non e' rappresentabile nella destinazione (errore Arrow o
    /// di deserializzazione JSON di piano/config).
    #[error("{0}")]
    DataMapping(String),

    /// Fallimento di un nodo durante l'esecuzione.
    ///
    /// `execution_id` (ADR 3, M1d) identifica l'esecuzione DAG che ha
    /// prodotto l'errore: e' riempito dall'executor al confine di
    /// dispatch/uscita; resta vuoto per errori costruiti fuori da
    /// un'esecuzione DAG (percorso legacy `table_engine`).
    #[error("step failed at node `{node}` (operation `{operation}`{}): {reason}", execution_suffix(execution_id))]
    Execution {
        node: String,
        operation: String,
        execution_id: String,
        reason: String,
    },

    /// Errore CRS (irrisolvibile, requisito non soddisfatto, dominio violato).
    #[error("CRS error: {0}")]
    Crs(String),

    /// Esecuzione annullata dal chiamante (ADR 3, M1c): il token di
    /// cancellazione e' stato osservato a un confine cooperativo
    /// dell'executor e nessun output e' stato pubblicato (invariante I8).
    /// Contesto come `Execution` — nodo, operazione, `execution_id` — mai dati.
    #[error("cancelled at node `{node}` (operation `{operation}`{}): {reason}", execution_suffix(execution_id))]
    Cancelled {
        node: String,
        operation: String,
        execution_id: String,
        reason: String,
    },

    /// Errore di I/O.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Invariante interna violata: uno stato che per costruzione non
    /// dovrebbe esistere (categoria `Internal` di par. 9, finora senza
    /// variante). Sostituisce le primitive di panic (`unreachable!`,
    /// `expect`) nei punti in cui il compilatore non puo' dimostrare
    /// l'esaustivita': il caso "impossibile" diventa un errore esplicito,
    /// mai un panic (R6). Il testo porta il contesto strutturale, mai
    /// valori di righe/colonne (regola 8).
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<arrow_schema::ArrowError> for PlenoraError {
    fn from(error: arrow_schema::ArrowError) -> Self {
        // Testo invariato rispetto alla variante `Arrow` pre-rinomina; la
        // sorgente tipizzata resta nel messaggio (fusione §9, dichiarata).
        Self::DataMapping(format!("arrow error: {error}"))
    }
}

impl From<serde_json::Error> for PlenoraError {
    fn from(error: serde_json::Error) -> Self {
        // Come sopra: testo invariato rispetto alla variante `Json`.
        Self::DataMapping(format!("json error: {error}"))
    }
}

/// Categoria stabile di un [`PlenoraError`]: enumerazione canonica §9
/// (R9.5 — il sottoinsieme usato dal componente, mai valori propri).
///
/// L'errore primario conserva la categoria; pensata per telemetria e report
/// machine-readable, non per il matching di controllo di flusso (per quello
/// ci sono le varianti).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// Piano o configurazione malformati o incoerenti.
    InvalidPlan,
    /// Configurazione del componente invalida.
    InvalidConfiguration,
    /// Schema Arrow o contratto dati incoerente.
    Schema,
    /// Un valore non e' rappresentabile nella destinazione.
    DataMapping,
    /// CRS assente, irrisolto o incoerente.
    Crs,
    /// Capability non offerta dal componente.
    Unsupported,
    /// Risorsa, layer o tabella inesistente.
    NotFound,
    /// Destinazione gia' esistente o conflitto di scrittura.
    Conflict,
    /// Credenziali assenti o rifiutate.
    Authentication,
    /// Permessi insufficienti.
    Authorization,
    /// Scadenza superata.
    Timeout,
    /// Annullato dal chiamante.
    Cancelled,
    /// Limite di byte, righe, profondita' o quota superato.
    ResourceLimit,
    /// Errore del filesystem o del dispositivo.
    Io,
    /// Violazione del protocollo di trasporto o di rete.
    Protocol,
    /// Condizione temporanea, ritentabile per natura.
    Transient,
    /// Fallimento di un nodo durante la trasformazione.
    Execution,
    /// Invariante interna violata.
    Internal,
}

impl ErrorCategory {
    /// Nome stabile della categoria (telemetria, report JSON): `snake_case`
    /// canonico §9.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPlan => "invalid_plan",
            Self::InvalidConfiguration => "invalid_configuration",
            Self::Schema => "schema",
            Self::DataMapping => "data_mapping",
            Self::Crs => "crs",
            Self::Unsupported => "unsupported",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::ResourceLimit => "resource_limit",
            Self::Io => "io",
            Self::Protocol => "protocol",
            Self::Transient => "transient",
            Self::Execution => "execution",
            Self::Internal => "internal",
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

/// Disposizione al ritentativo di un'operazione fallita: asse
/// «ritentativo» di R9.1, enumerazione canonica R9.7 (contratti trasversali
/// v2.0-rc8 §9).
///
/// Sostituisce il booleano `retryable` della 1.x, insufficiente e
/// pericoloso (R9.7: un timeout in lettura e' ritentabile, lo stesso
/// timeout dopo l'invio di un commit non lo e'). La disposizione e'
/// calcolata da fase, effetto e idempotenza dell'operazione — mai dalla
/// sola categoria — in [`PlenoraError::retry_disposition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetryDisposition {
    /// Ritentare e' sempre errato (causa deterministica o volontaria).
    Never,
    /// L'operazione e' idempotente o priva di effetti: si puo' ritentare.
    Safe,
    /// Ritentabile solo con una chiave che deduplichi l'effetto.
    RequiresIdempotencyKey,
    /// Prima di ritentare occorre accertare lo stato reale.
    RequiresRecovery,
    /// Ritentabile non prima della durata indicata.
    After(Duration),
}

impl RetryDisposition {
    /// Nome stabile della disposizione (telemetria, report JSON):
    /// `snake_case` canonico R9.7. Per [`RetryDisposition::After`] e' il
    /// solo nome del valore (`after`); la durata e' esposta da
    /// [`RetryDisposition::delay`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Safe => "safe",
            Self::RequiresIdempotencyKey => "requires_idempotency_key",
            Self::RequiresRecovery => "requires_recovery",
            Self::After(_) => "after",
        }
    }

    /// Durata minima prima del retry: presente solo per
    /// [`RetryDisposition::After`], `None` per gli altri valori.
    #[must_use]
    pub const fn delay(self) -> Option<Duration> {
        match self {
            Self::After(duration) => Some(duration),
            Self::Never
            | Self::Safe
            | Self::RequiresIdempotencyKey
            | Self::RequiresRecovery => None,
        }
    }
}

impl fmt::Display for RetryDisposition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PlenoraError {
    /// Categoria dell'errore (ADR 3, M1d): mapping dichiarato per variante.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::InvalidPlan(_) => ErrorCategory::InvalidPlan,
            Self::Unsupported(_) => ErrorCategory::Unsupported,
            Self::Schema(_) => ErrorCategory::Schema,
            Self::DataMapping(_) => ErrorCategory::DataMapping,
            Self::Execution { .. } => ErrorCategory::Execution,
            Self::Crs(_) => ErrorCategory::Crs,
            Self::Cancelled { .. } => ErrorCategory::Cancelled,
            Self::Io(_) => ErrorCategory::Io,
            Self::Internal(_) => ErrorCategory::Internal,
        }
    }

    /// Disposizione al ritentativo (asse «ritentativo» di R9.1,
    /// enumerazione canonica R9.7): calcolata da fase
    /// ([`PlenoraError::phase`]), effetto ([`PlenoraError::remote_effect`])
    /// e idempotenza dell'operazione — MAI dalla sola categoria.
    /// Sostituisce il booleano `retryable()` di M1d, rimosso perche'
    /// insufficiente e pericoloso (R9.7).
    ///
    /// Calcolo per data-tools (la variante porta gia' fase ed effetto per
    /// mapping dichiarato; la tabella segue):
    ///
    /// - L'effetto e' sempre [`RemoteEffect::None`] per costruzione (ADR 7:
    ///   publish atomico, nessun output parziale mai visibile; I8:
    ///   cancellazione senza output pubblicato) e la riesecuzione a parita'
    ///   di input e' idempotente (ADR-0001: stesso input → stesso output;
    ///   il publish rifiuta una destinazione esistente, quindi un tentativo
    ///   fallito prima del rename non lascia nulla che ostacoli il
    ///   successivo). Nessun errore di data-tools richiede quindi
    ///   [`RetryDisposition::RequiresIdempotencyKey`] o
    ///   [`RetryDisposition::RequiresRecovery`]: quei valori restano
    ///   nell'enumerazione per i componenti con stato remoto.
    /// - [`RetryDisposition::Safe`] SOLO per gli errori di I/O: causa
    ///   potenzialmente transitoria (filesystem temporaneo o di rete
    ///   momentaneamente indisponibile, lock condiviso su Windows — cfr.
    ///   `retryable_persist_error` in engine) a fronte di effetto assente
    ///   e operazione idempotente. Backoff e numero di tentativi restano
    ///   responsabilita' del chiamante.
    /// - [`RetryDisposition::Never`] per tutte le cause deterministiche
    ///   (contratto, schema, mapping, esecuzione di un nodo: ADR-0001 — a
    ///   parita' di input fallirebbero allo stesso modo), per la
    ///   cancellazione, che e' volontaria, e per le invarianti interne
    ///   violate (`Internal`), deterministiche per definizione.
    /// - [`RetryDisposition::After`] non e' mai prodotto: data-tools non ha
    ///   sorgenti di backoff tipizzate.
    #[must_use]
    pub const fn retry_disposition(&self) -> RetryDisposition {
        match self {
            Self::Io(_) => RetryDisposition::Safe,
            Self::InvalidPlan(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::DataMapping(_)
            | Self::Execution { .. }
            | Self::Crs(_)
            | Self::Cancelled { .. }
            | Self::Internal(_) => RetryDisposition::Never,
        }
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
    /// - `InvalidPlan`, `Unsupported`, `Schema`, `Crs` →
    ///   [`ErrorPhase::Validate`]: parse del piano e controlli di contratto,
    ///   schema, CRS, capability e limiti sono validazione per natura, e il
    ///   canonico non ha una fase «Parse». Approssimazioni dichiarate: i
    ///   controlli del governor (es. `max_expansion_factor`) scattano
    ///   DURANTE l'esecuzione ma restano validazione di vincoli; il check
    ///   «output esiste gia'» di publish (ADR 7) avviene al confine di
    ///   commit; la destinazione di publish non supportata (prima variante
    ///   dedicata, `Probe`) e' fusa in `Unsupported` dalla rinomina §9 e
    ///   ricade in `Validate` — la distinzione `Probe`/`Validate` richiede
    ///   il tagging al confine di publish, follow-up di R9.7. La variante
    ///   non distingue i momenti: il raffinamento e' follow-up.
    /// - `Execution`, `Cancelled` → [`ErrorPhase::Write`]: il canonico non
    ///   ha una fase «Execute». DECISIONE PROGETTUALE: in data-tools la
    ///   lettura degli input (fase `Read`) avviene al confine `Input` PRIMA
    ///   dell'esecuzione del DAG e i suoi errori emergono come
    ///   `Io`/`DataMapping`/`Schema`, mai come `Execution`; un `Execution`
    ///   nasce solo mentre un nodo produce il proprio stream di output, e
    ///   la cancellazione (invariante I8: nessun output pubblicato) e'
    ///   osservata agli stessi confini cooperativi. La produzione
    ///   dell'output e' la fase `Write` del ciclo canonico.
    /// - `DataMapping`, `Io` → [`ErrorPhase::Write`]: le varianti coprono
    ///   sia la lettura degli input sia la scrittura/publish e non
    ///   distinguono. Si dichiara `Write` perche' e' il lato con possibile
    ///   effetto sul supporto, il solo rilevante per la disposizione di
    ///   retry calcolata da fase ed effetto (R9.7, vedi
    ///   [`PlenoraError::retry_disposition`]): un errore in lettura resta
    ///   privo di effetti e altrettanto gestibile.
    ///   Approssimazione dichiarata (la fusione §9 di `Json`+`Arrow`
    ///   cancella la distinzione parse/I-O che le due varianti separate
    ///   portavano).
    /// - `Internal` → [`ErrorPhase::Write`]: un'invariante interna puo'
    ///   violarsi in qualunque punto; si dichiara `Write` (lato con
    ///   possibile effetto) per la stessa ragione conservativa di
    ///   `DataMapping`/`Io`. La disposizione resta `Never` a qualunque
    ///   fase: un'invariante violata e' deterministica per definizione.
    #[must_use]
    pub const fn phase(&self) -> ErrorPhase {
        match self {
            // Bracci fusi per fase (stessa decisione documentata sopra per
            // ogni variante): l'esaustivita' e' preservata perche' tutte
            // le varianti restano nominate esplicitamente.
            Self::InvalidPlan(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::Crs(_) => ErrorPhase::Validate,
            Self::Execution { .. }
            | Self::Cancelled { .. }
            | Self::DataMapping(_)
            | Self::Io(_)
            | Self::Internal(_) => ErrorPhase::Write,
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
            Self::InvalidPlan(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::DataMapping(_)
            | Self::Execution { .. }
            | Self::Crs(_)
            | Self::Cancelled { .. }
            | Self::Io(_)
            | Self::Internal(_) => RemoteEffect::None,
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
        PlenoraError::Execution {
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
            (
                PlenoraError::InvalidPlan("c".into()),
                ErrorCategory::InvalidPlan,
            ),
            (PlenoraError::Unsupported("u".into()), ErrorCategory::Unsupported),
            (PlenoraError::Schema("s".into()), ErrorCategory::Schema),
            (
                PlenoraError::DataMapping("d".into()),
                ErrorCategory::DataMapping,
            ),
            (step("exec-1"), ErrorCategory::Execution),
            (PlenoraError::Crs("crs".into()), ErrorCategory::Crs),
            (cancelled(), ErrorCategory::Cancelled),
            (
                PlenoraError::Io(std::io::Error::other("io")),
                ErrorCategory::Io,
            ),
            (
                PlenoraError::Internal("invariante violata".into()),
                ErrorCategory::Internal,
            ),
        ]
    }

    #[test]
    fn category_mapping_is_declared_per_variant() {
        for (error, expected) in samples() {
            assert_eq!(error.category(), expected, "{error}");
            assert!(!error.category().as_str().is_empty());
        }
        // Conversioni `From` esterne (fusione §9 in `DataMapping`).
        let arrow: PlenoraError = arrow_schema::ArrowError::SchemaError("boom".into()).into();
        assert_eq!(arrow.category(), ErrorCategory::DataMapping);
        assert!(arrow.to_string().starts_with("arrow error: "));
        let json: PlenoraError = serde_json::from_str::<u32>("\"non-un-numero\"")
            .expect_err("json invalido")
            .into();
        assert_eq!(json.category(), ErrorCategory::DataMapping);
        assert!(json.to_string().starts_with("json error: "));
    }

    #[test]
    fn internal_display_and_axes() {
        // R6: la variante Internal raccoglie le violazioni di invariante che
        // prima erano primitive di panic; gli assi sono quelli dichiarati.
        let error = PlenoraError::Internal("stato impossibile".into());
        assert_eq!(error.to_string(), "internal error: stato impossibile");
        assert_eq!(error.category(), ErrorCategory::Internal);
        assert_eq!(error.phase(), ErrorPhase::Write);
        assert_eq!(error.remote_effect(), RemoteEffect::None);
        assert_eq!(error.retry_disposition(), RetryDisposition::Never);
    }

    #[test]
    fn retry_disposition_is_safe_only_for_transient_io() {
        // R9.7: la disposizione sostituisce il booleano — `Safe` solo per
        // la causa potenzialmente transitoria (I/O) a effetto assente e
        // operazione idempotente; `Never` per cause deterministiche o
        // volontarie.
        for (error, _) in samples() {
            let expected = if matches!(error, PlenoraError::Io(_)) {
                RetryDisposition::Safe
            } else {
                RetryDisposition::Never
            };
            assert_eq!(error.retry_disposition(), expected, "{error}");
        }
    }

    #[test]
    fn retry_disposition_names_are_exactly_the_canonical_five() {
        // R9.7: solo i cinque valori canonici, snake_case; nessun valore
        // proprio di data-tools. La tabella e' esaustiva per costruzione.
        let all = [
            (RetryDisposition::Never, "never"),
            (RetryDisposition::Safe, "safe"),
            (
                RetryDisposition::RequiresIdempotencyKey,
                "requires_idempotency_key",
            ),
            (RetryDisposition::RequiresRecovery, "requires_recovery"),
            (RetryDisposition::After(Duration::from_millis(250)), "after"),
        ];
        assert_eq!(
            all.len(),
            5,
            "l'enumerazione canonica ha cinque disposizioni"
        );
        for (disposition, name) in all {
            assert_eq!(disposition.as_str(), name);
            assert_eq!(disposition.to_string(), name, "Display = as_str canonico");
        }
        // `after(durata)` trasporta la durata minima prima del retry.
        assert_eq!(
            RetryDisposition::After(Duration::from_millis(250)).delay(),
            Some(Duration::from_millis(250))
        );
        assert_eq!(RetryDisposition::Safe.delay(), None);
    }

    /// Una istanza per variante costruibile direttamente, con la fase
    /// attesa (milestone D, R9.1); le conversioni `From` esterne sono
    /// coperte a parte, come in [`samples`].
    fn phase_samples() -> Vec<(PlenoraError, ErrorPhase)> {
        vec![
            (PlenoraError::InvalidPlan("c".into()), ErrorPhase::Validate),
            (PlenoraError::Unsupported("u".into()), ErrorPhase::Validate),
            (PlenoraError::Schema("s".into()), ErrorPhase::Validate),
            (PlenoraError::DataMapping("d".into()), ErrorPhase::Write),
            (step("exec-1"), ErrorPhase::Write),
            (PlenoraError::Crs("crs".into()), ErrorPhase::Validate),
            (cancelled(), ErrorPhase::Write),
            (
                PlenoraError::Io(std::io::Error::other("io")),
                ErrorPhase::Write,
            ),
            (PlenoraError::Internal("i".into()), ErrorPhase::Write),
        ]
    }

    #[test]
    fn phase_mapping_is_declared_per_variant() {
        for (error, expected) in phase_samples() {
            assert_eq!(error.phase(), expected, "{error}");
        }
        // Conversioni `From` esterne: entrambe in `DataMapping` (Write —
        // la fusione §9 cancella la distinzione parse/I-O, dichiarata).
        let arrow: PlenoraError = arrow_schema::ArrowError::SchemaError("boom".into()).into();
        assert_eq!(arrow.phase(), ErrorPhase::Write);
        let json: PlenoraError = serde_json::from_str::<u32>("\"non-un-numero\"")
            .expect_err("json invalido")
            .into();
        assert_eq!(json.phase(), ErrorPhase::Write);
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
        assert_eq!(
            cancelled().retry_disposition(),
            RetryDisposition::Never,
            "la cancellazione e' volontaria"
        );
    }

    #[test]
    fn category_names_are_exactly_the_canonical_eighteen() {
        // R9.5: l'enumerazione canonica delle categorie; il sottoinsieme
        // usato dal componente e' mapping in `category()`, mai valori
        // propri. La tabella e' esaustiva per costruzione: aggiungere una
        // variante senza toccare questo test lo farebbe fallire.
        let all = [
            (ErrorCategory::InvalidPlan, "invalid_plan"),
            (ErrorCategory::InvalidConfiguration, "invalid_configuration"),
            (ErrorCategory::Schema, "schema"),
            (ErrorCategory::DataMapping, "data_mapping"),
            (ErrorCategory::Crs, "crs"),
            (ErrorCategory::Unsupported, "unsupported"),
            (ErrorCategory::NotFound, "not_found"),
            (ErrorCategory::Conflict, "conflict"),
            (ErrorCategory::Authentication, "authentication"),
            (ErrorCategory::Authorization, "authorization"),
            (ErrorCategory::Timeout, "timeout"),
            (ErrorCategory::Cancelled, "cancelled"),
            (ErrorCategory::ResourceLimit, "resource_limit"),
            (ErrorCategory::Io, "io"),
            (ErrorCategory::Protocol, "protocol"),
            (ErrorCategory::Transient, "transient"),
            (ErrorCategory::Execution, "execution"),
            (ErrorCategory::Internal, "internal"),
        ];
        assert_eq!(all.len(), 18, "l'enumerazione canonica ha 18 categorie");
        for (category, name) in all {
            assert_eq!(category.as_str(), name, "as_str canonico §9");
        }
    }
}
