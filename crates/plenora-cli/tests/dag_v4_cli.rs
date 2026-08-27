//! Test end-to-end della CLI sul DAG: `validate` e `run` di piani
//! `schema_version: 5` attraverso planner/executor di `plenora-engine`, con
//! il piano legacy che continua a funzionare invariato e i piani v4 che
//! entrano dalla migrazione (errori-e-limiti.md#memoria-governata).
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

/// Invoca `run --plan <piano> --inputs <ingresso> --output <uscita>`.
///
/// Rende l'uscita del processo **grezza**: codice, stdout e stderr restano da
/// esaminare a chi chiama, perche' sono gli oracoli e non possono stare qui.
fn cli_run(
    piano: &std::path::Path,
    ingresso: &std::path::Path,
    uscita: &std::path::Path,
) -> std::process::Output {
    cli()
        .args(["run", "--plan"])
        .arg(piano)
        .arg("--inputs")
        .arg(ingresso)
        .arg("--output")
        .arg(uscita)
        .output()
        .expect("il processo si avvia")
}

/// Invoca `validate --plan <piano> --inputs <ingresso>`.
///
/// Come [`cli_run`]: l'uscita torna grezza.
fn cli_validate(piano: &std::path::Path, ingresso: &std::path::Path) -> std::process::Output {
    cli()
        .args(["validate", "--plan"])
        .arg(piano)
        .arg("--inputs")
        .arg(ingresso)
        .output()
        .expect("il processo si avvia")
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
        "schema_version": 5,
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

    let result = cli_validate(&plan, &input);
    assert!(
        result.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["schema_version"], 5);
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
    assert_eq!(
        field_names,
        ["id", "label"],
        "la rinomina e' inferita a secco"
    );

    let segments = summary["segments"].as_array().expect("segmenti");
    assert_eq!(segments.len(), 1, "catena streaming fusa in un segmento");
    assert_eq!(segments[0]["mode"], "LinearStreaming");
    assert_eq!(segments[0]["parallelism"], "SerialFused");
    assert_eq!(segments[0]["nodes"], json!(["f", "r"]));

    assert_eq!(
        summary["required_capabilities"],
        json!(["atomic_publish"]),
        "piano solo tabellare: solo il profilo di publish di default (errori-e-limiti.md#publish-e-cleanup)"
    );
    assert_eq!(
        summary["input_contract_fingerprints"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[test]
fn run_v4_scrive_output_e_metriche_json_su_stdout() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");

    let result = cli_run(&plan, &input, &output_path);
    assert!(
        result.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
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
    assert!(metrics["segments"]
        .as_object()
        .is_some_and(|s| !s.is_empty()));

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
    let stderr = String::from_utf8_lossy(&result.stdout);
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
    // Golden di canale: l'envelope vive su STDOUT e stderr resta vuoto,
    // come in `plenora-database-tools`.
    assert!(
        result.stderr.is_empty(),
        "stderr deve restare vuoto: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("stdout deve essere l'envelope JSON par. 9");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["protocol_version"], 1);
    for axis in ["category", "phase", "remote_effect", "message"] {
        assert!(
            envelope["error"]
                .get(axis)
                .is_some_and(serde_json::Value::is_string),
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
            "schema_version": 5,
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

    let result = cli_run(&plan, &input, &output_path);
    assert!(!result.status.success());
    assert!(!result.stdout.is_empty(), "errore diagnostico presente");
    assert!(
        result.stderr.is_empty(),
        "l'envelope va su stdout e stderr resta vuoto"
    );
    assert!(!output_path.exists(), "nessun output parziale");
}

#[test]
fn run_v4_non_sovrascrive_un_output_esistente() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("output.arrow");
    std::fs::write(&output_path, b"contenuto precedente").expect("output preesistente");

    let result = cli_run(&plan, &input, &output_path);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stdout);
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
    write_ipc(
        &input,
        &table_schema(),
        &[table_batch(&[1, 2], &["a", "b"])],
    );

    // validate legacy: riepilogo di Fase 1, senza i campi del DAG v4.
    let validate = cli_validate(&plan, &input);
    assert!(
        validate.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&validate.stdout)
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
        "stdout: {}",
        String::from_utf8_lossy(&run.stdout)
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
        "schema_version": 5,
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
    std::fs::write(
        &plan,
        serde_json::to_vec(&filter_only_plan()).expect("json"),
    )
    .expect("plan");
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
    let validate = cli_validate(&plan, &input);
    assert!(
        validate.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&validate.stdout)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    let edges = summary["edges"].as_array().expect("archi");
    let geometry = &edges[0]["contract"]["geometry"];
    assert_eq!(geometry["name"], "geometry");
    assert_eq!(geometry["crs"], serde_json::Value::Null);
    assert_eq!(geometry["crs_resolution"], "missing");

    // run: i byte geometrici transitano invariati (mai decodificati).
    let output_path = directory.path().join("output.arrow");
    let run = cli_run(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&run.stdout)
    );
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let schema = reader.schema();
    // R4.6.4: lo stato mancante arriva al bordo di scrittura dichiarato —
    // `crs_resolution = missing` e NESSUNA chiave CRS (R2.2), mai un CRS
    // inventato (R4.4), mai la dichiarazione persa.
    let (_, geometry_field) = schema
        .column_with_name("geometry")
        .expect("colonna geometry");
    let metadata = geometry_field.metadata();
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
            .map(String::as_str),
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
        metadata
            .get(adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY)
            .map(String::as_str),
        Some("unknown")
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_ENCODING_KEY)
            .map(String::as_str),
        Some("wkb")
    );
    assert_eq!(
        schema
            .metadata()
            .get(adapter::PLENORA_CONTRACT_VERSION_KEY)
            .map(String::as_str),
        Some("1"),
        "versione di protocollo R2.5 con chiavi canoniche"
    );

    // Round-trip R4.6.4: l'output (con le chiavi canoniche `missing`)
    // rientra come input dello stesso piano e ripassa — la dichiarazione
    // sopravvive al giro bordo-centro-bordo senza risoluzioni.
    let revalidate = cli_validate(&plan, &output_path);
    assert!(
        revalidate.status.success(),
        "round-trip stdout: {}",
        String::from_utf8_lossy(&revalidate.stdout)
    );
}

#[cfg(feature = "proj-backend")]
#[test]
fn dag_v4_geo_pregate_wkb_rejection_carries_authoritative_step_context() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("output.arrow");
    let plan_json = json!({
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": [
            {"id": "centroid-node", "op": "geo.centroid", "in": ["main"], "config": {}}
        ],
        "output": "centroid-node"
    });
    let (plan, input) =
        canonical_crs_fixture(directory.path(), &plan_json, &monte_mario_resolved_pairs());
    let schema = FileReader::try_new(std::fs::File::open(&input).expect("input"), None)
        .expect("reader")
        .schema();
    let invalid = [vec![0x01, 0x09, 0x00]];
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![7_i64])) as ArrayRef,
            Arc::new(
                invalid
                    .iter()
                    .map(|wkb| Some(wkb.as_slice()))
                    .collect::<BinaryArray>(),
            ) as ArrayRef,
        ],
    )
    .expect("batch WKB invalido");
    write_ipc(&input, &schema, &[batch]);

    let result = cli_run(&plan, &input, &output);

    // WKB invalido: categoria `data_mapping` -> exit 3 (cli.md#exit-code).
    assert_eq!(result.status.code(), Some(3));
    assert!(
        result.stderr.is_empty(),
        "stderr deve restare vuoto: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.exists(), "nessun output parziale");
    let envelope: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("envelope JSON");
    assert_eq!(
        envelope["error"]["context"]["node"], "centroid-node",
        "{envelope}"
    );
    assert_eq!(envelope["error"]["context"]["operation"], "geo.centroid");
    let execution_id = envelope["error"]["context"]["execution_id"]
        .as_str()
        .expect("execution_id autorevole");
    assert!(execution_id.starts_with("exec-"), "{execution_id}");
    assert_eq!(
        envelope["error"]["row_diagnostics"],
        json!({
            "contract": "plenora-row-diagnostics-v1",
            "scope": "read",
            "index_basis": "source_row_zero_based",
            "completeness": "complete",
            "observed_total": 1,
            "total": 1,
            "counts": {"geometry.invalid_wkb": 1},
            "examples_limit": 10,
            "examples_truncated": false,
            "examples": [{
                "source_index": 0,
                "cause": "geometry.invalid_wkb",
                "column": "geometry"
            }]
        }),
        "la diagnostica row-scoped resta invariata"
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
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    });
    std::fs::write(&plan, serde_json::to_vec(&buffer_plan).expect("json")).expect("plan");

    let validate = cli_validate(&plan, &input);
    assert!(
        !validate.status.success(),
        "geo.buffer su CRS mancante deve fallire"
    );
    let stderr = String::from_utf8_lossy(&validate.stdout);
    assert!(
        stderr.contains("nessun CRS dichiarato in alcuna rappresentazione accettata"),
        "stderr: {stderr}"
    );

    // Stesso esito in `run` (la validazione del piano e' il gate, mai lo
    // stream a meta' esecuzione).
    let output_path = directory.path().join("output.arrow");
    let run = cli_run(&plan, &input, &output_path);
    assert!(!run.status.success(), "run deve fallire in validazione");
    let stderr = String::from_utf8_lossy(&run.stdout);
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
        (
            adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
            "declared_unresolved",
        ),
        (adapter::PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:99999"),
        (adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
    ]
}

