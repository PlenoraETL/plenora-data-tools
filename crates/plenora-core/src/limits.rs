//! Le tre famiglie di limiti (decisione D19, ADR 6).
//!
//! - [`RowLimits`]: espansioni logiche dei dati;
//! - [`PlanLimits`]: complessità del piano, applicati durante il parsing;
//! - [`Limits`]: contenitore unico (dati/runtime, piano, stringhe).

use serde::Deserialize;

/// Limiti di righe, semanticamente distinti (ADR 6).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowLimits {
    /// Righe lette da ciascuna sorgente di input.
    pub max_input_rows: u64,
    /// Righe dell'output finale.
    pub max_output_rows: u64,
    /// Righe su qualunque arco intermedio del DAG.
    pub max_rows_per_edge: u64,
    /// Fattore di espansione output/input; la base è per classe di operazione.
    pub max_expansion_factor: f64,
}

impl Default for RowLimits {
    fn default() -> Self {
        Self {
            max_input_rows: 10_000_000,
            max_output_rows: 10_000_000,
            max_rows_per_edge: 10_000_000,
            max_expansion_factor: 100.0,
        }
    }
}

/// Limiti alla complessità del piano, applicati durante il parsing (ADR 6).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanLimits {
    pub max_plan_json_bytes: usize,
    pub max_plan_nodes: usize,
    pub max_plan_edges: usize,
    pub max_plan_depth: usize,
    pub max_fan_out: usize,
    pub max_inputs: usize,
    pub max_config_bytes_per_node: usize,
    pub max_identifier_bytes: usize,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_plan_json_bytes: 16 * 1024 * 1024,
            max_plan_nodes: 1_024,
            max_plan_edges: 4_096,
            max_plan_depth: 256,
            max_fan_out: 64,
            max_inputs: 16,
            max_config_bytes_per_node: 1024 * 1024,
            max_identifier_bytes: 256,
        }
    }
}

/// Contenitore unico dei limiti (Architetture.md par. 3.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(flatten)]
    pub rows: RowLimits,
    #[serde(default)]
    pub plan: PlanLimits,
    /// Budget memoria globale del piano (perimetro: ADR 2).
    pub max_memory_bytes: u64,
    /// Quota spill su disco.
    pub max_temp_bytes: u64,
    /// Partizioni di spill.
    pub spill_partitions: u32,
    /// Grado massimo di parallelismo (risorsa, non proprietà dei nodi).
    pub max_parallelism: u32,
    /// Limite per cella WKB (da geo-tools-arrow).
    pub max_wkb_cell_bytes: u64,
    /// Limite payload complessivo in lettura.
    pub max_payload_bytes: u64,
    /// Numero massimo di batch per arco.
    pub max_batches: u64,
    /// Profondità geometrica massima (componenti annidate).
    pub max_geometry_depth: u32,
    /// Limite per singola stringa.
    pub max_string_bytes: usize,
    /// Limite per pattern regex.
    pub max_regex_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            rows: RowLimits::default(),
            plan: PlanLimits::default(),
            max_memory_bytes: 512 * 1024 * 1024,
            max_temp_bytes: 8 * 1024 * 1024 * 1024,
            spill_partitions: 64,
            max_parallelism: 0, // 0 = numero di core logici
            max_wkb_cell_bytes: 64 * 1024 * 1024,
            max_payload_bytes: 16 * 1024 * 1024 * 1024,
            max_batches: 65_536,
            max_geometry_depth: 64,
            max_string_bytes: 16 * 1024 * 1024,
            max_regex_bytes: 64 * 1024,
        }
    }
}
