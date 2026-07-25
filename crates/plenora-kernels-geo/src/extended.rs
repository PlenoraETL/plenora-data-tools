//! Operations beyond the original Manipola catalog.

use geo::algorithm::concave_hull::ConcaveHullOptions;
use geo::algorithm::line_measures::{Distance, Geodesic, Haversine, Length};
use geo::algorithm::validation::Validation;
use geo::{
    AffineOps, AffineTransform, ConcaveHull, CoordsIter, Geometry, HausdorffDistance, Point,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtendedError {
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    #[error("geometria prodotta non valida: {0}")]
    InvalidOutput(String),
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("confronti Hausdorff oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error("coordinate geografiche fuori intervallo lon/lat")]
    InvalidGeographicCoordinate,
    #[error("indice non rappresentabile come uint64")]
    IndexOverflow,
}

fn validate_input(geometry: &Geometry<f64>) -> Result<(), ExtendedError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(ExtendedError::InvalidInput(
            "coordinate NaN o infinite".to_owned(),
        ));
    }
    geometry
        .check_validation()
        .map_err(|error| ExtendedError::InvalidInput(error.to_string()))
}

fn validate_output(geometry: Geometry<f64>) -> Result<Geometry<f64>, ExtendedError> {
    geometry
        .check_validation()
        .map_err(|error| ExtendedError::InvalidOutput(error.to_string()))?;
    Ok(geometry)
}

/// Applies the standard six-coefficient 2D affine matrix
/// `[a, b, xoff, d, e, yoff]`.
pub fn affine_transform(
    geometry: &Geometry<f64>,
    coefficients: [f64; 6],
) -> Result<Geometry<f64>, ExtendedError> {
    validate_input(geometry)?;
    if coefficients.iter().any(|value| !value.is_finite()) {
        return Err(ExtendedError::InvalidParameter {
            name: "coefficients",
            reason: "devono essere finiti",
        });
    }
    let [a, b, xoff, d, e, yoff] = coefficients;
    let transform = AffineTransform::new(a, b, xoff, d, e, yoff);
    validate_output(geometry.affine_transform(&transform))
}

pub fn translate(
    geometry: &Geometry<f64>,
    x_offset: f64,
    y_offset: f64,
) -> Result<Geometry<f64>, ExtendedError> {
    affine_transform(geometry, [1.0, 0.0, x_offset, 0.0, 1.0, y_offset])
}

pub fn scale_about(
    geometry: &Geometry<f64>,
    x_factor: f64,
    y_factor: f64,
    origin: Point<f64>,
) -> Result<Geometry<f64>, ExtendedError> {
    let x_offset = origin.x() * (1.0 - x_factor);
    let y_offset = origin.y() * (1.0 - y_factor);
    affine_transform(geometry, [x_factor, 0.0, x_offset, 0.0, y_factor, y_offset])
}

pub fn rotate_about(
    geometry: &Geometry<f64>,
    degrees: f64,
    origin: Point<f64>,
) -> Result<Geometry<f64>, ExtendedError> {
    if !degrees.is_finite() {
        return Err(ExtendedError::InvalidParameter {
            name: "degrees",
            reason: "deve essere finito",
        });
    }
    let radians = degrees.to_radians();
    let cosine = radians.cos();
    let sine = radians.sin();
    let x_offset = origin.x() - cosine * origin.x() + sine * origin.y();
    let y_offset = origin.y() - sine * origin.x() - cosine * origin.y();
    affine_transform(geometry, [cosine, -sine, x_offset, sine, cosine, y_offset])
}

pub fn concave_hull(
    geometry: &Geometry<f64>,
    concavity: f64,
    length_threshold: f64,
    max_coordinates: u64,
) -> Result<Geometry<f64>, ExtendedError> {
    validate_input(geometry)?;
    if !concavity.is_finite() || concavity <= 0.0 {
        return Err(ExtendedError::InvalidParameter {
            name: "concavity",
            reason: "deve essere finita e maggiore di zero",
        });
    }
    if !length_threshold.is_finite() || length_threshold < 0.0 {
        return Err(ExtendedError::InvalidParameter {
            name: "length_threshold",
            reason: "deve essere finita e non negativa",
        });
    }
    let coordinates: Vec<_> = geometry.coords_iter().collect();
    let actual = u64::try_from(coordinates.len()).map_err(|_| ExtendedError::IndexOverflow)?;
    if actual > max_coordinates {
        return Err(ExtendedError::CoordinateLimit {
            actual,
            limit: max_coordinates,
        });
    }
    let hull = coordinates.concave_hull_with_options(ConcaveHullOptions {
        concavity,
        length_threshold,
    });
    validate_output(Geometry::Polygon(hull))
}

