//! Test end-to-end della CLI sul DAG v4 (Fase 2A): `validate` e `run` di
//! piani `schema_version: 4` attraverso planner/executor di `plenora-engine`,
//! con il piano legacy che continua a funzionare invariato.
//!
//! Le fixture Arrow sono generate nei test (stesso stile di
//! `roundtrip_smoke.rs`). Il test con geometria richiede la risoluzione CRS
//! PROJ (la scoperta del contratto risolve il `geo.crs` dei metadati): e'
//! compilato solo con la feature `proj-backend`.

use std::process::Command;
use std::sync::Arc;

use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::ipc::reader::FileReader;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use serde_json::json;

#[cfg(feature = "proj-backend")]
use plenora_core::arrow::array::{Array, ArrayRef, BinaryArray, Float64Array};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_plenora-data-tools"))
}

fn write_ipc(path: &std::path::Path, schema: &SchemaRef, batches: &[RecordBatch]) {
    let file = std::fs::File::create(path).expect("create input");
    let mut writer = FileWriter::try_new(file, schema).expect("writer");
    for batch in batches {
        writer.write(batch).expect("write batch");
    }
    writer.finish().expect("finish");
}

fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]))
}

fn table_batch(ids: &[i64], names: &[&str]) -> RecordBatch {
    RecordBatch::try_new(
        table_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())),
            Arc::new(StringArray::from(
                names.iter().map(|name| Some(*name)).collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("batch fixture")
}

/// Piano v4 tabellare: filter `id > 0` poi rename `name` -> `label`.
fn table_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "r", "op": "table.rename", "in": ["f"],
             "config": {"renames": [{"old_name": "name", "new_name": "label"}]}},
        ],
        "output": "r",
    })
}

/// Scrive piano e input tabellare standard nella directory data.
fn write_table_fixture(directory: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let plan = directory.join("plan.json");
    let input = directory.join("input.arrow");
    std::fs::write(&plan, serde_json::to_vec(&table_plan()).expect("json")).expect("plan");
    write_ipc(
        &input,
        &table_schema(),
        &[table_batch(&[0, 1, 2], &["a", "b", "c"])],
    );
    (plan, input)
}

#[test]
fn validate_v4_stampa_il_riepilogo_del_dag() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());

    let result = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["schema_version"], 4);
    let plan_hash = summary["plan_hash"].as_str().expect("plan_hash");
    assert_eq!(plan_hash.len(), 64, "plan_hash SHA-256 esadecimale");
    assert_eq!(summary["inputs"], json!(["main"]));
    assert_eq!(summary["topological_order"], json!(["f", "r"]));

    let nodes = summary["nodes"].as_array().expect("nodi");
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["id"], "f");
    assert_eq!(nodes[0]["op"], "table.filter");

    let edges = summary["edges"].as_array().expect("archi");
    assert_eq!(edges.len(), 3, "un arco per input + uno per nodo");
    assert_eq!(edges[0]["edge"], "main");
    assert_eq!(edges[0]["kind"], "input");
    assert_eq!(edges[0]["contract"]["geometry"], serde_json::Value::Null);
    let field_names: Vec<&str> = edges[2]["contract"]["fields"]
        .as_array()
        .expect("campi")
        .iter()
        .filter_map(|field| field["name"].as_str())
        .collect();
    assert_eq!(field_names, ["id", "label"], "la rinomina e' inferita a secco");

    let segments = summary["segments"].as_array().expect("segmenti");
    assert_eq!(segments.len(), 1, "catena streaming fusa in un segmento");
    assert_eq!(segments[0]["mode"], "LinearStreaming");
    assert_eq!(segments[0]["parallelism"], "SerialFused");
    assert_eq!(segments[0]["nodes"], json!(["f", "r"]));

    assert_eq!(
        summary["required_capabilities"],
        json!(["atomic_publish"]),
        "piano solo tabellare: solo il profilo di publish di default (ADR 7)"
    );
    assert_eq!(
        summary["input_contract_fingerprints"].as_array().map(Vec::len),
        Some(1)
    );
}

#[test]
fn run_v4_scrive_output_e_metriche_json_su_stdout() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON metriche");
    assert_eq!(metrics["status"], "ok");
    assert_eq!(metrics["output_rows"], 2, "filter id>0 lascia due righe");
    assert_eq!(metrics["nodes"]["f"]["operation"], "table.filter");
    assert_eq!(metrics["nodes"]["f"]["rows_in"], 3);
    assert_eq!(metrics["nodes"]["f"]["rows_out"], 2);
    assert_eq!(metrics["nodes"]["r"]["operation"], "table.rename");
    assert_eq!(metrics["nodes"]["r"]["rows_out"], 2);
    assert!(
        metrics["nodes"]["f"]["wall_time_ms"].as_f64().is_some(),
        "wall time per nodo presente"
    );
    assert!(metrics["segments"].as_object().is_some_and(|s| !s.is_empty()));

    // Output rileggibile: la rinomina e' applicata, le righe sono due.
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    assert!(reader.schema().field_with_name("label").is_ok());
    let rows: usize = reader.map(|batch| batch.expect("batch").num_rows()).sum();
    assert_eq!(rows, 2);
}

#[test]
fn run_v4_senza_inputs_fallisce() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, _input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("input"), "stderr: {stderr}");
    assert!(!output_path.exists(), "nessun output parziale");
}