/// Coppie canoniche del caso `conflicting_crs` del corpus: `crs_id` e
/// `srid` discordano (R4.3.1). La fixture non dichiara `crs_resolution`, ma
/// il conflitto numerico viene preservato anche quando il produttore dichiara
/// `resolved`: una dichiarazione esplicita non puo' nascondere H-06.
fn conflicting_crs_pairs() -> Vec<(&'static str, &'static str)> {
    use plenora_kernels_geo::arrow_adapter as adapter;
    vec![
        (adapter::PLENORA_GEOMETRY_ENCODING_KEY, "wkb"),
        (adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy"),
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
    let (plan, input) = canonical_crs_fixture(
        directory.path(),
        &filter_only_plan(),
        &crs_unresolved_pairs(),
    );

    let validate = cli_validate(&plan, &input);
    assert!(
        validate.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&validate.stdout)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    let geometry = &summary["edges"][0]["contract"]["geometry"];
    assert_eq!(geometry["crs_resolution"], "declared_unresolved");

    let output_path = directory.path().join("output.arrow");
    let run = cli_run(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&run.stdout)
    );
    let metadata = geometry_metadata_of(&output_path);
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
            .map(String::as_str),
        Some("declared_unresolved"),
        "lo stato si propaga dichiarato (R4.6.4), mai collassato (R4.1)"
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY)
            .map(String::as_str),
        Some("EPSG:99999"),
        "dichiarazione originale ri-emessa invariata"
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY)
            .map(String::as_str),
        Some("unknown")
    );

    // Round-trip: l'output rientra come input dello stesso piano e ripassa.
    let revalidate = cli_validate(&plan, &output_path);
    assert!(
        revalidate.status.success(),
        "round-trip stdout: {}",
        String::from_utf8_lossy(&revalidate.stdout)
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
    let (plan, input) = canonical_crs_fixture(
        directory.path(),
        &filter_only_plan(),
        &conflicting_crs_pairs(),
    );

    let output_path = directory.path().join("output.arrow");
    let run = cli_run(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "il centro preserva, non rifiuta — stdout: {}",
        String::from_utf8_lossy(&run.stdout)
    );
    let metadata = geometry_metadata_of(&output_path);
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
            .map(String::as_str),
        Some("declared_unresolved"),
        "input non dichiarato con conflitto decidibile: l'incoerenza e' preservata e dichiarata"
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY)
            .map(String::as_str),
        Some("EPSG:4326"),
        "crs_id originale preservato"
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_SRID_KEY)
            .map(String::as_str),
        Some("3003"),
        "srid originale preservato (lineage R2.4)"
    );
}

