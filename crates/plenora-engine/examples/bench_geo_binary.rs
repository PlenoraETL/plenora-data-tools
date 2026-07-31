//! Misura A/B dei binari geo (ADR-0014 M3, D14.10 — chiusura del cantiere):
//! CLI standalone (trasporto v3 `pair_arrow`, la stessa camminata del
//! comando `pair-arrow` senza processo) contro piano v4 (`geo.sjoin`,
//! `geo.within`), piu' il controllo di non regressione dei `table.*` binari
//! sul guscio condiviso `run_binary_blocking` (smistamento su
//! `PreparedConfig` aggiunto da D14.2 — il ramo tabellare deve restare
//! invariato di fatto, non solo di lettura).
//!
//! Perimetro di misura (dichiarato: le due modalita' NON misurano lo
//! stesso perimetro, il confronto e' sull'ordine di grandezza —
//! accettazione D14.10: «bande non sovrapposte o delta entro rumore
//! documentato», stampato e non bloccante):
//!
//! - lato v3 (`v3_cli_pair_arrow`): gli envelope IPC sono costruiti FUORI
//!   dal timing (una sola volta); nel timing c'e' `pair_arrow` completo —
//!   envelope read, decode IPC, decode WKB validante, kernel, encode
//!   output. La decodifica dell'output per l'oracolo e' fuori timing;
//! - lato v4 (`v4_plan`): nel timing c'e'
//!   `execute(graph, inputs, runtime).collect_batches()` con
//!   `Input::from_batches` — input Arrow gia' in memoria, niente IPC decode
//!   (differenza di contratto, dichiarata); il v4 paga invece governor,
//!   framing dei batch e preflight D14.4. `validate` e la costruzione
//!   delle fixture sono fuori timing.
//!
//! Fixture geo (deterministica, niente RNG — ADR-0001): 100 000 righe per
//! lato, batch da 10 000, griglia di 500 colonne a passo 100 (come
//! `bench_geo_fusion`): left alterna quadrati 20x20 all'origine della cella
//! e punti al centro, con l'1 per mille di null; right e' quadrati sulla
//! stessa griglia traslati di (+10,+10). Conti a mano (sanity stampata,
//! non soglia): left ha due righe per cella, right una (cella = indice di
//! riga right), quindi la candidata di una riga left e' la right della
//! stessa cella e basta — i quadrati right 20x20 coprono [x+10, x+30]:
//!
//! - scenario `sjoin`: i quadrati left (righe pari, mai null) intersecano
//!   la right della cella (overlap 10x10), i punti (x+50) restano fuori da
//!   ogni fascia -> 49 950 coppie attese: 50 000 quadrati meno le 50 right
//!   null nella regione condivisa (left copre le celle 0..49 999 — due righe
//!   per cella — right 0..99 999; le right null sono le righe j ≡ 999 mod
//!   1000, 100 in tutto ma solo 50 sotto 50 000), "circa meta' delle righe
//!   left";
//! - scenario `within`: con quadrati 20x20 NESSUN left sarebbe within
//!   (quadrati di pari lato traslati; punti fuori fascia) — oracolo
//!   banale. Il lato right di questo scenario usa quadrati 60x60 (stessa
//!   griglia, stessa traslazione +10, deviazione dichiarata): i punti
//!   (x+50, y+50) cadono strettamente dentro la right della cella, i
//!   quadrati left non sono mai within -> 49 900 flag true attesi (tutti i
//!   punti non null: le 50 right null della regione condivisa cadono su
//!   celle c ≡ 999 mod 1000, che sono esattamente celle con punto null —
//!   c ≡ 999 mod 1000 implica c ≡ 499 mod 500, la regola dei null left),
//!   100 flag null (righe left null), "circa meta'" dei non null.
//!
//! Controllo `table_join_control` (solo v4): join inner 1:1 su `id` di due
//! tabelle da 1M righe (id Int64 + payload Int64), stessa forma del
//! riferimento storico `benchmarks/baseline/baseline.md` par. 1 («join
//! inner su id | 1M | 0,6361s», stesso container rust:1.92 ma sensibile
//! all'host — citato come riferimento, NON come soglia).
//!
//! Oracolo (bloccante, exit != 0 al fallimento):
//!
//! - cross-run (ADR-0001): serializzazione IPC (`FileWriter`) dell'output
//!   di OGNI run di una modalita' -> byte-identici tra run;
//! - cross-path (semantico, MAI byte — gli schemi v3/v4 differiscono per
//!   contratto D14.8): sjoin confronta le coppie (left, right) in ordine
//!   canonico (v3: colonne lineage; v4: colonna `id` = indice di riga left
//!   della fixture + `__right_index`); within confronta il vettore di flag
//!   allineato alle righe left.
//!
//! Mitigazione allocatore (documentata in `benchmarks/sweep/geo_sweep.md`:
//! stallo `brk`/`__vma_start_write` su WSL2 sotto carico di allocazioni
//! intensive): eseguire con `MALLOC_ARENA_MAX=4
//! MALLOC_MMAP_THRESHOLD_=32768` nel container rust:1.92.
//!
//! Uso: `cargo run -p plenora-engine --release --locked --example
//! bench_geo_binary` — una riga JSON per scenario+modalita' su stdout,
//! piu' una riga di sintesi per scenario con delta percentuale, sovrapposizione
//! delle bande e note di sanita'.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Instant;

