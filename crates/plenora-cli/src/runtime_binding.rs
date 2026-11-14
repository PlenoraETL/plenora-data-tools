//! Runtime Binding 2.0 al pin 38d68b4: wire 1, capability plenora.data-tools.
//!
//! Ammissione prima dei resolver; i calcoli chiamano esclusivamente le API comuni.
//! Il risultato @3 resta chiuso: gli effetti C-01 sono un metadato opzionale
//! component-owned, separato dal payload del manifesto artefatti.
use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::api::RisolutoreArtefatti;
use crate::operazioni::{OperazionePubblica, ARROW_FILE, ARROW_STREAM, JSON, OPERAZIONI, PARQUET};
use plenora_core::{ErrorPhase, PlenoraError, RemoteEffect, Result};
use plenora_pipeline::{Interruzione, Pipeline};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Nome della capability, distinto dal componente e dall'artefatto CLI.
pub const CAPABILITY_NAME: &str = "plenora.data-tools";
/// Versione del wire, non della specifica (RT-025).
pub const RUNTIME_BINDING_VERSION: u32 = 1;
/// Discovery indipendente dalle operazioni di dominio.
pub const DISCOVERY_ENTRYPOINT: &str = "plenora.data-tools#capabilities@1";
/// Tetto del risultato JSON compatto, senza troncamento (RT-024).
pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
/// Tetto del documento JSON della richiesta; i dati Arrow hanno il budget comune.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MESSAGE_ID: &str = "plenora.message.id";
const CAUSATION_ID: &str = "plenora.message.causation_id";
const CORRELATION_ID: &str = "plenora.trace.correlation_id";
const OPERATION: &str = "plenora.capability.operation";
const OPERATION_VERSION: &str = "plenora.operation.version";
const OUTPUT_CONTRACT: &str = "plenora.output.contract";
const DEADLINE: &str = "plenora.execution.deadline";
const EFFECTS: &str = "plenora.data.localized_effects";

/// Payload del trasporto: JSON UTF-8 oppure byte Arrow, senza percorsi locali.
#[derive(Clone, Debug)]
pub struct RuntimeInvocation {
    pub content_type: String,
    pub metadata: BTreeMap<String, Value>,
    pub payload: Vec<u8>,
}

/// Risultato terminale. Le chiavi dei metadati sono quelle del wire normativo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeResult {
    pub content_type: String,
    pub metadata: BTreeMap<String, String>,
    pub payload: Vec<u8>,
}

/// Discovery della superficie realmente implementata, dalla tabella comune.
#[must_use]
pub fn runtime_capabilities() -> Value {
    crate::capacita::documento_della(crate::capacita::Superficie::Runtime)
}

/// Entrypoint canonico di una versione dell'operazione.
#[must_use]
pub fn entrypoint(operation: &str, version: u32) -> String {
    format!("{CAPABILITY_NAME}#{operation}@{version}")
}

/// Binding ospitato dall'applicazione; nessuna dipendenza da runtime-tools.
pub struct RuntimeBinding<'a> {
    pub resolver: &'a dyn RisolutoreArtefatti,
}

impl RuntimeInvocation {
    fn optional(&self, key: &str, grammar: fn(&str) -> bool) -> Result<Option<&str>> {
        match self.metadata.get(key) {
            None => Ok(None),
            Some(Value::String(value)) if grammar(value) => Ok(Some(value)),
            _ => Err(protocol("metadato runtime facoltativo malformato")),
        }
    }
    fn required(&self, key: &str, grammar: fn(&str) -> bool) -> Result<&str> {
        self.optional(key, grammar)?
            .ok_or_else(|| protocol("metadato runtime obbligatorio assente"))
    }
}

