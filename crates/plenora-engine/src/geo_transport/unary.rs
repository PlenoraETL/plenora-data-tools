//! Macchina delle operazioni unary del trasporto Arrow v3: dispatch per
//! forma (1:1, 1:N, N:1, collettiva, diagnostica, da coordinate), handle
//! prepared e pipeline `transform_arrow` completa.

use std::io::{Read, Write};

use plenora_core::arrow::array::{
    Array, BinaryArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use geo::Geometry;
use geozero::{wkb::Wkb, CoordDimensions, ToGeo, ToWkb};
use rayon::prelude::*;

use plenora_kernels_geo::advanced::voronoi_cells;
use plenora_kernels_geo::construction::{
    line_from_ordered_points, point_from_lon_lat, polygon_from_ordered_points,
};
use plenora_core::crs::MAX_CRS_DEFINITION_BYTES;
use plenora_core::contract::GeometryDimensions;
use plenora_kernels_geo::extended::{
    affine_transform, concave_hull, geodesic_line_length_m, rotate_about, scale_about, translate,
};
use plenora_kernels_geo::extended_algorithms::{
    delaunay, densify, geodesic_area_m2, geometry_diagnostics, line_interpolate_point, line_merge,
    line_substring, snap_to_grid,
};
#[cfg(feature = "geos-backend")]
use plenora_kernels_geo::geos_backend::{make_valid_wkb, polygonize_linework, RepairMethod};
use plenora_kernels_geo::operations::{
    area, boundary, bounds, buffer_with_cap, explode, length, perimeter, point_on_surface,
    simplify_with_policy, to_wkt, vertex_count, BufferCapStyle, OperationError, SimplifyPolicy,
};
use plenora_kernels_geo::predicates::SpatialPredicate;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::proj_backend::Reprojector;
use super::protocol::MAX_ROWS;
use plenora_kernels_geo::topology::{clean_valid_polygon_topology, dissolve};
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb};

use super::envelope::{EnvelopeReader, EnvelopeWriter};
use super::error::ArrowTransportError;
use super::ipc::{decode_ipc, encode_ipc};
use super::schema::{
    ArrowOperation, ArrowShape, BufferCap, SimplifyPolicyParam, TransformArrowSchema,
    TransformArrowSummary,
};
use super::transport::{
    CLASS_COLUMN, DEFAULT_MAX_POINTS, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
    MAX_CELL_BYTES, MAX_CELL_COORDINATES, MAX_CLEAN_VERTICES, PARENT_INDEX_COLUMN, WKT_COLUMN,
};
#[cfg(feature = "geos-backend")]
use super::transport::MAX_NODING_WORK;

#[cfg(feature = "proj-backend")]
std::thread_local! {
    /// Pipeline PROJ per thread: `Reprojector` non e' `Sync`, quindi ogni
    /// thread rayon costruisce la sua una sola volta per coppia CRS.
    static REPROJECTOR: std::cell::RefCell<Option<(String, String, Reprojector)>> =
        const { std::cell::RefCell::new(None) };
}

pub(in crate::geo_transport) fn geometry_column_index(schema: &Schema, name: &str) -> Result<usize, ArrowTransportError> {
    let (index, field) = schema
        .column_with_name(name)
        .ok_or_else(|| ArrowTransportError::MissingGeometryColumn(name.to_owned()))?;
    if field.data_type() != &DataType::Binary {
        return Err(ArrowTransportError::GeometryColumnNotBinary {
            name: name.to_owned(),
            actual: field.data_type().to_string(),
        });
    }
    let extension = field.metadata().get(GEOARROW_EXTENSION_KEY);
    if extension.map(String::as_str) != Some(GEOARROW_WKB_EXTENSION) {
        return Err(ArrowTransportError::MissingGeoArrowMetadata(
            name.to_owned(),
        ));
    }
    Ok(index)
}

