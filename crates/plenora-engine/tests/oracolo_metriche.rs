//! Oracolo delle metriche deterministiche dell'executor.
//!
//! # Perche' esiste
//!
//! `executor.rs` concentra il governo della memoria, il fan-out, lo spill e
//! la cancellazione. Riorganizzarlo non ha un criterio di uscita
//! verificabile come l'output byte-identico della CLI: la suite
//! dell'executor verifica che il **comportamento osservato** non cambi, ma
//! non guarda i conteggi.
//!
//! Uno spostamento meccanico non deve cambiare quante righe attraversano un
//! nodo, quanti batch escono, quanti byte si spillano o quante volte il
//! runner fuso ricade sul percorso non fuso. Sono proprio le grandezze che
//! una riorganizzazione puo' spostare senza rompere un test: un batch in piu'
//! o in meno non cambia il risultato, cambia il come — ed e' il come che
//! stiamo riorganizzando.
//!
//! # Che cosa NON registra, e perche'
//!
//! **I tempi.** `wall_time` per nodo e per segmento, e tutto cio' che in
//! `MemoryMetrics` dipende da quando si guarda (eta' del lease piu' vecchio,
//! byte trattenuti al momento della lettura). Registrarli renderebbe
//! l'oracolo instabile, e un oracolo che fallisce a caso viene rigenerato
//! senza guardarlo — cioe' smette di essere un oracolo.
//!
//! Resta fuori anche il **picco** di memoria: dipende dall'ordine con cui il
//! sistema operativo restituisce le allocazioni, non dal piano.
//!
//! # Rigenerazione
//!
//! ```sh
//! PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-engine --test oracolo_metriche
//! ```
//!
//! Un cambiamento **intenzionale** del piano fisico si vede qui come diff dei
//! conteggi, ed e' esattamente cio' che deve arrivare in review.

use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::DataContract;
use plenora_engine::planner::validate;
use plenora_engine::{execute, ExecutionMetrics, Input, Inputs, RuntimeContext};
use serde_json::{json, Value};

const ORACOLO_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracolo_metriche.snap");

// ---------------------------------------------------------------------------
// Dati di ingresso: fissi, e in piu' batch
// ---------------------------------------------------------------------------

fn schema_tabellare() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("gruppo", DataType::Utf8, true),
        Field::new("valore", DataType::Float64, true),
    ]))
}

fn contratto() -> DataContract {
    DataContract::tabular(schema_tabellare())
}

/// Tre batch da quattro righe. Piu' di un batch **serve**: con un batch solo
/// i conteggi `batches_in`/`batches_out` sarebbero sempre 1 e l'oracolo non
/// direbbe nulla su come lo streaming raggruppa le righe.
fn batch(inizio: i64) -> RecordBatch {
    let id: Vec<i64> = (inizio..inizio + 4).collect();
    let gruppo: Vec<&str> = id
        .iter()
        .map(|i| if i % 2 == 0 { "a" } else { "b" })
        .collect();
    let valore: Vec<f64> = id
        .iter()
        .map(|i| f64::from(i32::try_from(*i).unwrap_or(0)) * 1.5)
        .collect();
    RecordBatch::try_new(
        schema_tabellare(),
        vec![
            Arc::new(Int64Array::from(id)),
            Arc::new(StringArray::from(gruppo)),
            Arc::new(Float64Array::from(valore)),
        ],
    )
    .expect("batch valido")
}

fn ingresso() -> Vec<RecordBatch> {
    vec![batch(0), batch(4), batch(8)]
}

// ---------------------------------------------------------------------------
// I piani
// ---------------------------------------------------------------------------

struct Caso {
    nome: &'static str,
    /// Che cosa questo piano mette alla prova nelle metriche.
    perche: &'static str,
    binario: bool,
    piano: Value,
}

