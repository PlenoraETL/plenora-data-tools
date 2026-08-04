//! Macchina delle operazioni unary del trasporto Arrow v3: dispatch per
//! forma (1:1, 1:N, N:1, collettiva, diagnostica, da coordinate), handle
//! prepared e pipeline `transform_arrow` completa.

use std::io::{Read, Write};

use geo::Geometry;
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{
    Array, BinaryArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use rayon::prelude::*;

use super::protocol::MAX_ROWS;
use plenora_core::contract::{CrsResolution, GeometryDimensions, GeometryEncoding};
use plenora_core::crs::MAX_CRS_DEFINITION_BYTES;
use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::PlenoraError;
use plenora_kernels_geo::advanced::voronoi_cells;
use plenora_kernels_geo::construction::{
    line_from_ordered_points, point_from_lon_lat, polygon_from_ordered_points,
};
use plenora_kernels_geo::extended::{
    affine_transform_validated, concave_hull_validated, geodesic_line_length_m,
    rotate_about_validated, scale_about_validated, translate_validated,
};
use plenora_kernels_geo::extended_algorithms::{
    delaunay, densify, geodesic_area_m2, geometry_diagnostics, line_interpolate_point, line_merge,
    line_substring, snap_to_grid,
};
use plenora_kernels_geo::geometry_contract::{validate_geometry_structural, wkb_size_xy};
#[cfg(feature = "geos-backend")]
use plenora_kernels_geo::geos_backend::{
    make_valid_geometry, make_valid_wkb, polygonize_linework, RepairMethod,
};
use plenora_kernels_geo::operations::{
    area, boundary, bounds, buffer_with_cap, explode, length, perimeter, point_on_surface,
    simplify_with_policy, to_wkt, vertex_count, BufferCapStyle, OperationError, SimplifyPolicy,
};
use plenora_kernels_geo::predicates::SpatialPredicate;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::proj_backend::Reprojector;
use plenora_kernels_geo::topology::{clean_valid_polygon_topology_validated, dissolve_validated};
use plenora_kernels_geo::{
    check_geometry_valid, geometry_from_wkb, transform_geometry_canonical, transform_wkb,
    Operation, MAX_WKB_COMPONENTS, MAX_WKB_DEPTH,
};

use super::envelope::{EnvelopeReader, EnvelopeWriter};
use super::error::ArrowTransportError;
use super::ipc::{decode_ipc, encode_ipc};
use super::schema::{
    ArrowOperation, ArrowShape, BufferCap, SimplifyPolicyParam, TransformArrowSchema,
    TransformArrowSummary,
};
#[cfg(feature = "geos-backend")]
use super::transport::MAX_NODING_WORK;
use super::transport::{
    CLASS_COLUMN, DEFAULT_MAX_POINTS, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
    GEO_METADATA_KEY, MAX_CELL_BYTES, MAX_CELL_COORDINATES, MAX_CLEAN_VERTICES,
    PARENT_INDEX_COLUMN, WKT_COLUMN,
};
use plenora_kernels_geo::arrow_adapter::{
    canonical_geometry_metadata_for_resolved_definition, field_declares_wkb_geometry,
    GeometryMetadataDetails, PLENORA_CONTRACT_VERSION, PLENORA_CONTRACT_VERSION_KEY,
    PLENORA_GEOMETRY_NAMESPACE_PREFIX,
};

#[cfg(feature = "proj-backend")]
std::thread_local! {
    /// Pipeline PROJ per thread: `Reprojector` non e' `Sync`, quindi ogni
    /// thread rayon costruisce la sua una sola volta per coppia CRS.
    static REPROJECTOR: std::cell::RefCell<Option<(String, String, Reprojector)>> =
        const { std::cell::RefCell::new(None) };
}

pub(in crate::geo_transport) fn geometry_column_index(
    schema: &Schema,
    name: &str,
) -> Result<usize, ArrowTransportError> {
    let (index, field) = schema
        .column_with_name(name)
        .ok_or_else(|| ArrowTransportError::MissingGeometryColumn(name.to_owned()))?;
    if field.data_type() != &DataType::Binary {
        return Err(ArrowTransportError::GeometryColumnNotBinary {
            name: name.to_owned(),
            actual: field.data_type().to_string(),
        });
    }
    // ADR-0009 decisione 8: estensione `geoarrow.wkb` OPPURE sole chiavi
    // canoniche — criterio condiviso con analyze (stessa funzione), cosi' il
    // rifiuto a compile-plan e l'accettazione a esecuzione non possono
    // divergere. Piani validati non arrivano mai qui con una colonna non
    // identificabile: il check resta come difesa in profondita'.
    if !field_declares_wkb_geometry(field) {
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
pub(in crate::geo_transport) fn geo_metadata_json(
    crs: &str,
) -> Result<String, ArrowTransportError> {
    if crs.trim().is_empty() {
        return Err(ArrowTransportError::CrsRequired);
    }
    if crs.len() > MAX_CRS_DEFINITION_BYTES {
        return Err(ArrowTransportError::CrsTooLarge);
    }
    plenora_kernels_geo::arrow_adapter::geo_metadata_json(crs)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

pub(in crate::geo_transport) fn geometry_output_field(
    name: &str,
    crs: &str,
) -> Result<Field, ArrowTransportError> {
    // Validazione CRS con le varianti strutturate del trasporto; la
    // costruzione del campo (metadati geoarrow.wkb + geo.crs +
    // geo.dimensions) e' unica in `arrow_adapter` (unificazione B1.1).
    // BLOCK-06: il blocco canonico `plenora.geometry.*` NON e' aggiunto qui
    // ma nel post-processo centrale `canonical_legacy_output` (entry point
    // `transform_arrow`/`pair_arrow`), che copre anche i campi propagati
    // invariati dalle op pass-through.
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

// ---------------------------------------------------------------------------
// BLOCK-06 (decisione owner 2026-07-30 — parita' col percorso v4, DER-002
// estesa): doppia emissione delle chiavi canoniche `plenora.geometry.*` e
// `plenora.contract.version` anche sugli output del trasporto legacy.
// ---------------------------------------------------------------------------

/// Campo di output arricchito del blocco canonico R2.2 (BLOCK-06).
///
/// Regole (stesse del post-processo v4 `canonical_output_schema`):
///
/// - solo i campi con estensione `geoarrow.wkb` sono colonne geometriche:
///   ogni altro campo e' restituito invariato;
/// - un campo che porta GIA' chiavi canoniche (lineage dal produttore, es.
///   input dal percorso v4 propagato invariato dalle op pass-through come
///   `within`/`count`) le conserva invariate — R2.4: il trasporto non
///   interpreta le chiavi canoniche, le propaga; la coerenza R2.6 con il
///   `geo.crs` era responsabilita' del produttore;
/// - altrimenti il blocco e' derivato dal metadato legacy `geo` del campo
///   stesso (la dichiarazione che il trasporto ha sempre emesso/propagato):
///   `geo.crs` → stato `resolved` con la stessa definizione (forma v4,
///   [`canonical_geometry_metadata_for_resolved_definition`]), `geo.dimensions`
///   → `dimensions` (assente → `unknown`, R3.4: i campi pass-through non
///   ricodificano le celle, dichiarare `xy` sarebbe inventare),
///   `geo.encoding` → `encoding` solo se dichiarato (R5.2);
/// - nessuna dichiarazione CRS (`geo.crs` assente o vuota) →
///   `crs_resolution = missing` senza chiavi CRS (R4.6.3/R4.6.4: lo stato
///   mancante si propaga invariato, mai un CRS inventato — R4.4);
/// - i tipi NON sono mai emessi (il trasporto non li dichiara, R3.4.1).
///
/// La derivazione non introduce rifiuti nuovi sui metadati di lineage: un
/// `geo` malformato resta propagato com'e' (la sua lettura fail-closed e'
/// della discovery v4, non del trasporto legacy).
fn canonical_legacy_field(field: &Field) -> Field {
    if field
        .metadata()
        .get(GEOARROW_EXTENSION_KEY)
        .map(String::as_str)
        != Some(GEOARROW_WKB_EXTENSION)
    {
        return field.clone();
    }
    if field
        .metadata()
        .keys()
        .any(|key| key.starts_with(PLENORA_GEOMETRY_NAMESPACE_PREFIX))
    {
        return field.clone();
    }
    let geo = field
        .metadata()
        .get(GEO_METADATA_KEY)
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let crs = geo
        .as_ref()
        .and_then(|value| value.get("crs"))
        .and_then(|value| match value {
            serde_json::Value::String(definition) => Some(definition.clone()),
            object @ serde_json::Value::Object(_) => serde_json::to_string(object).ok(),
            _ => None,
        })
        .filter(|definition| !definition.trim().is_empty());
    let dimensions = geo
        .as_ref()
        .and_then(|value| value.get("dimensions"))
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| raw.parse::<GeometryDimensions>().ok())
        .unwrap_or(GeometryDimensions::Unknown);
    let encoding = geo
        .as_ref()
        .and_then(|value| value.get("encoding"))
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| raw.parse::<GeometryEncoding>().ok());
    let canonical = crs.map_or_else(
        || {
            // R4.6.3: nessuna dichiarazione CRS → `missing`, nessuna chiave
            // CRS (R2.2: `missing` non ammette `crs_id`/`crs_definition`/
            // `srid`/`axis_order`).
            let mut metadata = std::collections::HashMap::new();
            if let Some(encoding) = encoding {
                metadata.insert(
                    plenora_kernels_geo::arrow_adapter::PLENORA_GEOMETRY_ENCODING_KEY.to_owned(),
                    encoding.as_str().to_owned(),
                );
            }
            metadata.insert(
                plenora_kernels_geo::arrow_adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(),
                dimensions.as_str().to_owned(),
            );
            metadata.insert(
                plenora_kernels_geo::arrow_adapter::PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
                CrsResolution::Missing.as_str().to_owned(),
            );
            metadata
        },
        |definition| {
            canonical_geometry_metadata_for_resolved_definition(
                &definition,
                dimensions,
                encoding,
                &GeometryMetadataDetails::default(),
            )
        },
    );
    let mut metadata = field.metadata().clone();
    metadata.extend(canonical);
    field.clone().with_metadata(metadata)
}

/// Post-processo CENTRALE della doppia emissione BLOCK-06 sugli output del
/// trasporto legacy: arricchisce ogni campo geometria del blocco canonico
/// R2.2 ([`canonical_legacy_field`]) e aggiunge la versione di protocollo
/// R2.5 (`plenora.contract.version`) ai metadati dello schema, poi riveste i
/// batch col nuovo schema (stesso schema dei batch, mai solo dell'header —
/// come `canonical_output_schema` nel percorso v4).
///
/// Punto unico di applicazione: gli entry point `transform_arrow` e
/// `pair_arrow`, subito prima di `encode_ipc`. Un output senza colonne
/// geometriche canoniche (lineage di coppie, `lineage_schema`) e'
/// restituito invariato, versione compresa (R2.5: la versione accompagna le
/// chiavi canoniche, mai da sola).
///
/// # Errors
///
/// `ArrowTransportError::Arrow` se la chiave `plenora.contract.version` e'
/// gia' presente con un valore diverso da quello corrente (R2.6: il
/// componente fallisce, non sovrascrive) o se il rivestimento dei batch
/// fallisce.
pub(in crate::geo_transport) fn canonical_legacy_output(
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let mut canonical_present = false;
    let mut fields = Vec::with_capacity(schema.fields().len());
    for field in schema.fields() {
        let field = canonical_legacy_field(field);
        canonical_present |= field
            .metadata()
            .keys()
            .any(|key| key.starts_with(PLENORA_GEOMETRY_NAMESPACE_PREFIX));
        fields.push(field);
    }
    if !canonical_present {
        return Ok((schema, batches));
    }
    let mut schema_metadata = schema.metadata().clone();
    match schema_metadata.get(PLENORA_CONTRACT_VERSION_KEY) {
        Some(existing) if existing != &PLENORA_CONTRACT_VERSION.to_string() => {
            return Err(ArrowTransportError::Arrow(format!(
                "chiave `{PLENORA_CONTRACT_VERSION_KEY}` dello schema gia' presente con un \
                 valore diverso (R2.6: il componente fallisce, non sovrascrive)"
            )));
        }
        Some(_) => {}
        None => {
            schema_metadata.insert(
                PLENORA_CONTRACT_VERSION_KEY.to_owned(),
                PLENORA_CONTRACT_VERSION.to_string(),
            );
        }
    }
    let output_schema = std::sync::Arc::new(Schema::new_with_metadata(fields, schema_metadata));
    let batches = batches
        .into_iter()
        .map(|batch| {
            RecordBatch::try_new(output_schema.clone(), batch.columns().to_vec())
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((output_schema, batches))
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
pub(in crate::geo_transport) fn encode_geometry(
    geometry: &Geometry<f64>,
) -> Result<Vec<u8>, ArrowTransportError> {
    let payload = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| OperationError::InvalidOutput(error.to_string()))?;
    if payload.len() as u64 > MAX_CELL_BYTES {
        return Err(ArrowTransportError::CellTooLarge(payload.len() as u64));
    }
    Ok(payload)
}

/// Parametri risolti di una trasformazione 1:1 fondibile (ADR-0012,
/// perimetro M1+M3): l'estrazione/validazione dei parametri avviene UNA
/// volta per kernel ([`resolve_transform`]), prima di toccare le celle —
/// stessa posizione e stessi errori del braccio corrispondente di
/// `transform_cells`, anche a batch vuoto.
enum ResolvedTransform {
    /// Profilo A (D12.4): `centroid`/`convex_hull`/`envelope` — pipeline
    /// canonica di `transform_wkb` su geometria decodificata (kernel + OGC
    /// in uscita + limite 64 MiB + validazione strutturale, tutto attribuito
    /// al nodo).
    Canonical(Operation),
    Buffer {
        distance: f64,
        cap: BufferCapStyle,
    },
    Simplify {
        tolerance: f64,
        policy: SimplifyPolicy,
    },
    Boundary,
    PointOnSurface,
    AffineTransform {
        coefficients: [f64; 6],
    },
    Translate {
        x: f64,
        y: f64,
    },
    Scale {
        x_factor: f64,
        y_factor: f64,
        origin: geo::Point<f64>,
    },
    Rotate {
        degrees: f64,
        origin: geo::Point<f64>,
    },
    ConcaveHull {
        concavity: f64,
        length_threshold: f64,
    },
    Densify {
        max_segment_length: f64,
    },
    SnapToGrid {
        grid_size: f64,
    },
    /// `make_valid` (ADR-0012 M3): ammette input OGC-invalido — e' cio' che
    /// ripara; la validazione che lo precede e' SOLO strutturale (trappola
    /// 1, vedi [`accepts_ogc_invalid_input`]).
    #[cfg(feature = "geos-backend")]
    MakeValid,
    /// `reproject` (ADR-0012 M3): la coppia CRS e' risolta una volta per
    /// kernel, come l'estrazione in testa al braccio di `transform_cells`;
    /// la pipeline PROJ resta thread-local nel passo per-cella.
    #[cfg(feature = "proj-backend")]
    Reproject {
        source: String,
        target: String,
    },
}

/// L'operazione ammette input OGC-invalido in ingresso? Solo `make_valid`
/// (ADR-0012 M3, trappola 1): nel percorso non fuso il suo "decode" e' il
/// SOLO gate strutturale di `make_valid_wkb` (`validate_wkb_contract`,
/// nessun check OGC — l'input invalido e' esattamente cio' che l'operazione
/// ripara). Il runner fuso riproduce la stessa semantica: decode iniziale
/// del gruppo (se `make_valid` lo apre) e validazione inter-passo davanti a
/// un nodo `make_valid` sono SOLO strutturali, mai OGC. Eccezione speculare
/// a `geometry_diagnostics` (che valuta la validita' come dato).
const fn accepts_ogc_invalid_input(params: &TransformArrowSchema) -> bool {
    matches!(params.operation, ArrowOperation::MakeValid)
}

/// Risolve i parametri di un'operazione fondibile: stesse estrazioni (e
/// stessi errori) dei bracci di `transform_cells`, eseguite una sola volta.
///
/// # Errors
///
/// Come il braccio corrispondente di `transform_cells` per la parte
/// parametri; `ArrowTransportError::Internal` per operazioni fuori dal
/// perimetro fondibile (mai raggiungibile: i gruppi sono annotati da
/// `prepare` solo sulle op del perimetro ADR-0012 — difesa in profondita',
/// non un caso d'uso). A feature spenta `make_valid`/`reproject` danno
/// `BackendUnavailable` esattamente come i bracci non fusi (M3).
fn resolve_transform(
    params: &TransformArrowSchema,
) -> Result<ResolvedTransform, ArrowTransportError> {
    match params.operation {
        ArrowOperation::Centroid | ArrowOperation::ConvexHull | ArrowOperation::Envelope => {
            let kernel =
                params
                    .operation
                    .geometry_kernel()
                    .ok_or(ArrowTransportError::Internal(
                        "operazione geometrica senza kernel",
                    ))?;
            Ok(ResolvedTransform::Canonical(kernel))
        }
        ArrowOperation::Buffer => Ok(ResolvedTransform::Buffer {
            distance: params.required_distance()?,
            cap: BufferCapStyle::from(params.cap.unwrap_or(BufferCap::Round)),
        }),
        ArrowOperation::Simplify => Ok(ResolvedTransform::Simplify {
            tolerance: params.required_tolerance()?,
            policy: SimplifyPolicy::from(
                params
                    .simplify_policy
                    .unwrap_or(SimplifyPolicyParam::DouglasPeucker),
            ),
        }),
        ArrowOperation::Boundary => Ok(ResolvedTransform::Boundary),
        ArrowOperation::PointOnSurface => Ok(ResolvedTransform::PointOnSurface),
        ArrowOperation::AffineTransform => {
            let coefficients: [f64; 6] = params
                .coefficients
                .as_deref()
                .ok_or(ArrowTransportError::Internal(
                    "coefficients validato assente",
                ))?
                .try_into()
                .map_err(|_| {
                    ArrowTransportError::Internal("coefficients validato non di 6 elementi")
                })?;
            Ok(ResolvedTransform::AffineTransform { coefficients })
        }
        ArrowOperation::Translate => Ok(ResolvedTransform::Translate {
            x: params.required_f64("x_offset", params.x_offset)?,
            y: params.required_f64("y_offset", params.y_offset)?,
        }),
        ArrowOperation::Scale => Ok(ResolvedTransform::Scale {
            x_factor: params.required_f64("x_factor", params.x_factor)?,
            y_factor: params.required_f64("y_factor", params.y_factor)?,
            origin: geo::Point::new(
                params.x_origin.unwrap_or(0.0),
                params.y_origin.unwrap_or(0.0),
            ),
        }),
        ArrowOperation::Rotate => Ok(ResolvedTransform::Rotate {
            degrees: params.required_f64("degrees", params.degrees)?,
            origin: geo::Point::new(
                params.x_origin.unwrap_or(0.0),
                params.y_origin.unwrap_or(0.0),
            ),
        }),
        ArrowOperation::ConcaveHull => Ok(ResolvedTransform::ConcaveHull {
            concavity: params.required_f64("concavity", params.concavity)?,
            length_threshold: params.length_threshold.unwrap_or(0.0),
        }),
        ArrowOperation::Densify => Ok(ResolvedTransform::Densify {
            max_segment_length: params
                .required_f64("max_segment_length", params.max_segment_length)?,
        }),
        ArrowOperation::SnapToGrid => Ok(ResolvedTransform::SnapToGrid {
            grid_size: params.required_f64("grid_size", params.grid_size)?,
        }),
        #[cfg(feature = "geos-backend")]
        ArrowOperation::MakeValid => Ok(ResolvedTransform::MakeValid),
        // A feature spenta: stesso esito del braccio non fuso
        // (`BackendUnavailable`, senza toccare i parametri — stesso ordine).
        #[cfg(not(feature = "geos-backend"))]
        ArrowOperation::MakeValid => Err(ArrowTransportError::BackendUnavailable {
            operation: params.operation.name(),
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
            Ok(ResolvedTransform::Reproject { source, target })
        }
        #[cfg(not(feature = "proj-backend"))]
        ArrowOperation::Reproject => Err(ArrowTransportError::BackendUnavailable {
            operation: params.operation.name(),
            feature: "proj-backend",
        }),
        _ => Err(ArrowTransportError::Internal(
            "operazione fuori dal perimetro fondibile (ADR-0012)",
        )),
    }
}

/// Applica una trasformazione fondibile a UNA geometria decodificata
/// (ADR-0012): gli stessi kernel dei bracci di `transform_cells`, chiamati
/// con gli stessi argomenti — usata sia dal percorso nodo-per-nodo sia dal
/// runner fuso, perche' il dispatch per singolo kernel resti unico. `None`
/// per gli output vuoti ammessi (`point_on_surface` di geometria vuota), che
/// diventano celle null come nel percorso non fuso.
///
/// La geometria in ingresso e' SEMPRE gia' validata OGC (per costruzione:
/// `geometry_from_wkb` al decode per-cella, ovvero decode iniziale +
/// validazione inter-passo `check_geometry_valid` nel runner fuso —
/// l'unica eccezione e' `make_valid`, che ammette input invalido per
/// contratto e non ha gate): per questo i kernel con gate di ingresso nel
/// perimetro dello scoping binari sono chiamati nelle varianti
/// `*_validated` (R0.1), mentre i restanti kernel mantengono il proprio
/// gate (fuori perimetro, nessuna inferenza).
///
/// Per il profilo A l'applicazione include la pipeline di validazione
/// canonica di `transform_wkb` (OGC in uscita, limite 64 MiB, strutturale):
/// gli errori sono gia' completi e attribuiti al kernel che la invoca.
///
/// # Errors
///
/// Come il braccio corrispondente di `transform_cells` per la parte kernel.
fn apply_transform_cell(
    resolved: &ResolvedTransform,
    geometry: &Geometry<f64>,
) -> Result<Option<Geometry<f64>>, ArrowTransportError> {
    match resolved {
        ResolvedTransform::Canonical(kernel) => {
            Ok(Some(transform_geometry_canonical(*kernel, geometry)?))
        }
        ResolvedTransform::Buffer { distance, cap } => {
            Ok(Some(buffer_with_cap(geometry, *distance, *cap)?))
        }
        ResolvedTransform::Simplify { tolerance, policy } => {
            Ok(Some(simplify_with_policy(geometry, *tolerance, *policy)?))
        }
        ResolvedTransform::Boundary => Ok(Some(boundary(geometry)?)),
        ResolvedTransform::PointOnSurface => Ok(point_on_surface(geometry)?),
        ResolvedTransform::AffineTransform { coefficients } => {
            Ok(Some(affine_transform_validated(geometry, *coefficients)?))
        }
        ResolvedTransform::Translate { x, y } => Ok(Some(translate_validated(geometry, *x, *y)?)),
        ResolvedTransform::Scale {
            x_factor,
            y_factor,
            origin,
        } => Ok(Some(scale_about_validated(
            geometry, *x_factor, *y_factor, *origin,
        )?)),
        ResolvedTransform::Rotate { degrees, origin } => {
            Ok(Some(rotate_about_validated(geometry, *degrees, *origin)?))
        }
        ResolvedTransform::ConcaveHull {
            concavity,
            length_threshold,
        } => Ok(Some(concave_hull_validated(
            geometry,
            *concavity,
            *length_threshold,
            MAX_CELL_COORDINATES,
        )?)),
        ResolvedTransform::Densify { max_segment_length } => Ok(Some(densify(
            geometry,
            *max_segment_length,
            MAX_CELL_COORDINATES,
        )?)),
        ResolvedTransform::SnapToGrid { grid_size } => {
            Ok(Some(snap_to_grid(geometry, *grid_size)?))
        }
        #[cfg(feature = "geos-backend")]
        ResolvedTransform::MakeValid => {
            // Come il braccio non fuso (`make_valid_wkb` sul payload):
            // l'input puo' essere OGC-invalido — nessun check OGC qui; la
            // riparazione e la rivalidazione dell'output sono dentro
            // `make_valid_geometry`, che riusa `make_valid_wkb` sulla
            // stessa forma canonica XY.
            Ok(Some(make_valid_geometry(
                geometry,
                RepairMethod::Linework,
                true,
            )?))
        }
        #[cfg(feature = "proj-backend")]
        ResolvedTransform::Reproject { source, target } => {
            // Una pipeline PROJ per thread (PROJ non e' Sync), riusata su
            // tutte le celle del kernel e ricreata solo se cambia coppia —
            // stesso pattern thread-local del braccio non fuso; le guardie
            // del kernel (input finito/valido, dominio CRS, limiti, output
            // finito/valido) si applicano identiche sulla forma decodificata.
            REPROJECTOR.with(|slot| {
                let mut slot = slot.borrow_mut();
                let stale = slot
                    .as_ref()
                    .is_none_or(|(s, t, _)| s != source || t != target);
                if stale {
                    *slot = Some((
                        source.clone(),
                        target.clone(),
                        Reprojector::new(source, target, MAX_CELL_COORDINATES)?,
                    ));
                }
                let (_, _, reprojector) = slot.as_ref().ok_or(ArrowTransportError::Internal(
                    "pipeline appena creata assente",
                ))?;
                Ok(Some(reprojector.reproject(geometry)?))
            })
        }
    }
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
    // per riga prima (ordine preservato dal collect indicizzato), poi la
    // selezione deterministica degli errori IN ORDINE DI RIGA; mai la
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
    // R9.9: i fallimenti attribuibili alla riga sono raccolti COMPLETI su
    // tutte le celle prima di chiudere — l'errore primario resta quello
    // della prima riga difettosa, arricchito della diagnostica bounded.
    let mut failures: Vec<(u64, ArrowTransportError)> = Vec::new();
    let mut values = Vec::with_capacity(results.len());
    for (row, result) in results.into_iter().enumerate() {
        match result {
            Ok(value) => values.push(value),
            Err(error) => {
                failures.push((row as u64, error));
                values.push(None);
            }
        }
    }
    if failures.is_empty() {
        return Ok(values);
    }
    Err(collect_cell_failures(failures))
}

/// Causa `plenora-row-diagnostics-v1` di un fallimento di cella attribuibile
/// alla riga; `None` per errori non row-scoped (difetti interni del
/// trasporto, parametri): propagano senza diagnostica, fail-closed com'e'.
/// Il vocabolario e' quello degli altri emettitori geo (gate WKB,
/// `geo.from_wkt`, `geo.from_coords`).
const fn cell_failure_cause(error: &ArrowTransportError) -> Option<&'static str> {
    match error {
        ArrowTransportError::CellTooLarge(_) => Some("geometry.cell_too_large"),
        ArrowTransportError::WrongGeometryType { .. } => Some("geometry.wrong_type"),
        // Decode e validazione WKB arrivano come `Geometry` (conversione
        // `From<PlenoraError>` dei payload di contratto).
        ArrowTransportError::Geometry(_) => Some("geometry.invalid_wkb"),
        #[cfg(feature = "proj-backend")]
        ArrowTransportError::Reproject(_) => Some("geometry.reprojection_failed"),
        #[cfg(feature = "geos-backend")]
        ArrowTransportError::MakeValid(_) => Some("geometry.repair_failed"),
        ArrowTransportError::Kernel(_)
        | ArrowTransportError::Topology(_)
        | ArrowTransportError::Construction(_)
        | ArrowTransportError::Advanced(_)
        | ArrowTransportError::Extended(_)
        | ArrowTransportError::ExtendedAlgorithm(_)
        | ArrowTransportError::Predicate(_)
        | ArrowTransportError::Analysis(_) => Some("geometry.kernel_failed"),
        _ => None,
    }
}

/// Chiude un insieme di fallimenti di cella (indice riga batch-locale,
/// errore): se OGNI fallimento e' attribuibile alla riga, l'errore della
/// prima riga e' restituito con la diagnostica completa allegata; se uno
/// qualunque non lo e', propaga il primo errore non attribuibile com'e'
/// (fail-closed senza diagnostica, mai un report parziale spacciato per
/// completo).
fn collect_cell_failures(failures: Vec<(u64, ArrowTransportError)>) -> ArrowTransportError {
    let mut rows = std::collections::BTreeMap::new();
    for (row, error) in &failures {
        let Some(cause) = cell_failure_cause(error) else {
            return match failures
                .into_iter()
                .find(|(_, candidate)| cell_failure_cause(candidate).is_none())
            {
                Some((_, error)) => error,
                None => ArrowTransportError::Internal("classificazione celle incoerente"),
            };
        };
        rows.entry(*row).or_insert(cause);
    }
    let report = cell_diagnostics_report(&rows);
    let Some((_, first)) = failures.into_iter().next() else {
        return ArrowTransportError::Internal("raccolta celle vuota");
    };
    first.with_row_diagnostics(report)
}

/// Report `plenora-row-diagnostics-v1` completo per fallimenti di cella
/// (scope Read, indici batch-locali zero-based, esempi bounded a 10, nessun
/// valore): la traduzione batch-locale -> assoluta e il merge cross-batch
/// spettano all'executor (segmenti `segment_emits_row_diagnostics`).
fn cell_diagnostics_report(rows: &std::collections::BTreeMap<u64, &'static str>) -> RowDiagnostics {
    const EXAMPLES_LIMIT: u64 = 10;
    let observed_total = rows.len() as u64;
    let mut counts = std::collections::BTreeMap::new();
    let mut examples = Vec::new();
    for (row, cause) in rows {
        *counts.entry((*cause).to_owned()).or_insert(0_u64) += 1;
        if u64::try_from(examples.len()).unwrap_or(u64::MAX) < EXAMPLES_LIMIT {
            examples.push(RowDiagnosticExample {
                source_index: *row,
                cause: (*cause).to_owned(),
                column: None,
                key: None,
                write_state: None,
            });
        }
    }
    RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness: RowDiagnosticsCompleteness::Complete,
        knowledge_limits: None,
        observed_total,
        total: Some(observed_total),
        input_total: None,
        counts,
        examples_limit: EXAMPLES_LIMIT,
        examples_truncated: observed_total > EXAMPLES_LIMIT,
        examples,
        diagnostic_state_counts: None,
        write_outcome: None,
    }
}

/// Braccio condiviso delle trasformazioni 1:1 fondibili di profilo B
/// (ADR-0012): i parametri sono risolti UNA volta ([`resolve_transform`],
/// stessi errori e stessa posizione dei bracci storici), poi per cella
/// decode -> kernel ([`apply_transform_cell`]) -> encode, con il primo
/// errore in ordine di riga (pattern di `map_nullable`). Comportamento
/// identico ai bracci per-operazione che sostituisce: stesse chiamate,
/// stesso ordine.
fn transform_cells_fusible(
    params: &TransformArrowSchema,
    cells: &BinaryArray,
) -> Result<TransformedColumn, ArrowTransportError> {
    let resolved = resolve_transform(params)?;
    Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
        let geometry = geometry_from_wkb(payload)?;
        apply_transform_cell(&resolved, &geometry)?
            .map(|output| encode_geometry(&output))
            .transpose()
    })?))
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
                .ok_or(ArrowTransportError::Internal(
                    "operazione geometrica senza kernel",
                ))?;
            Ok(TransformedColumn::Binary(map_nullable(cells, |payload| {
                Ok(transform_wkb(kernel, payload).map(Some)?)
            })?))
        }
        ArrowOperation::Buffer
        | ArrowOperation::Simplify
        | ArrowOperation::Boundary
        | ArrowOperation::PointOnSurface
        | ArrowOperation::AffineTransform
        | ArrowOperation::Translate
        | ArrowOperation::Scale
        | ArrowOperation::Rotate
        | ArrowOperation::ConcaveHull
        | ArrowOperation::Densify
        | ArrowOperation::SnapToGrid => transform_cells_fusible(params, cells),
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
                        let (_, _, reprojector) = slot.as_ref().ok_or(
                            ArrowTransportError::Internal("pipeline appena creata assente"),
                        )?;
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
                        });
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

