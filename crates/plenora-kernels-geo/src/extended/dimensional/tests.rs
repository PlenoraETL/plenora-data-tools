//! C1: estremi della retta y=2x+1 e determinante intero indipendente.

use super::*;

#[test]
fn legacy_metric_extreme_domain_is_rejected_explicitly() {
    for scale in [1e200, 1e-200] {
        let input =
            geo::MultiPoint::from(vec![(0.0, 0.0), (scale, 0.0), (scale / 2.0, 1.0)]).into();
        assert!(matches!(
            concave_hull_dimensional_with_limits(
                &input,
                2.0,
                0.0,
                0.125,
                3,
                RelateLimits::default(),
                None,
            ),
            Err(DimensionalHullError::Metric(
                crate::operations::OperationError::PrecisionInsufficient
            ))
        ));
    }
}

#[test]
fn legacy_metric_large_finite_cancellation_is_rejected() {
    // C1: deviazione 8192/sqrt(2) > 5000, main perde oltre mille unita'.
    let input =
        geo::MultiPoint::from(vec![(0.0, 0.0), (1e20, 1e20), (5e19, 5e19_f64.next_up())]).into();
    assert!(matches!(
        concave_hull_dimensional_with_limits(
            &input,
            2.0,
            0.0,
            5000.0,
            3,
            RelateLimits::default(),
            None,
        ),
        Err(DimensionalHullError::Metric(
            crate::operations::OperationError::PrecisionInsufficient
        ))
    ));
}

#[test]
fn positive_threshold_inclusive_point_segment_and_polygon() {
    let run = |points: &[(f64, f64)], threshold| {
        concave_hull_dimensional_with_limits(
            &geo::MultiPoint::from(points.to_vec()).into(),
            2.0,
            0.0,
            threshold,
            1000,
            RelateLimits::default(),
            None,
        )
        .unwrap()
    };
    // Oracolo C1: distanze analitiche su asse X, +/- 1/8, senza radici.
    for sign in [-1.0, 1.0] {
        let points = [(0.0, 0.0), (4.0, 0.0), (2.0, sign * 0.125)];
        assert!(matches!(
            run(&points, 0.125_f64.next_down()),
            Geometry::Polygon(_)
        ));
        assert_eq!(
            run(&points, 0.125),
            LineString::from(vec![(0.0, 0.0), (4.0, 0.0)]).into()
        );
        assert_eq!(run(&points, 0.125_f64.next_up()), run(&points, 0.125));
        assert_eq!(
            run(&[points[2], points[1], points[0], points[2]], 0.125),
            run(&points, 0.125)
        );
    }
    assert_eq!(
        run(&[(0.0, 0.0), (0.125, 0.0)], 0.125),
        Point::new(0.0, 0.0).into()
    );
    assert!(matches!(
        run(&[(0.0, 0.0), (0.125, 0.0)], 0.125_f64.next_down()),
        Geometry::LineString(_)
    ));
    assert_eq!(run(&[], 0.125), Polygon::empty().into());
}

#[test]
fn counted_concaveman_matches_legacy_including_ties() {
    use geo::ConcaveHull as _;
    for seed in 0_i32..80 {
        let mut points = vec![
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 16.0, y: 0.0 },
            Coord { x: 16.0, y: 16.0 },
            Coord { x: 0.0, y: 16.0 },
        ];
        for i in 0_i32..(5 + seed % 31) {
            points.push(Coord {
                x: f64::from((seed * 13 + i * 7 + i * i) % 17),
                y: f64::from((seed * 11 + i * i + seed / 17) % 17),
            });
        }
        for concavity in [0.5, 1.0, 2.0, 8.0] {
            for length_threshold in [0.0, 4.0, 32.0] {
                let expected =
                    points.concave_hull_with_options(geo::concave_hull::ConcaveHullOptions {
                        concavity,
                        length_threshold,
                    });
                let mut work = Work::new(RelateLimits::default(), None);
                let actual = concaveman::run(
                    &mut points.clone(),
                    &geo::concave_hull::ConcaveHullOptions {
                        concavity,
                        length_threshold,
                    },
                    &mut work,
                )
                .unwrap();
                assert_eq!(actual, expected);
                assert!(work.steps > 0);
            }
        }
    }
}

