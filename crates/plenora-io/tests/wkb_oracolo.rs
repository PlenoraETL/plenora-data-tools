//! Oracolo della camminata WKB: tipo di primo livello e riquadro XY contro
//! la codifica di riferimento dei kernel (`encode_geometry`) e le
//! coordinate di `geo::CoordsIter`, geometria per geometria. Il riquadro
//! comprende tutte le coordinate, anelli interni compresi (`BoundingRect` di
//! `geo` guarda solo l'esterno dei poligoni).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use geo::{
    Coord, CoordsIter, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint,
    MultiPolygon, Point, Polygon,
};
use plenora_core::contract::GeometryType;
use plenora_io::wkb::{scansiona_cella, Riquadro, Sommario};
use plenora_kernels_geo::arrow_adapter::encode_geometry;
use proptest::prelude::*;

fn coordinata() -> impl Strategy<Value = Coord<f64>> {
    (-1.0e7..1.0e7_f64, -1.0e7..1.0e7_f64).prop_map(|(x, y)| Coord { x, y })
}

fn linea() -> impl Strategy<Value = LineString<f64>> {
    prop::collection::vec(coordinata(), 2..6).prop_map(LineString::new)
}

fn anello() -> impl Strategy<Value = LineString<f64>> {
    prop::collection::vec(coordinata(), 3..6).prop_map(|mut punti| {
        punti.push(punti[0]);
        LineString::new(punti)
    })
}

fn poligono() -> impl Strategy<Value = Polygon<f64>> {
    (anello(), prop::collection::vec(anello(), 0..2))
        .prop_map(|(esterno, interni)| Polygon::new(esterno, interni))
}

fn semplice() -> impl Strategy<Value = Geometry<f64>> {
    prop_oneof![
        coordinata().prop_map(|c| Geometry::Point(Point(c))),
        linea().prop_map(Geometry::LineString),
        poligono().prop_map(Geometry::Polygon),
        prop::collection::vec(coordinata().prop_map(Point), 0..4)
            .prop_map(|p| Geometry::MultiPoint(MultiPoint(p))),
        prop::collection::vec(linea(), 0..3)
            .prop_map(|l| Geometry::MultiLineString(MultiLineString(l))),
        prop::collection::vec(poligono(), 0..3)
            .prop_map(|p| Geometry::MultiPolygon(MultiPolygon(p))),
    ]
}

fn geometria() -> impl Strategy<Value = Geometry<f64>> {
    prop_oneof![
        4 => semplice(),
        1 => prop::collection::vec(semplice(), 0..3)
            .prop_map(|g| Geometry::GeometryCollection(GeometryCollection(g))),
    ]
}

const fn tipo(geometria: &Geometry<f64>) -> GeometryType {
    match geometria {
        Geometry::Point(_) => GeometryType::Point,
        Geometry::LineString(_) | Geometry::Line(_) => GeometryType::LineString,
        Geometry::Polygon(_) | Geometry::Rect(_) | Geometry::Triangle(_) => GeometryType::Polygon,
        Geometry::MultiPoint(_) => GeometryType::MultiPoint,
        Geometry::MultiLineString(_) => GeometryType::MultiLineString,
        Geometry::MultiPolygon(_) => GeometryType::MultiPolygon,
        Geometry::GeometryCollection(_) => GeometryType::GeometryCollection,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn tipo_e_riquadro_come_il_riferimento(geometrie in prop::collection::vec(geometria(), 1..5)) {
        let mut sommario = Sommario::default();
        let mut atteso: Option<geo::Rect<f64>> = None;
        let mut tipi = std::collections::BTreeSet::new();
        for g in &geometrie {
            let cella = encode_geometry(g).expect("codifica di riferimento");
            scansiona_cella(&cella, &mut sommario).expect("camminata");
            tipi.insert((tipo(g), false));
            for c in g.coords_iter() {
                atteso = Some(atteso.map_or_else(
                    || geo::Rect::new(c, c),
                    |a| geo::Rect::new(
                        Coord { x: a.min().x.min(c.x), y: a.min().y.min(c.y) },
                        Coord { x: a.max().x.max(c.x), y: a.max().y.max(c.y) },
                    ),
                ));
            }
        }
        prop_assert_eq!(sommario.tipi, tipi);
        let atteso = atteso.map(|r| Riquadro {
            xmin: r.min().x,
            ymin: r.min().y,
            xmax: r.max().x,
            ymax: r.max().y,
        });
        prop_assert_eq!(sommario.riquadro, atteso);
    }

    #[test]
    fn i_prefissi_troncati_si_rifiutano(g in geometria(), taglio in 0.0..1.0_f64) {
        let cella = encode_geometry(&g).expect("codifica di riferimento");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        let n = ((cella.len() as f64) * taglio) as usize;
        prop_assume!(n < cella.len());
        prop_assert!(scansiona_cella(&cella[..n], &mut Sommario::default()).is_err());
    }
}
