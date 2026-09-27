//! Scoperta del contratto di un input Arrow, e le decisioni sul CRS.
//!
//! Il contratto si legge dai soli metadati dello schema, senza toccare una
//! riga: ne nascono `describe`, la validazione del piano e la verifica degli
//! input. Una dichiarazione CRS incoerente resta `DeclaredUnresolved`, e la
//! sola risoluzione e' la decisione del piano ([`apply_crs_decisions`]).

use std::path::{Path, PathBuf};

use plenora_core::arrow::schema::{DataType, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, ContractProperty, CrsDefinitionFormat, CrsResolution,
    DataContract, FieldId, GeometryColumnContract, GeometryDimensions, PropertyConfidence,
    PropertyScope,
};
use plenora_core::crs::ResolvedCrs;
use plenora_core::PlenoraError;
use plenora_engine::{ipc_boundary, Input, IpcLimits};
use plenora_kernels_geo::arrow_adapter::{
    read_contract_version, read_geometry_contract_keys, CanonicalGeometryKeys,
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
};

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

use crate::{contract, DagInputs, PlanInputsProbe};

// La conversione schema -> contratto e' autorita' di `plenora-core`: qui
// resta il contesto di file e input, che core non deve conoscere.
pub use plenora_core::contract::arrow_schema::{
    contract_crs_from_keys, contract_from_arrow_schema as discover_input_contract_from_schema,
    crs_definition_from_metadata, geometry_contract_from_field,
};

/// Schema Arrow dell'header IPC di un input (file o stream format): nessuna
/// riga di dati letta.
///
/// Passa dal lettore di confine condiviso ([`plenora_engine::ipc_boundary`]);
/// gli errori di apertura e parse dell'header portano [`ErrorPhase::Read`]
/// (BLOCK-03).
pub fn ipc_header_schema(path: &Path) -> Result<SchemaRef, PlenoraError> {
    ipc_boundary::header_schema(path, &IpcLimits::default())
}

/// Input lazy per l'executor: IPC file o stream format, sniffato dal magic.
pub fn open_input(path: &Path, limits: &IpcLimits) -> Result<Input, PlenoraError> {
    Input::read_ipc_with_limits(path, limits)
}

/// Scoperta del `DataContract` di un input dal solo header IPC: schema Arrow
/// e colonne geometria se presenti, CRS risolto SE dichiarato. Fail-closed
/// su metadati incoerenti. Il `FieldId` e' provvisorio: il planner lo rimappa
/// nel namespace globale del grafo (D16).
///
/// Protocollo delle chiavi canoniche (contratti trasversali §2):
///
/// - gate R2.5 all'ingresso: [`read_contract_version`];
/// - per ogni campo geometria [`read_geometry_contract_keys`] applica chiavi
///   canoniche, coerenza con le legacy (R2.6) e precedenza (R2.7);
/// - le chiavi `plenora.geometry.*` bastano a riconoscere una colonna
///   geometrica; il tipo deve comunque essere `Binary`;
/// - lo stato CRS lo decide [`contract_crs_from_keys`]. Un campo senza CRS
///   diventa [`ContractCrs::Missing`] (R4.6.3, R4.4) e ferma solo le op con
///   un `CrsRequirement`; un'incoerenza dichiarata diventa
///   [`ContractCrs::DeclaredUnresolved`]; `crs_resolution` senza alcuna
///   rappresentazione resta un errore (R4.1).
pub fn discover_input_contract(path: &Path) -> Result<DataContract, PlenoraError> {
    // Il risolutore e' quello scelto dalla build (base oppure PROJ): l'autorita'
    // in core interpreta lo schema, il chiamante fornisce il backend. Non e'
    // una scelta di questo file — e' la stessa che il worker isolato dovra'
    // ricevere dal supervisore, invece di dedurla dalla propria compilazione.
    discover_input_contract_from_schema(ipc_header_schema(path)?, resolve_crs)
}

/// Contesto "input `nome` (percorso)" sull'errore, preservando la variante.
pub fn at_input(name: &str, path: &Path, error: PlenoraError) -> PlenoraError {
    error.con_contesto(&format!("input `{name}` ({})", path.display()))
}

/// Accoppia gli input della riga di comando a quelli dichiarati dal piano DAG.
///
/// Nella forma nominale ogni nome dev'essere dichiarato e ogni input
/// dichiarato fornito una volta. La forma posizionale e' ammessa solo con un
/// input dichiarato: con due file scambiati dello stesso schema il piano
/// girerebbe e darebbe un risultato sbagliato.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` su nome non dichiarato, input dichiarato ma non
/// fornito, forma posizionale con piu' di un input dichiarato, o conteggio
/// diverso nella forma posizionale.
pub fn pair_v4_inputs(
    probe: &PlanInputsProbe,
    inputs: &DagInputs,
) -> Result<Vec<(String, PathBuf)>, PlenoraError> {
    // Il tetto sugli input lo applica anche `planner::validate`, ma dopo la
    // scoperta dei contratti, che apre ogni file. Qui vale lo stesso tetto,
    // sullo stesso conteggio — gli input **dichiarati** — prima che se ne apra
    // uno: `run` e `validate` passano entrambi di qui.
    let tetto = probe.tetto_ingressi();
    if probe.inputs.len() > tetto {
        return Err(contract(format!(
            "max_inputs superato: {} input > {tetto}",
            probe.inputs.len()
        )));
    }
    let paths = match inputs {
        DagInputs::Named(named) => {
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
        DagInputs::Positional(paths) => paths,
    };
    if probe.inputs.len() > 1 {
        // Con piu' di un input la forma posizionale non e' verificabile: si
        // rifiuta prima di toccare i file, indicando la forma nominale.
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
pub fn discover_contracts(
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
/// R4.6.3) ai contratti scoperti.
///
/// La definizione decisa si risolve contro il backend e porta lo stato da
/// [`ContractCrs::DeclaredUnresolved`] a [`ContractCrs::ResolvedByDecision`],
/// che in emissione sostituisce le dichiarazioni della sorgente. Lo schema
/// del contratto di input non cambia; il fingerprint si', e la decisione e'
/// coperta dal `plan_hash`.
///
/// Errori espliciti: input non fornito o senza colonna geometrica; stato
/// diverso da `DeclaredUnresolved` (R4.4); definizione non risolvibile. I
/// messaggi non riportano valori di dichiarazioni.
pub fn apply_crs_decisions(
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
