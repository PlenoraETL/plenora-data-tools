//! Oracolo esteso agli errori dei binari geo nel piano v4 contro il
//! trasporto CLI `pair-arrow` v3 (ADR-0014 D14.9, gate di M2): per i quattro
//! op del perimetro (`geo.sjoin`, `geo.nearest`, `geo.within`,
//! `geo.count_points_in_polygons`) lo stesso input produce sui due percorsi
//! lo stesso risultato — confronto SEMANTICO contro attesi codificati nel
//! test (coppie, flag, conteggi e attributi letterali in ordine canonico
//! D14.7 left-major/right-minor, cosi' l'uguaglianza vettoriale verifica
//! anche l'ordine) — oppure lo stesso errore: testo base identico (v3
//! `ArrowTransportError` vs v4 `Execution` al nodo `j` con fase `Read` e
//! dettaglio diagnostico `side=… row=…`, D14.5). Gli schemi di output v3/v4
//! differiscono per contratto (D14.8: lineage indici v3 vs left passthrough
//! + colonne derivate v4) — mai confrontati byte-per-byte.
//!
//! Mappa di copertura dei casi obbligatori D14.9:
//!
//! - (a) percorso felice multi-tipo per i 4 op: questo file
//!   (`a_*_happy_path`);
//! - (b) OGC-invalido (bowtie) su left e su right, inclusa una cella MAI
//!   candidata al prefilter (decode totale D14.3) e il primo errore in
//!   ordine (side, riga) (D14.5.3): questo file (`b_*`);
//! - (c) cella oltre `MAX_CELL_BYTES` su ciascun lato: questo file (`c_*`);
//! - (d) cancellazione in drenaggio e post-drenaggio (solo v4: il trasporto
//!   v3 non ha un token di cancellazione — non c'e' un lato da confrontare):
//!   questo file (`d1_*`, `d2_*`);
//! - (e) panic iniettato via hook: `crates/plenora-engine/src/executor/tests.rs`
//!   (`e_geo_binary_kernel_panic_is_attributed_to_the_node`) — l'hook
//!   `PANIC_AT_NODES` e' `#[cfg(test)]` e privato del crate, irraggiungibile
//!   da un test di integrazione senza modificare il codice di produzione;
//! - (f) espansione oltre il vincolo: questo file (`f_*`);
//! - (g) governor che rifiuta la reservation decodificata (condizione di
//!   attivazione del perimetro, ruolo DER-003): questo file (`g_*`);
//! - (h) conservativita' di `decoded_size_xy` su corpus: test di modulo in
//!   `crates/plenora-kernels-geo/src/decoded_size.rs` (M2), non duplicato.
//!
//! Divergenze osservate rispetto alla lettera dell'ADR (regola 4 del
//! progetto: asserito il comportamento REALE, motivazione nel commento del
//! test; segnalate nel report di M2):
//!
//! 1. (f)/(g) gli errori di `check_join_expansion` (ADR 6) e del governor
//!    (`reserve`, ADR-0002) arrivano al chiamante come `InvalidPlan` GREZZO
//!    — nodo/owner nel testo, fase `Validate` derivata dalla variante — NON
//!    come `Execution { node: "j" }` della lettera di D14.5.1: propagano via
//!    `?` dal guscio di `run_geo_binary_blocking`, mai dal carrier
//!    `GeoBinaryStepError`. Stessa forma del ramo tabellare e del test
//!    governor DER-003 di ADR-0012: la divergenza e' di attribuzione, non di
//!    rifiuto (fail-closed in entrambe le forme);
//! 2. (f) i due meccanismi NON sono omologhi: il v4 applica il vincolo
//!    RELATIVO di catalogo (`MaxRelative` per `geo.sjoin`, ADR 6), il v3 non
//!    ha alcun vincolo di espansione — si usa il suo tetto assoluto
//!    `max_output_rows` (`OutputRowsExceeded`): stesso output rifiutato
//!    fail-closed, asserzioni separate per lato;
//! 3. (c) il gate `MAX_CELL_BYTES` del NODO (camminata condivisa
//!    `decode_geometry_batches`, D14.2) e' IRRAGGIUNGIBILE da un arco di
//!    input nel v4: la validazione perimetrale dell'arco
//!    (`validate_wkb_cells`) rifiuta la cella oltre 64 MiB PRIMA del nodo
//!    (`max_wkb_cell_bytes`, stessa soglia di 64 MiB; con
//!    `max_wkb_cell_bytes` alzato interviene il tetto strutturale interno
//!    del validatore, sempre 64 MiB; con `max_batch_bytes` di default
//!    interviene ancora prima il tetto batch) — `InvalidPlan` d'arco senza
//!    nodo e fase `Validate`, contro il `CellTooLarge` del v3 (il trasporto
//!    non ha validazione d'arco: la cella arriva al decode condiviso).
//!    Entrambi i percorsi rifiutano fail-closed la stessa cella alla stessa
//!    soglia per-cella; variante, attribuzione e misura riportata (limite vs
//!    byte della cella) differiscono — asserzioni separate per lato. Il
//!    gate del nodo resta difesa in profondita' (anche gli archi intermedi
//!    non possono portare celle oltre soglia: ogni kernel geo valida il
//!    proprio output, ADR-0012 D12.3);
//! 4. (d) la fase osservata degli errori `Cancelled` e' `Write` per
//!    derivazione di variante (`PlenoraError::phase`): la cancellazione non
//!    e' taggata di fase ai confini dell'executor.

use std::io::Cursor;
use std::sync::Arc;

use geo::{polygon, Geometry, LineString, MultiLineString, Point, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_core::{ErrorCategory, ErrorPhase, PlenoraError, RemoteEffect, RetryDisposition};
use plenora_engine::geo_transport::transport::{
    decode_ipc, encode_ipc, pair_arrow, preflight_decoded_bytes, ArrowTransportError,
    EnvelopeReader, EnvelopeWriter, PairArrowSchema, COUNT_COLUMN, DISTANCE_COLUMN,
    LEFT_INDEX_COLUMN, MAX_CELL_BYTES, RIGHT_INDEX_COLUMN, WITHIN_COLUMN,
};
use plenora_engine::planner::{validate, ValidatedGraph};
use plenora_engine::{
    execute, BatchTarget, CancellationToken, ExecutionMetrics, Input, Inputs, Output,
    RuntimeContext,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Tetti assoluti D14.6 risolti dalla prepare (fonte delle costanti)
// ---------------------------------------------------------------------------

/// Tetto coppie/risultati risolto dalla prepare per un piano a nodo singolo
/// con `output = "j"` (D14.6): il `max_output_rows` effettivo. Fonte:
/// `RowLimits::default` in `plenora-core/src/limits.rs` (10^7 per
/// `max_input_rows`/`max_output_rows`/`max_rows_per_edge`), risoluzione in
/// `prepare_geo_binary` (`crates/plenora-engine/src/prepare.rs`,
/// `row_cap = max_output_rows` perche' il nodo produce l'output del piano).
/// Uguale a `MAX_PAIRS` del protocollo coppie (10^7): gli schemi v3 del caso
/// (a) usano gli STESSI valori risolti dal v4, come richiesto dal caso.
const RESOLVED_MAX_PAIRS: u64 = 10_000_000;

/// `geo.nearest`: come `RESOLVED_MAX_PAIRS` (`max_results = row_cap`).
const RESOLVED_MAX_RESULTS: u64 = RESOLVED_MAX_PAIRS;

/// `geo.nearest`: quadrato del massimo tra `max_input_rows` e
/// `max_rows_per_edge` (entrambi 10^7 di default, stessa fonte sopra).
const RESOLVED_MAX_COMPARISONS: u64 = 100_000_000_000_000;

// ---------------------------------------------------------------------------
// Fixture (stessa forma degli altri test geo: colonna `id` + WKB XY EPSG:32632)
// ---------------------------------------------------------------------------

fn geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        plenora_kernels_geo::arrow_adapter::geometry_output_field("geom", "EPSG:32632")
            .expect("campo geometria"),
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

fn to_wkb(geometry: &Geometry<f64>) -> Vec<u8> {
    geometry.to_wkb(CoordDimensions::xy()).expect("wkb fixture")
}

fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    to_wkb(&Geometry::Point(Point::new(x, y)))
}

/// Quadrato allineato agli assi (min corner, max corner).
fn square_wkb(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<u8> {
    to_wkb(&Geometry::Polygon(polygon![
        (x: min_x, y: min_y),
        (x: max_x, y: min_y),
        (x: max_x, y: max_y),
        (x: min_x, y: max_y),
        (x: min_x, y: min_y),
    ]))
}

/// Poligono con buco (30,30)-(50,50), buco (35,35)-(45,45): il buco e'
/// strettamente interno, la geometria e' OGC-valida; un punto nel buco NON
/// interseca ne' e' contenuto nel poligono — la fixture misura che il buco
/// conti davvero nella topologia dei kernel.
fn holed_polygon_wkb() -> Vec<u8> {
    to_wkb(&Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (30.0, 30.0),
            (50.0, 30.0),
            (50.0, 50.0),
            (30.0, 50.0),
            (30.0, 30.0),
        ]),
        vec![LineString::from(vec![
            (35.0, 35.0),
            (45.0, 35.0),
            (45.0, 45.0),
            (35.0, 45.0),
            (35.0, 35.0),
        ])],
    )))
}