pub(in crate::geo_transport) const fn spatial_predicate_name(
    predicate: SpatialPredicate,
) -> &'static str {
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
            _ => Err(ArrowTransportError::Internal(
                "shape Collective non coperta",
            )),
        },
        ArrowShape::WholeToMany => match params.operation {
            ArrowOperation::Polygonize => polygonize_batches(schema, batches, params),
            ArrowOperation::LineMerge => line_merge_batches(schema, batches, params),
            _ => Err(ArrowTransportError::Internal(
                "shape WholeToMany non coperta",
            )),
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
                ));
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
                values
                    .iter()
                    .map(|cell| cell.as_deref())
                    .collect::<BinaryArray>(),
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

// ---------------------------------------------------------------------------
// Runner fuso dei gruppi geo (ADR-0012)
// ---------------------------------------------------------------------------

/// Errore del runner fuso con l'attribuzione al kernel del gruppo
/// (ADR-0012 D12.6): il runner e' un'esecuzione alternativa del gruppo, non
/// una rimozione dei nodi — ogni fallimento dice sempre QUALE nodo ha rotto.
#[derive(Debug)]
pub enum FusedStepError {
    /// Errore di cella attribuito al kernel del gruppo (indice nel gruppo):
    /// stessa variante e stesso nodo del percorso non fuso (D12.3/D12.4).
    Kernel {
        /// Indice del kernel del gruppo a cui l'errore e' attribuito.
        index: usize,
        /// L'errore vero e proprio.
        error: ArrowTransportError,
    },
    /// Errore della misura terminale del gruppo (ADR-0012 M2): il percorso
    /// non fuso delle misure (`geo_measure_batch` nell'executor) NON transita
    /// da `ArrowTransportError` — il decode e' `decode_geometry_cell` chiuso
    /// direttamente da `step_error` e il kernel e' chiuso in `InvalidPlan`
    /// dal display dell'`OperationError`. La variante porta quindi il
    /// `PlenoraError` gia' nella forma del percorso non fuso: l'executor lo
    /// chiude con `step_error` al nodo misura, senza wrap aggiuntivi.
    Measure {
        /// Indice del nodo misura nel gruppo (l'ultimo membro).
        index: usize,
        /// L'errore vero e proprio, nella forma del percorso non fuso.
        error: PlenoraError,
    },
    /// Controllo dell'executor tra due kernel (cancellazione cooperativa,
    /// ADR 3): gia' nella forma finale (`PlenoraError::Cancelled` con nodo,
    /// operazione ed `execution_id`), propaga invariato.
    Control(PlenoraError),
}

