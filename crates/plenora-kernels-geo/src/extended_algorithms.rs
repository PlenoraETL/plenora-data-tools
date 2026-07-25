//! Bounded algorithms extending the original Manipola operation set.

use geo::algorithm::line_measures::{
    Bearing, Densify, Euclidean, FrechetDistance, Geodesic, InterpolateLine, Length,
};
use geo::algorithm::orient::{Direction, Orient};
use geo::algorithm::validation::Validation;
use geo::line_intersection::{line_intersection, LineIntersection};
use geo::{
    Coord, CoordsIter, GeodesicArea, Geometry, Line, LineString, MapCoords, MultiPolygon, Point,
    Polygon, TriangulateDelaunayUnconstrained,
};
use rstar::{RTree, RTreeObject, AABB};
use serde::Serialize;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtendedAlgorithmError {
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    #[error("geometria prodotta non valida: {0}")]
    InvalidOutput(String),
    #[error("tipo geometria non supportato da {operation}: {actual}")]
    UnsupportedGeometry {
        operation: &'static str,
        actual: &'static str,
    },
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("output oltre il limite di {limit}: {actual}")]
    OutputLimit { actual: u64, limit: u64 },
    #[error("lavoro quadratico oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error("triangolazione fallita: {0}")]
    Triangulation(String),
    #[error("coordinate geografiche fuori intervallo lon/lat")]
    InvalidGeographicCoordinate,
    #[error("conteggio non rappresentabile come uint64")]
    IndexOverflow,
}

fn geometry_type(geometry: &Geometry<f64>) -> &'static str {
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

fn validate_input(geometry: &Geometry<f64>) -> Result<(), ExtendedAlgorithmError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtendedAlgorithmError::InvalidInput(
            "coordinate NaN o infinite".to_owned(),
        ));
    }
    geometry
        .check_validation()
        .map_err(|error| ExtendedAlgorithmError::InvalidInput(error.to_string()))
}

fn validate_output(geometry: Geometry<f64>) -> Result<Geometry<f64>, ExtendedAlgorithmError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "coordinate NaN o infinite".to_owned(),
        ));
    }
    geometry
        .check_validation()
        .map_err(|error| ExtendedAlgorithmError::InvalidOutput(error.to_string()))?;
    Ok(geometry)
}

fn coordinate_count(geometry: &Geometry<f64>) -> Result<u64, ExtendedAlgorithmError> {
    u64::try_from(geometry.coords_count()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)
}

fn checked_densified_line_count(
    line: &LineString<f64>,
    max_segment_length: f64,
) -> Result<u64, ExtendedAlgorithmError> {
    if line.0.is_empty() {
        return Ok(0);
    }
    let mut total = 1_u64;
    for segment in line.lines() {
        let dx = segment.end.x - segment.start.x;
        let dy = segment.end.y - segment.start.y;
        let length = dx.hypot(dy);
        let pieces = (length / max_segment_length).ceil();
        if !pieces.is_finite() || pieces > u64::MAX as f64 {
            return Err(ExtendedAlgorithmError::IndexOverflow);
        }
        total = total
            .checked_add((pieces as u64).max(1))
            .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
    }
    Ok(total)
}

fn densified_count(
    geometry: &Geometry<f64>,
    max_segment_length: f64,
) -> Result<u64, ExtendedAlgorithmError> {
    match geometry {
        Geometry::Point(_) => Ok(1),
        Geometry::Line(_) => Err(ExtendedAlgorithmError::UnsupportedGeometry {
            operation: "densify",
            actual: "Line",
        }),
        Geometry::LineString(line) => checked_densified_line_count(line, max_segment_length),
        Geometry::Polygon(polygon) => {
            let mut total = checked_densified_line_count(polygon.exterior(), max_segment_length)?;
            for ring in polygon.interiors() {
                total = total
                    .checked_add(checked_densified_line_count(ring, max_segment_length)?)
                    .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
            }
            Ok(total)
        }
        Geometry::MultiPoint(points) => {
            u64::try_from(points.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)
        }
        Geometry::MultiLineString(lines) => lines.0.iter().try_fold(0_u64, |total, line| {
            total
                .checked_add(checked_densified_line_count(line, max_segment_length)?)
                .ok_or(ExtendedAlgorithmError::IndexOverflow)
        }),
        Geometry::MultiPolygon(polygons) => polygons.0.iter().try_fold(0_u64, |total, polygon| {
            let count = densified_count(&Geometry::Polygon(polygon.clone()), max_segment_length)?;
            total
                .checked_add(count)
                .ok_or(ExtendedAlgorithmError::IndexOverflow)
        }),
        Geometry::GeometryCollection(collection) => {
            collection.0.iter().try_fold(0_u64, |total, child| {
                total
                    .checked_add(densified_count(child, max_segment_length)?)
                    .ok_or(ExtendedAlgorithmError::IndexOverflow)
            })
        }
        Geometry::Rect(_) | Geometry::Triangle(_) => {
            Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "densify",
                actual: geometry_type(geometry),
            })
        }
    }
}

