//! Oracoli geometrici, compatibilita' e soglie del costruttore.
use super::*;
use geo::{CoordsIter, Geometry, MultiPoint, Point};
use proptest::prelude::*;
use std::sync::atomic::AtomicBool;

fn bits(polygons: &[Polygon<f64>]) -> Vec<Vec<(u64, u64)>> {
    polygons
        .iter()
        .map(|p| {
            p.coords_iter()
                .map(|c| (c.x.to_bits(), c.y.to_bits()))
                .collect()
        })
        .collect()
}

fn compare(points: &[Coord<f64>]) {
    let geometry = Geometry::MultiPoint(MultiPoint(points.iter().copied().map(Point).collect()));
    let old = crate::extended_algorithms::delaunay(&geometry, u64::MAX, u64::MAX).unwrap();
    let new = delaunay_points_with_limits(points, u64::MAX, RelateLimits::default(), None).unwrap();
    assert_eq!(bits(&new), bits(&old));
}

/// La validazione del motore razionale ha un contratto diverso da quella
/// storica: non puo' sostituirla in un adapter Delaunay compatibile.
#[test]
fn generic_validation_cannot_use_rational_relate_as_a_gate() {
    let geometries = [
        Geometry::Polygon(Polygon::new(
            LineString::from(vec![(0., 0.), (4., 0.), (4., 4.), (0., 4.), (0., 0.)]),
            vec![LineString::new(Vec::new())],
        )),
        Geometry::Polygon(Polygon::new(
            LineString::new(Vec::new()),
            vec![LineString::from(vec![
                (0., 0.),
                (4., 0.),
                (4., 4.),
                (0., 4.),
                (0., 0.),
            ])],
        )),
    ];
    let empty = Geometry::GeometryCollection(geo::GeometryCollection::default());
    for geometry in geometries {
        let old = crate::extended_algorithms::delaunay(&geometry, u64::MAX, u64::MAX).unwrap();
        assert!(!old.is_empty());
        let points: Vec<_> = geometry.coords_iter().collect();
        let bounded =
            delaunay_points_with_limits(&points, u64::MAX, RelateLimits::default(), None).unwrap();
        assert_eq!(bits(&old), bits(&bounded));
        assert!(matches!(
            crate::predicates::arrangement_indexed::relate(
                &geometry,
                &empty,
                &mut Work::new(RelateLimits::default(), None),
            ),
            Err(PredicateError::InvalidGeometry { .. })
        ));
    }
}

proptest! {
    #[test]
    fn bounded_order_and_bits(points in prop::collection::vec((-20i32..20, -20i32..20, any::<bool>()), 0..200)) {
        let points: Vec<_> = points.into_iter().map(|(x,y,negative)| Coord {
            x: if x == 0 && negative { -0.0 } else { f64::from(x) },
            y: if y == 0 && negative { -0.0 } else { f64::from(y) },
        }).collect();
        compare(&points);
    }
}

#[test]
fn bounded_degenerate_and_extreme() {
    let points: Vec<_> = [
        (0., 0.),
        (-0., -0.),
        (1., 0.),
        (0., -0.),
        (2., 0.),
        (0., 1.),
        (-0., 0.),
    ]
    .into_iter()
    .map(|(x, y)| Coord { x, y })
    .collect();
    for end in 0..=points.len() {
        compare(&points[..end]);
    }
    for scale in [2_f64.powi(-140), 1., 2_f64.powi(195)] {
        let points: Vec<_> = [(0., 0.), (1., 0.), (1., 1.), (0., 1.)]
            .into_iter()
            .map(|(x, y)| Coord {
                x: x * scale,
                y: y * scale,
            })
            .collect();
        compare(&points);
    }
    for x in [f64::NAN, f64::INFINITY, f64::MAX, f64::MIN_POSITIVE] {
        assert!(matches!(
            delaunay_points_with_limits(&[Coord { x, y: 0. }], 10, RelateLimits::default(), None),
            Err(DelaunayBuildError::Triangulation(_))
        ));
    }
}

fn grid(side: u32) -> Vec<Coord<f64>> {
    (0..side)
        .flat_map(|x| {
            (0..side).map(move |y| Coord {
                x: f64::from(x),
                y: f64::from(y),
            })
        })
        .collect()
}

