//! Schemi JSON dei comandi transform-arrow (`TransformArrowSchema`) e
//! catalogo delle operazioni unary (`ArrowOperation`).

use serde::Deserialize;

use plenora_core::crs::MAX_CRS_DEFINITION_BYTES;
use plenora_kernels_geo::operations::{BufferCapStyle, SimplifyPolicy};
use plenora_kernels_geo::Operation;

use super::error::ArrowTransportError;
use super::protocol::MAX_ROWS;
use super::transport::{DEFAULT_GEOMETRY_COLUMN, DEFAULT_X_COLUMN, DEFAULT_Y_COLUMN};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ArrowOperation {
    Centroid,
    ConvexHull,
    Envelope,
    Buffer,
    Simplify,
    Boundary,
    PointOnSurface,
    MakeValid,
    Reproject,
    Area,
    Length,
    Perimeter,
    VertexCount,
    Bounds,
    ToWkt,
    Explode,
    Dissolve,
    LineBuilder,
    PolygonBuilder,
    Voronoi,
    FromCoords,
    CleanTopology,
    AffineTransform,
    Translate,
    Scale,
    Rotate,
    ConcaveHull,
    Densify,
    SnapToGrid,
    LineSubstring,
    LineInterpolatePoint,
    GeodesicLineLength,
    GeodesicArea,
    GeometryDiagnostics,
    Delaunay,
    Polygonize,
    LineMerge,
}

/// Cardinalita' input/output dell'operazione sulle righe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrowShape {
    OneToOne,
    OneToMany,
    ManyToOne,
    /// Calcolo sull'intera tabella con output allineato alle righe non-null
    /// (`voronoi`, `clean_topology`).
    Collective,
    /// Tutta la tabella produce un insieme non allineato di righe
    /// (`polygonize`, `line_merge`).
    WholeToMany,
    /// Nessuna colonna geometria in input: Point da due colonne numeriche.
    FromCoords,
    /// Struct diagnostico per riga (`geometry_diagnostics`).
    Diagnostic,
}

