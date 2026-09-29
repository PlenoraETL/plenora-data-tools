//! Dispatcher per forma di risultato: validazione dei parametri delle
//! trasformazioni 1:1, inferenza delle op "unarie" con secondo operando da
//! config e dispatcher esaustivi per le arieta' unaria e binaria.

use plenora_core::arrow::{DataType, Field};
use plenora_core::catalog::OperationDescriptor;
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldAllocator, GeometryColumnContract,
    GeometryType, GeometryTypesProperty, TypesDeclaration,
};
use plenora_core::crs::{validate_requirement, ResolvedCrs};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::config::{
    AffineTransformConfig, BufferConfig, CleanTopologyConfig, ConcaveHullConfig, DensifyConfig,
    EmptyConfig, LineInterpolatePointConfig, LineSubstringConfig, NearestConfig, OtherWkbConfig,
    OutputColumnConfig, OverlayConfig, PolygonizeConfig, RotateConfig, SJoinConfig, ScaleConfig,
    SimplifyConfig, SnapToGridConfig, SplitConfig, TranslateConfig, VoronoiConfig,
};
use super::helpers::{
    crs_requirement, ensure_finite, ensure_name_free, ensure_non_negative, ensure_positive,
    ensure_ratio, invalid_param, merge_schema_metadata, output_fields, output_name, parse_config,
    rebuild, require_identifiable_geometry, require_xy_dimensions, short_id, single_geometry,
    validate_other_wkb, with_geometry_types, with_geometry_types_property, with_schema_metadata,
};
use super::measures::{
    analyze_bounds, analyze_diagnostics, analyze_geometry_accessors, analyze_line_locate_point,
    analyze_measure,
};
use super::producers::{
    analyze_add_column, analyze_collect, analyze_expand, analyze_geometry_only, analyze_snap,
    analyze_subdivide,
};
use super::quality::{analyze_cluster_dbscan, analyze_coverage_validate, analyze_shared_paths};
use super::{
    CLASS_COLUMN, COUNT_COLUMN, DISTANCE_COLUMN, LEFT_INDEX_COLUMN, RIGHT_INDEX_COLUMN,
    WITHIN_COLUMN, WKT_COLUMN,
};

// ---------------------------------------------------------------------------
// Validazione parametri per gruppo di operazioni.
// ---------------------------------------------------------------------------

/// Gate R4.6.3 del CRS risolto, a compile-plan e mai a meta' stream.
///
/// Una colonna con CRS non risolto attraversa le op senza `CrsRequirement` e
/// si ferma qui, quando la tocca un'op che dichiara il requisito. L'errore e'
/// `PlenoraError::Crs` e il messaggio distingue `Missing` (nessun CRS
/// dichiarato, R4.4: mai un CRS inventato) da `DeclaredUnresolved` (la
/// colonna dichiara un'incoerenza che solo il piano puo' risolvere).
pub(in crate::analyze) fn require_resolved_crs<'a>(
    op: &str,
    geometry: &'a GeometryColumnContract,
) -> Result<&'a ResolvedCrs> {
    match &geometry.crs {
        ContractCrs::Resolved(crs) | ContractCrs::ResolvedByDecision(crs) => Ok(crs),
        ContractCrs::Missing => Err(PlenoraError::Crs(format!(
            "{op}: colonna geometria `{}`: nessun CRS dichiarato in alcuna \
             rappresentazione accettata",
            geometry.name
        ))),
        ContractCrs::DeclaredUnresolved { .. } => Err(PlenoraError::Crs(format!(
            "{op}: colonna geometria `{}`: la colonna dichiara un'incoerenza CRS \
             non risolta (`declared_unresolved`); la risoluzione richiede una \
             decisione esplicita nel piano (R4.6.3)",
            geometry.name
        ))),
    }
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
                return Err(invalid_param(
                    op,
                    "coefficients",
                    "devono essere esattamente 6 coefficienti",
                ));
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

/// Predicati e distanze "unari" con secondo operando da config (`other_wkb`),
/// che deve stare nel dominio di validita' del CRS dell'input.
pub(in crate::analyze) fn analyze_unary_pair(
    op: &str,
    input: &DataContract,
    config: &Value,
    data_type: DataType,
) -> Result<DataContract> {
    let parsed: OtherWkbConfig = parse_config(op, config)?;
    validate_other_wkb(op, &parsed.other_wkb)?;
    super::helpers::validate_other_wkb_domain(op, &parsed.other_wkb, input)?;
    let name = output_name(op, parsed.output_column.as_deref(), short_id(op))?;
    analyze_add_column(op, input, name, data_type)
}