#[test]
fn concaveman_work_memory_and_internal_interruption() {
    let points: Vec<Coord> = vec![
        (0.0, 0.0).into(),
        (4.0, 0.0).into(),
        (3.0, 2.0).into(),
        (4.0, 4.0).into(),
        (0.0, 4.0).into(),
    ];
    let run = |work: &mut Work<'_>| {
        concaveman::run(
            &mut points.clone(),
            &geo::concave_hull::ConcaveHullOptions {
                concavity: 1.0,
                length_threshold: 0.0,
            },
            work,
        )
    };
    let mut measured = Work::new(RelateLimits::default(), None);
    let expected = run(&mut measured).unwrap();
    let steps = measured.steps;
    for max_steps in [steps - 1, steps, steps + 1] {
        let mut work = Work::new(
            RelateLimits {
                max_steps,
                ..RelateLimits::default()
            },
            None,
        );
        let result = run(&mut work);
        if max_steps < steps {
            assert!(matches!(result, Err(PredicateError::WorkLimit)));
        } else {
            assert_eq!(result.unwrap(), expected);
        }
    }
    for max_reserved_bytes in [measured.peak_bytes() - 1, measured.peak_bytes()] {
        let mut work = Work::new(
            RelateLimits {
                max_reserved_bytes,
                ..RelateLimits::default()
            },
            None,
        );
        if max_reserved_bytes < measured.peak_bytes() {
            assert!(matches!(
                run(&mut work),
                Err(PredicateError::ReservationLimit)
            ));
        } else {
            assert_eq!(run(&mut work).unwrap(), expected);
        }
    }
    for deadline in [false, true] {
        let cancelled = AtomicBool::new(false);
        let mut work = Work::new(RelateLimits::default(), Some(&cancelled));
        work.interrupt_at = Some((steps / 2, deadline));
        let result = run(&mut work);
        if deadline {
            assert!(matches!(result, Err(PredicateError::DeadlineExceeded)));
        } else {
            assert!(matches!(result, Err(PredicateError::Cancelled)));
        }
    }
}

fn hull(points: &[(f64, f64)]) -> Geometry<f64> {
    concave_hull_dimensional_with_limits(
        &geo::MultiPoint::from(points.to_vec()).into(),
        2.0,
        0.0,
        0.0,
        1000,
        RelateLimits::default(),
        None,
    )
    .unwrap()
}

#[test]
fn empty_point_coincident_two_and_collinear() {
    assert_eq!(hull(&[]), Geometry::Polygon(Polygon::empty()));
    assert_eq!(hull(&[(3.0, 7.0)]), Point::new(3.0, 7.0).into());
    assert_eq!(hull(&[(3.0, 7.0); 4]), Point::new(3.0, 7.0).into());
    let expected: Geometry<f64> = LineString::from(vec![(-2.0, -3.0), (3.0, 7.0)]).into();
    assert_eq!(hull(&[(3.0, 7.0), (-2.0, -3.0)]), expected);
    assert_eq!(
        hull(&[(3.0, 7.0), (0.0, 1.0), (-2.0, -3.0), (0.0, 1.0)]),
        expected
    );
    assert_eq!(hull(&[(0.0, 1.0), (-2.0, -3.0), (3.0, 7.0)]), expected);
}

#[test]
fn extreme_and_subnormal_coordinates_are_exact() {
    for scale in [f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1)] {
        let expected: Geometry<f64> =
            LineString::from(vec![(-scale, -scale), (scale, scale)]).into();
        assert_eq!(
            hull(&[(scale, scale), (0.0, 0.0), (-scale, -scale)]),
            expected
        );
    }
    let point = hull(&[(-0.0, 0.0), (0.0, -0.0)]);
    let Geometry::Point(point) = point else {
        panic!("point expected")
    };
    assert_eq!(point.x().to_bits(), 0);
    assert_eq!(point.y().to_bits(), 0);
    assert_eq!(
        hull(&[(2.0, 3.0), (2.0, -3.0), (2.0, 1.0)]),
        LineString::from(vec![(2.0, -3.0), (2.0, 3.0)]).into()
    );
}

