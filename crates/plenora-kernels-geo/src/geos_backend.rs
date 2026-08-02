//! GEOS-backed operations for cases where pure `GeoRust` must not approximate
//! established topology semantics.

use geo::{Area, CoordsIter, Geometry, LineString, Polygon};
use geos::{Geom, Geometry as GeosGeometry, MakeValidMethod, MakeValidParams};
use geozero::{CoordDimensions, ToWkb};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{geometry_from_wkb, validate_wkb_contract};
use plenora_core::PlenoraError;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairMethod {
    Linework,
    Structure,
}

#[derive(Debug, Error)]
pub enum GeosBackendError {
    #[error(transparent)]
    InputContract(#[from] PlenoraError),
    #[error("GEOS: {0}")]
    Geos(String),
    #[error("GEOS make-valid ha prodotto una geometria ancora non valida")]
    InvalidRepair,
    #[error("tipo geometria non supportato da {operation}: {actual}")]
    UnsupportedGeometry {
        operation: &'static str,
        actual: &'static str,
    },
    #[error("coordinate oltre il limite di {limit}: {actual}")]
    CoordinateLimit { actual: u64, limit: u64 },
    #[error("output oltre il limite di {limit}: {actual}")]
    OutputLimit { actual: u64, limit: u64 },
    #[error("lavoro di noding oltre il limite di {limit}: {actual}")]
    WorkLimit { actual: u64, limit: u64 },
    #[error(
        "polygonize incompleto: cuts={cuts}, dangles={dangles}, invalid_rings={invalid_rings}"
    )]
    IncompletePolygonize {
        cuts: usize,
        dangles: usize,
        invalid_rings: usize,
    },
    #[error("output GEOS non valido: {0}")]
    InvalidOutput(String),
    #[error("lo split poligonale non conserva l'area: input={input}, output={output}")]
    AreaMismatch { input: f64, output: f64 },
    #[error("lo split poligonale non ricopre esattamente l'input: symmetric_difference={area}")]
    CoverageMismatch { area: f64 },
}

// Passato per valore per contratto di `map_err` (usato come fn-pointer sui
// `Result` di GEOS): la conversione in stringa non richiede il consumo, ma il
// puntatore a funzione si.
#[allow(clippy::needless_pass_by_value)]
fn geos_error(error: geos::Error) -> GeosBackendError {
    GeosBackendError::Geos(error.to_string())
}

/// Repairs a structurally valid 2D WKB geometry.
///
/// Unlike the normal decoder, the input is allowed to be OGC-invalid because
/// that is precisely what this operation repairs. Dimensions, non-finite
/// coordinates and malformed WKB remain rejected before entering GEOS.
///
/// # Errors
///
/// Restituisce `GeosBackendError` se il payload viola il contratto
/// strutturale WKB, se GEOS fallisce la lettura o la riparazione, se
/// l'output riparato e' ancora non valido o se la rivalidazione
/// dell'output fallisce.
pub fn make_valid_wkb(
    payload: &[u8],
    method: RepairMethod,
    keep_collapsed: bool,
) -> Result<Vec<u8>, GeosBackendError> {
    validate_wkb_contract(payload)?;
    let geometry = GeosGeometry::new_from_wkb(payload).map_err(geos_error)?;
    if geometry.is_valid().map_err(geos_error)? {
        return Ok(payload.to_vec());
    }
    let method = match method {
        RepairMethod::Linework => MakeValidMethod::Linework,
        RepairMethod::Structure => MakeValidMethod::Structure,
    };
    let params = MakeValidParams::builder()
        .method(method)
        .keep_collapsed(keep_collapsed)
        .build()
        .map_err(geos_error)?;
    let repaired = geometry
        .make_valid_with_params(&params)
        .map_err(geos_error)?;
    if !repaired.is_valid().map_err(geos_error)? {
        return Err(GeosBackendError::InvalidRepair);
    }
    let output = repaired.to_wkb().map_err(geos_error)?;
    // Rivalidazione dell'output (R0.1: nessuna fiducia nel produttore):
    // il decoder validante (ADR-0011) applica lo stesso contratto
    // strutturale DURANTE la costruzione — una passata, garanzia identica.
    geometry_from_wkb(&output)?;
    Ok(output)
}