/// Metadato `GeoArrow` `geo` con la chiave `crs`: PROJJSON se la definizione e'
/// gia' un oggetto JSON, altrimenti la forma authority:code come stringa.
///
/// Unificazione B1.1: l'assemblaggio JSON e' unico in
/// [`plenora_kernels_geo::arrow_adapter::geo_metadata_json`] (stesso output
/// byte-per-byte); qui restano solo le validazioni con le varianti
/// d'errore strutturate del trasporto.
pub(in crate::geo_transport) fn geo_metadata_json(crs: &str) -> Result<String, ArrowTransportError> {
    if crs.trim().is_empty() {
        return Err(ArrowTransportError::CrsRequired);
    }
    if crs.len() > MAX_CRS_DEFINITION_BYTES {
        return Err(ArrowTransportError::CrsTooLarge);
    }
    plenora_kernels_geo::arrow_adapter::geo_metadata_json(crs)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

pub(in crate::geo_transport) fn geometry_output_field(name: &str, crs: &str) -> Result<Field, ArrowTransportError> {
    // Validazione CRS con le varianti strutturate del trasporto; la
    // costruzione del campo (metadati geoarrow.wkb + geo.crs +
    // geo.dimensions) e' unica in `arrow_adapter` (unificazione B1.1).
    geo_metadata_json(crs)?;
    // B1.3: la dimensionalita' dichiarata e' Xy ESPLICITO, non un default
    // silenzioso — ogni output di questo trasporto e' prodotto decodificando
    // in `Geometry<f64>` e ricodificando `to_wkb(CoordDimensions::xy())`,
    // quindi le celle sono sempre WKB 2D; gli input Z/M sono rifiutati a
    // compile-plan (`analyze_geo_contract`) prima di arrivare qui.
    // B1.4: per lo stesso motivo l'encoding e' `None` — le celle ricodificate
    // sono WKB ISO XY e la chiave `encoding` e' omessa (mai ereditata
    // dall'input, fingerprint invariato).
    plenora_kernels_geo::arrow_adapter::geometry_output_field_with_encoding(
        name,
        crs,
        GeometryDimensions::Xy,
        None,
    )
    .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

/// Risultato per cella di una trasformazione 1:1.
enum TransformedColumn {
    Binary(Vec<Option<Vec<u8>>>),
    Float64(Vec<Option<f64>>),
    UInt64(Vec<Option<u64>>),
    Utf8(Vec<Option<String>>),
    Bounds(Vec<Option<[f64; 4]>>),
}

/// Codifica una geometria gia' validata dal kernel in WKB 2D entro il limite
/// per cella.
pub(in crate::geo_transport) fn encode_geometry(geometry: &Geometry<f64>) -> Result<Vec<u8>, ArrowTransportError> {
    let payload = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| OperationError::InvalidOutput(error.to_string()))?;
    if payload.len() as u64 > MAX_CELL_BYTES {
        return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
    }
    Ok(payload)
}

/// Applica `f` a ogni cella non-null preservando i null; il limite per cella
/// e' applicato prima di toccare i dati. Le righe sono indipendenti:
/// l'iterazione e' parallela (rayon) con collect indicizzato, quindi
/// l'ordine dell'output resta deterministico. Ogni kernel per-cella e'
/// thread-safe: puri Rust, GEOS con contesto thread-local, PROJ con
/// pipeline thread-local (vedi il ramo `Reproject`).
fn map_nullable<T: Send>(
    cells: &BinaryArray,
    f: impl Fn(&[u8]) -> Result<Option<T>, ArrowTransportError> + Sync,
) -> Result<Vec<Option<T>>, ArrowTransportError> {
    let cell_values: Vec<Option<&[u8]>> = cells.iter().collect();
    // ADR-0001: come la primitiva omonima in arrow_adapter — i `Result`
    // per riga prima (ordine preservato dal collect indicizzato), il primo
    // errore IN ORDINE DI RIGA poi, dal collect sequenziale; mai la
    // selezione non deterministica di rayon.
    let results: Vec<Result<Option<T>, ArrowTransportError>> = cell_values
        .into_par_iter()
        .map(|cell| match cell {
            None => Ok(None),
            Some(payload) => {
                if payload.len() as u64 > MAX_CELL_BYTES {
                    return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
                }
                f(payload)
            }
        })
        .collect();
    results.into_iter().collect()
}

// Dispatch per operazione intenzionalmente monolitico: ogni braccio e' un
// caso della tabella operazione -> kernel; la scomposizione strutturale e'
// rimandata a una fase dedicata.
#[allow(clippy::too_many_lines)]
fn transform_cells(
    params: &TransformArrowSchema,
    cells: &BinaryArray,
) -> Result<TransformedColumn, ArrowTransportError> {
    let operation = params.operation;
    match operation {
        ArrowOperation::Centroid | ArrowOperation::ConvexHull | ArrowOperation::Envelope => {
            let kernel = operation
                .geometry_kernel()
                .ok_or(ArrowTransportError::Internal("operazione geometrica senza kernel"))?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                Ok(transform_wkb(kernel, payload).map(Some)?)
            })?))
        }
        ArrowOperation::Buffer => {
            let distance = params.required_distance()?;
            let cap = BufferCapStyle::from(params.cap.unwrap_or(BufferCap::Round));
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&buffer_with_cap(&geometry, distance, cap)?).map(Some)
            })?))
        }
        ArrowOperation::Simplify => {
            let tolerance = params.required_tolerance()?;
            let policy = SimplifyPolicy::from(
                params
                    .simplify_policy
                    .unwrap_or(SimplifyPolicyParam::DouglasPeucker),
            );
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&simplify_with_policy(&geometry, tolerance, policy)?).map(Some)
            })?))
        }
        ArrowOperation::Boundary => {
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&boundary(&geometry)?).map(Some)
            })?))
        }
        ArrowOperation::PointOnSurface => {
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                point_on_surface(&geometry)?
                    .map(|point| encode_geometry(&point))
                    .transpose()
            })?))
        }
        #[cfg(feature = "geos-backend")]
        ArrowOperation::MakeValid => {
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                // A differenza delle altre operazioni, l'input puo' essere
                // OGC-invalido: e' esattamente cio' che make_valid ripara.
                Ok(make_valid_wkb(payload, RepairMethod::Linework, true).map(Some)?)
            })?))
        }
        #[cfg(not(feature = "geos-backend"))]
        ArrowOperation::MakeValid => Err(ArrowTransportError::BackendUnavailable {
            operation: operation.name(),
            feature: "geos-backend",
        }),
        #[cfg(feature = "proj-backend")]
        ArrowOperation::Reproject => {
            let source = params
                .crs
                .as_deref()
                .ok_or(ArrowTransportError::CrsRequired)?
                .to_owned();
            let target = params.required_target_crs()?.to_owned();
            Ok(TransformedColumn::Binary(map_nullable(
                cells,
                move |payload| {
                    let geometry = geometry_from_wkb(payload)?;
                    // Una pipeline PROJ per thread (PROJ non e' Sync), riusata su
                    // tutte le celle del batch e ricreata solo se cambia coppia.
                    REPROJECTOR.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let stale = slot
                            .as_ref()
                            .is_none_or(|(s, t, _)| s != &source || t != &target);
                        if stale {
                            *slot = Some((
                                source.clone(),
                                target.clone(),
                                Reprojector::new(&source, &target, MAX_CELL_COORDINATES)?,
                            ));
                        }
                        let (_, _, reprojector) = slot
                            .as_ref()
                            .ok_or(ArrowTransportError::Internal("pipeline appena creata assente"))?;
                        let reprojected = reprojector.reproject(&geometry)?;
                        encode_geometry(&reprojected).map(Some)
                    })
                },
            )?))
        }
        #[cfg(not(feature = "proj-backend"))]
        ArrowOperation::Reproject => Err(ArrowTransportError::BackendUnavailable {
            operation: operation.name(),
            feature: "proj-backend",
        }),
        ArrowOperation::Area => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                Ok(area(&geometry).map(Some)?)
            },
        )?)),
        ArrowOperation::Length => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                Ok(length(&geometry).map(Some)?)
            },
        )?)),
        ArrowOperation::Perimeter => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                Ok(perimeter(&geometry).map(Some)?)
            },
        )?)),
        ArrowOperation::VertexCount => {
            Ok(TransformedColumn::UInt64(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                Ok(vertex_count(&geometry).map(Some)?)
            })?))
        }
        ArrowOperation::Bounds => Ok(TransformedColumn::Bounds(map_nullable(cells, |payload| {
            let geometry = geometry_from_wkb(payload)?;
            Ok(bounds(&geometry)?)
        })?)),
        ArrowOperation::ToWkt => Ok(TransformedColumn::Utf8(map_nullable(cells, |payload| {
            let geometry = geometry_from_wkb(payload)?;
            Ok(to_wkt(&geometry).map(Some)?)
        })?)),
        ArrowOperation::AffineTransform => {
            let coefficients: [f64; 6] = params
                .coefficients
                .as_deref()
                .ok_or(ArrowTransportError::Internal("coefficients validato assente"))?
                .try_into()
                .map_err(|_| {
                    ArrowTransportError::Internal("coefficients validato non di 6 elementi")
                })?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&affine_transform(&geometry, coefficients)?).map(Some)
            })?))
        }
        ArrowOperation::Translate => {
            let x = params.required_f64("x_offset", params.x_offset)?;
            let y = params.required_f64("y_offset", params.y_offset)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&translate(&geometry, x, y)?).map(Some)
            })?))
        }
        ArrowOperation::Scale => {
            let x_factor = params.required_f64("x_factor", params.x_factor)?;
            let y_factor = params.required_f64("y_factor", params.y_factor)?;
            let origin = geo::Point::new(
                params.x_origin.unwrap_or(0.0),
                params.y_origin.unwrap_or(0.0),
            );
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&scale_about(&geometry, x_factor, y_factor, origin)?).map(Some)
            })?))
        }
        ArrowOperation::Rotate => {
            let degrees = params.required_f64("degrees", params.degrees)?;
            let origin = geo::Point::new(
                params.x_origin.unwrap_or(0.0),
                params.y_origin.unwrap_or(0.0),
            );
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&rotate_about(&geometry, degrees, origin)?).map(Some)
            })?))
        }
        ArrowOperation::ConcaveHull => {
            let concavity = params.required_f64("concavity", params.concavity)?;
            let length_threshold = params.length_threshold.unwrap_or(0.0);
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&concave_hull(
                    &geometry,
                    concavity,
                    length_threshold,
                    MAX_CELL_COORDINATES,
                )?)
                .map(Some)
            })?))
        }
        ArrowOperation::Densify => {
            let max_segment_length =
                params.required_f64("max_segment_length", params.max_segment_length)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&densify(
                    &geometry,
                    max_segment_length,
                    MAX_CELL_COORDINATES,
                )?)
                .map(Some)
            })?))
        }
        ArrowOperation::SnapToGrid => {
            let grid_size = params.required_f64("grid_size", params.grid_size)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                encode_geometry(&snap_to_grid(&geometry, grid_size)?).map(Some)
            })?))
        }
        ArrowOperation::LineSubstring => {
            let start = params.required_f64("start_ratio", params.start_ratio)?;
            let end = params.required_f64("end_ratio", params.end_ratio)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                let line = expect_line_string(&geometry, operation.name())?;
                line_substring(&line, start, end)?
                    .map(|output| encode_geometry(&output))
                    .transpose()
            })?))
        }
        ArrowOperation::LineInterpolatePoint => {
            let ratio = params.required_f64("ratio", params.ratio)?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                let geometry = geometry_from_wkb(payload)?;
                let line = expect_line_string(&geometry, operation.name())?;
                line_interpolate_point(&line, ratio)?
                    .map(|point| encode_geometry(&Geometry::Point(point)))
                    .transpose()
            })?))
        }
        ArrowOperation::GeodesicLineLength => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                let line = expect_line_string(&geometry, operation.name())?;
                Ok(geodesic_line_length_m(&line).map(Some)?)
            },
        )?)),
        ArrowOperation::GeodesicArea => Ok(TransformedColumn::Float64(map_nullable(
            cells,
            |payload| {
                let geometry = geometry_from_wkb(payload)?;
                match &geometry {
                    Geometry::Polygon(_) | Geometry::MultiPolygon(_) => {}
                    other => {
                        return Err(ArrowTransportError::WrongGeometryType {
                            operation: operation.name(),
                            expected: "Polygon/MultiPolygon",
                            actual: geometry_type_name(other).to_owned(),
                        })
                    }
                }
                Ok(geodesic_area_m2(&geometry).map(Some)?)
            },
        )?)),
        ArrowOperation::Explode
        | ArrowOperation::Dissolve
        | ArrowOperation::LineBuilder
        | ArrowOperation::PolygonBuilder
        | ArrowOperation::Voronoi
        | ArrowOperation::FromCoords
        | ArrowOperation::CleanTopology
        | ArrowOperation::GeometryDiagnostics
        | ArrowOperation::Delaunay
        | ArrowOperation::Polygonize
        | ArrowOperation::LineMerge => Err(ArrowTransportError::Arrow(
            "operazione non 1:1 nel percorso per-cella".to_owned(),
        )),
    }
}

