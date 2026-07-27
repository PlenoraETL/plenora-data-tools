//! Pure geometry kernels shared by future transport adapters.

use geo::algorithm::buffer::{BufferStyle, LineCap};
use geo::algorithm::line_measures::{Distance, Euclidean, Length};
use geo::algorithm::validation::Validation;
use geo::{
    Area, BoundingRect, Buffer, Coord, CoordsIter, Geometry, InteriorPoint, LineString, MapCoords,
    MultiLineString, MultiPoint, Simplify, SimplifyVwPreserve,
};
use std::collections::BTreeMap;
use thiserror::Error;
use wkt::ToWkt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferCapStyle {
    Round,
    Flat,
    Square,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SimplifyPolicy {
    DouglasPeucker,
    PreserveTopology,
}

#[derive(Debug, Error)]
pub enum OperationError {
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    #[error("geometria prodotta non valida: {0}")]
    InvalidOutput(String),
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    #[error("serializzazione WKT fallita: {0}")]
    WktSerialization(String),
    /// Invariante interna violata (R6: errore propagato, mai panic).
    #[error("internal error: {0}")]
    Internal(&'static str),
}

fn ensure_valid(geometry: &Geometry<f64>) -> Result<(), OperationError> {
    geometry
        .check_validation()
        .map_err(|error| OperationError::InvalidInput(error.to_string()))
}

fn validate_output(geometry: Geometry<f64>) -> Result<Geometry<f64>, OperationError> {
    geometry
        .check_validation()
        .map_err(|error| OperationError::InvalidOutput(error.to_string()))?;
    Ok(geometry)
}

/// Planar unsigned area. CRS/unit policy remains the responsibility of the
/// caller; geographic coordinates must be projected before this kernel.
pub fn area(geometry: &Geometry<f64>) -> Result<f64, OperationError> {
    ensure_valid(geometry)?;
    Ok(geometry.unsigned_area())
}

/// Planar geometry length with Shapely-compatible semantics for polygons:
/// polygon length is the sum of exterior and interior ring lengths.
pub fn length(geometry: &Geometry<f64>) -> Result<f64, OperationError> {
    ensure_valid(geometry)?;
    Ok(length_unchecked(geometry))
}

fn length_unchecked(geometry: &Geometry<f64>) -> f64 {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => 0.0,
        Geometry::Line(line) => Euclidean.length(line),
        Geometry::LineString(line) => Euclidean.length(line),
        Geometry::MultiLineString(lines) => Euclidean.length(lines),
        Geometry::Polygon(polygon) => {
            Euclidean.length(polygon.exterior())
                + polygon
                    .interiors()
                    .iter()
                    .map(|ring| Euclidean.length(ring))
                    .sum::<f64>()
        }
        Geometry::MultiPolygon(polygons) => polygons
            .iter()
            .map(|polygon| length_unchecked(&Geometry::Polygon(polygon.clone())))
            .sum(),
        Geometry::GeometryCollection(collection) => collection.iter().map(length_unchecked).sum(),
        Geometry::Rect(rect) => length_unchecked(&Geometry::Polygon(rect.to_polygon())),
        Geometry::Triangle(triangle) => length_unchecked(&Geometry::Polygon(triangle.to_polygon())),
    }
}

/// Manipola currently defines perimeter through GeoSeries.length, therefore
/// this intentionally shares the same semantics as `length`.
pub fn perimeter(geometry: &Geometry<f64>) -> Result<f64, OperationError> {
    length(geometry)
}

pub fn distance(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
) -> Result<Option<f64>, OperationError> {
    ensure_valid(left)?;
    ensure_valid(right)?;
    if left.coords_count() == 0 || right.coords_count() == 0 {
        return Ok(None);
    }
    Ok(Some(Euclidean.distance(left, right)))
}

pub fn bounds(geometry: &Geometry<f64>) -> Result<Option<[f64; 4]>, OperationError> {
    ensure_valid(geometry)?;
    Ok(geometry.bounding_rect().map(|rect| {
        let min = rect.min();
        let max = rect.max();
        [min.x, min.y, max.x, max.y]
    }))
}

pub fn vertex_count(geometry: &Geometry<f64>) -> Result<u64, OperationError> {
    ensure_valid(geometry)?;
    Ok(u64::try_from(geometry.coords_count())
        .map_err(|_| OperationError::Internal("usize always fits in u64 on supported targets"))?)
}

pub fn point_on_surface(geometry: &Geometry<f64>) -> Result<Option<Geometry<f64>>, OperationError> {
    ensure_valid(geometry)?;
    Ok(geometry.interior_point().map(Geometry::Point))
}

pub fn to_wkt(geometry: &Geometry<f64>) -> Result<String, OperationError> {
    ensure_valid(geometry)?;
    Ok(geometry.wkt_string())
}

pub fn buffer(geometry: &Geometry<f64>, distance: f64) -> Result<Geometry<f64>, OperationError> {
    buffer_with_cap(geometry, distance, BufferCapStyle::Round)
}

pub fn buffer_with_cap(
    geometry: &Geometry<f64>,
    distance: f64,
    cap_style: BufferCapStyle,
) -> Result<Geometry<f64>, OperationError> {
    ensure_valid(geometry)?;
    if !distance.is_finite() {
        return Err(OperationError::InvalidParameter {
            name: "distance",
            reason: "deve essere finita",
        });
    }
    let line_cap = match cap_style {
        BufferCapStyle::Round => {
            return validate_output(Geometry::MultiPolygon(geometry.buffer(distance)))
        }
        BufferCapStyle::Flat => LineCap::Butt,
        BufferCapStyle::Square => LineCap::Square,
    };
    let style = BufferStyle::new(distance).line_cap(line_cap);
    validate_output(Geometry::MultiPolygon(geometry.buffer_with_style(style)))
}

pub fn simplify(geometry: &Geometry<f64>, tolerance: f64) -> Result<Geometry<f64>, OperationError> {
    simplify_with_policy(geometry, tolerance, SimplifyPolicy::DouglasPeucker)
}

pub fn simplify_with_policy(
    geometry: &Geometry<f64>,
    tolerance: f64,
    policy: SimplifyPolicy,
) -> Result<Geometry<f64>, OperationError> {
    ensure_valid(geometry)?;
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(OperationError::InvalidParameter {
            name: "tolerance",
            reason: "deve essere finita e non negativa",
        });
    }
    if let Geometry::GeometryCollection(values) = geometry {
        return validate_output(Geometry::GeometryCollection(
            values
                .iter()
                .map(|value| simplify_with_policy(value, tolerance, policy))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ));
    }

    // Squared-distance kernels can overflow/underflow for perfectly finite
    // coordinates close to f64 limits. Work in a uniformly scaled space and
    // restore the original units after simplification.
    let scale = geometry.coords_iter().fold(0.0_f64, |maximum, coordinate| {
        maximum.max(coordinate.x.abs()).max(coordinate.y.abs())
    });
    let normalize = scale > 1e150 || (scale > 0.0 && scale < 1e-150);
    let working = if normalize {
        geometry.map_coords(|coordinate| Coord {
            x: coordinate.x / scale,
            y: coordinate.y / scale,
        })
    } else {
        geometry.clone()
    };
    let working_tolerance = if normalize {
        let value = tolerance / scale;
        if value.is_finite() {
            value
        } else {
            f64::MAX
        }
    } else {
        tolerance
    };

    let simplified = match (&working, policy) {
        (Geometry::LineString(value), SimplifyPolicy::DouglasPeucker) => {
            Geometry::LineString(value.simplify(working_tolerance))
        }
        (Geometry::MultiLineString(value), SimplifyPolicy::DouglasPeucker) => {
            Geometry::MultiLineString(value.simplify(working_tolerance))
        }
        (Geometry::Polygon(value), SimplifyPolicy::DouglasPeucker) => {
            Geometry::Polygon(value.simplify(working_tolerance))
        }
        (Geometry::MultiPolygon(value), SimplifyPolicy::DouglasPeucker) => {
            Geometry::MultiPolygon(value.simplify(working_tolerance))
        }
        (Geometry::LineString(value), SimplifyPolicy::PreserveTopology) => {
            Geometry::LineString(value.simplify_vw_preserve(working_tolerance))
        }
        (Geometry::MultiLineString(value), SimplifyPolicy::PreserveTopology) => {
            Geometry::MultiLineString(value.simplify_vw_preserve(working_tolerance))
        }
        (Geometry::Polygon(value), SimplifyPolicy::PreserveTopology) => {
            Geometry::Polygon(value.simplify_vw_preserve(working_tolerance))
        }
        (Geometry::MultiPolygon(value), SimplifyPolicy::PreserveTopology) => {
            Geometry::MultiPolygon(value.simplify_vw_preserve(working_tolerance))
        }
        (value, _) => value.clone(),
    };
    let simplified = if normalize {
        simplified.map_coords(|coordinate| Coord {
            x: coordinate.x * scale,
            y: coordinate.y * scale,
        })
    } else {
        simplified
    };
    validate_output(simplified)
}

