//! PROJ-backed CRS transformations using the bundled CRS database.

use crate::crs::{resolve_crs, validate_geometry_domain, validate_requirement, CrsError};
use plenora_core::catalog::CrsRequirement;
use geo::algorithm::validation::Validation;
use geo::{CoordsIter, Geometry, MapCoords};
use proj::Proj;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjBackendError {
    #[error(transparent)]
    Crs(#[from] CrsError),
    #[error("geometria di input non valida: {0}")]
    InvalidInput(String),
    #[error("trasformazione PROJ fallita: {0}")]
    Transformation(String),
    #[error("PROJ ha prodotto coordinate NaN o infinite")]
    NonFiniteOutput,
    #[error("geometria riproiettata non valida: {0}")]
    InvalidOutput(String),
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("numero coordinate non rappresentabile come uint64")]
    IndexOverflow,
}

/// Reprojects all coordinates using PROJ's normalized GIS axis order.
///
/// The normalized order is longitude/latitude and easting/northing. No
/// network grid download is enabled; the release artifact must ship an
/// explicitly versioned grid set.
///
/// # Errors
///
/// Restituisce `ProjBackendError` se un CRS non si risolve, se la
/// geometria di input non e' valida o supera `max_coordinates`, se la
/// trasformazione PROJ fallisce o produce coordinate non finite, o se
/// l'output non e' valido.
pub fn reproject_geometry(
    geometry: &Geometry<f64>,
    source_crs: &str,
    target_crs: &str,
    max_coordinates: u64,
) -> Result<Geometry<f64>, ProjBackendError> {
    Reprojector::new(source_crs, target_crs, max_coordinates)?.reproject(geometry)
}

/// Trasformazione riusabile per una coppia (source, target).
///
/// Risolve i CRS e costruisce la pipeline PROJ una sola volta per coppia,
/// poi la applica a ogni geometria. Non e' `Sync` (PROJ non lo e'): va
/// creata per thread o usata da un solo thread. Il comportamento e'
/// identico a `reproject_geometry`.
pub struct Reprojector {
    source: crate::crs::ResolvedCrs,
    target: crate::crs::ResolvedCrs,
    projection: Option<Proj>,
    max_coordinates: u64,
}

impl Reprojector {
    /// Risolve i CRS e costruisce la pipeline PROJ per la coppia.
    ///
    /// # Errors
    ///
    /// Restituisce `ProjBackendError` se un CRS non si risolve, se il
    /// requisito di riproiezione non e' soddisfatto o se la costruzione
    /// della pipeline PROJ fallisce.
    pub fn new(
        source_crs: &str,
        target_crs: &str,
        max_coordinates: u64,
    ) -> Result<Self, ProjBackendError> {
        let source = resolve_crs(source_crs, "source_crs")?;
        let target = resolve_crs(target_crs, "target_crs")?;
        validate_requirement(CrsRequirement::Reprojection, &[&source, &target])?;
        let projection = if source_crs.eq_ignore_ascii_case(target_crs) {
            None
        } else {
            Some(
                Proj::new_known_crs(source_crs, target_crs, None).map_err(|error| {
                    CrsError::InvalidDefinition {
                        name: "source_crs/target_crs",
                        reason: error.to_string(),
                    }
                })?,
            )
        };
        Ok(Self {
            source,
            target,
            projection,
            max_coordinates,
        })
    }

