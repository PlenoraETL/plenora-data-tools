//! C1: facce cartesiane e anelli concentrici con attese analitiche.
use super::*;
use geo::{Area, MultiLineString};

fn line(points: &[(f64, f64)]) -> LineString<f64> {
    LineString::from(points.to_vec())
}
fn square(lo: f64, hi: f64) -> LineString<f64> {
    line(&[(lo, lo), (hi, lo), (hi, hi), (lo, hi), (lo, lo)])
}
fn input(lines: Vec<LineString<f64>>) -> Geometry<f64> {
    Geometry::MultiLineString(MultiLineString(lines))
}
fn run(g: &Geometry<f64>) -> Vec<Polygon<f64>> {
    polygonize_with_limits(g, RelateLimits::default(), None).expect("C1")
}

fn ring_key(line: &LineString<f64>) -> Vec<(u64, u64)> {
    let mut coords = line.0[..line.0.len() - 1].to_vec();
    // Normalizzazione geometrica: un vertice collineare non cambia il poligono.
    loop {
        let redundant = (0..coords.len()).find(|&i| {
            let a = coords[(i + coords.len() - 1) % coords.len()];
            let b = coords[i];
            let c = coords[(i + 1) % coords.len()];
            (b.x - a.x) * (c.y - b.y) == (b.y - a.y) * (c.x - b.x)
        });
        if let Some(i) = redundant {
            coords.remove(i);
        } else {
            break;
        }
    }
    let keys: Vec<_> = coords
        .iter()
        .map(|p| (p.x.to_bits(), p.y.to_bits()))
        .collect();
    (0..keys.len())
        .flat_map(|start| {
            let forward: Vec<_> = (0..keys.len())
                .map(|i| keys[(start + i) % keys.len()])
                .collect();
            let backward: Vec<_> = (0..keys.len())
                .map(|i| keys[(start + keys.len() - i) % keys.len()])
                .collect();
            [forward, backward]
        })
        .min()
        .expect("anello")
}
fn polygon_keys(polygons: &[Polygon<f64>]) -> Vec<Vec<Vec<(u64, u64)>>> {
    let mut keys: Vec<_> = polygons
        .iter()
        .map(|p| {
            let mut holes: Vec<_> = p.interiors().iter().map(ring_key).collect();
            holes.sort();
            let mut rings = vec![ring_key(p.exterior())];
            rings.extend(holes);
            rings
        })
        .collect();
    keys.sort();
    keys
}

