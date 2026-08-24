//! I comandi ereditati dai due binari di origine.
//!
//! `run` sui piani `schema_version <= 3` (port da
//! `plenora-nogeo-tools`), il trasporto WKB v2 e v3 — `transform`,
//! `spatial-join`, `transform-arrow`, `pair-arrow` — (port dal livello
//! comandi di `plenora-geo-tools-arrow`), e `self-test`.
//!
//! # Perche' stanno insieme, e perche' qui
//!
//! Non sono un'accozzaglia: sono cio' che la fase 5 del refactor prevede di
//! ridurre a un **confine di migrazione** — i piani traducibili instradati nel
//! DAG v5, gli altri isolati — e poi rimuovere nella prossima major. Averli in
//! un modulo solo rende quel confine visibile: si vede che cosa dovra' sparire
//! e che cosa no, invece di doverlo dedurre.
//!
//! Il comportamento e' invariato rispetto ai sorgenti. Non e' codice da
//! migliorare: e' codice da tenere fermo finche' non lo si toglie.

use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::catalog::{find_operation, CrsRequirement, CATALOG};
use plenora_core::crs::{required_definition, validate_requirement};
use plenora_core::{ErrorPhase, PlenoraError};
use plenora_engine::geo_transport::pair_protocol::{write_pairs, MAX_PAIRS};
use plenora_engine::geo_transport::protocol::{Frame, FrameReader, FrameWriter};
use plenora_engine::geo_transport::publish::{
    publish_with_profile, validate_pair_arrow_crs, validate_transform_arrow_crs, PublishProfile,
};
use plenora_engine::geo_transport::transport::{
    pair_arrow_with_format, transform_arrow_with_format, ArrowOutputFormat, PairArrowSchema,
    PairArrowSummary, TransformArrowSchema, TransformArrowSummary,
};
use plenora_engine::table_engine::{execute_batch, execute_binary, Plan, ValidatedPlan};
use plenora_engine::{ipc_boundary, IpcFormat};
use plenora_kernels_geo::spatial_join::{spatial_join_nullable_validated, JoinPredicate};
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, Operation};
use rayon::prelude::*;
use serde::Deserialize;

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

use crate::{
    contract, durabilita_confermata, limite_risorsa, optional_value_after, read_control_json,
};

/// Materializza un input completo entro un budget di memoria RESIDUO,
/// restituendo il consumo che resta vivo dopo la concatenazione.
///
/// Il budget dev'essere globale, non per input: con la contabilita' per
/// singolo input i due lati di un piano binario potevano occupare ciascuno
/// l'intero `max_governed_memory_bytes`, cioe' il doppio del dichiarato. Il chiamante
/// scala il residuo e passa quello.
///
/// La CONCATENAZIONE finale duplica temporaneamente i dati — i batch di
/// partenza restano vivi finche' `concat_batches` non ha finito — quindi si
/// verifica il PICCO, non solo la somma dei batch accumulati.
/// Memoria che resta del budget dichiarato dal piano dopo `trattenuti` byte.
///
/// Fallisce CHIUSO, e la soglia e' lo ZERO, non il segno. La versione
/// precedente rifiutava solo la sottrazione negativa e restituiva `0` quando
/// il budget era esattamente esaurito: il passo successivo partiva comunque e
/// veniva fermato piu' tardi, da `with_memory_budget(0)`. Fra i due momenti
/// c'era spazio per allocare. Zero memoria residua e' gia' l'esaurimento:
/// l'errore va dato qui.
///
/// Il testo dice chi ha trattenuto la memoria, perche' e' l'informazione che
/// serve a chi deve alzare il budget.
fn residuo_di(budget: usize, trattenuti: usize, chi: &str) -> Result<usize, PlenoraError> {
    // `saturating_sub`: la sottrazione sotto zero e lo zero esatto sono lo
    // stesso caso — nessuna memoria residua — e vanno trattati insieme.
    let residuo = budget.saturating_sub(trattenuti);
    if residuo == 0 {
        return Err(PlenoraError::ResourceLimit(format!(
            "{chi} esaurisce il budget di memoria ({budget} byte): \
             {trattenuti} gia' trattenuti, nessuna memoria residua"
        )));
    }
    Ok(residuo)
}

