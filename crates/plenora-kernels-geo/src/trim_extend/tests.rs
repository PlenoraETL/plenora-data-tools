use super::*;
use geo::Coord;
use std::time::{Duration, Instant};

#[test]
#[deny(clippy::disallowed_methods)]
fn resource_prefixes_never_publish_a_partial_trim() {
    let target = line(&[(0.0, 0.0), (4.0, 0.0)]);
    let guide = line(&[(2.0, 10.0), (2.0, 11.0)]);
    // GOP-018: la semiretta sinistra della guida e' x<=2.
    let expected = LineString::from(vec![(0.0, 0.0), (2.0, 0.0)]);
    // GOP-026/027: ogni prefisso del budget deve fallire atomicamente;
    // dopo il primo successo aumentare il budget non cambia la geometria.
    for resource in 0..4 {
        let memory = resource == 1;
        let mut succeeded = false;
        for budget in (0..=4096)
            .map(|value| if memory { value * 16 } else { value })
            .chain((17..=26).map(|power| 1_u64 << power))
        {
            if succeeded && budget < (1_u64 << 26) {
                continue;
            }
            let mut limits = RelateLimits::default();
            match resource {
                0 => limits.max_steps = budget,
                1 => limits.max_reserved_bytes = budget,
                2 => limits.max_nodes = budget,
                _ => limits.max_segment_pairs = budget,
            }
            match trim_extend_with_limits(
                &target,
                &guide,
                TrimExtendOptions::default(),
                limits,
                None,
            ) {
                Ok(actual) => {
                    assert_eq!(actual, expected);
                    succeeded = true;
                }
                Err(TrimExtendError::Predicate(error)) => {
                    assert!(!succeeded);
                    if memory {
                        assert!(matches!(error, PredicateError::ReservationLimit));
                    } else {
                        assert!(matches!(error, PredicateError::WorkLimit));
                    }
                }
                Err(error) => panic!("categoria inattesa: {error:?}"),
            }
        }
        assert!(succeeded);
    }
}

#[deny(clippy::disallowed_methods)]
fn line(points: &[(f64, f64)]) -> Geometry<f64> {
    Geometry::LineString(LineString::from(points.to_vec()))
}
#[deny(clippy::disallowed_methods)]
fn run(
    target: &Geometry<f64>,
    guide: &Geometry<f64>,
    ends: TrimExtendEnds,
    mode: TrimExtendMode,
) -> Result<LineString<f64>, TrimExtendError> {
    trim_extend_with_limits(
        target,
        guide,
        TrimExtendOptions { ends, mode },
        RelateLimits::default(),
        None,
    )
}
#[deny(clippy::disallowed_methods)]
fn default_run(
    target: &Geometry<f64>,
    guide: &Geometry<f64>,
) -> Result<LineString<f64>, TrimExtendError> {
    trim_extend_with_limits(
        target,
        guide,
        TrimExtendOptions::default(),
        RelateLimits::default(),
        None,
    )
}