/// Envelope §9 (R9.1/R9.2): l'uscita CLI di un errore e' JSON parsabile
/// con i quattro assi espliciti — categoria, fase, effetto, retry — mai
/// da dedurre dal messaggio.
#[test]
fn errore_cli_emette_envelope_a_quattro_assi() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, _input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(!result.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&result.stderr)
        .expect("stderr deve essere l'envelope JSON par. 9");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["protocol_version"], 1);
    for axis in ["category", "phase", "remote_effect", "message"] {
        assert!(
            envelope["error"].get(axis).is_some_and(serde_json::Value::is_string),
            "asse `{axis}` presente e testuale: {envelope}"
        );
    }
    assert!(
        envelope["error"]["retry"]["kind"].is_string(),
        "retry in forma taggata con `kind` testuale: {envelope}"
    );
    assert_eq!(envelope["error"]["remote_effect"], "none");
}

#[test]
fn run_v4_schema_mismatch_fallisce_in_validazione() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");
    // Il piano filtra su una colonna assente dallo schema dell'input.
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "mancante", "operator": ">", "value": 0}},
            ],
            "output": "f",
        }))
        .expect("json"),
    )
    .expect("plan");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(!result.status.success());
    assert!(!result.stderr.is_empty(), "errore diagnostico presente");
    assert!(!output_path.exists(), "nessun output parziale");
}

#[test]
fn run_v4_non_sovrascrive_un_output_esistente() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");
    std::fs::write(&output_path, b"contenuto precedente").expect("output preesistente");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("esistente"), "stderr: {stderr}");
    assert_eq!(
        std::fs::read(&output_path).expect("output intatto"),
        b"contenuto precedente",
        "no-clobber: il file esistente non e' toccato"
    );
}

#[test]
fn piano_legacy_continua_a_funzionare_invariato() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    let input = directory.path().join("input.arrow");
    let output_path = directory.path().join("output.arrow");
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"steps":[{"operation":"rename","config":{"renames":[{"old_name":"name","new_name":"label"}]}}]}"#,
    )
    .expect("plan");
    write_ipc(&input, &table_schema(), &[table_batch(&[1, 2], &["a", "b"])]);

    // validate legacy: riepilogo di Fase 1, senza i campi del DAG v4.
    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate legacy");
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["steps"], 1);
    assert!(summary.get("plan_hash").is_none());

    // run legacy: stessa invocazione di sempre (--input singolo).
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run legacy");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    assert!(reader.schema().field_with_name("label").is_ok());
}

// ---------------------------------------------------------------------------
// R4.6.3 (contratti trasversali v2.0-rc9/rc10): una colonna geometria senza
// CRS dichiarato NON ferma le operazioni che non lo richiedono. I test di
// questa sezione non risolvono mai un CRS: valgono con e senza backend PROJ.
// ---------------------------------------------------------------------------

/// Fixture: `id` Int64 + colonna `geometry` GeoArrow-WKB SENZA CRS — solo il
/// nome di estensione e, opzionalmente, un metadato `geo` senza chiave
/// `crs`; nessuna chiave canonica.
fn geometry_without_crs_schema(geo_json: Option<&str>) -> SchemaRef {
    use plenora_kernels_geo::arrow_adapter as adapter;
    let mut metadata = std::collections::HashMap::from([(
        adapter::GEOARROW_EXTENSION_KEY.to_owned(),
        adapter::GEOARROW_WKB_EXTENSION.to_owned(),
    )]);
    if let Some(geo) = geo_json {
        metadata.insert(adapter::GEO_METADATA_KEY.to_owned(), geo.to_owned());
    }
    let geometry = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        geometry,
    ]))
}

/// WKB little-endian di un punto (fixture tabellare: mai decodificato).
fn point_wkb_le(x: f64, y: f64) -> Vec<u8> {
    let mut wkb = Vec::with_capacity(21);
    wkb.push(1_u8);
    wkb.extend_from_slice(&1_u32.to_le_bytes());
    wkb.extend_from_slice(&x.to_le_bytes());
    wkb.extend_from_slice(&y.to_le_bytes());
    wkb
}

fn geometry_without_crs_batch(
    geo_json: Option<&str>,
    ids: &[i64],
    cells: &[Option<Vec<u8>>],
) -> RecordBatch {
    use plenora_core::arrow::array::{ArrayRef, BinaryArray};
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    RecordBatch::try_new(
        geometry_without_crs_schema(geo_json),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch fixture senza CRS")
}

/// Piano v4: solo `table.filter` su `id` (colonna non geometrica).
fn filter_only_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    })
}

fn write_geometry_without_crs_fixture(
    directory: &std::path::Path,
    geo_json: Option<&str>,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let plan = directory.join("plan.json");
    let input = directory.join("input.arrow");
    std::fs::write(&plan, serde_json::to_vec(&filter_only_plan()).expect("json")).expect("plan");
    write_ipc(
        &input,
        &geometry_without_crs_schema(geo_json),
        &[geometry_without_crs_batch(
            geo_json,
            &[0, 1, 2],
            &[
                Some(point_wkb_le(0.0, 0.0)),
                Some(point_wkb_le(1.0, 1.0)),
                None,
            ],
        )],
    );
    (plan, input)
}