/// WKB di poligono a farfalla (anello auto-intersecante): strutturalmente
/// ben formato (anello chiuso, coordinate finite) ma OGC-invalido — supera la
/// validazione strutturale dell'arco di input e fallisce al check OGC del
/// decode del nodo (`geometry_from_wkb`, ADR-0011). Stessa costruzione della
/// fixture dell'oracolo ADR-0012.
fn bowtie_wkb(origin_x: f64, origin_y: f64) -> Vec<u8> {
    to_wkb(&Geometry::Polygon(polygon![
        (x: origin_x, y: origin_y),
        (x: origin_x + 10.0, y: origin_y + 10.0),
        (x: origin_x + 10.0, y: origin_y),
        (x: origin_x, y: origin_y + 10.0),
        (x: origin_x, y: origin_y),
    ]))
}

/// Cella WKB oltre `MAX_CELL_BYTES` (64 MiB) ma strutturalmente e
/// OGC-valida: `MultiLineString` di 1000 parti da 4194 coordinate distinte
/// (~67 MiB). Linee e non poligoni: la validazione OGC dei poligoni e'
/// appaiata O(n^2) e renderebbe la fixture troppo lenta in debug, le linee
/// si validano in O(n) — stessa scelta di `oversized_cell_batches`
/// dell'oracolo ADR-0012. Le parti sono disgiunte (fascia y di 1 ogni 10) e
/// monotone: nessuna auto-intersezione.
fn oversized_cell_wkb() -> Vec<u8> {
    let lines: Vec<LineString<f64>> = (0..1_000_u32)
        .map(|index| {
            let base = f64::from(index) * 10.0;
            LineString::from(
                (0..4_194_u32)
                    .map(|step| (0.0, base + f64::from(step) / 4_193.0))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    to_wkb(&Geometry::MultiLineString(MultiLineString::new(lines)))
}

/// `LineString` monotona di `points` coordinate distinte (x strettamente
/// crescente: nessuna auto-intersezione possibile, OGC-valida per
/// costruzione) — ~16 byte per coordinata in WKB e altrettanti nella forma
/// decodificata piu' gli overhead dei `Vec`: fixture di (g), dove serve un
/// lato la cui reservation decodificata domini il budget. Linea e non
/// poligono per la stessa ragione di [`oversized_cell_wkb`] (validazione
/// OGC in O(n)).
fn big_linestring_wkb(points: u32) -> Vec<u8> {
    to_wkb(&Geometry::LineString(LineString::from(
        (0..points)
            .map(|index| (f64::from(index), f64::from(index % 7)))
            .collect::<Vec<_>>(),
    )))
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

// ---------------------------------------------------------------------------
// Infrastruttura lato v4 (plan executor)
// ---------------------------------------------------------------------------

fn binary_plan(op: &str, config: &Value) -> Value {
    json!({
        "schema_version": 5,
        "inputs": ["left_in", "right_in"],
        "nodes": [
            {"id": "j", "op": op, "in": ["left_in", "right_in"], "config": config},
        ],
        "output": "j",
    })
}

fn graph(plan: &Value) -> ValidatedGraph {
    validate(
        &plan.to_string(),
        &[
            ("left_in".to_owned(), geo_contract()),
            ("right_in".to_owned(), geo_contract()),
        ],
    )
    .expect("piano valido")
}

// Percorso permissivo (`Inputs::with`), deprecato ma ancora supportato:
// questi oracoli non dichiarano contratti e ne coprono il comportamento.
#[allow(deprecated)]
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

/// Diagnostica attiva (ADR 3 M1d): il dettaglio strutturato `side=… row=…`
/// di D14.5.2 esiste solo in questo canale (mai nel testo base, regola 8).
fn runtime() -> RuntimeContext {
    RuntimeContext {
        diagnostics: true,
        ..RuntimeContext::default()
    }
}

/// Drena lo stream fino al primo errore (gli eventuali batch precedenti sono
/// gia' stati pubblicati nel canale, come nel consumo reale).
fn first_stream_error(output: &mut Output) -> Option<PlenoraError> {
    for item in output.by_ref() {
        if let Err(error) = item {
            return Some(error);
        }
    }
    None
}

fn run_ok_v4(
    plan: &Value,
    left: Vec<RecordBatch>,
    right: Vec<RecordBatch>,
) -> (Vec<RecordBatch>, ExecutionMetrics) {
    execute(&graph(plan), two_geo_inputs(left, right), runtime())
        .expect("execute")
        .collect_batches()
        .expect("stream ok")
}

fn run_err_v4(plan: &Value, left: Vec<RecordBatch>, right: Vec<RecordBatch>) -> PlenoraError {
    let mut output =
        execute(&graph(plan), two_geo_inputs(left, right), runtime()).expect("execute");
    first_stream_error(&mut output).expect("atteso un errore, lo stream e' terminato con successo")
}

/// Forma dell'errore v4 confrontata dall'oracolo: variante, nodo,
/// operazione, motivo (con il suffisso diagnostico `side=… row=…`),
/// categoria e fase. `execution_id` e' escluso PER COSTRUZIONE (UUID nuovo a
/// ogni esecuzione, non un osservabile confrontabile). Stessa forma di
/// `ErrorSignature` dell'oracolo ADR-0012.
#[derive(Debug, PartialEq, Eq)]
struct ErrorSignature {
    variant: &'static str,
    node: Option<String>,
    operation: Option<String>,
    reason: String,
    category: ErrorCategory,
    phase: ErrorPhase,
    remote_effect: RemoteEffect,
    retry: RetryDisposition,
}

fn error_signature(error: &PlenoraError) -> ErrorSignature {
    let (variant, node, operation, reason) = match error {
        PlenoraError::InvalidPlan(reason)
        | PlenoraError::Unsupported(reason)
        | PlenoraError::Schema(reason)
        | PlenoraError::DataMapping(reason)
        | PlenoraError::Crs(reason)
        | PlenoraError::ResourceLimit(reason)
        | PlenoraError::Internal(reason) => (variant_name(error), None, None, reason.clone()),
        PlenoraError::Execution {
            node,
            operation,
            reason,
            ..
        }
        | PlenoraError::Cancelled {
            node,
            operation,
            reason,
            ..
        } => (
            variant_name(error),
            Some(node.clone()),
            Some(operation.clone()),
            reason.clone(),
        ),
        PlenoraError::Io(error) => ("Io", None, None, error.to_string()),
        PlenoraError::Replayed(replayed) => (
            "Replayed",
            replayed.node.clone(),
            replayed.operation.clone(),
            replayed.message.clone(),
        ),
        // Wrapper di fase: la firma vede la variante interna; la fase
        // (taggata) e' letta sotto da `error.phase()`.
        PlenoraError::Tagged { source, .. } | PlenoraError::RowDiagnostics { source, .. } => {
            let inner = error_signature(source);
            return ErrorSignature {
                phase: error.phase(),
                ..inner
            };
        }
    };
    ErrorSignature {
        variant,
        node,
        operation,
        reason,
        category: error.category(),
        phase: error.phase(),
        remote_effect: error.remote_effect(),
        retry: error.retry_disposition(),
    }
}

const fn variant_name(error: &PlenoraError) -> &'static str {
    match error {
        PlenoraError::InvalidPlan(_) => "InvalidPlan",
        PlenoraError::Unsupported(_) => "Unsupported",
        PlenoraError::Schema(_) => "Schema",
        PlenoraError::DataMapping(_) => "DataMapping",
        PlenoraError::Execution { .. } => "Execution",
        PlenoraError::Crs(_) => "Crs",
        PlenoraError::Cancelled { .. } => "Cancelled",
        PlenoraError::Io(_) => "Io",
        PlenoraError::ResourceLimit(_) => "ResourceLimit",
        PlenoraError::Internal(_) => "Internal",
        PlenoraError::Replayed(_) => "Replayed",
        PlenoraError::Tagged { source, .. } | PlenoraError::RowDiagnostics { source, .. } => {
            variant_name(source)
        }
    }
}

// ---------------------------------------------------------------------------
// Infrastruttura lato v3 (trasporto pair-arrow)
// ---------------------------------------------------------------------------

/// Envelope v3 (`PLNGEO3`) attorno al payload IPC dei batch — la stessa
/// codifica che la CLI `pair-arrow` legge da file.
fn encode_envelope(batches: &[RecordBatch]) -> Vec<u8> {
    let payload = encode_ipc(&batches[0].schema(), batches).expect("payload IPC");
    let mut bytes = Vec::new();
    let mut writer = EnvelopeWriter::new(&mut bytes, payload.len() as u64).expect("envelope");
    writer.write_payload(&payload).expect("scrittura payload");
    writer.finish().expect("chiusura envelope");
    bytes
}

fn v3_schema(definition: Value) -> PairArrowSchema {
    serde_json::from_value(definition).expect("schema v3 valido")
}

/// Pipeline v3: due envelope -> `pair_arrow` -> envelope di output
/// decodificato (stessa camminata della CLI, senza processo).
fn run_v3(
    schema: &PairArrowSchema,
    left: &[RecordBatch],
    right: &[RecordBatch],
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let left_envelope = encode_envelope(left);
    let right_envelope = encode_envelope(right);
    let mut output = Vec::new();
    pair_arrow(
        Cursor::new(left_envelope),
        Cursor::new(right_envelope),
        &mut output,
        schema,
    )?;
    let payload = EnvelopeReader::new(Cursor::new(output))?.read_payload()?;
    decode_ipc(&payload)
}

// ---------------------------------------------------------------------------
// Accesso tipizzato alle colonne (label del caso nei messaggi)
// ---------------------------------------------------------------------------

fn int64_column<'a>(batch: &'a RecordBatch, index: usize, case: &str) -> &'a Int64Array {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap_or_else(|| panic!("{case}: colonna {index} non Int64"))
}

fn uint64_column<'a>(batch: &'a RecordBatch, index: usize, case: &str) -> &'a UInt64Array {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .unwrap_or_else(|| panic!("{case}: colonna {index} non UInt64"))
}

