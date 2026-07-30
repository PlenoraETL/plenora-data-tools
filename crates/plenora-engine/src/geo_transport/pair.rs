//! Percorso pair del trasporto Arrow v3: operazioni binarie su due envelope
//! (left/right), schema `PairArrowSchema`, lineage e pipeline `pair_arrow`.

use std::io::{Read, Write};

use plenora_core::arrow::array::{BinaryArray, Float64Array, RecordBatch, UInt64Array};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use geo::{CoordsIter, Geometry};
use rayon::prelude::*;
use serde::Deserialize;

use plenora_kernels_geo::analysis::{
    count_points_in_polygons, minimum_distances, nearest_matches, within_indexes,
};
use plenora_kernels_geo::extended::{geodesic_distance_m, hausdorff_distance, haversine_distance_m};
#[cfg(feature = "geos-backend")]
use plenora_kernels_geo::extended_algorithms::split_line;
use plenora_kernels_geo::extended_algorithms::{frechet_distance, geodesic_bearing_degrees};
#[cfg(feature = "geos-backend")]
use plenora_kernels_geo::geos_backend::split_polygon_by_linework;
use super::pair_protocol::MAX_PAIRS;
use plenora_kernels_geo::predicates::{evaluate as evaluate_predicate, SpatialPredicate};
use super::protocol::MAX_ROWS;
use plenora_kernels_geo::spatial_join::{spatial_join_nullable, JoinPredicate};
use plenora_kernels_geo::topology::{
    boolean_operation, clip_to_mask, polygon_overlay, BooleanOperation, OverlayMode,
};
use plenora_kernels_geo::geometry_from_wkb;

use super::envelope::{EnvelopeReader, EnvelopeWriter};
use super::error::ArrowTransportError;
use super::ipc::{decode_ipc, encode_ipc};
use super::transport::{
    COUNT_COLUMN, DEFAULT_GEOMETRY_COLUMN, DISTANCE_COLUMN, LEFT_INDEX_COLUMN, MAX_CELL_BYTES,
    RIGHT_INDEX_COLUMN, WITHIN_COLUMN,
};
#[cfg(feature = "geos-backend")]
use super::transport::{MAX_CELL_COORDINATES, MAX_NODING_WORK, MAX_SPLIT_WORK, PARENT_INDEX_COLUMN};
use super::unary::{
    batch_geometry_cells, canonical_legacy_output, encode_geometry, expect_line_string,
    expect_point, geometry_column_index, geometry_output_field, spatial_predicate_name,
};
#[cfg(feature = "geos-backend")]
use super::unary::geometry_type_name;

// --- Forma binary + lineage (Fase C) ---------------------------------------

/// Operazioni binarie su due envelope v3 (left/right).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PairOperation {
    #[serde(rename = "sjoin")]
    SJoin,
    Distance,
    Nearest,
    Clip,
    Overlay,
    Within,
    CountPointsInPolygons,
    Intersection,
    Union,
    Difference,
    SymmetricDifference,
    Predicate,
    HausdorffDistance,
    FrechetDistance,
    HaversineDistance,
    GeodesicDistance,
    Bearing,
    Split,
}

impl PairOperation {
    pub const ALL: [Self; 18] = [
        Self::SJoin,
        Self::Distance,
        Self::Nearest,
        Self::Clip,
        Self::Overlay,
        Self::Within,
        Self::CountPointsInPolygons,
        Self::Intersection,
        Self::Union,
        Self::Difference,
        Self::SymmetricDifference,
        Self::Predicate,
        Self::HausdorffDistance,
        Self::FrechetDistance,
        Self::HaversineDistance,
        Self::GeodesicDistance,
        Self::Bearing,
        Self::Split,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SJoin => "sjoin",
            Self::Distance => "distance",
            Self::Nearest => "nearest",
            Self::Clip => "clip",
            Self::Overlay => "overlay",
            Self::Within => "within",
            Self::CountPointsInPolygons => "count_points_in_polygons",
            Self::Intersection => "intersection",
            Self::Union => "union",
            Self::Difference => "difference",
            Self::SymmetricDifference => "symmetric_difference",
            Self::Predicate => "predicate",
            Self::HausdorffDistance => "hausdorff_distance",
            Self::FrechetDistance => "frechet_distance",
            Self::HaversineDistance => "haversine_distance",
            Self::GeodesicDistance => "geodesic_distance",
            Self::Bearing => "bearing",
            Self::Split => "split",
        }
    }

    /// Nome della voce di catalogo usata dal livello comandi per il requisito CRS.
    #[must_use]
    pub const fn catalog_name(self) -> &'static str {
        match self {
            Self::SJoin => "sjoin",
            Self::Distance => "geo_distance",
            Self::Nearest => "geo_nearest",
            Self::Clip => "geo_clip",
            Self::Overlay => "geo_overlay",
            Self::Within => "geo_within",
            Self::CountPointsInPolygons => "geo_count_points_in_polygons",
            Self::Intersection => "geo_intersection",
            Self::Union => "geo_union",
            Self::Difference => "geo_difference",
            Self::SymmetricDifference => "geo_symmetric_difference",
            // Tutti i predicati DE-9IM condividono famiglia e requisito CRS.
            Self::Predicate => "predicate_intersects",
            Self::HausdorffDistance => "hausdorff_distance",
            Self::FrechetDistance => "frechet_distance",
            Self::HaversineDistance => "haversine_distance",
            Self::GeodesicDistance => "geodesic_distance",
            Self::Bearing => "bearing",
            Self::Split => "split",
        }
    }

    const fn boolean_kernel(self) -> Option<BooleanOperation> {
        match self {
            Self::Intersection => Some(BooleanOperation::Intersection),
            Self::Union => Some(BooleanOperation::Union),
            Self::Difference => Some(BooleanOperation::Difference),
            Self::SymmetricDifference => Some(BooleanOperation::SymmetricDifference),
            _ => None,
        }
    }
}

