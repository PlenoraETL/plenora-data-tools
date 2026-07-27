//! plenora-data-tools CLI — Fase 1 "coesistenza" (Architetture.md par. 3.5).
//!
//! Fusione meccanica dei due binari di origine, senza modifiche di
//! comportamento sui comandi legacy:
//!
//! - da `plenora-nogeo-tools/src/main.rs`: `run --plan --input [--right]
//!   --output` (lettura Arrow IPC file format, streaming batch-per-batch se
//!   nessuno step e' blocking, limiti righe globali, publish atomico con
//!   `persist_noclobber`), `self-test` (integrita' del catalogo);
//! - da `plenora-geo-tools-arrow/src/main.rs`: `capabilities`, `transform`
//!   (framing WKB v2 `PLNGEO2`), `spatial-join` (v2), `transform-arrow`
//!   (envelope v3 `PLNGEO3`), `pair-arrow` (v3), `self-test --output`;
//! - nuovi di Fase 1: `catalog [--family table|geo]` (catalogo unificato di
//!   `plenora-core`, 146 operazioni) e `validate --plan --inputs ...`;
//! - Fase 2A: collegamento al DAG v4. Se il piano dichiara `schema_version: 4`,
//!   `validate` e `run` usano il planner/executor del DAG
//!   (`plenora_engine::planner::validate` + `plenora_engine::execute`); i piani
//!   legacy (`schema_version` <= 3) restano sul `table_engine`, comportamento
//!   invariato. Dettagli nella sezione "DAG v4 (Fase 2A)" piu' sotto.
//!
//! Fail-closed come nei sorgenti: nessun output parziale, publish atomico su
//! tempfile + `persist_noclobber`, exit code 2 su qualunque errore, messaggi
//! senza dati sensibili.

use std::env;
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::reader::{FileReader, StreamReader};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, SchemaRef};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::catalog::{find_operation, CrsRequirement, Family, OperationDescriptor, CATALOG};
use plenora_core::contract::{
    ContractProperties, DataContract, FieldId, GeometryColumnContract, GeometryDimensions,
};
use plenora_core::crs::{required_definition, validate_requirement};
use plenora_core::PlenoraError;
use plenora_engine::geo_transport::pair_protocol::{write_pairs, MAX_PAIRS};
use plenora_engine::geo_transport::protocol::{Frame, FrameReader, FrameWriter};
use plenora_engine::geo_transport::publish::{
    publish_with_profile, validate_pair_arrow_crs, validate_transform_arrow_crs, PublishOutcome,
    PublishProfile,
};
use plenora_engine::geo_transport::transport::{
    pair_arrow, transform_arrow, PairArrowSchema, PairArrowSummary, TransformArrowSchema,
    TransformArrowSummary,
};
use plenora_engine::plan::PLAN_SCHEMA_VERSION_V4;
use plenora_engine::planner::{self, ValidatedGraph};
use plenora_engine::table_engine::{execute_batch, execute_binary, Plan, ValidatedPlan};
use plenora_engine::{
    execute, explain, ExecutionMetrics, ExecutionPlan, Input, Inputs, RuntimeContext,
};
use plenora_kernels_geo::arrow_adapter::{
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
};
use plenora_kernels_geo::spatial_join::{spatial_join_nullable, JoinPredicate};
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, Operation};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

// ---------------------------------------------------------------------------
// Helper comuni
// ---------------------------------------------------------------------------

fn contract(message: impl Into<String>) -> PlenoraError {
    PlenoraError::Contract(message.into())
}

