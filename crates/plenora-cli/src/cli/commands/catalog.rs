//! `capabilities` e `catalog`: i due documenti dichiarativi.
//!
//! Rispondono alla stessa domanda da due lati — che cosa questo binario sa
//! fare — e non eseguono nulla. `capabilities` descrive la build (versione,
//! backend compilati, Arrow); `catalog` descrive le operazioni.
//!
//! Entrambi sono fissati byte per byte da `tests/oracolo_superficie_cli.snap`:
//! 44 KB il primo, 70 KB il secondo.

use std::error::Error;

use plenora_core::catalog::{Family, OperationDescriptor, CATALOG};

use crate::cli::rendering::{backends_compilati, descriptor_json};
use crate::{contract, optional_value_after, OutputFormat};

/// `capabilities`: il documento dichiarativo di `plenora-core` piu'
/// l'identita' di questo binario (versione e backend), che il documento non
/// puo' conoscere.
pub fn capabilities_command() -> Result<(), Box<dyn Error>> {
    // ICD §10 R10.2: capability dichiarative interrogabili prima
    // dell'esecuzione, in forma leggibile da un programma.
    let documento = plenora_core::capabilities::component_capabilities();
    let mut valore = serde_json::to_value(&documento)?;
    if let Some(oggetto) = valore.as_object_mut() {
        oggetto.insert(
            "component_version".to_owned(),
            serde_json::Value::String(env!("CARGO_PKG_VERSION").to_owned()),
        );
        oggetto.insert(
            "backends".to_owned(),
            serde_json::json!(backends_compilati()),
        );
    }
    match OutputFormat::active() {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&valore)?),
        OutputFormat::Markdown => {
            println!("# Capability di plenora-data-tools\n");
            println!("| | |");
            println!("|---|---|");
            println!("| versione | {} |", env!("CARGO_PKG_VERSION"));
            println!("| Arrow | {} |", documento.arrow_version);
            println!(
                "| backend | {} |",
                if backends_compilati().is_empty() {
                    "nessuno".to_owned()
                } else {
                    backends_compilati().join(", ")
                }
            );
            println!("| operazioni a catalogo | {} |", CATALOG.len());
        }
    }
    Ok(())
}

pub fn catalog_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    let family = optional_value_after(args, "--family")?;
    let family = family
        .as_deref()
        .map(|value| {
            value.to_str().and_then(|name| match name {
                "table" => Some(Family::Table),
                "geo" => Some(Family::Geo),
                _ => None,
            })
        })
        .map(|parsed| {
            parsed
                .ok_or_else(|| contract("famiglia sconosciuta: attesa `table` o `geo`".to_owned()))
        })
        .transpose()?;
    let descrittori: Vec<&OperationDescriptor> = CATALOG
        .iter()
        .filter(|descriptor| family.is_none_or(|wanted| descriptor.family == wanted))
        .collect();
    match OutputFormat::active() {
        OutputFormat::Json => {
            let entries: Vec<serde_json::Value> =
                descrittori.iter().copied().map(descriptor_json).collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
        }
        OutputFormat::Markdown => {
            println!("| operazione | famiglia | arieta' | maturita' |");
            println!("|---|---|---|---|");
            for descriptor in descrittori {
                println!(
                    "| `{}` | {:?} | {:?} | {:?} |",
                    descriptor.id, descriptor.family, descriptor.arity, descriptor.maturity
                );
            }
        }
    }
    Ok(())
}
