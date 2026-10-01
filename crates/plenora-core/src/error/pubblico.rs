//! Forma pubblica di [`PlenoraError`]: il documento `plenora-error-v1`
//! (`plenora-contracts`, `specs/errors/ERRORS-1.0.md` e
//! `schemas/error-v1.schema.json`).
//!
//! La proiezione non aggiunge dati: il messaggio Ã¨ il `Display` dell'errore
//! (giÃ  senza valori di righe o colonne, regola del modulo padre), troncato
//! a [`MAX_MESSAGE_CHARS`] caratteri; `details` porta solo il documento
//! `plenora-row-diagnostics-v1` (indici, conteggi e codici, mai valori).
//!
//! Limiti del contratto applicati qui, oltre allo schema: ERR-006 (effetto
//! ignoto, nessun ritentativo automatico), ERR-007 (`delay_ms` solo con
//! `after`, entro un giorno), ERR-011 e ERR-012 (byte, profonditÃ ,
//! proprietÃ , elementi, stringhe e nodi di `details`). Un `details` oltre i
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
/// ERR-012: profonditÃ  di `details` (l'oggetto `details` Ã¨ profonditÃ  1).
pub const MAX_DETAILS_DEPTH: usize = 8;
/// ERR-012: proprietÃ  di un oggetto, elementi di un array.
pub const MAX_DETAILS_FANOUT: usize = 128;
/// ERR-012: byte UTF-8 di una stringa.
pub const MAX_DETAILS_STRING_BYTES: usize = 4_096;
/// ERR-012: nodi JSON (contenitori e scalari).
pub const MAX_DETAILS_NODES: usize = 2_048;

/// Codice di [`PlenoraError::Timeout`]: la scadenza dell'esecuzione Ã¨
/// passata (vettore `data-run-timeout-error` del contratto).
pub const CODE_DEADLINE_EXCEEDED: &str = "EXECUTION_DEADLINE_EXCEEDED";
/// Codice di [`PlenoraError::Cancelled`].
pub const CODE_CANCELLED: &str = "EXECUTION_CANCELLED";
/// Codice della proiezione sostitutiva quando `details` non Ã¨ pubblicabile:
/// oltre i limiti ERR-011/ERR-012, o diagnostica per riga non valida.
pub const CODE_DETAILS_NOT_PUBLISHABLE: &str = "ERROR_DETAILS_NOT_PUBLISHABLE";

/// Documento `plenora-error-v1` di un [`PlenoraError`]; si ottiene con
/// [`PlenoraError::public_projection`] e si serializza con `serde`.
///
/// `provider` ed `execution_id` non ci sono: nessun errore di questo
/// workspace ne ha uno.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PublicError {
    pub category: ErrorCategory,
    pub phase: ErrorPhase,
    pub remote_effect: RemoteEffect,
    pub retry: RetryDisposition,
    /// Codice stabile (`^[A-Z][A-Z0-9_]{1,63}$`), se l'errore ne ha uno
    /// ([`PlenoraError::code`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Non vuoto, al piÃ¹ [`MAX_MESSAGE_CHARS`] caratteri.
    pub message: String,
    /// `{"row_diagnostics": <plenora-row-diagnostics-v1>}`, se l'errore ha
    /// una diagnostica per riga.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
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

/// Codici in testa ai messaggi di [`crate::crs::CrsError`] (e di chi ne
/// ripete uno, come `plenora-io` per `CRS_NOT_BUILTIN`): solo questi
/// diventano `code` di un errore `Crs`. Un elenco chiuso, perchÃ© un
/// segmento del messaggio in maiuscole non Ã¨ per forza un codice.
const CODICI_CRS: &[&str] = &[
    "CRS_REQUIRED",
    "CRS_INVALID",
    "CRS_BACKEND_UNAVAILABLE",
    "CRS_NOT_BUILTIN",
    "CRS_TYPE_UNSUPPORTED",
    "LINEAR_UNIT_REQUIRED",
    "PROJECTED_CRS_REQUIRED",
    "GEOGRAPHIC_CRS_REQUIRED",
    "ELLIPSOID_REQUIRED",
    "CRS_MISMATCH",
    "COORDINATE_OUT_OF_CRS_DOMAIN",
    "CRS_CONTRACT_INVALID",
    "REPROJECTION_PATH_UNAVAILABLE",
    "REPROJECTION_ACCURACY_NOT_ACCEPTED",
    "REPROJECTION_CONFIG_INVALID",
    "REPROJECTION_OUTSIDE_TRANSFORMATION_AREA",
    "REPROJECTION_MIXED_TRANSFORMATION_AREAS",
    "REPROJECTION_NOT_CONVERGED",
    "REPROJECTION_EDGE_NOT_CONVERGED",
    "NTV2_GRID_UNREADABLE",
    "NTV2_GRID_INVALID",
];

