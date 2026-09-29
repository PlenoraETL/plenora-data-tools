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

/// Estremita' delle linee di `geo.buffer` (`cap`; default `round`, come il
/// kernel). Specchio di `operations::BufferCapStyle`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BufferCapParam {
    /// `round`: arco attorno all'estremo.
    Round,
    /// `flat`: taglio netto all'estremo; i punti non hanno buffer.
    Flat,
    /// `square`: quadrato che sporge di `|distance|` oltre l'estremo.
    Square,
}

/// Algoritmo di `geo.simplify` (`policy`; default `douglas_peucker`).
/// Specchio di `operations::SimplifyPolicy`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SimplifyPolicyParam {
    /// `douglas_peucker`: Ramer-Douglas-Peucker, `tolerance` e' una
    /// distanza.
    DouglasPeucker,
    /// `preserve_topology`: Visvalingam-Whyatt con conservazione della
    /// topologia, `tolerance` e' un'area.
    PreserveTopology,
}

/// Config di `geo.buffer`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BufferConfig {
    /// Distanza del buffer nelle unita' del CRS, obbligatoria e finita;
    /// negativa erode le parti areali, nulla ne da' l'unione.
    pub distance: f64,
    /// Estremita' delle linee; assente vale `round`.
    pub cap: Option<BufferCapParam>,
}

/// Config di `geo.simplify`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimplifyConfig {
    /// Soglia, obbligatoria, finita e non negativa: distanza (unita' del
    /// CRS) con `douglas_peucker`, area (unita' al quadrato) con
    /// `preserve_topology`; 0 lascia la geometria invariata.
    pub tolerance: f64,
    /// Algoritmo; assente vale `douglas_peucker`.
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

/// `geo.clean_topology`: pulizia topologica ordinata di poligoni validi
/// (kernel `topology::clean_valid_polygon_topology`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanTopologyConfig {
    /// Raggio della chiusura morfologica, nelle unita' del CRS;
    /// obbligatorio, finito e non negativo (`InvalidPlan` altrimenti).
    /// Con `0` la chiusura non si fa.
    pub snap_tolerance: f64,
    /// Toglie a ogni riga la parte coperta dalle righe precedenti.
    /// Facoltativo: l'analisi lo accetta senza leggerlo, e quando manca il
    /// runner usa `true`.
    pub remove_overlaps: Option<bool>,
    /// Chiude, riga per riga, rientranze e varchi piu' stretti di
    /// `2 * snap_tolerance`. Facoltativo, come `remove_overlaps` (assente:
    /// `true`).
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

/// Config di `geo.from_coords`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromCoordsConfig {
    /// Colonna della x, `float64` o `int64`; assente vale `x`.
    pub x_column: Option<String>,
    /// Colonna della y, `float64` o `int64`; assente vale `y`.
    pub y_column: Option<String>,
    /// Nome della colonna geometria prodotta, libero; assente vale
    /// `geometry`.
    pub geometry_column: Option<String>,
    /// CRS della colonna prodotta (tabella integrata, o la definizione del
    /// CRS di piano scritta uguale); assente vale il CRS di piano, e senza
    /// nessuno dei due il passo si rifiuta. Deve essere proiettato.
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

/// `geo.sjoin`: join spaziale (kernel `spatial_join::spatial_join_nullable`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SJoinConfig {
    /// Il predicato che una coppia (sinistra, destra) deve soddisfare;
    /// obbligatorio: `intersects`, `contains`, `within`, `crosses`,
    /// `overlaps`, `touches` ([`JoinPredicate`]).
    pub predicate: JoinPredicate,
}

/// `geo.nearest`: vicino piu' prossimo (kernel
/// `analysis::nearest_matches`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NearestConfig {
    /// Distanza massima, nelle unita' del CRS: una riga sinistra il cui
    /// vicino e' piu' lontano non ha abbinamenti. Facoltativa (assente:
    /// nessun limite); finita e non negativa (`InvalidPlan` altrimenti).
    pub max_distance: Option<f64>,
}

/// `geo.overlay`: overlay poligonale con provenienza (kernel
/// `topology::polygon_overlay`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    /// Quali pezzi emettere; obbligatorio: `intersection`, `union`,
    /// `identity`, `symmetric_difference` ([`OverlayMode`]).
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
