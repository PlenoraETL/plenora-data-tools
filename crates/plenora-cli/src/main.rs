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
//! - Fase 2A: collegamento al DAG. `validate` e `run` usano il
//!   planner/executor del DAG (`plenora_engine::planner::validate` +
//!   `plenora_engine::execute`) per tre versioni del formato, che NON
//!   collassano l'una nell'altra:
//!   `schema_version: 5`; `4`, migrato al canonico v5 prima di ogni altra
//!   cosa (piano-v5.md, migrazione), con cui condivide l'identita'; e `6`,
//!   che ha un parser proprio, puo' dichiarare `max_domain_memory_bytes` e
//!   sta nel **proprio** dominio di `plan_hash`. I piani legacy
//!   (`schema_version` <= 3) restano sul `table_engine`, comportamento
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
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use plenora_core::limits::PlanLimits;
use plenora_core::{ErrorPhase, PlenoraError};
use plenora_engine::geo_transport::publish::PublishOutcome;
use plenora_engine::geo_transport::transport::ArrowOutputFormat;
use plenora_engine::plan::{
    migrazione_v4, PLAN_SCHEMA_VERSION_V4, PLAN_SCHEMA_VERSION_V5, PLAN_SCHEMA_VERSION_V6,
};
use plenora_engine::planner::ValidatedGraph;
use plenora_engine::{CancellationToken, ExecutionMetrics, ExecutionPlan};
use serde::Deserialize;

mod cli;

use cli::args::{help_text, reject_unknown_flags, subcommand_help_text};
use cli::commands::catalog::{capabilities_command, catalog_command};
use cli::commands::describe::describe_command;
use cli::commands::legacy::{
    execute_pair_arrow, execute_spatial_join, execute_transform, execute_transform_arrow,
    run_pipeline, self_test_command,
};
use cli::commands::run::{run_command, DagInputs};
use cli::commands::validate::validate_command;
use cli::error_envelope::{emit_error_envelope, error_envelope, EXIT_CANCELLED, EXIT_INTERNO};
use cli::process::{descrivi_panico_locale, esegui_processo};
use cli::rendering::{contract_json, hex_digest, version_json};

// Quello che serve SOLO ai test di questo file, che raggiungono i nomi di
// main.rs con `use super::*`. Tenerli fra gli import normali lascerebbe un
// warning permanente nella build del binario, ed e' cosi' che i warning
// smettono di volere dire qualcosa.
#[cfg(test)]
use cli::commands::legacy::{read_geometry_stream, transform_stream, TransformSchema};
#[cfg(test)]
use cli::commands::run::{is_named_input, reject_legacy_row_diagnostics_plan, v4_inputs};
#[cfg(test)]
use cli::contract_discovery::{
    apply_crs_decisions, at_input, contract_crs_from_keys, crs_definition_from_metadata,
    discover_input_contract_from_schema, geometry_contract_from_field, ipc_header_schema,
    open_input, pair_v4_inputs,
};
#[cfg(test)]
use cli::error_envelope::error_exit_code;
#[cfg(test)]
use plenora_core::arrow::array::RecordBatch;
#[cfg(test)]
use plenora_core::arrow::ipc::writer::FileWriter;
#[cfg(test)]
use plenora_core::arrow::schema::{DataType, SchemaRef};
#[cfg(test)]
use plenora_core::contract::{ContractCrs, DataContract};
#[cfg(test)]
use plenora_core::contract::{
    ContractProperties, CrsResolution, FieldId, GeometryColumnContract, PropertyConfidence,
    PropertyScope,
};
#[cfg(test)]
use plenora_engine::geo_transport::protocol::{Frame, FrameReader, FrameWriter};
#[cfg(test)]
use plenora_engine::{ipc_boundary, IpcFormat};
#[cfg(test)]
use plenora_engine::{Input, IpcLimits};
#[cfg(test)]
use plenora_kernels_geo::arrow_adapter::{
    read_geometry_contract_keys, CanonicalGeometryKeys, GEOARROW_EXTENSION_KEY,
    GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
};
#[cfg(test)]
use plenora_kernels_geo::Operation;

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

