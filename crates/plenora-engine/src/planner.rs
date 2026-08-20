//! Planner del DAG v5 — fase 1 `validate` (Architetture.md par. 6.1/6.2,
//! ADR 4, ADR 5) — Fase 2A-3.
//!
//! [`validate`] e' una funzione pura e a secco: legge il piano JSON e i
//! contratti di input (schemi Arrow dagli header IPC, nessuna riga di dati) e
//! produce un [`ValidatedGraph`] immutabile contenente **solo decisioni
//! semantiche stabili** — struttura, tipi, CRS, ordini dichiarati, identita'
//! (ADR 4). Nessuna decisione fisica: `prepare`/`ExecutionPlan` sono Fase 2A-4
//! (ADR 5) e NON sono implementati qui.
//!
//! Passi (Architetture.md par. 6.1):
//!
//! 1. `PlanLimits` di default durante il parsing (ADR 6), poi validazione
//!    strutturale e risoluzione alias — in [`PlanV5::parse`];
//! 2. risoluzione del CRS di piano (campo `crs`): feature-dispatch come
//!    `geo_transport` — con `proj-backend` la risoluzione PROJ reale, senza
//!    fail-closed `CRS_BACKEND_UNAVAILABLE`;
//! 3. verifica delle `required_capabilities` di ogni op contro i backend
//!    compilati (`geos-backend`/`proj-backend`): un'op senza backend fallisce
//!    qui, non a meta' esecuzione;
//! 4. inferenza dei `DataContract` arco per arco in ordine topologico
//!    (`analyze_table_contract` / `analyze_geo_contract` per famiglia), con un
//!    unico [`FieldAllocator`] per grafo: i `FieldId` delle geometrie di input
//!    sono **rimappati all'ingresso** nel namespace globale del grafo
//!    (decisione D16: due input non possono collidere);
//! 5. costruzione dell'identita' (ADR 4): `plan_hash` (SHA-256 del piano
//!    canonico serializzato stabile), `catalog_fingerprint` (hash dei
//!    descrittori delle op usate, in ordine stabile, con le quattro versioni
//!    per-componente), `engine_version`, `arrow_version`,
//!    `required_capabilities`, `input_contract_fingerprints`,
//!    `plan_format_version`.
//!
//! Scelte v1 documentate:
//!
//! - **Rimappatura dei `FieldId` di input**: gli id nei contratti di input
//!   sono ignorati; ogni colonna geometrica di input riceve un id fresco dal
//!   namespace del grafo (in ordine di dichiarazione degli input) e il nome
//!   e' legato al nuovo id nell'allocatore, cosi' un `intern` successivo sul
//!   nome della geometria resta coerente. Di conseguenza una proprieta'
//!   `sorted_by` con chiavi su un contratto di input e' rifiutata (fail-closed):
//!   le chiavi `FieldId` del chiamante non sono riferibili a colonne nel
//!   namespace del grafo (punto aperto: trasportare le chiavi per nome);
//! - **`input_contract_fingerprints`**: hash di schema + geometria
//!   (serializzazione stabile), `FieldId` esclusi perche' identita' interna
//!   del grafo, non dell'input; il CRS entra con definizione, tipo e unita'
//!   lineare — due definizioni testualmente diverse dello stesso CRS producono
//!   fingerprint diversi (conservativo, fail-closed) — e lo stato `missing`
//!   (R4.6.3) entra come valore canonico: un contratto senza CRS non ha lo
//!   stesso fingerprint di uno con CRS risolto;
//! - **profilo di publish**: il formato piano non dichiara ancora un
//!   profilo (`AtomicPublish`/`DurableAtomicPublish`, ADR 7); finche' non lo
//!   fara', il default `AtomicPublish` entra nelle `required_capabilities`
//!   qui raccolte ed e' verificato da [`check_compatibility`] contro le
//!   capability dell'ambiente, come i backend — senza cambi di API;
//! - fingerprint e hash usano serializzazioni JSON stabili (chiavi ordinate);
//!   i tipi Arrow entrano con la loro forma `Debug`: i fingerprint vivono solo
//!   in memoria nella v1 (ADR 4, serializzazione persistente rimandata), la
//!   stabilita' cross-build non e' richiesta.
//!
//! # Type-state
//!
//! [`ValidatedGraph`] non ha costruttori pubblici: si ottiene solo da
//! [`validate`]. La futura `execute` (Fase 2A-4) accettera' esclusivamente
//! `&ValidatedGraph` — nessun percorso non validato puo' raggiungere
//! l'esecuzione (ADR 5).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use plenora_core::catalog::{
    find_operation, Arity, CancellationBehavior, CrsRequirement, DeterminismPolicy, ExecutionClass,
    ExpansionConstraint, Family, Maturity, OperationDescriptor, Origin, ResultShape,
    SourceRowProvenance,
};
use plenora_core::contract::{
    ContractCrs, DataContract, FieldAllocator, PropertyConfidence, PropertyScope,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_core::limits::{Limits, PlanLimits};
use plenora_core::{PlenoraError, Result};
use plenora_kernels_geo::analyze::analyze_geo_contract;
use plenora_kernels_table::analyze::analyze_table_contract;

// Feature-dispatch come `geo_transport::publish`: senza `proj-backend` la
// risoluzione fallisce chiusa (`CRS_BACKEND_UNAVAILABLE`).
#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

use crate::geo_transport::publish::PublishProfile;
use crate::plan::{migrazione_v4, PlanV5, ValidatedPlanV5, PLAN_SCHEMA_VERSION_V5};

#[cfg(test)]
mod tests;

/// Versione dell'engine che ha prodotto il grafo validato (ADR 4).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Separatore di dominio del `plan_hash` (ADR 4, esteso da ADR 15).
///
/// Il `plan_hash` non e' piu' `SHA256(canonical_json)` ma
/// `SHA256(dominio || canonical_json)`. Il dominio nomina la versione del
/// formato canonico, e questo **invalida esplicitamente** ogni hash prodotto
/// prima della v5.
///
/// Senza di esso l'invalidazione sarebbe soltanto probabile: il piano
/// canonico v5 nomina il budget di memoria diversamente dalla v4 (vedi
/// [`crate::plan::migrazione_v4`]), quindi in pratica ogni hash cambierebbe
/// comunque — ma «in pratica» e' una proprieta' del contenuto, non una
/// garanzia della funzione. Un grafo riusato per sbaglio con un hash di prima
/// e' un piano eseguito sotto un contratto di memoria che non e' piu' il suo.
///
/// Che cosa il dominio garantisce, con precisione: che **nessun testo
/// canonico** possa produrre, sotto la regola nuova, l'hash che un testo
/// canonico produceva sotto quella vecchia — i due insiemi di input della
/// funzione di hash sono disgiunti, perche' uno ha il prefisso e l'altro no.
/// Resta la resistenza alle collisioni di SHA-256, che e' l'assunzione su cui
/// il `plan_hash` poggiava gia' prima: il dominio **separa** gli spazi, non
/// abolisce la crittografia.
///
/// La difesa vera contro il riuso di un grafo validato sotto un'altra
/// versione del formato non e' l'hash ma il confronto esplicito di
/// `plan_format_version` in [`check_compatibility`].
const PLAN_HASH_DOMAIN: &[u8] = b"plenora/plan_hash/v5\0";

/// Versione dei crate Arrow in questa build (ADR 4: entra nell'identita' del
/// grafo — un grafo validato con una versione Arrow diversa non e' riusabile).
pub const ARROW_VERSION: &str = plenora_core::arrow::VERSION;

/// Capability disponibili in questa build (backend compilati).
///
/// I profili di publish non dipendono dalle feature di compilazione (ADR 7):
/// NON sono inclusi qui — per l'ambiente locale completo (backend + profili
/// di publish implementati dall'engine) si usi [`local_capabilities`]; un
/// ambiente diverso dichiara le proprie capability al chiamante di
/// [`check_compatibility`].
#[must_use]
pub fn compiled_capabilities() -> CapabilitySet {
    let mut set = CapabilitySet::default();
    if cfg!(feature = "geos-backend") {
        set.insert("geos");
    }
    if cfg!(feature = "proj-backend") {
        set.insert("proj");
    }
    set
}

/// Capability dell'ambiente locale: backend compilati piu' i profili di
/// publish implementati dall'engine (ADR 7).
///
/// Entrambi i profili sono inclusi; il riconoscimento fail-closed del
/// filesystem di destinazione resta al momento del publish, non e' una
/// capability statica.
///
/// E' l'insieme contro cui `execute` riverifica l'identita' del grafo.
#[must_use]
pub fn local_capabilities() -> CapabilitySet {
    let mut set = compiled_capabilities();
    set.insert(PublishProfile::Atomic.capability_name());
    set.insert(PublishProfile::DurableAtomic.capability_name());
    set
}

/// Insieme ordinato di capability (`geos`, `proj`, profili di publish).
///
/// L'ordine lessicografico rende stabili serializzazione e confronti (ADR 4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapabilitySet(BTreeSet<String>);

impl CapabilitySet {
    /// Insieme da iteratore di nomi.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        names
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into()
    }

    /// Inserisce una capability.
    pub fn insert(&mut self, name: &str) {
        self.0.insert(name.to_owned());
    }

    /// `true` se la capability e' presente.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    /// Nomi in ordine lessicografico.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    /// `true` se vuoto.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<BTreeSet<String>> for CapabilitySet {
    fn from(set: BTreeSet<String>) -> Self {
        Self(set)
    }
}

