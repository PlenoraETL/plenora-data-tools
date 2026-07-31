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
//! senza dati sensibili. Fase 2B M1c (ADR 3): `run` installa un handler
//! Ctrl-C che cancella cooperativamente l'esecuzione DAG v4 tramite
//! `CancellationToken` — al cancel nessun output e' pubblicato, messaggio
//! pulito ed exit code dedicato 130 (128 + SIGINT); un secondo Ctrl-C forza
//! l'uscita immediata.

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
    ContractCrs, ContractProperties, ContractProperty, CrsResolution, DataContract, FieldId,
    GeometryColumnContract, GeometryDimensions, PropertyConfidence, PropertyScope,
};
use plenora_core::crs::{required_definition, validate_requirement};
use plenora_core::{ErrorPhase, PlenoraError};
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
    execute, explain, CancellationToken, ExecutionMetrics, ExecutionPlan, Input, Inputs,
    RuntimeContext,
};
use plenora_kernels_geo::arrow_adapter::{
    read_contract_version, read_geometry_contract_keys, CanonicalGeometryKeys,
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
};
use plenora_kernels_geo::spatial_join::{spatial_join_nullable, JoinPredicate};
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, Operation};
use rayon::prelude::*;
use serde::Deserialize;

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

// ---------------------------------------------------------------------------
// Helper comuni
// ---------------------------------------------------------------------------

fn contract(message: impl Into<String>) -> PlenoraError {
    PlenoraError::InvalidPlan(message.into())
}

/// Exit code dedicato alla cancellazione (ADR 3, M1c): 128 + SIGINT,
/// convenzione POSIX — distinto dal 2 generico degli errori.
const EXIT_CANCELLED: i32 = 130;

/// Handler Ctrl-C (ADR 3, M1c): il primo Ctrl-C cancella il token —
/// l'executor si ferma al prossimo confine cooperativo con
/// `PlenoraError::Cancelled` e la CLI esce con [`EXIT_CANCELLED`] senza
/// pubblicare nulla; il secondo forza l'uscita immediata (comportamento
/// accettato e documentato in ADR 3: un kernel `NonInterruptible` in corso
/// non offre altri punti di interruzione).
///
/// `ctrlc::set_handler` e' installabile una sola volta per processo: la CLI
/// esegue un comando per processo, quindi un fallimento e' un errore vero
/// (fail-closed).
fn install_ctrlc_handler(token: &CancellationToken) -> Result<(), PlenoraError> {
    let token = token.clone();
    let requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    ctrlc::set_handler(move || {
        if requested.swap(true, std::sync::atomic::Ordering::SeqCst) {
            eprintln!("plenora-data-tools: secondo ctrl-c: uscita forzata");
            std::process::exit(EXIT_CANCELLED);
        }
        eprintln!("plenora-data-tools: ctrl-c: annullamento in corso (un secondo ctrl-c forza l'uscita)...");
        token.cancel();
    })
    .map_err(|error| contract(format!("handler ctrl-c non installabile: {error}")))
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
    const BATCH_ROWS: usize = 4096;
    const BATCH_BYTES: usize = 64 * 1024 * 1024;

    if schema.schema_version != 2 {
        return Err(contract(format!(
            "schema_version {} non supportata",
            schema.schema_version
        ))
        .into());
    }

    let mut input = FrameReader::new(reader, schema.row_count)?;
    let mut output = FrameWriter::new(writer, schema.row_count)?;

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
    const MAX_JOIN_ROWS_PER_SIDE: u64 = 2_000_000;
    const MAX_JOIN_INPUT_BYTES: u64 = 1024 * 1024 * 1024;

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
        return validate_dag_v4(&plan_text, &inputs, !has_flag(args, "--no-geo-fusion"));
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

// Quoting Debug intenzionale nel ramo geo: produce la stringa JSON del
// percorso (virgolette ed escape); il `.display()` suggerito da clippy
// cambierebbe l'output del comando (contratto CLI).
#[allow(clippy::unnecessary_debug_formatting)]
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
//   + metadato `geo` con chiave `crs`) o le chiavi canoniche, il
//   `GeometryColumnContract` con lo stato CRS deciso da
//   `contract_crs_from_keys` (risolto se una sola rappresentazione,
//   `DeclaredUnresolved` se dichiarato o in conflitto decidibile, `Missing`
//   se assente — R4.6.3). Metadati incoerenti (estensione senza `geo.crs`,
//   `geo` senza estensione, colonna non `Binary`, piu' di una colonna
//   geometrica — D16) sono rifiutati. Il `FieldId` della geometria di
//   input e' provvisorio: il planner lo rimappa nel namespace del grafo;
// - **decisioni CRS del piano** (`crs_decisions`, R4.6.3): applicate ai
//   contratti scoperti prima della validazione — la definizione decisa
//   sostituisce uno stato `DeclaredUnresolved` (risolta col backend PROJ,
//   feature-dispatch come gli altri percorsi);
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
/// con i percorsi CLI; la validazione vera resta al planner) e delle
/// decisioni CRS esplicite (R4.6.3, applicate da [`apply_crs_decisions`]).
#[derive(Debug, Deserialize)]
struct PlanInputsProbe {
    #[serde(default)]
    inputs: Vec<String>,
    #[serde(default)]
    crs_decisions: std::collections::BTreeMap<String, String>,
}

/// `schema_version` del piano, senza validazione strutturale.
fn plan_schema_version(plan_text: &str) -> Result<u32, PlenoraError> {
    Ok(serde_json::from_str::<PlanVersionProbe>(plan_text)?.schema_version)
}

/// `true` se il file inizia con il magic dell'Arrow IPC **file format**
/// (`ARROW1`); altrimenti e' trattato come IPC stream format.
///
/// Confine di lettura (BLOCK-03): gli errori I/O dello sniffing nascono
/// leggendo la sorgente — tag [`ErrorPhase::Read`].
fn is_ipc_file_format(path: &Path) -> Result<bool, PlenoraError> {
    let sniffed = (|| -> Result<bool, PlenoraError> {
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
    })();
    sniffed.map_err(|error| error.with_phase(ErrorPhase::Read))
}

/// Schema Arrow dell'header IPC di un input (file o stream format): nessuna
/// riga di dati letta.
///
/// Confine di lettura (BLOCK-03): gli errori `Io`/`DataMapping` di apertura
/// e parse dell'header nascono leggendo la sorgente — tag
/// [`ErrorPhase::Read`] (lo sniffing del formato e' gia' taggato da
/// [`is_ipc_file_format`], il primo tag vince).
fn ipc_header_schema(path: &Path) -> Result<SchemaRef, PlenoraError> {
    let header = (|| -> Result<SchemaRef, PlenoraError> {
        if is_ipc_file_format(path)? {
            Ok(FileReader::try_new(File::open(path)?, None)?.schema())
        } else {
            Ok(StreamReader::try_new(File::open(path)?, None)?.schema())
        }
    })();
    header.map_err(|error| error.with_phase(ErrorPhase::Read))
}

/// Input lazy per l'executor: IPC file o stream format, sniffato dal magic.
fn open_input(path: &Path) -> Result<Input, PlenoraError> {
    if is_ipc_file_format(path)? {
        Input::read_ipc_file(path)
    } else {
        Input::read_ipc_stream(path)
    }
}

/// Definizione CRS dal metadato `geo` di una colonna `GeoArrow`: stringa
/// `authority:code` oppure PROJJSON come oggetto (serializzato compatto).
///
/// R4.6.3: il metadato mancante o privo della chiave `crs` NON e' piu' un
/// errore (restituisce `None` → [`ContractCrs::Missing`]): il centro non puo'
/// pretendere un CRS risolvibile per operazioni che non lo richiedono. Un
/// metadato MALFORMATO resta un errore (R5.1: «illeggibile» non e'
/// «assente»).
fn crs_definition_from_metadata(
    field_name: &str,
    geo_metadata: Option<&String>,
) -> Result<Option<String>, PlenoraError> {
    let Some(raw) = geo_metadata else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(raw)?;
    match value.get("crs") {
        None => Ok(None),
        Some(serde_json::Value::String(definition)) => Ok(Some(definition.clone())),
        Some(object @ serde_json::Value::Object(_)) => Ok(Some(serde_json::to_string(object)?)),
        Some(_) => Err(contract(format!(
            "colonna geometria `{field_name}`: metadato `{GEO_METADATA_KEY}` senza \
             chiave `crs` valida"
        ))),
    }
}

/// Scoperta del `DataContract` di un input dal solo header IPC: schema Arrow
/// e colonne geometria se presenti, CRS risolto SE dichiarato. Fail-closed
/// su metadati incoerenti. Il `FieldId` e' provvisorio: il planner lo rimappa
/// nel namespace globale del grafo (D16).
///
/// Milestone C (protocollo chiavi canoniche, contratti trasversali §2):
///
/// - gate R2.5 all'ingresso: [`read_contract_version`] — una versione
///   successiva a quella nota, una chiave di versione malformata o chiavi
///   canoniche senza versione sono errori propagati, mai ignorati;
/// - sorgente primaria per ogni campo geometria:
///   [`read_geometry_contract_keys`] — chiavi canoniche fail-closed,
///   coerenza canonica/legacy (R2.6: divergenza -> errore) e completamento
///   per precedenza (R2.7) sono applicati dal reader;
/// - riconoscimento (1c): le chiavi canoniche sono autosufficienti (tabella
///   §2), quindi un campo con chiavi `plenora.geometry.*` e' riconosciuto
///   come colonna geometrica anche SENZA l'estensione `geoarrow.wkb` e il
///   metadato `geo`; in entrambi i percorsi il tipo DEVE essere `Binary`,
///   altrimenti errore esplicito;
/// - CRS: lo stato e' deciso da [`contract_crs_from_keys`] sulla
///   rappresentazione completata (canonica o legacy) e sulla
///   `crs_resolution` dichiarata. R4.6.3 (v2.0-rc9/rc10): un campo
///   geometrico SENZA CRS dichiarato NON e' un errore — il centro non puo'
///   pretendere un CRS risolvibile per operazioni che non lo richiedono;
///   lo stato entra nel contratto come [`ContractCrs::Missing`] (R4.4: mai
///   un CRS inventato) e ferma solo le op che dichiarano un
///   `CrsRequirement`, in analyze. Un'incoerenza dichiarata
///   (`declared_unresolved`) o un conflitto decidibile fra rappresentazioni
///   diventano [`ContractCrs::DeclaredUnresolved`], preservati e mai
///   risolti in assenza di una decisione esplicita nel piano
///   (`crs_decisions`). Resta errore la dichiarazione contraddittoria
///   (`crs_resolution` valorizzata ma nessuna rappresentazione: R4.1 vieta
///   di collassarla su `missing`).
fn discover_input_contract(path: &Path) -> Result<DataContract, PlenoraError> {
    discover_input_contract_from_schema(ipc_header_schema(path)?)
}

/// Scoperta da schema Arrow gia' letto (seam di test: nessun file toccato).
/// Le regole sono quelle di [`discover_input_contract`].
fn discover_input_contract_from_schema(schema: SchemaRef) -> Result<DataContract, PlenoraError> {
    // Gate R2.5: la versione del protocollo vive nei metadati dello schema.
    read_contract_version(&schema)?;
    let mut geometries = Vec::new();
    for field in schema.fields() {
        let extension = field.metadata().get(GEOARROW_EXTENSION_KEY);
        let geo_metadata = field.metadata().get(GEO_METADATA_KEY);
        if let Some(extension) = extension {
            if extension != GEOARROW_WKB_EXTENSION {
                return Err(contract(format!(
                    "colonna `{}`: estensione `{extension}` non supportata \
                     (attesa `{GEOARROW_WKB_EXTENSION}`)",
                    field.name()
                )));
            }
        } else {
            if geo_metadata.is_some() {
                return Err(contract(format!(
                    "colonna `{}`: metadato `{GEO_METADATA_KEY}` senza estensione \
                     `{GEOARROW_EXTENSION_KEY}`: metadati incoerenti",
                    field.name()
                )));
            }
            // (1c) le chiavi canoniche sono autosufficienti (tabella §2):
            // il campo si dichiara colonna geometrica da solo.
            let canonical = field
                .metadata()
                .keys()
                .any(|key| key.starts_with(PLENORA_GEOMETRY_NAMESPACE_PREFIX));
            if !canonical {
                continue;
            }
        }
        if field.data_type() != &DataType::Binary {
            return Err(contract(format!(
                "colonna geometria `{}` di tipo {}, atteso Binary",
                field.name(),
                field.data_type()
            )));
        }
        let keys = read_geometry_contract_keys(field)?;
        let crs = contract_crs_from_keys(field.name(), geo_metadata, &keys)?;
        geometries.push(geometry_contract_from_field(field, crs, &keys));
    }
    let active_geometry = if geometries.is_empty() {
        None
    } else {
        Some(FieldId(0))
    };
    DataContract::new(schema, geometries, active_geometry, ContractProperties::default())
}

/// Stato CRS del contratto dalla lettura di contratto completata (R2.7),
/// con la collocazione di R4.6.3 (v2.0-rc9/rc10): il centro NON risolve
/// un'incoerenza dichiarata in assenza di una decisione esplicita nel piano
/// — la preserva come [`ContractCrs::DeclaredUnresolved`] con le
/// dichiarazioni originali, mai un errore (non e' il bordo di scrittura) e
/// mai una scelta silenziosa.
///
/// Regole, in ordine:
///
/// 1. `crs_resolution = declared_unresolved` con almeno una
///    rappresentazione: il produttore dichiara l'incoerenza — preservata
///    cosi' com'e', NESSUNA risoluzione tentata (cambio di comportamento
///    dichiarato: prima una definizione risolvibile era risolta ed emessa
///    come `resolved`; nessuna chiamata al backend, quindi nessun
///    `BackendUnavailable`);
/// 2. conflitto DECIDIBILE senza backend: `crs_id` e `crs_definition`
///    co-presenti (l'accordo non e' decidibile testualmente — R2.7: mai
///    arbitrato sul dato; prima vinceva `crs_definition`, scelta
///    silenziosa) oppure `crs_id` nella forma `authority:code` con codice
///    numerico discordante da `srid` (R4.3.1; prima lo `srid` era ignorato
///    e l'identificatore risolto — conciliazione silenziosa): lo stato
///    diventa `DeclaredUnresolved` con le dichiarazioni;
/// 3. una sola rappresentazione (canonica o legacy `geo.crs`): risoluzione
///    contro il backend PROJ, come sempre — un fallimento di risoluzione
///    resta un errore `Crs`, NON diventa `DeclaredUnresolved` (limite
///    dichiarato: il produttore che sa di non poter garantire la
///    risoluzione dichiara `declared_unresolved` esplicitamente, come nel
///    corpus di conformita');
/// 4. nessuna rappresentazione: [`ContractCrs::Missing`] (R4.4: mai un CRS
///    inventato), salvo la contraddizione R4.1 — `resolved`/
///    `declared_unresolved` senza alcuna rappresentazione — che resta
///    errore di discovery.
fn contract_crs_from_keys(
    field_name: &str,
    geo_metadata: Option<&String>,
    keys: &CanonicalGeometryKeys,
) -> Result<ContractCrs, PlenoraError> {
    let crs_id = keys.crs_id.clone();
    let definition = keys.crs_definition.clone();
    // (1) Incoerenza dichiarata dal produttore: preservata, mai risolta.
    if keys.crs_resolution == Some(CrsResolution::DeclaredUnresolved)
        && (crs_id.is_some() || definition.is_some())
    {
        return Ok(ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format: keys.crs_definition_format,
        });
    }
    // (2a) Due rappresentazioni risolvibili co-presenti: accordo non
    // decidibile, il centro non sceglie.
    if crs_id.is_some() && definition.is_some() {
        return Ok(ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format: keys.crs_definition_format,
        });
    }
    // (2b) Identificatore con codice numerico contro SRID discordante.
    if let (Some(id), Some(srid)) = (&crs_id, keys.srid) {
        if authority_code(id).is_some_and(|code| code != srid) {
            return Ok(ContractCrs::DeclaredUnresolved {
                crs_id,
                definition: None,
                definition_format: None,
            });
        }
    }
    // (3) La rappresentazione completata (canonica o legacy) alimenta la
    // stessa risoluzione di sempre.
    if let Some(definition) = definition.or(crs_id) {
        return Ok(ContractCrs::Resolved(resolve_crs(&definition, "crs")?));
    }
    if let Some(definition) = crs_definition_from_metadata(field_name, geo_metadata)? {
        return Ok(ContractCrs::Resolved(resolve_crs(&definition, "crs")?));
    }
    // (4) R4.1: mai collassare una dichiarazione esplicita su `missing` —
    // `resolved`/`declared_unresolved` senza alcuna rappresentazione e' una
    // contraddizione, non un'assenza.
    if let Some(resolution) = keys.crs_resolution {
        if resolution != CrsResolution::Missing {
            return Err(contract(format!(
                "colonna geometria `{field_name}`: chiave \
                 `{PLENORA_GEOMETRY_CRS_RESOLUTION_KEY}` dichiara `{resolution}` ma \
                 nessun CRS e' dichiarato in alcuna rappresentazione accettata"
            )));
        }
    }
    Ok(ContractCrs::Missing)
}