/// Inserts vertices using planar Euclidean distance. The output bound is
/// computed before allocation and checked again after the operation.
pub fn densify(
    geometry: &Geometry<f64>,
    max_segment_length: f64,
    max_output_coordinates: u64,
) -> Result<Geometry<f64>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    if !max_segment_length.is_finite() || max_segment_length <= 0.0 {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "max_segment_length",
            reason: "deve essere finita e maggiore di zero",
        });
    }
    let estimated = densified_count(geometry, max_segment_length)?;
    if estimated > max_output_coordinates {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual: estimated,
            limit: max_output_coordinates,
        });
    }
    let output = match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => geometry.clone(),
        Geometry::LineString(line) => {
            Geometry::LineString(Euclidean.densify(line, max_segment_length))
        }
        Geometry::Polygon(polygon) => {
            Geometry::Polygon(Euclidean.densify(polygon, max_segment_length))
        }
        Geometry::MultiLineString(lines) => {
            Geometry::MultiLineString(Euclidean.densify(lines, max_segment_length))
        }
        Geometry::MultiPolygon(polygons) => {
            Geometry::MultiPolygon(Euclidean.densify(polygons, max_segment_length))
        }
        Geometry::GeometryCollection(collection) => Geometry::GeometryCollection(
            collection
                .0
                .iter()
                .map(|child| densify(child, max_segment_length, max_output_coordinates))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ),
        Geometry::Line(_) | Geometry::Rect(_) | Geometry::Triangle(_) => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "densify",
                actual: geometry_type(geometry),
            });
        }
    };
    let actual = coordinate_count(&output)?;
    if actual > max_output_coordinates {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual,
            limit: max_output_coordinates,
        });
    }
    validate_output(output)
}

/// Rounds coordinates to an explicit grid. Collapses that make the geometry
/// invalid are rejected instead of being silently repaired.
pub fn snap_to_grid(
    geometry: &Geometry<f64>,
    grid_size: f64,
) -> Result<Geometry<f64>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    if !grid_size.is_finite() || grid_size <= 0.0 {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "grid_size",
            reason: "deve essere finita e maggiore di zero",
        });
    }
    let output: Geometry<f64> = geometry.try_map_coords(|coordinate| {
        let x = (coordinate.x / grid_size).round() * grid_size;
        let y = (coordinate.y / grid_size).round() * grid_size;
        if x.is_finite() && y.is_finite() {
            Ok(Coord {
                x: if x == 0.0 { 0.0 } else { x },
                y: if y == 0.0 { 0.0 } else { y },
            })
        } else {
            Err(ExtendedAlgorithmError::InvalidOutput(
                "overflow durante lo snap".to_owned(),
            ))
        }
    })?;
    validate_output(output)
}

pub fn delaunay(
    geometry: &Geometry<f64>,
    max_input_coordinates: u64,
    max_triangles: u64,
) -> Result<Vec<Polygon<f64>>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    let coordinates = coordinate_count(geometry)?;
    if coordinates > max_input_coordinates {
        return Err(ExtendedAlgorithmError::CoordinateLimit {
            actual: coordinates,
            limit: max_input_coordinates,
        });
    }
    let triangles = geometry
        .unconstrained_triangulation()
        .map_err(|error| ExtendedAlgorithmError::Triangulation(error.to_string()))?;
    let actual =
        u64::try_from(triangles.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if actual > max_triangles {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual,
            limit: max_triangles,
        });
    }
    triangles
        .into_iter()
        .map(|triangle| {
            let polygon = triangle.to_polygon();
            validate_output(Geometry::Polygon(polygon.clone()))?;
            Ok(polygon)
        })
        .collect()
}

fn validate_ratio(value: f64, name: &'static str) -> Result<(), ExtendedAlgorithmError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name,
            reason: "deve essere finito e compreso tra zero e uno",
        });
    }
    Ok(())
}

pub fn line_interpolate_point(
    line: &LineString<f64>,
    ratio: f64,
) -> Result<Option<Point<f64>>, ExtendedAlgorithmError> {
    validate_ratio(ratio, "ratio")?;
    validate_input(&Geometry::LineString(line.clone()))?;
    Ok(Euclidean.point_at_ratio_from_start(line, ratio))
}

pub fn line_substring(
    line: &LineString<f64>,
    start_ratio: f64,
    end_ratio: f64,
) -> Result<Option<Geometry<f64>>, ExtendedAlgorithmError> {
    validate_ratio(start_ratio, "start_ratio")?;
    validate_ratio(end_ratio, "end_ratio")?;
    if start_ratio > end_ratio {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "start_ratio/end_ratio",
            reason: "start_ratio non puo superare end_ratio",
        });
    }
    validate_input(&Geometry::LineString(line.clone()))?;
    let Some(start) = Euclidean.point_at_ratio_from_start(line, start_ratio) else {
        return Ok(None);
    };
    if start_ratio == end_ratio {
        return Ok(Some(Geometry::Point(start)));
    }
    let Some(end) = Euclidean.point_at_ratio_from_start(line, end_ratio) else {
        return Ok(None);
    };
    let total = geo::algorithm::line_measures::Length::length(&Euclidean, line);
    if total == 0.0 {
        return Ok(Some(Geometry::Point(start)));
    }
    let start_distance = start_ratio * total;
    let end_distance = end_ratio * total;
    let mut coordinates = vec![start.0];
    let mut traversed = 0.0;
    for segment in line.lines() {
        let segment_length =
            (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y);
        traversed += segment_length;
        if traversed > start_distance
            && traversed < end_distance
            && coordinates.last() != Some(&segment.end)
        {
            coordinates.push(segment.end);
        }
    }
    if coordinates.last() != Some(&end.0) {
        coordinates.push(end.0);
    }
    validate_output(Geometry::LineString(LineString::new(coordinates))).map(Some)
}

