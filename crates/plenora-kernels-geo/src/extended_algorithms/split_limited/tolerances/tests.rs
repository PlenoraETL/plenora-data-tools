//! C1: proiezione cartesiana e somme di lunghezze analitiche, contratto 10.
use super::*;
use geo::{MultiPoint, Point};

fn run(source: &LineString<f64>, xy: &[(f64, f64)], snap: f64, merge: f64) -> Vec<LineString<f64>> {
    split_line_with_tolerances_with_limits(
        source,
        &MultiPoint::from(xy.to_vec()).into(),
        SplitLineTolerances {
            snap_distance: snap,
            merge_distance: merge,
        },
        RelateLimits::default(),
        None,
    )
    .expect("split analitico")
}

#[test]
fn independent_snap_and_merge_inclusive_thresholds() {
    let source = LineString::from(vec![(0., 0.), (100., 0.)]);
    for distance in [0.0, 0.6, 30.0, 60.0, 100.0, 200.0] {
        for cuts in [
            vec![(20., distance)],
            vec![(60., distance)],
            vec![(20., distance), (60., distance)],
        ] {
            let actual = run(&source, &cuts, distance, 0.0);
            assert_eq!(actual.len(), cuts.len() + 1);
            for (piece, cut) in actual.iter().zip(&cuts) {
                assert_eq!(piece.0.last(), Some(&geo::Coord { x: cut.0, y: 0.0 }));
            }
            if distance > 0.0 {
                assert_eq!(
                    run(&source, &cuts, distance.next_down(), 0.0),
                    vec![source.clone()]
                );
            }
        }
    }
    for merge in [40.0_f64.next_down(), 40., 60., 100., 200.] {
        let pieces = run(&source, &[(60., 30.), (20., 60.)], 60., merge);
        assert_eq!(pieces.len(), if merge < 40.0 { 3 } else { 2 });
        assert_eq!(pieces[0].0.last(), Some(&geo::Coord { x: 20., y: 0. }));
    }
    // Decisione del titolare: l'ingresso storico conserva la fusione doppia della tolerance.
    assert_eq!(
        super::super::super::split_line(
            &source,
            &MultiPoint::from(vec![(20., 0.), (60., 0.)]).into(),
            60.,
            100,
            100,
            100,
            100
        )
        .expect("compatibilita storica")
        .len(),
        2
    );
}

#[test]
fn duplicates_near_cuts_greedy_merge_and_endpoints() {
    let source = LineString::from(vec![(0., 0.), (100., 0.)]);
    let cuts = [
        (0., 0.),
        (20., 0.),
        (20., 0.),
        (20.0_f64.next_up(), 0.),
        (50., 0.),
        (80., 0.),
        (100., 0.),
    ];
    assert_eq!(run(&source, &cuts, 0., 0.).len(), 5);
    let result = run(&source, &cuts, 0., 40.);
    assert_eq!(result.len(), 3);
    assert_eq!(result[1], LineString::from(vec![(20., 0.), (80., 0.)]));
}

#[test]
fn closed_lines_use_path_distance_and_preserve_vertices() {
    let source = LineString::from(vec![(0., 0.), (10., 0.), (10., 10.), (0., 10.), (0., 0.)]);
    let cuts = [(1., 0.), (0., 1.)];
    // Distanza euclidea sqrt(2), distanza lungo il percorso 38.
    assert_eq!(run(&source, &cuts, 0., 2.).len(), 3);
    assert_eq!(run(&source, &cuts, 0., 38.).len(), 2);
    assert_eq!(
        run(&source, &cuts, 0., 100.),
        vec![
            LineString::from(vec![(0., 0.), (1., 0.)]),
            LineString::from(vec![(1., 0.), (10., 0.), (10., 10.), (0., 10.), (0., 0.)])
        ]
    );
}