/// Controllo di AMMISSIONE dell'output, **dopo** che il kernel l'ha
/// costruito.
///
/// Il nome dice cosa e' e cosa non e'. L'output e' memoria trattenuta:
/// finche' non e' pubblicato convive con gli input, quindi la somma
/// dev'essere dentro il budget dichiarato, e senza questo controllo il
/// caricamento era limitato e la produzione no. Ma il controllo avviene
/// **dopo l'allocazione**: se il kernel alloca oltre la memoria disponibile,
/// il processo esaurisce la memoria e questo errore non viene mai raggiunto.
///
/// Non e' quindi un tetto duro sulla memoria: e' un'ammissione a valle, che
/// impedisce di PUBBLICARE un risultato fuori budget e rende l'eccesso
/// diagnosticabile quando la macchina regge. Il rifiuto PREVENTIVO, dove
/// esiste, vive nei kernel (`preflight_output_bytes`) ed e' applicato alle
/// operazioni il cui numero di righe di output e' noto prima di allocare.
/// Le altre restano coperte solo da qui: residuo dichiarato in
/// errori-e-limiti.md#che-cosa-la-memoria-governata-non-garantisce.
fn ammissione_output(
    output: &RecordBatch,
    budget: usize,
    trattenuti: usize,
) -> Result<(), PlenoraError> {
    let prodotti = output.get_array_memory_size();
    let totale = trattenuti.checked_add(prodotti).ok_or_else(|| {
        PlenoraError::ResourceLimit(
            "overflow nel conteggio della memoria di input piu' output".into(),
        )
    })?;
    if totale > budget {
        return Err(PlenoraError::ResourceLimit(format!(
            "l'output supera il budget di memoria dichiarato ({budget} byte): \
             {trattenuti} trattenuti dagli input piu' {prodotti} prodotti"
        )));
    }
    Ok(())
}

