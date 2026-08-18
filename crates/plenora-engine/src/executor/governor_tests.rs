//! Test del resource accounting (ADR-0002) e della sequenza logica
//! (ADR-0001) nell'executor — Fase 2B, milestone M1a/M1b.

use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};

use super::*;
use crate::planner::validate;

// ---------------------------------------------------------------------------
// Fixture (minime, locali: i test del governor restano auto-contenuti)
// ---------------------------------------------------------------------------

fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]))
}

fn table_contract() -> DataContract {
    DataContract::tabular(table_schema())
}

fn table_batch(ids: &[i64], names: &[&str]) -> RecordBatch {
    RecordBatch::try_new(
        table_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(StringArray::from(
                names.iter().map(|n| Some(*n)).collect::<Vec<_>>(),
            )) as ArrayRef,
        ],
    )
    .expect("batch fixture valido")
}

fn run(
    plan: &serde_json::Value,
    inputs: Inputs,
    contracts: &[(String, DataContract)],
) -> Result<Output> {
    let graph = validate(&plan.to_string(), contracts)?;
    execute(&graph, inputs, RuntimeContext::default())
}

fn single_input(name: &str, batches: Vec<RecordBatch>) -> Inputs {
    Inputs::new()
        .with(name, Input::from_batches(batches).expect("input non vuoto"))
        .expect("input unico")
}

fn streaming_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "r", "op": "table.rename", "in": ["f"],
             "config": {"renames": [{"old_name": "id", "new_name": "key"}]}},
        ],
        "output": "r",
    })
}

// ---------------------------------------------------------------------------
// Tee fan-out: accounting unico (ADR-0002)
// ---------------------------------------------------------------------------

#[test]
fn tee_fan_out_counts_quota_once_and_releases_after_last_reader() {
    let governor = MemoryGovernor::new(1 << 20);
    let batch = table_batch(&[1, 2], &["a", "b"]);
    let bytes = batch.get_array_memory_size() as u64;
    let lease = governor.reserve(bytes, "arco").expect("quota disponibile");
    let upstream: BatchStream = Box::new(std::iter::once(Ok(GovernedBatch::new(
        batch,
        Some(lease),
        None,
    ))));
    let shared = EdgeShared::new(upstream);
    let mut first = shared.register_reader();
    let mut second = shared.register_reader();

    // Il primo consumatore tira l'upstream: il batch e' bufferizzato per il
    // secondo. La quota resta UNA (lease condiviso, mai doppio conteggio).
    let held = first.next().expect("un batch").expect("batch valido");
    assert_eq!(
        governor.reserved_bytes(),
        bytes,
        "accounting unico al fan-out"
    );
    assert_eq!(governor.live_leases(), 1);
    drop(held);
    assert_eq!(
        governor.reserved_bytes(),
        bytes,
        "il buffer del tee trattiene un riferimento per il secondo consumatore"
    );
    assert!(first.next().is_none(), "upstream esaurito");

    // Il secondo consumatore legge dal buffer condiviso: stessa quota.
    let held = second.next().expect("un batch").expect("batch valido");
    assert_eq!(governor.reserved_bytes(), bytes);
    drop(held);
    assert_eq!(
        governor.reserved_bytes(),
        0,
        "nessun leak: la quota torna al governor al Drop dell'ultimo riferimento"
    );
    assert_eq!(governor.live_leases(), 0);
    assert_eq!(governor.peak_reserved_bytes(), bytes);
}

// ---------------------------------------------------------------------------
// BatchSequence: assegnazione in ingresso e propagazione (ADR-0001)
// ---------------------------------------------------------------------------

#[test]
fn input_edge_assigns_lease_and_batch_sequence() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input(
        "main",
        vec![table_batch(&[1], &["a"]), table_batch(&[2], &["b"])],
    );
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");
    let (governed, metrics) = output.collect_governed().expect("stream ok");

    assert_eq!(governed.len(), 2);
    for (index, batch) in governed.iter().enumerate() {
        let seq = batch.seq.as_ref().expect("sequenza assegnata sull'input");
        assert_eq!(seq.source_node, "main");
        assert_eq!(seq.input_partition, 0);
        assert_eq!(seq.sequence_number, index as u64);
        let lease = batch.lease.as_ref().expect("lease assegnato sull'input");
        assert_eq!(lease.bytes(), batch.batch.get_array_memory_size() as u64);
        assert_eq!(lease.owner(), "main");
    }
    // I batch raccolti trattengono ancora i lease: osservabilita' ADR-0002.
    let held: u64 = governed.iter().map(GovernedBatch::accounted_bytes).sum();
    assert_eq!(metrics.memory.reserved_bytes, held);
    assert_eq!(metrics.memory.live_leases, 2);
}