/// Misura terminale di un gruppo fuso (ADR-0012 M2): il kernel scalare che
/// chiude il gruppo consumando la forma decodificata dell'ultimo passo,
/// senza ri-decodificare il WKB di confine. Le 5 misure "add column" dei
/// piani v4 (ramo `geo_measure_batch` dell'executor).
#[derive(Clone, Copy, Debug)]
pub enum FusedTerminalMeasure {
    /// `geo.area` -> colonna `Float64`.
    Area,
    /// `geo.length` -> colonna `Float64`.
    Length,
    /// `geo.perimeter` -> colonna `Float64`.
    Perimeter,
    /// `geo.vertex_count` -> colonna `UInt64`.
    VertexCount,
    /// `geo.to_wkt` -> colonna `Utf8`.
    ToWkt,
}

/// Terminale misura di un gruppo fuso (ADR-0012 M2): il kernel scalare e lo
/// schema di output del nodo misura (contratto inferito in validazione —
/// input + colonna misura appesa in coda, semantica v4 "add column").
#[derive(Clone, Copy)]
pub struct FusedTerminal<'a> {
    /// Misura da applicare sulla forma decodificata dell'ultimo passo.
    pub measure: FusedTerminalMeasure,
    /// Schema di output del nodo misura (input + colonna misura).
    pub output_schema: &'a SchemaRef,
}

/// Celle prodotte da un gruppo fuso (ADR-0012): la geometria ri-encodata UNA
/// volta (sempre — nel perimetro M2 la colonna geometria SOPRAVVIVE alla
/// misura, semantica v4 "add column") e, se il gruppo chiude con una misura
/// terminale, la colonna scalare calcolata sulla forma decodificata.
struct FusedCells {
    /// Geometrie al confine del gruppo, WKB canonico XY.
    geometry: Vec<Option<Vec<u8>>>,
    /// Colonna della misura terminale, se il gruppo ne ha una (M2).
    measure: Option<TransformedColumn>,
}