use geo::{polygon, Geometry, Point};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Int64Array, RecordBatch, UInt64Array,
};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_engine::geo_transport::transport::{
    decode_ipc, encode_ipc, pair_arrow, EnvelopeReader, EnvelopeWriter, PairArrowSchema,
};
use plenora_engine::planner::{validate, ValidatedGraph};
use plenora_engine::{execute, Input, Inputs, RuntimeContext};
use plenora_kernels_geo::arrow_adapter::geometry_output_field;
use serde_json::{json, Value};

/// Righe per lato degli scenari geo (dimensione dichiarata: ogni run resta
/// entro pochi secondi in release nel container rust:1.92).
const GEO_ROWS: usize = 100_000;
/// Righe per batch degli scenari geo (come `bench_geo_fusion`).
const GEO_BATCH_ROWS: usize = 10_000;
/// Righe per lato del controllo tabellare (come il baseline storico).
const TABLE_ROWS: usize = 1_000_000;
/// Righe per batch del controllo tabellare (batch grandi: il join e'
/// monolitico per costruzione, il framing non e' la variabile misurata).
const TABLE_BATCH_ROWS: usize = 100_000;
/// Run per modalita', alternate A,B,A,B... per non legare il risultato
/// all'ordine di esecuzione.
const RUNS: usize = 5;
/// `max_pairs` degli schemi v3: lo stesso tetto risolto dal v4 per un piano
/// a nodo singolo con output sul nodo (D14.6: `max_output_rows` di default
/// = `MAX_PAIRS` del protocollo coppie — stesse costanti dell'oracolo
/// `geo_binary_oracle.rs`).
const V3_MAX_PAIRS: u64 = 10_000_000;

// ---------------------------------------------------------------------------
// Fixture geo (schema/contratto identici all'oracolo D14.9)
// ---------------------------------------------------------------------------

fn geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        geometry_output_field("geom", "EPSG:32632").expect("campo geometria"),
    ]))
}

fn geo_contract() -> DataContract {
    DataContract::new(
        geo_schema(),
        vec![GeometryColumnContract {
            field_id: FieldId(3),
            name: "geom".to_owned(),
            crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                "EPSG:32632".to_owned(),
                json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
                CrsKind::Projected,
                Some(1.0),
            )),
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto fixture valido")
}

fn square_wkb(origin_x: f64, origin_y: f64, side: f64) -> Vec<u8> {
    Geometry::Polygon(polygon![
        (x: origin_x, y: origin_y),
        (x: origin_x + side, y: origin_y),
        (x: origin_x + side, y: origin_y + side),
        (x: origin_x, y: origin_y + side),
        (x: origin_x, y: origin_y),
    ])
    .to_wkb(CoordDimensions::xy())
    .expect("wkb fixture")
}

fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    Geometry::Point(Point::new(x, y))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb fixture")
}

