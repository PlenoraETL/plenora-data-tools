#![allow(clippy::float_cmp)]
use super::*;
use crate::predicates::{relate_matrix, PredicateError};
use geo::{Area, GeometryCollection, LineString, MultiPolygon, Point, Rect};

fn rect(x: f64, y: f64, xx: f64, yy: f64) -> Geometry<f64> {
    Geometry::Polygon(Rect::new((x, y), (xx, yy)).to_polygon())
}
fn point(x: f64, y: f64) -> Geometry<f64> {
    Geometry::Point(Point::new(x, y))
}
fn line() -> Geometry<f64> {
    Geometry::LineString(LineString::from(vec![(-1., 1.), (3., 1.)]))
}
fn run(a: &Geometry<f64>, b: &Geometry<f64>, op: GeneralBooleanOp) -> Geometry<f64> {
    boolean_with_limits(a, b, op, None, GeneralBooleanLimits::default(), None).unwrap()
}
fn equivalent(a: &Geometry<f64>, b: &Geometry<f64>) {
    use geo::Relate;
    // O1 SFA 6.1.15: oracle geo, distinto dall'arrangement razionale in prova.
    assert!(a.relate(b).matches("**F**FFF*").unwrap());
}

/// GOP-014/015, O1: tutte le nove coppie ordinate, con P contenuto in L in S.
#[cfg(test)]
#[deny(clippy::disallowed_methods)]
#[test]
fn every_dimension_pair_and_filter() {
    let gs = [
        point(1., 1.),
        Geometry::LineString(LineString::from(vec![(0., 1.), (2., 1.)])),
        rect(0., 0., 2., 2.),
    ];
    // O1 SFA 6.1.15: insiemi razionali annidati P=(1,1), L=[0,2]x{1},
    // S=[0,2]x[0,2]; estremi di L sul confine di S. Tutte le nove celle.
    let matrices = [
        [*b"0FFFFFFF2", *b"0FFFFF102", *b"0FFFFF212"],
        [*b"0F1FF0FF2", *b"1FFF0FFF2", *b"1FFF0F212"],
        [*b"0F2FF1FF2", *b"1F2F01FF2", *b"2FFF1FFF2"],
    ];
    let empty_matrices = [*b"FFFFFF0F2", *b"FFFFFF102", *b"FFFFFF212"];
    for (i, a) in gs.iter().enumerate() {
        for (j, b) in gs.iter().enumerate() {
            assert_eq!(
                relate_matrix(&run(a, b, GeneralBooleanOp::Union), a)
                    .unwrap()
                    .cells(),
                matrices[i.max(j)][i]
            );
            assert_eq!(
                relate_matrix(&run(a, b, GeneralBooleanOp::Intersection), a)
                    .unwrap()
                    .cells(),
                matrices[i.min(j)][i]
            );
            equivalent(&run(a, b, GeneralBooleanOp::Union), &gs[i.max(j)]);
            equivalent(&run(a, b, GeneralBooleanOp::Intersection), &gs[i.min(j)]);
            let diff = run(a, b, GeneralBooleanOp::Difference);
            assert_eq!(
                relate_matrix(&diff, a).unwrap().cells(),
                if i <= j {
                    empty_matrices[i]
                } else {
                    matrices[i][i]
                }
            );
            if i <= j {
                assert!(matches!(diff, Geometry::GeometryCollection(ref gs) if gs.0.is_empty()));
            } else {
                equivalent(&diff, a);
            }
            for d in 0..=2 {
                let value = boolean_with_limits(
                    a,
                    b,
                    GeneralBooleanOp::Intersection,
                    Some(d),
                    GeneralBooleanLimits::default(),
                    None,
                )
                .unwrap();
                if usize::from(d) == i.min(j) {
                    equivalent(&value, &gs[i.min(j)]);
                } else {
                    assert_eq!(value.coords_count(), 0);
                }
            }
        }
    }
}
use geo::CoordsIter;

