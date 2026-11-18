use super::*;
use geo::Coord;
use std::time::{Duration, Instant};

#[deny(clippy::disallowed_methods)]
fn line(points: &[(f64, f64)]) -> Geometry<f64> {
    Geometry::LineString(LineString::from(points.to_vec()))
}
#[deny(clippy::disallowed_methods)]
fn run(target: &Geometry<f64>, reshaper: &Geometry<f64>) -> Result<LineString<f64>, ReshapeError> {
    reshape_with_limits(target, reshaper, RelateLimits::default(), None)
}

#[test]
#[deny(clippy::disallowed_methods)]
fn directed_replacement_and_vertices() {
    let target = line(&[(0., 0.), (10., 0.)]);
    let shape = [(2., -2.), (2., 2.), (8., 2.), (8., -2.)];
    let expected = LineString::from(vec![
        (0., 0.),
        (2., 0.),
        (2., 2.),
        (8., 2.),
        (8., 0.),
        (10., 0.),
    ]);
    // Fonte C1: GOP-019 costruzione: rimpiazza [2,8] e conserva verso +x.
    assert_eq!(run(&target, &line(&shape)).unwrap(), expected);
    let reversed: Vec<_> = shape.into_iter().rev().collect();
    // Fonte C1: GOP-019, reshaper discorde invertito nel risultato.
    assert_eq!(run(&target, &line(&reversed)).unwrap(), expected);
    // Fonte C1: GOP-019: invertendo il target si inverte l'intero risultato.
    let reversed_expected = LineString(expected.0.iter().rev().copied().collect());
    assert_eq!(
        run(&line(&[(10., 0.), (0., 0.)]), &line(&shape)).unwrap(),
        reversed_expected
    );
    // Fonte C1: GOP-019: entrambi i tagli sullo stesso lato del reshaper,
    // nessun vertice intermedio: intersezioni analitiche (-1,0) e (1,0).
    assert_eq!(
        run(
            &line(&[(-2., -2.), (0., 2.), (2., -2.)]),
            &line(&[(-3., 0.), (3., 0.)])
        )
        .unwrap(),
        LineString::from(vec![(-2., -2.), (-1., 0.), (1., 0.), (2., -2.)])
    );
    let target = line(&[(0., 0.), (2., -0.), (5., 0.), (8., 0.), (10., 0.)]);
    let shape = line(&[(2., -2.), (2., 0.), (2., 2.), (8., 2.), (8., 0.), (8., -2.)]);
    let result = run(&target, &shape).unwrap();
    // Fonte C1: GOP-019 incroci in vertici di entrambi, deduplicati per sito.
    assert_eq!(result, expected);
    // Fonte C1: GOP-015A, il vertice target conservato mantiene -0.
    assert_eq!(result.0[1].y.to_bits(), (-0.0_f64).to_bits());
    // Fonte C1: GOP-019, vertici soltanto sul target o sul reshaper.
    assert_eq!(
        run(&target, &line(&[(2., -2.), (2., 2.), (8., 2.), (8., -2.)])).unwrap(),
        expected
    );
    assert_eq!(
        run(&line(&[(0., 0.), (10., 0.)]), &shape).unwrap(),
        expected
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn excluded_intersections_and_inputs() {
    let target = line(&[(0., 0.), (10., 0.)]);
    for shape in [
        line(&[(2., 1.), (8., 1.)]),
        line(&[(2., -1.), (2., 1.)]),
        line(&[(2., -1.), (3., 1.), (5., -1.), (7., 1.)]),
        line(&[(1., -1.), (2., 0.), (4., 0.), (5., 1.)]),
        line(&[(1., 1.), (2., 0.), (3., 1.), (8., 1.), (8., -1.)]),
        line(&[(2., 0.), (2., 2.), (8., 2.), (8., -1.)]),
        line(&[(0., -1.), (0., 2.), (8., 2.), (8., -1.)]),
        line(&[(2., -1.), (8., 2.), (2., 2.), (8., -1.)]),
    ] {
        // Fonte C1: GOP-019: zero/uno/tre incroci, overlap, tangenza,
        // contatto terminale di ciascun operando e reshaper non semplice.
        assert!(matches!(
            run(&target, &shape),
            Err(ReshapeError::Unsupported)
        ));
    }
    let shape = line(&[(2., -1.), (2., 2.), (8., 2.), (8., -1.)]);
    for target in [
        line(&[(0., 0.), (10., 0.), (0., 0.)]),
        Geometry::MultiLineString(geo::MultiLineString(vec![LineString::from(vec![
            (0., 0.),
            (10., 0.),
        ])])),
        Geometry::Polygon(geo::Polygon::new(
            LineString::from(vec![(0., 0.), (10., 0.), (0., 10.), (0., 0.)]),
            vec![],
        )),
        line(&[(0., 0.), (0., 0.), (10., 0.)]),
    ] {
        // Fonte C1: GOP-019 target chiuso/multiparte/poligono/degenere escluso.
        assert!(matches!(
            run(&target, &shape),
            Err(ReshapeError::Unsupported)
        ));
    }
    for bad in [
        line(&[]),
        line(&[(0., 0.)]),
        line(&[(0., 0.), (f64::NAN, 1.)]),
    ] {
        // Fonte C1: GOP-002/029, malformazione data_mapping in entrambi i ruoli.
        assert!(matches!(
            run(&bad, &shape),
            Err(ReshapeError::InvalidGeometry)
        ));
        assert!(matches!(
            run(&target, &bad),
            Err(ReshapeError::InvalidGeometry)
        ));
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn bent_crossings_and_closed_reshaper() {
    let target = line(&[(0., -1.), (2., 0.), (4., -1.), (6., 0.), (8., -1.)]);
    let shape = line(&[(2., -2.), (2., 0.), (4., 2.), (6., 0.), (6., -2.)]);
    // Fonte C1: GOP-019, i quattro rami alternano anche in vertici angolosi.
    assert_eq!(
        run(&target, &shape).unwrap(),
        LineString::from(vec![(0., -1.), (2., 0.), (4., 2.), (6., 0.), (8., -1.)])
    );
    // Fonte C1: GOP-019, reshaper singolo semplice chiuso; intervallo interno
    // all'ordine memorizzato, non scelta di regione poligonale.
    assert_eq!(
        run(
            &line(&[(0., 0.), (10., 0.)]),
            &line(&[(2., -2.), (2., 2.), (8., 2.), (8., -2.), (2., -2.)])
        )
        .unwrap(),
        LineString::from(vec![
            (0., 0.),
            (2., 0.),
            (2., 2.),
            (8., 2.),
            (8., 0.),
            (10., 0.)
        ])
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn reconstructed_self_contact_is_rejected() {
    let target = line(&[(0., 0.), (10., 0.)]);
    let shape = line(&[
        (2., -1.),
        (2., 2.),
        (-1., 2.),
        (-1., -2.),
        (8., -2.),
        (8., 1.),
    ]);
    // Fonte C1: GOP-019, ricostruzione semplice: questo arco circonda start
    // senza toccarlo, e' consentito nonostante non sia monotono in x.
    assert_eq!(
        run(&target, &shape).unwrap(),
        LineString::from(vec![
            (0., 0.),
            (2., 0.),
            (2., 2.),
            (-1., 2.),
            (-1., -2.),
            (8., -2.),
            (8., 0.),
            (10., 0.)
        ])
    );
    let shape = line(&[
        (2., -1.),
        (2., 2.),
        (1., 2.),
        (1., -2.),
        (8., -2.),
        (8., 1.),
    ]);
    // Fonte C1: GOP-019: il tratto sostitutivo incrocia il prefisso in (1,0).
    // E' anche un terzo incontro: la validazione lo rifiuta prima del montaggio.
    assert!(matches!(
        run(&target, &shape),
        Err(ReshapeError::Unsupported)
    ));
    let mut work = Work::with_deadline_precedence(RelateLimits::default(), None);
    let mut points = CountedVec::new();
    for (x, y) in [
        (0., 0.),
        (2., 0.),
        (2., 2.),
        (1., 2.),
        (1., -2.),
        (8., -2.),
        (8., 0.),
        (10., 0.),
    ] {
        work.push(&mut points, P::of_in(Coord { x, y }, &work).unwrap())
            .unwrap();
    }
    // Fonte C1: GOP-019/O1, anche il gate di ricostruzione rifiuta l'incrocio.
    assert!(matches!(
        simple(&points, &mut work),
        Err(TrimExtendError::Unsupported)
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_nodes_and_rounding_topology() {
    let target = line(&[(0., 0.), (10., 0.)]);
    let result = run(&target, &line(&[(0., -1.), (1., 2.), (8., 2.), (8., -2.)])).unwrap();
    // Fonte C1: GOP-019: -1+3x=0; GOP-015A nearest-even(1/3).
    assert_eq!(result.0[1].x.to_bits(), 0x3fd5_5555_5555_5555);
    let work = Work::with_deadline_precedence(RelateLimits::default(), None);
    let exact = work
        .integer(1)
        .unwrap()
        .try_div(&work.integer(3).unwrap())
        .unwrap();
    let rounded = P::of_in(result.0[1], &work).unwrap();
    let error = exact.try_sub(&rounded.x).unwrap();
    let half = P::of_in(
        Coord {
            x: 2.0_f64.powi(-55),
            y: 0.,
        },
        &work,
    )
    .unwrap()
    .x;
    // Fonte C1: GOP-015A: errore esatto 1/(3*2^54), inferiore a 2^-55.
    assert!(error.sign().is_gt());
    assert!(error.try_cmp(&half).unwrap().is_lt());
    for (low, high, expected) in [
        (1., 1.0_f64.next_up(), 1.),
        (
            1.0_f64.next_up(),
            1.0_f64.next_up().next_up(),
            1.0_f64.next_up().next_up(),
        ),
    ] {
        // Fonte C1: GOP-015A, midpoint ties-to-even con mantissa pari/dispari.
        assert_eq!(
            run(
                &target,
                &line(&[(low, -1.), (high, 1.), (8., 1.), (8., -1.)])
            )
            .unwrap()
            .0[1]
                .x
                .to_bits(),
            expected.to_bits()
        );
    }
    // Fonte C1: GOP-015A, x=1+2^-53 e' interno esatto ma arrotonda su start.
    assert!(matches!(
        run(
            &line(&[(1., 0.), (10., 0.)]),
            &line(&[(1., -1.), (1.0_f64.next_up(), 1.), (8., 1.), (8., -1.)])
        ),
        Err(ReshapeError::InvalidRoundedTopology)
    ));
}
#[test]
#[deny(clippy::disallowed_methods)]
fn exact_resource_thresholds() {
    let target = line(&[(0.0, 0.0), (10.0, 0.0)]);
    let guide = line(&[(2.0, -1.0), (2.0, 2.0), (8.0, 2.0), (8.0, -1.0)]);
    let mut measured = Work::with_deadline_precedence(RelateLimits::default(), None);
    execute(&target, &guide, &mut measured).unwrap();
    for delta in [-1_i64, 0, 1] {
        for axis in 0..4 {
            let adjust = |n: u64| n.checked_add_signed(delta).unwrap();
            let mut limits = RelateLimits::default();
            match axis {
                0 => limits.max_steps = adjust(measured.steps),
                1 => limits.max_segment_pairs = adjust(measured.pairs),
                2 => limits.max_nodes = adjust(measured.nodes),
                _ => limits.max_reserved_bytes = adjust(measured.peak_bytes()),
            }
            let result = reshape_with_limits(&target, &guide, limits, None);
            // Fonte C1: GOP-026/027 limite esatto incluso, un addebito in piu'
            // fallisce; picco dei byte richiesti vivi, non una stima per vertice.
            if delta < 0 {
                if axis == 3 {
                    assert!(matches!(
                        result,
                        Err(ReshapeError::Predicate(PredicateError::ReservationLimit))
                    ));
                } else {
                    assert!(matches!(
                        result,
                        Err(ReshapeError::Predicate(PredicateError::WorkLimit))
                    ));
                }
            } else {
                assert_eq!(
                    result.unwrap(),
                    LineString::from(vec![
                        (0., 0.),
                        (2., 0.),
                        (2., 2.),
                        (8., 2.),
                        (8., 0.),
                        (10., 0.)
                    ])
                );
            }
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn cancellation_deadline_entry_and_during_work() {
    let target = line(&[(0.0, 0.0), (1.0, 0.0)]);
    let guide = line(&[(2.0, -1.0), (2.0, 1.0)]);
    let flag = AtomicBool::new(true);
    // Fonte C1: GOP-028, checkpoint d'ingresso e precedenza deadline.
    assert!(matches!(
        reshape_with_limits(&target, &guide, RelateLimits::default(), Some(&flag)),
        Err(ReshapeError::Predicate(PredicateError::Cancelled))
    ));
    assert!(matches!(
        reshape_with_limits(
            &target,
            &guide,
            RelateLimits {
                deadline: Some(Instant::now()),
                ..RelateLimits::default()
            },
            Some(&flag)
        ),
        Err(ReshapeError::Predicate(PredicateError::DeadlineExceeded))
    ));
    let coords: Vec<_> = (0..20_000)
        .map(|i| (f64::from(i), f64::from(i % 2)))
        .collect();
    let target = line(&coords);
    let flag = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let signal = &flag;
        scope.spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            signal.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        // Fonte C1: GOP-028, segnale dopo l'ingresso durante la validazione.
        assert!(matches!(
            reshape_with_limits(&target, &guide, RelateLimits::default(), Some(&flag)),
            Err(ReshapeError::Predicate(PredicateError::Cancelled))
        ));
    });
    // Fonte C1: GOP-028, deadline futura scade durante il lavoro.
    assert!(matches!(
        reshape_with_limits(
            &target,
            &guide,
            RelateLimits {
                deadline: Some(Instant::now() + Duration::from_millis(20)),
                ..RelateLimits::default()
            },
            None
        ),
        Err(ReshapeError::Predicate(PredicateError::DeadlineExceeded))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_domain_boundary_and_zero_budgets() {
    let target = line(&[(0., 0.), (10., 0.)]);
    let tiny = f64::from_bits(1);
    let shape = |x| line(&[(x, -1.), (x, 1.), (8., 1.), (8., -1.)]);
    // Fonte C1: GOP-019: il minimo binary64 positivo e' interno, senza epsilon.
    assert_eq!(
        run(&target, &shape(tiny)).unwrap().0[1].x.to_bits(),
        tiny.to_bits()
    );
    for x in [0., -tiny] {
        // Fonte C1: GOP-019: contatto terminale o un solo incontro sono esclusi.
        assert!(matches!(
            run(&target, &shape(x)),
            Err(ReshapeError::Unsupported)
        ));
    }
    for axis in 0..4 {
        let mut limits = RelateLimits::default();
        match axis {
            0 => limits.max_steps = 0,
            1 => limits.max_segment_pairs = 0,
            2 => limits.max_nodes = 0,
            _ => limits.max_reserved_bytes = 0,
        }
        let result = reshape_with_limits(&target, &shape(2.), limits, None);
        // Fonte C1: GOP-026/027: zero permette zero lavoro o memoria, errore tipizzato.
        if axis == 3 {
            assert!(matches!(
                result,
                Err(ReshapeError::Predicate(PredicateError::ReservationLimit))
            ));
        } else {
            assert!(matches!(
                result,
                Err(ReshapeError::Predicate(PredicateError::WorkLimit))
            ));
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn observed_domain_failure_keeps_precedence() {
    let target = line(&[(0., 0.), (10., 0.)]);
    let shape = line(&[(1., -1.), (2., 0.), (4., 0.), (5., 1.)]);
    let mut measured = Work::with_deadline_precedence(RelateLimits::default(), None);
    // Fonte C1: GOP-019, overlap sempre unsupported senza limiti raggiunti.
    assert!(matches!(
        execute(&target, &shape, &mut measured),
        Err(ReshapeError::Unsupported)
    ));
    // Fonte C1: GOP-028: overlap riconosciuto prima della fine della scansione;
    // un passo mancante al budget completo non puo' mascherarlo con WorkLimit.
    assert!(matches!(
        reshape_with_limits(
            &target,
            &shape,
            RelateLimits {
                max_steps: measured.steps - 1,
                ..RelateLimits::default()
            },
            None
        ),
        Err(ReshapeError::Unsupported)
    ));
    let mut observed = false;
    for max_steps in 0..=measured.steps {
        let result = reshape_with_limits(
            &target,
            &shape,
            RelateLimits {
                max_steps,
                ..RelateLimits::default()
            },
            None,
        );
        if matches!(result, Err(ReshapeError::Unsupported)) {
            observed = true;
        }
        // Fonte C1: GOP-028: appena osservato il rifiuto di dominio, una
        // successiva interruzione del noder non ne cambia la categoria.
        if observed {
            assert!(matches!(result, Err(ReshapeError::Unsupported)));
        } else {
            assert!(matches!(
                result,
                Err(ReshapeError::Predicate(PredicateError::WorkLimit))
            ));
        }
    }
    // Fonte C1: GOP-019/028: il budget completo raggiunge il rifiuto di dominio.
    assert!(observed);
}