fn geo_batch(ids: &[i64], cells: &[Option<Vec<u8>>]) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    RecordBatch::try_new(
        geo_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geo fixture valido")
}

/// Lato left: due righe per cella di griglia (cella = riga / 2; colonna =
/// cella % 500, linea = cella / 500) — righe pari quadrato 20x20
/// all'origine, righe dispari punto al centro (x+50, y+50); 1 per mille di
/// null (righe 999, 1999, ... — tutte dispari: i null cadono sui punti).
/// `id` = indice di riga: e' la chiave dell'oracolo cross-path sjoin
/// ((id, `__right_index`) del v4 == (left, right) del lineage v3).
fn geo_left_batches() -> Vec<RecordBatch> {
    let mut batches = Vec::with_capacity(GEO_ROWS.div_ceil(GEO_BATCH_ROWS));
    for first in (0..GEO_ROWS).step_by(GEO_BATCH_ROWS) {
        let len = GEO_BATCH_ROWS.min(GEO_ROWS - first);
        let ids: Vec<i64> = (first..first + len)
            .map(|row| i64::try_from(row).expect("righe < i64::MAX"))
            .collect();
        let cells: Vec<Option<Vec<u8>>> = (first..first + len)
            .map(|row| {
                if row % 1_000 == 999 {
                    None
                } else {
                    let cell = row / 2;
                    let x =
                        f64::from(u32::try_from(cell % 500).expect("colonna < u32::MAX")) * 100.0;
                    let y = f64::from(u32::try_from(cell / 500).expect("linea < u32::MAX")) * 100.0;
                    if row % 2 == 0 {
                        Some(square_wkb(x, y, 20.0))
                    } else {
                        Some(point_wkb(x + 50.0, y + 50.0))
                    }
                }
            })
            .collect();
        batches.push(geo_batch(&ids, &cells));
    }
    batches
}

/// Lato right: una riga per cella (cella = indice di riga, stessa formula
/// di griglia del left), quadrato `side`x`side` traslato di (+10,+10);
/// 1 per mille di null. `side` = 20 per `sjoin`, 60 per `within` (vedi
/// l'header: con 20 nessun left sarebbe within — oracolo banale).
fn geo_right_batches(side: f64) -> Vec<RecordBatch> {
    let mut batches = Vec::with_capacity(GEO_ROWS.div_ceil(GEO_BATCH_ROWS));
    for first in (0..GEO_ROWS).step_by(GEO_BATCH_ROWS) {
        let len = GEO_BATCH_ROWS.min(GEO_ROWS - first);
        let ids: Vec<i64> = (first..first + len)
            .map(|row| i64::try_from(row).expect("righe < i64::MAX"))
            .collect();
        let cells: Vec<Option<Vec<u8>>> = (first..first + len)
            .map(|row| {
                if row % 1_000 == 999 {
                    None
                } else {
                    let x =
                        f64::from(u32::try_from(row % 500).expect("colonna < u32::MAX")) * 100.0;
                    let y = f64::from(u32::try_from(row / 500).expect("linea < u32::MAX")) * 100.0;
                    Some(square_wkb(x + 10.0, y + 10.0, side))
                }
            })
            .collect();
        batches.push(geo_batch(&ids, &cells));
    }
    batches
}

// ---------------------------------------------------------------------------
// Fixture tabellare (controllo table.* — id Int64 + payload Int64)
// ---------------------------------------------------------------------------

fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("value", DataType::Int64, false),
    ]))
}

