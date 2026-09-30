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
    CollectConfig, FromCoordsConfig, FromWktConfig, GenerateGridConfig, SnapConfig, SubdivideConfig,
};
use super::helpers::{
    ensure_name, ensure_name_free, ensure_non_negative, geometry_field, input_crs, invalid_param,
    new_geometry_field, output_fields, parametro_non_decodificabile, parse_config, rebuild,
    resolve_definition, validate_config_geometry_domain, validate_wkb_hex,
};
use super::{
    CELL_I_COLUMN, CELL_J_COLUMN, CENTROID_X_COLUMN, CENTROID_Y_COLUMN, DEFAULT_X_COLUMN,
    DEFAULT_Y_COLUMN, PARENT_INDEX_COLUMN,
};

/// `reproject`: schema invariato, CRS del contratto e campo geometria
/// riscritti sul target risolto. La sorgente deve avere un CRS risolto (mai
/// `Missing` o `DeclaredUnresolved`), della tabella integrata (lo verifica
/// il piano dei percorsi: `CRS_NOT_BUILTIN`), e un ordine degli assi
/// dichiarato normalizzato o assente; la config si verifica per intero qui,
/// con la stessa funzione del kernel
/// ([`crate::riproiezione::ReprojectParams::da_config`]): target, percorsi
/// fra i datum, regola dell'accuratezza, griglie. I file delle griglie si
/// leggono solo in esecuzione.
///
/// Il campo d'uscita e' [`crate::riproiezione::campo_riproiettato`]: `geo`
/// sul target, chiavi canoniche CRS della sorgente tolte (sostituite, mai
/// fuse con quelle del target), `axis_order` GIS normalizzato del target.
/// Righe, tipi geometrici, `FieldId` e proprieta' del contratto restano.
pub(in crate::analyze) fn analyze_reproject(
    op: &str,
    input: &DataContract,
    config: &Value,
) -> Result<DataContract> {
    let geometry = super::helpers::single_geometry(op, input)?;
    super::helpers::require_identifiable_geometry(op, input, geometry)?;
    super::helpers::require_xy_dimensions(op, geometry)?;
    // CRS risolto e ordine degli assi prima del target: dipendono dalla
    // sola sorgente, e con una sorgente non valida l'errore e' quello,
    // qualunque sia la config.
    let source = super::dispatch::require_resolved_crs(op, geometry)?;
    let field = input.schema.field_with_name(&geometry.name).map_err(|_| {
        PlenoraError::Schema(format!(
            "{op}: colonna geometria `{}` assente dallo schema",
            geometry.name
        ))
    })?;
    crate::riproiezione::richiedi_assi_normalizzati(op, field, source)?;
    let params = crate::riproiezione::ReprojectParams::da_config(op, config, source)?;
    let target = params.target().clone();
    validate_requirement(CrsRequirement::Reprojection, &[source, &target])?;
    let mut fields = output_fields(input);
    for campo in &mut fields {
        if campo.name() == &geometry.name {
            *campo = crate::riproiezione::campo_riproiettato(
                campo,
                &target,
                geometry.dimensions,
                geometry.encoding,
            )?;
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

// ---------------------------------------------------------------------------
// Inferenza per forma di risultato.
// ---------------------------------------------------------------------------

/// Aggiunge una colonna scalare in coda allo schema (geometria preservata).
///
/// `nullable` e' quello che il kernel emette davvero: la colonna e' null
/// dove la geometria della riga e' null, quindi nullable se lo e' la
/// geometria ([`geometry_nullable`]), e in piu' dove il kernel rende un
/// valore assente per una geometria presente (distanze da una geometria
/// vuota, `line_locate_point` di una non linea). Il test
/// `geo_nullabilita` del runner confronta la dichiarazione con l'uscita.
pub(in crate::analyze) fn analyze_add_column(
    op: &str,
    input: &DataContract,
    name: &str,
    data_type: DataType,
    nullable: bool,
) -> Result<DataContract> {
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(Field::new(name, data_type, nullable));
    rebuild(input, fields, input.properties.clone())
}

/// Se la colonna geometria attiva dell'ingresso ammette null (in assenza,
/// `true`: la dichiarazione prudente).
pub(in crate::analyze) fn geometry_nullable(input: &DataContract) -> bool {
    input
        .active_geometry_column()
        .is_none_or(|geometry| geometry.nullable)
}

/// Espansione 1:N (`explode`, `delaunay`, `split`): schema invariato piu'
/// `__parent_index`; `sorted_by` preservato (espansione stabile), `row_count`
/// eliminato. La geometria d'uscita non e' nullable: una riga a geometria
/// null non produce righe, e ogni parte prodotta c'e'.
pub(in crate::analyze) fn analyze_expand(op: &str, input: &DataContract) -> Result<DataContract> {
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, PARENT_INDEX_COLUMN)?;
    fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
    let mut properties = input.properties.clone();
    properties.row_count = None;
    let output = rebuild(input, fields, properties)?;
    with_active_geometry_nullability(&output, false)
}

/// Il contratto con la colonna geometria attiva dichiarata `nullable` (campo
/// e contratto di colonna), il resto invariato.
pub(in crate::analyze) fn with_active_geometry_nullability(
    contract: &DataContract,
    nullable: bool,
) -> Result<DataContract> {
    let Some(active) = contract.active_geometry_column() else {
        return Ok(contract.clone());
    };
    let (name, field_id) = (active.name.clone(), active.field_id);
    let fields: Vec<Field> = contract
        .schema
        .fields()
        .iter()
        .map(|field| {
            if field.name() == &name {
                field.as_ref().clone().with_nullable(nullable)
            } else {
                field.as_ref().clone()
            }
        })
        .collect();
    let mut geometries = contract.geometries.clone();
    for candidate in &mut geometries {
        if candidate.field_id == field_id {
            candidate.nullable = nullable;
        }
    }
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            contract.schema.metadata().clone(),
        )),
        geometries,
        contract.active_geometry,
        contract.properties.clone(),
    )
}