/// Explodes one multipart/collection level while preserving deterministic
/// component order. Simple geometries produce exactly one row.
pub fn explode(geometry: &Geometry<f64>) -> Result<Vec<Geometry<f64>>, OperationError> {
    ensure_valid(geometry)?;
    Ok(match geometry {
        Geometry::MultiPoint(values) => values.iter().copied().map(Geometry::Point).collect(),
        Geometry::MultiLineString(values) => {
            values.iter().cloned().map(Geometry::LineString).collect()
        }
        Geometry::MultiPolygon(values) => values.iter().cloned().map(Geometry::Polygon).collect(),
        Geometry::GeometryCollection(values) => values.iter().cloned().collect(),
        value => vec![value.clone()],
    })
}

/// OGC boundary for the WKB geometry variants used by Plenora-Geo.
pub fn boundary(geometry: &Geometry<f64>) -> Result<Geometry<f64>, OperationError> {
    ensure_valid(geometry)?;
    let output = boundary_unchecked(geometry);
    validate_output(output)
}

fn boundary_unchecked(geometry: &Geometry<f64>) -> Geometry<f64> {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => {
            Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into())
        }
        Geometry::Line(line) => {
            Geometry::MultiPoint(MultiPoint::new(vec![line.start_point(), line.end_point()]))
        }
        Geometry::LineString(line) => line_string_boundary(line),
        Geometry::Polygon(polygon) => Geometry::MultiLineString(MultiLineString::new(
            std::iter::once(polygon.exterior().clone())
                .chain(polygon.interiors().iter().cloned())
                .collect(),
        )),
        Geometry::MultiPolygon(polygons) => Geometry::MultiLineString(MultiLineString::new(
            polygons
                .iter()
                .flat_map(|polygon| {
                    std::iter::once(polygon.exterior().clone())
                        .chain(polygon.interiors().iter().cloned())
                })
                .collect(),
        )),
        Geometry::MultiLineString(lines) => multi_line_string_boundary(lines),
        Geometry::GeometryCollection(values) => Geometry::GeometryCollection(
            values
                .iter()
                .map(boundary_unchecked)
                .collect::<Vec<_>>()
                .into(),
        ),
        Geometry::Rect(rect) => boundary_unchecked(&Geometry::Polygon(rect.to_polygon())),
        Geometry::Triangle(triangle) => {
            boundary_unchecked(&Geometry::Polygon(triangle.to_polygon()))
        }
    }
}