/// Codice numerico di un identificatore `authority:code` (es. `EPSG:4326`
/// -> 4326); `None` per ogni altra forma — il confronto con `srid` non e'
/// decidibile e l'identificatore resta intero alla risoluzione.
fn authority_code(crs_id: &str) -> Option<u32> {
    let (authority, code) = crs_id.rsplit_once(':')?;
    if authority.is_empty() || code.is_empty() || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    code.parse().ok()
}

/// Contesto "input `nome` (percorso)" sull'errore, preservando la variante.
fn at_input(name: &str, path: &Path, error: PlenoraError) -> PlenoraError {
    let prefix = |message: &String| format!("input `{name}` ({}): {message}", path.display());
    match error {
        PlenoraError::InvalidPlan(message) => PlenoraError::InvalidPlan(prefix(&message)),
        PlenoraError::Unsupported(message) => PlenoraError::Unsupported(prefix(&message)),
        PlenoraError::Schema(message) => PlenoraError::Schema(prefix(&message)),
        PlenoraError::Crs(message) => PlenoraError::Crs(prefix(&message)),
        other => other,
    }
}

/// Contratto della colonna geometria dalla lettura di contratto completata
/// (milestone C: [`read_geometry_contract_keys`] come sorgente primaria —
/// fail-closed R2.6 e completamento R2.7 gia' applicati dal reader).
///
/// Dimensionalita' ed encoding arrivano dalle chiavi completate: assenti ->
/// `Unknown` / `None` (R3.4: MAI un default silenzioso `Xy`). `types`: la
/// coppia `types_declaration`/`types`, se presente, entra nel contratto con
/// confidence `Declared` e scope `Schema`; assente (ingresso legacy) ->
/// [`GeometryColumnContract::undeclared_types`] (R3.4.1: «proprieta' non
/// dichiarata», MAI interpretata come `unresolved`). Il `FieldId` e'
/// provvisorio (rimappato dal planner, D16).
fn geometry_contract_from_field(
    field: &plenora_core::arrow::schema::Field,
    crs: ContractCrs,
    keys: &CanonicalGeometryKeys,
) -> GeometryColumnContract {
    let types = keys.types.as_ref().map_or_else(
        GeometryColumnContract::undeclared_types,
        |types| {
            ContractProperty::new(
                PropertyConfidence::Declared(types.clone()),
                PropertyScope::Schema,
            )
        },
    );
    GeometryColumnContract {
        field_id: FieldId(0),
        name: field.name().clone(),
        crs,
        dimensions: keys.dimensions.unwrap_or(GeometryDimensions::Unknown),
        encoding: keys.encoding,
        nullable: field.is_nullable(),
        types,
    }
}

