//! Envelope di errore e proiezione della categoria in exit code.
//!
//! Il contratto verso chi automatizza: un solo documento JSON su stdout, con
//! i quattro assi espliciti, e un numero che gli script possono leggere senza
//! parsare JSON. Vive in un modulo proprio perche' e' l'unica superficie che
//! **ogni** fallimento attraversa, qualunque comando l'abbia prodotto.

use std::error::Error;
use std::io::Write;

use plenora_core::{ErrorCategory, ErrorPhase, PlenoraError, RemoteEffect, RetryDisposition};
use plenora_engine::geo_transport::transport::ArrowTransportError;

/// Cancellazione cooperativa (128 + SIGINT).
pub const EXIT_CANCELLED: i32 = 130;

/// Difetto interno (convenzione `sysexits`: `EX_SOFTWARE`).
pub const EXIT_INTERNO: i32 = 70;

/// Envelope di errore §9 su **stdout**, con `stderr` lasciato vuoto.
///
/// E' la convenzione di `plenora-database-tools`, e due componenti della
/// stessa famiglia non possono dividersi su dove cercare un errore. La
/// rottura per chi parsa stderr e' registrata in `docs/release.md`.
pub fn emit_error_envelope(
    mut stdout: impl Write,
    envelope: &serde_json::Value,
) -> std::io::Result<()> {
    writeln!(stdout, "{envelope}")
}

/// Exit code stabile derivato dalla CATEGORIA dell'envelope.
///
/// Il codice e' una proiezione grossolana della categoria per gli script;
/// una stringa che non e' una categoria finisce su `70`. Diverge da
/// `plenora-database-tools`, che rende `1` per ogni errore
/// (cli.md#exit-code): chi scrive codice portabile legge `error.category`.
///
/// | codice | significato |
/// |---|---|
/// | 0 | successo |
/// | 2 | piano o configurazione invalidi |
/// | 3 | contratto, schema o capability incompatibili |
/// | 4 | limite di risorsa superato |
/// | 5 | I/O, pubblicazione, rete, autorizzazioni, protocollo, ambiente |
/// | 6 | fallimento di esecuzione di un nodo |
/// | 70 | difetto interno |
/// | 130 | cancellato (128 + SIGINT) |
pub fn error_exit_code(envelope: &serde_json::Value) -> i32 {
    envelope["error"]["category"]
        .as_str()
        .and_then(ErrorCategory::from_stable_name)
        .map_or(EXIT_INTERNO, exit_code_di)
}

/// Exit code di una categoria, deciso **per ciascuna**.
///
/// Il `match` esaustivo su [`ErrorCategory`] obbliga a scegliere l'exit code
/// di ogni categoria nuova; `ogni_categoria_ha_l_exit_code_dichiarato` lo
/// verifica anche in test. Nessuno dei due sorveglia la tabella di
/// [`cli.md`](../../../../docs/cli.md), da rileggere quando si tocca questo
/// `match`. Le classi restano grossolane di proposito: la distinzione
/// precisa e' `error.category`.
#[must_use]
pub const fn exit_code_di(categoria: ErrorCategory) -> i32 {
    match categoria {
        // Piano o configurazione: qualcosa a monte va sistemato prima di
        // riprovare. NON necessariamente cio' che il chiamante ha mandato —
        // una configurazione incoerente si corregge nel dispiegamento o
        // nell'ambiente. Il numero raggruppa, `error.category` distingue.
        ErrorCategory::InvalidPlan | ErrorCategory::InvalidConfiguration => 2,
        // Contratto, schema, capability: i dati o le attese non combaciano.
        ErrorCategory::Schema
        | ErrorCategory::DataMapping
        | ErrorCategory::Crs
        | ErrorCategory::Unsupported => 3,
        // Limite di risorsa DIMOSTRATO.
        ErrorCategory::ResourceLimit => 4,
        // Condizioni operative e d'ambiente. `IsolationUnavailable`: il piano
        // e' valido, e' l'ambiente a mancare. `UnattributedMemoryPressure`:
        // 4 o 70 attribuirebbero una causa senza prova.
        ErrorCategory::Io
        | ErrorCategory::NotFound
        | ErrorCategory::Conflict
        | ErrorCategory::Protocol
        | ErrorCategory::Authentication
        | ErrorCategory::Authorization
        | ErrorCategory::Timeout
        | ErrorCategory::Transient
        | ErrorCategory::IsolationUnavailable
        | ErrorCategory::UnattributedMemoryPressure => 5,
        // Fallimento di un nodo del DAG.
        ErrorCategory::Execution => 6,
        ErrorCategory::Internal => EXIT_INTERNO,
        ErrorCategory::Cancelled => EXIT_CANCELLED,
    }
}

