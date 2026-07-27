//! Formato piano v4: DAG dichiarativo (Architetture.md par. 5, ADR 4, ADR 6).
//!
//! Fondamenta della Fase 2A — codice NUOVO, non trasloco:
//!
//! - [`PlanV4`]/[`NodeV4`]: il formato dichiarativo (solo dipendenze e
//!   configurazioni, nessuna annotazione di esecuzione), serde con
//!   `deny_unknown_fields` a ogni livello;
//! - [`PlanV4::parse`]: applicazione dei [`PlanLimits`] DURANTE il parsing
//!   (ADR 6: il prima possibile, prima di allocazioni guidate dal contenuto),
//!   poi validazione strutturale (id unici, riferimenti esistenti, aciclicità,
//!   output raggiungibile, arietà da catalogo) e risoluzione alias verso gli
//!   id canonici;
//! - [`PlanV4::from_legacy`]: migrazione del piano lineare legacy
//!   (`Plan{steps}`, schema_version <= 3) nel caso degenerato del DAG —
//!   deterministica e idempotente (decisione D20, ADR 4);
//! - [`canonical_json`]: serializzazione canonica ai fini del `plan_hash`
//!   (ADR 4): chiavi ordinate, nodi in ordine topologico deterministico,
//!   alias sostituiti dagli id canonici, numeri della config normalizzati
//!   (`100` ≡ `100.0`), default noti materializzati (limiti effettivi,
//!   config omessa ≡ `{}`).
//!
//! Scelte v1 documentate:
//!
//! - i `PlanLimits` di parsing arrivano dal CHIAMANTE (default fail-closed
//!   [`PlanLimits::default`]): `max_plan_json_bytes` va applicato prima del
//!   parse, quindi non può provenire dal piano stesso. Il campo `limits` del
//!   piano (override opzionali, [`LimitsOverride`]) riguarda dati/runtime ed è
//!   materializzato nella canonicalizzazione; la sua sotto-sezione `plan`
//!   NON influenza il parsing in corso;
//! - `output` può riferire un nodo o un input (piano pass-through); ogni nodo
//!   deve essere antenato dell'output (niente nodi morti) e ogni input
//!   dichiarato deve essere referenziato da almeno un nodo (niente input
//!   morti), a meno che non sia l'output stesso;
//! - la materializzazione dei default PER OPERAZIONE dentro `config` è
//!   rimandata: richiede schemi di config tipizzati per operazione, che la v1
//!   non ha (fuori scope) — qui la config resta `serde_json::Value`
//!   (l'equivalenza null ≡ `{}` e la normalizzazione dei numeri sono già
//!   applicate).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Number, Value};

use plenora_core::catalog::{find_operation, Arity, Family, Maturity};
use plenora_core::limits::{Limits, PlanLimits};
use plenora_core::{PlenoraError, Result};

use crate::table_engine;

/// Versione del formato piano DAG (Architetture.md par. 5).
pub const PLAN_SCHEMA_VERSION_V4: u16 = 4;

/// Override opzionali dei limiti dati/runtime nel piano v4.
///
/// Specchio di [`Limits`] con tutti i campi opzionali: un piano dichiara solo
/// ciò che vuole restringere, i default di [`Limits::default`] coprono il
/// resto. La sotto-sezione `plan` è riservata alle fasi successive e NON
/// influenza il parsing del piano che la contiene.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rows_per_edge: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_expansion_factor: Option<f64>,
    #[serde(default)]
    pub plan: PlanLimitsOverride,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_temp_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_partitions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallelism: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wkb_cell_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_payload_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_batches: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_geometry_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_string_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_regex_bytes: Option<usize>,
}