/// Esegue un gruppo di trasformazioni 1:1 fondibili su una colonna WKB con
/// UN decode e UN encode per batch (ADR-0012 D12.1): la forma decodificata
/// vive solo per la durata del gruppo sul singolo batch. Struttura
/// kernel-esterno/celle-interno: attribuzione errori esatta per kernel,
/// cancellazione per kernel (via `control`, stessa granularita' per batch
/// del percorso non fuso) e `catch_unwind` per kernel (l'executor sa quale
/// kernel e' in corso dall'ultimo `control` ritornato).
///
/// Tabella di attribuzione (D12.3/D12.4), per kernel i del gruppo:
///
/// - errore del kernel -> kernel i;
/// - output oltre `MAX_CELL_BYTES` (misura ESATTA via `wkb_size_xy`,
///   nessuna serializzazione) -> `CellTooLarge` al kernel i — riproduce il
///   check di `encode_geometry`;
/// - profilo A (`centroid`/`convex_hull`/`envelope`): la pipeline canonica
///   e' dentro `apply_transform_cell` (`transform_geometry_canonical`: OGC
///   in uscita, limite 64 MiB e validazione strutturale, come
///   `transform_wkb`) -> kernel i;
/// - profilo B con un kernel i+1 nel gruppo: `validate_geometry_structural`
///   poi `check_geometry_valid` (il fallimento del decode del nodo
///   successivo, nell'ordine di `geometry_from_wkb`) -> kernel i+1.
///   ECCEZIONE M3 (trappola 1): se il kernel i+1 e' `make_valid` il check
///   OGC e' OMESSO — nel percorso non fuso quel nodo legge l'input col solo
///   gate strutturale di `make_valid_wkb` (l'OGC-invalido e' cio' che
///   ripara); la validazione strutturale resta;
/// - profilo B sull'ULTIMO kernel SENZA misura terminale: NESSUNA
///   validazione extra — nel percorso non fuso l'output esce dopo
///   `encode_geometry` senza altra validazione e decodera' chi consuma;
/// - con misura terminale (M2): la validazione del "decode" prima della
///   misura (strutturale, poi OGC) e' nel passo della misura -> nodo misura
///   (variante [`FusedStepError::Measure`], mai `ArrowTransportError`: il
///   ramo non fuso delle misure non la attraversa).
///
/// Il decode iniziale (con il check `MAX_CELL_BYTES` sull'input, pattern di
/// `map_nullable`) e' attribuito al primo kernel del gruppo, l'encode finale
/// all'ultimo — come nel percorso non fuso. ECCEZIONE M3 (trappola 1): se il
/// PRIMO kernel e' `make_valid` il decode iniziale e' SOLO strutturale
/// (`wkb_decoder::decode_validated`, la stessa camminata validante senza il
/// check OGC) — nel percorso non fuso quel nodo non chiama affatto
/// `geometry_from_wkb` sull'input.
///
/// `control` e' invocato con l'indice del kernel PRIMA di ogni passo — la
/// misura terminale inclusa, con indice `group.len()` (il nodo misura e'
/// l'ultimo membro del gruppo): e' il punto di cancellazione cooperativa
/// dell'executor (errore `Control`) e il suo marker del kernel in corso per
/// l'attribuzione dei panic. La cancellazione resta TRA i kernel, mai dentro
/// — compatibile per costruzione col `NonInterruptible` di
/// `make_valid`/`reproject` (M3): il callback dell'executor onora il
/// behavior di catalogo del nodo, come il check del loop non fuso.
///
/// # Errors
///
/// [`FusedStepError::Kernel`] con l'indice del kernel responsabile per gli
/// errori di cella delle trasformazioni; [`FusedStepError::Measure`] per gli
/// errori della misura terminale; [`FusedStepError::Control`] per gli errori
/// del controllo executor (cancellazione).
fn transform_cells_fused(
    group: &[&TransformArrowSchema],
    terminal: Option<FusedTerminalMeasure>,
    cells: &BinaryArray,
    control: &mut dyn FnMut(usize) -> Result<(), PlenoraError>,
) -> Result<FusedCells, FusedStepError> {
    if group.is_empty() {
        return Err(FusedStepError::Kernel {
            index: 0,
            error: ArrowTransportError::Internal("gruppo fuso vuoto"),
        });
    }
    // Decode UNA volta: errori attribuiti al primo kernel del gruppo (come il
    // fallimento di `geometry_from_wkb` al primo nodo del percorso non fuso).
    // M3 (trappola 1): con `make_valid` in testa il gate e' SOLO strutturale.
    let first_repairs = group
        .first()
        .is_some_and(|params| accepts_ogc_invalid_input(params));
    let mut geometries = map_nullable(cells, |payload| {
        let geometry = if first_repairs {
            plenora_kernels_geo::wkb_decoder::decode_validated(payload)?
        } else {
            geometry_from_wkb(payload)?
        };
        Ok(Some(geometry))
    })
    .map_err(|error| FusedStepError::Kernel { index: 0, error })?;
    for (index, params) in group.iter().enumerate() {
        control(index).map_err(FusedStepError::Control)?;
        // Risoluzione parametri del kernel i: nel percorso non fuso avviene
        // in testa al braccio, prima di toccare le celle — stessa posizione
        // e stessa attribuzione anche a batch vuoto.
        let resolved =
            resolve_transform(params).map_err(|error| FusedStepError::Kernel { index, error })?;
        // M3 (trappola 1): davanti a un nodo `make_valid` la validazione
        // inter-passo e' SOLO strutturale (vedi la tabella sopra).
        let successor_repairs = group
            .get(index + 1)
            .is_some_and(|next| accepts_ogc_invalid_input(next));
        apply_fused_kernel(
            &resolved,
            &mut geometries,
            index,
            group.len(),
            successor_repairs,
        )?;
    }
    // Misura terminale (M2): passo dedicato DOPO il loop dei kernel. Nel
    // percorso non fuso il nodo trasformazione completa TUTTE le righe
    // (kernel + encode) prima che il nodo misura decodifichi la prima cella;
    // il passo separato riproduce esattamente questa precedenza, con lo
    // stesso confine di cancellazione (`control` sull'indice del nodo
    // misura).
    let measure = match terminal {
        None => None,
        Some(terminal) => {
            control(group.len()).map_err(FusedStepError::Control)?;
            Some(apply_fused_measure(terminal, &geometries, group.len())?)
        }
    };
    // Encode UNA volta alla fine: errori attribuiti all'ultimo kernel, con
    // raccolta completa per riga come nel percorso non fuso (`map_nullable`).
    let last = group.len() - 1;
    let results: Vec<Result<Option<Vec<u8>>, ArrowTransportError>> = geometries
        .par_iter()
        .map(|slot| slot.as_ref().map(encode_geometry).transpose())
        .collect();
    let mut failures: Vec<(u64, ArrowTransportError)> = Vec::new();
    let mut values = Vec::with_capacity(results.len());
    for (row, result) in results.into_iter().enumerate() {
        match result {
            Ok(value) => values.push(value),
            Err(error) => {
                failures.push((row as u64, error));
                values.push(None);
            }
        }
    }
    if !failures.is_empty() {
        return Err(FusedStepError::Kernel {
            index: last,
            error: collect_cell_failures(failures),
        });
    }
    Ok(FusedCells {
        geometry: values,
        measure,
    })
}

/// Un kernel del gruppo su tutte le celle decodificate: rayon con collect
/// indicizzato (ADR-0001), poi raccolta COMPLETA dei fallimenti per riga
/// (R9.9): gli errori del kernel sono attribuiti al kernel stesso, quelli
/// della validazione inter-passo al kernel successivo — il kernel ha la
/// precedenza (nel percorso non fuso il suo nodo fallirebbe prima, col suo
/// report completo, e il successivo non partirebbe mai). La tabella di
/// attribuzione e' quella di [`transform_cells_fused`].
/// `successor_accepts_ogc_invalid` e' vero solo quando il kernel successivo
/// e' `make_valid` (M3, trappola 1): la validazione inter-passo resta
/// strutturale ma omette il check OGC.
fn apply_fused_kernel(
    resolved: &ResolvedTransform,
    geometries: &mut [Option<Geometry<f64>>],
    index: usize,
    group_len: usize,
    successor_accepts_ogc_invalid: bool,
) -> Result<(), FusedStepError> {
    // Profilo A (D12.4): la validazione post-kernel e' interamente dentro
    // `transform_geometry_canonical` (OGC, 64 MiB, strutturale — kernel i).
    let profile_a = matches!(resolved, ResolvedTransform::Canonical(_));
    let results: Vec<Result<(), FusedCellFailure>> = geometries
        .par_iter_mut()
        .map(|slot| {
            let Some(input) = slot.take() else {
                return Ok(());
            };
            let output =
                apply_transform_cell(resolved, &input).map_err(FusedCellFailure::Kernel)?;
            if !profile_a {
                if let Some(geometry) = &output {
                    // D12.3: il limite di cella scatta a ogni nodo intermedio
                    // con attribuzione al produttore — misura ESATTA via
                    // `wkb_size_xy`, stessa variante di `encode_geometry`.
                    let size = wkb_size_xy(geometry);
                    if size > MAX_CELL_BYTES {
                        return Err(FusedCellFailure::Kernel(ArrowTransportError::CellTooLarge(
                            size,
                        )));
                    }
                    // D12.4 profilo B: l'intermedio invalido fallirebbe al
                    // decode del nodo successivo (strutturale, poi OGC —
                    // l'ordine di `geometry_from_wkb`) -> kernel i+1.
                    // Sull'ultimo kernel del gruppo NESSUNA validazione
                    // extra: l'output esce dopo `encode_geometry`, come nel
                    // percorso non fuso.
                    if index + 1 < group_len {
                        validate_geometry_structural(geometry, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS)
                            .map_err(ArrowTransportError::from)
                            .map_err(FusedCellFailure::Successor)?;
                        // M3 (trappola 1): il check OGC e' omesso SOLO
                        // davanti a `make_valid` — il suo "decode" non fuso
                        // e' il solo gate strutturale di `make_valid_wkb`.
                        if !successor_accepts_ogc_invalid {
                            check_geometry_valid(geometry)
                                .map_err(ArrowTransportError::from)
                                .map_err(FusedCellFailure::Successor)?;
                        }
                    }
                }
            }
            *slot = output;
            Ok(())
        })
        .collect();
    let mut kernel_failures: Vec<(u64, ArrowTransportError)> = Vec::new();
    let mut successor_failures: Vec<(u64, ArrowTransportError)> = Vec::new();
    for (row, result) in results.into_iter().enumerate() {
        match result {
            Ok(()) => {}
            Err(FusedCellFailure::Kernel(error)) => kernel_failures.push((row as u64, error)),
            Err(FusedCellFailure::Successor(error)) => {
                successor_failures.push((row as u64, error));
            }
        }
    }
    if !kernel_failures.is_empty() {
        return Err(FusedStepError::Kernel {
            index,
            error: collect_cell_failures(kernel_failures),
        });
    }
    if !successor_failures.is_empty() {
        return Err(FusedStepError::Kernel {
            index: index + 1,
            error: collect_cell_failures(successor_failures),
        });
    }
    Ok(())
}

/// Fallimento di una cella nel runner fuso: del kernel in corso o della
/// validazione inter-passo attribuita al kernel successivo (D12.3/D12.4).
enum FusedCellFailure {
    /// Errore del kernel in corso.
    Kernel(ArrowTransportError),
    /// Errore della validazione inter-passo (attribuito al kernel + 1).
    Successor(ArrowTransportError),
}

/// Misura terminale di un gruppo fuso sulle geometrie decodificate
/// (ADR-0012 M2); `index` e' l'indice del nodo misura nel gruppo (numero di
/// trasformazioni). Per cella, nell'ordine del percorso non fuso
/// (`geo_measure_batch`): validazione del "decode" (strutturale, poi OGC —
/// l'ordine di `geometry_from_wkb`, profilo B di D12.4) poi kernel scalare;
/// null-in -> null-out senza validazione ne' kernel, come il ramo non fuso.
///
/// Il check `MAX_CELL_BYTES` input-side del decode non fuso non e'
/// riprodotto: irraggiungibile (l'encode del nodo a monte scatta prima —
/// stessa classe del check input-side dei nodi interni, D12.3).
///
/// # Errors
///
/// [`FusedStepError::Measure`] con il `PlenoraError` nella forma del
/// percorso non fuso (validazione grezza; kernel chiuso in `InvalidPlan`
/// dal display dell'`OperationError`), attribuito al nodo misura.
fn apply_fused_measure(
    measure: FusedTerminalMeasure,
    geometries: &[Option<Geometry<f64>>],
    index: usize,
) -> Result<TransformedColumn, FusedStepError> {
    match measure {
        FusedTerminalMeasure::Area => Ok(TransformedColumn::Float64(measure_cells(
            geometries, index, area,
        )?)),
        FusedTerminalMeasure::Length => Ok(TransformedColumn::Float64(measure_cells(
            geometries, index, length,
        )?)),
        FusedTerminalMeasure::Perimeter => Ok(TransformedColumn::Float64(measure_cells(
            geometries, index, perimeter,
        )?)),
        FusedTerminalMeasure::VertexCount => Ok(TransformedColumn::UInt64(measure_cells(
            geometries,
            index,
            vertex_count,
        )?)),
        FusedTerminalMeasure::ToWkt => Ok(TransformedColumn::Utf8(measure_cells(
            geometries, index, to_wkt,
        )?)),
    }
}