impl ArrowOperation {
    pub const ALL: [Self; 37] = [
        Self::Centroid,
        Self::ConvexHull,
        Self::Envelope,
        Self::Buffer,
        Self::Simplify,
        Self::Boundary,
        Self::PointOnSurface,
        Self::MakeValid,
        Self::Reproject,
        Self::Area,
        Self::Length,
        Self::Perimeter,
        Self::VertexCount,
        Self::Bounds,
        Self::ToWkt,
        Self::Explode,
        Self::Dissolve,
        Self::LineBuilder,
        Self::PolygonBuilder,
        Self::Voronoi,
        Self::FromCoords,
        Self::CleanTopology,
        Self::AffineTransform,
        Self::Translate,
        Self::Scale,
        Self::Rotate,
        Self::ConcaveHull,
        Self::Densify,
        Self::SnapToGrid,
        Self::LineSubstring,
        Self::LineInterpolatePoint,
        Self::GeodesicLineLength,
        Self::GeodesicArea,
        Self::GeometryDiagnostics,
        Self::Delaunay,
        Self::Polygonize,
        Self::LineMerge,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Centroid => "centroid",
            Self::ConvexHull => "convex_hull",
            Self::Envelope => "envelope",
            Self::Buffer => "buffer",
            Self::Simplify => "simplify",
            Self::Boundary => "boundary",
            Self::PointOnSurface => "point_on_surface",
            Self::MakeValid => "make_valid",
            Self::Reproject => "reproject",
            Self::Area => "area",
            Self::Length => "length",
            Self::Perimeter => "perimeter",
            Self::VertexCount => "vertex_count",
            Self::Bounds => "bounds",
            Self::ToWkt => "to_wkt",
            Self::Explode => "explode",
            Self::Dissolve => "dissolve",
            Self::LineBuilder => "line_builder",
            Self::PolygonBuilder => "polygon_builder",
            Self::Voronoi => "voronoi",
            Self::FromCoords => "from_coords",
            Self::CleanTopology => "clean_topology",
            Self::AffineTransform => "affine_transform",
            Self::Translate => "translate",
            Self::Scale => "scale",
            Self::Rotate => "rotate",
            Self::ConcaveHull => "concave_hull",
            Self::Densify => "densify",
            Self::SnapToGrid => "snap_to_grid",
            Self::LineSubstring => "line_substring",
            Self::LineInterpolatePoint => "line_interpolate_point",
            Self::GeodesicLineLength => "geodesic_line_length",
            Self::GeodesicArea => "geodesic_area",
            Self::GeometryDiagnostics => "geometry_diagnostics",
            Self::Delaunay => "delaunay",
            Self::Polygonize => "polygonize",
            Self::LineMerge => "line_merge",
        }
    }

    /// Nome della voce di catalogo usata dal livello comandi per il requisito CRS.
    #[must_use]
    pub const fn catalog_name(self) -> &'static str {
        match self {
            Self::Centroid => "geo_centroid",
            Self::ConvexHull => "geo_convex_hull",
            Self::Envelope => "geo_envelope",
            Self::Buffer => "geo_buffer",
            Self::Simplify => "geo_simplify",
            Self::Boundary => "geo_boundary",
            Self::PointOnSurface => "geo_point_on_surface",
            Self::MakeValid => "geo_make_valid",
            Self::Reproject => "geo_reproject",
            Self::Area => "geo_area",
            Self::Length => "geo_length",
            Self::Perimeter => "geo_perimeter",
            Self::VertexCount => "geo_vertex_count",
            Self::Bounds => "geo_bounds_extractor",
            Self::ToWkt => "geo_to_wkt",
            Self::Explode => "geo_explode",
            Self::Dissolve => "geo_dissolve",
            Self::LineBuilder => "geo_line_builder",
            Self::PolygonBuilder => "geo_polygon_builder",
            Self::Voronoi => "geo_voronoi",
            Self::FromCoords => "geo_from_coords",
            Self::CleanTopology => "geo_clean_topology",
            Self::AffineTransform => "affine_transform",
            Self::Translate => "translate",
            Self::Scale => "scale",
            Self::Rotate => "rotate",
            Self::ConcaveHull => "concave_hull",
            Self::Densify => "densify",
            Self::SnapToGrid => "snap_to_grid",
            Self::Delaunay => "delaunay",
            Self::Polygonize => "polygonize",
            Self::LineMerge => "line_merge",
            Self::LineSubstring => "line_substring",
            Self::LineInterpolatePoint => "line_interpolate_point",
            Self::GeodesicLineLength => "geodesic_line_length",
            Self::GeodesicArea => "geodesic_area",
            Self::GeometryDiagnostics => "geometry_diagnostics",
        }
    }

    #[must_use]
    pub const fn shape(self) -> ArrowShape {
        match self {
            Self::Explode | Self::Delaunay => ArrowShape::OneToMany,
            Self::Dissolve
            | Self::LineBuilder
            | Self::PolygonBuilder => ArrowShape::ManyToOne,
            Self::Voronoi | Self::CleanTopology => ArrowShape::Collective,
            Self::Polygonize | Self::LineMerge => ArrowShape::WholeToMany,
            Self::GeometryDiagnostics => ArrowShape::Diagnostic,
            Self::FromCoords => ArrowShape::FromCoords,
            _ => ArrowShape::OneToOne,
        }
    }

    pub(in crate::geo_transport) const fn geometry_kernel(self) -> Option<Operation> {
        match self {
            Self::Centroid => Some(Operation::Centroid),
            Self::ConvexHull => Some(Operation::ConvexHull),
            Self::Envelope => Some(Operation::Envelope),
            _ => None,
        }
    }

    /// Vero se l'output sostituisce la colonna geometria con una nuova
    /// colonna GeoArrow-WKB (con metadato CRS), falso per output scalari.
    /// Rilevante solo per le operazioni 1:1.
    pub(in crate::geo_transport) const fn produces_geometry(self) -> bool {
        matches!(
            self,
            Self::Centroid
                | Self::ConvexHull
                | Self::Envelope
                | Self::Buffer
                | Self::Simplify
                | Self::Boundary
                | Self::PointOnSurface
                | Self::MakeValid
                | Self::Reproject
                | Self::AffineTransform
                | Self::Translate
                | Self::Scale
                | Self::Rotate
                | Self::ConcaveHull
                | Self::Densify
                | Self::SnapToGrid
                | Self::LineSubstring
                | Self::LineInterpolatePoint
        )
    }
}