impl LimitsOverride {
    /// Applica gli override su una base, producendo i limiti effettivi.
    #[must_use]
    pub fn apply_to(&self, base: &Limits) -> Limits {
        let mut effective = base.clone();
        if let Some(value) = self.max_input_rows {
            effective.rows.max_input_rows = value;
        }
        if let Some(value) = self.max_output_rows {
            effective.rows.max_output_rows = value;
        }
        if let Some(value) = self.max_rows_per_edge {
            effective.rows.max_rows_per_edge = value;
        }
        if let Some(value) = self.max_expansion_factor {
            effective.rows.max_expansion_factor = value;
        }
        effective.plan = self.plan.apply_to(&base.plan);
        if let Some(value) = self.max_memory_bytes {
            effective.max_memory_bytes = value;
        }
        if let Some(value) = self.max_temp_bytes {
            effective.max_temp_bytes = value;
        }
        if let Some(value) = self.spill_partitions {
            effective.spill_partitions = value;
        }
        if let Some(value) = self.max_parallelism {
            effective.max_parallelism = value;
        }
        if let Some(value) = self.max_wkb_cell_bytes {
            effective.max_wkb_cell_bytes = value;
        }
        if let Some(value) = self.max_payload_bytes {
            effective.max_payload_bytes = value;
        }
        if let Some(value) = self.max_batches {
            effective.max_batches = value;
        }
        if let Some(value) = self.max_geometry_depth {
            effective.max_geometry_depth = value;
        }
        if let Some(value) = self.max_string_bytes {
            effective.max_string_bytes = value;
        }
        if let Some(value) = self.max_regex_bytes {
            effective.max_regex_bytes = value;
        }
        effective
    }

    /// Limiti effettivi a partire dai default.
    #[must_use]
    pub fn effective(&self) -> Limits {
        self.apply_to(&Limits::default())
    }
}

/// Override opzionali dei [`PlanLimits`] (stessa logica di [`LimitsOverride`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanLimitsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_plan_json_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_plan_nodes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_plan_edges: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_plan_depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fan_out: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inputs: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_config_bytes_per_node: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_identifier_bytes: Option<usize>,
}

impl PlanLimitsOverride {
    /// Applica gli override su una base, producendo i limiti effettivi.
    #[must_use]
    pub fn apply_to(&self, base: &PlanLimits) -> PlanLimits {
        let mut effective = base.clone();
        if let Some(value) = self.max_plan_json_bytes {
            effective.max_plan_json_bytes = value;
        }
        if let Some(value) = self.max_plan_nodes {
            effective.max_plan_nodes = value;
        }
        if let Some(value) = self.max_plan_edges {
            effective.max_plan_edges = value;
        }
        if let Some(value) = self.max_plan_depth {
            effective.max_plan_depth = value;
        }
        if let Some(value) = self.max_fan_out {
            effective.max_fan_out = value;
        }
        if let Some(value) = self.max_inputs {
            effective.max_inputs = value;
        }
        if let Some(value) = self.max_config_bytes_per_node {
            effective.max_config_bytes_per_node = value;
        }
        if let Some(value) = self.max_identifier_bytes {
            effective.max_identifier_bytes = value;
        }
        effective
    }
}

/// Nodo del DAG: solo dipendenze e configurazione (Architetture.md par. 5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeV4 {
    /// Identificatore unico nel piano.
    pub id: String,
    /// Id operazione: canonico (`table.*`/`geo.*`) o alias legacy; normalizzato
    /// all'id canonico durante la validazione.
    pub op: String,
    /// Riferimenti agli input dichiarati o ad altri nodi. Per le operazioni
    /// `BinaryOrdered` l'ordine è semantico: `[left, right]`.
    #[serde(default, rename = "in")]
    pub inputs: Vec<String>,
    /// Configurazione grezza; la deserializzazione tipizzata contro lo schema
    /// dell'operazione è Fase 2A-2. `null` (omessa) equivale a `{}`.
    #[serde(default)]
    pub config: Value,
}

