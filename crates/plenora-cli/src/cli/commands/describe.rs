//! `describe` (e il suo alias `inspect-dataset`): che cosa un input dichiara
//! di se'.
//!
//! Legge i soli metadati dello schema — campi, colonna geometrica, CRS,
//! encoding, tipi dichiarati, fingerprint del contratto — e non esegue nulla.
//! E' la risposta alla domanda «questo file lo posso dare in pasto a quel
//! piano?» senza doverlo scoprire eseguendolo.

use std::error::Error;
use std::path::Path;

use plenora_core::contract::{ContractCrs, DataContract, GeometryEncoding, GeometryTypesProperty};
use plenora_core::PlenoraError;
use plenora_engine::planner;

use crate::cli::contract_discovery::discover_input_contract;
use crate::cli::rendering::contract_json;
use crate::{contract, value_after, OutputFormat};

/// Descrizione completa di un input: cio' che serve per SCRIVERE un piano
/// contro quel file, e il fingerprint con cui il piano sara' poi verificato.
///
/// I campi non geometrici non hanno un `field_id` nel contratto — l'identita'
/// interna e' assegnata dal grafo, non dall'input — e non se ne inventa uno.
pub fn describe_json(
    path: &Path,
    contract: &DataContract,
) -> Result<serde_json::Value, PlenoraError> {
    let fingerprint = planner::contract_fingerprint(contract)?;
    let geometries: Vec<serde_json::Value> = contract
        .geometries
        .iter()
        .map(|geometry| {
            serde_json::json!({
                "name": geometry.name,
                "field_id": geometry.field_id.0,
                "nullable": geometry.nullable,
                "dimensions": geometry.dimensions.as_str(),
                "encoding": geometry.encoding.map(GeometryEncoding::as_str),
                "crs_resolution": geometry.crs.resolution().as_str(),
                "crs": match &geometry.crs {
                    ContractCrs::Resolved(crs) | ContractCrs::ResolvedByDecision(crs) =>
                        serde_json::Value::String(crs.definition().to_owned()),
                    ContractCrs::DeclaredUnresolved { .. } | ContractCrs::Missing =>
                        serde_json::Value::Null,
                },
                "types_declaration": geometry
                    .types
                    .value()
                    .map(|types| types.declaration().as_str()),
                "types": geometry
                    .types
                    .value()
                    .map(GeometryTypesProperty::to_canonical_list),
                "active": contract
                    .active_geometry_column()
                    .is_some_and(|active| active.name == geometry.name),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "status": "ok",
        "protocol_version": 1,
        "input": path.display().to_string(),
        "contract_fingerprint": fingerprint.to_hex(),
        "fields": contract_json(contract)["fields"],
        "geometries": geometries,
    }))
}

/// `describe`: cosa contiene un input, senza eseguire nulla.
///
/// E' il primo comando da invocare per scrivere un piano: senza, i nomi delle
/// colonne, il CRS e l'encoding si scoprono solo facendo fallire un `run`.
/// L'input passa dal confine IPC come in esecuzione — framing pre-validato,
/// barriera anti-panico — quindi cio' che `describe` accetta e' cio' che `run`
/// accettera'.
pub fn describe_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    let input = value_after(args, "--input")?;
    let contract = discover_input_contract(Path::new(&input))?;
    let documento = describe_json(Path::new(&input), &contract)?;
    match OutputFormat::active() {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&documento)?),
        OutputFormat::Markdown => print!("{}", describe_markdown(&documento)),
    }
    Ok(())
}

/// Resa leggibile di [`describe_json`]. Stesso contenuto, altra forma: un
/// campo che compare nel JSON e non qui sarebbe una descrizione parziale
/// travestita da descrizione.
pub fn describe_markdown(documento: &serde_json::Value) -> String {
    use std::fmt::Write as _;
    let mut testo = String::new();
    let vuoto = Vec::new();
    // `write!` su `String` non fallisce; l'esito si ignora esplicitamente
    // invece di propagarlo per un canale che non ha errori.
    let _ = writeln!(
        testo,
        "# {}\n",
        documento["input"].as_str().unwrap_or("(input)")
    );
    let _ = writeln!(
        testo,
        "Fingerprint del contratto: `{}`\n",
        documento["contract_fingerprint"].as_str().unwrap_or("?")
    );
    let _ = writeln!(testo, "## Campi\n");
    let _ = writeln!(testo, "| nome | tipo | nullable |");
    let _ = writeln!(testo, "|---|---|---|");
    for campo in documento["fields"].as_array().unwrap_or(&vuoto) {
        let _ = writeln!(
            testo,
            "| `{}` | {} | {} |",
            campo["name"].as_str().unwrap_or("?"),
            campo["data_type"].as_str().unwrap_or("?"),
            campo["nullable"]
        );
    }
    let geometrie = documento["geometries"].as_array().unwrap_or(&vuoto);
    if geometrie.is_empty() {
        let _ = writeln!(testo, "\nNessuna colonna geometrica.");
        return testo;
    }
    let _ = writeln!(testo, "\n## Geometrie\n");
    for geometria in geometrie {
        let _ = writeln!(
            testo,
            "- `{}` (field_id {}){}",
            geometria["name"].as_str().unwrap_or("?"),
            geometria["field_id"],
            if geometria["active"] == serde_json::Value::Bool(true) {
                " — attiva"
            } else {
                ""
            }
        );
        let _ = writeln!(
            testo,
            "  - dimensioni: {} · encoding: {} · CRS: {} ({})",
            geometria["dimensions"].as_str().unwrap_or("?"),
            geometria["encoding"].as_str().unwrap_or("non dichiarato"),
            geometria["crs"].as_str().unwrap_or("assente"),
            geometria["crs_resolution"].as_str().unwrap_or("?")
        );
        let _ = writeln!(
            testo,
            "  - tipi: {} ({})",
            geometria["types"].as_str().unwrap_or("non dichiarati"),
            geometria["types_declaration"]
                .as_str()
                .unwrap_or("non dichiarata")
        );
    }
    testo
}