#[test]
fn rectangular_grids_have_exact_faces() {
    for (n, m) in [(2, 2), (3, 5), (5, 4)] {
        let mut lines = Vec::new();
        for y in 0..n {
            lines.push(line(&[
                (0., 2. * f64::from(y)),
                (3. * f64::from(m - 1), 2. * f64::from(y)),
            ]));
        }
        for x in 0..m {
            lines.push(line(&[
                (3. * f64::from(x), 0.),
                (3. * f64::from(x), 2. * f64::from(n - 1)),
            ]));
        }
        let g = input(lines);
        let result = run(&g);
        // Fonte: partizione cartesiana, (n-1)(m-1) rettangoli 3x2.
        assert_eq!(
            result.len(),
            usize::try_from((n - 1) * (m - 1)).expect("conteggio")
        );
        assert!(result
            .iter()
            .all(|p| p.unsigned_area() == 6. && p.interiors().is_empty()));
        let expected: Vec<_> = (0..n - 1)
            .flat_map(|y| {
                (0..m - 1).map(move |x| {
                    let (x, y) = (3. * f64::from(x), 2. * f64::from(y));
                    Polygon::new(
                        line(&[(x, y), (x + 3., y), (x + 3., y + 2.), (x, y + 2.), (x, y)]),
                        vec![],
                    )
                })
            })
            .collect();
        assert_eq!(polygon_keys(&result), polygon_keys(&expected));
    }
}
#[test]
fn nested_rings_holes_overlaps_and_dangles() {
    let g = input(vec![square(0., 10.), square(2., 8.), square(4., 6.)]);
    let result = run(&g);
    let mut areas: Vec<_> = result.iter().map(Area::unsigned_area).collect();
    areas.sort_by(f64::total_cmp);
    // Fonte: quadrati concentrici: 100-36, 36-4, 4; due buchi.
    assert_eq!(areas, [4., 32., 64.]);
    assert_eq!(result.iter().map(|p| p.interiors().len()).sum::<usize>(), 2);
    let expected = [
        Polygon::new(square(0., 10.), vec![square(2., 8.)]),
        Polygon::new(square(2., 8.), vec![square(4., 6.)]),
        Polygon::new(square(4., 6.), vec![]),
    ];
    assert_eq!(polygon_keys(&result), polygon_keys(&expected));
    let g = input(vec![
        square(0., 4.),
        line(&[(1., 0.), (3., 0.)]),
        line(&[(2., 0.), (2., 2.)]),
        line(&[(4., 4.), (6., 6.)]),
    ]);
    let result = run(&g);
    // Fonte: unione dei lati coincidenti, due alberi attaccati non separano
    // il piano (Eulero); resta il quadrato di area 16, senza buchi.
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].unsigned_area(), 16.);
    assert!(result[0].interiors().is_empty());
    assert_eq!(
        polygon_keys(&result),
        polygon_keys(&[Polygon::new(square(0., 4.), vec![])])
    );
    let g = input(vec![
        square(0., 10.),
        square(2., 8.),
        line(&[(0., 0.), (2., 2.)]),
    ]);
    // Fonte: un ponte tra anelli non separa la corona: aree 64 e 36.
    let result = run(&g);
    assert_eq!(result.len(), 2);
    assert_eq!(
        polygon_keys(&result),
        polygon_keys(&[
            Polygon::new(square(0., 10.), vec![square(2., 8.)]),
            Polygon::new(square(2., 8.), vec![])
        ])
    );
}
#[test]
fn large_sparse_input_succeeds_without_pairs() {
    let g = input(
        (0..4096)
            .map(|i| {
                let x = 3. * f64::from(i);
                line(&[(x, 0.), (x + 1., 1.)])
            })
            .collect(),
    );
    // Fonte: 4096 segmenti con inviluppi X disgiunti, zero coppie e cicli.
    assert!(polygonize_with_limits(
        &g,
        RelateLimits {
            max_segment_pairs: 0,
            ..RelateLimits::default()
        },
        None
    )
    .expect("sparso")
    .is_empty());
}
#[test]
fn exact_limits_and_interruptions() {
    let g = input(vec![square(0., 2.)]);
    let mut work = Work::new(RelateLimits::default(), None);
    let expected = polygonize(&g, &mut work).expect("misura");
    // Fonte: quattro coppie per sweep, piu' otto per la validazione
    // superficiale comune degli anelli materializzati.
    assert_eq!(work.pairs, 16);
    assert!(work.nodes > 36);
    // Fonte contabile: checkpoint e picco ricevute del percorso completo.
    let exact = RelateLimits {
        max_segment_pairs: work.pairs,
        max_nodes: work.nodes,
        max_steps: work.steps,
        max_reserved_bytes: work.peak_bytes(),
        deadline: None,
    };
    assert_eq!(
        polygonize_with_limits(&g, exact, None).expect("soglia"),
        expected
    );
    for limits in [
        RelateLimits {
            max_segment_pairs: exact.max_segment_pairs - 1,
            ..exact
        },
        RelateLimits {
            max_nodes: exact.max_nodes - 1,
            ..exact
        },
        RelateLimits {
            max_steps: exact.max_steps - 1,
            ..exact
        },
    ] {
        // Fonte: contratto inclusivo RelateLimits, consumo meno una unita'.
        assert!(matches!(
            polygonize_with_limits(&g, limits, None),
            Err(PredicateError::WorkLimit)
        ));
    }
    assert!(matches!(
        polygonize_with_limits(
            &g,
            RelateLimits {
                max_reserved_bytes: exact.max_reserved_bytes - 1,
                ..exact
            },
            None
        ),
        Err(PredicateError::ReservationLimit)
    ));
    let expired = RelateLimits {
        deadline: Some(std::time::Instant::now()),
        ..RelateLimits::default()
    };
    let flag = std::sync::atomic::AtomicBool::new(true);
    // Fonte: Work::step, deadline prima della cancellazione, anche su vuoto.
    let empty = input(vec![]);
    assert!(matches!(
        polygonize_with_limits(&empty, expired, Some(&flag)),
        Err(PredicateError::DeadlineExceeded)
    ));
    assert!(matches!(
        polygonize_with_limits(&empty, expired, None),
        Err(PredicateError::DeadlineExceeded)
    ));
}
#[test]
fn invalid_coordinates_are_explicit() {
    // Fonte: contratto finitezza dell'ingresso.
    assert!(matches!(
        polygonize_with_limits(
            &input(vec![line(&[(0., 0.), (f64::NAN, 1.)])]),
            RelateLimits::default(),
            None
        ),
        Err(PredicateError::NonFiniteCoordinate { .. })
    ));
}

