//! Forma pubblica di [`PlenoraError`]: il documento `plenora-error-v1`
//! (`plenora-contracts`, `specs/errors/ERRORS-1.0.md` e
//! `schemas/error-v1.schema.json`).
//!
//! La proiezione non aggiunge dati: il messaggio è il `Display` dell'errore
//! (già senza valori di righe o colonne, regola del modulo padre), troncato
//! a [`MAX_MESSAGE_CHARS`] caratteri; `details` porta solo il documento
//! `plenora-row-diagnostics-v1` (indici, conteggi e codici, mai valori).
//!
//! Limiti del contratto applicati qui, oltre allo schema: ERR-006 (effetto
//! ignoto, nessun ritentativo automatico), ERR-007 (`delay_ms` solo con
//! `after`, entro un giorno), ERR-011 e ERR-012 (byte, profondità,
//! proprietà, elementi, stringhe e nodi di `details`). Un `details` oltre i
//! limiti non si tronca: la proiezione diventa un errore `internal`
//! esplicito, senza `details` ([`CODE_DETAILS_NOT_PUBLISHABLE`]).

use serde::ser::{Error as _, SerializeMap};
use serde::{Serialize, Serializer};
use serde_json::Value;

use super::{ErrorCategory, ErrorPhase, PlenoraError, RemoteEffect, RetryDisposition};

/// Massimo di caratteri del messaggio pubblico (schema: `maxLength`).
pub const MAX_MESSAGE_CHARS: usize = 2_048;
/// Massimo di `delay_ms` di un ritentativo `after` (schema: un giorno).
pub const MAX_RETRY_DELAY_MS: u64 = 86_400_000;
/// ERR-011: byte della codifica JSON compatta dell'errore intero.
pub const MAX_ERROR_BYTES: usize = 524_288;
/// ERR-011: byte della codifica JSON compatta di `details`.
pub const MAX_DETAILS_BYTES: usize = 262_144;
/// ERR-012: profondità di `details` (l'oggetto `details` è profondità 1).
pub const MAX_DETAILS_DEPTH: usize = 8;
/// ERR-012: proprietà di un oggetto, elementi di un array.
pub const MAX_DETAILS_FANOUT: usize = 128;
/// ERR-012: byte UTF-8 di una stringa.
pub const MAX_DETAILS_STRING_BYTES: usize = 4_096;
/// ERR-012: nodi JSON (contenitori e scalari).
pub const MAX_DETAILS_NODES: usize = 2_048;

/// Codice di [`PlenoraError::Timeout`]: la scadenza dell'esecuzione è
/// passata (vettore `data-run-timeout-error` del contratto).
pub const CODE_DEADLINE_EXCEEDED: &str = "EXECUTION_DEADLINE_EXCEEDED";
/// Codice di [`PlenoraError::Cancelled`].
pub const CODE_CANCELLED: &str = "EXECUTION_CANCELLED";
/// Codice della proiezione sostitutiva quando `details` non è pubblicabile:
/// oltre i limiti ERR-011/ERR-012, o diagnostica per riga non valida.
pub const CODE_DETAILS_NOT_PUBLISHABLE: &str = "ERROR_DETAILS_NOT_PUBLISHABLE";