pub fn frechet_distance(
    left: &LineString<f64>,
    right: &LineString<f64>,
    max_coordinate_pairs: u64,
) -> Result<Option<f64>, ExtendedAlgorithmError> {
    validate_input(&Geometry::LineString(left.clone()))?;
    validate_input(&Geometry::LineString(right.clone()))?;
    let left_count =
        u64::try_from(left.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    let right_count =
        u64::try_from(right.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if left_count == 0 || right_count == 0 {
        return Ok(None);
    }
    let actual = left_count
        .checked_mul(right_count)
        .ok_or(ExtendedAlgorithmError::WorkLimit {
            actual: u64::MAX,
            limit: max_coordinate_pairs,
        })?;
    if actual > max_coordinate_pairs {
        return Err(ExtendedAlgorithmError::WorkLimit {
            actual,
            limit: max_coordinate_pairs,
        });
    }
    Ok(Some(Euclidean.frechet_distance(left, right)))
}

fn validate_geographic_geometry(geometry: &Geometry<f64>) -> Result<(), ExtendedAlgorithmError> {
    validate_input(geometry)?;
    if geometry.coords_iter().any(|coordinate| {
        !(-180.0..=180.0).contains(&coordinate.x) || !(-90.0..=90.0).contains(&coordinate.y)
    }) {
        return Err(ExtendedAlgorithmError::InvalidGeographicCoordinate);
    }
    Ok(())
}

pub fn geodesic_bearing_degrees(
    origin: Point<f64>,
    destination: Point<f64>,
) -> Result<f64, ExtendedAlgorithmError> {
    validate_geographic_geometry(&Geometry::MultiPoint(vec![origin, destination].into()))?;
    Ok(Geodesic.bearing(origin, destination))
}

pub fn geodesic_area_m2(geometry: &Geometry<f64>) -> Result<f64, ExtendedAlgorithmError> {
    validate_geographic_geometry(geometry)?;
    let area = match geometry {
        Geometry::Polygon(polygon) => polygon.orient(Direction::Default).geodesic_area_unsigned(),
        Geometry::MultiPolygon(MultiPolygon(polygons)) => polygons
            .iter()
            .map(|polygon| polygon.orient(Direction::Default).geodesic_area_unsigned())
            .sum(),
        _ => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "geodesic_area",
                actual: geometry_type(geometry),
            })
        }
    };
    if !area.is_finite() {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "area NaN o infinita".to_owned(),
        ));
    }
    Ok(area)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeometryDiagnostics {
    pub geometry_type: &'static str,
    pub coordinate_count: u64,
    pub is_empty: bool,
    pub is_finite: bool,
    pub is_valid: bool,
    pub validity_reason: Option<String>,
    pub bounds: Option<[f64; 4]>,
}

/// Diagnostics intentionally accept invalid topology; they never run an
/// algorithm on non-finite coordinates.
pub fn geometry_diagnostics(
    geometry: &Geometry<f64>,
) -> Result<GeometryDiagnostics, ExtendedAlgorithmError> {
    use geo::BoundingRect;

    let is_finite = geometry
        .coords_iter()
        .all(|coordinate| coordinate.x.is_finite() && coordinate.y.is_finite());
    let validation = if is_finite {
        geometry
            .check_validation()
            .map_err(|error| error.to_string())
    } else {
        Err("coordinate NaN o infinite".to_owned())
    };
    let bounds = if is_finite {
        geometry
            .bounding_rect()
            .map(|rect| [rect.min().x, rect.min().y, rect.max().x, rect.max().y])
    } else {
        None
    };
    let coordinate_count = coordinate_count(geometry)?;
    Ok(GeometryDiagnostics {
        geometry_type: geometry_type(geometry),
        coordinate_count,
        is_empty: coordinate_count == 0,
        is_finite,
        is_valid: validation.is_ok(),
        validity_reason: validation.err(),
        bounds,
    })
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct EndpointKey {
    x: u64,
    y: u64,
}

impl EndpointKey {
    fn new(coordinate: Coord<f64>) -> Self {
        fn canonical_bits(value: f64) -> u64 {
            if value == 0.0 {
                0.0_f64.to_bits()
            } else {
                value.to_bits()
            }
        }
        Self {
            x: canonical_bits(coordinate.x),
            y: canonical_bits(coordinate.y),
        }
    }
}

#[derive(Clone, Copy)]
struct MergeEdge<'a> {
    line: &'a LineString<f64>,
    start: EndpointKey,
    end: EndpointKey,
}

fn collect_lines<'a>(
    geometry: &'a Geometry<f64>,
    operation: &'static str,
    output: &mut Vec<&'a LineString<f64>>,
) -> Result<(), ExtendedAlgorithmError> {
    match geometry {
        Geometry::LineString(line) => output.push(line),
        Geometry::MultiLineString(lines) => output.extend(lines.0.iter()),
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                collect_lines(child, operation, output)?;
            }
        }
        _ => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation,
                actual: geometry_type(geometry),
            });
        }
    }
    Ok(())
}