/// Una misura scalare su tutte le celle decodificate del gruppo (M2): per
/// cella validazione pre-misura poi kernel, con raccolta COMPLETA dei
/// fallimenti per riga (R9.9 — il ramo non fuso raccoglie gli stessi
/// fallimenti in `map_nullable`/`geo_measure_batch`); l'errore primario e'
/// quello della prima riga difettosa, nella forma del percorso non fuso.
fn measure_cells<T: Send>(
    geometries: &[Option<Geometry<f64>>],
    index: usize,
    kernel: impl Fn(&Geometry<f64>) -> Result<T, OperationError> + Sync,
) -> Result<Vec<Option<T>>, FusedStepError> {
    let results: Vec<Result<Option<T>, MeasureCellFailure>> = geometries
        .par_iter()
        .map(|slot| {
            let Some(geometry) = slot.as_ref() else {
                return Ok(None);
            };
            // Validazione inter-passo prima della misura (D12.4 profilo B):
            // l'intermedio invalido fallirebbe al decode del nodo misura
            // (strutturale, poi OGC — l'ordine di `geometry_from_wkb`) ->
            // attribuzione al nodo misura, con il `PlenoraError` grezzo del
            // ramo non fuso.
            validate_geometry_structural(geometry, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS)
                .and_then(|()| check_geometry_valid(geometry))
                .map_err(|error| MeasureCellFailure {
                    cause: "geometry.invalid_wkb",
                    error,
                })?;
            // Stessa chiusura del ramo non fuso: `OperationError` ->
            // `InvalidPlan` del suo display.
            kernel(geometry)
                .map(Some)
                .map_err(|error| MeasureCellFailure {
                    cause: "geometry.kernel_failed",
                    error: PlenoraError::InvalidPlan(error.to_string()),
                })
        })
        .collect();
    let mut failures: Vec<(u64, &'static str, PlenoraError)> = Vec::new();
    let mut values = Vec::with_capacity(results.len());
    for (row, result) in results.into_iter().enumerate() {
        match result {
            Ok(value) => values.push(value),
            Err(failure) => {
                failures.push((row as u64, failure.cause, failure.error));
                values.push(None);
            }
        }
    }
    if failures.is_empty() {
        return Ok(values);
    }
    Err(FusedStepError::Measure {
        index,
        error: collect_measure_failures(failures),
    })
}

/// Fallimento di una cella della misura terminale: causa gia' assegnata al
/// sito (validazione del "decode" o kernel scalare).
struct MeasureCellFailure {
    cause: &'static str,
    error: PlenoraError,
}

/// Chiude i fallimenti per riga della misura terminale: report completo
/// allegato all'errore della prima riga difettosa (forma del percorso non
/// fuso, `PlenoraError`).
fn collect_measure_failures(failures: Vec<(u64, &'static str, PlenoraError)>) -> PlenoraError {
    let mut rows = std::collections::BTreeMap::new();
    let mut first = None;
    for (row, cause, error) in failures {
        rows.entry(row).or_insert(cause);
        if first.is_none() {
            first = Some(error);
        }
    }
    let Some(first) = first else {
        return PlenoraError::Internal("raccolta misura vuota".to_owned());
    };
    first.with_row_diagnostics(cell_diagnostics_report(&rows))
}

/// Batch trasformato da un gruppo fuso, con l'handle prepared del PRIMO
/// kernel del gruppo (ADR-0012) per la validazione della colonna di input
/// (tipo Binary + metadati geoarrow, attribuita al primo nodo come nel
/// percorso non fuso) e l'handle dell'ULTIMA trasformazione per lo schema
/// di output: con `reproject` nel gruppo (M3) il CRS del campo geometria
/// cambia a meta' catena e lo schema di confine e' quello dell'ultimo nodo
/// — per le op M1/M2 (CRS invariato lungo il gruppo) coincide con quello
/// del primo kernel, perche' la ricostruzione canonica del campo dipende
/// solo da (nome colonna, CRS di output) e gli altri campi passano
/// invariati.
///
/// Misura terminale (M2): con `terminal` il runner applica il kernel scalare
/// sulla forma decodificata dell'ultimo passo e appende la colonna misura in
/// coda — la STESSA sequenza del percorso non fuso (`one_to_one_batch_prepared`
/// dell'ultima trasformazione, poi `append_output_column` del nodo misura):
/// la colonna geometria SOPRAVVIVE (ri-encodata una sola volta) e il batch
/// finale e' costruito sullo schema del contratto del nodo misura.
///
/// # Errors
///
/// [`FusedStepError::Kernel`] con l'attribuzione al kernel del gruppo
/// (colonna di input non Binary al primo kernel, errori di cella secondo la
/// tabella di [`transform_cells_fused`], schema incoerente all'ultima
/// trasformazione); [`FusedStepError::Measure`] per gli errori della misura
/// terminale (validazione pre-misura, kernel, schema del nodo misura);
/// [`FusedStepError::Control`] per la cancellazione dell'executor.
pub fn one_to_one_batch_fused(
    batch: &RecordBatch,
    group: &[&TransformArrowSchema],
    terminal: Option<FusedTerminal<'_>>,
    prepared: &OneToOnePrepared,
    output: &OneToOnePrepared,
    control: &mut dyn FnMut(usize) -> Result<(), PlenoraError>,
) -> Result<RecordBatch, FusedStepError> {
    let geometry_index = prepared.geometry_index;
    let cells = batch
        .column(geometry_index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| FusedStepError::Kernel {
            index: 0,
            error: ArrowTransportError::GeometryColumnNotBinary {
                name: group
                    .first()
                    .map_or("geometry", |params| params.geometry_column())
                    .to_owned(),
                actual: batch.column(geometry_index).data_type().to_string(),
            },
        })?;
    let fused = transform_cells_fused(
        group,
        terminal.map(|terminal| terminal.measure),
        cells,
        control,
    )?;
    let mut columns = batch.columns().to_vec();
    columns[geometry_index] = std::sync::Arc::new(
        fused
            .geometry
            .iter()
            .map(|cell| cell.as_deref())
            .collect::<BinaryArray>(),
    );
    let last = group.len().saturating_sub(1);
    // Batch al confine dell'ultima trasformazione: la costruzione del
    // percorso non fuso (`one_to_one_batch_prepared`), stessa attribuzione;
    // lo schema e' quello dell'ULTIMA trasformazione (M3: `reproject` puo'
    // cambiare il CRS del campo geometria a meta' gruppo).
    let output = RecordBatch::try_new(output.output_schema.clone(), columns)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
        .map_err(|error| FusedStepError::Kernel { index: last, error })?;
    let Some(terminal) = terminal else {
        return Ok(output);
    };
    // Append della colonna misura sullo schema del nodo misura: la sequenza
    // di `append_output_column`, con lo stesso errore (`PlenoraError` da
    // `ArrowError`, attribuito al nodo misura).
    let column: plenora_core::arrow::array::ArrayRef = match fused.measure {
        Some(TransformedColumn::Float64(values)) => std::sync::Arc::new(Float64Array::from(values)),
        Some(TransformedColumn::UInt64(values)) => std::sync::Arc::new(UInt64Array::from(values)),
        Some(TransformedColumn::Utf8(values)) => std::sync::Arc::new(StringArray::from(values)),
        // Invariante di `transform_cells_fused` (il tipo della colonna e'
        // funzione della misura): mai raggiungibile — errore, non panic.
        _ => {
            return Err(FusedStepError::Measure {
                index: group.len(),
                error: PlenoraError::Internal(
                    "runner fuso: colonna misura assente o di tipo inatteso".to_owned(),
                ),
            });
        }
    };
    let mut columns = output.columns().to_vec();
    columns.push(column);
    RecordBatch::try_new(terminal.output_schema.clone(), columns)
        .map_err(PlenoraError::from)
        .map_err(|error| FusedStepError::Measure {
            index: group.len(),
            error,
        })
}

/// Aggrega un report batch-locale in quello accumulato applicando l'offset
/// sorgente assoluto (checked): conteggi sommati, esempi bounded al limite,
/// completeness degradata se uno qualunque dei contributi non e' completo.
/// Stessa disciplina del merge dell'executor (R9.9): mai un indice inventato,
/// mai un overflow silenzioso.
fn merge_report_with_offset(
    aggregate: &mut Option<RowDiagnostics>,
    incoming: &RowDiagnostics,
    source_offset: u64,
) -> Result<(), ArrowTransportError> {
    let mut shifted = incoming.clone();
    for example in &mut shifted.examples {
        example.source_index = source_offset.checked_add(example.source_index).ok_or(
            ArrowTransportError::Internal("indice sorgente fuori intervallo"),
        )?;
    }
    let Some(existing) = aggregate.as_ref() else {
        *aggregate = Some(shifted);
        return Ok(());
    };
    let mut merged = existing.clone();
    if merged.contract != shifted.contract
        || merged.scope != shifted.scope
        || merged.index_basis != shifted.index_basis
        || merged.examples_limit != shifted.examples_limit
    {
        return Err(ArrowTransportError::Internal(
            "report row-scoped incompatibili nello stesso stream",
        ));
    }
    merged.observed_total = merged
        .observed_total
        .checked_add(shifted.observed_total)
        .ok_or(ArrowTransportError::Internal(
            "conteggio row-scoped fuori intervallo",
        ))?;
    merged.total = match (merged.total, shifted.total) {
        (Some(left), Some(right)) => Some(left.checked_add(right).ok_or(
            ArrowTransportError::Internal("totale row-scoped fuori intervallo"),
        )?),
        _ => None,
    };
    merged.input_total = match (merged.input_total, shifted.input_total) {
        (Some(left), Some(right)) => Some(left.checked_add(right).ok_or(
            ArrowTransportError::Internal("input_total diagnostico overflow"),
        )?),
        _ => None,
    };
    for (cause, count) in shifted.counts {
        let entry = merged.counts.entry(cause).or_insert(0_u64);
        *entry = entry
            .checked_add(count)
            .ok_or(ArrowTransportError::Internal(
                "conteggio causa fuori intervallo",
            ))?;
    }
    let incoming_example_count = shifted.examples.len();
    let before = merged.examples.len();
    for example in shifted.examples {
        if u64::try_from(merged.examples.len())
            .map_err(|_| ArrowTransportError::Internal("numero esempi fuori intervallo"))?
            >= merged.examples_limit
        {
            break;
        }
        merged.examples.push(example);
    }
    merged.examples_truncated = merged.examples_truncated
        || shifted.examples_truncated
        || merged.examples.len().saturating_sub(before) < incoming_example_count;
    if shifted.completeness != RowDiagnosticsCompleteness::Complete {
        merged.completeness = shifted.completeness;
        let mut knowledge_limits = merged.knowledge_limits.take().unwrap_or_default();
        for limit in shifted.knowledge_limits.unwrap_or_default() {
            if !knowledge_limits.contains(&limit) {
                knowledge_limits.push(limit);
            }
        }
        merged.knowledge_limits = (!knowledge_limits.is_empty()).then_some(knowledge_limits);
    }
    *aggregate = Some(merged);
    Ok(())
}

/// Allega il report accumulato a un errore tardivo non row-scoped (R9.9):
/// la scansione completa non e' piu' dimostrabile, quindi il report e'
/// declassato a `Partial` con `total` sconosciuto e il knowledge limit
/// dichiarato — stessa disciplina e stesso vocabolario `data_tools.*`
/// dell'executor (`attach_partial_row_diagnostics`). Senza report
/// accumulato l'errore propaga com'e'.
fn attach_partial_report(
    error: ArrowTransportError,
    aggregate: &mut Option<RowDiagnostics>,
    knowledge_limit: &str,
) -> ArrowTransportError {
    let Some(mut report) = aggregate.take() else {
        return error;
    };
    report.completeness = RowDiagnosticsCompleteness::Partial;
    report.total = None;
    report.knowledge_limits = Some(vec![knowledge_limit.to_owned()]);
    error.with_row_diagnostics(report)
}

/// Operazioni 1:1: la colonna geometria e' sostituita dal risultato (Binary
/// GeoArrow-WKB, Float64, `UInt64`, Utf8 oppure quattro colonne Float64 per
/// `bounds`); tutte le altre colonne passano invariate; i null sono preservati.
///
/// Fallimenti row-scoped (R9.9): TUTTI i batch sono scansionati, i report
/// batch-locali sono aggregati con offset sorgente assoluti (checked) in un
/// unico report completo allegato all'errore della prima riga invalida; un
/// errore tardivo non row-scoped propaga l'errore reale fail-closed con il
/// report accumulato declassato a `Partial` ([`attach_partial_report`]),
/// mai la perdita silenziosa della diagnostica gia' osservata.
/// In caso di rifiuto nessun batch di output e' pubblicato.
fn one_to_one_batches(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    params: &TransformArrowSchema,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let prepared = prepare_one_to_one(schema, params)?;
    let mut output_batches = Vec::with_capacity(batches.len());
    let mut source_offset = 0_u64;
    let mut diagnostics: Option<RowDiagnostics> = None;
    let mut first_error: Option<ArrowTransportError> = None;
    for batch in batches {
        match one_to_one_batch_prepared(batch, params, &prepared) {
            Ok(output) => {
                if diagnostics.is_none() {
                    output_batches.push(output);
                }
            }
            Err(error) => {
                let report = error.row_diagnostics().cloned();
                if let Some(report) = report {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    if let Err(error) =
                        merge_report_with_offset(&mut diagnostics, &report, source_offset)
                    {
                        return Err(attach_partial_report(
                            error,
                            &mut diagnostics,
                            "data_tools.diagnostic_merge_failed",
                        ));
                    }
                } else {
                    return Err(attach_partial_report(
                        error,
                        &mut diagnostics,
                        "data_tools.processing_interrupted",
                    ));
                }
            }
        }
        source_offset =
            source_offset
                .checked_add(u64::try_from(batch.num_rows()).map_err(|_| {
                    ArrowTransportError::Internal("cardinalita batch fuori intervallo")
                })?)
                .ok_or(ArrowTransportError::Internal(
                    "indice sorgente stream fuori intervallo",
                ))?;
    }
    if let Some(report) = diagnostics {
        let base = first_error.map_or_else(
            || ArrowTransportError::Internal("diagnostica senza errore sorgente"),
            ArrowTransportError::into_source,
        );
        return Err(base.with_row_diagnostics(report));
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
                    ));
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
        let mut columns: Vec<plenora_core::arrow::array::ArrayRef> =
            Vec::with_capacity(batch.num_columns() + 1);
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
                Some(dissolve_validated(&polygons)?)
            }
        }
        ArrowOperation::LineBuilder => line_from_ordered_points(&geometries)?,
        ArrowOperation::PolygonBuilder => polygon_from_ordered_points(&geometries)?,
        _ => {
            return Err(ArrowTransportError::Internal(
                "collect_batches: operazione non N:1",
            ));
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
    let snap_tolerance =
        params
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
    let cleaned = clean_valid_polygon_topology_validated(
        &geometries,
        snap_tolerance,
        remove_overlaps,
        fill_gaps,
        MAX_ROWS,
        MAX_CLEAN_VERTICES,
    )?;
    let mut encoded: Vec<Option<Vec<u8>>> = Vec::with_capacity(cleaned.len());
    for geometry in &cleaned {
        encoded.push(geometry.as_ref().map(encode_geometry).transpose()?);
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
            // Decoder validante (ADR-0011): una passata, stesso contratto
            // strutturale di validate_wkb_contract + costruzione della
            // geometria. Niente `check_validation` OGC: la validita' e' il
            // dato che geometry_diagnostics stessa produce.
            let geometry = plenora_kernels_geo::wkb_decoder::decode_validated(payload)?;
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
        rows.iter()
            .map(|row| row.0.as_deref())
            .collect::<BinaryArray>(),
    )];
    if with_class {
        let classes: Vec<&'static str> = rows
            .iter()
            .map(|row| {
                row.1
                    .ok_or(ArrowTransportError::Internal("classe mancante"))
            })
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

/// Valori numerici di una colonna coordinate: ogni cella e' `Ok(Some(f64))`
/// (finita o meno — la finitezza e' giudicata dal kernel punto), `Ok(None)`
/// per i null, oppure `Err(causa)` per un difetto row-scoped.
///
/// Guardia di range (R5.4): oltre 2^53 in valore assoluto la conversione
/// i64 -> f64 non e' esatta e sposterebbe la coordinata in silenzio; la riga
/// e' marcata con la causa `geometry.inexact_integer_coordinate` invece di
/// produrre una geometria imprecisa — la raccolta completa spetta al
/// chiamante ([`from_coords_batches`]).
fn numeric_values(
    batch: &RecordBatch,
    index: usize,
    name: &str,
) -> Result<Vec<Result<Option<f64>, &'static str>>, ArrowTransportError> {
    let column = batch.column(index);
    match column.data_type() {
        DataType::Float64 => Ok(column
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or(ArrowTransportError::Internal("tipo verificato Float64"))?
            .iter()
            .map(Ok)
            .collect()),
        DataType::Int64 => {
            const MAX_EXACT: u64 = 1_u64 << 53;
            let values = column
                .as_any()
                .downcast_ref::<plenora_core::arrow::array::Int64Array>()
                .ok_or(ArrowTransportError::Internal("tipo verificato Int64"))?
                .iter()
                .map(|value| {
                    value
                        .map(|x| {
                            if x.unsigned_abs() > MAX_EXACT {
                                Err("geometry.inexact_integer_coordinate")
                            } else {
                                // Esattezza garantita dalla guardia: |x| <= 2^53.
                                #[allow(clippy::cast_precision_loss)]
                                Ok(x as f64)
                            }
                        })
                        .transpose()
                })
                .collect::<Vec<_>>();
            Ok(values)
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
/// o intere oltre 2^53 sono difetti row-scoped: raccolta completa su tutti
/// i batch (indice sorgente assoluto zero-based) e rifiuto fail-closed con
/// diagnostica `plenora-row-diagnostics-v1`, mai valori nei messaggi.
/// Un errore tardivo non row-scoped propaga l'errore reale con il report
/// delle rejection gia' osservate declassato a `Partial`
/// ([`attach_partial_coordinate_report`]), mai la perdita silenziosa.
/// Tutte le colonne di input passano invariate.
// Sequenza lineare di raccolta per batch: lunga per costruzione (R9.9).
#[allow(clippy::too_many_lines)]
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
    let mut rejections: Vec<CoordinateRejection<'_>> = Vec::new();
    let mut source_offset = 0_u64;
    let mut output_batches = Vec::with_capacity(batches.len());
    for batch in batches {
        if let Err(error) = from_coords_one_batch(
            batch,
            params,
            limit,
            x_index,
            y_index,
            &output_schema,
            &mut rejections,
            &mut source_offset,
            &mut output_batches,
        ) {
            // Errore tardivo non row-scoped con rejection gia' osservate:
            // propaga l'errore reale con il report parziale allegato, mai la
            // perdita silenziosa (stessa disciplina di
            // [`attach_partial_report`]); zero accepted (si risponde Err).
            return Err(attach_partial_coordinate_report(error, &rejections));
        }
    }
    reject_coordinate_rows(&rejections)?;
    Ok((std::sync::Arc::new(output_schema), output_batches))
}

/// Corpo per-batch di `from_coords`: validazione cardinalita', raccolta
/// delle rejection row-scoped (indice assoluto via `source_offset`) e
/// costruzione del batch di output. Estratto dal ciclo affinche' un errore
/// tardivo non row-scoped possa essere arricchito dal chiamante con il
/// report parziale delle rejection gia' osservate (R9.9).
#[allow(clippy::too_many_arguments)]
fn from_coords_one_batch<'a>(
    batch: &RecordBatch,
    params: &'a TransformArrowSchema,
    limit: u64,
    x_index: usize,
    y_index: usize,
    output_schema: &Schema,
    rejections: &mut Vec<CoordinateRejection<'a>>,
    source_offset: &mut u64,
    output_batches: &mut Vec<RecordBatch>,
) -> Result<(), ArrowTransportError> {
    if batch.num_rows() as u64 > limit {
        return Err(ArrowTransportError::OutputRowsExceeded {
            actual: batch.num_rows() as u64,
            limit,
        });
    }
    let xs = numeric_values(batch, x_index, params.x_column())?;
    let ys = numeric_values(batch, y_index, params.y_column())?;
    let mut points: Vec<Option<Vec<u8>>> = Vec::with_capacity(batch.num_rows());
    for (row, (x, y)) in xs.into_iter().zip(ys).enumerate() {
        let source_index = source_offset
            .checked_add(u64::try_from(row).map_err(|_| {
                ArrowTransportError::Internal("indice coordinata non rappresentabile")
            })?)
            .ok_or(ArrowTransportError::Internal("overflow indice coordinata"))?;
        let evaluated = match (x, y) {
            (Ok(x), Ok(y)) => Ok((x, y)),
            (Err(cause), _) => Err((cause, params.x_column())),
            (_, Err(cause)) => Err((cause, params.y_column())),
        };
        let (x, y) = match evaluated {
            Ok(pair) => pair,
            Err((cause, column)) => {
                rejections.push(CoordinateRejection {
                    source_index,
                    cause,
                    column,
                });
                points.push(None);
                continue;
            }
        };
        match (x, y) {
            (Some(x), Some(y)) => match point_from_lon_lat(x, y) {
                Ok(point) => points.push(Some(encode_geometry(&point)?)),
                Err(
                    plenora_kernels_geo::construction::ConstructionError::NonFiniteCoordinate {
                        name,
                    },
                ) => {
                    let column = if name == "lon" {
                        params.x_column()
                    } else {
                        params.y_column()
                    };
                    rejections.push(CoordinateRejection {
                        source_index,
                        cause: "geometry.non_finite_coordinate",
                        column,
                    });
                    points.push(None);
                }
                Err(error) => return Err(ArrowTransportError::Construction(error)),
            },
            _ => points.push(None),
        }
    }
    *source_offset = source_offset
        .checked_add(batch.num_rows() as u64)
        .ok_or(ArrowTransportError::Internal("overflow offset sorgente"))?;
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
    Ok(())
}

/// Rifiuto row-scoped di `from_coords`: indice sorgente assoluto, causa e
/// nome della colonna coordinata (mai il valore).
struct CoordinateRejection<'a> {
    source_index: u64,
    cause: &'static str,
    column: &'a str,
}

