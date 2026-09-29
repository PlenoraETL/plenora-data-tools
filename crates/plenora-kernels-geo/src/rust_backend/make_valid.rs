//! Riparazione geometrica (`LINEWORK` e `STRUCTURE`) in Rust puro, sopra il
//! polygonize di [`super::polygonize`] e l'overlay di `geo`.
//!
//! Portato da `plenora-memory-lab/operations/geo_rust/make_valid/src/lib.rs`
//! (SHA-256 `c32ce24489a53aaee7b814e873dbf166f6611df196899363c45619568e99630e`,
//! lo stesso registrato in `results/geo-rust/fuzz-provenance.json`). Le
//! modifiche sono solo quelle elencate nel modulo padre.
#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// Codice portato dal laboratorio (vedi il modulo padre): la numerica e'
// quella qualificata contro GEOS, e i lint qui sotto la toccherebbero.
// `mul_add` cambierebbe l'arrotondamento della normalizzazione dell'overlay
// e del test punto-su-segmento; `linework` resta una funzione sola come
// nella sorgente qualificata, e i nomi dei binding sono quelli.
#![allow(
    clippy::suboptimal_flops,
    clippy::similar_names,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};

use super::polygonize::{
    polygonize_linework_rust, PolygonizeError, PolygonizeLimits, PolygonizeOptions,
};
use geo::algorithm::validation::Validation;
use geo::line_intersection::{line_intersection, LineIntersection};
use geo::{
    Area, BooleanOps, Coord, CoordsIter, Geometry, GeometryCollection, Intersects, LineString,
    MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairMethod {
    Linework,
    Structure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MakeValidLimits {
    pub max_input_coordinates: u64,
    pub max_noding_work: u64,
    pub max_output_geometries: u64,
    pub max_output_coordinates: u64,
}

impl MakeValidLimits {
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_input_coordinates: u64::MAX,
            max_noding_work: u64::MAX,
            max_output_geometries: u64::MAX,
            max_output_coordinates: u64::MAX,
        }
    }

    /// Indica che input, noding e output hanno tutti un tetto esplicito.
    #[must_use]
    pub const fn is_fully_bounded(self) -> bool {
        self.max_input_coordinates != u64::MAX
            && self.max_noding_work != u64::MAX
            && self.max_output_geometries != u64::MAX
            && self.max_output_coordinates != u64::MAX
    }
}