/// Piano v4: DAG dichiarativo (Architetture.md par. 5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanV4 {
    pub schema_version: u16,
    #[serde(default)]
    pub limits: LimitsOverride,
    /// CRS dichiarato a livello di piano (risoluzione e verifica dei
    /// requisiti per nodo: Fase 2A-2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crs: Option<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    pub nodes: Vec<NodeV4>,
    /// Id del nodo (o dell'input, per un piano pass-through) che produce
    /// l'output del piano.
    pub output: String,
}

/// Piano v4 che ha superato parsing, limiti e validazione strutturale.
///
/// Contiene solo decisioni strutturali stabili: l'inferenza dei contratti
/// degli archi (`analyze_contract`) è Fase 2A-2 e produrrà il
/// `ValidatedGraph` completo.
#[derive(Debug, Clone)]
pub struct ValidatedPlanV4 {
    plan: PlanV4,
    /// Id dei nodi in ordine topologico deterministico (Kahn con tie-break
    /// lessicografico): è l'ordine usato da `analyze_contract` e dalla
    /// canonicalizzazione.
    topo_order: Vec<String>,
}

impl ValidatedPlanV4 {
    #[must_use]
    pub fn plan(&self) -> &PlanV4 {
        &self.plan
    }

    #[must_use]
    pub fn into_plan(self) -> PlanV4 {
        self.plan
    }

    /// Id dei nodi in ordine topologico deterministico.
    #[must_use]
    pub fn topological_order(&self) -> &[String] {
        &self.topo_order
    }

    /// Limiti dati/runtime effettivi (override del piano sui default).
    #[must_use]
    pub fn effective_limits(&self) -> Limits {
        self.plan.limits.effective()
    }

    /// Serializzazione canonica ai fini del futuro `plan_hash` (ADR 4).
    #[must_use]
    pub fn canonical_json(&self) -> Value {
        canonical_json(&self.plan)
    }
}

fn contract_error(message: String) -> PlenoraError {
    PlenoraError::Contract(message)
}

impl PlanV4 {
    /// Parse completo di un piano v4: limiti durante il parsing (ADR 6),
    /// validazione strutturale, risoluzione alias.
    ///
    /// # Errors
    ///
    /// Restituisce `PlenoraError::Contract` per ogni violazione di limiti o
    /// di struttura, `PlenoraError::Json` per JSON malformato,
    /// `PlenoraError::Unsupported` per operazioni catalogate ma non
    /// disponibili (`Maturity::Planned`).
    pub fn parse(json_text: &str, plan_limits: &PlanLimits) -> Result<ValidatedPlanV4> {
        // ADR 6: il limite sui byte del JSON si applica PRIMA del parse.
        if json_text.len() > plan_limits.max_plan_json_bytes {
            return Err(contract_error(format!(
                "max_plan_json_bytes superato: {} byte > {}",
                json_text.len(),
                plan_limits.max_plan_json_bytes
            )));
        }
        let mut plan: PlanV4 = serde_json::from_str(json_text)?;
        let topo_order = plan.validate_structure(plan_limits)?;
        Ok(ValidatedPlanV4 { plan, topo_order })
    }

    /// Parse con i `PlanLimits` di default (fail-closed).
    ///
    /// # Errors
    /// Come [`PlanV4::parse`].
    pub fn parse_default(json_text: &str) -> Result<ValidatedPlanV4> {
        Self::parse(json_text, &PlanLimits::default())
    }