fn float64_column<'a>(batch: &'a RecordBatch, index: usize, case: &str) -> &'a Float64Array {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap_or_else(|| panic!("{case}: colonna {index} non Float64"))
}

fn boolean_column<'a>(batch: &'a RecordBatch, index: usize, case: &str) -> &'a BooleanArray {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<BooleanArray>()
        .unwrap_or_else(|| panic!("{case}: colonna {index} non Boolean"))
}

fn binary_column<'a>(batch: &'a RecordBatch, index: usize, case: &str) -> &'a BinaryArray {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap_or_else(|| panic!("{case}: colonna {index} non Binary"))
}

// ---------------------------------------------------------------------------
// (a) Percorso felice multi-tipo — fixture condivisa
// ---------------------------------------------------------------------------

/// Ids left della fixture condivisa (indice di riga = posizione nel vettore,
/// in ordine di batch: il v4 concatena i due batch in quest'ordine e gli
/// indici di coppia sono globali sulla sequenza concatenata).
const LEFT_IDS: [i64; 6] = [0, 1, 2, 3, 4, 5];

/// Fixture multi-tipo (punti, poligoni — uno con buco — e null su entrambi
/// i lati, due batch per lato) con attesi calcolabili a mano per tutti e
/// quattro gli op:
///
/// - left: `Point(1,1)`, quadrato (10,10)-(20,20), poligono bucato
///   (30,30)-(50,50) con buco (35,35)-(45,45), null, `Point(17,16)`,
///   quadrato (200,200)-(210,210);
/// - right: quadrato (0,0)-(3,3), `Point(12,12)`, null, quadrato
///   (15,15)-(25,25), `Point(37,37)` (nel BUCO del poligono left: non
///   interseca ne' e' contenuto), quadrato (190,190)-(220,220).
///
/// Restituisce i batch dei due lati e le celle left appiattite (per il
/// confronto del passthrough/take della geometria).
fn shared_fixture() -> (Vec<RecordBatch>, Vec<RecordBatch>, Vec<Option<Vec<u8>>>) {
    let left_cells = vec![
        Some(point_wkb(1.0, 1.0)),
        Some(square_wkb(10.0, 10.0, 20.0, 20.0)),
        Some(holed_polygon_wkb()),
        None,
        Some(point_wkb(17.0, 16.0)),
        Some(square_wkb(200.0, 200.0, 210.0, 210.0)),
    ];
    let right_cells = [
        Some(square_wkb(0.0, 0.0, 3.0, 3.0)),
        Some(point_wkb(12.0, 12.0)),
        None,
        Some(square_wkb(15.0, 15.0, 25.0, 25.0)),
        Some(point_wkb(37.0, 37.0)),
        Some(square_wkb(190.0, 190.0, 220.0, 220.0)),
    ];
    let left = vec![
        geo_batch(&LEFT_IDS[..3], &left_cells[..3]),
        geo_batch(&LEFT_IDS[3..], &left_cells[3..]),
    ];
    let right = vec![
        geo_batch(&[10, 11, 12], &right_cells[..3]),
        geo_batch(&[13, 14, 15], &right_cells[3..]),
    ];
    (left, right, left_cells)
}

/// Verifica il passthrough delle colonne left (id in ordine di riga,
/// geometria identica alla cella WKB di origine) su un output allineato al
/// left (within/count, v3 o v4).
fn assert_left_passthrough(case: &str, batches: &[RecordBatch], left_cells: &[Option<Vec<u8>>]) {
    let mut row = 0_usize;
    for batch in batches {
        let ids = int64_column(batch, 0, case);
        let geometries = binary_column(batch, 1, case);
        for batch_row in 0..batch.num_rows() {
            assert_eq!(
                ids.value(batch_row),
                LEFT_IDS[row],
                "{case}: id left alla riga {row}"
            );
            assert_eq!(
                geometries.is_null(batch_row),
                left_cells[row].is_none(),
                "{case}: null geometria alla riga {row}"
            );
            if let Some(cell) = &left_cells[row] {
                assert_eq!(
                    geometries.value(batch_row),
                    cell.as_slice(),
                    "{case}: geometria passthrough alla riga {row}"
                );
            }
            row += 1;
        }
    }
    assert_eq!(row, LEFT_IDS.len(), "{case}: righe left in uscita");
}

/// (a) `geo.sjoin` (predicato `intersects`): coppie attese in ordine
/// canonico D14.7 — il punto (1,1) interseca il quadrato (0,0)-(3,3); il
/// quadrato (10,10)-(20,20) contiene il punto (12,12) e si sovrappone al
/// quadrato (15,15)-(25,25); il punto (17,16) e' dentro (15,15)-(25,25); il
/// quadrato (200,200)-(210,210) e' dentro (190,190)-(220,220). Il punto
/// (37,37) nel BUCO non interseca il poligono bucato: nessuna coppia (2,*).
/// v4: take(left) + `__right_index` non-null; v3: batch lineage indici.
#[test]
fn a_sjoin_happy_path() {
    let case = "a-sjoin";
    let (left, right, left_cells) = shared_fixture();
    let expected_pairs: [(u64, u64); 5] = [(0, 0), (1, 1), (1, 3), (4, 3), (5, 5)];

    let plan = binary_plan("geo.sjoin", &json!({"predicate": "intersects"}));
    let (batches, metrics) = run_ok_v4(&plan, left.clone(), right.clone());
    assert_eq!(
        batches.len(),
        1,
        "{case} v4: output blocking a batch singolo"
    );
    let batch = &batches[0];
    assert_eq!(
        batch.num_rows(),
        expected_pairs.len(),
        "{case} v4: righe = coppie"
    );
    let ids = int64_column(batch, 0, case);
    let geometries = binary_column(batch, 1, case);
    let right_index = uint64_column(batch, 2, case);
    for (row, (left_row, right_row)) in expected_pairs.iter().enumerate() {
        // Indici di fixture letterali (<= 5): la conversione non tronca.
        let left_row = usize::try_from(*left_row).expect("indice left nella fixture");
        assert_eq!(
            ids.value(row),
            LEFT_IDS[left_row],
            "{case} v4: id left alla riga {row}"
        );
        assert_eq!(
            right_index.value(row),
            *right_row,
            "{case} v4: right_index alla riga {row}"
        );
        assert_eq!(
            geometries.value(row),
            left_cells[left_row]
                .as_deref()
                .expect("coppia con left non null"),
            "{case} v4: geometria take alla riga {row}"
        );
    }
    assert_eq!(
        right_index.null_count(),
        0,
        "{case} v4: inner join, right_index non-null"
    );
    assert_eq!(
        (metrics.nodes["j"].rows_in, metrics.nodes["j"].rows_out),
        (12, 5),
        "{case} v4: righe in = left + right, righe out = coppie"
    );

    let schema = v3_schema(json!({
        "schema_version": 3,
        "operation": "sjoin",
        "left_row_count": 6,
        "right_row_count": 6,
        "left_crs": "EPSG:32632",
        "right_crs": "EPSG:32632",
        "geometry_column": "geom",
        "predicate": "intersects",
        "max_pairs": RESOLVED_MAX_PAIRS,
    }));
    let (out_schema, v3_batches) = run_v3(&schema, &left, &right).expect("a-sjoin v3 ok");
    assert_eq!(
        out_schema.field(0).name(),
        LEFT_INDEX_COLUMN,
        "{case} v3: colonna lineage left"
    );
    assert_eq!(
        out_schema.field(1).name(),
        RIGHT_INDEX_COLUMN,
        "{case} v3: colonna lineage right"
    );
    assert_eq!(v3_batches.len(), 1, "{case} v3: batch lineage singolo");
    let v3_batch = &v3_batches[0];
    let v3_left = uint64_column(v3_batch, 0, case);
    let v3_right = uint64_column(v3_batch, 1, case);
    let pairs: Vec<(u64, u64)> = (0..v3_batch.num_rows())
        .map(|row| (v3_left.value(row), v3_right.value(row)))
        .collect();
    assert_eq!(
        pairs,
        expected_pairs.to_vec(),
        "{case} v3: coppie in ordine canonico D14.7 (left-major, right-minor)"
    );
}