/// Documento `plenora-error-v1` di un [`PlenoraError`]; si ottiene con
/// [`PlenoraError::public_projection`] e si serializza con `serde`.
///
/// `provider` ed `execution_id` non ci sono: nessun errore di questo
/// workspace ne ha uno. I campi sono privati: un `PublicError` nasce solo
/// dalla proiezione, che ne garantisce la validità.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PublicError {
    category: ErrorCategory,
    phase: ErrorPhase,
    remote_effect: RemoteEffect,
    retry: RetryDisposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl PublicError {
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        self.category
    }

    #[must_use]
    pub const fn phase(&self) -> ErrorPhase {
        self.phase
    }

    #[must_use]
    pub const fn remote_effect(&self) -> RemoteEffect {
        self.remote_effect
    }

    #[must_use]
    pub const fn retry(&self) -> RetryDisposition {
        self.retry
    }

    /// Codice stabile (`^[A-Z][A-Z0-9_]{1,63}$`), se l'errore ne ha uno
    /// ([`PlenoraError::code`]).
    #[must_use]
    pub const fn code(&self) -> Option<&'static str> {
        self.code
    }

    /// Non vuoto, al più [`MAX_MESSAGE_CHARS`] caratteri.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// `{"row_diagnostics": <plenora-row-diagnostics-v1>}`, se l'errore ha
    /// una diagnostica per riga.
    #[must_use]
    pub const fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }
}

impl Serialize for ErrorCategory {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for ErrorPhase {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for RemoteEffect {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// `{"kind": ...}`, con `delay_ms` solo per `after` (ERR-007). Un ritardo
/// oltre [`MAX_RETRY_DELAY_MS`] non si serializza: la proiezione lo porta
/// prima a `never`.
impl Serialize for RetryDisposition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let delay_ms = self
            .delay()
            .map(|delay| {
                // Per eccesso: un ritardo troncato permetterebbe di ritentare
                // prima del consentito.
                u64::try_from(delay.as_nanos().div_ceil(1_000_000))
                    .ok()
                    .filter(|ms| *ms <= MAX_RETRY_DELAY_MS)
                    .ok_or_else(|| S::Error::custom("delay_ms oltre il massimo del contratto"))
            })
            .transpose()?;
        let mut map = serializer.serialize_map(Some(1 + usize::from(delay_ms.is_some())))?;
        map.serialize_entry("kind", self.as_str())?;
        if let Some(delay_ms) = delay_ms {
            map.serialize_entry("delay_ms", &delay_ms)?;
        }
        map.end()
    }
}

impl PlenoraError {
    /// Codice stabile dell'errore, per il campo `code` di
    /// `plenora-error-v1`; `None` se l'errore non ne ha uno.
    ///
    /// - `Timeout`: [`CODE_DEADLINE_EXCEEDED`]; `Cancelled`:
    ///   [`CODE_CANCELLED`];
    /// - `CrsCoded`: il codice di [`crate::crs::CrsError`], tipizzato;
    /// - i wrapper delegano alla sorgente, le altre varianti (anche `Crs`,
    ///   un messaggio senza codice) non hanno codice.
    ///
    /// Il codice è un'informazione in più: la sua assenza non cambia il
    /// significato degli assi (ERR-013).
    #[must_use]
    pub const fn code(&self) -> Option<&'static str> {
        match self {
            Self::Timeout(_) => Some(CODE_DEADLINE_EXCEEDED),
            Self::Cancelled(_) => Some(CODE_CANCELLED),
            Self::CrsCoded { code, .. } => Some(code.as_str()),
            Self::Tagged { source, .. }
            | Self::RowDiagnostics { source, .. }
            | Self::WithRemoteEffect { source, .. } => source.code(),
            Self::InvalidPlan(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::DataMapping(_)
            | Self::Execution { .. }
            | Self::Crs(_)
            | Self::ResourceLimit(_)
            | Self::Io(_)
            | Self::Protocol(_)
            | Self::Conflict(_)
            | Self::InvalidConfiguration(_)
            | Self::Internal(_) => None,
        }
    }

