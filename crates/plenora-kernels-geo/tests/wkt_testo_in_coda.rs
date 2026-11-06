//! Il WKT e' tutto il testo, non il suo inizio.
//!
//! Il parser di `wkt` 0.14 si ferma alla fine della geometria di primo livello
//! e non guarda il resto, e il suo tokenizer tratta `\0` come fine
//! dell'ingresso: senza il controllo di `geometry_from_wkt` un testo che dice di
//! piu' di quel che viene letto sarebbe accettato, e la parte in piu' scartata
//! in silenzio.

use plenora_kernels_geo::construction::{geometry_from_wkt, ConstructionError};

fn rifiutato(testo: &str) {
    assert!(
        matches!(
            geometry_from_wkt(testo),
            Err(ConstructionError::InvalidWkt(_))
        ),
        "{testo:?} deve essere rifiutato come WKT non valido, ottenuto {:?}",
        geometry_from_wkt(testo)
    );
}

fn accettato(testo: &str, atteso: &geo::Geometry<f64>) {
    assert_eq!(geometry_from_wkt(testo).expect("WKT valido"), *atteso);
}

#[test]
fn il_testo_dopo_la_geometria_e_rifiutato() {
    rifiutato("POINT(1 2) garbage");
    rifiutato("POINT(1 2))");
    rifiutato("POINT(1 2),");
    rifiutato("POINT(1 2) POINT(3 4)");
    rifiutato("POLYGON((0 0,1 0,1 1,0 0)) resto");
    rifiutato("GEOMETRYCOLLECTION(POINT(1 2)) x");
}

#[test]
fn un_nul_e_rifiutato_ovunque() {
    rifiutato("POINT(1 2)\0");
    rifiutato("POINT(1 2)\0garbage");
    rifiutato("POINT(1\0 2)");
}

#[test]
fn dopo_empty_non_c_e_altro() {
    rifiutato("POINT EMPTY garbage");
    rifiutato("POINT EMPTY (1 2)");
}

/// Controcasi: lo spazio bianco del tokenizer in coda, le collezioni
/// annidate e le forme vuote restano valide.
#[test]
fn il_wkt_valido_resta_valido() {
    accettato(
        "POINT(1 2)",
        &geo::Geometry::Point(geo::Point::new(1.0, 2.0)),
    );
    accettato(
        "POINT (1 2)\n",
        &geo::Geometry::Point(geo::Point::new(1.0, 2.0)),
    );
    accettato(
        " \tPOINT (1 2) \t\r\n",
        &geo::Geometry::Point(geo::Point::new(1.0, 2.0)),
    );
    accettato(
        "POINT EMPTY",
        &geo::Geometry::MultiPoint(geo::MultiPoint(vec![])),
    );
    accettato(
        "point empty ",
        &geo::Geometry::MultiPoint(geo::MultiPoint(vec![])),
    );
    accettato(
        "GEOMETRYCOLLECTION(POINT EMPTY,POINT(1 2))",
        &geo::Geometry::GeometryCollection(geo::GeometryCollection(vec![
            geo::Geometry::MultiPoint(geo::MultiPoint(vec![])),
            geo::Geometry::Point(geo::Point::new(1.0, 2.0)),
        ])),
    );
    accettato(
        "MULTIPOLYGON(((0 0,1 0,1 1,0 0)),((5 5,6 5,6 6,5 5)))",
        &geo::Geometry::MultiPolygon(geo::MultiPolygon(vec![
            geo::Polygon::new(
                geo::LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0)]),
                vec![],
            ),
            geo::Polygon::new(
                geo::LineString::from(vec![(5.0, 5.0), (6.0, 5.0), (6.0, 6.0), (5.0, 5.0)]),
                vec![],
            ),
        ])),
    );
    accettato(
        "POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,2 1,2 2,1 1))",
        &geo::Geometry::Polygon(geo::Polygon::new(
            geo::LineString::from(vec![
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ]),
            vec![geo::LineString::from(vec![
                (1.0, 1.0),
                (2.0, 1.0),
                (2.0, 2.0),
                (1.0, 1.0),
            ])],
        )),
    );
}

/// `EMPTY` seguito da un segno: il tokenizer chiude la parola davanti a `)` e
/// `,`, e il resto non verrebbe letto.
#[test]
fn empty_seguito_da_un_segno_e_rifiutato() {
    rifiutato("GEOMETRYCOLLECTION EMPTY)");
    rifiutato("GEOMETRYCOLLECTION EMPTY,resto");
    rifiutato("POINT EMPTY,");
}

/// Una Z o una M che il prefisso non mostra si perderebbe nella conversione
/// in `geo`, che conserva x e y: sono rifiutate, attaccate al tipo o annidate.
#[test]
fn le_dimensioni_oltre_xy_sono_rifiutate_ovunque() {
    for testo in [
        "POINTZ(1 2 3)",
        "POINTM(1 2 3)",
        "POINTZM(1 2 3 4)",
        "GEOMETRYCOLLECTION(POINT Z(1 2 3))",
        "GEOMETRYCOLLECTION(POINT(1 2),LINESTRING Z(0 0 1,1 1 1))",
    ] {
        assert!(
            matches!(
                geometry_from_wkt(testo),
                Err(ConstructionError::UnsupportedWktDimension)
            ),
            "{testo:?} deve essere rifiutato per dimensione: {:?}",
            geometry_from_wkt(testo)
        );
    }
    accettato(
        "GEOMETRYCOLLECTION(POINT(1 2),LINESTRING(0 0,1 1))",
        &geo::Geometry::GeometryCollection(geo::GeometryCollection(vec![
            geo::Geometry::Point(geo::Point::new(1.0, 2.0)),
            geo::Geometry::LineString(geo::LineString::from(vec![(0.0, 0.0), (1.0, 1.0)])),
        ])),
    );
}
