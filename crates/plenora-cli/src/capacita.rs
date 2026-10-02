//! Capability Discovery 2.0 della CLI e mappa degli export Rust, entrambe
//! dalla tabella [`crate::operazioni::OPERAZIONI`].

use serde_json::{json, Value};

use crate::operazioni::{OperazionePubblica, CONTRATTO_ATTRIBUTI, OPERAZIONI, REGISTRO_KERNEL};
use crate::{ARTEFATTO_CLI, ARTEFATTO_RUST, COMPONENTE, PROTOCOLLO_CLI, VERSIONE_COMPONENTE};

/// Contratto del documento delle capacità.
pub const CONTRATTO_CAPACITA: &str = "plenora-capabilities-v2";
/// Contratto della superficie CLI.
pub const CONTRATTO_CLI: &str = "plenora-cli-v2";
/// Contratto della mappa degli export Rust (component-owned).
pub const CONTRATTO_MAPPA_RUST: &str = "plenora-data-rust-surface-v1";

/// Attributi tipizzati (`plenora-data-capability-attributes-v1`): ciò che
/// un consumatore usa per scegliere l'operazione e che i campi comuni non
/// dicono. Vuoti se l'operazione non ha nulla da aggiungere.
fn attributi(operazione: &OperazionePubblica) -> Option<Value> {
    let mut campi = serde_json::Map::new();
    if operazione.usa_registro {
        campi.insert("kernel_registry".to_owned(), json!(REGISTRO_KERNEL));
    }
    if !operazione.estensioni_ingresso.is_empty() || !operazione.estensioni_uscita.is_empty() {
        campi.insert(
            "extension_content_types".to_owned(),
            json!({
                "input": operazione.estensioni_ingresso,
                "output": operazione.estensioni_uscita,
            }),
        );
    }
    if operazione.materializzazione_limitata {
        campi.insert("bounded_materialization".to_owned(), json!(true));
    }
    if campi.is_empty() {
        return None;
    }
    campi.insert("contract".to_owned(), json!(CONTRATTO_ATTRIBUTI));
    Some(Value::Object(campi))
}

fn descrittore(operazione: &OperazionePubblica) -> Value {
    let mut voce = json!({
        "id": operazione.id,
        "version": operazione.versione,
        "status": "available",
        "surfaces": ["cli"],
        "input": {
            "contract": operazione.ingresso,
            "content_types": operazione.tipi_ingresso,
        },
        "output": {
            "contract": operazione.uscita,
            "content_types": operazione.tipi_uscita,
        },
        "side_effect": operazione.effetto.come_testo(),
        "controls": {
            "cancellation": operazione.annullamento,
            "deadline": operazione.scadenza,
            "idempotency_key": false,
        },
    });
    if let (Some(attributi), Value::Object(campi)) = (attributi(operazione), &mut voce) {
        campi.insert("attributes".to_owned(), attributi);
    }
    voce
}

/// Il documento `capabilities-v2` del binario `plenora-data`: l'unica
/// interfaccia è la CLI che risponde (CAP-002, CAP-003).
#[must_use]
pub fn documento() -> Value {
    json!({
        "schema_version": 2,
        "component": COMPONENTE,
        "component_version": VERSIONE_COMPONENTE,
        "interfaces": [{
            "kind": "cli",
            "contract": CONTRATTO_CLI,
            "version": PROTOCOLLO_CLI,
            "artifact": ARTEFATTO_CLI,
        }],
        "operations": OPERAZIONI.iter().map(descrittore).collect::<Vec<_>>(),
    })
}

/// La mappa versionata delle operazioni verso gli export Rust pubblici
/// (Surface Bindings 1.0, sezione 2), dalla stessa tabella delle capacità:
/// un'operazione nuova non si pubblica senza il suo export.
#[must_use]
pub fn mappa_rust() -> Value {
    json!({
        "contract": CONTRATTO_MAPPA_RUST,
        "schema_version": 1,
        "component": COMPONENTE,
        "artifact": ARTEFATTO_RUST,
        "artifact_version": VERSIONE_COMPONENTE,
        "bindings": OPERAZIONI
            .iter()
            .map(|operazione| json!({
                "operation": operazione.id,
                "version": operazione.versione,
                "entrypoints": operazione.export_rust,
            }))
            .collect::<Vec<_>>(),
    })
}