/// Coppie canoniche della catena `MySQL` TLS Database→Data: il provider
/// conosce lo SRID numerico dal catalogo ma non puo' inventare l'autorita'
/// (R4.4) — dichiara `declared_unresolved` con SOLO `srid`, senza
/// `crs_id`/`crs_definition`/`axis_order` e senza metadato legacy `geo`.
/// R4.3.1: lo SRID numerico e' una rappresentazione CRS (dopo definizione e
/// identificatore), quindi la dichiarazione e' legittima.
fn mysql_srid_only_unresolved_pairs() -> Vec<(&'static str, &'static str)> {
    use plenora_kernels_geo::arrow_adapter as adapter;
    vec![
        (adapter::PLENORA_GEOMETRY_ENCODING_KEY, "wkb"),
        (adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy"),
        (
            adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
            "declared_unresolved",
        ),
        (adapter::PLENORA_GEOMETRY_SRID_KEY, "4326"),
    ]
}

#[test]
fn dag_v4_filter_accepts_srid_only_declared_unresolved_without_synthesis() {
    // Reproducer della catena MySQL TLS: IPC con `srid=4326` +
    // `declared_unresolved`, senza crs_id/definition/legacy geo, piano
    // identity `table.filter`. Prima del fix la discovery falliva con
    // «declared_unresolved ma nessun CRS e' dichiarato in alcuna
    // rappresentazione accettata»: lo SRID non contava come
    // rappresentazione. Ora validate e run passano SENZA backend e lo SRID
    // attraversa invariato via lineage (R2.4), senza che il centro
    // sintetizzi crs_id, definizione o axis_order (R4.4).
    use plenora_kernels_geo::arrow_adapter as adapter;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = canonical_crs_fixture(
        directory.path(),
        &filter_only_plan(),
        &mysql_srid_only_unresolved_pairs(),
    );

    let validate = cli_validate(&plan, &input);
    assert!(
        validate.status.success(),
        "stdout: {} — stderr: {}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stdout)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    let geometry = &summary["edges"][0]["contract"]["geometry"];
    assert_eq!(geometry["crs_resolution"], "declared_unresolved");
    assert_eq!(
        geometry["crs"],
        serde_json::Value::Null,
        "nessun CRS risolto da dichiarare: lo stato non e' promosso"
    );

    let output_path = directory.path().join("output.arrow");
    let run = cli_run(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "stdout: {} — stderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stdout)
    );
    let metadata = geometry_metadata_of(&output_path);
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
            .map(String::as_str),
        Some("declared_unresolved"),
        "lo stato si propaga dichiarato (R4.6.4), mai collassato (R4.1)"
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_SRID_KEY)
            .map(String::as_str),
        Some("4326"),
        "SRID originale preservato dalla lineage (R2.4)"
    );
    for key in [
        adapter::PLENORA_GEOMETRY_CRS_ID_KEY,
        adapter::PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
        adapter::PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY,
        adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY,
    ] {
        assert!(
            !metadata.contains_key(key),
            "chiave `{key}` sintetizzata dal centro (R4.4: vietato)"
        );
    }

    // Round-trip: l'output rientra come input dello stesso piano e ripassa.
    let revalidate = cli_validate(&plan, &output_path);
    assert!(
        revalidate.status.success(),
        "round-trip stdout: {} — stderr: {}",
        String::from_utf8_lossy(&revalidate.stdout),
        String::from_utf8_lossy(&revalidate.stdout)
    );
}

/// WKT1 realistico di Monte Mario / Italy zone 1 con `AUTHORITY` e
/// `TOWGS84` (EPSG:3003): la forma dello shapefile catastale owner.
#[cfg(feature = "proj-backend")]
const MONTE_MARIO_WKT: &str = concat!(
    r#"PROJCS["Monte Mario / Italy zone 1",GEOGCS["Monte Mario","#,
    r#"DATUM["Monte_Mario",SPHEROID["International 1924",6378388,297],"#,
    r#"TOWGS84[-104.1,-49.1,-9.9,0.971,-2.917,0.714,-11.68]],"#,
    r#"PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],"#,
    r#"PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],"#,
    r#"PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],"#,
    r#"PARAMETER["false_easting",1500000],PARAMETER["false_northing",0],"#,
    r#"UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH],"#,
    r#"AUTHORITY["EPSG","3003"]]"#
);

/// Coppie canoniche del caso owner: input `resolved` con doppia
/// rappresentazione coerente — `crs_id` EPSG:3003 + definizione WKT Monte
/// Mario (`wkt`) + `axis_order` `easting_northing`.
#[cfg(feature = "proj-backend")]
fn monte_mario_resolved_pairs() -> Vec<(&'static str, &'static str)> {
    use plenora_kernels_geo::arrow_adapter as adapter;
    vec![
        (adapter::PLENORA_GEOMETRY_ENCODING_KEY, "wkb"),
        (adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy"),
        (adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
        (adapter::PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:3003"),
        (
            adapter::PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
            MONTE_MARIO_WKT,
        ),
        (adapter::PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt"),
        (adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY, "easting_northing"),
    ]
}

#[cfg(feature = "proj-backend")]
#[test]
fn dag_v4_filter_preserves_resolved_wkt_double_representation() {
    // REPRODUCER del caso owner (shapefile catastale EPSG:3003): input
    // `resolved` con doppia rappresentazione WKT coerente + `table.filter`
    // su colonna numerica. Prima dell'emendamento 2026-07-31 (classe A) la
    // discovery rovesciava il `resolved` in `declared_unresolved` (regola
    // (2a) non condizionata) e R4.6.3 bloccava la riproiezione a valle;
    // ora la risoluzione + la verifica di coerenza confermano il `resolved`
    // e l'output ri-emette la definizione WKT col suo formato (classe B:
    // passthrough, mai WKT in `crs_id`).
    use plenora_kernels_geo::arrow_adapter as adapter;
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = canonical_crs_fixture(
        directory.path(),
        &filter_only_plan(),
        &monte_mario_resolved_pairs(),
    );

    let validate = cli_validate(&plan, &input);
    assert!(
        validate.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&validate.stdout)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    assert_eq!(
        summary["edges"][0]["contract"]["geometry"]["crs_resolution"], "resolved",
        "la doppia rappresentazione coerente resta resolved"
    );

    let output_path = directory.path().join("output.arrow");
    let run = cli_run(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&run.stdout)
    );
    let metadata = geometry_metadata_of(&output_path);
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
            .map(String::as_str),
        Some("resolved")
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_DEFINITION_KEY)
            .map(String::as_str),
        Some(MONTE_MARIO_WKT),
        "definizione WKT ri-emessa byte-per-byte (classe B)"
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY)
            .map(String::as_str),
        Some("EPSG:3003"),
        "identificatore d'autorita' originale preservato"
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY)
            .map(String::as_str),
        Some("wkt")
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_SRID_KEY)
            .map(String::as_str),
        Some("3003"),
        "srid dedotto dal canonical del risolto riempie l'assente"
    );
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY)
            .map(String::as_str),
        Some("easting_northing"),
        "lineage preservata (R2.7: mai arbitrato)"
    );
}