/// Proprieta' `exact` della mappa tipi di output: le liste della mappa sono
/// statiche e note rispettare R3.4.1; un fallimento di costruzione e' una
/// invariante interna violata, mai un errore dell'utente.
fn exact_types(types: Vec<GeometryType>) -> Result<GeometryTypesProperty> {
    GeometryTypesProperty::new(TypesDeclaration::Exact, types).map_err(|error| {
        PlenoraError::Internal(format!("mappa tipi di output incoerente: {error}"))
    })
}

/// Tipi geometrici dell'output delle trasformazioni 1:1 in place (mappa
/// per-op, piano-v5.md#contratti-di-input decisione 8).
///
/// `Some` con i tipi dell'OUTPUT, verificati contro i kernel, per le op che
/// cambiano il tipo; `None` per quelle che lo preservano. Casi non ovvi:
/// `convex_hull` e `concave_hull` producono sempre `Polygon`, anche
/// degenere; `make_valid` dichiara `mixed` senza elenco, perche' la
/// riparazione cella per cella (backend Rust, `rust_backend`) puo' cambiare
/// tipo e l'insieme non e' enumerabile a secco (R3.4.1).
fn transform_output_types(op: &str) -> Result<Option<GeometryTypesProperty>> {
    match op {
        "geo.centroid" | "geo.point_on_surface" | "geo.line_interpolate_point" => {
            exact_types(vec![GeometryType::Point]).map(Some)
        }
        "geo.convex_hull" | "geo.concave_hull" => {
            exact_types(vec![GeometryType::Polygon]).map(Some)
        }
        "geo.envelope" => exact_types(vec![
            GeometryType::Point,
            GeometryType::LineString,
            GeometryType::Polygon,
        ])
        .map(Some),
        "geo.line_substring" => {
            exact_types(vec![GeometryType::Point, GeometryType::LineString]).map(Some)
        }
        "geo.buffer" => exact_types(vec![GeometryType::MultiPolygon]).map(Some),
        "geo.boundary" => exact_types(vec![
            GeometryType::MultiPoint,
            GeometryType::MultiLineString,
            GeometryType::GeometryCollection,
        ])
        .map(Some),
        "geo.make_valid" => GeometryTypesProperty::new(TypesDeclaration::Mixed, Vec::new())
            .map(Some)
            .map_err(|error| {
                PlenoraError::Internal(format!("mappa tipi di output incoerente: {error}"))
            }),
        "geo.simplify"
        | "geo.affine_transform"
        | "geo.translate"
        | "geo.scale"
        | "geo.rotate"
        | "geo.densify"
        | "geo.snap_to_grid" => Ok(None),
        _ => Err(PlenoraError::Internal(format!(
            "transform_output_types: `{op}` non e' una trasformazione 1:1"
        ))),
    }
}

/// Inferenza per le operazioni unarie (tutto tranne `from_coords` e le
/// binarie, gestite altrove). `fields` alloca i `FieldId` delle op che
/// creano una nuova colonna geometria.
// Dispatcher esaustivo sulle op unarie del catalogo: la lunghezza e' la
// sequenza lineare dei casi sul contratto, non complessita' logica.
pub(in crate::analyze) fn analyze_unary(
    descriptor: &OperationDescriptor,
    input: &DataContract,
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let output = analyze_unary_shape(descriptor, input, config, fields)?;
    let geometry = single_geometry(descriptor.id, input)?;
    redeclare_output_types(descriptor.id, geometry, output)
}

/// I tipi dell'uscita delle operazioni che spezzano, raccolgono o
/// ricostruiscono le geometrie (`analyze::tipi`); le altre passano
/// invariate.
fn redeclare_output_types(
    op: &str,
    geometry: &GeometryColumnContract,
    output: DataContract,
) -> Result<DataContract> {
    let Some(types) = super::tipi::tipi_di_uscita(op, &geometry.types)? else {
        return Ok(output);
    };
    let target = output
        .geometries
        .iter()
        .find(|candidate| candidate.field_id == geometry.field_id)
        .cloned()
        .ok_or_else(|| {
            PlenoraError::Internal(format!("{op}: colonna geometria assente dall'uscita"))
        })?;
    with_geometry_types_property(&output, &target, types)
}

