//! Geometry construction kernels. Grouping and ordering columns are handled
//! by the future Arrow adapter; these functions operate on one ordered group.

use geo::algorithm::validation::Validation;
use geo::{Geometry, LineString, Point, Polygon};
use thiserror::Error;
use wkt::TryFromWkt;

#[derive(Debug, Error)]
pub enum ConstructionError {
    #[error("coordinata {name} non finita")]
    NonFiniteCoordinate { name: &'static str },
    #[error("atteso Point alla posizione {index}, ricevuto {geometry_type}")]
    ExpectedPoint {
        index: usize,
        geometry_type: &'static str,
    },
    #[error("geometria costruita non valida: {0}")]
    InvalidOutput(String),
    #[error("WKT non valido: {0}")]
    InvalidWkt(String),
    #[error("WKT con SRID o dimensioni Z/M non supportato dal contratto XY")]
    UnsupportedWktDimension,
}

const fn geometry_name(geometry: &Geometry<f64>) -> &'static str {
    match geometry {
        Geometry::Point(_) => "Point",
        Geometry::Line(_) => "Line",
        Geometry::LineString(_) => "LineString",
        Geometry::Polygon(_) => "Polygon",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
        Geometry::Rect(_) => "Rect",
        Geometry::Triangle(_) => "Triangle",
    }
}

/// Costruisce un `Point` da longitudine e latitudine.
///
/// # Errors
///
/// `ConstructionError::NonFiniteCoordinate` se `lon` o `lat` e' NaN o
/// infinita.
pub fn point_from_lon_lat(lon: f64, lat: f64) -> Result<Geometry<f64>, ConstructionError> {
    if !lon.is_finite() {
        return Err(ConstructionError::NonFiniteCoordinate { name: "lon" });
    }
    if !lat.is_finite() {
        return Err(ConstructionError::NonFiniteCoordinate { name: "lat" });
    }
    Ok(Geometry::Point(Point::new(lon, lat)))
}

/// Decodifica una geometria dal testo WKT (solo XY, senza SRID).
///
/// # Errors
///
/// - `ConstructionError::InvalidWkt`: testo oltre il limite di 64 MiB o
///   parsing WKT fallito;
/// - `ConstructionError::UnsupportedWktDimension`: il testo dichiara uno
///   SRID (`SRID=...`) o dimensioni Z/M/ZM, non supportate dal contratto
///   XY;
/// - `ConstructionError::InvalidOutput`: la geometria decodificata non
///   supera la validazione OGC.
pub fn geometry_from_wkt(value: &str) -> Result<Geometry<f64>, ConstructionError> {
    const MAX_WKT_BYTES: usize = 64 * 1024 * 1024;
    if value.len() > MAX_WKT_BYTES {
        return Err(ConstructionError::InvalidWkt(
            "testo oltre il limite di 64 MiB".to_owned(),
        ));
    }
    // Riconoscimento del tipo senza allocare: si ispeziona solo la porzione
    // di testo prima del primo `(`, dove vivono il type name OGC (tutti
    // entro una ventina di caratteri ASCII: POINT, LINESTRING, POLYGON,
    // MULTIPOINT, ...) e l'eventuale suffisso dimensionale. Il confronto
    // ASCII case-insensitive e' equivalente per costruzione alla precedente
    // copia `to_ascii_uppercase` dell'intera cella: i token cercati
    // ("SRID=", "Z", "M", "ZM") sono puramente ASCII e
    // `to_ascii_uppercase` non altera ne' i byte non ASCII ne' lo
    // whitespace, quindi tokenizzazione ed esito sono identici su ogni
    // input, inclusi prefissi malformati o arbitrariamente lunghi.
    let head = value.trim_start();
    let prefix_end = head.find('(').unwrap_or(head.len());
    let prefix = &head[..prefix_end];
    let srid = head
        .get(..5)
        .is_some_and(|start| start.eq_ignore_ascii_case("SRID="));
    let dimensional = prefix.split_whitespace().skip(1).any(|token| {
        ["Z", "M", "ZM"]
            .iter()
            .any(|suffix| token.eq_ignore_ascii_case(suffix))
    });
    if srid || dimensional {
        return Err(ConstructionError::UnsupportedWktDimension);
    }
    let geometry = Geometry::<f64>::try_from_wkt_str(value)
        .map_err(|error| ConstructionError::InvalidWkt(error.to_string()))?;
    geometry
        .check_validation()
        .map_err(|error| ConstructionError::InvalidOutput(error.to_string()))?;
    Ok(geometry)
}

fn collect_points(
    geometries: &[Option<Geometry<f64>>],
) -> Result<Vec<Point<f64>>, ConstructionError> {
    geometries
        .iter()
        .enumerate()
        .filter_map(|(index, geometry)| geometry.as_ref().map(|geometry| (index, geometry)))
        .map(|(index, geometry)| match geometry {
            Geometry::Point(point) => Ok(*point),
            value => Err(ConstructionError::ExpectedPoint {
                index,
                geometry_type: geometry_name(value),
            }),
        })
        .collect()
}