/// Stile di cap per `buffer` (default round, come il kernel).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BufferCap {
    Round,
    Flat,
    Square,
}

impl From<BufferCap> for BufferCapStyle {
    fn from(cap: BufferCap) -> Self {
        match cap {
            BufferCap::Round => Self::Round,
            BufferCap::Flat => Self::Flat,
            BufferCap::Square => Self::Square,
        }
    }
}

/// Politica di `simplify`: Douglas-Peucker (default) oppure topology-preserving.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SimplifyPolicyParam {
    DouglasPeucker,
    PreserveTopology,
}

impl From<SimplifyPolicyParam> for SimplifyPolicy {
    fn from(policy: SimplifyPolicyParam) -> Self {
        match policy {
            SimplifyPolicyParam::DouglasPeucker => Self::DouglasPeucker,
            SimplifyPolicyParam::PreserveTopology => Self::PreserveTopology,
        }
    }
}

/// Schema JSON del comando transform-arrow (`schema_version: 3`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformArrowSchema {
    pub schema_version: u32,
    pub operation: ArrowOperation,
    pub row_count: u64,
    pub crs: Option<String>,
    pub geometry_column: Option<String>,
    pub distance: Option<f64>,
    pub cap: Option<BufferCap>,
    pub tolerance: Option<f64>,
    pub simplify_policy: Option<SimplifyPolicyParam>,
    pub target_crs: Option<String>,
    pub max_output_rows: Option<u64>,
    pub max_points: Option<u64>,
    pub x_column: Option<String>,
    pub y_column: Option<String>,
    pub snap_tolerance: Option<f64>,
    pub remove_overlaps: Option<bool>,
    pub fill_gaps: Option<bool>,
    pub coefficients: Option<Vec<f64>>,
    pub x_offset: Option<f64>,
    pub y_offset: Option<f64>,
    pub x_factor: Option<f64>,
    pub y_factor: Option<f64>,
    pub degrees: Option<f64>,
    pub x_origin: Option<f64>,
    pub y_origin: Option<f64>,
    pub concavity: Option<f64>,
    pub length_threshold: Option<f64>,
    pub max_segment_length: Option<f64>,
    pub grid_size: Option<f64>,
    pub start_ratio: Option<f64>,
    pub end_ratio: Option<f64>,
    pub ratio: Option<f64>,
    pub node_input: Option<bool>,
    pub require_complete: Option<bool>,
}

impl TransformArrowSchema {
    pub const VERSION: u32 = 3;

    #[must_use]
    pub fn geometry_column(&self) -> &str {
        self.geometry_column
            .as_deref()
            .unwrap_or(DEFAULT_GEOMETRY_COLUMN)
    }

    #[must_use]
    pub fn x_column(&self) -> &str {
        self.x_column.as_deref().unwrap_or(DEFAULT_X_COLUMN)
    }

    #[must_use]
    pub fn y_column(&self) -> &str {
        self.y_column.as_deref().unwrap_or(DEFAULT_Y_COLUMN)
    }

    /// Limite di espansione righe: obbligatorio per le operazioni 1:N,
    /// default `MAX_ROWS` per le altre.
    #[must_use]
    pub fn max_output_rows_limit(&self) -> u64 {
        self.max_output_rows.unwrap_or(MAX_ROWS)
    }

    pub(in crate::geo_transport) fn required_max_output_rows(&self) -> Result<u64, ArrowTransportError> {
        self.max_output_rows
            .ok_or_else(|| ArrowTransportError::MissingParameter {
                operation: self.operation.name(),
                name: "max_output_rows",
            })
    }