/// Aggregazione a sole geometrie (`dissolve`, builder, `polygonize`,
/// `line_merge`, `overlay`, `collect`): le colonne attributo non sono
/// propagate; la geometria aggregata e' nullable dove il runner la emette
/// null (`nullable`): `dissolve` e i builder su una tabella vuota o tutta
/// null (una riga null), `collect` per un gruppo di sole geometrie null;
/// mai `line_merge`, `polygonize` e `overlay`, che emettono una riga per
/// geometria prodotta.
///
/// I metadati dello SCHEMA di input restano: riguardano il dataset.
/// Le colonne `extra` sono `Field` interi: quelle copiate dall'input (chiavi
/// di `collect`) conservano i loro metadati, quelle sintetiche nascono senza.
pub(in crate::analyze) fn analyze_geometry_only(
    input: &DataContract,
    geometry: &GeometryColumnContract,
    extra: &[Field],
    nullable: bool,
) -> Result<DataContract> {
    let mut fields = vec![geometry_field(input, geometry, nullable)?];
    fields.extend(extra.iter().cloned());
    let active = input
        .active_geometry
        .filter(|active| *active == geometry.field_id);
    let aggregated = GeometryColumnContract {
        nullable,
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

/// I produttori creano la colonna geometria: un input che ne ha gia' una si
/// rifiuta.
fn require_no_geometry(op: &str, input: &DataContract) -> Result<()> {
    if !input.geometries.is_empty() {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input ha gia' una colonna geometria"
        )));
    }
    Ok(())
}

