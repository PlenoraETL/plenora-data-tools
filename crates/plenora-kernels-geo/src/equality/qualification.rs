//! Voce memoria 14: oracolo enumerativo e regressione bit per bit 6.1.
use super::*;
use geo::{CoordsIter, MultiLineString, MultiPoint, MultiPolygon, Point};
use proptest::prelude::*;
use std::sync::atomic::AtomicBool;

fn bits(coords: &[Coord]) -> Vec<(u64, u64)> {
    coords
        .iter()
        .map(|c| (c.x.to_bits(), c.y.to_bits()))
        .collect()
}
fn oracle(coords: &[Coord]) -> Vec<Coord> {
    // Implementazione indipendente: enumera ogni rotazione, confronta chiavi
    // scalari esplicite e mantiene la prima in caso di parita'.
    let key = |c: &Coord| {
        [
            if c.x == 0.0 { 0.0 } else { c.x },
            if c.y == 0.0 { 0.0 } else { c.y },
        ]
    };
    let mut answer = coords.to_vec();
    for start in 1..coords.len() {
        let candidate: Vec<_> = (0..coords.len())
            .map(|i| coords[(start + i) % coords.len()])
            .collect();
        let order = candidate
            .iter()
            .zip(&answer)
            .flat_map(|(a, b)| key(a).into_iter().zip(key(b)))
            .map(|(a, b)| a.total_cmp(&b))
            .find(|order| !order.is_eq());
        if order == Some(Ordering::Less) {
            answer = candidate;
        }
    }
    answer
}
fn structure(geometry: &Geometry) -> Vec<usize> {
    match geometry {
        Geometry::Point(_) => vec![0],
        Geometry::Line(_) => vec![1],
        Geometry::Rect(_) => vec![2],
        Geometry::Triangle(_) => vec![3],
        Geometry::LineString(line) => vec![4, line.0.len()],
        Geometry::Polygon(polygon) => std::iter::once(5)
            .chain(std::iter::once(polygon.exterior().0.len()))
            .chain(std::iter::once(polygon.interiors().len()))
            .chain(polygon.interiors().iter().map(|r| r.0.len()))
            .collect(),
        Geometry::MultiPoint(points) => vec![6, points.0.len()],
        Geometry::MultiLineString(lines) => std::iter::once(7)
            .chain(std::iter::once(lines.0.len()))
            .chain(lines.0.iter().map(|l| l.0.len()))
            .collect(),
        Geometry::MultiPolygon(polygons) => std::iter::once(8)
            .chain(std::iter::once(polygons.0.len()))
            .chain(
                polygons
                    .0
                    .iter()
                    .flat_map(|p| structure(&Geometry::Polygon(p.clone()))),
            )
            .collect(),
        Geometry::GeometryCollection(collection) => std::iter::once(9)
            .chain(std::iter::once(collection.0.len()))
            .chain(collection.0.iter().flat_map(structure))
            .collect(),
    }
}
fn assert_bits(a: &Geometry, b: &Geometry) {
    assert_eq!(structure(a), structure(b));
    assert_eq!(
        bits(&a.coords_iter().collect::<Vec<_>>()),
        bits(&b.coords_iter().collect::<Vec<_>>())
    );
}
fn generous() -> EqualityLimits {
    EqualityLimits {
        max_work: u64::MAX,
        deadline: None,
    }
}

