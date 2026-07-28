//! Advanced pure-Rust kernels whose output cardinality differs from the input.

use geo::algorithm::validation::Validation;
use geo::{Geometry, Intersects, MultiPoint, Point, Voronoi};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdvancedError {
    #[error("max_points deve essere almeno 2")]
    InvalidPointLimit,
    #[error("Voronoi richiede almeno due punti")]
    InsufficientPoints,
    #[error("Voronoi: {actual} punti oltre il limite di {limit}")]
    PointLimitExceeded { actual: usize, limit: usize },
    #[error("Voronoi accetta solo Point; riga {index}: {geometry_type}")]
    ExpectedPoint {
        index: usize,
        geometry_type: &'static str,
    },
    #[error("punto non valido alla riga {index}: {reason}")]
    InvalidPoint { index: usize, reason: String },
    #[error("costruzione Voronoi fallita: {0}")]
    Voronoi(String),
    #[error("nessuna cella Voronoi associabile alla riga {0}")]
    UnmatchedPoint(usize),
    #[error("cella Voronoi non valida: {0}")]
    InvalidOutput(String),
}

const fn geometry_name(geometry: &Geometry<f64>) -> &'static str {
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

/// One bounded Voronoi polygon for every input point, retaining input order.
///
/// Duplicate points receive the same cell. Nulls are intentionally not
/// accepted because Manipola's current `MultiPoint` construction rejects them.
///
/// # Errors
///
/// - `InvalidPointLimit`: `max_points` is below 2.
/// - `InsufficientPoints`: fewer than 2 input geometries.
/// - `PointLimitExceeded`: more than `max_points` input geometries.
/// - `InvalidPoint`: an input geometry fails OGC validation (e.g. NaN
///   coordinates).
/// - `ExpectedPoint`: an input geometry is not a `Point`.
/// - `Voronoi`: the Voronoi construction itself failed.
/// - `InvalidOutput`: a produced cell fails OGC validation.
/// - `UnmatchedPoint`: no produced cell intersects an input point.
pub fn voronoi_cells(
    geometries: &[Geometry<f64>],
    max_points: usize,
) -> Result<Vec<Geometry<f64>>, AdvancedError> {
    if max_points < 2 {
        return Err(AdvancedError::InvalidPointLimit);
    }
    if geometries.len() < 2 {
        return Err(AdvancedError::InsufficientPoints);
    }
    if geometries.len() > max_points {
        return Err(AdvancedError::PointLimitExceeded {
            actual: geometries.len(),
            limit: max_points,
        });
    }

    let points: Vec<Point<f64>> = geometries
        .iter()
        .enumerate()
        .map(|(index, geometry)| {
            geometry
                .check_validation()
                .map_err(|error| AdvancedError::InvalidPoint {
                    index,
                    reason: error.to_string(),
                })?;
            match geometry {
                Geometry::Point(point) => Ok(*point),
                value => Err(AdvancedError::ExpectedPoint {
                    index,
                    geometry_type: geometry_name(value),
                }),
            }
        })
        .collect::<Result<_, _>>()?;

    let cells = MultiPoint::new(points.clone())
        .voronoi_cells()
        .map_err(|error| AdvancedError::Voronoi(error.to_string()))?;
    for cell in &cells {
        cell.check_validation()
            .map_err(|error| AdvancedError::InvalidOutput(error.to_string()))?;
    }

    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            cells
                .iter()
                .find(|cell| cell.intersects(point))
                .cloned()
                .map(Geometry::Polygon)
                .ok_or(AdvancedError::UnmatchedPoint(index))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Area, Contains};

    #[test]
    fn voronoi_preserves_input_order_and_contains_each_site() {
        let sites = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Point(Point::new(2.0, 0.0)),
            Geometry::Point(Point::new(1.0, 2.0)),
        ];
        let cells = voronoi_cells(&sites, 10).unwrap();
        assert_eq!(cells.len(), sites.len());
        for (cell, site) in cells.iter().zip(&sites) {
            assert!(cell.contains(site) || cell.intersects(site));
            assert!(cell.unsigned_area() > 0.0);
        }
    }

    #[test]
    fn voronoi_rejects_invalid_shape_and_resource_limit() {
        let sites = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Point(Point::new(2.0, 0.0)),
            Geometry::Point(Point::new(1.0, 2.0)),
        ];
        assert!(matches!(
            voronoi_cells(&sites, 2),
            Err(AdvancedError::PointLimitExceeded { .. })
        ));
        let non_points = vec![
            sites[0].clone(),
            geo::Rect::new((0.0, 0.0), (1.0, 1.0)).into(),
        ];
        assert!(matches!(
            voronoi_cells(&non_points, 10),
            Err(AdvancedError::ExpectedPoint { index: 1, .. })
        ));
        assert!(matches!(
            voronoi_cells(&[], 1),
            Err(AdvancedError::InvalidPointLimit)
        ));
        assert!(matches!(
            voronoi_cells(&[], 2),
            Err(AdvancedError::InsufficientPoints)
        ));
        let invalid = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Point(Point::new(f64::NAN, 1.0)),
        ];
        assert!(matches!(
            voronoi_cells(&invalid, 2),
            Err(AdvancedError::InvalidPoint { .. })
        ));
        let variants = vec![
            Geometry::Line(geo::Line::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::LineString(geo::LineString::from(vec![(0.0, 0.0), (1.0, 1.0)])),
            Geometry::Polygon(geo::Rect::new((0.0, 0.0), (1.0, 1.0)).to_polygon()),
            Geometry::MultiPoint(vec![Point::new(0.0, 0.0)].into()),
            Geometry::MultiLineString(geo::MultiLineString::new(Vec::new())),
            Geometry::MultiPolygon(geo::MultiPolygon::new(Vec::new())),
            Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into()),
            Geometry::Triangle(geo::Triangle::new(
                geo::Coord { x: 0.0, y: 0.0 },
                geo::Coord { x: 1.0, y: 0.0 },
                geo::Coord { x: 0.0, y: 1.0 },
            )),
        ];
        for variant in variants {
            assert!(matches!(
                voronoi_cells(&[sites[0].clone(), variant], 2),
                Err(AdvancedError::ExpectedPoint { .. })
            ));
        }
    }
}
