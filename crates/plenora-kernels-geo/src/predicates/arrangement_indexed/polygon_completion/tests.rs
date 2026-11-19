//! Attese GOP-020/015A/026/027/028 e OGC SFA 1.2.1, 6.1.11.
use super::*;
use std::time::{Duration, Instant};

fn geometry(text: &str) -> Geometry<f64> {
    use wkt::TryFromWkt;
    Geometry::try_from_wkt_str(text).expect("fixture WKT")
}
fn polygon(text: &str) -> Polygon<f64> {
    let Geometry::Polygon(p) = geometry(text) else {
        panic!("fixture poligonale")
    };
    p
}
fn base() -> Polygon<f64> {
    polygon("POLYGON((0 0,2 0,2 2,0 2,0 0))")
}
fn extension() -> Geometry<f64> {
    geometry("LINESTRING(2 0,4 0,4 2,2 2)")
}
fn run(polygons: &[Polygon<f64>], lines: &Geometry<f64>) -> Vec<Polygon<f64>> {
    polygon_completion_with_limits(polygons, lines, RelateLimits::default(), None)
        .expect("fixture completamento")
}
fn valid(polygons: &[Polygon<f64>]) {
    use geo::Validation;
    for polygon in polygons {
        // Fonte OGC SFA 1.2.1, 6.1.11: validatore indipendente dal kernel.
        assert!(polygon.is_valid());
    }
}

#[test]
fn one_and_two_faces_canonical_and_valid() {
    let one = run(&[base()], &extension());
    // Fonte GOP-020: unica faccia non occupata con lato esistente x=2.
    assert_eq!(one, vec![polygon("POLYGON((2 0,4 0,4 2,2 2,2 0))")]);
    let lines = geometry("MULTILINESTRING((2 0,4 0,4 2,2 2),(0 2,-2 2,-2 0,0 0))");
    let two = run(&[base()], &lines);
    // Fonte ordine GOP-020: minimo (-2,0) precede (2,0), gusci antiorari.
    assert_eq!(
        two,
        vec![polygon("POLYGON((-2 0,0 0,0 2,-2 2,-2 0))"), one[0].clone()]
    );
    valid(&two);
}

#[test]
fn point_contacts_residuals_and_line_only_faces() {
    for lines in [
        "LINESTRING(2 2,4 2,4 4,2 2)",
        "LINESTRING(2 1,5 1)",
        "LINESTRING(5 5,7 5,7 7,5 5)",
    ] {
        // Fonte GOP-020: punto non e' un lato positivo; residui e sole linee esclusi.
        assert!(run(&[base()], &geometry(lines)).is_empty());
    }
    let lines = geometry("MULTILINESTRING((2 0,4 0,4 2,2 2),(1 1,1 4))");
    // Fonte GOP-020: il residuo aperto non cambia la faccia adiacente.
    assert_eq!(run(&[base()], &lines), run(&[base()], &extension()));
}

#[test]
fn occupied_empty_and_preexisting_holes() {
    // Fonte GOP-020: nessuna linea e nessun poligono danno nessuna faccia qualificata.
    assert!(run(&[base()], &geometry("LINESTRING EMPTY")).is_empty());
    assert!(run(&[], &extension()).is_empty());
    assert!(run(&[base()], &geometry("LINESTRING(0 1,2 1)")).is_empty());
    let p = polygon("POLYGON((0 0,8 0,8 8,0 8,0 0),(2 2,2 6,6 6,6 2,2 2))");
    // Fonte GOP-020: il buco preesistente non adiacente a nuovi lati e' escluso.
    assert!(run(std::slice::from_ref(&p), &geometry("LINESTRING(9 0,9 8)")).is_empty());
    let out = run(&[p], &geometry("LINESTRING(2 4,6 4)"));
    // Fonte GOP-020: il taglio del buco delimita esattamente due facce non occupate.
    assert_eq!(
        out,
        vec![
            polygon("POLYGON((2 2,6 2,6 4,2 4,2 2))"),
            polygon("POLYGON((2 4,6 4,6 6,2 6,2 4))")
        ]
    );
    valid(&out);
}