#[test]
fn dag_v4_geo_op_on_declared_unresolved_fails_with_distinct_cause() {
    // Gate R4.6.3: le op con CrsRequirement si fermano in validazione con
    // categoria Crs e un messaggio DISTINTO da `missing` — la colonna
    // dichiara un'incoerenza, non un'assenza.
    let directory = tempfile::tempdir().expect("tempdir");
    let buffer_plan = json!({
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    });
    let (plan, input) =
        canonical_crs_fixture(directory.path(), &buffer_plan, &crs_unresolved_pairs());
    let validate = cli_validate(&plan, &input);
    assert!(!validate.status.success(), "geo.buffer deve fallire");
    let stderr = String::from_utf8_lossy(&validate.stdout);
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
        "schema_version": 5,
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
    let validate = cli_validate(&plan, &input);
    assert!(
        !validate.status.success(),
        "decisione su missing deve fallire"
    );
    let stderr = String::from_utf8_lossy(&validate.stdout);
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
        "schema_version": 5,
        "inputs": ["main"],
        "crs_decisions": {"main": "EPSG:32632"},
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 1.0}},
        ],
        "output": "b",
    });
    let (plan, input) = canonical_crs_fixture(directory.path(), &plan, &crs_unresolved_pairs());

    let output_path = directory.path().join("output.arrow");
    let run = cli_run(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "con la decisione il piano geo valida — stdout: {}",
        String::from_utf8_lossy(&run.stdout)
    );
    let metadata = geometry_metadata_of(&output_path);
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
            .map(String::as_str),
        Some("resolved")
    );
    // geo.buffer produce una NUOVA geometria nel CRS deciso: la definizione
    // dichiarata e' quella decisa, mai EPSG:99999.
    let declared = metadata
        .get(adapter::PLENORA_GEOMETRY_CRS_ID_KEY)
        .or_else(|| metadata.get(adapter::PLENORA_GEOMETRY_CRS_DEFINITION_KEY))
        .map(String::as_str);
    assert_eq!(declared, Some("EPSG:32632"));
    assert_eq!(
        metadata
            .get(adapter::PLENORA_GEOMETRY_SRID_KEY)
            .map(String::as_str),
        Some("32632"),
        "srid del CRS DECISO dedotto dalla definizione d'autorita' (emendamento \
         2026-07-31), non una dichiarazione superstite dalla sorgente"
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

/// Piano v4 misto: buffer(10) -> area -> filter `id > 0`.
#[cfg(feature = "proj-backend")]
fn mixed_plan() -> serde_json::Value {
    // Le op geo (diagnostica row-scoped) precedono il filter
    // cardinality-changing: il gate provenance rifiuterebbe l'ordine
    // inverso; le asserzioni sulle righe filtrate sono invariate.
    json!({
        "schema_version": 5,
        "inputs": ["main"],
        "nodes": [
            {"id": "b", "op": "geo.buffer", "in": ["main"], "config": {"distance": 10.0}},
            {"id": "a", "op": "geo.area", "in": ["b"], "config": {}},
            {"id": "f", "op": "table.filter", "in": ["a"],
             "config": {"column": "id", "operator": ">", "value": 0}},
        ],
        "output": "f",
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
            &[
                Some(point_wkb(0.0, 0.0)),
                Some(point_wkb(100.0, 100.0)),
                None,
            ],
        )],
    );

    // validate-only: il contratto di input e' scoperto dai metadati GeoArrow.
    let validate = cli_validate(&plan, &input);
    assert!(
        validate.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&validate.stdout)
    );
    let summary: serde_json::Value = serde_json::from_slice(&validate.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    let edges = summary["edges"].as_array().expect("archi");
    let geometry = &edges[0]["contract"]["geometry"];
    assert_eq!(geometry["name"], "geometry");
    assert_eq!(geometry["crs"], "EPSG:32632");
    assert_eq!(geometry["crs_kind"], "Projected");

    // run: esecuzione del DAG e metriche per nodo.
    let run = cli_run(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&run.stdout)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&run.stdout).expect("JSON metriche");
    assert_eq!(metrics["status"], "ok");
    assert_eq!(metrics["output_rows"], 2);
    assert_eq!(metrics["nodes"]["b"]["operation"], "geo.buffer");
    assert_eq!(metrics["nodes"]["b"]["rows_out"], 3);
    assert_eq!(metrics["nodes"]["a"]["operation"], "geo.area");
    assert_eq!(metrics["nodes"]["f"]["rows_out"], 2);

    // Output: geometria preservata dal buffer, colonna `area` aggiunta.
    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let batches: Vec<RecordBatch> = reader.map(|batch| batch.expect("batch")).collect();
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(rows, 2);
    let batch = &batches[0];
    assert!(batch.schema().field_with_name("geometry").is_ok());
    let (area_index, _) = batch
        .schema()
        .column_with_name("area")
        .expect("colonna area");
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
/// mai i byte, architettura.md#geometrie); il contatore dei fallback e' esposto nel JSON.
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
            &[
                Some(point_wkb(0.0, 0.0)),
                Some(point_wkb(100.0, 100.0)),
                None,
            ],
        )],
    );

    let mut outputs = Vec::new();
    let mut avviati = Vec::new();
    for (label, extra_args) in [("fuso", vec![]), ("non-fuso", vec!["--no-geo-fusion"])] {
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
            "{label}, stdout: {}",
            String::from_utf8_lossy(&run.stdout)
        );
        let metrics: serde_json::Value =
            serde_json::from_slice(&run.stdout).expect("JSON metriche");
        assert_eq!(
            metrics["geo_fusion_fallbacks"], 0,
            "{label}: nessun fallback governor sulla fixture"
        );
        avviati.push(
            metrics["geo_fusion_groups_started"]
                .as_u64()
                .unwrap_or_else(|| {
                    panic!("{label}: geo_fusion_groups_started assente o non intero")
                }),
        );
        outputs.push(std::fs::read(&output_path).expect("output"));
    }
    // Senza questi due l'identita' degli output sarebbe compatibile con la
    // fusione mai eseguita: due volte lo stesso percorso generico.
    assert_eq!(
        avviati[0], 1,
        "un batch nella fixture, un ingresso nel runner fuso"
    );
    assert_eq!(avviati[1], 0, "il kill switch non ha spento il runner fuso");
    assert_eq!(
        outputs[0], outputs[1],
        "l'output col kill switch spento deve essere byte-identico al fuso"
    );
}

