//! Dispatcher per forma di risultato: validazione dei parametri delle
//! trasformazioni 1:1, inferenza delle op "unarie" con secondo operando da
//! config e dispatcher esaustivi per le arieta' unaria e binaria.

use plenora_core::arrow::{DataType, Field};
use plenora_core::catalog::OperationDescriptor;
use plenora_core::contract::{
    ContractProperties, DataContract, FieldAllocator, GeometryColumnContract,
};
use plenora_core::crs::{validate_requirement, ResolvedCrs};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::config::{
    AffineTransformConfig, BufferConfig, CleanTopologyConfig, ConcaveHullConfig, DensifyConfig,
    EmptyConfig, LineInterpolatePointConfig, LineSubstringConfig, NearestConfig, OtherWkbConfig,
    OutputColumnConfig, OverlayConfig, PolygonizeConfig, RotateConfig, ScaleConfig, SimplifyConfig,
    SJoinConfig, SnapToGridConfig, SplitConfig, TranslateConfig, VoronoiConfig,
};
use super::helpers::{
    ensure_finite, ensure_name_free, ensure_non_negative, ensure_positive, ensure_ratio,
    invalid_param, merge_schema_metadata, output_fields, output_name, parse_config, rebuild,
    require_xy_dimensions, short_id, single_geometry, validate_other_wkb, with_schema_metadata,
};
use super::measures::{
    analyze_bounds, analyze_diagnostics, analyze_geometry_accessors, analyze_line_locate_point,
    analyze_measure,
};
use super::producers::{
    analyze_add_column, analyze_collect, analyze_expand, analyze_geometry_only, analyze_reproject,
    analyze_snap, analyze_subdivide,
};
use super::quality::{analyze_cluster_dbscan, analyze_coverage_validate, analyze_shared_paths};
use super::{
    CLASS_COLUMN, COUNT_COLUMN, DISTANCE_COLUMN, LEFT_INDEX_COLUMN, RIGHT_INDEX_COLUMN,
    WITHIN_COLUMN, WKT_COLUMN,
};

// ---------------------------------------------------------------------------
// Validazione parametri per gruppo di operazioni.
// ---------------------------------------------------------------------------

/// R4.6.3: il requisito di CRS risolvibile e' condizionato alle operazioni
/// che lo usano. Una colonna con CRS `Missing` (nessun CRS dichiarato in
/// alcuna rappresentazione accettata; R4.4: mai un CRS inventato) attraversa
/// libera le op senza `CrsRequirement` e si ferma QUI — il punto in cui un'op
/// che dichiara il requisito tocca la colonna, in validazione del piano
/// (compile-plan, deterministico), mai a meta' stream. Stessa categoria degli
/// altri requisiti CRS non soddisfatti (`PlenoraError::Crs`); il messaggio
/// dichiara la causa, non l'ultimo tentativo di lettura fallito.
pub(in crate::analyze) fn require_resolved_crs<'a>(
    op: &str,
    geometry: &'a GeometryColumnContract,
) -> Result<&'a ResolvedCrs> {
    geometry.crs.as_resolved().ok_or_else(|| {
        PlenoraError::Crs(format!(
            "{op}: colonna geometria `{}`: nessun CRS dichiarato in alcuna \
             rappresentazione accettata",
            geometry.name
        ))
    })
}