/// Hash SHA-256 del piano canonico migrato (ADR 4).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanHash([u8; 32]);

/// Fingerprint del catalogo ristretto alle op usate dal piano (ADR 4).
///
/// Deriva dai descrittori serializzati in ordine stabile con le loro
/// versioni esplicite — mai da hash del binario.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CatalogFingerprint([u8; 32]);

/// Fingerprint di un `DataContract` di input: schema + geometria (ADR 4).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContractFingerprint([u8; 32]);

macro_rules! impl_hash_newtype {
    ($type:ident) => {
        impl $type {
            /// I 32 byte grezzi dell'hash.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }

            /// Rappresentazione esadecimale minuscola.
            #[must_use]
            pub fn to_hex(&self) -> String {
                self.0.iter().map(|byte| format!("{byte:02x}")).collect()
            }
        }

        impl fmt::Debug for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, concat!(stringify!($type), "({})"), self.to_hex())
            }
        }

        impl fmt::Display for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.to_hex())
            }
        }
    };
}

impl_hash_newtype!(PlanHash);
impl_hash_newtype!(CatalogFingerprint);
impl_hash_newtype!(ContractFingerprint);

/// Versione dell'engine che ha validato il grafo (ADR 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineVersion(pub String);

impl fmt::Display for EngineVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Versione dei crate Arrow della build che ha validato il grafo (ADR 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrowVersion(pub String);

impl fmt::Display for ArrowVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Grafo validato: il solo ingresso ammesso alla futura `execute` (ADR 5).
///
/// Contiene esclusivamente decisioni semantiche stabili (struttura, tipi,
/// CRS, ordini dichiarati) e l'identita' ADR 4. Nessun costruttore pubblico:
/// si costruisce solo tramite [`validate`]. Immutabile per convenzione di
/// API: tutti gli accessor restituiscono riferimenti condivisi.
#[derive(Debug)]
pub struct ValidatedGraph {
    // --- Identita' (ADR 4) ---
    plan_hash: PlanHash,
    catalog_fingerprint: CatalogFingerprint,
    engine_version: EngineVersion,
    arrow_version: ArrowVersion,
    required_capabilities: CapabilitySet,
    /// Allineati per posizione a `plan.inputs`.
    input_contract_fingerprints: Vec<ContractFingerprint>,
    plan_format_version: u16,
    // --- Decisioni semantiche stabili ---
    plan: ValidatedPlanV5,
    /// Contratti per arco, chiave = nome input o id nodo (namespace unico,
    /// garantito dalla validazione strutturale).
    edge_contracts: BTreeMap<String, DataContract>,
    /// Ids delle op usate, in ordine lessicografico (per il confronto del
    /// fingerprint in [`check_compatibility`]).
    used_operations: Vec<String>,
    effective_limits: Limits,
    plan_crs: Option<ResolvedCrs>,
}

impl ValidatedGraph {
    /// Hash canonico del piano migrato (ADR 4).
    #[must_use]
    pub const fn plan_hash(&self) -> PlanHash {
        self.plan_hash
    }

    /// Fingerprint del catalogo ristretto alle op usate (ADR 4).
    #[must_use]
    pub const fn catalog_fingerprint(&self) -> CatalogFingerprint {
        self.catalog_fingerprint
    }

    /// Versione dell'engine che ha validato il grafo.
    #[must_use]
    pub const fn engine_version(&self) -> &EngineVersion {
        &self.engine_version
    }

    /// Versione Arrow della build che ha validato il grafo.
    #[must_use]
    pub const fn arrow_version(&self) -> &ArrowVersion {
        &self.arrow_version
    }

    /// Capability richieste dal grafo: backend delle op usate e profilo di
    /// publish (ADR 7 — default `AtomicPublish` finche' il formato piano non
    /// dichiara un profilo).
    #[must_use]
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Fingerprint dei contratti di input, allineati a `plan.inputs`.
    #[must_use]
    pub fn input_contract_fingerprints(&self) -> &[ContractFingerprint] {
        &self.input_contract_fingerprints
    }

    /// Versione del formato piano (4).
    #[must_use]
    pub const fn plan_format_version(&self) -> u16 {
        self.plan_format_version
    }

    /// Il piano validato strutturalmente (alias gia' risolti agli id canonici).
    #[must_use]
    pub const fn plan(&self) -> &ValidatedPlanV5 {
        &self.plan
    }

    /// Id dei nodi in ordine topologico deterministico.
    #[must_use]
    pub fn topological_order(&self) -> &[String] {
        self.plan.topological_order()
    }

    /// Limiti dati/runtime effettivi (override del piano sui default).
    #[must_use]
    pub const fn effective_limits(&self) -> &Limits {
        &self.effective_limits
    }

    /// CRS di piano risolto, se dichiarato.
    #[must_use]
    pub const fn plan_crs(&self) -> Option<&ResolvedCrs> {
        self.plan_crs.as_ref()
    }

    /// Contratto di un arco (nome di input o id nodo).
    #[must_use]
    pub fn edge_contract(&self, edge: &str) -> Option<&DataContract> {
        self.edge_contracts.get(edge)
    }

    /// Contratto dell'arco di output del piano.
    ///
    /// # Errors
    ///
    /// `Internal` se l'arco di output manca dai contratti: impossibile per
    /// costruzione su un grafo validato (garanzia della validazione
    /// strutturale + inferenza), ma il compilatore non puo' dimostrarlo —
    /// l'invariante violata diventa un errore esplicito, mai un panic (R6).
    pub fn output_contract(&self) -> Result<&DataContract> {
        self.edge_contracts
            .get(&self.plan.plan().output)
            .ok_or_else(|| PlenoraError::Internal("l'output e' un arco del grafo validato".into()))
    }

    /// Ids canonici delle op usate, in ordine lessicografico.
    #[must_use]
    pub fn used_operations(&self) -> &[String] {
        &self.used_operations
    }

    /// Solo test: sovrascrive la versione engine registrata nell'identita',
    /// per verificare che l'executor rifiuti un grafo la cui identita' non
    /// combacia con l'ambiente corrente (ADR 4: mai procedere alla cieca).
    #[cfg(test)]
    pub fn set_engine_version_for_test(&mut self, version: &str) {
        self.engine_version = EngineVersion(version.to_owned());
    }
}

/// Fase 1 `validate` del DAG v5 (Architetture.md par. 6.1, ADR 4, ADR 5).
///
/// Un piano `schema_version: 4` entra da qui attraverso la migrazione
/// esplicita (ADR 15): il resto della fase 1 conosce una sola forma.
///
/// `input_contracts` associa a ogni nome dichiarato in `inputs` il contratto
/// letto dagli header (nessuna riga di dati): nomi duplicati, mancanti o
/// extra sono rifiutati (fail-closed). I `FieldId` delle colonne geometriche
/// di input sono rimappati nel namespace globale del grafo; i fingerprint di
/// input sono calcolati sui contratti come forniti (schema + geometria).
///
/// # Errors
///
/// - `InvalidPlan`: limiti o struttura del piano violati, input duplicati /
///   mancanti / extra, `sorted_by` con chiavi su un input, config non valide,
///   grafo che perde la geometria prima di un'op geo;
/// - `Unsupported`: operazione sconosciuta/`Planned`, capability non
///   compilata (`geos`/`proj`), schema di output non inferibile a secco;
/// - `Schema`: contratti (di input o inferiti) che violano le regole v1 (D16);
/// - `Crs`: CRS di piano assente/invalido, backend PROJ non compilato,
///   requisito CRS di un nodo non soddisfatto, mismatch CRS left/right;
/// - `Internal`: invariante interna violata (op risolta in parsing, arco
///   risolto dalla validazione strutturale) — impossibile su input esterno,
///   per quanto malformato; il caso "impossibile" e' un errore esplicito,
///   mai un panic (R6).
#[allow(clippy::too_many_lines)] // Passi sequenziali di par. 6.1: spezzarli nuocerebbe alla leggibilita'.
pub fn validate(
    plan_json: &str,
    input_contracts: &[(String, DataContract)],
) -> Result<ValidatedGraph> {
    // Passo 0: versione. Un piano v4 viene migrato al canonico v5 qui, in
    // un punto solo (ADR 15): il resto della fase 1 conosce una sola forma.
    // Un piano v5 attraversa senza copia.
    let plan_limits = PlanLimits::default();
    let plan_json = migrazione_v4::testo_canonico_v5(plan_json, &plan_limits)?;
    // Passo 1: limiti di default DURANTE il parsing, struttura, arieta',
    // risoluzione alias (PlanV5::parse, ADR 6).
    let plan = PlanV5::parse(plan_json.as_ref(), &plan_limits)?;
    // Validazione dei limiti effettivi in un punto solo, prima di qualunque
    // decisione: qui la attraversano TUTTI i piani, compresi quelli solo-geo
    // che non passano dal preparer tabellare dove il controllo viveva prima
    // (e dove correggeva in silenzio invece di rifiutare).
    plan.effective_limits().validate()?;
    let plan_ref = plan.plan();

    // Passo 2: contratti di input — corrispondenza esatta con i nomi
    // dichiarati, validita' strutturale, niente chiavi sorted_by nel
    // namespace del chiamante (rimappatura D16, vedi doc di modulo).
    let mut provided: HashMap<&str, &DataContract> = HashMap::with_capacity(input_contracts.len());
    for (name, contract) in input_contracts {
        if provided.insert(name.as_str(), contract).is_some() {
            return Err(PlenoraError::InvalidPlan(format!(
                "contratto di input duplicato per `{name}`"
            )));
        }
        contract.validate()?;
        if let Some(sorted) = &contract.properties.sorted_by {
            if sorted.confidence.value().is_some() {
                return Err(PlenoraError::InvalidPlan(format!(
                    "l'input `{name}` dichiara sorted_by con chiavi FieldId: il namespace \
                     dei FieldId e' assegnato dal planner (D16) e le chiavi non sono \
                     riferibili a colonne — v1 fail-closed"
                )));
            }
        }
    }
    for declared in &plan_ref.inputs {
        if !provided.contains_key(declared.as_str()) {
            return Err(PlenoraError::InvalidPlan(format!(
                "manca il contratto per l'input `{declared}`"
            )));
        }
    }
    if let Some(extra) = provided
        .keys()
        .filter(|name| !plan_ref.inputs.iter().any(|i| i.as_str() == **name))
        .min()
    {
        return Err(PlenoraError::InvalidPlan(format!(
            "contratto fornito per `{extra}`, non dichiarato tra gli input del piano"
        )));
    }
    let input_fingerprints: Vec<ContractFingerprint> = plan_ref
        .inputs
        .iter()
        .map(|name| contract_fingerprint(provided[name.as_str()]))
        .collect::<Result<_>>()?;

    // Passo 3: risoluzione del CRS di piano (feature-dispatch, fail-closed
    // senza proj-backend).
    let plan_crs = plan_ref
        .crs
        .as_deref()
        .map(|definition| resolve_crs(definition, "crs").map_err(PlenoraError::from))
        .transpose()?;

    // Passo 4: required_capabilities di ogni op contro i backend compilati.
    // Piu' il profilo di publish (ADR 7): il formato piano non dichiara
    // ancora un profilo, quindi si registra il default `AtomicPublish`;
    // quando il piano lo dichiarera' entrera' qui il profilo scelto, senza
    // cambi di API (la verifica resta in `check_compatibility`).
    let available = compiled_capabilities();
    let mut required = CapabilitySet::default();
    required.insert(PublishProfile::Atomic.capability_name());
    let mut used_operations: BTreeSet<String> = BTreeSet::new();
    for node in &plan_ref.nodes {
        let descriptor = find_operation(&node.op).ok_or_else(|| {
            PlenoraError::Internal(format!("op del nodo `{}` non risolta dal parse", node.id))
        })?;
        used_operations.insert(descriptor.id.to_owned());
        for capability in descriptor.required_capabilities {
            required.insert(capability);
            if !available.contains(capability) {
                return Err(PlenoraError::Unsupported(format!(
                    "nodo `{}`: {} richiede la capability `{capability}`, non compilata \
                     in questa build",
                    node.id, descriptor.id
                )));
            }
        }
    }

    // Passo 5: inferenza dei contratti arco per arco in ordine topologico.
    // Un unico FieldAllocator per grafo; i FieldId delle geometrie di input
    // sono rimappati all'ingresso (D16) con una allocazione fresca SENZA
    // legare il nome nell'allocatore: input diversi possono dichiarare
    // colonne omonime e un binding per nome farebbe vincere l'ultimo input
    // (l'interning per nome resta riservato alle chiavi `sorted_by` degli
    // analyze).
    let mut fields = FieldAllocator::default();
    let mut edge_contracts: BTreeMap<String, DataContract> = BTreeMap::new();
    let mut edge_provenance: BTreeMap<String, bool> = BTreeMap::new();
    for declared in &plan_ref.inputs {
        let mut contract = provided[declared.as_str()].clone();
        for geometry in &mut contract.geometries {
            let remapped = fields.alloc()?;
            if contract.active_geometry == Some(geometry.field_id) {
                contract.active_geometry = Some(remapped);
            }
            geometry.field_id = remapped;
        }
        edge_contracts.insert(declared.clone(), contract);
        edge_provenance.insert(declared.clone(), true);
    }

    let nodes_by_id: HashMap<&str, &crate::plan::NodeV5> = plan_ref
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    for node_id in plan.topological_order() {
        let node = nodes_by_id[node_id.as_str()];
        let descriptor = find_operation(&node.op).ok_or_else(|| {
            PlenoraError::Internal(format!("op del nodo `{}` non risolta dal parse", node.id))
        })?;
        let source_provenance = node
            .inputs
            .iter()
            .all(|source| edge_provenance.get(source).copied().unwrap_or(false));
        // Autorita' unica catalog-level (config-sensitive): stessa
        // classificazione del machinery executor (flag per kernel risolto in
        // `prepare`) e del gate legacy della CLI. Nessuna lista locale.
        let emits_row_diagnostics = descriptor.emits_row_diagnostics(&node.config);
        if emits_row_diagnostics && !source_provenance {
            return Err(PlenoraError::InvalidPlan(format!(
                "nodo `{}`: {} richiede provenance row-level originale; \
                 l'arco di input cambia cardinalita' o ordine e non porta lineage",
                node.id, descriptor.id
            )));
        }
        let inputs: Vec<DataContract> = node
            .inputs
            .iter()
            .map(|reference| {
                edge_contracts.get(reference).cloned().ok_or_else(|| {
                    PlenoraError::Internal(format!(
                        "arco `{reference}` non risolto dalla validazione"
                    ))
                })
            })
            .collect::<Result<_>>()?;
        let output = match descriptor.family {
            Family::Table => {
                analyze_table_contract(descriptor.id, &inputs, &node.config, &mut fields)
            }
            Family::Geo => analyze_geo_contract(
                descriptor.id,
                &inputs,
                &node.config,
                plan_crs.as_ref(),
                &mut fields,
            ),
        }
        .map_err(|error| at_node(&node.id, error))?;
        edge_contracts.insert(node.id.clone(), output);
        edge_provenance.insert(
            node.id.clone(),
            source_provenance
                && descriptor.source_row_provenance() == SourceRowProvenance::Preserved,
        );
    }

    // Passo 6: identita' ADR 4.
    let canonical = plan.canonical_json();
    let canonical_bytes = serde_json::to_vec(&canonical)?;
    let mut hasher = Sha256::new();
    hasher.update(PLAN_HASH_DOMAIN);
    hasher.update(&canonical_bytes);
    let plan_hash = PlanHash(hasher.finalize().into());
    let used: Vec<&OperationDescriptor> = used_operations
        .iter()
        .map(|id| {
            find_operation(id).ok_or_else(|| {
                PlenoraError::Internal(format!("op `{id}` usata dal piano assente dal catalogo"))
            })
        })
        .collect::<Result<_>>()?;
    let catalog_fingerprint = catalog_fingerprint(&used)?;

    Ok(ValidatedGraph {
        plan_hash,
        catalog_fingerprint,
        engine_version: EngineVersion(ENGINE_VERSION.to_owned()),
        arrow_version: ArrowVersion(ARROW_VERSION.to_owned()),
        required_capabilities: required,
        input_contract_fingerprints: input_fingerprints,
        plan_format_version: PLAN_SCHEMA_VERSION_V5,
        effective_limits: plan.effective_limits(),
        plan,
        edge_contracts,
        used_operations: used_operations.into_iter().collect(),
        plan_crs,
    })
}

/// Verifica di compatibilita' di un grafo validato con l'ambiente corrente
/// (ADR 4): qualunque mismatch rifiuta il grafo, mai procedere alla cieca.
///
/// I mismatch rilevati: **versione del formato piano diversa**, catalogo
/// cambiato (o op usata rimossa), versione engine diversa, versione Arrow
/// diversa, capability non piu' disponibili (backend o profilo di publish,
/// ADR 7).
///
/// `current_catalog` e' il catalogo contro cui riverificare (in produzione
/// `plenora_core::catalog::CATALOG`); `engine_version` e `arrow_version`
/// sono quelli dell'ambiente corrente (in produzione [`ENGINE_VERSION`] e
/// [`ARROW_VERSION`]); `capabilities` sono quelle offerte dall'ambiente
/// corrente (backend compilati + profili di publish supportati, ADR 7).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` con prefisso `GRAPH_MISMATCH` al primo mismatch.
pub fn check_compatibility(
    graph: &ValidatedGraph,
    current_catalog: &[OperationDescriptor],
    engine_version: &str,
    arrow_version: &str,
    capabilities: &CapabilitySet,
) -> Result<()> {
    let mismatch = |reason: String| PlenoraError::InvalidPlan(format!("GRAPH_MISMATCH: {reason}"));

    // Per primo, prima di qualunque altra cosa: se il grafo e' stato
    // validato sotto un'altra versione del formato piano, tutto cio' che
    // segue lo interpreta con regole che non sono le sue. Il campo era
    // registrato in `ValidatedGraph` ma non veniva confrontato con nulla —
    // un'identita' scritta e mai letta e' una garanzia solo apparente.
    if graph.plan_format_version != PLAN_SCHEMA_VERSION_V5 {
        return Err(mismatch(format!(
            "plan_format_version {} del grafo diversa da {PLAN_SCHEMA_VERSION_V5}",
            graph.plan_format_version
        )));
    }

    if graph.engine_version.0 != engine_version {
        return Err(mismatch(format!(
            "engine_version {} del grafo diversa da {engine_version}",
            graph.engine_version
        )));
    }

    if graph.arrow_version.0 != arrow_version {
        return Err(mismatch(format!(
            "arrow_version {} del grafo diversa da {arrow_version}",
            graph.arrow_version
        )));
    }

    let descriptors: Vec<&OperationDescriptor> = graph
        .used_operations
        .iter()
        .map(|id| {
            current_catalog
                .iter()
                .find(|descriptor| descriptor.id == id)
                .ok_or_else(|| mismatch(format!("operazione `{id}` assente dal catalogo corrente")))
        })
        .collect::<Result<_>>()?;
    if catalog_fingerprint(&descriptors)? != graph.catalog_fingerprint {
        return Err(mismatch(
            "catalog_fingerprint diverso: la semantica delle op usate e' cambiata".into(),
        ));
    }

    for capability in graph.required_capabilities.names() {
        if !capabilities.contains(capability) {
            return Err(mismatch(format!(
                "capability `{capability}` richiesta dal grafo non disponibile"
            )));
        }
    }
    Ok(())
}

/// Verifica i contratti DICHIARATI dal chiamante all'esecuzione contro i
/// fingerprint del grafo.
///
/// A differenza di [`check_input_compatibility`] non pretende che siano
/// presenti tutti gli input: `execute` la chiama sui soli input per cui il
/// chiamante ha passato un contratto ([`crate::Inputs::add_with_contract`]),
/// e per gli altri resta il confronto sullo schema.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` con prefisso `GRAPH_MISMATCH` per un contratto
/// non valido, non dichiarato dal piano o con fingerprint diverso.
pub fn check_declared_input_contracts(
    graph: &ValidatedGraph,
    declared: &[(String, DataContract)],
) -> Result<()> {
    let mismatch = |reason: String| PlenoraError::InvalidPlan(format!("GRAPH_MISMATCH: {reason}"));
    for (name, contract) in declared {
        let position = graph
            .plan
            .plan()
            .inputs
            .iter()
            .position(|declared| declared == name)
            .ok_or_else(|| mismatch(format!("input `{name}` non dichiarato dal piano")))?;
        let fingerprint = graph
            .input_contract_fingerprints
            .get(position)
            .ok_or_else(|| {
                PlenoraError::Internal(format!("fingerprint mancante per l'input `{name}`"))
            })?;
        contract
            .validate()
            .map_err(|error| mismatch(format!("contratto dell'input `{name}`: {error}")))?;
        if &contract_fingerprint(contract)? != fingerprint {
            return Err(mismatch(format!(
                "il contratto dell'input `{name}` e' diverso da quello validato"
            )));
        }
    }
    Ok(())
}

/// Riverifica i contratti di input contro i fingerprint registrati nel grafo
/// (ADR 4): un input con contratto diverso non riusa il grafo.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` con prefisso `GRAPH_MISMATCH` al primo mismatch
/// (nome mancante/extra/duplicato o fingerprint diverso).
pub fn check_input_compatibility(
    graph: &ValidatedGraph,
    input_contracts: &[(String, DataContract)],
) -> Result<()> {
    let mismatch = |reason: String| PlenoraError::InvalidPlan(format!("GRAPH_MISMATCH: {reason}"));

    let mut provided: HashMap<&str, &DataContract> = HashMap::with_capacity(input_contracts.len());
    for (name, contract) in input_contracts {
        if provided.insert(name.as_str(), contract).is_some() {
            return Err(mismatch(format!(
                "contratto di input duplicato per `{name}`"
            )));
        }
    }
    for (declared, fingerprint) in graph
        .plan
        .plan()
        .inputs
        .iter()
        .zip(&graph.input_contract_fingerprints)
    {
        let contract = provided
            .get(declared.as_str())
            .ok_or_else(|| mismatch(format!("manca il contratto per l'input `{declared}`")))?;
        // Il contratto fornito va validato PRIMA di confrontarne il
        // fingerprint: un contratto strutturalmente invalido — geometria non
        // Binary, nomi di campo ripetuti, tipi incoerenti coi metadati — ha
        // comunque un fingerprint, e senza questo passo poteva superare il
        // controllo di compatibilita' per poi fallire a runtime.
        contract
            .validate()
            .map_err(|error| mismatch(format!("contratto dell'input `{declared}`: {error}")))?;
        if &contract_fingerprint(contract)? != fingerprint {
            return Err(mismatch(format!(
                "il contratto dell'input `{declared}` e' diverso da quello validato"
            )));
        }
    }
    if let Some(extra) = provided
        .keys()
        .filter(|name| {
            !graph
                .plan
                .plan()
                .inputs
                .iter()
                .any(|i| i.as_str() == **name)
        })
        .min()
    {
        return Err(mismatch(format!(
            "contratto fornito per `{extra}`, non dichiarato tra gli input del piano"
        )));
    }
    Ok(())
}

/// Prefisso di contesto nodo preservando la variante d'errore.
fn at_node(node_id: &str, error: PlenoraError) -> PlenoraError {
    let prefix = |message: &String| format!("nodo `{node_id}`: {message}");
    match error {
        PlenoraError::InvalidPlan(message) => PlenoraError::InvalidPlan(prefix(&message)),
        PlenoraError::Unsupported(message) => PlenoraError::Unsupported(prefix(&message)),
        PlenoraError::Schema(message) => PlenoraError::Schema(prefix(&message)),
        PlenoraError::Crs(message) => PlenoraError::Crs(prefix(&message)),
        other => other,
    }
}

/// Fingerprint del catalogo ristretto alle op date (ADR 4): descrittori
/// serializzati in ordine stabile (il chiamante li passa ordinati per id) con
/// le quattro versioni per-componente, capability, classe di esecuzione e
/// determinismo.
///
/// La serializzazione di un `Value` non fallisce in pratica; il caso
/// "impossibile" e' un errore `Internal` esplicito, mai un panic (R6).
fn catalog_fingerprint(descriptors: &[&OperationDescriptor]) -> Result<CatalogFingerprint> {
    let mut hasher = Sha256::new();
    for descriptor in descriptors {
        let canonical = serde_json::to_vec(&descriptor_canonical(descriptor)).map_err(|error| {
            PlenoraError::Internal(format!(
                "serializzazione del descrittore di catalogo fallita: {error}"
            ))
        })?;
        hasher.update((canonical.len() as u64).to_le_bytes());
        hasher.update(canonical);
    }
    Ok(CatalogFingerprint(hasher.finalize().into()))
}

/// Serializzazione stabile di un descrittore (nomi enum espliciti, non
/// `Debug`: il fingerprint non deve dipendere dai nomi Rust).
///
/// ADR-0012 D12.2 (decisione deliberata): `geo_fusion` resta FUORI da
/// questa forma canonica e quindi dal `catalog_fingerprint` — il fingerprint
/// guarda la compatibilita' semantica dei piani, la fondibilita' e' una
/// capability FISICA (ADR 5): aggiungerla invaliderebbe piani semanticamente
/// identici. Il campo resta osservabile nello snapshot di catalogo
/// (`planner/tests.rs`) e nelle capability JSON (`capabilities.rs`).
fn descriptor_canonical(descriptor: &OperationDescriptor) -> Value {
    json!({
        "id": descriptor.id,
        "family": match descriptor.family {
            Family::Table => "table",
            Family::Geo => "geo",
        },
        "origin": match descriptor.origin {
            Origin::ManipolaCompat => "manipola_compat",
            Origin::Extension => "extension",
        },
        "arity": match descriptor.arity {
            Arity::Unary => "unary",
            Arity::BinaryOrdered => "binary_ordered",
            Arity::NAry => "n_ary",
        },
        "execution_class": match descriptor.execution_class {
            ExecutionClass::Streaming => "streaming",
            ExecutionClass::Blocking => "blocking",
            ExecutionClass::BinaryBlocking => "binary_blocking",
        },
        "cancellation_behavior": match descriptor.cancellation_behavior {
            CancellationBehavior::Cooperative => "cooperative",
            CancellationBehavior::BoundaryOnly => "boundary_only",
            CancellationBehavior::NonInterruptible => "non_interruptible",
        },
        "result_shape": descriptor.result_shape.map(|shape| match shape {
            ResultShape::OneToOne => "one_to_one",
            ResultShape::OneToMany => "one_to_many",
            ResultShape::ManyToOne => "many_to_one",
            ResultShape::Collective => "collective",
            ResultShape::WholeToMany => "whole_to_many",
            ResultShape::FromCoords => "from_coords",
            ResultShape::Diagnostic => "diagnostic",
        }),
        "crs_requirement": descriptor.crs_requirement.map(|requirement| match requirement {
            CrsRequirement::Known => "known",
            CrsRequirement::Projected => "projected",
            CrsRequirement::Geographic => "geographic",
            CrsRequirement::SameProjected => "same_projected",
            CrsRequirement::Reprojection => "reprojection",
        }),
        "required_capabilities": descriptor.required_capabilities,
        "determinism": match descriptor.determinism {
            DeterminismPolicy::DefinedOrder => "defined_order",
            DeterminismPolicy::InputOrder => "input_order",
            DeterminismPolicy::StableKeyOrder => "stable_key_order",
            DeterminismPolicy::CanonicalOrder => "canonical_order",
        },
        "expansion_constraint": match descriptor.expansion_constraint {
            ExpansionConstraint::SumRelative => json!("sum_relative"),
            ExpansionConstraint::LeftRelative => json!("left_relative"),
            ExpansionConstraint::RightRelative => json!("right_relative"),
            ExpansionConstraint::MaxRelative => json!("max_relative"),
            // Fattore in forma stabile per bit (mai `Debug` di float): due
            // build concordano sul fingerprint a parita' di costante (ADR 4).
            ExpansionConstraint::Custom(factor) => json!({ "custom": factor.to_bits() }),
        },
        "expansion_factor_exempt": descriptor.expansion_factor_exempt,
        "maturity": match descriptor.maturity {
            Maturity::Planned => "planned",
            Maturity::BackendPending => "backend_pending",
            Maturity::KernelValidated => "kernel_validated",
            Maturity::PublicProtocol => "public_protocol",
        },
        "semantic_version": descriptor.semantic_version,
        "config_schema_version": descriptor.config_schema_version,
        "contract_analysis_version": descriptor.contract_analysis_version,
        "kernel_version": descriptor.kernel_version,
    })
}

/// Fingerprint di un contratto di input: schema + geometria (ADR 4).
///
/// I `FieldId` sono esclusi: identita' interna del grafo (rimappata
/// all'ingresso, D16), non dell'input. `active_geometry` e le proprieta' di
/// `ContractProperties` sono escluse per lo stesso motivo (riferiscono
/// `FieldId`). Le proprieta' tipizzate della GEOMETRIA — `encoding`, `types` —
/// non riferiscono `FieldId` e sono invece parte del contratto osservabile:
/// entrano nel fingerprint.
///
/// # Errors
///
/// `Internal` se il contratto non e' serializzabile: impossibile per
/// costruzione, ma non dimostrabile al compilatore.
pub fn contract_fingerprint(contract: &DataContract) -> Result<ContractFingerprint> {
    let canonical = serde_json::to_vec(&contract_canonical(contract)).map_err(|error| {
        PlenoraError::Internal(format!(
            "serializzazione del contratto di input fallita: {error}"
        ))
    })?;
    Ok(ContractFingerprint(Sha256::digest(&canonical).into()))
}

/// Serializzazione stabile di schema + geometrie (chiavi ordinate da
/// `serde_json::Map`; i tipi Arrow in forma `Debug` — fingerprint solo in
/// memoria nella v1, ADR 4).
fn contract_canonical(contract: &DataContract) -> Value {
    let fields: Vec<Value> = contract
        .schema
        .fields()
        .iter()
        .map(|field| {
            json!({
                "name": field.name(),
                "data_type": format!("{:?}", field.data_type()),
                "nullable": field.is_nullable(),
                "metadata": sorted_metadata(field.metadata()),
            })
        })
        .collect();
    let geometries: Vec<Value> = contract
        .geometries
        .iter()
        .map(|geometry| {
            // R4.6.3/ADR 4: lo stato del CRS ENTRA nel fingerprint — un
            // contratto con CRS risolto e uno con CRS mancante non sono lo
            // stesso contratto (altrimenti un piano validato su input
            // risolto accetterebbe in riesecuzione un input senza CRS senza
            // rivalidazione, spostando il fallimento a runtime). La forma
            // risolta e' byte-identica a prima (fingerprint esistenti
            // stabili); `missing` e' il valore canonico R2.2.
            // `DeclaredUnresolved` entra in forma canonica con le
            // dichiarazioni: due incoerenze dichiarate diverse non sono lo
            // stesso contratto, e nessuno dei tre stati collide con un
            // altro.
            let crs = match &geometry.crs {
                // `ResolvedByDecision` ha la stessa forma canonica di
                // `Resolved`: il contratto effettivo e' lo stesso CRS
                // risolto (la decisione e' nel piano e nel `plan_hash`).
                ContractCrs::Resolved(crs) | ContractCrs::ResolvedByDecision(crs) => json!({
                    "definition": crs.definition(),
                    "kind": match crs.kind() {
                        CrsKind::Geographic => "geographic",
                        CrsKind::Projected => "projected",
                    },
                    "horizontal_unit_to_metre": crs
                        .horizontal_unit_to_metre()
                        .map(f64::to_bits),
                }),
                ContractCrs::DeclaredUnresolved {
                    crs_id,
                    definition,
                    definition_format,
                } => json!({
                    "resolution": "declared_unresolved",
                    "crs_id": crs_id,
                    "definition": definition,
                    "definition_format": definition_format
                        .map(plenora_core::contract::CrsDefinitionFormat::as_str),
                }),
                ContractCrs::Missing => json!(geometry.crs.resolution().as_str()),
            };
            let mut canonical = json!({
                "name": geometry.name,
                "crs": crs,
                "dimensions": geometry.dimensions.as_str(),
                "nullable": geometry.nullable,
            });
            // B1.3: `encoding` entra nel fingerprint SOLO quando dichiarato —
            // un contratto senza encoding produce lo stesso JSON di prima
            // (stabilita' dei fingerprint esistenti).
            if let Some(encoding) = geometry.encoding {
                if let Value::Object(map) = &mut canonical {
                    map.insert(
                        "encoding".to_owned(),
                        Value::String(encoding.as_str().to_owned()),
                    );
                }
            }
            // La dichiarazione dei tipi geometrici e' parte del contratto, non
            // un'identita' interna del grafo: entra nel fingerprint quando c'e'.
            // Restandone fuori, due contratti che dichiaravano tipi diversi —
            // `exact:point` e `exact:polygon` — condividevano il fingerprint, e
            // un grafo validato sul primo veniva riusato sul secondo senza
            // rivalidazione. Come per `encoding`, un contratto che non dichiara
            // nulla produce lo stesso JSON di prima.
            if let Some(types) = geometry.types.value() {
                if let Value::Object(map) = &mut canonical {
                    map.insert(
                        "types".to_owned(),
                        json!({
                            // La confidence distingue una dichiarazione esterna
                            // da una dimostrata: sono contratti diversi.
                            "confidence": confidence_label(&geometry.types.confidence),
                            // Anche lo scope: la stessa dichiarazione vale per
                            // lo schema, per il batch o per il dataset intero,
                            // e sono garanzie di forza diversa.
                            "scope": scope_label(geometry.types.scope),
                            "declaration": types.declaration().as_str(),
                            "list": types.to_canonical_list(),
                        }),
                    );
                }
            }
            canonical
        })
        .collect();
    let mut canonical = json!({
        "schema": {
            "fields": fields,
            "metadata": sorted_metadata(contract.schema.metadata()),
        },
        "geometries": geometries,
    });
    // `row_count` NON riferisce `FieldId` — a differenza di `sorted_by`, che
    // resta escluso — ed entra nell'analisi: `map_row_count` lo propaga nei
    // contratti d'arco e quindi nelle decisioni a valle. Due input che
    // dichiarano cardinalita' diverse producono analisi diverse e non sono
    // lo stesso contratto. Entra solo quando c'e': un contratto che non lo
    // dichiara produce lo stesso JSON di prima.
    if let Some(row_count) = &contract.properties.row_count {
        if let (Value::Object(map), Some(value)) = (&mut canonical, row_count.value()) {
            map.insert(
                "row_count".to_owned(),
                json!({
                    "confidence": confidence_label(&row_count.confidence),
                    "scope": scope_label(row_count.scope),
                    "value": value,
                }),
            );
        }
    }
    canonical
}

/// Etichetta stabile del livello di fiducia di una proprieta' tipizzata.
const fn confidence_label<T>(confidence: &PropertyConfidence<T>) -> &'static str {
    match confidence {
        PropertyConfidence::Declared(_) => "declared",
        PropertyConfidence::Proven(_) => "proven",
        PropertyConfidence::Estimated(_) => "estimated",
        PropertyConfidence::Unknown => "unknown",
    }
}

/// Etichetta stabile dello scope di una proprieta' tipizzata.
const fn scope_label(scope: PropertyScope) -> &'static str {
    match scope {
        PropertyScope::Schema => "schema",
        PropertyScope::Batch => "batch",
        PropertyScope::Stream => "stream",
        PropertyScope::Dataset => "dataset",
    }
}

/// Metadati Arrow (nome/valore) come oggetto JSON a chiavi ordinate.
fn sorted_metadata(metadata: &HashMap<String, String>) -> Value {
    metadata
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect::<Map<String, Value>>()
        .into()
}
