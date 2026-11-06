//! Regressione dello scavo: oracolo razionale esterno e invarianti del bordo.
use geo::{ConcaveHull, Geometry, MultiPoint};
use plenora_kernels_geo::extended::concave_hull;

fn canonical(mut ids: Vec<usize>) -> Vec<usize> {
    let mut variants = Vec::new();
    for _ in 0..2 {
        for _ in 0..ids.len() {
            variants.push(ids.clone());
            ids.rotate_left(1);
        }
        ids.reverse();
    }
    variants.into_iter().min().unwrap_or_default()
}

#[test]
fn divergenza_dodici_punti() {
    let points: MultiPoint<f64> = (0..12)
        .map(|i| {
            (
                500_000.0 + f64::from(i * 10),
                5_000_000.0 + f64::from(i * i % 7 * 10),
            )
        })
        .collect::<Vec<_>>()
        .into();
    let Geometry::Polygon(hull) =
        concave_hull(&Geometry::MultiPoint(points.clone()), 2.0, 0.0, 100).unwrap()
    else {
        panic!("tipo inatteso")
    };
    let ids = hull
        .exterior()
        .0
        .iter()
        .take(hull.exterior().0.len() - 1)
        .map(|p| points.0.iter().position(|q| q.0 == *p).unwrap())
        .collect();
    assert_eq!(
        canonical(ids),
        canonical(vec![0, 1, 2, 5, 9, 11, 10, 8, 7, 6, 4, 3])
    );
    assert_eq!(hull, points.concave_hull());
}

#[derive(serde::Deserialize)]
struct Case {
    points: Vec<[i32; 2]>,
    unique: Vec<[i32; 2]>,
    concavity: f64,
    threshold: f64,
    allowed: Vec<Vec<usize>>,
}

#[test]
fn oracolo_razionale_enumerativo() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/concave_oracolo.json")).unwrap();
    for (n, case) in cases.iter().enumerate() {
        for reverse in [false, true] {
            let mut points = case.points.clone();
            if reverse {
                points.reverse();
            }
            let points: MultiPoint<f64> = points
                .iter()
                .map(|p| (f64::from(p[0]), f64::from(p[1])))
                .collect::<Vec<_>>()
                .into();
            let result = concave_hull(
                &Geometry::MultiPoint(points),
                case.concavity,
                case.threshold,
                100,
            );
            if case.allowed.is_empty() {
                assert!(matches!(
                    result,
                    Err(plenora_kernels_geo::extended::ExtendedError::InvalidOutput(
                        _
                    ))
                ));
                continue;
            }
            let Geometry::Polygon(hull) = result.unwrap() else {
                panic!("tipo inatteso")
            };
            let ids = hull
                .exterior()
                .0
                .iter()
                .take(hull.exterior().0.len() - 1)
                .map(|p| {
                    case.unique
                        .iter()
                        .position(|q| {
                            p.x.to_bits() == f64::from(q[0]).to_bits()
                                && p.y.to_bits() == f64::from(q[1]).to_bits()
                        })
                        .unwrap()
                })
                .collect();
            assert!(case.allowed.contains(&canonical(ids)), "caso {n}");
        }
    }
}

// Interi piccoli: prodotti e somme esatti, indipendenti dai predicati geo.
const fn cross(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> i64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
fn on(p: [i64; 2], a: [i64; 2], b: [i64; 2]) -> bool {
    cross(a, b, p) == 0 && (0..2).all(|i| p[i] >= a[i].min(b[i]) && p[i] <= a[i].max(b[i]))
}
fn covered(p: [i64; 2], ring: &[[i64; 2]]) -> bool {
    let mut winding = 0;
    for edge in ring.windows(2) {
        let (a, b) = (edge[0], edge[1]);
        if on(p, a, b) {
            return true;
        }
        if a[1] <= p[1] && b[1] > p[1] && cross(a, b, p) > 0 {
            winding += 1;
        }
        if a[1] > p[1] && b[1] <= p[1] && cross(a, b, p) < 0 {
            winding -= 1;
        }
    }
    winding != 0
}
fn verify(points: &[[i32; 2]], hull: &geo::Polygon<f64>) {
    let ring: Vec<[i64; 2]> = hull
        .exterior()
        .0
        .iter()
        .map(|p| {
            let q = points
                .iter()
                .find(|q| {
                    p.x.to_bits() == f64::from(q[0]).to_bits()
                        && p.y.to_bits() == f64::from(q[1]).to_bits()
                })
                .unwrap();
            [i64::from(q[0]), i64::from(q[1])]
        })
        .collect();
    assert_eq!(ring.first(), ring.last());
    for (i, e) in ring.windows(2).enumerate() {
        assert_ne!(e[0], e[1]);
        for (j, f) in ring.windows(2).enumerate().take(i) {
            if i == j + 1 || (j == 0 && i == ring.len() - 2) {
                for p in e {
                    if !f.contains(p) {
                        assert!(!on(*p, f[0], f[1]));
                    }
                }
                for p in f {
                    if !e.contains(p) {
                        assert!(!on(*p, e[0], e[1]));
                    }
                }
            } else {
                assert!(
                    !on(e[0], f[0], f[1])
                        && !on(e[1], f[0], f[1])
                        && !on(f[0], e[0], e[1])
                        && !on(f[1], e[0], e[1])
                );
                assert!(
                    !(cross(e[0], e[1], f[0]).signum() * cross(e[0], e[1], f[1]).signum() < 0
                        && cross(f[0], f[1], e[0]).signum() * cross(f[0], f[1], e[1]).signum() < 0)
                );
            }
        }
    }
    for p in points {
        assert!(covered([i64::from(p[0]), i64::from(p[1])], &ring));
    }
}

#[test]
fn bordi_semplici_e_copertura_deterministici() {
    let count = match std::env::var("PLENORA_TEST_LUNGHI").as_deref() {
        Ok("1") => 10000,
        Ok("0") | Err(_) => 300,
        _ => panic!("PLENORA_TEST_LUNGHI non valido"),
    };
    let mut state = 17_u64;
    for _ in 0..count {
        let mut points = vec![[0, 0], [1000, 0], [1000, 1000], [0, 1000]];
        for _ in 0..24 {
            let mut p = [0; 2];
            for v in &mut p {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                *v = i32::try_from((state >> 32) % 1001).unwrap();
            }
            points.push(p);
        }
        let input: MultiPoint<f64> = points
            .iter()
            .map(|p| (f64::from(p[0]), f64::from(p[1])))
            .collect::<Vec<_>>()
            .into();
        for concavity in [0.5, 1.0, 2.0, 3.0] {
            let hull = input.concave_hull_with_options(geo::concave_hull::ConcaveHullOptions {
                concavity,
                length_threshold: 0.0,
            });
            verify(&points, &hull);
            assert_eq!(
                hull,
                input.concave_hull_with_options(geo::concave_hull::ConcaveHullOptions {
                    concavity,
                    length_threshold: 0.0
                })
            );
        }
    }
}