/// Trasformazioni 1:1 in place con parametri: domini identici a
/// `TransformArrowSchema::validate_parameters` (senza i limiti di trasporto).
pub(in crate::analyze) fn validate_transform_params(op: &str, config: &Value) -> Result<()> {
    match op {
        "geo.centroid"
        | "geo.convex_hull"
        | "geo.envelope"
        | "geo.boundary"
        | "geo.point_on_surface"
        | "geo.make_valid" => {
            let _: EmptyConfig = parse_config(op, config)?;
        }
        "geo.buffer" => {
            let parsed: BufferConfig = parse_config(op, config)?;
            ensure_finite(op, "distance", parsed.distance)?;
            let _ = &parsed.cap;
        }
        "geo.simplify" => {
            let parsed: SimplifyConfig = parse_config(op, config)?;
            ensure_non_negative(op, "tolerance", parsed.tolerance)?;
            let _ = &parsed.policy;
        }
        "geo.affine_transform" => {
            let parsed: AffineTransformConfig = parse_config(op, config)?;
            if parsed.coefficients.len() != 6 {
                return Err(invalid_param(op, "coefficients", "devono essere esattamente 6 coefficienti"));
            }
            if parsed.coefficients.iter().any(|value| !value.is_finite()) {
                return Err(invalid_param(op, "coefficients", "devono essere finiti"));
            }
        }
        "geo.translate" => {
            let parsed: TranslateConfig = parse_config(op, config)?;
            ensure_finite(op, "x_offset", parsed.x_offset)?;
            ensure_finite(op, "y_offset", parsed.y_offset)?;
        }
        "geo.scale" => {
            let parsed: ScaleConfig = parse_config(op, config)?;
            ensure_finite(op, "x_factor", parsed.x_factor)?;
            ensure_finite(op, "y_factor", parsed.y_factor)?;
            for (name, value) in [("x_origin", parsed.x_origin), ("y_origin", parsed.y_origin)] {
                if let Some(value) = value {
                    ensure_finite(op, name, value)?;
                }
            }
        }
        "geo.rotate" => {
            let parsed: RotateConfig = parse_config(op, config)?;
            ensure_finite(op, "degrees", parsed.degrees)?;
            for (name, value) in [("x_origin", parsed.x_origin), ("y_origin", parsed.y_origin)] {
                if let Some(value) = value {
                    ensure_finite(op, name, value)?;
                }
            }
        }
        "geo.concave_hull" => {
            let parsed: ConcaveHullConfig = parse_config(op, config)?;
            ensure_positive(op, "concavity", parsed.concavity)?;
            if let Some(value) = parsed.length_threshold {
                ensure_non_negative(op, "length_threshold", value)?;
            }
        }
        "geo.densify" => {
            let parsed: DensifyConfig = parse_config(op, config)?;
            ensure_positive(op, "max_segment_length", parsed.max_segment_length)?;
        }
        "geo.snap_to_grid" => {
            let parsed: SnapToGridConfig = parse_config(op, config)?;
            ensure_positive(op, "grid_size", parsed.grid_size)?;
        }
        "geo.line_substring" => {
            let parsed: LineSubstringConfig = parse_config(op, config)?;
            ensure_ratio(op, "start_ratio", parsed.start_ratio)?;
            ensure_ratio(op, "end_ratio", parsed.end_ratio)?;
            if parsed.start_ratio > parsed.end_ratio {
                return Err(invalid_param(
                    op,
                    "start_ratio/end_ratio",
                    "start_ratio non puo superare end_ratio",
                ));
            }
        }
        "geo.line_interpolate_point" => {
            let parsed: LineInterpolatePointConfig = parse_config(op, config)?;
            ensure_ratio(op, "ratio", parsed.ratio)?;
        }
        _ => {
            return Err(PlenoraError::Internal(
                "validate_transform_params: op non una trasformazione".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Predicati e distanze "unari" con secondo operando da config (`other_wkb`).
pub(in crate::analyze) fn analyze_unary_pair(op: &str, input: &DataContract, config: &Value, data_type: DataType) -> Result<DataContract> {
    let parsed: OtherWkbConfig = parse_config(op, config)?;
    validate_other_wkb(op, &parsed.other_wkb)?;
    let name = output_name(op, parsed.output_column.as_deref(), short_id(op))?;
    analyze_add_column(op, input, name, data_type)
}

/// Inferenza per le operazioni unarie (tutto tranne `from_coords` e le
/// binarie, gestite altrove). `fields` alloca i `FieldId` delle op v1.3 che
/// creano una nuova colonna geometria.
// Dispatcher esaustivo sulle op unarie del catalogo: la lunghezza e' la
// sequenza lineare dei casi sul contratto, non complessita' logica.
#[allow(clippy::too_many_lines)]
pub(in crate::analyze) fn analyze_unary(
    descriptor: &OperationDescriptor,
    input: &DataContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let op = descriptor.id;
    let geometry = single_geometry(op, input)?;
    // B1.3: ogni op unaria che consuma una geometria la decodifica in XY —
    // dimensionalita' diversa rifiutata a compile-plan (mai a meta' stream).
    require_xy_dimensions(op, geometry)?;
    let requirement = descriptor.crs_requirement.ok_or_else(|| {
        PlenoraError::InvalidPlan(format!("{op}: crs_requirement assente nel catalogo"))
    })?;
    match op {
        // Trasformazioni 1:1 in place: schema e FieldId invariati.
        "geo.centroid" | "geo.convex_hull" | "geo.envelope" | "geo.boundary"
        | "geo.point_on_surface" | "geo.make_valid" | "geo.buffer" | "geo.simplify"
        | "geo.affine_transform" | "geo.translate" | "geo.scale" | "geo.rotate"
        | "geo.concave_hull" | "geo.densify" | "geo.snap_to_grid" | "geo.line_substring"
        | "geo.line_interpolate_point" => {
            validate_transform_params(op, config)?;
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            Ok(input.clone())
        }
        "geo.reproject" => analyze_reproject(op, input, geometry, config, plan_crs),
        "geo.area" | "geo.length" | "geo.perimeter" | "geo.geodesic_line_length"
        | "geo.geodesic_area" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_measure(op, input, config)
        }
        "geo.vertex_count" => {
            let parsed: OutputColumnConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            let name = output_name(op, parsed.output_column.as_deref(), short_id(op))?;
            analyze_add_column(op, input, name, DataType::UInt64)
        }
        "geo.to_wkt" => {
            let parsed: OutputColumnConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            let name = output_name(op, parsed.output_column.as_deref(), WKT_COLUMN)?;
            analyze_add_column(op, input, name, DataType::Utf8)
        }
        "geo.bounds_extractor" => {
            let _: EmptyConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_bounds(op, input, geometry)
        }
        "geo.geometry_diagnostics" => {
            let _: EmptyConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_diagnostics(op, input, geometry)
        }
        "geo.explode" | "geo.delaunay" => {
            let _: EmptyConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_expand(op, input)
        }
        "geo.split" => {
            let parsed: SplitConfig = parse_config(op, config)?;
            validate_other_wkb(op, &parsed.other_wkb)?;
            if let Some(tolerance) = parsed.tolerance {
                ensure_non_negative(op, "tolerance", tolerance)?;
            }
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_expand(op, input)
        }
        "geo.voronoi" => {
            let parsed: VoronoiConfig = parse_config(op, config)?;
            if let Some(max_points) = parsed.max_points {
                if max_points < 2 {
                    return Err(invalid_param(op, "max_points", "deve essere almeno 2"));
                }
            }
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            Ok(input.clone())
        }
        "geo.clean_topology" => {
            let parsed: CleanTopologyConfig = parse_config(op, config)?;
            ensure_non_negative(op, "snap_tolerance", parsed.snap_tolerance)?;
            let _ = (&parsed.remove_overlaps, &parsed.fill_gaps);
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            Ok(input.clone())
        }
        "geo.dissolve" | "geo.line_builder" | "geo.polygon_builder" | "geo.line_merge" => {
            let _: EmptyConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_geometry_only(input, geometry, &[])
        }
        "geo.collect" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_collect(op, input, geometry, config)
        }
        "geo.geometry_accessors" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_geometry_accessors(op, input, config)
        }
        "geo.line_locate_point" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_line_locate_point(op, input, config)
        }
        "geo.subdivide" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_subdivide(op, input, geometry, config)
        }
        "geo.snap" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_snap(op, input, config)
        }
        "geo.coverage_validate" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_coverage_validate(op, input, geometry, config, fields)
        }
        "geo.shared_paths" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_shared_paths(op, input, geometry, config, fields)
        }
        "geo.cluster_dbscan" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_cluster_dbscan(op, input, config)
        }
        "geo.polygonize" => {
            let parsed: PolygonizeConfig = parse_config(op, config)?;
            let _ = (&parsed.node_input, &parsed.require_complete);
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_geometry_only(input, geometry, &[Field::new(CLASS_COLUMN, DataType::Utf8, false)])
        }
        "geo.distance" | "geo.hausdorff_distance" | "geo.frechet_distance"
        | "geo.haversine_distance" | "geo.geodesic_distance" | "geo.bearing" => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_unary_pair(op, input, config, DataType::Float64)
        }
        _ if op.starts_with("geo.predicate_") => {
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            analyze_unary_pair(op, input, config, DataType::Boolean)
        }
        _ => Err(PlenoraError::Unsupported(format!(
            "{op}: analyze_contract non implementata"
        ))),
    }
}