#[test]
fn dag_v4_filter_on_geometry_without_crs_passes_and_propagates_missing() {
    use plenora_kernels_geo::arrow_adapter as adapter;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_geometry_without_crs_fixture(directory.path(), None);

    // R4.6.3: il filtro tabellare non richiede alcun CRS — validate passa e
    // dichiara lo stato, non un CRS.
    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    let edges = summary["edges"].as_array().expect("archi");
    let geometry = &edges[0]["contract"]["geometry"];
    assert_eq!(geometry["name"], "geometry");
    assert_eq!(geometry["crs"], serde_json::Value::Null);
    assert_eq!(geometry["crs_resolution"], "missing");

    // run: i byte geometrici transitano invariati (mai decodificati).
    let output_path = directory.path().join("output.arrow");
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let schema = reader.schema();
    // R4.6.4: lo stato mancante arriva al bordo di scrittura dichiarato —
    // `crs_resolution = missing` e NESSUNA chiave CRS (R2.2), mai un CRS
    // inventato (R4.4), mai la dichiarazione persa.
    let (_, geometry_field) = schema.column_with_name("geometry").expect("colonna geometry");
    let metadata = geometry_field.metadata();
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY).map(String::as_str),
        Some("missing")
    );
    for key in [
        adapter::PLENORA_GEOMETRY_CRS_ID_KEY,
        adapter::PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
        adapter::PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY,
        adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY,
        adapter::PLENORA_GEOMETRY_SRID_KEY,
    ] {
        assert!(
            !metadata.contains_key(key),
            "chiave `{key}` emessa con CRS mancante"
        );
    }
    // Le altre nozioni restano oneste: dimensions non dichiarata ->
    // `unknown` (R3.4), encoding completato dall'estensione (R2.7).
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY).map(String::as_str),
        Some("unknown")
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_ENCODING_KEY).map(String::as_str),
        Some("wkb")
    );
    assert_eq!(
        schema.metadata().get(adapter::PLENORA_CONTRACT_VERSION_KEY).map(String::as_str),
        Some("1"),
        "versione di protocollo R2.5 con chiavi canoniche"
    );

    // Round-trip R4.6.4: l'output (con le chiavi canoniche `missing`)
    // rientra come input dello stesso piano e ripassa — la dichiarazione
    // sopravvive al giro bordo-centro-bordo senza risoluzioni.
    let revalidate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&output_path)
        .output()
        .expect("re-validate");
    assert!(
        revalidate.status.success(),
        "round-trip stderr: {}",
        String::from_utf8_lossy(&revalidate.stderr)
    );
}

#[test]
fn dag_v4_geo_op_on_geometry_without_crs_fails_with_the_declared_cause() {
    let directory = tempfile::tempdir().expect("tempdir");
    // Dimensionalita' dichiarata `xy` (il gate B1.3 sulle dimensioni e'
    // valutato prima del requisito CRS): qui sotto test e' SOLO il gate CRS.
    let (_, input) =
        write_geometry_without_crs_fixture(directory.path(), Some(r#"{"dimensions":"xy"}"#));
    // geo.buffer dichiara `CrsRequirement::Projected`: si ferma in
    // validazione con la causa dichiarata (non l'ultimo tentativo di
    // lettura fallito), categoria d'errore CRS.
    let plan = directory.path().join("plan.json");
    let buffer_plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    });
    std::fs::write(&plan, serde_json::to_vec(&buffer_plan).expect("json")).expect("plan");

    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(
        !validate.status.success(),
        "geo.buffer su CRS mancante deve fallire"
    );
    let stderr = String::from_utf8_lossy(&validate.stderr);
    assert!(
        stderr.contains("nessun CRS dichiarato in alcuna rappresentazione accettata"),
        "stderr: {stderr}"
    );

    // Stesso esito in `run` (la validazione del piano e' il gate, mai lo
    // stream a meta' esecuzione).
    let output_path = directory.path().join("output.arrow");
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(!run.status.success(), "run deve fallire in validazione");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("nessun CRS dichiarato in alcuna rappresentazione accettata"),
        "stderr: {stderr}"
    );
    assert!(
        !output_path.exists(),
        "nessun output pubblicato su piano rifiutato"
    );
}

// ---------------------------------------------------------------------------
// R4.6.3 (BLOCK-08): `declared_unresolved` — preservato e propagato senza
// decisione, risolto solo da una decisione esplicita nel piano.
// ---------------------------------------------------------------------------

/// Fixture con chiavi canoniche: `id` Int64 + colonna `geometry` Binary con
/// le coppie canoniche date + `plenora.contract.version` sullo schema
/// (R2.5). Il campo porta anche il nome di estensione `geoarrow.wkb`:
/// richiesto dai kernel geo a runtime (la discovery riconosce le sole
/// chiavi canoniche, l'esecuzione no). Il batch contiene un punto WKB
/// (mai decodificato dai test tabellari).
fn canonical_crs_fixture(
    directory: &std::path::Path,
    plan: &serde_json::Value,
    pairs: &[(&str, &str)],
) -> (std::path::PathBuf, std::path::PathBuf) {
    use plenora_core::arrow::array::{ArrayRef, BinaryArray};
    use plenora_kernels_geo::arrow_adapter as adapter;
    let plan_path = directory.join("plan.json");
    let input = directory.join("input.arrow");
    std::fs::write(&plan_path, serde_json::to_vec(plan).expect("json")).expect("plan");
    let mut metadata: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    metadata.insert(
        adapter::GEOARROW_EXTENSION_KEY.to_owned(),
        adapter::GEOARROW_WKB_EXTENSION.to_owned(),
    );
    let geometry = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
    let schema = Arc::new(Schema::new_with_metadata(
        vec![Field::new("id", DataType::Int64, false), geometry],
        std::collections::HashMap::from([(
            adapter::PLENORA_CONTRACT_VERSION_KEY.to_owned(),
            "1".to_owned(),
        )]),
    ));
    let cells = [Some(point_wkb_le(0.0, 0.0)), Some(point_wkb_le(1.0, 1.0))];
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch fixture");
    write_ipc(&input, &schema, &[batch]);
    (plan_path, input)
}