/// Variante di [`make_valid_wkb`] su geometria gia' decodificata.
///
/// Fusione dei segmenti geo (ADR-0012 M3): l'input resta ammesso OGC-invalido
/// — e' esattamente cio' che l'operazione ripara. Il WKB intermedio e' la
/// stessa forma canonica XY che il percorso non fuso consegnerebbe al nodo,
/// quindi gate strutturale, riparazione GEOS e rivalidazione dell'output
/// sono letteralmente quelli di [`make_valid_wkb`]: stesso risultato e
/// stessi errori nei due percorsi.
///
/// # Errors
///
/// Come [`make_valid_wkb`]; in piu' `GeosBackendError::Geos` se la
/// codifica WKB intermedia fallisce.
pub fn make_valid_geometry(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
) -> Result<Geometry<f64>, GeosBackendError> {
    let payload = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| GeosBackendError::Geos(error.to_string()))?;
    let repaired = make_valid_wkb(&payload, method, keep_collapsed)?;
    geometry_from_wkb(&repaired).map_err(GeosBackendError::from)
}

use crate::geometry_type_name as geometry_type;

fn ensure_linework(
    geometry: &Geometry<f64>,
    operation: &'static str,
) -> Result<(), GeosBackendError> {
    match geometry {
        Geometry::LineString(_) | Geometry::MultiLineString(_) => Ok(()),
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                ensure_linework(child, operation)?;
            }
            Ok(())
        }
        _ => Err(GeosBackendError::UnsupportedGeometry {
            operation,
            actual: geometry_type(geometry),
        }),
    }
}

fn checked_geos_input(
    geometry: &Geometry<f64>,
    max_coordinates: u64,
) -> Result<GeosGeometry, GeosBackendError> {
    let actual =
        u64::try_from(geometry.coords_count()).map_err(|_| GeosBackendError::CoordinateLimit {
            actual: u64::MAX,
            limit: max_coordinates,
        })?;
    if actual > max_coordinates {
        return Err(GeosBackendError::CoordinateLimit {
            actual,
            limit: max_coordinates,
        });
    }
    let payload = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| GeosBackendError::Geos(error.to_string()))?;
    geometry_from_wkb(&payload)?;
    GeosGeometry::new_from_wkb(&payload).map_err(geos_error)
}

fn checked_noding_work(
    geometries: &[&Geometry<f64>],
    max_work: u64,
) -> Result<(), GeosBackendError> {
    fn line_segments(line: &LineString<f64>) -> Result<u64, GeosBackendError> {
        u64::try_from(line.0.len().saturating_sub(1)).map_err(|_| GeosBackendError::WorkLimit {
            actual: u64::MAX,
            limit: u64::MAX,
        })
    }

    fn segment_count(geometry: &Geometry<f64>) -> Result<u64, GeosBackendError> {
        let add = |left: u64, right: u64| {
            left.checked_add(right).ok_or(GeosBackendError::WorkLimit {
                actual: u64::MAX,
                limit: u64::MAX,
            })
        };
        match geometry {
            Geometry::Point(_) | Geometry::MultiPoint(_) => Ok(0),
            Geometry::Line(_) => Ok(1),
            Geometry::LineString(line) => line_segments(line),
            Geometry::Polygon(polygon) => {
                let mut total = line_segments(polygon.exterior())?;
                for ring in polygon.interiors() {
                    total = add(total, line_segments(ring)?)?;
                }
                Ok(total)
            }
            Geometry::MultiLineString(multi) => multi
                .0
                .iter()
                .try_fold(0_u64, |total, line| add(total, line_segments(line)?)),
            Geometry::MultiPolygon(multi) => multi.0.iter().try_fold(0_u64, |total, polygon| {
                add(total, segment_count(&Geometry::Polygon(polygon.clone()))?)
            }),
            Geometry::GeometryCollection(collection) => collection
                .0
                .iter()
                .try_fold(0_u64, |total, child| add(total, segment_count(child)?)),
            Geometry::Rect(_) => Ok(4),
            Geometry::Triangle(_) => Ok(3),
        }
    }

    let mut segments = 0_u64;
    for geometry in geometries {
        let count = segment_count(geometry)?;
        segments = segments
            .checked_add(count)
            .ok_or(GeosBackendError::WorkLimit {
                actual: u64::MAX,
                limit: max_work,
            })?;
    }
    let actual = segments
        .checked_mul(segments)
        .ok_or(GeosBackendError::WorkLimit {
            actual: u64::MAX,
            limit: max_work,
        })?;
    if actual > max_work {
        return Err(GeosBackendError::WorkLimit {
            actual,
            limit: max_work,
        });
    }
    Ok(())
}