#[allow(clippy::too_many_lines)]
fn analyze_unary_shape(
    descriptor: &OperationDescriptor,
    input: &DataContract,
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let op = descriptor.id;
    let geometry = single_geometry(op, input)?;
    // piano-v5.md#contratti-di-input decisione 8: la colonna deve essere identificabile dal
    // trasporto (estensione `geoarrow.wkb` o sole chiavi canoniche) —
    // rifiuto a compile-plan, mai a meta' esecuzione (architettura.md#geometrie).
    require_identifiable_geometry(op, input, geometry)?;
    // Ogni op unaria che consuma una geometria la decodifica in XY —
    // dimensionalita' diversa rifiutata a compile-plan (mai a meta' stream).
    require_xy_dimensions(op, geometry)?;
    let requirement = crs_requirement(op, descriptor)?;
    match op {
        // Trasformazioni 1:1 in place: schema e FieldId invariati; le op
        // che CAMBIANO il tipo geometrico dichiarano i tipi dell'output
        // (piano-v5.md#contratti-di-input decisione 8, `transform_output_types`).
        "geo.centroid"
        | "geo.convex_hull"
        | "geo.envelope"
        | "geo.boundary"
        | "geo.point_on_surface"
        | "geo.make_valid"
        | "geo.buffer"
        | "geo.simplify"
        | "geo.affine_transform"
        | "geo.translate"
        | "geo.scale"
        | "geo.rotate"
        | "geo.concave_hull"
        | "geo.densify"
        | "geo.snap_to_grid"
        | "geo.line_substring"
        | "geo.line_interpolate_point" => {
            validate_transform_params(op, config)?;
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            transform_output_types(op)?.map_or_else(
                || Ok(input.clone()),
                |types| with_geometry_types(input, geometry, types),
            )
        }
        "geo.area"
        | "geo.length"
        | "geo.perimeter"
        | "geo.geodesic_line_length"
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
            // La lama arriva dalla config nel CRS dell'input (D16): stesso
            // dominio del secondo operando di distanze e predicati.
            super::helpers::validate_other_wkb_domain(op, &parsed.other_wkb, input)?;
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
            // piano-v5.md#contratti-di-input decisione 8: le celle di Voronoi sono sempre poligoni
            // (l'input puntuale non e' il tipo dell'output).
            with_geometry_types(input, geometry, exact_types(vec![GeometryType::Polygon])?)
        }
        "geo.clean_topology" => {
            let parsed: CleanTopologyConfig = parse_config(op, config)?;
            ensure_non_negative(op, "snap_tolerance", parsed.snap_tolerance)?;
            let _ = (&parsed.remove_overlaps, &parsed.fill_gaps);
            validate_requirement(requirement, &[require_resolved_crs(op, geometry)?])?;
            // piano-v5.md#contratti-di-input decisione 8: input poligonale, output poligonale — ma
            // la rimozione degli overlap (differenza) puo' spezzare un
            // `Polygon` in `MultiPolygon`: insieme noto, dichiarazione exact.
            with_geometry_types(
                input,
                geometry,
                exact_types(vec![GeometryType::Polygon, GeometryType::MultiPolygon])?,
            )
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
            analyze_geometry_only(
                input,
                geometry,
                &[Field::new(CLASS_COLUMN, DataType::Utf8, false)],
            )
        }
        "geo.distance"
        | "geo.hausdorff_distance"
        | "geo.frechet_distance"
        | "geo.haversine_distance"
        | "geo.geodesic_distance"
        | "geo.bearing" => {
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
    // piano-v5.md#contratti-di-input decisione 8 (come per le unarie): identificabilita' della
    // colonna verificata a compile-plan su entrambi gli operandi.
    require_identifiable_geometry(op, left, left_geometry)?;
    require_identifiable_geometry(op, right, right_geometry)?;
    // Come per le unarie: entrambi gli operandi devono essere XY.
    require_xy_dimensions(op, left_geometry)?;
    require_xy_dimensions(op, right_geometry)?;
    let requirement = crs_requirement(op, descriptor)?;
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
        // piano-v5.md#contratti-di-input decisione 8: le booleane poligonali producono sempre
        // `MultiPolygon` (forma unica del kernel), non il tipo di left.
        "geo.clip"
        | "geo.intersection"
        | "geo.union"
        | "geo.difference"
        | "geo.symmetric_difference" => {
            let _: EmptyConfig = parse_config(op, config)?;
            with_geometry_types(
                left,
                left_geometry,
                exact_types(vec![GeometryType::MultiPolygon])?,
            )
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
    let output = redeclare_output_types(op, left_geometry, output)?;
    // R2.4: i metadati di SCHEMA delle due sorgenti sono fusi; un conflitto
    // su valori diversi fallisce qui, in validazione, mai a runtime.
    let schema_metadata = merge_schema_metadata(op, left, right)?;
    with_schema_metadata(&output, schema_metadata)
}
