//! plenora-kernels-geo — kernel geografici su `geo::Geometry<f64>` e adapter
//! Arrow per il canone GeoArrow-WKB (Architetture.md par. 3.3 e par. 2).
//!
//! Port Fase 1 ("coesistenza") da plenora-geo-tools-arrow: validatore WKB
//! strutturale, kernel puri (`operations`, `analysis`, `topology`,
//! `predicates`, `construction`, `extended`, `extended_algorithms`,
//! `advanced`, `spatial_join`, `extensions`, `extensions2`, `extensions3`,
//! `cluster`),
//! backend opzionali (`geos_backend`,
//! `proj_backend`) e l'adapter Arrow di rappresentazione (`arrow_adapter`).
//!
//! L'adapter è progettato per ammettere la cache di decode per segmento
//! (Prestazioni.md, vincoli V6/G1/G2) senza modifiche ai contratti.
//!
//! - [`arrow_adapter`](crate::arrow_adapter) per la rappresentazione
//!   GeoArrow-WKB e [`analyze`] per l'inferenza a secco dei contratti
//!   (`analyze_contract` del catalogo, Fase 2A-2b).
//!
//! Errori: il sorgente usava `GeoEngineError`; qui le stesse condizioni sono
//! mappate su [`plenora_core::PlenoraError`] preservando i messaggi:
//! - `InvalidWkb` / `EmptyGeometry` / `WkbSerialization` / `NonFiniteCoordinate`
//!   / `InvalidWkbStructure` / `InvalidGeometry` → `PlenoraError::Contract`;
//! - `UnsupportedWkbDimension` → `PlenoraError::Unsupported`.
//!
//! Feature: `geos-backend`, `proj-backend`, `full-backends`.

pub mod advanced;
pub mod analysis;
pub mod analyze;
pub mod arrow_adapter;
pub mod cluster;
pub mod construction;
pub mod crs;
pub mod extended;
pub mod extended_algorithms;
pub mod extensions;
pub mod extensions2;
pub mod extensions3;
#[cfg(feature = "geos-backend")]
pub mod geos_backend;
pub mod operations;
pub mod predicates;
#[cfg(feature = "proj-backend")]
pub mod proj_backend;
pub mod spatial_join;
pub mod topology;

use geo::algorithm::validation::Validation;
use geo::{
    BoundingRect, Centroid, ConvexHull, Coord, CoordsIter, Geometry, LineString, MapCoords, Point,
};
use geozero::{wkb::Wkb, CoordDimensions, ToGeo, ToWkb};
use plenora_core::PlenoraError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Centroid,
    ConvexHull,
    Envelope,
}

impl Operation {
    pub const ALL: [Operation; 3] = [
        Operation::Centroid,
        Operation::ConvexHull,
        Operation::Envelope,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Operation::Centroid => "centroid",
            Operation::ConvexHull => "convex_hull",
            Operation::Envelope => "envelope",
        }
    }
}

/// Mappatura delle varianti `GeoEngineError` del sorgente su `PlenoraError`
/// (messaggi invariati).
fn invalid_wkb(error: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::Contract(format!("WKB non valido: {error}"))
}

fn empty_geometry(operation: &'static str) -> PlenoraError {
    PlenoraError::Contract(format!(
        "geometria vuota non supportata da {operation}"
    ))
}

fn wkb_serialization(error: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::Contract(format!("serializzazione WKB fallita: {error}"))
}

fn unsupported_wkb_dimension() -> PlenoraError {
    PlenoraError::Unsupported(
        "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D".to_owned(),
    )
}

fn non_finite_coordinate() -> PlenoraError {
    PlenoraError::Contract("WKB contiene coordinate NaN o infinite".to_owned())
}

fn invalid_wkb_structure(reason: &'static str) -> PlenoraError {
    PlenoraError::Contract(format!("struttura WKB non valida: {reason}"))
}

fn invalid_geometry(error: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::Contract(format!("geometria OGC non valida: {error}"))
}