/// Presenta l'esito tipizzato del publish (ADR 7): se il file e' stato
/// pubblicato ma la durabilita' non e' confermata, avvisa su stderr. Con il
/// profilo `Atomic` l'esito e' sempre `Published`; il ramo warning serve ai
/// chiamanti che useranno il profilo `DurableAtomic`.
fn report_publish_outcome(outcome: PublishOutcome, output_path: &Path) {
    if outcome == PublishOutcome::PublishedButDurabilityUnconfirmed {
        eprintln!(
            "avviso: {} pubblicato, ma la durabilita' non e' confermata \
             (fsync della directory non supportato o fallito)",
            output_path.display()
        );
    }
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

/// Helper stile nogeo: valore obbligatorio dopo un flag.
fn value_after(args: &[String], flag: &str) -> Result<PathBuf, PlenoraError> {
    let index = args
        .iter()
        .position(|argument| argument == flag)
        .ok_or_else(|| contract(format!("argomento mancante: {flag}")))?;
    let value = args
        .get(index + 1)
        .ok_or_else(|| contract(format!("valore mancante dopo {flag}")))?;
    Ok(PathBuf::from(value))
}

/// Helper stile nogeo: valore opzionale dopo un flag.
fn optional_value_after(args: &[String], flag: &str) -> Result<Option<PathBuf>, PlenoraError> {
    let Some(index) = args.iter().position(|argument| argument == flag) else {
        return Ok(None);
    };
    let value = args
        .get(index + 1)
        .ok_or_else(|| contract(format!("valore mancante dopo {flag}")))?;
    Ok(Some(PathBuf::from(value)))
}

/// Helper stile geo: valore obbligatorio dopo un flag (messaggi del sorgente).
fn argument_value(args: &[String], name: &str) -> Result<String, PlenoraError> {
    let position = args
        .iter()
        .position(|value| value == name)
        .ok_or_else(|| contract(format!("argomento obbligatorio mancante: {name}")))?;
    args.get(position + 1)
        .cloned()
        .ok_or_else(|| contract(format!("valore mancante per {name}")))
}

// ---------------------------------------------------------------------------
// Pipeline tabellare (port da plenora-nogeo-tools/src/main.rs)
// ---------------------------------------------------------------------------

fn load_complete(path: &Path, plan: &ValidatedPlan) -> Result<RecordBatch, PlenoraError> {
    let reader = FileReader::try_new(File::open(path)?, None)?;
    let schema = reader.schema();
    let mut batches = Vec::new();
    let mut rows = 0_usize;
    for batch in reader {
        let batch = batch?;
        rows = rows
            .checked_add(batch.num_rows())
            .ok_or_else(|| contract("overflow conteggio righe"))?;
        if rows > plan.limits().max_rows {
            return Err(contract(format!(
                "file con oltre {} righe",
                plan.limits().max_rows
            )));
        }
        batches.push(batch);
    }
    if batches.is_empty() {
        return Ok(RecordBatch::new_empty(schema));
    }
    Ok(concat_batches(&schema, &batches)?)
}

fn publish_one(output_path: &Path, output: &RecordBatch) -> Result<(), PlenoraError> {
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer = FileWriter::try_new(temporary.as_file_mut(), &output.schema())?;
    writer.write(output)?;
    writer.finish()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(output_path)
        .map_err(|error| error.error)?;
    Ok(())
}

fn run_pipeline(
    plan_path: &Path,
    input_path: &Path,
    right_path: Option<&Path>,
    output_path: &Path,
) -> Result<(), PlenoraError> {
    if output_path.exists() {
        return Err(contract(format!(
            "output gia' esistente, rifiuto di sovrascriverlo: {}",
            output_path.display()
        )));
    }
    let plan: Plan = serde_json::from_reader(File::open(plan_path)?)?;
    let plan = plan.validate()?;
    if plan.requires_secondary() || plan.requires_blocking() {
        let left = load_complete(input_path, &plan)?;
        let output = if plan.requires_secondary() {
            let right_path =
                right_path.ok_or_else(|| contract("il piano richiede --right"))?;
            let right = load_complete(right_path, &plan)?;
            execute_binary(&left, &right, &plan)?
        } else {
            execute_batch(left, &plan)?
        };
        return publish_one(output_path, &output);
    }
    let reader = FileReader::try_new(File::open(input_path)?, None)?;
    let input_schema = reader.schema();

    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer: Option<FileWriter<&mut File>> = None;

    let mut wrote_batch = false;
    let mut total_rows = 0_usize;
    for input in reader {
        let input = input?;
        total_rows = total_rows.checked_add(input.num_rows()).ok_or_else(|| {
            contract("overflow nel conteggio complessivo delle righe")
        })?;
        if total_rows > plan.limits().max_rows {
            return Err(contract(format!(
                "file con oltre {} righe",
                plan.limits().max_rows
            )));
        }
        let output = execute_batch(input, &plan)?;
        if let Some(existing) = writer.as_mut() {
            if existing.schema().as_ref() != output.schema().as_ref() {
                return Err(PlenoraError::Schema(
                    "la catena ha prodotto schemi diversi tra batch".into(),
                ));
            }
            existing.write(&output)?;
        } else {
            let mut created = FileWriter::try_new(temporary.as_file_mut(), &output.schema())?;
            created.write(&output)?;
            writer = Some(created);
        }
        wrote_batch = true;
    }
    if !wrote_batch {
        let output = execute_batch(RecordBatch::new_empty(input_schema), &plan)?;
        let mut created = FileWriter::try_new(temporary.as_file_mut(), &output.schema())?;
        created.write(&output)?;
        writer = Some(created);
    }
    if let Some(mut writer) = writer {
        writer.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(output_path)
        .map_err(|error| error.error)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Trasporto WKB v2 (port dal livello comandi di plenora-geo-tools-arrow)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformSchema {
    schema_version: u32,
    operation: Operation,
    row_count: u64,
    crs: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpatialJoinSchema {
    schema_version: u32,
    predicate: JoinPredicate,
    left_row_count: u64,
    right_row_count: u64,
    max_pairs: u64,
    left_crs: Option<String>,
    right_crs: Option<String>,
}

#[derive(Debug, Serialize)]
struct Capability<'a> {
    name: &'a str,
    input: &'a str,
    output: &'a str,
}

#[derive(Debug)]
struct TransformSummary {
    rows: u64,
    checksum: [u8; 32],
}

#[derive(Debug)]
struct SpatialJoinSummary {
    pairs: u64,
    checksum: [u8; 32],
}

fn transform_stream(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    schema: &TransformSchema,
) -> Result<TransformSummary, Box<dyn Error>> {
    if schema.schema_version != 2 {
        return Err(contract(format!(
            "schema_version {} non supportata",
            schema.schema_version
        ))
        .into());
    }

    let mut input = FrameReader::new(reader, schema.row_count)?;
    let mut output = FrameWriter::new(writer, schema.row_count)?;

    const BATCH_ROWS: usize = 4096;
    const BATCH_BYTES: usize = 64 * 1024 * 1024;
    let mut rows = 0_u64;
    let mut batch_bytes = 0_usize;
    let mut batch: Vec<Option<Vec<u8>>> = Vec::with_capacity(BATCH_ROWS);
    while let Some(frame) = input.next_frame()? {
        let payload = match frame {
            Frame::Null => None,
            Frame::Wkb(payload) => Some(payload),
        };
        let payload_bytes = payload.as_ref().map_or(0, Vec::len);
        if !batch.is_empty()
            && (batch.len() == BATCH_ROWS
                || batch_bytes.saturating_add(payload_bytes) > BATCH_BYTES)
        {
            rows += transform_batch(&mut output, schema.operation, &batch)? as u64;
            batch.clear();
            batch_bytes = 0;
        }
        batch_bytes = batch_bytes.saturating_add(payload_bytes);
        batch.push(payload);
    }
    if !batch.is_empty() {
        rows += transform_batch(&mut output, schema.operation, &batch)? as u64;
    }

    if rows != schema.row_count {
        return Err(contract(format!(
            "row_count non coerente: atteso {}, ricevuto {rows}",
            schema.row_count
        ))
        .into());
    }
    let (_, checksum) = output.finish()?;
    Ok(TransformSummary { rows, checksum })
}

fn transform_batch<W: Write>(
    writer: &mut FrameWriter<W>,
    operation: Operation,
    batch: &[Option<Vec<u8>>],
) -> Result<usize, Box<dyn Error>> {
    let transformed: Result<Vec<Option<Vec<u8>>>, _> = batch
        .par_iter()
        .map(|payload| {
            payload
                .as_deref()
                .map(|wkb| transform_wkb(operation, wkb))
                .transpose()
        })
        .collect();

    for payload in transformed? {
        writer.write_frame(payload.as_deref())?;
    }
    Ok(batch.len())
}

fn validate_transform_crs(schema: &TransformSchema) -> Result<(), PlenoraError> {
    let definition = required_definition(schema.crs.as_deref(), "crs")?;
    let crs = resolve_crs(definition, "crs")?;
    let catalog_name = match schema.operation {
        Operation::Centroid => "geo_centroid",
        Operation::ConvexHull => "geo_convex_hull",
        Operation::Envelope => "geo_envelope",
    };
    let descriptor = find_operation(catalog_name)
        .ok_or_else(|| contract(format!("operazione {catalog_name} assente dal catalogo")))?;
    validate_requirement(
        descriptor.crs_requirement.unwrap_or(CrsRequirement::Known),
        &[&crs],
    )?;
    Ok(())
}

fn execute_transform(
    input: &str,
    schema_path: &Path,
    output: &str,
) -> Result<TransformSummary, Box<dyn Error>> {
    if output == "-" {
        return Err(contract(
            "output stdout disabilitato: la pubblicazione deve essere transazionale",
        )
        .into());
    }
    let schema: TransformSchema = serde_json::from_reader(BufReader::with_capacity(
        64 * 1024,
        File::open(schema_path)?,
    ))?;
    if schema.schema_version != 2 {
        return Err(contract(format!(
            "schema_version {} non supportata",
            schema.schema_version
        ))
        .into());
    }
    validate_transform_crs(&schema)?;

    let mut input_reader: Box<dyn Read> = if input == "-" {
        Box::new(BufReader::with_capacity(1024 * 1024, std::io::stdin()))
    } else {
        Box::new(BufReader::with_capacity(1024 * 1024, File::open(input)?))
    };
    let output_path = Path::new(output);
    let (result, outcome) = publish_with_profile(output_path, PublishProfile::Atomic, |output_writer| {
        transform_stream(&mut input_reader, output_writer, &schema)
            .map_err(|error| contract(error.to_string()))
    })?;
    report_publish_outcome(outcome, output_path);
    Ok(result)
}

fn execute_transform_arrow(
    input: &str,
    schema_path: &Path,
    output: &str,
) -> Result<TransformArrowSummary, Box<dyn Error>> {
    if output == "-" {
        return Err(contract(
            "output stdout disabilitato: la pubblicazione deve essere transazionale",
        )
        .into());
    }
    let schema: TransformArrowSchema = serde_json::from_reader(BufReader::with_capacity(
        64 * 1024,
        File::open(schema_path)?,
    ))?;
    if schema.schema_version != TransformArrowSchema::VERSION {
        return Err(contract(format!(
            "schema_version {} non supportata",
            schema.schema_version
        ))
        .into());
    }
    schema.validate_parameters()?;
    validate_transform_arrow_crs(&schema)?;

    let mut input_reader: Box<dyn Read> = if input == "-" {
        Box::new(BufReader::with_capacity(1024 * 1024, std::io::stdin()))
    } else {
        Box::new(BufReader::with_capacity(1024 * 1024, File::open(input)?))
    };
    let output_path = Path::new(output);
    let (summary, outcome) = publish_with_profile(output_path, PublishProfile::Atomic, |output_writer| {
        transform_arrow(&mut input_reader, output_writer, &schema)
            .map_err(|error| contract(error.to_string()))
    })?;
    report_publish_outcome(outcome, output_path);
    Ok(summary)
}

fn execute_pair_arrow(
    left_path: &Path,
    right_path: &Path,
    schema_path: &Path,
    output_path: &Path,
) -> Result<PairArrowSummary, Box<dyn Error>> {
    let schema: PairArrowSchema = serde_json::from_reader(BufReader::with_capacity(
        64 * 1024,
        File::open(schema_path)?,
    ))?;
    if schema.schema_version != PairArrowSchema::VERSION {
        return Err(contract(format!(
            "schema_version {} non supportata",
            schema.schema_version
        ))
        .into());
    }
    schema.validate_parameters()?;
    validate_pair_arrow_crs(&schema)?;

    let mut left_reader = BufReader::with_capacity(1024 * 1024, File::open(left_path)?);
    let mut right_reader = BufReader::with_capacity(1024 * 1024, File::open(right_path)?);
    let (summary, outcome) = publish_with_profile(output_path, PublishProfile::Atomic, |output_writer| {
        pair_arrow(
            &mut left_reader,
            &mut right_reader,
            output_writer,
            &schema,
        )
        .map_err(|error| contract(error.to_string()))
    })?;
    report_publish_outcome(outcome, output_path);
    Ok(summary)
}

fn read_geometry_stream(
    path: &Path,
    row_count: u64,
) -> Result<Vec<Option<geo::Geometry<f64>>>, Box<dyn Error>> {
    let reader = BufReader::with_capacity(1024 * 1024, File::open(path)?);
    let mut frames = FrameReader::new(reader, row_count)?;
    let capacity = usize::try_from(row_count)
        .map_err(|_| contract("row_count non rappresentabile in memoria"))?;
    let mut geometries = Vec::with_capacity(capacity);
    while let Some(frame) = frames.next_frame()? {
        geometries.push(match frame {
            Frame::Null => None,
            Frame::Wkb(payload) => Some(geometry_from_wkb(&payload)?),
        });
    }
    Ok(geometries)
}

fn validate_spatial_join_crs(schema: &SpatialJoinSchema) -> Result<(), PlenoraError> {
    let left_definition = required_definition(schema.left_crs.as_deref(), "left_crs")?;
    let right_definition = required_definition(schema.right_crs.as_deref(), "right_crs")?;
    let left_crs = resolve_crs(left_definition, "left_crs")?;
    let right_crs = resolve_crs(right_definition, "right_crs")?;
    validate_requirement(CrsRequirement::SameProjected, &[&left_crs, &right_crs])?;
    Ok(())
}

fn execute_spatial_join(
    left_path: &Path,
    right_path: &Path,
    schema_path: &Path,
    output_path: &Path,
) -> Result<SpatialJoinSummary, Box<dyn Error>> {
    let schema: SpatialJoinSchema = serde_json::from_reader(BufReader::with_capacity(
        64 * 1024,
        File::open(schema_path)?,
    ))?;
    if schema.schema_version != 2 {
        return Err(contract(format!(
            "schema_version {} non supportata",
            schema.schema_version
        ))
        .into());
    }
    if schema.max_pairs == 0 || schema.max_pairs > MAX_PAIRS {
        return Err(contract(format!("max_pairs deve essere tra 1 e {MAX_PAIRS}")).into());
    }
    validate_spatial_join_crs(&schema)?;
    const MAX_JOIN_ROWS_PER_SIDE: u64 = 2_000_000;
    const MAX_JOIN_INPUT_BYTES: u64 = 1024 * 1024 * 1024;
    if schema.left_row_count > MAX_JOIN_ROWS_PER_SIDE
        || schema.right_row_count > MAX_JOIN_ROWS_PER_SIDE
    {
        return Err(contract(format!(
            "spatial-join oltre il limite di {MAX_JOIN_ROWS_PER_SIDE} righe per lato"
        ))
        .into());
    }
    for path in [left_path, right_path] {
        let bytes = path.metadata()?.len();
        if bytes > MAX_JOIN_INPUT_BYTES {
            return Err(contract(format!(
                "input spatial-join {} oltre il limite di {MAX_JOIN_INPUT_BYTES} byte",
                path.display()
            ))
            .into());
        }
    }

    // Entrambi gli input con checksum sono verificati per intero prima del
    // calcolo.
    let left = read_geometry_stream(left_path, schema.left_row_count)?;
    let right = read_geometry_stream(right_path, schema.right_row_count)?;
    let pairs = spatial_join_nullable(&left, &right, schema.predicate, schema.max_pairs)?;
    let pair_count = u64::try_from(pairs.len())
        .map_err(|_| contract("pair_count non rappresentabile"))?;
    let (checksum, outcome) = publish_with_profile(output_path, PublishProfile::Atomic, |writer| {
        let (_, checksum) =
            write_pairs(writer, &pairs).map_err(|error| contract(error.to_string()))?;
        Ok(checksum)
    })?;
    report_publish_outcome(outcome, output_path);
    Ok(SpatialJoinSummary {
        pairs: pair_count,
        checksum,
    })
}

fn write_self_test(path: &Path) -> Result<(), Box<dyn Error>> {
    // POINT (2 3), little-endian OGC WKB.
    let point = [
        1_u8, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 8, 64,
    ];
    let transformed = transform_wkb(Operation::Centroid, &point)?;
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut output = FrameWriter::new(BufWriter::new(file), 1)?;
    output.write_frame(Some(&transformed))?;
    let (mut writer, _) = output.finish()?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Catalogo unificato e validate (nuovi comandi di Fase 1)
// ---------------------------------------------------------------------------

fn descriptor_json(descriptor: &OperationDescriptor) -> serde_json::Value {
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

fn catalog_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    let family = optional_value_after(args, "--family")?;
    let family = family
        .as_deref()
        .map(|value| {
            value.to_str().and_then(|name| match name {
                "table" => Some(Family::Table),
                "geo" => Some(Family::Geo),
                _ => None,
            })
        })
        .map(|parsed| {
            parsed.ok_or_else(|| {
                contract("famiglia sconosciuta: attesa `table` o `geo`".to_owned())
            })
        })
        .transpose()?;
    let entries: Vec<serde_json::Value> = CATALOG
        .iter()
        .filter(|descriptor| family.is_none_or(|wanted| descriptor.family == wanted))
        .map(descriptor_json)
        .collect();
    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

fn validate_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    let plan_path = value_after(args, "--plan")?;
    // `--inputs i1 i2 ...`: tutti i valori fino al prossimo flag. Per i piani
    // v4 sono accoppiati in ordine agli input dichiarati dal piano; per i
    // piani legacy sono solo elencati nel riepilogo.
    let inputs: Vec<PathBuf> = args
        .iter()
        .position(|argument| argument == "--inputs")
        .map_or_else(Vec::new, |index| {
            args[index + 1..]
                .iter()
                .take_while(|argument| !argument.starts_with("--"))
                .map(PathBuf::from)
                .collect()
        });
    let plan_text = std::fs::read_to_string(&plan_path)?;
    if plan_schema_version(&plan_text)? == u32::from(PLAN_SCHEMA_VERSION_V4) {
        return validate_dag_v4(&plan_text, &inputs);
    }
    let plan: Plan = serde_json::from_str(&plan_text)?;
    let plan = plan.validate()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "ok",
            "schema_version": 1,
            "steps": plan.steps().len(),
            "requires_secondary": plan.requires_secondary(),
            "requires_blocking": plan.requires_blocking(),
            "max_rows": plan.limits().max_rows,
            "inputs": inputs,
        }))?
    );
    Ok(())
}