/// 1M righe (id = 0..N, chiavi identiche nei due lati -> join inner 1:1 da
/// 1M righe di output); payload deterministico con sale diverso per lato
/// (niente RNG, ADR-0001).
fn table_batches(salt: i64) -> Vec<RecordBatch> {
    let mut batches = Vec::with_capacity(TABLE_ROWS.div_ceil(TABLE_BATCH_ROWS));
    for first in (0..TABLE_ROWS).step_by(TABLE_BATCH_ROWS) {
        let len = TABLE_BATCH_ROWS.min(TABLE_ROWS - first);
        let ids: Vec<i64> = (first..first + len)
            .map(|row| i64::try_from(row).expect("righe < i64::MAX"))
            .collect();
        let values: Vec<i64> = (first..first + len)
            .map(|row| i64::try_from(row).expect("righe < i64::MAX") * salt % 1_000_003)
            .collect();
        batches.push(
            RecordBatch::try_new(
                table_schema(),
                vec![
                    Arc::new(Int64Array::from(ids)) as ArrayRef,
                    Arc::new(Int64Array::from(values)) as ArrayRef,
                ],
            )
            .expect("batch tabellare fixture valido"),
        );
    }
    batches
}

// ---------------------------------------------------------------------------
// Harness di misura
// ---------------------------------------------------------------------------

/// Envelope v3 (`PLNGEO3`) attorno al payload IPC — costruito UNA VOLTA
/// fuori dal timing (perimetro dichiarato nell'header).
fn encode_envelope(batches: &[RecordBatch]) -> Vec<u8> {
    let payload = encode_ipc(&batches[0].schema(), batches).expect("payload IPC");
    let mut bytes = Vec::new();
    let mut writer = EnvelopeWriter::new(&mut bytes, payload.len() as u64).expect("envelope");
    writer.write_payload(&payload).expect("scrittura payload");
    writer.finish().expect("chiusura envelope");
    bytes
}

/// Serializzazione IPC dell'output (oracolo cross-run byte-per-byte, come
/// `ipc_bytes` nei test dell'executor).
fn ipc_bytes(batches: &[RecordBatch]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut writer = FileWriter::try_new(&mut bytes, &batches[0].schema()).expect("writer");
        for batch in batches {
            writer.write(batch).expect("write");
        }
        writer.finish().expect("finish");
    }
    bytes
}

/// Una run v3 (NEL timing: envelope read + decode IPC + decode WKB
/// validante + kernel + encode output — l'intero `pair_arrow`). La
/// decodifica dell'output per l'oracolo e' fuori timing ([`decode_v3_output`]).
fn run_v3_timed(
    left_envelope: &[u8],
    right_envelope: &[u8],
    schema: &PairArrowSchema,
) -> (Vec<u8>, f64) {
    let mut output = Vec::new();
    let start = Instant::now();
    pair_arrow(
        Cursor::new(left_envelope),
        Cursor::new(right_envelope),
        &mut output,
        schema,
    )
    .expect("pair_arrow");
    (output, start.elapsed().as_secs_f64())
}

/// Decodifica dell'output v3 (fuori timing, solo per l'oracolo).
fn decode_v3_output(bytes: &[u8]) -> Vec<RecordBatch> {
    let payload = EnvelopeReader::new(Cursor::new(bytes))
        .expect("envelope di output")
        .read_payload()
        .expect("payload di output");
    let (_schema, batches) = decode_ipc(&payload).expect("IPC di output");
    batches
}

fn two_geo_inputs(left: Vec<RecordBatch>, right: Vec<RecordBatch>) -> Inputs {
    Inputs::new()
        .with(
            "left_in",
            Input::from_batches(left).expect("left non vuoto"),
        )
        .expect("input left")
        .with(
            "right_in",
            Input::from_batches(right).expect("right non vuoto"),
        )
        .expect("input right")
}