/// Accoppia i percorsi CLI agli input dichiarati dal piano v4, in ordine di
/// dichiarazione (posizionale, deterministico). Un conteggio diverso e' un
/// errore esplicito, prima ancora di toccare i file.
fn pair_v4_inputs(probe: &PlanInputsProbe, paths: &[PathBuf]) -> Result<Vec<(String, PathBuf)>, PlenoraError> {
    if probe.inputs.len() != paths.len() {
        return Err(contract(format!(
            "il piano dichiara {} input ({}) ma ne sono stati forniti {}",
            probe.inputs.len(),
            probe.inputs.join(", "),
            paths.len()
        )));
    }
    Ok(probe.inputs.iter().cloned().zip(paths.iter().cloned()).collect())
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

/// Applica le decisioni CRS esplicite del piano v4 (`crs_decisions`,
/// R4.6.3) ai contratti scoperti: per ogni input nominato, la definizione
/// decisa e' risolta contro il backend (senza `proj-backend`:
/// `BackendUnavailable`, come il CRS di piano) e sostituisce lo stato
/// [`ContractCrs::DeclaredUnresolved`] con
/// [`ContractCrs::ResolvedByDecision`] — un CRS risolto a tutti gli effetti
/// per le op a valle, marcato perche' l'emissione SOSTITUISCA le
/// dichiarazioni della sorgente con il CRS deciso
/// (`strip_decided_crs_declarations` nella fusione dello schema di
/// output). Lo schema del contratto di input NON e' toccato: il check
/// fail-closed dell'executor confronta i campi del file con quelli del
/// contratto validato (metadati inclusi). La decisione resta esplicita nel
/// piano e coperta dal `plan_hash` (ADR 4); il fingerprint del contratto
/// di input cambia di conseguenza (un piano con decisione non accetta in
/// riesecuzione l'input non deciso senza rivalidazione).
///
/// Errori espliciti (mai una decisione ignorata in silenzio): input non
/// fornito o senza colonna geometrica; stato diverso da
/// `DeclaredUnresolved` (su `Missing` sarebbe un CRS inventato — R4.4; su
/// `Resolved` una contraddizione del piano); definizione non risolvibile.
/// I messaggi non riportano valori di dichiarazioni (regola «errori senza
/// dati»).
fn apply_crs_decisions(
    probe: &PlanInputsProbe,
    contracts: &mut [(String, DataContract)],
) -> Result<(), PlenoraError> {
    for (input, definition) in &probe.crs_decisions {
        let Some((_, input_contract)) = contracts.iter_mut().find(|(name, _)| name == input) else {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` non e' tra i contratti scoperti"
            )));
        };
        if input_contract.geometries.len() != 1 {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` non dichiara esattamente una colonna \
                 geometrica: la decisione non e' applicabile"
            )));
        }
        let geometry = &mut input_contract.geometries[0];
        if !matches!(geometry.crs, ContractCrs::DeclaredUnresolved { .. }) {
            return Err(contract(format!(
                "crs_decisions: l'input `{input}` dichiara il CRS come `{}`, non come \
                 `declared_unresolved`: la decisione non e' applicabile",
                geometry.crs.resolution()
            )));
        }
        geometry.crs = ContractCrs::ResolvedByDecision(resolve_crs(definition, "crs")?);
    }
    Ok(())
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

/// Metriche JSON di un `run` v4: per nodo logico e per segmento (righe,
/// batch e byte in/out, wall time in millisecondi), i totali di
/// pubblicazione, il contatore dei fallback della fusione geo (D12.7: ogni
/// fallback governor e' osservabile, mai silenzioso), l'osservabilita' dei
/// lease di memoria e le metriche di spill aggregate (ADR-0002).
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
        "schema_version": PLAN_SCHEMA_VERSION_V4,
        "plan_hash": graph.plan_hash().to_hex(),
        "output_rows": metrics.output_rows,
        "output_batches": metrics.output_batches,
        "total_rows_processed": metrics.total_rows_processed,
        "geo_fusion_fallbacks": metrics.geo_fusion_fallbacks,
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

/// Presenza di un flag booleano negli argomenti (es. `--no-geo-fusion`).
fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|argument| argument == flag)
}

/// `validate` di un piano v4: planner DAG + `explain` per la strategia, con
/// riepilogo JSON su stdout (ADR 5: `prepare` e' interna all'engine).
/// `geo_fusion` e' il kill switch D12.9 (flag `--no-geo-fusion`): a `false`
/// i gruppi di fusione non si formano e `explain` mostra la strategia non
/// fusa.
fn validate_dag_v4(
    plan_text: &str,
    paths: &[PathBuf],
    geo_fusion: bool,
) -> Result<(), Box<dyn Error>> {
    let probe: PlanInputsProbe = serde_json::from_str(plan_text)?;
    let pairs = pair_v4_inputs(&probe, paths)?;
    let mut contracts = discover_contracts(&pairs)?;
    apply_crs_decisions(&probe, &mut contracts)?;
    let graph = planner::validate(plan_text, &contracts)?;
    let execution = explain(&graph, &RuntimeContext {
        geo_fusion,
        ..RuntimeContext::default()
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&graph_summary_json(&graph, &execution))?
    );
    Ok(())
}

/// `run` di un piano v4: esecuzione DAG e pubblicazione atomica dell'output,
/// con metriche JSON su stdout. Installa l'handler Ctrl-C: al cancel
/// l'executor propaga `PlenoraError::Cancelled`, il publish atomico non e'
/// mai raggiunto e `main` esce con [`EXIT_CANCELLED`]. `geo_fusion` e' il
/// kill switch D12.9 (flag `--no-geo-fusion`).
fn run_dag_v4(
    plan_text: &str,
    paths: &[PathBuf],
    output_path: &Path,
    geo_fusion: bool,
) -> Result<(), Box<dyn Error>> {
    if output_path.exists() {
        return Err(contract(format!(
            "output gia' esistente, rifiuto di sovrascriverlo: {}",
            output_path.display()
        ))
        .into());
    }
    let probe: PlanInputsProbe = serde_json::from_str(plan_text)?;
    let pairs = pair_v4_inputs(&probe, paths)?;
    let mut contracts = discover_contracts(&pairs)?;
    apply_crs_decisions(&probe, &mut contracts)?;
    let graph = planner::validate(plan_text, &contracts)?;
    let mut inputs = Inputs::new();
    for (name, path) in &pairs {
        inputs.add(name.clone(), open_input(path)?)?;
    }
    let token = CancellationToken::new();
    install_ctrlc_handler(&token)?;
    let runtime = RuntimeContext {
        cancellation: token,
        geo_fusion,
        ..RuntimeContext::default()
    };
    let output = execute(&graph, inputs, runtime)?;
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
        return run_dag_v4(
            &plan_text,
            &v4_input_paths(args)?,
            &output_path,
            !has_flag(args, "--no-geo-fusion"),
        );
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
        "plenora-data-tools {}\n\n  plenora-data-tools catalog [--family table|geo]\n  plenora-data-tools validate --plan PLAN.json --inputs INPUT.arrow... [--no-geo-fusion]\n  plenora-data-tools run --plan PLAN.json --input INPUT.arrow [--right RIGHT.arrow] --output OUTPUT.arrow   (piani legacy, schema_version <= 3)\n  plenora-data-tools run --plan PLAN.json --inputs INPUT.arrow... --output OUTPUT.arrow [--no-geo-fusion]   (piani DAG v4: percorsi nell'ordine degli input dichiarati)\n  plenora-data-tools capabilities\n  plenora-data-tools transform --input INPUT --schema SCHEMA.json --output OUTPUT\n  plenora-data-tools spatial-join --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS\n  plenora-data-tools transform-arrow --input INPUT --schema SCHEMA.json --output OUTPUT\n  plenora-data-tools pair-arrow --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS\n  plenora-data-tools self-test [--output RESULT.bin]\n  plenora-data-tools --version",
        env!("CARGO_PKG_VERSION")
    );
}

// Dispatch unico dei sottocomandi: la lunghezza e' data dalla sequenza
// lineare dei casi, non da complessita' logica; uno spezzone artificiale
// peggiorerebbe solo la leggibilita' (fase di pulizia: niente refactor
// strutturali).
#[allow(clippy::too_many_lines)]
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
            // ICD §10 R10.2: capability dichiarative interrogabili prima
            // dell'esecuzione, in forma leggibile da un programma — il
            // documento completo (modello geometrico + catalogo, fonte
            // unica in plenora-core::capabilities).
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &plenora_core::capabilities::component_capabilities()
                )?
            );
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
        // Cancellazione cooperativa (ADR 3, M1c): exit code dedicato; il
        // publish atomico garantisce che nessun output parziale sia stato
        // pubblicato. Envelope §9 anche per la cancellazione (categoria
        // dedicata, fase/effetto/retry dagli assi).
        let cancelled =
            matches!(error.downcast_ref::<PlenoraError>(), Some(PlenoraError::Cancelled { .. }));
        eprintln!("{}", error_envelope(error.as_ref(), cancelled));
        std::process::exit(if cancelled { EXIT_CANCELLED } else { 2 });
    }
}