/// O-M4: zeri con segno, NaN/payload, infiniti, casi periodici e pari.
#[test]
fn exhaustive_rotations_match_independent_oracle() {
    let alphabet = [
        -0.0,
        0.0,
        -1.0,
        1.0,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::from_bits(0x7ff8_0000_0000_0001),
        f64::from_bits(0xfff8_0000_0000_0002),
    ];
    for length in 0..=5_u32 {
        for mut word in 0..alphabet.len().pow(length) {
            let coords: Vec<_> = (0..length)
                .map(|_| {
                    let value = alphabet[word % alphabet.len()];
                    word /= alphabet.len();
                    Coord { x: value, y: value }
                })
                .collect();
            let actual = infallible(best_rotation(&coords, &mut Unlimited));
            assert_eq!(bits(&actual), bits(&oracle(&coords)));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { rng_seed: proptest::test_runner::RngSeed::Fixed(62014), cases: 256, ..ProptestConfig::default() })]
    /// O-M4: tutte le famiglie normalizzate, ricorsione e ordinamento stabile.
    #[test]
    fn legacy_bits_and_results(values in prop::collection::vec(any::<u64>(), 0..30), tolerance in any::<f64>()) {
        let coords: Vec<_> = values.iter().map(|b| Coord { x: f64::from_bits(*b), y: f64::from_bits(b.rotate_left(13)) }).collect();
        let mut reverse = coords.clone(); reverse.reverse();
        let a = LineString(coords.clone()); let b = LineString(reverse);
        let polygon = Polygon::new(a.clone(), vec![b.clone(),a.clone()]);
        let families = vec![
            Geometry::LineString(a.clone()), Geometry::Polygon(polygon.clone()),
            Geometry::MultiPoint(MultiPoint(coords.iter().copied().map(Point).collect())),
            Geometry::MultiLineString(MultiLineString(vec![b,a])),
            Geometry::MultiPolygon(MultiPolygon(vec![polygon.clone(),polygon])),
        ];
        let collection = Geometry::GeometryCollection(geo::GeometryCollection(families.clone()));
        for input in families.iter().chain(std::iter::once(&collection)) {
            let expected = legacy::normalize_geometry(input);
            assert_bits(&normalize_geometry(input), &expected);
            assert_bits(&normalize_geometry_with_limits(input, generous(), None).unwrap(), &expected);
            for normalize in [false,true] {
                let comparison = GeometryComparison { tolerance, normalize };
                let old = legacy::geo_equals_with_tolerance(input,&expected,comparison);
                prop_assert_eq!(geo_equals_with_tolerance(input,&expected,comparison), old);
                prop_assert_eq!(geo_equals_with_tolerance_with_limits(input,&expected,comparison,generous(),None).unwrap(),old);
                prop_assert_eq!(geo_equals_with_tolerance(&expected,input,comparison),old);
            }
        }
    }
}

/// Contratto locale: equivalenza a zero, non transitivita' a tolleranza positiva.
#[test]
fn rotations_reversals_transitivity_and_positive_counterexample() {
    let ring = [
        Coord { x: 0.0, y: 0.0 },
        Coord { x: 2.0, y: 0.0 },
        Coord { x: 1.0, y: 3.0 },
        Coord { x: 1.0, y: 1.0 },
    ];
    let mut variants = Vec::new();
    for reverse in [false, true] {
        for start in 0..ring.len() {
            let mut coords: Vec<_> = (0..ring.len())
                .map(|i| ring[(start + i) % ring.len()])
                .collect();
            if reverse {
                coords.reverse();
            }
            variants.push(Geometry::Polygon(Polygon::new(LineString(coords), vec![])));
        }
    }
    let comparison = GeometryComparison::new(0.0).with_normalization(true);
    for a in &variants {
        for b in &variants {
            for c in &variants {
                assert!(geo_equals_with_tolerance(a, b, comparison));
                assert!(geo_equals_with_tolerance(b, c, comparison));
                assert!(geo_equals_with_tolerance(a, c, comparison));
            }
        }
    }
    for normalize in [false, true] {
        let comparison = GeometryComparison::new(1.0).with_normalization(normalize);
        let [a, b, c] = [0.0, 0.75, 1.5].map(|x| Geometry::Point(Point::new(x, 0.0)));
        assert!(geo_equals_with_tolerance(&a, &b, comparison));
        assert!(geo_equals_with_tolerance(&b, &c, comparison));
        assert!(!geo_equals_with_tolerance(&a, &c, comparison));
    }
}

/// O-M3: numero effettivo di passi, anche con prefissi quasi interamente uguali.
#[test]
fn periodic_and_long_prefix_work_is_linear() {
    for n in [10_000, 20_000, 40_000, 85_000] {
        for period in [1, 2, 7] {
            let mut coords: Vec<_> = (0..n)
                .map(|i| Coord {
                    x: f64::from(i % period),
                    y: 0.0,
                })
                .collect();
            if period == 1 {
                coords[usize::try_from(n).unwrap() - 1].x = 1.0;
            }
            let mut work = EqualityWork::new(generous(), None);
            let result = best_rotation(&coords, &mut work).unwrap();
            assert_eq!(result.len(), coords.len());
            assert!(work.used <= 4 * u64::try_from(n).unwrap());
            println!("rotation n={n} period={period} work={}", work.used);
        }
    }
}