/// GOP-015: un punto al midpoint non seleziona un intervallo; incroci nodati.
#[test]
fn point_midpoint_and_crossing_lines() {
    let a = line();
    let p = point(1., 1.);
    equivalent(&run(&a, &p, GeneralBooleanOp::Intersection), &p);
    equivalent(&run(&a, &p, GeneralBooleanOp::Difference), &a);
    let b = Geometry::LineString(LineString::from(vec![(1., 0.), (1., 2.)]));
    equivalent(&run(&a, &b, GeneralBooleanOp::Intersection), &p);
    let surface = rect(0., 0., 2., 2.);
    let clipped = Geometry::LineString(LineString::from(vec![(0., 1.), (2., 1.)]));
    equivalent(&run(&a, &surface, GeneralBooleanOp::Intersection), &clipped);
}

/// GOP-002 e O1 6.1.14: contatto puntuale valido; overlap e lato comune invalidi.
#[test]
fn multipolygon_contacts_and_invalid_overlaps() {
    for (x, y, valid) in [(1., 1., true), (1., 0., false), (0.5, 0.5, false)] {
        let input = Geometry::MultiPolygon(MultiPolygon(vec![
            Rect::new((0., 0.), (1., 1.)).to_polygon(),
            Rect::new((x, y), (x + 1., y + 1.)).to_polygon(),
        ]));
        let result = union_with_limits(&[input], None, GeneralBooleanLimits::default(), None);
        if valid {
            assert_eq!(result.unwrap().unsigned_area(), 2.);
        } else {
            assert!(matches!(
                result,
                Err(GeneralBooleanError::Predicate(
                    PredicateError::InvalidGeometry { .. }
                ))
            ));
        }
    }
}

/// GOP-014: unione delle componenti, eliminazione delle parti coperte, ordine 0/1/2.
#[test]
fn nested_mixed_collection_and_grid_union() {
    let mixed = Geometry::GeometryCollection(GeometryCollection(vec![
        point(9., 9.),
        line(),
        rect(0., 0., 2., 2.),
        Geometry::GeometryCollection(GeometryCollection(vec![
            point(1., 1.),
            rect(1., 0., 3., 2.),
        ])),
    ]));
    let result = union_with_limits(&[mixed], None, GeneralBooleanLimits::default(), None).unwrap();
    let Geometry::GeometryCollection(gs) = result else {
        panic!("collezione attesa")
    };
    assert_eq!(gs.0.len(), 3);
    equivalent(&gs.0[0], &point(9., 9.));
    equivalent(
        &gs.0[1],
        &Geometry::LineString(LineString::from(vec![(-1., 1.), (0., 1.)])),
    );
    assert_eq!(gs.0[2].unsigned_area(), 6.);
    // Oracolo combinatorio indipendente: n quadrati unitari adiacenti, area n.
    for n in [0, 1, 2, 5] {
        let inputs: Vec<_> = (0..n)
            .map(|i| rect(f64::from(i), 0., f64::from(i + 1), 1.))
            .collect();
        let result =
            union_with_limits(&inputs, None, GeneralBooleanLimits::default(), None).unwrap();
        assert_eq!(result.unsigned_area(), f64::from(n));
        if n > 0 {
            equivalent(&result, &rect(0., 0., f64::from(n), 1.));
        }
    }
}

/// GOP-014/027: cardinalita', ordine, duplicati e budget condiviso fra le righe.
#[test]
fn arrays_atomic_order_and_cumulative_limits() {
    let b = rect(0., 0., 2., 2.);
    for inputs in [
        vec![],
        vec![point(1., 1.)],
        vec![point(3., 3.), point(1., 1.), point(1., 1.)],
    ] {
        for op in [GeneralBooleanOp::Intersection, GeneralBooleanOp::Difference] {
            let result =
                array(&inputs, &b, op, None, GeneralBooleanLimits::default(), None).unwrap();
            assert_eq!(result.len(), inputs.len());
            for (a, r) in inputs.iter().zip(result) {
                assert_eq!(r, run(a, &b, op));
            }
        }
    }
    let limits = GeneralBooleanLimits {
        max_rings: 1,
        ..GeneralBooleanLimits::default()
    };
    assert_eq!(
        intersection_with_limits(std::slice::from_ref(&b), &b, None, limits, None)
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        intersection_with_limits(&[b.clone(), b.clone()], &b, None, limits, None),
        Err(GeneralBooleanError::Predicate(PredicateError::WorkLimit))
    ));
    let invalid = Geometry::LineString(LineString::from(vec![(0., 0.)]));
    assert!(matches!(
        intersection_with_limits(
            &[b.clone(), invalid],
            &b,
            None,
            GeneralBooleanLimits::default(),
            None
        ),
        Err(GeneralBooleanError::Predicate(
            PredicateError::InvalidGeometry { .. }
        ))
    ));
}

