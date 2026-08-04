//! Produttori e trasformazioni: inferenza per le op che creano o
//! ristrutturano la colonna geometria (`from_coords`, `from_wkt`,
//! `generate_grid`, `reproject`, `collect`, `subdivide`, `snap`) e le forme
//! di risultato condivise (colonna aggiunta, espansione, sole geometrie).

use std::sync::Arc;

use plenora_core::arrow::{DataType, Field, Schema};
use plenora_core::catalog::CrsRequirement;
use plenora_core::contract::{
    ContractCrs, ContractProperties, ContractProperty, DataContract, FieldAllocator,
    GeometryColumnContract, GeometryDimensions, GeometryEncoding, GeometryType,
    GeometryTypesProperty, PropertyConfidence, PropertyScope, TypesDeclaration,
};
use plenora_core::crs::{validate_requirement, ResolvedCrs};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use crate::arrow_adapter::DEFAULT_GEOMETRY_COLUMN;

use super::config::{
    CollectConfig, FromCoordsConfig, FromWktConfig, GenerateGridConfig, ReprojectConfig,
    SnapConfig, SubdivideConfig,
};
use super::dispatch::require_resolved_crs;
use super::helpers::{
    ensure_name, ensure_name_free, ensure_non_negative, geometry_field, invalid_param,
    new_geometry_field, output_fields, parse_config, rebuild, require_normalized_axis_order,
    resolve_definition, set_geometry_crs, validate_wkb_hex,
};
use super::{
    CELL_I_COLUMN, CELL_J_COLUMN, CENTROID_X_COLUMN, CENTROID_Y_COLUMN, DEFAULT_X_COLUMN,
    DEFAULT_Y_COLUMN, PARENT_INDEX_COLUMN,
};

// ---------------------------------------------------------------------------
// Inferenza per forma di risultato.
// ---------------------------------------------------------------------------

/// Aggiunge una colonna scalare in coda allo schema (geometria preservata).
pub(in crate::analyze) fn analyze_add_column(
    op: &str,
    input: &DataContract,
    name: &str,
    data_type: DataType,
) -> Result<DataContract> {
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(Field::new(name, data_type, true));
    rebuild(input, fields, input.properties.clone())
}

/// Espansione 1:N (`explode`, `delaunay`, `split`): schema invariato piu'
/// `__parent_index`; `sorted_by` preservato (espansione stabile), `row_count`
/// eliminato.
pub(in crate::analyze) fn analyze_expand(op: &str, input: &DataContract) -> Result<DataContract> {
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, PARENT_INDEX_COLUMN)?;
    fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
    let mut properties = input.properties.clone();
    properties.row_count = None;
    rebuild(input, fields, properties)
}

/// Aggregazione a sole geometrie (`dissolve`, builder, `polygonize`,
/// `line_merge`, `overlay`): le colonne attributo non sono propagate; la
/// geometria aggregata e' nullable (input vuoto -> geometria null).
///
/// I metadati dello SCHEMA di input sono conservati (R2.4): riguardano il
/// dataset, non le colonne attributo eliminate. Le colonne `extra` sono
/// passate come `Field` interi: quelle copiate dall'input (le chiavi di
/// `collect`) conservano i loro metadati (R2.4 identity-preserving), quelle
/// sintetiche (`polygonize`, `overlay`) nascono senza.
pub(in crate::analyze) fn analyze_geometry_only(
    input: &DataContract,
    geometry: &GeometryColumnContract,
    extra: &[Field],
) -> Result<DataContract> {
    let mut fields = vec![geometry_field(input, geometry, true)?];
    fields.extend(extra.iter().cloned());
    let active = input
        .active_geometry
        .filter(|active| *active == geometry.field_id);
    let aggregated = GeometryColumnContract {
        nullable: true,
        ..geometry.clone()
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![aggregated],
        active,
        ContractProperties::default(),
    )
}

