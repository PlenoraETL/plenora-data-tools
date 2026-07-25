//! Exact binary and aggregate kernels with explicit expansion/work limits.
//!
//! Attribute propagation and CRS transformation live in the tabular adapter;
//! this module returns deterministic row lineage and scalar results.

use std::sync::atomic::{AtomicU64, Ordering};

use geo::algorithm::line_measures::{Distance, Euclidean};
use geo::algorithm::validation::Validation;
use geo::{CoordsIter, Geometry};
use rayon::prelude::*;
use thiserror::Error;

use crate::spatial_join::{spatial_join_nullable, JoinPredicate, SpatialJoinError};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearestMatch {
    pub left: u64,
    pub right: u64,
    pub distance: f64,
}

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error(transparent)]
    SpatialJoin(#[from] SpatialJoinError),
    #[error("limite di lavoro deve essere maggiore di zero")]
    InvalidWorkLimit,
    #[error("numero di confronti oltre il limite di {limit}")]
    WorkLimitExceeded { limit: u64 },
    #[error("numero di risultati oltre il limite di {limit}")]
    ResultLimitExceeded { limit: u64 },
    #[error("max_distance deve essere finita e non negativa")]
    InvalidMaximumDistance,
    #[error("indice non rappresentabile come uint64")]
    IndexOverflow,
    #[error("geometria {side}[{index}] non valida: {reason}")]
    InvalidGeometry {
        side: &'static str,
        index: usize,
        reason: String,
    },
}

fn validate_geometries(
    geometries: &[Option<Geometry<f64>>],
    side: &'static str,
) -> Result<(), AnalysisError> {
    for (index, geometry) in geometries.iter().enumerate() {
        let Some(geometry) = geometry else {
            continue;
        };
        if geometry
            .coords_iter()
            .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
        {
            return Err(AnalysisError::InvalidGeometry {
                side,
                index,
                reason: "coordinate NaN o infinite".to_owned(),
            });
        }
        geometry
            .check_validation()
            .map_err(|error| AnalysisError::InvalidGeometry {
                side,
                index,
                reason: error.to_string(),
            })?;
    }
    Ok(())
}

/// For every left row, returns the minimum planar distance to any non-null,
/// non-empty right geometry. `None` is returned when either the left geometry
/// is null/empty or the right side has no usable geometry.
pub fn minimum_distances(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_comparisons: u64,
) -> Result<Vec<Option<f64>>, AnalysisError> {
    if max_comparisons == 0 {
        return Err(AnalysisError::InvalidWorkLimit);
    }
    validate_geometries(left, "left")?;
    validate_geometries(right, "right")?;
    let usable_right: Vec<_> = right
        .iter()
        .filter_map(Option::as_ref)
        .filter(|geometry| geometry.coords_count() > 0)
        .collect();
    let comparisons = u64::try_from(left.iter().flatten().count())
        .map_err(|_| AnalysisError::IndexOverflow)?
        .checked_mul(u64::try_from(usable_right.len()).map_err(|_| AnalysisError::IndexOverflow)?)
        .ok_or(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        })?;
    if comparisons > max_comparisons {
        return Err(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        });
    }

    Ok(left
        .par_iter()
        .map(|geometry| {
            let geometry = geometry.as_ref()?;
            if geometry.coords_count() == 0 || usable_right.is_empty() {
                return None;
            }
            usable_right
                .iter()
                .map(|right| Euclidean.distance(geometry, *right))
                .reduce(f64::min)
        })
        .collect())
}