    /// Il documento `plenora-error-v1` dell'errore.
    ///
    /// Gli assi sono quelli dei metodi omonimi, con due correzioni del
    /// contratto che oggi non scattano (nessun errore del workspace le
    /// richiede) ma valgono anche per un errore costruito a mano:
    /// effetto `unknown` con ritentativo `safe`, `after` o
    /// `requires_idempotency_key` diventa `requires_recovery` (ERR-006);
    /// `after` oltre un giorno diventa `never`.
    ///
    /// Se `details` supera i limiti ERR-011/ERR-012 (una diagnostica per
    /// riga con troppi esempi o cause), la proiezione è un errore
    /// `internal` con [`CODE_DETAILS_NOT_PUBLISHABLE`], stessa fase ed effetto,
    /// ritentativo `never` e senza `details`: mai un documento troncato.
    #[must_use]
    pub fn public_projection(&self) -> PublicError {
        let remote_effect = self.remote_effect();
        let retry = retry_pubblico(remote_effect, self.retry_disposition());
        let details = self
            .row_diagnostics()
            .map(|diagnostica| serde_json::to_value(diagnostica).map(wrap_row));
        let proiezione = PublicError {
            category: self.category(),
            phase: self.phase(),
            remote_effect,
            retry,
            code: self.code(),
            message: messaggio_limitato(&self.messaggio_pubblico()),
            details: match details {
                Some(Ok(details)) => Some(details),
                // Non serializzabile = non supera `validate_for_emission`:
                // `with_row_diagnostics` non lo allega, ma la variante
                // `RowDiagnostics` si può costruire anche a mano.
                Some(Err(_)) => return sostitutiva(self, "diagnostica per riga non valida"),
                None => None,
            },
        };
        if !entro_i_limiti(&proiezione) {
            return sostitutiva(
                self,
                "details oltre i limiti di plenora-error-v1 (ERR-011, ERR-012)",
            );
        }
        proiezione
    }
}

impl PlenoraError {
    /// Il testo pubblico: il `Display`, tranne per `Io`, dove il testo del
    /// sistema operativo (che può contenere percorsi) lascia il posto a un
    /// testo fisso per `ErrorKind`, con i soli contesti nostri davanti
    /// (ERR-009, ERR-010).
    fn messaggio_pubblico(&self) -> String {
        match self {
            Self::Io(errore) => {
                let mut testo = String::from("io error: ");
                for contesto in super::contesti_io(errore) {
                    testo.push_str(contesto);
                    testo.push_str(": ");
                }
                testo.push_str(&errore.kind().to_string());
                testo
            }
            Self::Tagged { source, .. }
            | Self::RowDiagnostics { source, .. }
            | Self::WithRemoteEffect { source, .. } => source.messaggio_pubblico(),
            altro => altro.to_string(),
        }
    }
}

/// ERR-006 ed ERR-007: con effetto `unknown` niente ritentativo
/// automatico (`requires_recovery`); `after` oltre un giorno diventa `never`.
fn retry_pubblico(effetto: RemoteEffect, retry: RetryDisposition) -> RetryDisposition {
    match (effetto, retry) {
        (
            RemoteEffect::Unknown,
            RetryDisposition::Safe
            | RetryDisposition::After(_)
            | RetryDisposition::RequiresIdempotencyKey,
        ) => RetryDisposition::RequiresRecovery,
        (_, RetryDisposition::After(delay))
            if delay.as_nanos().div_ceil(1_000_000) > u128::from(MAX_RETRY_DELAY_MS) =>
        {
            RetryDisposition::Never
        }
        (_, retry) => retry,
    }
}

fn wrap_row(row_diagnostics: Value) -> Value {
    let mut details = serde_json::Map::new();
    details.insert("row_diagnostics".to_owned(), row_diagnostics);
    Value::Object(details)
}

/// Il messaggio entro [`MAX_MESSAGE_CHARS`] caratteri, mai vuoto.
fn messaggio_limitato(testo: &str) -> String {
    let limitato: String = testo.chars().take(MAX_MESSAGE_CHARS).collect();
    if limitato.is_empty() {
        "errore senza messaggio".to_owned()
    } else {
        limitato
    }
}

/// Proiezione esplicita al posto di una che violerebbe il contratto.
fn sostitutiva(errore: &PlenoraError, motivo: &str) -> PublicError {
    PublicError {
        category: ErrorCategory::Internal,
        phase: errore.phase(),
        remote_effect: errore.remote_effect(),
        retry: RetryDisposition::Never,
        code: Some(CODE_DETAILS_NOT_PUBLISHABLE),
        message: messaggio_limitato(&format!(
            "{motivo}; errore originale: {}",
            errore.messaggio_pubblico()
        )),
        details: None,
    }
}

/// ERR-011 sull'errore intero e su `details`, ERR-012 su `details`.
fn entro_i_limiti(proiezione: &PublicError) -> bool {
    let Ok(intero) = serde_json::to_vec(proiezione) else {
        return false;
    };
    if intero.len() > MAX_ERROR_BYTES {
        return false;
    }
    let Some(details) = &proiezione.details else {
        return true;
    };
    let Ok(compatto) = serde_json::to_vec(details) else {
        return false;
    };
    let mut nodi = 0_usize;
    compatto.len() <= MAX_DETAILS_BYTES && struttura_entro_i_limiti(details, 1, &mut nodi)
}

/// ERR-012: profondità (`details` = 1), proprietà ed elementi, byte delle
/// stringhe (anche dei nomi di proprietà, più severo del contratto), nodi.
fn struttura_entro_i_limiti(valore: &Value, profondita: usize, nodi: &mut usize) -> bool {
    *nodi += 1;
    if *nodi > MAX_DETAILS_NODES || profondita > MAX_DETAILS_DEPTH {
        return false;
    }
    match valore {
        Value::Object(oggetto) => {
            oggetto.len() <= MAX_DETAILS_FANOUT
                && oggetto.iter().all(|(nome, figlio)| {
                    nome.len() <= MAX_DETAILS_STRING_BYTES
                        && struttura_entro_i_limiti(figlio, profondita + 1, nodi)
                })
        }
        Value::Array(elementi) => {
            elementi.len() <= MAX_DETAILS_FANOUT
                && elementi
                    .iter()
                    .all(|figlio| struttura_entro_i_limiti(figlio, profondita + 1, nodi))
        }
        Value::String(testo) => testo.len() <= MAX_DETAILS_STRING_BYTES,
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::diagnostics::tests::{example, report};

    #[test]
    fn ogni_crs_error_ha_il_suo_codice_in_testa_al_messaggio() {
        use crate::crs::{CoordinateDomainViolation, CrsError, CrsKind};
        let campioni = [
            CrsError::Required { name: "crs" },
            CrsError::InvalidDefinition {
                name: "crs",
                reason: "r".to_owned(),
            },
            CrsError::BackendUnavailable,
            CrsError::NotBuiltin,
            CrsError::UnsupportedType("t".to_owned()),
            CrsError::MissingLinearUnit,
            CrsError::ProjectedRequired {
                actual: CrsKind::Geographic,
            },
            CrsError::GeographicRequired {
                actual: CrsKind::Projected,
            },
            CrsError::EllipsoidRequired,
            CrsError::Mismatch,
            CrsError::CoordinateOutOfDomain {
                violation: CoordinateDomainViolation::NonFinite,
            },
            CrsError::InvalidContract("c"),
            CrsError::ReprojectionPathUnavailable,
            CrsError::ReprojectionAccuracyNotAccepted { accuracy_m: 1.0 },
            CrsError::ReprojectionConfig("c"),
            CrsError::ReprojectionOutsideTransformationArea,
            CrsError::ReprojectionMixedTransformationAreas,
            CrsError::ReprojectionNotConverged,
            CrsError::ReprojectionEdgeNotConverged,
            CrsError::GridUnreadable,
            CrsError::GridInvalid { reason: "r" },
        ];
        let mut visti = std::collections::BTreeSet::new();
        for campione in campioni {
            let codice = campione.code().as_str();
            assert!(
                codice.len() >= 2
                    && codice.len() <= 64
                    && codice.starts_with(|c: char| c.is_ascii_uppercase())
                    && codice
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                "{codice}: fuori dal pattern di plenora-error-v1"
            );
            assert!(campione.to_string().starts_with(&format!("{codice}: ")));
            assert!(visti.insert(codice), "{codice} ripetuto");
            // Il codice viaggia tipizzato, anche sotto un contesto.
            let errore = PlenoraError::from(campione).con_contesto("geo.buffer");
            assert_eq!(errore.code(), Some(codice), "{errore}");
            assert_eq!(errore.category(), ErrorCategory::Crs);
        }
        // Il messaggio non si interpreta: un `Crs` di solo testo non ha
        // codice, anche se ne contiene uno.
        assert_eq!(PlenoraError::Crs("CRS_NOT_BUILTIN: x".into()).code(), None);
    }

    #[test]
    fn la_proiezione_porta_gli_assi_e_il_codice() {
        let timeout = PlenoraError::Timeout("prima del passo `x`".into()).public_projection();
        // Gli stessi assi del vettore `data-run-timeout-error.json` del
        // contratto, tranne la fase (qui `write`: il controllo scatta fra i
        // passi).
        assert_eq!(
            serde_json::to_value(&timeout).expect("serializzabile"),
            json!({
                "category": "timeout",
                "phase": "write",
                "remote_effect": "none",
                "retry": {"kind": "safe"},
                "code": "EXECUTION_DEADLINE_EXCEEDED",
                "message": "timeout: prima del passo `x`"
            })
        );
        let annullato = PlenoraError::Cancelled("c".into()).public_projection();
        assert_eq!(annullato.category(), ErrorCategory::Cancelled);
        assert_eq!(annullato.code(), Some(CODE_CANCELLED));
        let senza_codice = PlenoraError::Schema("s".into()).public_projection();
        assert_eq!(senza_codice.code(), None);
        assert_eq!(senza_codice.retry(), RetryDisposition::Never);
    }

    #[test]
    fn il_messaggio_e_limitato_e_mai_vuoto() {
        let lungo = PlenoraError::Internal("é".repeat(5_000)).public_projection();
        assert_eq!(lungo.message().chars().count(), MAX_MESSAGE_CHARS);
        let vuoto = PlenoraError::DataMapping(String::new()).public_projection();
        assert!(!vuoto.message().is_empty());
    }

    #[test]
    fn retry_kind_e_delay_ms() {
        let dopo = RetryDisposition::After(Duration::from_millis(1_500));
        assert_eq!(
            serde_json::to_value(dopo).expect("serializzabile"),
            json!({"kind": "after", "delay_ms": 1500})
        );
        assert_eq!(
            serde_json::to_value(RetryDisposition::Quarantine).expect("serializzabile"),
            json!({"kind": "quarantine"})
        );
        assert!(
            serde_json::to_value(RetryDisposition::After(Duration::from_secs(86_401))).is_err()
        );
        // Per eccesso: 1,5 ms non diventa 1 ms.
        assert_eq!(
            serde_json::to_value(RetryDisposition::After(Duration::from_micros(1_500)))
                .expect("serializzabile"),
            json!({"kind": "after", "delay_ms": 2})
        );
    }

    #[test]
    fn la_diagnostica_va_in_details_row_diagnostics() {
        let diagnostica = report(1, vec![example(0)]);
        let errore =
            PlenoraError::DataMapping("rifiutate".into()).with_row_diagnostics(diagnostica.clone());
        let proiezione = errore.public_projection();
        assert_eq!(
            proiezione.details(),
            Some(&json!({"row_diagnostics": serde_json::to_value(&diagnostica).expect("valida")}))
        );
    }

    #[test]
    fn details_oltre_i_limiti_diventa_un_errore_interno_esplicito() {
        // 200 cause: l'oggetto `counts` supera le 128 proprietà (ERR-012).
        let mut diagnostica = report(0, Vec::new());
        diagnostica.counts = (0..200)
            .map(|indice| (format!("conversion.causa_{indice}"), 1))
            .collect::<BTreeMap<_, _>>();
        diagnostica.observed_total = 200;
        diagnostica.total = Some(200);
        diagnostica.examples_limit = 1;
        let mut esempio = example(0);
        "conversion.causa_0".clone_into(&mut esempio.cause);
        diagnostica.examples = vec![esempio];
        diagnostica.examples_truncated = true;
        assert_eq!(diagnostica.validate_for_emission(), Ok(()));
        let proiezione = PlenoraError::DataMapping("rifiutate".into())
            .with_row_diagnostics(diagnostica)
            .public_projection();
        assert_eq!(proiezione.category(), ErrorCategory::Internal);
        assert_eq!(proiezione.code(), Some(CODE_DETAILS_NOT_PUBLISHABLE));
        assert_eq!(proiezione.details(), None);
        assert_eq!(proiezione.retry(), RetryDisposition::Never);
    }

    #[test]
    fn i_limiti_strutturali_di_details() {
        let mut nodi = 0;
        assert!(struttura_entro_i_limiti(
            &json!({"a": [1, 2]}),
            1,
            &mut nodi
        ));
        assert_eq!(nodi, 4);
        // Profondità 9: `details` è 1, ogni annidamento aggiunge 1.
        // Profondità 8 esatta (scalare a 8): ammessa.
        let mut nodi = 0;
        assert!(struttura_entro_i_limiti(
            &json!({"a": [[[[[[0]]]]]]}),
            1,
            &mut nodi
        ));
        let mut profondo = json!(0);
        for _ in 0..7 {
            profondo = json!([profondo]);
        }
        let mut nodi = 0;
        assert!(!struttura_entro_i_limiti(
            &json!({"a": profondo}),
            1,
            &mut nodi
        ));
        let mut nodi = 0;
        assert!(!struttura_entro_i_limiti(
            &json!({"s": "x".repeat(MAX_DETAILS_STRING_BYTES + 1)}),
            1,
            &mut nodi
        ));
        let mut nodi = 0;
        assert!(!struttura_entro_i_limiti(
            &json!({"a": vec![0; MAX_DETAILS_FANOUT + 1]}),
            1,
            &mut nodi
        ));
        // 128 array da 15 elementi: ogni contenitore nei limiti, 2 049 nodi.
        let molti: serde_json::Map<String, Value> = (0..128)
            .map(|indice| (format!("k{indice}"), json!(vec![0; 15])))
            .collect();
        let mut nodi = 0;
        assert!(!struttura_entro_i_limiti(
            &Value::Object(molti),
            1,
            &mut nodi
        ));
    }

    #[test]
    fn effetto_ignoto_non_ammette_ritentativo_automatico() {
        // Nessuna variante produce `unknown`: la regola si prova sulla
        // funzione che la proiezione applica.
        let ignoto = |retry| retry_pubblico(RemoteEffect::Unknown, retry);
        for automatico in [
            RetryDisposition::Safe,
            RetryDisposition::After(Duration::from_millis(1)),
            RetryDisposition::RequiresIdempotencyKey,
        ] {
            assert_eq!(ignoto(automatico), RetryDisposition::RequiresRecovery);
        }
        for ammesso in [
            RetryDisposition::Never,
            RetryDisposition::Quarantine,
            RetryDisposition::RequiresRecovery,
        ] {
            assert_eq!(ignoto(ammesso), ammesso);
        }
        assert_eq!(
            retry_pubblico(RemoteEffect::None, RetryDisposition::Safe),
            RetryDisposition::Safe
        );
        assert_eq!(
            retry_pubblico(
                RemoteEffect::None,
                RetryDisposition::After(Duration::from_secs(86_401))
            ),
            RetryDisposition::Never
        );
    }
}