fn self_test_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    if let Some(output) = optional_value_after(args, "--output")? {
        // Variante geo del sorgente: scrive un frame WKB v2 di controllo.
        write_self_test(&output)?;
        println!("{{\"status\":\"ok\",\"output\":{output:?}}}");
        return Ok(());
    }
    // Variante nogeo del sorgente: integrita' del catalogo.
    let unique: std::collections::HashSet<_> =
        CATALOG.iter().map(|operation| operation.id).collect();
    if unique.len() != CATALOG.len() {
        return Err(contract("catalogo non integro").into());
    }
    println!("ok: {} operazioni catalogate", CATALOG.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// DAG v4 (Fase 2A): scoperta dei contratti, validate e run
// ---------------------------------------------------------------------------
//
// Un piano con `schema_version: 4` segue il percorso del DAG:
//
// - **scoperta dei contratti di input**: per ogni percorso si legge il solo
//   header Arrow IPC (file format o stream format, sniffato dal magic
//   `ARROW1`) e si costruisce il `DataContract`: schema Arrow e, se una
//   colonna porta i metadati GeoArrow (`ARROW:extension:name = geoarrow.wkb`
//   + metadato `geo` con chiave `crs`), il `GeometryColumnContract` con CRS
//   risolto (feature-dispatch come i comandi geo legacy: senza `proj-backend`
//   la risoluzione fallisce chiusa). Metadati incoerenti (estensione senza
//   `geo.crs`, `geo` senza estensione, colonna non `Binary`, piu' di una
//   colonna geometrica — D16) sono rifiutati. Il `FieldId` della geometria di
//   input e' provvisorio: il planner lo rimappa nel namespace del grafo;
// - **accoppiamento input**: i percorsi di `--input`/`--inputs` sono legati
//   agli input dichiarati dal piano **in ordine di dichiarazione**
//   (posizionale, deterministico); un conteggio diverso e' un errore;
// - **validate**: `planner::validate` (fase 1, ADR 4/5) e poi `explain` con
//   il `RuntimeContext` di default per il riepilogo della strategia fisica —
//   un piano valido semanticamente ma fuori dal dispatch v1 fallisce qui,
//   non a meta' esecuzione. Il riepilogo JSON su stdout riporta: nodi, archi
//   con contratti (campi + geometria), ordine topologico, segmenti con modo e
//   strategia di parallelismo, capability richieste, fingerprint dei
//   contratti di input e `plan_hash`;
// - **run**: `execute` sul grafo validato con input lazy (`Input::read_ipc_*`,
//   stesso sniffing del formato) e scrittura con `Output::write_ipc_file`
//   (publish atomico no-clobber gia' dentro). Le metriche per nodo e per
//   segmento (righe in/out, batch, wall time in ms) sono stampate in JSON su
//   **stdout**, come gli altri riepiloghi della CLI.

/// Sonda del solo `schema_version`: decide il percorso (DAG v4 vs legacy).
#[derive(Debug, Deserialize)]
struct PlanVersionProbe {
    schema_version: u32,
}

/// Sonda dei nomi di input dichiarati dal piano v4 (accoppiamento posizionale
/// con i percorsi CLI; la validazione vera resta al planner).
#[derive(Debug, Deserialize)]
struct PlanInputsProbe {
    #[serde(default)]
    inputs: Vec<String>,
}

/// `schema_version` del piano, senza validazione strutturale.
fn plan_schema_version(plan_text: &str) -> Result<u32, PlenoraError> {
    Ok(serde_json::from_str::<PlanVersionProbe>(plan_text)?.schema_version)
}

/// `true` se il file inizia con il magic dell'Arrow IPC **file format**
/// (`ARROW1`); altrimenti e' trattato come IPC stream format.
fn is_ipc_file_format(path: &Path) -> Result<bool, PlenoraError> {
    const MAGIC: &[u8; 6] = b"ARROW1";
    let mut file = File::open(path)?;
    let mut buffer = [0_u8; 6];
    let mut read = 0_usize;
    while read < buffer.len() {
        let count = file.read(&mut buffer[read..])?;
        if count == 0 {
            break;
        }
        read += count;
    }
    Ok(read == MAGIC.len() && &buffer == MAGIC)
}

/// Schema Arrow dell'header IPC di un input (file o stream format): nessuna
/// riga di dati letta.
fn ipc_header_schema(path: &Path) -> Result<SchemaRef, PlenoraError> {
    if is_ipc_file_format(path)? {
        Ok(FileReader::try_new(File::open(path)?, None)?.schema())
    } else {
        Ok(StreamReader::try_new(File::open(path)?, None)?.schema())
    }
}

/// Input lazy per l'executor: IPC file o stream format, sniffato dal magic.
fn open_input(path: &Path) -> Result<Input, PlenoraError> {
    if is_ipc_file_format(path)? {
        Input::read_ipc_file(path)
    } else {
        Input::read_ipc_stream(path)
    }
}

/// Definizione CRS dal metadato `geo` di una colonna GeoArrow: stringa
/// `authority:code` oppure PROJJSON come oggetto (serializzato compatto).
/// Fail-closed su metadato mancante o malformato.
fn crs_definition_from_metadata(
    field_name: &str,
    geo_metadata: Option<&String>,
) -> Result<String, PlenoraError> {
    let raw = geo_metadata.ok_or_else(|| {
        contract(format!(
            "colonna geometria `{field_name}` senza metadato `{GEO_METADATA_KEY}`: \
             impossibile determinare il CRS"
        ))
    })?;
    let value: serde_json::Value = serde_json::from_str(raw)?;
    match value.get("crs") {
        Some(serde_json::Value::String(definition)) => Ok(definition.clone()),
        Some(object @ serde_json::Value::Object(_)) => Ok(serde_json::to_string(object)?),
        _ => Err(contract(format!(
            "colonna geometria `{field_name}`: metadato `{GEO_METADATA_KEY}` senza \
             chiave `crs` valida"
        ))),
    }
}

/// Scoperta del `DataContract` di un input dal solo header IPC: schema Arrow
/// e colonna geometria se presente (metadati `geoarrow.wkb` + `geo.crs`, CRS
/// risolto). Fail-closed su metadati incoerenti. Il `FieldId` e' provvisorio:
/// il planner lo rimappa nel namespace globale del grafo (D16).
fn discover_input_contract(path: &Path) -> Result<DataContract, PlenoraError> {
    let schema = ipc_header_schema(path)?;
    let mut geometries = Vec::new();
    for field in schema.fields() {
        let extension = field.metadata().get(GEOARROW_EXTENSION_KEY);
        let geo_metadata = field.metadata().get(GEO_METADATA_KEY);
        let Some(extension) = extension else {
            if geo_metadata.is_some() {
                return Err(contract(format!(
                    "colonna `{}`: metadato `{GEO_METADATA_KEY}` senza estensione \
                     `{GEOARROW_EXTENSION_KEY}`: metadati incoerenti",
                    field.name()
                )));
            }
            continue;
        };
        if extension != GEOARROW_WKB_EXTENSION {
            return Err(contract(format!(
                "colonna `{}`: estensione `{extension}` non supportata \
                 (attesa `{GEOARROW_WKB_EXTENSION}`)",
                field.name()
            )));
        }
        if field.data_type() != &DataType::Binary {
            return Err(contract(format!(
                "colonna geometria `{}` di tipo {}, atteso Binary",
                field.name(),
                field.data_type()
            )));
        }
        let definition = crs_definition_from_metadata(field.name(), geo_metadata)?;
        let crs = resolve_crs(&definition, "crs")?;
        geometries.push(GeometryColumnContract {
            field_id: FieldId(0),
            name: field.name().clone(),
            crs,
            dimensions: GeometryDimensions::Xy,
            nullable: field.is_nullable(),
        });
    }
    let active_geometry = if geometries.is_empty() {
        None
    } else {
        Some(FieldId(0))
    };
    DataContract::new(schema, geometries, active_geometry, ContractProperties::default())
}

/// Contesto "input `nome` (percorso)" sull'errore, preservando la variante.
fn at_input(name: &str, path: &Path, error: PlenoraError) -> PlenoraError {
    let prefix = |message: &String| format!("input `{name}` ({}): {message}", path.display());
    match error {
        PlenoraError::Contract(message) => PlenoraError::Contract(prefix(&message)),
        PlenoraError::Unsupported(message) => PlenoraError::Unsupported(prefix(&message)),
        PlenoraError::Schema(message) => PlenoraError::Schema(prefix(&message)),
        PlenoraError::Crs(message) => PlenoraError::Crs(prefix(&message)),
        other => other,
    }
}

/// Accoppia i percorsi CLI agli input dichiarati dal piano v4, in ordine di
/// dichiarazione (posizionale, deterministico). Un conteggio diverso e' un
/// errore esplicito, prima ancora di toccare i file.
fn pair_v4_inputs(plan_text: &str, paths: &[PathBuf]) -> Result<Vec<(String, PathBuf)>, PlenoraError> {
    let probe: PlanInputsProbe = serde_json::from_str(plan_text)?;
    if probe.inputs.len() != paths.len() {
        return Err(contract(format!(
            "il piano dichiara {} input ({}) ma ne sono stati forniti {}",
            probe.inputs.len(),
            probe.inputs.join(", "),
            paths.len()
        )));
    }
    Ok(probe.inputs.into_iter().zip(paths.iter().cloned()).collect())
}

/// Contratti degli input di un piano v4, scoperti dagli header IPC.
fn discover_contracts(pairs: &[(String, PathBuf)]) -> Result<Vec<(String, DataContract)>, PlenoraError> {
    pairs
        .iter()
        .map(|(name, path)| {
            discover_input_contract(path)
                .map(|contract| (name.clone(), contract))
                .map_err(|error| at_input(name, path, error))
        })
        .collect()
}

/// Sintesi JSON di un contratto d'arco: campi dello schema e geometria attiva.
fn contract_json(contract: &DataContract) -> serde_json::Value {
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
        serde_json::json!({
            "name": geometry.name,
            "crs": geometry.crs.definition(),
            "crs_kind": format!("{:?}", geometry.crs.kind()),
        })
    });
    serde_json::json!({
        "fields": fields,
        "geometry": geometry,
    })
}