impl RuntimeBinding<'_> {
    /// Ammette ed esegue una richiesta. Ogni errore e' un risultato pubblico.
    /// Controlli malformati precedono selettori non serviti e deadline scadute.
    #[must_use]
    pub fn invoke(&self, invocation: &RuntimeInvocation, context: &Interruzione) -> RuntimeResult {
        let identity = result_identity(invocation);
        let admitted = admit(invocation, context);
        let (operation, interruption) = match admitted {
            Ok(value) => value,
            Err(error) => return rejection_result(identity, &error),
        };
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.execute(invocation, operation, &interruption)
        }));
        let result = attempt.unwrap_or_else(|_| {
            Err(
                PlenoraError::Internal("panico nel binding runtime".into()).override_remote_effect(
                    if operation.effetto == crate::operazioni::Effetto::Remoto {
                        RemoteEffect::Unknown
                    } else {
                        RemoteEffect::None
                    },
                ),
            )
        });
        match result {
            Ok((payload, report)) => {
                let mut metadata = identity;
                metadata.insert(OUTPUT_CONTRACT.into(), operation.uscita.into());
                if let Some(report) = report {
                    metadata.insert(EFFECTS.into(), report.to_string());
                }
                if let Err(error) = result_budget(
                    &payload,
                    &metadata,
                    operation.id == "data.run",
                    MAX_RESULT_BYTES,
                ) {
                    return error_result(metadata, &error);
                }
                RuntimeResult {
                    content_type: JSON.into(),
                    metadata,
                    payload,
                }
            }
            Err(error) => error_result(identity, &error),
        }
    }

    fn execute(
        &self,
        invocation: &RuntimeInvocation,
        operation: &OperazionePubblica,
        interruption: &Interruzione,
    ) -> Result<(Vec<u8>, Option<Value>)> {
        interruption
            .verifica("prima dell'invocazione runtime")
            .map_err(|e| e.with_phase(ErrorPhase::Validate))?;
        let (document, report) = match operation.id {
            "data.catalog" => {
                let query: Value = json_payload(invocation)?;
                if query != json!({}) {
                    return Err(configuration("query catalogo non vuota"));
                }
                (crate::api::catalogo(), None)
            }
            "data.describe" => (
                with_arrow(invocation, |path| crate::api::descrivi(path, interruption))?,
                None,
            ),
            "data.validate" if invocation.content_type == JSON => {
                let text = json_text(invocation)?;
                (
                    crate::artefatti::valida_artefatti(text, self.resolver, interruption)?,
                    None,
                )
            }
            "data.validate" => {
                let plan = invocation.required("plenora.data.plan", |s| !s.is_empty())?;
                let plan = Pipeline::from_json(plan)?;
                if plan.inputs.len() != 1 {
                    return Err(configuration(
                        "payload Arrow richiede un solo input del piano",
                    ));
                }
                let result = with_arrow(invocation, |path| {
                    crate::api::valida_ingressi(
                        &plan,
                        vec![plenora_io::Ingresso::File(plenora_io::FileIngresso {
                            nome: plan.inputs[0].clone(),
                            percorso: path.to_path_buf(),
                            formato: None,
                        })],
                        interruption,
                    )
                })?;
                (result, None)
            }
            "data.run" => {
                let (document, report) = crate::artefatti::esegui_artefatti_con_report(
                    json_text(invocation)?,
                    self.resolver,
                    interruption,
                )?;
                let effects = json!({"contract": plenora_core::contract::geometry_metadata_policy::report::CONTRACT,
                    "steps": report.passi.iter().map(|p| json!({"out":p.out,"op":p.op,"effects":p.effetti_localizzati})).collect::<Vec<_>>()});
                (document, Some(effects))
            }
            _ => return Err(unsupported("operazione runtime non implementata")),
        };
        let payload = serde_json::to_vec(&document)
            .map_err(|_| PlenoraError::Internal("risultato runtime non serializzabile".into()))?;
        Ok((payload, report))
    }
}

fn result_budget(
    payload: &[u8],
    metadata: &BTreeMap<String, String>,
    committed: bool,
    limit: usize,
) -> Result<()> {
    let bytes = serde_json::to_vec(metadata)
        .map_err(|_| PlenoraError::Internal("metadati runtime non serializzabili".into()))?;
    if payload.len().saturating_add(bytes.len()) > limit {
        return Err(PlenoraError::ResourceLimit(
            "risultato runtime oltre il limite dichiarato".into(),
        )
        .with_phase(ErrorPhase::Finalize)
        .override_remote_effect(if committed {
            RemoteEffect::Committed
        } else {
            RemoteEffect::None
        }));
    }
    Ok(())
}