#[test]
fn holes_are_clockwise_sorted_and_input_permutation_invariant() {
    let outer = polygon("POLYGON((0 0,10 0,10 10,0 10,0 0),(1 1,1 9,9 9,9 1,1 1))");
    let a = polygon("POLYGON((3 3,4 3,4 4,3 4,3 3))");
    let b = polygon("POLYGON((6 3,7 3,7 4,6 4,6 3))");
    let lines = geometry("LINESTRING(1 2,9 2)");
    let result = run(&[outer.clone(), b.clone(), a.clone()], &lines);
    // Fonte GOP-020: faccia superiore con due isole occupate, buchi orari per minimo XY.
    assert_eq!(
        result[1],
        polygon("POLYGON((1 2,9 2,9 9,1 9,1 2),(3 3,3 4,4 4,4 3,3 3),(6 3,6 4,7 4,7 3,6 3))")
    );
    assert_eq!(
        result,
        run(&[a, outer, b], &geometry("LINESTRING(9 2,1 2)"))
    );
    valid(&result);
}

#[test]
fn unsupported_provenance_overlap_and_nonsimple_lines() {
    // Fonte GOP-020: solo la coincidenza nuovo/esistente ha provenienza ambigua.
    assert!(matches!(
        polygon_completion_with_limits(
            &[base()],
            &geometry("LINESTRING(0 0,2 0)"),
            RelateLimits::default(),
            None
        ),
        Err(PolygonCompletionError::AmbiguousProvenance)
    ));
    // GOP-020 richiede linework semplice: parti nuove sovrapposte sono escluse.
    assert!(matches!(
        polygon_completion_with_limits(
            &[base()],
            &geometry("MULTILINESTRING((2 0,4 0),(4 0,2 0))"),
            RelateLimits::default(),
            None
        ),
        Err(PolygonCompletionError::UnsupportedLinework)
    ));
    for other in [base(), polygon("POLYGON((1 1,3 1,3 3,1 3,1 1))")] {
        // Fonte GOP-020: interni sovrapposti, incluso duplicato, sono unsupported.
        assert!(matches!(
            polygon_completion_with_limits(
                &[base(), other],
                &extension(),
                RelateLimits::default(),
                None
            ),
            Err(PolygonCompletionError::OverlappingInteriors)
        ));
    }
    // Fonte GOP-020: linework semplice, nessun repair del bow-tie.
    assert!(matches!(
        polygon_completion_with_limits(
            &[base()],
            &geometry("LINESTRING(3 0,5 2,3 2,5 0)"),
            RelateLimits::default(),
            None
        ),
        Err(PolygonCompletionError::UnsupportedLinework)
    ));
}

#[test]
fn invalid_inputs_are_typed_and_without_data() {
    let bad = polygon("POLYGON((0 0,2 2,0 2,2 0,0 0))");
    // Fonte GOP-002/029: poligono invalido e' data_mapping, non unsupported.
    assert!(matches!(
        polygon_completion_with_limits(&[bad], &extension(), RelateLimits::default(), None),
        Err(PolygonCompletionError::Predicate(
            PredicateError::InvalidGeometry { .. }
        ))
    ));
    assert!(matches!(
        polygon_completion_with_limits(
            &[],
            &geometry("POINT(12345 67890)"),
            RelateLimits::default(),
            None
        ),
        Err(PolygonCompletionError::UnsupportedLinework)
    ));
    let err = polygon_completion_with_limits(
        &[],
        &geometry("POINT(12345 67890)"),
        RelateLimits::default(),
        None,
    )
    .expect_err("fixture tipo");
    // Fonte GOP-029: nessun valore dell'ingresso nei messaggi.
    assert!(!err.to_string().contains("12345"));
}

#[test]
fn rational_constructed_coordinates_and_original_bits() {
    let p = polygon("POLYGON((0 0,1 0,1 2,0 2,0 0))");
    let result = run(&[p], &geometry("LINESTRING(0 0,3 1,3 2,1 2)"));
    // Fonte GOP-015A: y=x/3 interseca x=1 in (1,1/3), nearest-even.
    assert_eq!(result[0].exterior().0[0], geo::Coord { x: 1., y: 1. / 3. });
    let work = Work::new(RelateLimits::default(), None);
    let exact = work
        .integer(1)
        .expect("fixture uno")
        .try_div(&work.integer(3).expect("fixture tre"))
        .expect("fixture terzo");
    let rounded = result[0].exterior().0[0].y;
    let before = P::of_in((0., f64::from_bits(rounded.to_bits() - 1)).into(), &work)
        .expect("fixture prima")
        .y;
    let after = P::of_in((0., f64::from_bits(rounded.to_bits() + 1)).into(), &work)
        .expect("fixture dopo")
        .y;
    let center = P::of_in((0., rounded).into(), &work)
        .expect("fixture centro")
        .y;
    // Fonte IEEE 754 nearest-even: l'esatto sta nella cella dei due punti medi.
    assert!(exact
        .try_cmp(&before.try_midpoint(&center).expect("fixture medio"))
        .expect("fixture ordine")
        .is_gt());
    assert!(exact
        .try_cmp(&after.try_midpoint(&center).expect("fixture medio"))
        .expect("fixture ordine")
        .is_lt());
    valid(&result);
    let mut p = base();
    p.exterior_mut(|ring| ring.0[1].y = -0.0);
    let result = run(&[p], &extension());
    // Fonte GOP-015A: il primo sito originale (2,-0) conserva i bit.
    assert_eq!(result[0].exterior().0[0].y.to_bits(), (-0.0_f64).to_bits());
}