/// Report `plenora-row-diagnostics-v1` delle rejection coordinate: indice
/// sorgente assoluto, conteggi esatti, esempi bounded (mai valori). Il
/// chiamante decide completeness/knowledge limits: completo a scansione
/// finita ([`reject_coordinate_rows`]), parziale su errore tardivo
/// ([`attach_partial_coordinate_report`]).
fn coordinate_diagnostics_report(
    rejections: &[CoordinateRejection<'_>],
) -> Result<RowDiagnostics, ArrowTransportError> {
    const EXAMPLES_LIMIT: u64 = 10;
    let mut rows = std::collections::BTreeMap::new();
    for rejection in rejections {
        rows.entry(rejection.source_index).or_insert(rejection);
    }
    let observed_total = u64::try_from(rows.len())
        .map_err(|_| ArrowTransportError::Internal("troppe rejection coordinate"))?;
    let mut counts = std::collections::BTreeMap::new();
    let mut examples = Vec::new();
    for rejection in rows.values() {
        let count = counts.entry(rejection.cause.to_owned()).or_insert(0_u64);
        *count = count.checked_add(1).ok_or(ArrowTransportError::Internal(
            "overflow conteggio causa coordinate",
        ))?;
        if u64::try_from(examples.len())
            .map_err(|_| ArrowTransportError::Internal("troppi esempi coordinate"))?
            < EXAMPLES_LIMIT
        {
            examples.push(RowDiagnosticExample {
                source_index: rejection.source_index,
                cause: rejection.cause.to_owned(),
                column: Some(rejection.column.to_owned()),
                key: None,
                write_state: None,
            });
        }
    }
    Ok(RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness: RowDiagnosticsCompleteness::Complete,
        knowledge_limits: None,
        observed_total,
        total: Some(observed_total),
        input_total: None,
        counts,
        examples_limit: EXAMPLES_LIMIT,
        examples_truncated: observed_total > EXAMPLES_LIMIT,
        examples,
        diagnostic_state_counts: None,
        write_outcome: None,
    })
}