fn load_complete_within(
    path: &Path,
    plan: &ValidatedPlan,
    budget: usize,
) -> Result<(RecordBatch, usize), PlenoraError> {
    // Ingresso non fidato: passa dal lettore di confine condiviso (framing e
    // limiti pre-validati, panico di arrow convertito in errore). I tetti
    // derivano dal budget residuo, non dai default del confine.
    let limits = plan.limits();
    let (schema, reader) = ipc_boundary::open_with_format(
        path,
        IpcFormat::File,
        &ipc_boundary::limits_from_memory_budget(budget),
    )?;
    let mut batches = Vec::new();
    let mut rows = 0_usize;
    let mut bytes = 0_usize;
    for batch in reader {
        let batch = batch?;
        rows = rows
            .checked_add(batch.num_rows())
            .ok_or_else(|| limite_risorsa("overflow conteggio righe"))?;
        if rows > limits.max_rows {
            // Limite di RISORSA: il piano e' corretto, sono i dati a non
            // entrare nel budget dichiarato (categoria `resource_limit`).
            return Err(PlenoraError::ResourceLimit(format!(
                "file con oltre {} righe",
                limits.max_rows
            ))
            .with_phase(ErrorPhase::Read));
        }
        // Questo percorso MATERIALIZZA l'intero input (i piani blocking e
        // binari lo richiedono): il budget di memoria va quindi verificato
        // mentre si accumula, non dopo. Prima nessuno lo guardava e un file
        // grande a piacere veniva concatenato in memoria fino all'esaurimento
        // della macchina.
        bytes = bytes
            .checked_add(batch.get_array_memory_size())
            .ok_or_else(|| limite_risorsa("overflow nel conteggio dei byte"))?;
        // Il picco include la copia che `concat_batches` produce: i batch di
        // partenza restano vivi mentre la concatenazione alloca il risultato.
        //
        // Aritmetica CONTROLLATA, non saturante: `saturating_mul` a fondo
        // scala restituisce `usize::MAX`, e se il budget fosse anch'esso a
        // fondo scala il confronto `picco > budget` sarebbe falso proprio
        // quando la stima non e' piu' misurabile. Un numero che ha perso il
        // conto non puo' autorizzare nulla: il traboccamento e' esso stesso
        // il superamento del budget.
        let Some(picco) = bytes.checked_mul(2) else {
            return Err(PlenoraError::ResourceLimit(format!(
                "stima del picco di memoria non piu' rappresentabile a {bytes} byte \
                 accumulati: budget ({budget} byte) considerato superato"
            ))
            .with_phase(ErrorPhase::Read));
        };
        if picco > budget {
            return Err(PlenoraError::ResourceLimit(format!(
                "l'input materializzato supera il budget di memoria residuo \
                 ({budget} byte): {bytes} accumulati, picco stimato {picco} \
                 con la concatenazione"
            ))
            .with_phase(ErrorPhase::Read));
        }
        batches.push(batch);
    }
    if batches.is_empty() {
        return Ok((RecordBatch::new_empty(schema), 0));
    }
    let unito = concat_batches(&schema, &batches)?;
    // Consumo dichiarato al chiamante: cio' che resta vivo dopo la
    // concatenazione, cioe' il batch unito.
    let consumo = unito.get_array_memory_size();
    Ok((unito, consumo))
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

pub fn run_pipeline(
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
    let plan: Plan = read_control_json(plan_path)?;
    let plan = plan.validate()?;
    if plan.requires_secondary() || plan.requires_blocking() {
        // Contabilita' GLOBALE del budget, non per input: il secondo lato
        // riceve cio' che resta dopo il primo. Chiamando due volte
        // `load_complete` ciascun lato riceveva l'intero `max_governed_memory_bytes`,
        // cioe' il doppio del dichiarato.
        //
        // ATTENZIONE a cosa questo garantisce. Il CARICAMENTO e' limitato
        // davvero: i batch si contano mentre si accumulano e si smette prima
        // di superare il budget. L'ESECUZIONE no: i kernel ricevono il
        // budget residuo, e alcuni lo usano per rifiutare in anticipo
        // (`preflight_output_bytes`), ma gli altri costruiscono l'output e
        // solo dopo lo si ammette o lo si rifiuta. Su quelli il budget e' un
        // controllo di ammissione a valle, non un tetto duro: vedi errori-e-limiti.md#che-cosa-la-memoria-governata-non-garantisce.
        // Non chiamarlo «budget globale» senza questa distinzione.
        let budget = plan.limits().max_governed_memory_bytes;
        let (left, usati) = load_complete_within(input_path, &plan, budget)?;
        let output = if plan.requires_secondary() {
            let right_path = right_path.ok_or_else(|| contract("il piano richiede --right"))?;
            let residuo = residuo_di(budget, usati, "il primo input")?;
            let (right, usati_destra) = load_complete_within(right_path, &plan, residuo)?;
            // Durante l'esecuzione ENTRAMBI gli input restano vivi: sono
            // prestati al kernel, non consumati. Il budget dell'esecuzione e'
            // quindi cio' che resta dopo averli trattenuti tutti e due, e i
            // kernel devono vedere quel numero — non il budget iniziale —
            // perche' e' con `limits.max_governed_memory_bytes` che dimensionano le
            // proprie tabelle di lavoro.
            let trattenuti = usati.checked_add(usati_destra).ok_or_else(|| {
                PlenoraError::ResourceLimit(
                    "overflow nel conteggio della memoria trattenuta dagli input".into(),
                )
            })?;
            let esecuzione =
                plan.with_memory_budget(residuo_di(budget, trattenuti, "i due input insieme")?)?;
            let output = execute_binary(&left, &right, &esecuzione)?;
            ammissione_output(&output, budget, trattenuti)?;
            output
        } else {
            // Qui l'input e' CONSUMATO dal kernel, ma resta vivo come batch
            // di lavoro per tutta la catena: va addebitato lo stesso.
            let esecuzione = plan.with_memory_budget(residuo_di(budget, usati, "l'input")?)?;
            let output = plenora_engine::execute_complete_batch(left, &esecuzione)?;
            ammissione_output(&output, budget, usati)?;
            output
        };
        return publish_one(output_path, &output);
    }
    // Percorso streaming: un batch alla volta, ma il tetto del confine deve
    // comunque derivare dal budget del piano — e' quello che impedisce ad
    // arrow di allocare un messaggio piu' grande del budget prima ancora che
    // il batch esista.
    let (input_schema, reader) = ipc_boundary::open_with_format(
        input_path,
        IpcFormat::File,
        &ipc_boundary::limits_from_memory_budget(plan.limits().max_governed_memory_bytes),
    )?;

    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer: Option<FileWriter<&mut File>> = None;

    let mut wrote_batch = false;
    let mut total_rows = 0_usize;
    for input in reader {
        let input = input?;
        total_rows = total_rows
            .checked_add(input.num_rows())
            .ok_or_else(|| limite_risorsa("overflow nel conteggio complessivo delle righe"))?;
        if total_rows > plan.limits().max_rows {
            return Err(PlenoraError::ResourceLimit(format!(
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
pub struct TransformSchema {
    pub schema_version: u32,
    pub operation: Operation,
    pub row_count: u64,
    pub crs: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpatialJoinSchema {
    pub schema_version: u32,
    pub predicate: JoinPredicate,
    pub left_row_count: u64,
    pub right_row_count: u64,
    pub max_pairs: u64,
    pub left_crs: Option<String>,
    pub right_crs: Option<String>,
}

#[derive(Debug)]
pub struct TransformSummary {
    pub rows: u64,
    pub checksum: [u8; 32],
}

#[derive(Debug)]
pub struct SpatialJoinSummary {
    pub pairs: u64,
    pub checksum: [u8; 32],
}

pub fn transform_stream(
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

pub fn execute_transform(
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
    let schema: TransformSchema = read_control_json(schema_path)?;
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
    let (result, outcome) =
        publish_with_profile(output_path, PublishProfile::Atomic, |output_writer| {
            transform_stream(&mut input_reader, output_writer, &schema)
                .map_err(|error| contract(error.to_string()))
        })?;
    let _ = durabilita_confermata(outcome);
    Ok(result)
}

pub fn execute_transform_arrow(
    input: &str,
    schema_path: &Path,
    output: &str,
    output_format: ArrowOutputFormat,
) -> Result<TransformArrowSummary, Box<dyn Error>> {
    if output == "-" {
        return Err(contract(
            "output stdout disabilitato: la pubblicazione deve essere transazionale",
        )
        .into());
    }
    let schema: TransformArrowSchema = read_control_json(schema_path)?;
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
    let (summary, outcome) =
        publish_with_profile(output_path, PublishProfile::Atomic, |output_writer| {
            transform_arrow_with_format(&mut input_reader, output_writer, &schema, output_format)
                .map_err(|error| {
                    // Un rifiuto row-scoped (R9.9) e' un difetto del DATO letto,
                    // non del piano: assi data_mapping/read e diagnostica
                    // preservata, mai riclassificato invalid_plan/validate.
                    // Gli errori non row-scoped mantengono la classificazione
                    // storica `contract` (unico percorso del trasporto legacy che
                    // produce diagnostica: `transform_arrow`; `pair_arrow` e il
                    // v2 a frame WKB non ne emettono).
                    error.row_diagnostics().map_or_else(
                        || contract(error.to_string()),
                        |diagnostics| {
                            PlenoraError::DataMapping(error.to_string())
                                .with_phase(ErrorPhase::Read)
                                .with_row_diagnostics(diagnostics.clone())
                        },
                    )
                })
        })?;
    let _ = durabilita_confermata(outcome);
    Ok(summary)
}

pub fn execute_pair_arrow(
    left_path: &Path,
    right_path: &Path,
    schema_path: &Path,
    output_path: &Path,
    output_format: ArrowOutputFormat,
) -> Result<PairArrowSummary, Box<dyn Error>> {
    let schema: PairArrowSchema = read_control_json(schema_path)?;
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
    let (summary, outcome) =
        publish_with_profile(output_path, PublishProfile::Atomic, |output_writer| {
            pair_arrow_with_format(
                &mut left_reader,
                &mut right_reader,
                output_writer,
                &schema,
                output_format,
            )
            .map_err(|error| contract(error.to_string()))
        })?;
    let _ = durabilita_confermata(outcome);
    Ok(summary)
}

pub fn read_geometry_stream(
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

pub fn execute_spatial_join(
    left_path: &Path,
    right_path: &Path,
    schema_path: &Path,
    output_path: &Path,
) -> Result<SpatialJoinSummary, Box<dyn Error>> {
    const MAX_JOIN_ROWS_PER_SIDE: u64 = 2_000_000;
    const MAX_JOIN_INPUT_BYTES: u64 = 1024 * 1024 * 1024;

    let schema: SpatialJoinSchema = read_control_json(schema_path)?;
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
        return Err(limite_risorsa(format!(
            "spatial-join oltre il limite di {MAX_JOIN_ROWS_PER_SIDE} righe per lato"
        ))
        .into());
    }
    for path in [left_path, right_path] {
        let bytes = path.metadata()?.len();
        if bytes > MAX_JOIN_INPUT_BYTES {
            return Err(limite_risorsa(format!(
                "input spatial-join {} oltre il limite di {MAX_JOIN_INPUT_BYTES} byte",
                path.display()
            ))
            .into());
        }
    }

    // Entrambi gli input con checksum sono verificati per intero prima del
    // calcolo. `read_geometry_stream` decodifica con `geometry_from_wkb`
    // (validazione OGC per geometria): precondizione dimostrata per
    // costruzione della variante `*_validated` del join (R0.1).
    let left = read_geometry_stream(left_path, schema.left_row_count)?;
    let right = read_geometry_stream(right_path, schema.right_row_count)?;
    let pairs = spatial_join_nullable_validated(&left, &right, schema.predicate, schema.max_pairs)?;
    let pair_count =
        u64::try_from(pairs.len()).map_err(|_| contract("pair_count non rappresentabile"))?;
    let (checksum, outcome) =
        publish_with_profile(output_path, PublishProfile::Atomic, |writer| {
            let (_, checksum) =
                write_pairs(writer, &pairs).map_err(|error| contract(error.to_string()))?;
            Ok(checksum)
        })?;
    let _ = durabilita_confermata(outcome);
    Ok(SpatialJoinSummary {
        pairs: pair_count,
        checksum,
    })
}

pub fn write_self_test(path: &Path) -> Result<(), Box<dyn Error>> {
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

// Quoting Debug intenzionale nel ramo geo: produce la stringa JSON del
// percorso (virgolette ed escape); il `.display()` suggerito da clippy
// cambierebbe l'output del comando (contratto CLI).
#[allow(clippy::unnecessary_debug_formatting)]
pub fn self_test_command(args: &[String]) -> Result<(), Box<dyn Error>> {
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
