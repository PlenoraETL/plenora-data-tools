//! plenora-data-tools CLI — Fase 1 "coesistenza" (architettura.md).
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
//! - Fase 2A: collegamento al DAG. Se il piano dichiara `schema_version: 5`
//!   — o `4`, che viene migrato al canonico prima di ogni altra cosa (piano-v5.md,
//!   migrazione) — `validate` e `run` usano il planner/executor del DAG
//!   (`plenora_engine::planner::validate` + `plenora_engine::execute`); i piani
//!   legacy (`schema_version` <= 3) restano sul `table_engine`, comportamento
//!   invariato. Dettagli nella sezione "DAG (Fase 2A)" piu' sotto.
//!
//! Fail-closed come nei sorgenti: nessun output parziale, publish atomico su
//! tempfile + `persist_noclobber`, exit code 2 su qualunque errore, messaggi
//! senza dati sensibili. Fase 2B, cancellazione cooperativa
//! (errori-e-limiti.md#cancellazione): `run` installa un handler
//! Ctrl-C che cancella cooperativamente l'esecuzione DAG tramite
//! `CancellationToken` — al cancel nessun output e' pubblicato, messaggio
//! pulito ed exit code dedicato 130 (128 + SIGINT); un secondo Ctrl-C forza
//! l'uscita immediata.

use std::borrow::Cow;
use std::env;
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, SchemaRef};
use plenora_core::arrow::select::concat::concat_batches;
use plenora_core::catalog::{find_operation, CrsRequirement, Family, OperationDescriptor, CATALOG};
use plenora_core::contract::{
    ContractCrs, ContractProperties, ContractProperty, CrsDefinitionFormat, CrsResolution,
    DataContract, FieldId, GeometryColumnContract, GeometryDimensions, GeometryEncoding,
    GeometryTypesProperty, PropertyConfidence, PropertyScope,
};
use plenora_core::crs::{required_definition, validate_requirement, ResolvedCrs};
use plenora_core::limits::PlanLimits;
use plenora_core::{ErrorPhase, PlenoraError, RetryDisposition};
use plenora_engine::geo_transport::pair_protocol::{write_pairs, MAX_PAIRS};
use plenora_engine::geo_transport::protocol::{Frame, FrameReader, FrameWriter};
use plenora_engine::geo_transport::publish::{
    publish_with_profile, validate_pair_arrow_crs, validate_transform_arrow_crs, PublishOutcome,
    PublishProfile,
};
use plenora_engine::geo_transport::transport::{
    pair_arrow_with_format, transform_arrow_with_format, ArrowOutputFormat, ArrowTransportError,
    PairArrowSchema, PairArrowSummary, TransformArrowSchema, TransformArrowSummary,
};
use plenora_engine::plan::{migrazione_v4, PLAN_SCHEMA_VERSION_V4, PLAN_SCHEMA_VERSION_V5};
use plenora_engine::planner::{self, ValidatedGraph};
use plenora_engine::table_engine::{execute_batch, execute_binary, Plan, ValidatedPlan};
use plenora_engine::{
    execute, explain, ipc_boundary, parallelism, CancellationToken, ExecutionMetrics,
    ExecutionPlan, Input, Inputs, IpcFormat, IpcLimits, RuntimeContext,
};
use plenora_kernels_geo::arrow_adapter::{
    read_contract_version, read_geometry_contract_keys, CanonicalGeometryKeys,
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
};
use plenora_kernels_geo::spatial_join::{spatial_join_nullable_validated, JoinPredicate};
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

/// Costruttore esplicito dei limiti di RISORSA della CLI.
///
/// Esiste per la stessa ragione per cui esiste `contract`: rendere la
/// categoria visibile nel punto d'uso. Prima i tetti e i traboccamenti della
/// CLI passavano da `contract`, cioe' uscivano come `invalid_plan` — e il
/// censimento della classe, che cercava le occorrenze di
/// `PlenoraError::InvalidPlan`, non li vedeva nemmeno: erano nascosti dietro
/// un helper. Due costruttori distinti rendono la scelta leggibile a chi
/// scrive e cercabile a chi verifica.
fn limite_risorsa(message: impl Into<String>) -> PlenoraError {
    PlenoraError::ResourceLimit(message.into())
}

/// Exit code dedicato alla cancellazione (errori-e-limiti.md#cancellazione): 128 + SIGINT,
/// convenzione POSIX — distinto dagli altri codici di errore.
const EXIT_CANCELLED: i32 = 130;

/// Formato dell'output dei comandi, scelto dal flag globale `--format`.
///
/// Stessa convenzione di `plenora-database-tools`: il flag e' globale, viene
/// tolto dagli argomenti PRIMA del dispatch e vale per il comando che segue.
/// `junit` non c'e': un formato senza un consumatore e' codice non provato, e
/// qui nessun gate lo legge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    /// JSON: il default, ed e' cio' che uno script deve poter assumere.
    Json,
    /// Markdown: leggibile da una persona, per i comandi che descrivono.
    Markdown,
}

static ACTIVE_FORMAT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

impl OutputFormat {
    fn set_active(self) {
        let value = match self {
            Self::Json => 0,
            Self::Markdown => 1,
        };
        ACTIVE_FORMAT.store(value, std::sync::atomic::Ordering::Relaxed);
    }

    fn active() -> Self {
        if ACTIVE_FORMAT.load(std::sync::atomic::Ordering::Relaxed) == 1 {
            Self::Markdown
        } else {
            Self::Json
        }
    }

    /// Esige il formato JSON: i comandi che non hanno una resa leggibile
    /// rifiutano `--format markdown` invece di ignorarlo. Un flag accettato e
    /// disatteso e' peggio di un flag rifiutato.
    fn require_json(comando: &str) -> Result<(), PlenoraError> {
        if Self::active() == Self::Markdown {
            return Err(contract(format!(
                "`--format markdown` non e' disponibile per `{comando}`: \
                 formati supportati `json`"
            )));
        }
        Ok(())
    }
}

/// Toglie `--format VALORE` dagli argomenti e lo registra come formato
/// attivo. Il flag e' globale: puo' precedere o seguire il sottocomando.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se il valore manca o non e' riconosciuto.
fn strip_output_format(args: Vec<String>) -> Result<Vec<String>, PlenoraError> {
    let mut rimanenti = Vec::with_capacity(args.len());
    let mut visto = false;
    let mut iteratore = args.into_iter();
    while let Some(argument) = iteratore.next() {
        if argument != "--format" {
            rimanenti.push(argument);
            continue;
        }
        let valore = iteratore
            .next()
            .ok_or_else(|| contract("valore mancante per --format (json|markdown)"))?;
        if visto {
            // Due `--format` con valori diversi sono due richieste diverse:
            // farne vincere una in silenzio significa eseguire quella che
            // l'utente non ha scritto.
            return Err(contract(
                "flag `--format` ripetuto: se ne accetta una sola occorrenza",
            ));
        }
        visto = true;
        match valore.as_str() {
            "json" => OutputFormat::Json.set_active(),
            "markdown" => OutputFormat::Markdown.set_active(),
            altro => {
                return Err(contract(format!(
                    "formato `{altro}` non riconosciuto: attesi `json` o `markdown`"
                )));
            }
        }
    }
    Ok(rimanenti)
}

/// Handler Ctrl-C (errori-e-limiti.md#cancellazione): il primo Ctrl-C cancella il token —
/// l'executor si ferma al prossimo confine cooperativo con
/// `PlenoraError::Cancelled` e la CLI esce con [`EXIT_CANCELLED`] senza
/// pubblicare nulla; il secondo forza l'uscita immediata (comportamento
/// accettato e documentato in errori-e-limiti.md: un kernel `NonInterruptible` in corso
/// non offre altri punti di interruzione).
///
/// `ctrlc::set_handler` e' installabile una sola volta per processo: la CLI
/// esegue un comando per processo, quindi un fallimento e' un errore vero
/// (fail-closed).
fn install_ctrlc_handler(token: &CancellationToken) -> Result<(), PlenoraError> {
    let token = token.clone();
    let requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    ctrlc::set_handler(move || {
        // Gli avvisi sono per una PERSONA davanti a un terminale. Se stderr
        // non e' un terminale c'e' un programma dall'altro lato, e per lui il
        // canale resta vuoto: l'esito della cancellazione arriva comunque
        // come envelope su stdout con categoria `cancelled` ed exit 130.
        // La garanzia «stderr vuoto» (errori-e-limiti.md#envelope-e-canali)
        // vale quindi senza eccezioni
        // per ogni consumatore non interattivo.
        let interattivo = std::io::IsTerminal::is_terminal(&std::io::stderr());
        if requested.swap(true, std::sync::atomic::Ordering::SeqCst) {
            if interattivo {
                eprintln!("plenora-data-tools: secondo ctrl-c: uscita forzata");
            }
            std::process::exit(EXIT_CANCELLED);
        }
        if interattivo {
            eprintln!(
                "plenora-data-tools: ctrl-c: annullamento in corso (un secondo ctrl-c forza l'uscita)..."
            );
        }
        token.cancel();
    })
    .map_err(|error| contract(format!("handler ctrl-c non installabile: {error}")))
}

/// Esito tipizzato del publish (errori-e-limiti.md#publish-e-cleanup) in forma verificabile, **senza
/// scrivere su stderr**.
///
/// Era un avviso su stderr: invisibile a un consumatore automatico e insieme
/// una crepa nel contratto «stderr vuoto»
/// (errori-e-limiti.md#envelope-e-canali). Ora il chiamante lo
/// riporta nel proprio documento di uscita, dove chi legge le metriche lo
/// trova senza intercettare un canale che per contratto non porta nulla.
///
/// Con il profilo `Atomic` l'esito e' sempre `Published`; il ramo non
/// confermato serve ai chiamanti che useranno `DurableAtomic`.
const fn durabilita_confermata(outcome: PublishOutcome) -> bool {
    !matches!(outcome, PublishOutcome::PublishedButDurabilityUnconfirmed)
}

/// Digest esadecimale minuscolo, senza primitive di panic (gate R6).
///
/// La formattazione su `String` non puo' fallire, ma `write!` restituisce
/// comunque un `Result` che andrebbe scartato con `expect`. La tabella dei
/// nibble e' indicizzata da un valore provabilmente in `0..16` (shift e
/// maschera su `u8`): esatta per costruzione, nessun `Result` da gestire.
fn hex_digest(digest: &[u8; 32]) -> String {
    const NIBBLE: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    let mut output = String::with_capacity(64);
    for &byte in digest {
        output.push(NIBBLE[usize::from(byte >> 4)]);
        output.push(NIBBLE[usize::from(byte & 0x0f)]);
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

/// Formato dei due output Arrow legacy. L'assenza del flag conserva
/// l'envelope PLNGEO3 storico; qualunque valore non dichiarato fallisce
/// prima di aprire il percorso di pubblicazione.
fn arrow_output_format(args: &[String]) -> Result<ArrowOutputFormat, PlenoraError> {
    let Some(position) = args.iter().position(|value| value == "--output-format") else {
        return Ok(ArrowOutputFormat::PlnGeo3);
    };
    match args.get(position + 1).map(String::as_str) {
        Some("plngeo3") => Ok(ArrowOutputFormat::PlnGeo3),
        Some("ipc-file") => Ok(ArrowOutputFormat::IpcFile),
        Some(_) => Err(contract(
            "--output-format non valido (ammessi: plngeo3, ipc-file)",
        )),
        None => Err(contract("valore mancante per --output-format")),
    }
}

// ---------------------------------------------------------------------------
// Pipeline tabellare (port da plenora-nogeo-tools/src/main.rs)
// ---------------------------------------------------------------------------

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

fn execute_transform_arrow(
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

fn execute_pair_arrow(
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

/// Identita' del binario in forma leggibile da un programma: versione del
/// componente, versione Arrow, backend compilati.
///
/// I backend derivano dalle feature con cui QUESTO binario e' stato
/// compilato, non da una lista scritta a mano: e' l'unica risposta che non
/// puo' mentire.
fn version_json() -> serde_json::Value {
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
fn backends_compilati() -> Vec<&'static str> {
    let mut backends = Vec::new();
    if cfg!(feature = "geos-backend") {
        backends.push("geos");
    }
    if cfg!(feature = "proj-backend") {
        backends.push("proj");
    }
    backends
}

/// `capabilities`: il documento dichiarativo di `plenora-core` piu'
/// l'identita' di questo binario (versione e backend), che il documento non
/// puo' conoscere.
fn capabilities_command() -> Result<(), Box<dyn Error>> {
    // ICD §10 R10.2: capability dichiarative interrogabili prima
    // dell'esecuzione, in forma leggibile da un programma.
    let documento = plenora_core::capabilities::component_capabilities();
    let mut valore = serde_json::to_value(&documento)?;
    if let Some(oggetto) = valore.as_object_mut() {
        oggetto.insert(
            "component_version".to_owned(),
            serde_json::Value::String(env!("CARGO_PKG_VERSION").to_owned()),
        );
        oggetto.insert(
            "backends".to_owned(),
            serde_json::json!(backends_compilati()),
        );
    }
    match OutputFormat::active() {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&valore)?),
        OutputFormat::Markdown => {
            println!("# Capability di plenora-data-tools\n");
            println!("| | |");
            println!("|---|---|");
            println!("| versione | {} |", env!("CARGO_PKG_VERSION"));
            println!("| Arrow | {} |", documento.arrow_version);
            println!(
                "| backend | {} |",
                if backends_compilati().is_empty() {
                    "nessuno".to_owned()
                } else {
                    backends_compilati().join(", ")
                }
            );
            println!("| operazioni a catalogo | {} |", CATALOG.len());
        }
    }
    Ok(())
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
            parsed
                .ok_or_else(|| contract("famiglia sconosciuta: attesa `table` o `geo`".to_owned()))
        })
        .transpose()?;
    let descrittori: Vec<&OperationDescriptor> = CATALOG
        .iter()
        .filter(|descriptor| family.is_none_or(|wanted| descriptor.family == wanted))
        .collect();
    match OutputFormat::active() {
        OutputFormat::Json => {
            let entries: Vec<serde_json::Value> =
                descrittori.iter().copied().map(descriptor_json).collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
        }
        OutputFormat::Markdown => {
            println!("| operazione | famiglia | arieta' | maturita' |");
            println!("|---|---|---|---|");
            for descriptor in descrittori {
                println!(
                    "| `{}` | {:?} | {:?} | {:?} |",
                    descriptor.id, descriptor.family, descriptor.arity, descriptor.maturity
                );
            }
        }
    }
    Ok(())
}