/// GOP-028: deadline prima del token, anche per l'unione e gli array vuoti.
#[test]
fn entry_deadline_cancellation_and_filter() {
    let flag = AtomicBool::new(true);
    let mut limits = GeneralBooleanLimits::default();
    assert!(matches!(
        union_with_limits(&[], None, limits, Some(&flag)),
        Err(GeneralBooleanError::Predicate(PredicateError::Cancelled))
    ));
    limits.work.deadline = Some(std::time::Instant::now());
    assert!(matches!(
        union_with_limits(&[], None, limits, Some(&flag)),
        Err(GeneralBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
    assert!(matches!(
        intersection_with_limits(&[], &point(0., 0.), None, limits, Some(&flag)),
        Err(GeneralBooleanError::Predicate(
            PredicateError::DeadlineExceeded
        ))
    ));
    assert!(matches!(
        union_with_limits(&[], Some(3), GeneralBooleanLimits::default(), None),
        Err(GeneralBooleanError::InvalidOutputDimension)
    ));
}

/// GOP-026/027/028: soglia esatta ammessa, addebito successivo rifiutato;
/// checkpoint distribuiti tra ingresso, noding, assemblaggio e pubblicazione.
#[test]
fn measured_limits_and_interruptions() {
    let inputs = [
        Geometry::GeometryCollection(GeometryCollection(vec![
            rect(0., 0., 2., 2.),
            line(),
            point(8., 8.),
        ])),
        rect(1., 0., 3., 2.),
    ];
    let limits = GeneralBooleanLimits::default();
    let mut measure = Work::new(limits.work, None);
    polygon_boolean::execute_many(
        &inputs,
        &[],
        GeneralBooleanOp::Union,
        None,
        &limits,
        &mut measure,
    )
    .unwrap();
    for (kind, threshold) in [
        (0, measure.steps),
        (1, measure.nodes),
        (2, measure.pairs),
        (3, measure.peak_bytes()),
    ] {
        for delta in [-1_i32, 0, 1] {
            let mut bounded = limits;
            let value = if delta < 0 {
                threshold - 1
            } else {
                threshold + u64::try_from(delta).unwrap()
            };
            match kind {
                0 => bounded.work.max_steps = value,
                1 => bounded.work.max_nodes = value,
                2 => bounded.work.max_segment_pairs = value,
                _ => bounded.work.max_reserved_bytes = value,
            }
            let result = union_with_limits(&inputs, None, bounded, None);
            if delta < 0 {
                if kind == 3 {
                    assert!(matches!(
                        result,
                        Err(GeneralBooleanError::Predicate(
                            PredicateError::ReservationLimit
                        ))
                    ));
                } else {
                    assert!(matches!(
                        result,
                        Err(GeneralBooleanError::Predicate(PredicateError::WorkLimit))
                    ));
                }
            } else {
                result.unwrap();
            }
        }
    }
    for at in [
        0,
        1,
        measure.steps / 4,
        measure.steps / 2,
        3 * measure.steps / 4,
        measure.steps - 1,
    ] {
        for deadline in [false, true] {
            let flag = AtomicBool::new(false);
            let mut work = Work::new(limits.work, Some(&flag));
            work.interrupt_at = Some((at, deadline));
            let result = polygon_boolean::execute_many(
                &inputs,
                &[],
                GeneralBooleanOp::Union,
                None,
                &limits,
                &mut work,
            );
            if deadline {
                assert!(matches!(
                    result,
                    Err(GeneralBooleanError::Predicate(
                        PredicateError::DeadlineExceeded
                    ))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(GeneralBooleanError::Predicate(PredicateError::Cancelled))
                ));
            }
            assert_eq!(work.steps, at);
        }
    }
}

/// GOP-014, O1: composizione di intersezioni su una griglia razionale;
/// le celle occupate sono contate con interi, indipendentemente dall'overlay.
#[test]
fn many_overlapping_rectangles_combinatorial_grid() {
    for n in [1, 2, 4, 6] {
        let inputs: Vec<_> = (0..n)
            .map(|i| {
                rect(
                    f64::from(i),
                    f64::from(i % 2),
                    f64::from(i + 2),
                    f64::from(i % 2 + 2),
                )
            })
            .collect();
        let expected: i32 = (0..n + 2)
            .flat_map(|x| (0..4).map(move |y| (x, y)))
            .map(|(x, y)| {
                i32::from((0..n).any(|i| i <= x && x < i + 2 && i % 2 <= y && y < i % 2 + 2))
            })
            .sum();
        let result =
            union_with_limits(&inputs, None, GeneralBooleanLimits::default(), None).unwrap();
        assert_eq!(result.unsigned_area(), f64::from(expected));
        for input in &inputs {
            let cells = relate_matrix(input, &result).unwrap().cells();
            // SFA 6.1.15: ogni rettangolo appartiene all'unione.
            assert_eq!(cells[2], b'F');
            assert_eq!(cells[5], b'F');
        }
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn rounded_isolated_point_cannot_enter_surface() {
    use wkt::TryFromWkt;
    // Fraction: 7*x+3*y-21 e' positivo al nodo esatto e negativo al pubblicato.
    let a = Geometry::try_from_wkt_str("GEOMETRYCOLLECTION(POLYGON((0 0,3 0,0 7,0 0)),LINESTRING(1.3238327648331625 3.9110568820559544,1.3948234964231734 3.7454118416792626))").unwrap();
    let b = Geometry::try_from_wkt_str("GEOMETRYCOLLECTION(POLYGON((0 0,3 0,0 7,0 0)),LINESTRING(1.4336456836623859 3.6548267381210997,1.0829846946613322 4.473035712456892))").unwrap();
    assert!(matches!(
        boolean_with_limits(
            &a,
            &b,
            GeneralBooleanOp::Intersection,
            None,
            GeneralBooleanLimits::default(),
            None
        ),
        Err(GeneralBooleanError::InvalidRoundedTopology)
    ));
}

#[test]
fn resource_prefixes_keep_array_intersection_atomic() {
    let inputs = [point(1.0, 1.0), rect(0.0, 0.0, 2.0, 2.0)];
    let right = rect(1.0, 0.0, 3.0, 2.0);
    // O1 SFA 6.1.15: il punto e' sul bordo; la superficie comune
    // e' [1,2]x[0,2]. GOP-015 preserva l'ordine dell'array.
    let expected = [point(1.0, 1.0), rect(1.0, 0.0, 2.0, 2.0)];
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
            let mut limits = GeneralBooleanLimits::default();
            match resource {
                0 => limits.work.max_steps = budget,
                1 => limits.work.max_reserved_bytes = budget,
                2 => limits.work.max_nodes = budget,
                _ => limits.work.max_segment_pairs = budget,
            }
            match intersection_with_limits(&inputs, &right, None, limits, None) {
                Ok(actual) => {
                    assert_eq!(actual.len(), expected.len());
                    for (a, b) in actual.iter().zip(&expected) {
                        equivalent(a, b);
                    }
                    succeeded = true;
                }
                Err(GeneralBooleanError::Predicate(error)) => {
                    // GOP-026/027: il primo risultato resta contabilizzato
                    // durante il secondo; nessun prefisso viene restituito.
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
