//! Il registro dei kernel (`plenora-data-kernel-catalog-v1`), risultato di
//! `data.catalog`.
//!
//! Deriva per intero da `plenora_core::catalog::CATALOG` e dall'elenco delle
//! operazioni che il runner non esegue
//! (`plenora_pipeline::disponibilita`): nessuna lista scritta a mano.
//!
//! Il documento ha due parti:
//!
//! - `registry`: un documento `plenora-operation-registry-v1` (schema dei
//!   contratti) con i soli kernel che un piano può eseguire in questo
//!   artefatto, `{id, version, family}`;
//! - `kernels`: un descrittore per **ogni** kernel del catalogo, eseguibile
//!   o no (`status`, e `reason` per i non eseguibili), con le proprietà che
//!   il profilo data-tools chiede di esporre (arietà, forma del risultato,
//!   determinismo, backend richiesti, requisito CRS) e le quattro versioni
//!   per componente.
//!
//! `version` è la versione della semantica osservabile
//! (`semantic_version`): il registro comune dei contratti dichiara 1 per
//! tutti, ed è una deviazione dichiarata (docs/cli.md, «CLI `plenora-data`»).

use plenora_core::catalog::{
    Arity, CrsRequirement, DeterminismPolicy, Family, OperationDescriptor, ResultShape, CATALOG,
};
use plenora_pipeline::disponibilita::motivo_non_eseguibile;
use serde_json::{json, Value};

use crate::operazioni::REGISTRO_KERNEL;
use crate::COMPONENTE;

/// Contratto del registro incorporato (schema `operation-registry-v1`).
pub const CONTRATTO_REGISTRO: &str = "plenora-operation-registry-v1";
/// Formato dei piani accettati da `data.validate` e `data.run`.
pub const FORMATO_PIANO: &str = "plenora-data-plan-v1";

const fn famiglia(famiglia: Family) -> &'static str {
    match famiglia {
        Family::Table => "table",
        Family::Geo => "geo",
    }
}

const fn arieta(arieta: Arity) -> &'static str {
    match arieta {
        Arity::Unary => "unary",
        Arity::BinaryOrdered => "binary_ordered",
        Arity::NAry => "n_ary",
    }
}

const fn forma(forma: ResultShape) -> &'static str {
    match forma {
        ResultShape::OneToOne => "one_to_one",
        ResultShape::OneToMany => "one_to_many",
        ResultShape::ManyToOne => "many_to_one",
        ResultShape::Collective => "collective",
        ResultShape::WholeToMany => "whole_to_many",
        ResultShape::FromCoords => "from_coords",
        ResultShape::Diagnostic => "diagnostic",
    }
}

const fn determinismo(politica: DeterminismPolicy) -> &'static str {
    match politica {
        DeterminismPolicy::DefinedOrder => "defined_order",
        DeterminismPolicy::InputOrder => "input_order",
        DeterminismPolicy::StableKeyOrder => "stable_key_order",
        DeterminismPolicy::CanonicalOrder => "canonical_order",
    }
}

const fn requisito_crs(requisito: CrsRequirement) -> &'static str {
    match requisito {
        CrsRequirement::Known => "known",
        CrsRequirement::Projected => "projected",
        CrsRequirement::Geographic => "geographic",
        CrsRequirement::SameProjected => "same_projected",
        CrsRequirement::Reprojection => "reprojection",
    }
}

/// Le operazioni del catalogo in ordine di id: l'ordine del documento non
/// dipende da quello delle voci nel sorgente.
fn ordinate() -> Vec<&'static OperationDescriptor> {
    let mut operazioni: Vec<&'static OperationDescriptor> = CATALOG.iter().collect();
    operazioni.sort_by(|a, b| a.id.cmp(b.id));
    operazioni
}

fn descrittore(operazione: &OperationDescriptor) -> Value {
    let mut voce = json!({
        "id": operazione.id,
        "version": operazione.semantic_version,
        "family": famiglia(operazione.family),
        "status": "available",
        "versions": {
            "semantic": operazione.semantic_version,
            "config_schema": operazione.config_schema_version,
            "contract_analysis": operazione.contract_analysis_version,
            "kernel": operazione.kernel_version,
        },
        "arity": arieta(operazione.arity),
        "result_shape": operazione.result_shape.map(forma),
        "determinism": determinismo(operazione.determinism),
        "crs_requirement": operazione.crs_requirement.map(requisito_crs),
        "required_backends": operazione.required_capabilities,
    });
    if let (Some(motivo), Value::Object(campi)) = (motivo_non_eseguibile(operazione.id), &mut voce)
    {
        campi.insert("status".to_owned(), json!("unavailable"));
        campi.insert("reason".to_owned(), json!(motivo));
    }
    voce
}

/// Il documento `plenora-data-kernel-catalog-v1` di questo artefatto.
#[must_use]
pub fn documento() -> Value {
    let operazioni = ordinate();
    let registro: Vec<Value> = operazioni
        .iter()
        .filter(|operazione| motivo_non_eseguibile(operazione.id).is_none())
        .map(|operazione| {
            json!({
                "id": operazione.id,
                "version": operazione.semantic_version,
                "family": famiglia(operazione.family),
            })
        })
        .collect();
    json!({
        "registry": {
            "schema_version": 1,
            "contract": CONTRATTO_REGISTRO,
            "component": COMPONENTE,
            "registry": REGISTRO_KERNEL,
            "operations": registro,
        },
        "plan_format": FORMATO_PIANO,
        "kernels": operazioni.iter().map(|operazione| descrittore(operazione)).collect::<Vec<_>>(),
    })
}
