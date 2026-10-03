//! Codifica WKB ISO XY little-endian dell'output dei kernel, con il
//! poligono vuoto a **zero anelli**: la usa ogni cella geometria d'uscita,
//! attraverso `arrow_adapter::encode_geometry`.
//!
//! L'encoder canonico (`geozero` 0.15, `to_wkb(CoordDimensions::xy())`)
//! scrive sempre l'anello esterno, anche vuoto, e il decoder del workspace
//! rifiuta un anello con meno di quattro coordinate: il poligono vuoto non
//! vi e' rappresentabile. GEOS lo scriveva a zero anelli (`POLYGON EMPTY`),
//! forma che il decoder accetta e ricostruisce come poligono con esterno
//! vuoto. `make_valid` in `STRUCTURE` produce proprio quel poligono quando
//! un anello collassa del tutto e `keep_collapsed` e' spento.
//!
//! Su ogni altra geometria i byte coincidono con l'encoder canonico: lo
//! verifica l'oracolo differenziale nei test.

use geo::{Coord, Geometry, LineString, Point, Polygon};

use super::RustBackendError;

/// Codifica `geometry` in WKB ISO XY little-endian.
///
/// # Errors
///
/// [`RustBackendError::Internal`] per un conteggio oltre `u32`.
pub fn wkb_xy(geometry: &Geometry<f64>) -> Result<Vec<u8>, RustBackendError> {
    let mut output = Vec::new();
    write_geometry(&mut output, geometry)?;
    Ok(output)
}

fn write_header(output: &mut Vec<u8>, type_code: u32) {
    output.push(1);
    output.extend_from_slice(&type_code.to_le_bytes());
}

fn write_count(output: &mut Vec<u8>, count: usize) -> Result<(), RustBackendError> {
    let count =
        u32::try_from(count).map_err(|_| RustBackendError::Internal("conteggio WKB oltre u32"))?;
    output.extend_from_slice(&count.to_le_bytes());
    Ok(())
}

fn write_coord(output: &mut Vec<u8>, coord: Coord<f64>) {
    output.extend_from_slice(&coord.x.to_le_bytes());
    output.extend_from_slice(&coord.y.to_le_bytes());
}

fn write_points(output: &mut Vec<u8>, line: &LineString<f64>) -> Result<(), RustBackendError> {
    write_count(output, line.0.len())?;
    for coord in &line.0 {
        write_coord(output, *coord);
    }
    Ok(())
}

fn write_point(output: &mut Vec<u8>, point: &Point<f64>) {
    write_header(output, 1);
    write_coord(output, point.0);
}

fn write_line_string(output: &mut Vec<u8>, line: &LineString<f64>) -> Result<(), RustBackendError> {
    write_header(output, 2);
    write_points(output, line)
}

fn write_polygon(output: &mut Vec<u8>, polygon: &Polygon<f64>) -> Result<(), RustBackendError> {
    write_header(output, 3);
    if polygon.exterior().0.is_empty() && polygon.interiors().is_empty() {
        // `POLYGON EMPTY`: zero anelli, la sola differenza dall'encoder
        // canonico.
        return write_count(output, 0);
    }
    write_count(output, polygon.interiors().len().saturating_add(1))?;
    write_points(output, polygon.exterior())?;
    for ring in polygon.interiors() {
        write_points(output, ring)?;
    }
    Ok(())
}

