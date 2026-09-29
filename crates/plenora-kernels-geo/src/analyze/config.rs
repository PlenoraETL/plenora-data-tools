//! Struct serde di configurazione per l'analisi dei contratti `geo.*`.
//!
//! Pubbliche perche' l'esecutore (`plenora-pipeline`) legga la config con
//! gli stessi tipi dell'analisi: una sola lettura della config, nessuna
//! seconda copia dei nomi e dei default che potrebbe divergere.

use serde::Deserialize;

use crate::spatial_join::JoinPredicate;
use crate::topology::OverlayMode;

// ---------------------------------------------------------------------------
// Config serde minimali: stessi nomi e domini del protocollo legacy, senza i
// parametri di trasporto.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyConfig {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputColumnConfig {
    pub output_column: Option<String>,
}

/// Stile di cap per `buffer` (default round, come il kernel).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BufferCapParam {
    Round,
    Flat,
    Square,
}

/// Politica di `simplify`: Douglas-Peucker (default) o topology-preserving.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SimplifyPolicyParam {
    DouglasPeucker,
    PreserveTopology,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BufferConfig {
    pub distance: f64,
    pub cap: Option<BufferCapParam>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimplifyConfig {
    pub tolerance: f64,
    pub policy: Option<SimplifyPolicyParam>,
}

/// `affine_transform`: la matrice affine 2D.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffineTransformConfig {
    /// Obbligatorio: esattamente sei numeri finiti `[a, b, xoff, d, e,
    /// yoff]`; `(x, y)` diventa `(a x + b y + xoff, d x + e y + yoff)`.
    pub coefficients: Vec<f64>,
}

/// `translate`: spostamento nelle unita' del CRS.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranslateConfig {
    /// Obbligatorio, finito: spostamento lungo x.
    pub x_offset: f64,
    /// Obbligatorio, finito: spostamento lungo y.
    pub y_offset: f64,
}

/// `scale`: fattori per asse attorno a un'origine.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScaleConfig {
    /// Obbligatorio, finito: fattore lungo x (negativo: riflessione).
    pub x_factor: f64,
    /// Obbligatorio, finito: fattore lungo y (negativo: riflessione).
    pub y_factor: f64,
    /// Facoltativo, finito: x dell'origine che resta ferma. Il valore
    /// usato quando manca non e' deciso qui: il kernel riceve l'origine
    /// esplicita e nessun esecutore lo chiama ancora.
    pub x_origin: Option<f64>,
    /// Facoltativo, finito: y dell'origine, come `x_origin`.
    pub y_origin: Option<f64>,
}

/// `rotate`: angolo attorno a un centro.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotateConfig {
    /// Obbligatorio, finito: angolo in gradi, positivo in verso
    /// antiorario.
    pub degrees: f64,
    /// Facoltativo, finito: x del centro di rotazione. Il valore usato
    /// quando manca non e' deciso qui: il kernel riceve il centro esplicito
    /// e nessun esecutore lo chiama ancora.
    pub x_origin: Option<f64>,
    /// Facoltativo, finito: y del centro di rotazione, come `x_origin`.
    pub y_origin: Option<f64>,
}

/// `concave_hull`: parametri dell'algoritmo di `geo`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcaveHullConfig {
    /// Obbligatorio, finito e maggiore di zero: piu' piccolo, piu'
    /// concavo.
    pub concavity: f64,
    /// Facoltativo, finito e non negativo: lunghezza sotto la quale un
    /// lato non si scava. Il valore usato quando manca non e' deciso qui:
    /// il kernel lo riceve esplicito e nessun esecutore lo chiama ancora.
    pub length_threshold: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DensifyConfig {
    pub max_segment_length: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapToGridConfig {
    pub grid_size: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineSubstringConfig {
    pub start_ratio: f64,
    pub end_ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineInterpolatePointConfig {
    pub ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanTopologyConfig {
    pub snap_tolerance: f64,
    pub remove_overlaps: Option<bool>,
    pub fill_gaps: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoronoiConfig {
    pub max_points: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolygonizeConfig {
    pub node_input: Option<bool>,
    pub require_complete: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromCoordsConfig {
    pub x_column: Option<String>,
    pub y_column: Option<String>,
    pub geometry_column: Option<String>,
    pub crs: Option<String>,
}

/// Secondo operando geometrico da config (D16: una sola colonna
/// geometria per input): WKB codificato esadecimale, validato in analisi.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OtherWkbConfig {
    pub other_wkb: String,
    pub output_column: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitConfig {
    pub other_wkb: String,
    pub tolerance: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SJoinConfig {
    pub predicate: JoinPredicate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NearestConfig {
    pub max_distance: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    pub mode: OverlayMode,
}

/// `from_wkt`: colonna Utf8 con il testo WKT; la politica `on_error`
/// (default `null`) e' semantica di runtime, qui solo validata.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromWktConfig {
    pub wkt_column: String,
    pub output_column: Option<String>,
    pub on_error: Option<crate::extensions::OnWktError>,
    pub crs: Option<String>,
}

/// Campo accessorio richiedibile in `geometry_accessors.fields`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AccessorFieldParam {
    GeometryType,
    NumGeometries,
    NumInteriorRings,
    StartPoint,
    EndPoint,
    IsClosed,
}

impl AccessorFieldParam {
    /// Indice in [`super::ACCESSOR_COLUMNS`] (ordine canonico di output).
    #[must_use]
    pub const fn column_index(self) -> usize {
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
pub struct GeometryAccessorsConfig {
    pub fields: Option<Vec<AccessorFieldParam>>,
    pub output_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectConfig {
    pub group_by: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineLocatePointConfig {
    pub point_wkb: String,
    pub output_column: Option<String>,
}

/// Extent di `generate_grid`: finito e non degenere (dominio verificato dal
/// kernel [`crate::extensions2::GridExtent`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridExtentConfig {
    pub xmin: f64,
    pub ymin: f64,
    pub xmax: f64,
    pub ymax: f64,
}

/// `generate_grid` (generativa): `shape` default `square`,
/// `include_centroid` default false, CRS da `crs` o di piano.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateGridConfig {
    pub extent: GridExtentConfig,
    pub cell_size: f64,
    pub shape: Option<crate::extensions2::GridShape>,
    pub crs: Option<String>,
    pub include_centroid: Option<bool>,
}

/// `subdivide`: `output_column` rinomina la colonna geometria
/// (default: nome invariato, in place come `explode`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubdivideConfig {
    pub max_vertices: usize,
    pub output_column: Option<String>,
}

/// `snap`: riferimento WKB hex da config (convenzione D16, stesso CRS
/// dell'input), validato strutturalmente e decodificato in analisi.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapConfig {
    pub reference_wkb: String,
    pub tolerance: f64,
}

/// `coverage_validate`: tutti i campi opzionali; default kernel
/// (`tolerance` 0, `max_issues` [`crate::extensions3::DEFAULT_MAX_ISSUES`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageValidateConfig {
    pub tolerance: Option<f64>,
    pub max_issues: Option<usize>,
}

/// `shared_paths`: tutti i campi opzionali; default kernel
/// (`tolerance` 0, `min_length` 0).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPathsConfig {
    pub tolerance: Option<f64>,
    pub min_length: Option<f64>,
}

/// `cluster_dbscan`: `eps` e `min_points` obbligatori; `output_column`
/// opzionale (default [`super::CLUSTER_ID_COLUMN`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterDbscanConfig {
    pub eps: f64,
    pub min_points: usize,
    pub output_column: Option<String>,
}