/// Una run v4 (NEL timing: `execute` + drenaggio completo dello stream;
/// input Arrow gia' in memoria — niente IPC decode, differenza di contratto
/// dichiarata nell'header). `validate` e' fuori (il `ValidatedGraph`
/// arriva pronto).
fn run_v4_timed(
    graph: &ValidatedGraph,
    left: &[RecordBatch],
    right: &[RecordBatch],
) -> (Vec<RecordBatch>, f64) {
    let inputs = two_geo_inputs(left.to_vec(), right.to_vec());
    let start = Instant::now();
    let (batches, _metrics) = execute(graph, inputs, RuntimeContext::default())
        .expect("execute")
        .collect_batches()
        .expect("stream ok");
    (batches, start.elapsed().as_secs_f64())
}

/// Riga JSON di una modalita': righe/batch della fixture, statistica dei
/// RUNS run (mediana/min/max in secondi) e throughput sulle righe di input
/// totali (left + right). Ritorna (mediana, min, max) per la sintesi.
fn report(
    scenario: &str,
    mode: &str,
    rows: usize,
    batches: usize,
    durations: &mut [f64],
) -> (f64, f64, f64) {
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    let min = durations[0];
    let max = durations[durations.len() - 1];
    // rows << 2^53: la conversione e' esatta per costruzione.
    #[allow(clippy::cast_precision_loss)]
    let rate = rows as f64 / median;
    println!(
        "{}",
        json!({
            "scenario": scenario,
            "mode": mode,
            "runs": RUNS,
            "rows": rows,
            "batches": batches,
            "median_seconds": median,
            "min_seconds": min,
            "max_seconds": max,
            "rows_per_second": rate,
        })
    );
    (median, min, max)
}

/// Sovrapposizione delle bande [min, max] di due modalita' (accettazione
/// D14.10 stampata, non bloccante).
fn bands_overlap(a: (f64, f64, f64), b: (f64, f64, f64)) -> bool {
    !(a.2 < b.1 || b.2 < a.1)
}

// ---------------------------------------------------------------------------
// Accesso tipizzato alle colonne (oracolo cross-path)
// ---------------------------------------------------------------------------

fn uint64_column<'a>(batch: &'a RecordBatch, index: usize, case: &str) -> &'a UInt64Array {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap_or_else(|| panic!("{case}: colonna {index} non UInt64"))
}

fn int64_column<'a>(batch: &'a RecordBatch, index: usize, case: &str) -> &'a Int64Array {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap_or_else(|| panic!("{case}: colonna {index} non Int64"))
}

fn boolean_column<'a>(batch: &'a RecordBatch, index: usize, case: &str) -> &'a BooleanArray {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<BooleanArray>()
        .unwrap_or_else(|| panic!("{case}: colonna {index} non Boolean"))
}

/// Coppie (left, right) dal lineage v3 (colonne `__left_index`,
/// `__right_index`).
fn v3_pairs(batches: &[RecordBatch]) -> Vec<(u64, u64)> {
    let mut pairs = Vec::new();
    for batch in batches {
        let left = uint64_column(batch, 0, "sjoin-v3");
        let right = uint64_column(batch, 1, "sjoin-v3");
        for row in 0..batch.num_rows() {
            pairs.push((left.value(row), right.value(row)));
        }
    }
    pairs
}

/// Coppie (left, right) dall'output v4 di `geo.sjoin`: colonna `id` (=
/// indice di riga left della fixture, per costruzione) + `__right_index`.
fn v4_sjoin_pairs(batches: &[RecordBatch]) -> Vec<(u64, u64)> {
    let mut pairs = Vec::new();
    for batch in batches {
        let ids = int64_column(batch, 0, "sjoin-v4");
        let right = uint64_column(batch, 2, "sjoin-v4");
        for row in 0..batch.num_rows() {
            pairs.push((
                u64::try_from(ids.value(row)).expect("id fixture non negativo"),
                right.value(row),
            ));
        }
    }
    pairs
}