pub(in crate::geo_transport) const fn geometry_type_name(geometry: &Geometry<f64>) -> &'static str {
    match geometry {
        Geometry::Point(_) => "Point",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::Line(_) => "Line",
        Geometry::LineString(_) => "LineString",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::Polygon(_) => "Polygon",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
        Geometry::Rect(_) => "Rect",
        Geometry::Triangle(_) => "Triangle",
    }
}

pub(in crate::geo_transport) fn expect_line_string(
    geometry: &Geometry<f64>,
    operation: &'static str,
) -> Result<geo::LineString<f64>, ArrowTransportError> {
    match geometry {
        Geometry::LineString(line) => Ok(line.clone()),
        other => Err(ArrowTransportError::WrongGeometryType {
            operation,
            expected: "LineString",
            actual: geometry_type_name(other).to_owned(),
        }),
    }
}

pub(in crate::geo_transport) const fn spatial_predicate_name(predicate: SpatialPredicate) -> &'static str {
    match predicate {
        SpatialPredicate::Intersects => "intersects",
        SpatialPredicate::Disjoint => "disjoint",
        SpatialPredicate::Contains => "contains",
        SpatialPredicate::Within => "within",
        SpatialPredicate::EqualsTopo => "equals_topo",
        SpatialPredicate::Covers => "covers",
        SpatialPredicate::CoveredBy => "covered_by",
        SpatialPredicate::ContainsProperly => "contains_properly",
        SpatialPredicate::Touches => "touches",
        SpatialPredicate::Crosses => "crosses",
        SpatialPredicate::Overlaps => "overlaps",
    }
}

pub(in crate::geo_transport) fn expect_point(
    geometry: &Geometry<f64>,
    operation: &'static str,
) -> Result<geo::Point<f64>, ArrowTransportError> {
    match geometry {
        Geometry::Point(point) => Ok(*point),
        other => Err(ArrowTransportError::WrongGeometryType {
            operation,
            expected: "Point",
            actual: geometry_type_name(other).to_owned(),
        }),
    }
}