/// Errore tardivo non row-scoped di `from_coords` con rejection gia'
/// osservate: allega il report declassato a `Partial` (`total` sconosciuto,
/// knowledge limit dichiarato — stessa disciplina di
/// [`attach_partial_report`]). Senza rejection l'errore propaga com'e'.
fn attach_partial_coordinate_report(
    error: ArrowTransportError,
    rejections: &[CoordinateRejection<'_>],
) -> ArrowTransportError {
    if rejections.is_empty() {
        return error;
    }
    match coordinate_diagnostics_report(rejections) {
        Ok(mut report) => {
            report.completeness = RowDiagnosticsCompleteness::Partial;
            report.total = None;
            report.knowledge_limits = Some(vec!["data_tools.processing_interrupted".to_owned()]);
            error.with_row_diagnostics(report)
        }
        Err(build_error) => build_error,
    }
}

/// Chiude fail-closed quando una o piu' coordinate sono state rifiutate:
/// report completo e deterministico (conteggi esatti, esempi bounded).
fn reject_coordinate_rows(
    rejections: &[CoordinateRejection<'_>],
) -> Result<(), ArrowTransportError> {
    if rejections.is_empty() {
        return Ok(());
    }
    let report = coordinate_diagnostics_report(rejections)?;
    Err(ArrowTransportError::Geometry(
        "coordinate non conformi; consultare row_diagnostics".to_owned(),
    )
    .with_row_diagnostics(report))
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
    // BLOCK-06: doppia emissione delle chiavi canoniche §2 (parita' col v4,
    // DER-002 estesa) — post-processo centrale prima della codifica IPC.
    let (output_schema, output_batches) = canonical_legacy_output(output_schema, output_batches)?;
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

// ---------------------------------------------------------------------------
// Test del runner fuso (ADR-0012)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use geo::{line_string, polygon, LineString, MultiPoint, Point};

    use super::*;

    /// Parametri di una trasformazione 1:1 (tutti i default, CRS fissato).
    fn fused_params(operation: ArrowOperation) -> TransformArrowSchema {
        TransformArrowSchema {
            schema_version: TransformArrowSchema::VERSION,
            operation,
            row_count: 0,
            crs: Some("EPSG:32632".to_owned()),
            geometry_column: Some("geom".to_owned()),
            distance: None,
            cap: None,
            tolerance: None,
            simplify_policy: None,
            target_crs: None,
            max_output_rows: None,
            max_points: None,
            x_column: None,
            y_column: None,
            snap_tolerance: None,
            remove_overlaps: None,
            fill_gaps: None,
            coefficients: None,
            x_offset: None,
            y_offset: None,
            x_factor: None,
            y_factor: None,
            degrees: None,
            x_origin: None,
            y_origin: None,
            concavity: None,
            length_threshold: None,
            max_segment_length: None,
            grid_size: None,
            start_ratio: None,
            end_ratio: None,
            ratio: None,
            node_input: None,
            require_complete: None,
        }
    }

    fn wkb(geometry: &Geometry<f64>) -> Vec<u8> {
        geometry.to_wkb(CoordDimensions::xy()).expect("fixture wkb")
    }

    /// Fixture multi-tipo: punto, linea, poligono con buco, multipoint, null.
    fn multi_type_cells() -> Vec<Option<Vec<u8>>> {
        let point = Geometry::Point(Point::new(1.0, 2.0));
        let line = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0),
            (x: 10.0, y: 0.0),
            (x: 10.0, y: 5.0),
        ]);
        let holed = Geometry::Polygon(polygon!(
            exterior: [
                (x: 0.0, y: 0.0),
                (x: 20.0, y: 0.0),
                (x: 20.0, y: 20.0),
                (x: 0.0, y: 20.0),
                (x: 0.0, y: 0.0),
            ],
            interiors: [[
                (x: 5.0, y: 5.0),
                (x: 10.0, y: 5.0),
                (x: 5.0, y: 10.0),
                (x: 5.0, y: 5.0),
            ]],
        ));
        let multi = Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(3.0, 4.0),
        ]));
        vec![
            Some(wkb(&point)),
            Some(wkb(&line)),
            Some(wkb(&holed)),
            Some(wkb(&multi)),
            None,
        ]
    }

    fn cells_array(cells: &[Option<Vec<u8>>]) -> BinaryArray {
        cells
            .iter()
            .map(|cell| cell.as_deref())
            .collect::<BinaryArray>()
    }

    /// Minore 1 (ADR-0009 decisione 8): la colonna si identifica anche con
    /// le sole chiavi canoniche — l'estensione `geoarrow.wkb` e' ammessa,
    /// non richiesta. Difesa in profondita': i piani validati non arrivano
    /// qui con colonne non identificabili (il rifiuto e' in analyze).
    #[test]
    fn geometry_column_index_accepts_canonical_only_and_rejects_unmarked() {
        let canonical_only = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("geom", DataType::Binary, true).with_metadata(
                std::collections::HashMap::from([
                    (
                        plenora_kernels_geo::arrow_adapter::PLENORA_GEOMETRY_DIMENSIONS_KEY
                            .to_owned(),
                        "xy".to_owned(),
                    ),
                    (
                        plenora_kernels_geo::arrow_adapter::PLENORA_GEOMETRY_ENCODING_KEY
                            .to_owned(),
                        "wkb".to_owned(),
                    ),
                ]),
            ),
        ]);
        assert_eq!(
            geometry_column_index(&canonical_only, "geom").expect("canonica-only"),
            1
        );

        let bare = Schema::new(vec![Field::new("geom", DataType::Binary, true)]);
        assert!(matches!(
            geometry_column_index(&bare, "geom"),
            Err(ArrowTransportError::MissingGeoArrowMetadata(_))
        ));
    }

    /// Riferimento non fuso: `transform_cells` in sequenza, nodo per nodo.
    fn run_sequential(
        group: &[&TransformArrowSchema],
        cells: &BinaryArray,
    ) -> Result<Vec<Option<Vec<u8>>>, ArrowTransportError> {
        let mut current: Vec<Option<Vec<u8>>> =
            cells.iter().map(|cell| cell.map(<[u8]>::to_vec)).collect();
        for params in group {
            let array = cells_array(&current);
            let TransformedColumn::Binary(values) = transform_cells(params, &array)? else {
                return Err(ArrowTransportError::Internal(
                    "fixture: attesa colonna Binary",
                ));
            };
            current = values;
        }
        Ok(current)
    }

    fn run_fused(
        group: &[&TransformArrowSchema],
        cells: &BinaryArray,
    ) -> Result<Vec<Option<Vec<u8>>>, FusedStepError> {
        Ok(transform_cells_fused(group, None, cells, &mut |_| Ok(()))?.geometry)
    }

    /// Gruppo [buffer, simplify, centroid] (profili B, B, A): stesso output
    /// byte-per-byte del percorso non fuso, su fixture multi-tipo con null.
    #[test]
    fn fused_matches_sequential_buffer_simplify_centroid() {
        let mut buffer = fused_params(ArrowOperation::Buffer);
        buffer.distance = Some(1.0);
        let mut simplify = fused_params(ArrowOperation::Simplify);
        simplify.tolerance = Some(0.01);
        let centroid = fused_params(ArrowOperation::Centroid);
        let group: Vec<&TransformArrowSchema> = [&buffer, &simplify, &centroid].to_vec();

        let cells = cells_array(&multi_type_cells());
        let expected = run_sequential(&group, &cells).expect("percorso non fuso");
        let fused = run_fused(&group, &cells).expect("percorso fuso");
        assert_eq!(fused, expected, "output diverso byte-per-byte");
    }

    /// Gruppo [translate, scale, envelope] (profili B, B, A): copre il check
    /// inter-passo strutturale+OGC tra kernel di profilo B e il kernel di
    /// profilo A in chiusura.
    #[test]
    fn fused_matches_sequential_translate_scale_envelope() {
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(5.0);
        translate.y_offset = Some(-2.0);
        let mut scale = fused_params(ArrowOperation::Scale);
        scale.x_factor = Some(1.5);
        scale.y_factor = Some(0.5);
        let envelope = fused_params(ArrowOperation::Envelope);
        let group: Vec<&TransformArrowSchema> = [&translate, &scale, &envelope].to_vec();

        let cells = cells_array(&multi_type_cells());
        let expected = run_sequential(&group, &cells).expect("percorso non fuso");
        let fused = run_fused(&group, &cells).expect("percorso fuso");
        assert_eq!(fused, expected, "output diverso byte-per-byte");
    }

    /// Cella che eccede `MAX_CELL_BYTES` al kernel 1 di 3 (D12.3): densify
    /// produce esattamente `MAX_CELL_COORDINATES` coordinate (il cap del
    /// kernel passa) ma il WKB equivalente supera 64 MiB -> `CellTooLarge`
    /// attribuito al kernel che l'ha prodotta, come `encode_geometry`.
    // Il cast e' esatto (MAX_CELL_COORDINATES < 2^53): e' la lunghezza che
    // produce esattamente il cap di coordinate consentito dal kernel.
    #[allow(clippy::cast_precision_loss)]
    #[test]
    fn cell_too_large_is_attributed_to_the_producing_kernel() {
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(1.0);
        translate.y_offset = Some(0.0);
        let mut densify = fused_params(ArrowOperation::Densify);
        densify.max_segment_length = Some(16.0);
        let group: Vec<&TransformArrowSchema> = [&translate, &densify, &translate].to_vec();

        // Segmento lungo 16 * (MAX_CELL_COORDINATES - 1): il conteggio
        // densificato e' esattamente il cap del kernel (1 + ceil(L/s)),
        // quindi il kernel NON scatta; il WKB risultante e' 5 + 4 + 16 *
        // MAX_CELL_COORDINATES = 64 MiB + 9 byte -> oltre il limite.
        let length = 16.0 * (MAX_CELL_COORDINATES - 1) as f64;
        let line = Geometry::LineString(LineString::from(vec![(0.0, 0.0), (length, 0.0)]));
        let cells = cells_array(&[Some(wkb(&line))]);
        let expected_size = 5 + 4 + 16 * MAX_CELL_COORDINATES;

        let error = run_fused(&group, &cells).expect_err("cella oltre il limite");
        match error {
            FusedStepError::Kernel { index, error } => {
                assert_eq!(index, 1, "attribuzione al kernel che ha prodotto la cella");
                assert!(
                    matches!(error.source_error(), ArrowTransportError::CellTooLarge(size) if *size == expected_size),
                    "variante/misura diverse da encode_geometry: {error}"
                );
                // D12.3 + R9.9: la riga difettosa e' riportata con
                // diagnostica completa (indice batch-locale).
                let report = error.row_diagnostics().expect("diagnostica row-scoped");
                assert_eq!(report.observed_total, 1);
                assert_eq!(report.counts["geometry.cell_too_large"], 1);
                assert_eq!(report.examples[0].source_index, 0);
            }
            FusedStepError::Control(_) => panic!("atteso errore di kernel, trovato Control"),
            FusedStepError::Measure { .. } => panic!("atteso errore di kernel, trovato Measure"),
        }
    }

    /// Input malformato: l'errore di decode e' attribuito al PRIMO kernel
    /// del gruppo, con la stessa variante e lo stesso messaggio del percorso
    /// non fuso.
    #[test]
    fn malformed_input_is_attributed_to_the_first_kernel() {
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(1.0);
        translate.y_offset = Some(0.0);
        let centroid = fused_params(ArrowOperation::Centroid);
        let group: Vec<&TransformArrowSchema> = [&translate, &centroid].to_vec();

        let cells = cells_array(&[
            Some(wkb(&Geometry::Point(Point::new(0.0, 0.0)))),
            Some(vec![0x01, 0x09, 0x00]), // WKB troncato
        ]);
        let sequential_error = run_sequential(&group, &cells).expect_err("input malformato");
        let fused_error = run_fused(&group, &cells).expect_err("input malformato");
        match fused_error {
            FusedStepError::Kernel { index, error } => {
                assert_eq!(index, 0, "attribuzione al primo kernel del gruppo");
                assert_eq!(error.to_string(), sequential_error.to_string());
            }
            FusedStepError::Control(_) => panic!("atteso errore di kernel, trovato Control"),
            FusedStepError::Measure { .. } => panic!("atteso errore di kernel, trovato Measure"),
        }
    }

    /// Righe difettose nel percorso non fuso (`transform_cells`): la
    /// diagnostica row-scoped e' completa (conteggi esatti su tutte le
    /// righe, esempi bounded, indici batch-locali zero-based) e l'errore
    /// primario resta quello della prima riga difettosa.
    #[test]
    fn transform_cells_reports_complete_row_diagnostics() {
        let params = fused_params(ArrowOperation::Centroid);
        let cells = cells_array(&[
            Some(wkb(&Geometry::Point(Point::new(0.0, 0.0)))),
            Some(vec![0x01, 0x09, 0x00]), // WKB troncato
            None,
            Some(vec![0x02]), // endian flag invalida
        ]);
        let Err(error) = transform_cells(&params, &cells) else {
            panic!("celle malformate accettate");
        };
        let report = error.row_diagnostics().expect("diagnostica row-scoped");
        assert_eq!(report.observed_total, 2);
        assert_eq!(report.total, Some(2));
        assert_eq!(report.counts["geometry.invalid_wkb"], 2);
        assert_eq!(
            report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(report.validate_for_emission().is_ok());
    }

    /// Parita' D12.3/D12.4 estesa alla diagnostica: un fallimento per riga
    /// produce lo STESSO report nel percorso fuso e in quello sequenziale —
    /// stessa attribuzione (kernel 0 del gruppo == primo nodo), stesse
    /// cause, stessi indici, stesso errore primario.
    #[test]
    fn fused_and_sequential_report_identical_row_diagnostics() {
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(1.0);
        translate.y_offset = Some(0.0);
        let centroid = fused_params(ArrowOperation::Centroid);
        let group: Vec<&TransformArrowSchema> = [&translate, &centroid].to_vec();

        // Righe 1 e 3 con WKB malformato: il decode fallisce in entrambi i
        // percorsi, attribuito al primo kernel/nodo del gruppo.
        let cells = cells_array(&[
            Some(wkb(&Geometry::Point(Point::new(0.0, 0.0)))),
            Some(vec![0x01, 0x09, 0x00]), // WKB troncato
            Some(wkb(&Geometry::Point(Point::new(1.0, 1.0)))),
            Some(vec![0x02]), // endian flag invalida
        ]);
        let Err(sequential_error) = run_sequential(&group, &cells) else {
            panic!("WKB malformato accettato");
        };
        let fused_error = run_fused(&group, &cells).expect_err("WKB malformato");

        let sequential_report = sequential_error
            .row_diagnostics()
            .expect("diagnostica nel percorso non fuso");
        assert_eq!(sequential_report.observed_total, 2);
        assert_eq!(sequential_report.counts["geometry.invalid_wkb"], 2);
        assert_eq!(
            sequential_report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );

        let FusedStepError::Kernel { index, error } = fused_error else {
            panic!("atteso errore di kernel");
        };
        assert_eq!(index, 0, "attribuzione al primo kernel del gruppo");
        let fused_report = error
            .row_diagnostics()
            .unwrap_or_else(|| panic!("diagnostica nel percorso fuso: {error}"));
        assert_eq!(
            fused_report, sequential_report,
            "report diverso fra i percorsi"
        );
        assert_eq!(
            error.to_string(),
            sequential_error.to_string(),
            "errore primario diverso fra i percorsi"
        );
    }

    // -----------------------------------------------------------------------
    // Misura terminale del gruppo fuso (ADR-0012 M2)
    // -----------------------------------------------------------------------

    /// Riferimento non fuso della misura: il braccio misura di
    /// `transform_cells` sulle celle gia' trasformate in sequenza (stesso
    /// decode + stesso kernel scalare del ramo v4 `geo_measure_batch`, che
    /// usa gli stessi `plenora_kernels_geo::operations`).
    fn run_sequential_measure(
        group: &[&TransformArrowSchema],
        measure_params: &TransformArrowSchema,
        cells: &BinaryArray,
    ) -> Result<TransformedColumn, ArrowTransportError> {
        let transformed = run_sequential(group, cells)?;
        transform_cells(measure_params, &cells_array(&transformed))
    }

    fn run_fused_measured(
        group: &[&TransformArrowSchema],
        terminal: FusedTerminalMeasure,
        cells: &BinaryArray,
    ) -> Result<FusedCells, FusedStepError> {
        transform_cells_fused(group, Some(terminal), cells, &mut |_| Ok(()))
    }

    /// Gruppo [translate, simplify] + misura terminale `area` (M2): la
    /// geometria ri-encodata e la colonna misura sono identiche byte-per-byte
    /// al riferimento nodo-per-nodo, su fixture multi-tipo con null
    /// (null-in -> null-out sulla misura).
    #[test]
    fn fused_terminal_area_matches_sequential() {
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(5.0);
        translate.y_offset = Some(-2.0);
        let mut simplify = fused_params(ArrowOperation::Simplify);
        simplify.tolerance = Some(0.01);
        let group: Vec<&TransformArrowSchema> = [&translate, &simplify].to_vec();
        let area_params = fused_params(ArrowOperation::Area);

        let cells = cells_array(&multi_type_cells());
        let expected_geometry = run_sequential(&group, &cells).expect("sequenziale");
        let TransformedColumn::Float64(expected_area) =
            run_sequential_measure(&group, &area_params, &cells).expect("misura sequenziale")
        else {
            panic!("fixture: attesa colonna Float64");
        };
        let fused =
            run_fused_measured(&group, FusedTerminalMeasure::Area, &cells).expect("percorso fuso");
        assert_eq!(
            fused.geometry, expected_geometry,
            "geometria diversa byte-per-byte"
        );
        let Some(TransformedColumn::Float64(fused_area)) = fused.measure else {
            panic!("attesa colonna misura Float64");
        };
        assert_eq!(fused_area, expected_area, "misura diversa dal riferimento");
        assert!(fused_area[4].is_none(), "null-in -> null-out sulla misura");
    }

    /// Gruppo [translate, simplify] + misura terminale `to_wkt` (M2): parita'
    /// byte-per-byte della colonna Utf8 e della geometria di confine.
    #[test]
    fn fused_terminal_to_wkt_matches_sequential() {
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(5.0);
        translate.y_offset = Some(-2.0);
        let mut simplify = fused_params(ArrowOperation::Simplify);
        simplify.tolerance = Some(0.01);
        let group: Vec<&TransformArrowSchema> = [&translate, &simplify].to_vec();
        let wkt_params = fused_params(ArrowOperation::ToWkt);

        let cells = cells_array(&multi_type_cells());
        let expected_geometry = run_sequential(&group, &cells).expect("sequenziale");
        let TransformedColumn::Utf8(expected_wkt) =
            run_sequential_measure(&group, &wkt_params, &cells).expect("misura sequenziale")
        else {
            panic!("fixture: attesa colonna Utf8");
        };
        let fused =
            run_fused_measured(&group, FusedTerminalMeasure::ToWkt, &cells).expect("percorso fuso");
        assert_eq!(
            fused.geometry, expected_geometry,
            "geometria diversa byte-per-byte"
        );
        let Some(TransformedColumn::Utf8(fused_wkt)) = fused.measure else {
            panic!("attesa colonna misura Utf8");
        };
        assert_eq!(fused_wkt, expected_wkt, "misura diversa dal riferimento");
        assert!(fused_wkt[4].is_none(), "null-in -> null-out sulla misura");
    }

    /// Validazione pre-misura (D12.4 profilo B -> nodo misura): un intermedio
    /// OGC-invalido fallirebbe al decode del nodo misura nel percorso non
    /// fuso — il runner fuso lo rifiuta con la STESSA variante e lo STESSO
    /// messaggio di `check_geometry_valid` (nessun transito da
    /// `ArrowTransportError`, come `decode_geometry_cell` +
    /// `step_error`). Difesa in profondita': gli op di M1 non producono
    /// intermedi invalidi, quindi il trigger e' diretto su
    /// `apply_fused_measure` (stesso stato dei casi (d2)/(e) dell'oracolo).
    #[test]
    fn measure_validation_error_is_attributed_to_the_measure_node() {
        let bowtie = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 10.0, y: 10.0),
            (x: 10.0, y: 0.0),
            (x: 0.0, y: 10.0),
            (x: 0.0, y: 0.0),
        ]);
        let expected = check_geometry_valid(&bowtie).expect_err("bowtie OGC-invalido");
        let geometries = vec![Some(bowtie), None];
        let Err(error) = apply_fused_measure(FusedTerminalMeasure::VertexCount, &geometries, 2)
        else {
            panic!("misura su geometria invalida riuscita");
        };
        match error {
            FusedStepError::Measure { index, error } => {
                assert_eq!(index, 2, "attribuzione al nodo misura");
                assert_eq!(
                    error.to_string(),
                    expected.to_string(),
                    "forma del non fuso"
                );
            }
            FusedStepError::Kernel { .. } => panic!("atteso Measure, trovato Kernel"),
            FusedStepError::Control(_) => panic!("atteso Measure, trovato Control"),
        }
    }

    // -----------------------------------------------------------------------
    // Perimetro M3: make_valid / reproject (backend feature-gated)
    // -----------------------------------------------------------------------

    /// Poligono a farfalla: strutturalmente ben formato ma OGC-invalido.
    #[cfg(feature = "geos-backend")]
    fn bowtie_geometry() -> Geometry<f64> {
        Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 10.0, y: 10.0),
            (x: 10.0, y: 0.0),
            (x: 0.0, y: 10.0),
            (x: 0.0, y: 0.0),
        ])
    }

    #[cfg(feature = "geos-backend")]
    #[test]
    fn fused_control_observes_cancellation_after_non_interruptible_make_valid() {
        let make_valid = fused_params(ArrowOperation::MakeValid);
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(1.0);
        translate.y_offset = Some(2.0);
        let group: Vec<&TransformArrowSchema> = [&make_valid, &translate].to_vec();
        let cells = cells_array(&[Some(wkb(&Geometry::Point(Point::new(3.0, 4.0))))]);
        let mut visited = Vec::new();

        let Err(error) = transform_cells_fused(&group, None, &cells, &mut |index| {
            visited.push(index);
            if index == 0 {
                // Il caller executor salta la cancellazione davanti al nodo
                // `NonInterruptible`; il kernel deve completare.
                Ok(())
            } else {
                Err(PlenoraError::Cancelled {
                    node: "t".to_owned(),
                    operation: "geo.translate".to_owned(),
                    execution_id: "exec-fused-control".to_owned(),
                    reason: "cancellazione richiesta".to_owned(),
                })
            }
        }) else {
            panic!("cancellazione non osservata al primo confine cooperativo");
        };

        assert_eq!(visited, vec![0, 1], "make_valid completa prima del cancel");
        match error {
            FusedStepError::Control(PlenoraError::Cancelled {
                node, operation, ..
            }) => {
                assert_eq!(node, "t");
                assert_eq!(operation, "geo.translate");
            }
            other => panic!("atteso Control(Cancelled), ottenuto {other:?}"),
        }
    }

    /// M3, trappola 1 — il caso centrale: `make_valid` in testa al gruppo
    /// riceve un input OGC-INVALIDO (il farfalla, che supera il solo gate
    /// strutturale). Il percorso fuso NON deve rifiutarlo al decode iniziale
    /// (nessun check OGC davanti a `make_valid`): riparato identico nei due
    /// percorsi, NESSUN errore.
    #[cfg(feature = "geos-backend")]
    #[test]
    fn fused_make_valid_first_accepts_ogc_invalid_input() {
        let make_valid = fused_params(ArrowOperation::MakeValid);
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(1.0);
        translate.y_offset = Some(2.0);
        let group: Vec<&TransformArrowSchema> = [&make_valid, &translate].to_vec();

        let cells = cells_array(&[
            Some(wkb(&bowtie_geometry())),
            Some(wkb(&Geometry::Point(Point::new(3.0, 4.0)))),
            None,
        ]);
        let expected = run_sequential(&group, &cells).expect("percorso non fuso ripara");
        let fused = run_fused(&group, &cells).expect("il fuso NON rifiuta l'OGC-invalido");
        assert_eq!(fused, expected, "riparazione diversa tra i percorsi");
        // L'output riparato e' OGC-valido in entrambi i percorsi.
        for cell in fused.iter().flatten() {
            geometry_from_wkb(cell).expect("output riparato valido");
        }
    }

    /// M3: `make_valid` UNICO kernel del runner (forma limite di un gruppo
    /// con sola misura a valle): nel percorso non fuso il nodo emette i byte
    /// WKB di GEOS (o il passthrough dell'input valido); il runner fuso
    /// ri-encoda la forma decodificata — i byte di confine devono coincidere
    /// (parita' GEOS/geozero sulla stessa geometria riparata).
    #[cfg(feature = "geos-backend")]
    #[test]
    fn fused_make_valid_boundary_bytes_match_the_geos_encoding() {
        let make_valid = fused_params(ArrowOperation::MakeValid);
        let group: Vec<&TransformArrowSchema> = [&make_valid].to_vec();

        let cells = cells_array(&[
            Some(wkb(&bowtie_geometry())),
            Some(wkb(&Geometry::Polygon(polygon![
                (x: 0.0, y: 0.0),
                (x: 4.0, y: 0.0),
                (x: 4.0, y: 4.0),
                (x: 0.0, y: 4.0),
                (x: 0.0, y: 0.0),
            ]))),
            None,
        ]);
        let expected = run_sequential(&group, &cells).expect("percorso non fuso");
        let fused = run_fused(&group, &cells).expect("percorso fuso");
        assert_eq!(
            fused, expected,
            "byte di confine diversi (GEOS vs canonico)"
        );
    }

    /// M3: `make_valid` a meta' catena con successore — la validazione
    /// inter-passo standard (strutturale + OGC) resta dopo la riparazione;
    /// su input valido `make_valid` e' un passthrough byte-identico.
    #[cfg(feature = "geos-backend")]
    #[test]
    fn fused_make_valid_mid_chain_matches_sequential() {
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(5.0);
        translate.y_offset = Some(-2.0);
        let make_valid = fused_params(ArrowOperation::MakeValid);
        let mut rotate = fused_params(ArrowOperation::Rotate);
        rotate.degrees = Some(15.0);
        let group: Vec<&TransformArrowSchema> = [&translate, &make_valid, &rotate].to_vec();

        let cells = cells_array(&multi_type_cells());
        let expected = run_sequential(&group, &cells).expect("percorso non fuso");
        let fused = run_fused(&group, &cells).expect("percorso fuso");
        assert_eq!(fused, expected, "output diverso byte-per-byte");
    }

    /// M3: catena con `reproject` (EPSG:32632 -> EPSG:4326) seguito da un
    /// altro transform — stesso output byte-per-byte (le guardie del kernel
    /// PROJ si applicano identiche sulla forma decodificata).
    #[cfg(feature = "proj-backend")]
    #[test]
    fn fused_reproject_matches_sequential() {
        let mut reproject = fused_params(ArrowOperation::Reproject);
        reproject.target_crs = Some("EPSG:4326".to_owned());
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(0.001);
        translate.y_offset = Some(-0.002);
        let group: Vec<&TransformArrowSchema> = [&reproject, &translate].to_vec();

        let cells = cells_array(&multi_type_cells());
        let expected = run_sequential(&group, &cells).expect("percorso non fuso");
        let fused = run_fused(&group, &cells).expect("percorso fuso");
        assert_eq!(fused, expected, "output diverso byte-per-byte");
    }

    /// M3 a feature spenta: `make_valid` in un gruppo da' lo STESSO
    /// `BackendUnavailable` del percorso non fuso, attribuito al suo kernel
    /// (difesa in profondita': i piani con `make_valid` sono gia' rifiutati
    /// in validazione senza la feature).
    #[cfg(not(feature = "geos-backend"))]
    #[test]
    fn fused_make_valid_backend_unavailable_matches_sequential() {
        let make_valid = fused_params(ArrowOperation::MakeValid);
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(1.0);
        translate.y_offset = Some(2.0);
        let group: Vec<&TransformArrowSchema> = [&make_valid, &translate].to_vec();

        let cells = cells_array(&[Some(wkb(&Geometry::Point(Point::new(1.0, 2.0))))]);
        let sequential_error = run_sequential(&group, &cells).expect_err("backend assente");
        assert!(matches!(
            sequential_error,
            ArrowTransportError::BackendUnavailable { .. }
        ));
        match run_fused(&group, &cells).expect_err("backend assente") {
            FusedStepError::Kernel { index, error } => {
                assert_eq!(index, 0, "attribuzione al kernel make_valid");
                assert_eq!(error.to_string(), sequential_error.to_string());
            }
            FusedStepError::Control(_) => panic!("atteso errore di kernel, trovato Control"),
            FusedStepError::Measure { .. } => panic!("atteso errore di kernel, trovato Measure"),
        }
    }

    /// M3 a feature spenta: `reproject` a meta' gruppo da' lo STESSO
    /// `BackendUnavailable` del percorso non fuso, attribuito al suo kernel.
    #[cfg(not(feature = "proj-backend"))]
    #[test]
    fn fused_reproject_backend_unavailable_matches_sequential() {
        let mut translate = fused_params(ArrowOperation::Translate);
        translate.x_offset = Some(1.0);
        translate.y_offset = Some(2.0);
        let reproject = fused_params(ArrowOperation::Reproject);
        let group: Vec<&TransformArrowSchema> = [&translate, &reproject].to_vec();

        let cells = cells_array(&[Some(wkb(&Geometry::Point(Point::new(1.0, 2.0))))]);
        let sequential_error = run_sequential(&group, &cells).expect_err("backend assente");
        assert!(matches!(
            sequential_error,
            ArrowTransportError::BackendUnavailable { .. }
        ));
        match run_fused(&group, &cells).expect_err("backend assente") {
            FusedStepError::Kernel { index, error } => {
                assert_eq!(index, 1, "attribuzione al kernel reproject");
                assert_eq!(error.to_string(), sequential_error.to_string());
            }
            FusedStepError::Control(_) => panic!("atteso errore di kernel, trovato Control"),
            FusedStepError::Measure { .. } => panic!("atteso errore di kernel, trovato Measure"),
        }
    }
}