/// Vettore di flag (nullable) allineato alle righe left, colonna `index`
/// (v3: `within` del protocollo; v4: nome dal contratto — confronto sui
/// valori, D14.8).
fn boolean_flags(batches: &[RecordBatch], index: usize, case: &str) -> Vec<Option<bool>> {
    let mut flags = Vec::new();
    for batch in batches {
        let column = boolean_column(batch, index, case);
        for row in 0..batch.num_rows() {
            flags.push((!column.is_null(row)).then(|| column.value(row)));
        }
    }
    flags
}

// ---------------------------------------------------------------------------
// Scenari geo A/B
// ---------------------------------------------------------------------------

fn binary_plan(op: &str, config: &Value) -> Value {
    json!({
        "schema_version": 4,
        "inputs": ["left_in", "right_in"],
        "nodes": [
            {"id": "j", "op": op, "in": ["left_in", "right_in"], "config": config},
        ],
        "output": "j",
    })
}

/// Descrizione di uno scenario geo A/B (sjoin/within differiscono per
/// config, schema v3, lato right e forma dell'oracolo cross-path).
struct GeoScenario {
    /// Nome nelle righe JSON.
    name: &'static str,
    /// Operazione (`geo.sjoin` / `geo.within`): seleziona l'oracolo
    /// cross-path (coppie vs flag).
    op: &'static str,
    /// Config del nodo v4.
    v4_config: Value,
    /// Lato del quadrato right (20 per sjoin, 60 per within — vedi header).
    right_square_side: f64,
    /// Nota di sanita' (ordine di grandezza atteso a mano, mai soglia).
    sanity_expected: &'static str,
}

fn v3_geo_schema(scenario: &GeoScenario) -> PairArrowSchema {
    let mut definition = json!({
        "schema_version": 3,
        "operation": scenario.op,
        "left_row_count": GEO_ROWS,
        "right_row_count": GEO_ROWS,
        "left_crs": "EPSG:32632",
        "right_crs": "EPSG:32632",
        "geometry_column": "geom",
        "max_pairs": V3_MAX_PAIRS,
    });
    if scenario.op == "sjoin" {
        definition["predicate"] = json!("intersects");
    }
    serde_json::from_value(definition).expect("schema v3 valido")
}

/// Esito delle run alternate di uno scenario geo: durate per modalita' e
/// output di riferimento (prima run) per l'oracolo cross-path.
struct AlternatedRuns {
    durations_v3: Vec<f64>,
    durations_v4: Vec<f64>,
    output_v3: Vec<RecordBatch>,
    output_v4: Vec<RecordBatch>,
}

/// Le RUNS run per modalita' alternate A,B,A,B... con oracolo cross-run
/// (serializzazione IPC byte-per-byte tra run della stessa modalita',
/// ADR-0001). La mediana di 5 per modalita' non dipende dall'ordine e un
/// eventuale drift termico/di cache pesa su entrambe le modalita'.
fn run_geo_alternated(
    scenario: &GeoScenario,
    graph: &ValidatedGraph,
    left: &[RecordBatch],
    right: &[RecordBatch],
    left_envelope: &[u8],
    right_envelope: &[u8],
    v3_schema: &PairArrowSchema,
) -> AlternatedRuns {
    let mut runs = AlternatedRuns {
        durations_v3: Vec::with_capacity(RUNS),
        durations_v4: Vec::with_capacity(RUNS),
        output_v3: Vec::new(),
        output_v4: Vec::new(),
    };
    let mut reference_v3: Option<Vec<u8>> = None;
    let mut reference_v4: Option<Vec<u8>> = None;
    for run in 0..RUNS * 2 {
        if run % 2 == 0 {
            let (output, seconds) = run_v4_timed(graph, left, right);
            runs.durations_v4.push(seconds);
            let serialized = ipc_bytes(&output);
            if let Some(reference) = &reference_v4 {
                assert_eq!(
                    *reference, serialized,
                    "{}: output v4 diverso tra run (ADR-0001)",
                    scenario.name
                );
            } else {
                reference_v4 = Some(serialized);
                runs.output_v4 = output;
            }
        } else {
            let (output, seconds) = run_v3_timed(left_envelope, right_envelope, v3_schema);
            runs.durations_v3.push(seconds);
            // La decodifica per l'oracolo e' fuori timing (perimetro
            // dichiarato); la serializzazione e' uniforme tra le modalita'.
            let decoded = decode_v3_output(&output);
            let serialized = ipc_bytes(&decoded);
            if let Some(reference) = &reference_v3 {
                assert_eq!(
                    *reference, serialized,
                    "{}: output v3 diverso tra run (ADR-0001)",
                    scenario.name
                );
            } else {
                reference_v3 = Some(serialized);
                runs.output_v3 = decoded;
            }
        }
    }
    runs
}