/// I contatori della fusione geometrica sono esposti anche per piani senza
/// geo (campi top-level del JSON metriche, sempre presenti).
#[test]
fn run_v4_espone_contatori_geo_fusion_anche_senza_geo() {
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
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON metriche");
    assert_eq!(metrics["geo_fusion_fallbacks"], 0);
    assert_eq!(metrics["geo_fusion_groups_started"], 0);
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
    let legacy =
        adapter::geometry_output_field("geometry", "EPSG:32632").expect("campo geometria legacy");
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
            "schema_version": 5,
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

    let run = cli_run(&plan, &input, &output_path);
    assert!(
        run.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&run.stdout)
    );

    let reader = FileReader::try_new(std::fs::File::open(&output_path).expect("output"), None)
        .expect("reader");
    let out_schema = reader.schema();
    // R2.5: la versione di protocollo sopravvive (o e' riemessa) sullo
    // schema di output.
    assert_eq!(
        out_schema
            .metadata()
            .get(adapter::PLENORA_CONTRACT_VERSION_KEY),
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
    // l'assenza di una (il DataContract non lo modella, piano-v5.md#contratti-di-input) e non
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
// piano-v5.md#contratti-di-input decisione 8: op che riscrivono fatti canonici (CRS di `reproject`,
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
        (
            adapter::PLENORA_GEOMETRY_SRID_KEY.to_owned(),
            "32632".to_owned(),
        ),
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
    let run = cli_run(&plan_path, &input, &output_path);
    assert!(
        run.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&run.stdout)
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
        "schema_version": 5,
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
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_SRID_KEY),
        Some(&"4326".to_owned()),
        "srid del TARGET dedotto dalla definizione d'autorita' (emendamento 2026-07-31), \
         non quello della sorgente"
    );
    assert_eq!(
        metadata.get(adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY),
        Some(&"lon_lat".to_owned()),
        "axis_order delle coordinate GIS normalizzate emesse dal backend, non l'ordine authority del target"
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
        "schema_version": 5,
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
    let rows: usize = reader.map(|batch| batch.expect("batch").num_rows()).sum();
    assert_eq!(rows, 2);
}

// ---------------------------------------------------------------------------
// Binding degli input: due input invertiti non devono MAI arrivare
// all'esecuzione
// ---------------------------------------------------------------------------

/// Piano v4 con DUE input: left join su `id`. E' asimmetrico, quindi
/// scambiare i due lati cambia il risultato — ed e' esattamente cio' che la
/// forma posizionale non poteva intercettare.
fn plan_due_input() -> serde_json::Value {
    json!({
        "schema_version": 5,
        "inputs": ["sinistra", "destra"],
        "nodes": [
            {"id": "j", "op": "table.join", "in": ["sinistra", "destra"],
             "config": {"left_keys": ["id"], "right_keys": ["id"], "how": "left"}},
        ],
        "output": "j",
    })
}

/// Due input con lo STESSO schema e contenuti diversi: e' il caso in cui uno
/// scambio non produce alcun errore di schema.
fn scrivi_due_input(
    directory: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let plan = directory.join("plan.json");
    std::fs::write(&plan, serde_json::to_vec(&plan_due_input()).expect("json")).expect("plan");
    let sinistra = directory.join("sinistra.arrow");
    let destra = directory.join("destra.arrow");
    write_ipc(
        &sinistra,
        &table_schema(),
        &[table_batch(&[1, 2], &["a", "b"])],
    );
    write_ipc(
        &destra,
        &table_schema(),
        &[table_batch(&[2, 3], &["x", "y"])],
    );
    (plan, sinistra, destra)
}

fn righe_id(path: &std::path::Path) -> Vec<i64> {
    let file = std::fs::File::open(path).expect("open output");
    let reader = FileReader::try_new(file, None).expect("reader");
    let mut ids = Vec::new();
    for batch in reader {
        let batch = batch.expect("batch");
        let colonna = batch
            .column_by_name("id")
            .and_then(|column| column.as_any().downcast_ref::<Int64Array>())
            .expect("colonna id");
        ids.extend((0..batch.num_rows()).map(|riga| colonna.value(riga)));
    }
    ids
}