/// Riepilogo JSON di `validate` per un piano v4: nodi, archi con contratti,
/// segmenti con modo e strategia, capability e identita' ADR 4.
fn graph_summary_json(graph: &ValidatedGraph, execution: &ExecutionPlan) -> serde_json::Value {
    let plan = graph.plan().plan();
    let nodes: Vec<serde_json::Value> = plan
        .nodes
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
    for name in &plan.inputs {
        edges.push(serde_json::json!({
            "edge": name,
            "kind": "input",
            "contract": contract_json(graph.edge_contract(name).expect("input del grafo validato")),
        }));
    }
    for node_id in graph.topological_order() {
        edges.push(serde_json::json!({
            "edge": node_id,
            "kind": "node",
            "contract": contract_json(graph.edge_contract(node_id).expect("arco del grafo validato")),
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
    serde_json::json!({
        "status": "ok",
        "schema_version": PLAN_SCHEMA_VERSION_V4,
        "plan_hash": graph.plan_hash().to_hex(),
        "engine_version": graph.engine_version().to_string(),
        "inputs": plan.inputs,
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
    })
}

/// Metriche JSON di un `run` v4: per nodo logico e per segmento (righe e
/// batch in/out, wall time in millisecondi), piu' i totali di pubblicazione.
fn metrics_json(graph: &ValidatedGraph, metrics: &ExecutionMetrics) -> serde_json::Value {
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
        "schema_version": PLAN_SCHEMA_VERSION_V4,
        "plan_hash": graph.plan_hash().to_hex(),
        "output_rows": metrics.output_rows,
        "output_batches": metrics.output_batches,
        "total_rows_processed": metrics.total_rows_processed,
        "nodes": nodes,
        "segments": segments,
    })
}

/// `validate` di un piano v4: planner DAG + `explain` per la strategia, con
/// riepilogo JSON su stdout (ADR 5: `prepare` e' interna all'engine).
fn validate_dag_v4(plan_text: &str, paths: &[PathBuf]) -> Result<(), Box<dyn Error>> {
    let pairs = pair_v4_inputs(plan_text, paths)?;
    let contracts = discover_contracts(&pairs)?;
    let graph = planner::validate(plan_text, &contracts)?;
    let execution = explain(&graph, &RuntimeContext::default())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&graph_summary_json(&graph, &execution))?
    );
    Ok(())
}