#[test]
fn exact_resource_thresholds() {
    let polygons = [base()];
    let lines = extension();
    let mut measured = Work::new(RelateLimits::default(), None);
    execute(&polygons, &lines, &mut measured).expect("fixture misura");
    for (which, threshold) in [
        (0, measured.steps),
        (1, measured.nodes),
        (2, measured.pairs),
        (3, measured.peak_bytes()),
    ] {
        for delta in [-1_i64, 0, 1] {
            let value = threshold.checked_add_signed(delta).expect("fixture soglia");
            let mut limits = RelateLimits::default();
            match which {
                0 => limits.max_steps = value,
                1 => limits.max_nodes = value,
                2 => limits.max_segment_pairs = value,
                _ => limits.max_reserved_bytes = value,
            }
            let result = polygon_completion_with_limits(&polygons, &lines, limits, None);
            // Fonte GOP-026/027: soglia esatta ammessa; il precedente addebito fallisce.
            if delta < 0 {
                if which == 3 {
                    assert!(matches!(
                        result,
                        Err(PolygonCompletionError::Predicate(
                            PredicateError::ReservationLimit
                        ))
                    ));
                } else {
                    assert!(matches!(
                        result,
                        Err(PolygonCompletionError::Predicate(PredicateError::WorkLimit))
                    ));
                }
            } else {
                assert_eq!(result.expect("soglia ammessa"), run(&polygons, &lines));
            }
        }
    }
}

