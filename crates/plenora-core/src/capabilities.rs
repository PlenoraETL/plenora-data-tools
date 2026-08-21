//! Capability dichiarative del componente (ICD §10 R10.2, contratti
//! trasversali v2.0-rc10 — proposta in attesa di ratifica, la forma e'
//! definita localmente in attesa di una convenzione trasversale).
//!
//! Interrogabili PRIMA dell'esecuzione, in forma leggibile da un programma
//! (JSON via CLI `capabilities`), mai desumibili per tentativi. La fonte
//! e' UNA SOLA: il modello geometrico canonico (R3.1-R3.5) e il catalogo
//! delle operazioni ([`crate::catalog::CATALOG`]) — le capability non
//! possono divergere da cio' che il planner accetta o rifiuta.
//!
//! Regole collegate: R10.1 (il planner fallisce a compile-plan, mai a
//! meta' esecuzione), R10.3 (il rifiuto nomina la capability mancante in
//! forma tipizzata — `planner.rs` passo 4), R10.4 (nessuna degradazione
//! silenziosa: un tipo non supportato e' rifiutato, mai approssimato),
//! R3.3/R3.3.1 (le cinque dimensioni si rappresentano e propagano sempre;
//! l'elaborazione e' solo XY con rifiuto esplicito per Z/M).

use serde::Serialize;

use crate::catalog::{
    CancellationBehavior, CrsRequirement, DeterminismPolicy, ExecutionClass, Family,
    OperationDescriptor, CATALOG,
};

/// Versione del protocollo delle capability (R2.5 per i metadati Arrow;
/// qui: formato del documento JSON emesso da `capabilities`).
pub const CAPABILITIES_PROTOCOL_VERSION: u32 = 1;

/// Versione Arrow pinnata (R1).
pub const ARROW_VERSION: &str = "59.1.0";

/// Modello geometrico dichiarato (R3.1-R3.5): che cosa il componente sa
/// rappresentare, propagare ed elaborare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GeometryCapabilities {
    /// Dimensioni rappresentabili e propagabili (R3.3: tutte e cinque,
    /// obbligo anche senza elaborazione Z/M).
    pub dimensions_propagated: [&'static str; 5],
    /// Dimensioni elaborate dai kernel geo (R3.3.1: solo `xy`; le altre
    /// sono rifiutate esplicitamente in analisi, mai scartate in silenzio).
    pub dimensions_elaborated: [&'static str; 1],
    /// Encoding delle celle accettati (R3.5, enum chiuso).
    pub encodings: [&'static str; 2],
    /// Tipi geometrici supportati dalla decodifica (sottoinsieme R3.2:
    /// i sette tipi base concreti).
    pub types_supported: [&'static str; 7],
    /// Tipi canonici R3.1 rifiutati esplicitamente (R3.2: rifiuto, mai
    /// degradazione o ignoramento silenzioso). `unknown` NON e' qui: per
    /// R3.4 i byte si preservano e la dimensionalita' resta non risolta —
    /// e' propagato, non decodificato.
    pub types_rejected: [&'static str; 8],
}

/// Capability dichiarata di una singola operazione del catalogo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationCapability {
    /// Id namespaced (`table.*` / `geo.*`).
    pub id: &'static str,
    /// Famiglia (`table` | `geo`).
    pub family: &'static str,
    /// Classe di esecuzione (`streaming` | `blocking` | `binary_blocking`).
    pub execution_class: &'static str,
    /// Comportamento alla cancellazione (`cooperative` | `boundary_only` |
    /// `non_interruptible`).
    pub cancellation_behavior: &'static str,
    /// Politica di determinismo (`defined_order` | `input_order` |
    /// `stable_key_order` | `canonical_order`).
    pub determinism: &'static str,
    /// Fondibilita' nella fusione dei segmenti geo (architettura.md#geometrie D12.2:
    /// `not_fusible` | `transform_in_place` | `terminal_measure`).
    /// Capability fisica: esposta qui ma FUORI dal `catalog_fingerprint`.
    pub geo_fusion: &'static str,
    /// Backend/feature richiesti (`geos`, `proj`): vuoto se nessuno.
    pub required_capabilities: &'static [&'static str],
    /// Requisito CRS, solo op geo (`snake_case` di [`CrsRequirement`]).
    pub crs_requirement: Option<&'static str>,
}

/// Documento completo delle capability del componente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentCapabilities {
    /// Versione del formato di questo documento.
    pub protocol_version: u32,
    /// Nome del componente.
    pub component: &'static str,
    /// Versione Arrow pinnata (R1).
    pub arrow_version: &'static str,
    /// Modello geometrico dichiarato.
    pub geometry: GeometryCapabilities,
    /// Una capability per ogni operazione del catalogo, in ordine di
    /// catalogo (deterministico, architettura.md#determinismo).
    pub operations: Vec<OperationCapability>,
}

const fn family_name(family: Family) -> &'static str {
    match family {
        Family::Table => "table",
        Family::Geo => "geo",
    }
}

const fn execution_class_name(class: ExecutionClass) -> &'static str {
    match class {
        ExecutionClass::Streaming => "streaming",
        ExecutionClass::Blocking => "blocking",
        ExecutionClass::BinaryBlocking => "binary_blocking",
    }
}

const fn cancellation_name(behavior: CancellationBehavior) -> &'static str {
    match behavior {
        CancellationBehavior::Cooperative => "cooperative",
        CancellationBehavior::BoundaryOnly => "boundary_only",
        CancellationBehavior::NonInterruptible => "non_interruptible",
    }
}