/// Exact nearest-neighbour lineage. All equidistant nearest rows are emitted,
/// matching the duplicate-on-tie behaviour of GeoPandas `sjoin_nearest`.
pub fn nearest_matches(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_distance: Option<f64>,
    max_comparisons: u64,
    max_results: u64,
) -> Result<Vec<NearestMatch>, AnalysisError> {
    if max_comparisons == 0 || max_results == 0 {
        return Err(AnalysisError::InvalidWorkLimit);
    }
    if max_distance.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err(AnalysisError::InvalidMaximumDistance);
    }
    validate_geometries(left, "left")?;
    validate_geometries(right, "right")?;
    let usable_right: Vec<_> = right
        .iter()
        .enumerate()
        .filter_map(|(index, geometry)| {
            geometry
                .as_ref()
                .filter(|value| value.coords_count() > 0)
                .map(|value| (index, value))
        })
        .collect();
    let comparisons = u64::try_from(left.iter().flatten().count())
        .map_err(|_| AnalysisError::IndexOverflow)?
        .checked_mul(u64::try_from(usable_right.len()).map_err(|_| AnalysisError::IndexOverflow)?)
        .ok_or(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        })?;
    if comparisons > max_comparisons {
        return Err(AnalysisError::WorkLimitExceeded {
            limit: max_comparisons,
        });
    }

    let result_count = AtomicU64::new(0);
    let grouped: Result<Vec<Vec<NearestMatch>>, AnalysisError> = left
        .par_iter()
        .enumerate()
        .map(|(left_index, geometry)| {
            let Some(geometry) = geometry.as_ref().filter(|value| value.coords_count() > 0) else {
                return Ok(Vec::new());
            };
            let mut distances: Vec<_> = usable_right
                .iter()
                .map(|(right_index, right)| (*right_index, Euclidean.distance(geometry, *right)))
                .collect();
            let Some(minimum) = distances
                .iter()
                .map(|(_, distance)| *distance)
                .reduce(f64::min)
            else {
                return Ok(Vec::new());
            };
            if max_distance.is_some_and(|limit| minimum > limit) {
                return Ok(Vec::new());
            }
            distances.retain(|(_, distance)| *distance == minimum);
            distances.sort_unstable_by_key(|(right_index, _)| *right_index);
            let additional =
                u64::try_from(distances.len()).map_err(|_| AnalysisError::IndexOverflow)?;
            result_count
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    current
                        .checked_add(additional)
                        .filter(|next| *next <= max_results)
                })
                .map_err(|_| AnalysisError::ResultLimitExceeded { limit: max_results })?;
            let left = u64::try_from(left_index).map_err(|_| AnalysisError::IndexOverflow)?;
            distances
                .into_iter()
                .map(|(right_index, distance)| {
                    Ok(NearestMatch {
                        left,
                        right: u64::try_from(right_index)
                            .map_err(|_| AnalysisError::IndexOverflow)?,
                        distance,
                    })
                })
                .collect()
        })
        .collect();
    Ok(grouped?.into_iter().flatten().collect())
}

/// Returns the stable left row indexes that are within at least one right row.
pub fn within_indexes(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    max_pairs: u64,
) -> Result<Vec<u64>, AnalysisError> {
    let pairs = spatial_join_nullable(left, right, JoinPredicate::Within, max_pairs)?;
    let mut indexes: Vec<_> = pairs.into_iter().map(|pair| pair.left).collect();
    indexes.dedup();
    Ok(indexes)
}