/// Il CRS di un produttore: quello della config `crs` se presente,
/// altrimenti quello di piano, altrimenti errore `Crs`; poi il requisito del
/// catalogo.
fn producer_crs(
    op: &str,
    definition: Option<&str>,
    plan_crs: Option<&ResolvedCrs>,
    requirement: CrsRequirement,
) -> Result<ResolvedCrs> {
    let crs = match definition {
        Some(definition) => resolve_definition(definition, plan_crs)?,
        None => plan_crs.cloned().ok_or_else(|| {
            PlenoraError::Crs(format!(
                "{op}: CRS obbligatorio (config `crs` o CRS di piano)"
            ))
        })?,
    };
    validate_requirement(requirement, &[&crs])?;
    Ok(crs)
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
    require_no_geometry(op, input)?;
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
    let crs = producer_crs(op, parsed.crs.as_deref(), plan_crs, requirement)?;
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(new_geometry_field(
        name,
        &crs,
        GeometryDimensions::Xy,
        None,
        false,
    )?);
    let field_id = fields_allocator.alloc()?;
    let geometry = GeometryColumnContract {
        field_id,
        name: name.to_owned(),
        crs: ContractCrs::Resolved(crs),
        // Produttore: `from_coords` costruisce punti XY — dichiara Xy.
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
    require_no_geometry(op, input)?;
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
    let crs = producer_crs(op, parsed.crs.as_deref(), plan_crs, requirement)?;
    // Null solo per una cella WKT null: un WKT non valido ferma la colonna
    // con entrambi i valori di `on_error`.
    let nullable = wkt_field.is_nullable();
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(new_geometry_field(
        name,
        &crs,
        GeometryDimensions::Xy,
        None,
        nullable,
    )?);
    let field_id = fields_allocator.alloc()?;
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
        // Produttore: il parser WKT decodifica in `Geometry<f64>` —
        // dichiara Xy.
        dimensions: GeometryDimensions::Xy,
        encoding: Some(GeometryEncoding::Wkb),
        nullable,
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
        // I gruppi escono nell'ordine naturale dei valori, con il
        // comparatore di `table.sort` (`compare_cells_typed`): un tipo che
        // non ha un confronto nativo si rifiuta qui, non in esecuzione.
        if !plenora_kernels_table::aggregation::is_sortable(field.data_type()) {
            return Err(PlenoraError::InvalidPlan(format!(
                "{op}: parametro `group_by` non valido: colonna `{name}` di tipo {:?} senza \
                 un ordine naturale",
                field.data_type()
            )));
        }
        // La colonna chiave sopravvive invariata: si clona il `Field`
        // intero, metadati compresi.
        extra.push(field.clone());
    }
    // Null solo per un gruppo di sole geometrie null.
    analyze_geometry_only(input, geometry, &extra, geometry.nullable)
}

/// `generate_grid` (generativa): input senza geometrie (trigger); lo
/// schema di output e' nuovo (geometria nuovo `FieldId` non null +
/// `cell_i`/`cell_j`, piu' centroidi opzionali) e il numero di celle,
/// limitato da [`crate::extensions2::MAX_GRID_CELLS`], e' noto a secco
/// (`row_count` `Estimated` col valore esatto).
///
/// Le colonne del trigger non sono propagate, ma i metadati dello SCHEMA si'
/// perche' riguardano il dataset, non le colonne soppresse.
pub(in crate::analyze) fn analyze_generate_grid(
    op: &str,
    input: &DataContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
    requirement: CrsRequirement,
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let parsed: GenerateGridConfig = parse_config(op, config)?;
    require_no_geometry(op, input)?;
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
    let crs = producer_crs(op, parsed.crs.as_deref(), plan_crs, requirement)?;
    // La griglia copre il rettangolo `extent`: i quattro vertici dentro il
    // dominio di validita' (un rettangolo o il mondo lon/lat) bastano.
    plenora_core::crs::validate_geometry_domain(
        [
            (parsed.extent.xmin, parsed.extent.ymin),
            (parsed.extent.xmax, parsed.extent.ymin),
            (parsed.extent.xmax, parsed.extent.ymax),
            (parsed.extent.xmin, parsed.extent.ymax),
        ]
        .into_iter(),
        &crs,
    )
    .map_err(|error| PlenoraError::Crs(format!("{op}: parametro `extent`: {error}")))?;
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
    let field_id = fields_allocator.alloc()?;
    let output_types =
        GeometryTypesProperty::new(TypesDeclaration::Exact, vec![GeometryType::Polygon])
            .map_err(|error| PlenoraError::Internal(error.to_string()))?;
    let geometry = GeometryColumnContract {
        field_id,
        name: DEFAULT_GEOMETRY_COLUMN.to_owned(),
        crs: ContractCrs::Resolved(crs),
        // Produttore: le celle griglia sono poligoni XY — dichiara Xy.
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

/// `subdivide`: espansione 1:N come `explode` (`__parent_index`,
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

/// `snap`: 1:1 in place; `reference_wkb` validato strutturalmente,
/// decodificato in analisi e nel dominio di validita' del CRS dell'input,
/// `tolerance` finita e non negativa.
pub(in crate::analyze) fn analyze_snap(
    op: &str,
    input: &DataContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: SnapConfig = parse_config(op, config)?;
    let bytes = validate_wkb_hex(op, "reference_wkb", &parsed.reference_wkb)?;
    let reference = crate::geometry_from_wkb(&bytes).map_err(|error| {
        parametro_non_decodificabile(op, "reference_wkb", "WKB non decodificabile", &error)
    })?;
    validate_config_geometry_domain(op, "reference_wkb", &reference, input_crs(op, input)?)?;
    ensure_non_negative(op, "tolerance", parsed.tolerance)?;
    Ok(input.clone())
}