#[test]
fn deadline_precedence_is_shared() {
    let flag = AtomicBool::new(true);
    let limits = RelateLimits {
        deadline: Some(Instant::now()),
        ..RelateLimits::default()
    };
    // Fonte GOP-028: timeout precede cancellazione osservata allo stesso checkpoint.
    assert!(matches!(
        polygon_completion_with_limits(&[], &geometry("LINESTRING EMPTY"), limits, Some(&flag)),
        Err(PolygonCompletionError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
    // GOP-028: un solo costruttore impone la medesima precedenza.
    assert!(matches!(
        Work::new(limits, Some(&flag)).step(),
        Err(PredicateError::DeadlineExceeded)
    ));
    assert!(matches!(
        polygon_completion_with_limits(
            &[],
            &geometry("LINESTRING EMPTY"),
            RelateLimits::default(),
            Some(&flag)
        ),
        Err(PolygonCompletionError::Predicate(PredicateError::Cancelled))
    ));
}

#[test]
fn cancellation_and_deadline_after_work_started() {
    let polygons: Vec<_> = (0..1000)
        .map(|i| {
            let x = f64::from(i) * 4.;
            Polygon::new(
                LineString::from(vec![(x, 0.), (x + 2., 0.), (x + 2., 2.), (x, 2.), (x, 0.)]),
                vec![],
            )
        })
        .collect();
    let lines = geometry("LINESTRING EMPTY");
    let flag = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        // Fonte GOP-028: ingresso non cancellato, interruzione cooperativa durante lavoro pubblico.
        assert!(matches!(
            polygon_completion_with_limits(&polygons, &lines, RelateLimits::default(), Some(&flag)),
            Err(PolygonCompletionError::Predicate(PredicateError::Cancelled))
        ));
    });
    let limits = RelateLimits {
        deadline: Some(Instant::now() + Duration::from_millis(20)),
        ..RelateLimits::default()
    };
    assert!(matches!(
        polygon_completion_with_limits(&polygons, &lines, limits, None),
        Err(PolygonCompletionError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
}

#[test]
fn unpublished_intersections_do_not_force_rounding() {
    let p = polygon("POLYGON((0 0,3 1,0 2,0 0))");
    // Fonte GOP-020/015A: nessuna faccia esterna; nodi 1/3 e 5/3 non pubblicati.
    assert!(run(&[p], &geometry("LINESTRING(1 -1,1 3)")).is_empty());
}

#[test]
fn rounded_boundary_node_uses_shared_publication() {
    let p = polygon("POLYGON((0 0,3 1,0 2,0 0))");
    let line = geometry("LINESTRING(1 1,1 -1,3 -1,3 1)");
    // GOP-015A/020: float(Fraction(1,3)) e' il nodo pubblicato del bordo.
    let expected = Polygon::new(
        LineString::from(vec![
            (1., -1.),
            (3., -1.),
            (3., 1.),
            (1., 1. / 3.),
            (1., -1.),
        ]),
        vec![],
    );
    assert_eq!(run(&[p], &line), vec![expected]);
}

#[test]
fn canonical_area_and_sequence_ties() {
    let p = polygon("POLYGON((0 0,4 1,1 4,0 0))");
    for (height, expected_upper) in [
        (2., "POLYGON((0 0,1 4,0 2,0 0))"),
        (4., "POLYGON((0 0,1 4,0 4,0 0))"),
    ] {
        let lines = Geometry::MultiLineString(geo::MultiLineString::new(vec![
            LineString::from(vec![(0., 0.), (4., 0.), (4., 1.)]),
            LineString::from(vec![(0., 0.), (0., height), (1., 4.)]),
        ]));
        let result = run(std::slice::from_ref(&p), &lines);
        // Fonte ordine GOP-020: stesso minimo; area 1 prima di 2, poi
        // a parita' di area sequenza con secondo vertice (1,4) prima di (4,0).
        assert_eq!(
            result,
            vec![
                polygon(expected_upper),
                polygon("POLYGON((0 0,4 0,4 1,0 0))")
            ]
        );
        valid(&result);
    }
}

#[test]
fn existing_role_on_hole_and_subnormal_collapse() {
    let lines = geometry("LINESTRING(-1 -1,3 -1,3 3,-1 3,-1 -1)");
    let result = run(&[base()], &lines);
    // Fonte GOP-020: lato esistente sul buco e lato nuovo sul guscio qualificano.
    assert_eq!(
        result,
        vec![polygon(
            "POLYGON((-1 -1,3 -1,3 3,-1 3,-1 -1),(0 0,0 2,2 2,2 0,0 0))"
        )]
    );
    let p = polygon("POLYGON((0 0,1 0,1 1,0 1,0 0))");
    let lines = Geometry::LineString(LineString::from(vec![
        (0., f64::from_bits(1)),
        (2., 0.),
        (2., 1.),
        (1., 1.),
    ]));
    // Fonte GOP-015A: il nodo (1,2^-1075) collassa su (1,0) dopo ties-to-even.
    assert!(matches!(
        polygon_completion_with_limits(&[p], &lines, RelateLimits::default(), None),
        Err(PolygonCompletionError::InvalidRoundedTopology)
    ));
}

#[test]
fn open_input_rings_are_not_implicitly_closed() {
    let mut open = base();
    let result = open.try_exterior_mut(|ring| {
        ring.0.pop();
        Err("fixture interrotta")
    });
    assert_eq!(result, Err("fixture interrotta"));
    // Fonte GOP-002/020: invalidita' strutturale rifiutata prima del costruttore.
    assert_ne!(open.exterior().0.first(), open.exterior().0.last());
    assert!(matches!(
        polygon_completion_with_limits(&[open], &extension(), RelateLimits::default(), None),
        Err(PolygonCompletionError::Predicate(
            PredicateError::InvalidGeometry { .. }
        ))
    ));
}

#[test]
#[deny(clippy::disallowed_methods)]
fn adjacent_existing_polygons_and_null_line_edges() {
    let existing = [base(), polygon("POLYGON((2 0,4 0,4 2,2 2,2 0))")];
    // GOP-020: il rettangolo [1,3] x [-1,0] e' l'unica faccia nuova.
    let expected = vec![polygon("POLYGON((1 -1,3 -1,3 0,2 0,1 0,1 -1))")];
    for text in [
        "LINESTRING(1 0,1 -1,3 -1,3 0)",
        "LINESTRING(1 0,1 0,1 -1,1 -1,3 -1,3 0,3 0)",
    ] {
        assert_eq!(run(&existing, &geometry(text)), expected);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn resource_prefixes_are_atomic_and_monotone() {
    // GOP-020: unica faccia nuova adiacente al lato x=2.
    let polygons = [base()];
    let lines = extension();
    let expected = vec![polygon("POLYGON((2 0,4 0,4 2,2 2,2 0))")];
    // GOP-026/027: nessun prefisso pubblicato; aumentare un budget
    // sufficiente conserva il risultato analitico, anche alla pubblicazione.
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
            match polygon_completion_with_limits(&polygons, &lines, limits, None) {
                Ok(actual) => {
                    assert_eq!(actual, expected);
                    succeeded = true;
                }
                Err(PolygonCompletionError::Predicate(error)) => {
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
