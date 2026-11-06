//! Oracoli delle 17 trasformazioni, prodotti da implementazioni esterne.
//! `genera_trasformazioni.py` fissa versioni, fixture e algoritmi.
// Formule elementari dell'oracolo, volutamente senza riscritture FMA.
#![allow(clippy::suboptimal_flops)]
use geo::{Coord, Geometry, LineString, MultiLineString, MultiPolygon, Point, Polygon};
use serde_json::Value;

fn coord(value: &Value) -> Coord<f64> {
    let pair = value.as_array().expect("coordinata JSON");
    assert_eq!(pair.len(), 2);
    Coord {
        x: pair[0].as_f64().unwrap(),
        y: pair[1].as_f64().unwrap(),
    }
}
fn line(value: &Value) -> LineString<f64> {
    LineString::new(value.as_array().unwrap().iter().map(coord).collect())
}
fn polygon(value: &Value) -> Polygon<f64> {
    let rings = value.as_array().unwrap();
    Polygon::new(line(&rings[0]), rings[1..].iter().map(line).collect())
}
fn geometry(value: &Value) -> Geometry<f64> {
    let c = &value["coordinates"];
    match value["type"].as_str().unwrap() {
        "Point" => Geometry::Point(Point::from(coord(c))),
        "LineString" => Geometry::LineString(line(c)),
        "MultiLineString" => Geometry::MultiLineString(MultiLineString(
            c.as_array().unwrap().iter().map(line).collect(),
        )),
        "Polygon" => Geometry::Polygon(polygon(c)),
        "MultiPolygon" => Geometry::MultiPolygon(MultiPolygon(
            c.as_array().unwrap().iter().map(polygon).collect(),
        )),
        _ => panic!("tipo golden non censito"),
    }
}
pub fn casi() -> Vec<(String, Vec<Option<Geometry<f64>>>)> {
    let golden: Value = serde_json::from_str(include_str!("trasformazioni_attese.json")).unwrap();
    golden["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            (
                case["op"].as_str().unwrap().to_owned(),
                case["geometries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| (!v.is_null()).then(|| geometry(v)))
                    .collect(),
            )
        })
        .collect()
}
fn distance(a: Coord<f64>, b: Coord<f64>) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}
fn point_segment(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    distance(
        p,
        Coord {
            x: a.x + t * dx,
            y: a.y + t * dy,
        },
    )
}
fn ring_distance(p: Coord<f64>, ring: &LineString<f64>) -> f64 {
    ring.0
        .windows(2)
        .map(|s| point_segment(p, s[0], s[1]))
        .fold(f64::INFINITY, f64::min)
}
/// Distanza diretta continua, certificata dalla proprieta' 1-Lipschitz
/// della distanza da un insieme. Nessun campionamento a passo fisso:
/// si suddivide finche' il limite superiore dell'intero segmento e' <=9.9mm.
/// Il margine residuo copre l'approssimazione GEOS degli archi (<0.024mm).
fn within_cm(source: &LineString<f64>, target: &LineString<f64>) {
    let mut pending: Vec<_> = source
        .0
        .windows(2)
        .map(|s| {
            (
                s[0],
                s[1],
                ring_distance(s[0], target),
                ring_distance(s[1], target),
            )
        })
        .collect();
    let mut subdivisions = 0;
    while let Some((a, b, da, db)) = pending.pop() {
        assert!(
            da <= 0.0099 && db <= 0.0099,
            "buffer oltre 1 cm dal riferimento indipendente"
        );
        if (da + db + distance(a, b)) * 0.5 <= 0.0099 {
            continue;
        }
        let mid = Coord {
            x: a.x.midpoint(b.x),
            y: a.y.midpoint(b.y),
        };
        let dm = ring_distance(mid, target);
        pending.push((a, mid, da, dm));
        pending.push((mid, b, dm, db));
        subdivisions += 1;
        assert!(
            subdivisions < 1_000_000,
            "distanza non certificabile nel margine della prova"
        );
    }
}
fn area(ring: &LineString<f64>) -> f64 {
    let origin = ring.0[0];
    ring.0
        .windows(2)
        .map(|s| {
            (s[0].x - origin.x) * (s[1].y - origin.y) - (s[1].x - origin.x) * (s[0].y - origin.y)
        })
        .sum::<f64>()
        .abs()
        * 0.5
}
fn simple(ring: &LineString<f64>) {
    assert_eq!(ring.0.first(), ring.0.last());
    assert!(ring.0.len() >= 4);
    let orient = |a: Coord<f64>, b: Coord<f64>, c: Coord<f64>| {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    };
    for (i, a) in ring.0.windows(2).enumerate() {
        assert!(distance(a[0], a[1]) > 0.0);
        for (j, b) in ring.0.windows(2).enumerate().skip(i + 2) {
            if i == 0 && j == ring.0.len() - 2 {
                continue;
            }
            let c1 = orient(a[0], a[1], b[0]);
            let c2 = orient(a[0], a[1], b[1]);
            let c3 = orient(b[0], b[1], a[0]);
            let c4 = orient(b[0], b[1], a[1]);
            assert!(
                !(c1 * c2 < 0.0 && c3 * c4 < 0.0),
                "contorno autointersecante"
            );
            assert!(
                point_segment(a[0], b[0], b[1]) > 0.0
                    && point_segment(a[1], b[0], b[1]) > 0.0
                    && point_segment(b[0], a[0], a[1]) > 0.0
                    && point_segment(b[1], a[0], a[1]) > 0.0,
                "contatto o sovrapposizione fra lati non adiacenti"
            );
        }
    }
}
fn canonical_polygon(geometry: &Geometry<f64>) -> Vec<(u64, u64)> {
    let Geometry::Polygon(p) = geometry else {
        panic!("concave hull non poligonale");
    };
    assert!(p.interiors().is_empty());
    let mut ring: Vec<_> = p
        .exterior()
        .0
        .iter()
        .map(|c| (c.x.to_bits(), c.y.to_bits()))
        .collect();
    assert_eq!(ring.first(), ring.last());
    ring.pop();
    let mut variants = Vec::new();
    for reverse in [false, true] {
        let mut ring = ring.clone();
        if reverse {
            ring.reverse();
        }
        for _ in 0..ring.len() {
            variants.push(ring.clone());
            ring.rotate_left(1);
        }
    }
    variants.into_iter().min().unwrap()
}
pub fn verifica(op: &str, actual: &Geometry<f64>, expected: &Geometry<f64>) {
    if op == "geo.concave_hull" {
        let golden: Value =
            serde_json::from_str(include_str!("trasformazioni_attese.json")).unwrap();
        let terminali: Vec<_> = golden["concave_allowed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| canonical_polygon(&geometry(g)))
            .collect();
        assert_eq!(terminali.len(), 1, "corpus di pareggi cambiato");
        assert!(
            terminali.contains(&canonical_polygon(actual)),
            "bordo fuori dalle soluzioni terminali del modello razionale"
        );
        return;
    }
    if op == "geo.buffer" {
        let (Geometry::MultiPolygon(actual), Geometry::MultiPolygon(expected)) = (actual, expected)
        else {
            panic!("buffer deve produrre MultiPolygon");
        };
        assert_eq!(actual.0.len(), 1);
        assert_eq!(expected.0.len(), 1);
        let (a, e) = (&actual.0[0], &expected.0[0]);
        assert!(a.interiors().is_empty() && e.interiors().is_empty());
        simple(a.exterior());
        within_cm(a.exterior(), e.exterior());
        within_cm(e.exterior(), a.exterior());
        let perimeter: f64 = e
            .exterior()
            .0
            .windows(2)
            .map(|s| distance(s[0], s[1]))
            .sum();
        assert!((area(a.exterior()) - area(e.exterior())).abs() <= perimeter * 0.01);
        return;
    }
    match (actual, expected) {
        (Geometry::Point(a), Geometry::Point(e)) => assert!(distance(a.0, e.0) <= 1e-8),
        (Geometry::LineString(a), Geometry::LineString(e)) => ordered(a, e),
        (Geometry::MultiLineString(a), Geometry::MultiLineString(e)) => {
            assert_eq!(a.0.len(), e.0.len());
            for (a, e) in a.0.iter().zip(&e.0) {
                ordered(a, e);
            }
        }
        (Geometry::Polygon(_), Geometry::Polygon(_)) => {
            crate::comune_geo::assert_poligoni(actual, expected);
        }
        _ => panic!("tipo geometrico diverso dall'atteso indipendente"),
    }
}
fn ordered(actual: &LineString<f64>, expected: &LineString<f64>) {
    assert_eq!(actual.0.len(), expected.0.len());
    for (&a, &e) in actual.0.iter().zip(&expected.0) {
        assert!(distance(a, e) <= 1e-8, "vertice lineare diverso");
    }
}