    /// Migrazione del piano lineare legacy (`Plan{steps}`, schema_version
    /// <= 3) nel caso degenerato del DAG: ogni step diventa un nodo il cui
    /// unico `in` è il nodo precedente (o l'unico input); il singolo step
    /// binario del protocollo a due input diventa un nodo con
    /// `in = ["left", "right"]`.
    ///
    /// Deterministica (id `n0..nN`, input `main`/`left`/`right` fissi) e
    /// idempotente (decisione D20, ADR 4): il risultato è già un piano v4
    /// validato con i limiti di default.
    ///
    /// # Errors
    ///
    /// Restituisce `PlenoraError::Contract` se il piano legacy è vuoto, ha
    /// operazioni sconosciute alla famiglia tabellare o viola la regola
    /// "catena binaria = un solo step"; più le violazioni dei `PlanLimits`
    /// di default.
    pub fn from_legacy(legacy: &table_engine::Plan) -> Result<Self> {
        if legacy.schema_version == 0 || legacy.schema_version > 3 {
            return Err(contract_error(format!(
                "schema_version legacy {} non migrabile (attese 1..=3)",
                legacy.schema_version
            )));
        }
        if legacy.steps.is_empty() {
            return Err(contract_error(
                "il piano legacy non contiene passi".into(),
            ));
        }

        let descriptors = legacy
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                find_operation(&step.operation)
                    .filter(|descriptor| descriptor.family == Family::Table)
                    .ok_or_else(|| {
                        contract_error(format!(
                            "operazione sconosciuta al passo {index}: {}",
                            step.operation
                        ))
                    })
            })
            .collect::<Result<Vec<_>>>()?;

        let binary_steps = descriptors
            .iter()
            .filter(|descriptor| descriptor.arity != Arity::Unary)
            .count();
        if binary_steps > 0 && (binary_steps != 1 || legacy.steps.len() != 1) {
            return Err(contract_error(
                "il protocollo legacy a due input accetta esattamente un passo binario".into(),
            ));
        }

        let (plan_inputs, first_inputs) = if binary_steps == 1 {
            (
                vec!["left".to_owned(), "right".to_owned()],
                vec!["left".to_owned(), "right".to_owned()],
            )
        } else {
            (vec!["main".to_owned()], vec!["main".to_owned()])
        };

        let nodes = legacy
            .steps
            .iter()
            .zip(descriptors)
            .enumerate()
            .map(|(index, (step, descriptor))| NodeV4 {
                id: format!("n{index}"),
                op: descriptor.id.to_owned(),
                inputs: if index == 0 {
                    first_inputs.clone()
                } else {
                    vec![format!("n{}", index - 1)]
                },
                config: step.config.clone(),
            })
            .collect::<Vec<_>>();

        let output = format!("n{}", nodes.len() - 1);
        let limits = legacy_limits_override(&legacy.limits);

        let mut plan = Self {
            schema_version: PLAN_SCHEMA_VERSION_V4,
            limits,
            crs: None,
            inputs: plan_inputs,
            nodes,
            output,
        };
        // La migrazione produce un piano già valido: la validazione con i
        // limiti di default è una verifica interna, non un secondo ingresso.
        plan.validate_structure(&PlanLimits::default())?;
        Ok(plan)
    }

    /// Limiti durante il parsing + validazione strutturale + risoluzione
    /// alias. Restituisce l'ordine topologico deterministico dei nodi.
    fn validate_structure(&mut self, plan_limits: &PlanLimits) -> Result<Vec<String>> {
        if self.schema_version != PLAN_SCHEMA_VERSION_V4 {
            return Err(contract_error(format!(
                "schema_version {} non supportata; attesa {PLAN_SCHEMA_VERSION_V4}",
                self.schema_version
            )));
        }

        // --- Conteggi e byte (ADR 6: prima delle validazioni guidate dal
        // contenuto) ---
        if self.inputs.len() > plan_limits.max_inputs {
            return Err(contract_error(format!(
                "max_inputs superato: {} input > {}",
                self.inputs.len(),
                plan_limits.max_inputs
            )));
        }
        if self.nodes.len() > plan_limits.max_plan_nodes {
            return Err(contract_error(format!(
                "max_plan_nodes superato: {} nodi > {}",
                self.nodes.len(),
                plan_limits.max_plan_nodes
            )));
        }
        let edge_count: usize = self.nodes.iter().map(|node| node.inputs.len()).sum();
        if edge_count > plan_limits.max_plan_edges {
            return Err(contract_error(format!(
                "max_plan_edges superato: {edge_count} archi > {}",
                plan_limits.max_plan_edges
            )));
        }
        for input in &self.inputs {
            check_identifier("input", input, plan_limits)?;
        }
        check_identifier("output", &self.output, plan_limits)?;
        for node in &mut self.nodes {
            check_identifier("id nodo", &node.id, plan_limits)?;
            check_identifier("operazione", &node.op, plan_limits)?;
            // Config omessa (null) equivale a oggetto vuoto (ADR 4: config
            // omessa ≡ config con default esplicito).
            if node.config.is_null() {
                node.config = json!({});
            }
            if !node.config.is_object() {
                return Err(contract_error(format!(
                    "config del nodo `{}` deve essere un oggetto",
                    node.id
                )));
            }
            let config_bytes = serde_json::to_vec(&node.config)?.len();
            if config_bytes > plan_limits.max_config_bytes_per_node {
                return Err(contract_error(format!(
                    "max_config_bytes_per_node superato al nodo `{}`: {config_bytes} byte > {}",
                    node.id, plan_limits.max_config_bytes_per_node
                )));
            }
        }

        // --- Identificatori unici e non ambigui ---
        let input_names: HashSet<&str> = self.inputs.iter().map(String::as_str).collect();
        if input_names.len() != self.inputs.len() {
            return Err(contract_error("nomi di input duplicati".into()));
        }
        let mut node_ids: HashSet<String> = HashSet::with_capacity(self.nodes.len());
        for node in &self.nodes {
            if !node_ids.insert(node.id.clone()) {
                return Err(contract_error(format!("id nodo duplicato: `{}`", node.id)));
            }
            if input_names.contains(node.id.as_str()) {
                return Err(contract_error(format!(
                    "l'id nodo `{}` collide con un nome di input",
                    node.id
                )));
            }
        }

        // --- Risoluzione alias e arietà da catalogo ---
        for node in &mut self.nodes {
            let descriptor = find_operation(&node.op).ok_or_else(|| {
                contract_error(format!(
                    "operazione sconosciuta al nodo `{}`: {}",
                    node.id, node.op
                ))
            })?;
            if descriptor.maturity == Maturity::Planned {
                return Err(PlenoraError::Unsupported(format!(
                    "{} e' catalogata ma non ancora validata",
                    descriptor.id
                )));
            }
            let expected = match descriptor.arity {
                Arity::Unary => node.inputs.len() == 1,
                Arity::BinaryOrdered => node.inputs.len() == 2,
                Arity::NAry => !node.inputs.is_empty(),
            };
            if !expected {
                return Err(contract_error(format!(
                    "arieta' non coerente al nodo `{}`: {} richiede {} input, dichiarati {}",
                    node.id,
                    descriptor.id,
                    match descriptor.arity {
                        Arity::Unary => "esattamente 1",
                        Arity::BinaryOrdered => "esattamente 2",
                        Arity::NAry => "almeno 1",
                    },
                    node.inputs.len()
                )));
            }
            // Alias sostituito dall'id canonico (ADR 4).
            node.op = descriptor.id.to_owned();
        }

        // --- Riferimenti esistenti e fan-out ---
        let mut fan_out: HashMap<&str, usize> = HashMap::new();
        for node in &self.nodes {
            for reference in &node.inputs {
                if !input_names.contains(reference.as_str()) && !node_ids.contains(reference.as_str())
                {
                    return Err(contract_error(format!(
                        "il nodo `{}` riferisce `{reference}`, che non e' un input ne' un nodo",
                        node.id
                    )));
                }
                let count = fan_out.entry(reference.as_str()).or_insert(0);
                *count += 1;
                if *count > plan_limits.max_fan_out {
                    return Err(contract_error(format!(
                        "max_fan_out superato: `{reference}` ha {count} consumatori > {}",
                        plan_limits.max_fan_out
                    )));
                }
            }
        }

        // --- Input dichiarati ma non referenziati (fan-out 0): resterebbero
        // sorgenti lette mai usate; l'unico input senza consumatori ammesso
        // e' quello riferito direttamente dall'output (piano pass-through) ---
        for input in &self.inputs {
            if *input != self.output && !fan_out.contains_key(input.as_str()) {
                return Err(contract_error(format!(
                    "l'input `{input}` non e' referenziato da alcun nodo del piano",
                )));
            }
        }

        // --- Aciclicità, profondità, raggiungibilità dell'output ---
        let topo_order = topological_order(self, plan_limits)?;
        if !input_names.contains(self.output.as_str()) && !node_ids.contains(self.output.as_str()) {
            return Err(contract_error(format!(
                "output `{}` non e' un input ne' un nodo del piano",
                self.output
            )));
        }
        let reachable = ancestors_of_output(self);
        if let Some(dead) = self
            .nodes
            .iter()
            .find(|node| !reachable.contains(node.id.as_str()))
        {
            return Err(contract_error(format!(
                "il nodo `{}` non raggiunge l'output `{}`",
                dead.id, self.output
            )));
        }
        Ok(topo_order)
    }
}