/// Coppie canoniche del caso `crs_unresolved` del corpus di conformita':
/// `declared_unresolved` con `crs_id` non risolvibile (EPSG:99999) e
/// `axis_order` `unknown` (onesto: il CRS non si risolve).
fn crs_unresolved_pairs() -> Vec<(&'static str, &'static str)> {
    use plenora_kernels_geo::arrow_adapter as adapter;
    vec![
        (adapter::PLENORA_GEOMETRY_ENCODING_KEY, "wkb"),
        (adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy"),
        (adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "declared_unresolved"),
        (adapter::PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:99999"),
        (adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
    ]
}

/// Coppie canoniche del caso `conflicting_crs` del corpus: il produttore
/// dichiara `resolved` ma `crs_id` e `srid` discordano (R4.3.1).
fn conflicting_crs_pairs() -> Vec<(&'static str, &'static str)> {
    use plenora_kernels_geo::arrow_adapter as adapter;
    vec![
        (adapter::PLENORA_GEOMETRY_ENCODING_KEY, "wkb"),
        (adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy"),
        (adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
        (adapter::PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
        (adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lon_lat"),
        (adapter::PLENORA_GEOMETRY_SRID_KEY, "3003"),
    ]
}

/// Legge i metadati della colonna `geometry` di un output IPC.
fn geometry_metadata_of(path: &std::path::Path) -> std::collections::HashMap<String, String> {
    let reader =
        FileReader::try_new(std::fs::File::open(path).expect("output"), None).expect("reader");
    let schema = reader.schema();
    let (_, field) = schema.column_with_name("geometry").expect("geometry");
    field.metadata().clone()
}

#[test]
fn dag_v4_filter_propagates_declared_unresolved_unchanged() {
    // Il caso `crs_unresolved` del corpus (attesa preserve): il filtro
    // tabellare non richiede alcun CRS — validate e run passano SENZA
    // backend di risoluzione e le dichiarazioni arrivano al bordo di
    // scrittura invariate (R4.6.4). Cambio dichiarato: prima la discovery
    // risolveva una definizione risolvibile ed emetteva `resolved`.
    use plenora_kernels_geo::arrow_adapter as adapter;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) =
        canonical_crs_fixture(directory.path(), &filter_only_plan(), &crs_unresolved_pairs());

    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    let geometry = &summary["edges"][0]["contract"]["geometry"];
    assert_eq!(geometry["crs_resolution"], "declared_unresolved");

    let output_path = directory.path().join("output.arrow");
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let metadata = geometry_metadata_of(&output_path);
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY).map(String::as_str),
        Some("declared_unresolved"),
        "lo stato si propaga dichiarato (R4.6.4), mai collassato (R4.1)"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY).map(String::as_str),
        Some("EPSG:99999"),
        "dichiarazione originale ri-emessa invariata"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY).map(String::as_str),
        Some("unknown")
    );

    // Round-trip: l'output rientra come input dello stesso piano e ripassa.
    let revalidate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&output_path)
        .output()
        .expect("re-validate");
    assert!(
        revalidate.status.success(),
        "round-trip stderr: {}",
        String::from_utf8_lossy(&revalidate.stderr)
    );
}

#[test]
fn dag_v4_conflicting_crs_is_preserved_and_declared_not_reconciled() {
    // Il caso `conflicting_crs` del corpus (attesa per il centro: preserve).
    // Il centro NON fallisce (non e' il bordo di scrittura) e NON concilia
    // in silenzio: lo stato diventa `declared_unresolved` e le
    // rappresentazioni in conflitto attraversano invariate (R4.6.4) — saranno
    // il bordo di scrittura a fallire chiuso (R4.6.2).
    use plenora_kernels_geo::arrow_adapter as adapter;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) =
        canonical_crs_fixture(directory.path(), &filter_only_plan(), &conflicting_crs_pairs());

    let output_path = directory.path().join("output.arrow");
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "il centro preserva, non rifiuta — stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let metadata = geometry_metadata_of(&output_path);
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY).map(String::as_str),
        Some("declared_unresolved"),
        "l'incoerenza e' dichiarata, non silenziata dietro il `resolved` del produttore"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY).map(String::as_str),
        Some("EPSG:4326"),
        "crs_id originale preservato"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_SRID_KEY).map(String::as_str),
        Some("3003"),
        "srid originale preservato (lineage R2.4)"
    );
}

#[test]
fn dag_v4_geo_op_on_declared_unresolved_fails_with_distinct_cause() {
    // Gate R4.6.3: le op con CrsRequirement si fermano in validazione con
    // categoria Crs e un messaggio DISTINTO da `missing` — la colonna
    // dichiara un'incoerenza, non un'assenza.
    let directory = tempfile::tempdir().expect("tempdir");
    let buffer_plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    });
    let (plan, input) =
        canonical_crs_fixture(directory.path(), &buffer_plan, &crs_unresolved_pairs());
    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(!validate.status.success(), "geo.buffer deve fallire");
    let stderr = String::from_utf8_lossy(&validate.stderr);
    assert!(
        stderr.contains("declared_unresolved") && stderr.contains("decisione esplicita nel piano"),
        "stderr: {stderr}"
    );
}