/// `run` di un piano v4: esecuzione DAG e pubblicazione atomica dell'output,
/// con metriche JSON su stdout.
fn run_dag_v4(plan_text: &str, paths: &[PathBuf], output_path: &Path) -> Result<(), Box<dyn Error>> {
    if output_path.exists() {
        return Err(contract(format!(
            "output gia' esistente, rifiuto di sovrascriverlo: {}",
            output_path.display()
        ))
        .into());
    }
    let pairs = pair_v4_inputs(plan_text, paths)?;
    let contracts = discover_contracts(&pairs)?;
    let graph = planner::validate(plan_text, &contracts)?;
    let mut inputs = Inputs::new();
    for (name, path) in &pairs {
        inputs.add(name.clone(), open_input(path)?)?;
    }
    let output = execute(&graph, inputs, RuntimeContext::default())?;
    let (metrics, outcome) =
        output.write_ipc_file_with_profile(output_path, PublishProfile::Atomic)?;
    report_publish_outcome(outcome, output_path);
    println!(
        "{}",
        serde_json::to_string_pretty(&metrics_json(&graph, &metrics))?
    );
    Ok(())
}

/// Percorsi di input per un piano v4: `--input` singolo e/o `--inputs`
/// multiplo (valori fino al prossimo flag).
fn v4_input_paths(args: &[String]) -> Result<Vec<PathBuf>, PlenoraError> {
    let mut paths = Vec::new();
    if let Some(single) = optional_value_after(args, "--input")? {
        paths.push(single);
    }
    if let Some(index) = args.iter().position(|argument| argument == "--inputs") {
        paths.extend(
            args[index + 1..]
                .iter()
                .take_while(|argument| !argument.starts_with("--"))
                .map(PathBuf::from),
        );
    }
    Ok(paths)
}