fn validate_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    OutputFormat::require_json("validate")?;
    let plan_path = value_after(args, "--plan")?;
    // Stesso parser di `run`: le due forme — `--input nome=percorso` e
    // `--inputs` posizionale — devono comportarsi allo stesso modo nei due
    // comandi, altrimenti si valida un accoppiamento e se ne esegue un altro.
    let inputs = v4_inputs(args)?;
    let plan_text = read_control_plan_text(Path::new(&plan_path))?;
    if let Some(plan_text) = testo_piano_dag(&plan_text)? {
        return validate_dag_v4(
            plan_text.as_ref(),
            &inputs,
            !has_flag(args, "--no-geo-fusion"),
        );
    }
    let inputs: Vec<PathBuf> = match inputs {
        V4Inputs::Positional(paths) => paths,
        // I piani legacy non hanno input nominati: il riepilogo elenca i soli
        // percorsi, e accettare una forma che non sanno usare confonderebbe.
        V4Inputs::Named(_) => {
            return Err(contract(
                "`--input nome=percorso` richiede un piano DAG; \
                 per i piani legacy usare `--input PERCORSO`",
            )
            .into());
        }
    };
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
// DAG (Fase 2A): scoperta dei contratti, validate e run
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
// - **validate**: `planner::validate` (fase 1,
//   piano-v5.md#identita-e-fingerprint,
//   architettura.md#planner-ed-executor) e poi `explain` con
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

/// Sonda del solo `schema_version`: decide il percorso (DAG vs legacy).
#[derive(Debug, Deserialize)]
struct PlanVersionProbe {
    schema_version: u32,
}

/// Sonda dei nomi di input dichiarati dal piano DAG (accoppiamento posizionale
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

/// Porta il testo del piano al canonico v5 se il piano dichiara un formato
/// DAG; `None` se dichiara la forma lineare legacy (`schema_version <= 3`),
/// che prosegue sul percorso invariato.
///
/// La CLI sonda il piano piu' volte (input dichiarati, decisioni CRS) prima
/// di chiamare il planner: se la migrazione avvenisse solo dentro il planner,
/// quelle sonde leggerebbero il testo v4 e il planner un altro testo. Qui
/// esiste **un** testo, deciso una volta, e da li' in poi e' v5.
fn testo_piano_dag(plan_text: &str) -> Result<Option<Cow<'_, str>>, PlenoraError> {
    if plan_schema_version(plan_text)? < u32::from(PLAN_SCHEMA_VERSION_V4) {
        return Ok(None);
    }
    migrazione_v4::testo_canonico_v5(plan_text, &PlanLimits::default()).map(Some)
}

/// Tetto sui byte di un documento JSON di controllo letto da file.
///
/// Coincide con `PlanLimits::max_plan_json_bytes` di default: i piani legacy
/// e gli schemi di comando sono documenti di controllo della stessa classe, e
/// non c'e' ragione perche' abbiano un tetto diverso — o nessun tetto.
const MAX_CONTROL_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// Legge un documento JSON di CONTROLLO da file: limitato nei byte e
/// rifiutato se contiene chiavi duplicate.
///
/// E' l'unico lettore dei documenti di controllo della CLI. Prima ogni sito
/// chiamava `serde_json::from_reader` per conto proprio: nessun tetto sui
/// byte, e chiavi duplicate risolte con «vince l'ultima» — la stessa
/// ambiguita' che il piano DAG rifiuta, lasciata aperta sui piani legacy,
/// sugli schemi di comando e sulle sonde di instradamento.
///
/// Confine di lettura (BLOCK-03): gli errori nascono leggendo la sorgente.
fn read_control_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, PlenoraError> {
    let text = read_control_json_text(path)?;
    plenora_core::json::ensure_no_duplicate_keys(&text)?;
    Ok(serde_json::from_str(&text)?)
}

/// Testo di un documento JSON di controllo, entro [`MAX_CONTROL_JSON_BYTES`].
fn read_control_json_text(path: &Path) -> Result<String, PlenoraError> {
    let read = (|| -> Result<String, PlenoraError> {
        let file = File::open(path)?;
        let declared = file.metadata()?.len();
        if declared > MAX_CONTROL_JSON_BYTES {
            return Err(contract(format!(
                "documento di controllo da {declared} byte oltre il limite {MAX_CONTROL_JSON_BYTES}"
            )));
        }
        // Il tetto si applica anche alla lettura, non solo alla dimensione
        // dichiarata: fra `metadata()` e la lettura il file puo' crescere.
        let mut text = String::new();
        BufReader::with_capacity(64 * 1024, file)
            .take(MAX_CONTROL_JSON_BYTES.saturating_add(1))
            .read_to_string(&mut text)?;
        if text.len() as u64 > MAX_CONTROL_JSON_BYTES {
            return Err(contract(format!(
                "documento di controllo oltre il limite {MAX_CONTROL_JSON_BYTES} byte"
            )));
        }
        Ok(text)
    })();
    read.map_err(|error| error.with_phase(ErrorPhase::Read))
}

/// Contratto assente per un input gia' accoppiato: invariante nostra, non un
/// errore del chiamante.
fn contract_error_missing(name: &str) -> PlenoraError {
    PlenoraError::Internal(format!(
        "contratto di discovery assente per l'input `{name}`"
    ))
}

/// Testo del piano, letto una volta e gia' verificato contro le chiavi
/// duplicate.
///
/// Il piano attraversa piu' sonde (`schema_version`, `inputs`,
/// `crs_decisions`) e infine la deserializzazione vera: il controllo si fa
/// QUI, sul testo, cosi' vale per tutte insieme invece che dipendere da quale
/// sonda lo legge per prima. Per i piani DAG `PlanV5::parse` ripete la
/// verifica: e' idempotente e copre anche chi non passa dalla CLI.
fn read_control_plan_text(path: &Path) -> Result<String, PlenoraError> {
    let text = read_control_json_text(path)?;
    plenora_core::json::ensure_no_duplicate_keys(&text)?;
    Ok(text)
}

/// Schema Arrow dell'header IPC di un input (file o stream format): nessuna
/// riga di dati letta.
///
/// Passa dal lettore di confine condiviso ([`plenora_engine::ipc_boundary`]):
/// framing e limiti pre-validati prima che arrow allochi, panico di
/// `fb_to_schema` convertito in errore. La CLI non apre piu'
/// `FileReader`/`StreamReader` per conto proprio su input non fidati.
///
/// Confine di lettura (BLOCK-03): gli errori `Io`/`DataMapping` di apertura
/// e parse dell'header nascono leggendo la sorgente — tag
/// [`ErrorPhase::Read`].
fn ipc_header_schema(path: &Path) -> Result<SchemaRef, PlenoraError> {
    ipc_boundary::header_schema(path, &IpcLimits::default())
}

/// Input lazy per l'executor: IPC file o stream format, sniffato dal magic.
fn open_input(path: &Path, limits: &IpcLimits) -> Result<Input, PlenoraError> {
    Input::read_ipc_with_limits(path, limits)
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
    DataContract::new(
        schema,
        geometries,
        active_geometry,
        ContractProperties::default(),
    )
}

/// Stato CRS del contratto dalla lettura di contratto completata (R2.7),
/// con la collocazione di R4.6.3 (v2.0-rc9/rc10): il centro NON risolve
/// un'incoerenza dichiarata in assenza di una decisione esplicita nel piano
/// — la preserva come [`ContractCrs::DeclaredUnresolved`] con le
/// dichiarazioni originali, mai un errore (non e' il bordo di scrittura) e
/// mai una scelta silenziosa.
///
/// Regole, in ordine (emendamento 2026-07-31 — classe A: la co-presenza
/// `crs_id` + `crs_definition` della regola (2a) vale SOLO per input NON
/// dichiarati; il conflitto numerico `crs_id`/`srid` della regola (2b) resta
/// sempre bloccante; un `resolved` dichiarato con doppia rappresentazione si
/// onora con risoluzione + verifica di coerenza):
///
/// 1. `crs_resolution = declared_unresolved` con almeno una
///    rappresentazione: il produttore dichiara l'incoerenza — preservata
///    cosi' com'e', NESSUNA risoluzione tentata (cambio di comportamento
///    dichiarato: prima una definizione risolvibile era risolta ed emessa
///    come `resolved`; nessuna chiamata al backend, quindi nessun
///    `BackendUnavailable`). Le rappresentazioni contano per precedenza
///    R4.3.1: definizione, identificatore, poi SRID numerico — un
///    `declared_unresolved` con SOLO `srid` (il produttore conosce il
///    codice dal catalogo ma non puo' inventare l'autorita', R4.4) e'
///    legittimo: lo stato e' `DeclaredUnresolved` con
///    `crs_id`/`definition` assenti (mai sintetizzati) e lo SRID resta
///    custodito dallo schema Arrow originale;
/// 2. conflitti DECIDIBILI senza backend: (2a) SOLO per input NON dichiarati
///    (`crs_resolution` assente — il caso per cui la regola e' nata, la
///    doppia rappresentazione `GeoArrow` legacy), `crs_id` e
///    `crs_definition` co-presenti (l'accordo non e' decidibile
///    testualmente — R2.7: mai arbitrato sul dato; prima vinceva
///    `crs_definition`, scelta silenziosa); (2b) SEMPRE, anche con
///    `crs_resolution = resolved`, `crs_id` nella forma `authority:code` con
///    codice numerico discordante da `srid` (R4.3.1; prima lo `srid` era
///    ignorato e l'identificatore risolto — conciliazione silenziosa). Lo
///    stato diventa `DeclaredUnresolved` con le dichiarazioni. Prima
///    dell'emendamento la sola (2a) scattava anche con `crs_resolution`
///    esplicitamente dichiarato, rovesciando la dichiarazione del produttore
///    (bug del caso owner: shapefile EPSG:3003 con WKT coerente degradato a
///    `declared_unresolved`);
/// 3. una rappresentazione (canonica o legacy `geo.crs`), o `resolved`
///    dichiarato: risoluzione contro il backend PROJ, come sempre — un
///    fallimento di risoluzione resta un errore `Crs`, NON diventa
///    `DeclaredUnresolved` (limite dichiarato: il produttore che sa di non
///    poter garantire la risoluzione dichiara `declared_unresolved`
///    esplicitamente, come nel corpus di conformita'). Con `resolved`
///    dichiarato ED ENTRAMBE `crs_id` e `crs_definition`, alla risoluzione
///    riuscita segue la verifica di coerenza decidibile
///    ([`verify_declared_coherence`]): coerenza → `Resolved`; mismatch o
///    confronto non decidibile → `DeclaredUnresolved` con le dichiarazioni
///    originali (mai un rovesciamento silenzioso). Effetto collaterale
///    DICHIARATO: senza `proj-backend`, un input `resolved` con doppia
///    rappresentazione prima passava come `DeclaredUnresolved` (la (2a)
///    scattava senza backend), ora fallisce con errore `Crs` (risoluzione
///    impossibile) — coerente col comportamento per `resolved` a
///    rappresentazione singola di questa regola: era la (2a) l'anomalia;
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
    // R4.3.1: anche il solo SRID numerico e' una rappresentazione (dopo
    // definizione e identificatore) — senza `crs_id`/`definition` lo stato
    // li porta assenti (R4.4: mai sintetizzarli).
    if keys.crs_resolution == Some(CrsResolution::DeclaredUnresolved)
        && (crs_id.is_some() || definition.is_some() || keys.srid.is_some())
    {
        return Ok(ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format: keys.crs_definition_format,
        });
    }
    // (2) Un conflitto numerico decidibile fra identificatore e SRID non puo'
    // essere nascosto da una dichiarazione `resolved`: si preservano tutte
    // le rappresentazioni originali e non si invoca il backend CRS.
    if let (Some(id), Some(srid)) = (&crs_id, keys.srid) {
        if authority_code(id).is_some_and(|code| code != srid) {
            return Ok(ContractCrs::DeclaredUnresolved {
                crs_id,
                definition,
                definition_format: keys.crs_definition_format,
            });
        }
    }
    // (3) La co-presenza di due rappresentazioni risolvibili resta
    // indecidibile per gli input che non dichiarano uno stato.
    if keys.crs_resolution.is_none() {
        // Due rappresentazioni risolvibili co-presenti: accordo non
        // decidibile, il centro non sceglie.
        if crs_id.is_some() && definition.is_some() {
            return Ok(ContractCrs::DeclaredUnresolved {
                crs_id,
                definition,
                definition_format: keys.crs_definition_format,
            });
        }
    }
    // (4) La rappresentazione completata (canonica o legacy) alimenta la
    // stessa risoluzione di sempre; la verifica di coerenza post-risoluzione
    // riguarda il solo caso `resolved` dichiarato con doppia
    // rappresentazione.
    if let Some(definition_text) = definition.as_deref().or(crs_id.as_deref()) {
        let resolved = resolve_crs(definition_text, "crs")?;
        if keys.crs_resolution == Some(CrsResolution::Resolved) {
            if let (Some(id), Some(text)) = (crs_id.as_deref(), definition.as_deref()) {
                return Ok(verify_declared_coherence(
                    resolved,
                    id,
                    text,
                    keys.crs_definition_format,
                ));
            }
        }
        return Ok(ContractCrs::Resolved(resolved));
    }
    if let Some(definition) = crs_definition_from_metadata(field_name, geo_metadata)? {
        return Ok(ContractCrs::Resolved(resolve_crs(&definition, "crs")?));
    }
    // (5) R4.1: mai collassare una dichiarazione esplicita su `missing` —
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