/// Emette l'envelope su stdout e rende l'exit code del processo.
///
/// Se l'envelope non esce, il chiamante non vede nessun documento: l'exit
/// code della categoria dichiarerebbe un errore che nessuno puo' leggere, e
/// si rende [`EXIT_INTERNO`]. E' un limite dichiarato della garanzia
/// dell'envelope (errori-e-limiti.md#envelope-e-canali).
pub fn emetti_e_codice(envelope: &serde_json::Value) -> i32 {
    let codice = error_exit_code(envelope);
    if emit_error_envelope(std::io::stdout().lock(), envelope).is_err() {
        return EXIT_INTERNO;
    }
    codice
}

/// Envelope d'errore a quattro assi (R9.1, `protocol_version` 1).
///
/// Una riga JSON su stdout con categoria, fase, effetto remoto e
/// disposizione di retry espliciti, mai dedotti dal messaggio (R9.2).
/// `retry` e' nella forma taggata condivisa (conformance/components.json);
/// `context`, per gli errori di un'esecuzione DAG, riporta nodo, operazione
/// ed `execution_id`. I nomi vengono da [`ErrorCategory`], [`ErrorPhase`] e
/// [`RemoteEffect`]. Errori non `PlenoraError`: parametro del trasporto Arrow
/// -> `invalid_plan`/`validate`/`none`/`never`; I/O nudo ->
/// `io`/`read`/`none`/`safe`; parse JSON del piano ->
/// `data_mapping`/`validate`/`none`/`never`; altro ->
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
    // Gli assi restano TIPIZZATI fino all'ultimo passo, anche sul ramo di
    // ripiego. Scrivendo qui i nomi canonici a mano — `"invalid_plan"`,
    // `"io"`, `"data_mapping"`, `"internal"` — per i quattro casi
    // non-`PlenoraError`, una rinomina di categoria li lascerebbe indietro
    // senza che nulla se ne accorga: e' la stessa classe di difetto di una
    // proiezione su exit code che confronta stringhe.
    let (category, phase, remote_effect, disposition) = plenora_error.map_or_else(
        || {
            if public_transport_parameter_error {
                (
                    ErrorCategory::InvalidPlan,
                    ErrorPhase::Validate,
                    RemoteEffect::None,
                    RetryDisposition::Never,
                )
            } else if error.downcast_ref::<std::io::Error>().is_some() {
                (
                    ErrorCategory::Io,
                    ErrorPhase::Read,
                    RemoteEffect::None,
                    RetryDisposition::Safe,
                )
            } else if error.downcast_ref::<serde_json::Error>().is_some() {
                (
                    ErrorCategory::DataMapping,
                    ErrorPhase::Validate,
                    RemoteEffect::None,
                    RetryDisposition::Never,
                )
            } else {
                (
                    ErrorCategory::Internal,
                    ErrorPhase::Validate,
                    RemoteEffect::None,
                    RetryDisposition::Never,
                )
            }
        },
        |plenora| {
            (
                plenora.category(),
                plenora.phase(),
                plenora.remote_effect(),
                plenora.retry_disposition(),
            )
        },
    );
    let (category, phase, remote_effect) =
        (category.as_str(), phase.as_str(), remote_effect.as_str());
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