fn bounds_column_names(geometry_column: &str) -> [String; 4] {
    [
        format!("{geometry_column}_minx"),
        format!("{geometry_column}_miny"),
        format!("{geometry_column}_maxx"),
        format!("{geometry_column}_maxy"),
    ]
}

/// Applica l'operazione ai batch in input secondo la sua forma
/// (1:1, 1:N, N:1, collettiva, costruzione da coordinate).
///
/// # Errors
///
/// Propaga gli errori del percorso specifico della forma (validazione
/// parametri, limiti di risorse, kernel); `ArrowTransportError::Internal` se
/// una forma non e' coperta dal dispatch (difetto del trasporto).
pub fn transform_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    match params.operation.shape() {
        ArrowShape::OneToOne => one_to_one_batches(schema, batches, params),
        ArrowShape::OneToMany => explode_batches(schema, batches, params),
        ArrowShape::ManyToOne => collect_batches(schema, batches, params),
        ArrowShape::Collective => match params.operation {
            ArrowOperation::Voronoi => voronoi_batches(schema, batches, params),
            ArrowOperation::CleanTopology => clean_topology_batches(schema, batches, params),
            _ => Err(ArrowTransportError::Internal("shape Collective non coperta")),
        },
        ArrowShape::WholeToMany => match params.operation {
            ArrowOperation::Polygonize => polygonize_batches(schema, batches, params),
            ArrowOperation::LineMerge => line_merge_batches(schema, batches, params),
            _ => Err(ArrowTransportError::Internal("shape WholeToMany non coperta")),
        },
        ArrowShape::FromCoords => from_coords_batches(schema, batches, params),
        ArrowShape::Diagnostic => diagnostics_batches(schema, batches, params),
    }
}

/// Handle prepared delle operazioni 1:1 (V2).
///
/// Indice di colonna e schema di output sono risolti UNA volta per kernel,
/// non per batch — il lavoro che `one_to_one_batches` rifaceva a ogni
/// chiamata (clone dei `Field` con le mappe metadata, serializzazione JSON
/// del metadato `geo`, ricerca per nome).
pub struct OneToOnePrepared {
    geometry_index: usize,
    output_schema: SchemaRef,
}

/// Risolve l'handle prepared di un'operazione 1:1 (V2).
///
/// # Errors
///
/// Come `one_to_one_batches` per la parte di risoluzione (colonna
/// geometria assente, CRS richiesto assente, operazione non coperta).
pub fn prepare_one_to_one(
    schema: &SchemaRef,
    params: &TransformArrowSchema,
) -> Result<OneToOnePrepared, ArrowTransportError> {
    let operation = params.operation;
    let geometry_column = params.geometry_column();
    let output_crs = match operation {
        ArrowOperation::Reproject => params.required_target_crs()?,
        _ => params
            .crs
            .as_deref()
            .ok_or(ArrowTransportError::CrsRequired)?,
    };
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    if operation.produces_geometry() {
        output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    } else {
        match operation {
            ArrowOperation::Area
            | ArrowOperation::Length
            | ArrowOperation::Perimeter
            | ArrowOperation::GeodesicLineLength
            | ArrowOperation::GeodesicArea => {
                output_fields[geometry_index] =
                    Field::new(operation.name(), DataType::Float64, true);
            }
            ArrowOperation::VertexCount => {
                output_fields[geometry_index] =
                    Field::new(operation.name(), DataType::UInt64, true);
            }
            ArrowOperation::ToWkt => {
                output_fields[geometry_index] = Field::new(WKT_COLUMN, DataType::Utf8, true);
            }
            ArrowOperation::Bounds => {
                let bounds_fields: Vec<Field> = bounds_column_names(geometry_column)
                    .into_iter()
                    .map(|name| Field::new(name, DataType::Float64, true))
                    .collect();
                output_fields.splice(geometry_index..=geometry_index, bounds_fields);
            }
            _ => {
                return Err(ArrowTransportError::Internal(
                    "operazione non geometrica non coperta",
                ))
            }
        }
    }
    let output_schema = std::sync::Arc::new(Schema::new_with_metadata(
        output_fields,
        schema.metadata().clone(),
    ));
    Ok(OneToOnePrepared {
        geometry_index,
        output_schema,
    })
}

/// Batch trasformato con l'handle prepared: nessuna ricostruzione di
/// schema per batch (`try_new` rivalida le colonne — fail-closed).
///
/// # Errors
///
/// Come `one_to_one_batches` per la parte dati (colonna non Binary,
/// errori del kernel di cella, schema incoerente con le colonne).
pub fn one_to_one_batch_prepared(
    batch: &RecordBatch,
    params: &TransformArrowSchema,
    prepared: &OneToOnePrepared,
) -> Result<RecordBatch, ArrowTransportError> {
    let geometry_index = prepared.geometry_index;
    let cells = batch
        .column(geometry_index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| ArrowTransportError::GeometryColumnNotBinary {
            name: params.geometry_column().to_owned(),
            actual: batch.column(geometry_index).data_type().to_string(),
        })?;
    let transformed = transform_cells(params, cells)?;
    let mut columns = batch.columns().to_vec();
    match transformed {
        TransformedColumn::Binary(values) => {
            columns[geometry_index] = std::sync::Arc::new(
                values.iter().map(|cell| cell.as_deref()).collect::<BinaryArray>(),
            );
        }
        TransformedColumn::Float64(values) => {
            columns[geometry_index] = std::sync::Arc::new(Float64Array::from(values));
        }
        TransformedColumn::UInt64(values) => {
            columns[geometry_index] = std::sync::Arc::new(UInt64Array::from(values));
        }
        TransformedColumn::Utf8(values) => {
            columns[geometry_index] = std::sync::Arc::new(StringArray::from(values));
        }
        TransformedColumn::Bounds(values) => {
            let arrays: Vec<plenora_core::arrow::array::ArrayRef> = (0..4)
                .map(|axis| {
                    std::sync::Arc::new(Float64Array::from(
                        values
                            .iter()
                            .map(|value| value.map(|bounds| bounds[axis]))
                            .collect::<Vec<Option<f64>>>(),
                    )) as plenora_core::arrow::array::ArrayRef
                })
                .collect();
            columns.splice(geometry_index..=geometry_index, arrays);
        }
    }
    RecordBatch::try_new(prepared.output_schema.clone(), columns)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

/// Operazioni 1:1: la colonna geometria e' sostituita dal risultato (Binary
/// GeoArrow-WKB, Float64, `UInt64`, Utf8 oppure quattro colonne Float64 per
/// `bounds`); tutte le altre colonne passano invariate; i null sono preservati.
fn one_to_one_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let prepared = prepare_one_to_one(schema, params)?;
    let mut output_batches = Vec::with_capacity(batches.len());
    for batch in batches {
        output_batches.push(one_to_one_batch_prepared(batch, params, &prepared)?);
    }
    Ok((prepared.output_schema, output_batches))
}