/// Envelope d'errore a quattro assi (R9.1, `protocol_version` 1): l'uscita
/// CLI riporta categoria, fase, effetto remoto e disposizione di retry
/// espliciti — mai dedotti dal messaggio (R9.2). Una riga JSON su stderr;
/// `message` porta il testo dell'errore invariato. `context` (presente
/// solo per errori nati in un'esecuzione DAG) riporta nodo, operazione ed
/// `execution_id` — la risposta a «quale step ha rotto» senza parsare il
/// messaggio. Gli exit code restano 2 (errore) e 130 (cancellazione).
///
/// Mapping dichiarato: `PlenoraError` -> i quattro assi del tipo; errori
/// I/O nudi (lettura piano/argomenti) -> `io`/`read`/`none`/`safe`;
/// errori di parse JSON del piano -> `data_mapping`/`validate`/`none`/
/// `never`; qualunque altro tipo -> `internal`/`validate`/`none`/`never`.
fn error_envelope(error: &(dyn Error + 'static), cancelled: bool) -> serde_json::Value {
    let (category, phase, remote_effect, retry) = error.downcast_ref::<PlenoraError>().map_or_else(
        || {
            if error.downcast_ref::<std::io::Error>().is_some() {
                ("io", "read", "none", "safe")
            } else if error.downcast_ref::<serde_json::Error>().is_some() {
                ("data_mapping", "validate", "none", "never")
            } else {
                ("internal", "validate", "none", "never")
            }
        },
        |plenora| {
            (
                plenora.category().as_str(),
                plenora.phase().as_str(),
                plenora.remote_effect().as_str(),
                plenora.retry_disposition().as_str(),
            )
        },
    );
    let message = if cancelled {
        format!("esecuzione annullata: {error}")
    } else {
        error.to_string()
    };
    let mut body = serde_json::json!({
        "category": category,
        "phase": phase,
        "remote_effect": remote_effect,
        "retry": retry,
        "message": message,
    });
    if let Some(
        PlenoraError::Execution { node, operation, execution_id, .. }
        | PlenoraError::Cancelled { node, operation, execution_id, .. },
    ) = error.downcast_ref::<PlenoraError>()
    {
        let mut context = serde_json::json!({ "node": node, "operation": operation });
        if !execution_id.is_empty() {
            context["execution_id"] = serde_json::Value::String(execution_id.clone());
        }
        body["context"] = context;
    }
    serde_json::json!({
        "status": "error",
        "protocol_version": 1,
        "error": body,
    })
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use plenora_core::arrow::schema::{Field, Schema};
    use plenora_core::contract::{GeometryDimensions, GeometryEncoding};
    #[cfg(feature = "proj-backend")]
    use plenora_core::contract::{GeometryType, TypesDeclaration};
    use plenora_core::crs::{CrsKind, ResolvedCrs};
    use plenora_kernels_geo::arrow_adapter::{
        PLENORA_CONTRACT_VERSION_KEY, PLENORA_GEOMETRY_AXIS_ORDER_KEY,
        PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
        PLENORA_GEOMETRY_CRS_ID_KEY, PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
        PLENORA_GEOMETRY_DIMENSIONS_KEY, PLENORA_GEOMETRY_ENCODING_KEY,
        PLENORA_GEOMETRY_SRID_KEY, PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
        PLENORA_GEOMETRY_TYPES_KEY,
    };

    use super::*;

    /// Envelope §9: gli assi di un `PlenoraError` arrivano in uscita
    /// espliciti (R9.2), il contesto DAG solo quando presente.
    #[test]
    fn error_envelope_carries_the_four_axes_and_dag_context() {
        let error = PlenoraError::Execution {
            node: "t".to_owned(),
            operation: "geo.centroid".to_owned(),
            execution_id: "exec-1".to_owned(),
            reason: "boom".to_owned(),
        };
        let envelope = error_envelope(&error, false);
        assert_eq!(envelope["status"], "error");
        assert_eq!(envelope["protocol_version"], 1);
        assert_eq!(envelope["error"]["category"], "execution");
        assert_eq!(envelope["error"]["phase"], "write");
        assert_eq!(envelope["error"]["remote_effect"], "none");
        assert_eq!(envelope["error"]["retry"], "never");
        assert_eq!(envelope["error"]["context"]["node"], "t");
        assert_eq!(envelope["error"]["context"]["operation"], "geo.centroid");
        assert_eq!(envelope["error"]["context"]["execution_id"], "exec-1");
        assert!(
            envelope["error"]["message"]
                .as_str()
                .expect("message")
                .contains("step failed at node `t`"),
            "il testo Display resta in message: {envelope}"
        );
    }

    #[test]
    fn error_envelope_omits_context_and_empty_execution_id() {
        let contract = PlenoraError::Unsupported("operazione sconosciuta".to_owned());
        let envelope = error_envelope(&contract, false);
        assert_eq!(envelope["error"]["category"], "unsupported");
        assert_eq!(envelope["error"]["phase"], "validate");
        assert_eq!(envelope["error"]["retry"], "never");
        assert!(envelope["error"].get("context").is_none());

        let legacy_step = PlenoraError::Execution {
            node: "n".to_owned(),
            operation: "table.filter".to_owned(),
            execution_id: String::new(),
            reason: "boom".to_owned(),
        };
        let legacy = error_envelope(&legacy_step, false);
        assert!(legacy["error"]["context"].get("execution_id").is_none());
    }

    #[test]
    fn error_envelope_maps_io_json_and_unknown_errors() {
        let io = std::io::Error::other("disco pieno");
        let envelope = error_envelope(&io, false);
        assert_eq!(envelope["error"]["category"], "io");
        assert_eq!(envelope["error"]["phase"], "read");
        assert_eq!(envelope["error"]["retry"], "safe");

        let json = serde_json::from_str::<u32>("\"non-un-numero\"").expect_err("json invalido");
        let envelope = error_envelope(&json, false);
        assert_eq!(envelope["error"]["category"], "data_mapping");
        assert_eq!(envelope["error"]["phase"], "validate");
        assert_eq!(envelope["error"]["retry"], "never");
    }

    #[test]
    fn error_envelope_cancelled_keeps_dedicated_message_and_axes() {
        let error = PlenoraError::Cancelled {
            node: "t".to_owned(),
            operation: "table.filter".to_owned(),
            execution_id: "exec-9".to_owned(),
            reason: "cancellazione richiesta".to_owned(),
        };
        let envelope = error_envelope(&error, true);
        assert_eq!(envelope["error"]["category"], "cancelled");
        assert_eq!(envelope["error"]["retry"], "never");
        assert!(
            envelope["error"]["message"]
                .as_str()
                .expect("message")
                .starts_with("esecuzione annullata: "),
            "messaggio dedicato preservato: {envelope}"
        );
        assert_eq!(envelope["error"]["context"]["execution_id"], "exec-9");
    }

    fn projected_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            serde_json::json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    /// Campo geometria GeoArrow-WKB con il metadato `geo` dato (o senza).
    fn geometry_field(geo_json: Option<&str>) -> Field {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        );
        if let Some(geo) = geo_json {
            metadata.insert(GEO_METADATA_KEY.to_owned(), geo.to_owned());
        }
        Field::new("geometry", DataType::Binary, true).with_metadata(metadata)
    }

    /// Campo geometria con SOLE chiavi canoniche (niente `GeoArrow` legacy).
    fn canonical_geometry_field(data_type: DataType) -> Field {
        let metadata = std::collections::HashMap::from([
            (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned()),
            (
                PLENORA_GEOMETRY_TYPES_DECLARATION_KEY.to_owned(),
                "exact".to_owned(),
            ),
            (PLENORA_GEOMETRY_TYPES_KEY.to_owned(), "point".to_owned()),
            (
                PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                "resolved".to_owned(),
            ),
            (
                PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(),
                "EPSG:32632".to_owned(),
            ),
            (
                PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                "unknown".to_owned(),
            ),
        ]);
        Field::new("geometry", data_type, true).with_metadata(metadata)
    }

    /// Schema con la versione di protocollo R2.5 nei metadati di schema.
    fn schema_v1(fields: Vec<Field>) -> SchemaRef {
        std::sync::Arc::new(Schema::new_with_metadata(
            fields,
            std::collections::HashMap::from([(
                PLENORA_CONTRACT_VERSION_KEY.to_owned(),
                "1".to_owned(),
            )]),
        ))
    }

    /// Lettura di contratto del campo + costruzione del contratto, come nel
    /// loop di discovery (la risoluzione CRS resta iniettata dai test).
    fn contract_from_field(field: &Field) -> Result<GeometryColumnContract, PlenoraError> {
        let keys = read_geometry_contract_keys(field)?;
        Ok(geometry_contract_from_field(
            field,
            ContractCrs::Resolved(projected_crs()),
            &keys,
        ))
    }

    #[test]
    fn discovery_reads_dimensions_and_encoding_from_metadata() {
        let contract = contract_from_field(&geometry_field(Some(
            r#"{"crs":"EPSG:32632","dimensions":"xyz","encoding":"ewkb"}"#,
        )))
        .expect("discovery");
        assert_eq!(contract.dimensions, GeometryDimensions::Xyz);
        assert_eq!(contract.encoding, Some(GeometryEncoding::Ewkb));

        // Forma scritta dai writer correnti (dimensions xy, niente encoding).
        let written = plenora_kernels_geo::arrow_adapter::geometry_output_field(
            "geometry",
            "EPSG:32632",
        )
        .expect("field");
        let contract = contract_from_field(&written).expect("discovery");
        assert_eq!(contract.dimensions, GeometryDimensions::Xy);
        // Milestone C (R2.7): il nome di estensione `geoarrow.wkb` dichiara
        // la famiglia WKB e completa l'encoding assente altrove (ultimo
        // rango della precedenza): non piu' `None`.
        assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
    }

    #[test]
    fn discovery_without_dimensions_metadata_propagates_unknown_never_xy() {
        // (b) R3.4: chiave `dimensions` assente -> Unknown propagato nel
        // contratto, MAI un default silenzioso Xy.
        let contract = contract_from_field(&geometry_field(Some(r#"{"crs":"EPSG:32632"}"#)))
            .expect("discovery");
        assert_eq!(contract.dimensions, GeometryDimensions::Unknown);
        // Come sopra: encoding completato dal nome di estensione (R2.7).
        assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
    }

    #[test]
    fn discovery_rejects_unreadable_dimensions_never_ignores_them() {
        // Milestone C (reader strict, R5.1): valore `dimensions` non canonico
        // o non testuale -> errore esplicito, mai ignorato ne' mappato a
        // Unknown. Comportamento piu' stretto della lettura lenient pre-C.
        for geo_json in [
            r#"{"crs":"EPSG:32632","dimensions":"2d"}"#,
            r#"{"crs":"EPSG:32632","dimensions":42}"#,
        ] {
            let result = read_geometry_contract_keys(&geometry_field(Some(geo_json)));
            assert!(result.is_err(), "geo: {geo_json}");
        }
    }

    #[test]
    fn discovery_rejects_unrepresentable_encoding() {
        // (d) R3.5: framing fuori dall'enum chiuso -> rifiuto esplicito
        // (Unsupported), mai mappato a un encoding noto.
        for geo_json in [
            r#"{"crs":"EPSG:32632","encoding":"gpkg"}"#,
            r#"{"crs":"EPSG:32632","encoding":"twkb"}"#,
            r#"{"crs":"EPSG:32632","encoding":42}"#,
        ] {
            let result = read_geometry_contract_keys(&geometry_field(Some(geo_json)));
            assert!(
                matches!(result, Err(PlenoraError::Unsupported(_))),
                "geo: {geo_json}"
            );
        }

        // Encoding rappresentabile -> propagato nel contratto.
        let contract = contract_from_field(&geometry_field(Some(
            r#"{"crs":"EPSG:32632","encoding":"wkb"}"#,
        )))
        .expect("discovery");
        assert_eq!(contract.encoding, Some(GeometryEncoding::Wkb));
    }

    #[test]
    fn discovery_legacy_field_leaves_types_undeclared() {
        // R3.4.1: ingresso legacy senza la coppia types_declaration/types ->
        // «proprieta' non dichiarata» (confidence Unknown), MAI unresolved.
        let contract = contract_from_field(&geometry_field(Some(r#"{"crs":"EPSG:32632"}"#)))
            .expect("discovery");
        assert!(contract.types.value().is_none());
    }

    #[test]
    fn discovery_recognizes_canonical_only_geometry_field() {
        // (a) tabella §2: le chiavi canoniche sono autosufficienti — il campo
        // e' riconosciuto come geometria anche senza estensione `geoarrow.wkb`
        // e metadato `geo`, con types Declared/Schema dalla coppia canonica.
        let schema = schema_v1(vec![
            Field::new("id", DataType::Int64, false),
            canonical_geometry_field(DataType::Binary),
        ]);
        let result = discover_input_contract_from_schema(schema);
        #[cfg(feature = "proj-backend")]
        {
            let contract = result.expect("discovery canonica");
            assert_eq!(contract.geometries.len(), 1);
            let geometry = &contract.geometries[0];
            assert_eq!(geometry.dimensions, GeometryDimensions::Xyz);
            assert_eq!(geometry.encoding, Some(GeometryEncoding::Wkb));
            assert!(
                matches!(geometry.types.confidence, PropertyConfidence::Declared(_)),
                "types dichiarati dalla coppia canonica"
            );
            assert_eq!(geometry.types.scope, PropertyScope::Schema);
            let types = geometry.types.value().expect("types");
            assert_eq!(types.declaration(), TypesDeclaration::Exact);
            assert_eq!(types.types(), &[GeometryType::Point]);
        }
        #[cfg(not(feature = "proj-backend"))]
        {
            // Senza backend PROJ la risoluzione CRS fallisce chiusa DOPO il
            // riconoscimento: un errore `Crs` (non `InvalidPlan`) dimostra che
            // il campo canonico-only e' stato riconosciuto come geometria e
            // le chiavi lette senza errori.
            assert!(
                matches!(result, Err(PlenoraError::Crs(_))),
                "atteso fallimento di risoluzione CRS, ottenuto {result:?}"
            );
        }
    }

    #[test]
    fn discovery_rejects_canonical_geometry_field_of_non_binary_type() {
        // (1c) chiavi canoniche coerenti ma tipo non Binary -> errore.
        let schema = schema_v1(vec![canonical_geometry_field(DataType::Utf8)]);
        let result = discover_input_contract_from_schema(schema);
        assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))));
    }

    #[test]
    fn discovery_rejects_contract_version_newer_than_supported() {
        // (b) R2.5: versione successiva a quella nota -> fallimento
        // esplicito (Unsupported), mai interpretazione parziale.
        let schema = std::sync::Arc::new(Schema::new_with_metadata(
            vec![Field::new("id", DataType::Int64, false)],
            std::collections::HashMap::from([(
                PLENORA_CONTRACT_VERSION_KEY.to_owned(),
                "2".to_owned(),
            )]),
        ));
        let result = discover_input_contract_from_schema(schema);
        assert!(matches!(result, Err(PlenoraError::Unsupported(_))));
    }

    #[test]
    fn discovery_rejects_canonical_keys_without_contract_version() {
        // R2.5: chiavi canoniche senza `plenora.contract.version` nei
        // metadati dello schema -> errore esplicito.
        let schema = std::sync::Arc::new(Schema::new(vec![canonical_geometry_field(
            DataType::Binary,
        )]));
        let result = discover_input_contract_from_schema(schema);
        assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))));
    }

    #[test]
    fn discovery_rejects_canonical_legacy_divergence() {
        // (c) R2.6: nozione divergente fra chiavi canoniche e metadato legacy
        // -> il componente fallisce, non sceglie.
        let field = geometry_field(Some(r#"{"crs":"EPSG:32632","dimensions":"xy"}"#));
        let mut metadata = field.metadata().clone();
        metadata.insert(PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned());
        let field = field.with_metadata(metadata);
        let result = discover_input_contract_from_schema(schema_v1(vec![field]));
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(message.contains("divergente"), "{message}");
            }
            other => panic!("attesa divergenza R2.6, ottenuto {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // R4.6.3 (contratti trasversali v2.0-rc9/rc10): la discovery non
    // pretende un CRS risolvibile — lo stato `missing` entra nel contratto.
    // -------------------------------------------------------------------

    #[test]
    fn discovery_geometry_without_crs_is_missing_not_an_error() {
        // Colonna GeoArrow-WKB senza metadato `geo` e senza chiavi
        // canoniche: nessun CRS dichiarato in alcuna rappresentazione
        // accettata -> `ContractCrs::Missing` (R4.4: mai un CRS inventato),
        // non un errore. La discovery non chiede la risoluzione, quindi il
        // test vale con e senza backend PROJ.
        let schema = std::sync::Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            geometry_field(None),
        ]));
        let contract = discover_input_contract_from_schema(schema).expect("discovery");
        assert_eq!(contract.geometries.len(), 1);
        assert!(
            matches!(contract.geometries[0].crs, ContractCrs::Missing),
            "CRS assente -> stato missing"
        );
    }

    #[test]
    fn discovery_geometry_with_geo_metadata_without_crs_is_missing() {
        // Metadato `geo` presente ma senza chiave `crs` (dimensions sola):
        // anche qui nessun CRS dichiarato -> `missing`, mai errore.
        let schema = std::sync::Arc::new(Schema::new(vec![geometry_field(Some(
            r#"{"dimensions":"xy"}"#,
        ))]));
        let contract = discover_input_contract_from_schema(schema).expect("discovery");
        assert!(matches!(contract.geometries[0].crs, ContractCrs::Missing));
        assert_eq!(contract.geometries[0].dimensions, GeometryDimensions::Xy);
    }

    #[test]
    fn discovery_canonical_missing_resolution_is_carried() {
        // `crs_resolution = missing` dichiarato canonicamente (senza chiavi
        // CRS, come impone la coerenza R2.2) -> stato missing nel contratto.
        let metadata = std::collections::HashMap::from([
            (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
            (
                PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                "missing".to_owned(),
            ),
        ]);
        let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        assert!(matches!(contract.geometries[0].crs, ContractCrs::Missing));
    }

    #[test]
    fn discovery_rejects_resolution_declaration_without_any_crs() {
        // R4.1: una dichiarazione `resolved` (o `declared_unresolved`) senza
        // alcuna rappresentazione CRS e' una contraddizione — MAI collassata
        // su `missing`: errore esplicito che nomina la chiave.
        for resolution in ["resolved", "declared_unresolved"] {
            let metadata = std::collections::HashMap::from([
                (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
                (
                    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                    resolution.to_owned(),
                ),
            ]);
            let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
            let result = discover_input_contract_from_schema(schema_v1(vec![field]));
            match result {
                Err(PlenoraError::InvalidPlan(message)) => {
                    assert!(
                        message.contains("nessun CRS e' dichiarato in alcuna rappresentazione"),
                        "{resolution}: {message}"
                    );
                }
                other => panic!("{resolution}: attesa contraddizione, ottenuto {other:?}"),
            }
        }
    }

    #[test]
    fn discovery_rejects_malformed_geo_metadata_never_treats_it_as_missing() {
        // R5.1: un metadato `geo` illeggibile non diventa «CRS assente» —
        // «illeggibile» non e' «assente»: errore, come prima di R4.6.3.
        let schema = std::sync::Arc::new(Schema::new(vec![geometry_field(Some("not json"))]));
        let result = discover_input_contract_from_schema(schema);
        assert!(result.is_err(), "metadato geo malformato -> errore");
    }

    // -------------------------------------------------------------------
    // R4.6.3 (BLOCK-08): `declared_unresolved` — preservato, mai risolto
    // in assenza di una decisione esplicita nel piano.
    // -------------------------------------------------------------------

    /// Campo geometria canonico con le chiavi date (helper delle fixture
    /// CRS: schema con versione R2.5, colonna `id` + `geometry`).
    fn canonical_crs_field(pairs: &[(&str, &str)]) -> Field {
        let mut metadata = std::collections::HashMap::from([
            (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
        ]);
        for (key, value) in pairs {
            metadata.insert((*key).to_owned(), (*value).to_owned());
        }
        Field::new("geometry", DataType::Binary, true).with_metadata(metadata)
    }

    #[test]
    fn discovery_declared_unresolved_is_preserved_never_auto_resolved() {
        // Cambio di comportamento dichiarato (R4.6.3): una dichiarazione
        // `declared_unresolved` con crs_id RISOLVIBILE (EPSG:32632) non e'
        // piu' risolta ed emessa come `resolved` — il centro preserva lo
        // stato dichiarato. Nessun backend coinvolto: il test vale con e
        // senza `proj-backend`.
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "declared_unresolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:32632"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
        ]);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        let ContractCrs::DeclaredUnresolved { crs_id, definition, .. } =
            &contract.geometries[0].crs
        else {
            panic!("atteso DeclaredUnresolved: {:?}", contract.geometries[0].crs);
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:32632"));
        assert_eq!(definition, &None);
        assert_eq!(
            contract.geometries[0].crs.resolution(),
            CrsResolution::DeclaredUnresolved
        );
    }

    #[test]
    fn discovery_conflicting_crs_id_and_srid_become_declared_unresolved() {
        // Il caso `conflicting_crs` del corpus di conformita':
        // crs_id=EPSG:4326 con srid=3003 (R4.3.1). Il centro PRESERVA
        // (expect_by_role: transformation_core = preserve): niente errore,
        // niente scelta silenziosa — lo stato diventa DeclaredUnresolved
        // con la dichiarazione originale, anche se il produttore dichiara
        // `resolved` (la contraddizione la dichiara il centro, R4.6.4).
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lon_lat"),
            (PLENORA_GEOMETRY_SRID_KEY, "3003"),
        ]);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        let ContractCrs::DeclaredUnresolved { crs_id, definition, .. } =
            &contract.geometries[0].crs
        else {
            panic!("atteso DeclaredUnresolved: {:?}", contract.geometries[0].crs);
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(definition, &None);
    }

    #[test]
    fn discovery_crs_id_and_definition_copresent_become_declared_unresolved() {
        // Due rappresentazioni risolvibili co-presenti: l'accordo non e'
        // decidibile testualmente (R2.7: mai arbitrato sul dato) — prima
        // vinceva `crs_definition` (scelta silenziosa), ora lo stato e'
        // DeclaredUnresolved con ENTRAMBE le dichiarazioni.
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, r#"{"type":"GeographicCRS"}"#),
            (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "projjson"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lat_lon"),
        ]);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format,
        } = &contract.geometries[0].crs
        else {
            panic!("atteso DeclaredUnresolved: {:?}", contract.geometries[0].crs);
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(
            definition.as_deref(),
            Some(r#"{"type":"GeographicCRS"}"#)
        );
        assert_eq!(
            definition_format.map(plenora_core::contract::CrsDefinitionFormat::as_str),
            Some("projjson")
        );
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn discovery_coherent_crs_id_and_srid_still_resolves() {
        // srid coerente con il codice dell'identificatore (come il caso
        // `multipolygon_xyzm_srid` del corpus): nessun conflitto, la
        // risoluzione avviene come sempre.
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:32632"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "easting_northing"),
            (PLENORA_GEOMETRY_SRID_KEY, "32632"),
        ]);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        assert!(
            matches!(contract.geometries[0].crs, ContractCrs::Resolved(_)),
            "srid coerente -> risoluzione"
        );
    }

    // -------------------------------------------------------------------
    // Decisione esplicita nel piano (R4.6.3, campo v4 `crs_decisions`).
    // -------------------------------------------------------------------

    /// Contratto di input con geometria nello stato dato, per i test di
    /// `apply_crs_decisions` (schema con le chiavi canoniche CRS dichiarate
    /// e metadato legacy `geo.crs`).
    fn contract_with_crs_state(crs: ContractCrs) -> DataContract {
        let mut metadata = std::collections::HashMap::from([
            (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
            (
                PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                crs.resolution().as_str().to_owned(),
            ),
            (
                GEO_METADATA_KEY.to_owned(),
                r#"{"crs":"EPSG:99999","encoding":"wkb"}"#.to_owned(),
            ),
        ]);
        if let ContractCrs::DeclaredUnresolved { crs_id: Some(id), .. } = &crs {
            metadata.insert(PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(), id.clone());
            metadata.insert(PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(), "unknown".to_owned());
            metadata.insert(PLENORA_GEOMETRY_SRID_KEY.to_owned(), "99999".to_owned());
        }
        let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);
        let geometry = GeometryColumnContract {
            field_id: FieldId(0),
            name: "geometry".to_owned(),
            crs,
            dimensions: GeometryDimensions::Xy,
            encoding: Some(GeometryEncoding::Wkb),
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        };
        DataContract::new(
            schema_v1(vec![field]),
            vec![geometry],
            Some(FieldId(0)),
            ContractProperties::default(),
        )
        .expect("contratto fixture valido")
    }

    fn declared_unresolved_state() -> ContractCrs {
        ContractCrs::DeclaredUnresolved {
            crs_id: Some("EPSG:99999".to_owned()),
            definition: None,
            definition_format: None,
        }
    }

    fn decisions_probe(definition: &str) -> PlanInputsProbe {
        PlanInputsProbe {
            inputs: vec!["main".to_owned()],
            crs_decisions: std::collections::BTreeMap::from([(
                "main".to_owned(),
                definition.to_owned(),
            )]),
        }
    }

    #[test]
    fn crs_decisions_on_unknown_input_is_an_error() {
        let mut contracts = vec![("main".to_owned(), contract_with_crs_state(
            declared_unresolved_state(),
        ))];
        let probe = PlanInputsProbe {
            inputs: vec!["main".to_owned()],
            crs_decisions: std::collections::BTreeMap::from([(
                "other".to_owned(),
                "EPSG:32632".to_owned(),
            )]),
        };
        let error = apply_crs_decisions(&probe, &mut contracts).expect_err("input ignoto");
        assert!(error.to_string().contains("crs_decisions"), "{error}");
    }

    #[test]
    fn crs_decisions_on_missing_or_resolved_state_is_an_error() {
        // Una decisione su `missing` inventerebbe un CRS per un ingresso che
        // non ne dichiara (R4.4); su `resolved` e' una contraddizione del
        // piano. Mai ignorata in silenzio: errore esplicito in entrambi i
        // casi, prima di toccare il backend (il test vale senza PROJ).
        for state in [
            ContractCrs::Missing,
            ContractCrs::Resolved(projected_crs()),
        ] {
            let mut contracts = vec![("main".to_owned(), contract_with_crs_state(state))];
            let error =
                apply_crs_decisions(&decisions_probe("EPSG:32632"), &mut contracts)
                    .expect_err("stato non decidibile");
            assert!(
                error.to_string().contains("non e' applicabile"),
                "{error}"
            );
        }
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn crs_decisions_resolve_declared_unresolved_keeping_the_schema_intact() {
        // La decisione esplicita del piano risolve l'incoerenza: il
        // contratto diventa `ResolvedByDecision` con la definizione decisa
        // (risolta contro PROJ). Lo schema di input NON e' toccato: il
        // check fail-closed dell'executor confronta i campi del file con il
        // contratto validato — la sostituzione delle dichiarazioni avviene
        // solo in emissione (strip nella fusione dello schema di output).
        let mut contracts = vec![("main".to_owned(), contract_with_crs_state(
            declared_unresolved_state(),
        ))];
        apply_crs_decisions(&decisions_probe("EPSG:32632"), &mut contracts)
            .expect("decisione");
        let contract = &contracts[0].1;
        let ContractCrs::ResolvedByDecision(crs) = &contract.geometries[0].crs else {
            panic!("atteso ResolvedByDecision: {:?}", contract.geometries[0].crs);
        };
        assert_eq!(crs.definition(), "EPSG:32632");
        assert_eq!(contract.geometries[0].crs.resolution(), CrsResolution::Resolved);
        let metadata = contract.schema.field_with_name("geometry").expect("campo").metadata();
        assert_eq!(
            metadata.get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY).map(String::as_str),
            Some("declared_unresolved"),
            "lo schema di input resta quello scoperto"
        );
    }

    // -------------------------------------------------------------------
    // Tagging di fase al confine di lettura (BLOCK-03, ADR-0009)
    // -------------------------------------------------------------------

    #[test]
    fn ipc_probes_tag_read_errors_at_the_input_boundary() {
        // File assente: Io dello sniffing -> fase Read; testo invariato.
        let missing = Path::new("input-che-non-esiste.arrow");
        let error = is_ipc_file_format(missing).expect_err("file assente");
        assert_eq!(error.phase(), ErrorPhase::Read);
        assert_eq!(error.phase_tag(), Some(ErrorPhase::Read));
        assert!(error.to_string().starts_with("io error: "), "{error}");
        // Stesso tag dal lato dell'header.
        let error = ipc_header_schema(missing).expect_err("file assente");
        assert_eq!(error.phase(), ErrorPhase::Read);

        // Header malformato: DataMapping nato leggendo la sorgente -> Read.
        let directory = tempfile::tempdir().expect("tempdir");
        let garbage = directory.path().join("garbage.arrow");
        std::fs::write(&garbage, b"non-e-un-flusso-ipc").expect("fixture");
        let error = ipc_header_schema(&garbage).expect_err("header malformato");
        assert_eq!(error.phase(), ErrorPhase::Read);
        assert!(matches!(error.untag(), PlenoraError::DataMapping(_)));
    }

    #[test]
    fn discovery_contract_errors_keep_the_derived_validate_phase() {
        // Regressione: gli errori della discovery del contratto (coerenza
        // dei metadati, R2.6) NON sono taggati — restano validazione
        // derivata per variante. Il tagging copre solo la lettura fisica.
        let field = geometry_field(Some(r#"{"crs":"EPSG:32632","dimensions":"xy"}"#));
        let mut metadata = field.metadata().clone();
        metadata.insert(PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xyz".to_owned());
        let field = field.with_metadata(metadata);
        let error = discover_input_contract_from_schema(schema_v1(vec![field]))
            .expect_err("divergenza R2.6");
        assert_eq!(error.phase(), ErrorPhase::Validate);
        assert_eq!(error.phase_tag(), None, "nessun tag: fase derivata");
    }

    // -------------------------------------------------------------------
    // Helper di presentazione e parsing argomenti
    // -------------------------------------------------------------------

    #[test]
    fn hex_digest_renders_every_byte_as_two_lowercase_hex_digits() {
        let mut digest = [0_u8; 32];
        digest[1] = 0x0f;
        digest[2] = 0xa5;
        digest[31] = 0xff;
        let hex = hex_digest(&digest);
        assert_eq!(hex.len(), 64);
        assert!(hex.starts_with("000fa5"), "{hex}");
        assert!(hex.ends_with("ff"), "{hex}");
        assert!(hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn argument_value_requires_flag_and_value() {
        let args: Vec<String> = ["transform", "--input", "in.bin", "--schema", "s.json"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(argument_value(&args, "--schema").expect("presente"), "s.json");
        let missing = argument_value(&args, "--output").expect_err("flag assente");
        assert!(missing.to_string().contains("--output"), "{missing}");
        let dangling: Vec<String> = vec!["--input".to_string()];
        let error = argument_value(&dangling, "--input").expect_err("valore mancante");
        assert!(error.to_string().contains("--input"), "{error}");
    }

    #[test]
    fn at_input_prefixes_context_and_preserves_the_variant() {
        let path = Path::new("dati.arrow");
        let prefixed = at_input("main", path, PlenoraError::InvalidPlan("boom".into()));
        match prefixed {
            PlenoraError::InvalidPlan(message) => {
                assert_eq!(message, "input `main` (dati.arrow): boom");
            }
            other => panic!("variante non preservata: {other:?}"),
        }
        for make in [
            PlenoraError::Unsupported as fn(String) -> PlenoraError,
            PlenoraError::Schema,
            PlenoraError::Crs,
        ] {
            let prefixed = at_input("main", path, make("boom".to_owned()));
            let message = match &prefixed {
                PlenoraError::Unsupported(message)
                | PlenoraError::Schema(message)
                | PlenoraError::Crs(message) => message,
                other => panic!("variante non preservata: {other:?}"),
            };
            assert_eq!(message, "input `main` (dati.arrow): boom");
        }
        // Le altre varianti passano inalterate (testo e tipo).
        let io = at_input("main", path, PlenoraError::Io(std::io::Error::other("disco")));
        assert!(matches!(io, PlenoraError::Io(_)));
        assert_eq!(io.to_string(), "io error: disco");
    }

    // -------------------------------------------------------------------
    // Definizione CRS dalle rappresentazioni accettate (R4.x)
    // -------------------------------------------------------------------

    #[test]
    fn crs_definition_from_metadata_accepts_objects_and_rejects_other_types() {
        // PROJJSON come oggetto: serializzato compatto, mai perso (chiavi in
        // ordine canonico: `serde_json::Value` le riordina alfabeticamente).
        let object = r#"{"crs":{"type":"GeographicCRS","name":"WGS 84"}}"#.to_owned();
        let definition =
            crs_definition_from_metadata("geometry", Some(&object)).expect("oggetto PROJJSON");
        assert_eq!(
            definition.as_deref(),
            Some(r#"{"name":"WGS 84","type":"GeographicCRS"}"#)
        );
        // Tipo non stringa/oggetto: metadato malformato -> errore (R5.1).
        let invalid = r#"{"crs":32632}"#.to_owned();
        let result = crs_definition_from_metadata("geometry", Some(&invalid));
        assert!(
            matches!(result, Err(PlenoraError::InvalidPlan(_))),
            "{result:?}"
        );
        // Senza chiave `crs` e senza metadato: assenza, non errore (R4.6.3).
        let bare = "{}".to_owned();
        assert_eq!(
            crs_definition_from_metadata("geometry", Some(&bare)).expect("nessun crs"),
            None
        );
        assert_eq!(
            crs_definition_from_metadata("geometry", None).expect("nessun metadato"),
            None
        );
    }

    #[test]
    fn contract_crs_from_keys_co_presence_is_declared_unresolved_not_a_choice() {
        // `crs_id` + `crs_definition` co-presenti: l'accordo non e'
        // decidibile testualmente (R2.7: mai arbitrato) — nessuna
        // precedenza silenziosa (prima vinceva `crs_definition`): lo stato
        // e' `DeclaredUnresolved` con entrambe le dichiarazioni.
        let keys = CanonicalGeometryKeys {
            crs_definition: Some(r#"{"type":"ProjectedCRS"}"#.to_owned()),
            crs_id: Some("EPSG:32632".to_owned()),
            ..CanonicalGeometryKeys::default()
        };
        let ContractCrs::DeclaredUnresolved { crs_id, definition, .. } =
            contract_crs_from_keys("geometry", None, &keys).expect("stato")
        else {
            panic!("atteso DeclaredUnresolved");
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:32632"));
        assert_eq!(definition.as_deref(), Some(r#"{"type":"ProjectedCRS"}"#));
    }

    #[test]
    fn contract_crs_from_keys_legacy_fallback_feeds_the_resolution() {
        // Nessuna forma canonica: il legacy `geo.crs` alimenta la
        // risoluzione (con backend -> Resolved; senza -> errore `Crs` di
        // backend, mai `Missing` inventato).
        let legacy = r#"{"crs":"EPSG:32632"}"#.to_owned();
        let keys = CanonicalGeometryKeys::default();
        let result = contract_crs_from_keys("geometry", Some(&legacy), &keys);
        #[cfg(feature = "proj-backend")]
        assert!(
            matches!(result, Ok(ContractCrs::Resolved(_))),
            "{result:?}"
        );
        #[cfg(not(feature = "proj-backend"))]
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "{result:?}"
        );
        // Nessuna rappresentazione: `Missing`, mai errore (R4.6.3).
        let missing = contract_crs_from_keys("geometry", None, &keys).expect("assente");
        assert!(matches!(missing, ContractCrs::Missing));
    }

    #[test]
    fn discovery_rejects_incoherent_geometry_metadata() {
        // Estensione diversa da `geoarrow.wkb`: rifiuto esplicito.
        let unknown_extension = Field::new("geometry", DataType::Binary, true).with_metadata(
            std::collections::HashMap::from([(
                GEOARROW_EXTENSION_KEY.to_owned(),
                "geoarrow.point".to_owned(),
            )]),
        );
        let result = discover_input_contract_from_schema(std::sync::Arc::new(Schema::new(
            vec![unknown_extension],
        )));
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(message.contains("non supportata"), "{message}");
            }
            other => panic!("atteso rifiuto estensione, ottenuto {other:?}"),
        }
        // Metadato `geo` senza estensione: metadati incoerenti.
        let orphan = Field::new("geometry", DataType::Binary, true).with_metadata(
            std::collections::HashMap::from([(GEO_METADATA_KEY.to_owned(), "{}".to_owned())]),
        );
        let result =
            discover_input_contract_from_schema(std::sync::Arc::new(Schema::new(vec![orphan])));
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(message.contains("incoerenti"), "{message}");
            }
            other => panic!("attesi metadati incoerenti, ottenuto {other:?}"),
        }
    }

    #[test]
    fn canonical_types_enter_the_contract_as_declared_with_schema_scope() {
        // Variante proj-indipendente del riconoscimento canonico: la coppia
        // types_declaration/types entra nel contratto come Declared/Schema.
        let contract =
            contract_from_field(&canonical_geometry_field(DataType::Binary)).expect("lettura");
        assert!(
            matches!(contract.types.confidence, PropertyConfidence::Declared(_)),
            "types dichiarati dalla coppia canonica"
        );
        assert_eq!(contract.types.scope, PropertyScope::Schema);
    }

    #[test]
    fn contract_json_renders_a_resolved_crs_with_kind_and_resolution() {
        let schema = std::sync::Arc::new(Schema::new(vec![Field::new(
            "geometry",
            DataType::Binary,
            true,
        )]));
        let contract = DataContract::new(
            schema,
            vec![GeometryColumnContract {
                field_id: FieldId(0),
                name: "geometry".to_owned(),
                crs: ContractCrs::Resolved(projected_crs()),
                dimensions: GeometryDimensions::Xy,
                encoding: None,
                nullable: true,
                types: GeometryColumnContract::undeclared_types(),
            }],
            Some(FieldId(0)),
            ContractProperties::default(),
        )
        .expect("contratto");
        let summary = contract_json(&contract);
        assert_eq!(summary["geometry"]["crs"], "EPSG:32632");
        assert_eq!(summary["geometry"]["crs_kind"], "Projected");
        assert_eq!(summary["geometry"]["crs_resolution"], "resolved");
    }

    #[test]
    fn contract_json_renders_a_missing_crs_as_null() {
        let schema = std::sync::Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geometry", DataType::Binary, true),
        ]));
        let contract = DataContract::new(
            schema,
            vec![GeometryColumnContract {
                field_id: FieldId(0),
                name: "geometry".to_owned(),
                crs: ContractCrs::Missing,
                dimensions: GeometryDimensions::Unknown,
                encoding: None,
                nullable: true,
                types: GeometryColumnContract::undeclared_types(),
            }],
            Some(FieldId(0)),
            ContractProperties::default(),
        )
        .expect("contratto");
        let summary = contract_json(&contract);
        assert_eq!(summary["geometry"]["crs"], serde_json::Value::Null);
        assert_eq!(summary["geometry"]["crs_kind"], serde_json::Value::Null);
        assert_eq!(summary["geometry"]["crs_resolution"], "missing");
        assert_eq!(summary["fields"][0]["name"], "id");
        assert_eq!(summary["fields"][0]["data_type"], "Int64");
        assert_eq!(summary["fields"][0]["nullable"], false);
        assert_eq!(summary["fields"][1]["nullable"], true);
    }

    // -------------------------------------------------------------------
    // Sniffing del framing IPC e input lazy
    // -------------------------------------------------------------------

    #[test]
    fn ipc_sniffing_treats_short_and_non_magic_files_as_streams() {
        let directory = tempfile::tempdir().expect("tempdir");
        // Piu' corto del magic: lettura parziale, nessun errore, non-file.
        let short = directory.path().join("short.bin");
        std::fs::write(&short, b"ARR").expect("fixture");
        assert!(!is_ipc_file_format(&short).expect("sniffing"));
        // Sei byte ma magic diverso: non e' IPC file format.
        let other = directory.path().join("other.bin");
        std::fs::write(&other, b"ARROW2").expect("fixture");
        assert!(!is_ipc_file_format(&other).expect("sniffing"));
    }

    #[test]
    fn open_input_accepts_file_and_stream_ipc_framings() {
        let directory = tempfile::tempdir().expect("tempdir");
        let schema: SchemaRef = std::sync::Arc::new(Schema::new(vec![Field::new(
            "id",
            DataType::Int64,
            false,
        )]));
        let batch = RecordBatch::try_new(
            std::sync::Arc::clone(&schema),
            vec![std::sync::Arc::new(
                plenora_core::arrow::array::Int64Array::from(vec![1, 2]),
            )],
        )
        .expect("batch");
        // IPC file format.
        let file_path = directory.path().join("in.arrow");
        let mut writer =
            FileWriter::try_new(File::create(&file_path).expect("create"), &schema).expect("writer");
        writer.write(&batch).expect("write");
        writer.finish().expect("finish");
        assert!(is_ipc_file_format(&file_path).expect("sniff"));
        // IPC stream format.
        let stream_path = directory.path().join("in.stream");
        let mut writer = plenora_core::arrow::ipc::writer::StreamWriter::try_new(
            File::create(&stream_path).expect("create"),
            &schema,
        )
        .expect("writer");
        writer.write(&batch).expect("write");
        writer.finish().expect("finish");
        assert!(!is_ipc_file_format(&stream_path).expect("sniff"));
        // Entrambi si aprono come input lazy con lo schema dichiarato.
        for path in [&file_path, &stream_path] {
            let input = open_input(path).expect("open_input");
            match input {
                Input::Stream { schema: declared, .. } => assert_eq!(declared, schema),
                Input::Batches(_) => panic!("gli input da percorso sono lazy"),
            }
        }
    }

    #[test]
    fn v4_input_paths_combines_single_and_multiple_flags() {
        let single: Vec<String> = ["run", "--input", "a.arrow"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            v4_input_paths(&single).expect("paths"),
            vec![PathBuf::from("a.arrow")]
        );
        let multiple: Vec<String> = ["run", "--inputs", "b.arrow", "c.arrow", "--output", "o.arrow"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            v4_input_paths(&multiple).expect("paths"),
            vec![PathBuf::from("b.arrow"), PathBuf::from("c.arrow")],
            "i valori si fermano al prossimo flag"
        );
        let both: Vec<String> = ["run", "--input", "a.arrow", "--inputs", "b.arrow"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(v4_input_paths(&both).expect("paths").len(), 2);
        let dangling: Vec<String> = vec!["--input".to_string()];
        assert!(v4_input_paths(&dangling).is_err());
    }

    // -------------------------------------------------------------------
    // Trasporto WKB v2: transform_stream e read_geometry_stream
    // (i comandi che li invocano richiedono la risoluzione CRS — feature
    // `proj-backend`; il framing e la trasformazione no)
    // -------------------------------------------------------------------

    /// POINT (2 3), little-endian OGC WKB.
    const POINT_WKB: [u8; 21] = [
        1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 8, 64,
    ];

    /// Scrive `frames` (con null) in framing WKB v2 e restituisce i byte.
    fn framed_v2(frames: &[Option<&[u8]>]) -> Vec<u8> {
        let mut writer = FrameWriter::new(Vec::new(), frames.len() as u64).expect("writer");
        for frame in frames {
            writer.write_frame(*frame).expect("frame");
        }
        writer.finish().expect("finish").0
    }

    fn centroid_schema(row_count: u64) -> TransformSchema {
        TransformSchema {
            schema_version: 2,
            operation: Operation::Centroid,
            row_count,
            crs: None,
        }
    }

    #[test]
    fn transform_stream_rechecks_the_schema_version_fail_closed() {
        // Difesa in profondita': `execute_transform` verifica la versione
        // prima della validazione CRS, ma il motore di stream non si fida
        // del chiamante.
        let input = framed_v2(&[Some(&POINT_WKB)]);
        let mut output = Vec::new();
        let mut schema = centroid_schema(1);
        schema.schema_version = 3;
        let result = transform_stream(&mut input.as_slice(), &mut output, &schema);
        let error = result.expect_err("versione non supportata");
        assert!(error.to_string().contains("schema_version"), "{error}");
    }

    #[test]
    fn transform_stream_transforms_frames_and_preserves_nulls() {
        let input = framed_v2(&[Some(&POINT_WKB), None, Some(&POINT_WKB)]);
        let mut output = Vec::new();
        let summary = transform_stream(&mut input.as_slice(), &mut output, &centroid_schema(3))
            .expect("transform");
        assert_eq!(summary.rows, 3);
        // Il checksum e' quello dello stream prodotto: rileggendolo i frame
        // sono coerenti (il reader verifica il footer).
        let mut reader = FrameReader::new(output.as_slice(), 3).expect("reader");
        let first = reader.next_frame().expect("frame").expect("riga 1");
        let Frame::Wkb(payload) = first else {
            panic!("atteso WKB");
        };
        // Il centroide di un punto e' il punto stesso.
        assert_eq!(payload.as_slice(), &POINT_WKB);
        assert!(
            matches!(reader.next_frame().expect("frame"), Some(Frame::Null)),
            "il null e' preservato in posizione"
        );
        assert!(reader.next_frame().expect("frame").is_some());
        assert!(reader.next_frame().expect("fine").is_none());
    }

    #[test]
    fn transform_stream_rejects_a_short_stream_against_row_count() {
        // Il flusso dichiara 3 righe ma ne arrivano 2: errore esplicito, mai
        // un output pubblicato come completo.
        let input = framed_v2(&[Some(&POINT_WKB), Some(&POINT_WKB)]);
        let mut output = Vec::new();
        let result = transform_stream(&mut input.as_slice(), &mut output, &centroid_schema(3));
        let error = result.expect_err("row_count non coerente");
        assert!(error.to_string().contains("row_count non coerente"), "{error}");
    }

    #[test]
    fn transform_stream_fails_closed_on_undecodable_wkb() {
        // Payload WKB corrotto: la trasformazione fallisce, nessun frame
        // inventato attraversa il confine.
        let garbage = [0x01, 0x01, 0x00, 0xFF];
        let input = framed_v2(&[Some(&garbage)]);
        let mut output = Vec::new();
        let result = transform_stream(&mut input.as_slice(), &mut output, &centroid_schema(1));
        assert!(result.is_err(), "WKB corrotto deve fallire");
    }

    #[test]
    fn transform_stream_flushes_full_batches_mid_stream() {
        // Oltre BATCH_ROWS (4096) righe: il flush intermedio e' esercitato e
        // il conteggio finale resta esatto.
        const ROWS: u64 = 4097;
        let frames: Vec<Option<&[u8]>> = (0..ROWS).map(|_| Some(&POINT_WKB[..])).collect();
        let input = framed_v2(&frames);
        let mut output = Vec::new();
        let summary = transform_stream(&mut input.as_slice(), &mut output, &centroid_schema(ROWS))
            .expect("transform");
        assert_eq!(summary.rows, ROWS);
        let mut reader = FrameReader::new(output.as_slice(), ROWS).expect("reader");
        let mut count = 0_u64;
        while reader.next_frame().expect("frame").is_some() {
            count += 1;
        }
        assert_eq!(count, ROWS);
    }

    #[test]
    fn read_geometry_stream_decodes_payloads_and_nulls() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("geometries.bin");
        std::fs::write(&path, framed_v2(&[Some(&POINT_WKB), None])).expect("fixture");
        let geometries = read_geometry_stream(&path, 2).expect("lettura");
        assert_eq!(geometries.len(), 2);
        match &geometries[0] {
            Some(geo::Geometry::Point(point)) => {
                assert!((point.x() - 2.0).abs() < f64::EPSILON);
                assert!((point.y() - 3.0).abs() < f64::EPSILON);
            }
            other => panic!("atteso Point, ottenuto {other:?}"),
        }
        assert!(geometries[1].is_none(), "null preservato");
        // Payload non decodificabile: errore, mai geometria inventata.
        let garbage = [0xDE, 0xAD];
        let bad_path = directory.path().join("garbage.bin");
        std::fs::write(&bad_path, framed_v2(&[Some(&garbage)])).expect("fixture");
        assert!(read_geometry_stream(&bad_path, 1).is_err());
    }
}
