//! La mappa operazione → export Rust (Surface Bindings 1.0, sezione 2),
//! verificata da un crate consumatore che usa solo gli export documentati.
//!
//! Le funzioni `_compila_*` non girano: la loro compilazione prova che un
//! crate esterno può chiamare ogni export della mappa con i tipi pubblici.

use std::collections::BTreeSet;
use std::path::Path;

use plenora_cli::api;
use plenora_cli::capacita::{documento, mappa_rust};
use plenora_core::arrow::array::RecordBatch;
use plenora_core::Result;
use plenora_io::{FileIngresso, FileUscita, Ingresso, OpzioniScrittura};
use plenora_pipeline::{Interruzione, Pipeline};
use serde_json::Value;

/// Gli export documentati, scritti qui a mano: la mappa deve dire
/// esattamente questi.
const EXPORT_DOCUMENTATI: &[&str] = &[
    "plenora_cli::api::catalogo",
    "plenora_cli::api::descrivi",
    "plenora_cli::api::descrivi_tabella",
    "plenora_cli::api::valida",
    "plenora_cli::api::valida_ingressi",
    "plenora_cli::api::esegui",
    "plenora_cli::api::esegui_ingressi",
    "plenora_cli::api::esegui_in_memoria",
];

#[test]
fn la_mappa_copre_esattamente_le_operazioni_pubblicate() {
    let mappa = mappa_rust();
    assert_eq!(mappa["contract"], "plenora-data-rust-surface-v1");
    assert_eq!(mappa["artifact"], "plenora-cli");
    assert_eq!(mappa["artifact_version"], env!("CARGO_PKG_VERSION"));
    let legami = mappa["bindings"].as_array().expect("bindings");
    let mappate: BTreeSet<(String, u64)> = legami
        .iter()
        .map(|legame| {
            (
                legame["operation"].as_str().expect("operation").to_owned(),
                legame["version"].as_u64().expect("version"),
            )
        })
        .collect();
    let pubblicate: BTreeSet<(String, u64)> = documento()["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|operazione| {
            (
                operazione["id"].as_str().expect("id").to_owned(),
                operazione["version"].as_u64().expect("version"),
            )
        })
        .collect();
    assert_eq!(mappate, pubblicate);
    let export: BTreeSet<&str> = legami
        .iter()
        .flat_map(|legame| legame["entrypoints"].as_array().expect("entrypoints"))
        .map(|export| export.as_str().expect("export"))
        .collect();
    assert_eq!(export, EXPORT_DOCUMENTATI.iter().copied().collect());
}

#[test]
fn catalogo_dalla_superficie_rust_e_quello_della_cli() {
    let uscita = std::process::Command::new(env!("CARGO_BIN_EXE_plenora-data"))
        .args(["catalog", "--format", "json"])
        .output()
        .expect("avvio");
    let documento: Value = serde_json::from_slice(&uscita.stdout).expect("JSON");
    assert_eq!(documento["result"], api::catalogo());
}

#[allow(dead_code)]
fn _compila_catalogo() -> Value {
    api::catalogo()
}

#[allow(dead_code)]
fn _compila_descrivi(ingresso: &Path, interruzione: &Interruzione) -> Result<Value> {
    api::descrivi(ingresso, interruzione)
}

#[allow(dead_code)]
fn _compila_valida(
    piano: &Pipeline,
    ingressi: &[FileIngresso],
    interruzione: &Interruzione,
) -> Result<Value> {
    api::valida(piano, ingressi, interruzione)
}

#[allow(dead_code)]
fn _compila_esegui(
    piano: &Pipeline,
    ingressi: &[FileIngresso],
    uscite: &[FileUscita],
    opzioni: OpzioniScrittura,
    interruzione: &Interruzione,
) -> Result<Value> {
    api::esegui(piano, ingressi, uscite, &opzioni, interruzione)
}

#[allow(dead_code)]
fn _compila_descrivi_tabella(tabella: &RecordBatch, interruzione: &Interruzione) -> Result<Value> {
    api::descrivi_tabella(tabella, interruzione)
}

#[allow(dead_code)]
fn _compila_valida_ingressi(
    piano: &Pipeline,
    ingressi: Vec<Ingresso>,
    interruzione: &Interruzione,
) -> Result<Value> {
    api::valida_ingressi(piano, ingressi, interruzione)
}

#[allow(dead_code)]
fn _compila_esegui_ingressi(
    piano: &Pipeline,
    ingressi: Vec<Ingresso>,
    uscite: &[FileUscita],
    opzioni: OpzioniScrittura,
    interruzione: &Interruzione,
) -> Result<Value> {
    api::esegui_ingressi(piano, ingressi, uscite, &opzioni, interruzione)
}

#[allow(dead_code)]
fn _compila_esegui_in_memoria(
    piano: &Pipeline,
    ingressi: Vec<Ingresso>,
    interruzione: &Interruzione,
) -> Result<(Value, Vec<(String, RecordBatch)>)> {
    api::esegui_in_memoria(piano, ingressi, interruzione)
}