impl PlenoraError {
    /// Codice stabile dell'errore, per il campo `code` di
    /// `plenora-error-v1`; `None` se l'errore non ne ha uno.
    ///
    /// - `Timeout`: [`CODE_DEADLINE_EXCEEDED`]; `Cancelled`:
    ///   [`CODE_CANCELLED`];
    /// - `Crs`: il codice di [`crate::crs::CrsError`] che il messaggio porta
    ///   in un suo segmento (separatore `": "`, dopo gli eventuali contesti
    ///   anteposti), se Ã¨ fra quelli noti;
    /// - i wrapper delegano alla sorgente, le altre varianti non hanno
    ///   codice.
    ///
    /// Il codice Ã¨ un'informazione in piÃ¹: la sua assenza non cambia il
    /// significato degli assi (ERR-013).
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Timeout(_) => Some(CODE_DEADLINE_EXCEEDED),
            Self::Cancelled(_) => Some(CODE_CANCELLED),
            Self::Crs(messaggio) => messaggio
                .split(": ")
                .find(|segmento| CODICI_CRS.contains(segmento)),
            Self::Tagged { source, .. } | Self::RowDiagnostics { source, .. } => source.code(),
            Self::InvalidPlan(_)
            | Self::Unsupported(_)
            | Self::Schema(_)
            | Self::DataMapping(_)
            | Self::Execution { .. }
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
    /// riga con troppi esempi o cause), la proiezione Ã¨ un errore
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
            code: self.code().map(str::to_owned),
            message: messaggio_limitato(&self.to_string()),
            details: match details {
                Some(Ok(details)) => Some(details),
                // Non serializzabile = non supera `validate_for_emission`:
                // `with_row_diagnostics` non lo allega, ma la variante
                // `RowDiagnostics` si puÃ² costruire anche a mano.
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
        code: Some(CODE_DETAILS_NOT_PUBLISHABLE.to_owned()),
        message: messaggio_limitato(&format!("{motivo}; errore originale: {errore}")),
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

/// ERR-012: profonditÃ  (`details` = 1), proprietÃ  ed elementi, byte delle
/// stringhe (anche dei nomi di proprietÃ , piÃ¹ severo del contratto), nodi.
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
    fn i_codici_crs_sono_quelli_in_testa_ai_messaggi_di_crs_error() {
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
        assert_eq!(campioni.len(), CODICI_CRS.len(), "un campione per codice");
        for (campione, atteso) in campioni.into_iter().zip(CODICI_CRS) {
            let errore = PlenoraError::from(campione).con_contesto("geo.buffer");
            assert_eq!(errore.code(), Some(*atteso), "{errore}");
        }
        // Un segmento in maiuscole che non Ã¨ un codice noto non diventa
        // `code` (per esempio il nome di una colonna).
        assert_eq!(PlenoraError::Crs("COLONNA_X: motivo".into()).code(), None);
        assert_eq!(PlenoraError::Crs("crs obbligatorio".into()).code(), None);
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
        assert_eq!(annullato.category, ErrorCategory::Cancelled);
        assert_eq!(annullato.code.as_deref(), Some(CODE_CANCELLED));
        let senza_codice = PlenoraError::Schema("s".into()).public_projection();
        assert_eq!(senza_codice.code, None);
        assert_eq!(senza_codice.retry, RetryDisposition::Never);
    }

    #[test]
    fn il_messaggio_e_limitato_e_mai_vuoto() {
        let lungo = PlenoraError::Internal("Ã©".repeat(5_000)).public_projection();
        assert_eq!(lungo.message.chars().count(), MAX_MESSAGE_CHARS);
        let vuoto = PlenoraError::DataMapping(String::new()).public_projection();
        assert!(!vuoto.message.is_empty());
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
            proiezione.details,
            Some(json!({"row_diagnostics": serde_json::to_value(&diagnostica).expect("valida")}))
        );
    }

    #[test]
    fn details_oltre_i_limiti_diventa_un_errore_interno_esplicito() {
        // 200 cause: l'oggetto `counts` supera le 128 proprietÃ  (ERR-012).
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
        assert_eq!(proiezione.category, ErrorCategory::Internal);
        assert_eq!(
            proiezione.code.as_deref(),
            Some(CODE_DETAILS_NOT_PUBLISHABLE)
        );
        assert_eq!(proiezione.details, None);
        assert_eq!(proiezione.retry, RetryDisposition::Never);
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
        // ProfonditÃ  9: `details` Ã¨ 1, ogni annidamento aggiunge 1.
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
