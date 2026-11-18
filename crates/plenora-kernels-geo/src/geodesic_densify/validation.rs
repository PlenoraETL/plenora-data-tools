//! Adattamento dei budget GOP-007 alla validazione comune GOP-005.
use super::{GeodesicDensifyError, GeodesicGeometryDensifyLimits};
use crate::geodesic_distance::{GeodesicDistanceError, GeodesicDistanceLimits};
use crate::geodesic_surface::{
    self as surface,
    metric::{GeometryIndex, Work, ROUND_M},
};
use crate::geodetica::EllissoideGeodetico;
use geo::Geometry;
use std::sync::atomic::AtomicBool;

const fn metric_limits(limits: GeodesicGeometryDensifyLimits) -> GeodesicDistanceLimits {
    GeodesicDistanceLimits {
        max_work: limits.max_validation_work,
        max_reserved_bytes: limits.output.max_reserved_bytes,
        deadline: limits.output.deadline,
    }
}

const fn densify_error(error: GeodesicDistanceError) -> GeodesicDensifyError {
    match error {
        GeodesicDistanceError::GeographicRequired => GeodesicDensifyError::GeographicRequired,
        GeodesicDistanceError::UnsupportedEllipsoid => GeodesicDensifyError::UnsupportedEllipsoid,
        GeodesicDistanceError::InvalidGeometry | GeodesicDistanceError::EmptyGeometry => {
            GeodesicDensifyError::InvalidGeometry
        }
        GeodesicDistanceError::UnsupportedGeometry => GeodesicDensifyError::UnsupportedGeometry,
        GeodesicDistanceError::AmbiguousGeodesic => GeodesicDensifyError::AmbiguousGeodesic,
        GeodesicDistanceError::AccuracyUnproven => GeodesicDensifyError::AccuracyUnproven,
        GeodesicDistanceError::UnsupportedSurface => GeodesicDensifyError::UnsupportedSurface,
        GeodesicDistanceError::WorkLimit => GeodesicDensifyError::WorkLimit,
        GeodesicDistanceError::MemoryLimit => GeodesicDensifyError::MemoryLimit,
        GeodesicDistanceError::Allocation => GeodesicDensifyError::Allocation,
        GeodesicDistanceError::Cancelled => GeodesicDensifyError::Cancelled,
        GeodesicDistanceError::DeadlineExceeded => GeodesicDensifyError::DeadlineExceeded,
    }
}

pub(super) fn validate(
    geometry: &Geometry,
    ellipsoid: &EllissoideGeodetico,
    limits: GeodesicGeometryDensifyLimits,
    cancel: Option<&AtomicBool>,
) -> Result<u64, GeodesicDensifyError> {
    validate_surface(geometry, ellipsoid, limits, cancel).map_err(densify_error)
}