#[test]
fn irrational_lengths_and_rational_pythagorean_boundary() {
    for (source, cuts, threshold) in [
        (
            vec![(0., 0.), (1., 1.), (2., 0.), (3., 1.)],
            vec![(0.5, 0.5), (2.5, 0.5)],
            2.0_f64.sqrt() * 2.0,
        ),
        (
            vec![(0., 0.), (3., 4.), (6., 0.), (9., 4.)],
            vec![(1.5, 2.), (7.5, 2.)],
            10.,
        ),
    ] {
        let line = LineString::from(source);
        assert_eq!(run(&line, &cuts, 0., threshold.next_down()).len(), 3);
        assert_eq!(run(&line, &cuts, 0., threshold.next_up()).len(), 2);
    }
}

#[test]
fn extreme_coordinates_no_translation_slack_and_nearest_even() {
    for base in [0., 2.0_f64.powi(48), -2.0_f64.powi(48)] {
        let source = LineString::from(vec![(base, base), (base + 100., base)]);
        assert_eq!(
            run(&source, &[(base + 20., base + 0.5)], 0., 0.),
            vec![source.clone()]
        );
        assert_eq!(run(&source, &[(base + 20., base + 0.5)], 0.5, 0.).len(), 2);
    }
    let extreme = LineString::from(vec![(-f64::MAX, 0.), (f64::MAX, 0.)]);
    let actual = run(&extreme, &[(0., 60.)], 60., f64::MAX);
    assert_eq!(actual.len(), 2);
    assert_eq!(actual[0].0.last(), Some(&geo::Coord { x: 0., y: 0. }));
    // (1,0) proiettato su (0,0)..(1,1): t=1/2, coordinate esatte.
    assert_eq!(
        run(
            &LineString::from(vec![(0., 0.), (1., 1.)]),
            &[(1., 0.)],
            1.,
            0.
        )[0],
        LineString::from(vec![(0., 0.), (0.5, 0.5)])
    );
    let small = f64::from_bits(1);
    assert_eq!(
        run(
            &LineString::from(vec![(0., 0.), (small * 4., 0.)]),
            &[(small, 0.)],
            0.,
            0.
        )
        .len(),
        2
    );
}

#[test]
fn typed_failures_empty_and_degenerate() {
    let source = LineString::from(vec![(0., 0.), (100., 0.)]);
    let cutter = Geometry::Point(Point::new(20., 60.));
    for value in [-1., f64::INFINITY, f64::NAN] {
        for (snap, merge) in [(value, 0.), (0., value)] {
            assert!(matches!(
                split_line_with_tolerances_with_limits(
                    &source,
                    &cutter,
                    SplitLineTolerances {
                        snap_distance: snap,
                        merge_distance: merge
                    },
                    RelateLimits::default(),
                    None
                ),
                Err(SplitLineError::InvalidTolerance)
            ));
        }
    }
    assert_eq!(
        run(&LineString::new(vec![]), &[(0., 0.)], 100., 100.),
        vec![LineString::new(vec![])]
    );
    assert!(matches!(
        split_line_with_tolerances_with_limits(
            &LineString::from(vec![(0., 0.), (0., 0.)]),
            &cutter,
            SplitLineTolerances::default(),
            RelateLimits::default(),
            None
        ),
        Err(SplitLineError::Predicate(
            PredicateError::InvalidGeometry { .. }
        ))
    ));
}

#[test]
fn repeated_segments_are_not_points_and_retraced_paths_are_certified() {
    let line = LineString::from(vec![(0., 0.), (10., 0.)]);
    let cutter = LineString::from(vec![(5., 1.), (5., 1.), (6., 1.)]).into();
    assert_eq!(
        split_line_with_tolerances_with_limits(
            &line,
            &cutter,
            SplitLineTolerances {
                snap_distance: 1.,
                merge_distance: 0.
            },
            RelateLimits::default(),
            None
        )
        .expect("lama lineare"),
        vec![line]
    );
    let source = LineString::from(vec![(0., 0.), (10., 0.), (0., 0.)]);
    assert_eq!(
        run(&source, &[(5., 1.)], 1., 100.),
        vec![
            LineString::from(vec![(0., 0.), (5., 0.)]),
            LineString::from(vec![(5., 0.), (10., 0.), (0., 0.)])
        ]
    );
}