fn admit(
    invocation: &RuntimeInvocation,
    context: &Interruzione,
) -> Result<(&'static OperazionePubblica, Interruzione)> {
    invocation.required(MESSAGE_ID, canonical_uuid)?;
    invocation.required(CORRELATION_ID, canonical_uuid)?;
    invocation.optional(CAUSATION_ID, canonical_uuid)?;
    let capability = invocation.required("plenora.capability.name", well_formed_capability_name)?;
    let wire = invocation.required("plenora.capability.version", canonical_version)?;
    let requested = invocation.required(OPERATION, well_formed_operation)?;
    let version = invocation.required(OPERATION_VERSION, canonical_version)?;
    let contract = invocation.required("plenora.input.contract", well_formed_contract)?;
    let deadline = invocation.optional(DEADLINE, utc_deadline)?;
    let idempotency = invocation.optional("plenora.execution.idempotency_key", |s| {
        !s.is_empty() && s.len() <= 256
    })?;
    if !well_formed_media_type(&invocation.content_type) {
        return Err(protocol("content type malformato"));
    }
    if capability != CAPABILITY_NAME || wire != "1" {
        return Err(unsupported("capability runtime non servita"));
    }
    let operation = OPERAZIONI
        .iter()
        .find(|o| {
            o.id == requested
                && o.versione.to_string() == version
                && (o.id != "data.run" || o.versione == 3)
        })
        .ok_or_else(|| unsupported("operazione o versione runtime non annunciata"))?;
    if contract != operation.ingresso
        || !(operation
            .tipi_ingresso
            .contains(&invocation.content_type.as_str())
            || (operation.ingresso != crate::artefatti::CONTRATTO_RICHIESTA
                && operation
                    .estensioni_ingresso
                    .contains(&invocation.content_type.as_str())))
    {
        return Err(unsupported("contratto o content type non annunciato"));
    }
    if idempotency.is_some()
        || ((deadline.is_some() || context.scadenza.is_some()) && !operation.scadenza)
        || (context.annullamento.is_some() && !operation.annullamento)
    {
        return Err(unsupported("controllo runtime non supportato"));
    }
    let mut interruption = context.clone();
    if let Some(value) = deadline {
        let parsed = deadline_instant(value)?;
        interruption.scadenza = Some(interruption.scadenza.map_or(parsed, |old| old.min(parsed)));
    }
    interruption
        .verifica("prima dell'invocazione runtime")
        .map_err(|e| e.with_phase(ErrorPhase::Validate))?;
    Ok((operation, interruption))
}

fn protocol(message: &str) -> PlenoraError {
    PlenoraError::Protocol(message.into()).with_phase(ErrorPhase::Validate)
}
fn unsupported(message: &str) -> PlenoraError {
    PlenoraError::Unsupported(message.into()).with_phase(ErrorPhase::Validate)
}
fn configuration(message: &str) -> PlenoraError {
    PlenoraError::InvalidConfiguration(message.into()).with_phase(ErrorPhase::Validate)
}

fn json_text(invocation: &RuntimeInvocation) -> Result<&str> {
    if invocation.payload.len() > MAX_REQUEST_BYTES {
        return Err(
            PlenoraError::ResourceLimit("richiesta runtime oltre il limite".into())
                .with_phase(ErrorPhase::Validate),
        );
    }
    let text =
        std::str::from_utf8(&invocation.payload).map_err(|_| configuration("JSON non UTF-8"))?;
    plenora_core::json::ensure_no_duplicate_keys(text)?;
    plenora_core::json::ensure_numbers_exact(text)?;
    Ok(text)
}
fn json_payload(invocation: &RuntimeInvocation) -> Result<Value> {
    serde_json::from_str(json_text(invocation)?)
        .map_err(|_| configuration("JSON runtime non valido"))
}