#[test]
fn bounded_scale_and_exact_limits() {
    for side in [16, 32, 64] {
        let points = grid(side);
        let mut work = Work::new(RelateLimits::default(), None);
        let output = build(&points, u64::MAX, &mut work).unwrap();
        assert_eq!(
            output.len(),
            2 * usize::try_from((side - 1) * (side - 1)).unwrap()
        );
        assert!(work.peak_bytes() < 8_000_000);
        assert!(work.steps < 2_000_000);
        compare(&points);
    }
    let points = grid(8);
    let mut measured = Work::new(RelateLimits::default(), None);
    let output = build(&points, u64::MAX, &mut measured).unwrap();
    let exact = RelateLimits {
        max_steps: measured.steps,
        max_nodes: measured.nodes,
        max_reserved_bytes: measured.peak_bytes(),
        ..RelateLimits::default()
    };
    for plus in [0, 1] {
        let limits = RelateLimits {
            max_steps: exact.max_steps + plus,
            max_nodes: exact.max_nodes + plus,
            max_reserved_bytes: exact.max_reserved_bytes + plus,
            ..exact
        };
        assert_eq!(
            bits(&delaunay_points_with_limits(&points, u64::MAX, limits, None).unwrap()),
            bits(&output)
        );
    }
    for limits in [
        RelateLimits {
            max_steps: exact.max_steps - 1,
            ..exact
        },
        RelateLimits {
            max_nodes: exact.max_nodes - 1,
            ..exact
        },
    ] {
        assert!(matches!(
            delaunay_points_with_limits(&points, u64::MAX, limits, None),
            Err(DelaunayBuildError::Resource(PredicateError::WorkLimit))
        ));
    }
    assert!(matches!(
        delaunay_points_with_limits(
            &points,
            u64::MAX,
            RelateLimits {
                max_reserved_bytes: exact.max_reserved_bytes - 1,
                ..exact
            },
            None
        ),
        Err(DelaunayBuildError::Resource(
            PredicateError::ReservationLimit
        ))
    ));
    assert!(matches!(
        delaunay_points_with_limits(&points, 0, RelateLimits::default(), None),
        Err(DelaunayBuildError::TriangleLimit)
    ));
}

#[test]
fn bounded_midpoint_and_empty_interruption() {
    let points = grid(16);
    let mut measured = Work::new(RelateLimits::default(), None);
    drop(build(&points, u64::MAX, &mut measured).unwrap());
    for at in [1, measured.steps / 2, measured.steps - 1] {
        for timeout in [false, true] {
            let flag = AtomicBool::new(false);
            let mut work = Work::new(RelateLimits::default(), Some(&flag));
            work.interrupt_at = Some((at, timeout));
            let result = build(&points, u64::MAX, &mut work);
            if timeout {
                assert!(matches!(
                    result,
                    Err(DelaunayBuildError::Resource(
                        PredicateError::DeadlineExceeded
                    ))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(DelaunayBuildError::Resource(PredicateError::Cancelled))
                ));
            }
            assert_eq!(work.steps, at);
        }
    }
    let flag = AtomicBool::new(true);
    assert!(matches!(
        delaunay_points_with_limits(&[], 0, RelateLimits::default(), Some(&flag)),
        Err(DelaunayBuildError::Resource(PredicateError::Cancelled))
    ));
    assert!(matches!(
        delaunay_points_with_limits(
            &[],
            0,
            RelateLimits {
                deadline: Some(std::time::Instant::now()),
                ..RelateLimits::default()
            },
            Some(&flag)
        ),
        Err(DelaunayBuildError::Resource(
            PredicateError::DeadlineExceeded
        ))
    ));
}

/// Cross binary64 negativo, determinante esatto positivo: `Triangle::new`
/// della baseline inverte la terna. La scelta resta osservabile nei bit.
#[test]
fn bounded_preserves_triangle_constructor_order() {
    compare(&[
        Coord {
            x: 4_747_956_090.001_08,
            y: 7_669_553_332.959_507,
        },
        Coord {
            x: 4_294_562_409.486_185,
            y: -5_466_630_960.501_853,
        },
        Coord {
            x: 4_079_497_574.017_294,
            y: -11_697_708_796.774_19,
        },
    ]);
}