/// Costruisce una `LineString` dai punti del gruppo ordinato.
///
/// Le righe `None` sono ignorate; con meno di due punti utili il risultato
/// e' `None` (gruppo omesso, non un errore).
///
/// # Errors
///
/// - `ConstructionError::ExpectedPoint`: una geometria non nulla non e' un
///   `Point`;
/// - `ConstructionError::InvalidOutput`: la linea costruita non supera la
///   validazione OGC.
pub fn line_from_ordered_points(
    geometries: &[Option<Geometry<f64>>],
) -> Result<Option<Geometry<f64>>, ConstructionError> {
    let points = collect_points(geometries)?;
    if points.len() < 2 {
        return Ok(None);
    }
    let line = Geometry::LineString(LineString::new(
        points.into_iter().map(|point| point.0).collect(),
    ));
    line.check_validation()
        .map_err(|error| ConstructionError::InvalidOutput(error.to_string()))?;
    Ok(Some(line))
}

/// Costruisce un `Polygon` (anello esterno senza buchi) dai punti del
/// gruppo ordinato.
///
/// Le righe `None` sono ignorate; con meno di tre punti utili il risultato
/// e' `None` (gruppo omesso, non un errore).
///
/// # Errors
///
/// - `ConstructionError::ExpectedPoint`: una geometria non nulla non e' un
///   `Point`;
/// - `ConstructionError::InvalidOutput`: il poligono costruito non supera
///   la validazione OGC (es. auto-intersezione).
pub fn polygon_from_ordered_points(
    geometries: &[Option<Geometry<f64>>],
) -> Result<Option<Geometry<f64>>, ConstructionError> {
    let points = collect_points(geometries)?;
    if points.len() < 3 {
        return Ok(None);
    }
    let polygon = Geometry::Polygon(Polygon::new(
        LineString::new(points.into_iter().map(|point| point.0).collect()),
        Vec::new(),
    ));
    polygon
        .check_validation()
        .map_err(|error| ConstructionError::InvalidOutput(error.to_string()))?;
    Ok(Some(polygon))
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use geo::{Area, CoordsIter};

    // unnecessary_wraps: l'Option e' il contratto dei fixture (colonne con
    // righe null), non un possibile fallimento dell'helper.
    #[allow(clippy::unnecessary_wraps)]
    fn point(x: f64, y: f64) -> Option<Geometry<f64>> {
        Some(Geometry::Point(Point::new(x, y)))
    }

    #[test]
    fn creates_point_line_and_polygon_with_strict_validation() {
        assert_eq!(
            point_from_lon_lat(12.0, 41.0).unwrap(),
            point(12.0, 41.0).unwrap()
        );
        assert!(point_from_lon_lat(f64::NAN, 41.0).is_err());

        let points = vec![point(0.0, 0.0), None, point(2.0, 0.0), point(2.0, 2.0)];
        let line = line_from_ordered_points(&points).unwrap().unwrap();
        assert_eq!(line.coords_count(), 3);
        let polygon = polygon_from_ordered_points(&points).unwrap().unwrap();
        assert_eq!(polygon.coords_count(), 4);
        assert_eq!(polygon.unsigned_area(), 2.0);

        assert_eq!(
            geometry_from_wkt("POINT(12 41)").unwrap(),
            Geometry::Point(Point::new(12.0, 41.0))
        );
        assert!(matches!(
            geometry_from_wkt("POINT Z (12 41 3)"),
            Err(ConstructionError::UnsupportedWktDimension)
        ));
        assert!(geometry_from_wkt("POINT (12 41 3)").is_err());
        assert!(geometry_from_wkt("SRID=4326;POINT (12 41)").is_err());
    }

    #[test]
    fn insufficient_points_are_omitted_like_manipola_groups() {
        assert!(line_from_ordered_points(&[point(0.0, 0.0)])
            .unwrap()
            .is_none());
        assert!(
            polygon_from_ordered_points(&[point(0.0, 0.0), point(1.0, 0.0)])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_wrong_geometry_type_and_self_intersection() {
        let wrong = vec![Some(Geometry::LineString(LineString::new(vec![
            (0.0, 0.0).into(),
            (1.0, 1.0).into(),
        ])))];
        assert!(matches!(
            line_from_ordered_points(&wrong),
            Err(ConstructionError::ExpectedPoint { index: 0, .. })
        ));

        let bow_tie = vec![
            point(0.0, 0.0),
            point(2.0, 2.0),
            point(0.0, 2.0),
            point(2.0, 0.0),
        ];
        assert!(matches!(
            polygon_from_ordered_points(&bow_tie),
            Err(ConstructionError::InvalidOutput(_))
        ));
        assert!(matches!(
            point_from_lon_lat(1.0, f64::INFINITY),
            Err(ConstructionError::NonFiniteCoordinate { name: "lat" })
        ));
        let too_long = " ".repeat(64 * 1024 * 1024 + 1);
        assert!(matches!(
            geometry_from_wkt(&too_long),
            Err(ConstructionError::InvalidWkt(_))
        ));
        let variants = vec![
            Geometry::Line(geo::Line::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::Polygon(geo::Rect::new((0.0, 0.0), (1.0, 1.0)).to_polygon()),
            Geometry::MultiPoint(vec![Point::new(0.0, 0.0)].into()),
            Geometry::MultiLineString(geo::MultiLineString::new(Vec::new())),
            Geometry::MultiPolygon(geo::MultiPolygon::new(Vec::new())),
            Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into()),
            Geometry::Rect(geo::Rect::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::Triangle(geo::Triangle::new(
                geo::Coord { x: 0.0, y: 0.0 },
                geo::Coord { x: 1.0, y: 0.0 },
                geo::Coord { x: 0.0, y: 1.0 },
            )),
        ];
        for variant in variants {
            assert!(matches!(
                line_from_ordered_points(&[Some(variant)]),
                Err(ConstructionError::ExpectedPoint { .. })
            ));
        }
    }
}