#[test]
fn dag_v4_crs_decision_on_missing_crs_is_an_error() {
    // Una decisione su uno stato diverso da `declared_unresolved` non e'
    // applicabile (su `missing` sarebbe un CRS inventato, R4.4): errore
    // esplicito, mai ignorata in silenzio.
    let directory = tempfile::tempdir().expect("tempdir");
    let decision_plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "crs_decisions": {"main": "EPSG:32632"},
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
    });
    let (plan, input) = write_geometry_without_crs_fixture(directory.path(), None);
    std::fs::write(&plan, serde_json::to_vec(&decision_plan).expect("json")).expect("plan");
    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(!validate.status.success(), "decisione su missing deve fallire");
    let stderr = String::from_utf8_lossy(&validate.stderr);
    assert!(stderr.contains("non e' applicabile"), "stderr: {stderr}");
}

#[cfg(feature = "proj-backend")]
#[test]
fn dag_v4_crs_decision_resolves_declared_unresolved() {
    // R4.6.3: la decisione esplicita nel piano risolve l'incoerenza — con la
    // decisione il piano geo valida e l'output dichiara `resolved` con la
    // definizione decisa; le dichiarazioni in conflitto sono sostituite
    // (la decisione resta nel piano, coperta da plan_hash).
    use plenora_kernels_geo::arrow_adapter as adapter;
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "crs_decisions": {"main": "EPSG:32632"},
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    });
    let (plan, input) = canonical_crs_fixture(directory.path(), &plan, &crs_unresolved_pairs());

    let output_path = directory.path().join("output.arrow");
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "con la decisione il piano geo valida — stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let metadata = geometry_metadata_of(&output_path);
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY).map(String::as_str),
        Some("resolved")
    );
    // geo.buffer produce una NUOVA geometria nel CRS deciso: la definizione
    // dichiarata e' quella decisa, mai EPSG:99999.
    let declared = metadata
        .get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY)
        .or_else(|| metadata.get(adapter::PLENORA_GEOMETRY_CRS_DEFINITION_KEY))
        .map(String::as_str);
    assert_eq!(declared, Some("EPSG:32632"));
    assert!(
        !metadata.contains_key(adapter::PLENORA_GEOMETRY_SRID_KEY),
        "nessuna dichiarazione superstite dalla sorgente"
    );
}

// ---------------------------------------------------------------------------
// Piano v4 misto (tabellare + geo): richiede il backend PROJ per la
// risoluzione del CRS nella scoperta del contratto di input.
// ---------------------------------------------------------------------------

/// Fixture geo: `id` Int64 + colonna `geometry` GeoArrow-WKB (EPSG:32632).
#[cfg(feature = "proj-backend")]
fn geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        plenora_kernels_geo::arrow_adapter::geometry_output_field("geometry", "EPSG:32632")
            .expect("campo geometria"),
    ]))
}

#[cfg(feature = "proj-backend")]
fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    use geo::{Geometry, Point};
    use geozero::{CoordDimensions, ToWkb};

    Geometry::Point(Point::new(x, y))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb fixture")
}

#[cfg(feature = "proj-backend")]
fn geo_batch(ids: &[i64], cells: &[Option<Vec<u8>>]) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    RecordBatch::try_new(
        geo_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geo fixture")
}

/// Piano v4 misto: filter `id > 0` -> buffer(10) -> area.
#[cfg(feature = "proj-backend")]
fn mixed_plan() -> serde_json::Value {
    json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "f", "op": "table.filter", "in": ["main"],
             "config": {"column": "id", "operator": ">", "value": 0}},
            {"id": "b", "op": "geo.buffer", "in": ["f"], "config": {"distance": 10.0}},
            {"id": "a", "op": "geo.area", "in": ["b"], "config": {}},
        ],
        "output": "a",
    })
}

#[cfg(feature = "proj-backend")]
#[test]
fn dag_v4_misto_geo_end_to_end() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    let input = directory.path().join("input.arrow");
    let output_path = directory.path().join("output.arrow");
    std::fs::write(&plan, serde_json::to_vec(&mixed_plan()).expect("json")).expect("plan");
    write_ipc(
        &input,
        &geo_schema(),
        &[geo_batch(
            &[0, 1, 2],
            &[Some(point_wkb(0.0, 0.0)), Some(point_wkb(100.0, 100.0)), None],
        )],
    );

    // validate-only: il contratto di input e' scoperto dai metadati GeoArrow.
    let validate = cli()
        .args(["validate", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .output()
        .expect("validate");
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    let edges = summary["edges"].as_array().expect("archi");
    let geometry = &edges[0]["contract"]["geometry"];
    assert_eq!(geometry["name"], "geometry");
    assert_eq!(geometry["crs"], "EPSG:32632");
    assert_eq!(geometry["crs_kind"], "Projected");

    // run: esecuzione del DAG e metriche per nodo.
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&run.stdout).expect("JSON metriche");
    assert_eq!(metrics["status"], "ok");
    assert_eq!(metrics["output_rows"], 2);
    assert_eq!(metrics["nodes"]["b"]["operation"], "geo.buffer");
    assert_eq!(metrics["nodes"]["b"]["rows_out"], 2);
    assert_eq!(metrics["nodes"]["a"]["operation"], "geo.area");

    // Output: geometria preservata dal buffer, colonna `area` aggiunta.
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let batches: Vec<RecordBatch> = reader.map(|batch| batch.expect("batch")).collect();
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 2);
    let batch = &batches[0];
    assert!(batch.schema().field_with_name("geometry").is_ok());
    let (area_index, _) = batch.schema().column_with_name("area").expect("colonna area");
    let areas = batch
        .column(area_index)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("area Float64");
    // Buffer di un punto con raggio 10 ~ cerchio di area pi*100 (con
    // l'approssimazione poligonale di `geo`, ~306).
    let expected = 100.0 * std::f64::consts::PI;
    assert!(
        (areas.value(0) - expected).abs() < 15.0,
        "area del buffer: {} vs ~{expected}",
        areas.value(0)
    );
    assert!(areas.is_null(1), "null in -> null out");
}