/// `reproject`: schema invariato, CRS del contratto e metadato `geo.crs`
/// aggiornati al target risolto. La sorgente DEVE avere un CRS risolto
/// (R4.6.3: il requisito e' dell'operazione, gate in analyze).
///
/// ADR-0009 decisione 8: la riproiezione CAMBIA il fatto (il CRS della
/// colonna), non ne descrive uno diverso — le chiavi canoniche CRS ereditate
/// dal campo di input (`crs_id`, `crs_definition`+formato, `srid`,
/// `axis_order`, `crs_resolution`) sono SOSTITUITE, non fuse: rimosse qui e
/// ri-emesse dal contratto in `executor.rs::canonical_output_schema`, senza
/// che il guard R2.6 veda mai una divergenza. Il guard resta intatto per
/// tutte le altre chiavi. `srid` resta metadato d'autorita'; `axis_order`
/// descrive invece le coordinate di OUTPUT del backend e viene fissato qui
/// all'ordine GIS normalizzato (`lon_lat` o `easting_northing`), separato
/// dall'ordine nativo della definizione d'autorita' (per esempio EPSG:4326
/// e' authority `lat_lon`, ma PROJ produce x=longitudine/y=latitudine).
///
/// Quell'affermazione vale solo se le coordinate ENTRANO nell'ordine GIS
/// normalizzato: [`require_normalized_axis_order`] lo impone fail-closed
/// prima di riscrivere la chiave. Senza il gate, con source == target il
/// backend restituirebbe i byte invariati (nessuna pipeline PROJ) e
/// l'`axis_order` riscritto contraddirebbe le coordinate che descrive.
pub(in crate::analyze) fn analyze_reproject(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
) -> Result<DataContract> {
    let parsed: ReprojectConfig = parse_config(op, config)?;
    // Gate R4.6.3 PRIMA della risoluzione del target: un CRS sorgente
    // `Missing` ferma l'op con lo stesso errore a prescindere dal backend
    // compilato (determinismo del fallimento).
    let source = require_resolved_crs(op, geometry)?;
    // Gate dell'ordine assi PRIMA della risoluzione del target, come quello
    // R4.6.3 sopra: dipende dalla sola sorgente e fallisce identico a
    // prescindere dal backend compilato (determinismo del fallimento).
    require_normalized_axis_order(op, input, geometry, source)?;
    let target = resolve_definition(&parsed.target_crs, plan_crs)?;
    validate_requirement(CrsRequirement::Reprojection, &[source, &target])?;
    let mut fields = output_fields(input);
    set_geometry_crs(&mut fields, geometry, &target)?;
    for field in &mut fields {
        if field.name() == &geometry.name {
            let mut metadata = field.metadata().clone();
            crate::arrow_adapter::strip_rewritten_crs_keys(&mut metadata);
            metadata.insert(
                crate::arrow_adapter::PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                target.normalized_gis_axis_order().as_str().to_owned(),
            );
            *field = field.clone().with_metadata(metadata);
        }
    }
    let reprojected = GeometryColumnContract {
        crs: ContractCrs::Resolved(target),
        ..geometry.clone()
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![reprojected],
        input.active_geometry,
        input.properties.clone(),
    )
}