/// Schema JSON del comando pair-arrow (`schema_version: 3`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairArrowSchema {
    pub schema_version: u32,
    pub operation: PairOperation,
    pub left_row_count: u64,
    pub right_row_count: u64,
    pub left_crs: Option<String>,
    pub right_crs: Option<String>,
    pub geometry_column: Option<String>,
    pub predicate: Option<JoinPredicate>,
    pub overlay_mode: Option<OverlayMode>,
    pub max_pairs: Option<u64>,
    pub max_comparisons: Option<u64>,
    pub max_results: Option<u64>,
    pub max_distance: Option<f64>,
    pub max_output_rows: Option<u64>,
    pub spatial_predicate: Option<SpatialPredicate>,
    pub max_coordinate_pairs: Option<u64>,
    pub tolerance: Option<f64>,
}

impl PairArrowSchema {
    pub const VERSION: u32 = 3;

    #[must_use]
    pub fn geometry_column(&self) -> &str {
        self.geometry_column
            .as_deref()
            .unwrap_or(DEFAULT_GEOMETRY_COLUMN)
    }

    #[must_use]
    pub fn max_output_rows_limit(&self) -> u64 {
        self.max_output_rows.unwrap_or(MAX_ROWS)
    }

    /// Verifica che i parametri presenti siano esattamente quelli previsti
    /// dall'operazione e che i valori siano nel dominio del kernel.
    ///
    /// # Errors
    ///
    /// `ArrowTransportError::UnexpectedParameter` se e' presente un parametro
    /// non previsto dall'operazione, `ArrowTransportError::MissingParameter`
    /// se ne manca uno obbligatorio, `ArrowTransportError::InvalidParameter`
    /// se un valore e' fuori dominio.
    // Tabella parametri per operazione intenzionalmente in un'unica funzione;
    // la scomposizione strutturale e' rimandata a una fase dedicata.
    #[allow(clippy::too_many_lines)]
    pub fn validate_parameters(&self) -> Result<(), ArrowTransportError> {
        let operation = self.operation.name();
        // Parametri ammessi per operazione: tutto il resto e' rifiutato
        // prima di toccare i dati.
        let allowed: &[&'static str] = match self.operation {
            PairOperation::SJoin => &["predicate", "max_pairs"],
            PairOperation::Distance => &["max_comparisons"],
            PairOperation::Nearest => &["max_comparisons", "max_results", "max_distance"],
            PairOperation::Overlay => &["overlay_mode", "max_pairs"],
            PairOperation::Within | PairOperation::CountPointsInPolygons => &["max_pairs"],
            PairOperation::Predicate => &["spatial_predicate"],
            PairOperation::HausdorffDistance | PairOperation::FrechetDistance => {
                &["max_coordinate_pairs"]
            }
            PairOperation::Split => &["tolerance"],
            // Operazioni senza parametri.
            PairOperation::Clip
            | PairOperation::Intersection
            | PairOperation::Union
            | PairOperation::Difference
            | PairOperation::SymmetricDifference
            | PairOperation::HaversineDistance
            | PairOperation::GeodesicDistance
            | PairOperation::Bearing => &[],
        };
        let mut present: Vec<(&'static str, bool)> = vec![
            ("predicate", self.predicate.is_some()),
            ("overlay_mode", self.overlay_mode.is_some()),
            ("max_pairs", self.max_pairs.is_some()),
            ("max_comparisons", self.max_comparisons.is_some()),
            ("max_results", self.max_results.is_some()),
            ("max_distance", self.max_distance.is_some()),
            ("spatial_predicate", self.spatial_predicate.is_some()),
            ("max_coordinate_pairs", self.max_coordinate_pairs.is_some()),
            ("tolerance", self.tolerance.is_some()),
        ];
        present.retain(|(_, is_present)| *is_present);
        for (name, _) in &present {
            if !allowed.contains(name) {
                return Err(ArrowTransportError::UnexpectedParameter { operation, name });
            }
        }
        let required = |name: &'static str, value: Option<u64>| {
            value.ok_or(ArrowTransportError::MissingParameter { operation, name })
        };
        let positive = |name: &'static str, value: u64| {
            if value == 0 {
                Err(ArrowTransportError::InvalidParameter {
                    operation,
                    name,
                    reason: "deve essere maggiore di zero",
                })
            } else {
                Ok(value)
            }
        };
        match self.operation {
            PairOperation::SJoin => {
                if self.predicate.is_none() {
                    return Err(ArrowTransportError::MissingParameter {
                        operation,
                        name: "predicate",
                    });
                }
                let max_pairs = positive("max_pairs", required("max_pairs", self.max_pairs)?)?;
                if max_pairs > MAX_PAIRS {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "max_pairs",
                        reason: "oltre il limite del protocollo coppie",
                    });
                }
            }
            PairOperation::Distance => {
                positive(
                    "max_comparisons",
                    required("max_comparisons", self.max_comparisons)?,
                )?;
            }
            PairOperation::Nearest => {
                positive(
                    "max_comparisons",
                    required("max_comparisons", self.max_comparisons)?,
                )?;
                positive("max_results", required("max_results", self.max_results)?)?;
                if self
                    .max_distance
                    .is_some_and(|value| !value.is_finite() || value < 0.0)
                {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "max_distance",
                        reason: "deve essere finita e non negativa",
                    });
                }
            }
            PairOperation::Overlay => {
                if self.overlay_mode.is_none() {
                    return Err(ArrowTransportError::MissingParameter {
                        operation,
                        name: "overlay_mode",
                    });
                }
                positive("max_pairs", required("max_pairs", self.max_pairs)?)?;
            }
            PairOperation::Within | PairOperation::CountPointsInPolygons => {
                let max_pairs = positive("max_pairs", required("max_pairs", self.max_pairs)?)?;
                if max_pairs > MAX_PAIRS {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "max_pairs",
                        reason: "oltre il limite del protocollo coppie",
                    });
                }
            }
            PairOperation::Predicate => {
                if self.spatial_predicate.is_none() {
                    return Err(ArrowTransportError::MissingParameter {
                        operation,
                        name: "spatial_predicate",
                    });
                }
            }
            PairOperation::HausdorffDistance | PairOperation::FrechetDistance => {
                positive(
                    "max_coordinate_pairs",
                    required("max_coordinate_pairs", self.max_coordinate_pairs)?,
                )?;
            }
            PairOperation::Split => {
                if self
                    .tolerance
                    .is_some_and(|value| !value.is_finite() || value < 0.0)
                {
                    return Err(ArrowTransportError::InvalidParameter {
                        operation,
                        name: "tolerance",
                        reason: "deve essere finita e non negativa",
                    });
                }
            }
            _ => {}
        }
        if let Some(limit) = self.max_output_rows {
            if limit > MAX_ROWS {
                return Err(ArrowTransportError::InvalidParameter {
                    operation,
                    name: "max_output_rows",
                    reason: "oltre il limite righe del trasporto",
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct PairArrowSummary {
    pub left_rows: u64,
    pub right_rows: u64,
    pub output_rows: u64,
    pub checksum: [u8; 32],
}

/// Lato di una coppia decodificato: schema, batch IPC e geometrie validate.
type DecodedSide = (SchemaRef, Vec<RecordBatch>, Vec<Option<Geometry<f64>>>);

/// Decodifica un lato (envelope + IPC + colonna geometria) e materializza le
/// geometrie validate: entrambi i lati sono verificati prima del calcolo.
fn decode_geometry_side(
    reader: impl Read,
    expected_rows: u64,
    geometry_column: &str,
    side: &'static str,
) -> Result<DecodedSide, ArrowTransportError> {
    if expected_rows > MAX_ROWS {
        return Err(ArrowTransportError::TooManyRows(expected_rows));
    }
    let payload = EnvelopeReader::new(reader)?.read_payload()?;
    let (schema, batches) = decode_ipc(&payload)?;
    let rows: u64 = batches.iter().map(|batch| batch.num_rows() as u64).sum();
    if rows != expected_rows {
        return Err(ArrowTransportError::PairRowCountMismatch {
            side,
            schema: expected_rows,
            stream: rows,
        });
    }
    let geometry_index = geometry_column_index(&schema, geometry_column)?;
    let mut geometries = Vec::with_capacity(
        usize::try_from(rows).map_err(|_| ArrowTransportError::TooManyRows(rows))?,
    );
    for batch in &batches {
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
    Ok((schema, batches, geometries))
}

fn lineage_schema(nullable: bool, with_distance: bool) -> Schema {
    let mut fields = vec![
        Field::new(LEFT_INDEX_COLUMN, DataType::UInt64, nullable),
        Field::new(RIGHT_INDEX_COLUMN, DataType::UInt64, nullable),
    ];
    if with_distance {
        fields.push(Field::new(DISTANCE_COLUMN, DataType::Float64, false));
    }
    // Dataset derivato (indici di coppia, valori da entrambe le sorgenti):
    // per R2.4 NON si ereditano i metadati di schema degli input —
    // descriverebbero il risultato con le proprieta' della sorgente
    // (stessa classe di `reconcile` in plenora-kernels-table/analyze.rs).
    Schema::new(fields)
}

fn pairs_batch(
    pairs: &[(u64, u64, Option<f64>)],
    schema: &Schema,
) -> Result<RecordBatch, ArrowTransportError> {
    let left = UInt64Array::from_iter_values(pairs.iter().map(|pair| pair.0));
    let right = UInt64Array::from_iter_values(pairs.iter().map(|pair| pair.1));
    let mut columns: Vec<plenora_core::arrow::array::ArrayRef> =
        vec![std::sync::Arc::new(left), std::sync::Arc::new(right)];
    if schema.fields().len() == 3 {
        let distances: Vec<f64> = pairs
            .iter()
            .map(|pair| {
                pair.2
                    .ok_or(ArrowTransportError::Internal("distance mancante"))
            })
            .collect::<Result<_, _>>()?;
        columns.push(std::sync::Arc::new(Float64Array::from_iter_values(
            distances,
        )));
    }
    RecordBatch::try_new(std::sync::Arc::new(schema.clone()), columns)
        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))
}

/// Colonna scalare allineata alle righe left da accodare agli attributi.
enum AppendedColumn {
    Boolean(Vec<Option<bool>>),
    UInt64(Vec<Option<u64>>),
    Float64(Vec<Option<f64>>),
}

/// Accoda una colonna scalare alle colonne left preservando i batch.
fn append_column_batches(
    left_schema: &SchemaRef,
    left_batches: &[RecordBatch],
    field: Field,
    values: &AppendedColumn,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let mut output_fields: Vec<Field> = left_schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields.push(field);
    let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
        output_fields,
        left_schema.metadata().clone(),
    ));
    let mut out_batches = Vec::with_capacity(left_batches.len());
    let mut offset = 0_usize;
    for batch in left_batches {
        let end = offset + batch.num_rows();
        let column: plenora_core::arrow::array::ArrayRef = match values {
            AppendedColumn::Boolean(values) => std::sync::Arc::new(
                plenora_core::arrow::array::BooleanArray::from(values[offset..end].to_vec()),
            ),
            AppendedColumn::UInt64(values) => {
                std::sync::Arc::new(UInt64Array::from(values[offset..end].to_vec()))
            }
            AppendedColumn::Float64(values) => {
                std::sync::Arc::new(Float64Array::from(values[offset..end].to_vec()))
            }
        };
        offset = end;
        let mut columns = batch.columns().to_vec();
        columns.push(column);
        out_batches.push(
            RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((out_schema, out_batches))
}

/// Sostituisce la colonna geometria left con i valori dati (null inclusi).
fn replace_geometry_batches(
    left_schema: &SchemaRef,
    left_batches: &[RecordBatch],
    geometry_column: &str,
    output_crs: &str,
    values: &[Option<Vec<u8>>],
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowTransportError> {
    let geometry_index = geometry_column_index(left_schema, geometry_column)?;
    let mut output_fields: Vec<Field> = left_schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
    let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
        output_fields,
        left_schema.metadata().clone(),
    ));
    let mut out_batches = Vec::with_capacity(left_batches.len());
    let mut offset = 0_usize;
    for batch in left_batches {
        let end = offset + batch.num_rows();
        let mut columns = batch.columns().to_vec();
        columns[geometry_index] = std::sync::Arc::new(
            values[offset..end]
                .iter()
                .map(|value| value.as_deref())
                .collect::<BinaryArray>(),
        );
        offset = end;
        out_batches.push(
            RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
        );
    }
    Ok((out_schema, out_batches))
}

/// Pipeline pair-arrow: due envelope v3 -> kernel binario -> envelope v3.
/// I CRS sono trattati come metadati opachi; l'uguaglianza semantica e'
/// verificata dal livello comandi.
///
/// # Errors
///
/// - `ArrowTransportError::UnsupportedSchemaVersion`: `schema_version` non
///   supportata;
/// - `ArrowTransportError::CrsRequired`: `left_crs` o `right_crs` assente;
/// - come `PairArrowSchema::validate_parameters` per i parametri;
/// - `ArrowTransportError::OutputRowsExceeded`: righe di output oltre
///   `max_output_rows`;
/// - `ArrowTransportError::SideLengthMismatch`: `row_count` left/right diversi
///   per le operazioni allineate per riga;
/// - `ArrowTransportError::Internal`: invariante interna violata (parametro
///   gia' validato assente — difetto del trasporto, non dell'input);
/// - propaga gli errori di decodifica dei due lati (envelope, IPC, limiti,
///   validazione WKB), dei kernel binari e di codifica dell'output
///   (`encode_ipc`, `EnvelopeWriter`).
// Pipeline unica su tutte le PairOperation: la lunghezza e' data dalla
// sequenza lineare dei casi del dispatcher sul contratto v3, non da
// complessita' logica (fase di pulizia: niente refactor strutturali).
#[allow(clippy::too_many_lines)]
pub fn pair_arrow(
    left_reader: impl Read,
    right_reader: impl Read,
    writer: impl Write,
    schema: &PairArrowSchema,
) -> Result<PairArrowSummary, ArrowTransportError> {
    if schema.schema_version != PairArrowSchema::VERSION {
        return Err(ArrowTransportError::UnsupportedSchemaVersion(
            schema.schema_version,
        ));
    }
    schema.validate_parameters()?;
    if schema.left_crs.is_none() || schema.right_crs.is_none() {
        return Err(ArrowTransportError::CrsRequired);
    }
    let geometry_column = schema.geometry_column();
    let (left_schema, left_batches, left) =
        decode_geometry_side(left_reader, schema.left_row_count, geometry_column, "left")?;
    let (_right_schema, _right_batches, right) = decode_geometry_side(
        right_reader,
        schema.right_row_count,
        geometry_column,
        "right",
    )?;
    let left_rows = schema.left_row_count;
    let right_rows = schema.right_row_count;
    let limit = schema.max_output_rows_limit();

    let (output_schema, output_batches): (SchemaRef, Vec<RecordBatch>) = match schema.operation {
        PairOperation::SJoin => {
            let pairs = spatial_join_nullable(
                &left,
                &right,
                schema
                    .predicate
                    .ok_or(ArrowTransportError::Internal("predicate validato assente"))?,
                schema
                    .max_pairs
                    .ok_or(ArrowTransportError::Internal("max_pairs validato assente"))?,
            )?;
            if pairs.len() as u64 > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: pairs.len() as u64,
                    limit,
                });
            }
            let out_schema = std::sync::Arc::new(lineage_schema(false, false));
            let flat: Vec<(u64, u64, Option<f64>)> = pairs
                .iter()
                .map(|pair| (pair.left, pair.right, None))
                .collect();
            let batch = pairs_batch(&flat, &out_schema)?;
            (out_schema, vec![batch])
        }
        PairOperation::Distance => {
            let distances = minimum_distances(
                &left,
                &right,
                schema
                    .max_comparisons
                    .ok_or(ArrowTransportError::Internal("max_comparisons validato assente"))?,
            )?;
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            // Output allineato a left: colonne invariate, `distance` in coda.
            let mut output_fields: Vec<Field> = left_schema
                .fields()
                .iter()
                .map(|field| field.as_ref().clone())
                .collect();
            output_fields.push(Field::new(DISTANCE_COLUMN, DataType::Float64, true));
            let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
                output_fields,
                left_schema.metadata().clone(),
            ));
            let mut out_batches = Vec::with_capacity(left_batches.len());
            let mut offset = 0_usize;
            for batch in &left_batches {
                let values: Vec<Option<f64>> =
                    distances[offset..offset + batch.num_rows()].to_vec();
                offset += batch.num_rows();
                let mut columns = batch.columns().to_vec();
                columns.push(std::sync::Arc::new(Float64Array::from(values)));
                out_batches.push(
                    RecordBatch::try_new(out_schema.clone(), columns)
                        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
                );
            }
            (out_schema, out_batches)
        }
        PairOperation::Nearest => {
            let matches = nearest_matches(
                &left,
                &right,
                schema.max_distance,
                schema
                    .max_comparisons
                    .ok_or(ArrowTransportError::Internal("max_comparisons validato assente"))?,
                schema
                    .max_results
                    .ok_or(ArrowTransportError::Internal("max_results validato assente"))?,
            )?;
            if matches.len() as u64 > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: matches.len() as u64,
                    limit,
                });
            }
            let out_schema = std::sync::Arc::new(lineage_schema(false, true));
            let flat: Vec<(u64, u64, Option<f64>)> = matches
                .iter()
                .map(|m| (m.left, m.right, Some(m.distance)))
                .collect();
            let batch = pairs_batch(&flat, &out_schema)?;
            (out_schema, vec![batch])
        }
        PairOperation::Clip => {
            let masks: Vec<Geometry<f64>> = right.into_iter().flatten().collect();
            let mut left_values: Vec<Geometry<f64>> = Vec::new();
            let mut positions: Vec<u64> = Vec::new();
            for (index, geometry) in left.iter().enumerate() {
                if let Some(geometry) = geometry {
                    left_values.push(geometry.clone());
                    positions.push(index as u64);
                }
            }
            let clipped = clip_to_mask(&left_values, &masks)?;
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let output_crs = schema
                .left_crs
                .as_deref()
                .ok_or(ArrowTransportError::Internal("left_crs validato assente"))?;
            let mut output_fields: Vec<Field> = left_schema
                .fields()
                .iter()
                .map(|field| field.as_ref().clone())
                .collect();
            let geometry_index = geometry_column_index(&left_schema, geometry_column)?;
            output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
            let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
                output_fields,
                left_schema.metadata().clone(),
            ));
            let mut encoded: Vec<Option<Vec<u8>>> = Vec::with_capacity(clipped.len());
            for geometry in &clipped {
                encoded.push(
                    geometry
                        .as_ref()
                        .map(encode_geometry)
                        .transpose()?,
                );
            }
            let mut out_batches = Vec::with_capacity(left_batches.len());
            let mut cursor = 0_usize;
            let mut row_offset = 0_u64;
            for batch in &left_batches {
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
                out_batches.push(
                    RecordBatch::try_new(out_schema.clone(), columns)
                        .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
                );
            }
            (out_schema, out_batches)
        }
        PairOperation::Overlay => {
            let filter = |geometries: &[Option<Geometry<f64>>]| {
                let mut values = Vec::new();
                let mut positions = Vec::new();
                for (index, geometry) in geometries.iter().enumerate() {
                    if let Some(geometry) = geometry {
                        values.push(geometry.clone());
                        positions.push(index as u64);
                    }
                }
                (values, positions)
            };
            let (left_values, left_positions) = filter(&left);
            let (right_values, right_positions) = filter(&right);
            let pieces = polygon_overlay(
                &left_values,
                &right_values,
                schema
                    .overlay_mode
                    .ok_or(ArrowTransportError::Internal("overlay_mode validato assente"))?,
                schema
                    .max_pairs
                    .ok_or(ArrowTransportError::Internal("max_pairs validato assente"))?,
                limit,
            )?;
            let output_crs = schema
                .left_crs
                .as_deref()
                .ok_or(ArrowTransportError::Internal("left_crs validato assente"))?;
            // Schema del dataset derivato (pezzi dell'overlay): per R2.4
            // NON si ereditano i metadati di schema degli input (stessa
            // classe di `reconcile`); la geometria prodotta e' un campo
            // derivato e non eredita le chiavi canoniche dell'ingresso —
            // l'emissione canonica resta nel percorso v4
            // (`canonical_output_schema`).
            let out_schema = std::sync::Arc::new(Schema::new(vec![
                geometry_output_field(geometry_column, output_crs)?,
                Field::new(LEFT_INDEX_COLUMN, DataType::UInt64, true),
                Field::new(RIGHT_INDEX_COLUMN, DataType::UInt64, true),
            ]));
            let mut encoded: Vec<Vec<u8>> = Vec::with_capacity(pieces.len());
            let mut left_index: Vec<Option<u64>> = Vec::with_capacity(pieces.len());
            let mut right_index: Vec<Option<u64>> = Vec::with_capacity(pieces.len());
            for piece in &pieces {
                encoded.push(encode_geometry(&piece.geometry)?);
                // Indici prodotti dal kernel overlay: la conversione e'
                // totale; un u64 che non entra in usize (target a 32 bit)
                // e' un difetto del kernel, non un valore da troncare.
                left_index.push(match piece.left {
                    Some(index) => Some(left_positions[usize::try_from(index).map_err(
                        |_| ArrowTransportError::Internal("indice overlay left oltre usize"),
                    )?]),
                    None => None,
                });
                right_index.push(match piece.right {
                    Some(index) => Some(right_positions[usize::try_from(index).map_err(
                        |_| ArrowTransportError::Internal("indice overlay right oltre usize"),
                    )?]),
                    None => None,
                });
            }
            let batch = RecordBatch::try_new(
                out_schema.clone(),
                vec![
                    std::sync::Arc::new(
                        encoded
                            .iter()
                            .map(|value| Some(value.as_slice()))
                            .collect::<BinaryArray>(),
                    ),
                    std::sync::Arc::new(UInt64Array::from(left_index)),
                    std::sync::Arc::new(UInt64Array::from(right_index)),
                ],
            )
            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
            (out_schema, vec![batch])
        }
        PairOperation::Within => {
            let indexes = within_indexes(
                &left,
                &right,
                schema
                    .max_pairs
                    .ok_or(ArrowTransportError::Internal("max_pairs validato assente"))?,
            )?;
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let matched: std::collections::HashSet<u64> = indexes.into_iter().collect();
            let flags: Vec<Option<bool>> = left
                .iter()
                .enumerate()
                .map(|(index, geometry)| {
                    geometry.as_ref().map(|_| matched.contains(&(index as u64)))
                })
                .collect();
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(WITHIN_COLUMN, DataType::Boolean, true),
                &AppendedColumn::Boolean(flags),
            )?
        }
        PairOperation::CountPointsInPolygons => {
            // Contratto: left = poligoni (output allineato), right = punti.
            let counts = count_points_in_polygons(
                &left,
                &right,
                schema
                    .max_pairs
                    .ok_or(ArrowTransportError::Internal("max_pairs validato assente"))?,
            )?;
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let values: Vec<Option<u64>> = counts
                .iter()
                .enumerate()
                .map(|(index, count)| left[index].as_ref().map(|_| *count))
                .collect();
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(COUNT_COLUMN, DataType::UInt64, true),
                &AppendedColumn::UInt64(values),
            )?
        }
        operation @ (PairOperation::Intersection
        | PairOperation::Union
        | PairOperation::Difference
        | PairOperation::SymmetricDifference) => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let kernel = operation
                .boolean_kernel()
                .ok_or(ArrowTransportError::Internal("booleana pairwise senza kernel"))?;
            // righe indipendenti: parallelo con ordine deterministico.
            // ADR-0001: primo errore in ordine di riga (collect
            // sequenziale dopo quello parallelo indicizzato), mai la
            // selezione non deterministica di rayon.
            let results: Vec<Result<Option<Vec<u8>>, ArrowTransportError>> = left
                .par_iter()
                .zip(right.par_iter())
                .map(|(left_geometry, right_geometry)| {
                    match (left_geometry, right_geometry) {
                        (Some(left_geometry), Some(right_geometry)) => {
                            let result = boolean_operation(left_geometry, right_geometry, kernel)?;
                            // EMPTY -> null, convenzione coerente con `clip`.
                            if result.coords_count() == 0 {
                                Ok(None)
                            } else {
                                Ok(Some(encode_geometry(&result)?))
                            }
                        }
                        _ => Ok(None),
                    }
                })
                .collect();
            let values: Vec<Option<Vec<u8>>> =
                results.into_iter().collect::<Result<_, ArrowTransportError>>()?;
            let output_crs = schema
                .left_crs
                .as_deref()
                .ok_or(ArrowTransportError::Internal("left_crs validato assente"))?;
            replace_geometry_batches(
                &left_schema,
                &left_batches,
                geometry_column,
                output_crs,
                &values,
            )?
        }
        PairOperation::Predicate => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let predicate = schema
                .spatial_predicate
                .ok_or(ArrowTransportError::Internal("spatial_predicate validato assente"))?;
            // ADR-0001: primo errore in ordine di riga (collect
            // sequenziale dopo quello parallelo indicizzato).
            let results: Vec<Result<Option<bool>, ArrowTransportError>> = left
                .par_iter()
                .zip(right.par_iter())
                .map(|(left_geometry, right_geometry)| {
                    Ok(match (left_geometry, right_geometry) {
                        (Some(left_geometry), Some(right_geometry)) => Some(
                            evaluate_predicate(left_geometry, right_geometry, predicate)?,
                        ),
                        _ => None,
                    })
                })
                .collect();
            let flags: Vec<Option<bool>> =
                results.into_iter().collect::<Result<_, ArrowTransportError>>()?;
            let column_name = format!("predicate_{}", spatial_predicate_name(predicate));
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(column_name, DataType::Boolean, true),
                &AppendedColumn::Boolean(flags),
            )?
        }
        PairOperation::HausdorffDistance | PairOperation::FrechetDistance => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            let max_pairs = schema
                .max_coordinate_pairs
                .ok_or(ArrowTransportError::Internal("max_coordinate_pairs validato assente"))?;
            // ADR-0001: primo errore in ordine di riga (collect
            // sequenziale dopo quello parallelo indicizzato).
            let results: Vec<Result<Option<f64>, ArrowTransportError>> = left
                .par_iter()
                .zip(right.par_iter())
                .map(|(left_geometry, right_geometry)| {
                    Ok(match (left_geometry, right_geometry) {
                        (Some(left_geometry), Some(right_geometry)) => {
                            if schema.operation == PairOperation::HausdorffDistance {
                                hausdorff_distance(left_geometry, right_geometry, max_pairs)?
                            } else {
                                let left_line =
                                    expect_line_string(left_geometry, schema.operation.name())?;
                                let right_line =
                                    expect_line_string(right_geometry, schema.operation.name())?;
                                frechet_distance(&left_line, &right_line, max_pairs)?
                            }
                        }
                        _ => None,
                    })
                })
                .collect();
            let values: Vec<Option<f64>> =
                results.into_iter().collect::<Result<_, ArrowTransportError>>()?;
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(schema.operation.name(), DataType::Float64, true),
                &AppendedColumn::Float64(values),
            )?
        }
        PairOperation::HaversineDistance
        | PairOperation::GeodesicDistance
        | PairOperation::Bearing => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            if left_rows > limit {
                return Err(ArrowTransportError::OutputRowsExceeded {
                    actual: left_rows,
                    limit,
                });
            }
            // ADR-0001: primo errore in ordine di riga (collect
            // sequenziale dopo quello parallelo indicizzato).
            let results: Vec<Result<Option<f64>, ArrowTransportError>> = left
                .par_iter()
                .zip(right.par_iter())
                .map(|(left_geometry, right_geometry)| {
                    Ok(match (left_geometry, right_geometry) {
                        (Some(left_geometry), Some(right_geometry)) => {
                            let left_point = expect_point(left_geometry, schema.operation.name())?;
                            let right_point =
                                expect_point(right_geometry, schema.operation.name())?;
                            Some(match schema.operation {
                                PairOperation::HaversineDistance => {
                                    haversine_distance_m(left_point, right_point)?
                                }
                                PairOperation::GeodesicDistance => {
                                    geodesic_distance_m(left_point, right_point)?
                                }
                                _ => geodesic_bearing_degrees(left_point, right_point)?,
                            })
                        }
                        _ => None,
                    })
                })
                .collect();
            let values: Vec<Option<f64>> =
                results.into_iter().collect::<Result<_, ArrowTransportError>>()?;
            append_column_batches(
                &left_schema,
                &left_batches,
                Field::new(schema.operation.name(), DataType::Float64, true),
                &AppendedColumn::Float64(values),
            )?
        }
        #[cfg(feature = "geos-backend")]
        PairOperation::Split => {
            if left_rows != right_rows {
                return Err(ArrowTransportError::SideLengthMismatch {
                    left: left_rows,
                    right: right_rows,
                });
            }
            let tolerance = schema.tolerance.unwrap_or(0.0);
            let output_crs = schema
                .left_crs
                .as_deref()
                .ok_or(ArrowTransportError::Internal("left_crs validato assente"))?;
            let geometry_index = geometry_column_index(&left_schema, geometry_column)?;
            let mut encoded: Vec<Vec<u8>> = Vec::new();
            let mut take_indices: Vec<u64> = Vec::new();
            let mut parents: Vec<u64> = Vec::new();
            let mut total = 0_u64;
            for (row, (left_geometry, right_geometry)) in left.iter().zip(&right).enumerate() {
                let (Some(left_geometry), Some(right_geometry)) = (left_geometry, right_geometry)
                else {
                    continue;
                };
                let pieces: Vec<Geometry<f64>> = match left_geometry {
                    Geometry::LineString(line) => split_line(
                        line,
                        right_geometry,
                        tolerance,
                        MAX_CELL_COORDINATES,
                        MAX_SPLIT_WORK,
                        limit,
                        MAX_CELL_COORDINATES,
                    )?
                    .into_iter()
                    .map(Geometry::LineString)
                    .collect(),
                    Geometry::Polygon(_) | Geometry::MultiPolygon(_) => split_polygon_by_linework(
                        left_geometry,
                        right_geometry,
                        MAX_CELL_COORDINATES,
                        MAX_NODING_WORK,
                        limit,
                        MAX_CELL_COORDINATES,
                    )?
                    .into_iter()
                    .map(Geometry::Polygon)
                    .collect(),
                    other => {
                        return Err(ArrowTransportError::WrongGeometryType {
                            operation: schema.operation.name(),
                            expected: "LineString o Polygon/MultiPolygon",
                            actual: geometry_type_name(other).to_owned(),
                        })
                    }
                };
                let next = total
                    .checked_add(pieces.len() as u64)
                    .ok_or(ArrowTransportError::StreamTooLarge)?;
                if next > limit {
                    return Err(ArrowTransportError::OutputRowsExceeded {
                        actual: next,
                        limit,
                    });
                }
                total = next;
                for piece in &pieces {
                    encoded.push(encode_geometry(piece)?);
                    take_indices.push(row as u64);
                    parents.push(row as u64);
                }
            }
            let mut output_fields: Vec<Field> = left_schema
                .fields()
                .iter()
                .map(|field| field.as_ref().clone())
                .collect();
            output_fields[geometry_index] = geometry_output_field(geometry_column, output_crs)?;
            output_fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
            let out_schema = std::sync::Arc::new(Schema::new_with_metadata(
                output_fields,
                left_schema.metadata().clone(),
            ));
            let take_indices = UInt64Array::from(take_indices);
            let mut columns: Vec<plenora_core::arrow::array::ArrayRef> =
                Vec::with_capacity(left_schema.fields().len() + 1);
            for index in 0..left_schema.fields().len() {
                if index == geometry_index {
                    columns.push(std::sync::Arc::new(
                        encoded
                            .iter()
                            .map(|piece| Some(piece.as_slice()))
                            .collect::<BinaryArray>(),
                    ));
                } else {
                    let column = plenora_core::arrow::select::concat::concat(
                        &left_batches
                            .iter()
                            .map(|batch| batch.column(index).as_ref())
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
                    columns.push(
                        plenora_core::arrow::select::take::take(&column, &take_indices, None)
                            .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?,
                    );
                }
            }
            columns.push(std::sync::Arc::new(UInt64Array::from(parents)));
            let batch = RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|error| ArrowTransportError::Arrow(error.to_string()))?;
            (out_schema, vec![batch])
        }
        #[cfg(not(feature = "geos-backend"))]
        PairOperation::Split => {
            return Err(ArrowTransportError::BackendUnavailable {
                operation: schema.operation.name(),
                feature: "geos-backend",
            });
        }
    };

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
    Ok(PairArrowSummary {
        left_rows,
        right_rows,
        output_rows,
        checksum,
    })
}