/// Kill switch D12.9 via CLI: `--no-geo-fusion` e' accettato, il run
/// riesce e l'output e' identico al percorso fuso (la fusione non cambia
/// mai i byte, ADR-0012); il contatore dei fallback e' esposto nel JSON.
#[cfg(feature = "proj-backend")]
#[test]
fn run_v4_no_geo_fusion_output_identico_e_contatore_esposto() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    let input = directory.path().join("input.arrow");
    std::fs::write(&plan, serde_json::to_vec(&mixed_plan()).expect("json")).expect("plan");
    write_ipc(
        &input,
        &geo_schema(),
        &[geo_batch(
            &[0, 1, 2],
            &[Some(point_wkb(0.0, 0.0)), Some(point_wkb(100.0, 100.0)), None],
        )],
    );

    let mut outputs = Vec::new();
    for (label, extra_args) in [
        ("fuso", vec![]),
        ("non-fuso", vec!["--no-geo-fusion"]),
    ] {
        let output_path = directory.path().join(format!("output-{label}.arrow"));
        let mut command = cli();
        command
            .args(["run", "--plan"])
            .arg(&plan)
            .arg("--inputs")
            .arg(&input)
            .arg("--output")
            .arg(&output_path);
        command.args(extra_args);
        let run = command.output().expect("run");
        assert!(
            run.status.success(),
            "{label}, stderr: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let metrics: serde_json::Value =
            serde_json::from_slice(&run.stdout).expect("JSON metriche");
        assert_eq!(
            metrics["geo_fusion_fallbacks"], 0,
            "{label}: nessun fallback governor sulla fixture"
        );
        outputs.push(std::fs::read(&output_path).expect("output"));
    }
    assert_eq!(
        outputs[0], outputs[1],
        "l'output col kill switch spento deve essere byte-identico al fuso"
    );
}

/// Il contatore dei fallback e' esposto anche per piani senza geo (campo
/// top-level del JSON metriche, sempre presente).
#[test]
fn run_v4_espone_geo_fusion_fallbacks_anche_senza_geo() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");

    let result = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .arg("--no-geo-fusion")
        .output()
        .expect("run");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON metriche");
    assert_eq!(metrics["geo_fusion_fallbacks"], 0);
}

// ---------------------------------------------------------------------------
// Test di catena completa (plenora-contracts v2.0-rc10, Appendice A "cosa
// manca alla catena"): un dataset con coordinate Z, CRS risolto e
// axis_order dichiarato entra da un bordo, attraversa il centro (DAG v4) ed
// esce — le chiavi canoniche sopravvivono e i byte Z passano invariati.
// ---------------------------------------------------------------------------

/// Fixture di catena: colonna geometria con chiavi legacy `geo` (crs +
/// dimensions xyz) E chiavi canoniche coerenti (R2.6); versione di
/// protocollo R2.5 nei metadati di schema.
#[cfg(feature = "proj-backend")]
fn chain_schema() -> SchemaRef {
    use plenora_kernels_geo::arrow_adapter as adapter;
    let legacy = adapter::geometry_output_field("geometry", "EPSG:32632")
        .expect("campo geometria legacy");
    let mut metadata = legacy.metadata().clone();
    // Legacy: dimensions dichiarate xyz nel metadato `geo`.
    metadata.insert(
        adapter::GEO_METADATA_KEY.to_owned(),
        r#"{"crs":"EPSG:32632","dimensions":"xyz"}"#.to_owned(),
    );
    // Chiavi canoniche R2.2 coerenti con il legacy (R2.6).
    metadata.insert(
        adapter::PLENORA_GEOMETRY_ENCODING_KEY.to_owned(),
        "wkb".to_owned(),
    );
    metadata.insert(
        adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(),
        "xyz".to_owned(),
    );
    metadata.insert(
        adapter::PLENORA_GEOMETRY_TYPES_DECLARATION_KEY.to_owned(),
        "exact".to_owned(),
    );
    metadata.insert(
        adapter::PLENORA_GEOMETRY_TYPES_KEY.to_owned(),
        "point".to_owned(),
    );
    metadata.insert(
        adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
        "resolved".to_owned(),
    );
    metadata.insert(
        adapter::PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(),
        "EPSG:32632".to_owned(),
    );
    metadata.insert(
        adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
        "lat_lon".to_owned(),
    );
    let schema_metadata = std::collections::HashMap::from([(
        adapter::PLENORA_CONTRACT_VERSION_KEY.to_owned(),
        "1".to_owned(),
    )]);
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geometry", DataType::Binary, true).with_metadata(metadata),
        ],
        schema_metadata,
    ))
}

/// Punto 3D in WKB ISO little-endian (type code 1001): i byte Z
/// attraversano il centro invariati su operazioni tabellari (passthrough,
/// nessun decode).
#[cfg(feature = "proj-backend")]
fn point_z_wkb(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut payload = vec![1_u8];
    payload.extend_from_slice(&1001_u32.to_le_bytes());
    for value in [x, y, z] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload
}