/// Fixture dedicata a `geo.nearest` (due batch per lato, null su entrambi):
/// punti con distanze ESATTE per costruzione in f64 (2.0, 3.0, 5.0 — terne
/// pitagoriche intere), cosi' il confronto e' sull'uguaglianza esatta.
/// Separata dalla fixture multi-tipo: sovrapposizioni utili a sjoin/within/
/// count darebbero distanze 0.0 poco significative.
fn nearest_fixture() -> (Vec<RecordBatch>, Vec<RecordBatch>, Vec<Option<Vec<u8>>>) {
    let left_cells = vec![
        Some(point_wkb(0.0, 0.0)),
        Some(point_wkb(10.0, 0.0)),
        None,
        Some(point_wkb(3.0, 9.0)),
    ];
    let right_cells = [
        Some(point_wkb(3.0, 4.0)),
        None,
        Some(point_wkb(0.0, 2.0)),
        Some(point_wkb(10.0, 3.0)),
    ];
    let left = vec![
        geo_batch(&[0, 1], &left_cells[..2]),
        geo_batch(&[2, 3], &left_cells[2..]),
    ];
    let right = vec![
        geo_batch(&[10, 11], &right_cells[..2]),
        geo_batch(&[12, 13], &right_cells[2..]),
    ];
    (left, right, left_cells)
}

/// (a) `geo.nearest`: un match per riga left non null, il piu' vicino —
/// (0,0)→(0,2) distanza 2.0; (10,0)→(10,3) distanza 3.0; (3,9)→(3,4)
/// distanza 5.0; il null left non produce righe, i null right non sono
/// candidati. v4: take(left) + `__right_index` + `distance`; v3: lineage +
/// colonna distanza.
// Uguaglianza esatta sulle distanze: esatte per costruzione (terne
// pitagoriche intere, rappresentabili in f64).
#[allow(clippy::float_cmp)]
#[test]
fn a_nearest_happy_path() {
    let case = "a-nearest";
    let (left, right, left_cells) = nearest_fixture();
    let left_ids: [i64; 4] = [0, 1, 2, 3];
    let expected: [(u64, u64, f64); 3] = [(0, 2, 2.0), (1, 3, 3.0), (3, 0, 5.0)];

    let plan = binary_plan("geo.nearest", &json!({}));
    let (batches, metrics) = run_ok_v4(&plan, left.clone(), right.clone());
    assert_eq!(
        batches.len(),
        1,
        "{case} v4: output blocking a batch singolo"
    );
    let batch = &batches[0];
    assert_eq!(
        batch.num_rows(),
        expected.len(),
        "{case} v4: una riga per match"
    );
    let ids = int64_column(batch, 0, case);
    let geometries = binary_column(batch, 1, case);
    let right_index = uint64_column(batch, 2, case);
    let distances = float64_column(batch, 3, case);
    for (row, (left_row, right_row, distance)) in expected.iter().enumerate() {
        // Indici di fixture letterali (<= 3): la conversione non tronca.
        let left_row = usize::try_from(*left_row).expect("indice left nella fixture");
        assert_eq!(
            ids.value(row),
            left_ids[left_row],
            "{case} v4: id left alla riga {row}"
        );
        assert_eq!(
            right_index.value(row),
            *right_row,
            "{case} v4: right_index alla riga {row}"
        );
        assert_eq!(
            distances.value(row),
            *distance,
            "{case} v4: distanza alla riga {row}"
        );
        assert_eq!(
            geometries.value(row),
            left_cells[left_row]
                .as_deref()
                .expect("match con left non null"),
            "{case} v4: geometria take alla riga {row}"
        );
    }
    assert_eq!(
        (metrics.nodes["j"].rows_in, metrics.nodes["j"].rows_out),
        (8, 3),
        "{case} v4: righe in = left + right, righe out = match"
    );

    let schema = v3_schema(json!({
        "schema_version": 3,
        "operation": "nearest",
        "left_row_count": 4,
        "right_row_count": 4,
        "left_crs": "EPSG:32632",
        "right_crs": "EPSG:32632",
        "geometry_column": "geom",
        "max_comparisons": RESOLVED_MAX_COMPARISONS,
        "max_results": RESOLVED_MAX_RESULTS,
    }));
    let (out_schema, v3_batches) = run_v3(&schema, &left, &right).expect("a-nearest v3 ok");
    assert_eq!(
        out_schema.field(0).name(),
        LEFT_INDEX_COLUMN,
        "{case} v3: colonna lineage left"
    );
    assert_eq!(
        out_schema.field(1).name(),
        RIGHT_INDEX_COLUMN,
        "{case} v3: colonna lineage right"
    );
    assert_eq!(
        out_schema.field(2).name(),
        DISTANCE_COLUMN,
        "{case} v3: colonna distanza"
    );
    assert_eq!(v3_batches.len(), 1, "{case} v3: batch lineage singolo");
    let v3_batch = &v3_batches[0];
    let v3_left = uint64_column(v3_batch, 0, case);
    let v3_right = uint64_column(v3_batch, 1, case);
    let v3_distances = float64_column(v3_batch, 2, case);
    let matches: Vec<(u64, u64, f64)> = (0..v3_batch.num_rows())
        .map(|row| {
            (
                v3_left.value(row),
                v3_right.value(row),
                v3_distances.value(row),
            )
        })
        .collect();
    assert_eq!(
        matches,
        expected.to_vec(),
        "{case} v3: match in ordine canonico D14.7 (left-major, right-minor)"
    );
}