fn casi() -> Vec<Caso> {
    vec![
        Caso {
            nome: "streaming_filtro",
            perche: "1:1 in streaming: i batch escono come entrano, le righe no",
            binario: false,
            piano: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "f", "op": "table.filter", "in": ["main"],
                     "config": {"column": "valore", "operator": ">", "value": 5.0}}
                ],
                "output": "f"
            }),
        },
        Caso {
            nome: "blocking_ordinamento",
            perche: "blocking: materializza tutto e pubblica un batch solo",
            binario: false,
            piano: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "o", "op": "table.sort", "in": ["main"],
                     "config": {"columns": ["valore"], "ascending": false}}
                ],
                "output": "o"
            }),
        },
        Caso {
            nome: "blocking_aggregazione",
            perche: "riduce le righe e cambia lo schema: rows_out non segue rows_in",
            binario: false,
            piano: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "a", "op": "table.aggregate", "in": ["main"],
                     "config": {"group_by": ["gruppo"],
                                "aggregations": [{"column": "valore", "function": "sum"}]}}
                ],
                "output": "a"
            }),
        },
        Caso {
            nome: "catena_streaming_blocking",
            perche: "due nodi: total_rows_processed somma gli ingressi, non le uscite",
            binario: false,
            piano: json!({
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "f", "op": "table.filter", "in": ["main"],
                     "config": {"column": "id", "operator": ">=", "value": 2}},
                    {"id": "o", "op": "table.sort", "in": ["f"],
                     "config": {"columns": ["id"], "ascending": true}}
                ],
                "output": "o"
            }),
        },
        Caso {
            nome: "binaria_join",
            perche: "due ingressi: le righe processate contano entrambi i lati",
            binario: true,
            piano: json!({
                "schema_version": 5,
                "inputs": ["sinistra", "destra"],
                "nodes": [
                    {"id": "j", "op": "table.join", "in": ["sinistra", "destra"],
                     "config": {"left_keys": ["id"], "right_keys": ["id"], "how": "inner"}}
                ],
                "output": "j"
            }),
        },
    ]
}

// ---------------------------------------------------------------------------
// Estrazione delle sole grandezze deterministiche
// ---------------------------------------------------------------------------

/// Tutto tranne i tempi e le grandezze che dipendono da QUANDO si guarda.
fn deterministiche(metriche: &ExecutionMetrics) -> Value {
    let nodi: Vec<Value> = metriche
        .nodes
        .iter()
        .map(|(id, n)| {
            json!({
                "id": id,
                "operation": n.operation,
                "rows_in": n.rows_in,
                "rows_out": n.rows_out,
                "batches_in": n.batches_in,
                "batches_out": n.batches_out,
                "bytes_in": n.bytes_in,
                "bytes_out": n.bytes_out,
            })
        })
        .collect();
    let segmenti: Vec<Value> = metriche
        .segments
        .iter()
        .map(|(id, s)| {
            json!({
                "id": id,
                "mode": format!("{:?}", s.mode),
                "rows_in": s.rows_in,
                "rows_out": s.rows_out,
                "batches_in": s.batches_in,
                "batches_out": s.batches_out,
            })
        })
        .collect();
    json!({
        "output_rows": metriche.output_rows,
        "output_batches": metriche.output_batches,
        "total_rows_processed": metriche.total_rows_processed,
        "geo_fusion_fallbacks": metriche.geo_fusion_fallbacks,
        "geo_fusion_groups_started": metriche.geo_fusion_groups_started,
        "counters_saturated": metriche.counters_saturated,
        "spill_files": metriche.spill.files,
        "spill_bytes_written": metriche.spill.bytes_written,
        "spill_bytes_read": metriche.spill.bytes_read,
        "nodes": nodi,
        "segments": segmenti,
    })
}

