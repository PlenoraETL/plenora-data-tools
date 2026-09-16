//! `run`: l'unico comando che esegue e pubblica.
//!
//! Tutto cio' che sta qui e' fail-closed per costruzione: al primo errore
//! nessun output e' pubblicato, perche' la pubblicazione e' atomica e avviene
//! solo alla fine. La cancellazione cooperativa ha un exit code dedicato e le
//! stesse garanzie.

use std::error::Error;
use std::path::{Path, PathBuf};

use plenora_core::catalog::find_operation;
use plenora_core::PlenoraError;
use plenora_engine::geo_transport::publish::PublishProfile;
use plenora_engine::planner;
use plenora_engine::table_engine::Plan;
use plenora_engine::{
    execute, ipc_boundary, parallelism, CancellationToken, Inputs, RuntimeContext,
};

use crate::cli::contract_discovery::{
    apply_crs_decisions, discover_contracts, open_input, pair_v4_inputs,
};
#[cfg(target_os = "linux")]
use crate::installa_gestore_segnale_isolato;
use crate::{
    campi_della_pubblicazione, contract, contract_error_missing, has_flag, install_ctrlc_handler,
    metrics_json, optional_value_after, read_control_plan_text, run_pipeline, testo_piano_dag,
    value_after, OutputFormat, PlanInputsProbe,
};

/// `run` di un piano DAG: esecuzione DAG e pubblicazione atomica dell'output,
/// con metriche JSON su stdout. Installa l'handler Ctrl-C: al cancel
/// l'executor propaga `PlenoraError::Cancelled`, il publish atomico non e'
/// mai raggiunto e `main` esce con [`EXIT_CANCELLED`]. `geo_fusion` e' il
/// kill switch D12.9 (flag `--no-geo-fusion`).
pub fn run_dag(
    plan_text: &str,
    inputs: &DagInputs,
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
    // `PR-12`: un piano che dichiara `max_domain_memory_bytes` chiede il
    // profilo isolato. `planner::validate` ha gia' respinto la richiesta
    // fuori da Linux (`Unsupported`) prima che questo `graph` potesse
    // esistere: se arriviamo qui con una richiesta, siamo su Linux per
    // costruzione, e il percorso e' un altro — mai l'esecuzione in-process
    // sotto. Su chi non e' Linux questo ramo non si compila nemmeno: non
    // esiste un `graph.plan().max_domain_memory_bytes() == Some(_)`
    // raggiungibile li'.
    #[cfg(target_os = "linux")]
    if let Some(richiesto_byte) = graph.plan().max_domain_memory_bytes() {
        return run_isolato(&graph, &pairs, output_path, richiesto_byte);
    }
    // `max_parallelism` si applica QUI, prima di aprire gli input e prima di
    // qualunque uso di Rayon: dimensiona il pool del processo, che e' l'unica
    // leva che vincola davvero tutti i percorsi paralleli dei kernel. Senza
    // questo passo il limite sarebbe una promessa di risorsa, non un tetto.
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
    let (metrics, esito) =
        output.write_ipc_file_with_profile(output_path, PublishProfile::Atomic)?;
    let mut documento = metrics_json(&graph, &metrics);
    if let Some(oggetto) = documento.as_object_mut() {
        for (chiave, valore) in campi_della_pubblicazione(&esito) {
            oggetto.insert(chiave, valore);
        }
    }
    println!("{}", serde_json::to_string_pretty(&documento)?);
    Ok(())
}