/// Dispatch di `run`: DAG v4 se il piano dichiara `schema_version: 4`,
/// pipeline tabellare legacy altrimenti (comportamento invariato).
fn run_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    let plan_path = value_after(args, "--plan")?;
    let output_path = value_after(args, "--output")?;
    let plan_text = std::fs::read_to_string(&plan_path)?;
    if plan_schema_version(&plan_text)? == u32::from(PLAN_SCHEMA_VERSION_V4) {
        if args.iter().any(|argument| argument == "--right") {
            return Err(contract(
                "--right non e' ammesso per i piani v4: usare --inputs con i percorsi \
                 nell'ordine di dichiarazione degli input del piano",
            )
            .into());
        }
        return run_dag_v4(&plan_text, &v4_input_paths(args)?, &output_path);
    }
    Ok(run_pipeline(
        &plan_path,
        &value_after(args, "--input")?,
        optional_value_after(args, "--right")?.as_deref(),
        &output_path,
    )?)
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn print_help() {
    eprintln!(
        "plenora-data-tools {}\n\n  plenora-data-tools catalog [--family table|geo]\n  plenora-data-tools validate --plan PLAN.json --inputs INPUT.arrow...\n  plenora-data-tools run --plan PLAN.json --input INPUT.arrow [--right RIGHT.arrow] --output OUTPUT.arrow   (piani legacy, schema_version <= 3)\n  plenora-data-tools run --plan PLAN.json --inputs INPUT.arrow... --output OUTPUT.arrow   (piani DAG v4: percorsi nell'ordine degli input dichiarati)\n  plenora-data-tools capabilities\n  plenora-data-tools transform --input INPUT --schema SCHEMA.json --output OUTPUT\n  plenora-data-tools spatial-join --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS\n  plenora-data-tools transform-arrow --input INPUT --schema SCHEMA.json --output OUTPUT\n  plenora-data-tools pair-arrow --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS\n  plenora-data-tools self-test [--output RESULT.bin]\n  plenora-data-tools --version",
        env!("CARGO_PKG_VERSION")
    );
}