fn geos_to_geo(geometry: &GeosGeometry) -> Result<Geometry<f64>, GeosBackendError> {
    let payload = geometry.to_wkb().map_err(geos_error)?;
    // Come in make_valid_wkb: geometry_from_wkb include il contratto
    // strutturale (decoder validante, ADR-0011) — nessuna scansione
    // separata, la garanzia fail-closed e' identica.
    geometry_from_wkb(&payload).map_err(GeosBackendError::from)
}

fn collect_polygons(
    geometry: Geometry<f64>,
    output: &mut Vec<Polygon<f64>>,
) -> Result<(), GeosBackendError> {
    match geometry {
        Geometry::Polygon(polygon) => output.push(polygon),
        Geometry::MultiPolygon(multi) => output.extend(multi.0),
        Geometry::GeometryCollection(collection) => {
            for child in collection.0 {
                collect_polygons(child, output)?;
            }
        }
        other if other.coords_count() == 0 => {}
        other => {
            return Err(GeosBackendError::InvalidOutput(format!(
                "polygonize ha prodotto {} nella collezione poligonale",
                geometry_type(&other)
            )));
        }
    }
    Ok(())
}

fn collect_linework(
    geometry: Geometry<f64>,
    output: &mut Vec<LineString<f64>>,
) -> Result<(), GeosBackendError> {
    match geometry {
        Geometry::LineString(line) => output.push(line),
        Geometry::MultiLineString(multi) => output.extend(multi.0),
        Geometry::GeometryCollection(collection) => {
            for child in collection.0 {
                collect_linework(child, output)?;
            }
        }
        other if other.coords_count() == 0 => {}
        other => {
            return Err(GeosBackendError::InvalidOutput(format!(
                "polygonize ha prodotto {} nei residui lineari",
                geometry_type(&other)
            )));
        }
    }
    Ok(())
}

fn optional_linework(
    geometry: Option<GeosGeometry>,
) -> Result<Vec<LineString<f64>>, GeosBackendError> {
    let Some(geometry) = geometry else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    collect_linework(geos_to_geo(&geometry)?, &mut output)?;
    Ok(output)
}

#[derive(Clone, Debug)]
pub struct PolygonizeResult {
    pub polygons: Vec<Polygon<f64>>,
    pub cut_edges: Vec<LineString<f64>>,
    pub dangles: Vec<LineString<f64>>,
    pub invalid_ring_lines: Vec<LineString<f64>>,
}

impl PolygonizeResult {
    #[must_use]
    pub const fn residual_count(&self) -> usize {
        self.cut_edges.len() + self.dangles.len() + self.invalid_ring_lines.len()
    }
}

fn checked_output_coordinates<'a>(
    geometries: impl IntoIterator<Item = &'a Geometry<f64>>,
    limit: u64,
) -> Result<(), GeosBackendError> {
    let mut actual = 0_u64;
    for geometry in geometries {
        let count =
            u64::try_from(geometry.coords_count()).map_err(|_| GeosBackendError::OutputLimit {
                actual: u64::MAX,
                limit,
            })?;
        actual = actual
            .checked_add(count)
            .ok_or(GeosBackendError::OutputLimit {
                actual: u64::MAX,
                limit,
            })?;
    }
    if actual > limit {
        return Err(GeosBackendError::OutputLimit { actual, limit });
    }
    Ok(())
}