/// Oracolo cross-path semantico (D14.8: schemi diversi, MAI byte) —
/// ritorna la nota di sanita' osservata per la sintesi.
fn assert_cross_path(
    scenario: &GeoScenario,
    output_v3: &[RecordBatch],
    output_v4: &[RecordBatch],
) -> Value {
    match scenario.op {
        "sjoin" => {
            let pairs_v3 = v3_pairs(output_v3);
            let pairs_v4 = v4_sjoin_pairs(output_v4);
            assert_eq!(
                pairs_v3, pairs_v4,
                "{}: coppie v3/v4 diverse (ordine canonico D14.7)",
                scenario.name
            );
            assert!(
                !pairs_v3.is_empty(),
                "{}: fixture degenere — nessuna coppia, oracolo vacuo",
                scenario.name
            );
            json!({"sanity_pairs": pairs_v3.len()})
        }
        "within" => {
            let flags_v3 = boolean_flags(output_v3, 2, "within-v3");
            let flags_v4 = boolean_flags(output_v4, 2, "within-v4");
            assert_eq!(
                flags_v3, flags_v4,
                "{}: flag v3/v4 diversi (allineati alle righe left)",
                scenario.name
            );
            let flags_true = flags_v3.iter().filter(|flag| **flag == Some(true)).count();
            assert!(
                flags_true > 0,
                "{}: fixture degenere — nessun flag true, oracolo vacuo",
                scenario.name
            );
            json!({"sanity_flags_true": flags_true})
        }
        other => unreachable_op(other),
    }
}

/// Uno scenario geo A/B completo: RUNS run per modalita' alternate
/// A,B,A,B..., oracolo cross-run (IPC byte-per-byte) e cross-path
/// (semantico), due righe JSON di modalita' e la sintesi.
fn run_geo_scenario(scenario: &GeoScenario) {
    // Costruzione fuori timing (perimetro dichiarato): fixture, envelope
    // v3, grafo validato.
    let left = geo_left_batches();
    let right = geo_right_batches(scenario.right_square_side);
    let left_envelope = encode_envelope(&left);
    let right_envelope = encode_envelope(&right);
    let v3_schema = v3_geo_schema(scenario);
    let graph = validate(
        &binary_plan(&format!("geo.{}", scenario.op), &scenario.v4_config).to_string(),
        &[
            ("left_in".to_owned(), geo_contract()),
            ("right_in".to_owned(), geo_contract()),
        ],
    )
    .expect("validate");

    let mut runs = run_geo_alternated(
        scenario,
        &graph,
        &left,
        &right,
        &left_envelope,
        &right_envelope,
        &v3_schema,
    );
    let sanity_observed = assert_cross_path(scenario, &runs.output_v3, &runs.output_v4);

    let input_rows = GEO_ROWS * 2;
    let input_batches = GEO_ROWS.div_ceil(GEO_BATCH_ROWS) * 2;
    let band_v3 = report(
        scenario.name,
        "v3_cli_pair_arrow",
        input_rows,
        input_batches,
        &mut runs.durations_v3,
    );
    let band_v4 = report(
        scenario.name,
        "v4_plan",
        input_rows,
        input_batches,
        &mut runs.durations_v4,
    );
    let delta = (band_v4.0 - band_v3.0) / band_v3.0 * 100.0;
    println!(
        "{}",
        json!({
            "scenario": scenario.name,
            "synthesis": true,
            "delta_percent": delta,
            "bands_overlap": bands_overlap(band_v3, band_v4),
            "oracle_cross_run": true,
            "oracle_cross_path": true,
            "sanity_expected": scenario.sanity_expected,
            "sanity_observed": sanity_observed,
        })
    );
}