/// Inferenza per le operazioni binarie ordinate (left, right).
///
/// I metadati di SCHEMA dell'output sono il merge R2.4 delle due sorgenti
/// ([`super::helpers::merge_schema_metadata`]): chiave presente in una sola copiata, uguale
/// in entrambe copiata, diversa in entrambe -> errore che nomina la chiave.
pub(in crate::analyze) fn analyze_binary(
    descriptor: &OperationDescriptor,
    inputs: &[DataContract],
    config: &Value,
) -> Result<DataContract> {
    let op = descriptor.id;
    let left = &inputs[0];
    let right = &inputs[1];
    let left_geometry = single_geometry(op, left)?;
    let right_geometry = single_geometry(op, right)?;
    // B1.3: come per le unarie — entrambi gli operandi devono essere XY.
    require_xy_dimensions(op, left_geometry)?;
    require_xy_dimensions(op, right_geometry)?;
    let requirement = descriptor.crs_requirement.ok_or_else(|| {
        PlenoraError::InvalidPlan(format!("{op}: crs_requirement assente nel catalogo"))
    })?;
    validate_requirement(
        requirement,
        &[
            require_resolved_crs(op, left_geometry)?,
            require_resolved_crs(op, right_geometry)?,
        ],
    )?;
    let output = match op {
        // Schema left invariato, geometria sostituita in place; righe
        // allineate a left nel protocollo legacy: proprieta' preservate.
        "geo.clip" | "geo.intersection" | "geo.union" | "geo.difference"
        | "geo.symmetric_difference" => {
            let _: EmptyConfig = parse_config(op, config)?;
            Ok(left.clone())
        }
        "geo.within" => {
            let parsed: OutputColumnConfig = parse_config(op, config)?;
            let name = output_name(op, parsed.output_column.as_deref(), WITHIN_COLUMN)?;
            analyze_add_column(op, left, name, DataType::Boolean)
        }
        "geo.count_points_in_polygons" => {
            let parsed: OutputColumnConfig = parse_config(op, config)?;
            let name = output_name(op, parsed.output_column.as_deref(), COUNT_COLUMN)?;
            analyze_add_column(op, left, name, DataType::UInt64)
        }
        // Join con lineage: righe moltiplicate, proprieta' eliminate.
        "geo.sjoin" => {
            let parsed: SJoinConfig = parse_config(op, config)?;
            let _ = &parsed.predicate;
            let mut fields = output_fields(left);
            ensure_name_free(op, &fields, RIGHT_INDEX_COLUMN)?;
            fields.push(Field::new(RIGHT_INDEX_COLUMN, DataType::UInt64, false));
            rebuild(left, fields, ContractProperties::default())
        }
        "geo.nearest" => {
            let parsed: NearestConfig = parse_config(op, config)?;
            if let Some(max_distance) = parsed.max_distance {
                ensure_non_negative(op, "max_distance", max_distance)?;
            }
            let mut fields = output_fields(left);
            for (name, data_type) in [
                (RIGHT_INDEX_COLUMN, DataType::UInt64),
                (DISTANCE_COLUMN, DataType::Float64),
            ] {
                ensure_name_free(op, &fields, name)?;
                fields.push(Field::new(name, data_type, true));
            }
            rebuild(left, fields, ContractProperties::default())
        }
        "geo.overlay" => {
            let parsed: OverlayConfig = parse_config(op, config)?;
            let _ = &parsed.mode;
            analyze_geometry_only(
                left,
                left_geometry,
                &[
                    Field::new(LEFT_INDEX_COLUMN, DataType::UInt64, true),
                    Field::new(RIGHT_INDEX_COLUMN, DataType::UInt64, true),
                ],
            )
        }
        _ => Err(PlenoraError::Unsupported(format!(
            "{op}: analyze_contract non implementata"
        ))),
    }?;
    // R2.4: i metadati di SCHEMA delle due sorgenti sono fusi; un conflitto
    // su valori diversi fallisce qui, in validazione, mai a runtime.
    let schema_metadata = merge_schema_metadata(op, left, right)?;
    with_schema_metadata(&output, schema_metadata)
}
