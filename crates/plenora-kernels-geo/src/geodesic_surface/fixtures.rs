//! Lettura del corpus indipendente `GeographicLib` Python.
#![allow(
    clippy::redundant_pub_crate,
    reason = "Visibilita esplicita del supporto condiviso interno al crate"
)]
use geo::{Coord, Geometry, LineString, MultiPolygon, Point, Polygon};
// G2 Python 2.1, reference.py: coordinate e bracket metrici generati
// indipendentemente; winding locale solo per le fixture con anelli convessi.
pub(crate) fn fixture_geometry(value: &serde_json::Value, reverse: bool) -> Geometry {
    let parts: Vec<LineString> = value["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| {
            let mut coords: Vec<Coord> = part
                .as_array()
                .unwrap()
                .iter()
                .map(|p| Coord {
                    x: p[0].as_f64().unwrap(),
                    y: p[1].as_f64().unwrap(),
                })
                .collect();
            if reverse {
                coords.reverse();
            }
            LineString::new(coords)
        })
        .collect();
    match value["kind"].as_str().unwrap() {
        "point" => Point(parts[0].0[0]).into(),
        "line" => parts[0].clone().into(),
        "polygon" => Polygon::new(parts[0].clone(), parts[1..].to_vec()).into(),
        "multi_polygon" => MultiPolygon::new(
            parts
                .into_iter()
                .map(|ring| Polygon::new(ring, vec![]))
                .collect(),
        )
        .into(),
        _ => unreachable!(),
    }
}
