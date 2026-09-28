//! Rendering: come i documenti della CLI diventano JSON.
//!
//! Descrittore di un'operazione, versione, backend compilati, contratto di un
//! input: superficie pubblica quanto i nomi dei comandi, e insieme perche'
//! non descrivano lo stesso concetto con nomi diversi.
//! `tests/oracolo_superficie_cli.snap` fissa `capabilities` e `catalog` byte
//! per byte.

use plenora_core::catalog::{OperationDescriptor, CATALOG};
use plenora_core::contract::{ContractCrs, DataContract};
use plenora_core::PlenoraError;
use plenora_engine::planner::ValidatedGraph;
use plenora_engine::{ExecutionMetrics, ExecutionPlan};

/// Digest esadecimale minuscolo.
pub fn hex_digest(digest: &[u8; 32]) -> String {
    plenora_core::esadecimale::esadecimale(digest)
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

/// Riepilogo JSON di `validate` per un piano DAG: nodi, archi con contratti,
/// segmenti con modo e strategia, capability e identita' piano-v5.md#identita-e-fingerprint.
///
/// # Errors
///
/// `Internal` se un arco del grafo manca dai contratti: impossibile su un
/// grafo validato (come [`ValidatedGraph::output_contract`]), ma reso
/// errore esplicito invece di un panic (R6).
pub fn graph_summary_json(
    graph: &ValidatedGraph,
    execution: &ExecutionPlan,
) -> Result<serde_json::Value, PlenoraError> {
    let plan = graph.plan();
    let nodes: Vec<serde_json::Value> = plan
        .nodes()
        .iter()
        .map(|node| {
            serde_json::json!({
                "id": node.id,
                "op": node.op,
                "in": node.inputs,
            })
        })
        .collect();
    let mut edges: Vec<serde_json::Value> = Vec::new();
    for name in plan.inputs() {
        let contract = graph.edge_contract(name).ok_or_else(|| {
            PlenoraError::Internal("l'input e' un arco del grafo validato".into())
        })?;
        edges.push(serde_json::json!({
            "edge": name,
            "kind": "input",
            "contract": contract_json(contract),
        }));
    }
    for node_id in graph.topological_order() {
        let contract = graph.edge_contract(node_id).ok_or_else(|| {
            PlenoraError::Internal("il nodo e' un arco del grafo validato".into())
        })?;
        edges.push(serde_json::json!({
            "edge": node_id,
            "kind": "node",
            "contract": contract_json(contract),
        }));
    }
    let segments: Vec<serde_json::Value> = execution
        .segments()
        .iter()
        .map(|segment| {
            serde_json::json!({
                "id": segment.id,
                "mode": format!("{:?}", segment.mode),
                "parallelism": format!("{:?}", segment.parallelism),
                "nodes": segment.kernels.iter().map(|kernel| &kernel.node_id).collect::<Vec<_>>(),
                "materialize_output": segment.materialize_output,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "status": "ok",
        // La versione la dice il PIANO, non questa riga. Fissarla a 5
        // descriverebbe un piano v6 come un v5 — con accanto un `plan_hash`
        // di un altro dominio.
        "schema_version": plan.schema_version(),
        "plan_hash": graph.plan_hash().to_hex(),
        "engine_version": graph.engine_version().to_string(),
        "inputs": plan.inputs(),
        "topological_order": graph.topological_order(),
        "nodes": nodes,
        "edges": edges,
        "segments": segments,
        "required_capabilities": graph.required_capabilities().names().collect::<Vec<_>>(),
        "input_contract_fingerprints": graph
            .input_contract_fingerprints()
            .iter()
            .map(plenora_engine::planner::ContractFingerprint::to_hex)
            .collect::<Vec<_>>(),
    }))
}

/// Metriche JSON di un `run` v4: per nodo logico e per segmento (righe,
/// batch e byte in/out, wall time in millisecondi), i totali di
/// pubblicazione, il contatore dei fallback della fusione geo (D12.7: ogni
/// fallback governor e' osservabile, mai silenzioso), l'osservabilita' dei
/// lease di memoria e le metriche di spill aggregate (architettura.md#memoria).
pub fn metrics_json(graph: &ValidatedGraph, metrics: &ExecutionMetrics) -> serde_json::Value {
    let nodes: serde_json::Map<String, serde_json::Value> = metrics
        .nodes
        .iter()
        .map(|(id, node)| {
            (
                id.clone(),
                serde_json::json!({
                    "operation": node.operation,
                    "rows_in": node.rows_in,
                    "rows_out": node.rows_out,
                    "batches_in": node.batches_in,
                    "batches_out": node.batches_out,
                    "bytes_in": node.bytes_in,
                    "bytes_out": node.bytes_out,
                    "wall_time_ms": node.wall_time.as_secs_f64() * 1000.0,
                }),
            )
        })
        .collect();
    let segments: serde_json::Map<String, serde_json::Value> = metrics
        .segments
        .iter()
        .map(|(id, segment)| {
            (
                id.clone(),
                serde_json::json!({
                    "mode": format!("{:?}", segment.mode),
                    "rows_in": segment.rows_in,
                    "rows_out": segment.rows_out,
                    "batches_in": segment.batches_in,
                    "batches_out": segment.batches_out,
                    "wall_time_ms": segment.wall_time.as_secs_f64() * 1000.0,
                }),
            )
        })
        .collect();
    serde_json::json!({
        "status": "ok",
        // Come in `explain`: la versione la dice il piano. Fissandola a 5, un
        // riepilogo di `run` su un piano v6 dichiarerebbe 5 accanto a un
        // `plan_hash` di un altro dominio — cioe' proprio la coppia che rende
        // irriconoscibile un'identita' conservata.
        "schema_version": graph.plan_format_version(),
        "plan_hash": graph.plan_hash().to_hex(),
        "output_rows": metrics.output_rows,
        "output_batches": metrics.output_batches,
        "total_rows_processed": metrics.total_rows_processed,
        "geo_fusion_fallbacks": metrics.geo_fusion_fallbacks,
        "geo_fusion_groups_started": metrics.geo_fusion_groups_started,
        "memory": {
            "budget_bytes": metrics.memory.budget_bytes,
            "reserved_bytes": metrics.memory.reserved_bytes,
            "peak_reserved_bytes": metrics.memory.peak_reserved_bytes,
            "live_leases": metrics.memory.live_leases,
            "oldest_lease_age_ms": metrics
                .memory
                .oldest_lease_age
                .map(|age| age.as_secs_f64() * 1000.0),
        },
        "spill": {
            "bytes_written": metrics.spill.bytes_written,
            "bytes_read": metrics.spill.bytes_read,
            "files": metrics.spill.files,
        },
        "nodes": nodes,
        "segments": segments,
    })
}