/// Counts points strictly within every polygon row. Boundary points are not
/// counted, matching Manipola's `predicate="within"` contract.
pub fn count_points_in_polygons(
    polygons: &[Option<Geometry<f64>>],
    points: &[Option<Geometry<f64>>],
    max_pairs: u64,
) -> Result<Vec<u64>, AnalysisError> {
    let pairs = spatial_join_nullable(points, polygons, JoinPredicate::Within, max_pairs)?;
    let mut counts = vec![0_u64; polygons.len()];
    for pair in pairs {
        let index = usize::try_from(pair.right).map_err(|_| AnalysisError::IndexOverflow)?;
        counts[index] = counts[index]
            .checked_add(1)
            .ok_or(AnalysisError::IndexOverflow)?;
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{polygon, Point};

    fn point(x: f64, y: f64) -> Option<Geometry<f64>> {
        Some(Geometry::Point(Point::new(x, y)))
    }

    fn square() -> Option<Geometry<f64>> {
        Some(Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 2.0), (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ]))
    }

    #[test]
    fn distances_preserve_null_rows_and_reject_unbounded_work() {
        let left = vec![point(0.0, 0.0), None, point(3.0, 4.0)];
        let right = vec![point(0.0, 4.0)];
        assert_eq!(
            minimum_distances(&left, &right, 10).unwrap(),
            vec![Some(4.0), None, Some(3.0)]
        );
        assert!(matches!(
            minimum_distances(&left, &right, 1),
            Err(AnalysisError::WorkLimitExceeded { limit: 1 })
        ));
    }

    #[test]
    fn invalid_topology_is_rejected_on_both_sides_before_distance_work() {
        let invalid = Some(Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
        ]));
        assert!(matches!(
            minimum_distances(std::slice::from_ref(&invalid), &[point(0.0, 0.0)], 1),
            Err(AnalysisError::InvalidGeometry { side: "left", .. })
        ));
        assert!(matches!(
            minimum_distances(&[point(0.0, 0.0)], std::slice::from_ref(&invalid), 1),
            Err(AnalysisError::InvalidGeometry { side: "right", .. })
        ));
        assert!(matches!(
            nearest_matches(
                &[point(0.0, 0.0)],
                std::slice::from_ref(&invalid),
                None,
                1,
                1,
            ),
            Err(AnalysisError::InvalidGeometry { side: "right", .. })
        ));
    }

    #[test]
    fn nearest_emits_stable_ties_and_honours_max_distance() {
        let left = vec![point(0.0, 0.0)];
        let right = vec![point(-1.0, 0.0), None, point(1.0, 0.0)];
        assert_eq!(
            nearest_matches(&left, &right, None, 10, 10).unwrap(),
            vec![
                NearestMatch {
                    left: 0,
                    right: 0,
                    distance: 1.0
                },
                NearestMatch {
                    left: 0,
                    right: 2,
                    distance: 1.0
                },
            ]
        );
        assert!(nearest_matches(&left, &right, Some(0.5), 10, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn within_and_point_counts_use_strict_boundary_semantics() {
        let polygons = vec![square(), None];
        let points = vec![point(1.0, 1.0), point(0.0, 1.0), None];
        assert_eq!(within_indexes(&points, &polygons, 10).unwrap(), vec![0]);
        assert_eq!(
            count_points_in_polygons(&polygons, &points, 10).unwrap(),
            vec![1, 0]
        );
    }

    #[test]
    fn adversarial_limits_empty_inputs_and_invalid_coordinates() {
        assert!(matches!(
            minimum_distances(&[], &[], 0),
            Err(AnalysisError::InvalidWorkLimit)
        ));
        assert_eq!(minimum_distances(&[None], &[], 1).unwrap(), vec![None]);
        assert!(minimum_distances(
            &[Some(Geometry::Point(Point::new(f64::NAN, 0.0)))],
            &[point(0.0, 0.0)],
            10,
        )
        .is_err());
        assert!(nearest_matches(
            &[point(0.0, 0.0)],
            &[point(0.0, 0.0)],
            Some(f64::NAN),
            10,
            10,
        )
        .is_err());
        assert!(nearest_matches(&[point(0.0, 0.0)], &[point(0.0, 0.0)], None, 0, 10).is_err());
        assert!(matches!(
            nearest_matches(
                &[point(0.0, 0.0)],
                &[point(-1.0, 0.0), point(1.0, 0.0)],
                None,
                10,
                1,
            ),
            Err(AnalysisError::ResultLimitExceeded { .. })
        ));
        assert!(nearest_matches(&[None], &[], None, 1, 1)
            .unwrap()
            .is_empty());
    }
}
