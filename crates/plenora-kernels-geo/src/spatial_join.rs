//! Deterministic, bounded spatial join based on an R-tree candidate index.

use std::sync::atomic::{AtomicU64, Ordering};

use geo::algorithm::validation::Validation;
use geo::{BoundingRect, Contains, CoordsIter, Geometry, Intersects, Relate};
use rayon::prelude::*;
use rstar::{RTree, RTreeObject, AABB};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinPredicate {
    Intersects,
    Contains,
    Within,
    Crosses,
    Overlaps,
    Touches,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JoinPair {
    pub left: u64,
    pub right: u64,
}

#[derive(Debug, Error)]
pub enum SpatialJoinError {
    #[error("numero geometrie non rappresentabile nel protocollo uint64")]
    IndexOverflow,
    #[error("max_pairs deve essere maggiore di zero")]
    InvalidPairLimit,
    #[error("spatial join oltre il limite di {limit} coppie")]
    PairLimitExceeded { limit: u64 },
    #[error("geometria {side}[{index}] contiene coordinate NaN o infinite")]
    NonFiniteCoordinate { side: &'static str, index: usize },
    #[error("geometria {side}[{index}] non valida: {reason}")]
    InvalidGeometry {
        side: &'static str,
        index: usize,
        reason: String,
    },
}

#[derive(Clone, Copy)]
struct IndexedEnvelope {
    index: usize,
    envelope: AABB<[f64; 2]>,
}

impl RTreeObject for IndexedEnvelope {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

fn checked_envelope(
    geometry: &Geometry<f64>,
    side: &'static str,
    index: usize,
) -> Result<Option<AABB<[f64; 2]>>, SpatialJoinError> {
    if geometry
        .coords_iter()
        .any(|coordinate| !coordinate.x.is_finite() || !coordinate.y.is_finite())
    {
        return Err(SpatialJoinError::NonFiniteCoordinate { side, index });
    }
    geometry
        .check_validation()
        .map_err(|error| SpatialJoinError::InvalidGeometry {
            side,
            index,
            reason: error.to_string(),
        })?;
    let Some(rect) = geometry.bounding_rect() else {
        return Ok(None);
    };
    let min = rect.min();
    let max = rect.max();
    if !min.x.is_finite() || !min.y.is_finite() || !max.x.is_finite() || !max.y.is_finite() {
        return Err(SpatialJoinError::NonFiniteCoordinate { side, index });
    }
    Ok(Some(AABB::from_corners([min.x, min.y], [max.x, max.y])))
}

fn exact_match(left: &Geometry<f64>, right: &Geometry<f64>, predicate: JoinPredicate) -> bool {
    match predicate {
        JoinPredicate::Intersects => left.intersects(right),
        JoinPredicate::Contains => left.contains(right),
        JoinPredicate::Within => right.contains(left),
        JoinPredicate::Crosses => left.relate(right).is_crosses(),
        JoinPredicate::Overlaps => left.relate(right).is_overlaps(),
        JoinPredicate::Touches => left.relate(right).is_touches(),
    }
}

/// Returns `(left_index, right_index)` pairs in stable lexicographic order.
///
/// Empty geometries produce no pairs. Bounding boxes only select candidates;
/// every result is confirmed by the requested exact geometry predicate.
pub fn spatial_join(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    predicate: JoinPredicate,
    max_pairs: u64,
) -> Result<Vec<JoinPair>, SpatialJoinError> {
    let left_refs: Vec<_> = left.iter().map(Some).collect();
    let right_refs: Vec<_> = right.iter().map(Some).collect();
    spatial_join_refs(&left_refs, &right_refs, predicate, max_pairs)
}

/// Nullable variant used by framed transports. `None` rows never match, but
/// retain their original positional index in every emitted pair.
pub fn spatial_join_nullable(
    left: &[Option<Geometry<f64>>],
    right: &[Option<Geometry<f64>>],
    predicate: JoinPredicate,
    max_pairs: u64,
) -> Result<Vec<JoinPair>, SpatialJoinError> {
    let left_refs: Vec<_> = left.iter().map(Option::as_ref).collect();
    let right_refs: Vec<_> = right.iter().map(Option::as_ref).collect();
    spatial_join_refs(&left_refs, &right_refs, predicate, max_pairs)
}

fn spatial_join_refs(
    left: &[Option<&Geometry<f64>>],
    right: &[Option<&Geometry<f64>>],
    predicate: JoinPredicate,
    max_pairs: u64,
) -> Result<Vec<JoinPair>, SpatialJoinError> {
    if max_pairs == 0 {
        return Err(SpatialJoinError::InvalidPairLimit);
    }
    u64::try_from(left.len()).map_err(|_| SpatialJoinError::IndexOverflow)?;
    u64::try_from(right.len()).map_err(|_| SpatialJoinError::IndexOverflow)?;

    let right_envelopes: Result<Vec<Option<_>>, _> = right
        .iter()
        .enumerate()
        .map(|(index, geometry)| {
            let Some(geometry) = geometry else {
                return Ok(None);
            };
            checked_envelope(geometry, "right", index)
                .map(|envelope| envelope.map(|envelope| IndexedEnvelope { index, envelope }))
        })
        .collect();
    let tree = RTree::bulk_load(right_envelopes?.into_iter().flatten().collect());
    let pair_count = AtomicU64::new(0);

    let grouped: Result<Vec<Vec<JoinPair>>, SpatialJoinError> = left
        .par_iter()
        .enumerate()
        .map(|(left_index, left_geometry)| {
            let Some(left_geometry) = *left_geometry else {
                return Ok(Vec::new());
            };
            let Some(envelope) = checked_envelope(left_geometry, "left", left_index)? else {
                return Ok(Vec::new());
            };
            // The pair limit is enforced per confirmed match, before pushing
            // onto the group vector: a single left geometry with millions of
            // matches must fail without materializing them all first.
            let mut right_indexes: Vec<usize> = Vec::new();
            for candidate in tree.locate_in_envelope_intersecting(&envelope) {
                let right_geometry =
                    right[candidate.index].expect("R-tree contains only non-null right geometries");
                if exact_match(left_geometry, right_geometry, predicate) {
                    pair_count
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                            current.checked_add(1).filter(|next| *next <= max_pairs)
                        })
                        .map_err(|_| SpatialJoinError::PairLimitExceeded { limit: max_pairs })?;
                    right_indexes.push(candidate.index);
                }
            }
            right_indexes.sort_unstable();

            let left = u64::try_from(left_index).map_err(|_| SpatialJoinError::IndexOverflow)?;
            right_indexes
                .into_iter()
                .map(|right_index| {
                    Ok(JoinPair {
                        left,
                        right: u64::try_from(right_index)
                            .map_err(|_| SpatialJoinError::IndexOverflow)?,
                    })
                })
                .collect()
        })
        .collect();

    Ok(grouped?.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, Point};
    use proptest::prelude::*;

    fn brute_force(
        left: &[Geometry<f64>],
        right: &[Geometry<f64>],
        predicate: JoinPredicate,
    ) -> Vec<JoinPair> {
        let mut pairs = Vec::new();
        for (left_index, left_geometry) in left.iter().enumerate() {
            for (right_index, right_geometry) in right.iter().enumerate() {
                if exact_match(left_geometry, right_geometry, predicate) {
                    pairs.push(JoinPair {
                        left: left_index as u64,
                        right: right_index as u64,
                    });
                }
            }
        }
        pairs
    }

    fn rectangle(spec: (i16, i16, u8, u8)) -> Geometry<f64> {
        let (x, y, width, height) = spec;
        let x = f64::from(x);
        let y = f64::from(y);
        let width = f64::from(width.max(1));
        let height = f64::from(height.max(1));
        Geometry::Polygon(polygon![
            (x: x, y: y), (x: x + width, y: y),
            (x: x + width, y: y + height), (x: x, y: y + height),
            (x: x, y: y),
        ])
    }

    #[test]
    fn join_is_exact_and_deterministically_ordered() {
        let left = vec![
            Geometry::Point(Point::new(2.0, 2.0)),
            Geometry::Point(Point::new(0.5, 0.5)),
        ];
        let right = vec![
            Geometry::Polygon(polygon![
                (x: 0.0, y: 0.0), (x: 1.0, y: 0.0),
                (x: 1.0, y: 1.0), (x: 0.0, y: 1.0),
                (x: 0.0, y: 0.0),
            ]),
            Geometry::LineString(line_string![(x: 0.0, y: 2.0), (x: 3.0, y: 2.0)]),
        ];
        assert_eq!(
            spatial_join(&left, &right, JoinPredicate::Intersects, 10).unwrap(),
            vec![
                JoinPair { left: 0, right: 1 },
                JoinPair { left: 1, right: 0 },
            ]
        );
    }

    #[test]
    fn contains_and_within_have_explicit_direction() {
        let area = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 2.0), (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ]);
        let point = Geometry::Point(Point::new(1.0, 1.0));
        assert_eq!(
            spatial_join(
                std::slice::from_ref(&area),
                std::slice::from_ref(&point),
                JoinPredicate::Contains,
                1,
            )
            .unwrap(),
            vec![JoinPair { left: 0, right: 0 }]
        );
        assert_eq!(
            spatial_join(&[point], &[area], JoinPredicate::Within, 1).unwrap(),
            vec![JoinPair { left: 0, right: 0 }]
        );
    }

    #[test]
    fn de9im_predicates_cover_full_manipola_sjoin_contract() {
        let first = rectangle((0, 0, 2, 2));
        let touching = rectangle((2, 0, 2, 2));
        let overlapping = rectangle((1, 1, 2, 2));
        assert!(exact_match(&first, &touching, JoinPredicate::Touches));
        assert!(exact_match(&first, &overlapping, JoinPredicate::Overlaps));

        let horizontal = Geometry::LineString(line_string![
            (x: -1.0, y: 0.0), (x: 1.0, y: 0.0)
        ]);
        let vertical = Geometry::LineString(line_string![
            (x: 0.0, y: -1.0), (x: 0.0, y: 1.0)
        ]);
        assert!(exact_match(&horizontal, &vertical, JoinPredicate::Crosses));
    }

    #[test]
    fn pair_limit_fails_closed() {
        let points = vec![Geometry::Point(Point::new(1.0, 1.0)); 4];
        assert!(matches!(
            spatial_join(&points, &points, JoinPredicate::Intersects, 3),
            Err(SpatialJoinError::PairLimitExceeded { limit: 3 })
        ));
    }

    #[test]
    fn pair_limit_is_enforced_inside_a_single_left_group() {
        let left = vec![Geometry::Point(Point::new(1.0, 1.0))];
        let right = vec![Geometry::Point(Point::new(1.0, 1.0)); 8];
        assert_eq!(
            spatial_join(&left, &right, JoinPredicate::Intersects, 8)
                .unwrap()
                .len(),
            8
        );
        assert!(matches!(
            spatial_join(&left, &right, JoinPredicate::Intersects, 7),
            Err(SpatialJoinError::PairLimitExceeded { limit: 7 })
        ));
    }

    #[test]
    fn rejects_non_finite_envelopes() {
        let invalid = Geometry::Point(Point::new(f64::NAN, 1.0));
        assert!(matches!(
            spatial_join(&[invalid], &[], JoinPredicate::Intersects, 1),
            Err(SpatialJoinError::NonFiniteCoordinate {
                side: "left",
                index: 0
            })
        ));
    }

    #[test]
    fn invalid_topology_and_empty_geometries_are_handled_on_both_sides() {
        let valid = Geometry::Point(Point::new(0.0, 0.0));
        let invalid = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
        ]);
        assert!(matches!(
            spatial_join(
                std::slice::from_ref(&invalid),
                std::slice::from_ref(&valid),
                JoinPredicate::Intersects,
                1,
            ),
            Err(SpatialJoinError::InvalidGeometry { side: "left", .. })
        ));
        assert!(matches!(
            spatial_join(
                std::slice::from_ref(&valid),
                std::slice::from_ref(&invalid),
                JoinPredicate::Intersects,
                1,
            ),
            Err(SpatialJoinError::InvalidGeometry { side: "right", .. })
        ));
        let empty = Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into());
        assert!(spatial_join(
            std::slice::from_ref(&empty),
            std::slice::from_ref(&valid),
            JoinPredicate::Intersects,
            1,
        )
        .unwrap()
        .is_empty());
        assert!(spatial_join(
            std::slice::from_ref(&valid),
            std::slice::from_ref(&empty),
            JoinPredicate::Intersects,
            1,
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn nullable_rows_preserve_original_indexes() {
        let left = vec![None, Some(Geometry::Point(Point::new(1.0, 1.0)))];
        let right = vec![
            None,
            Some(Geometry::Point(Point::new(50.0, 50.0))),
            Some(Geometry::Point(Point::new(1.0, 1.0))),
        ];
        assert_eq!(
            spatial_join_nullable(&left, &right, JoinPredicate::Intersects, 10).unwrap(),
            vec![JoinPair { left: 1, right: 2 }]
        );
    }

    proptest! {
        #[test]
        fn indexed_join_matches_exhaustive_reference(
            left_specs in prop::collection::vec(( -100_i16..100, -100_i16..100, 1_u8..20, 1_u8..20), 0..24),
            right_specs in prop::collection::vec(( -100_i16..100, -100_i16..100, 1_u8..20, 1_u8..20), 0..24),
        ) {
            let left: Vec<_> = left_specs.into_iter().map(rectangle).collect();
            let right: Vec<_> = right_specs.into_iter().map(rectangle).collect();
            for predicate in [
                JoinPredicate::Intersects,
                JoinPredicate::Contains,
                JoinPredicate::Within,
                JoinPredicate::Crosses,
                JoinPredicate::Overlaps,
                JoinPredicate::Touches,
            ] {
                let expected = brute_force(&left, &right, predicate);
                let actual = spatial_join(&left, &right, predicate, 10_000).unwrap();
                prop_assert_eq!(actual, expected);
            }
        }
    }
}