    /// Applica la trasformazione a una geometria.
    ///
    /// # Errors
    ///
    /// Restituisce `ProjBackendError` se la geometria di input non e'
    /// valida o supera il limite di coordinate, se la trasformazione PROJ
    /// fallisce o produce coordinate non finite, o se l'output non e'
    /// valido.
    pub fn reproject(&self, geometry: &Geometry<f64>) -> Result<Geometry<f64>, ProjBackendError> {
        if geometry
            .coords_iter()
            .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
        {
            return Err(ProjBackendError::InvalidInput(
                "coordinate NaN o infinite".to_owned(),
            ));
        }
        geometry
            .check_validation()
            .map_err(|error| ProjBackendError::InvalidInput(error.to_string()))?;
        validate_geometry_domain(geometry, &self.source)?;
        let actual =
            u64::try_from(geometry.coords_count()).map_err(|_| ProjBackendError::IndexOverflow)?;
        if actual > self.max_coordinates {
            return Err(ProjBackendError::CoordinateLimit {
                actual,
                limit: self.max_coordinates,
            });
        }
        let Some(projection) = &self.projection else {
            return Ok(geometry.clone());
        };

        let output = geometry
            .try_map_coords(|coordinate| projection.convert(coordinate))
            .map_err(|error| ProjBackendError::Transformation(error.to_string()))?;
        if output
            .coords_iter()
            .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
        {
            return Err(ProjBackendError::NonFiniteOutput);
        }
        output
            .check_validation()
            .map_err(|error| ProjBackendError::InvalidOutput(error.to_string()))?;
        validate_geometry_domain(&output, &self.target)?;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{polygon, Area, Point};

    #[test]
    fn wgs84_web_mercator_roundtrip_is_stable() {
        let input = Geometry::Point(Point::new(11.3426, 44.4949));
        let projected = reproject_geometry(&input, "EPSG:4326", "EPSG:3857", 10).unwrap();
        let Geometry::Point(projected_point) = &projected else {
            panic!("point expected")
        };
        assert!(projected_point.x() > 1_260_000.0 && projected_point.x() < 1_270_000.0);
        assert!(projected_point.y() > 5_540_000.0 && projected_point.y() < 5_550_000.0);
        let roundtrip = reproject_geometry(&projected, "EPSG:3857", "EPSG:4326", 10).unwrap();
        let Geometry::Point(roundtrip) = roundtrip else {
            panic!("point expected")
        };
        assert!((roundtrip.x() - 11.3426).abs() < 1e-9);
        assert!((roundtrip.y() - 44.4949).abs() < 1e-9);
    }

    #[test]
    fn polygon_remains_valid_and_limits_fail_closed() {
        let polygon = Geometry::Polygon(polygon![
            (x: 11.0, y: 44.0), (x: 12.0, y: 44.0),
            (x: 12.0, y: 45.0), (x: 11.0, y: 45.0),
            (x: 11.0, y: 44.0),
        ]);
        let projected = reproject_geometry(&polygon, "EPSG:4326", "EPSG:32632", 10).unwrap();
        assert!(projected.unsigned_area() > 8_000_000_000.0);
        assert!(matches!(
            reproject_geometry(&polygon, "EPSG:4326", "EPSG:32632", 4),
            Err(ProjBackendError::CoordinateLimit { .. })
        ));
        assert!(reproject_geometry(&polygon, "EPSG:NOT_REAL", "EPSG:32632", 10).is_err());
    }

    #[test]
    fn identical_crs_is_an_exact_noop() {
        let input = Geometry::Point(Point::new(11.3426, 44.4949));
        assert_eq!(
            reproject_geometry(&input, "EPSG:4326", "epsg:4326", 10).unwrap(),
            input
        );
    }

    #[test]
    fn reprojector_reuses_one_pipeline_with_identical_output() {
        let reprojector = Reprojector::new("EPSG:4326", "EPSG:3857", 100).unwrap();
        let inputs = [
            Geometry::Point(Point::new(11.3426, 44.4949)),
            Geometry::Point(Point::new(12.0, 41.0)),
            Geometry::Point(Point::new(-3.7, 40.4)),
        ];
        for _ in 0..4 {
            for input in &inputs {
                assert_eq!(
                    reprojector.reproject(input).unwrap(),
                    reproject_geometry(input, "EPSG:4326", "EPSG:3857", 100).unwrap()
                );
            }
        }
        assert!(matches!(
            Reprojector::new("EPSG:4326", "EPSG:NOT_REAL", 100),
            Err(ProjBackendError::Crs(_))
        ));
    }

    #[test]
    fn rejects_hostile_crs_and_non_finite_geometry() {
        let point = Geometry::Point(Point::new(0.0, 0.0));
        for crs in ["", "\0EPSG:4326"] {
            assert!(matches!(
                reproject_geometry(&point, crs, "EPSG:3857", 10),
                Err(ProjBackendError::Crs(_))
            ));
        }
        let too_long = "X".repeat(513);
        assert!(reproject_geometry(&point, "EPSG:4326", &too_long, 10).is_err());
        assert!(matches!(
            reproject_geometry(
                &Geometry::Point(Point::new(f64::NAN, 0.0)),
                "EPSG:4326",
                "EPSG:3857",
                10,
            ),
            Err(ProjBackendError::InvalidInput(_))
        ));
    }
}