#[test]
fn due_input_invertiti_non_raggiungono_mai_l_esecuzione() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, sinistra, destra) = scrivi_due_input(directory.path());

    // 1. Forma posizionale con i percorsi INVERTITI: rifiutata, e senza aver
    //    prodotto alcun output.
    let output_invertito = directory.path().join("invertito.arrow");
    let esito = cli()
        .arg("run")
        .arg("--plan")
        .arg(&plan)
        .arg("--inputs")
        .arg(&destra)
        .arg(&sinistra)
        .arg("--output")
        .arg(&output_invertito)
        .output()
        .expect("run");
    assert!(
        !esito.status.success(),
        "la forma posizionale con due input deve fallire"
    );
    assert!(
        !output_invertito.exists(),
        "nessun output deve essere pubblicato da un binding non verificabile"
    );
    let messaggio = String::from_utf8_lossy(&esito.stdout);
    assert!(
        messaggio.contains("--input sinistra=PERCORSO")
            && messaggio.contains("--input destra=PERCORSO"),
        "l'errore deve indicare il rimedio nominale: {messaggio}"
    );

    // 2. Anche con i percorsi nell'ordine GIUSTO la forma posizionale e'
    //    rifiutata: il difetto non e' l'ordine sbagliato, e' che l'ordine non
    //    e' verificabile. Accettarla quando per caso e' giusta significa
    //    accettarla sempre.
    let output_ordinato = directory.path().join("ordinato.arrow");
    let esito = cli()
        .arg("run")
        .arg("--plan")
        .arg(&plan)
        .arg("--inputs")
        .arg(&sinistra)
        .arg(&destra)
        .arg("--output")
        .arg(&output_ordinato)
        .output()
        .expect("run");
    assert!(!esito.status.success());
    assert!(!output_ordinato.exists());

    // 3. `validate` si comporta allo stesso modo: il rifiuto non arriva
    //    all'ultimo momento, e nemmeno da un percorso diverso.
    let esito = cli()
        .arg("validate")
        .arg("--plan")
        .arg(&plan)
        .arg("--inputs")
        .arg(&sinistra)
        .arg(&destra)
        .output()
        .expect("validate");
    assert!(!esito.status.success());

    // 4. La forma nominale esegue, e il binding conta davvero: con i nomi
    //    scambiati il risultato e' DIVERSO. E' la dimostrazione che la forma
    //    posizionale, prima, poteva pubblicare in silenzio l'output sbagliato.
    let corretto = directory.path().join("corretto.arrow");
    let esito = cli()
        .arg("run")
        .arg("--plan")
        .arg(&plan)
        .arg("--input")
        .arg(format!("sinistra={}", sinistra.display()))
        .arg("--input")
        .arg(format!("destra={}", destra.display()))
        .arg("--output")
        .arg(&corretto)
        .output()
        .expect("run");
    assert!(
        esito.status.success(),
        "la forma nominale deve eseguire: {}",
        String::from_utf8_lossy(&esito.stdout)
    );

    let scambiato = directory.path().join("scambiato.arrow");
    let esito = cli()
        .arg("run")
        .arg("--plan")
        .arg(&plan)
        .arg("--input")
        .arg(format!("sinistra={}", destra.display()))
        .arg("--input")
        .arg(format!("destra={}", sinistra.display()))
        .arg("--output")
        .arg(&scambiato)
        .output()
        .expect("run");
    assert!(esito.status.success());

    // Left join: le righe sono quelle del lato sinistro. Scambiando i due
    // file cambia proprio l'insieme delle righe pubblicate.
    assert_eq!(righe_id(&corretto), vec![1, 2]);
    assert_eq!(righe_id(&scambiato), vec![2, 3]);
}

#[test]
fn un_solo_input_resta_compatibile_con_la_forma_posizionale() {
    // Con un input dichiarato non c'e' niente da scambiare: la forma
    // posizionale resta accettata, come deciso per il rilascio.
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_table_fixture(directory.path());
    let output_path = directory.path().join("uno.arrow");
    let esito = cli_run(&plan, &input, &output_path);
    assert!(
        esito.status.success(),
        "un solo input deve restare compatibile: {}",
        String::from_utf8_lossy(&esito.stdout)
    );
    assert!(output_path.exists());
}

// ---------------------------------------------------------------------------
// Ottavo giro, finding 2 — `ResourceLimit` instradato senza perdite
// ---------------------------------------------------------------------------

#[test]
fn un_limite_alzato_dentro_un_kernel_arriva_intatto_all_envelope() {
    // Il difetto: un limite di risorsa alzato DENTRO un passo veniva
    // riavvolto da `step_error` in un `Replayed` la cui categoria non era
    // piu' `resource_limit`. L'errore restava, ma cambiava natura: exit code
    // e categoria dicevano «piano invalido» per un piano corretto i cui dati
    // non entravano nel budget.
    //
    // Il join e' il posto giusto per verificarlo: il limite non lo alza
    // l'executor ma il kernel, quindi l'errore attraversa `step_error` prima
    // di diventare un envelope.
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": 5,
            "inputs": ["sinistra", "destra"],
            // Quattro righe per lato entrano (max_input_rows = 8), ma la
            // chiave e' la stessa per tutte: il join produce 16 righe e il
            // kernel chiude sul PROPRIO conteggio.
            "limits": {"max_input_rows": 8},
            "nodes": [
                {"id": "j", "op": "table.join", "in": ["sinistra", "destra"],
                 "config": {"left_keys": ["id"], "right_keys": ["id"], "how": "inner"}},
            ],
            "output": "j",
        }))
        .expect("json"),
    )
    .expect("plan");
    let sinistra = directory.path().join("sinistra.arrow");
    let destra = directory.path().join("destra.arrow");
    write_ipc(
        &sinistra,
        &table_schema(),
        &[table_batch(&[1, 1, 1, 1], &["a", "b", "c", "d"])],
    );
    write_ipc(
        &destra,
        &table_schema(),
        &[table_batch(&[1, 1, 1, 1], &["w", "x", "y", "z"])],
    );
    let uscita = directory.path().join("out.arrow");

    let esito = cli()
        .arg("run")
        .arg("--plan")
        .arg(&plan)
        .arg("--input")
        .arg(format!("sinistra={}", sinistra.display()))
        .arg("--input")
        .arg(format!("destra={}", destra.display()))
        .arg("--output")
        .arg(&uscita)
        .output()
        .expect("cli");

    assert!(!esito.status.success(), "il limite deve chiudere");
    let envelope: serde_json::Value =
        serde_json::from_slice(&esito.stdout).expect("envelope su stdout");
    assert_eq!(
        envelope["error"]["category"], "resource_limit",
        "la categoria non deve perdersi attraversando `step_error`: {envelope}"
    );
    assert_eq!(
        esito.status.code(),
        Some(4),
        "e l'exit code dedicato deve seguirla: {envelope}"
    );
    // La diagnostica del passo non va persa nel preservare la categoria: il
    // nodo che ha alzato il limite resta nell'envelope.
    let testo = envelope.to_string();
    assert!(
        testo.contains("\"j\"") || testo.contains("table.join"),
        "nodo e operazione restano nella diagnostica: {envelope}"
    );
    assert!(!uscita.exists(), "nessun output da un limite superato");
}

