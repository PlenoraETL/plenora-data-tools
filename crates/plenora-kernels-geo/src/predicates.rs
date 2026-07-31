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
    Ok(evaluate_unchecked(left, right, predicate))
}

/// Variante di [`evaluate`] SENZA il gate di ingresso (scansione di
/// finitezza + validazione OGC su entrambe le geometrie).
///
/// # Precondizione (contratto del chiamante)
///
/// Entrambe le geometrie devono essere GIA' validate: coordinate finite e
/// validita' OGC, come garantito da [`crate::geometry_from_wkb`] al decode
/// o da un kernel che valida il proprio output. Su input che viola la
/// precondizione il risultato e' indefinito e nessun errore dedicato e'
/// garantito: la variante e' per i soli percorsi in cui la validazione e'
/// dimostrata per costruzione (R0.1: mai un'inferenza sui chiamanti — il
/// gate resta nella forma pubblica [`evaluate`]).
///
/// # Errors
///
/// Infallibile su input che rispetta la precondizione (il `Result` resta
/// per simmetria con [`evaluate`]); nessuna variante d'errore e'
/// raggiungibile.
pub fn evaluate_validated(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: SpatialPredicate,
) -> Result<bool, PredicateError> {
    Ok(evaluate_unchecked(left, right, predicate))
}

fn evaluate_unchecked(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    predicate: SpatialPredicate,
) -> bool {
    let matrix = left.relate(right);
    match predicate {
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
    }
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

    #[test]
    fn evaluate_validated_matches_the_gated_path_on_valid_inputs() {
        let area = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 2.0), (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ]);
        let others = [
            Geometry::Point(Point::new(1.0, 1.0)),
            Geometry::Point(Point::new(0.0, 1.0)),
            Geometry::Point(Point::new(100.0, 100.0)),
            Geometry::LineString(line_string![(x: -1.0, y: 1.0), (x: 3.0, y: 1.0)]),
        ];
        for other in &others {
            for predicate in [
                SpatialPredicate::Intersects,
                SpatialPredicate::Disjoint,
                SpatialPredicate::Contains,
                SpatialPredicate::Within,
                SpatialPredicate::EqualsTopo,
                SpatialPredicate::Covers,
                SpatialPredicate::CoveredBy,
                SpatialPredicate::ContainsProperly,
                SpatialPredicate::Touches,
                SpatialPredicate::Crosses,
                SpatialPredicate::Overlaps,
            ] {
                assert_eq!(
                    evaluate(&area, other, predicate).unwrap(),
                    evaluate_validated(&area, other, predicate).unwrap(),
                    "{predicate:?}"
                );
            }
        }
    }

    #[test]
    fn evaluate_validated_documents_the_caller_precondition() {
        // Test di documentazione del contratto, NON un nuovo modo di
        // accettare geometrie invalide in produzione: il percorso gated
        // rifiuta il bowtie (gate intatto), la variante validated lo prende
        // perche' la precondizione e' del chiamante — qui violata ad arte.
        let bowtie = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
        ]);
        let valid = Geometry::Point(Point::new(1.0, 1.0));
        assert!(matches!(
            evaluate(&bowtie, &valid, SpatialPredicate::Intersects),
            Err(PredicateError::InvalidGeometry { side: "left", .. })
        ));
        assert!(evaluate_validated(&bowtie, &valid, SpatialPredicate::Intersects).is_ok());
    }
}