pub(in crate::geo_transport) fn batch_geometry_cells<'a>(
    batch: &'a RecordBatch,
    geometry_index: usize,
    geometry_column: &str,
) -> Result<&'a BinaryArray, ArrowTransportError> {
    batch
        .column(geometry_index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| ArrowTransportError::GeometryColumnNotBinary {
            name: geometry_column.to_owned(),
            actual: batch.column(geometry_index).data_type().to_string(),
        })
}

/// `explode` (1:N): ogni geometria non-null produce le sue componenti
/// nell'ordine del kernel; i null non producono righe figlie. Gli attributi
/// sono replicati sulle righe figlie e `__parent_index` (`UInt64`) riporta
/// l'indice globale della riga di input. L'espansione e' controllata
/// incrementalmente contro `max_output_rows` prima di codificare i figli.
fn explode_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let limit = params.required_max_output_rows()?;
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    output_fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let mut output_batches = Vec::with_capacity(batches.len());
    let mut total_rows = 0_u64;
    let mut row_offset = 0_u64;
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        let mut encoded: Vec<Vec<u8>> = Vec::new();
        let mut take_indices: Vec<u64> = Vec::new();
        let mut parents: Vec<u64> = Vec::new();
        for (row, cell) in cells.iter().enumerate() {
            let Some(payload) = cell else {
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            let geometry = geometry_from_wkb(payload)?;
            let parts: Vec<Geometry<f64>> = match params.operation {
                ArrowOperation::Explode => explode(&geometry)?,
                ArrowOperation::Delaunay => delaunay(&geometry, MAX_CELL_COORDINATES, limit)?
                    .into_iter()
                    .map(Geometry::Polygon)
                    .collect(),
                _ => {
                    return Err(ArrowTransportError::Internal(
                        "explode_batches: operazione non 1:N",
                    ))
                }
            };
            let next = total_rows
                .checked_add(parts.len() as u64)
                .ok_or(ArrowTransportError::StreamTooLarge)?;
            if next > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: next,
                    limit,
                });
            }
            total_rows = next;
            for part in &parts {
                encoded.push(encode_geometry(part)?);
                take_indices.push(row as u64);
                parents.push(row_offset + row as u64);
            }
        }
        let take_indices = UInt64Array::from(take_indices);
        let mut columns: Vec<plenora_core::arrow::array::ArrayRef> = Vec::with_capacity(batch.num_columns() + 1);
        for (index, column) in batch.columns().iter().enumerate() {
            if index == geometry_index {
                columns.push(std::sync::Arc::new(
                    encoded
                        .iter()
                        .map(|part| Some(part.as_slice()))
                        .collect::<BinaryArray>(),
                ));
            } else {
                columns.push(
                    plenora_core::arrow::select::take::take(column, &take_indices, None)
                        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
                );
            }
        }
        columns.push(std::sync::Arc::new(UInt64Array::from(parents)));
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
        row_offset += batch.num_rows() as u64;
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// Operazioni N:1 (`dissolve`, `line_builder`, `polygon_builder`): tutto
/// l'input produce una sola riga; il grouping resta nell'adapter. Le colonne
/// attributo non sono propagate (l'aggregazione e' un compito dell'adapter):
/// l'output contiene solo la colonna geometria. Input senza geometrie non
/// null (o punti insufficienti per i builder) produce una riga con geometria
/// null. I null sono ignorati dai builder, che mantengono l'ordine righe.
fn collect_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let operation = params.operation;
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut geometries: Vec<Option<Geometry<f64>>> = Vec::new();
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for cell in cells {
            match cell {
                None => geometries.push(None),
                Some(payload) => {
                    if payload.len() as u64 > MAX_CELL_BYTES {
                        return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
                    }
                    geometries.push(Some(geometry_from_wkb(payload)?));
                }
            }
        }
    }
    let result: Option<Geometry<f64>> = match operation {
        ArrowOperation::Dissolve => {
            let polygons: Vec<Geometry<f64>> = geometries.into_iter().flatten().collect();
            if polygons.is_empty() {
                None
            } else {
                Some(dissolve(&polygons)?)
            }
        }
        ArrowOperation::LineBuilder => line_from_ordered_points(&geometries)?,
        ArrowOperation::PolygonBuilder => polygon_from_ordered_points(&geometries)?,
        _ => {
            return Err(ArrowTransportError::Internal(
                "collect_batches: operazione non N:1",
            ))
        }
    };
    let limit = params.max_output_rows_limit();
    if limit == 0 {
        return Err(ArrowTransportError::OutputRowsExceeded { actual: 1, limit });
    }
    let value = result
        .map(|geometry| encode_geometry(&geometry))
        .transpose()?;
    let output_schema = std::sync::Arc::new(Schema::new_with_metadata(
        vec![geometry_output_field(geometry_column, output_crs)?],
        schema.metadata().clone(),
    ));
    let batch = RecordBatch::try_new(
        output_schema.clone(),
        vec![std::sync::Arc::new(BinaryArray::from_iter([
            value.as_deref()
        ]))],
    )
    .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    Ok((output_schema, vec![batch]))
}

