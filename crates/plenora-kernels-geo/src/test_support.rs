//! Fixture condivise dai test di unita' del crate.
//!
//! Solo impalcatura: geometrie di prova, encoder di colonna e costruttori di
//! WKB little-endian scritto a mano. Nessuna asserzione vive qui, se non
//! `assert_close`, che e' la stessa tolleranza nei moduli che la usano.

use geo::{
    polygon, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon,
    Point, Polygon,
};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::BinaryArray;

// --- geometrie ---------------------------------------------------------------

/// Rettangolo con lati paralleli agli assi. L'anello e' antiorario e chiuso:
/// (xmin, ymin), (xmax, ymin), (xmax, ymax), (xmin, ymax), (xmin, ymin).
pub fn rect_polygon(xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> Polygon<f64> {
    polygon![
        (x: xmin, y: ymin), (x: xmax, y: ymin),
        (x: xmax, y: ymax), (x: xmin, y: ymax),
        (x: xmin, y: ymin),
    ]
}

/// [`rect_polygon`] come `Geometry`.
pub fn rect(xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> Geometry<f64> {
    Geometry::Polygon(rect_polygon(xmin, ymin, xmax, ymax))
}

/// Poligono «a farfalla» sul quadrato 0-2: l'anello esterno interseca se
/// stesso. Supera la decodifica strutturale e cade sulla validazione OGC.
pub fn bowtie() -> Geometry<f64> {
    Geometry::Polygon(polygon![
        (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
        (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
        (x: 0.0, y: 0.0),
    ])
}

// unnecessary_wraps: l'Option e' il contratto dei fixture (colonne con
// righe null), non un possibile fallimento dell'helper.
#[allow(clippy::unnecessary_wraps)]
pub fn some_point(x: f64, y: f64) -> Option<Geometry<f64>> {
    Some(Geometry::Point(Point::new(x, y)))
}

/// Il triangolo del corpus multi-tipo.
pub fn corpus_triangle() -> Polygon<f64> {
    Polygon::new(
        LineString::from(vec![(0.0, 0.0), (4.0, 0.0), (2.0, 3.0), (0.0, 0.0)]),
        Vec::new(),
    )
}

/// Il poligono con un buco del corpus multi-tipo.
pub fn corpus_holed() -> Polygon<f64> {
    Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]),
        vec![LineString::from(vec![
            (2.0, 2.0),
            (4.0, 2.0),
            (2.0, 4.0),
            (2.0, 2.0),
        ])],
    )
}

/// Corpus multi-tipo di geometrie valide: punti, linee, poligoni con e senza
/// buchi, multi-*, collection annidate, vuote.
///
/// La multipolygon e' del chiamante: `geometry_contract` e `decoded_size` ne
/// usano due diverse, e ciascuno la sua.
pub fn valid_corpus(multipolygon: MultiPolygon<f64>) -> Vec<(&'static str, Geometry<f64>)> {
    let triangle = Geometry::Polygon(corpus_triangle());
    let holed = Geometry::Polygon(corpus_holed());
    vec![
        ("point", Geometry::Point(Point::new(1.5, -2.5))),
        (
            "linestring",
            Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 0.5)])),
        ),
        (
            "linestring vuota",
            Geometry::LineString(LineString::from(Vec::<(f64, f64)>::new())),
        ),
        ("polygon semplice", triangle.clone()),
        ("polygon con buco", holed.clone()),
        (
            "multipoint",
            Geometry::MultiPoint(MultiPoint::new(vec![
                Point::new(0.0, 0.0),
                Point::new(3.0, 4.0),
            ])),
        ),
        (
            "multipoint vuota",
            Geometry::MultiPoint(MultiPoint::new(Vec::new())),
        ),
        (
            "multilinestring",
            Geometry::MultiLineString(MultiLineString::new(vec![
                LineString::from(vec![(0.0, 0.0), (1.0, 1.0)]),
                LineString::from(vec![(2.0, 2.0), (3.0, 3.0), (4.0, 2.0)]),
            ])),
        ),
        ("multipolygon", Geometry::MultiPolygon(multipolygon)),
        (
            "collection annidata",
            Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                Geometry::Point(Point::new(0.0, 0.0)),
                Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                    Geometry::LineString(LineString::from(vec![(0.0, 0.0), (5.0, 5.0)])),
                    triangle,
                ])),
                holed,
            ])),
        ),
        (
            "collection vuota",
            Geometry::GeometryCollection(GeometryCollection::new_from(Vec::new())),
        ),
    ]
}

// --- confronti ---------------------------------------------------------------