pub const MAX_WKB_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_WKB_COMPONENTS: u64 = 100_000;
/// Profondita' massima di annidamento (multi-geometrie) di default.
pub const MAX_WKB_DEPTH: usize = 64;

pub fn geometry_from_wkb(payload: &[u8]) -> Result<Geometry<f64>, PlenoraError> {
    validate_wkb_contract(payload)?;
    let geometry = Wkb(payload)
        .to_geo()
        .map_err(|error| invalid_wkb(error.to_string()))?;
    geometry
        .check_validation()
        .map_err(|error| invalid_geometry(error.to_string()))?;
    Ok(geometry)
}

struct WkbCursor<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> WkbCursor<'a> {
    fn new(payload: &'a [u8]) -> Self {
        Self { payload, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.payload.len().saturating_sub(self.offset)
    }

    fn read_u8(&mut self) -> Result<u8, PlenoraError> {
        let value = *self
            .payload
            .get(self.offset)
            .ok_or_else(|| invalid_wkb_structure("byte mancante"))?;
        self.offset += 1;
        Ok(value)
    }

    fn read_u32(&mut self, little_endian: bool) -> Result<u32, PlenoraError> {
        let bytes: [u8; 4] = self
            .payload
            .get(self.offset..self.offset + 4)
            .ok_or_else(|| invalid_wkb_structure("uint32 troncato"))?
            .try_into()
            .expect("4 bytes");
        self.offset += 4;
        Ok(if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn read_f64(&mut self, little_endian: bool) -> Result<f64, PlenoraError> {
        let bytes: [u8; 8] = self
            .payload
            .get(self.offset..self.offset + 8)
            .ok_or_else(|| invalid_wkb_structure("float64 troncato"))?
            .try_into()
            .expect("8 bytes");
        self.offset += 8;
        Ok(if little_endian {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        })
    }

    fn read_xy(&mut self, little_endian: bool) -> Result<(f64, f64), PlenoraError> {
        let x = self.read_f64(little_endian)?;
        let y = self.read_f64(little_endian)?;
        if !x.is_finite() || !y.is_finite() {
            return Err(non_finite_coordinate());
        }
        Ok((x, y))
    }
}

fn checked_count(
    value: u32,
    remaining: usize,
    minimum_item_bytes: usize,
) -> Result<usize, PlenoraError> {
    let count = value as usize;
    if count > remaining / minimum_item_bytes.max(1) {
        return Err(invalid_wkb_structure(
            "conteggio elementi oltre i byte disponibili",
        ));
    }
    Ok(count)
}

fn validate_wkb_geometry(
    cursor: &mut WkbCursor<'_>,
    depth: usize,
    max_depth: usize,
    components: &mut u64,
) -> Result<u32, PlenoraError> {
    if depth > max_depth {
        return Err(invalid_wkb_structure(
            "annidamento geometrie oltre il limite",
        ));
    }
    *components = components
        .checked_add(1)
        .ok_or_else(|| invalid_wkb_structure("conteggio componenti oltre il limite"))?;
    if *components > MAX_WKB_COMPONENTS {
        return Err(invalid_wkb_structure(
            "conteggio componenti oltre il limite",
        ));
    }
    let byte_order = cursor.read_u8()?;
    let little_endian = match byte_order {
        0 => false,
        1 => true,
        _ => return Err(invalid_wkb_structure("byte order non valido")),
    };
    let raw_type = cursor.read_u32(little_endian)?;
    if raw_type & 0xE000_0000 != 0 {
        return Err(unsupported_wkb_dimension());
    }
    let dimension_code = raw_type / 1000;
    if dimension_code != 0 {
        return Err(unsupported_wkb_dimension());
    }
    let geometry_type = raw_type % 1000;
    match geometry_type {
        1 => {
            cursor.read_xy(little_endian)?;
        }
        2 => {
            let count = cursor.read_u32(little_endian)?;
            let count = checked_count(count, cursor.remaining(), 16)?;
            if count == 1 {
                return Err(invalid_wkb_structure(
                    "LineString deve essere vuota o avere almeno due coordinate",
                ));
            }
            for _ in 0..count {
                cursor.read_xy(little_endian)?;
            }
        }
        3 => {
            let rings = cursor.read_u32(little_endian)?;
            let rings = checked_count(rings, cursor.remaining(), 4)?;
            for _ in 0..rings {
                let count = cursor.read_u32(little_endian)?;
                let count = checked_count(count, cursor.remaining(), 16)?;
                if count < 4 {
                    return Err(invalid_wkb_structure(
                        "anello poligonale con meno di quattro coordinate",
                    ));
                }
                let first = cursor.read_xy(little_endian)?;
                let mut last = first;
                for _ in 1..count {
                    last = cursor.read_xy(little_endian)?;
                }
                if first != last {
                    return Err(invalid_wkb_structure("anello poligonale non chiuso"));
                }
            }
        }
        4..=7 => {
            let children = cursor.read_u32(little_endian)?;
            let children = checked_count(children, cursor.remaining(), 5)?;
            for _ in 0..children {
                let child_type = validate_wkb_geometry(cursor, depth + 1, max_depth, components)?;
                let valid_child = match geometry_type {
                    4 => child_type == 1,
                    5 => child_type == 2,
                    6 => child_type == 3,
                    7 => true,
                    _ => unreachable!(),
                };
                if !valid_child {
                    return Err(invalid_wkb_structure(
                        "tipo figlio incompatibile con multi-geometria",
                    ));
                }
            }
        }
        _ => {
            return Err(invalid_wkb_structure("tipo geometria non supportato"));
        }
    }
    Ok(geometry_type)
}

pub fn validate_wkb_contract(payload: &[u8]) -> Result<(), PlenoraError> {
    validate_wkb_contract_with_depth(payload, MAX_WKB_DEPTH)
}

/// Variante con profondita' di annidamento configurabile (il limite arriva
/// dai `Limits` effettivi del piano, `max_geometry_depth`; il default di
/// [`validate_wkb_contract`] resta [`MAX_WKB_DEPTH`]).
pub fn validate_wkb_contract_with_depth(payload: &[u8], max_depth: usize) -> Result<(), PlenoraError> {
    if payload.len() > MAX_WKB_BYTES {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    let mut cursor = WkbCursor::new(payload);
    let mut components = 0_u64;
    validate_wkb_geometry(&mut cursor, 0, max_depth, &mut components)?;
    if cursor.remaining() != 0 {
        return Err(invalid_wkb_structure("byte residui dopo la geometria"));
    }
    Ok(())
}

fn envelope(geometry: &Geometry<f64>) -> Result<Geometry<f64>, PlenoraError> {
    let rect = geometry
        .bounding_rect()
        .ok_or_else(|| empty_geometry("envelope"))?;
    let min = rect.min();
    let max = rect.max();

    if min.x == max.x && min.y == max.y {
        return Ok(Geometry::Point(Point::new(min.x, min.y)));
    }
    if min.x == max.x || min.y == max.y {
        return Ok(Geometry::LineString(LineString::from(vec![min, max])));
    }
    Ok(Geometry::Polygon(rect.to_polygon()))
}

fn robust_convex_hull(geometry: &Geometry<f64>) -> Geometry<f64> {
    // `geo`'s orientation math can overflow for otherwise finite coordinates
    // near f64 limits. Uniform normalization preserves hull topology while
    // keeping every intermediate determinant in a safe numeric range.
    let scale = geometry.coords_iter().fold(0.0_f64, |maximum, coordinate| {
        maximum.max(coordinate.x.abs()).max(coordinate.y.abs())
    });
    if scale == 0.0 {
        return Geometry::Polygon(geometry.convex_hull());
    }
    let normalized = geometry.map_coords(|coordinate| Coord {
        x: coordinate.x / scale,
        y: coordinate.y / scale,
    });
    Geometry::Polygon(normalized.convex_hull().map_coords(|coordinate| Coord {
        x: coordinate.x * scale,
        y: coordinate.y * scale,
    }))
}

pub fn transform_geometry(
    operation: Operation,
    geometry: &Geometry<f64>,
) -> Result<Geometry<f64>, PlenoraError> {
    geometry
        .check_validation()
        .map_err(|error| invalid_geometry(error.to_string()))?;
    transform_geometry_validated(operation, geometry)
}

fn transform_geometry_validated(
    operation: Operation,
    geometry: &Geometry<f64>,
) -> Result<Geometry<f64>, PlenoraError> {
    let output = match operation {
        Operation::Centroid => geometry
            .centroid()
            .map(Geometry::Point)
            .ok_or_else(|| empty_geometry("centroid")),
        Operation::ConvexHull => Ok(robust_convex_hull(geometry)),
        Operation::Envelope => envelope(geometry),
    }?;
    output
        .check_validation()
        .map_err(|error| invalid_geometry(error.to_string()))?;
    Ok(output)
}

pub fn transform_wkb(operation: Operation, payload: &[u8]) -> Result<Vec<u8>, PlenoraError> {
    let geometry = geometry_from_wkb(payload)?;
    let transformed = transform_geometry_validated(operation, &geometry)?;
    let output = transformed
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| wkb_serialization(error.to_string()))?;
    validate_wkb_contract(&output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, Area};
    use proptest::prelude::*;

    fn round_trip(operation: Operation, geometry: Geometry<f64>) -> Geometry<f64> {
        let payload = geometry
            .to_wkb(CoordDimensions::xy())
            .expect("encode fixture");
        geometry_from_wkb(&transform_wkb(operation, &payload).expect("transform"))
            .expect("decode result")
    }

    /// Equivalente dei match sulle varianti `GeoEngineError` del sorgente:
    /// la condizione e' identificata dal messaggio, preservato verbatim.
    fn is_contract_error(result: &Result<Geometry<f64>, PlenoraError>, message: &str) -> bool {
        matches!(result, Err(PlenoraError::Contract(reason)) if reason == message)
    }

    #[test]
    fn centroid_transforms_polygon_to_expected_point() {
        let input = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 4.0, y: 0.0),
            (x: 4.0, y: 2.0),
            (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ]);
        let result = round_trip(Operation::Centroid, input);
        assert_eq!(result, Geometry::Point(Point::new(2.0, 1.0)));
    }

    #[test]
    fn envelope_preserves_degenerate_dimension() {
        let point = Geometry::Point(Point::new(2.0, 3.0));
        assert_eq!(round_trip(Operation::Envelope, point.clone()), point);

        let line = Geometry::LineString(line_string![(x: 1.0, y: 4.0), (x: 5.0, y: 4.0)]);
        assert_eq!(round_trip(Operation::Envelope, line.clone()), line);
    }

    #[test]
    fn convex_hull_contains_input_area() {
        let input = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0),
            (x: 4.0, y: 0.0),
            (x: 2.0, y: 3.0),
            (x: 2.0, y: 1.0),
        ]);
        let result = round_trip(Operation::ConvexHull, input);
        assert!(result.unsigned_area() > 0.0);
    }

    #[test]
    fn convex_hull_normalizes_extreme_finite_coordinates_without_panicking() {
        let input = Geometry::LineString(LineString::from(vec![
            (-3.477300121932381e-164, 2.781342323781663e-309),
            (1.344974619049452e-284, 6.3542808404505305e-183),
            (2.639614224254873e-309, 3.236069361538085e-111),
            (-5.48880284031224e303, -6.971241357778827e182),
            (-5.486124068793689e303, 7.064166183585296e-304),
        ]));
        let hull = transform_geometry(Operation::ConvexHull, &input).unwrap();
        assert!(hull.check_validation().is_ok());
        assert!(hull
            .coords_iter()
            .all(|coordinate| coordinate.x.is_finite() && coordinate.y.is_finite()));
        let payload = input.to_wkb(CoordDimensions::xy()).unwrap();
        assert!(transform_wkb(Operation::ConvexHull, &payload).is_ok());
    }

    #[test]
    fn rejects_non_finite_and_dimensional_wkb() {
        let mut nan_point = vec![1_u8, 1, 0, 0, 0];
        nan_point.extend_from_slice(&f64::NAN.to_le_bytes());
        nan_point.extend_from_slice(&1.0_f64.to_le_bytes());
        assert!(is_contract_error(
            &geometry_from_wkb(&nan_point),
            "WKB contiene coordinate NaN o infinite"
        ));

        let mut z_point = vec![1_u8];
        z_point.extend_from_slice(&1001_u32.to_le_bytes());
        z_point.extend_from_slice(&1.0_f64.to_le_bytes());
        z_point.extend_from_slice(&2.0_f64.to_le_bytes());
        z_point.extend_from_slice(&3.0_f64.to_le_bytes());
        assert!(matches!(
            geometry_from_wkb(&z_point),
            Err(PlenoraError::Unsupported(message))
            if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
        ));
    }

    #[test]
    fn rejects_dimensional_wkb_hidden_in_collection() {
        let mut collection = vec![1_u8];
        collection.extend_from_slice(&7_u32.to_le_bytes());
        collection.extend_from_slice(&1_u32.to_le_bytes());
        collection.push(1_u8);
        collection.extend_from_slice(&1001_u32.to_le_bytes());
        collection.extend_from_slice(&1.0_f64.to_le_bytes());
        collection.extend_from_slice(&2.0_f64.to_le_bytes());
        collection.extend_from_slice(&3.0_f64.to_le_bytes());
        assert!(matches!(
            geometry_from_wkb(&collection),
            Err(PlenoraError::Unsupported(message))
            if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
        ));
    }

    #[test]
    fn rejects_unclosed_or_too_short_polygon_rings() {
        let mut polygon = vec![1_u8];
        polygon.extend_from_slice(&3_u32.to_le_bytes());
        polygon.extend_from_slice(&1_u32.to_le_bytes());
        polygon.extend_from_slice(&4_u32.to_le_bytes());
        for (x, y) in [(0.0_f64, 0.0_f64), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            polygon.extend_from_slice(&x.to_le_bytes());
            polygon.extend_from_slice(&y.to_le_bytes());
        }
        assert!(is_contract_error(
            &geometry_from_wkb(&polygon),
            "struttura WKB non valida: anello poligonale non chiuso"
        ));

        let mut short = vec![1_u8];
        short.extend_from_slice(&3_u32.to_le_bytes());
        short.extend_from_slice(&1_u32.to_le_bytes());
        short.extend_from_slice(&3_u32.to_le_bytes());
        for _ in 0..3 {
            short.extend_from_slice(&0.0_f64.to_le_bytes());
            short.extend_from_slice(&0.0_f64.to_le_bytes());
        }
        assert!(is_contract_error(
            &geometry_from_wkb(&short),
            "struttura WKB non valida: anello poligonale con meno di quattro coordinate"
        ));
    }

    #[test]
    fn wkb_validator_covers_endianness_truncation_counts_and_trailing_bytes() {
        let mut big_endian_point = vec![0_u8];
        big_endian_point.extend_from_slice(&1_u32.to_be_bytes());
        big_endian_point.extend_from_slice(&2.0_f64.to_be_bytes());
        big_endian_point.extend_from_slice(&3.0_f64.to_be_bytes());
        assert_eq!(
            geometry_from_wkb(&big_endian_point).unwrap(),
            Geometry::Point(Point::new(2.0, 3.0))
        );

        for malformed in [
            vec![],
            vec![2],
            vec![1, 1, 0],
            vec![1, 1, 0, 0, 0, 0],
            vec![1, 2, 0, 0, 0, 1, 0, 0, 0],
            vec![1, 2, 0, 0, 0, 2, 0, 0, 0],
        ] {
            assert!(geometry_from_wkb(&malformed).is_err(), "{malformed:?}");
        }

        let mut trailing = big_endian_point.clone();
        trailing.push(0xff);
        assert!(is_contract_error(
            &geometry_from_wkb(&trailing),
            "struttura WKB non valida: byte residui dopo la geometria"
        ));

        let mut unsupported = vec![1_u8];
        unsupported.extend_from_slice(&99_u32.to_le_bytes());
        assert!(is_contract_error(
            &geometry_from_wkb(&unsupported),
            "struttura WKB non valida: tipo geometria non supportato"
        ));
    }

    #[test]
    fn wkb_validator_accepts_valid_multi_types_and_rejects_wrong_children() {
        for geometry in [
            Geometry::MultiPoint(geo::MultiPoint::new(vec![Point::new(1.0, 2.0)])),
            Geometry::MultiLineString(geo::MultiLineString::new(vec![line_string![
                (x: 0.0, y: 0.0), (x: 1.0, y: 1.0)
            ]])),
            Geometry::MultiPolygon(geo::MultiPolygon::new(vec![polygon![
                (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 0.0, y: 1.0), (x: 0.0, y: 0.0)
            ]])),
            Geometry::GeometryCollection(geo::GeometryCollection(vec![Geometry::Point(
                Point::new(1.0, 2.0),
            )])),
        ] {
            let payload = geometry.to_wkb(CoordDimensions::xy()).unwrap();
            assert_eq!(geometry_from_wkb(&payload).unwrap(), geometry);
        }

        for parent_type in [4_u32, 5, 6] {
            let mut payload = vec![1_u8];
            payload.extend_from_slice(&parent_type.to_le_bytes());
            payload.extend_from_slice(&1_u32.to_le_bytes());
            let wrong_child = if parent_type == 4 {
                Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)])
            } else {
                Geometry::Point(Point::new(0.0, 0.0))
            };
            payload.extend_from_slice(&wrong_child.to_wkb(CoordDimensions::xy()).unwrap());
            assert!(is_contract_error(
                &geometry_from_wkb(&payload),
                "struttura WKB non valida: tipo figlio incompatibile con multi-geometria"
            ));
        }
    }

    #[test]
    fn validate_wkb_contract_with_depth_enforces_the_configurable_limit() {
        // GC(GC(Point)): il punto e' a profondita' 2.
        let nested = Geometry::GeometryCollection(geo::GeometryCollection(vec![
            Geometry::GeometryCollection(geo::GeometryCollection(vec![Geometry::Point(
                Point::new(1.0, 2.0),
            )])),
        ]));
        let payload = nested.to_wkb(CoordDimensions::xy()).unwrap();
        assert!(validate_wkb_contract_with_depth(&payload, MAX_WKB_DEPTH).is_ok());
        assert!(validate_wkb_contract_with_depth(&payload, 2).is_ok());
        assert!(matches!(
            validate_wkb_contract_with_depth(&payload, 1),
            Err(PlenoraError::Contract(reason))
                if reason == "struttura WKB non valida: annidamento geometrie oltre il limite"
        ));
        // Il default resta 64 (comportamento invariato di validate_wkb_contract).
        assert!(validate_wkb_contract(&payload).is_ok());
    }

    proptest! {
        #[test]
        fn arbitrary_wkb_bytes_never_panic_and_successes_remain_roundtrippable(
            payload in proptest::collection::vec(any::<u8>(), 0..4096)
        ) {
            if let Ok(geometry) = geometry_from_wkb(&payload) {
                prop_assert!(geometry.check_validation().is_ok());
                for operation in Operation::ALL {
                    if let Ok(output) = transform_wkb(operation, &payload) {
                        prop_assert!(geometry_from_wkb(&output).is_ok());
                    }
                }
            }
        }

        #[test]
        fn single_byte_mutations_of_valid_wkb_never_escape_the_contract(
            index in any::<usize>(), replacement in any::<u8>()
        ) {
            let mut payload = Geometry::Polygon(polygon![
                (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
                (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
                (x: 0.0, y: 0.0),
            ]).to_wkb(CoordDimensions::xy()).unwrap();
            let position = index % payload.len();
            payload[position] = replacement;
            if let Ok(geometry) = geometry_from_wkb(&payload) {
                prop_assert!(geometry.check_validation().is_ok());
                let encoded = geometry.to_wkb(CoordDimensions::xy()).unwrap();
                prop_assert!(validate_wkb_contract(&encoded).is_ok());
            }
        }
    }
}