/// Vertex-based Hausdorff distance, bounded because its complexity is O(n*m).
pub fn hausdorff_distance(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    max_coordinate_pairs: u64,
) -> Result<Option<f64>, ExtendedError> {
    validate_input(left)?;
    validate_input(right)?;
    let left_count =
        u64::try_from(left.coords_count()).map_err(|_| ExtendedError::IndexOverflow)?;
    let right_count =
        u64::try_from(right.coords_count()).map_err(|_| ExtendedError::IndexOverflow)?;
    if left_count == 0 || right_count == 0 {
        return Ok(None);
    }
    let actual = left_count
        .checked_mul(right_count)
        .ok_or(ExtendedError::WorkLimit {
            actual: u64::MAX,
            limit: max_coordinate_pairs,
        })?;
    if actual > max_coordinate_pairs {
        return Err(ExtendedError::WorkLimit {
            actual,
            limit: max_coordinate_pairs,
        });
    }
    Ok(Some(left.hausdorff_distance(right)))
}

fn validate_geographic_point(point: Point<f64>) -> Result<(), ExtendedError> {
    if !point.x().is_finite()
        || !point.y().is_finite()
        || !(-180.0..=180.0).contains(&point.x())
        || !(-90.0..=90.0).contains(&point.y())
    {
        return Err(ExtendedError::InvalidGeographicCoordinate);
    }
    Ok(())
}

pub fn haversine_distance_m(left: Point<f64>, right: Point<f64>) -> Result<f64, ExtendedError> {
    validate_geographic_point(left)?;
    validate_geographic_point(right)?;
    Ok(Haversine.distance(left, right))
}

pub fn geodesic_distance_m(left: Point<f64>, right: Point<f64>) -> Result<f64, ExtendedError> {
    validate_geographic_point(left)?;
    validate_geographic_point(right)?;
    Ok(Geodesic.distance(left, right))
}

pub fn geodesic_line_length_m(line: &geo::LineString<f64>) -> Result<f64, ExtendedError> {
    for coordinate in line.coords() {
        validate_geographic_point(Point::from(*coordinate))?;
    }
    Ok(Geodesic.length(line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, Area};

    #[test]
    fn affine_wrappers_preserve_expected_coordinates_and_area() {
        let geometry = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 1.0), (x: 0.0, y: 1.0),
            (x: 0.0, y: 0.0),
        ]);
        let translated = translate(&geometry, 10.0, -5.0).unwrap();
        assert_eq!(translated.unsigned_area(), geometry.unsigned_area());
        let scaled = scale_about(&geometry, 2.0, 3.0, Point::new(0.0, 0.0)).unwrap();
        assert_eq!(scaled.unsigned_area(), 12.0);
        let rotated = rotate_about(&geometry, 90.0, Point::new(0.0, 0.0)).unwrap();
        assert!((rotated.unsigned_area() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn concave_hull_and_hausdorff_are_bounded() {
        let geometry = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 1.5, y: 1.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0),
        ]);
        let hull = concave_hull(&geometry, 1.0, 0.0, 10).unwrap();
        assert!(hull.unsigned_area() > 0.0);
        assert!(matches!(
            concave_hull(&geometry, 1.0, 0.0, 2),
            Err(ExtendedError::CoordinateLimit { .. })
        ));
        assert_eq!(
            hausdorff_distance(&geometry, &geometry, 100).unwrap(),
            Some(0.0)
        );
        assert!(matches!(
            hausdorff_distance(&geometry, &geometry, 1),
            Err(ExtendedError::WorkLimit { .. })
        ));
    }

    #[test]
    fn geographic_distances_validate_ranges_and_units() {
        let bologna = Point::new(11.3426, 44.4949);
        let modena = Point::new(10.9252, 44.6471);
        let geodesic = geodesic_distance_m(bologna, modena).unwrap();
        let haversine = haversine_distance_m(bologna, modena).unwrap();
        assert!(geodesic > 35_000.0 && geodesic < 40_000.0);
        assert!((geodesic - haversine).abs() < 200.0);
        assert!(geodesic_distance_m(Point::new(200.0, 0.0), modena).is_err());

        let equator = line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)];
        let length = geodesic_line_length_m(&equator).unwrap();
        assert!((length - 111_319.490_793).abs() < 0.01);
    }

    #[test]
    fn every_invalid_parameter_and_empty_path_is_fail_closed() {
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]);
        assert!(affine_transform(&line, [f64::NAN; 6]).is_err());
        assert!(affine_transform(
            &Geometry::Point(Point::new(f64::NAN, 0.0)),
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        )
        .is_err());
        assert!(rotate_about(&line, f64::INFINITY, Point::new(0.0, 0.0)).is_err());
        for (concavity, threshold) in [(0.0, 0.0), (f64::NAN, 0.0), (1.0, -1.0), (1.0, f64::NAN)] {
            assert!(concave_hull(&line, concavity, threshold, 100).is_err());
        }
        let empty = Geometry::LineString(geo::LineString::new(Vec::new()));
        assert_eq!(hausdorff_distance(&empty, &line, 100).unwrap(), None);
        assert!(hausdorff_distance(&line, &line, 1).is_err());
        for point in [
            Point::new(181.0, 0.0),
            Point::new(0.0, 91.0),
            Point::new(f64::NAN, 0.0),
        ] {
            assert!(haversine_distance_m(point, Point::new(0.0, 0.0)).is_err());
            assert!(geodesic_distance_m(point, Point::new(0.0, 0.0)).is_err());
        }
    }
}