/// Verifica di coerenza DECIDIBILE dopo la risoluzione, per un input
/// `resolved` con doppia rappresentazione (piano-v5.md#contratti-di-input, emendamento 2026-07-31
/// — classe A): risolve anche `crs_id` e confronta l'intera coppia
/// autorita'+codice dedotta dai due canonical.
///
/// - entrambi decidibili e UGUALI: la doppia dichiarazione e' coerente →
///   `Resolved` (il caso owner: WKT Monte Mario risolve a id EPSG:3003);
/// - entrambi decidibili e DIVERSI: la dichiarazione `resolved` e'
///   dimostrabilmente falsa → `DeclaredUnresolved` con le dichiarazioni
///   originali (forma identica al braccio (1)): non passa e nulla si perde;
/// - confronto NON decidibile (identificatore non risolvibile, codice non
///   numerico o canonical senza `id`): mai arbitrato (R2.7) — la co-presenza
///   non verificabile resta un'incoerenza dichiarabile →
///   `DeclaredUnresolved`.
fn verify_declared_coherence(
    resolved: ResolvedCrs,
    crs_id: &str,
    definition: &str,
    definition_format: Option<CrsDefinitionFormat>,
) -> ContractCrs {
    let resolved_identifier = resolved.authority_identifier();
    let simple_identifier = plenora_core::crs::authority_code_identifier(crs_id);
    let coherent = simple_identifier.map_or_else(
        || {
            resolve_crs(crs_id, "crs").ok().is_some_and(|declared| {
                matches!(
                    (declared.authority_identifier(), resolved_identifier),
                    (Some(left), Some(right))
                        if left.0.eq_ignore_ascii_case(right.0) && left.1 == right.1
                )
            })
        },
        |declared| {
            resolved_identifier.is_some_and(|canonical| {
                declared.0.eq_ignore_ascii_case(canonical.0) && declared.1 == canonical.1
            })
        },
    );
    if coherent {
        return ContractCrs::Resolved(resolved);
    }
    ContractCrs::DeclaredUnresolved {
        crs_id: Some(crs_id.to_owned()),
        definition: Some(definition.to_owned()),
        definition_format,
    }
}

/// Codice numerico di un identificatore `authority:code` (es. `EPSG:4326`
/// -> 4326); `None` per ogni altra forma — il confronto con `srid` non e'
/// decidibile e l'identificatore resta intero alla risoluzione. Il parsing
/// vive in `plenora-core` (unica fonte condivisa, piano-v5.md#contratti-di-input emendamento
/// 2026-07-31: lo stesso helper alimenta la deduzione `srid` del percorso
/// legacy in `arrow_adapter`).
fn authority_code(crs_id: &str) -> Option<u32> {
    plenora_core::crs::authority_code_srid(crs_id)
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
    let types =
        keys.types
            .as_ref()
            .map_or_else(GeometryColumnContract::undeclared_types, |types| {
                ContractProperty::new(
                    PropertyConfidence::Declared(types.clone()),
                    PropertyScope::Schema,
                )
            });
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

/// Accoppia gli input della riga di comando a quelli dichiarati dal piano DAG.
///
/// Nella forma NOMINALE l'accoppiamento e' quello scritto: ogni nome dev'essere
/// dichiarato dal piano e ogni input dichiarato dev'essere fornito, una volta
/// sola.
///
/// La forma POSIZIONALE e' ammessa **solo con un input dichiarato**. Con due o
/// piu' input non e' verificabile: due file scambiati con lo stesso schema
/// producono un risultato sbagliato invece di un errore — il piano gira, i
/// contratti combaciano, e nessuno se ne accorge. Era l'unico posto della CLI
/// in cui uno scambio dell'utente non era intercettabile dal componente; ora
/// quella forma non arriva all'esecuzione.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` su nome non dichiarato, input dichiarato ma non
/// fornito, forma posizionale con piu' di un input dichiarato, o conteggio
/// diverso nella forma posizionale.
fn pair_v4_inputs(
    probe: &PlanInputsProbe,
    inputs: &V4Inputs,
) -> Result<Vec<(String, PathBuf)>, PlenoraError> {
    let paths = match inputs {
        V4Inputs::Named(named) => {
            for (name, _) in named {
                if !probe.inputs.iter().any(|declared| declared == name) {
                    return Err(contract(format!(
                        "input `{name}` non dichiarato dal piano (dichiarati: {})",
                        probe.inputs.join(", ")
                    )));
                }
            }
            // L'ordine restituito e' quello del PIANO, non quello della riga
            // di comando: cosi' l'ordine degli argomenti non e' osservabile a
            // valle e non puo' diventare una dipendenza implicita.
            return probe
                .inputs
                .iter()
                .map(|declared| {
                    named
                        .iter()
                        .find(|(name, _)| name == declared)
                        .map(|(name, path)| (name.clone(), path.clone()))
                        .ok_or_else(|| {
                            contract(format!(
                                "input `{declared}` dichiarato dal piano ma non fornito: \
                                 aggiungere `--input {declared}=PERCORSO`"
                            ))
                        })
                })
                .collect();
        }
        V4Inputs::Positional(paths) => paths,
    };
    if probe.inputs.len() > 1 {
        // Con piu' di un input la forma posizionale non e' VERIFICABILE: due
        // percorsi scambiati sono indistinguibili da due percorsi giusti, e
        // se gli schemi coincidono il piano gira producendo il risultato
        // sbagliato. Un avviso non basta — nei log di una pipeline non lo
        // legge nessuno — quindi si rifiuta prima di toccare i file,
        // indicando la forma che chiude il problema. Resta ammessa con un
        // input solo, dove non c'e' niente da scambiare.
        return Err(contract(format!(
            "`--inputs` accoppia i percorsi per POSIZIONE e non e' ammesso con {} input \
             dichiarati: usare la forma nominale `{}`",
            probe.inputs.len(),
            probe
                .inputs
                .iter()
                .map(|name| format!("--input {name}=PERCORSO"))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }
    if probe.inputs.len() != paths.len() {
        return Err(contract(format!(
            "il piano dichiara {} input ({}) ma ne sono stati forniti {}",
            probe.inputs.len(),
            probe.inputs.join(", "),
            paths.len()
        )));
    }
    Ok(probe
        .inputs
        .iter()
        .cloned()
        .zip(paths.iter().cloned())
        .collect())
}

/// Contratti degli input di un piano DAG, scoperti dagli header IPC.
fn discover_contracts(
    pairs: &[(String, PathBuf)],
) -> Result<Vec<(String, DataContract)>, PlenoraError> {
    pairs
        .iter()
        .map(|(name, path)| {
            discover_input_contract(path)
                .map(|contract| (name.clone(), contract))
                .map_err(|error| at_input(name, path, error))
        })
        .collect()
}

/// Applica le decisioni CRS esplicite del piano DAG (`crs_decisions`,
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
/// piano e coperta dal `plan_hash` (piano-v5.md#identita-e-fingerprint); il fingerprint del contratto
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

/// Descrizione completa di un input: cio' che serve per SCRIVERE un piano
/// contro quel file, e il fingerprint con cui il piano sara' poi verificato.
///
/// I campi non geometrici non hanno un `field_id` nel contratto — l'identita'
/// interna e' assegnata dal grafo, non dall'input — e non se ne inventa uno.
fn describe_json(path: &Path, contract: &DataContract) -> Result<serde_json::Value, PlenoraError> {
    let fingerprint = planner::contract_fingerprint(contract)?;
    let geometries: Vec<serde_json::Value> = contract
        .geometries
        .iter()
        .map(|geometry| {
            serde_json::json!({
                "name": geometry.name,
                "field_id": geometry.field_id.0,
                "nullable": geometry.nullable,
                "dimensions": geometry.dimensions.as_str(),
                "encoding": geometry.encoding.map(GeometryEncoding::as_str),
                "crs_resolution": geometry.crs.resolution().as_str(),
                "crs": match &geometry.crs {
                    ContractCrs::Resolved(crs) | ContractCrs::ResolvedByDecision(crs) =>
                        serde_json::Value::String(crs.definition().to_owned()),
                    ContractCrs::DeclaredUnresolved { .. } | ContractCrs::Missing =>
                        serde_json::Value::Null,
                },
                "types_declaration": geometry
                    .types
                    .value()
                    .map(|types| types.declaration().as_str()),
                "types": geometry
                    .types
                    .value()
                    .map(GeometryTypesProperty::to_canonical_list),
                "active": contract
                    .active_geometry_column()
                    .is_some_and(|active| active.name == geometry.name),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "status": "ok",
        "protocol_version": 1,
        "input": path.display().to_string(),
        "contract_fingerprint": fingerprint.to_hex(),
        "fields": contract_json(contract)["fields"],
        "geometries": geometries,
    }))
}

/// `describe`: cosa contiene un input, senza eseguire nulla.
///
/// E' il primo comando da invocare per scrivere un piano: senza, i nomi delle
/// colonne, il CRS e l'encoding si scoprono solo facendo fallire un `run`.
/// L'input passa dal confine IPC come in esecuzione — framing pre-validato,
/// barriera anti-panico — quindi cio' che `describe` accetta e' cio' che `run`
/// accettera'.
fn describe_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    let input = value_after(args, "--input")?;
    let contract = discover_input_contract(Path::new(&input))?;
    let documento = describe_json(Path::new(&input), &contract)?;
    match OutputFormat::active() {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&documento)?),
        OutputFormat::Markdown => print!("{}", describe_markdown(&documento)),
    }
    Ok(())
}

/// Resa leggibile di [`describe_json`]. Stesso contenuto, altra forma: un
/// campo che compare nel JSON e non qui sarebbe una descrizione parziale
/// travestita da descrizione.
fn describe_markdown(documento: &serde_json::Value) -> String {
    use std::fmt::Write as _;
    let mut testo = String::new();
    let vuoto = Vec::new();
    // `write!` su `String` non fallisce; l'esito si ignora esplicitamente
    // invece di propagarlo per un canale che non ha errori.
    let _ = writeln!(
        testo,
        "# {}\n",
        documento["input"].as_str().unwrap_or("(input)")
    );
    let _ = writeln!(
        testo,
        "Fingerprint del contratto: `{}`\n",
        documento["contract_fingerprint"].as_str().unwrap_or("?")
    );
    let _ = writeln!(testo, "## Campi\n");
    let _ = writeln!(testo, "| nome | tipo | nullable |");
    let _ = writeln!(testo, "|---|---|---|");
    for campo in documento["fields"].as_array().unwrap_or(&vuoto) {
        let _ = writeln!(
            testo,
            "| `{}` | {} | {} |",
            campo["name"].as_str().unwrap_or("?"),
            campo["data_type"].as_str().unwrap_or("?"),
            campo["nullable"]
        );
    }
    let geometrie = documento["geometries"].as_array().unwrap_or(&vuoto);
    if geometrie.is_empty() {
        let _ = writeln!(testo, "\nNessuna colonna geometrica.");
        return testo;
    }
    let _ = writeln!(testo, "\n## Geometrie\n");
    for geometria in geometrie {
        let _ = writeln!(
            testo,
            "- `{}` (field_id {}){}",
            geometria["name"].as_str().unwrap_or("?"),
            geometria["field_id"],
            if geometria["active"] == serde_json::Value::Bool(true) {
                " — attiva"
            } else {
                ""
            }
        );
        let _ = writeln!(
            testo,
            "  - dimensioni: {} · encoding: {} · CRS: {} ({})",
            geometria["dimensions"].as_str().unwrap_or("?"),
            geometria["encoding"].as_str().unwrap_or("non dichiarato"),
            geometria["crs"].as_str().unwrap_or("assente"),
            geometria["crs_resolution"].as_str().unwrap_or("?")
        );
        let _ = writeln!(
            testo,
            "  - tipi: {} ({})",
            geometria["types"].as_str().unwrap_or("non dichiarati"),
            geometria["types_declaration"]
                .as_str()
                .unwrap_or("non dichiarata")
        );
    }
    testo
}

/// Riepilogo JSON di `validate` per un piano DAG: nodi, archi con contratti,
/// segmenti con modo e strategia, capability e identita' piano-v5.md#identita-e-fingerprint.
///
/// # Errors
///
/// `Internal` se un arco del grafo manca dai contratti: impossibile per
/// costruzione su un grafo validato (stessa invariante di
/// [`ValidatedGraph::output_contract`]), ma il compilatore non puo'
/// dimostrarlo — l'invariante violata diventa un errore esplicito, mai un
/// panic (R6).
fn graph_summary_json(
    graph: &ValidatedGraph,
    execution: &ExecutionPlan,
) -> Result<serde_json::Value, PlenoraError> {
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
        "schema_version": PLAN_SCHEMA_VERSION_V5,
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
    }))
}

/// Metriche JSON di un `run` v4: per nodo logico e per segmento (righe,
/// batch e byte in/out, wall time in millisecondi), i totali di
/// pubblicazione, il contatore dei fallback della fusione geo (D12.7: ogni
/// fallback governor e' osservabile, mai silenzioso), l'osservabilita' dei
/// lease di memoria e le metriche di spill aggregate (architettura.md#memoria).
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
        "schema_version": PLAN_SCHEMA_VERSION_V5,
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

/// `validate` di un piano DAG: planner DAG + `explain` per la strategia, con
/// riepilogo JSON su stdout (architettura.md#planner-ed-executor: `prepare` e' interna all'engine).
/// `geo_fusion` e' il kill switch D12.9 (flag `--no-geo-fusion`): a `false`
/// i gruppi di fusione non si formano e `explain` mostra la strategia non
/// fusa.
fn validate_dag_v4(
    plan_text: &str,
    inputs: &V4Inputs,
    geo_fusion: bool,
) -> Result<(), Box<dyn Error>> {
    let probe: PlanInputsProbe = serde_json::from_str(plan_text)?;
    let pairs = pair_v4_inputs(&probe, inputs)?;
    let mut contracts = discover_contracts(&pairs)?;
    apply_crs_decisions(&probe, &mut contracts)?;
    let graph = planner::validate(plan_text, &contracts)?;
    let execution = explain(
        &graph,
        &RuntimeContext {
            geo_fusion,
            ..RuntimeContext::default()
        },
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&graph_summary_json(&graph, &execution)?)?
    );
    Ok(())
}

