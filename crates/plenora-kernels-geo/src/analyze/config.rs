//! Struct serde di configurazione per l'analisi dei contratti `geo.*`.

use serde::Deserialize;

use crate::spatial_join::JoinPredicate;
use crate::topology::OverlayMode;

// ---------------------------------------------------------------------------
// Config serde minimali (duplicazione documentata: stessi nomi e domini del
// protocollo legacy, senza i parametri di trasporto).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct EmptyConfig {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct OutputColumnConfig {
    pub(in crate::analyze) output_column: Option<String>,
}

/// Stile di cap per `buffer` (default round, come il kernel).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(in crate::analyze) enum BufferCapParam {
    Round,
    Flat,
    Square,
}

/// Politica di `simplify`: Douglas-Peucker (default) o topology-preserving.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(in crate::analyze) enum SimplifyPolicyParam {
    DouglasPeucker,
    PreserveTopology,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct BufferConfig {
    pub(in crate::analyze) distance: f64,
    pub(in crate::analyze) cap: Option<BufferCapParam>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct SimplifyConfig {
    pub(in crate::analyze) tolerance: f64,
    pub(in crate::analyze) policy: Option<SimplifyPolicyParam>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct ReprojectConfig {
    pub(in crate::analyze) target_crs: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct AffineTransformConfig {
    pub(in crate::analyze) coefficients: Vec<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct TranslateConfig {
    pub(in crate::analyze) x_offset: f64,
    pub(in crate::analyze) y_offset: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct ScaleConfig {
    pub(in crate::analyze) x_factor: f64,
    pub(in crate::analyze) y_factor: f64,
    pub(in crate::analyze) x_origin: Option<f64>,
    pub(in crate::analyze) y_origin: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct RotateConfig {
    pub(in crate::analyze) degrees: f64,
    pub(in crate::analyze) x_origin: Option<f64>,
    pub(in crate::analyze) y_origin: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct ConcaveHullConfig {
    pub(in crate::analyze) concavity: f64,
    pub(in crate::analyze) length_threshold: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct DensifyConfig {
    pub(in crate::analyze) max_segment_length: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct SnapToGridConfig {
    pub(in crate::analyze) grid_size: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct LineSubstringConfig {
    pub(in crate::analyze) start_ratio: f64,
    pub(in crate::analyze) end_ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct LineInterpolatePointConfig {
    pub(in crate::analyze) ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct CleanTopologyConfig {
    pub(in crate::analyze) snap_tolerance: f64,
    pub(in crate::analyze) remove_overlaps: Option<bool>,
    pub(in crate::analyze) fill_gaps: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct VoronoiConfig {
    pub(in crate::analyze) max_points: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct PolygonizeConfig {
    pub(in crate::analyze) node_input: Option<bool>,
    pub(in crate::analyze) require_complete: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct FromCoordsConfig {
    pub(in crate::analyze) x_column: Option<String>,
    pub(in crate::analyze) y_column: Option<String>,
    pub(in crate::analyze) geometry_column: Option<String>,
    pub(in crate::analyze) crs: Option<String>,
}

/// Secondo operando geometrico da config (v1, D16: una sola colonna
/// geometria per input): WKB codificato esadecimale, validato in analisi.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct OtherWkbConfig {
    pub(in crate::analyze) other_wkb: String,
    pub(in crate::analyze) output_column: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct SplitConfig {
    pub(in crate::analyze) other_wkb: String,
    pub(in crate::analyze) tolerance: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct SJoinConfig {
    pub(in crate::analyze) predicate: JoinPredicate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct NearestConfig {
    pub(in crate::analyze) max_distance: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct OverlayConfig {
    pub(in crate::analyze) mode: OverlayMode,
}

/// `from_wkt`: colonna Utf8 con il testo WKT; la politica `on_error`
/// (default `null`) e' semantica di runtime, qui solo validata.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct FromWktConfig {
    pub(in crate::analyze) wkt_column: String,
    pub(in crate::analyze) output_column: Option<String>,
    pub(in crate::analyze) on_error: Option<crate::extensions::OnWktError>,
    pub(in crate::analyze) crs: Option<String>,
}

/// Campo accessorio richiedibile in `geometry_accessors.fields`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(in crate::analyze) enum AccessorFieldParam {
    GeometryType,
    NumGeometries,
    NumInteriorRings,
    StartPoint,
    EndPoint,
    IsClosed,
}

impl AccessorFieldParam {
    /// Indice in [`super::ACCESSOR_COLUMNS`] (ordine canonico di output).
    pub(in crate::analyze) const fn column_index(self) -> usize {
        match self {
            Self::GeometryType => 0,
            Self::NumGeometries => 1,
            Self::NumInteriorRings => 2,
            Self::StartPoint => 3,
            Self::EndPoint => 4,
            Self::IsClosed => 5,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct GeometryAccessorsConfig {
    pub(in crate::analyze) fields: Option<Vec<AccessorFieldParam>>,
    pub(in crate::analyze) output_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct CollectConfig {
    pub(in crate::analyze) group_by: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct LineLocatePointConfig {
    pub(in crate::analyze) point_wkb: String,
    pub(in crate::analyze) output_column: Option<String>,
}

/// Extent di `generate_grid`: finito e non degenere (dominio verificato dal
/// kernel [`crate::extensions2::GridExtent`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct GridExtentConfig {
    pub(in crate::analyze) xmin: f64,
    pub(in crate::analyze) ymin: f64,
    pub(in crate::analyze) xmax: f64,
    pub(in crate::analyze) ymax: f64,
}

/// `generate_grid` (v1.2, generativa): `shape` default `square`,
/// `include_centroid` default false, CRS da `crs` o di piano.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct GenerateGridConfig {
    pub(in crate::analyze) extent: GridExtentConfig,
    pub(in crate::analyze) cell_size: f64,
    pub(in crate::analyze) shape: Option<crate::extensions2::GridShape>,
    pub(in crate::analyze) crs: Option<String>,
    pub(in crate::analyze) include_centroid: Option<bool>,
}

/// `subdivide` (v1.2): `output_column` rinomina la colonna geometria
/// (default: nome invariato, in place come `explode`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct SubdivideConfig {
    pub(in crate::analyze) max_vertices: usize,
    pub(in crate::analyze) output_column: Option<String>,
}

/// `snap` (v1.2): riferimento WKB hex da config (convenzione D16, stesso CRS
/// dell'input), validato strutturalmente e decodificato in analisi.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct SnapConfig {
    pub(in crate::analyze) reference_wkb: String,
    pub(in crate::analyze) tolerance: f64,
}

/// `coverage_validate` (v1.3): tutti i campi opzionali; default kernel
/// (`tolerance` 0, `max_issues` [`crate::extensions3::DEFAULT_MAX_ISSUES`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct CoverageValidateConfig {
    pub(in crate::analyze) tolerance: Option<f64>,
    pub(in crate::analyze) max_issues: Option<usize>,
}

/// `shared_paths` (v1.3): tutti i campi opzionali; default kernel
/// (`tolerance` 0, `min_length` 0).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct SharedPathsConfig {
    pub(in crate::analyze) tolerance: Option<f64>,
    pub(in crate::analyze) min_length: Option<f64>,
}

/// `cluster_dbscan` (v1.3): `eps` e `min_points` obbligatori; `output_column`
/// opzionale (default [`super::CLUSTER_ID_COLUMN`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::analyze) struct ClusterDbscanConfig {
    pub(in crate::analyze) eps: f64,
    pub(in crate::analyze) min_points: usize,
    pub(in crate::analyze) output_column: Option<String>,
}