fn run_with_args(args: &[String]) -> Result<(), Box<dyn Error>> {
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("plenora-data-tools {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("catalog") => catalog_command(args),
        Some("validate") => validate_command(args),
        Some("run") => run_command(args),
        Some("capabilities") => {
            let mut capabilities: Vec<_> = Operation::ALL
                .iter()
                .map(|operation| Capability {
                    name: operation.name(),
                    input: "wkb-frame-v2-sha256",
                    output: "wkb-frame-v2-sha256",
                })
                .collect();
            capabilities.push(Capability {
                name: "spatial_join",
                input: "2 x wkb-frame-v2-sha256",
                output: "index-pair-v1-sha256",
            });
            println!("{}", serde_json::to_string(&capabilities)?);
            Ok(())
        }
        Some("transform") => {
            let input = argument_value(args, "--input")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            let summary = execute_transform(&input, Path::new(&schema), &output)?;
            println!(
                "{{\"status\":\"ok\",\"rows\":{},\"sha256\":\"{}\"}}",
                summary.rows,
                hex_digest(&summary.checksum)
            );
            Ok(())
        }
        Some("transform-arrow") => {
            let input = argument_value(args, "--input")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            let summary = execute_transform_arrow(&input, Path::new(&schema), &output)?;
            println!(
                "{{\"status\":\"ok\",\"rows\":{},\"output_rows\":{},\"sha256\":\"{}\"}}",
                summary.rows,
                summary.output_rows,
                hex_digest(&summary.checksum)
            );
            Ok(())
        }
        Some("pair-arrow") => {
            let left = argument_value(args, "--left")?;
            let right = argument_value(args, "--right")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            if left == "-" || right == "-" || output == "-" {
                return Err(contract(
                    "pair-arrow richiede percorsi file per due input e output transazionale",
                )
                .into());
            }
            let summary = execute_pair_arrow(
                Path::new(&left),
                Path::new(&right),
                Path::new(&schema),
                Path::new(&output),
            )?;
            println!(
                "{{\"status\":\"ok\",\"left_rows\":{},\"right_rows\":{},\"output_rows\":{},\"sha256\":\"{}\"}}",
                summary.left_rows,
                summary.right_rows,
                summary.output_rows,
                hex_digest(&summary.checksum)
            );
            Ok(())
        }
        Some("spatial-join") => {
            let left = argument_value(args, "--left")?;
            let right = argument_value(args, "--right")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            if left == "-" || right == "-" || output == "-" {
                return Err(contract(
                    "spatial-join richiede percorsi file per due input e output transazionale",
                )
                .into());
            }
            let summary = execute_spatial_join(
                Path::new(&left),
                Path::new(&right),
                Path::new(&schema),
                Path::new(&output),
            )?;
            println!(
                "{{\"status\":\"ok\",\"pairs\":{},\"sha256\":\"{}\"}}",
                summary.pairs,
                hex_digest(&summary.checksum)
            );
            Ok(())
        }
        Some("self-test") => self_test_command(args),
        _ => {
            print_help();
            Err(contract("comando non valido").into())
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Err(error) = run_with_args(&args) {
        eprintln!("plenora-data-tools: {error}");
        std::process::exit(2);
    }
}