/// Mappa i limiti legacy (`plenora_kernels_table::Limits`) sugli override v4.
fn legacy_limits_override(legacy: &table_engine::Limits) -> LimitsOverride {
    let max_rows = legacy.max_rows as u64;
    LimitsOverride {
        max_input_rows: Some(max_rows),
        max_output_rows: Some(max_rows),
        max_rows_per_edge: Some(max_rows),
        max_memory_bytes: Some(legacy.max_memory_bytes as u64),
        max_temp_bytes: Some(legacy.max_temp_bytes),
        spill_partitions: Some(legacy.spill_partitions as u32),
        max_string_bytes: Some(legacy.max_string_bytes),
        max_regex_bytes: Some(legacy.max_regex_bytes),
        ..LimitsOverride::default()
    }
}

fn check_identifier(kind: &str, value: &str, plan_limits: &PlanLimits) -> Result<()> {
    if value.is_empty() {
        return Err(contract_error(format!("{kind} vuoto")));
    }
    if value.len() > plan_limits.max_identifier_bytes {
        return Err(contract_error(format!(
            "max_identifier_bytes superato per {kind}: {} byte > {}",
            value.len(),
            plan_limits.max_identifier_bytes
        )));
    }
    Ok(())
}

/// Ordinamento topologico deterministico (Kahn con ready-set ordinato
/// lessicograficamente) con rilevamento dei cicli e della profondità.
fn topological_order(plan: &PlanV4, plan_limits: &PlanLimits) -> Result<Vec<String>> {
    let node_ids: HashSet<&str> = plan.nodes.iter().map(|node| node.id.as_str()).collect();
    // indegree conta solo gli archi nodo -> nodo (gli input sono sorgenti).
    let mut indegree: BTreeMap<&str, usize> =
        plan.nodes.iter().map(|node| (node.id.as_str(), 0)).collect();
    let mut consumers: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in &plan.nodes {
        for reference in &node.inputs {
            if node_ids.contains(reference.as_str()) {
                indegree.entry(node.id.as_str()).and_modify(|d| *d += 1);
                consumers
                    .entry(reference.as_str())
                    .or_default()
                    .push(node.id.as_str());
            }
        }
    }

    let mut depth: HashMap<&str, usize> = HashMap::new();
    let mut ready: BTreeSet<&str> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut order = Vec::with_capacity(plan.nodes.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.to_owned());
        let node_depth = depth.get(id).copied().unwrap_or(1);
        if node_depth > plan_limits.max_plan_depth {
            return Err(contract_error(format!(
                "max_plan_depth superato: profondita' {node_depth} > {}",
                plan_limits.max_plan_depth
            )));
        }
        if let Some(children) = consumers.get(id) {
            for child in children {
                let child_depth = depth.entry(child).or_insert(1);
                *child_depth = (*child_depth).max(node_depth + 1);
                if let Some(degree) = indegree.get_mut(child) {
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert(child);
                    }
                }
            }
        }
    }

    if order.len() != plan.nodes.len() {
        return Err(contract_error(
            "il grafo contiene un ciclo: ordinamento topologico impossibile".into(),
        ));
    }
    Ok(order)
}