#[test]
fn cumulative_exact_resource_thresholds() {
    let source = LineString::from(vec![(0., 0.), (100., 0.)]);
    let cutter = MultiPoint::from(vec![(20., 60.), (60., 30.)]).into();
    let options = SplitLineTolerances {
        snap_distance: 60.,
        merge_distance: 40.,
    };
    let mut work = Work::new(RelateLimits::default(), None);
    let expected = split(&source, &cutter, Some(options), &mut work).expect("misura del consumo");
    let exact = RelateLimits {
        max_steps: work.steps + 1,
        max_segment_pairs: work.pairs,
        max_nodes: work.nodes,
        max_reserved_bytes: work.peak_bytes(),
        ..RelateLimits::default()
    };
    assert_eq!(
        split_line_with_tolerances_with_limits(&source, &cutter, options, exact, None)
            .expect("soglia inclusiva"),
        expected
    );
    for resource in 0..4 {
        let mut limits = exact;
        match resource {
            0 => limits.max_steps -= 1,
            1 => limits.max_segment_pairs -= 1,
            2 => limits.max_nodes -= 1,
            _ => limits.max_reserved_bytes -= 1,
        }
        let error = split_line_with_tolerances_with_limits(&source, &cutter, options, limits, None)
            .expect_err("sotto soglia");
        if resource == 3 {
            assert!(matches!(
                error,
                SplitLineError::Predicate(PredicateError::ReservationLimit)
            ));
        } else {
            assert!(matches!(
                error,
                SplitLineError::Predicate(PredicateError::WorkLimit)
            ));
        }
    }
}

#[test]
fn projected_shared_vertex_belongs_to_preceding_occurrence() {
    for vertices in [
        vec![(0., 0.), (10., 0.), (10., 10.)],
        vec![(0., 0.), (10., 0.), (10., 0.), (10., 10.)],
    ] {
        let source = LineString::from(vertices);
        let pieces = run(&source, &[(9., -1.)], 2., 0.);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0], LineString::from(vec![(0., 0.), (9., 0.)]));
        assert_eq!(pieces[1], LineString::from(vec![(9., 0.), (10., 0.)]));
    }
}

#[test]
fn projected_rationals_use_verified_nearest_even() {
    // Dot/norm = 1/5, proiezione esatta (1/5,2/5).
    let source = LineString::from(vec![(0., 0.), (1., 2.)]);
    assert_eq!(
        run(&source, &[(1., 0.)], 1., 0.)[0],
        LineString::from(vec![(0., 0.), (0.2, 0.4)])
    );
    // Mezza minima subnormale arrotonda a zero: il collasso e' un errore.
    let tiny = f64::from_bits(1);
    let source = LineString::from(vec![(0., 0.), (tiny, tiny)]);
    assert!(matches!(
        split_line_with_tolerances_with_limits(
            &source,
            &Point::new(tiny, 0.).into(),
            SplitLineTolerances {
                snap_distance: tiny,
                merge_distance: 0.
            },
            RelateLimits::default(),
            None
        ),
        Err(SplitLineError::Predicate(
            PredicateError::InvalidRoundedTopology
        ))
    ));
}

proptest::proptest! {
    #[test]
    fn integer_axis_oracle(mut positions in proptest::collection::vec(1_i32..100, 0..20), merge in 0_i32..120) {
        positions.sort_unstable();
        positions.dedup();
        let mut boundaries = vec![0];
        let mut last = None;
        for x in &positions { if last.is_none_or(|p| x - p > merge) { boundaries.push(*x); last = Some(*x); } }
        boundaries.push(100);
        let expected: Vec<_> = boundaries.windows(2).map(|w| LineString::from(vec![(f64::from(w[0]), 0.), (f64::from(w[1]), 0.)])).collect();
        let cuts: Vec<_> = positions.iter().rev().map(|x| (f64::from(*x), 60.)).collect();
        proptest::prop_assert_eq!(run(&LineString::from(vec![(0., 0.), (100., 0.)]), &cuts, 60., f64::from(merge)), expected);
    }
}