#[test]
fn un_tetto_del_trasporto_e_un_limite_di_risorsa_in_fase_di_lettura() {
    // Stessa classe dal lato del confine IPC: un messaggio che sfonda un
    // tetto del trasporto non e' un dato malformato (`data_mapping`) ma una
    // risorsa esaurita, e la fase e' `read` perche' il tetto scatta mentre si
    // legge, non mentre si scrive.
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan.json");
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": 5,
            "inputs": ["main"],
            "limits": {"max_payload_bytes": 512},
            "nodes": [{"id": "f", "op": "table.filter", "in": ["main"],
                       "config": {"column": "id", "operator": ">", "value": 0}}],
            "output": "f",
        }))
        .expect("json"),
    )
    .expect("plan");
    let input = directory.path().join("input.arrow");
    let ids: Vec<i64> = (0..4_000).collect();
    let etichette: Vec<String> = ids.iter().map(|id| format!("riga-{id}")).collect();
    let riferimenti: Vec<&str> = etichette.iter().map(String::as_str).collect();
    write_ipc(&input, &table_schema(), &[table_batch(&ids, &riferimenti)]);
    let uscita = directory.path().join("out.arrow");

    let esito = cli()
        .arg("run")
        .arg("--plan")
        .arg(&plan)
        .arg("--input")
        .arg(format!("main={}", input.display()))
        .arg("--output")
        .arg(&uscita)
        .output()
        .expect("cli");

    assert!(!esito.status.success(), "il tetto deve chiudere");
    let envelope: serde_json::Value =
        serde_json::from_slice(&esito.stdout).expect("envelope su stdout");
    assert_eq!(
        envelope["error"]["category"], "resource_limit",
        "un tetto del trasporto e' una risorsa, non una mappatura: {envelope}"
    );
    assert_eq!(
        envelope["error"]["phase"], "read",
        "la fase e' quella in cui il tetto scatta: {envelope}"
    );
    assert!(!uscita.exists(), "nessun output da un tetto superato");
}

// ---------------------------------------------------------------------------
// Migrazione v4 -> v5 attraverso la CLI (errori-e-limiti.md#memoria-governata)
// ---------------------------------------------------------------------------

/// Lo stesso piano tabellare nella forma v4, col nome che la v4 dava al
/// budget di memoria.
fn table_plan_v4() -> serde_json::Value {
    let mut piano = table_plan();
    piano["schema_version"] = json!(4);
    piano["limits"] = json!({"max_memory_bytes": 4_194_304});
    piano
}

fn riepilogo_validate(plan: &std::path::Path, input: &std::path::Path) -> serde_json::Value {
    let result = cli_validate(plan, input);
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).expect("JSON")
}

#[test]
fn un_piano_v4_entra_dalla_migrazione_e_riporta_la_versione_canonica() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (_, input) = write_table_fixture(directory.path());

    let piano_v4 = directory.path().join("plan_v4.json");
    std::fs::write(
        &piano_v4,
        serde_json::to_vec(&table_plan_v4()).expect("json"),
    )
    .expect("plan");
    let piano_v5 = directory.path().join("plan_v5.json");
    let mut equivalente = table_plan();
    equivalente["limits"] = json!({"max_governed_memory_bytes": 4_194_304});
    std::fs::write(&piano_v5, serde_json::to_vec(&equivalente).expect("json")).expect("plan");

    let da_v4 = riepilogo_validate(&piano_v4, &input);
    let da_v5 = riepilogo_validate(&piano_v5, &input);

    // La CLI riporta la versione CANONICA, non quella scritta nel file: e'
    // la versione sotto cui il piano viene effettivamente eseguito.
    assert_eq!(da_v4["schema_version"], 5);
    assert_eq!(da_v4["plan_hash"], da_v5["plan_hash"]);
    assert_eq!(da_v4["topological_order"], da_v5["topological_order"]);
}

#[test]
fn un_piano_v4_col_nome_della_v5_e_rifiutato_dalla_cli() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (_, input) = write_table_fixture(directory.path());

    let mut piano = table_plan_v4();
    piano["limits"] = json!({"max_governed_memory_bytes": 4_194_304});
    let percorso = directory.path().join("plan_misto.json");
    std::fs::write(&percorso, serde_json::to_vec(&piano).expect("json")).expect("plan");

    let result = cli_validate(&percorso, &input);
    assert!(!result.status.success());
    // Canale congelato: l'envelope diagnostico vive su stdout, stderr resta
    // vuoto (par. 9).
    assert!(result.stderr.is_empty());
    let envelope = String::from_utf8_lossy(&result.stdout);
    assert!(envelope.contains("non ha alias"), "{envelope}");
}

#[test]
fn un_piano_v5_col_nome_della_v4_e_rifiutato_dalla_cli() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (_, input) = write_table_fixture(directory.path());

    let mut piano = table_plan();
    piano["limits"] = json!({"max_memory_bytes": 4_194_304});
    let percorso = directory.path().join("plan_vecchio.json");
    std::fs::write(&percorso, serde_json::to_vec(&piano).expect("json")).expect("plan");

    let result = cli_validate(&percorso, &input);
    assert!(!result.status.success());
    assert!(result.stderr.is_empty());
    let envelope = String::from_utf8_lossy(&result.stdout);
    assert!(envelope.contains("max_memory_bytes"), "{envelope}");
}