/// Polygonizes valid 2D linework and preserves every residual category.
/// `node_input` explicitly inserts crossing nodes before polygonization.
///
/// # Errors
///
/// Restituisce `GeosBackendError` se l'input non e' linework o supera i
/// limiti di coordinate/noding, se GEOS fallisce, se l'output supera i
/// limiti dichiarati o se `require_complete` e' attivo e restano residui.
pub fn polygonize_linework(
    linework: &Geometry<f64>,
    node_input: bool,
    require_complete: bool,
    max_input_coordinates: u64,
    max_noding_work: u64,
    max_output_geometries: u64,
    max_output_coordinates: u64,
) -> Result<PolygonizeResult, GeosBackendError> {
    ensure_linework(linework, "polygonize")?;
    if node_input {
        checked_noding_work(&[linework], max_noding_work)?;
    }
    let input = checked_geos_input(linework, max_input_coordinates)?;
    let working = if node_input {
        input.node().map_err(geos_error)?
    } else {
        input
    };
    let (polygons, cuts, dangles, invalid_ring_lines) =
        working.polygonize_full().map_err(geos_error)?;
    let mut polygon_output = Vec::new();
    collect_polygons(geos_to_geo(&polygons)?, &mut polygon_output)?;
    let result = PolygonizeResult {
        polygons: polygon_output,
        cut_edges: optional_linework(cuts)?,
        dangles: optional_linework(dangles)?,
        invalid_ring_lines: optional_linework(invalid_ring_lines)?,
    };
    let actual = result
        .polygons
        .len()
        .checked_add(result.residual_count())
        .ok_or(GeosBackendError::OutputLimit {
            actual: u64::MAX,
            limit: max_output_geometries,
        })?;
    let actual = u64::try_from(actual).map_err(|_| GeosBackendError::OutputLimit {
        actual: u64::MAX,
        limit: max_output_geometries,
    })?;
    if actual > max_output_geometries {
        return Err(GeosBackendError::OutputLimit {
            actual,
            limit: max_output_geometries,
        });
    }
    let output_geometries = result
        .polygons
        .iter()
        .cloned()
        .map(Geometry::Polygon)
        .chain(result.cut_edges.iter().cloned().map(Geometry::LineString))
        .chain(result.dangles.iter().cloned().map(Geometry::LineString))
        .chain(
            result
                .invalid_ring_lines
                .iter()
                .cloned()
                .map(Geometry::LineString),
        )
        .collect::<Vec<_>>();
    checked_output_coordinates(output_geometries.iter(), max_output_coordinates)?;
    if require_complete && result.residual_count() != 0 {
        return Err(GeosBackendError::IncompletePolygonize {
            cuts: result.cut_edges.len(),
            dangles: result.dangles.len(),
            invalid_rings: result.invalid_ring_lines.len(),
        });
    }
    Ok(result)
}