/// `from_coords`: nessuna geometria in input; due colonne numeriche
/// (`Float64`/`Int64`) producono la colonna geometria (nuovo `FieldId`,
/// `nullable=false`), CRS da config o di piano.
pub(in crate::analyze) fn analyze_from_coords(
    op: &str,
    input: &DataContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
    requirement: CrsRequirement,
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let parsed: FromCoordsConfig = parse_config(op, config)?;
    if !input.geometries.is_empty() {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input ha gia' una colonna geometria"
        )));
    }
    let x_column = parsed.x_column.as_deref().unwrap_or(DEFAULT_X_COLUMN);
    let y_column = parsed.y_column.as_deref().unwrap_or(DEFAULT_Y_COLUMN);
    let name = parsed
        .geometry_column
        .as_deref()
        .unwrap_or(DEFAULT_GEOMETRY_COLUMN);
    for (param, column) in [("x_column", x_column), ("y_column", y_column)] {
        if !ensure_name(column) {
            return Err(invalid_param(op, param, "non deve essere vuoto"));
        }
        let field = input.schema.field_with_name(column).map_err(|_| {
            PlenoraError::Schema(format!("{op}: colonna `{column}` assente dallo schema"))
        })?;
        if !matches!(field.data_type(), DataType::Float64 | DataType::Int64) {
            return Err(PlenoraError::Schema(format!(
                "{op}: colonna `{column}` di tipo {}, attesa Float64 o Int64",
                field.data_type()
            )));
        }
    }
    if !ensure_name(name) {
        return Err(invalid_param(
            op,
            "geometry_column",
            "non deve essere vuoto",
        ));
    }
    let crs = match &parsed.crs {
        Some(definition) => resolve_definition(definition, plan_crs)?,
        None => plan_crs.cloned().ok_or_else(|| {
            PlenoraError::Crs(format!(
                "{op}: CRS obbligatorio (config `crs` o CRS di piano)"
            ))
        })?,
    };
    validate_requirement(requirement, &[&crs])?;
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(new_geometry_field(
        name,
        &crs,
        GeometryDimensions::Xy,
        None,
        false,
    )?);
    let field_id = fields_allocator.alloc();
    let geometry = GeometryColumnContract {
        field_id,
        name: name.to_owned(),
        crs: ContractCrs::Resolved(crs),
        // Produttore (B1.3): `from_coords` costruisce punti XY — dichiara Xy.
        dimensions: GeometryDimensions::Xy,
        encoding: None,
        nullable: false,
        types: GeometryColumnContract::undeclared_types(),
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![geometry],
        Some(field_id),
        input.properties.clone(),
    )
}

/// `from_wkt`: nessuna geometria in input; una colonna `Utf8` WKT produce la
/// colonna geometria (nuovo `FieldId`, **nullable** per le celle sorgente
/// null; WKT invalido rifiuta l'output con diagnostica row-scoped). CRS da config `crs` o di
/// piano; requisito del catalogo (`Known`).
pub(in crate::analyze) fn analyze_from_wkt(
    op: &str,
    input: &DataContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
    requirement: CrsRequirement,
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let parsed: FromWktConfig = parse_config(op, config)?;
    let _ = &parsed.on_error;
    if !input.geometries.is_empty() {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input ha gia' una colonna geometria"
        )));
    }
    if !ensure_name(&parsed.wkt_column) {
        return Err(invalid_param(op, "wkt_column", "non deve essere vuoto"));
    }
    let wkt_field = input
        .schema
        .field_with_name(&parsed.wkt_column)
        .map_err(|_| {
            PlenoraError::Schema(format!(
                "{op}: colonna `{}` assente dallo schema",
                parsed.wkt_column
            ))
        })?;
    if wkt_field.data_type() != &DataType::Utf8 {
        return Err(PlenoraError::Schema(format!(
            "{op}: colonna `{}` di tipo {}, attesa Utf8",
            parsed.wkt_column,
            wkt_field.data_type()
        )));
    }
    let name = parsed
        .output_column
        .as_deref()
        .unwrap_or(DEFAULT_GEOMETRY_COLUMN);
    if !ensure_name(name) {
        return Err(invalid_param(op, "output_column", "non deve essere vuoto"));
    }
    let crs = match &parsed.crs {
        Some(definition) => resolve_definition(definition, plan_crs)?,
        None => plan_crs.cloned().ok_or_else(|| {
            PlenoraError::Crs(format!(
                "{op}: CRS obbligatorio (config `crs` o CRS di piano)"
            ))
        })?,
    };
    validate_requirement(requirement, &[&crs])?;
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(new_geometry_field(
        name,
        &crs,
        GeometryDimensions::Xy,
        None,
        true,
    )?);
    let field_id = fields_allocator.alloc();
    let output_types = GeometryTypesProperty::new(
        TypesDeclaration::Mixed,
        vec![
            GeometryType::Point,
            GeometryType::LineString,
            GeometryType::Polygon,
            GeometryType::MultiPoint,
            GeometryType::MultiLineString,
            GeometryType::MultiPolygon,
            GeometryType::GeometryCollection,
        ],
    )
    .map_err(|error| PlenoraError::Internal(error.to_string()))?;
    let geometry = GeometryColumnContract {
        field_id,
        name: name.to_owned(),
        crs: ContractCrs::Resolved(crs),
        // Produttore (B1.3): il parser WKT decodifica in `Geometry<f64>` —
        // dichiara Xy.
        dimensions: GeometryDimensions::Xy,
        encoding: Some(GeometryEncoding::Wkb),
        nullable: true,
        // Il parser accetta per contratto tipi geometrici eterogenei; `mixed`
        // e' informazione esplicita, non dati non ispezionati (`unresolved`).
        types: ContractProperty::new(
            PropertyConfidence::Declared(output_types),
            PropertyScope::Schema,
        ),
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![geometry],
        Some(field_id),
        input.properties.clone(),
    )
}

