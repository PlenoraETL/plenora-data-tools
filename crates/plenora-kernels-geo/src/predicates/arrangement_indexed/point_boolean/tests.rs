use super::*;
use wkt::TryFromWkt as _;

fn g(text: &str) -> Geometry<f64> {
    Geometry::try_from_wkt_str(text).unwrap()
}
fn run(a: &Geometry<f64>, b: &Geometry<f64>, op: PointBooleanOp) -> Geometry<f64> {
    boolean_with_limits(a, b, op, None, RelateLimits::default(), None).unwrap()
}

#[test]
fn c1_sets_boundaries_holes_and_closure() {
    let a = g("MULTIPOINT((0 0),(1 0),(0 0))");
    let b = g("POINT(1 0)");
    // Fonte C1: GEO-booleane-estese voce 27-U; OGC SFA 1.2.1 6.1.2.4.
    assert_eq!(
        run(&a, &b, PointBooleanOp::Union),
        g("MULTIPOINT((0 0),(1 0))")
    );
    let p = g("POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,1 3,3 3,3 1,1 1))");
    let sites = g("MULTIPOINT((0 2),(2 2),(1 2),(5 5))");
    // Fonte C1: OGC SFA 1.2.1 6.1.11 e 6.1.15; GOP-014, confine incluso.
    assert_eq!(
        run(&sites, &p, PointBooleanOp::Intersection),
        g("MULTIPOINT((0 2),(1 2))")
    );
    // Fonte C1: GOP-014 differenza insiemistica; il buco e' esterno.
    assert_eq!(
        run(&sites, &p, PointBooleanOp::Difference),
        g("MULTIPOINT((2 2),(5 5))")
    );
    let line = g("LINESTRING(0 0,2 0,2 2)");
    let sites = g("MULTIPOINT((0 0),(1 0),(2 0),(1 1))");
    // Fonte C1: OGC SFA 1.2.1 6.1.7, 6.1.15: vertice e lato appartengono.
    assert_eq!(
        run(&sites, &line, PointBooleanOp::Intersection),
        g("MULTIPOINT((0 0),(1 0),(2 0))")
    );
    // Fonte C1: GOP-015 chiusura dei residui; nessun taglio aperto codificabile.
    assert_eq!(
        run(&line, &sites, PointBooleanOp::Difference),
        g("MULTILINESTRING((0 0,2 0,2 2))")
    );
    // Fonte C1: GOP-014 componenti crescenti e rimozione siti ridondanti.
    assert_eq!(
        run(&sites, &line, PointBooleanOp::Union),
        g("GEOMETRYCOLLECTION(MULTIPOINT((1 1)),MULTILINESTRING((0 0,2 0,2 2)))")
    );
    // Fonte C1: GOP-014 commutativita' insiemistica dell'intersezione.
    assert_eq!(
        run(&line, &sites, PointBooleanOp::Intersection),
        run(&sites, &line, PointBooleanOp::Intersection)
    );
    // Fonte C1: GOP-015 filtro esplicito non estrae il confine.
    assert_eq!(
        boolean_with_limits(
            &sites,
            &line,
            PointBooleanOp::Difference,
            Some(2),
            RelateLimits::default(),
            None
        )
        .unwrap(),
        g("MULTIPOLYGON EMPTY")
    );
    // Fonte C1: GOP-014 vuoto generale.
    assert_eq!(
        run(&a, &a, PointBooleanOp::Difference),
        g("GEOMETRYCOLLECTION EMPTY")
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn exact_extremes_and_differential_oracle() {
    for scale in [f64::from_bits(1), 0.125, 1.0, f64::MAX / 2.0] {
        let line = Geometry::LineString(LineString::from(vec![(-scale, -scale), (scale, scale)]));
        for (x, y) in [(0.0, 0.0), (scale, scale), (scale, 0.0)] {
            let p = Geometry::Point(Point::new(x, y));
            let result = run(&p, &line, PointBooleanOp::Intersection);
            // Fonte C1: intervallo razionale y=x, -scale <= x <= scale.
            let expected = if x == y {
                Geometry::MultiPoint(MultiPoint::new(vec![Point::new(x, y)]))
            } else {
                g("GEOMETRYCOLLECTION EMPTY")
            };
            assert_eq!(result, expected);
            // Oracolo generico indipendente dal selettore: matrice DE-9IM completa.
            let matrix = crate::predicates::relate_matrix(&p, &line).unwrap();
            assert_eq!(
                matches!(result, Geometry::MultiPoint(_)),
                matrix.cells()[0] != b'F' || matrix.cells()[1] != b'F'
            );
        }
    }
    // Fonte C1: GOP-015 uguaglianza topologica degli zeri con segno.
    assert_eq!(
        run(&g("POINT(-0 0)"), &g("POINT(0 -0)"), PointBooleanOp::Union),
        g("MULTIPOINT((0 0))")
    );
}

#[test]
fn ten_thousand_and_sort_oracle() {
    let points = Geometry::MultiPoint(MultiPoint::new(
        (0..10_000)
            .rev()
            .map(|i| Point::new(f64::from(i), 0.0))
            .collect(),
    ));
    let empty = g("MULTIPOINT EMPTY");
    // Fonte C1: GOP-014 identita' con vuoto; ordine lessicografico del profilo.
    let expected = Geometry::MultiPoint(MultiPoint::new(
        (0..10_000).map(|i| Point::new(f64::from(i), 0.0)).collect(),
    ));
    assert_eq!(run(&points, &empty, PointBooleanOp::Union), expected);
    // Fonte C1: GOP-014 idempotenza; esercita l'indice sul multipunto grande.
    assert_eq!(
        run(&points, &points, PointBooleanOp::Intersection),
        expected
    );
}

#[test]
fn exact_limits_and_interruptions() {
    let a = g("MULTIPOINT((1 0),(0 0),(1 0))");
    let b = g("LINESTRING(0 0,2 0)");
    let mut work = Work::new(RelateLimits::default(), None);
    let expected = execute(&a, &b, PointBooleanOp::Union, None, &mut work).unwrap();
    let (steps, bytes, nodes, pairs) = (work.steps, work.peak_bytes(), work.nodes, work.pairs);
    for delta in [0, 1] {
        let limits = RelateLimits {
            max_steps: steps + delta,
            max_reserved_bytes: bytes + delta,
            max_nodes: nodes + delta,
            max_segment_pairs: pairs + delta,
            ..RelateLimits::default()
        };
        // Fonte: RelateLimits, limiti inclusivi misurati sul percorso completo.
        assert_eq!(
            boolean_with_limits(&a, &b, PointBooleanOp::Union, None, limits, None).unwrap(),
            expected
        );
    }
    for limits in [
        RelateLimits {
            max_steps: steps - 1,
            ..RelateLimits::default()
        },
        RelateLimits {
            max_nodes: nodes - 1,
            ..RelateLimits::default()
        },
    ] {
        // Fonte: RelateLimits, rifiuto atomico al primo passo oltre soglia.
        assert!(matches!(
            boolean_with_limits(&a, &b, PointBooleanOp::Union, None, limits, None),
            Err(PointBooleanError::Predicate(PredicateError::WorkLimit))
        ));
    }
    // Fonte: RelateLimits, prenotazioni vive inclusive, nessun risultato parziale.
    assert!(matches!(
        boolean_with_limits(
            &a,
            &b,
            PointBooleanOp::Union,
            None,
            RelateLimits {
                max_reserved_bytes: bytes - 1,
                ..RelateLimits::default()
            },
            None
        ),
        Err(PointBooleanError::Predicate(
            PredicateError::ReservationLimit
        ))
    ));
    let cancelled = AtomicBool::new(true);
    let limits = RelateLimits {
        deadline: Some(std::time::Instant::now()),
        ..RelateLimits::default()
    };
    // Fonte: contratto pubblico relate_matrix_with_limits, precedenza deadline sulla cancellazione.
    assert!(matches!(
        boolean_with_limits(
            &a,
            &b,
            PointBooleanOp::Union,
            None,
            limits,
            Some(&cancelled)
        ),
        Err(PointBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
    // Fonte: RelateLimits deadline monotona inclusiva.
    assert!(matches!(
        boolean_with_limits(&a, &b, PointBooleanOp::Union, None, limits, None),
        Err(PointBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
}

#[test]
fn vertical_sites_default_limits() {
    let p = Geometry::MultiPoint(MultiPoint::new(
        (0..10_000)
            .rev()
            .map(|i| Point::new(0.0, f64::from(i)))
            .collect(),
    ));
    let expected = Geometry::MultiPoint(MultiPoint::new(
        (0..10_000).map(|i| Point::new(0.0, f64::from(i))).collect(),
    ));
    // Fonte C1: GOP-014 idempotenza, ordinamento XY dichiarato dal profilo.
    assert_eq!(run(&p, &p, PointBooleanOp::Intersection), expected);
}

#[test]
fn polygon_union_filters_and_typed_errors() {
    let polygon = g("POLYGON((0 0,4 0,4 4,0 4,0 0))");
    let sites = g("MULTIPOINT((0 2),(2 2),(5 5))");
    // Fonte C1: GOP-014 union rimuove i punti coperti, dimensioni crescenti.
    let expected = g("GEOMETRYCOLLECTION(MULTIPOINT((5 5)),MULTIPOLYGON(((0 0,4 0,4 4,0 4,0 0))))");
    assert_eq!(run(&sites, &polygon, PointBooleanOp::Union), expected);
    assert_eq!(run(&polygon, &sites, PointBooleanOp::Union), expected);
    // Fonte C1: GOP-015 filtro 0 conserva soltanto componenti puntuali residue.
    assert_eq!(
        union_with_limits(&sites, &polygon, Some(0), RelateLimits::default(), None).unwrap(),
        g("MULTIPOINT((5 5))")
    );
    // Fonte C1: GOP-015 filtro invalido e' invalid_configuration.
    assert!(matches!(
        union_with_limits(&sites, &polygon, Some(3), RelateLimits::default(), None),
        Err(PointBooleanError::InvalidOutputDimension)
    ));
    // Fonte: dominio pubblico, nessuna booleana poligonale delegata implicitamente.
    assert!(matches!(
        union_with_limits(&polygon, &polygon, None, RelateLimits::default(), None),
        Err(PointBooleanError::UnsupportedOperands)
    ));
    // Fonte C1: GOP-002 non finiti rifiutati anche con filtro che svuoterebbe l'uscita.
    assert!(matches!(
        union_with_limits(
            &Geometry::Point(Point::new(f64::NAN, 0.0)),
            &polygon,
            Some(1),
            RelateLimits::default(),
            None
        ),
        Err(PointBooleanError::Predicate(
            PredicateError::NonFiniteCoordinate { side: "left" }
        ))
    ));
    let invalid = g("POLYGON((0 0,2 2,0 2,2 0,0 0))");
    // Fonte C1: GOP-002 geometria invalida, attribuzione all'operando originale.
    assert!(matches!(
        union_with_limits(&sites, &invalid, None, RelateLimits::default(), None),
        Err(PointBooleanError::Predicate(
            PredicateError::InvalidGeometry { side: "right", .. }
        ))
    ));
}

#[test]
fn point_sets_against_generic_matrix() {
    for mask_a in 0_u32..16 {
        for mask_b in 0_u32..16 {
            let make = |mask: u32| {
                Geometry::MultiPoint(MultiPoint::new(
                    (0..4)
                        .filter(|i| mask & (1 << i) != 0)
                        .rev()
                        .map(|i| Point::new(0.0, f64::from(i)))
                        .collect(),
                ))
            };
            let (a, b) = (make(mask_a), make(mask_b));
            for (op, mask) in [
                (PointBooleanOp::Union, mask_a | mask_b),
                (PointBooleanOp::Intersection, mask_a & mask_b),
                (PointBooleanOp::Difference, mask_a & !mask_b),
            ] {
                let expected = if mask == 0 {
                    g("GEOMETRYCOLLECTION EMPTY")
                } else {
                    Geometry::MultiPoint(MultiPoint::new(
                        (0..4)
                            .filter(|i| mask & (1 << i) != 0)
                            .map(|i| Point::new(0.0, f64::from(i)))
                            .collect(),
                    ))
                };
                // Fonte C1: GOP-014 algebra di insiemi finiti; oracolo bitset indipendente.
                assert_eq!(run(&a, &b, op), expected);
            }
            for i in 0..4 {
                let p = Geometry::Point(Point::new(0.0, f64::from(i)));
                let matrix = crate::predicates::relate_matrix(&p, &b).unwrap();
                // Oracolo generico: appartenenza da II nella matrice completa.
                assert_eq!(
                    matches!(
                        run(&p, &b, PointBooleanOp::Intersection),
                        Geometry::MultiPoint(_)
                    ),
                    matrix.cells()[0] == b'0'
                );
            }
        }
    }
}

#[test]
fn polygon_pair_budget_exact_threshold() {
    let p = g("POINT(0 0)");
    let polygon = g("POLYGON((0 0,2 0,2 2,0 2,0 0))");
    let mut work = Work::new(RelateLimits::default(), None);
    let expected = execute(&p, &polygon, PointBooleanOp::Union, None, &mut work).unwrap();
    // Fonte: RelateLimits coppie effettive inclusive, validazione compresa.
    assert!(work.pairs > 0);
    assert_eq!(
        union_with_limits(
            &p,
            &polygon,
            None,
            RelateLimits {
                max_segment_pairs: work.pairs,
                ..RelateLimits::default()
            },
            None
        )
        .unwrap(),
        expected
    );
    assert!(matches!(
        union_with_limits(
            &p,
            &polygon,
            None,
            RelateLimits {
                max_segment_pairs: work.pairs - 1,
                ..RelateLimits::default()
            },
            None
        ),
        Err(PointBooleanError::Predicate(PredicateError::WorkLimit))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn original_zero_bits_match_general_boolean() {
    let a = Geometry::Point(Point::new(-0.0, 0.0));
    let b = Geometry::Point(Point::new(0.0, -0.0));
    for (op, general_op) in [
        (
            PointBooleanOp::Union,
            crate::general_boolean::GeneralBooleanOp::Union,
        ),
        (
            PointBooleanOp::Intersection,
            crate::general_boolean::GeneralBooleanOp::Intersection,
        ),
    ] {
        let actual = boolean_with_limits(&a, &b, op, None, RelateLimits::default(), None).unwrap();
        let expected = crate::general_boolean::boolean_with_limits(
            &a,
            &b,
            general_op,
            None,
            crate::general_boolean::GeneralBooleanLimits::default(),
            None,
        )
        .unwrap();
        for output in [actual, expected] {
            let Geometry::MultiPoint(points) = output else {
                panic!("punti attesi")
            };
            assert_eq!(points.0.len(), 1);
            assert_eq!(points.0[0].x().to_bits(), (-0.0_f64).to_bits());
            assert_eq!(points.0[0].y().to_bits(), 0.0_f64.to_bits());
        }
    }
}