/// Selettore di oracolo: i due op del benchmark sono coperti dai bracci
/// sopra; un nome diverso e' un errore di configurazione del bench, non un
/// dato (gli esempi non sono sotto il gate R6 — il panic e' deliberato).
fn unreachable_op(op: &str) -> ! {
    panic!("op di scenario non gestita: {op}")
}

// ---------------------------------------------------------------------------
// Controllo di non regressione table.* (solo v4)
// ---------------------------------------------------------------------------

/// `table_join_control`: join inner 1:1 su `id` di due tabelle da 1M righe
/// sul guscio binario condiviso (dopo lo smistamento D14.2 sul
/// `PreparedConfig`, il ramo tabellare non deve regredire). Riferimento
/// storico citato, non soglia: `benchmarks/baseline/baseline.md` par. 1
/// (0,6361s nello stesso container).
fn run_table_join_control() {
    let left = table_batches(7);
    let right = table_batches(13);
    let plan = json!({
        "schema_version": 4,
        "inputs": ["left_in", "right_in"],
        "nodes": [
            {"id": "j", "op": "table.join", "in": ["left_in", "right_in"],
             "config": {"left_keys": ["id"], "right_keys": ["id"]}},
        ],
        "output": "j",
    });
    let graph = validate(
        &plan.to_string(),
        &[
            ("left_in".to_owned(), DataContract::tabular(table_schema())),
            ("right_in".to_owned(), DataContract::tabular(table_schema())),
        ],
    )
    .expect("validate");

    let mut durations = Vec::with_capacity(RUNS);
    let mut reference: Option<Vec<u8>> = None;
    let mut output_rows = 0_usize;
    for _run in 0..RUNS {
        let (batches, seconds) = run_v4_timed(&graph, &left, &right);
        durations.push(seconds);
        let serialized = ipc_bytes(&batches);
        if let Some(reference) = &reference {
            assert_eq!(
                *reference, serialized,
                "table_join_control: output diverso tra run (ADR-0001)"
            );
        } else {
            reference = Some(serialized);
            output_rows = batches.iter().map(RecordBatch::num_rows).sum();
        }
    }
    assert_eq!(
        output_rows, TABLE_ROWS,
        "table_join_control: join 1:1 atteso (righe out = righe per lato)"
    );

    let band = report(
        "table_join_control",
        "v4_plan",
        TABLE_ROWS * 2,
        TABLE_ROWS.div_ceil(TABLE_BATCH_ROWS) * 2,
        &mut durations,
    );
    println!(
        "{}",
        json!({
            "scenario": "table_join_control",
            "synthesis": true,
            "median_seconds": band.0,
            "output_rows": output_rows,
            "oracle_cross_run": true,
            "baseline_reference_seconds": 0.6361,
            "baseline_note": "riferimento storico benchmarks/baseline/baseline.md par. 1 (stesso container rust:1.92, sensibile all'host — non una soglia)",
        })
    );
}

fn main() {
    run_geo_scenario(&GeoScenario {
        name: "sjoin",
        op: "sjoin",
        v4_config: json!({"predicate": "intersects"}),
        right_square_side: 20.0,
        sanity_expected:
            "49_950 coppie (circa meta' delle righe left: solo i quadrati, meno le 50 right null della regione condivisa — vedi header)",
    });
    run_geo_scenario(&GeoScenario {
        name: "within",
        op: "within",
        v4_config: json!({}),
        right_square_side: 60.0,
        sanity_expected: "49_900 flag true (tutti i punti non null: le right null della regione condivisa coincidono con celle a punto null — vedi header; right 60x60)",
    });
    run_table_join_control();
}
