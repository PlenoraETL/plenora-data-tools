use super::*;
use std::time::Instant;

#[deny(clippy::disallowed_methods)]
fn line(coords: &[(f64, f64)]) -> Geometry<f64> {
    Geometry::LineString(LineString::from(coords.to_vec()))
}
#[deny(clippy::disallowed_methods)]
fn options(distance: f64, join: OffsetJoin) -> OffsetOptions {
    OffsetOptions {
        distance,
        join,
        arc_tolerance: 0.001,
    }
}
#[deny(clippy::disallowed_methods)]
fn run(input: &Geometry<f64>, distance: f64, join: OffsetJoin) -> OffsetResult {
    offset_with_limits(
        input,
        options(distance, join),
        RelateLimits::default(),
        None,
    )
    .unwrap()
}

#[test]
#[deny(clippy::disallowed_methods)]
fn directed_butt_zero_and_collinear() {
    // Fonte C1: GOP-017, raccordo predefinito rounded senza default di servizio.
    assert!(matches!(
        OffsetOptions::new(2.0, 0.001).join,
        OffsetJoin::Rounded
    ));
    let input = line(&[(0.0, 0.0), (10.0, 0.0)]);
    // Fonte C1: GOP-017 e GEO-editing 29a, normale destra (dy,-dx).
    assert_eq!(
        run(&input, 2.0, OffsetJoin::default()).line,
        LineString::from(vec![(0.0, -2.0), (10.0, -2.0)])
    );
    assert_eq!(
        run(&input, -2.0, OffsetJoin::default()).line,
        LineString::from(vec![(0.0, 2.0), (10.0, 2.0)])
    );
    assert_eq!(
        run(
            &line(&[(10.0, 0.0), (0.0, 0.0)]),
            2.0,
            OffsetJoin::default()
        )
        .line,
        LineString::from(vec![(10.0, 2.0), (0.0, 2.0)])
    );
    let input = line(&[(-0.0, 0.0), (5.0, 0.0), (10.0, 0.0)]);
    // Fonte C1: GOP-017 distanza zero conserva ingresso; GOP-015A bit invariati.
    let zero = run(&input, -0.0, OffsetJoin::Rounded);
    assert_eq!(zero.line.0[0].x.to_bits(), (-0.0_f64).to_bits());
    assert_eq!(zero.line.0.len(), 3);
    assert_eq!(
        run(&input, 2.0, OffsetJoin::Rounded).line,
        LineString::from(vec![(0.0, -2.0), (10.0, -2.0)])
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn analytical_mitre_bevel_inner_and_limit() {
    let input = line(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    // Fonte C1: GOP-017; rette y=-2 e x=12, intersezione (12,-2).
    assert_eq!(
        run(&input, 2.0, OffsetJoin::Mitered { limit: 2.0 }).line,
        LineString::from(vec![(0.0, -2.0), (12.0, -2.0), (12.0, 10.0)])
    );
    // Fonte C1: GOP-017 bevel collega (10,-2) e (12,0); sqrt(2)>1.
    let bevel = LineString::from(vec![(0.0, -2.0), (10.0, -2.0), (12.0, 0.0), (12.0, 10.0)]);
    assert_eq!(run(&input, 2.0, OffsetJoin::Bevelled).line, bevel);
    assert_eq!(
        run(&input, 2.0, OffsetJoin::Mitered { limit: 1.0 }).line,
        bevel
    );
    // Fonte C1: GOP-017 raccordo interno sempre intersezione y=2, x=8.
    for join in [
        OffsetJoin::Rounded,
        OffsetJoin::Bevelled,
        OffsetJoin::Mitered { limit: 1.0 },
    ] {
        assert_eq!(
            run(&input, -2.0, join).line,
            LineString::from(vec![(0.0, 2.0), (8.0, 2.0), (8.0, 10.0)])
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn circular_arc_has_analytic_sagitta_certificate() {
    let result = run(
        &line(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]),
        2.0,
        OffsetJoin::Rounded,
    );
    // Fonte C1: GOP-017 e GOP-006; cerchio centro (10,0), r=2,
    // freccia r-sqrt(r^2-corda^2/4), bound Hausdorff simmetrico.
    for point in &result.line.0[1..result.line.0.len() - 1] {
        assert!(
            ((point.x - 10.0).hypot(point.y) - 2.0).abs() <= result.accuracy.rounding_bound + 1e-15
        );
    }
    for pair in result.line.0[1..result.line.0.len() - 1].windows(2) {
        let chord = (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y);
        let sagitta = 2.0 - (4.0 - chord * chord / 4.0).sqrt();
        assert!(sagitta <= 0.001 + result.accuracy.rounding_bound);
    }
    assert_eq!(
        result.accuracy.discretization_bound.to_bits(),
        0.001_f64.to_bits()
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn oblique_irrational_and_non_dyadic_publication() {
    // Fonte C1: GOP-017 normale (4/5,-3/5); GOP-015A nearest-even.
    let result = run(&line(&[(0.0, 0.0), (3.0, 4.0)]), 1.0, OffsetJoin::Rounded);
    assert_eq!(
        result.line.0[0],
        Coord {
            x: 4.0 / 5.0,
            y: -3.0 / 5.0
        }
    );
    let mut work = Work::new(RelateLimits::default(), None);
    for (value, n) in [(result.line.0[0].x, 4), (result.line.0[0].y, -3)] {
        let exact = work
            .integer(n)
            .unwrap()
            .try_div(&work.integer(5).unwrap())
            .unwrap();
        let actual = rational(value, &work).unwrap();
        let error = absolute(actual.try_sub(&exact).unwrap(), &work).unwrap();
        let gap = rational(value.next_up(), &work)
            .unwrap()
            .try_sub(&actual)
            .unwrap()
            .try_div(&work.integer(2).unwrap())
            .unwrap();
        assert!(error.try_cmp(&gap).unwrap().is_le());
    }
    // Fonte C1: normale (1/sqrt(2),-1/sqrt(2)); intervallo razionale
    // certifica l'arrotondamento senza dipendere dalla sqrt della piattaforma.
    let result = run(&line(&[(0.0, 0.0), (1.0, 1.0)]), 1.0, OffsetJoin::Rounded);
    assert_eq!(result.line.0[0].x.to_bits(), 0x3fe6_a09e_667f_3bcd);
    assert_eq!(result.line.0[0].y.to_bits(), 0xbfe6_a09e_667f_3bcd);
    // Fonte C1: GOP-015A, funzione unica anche per un razionale 1/3.
    let third = work
        .integer(1)
        .unwrap()
        .try_div(&work.integer(3).unwrap())
        .unwrap();
    assert_eq!(
        ordinate(&third, &mut work).unwrap().to_bits(),
        (1.0_f64 / 3.0).to_bits()
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn rejected_domains_and_parameters() {
    // Fonte C1: GOP-017 esclusioni esplicite, GOP-016 configurazione.
    for input in [
        line(&[(0.0, 0.0), (0.0, 0.0)]),
        line(&[(0.0, 0.0), (1.0, 0.0), (0.0, 0.0)]),
        line(&[(0.0, 0.0), (2.0, 2.0), (0.0, 2.0), (2.0, 0.0)]),
    ] {
        assert!(matches!(
            offset_with_limits(
                &input,
                options(1.0, OffsetJoin::Rounded),
                RelateLimits::default(),
                None
            ),
            Err(OffsetError::Unsupported)
        ));
    }
    let input = line(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]);
    for distance in [-1.0, -2.0] {
        assert!(matches!(
            offset_with_limits(
                &input,
                options(distance, OffsetJoin::Rounded),
                RelateLimits::default(),
                None
            ),
            Err(OffsetError::Unsupported)
        ));
    }
    for config in [
        options(f64::NAN, OffsetJoin::Rounded),
        options(1.0, OffsetJoin::Mitered { limit: 0.5 }),
        OffsetOptions {
            arc_tolerance: 0.0,
            ..options(1.0, OffsetJoin::Rounded)
        },
    ] {
        assert!(matches!(
            offset_with_limits(&input, config, RelateLimits::default(), None),
            Err(OffsetError::InvalidConfiguration)
        ));
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_work_thresholds_and_interruptions() {
    let input = line(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let config = options(1.0, OffsetJoin::Bevelled);
    let mut work = Work::new(RelateLimits::default(), None);
    execute(&input, config, &mut work).unwrap();
    let limits = RelateLimits {
        max_steps: work.steps,
        max_nodes: work.nodes,
        max_segment_pairs: work.pairs,
        max_reserved_bytes: work.peak_bytes(),
        deadline: None,
    };
    // Fonte C1: GOP-026 soglia inclusiva, successivo addebito rifiutato.
    offset_with_limits(&input, config, limits, None).unwrap();
    for limits in [
        RelateLimits {
            max_steps: limits.max_steps - 1,
            ..limits
        },
        RelateLimits {
            max_nodes: limits.max_nodes - 1,
            ..limits
        },
        RelateLimits {
            max_segment_pairs: limits.max_segment_pairs - 1,
            ..limits
        },
    ] {
        assert!(matches!(
            offset_with_limits(&input, config, limits, None),
            Err(OffsetError::Predicate(PredicateError::WorkLimit))
        ));
    }
    assert!(matches!(
        offset_with_limits(
            &input,
            config,
            RelateLimits {
                max_reserved_bytes: limits.max_reserved_bytes - 1,
                ..limits
            },
            None
        ),
        Err(OffsetError::Predicate(PredicateError::ReservationLimit))
    ));
    offset_with_limits(
        &input,
        config,
        RelateLimits {
            max_steps: limits.max_steps + 1,
            max_nodes: limits.max_nodes + 1,
            max_segment_pairs: limits.max_segment_pairs + 1,
            max_reserved_bytes: limits.max_reserved_bytes + 1,
            ..limits
        },
        None,
    )
    .unwrap();
    // Fonte C1: convenzione RelateLimits, cancellazione/deadline all'ingresso.
    let flag = AtomicBool::new(true);
    assert!(matches!(
        offset_with_limits(&input, config, RelateLimits::default(), Some(&flag)),
        Err(OffsetError::Predicate(PredicateError::Cancelled))
    ));
    assert!(matches!(
        offset_with_limits(
            &input,
            config,
            RelateLimits {
                deadline: Some(Instant::now()),
                ..RelateLimits::default()
            },
            None
        ),
        Err(OffsetError::Predicate(PredicateError::DeadlineExceeded))
    ));
    // Fonte C1: GOP-026, raffinazione degli archi contata prima della crescita.
    assert!(matches!(
        offset_with_limits(
            &input,
            OffsetOptions {
                arc_tolerance: 1e-100,
                ..options(1.0, OffsetJoin::Rounded)
            },
            RelateLimits {
                max_nodes: 100,
                ..RelateLimits::default()
            },
            None
        ),
        Err(OffsetError::Predicate(PredicateError::WorkLimit))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn non_simple_output_and_correlated_radicals() {
    // Fonte C1: GOP-017 preserva l'uscita non semplice; incrocio (7,2)
    // fra primo e ultimo lato, nessun lato collassato o invertito.
    let input = line(&[
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (5.0, 10.0),
        (5.0, 1.0),
    ]);
    let result = run(&input, -2.0, OffsetJoin::Rounded);
    assert_eq!(
        result.line,
        LineString::from(vec![
            (0.0, 2.0),
            (8.0, 2.0),
            (8.0, 8.0),
            (7.0, 8.0),
            (7.0, 1.0)
        ])
    );
    // Fonte C1: intersezione analitica delle parallele GOP-017,
    // x=0 per simmetria, y=-sqrt(2); GOP-015A nearest-even.
    let input = line(&[(-1.0, -1.0), (0.0, 0.0), (1.0, -1.0)]);
    let result = run(&input, 1.0, OffsetJoin::Rounded);
    assert_eq!(result.line.0[1].x.to_bits(), 0.0_f64.to_bits());
    assert_eq!(result.line.0[1].y.to_bits(), 0xbff6_a09e_667f_3bcd);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn triple_node_created_by_rounding_is_rejected() {
    let mut work = Work::new(RelateLimits::default(), None);
    let epsilon = rational(2.0_f64.powi(-56), &work).unwrap();
    let point = |x, y| P::of_in(Coord { x, y }, &work).unwrap();
    let mut c = Edge {
        a: point(-1.0, -1.0),
        b: point(1.0, 1.0),
    };
    c.a.y = c.a.y.try_add(&epsilon).unwrap();
    c.b.y = c.b.y.try_add(&epsilon).unwrap();
    let edges = [
        Edge {
            a: point(-2.0, 0.0),
            b: point(2.0, 0.0),
        },
        Edge {
            a: point(0.0, -2.0),
            b: point(0.0, 2.0),
        },
        c,
    ];
    let empty = line(&[]);
    let originals = Originals::new(&empty, &empty, &mut work).unwrap();
    let mut publications = CountedVec::new();
    for edge in &edges {
        let coords = [
            Coord {
                x: ordinate(&edge.a.x, &mut work).unwrap(),
                y: ordinate(&edge.a.y, &mut work).unwrap(),
            },
            Coord {
                x: ordinate(&edge.b.x, &mut work).unwrap(),
                y: ordinate(&edge.b.y, &mut work).unwrap(),
            },
        ];
        let publication = PublishedEdge::new(edge, coords, &mut work).unwrap();
        work.push(&mut publications, publication).unwrap();
    }
    // Fonte C1: GOP-015A. I tre incroci (0,0),(-eps,0),(0,eps)
    // non possono diventare un nodo triplo, pur conservando i segni pairwise.
    assert!(matches!(
        verify_publication(&publications, &originals, true, None, &mut work),
        Err(LineBooleanError::InvalidRoundedTopology)
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn cancellation_and_deadline_during_work() {
    let coords: Vec<_> = (0..20_000)
        .map(|i| (f64::from(i), f64::from(i % 2)))
        .collect();
    let input = line(&coords);
    // Fonte C1: RelateLimits, interruzione cooperativa anche dopo l'ingresso;
    // il thread segnala mentre validazione/costruzione visitano il percorso.
    let flag = AtomicBool::new(false);
    let flag_ref = &flag;
    std::thread::scope(|inner| {
        inner.spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            flag_ref.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert!(matches!(
            offset_with_limits(
                &input,
                options(0.1, OffsetJoin::Rounded),
                RelateLimits::default(),
                Some(&flag)
            ),
            Err(OffsetError::Predicate(PredicateError::Cancelled))
        ));
    });
    assert!(matches!(
        offset_with_limits(
            &input,
            options(0.1, OffsetJoin::Rounded),
            RelateLimits {
                deadline: Some(Instant::now() + std::time::Duration::from_millis(20)),
                ..RelateLimits::default()
            },
            None
        ),
        Err(OffsetError::Predicate(PredicateError::DeadlineExceeded))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn arc_lens_and_rounded_collapse_are_not_silent() {
    let mut work = Work::new(RelateLimits::default(), None);
    let mut points = CountedVec::new();
    for (x, y) in [
        (1.0, -1.0),
        (1.0, 0.0),
        (0.0, 1.0),
        (-1.0, 1.0),
        (0.7, 0.7),
        (0.8, 0.8),
    ] {
        let p = P::of_in(Coord { x, y }, &work).unwrap();
        work.push(&mut points, EnclosedPoint::exact(&p).unwrap())
            .unwrap();
    }
    let center = P::of_in(Coord { x: 0.0, y: 0.0 }, &work).unwrap();
    // Fonte C1: GOP-006 topologia e GOP-017 arco: la diagonale attraversa
    // il cerchio nella lente, ma non la corda x+y=1. Il certificato rifiuta.
    assert!(matches!(
        certify_arcs(
            &points,
            &[ArcRegion {
                first: 1,
                last: 2,
                center: &center
            }],
            options(1.0, OffsetJoin::Rounded),
            &mut work
        ),
        Err(OffsetError::AccuracyUnproven)
    ));
    let mut points = CountedVec::new();
    let a = P::of_in(Coord { x: 1.0, y: 1.0 }, &work).unwrap();
    let b = P {
        x: a.x
            .try_add(&rational(2.0_f64.powi(-55), &work).unwrap())
            .unwrap(),
        y: a.y.try_clone().unwrap(),
    };
    for p in [&a, &b] {
        work.push(&mut points, EnclosedPoint::exact(p).unwrap())
            .unwrap();
    }
    // Fonte C1: GOP-015A, due estremi distinti non possono fondersi.
    assert!(matches!(
        publish(&points, 0.0, &mut work),
        Err(OffsetError::InvalidRoundedTopology)
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_mitre_limit_and_clockwise_arc() {
    let input = line(&[(0.0, 0.0), (10.0, 0.0), (17.0, 24.0)]);
    // Fonte C1: GOP-017. Direzione (7,24), norma 25: il mitre (3/4,-1)
    // ha norma 5/4. La soglia esatta e' inclusiva, il binary64 inferiore no.
    let expected = LineString::from(vec![(0.0, -1.0), (10.75, -1.0), (17.96, 23.72)]);
    assert_eq!(
        run(&input, 1.0, OffsetJoin::Mitered { limit: 1.25 }).line,
        expected
    );
    assert_eq!(
        run(
            &input,
            1.0,
            OffsetJoin::Mitered {
                limit: 1.25_f64.next_down()
            }
        )
        .line,
        run(&input, 1.0, OffsetJoin::Bevelled).line
    );
    // Fonte C1: GOP-017, riflessione e cambio segno conservano il cerchio.
    let clockwise = run(
        &line(&[(0.0, 0.0), (10.0, 0.0), (10.0, -10.0)]),
        -2.0,
        OffsetJoin::Rounded,
    );
    let counterclockwise = run(
        &line(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]),
        2.0,
        OffsetJoin::Rounded,
    );
    assert_eq!(clockwise.line.0.len(), counterclockwise.line.0.len());
    for (a, b) in clockwise.line.0.iter().zip(&counterclockwise.line.0) {
        assert_eq!(a.x.to_bits(), b.x.to_bits());
        assert_eq!(a.y.abs().to_bits(), b.y.abs().to_bits());
        assert!(a.y * b.y <= 0.0);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn disjoint_collinearity_fraction_oracle() {
    // Oracolo Fraction (stair.py): parallele razionali alla scala assiale.
    let input = line(&[(0., 0.), (1., 0.), (1., 1.), (2., 1.), (2., 2.), (3., 2.)]);
    let expected = LineString::from(vec![
        (0., -0.3),
        (1., -0.3),
        (1.3, 0.),
        (1.3, 0.7),
        (2., 0.7),
        (2.3, 1.),
        (2.3, 1.7),
        (3., 1.7),
    ]);
    assert_eq!(run(&input, 0.3, OffsetJoin::Bevelled).line, expected);
    let irrational = line(&[(0., 0.), (1., 2.), (3., 1.), (4., 3.), (2., 4.), (3., 6.)]);
    let result = run(&irrational, 0.1, OffsetJoin::Bevelled);
    assert!(geo::Validation::is_valid(&result.line));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn finite_overflow_rounding_and_unused_tolerance() {
    let y = -2_f64.powi(969);
    for join in [OffsetJoin::Bevelled, OffsetJoin::Mitered { limit: 2.0 }] {
        let result = offset_with_limits(
            &line(&[(0., y), (1., y)]),
            OffsetOptions {
                distance: f64::MAX,
                join,
                arc_tolerance: f64::NAN,
            },
            RelateLimits::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            result.line,
            LineString::from(vec![(0., -f64::MAX), (1., -f64::MAX)])
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn rational_disjoint_edges_fraction_oracle() {
    let input = line(&[(0., 0.), (3., 4.), (7., 1.), (10., 5.), (6., 8.), (9., 12.)]);
    // Oracolo Fraction (collinear.py): normali di lati lunghi 5, d=1.
    let expected = LineString::from(vec![
        (0.8, -0.6),
        (3.2, 2.6),
        (6.4, 0.2),
        (7.8, 0.4),
        (10.8, 4.4),
        (10.6, 5.8),
        (7.4, 8.2),
        (9.8, 11.4),
    ]);
    assert_eq!(run(&input, 1., OffsetJoin::Bevelled).line, expected);
    assert_eq!(
        run(&line(&[(0., 0.), (1., 0.)]), 0., OffsetJoin::Rounded).line,
        LineString::from(vec![(0., 0.), (1., 0.)])
    );
}