/// GOP-028: budget esatto, token solo, ingresso vuoto e confronto senza normalize.
#[test]
fn limits_are_cumulative_and_errors_explicit() {
    let input = Geometry::MultiPoint(MultiPoint(vec![
        Point::new(-0.0, 1.0),
        Point::new(0.0, 1.0),
    ]));
    let mut work = EqualityWork::new(generous(), None);
    let expected = normalize(&input, &mut work).unwrap();
    let exact = EqualityLimits {
        max_work: work.used,
        deadline: None,
    };
    assert_bits(
        &normalize_geometry_with_limits(&input, exact, None).unwrap(),
        &expected,
    );
    assert_eq!(
        normalize_geometry_with_limits(
            &input,
            EqualityLimits {
                max_work: exact.max_work - 1,
                ..exact
            },
            None
        ),
        Err(EqualityError::WorkLimit)
    );
    let flag = AtomicBool::new(true);
    let empty = Geometry::GeometryCollection(geo::GeometryCollection(vec![]));
    assert_eq!(
        normalize_geometry_with_limits(&empty, generous(), Some(&flag)),
        Err(EqualityError::Cancelled)
    );
    let comparison = GeometryComparison::new(0.0);
    assert_eq!(
        geo_equals_with_tolerance_with_limits(&input, &input, comparison, generous(), Some(&flag)),
        Err(EqualityError::Cancelled)
    );
    assert_eq!(
        geo_equals_with_tolerance_with_limits(
            &input,
            &input,
            comparison,
            EqualityLimits {
                max_work: 0,
                deadline: None
            },
            None
        ),
        Err(EqualityError::WorkLimit)
    );
}

/// O-M4: chiavi identiche con bit distinti, prefissi lunghi e ordinamento stabile.
#[test]
fn ties_preserve_legacy_bits_in_rings_and_components() {
    for period in [1, 2, 7] {
        let coords: Vec<_> = (0..511)
            .map(|i| Coord {
                x: if i % period == 0 { -0.0 } else { 0.0 },
                y: f64::from(i % period),
            })
            .collect();
        assert_eq!(
            bits(&infallible(best_rotation(&coords, &mut Unlimited))),
            bits(&oracle(&coords))
        );
        let ring = LineString(coords);
        let polygon = Polygon::new(ring.clone(), vec![ring.clone()]);
        let input = Geometry::MultiPolygon(MultiPolygon(vec![polygon.clone(), polygon]));
        assert_bits(
            &normalize_geometry(&input),
            &legacy::normalize_geometry(&input),
        );
    }
    let zeros = [-0.0, 0.0, -0.0, 0.0, 0.0, -0.0];
    let input = Geometry::MultiPoint(MultiPoint(zeros.map(|x| Point::new(x, 0.0)).to_vec()));
    assert_bits(
        &normalize_geometry(&input),
        &legacy::normalize_geometry(&input),
    );
}

/// O-M4: matrice di risultati veri/falsi e tolleranze anche non usuali.
#[test]
fn controlled_comparison_matches_legacy_across_structures() {
    let a = Point::new(0.0, 0.0);
    let b = Point::new(0.75, 0.0);
    let line = LineString(vec![a.0, b.0]);
    let polygon = Polygon::new(line.clone(), vec![]);
    let fixtures = vec![
        Geometry::Point(a),
        Geometry::Point(b),
        Geometry::Line(geo::Line::new(a.0, b.0)),
        Geometry::Rect(geo::Rect::new(a.0, b.0)),
        Geometry::Triangle(geo::Triangle(a.0, b.0, a.0)),
        Geometry::LineString(line.clone()),
        Geometry::LineString(LineString(vec![])),
        Geometry::Polygon(polygon.clone()),
        Geometry::MultiPoint(MultiPoint(vec![a, b])),
        Geometry::MultiLineString(MultiLineString(vec![line])),
        Geometry::MultiPolygon(MultiPolygon(vec![polygon])),
        Geometry::GeometryCollection(geo::GeometryCollection(vec![Geometry::Point(a)])),
    ];
    for a in &fixtures {
        for b in &fixtures {
            for tolerance in [0.0, 1.0, -1.0, f64::NAN, f64::INFINITY] {
                for normalize in [false, true] {
                    let comparison = GeometryComparison {
                        tolerance,
                        normalize,
                    };
                    let expected = legacy::geo_equals_with_tolerance(a, b, comparison);
                    assert_eq!(
                        geo_equals_with_tolerance_with_limits(a, b, comparison, generous(), None)
                            .unwrap(),
                        expected
                    );
                    assert_eq!(geo_equals_with_tolerance(a, b, comparison), expected);
                }
            }
        }
    }
    let comparison = GeometryComparison::new(0.0).with_normalization(true);
    let input = &fixtures[9];
    let mut work = EqualityWork::new(generous(), None);
    let left = normalize(input, &mut work).unwrap();
    let right = normalize(input, &mut work).unwrap();
    assert!(equal(&left, &right, 0.0, &mut work).unwrap());
    let exact = EqualityLimits {
        max_work: work.used,
        deadline: None,
    };
    assert!(geo_equals_with_tolerance_with_limits(input, input, comparison, exact, None).unwrap());
    assert_eq!(
        geo_equals_with_tolerance_with_limits(
            input,
            input,
            comparison,
            EqualityLimits {
                max_work: exact.max_work - 1,
                ..exact
            },
            None
        ),
        Err(EqualityError::WorkLimit)
    );
}