/// Insieme dei nodi antenati dell'output (l'output incluso, se è un nodo).
fn ancestors_of_output(plan: &PlanV4) -> HashSet<&str> {
    let by_id: HashMap<&str, &NodeV4> = plan
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut reachable = HashSet::new();
    let mut stack = vec![plan.output.as_str()];
    while let Some(id) = stack.pop() {
        if let Some(node) = by_id.get(id) {
            if reachable.insert(node.id.as_str()) {
                stack.extend(node.inputs.iter().map(String::as_str));
            }
        }
    }
    reachable
}

/// Serializzazione canonica del piano ai fini del `plan_hash` (ADR 4).
///
/// Regole applicate: chiavi ordinate (garantito da `serde_json::Map`), nodi
/// in ordine topologico deterministico (tie-break lessicografico), alias
/// sostituiti dagli id canonici, numeri della config normalizzati (`100` ≡
/// `100.0`, vedi [`canonical_numbers`]), default noti materializzati: limiti
/// effettivi completi e config omessa ≡ `{}`.
///
/// NON applicata (scelta v1): la materializzazione dei default DENTRO le
/// config per operazione — richiede schemi di config tipizzati per operazione,
/// che la v1 non ha (la config resta `serde_json::Value`, fuori scope). Una
/// config omessa e una con i default resi espliciti producono quindi piani
/// canonici diversi finche' gli schemi tipizzati non saranno introdotti.
///
/// Il piano si assume strutturalmente valido (prodotto da [`PlanV4::parse`] o
/// [`PlanV4::from_legacy`]); su un piano non valido l'ordinamento ricade su
/// quello lessicografico degli id, mantenendo la funzione totale.
#[must_use]
pub fn canonical_json(plan: &PlanV4) -> Value {
    let order = canonical_node_order(plan);
    let by_id: HashMap<&str, &NodeV4> = plan
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    let nodes: Vec<Value> = order
        .iter()
        .map(|id| {
            let node = by_id[id.as_str()];
            let canonical_op = find_operation(&node.op).map_or(node.op.as_str(), |d| d.id);
            let config = if node.config.is_null() {
                json!({})
            } else {
                canonical_numbers(&node.config)
            };
            json!({
                "id": node.id,
                "op": canonical_op,
                "in": node.inputs,
                "config": config,
            })
        })
        .collect();

    let mut root = Map::new();
    root.insert("schema_version".to_owned(), json!(PLAN_SCHEMA_VERSION_V4));
    root.insert("limits".to_owned(), canonical_limits(&plan.limits.effective()));
    if let Some(crs) = &plan.crs {
        root.insert("crs".to_owned(), json!(crs));
    }
    root.insert("inputs".to_owned(), json!(plan.inputs));
    root.insert("nodes".to_owned(), Value::Array(nodes));
    root.insert("output".to_owned(), json!(plan.output));
    Value::Object(root)
}