fn with_arrow(
    invocation: &RuntimeInvocation,
    execute: impl FnOnce(&std::path::Path) -> Result<Value>,
) -> Result<Value> {
    let suffix = match invocation.content_type.as_str() {
        ARROW_FILE => "arrow",
        ARROW_STREAM => "arrows",
        PARQUET => "parquet",
        _ => return Err(unsupported("tipo Arrow non annunciato")),
    };
    if invocation.payload.len() as u64 > plenora_core::limits::DEFAULT_MAX_GOVERNED_MEMORY_BYTES {
        return Err(
            PlenoraError::ResourceLimit("payload Arrow oltre il budget".into())
                .with_phase(ErrorPhase::Read),
        );
    }
    let formato = match invocation.content_type.as_str() {
        ARROW_FILE => plenora_io::Formato::ArrowIpc,
        ARROW_STREAM => plenora_io::Formato::ArrowIpcStream,
        _ => plenora_io::Formato::Parquet,
    };
    if !crate::artefatti::firma_coerente(formato, &invocation.payload) {
        return Err(
            PlenoraError::DataMapping("contenuto diverso dal tipo dichiarato".into())
                .with_phase(ErrorPhase::Read),
        );
    }
    let directory =
        tempfile::tempdir().map_err(|e| PlenoraError::io_con_contesto("cartella temporanea", e))?;
    let path = directory.path().join(format!("input.{suffix}"));
    std::fs::write(&path, &invocation.payload)
        .map_err(|e| PlenoraError::io_con_contesto("input temporaneo", e))?;
    let result = execute(&path)?;
    directory.close().map_err(|e| {
        PlenoraError::io_con_contesto("cartella temporanea", e).with_phase(ErrorPhase::Cleanup)
    })?;
    Ok(result)
}

fn result_identity(invocation: &RuntimeInvocation) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut hash = Sha256::new()
        .chain_update(b"plenora.data-tools/runtime-result\0")
        .chain_update(invocation.payload.len().to_le_bytes())
        .chain_update(&invocation.payload)
        .chain_update(invocation.content_type.len().to_le_bytes())
        .chain_update(invocation.content_type.as_bytes());
    for (key, value) in &invocation.metadata {
        hash.update(key.len().to_le_bytes());
        hash.update(key.as_bytes());
        let value = value.to_string();
        hash.update(value.len().to_le_bytes());
        hash.update(value.as_bytes());
    }
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 15) | 128;
    bytes[8] = (bytes[8] & 63) | 128;
    let uuid = |bytes: &[u8]| {
        let h = hex(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &h[..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..]
        )
    };
    let mut id = uuid(&bytes);
    if invocation.metadata.get(MESSAGE_ID).and_then(Value::as_str) == Some(&id) {
        bytes[0] ^= 1;
        id = uuid(&bytes);
    }
    result.insert(MESSAGE_ID.into(), id);
    for (input, output, grammar) in [
        (MESSAGE_ID, CAUSATION_ID, canonical_uuid as fn(&str) -> bool),
        (CORRELATION_ID, CORRELATION_ID, canonical_uuid),
        (OPERATION, OPERATION, well_formed_operation),
        (OPERATION_VERSION, OPERATION_VERSION, canonical_version),
    ] {
        if let Some(value) = invocation
            .metadata
            .get(input)
            .and_then(Value::as_str)
            .filter(|v| grammar(v))
        {
            result.insert(output.into(), value.into());
        }
    }
    result
}

fn rejection_result(identity: BTreeMap<String, String>, error: &PlenoraError) -> RuntimeResult {
    let projection = error.public_projection();
    let document = json!({"category":projection.category(), "phase":"validate",
        "remote_effect":"none", "retry":{"kind":"never"}, "message":projection.message()});
    let mut result = error_result(identity, error);
    result.payload = document.to_string().into_bytes();
    result
}

fn error_result(mut identity: BTreeMap<String, String>, error: &PlenoraError) -> RuntimeResult {
    identity.remove(EFFECTS);
    identity.insert(OUTPUT_CONTRACT.into(), "plenora-error-v1".into());
    let payload = serde_json::to_vec(&error.public_projection()).unwrap_or_else(|_| {
        br#"{"category":"internal","phase":"finalize","remote_effect":"unknown","retry":{"kind":"requires_recovery"},"message":"errore runtime non serializzabile"}"#.to_vec()
    });
    RuntimeResult {
        content_type: "application/vnd.plenora.error+json".into(),
        metadata: identity,
        payload,
    }
}