/// `run` di un piano DAG: esecuzione DAG e pubblicazione atomica dell'output,
/// con metriche JSON su stdout. Installa l'handler Ctrl-C: al cancel
/// l'executor propaga `PlenoraError::Cancelled`, il publish atomico non e'
/// mai raggiunto e `main` esce con [`EXIT_CANCELLED`]. `geo_fusion` e' il
/// kill switch D12.9 (flag `--no-geo-fusion`).
fn run_dag_v4(
    plan_text: &str,
    inputs: &V4Inputs,
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
    let pairs = pair_v4_inputs(&probe, inputs)?;
    let mut contracts = discover_contracts(&pairs)?;
    apply_crs_decisions(&probe, &mut contracts)?;
    let graph = planner::validate(plan_text, &contracts)?;
    // `max_parallelism` si applica QUI, prima di aprire gli input e prima di
    // qualunque uso di Rayon: dimensiona il pool del processo, che e' l'unica
    // leva che vincola davvero tutti i percorsi paralleli dei kernel. Senza
    // questo passo il limite era una promessa di risorsa, non un tetto.
    parallelism::configure(graph.effective_limits().max_parallelism)?;
    let token = CancellationToken::new();
    install_ctrlc_handler(&token)?;
    let runtime = RuntimeContext {
        cancellation: token,
        geo_fusion,
        max_parallelism: graph.effective_limits().max_parallelism,
        ..RuntimeContext::default()
    };
    // I tetti del confine IPC derivano dai limiti EFFETTIVI del piano: il
    // body dichiarato di ogni messaggio e' confrontato con `max_batch_bytes`
    // prima che arrow allochi, non dopo che il batch e' stato costruito.
    let ipc_limits = ipc_boundary::limits_from_plan(
        graph.effective_limits(),
        runtime.batch_target.max_batch_bytes,
    );
    // Gli input portano il proprio contratto: l'esecuzione verifica allora il
    // fingerprint COMPLETO contro quello registrato nel grafo, non il solo
    // schema Arrow. E' lo stesso contratto su cui il piano e' stato validato,
    // quindi il confine si chiude senza rileggere nulla.
    // Profilo STRETTO: un input senza contratto non e' un'omissione tollerata
    // ma un errore. La CLI ha sempre i contratti della discovery, quindi il
    // profilo permissivo non le serve — e non averlo a disposizione e' cio'
    // che impedisce a una modifica futura di reintrodurlo per distrazione.
    let mut inputs = Inputs::strict();
    for (name, path) in &pairs {
        let contract = contracts
            .iter()
            .find(|(declared, _)| declared == name)
            .map(|(_, contract)| contract.clone())
            .ok_or_else(|| contract_error_missing(name))?;
        inputs.add_with_contract(name.clone(), open_input(path, &ipc_limits)?, contract)?;
    }
    let output = execute(&graph, inputs, runtime)?;
    let (metrics, outcome) =
        output.write_ipc_file_with_profile(output_path, PublishProfile::Atomic)?;
    let mut documento = metrics_json(&graph, &metrics);
    if let Some(oggetto) = documento.as_object_mut() {
        oggetto.insert(
            "durability_confirmed".to_owned(),
            serde_json::Value::Bool(durabilita_confermata(outcome)),
        );
    }
    println!("{}", serde_json::to_string_pretty(&documento)?);
    Ok(())
}

/// Percorsi di input per un piano DAG: `--input` singolo e/o `--inputs`
/// multiplo (valori fino al prossimo flag).
/// Sorgenti degli input di un piano DAG, come dichiarate sulla riga di
/// comando.
///
/// Le due forme non si mescolano: o l'accoppiamento e' esplicito, o e'
/// posizionale. Accettarle insieme darebbe una riga di comando in cui meta'
/// degli input e' verificabile a colpo d'occhio e meta' no.
#[derive(Debug, PartialEq, Eq)]
enum V4Inputs {
    /// Forma NOMINALE `--input nome=percorso`, ripetibile: l'accoppiamento e'
    /// scritto, non dedotto.
    Named(Vec<(String, PathBuf)>),
    /// Forma POSIZIONALE `--inputs a.arrow b.arrow` (deprecata): i percorsi
    /// seguono l'ordine di dichiarazione degli input nel piano.
    Positional(Vec<PathBuf>),
}

/// `true` se il valore di `--input` e' nella forma `nome=percorso`.
///
/// Il nome e' cio' che precede il PRIMO `=`: dev'essere non vuoto e non
/// contenere separatori di percorso ne' due punti, cosi' un percorso assoluto
/// resta un percorso anche se contenesse un `=` piu' avanti. Un file che si
/// chiama davvero `nome=x.arrow` si passa con `--inputs`, oppure prefissato
/// (`./nome=x.arrow`).
fn is_named_input(value: &str) -> bool {
    value.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && !name.contains(['/', '\\', ':']) && !name.starts_with('-')
    })
}

/// Raccoglie gli input di un piano DAG dalla riga di comando.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se le due forme sono mescolate, se un nome e'
/// ripetuto, se il percorso di una coppia e' vuoto, o se manca il valore di
/// un flag.
fn v4_inputs(args: &[String]) -> Result<V4Inputs, PlenoraError> {
    let mut named: Vec<(String, PathBuf)> = Vec::new();
    let mut positional: Vec<PathBuf> = Vec::new();

    // `--input` e' ripetibile nella forma nominale; in quella posizionale
    // resta il singolo percorso di un piano a un solo input.
    for (index, argument) in args.iter().enumerate() {
        if argument != "--input" {
            continue;
        }
        let value = args
            .get(index + 1)
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| contract("valore mancante per --input"))?;
        if !is_named_input(value) {
            positional.push(PathBuf::from(value));
            continue;
        }
        let (name, path) = value
            .split_once('=')
            .ok_or_else(|| contract("forma --input nome=percorso non riconosciuta"))?;
        if path.is_empty() {
            return Err(contract(format!(
                "input `{name}`: percorso vuoto in `--input {name}=`"
            )));
        }
        if named.iter().any(|(declared, _)| declared == name) {
            return Err(contract(format!("input `{name}` indicato due volte")));
        }
        named.push((name.to_owned(), PathBuf::from(path)));
    }

    if let Some(index) = args.iter().position(|argument| argument == "--inputs") {
        positional.extend(
            args[index + 1..]
                .iter()
                .take_while(|argument| !argument.starts_with("--"))
                .map(PathBuf::from),
        );
    }

    if !named.is_empty() && !positional.is_empty() {
        return Err(contract(
            "forma nominale e posizionale mescolate: usare `--input nome=percorso` per \
             tutti gli input, oppure `--inputs` per tutti",
        ));
    }
    if named.is_empty() {
        return Ok(V4Inputs::Positional(positional));
    }
    Ok(V4Inputs::Named(named))
}

fn reject_legacy_row_diagnostics_plan(plan_text: &str) -> Result<(), PlenoraError> {
    // Fail-closed su TUTTI i piani legacy che contengono op row-diagnostics,
    // anche blocking/secondary: nel percorso legacy non esiste gate
    // provenance (quello e' solo DAG) e un nodo blocking (es. sort)
    // renderebbe gli indici pubblicati posizioni post-riordino, non
    // `source_row_zero_based`. Nessun indice inventato: si richiede DAG.
    //
    // Autorita' UNICA: `OperationDescriptor::emits_row_diagnostics`
    // (catalogo plenora-core), la stessa del gate provenance del planner e
    // del machinery di segmento dell'executor — nessuna lista locale
    // duplicata (formula/expression erano omesse qui; hmac_sha256
    // non emette, md5/sha256 solo con null_policy=error). La scansione
    // precede la validazione legacy: un'op diagnostica richiede DAG
    // anche se il resto del piano sarebbe invalido — mai eseguire per poi
    // scoprire indici inventati.
    let document: serde_json::Value = serde_json::from_str(plan_text)?;
    let requires_v4 = document
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|steps| {
            steps.iter().any(|step| {
                let Some(operation) = step.get("operation").and_then(serde_json::Value::as_str)
                else {
                    return false;
                };
                // Risoluzione come in validazione v4: id canonico o alias
                // legacy; i nomi sconosciuti restano al rifiuto della
                // validazione legacy sotto (comportamento invariato).
                let Some(descriptor) = find_operation(operation) else {
                    return false;
                };
                let config = step.get("config").unwrap_or(&serde_json::Value::Null);
                descriptor.emits_row_diagnostics(config)
            })
        });
    if requires_v4 {
        return Err(PlenoraError::Unsupported(
            "operazione con diagnostics row-scoped richiede un piano DAG".to_owned(),
        ));
    }
    let validated: Plan = serde_json::from_str(plan_text)?;
    let _ = validated.validate()?;
    Ok(())
}

/// Dispatch di `run`: DAG se il piano dichiara `schema_version` >= 4,
/// pipeline tabellare legacy altrimenti (comportamento invariato).
fn run_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    OutputFormat::require_json("run")?;
    let plan_path = value_after(args, "--plan")?;
    let output_path = value_after(args, "--output")?;
    let plan_text = read_control_plan_text(Path::new(&plan_path))?;
    if let Some(plan_text) = testo_piano_dag(&plan_text)? {
        if args.iter().any(|argument| argument == "--right") {
            return Err(contract(
                "--right non e' ammesso per i piani DAG: usare --inputs con i percorsi \
                 nell'ordine di dichiarazione degli input del piano",
            )
            .into());
        }
        return run_dag_v4(
            plan_text.as_ref(),
            &v4_inputs(args)?,
            &output_path,
            !has_flag(args, "--no-geo-fusion"),
        );
    }
    reject_legacy_row_diagnostics_plan(&plan_text)?;
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

fn help_text() -> String {
    format!(
        "plenora-data-tools {}

  plenora-data-tools catalog [--family table|geo]
  plenora-data-tools describe --input INPUT.arrow                          (alias: inspect-dataset)
  plenora-data-tools validate --plan PLAN.json --input NOME=INPUT.arrow...
  plenora-data-tools run --plan PLAN.json --input NOME=INPUT.arrow... --output OUTPUT.arrow [--no-geo-fusion]   (piani DAG v5)
  plenora-data-tools run --plan PLAN.json --input INPUT.arrow [--right RIGHT.arrow] --output OUTPUT.arrow       (piani legacy, schema_version <= 3)
  plenora-data-tools run --plan PLAN.json --inputs INPUT.arrow --output OUTPUT.arrow                            (posizionale: solo piani a UN input)
  plenora-data-tools capabilities
  plenora-data-tools transform --input INPUT --schema SCHEMA.json --output OUTPUT                               (deprecato: usare run con un piano)
  plenora-data-tools spatial-join --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS                  (deprecato: usare run con un piano)
  plenora-data-tools transform-arrow --input INPUT --schema SCHEMA.json --output OUTPUT                          (deprecato: usare run con un piano)
  plenora-data-tools pair-arrow --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS                    (deprecato: usare run con un piano)
  plenora-data-tools self-test [--output RESULT.bin]
  plenora-data-tools --version",
        env!("CARGO_PKG_VERSION")
    )
}

fn subcommand_help_text(command: &str) -> Option<&'static str> {
    match command {
        "catalog" => Some("Usage: plenora-data-tools catalog [--family table|geo]"),
        "describe" | "inspect-dataset" => Some(
            "Usage: plenora-data-tools describe --input INPUT.arrow

Stampa in JSON il contratto dell'input: campi, colonna geometrica, CRS,
encoding, tipi dichiarati e fingerprint del contratto. Non esegue nulla.",
        ),
        "validate" => Some(
            "Usage:
  plenora-data-tools validate --plan PLAN.json --input NOME=INPUT.arrow... [--no-geo-fusion]
  plenora-data-tools validate --plan PLAN.json --inputs INPUT.arrow   (posizionale: solo piani a UN input)",
        ),
        "run" => Some(
            "Usage:
  plenora-data-tools run --plan PLAN.json --input NOME=INPUT.arrow... --output OUTPUT.arrow [--no-geo-fusion]
  plenora-data-tools run --plan PLAN.json --input INPUT.arrow [--right RIGHT.arrow] --output OUTPUT.arrow   (piani legacy)
  plenora-data-tools run --plan PLAN.json --inputs INPUT.arrow --output OUTPUT.arrow                        (posizionale: solo piani a UN input)

La forma nominale lega ogni percorso al nome dell'input dichiarato dal piano:
due file scambiati diventano un errore invece di un risultato sbagliato. Con
piu' di un input dichiarato e' l'unica forma ammessa.",
        ),
        "capabilities" => Some("Usage: plenora-data-tools capabilities"),
        "transform" => Some(
            "Usage: plenora-data-tools transform --input INPUT --schema SCHEMA.json --output OUTPUT",
        ),
        "spatial-join" => Some(
            "Usage: plenora-data-tools spatial-join --left LEFT --right RIGHT --schema SCHEMA.json --output PAIRS",
        ),
        "transform-arrow" => Some(
            "Usage: plenora-data-tools transform-arrow --input INPUT --schema SCHEMA.json --output OUTPUT [--output-format plngeo3|ipc-file]",
        ),
        "pair-arrow" => Some(
            "Usage: plenora-data-tools pair-arrow --left LEFT --right RIGHT --schema SCHEMA.json --output OUTPUT [--output-format plngeo3|ipc-file]",
        ),
        "self-test" => Some("Usage: plenora-data-tools self-test [--output RESULT.bin]"),
        _ => None,
    }
}

/// Flag accettati da ciascun sottocomando, e quali possono ripetersi.
///
/// E' l'unico posto in cui la superficie degli argomenti e' dichiarata: il
/// controllo di §1.4 la confronta con l'help, e il dispatch la usa per
/// rifiutare cio' che non conosce. Tre elenchi separati divergerebbero.
///
/// `--format` non compare: e' globale e viene tolto dagli argomenti prima
/// del dispatch (`strip_output_format`).
struct SuperficieComando {
    /// Flag ammessi, compresi quelli senza valore.
    flag: &'static [&'static str],
    /// Flag che possono comparire piu' di una volta.
    ripetibili: &'static [&'static str],
}