#[test]
fn streaming_chain_propagates_sequence_one_to_one() {
    let inputs = single_input(
        "main",
        vec![
            table_batch(&[0, 1], &["a", "b"]),
            table_batch(&[2, 3], &["c", "d"]),
        ],
    );
    let output = run(
        &streaming_plan(),
        inputs,
        &[("main".to_owned(), table_contract())],
    )
    .expect("execute");
    let (governed, _) = output.collect_governed().expect("stream ok");

    assert_eq!(
        governed.len(),
        2,
        "un batch in uscita per batch in ingresso"
    );
    for (index, batch) in governed.iter().enumerate() {
        // Propagazione 1:1: la sequenza resta quella dell'input di origine.
        let seq = batch.seq.as_ref().expect("sequenza propagata");
        assert_eq!(seq.source_node, "main");
        assert_eq!(seq.input_partition, 0);
        assert_eq!(seq.sequence_number, index as u64);
        // Il lease e' quello NUOVO dell'output del segmento, non l'input.
        let lease = batch.lease.as_ref().expect("lease dell'output");
        assert_eq!(lease.owner(), "r");
        assert_eq!(lease.bytes(), batch.batch.get_array_memory_size() as u64);
    }
}

#[test]
fn blocking_segment_reassigns_sequence_deterministically() {
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "g", "op": "table.aggregate", "in": ["main"],
             "config": {"group_by": ["id"], "aggregations": []}},
        ],
        "output": "g",
    });
    let blocking_seq = || {
        let inputs = single_input(
            "main",
            vec![table_batch(&[1, 2], &["a", "b"]), table_batch(&[3], &["c"])],
        );
        let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");
        let (governed, metrics) = output.collect_governed().expect("stream ok");
        assert_eq!(governed.len(), 1, "il blocking emette un batch unico");
        let node = &metrics.nodes["g"];
        assert!(
            node.bytes_in > 0 && node.bytes_out > 0,
            "metriche byte popolate"
        );
        governed[0].seq.clone().expect("sequenza riassegnata")
    };
    let first = blocking_seq();
    let second = blocking_seq();
    assert_eq!(first, second, "riassegnazione deterministica");
    // Regola v1 documentata in `blocking_output_sequence`.
    assert_eq!(first.source_node, "g");
    assert_eq!(first.input_partition, 0);
    assert_eq!(first.sequence_number, 0);
}

// ---------------------------------------------------------------------------
// Metriche e budget (ADR-0002)
// ---------------------------------------------------------------------------

#[test]
fn memory_metrics_are_populated_and_leak_free_after_drain() {
    let inputs = single_input(
        "main",
        vec![
            table_batch(&[0, 1], &["a", "b"]),
            table_batch(&[2, 3], &["c", "d"]),
        ],
    );
    let output = run(
        &streaming_plan(),
        inputs,
        &[("main".to_owned(), table_contract())],
    )
    .expect("execute");
    let (batches, metrics) = output.collect_batches().expect("stream ok");
    assert_eq!(batches.len(), 2);

    assert!(
        metrics.memory.budget_bytes > 0,
        "budget dai limiti effettivi"
    );
    assert!(metrics.memory.peak_reserved_bytes > 0, "picco registrato");
    assert_eq!(
        metrics.memory.reserved_bytes, 0,
        "dopo il drenaggio completo nessun lease resta trattenuto"
    );
    assert_eq!(metrics.memory.live_leases, 0);
    assert!(metrics.memory.oldest_lease_age.is_none());

    for node_id in ["f", "r"] {
        let node = &metrics.nodes[node_id];
        assert!(node.bytes_in > 0, "bytes_in popolato per {node_id}");
        assert!(node.bytes_out > 0, "bytes_out popolato per {node_id}");
    }
}

#[test]
fn memory_budget_exhaustion_fails_fast_with_contract_error() {
    let batch = table_batch(&[1, 2], &["a", "b"]);
    let bytes = batch.get_array_memory_size() as u64;
    let plan = json!({
        "schema_version": 4,
        "limits": {"max_memory_bytes": bytes - 1},
        "inputs": ["main"],
        "nodes": [],
        "output": "main",
    });
    let inputs = single_input("main", vec![batch]);
    let output = run(&plan, inputs, &[("main".to_owned(), table_contract())]).expect("execute");
    let error = output
        .collect_batches()
        .expect_err("budget esaurito: fail-fast");
    // Nono giro: categoria `resource_limit`, non `invalid_plan`. Attraversa
    // gli involucri, quindi si guarda `category()` e non la variante esterna.
    assert_eq!(
        error.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "errore ResourceLimit max_memory_bytes: {error}"
    );
    assert!(
        error.to_string().contains("max_memory_bytes"),
        "il testo dice quale tetto: {error}"
    );
}
