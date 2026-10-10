//! Capability Discovery 2.0 della CLI e dell'SDK Python, mappa degli
//! export Rust e mappa dei simboli Python, tutte dalla tabella
//! [`crate::operazioni::OPERAZIONI`].

use serde_json::{json, Value};

use crate::artefatti::CONTRATTO_RICHIESTA;
use crate::catalogo::FORMATO_PIANO;
use crate::operazioni::{
    OperazionePubblica, ARROW_FILE, ARROW_STREAM, CONTRATTO_ATTRIBUTI, OPERAZIONI, REGISTRO_KERNEL,
};
use crate::{
    ARTEFATTO_CLI, ARTEFATTO_PYTHON, ARTEFATTO_RUST, COMPONENTE, IMPORT_PYTHON, PROTOCOLLO_CLI,
    VERSIONE_COMPONENTE,
};

/// Contratto del documento delle capacità.
pub const CONTRATTO_CAPACITA: &str = "plenora-capabilities-v2";
/// Contratto della superficie CLI.
pub const CONTRATTO_CLI: &str = "plenora-cli-v2";
/// Contratto della superficie Python (`specs/sdk/PYTHON-SDK-1.0.md`).
pub const CONTRATTO_PYTHON: &str = "plenora-python-sdk-v1";
/// Versione del contratto della superficie Python.
pub const PROTOCOLLO_PYTHON: u32 = 1;
/// Contratto della mappa degli export Rust (component-owned).
pub const CONTRATTO_MAPPA_RUST: &str = "plenora-data-rust-surface-v1";
/// Contratto dei binding normativi di superficie, di cui
/// [`mappa_python`] è la sezione di questo componente.
pub const CONTRATTO_BINDING: &str = "plenora-surface-bindings-v1";
/// Simboli di scoperta dell'SDK Python: versione e capacità.
pub const SCOPERTA_PYTHON: &[&str] = &["plenora_data.version", "plenora_data.capabilities"];

/// Contratto di interscambio degli artefatti Arrow di `data.run` 3
/// (attributo `artifact_interchange_contracts` del catalogo).
const CONTRATTO_INTERSCAMBIO: &str = "plenora-arrow-interchange-v1";

/// Una superficie che risponde con un documento delle capacità.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Superficie {
    /// Il binario `plenora-data`.
    Cli,
    /// Il pacchetto Python `plenora-data` (`plenora_data`).
    Python,
}

impl Superficie {
    /// Il nome dello schema `capabilities-v2` (`kind`, `surfaces`).
    #[must_use]
    pub const fn come_testo(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Python => "python_sdk",
        }
    }
}