#[test]
#[deny(clippy::disallowed_methods)]
fn left_half_plane_and_internal_vertex() {
    let guide = line(&[(2.0, 10.0), (2.0, 11.0)]);
    let target = line(&[(0.0, 0.0), (4.0, 0.0)]);
    // Fonte C1: GEOMETRY-OPERATIONS-1.0 GOP-018, retta infinita diretta:
    // cross((0,1),(x-2,y-10))=2-x, parte sinistra x<=2.
    assert_eq!(
        default_run(&target, &guide).unwrap(),
        LineString::from(vec![(0.0, 0.0), (2.0, 0.0)])
    );
    // Fonte C1: GOP-018, inversione della guida conserva x>=2, ordine target.
    assert_eq!(
        default_run(&target, &line(&[(2.0, 11.0), (2.0, 10.0)])).unwrap(),
        LineString::from(vec![(2.0, 0.0), (4.0, 0.0)])
    );
    // Fonte C1: GOP-018, un attraversamento interno al percorso puo' essere
    // un vertice originale; neither disabilita solo le estensioni.
    assert_eq!(
        run(
            &line(&[(0.0, -0.0), (2.0, -0.0), (4.0, 1.0)]),
            &guide,
            TrimExtendEnds::Neither,
            TrimExtendMode::MoveTerminalVertex
        )
        .unwrap()
        .0[1]
            .y
            .to_bits(),
        (-0.0_f64).to_bits()
    );
    // Fonte C1: GOP-018, guida rettilinea con vertici intermedi stessa retta.
    assert_eq!(
        default_run(&target, &line(&[(2.0, 10.0), (2.0, 11.0), (2.0, 12.0)])).unwrap(),
        default_run(&target, &guide).unwrap()
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn terminal_rays_modes_and_selection() {
    let target = line(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]);
    for (x, end, disabled, added, moved) in [
        (
            -2.0,
            TrimExtendEnds::Start,
            TrimExtendEnds::End,
            vec![(-2.0, 0.0), (0.0, 0.0), (1.0, 0.0), (2.0, 0.0)],
            vec![(-2.0, 0.0), (1.0, 0.0), (2.0, 0.0)],
        ),
        (
            4.0,
            TrimExtendEnds::End,
            TrimExtendEnds::Start,
            vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (4.0, 0.0)],
            vec![(0.0, 0.0), (1.0, 0.0), (4.0, 0.0)],
        ),
    ] {
        let guide = line(&[(x, 10.0), (x, 11.0)]);
        // Fonte C1: GOP-018, raggio iniziale verso -x, finale verso +x;
        // default both/add vertex. Intersezione analitica (x,0).
        assert_eq!(
            default_run(&target, &guide).unwrap(),
            LineString::from(added.clone())
        );
        assert_eq!(
            run(&target, &guide, end, TrimExtendMode::AddVertex).unwrap(),
            LineString::from(added)
        );
        // Fonte C1: GOP-018, move terminal vertex sostituisce solo l'estremo.
        assert_eq!(
            run(&target, &guide, end, TrimExtendMode::MoveTerminalVertex).unwrap(),
            LineString::from(moved)
        );
        for ends in [disabled, TrimExtendEnds::Neither] {
            // Fonte C1: GOP-018, nessun raggio abilitato idoneo: invariato.
            assert_eq!(
                Geometry::LineString(
                    run(&target, &guide, ends, TrimExtendMode::AddVertex).unwrap()
                ),
                target
            );
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn zero_hits_preserve_every_bit() {
    let coords = [(-0.0, -0.0), (1.0, 0.0), (2.0, -0.0)];
    let result = default_run(&line(&coords), &line(&[(0.0, 1.0), (1.0, 1.0)])).unwrap();
    // Fonte C1: GOP-018 zero colpi conserva ingresso, GOP-015A vertici invariati.
    assert_eq!(result.0.len(), coords.len());
    for (actual, expected) in result.0.iter().zip(coords) {
        assert_eq!(actual.x.to_bits(), expected.0.to_bits());
        assert_eq!(actual.y.to_bits(), expected.1.to_bits());
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn two_hits_and_all_excluded_contacts() {
    let guide = line(&[(-10.0, 0.0), (10.0, 0.0)]);
    let arch = line(&[(0.0, 1.0), (1.0, 2.0), (2.0, 1.0)]);
    // Fonte C1: GOP-018, raggi colpiscono (-1,0) e (3,0): ambiguita'.
    assert!(matches!(
        default_run(&arch, &guide),
        Err(TrimExtendError::Unsupported)
    ));
    // Fonte C1: GOP-018, selezione esplicita elimina l'altro colpo idoneo.
    assert_eq!(
        run(
            &arch,
            &guide,
            TrimExtendEnds::Start,
            TrimExtendMode::AddVertex
        )
        .unwrap(),
        LineString::from(vec![(-1.0, 0.0), (0.0, 1.0), (1.0, 2.0), (2.0, 1.0)])
    );
    for target in [
        line(&[(0.0, 0.0), (1.0, 0.0)]),
        line(&[(0.0, 1.0), (1.0, 0.0), (2.0, 1.0)]),
        line(&[(0.0, 0.0), (1.0, 1.0)]),
        line(&[(0.0, 1.0), (1.0, 0.0)]),
        line(&[(0.0, 1.0), (1.0, 0.0), (2.0, 0.0), (3.0, -1.0)]),
        line(&[(0.0, 1.0), (1.0, -1.0), (2.0, 1.0)]),
        line(&[(0.0, 1.0), (1.0, 2.0), (0.0, 1.0)]),
        Geometry::MultiLineString(geo::MultiLineString(vec![LineString::from(vec![
            (0.0, 1.0),
            (1.0, 1.0),
        ])])),
        line(&[(0.0, 1.0), (0.0, 1.0), (1.0, 2.0)]),
        line(&[(0.0, 1.0), (2.0, 3.0), (0.0, 3.0), (2.0, 1.0)]),
    ] {
        // Fonte C1: GOP-018 esclusioni: collinearita', tangenza, contatti
        // terminali, overlap, piu' tagli, chiusura, multipart, degeneri/non semplici.
        assert!(matches!(
            default_run(&target, &guide),
            Err(TrimExtendError::Unsupported)
        ));
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn invalid_guide_and_reconstruction() {
    let target = line(&[(0.0, 1.0), (1.0, 2.0)]);
    for guide in [
        line(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]),
        line(&[(0.0, 0.0), (2.0, 0.0), (1.0, 0.0)]),
    ] {
        // Fonte C1: GOP-018, guida singola retta diretta non degenere.
        assert!(matches!(
            default_run(&target, &guide),
            Err(TrimExtendError::Unsupported)
        ));
    }
    for invalid in [
        line(&[]),
        line(&[(0.0, 0.0)]),
        line(&[(0.0, 0.0), (f64::NAN, 1.0)]),
    ] {
        // Fonte C1: GOP-002/029, struttura malformata: data_mapping tipizzato.
        assert!(matches!(
            default_run(&invalid, &target),
            Err(TrimExtendError::InvalidGeometry)
        ));
    }
    let target = line(&[(0.0, 1.0), (1.0, 1.0), (1.0, 3.0), (-1.0, 3.0), (-1.0, 0.5)]);
    for mode in [
        TrimExtendMode::AddVertex,
        TrimExtendMode::MoveTerminalVertex,
    ] {
        // Fonte C1: GOP-018, prolungare start verso x=-2 interseca il lato
        // x=-1 in (-1,1): ricostruzione non semplice, nessun repair.
        assert!(matches!(
            run(
                &target,
                &line(&[(-2.0, 0.0), (-2.0, 1.0)]),
                TrimExtendEnds::Start,
                mode
            ),
            Err(TrimExtendError::Unsupported)
        ));
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn nearest_even_and_rounded_collapse() {
    let result = default_run(
        &line(&[(0.0, 0.0), (1.0, 0.0)]),
        &line(&[(0.0, 1.0), (1.0, -2.0)]),
    )
    .unwrap();
    // Fonte C1: GOP-018 soluzione 1-3x=0; GOP-015A 1/3 nearest-even
    // e' 0x3fd5555555555555, errore esatto 1/(3*2^54)<2^-55.
    assert_eq!(result.0[0].x.to_bits(), 0x3fd5_5555_5555_5555);
    let mut work = Work::new(RelateLimits::default(), None);
    let third = work
        .integer(1)
        .unwrap()
        .try_div(&work.integer(3).unwrap())
        .unwrap();
    let rounded = P::of_in(result.0[0], &work).unwrap();
    let error = third.try_sub(&rounded.x).unwrap();
    let half_ulp = P::of_in(
        Coord {
            x: 2.0_f64.powi(-55),
            y: 0.0,
        },
        &work,
    )
    .unwrap()
    .x;
    // Fonte C1: GOP-015A, controllo del bound mediante razionali esatti.
    assert!(error.sign().is_gt());
    assert!(error.try_cmp(&half_ulp).unwrap().is_lt());
    // Fonte C1: GOP-015A, intersezione x=1+2^-53 a meta' ulp arrotonda
    // a 1 (pari), collassando il tratto conservato [1,1+2^-53].
    assert!(matches!(
        default_run(
            &line(&[(1.0, 0.0), (2.0, 0.0)]),
            &line(&[(1.0, -1.0), (1.0_f64.next_up(), 1.0)])
        ),
        Err(TrimExtendError::InvalidRoundedTopology)
    ));
    // Fonte C1: GOP-015A, lo stesso midpoint resta pubblicabile quando il
    // vertice vicino e' 0, quindi nessun collasso.
    assert_eq!(
        default_run(
            &line(&[(0.0, 0.0), (2.0, 0.0)]),
            &line(&[(1.0, -1.0), (1.0_f64.next_up(), 1.0)])
        )
        .unwrap()
        .0[1]
            .x
            .to_bits(),
        1.0_f64.to_bits()
    );
    // Fonte C1: GOP-015A, midpoint con mantissa inferiore dispari sale al pari.
    assert_eq!(
        default_run(
            &line(&[(0.0, 0.0), (2.0, 0.0)]),
            &line(&[
                (1.0_f64.next_up(), -1.0),
                (1.0_f64.next_up().next_up(), 1.0)
            ])
        )
        .unwrap()
        .0[1]
            .x
            .to_bits(),
        1.0_f64.next_up().next_up().to_bits()
    );
    // Fonte C1: GOP-015A, anche il pubblicatore comune rifiuta siti distinti fusi.
    let a = P::of_in(Coord { x: 1.0, y: 0.0 }, &work).unwrap();
    let b = P {
        x: a.x.try_add(&half_ulp).unwrap(),
        y: a.y.try_clone().unwrap(),
    };
    assert!(matches!(
        publish(&[a, b], &line(&[]), &mut work),
        Err(TrimExtendError::InvalidRoundedTopology)
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn positive_distance_threshold() {
    let target = line(&[(0.0, 0.0), (1.0, 0.0)]);
    let tiny = f64::from_bits(1);
    // Fonte C1: GOP-018, distanza strettamente positiva, nessun epsilon.
    assert_eq!(
        default_run(&target, &line(&[(-tiny, -1.0), (-tiny, 1.0)]))
            .unwrap()
            .0[0]
            .x
            .to_bits(),
        (-tiny).to_bits()
    );
    // Fonte C1: GOP-018, distanza zero e' contatto terminale unsupported.
    assert!(matches!(
        default_run(&target, &line(&[(0.0, -1.0), (0.0, 1.0)])),
        Err(TrimExtendError::Unsupported)
    ));
    // Fonte C1: GOP-018, appena all'interno si taglia, senza estensione.
    assert_eq!(
        default_run(&target, &line(&[(tiny, -1.0), (tiny, 1.0)])).unwrap(),
        LineString::from(vec![(0.0, 0.0), (tiny, 0.0)])
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_resource_thresholds() {
    let target = line(&[(0.0, 0.0), (2.0, 0.0)]);
    let guide = line(&[(1.0, -1.0), (1.0, 1.0)]);
    let mut measured = Work::new(RelateLimits::default(), None);
    execute(&target, &guide, TrimExtendOptions::default(), &mut measured).unwrap();
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
            let result = trim_extend_with_limits(
                &target,
                &guide,
                TrimExtendOptions::default(),
                limits,
                None,
            );
            // Fonte C1: GOP-026/027 limite esatto incluso, un addebito in piu'
            // fallisce; picco dei byte richiesti vivi, non una stima per vertice.
            if delta < 0 {
                if axis == 3 {
                    assert!(matches!(
                        result,
                        Err(TrimExtendError::Predicate(PredicateError::ReservationLimit))
                    ));
                } else {
                    assert!(matches!(
                        result,
                        Err(TrimExtendError::Predicate(PredicateError::WorkLimit))
                    ));
                }
            } else {
                assert_eq!(
                    result.unwrap(),
                    LineString::from(vec![(0.0, 0.0), (1.0, 0.0)])
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
        trim_extend_with_limits(
            &target,
            &guide,
            TrimExtendOptions::default(),
            RelateLimits::default(),
            Some(&flag)
        ),
        Err(TrimExtendError::Predicate(PredicateError::Cancelled))
    ));
    assert!(matches!(
        trim_extend_with_limits(
            &target,
            &guide,
            TrimExtendOptions::default(),
            RelateLimits {
                deadline: Some(Instant::now()),
                ..RelateLimits::default()
            },
            Some(&flag)
        ),
        Err(TrimExtendError::Predicate(PredicateError::DeadlineExceeded))
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
            trim_extend_with_limits(
                &target,
                &guide,
                TrimExtendOptions::default(),
                RelateLimits::default(),
                Some(&flag)
            ),
            Err(TrimExtendError::Predicate(PredicateError::Cancelled))
        ));
    });
    // Fonte C1: GOP-028, deadline futura scade durante il lavoro.
    assert!(matches!(
        trim_extend_with_limits(
            &target,
            &guide,
            TrimExtendOptions::default(),
            RelateLimits {
                deadline: Some(Instant::now() + Duration::from_millis(20)),
                ..RelateLimits::default()
            },
            None
        ),
        Err(TrimExtendError::Predicate(PredicateError::DeadlineExceeded))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn reused_signs_do_not_charge_candidates_and_overflow_is_typed() {
    let mut work = Work::new(RelateLimits::default(), None);
    let points = [
        P::of_in(Coord { x: 0.0, y: 0.0 }, &work).unwrap(),
        P::of_in(Coord { x: 2.0, y: 0.0 }, &work).unwrap(),
    ];
    let sides = [work.integer(1).unwrap(), work.integer(-1).unwrap()];
    let cut = cuts(&points, &sides, &mut work).unwrap().unwrap();
    // Fonte C1: GOP-026, classificare segni gia' valutati non e' un nuovo
    // test geometrico di coppia. GOP-018: x=1 per interpolazione razionale.
    assert_eq!(work.pairs, 0);
    assert_eq!(
        cut.point,
        P::of_in(Coord { x: 1.0, y: 0.0 }, &work).unwrap()
    );
    // Fonte C1: GOP-015A, raggio y=2^-1074*x incontra y=1 a x=2^1074,
    // non pubblicabile nel dominio binary64 finito: nessuna saturazione.
    assert!(matches!(
        default_run(
            &line(&[(0.0, 0.0), (1.0, f64::from_bits(1))]),
            &line(&[(0.0, 1.0), (1.0, 1.0)])
        ),
        Err(TrimExtendError::CoordinateOutOfRange)
    ));
    // Fonte C1: GOP-018, attraversamento interno in un vertice conserva
    // tutta e soltanto la parte sinistra x<=1, nel verso originale.
    assert_eq!(
        default_run(
            &line(&[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)]),
            &line(&[(1.0, 2.0), (1.0, 3.0)])
        )
        .unwrap(),
        LineString::from(vec![(0.0, 0.0), (1.0, 1.0)])
    );
    // Fonte C1: GOP-026, zero risorse permette zero lavoro, rifiuto tipizzato.
    assert!(matches!(
        trim_extend_with_limits(
            &line(&[(0.0, 0.0), (1.0, 0.0)]),
            &line(&[(2.0, 0.0), (2.0, 1.0)]),
            TrimExtendOptions::default(),
            RelateLimits {
                max_steps: 0,
                ..RelateLimits::default()
            },
            None
        ),
        Err(TrimExtendError::Predicate(PredicateError::WorkLimit))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn degenerate_guide_is_invalid_geometry() {
    assert!(matches!(
        trim_extend_with_limits(
            &line(&[(0., -1.), (0., 1.)]),
            &line(&[(0., 0.), (0., 0.)]),
            TrimExtendOptions::default(),
            RelateLimits::default(),
            None
        ),
        Err(TrimExtendError::InvalidGeometry)
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn resource_prefixes_terminal_extensions_are_atomic() {
    let target = line(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]);
    // GOP-018: rette x=-2 e x=4 incontrano solo il raggio terminale
    // corrispondente; add_vertex conserva anche il vecchio estremo.
    for (x, mode, expected) in [
        (
            -2.0,
            TrimExtendMode::AddVertex,
            vec![(-2.0, 0.0), (0.0, 0.0), (1.0, 0.0), (2.0, 0.0)],
        ),
        (
            4.0,
            TrimExtendMode::AddVertex,
            vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (4.0, 0.0)],
        ),
        (
            -2.0,
            TrimExtendMode::MoveTerminalVertex,
            vec![(-2.0, 0.0), (1.0, 0.0), (2.0, 0.0)],
        ),
        (
            4.0,
            TrimExtendMode::MoveTerminalVertex,
            vec![(0.0, 0.0), (1.0, 0.0), (4.0, 0.0)],
        ),
    ] {
        let guide = line(&[(x, 10.0), (x, 11.0)]);
        let expected = LineString::from(expected);
        let config = TrimExtendOptions {
            ends: TrimExtendEnds::Both,
            mode,
        };
        // GOP-026/027: ogni prefisso del budget deve fallire atomicamente;
        // dopo il primo successo aumentare il budget non cambia la geometria.
        for resource in 0..4 {
            let memory = resource == 1;
            let mut succeeded = false;
            for budget in (0..=4096)
                .map(|value| if memory { value * 16 } else { value })
                .chain((17..=26).map(|power| 1_u64 << power))
            {
                if succeeded && budget < (1_u64 << 26) {
                    continue;
                }
                let mut limits = RelateLimits::default();
                match resource {
                    0 => limits.max_steps = budget,
                    1 => limits.max_reserved_bytes = budget,
                    2 => limits.max_nodes = budget,
                    _ => limits.max_segment_pairs = budget,
                }
                match trim_extend_with_limits(&target, &guide, config, limits, None) {
                    Ok(actual) => {
                        assert_eq!(actual, expected);
                        succeeded = true;
                    }
                    Err(TrimExtendError::Predicate(error)) => {
                        assert!(!succeeded);
                        if memory {
                            assert!(matches!(error, PredicateError::ReservationLimit));
                        } else {
                            assert!(matches!(error, PredicateError::WorkLimit));
                        }
                    }
                    Err(error) => panic!("categoria inattesa: {error:?}"),
                }
            }
            assert!(succeeded);
        }
    }
}
