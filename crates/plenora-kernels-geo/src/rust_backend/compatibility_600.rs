//! Fonte: API `polygonize_linework` del tag v6.0.0. Reticoli cartesiani
//! con N^2 facce limitate; opzioni e limiti attraversano anche il wrapper.
use super::*;
use geo::{LineString, MultiLineString};

#[test]
fn published_wrapper_matches_literal_600() {
    let precision = Precision::new(0.01).expect("precisione metrica");
    for n in 1..=4 {
        let end = f64::from(n);
        let input = Geometry::MultiLineString(MultiLineString::new(
            (0..=n)
                .flat_map(|i| {
                    let v = f64::from(i);
                    [
                        LineString::from(vec![(v, 0.0), (v, end)]),
                        LineString::from(vec![(0.0, v), (end, v)]),
                    ]
                })
                .collect(),
        ));
        for node in [false, true] {
            for complete in [false, true] {
                for limits in [
                    [1000, 10000, 1000, 10000],
                    [0, 10000, 1000, 10000],
                    [1000, 0, 1000, 10000],
                    [1000, 10000, 0, 10000],
                    [1000, 10000, 1000, 0],
                    [u64::MAX; 4],
                ] {
                    let [coordinates, work, geometries, output] = limits;
                    let current = polygonize_linework(
                        &input,
                        node,
                        complete,
                        coordinates,
                        work,
                        geometries,
                        output,
                        precision,
                    );
                    let baseline = baseline_600::polygonize_linework(
                        &input,
                        node,
                        complete,
                        coordinates,
                        work,
                        geometries,
                        output,
                        precision,
                    );
                    assert_eq!(format!("{current:?}"), format!("{baseline:?}"));
                }
            }
        }
    }
}