const fn superficie(comando: &str) -> Option<SuperficieComando> {
    Some(match comando.as_bytes() {
        b"catalog" => SuperficieComando {
            flag: &["--family"],
            ripetibili: &[],
        },
        b"describe" | b"inspect-dataset" => SuperficieComando {
            flag: &["--input"],
            ripetibili: &[],
        },
        b"validate" => SuperficieComando {
            flag: &["--plan", "--input", "--inputs", "--no-geo-fusion"],
            // `--input NOME=PERCORSO` si ripete: un input per occorrenza.
            ripetibili: &["--input"],
        },
        b"run" => SuperficieComando {
            flag: &[
                "--plan",
                "--input",
                "--inputs",
                "--right",
                "--output",
                "--no-geo-fusion",
            ],
            ripetibili: &["--input"],
        },
        b"capabilities" => SuperficieComando {
            flag: &[],
            ripetibili: &[],
        },
        b"transform" => SuperficieComando {
            flag: &["--input", "--schema", "--output"],
            ripetibili: &[],
        },
        b"spatial-join" => SuperficieComando {
            flag: &["--left", "--right", "--schema", "--output"],
            ripetibili: &[],
        },
        b"transform-arrow" => SuperficieComando {
            flag: &["--input", "--schema", "--output", "--output-format"],
            ripetibili: &[],
        },
        b"pair-arrow" => SuperficieComando {
            flag: &[
                "--left",
                "--right",
                "--schema",
                "--output",
                "--output-format",
            ],
            ripetibili: &[],
        },
        b"self-test" => SuperficieComando {
            flag: &["--output"],
            ripetibili: &[],
        },
        _ => return None,
    })
}

/// Convalida la riga di comando di un sottocomando: nessun token puo'
/// restare inosservato.
///
/// Un parser che ignora cio' che non riconosce pubblica un output basato su
/// un'invocazione DIVERSA da quella che l'utente ha scritto. Qui ogni
/// argomento deve essere o un flag dichiarato, o il valore di un flag che ne
/// prende uno: tutto il resto e' un errore.
///
/// Casi chiusi, tutti verificati dalla matrice:
///
/// - flag sconosciuto (`--boh`), anche in forma breve (`-x`);
/// - flag a valore singolo ripetuto;
/// - **posizionale inatteso** (`run pippo --plan ...`);
/// - **flag usato come valore** (`--plan --output`), che silenziosamente
///   rendeva `--output` il nome del piano;
/// - **argomenti extra** dopo `--version` e `--help`.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` con l'elenco dei flag ammessi.
fn reject_unknown_flags(comando: &str, args: &[String]) -> Result<(), PlenoraError> {
    // `--help` e `--version` non prendono argomenti: qualunque token in piu'
    // e' un'invocazione che non si sta eseguendo.
    if matches!(comando, "--help" | "-h" | "--version" | "-V") {
        // `--json` e' il modificatore di formato di `--version` E SOLO SUO:
        // su `--help` veniva accettato e ignorato, cioe' un'invocazione che
        // il parser non eseguiva ma dichiarava valida.
        let ammette_json = matches!(comando, "--version" | "-V");
        let mut json_visto = false;
        for argument in args.iter().skip(1) {
            if ammette_json && argument.as_str() == "--json" && !json_visto {
                json_visto = true;
                continue;
            }
            return Err(contract(format!(
                "`{comando}` non accetta argomenti: `{argument}` di troppo"
            )));
        }
        return Ok(());
    }
    let Some(superficie) = superficie(comando) else {
        return Ok(());
    };
    let mut visti: Vec<&str> = Vec::new();
    let mut indice = 1;
    while indice < args.len() {
        let argument = args[indice].as_str();
        if argument == "--help" || argument == "-h" {
            // Ammesso, ma NON e' un lasciapassare per il resto della riga:
            // `run --help junk` deve fallire come qualunque altra
            // invocazione con un token estraneo.
            //
            // `--help` e `-h` sono lo STESSO flag: si registra la forma
            // canonica, altrimenti `run --help -h` non risultava una
            // ripetizione e passava.
            if visti.contains(&"--help") {
                return Err(contract(format!(
                    "flag `{argument}` ripetuto: `{comando}` ne accetta una sola occorrenza"
                )));
            }
            visti.push("--help");
            indice += 1;
            continue;
        }
        if !argument.starts_with('-') {
            return Err(contract(format!(
                "argomento posizionale `{argument}` non atteso da `{comando}`: \
                 ogni valore va introdotto dal proprio flag"
            )));
        }
        if !argument.starts_with("--") {
            // Forma breve: nessun sottocomando ne dichiara, e accettarla in
            // silenzio significherebbe ignorarla.
            return Err(contract(format!(
                "flag `{argument}` non riconosciuto da `{comando}`: le opzioni \
                 sono nella forma lunga `--nome`"
            )));
        }
        if !superficie.flag.contains(&argument) {
            return Err(contract(format!(
                "flag `{argument}` non riconosciuto da `{comando}` (ammessi: {})",
                if superficie.flag.is_empty() {
                    "nessuno".to_owned()
                } else {
                    superficie.flag.join(", ")
                }
            )));
        }
        if visti.contains(&argument) && !superficie.ripetibili.contains(&argument) {
            return Err(contract(format!(
                "flag `{argument}` ripetuto: `{comando}` ne accetta una sola occorrenza"
            )));
        }
        visti.push(argument);
        indice += 1;
        if argument == "--no-geo-fusion" {
            // Flag senza valore.
            continue;
        }
        if argument == "--inputs" {
            // Lista: consuma i valori fino al prossimo flag, ma almeno uno.
            let inizio = indice;
            while indice < args.len() && !(args[indice].starts_with('-') && args[indice].len() > 1)
            {
                indice += 1;
            }
            if indice == inizio {
                return Err(contract(format!("valore mancante per {argument}")));
            }
            continue;
        }
        // Flag a valore singolo: il valore deve esserci e NON deve essere un
        // altro flag. `--plan --output out.arrow` prendeva `--output` come
        // nome del piano e falliva molto piu' tardi, con un errore che non
        // parlava del vero problema.
        let Some(valore) = args.get(indice) else {
            return Err(contract(format!("valore mancante per {argument}")));
        };
        // Anche la forma breve e' un flag: `--plan -x` consumava `-x` come
        // nome del piano e falliva molto piu' tardi, con un errore che non
        // parlava del vero problema.
        if valore.starts_with('-') && valore.len() > 1 {
            return Err(contract(format!(
                "valore mancante per {argument}: `{valore}` e' un flag, non un valore"
            )));
        }
        indice += 1;
    }
    Ok(())
}

// Dispatch unico dei sottocomandi: la lunghezza e' data dalla sequenza
// lineare dei casi, non da complessita' logica; uno spezzone artificiale
// peggiorerebbe solo la leggibilita' (fase di pulizia: niente refactor
// strutturali).
#[allow(clippy::too_many_lines)]
fn run_with_args(args: &[String]) -> Result<(), Box<dyn Error>> {
    // La validazione precede QUALUNQUE uscita anticipata, help compreso:
    // `run --help junk` stampava l'aiuto e usciva con successo, ignorando
    // `junk`. Un parser che risponde «va bene» a un'invocazione che non ha
    // capito e' fail-open anche quando non pubblica nulla.
    if let Some(comando) = args.first() {
        reject_unknown_flags(comando, args)?;
    }
    if args
        .get(1)
        .is_some_and(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        if let Some(help) = args
            .first()
            .and_then(|command| subcommand_help_text(command))
        {
            println!("{help}");
            return Ok(());
        }
    }
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("{}", help_text());
            Ok(())
        }
        Some("--version" | "-V") => {
            // `--version --json` (o `--format json` esplicito) per gli
            // orchestratori: una versione che si legge solo a occhio non e'
            // verificabile da uno script.
            if has_flag(args, "--json") || OutputFormat::active() == OutputFormat::Json {
                println!("{}", serde_json::to_string_pretty(&version_json())?);
            } else {
                println!("plenora-data-tools {}", env!("CARGO_PKG_VERSION"));
            }
            Ok(())
        }
        Some("catalog") => catalog_command(args),
        Some("describe" | "inspect-dataset") => describe_command(args),
        Some("validate") => validate_command(args),
        Some("run") => run_command(args),
        Some("capabilities") => capabilities_command(),
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
            let output_format = arrow_output_format(args)?;
            let input = argument_value(args, "--input")?;
            let schema = argument_value(args, "--schema")?;
            let output = argument_value(args, "--output")?;
            let summary =
                execute_transform_arrow(&input, Path::new(&schema), &output, output_format)?;
            println!(
                "{{\"status\":\"ok\",\"rows\":{},\"output_rows\":{},\"sha256\":\"{}\"}}",
                summary.rows,
                summary.output_rows,
                hex_digest(&summary.checksum)
            );
            Ok(())
        }
        Some("pair-arrow") => {
            let output_format = arrow_output_format(args)?;
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
                output_format,
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
        _ => Err(contract(
            "comando non valido: `plenora-data-tools --help` elenca i comandi disponibili",
        )
        .into()),
    }
}

fn main() {
    // Ultima barriera del processo: un panico che sfugge — nostro o di una
    // dipendenza — deve diventare un ENVELOPE su stdout, non testo su stderr.
    //
    // Il gate R6 vieta le primitive di panico nelle nostre librerie, ma non
    // puo' vietarle ad arrow; e l'hook di default stampa su stderr prima
    // dell'unwinding, rompendo il contratto «stderr vuoto» proprio nel
    // momento peggiore. L'hook viene quindi silenziato e l'informazione
    // recuperata qui, dove ha un canale e un exit code.
    //
    // La politica vive in `plenora_core::panic_policy` perche' non riguarda
    // solo la CLI: un embedder — il futuro binding PyO3 compreso — ha lo
    // stesso problema su uno stderr che non e' nemmeno suo, e installa
    // `Sanitized`.
    //
    // L'ESITO va guardato, non ignorato. `install` risponde `false` se un
    // hook era gia' stato installato passando da quella API. Qui siamo il
    // processo e siamo la prima istruzione di `main`, quindi `false`
    // significa che qualcosa e' arrivato prima del nostro ingresso — e
    // allora il contratto «stderr vuoto» non e' piu' garantito, perche'
    // l'hook attivo non e' il nostro. Non e' un errore da cui uscire: e' un
    // fatto da DICHIARARE se poi un panico succede davvero. Silenziarlo
    // significherebbe promettere un canale pulito senza piu' governarlo.
    //
    // Nota di ambito: nemmeno un `true` rende l'hook inamovibile — un
    // `std::panic::set_hook` successivo, da qualunque componente, lo
    // sostituisce. Vedi la sezione «Che cosa questo modulo NON garantisce»
    // di `panic_policy`.
    let politica_nostra =
        plenora_core::panic_policy::install(plenora_core::panic_policy::PanicPolicy::Silent);
    let esito = std::panic::catch_unwind(esegui_processo);
    let codice = match esito {
        Ok(codice) => codice,
        Err(panico) => {
            let avvertenza = if politica_nostra {
                ""
            } else {
                "; hook di panico non installato da questo processo: \
                 il contenuto su stderr non e' sotto il nostro controllo"
            };
            let envelope = error_envelope(
                &PlenoraError::Internal(format!(
                    "panico non gestito: {}{avvertenza}",
                    descrivi_panico_locale(&panico)
                )),
                false,
            );
            let _ = emit_error_envelope(std::io::stdout().lock(), &envelope);
            EXIT_INTERNO
        }
    };
    std::process::exit(codice);
}

/// Descrizione PUBBLICA del payload di un panico: la forma, mai il contenuto.
///
/// Il testo di un panico non e' scritto da noi. Un `assert_eq!` dentro una
/// dipendenza puo' includere i valori confrontati, cioe' dati della riga:
/// pubblicarlo nell'envelope significherebbe esfiltrare contenuto dell'input
/// nei log di chi ci invoca. Stessa scelta del confine IPC e dell'executor.
fn descrivi_panico_locale(panico: &Box<dyn std::any::Any + Send>) -> &'static str {
    plenora_core::panic_policy::forma_payload(panico.as_ref())
}

/// Il processo vero e proprio: restituisce l'exit code invece di uscire, cosi'
/// la barriera anti-panico di `main` puo' avvolgerlo.
fn esegui_processo() -> i32 {
    let args: Vec<String> = env::args().skip(1).collect();
    let args = match strip_output_format(args) {
        Ok(args) => args,
        Err(error) => {
            let envelope = error_envelope(&error, false);
            let _ = emit_error_envelope(std::io::stdout().lock(), &envelope);
            return error_exit_code(&envelope);
        }
    };
    if let Err(error) = run_with_args(&args) {
        // Cancellazione cooperativa (errori-e-limiti.md#cancellazione): exit
        // code dedicato; il
        // publish atomico garantisce che nessun output parziale sia stato
        // pubblicato. Envelope §9 anche per la cancellazione (categoria
        // dedicata, fase/effetto/retry dagli assi).
        let cancelled = error
            .downcast_ref::<PlenoraError>()
            .is_some_and(PlenoraError::is_cancelled);
        let envelope = error_envelope(error.as_ref(), cancelled);
        let exit_code = error_exit_code(&envelope);
        if emit_error_envelope(std::io::stdout().lock(), &envelope).is_err() {
            return EXIT_INTERNO;
        }
        return exit_code;
    }
    0
}

/// Envelope di errore §9 su **stdout**, con `stderr` lasciato vuoto.
///
/// **Inversione dichiarata rispetto alla scelta precedente**, che teneva
/// l'envelope su stderr come «contratto pubblico storico».
/// Quella decisione precede l'esistenza di `plenora-database-tools`, che
/// emette gli errori su stdout e lascia stderr vuoto: due componenti della
/// stessa famiglia, orchestrati dallo stesso codice, non possono avere due
/// convenzioni opposte su dove cercare un errore. La rottura per chi oggi
/// parsa stderr e' registrata in `docs/release.md`.
fn emit_error_envelope(
    mut stdout: impl Write,
    envelope: &serde_json::Value,
) -> std::io::Result<()> {
    writeln!(stdout, "{envelope}")
}

/// Difetto interno (convenzione `sysexits`: `EX_SOFTWARE`).
const EXIT_INTERNO: i32 = 70;