/// Il percorso isolato di `run` (`PR-12`): autorizza la politica dell'host,
/// poi esegue per davvero — dominio, spawner, protocollo, verifica e
/// pubblicazione — invece del percorso in-process sopra.
///
/// Stesso formato d'uscita del percorso in-process (`campi_della_pubblicazione`),
/// ma senza le metriche per nodo: quelle restano nel processo worker, che le
/// scarta dopo aver scritto l'artefatto (documentato in
/// `isolamento::esecuzione_isolata`), non attraversano il confine.
#[cfg(target_os = "linux")]
fn run_isolato(
    graph: &plenora_engine::planner::ValidatedGraph,
    pairs: &[(String, PathBuf)],
    output_path: &Path,
    richiesto_byte: u64,
) -> Result<(), Box<dyn Error>> {
    use plenora_engine::isolamento::{attivazione, esecuzione_isolata};

    let governato_effettivo_byte = graph.effective_limits().max_governed_memory_bytes;
    let concessione = attivazione::prepara_e_autorizza(richiesto_byte, governato_effettivo_byte)?;
    // Stesso TOKEN del percorso in-process (un Ctrl-C cancella l'esecuzione
    // qualunque sia il percorso che il piano ha scelto), ma non lo stesso
    // HANDLER: `install_ctrlc_handler` fa nascere un thread (`ctrlc::
    // set_handler`) che vivrebbe per tutta la vita del processo e farebbe
    // fallire `isolamento::canale::accerta_monothread` a ogni finestra di
    // spawn (worker e verificatore) — `installa_gestore_segnale_isolato`
    // (main.rs) copre lo stesso token senza far nascere alcun thread.
    let token = CancellationToken::new();
    installa_gestore_segnale_isolato(&token)?;
    let esito = esecuzione_isolata::esegui_isolato(graph, pairs, output_path, concessione, token)?;

    let mut documento = serde_json::Map::new();
    documento.insert(
        "status".to_owned(),
        serde_json::Value::String("ok".to_owned()),
    );
    documento.insert(
        "schema_version".to_owned(),
        serde_json::Value::from(graph.plan_format_version()),
    );
    documento.insert(
        "plan_hash".to_owned(),
        serde_json::Value::String(graph.plan_hash().to_hex()),
    );
    // Machine-readable per costruzione (`ConcessioneDominio` deriva
    // `Serialize` proprio per questo): il tetto richiesto dal piano e quello
    // concesso dalla politica dell'host possono divergere (`min`), e chi
    // legge l'esito deve poterli distinguere senza rileggere i log del
    // preflight.
    documento.insert("isolation".to_owned(), serde_json::to_value(concessione)?);
    for (chiave, valore) in campi_della_pubblicazione(&esito) {
        documento.insert(chiave, valore);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(documento))?
    );
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
pub enum DagInputs {
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
pub fn is_named_input(value: &str) -> bool {
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
pub fn v4_inputs(args: &[String]) -> Result<DagInputs, PlenoraError> {
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
        return Ok(DagInputs::Positional(positional));
    }
    Ok(DagInputs::Named(named))
}

pub fn reject_legacy_row_diagnostics_plan(plan_text: &str) -> Result<(), PlenoraError> {
    // Fail-closed su TUTTI i piani legacy che contengono op row-diagnostics,
    // anche blocking/secondary: nel percorso legacy non esiste gate
    // provenance (quello e' solo DAG) e un nodo blocking (es. sort)
    // renderebbe gli indici pubblicati posizioni post-riordino, non
    // `source_row_zero_based`. Nessun indice inventato: si richiede DAG.
    //
    // Autorita' UNICA: `OperationDescriptor::emits_row_diagnostics`
    // (catalogo plenora-core), la stessa del gate provenance del planner e
    // del machinery di segmento dell'executor — nessuna lista locale
    // duplicata, che qui ometterebbe formula/expression (hmac_sha256 non
    // emette, md5/sha256 solo con null_policy=error). La scansione
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
pub fn run_command(args: &[String]) -> Result<(), Box<dyn Error>> {
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
        return run_dag(
            plan_text.as_ref(),
            &v4_inputs(args)?,
            &output_path,
            !has_flag(args, "--no-geo-fusion"),
        );
    }
    reject_legacy_row_diagnostics_plan(&plan_text)?;
    let esito = run_pipeline(
        &plan_path,
        &value_after(args, "--input")?,
        optional_value_after(args, "--right")?.as_deref(),
        &output_path,
    )?;
    // Il ramo legacy emette ora un documento di successo, come gia' fa il ramo
    // DAG dello stesso comando. Non e' cosmesi: senza un documento l'avvertenza
    // di pulizia non ha dove uscire, e una pubblicazione che lascia spazzatura
    // senza dirlo e' un fallimento silenzioso. Il formato d'uscita e' gia'
    // JSON per contratto — `OutputFormat::require_json` lo impone in testa a
    // questo comando — quindi il documento non introduce un canale nuovo: usa
    // quello che il comando dichiara di avere.
    let mut documento = serde_json::Map::new();
    documento.insert(
        "status".to_owned(),
        serde_json::Value::String("ok".to_owned()),
    );
    for (chiave, valore) in campi_della_pubblicazione(&esito) {
        documento.insert(chiave, valore);
    }
    println!(
        "{}",
        serde_json::to_string(&serde_json::Value::Object(documento))?
    );
    Ok(())
}
