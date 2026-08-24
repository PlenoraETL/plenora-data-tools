//! Envelope di errore e proiezione della categoria in exit code.
//!
//! Il contratto verso chi automatizza: un solo documento JSON su stdout, con
//! i quattro assi espliciti, e un numero che gli script possono leggere senza
//! parsare JSON. Vive in un modulo proprio perche' e' l'unica superficie che
//! **ogni** fallimento attraversa, qualunque comando l'abbia prodotto.

use std::error::Error;
use std::io::Write;

use plenora_core::{PlenoraError, RetryDisposition};
use plenora_engine::geo_transport::transport::ArrowTransportError;

/// Cancellazione cooperativa (128 + SIGINT).
pub const EXIT_CANCELLED: i32 = 130;

/// Difetto interno (convenzione `sysexits`: `EX_SOFTWARE`).
pub const EXIT_INTERNO: i32 = 70;

/// Envelope di errore §9 su **stdout**, con `stderr` lasciato vuoto.
///
/// **Inversione dichiarata rispetto alla scelta precedente**, che teneva
/// l'envelope su stderr come «contratto pubblico storico».
/// Quella decisione precede l'esistenza di `plenora-database-tools`, che
/// emette gli errori su stdout e lascia stderr vuoto: due componenti della
/// stessa famiglia, orchestrati dallo stesso codice, non possono avere due
/// convenzioni opposte su dove cercare un errore. La rottura per chi oggi
/// parsa stderr e' registrata in `docs/release.md`.
pub fn emit_error_envelope(
    mut stdout: impl Write,
    envelope: &serde_json::Value,
) -> std::io::Result<()> {
    writeln!(stdout, "{envelope}")
}

/// Exit code stabile derivato dalla CATEGORIA dell'envelope.
///
/// La categoria resta la fonte di verita': il codice e' una sua proiezione
/// grossolana, per gli script che non vogliono parsare JSON. Il mapping e'
/// totale su `plenora_core::ErrorCategory` — una categoria nuova che finisse
/// qui senza un codice sarebbe un errore silenzioso, quindi il caso di
/// default e' `70` e un test copre l'intero enum.
///
/// **Non e' allineato a `plenora-database-tools`**, che restituisce `1` per
/// qualunque errore: e' una divergenza dichiarata (cli.md#exit-code).
/// L'unica garanzia condivisa dalla famiglia e' «0 successo, non-zero
/// errore»; chi scrive codice portabile fra i due componenti legge
/// `error.category`, non questo numero.
///
/// | codice | significato |
/// |---|---|
/// | 0 | successo |
/// | 2 | piano o configurazione invalidi |
/// | 3 | contratto, schema o capability incompatibili |
/// | 4 | limite di risorsa superato |
/// | 5 | I/O, pubblicazione, rete o autorizzazioni |
/// | 6 | fallimento di esecuzione di un nodo |
/// | 70 | difetto interno |
/// | 130 | cancellato (128 + SIGINT) |
pub fn error_exit_code(envelope: &serde_json::Value) -> i32 {
    match envelope["error"]["category"].as_str() {
        Some("cancelled") => EXIT_CANCELLED,
        Some("invalid_plan" | "invalid_configuration") => 2,
        Some("schema" | "data_mapping" | "crs" | "unsupported") => 3,
        Some("resource_limit") => 4,
        Some(
            "io" | "not_found" | "conflict" | "protocol" | "authentication" | "authorization"
            | "timeout" | "transient",
        ) => 5,
        Some("execution") => 6,
        _ => EXIT_INTERNO,
    }
}