/// `collect`: aggregazione per gruppo a sole geometrie piu' le colonne
/// chiave; gli altri attributi non sono propagati (come `dissolve`).
pub(in crate::analyze) fn analyze_collect(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: CollectConfig = parse_config(op, config)?;
    if parsed.group_by.is_empty() {
        return Err(invalid_param(op, "group_by", "non deve essere vuoto"));
    }
    let mut extra: Vec<Field> = Vec::with_capacity(parsed.group_by.len());
    for name in &parsed.group_by {
        if !ensure_name(name) {
            return Err(invalid_param(op, "group_by", "nomi colonna non vuoti"));
        }
        if name == &geometry.name {
            return Err(invalid_param(
                op,
                "group_by",
                "la colonna geometria non puo' essere chiave di gruppo",
            ));
        }
        let field = input.schema.field_with_name(name).map_err(|_| {
            PlenoraError::Schema(format!("{op}: colonna `{name}` assente dallo schema"))
        })?;
        if extra.iter().any(|seen| seen.name() == name) {
            return Err(invalid_param(op, "group_by", "colonne duplicate"));
        }
        // R2.4 identity-preserving: la colonna chiave sopravvive invariata —
        // si clona il `Field` intero, metadati compresi.
        extra.push(field.clone());
    }
    analyze_geometry_only(input, geometry, &extra)
}

