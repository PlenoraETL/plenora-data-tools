//! Fonte: v6.0.0, `rust_backend::polygonize`; un reticolo cartesiano
//! [0,N]^2 con N+1 linee per asse ha N^2 facce limitate unitarie.
//! Pettine aciclico: nessuna faccia limitata (formula di Eulero).
#![allow(clippy::expect_used)]

use super::*;
use geo::MultiLineString;

fn options(node_input: bool) -> PolygonizeOptions {
    PolygonizeOptions {
        node_input,
        require_complete: false,
        limits: PolygonizeLimits {
            max_input_coordinates: 10000,
            max_noding_work: 1_000_000,
            max_output_geometries: 10000,
            max_output_coordinates: 100_000,
        },
        precision: 0.01,
    }
}

fn compare(input: &Geometry<f64>, options: PolygonizeOptions) {
    let current = polygonize_linework_rust_with_rounded(input, options);
    let baseline = baseline_600::polygonize_linework_rust_with_rounded(input, options);
    assert_eq!(format!("{current:?}"), format!("{baseline:?}"));
    let current = polygonize_linework_rust(input, options);
    let baseline = baseline_600::polygonize_linework_rust(input, options);
    assert_eq!(format!("{current:?}"), format!("{baseline:?}"));
    let current = polygonize_linework_rust_bounded(input, options);
    let baseline = baseline_600::polygonize_linework_rust_bounded(input, options);
    assert_eq!(format!("{current:?}"), format!("{baseline:?}"));
}

#[test]
fn published_polygonize_matches_literal_600_on_sourced_corpus() {
    for n in 1..=5 {
        let extent = f64::from(n);
        let lines: Vec<_> = (0..=n)
            .flat_map(|i| {
                let v = f64::from(i);
                [
                    LineString::from(vec![(v, 0.0), (v, extent)]),
                    LineString::from(vec![(0.0, v), (extent, v)]),
                ]
            })
            .collect();
        let input = Geometry::MultiLineString(MultiLineString::new(lines));
        let result = polygonize_linework_rust(&input, options(true)).expect("reticolo");
        assert_eq!(
            result.polygons.len(),
            usize::try_from(n * n).expect("numero facce")
        );
        assert_eq!(result.residual_count().expect("residui"), 0);
        compare(&input, options(true));
        compare(&input, options(false));
        let mut bounded = options(true);
        bounded.limits.max_noding_work = 0;
        compare(&input, bounded);
        bounded = options(true);
        bounded.limits.max_output_coordinates = 0;
        compare(&input, bounded);
        bounded = options(true);
        bounded.limits.max_input_coordinates = 0;
        compare(&input, bounded);
        bounded = options(true);
        bounded.limits.max_output_geometries = 0;
        compare(&input, bounded);
    }
    let mut comb = vec![LineString::from(vec![(0.0, 0.0), (20.0, 0.0)])];
    comb.extend((1..20).map(|i| LineString::from(vec![(f64::from(i), 0.0), (f64::from(i), 1.0)])));
    let comb = Geometry::MultiLineString(MultiLineString::new(comb));
    assert_eq!(
        polygonize_linework_rust(&comb, options(true))
            .expect("pettine")
            .polygons
            .len(),
        0
    );
    compare(&comb, options(true));
    let mut complete = options(true);
    complete.require_complete = true;
    compare(&comb, complete);
    // Fonte: contratto pubblicato, linework vuoto, non finito, dominio
    // numerico estremo, incrocio razionale non binario e tipo non lineare.
    for input in [
        Geometry::MultiLineString(MultiLineString::new(vec![])),
        Geometry::LineString(LineString::from(vec![(0.0, 0.0), (f64::NAN, 1.0)])),
        Geometry::LineString(LineString::from(vec![(-f64::MAX, 0.0), (f64::MAX, 1.0)])),
        Geometry::MultiLineString(MultiLineString::new(vec![
            LineString::from(vec![(0.0, 0.0), (1.0, 1.0)]),
            LineString::from(vec![(0.0, 1.0), (1.0, -1.0)]),
        ])),
        Geometry::Point(Point::new(0.0, 0.0)),
    ] {
        compare(&input, options(true));
    }
}