#[derive(Debug, Error)]
pub enum MakeValidError {
    #[error("coordinate non finite")]
    NonFiniteCoordinate,
    #[error("geometria strutturalmente non valida: {0}")]
    InvalidStructure(&'static str),
    #[error("tipo non supportato dal candidato: {0}")]
    UnsupportedGeometry(&'static str),
    #[error("profilo limiti incompleto: input, noding e output devono avere tetti espliciti")]
    UnboundedLimitConfiguration,
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("lavoro di noding oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error("output oltre il limite di {limit}: {actual}")]
    OutputLimit { actual: u64, limit: u64 },
    #[error("indice non rappresentabile")]
    IndexOverflow,
    #[error("polygonize fallita: {0}")]
    Polygonize(PolygonizeError),
    #[error("output Rust ancora non valido: {0}")]
    InvalidOutput(String),
    #[error("invariante interna violata: {0}")]
    InternalInvariant(&'static str),
    #[error("prenotazione di memoria fallita per {0}")]
    AllocationFailed(&'static str),
}

impl From<PolygonizeError> for MakeValidError {
    fn from(error: PolygonizeError) -> Self {
        match error {
            PolygonizeError::CoordinateLimit { actual, limit } => {
                Self::CoordinateLimit { actual, limit }
            }
            PolygonizeError::WorkLimit { actual, limit } => Self::WorkLimit { actual, limit },
            PolygonizeError::OutputLimit { actual, limit } => Self::OutputLimit { actual, limit },
            PolygonizeError::IndexOverflow => Self::IndexOverflow,
            other => Self::Polygonize(other),
        }
    }
}

const fn polygonize_limits(limits: MakeValidLimits) -> PolygonizeLimits {
    PolygonizeLimits {
        max_input_coordinates: limits.max_input_coordinates,
        max_noding_work: limits.max_noding_work,
        max_output_geometries: limits.max_output_geometries,
        max_output_coordinates: limits.max_output_coordinates,
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CoordKey {
    x: u64,
    y: u64,
}

impl CoordKey {
    fn new(coordinate: Coord<f64>) -> Self {
        let x = if coordinate.x == 0.0 {
            0.0
        } else {
            coordinate.x
        };
        let y = if coordinate.y == 0.0 {
            0.0
        } else {
            coordinate.y
        };
        Self {
            x: x.to_bits(),
            y: y.to_bits(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SegmentKey(CoordKey, CoordKey);

impl SegmentKey {
    fn new(start: Coord<f64>, end: Coord<f64>) -> Option<Self> {
        let start = CoordKey::new(start);
        let end = CoordKey::new(end);
        match start.cmp(&end) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Less => Some(Self(start, end)),
            std::cmp::Ordering::Greater => Some(Self(end, start)),
        }
    }
}

const fn geometry_type(geometry: &Geometry<f64>) -> &'static str {
    match geometry {
        Geometry::Point(_) => "Point",
        Geometry::Line(_) => "Line",
        Geometry::LineString(_) => "LineString",
        Geometry::Polygon(_) => "Polygon",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
        Geometry::Rect(_) => "Rect",
        Geometry::Triangle(_) => "Triangle",
    }
}

fn segment_count(geometry: &Geometry<f64>) -> Result<u64, MakeValidError> {
    let line_segments = |line: &LineString<f64>| {
        u64::try_from(line.0.len().saturating_sub(1)).map_err(|_| MakeValidError::IndexOverflow)
    };
    let add = |left: u64, right: u64| left.checked_add(right).ok_or(MakeValidError::IndexOverflow);
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => Ok(0),
        Geometry::Line(_) => Ok(1),
        Geometry::LineString(line) => line_segments(line),
        Geometry::Polygon(polygon) => {
            let mut total = line_segments(polygon.exterior())?;
            for ring in polygon.interiors() {
                total = add(total, line_segments(ring)?)?;
            }
            Ok(total)
        }
        Geometry::MultiLineString(lines) => lines
            .0
            .iter()
            .try_fold(0_u64, |total, line| add(total, line_segments(line)?)),
        Geometry::MultiPolygon(polygons) => polygons.0.iter().try_fold(0_u64, |total, polygon| {
            add(total, segment_count(&Geometry::Polygon(polygon.clone()))?)
        }),
        Geometry::GeometryCollection(collection) => collection
            .0
            .iter()
            .try_fold(0_u64, |total, child| add(total, segment_count(child)?)),
        Geometry::Rect(_) => Ok(4),
        Geometry::Triangle(_) => Ok(3),
    }
}

fn checked_preflight(
    geometry: &Geometry<f64>,
    limits: MakeValidLimits,
) -> Result<(), MakeValidError> {
    let coordinates =
        u64::try_from(geometry.coords_count()).map_err(|_| MakeValidError::IndexOverflow)?;
    if coordinates > limits.max_input_coordinates {
        return Err(MakeValidError::CoordinateLimit {
            actual: coordinates,
            limit: limits.max_input_coordinates,
        });
    }
    let segments = segment_count(geometry)?;
    let work = segments
        .checked_mul(segments)
        .ok_or(MakeValidError::WorkLimit {
            actual: u64::MAX,
            limit: limits.max_noding_work,
        })?;
    if work > limits.max_noding_work {
        return Err(MakeValidError::WorkLimit {
            actual: work,
            limit: limits.max_noding_work,
        });
    }
    Ok(())
}

fn output_geometry_count(geometry: &Geometry<f64>) -> Result<u64, MakeValidError> {
    let count = match geometry {
        Geometry::MultiPoint(points) => points.0.len(),
        Geometry::MultiLineString(lines) => lines.0.len(),
        Geometry::MultiPolygon(polygons) => polygons.0.len(),
        Geometry::GeometryCollection(collection) => {
            return collection.0.iter().try_fold(0_u64, |total, child| {
                total
                    .checked_add(output_geometry_count(child)?)
                    .ok_or(MakeValidError::IndexOverflow)
            });
        }
        _ => 1,
    };
    u64::try_from(count).map_err(|_| MakeValidError::IndexOverflow)
}

fn checked_limits_output(
    geometry: &Geometry<f64>,
    limits: MakeValidLimits,
) -> Result<(), MakeValidError> {
    let geometries = output_geometry_count(geometry)?;
    if geometries > limits.max_output_geometries {
        return Err(MakeValidError::OutputLimit {
            actual: geometries,
            limit: limits.max_output_geometries,
        });
    }
    let coordinates =
        u64::try_from(geometry.coords_count()).map_err(|_| MakeValidError::IndexOverflow)?;
    if coordinates > limits.max_output_coordinates {
        return Err(MakeValidError::OutputLimit {
            actual: coordinates,
            limit: limits.max_output_coordinates,
        });
    }
    Ok(())
}

fn structurally_valid_ring(ring: &LineString<f64>) -> bool {
    ring.0.len() >= 4 && ring.0.first() == ring.0.last()
}

fn validate_structure(geometry: &Geometry<f64>) -> Result<(), MakeValidError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(MakeValidError::NonFiniteCoordinate);
    }
    let validate_polygon = |polygon: &Polygon<f64>| {
        if !structurally_valid_ring(polygon.exterior())
            || polygon
                .interiors()
                .iter()
                .any(|ring| !structurally_valid_ring(ring))
        {
            Err(MakeValidError::InvalidStructure(
                "anello non chiuso o con meno di quattro coordinate",
            ))
        } else {
            Ok(())
        }
    };
    match geometry {
        Geometry::Polygon(polygon) => validate_polygon(polygon),
        Geometry::MultiPolygon(polygons) => {
            for polygon in &polygons.0 {
                validate_polygon(polygon)?;
            }
            Ok(())
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                validate_structure(child)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn fixed_ring(
    ring: &LineString<f64>,
    limits: MakeValidLimits,
) -> Result<MultiPolygon<f64>, MakeValidError> {
    let result = polygonize_linework_rust(
        &Geometry::LineString(ring.clone()),
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: polygonize_limits(limits),
        },
    )?;
    Ok(MultiPolygon::new(result.polygons))
}

fn as_polygonal_geometry(polygons: MultiPolygon<f64>) -> Geometry<f64> {
    let mut values = polygons.0;
    if values.len() == 1 {
        values.pop().map_or_else(
            || Geometry::MultiPolygon(MultiPolygon::empty()),
            Geometry::Polygon,
        )
    } else {
        Geometry::MultiPolygon(MultiPolygon::new(values))
    }
}

#[derive(Clone, Copy, Debug)]
struct OverlayNormalizer {
    minimum_x: f64,
    minimum_y: f64,
    span_x: f64,
    span_y: f64,
}

impl OverlayNormalizer {
    fn new(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> Self {
        let mut minimum_x = f64::INFINITY;
        let mut maximum_x = f64::NEG_INFINITY;
        let mut minimum_y = f64::INFINITY;
        let mut maximum_y = f64::NEG_INFINITY;
        for coordinate in left.coords_iter().chain(right.coords_iter()) {
            minimum_x = minimum_x.min(coordinate.x);
            maximum_x = maximum_x.max(coordinate.x);
            minimum_y = minimum_y.min(coordinate.y);
            maximum_y = maximum_y.max(coordinate.y);
        }
        if !minimum_x.is_finite() {
            return Self {
                minimum_x: 0.0,
                minimum_y: 0.0,
                span_x: 1.0,
                span_y: 1.0,
            };
        }
        let span_x = maximum_x - minimum_x;
        let span_y = maximum_y - minimum_y;
        Self {
            minimum_x,
            minimum_y,
            span_x: if span_x == 0.0 { 1.0 } else { span_x },
            span_y: if span_y == 0.0 { 1.0 } else { span_y },
        }
    }

    fn normalize(self, coordinate: Coord<f64>) -> Coord<f64> {
        Coord {
            x: (coordinate.x - self.minimum_x) / self.span_x,
            y: (coordinate.y - self.minimum_y) / self.span_y,
        }
    }

    fn restore(self, coordinate: Coord<f64>) -> Coord<f64> {
        Coord {
            x: coordinate.x * self.span_x + self.minimum_x,
            y: coordinate.y * self.span_y + self.minimum_y,
        }
    }

    fn restore_multi_snapped(
        self,
        polygons: &MultiPolygon<f64>,
        left: &MultiPolygon<f64>,
        right: &MultiPolygon<f64>,
    ) -> MultiPolygon<f64> {
        let source_coordinates = left
            .coords_iter()
            .chain(right.coords_iter())
            .collect::<Vec<_>>();
        let tolerance_x = self.span_x.abs() * 4.0 / f64::from(i32::MAX);
        let tolerance_y = self.span_y.abs() * 4.0 / f64::from(i32::MAX);
        let snap = |value: f64, x_axis: bool| {
            let tolerance = if x_axis { tolerance_x } else { tolerance_y };
            let mut best = value;
            let mut best_distance = tolerance;
            for source in &source_coordinates {
                let candidate = if x_axis { source.x } else { source.y };
                let distance = (value - candidate).abs();
                if distance <= best_distance {
                    best = candidate;
                    best_distance = distance;
                }
            }
            best
        };
        let restore_line = |line: &LineString<f64>| {
            LineString::new(
                line.0
                    .iter()
                    .copied()
                    .map(|coordinate| {
                        let restored = self.restore(coordinate);
                        Coord {
                            x: snap(restored.x, true),
                            y: snap(restored.y, false),
                        }
                    })
                    .collect(),
            )
        };
        MultiPolygon::new(
            polygons
                .0
                .iter()
                .map(|polygon| {
                    Polygon::new(
                        restore_line(polygon.exterior()),
                        polygon.interiors().iter().map(restore_line).collect(),
                    )
                })
                .collect(),
        )
    }

    fn map_line(
        self,
        line: &LineString<f64>,
        transform: fn(Self, Coord<f64>) -> Coord<f64>,
    ) -> LineString<f64> {
        LineString::new(
            line.0
                .iter()
                .copied()
                .map(|coordinate| transform(self, coordinate))
                .collect(),
        )
    }

    fn map_polygon(
        self,
        polygon: &Polygon<f64>,
        transform: fn(Self, Coord<f64>) -> Coord<f64>,
    ) -> Polygon<f64> {
        Polygon::new(
            self.map_line(polygon.exterior(), transform),
            polygon
                .interiors()
                .iter()
                .map(|ring| self.map_line(ring, transform))
                .collect(),
        )
    }

    fn map_multi(
        self,
        polygons: &MultiPolygon<f64>,
        transform: fn(Self, Coord<f64>) -> Coord<f64>,
    ) -> MultiPolygon<f64> {
        MultiPolygon::new(
            polygons
                .0
                .iter()
                .map(|polygon| self.map_polygon(polygon, transform))
                .collect(),
        )
    }
}

fn normalized_union(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> MultiPolygon<f64> {
    if !normalized_intersects(left, right) {
        let mut polygons = left.0.clone();
        polygons.extend(right.0.iter().cloned());
        return MultiPolygon::new(polygons);
    }
    let normalizer = OverlayNormalizer::new(left, right);
    let normalized = normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .union(&normalizer.map_multi(right, OverlayNormalizer::normalize));
    normalizer.restore_multi_snapped(&normalized, left, right)
}

fn normalized_difference(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> MultiPolygon<f64> {
    let normalizer = OverlayNormalizer::new(left, right);
    let normalized = normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .difference(&normalizer.map_multi(right, OverlayNormalizer::normalize));
    normalizer.restore_multi_snapped(&normalized, left, right)
}

fn normalized_xor(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> MultiPolygon<f64> {
    let normalizer = OverlayNormalizer::new(left, right);
    let normalized = normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .xor(&normalizer.map_multi(right, OverlayNormalizer::normalize));
    normalizer.restore_multi_snapped(&normalized, left, right)
}

fn normalized_intersects(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> bool {
    let normalizer = OverlayNormalizer::new(left, right);
    normalizer
        .map_multi(left, OverlayNormalizer::normalize)
        .intersects(&normalizer.map_multi(right, OverlayNormalizer::normalize))
}

fn merge_polygon(area: &mut MultiPolygon<f64>, polygon: &Polygon<f64>) {
    if area.0.is_empty() {
        *area = MultiPolygon::new(vec![polygon.clone()]);
    } else {
        *area = normalized_union(area, &MultiPolygon::new(vec![polygon.clone()]));
    }
}

fn merge_multipolygon(area: &mut MultiPolygon<f64>, polygons: &MultiPolygon<f64>) {
    if area.0.is_empty() {
        *area = polygons.clone();
    } else if !polygons.0.is_empty() {
        *area = normalized_union(area, polygons);
    }
}

fn cleaned_coordinates(line: &LineString<f64>) -> Vec<Coord<f64>> {
    let mut coordinates = Vec::with_capacity(line.0.len());
    for coordinate in &line.0 {
        if coordinates.last() != Some(coordinate) {
            coordinates.push(*coordinate);
        }
    }
    coordinates
}

fn fix_line_element(line: &LineString<f64>, keep_collapsed: bool) -> Option<Geometry<f64>> {
    let coordinates = cleaned_coordinates(line);
    match coordinates.as_slice() {
        [coordinate] if keep_collapsed => {
            Some(Geometry::Point(Point::new(coordinate.x, coordinate.y)))
        }
        [] | [_] => None,
        _ => Some(Geometry::LineString(LineString::new(coordinates))),
    }
}

fn fix_line(line: &LineString<f64>, keep_collapsed: bool) -> Geometry<f64> {
    fix_line_element(line, keep_collapsed)
        .unwrap_or_else(|| Geometry::LineString(LineString::new(Vec::new())))
}

fn geometry_from_components(mut components: Vec<Geometry<f64>>) -> Geometry<f64> {
    if components.len() == 1 {
        components
            .pop()
            .unwrap_or_else(|| Geometry::GeometryCollection(GeometryCollection::empty()))
    } else {
        Geometry::GeometryCollection(GeometryCollection::new_from(components))
    }
}

fn structure_polygon(
    polygon: &Polygon<f64>,
    keep_collapsed: bool,
    limits: MakeValidLimits,
) -> Result<Option<Geometry<f64>>, MakeValidError> {
    let mut fixed = fixed_ring(polygon.exterior(), limits)?;
    if fixed.0.is_empty() {
        return Ok(if keep_collapsed {
            fix_line_element(polygon.exterior(), true)
        } else {
            None
        });
    }
    let shell = fixed.clone();
    let mut subtractive_holes = MultiPolygon::empty();
    let mut promoted_holes = MultiPolygon::empty();
    for hole in polygon.interiors() {
        let fixed_hole = fixed_ring(hole, limits)?;
        if normalized_intersects(&shell, &fixed_hole) {
            subtractive_holes = normalized_union(&subtractive_holes, &fixed_hole);
        } else {
            promoted_holes = normalized_union(&promoted_holes, &fixed_hole);
        }
    }
    if !subtractive_holes.0.is_empty() {
        fixed = normalized_difference(&fixed, &subtractive_holes);
    }
    if !promoted_holes.0.is_empty() {
        fixed = normalized_union(&fixed, &promoted_holes);
    }
    Ok(Some(as_polygonal_geometry(fixed)))
}

fn normalize_collapsed_union(
    components: Vec<Geometry<f64>>,
    limits: MakeValidLimits,
) -> Result<Vec<Geometry<f64>>, MakeValidError> {
    let mut lines = Vec::new();
    let mut output = Vec::new();
    for component in components {
        match component {
            Geometry::LineString(line) => lines.push(line),
            Geometry::MultiLineString(multi) => lines.extend(multi.0),
            other => output.push(other),
        }
    }
    if lines.is_empty() {
        return Ok(output);
    }
    let result = polygonize_linework_rust(
        &Geometry::MultiLineString(MultiLineString::new(lines)),
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: polygonize_limits(limits),
        },
    )?;
    if !result.polygons.is_empty() {
        output.push(as_polygonal_geometry(MultiPolygon::new(result.polygons)));
    }
    let residuals = result
        .cut_edges
        .into_iter()
        .chain(result.dangles)
        .chain(result.invalid_ring_lines)
        .collect::<Vec<_>>();
    if let Some(lines) = line_geometry(residuals) {
        output.push(lines);
    }
    Ok(output)
}

fn structure(
    geometry: &Geometry<f64>,
    keep_collapsed: bool,
    limits: MakeValidLimits,
) -> Result<Geometry<f64>, MakeValidError> {
    let output = match geometry {
        Geometry::Point(point) => Geometry::Point(*point),
        Geometry::Line(line) => Geometry::Line(*line),
        Geometry::LineString(line) => fix_line(line, keep_collapsed),
        Geometry::MultiPoint(points) => Geometry::MultiPoint(points.clone()),
        Geometry::MultiLineString(lines) => {
            let mut components = Vec::new();
            for line in &lines.0 {
                if let Some(fixed) = fix_line_element(line, keep_collapsed) {
                    components.push(fixed);
                }
            }
            if components
                .iter()
                .all(|value| matches!(value, Geometry::LineString(_)))
            {
                let fixed_lines = components
                    .into_iter()
                    .filter_map(|value| match value {
                        Geometry::LineString(line) => Some(line),
                        _ => None,
                    })
                    .collect();
                Geometry::MultiLineString(MultiLineString::new(fixed_lines))
            } else {
                geometry_from_components(components)
            }
        }
        Geometry::Polygon(polygon) => structure_polygon(polygon, keep_collapsed, limits)?
            .unwrap_or_else(|| {
                Geometry::Polygon(Polygon::new(LineString::new(Vec::new()), vec![]))
            }),
        Geometry::MultiPolygon(polygons) => {
            let mut area = MultiPolygon::empty();
            let mut collapsed = Vec::new();
            for polygon in &polygons.0 {
                match structure_polygon(polygon, keep_collapsed, limits)? {
                    Some(Geometry::Polygon(polygon)) => merge_polygon(&mut area, &polygon),
                    Some(Geometry::MultiPolygon(polygons)) => {
                        merge_multipolygon(&mut area, &polygons);
                    }
                    Some(other) => collapsed.push(other),
                    None => {}
                }
            }
            if !collapsed.is_empty() && (!area.0.is_empty() || collapsed.len() > 1) {
                collapsed = normalize_collapsed_union(collapsed, limits)?;
            }
            if !area.0.is_empty() {
                collapsed.insert(0, as_polygonal_geometry(area));
            }
            geometry_from_components(collapsed)
        }
        Geometry::GeometryCollection(collection) => {
            let mut fixed = Vec::with_capacity(collection.0.len());
            for child in &collection.0 {
                // GeometryFixer di GEOS ripara i figli di una collection con
                // la configurazione predefinita e non propaga keep_collapsed.
                fixed.push(make_valid_geometry_rust_impl(
                    child,
                    RepairMethod::Structure,
                    false,
                    limits,
                )?);
            }
            Geometry::GeometryCollection(GeometryCollection::new_from(fixed))
        }
        Geometry::Rect(rectangle) => Geometry::Rect(*rectangle),
        Geometry::Triangle(triangle) => Geometry::Triangle(*triangle),
    };
    output
        .check_validation()
        .map_err(|error| MakeValidError::InvalidOutput(error.to_string()))?;
    Ok(output)
}

fn collect_polygon_elements<'a>(
    geometry: &'a Geometry<f64>,
    polygons: &mut Vec<&'a Polygon<f64>>,
) -> Result<(), MakeValidError> {
    match geometry {
        Geometry::Polygon(polygon) => polygons.push(polygon),
        Geometry::MultiPolygon(multi) => {
            for polygon in &multi.0 {
                polygons.push(polygon);
            }
        }
        other => return Err(MakeValidError::UnsupportedGeometry(geometry_type(other))),
    }
    Ok(())
}

fn ring_segment_signature(ring: &LineString<f64>) -> Result<Vec<SegmentKey>, MakeValidError> {
    let mut signature = Vec::new();
    signature
        .try_reserve_exact(ring.0.len().saturating_sub(1))
        .map_err(|_| MakeValidError::AllocationFailed("firma dell'anello"))?;
    for segment in ring.lines() {
        if let Some(key) = SegmentKey::new(segment.start, segment.end) {
            signature.push(key);
        }
    }
    signature.sort_unstable();
    signature.dedup();
    Ok(signature)
}

fn polygonal_boundaries(polygons: &MultiPolygon<f64>) -> impl Iterator<Item = geo::Line<f64>> + '_ {
    polygons.0.iter().flat_map(|polygon| {
        std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .flat_map(LineString::lines)
    })
}

fn shares_collinear_boundary(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> bool {
    polygonal_boundaries(left).any(|left_segment| {
        polygonal_boundaries(right).any(|right_segment| {
            matches!(
                line_intersection(left_segment, right_segment),
                Some(LineIntersection::Collinear { intersection })
                    if intersection.start != intersection.end
            )
        })
    })
}

fn add_line_segments(
    line: &LineString<f64>,
    segments: &mut BTreeMap<SegmentKey, (Coord<f64>, Coord<f64>)>,
) {
    for segment in line.lines() {
        if let Some(key) = SegmentKey::new(segment.start, segment.end) {
            segments.entry(key).or_insert((segment.start, segment.end));
        }
    }
}

fn add_polygon_segments(
    polygon: &Polygon<f64>,
    segments: &mut BTreeMap<SegmentKey, (Coord<f64>, Coord<f64>)>,
) {
    add_line_segments(polygon.exterior(), segments);
    for ring in polygon.interiors() {
        add_line_segments(ring, segments);
    }
}

fn point_on_segment(point: Coord<f64>, start: Coord<f64>, end: Coord<f64>) -> bool {
    let coordinate_scale = point
        .x
        .abs()
        .max(point.y.abs())
        .max(start.x.abs())
        .max(start.y.abs())
        .max(end.x.abs())
        .max(end.y.abs())
        .max(1.0);
    let tolerance = (coordinate_scale * f64::EPSILON * 64.0).max(1e-9);
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length = dx.hypot(dy);
    if length == 0.0 {
        return (point.x - start.x).hypot(point.y - start.y) <= tolerance;
    }
    let cross = (point.y - start.y) * dx - (point.x - start.x) * dy;
    if cross.abs() > length * tolerance {
        return false;
    }
    point.x >= start.x.min(end.x) - tolerance
        && point.x <= start.x.max(end.x) + tolerance
        && point.y >= start.y.min(end.y) - tolerance
        && point.y <= start.y.max(end.y) + tolerance
}

fn segment_is_area_boundary(start: Coord<f64>, end: Coord<f64>, area: &MultiPolygon<f64>) -> bool {
    area.0.iter().any(|polygon| {
        std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .flat_map(LineString::lines)
            .any(|boundary| {
                point_on_segment(start, boundary.start, boundary.end)
                    && point_on_segment(end, boundary.start, boundary.end)
            })
    })
}

fn coordinate_is_represented(
    coordinate: Coord<f64>,
    area: &MultiPolygon<f64>,
    lines: &[LineString<f64>],
) -> bool {
    area.0
        .iter()
        .flat_map(|polygon| {
            std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .flat_map(LineString::lines)
        })
        .chain(lines.iter().flat_map(LineString::lines))
        .any(|segment| point_on_segment(coordinate, segment.start, segment.end))
}

fn validate_linework_output(geometry: &Geometry<f64>) -> Result<(), MakeValidError> {
    match geometry {
        Geometry::MultiPolygon(polygons) => {
            for polygon in &polygons.0 {
                Geometry::Polygon(polygon.clone())
                    .check_validation()
                    .map_err(|error| MakeValidError::InvalidOutput(error.to_string()))?;
            }
            Ok(())
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                validate_linework_output(child)?;
            }
            Ok(())
        }
        _ => geometry
            .check_validation()
            .map_err(|error| MakeValidError::InvalidOutput(error.to_string())),
    }
}

fn line_geometry(mut lines: Vec<LineString<f64>>) -> Option<Geometry<f64>> {
    if lines.len() == 1 {
        lines.pop().map(Geometry::LineString)
    } else if lines.is_empty() {
        None
    } else {
        Some(Geometry::MultiLineString(MultiLineString::new(lines)))
    }
}

fn point_geometry(mut points: Vec<Point<f64>>) -> Option<Geometry<f64>> {
    if points.len() == 1 {
        points.pop().map(Geometry::Point)
    } else if points.is_empty() {
        None
    } else {
        Some(Geometry::MultiPoint(MultiPoint::new(points)))
    }
}

fn linework(
    geometry: &Geometry<f64>,
    limits: MakeValidLimits,
) -> Result<Geometry<f64>, MakeValidError> {
    match geometry {
        Geometry::GeometryCollection(collection) => {
            let mut fixed = Vec::with_capacity(collection.0.len());
            for child in &collection.0 {
                fixed.push(make_valid_geometry_rust_impl(
                    child,
                    RepairMethod::Linework,
                    true,
                    limits,
                )?);
            }
            return Ok(Geometry::GeometryCollection(GeometryCollection::new_from(
                fixed,
            )));
        }
        Geometry::LineString(line) => return Ok(fix_line(line, true)),
        Geometry::MultiLineString(_) => return structure(geometry, true, limits),
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {}
        other => return Err(MakeValidError::UnsupportedGeometry(geometry_type(other))),
    }

    let mut polygons = Vec::new();
    collect_polygon_elements(geometry, &mut polygons)?;
    let original_coordinates = polygons
        .iter()
        .flat_map(|polygon| {
            std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .flat_map(|ring| ring.0.iter().copied())
        })
        .map(CoordKey::new)
        .collect::<BTreeSet<_>>();
    let mut boundaries = Vec::new();
    for polygon in &polygons {
        boundaries.push(polygon.exterior().clone());
        boundaries.extend(polygon.interiors().iter().cloned());
    }
    let result = polygonize_linework_rust(
        &Geometry::MultiLineString(MultiLineString::new(boundaries)),
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: polygonize_limits(limits),
        },
    )?;
    let mut area = MultiPolygon::empty();
    let mut retain_internal_edges = false;
    for polygon in &polygons {
        let mut seen_rings = BTreeSet::new();
        let shell_signature = ring_segment_signature(polygon.exterior())?;
        seen_rings.insert(shell_signature);
        let shell = fixed_ring(polygon.exterior(), limits)?;
        let mut polygon_area = shell.clone();
        for ring in polygon.interiors() {
            if !seen_rings.insert(ring_segment_signature(ring)?) {
                continue;
            }
            let fixed = fixed_ring(ring, limits)?;
            if fixed.0.is_empty() {
                continue;
            }
            if shares_collinear_boundary(&shell, &fixed) {
                retain_internal_edges = true;
                let outside = normalized_difference(&fixed, &shell).unsigned_area();
                let fixed_area = fixed.unsigned_area();
                if outside > fixed_area.max(1.0) * 1e-12 {
                    polygon_area = normalized_union(&polygon_area, &fixed);
                }
            } else {
                polygon_area = normalized_xor(&polygon_area, &fixed);
            }
        }
        if !polygon_area.0.is_empty() {
            if shares_collinear_boundary(&area, &polygon_area) {
                retain_internal_edges = true;
                area = normalized_union(&area, &polygon_area);
            } else {
                area = normalized_xor(&area, &polygon_area);
            }
        }
    }
    let mut all_segments = BTreeMap::new();
    if retain_internal_edges {
        for polygon in &result.polygons {
            add_polygon_segments(polygon, &mut all_segments);
        }
    }
    let include_cut_edges = retain_internal_edges || area.0.is_empty();
    for line in &result.dangles {
        add_line_segments(line, &mut all_segments);
    }
    if include_cut_edges {
        for line in result.cut_edges.iter().chain(&result.invalid_ring_lines) {
            add_line_segments(line, &mut all_segments);
        }
    }
    let mut lines = Vec::new();
    for (start, end) in all_segments.into_values() {
        if !segment_is_area_boundary(start, end, &area) {
            lines
                .try_reserve(1)
                .map_err(|_| MakeValidError::AllocationFailed("residui linework"))?;
            lines.push(LineString::new(vec![start, end]));
        }
    }
    let collapse_points = original_coordinates
        .iter()
        .map(|key| Coord {
            x: f64::from_bits(key.x),
            y: f64::from_bits(key.y),
        })
        .filter(|coordinate| !coordinate_is_represented(*coordinate, &area, &lines))
        .map(Point::from)
        .collect::<Vec<_>>();

    let mut components = Vec::new();
    if !area.0.is_empty() {
        components.push(as_polygonal_geometry(area));
    }
    if let Some(line_output) = line_geometry(lines) {
        components.push(line_output);
    }
    if let Some(point_output) = point_geometry(collapse_points) {
        components.push(point_output);
    }
    let output = geometry_from_components(components);
    validate_linework_output(&output)?;
    Ok(output)
}

/// Ripara una geometria senza GEOS.
///
/// `Structure` ripara separatamente shell e anelli interni, quindi applica
/// overlay Rust. `Linework` noda tutto il bordo, estrae le facce e conserva i
/// residui lineari o puntuali. Gli input gia' validi passano invariati.
///
/// # Errors
///
/// Restituisce un errore per payload non finiti/strutturalmente malformati,
/// tipi non supportati, polygonize fallita o output ancora invalido.
fn make_valid_geometry_rust_impl(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
    limits: MakeValidLimits,
) -> Result<Geometry<f64>, MakeValidError> {
    validate_structure(geometry)?;
    let non_degenerate_area = match geometry {
        Geometry::Polygon(polygon) => polygon.unsigned_area() > 0.0,
        Geometry::MultiPolygon(polygons) => polygons
            .0
            .iter()
            .all(|polygon| polygon.unsigned_area() > 0.0),
        Geometry::GeometryCollection(collection) => collection.0.iter().all(|child| match child {
            Geometry::Polygon(polygon) => polygon.unsigned_area() > 0.0,
            Geometry::MultiPolygon(polygons) => polygons
                .0
                .iter()
                .all(|polygon| polygon.unsigned_area() > 0.0),
            _ => true,
        }),
        _ => true,
    };
    if non_degenerate_area && geometry.check_validation().is_ok() {
        checked_limits_output(geometry, limits)?;
        return Ok(geometry.clone());
    }
    let output = match method {
        RepairMethod::Linework => linework(geometry, limits),
        RepairMethod::Structure => structure(geometry, keep_collapsed, limits),
    }?;
    checked_limits_output(&output, limits)?;
    Ok(output)
}

/// Ripara una geometria senza GEOS usando limiti non restrittivi.
///
/// # Errors
///
/// Restituisce un errore per payload non finiti/strutturalmente malformati,
/// tipi non supportati, polygonize fallita o output ancora invalido.
pub fn make_valid_geometry_rust(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
) -> Result<Geometry<f64>, MakeValidError> {
    make_valid_geometry_rust_impl(
        geometry,
        method,
        keep_collapsed,
        MakeValidLimits::unlimited(),
    )
}

/// Variante limitata di [`make_valid_geometry_rust`].
///
/// Il budget di noding usa conservativamente il quadrato del numero totale
/// di segmenti. Questo domina la somma dei lavori quadratici eseguiti sui
/// singoli anelli e impedisce di aggirare il limite spezzando l'input in una
/// collection.
///
/// # Errors
///
/// Oltre agli errori della riparazione, restituisce un errore prima del
/// kernel se coordinate o lavoro eccedono i limiti e dopo il kernel se
/// geometrie o coordinate di output eccedono i limiti dichiarati.
pub fn make_valid_geometry_rust_with_limits(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
    limits: MakeValidLimits,
) -> Result<Geometry<f64>, MakeValidError> {
    checked_preflight(geometry, limits)?;
    let output = make_valid_geometry_rust_impl(geometry, method, keep_collapsed, limits)?;
    checked_limits_output(&output, limits)?;
    Ok(output)
}

/// Variante per l'integrazione controllata che richiede tutti i budget.
///
/// # Errors
///
/// Restituisce [`MakeValidError::UnboundedLimitConfiguration`] se almeno un
/// limite e' lasciato a [`u64::MAX`]; altrimenti propaga gli errori di
/// [`make_valid_geometry_rust_with_limits`].
pub fn make_valid_geometry_rust_bounded(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
    limits: MakeValidLimits,
) -> Result<Geometry<f64>, MakeValidError> {
    if !limits.is_fully_bounded() {
        return Err(MakeValidError::UnboundedLimitConfiguration);
    }
    make_valid_geometry_rust_with_limits(geometry, method, keep_collapsed, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{polygon, Area};

    #[test]
    fn repairs_bow_tie_with_both_methods() -> Result<(), MakeValidError> {
        let input = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0)
        ]);
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            let output = make_valid_geometry_rust(&input, method, false)?;
            if (output.unsigned_area() - 2.0).abs() > 1e-12 {
                return Err(MakeValidError::InvalidOutput(format!(
                    "area inattesa: {}",
                    output.unsigned_area()
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn limited_api_fails_before_or_after_the_kernel() -> Result<(), MakeValidError> {
        let input = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0)
        ]);
        let coordinate_limited = make_valid_geometry_rust_with_limits(
            &input,
            RepairMethod::Structure,
            false,
            MakeValidLimits {
                max_input_coordinates: 4,
                ..MakeValidLimits::unlimited()
            },
        );
        if !matches!(
            coordinate_limited,
            Err(MakeValidError::CoordinateLimit { .. })
        ) {
            return Err(MakeValidError::InvalidOutput(
                "limite coordinate input non applicato".to_owned(),
            ));
        }
        let work_limited = make_valid_geometry_rust_with_limits(
            &input,
            RepairMethod::Linework,
            false,
            MakeValidLimits {
                max_noding_work: 15,
                ..MakeValidLimits::unlimited()
            },
        );
        if !matches!(work_limited, Err(MakeValidError::WorkLimit { .. })) {
            return Err(MakeValidError::InvalidOutput(
                "limite lavoro non applicato".to_owned(),
            ));
        }
        let output_limited = make_valid_geometry_rust_with_limits(
            &input,
            RepairMethod::Structure,
            false,
            MakeValidLimits {
                max_output_coordinates: 7,
                ..MakeValidLimits::unlimited()
            },
        );
        if !matches!(output_limited, Err(MakeValidError::OutputLimit { .. })) {
            return Err(MakeValidError::InvalidOutput(
                "limite coordinate output non applicato".to_owned(),
            ));
        }
        let geometry_limited = make_valid_geometry_rust_with_limits(
            &input,
            RepairMethod::Structure,
            false,
            MakeValidLimits {
                max_output_geometries: 1,
                ..MakeValidLimits::unlimited()
            },
        );
        if !matches!(geometry_limited, Err(MakeValidError::OutputLimit { .. })) {
            return Err(MakeValidError::InvalidOutput(
                "limite geometrie output non applicato".to_owned(),
            ));
        }
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            let first = make_valid_geometry_rust(&input, method, true)?;
            let second = make_valid_geometry_rust(&input, method, true)?;
            if first != second {
                return Err(MakeValidError::InvalidOutput(
                    "esecuzioni identiche non deterministiche".to_owned(),
                ));
            }
        }
        let non_finite = make_valid_geometry_rust(
            &Geometry::Point(Point::new(f64::NAN, 0.0)),
            RepairMethod::Structure,
            false,
        );
        if !matches!(non_finite, Err(MakeValidError::NonFiniteCoordinate)) {
            return Err(MakeValidError::InvalidOutput(
                "coordinata non finita accettata".to_owned(),
            ));
        }
        Ok(())
    }

    #[test]
    fn applies_linework_parity_to_a_crossing_hole() -> Result<(), MakeValidError> {
        let source = Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ]),
            vec![LineString::from(vec![
                (3.0, 1.0),
                (5.0, 1.0),
                (5.0, 3.0),
                (3.0, 3.0),
                (3.0, 1.0),
            ])],
        );
        let shell = fixed_ring(source.exterior(), MakeValidLimits::unlimited())?;
        let hole = fixed_ring(&source.interiors()[0], MakeValidLimits::unlimited())?;
        let selected = normalized_xor(&shell, &hole);
        if (selected.unsigned_area() - 16.0).abs() > 1e-12 {
            return Err(MakeValidError::InvalidOutput(format!(
                "area selezionata={}",
                selected.unsigned_area()
            )));
        }
        Ok(())
    }
}
