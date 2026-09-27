//! Formati del piano DAG dichiarativo (architettura.md, piano-v5.md#identita-e-fingerprint, errori-e-limiti.md).
//!
//! Le versioni DAG sono **due** e non collassano: la **v5** qui, la **v6** in
//! [`formato_v6`] (aggiunge `max_domain_memory_bytes`, dominio d'identita'
//! proprio). La **v4** passa da [`migrazione_v4::testo_canonico_v5`] e
//! condivide il `plan_hash` della v5; i formati lineari sotto la `4` restano
//! al `table_engine`. L'ingresso comune e' [`valida_per_versione`].
//!
//! [`PlanV5::parse`] applica i [`PlanLimits`] durante il parsing, prima di
//! allocazioni guidate dal contenuto, poi la validazione strutturale e la
//! risoluzione alias. I `PlanLimits` arrivano dal chiamante e sono il tetto:
//! il blocco `limits` del piano puo' solo restringerli
//! ([`LimitsOverride::ensure_restringe`]), tranne il tetto sui byte, che si
//! applica prima di leggere il documento.
//!
//! [`canonical_json`] produce la forma del `plan_hash`. I default per
//! operazione dentro `config` non si materializzano: la config resta
//! `serde_json::Value` senza schemi tipizzati per operazione.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Number, Value};

use plenora_core::catalog::{find_operation, Arity, Family, Maturity};
use plenora_core::limits::{Limits, PlanLimits};
use plenora_core::{PlenoraError, Result};

use crate::table_engine;

pub mod migrazione_v4;

/// Versione CANONICA del formato piano DAG (architettura.md).
///
/// Il budget di memoria si chiama `max_governed_memory_bytes`
/// (errori-e-limiti.md#memoria-governata): governa la memoria che la
/// libreria controlla. Nessun alias col nome della v4, in nessuna delle due
/// direzioni; i piani v4 passano da [`migrazione_v4`].
pub const PLAN_SCHEMA_VERSION_V5: u16 = 5;

/// La v4, accettata **solo** dalla migrazione.
pub const PLAN_SCHEMA_VERSION_V4: u16 = 4;

/// Versione del formato che introduce `max_domain_memory_bytes`
/// (`Plan Budget 1.0`, `PLAN-002`).
///
/// **Non** e' la v5 con un campo in piu': e' un formato distinto, con il
/// proprio dominio di `plan_hash`. Vedi [`formato_v6`].
pub const PLAN_SCHEMA_VERSION_V6: u16 = 6;