fn walk_merged_path(
    edges: &[MergeEdge<'_>],
    adjacency: &HashMap<EndpointKey, Vec<usize>>,
    used: &mut [bool],
    start_node: EndpointKey,
    first_edge: usize,
) -> LineString<f64> {
    let mut output = Vec::new();
    let mut current_node = start_node;
    let mut edge_index = first_edge;
    loop {
        if used[edge_index] {
            break;
        }
        let edge = &edges[edge_index];
        let forward = edge.start == current_node;
        if forward {
            for &coordinate in &edge.line.0 {
                if output.last() != Some(&coordinate) {
                    output.push(coordinate);
                }
            }
        } else {
            for &coordinate in edge.line.0.iter().rev() {
                if output.last() != Some(&coordinate) {
                    output.push(coordinate);
                }
            }
        }
        used[edge_index] = true;
        current_node = if forward { edge.end } else { edge.start };
        let incident = &adjacency[&current_node];
        if incident.len() != 2 {
            break;
        }
        let Some(next) = incident.iter().copied().find(|candidate| !used[*candidate]) else {
            break;
        };
        edge_index = next;
    }
    LineString::new(output)
}

/// Merges maximal line paths. A node with degree other than two is always a
/// barrier, matching established line-merge topology semantics.
pub fn line_merge(
    geometry: &Geometry<f64>,
    max_input_coordinates: u64,
    max_output_lines: u64,
) -> Result<Vec<LineString<f64>>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    let input_coordinates = coordinate_count(geometry)?;
    if input_coordinates > max_input_coordinates {
        return Err(ExtendedAlgorithmError::CoordinateLimit {
            actual: input_coordinates,
            limit: max_input_coordinates,
        });
    }
    let mut lines = Vec::new();
    collect_lines(geometry, "line_merge", &mut lines)?;
    lines.retain(|line| !line.0.is_empty());
    let mut edges = Vec::with_capacity(lines.len());
    let mut adjacency: HashMap<EndpointKey, Vec<usize>> = HashMap::new();
    for line in lines {
        let start = EndpointKey::new(line.0[0]);
        let end = EndpointKey::new(*line.0.last().expect("non-empty line"));
        let index = edges.len();
        edges.push(MergeEdge { line, start, end });
        adjacency.entry(start).or_default().push(index);
        if end != start {
            adjacency.entry(end).or_default().push(index);
        }
    }

    let mut used = vec![false; edges.len()];
    let mut output = Vec::new();
    for index in 0..edges.len() {
        if used[index] {
            continue;
        }
        let edge = &edges[index];
        if edge.start == edge.end {
            used[index] = true;
            output.push(edge.line.clone());
            continue;
        }
        let start_degree = adjacency[&edge.start].len();
        let end_degree = adjacency[&edge.end].len();
        if start_degree != 2 || end_degree != 2 {
            let start = if start_degree != 2 {
                edge.start
            } else {
                edge.end
            };
            output.push(walk_merged_path(
                &edges, &adjacency, &mut used, start, index,
            ));
        }
    }
    for index in 0..edges.len() {
        if used[index] {
            continue;
        }
        let edge = &edges[index];
        let start = edge.start.min(edge.end);
        output.push(walk_merged_path(
            &edges, &adjacency, &mut used, start, index,
        ));
    }
    let actual = u64::try_from(output.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if actual > max_output_lines {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual,
            limit: max_output_lines,
        });
    }
    for line in &output {
        validate_output(Geometry::LineString(line.clone()))?;
    }
    Ok(output)
}

fn splitter_primitives(
    geometry: &Geometry<f64>,
    points: &mut Vec<Point<f64>>,
    lines: &mut Vec<Line<f64>>,
) -> Result<(), ExtendedAlgorithmError> {
    match geometry {
        Geometry::Point(point) => points.push(*point),
        Geometry::MultiPoint(multi) => points.extend(multi.0.iter().copied()),
        Geometry::LineString(line) => lines.extend(line.lines()),
        Geometry::MultiLineString(multi) => {
            for line in &multi.0 {
                lines.extend(line.lines());
            }
        }
        Geometry::Polygon(polygon) => {
            lines.extend(polygon.exterior().lines());
            for ring in polygon.interiors() {
                lines.extend(ring.lines());
            }
        }
        Geometry::MultiPolygon(multi) => {
            for polygon in &multi.0 {
                splitter_primitives(&Geometry::Polygon(polygon.clone()), points, lines)?;
            }
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                splitter_primitives(child, points, lines)?;
            }
        }
        _ => {
            return Err(ExtendedAlgorithmError::UnsupportedGeometry {
                operation: "split",
                actual: geometry_type(geometry),
            });
        }
    }
    Ok(())
}

fn ratio_on_source_segment(
    segment: Line<f64>,
    coordinate: Coord<f64>,
    distance_before: f64,
    total_length: f64,
) -> f64 {
    let segment_length = (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y);
    if segment_length == 0.0 || total_length == 0.0 {
        return 0.0;
    }
    let local = (coordinate.x - segment.start.x).hypot(coordinate.y - segment.start.y);
    (distance_before + local.min(segment_length)) / total_length
}

#[derive(Clone, Copy)]
struct IndexedSegment {
    line: Line<f64>,
    distance_before: f64,
    length: f64,
    envelope: AABB<[f64; 2]>,
}

impl IndexedSegment {
    fn new(line: Line<f64>, distance_before: f64) -> Self {
        let length = (line.end.x - line.start.x).hypot(line.end.y - line.start.y);
        Self {
            line,
            distance_before,
            length,
            envelope: AABB::from_corners(
                [line.start.x.min(line.end.x), line.start.y.min(line.end.y)],
                [line.start.x.max(line.end.x), line.start.y.max(line.end.y)],
            ),
        }
    }
}