#[cfg(feature = "proj-backend")]
// Lunghezza data dalla sequenza end-to-end CLI (fixture, run, asserzioni
// sull'output), non da complessita' logica.
#[allow(clippy::too_many_lines)]
#[test]
fn dag_v4_catena_completa_chiavi_canoniche_e_byte_z() {
    use plenora_kernels_geo::arrow_adapter as adapter;

    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    let input = directory.path().join("input.arrow");
    let output_path = directory.path().join("output.arrow");
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": 4,
            "inputs": ["main"],
            "nodes": [
                {"id": "f", "op": "table.filter", "in": ["main"],
                 "config": {"column": "id", "operator": ">", "value": 0}},
            ],
            "output": "f",
        }))
        .expect("json"),
    )
    .expect("plan");

    let cells = [
        Some(point_z_wkb(1.0, 2.0, 3.0)),
        Some(point_z_wkb(4.0, 5.0, 6.0)),
        Some(point_z_wkb(7.0, 8.0, 9.0)),
    ];
    let schema = chain_schema();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![0, 1, 2])) as ArrayRef,
            Arc::new(BinaryArray::from(
                cells.iter().map(|cell| cell.as_deref()).collect::<Vec<_>>(),
            )) as ArrayRef,
        ],
    )
    .expect("batch catena");
    write_ipc(&input, &schema, &[batch]);

    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run catena");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let out_schema = reader.schema();
    // R2.5: la versione di protocollo sopravvive (o e' riemessa) sullo
    // schema di output.
    assert_eq!(
        out_schema.metadata().get(adapter::PLENORA_CONTRACT_VERSION_KEY),
        Some(&"1".to_owned()),
        "plenora.contract.version sullo schema di output"
    );
    let (_, geometry_field) = out_schema
        .column_with_name("geometry")
        .expect("colonna geometria");
    let metadata = geometry_field.metadata();
    // Le chiavi canoniche attraversano il centro: dimensions xyz MAI
    // degradate a xy (i byte Z passano, il contratto resta onesto);
    // tipi dichiarati exact/point propagati; encoding e CRS coerenti.
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY),
        Some(&"xyz".to_owned()),
        "dimensions xyz preservate"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_TYPES_DECLARATION_KEY),
        Some(&"exact".to_owned()),
        "types_declaration propagata"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_TYPES_KEY),
        Some(&"point".to_owned()),
        "types propagati"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_ENCODING_KEY),
        Some(&"wkb".to_owned()),
        "encoding propagato"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY),
        Some(&"EPSG:32632".to_owned()),
        "crs_id propagato"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY),
        Some(&"resolved".to_owned()),
        "crs_resolution propagata"
    );
    // axis_order: la lineage (R2.4) preserva il valore dichiarato in
    // ingresso — `unknown` dell'emissione non e' una dichiarazione ma
    // l'assenza di una (il DataContract non lo modella, ADR-0009) e non
    // puo' sovrascrivere una chiave presente (R2.7). lat_lon attraversa
    // il centro.
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY),
        Some(&"lat_lon".to_owned()),
        "axis_order lat_lon preservato dalla lineage"
    );

    // I byte Z delle righe sopravvissute (id 1 e 2) sono BIT-IDENTICI
    // all'ingresso: passthrough tabellare, nessun decode/encode.
    let batches: Vec<RecordBatch> = reader.map(|batch| batch.expect("batch")).collect();
    assert_eq!(batches.len(), 1);
    let out_cells = batches[0]
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("geometry Binary");
    assert_eq!(out_cells.len(), 2);
    assert_eq!(out_cells.value(0), cells[1].as_deref().expect("cella 1"));
    assert_eq!(out_cells.value(1), cells[2].as_deref().expect("cella 2"));
}

// ---------------------------------------------------------------------------
// ADR-0009 decisione 8: op che riscrivono fatti canonici (CRS di `reproject`,
// tipi geometrici dei type-changer) — sostituzione, mai conflitto R2.6.
// Minore 1/2: colonna geometria identificata dalle sole chiavi canoniche.
// ---------------------------------------------------------------------------

/// Fixture: `id` Int64 + colonna `geometry` GeoArrow-WKB con CRS EPSG:32632 e
/// chiavi canoniche CRS complete (`crs_resolution`/`crs_id`/`srid`/
/// `axis_order`).
#[cfg(feature = "proj-backend")]
fn canonical_crs_schema() -> SchemaRef {
    use plenora_kernels_geo::arrow_adapter as adapter;
    let metadata = std::collections::HashMap::from([
        (
            adapter::GEOARROW_EXTENSION_KEY.to_owned(),
            adapter::GEOARROW_WKB_EXTENSION.to_owned(),
        ),
        (
            adapter::GEO_METADATA_KEY.to_owned(),
            r#"{"crs":"EPSG:32632","dimensions":"xy"}"#.to_owned(),
        ),
        (
            adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(),
            "xy".to_owned(),
        ),
        (
            adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
            "resolved".to_owned(),
        ),
        (
            adapter::PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(),
            "EPSG:32632".to_owned(),
        ),
        (adapter::PLENORA_GEOMETRY_SRID_KEY.to_owned(), "32632".to_owned()),
        (
            adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
            "easting_northing".to_owned(),
        ),
    ]);
    let schema_metadata = std::collections::HashMap::from([(
        adapter::PLENORA_CONTRACT_VERSION_KEY.to_owned(),
        "1".to_owned(),
    )]);
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geometry", DataType::Binary, true).with_metadata(metadata),
        ],
        schema_metadata,
    ))
}

