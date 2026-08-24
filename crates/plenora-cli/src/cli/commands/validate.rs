//! `validate`: il piano e' eseguibile su questi input?
//!
//! Non esegue nulla e non pubblica nulla. Attraversa il planner — limiti,
//! struttura del grafo, contratti, capability — e riporta l'identita' del
//! grafo validato piu' la strategia scelta da `explain`.

use std::error::Error;
use std::path::{Path, PathBuf};

use plenora_engine::planner;
use plenora_engine::table_engine::Plan;
use plenora_engine::{explain, RuntimeContext};

use crate::cli::commands::run::{v4_inputs, DagInputs};
use crate::cli::contract_discovery::{apply_crs_decisions, discover_contracts, pair_v4_inputs};
use crate::{
    contract, graph_summary_json, has_flag, read_control_plan_text, testo_piano_dag, value_after,
    OutputFormat, PlanInputsProbe,
};

pub fn validate_command(args: &[String]) -> Result<(), Box<dyn Error>> {
    OutputFormat::require_json("validate")?;
    let plan_path = value_after(args, "--plan")?;
    // Stesso parser di `run`: le due forme — `--input nome=percorso` e
    // `--inputs` posizionale — devono comportarsi allo stesso modo nei due
    // comandi, altrimenti si valida un accoppiamento e se ne esegue un altro.
    let inputs = v4_inputs(args)?;
    let plan_text = read_control_plan_text(Path::new(&plan_path))?;
    if let Some(plan_text) = testo_piano_dag(&plan_text)? {
        return validate_dag(
            plan_text.as_ref(),
            &inputs,
            !has_flag(args, "--no-geo-fusion"),
        );
    }
    let inputs: Vec<PathBuf> = match inputs {
        DagInputs::Positional(paths) => paths,
        // I piani legacy non hanno input nominati: il riepilogo elenca i soli
        // percorsi, e accettare una forma che non sanno usare confonderebbe.
        DagInputs::Named(_) => {
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

/// `validate` di un piano DAG: planner DAG + `explain` per la strategia, con
/// riepilogo JSON su stdout (architettura.md#planner-ed-executor: `prepare` e' interna all'engine).
/// `geo_fusion` e' il kill switch D12.9 (flag `--no-geo-fusion`): a `false`
/// i gruppi di fusione non si formano e `explain` mostra la strategia non
/// fusa.
pub fn validate_dag(
    plan_text: &str,
    inputs: &DagInputs,
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