impl RTreeObject for IndexedSegment {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

fn point_ratio_on_segment(
    segment: &IndexedSegment,
    point: Point<f64>,
    tolerance: f64,
    total_length: f64,
) -> Option<f64> {
    let dx = segment.line.end.x - segment.line.start.x;
    let dy = segment.line.end.y - segment.line.start.y;
    let length_squared = dx.mul_add(dx, dy * dy);
    if length_squared == 0.0 {
        return None;
    }
    let parameter = (((point.x() - segment.line.start.x) * dx
        + (point.y() - segment.line.start.y) * dy)
        / length_squared)
        .clamp(0.0, 1.0);
    let projected = Coord {
        x: segment.line.start.x + parameter * dx,
        y: segment.line.start.y + parameter * dy,
    };
    let distance = (point.x() - projected.x).hypot(point.y() - projected.y);
    let ratio = (segment.distance_before + parameter * segment.length) / total_length;
    // Projection arithmetic can move an exactly collinear decimal point by
    // a few ULPs (for example x=14 on a 0..100 segment). Zero user
    // tolerance must still mean topological coincidence, not bit equality.
    let coordinate_scale = segment
        .line
        .start
        .x
        .abs()
        .max(segment.line.start.y.abs())
        .max(segment.line.end.x.abs())
        .max(segment.line.end.y.abs())
        .max(point.x().abs())
        .max(point.y().abs())
        .max(1.0);
    let numeric_slack = coordinate_scale * f64::EPSILON * 16.0;
    if distance <= tolerance + numeric_slack {
        Some(ratio)
    } else {
        None
    }
}

fn expanded_point_envelope(point: Point<f64>, tolerance: f64) -> AABB<[f64; 2]> {
    let lower = |value: f64| {
        let result = value - tolerance;
        if result.is_finite() {
            result
        } else {
            -f64::MAX
        }
    };
    let upper = |value: f64| {
        let result = value + tolerance;
        if result.is_finite() {
            result
        } else {
            f64::MAX
        }
    };
    AABB::from_corners(
        [lower(point.x()), lower(point.y())],
        [upper(point.x()), upper(point.y())],
    )
}

/// Splits a LineString using points, linework or polygon boundaries. Work is
/// bounded before the quadratic segment-intersection loop.
pub fn split_line(
    source: &LineString<f64>,
    splitter: &Geometry<f64>,
    tolerance: f64,
    max_input_coordinates: u64,
    max_intersection_tests: u64,
    max_output_parts: u64,
    max_output_coordinates: u64,
) -> Result<Vec<LineString<f64>>, ExtendedAlgorithmError> {
    validate_input(&Geometry::LineString(source.clone()))?;
    validate_input(splitter)?;
    let input_coordinates = u64::try_from(source.0.len())
        .map_err(|_| ExtendedAlgorithmError::IndexOverflow)?
        .checked_add(coordinate_count(splitter)?)
        .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
    if input_coordinates > max_input_coordinates {
        return Err(ExtendedAlgorithmError::CoordinateLimit {
            actual: input_coordinates,
            limit: max_input_coordinates,
        });
    }
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(ExtendedAlgorithmError::InvalidParameter {
            name: "tolerance",
            reason: "deve essere finita e non negativa",
        });
    }
    let total_length = Euclidean.length(source);
    if !total_length.is_finite() {
        return Err(ExtendedAlgorithmError::InvalidInput(
            "lunghezza non finita per overflow numerico".to_owned(),
        ));
    }
    if total_length == 0.0 {
        return Ok(vec![source.clone()]);
    }
    let mut distance_before = 0.0;
    let source_segments: Vec<_> = source
        .lines()
        .map(|line| {
            let indexed = IndexedSegment::new(line, distance_before);
            distance_before += indexed.length;
            indexed
        })
        .collect();
    let mut points = Vec::new();
    let mut splitter_segments = Vec::new();
    splitter_primitives(splitter, &mut points, &mut splitter_segments)?;
    let source_count =
        u64::try_from(source_segments.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    let splitter_primitive_count = splitter_segments
        .len()
        .checked_add(points.len())
        .ok_or(ExtendedAlgorithmError::IndexOverflow)?;
    let splitter_work = u64::try_from(splitter_primitive_count)
        .map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    let work =
        source_count
            .checked_mul(splitter_work)
            .ok_or(ExtendedAlgorithmError::WorkLimit {
                actual: u64::MAX,
                limit: max_intersection_tests,
            })?;
    if work > max_intersection_tests {
        return Err(ExtendedAlgorithmError::WorkLimit {
            actual: work,
            limit: max_intersection_tests,
        });
    }

    let mut ratios = Vec::new();
    let source_tree = RTree::bulk_load(source_segments.clone());
    let coordinate_scale = source
        .coords_iter()
        .chain(splitter.coords_iter())
        .fold(1.0_f64, |scale, coordinate| {
            scale.max(coordinate.x.abs()).max(coordinate.y.abs())
        });
    let query_tolerance = (tolerance + coordinate_scale * f64::EPSILON * 16.0).min(f64::MAX);
    for point in points {
        let envelope = expanded_point_envelope(point, query_tolerance);
        ratios.extend(
            source_tree
                .locate_in_envelope_intersecting(&envelope)
                .filter_map(|segment| {
                    point_ratio_on_segment(segment, point, tolerance, total_length)
                }),
        );
    }
    let splitter_tree = RTree::bulk_load(
        splitter_segments
            .iter()
            .copied()
            .map(|line| IndexedSegment::new(line, 0.0))
            .collect(),
    );
    for source_segment in &source_segments {
        for splitter_segment in
            splitter_tree.locate_in_envelope_intersecting(&source_segment.envelope)
        {
            match line_intersection(source_segment.line, splitter_segment.line) {
                Some(LineIntersection::SinglePoint { intersection, .. }) => {
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection,
                        source_segment.distance_before,
                        total_length,
                    ))
                }
                Some(LineIntersection::Collinear { intersection }) => {
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection.start,
                        source_segment.distance_before,
                        total_length,
                    ));
                    ratios.push(ratio_on_source_segment(
                        source_segment.line,
                        intersection.end,
                        source_segment.distance_before,
                        total_length,
                    ));
                }
                None => {}
            }
        }
    }
    ratios.retain(|ratio| ratio.is_finite() && *ratio > 0.0 && *ratio < 1.0);
    ratios.sort_by(f64::total_cmp);
    // The tolerance requested by the caller may merge nearby cuts by design.
    // With an exact (zero) tolerance, only absorb floating duplicates around a
    // shared vertex; a fixed 1e-12 floor used to erase legitimate tiny pieces.
    let ratio_tolerance = (tolerance / total_length).max(f64::EPSILON * 8.0);
    ratios.dedup_by(|left, right| (*left - *right).abs() <= ratio_tolerance);
    let part_count =
        u64::try_from(ratios.len() + 1).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if part_count > max_output_parts {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual: part_count,
            limit: max_output_parts,
        });
    }
    let mut boundaries = Vec::with_capacity(ratios.len() + 2);
    boundaries.push(0.0);
    boundaries.extend(ratios);
    boundaries.push(1.0);
    let mut output = Vec::with_capacity(boundaries.len() - 1);
    for window in boundaries.windows(2) {
        let Some(piece) = line_substring(source, window[0], window[1])? else {
            continue;
        };
        let Geometry::LineString(piece) = piece else {
            continue;
        };
        output.push(piece);
    }
    let output_length: f64 = output.iter().map(|line| Euclidean.length(line)).sum();
    let allowed_error = total_length.abs().max(1.0) * 1e-10;
    if (output_length - total_length).abs() > allowed_error {
        return Err(ExtendedAlgorithmError::InvalidOutput(
            "lo split non conserva la lunghezza".to_owned(),
        ));
    }
    let output_coordinates = output.iter().try_fold(0_u64, |total, line| {
        let count =
            u64::try_from(line.0.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
        total
            .checked_add(count)
            .ok_or(ExtendedAlgorithmError::IndexOverflow)
    })?;
    if output_coordinates > max_output_coordinates {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual: output_coordinates,
            limit: max_output_coordinates,
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, Area};
    use proptest::prelude::*;

    #[test]
    fn densify_preflights_output_and_preserves_shape() {
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0)]);
        let output = densify(&line, 3.0, 10).unwrap();
        assert_eq!(output.coords_count(), 5);
        assert!(matches!(
            densify(&line, 0.001, 100),
            Err(ExtendedAlgorithmError::OutputLimit { .. })
        ));
    }

    #[test]
    fn snap_to_grid_rejects_collapsed_invalid_polygons() {
        let point = Geometry::Point(Point::new(1.24, -0.24));
        assert_eq!(
            snap_to_grid(&point, 0.5).unwrap(),
            Geometry::Point(Point::new(1.0, 0.0))
        );
        let tiny = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 0.1, y: 0.0),
            (x: 0.1, y: 0.1), (x: 0.0, y: 0.1),
            (x: 0.0, y: 0.0),
        ]);
        assert!(snap_to_grid(&tiny, 1.0).is_err());
    }

    #[test]
    fn delaunay_is_bounded_and_covers_square() {
        let input = Geometry::MultiPoint(
            vec![
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(1.0, 1.0),
                Point::new(0.0, 1.0),
            ]
            .into(),
        );
        let triangles = delaunay(&input, 10, 10).unwrap();
        assert_eq!(triangles.len(), 2);
        assert!((triangles.iter().map(Area::unsigned_area).sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(matches!(
            delaunay(&input, 3, 10),
            Err(ExtendedAlgorithmError::CoordinateLimit { .. })
        ));
    }

    #[test]
    fn linear_reference_is_deterministic() {
        let line = line_string![(x: 0.0, y: 0.0), (x: 0.0, y: 10.0), (x: 10.0, y: 10.0)];
        assert_eq!(
            line_interpolate_point(&line, 0.25).unwrap(),
            Some(Point::new(0.0, 5.0))
        );
        assert_eq!(
            line_substring(&line, 0.25, 0.75).unwrap(),
            Some(Geometry::LineString(line_string![
                (x: 0.0, y: 5.0), (x: 0.0, y: 10.0), (x: 5.0, y: 10.0)
            ]))
        );
        assert_eq!(
            line_substring(&line, 0.5, 0.5).unwrap(),
            Some(Geometry::Point(Point::new(0.0, 10.0)))
        );
    }

    #[test]
    fn frechet_bearing_and_geodesic_area_are_bounded() {
        let left = line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)];
        let right = line_string![(x: 0.0, y: 1.0), (x: 2.0, y: 1.0)];
        assert_eq!(frechet_distance(&left, &right, 4).unwrap(), Some(1.0));
        assert!(matches!(
            frechet_distance(&left, &right, 3),
            Err(ExtendedAlgorithmError::WorkLimit { .. })
        ));
        assert_eq!(
            geodesic_bearing_degrees(Point::new(0.0, 0.0), Point::new(0.0, 2.0)).unwrap(),
            0.0
        );
        let square = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0),
            (x: 1.0, y: 1.0), (x: 0.0, y: 1.0),
            (x: 0.0, y: 0.0),
        ]);
        let area = geodesic_area_m2(&square).unwrap();
        assert!(area > 12_000_000_000.0 && area < 13_000_000_000.0);
        let Geometry::Polygon(mut reversed) = square else {
            unreachable!()
        };
        reversed.exterior_mut(|ring| ring.0.reverse());
        assert!((geodesic_area_m2(&Geometry::Polygon(reversed)).unwrap() - area).abs() < 1e-6);
    }

    #[test]
    fn diagnostics_report_invalid_data_without_running_topology() {
        let valid = Geometry::Point(Point::new(2.0, 3.0));
        let report = geometry_diagnostics(&valid).unwrap();
        assert!(report.is_valid);
        assert_eq!(report.bounds, Some([2.0, 3.0, 2.0, 3.0]));

        let invalid = Geometry::Point(Point::new(f64::NAN, 3.0));
        let report = geometry_diagnostics(&invalid).unwrap();
        assert!(!report.is_finite);
        assert!(!report.is_valid);
        assert!(report.bounds.is_none());
    }

    #[test]
    fn line_merge_stops_at_branches_and_closes_cycles() {
        let chain = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 2.0, y: 0.0), (x: 1.0, y: 0.0)],
        ]));
        assert_eq!(line_merge(&chain, 100, 10).unwrap().len(), 1);

        let branch = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 2.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 1.0, y: 1.0)],
        ]));
        assert_eq!(line_merge(&branch, 100, 10).unwrap().len(), 3);
        assert!(matches!(
            line_merge(&branch, 100, 2),
            Err(ExtendedAlgorithmError::OutputLimit { .. })
        ));

        let ring = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 1.0, y: 1.0)],
            line_string![(x: 1.0, y: 1.0), (x: 0.0, y: 1.0)],
            line_string![(x: 0.0, y: 1.0), (x: 0.0, y: 0.0)],
        ]));
        let merged = line_merge(&ring, 100, 10).unwrap();
        assert_eq!(merged.len(), 1);
        assert!(merged[0].is_closed());
    }

    #[test]
    fn split_line_handles_points_crossings_and_overlap_endpoints() {
        let source = line_string![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0)];
        let points = Geometry::MultiPoint(vec![Point::new(2.0, 0.0), Point::new(7.0, 0.0)].into());
        let pieces = split_line(&source, &points, 0.0, 100, 100, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);
        assert_eq!(
            pieces
                .iter()
                .map(|line| Euclidean.length(line))
                .sum::<f64>(),
            10.0
        );
        assert!(matches!(
            split_line(&source, &points, 0.0, 100, 100, 2, 100),
            Err(ExtendedAlgorithmError::OutputLimit { .. })
        ));

        let cutters = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 5.0, y: -1.0), (x: 5.0, y: 1.0)],
            line_string![(x: 8.0, y: 0.0), (x: 12.0, y: 0.0)],
        ]));
        let pieces = split_line(&source, &cutters, 0.0, 100, 100, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].0.last().unwrap().x, 5.0);
        assert_eq!(pieces[1].0.last().unwrap().x, 8.0);
        assert!(matches!(
            split_line(&source, &cutters, 0.0, 100, 1, 10, 100),
            Err(ExtendedAlgorithmError::WorkLimit { .. })
        ));
    }

    #[test]
    fn split_line_cuts_every_occurrence_of_a_self_intersection_point() {
        let source = line_string![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0)
        ];
        let splitter = Geometry::Point(Point::new(1.0, 1.0));
        let pieces = split_line(&source, &splitter, 0.0, 100, 100, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);
        let is_crossing = |coord: &Coord<f64>| {
            (coord.x - 1.0).abs() <= f64::EPSILON * 8.0
                && (coord.y - 1.0).abs() <= f64::EPSILON * 8.0
        };
        assert!(pieces[0].0.last().is_some_and(is_crossing));
        assert!(pieces[1].0.first().is_some_and(is_crossing));
        assert!(pieces[1].0.last().is_some_and(is_crossing));
    }

    #[test]
    fn split_line_preserves_distinct_sub_picometer_cuts_and_all_bounds() {
        let source = line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)];
        let splitter =
            Geometry::MultiPoint(vec![Point::new(0.5, 0.0), Point::new(0.5 + 1e-13, 0.0)].into());
        let pieces = split_line(&source, &splitter, 0.0, 100, 100, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);
        assert!(Euclidean.length(&pieces[1]) > 0.0);
        assert!(matches!(
            split_line(&source, &splitter, 0.0, 3, 100, 10, 100),
            Err(ExtendedAlgorithmError::CoordinateLimit { .. })
        ));
        assert!(matches!(
            split_line(&source, &splitter, 0.0, 100, 100, 10, 5),
            Err(ExtendedAlgorithmError::OutputLimit { .. })
        ));

        let decimal_projection_source = line_string![(x: 0.0, y: 0.0), (x: 100.0, y: 0.0)];
        let decimal_point = Geometry::Point(Point::new(14.0, 0.0));
        assert_eq!(
            split_line(
                &decimal_projection_source,
                &decimal_point,
                0.0,
                100,
                100,
                10,
                100,
            )
            .unwrap()
            .len(),
            2
        );
        let off_line = Geometry::Point(Point::new(14.0, 1e-9));
        assert_eq!(
            split_line(
                &decimal_projection_source,
                &off_line,
                0.0,
                100,
                100,
                10,
                100,
            )
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            split_line(
                &decimal_projection_source,
                &off_line,
                1e-8,
                100,
                100,
                10,
                100,
            )
            .unwrap()
            .len(),
            2
        );
    }

    #[test]
    fn adversarial_geometry_families_and_numeric_overflow_fail_closed() {
        let polygon = polygon![
            exterior: [
                (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
                (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
                (x: 0.0, y: 0.0)
            ],
            interiors: [[
                (x: 1.0, y: 1.0), (x: 2.0, y: 1.0),
                (x: 2.0, y: 2.0), (x: 1.0, y: 2.0),
                (x: 1.0, y: 1.0)
            ]]
        ];
        let multi_polygon = Geometry::MultiPolygon(geo::MultiPolygon(vec![polygon.clone()]));
        let multi_line = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)],
            line_string![(x: 0.0, y: 1.0), (x: 2.0, y: 1.0)],
        ]));
        let collection = Geometry::GeometryCollection(
            vec![Geometry::Polygon(polygon.clone()), multi_line.clone()].into(),
        );
        for geometry in [
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::MultiPoint(vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)].into()),
            Geometry::Polygon(polygon.clone()),
            multi_polygon.clone(),
            multi_line.clone(),
            collection,
        ] {
            assert!(densify(&geometry, 0.75, 1_000).is_ok());
        }
        assert!(matches!(
            densify(
                &Geometry::LineString(line_string![
                    (x: -f64::MAX, y: 0.0), (x: f64::MAX, y: 0.0)
                ]),
                1.0,
                u64::MAX,
            ),
            Err(ExtendedAlgorithmError::IndexOverflow)
        ));
        assert!(snap_to_grid(&Geometry::Point(Point::new(f64::MAX, 0.0)), 0.1).is_err());

        let geodesic_multi = Geometry::MultiPolygon(geo::MultiPolygon(vec![polygon]));
        assert!(geodesic_area_m2(&geodesic_multi).unwrap() > 0.0);
        assert!(geodesic_area_m2(&Geometry::Point(Point::new(0.0, 0.0))).is_err());
        assert!(
            geometry_diagnostics(&Geometry::GeometryCollection(
                Vec::<Geometry<f64>>::new().into()
            ))
            .unwrap()
            .is_empty
        );

        assert!(line_merge(
            &Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into()),
            10,
            10
        )
        .unwrap()
        .is_empty());
        assert!(line_merge(&multi_line, 100, 10).is_ok());
        assert!(line_merge(&multi_polygon, 100, 10).is_err());
    }

    #[test]
    fn split_supports_polygon_boundaries_collections_zero_length_and_huge_values() {
        let source = line_string![(x: -1.0, y: 1.0), (x: 5.0, y: 1.0)];
        let polygon = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
            (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
            (x: 0.0, y: 0.0)
        ]);
        let pieces = split_line(&source, &polygon, 0.0, 100, 1_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 3);

        let multi_polygon =
            Geometry::MultiPolygon(geo::MultiPolygon(vec![match polygon.clone() {
                Geometry::Polygon(value) => value,
                _ => unreachable!(),
            }]));
        assert_eq!(
            split_line(&source, &multi_polygon, 0.0, 100, 1_000, 10, 100)
                .unwrap()
                .len(),
            3
        );
        let collection = Geometry::GeometryCollection(
            vec![
                Geometry::Point(Point::new(2.0, 1.0)),
                Geometry::LineString(line_string![(x: 3.0, y: 0.0), (x: 3.0, y: 2.0)]),
            ]
            .into(),
        );
        assert_eq!(
            split_line(&source, &collection, 0.0, 100, 1_000, 10, 100)
                .unwrap()
                .len(),
            3
        );

        let zero = LineString::new(Vec::new());
        assert_eq!(
            split_line(
                &zero,
                &Geometry::Point(Point::new(1.0, 1.0)),
                0.0,
                100,
                100,
                10,
                100,
            )
            .unwrap(),
            vec![zero]
        );
        let enormous = line_string![(x: -f64::MAX, y: 0.0), (x: f64::MAX, y: 0.0)];
        assert!(matches!(
            split_line(
                &enormous,
                &Geometry::Point(Point::new(0.0, 0.0)),
                0.0,
                100,
                100,
                10,
                100,
            ),
            Err(ExtendedAlgorithmError::InvalidInput(_))
        ));
    }

    proptest! {
        #[test]
        fn densify_never_exceeds_requested_segment_length(
            x1 in -100.0_f64..100.0,
            y1 in -100.0_f64..100.0,
            x2 in -100.0_f64..100.0,
            y2 in -100.0_f64..100.0,
            maximum in 0.5_f64..50.0,
        ) {
            prop_assume!(x1 != x2 || y1 != y2);
            let input = Geometry::LineString(LineString::from(vec![(x1, y1), (x2, y2)]));
            let output = densify(&input, maximum, 1_000).unwrap();
            let Geometry::LineString(output) = output else {
                unreachable!()
            };
            for segment in output.lines() {
                let length = (segment.end.x - segment.start.x)
                    .hypot(segment.end.y - segment.start.y);
                prop_assert!(length <= maximum * (1.0 + 1e-12));
            }
        }

        #[test]
        fn point_grid_snap_is_idempotent(
            x in -1_000_000.0_f64..1_000_000.0,
            y in -1_000_000.0_f64..1_000_000.0,
            grid in 0.01_f64..100.0,
        ) {
            let input = Geometry::Point(Point::new(x, y));
            let once = snap_to_grid(&input, grid).unwrap();
            let twice = snap_to_grid(&once, grid).unwrap();
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn line_merge_conserves_randomly_oriented_chain(
            orientations in proptest::collection::vec(any::<bool>(), 1..128),
        ) {
            let lines = orientations
                .iter()
                .enumerate()
                .map(|(index, reverse)| {
                    let start = Coord { x: index as f64, y: 0.0 };
                    let end = Coord { x: index as f64 + 1.0, y: 0.0 };
                    if *reverse {
                        LineString::new(vec![end, start])
                    } else {
                        LineString::new(vec![start, end])
                    }
                })
                .collect();
            let input = Geometry::MultiLineString(geo::MultiLineString(lines));
            let merged = line_merge(&input, 1_000, 2).unwrap();
            prop_assert_eq!(merged.len(), 1);
            prop_assert!((Euclidean.length(&merged[0]) - orientations.len() as f64).abs() < 1e-12);
        }

        #[test]
        fn split_line_conserves_length_for_distinct_integer_cuts(
            cuts in proptest::collection::btree_set(1_u16..999_u16, 0..64),
        ) {
            let source = LineString::from(vec![(0.0, 0.0), (1_000.0, 0.0)]);
            let points = cuts
                .iter()
                .map(|cut| Point::new(f64::from(*cut), 0.0))
                .collect::<Vec<_>>();
            let splitter = Geometry::MultiPoint(points.into());
            let pieces = split_line(&source, &splitter, 0.0, 1_000, 100_000, 100, 1_000).unwrap();
            prop_assert_eq!(pieces.len(), cuts.len() + 1);
            let length: f64 = pieces.iter().map(|piece| Euclidean.length(piece)).sum();
            prop_assert!((length - 1_000.0).abs() < 1e-9);
        }
    }
}