// ---------------------------------------------------------------------------
// Helper comuni
// ---------------------------------------------------------------------------

pub(crate) fn contract(message: impl Into<String>) -> PlenoraError {
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
pub(crate) fn limite_risorsa(message: impl Into<String>) -> PlenoraError {
    PlenoraError::ResourceLimit(message.into())
}

/// Formato dell'output dei comandi, scelto dal flag globale `--format`.
///
/// Stessa convenzione di `plenora-database-tools`: il flag e' globale, viene
/// tolto dagli argomenti PRIMA del dispatch e vale per il comando che segue.
/// `junit` non c'e': un formato senza un consumatore e' codice non provato, e
/// qui nessun gate lo legge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
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
pub(crate) fn strip_output_format(args: Vec<String>) -> Result<Vec<String>, PlenoraError> {
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
pub(crate) fn install_ctrlc_handler(token: &CancellationToken) -> Result<(), PlenoraError> {
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
pub(crate) const fn durabilita_confermata(outcome: PublishOutcome) -> bool {
    !matches!(outcome, PublishOutcome::PublishedButDurabilityUnconfirmed)
}

/// Helper stile nogeo: valore obbligatorio dopo un flag.
pub(crate) fn value_after(args: &[String], flag: &str) -> Result<PathBuf, PlenoraError> {
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
pub(crate) fn optional_value_after(
    args: &[String],
    flag: &str,
) -> Result<Option<PathBuf>, PlenoraError> {
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
pub(crate) fn arrow_output_format(args: &[String]) -> Result<ArrowOutputFormat, PlenoraError> {
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

// Catalogo unificato e validate (nuovi comandi di Fase 1)
// ---------------------------------------------------------------------------

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
pub(crate) struct PlanInputsProbe {
    #[serde(default)]
    inputs: Vec<String>,
    #[serde(default)]
    crs_decisions: std::collections::BTreeMap<String, String>,
}

/// `schema_version` del piano, senza validazione strutturale.
fn plan_schema_version(plan_text: &str) -> Result<u32, PlenoraError> {
    Ok(serde_json::from_str::<PlanVersionProbe>(plan_text)?.schema_version)
}

/// Fissa **un solo testo** per un piano DAG, e lo rende; `None` se il piano
/// dichiara la forma lineare legacy (`schema_version <= 3`), che prosegue sul
/// percorso invariato.
///
/// Che cosa succede a ciascuna versione:
///
/// - **v4**: migrata al canonico v5, con cui condivide l'identita';
/// - **v5**: prestata invariata;
/// - **v6**: prestata **invariata**. Non si migra e non si rinormalizza:
///   riscriverla anche solo per normalizzarla cambierebbe un documento che
///   porta la propria identita'.
///
/// La CLI sonda il piano piu' volte (input dichiarati, decisioni CRS) prima
/// di chiamare il planner: se la migrazione avvenisse solo dentro il planner,
/// quelle sonde leggerebbero il testo v4 e il planner un altro testo. Qui il
/// testo e' deciso una volta, e da li' in poi tutti guardano quello.
pub(crate) fn testo_piano_dag(plan_text: &str) -> Result<Option<Cow<'_, str>>, PlenoraError> {
    let versione = plan_schema_version(plan_text)?;
    if versione < u32::from(PLAN_SCHEMA_VERSION_V4) {
        return Ok(None);
    }
    // La v6 non si migra e non si tocca: e' gia' il testo che il suo parser
    // legge, e riscriverlo qui — anche solo per rinormalizzarlo — cambierebbe
    // un documento che porta la propria identita'. Passarlo da
    // `testo_canonico_v5` lo avrebbe fatto **rifiutare**, perche' quella
    // funzione conosce solo la v4 e la v5: con il risultato che nessun piano
    // v6 arrivava al planner.
    if versione == u32::from(PLAN_SCHEMA_VERSION_V6) {
        return Ok(Some(Cow::Borrowed(plan_text)));
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
pub(crate) fn contract_error_missing(name: &str) -> PlenoraError {
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
/// sonda lo legge per prima. Per i piani DAG la ripetono i parser di formato
/// — `PlanV5::parse` e `PlanV6::parse`, ciascuno per il proprio — e la
/// ripete il dispatch prima di leggere la versione: e' idempotente, e copre
/// anche chi non passa dalla CLI.
pub(crate) fn read_control_plan_text(path: &Path) -> Result<String, PlenoraError> {
    let text = read_control_json_text(path)?;
    plenora_core::json::ensure_no_duplicate_keys(&text)?;
    Ok(text)
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
pub(crate) fn graph_summary_json(
    graph: &ValidatedGraph,
    execution: &ExecutionPlan,
) -> Result<serde_json::Value, PlenoraError> {
    let plan = graph.plan();
    let nodes: Vec<serde_json::Value> = plan
        .nodes()
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
    for name in plan.inputs() {
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
        // La versione la dice il PIANO, non questa riga. Era fissata a 5, e
        // un piano v6 sarebbe stato descritto come un v5 — con accanto un
        // `plan_hash` di un altro dominio.
        "schema_version": plan.schema_version(),
        "plan_hash": graph.plan_hash().to_hex(),
        "engine_version": graph.engine_version().to_string(),
        "inputs": plan.inputs(),
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
pub(crate) fn metrics_json(
    graph: &ValidatedGraph,
    metrics: &ExecutionMetrics,
) -> serde_json::Value {
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
        // Come in `explain`: la versione la dice il piano. Era fissata a 5, e
        // un riepilogo di `run` su un piano v6 avrebbe dichiarato 5 accanto a
        // un `plan_hash` di un altro dominio — cioe' proprio la coppia che
        // rende irriconoscibile un'identita' conservata.
        "schema_version": graph.plan_format_version(),
        "plan_hash": graph.plan_hash().to_hex(),
        "output_rows": metrics.output_rows,
        "output_batches": metrics.output_batches,
        "total_rows_processed": metrics.total_rows_processed,
        "geo_fusion_fallbacks": metrics.geo_fusion_fallbacks,
        "geo_fusion_groups_started": metrics.geo_fusion_groups_started,
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
pub(crate) fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|argument| argument == flag)
}

// Dispatch unico dei sottocomandi: la lunghezza e' data dalla sequenza
// lineare dei casi, non da complessita' logica; uno spezzone artificiale
// peggiorerebbe solo la leggibilita' (fase di pulizia: niente refactor
// strutturali).
#[allow(clippy::too_many_lines)]
pub(crate) fn run_with_args(args: &[String]) -> Result<(), Box<dyn Error>> {
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
    use plenora_core::RetryDisposition;
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
        // La tabella e' scritta a mano APPOSTA: e' la seconda opinione. Se
        // fosse derivata da `exit_code_di` verificherebbe che il codice e'
        // uguale a se stesso.
        //
        // Il difetto della versione precedente non era la tabella ma il
        // giro: si iterava la tabella, quindi una categoria nuova non
        // appariva da nessuna parte e restava semplicemente non coperta, in
        // silenzio. Ora si itera `ErrorCategory::ALL` e si PRETENDE che la
        // tabella la nomini — chi aggiunge una categoria deve passare di
        // qui, come deve passare da `exit_code_di`.
        let atteso: [(&str, i32); 20] = [
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
            ("isolation_unavailable", 5),
            ("unattributed_memory_pressure", 5),
            ("execution", 6),
            ("internal", 70),
            ("cancelled", 130),
        ];
        for &categoria in plenora_core::ErrorCategory::ALL {
            let nome = categoria.as_str();
            let codice = atteso
                .iter()
                .find_map(|(atteso, codice)| (*atteso == nome).then_some(*codice))
                .unwrap_or_else(|| {
                    panic!("categoria `{nome}` senza exit code dichiarato in questa tabella")
                });
            // Il giro completo, come lo fa la CLI: envelope -> stringa ->
            // categoria -> numero.
            let envelope = serde_json::json!({"error": {"category": nome}});
            assert_eq!(error_exit_code(&envelope), codice, "categoria `{nome}`");
            // E il ramo tipizzato da solo, senza passare per il JSON: e'
            // quello che il compilatore presidia.
            assert_eq!(
                cli::error_envelope::exit_code_di(categoria),
                codice,
                "categoria `{nome}`"
            );
        }
        // Una stringa che non e' una categoria non passa per «successo», e
        // nemmeno per una categoria vicina di nome.
        for ignota in ["categoria-nuova", "", "INTERNAL", "internal "] {
            let envelope = serde_json::json!({"error": {"category": ignota}});
            assert_eq!(
                error_exit_code(&envelope),
                EXIT_INTERNO,
                "stringa non riconosciuta `{ignota}`"
            );
        }
        // Un envelope senza il campo affatto: stesso ripiego, nessun panic.
        assert_eq!(error_exit_code(&serde_json::json!({})), EXIT_INTERNO);
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
                serde_json::Value::from(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX));
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
        let result = discover_input_contract_from_schema(schema, resolve_crs);
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
        let result = discover_input_contract_from_schema(schema, resolve_crs);
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
        let result = discover_input_contract_from_schema(schema, resolve_crs);
        assert!(matches!(result, Err(PlenoraError::Unsupported(_))));
    }

    #[test]
    fn discovery_rejects_canonical_keys_without_contract_version() {
        // R2.5: chiavi canoniche senza `plenora.contract.version` nei
        // metadati dello schema -> errore esplicito.
        let schema = std::sync::Arc::new(Schema::new(vec![canonical_geometry_field(
            DataType::Binary,
        )]));
        let result = discover_input_contract_from_schema(schema, resolve_crs);
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
        let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
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
        let contract = discover_input_contract_from_schema(schema, resolve_crs).expect("discovery");
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
        let contract = discover_input_contract_from_schema(schema, resolve_crs).expect("discovery");
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
            let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
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
        let result = discover_input_contract_from_schema(schema, resolve_crs);
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
        let result = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs);
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
        let contract = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
            .expect("discovery");
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
        let error = discover_input_contract_from_schema(schema_v1(vec![field]), resolve_crs)
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
        } = contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("stato")
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
        } = contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("stato")
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
        let result = contract_crs_from_keys("geometry", None, &keys, resolve_crs);
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
        let result = contract_crs_from_keys("geometry", None, &keys, resolve_crs);
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
        let result = contract_crs_from_keys("geometry", Some(&legacy), &keys, resolve_crs);
        #[cfg(feature = "proj-backend")]
        assert!(matches!(result, Ok(ContractCrs::Resolved(_))), "{result:?}");
        #[cfg(not(feature = "proj-backend"))]
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "{result:?}");
        // Nessuna rappresentazione: `Missing`, mai errore (R4.6.3).
        let missing =
            contract_crs_from_keys("geometry", None, &keys, resolve_crs).expect("assente");
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
        let result = discover_input_contract_from_schema(
            std::sync::Arc::new(Schema::new(vec![unknown_extension])),
            resolve_crs,
        );
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
        let result = discover_input_contract_from_schema(
            std::sync::Arc::new(Schema::new(vec![orphan])),
            resolve_crs,
        );
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
            DagInputs::Positional(paths) => paths,
            DagInputs::Named(_) => panic!("attesa forma posizionale"),
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
            DagInputs::Named(vec![
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
