//! Standalone OGC/DE-9IM predicates for filtering and validation workflows.

use geo::algorithm::validation::Validation;
use geo::{CoordsIter, Geometry, Relate};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialPredicate {
    Intersects,
    Disjoint,
    Contains,
    Within,
    EqualsTopo,
    Covers,
    CoveredBy,
    ContainsProperly,
    Touches,
    Crosses,
    Overlaps,
}

#[derive(Debug, Error)]
pub enum PredicateError {
    #[error("geometria {side} contiene coordinate NaN o infinite")]
    NonFiniteCoordinate { side: &'static str },
    #[error("geometria {side} non valida: {reason}")]
    InvalidGeometry { side: &'static str, reason: String },
}

fn validate(geometry: &Geometry<f64>, side: &'static str) -> Result<(), PredicateError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(PredicateError::NonFiniteCoordinate { side });
    }
    geometry
        .check_validation()
        .map_err(|error| PredicateError::InvalidGeometry {
            side,
            reason: error.to_string(),
        })
}

/// Valuta il predicato OGC/DE-9IM fra due geometrie, dopo la validazione.
///
/// # Errors
///
/// - `PredicateError::NonFiniteCoordinate`: `left` o `right` contiene
///   coordinate NaN o infinite;
/// - `PredicateError::InvalidGeometry`: `left` o `right` non supera la
///   validazione OGC (es. anello auto-intersecato).
pub fn evaluate(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: SpatialPredicate,
) -> Result<bool, PredicateError> {
    validate(left, "left")?;
    validate(right, "right")?;
    let matrix = left.relate(right);
    Ok(match predicate {
        SpatialPredicate::Intersects => matrix.is_intersects(),
        SpatialPredicate::Disjoint => matrix.is_disjoint(),
        SpatialPredicate::Contains => matrix.is_contains(),
        SpatialPredicate::Within => matrix.is_within(),
        SpatialPredicate::EqualsTopo => matrix.is_equal_topo(),
        SpatialPredicate::Covers => matrix.is_covers(),
        SpatialPredicate::CoveredBy => matrix.is_coveredby(),
        SpatialPredicate::ContainsProperly => matrix.is_contains_properly(),
        SpatialPredicate::Touches => matrix.is_touches(),
        SpatialPredicate::Crosses => matrix.is_crosses(),
        SpatialPredicate::Overlaps => matrix.is_overlaps(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, Point};

    #[test]
    fn de9im_predicates_distinguish_boundary_and_interior() {
        let area = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 2.0), (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ]);
        let inside = Geometry::Point(Point::new(1.0, 1.0));
        let boundary = Geometry::Point(Point::new(0.0, 1.0));
        assert!(evaluate(&area, &inside, SpatialPredicate::Contains).unwrap());
        assert!(evaluate(&area, &inside, SpatialPredicate::ContainsProperly).unwrap());
        assert!(!evaluate(&area, &boundary, SpatialPredicate::Contains).unwrap());
        assert!(evaluate(&area, &boundary, SpatialPredicate::Covers).unwrap());
        assert!(evaluate(&boundary, &area, SpatialPredicate::CoveredBy).unwrap());
        assert!(evaluate(&area, &boundary, SpatialPredicate::Touches).unwrap());
    }

    #[test]
    fn equality_crossing_overlap_and_disjoint_are_exact() {
        let horizontal = Geometry::LineString(line_string![
            (x: -1.0, y: 0.0), (x: 1.0, y: 0.0)
        ]);
        let vertical = Geometry::LineString(line_string![
            (x: 0.0, y: -1.0), (x: 0.0, y: 1.0)
        ]);
        assert!(evaluate(&horizontal, &vertical, SpatialPredicate::Crosses).unwrap());
        assert!(evaluate(&horizontal, &horizontal, SpatialPredicate::EqualsTopo).unwrap());
        let far = Geometry::Point(Point::new(100.0, 100.0));
        assert!(evaluate(&horizontal, &far, SpatialPredicate::Disjoint).unwrap());
    }

    #[test]
    fn invalid_left_and_right_inputs_are_rejected_before_relate() {
        let valid = Geometry::Point(Point::new(0.0, 0.0));
        let nan = Geometry::Point(Point::new(f64::NAN, 0.0));
        assert!(matches!(
            evaluate(&nan, &valid, SpatialPredicate::Intersects),
            Err(PredicateError::NonFiniteCoordinate { side: "left" })
        ));
        assert!(matches!(
            evaluate(&valid, &nan, SpatialPredicate::Intersects),
            Err(PredicateError::NonFiniteCoordinate { side: "right" })
        ));
        let invalid = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
        ]);
        assert!(matches!(
            evaluate(&valid, &invalid, SpatialPredicate::Intersects),
            Err(PredicateError::InvalidGeometry { side: "right", .. })
        ));
    }
}