/// Limiti effettivi completamente materializzati (ADR 4: default espliciti).
fn canonical_limits(limits: &Limits) -> Value {
    json!({
        "max_input_rows": limits.rows.max_input_rows,
        "max_output_rows": limits.rows.max_output_rows,
        "max_rows_per_edge": limits.rows.max_rows_per_edge,
        "max_expansion_factor": limits.rows.max_expansion_factor,
        "plan": {
            "max_plan_json_bytes": limits.plan.max_plan_json_bytes,
            "max_plan_nodes": limits.plan.max_plan_nodes,
            "max_plan_edges": limits.plan.max_plan_edges,
            "max_plan_depth": limits.plan.max_plan_depth,
            "max_fan_out": limits.plan.max_fan_out,
            "max_inputs": limits.plan.max_inputs,
            "max_config_bytes_per_node": limits.plan.max_config_bytes_per_node,
            "max_identifier_bytes": limits.plan.max_identifier_bytes,
        },
        "max_memory_bytes": limits.max_memory_bytes,
        "max_temp_bytes": limits.max_temp_bytes,
        "spill_partitions": limits.spill_partitions,
        "max_parallelism": limits.max_parallelism,
        "max_wkb_cell_bytes": limits.max_wkb_cell_bytes,
        "max_payload_bytes": limits.max_payload_bytes,
        "max_batches": limits.max_batches,
        "max_geometry_depth": limits.max_geometry_depth,
        "max_string_bytes": limits.max_string_bytes,
        "max_regex_bytes": limits.max_regex_bytes,
    })
}