/// Splits a Polygon/MultiPolygon with linear geometry.
///
/// Candidate faces are polygonized from fully noded boundary + splitter
/// linework, filtered using an interior point, then checked for exact area
/// conservation within a tight floating tolerance.
///
/// # Errors
///
/// Restituisce `GeosBackendError` se gli input non sono dei tipi attesi o
/// superano i limiti di coordinate/noding, se GEOS fallisce, se l'output
/// supera i limiti dichiarati o se la conservazione di area/copertura non
/// e' verificata.
pub fn split_polygon_by_linework(
    source: &Geometry<f64>,
    splitter: &Geometry<f64>,
    max_input_coordinates: u64,
    max_noding_work: u64,
    max_output_parts: u64,
    max_output_coordinates: u64,
) -> Result<Vec<Polygon<f64>>, GeosBackendError> {
    if !matches!(source, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
        return Err(GeosBackendError::UnsupportedGeometry {
            operation: "split",
            actual: geometry_type(source),
        });
    }
    ensure_linework(splitter, "split")?;
    checked_noding_work(&[source, splitter], max_noding_work)?;
    let source_geos = checked_geos_input(source, max_input_coordinates)?;
    let splitter_geos = checked_geos_input(splitter, max_input_coordinates)?;
    let boundary = source_geos.boundary().map_err(geos_error)?;
    let combined = boundary.union(&splitter_geos).map_err(geos_error)?;
    let noded = combined.node().map_err(geos_error)?;
    let (faces, _, _, _) = noded.polygonize_full().map_err(geos_error)?;
    let mut candidates = Vec::new();
    collect_polygons(geos_to_geo(&faces)?, &mut candidates)?;

    let mut output = Vec::new();
    for polygon in candidates {
        let candidate = Geometry::Polygon(polygon.clone());
        let Some(point) = crate::operations::point_on_surface(&candidate)
            .map_err(|error| GeosBackendError::InvalidOutput(error.to_string()))?
        else {
            continue;
        };
        if crate::predicates::evaluate(source, &point, crate::predicates::SpatialPredicate::Covers)
            .map_err(|error| GeosBackendError::InvalidOutput(error.to_string()))?
        {
            output.push(polygon);
        }
    }
    let actual = u64::try_from(output.len()).map_err(|_| GeosBackendError::OutputLimit {
        actual: u64::MAX,
        limit: max_output_parts,
    })?;
    if actual > max_output_parts {
        return Err(GeosBackendError::OutputLimit {
            actual,
            limit: max_output_parts,
        });
    }
    let output_geometries = output
        .iter()
        .cloned()
        .map(Geometry::Polygon)
        .collect::<Vec<_>>();
    checked_output_coordinates(output_geometries.iter(), max_output_coordinates)?;
    let input_area = source.unsigned_area();
    let output_area: f64 = output.iter().map(Area::unsigned_area).sum();
    let allowed_error = input_area.abs().max(1.0) * 1e-9;
    if (output_area - input_area).abs() > allowed_error {
        return Err(GeosBackendError::AreaMismatch {
            input: input_area,
            output: output_area,
        });
    }
    // Area sums alone can hide a gap and an overlap of equal size. Verify the
    // actual covered set independently using GEOS' robust unary union and
    // symmetric difference against the source.
    let output_geometry = Geometry::MultiPolygon(geo::MultiPolygon(output.clone()));
    let output_payload = output_geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| GeosBackendError::Geos(error.to_string()))?;
    let output_geos = GeosGeometry::new_from_wkb(&output_payload).map_err(geos_error)?;
    let merged_output = output_geos.unary_union().map_err(geos_error)?;
    let coverage_difference = source_geos
        .sym_difference(&merged_output)
        .map_err(geos_error)?;
    let coverage_error = coverage_difference.area().map_err(geos_error)?;
    if !coverage_error.is_finite() || coverage_error > allowed_error {
        return Err(GeosBackendError::CoverageMismatch {
            area: coverage_error,
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::algorithm::validation::Validation;
    use geo::{line_string, polygon, Area, Geometry};
    use geozero::{CoordDimensions, ToWkb};

    fn bow_tie_wkb() -> Vec<u8> {
        Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
            (x: 0.0, y: 2.0), (x: 2.0, y: 0.0),
            (x: 0.0, y: 0.0),
        ])
        .to_wkb(CoordDimensions::xy())
        .unwrap()
    }

    #[test]
    fn repairs_self_intersection_with_both_geos_methods() {
        let input = bow_tie_wkb();
        assert!(geometry_from_wkb(&input).is_err());
        for method in [RepairMethod::Linework, RepairMethod::Structure] {
            let output = make_valid_wkb(&input, method, false).unwrap();
            let repaired = geometry_from_wkb(&output).unwrap();
            assert!((repaired.unsigned_area() - 2.0).abs() < 1e-12);
        }
    }

    #[test]
    fn valid_input_is_returned_byte_for_byte() {
        let input = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 2.0), (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ])
        .to_wkb(CoordDimensions::xy())
        .unwrap();
        assert_eq!(
            make_valid_wkb(&input, RepairMethod::Structure, false).unwrap(),
            input
        );
    }

    #[test]
    fn malformed_or_non_finite_wkb_never_reaches_geos() {
        assert!(make_valid_wkb(&[1, 2, 3], RepairMethod::Structure, false).is_err());
        let mut nan_point = vec![1_u8, 1, 0, 0, 0];
        nan_point.extend_from_slice(&f64::NAN.to_le_bytes());
        nan_point.extend_from_slice(&1.0_f64.to_le_bytes());
        assert!(make_valid_wkb(&nan_point, RepairMethod::Structure, false).is_err());
    }

    #[test]
    fn polygonize_preserves_residual_linework_and_can_fail_closed() {
        let linework = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)],
            line_string![(x: 2.0, y: 0.0), (x: 2.0, y: 2.0)],
            line_string![(x: 2.0, y: 2.0), (x: 0.0, y: 2.0)],
            line_string![(x: 0.0, y: 2.0), (x: 0.0, y: 0.0)],
            line_string![(x: 2.0, y: 2.0), (x: 3.0, y: 2.0)],
        ]));
        let result = polygonize_linework(&linework, true, false, 100, 1_000, 100, 100).unwrap();
        assert_eq!(result.polygons.len(), 1);
        assert_eq!(result.residual_count(), 1);
        assert!(matches!(
            polygonize_linework(&linework, true, true, 100, 1_000, 100, 100),
            Err(GeosBackendError::IncompletePolygonize { .. })
        ));
        assert!(matches!(
            polygonize_linework(&linework, true, false, 4, 1_000, 100, 100),
            Err(GeosBackendError::CoordinateLimit { .. })
        ));
        assert!(matches!(
            polygonize_linework(&linework, true, false, 100, 1_000, 1, 100),
            Err(GeosBackendError::OutputLimit { .. })
        ));
        assert!(matches!(
            polygonize_linework(&linework, true, false, 100, 1, 100, 100),
            Err(GeosBackendError::WorkLimit { .. })
        ));
        assert!(matches!(
            polygonize_linework(&linework, true, false, 100, 1_000, 100, 6),
            Err(GeosBackendError::OutputLimit { .. })
        ));
    }

    #[test]
    fn polygon_split_conserves_area_and_ignores_outside_faces() {
        let source = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
            (x: 10.0, y: 10.0), (x: 0.0, y: 10.0),
            (x: 0.0, y: 0.0),
        ]);
        let splitter = Geometry::MultiLineString(geo::MultiLineString(vec![
            line_string![(x: 5.0, y: -1.0), (x: 5.0, y: 11.0)],
            line_string![(x: 20.0, y: 20.0), (x: 22.0, y: 20.0),
                         (x: 22.0, y: 22.0), (x: 20.0, y: 22.0),
                         (x: 20.0, y: 20.0)],
        ]));
        let pieces = split_polygon_by_linework(&source, &splitter, 100, 10_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 2);
        let total_area = pieces.iter().map(Area::unsigned_area).sum::<f64>();
        assert!((total_area - 100.0).abs() < f64::EPSILON);
        assert!(matches!(
            split_polygon_by_linework(&source, &splitter, 100, 10_000, 1, 100),
            Err(GeosBackendError::OutputLimit { .. })
        ));
        assert!(matches!(
            split_polygon_by_linework(&source, &splitter, 100, 1, 10, 100),
            Err(GeosBackendError::WorkLimit { .. })
        ));
        assert!(matches!(
            split_polygon_by_linework(&source, &splitter, 100, 10_000, 10, 9),
            Err(GeosBackendError::OutputLimit { .. })
        ));
    }

    #[test]
    fn polygon_split_handles_holes_multipolygons_and_boundary_coincidence() {
        let holed = Geometry::Polygon(Polygon::new(
            line_string![
                (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
                (x: 10.0, y: 10.0), (x: 0.0, y: 10.0),
                (x: 0.0, y: 0.0)
            ],
            vec![line_string![
                (x: 4.0, y: 4.0), (x: 6.0, y: 4.0),
                (x: 6.0, y: 6.0), (x: 4.0, y: 6.0),
                (x: 4.0, y: 4.0)
            ]],
        ));
        let through_hole = Geometry::LineString(line_string![(x: 5.0, y: -1.0), (x: 5.0, y: 11.0)]);
        let pieces =
            split_polygon_by_linework(&holed, &through_hole, 100, 10_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 2);
        assert!((pieces.iter().map(Area::unsigned_area).sum::<f64>() - 96.0).abs() < 1e-12);

        let source = Geometry::MultiPolygon(geo::MultiPolygon(vec![
            polygon![
                (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
                (x: 2.0, y: 2.0), (x: 0.0, y: 2.0),
                (x: 0.0, y: 0.0)
            ],
            polygon![
                (x: 4.0, y: 0.0), (x: 6.0, y: 0.0),
                (x: 6.0, y: 2.0), (x: 4.0, y: 2.0),
                (x: 4.0, y: 0.0)
            ],
        ]));
        let cutter = Geometry::LineString(line_string![(x: -1.0, y: 1.0), (x: 7.0, y: 1.0)]);
        let pieces = split_polygon_by_linework(&source, &cutter, 100, 10_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 4);
        assert!((pieces.iter().map(Area::unsigned_area).sum::<f64>() - 8.0).abs() < 1e-12);

        let boundary = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)]);
        let pieces = split_polygon_by_linework(&source, &boundary, 100, 10_000, 10, 100).unwrap();
        assert_eq!(pieces.len(), 2);
        assert!((pieces.iter().map(Area::unsigned_area).sum::<f64>() - 8.0).abs() < 1e-12);
    }

    #[test]
    fn adversarial_backend_types_empty_linework_and_non_noded_modes() {
        let invalid_types = vec![
            Geometry::Point(geo::Point::new(0.0, 0.0)),
            Geometry::MultiPoint(vec![geo::Point::new(0.0, 0.0)].into()),
            Geometry::Polygon(polygon![
                (x: 0.0, y: 0.0), (x: 1.0, y: 0.0),
                (x: 1.0, y: 1.0), (x: 0.0, y: 1.0),
                (x: 0.0, y: 0.0)
            ]),
            Geometry::Rect(geo::Rect::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::Triangle(geo::Triangle::new(
                geo::Coord { x: 0.0, y: 0.0 },
                geo::Coord { x: 1.0, y: 0.0 },
                geo::Coord { x: 0.0, y: 1.0 },
            )),
        ];
        for geometry in invalid_types {
            assert!(matches!(
                polygonize_linework(&geometry, true, false, 100, 1_000, 100, 100),
                Err(GeosBackendError::UnsupportedGeometry { .. })
            ));
        }

        let empty = Geometry::MultiLineString(geo::MultiLineString(Vec::new()));
        let result = polygonize_linework(&empty, false, true, 100, 0, 100, 100).unwrap();
        assert!(result.polygons.is_empty());
        assert_eq!(result.residual_count(), 0);

        let square_lines = Geometry::GeometryCollection(
            vec![
                Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)]),
                Geometry::MultiLineString(geo::MultiLineString(vec![
                    line_string![(x: 2.0, y: 0.0), (x: 2.0, y: 2.0)],
                    line_string![(x: 2.0, y: 2.0), (x: 0.0, y: 2.0)],
                    line_string![(x: 0.0, y: 2.0), (x: 0.0, y: 0.0)],
                ])),
            ]
            .into(),
        );
        assert_eq!(
            polygonize_linework(&square_lines, false, true, 100, 0, 100, 100)
                .unwrap()
                .polygons
                .len(),
            1
        );

        let source = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 2.0), (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0)
        ]);
        assert_eq!(
            split_polygon_by_linework(&source, &empty, 100, 1_000, 10, 100)
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            split_polygon_by_linework(&source, &square_lines, 1, 1_000, 10, 100),
            Err(GeosBackendError::CoordinateLimit { .. })
        ));
    }

    #[test]
    fn make_valid_geometry_matches_the_wkb_path() {
        // ADR-0012 M3: la variante su forma decodificata deve produrre la
        // STESSA geometria del percorso WKB (che la fusione sostituisce),
        // sull'input OGC-invalido che l'operazione esiste per riparare.
        let input = bow_tie_wkb();
        let decoded = crate::wkb_decoder::decode_validated(&input).expect("gate solo strutturale");
        let via_geometry = make_valid_geometry(&decoded, RepairMethod::Linework, true)
            .expect("riparazione su forma decodificata");
        let via_wkb = make_valid_wkb(&input, RepairMethod::Linework, true).expect("wkb");
        assert_eq!(
            via_geometry,
            geometry_from_wkb(&via_wkb).expect("output wkb valido"),
            "risultato diverso tra forma decodificata e WKB"
        );
        // Input gia' valido: passthrough (stessa geometria in uscita).
        let valid = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 2.0, y: 0.0),
            (x: 2.0, y: 2.0), (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ]);
        assert_eq!(
            make_valid_geometry(&valid, RepairMethod::Linework, true).expect("passthrough"),
            valid
        );
    }

    #[test]
    fn make_valid_keep_collapsed_handles_degenerate_invalid_polygon() {
        let degenerate = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 1.0, y: 0.0),
            (x: 2.0, y: 0.0), (x: 0.0, y: 0.0)
        ])
        .to_wkb(CoordDimensions::xy())
        .unwrap();
        for keep_collapsed in [false, true] {
            let output =
                make_valid_wkb(&degenerate, RepairMethod::Structure, keep_collapsed).unwrap();
            let repaired = geometry_from_wkb(&output).unwrap();
            assert!(repaired.check_validation().is_ok());
        }
    }
}