/// Override opzionali dei limiti dati/runtime nel piano v5.
///
/// Specchio di [`Limits`] con tutti i campi opzionali: un piano dichiara solo
/// ciò che vuole restringere, i default di [`Limits::default`] coprono il
/// resto. «Restringere» è un vincolo imposto, non una convenzione: vedi
/// [`LimitsOverride::ensure_restringe`]. La sotto-sezione `plan` è applicata
/// al parsing del piano che la contiene ([`PlanV5::parse`]).
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
    pub max_governed_memory_bytes: Option<u64>,
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
        if let Some(value) = self.max_governed_memory_bytes {
            effective.max_governed_memory_bytes = value;
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

    /// Verifica che i limiti **di piano** dichiarati non superino quelli
    /// di chi esegue il parse (piano-v5.md#limiti-dichiarabili-nel-piano).
    ///
    /// Un documento non decide da se' quanto puo' costare interpretarlo. Sui
    /// limiti dati/runtime la base e' [`Limits::default`], non un tetto
    /// dell'host: dichiararli, anche piu' alti, e' il modo di configurare
    /// l'esecuzione, chiuso da [`Limits::validate`]. Che cosa questo non
    /// protegge per chi accetta piani non fidati e' in errori-e-limiti.md.
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` al primo campo che amplia la policy,
    /// nominando il campo, il valore chiesto e quello consentito.
    pub fn ensure_restringe(&self, base: &Limits) -> Result<()> {
        self.plan.ensure_restringe(&base.plan)
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

    /// Come [`LimitsOverride::ensure_restringe`], sui limiti di piano: sono
    /// tutti tetti monotoni, quindi «restringere» e' sempre «non superare».
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` al primo campo che amplia la base.
    pub fn ensure_restringe(&self, base: &PlanLimits) -> Result<()> {
        let campi: [(&str, Option<usize>, usize); 8] = [
            (
                "plan.max_plan_json_bytes",
                self.max_plan_json_bytes,
                base.max_plan_json_bytes,
            ),
            (
                "plan.max_plan_nodes",
                self.max_plan_nodes,
                base.max_plan_nodes,
            ),
            (
                "plan.max_plan_edges",
                self.max_plan_edges,
                base.max_plan_edges,
            ),
            (
                "plan.max_plan_depth",
                self.max_plan_depth,
                base.max_plan_depth,
            ),
            ("plan.max_fan_out", self.max_fan_out, base.max_fan_out),
            ("plan.max_inputs", self.max_inputs, base.max_inputs),
            (
                "plan.max_config_bytes_per_node",
                self.max_config_bytes_per_node,
                base.max_config_bytes_per_node,
            ),
            (
                "plan.max_identifier_bytes",
                self.max_identifier_bytes,
                base.max_identifier_bytes,
            ),
        ];
        for (nome, richiesto, consentito) in campi {
            if let Some(valore) = richiesto {
                if valore > consentito {
                    return Err(contract_error(format!(
                        "il piano puo' solo restringere i limiti: {nome} chiesto {valore}, \
                         consentito al massimo {consentito}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Nodo del DAG: solo dipendenze e configurazione (architettura.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeV5 {
    /// Identificatore unico nel piano.
    pub id: String,
    /// Id operazione: canonico (`table.*`/`geo.*`) o alias legacy; normalizzato
    /// all'id canonico durante la validazione.
    pub op: String,
    /// Riferimenti agli input dichiarati o ad altri nodi. Per le operazioni
    /// `BinaryOrdered` l'ordine è semantico: `[left, right]`.
    #[serde(default, rename = "in")]
    pub inputs: Vec<String>,
    /// Configurazione grezza: qui non è ancora tipizzata contro lo schema
    /// dell'operazione, lo diventa in `prepare`. `null` (omessa) equivale
    /// a `{}`.
    #[serde(default)]
    pub config: Value,
}

/// Piano v5: DAG dichiarativo (architettura.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanV5 {
    pub schema_version: u16,
    #[serde(default)]
    pub limits: LimitsOverride,
    /// CRS dichiarato a livello di piano; la risoluzione e la verifica dei
    /// requisiti per nodo non avvengono qui.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crs: Option<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Decisioni CRS esplicite del piano (R4.6.3): nome input → definizione
    /// CRS che risolve un'incoerenza dichiarata (`declared_unresolved`).
    ///
    /// E' la sola forma di risoluzione, coperta dal `plan_hash` e applicata al
    /// contratto prima della validazione. Una decisione su `missing`
    /// inventerebbe un CRS (R4.4), su `resolved` contraddirebbe il piano:
    /// entrambe sono errori.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub crs_decisions: BTreeMap<String, String>,
    pub nodes: Vec<NodeV5>,
    /// Id del nodo (o dell'input, per un piano pass-through) che produce
    /// l'output del piano.
    pub output: String,
}

/// Cio' che la validazione strutturale produce, **senza** il documento.
///
/// Nodi, archi, arieta' e regola di sola restrizione sono gli stessi per v5 e
/// v6; il documento resta separato, cosi' una variante non puo' rendere il
/// tipo dell'altra. L'inferenza dei contratti avviene nel planner.
#[derive(Debug, Clone)]
struct NucleoPianoValidato {
    /// Id dei nodi in ordine topologico deterministico (Kahn con tie-break
    /// lessicografico).
    topo_order: Vec<String>,
    /// I `PlanLimits` con cui il piano e' stato **effettivamente** validato.
    plan_limits: PlanLimits,
}

#[derive(Debug, Clone)]
pub struct ValidatedPlanV5 {
    /// Il documento v5 validato.
    plan: PlanV5,
    /// I `PlanLimits` con cui il piano e' stato **effettivamente** validato:
    /// quelli del chiamante, ristretti dal piano.
    ///
    /// La forma canonica li materializza: con i default della libreria, un
    /// piano accettato sotto una policy piu' larga dichiarerebbe vincoli che
    /// non rispetta, e il suo canonico si rifiuterebbe da solo.
    nucleo: NucleoPianoValidato,
}

impl ValidatedPlanV5 {
    #[must_use]
    pub const fn plan(&self) -> &PlanV5 {
        &self.plan
    }

    #[must_use]
    pub fn into_plan(self) -> PlanV5 {
        self.plan
    }

    /// Id dei nodi in ordine topologico deterministico.
    #[must_use]
    pub fn topological_order(&self) -> &[String] {
        &self.nucleo.topo_order
    }

    /// Limiti dati/runtime effettivi (override del piano sui default).
    ///
    /// La sotto-sezione `plan` riporta i limiti con cui il piano e' stato
    /// davvero validato, non i default della libreria.
    #[must_use]
    pub fn effective_limits(&self) -> Limits {
        let mut limits = self.plan.limits.effective();
        limits.plan = self.nucleo.plan_limits.clone();
        limits
    }

    /// I `PlanLimits` con cui il piano e' stato validato.
    #[must_use]
    pub const fn plan_limits(&self) -> &PlanLimits {
        &self.nucleo.plan_limits
    }

    /// Scompone il piano validato nel documento e nel nucleo.
    ///
    /// Serve al percorso v6, che valida la **struttura** attraverso la forma
    /// v5 condivisa e poi ricompone il proprio documento: senza questo,
    /// dovrebbe conservare un `PlanV5` che non e' il suo.
    pub(crate) fn in_parti(self) -> (PlanV5, NucleoPianoValidato) {
        (self.plan, self.nucleo)
    }

    /// Serializzazione canonica ai fini del `plan_hash` (piano-v5.md#identita-e-fingerprint).
    ///
    /// Materializza i limiti di piano contro i **default della libreria**: il
    /// `plan_hash` e' una proprieta' del piano e non dipende dalla policy del
    /// chiamante. Un piano accettato solo grazie a una policy piu' larga ha un
    /// canonico che i default rifiutano (dichiarato in piano-v5.md).
    #[must_use]
    pub fn canonical_json(&self) -> Value {
        canonical_json(&self.plan)
    }
}

/// Piano **v6** che ha superato parsing, limiti e validazione strutturale.
///
/// Il documento e' un [`formato_v6::PlanV6`] vero: versione e tetto sono suoi
/// campi, e nessun accessore rende l'uno senza l'altro. Un [`ValidatedPlanV5`]
/// con `schema_version` mutato sarebbe un documento che nessun parser
/// produce. Con la v5 condivide solo il [`NucleoPianoValidato`].
#[derive(Debug, Clone)]
pub struct ValidatedPlanV6 {
    plan: formato_v6::PlanV6,
    nucleo: NucleoPianoValidato,
}

impl ValidatedPlanV6 {
    pub(crate) const fn nuovo(plan: formato_v6::PlanV6, nucleo: NucleoPianoValidato) -> Self {
        Self { plan, nucleo }
    }

    /// Il documento v6 validato, tetto compreso.
    #[must_use]
    pub const fn plan(&self) -> &formato_v6::PlanV6 {
        &self.plan
    }

    /// Il tetto di memoria che il piano **richiede** per il proprio dominio.
    ///
    /// `None` quando il piano non lo dichiara: il profilo isolato non e'
    /// selezionabile, mai un tetto implicito (`PLAN-010`). E' il tetto
    /// richiesto, non quello concesso (`PLAN-014`).
    #[must_use]
    pub const fn max_domain_memory_bytes(&self) -> Option<u64> {
        self.plan.limits.max_domain_memory_bytes
    }

    /// Id dei nodi in ordine topologico deterministico.
    #[must_use]
    pub fn topological_order(&self) -> &[String] {
        &self.nucleo.topo_order
    }

    /// Limiti dati/runtime effettivi (override del piano sui default).
    #[must_use]
    pub fn effective_limits(&self) -> Limits {
        let mut limits = self.plan.limits.condivisi().effective();
        limits.plan = self.nucleo.plan_limits.clone();
        limits
    }

    /// I `PlanLimits` con cui il piano e' stato validato.
    #[must_use]
    pub const fn plan_limits(&self) -> &PlanLimits {
        &self.nucleo.plan_limits
    }

    /// Forma canonica **v6**: dichiara `schema_version: 6` e materializza il
    /// tetto del dominio quando il piano lo dichiara (`PLAN-017`).
    #[must_use]
    pub fn canonical_json(&self) -> Value {
        canonico(
            &self.plan.come_struttura_condivisa(),
            PLAN_SCHEMA_VERSION_V6,
            self.max_domain_memory_bytes(),
        )
    }
}

/// Un piano validato, con la propria versione di formato.
///
/// Le due versioni DAG non condividono un tipo che ne nasconda la
/// differenza. `non_exhaustive`: una versione nuova non rompe chi fa `match`
/// da fuori; chi non deve distinguere usa gli accessori, che delegano.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum PianoValidato {
    /// Un piano v5 — o un v4 migrato nel canonico v5, che ne condivide
    /// l'identita' (`PLAN-019`).
    V5(ValidatedPlanV5),
    /// Un piano v6, con il proprio dominio d'identita' (`PLAN-018`).
    V6(ValidatedPlanV6),
}

impl PianoValidato {
    /// La versione del formato **canonico** di questo piano: 5 o 6.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        match self {
            Self::V5(_) => PLAN_SCHEMA_VERSION_V5,
            Self::V6(_) => PLAN_SCHEMA_VERSION_V6,
        }
    }

    /// I nomi degli input dichiarati dal piano.
    ///
    /// Gli accessori sono per **campo** e non per documento: rendere un
    /// `&PlanV5` avrebbe significato, per la variante v6, o fabbricarne uno
    /// che il parser non ha prodotto, o mentire sul tipo.
    #[must_use]
    pub fn inputs(&self) -> &[String] {
        match self {
            Self::V5(piano) => &piano.plan().inputs,
            Self::V6(piano) => &piano.plan().inputs,
        }
    }

    /// L'id del nodo (o dell'input) che produce l'output del piano.
    #[must_use]
    pub fn output(&self) -> &str {
        match self {
            Self::V5(piano) => &piano.plan().output,
            Self::V6(piano) => &piano.plan().output,
        }
    }

    /// I nodi del piano.
    #[must_use]
    pub fn nodes(&self) -> &[NodeV5] {
        match self {
            Self::V5(piano) => &piano.plan().nodes,
            Self::V6(piano) => &piano.plan().nodes,
        }
    }

    /// Il CRS dichiarato a livello di piano.
    #[must_use]
    pub const fn crs(&self) -> Option<&String> {
        match self {
            Self::V5(piano) => piano.plan().crs.as_ref(),
            Self::V6(piano) => piano.plan().crs.as_ref(),
        }
    }

    /// Le decisioni CRS esplicite del piano.
    #[must_use]
    pub const fn crs_decisions(&self) -> &BTreeMap<String, String> {
        match self {
            Self::V5(piano) => &piano.plan().crs_decisions,
            Self::V6(piano) => &piano.plan().crs_decisions,
        }
    }

    /// Id dei nodi in ordine topologico deterministico.
    #[must_use]
    pub fn topological_order(&self) -> &[String] {
        match self {
            Self::V5(piano) => piano.topological_order(),
            Self::V6(piano) => piano.topological_order(),
        }
    }

    /// Limiti effettivi del piano.
    #[must_use]
    pub fn effective_limits(&self) -> Limits {
        match self {
            Self::V5(piano) => piano.effective_limits(),
            Self::V6(piano) => piano.effective_limits(),
        }
    }

    /// I `PlanLimits` con cui il piano e' stato validato.
    #[must_use]
    pub const fn plan_limits(&self) -> &PlanLimits {
        match self {
            Self::V5(piano) => piano.plan_limits(),
            Self::V6(piano) => piano.plan_limits(),
        }
    }

    /// Il tetto del dominio, che solo un v6 puo' dichiarare.
    #[must_use]
    pub const fn max_domain_memory_bytes(&self) -> Option<u64> {
        match self {
            Self::V5(_) => None,
            Self::V6(piano) => piano.max_domain_memory_bytes(),
        }
    }

    /// La **proiezione strutturale** condivisa, per uso interno al crate.
    ///
    /// Per la v5 e' il documento stesso, prestato; per la v6 una copia che
    /// dichiara 5, perche' e' una proiezione. `pub(crate)`: da fuori un
    /// `PlanV5` ricavato da un v6 darebbe un `plan_hash` del dominio sbagliato.
    pub(crate) fn struttura_condivisa(&self) -> std::borrow::Cow<'_, PlanV5> {
        match self {
            Self::V5(piano) => std::borrow::Cow::Borrowed(piano.plan()),
            Self::V6(piano) => std::borrow::Cow::Owned(piano.plan().come_struttura_condivisa()),
        }
    }

    /// La forma canonica **della propria versione**.
    ///
    /// E' il solo punto da cui il canonico di un piano validato si ottiene, e
    /// per costruzione non puo' produrre la forma sbagliata per la versione
    /// sbagliata.
    #[must_use]
    pub fn canonical_json(&self) -> Value {
        match self {
            Self::V5(piano) => piano.canonical_json(),
            Self::V6(piano) => piano.canonical_json(),
        }
    }
}

/// Dispatch per versione: **una sola lettura decide il percorso**.
///
/// Il parser scelto riverifica il proprio formato, perche' `PlanV5::parse`
/// non deve accettare un v6 se chiamato direttamente. Limite sui byte e
/// chiavi duplicate vengono prima della lettura: con una `schema_version`
/// duplicata `serde_json` farebbe vincere l'ultima.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per un piano oltre `max_plan_json_bytes`, con
/// chiavi duplicate, senza `schema_version` leggibile, o con una versione che
/// non ha un percorso DAG; piu' gli errori del parser scelto.
pub fn valida_per_versione(json_text: &str, plan_limits: &PlanLimits) -> Result<PianoValidato> {
    if json_text.len() > plan_limits.max_plan_json_bytes {
        return Err(contract_error(format!(
            "max_plan_json_bytes superato: {} byte > {}",
            json_text.len(),
            plan_limits.max_plan_json_bytes
        )));
    }
    plenora_core::json::ensure_no_duplicate_keys(json_text)?;
    match migrazione_v4::versione_dichiarata(json_text)? {
        PLAN_SCHEMA_VERSION_V5 => Ok(PianoValidato::V5(PlanV5::parse(json_text, plan_limits)?)),
        PLAN_SCHEMA_VERSION_V6 => Ok(PianoValidato::V6(formato_v6::PlanV6::parse(
            json_text,
            plan_limits,
        )?)),
        PLAN_SCHEMA_VERSION_V4 => {
            // La v4 e' migrata NEL canonico v5 e ne condivide il `plan_hash`
            // (`PLAN-019`): non ha un dominio proprio, e non deve averlo.
            let migrato = migrazione_v4::testo_canonico_v5(json_text, plan_limits)?;
            Ok(PianoValidato::V5(PlanV5::parse(
                migrato.as_ref(),
                plan_limits,
            )?))
        }
        legacy if legacy <= 3 => Err(contract_error(format!(
            "`schema_version: {legacy}` e' un piano lineare legacy: non ha un percorso DAG diretto, va convertito dal chiamante legacy"
        ))),
        altra => Err(contract_error(format!(
            "`schema_version: {altra}` non e' supportata: le versioni DAG sono {PLAN_SCHEMA_VERSION_V5} e {PLAN_SCHEMA_VERSION_V6}, la {PLAN_SCHEMA_VERSION_V4} passa dalla migrazione"
        ))),
    }
}

const fn contract_error(message: String) -> PlenoraError {
    PlenoraError::InvalidPlan(message)
}

impl PlanV5 {
    /// Parse completo di un piano v5: limiti durante il parsing (errori-e-limiti.md),
    /// validazione strutturale, risoluzione alias.
    ///
    /// # Errors
    ///
    /// Restituisce `PlenoraError::InvalidPlan` per ogni violazione di limiti o
    /// di struttura, `PlenoraError::DataMapping` per JSON malformato,
    /// `PlenoraError::Unsupported` per operazioni catalogate ma non
    /// disponibili (`Maturity::Planned`).
    pub fn parse(json_text: &str, plan_limits: &PlanLimits) -> Result<ValidatedPlanV5> {
        // errori-e-limiti.md: il limite sui byte del JSON si applica PRIMA del parse.
        if json_text.len() > plan_limits.max_plan_json_bytes {
            return Err(contract_error(format!(
                "max_plan_json_bytes superato: {} byte > {}",
                json_text.len(),
                plan_limits.max_plan_json_bytes
            )));
        }
        // Le chiavi duplicate si rifiutano PRIMA della deserializzazione:
        // `serde_json` le risolverebbe con «vince l'ultima», e la risoluzione
        // avverrebbe prima della validazione e prima del `plan_hash` — due
        // testi diversi con lo stesso piano canonico e lo stesso hash.
        plenora_core::json::ensure_no_duplicate_keys(json_text)?;
        let plan: Self = serde_json::from_str(json_text)?;
        // La v5 pretende la propria versione, e anche `validate_structure`
        // la pretende: un `PlanV5` dichiara 5 ovunque, compreso qui dentro.
        // Il controllo e' doppio di proposito — questo dice «il documento non
        // e' un v5», quello dice «la struttura non e' un v5» — e il secondo
        // e' l'invariante che regge se qualcuno costruisse un `PlanV5` a mano
        // invece di deserializzarlo.
        if plan.schema_version != PLAN_SCHEMA_VERSION_V5 {
            return Err(contract_error(format!(
                "schema_version {} non e' un piano v5",
                plan.schema_version
            )));
        }
        Self::valida_struttura_condivisa(plan, plan_limits)
    }

    /// Limiti effettivi e validazione strutturale, **condivisi** fra v5 e v6.
    ///
    /// Una sola validazione di nodi, archi, arieta', alias e sola restrizione.
    /// Non controlla la versione: lo fa il parser di ciascun formato.
    pub(crate) fn valida_struttura_condivisa(
        mut plan: Self,
        plan_limits: &PlanLimits,
    ) -> Result<ValidatedPlanV5> {
        // Il blocco `limits` puo' solo RESTRINGERE la policy di chi esegue
        // (piano-v5.md#limiti-dichiarabili-nel-piano). La base e' quella del
        // chiamante per i limiti di piano e quella della libreria per i
        // limiti dati/runtime.
        let base = Limits {
            plan: plan_limits.clone(),
            ..Limits::default()
        };
        plan.limits.ensure_restringe(&base)?;
        // ...e, una volta accertato che restringono, i limiti di piano
        // dichiarati governano DAVVERO il piano che li dichiara. Senza questa
        // riga finirebbero nella forma canonica, e quindi nel `plan_hash`,
        // senza che nulla li applichi: l'identita' del piano affermerebbe una
        // proprieta' che il parser non ha verificato.
        let effective_plan_limits = plan.limits.plan.apply_to(plan_limits);
        // `max_plan_json_bytes` non si applica al documento che lo dichiara:
        // un tetto sul testo precede la lettura del testo. Riapplicarlo dopo
        // rifiuterebbe la forma canonica, che materializza tutti i limiti ed
        // e' sempre piu' grande del documento compatto. Resta soggetto alla
        // sola restrizione, e nel canonico entra il valore dichiarato: il
        // tetto del chiamante cambierebbe il `plan_hash`.
        let topo_order = plan.validate_structure(&effective_plan_limits)?;
        Ok(ValidatedPlanV5 {
            plan,
            nucleo: NucleoPianoValidato {
                topo_order,
                plan_limits: effective_plan_limits,
            },
        })
    }

    /// Parse con i `PlanLimits` di default (fail-closed).
    ///
    /// # Errors
    /// Come [`PlanV5::parse`].
    pub fn parse_default(json_text: &str) -> Result<ValidatedPlanV5> {
        Self::parse(json_text, &PlanLimits::default())
    }

    /// Migrazione del piano lineare legacy (`Plan{steps}`, `schema_version`
    /// <= 3) nel caso degenerato del DAG: ogni step diventa un nodo il cui
    /// unico `in` è il nodo precedente (o l'unico input); il singolo step
    /// binario del protocollo a due input diventa un nodo con
    /// `in = ["left", "right"]`.
    ///
    /// Deterministica (id `n0..nN`, input `main`/`left`/`right` fissi) e
    /// idempotente (decisione D20, piano-v5.md#identita-e-fingerprint): il risultato è già un piano v5
    /// validato con i limiti di default.
    ///
    /// # Errors
    ///
    /// Restituisce `PlenoraError::InvalidPlan` se il piano legacy è vuoto, ha
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
            return Err(contract_error("il piano legacy non contiene passi".into()));
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
            .map(|(index, (step, descriptor))| NodeV5 {
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
        let limits = legacy_limits_override(&legacy.limits)?;

        let mut plan = Self {
            schema_version: PLAN_SCHEMA_VERSION_V5,
            limits,
            crs: None,
            inputs: plan_inputs,
            crs_decisions: BTreeMap::new(),
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
    // Validazione strutturale in un'unica passata: la lunghezza e' data
    // dalla sequenza lineare dei controlli sul contratto del piano, non da
    // complessita' logica, e spezzarla in funzioni artificiali
    // peggiorerebbe solo la leggibilita'.
    #[allow(clippy::too_many_lines)]
    fn validate_structure(&mut self, plan_limits: &PlanLimits) -> Result<Vec<String>> {
        // Un `PlanV5` dichiara **5**, anche qui dentro: la v6 si proietta
        // esplicitamente in una struttura v5, quindi nemmeno il codice interno
        // costruisce un `ValidatedPlanV5` che afferma di essere un v6.
        if self.schema_version != PLAN_SCHEMA_VERSION_V5 {
            return Err(contract_error(format!(
                "schema_version {} non supportata; attesa {PLAN_SCHEMA_VERSION_V5}",
                self.schema_version
            )));
        }

        // --- Conteggi e byte (errori-e-limiti.md: prima delle validazioni guidate dal
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
        // R4.6.3: le decisioni CRS devono nominare input dichiarati e
        // portare una definizione non vuota (la risoluzione e l'applicazione
        // allo stato `declared_unresolved` sono nel perimetro della CLI,
        // dove piano e contratti scoperti si incontrano).
        for (input, definition) in &self.crs_decisions {
            check_identifier("input di crs_decisions", input, plan_limits)?;
            if !self.inputs.iter().any(|declared| declared == input) {
                return Err(contract_error(format!(
                    "crs_decisions dichiara una decisione per `{input}`, che non e' \
                     tra gli input del piano"
                )));
            }
            if definition.trim().is_empty() {
                return Err(contract_error(format!(
                    "crs_decisions: definizione CRS vuota per l'input `{input}`"
                )));
            }
        }
        check_identifier("output", &self.output, plan_limits)?;
        for node in &mut self.nodes {
            check_identifier("id nodo", &node.id, plan_limits)?;
            check_identifier("operazione", &node.op, plan_limits)?;
            // Config omessa (null) equivale a oggetto vuoto (piano-v5.md#identita-e-fingerprint: config
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
            // Alias sostituito dall'id canonico (piano-v5.md#identita-e-fingerprint).
            node.op = descriptor.id.to_owned();
        }

        // --- Riferimenti esistenti e fan-out ---
        let mut fan_out: HashMap<&str, usize> = HashMap::new();
        for node in &self.nodes {
            for reference in &node.inputs {
                if !input_names.contains(reference.as_str())
                    && !node_ids.contains(reference.as_str())
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

/// Mappa i limiti legacy (`plenora_kernels_table::Limits`) sugli override v5.
/// Conversione totale (R5.4): i campi `usize` della config legacy possono
/// superare `u32`/`u64` solo per errori di configurazione, che vanno
/// rifiutati e non troncati.
fn legacy_limits_override(legacy: &table_engine::Limits) -> Result<LimitsOverride> {
    let max_rows = legacy.max_rows as u64;
    Ok(LimitsOverride {
        max_input_rows: Some(max_rows),
        max_output_rows: Some(max_rows),
        max_rows_per_edge: Some(max_rows),
        max_governed_memory_bytes: Some(legacy.max_governed_memory_bytes as u64),
        max_temp_bytes: Some(legacy.max_temp_bytes),
        spill_partitions: Some(
            u32::try_from(legacy.spill_partitions)
                .map_err(|_| contract_error("spill_partitions legacy oltre il range u32".into()))?,
        ),
        max_string_bytes: Some(legacy.max_string_bytes),
        max_regex_bytes: Some(legacy.max_regex_bytes),
        ..LimitsOverride::default()
    })
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
fn topological_order(plan: &PlanV5, plan_limits: &PlanLimits) -> Result<Vec<String>> {
    let node_ids: HashSet<&str> = plan.nodes.iter().map(|node| node.id.as_str()).collect();
    // indegree conta solo gli archi nodo -> nodo (gli input sono sorgenti).
    let mut indegree: BTreeMap<&str, usize> = plan
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), 0))
        .collect();
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
fn ancestors_of_output(plan: &PlanV5) -> HashSet<&str> {
    let by_id: HashMap<&str, &NodeV5> = plan
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

/// Serializzazione canonica del piano ai fini del `plan_hash` (piano-v5.md#identita-e-fingerprint).
///
/// Regole applicate: chiavi ordinate (garantito da `serde_json::Map`), nodi
/// in ordine topologico deterministico (tie-break lessicografico), alias
/// sostituiti dagli id canonici, numeri della config normalizzati (`100` ≡
/// `100.0`, vedi [`canonical_numbers`]), default noti materializzati: limiti
/// effettivi completi e config omessa ≡ `{}`.
///
/// I default dentro le config per operazione non si materializzano: una
/// config omessa e una con i default espliciti danno canonici diversi. Su un
/// piano non valido l'ordinamento ricade su quello lessicografico degli id,
/// e la funzione resta totale.
#[must_use]
pub fn canonical_json(plan: &PlanV5) -> Value {
    canonico(plan, PLAN_SCHEMA_VERSION_V5, None)
}

/// Il costruttore vero della forma canonica, **privato**.
///
/// Versione e tetto sono parametri perche' i due formati li scelgono
/// diversamente; esporlo permetterebbe combinazioni che nessun parser
/// produce. Chiamanti: [`canonical_json`] (v5) e
/// [`ValidatedPlanV6::canonical_json`] (v6).
fn canonico(plan: &PlanV5, versione: u16, tetto: Option<u64>) -> Value {
    let plan_limits = plan.limits.plan.apply_to(&PlanLimits::default());
    let order = canonical_node_order(plan);
    let by_id: HashMap<&str, &NodeV5> = plan
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
    root.insert("schema_version".to_owned(), json!(versione));
    let mut limiti_effettivi = plan.limits.effective();
    limiti_effettivi.plan = plan_limits;
    let mut limiti = canonical_limits(&limiti_effettivi);
    // `PLAN-017`: il campo entra nel canonico — e quindi nel `plan_hash` —
    // **solo quando e' presente**. Un v6 che lo dichiara e uno che lo omette
    // sono piani diversi; un v5 non lo dichiara mai, e i suoi byte restano
    // quelli di prima.
    if let (Some(tetto), Some(oggetto)) = (tetto, limiti.as_object_mut()) {
        oggetto.insert("max_domain_memory_bytes".to_owned(), json!(tetto));
    }
    root.insert("limits".to_owned(), limiti);
    if let Some(crs) = &plan.crs {
        root.insert("crs".to_owned(), json!(crs));
    }
    root.insert("inputs".to_owned(), json!(plan.inputs));
    // R4.6.3: le decisioni CRS fanno parte dell'identita' del piano (piano-v5.md#identita-e-fingerprint)
    // — una decisione diversa e' un piano diverso. Mappa ordinata per
    // costruzione (BTreeMap), assente quando vuota (piani esistenti
    // invariati).
    if !plan.crs_decisions.is_empty() {
        root.insert("crs_decisions".to_owned(), json!(plan.crs_decisions));
    }
    root.insert("nodes".to_owned(), Value::Array(nodes));
    root.insert("output".to_owned(), json!(plan.output));
    Value::Object(root)
}

/// Limiti effettivi completamente materializzati (piano-v5.md#identita-e-fingerprint: default espliciti).
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
        "max_governed_memory_bytes": limits.max_governed_memory_bytes,
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

/// Normalizzazione ricorsiva dei numeri nella config (piano-v5.md#identita-e-fingerprint): `100` e
/// `100.0` denotano lo stesso valore e devono produrre lo stesso piano
/// canonico. Ogni float a valore intero entro 2^53 (rappresentazione esatta
/// garantita) e' convertito all'intero corrispondente; i float con frazione
/// e i float oltre 2^53 mantengono la forma originale, cosi' valori
/// distinti non collassano mai (fail-closed). Gli interi JSON sono gia'
/// canonici e passano invariati a qualunque magnitudo, `u64::MAX` incluso.
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
///
/// Solo i numeri in virgola mobile si canonicalizzano: un `i64`/`u64` fatto
/// passare per `f64` perde le cifre oltre 2^53 (2^53+1 diventerebbe 2^53), e
/// due config diverse avrebbero lo stesso `plan_hash`
/// (architettura.md#determinismo).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]
// I cast sono sicuri: guardati da `trunc` (valore intero) e da |v| <= 2^53.
// Il confronto `float == float.trunc()` e' esatto volutamente: la
// canonicalizzazione del plan_hash (piano-v5.md#identita-e-fingerprint) richiede uguaglianza per bit.
fn canonical_number(number: &Number) -> Value {
    if number.is_i64() || number.is_u64() {
        return Value::Number(number.clone());
    }
    if let Some(float) = number.as_f64() {
        // `-0.0` NON si canonicalizza sull'intero zero: il bit di segno e'
        // osservabile a valle — `table.align_schema` lo conserva nel default
        // di una colonna Float64 — quindi due config distinguibili
        // collasserebbero sullo stesso piano canonico e sullo stesso
        // `plan_hash`. Resta un float, come ogni altro valore che l'intero
        // non rappresenta.
        let zero_negativo = float == 0.0 && float.is_sign_negative();
        if !zero_negativo && float == float.trunc() && float.abs() <= MAX_EXACT_F64_INT as f64 {
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
/// L'ordinamento non dipende dai `PlanLimits`: un piano valido oltre i
/// default canonicalizza nello stesso ordine topologico. Il fallback resta
/// per i grafi ciclici.
fn canonical_node_order(plan: &PlanV5) -> Vec<String> {
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

pub mod formato_v6;

#[cfg(test)]
mod tests;