#[test]
fn rounded_triangle_cannot_silently_become_collinear() {
    // Fonte: equazioni y=x-X e y=1-(1.999/2)*(x-X+1).
    // Apice x=X+circa 0.00025006, ULP(X)=0.0078125: arrotonda X.
    // Fonte GOP-015A: tre vertici distinti diventano collineari dopo rounding.
    let x = 2_f64.powi(45);
    let g = input(vec![
        line(&[(x, -2.), (x, 2.)]),
        line(&[(x - 1., -1.), (x + 1., 1.)]),
        line(&[(x - 1., 1.), (x + 1., -0.999)]),
    ]);
    assert!(matches!(
        polygonize_with_limits(&g, RelateLimits::default(), None),
        Err(PredicateError::InvalidRoundedTopology)
    ));
}

#[test]
fn gop015a_face_nodes_ties_and_subnormals() {
    // Fonte: GOP-015A; incrocio a meta' delle ascisse, pareggio al pari.
    for bits in [1.0_f64.to_bits(), 1.0_f64.to_bits() + 1, 1] {
        let a = f64::from_bits(bits);
        let b = f64::from_bits(bits + 1);
        let g = input(vec![
            line(&[(0., -1.), (2., -1.), (2., 1.), (0., 1.), (0., -1.)]),
            line(&[(0., 0.), (2., 0.)]),
            line(&[(a, -1.), (b, 1.)]),
        ]);
        let result = run(&g);
        let expected = f64::from_bits(if bits & 1 == 0 { bits } else { bits + 1 });
        assert_eq!(result.len(), 4);
        assert!(result
            .iter()
            .flat_map(geo::CoordsIter::coords_iter)
            .any(|c| c.x.to_bits() == expected.to_bits() && c.y == 0.));
    }
}

#[test]
fn gop015a_nondyadic_faces_and_discarded_dangles() {
    // Fonte: y=-1+d*x, incrocio con y=0 in x=1/d; GOP-015A.
    for d in [3, 5, 7] {
        let g = input(vec![
            square(0., 1.),
            line(&[(0., 0.), (1., 0.)]),
            line(&[(0., -1.), (1., f64::from(d - 1))]),
        ]);
        let result = run(&g);
        assert!(result
            .iter()
            .flat_map(geo::CoordsIter::coords_iter)
            .any(|c| c.x == 1. / f64::from(d) && c.y == 0.));
    }
    // Fonte GOP-015A: X+1/10 si pubblica X; facce valide anche con ulp=1/4.
    let x = 2_f64.powi(50);
    let faces = input(vec![
        line(&[
            (x - 1., 0.),
            (x + 2., 0.),
            (x + 2., 1.),
            (x - 1., 1.),
            (x - 1., 0.),
        ]),
        line(&[(x, -1.), (x + 1., 9.)]),
    ]);
    assert_eq!(run(&faces).len(), 2);
    // Fonte: nessuna faccia, quindi il nodo costruito non viene pubblicato.
    let x = 2_f64.powi(50);
    let g = input(vec![
        line(&[(x, 0.), (x + 1., 0.)]),
        line(&[(x, -1.), (x + 1., 9.)]),
    ]);
    assert!(run(&g).is_empty());
}

#[test]
#[deny(clippy::disallowed_methods)]
fn excluded_point_is_typed_unsupported() {
    assert!(matches!(
        polygonize_with_limits(
            &Geometry::Point(geo::Point::new(0., 0.)),
            RelateLimits::default(),
            None
        ),
        Err(PredicateError::UnsupportedGeometry)
    ));
}