fn multi_line_string_boundary(lines: &MultiLineString<f64>) -> Geometry<f64> {
    let mut endpoints: BTreeMap<(u64, u64), (geo::Point<f64>, bool)> = BTreeMap::new();
    for line in lines {
        if line.0.len() < 2 || line.is_closed() {
            continue;
        }
        for coordinate in [line.0[0], line.0[line.0.len() - 1]] {
            let canonical_x = if coordinate.x == 0.0 {
                0.0
            } else {
                coordinate.x
            };
            let canonical_y = if coordinate.y == 0.0 {
                0.0
            } else {
                coordinate.y
            };
            let key = (canonical_x.to_bits(), canonical_y.to_bits());
            endpoints
                .entry(key)
                .and_modify(|(_, odd)| *odd = !*odd)
                .or_insert((geo::Point::new(canonical_x, canonical_y), true));
        }
    }
    Geometry::MultiPoint(MultiPoint::new(
        endpoints
            .into_values()
            .filter_map(|(point, odd)| odd.then_some(point))
            .collect(),
    ))
}

fn line_string_boundary(line: &LineString<f64>) -> Geometry<f64> {
    if line.0.len() < 2 || line.is_closed() {
        return Geometry::MultiPoint(MultiPoint::new(Vec::new()));
    }
    Geometry::MultiPoint(MultiPoint::new(vec![
        geo::Point::from(line.0[0]),
        geo::Point::from(line.0[line.0.len() - 1]),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{
        line_string, polygon, Contains, GeometryCollection, Line, MultiPolygon, Point, Rect,
        Triangle,
    };
    use proptest::prelude::*;

    fn rectangle() -> Geometry<f64> {
        Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
            (x: 4.0, y: 2.0), (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ])
    }

    #[test]
    fn scalar_measurements_match_known_geometry() {
        let geometry = rectangle();
        assert_eq!(area(&geometry).unwrap(), 8.0);
        assert_eq!(length(&geometry).unwrap(), 12.0);
        assert_eq!(perimeter(&geometry).unwrap(), 12.0);
        assert_eq!(bounds(&geometry).unwrap(), Some([0.0, 0.0, 4.0, 2.0]));
        assert_eq!(vertex_count(&geometry).unwrap(), 5);
        assert!(to_wkt(&geometry).unwrap().starts_with("POLYGON("));
    }

    #[test]
    fn distance_and_interior_point_are_exact_for_simple_fixture() {
        let left = Geometry::Point(Point::new(0.0, 0.0));
        let right = Geometry::Point(Point::new(3.0, 4.0));
        assert_eq!(distance(&left, &right).unwrap(), Some(5.0));
        let point = point_on_surface(&rectangle())
            .unwrap()
            .expect("interior point");
        assert!(rectangle().contains(&point));
    }

    #[test]
    fn buffer_rejects_non_finite_distance_and_produces_valid_polygon() {
        assert!(buffer(&rectangle(), f64::NAN).is_err());
        let result = buffer(&Geometry::Point(Point::new(0.0, 0.0)), 2.0).unwrap();
        assert!(result.unsigned_area() > 12.0);
        assert!(result.unsigned_area() < 13.0);

        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)]);
        let flat = buffer_with_cap(&line, 1.0, BufferCapStyle::Flat).unwrap();
        let square = buffer_with_cap(&line, 1.0, BufferCapStyle::Square).unwrap();
        let round = buffer_with_cap(&line, 1.0, BufferCapStyle::Round).unwrap();
        assert!((flat.unsigned_area() - 4.0).abs() < 1e-9);
        assert!(square.unsigned_area() > flat.unsigned_area());
        assert!(round.unsigned_area() > flat.unsigned_area());
    }

    #[test]
    fn simplify_fails_if_result_would_be_invalid() {
        let line = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.01), (x: 2.0, y: 0.0)
        ]);
        let simplified = simplify(&line, 0.1).unwrap();
        assert_eq!(vertex_count(&simplified).unwrap(), 2);
        assert!(simplify(&line, -1.0).is_err());

        let preserved =
            simplify_with_policy(&rectangle(), 0.5, SimplifyPolicy::PreserveTopology).unwrap();
        assert!(preserved.check_validation().is_ok());
    }

    #[test]
    fn simplify_normalizes_extreme_coordinates_without_panicking() {
        let line = Geometry::LineString(LineString::from(vec![
            (-5.48880284031224e303, -6.971241357778827e182),
            (-5.486124068793689e303, 7.064166183585296e-304),
            (0.0, 0.0),
        ]));
        for tolerance in [0.0, 1.0] {
            for policy in [
                SimplifyPolicy::DouglasPeucker,
                SimplifyPolicy::PreserveTopology,
            ] {
                let output = simplify_with_policy(&line, tolerance, policy).unwrap();
                assert!(output.check_validation().is_ok());
                assert!(output
                    .coords_iter()
                    .all(|coordinate| coordinate.x.is_finite() && coordinate.y.is_finite()));
            }
        }
    }

    #[test]
    fn explode_preserves_component_order() {
        let collection = Geometry::GeometryCollection(GeometryCollection(vec![
            Geometry::Point(Point::new(1.0, 2.0)),
            Geometry::Point(Point::new(3.0, 4.0)),
        ]));
        assert_eq!(
            explode(&collection).unwrap(),
            vec![
                Geometry::Point(Point::new(1.0, 2.0)),
                Geometry::Point(Point::new(3.0, 4.0)),
            ]
        );
    }

    #[test]
    fn boundary_handles_open_and_closed_lines() {
        let open = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)]);
        assert_eq!(vertex_count(&boundary(&open).unwrap()).unwrap(), 2);
        let closed = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 0.0, y: 0.0)
        ]);
        assert_eq!(vertex_count(&boundary(&closed).unwrap()).unwrap(), 0);

        let touching = Geometry::MultiLineString(MultiLineString::new(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 1.0, y: 0.0), (x: 2.0, y: 0.0)],
        ]));
        assert_eq!(vertex_count(&boundary(&touching).unwrap()).unwrap(), 2);
    }

    #[test]
    fn measurements_simplify_explode_and_boundary_cover_all_geometry_families() {
        let polygon = match rectangle() {
            Geometry::Polygon(value) => value,
            _ => unreachable!(),
        };
        let values = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::MultiPoint(MultiPoint::new(vec![Point::new(0.0, 0.0)])),
            Geometry::Line(Line::new((0.0, 0.0), (3.0, 4.0))),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 3.0, y: 4.0)]),
            Geometry::MultiLineString(MultiLineString::new(vec![line_string![
                (x: 0.0, y: 0.0), (x: 3.0, y: 4.0)
            ]])),
            Geometry::Polygon(polygon.clone()),
            Geometry::MultiPolygon(MultiPolygon::new(vec![polygon.clone()])),
            Geometry::GeometryCollection(GeometryCollection(vec![rectangle()])),
            Geometry::Rect(Rect::new((0.0, 0.0), (2.0, 1.0))),
            Geometry::Triangle(Triangle::new(
                (0.0, 0.0).into(),
                (2.0, 0.0).into(),
                (0.0, 1.0).into(),
            )),
        ];
        for value in &values {
            assert!(length(value).unwrap().is_finite());
            assert!(boundary(value).unwrap().check_validation().is_ok());
            assert!(!explode(value).unwrap().is_empty());
            for policy in [
                SimplifyPolicy::DouglasPeucker,
                SimplifyPolicy::PreserveTopology,
            ] {
                assert!(simplify_with_policy(value, 0.01, policy)
                    .unwrap()
                    .check_validation()
                    .is_ok());
            }
        }

        assert_eq!(
            distance(&Geometry::MultiPoint(MultiPoint::new(vec![])), &values[0]).unwrap(),
            None
        );
        assert_eq!(
            bounds(&Geometry::MultiPoint(MultiPoint::new(vec![]))).unwrap(),
            None
        );
        assert_eq!(
            point_on_surface(&Geometry::MultiPoint(MultiPoint::new(vec![]))).unwrap(),
            None
        );
    }

    #[test]
    fn multipart_explode_and_boundary_endpoint_parity_are_deterministic() {
        let polygon = match rectangle() {
            Geometry::Polygon(value) => value,
            _ => unreachable!(),
        };
        assert_eq!(
            explode(&Geometry::MultiPoint(MultiPoint::new(vec![
                Point::new(0.0, 0.0),
                Point::new(1.0, 1.0),
            ])))
            .unwrap()
            .len(),
            2
        );
        assert_eq!(
            explode(&Geometry::MultiLineString(MultiLineString::new(vec![
                line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
                line_string![(x: 2.0, y: 0.0), (x: 3.0, y: 0.0)],
            ])))
            .unwrap()
            .len(),
            2
        );
        assert_eq!(
            explode(&Geometry::MultiPolygon(MultiPolygon::new(vec![
                polygon.clone(),
                polygon![
                    (x: 10.0, y: 0.0), (x: 14.0, y: 0.0),
                    (x: 14.0, y: 2.0), (x: 10.0, y: 2.0),
                    (x: 10.0, y: 0.0),
                ],
            ])))
            .unwrap()
            .len(),
            2
        );

        let duplicated = Geometry::MultiLineString(MultiLineString::new(vec![
            line_string![(x: -0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 0.0, y: -0.0), (x: 1.0, y: 0.0)],
            line_string![],
            line_string![(x: 5.0, y: 5.0), (x: 5.0, y: 5.0)],
        ]));
        assert_eq!(vertex_count(&boundary_unchecked(&duplicated)).unwrap(), 0);
    }

    proptest! {
        #[test]
        fn rectangle_measurements_hold_for_generated_inputs(
            x in -10_000_i32..10_000,
            y in -10_000_i32..10_000,
            width in 1_u16..1000,
            height in 1_u16..1000,
        ) {
            let x = f64::from(x);
            let y = f64::from(y);
            let width = f64::from(width);
            let height = f64::from(height);
            let geometry = Geometry::Polygon(polygon![
                (x: x, y: y), (x: x + width, y: y),
                (x: x + width, y: y + height), (x: x, y: y + height),
                (x: x, y: y),
            ]);
            prop_assert_eq!(area(&geometry).unwrap(), width * height);
            prop_assert_eq!(length(&geometry).unwrap(), 2.0 * (width + height));
            prop_assert_eq!(bounds(&geometry).unwrap(), Some([x, y, x + width, y + height]));
            prop_assert_eq!(vertex_count(&geometry).unwrap(), 5);
            let interior = point_on_surface(&geometry).unwrap().unwrap();
            prop_assert!(geometry.contains(&interior));
        }
    }
}