const fn determinism_name(policy: DeterminismPolicy) -> &'static str {
    match policy {
        DeterminismPolicy::DefinedOrder => "defined_order",
        DeterminismPolicy::InputOrder => "input_order",
        DeterminismPolicy::StableKeyOrder => "stable_key_order",
        DeterminismPolicy::CanonicalOrder => "canonical_order",
    }
}

const fn crs_requirement_name(requirement: CrsRequirement) -> &'static str {
    match requirement {
        CrsRequirement::Known => "known",
        CrsRequirement::Projected => "projected",
        CrsRequirement::Geographic => "geographic",
        CrsRequirement::SameProjected => "same_projected",
        CrsRequirement::Reprojection => "reprojection",
    }
}

/// Capability della singola op, derivata dal descriptor di catalogo (una
/// sola fonte: non puo' divergere da cio' che il planner verifica).
fn operation_capability(descriptor: &OperationDescriptor) -> OperationCapability {
    OperationCapability {
        id: descriptor.id,
        family: family_name(descriptor.family),
        execution_class: execution_class_name(descriptor.execution_class),
        cancellation_behavior: cancellation_name(descriptor.cancellation_behavior),
        determinism: determinism_name(descriptor.determinism),
        geo_fusion: descriptor.geo_fusion.as_str(),
        required_capabilities: descriptor.required_capabilities,
        crs_requirement: descriptor.crs_requirement.map(crs_requirement_name),
    }
}

/// Il documento delle capability del componente, costruito dal catalogo.
#[must_use]
pub fn component_capabilities() -> ComponentCapabilities {
    ComponentCapabilities {
        protocol_version: CAPABILITIES_PROTOCOL_VERSION,
        component: "plenora-data-tools",
        arrow_version: ARROW_VERSION,
        geometry: GeometryCapabilities {
            dimensions_propagated: ["xy", "xyz", "xym", "xyzm", "unknown"],
            dimensions_elaborated: ["xy"],
            encodings: ["wkb", "ewkb"],
            types_supported: [
                "point",
                "linestring",
                "polygon",
                "multipoint",
                "multilinestring",
                "multipolygon",
                "geometrycollection",
            ],
            types_rejected: [
                "circularstring",
                "compoundcurve",
                "curvepolygon",
                "multicurve",
                "multisurface",
                "polyhedralsurface",
                "tin",
                "triangle",
            ],
        },
        operations: CATALOG.iter().map(operation_capability).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ogni_op_del_catalogo_ha_una_capability() {
        let capabilities = component_capabilities();
        assert_eq!(capabilities.operations.len(), CATALOG.len());
        let mut ids: Vec<&str> = capabilities.operations.iter().map(|op| op.id).collect();
        let mut catalog_ids: Vec<&str> = CATALOG.iter().map(|op| op.id).collect();
        ids.sort_unstable();
        catalog_ids.sort_unstable();
        assert_eq!(
            ids, catalog_ids,
            "ogni op del catalogo esattamente una volta"
        );
    }

    #[test]
    fn il_modello_geometrico_dichiarato_e_quello_canonico() {
        let geometry = &component_capabilities().geometry;
        // R3.3: tutte e cinque le dimensioni propagate.
        assert_eq!(
            geometry.dimensions_propagated,
            ["xy", "xyz", "xym", "xyzm", "unknown"]
        );
        // R3.3.1: elaborazione solo XY.
        assert_eq!(geometry.dimensions_elaborated, ["xy"]);
        // R3.5: encoding come enum chiuso.
        assert_eq!(geometry.encodings, ["wkb", "ewkb"]);
        // R3.1: i sedici tipi canonici sono 7 supportati + 8 rifiutati +
        // unknown (propagato per R3.4, non decodificato).
        let total = geometry.types_supported.len() + geometry.types_rejected.len();
        assert_eq!(total, 15, "7 supportati + 8 rifiutati + unknown = 16");
        assert!(!geometry.types_rejected.contains(&"unknown"));
    }

    #[test]
    fn il_documento_e_deterministico_e_serializzabile() {
        let first = serde_json::to_string(&component_capabilities()).expect("serialize");
        let second = serde_json::to_string(&component_capabilities()).expect("serialize");
        assert_eq!(
            first, second,
            "stesso documento, stessi byte (architettura.md#determinismo)"
        );
        assert!(first.contains("\"protocol_version\":1"));
        assert!(first.contains("\"geo.reproject\""));
        assert!(first.contains("\"proj\""));
        assert!(first.contains("\"geo_fusion\":\"transform_in_place\""));
    }

    #[test]
    fn geo_fusion_esposta_dalla_stessa_fonte_del_catalogo() {
        // architettura.md#geometrie D12.2: la capability non puo' divergere dal descriptor —
        // il valore serializzato e' il nome stabile del campo di catalogo.
        let capabilities = component_capabilities();
        for descriptor in CATALOG {
            let capability = capabilities
                .operations
                .iter()
                .find(|op| op.id == descriptor.id)
                .expect("ogni op ha una capability");
            assert_eq!(
                capability.geo_fusion,
                descriptor.geo_fusion.as_str(),
                "{}",
                descriptor.id
            );
        }
        // Spot-check delle tre classi sul perimetro fondibile,
        // `reproject` e `make_valid` inclusi.
        let by_id = |id: &str| {
            capabilities
                .operations
                .iter()
                .find(|op| op.id == id)
                .expect("op in catalogo")
                .geo_fusion
        };
        assert_eq!(by_id("geo.buffer"), "transform_in_place");
        assert_eq!(by_id("geo.area"), "terminal_measure");
        assert_eq!(by_id("table.filter"), "not_fusible");
        assert_eq!(by_id("geo.reproject"), "transform_in_place");
        assert_eq!(by_id("geo.make_valid"), "transform_in_place");
    }
}