fn esegui(caso: &Caso) -> Value {
    let grafo = validate(
        &caso.piano.to_string(),
        &if caso.binario {
            vec![
                ("sinistra".to_owned(), contratto()),
                ("destra".to_owned(), contratto()),
            ]
        } else {
            vec![("main".to_owned(), contratto())]
        },
    )
    .unwrap_or_else(|error| panic!("il piano `{}` deve essere valido: {error}", caso.nome));

    let inputs = if caso.binario {
        Inputs::strict()
            .with_contract(
                "sinistra",
                Input::from_batches(ingresso()).expect("input sinistro"),
                contratto(),
            )
            .expect("sinistra")
            .with_contract(
                "destra",
                Input::from_batches(ingresso()).expect("input destro"),
                contratto(),
            )
            .expect("destra")
    } else {
        Inputs::strict()
            .with_contract(
                "main",
                Input::from_batches(ingresso()).expect("input"),
                contratto(),
            )
            .expect("main")
    };

    let (batches, metriche) = execute(&grafo, inputs, RuntimeContext::default())
        .unwrap_or_else(|error| panic!("`{}`: execute fallita: {error}", caso.nome))
        .collect_batches()
        .unwrap_or_else(|error| panic!("`{}`: stream fallito: {error}", caso.nome));

    let righe: usize = batches.iter().map(RecordBatch::num_rows).sum();
    json!({
        "nome": caso.nome,
        "perche": caso.perche,
        "batch_prodotti": batches.len(),
        "righe_prodotte": righe,
        "metriche": deterministiche(&metriche),
    })
}

fn oracolo_content() -> String {
    let voci: Vec<Value> = casi().iter().map(esegui).collect();
    let mut content = serde_json::to_string_pretty(&voci).expect("la serializzazione non fallisce");
    content.push('\n');
    content
}

/// Le metriche deterministiche devono coincidere con l'oracolo committato.
#[test]
fn le_metriche_deterministiche_coincidono_con_l_oracolo() {
    let attuale = oracolo_content();
    let path = std::path::Path::new(ORACOLO_PATH);
    if std::env::var_os("PLENORA_UPDATE_SNAPSHOT").is_some() {
        std::fs::write(path, &attuale).expect("rigenerazione dell'oracolo delle metriche");
        eprintln!("oracolo delle metriche rigenerato in {ORACOLO_PATH}");
        return;
    }
    let atteso = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("oracolo non leggibile ({error}): generarlo con PLENORA_UPDATE_SNAPSHOT=1")
    });
    let atteso = atteso.replace("\r\n", "\n");
    assert!(
        attuale == atteso,
        "le metriche deterministiche divergono dall'oracolo committato {ORACOLO_PATH}. \
         Uno spostamento meccanico di codice NON deve produrre questo diff: se lo produce, \
         il piano fisico e' cambiato. Se il cambiamento e' intenzionale, rigenerare con \
         `PLENORA_UPDATE_SNAPSHOT=1 cargo test -p plenora-engine --test oracolo_metriche` \
         e spiegare nel messaggio di commit quale scelta fisica e' cambiata"
    );
}

/// Le grandezze escluse dall'oracolo esistono davvero e restano leggibili:
/// se un giorno sparissero, l'oracolo continuerebbe a passare mentendo per
/// omissione.
#[test]
fn le_grandezze_escluse_restano_osservabili() {
    let caso = &casi()[0];
    let grafo = validate(&caso.piano.to_string(), &[("main".to_owned(), contratto())])
        .expect("piano valido");
    let inputs = Inputs::strict()
        .with_contract(
            "main",
            Input::from_batches(ingresso()).expect("input"),
            contratto(),
        )
        .expect("main");
    let (_, metriche) = execute(&grafo, inputs, RuntimeContext::default())
        .expect("execute")
        .collect_batches()
        .expect("stream");
    assert!(
        metriche
            .nodes
            .values()
            .all(|n| n.wall_time.as_nanos() < u128::MAX),
        "wall_time per nodo resta leggibile"
    );
    assert!(
        metriche
            .segments
            .values()
            .all(|s| s.wall_time.as_nanos() < u128::MAX),
        "wall_time per segmento resta leggibile"
    );
}