/// Massimo intero esattamente rappresentabile in `f64` (2^53): oltre questa
/// soglia un intero JSON puo' non avere un `f64` esatto e le forme intera e
/// in virgola mobile NON vanno unificate.
const MAX_EXACT_F64_INT: u64 = 1 << 53;

/// Normalizzazione ricorsiva dei numeri nella config (ADR 4): `100` e
/// `100.0` denotano lo stesso valore e devono produrre lo stesso piano
/// canonico. Ogni float a valore intero entro 2^53 (rappresentazione esatta
/// garantita) e' convertito all'intero corrispondente; i float con frazione
/// e i numeri oltre 2^53 mantengono la forma originale, cosi' valori
/// distinti non collassano mai (fail-closed).
fn canonical_numbers(value: &Value) -> Value {
    match value {
        Value::Number(number) => canonical_number(number),
        Value::Array(items) => Value::Array(items.iter().map(canonical_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), canonical_numbers(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Forma canonica di un numero: float a valore intero esattamente
/// rappresentabile (|v| <= 2^53) diventa intero; tutto il resto e' invariato.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
// I cast sono sicuri: guardati da `trunc` (valore intero) e da |v| <= 2^53.
fn canonical_number(number: &Number) -> Value {
    if let Some(float) = number.as_f64() {
        if float == float.trunc() && float.abs() <= MAX_EXACT_F64_INT as f64 {
            if float >= 0.0 {
                return Value::from(float as u64);
            }
            return Value::from(float as i64);
        }
    }
    Value::Number(number.clone())
}

/// Ordine topologico deterministico; su grafo invalido ricade
/// sull'ordinamento lessicografico degli id (funzione totale).
///
/// L'ordinamento NON dipende dai `PlanLimits`: un piano valido oltre i
/// default (parsing con limiti custom, es. profondita' maggiore) deve
/// canonicalizzare nello stesso ordine topologico, non ricadere sul
/// lessicografico. Il fallback resta solo per i grafi ciclici (piani non
/// passati per [`PlanV4::parse`]).
fn canonical_node_order(plan: &PlanV4) -> Vec<String> {
    let without_limits = PlanLimits {
        max_plan_json_bytes: usize::MAX,
        max_plan_nodes: usize::MAX,
        max_plan_edges: usize::MAX,
        max_plan_depth: usize::MAX,
        max_fan_out: usize::MAX,
        max_inputs: usize::MAX,
        max_config_bytes_per_node: usize::MAX,
        max_identifier_bytes: usize::MAX,
    };
    topological_order(plan, &without_limits).unwrap_or_else(|_| {
        let mut ids: Vec<String> = plan.nodes.iter().map(|node| node.id.clone()).collect();
        ids.sort_unstable();
        ids
    })
}

#[cfg(test)]
mod tests;