/// `voronoi` (collettiva): i punti non-null producono una cella a testa
/// nell'ordine input; le righe null restano null e le posizioni riga sono
/// preservate. Il kernel impone il cap punti (`max_points`) e rifiuta input
/// non puntuali o meno di due punti.
fn voronoi_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let max_points =
        usize::try_from(params.max_points.unwrap_or(DEFAULT_MAX_POINTS)).map_err(|_| {
            ArrowTransportError::InvalidParameter {
                operation: params.operation.name(),
                name: "max_points",
                reason: "non rappresentabile su questa piattaforma",
            }
        })?;
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut points: Vec<Geometry<f64>> = Vec::new();
    let mut positions: Vec<u64> = Vec::new();
    let mut row_offset = 0_u64;
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for (row, cell) in cells.iter().enumerate() {
            let Some(payload) = cell else {
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            points.push(geometry_from_wkb(payload)?);
            positions.push(row_offset + row as u64);
        }
        row_offset += batch.num_rows() as u64;
    }
    let limit = params.max_output_rows_limit();
    if row_offset > limit {
        return Err(ArrowTransportError::OutputRowsExceeded {
            actual: row_offset,
            limit,
        });
    }
    let cells = voronoi_cells(&points, max_points)?;
    let mut encoded: Vec<Vec<u8>> = Vec::with_capacity(cells.len());
    for cell in &cells {
        encoded.push(encode_geometry(cell)?);
    }

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let mut output_batches = Vec::with_capacity(batches.len());
    let mut cursor = 0_usize;
    let mut row_offset = 0_u64;
    for batch in batches {
        let mut values: Vec<Option<&[u8]>> = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() as u64 {
            if cursor < positions.len() && positions[cursor] == row_offset + row {
                values.push(Some(encoded[cursor].as_slice()));
                cursor += 1;
            } else {
                values.push(None);
            }
        }
        row_offset += batch.num_rows() as u64;
        let mut columns = batch.columns().to_vec();
        columns[geometry_index] = std::sync::Arc::new(BinaryArray::from_iter(values));
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// `clean_topology` (collettiva): cleanup ordinato dell'intera tabella
/// (gap close morfologico e sovrapposizioni first-row-wins). Output allineato
/// all'input: attributi invariati, geometria ripulita; i null restano null e
/// le righe assorbite da una riga precedente diventano null. Input non
/// poligonali o invalidi sono rifiutati (la riparazione spetta a `make_valid`).
fn clean_topology_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let snap_tolerance = params
        .snap_tolerance
        .ok_or_else(|| ArrowTransportError::MissingParameter {
            operation: params.operation.name(),
            name: "snap_tolerance",
        })?;
    let remove_overlaps = params.remove_overlaps.unwrap_or(true);
    let fill_gaps = params.fill_gaps.unwrap_or(true);
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let mut geometries: Vec<Geometry<f64>> = Vec::new();
    let mut positions: Vec<u64> = Vec::new();
    let mut row_offset = 0_u64;
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for (row, cell) in cells.iter().enumerate() {
            let Some(payload) = cell else {
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            geometries.push(geometry_from_wkb(payload)?);
            positions.push(row_offset + row as u64);
        }
        row_offset += batch.num_rows() as u64;
    }
    let cleaned = clean_valid_polygon_topology(
        &geometries,
        snap_tolerance,
        remove_overlaps,
        fill_gaps,
        MAX_ROWS,
        MAX_CLEAN_VERTICES,
    )?;
    let mut encoded: Vec<Option<Vec<u8>>> = Vec::with_capacity(cleaned.len());
    for geometry in &cleaned {
        encoded.push(
            geometry
                .as_ref()
                .map(encode_geometry)
                .transpose()?,
        );
    }

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let mut output_batches = Vec::with_capacity(batches.len());
    let mut cursor = 0_usize;
    let mut row_offset = 0_u64;
    for batch in batches {
        let mut values: Vec<Option<&[u8]>> = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() as u64 {
            if cursor < positions.len() && positions[cursor] == row_offset + row {
                values.push(encoded[cursor].as_deref());
                cursor += 1;
            } else {
                values.push(None);
            }
        }
        row_offset += batch.num_rows() as u64;
        let mut columns = batch.columns().to_vec();
        columns[geometry_index] = std::sync::Arc::new(BinaryArray::from_iter(values));
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// `geometry_diagnostics` (1:1 struct): la colonna geometria e' sostituita
/// da colonne esplicative (`geometry_type`, `coordinate_count`, `is_empty`,
/// `is_finite`, `is_valid`, `validity_reason`, `bounds_minx/miny/maxx/maxy`).
/// A differenza delle altre operazioni l'input OGC-invalido e' ACCETTATO
/// (solo il contratto strutturale WKB e' verificato): diagnosticare geometrie
/// invalide e' lo scopo dell'operazione.
fn diagnostics_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_column = params.geometry_column();
    let geometry_index = geometry_column_index(schema, geometry_column)?;

    let diagnostic_fields = vec![
        Field::new("geometry_type", DataType::Utf8, true),
        Field::new("coordinate_count", DataType::UInt64, true),
        Field::new("is_empty", DataType::Boolean, true),
        Field::new("is_finite", DataType::Boolean, true),
        Field::new("is_valid", DataType::Boolean, true),
        Field::new("validity_reason", DataType::Utf8, true),
        Field::new("bounds_minx", DataType::Float64, true),
        Field::new("bounds_miny", DataType::Float64, true),
        Field::new("bounds_maxx", DataType::Float64, true),
        Field::new("bounds_maxy", DataType::Float64, true),
    ];
    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields.splice(
        geometry_index..=geometry_index,
        diagnostic_fields.iter().cloned(),
    );
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let mut output_batches = Vec::with_capacity(batches.len());
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        let mut geometry_type: Vec<Option<String>> = Vec::with_capacity(batch.num_rows());
        let mut coordinate_count: Vec<Option<u64>> = Vec::with_capacity(batch.num_rows());
        let mut is_empty: Vec<Option<bool>> = Vec::with_capacity(batch.num_rows());
        let mut is_finite: Vec<Option<bool>> = Vec::with_capacity(batch.num_rows());
        let mut is_valid: Vec<Option<bool>> = Vec::with_capacity(batch.num_rows());
        let mut validity_reason: Vec<Option<String>> = Vec::with_capacity(batch.num_rows());
        let mut bounds: Vec<Option<[f64; 4]>> = Vec::with_capacity(batch.num_rows());
        for cell in cells {
            let Some(payload) = cell else {
                geometry_type.push(None);
                coordinate_count.push(None);
                is_empty.push(None);
                is_finite.push(None);
                is_valid.push(None);
                validity_reason.push(None);
                bounds.push(None);
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            plenora_kernels_geo::validate_wkb_contract(payload)?;
            let geometry = Wkb(payload)
                .to_geo()
                .map_err(|error| ArrowTransportError::Geometry(format!("WKB non valido: {error}")))?;
            let diagnostics = geometry_diagnostics(&geometry)?;
            geometry_type.push(Some(diagnostics.geometry_type.to_owned()));
            coordinate_count.push(Some(diagnostics.coordinate_count));
            is_empty.push(Some(diagnostics.is_empty));
            is_finite.push(Some(diagnostics.is_finite));
            is_valid.push(Some(diagnostics.is_valid));
            validity_reason.push(diagnostics.validity_reason);
            bounds.push(diagnostics.bounds);
        }
        let mut columns = batch.columns().to_vec();
        let diagnostic_columns: Vec<plenora_core::arrow::array::ArrayRef> = vec![
            std::sync::Arc::new(StringArray::from(geometry_type)),
            std::sync::Arc::new(UInt64Array::from(coordinate_count)),
            std::sync::Arc::new(plenora_core::arrow::array::BooleanArray::from(is_empty)),
            std::sync::Arc::new(plenora_core::arrow::array::BooleanArray::from(is_finite)),
            std::sync::Arc::new(plenora_core::arrow::array::BooleanArray::from(is_valid)),
            std::sync::Arc::new(StringArray::from(validity_reason)),
            std::sync::Arc::new(Float64Array::from(
                bounds.iter().map(|b| b.map(|b| b[0])).collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                bounds.iter().map(|b| b.map(|b| b[1])).collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                bounds.iter().map(|b| b.map(|b| b[2])).collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                bounds.iter().map(|b| b.map(|b| b[3])).collect::<Vec<_>>(),
            )),
        ];
        columns.splice(geometry_index..=geometry_index, diagnostic_columns);
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// Raccoglie tutte le celle non-null dell'intera tabella in una
/// `GeometryCollection` per i kernel collettivi (`polygonize`, `line_merge`).
fn collect_linework(
    batches: &[RecordBatch],
    geometry_index: usize,
    geometry_column: &str,
) -> Result<Geometry<f64>, ArrowTransportError> {
    let mut lines = Vec::new();
    for batch in batches {
        let cells = batch_geometry_cells(batch, geometry_index, geometry_column)?;
        for cell in cells {
            let Some(payload) = cell else {
                continue;
            };
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
            }
            lines.push(geometry_from_wkb(payload)?);
        }
    }
    Ok(Geometry::GeometryCollection(lines.into()))
}

/// Output di sole geometrie (una riga per pezzo) per i kernel collettivi,
/// con colonna di classificazione opzionale.
fn geometry_rows_output(
    schema: &SchemaRef,
    geometry_column: &str,
    output_crs: &str,
    rows: &[(Option<Vec<u8>>, Option<&'static str>)],
    with_class: bool,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let mut fields = vec![geometry_output_field(geometry_column, output_crs)?];
    if with_class {
        fields.push(Field::new(CLASS_COLUMN, DataType::Utf8, false));
    }
    let output_schema =
        std::sync::Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));
    let mut columns: Vec<plenora_core::arrow::array::ArrayRef> = vec![std::sync::Arc::new(
        rows.iter().map(|row| row.0.as_deref()).collect::<BinaryArray>(),
    )];
    if with_class {
        let classes: Vec<&'static str> = rows
            .iter()
            .map(|row| row.1.ok_or(ArrowTransportError::Internal("classe mancante")))
            .collect::<Result<_, _>>()?;
        columns.push(std::sync::Arc::new(StringArray::from(classes)));
    }
    let batch = RecordBatch::try_new(output_schema.clone(), columns)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
    Ok((output_schema, vec![batch]))
}

/// `polygonize` (collettiva, richiede `geos-backend`): tutte le linee
/// non-null dell'input sono nodate e poligonizzate; l'output contiene una
/// riga per poligono e per residuo, classificati in `__class`
/// (`polygon`/`cut_edge`/`dangle`/`invalid_ring`). Nessun attributo
/// propagato, come per `dissolve`.
#[cfg(feature = "geos-backend")]
fn polygonize_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;
    let linework = collect_linework(batches, geometry_index, geometry_column)?;
    let result = polygonize_linework(
        &linework,
        params.node_input.unwrap_or(true),
        params.require_complete.unwrap_or(false),
        MAX_CLEAN_VERTICES,
        MAX_NODING_WORK,
        params.max_output_rows_limit(),
        MAX_CLEAN_VERTICES,
    )?;
    let mut rows: Vec<(Option<Vec<u8>>, Option<&'static str>)> = Vec::new();
    for polygon in &result.polygons {
        rows.push((
            Some(encode_geometry(&Geometry::Polygon(polygon.clone()))?),
            Some("polygon"),
        ));
    }
    for (class, lines) in [
        ("cut_edge", &result.cut_edges),
        ("dangle", &result.dangles),
        ("invalid_ring", &result.invalid_ring_lines),
    ] {
        for line in lines {
            rows.push((
                Some(encode_geometry(&Geometry::LineString(line.clone()))?),
                Some(class),
            ));
        }
    }
    geometry_rows_output(schema, geometry_column, output_crs, &rows, true)
}

#[cfg(not(feature = "geos-backend"))]
const fn polygonize_batches(
    _schema: &SchemaRef,
    _batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    Err(ArrowTransportError::BackendUnavailable {
        operation: params.operation.name(),
        feature: "geos-backend",
    })
}

/// `line_merge` (collettiva): le linee non-null dell'intera tabella sono
/// mergiate nei cammini massimali (barriera ai nodi di grado diverso da 2);
/// output di sole geometrie, una riga per linea mergiata.
fn line_merge_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    let geometry_index = geometry_column_index(schema, geometry_column)?;
    let linework = collect_linework(batches, geometry_index, geometry_column)?;
    let merged = line_merge(
        &linework,
        MAX_CLEAN_VERTICES,
        params.max_output_rows_limit(),
    )?;
    let mut rows: Vec<(Option<Vec<u8>>, Option<&'static str>)> = Vec::new();
    for line in &merged {
        rows.push((
            Some(encode_geometry(&Geometry::LineString(line.clone()))?),
            None,
        ));
    }
    geometry_rows_output(schema, geometry_column, output_crs, &rows, false)
}

/// Estrae i valori di una colonna numerica come float64 opzionali.
fn numeric_values(
    batch: &RecordBatch,
    index: usize,
    name: &str,
) -> Result<Vec<Option<f64>>, ArrowTransportError> {
    let column = batch.column(index);
    match column.data_type() {
        DataType::Float64 => Ok(column
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or(ArrowTransportError::Internal("tipo verificato Float64"))?
            .iter()
            .collect()),
        DataType::Int64 => {
            // Guardia di range (R5.4): oltre 2^53 in valore assoluto la
            // conversione i64 -> f64 non e' esatta e sposterebbe la
            // coordinata in silenzio; si rifiuta la riga con errore
            // tipizzato invece di produrre una geometria imprecisa.
            const MAX_EXACT: u64 = 1_u64 << 53;
            // Esattezza garantita dalla guardia: qui |x| <= 2^53, quindi
            // ogni i64 ammesso ha un f64 esattamente uguale.
            #[allow(clippy::cast_precision_loss)]
            let exact = |x: i64| -> Result<f64, ArrowTransportError> {
                if x.unsigned_abs() > MAX_EXACT {
                    Err(ArrowTransportError::IntegerCoordinateTooLarge {
                        name: name.to_owned(),
                    })
                } else {
                    Ok(x as f64)
                }
            };
            column
                .as_any()
                .downcast_ref::<plenora_core::arrow::array::Int64Array>()
                .ok_or(ArrowTransportError::Internal("tipo verificato Int64"))?
                .iter()
                .map(|value| value.map(exact).transpose())
                .collect()
        }
        other => Err(ArrowTransportError::ColumnNotNumeric {
            name: name.to_owned(),
            actual: other.to_string(),
        }),
    }
}

/// `from_coords` (1:1 senza colonna geometria in input): due colonne
/// numeriche (default `x`/`y`) producono una colonna geometria Point
/// aggiunta in coda; null in x o y -> geometria null; coordinate non finite
/// sono rifiutate dal kernel (fail-closed). Tutte le colonne di input
/// passano invariate.
fn from_coords_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_column = params.geometry_column();
    let output_crs = params
        .crs
        .as_deref()
        .ok_or(ArrowTransportError::CrsRequired)?;
    if schema.column_with_name(geometry_column).is_some() {
        return Err(ArrowTransportError::OutputColumnExists(
            geometry_column.to_owned(),
        ));
    }
    let (x_index, x_field) = schema
        .column_with_name(params.x_column())
        .ok_or_else(|| ArrowTransportError::MissingColumn(params.x_column().to_owned()))?;
    let (y_index, y_field) = schema
        .column_with_name(params.y_column())
        .ok_or_else(|| ArrowTransportError::MissingColumn(params.y_column().to_owned()))?;
    for (name, field) in [(params.x_column(), x_field), (params.y_column(), y_field)] {
        if !matches!(field.data_type(), DataType::Float64 | DataType::Int64) {
            return Err(ArrowTransportError::ColumnNotNumeric {
                name: name.to_owned(),
                actual: field.data_type().to_string(),
            });
        }
    }

    let mut output_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields.push(geometry_output_field(geometry_column, output_crs)?);
    let output_schema = Schema::new_with_metadata(output_fields, schema.metadata().clone());

    let limit = params.max_output_rows_limit();
    let mut output_batches = Vec::with_capacity(batches.len());
    for batch in batches {
        if batch.num_rows() as u64 > limit {
            return Err(ArrowTransportError::OutputRowsExceeded {
                actual: batch.num_rows() as u64,
                limit,
            });
        }
        let xs = numeric_values(batch, x_index, params.x_column())?;
        let ys = numeric_values(batch, y_index, params.y_column())?;
        let mut points: Vec<Option<Vec<u8>>> = Vec::with_capacity(batch.num_rows());
        for (x, y) in xs.into_iter().zip(ys) {
            match (x, y) {
                (Some(x), Some(y)) => {
                    let point = point_from_lon_lat(x, y)?;
                    points.push(Some(encode_geometry(&point)?));
                }
                _ => points.push(None),
            }
        }
        let mut columns = batch.columns().to_vec();
        columns.push(std::sync::Arc::new(
            points
                .iter()
                .map(|point| point.as_deref())
                .collect::<BinaryArray>(),
        ));
        output_batches.push(
            RecordBatch::try_new(std::sync::Arc::new(output_schema.clone()), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// Pipeline completa envelope -> Arrow -> kernel -> Arrow -> envelope.
/// Il CRS e' trattato come metadato opaco: la verifica semantica spetta al
/// livello comandi.
///
/// # Errors
///
/// `ArrowTransportError::UnsupportedSchemaVersion` per versioni non
/// supportate, `ArrowTransportError::TooManyRows` / `CrsRequired` /
/// `RowCountMismatch` per violazioni del contratto di schema; propaga gli
/// errori di envelope (`EnvelopeReader`/`EnvelopeWriter`), di decodifica
/// (`decode_ipc`), di trasformazione (`transform_batches`) e di codifica
/// (`encode_ipc`).
pub fn transform_arrow(
    reader: impl Read,
    writer: impl Write,
    schema: &TransformArrowSchema,
) -> Result<TransformArrowSummary, ArrowTransportError> {
    if schema.schema_version != TransformArrowSchema::VERSION {
        return Err(ArrowTransportError::UnsupportedSchemaVersion(
            schema.schema_version,
        ));
    }
    if schema.row_count > MAX_ROWS {
        return Err(ArrowTransportError::TooManyRows(schema.row_count));
    }
    schema.validate_parameters()?;
    if schema.crs.is_none() {
        return Err(ArrowTransportError::CrsRequired);
    }

    let payload = EnvelopeReader::new(reader)?.read_payload()?;
    let (input_schema, batches) = decode_ipc(&payload)?;
    let rows: u64 = batches.iter().map(|batch| batch.num_rows() as u64).sum();
    if rows != schema.row_count {
        return Err(ArrowTransportError::RowCountMismatch {
            schema: schema.row_count,
            stream: rows,
        });
    }

    let (output_schema, output_batches) = transform_batches(&input_schema, &batches, schema)?;
    let output_rows: u64 = output_batches
        .iter()
        .map(|batch| batch.num_rows() as u64)
        .sum();
    let output_payload = encode_ipc(&output_schema, &output_batches)?;
    let mut envelope = EnvelopeWriter::new(writer, output_payload.len() as u64)?;
    envelope.write_payload(&output_payload)?;
    let (_, checksum) = envelope.finish()?;
    Ok(TransformArrowSummary {
        rows,
        output_rows,
        checksum,
    })
}