/// Envelope d'errore a quattro assi (R9.1, `protocol_version` 1): l'uscita
/// CLI riporta categoria, fase, effetto remoto e disposizione di retry
/// espliciti — mai dedotti dal messaggio (R9.2). Una riga JSON su stdout;
/// `message` porta il testo dell'errore invariato. `retry` e' nella forma
/// taggata condivisa (conformance/components.json): `{"kind": ...}` piu'
/// `delay_ms` solo per `after(durata)`. `context` (presente
/// solo per errori nati in un'esecuzione DAG) riporta nodo, operazione ed
/// `execution_id` — la risposta a «quale step ha rotto» senza parsare il
/// messaggio. L'exit code e' la proiezione della categoria
/// ([`error_exit_code`]): 2, 3, 4, 5, 6, 70, piu' 130 per la cancellazione.
///
/// Mapping dichiarato: `PlenoraError` -> i quattro assi del tipo; errori
/// di parametro pubblico del trasporto Arrow -> `invalid_plan`/`validate`/
/// `none`/`never`; errori I/O nudi (lettura piano/argomenti) ->
/// `io`/`read`/`none`/`safe`; errori di parse JSON del piano ->
/// `data_mapping`/`validate`/`none`/`never`; qualunque altro tipo ->
/// `internal`/`validate`/`none`/`never`.
pub fn error_envelope(error: &(dyn Error + 'static), cancelled: bool) -> serde_json::Value {
    let plenora_error = error.downcast_ref::<PlenoraError>();
    let public_transport_parameter_error =
        error
            .downcast_ref::<ArrowTransportError>()
            .is_some_and(|transport| {
                matches!(
                    transport.source_error(),
                    ArrowTransportError::MissingParameter { .. }
                        | ArrowTransportError::UnexpectedParameter { .. }
                        | ArrowTransportError::InvalidParameter { .. }
                )
            });
    let (category, phase, remote_effect, disposition) = plenora_error.map_or_else(
        || {
            if public_transport_parameter_error {
                ("invalid_plan", "validate", "none", RetryDisposition::Never)
            } else if error.downcast_ref::<std::io::Error>().is_some() {
                ("io", "read", "none", RetryDisposition::Safe)
            } else if error.downcast_ref::<serde_json::Error>().is_some() {
                ("data_mapping", "validate", "none", RetryDisposition::Never)
            } else {
                ("internal", "validate", "none", RetryDisposition::Never)
            }
        },
        |plenora| {
            (
                plenora.category().as_str(),
                plenora.phase().as_str(),
                plenora.remote_effect().as_str(),
                plenora.retry_disposition(),
            )
        },
    );
    // Forma taggata fissata in conformance/components.json
    // (required_capability_shared): {"kind": ...} e, solo per
    // `after(durata)`, "delay_ms" — altrimenti il chiamante saprebbe DI
    // riprovare piu' tardi senza sapere QUANDO (R9.2/R9.7).
    let mut retry = serde_json::json!({ "kind": disposition.as_str() });
    if let Some(delay) = disposition.delay() {
        retry["delay_ms"] =
            serde_json::Value::from(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX));
    }
    let message = if cancelled {
        format!("esecuzione annullata: {error}")
    } else {
        error.to_string()
    };
    let mut body = serde_json::json!({
        "category": category,
        "phase": phase,
        "remote_effect": remote_effect,
        "retry": retry,
        "message": message,
    });
    if let Some((node, operation, execution_id)) =
        plenora_error.and_then(PlenoraError::execution_location)
    {
        let mut context = serde_json::json!({ "node": node, "operation": operation });
        if let Some(execution_id) = execution_id {
            context["execution_id"] = serde_json::Value::String(execution_id.to_owned());
        }
        body["context"] = context;
    }
    if let Some(diagnostics) = plenora_error.and_then(PlenoraError::row_diagnostics) {
        if let Ok(value) = serde_json::to_value(diagnostics) {
            body["row_diagnostics"] = value;
        } else {
            body["category"] = serde_json::Value::String("internal".to_owned());
            body["phase"] = serde_json::Value::String("write".to_owned());
            body["remote_effect"] = serde_json::Value::String("none".to_owned());
            body["retry"] = serde_json::json!({ "kind": "never" });
            body["message"] =
                serde_json::Value::String("row diagnostics interne non valide".to_owned());
        }
    }
    serde_json::json!({
        "status": "error",
        "protocol_version": 1,
        "error": body,
    })
}
