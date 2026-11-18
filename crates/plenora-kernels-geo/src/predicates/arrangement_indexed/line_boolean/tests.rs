use super::*;
use geo::CoordsIter;
use std::time::Instant;
use wkt::TryFromWkt as _;

#[deny(clippy::disallowed_methods)]
fn g(s: &str) -> Geometry<f64> {
    Geometry::try_from_wkt_str(s).unwrap()
}

#[deny(clippy::disallowed_methods)]
fn run(a: &Geometry<f64>, b: &Geometry<f64>, op: LineBooleanOp) -> Geometry<f64> {
    boolean_with_limits(a, b, op, None, RelateLimits::default(), None).unwrap()
}

#[test]
#[deny(clippy::disallowed_methods)]
fn overlaps_crossings_and_direction() {
    let a = g("LINESTRING(0 0,2 0)");
    let b = g("LINESTRING(3 0,1 0)");
    // Fonte C1: GEO-booleane-estese 27-U; SFA 1.2.1 6.1.15.
    // [0,3] rappresentato dai tre tratti atomici dichiarati dal profilo.
    let expected = g("MULTILINESTRING((0 0,1 0),(1 0,2 0),(2 0,3 0))");
    assert_eq!(run(&a, &b, LineBooleanOp::Union), expected);
    assert_eq!(run(&b, &a, LineBooleanOp::Union), expected);
    // Fonte C1: 27-I/27-D e GOP-015, intervalli e chiusura dei residui.
    assert_eq!(
        run(&a, &b, LineBooleanOp::Intersection),
        g("MULTILINESTRING((1 0,2 0))")
    );
    assert_eq!(
        run(&a, &b, LineBooleanOp::Difference),
        g("MULTILINESTRING((0 0,1 0))")
    );
    assert_eq!(
        run(&b, &a, LineBooleanOp::Difference),
        g("MULTILINESTRING((2 0,3 0))")
    );
    let a = g("LINESTRING(-1 0,1 0)");
    let b = g("LINESTRING(0 -1,0 1)");
    // Fonte C1: 27-U croce nodata; GOP-015 intersezione puntuale senza filtro.
    assert_eq!(
        run(&a, &b, LineBooleanOp::Union),
        g("MULTILINESTRING((-1 0,0 0),(0 -1,0 0),(0 0,0 1),(0 0,1 0))")
    );
    assert_eq!(
        run(&a, &b, LineBooleanOp::Intersection),
        g("MULTIPOINT((0 0))")
    );
    assert_eq!(
        intersection_with_limits(&a, &b, Some(1), RelateLimits::default(), None).unwrap(),
        g("MULTILINESTRING EMPTY")
    );
    // Fonte C1: GOP-015 sottrarre un punto non apre gli estremi rappresentati.
    assert_eq!(
        run(&a, &b, LineBooleanOp::Difference),
        g("MULTILINESTRING((-1 0,0 0),(0 0,1 0))")
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn polygon_hole_boundary_touch_and_mixed_union() {
    let polygon = g("POLYGON((0 0,6 0,6 6,0 6,0 0),(2 2,2 4,4 4,4 2,2 2))");
    let line = g("LINESTRING(-1 3,7 3)");
    // Fonte C1: 27-I/27-D, GOP-014/015; intervalli analitici del foro (2,4).
    assert_eq!(
        run(&line, &polygon, LineBooleanOp::Intersection),
        g("MULTILINESTRING((0 3,2 3),(4 3,6 3))")
    );
    let outside = g("MULTILINESTRING((-1 3,0 3),(2 3,4 3),(6 3,7 3))");
    assert_eq!(run(&line, &polygon, LineBooleanOp::Difference), outside);
    let Geometry::Polygon(p) = &polygon else {
        panic!("poligono");
    };
    let higher = Geometry::MultiPolygon(MultiPolygon::new(vec![p.clone()]));
    // Fonte C1: GOP-015 chiusura; il taglio di dimensione inferiore conserva l'area.
    assert_eq!(run(&polygon, &line, LineBooleanOp::Difference), higher);
    // Fonte C1: GOP-014 unione mista, linee esterne prima del poligono.
    let expected = Geometry::GeometryCollection(GeometryCollection(vec![outside, higher]));
    assert_eq!(run(&line, &polygon, LineBooleanOp::Union), expected);
    assert_eq!(run(&polygon, &line, LineBooleanOp::Union), expected);
    let boundary = g("LINESTRING(0 1,0 5)");
    // Fonte C1: SFA 1.2.1 6.1.15, confine incluso nell'intersezione.
    assert_eq!(
        run(&boundary, &polygon, LineBooleanOp::Intersection),
        g("MULTILINESTRING((0 1,0 5))")
    );
    assert_eq!(
        run(&boundary, &polygon, LineBooleanOp::Difference),
        g("GEOMETRYCOLLECTION EMPTY")
    );
    let touch = g("LINESTRING(-1 1,0 0,1 -1)");
    // Fonte C1: GOP-015 contatto isolato; filtro 1 non estrae un tratto degenere.
    assert_eq!(
        run(&touch, &polygon, LineBooleanOp::Intersection),
        g("MULTIPOINT((0 0))")
    );
    assert_eq!(
        intersection_with_limits(&touch, &polygon, Some(1), RelateLimits::default(), None).unwrap(),
        g("MULTILINESTRING EMPTY")
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn grids_and_large_sparse_default_limits() {
    for (n, m) in [(2, 3), (5, 7), (12, 20)] {
        let a = Geometry::MultiLineString(MultiLineString::new(
            (1..=n)
                .map(|y| {
                    LineString::from(vec![(0., f64::from(y)), (f64::from(m + 1), f64::from(y))])
                })
                .collect(),
        ));
        let b = Geometry::MultiLineString(MultiLineString::new(
            (1..=m)
                .map(|x| {
                    LineString::from(vec![(f64::from(x), 0.), (f64::from(x), f64::from(n + 1))])
                })
                .collect(),
        ));
        let Geometry::MultiLineString(lines) = run(&a, &b, LineBooleanOp::Union) else {
            panic!("linee");
        };
        // Fonte C1: 27-U, oracolo reticolo: n(m+1)+m(n+1) tratti atomici.
        assert_eq!(
            lines.0.len(),
            usize::try_from(n * (m + 1) + m * (n + 1)).unwrap()
        );
        let Geometry::MultiPoint(points) = run(&a, &b, LineBooleanOp::Intersection) else {
            panic!("punti");
        };
        // Fonte C1: GOP-015; nm incroci isolati distinti.
        assert_eq!(points.0.len(), usize::try_from(n * m).unwrap());
    }
    let lines = Geometry::MultiLineString(MultiLineString::new(
        (0..10_002)
            .map(|i| {
                let x = f64::from(i) * 6.;
                LineString::from(vec![(x, 0.), (x + 1., 1.)])
            })
            .collect(),
    ));
    let other = Geometry::MultiLineString(MultiLineString::new(
        (0..10_002)
            .map(|i| {
                let x = f64::from(i * 6 + 3);
                LineString::from(vec![(x, 0.), (x + 1., 1.)])
            })
            .collect(),
    ));
    let Geometry::MultiLineString(result) = run(&lines, &other, LineBooleanOp::Union) else {
        panic!("linee");
    };
    // Fonte C1: GOP-026, nessuna ammissione per prodotto; intervalli disgiunti.
    assert_eq!(result.0.len(), 20_004);
    let flag = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        // Fonte C1: GOP-028 interruzione cooperativa di un calcolo in corso.
        assert!(matches!(
            union_with_limits(&lines, &other, None, RelateLimits::default(), Some(&flag)),
            Err(LineBooleanError::Predicate(PredicateError::Cancelled))
        ));
        worker.join().unwrap();
    });
    let limits = RelateLimits {
        deadline: Some(Instant::now() + std::time::Duration::from_millis(10)),
        ..RelateLimits::default()
    };
    // Fonte C1: GOP-028 deadline durante lavoro; nessuna uscita parziale.
    assert!(matches!(
        union_with_limits(&lines, &other, None, limits, None),
        Err(LineBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn points_empty_filters_and_validation() {
    let line = g("MULTILINESTRING((0 0,2 0),(5 0,6 0))");
    let points = g("MULTIPOINT((1 0),(3 0),(3 0))");
    // Fonte C1: GOP-014 componenti inferiori coperte eliminate e multi ordinati.
    assert_eq!(
        run(&line, &points, LineBooleanOp::Union),
        g("GEOMETRYCOLLECTION(MULTIPOINT((3 0)),MULTILINESTRING((0 0,2 0),(5 0,6 0)))")
    );
    assert_eq!(
        run(&line, &points, LineBooleanOp::Intersection),
        g("MULTIPOINT((1 0))")
    );
    assert_eq!(
        run(&points, &line, LineBooleanOp::Difference),
        g("MULTIPOINT((3 0))")
    );
    assert_eq!(run(&line, &points, LineBooleanOp::Difference), line);
    for (dim, empty) in [
        (0, "MULTIPOINT EMPTY"),
        (1, "MULTILINESTRING EMPTY"),
        (2, "MULTIPOLYGON EMPTY"),
    ] {
        // Fonte C1: GOP-015 filtro conserva componenti, vuoto multi della dimensione.
        assert_eq!(
            difference_with_limits(&line, &line, Some(dim), RelateLimits::default(), None).unwrap(),
            g(empty)
        );
    }
    // Fonte C1: GOP-015 filtro invalido; GOP-002 validazione anche se output filtrato.
    assert!(matches!(
        union_with_limits(&line, &points, Some(3), RelateLimits::default(), None),
        Err(LineBooleanError::InvalidOutputDimension)
    ));
    assert!(matches!(
        run_error(&g("LINESTRING(0 0,0 0)"), &line),
        LineBooleanError::Predicate(PredicateError::InvalidGeometry { side: "left", .. })
    ));
    assert!(matches!(
        run_error(&line, &Geometry::Point(Point::new(f64::NAN, 0.))),
        LineBooleanError::Predicate(PredicateError::NonFiniteCoordinate { side: "right" })
    ));
    let polygon = g("POLYGON((0 0,2 2,0 2,2 0,0 0))");
    assert!(matches!(
        run_error(&line, &polygon),
        LineBooleanError::Predicate(PredicateError::InvalidGeometry { side: "right", .. })
    ));
    // Fonte: dominio additivo, nessuna delega implicita alle booleane poligonali.
    assert!(matches!(
        run_error(&polygon, &polygon),
        LineBooleanError::UnsupportedOperands
    ));
}

#[deny(clippy::disallowed_methods)]
fn run_error(a: &Geometry<f64>, b: &Geometry<f64>) -> LineBooleanError {
    match union_with_limits(a, b, None, RelateLimits::default(), None) {
        Err(error) => error,
        Ok(_) => panic!("atteso rifiuto tipizzato"),
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_thresholds_cancellation_and_deadline() {
    let a = g("LINESTRING(0 0,4 4)");
    let b = g("LINESTRING(0 4,4 0)");
    let mut work = Work::new(RelateLimits::default(), None);
    let expected = execute(&a, &b, LineBooleanOp::Union, None, &mut work).unwrap();
    for field in 0..4 {
        let exact = [work.steps, work.nodes, work.pairs, work.peak_bytes()][field];
        for delta in [-1_i64, 0, 1] {
            let value = exact.checked_add_signed(delta).unwrap();
            let mut limits = RelateLimits::default();
            match field {
                0 => limits.max_steps = value,
                1 => limits.max_nodes = value,
                2 => limits.max_segment_pairs = value,
                _ => limits.max_reserved_bytes = value,
            }
            let result = union_with_limits(&a, &b, None, limits, None);
            // Fonte C1: GOP-026/027, soglia inclusiva sul lavoro e sui byte effettivi.
            if delta >= 0 {
                assert_eq!(result.unwrap(), expected);
            } else if field == 3 {
                assert!(matches!(
                    result,
                    Err(LineBooleanError::Predicate(
                        PredicateError::ReservationLimit
                    ))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(LineBooleanError::Predicate(PredicateError::WorkLimit))
                ));
            }
        }
    }
    let cancelled = AtomicBool::new(true);
    let limits = RelateLimits {
        deadline: Some(Instant::now()),
        ..RelateLimits::default()
    };
    // Fonte: politica locale RelateLimits e 1c-a, cancellazione prima della deadline.
    assert!(matches!(
        union_with_limits(&a, &b, None, limits, Some(&cancelled)),
        Err(LineBooleanError::Predicate(PredicateError::Cancelled))
    ));
    assert!(matches!(
        union_with_limits(&a, &b, None, limits, None),
        Err(LineBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn exact_conversion_and_rounded_node() {
    for x in [
        0.,
        -0.,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MIN_POSITIVE,
        -f64::MAX,
        f64::MAX,
        1.5,
        -1.5,
    ] {
        let mut work = Work::new(RelateLimits::default(), None);
        let p = P::of_in(Coord { x, y: x }, &work).unwrap();
        let c = coordinate(&p, &mut work).unwrap();
        // Fonte: IEEE 754 binary64, roundtrip esatto e zero canonico del profilo.
        assert_eq!(c.x.to_bits(), if x == 0. { 0 } else { x.to_bits() });
    }
    let a = g("LINESTRING(0 0,1 1)");
    let b = g("LINESTRING(0 1,1 -1)");
    // GOP-015A: intersezione analitica (1/3,1/3), pubblicata nearest-even.
    let result = intersection_with_limits(&a, &b, None, RelateLimits::default(), None).unwrap();
    assert_eq!(
        result,
        Geometry::MultiPoint(MultiPoint::new(vec![Point::new(1. / 3., 1. / 3.)]))
    );
    let result = run(&a, &b, LineBooleanOp::Union);
    assert_eq!(result.coords_count(), 8);
}

#[test]
#[deny(clippy::disallowed_methods)]
fn interval_oracle_and_indexed_classification_against_generic() {
    for lo in -2..=2 {
        for hi in lo + 1..=3 {
            let a = Geometry::LineString(LineString::from(vec![(0., 0.), (2., 0.)]));
            let b = Geometry::LineString(LineString::from(vec![
                (f64::from(lo), 0.),
                (f64::from(hi), 0.),
            ]));
            for op in [
                LineBooleanOp::Union,
                LineBooleanOp::Intersection,
                LineBooleanOp::Difference,
            ] {
                let result = run(&a, &b, op);
                for i in -5..=7 {
                    let x = f64::from(i) / 2.;
                    if [0., 2., f64::from(lo), f64::from(hi)].contains(&x) {
                        continue;
                    }
                    let site = Geometry::Point(Point::new(x, 0.));
                    let matrix = crate::predicates::arrangement_exhaustive::relate(
                        &site,
                        &result,
                        &mut Work::new(RelateLimits::default(), None),
                    )
                    .unwrap();
                    // Oracolo differenziale: classificazione indicizzata e percorso esaustivo.
                    assert_eq!(
                        crate::predicates::relate_matrix(&site, &result).unwrap(),
                        matrix
                    );
                    let covered = matrix.cells()[0] != b'F' || matrix.cells()[1] != b'F';
                    // Fonte C1: 27-U/I/D intervalli analitici; oracolo generico DE-9IM.
                    assert_eq!(
                        covered,
                        selected(
                            op,
                            (0. ..=2.).contains(&x),
                            (f64::from(lo)..=f64::from(hi)).contains(&x)
                        )
                    );
                }
                // Fonte: ordine canonico dichiarato, invariabile al verso delle parti.
                let mut reverse = b.clone();
                if let Geometry::LineString(line) = &mut reverse {
                    line.0.reverse();
                }
                assert_eq!(run(&a, &reverse, op), result);
                assert!(result
                    .coords_iter()
                    .all(|c| c.x.is_finite() && c.y.is_finite()));
            }
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn isolated_contact_beside_overlap_and_retraced_parts() {
    let a = g("MULTILINESTRING((0 0,3 0,0 0),(5 0,7 0))");
    let b = g("MULTILINESTRING((1 0,2 0),(6 -1,6 1))");
    // Fonte C1: GOP-014/015 componenti puntuali isolate e overlap una sola volta.
    assert_eq!(
        run(&a, &b, LineBooleanOp::Intersection),
        g("GEOMETRYCOLLECTION(MULTIPOINT((6 0)),MULTILINESTRING((1 0,2 0)))")
    );
    assert_eq!(
        intersection_with_limits(&a, &b, Some(0), RelateLimits::default(), None).unwrap(),
        g("MULTIPOINT((6 0))")
    );
    assert_eq!(
        intersection_with_limits(&a, &b, Some(1), RelateLimits::default(), None).unwrap(),
        g("MULTILINESTRING((1 0,2 0))")
    );
    // Fonte C1: GOP-015, [0,3] meno [1,2] conserva la chiusura dei residui.
    assert_eq!(
        run(
            &g("LINESTRING(0 0,3 0)"),
            &g("LINESTRING(1 0,2 0)"),
            LineBooleanOp::Difference
        ),
        g("MULTILINESTRING((0 0,1 0),(2 0,3 0))")
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn nearest_even_rational_certificate_and_ties() {
    let mut work = Work::new(RelateLimits::default(), None);
    for numerator in [-7, -2, -1, 1, 2, 7] {
        for denominator in [3, 5, 11] {
            let exact = work
                .integer(numerator)
                .unwrap()
                .try_div(&work.integer(denominator).unwrap())
                .unwrap();
            let value = ordinate(&exact, &mut work).unwrap();
            let center = P::of_in(Coord { x: value, y: 0. }, &work).unwrap().x;
            // Oracolo in razionale: distanza entro entrambi i punti medi vicini.
            let magnitude = value.abs().to_bits();
            let sign = if value.is_sign_negative() { -1. } else { 1. };
            for bits in [magnitude - 1, magnitude + 1] {
                let neighbor = P::of_in(
                    Coord {
                        x: sign * f64::from_bits(bits),
                        y: 0.,
                    },
                    &work,
                )
                .unwrap()
                .x;
                let midpoint = center.try_midpoint(&neighbor).unwrap();
                assert_eq!(
                    exact.try_cmp(&midpoint).unwrap(),
                    center.try_cmp(&midpoint).unwrap()
                );
            }
        }
    }
    // IEEE 754: entrambi i versi del pareggio, subnormali e cambio binade.
    for bits in [
        0,
        1,
        2,
        f64::MIN_POSITIVE.to_bits() - 1,
        1_f64.to_bits() - 1,
        1_f64.to_bits(),
        1_f64.to_bits() + 1,
        f64::MAX.to_bits() - 1,
    ] {
        for sign in [1., -1.] {
            let a = P::of_in(
                Coord {
                    x: sign * f64::from_bits(bits),
                    y: 0.,
                },
                &work,
            )
            .unwrap()
            .x;
            let b = P::of_in(
                Coord {
                    x: sign * f64::from_bits(bits + 1),
                    y: 0.,
                },
                &work,
            )
            .unwrap()
            .x;
            let exact = a.try_midpoint(&b).unwrap();
            let expected = sign * f64::from_bits(if bits & 1 == 0 { bits } else { bits + 1 });
            assert_eq!(
                ordinate(&exact, &mut work).unwrap().to_bits(),
                expected.to_bits()
            );
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn input_bits_and_rounding_collapse() {
    let a = Geometry::LineString(LineString::from(vec![(-0., -0.), (1., 1.)]));
    let b = g("LINESTRING(0 1,1 -1)");
    let result = run(&a, &b, LineBooleanOp::Union);
    // GOP-015A: anche i bit dello zero d'ingresso sopravvivono alla ricomposizione.
    assert!(result
        .coords_iter()
        .any(|c| c.x.to_bits() == (-0_f64).to_bits() && c.y.to_bits() == (-0_f64).to_bits()));
    for input in a.coords_iter().chain(b.coords_iter()) {
        assert!(result
            .coords_iter()
            .any(|c| c.x.to_bits() == input.x.to_bits() && c.y.to_bits() == input.y.to_bits()));
    }
    // Un incrocio a meta' del minimo subnormale collassa nell'origine.
    let tiny = f64::from_bits(1);
    let a = Geometry::LineString(LineString::from(vec![(0., 0.), (tiny, tiny)]));
    let b = Geometry::LineString(LineString::from(vec![(0., tiny), (tiny, 0.)]));
    assert!(matches!(
        run_error(&a, &b),
        LineBooleanError::InvalidRoundedTopology
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn publication_rejects_manually_altered_graphs() {
    let mut work = Work::new(RelateLimits::default(), None);
    let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
    let originals = Originals::new(&empty, &empty, &mut work).unwrap();
    let edge = Edge {
        a: P::of_in((0., 0.).into(), &work).unwrap(),
        b: P::of_in((1., 1.).into(), &work).unwrap(),
    };
    let valid = PublishedEdge::new(&edge, [(0., 0.).into(), (1., 1.).into()], &mut work).unwrap();
    verify_publication(&[valid], &originals, &mut work).unwrap();
    for coords in [
        [(0., 0.), (2., 1.)],
        [(0., 0.), (0., 0.)],
        [(1., 1.), (0., 0.)],
    ] {
        let altered = PublishedEdge::new(&edge, coords.map(Into::into), &mut work).unwrap();
        assert!(matches!(
            verify_publication(&[altered], &originals, &mut work),
            Err(LineBooleanError::InvalidRoundedTopology)
        ));
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn rounded_new_contact_and_unpublished_boundary_nodes() {
    let mut work = Work::new(RelateLimits::default(), None);
    let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
    let originals = Originals::new(&empty, &empty, &mut work).unwrap();
    let a = Edge {
        a: P::of_in((0., 0.).into(), &work).unwrap(),
        b: P::of_in((2., 0.).into(), &work).unwrap(),
    };
    let mut b = Edge {
        a: P::of_in((1., f64::from_bits(1)).into(), &work).unwrap(),
        b: P::of_in((1., 1.).into(), &work).unwrap(),
    };
    b.a.y = b.a.y.try_div(&work.integer(3).unwrap()).unwrap();
    let ca = [
        coordinate(&a.a, &mut work).unwrap(),
        coordinate(&a.b, &mut work).unwrap(),
    ];
    let cb = [
        coordinate(&b.a, &mut work).unwrap(),
        coordinate(&b.b, &mut work).unwrap(),
    ];
    let publications = [
        PublishedEdge::new(&a, ca, &mut work).unwrap(),
        PublishedEdge::new(&b, cb, &mut work).unwrap(),
    ];
    // GOP-015A: rounding corretto, ma crea un contatto nell'interno di a.
    assert!(matches!(
        verify_publication(&publications, &originals, &mut work),
        Err(LineBooleanError::InvalidRoundedTopology)
    ));
    let polygon = g("POLYGON((0 1,1 -1,1 1,0 1))");
    let line = g("LINESTRING(0 0,1 1)");
    let Geometry::Polygon(p) = polygon.clone() else {
        panic!("poligono");
    };
    let expected = Geometry::MultiPolygon(MultiPolygon::new(vec![p]));
    // GOP-015: area meno linea e filtro 2 non pubblicano il nodo (1/3,1/3).
    assert_eq!(run(&polygon, &line, LineBooleanOp::Difference), expected);
    assert_eq!(
        union_with_limits(&polygon, &line, Some(2), RelateLimits::default(), None).unwrap(),
        expected
    );
    for slope in [3, 4, 6] {
        let line = Geometry::LineString(LineString::from(vec![
            (0., 1.),
            (1., 1. - f64::from(slope)),
        ]));
        let actual = run(
            &g("LINESTRING(0 0,1 1)"),
            &line,
            LineBooleanOp::Intersection,
        );
        let value = 1. / f64::from(slope + 1);
        assert_eq!(
            actual,
            Geometry::MultiPoint(MultiPoint::new(vec![Point::new(value, value)]))
        );
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn published_order_matches_generic_sort_and_signed_zero_mutation_fails() {
    let next = f64::from_bits(1_f64.to_bits() + 1);
    let a = Geometry::LineString(LineString::from(vec![(1., 4.), (next, -8.)]));
    let b = g("MULTILINESTRING((0 1,2 1),(0 0,2 0))");
    assert_eq!(
        run(&a, &b, LineBooleanOp::Intersection),
        g("MULTIPOINT((1 0),(1 1))")
    );
    // Oracolo generico dell'ordinamento: razionali esatti dei binary64 pubblicati.
    let mut work = Work::new(RelateLimits::default(), None);
    for n in 0..32 {
        let mut values = CountedVec::new();
        for i in (0..n).rev() {
            work.push(
                &mut values,
                Coord {
                    x: f64::from(i % 3),
                    y: f64::from(i),
                },
            )
            .unwrap();
        }
        let mut expected = values.iter().copied().collect::<Vec<_>>();
        expected.sort_by(|a, b| {
            point_cmp(&P::of_in(*a, &work).unwrap(), &P::of_in(*b, &work).unwrap()).unwrap()
        });
        sort_output(&mut values, |a, b| coordinate_cmp(*a, *b), &mut work).unwrap();
        assert_eq!(&*values, expected.as_slice());
    }
    let input = Geometry::Point(Point::new(-0., 0.));
    let originals = Originals::new(&input, &input, &mut work).unwrap();
    let p = P::of_in((-0., 0.).into(), &work).unwrap();
    let altered = PublishedEdge::point(&p, (0., 0.).into(), &mut work).unwrap();
    assert!(matches!(
        verify_publication(&[altered], &originals, &mut work),
        Err(LineBooleanError::InvalidRoundedTopology)
    ));
}