/// (a) `geo.within`: flag allineato alle righe left (left within QUALUNQUE
/// right) — true per il punto (1,1) nel quadrato (0,0)-(3,3), per il punto
/// (17,16) nel quadrato (15,15)-(25,25) e per il quadrato (200,200)-(210,210)
/// nel quadrato (190,190)-(220,220); false altrove; null su riga null. Il
/// punto (37,37) nel buco NON rende il poligono bucato within nulla. Gli
/// schemi v3/v4 differiscono solo per contratto (D14.8): la colonna flag e'
/// `WITHIN_COLUMN` nel v3, il nome dal contratto di output nel v4 (letto dal
/// grafo, fonte unica di verita' E1) — il confronto e' sui valori.
#[test]
fn a_within_happy_path() {
    let case = "a-within";
    let (left, right, left_cells) = shared_fixture();
    let expected_flags: [Option<bool>; 6] = [
        Some(true),
        Some(false),
        Some(false),
        None,
        Some(true),
        Some(true),
    ];

    let plan = binary_plan("geo.within", &json!({}));
    let validated = graph(&plan);
    let flag_name = validated
        .output_contract()
        .expect("contratto di output")
        .schema
        .fields()
        .last()
        .expect("colonna flag nel contratto")
        .name()
        .clone();
    let (batches, metrics) = execute(
        &validated,
        two_geo_inputs(left.clone(), right.clone()),
        runtime(),
    )
    .expect("execute")
    .collect_batches()
    .expect("stream ok");
    assert_eq!(
        batches.len(),
        1,
        "{case} v4: output blocking a batch singolo"
    );
    let batch = &batches[0];
    assert_eq!(
        batch.num_rows(),
        LEFT_IDS.len(),
        "{case} v4: allineato alle righe left"
    );
    assert_eq!(
        batch.schema().fields().last().expect("colonna flag").name(),
        &flag_name,
        "{case} v4: colonna flag come da contratto"
    );
    assert_left_passthrough(case, std::slice::from_ref(batch), &left_cells);
    let flags = boolean_column(batch, 2, case);
    for (row, expected) in expected_flags.iter().enumerate() {
        assert_eq!(
            (
                flags.is_null(row),
                (!flags.is_null(row)).then(|| flags.value(row))
            ),
            (expected.is_none(), *expected),
            "{case} v4: flag alla riga {row}"
        );
    }
    assert_eq!(
        (metrics.nodes["j"].rows_in, metrics.nodes["j"].rows_out),
        (12, 6),
        "{case} v4: righe in = left + right, righe out = righe left"
    );

    let schema = v3_schema(json!({
        "schema_version": 3,
        "operation": "within",
        "left_row_count": 6,
        "right_row_count": 6,
        "left_crs": "EPSG:32632",
        "right_crs": "EPSG:32632",
        "geometry_column": "geom",
        "max_pairs": RESOLVED_MAX_PAIRS,
    }));
    let (out_schema, v3_batches) = run_v3(&schema, &left, &right).expect("a-within v3 ok");
    assert_eq!(
        out_schema.fields().last().expect("colonna flag").name(),
        WITHIN_COLUMN,
        "{case} v3: colonna flag del protocollo"
    );
    assert_left_passthrough(case, &v3_batches, &left_cells);
    let v3_flags: Vec<Option<bool>> = v3_batches
        .iter()
        .flat_map(|batch| {
            let flags = boolean_column(batch, 2, case);
            (0..batch.num_rows())
                .map(|row| (!flags.is_null(row)).then(|| flags.value(row)))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        v3_flags,
        expected_flags.to_vec(),
        "{case} v3: flag allineati alle righe left"
    );
}

/// (a) `geo.count_points_in_polygons`: conteggio dei punti right
/// strettamente within ogni poligono left — 1 per il quadrato (10,10)-(20,20)
/// (contiene (12,12)); 0 per il poligono bucato ((37,37) e' nel buco: NON
/// contato — il buco conta nella topologia); 0 per il quadrato lontano e per
/// le righe left non poligonali; null su riga null. Colonna `COUNT_COLUMN`
/// nel v3, nome dal contratto nel v4 (D14.8, confronto sui valori).
#[test]
fn a_count_happy_path() {
    let case = "a-count";
    let (left, right, left_cells) = shared_fixture();
    let expected_counts: [Option<u64>; 6] = [Some(0), Some(1), Some(0), None, Some(0), Some(0)];

    let plan = binary_plan("geo.count_points_in_polygons", &json!({}));
    let (batches, metrics) = run_ok_v4(&plan, left.clone(), right.clone());
    assert_eq!(
        batches.len(),
        1,
        "{case} v4: output blocking a batch singolo"
    );
    let batch = &batches[0];
    assert_eq!(
        batch.num_rows(),
        LEFT_IDS.len(),
        "{case} v4: allineato alle righe left"
    );
    assert_left_passthrough(case, std::slice::from_ref(batch), &left_cells);
    let counts = uint64_column(batch, 2, case);
    for (row, expected) in expected_counts.iter().enumerate() {
        assert_eq!(
            (
                counts.is_null(row),
                (!counts.is_null(row)).then(|| counts.value(row))
            ),
            (expected.is_none(), *expected),
            "{case} v4: conteggio alla riga {row}"
        );
    }
    assert_eq!(
        (metrics.nodes["j"].rows_in, metrics.nodes["j"].rows_out),
        (12, 6),
        "{case} v4: righe in = left + right, righe out = righe left"
    );

    let schema = v3_schema(json!({
        "schema_version": 3,
        "operation": "count_points_in_polygons",
        "left_row_count": 6,
        "right_row_count": 6,
        "left_crs": "EPSG:32632",
        "right_crs": "EPSG:32632",
        "geometry_column": "geom",
        "max_pairs": RESOLVED_MAX_PAIRS,
    }));
    let (out_schema, v3_batches) = run_v3(&schema, &left, &right).expect("a-count v3 ok");
    assert_eq!(
        out_schema
            .fields()
            .last()
            .expect("colonna conteggio")
            .name(),
        COUNT_COLUMN,
        "{case} v3: colonna conteggio del protocollo"
    );
    assert_left_passthrough(case, &v3_batches, &left_cells);
    let v3_counts: Vec<Option<u64>> = v3_batches
        .iter()
        .flat_map(|batch| {
            let counts = uint64_column(batch, 2, case);
            (0..batch.num_rows())
                .map(|row| (!counts.is_null(row)).then(|| counts.value(row)))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        v3_counts,
        expected_counts.to_vec(),
        "{case} v3: conteggi allineati alle righe left"
    );
}

// ---------------------------------------------------------------------------
// (b) Geometria OGC-invalida (bowtie) — decode totale, mai lazy (D14.3)
// ---------------------------------------------------------------------------

/// Schema v3 sjoin della fixture di errore (conteggi dichiarati della
/// fixture passata).
fn v3_sjoin_schema(left_rows: u64, right_rows: u64) -> PairArrowSchema {
    v3_schema(json!({
        "schema_version": 3,
        "operation": "sjoin",
        "left_row_count": left_rows,
        "right_row_count": right_rows,
        "left_crs": "EPSG:32632",
        "right_crs": "EPSG:32632",
        "geometry_column": "geom",
        "predicate": "intersects",
        "max_pairs": RESOLVED_MAX_PAIRS,
    }))
}

/// Asserzioni comuni dei casi (b)/(c) lato v4: errore di decode al nodo `j`
/// — variante `Execution`, fase `Read` taggata (D14.5.4), categoria
/// `Execution`, motivo = testo base della sorgente (il prefisso
/// `contract violation: ` viene dalla serializzazione della variante
/// `InvalidPlan` del carrier in `step_error`, seguito dal testo v3) piu' il
/// suffisso diagnostico ` [side=… row=…]` (D14.5.2, solo con `diagnostics`
/// attivo).
///
/// Nota: usata dai casi (b); il caso (c) asserisce il rifiuto perimetrale
/// d'arco (divergenza documentata nel punto 3 dell'header), non questa
/// forma.
fn assert_v4_decode_error(
    case: &str,
    error: &PlenoraError,
    v3_text: &str,
    side: &str,
    row: u64,
) -> ErrorSignature {
    let signature = error_signature(error);
    assert_eq!(
        // Ottavo giro: la propagazione non sostituisce piu' la categoria con
        // `Execution`. Una geometria non conforme al contratto resta un
        // errore di CONTRATTO, e il contesto del passo si aggiunge tramite
        // `Replayed` invece di rimpiazzarlo.
        signature.variant,
        "Replayed",
        "{case}: errore di contratto con attribuzione (errore osservato: {error})"
    );
    assert_eq!(
        signature.node.as_deref(),
        Some("j"),
        "{case}: attribuzione al nodo"
    );
    assert_eq!(
        signature.operation.as_deref(),
        Some("geo.sjoin"),
        "{case}: operazione del nodo"
    );
    assert_eq!(
        signature.phase,
        ErrorPhase::Read,
        "{case}: fase Read del decode (D14.5.4)"
    );
    assert_eq!(
        signature.category,
        ErrorCategory::InvalidPlan,
        "{case}: la categoria dell'errore originale non viene sostituita"
    );
    let expected_reason = format!("contract violation: {v3_text} [side={side} row={row}]");
    assert_eq!(
        signature.reason, expected_reason,
        "{case}: testo base identico al v3 + dettaglio strutturato side/riga (D14.5.2)"
    );
    signature
}

/// (b) Bowtie su LEFT in posizione MAI candidata al prefilter: il suo
/// envelope (10000,10000)-(10010,10010) e' disgiunto da OGNI geometria del
/// lato right — con un decode lazy guidato dall'R-tree la cella non
/// verrebbe mai decodificata e l'input invalido passerebbe (non-determinismo
/// di contratto R12). Il decode totale D14.3 la rifiuta comunque: v3 e v4
/// producono lo stesso testo base; il v4 attribuisce al nodo `j` con fase
/// `Read` e riga strutturata (riga 2: terza riga della sequenza left
/// concatenata, secondo batch — l'indice e' globale sui batch, D14.5.2).
#[test]
fn b_ogc_invalid_left_never_prefilter_candidate() {
    let case = "b-left";
    let left = vec![
        geo_batch(
            &[0, 1],
            &[
                Some(point_wkb(1.0, 1.0)),
                Some(square_wkb(10.0, 10.0, 15.0, 15.0)),
            ],
        ),
        // Bowtie lontana: envelope disgiunto da ogni right (mai candidata).
        geo_batch(&[2], &[Some(bowtie_wkb(10_000.0, 10_000.0))]),
    ];
    let right = vec![geo_batch(&[10], &[Some(square_wkb(0.0, 0.0, 3.0, 3.0))])];
    let plan = binary_plan("geo.sjoin", &json!({"predicate": "intersects"}));

    let v3_error = run_v3(&v3_sjoin_schema(3, 1), &left, &right)
        .expect_err("b-left v3: atteso errore di validazione OGC");
    assert!(
        matches!(v3_error, ArrowTransportError::Geometry(_)),
        "{case} v3: variante Geometry del decoder validante: {v3_error}"
    );
    let v3_text = v3_error.to_string();

    let v4_error = run_err_v4(&plan, left, right);
    assert_v4_decode_error(case, &v4_error, &v3_text, "left", 2);
}

/// (b) Bowtie su RIGHT (speculare): stesso testo base nei due percorsi; il
/// v4 tagga `side=right` con la riga nella sequenza right (riga 1). Anche
/// qui la cella invalida e' lontana da ogni geometria left (mai candidata).
#[test]
fn b_ogc_invalid_right() {
    let case = "b-right";
    let left = vec![geo_batch(&[0], &[Some(square_wkb(0.0, 0.0, 3.0, 3.0))])];
    let right = vec![geo_batch(
        &[10, 11],
        &[
            Some(square_wkb(10.0, 10.0, 15.0, 15.0)),
            Some(bowtie_wkb(10_000.0, 10_000.0)),
        ],
    )];
    let plan = binary_plan("geo.sjoin", &json!({"predicate": "intersects"}));

    let v3_error = run_v3(&v3_sjoin_schema(1, 2), &left, &right)
        .expect_err("b-right v3: atteso errore di validazione OGC");
    assert!(
        matches!(v3_error, ArrowTransportError::Geometry(_)),
        "{case} v3: variante Geometry del decoder validante: {v3_error}"
    );
    let v3_text = v3_error.to_string();

    let v4_error = run_err_v4(&plan, left, right);
    assert_v4_decode_error(case, &v4_error, &v3_text, "right", 1);
}

/// (b) Primo errore in ordine (side, riga) (D14.5.3): celle invalide su
/// ENTRAMBI i lati con la riga right PIU' BASSA (right riga 0, left riga 1).
/// Il v4 riporta comunque `side=left row=1`: il decode left completo
/// precede qualunque contabilita' del lato right (ordine globale fisso
/// left→right del governor, reservation incluse) — la selezione non e'
/// temporale ne' per riga minima assoluta. Il v3 decodifica left prima di
/// right nello stesso ordine: stesso testo base.
#[test]
fn b_first_error_in_side_row_order() {
    let case = "b-primo-errore";
    let left = vec![geo_batch(
        &[0, 1],
        &[Some(point_wkb(1.0, 1.0)), Some(bowtie_wkb(100.0, 100.0))],
    )];
    // Riga right 0 invalida: piu' bassa della riga left 1, ma il lato left
    // e' decodificato per primo.
    let right = vec![geo_batch(
        &[10, 11],
        &[Some(bowtie_wkb(200.0, 200.0)), Some(point_wkb(2.0, 2.0))],
    )];
    let plan = binary_plan("geo.sjoin", &json!({"predicate": "intersects"}));

    let v3_error = run_v3(&v3_sjoin_schema(2, 2), &left, &right)
        .expect_err("b-primo-errore v3: atteso errore di validazione OGC");
    let v3_text = v3_error.to_string();

    let v4_error = run_err_v4(&plan, left, right);
    let signature = assert_v4_decode_error(case, &v4_error, &v3_text, "left", 1);
    // Il testo base e' quello della cella left: le due bowtie hanno lo
    // stesso messaggio, la PROVA dell'ordine (side, riga) e' nel dettaglio
    // strutturato — right row=0 non deve mai comparire.
    assert!(
        !signature.reason.contains("side=right"),
        "{case}: il lato right non e' mai contabilizzato prima del decode left"
    );
}

// ---------------------------------------------------------------------------
// (c) Cella oltre MAX_CELL_BYTES su ciascun lato
// ---------------------------------------------------------------------------

/// Runtime (c): tetto batch alzato a 128 MiB. Con il default (64 MiB) il
/// batch da ~67 MiB sarebbe rifiutato ancora prima dal tetto batch
/// (`max_batch_bytes` sull'arco di input) — stessa classe di difesa
/// perimetrale; qui si isola il gate PER-CELLA, controparte diretta del
/// `CellTooLarge` v3 (stessa soglia di 64 MiB sulla stessa cella).
fn oversized_runtime() -> RuntimeContext {
    RuntimeContext {
        batch_target: BatchTarget {
            target_batch_bytes: 128 * 1024 * 1024,
            max_batch_bytes: 128 * 1024 * 1024,
        },
        diagnostics: true,
        ..RuntimeContext::default()
    }
}

/// (c) Cella oltre `MAX_CELL_BYTES` su un lato (fixture a batch singolo per
/// lato: riga 0). Divergenza dalla lettera del caso (punto 3 dell'header):
/// nel v4 la cella NON raggiunge mai il gate del nodo — la validazione
/// perimetrale dell'arco di input la rifiuta prima (`InvalidPlan` d'arco,
/// nessun nodo, fase `Validate` derivata, riga nel testo e colonna nel
/// dettaglio diagnostico). Il v3 non ha perimetro: la cella arriva al decode
/// condiviso (`decode_geometry_batches`) che la rifiuta con `CellTooLarge` e
/// la misura esatta in byte. Entrambi fail-closed sulla stessa cella alla
/// stessa soglia per-cella di 64 MiB — asserzioni separate per lato, mai un
/// confronto campo-per-campo.
fn assert_cell_too_large(case: &str, left: Vec<RecordBatch>, right: Vec<RecordBatch>, _side: &str) {
    let v3_error = run_v3(&v3_sjoin_schema(1, 1), &left, &right)
        .expect_err("c: atteso CellTooLarge nel trasporto v3");
    let ArrowTransportError::CellTooLarge(cell_bytes) = v3_error else {
        panic!("{case} v3: atteso CellTooLarge, ottenuto {v3_error}");
    };
    assert!(
        cell_bytes > MAX_CELL_BYTES,
        "{case}: la fixture supera davvero il limite"
    );

    let mut output = execute(
        &graph(&binary_plan(
            "geo.sjoin",
            &json!({"predicate": "intersects"}),
        )),
        two_geo_inputs(left, right),
        oversized_runtime(),
    )
    .expect("execute");
    let v4_error = first_stream_error(&mut output)
        .expect("c: atteso il rifiuto perimetrale d'arco, stream riuscito");
    let signature = error_signature(&v4_error);
    assert_eq!(
        signature.variant, "DataMapping",
        "{case} v4: rifiuto perimetrale d'arco, non del nodo (vedi header, punto 3): {v4_error}"
    );
    assert_eq!(signature.node, None, "{case} v4: nessun nodo strutturato");
    assert_eq!(
        signature.category,
        ErrorCategory::DataMapping,
        "{case} v4: categoria"
    );
    assert_eq!(
        signature.phase,
        ErrorPhase::Read,
        "{case} v4: fase derivata dalla variante (validazione d'arco)"
    );
    assert_eq!(
        signature.reason, "righe non conformi al contratto di trasformazione",
        "{case} v4: dettagli row-scoped solo nella diagnostica strutturata"
    );
}

/// (c) Cella oversize su LEFT.
#[test]
fn c_cell_over_max_cell_bytes_on_left() {
    let cell = oversized_cell_wkb();
    let left = vec![geo_batch(&[0], &[Some(cell)])];
    let right = vec![geo_batch(&[10], &[Some(square_wkb(0.0, 0.0, 3.0, 3.0))])];
    assert_cell_too_large("c-left", left, right, "left");
}

/// (c) Cella oversize su RIGHT (speculare: il perimetro dell'arco
/// `right_in` rifiuta prima di qualunque contabilita' del nodo).
#[test]
fn c_cell_over_max_cell_bytes_on_right() {
    let cell = oversized_cell_wkb();
    let left = vec![geo_batch(&[0], &[Some(square_wkb(0.0, 0.0, 3.0, 3.0))])];
    let right = vec![geo_batch(&[10], &[Some(cell)])];
    assert_cell_too_large("c-right", left, right, "right");
}

// ---------------------------------------------------------------------------
// (d) Cancellazione (solo v4: il trasporto v3 non ha token — nessun lato da
// confrontare, dichiarato nel caso D14.9(d))
// ---------------------------------------------------------------------------

/// Input geo lazy che cancella il token in due punti possibili (pattern dei
/// test di cancellazione dell'executor e dell'oracolo ADR-0012):
///
/// - `cancel_at_pull: Some(n)`: quando l'executor tira il batch numero `n`
///   (1-based) — il Ctrl-C arriva a meta' stream ed e' osservato al confine
///   cooperativo successivo, DENTRO il ciclo di drenaggio (il check del
///   ciclo segue la `next()`);
/// - `cancel_at_exhaustion`: quando l'executor chiede il batch SUCCESSIVO
///   all'ultimo (la `next()` che restituisce `None`): il drenaggio si
///   completa con tutti i batch accettati e il token e' osservato al check
///   post-drenaggio pre-kernel — e' l'unico punto di attivazione
///   "esattamente dopo l'ultimo batch" che supera il ciclo (cancellare alla
///   pull finale lo farebbe osservare nel corpo della stessa iterazione).
struct CancellingGeoInput {
    batches: std::vec::IntoIter<RecordBatch>,
    pulled: usize,
    cancel_at_pull: Option<usize>,
    cancel_at_exhaustion: bool,
    token: CancellationToken,
}

impl Iterator for CancellingGeoInput {
    type Item = plenora_core::Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(batch) = self.batches.next() else {
            if self.cancel_at_exhaustion {
                self.token.cancel();
            }
            return None;
        };
        self.pulled += 1;
        if self.cancel_at_pull == Some(self.pulled) {
            self.token.cancel();
        }
        Some(Ok(batch))
    }
}

/// Due batch per lato di quadratini validi: il contenuto e' irrilevante, la
/// cancellazione scatta prima del kernel.
fn cancellation_fixture() -> (Vec<RecordBatch>, Vec<RecordBatch>) {
    let left = vec![
        geo_batch(&[0], &[Some(square_wkb(0.0, 0.0, 10.0, 10.0))]),
        geo_batch(&[1], &[Some(square_wkb(20.0, 20.0, 30.0, 30.0))]),
    ];
    let right = vec![
        geo_batch(&[10], &[Some(square_wkb(1.0, 1.0, 2.0, 2.0))]),
        geo_batch(&[11], &[Some(square_wkb(3.0, 3.0, 4.0, 4.0))]),
    ];
    (left, right)
}

// Percorso permissivo (`Inputs::with`), deprecato ma ancora supportato:
// questo oracolo non dichiarano contratti e ne coprono il comportamento.
#[allow(deprecated)]
fn run_cancellation(
    case: &str,
    cancel_at_pull: Option<usize>,
    cancel_at_exhaustion: bool,
) -> ErrorSignature {
    let (left, right) = cancellation_fixture();
    let plan = binary_plan("geo.sjoin", &json!({"predicate": "intersects"}));
    let token = CancellationToken::new();
    let inputs = Inputs::new()
        .with(
            "left_in",
            Input::from_batches(left).expect("left non vuoto"),
        )
        .expect("input left")
        .with(
            "right_in",
            Input::from_iter(
                geo_schema(),
                CancellingGeoInput {
                    batches: right.into_iter(),
                    pulled: 0,
                    cancel_at_pull,
                    cancel_at_exhaustion,
                    token: token.clone(),
                },
            ),
        )
        .expect("input right");
    let runtime = RuntimeContext {
        cancellation: token,
        ..runtime()
    };
    let mut output = execute(&graph(&plan), inputs, runtime).expect("execute");
    let error = first_stream_error(&mut output)
        .unwrap_or_else(|| panic!("{case}: atteso Cancelled, lo stream e' terminato"));
    error_signature(&error)
}

/// Asserzioni comuni (d): `Cancelled` attribuito al nodo `j` (`BoundaryOnly`
/// di catalogo, D14.5.5: confini di batch in drenaggio + post-drenaggio
/// pre-kernel, nessun check dentro il kernel). La fase osservata e' `Write`
/// per DERIVAZIONE di variante (`PlenoraError::phase` mappa `Cancelled` su
/// `Write`): gli errori di cancellazione non sono taggati di fase —
/// divergenza dalla fase `Read` del drenaggio dichiarata nell'header.
fn assert_cancelled_at_node(case: &str, signature: &ErrorSignature) {
    assert_eq!(signature.variant, "Cancelled", "{case}: variante Cancelled");
    assert_eq!(
        signature.node.as_deref(),
        Some("j"),
        "{case}: nodo osservante"
    );
    assert_eq!(
        signature.operation.as_deref(),
        Some("geo.sjoin"),
        "{case}: operazione del nodo"
    );
    assert_eq!(
        signature.reason, "cancellazione richiesta dal chiamante",
        "{case}: motivo canonico"
    );
    assert_eq!(
        signature.category,
        ErrorCategory::Cancelled,
        "{case}: categoria"
    );
    assert_eq!(
        signature.phase,
        ErrorPhase::Write,
        "{case}: fase derivata dalla variante (cancellazione non taggata)"
    );
}

/// (d1) Token attivato DURANTE il drenaggio del ramo right (alla prima pull:
/// il left e' gia' drenato, il ramo right e' a meta') -> osservato al check
/// di confine batch successivo dentro il ciclo di drenaggio -> `Cancelled`
/// al nodo `j`.
#[test]
fn d1_cancellation_during_drain() {
    let signature = run_cancellation("d1", Some(1), false);
    assert_cancelled_at_node("d1", &signature);
}

/// (d2) Token attivato dall'input right esattamente dopo l'ULTIMO batch (la
/// pull a esaurimento, dopo il numero di batch right: il token scatta quando
/// il ciclo chiede il batch successivo all'ultimo e il drenaggio si completa
/// con tutti i batch accettati) -> osservato al check post-drenaggio
/// pre-kernel di `run_geo_binary_blocking` (D14.5.5) -> `Cancelled` al nodo
/// `j`, prima di qualunque lavoro del kernel.
#[test]
fn d2_cancellation_post_drain_pre_kernel() {
    let signature = run_cancellation("d2", None, true);
    assert_cancelled_at_node("d2", &signature);
}

// ---------------------------------------------------------------------------
// (f) Espansione oltre il vincolo
// ---------------------------------------------------------------------------

/// (f) Fixture 1-a-molti: un poligono left che interseca K=4 poligoni right
/// -> 4 coppie. I due meccanismi NON sono omologhi (divergenza dichiarata
/// nell'header, punto 2): il v4 applica il vincolo RELATIVO di catalogo
/// (`MaxRelative` per `geo.sjoin`, ADR 6 — binding = max(output/left,
/// output/right) = 4 > 1 con `max_expansion_factor: 1`), il v3 non ha
/// vincolo di espansione e rifiuta lo stesso output col tetto assoluto
/// `max_output_rows: K-1` (`OutputRowsExceeded`). Entrambi fail-closed sullo
/// stesso output, asserzioni separate per lato.
///
/// Divergenza di attribuzione (punto 1 dell'header): il v4 produce
/// `InvalidPlan` GREZZO con il nodo nel testo e fase `Validate` derivata —
/// `check_join_expansion` propaga via `?` dal guscio, mai dal carrier
/// `GeoBinaryStepError` — NON `Execution { node: "j" }` della lettera di
/// D14.5.1 (stessa forma del ramo tabellare, che condivide il check).
#[test]
fn f_expansion_beyond_constraint() {
    let case = "f";
    let left = vec![geo_batch(&[0], &[Some(square_wkb(0.0, 0.0, 100.0, 100.0))])];
    let right = vec![geo_batch(
        &[10, 11, 12, 13],
        &[
            Some(square_wkb(10.0, 10.0, 20.0, 20.0)),
            Some(square_wkb(30.0, 10.0, 40.0, 20.0)),
            Some(square_wkb(10.0, 30.0, 20.0, 40.0)),
            Some(square_wkb(30.0, 30.0, 40.0, 40.0)),
        ],
    )];
    let mut plan = binary_plan("geo.sjoin", &json!({"predicate": "intersects"}));
    plan["limits"] = json!({"max_expansion_factor": 1});

    let v4_error = run_err_v4(&plan, left.clone(), right.clone());
    let signature = error_signature(&v4_error);
    // Il vincolo ADR 6 e' un limite di RISORSA: il piano e' corretto, sono i
    // dati a non entrarci. Categoria `resource_limit` (settimo giro, finding
    // 7), non `invalid_plan`; resta grezzo, non `Execution`.
    assert_eq!(
        signature.variant, "ResourceLimit",
        "{case} v4: vincolo ADR 6 grezzo, non Execution (vedi header, punto 1)"
    );
    assert_eq!(signature.node, None, "{case} v4: nessun nodo strutturato");
    assert_eq!(
        signature.category,
        ErrorCategory::ResourceLimit,
        "{case} v4: categoria"
    );
    // Fase derivata dalla variante: `ResourceLimit` nasce eseguendo, non
    // validando — il piano era valido, sono i dati a non entrare nel budget.
    // `Write` e' la fase runtime di questo codebase (stessa di `Execution`,
    // `DataMapping`, `Io`).
    assert_eq!(
        signature.phase,
        ErrorPhase::Write,
        "{case} v4: fase derivata dalla variante"
    );
    assert!(
        signature
            .reason
            .contains("max_expansion_factor superato al nodo `j`"),
        "{case} v4: il nodo e' nel testo: {}",
        signature.reason
    );
    assert!(
        signature.reason.contains("MaxRelative"),
        "{case} v4: vincolo di catalogo nel testo: {}",
        signature.reason
    );

    let mut schema_json = json!({
        "schema_version": 3,
        "operation": "sjoin",
        "left_row_count": 1,
        "right_row_count": 4,
        "left_crs": "EPSG:32632",
        "right_crs": "EPSG:32632",
        "geometry_column": "geom",
        "predicate": "intersects",
        "max_pairs": RESOLVED_MAX_PAIRS,
        "max_output_rows": 3,
    });
    let schema = v3_schema(schema_json.clone());
    let v3_error = run_v3(&schema, &left, &right).expect_err("f v3: atteso OutputRowsExceeded");
    assert_eq!(
        v3_error.to_string(),
        "righe di output 4 oltre il limite max_output_rows 3",
        "{case} v3: rifiuto fail-closed dello stesso output (tetto assoluto)"
    );
    // Igiene della fixture: lo schema e' ricostruito dal clone per
    // dimostrare che il JSON non viene mutato dalla deserializzazione.
    schema_json["max_output_rows"] = json!(4);
    let schema_ok = v3_schema(schema_json);
    let (_schema, batches) = run_v3(&schema_ok, &left, &right).expect("f v3 con tetto sufficiente");
    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(
        total_rows, 4,
        "{case} v3: con tetto = K l'output e' prodotto"
    );
}

// ---------------------------------------------------------------------------
// (g) Governor che rifiuta la reservation DECODIFICATA (solo v4 — condizione
// di attivazione del perimetro, ruolo DER-003; il v3 non ha governor)
// ---------------------------------------------------------------------------

/// (g) Fixture a batch singolo per lato (misura Arrow esatta per
/// costruzione: il concat di un batch singolo preserva il layout dei
/// buffer), con il lato left DOMINANTE: una `LineString` da 20 000
/// coordinate (~320 KiB sia in WKB sia nella forma decodificata, piu' gli
/// overhead) contro un right minuscolo (un punto). La forma asimmetrica e'
/// obbligata dalla sequenza delle reservation osservata: durante il
/// drenaggio i lease dei batch di INPUT (owner `left_in`/`right_in`) sono
/// VIVI e nel budget — il rifiuto deve scattare dopo, alla reservation
/// decodificata left, quindi serve `arrow_right < decodificata_left` (con
/// due lati piccoli e simmetrici il budget che fallisce la decodificata
/// fallirebbe prima il drenaggio del ramo right, owner `right_in` — prima
/// versione di questo test, corretta dopo l'osservazione). I lease di input
/// sono rilasciati prima delle reservation del nodo (drop in
/// `run_geo_binary_blocking`): al momento del rifiuto e' vivo solo il lease
/// Arrow left, quindi il testo riporta esattamente `left_arrow` riservati.
/// Linea e non poligono: la validazione OGC dei poligoni e' appaiata O(n^2)
/// (troppo lenta in debug su 20 000 vertici), le linee si validano in O(n).
///
/// Con budget `B = arrow_left + decodificata_left - 1` la reservation Arrow
/// left passa e quella della forma decodificata left FALLISCE — il governor
/// rifiuta PRIMA di allocare (D14.4: riservare prima di decodificare,
/// rifiutare prima di allocare) con owner `j`. Rieseguendo con budget che
/// copre Arrow+decodificata+output il piano riesce: la contabilita'
/// decodificata e' attiva e correttamente ordinata.
///
/// Divergenza di attribuzione (punto 1 dell'header): l'errore arriva come
/// `InvalidPlan` grezzo del governor (`max_governed_memory_bytes superato: `j`
/// richiede …`), fase `Validate` derivata — non `Execution` della lettera
/// di D14.5.1 (stessa forma del test DER-003 di ADR-0012).
#[test]
fn g_governor_rejects_decoded_reservation() {
    let case = "g";
    let left = vec![geo_batch(&[0], &[Some(big_linestring_wkb(20_000))])];
    let right = vec![geo_batch(&[10], &[Some(point_wkb(1.0, 1.0))])];
    // Misure a runtime (mai costanti magiche): stesse funzioni del ramo —
    // `get_array_memory_size` per la reservation Arrow, `preflight_decoded_bytes`
    // per quella decodificata (indice 1 = colonna `geom`).
    let left_arrow = left[0].get_array_memory_size() as u64;
    let right_arrow = right[0].get_array_memory_size() as u64;
    let left_decoded = preflight_decoded_bytes(&geo_schema(), std::slice::from_ref(&left[0]), 1);
    let right_decoded = preflight_decoded_bytes(&geo_schema(), std::slice::from_ref(&right[0]), 1);
    assert!(
        left_decoded > 1,
        "{case}: la forma decodificata e' misurata"
    );
    assert_ne!(
        left_arrow, left_decoded,
        "{case}: misure distinte — la reservation fallita e' identificabile dal testo"
    );
    // La reservation Arrow left passa (`left_arrow <= budget`), quella
    // decodificata left fallisce per 1 byte (`left_arrow + left_decoded >
    // budget`): nessuna ambiguita' su QUALE reservation rifiuta.
    let failing_budget = left_arrow + left_decoded - 1;
    // Budget sufficiente: Arrow+decodificata dei due lati + output (left
    // passthrough + colonna flag, misurato per eccesso con un secondo
    // `left_arrow`) — copre il picco del ramo con margine.
    let ok_budget = 2 * left_arrow + left_decoded + right_arrow + right_decoded + 1024;

    let mut plan = binary_plan("geo.within", &json!({}));
    plan["limits"] = json!({"max_governed_memory_bytes": failing_budget});
    let v4_error = run_err_v4(&plan, left.clone(), right.clone());
    let signature = error_signature(&v4_error);
    assert_eq!(
        // Nono giro: il budget di memoria esaurito e' `ResourceLimit`.
        signature.variant,
        "ResourceLimit",
        "{case}: reservation rifiutata grezza dal governor (vedi header, punto 1)"
    );
    assert_eq!(signature.node, None, "{case}: nessun nodo strutturato");
    assert_eq!(
        signature.category,
        ErrorCategory::ResourceLimit,
        "{case}: categoria"
    );
    assert_eq!(
        signature.phase,
        // Nono giro: la variante e' `ResourceLimit`, che deriva `Write` — la
        // reservation fallisce mentre il nodo PRODUCE, non mentre si valida.
        // I tetti del confine d'INGRESSO dichiarano invece `Read` con un tag
        // esplicito (ADR-0009, emendamento 2026-08-17).
        ErrorPhase::Write,
        "{case}: fase derivata dalla variante"
    );
    assert!(
        signature.reason.contains(&format!(
            "max_governed_memory_bytes superato: `j` richiede {left_decoded} byte"
        )),
        "{case}: la reservation DECODIFICATA left e' quella rifiutata (owner `j`): {}",
        signature.reason
    );

    plan["limits"] = json!({"max_governed_memory_bytes": ok_budget});
    let (batches, metrics) = run_ok_v4(&plan, left, right);
    let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(
        total_rows, 1,
        "{case}: con budget sufficiente il piano riesce"
    );
    assert_eq!(
        (metrics.nodes["j"].rows_in, metrics.nodes["j"].rows_out),
        (2, 1),
        "{case}: metriche del run riuscito"
    );
}