fn validate_surface(
    geometry: &Geometry,
    ellipsoid: &EllissoideGeodetico,
    limits: GeodesicGeometryDensifyLimits,
    cancel: Option<&AtomicBool>,
) -> Result<u64, GeodesicDistanceError> {
    let mut work = Work {
        limits: metric_limits(limits),
        cancel,
        count: 0,
        bytes: 0,
        peak: 0,
        ellipsoid: *ellipsoid,
        taylor: true,
        position_margin: ROUND_M,
    };
    let mut index = GeometryIndex {
        nodes: Vec::new(),
        rings: Vec::new(),
        representatives: Vec::new(),
        root: 0,
    };
    let mut polygon = |polygon: &geo::Polygon| {
        work.step()?;
        if polygon.exterior().0.is_empty() {
            if polygon.interiors().is_empty() {
                return Ok(());
            }
            return Err(GeodesicDistanceError::InvalidGeometry);
        }
        index.polygon(polygon, &mut work)
    };
    match geometry {
        Geometry::Polygon(value) => polygon(value)?,
        Geometry::MultiPolygon(values) => {
            for value in values {
                polygon(value)?;
            }
        }
        _ => return Ok(0),
    }
    if !index.nodes.is_empty() {
        index.root = index.branch(0, index.nodes.len(), &mut work)?;
        surface::validate(&index, &mut work)?;
    }
    work.checkpoint()?;
    Ok(work.count)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::geodesic_densify::{geodesic_densify_geometry_with_limits, GeodesicDensifyLimits};
    use geo::{Coord, LineString, Polygon};
    use plenora_core::crs::resolve_crs;

    #[test]
    fn exact_live_memory_and_cumulative_validation_work() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        let ellipsoid = EllissoideGeodetico::da_crs(&crs).unwrap();
        let polygon = Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 1.0),
                (0.0, 1.0),
                (0.0, 0.0),
            ]),
            vec![],
        );
        let limits = GeodesicGeometryDensifyLimits {
            output: GeodesicDensifyLimits {
                max_points: 5,
                max_reserved_bytes: 1_000_000,
                deadline: None,
            },
            max_validation_work: 1_000_000,
        };
        let mut work = Work {
            limits: metric_limits(limits),
            cancel: None,
            count: 0,
            bytes: 0,
            peak: 0,
            ellipsoid,
            taylor: true,
            position_margin: ROUND_M,
        };
        let mut index = GeometryIndex {
            nodes: Vec::new(),
            rings: Vec::new(),
            representatives: Vec::new(),
            root: 0,
        };
        work.step().unwrap();
        index.polygon(&polygon, &mut work).unwrap();
        index.root = index.branch(0, index.nodes.len(), &mut work).unwrap();
        surface::validate(&index, &mut work).unwrap();
        // GOP-026/027: due validazioni identiche senza suddivisioni; al secondo
        // picco e' vivo anche l'output di cinque Coord, nessun Vec di buchi.
        let exact = GeodesicGeometryDensifyLimits {
            output: GeodesicDensifyLimits {
                max_reserved_bytes: work.peak + 5 * size_of::<Coord>() as u64,
                ..limits.output
            },
            max_validation_work: 2 * work.count,
        };
        let input = Geometry::Polygon(polygon);
        let run = |budget| geodesic_densify_geometry_with_limits(&input, &crs, 1e6, budget, None);
        assert_eq!(run(exact).unwrap(), input);
        assert_eq!(
            run(GeodesicGeometryDensifyLimits {
                max_validation_work: exact.max_validation_work - 1,
                ..exact
            }),
            Err(GeodesicDensifyError::WorkLimit)
        );
        assert_eq!(
            run(GeodesicGeometryDensifyLimits {
                output: GeodesicDensifyLimits {
                    max_reserved_bytes: exact.output.max_reserved_bytes - 1,
                    ..exact.output
                },
                ..exact
            }),
            Err(GeodesicDensifyError::MemoryLimit)
        );
    }

    fn generous_limits() -> GeodesicGeometryDensifyLimits {
        GeodesicGeometryDensifyLimits {
            output: GeodesicDensifyLimits {
                max_points: 100_000,
                max_reserved_bytes: 128_000_000,
                deadline: None,
            },
            max_validation_work: 20_000_000,
        }
    }

    #[test]
    fn independent_surface_fixtures_after_densify() {
        use crate::geodesic_distance::{geodesic_distance_with_limits, ABSOLUTE_ERROR_M};
        use crate::geodesic_surface::fixtures::fixture_geometry;
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../geodesic_distance/reference_cases.json"))
                .unwrap();
        let budget = generous_limits();
        let mut checked = 0;
        for fixture in fixtures.as_array().unwrap() {
            if !["a", "b"].iter().any(|side| {
                matches!(
                    fixture[side]["kind"].as_str(),
                    Some("polygon" | "multi_polygon")
                )
            }) {
                continue;
            }
            checked += 1;
            for reverse in [false, true] {
                let densify = |side| {
                    let input = fixture_geometry(&fixture[side], reverse);
                    if matches!(input, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
                        let large = fixture["name"].as_str().unwrap().ends_with("_1.0");
                        geodesic_densify_geometry_with_limits(
                            &input,
                            &crs,
                            if large { 10_000.0 } else { 100.0 },
                            budget,
                            None,
                        )
                        .unwrap()
                    } else {
                        input
                    }
                };
                let a = densify("a");
                let b = densify("b");
                let lower = fixture["bracket"][0].as_f64().unwrap();
                let upper = fixture["bracket"][1].as_f64().unwrap();
                for (left, right) in [(&a, &b), (&b, &a)] {
                    let result = geodesic_distance_with_limits(
                        left,
                        right,
                        &crs,
                        metric_limits(budget),
                        None,
                    )
                    .unwrap();
                    assert!(
                        result.metres >= lower - ABSOLUTE_ERROR_M
                            && result.metres <= upper + ABSOLUTE_ERROR_M
                    );
                    if upper == 0.0 {
                        assert_eq!(result.metres, 0.0);
                    }
                }
            }
        }
        assert_eq!(checked, 25);
    }

    fn rectangle(x: f64, y: f64, width: f64) -> LineString {
        LineString::from(vec![
            (x, y),
            (x + width, y),
            (x + width, y + width),
            (x, y + width),
            (x, y),
        ])
    }

    #[test]
    fn small_holes_and_components_near_boundary() {
        use geo::MultiPolygon;
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        for gap in [0.00001, 0.0001, 0.001] {
            for reverse in [false, true] {
                let oriented = |mut ring: LineString| {
                    if reverse {
                        ring.0.reverse();
                    }
                    ring
                };
                let shell = Polygon::new(
                    oriented(rectangle(0.0, 0.0, 0.01)),
                    vec![
                        oriented(rectangle(gap, gap, 0.0001)),
                        oriented(rectangle(0.005, gap, 0.0001)),
                    ],
                );
                let other = Polygon::new(oriented(rectangle(0.01 + gap, 0.0, 0.0001)), vec![]);
                let input = Geometry::MultiPolygon(MultiPolygon(vec![shell.clone(), other]));
                let output = geodesic_densify_geometry_with_limits(
                    &input,
                    &crs,
                    100.0,
                    generous_limits(),
                    None,
                )
                .unwrap();
                let Geometry::MultiPolygon(parts) = output else {
                    panic!("unexpected geometry type")
                };
                assert_eq!(parts.0.len(), 2);
                assert_eq!(parts.0[0].interiors().len(), 2);
                let outside_hole = Polygon::new(
                    shell.exterior().clone(),
                    vec![oriented(rectangle(-gap - 0.0001, 0.0, 0.0001))],
                );
                assert_eq!(
                    geodesic_densify_geometry_with_limits(
                        &outside_hole.into(),
                        &crs,
                        100.0,
                        generous_limits(),
                        None
                    ),
                    Err(GeodesicDensifyError::InvalidGeometry)
                );
                let overlap = Polygon::new(oriented(rectangle(gap, 0.005, 0.0001)), vec![]);
                assert_eq!(
                    geodesic_densify_geometry_with_limits(
                        &MultiPolygon(vec![shell, overlap]).into(),
                        &crs,
                        100.0,
                        generous_limits(),
                        None
                    ),
                    Err(GeodesicDensifyError::UnsupportedSurface)
                );
            }
        }
    }

    #[test]
    fn repeated_vertices_and_ill_conditioned_long_edge() {
        let crs = resolve_crs("EPSG:4326", "crs").unwrap();
        let mut ring = rectangle(0.0, 0.0, 0.01);
        ring.0.insert(1, ring.0[0]);
        let input: Geometry = Polygon::new(ring, vec![]).into();
        assert_eq!(
            geodesic_densify_geometry_with_limits(&input, &crs, 1e6, generous_limits(), None)
                .unwrap(),
            input
        );
        let input: Geometry = Polygon::new(
            LineString::from(vec![(0.0, 0.0), (179.396, 0.0), (90.0, 10.0), (0.0, 0.0)]),
            vec![],
        )
        .into();
        assert_eq!(
            geodesic_densify_geometry_with_limits(&input, &crs, 1e8, generous_limits(), None),
            Err(GeodesicDensifyError::AccuracyUnproven)
        );
    }
}