/// Exit code stabile derivato dalla CATEGORIA dell'envelope.
///
/// La categoria resta la fonte di verita': il codice e' una sua proiezione
/// grossolana, per gli script che non vogliono parsare JSON. Il mapping e'
/// totale su `plenora_core::ErrorCategory` — una categoria nuova che finisse
/// qui senza un codice sarebbe un errore silenzioso, quindi il caso di
/// default e' `70` e un test copre l'intero enum.
///
/// **Non e' allineato a `plenora-database-tools`**, che restituisce `1` per
/// qualunque errore: e' una divergenza dichiarata (cli.md#exit-code).
/// L'unica garanzia condivisa dalla famiglia e' «0 successo, non-zero
/// errore»; chi scrive codice portabile fra i due componenti legge
/// `error.category`, non questo numero.
///
/// | codice | significato |
/// |---|---|
/// | 0 | successo |
/// | 2 | piano o configurazione invalidi |
/// | 3 | contratto, schema o capability incompatibili |
/// | 4 | limite di risorsa superato |
/// | 5 | I/O, pubblicazione, rete o autorizzazioni |
/// | 6 | fallimento di esecuzione di un nodo |
/// | 70 | difetto interno |
/// | 130 | cancellato (128 + SIGINT) |
fn error_exit_code(envelope: &serde_json::Value) -> i32 {
    match envelope["error"]["category"].as_str() {
        Some("cancelled") => EXIT_CANCELLED,
        Some("invalid_plan" | "invalid_configuration") => 2,
        Some("schema" | "data_mapping" | "crs" | "unsupported") => 3,
        Some("resource_limit") => 4,
        Some(
            "io" | "not_found" | "conflict" | "protocol" | "authentication" | "authorization"
            | "timeout" | "transient",
        ) => 5,
        Some("execution") => 6,
        _ => EXIT_INTERNO,
    }
}