/// `generate_grid` (v1.2, generativa): input senza geometrie (trigger); lo
/// schema di output e' nuovo (geometria nuovo `FieldId` non null +
/// `cell_i`/`cell_j`, piu' centroidi opzionali) e il numero di celle,
/// limitato da [`crate::extensions2::MAX_GRID_CELLS`], e' noto a secco
/// (`row_count` `Estimated` col valore esatto).
///
/// Le colonne del trigger non sono propagate, ma i metadati dello SCHEMA si'
/// (R2.4): riguardano il dataset, non le colonne soppresse.
pub(in crate::analyze) fn analyze_generate_grid(
    op: &str,
    input: &DataContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
    requirement: CrsRequirement,
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let parsed: GenerateGridConfig = parse_config(op, config)?;
    if !input.geometries.is_empty() {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input ha gia' una colonna geometria"
        )));
    }
    let extent = crate::extensions2::GridExtent::new(
        parsed.extent.xmin,
        parsed.extent.ymin,
        parsed.extent.xmax,
        parsed.extent.ymax,
    )
    .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: {error}")))?;
    let shape = parsed
        .shape
        .unwrap_or(crate::extensions2::GridShape::Square);
    let cells = crate::extensions2::grid_cell_count(&extent, parsed.cell_size, shape)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: {error}")))?;
    let crs = match &parsed.crs {
        Some(definition) => resolve_definition(definition, plan_crs)?,
        None => plan_crs.cloned().ok_or_else(|| {
            PlenoraError::Crs(format!(
                "{op}: CRS obbligatorio (config `crs` o CRS di piano)"
            ))
        })?,
    };
    validate_requirement(requirement, &[&crs])?;
    let mut fields = vec![new_geometry_field(
        DEFAULT_GEOMETRY_COLUMN,
        &crs,
        GeometryDimensions::Xy,
        None,
        false,
    )?];
    fields.push(Field::new(CELL_I_COLUMN, DataType::UInt64, false));
    fields.push(Field::new(CELL_J_COLUMN, DataType::UInt64, false));
    if parsed.include_centroid.unwrap_or(false) {
        fields.push(Field::new(CENTROID_X_COLUMN, DataType::Float64, false));
        fields.push(Field::new(CENTROID_Y_COLUMN, DataType::Float64, false));
    }
    let field_id = fields_allocator.alloc();
    let output_types =
        GeometryTypesProperty::new(TypesDeclaration::Exact, vec![GeometryType::Polygon])
            .map_err(|error| PlenoraError::Internal(error.to_string()))?;
    let geometry = GeometryColumnContract {
        field_id,
        name: DEFAULT_GEOMETRY_COLUMN.to_owned(),
        crs: ContractCrs::Resolved(crs),
        // Produttore (B1.3): le celle griglia sono poligoni XY — dichiara Xy.
        dimensions: GeometryDimensions::Xy,
        encoding: Some(GeometryEncoding::Wkb),
        nullable: false,
        types: ContractProperty::new(
            PropertyConfidence::Declared(output_types),
            PropertyScope::Schema,
        ),
    };
    let properties = ContractProperties {
        row_count: Some(ContractProperty::new(
            PropertyConfidence::Estimated(cells),
            PropertyScope::Dataset,
        )),
        ..ContractProperties::default()
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![geometry],
        Some(field_id),
        properties,
    )
}

/// `subdivide` (v1.2): espansione 1:N come `explode` (`__parent_index`,
/// `sorted_by` preservato, `row_count` eliminato); `output_column` rinomina
/// la colonna geometria preservando il `FieldId`.
pub(in crate::analyze) fn analyze_subdivide(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: SubdivideConfig = parse_config(op, config)?;
    if parsed.max_vertices < crate::extensions2::MIN_SUBDIVIDE_VERTICES {
        return Err(invalid_param(
            op,
            "max_vertices",
            "deve essere almeno 4 (anello chiuso minimo)",
        ));
    }
    let mut fields = output_fields(input);
    let mut output_geometry = geometry.clone();
    if let Some(name) = parsed.output_column.as_deref() {
        if !ensure_name(name) {
            return Err(invalid_param(op, "output_column", "non deve essere vuoto"));
        }
        if name != geometry.name {
            ensure_name_free(op, &fields, name)?;
            let position = fields
                .iter()
                .position(|field| field.name() == &geometry.name)
                .ok_or_else(|| {
                    PlenoraError::Schema(format!(
                        "colonna geometria `{}` assente dallo schema",
                        geometry.name
                    ))
                })?;
            let field = &fields[position];
            fields[position] = Field::new(name, field.data_type().clone(), field.is_nullable())
                .with_metadata(field.metadata().clone());
            name.clone_into(&mut output_geometry.name);
        }
    }
    ensure_name_free(op, &fields, PARENT_INDEX_COLUMN)?;
    fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
    let mut properties = input.properties.clone();
    properties.row_count = None;
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![output_geometry],
        input.active_geometry,
        properties,
    )
}

/// `snap` (v1.2): 1:1 in place; `reference_wkb` validato strutturalmente e
/// decodificato in analisi, `tolerance` finita e non negativa.
pub(in crate::analyze) fn analyze_snap(
    op: &str,
    input: &DataContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: SnapConfig = parse_config(op, config)?;
    let bytes = validate_wkb_hex(op, "reference_wkb", &parsed.reference_wkb)?;
    crate::geometry_from_wkb(&bytes)
        .map_err(|_| invalid_param(op, "reference_wkb", "WKB non decodificabile"))?;
    ensure_non_negative(op, "tolerance", parsed.tolerance)?;
    Ok(input.clone())
}