pub fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "atteso {expected}, ottenuto {actual}"
    );
}

// --- WKB via encoder -----------------------------------------------------------

/// WKB ISO XY prodotto da `geozero`.
pub fn to_wkb(geometry: &Geometry<f64>) -> Vec<u8> {
    geometry
        .to_wkb(CoordDimensions::xy())
        .expect("encode fixture")
}

/// Colonna binaria con i null preservati; `encode` e' l'encoder del chiamante.
pub fn wkb_column_with(
    geometries: &[Option<Geometry<f64>>],
    encode: impl Fn(&Geometry<f64>) -> Vec<u8>,
) -> BinaryArray {
    let cells: Vec<Option<Vec<u8>>> = geometries
        .iter()
        .map(|geometry| geometry.as_ref().map(&encode))
        .collect();
    cells.iter().map(|cell| cell.as_deref()).collect()
}

// --- WKB little-endian scritto a mano ------------------------------------------
//
// A mano e non con un encoder: cio' che questi casi fissano sono i byte che
// entrano nel confine, non il comportamento di chi li produce.

/// Header little-endian di una geometria con il type code dato.
pub fn push_header(payload: &mut Vec<u8>, raw_type: u32) {
    payload.push(1_u8);
    payload.extend_from_slice(&raw_type.to_le_bytes());
}

/// Conteggio (punti, anelli o membri) little-endian.
pub fn push_count(payload: &mut Vec<u8>, count: usize) {
    payload.extend_from_slice(
        &u32::try_from(count)
            .expect("fixture entro u32")
            .to_le_bytes(),
    );
}

/// Coordinata con X, Y e le ordinate extra (Z e/o M, nell'ordine del
/// type code): lo stride e' 16 + 8 * `extra.len()`.
pub fn push_coordinate(payload: &mut Vec<u8>, x: f64, y: f64, extra: &[f64]) {
    payload.extend_from_slice(&x.to_le_bytes());
    payload.extend_from_slice(&y.to_le_bytes());
    for value in extra {
        payload.extend_from_slice(&value.to_le_bytes());
    }
}

/// Conteggio seguito dalle coordinate XY.
fn push_points(payload: &mut Vec<u8>, points: &[(f64, f64)]) {
    push_count(payload, points.len());
    for &(x, y) in points {
        push_coordinate(payload, x, y, &[]);
    }
}

pub fn point_wkb_le(x: f64, y: f64) -> Vec<u8> {
    let mut payload = Vec::new();
    push_header(&mut payload, 1);
    push_coordinate(&mut payload, x, y, &[]);
    payload
}

pub fn linestring_wkb_le(points: &[(f64, f64)]) -> Vec<u8> {
    let mut payload = Vec::new();
    push_header(&mut payload, 2);
    push_points(&mut payload, points);
    payload
}

/// Poligono con il solo anello esterno, scritto cosi' com'e': ne' chiuso ne'
/// contato al posto del chiamante.
pub fn polygon_wkb_le(ring: &[(f64, f64)]) -> Vec<u8> {
    polygon_with_interiors_wkb_le(ring, &[])
}

/// Poligono con anelli interni: e' la forma su cui un decoder che
/// gestisse un solo anello perderebbe l'esterno.
pub fn polygon_with_interiors_wkb_le(
    exterior: &[(f64, f64)],
    interiors: &[&[(f64, f64)]],
) -> Vec<u8> {
    let mut payload = Vec::new();
    push_header(&mut payload, 3);
    push_count(&mut payload, 1 + interiors.len());
    for ring in std::iter::once(exterior).chain(interiors.iter().copied()) {
        push_points(&mut payload, ring);
    }
    payload
}

pub fn multipoint_wkb_le(points: &[(f64, f64)]) -> Vec<u8> {
    let parts: Vec<Vec<u8>> = points.iter().map(|&(x, y)| point_wkb_le(x, y)).collect();
    container_wkb_le(4, &parts)
}

pub fn multipolygon_wkb_le(polygons: &[Vec<u8>]) -> Vec<u8> {
    container_wkb_le(6, polygons)
}

pub fn collection_wkb_le(parts: &[Vec<u8>]) -> Vec<u8> {
    container_wkb_le(7, parts)
}

/// Multi-geometria o collection: header, conteggio, membri gia' codificati.
fn container_wkb_le(raw_type: u32, parts: &[Vec<u8>]) -> Vec<u8> {
    let mut payload = Vec::new();
    push_header(&mut payload, raw_type);
    push_count(&mut payload, parts.len());
    for part in parts {
        payload.extend_from_slice(part);
    }
    payload
}