fn write_geometry(output: &mut Vec<u8>, geometry: &Geometry<f64>) -> Result<(), RustBackendError> {
    match geometry {
        Geometry::Point(point) => write_point(output, point),
        Geometry::LineString(line) => write_line_string(output, line)?,
        Geometry::Polygon(polygon) => write_polygon(output, polygon)?,
        Geometry::MultiPoint(points) => {
            write_header(output, 4);
            write_count(output, points.0.len())?;
            for point in &points.0 {
                write_point(output, point);
            }
        }
        Geometry::MultiLineString(lines) => {
            write_header(output, 5);
            write_count(output, lines.0.len())?;
            for line in &lines.0 {
                write_line_string(output, line)?;
            }
        }
        Geometry::MultiPolygon(polygons) => {
            write_header(output, 6);
            write_count(output, polygons.0.len())?;
            for polygon in &polygons.0 {
                write_polygon(output, polygon)?;
            }
        }
        Geometry::GeometryCollection(collection) => {
            write_header(output, 7);
            write_count(output, collection.0.len())?;
            for child in &collection.0 {
                write_geometry(output, child)?;
            }
        }
        // Come geozero: `Line` e' una `LineString` di due coordinate, `Rect`
        // e `Triangle` sono il poligono di `to_polygon`, stesso ordine dei
        // vertici.
        Geometry::Line(line) => {
            write_line_string(output, &LineString::new(vec![line.start, line.end]))?;
        }
        Geometry::Rect(rect) => write_polygon(output, &rect.to_polygon())?,
        Geometry::Triangle(triangle) => write_polygon(output, &triangle.to_polygon())?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{
        line_string, polygon, GeometryCollection, MultiLineString, MultiPoint, MultiPolygon,
    };
    use geozero::{CoordDimensions, ToWkb};

    /// Oracolo: su ogni geometria senza poligoni vuoti i byte sono quelli
    /// dell'encoder canonico.
    #[test]
    fn coincide_con_l_encoder_canonico_senza_poligoni_vuoti() {
        let square = polygon![
            (x: 0.0, y: 0.0), (x: 4.0, y: 0.0), (x: 4.0, y: 4.0), (x: 0.0, y: 4.0), (x: 0.0, y: 0.0)
        ];
        let holed = Polygon::new(
            square.exterior().clone(),
            vec![line_string![
                (x: 1.0, y: 1.0), (x: 2.0, y: 1.0), (x: 2.0, y: 2.0), (x: 1.0, y: 1.0)
            ]],
        );
        let line = line_string![(x: -0.0, y: 1.5), (x: 3.25, y: -7.0)];
        let cases = vec![
            Geometry::Point(Point::new(-1.5, 2.0e-300)),
            Geometry::LineString(line.clone()),
            Geometry::LineString(LineString::new(Vec::new())),
            Geometry::Polygon(square.clone()),
            Geometry::Polygon(holed.clone()),
            Geometry::MultiPoint(MultiPoint::new(vec![
                Point::new(0.0, 0.0),
                Point::new(1.0, -1.0),
            ])),
            Geometry::MultiPoint(MultiPoint::new(Vec::new())),
            Geometry::MultiLineString(MultiLineString::new(vec![line.clone(), line.clone()])),
            Geometry::MultiPolygon(MultiPolygon::new(vec![square.clone(), holed])),
            Geometry::MultiPolygon(MultiPolygon::new(Vec::new())),
            Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                Geometry::Polygon(square),
                Geometry::LineString(line),
                Geometry::Point(Point::new(9.0, 9.0)),
                Geometry::GeometryCollection(GeometryCollection::new_from(Vec::new())),
            ])),
        ];
        for geometry in cases {
            assert_eq!(
                wkb_xy(&geometry).unwrap(),
                geometry.to_wkb(CoordDimensions::xy()).unwrap(),
                "{geometry:?}"
            );
        }
    }

    /// Il poligono vuoto esce a zero anelli e il decoder del workspace lo
    /// ricostruisce identico, anche dentro una collezione.
    #[test]
    fn il_poligono_vuoto_ha_zero_anelli_e_torna_identico() {
        let empty = Polygon::new(LineString::new(Vec::new()), Vec::new());
        assert_eq!(
            wkb_xy(&Geometry::Polygon(empty.clone())).unwrap(),
            vec![1, 3, 0, 0, 0, 0, 0, 0, 0]
        );
        for geometry in [
            Geometry::Polygon(empty.clone()),
            Geometry::MultiPolygon(MultiPolygon::new(vec![empty.clone()])),
            Geometry::GeometryCollection(GeometryCollection::new_from(vec![
                Geometry::Polygon(empty),
                Geometry::Point(Point::new(1.0, 2.0)),
            ])),
        ] {
            let bytes = wkb_xy(&geometry).unwrap();
            assert_eq!(
                crate::wkb_decoder::decode_validated(&bytes).unwrap(),
                geometry
            );
        }
    }

    /// Oracolo anche su `Line`, `Rect` e `Triangle`, da soli e annidati.
    #[test]
    fn line_rect_e_triangle_come_l_encoder_canonico() {
        let line = Geometry::Line(geo::Line::new(
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 1.0, y: -1.5 },
        ));
        let rect = Geometry::Rect(geo::Rect::new((3.0, 1.0), (-2.0, 4.5)));
        let triangle = Geometry::Triangle(geo::Triangle::new(
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 0.0, y: 2.0 },
            Coord { x: 5.0, y: 1.0 },
        ));
        let collection = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
            line.clone(),
            rect.clone(),
            triangle.clone(),
        ]));
        for geometry in [line, rect, triangle, collection] {
            assert_eq!(
                wkb_xy(&geometry).unwrap(),
                geometry.to_wkb(CoordDimensions::xy()).unwrap(),
                "{geometry:?}"
            );
        }
    }
}
