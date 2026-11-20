//! O-M4: baseline 6.1.0 congelata; ordine e bit, degeneri inclusi.
use super::*;
use crate::advanced::{voronoi_cells, voronoi_cells_distinct};
use crate::rust_backend::precision::Precision;
use geo::{CoordsIter, Geometry, Point};
use proptest::prelude::*;

fn bits(points: &[Coord<f64>]) -> Vec<(u64, u64)> {
    points
        .iter()
        .map(|p| (p.x.to_bits(), p.y.to_bits()))
        .collect()
}

fn compare(points: &[Coord<f64>]) {
    assert_eq!(
        bits(&siti_distinti(points)),
        bits(&baseline_610::siti_distinti(points))
    );
}

proptest! {
    #[test]
    fn dedup_preserves_total_order(points in prop::collection::vec((-8i32..8, -8i32..8, any::<bool>()), 0..300)) {
        let points: Vec<_> = points.into_iter().map(|(x,y,negative)| Coord {
            x: if x == 0 && negative { -0.0 } else { f64::from(x) },
            y: if y == 0 && negative { -0.0 } else { f64::from(y) },
        }).collect();
        compare(&points);
    }
}

#[test]
fn zero_bits_through_collinear_transition() {
    let points = [
        (0.0, 0.0),
        (-0.0, -0.0),
        (1.0, 0.0),
        (0.0, 0.0),
        (2.0, 0.0),
        (-0.0, 0.0),
        (0.0, 1.0),
        (0.0, -0.0),
    ];
    let points: Vec<_> = points.into_iter().map(|(x, y)| Coord { x, y }).collect();
    for len in 0..=points.len() {
        compare(&points[..len]);
    }
}

#[cfg(test)]
fn fingerprint(
    result: Result<Vec<Geometry<f64>>, impl std::fmt::Debug>,
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

#[test]
fn voronoi_baseline_bits_and_errors() {
    let precision = Precision::new(1.0e-4).unwrap();
    let fixtures = [
        vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)],
        vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0), (1.0, 2.0)],
        vec![(0.0, 0.0), (-0.0, -0.0), (1.0, 0.0)],
        vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)],
        vec![(0.0, 0.0), (f64::MAX, 0.0), (0.0, 1.0)],
        vec![(0.0, 0.0), (f64::NAN, 0.0), (0.0, 1.0)],
    ];
    for points in fixtures {
        let inputs: Vec<_> = points
            .into_iter()
            .map(|(x, y)| Geometry::Point(Point::new(x, y)))
            .collect();
        let expected = fingerprint(voronoi_610::voronoi_cells(&inputs, 100, precision));
        assert_eq!(
            fingerprint(voronoi_cells(&inputs, 100, precision)),
            expected
        );
        assert_eq!(
            fingerprint(voronoi_cells_distinct(&inputs, 100, precision).map(|r| r.materialize())),
            expected
        );
    }
}

/// O-M3: n/2n/4n copie non duplicano le coordinate delle celle condivise.
#[test]
fn distinct_cells_scale_with_sites_not_input_copies() {
    let precision = Precision::new(1.0e-4).unwrap();
    for copies in [256, 512, 1024] {
        let mut inputs = vec![Geometry::Point(Point::new(0.0, 0.0)); copies];
        inputs.extend(
            [(4.0, 0.0), (4.0, 4.0), (0.0, 4.0)].map(|(x, y)| Geometry::Point(Point::new(x, y))),
        );
        let result = voronoi_cells_distinct(&inputs, 2048, precision).unwrap();
        assert_eq!(result.cells().len(), 4);
        assert_eq!(result.input_indices().len(), copies + 3);
        assert!(result.input_indices()[..copies]
            .iter()
            .all(|i| *i == result.input_indices()[0]));
        let shared: usize = result.cells().iter().map(CoordsIter::coords_count).sum();
        let expanded: usize = result.per_input().map(CoordsIter::coords_count).sum();
        assert!(expanded > shared * copies / 8);
        assert_eq!(
            fingerprint(Ok::<_, ()>(result.materialize())),
            fingerprint(voronoi_610::voronoi_cells(&inputs, 2048, precision))
        );
    }
}

/// O-M1: quadrato e centro interi. I quattro circocentri sono esatti;
/// i semipiani equidistanti del sito centrale delimitano il rombo dato.
#[test]
#[allow(
    clippy::float_cmp,
    clippy::suboptimal_flops,
    reason = "Interi esatti binary64: coordinate fra zero e quattro, distanze quadrate al piu 32"
)]
fn exact_voronoi_delaunay_duality_square_and_center() {
    let points = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0), (2.0, 2.0)];
    let input: Vec<_> = points
        .map(|(x, y)| Geometry::Point(Point::new(x, y)))
        .into();
    let cells = voronoi_cells_distinct(&input, 5, Precision::new(1.0e-4).unwrap()).unwrap();
    let center = &cells.cells()[cells.input_indices()[4]];
    let mut vertices = bits(&center.exterior().0[..4]);
    vertices.sort_unstable();
    let centers = [(2.0, 0.0), (4.0, 2.0), (2.0, 4.0), (0.0, 2.0)];
    let mut expected: Vec<_> = centers
        .map(|(x, y): (f64, f64)| (x.to_bits(), y.to_bits()))
        .into();
    expected.sort_unstable();
    assert_eq!(vertices, expected);
    let input = Geometry::MultiPoint(geo::MultiPoint::from(points.to_vec()));
    let triangles = crate::extended_algorithms::delaunay(&input, 5, 4).unwrap();
    assert_eq!(triangles.len(), 4);
    for (x, y) in centers {
        // Distanze quadrate intere esatte: esattamente tre siti incidenti,
        // nessun sito dentro il cerchio (raggio quadrato 4).
        let distances: Vec<_> = points
            .iter()
            .map(|&(px, py)| (px - x) * (px - x) + (py - y) * (py - y))
            .collect();
        assert_eq!(distances.iter().filter(|&&d| d == 4.0).count(), 3);
        assert!(distances.iter().all(|&d| d >= 4.0));
        assert!(triangles.iter().any(|t| t.exterior().0[..3]
            .iter()
            .all(|p| (p.x - x) * (p.x - x) + (p.y - y) * (p.y - y) == 4.0)));
    }
}
