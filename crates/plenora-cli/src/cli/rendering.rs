//! Rendering: come i documenti della CLI diventano JSON.
//!
//! Sono le forme che l'utente e i suoi script leggono davvero — descrittore
//! di un'operazione, versione, backend compilati, contratto di un input — e
//! quindi una superficie pubblica quanto i nomi dei comandi. Stanno insieme
//! perche' cambiarne una senza vedere le altre e' il modo in cui due
//! documenti della stessa CLI finiscono per descrivere lo stesso concetto
//! con nomi diversi.
//!
//! `tests/oracolo_superficie_cli.snap` fissa `capabilities` e `catalog` byte
//! per byte: qualunque modifica qui si vede li'.

use plenora_core::catalog::{OperationDescriptor, CATALOG};
use plenora_core::contract::{ContractCrs, DataContract};

/// Digest esadecimale minuscolo, senza primitive di panic (gate R6).
///
/// La formattazione su `String` non puo' fallire, ma `write!` restituisce
/// comunque un `Result` che andrebbe scartato con `expect`. La tabella dei
/// nibble e' indicizzata da un valore provabilmente in `0..16` (shift e
/// maschera su `u8`): esatta per costruzione, nessun `Result` da gestire.
pub fn hex_digest(digest: &[u8; 32]) -> String {
    const NIBBLE: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    let mut output = String::with_capacity(64);
    for &byte in digest {
        output.push(NIBBLE[usize::from(byte >> 4)]);
        output.push(NIBBLE[usize::from(byte & 0x0f)]);
    }
    output
}

pub fn descriptor_json(descriptor: &OperationDescriptor) -> serde_json::Value {
    serde_json::json!({
        "id": descriptor.id,
        "family": format!("{:?}", descriptor.family),
        "origin": format!("{:?}", descriptor.origin),
        "arity": format!("{:?}", descriptor.arity),
        "execution_class": format!("{:?}", descriptor.execution_class),
        "cancellation_behavior": format!("{:?}", descriptor.cancellation_behavior),
        "result_shape": descriptor.result_shape.map(|shape| format!("{shape:?}")),
        "crs_requirement": descriptor.crs_requirement.map(|req| format!("{req:?}")),
        "required_capabilities": descriptor.required_capabilities,
        "determinism": format!("{:?}", descriptor.determinism),
        "maturity": format!("{:?}", descriptor.maturity),
        "semantic_version": descriptor.semantic_version,
        "config_schema_version": descriptor.config_schema_version,
        "contract_analysis_version": descriptor.contract_analysis_version,
        "kernel_version": descriptor.kernel_version,
    })
}

/// Identita' del binario in forma leggibile da un programma: versione del
/// componente, versione Arrow, backend compilati.
///
/// I backend derivano dalle feature con cui QUESTO binario e' stato
/// compilato, non da una lista scritta a mano: e' l'unica risposta che non
/// puo' mentire.
pub fn version_json() -> serde_json::Value {
    serde_json::json!({
        "status": "ok",
        "protocol_version": 1,
        "component": "plenora-data-tools",
        "component_version": env!("CARGO_PKG_VERSION"),
        "arrow_version": plenora_core::capabilities::component_capabilities().arrow_version,
        "backends": backends_compilati(),
        "operations": CATALOG.len(),
    })
}

/// Backend geografici effettivamente compilati in questo binario.
pub fn backends_compilati() -> Vec<&'static str> {
    let mut backends = Vec::new();
    if cfg!(feature = "geos-backend") {
        backends.push("geos");
    }
    if cfg!(feature = "proj-backend") {
        backends.push("proj");
    }
    backends
}

/// Sintesi JSON di un contratto d'arco: campi dello schema e geometria attiva.
pub fn contract_json(contract: &DataContract) -> serde_json::Value {
    let fields: Vec<serde_json::Value> = contract
        .schema
        .fields()
        .iter()
        .map(|field| {
            serde_json::json!({
                "name": field.name(),
                "data_type": format!("{:?}", field.data_type()),
                "nullable": field.is_nullable(),
            })
        })
        .collect();
    let geometry = contract.active_geometry_column().map(|geometry| {
        match &geometry.crs {
            ContractCrs::Resolved(crs) | ContractCrs::ResolvedByDecision(crs) => {
                serde_json::json!({
                    "name": geometry.name,
                    "crs": crs.definition(),
                    "crs_kind": format!("{:?}", crs.kind()),
                    "crs_resolution": geometry.crs.resolution().as_str(),
                })
            }
            // Nessun CRS risolto da dichiarare: lo stato (canonico R2.2)
            // distingue `missing` da `declared_unresolved` (R4.1).
            ContractCrs::DeclaredUnresolved { .. } | ContractCrs::Missing => serde_json::json!({
                "name": geometry.name,
                "crs": serde_json::Value::Null,
                "crs_kind": serde_json::Value::Null,
                "crs_resolution": geometry.crs.resolution().as_str(),
            }),
        }
    });
    serde_json::json!({
        "fields": fields,
        "geometry": geometry,
    })
}