/// Fixture: `id` Int64 + colonna `geometry` identificata dalle SOLE chiavi
/// canoniche (niente `ARROW:extension:name`, niente metadato `geo`).
#[cfg(feature = "proj-backend")]
fn canonical_only_schema() -> SchemaRef {
    use plenora_kernels_geo::arrow_adapter as adapter;
    let metadata = std::collections::HashMap::from([
        (
            adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(),
            "xy".to_owned(),
        ),
        (
            adapter::PLENORA_GEOMETRY_ENCODING_KEY.to_owned(),
            "wkb".to_owned(),
        ),
        (
            adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
            "resolved".to_owned(),
        ),
        (
            adapter::PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(),
            "EPSG:32632".to_owned(),
        ),
        (
            adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
            "unknown".to_owned(),
        ),
    ]);
    let schema_metadata = std::collections::HashMap::from([(
        adapter::PLENORA_CONTRACT_VERSION_KEY.to_owned(),
        "1".to_owned(),
    )]);
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geometry", DataType::Binary, true).with_metadata(metadata),
        ],
        schema_metadata,
    ))
}

#[cfg(feature = "proj-backend")]
fn write_fixture_batch(path: &std::path::Path, schema: &SchemaRef) {
    let cells = [
        Some(point_wkb_le(500_000.0, 4_649_776.0)),
        Some(point_wkb_le(500_100.0, 4_649_876.0)),
    ];
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    let batch = RecordBatch::try_new(
        Arc::clone(schema),
        vec![
            Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch fixture");
    write_ipc(path, schema, &[batch]);
}

#[cfg(feature = "proj-backend")]
fn run_plan(
    directory: &std::path::Path,
    plan: &serde_json::Value,
    schema: &SchemaRef,
) -> std::path::PathBuf {
    let plan_path = directory.join("plan.json");
    let input = directory.join("input.arrow");
    let output_path = directory.join("output.arrow");
    std::fs::write(&plan_path, serde_json::to_vec(plan).expect("json")).expect("plan");
    write_fixture_batch(&input, schema);
    let run = cli()
        .args(["run", "--plan"])
        .arg(&plan_path)
        .arg("--inputs")
        .arg(&input)
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    output_path
}

/// Reperto 2, end-to-end: `geo.reproject` con chiavi canoniche della sorgente
/// sul campo — il contratto dice il target e l'esecuzione arriva in fondo
/// (prima il guard R2.6 rifiutava l'output).
#[cfg(feature = "proj-backend")]
#[test]
fn dag_v4_reproject_replaces_canonical_crs_keys() {
    use plenora_kernels_geo::arrow_adapter as adapter;
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "p", "op": "geo.reproject", "in": ["main"],
             "config": {"target_crs": "EPSG:4326"}},
        ],
        "output": "p",
    });
    let output_path = run_plan(directory.path(), &plan, &canonical_crs_schema());

    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let out_schema = reader.schema();
    let (_, geometry_field) = out_schema
        .column_with_name("geometry")
        .expect("colonna geometria");
    let metadata = geometry_field.metadata();
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY),
        Some(&"EPSG:4326".to_owned()),
        "il CRS emesso e' il target"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY),
        Some(&"resolved".to_owned())
    );
    assert!(
        !metadata.contains_key(adapter::PLENORA_GEOMETRY_SRID_KEY),
        "srid della sorgente sostituito, non propagato"
    );
    assert!(
        !metadata.contains_key(adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY)
            || metadata.get(adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY)
                == Some(&"unknown".to_owned()),
        "axis_order della sorgente non sopravvive"
    );
    let geo = metadata
        .get(adapter::GEO_METADATA_KEY)
        .expect("geo metadata");
    assert!(geo.contains("EPSG:4326"), "geo.crs legacy al target: {geo}");
}

/// Minori 1+2 + reperto 1, end-to-end: tabella la cui colonna geometria si
/// dichiara con le sole chiavi canoniche — accettata (estensione ammessa,
/// non richiesta), eseguita, e l'output dichiara i tipi dell'operazione.
#[cfg(feature = "proj-backend")]
#[test]
fn dag_v4_canonical_only_geometry_executes_and_emits_output_types() {
    use plenora_kernels_geo::arrow_adapter as adapter;
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = json!({
        "schema_version": 4,
        "inputs": ["main"],
        "nodes": [
            {"id": "c", "op": "geo.centroid", "in": ["main"], "config": {}},
        ],
        "output": "c",
    });
    let output_path = run_plan(directory.path(), &plan, &canonical_only_schema());

    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let out_schema = reader.schema();
    let (_, geometry_field) = out_schema
        .column_with_name("geometry")
        .expect("colonna geometria");
    let metadata = geometry_field.metadata();
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_TYPES_DECLARATION_KEY),
        Some(&"exact".to_owned()),
        "i tipi emessi sono quelli dell'operazione"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_TYPES_KEY),
        Some(&"point".to_owned()),
        "centroid -> point"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY),
        Some(&"EPSG:32632".to_owned()),
        "CRS preservato"
    );
    // Le righe ci sono tutte (esecuzione completata, mai un rifiuto a meta').
    let rows: usize = reader
        .map(|batch| batch.expect("batch").num_rows())
        .sum();
    assert_eq!(rows, 2);
}