    pub(in crate::geo_transport) fn required_distance(&self) -> Result<f64, ArrowTransportError> {
        let distance = self.distance.ok_or_else(|| ArrowTransportError::MissingParameter {
            operation: self.operation.name(),
            name: "distance",
        })?;
        if !distance.is_finite() {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name: "distance",
                reason: "deve essere finita",
            });
        }
        Ok(distance)
    }

    pub(in crate::geo_transport) fn required_tolerance(&self) -> Result<f64, ArrowTransportError> {
        let tolerance = self
            .tolerance
            .ok_or_else(|| ArrowTransportError::MissingParameter {
                operation: self.operation.name(),
                name: "tolerance",
            })?;
        if !tolerance.is_finite() || tolerance < 0.0 {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name: "tolerance",
                reason: "deve essere finita e non negativa",
            });
        }
        Ok(tolerance)
    }

    pub(in crate::geo_transport) fn required_target_crs(&self) -> Result<&str, ArrowTransportError> {
        let target = self
            .target_crs
            .as_deref()
            .ok_or_else(|| ArrowTransportError::MissingParameter {
                operation: self.operation.name(),
                name: "target_crs",
            })?;
        if target.trim().is_empty() {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name: "target_crs",
                reason: "non deve essere vuoto",
            });
        }
        if target.len() > MAX_CRS_DEFINITION_BYTES {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name: "target_crs",
                reason: "oltre il limite di byte per definizione CRS",
            });
        }
        Ok(target)
    }

    fn present_extension_params(&self) -> Vec<&'static str> {
        let mut present = Vec::new();
        if self.coefficients.is_some() {
            present.push("coefficients");
        }
        if self.x_offset.is_some() {
            present.push("x_offset");
        }
        if self.y_offset.is_some() {
            present.push("y_offset");
        }
        if self.x_factor.is_some() {
            present.push("x_factor");
        }
        if self.y_factor.is_some() {
            present.push("y_factor");
        }
        if self.degrees.is_some() {
            present.push("degrees");
        }
        if self.x_origin.is_some() {
            present.push("x_origin");
        }
        if self.y_origin.is_some() {
            present.push("y_origin");
        }
        if self.concavity.is_some() {
            present.push("concavity");
        }
        if self.length_threshold.is_some() {
            present.push("length_threshold");
        }
        if self.max_segment_length.is_some() {
            present.push("max_segment_length");
        }
        if self.grid_size.is_some() {
            present.push("grid_size");
        }
        if self.start_ratio.is_some() {
            present.push("start_ratio");
        }
        if self.end_ratio.is_some() {
            present.push("end_ratio");
        }
        if self.ratio.is_some() {
            present.push("ratio");
        }
        if self.node_input.is_some() {
            present.push("node_input");
        }
        if self.require_complete.is_some() {
            present.push("require_complete");
        }
        present
    }

    fn check_extension_params(&self, allowed: &[&'static str]) -> Result<(), ArrowTransportError> {
        for name in self.present_extension_params() {
            if !allowed.contains(&name) {
                return Err(ArrowTransportError::UnexpectedParameter {
                    operation: self.operation.name(),
                    name,
                });
            }
        }
        Ok(())
    }

    pub(in crate::geo_transport) fn required_f64(
        &self,
        name: &'static str,
        value: Option<f64>,
    ) -> Result<f64, ArrowTransportError> {
        value.ok_or_else(|| ArrowTransportError::MissingParameter {
            operation: self.operation.name(),
            name,
        })
    }

    const fn finite_param(&self, name: &'static str, value: f64) -> Result<f64, ArrowTransportError> {
        if !value.is_finite() {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name,
                reason: "deve essere finito",
            });
        }
        Ok(value)
    }

    fn ratio_param(&self, name: &'static str, value: f64) -> Result<f64, ArrowTransportError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ArrowTransportError::InvalidParameter {
                operation: self.operation.name(),
                name,
                reason: "deve essere finito e compreso tra zero e uno",
            });
        }
        Ok(value)
    }

    /// Verifica che i parametri presenti siano esattamente quelli previsti
    /// dall'operazione e che i valori siano nel dominio del kernel.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::MissingParameter` se manca un parametro
    /// obbligatorio, `ArrowTransportError::UnexpectedParameter` se ne e'
    /// presente uno non previsto dall'operazione,
    /// `ArrowTransportError::InvalidParameter` se un valore e' fuori dominio.
    // Dispatch per operazione intenzionalmente in un'unica funzione: la
    // tabella parametri ammessi/obbligatori resta leggibile come tabella;
    // la scomposizione strutturale e' rimandata a una fase dedicata.
    #[allow(clippy::too_many_lines)]
    pub fn validate_parameters(&self) -> Result<(), ArrowTransportError> {
        let operation = self.operation.name();
        let unexpected = |name: &'static str, present: bool| {
            if present {
                Err(ArrowTransportError::UnexpectedParameter { operation, name })
            } else {
                Ok(())
            }
        };
        // Parametri delle estensioni ammessi per operazione; tutti gli altri
        // sono rifiutati prima di toccare i dati.
        let extension_allowed: &[&'static str] = match self.operation {
            ArrowOperation::AffineTransform => &["coefficients"],
            ArrowOperation::Translate => &["x_offset", "y_offset"],
            ArrowOperation::Scale => &["x_factor", "y_factor", "x_origin", "y_origin"],
            ArrowOperation::Rotate => &["degrees", "x_origin", "y_origin"],
            ArrowOperation::ConcaveHull => &["concavity", "length_threshold"],
            ArrowOperation::Densify => &["max_segment_length"],
            ArrowOperation::SnapToGrid => &["grid_size"],
            ArrowOperation::LineSubstring => &["start_ratio", "end_ratio"],
            ArrowOperation::LineInterpolatePoint => &["ratio"],
            ArrowOperation::Polygonize => &["node_input", "require_complete"],
            _ => &[],
        };
        self.check_extension_params(extension_allowed)?;
        let reject_geometry_params = |schema: &Self| -> Result<(), ArrowTransportError> {
            unexpected("distance", schema.distance.is_some())?;
            unexpected("cap", schema.cap.is_some())?;
            unexpected("tolerance", schema.tolerance.is_some())?;
            unexpected("simplify_policy", schema.simplify_policy.is_some())?;
            unexpected("target_crs", schema.target_crs.is_some())?;
            Ok(())
        };
        let reject_builder_params = |schema: &Self| -> Result<(), ArrowTransportError> {
            unexpected("max_points", schema.max_points.is_some())?;
            unexpected("x_column", schema.x_column.is_some())?;
            unexpected("y_column", schema.y_column.is_some())?;
            Ok(())
        };
        let reject_clean_params = |schema: &Self| -> Result<(), ArrowTransportError> {
            unexpected("snap_tolerance", schema.snap_tolerance.is_some())?;
            unexpected("remove_overlaps", schema.remove_overlaps.is_some())?;
            unexpected("fill_gaps", schema.fill_gaps.is_some())?;
            Ok(())
        };
        match self.operation {
            ArrowOperation::Buffer => {
                self.required_distance()?;
                unexpected("tolerance", self.tolerance.is_some())?;
                unexpected("simplify_policy", self.simplify_policy.is_some())?;
                unexpected("target_crs", self.target_crs.is_some())?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
            ArrowOperation::Simplify => {
                self.required_tolerance()?;
                unexpected("distance", self.distance.is_some())?;
                unexpected("cap", self.cap.is_some())?;
                unexpected("target_crs", self.target_crs.is_some())?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
            ArrowOperation::Reproject => {
                self.required_target_crs()?;
                unexpected("distance", self.distance.is_some())?;
                unexpected("cap", self.cap.is_some())?;
                unexpected("tolerance", self.tolerance.is_some())?;
                unexpected("simplify_policy", self.simplify_policy.is_some())?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
            ArrowOperation::Explode | ArrowOperation::Delaunay => {
                self.required_max_output_rows()?;
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
            ArrowOperation::Voronoi => {
                reject_geometry_params(self)?;
                unexpected("x_column", self.x_column.is_some())?;
                unexpected("y_column", self.y_column.is_some())?;
                reject_clean_params(self)?;
                if let Some(max_points) = self.max_points {
                    if !(2..=MAX_ROWS).contains(&max_points) {
                        return Err(ArrowTransportError::InvalidParameter {
                            operation,
                            name: "max_points",
                            reason: "deve essere tra 2 e il limite righe del trasporto",
                        });
                    }
                }
            }
            ArrowOperation::FromCoords => {
                reject_geometry_params(self)?;
                unexpected("max_points", self.max_points.is_some())?;
                reject_clean_params(self)?;
                for (name, value) in [
                    ("x_column", self.x_column.as_deref()),
                    ("y_column", self.y_column.as_deref()),
                ] {
                    if value.is_some_and(|column| column.trim().is_empty()) {
                        return Err(ArrowTransportError::InvalidParameter {
                            operation,
                            name,
                            reason: "non deve essere vuoto",
                        });
                    }
                }
            }
            ArrowOperation::AffineTransform => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let coefficients =
                    self.coefficients
                        .as_ref()
                        .ok_or(ArrowTransportError::MissingParameter {
                            operation,
                            name: "coefficients",
                        })?;
                if coefficients.len() != 6 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "coefficients",
                        reason: "devono essere esattamente 6 coefficienti",
                    });
                }
                if coefficients.iter().any(|value| !value.is_finite()) {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "coefficients",
                        reason: "devono essere finiti",
                    });
                }
            }
            ArrowOperation::Translate => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                self.finite_param("x_offset", self.required_f64("x_offset", self.x_offset)?)?;
                self.finite_param("y_offset", self.required_f64("y_offset", self.y_offset)?)?;
            }
            ArrowOperation::Scale => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                self.finite_param("x_factor", self.required_f64("x_factor", self.x_factor)?)?;
                self.finite_param("y_factor", self.required_f64("y_factor", self.y_factor)?)?;
                if let Some(value) = self.x_origin {
                    self.finite_param("x_origin", value)?;
                }
                if let Some(value) = self.y_origin {
                    self.finite_param("y_origin", value)?;
                }
            }
            ArrowOperation::Rotate => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                self.finite_param("degrees", self.required_f64("degrees", self.degrees)?)?;
                if let Some(value) = self.x_origin {
                    self.finite_param("x_origin", value)?;
                }
                if let Some(value) = self.y_origin {
                    self.finite_param("y_origin", value)?;
                }
            }
            ArrowOperation::ConcaveHull => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let concavity = self
                    .finite_param("concavity", self.required_f64("concavity", self.concavity)?)?;
                if concavity <= 0.0 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "concavity",
                        reason: "deve essere maggiore di zero",
                    });
                }
                if let Some(value) = self.length_threshold {
                    let value = self.finite_param("length_threshold", value)?;
                    if value < 0.0 {
                        return Err(ArrowTransportError::InvalidParameter {
                            operation,
                            name: "length_threshold",
                            reason: "deve essere non negativa",
                        });
                    }
                }
            }
            ArrowOperation::Densify => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let value = self.finite_param(
                    "max_segment_length",
                    self.required_f64("max_segment_length", self.max_segment_length)?,
                )?;
                if value <= 0.0 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "max_segment_length",
                        reason: "deve essere maggiore di zero",
                    });
                }
            }
            ArrowOperation::SnapToGrid => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let value = self
                    .finite_param("grid_size", self.required_f64("grid_size", self.grid_size)?)?;
                if value <= 0.0 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "grid_size",
                        reason: "deve essere maggiore di zero",
                    });
                }
            }
            ArrowOperation::LineSubstring => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                let start = self.ratio_param(
                    "start_ratio",
                    self.required_f64("start_ratio", self.start_ratio)?,
                )?;
                let end =
                    self.ratio_param("end_ratio", self.required_f64("end_ratio", self.end_ratio)?)?;
                if start > end {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "start_ratio/end_ratio",
                        reason: "start_ratio non puo superare end_ratio",
                    });
                }
            }
            ArrowOperation::LineInterpolatePoint => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
                self.ratio_param("ratio", self.required_f64("ratio", self.ratio)?)?;
            }
            ArrowOperation::CleanTopology => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                let tolerance =
                    self.snap_tolerance
                        .ok_or(ArrowTransportError::MissingParameter {
                            operation,
                            name: "snap_tolerance",
                        })?;
                if !tolerance.is_finite() || tolerance < 0.0 {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "snap_tolerance",
                        reason: "deve essere finita e non negativa",
                    });
                }
            }
            _ => {
                reject_geometry_params(self)?;
                reject_builder_params(self)?;
                reject_clean_params(self)?;
            }
        }
        if let Some(limit) = self.max_output_rows {
            if limit > MAX_ROWS {
                return Err(ArrowTransportError::InvalidParameter {
                    operation,
                    name: "max_output_rows",
                    reason: "oltre il limite righe del trasporto",
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct TransformArrowSummary {
    /// Righe di input lette (deve coincidere con `row_count` dello schema).
    pub rows: u64,
    /// Righe di output prodotte: diverge da `rows` per le forme 1:N e N:1.
    pub output_rows: u64,
    pub checksum: [u8; 32],
}
