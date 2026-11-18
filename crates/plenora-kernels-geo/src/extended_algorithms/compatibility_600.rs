//! Fonte: v6.0.0, `extended_algorithms::split_line`; costruzioni cartesiane
//! con intersezioni intere. N lame verticali distinte interne a [0,N+1]
//! producono N+1 intervalli. La baseline conserva anche rifiuti e ordine.
use super::*;
use geo::MultiLineString;

fn compare(source: &LineString<f64>, blade: &Geometry<f64>, tolerance: f64, limits: [u64; 4]) {
    let [coordinates, work, parts, output] = limits;
    let current = split_line(source, blade, tolerance, coordinates, work, parts, output);
    let baseline =
        baseline_600::split_line(source, blade, tolerance, coordinates, work, parts, output);
    // Debug preserva anche il segno dello zero; gli errori sono confrontati
    // per variante e campi, non soltanto per il messaggio.
    assert_eq!(format!("{current:?}"), format!("{baseline:?}"));
}

#[test]
fn published_split_matches_literal_600_on_sourced_corpus() {
    for n in 1..=12 {
        let source = LineString::from(vec![(0.0, 0.0), (f64::from(n + 1), 0.0)]);
        let blade = Geometry::MultiLineString(MultiLineString::new(
            (1..=n)
                .map(|i| LineString::from(vec![(f64::from(i), -1.0), (f64::from(i), 1.0)]))
                .collect(),
        ));
        let result =
            split_line(&source, &blade, 0.0, 1000, 1000, 1000, 1000).expect("reticolo lineare");
        assert_eq!(result.len(), usize::try_from(n + 1).expect("numero pezzi"));
        compare(&source, &blade, 0.0, [1000; 4]);
        for axis in 0..4 {
            let mut limits = [1000; 4];
            limits[axis] = 0;
            compare(&source, &blade, 0.0, limits);
        }
        compare(&source, &blade, -1.0, [1000; 4]);
        compare(&source, &blade, f64::NAN, [1000; 4]);
    }
    // Fonte: contratto 6.0.0, proiezione di punti, overlap, inversione,
    // vertice condiviso, nessun incrocio, non finiti e overflow lunghezza.
    let source = LineString::from(vec![(-0.0, 0.0), (5.0, 0.0), (10.0, 0.0)]);
    let blades = [
        Geometry::Point(Point::new(5.0, 0.0)),
        Geometry::Point(Point::new(5.0, 0.25)),
        Geometry::LineString(LineString::from(vec![(8.0, 0.0), (2.0, 0.0)])),
        Geometry::LineString(LineString::from(vec![(20.0, -1.0), (20.0, 1.0)])),
        Geometry::Point(Point::new(f64::NAN, 0.0)),
        Geometry::Line(Line::new((0.0, 0.0), (1.0, 1.0))),
    ];
    for blade in &blades {
        for tolerance in [0.0, 0.5] {
            compare(&source, blade, tolerance, [1000; 4]);
            let mut reversed = source.clone();
            reversed.0.reverse();
            compare(&reversed, blade, tolerance, [1000; 4]);
        }
    }
    for source in [
        LineString::new(vec![]),
        LineString::from(vec![(-f64::MAX, 0.0), (f64::MAX, 0.0)]),
    ] {
        compare(&source, &blades[0], 0.0, [1000; 4]);
    }
}
