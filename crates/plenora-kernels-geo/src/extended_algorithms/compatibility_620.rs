//! O-M4: confronto integrale contro 6.1.0, anche quando Delaunay non e' unica.
use super::*;
use proptest::prelude::*;

#[cfg(test)]
fn fingerprint(
    result: Result<Vec<Polygon<f64>>, ExtendedAlgorithmError>,
) -> Result<Vec<Vec<(u64, u64)>>, String> {
    result
        .map(|cells| {
            cells
                .iter()
                .map(|g| {
                    g.coords_iter()
                        .map(|c| (c.x.to_bits(), c.y.to_bits()))
                        .collect()
                })
                .collect()
        })
        .map_err(|e| format!("{e:?}"))
}

fn compare(points: Vec<(f64, f64)>, limit: u64) {
    let geometry = Geometry::MultiPoint(geo::MultiPoint::from(points));
    assert_eq!(
        fingerprint(delaunay(&geometry, 1000, limit)),
        fingerprint(delaunay_610::delaunay(&geometry, 1000, limit))
    );
}

proptest! {
    #[test]
    fn all_bits_and_order(points in prop::collection::vec((-16i32..16, -16i32..16), 0..100)) {
        compare(points.into_iter().map(|(x,y)| (f64::from(x),f64::from(y))).collect(),1000);
    }
}

#[test]
fn degenerate_and_extreme_bits() {
    for scale in [2.0_f64.powi(-140), 1.0, 2.0_f64.powi(199)] {
        for limit in [0, 1, 1000] {
            compare(
                vec![
                    (0.0, -0.0),
                    (scale, 0.0),
                    (scale, scale),
                    (0.0, scale),
                    (-0.0, 0.0),
                ],
                limit,
            );
            compare(vec![(0.0, 0.0), (scale, 0.0), (scale * 2.0, 0.0)], limit);
        }
    }
    compare(vec![(f64::MAX, 0.0), (0.0, 0.0), (0.0, 1.0)], 1000);
}