/// Envelope d'errore a quattro assi (R9.1, `protocol_version` 1): l'uscita
/// CLI riporta categoria, fase, effetto remoto e disposizione di retry
/// espliciti — mai dedotti dal messaggio (R9.2). Una riga JSON su stdout;
/// `message` porta il testo dell'errore invariato. `retry` e' nella forma
/// taggata condivisa (conformance/components.json): `{"kind": ...}` piu'
/// `delay_ms` solo per `after(durata)`. `context` (presente
/// solo per errori nati in un'esecuzione DAG) riporta nodo, operazione ed
/// `execution_id` — la risposta a «quale step ha rotto» senza parsare il
/// messaggio. L'exit code e' la proiezione della categoria
/// ([`error_exit_code`]): 2, 3, 4, 5, 6, 70, piu' 130 per la cancellazione.
///
/// Mapping dichiarato: `PlenoraError` -> i quattro assi del tipo; errori
/// di parametro pubblico del trasporto Arrow -> `invalid_plan`/`validate`/
/// `none`/`never`; errori I/O nudi (lettura piano/argomenti) ->
/// `io`/`read`/`none`/`safe`; errori di parse JSON del piano ->
/// `data_mapping`/`validate`/`none`/`never`; qualunque altro tipo ->
/// `internal`/`validate`/`none`/`never`.
fn error_envelope(error: &(dyn Error + 'static), cancelled: bool) -> serde_json::Value {
    let plenora_error = error.downcast_ref::<PlenoraError>();
    let public_transport_parameter_error =
        error
            .downcast_ref::<ArrowTransportError>()
            .is_some_and(|transport| {
                matches!(
                    transport.source_error(),
                    ArrowTransportError::MissingParameter { .. }
                        | ArrowTransportError::UnexpectedParameter { .. }
                        | ArrowTransportError::InvalidParameter { .. }
                )
            });
    let (category, phase, remote_effect, disposition) = plenora_error.map_or_else(
        || {
            if public_transport_parameter_error {
                ("invalid_plan", "validate", "none", RetryDisposition::Never)
            } else if error.downcast_ref::<std::io::Error>().is_some() {
                ("io", "read", "none", RetryDisposition::Safe)
            } else if error.downcast_ref::<serde_json::Error>().is_some() {
                ("data_mapping", "validate", "none", RetryDisposition::Never)
            } else {
                ("internal", "validate", "none", RetryDisposition::Never)
            }
        },
        |plenora| {
            (
                plenora.category().as_str(),
                plenora.phase().as_str(),
                plenora.remote_effect().as_str(),
                plenora.retry_disposition(),
            )
        },
    );
    // Forma taggata fissata in conformance/components.json
    // (required_capability_shared): {"kind": ...} e, solo per
    // `after(durata)`, "delay_ms" — altrimenti il chiamante saprebbe DI
    // riprovare piu' tardi senza sapere QUANDO (R9.2/R9.7).
    let mut retry = serde_json::json!({ "kind": disposition.as_str() });
    if let Some(delay) = disposition.delay() {
        retry["delay_ms"] =
            serde_json::Value::from(u64::try_from(delay.as_millis()).map_or(u64::MAX, |v| v));
    }
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
    if let Some((node, operation, execution_id)) =
        plenora_error.and_then(PlenoraError::execution_location)
    {
        let mut context = serde_json::json!({ "node": node, "operation": operation });
        if let Some(execution_id) = execution_id {
            context["execution_id"] = serde_json::Value::String(execution_id.to_owned());
        }
        body["context"] = context;
    }
    if let Some(diagnostics) = plenora_error.and_then(PlenoraError::row_diagnostics) {
        if let Ok(value) = serde_json::to_value(diagnostics) {
            body["row_diagnostics"] = value;
        } else {
            body["category"] = serde_json::Value::String("internal".to_owned());
            body["phase"] = serde_json::Value::String("write".to_owned());
            body["remote_effect"] = serde_json::Value::String("none".to_owned());
            body["retry"] = serde_json::json!({ "kind": "never" });
            body["message"] =
                serde_json::Value::String("row diagnostics interne non valide".to_owned());
        }
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
        PLENORA_GEOMETRY_DIMENSIONS_KEY, PLENORA_GEOMETRY_ENCODING_KEY, PLENORA_GEOMETRY_SRID_KEY,
        PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, PLENORA_GEOMETRY_TYPES_KEY,
    };

    #[test]
    fn error_envelope_is_capturable_on_declared_stdout_channel() {
        let envelope = error_envelope(&PlenoraError::InvalidPlan("piano invalido".into()), false);
        let mut stdout = Vec::new();
        emit_error_envelope(&mut stdout, &envelope).expect("stdout catturabile");
        let decoded: serde_json::Value = serde_json::from_slice(&stdout).expect("envelope JSON");
        assert_eq!(decoded, envelope);
    }

    /// Ogni categoria di `plenora_core::ErrorCategory` ha un exit code, e i
    /// codici sono quelli dichiarati: la tabella e' un contratto, non una
    /// tendenza. Il caso di default e' `70`, quindi una categoria nuova non
    /// resta senza codice in silenzio.
    #[test]
    fn un_errore_interno_di_passo_esce_come_interno_non_come_esecuzione() {
        // Settimo giro, finding 3. I due propagatori aggiungono il contesto
        // del passo avvolgendo l'errore in `Replayed`, che porta con se' la
        // categoria. Questo test chiude l'ULTIMO anello della catena: che una
        // categoria `Internal` arrivata fin qui dentro un `Replayed` diventi
        // `internal`/exit 70 e non `execution`/exit 6.
        //
        // Gli anelli precedenti sono verificati altrove: la scelta di quali
        // categorie preservare sta in `plenora_engine::error_propagation`, con
        // il proprio test, e i due propagatori usano quel predicato invece di
        // due elenchi scritti a mano. Non esiste un test end-to-end da un
        // piano perche' nessun `Internal` dei kernel e' raggiungibile da un
        // piano valido — sono rami difensivi e invarianti di file temporanei
        // — quindi la catena e' verificata a segmenti, e questo e' l'ultimo.
        let replayed = PlenoraError::Replayed(Box::new(plenora_core::error::ReplayedError {
            category: plenora_core::ErrorCategory::Internal,
            phase: ErrorPhase::Write,
            remote_effect: plenora_core::RemoteEffect::None,
            retry: plenora_core::RetryDisposition::Never,
            message: "invariante nostra violata al passo".into(),
            node: Some("n".into()),
            operation: Some("table.sort".into()),
            execution_id: None,
            execution_reason: None,
        }));
        let envelope = error_envelope(&replayed, false);
        assert_eq!(
            envelope["error"]["category"], "internal",
            "la categoria non si perde nell'involucro: {envelope}"
        );
        assert_eq!(
            error_exit_code(&envelope),
            EXIT_INTERNO,
            "e proietta su 70, non su 6: {envelope}"
        );
        // Il contesto del passo resta: preservare la categoria non deve
        // costare l'attribuzione.
        let testo = envelope.to_string();
        assert!(
            testo.contains("table.sort"),
            "nodo e operazione restano nella diagnostica: {envelope}"
        );
    }

    #[test]
    fn ogni_categoria_ha_l_exit_code_dichiarato() {
        let atteso: [(&str, i32); 18] = [
            ("invalid_plan", 2),
            ("invalid_configuration", 2),
            ("schema", 3),
            ("data_mapping", 3),
            ("crs", 3),
            ("unsupported", 3),
            ("resource_limit", 4),
            ("io", 5),
            ("not_found", 5),
            ("conflict", 5),
            ("protocol", 5),
            ("authentication", 5),
            ("authorization", 5),
            ("timeout", 5),
            ("transient", 5),
            ("execution", 6),
            ("internal", 70),
            ("cancelled", 130),
        ];
        for (categoria, codice) in atteso {
            let envelope = serde_json::json!({"error": {"category": categoria}});
            assert_eq!(
                error_exit_code(&envelope),
                codice,
                "categoria `{categoria}`"
            );
        }
        // Una categoria sconosciuta non passa per «successo».
        let ignota = serde_json::json!({"error": {"category": "categoria-nuova"}});
        assert_eq!(error_exit_code(&ignota), EXIT_INTERNO);
    }

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
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
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
    fn error_envelope_omits_context_for_non_execution_and_omits_empty_execution_id() {
        let contract = PlenoraError::Unsupported("operazione sconosciuta".to_owned());
        let envelope = error_envelope(&contract, false);
        assert_eq!(envelope["error"]["category"], "unsupported");
        assert_eq!(envelope["error"]["phase"], "validate");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
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
    fn legacy_date32_coerce_requires_dag_v4() {
        let plan = serde_json::json!({
            "schema_version": 1,
            "steps": [{
                "operation": "type_cast",
                "config": {"column": "effective_date", "target_type": "date32", "errors": "coerce"}
            }]
        });
        let error = reject_legacy_row_diagnostics_plan(&plan.to_string())
            .expect_err("legacy non può dichiarare completezza cross-batch");
        assert!(matches!(error, PlenoraError::Unsupported(_)));
    }

    #[test]
    fn every_catalog_row_diagnostic_operation_requires_dag_v4_in_legacy_plans() {
        // Catalog-driven (anti-drift): l'universo delle op arriva dal
        // catalogo, NON da una lista duplicata nel test. Per ogni
        // (descrittore, config) che l'autorita' dichiara diagnostica, ogni
        // nome risolvibile (id canonico + alias legacy) in un piano
        // sort -> op deve essere rifiutato DAL GATE (errore Unsupported, mai
        // da una validazione incidentale).
        let mut probes = vec![serde_json::json!({})];
        for target in [
            "int",
            "float",
            "bool",
            "uint64",
            "date",
            "datetime",
            "date32",
            "timestamp_millis",
            "decimal128",
            "str",
        ] {
            probes.push(serde_json::json!({"target_type": target}));
            probes.push(serde_json::json!({"target_type": target, "errors": "coerce"}));
            probes.push(serde_json::json!({"target_type": target, "errors": "raise"}));
        }
        for policy in ["error", "empty", "literal"] {
            probes.push(serde_json::json!({"null_policy": policy}));
        }
        let mut gated = std::collections::BTreeSet::new();
        for descriptor in plenora_core::catalog::CATALOG {
            let mut names = vec![descriptor.id.to_owned()];
            names.extend(
                plenora_core::catalog::ALIASES
                    .iter()
                    .filter(|(_, _, canonical)| *canonical == descriptor.id)
                    .map(|(_, alias, _)| (*alias).to_owned()),
            );
            for config in &probes {
                if !descriptor.emits_row_diagnostics(config) {
                    continue;
                }
                for name in &names {
                    let plan = serde_json::json!({
                        "schema_version": 1,
                        "steps": [
                            {"operation": "sort", "config": {"columns": ["id"]}},
                            {"operation": name, "config": config}
                        ]
                    });
                    let error = reject_legacy_row_diagnostics_plan(&plan.to_string())
                        .expect_err("op diagnostica legacy non bloccata dal gate");
                    assert!(
                        matches!(error, PlenoraError::Unsupported(_)),
                        "{} (nome `{name}`, config {config}): rifiuto atteso dal gate \
                         (Unsupported), ottenuto {error:?}",
                        descriptor.id
                    );
                    gated.insert(descriptor.id);
                }
            }
        }
        // Lock espliciti del perimetro (formula ed expression erano il bypass;
        // md5/sha256 con null_policy=error; type_cast fallibile).
        for expected in [
            "table.formula",
            "table.expression",
            "table.type_cast",
            "table.md5_hash",
            "table.sha256_hash",
            "table.flatten_json",
            "table.assert_not_null",
            "geo.from_wkt",
            "geo.centroid",
        ] {
            assert!(
                gated.contains(expected),
                "{expected}: op diagnostica non coperta dal gate legacy"
            );
        }
    }

    #[test]
    fn legacy_gate_passes_operations_that_do_not_emit_row_diagnostics() {
        // md5/sha256 senza null_policy=error, type_cast verso `str`
        // e hmac_sha256 non emettono diagnostica row-scoped: il gate li
        // lascia passare (resta la validazione legacy, qui con config
        // valide). Lock anti-regressione sull'autorita' config-sensitive.
        for plan in [
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "md5_hash", "config": {"columns": ["id"]}}
            ]}),
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "md5_hash", "config": {"columns": ["id"], "null_policy": "empty"}}
            ]}),
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "sha256_hash", "config": {
                    "columns": ["id"], "null_policy": "literal", "null_literal": "<null>"
                }}
            ]}),
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "type_cast", "config": {
                    "column": "id", "target_type": "str", "date_format": "", "errors": "coerce"
                }}
            ]}),
            serde_json::json!({"schema_version": 1, "steps": [
                {"operation": "table.hmac_sha256", "config": {
                    "columns": ["id"], "key_env": "PLENORA_GATE_TEST_KEY"
                }}
            ]}),
        ] {
            reject_legacy_row_diagnostics_plan(&plan.to_string())
                .expect("op non diagnostica bloccata dal gate legacy");
        }
    }

    #[test]
    fn legacy_diagnostic_plans_are_rejected_even_when_blocking() {
        // Nel percorso legacy non esiste gate provenance (solo DAG):
        // qualsiasi op row-diagnostics, anche come primo step o dopo op
        // blocking row-preserving, richiede DAG — la materializzazione
        // completa non attesta la provenance `source_row_zero_based`.
        for plan in [
            serde_json::json!({
                "schema_version": 1,
                "steps": [{
                    "operation": "assert_unique",
                    "config": {"columns": ["id"], "nulls_equal": true}
                }]
            }),
            serde_json::json!({
                "schema_version": 1,
                "steps": [{
                    "operation": "assert_foreign_key",
                    "config": {"left_keys": ["id"], "right_keys": ["id"]}
                }]
            }),
            serde_json::json!({
                "schema_version": 1,
                "steps": [
                    {"operation": "bin", "config": {"column": "value", "bins": 2}},
                    {"operation": "assert_regex", "config": {"column": "name", "pattern": ".*"}}
                ]
            }),
            serde_json::json!({
                "schema_version": 1,
                "steps": [
                    {"operation": "add_row_number", "config": {}},
                    {"operation": "type_cast", "config": {"column": "value", "target_type": "date32"}}
                ]
            }),
        ] {
            let error = reject_legacy_row_diagnostics_plan(&plan.to_string())
                .expect_err("piano legacy diagnostico: provenance non attestabile");
            assert!(matches!(error, PlenoraError::Unsupported(_)));
        }
    }

    #[test]
    fn error_envelope_preserves_optional_row_diagnostics() {
        let mut counts = std::collections::BTreeMap::new();
        counts.insert("conversion.invalid_date".to_owned(), 1);
        let report = plenora_core::diagnostics::RowDiagnostics {
            contract: plenora_core::diagnostics::ROW_DIAGNOSTICS_CONTRACT.to_owned(),
            scope: plenora_core::diagnostics::RowDiagnosticScope::Read,
            index_basis: plenora_core::diagnostics::ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
            completeness: plenora_core::diagnostics::RowDiagnosticsCompleteness::Complete,
            knowledge_limits: None,
            total: Some(1),
            observed_total: 1,
            counts,
            examples_limit: 10,
            examples_truncated: false,
            examples: vec![plenora_core::diagnostics::RowDiagnosticExample {
                source_index: 4,
                cause: "conversion.invalid_date".to_owned(),
                column: Some("effective_date".to_owned()),
                key: None,
                write_state: None,
            }],
            input_total: None,
            diagnostic_state_counts: None,
            write_outcome: None,
        };
        let error = PlenoraError::DataMapping("conversione data rifiutata".to_owned())
            .with_phase(ErrorPhase::Read)
            .with_row_diagnostics(report.clone());
        let envelope = error_envelope(&error, false);

        assert_eq!(
            envelope["error"]["row_diagnostics"],
            serde_json::to_value(&report).expect("serializzazione report")
        );
        assert_eq!(envelope["error"]["category"], "data_mapping");
        assert_eq!(envelope["error"]["phase"], "read");

        let ordinary = error_envelope(
            &PlenoraError::DataMapping("errore ordinario".to_owned()),
            false,
        );
        assert!(ordinary["error"].get("row_diagnostics").is_none());

        let mut invalid = report;
        invalid.examples_limit = 0;
        let direct = PlenoraError::RowDiagnostics {
            source: Box::new(PlenoraError::DataMapping("righe non conformi".to_owned())),
            diagnostics: Box::new(invalid.clone()),
        };
        let fail_closed = error_envelope(&direct, false);
        assert!(fail_closed["error"].get("row_diagnostics").is_none());
        assert_eq!(fail_closed["error"]["category"], "internal");
        assert_eq!(
            fail_closed["error"]["message"],
            "row diagnostics interne non valide"
        );

        let cancelled_direct = PlenoraError::RowDiagnostics {
            source: Box::new(PlenoraError::Cancelled {
                node: "cast".to_owned(),
                operation: "table.type_cast".to_owned(),
                execution_id: "exec-invalid".to_owned(),
                reason: "cancellazione richiesta".to_owned(),
            }),
            diagnostics: Box::new(invalid.clone()),
        };
        let cancelled_fail_closed = error_envelope(&cancelled_direct, true);
        assert_eq!(cancelled_fail_closed["error"]["category"], "internal");
        // Il declassamento fail-closed cambia la categoria in `internal`, e
        // con essa il codice: 70, non 130. Il codice segue la categoria
        // pubblicata, mai l'intenzione originale.
        assert_eq!(error_exit_code(&cancelled_fail_closed), 70);

        let rejected = PlenoraError::DataMapping("righe non conformi".to_owned())
            .with_row_diagnostics(invalid);
        assert_eq!(rejected.category(), plenora_core::ErrorCategory::Internal);
        assert!(rejected.row_diagnostics().is_none());
    }

    #[test]
    fn error_envelope_maps_io_json_and_unknown_errors() {
        let io = std::io::Error::other("disco pieno");
        let envelope = error_envelope(&io, false);
        assert_eq!(envelope["error"]["category"], "io");
        assert_eq!(envelope["error"]["phase"], "read");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "safe"})
        );

        let json = serde_json::from_str::<u32>("\"non-un-numero\"").expect_err("json invalido");
        let envelope = error_envelope(&json, false);
        assert_eq!(envelope["error"]["category"], "data_mapping");
        assert_eq!(envelope["error"]["phase"], "validate");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
    }

    #[test]
    fn legacy_execution_without_id_keeps_node_and_operation_context() {
        let error = PlenoraError::Execution {
            node: "0".to_owned(),
            operation: "table.transpose".to_owned(),
            execution_id: String::new(),
            reason: "transpose supera i limiti".to_owned(),
        };

        let envelope = error_envelope(&error, false);

        assert_eq!(envelope["error"]["context"]["node"], "0");
        assert_eq!(envelope["error"]["context"]["operation"], "table.transpose");
        assert!(envelope["error"]["context"].get("execution_id").is_none());
    }

    #[test]
    fn pair_arrow_invalid_public_parameter_is_invalid_plan() {
        let error =
            plenora_engine::geo_transport::transport::ArrowTransportError::InvalidParameter {
                operation: "haversine_distance",
                name: "max_output_rows",
                reason: "oltre il limite righe del trasporto",
            };

        let envelope = error_envelope(&error, false);

        assert_eq!(envelope["error"]["category"], "invalid_plan");
        assert_eq!(envelope["error"]["phase"], "validate");
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
    }

    #[test]
    fn error_envelope_retry_after_carries_delay_ms() {
        // La forma taggata di conformance/components.json: senza delay_ms
        // il chiamante saprebbe DI riprovare piu' tardi, non QUANDO.
        let retry = RetryDisposition::After(std::time::Duration::from_millis(5000));
        let mut serialized = serde_json::json!({ "kind": retry.as_str() });
        if let Some(delay) = retry.delay() {
            serialized["delay_ms"] =
                serde_json::Value::from(u64::try_from(delay.as_millis()).map_or(u64::MAX, |v| v));
        }
        assert_eq!(
            serialized,
            serde_json::json!({"kind": "after", "delay_ms": 5000})
        );
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
        assert_eq!(error_exit_code(&envelope), EXIT_CANCELLED);
        assert_eq!(
            envelope["error"]["retry"],
            serde_json::json!({"kind": "never"})
        );
        assert!(
            envelope["error"]["message"]
                .as_str()
                .expect("message")
                .starts_with("esecuzione annullata: "),
            "messaggio dedicato preservato: {envelope}"
        );
        assert_eq!(envelope["error"]["context"]["execution_id"], "exec-9");

        let wrapped = error.with_row_diagnostics(plenora_core::diagnostics::RowDiagnostics {
            contract: plenora_core::diagnostics::ROW_DIAGNOSTICS_CONTRACT.to_owned(),
            scope: plenora_core::diagnostics::RowDiagnosticScope::Read,
            index_basis: plenora_core::diagnostics::ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
            completeness: plenora_core::diagnostics::RowDiagnosticsCompleteness::Partial,
            observed_total: 1,
            total: None,
            input_total: None,
            counts: std::collections::BTreeMap::from([("conversion.invalid_date".to_owned(), 1)]),
            examples_limit: 10,
            examples_truncated: false,
            examples: vec![plenora_core::diagnostics::RowDiagnosticExample {
                source_index: 4,
                cause: "conversion.invalid_date".to_owned(),
                column: Some("effective_date".to_owned()),
                key: None,
                write_state: None,
            }],
            knowledge_limits: Some(vec!["data_tools.processing_interrupted".to_owned()]),
            diagnostic_state_counts: None,
            write_outcome: None,
        });
        assert!(wrapped.is_cancelled(), "exit code deve restare 130");
        let wrapped_envelope = error_envelope(&wrapped, wrapped.is_cancelled());
        assert_eq!(wrapped_envelope["error"]["category"], "cancelled");
        assert_eq!(error_exit_code(&wrapped_envelope), EXIT_CANCELLED);
        assert_eq!(
            wrapped_envelope["error"]["context"]["execution_id"],
            "exec-9"
        );
        assert!(wrapped_envelope["error"].get("row_diagnostics").is_some());
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
        let written =
            plenora_kernels_geo::arrow_adapter::geometry_output_field("geometry", "EPSG:32632")
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
        let ContractCrs::DeclaredUnresolved {
            crs_id, definition, ..
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
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
        // con la dichiarazione originale.
        // La fixture omette `crs_resolution`; lo stesso conflitto resta
        // preservato anche quando il produttore dichiara `resolved` (test
        // successivo): una dichiarazione non puo' nascondere H-06.
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lon_lat"),
            (PLENORA_GEOMETRY_SRID_KEY, "3003"),
        ]);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id, definition, ..
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(definition, &None);
    }

    #[test]
    fn discovery_declared_resolved_with_conflicting_crs_id_and_srid_stays_unresolved() {
        // Una dichiarazione `resolved` non puo' nascondere un conflitto
        // numerico decidibile fra identificatore e SRID (H-06/R4.1).
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "lon_lat"),
            (PLENORA_GEOMETRY_SRID_KEY, "3003"),
        ]);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id, definition, ..
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
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
        // Emendamento 2026-07-31 (classe A): la regola (2a) vale SOLO per
        // input NON dichiarati — la fixture non porta `crs_resolution`
        // (prima la portava `resolved`: il rovesciamento della
        // dichiarazione esplicita era il bug del caso owner).
        let field = canonical_crs_field(&[
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (
                PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
                r#"{"type":"GeographicCRS"}"#,
            ),
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
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(definition.as_deref(), Some(r#"{"type":"GeographicCRS"}"#));
        assert_eq!(
            definition_format.map(plenora_core::contract::CrsDefinitionFormat::as_str),
            Some("projjson")
        );
    }

    // -------------------------------------------------------------------
    // Emendamento 2026-07-31 (classe A): `resolved` dichiarato con doppia
    // rappresentazione — risoluzione + verifica di coerenza decidibile.
    // -------------------------------------------------------------------

    /// WKT1 realistico di Monte Mario / Italy zone 1 con `AUTHORITY` e
    /// `TOWGS84` (EPSG:3003): la forma dello shapefile catastale owner.
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

    /// Coppie canoniche del caso owner: `resolved` dichiarato, doppia
    /// rappresentazione (`crs_id` + definizione WKT) con formato `wkt`.
    fn monte_mario_resolved_pairs(crs_id: &str) -> Vec<(&'static str, String)> {
        vec![
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "resolved".to_owned()),
            (PLENORA_GEOMETRY_CRS_ID_KEY, crs_id.to_owned()),
            (
                PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
                MONTE_MARIO_WKT.to_owned(),
            ),
            (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt".to_owned()),
            (
                PLENORA_GEOMETRY_AXIS_ORDER_KEY,
                "easting_northing".to_owned(),
            ),
        ]
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn discovery_resolved_with_coherent_wkt_resolves_with_authority_srid() {
        // Il caso owner: `resolved` + crs_id=EPSG:3003 + WKT Monte Mario
        // coerente. La (2a) NON rovescia la dichiarazione: il WKT risolve
        // contro PROJ e la verifica di coerenza (crs_id 3003 == srid del
        // canonical) conferma — `Resolved`, con `authority_srid` 3003.
        let pairs = monte_mario_resolved_pairs("EPSG:3003");
        let pairs_ref: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let field = canonical_crs_field(&pairs_ref);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        let ContractCrs::Resolved(resolved) = &contract.geometries[0].crs else {
            panic!("atteso Resolved: {:?}", contract.geometries[0].crs);
        };
        assert_eq!(resolved.authority_srid(), Some(3003));
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn discovery_resolved_with_divergent_crs_id_becomes_declared_unresolved() {
        // Stessa fixture ma crs_id=EPSG:4326: il WKT risolve a 3003, il
        // confronto decidibile smentisce il `resolved` dichiarato —
        // `DeclaredUnresolved` con le dichiarazioni ORIGINALI preservate
        // (non passa e nulla si perde).
        let pairs = monte_mario_resolved_pairs("EPSG:4326");
        let pairs_ref: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let field = canonical_crs_field(&pairs_ref);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        let ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format,
        } = &contract.geometries[0].crs
        else {
            panic!(
                "atteso DeclaredUnresolved: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:4326"));
        assert_eq!(definition.as_deref(), Some(MONTE_MARIO_WKT));
        assert_eq!(
            definition_format.map(plenora_core::contract::CrsDefinitionFormat::as_str),
            Some("wkt")
        );
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn discovery_resolved_with_same_code_but_different_authority_stays_unresolved() {
        let pairs = monte_mario_resolved_pairs("FOO:3003");
        let pairs_ref: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let field = canonical_crs_field(&pairs_ref);
        let contract =
            discover_input_contract_from_schema(schema_v1(vec![field])).expect("discovery");
        assert!(
            matches!(
                contract.geometries[0].crs,
                ContractCrs::DeclaredUnresolved { .. }
            ),
            "un'autorita' diversa non puo' essere certificata dal solo codice numerico"
        );
    }

    #[cfg(not(feature = "proj-backend"))]
    #[test]
    fn discovery_resolved_with_double_representation_needs_the_backend() {
        // Effetto collaterale DICHIARATO dell'emendamento 2026-07-31
        // (classe A): senza `proj-backend` un input `resolved` con doppia
        // rappresentazione prima passava come `DeclaredUnresolved` (la (2a)
        // scattava senza backend); ora la dichiarazione si onora con la
        // regola (3) e la risoluzione impossibile fallisce con errore
        // `Crs` — coerente col `resolved` a rappresentazione singola.
        let pairs = monte_mario_resolved_pairs("EPSG:3003");
        let pairs_ref: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let field = canonical_crs_field(&pairs_ref);
        let result = discover_input_contract_from_schema(schema_v1(vec![field]));
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "atteso errore Crs senza backend: {result:?}"
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
        if let ContractCrs::DeclaredUnresolved {
            crs_id: Some(id), ..
        } = &crs
        {
            metadata.insert(PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(), id.clone());
            metadata.insert(
                PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                "unknown".to_owned(),
            );
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
        let mut contracts = vec![(
            "main".to_owned(),
            contract_with_crs_state(declared_unresolved_state()),
        )];
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
        for state in [ContractCrs::Missing, ContractCrs::Resolved(projected_crs())] {
            let mut contracts = vec![("main".to_owned(), contract_with_crs_state(state))];
            let error = apply_crs_decisions(&decisions_probe("EPSG:32632"), &mut contracts)
                .expect_err("stato non decidibile");
            assert!(error.to_string().contains("non e' applicabile"), "{error}");
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
        let mut contracts = vec![(
            "main".to_owned(),
            contract_with_crs_state(declared_unresolved_state()),
        )];
        apply_crs_decisions(&decisions_probe("EPSG:32632"), &mut contracts).expect("decisione");
        let contract = &contracts[0].1;
        let ContractCrs::ResolvedByDecision(crs) = &contract.geometries[0].crs else {
            panic!(
                "atteso ResolvedByDecision: {:?}",
                contract.geometries[0].crs
            );
        };
        assert_eq!(crs.definition(), "EPSG:32632");
        assert_eq!(
            contract.geometries[0].crs.resolution(),
            CrsResolution::Resolved
        );
        let metadata = contract
            .schema
            .field_with_name("geometry")
            .expect("campo")
            .metadata();
        assert_eq!(
            metadata
                .get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
                .map(String::as_str),
            Some("declared_unresolved"),
            "lo schema di input resta quello scoperto"
        );
    }

    // -------------------------------------------------------------------
    // Tagging di fase al confine di lettura (BLOCK-03, piano-v5.md#contratti-di-input)
    // -------------------------------------------------------------------

    #[test]
    fn ipc_probes_tag_read_errors_at_the_input_boundary() {
        // File assente: Io dello sniffing -> fase Read; testo invariato.
        let missing = Path::new("input-che-non-esiste.arrow");
        let error = ipc_boundary::sniff_format(missing).expect_err("file assente");
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
        // Diciannove byte di spazzatura: il file e' ROTTO, non troppo
        // grande. La lunghezza dichiarata dai primi quattro byte non e'
        // contenuta nel file, quindi il confine la tratta come troncamento
        // (`data_mapping`) e non come tetto superato (`resource_limit`).
        let senza_tag = error.untag();
        assert!(
            matches!(senza_tag, PlenoraError::DataMapping(_)),
            "{senza_tag:?}"
        );
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
    fn every_declared_subcommand_accepts_help() {
        for command in [
            "catalog",
            "validate",
            "run",
            "capabilities",
            "transform",
            "spatial-join",
            "transform-arrow",
            "pair-arrow",
            "self-test",
        ] {
            let args = vec![command.to_owned(), "--help".to_owned()];
            assert!(
                run_with_args(&args).is_ok(),
                "{command} --help deve terminare con successo"
            );
        }
    }

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
        assert_eq!(
            argument_value(&args, "--schema").expect("presente"),
            "s.json"
        );
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
        let io = at_input(
            "main",
            path,
            PlenoraError::Io(std::io::Error::other("disco")),
        );
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
        let ContractCrs::DeclaredUnresolved {
            crs_id, definition, ..
        } = contract_crs_from_keys("geometry", None, &keys).expect("stato")
        else {
            panic!("atteso DeclaredUnresolved");
        };
        assert_eq!(crs_id.as_deref(), Some("EPSG:32632"));
        assert_eq!(definition.as_deref(), Some(r#"{"type":"ProjectedCRS"}"#));
    }

    #[test]
    fn contract_crs_from_keys_srid_only_declared_unresolved_is_a_representation() {
        // Catena MySQL TLS Database→Data: il provider conosce lo SRID
        // numerico dal catalogo ma non puo' inventare l'autorita' (R4.4) —
        // dichiara `declared_unresolved` con SOLO `srid`, senza
        // `crs_id`/`crs_definition`. R4.3.1: lo SRID numerico e' la terza
        // rappresentazione CRS (dopo definizione e identificatore), quindi
        // la dichiarazione NON e' la contraddizione R4.1 — lo stato e'
        // `DeclaredUnresolved` con crs_id/definition/format ASSENTI (mai
        // sintetizzati); lo SRID resta custodito dallo schema Arrow
        // originale (il contratto non lo modella).
        let keys = CanonicalGeometryKeys {
            srid: Some(4326),
            crs_resolution: Some(CrsResolution::DeclaredUnresolved),
            ..CanonicalGeometryKeys::default()
        };
        let ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format,
        } = contract_crs_from_keys("geometry", None, &keys).expect("stato")
        else {
            panic!("atteso DeclaredUnresolved");
        };
        assert_eq!(crs_id, None, "crs_id mai sintetizzato");
        assert_eq!(definition, None, "definizione mai sintetizzata");
        assert_eq!(definition_format, None, "formato mai sintetizzato");
    }

    #[test]
    fn contract_crs_from_keys_declared_unresolved_without_any_representation_is_an_error() {
        // Fail-closed (R4.1): `declared_unresolved` senza crs_id, definition
        // E srid resta una contraddizione — errore esplicito, mai collasso
        // su `missing`.
        let keys = CanonicalGeometryKeys {
            crs_resolution: Some(CrsResolution::DeclaredUnresolved),
            ..CanonicalGeometryKeys::default()
        };
        let result = contract_crs_from_keys("geometry", None, &keys);
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(
                    message.contains("nessun CRS e' dichiarato in alcuna rappresentazione"),
                    "{message}"
                );
            }
            other => panic!("attesa contraddizione R4.1, ottenuto {other:?}"),
        }
    }

    #[test]
    fn contract_crs_from_keys_resolved_with_srid_only_is_never_promoted() {
        // Fail-closed: lo SRID numerico da solo non identifica un'autorita'
        // risolvibile e il centro non la inventa (R4.4) — un `resolved`
        // dichiarato con SOLO `srid` non e' promosso ne' risolto
        // implicitamente: resta la contraddizione R4.1 di sempre (errore).
        let keys = CanonicalGeometryKeys {
            srid: Some(4326),
            crs_resolution: Some(CrsResolution::Resolved),
            ..CanonicalGeometryKeys::default()
        };
        let result = contract_crs_from_keys("geometry", None, &keys);
        assert!(
            matches!(result, Err(PlenoraError::InvalidPlan(_))),
            "`resolved` srid-only non promosso: {result:?}"
        );
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
        assert!(matches!(result, Ok(ContractCrs::Resolved(_))), "{result:?}");
        #[cfg(not(feature = "proj-backend"))]
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
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
        let result = discover_input_contract_from_schema(std::sync::Arc::new(Schema::new(vec![
            unknown_extension,
        ])));
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
        assert_eq!(
            ipc_boundary::sniff_format(&short).expect("sniffing"),
            IpcFormat::Stream
        );
        // Sei byte ma magic diverso: non e' IPC file format.
        let other = directory.path().join("other.bin");
        std::fs::write(&other, b"ARROW2").expect("fixture");
        assert_eq!(
            ipc_boundary::sniff_format(&other).expect("sniffing"),
            IpcFormat::Stream
        );
    }

    #[test]
    fn open_input_accepts_file_and_stream_ipc_framings() {
        let directory = tempfile::tempdir().expect("tempdir");
        let schema: SchemaRef =
            std::sync::Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            std::sync::Arc::clone(&schema),
            vec![std::sync::Arc::new(
                plenora_core::arrow::array::Int64Array::from(vec![1, 2]),
            )],
        )
        .expect("batch");
        // IPC file format.
        let file_path = directory.path().join("in.arrow");
        let mut writer = FileWriter::try_new(File::create(&file_path).expect("create"), &schema)
            .expect("writer");
        writer.write(&batch).expect("write");
        writer.finish().expect("finish");
        assert_eq!(
            ipc_boundary::sniff_format(&file_path).expect("sniff"),
            IpcFormat::File
        );
        // IPC stream format.
        let stream_path = directory.path().join("in.stream");
        let mut writer = plenora_core::arrow::ipc::writer::StreamWriter::try_new(
            File::create(&stream_path).expect("create"),
            &schema,
        )
        .expect("writer");
        writer.write(&batch).expect("write");
        writer.finish().expect("finish");
        assert_eq!(
            ipc_boundary::sniff_format(&stream_path).expect("sniff"),
            IpcFormat::Stream
        );
        // Entrambi si aprono come input lazy con lo schema dichiarato.
        for path in [&file_path, &stream_path] {
            let input = open_input(path, &IpcLimits::default()).expect("open_input");
            match input {
                Input::Stream {
                    schema: declared, ..
                } => assert_eq!(declared, schema),
                Input::Batches { .. } => panic!("gli input da percorso sono lazy"),
            }
        }
    }

    #[test]
    fn v4_input_paths_combines_single_and_multiple_flags() {
        let argv =
            |args: &[&str]| -> Vec<String> { args.iter().map(ToString::to_string).collect() };
        let posizionali = |args: &[&str]| match v4_inputs(&argv(args)).expect("inputs") {
            V4Inputs::Positional(paths) => paths,
            V4Inputs::Named(_) => panic!("attesa forma posizionale"),
        };
        assert_eq!(
            posizionali(&["run", "--input", "a.arrow"]),
            vec![PathBuf::from("a.arrow")]
        );
        assert_eq!(
            posizionali(&["run", "--inputs", "b.arrow", "c.arrow", "--output", "o.arrow"]),
            vec![PathBuf::from("b.arrow"), PathBuf::from("c.arrow")],
            "i valori si fermano al prossimo flag"
        );
        assert_eq!(
            posizionali(&["run", "--input", "a.arrow", "--inputs", "b.arrow"]).len(),
            2
        );
        assert!(v4_inputs(&argv(&["--input"])).is_err());
    }

    #[test]
    fn la_forma_nominale_lega_ogni_input_al_suo_nome() {
        let argv =
            |args: &[&str]| -> Vec<String> { args.iter().map(ToString::to_string).collect() };
        let inputs = v4_inputs(&argv(&[
            "run",
            "--input",
            "destra=b.arrow",
            "--input",
            "sinistra=a.arrow",
            "--output",
            "o.arrow",
        ]))
        .expect("inputs");
        assert_eq!(
            inputs,
            V4Inputs::Named(vec![
                ("destra".to_owned(), PathBuf::from("b.arrow")),
                ("sinistra".to_owned(), PathBuf::from("a.arrow")),
            ])
        );

        // L'ordine restituito e' quello del PIANO, non della riga di comando:
        // e' il punto del difetto — con la forma posizionale questi due file
        // sarebbero finiti sugli input sbagliati.
        let probe = PlanInputsProbe {
            inputs: vec!["sinistra".to_owned(), "destra".to_owned()],
            crs_decisions: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            pair_v4_inputs(&probe, &inputs).expect("accoppiamento"),
            vec![
                ("sinistra".to_owned(), PathBuf::from("a.arrow")),
                ("destra".to_owned(), PathBuf::from("b.arrow")),
            ]
        );

        // Un nome che il piano non dichiara e' un errore, e il messaggio dice
        // quali sono i nomi buoni.
        let sconosciuto = v4_inputs(&argv(&["run", "--input", "centro=c.arrow"])).expect("inputs");
        let error = pair_v4_inputs(&probe, &sconosciuto).expect_err("nome non dichiarato");
        assert!(
            error.to_string().contains("non dichiarato dal piano"),
            "{error}"
        );
        assert!(error.to_string().contains("sinistra, destra"), "{error}");

        // Un input dichiarato ma non fornito e' un errore, con il rimedio.
        let parziale = v4_inputs(&argv(&["run", "--input", "sinistra=a.arrow"])).expect("inputs");
        let error = pair_v4_inputs(&probe, &parziale).expect_err("input mancante");
        assert!(
            error.to_string().contains("--input destra=PERCORSO"),
            "{error}"
        );

        // Nome ripetuto, percorso vuoto, forme mescolate: tutti errori.
        assert!(v4_inputs(&argv(&[
            "run",
            "--input",
            "sinistra=a.arrow",
            "--input",
            "sinistra=b.arrow"
        ]))
        .is_err());
        assert!(v4_inputs(&argv(&["run", "--input", "sinistra="])).is_err());
        assert!(v4_inputs(&argv(&[
            "run",
            "--input",
            "sinistra=a.arrow",
            "--inputs",
            "b.arrow"
        ]))
        .is_err());
    }

    #[test]
    fn un_percorso_non_e_scambiato_per_una_coppia_nominale() {
        // Un percorso assoluto o relativo non contiene `=` prima di un
        // separatore: resta un percorso. La regola e' quella documentata su
        // `is_named_input`, e vale su entrambi gli stili di separatore.
        assert!(!is_named_input("/dati/x.arrow"));
        assert!(!is_named_input(r"C:\dati\x.arrow"));
        assert!(!is_named_input("x.arrow"));
        assert!(!is_named_input("=x.arrow"));
        assert!(!is_named_input("/dati/a=b.arrow"));
        assert!(is_named_input("main=x.arrow"));
        assert!(is_named_input("main=/dati/x.arrow"));
        assert!(is_named_input(r"main=C:\dati\x.arrow"));
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
        assert!(
            error.to_string().contains("row_count non coerente"),
            "{error}"
        );
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