#[test]
fn empty_parts_consume_work_and_empty_input_observes_deadline() {
    let empty: Geometry<f64> = Geometry::GeometryCollection(geo::GeometryCollection::empty());
    let input = Geometry::GeometryCollection(geo::GeometryCollection(vec![empty.clone(); 100]));
    let run = |geometry, limits| {
        concave_hull_dimensional_with_limits(geometry, 2.0, 0.0, 0.0, 0, limits, None)
    };
    assert!(matches!(
        run(
            &input,
            RelateLimits {
                max_steps: 10,
                ..RelateLimits::default()
            }
        ),
        Err(DimensionalHullError::Predicate(PredicateError::WorkLimit))
    ));
    assert_eq!(
        run(&input, RelateLimits::default()).unwrap(),
        Polygon::empty().into()
    );
    assert!(matches!(
        run(
            &empty,
            RelateLimits {
                deadline: Some(std::time::Instant::now()),
                ..RelateLimits::default()
            }
        ),
        Err(DimensionalHullError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
}

#[test]
fn integer_determinant_oracle_and_polygon_compatibility() {
    for x in -4_i32..=4 {
        for y in -4_i32..=4 {
            let points = [(-5.0, -5.0), (5.0, 5.0), (f64::from(x), f64::from(y))];
            // det((10,10),(x+5,y+5)) = 10*(y-x), in i32 senza rounding.
            if 10 * (y - x) == 0 {
                assert!(matches!(hull(&points), Geometry::LineString(_)));
            } else {
                let input = geo::MultiPoint::from(points.to_vec()).into();
                assert_eq!(
                    hull(&points),
                    super::super::concave_hull(&input, 2.0, 0.0, 1000).unwrap()
                );
            }
        }
    }
}

#[test]
fn parameters_and_resources_are_not_ignored_on_degenerates() {
    let input: Geometry<f64> = Point::new(0.0, 0.0).into();
    let run = |concavity, length, tolerance, coordinates, limits| {
        concave_hull_dimensional_with_limits(
            &input,
            concavity,
            length,
            tolerance,
            coordinates,
            limits,
            None,
        )
    };
    for (concavity, length, tolerance) in [
        (0.0, 0.0, 0.0),
        (f64::INFINITY, 0.0, 0.0),
        (2.0, -1.0, 0.0),
        (2.0, f64::NAN, 0.0),
        (2.0, 0.0, -1.0),
        (2.0, 0.0, f64::NAN),
    ] {
        assert!(matches!(
            run(concavity, length, tolerance, 1, RelateLimits::default()),
            Err(DimensionalHullError::Extended(
                ExtendedError::InvalidParameter { .. }
            ))
        ));
    }
    assert_eq!(
        run(2.0, 0.0, 0.01, 1, RelateLimits::default()).unwrap(),
        input
    );
    assert!(matches!(
        run(2.0, 0.0, 0.0, 0, RelateLimits::default()),
        Err(DimensionalHullError::Extended(
            ExtendedError::CoordinateLimit { .. }
        ))
    ));
    assert!(matches!(
        run(
            2.0,
            0.0,
            0.0,
            1,
            RelateLimits {
                max_steps: 0,
                ..RelateLimits::default()
            }
        ),
        Err(DimensionalHullError::Predicate(PredicateError::WorkLimit))
    ));
    let input: Geometry<f64> = geo::MultiPoint::from(vec![(0.0, 0.0), (1.0, 1.0)]).into();
    assert!(matches!(
        concave_hull_dimensional_with_limits(
            &input,
            2.0,
            0.0,
            0.0,
            2,
            RelateLimits {
                max_reserved_bytes: 0,
                ..RelateLimits::default()
            },
            None
        ),
        Err(DimensionalHullError::Predicate(
            PredicateError::ReservationLimit
        ))
    ));
}