fn utc_deadline(text: &str) -> bool {
    matches!(text.as_bytes().get(10), Some(b'T' | b't'))
        && !text.ends_with("-00:00")
        && chrono::DateTime::parse_from_rfc3339(text)
            .is_ok_and(|v| v.offset().local_minus_utc() == 0)
}
fn deadline_instant(text: &str) -> Result<Instant> {
    let expired = || {
        PlenoraError::Timeout("deadline runtime gia trascorsa".into())
            .with_phase(ErrorPhase::Validate)
    };
    let parsed =
        chrono::DateTime::parse_from_rfc3339(text).map_err(|_| protocol("deadline non valida"))?;
    let seconds = u64::try_from(parsed.timestamp()).map_err(|_| expired())?;
    let absolute = SystemTime::UNIX_EPOCH
        .checked_add(Duration::new(seconds, parsed.timestamp_subsec_nanos()))
        .ok_or_else(|| protocol("deadline fuori intervallo"))?;
    let remaining = absolute
        .duration_since(SystemTime::now())
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or_else(expired)?;
    Instant::now()
        .checked_add(remaining)
        .ok_or_else(|| protocol("deadline non rappresentabile"))
}

fn canonical_version(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) && !value.starts_with('0')
}

fn well_formed_operation(value: &str) -> bool {
    let mut segments = value.split('.');
    let first = segments.next().is_some_and(identifier_segment);
    let mut rest = 0_usize;
    first
        && segments.all(|segment| {
            rest += 1;
            identifier_segment(segment)
        })
        && rest > 0
}

fn identifier_segment(segment: &str) -> bool {
    let mut characters = segment.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|next| {
            next.is_ascii_lowercase() || next.is_ascii_digit() || matches!(next, '_' | '-')
        })
}

fn well_formed_capability_name(value: &str) -> bool {
    value
        .strip_prefix("plenora.")
        .and_then(|rest| rest.strip_suffix("-tools"))
        .is_some_and(|middle| {
            let mut characters = middle.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_lowercase())
                && characters
                    .all(|next| next.is_ascii_lowercase() || next.is_ascii_digit() || next == '-')
        })
}

fn well_formed_contract(value: &str) -> bool {
    value
        .strip_prefix("plenora-")
        .and_then(|rest| rest.rsplit_once("-v"))
        .is_some_and(|(name, version)| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|next| next.is_ascii_lowercase() || next.is_ascii_digit() || next == '-')
                && canonical_version(version)
        })
}

fn well_formed_media_type(value: &str) -> bool {
    let token = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|next| next.is_ascii_alphanumeric() || "!#$&^_.+-".contains(next))
    };
    value
        .split_once('/')
        .is_some_and(|(kind, subtype)| token(kind) && token(subtype))
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn result_limit_counts_metadata_and_preserves_committed() {
        let metadata = BTreeMap::from([(EFFECTS.into(), "quoted\"".repeat(20))]);
        let payload = vec![b'x'; 100];
        let bytes = serde_json::to_vec(&metadata).expect("metadati JSON").len() + payload.len();
        result_budget(&payload, &metadata, true, bytes).expect("limite incluso");
        let error =
            result_budget(&payload, &metadata, true, bytes - 1).expect_err("limite superato");
        assert_eq!(error.category(), plenora_core::ErrorCategory::ResourceLimit);
        assert_eq!(error.phase(), ErrorPhase::Finalize);
        assert_eq!(error.remote_effect(), RemoteEffect::Committed);
        assert_eq!(
            error.retry_disposition(),
            plenora_core::RetryDisposition::Never
        );
        let result = error_result(metadata, &error);
        assert!(!result.metadata.contains_key(EFFECTS));
        assert_eq!(result.metadata[OUTPUT_CONTRACT], "plenora-error-v1");
    }
}