#[test]
fn un_piano_v4_esegue_e_produce_lo_stesso_output_del_v5() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (piano_v5, input) = write_table_fixture(directory.path());
    let piano_v4 = directory.path().join("plan_v4.json");
    let mut v4 = table_plan();
    v4["schema_version"] = json!(4);
    std::fs::write(&piano_v4, serde_json::to_vec(&v4).expect("json")).expect("plan");

    let esegui = |plan: &std::path::Path, output: &std::path::Path| {
        let result = cli_run(plan, &input, output);
        assert!(
            result.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    };

    let uscita_v4 = directory.path().join("out_v4.arrow");
    let uscita_v5 = directory.path().join("out_v5.arrow");
    esegui(&piano_v4, &uscita_v4);
    esegui(&piano_v5, &uscita_v5);

    assert_eq!(
        std::fs::read(&uscita_v4).expect("out v4"),
        std::fs::read(&uscita_v5).expect("out v5"),
        "la migrazione non deve cambiare i byte prodotti"
    );
}

#[test]
fn un_piano_legacy_conserva_il_nome_del_proprio_formato() {
    // Il formato lineare v1 e' un formato PUBBLICATO, distinguibile dagli
    // altri proprio da `schema_version: 1`. errori-e-limiti.md#memoria-governata ha rinominato il campo
    // della libreria, non quel formato: un piano gia' scritto non cambia
    // perche' noi abbiamo cambiato idea sul nome. La traduzione avviene
    // dentro, in `table_engine::contract::limiti_v1`.
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan_legacy.json");
    let input = directory.path().join("input.arrow");
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"limits":{"max_memory_bytes":4194304},
            "steps":[{"operation":"rename","config":{"renames":[
            {"old_name":"name","new_name":"label"}]}}]}"#,
    )
    .expect("plan");
    write_ipc(
        &input,
        &table_schema(),
        &[table_batch(&[1, 2], &["a", "b"])],
    );

    let result = cli_validate(&plan, &input);
    assert!(
        result.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );
}

#[test]
fn un_piano_legacy_col_nome_della_v5_e_rifiutato() {
    // Il rovescio, ed e' cio' che distingue una traduzione da un alias:
    // nessuno dei due nomi funziona in entrambi i formati. Se il nome nuovo
    // passasse anche qui, avremmo scritto un alias e chiamato traduzione.
    let directory = tempfile::tempdir().expect("tempdir");
    let plan = directory.path().join("plan_legacy.json");
    let input = directory.path().join("input.arrow");
    std::fs::write(
        &plan,
        br#"{"schema_version":1,"limits":{"max_governed_memory_bytes":4194304},
            "steps":[{"operation":"rename","config":{"renames":[
            {"old_name":"name","new_name":"label"}]}}]}"#,
    )
    .expect("plan");
    write_ipc(
        &input,
        &table_schema(),
        &[table_batch(&[1, 2], &["a", "b"])],
    );

    let result = cli_validate(&plan, &input);
    assert!(!result.status.success());
    assert!(result.stderr.is_empty());
    let envelope = String::from_utf8_lossy(&result.stdout);
    assert!(envelope.contains("max_governed_memory_bytes"), "{envelope}");
}

// ---------------------------------------------------------------------------
// Formato v6: la versione dichiarata negli output della CLI
// ---------------------------------------------------------------------------

/// Lo stesso piano tabellare, dichiarato `schema_version: 6` col tetto del
/// dominio.
fn table_plan_v6() -> serde_json::Value {
    let mut piano = table_plan();
    piano["schema_version"] = json!(6);
    piano["limits"] = json!({"max_domain_memory_bytes": 1_073_741_824_u64});
    piano
}

fn write_v6_fixture(directory: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let plan = directory.join("plan-v6.json");
    let input = directory.join("input.arrow");
    std::fs::write(&plan, serde_json::to_vec(&table_plan_v6()).expect("json")).expect("plan");
    write_ipc(
        &input,
        &table_schema(),
        &[table_batch(&[0, 1, 2], &["a", "b", "c"])],
    );
    (plan, input)
}

#[test]
fn validate_v6_dichiara_la_versione_sei() {
    // La versione negli output era **fissata** a 5. Un piano v6 sarebbe stato
    // descritto come un v5, con accanto un `plan_hash` di un altro dominio:
    // la coppia che rende irriconoscibile un'identita' conservata.
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_v6_fixture(directory.path());

    let result = cli_validate(&plan, &input);
    assert!(
        result.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON");
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["schema_version"], 6, "la versione la dice il piano");
}

#[test]
fn run_v6_dichiara_la_versione_sei() {
    // Il gemello nell'output di `run`: stesso difetto, stesso rimedio.
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan, input) = write_v6_fixture(directory.path());
    let output_path = directory.path().join("output-v6.arrow");

    let result = cli_run(&plan, &input, &output_path);
    assert!(
        result.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let metrics: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON");
    assert_eq!(metrics["status"], "ok");
    assert_eq!(metrics["schema_version"], 6, "la versione la dice il piano");
    assert_eq!(metrics["output_rows"], 2, "il piano esegue come il v5");
}

#[test]
fn un_v5_e_il_v6_equivalente_hanno_plan_hash_diversi() {
    // `PLAN-018`, osservato dal confine pubblico: due documenti per il resto
    // identici, due identita'.
    let directory = tempfile::tempdir().expect("tempdir");
    let (plan_v5, input) = write_table_fixture(directory.path());
    let plan_v6 = directory.path().join("solo-versione.json");
    let mut senza_tetto = table_plan();
    senza_tetto["schema_version"] = json!(6);
    std::fs::write(&plan_v6, serde_json::to_vec(&senza_tetto).expect("json")).expect("plan");

    let hash_di = |percorso: &std::path::Path| -> String {
        let result = cli_validate(percorso, &input);
        assert!(
            result.status.success(),
            "stdout: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        let summary: serde_json::Value = serde_json::from_slice(&result.stdout).expect("JSON");
        summary["plan_hash"].as_str().expect("plan_hash").to_owned()
    };
    assert_ne!(
        hash_di(&plan_v5),
        hash_di(&plan_v6),
        "un v5 e un v6 per il resto identici non condividono l'identita'"
    );
}