/// Attributi tipizzati (`plenora-data-capability-attributes-v1`): ciò che
/// un consumatore usa per scegliere l'operazione e che i campi comuni non
/// dicono. Vuoti se l'operazione non ha nulla da aggiungere.
///
/// Un'operazione su artefatti (`data.run` 3, la cui richiesta è
/// [`CONTRATTO_RICHIESTA`]) legge sorgenti e pubblica destinazioni per
/// riferimento: i suoi tipi stanno sotto `source`/`sink`, con i tipi Arrow
/// degli artefatti e il contratto di interscambio, come nel catalogo
/// pubblico; le altre hanno `input`/`output`. `data.run` 3 non entra nei
/// documenti delle capacità della CLI e dell'SDK, ma la forma è la stessa
/// del catalogo per tutte le operazioni (prova in fondo al file).
fn attributi(operazione: &OperazionePubblica) -> Option<Value> {
    let su_artefatti = operazione.ingresso == CONTRATTO_RICHIESTA;
    let (lato_ingresso, lato_uscita) = if su_artefatti {
        ("source", "sink")
    } else {
        ("input", "output")
    };
    let mut campi = serde_json::Map::new();
    if operazione.usa_registro {
        campi.insert("kernel_registry".to_owned(), json!(REGISTRO_KERNEL));
    }
    if operazione.usa_piano {
        campi.insert("plan_contract".to_owned(), json!(FORMATO_PIANO));
    }
    if su_artefatti {
        campi.insert(
            "artifact_content_types".to_owned(),
            json!({
                lato_ingresso: [ARROW_STREAM, ARROW_FILE],
                lato_uscita: [ARROW_STREAM, ARROW_FILE],
            }),
        );
        campi.insert(
            "artifact_interchange_contracts".to_owned(),
            json!([CONTRATTO_INTERSCAMBIO]),
        );
    }
    if !operazione.estensioni_ingresso.is_empty() || !operazione.estensioni_uscita.is_empty() {
        campi.insert(
            "extension_content_types".to_owned(),
            json!({
                lato_ingresso: operazione.estensioni_ingresso,
                lato_uscita: operazione.estensioni_uscita,
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

fn descrittore(operazione: &OperazionePubblica, superficie: Superficie) -> Value {
    let mut voce = json!({
        "id": operazione.id,
        "version": operazione.versione,
        "status": "available",
        "surfaces": [superficie.come_testo()],
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

/// Il documento `capabilities-v2` della superficie che risponde.
///
/// Una sola interfaccia, quella dell'artefatto che lo produce (CAP-002,
/// CAP-003), e le stesse operazioni con gli stessi contratti e controlli.
#[must_use]
pub fn documento_della(superficie: Superficie) -> Value {
    let interfaccia = match superficie {
        Superficie::Cli => json!({
            "kind": superficie.come_testo(),
            "contract": CONTRATTO_CLI,
            "version": PROTOCOLLO_CLI,
            "artifact": ARTEFATTO_CLI,
        }),
        Superficie::Python => json!({
            "kind": superficie.come_testo(),
            "contract": CONTRATTO_PYTHON,
            "version": PROTOCOLLO_PYTHON,
            "artifact": ARTEFATTO_PYTHON,
        }),
    };
    json!({
        "schema_version": 2,
        "component": COMPONENTE,
        "component_version": VERSIONE_COMPONENTE,
        "interfaces": [interfaccia],
        "operations": OPERAZIONI
            .iter()
            .filter(|operazione| !operazione.solo_rust)
            .map(|operazione| descrittore(operazione, superficie))
            .collect::<Vec<_>>(),
    })
}

/// Il documento `capabilities-v2` del binario `plenora-data`: l'unica
/// interfaccia è la CLI che risponde (CAP-002, CAP-003).
#[must_use]
pub fn documento() -> Value {
    documento_della(Superficie::Cli)
}

/// La mappa versionata delle operazioni verso gli export Rust pubblici
/// (Surface Bindings 1.0, sezione 2), dalla stessa tabella delle capacità:
/// un'operazione nuova non si pubblica senza il suo export.
///
/// Comprende `data.run` 3, che sta solo sulla superficie Rust.
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

/// La sezione di questo componente in `bindings/python-sdk-v1.json`.
///
/// È un elemento di `components` dello schema `surface-bindings-v1` di
/// `plenora-contracts`, dalla stessa tabella: la sezione di questo
/// componente in `bindings/python-sdk-v1.json`, e ciò che il pacchetto
/// Python espone.
///
/// `requirement` è `required` per tutte e quattro: lo dice il catalogo
/// pubblico (`catalogs/data-tools-v2.json`) per ogni operazione.
#[must_use]
pub fn mappa_python() -> Value {
    json!({
        "component": COMPONENTE,
        "artifact": format!("{ARTEFATTO_PYTHON} / {IMPORT_PYTHON}"),
        "discovery": SCOPERTA_PYTHON,
        "bindings": OPERAZIONI
            .iter()
            .filter(|operazione| !operazione.solo_rust)
            .map(|operazione| json!({
                "operation": operazione.id,
                "version": operazione.versione,
                "requirement": "required",
                "entrypoints": operazione.export_python,
            }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::attributi;
    use crate::operazioni::{CONTRATTO_ATTRIBUTI, OPERAZIONI};

    /// Gli attributi di ogni operazione della tabella, `data.run` 3
    /// compresa (che i documenti delle capacità non elencano e la prova di
    /// `tests/scoperta.rs` quindi non vede), sono quelli del catalogo
    /// pubblico, più il solo contratto degli attributi.
    #[test]
    fn gli_attributi_di_ogni_operazione_sono_quelli_del_catalogo() {
        let catalogo: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/contratti/data-tools-v2.json"
        ))
        .expect("catalogo pubblico");
        let pubbliche = catalogo["operations"].as_array().expect("operations");
        assert_eq!(pubbliche.len(), OPERAZIONI.len());
        for operazione in OPERAZIONI {
            let pubblica = pubbliche
                .iter()
                .find(|voce| voce["id"] == operazione.id && voce["version"] == operazione.versione)
                .expect("operazione nel catalogo");
            let mut nostri = attributi(operazione).unwrap_or(Value::Null);
            if let Some(campi) = nostri.as_object_mut() {
                assert_eq!(
                    campi.remove("contract"),
                    Some(Value::from(CONTRATTO_ATTRIBUTI))
                );
            }
            let mut attesi = pubblica["attributes"].clone();
            if let Some(campi) = attesi.as_object_mut() {
                // `registry` è il percorso del file nei contratti, non un
                // attributo dell'artefatto.
                campi.remove("registry");
            }
            assert_eq!(nostri, attesi, "{} {}", operazione.id, operazione.versione);
        }
    }
}
