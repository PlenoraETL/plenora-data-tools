//! Polygonal boolean kernels. Non-polygon inputs are rejected explicitly
//! until a backend with full GEOS-compatible dimensional semantics is wired.

use geo::algorithm::bool_ops::unary_union;
use geo::algorithm::validation::Validation;
use geo::{BooleanOps, Buffer, CoordsIter, Geometry, MultiPolygon};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOperation {
    Intersection,
    Union,
    Difference,
    SymmetricDifference,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    Intersection,
    Union,
    Identity,
    SymmetricDifference,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverlayPiece {
    pub geometry: Geometry<f64>,
    pub left: Option<u64>,
    pub right: Option<u64>,
}

#[derive(Debug, Error)]
pub enum TopologyError {
    #[error("operazione topologica supportata solo per Polygon/MultiPolygon, ricevuto {0}")]
    UnsupportedGeometry(&'static str),
    #[error("geometria topologica non valida: {0}")]
    InvalidGeometry(String),
    #[error("parametro {name} non valido: {reason}")]
    InvalidParameter {
        name: &'static str,
        reason: &'static str,
    },
    #[error("limite {name} superato: valore={actual}, limite={limit}")]
    ResourceLimit {
        name: &'static str,
        actual: u64,
        limit: u64,
    },
    #[error("indice non rappresentabile come uint64")]
    IndexOverflow,
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

fn as_multi_polygon(geometry: &Geometry<f64>) -> Result<MultiPolygon<f64>, TopologyError> {
    geometry
        .check_validation()
        .map_err(|error| TopologyError::InvalidGeometry(error.to_string()))?;
    match geometry {
        Geometry::Polygon(polygon) => Ok(MultiPolygon::new(vec![polygon.clone()])),
        Geometry::MultiPolygon(polygons) => Ok(polygons.clone()),
        value => Err(TopologyError::UnsupportedGeometry(geometry_name(value))),
    }
}

fn checked_result(result: MultiPolygon<f64>) -> Result<Geometry<f64>, TopologyError> {
    result
        .check_validation()
        .map_err(|error| TopologyError::InvalidGeometry(error.to_string()))?;
    Ok(Geometry::MultiPolygon(result))
}

/// Applies a polygonal boolean operation to two inputs.
///
/// # Errors
///
/// - `UnsupportedGeometry`: an input is not Polygon/MultiPolygon.
/// - `InvalidGeometry`: an input fails OGC validation, or the result is not
///   valid.
pub fn boolean_operation(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: BooleanOperation,
) -> Result<Geometry<f64>, TopologyError> {
    let left = as_multi_polygon(left)?;
    let right = as_multi_polygon(right)?;
    let result = match operation {
        BooleanOperation::Intersection => left.intersection(&right),
        BooleanOperation::Union => left.union(&right),
        BooleanOperation::Difference => left.difference(&right),
        BooleanOperation::SymmetricDifference => left.xor(&right),
    };
    checked_result(result)
}

/// Efficient polygonal dissolve/unary union. Grouping and attribute
/// aggregation remain a transport/adapter concern.
///
/// # Errors
///
/// - `UnsupportedGeometry`: an input is not Polygon/MultiPolygon.
/// - `InvalidGeometry`: an input fails OGC validation, or the dissolved
///   result is not valid.
pub fn dissolve(geometries: &[Geometry<f64>]) -> Result<Geometry<f64>, TopologyError> {
    let polygons: Vec<MultiPolygon<f64>> = geometries
        .iter()
        .map(as_multi_polygon)
        .collect::<Result<_, _>>()?;
    checked_result(unary_union(&polygons))
}

fn is_empty(geometry: &Geometry<f64>) -> bool {
    geometry.coords_count() == 0
}

/// Clips each polygonal input row to the dissolved polygonal mask. Empty
/// results become `None`, preserving the input row position for the adapter.
///
/// # Errors
///
/// Propagates the errors of [`dissolve`] (mask) and [`boolean_operation`]
/// (per-row intersection): non-polygonal or invalid inputs, invalid results.
pub fn clip_to_mask(
    geometries: &[Geometry<f64>],
    masks: &[Geometry<f64>],
) -> Result<Vec<Option<Geometry<f64>>>, TopologyError> {
    if masks.is_empty() {
        return Ok(vec![None; geometries.len()]);
    }
    let mask = dissolve(masks)?;
    geometries
        .iter()
        .map(|geometry| {
            let clipped = boolean_operation(geometry, &mask, BooleanOperation::Intersection)?;
            Ok((!is_empty(&clipped)).then_some(clipped))
        })
        .collect()
}

fn push_piece(
    pieces: &mut Vec<OverlayPiece>,
    geometry: Geometry<f64>,
    left: Option<usize>,
    right: Option<usize>,
    max_results: u64,
) -> Result<(), TopologyError> {
    if is_empty(&geometry) {
        return Ok(());
    }
    if u64::try_from(pieces.len()).map_err(|_| TopologyError::IndexOverflow)? >= max_results {
        return Err(TopologyError::ResourceLimit {
            name: "overlay_results",
            actual: u64::try_from(pieces.len())
                .map_err(|_| TopologyError::IndexOverflow)?
                .saturating_add(1),
            limit: max_results,
        });
    }
    pieces.push(OverlayPiece {
        geometry,
        left: left
            .map(u64::try_from)
            .transpose()
            .map_err(|_| TopologyError::IndexOverflow)?,
        right: right
            .map(u64::try_from)
            .transpose()
            .map_err(|_| TopologyError::IndexOverflow)?,
    });
    Ok(())
}

/// Polygonal overlay with explicit attribute lineage.
///
/// Boundary-only line/point intersections are intentionally excluded;
/// enabling `keep_geom_type=false` requires the GEOS backend and must not
/// silently use this kernel.
///
/// # Errors
///
/// - `InvalidParameter`: `max_candidate_pairs` or `max_results` is zero.
/// - `UnsupportedGeometry`: an input is not Polygon/MultiPolygon.
/// - `InvalidGeometry`: an input fails OGC validation, the candidate-pair
///   join fails, or a produced piece is not valid.
/// - `ResourceLimit`: the pieces exceed `max_results`.
/// - `IndexOverflow`: an index is not representable as `u64`/`usize`.
pub fn polygon_overlay(
    left: &[Geometry<f64>],
    right: &[Geometry<f64>],
    mode: OverlayMode,
    max_candidate_pairs: u64,
    max_results: u64,
) -> Result<Vec<OverlayPiece>, TopologyError> {
    if max_candidate_pairs == 0 || max_results == 0 {
        return Err(TopologyError::InvalidParameter {
            name: "overlay_limits",
            reason: "devono essere maggiori di zero",
        });
    }
    for geometry in left.iter().chain(right) {
        as_multi_polygon(geometry)?;
    }
    let pairs = crate::spatial_join::spatial_join(
        left,
        right,
        crate::spatial_join::JoinPredicate::Intersects,
        max_candidate_pairs,
    )
    .map_err(|error| TopologyError::InvalidGeometry(error.to_string()))?;
    let mut pieces = Vec::new();

    if matches!(
        mode,
        OverlayMode::Intersection | OverlayMode::Union | OverlayMode::Identity
    ) {
        for pair in pairs {
            let left_index =
                usize::try_from(pair.left).map_err(|_| TopologyError::IndexOverflow)?;
            let right_index =
                usize::try_from(pair.right).map_err(|_| TopologyError::IndexOverflow)?;
            let geometry = boolean_operation(
                &left[left_index],
                &right[right_index],
                BooleanOperation::Intersection,
            )?;
            push_piece(
                &mut pieces,
                geometry,
                Some(left_index),
                Some(right_index),
                max_results,
            )?;
        }
    }

    if matches!(
        mode,
        OverlayMode::Union | OverlayMode::Identity | OverlayMode::SymmetricDifference
    ) {
        let right_mask = (!right.is_empty()).then(|| dissolve(right)).transpose()?;
        for (index, geometry) in left.iter().enumerate() {
            let remainder = match &right_mask {
                Some(mask) => boolean_operation(geometry, mask, BooleanOperation::Difference)?,
                None => geometry.clone(),
            };
            push_piece(&mut pieces, remainder, Some(index), None, max_results)?;
        }
    }

    if matches!(mode, OverlayMode::Union | OverlayMode::SymmetricDifference) {
        let left_mask = (!left.is_empty()).then(|| dissolve(left)).transpose()?;
        for (index, geometry) in right.iter().enumerate() {
            let remainder = match &left_mask {
                Some(mask) => boolean_operation(geometry, mask, BooleanOperation::Difference)?,
                None => geometry.clone(),
            };
            push_piece(&mut pieces, remainder, None, Some(index), max_results)?;
        }
    }
    Ok(pieces)
}

/// Ordered topology cleanup for inputs that are already valid polygons.
///
/// Applies the same gap-closing morphology and first-row-wins overlap
/// policy as Manipola. Invalid inputs are rejected; repair belongs to GEOS
/// make-valid.
///
/// # Errors
///
/// - `InvalidParameter`: `snap_tolerance` is not finite or is negative.
/// - `ResourceLimit`: the input exceeds `max_geometries` or `max_vertices`.
/// - `UnsupportedGeometry`: an input is not Polygon/MultiPolygon.
/// - `InvalidGeometry`: an input fails OGC validation, or a morphology or
///   overlap-removal step produces an invalid geometry.
/// - `IndexOverflow`: a count is not representable as `u64`.
pub fn clean_valid_polygon_topology(
    geometries: &[Geometry<f64>],
    snap_tolerance: f64,
    remove_overlaps: bool,
    fill_gaps: bool,
    max_geometries: u64,
    max_vertices: u64,
) -> Result<Vec<Option<Geometry<f64>>>, TopologyError> {
    if !snap_tolerance.is_finite() || snap_tolerance < 0.0 {
        return Err(TopologyError::InvalidParameter {
            name: "snap_tolerance",
            reason: "deve essere finita e non negativa",
        });
    }
    let geometry_count =
        u64::try_from(geometries.len()).map_err(|_| TopologyError::IndexOverflow)?;
    if geometry_count > max_geometries {
        return Err(TopologyError::ResourceLimit {
            name: "geometries",
            actual: geometry_count,
            limit: max_geometries,
        });
    }
    let mut vertices = 0_u64;
    for geometry in geometries {
        as_multi_polygon(geometry)?;
        vertices = vertices
            .checked_add(
                u64::try_from(geometry.coords_count()).map_err(|_| TopologyError::IndexOverflow)?,
            )
            .ok_or(TopologyError::IndexOverflow)?;
    }
    if vertices > max_vertices {
        return Err(TopologyError::ResourceLimit {
            name: "vertices",
            actual: vertices,
            limit: max_vertices,
        });
    }

    let mut working: Vec<Option<Geometry<f64>>> = geometries.iter().cloned().map(Some).collect();
    if fill_gaps && snap_tolerance > 0.0 {
        for geometry in working.iter_mut().flatten() {
            let expanded = Geometry::MultiPolygon(geometry.buffer(snap_tolerance));
            let closed = Geometry::MultiPolygon(expanded.buffer(-snap_tolerance));
            *geometry = checked_result(as_multi_polygon(&closed)?)?;
        }
    }
    if remove_overlaps {
        let mut accumulated: Option<Geometry<f64>> = None;
        for geometry in &mut working {
            let Some(current) = geometry.take() else {
                continue;
            };
            let remainder = match &accumulated {
                Some(previous) => {
                    boolean_operation(&current, previous, BooleanOperation::Difference)?
                }
                None => current,
            };
            if is_empty(&remainder) {
                continue;
            }
            accumulated = Some(match accumulated {
                Some(previous) => {
                    boolean_operation(&previous, &remainder, BooleanOperation::Union)?
                }
                None => remainder.clone(),
            });
            *geometry = Some(remainder);
        }
    }
    Ok(working)
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use geo::{
        line_string, polygon, Area, GeometryCollection, Line, MultiLineString, MultiPoint, Point,
        Rect, Triangle,
    };
    use proptest::prelude::*;

    fn square(x: f64, y: f64, size: f64) -> Geometry<f64> {
        Geometry::Polygon(polygon![
            (x: x, y: y), (x: x + size, y: y),
            (x: x + size, y: y + size), (x: x, y: y + size),
            (x: x, y: y),
        ])
    }

    #[test]
    fn boolean_areas_match_known_overlapping_squares() {
        let left = square(0.0, 0.0, 2.0);
        let right = square(1.0, 0.0, 2.0);
        let cases = [
            (BooleanOperation::Intersection, 2.0),
            (BooleanOperation::Union, 6.0),
            (BooleanOperation::Difference, 2.0),
            (BooleanOperation::SymmetricDifference, 4.0),
        ];
        for (operation, expected_area) in cases {
            let result = boolean_operation(&left, &right, operation).unwrap();
            assert_eq!(result.unsigned_area(), expected_area);
        }
    }

    #[test]
    fn dissolve_merges_overlaps_and_rejects_non_polygons() {
        let result = dissolve(&[square(0.0, 0.0, 2.0), square(1.0, 0.0, 2.0)]).unwrap();
        assert_eq!(result.unsigned_area(), 6.0);
        assert!(matches!(
            dissolve(&[Geometry::Point(Point::new(0.0, 0.0))]),
            Err(TopologyError::UnsupportedGeometry("Point"))
        ));
    }

    #[test]
    fn clip_preserves_rows_and_marks_empty_results() {
        let inputs = vec![square(0.0, 0.0, 2.0), square(10.0, 10.0, 1.0)];
        let result = clip_to_mask(&inputs, &[square(1.0, 0.0, 2.0)]).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].as_ref().unwrap().unsigned_area(), 2.0);
        assert!(result[1].is_none());
    }

    #[test]
    fn overlay_lineage_and_areas_are_complete_and_deterministic() {
        let left = vec![square(0.0, 0.0, 2.0)];
        let right = vec![square(1.0, 0.0, 2.0)];
        let pieces = polygon_overlay(&left, &right, OverlayMode::Union, 10, 10).unwrap();
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].left, Some(0));
        assert_eq!(pieces[0].right, Some(0));
        assert_eq!(pieces[1].left, Some(0));
        assert_eq!(pieces[1].right, None);
        assert_eq!(pieces[2].left, None);
        assert_eq!(pieces[2].right, Some(0));
        assert_eq!(
            pieces
                .iter()
                .map(|piece| piece.geometry.unsigned_area())
                .sum::<f64>(),
            6.0
        );

        assert!(matches!(
            polygon_overlay(&left, &right, OverlayMode::Union, 10, 2),
            Err(TopologyError::ResourceLimit {
                name: "overlay_results",
                ..
            })
        ));
    }

    #[test]
    fn clean_topology_removes_overlap_with_first_row_wins() {
        let cleaned = clean_valid_polygon_topology(
            &[square(0.0, 0.0, 2.0), square(1.0, 0.0, 2.0)],
            0.0,
            true,
            false,
            10,
            100,
        )
        .unwrap();
        assert_eq!(cleaned[0].as_ref().unwrap().unsigned_area(), 4.0);
        assert_eq!(cleaned[1].as_ref().unwrap().unsigned_area(), 2.0);
        assert_eq!(
            boolean_operation(
                cleaned[0].as_ref().unwrap(),
                cleaned[1].as_ref().unwrap(),
                BooleanOperation::Intersection,
            )
            .unwrap()
            .unsigned_area(),
            0.0
        );
        assert!(matches!(
            clean_valid_polygon_topology(&[square(0.0, 0.0, 2.0)], 0.0, true, false, 0, 100),
            Err(TopologyError::ResourceLimit {
                name: "geometries",
                ..
            })
        ));
    }

    #[test]
    fn every_unsupported_geometry_family_is_rejected_explicitly() {
        let values = [
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Line(Line::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]),
            Geometry::MultiPoint(MultiPoint::new(vec![Point::new(0.0, 0.0)])),
            Geometry::MultiLineString(MultiLineString::new(vec![line_string![
                (x: 0.0, y: 0.0), (x: 1.0, y: 1.0)
            ]])),
            Geometry::GeometryCollection(GeometryCollection(vec![])),
            Geometry::Rect(Rect::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::Triangle(Triangle::new(
                (0.0, 0.0).into(),
                (1.0, 0.0).into(),
                (0.0, 1.0).into(),
            )),
        ];
        let names = [
            "Point",
            "Line",
            "LineString",
            "MultiPoint",
            "MultiLineString",
            "GeometryCollection",
            "Rect",
            "Triangle",
        ];
        for (value, expected) in values.iter().zip(names) {
            assert!(matches!(
                dissolve(std::slice::from_ref(value)),
                Err(TopologyError::UnsupportedGeometry(actual)) if actual == expected
            ));
        }
    }

    #[test]
    fn clip_overlay_and_cleanup_cover_empty_and_limit_boundaries() {
        let inputs = vec![square(0.0, 0.0, 1.0), square(3.0, 3.0, 1.0)];
        assert_eq!(clip_to_mask(&inputs, &[]).unwrap(), vec![None, None]);

        for mode in [
            OverlayMode::Intersection,
            OverlayMode::Identity,
            OverlayMode::SymmetricDifference,
        ] {
            let pieces = polygon_overlay(&inputs[..1], &inputs[1..], mode, 10, 10).unwrap();
            match mode {
                OverlayMode::Intersection => assert!(pieces.is_empty()),
                OverlayMode::Identity => assert_eq!(pieces.len(), 1),
                OverlayMode::SymmetricDifference => assert_eq!(pieces.len(), 2),
                OverlayMode::Union => unreachable!(),
            }
        }
        assert_eq!(
            polygon_overlay(&inputs[..1], &[], OverlayMode::Union, 10, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            polygon_overlay(&[], &inputs[..1], OverlayMode::Union, 10, 10)
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            polygon_overlay(&inputs, &inputs, OverlayMode::Union, 0, 10),
            Err(TopologyError::InvalidParameter { .. })
        ));
        assert!(matches!(
            polygon_overlay(&inputs, &inputs, OverlayMode::Union, 10, 0),
            Err(TopologyError::InvalidParameter { .. })
        ));

        assert!(matches!(
            clean_valid_polygon_topology(&inputs, f64::NAN, false, false, 10, 100),
            Err(TopologyError::InvalidParameter { .. })
        ));
        assert!(matches!(
            clean_valid_polygon_topology(&inputs, -1.0, false, false, 10, 100),
            Err(TopologyError::InvalidParameter { .. })
        ));
        assert!(matches!(
            clean_valid_polygon_topology(&inputs, 0.0, false, false, 10, 5),
            Err(TopologyError::ResourceLimit {
                name: "vertices",
                ..
            })
        ));
        let closed = clean_valid_polygon_topology(
            &[square(0.0, 0.0, 1.0), square(1.05, 0.0, 1.0)],
            0.1,
            true,
            true,
            10,
            100,
        )
        .unwrap();
        assert_eq!(closed.len(), 2);
        assert!(closed.iter().all(Option::is_some));

        let swallowed = clean_valid_polygon_topology(
            &[square(0.0, 0.0, 4.0), square(1.0, 1.0, 1.0)],
            0.0,
            true,
            false,
            10,
            100,
        )
        .unwrap();
        assert!(swallowed[0].is_some());
        assert!(swallowed[1].is_none());
    }

    proptest! {
        #[test]
        fn boolean_area_identities_hold_for_generated_rectangles(
            ax in -100_i16..100,
            ay in -100_i16..100,
            aw in 1_u8..30,
            ah in 1_u8..30,
            bx in -100_i16..100,
            by in -100_i16..100,
            bw in 1_u8..30,
            bh in 1_u8..30,
        ) {
            let left = square(f64::from(ax), f64::from(ay), f64::from(aw.min(ah)));
            let right = square(f64::from(bx), f64::from(by), f64::from(bw.min(bh)));
            let left_area = left.unsigned_area();
            let right_area = right.unsigned_area();
            let intersection = boolean_operation(
                &left, &right, BooleanOperation::Intersection
            ).unwrap().unsigned_area();
            let union = boolean_operation(
                &left, &right, BooleanOperation::Union
            ).unwrap().unsigned_area();
            let difference = boolean_operation(
                &left, &right, BooleanOperation::Difference
            ).unwrap().unsigned_area();
            let xor = boolean_operation(
                &left, &right, BooleanOperation::SymmetricDifference
            ).unwrap().unsigned_area();
            prop_assert!((left_area + right_area - union - intersection).abs() < 1e-9);
            prop_assert!((left_area - difference - intersection).abs() < 1e-9);
            prop_assert!((xor - (union - intersection)).abs() < 1e-9);
        }
    }
}
