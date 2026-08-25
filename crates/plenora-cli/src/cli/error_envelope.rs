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
/// # Perche' un `match` su un tipo e non su una stringa
///
/// La versione precedente confrontava il nome canonico e mandava tutto il
/// resto su `EXIT_INTERNO`. Funzionava, e nascondeva due cose:
///
/// - una categoria **nuova** non fermava nulla. Finiva in silenzio su 70,
///   cioe' dichiarava un difetto interno di una condizione che non lo era, e
///   nessun test poteva accorgersene perche' l'elenco atteso era anch'esso
///   scritto a mano;
/// - la tabella di [`cli.md`](../../../../docs/cli.md) poteva divergere dal
///   codice, e lo aveva fatto: `protocol` era mappato su 5 dal 2026 e non
///   compariva nella riga del 5.
///
/// Con un `match` esaustivo su [`ErrorCategory`] una categoria nuova **non
/// compila** finche' qualcuno non ne decide l'exit code. Il ripiego su 70
/// resta dove serve davvero: in [`error_exit_code`], per un envelope che
/// porta una stringa che non e' una categoria — un JSON di un'altra versione,
/// o corrotto.
///
/// # Gli exit code non si moltiplicano
///
/// Sono classi **grossolane** di proposito: la distinzione precisa vive in
/// `error.category` dell'envelope, che e' machine-readable. Aggiungere un
/// numero per ogni condizione nuova allargherebbe il contratto verso chi
/// invoca l'eseguibile senza dirgli nulla che non possa gia' leggere.
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
        // Condizioni operative e d'ambiente.
        //
        // `IsolationUnavailable` sta qui e non su 2: il piano e' valido, ed
        // e' l'ambiente a non offrire l'isolamento.
        //
        // `UnattributedMemoryPressure` sta qui e non su 4 ne' su 70: dire 4
        // attribuirebbe il superamento al budget del dominio senza prova,
        // dire 70 dichiarerebbe un difetto interno senza prova. Cinque dice
        // «condizione operativa», che e' quanto sappiamo.
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
/// `internal`/`validate`/`none`/`never`. I nomi non sono piu' scritti a mano
/// in nessuno dei quattro rami: vengono da [`ErrorCategory`], [`ErrorPhase`] e
/// [`RemoteEffect`], quindi seguono una rinomina invece di sopravviverle.
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
    // ripiego. Prima i quattro casi non-`PlenoraError` scrivevano i nomi
    // canonici a mano — `"invalid_plan"`, `"io"`, `"data_mapping"`,
    // `"internal"` — e una rinomina di categoria li avrebbe lasciati
    // indietro senza che nulla se ne accorgesse: sono la stessa classe di
    // difetto della proiezione su exit code che confrontava stringhe.
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
