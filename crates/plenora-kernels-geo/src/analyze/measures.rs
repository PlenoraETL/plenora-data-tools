//! Misure e diagnostica: inferenza per le op che aggiungono colonne scalari
//! (`bounds_extractor`, `geometry_accessors`, `line_locate_point`, misure) o
//! sostituiscono la geometria con le colonne diagnostiche.

use std::sync::Arc;

use plenora_core::arrow::{DataType, Field, Schema};
use plenora_core::contract::{DataContract, GeometryColumnContract};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::config::{GeometryAccessorsConfig, LineLocatePointConfig, OutputColumnConfig};
use super::helpers::{
    ensure_name, ensure_name_free, invalid_param, output_fields, output_name, parse_config,
    rebuild, short_id, validate_wkb_hex,
};
use super::producers::analyze_add_column;
use super::{ACCESSOR_COLUMNS, DIAGNOSTIC_COLUMNS, FRACTION_COLUMN};

/// `bounds_extractor`: quattro colonne `{geometria}_minx/miny/maxx/maxy`.
pub(in crate::analyze) fn analyze_bounds(op: &str, input: &DataContract, geometry: &GeometryColumnContract) -> Result<DataContract> {
    let mut fields = output_fields(input);
    for suffix in ["minx", "miny", "maxx", "maxy"] {
        let name = format!("{}_{suffix}", geometry.name);
        ensure_name_free(op, &fields, &name)?;
        fields.push(Field::new(name, DataType::Float64, true));
    }
    rebuild(input, fields, input.properties.clone())
}

/// `geometry_diagnostics`: la colonna geometria e' sostituita dalle 10
/// colonne diagnostiche; il contratto diventa non-geografico.
pub(in crate::analyze) fn analyze_diagnostics(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
) -> Result<DataContract> {
    let mut fields = output_fields(input);
    let position = fields
        .iter()
        .position(|field| field.name() == &geometry.name)
        .ok_or_else(|| {
            PlenoraError::Schema(format!(
                "colonna geometria `{}` assente dallo schema",
                geometry.name
            ))
        })?;
    for (name, _) in &DIAGNOSTIC_COLUMNS {
        if fields
            .iter()
            .enumerate()
            .any(|(index, field)| index != position && field.name() == name)
        {
            return Err(PlenoraError::Schema(format!(
                "{op}: la colonna diagnostica `{name}` esiste gia' nello schema"
            )));
        }
    }
    let diagnostic_fields: Vec<Field> = DIAGNOSTIC_COLUMNS
        .iter()
        .map(|(name, data_type)| Field::new(*name, data_type.clone(), true))
        .collect();
    fields
        .splice(position..=position, diagnostic_fields)
        .for_each(drop);
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        Vec::new(),
        None,
        input.properties.clone(),
    )
}

/// `geometry_accessors`: aggiunge le colonne richieste (default tutte) con
/// prefisso opzionale; 1:1 sulle righe, proprieta' preservate.
pub(in crate::analyze) fn analyze_geometry_accessors(
    op: &str,
    input: &DataContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: GeometryAccessorsConfig = parse_config(op, config)?;
    let mut selected: Vec<usize> = match &parsed.fields {
        None => (0..ACCESSOR_COLUMNS.len()).collect(),
        Some(fields) => {
            if fields.is_empty() {
                return Err(invalid_param(op, "fields", "non deve essere vuoto"));
            }
            let mut indexes: Vec<usize> = fields.iter().map(|field| field.column_index()).collect();
            indexes.sort_unstable();
            indexes.dedup();
            if indexes.len() != fields.len() {
                return Err(invalid_param(op, "fields", "campi duplicati"));
            }
            indexes
        }
    };
    selected.sort_unstable();
    let prefix = parsed.output_prefix.as_deref().unwrap_or("");
    let mut fields = output_fields(input);
    for index in selected {
        let (name, data_type) = &ACCESSOR_COLUMNS[index];
        let name = format!("{prefix}{name}");
        if !ensure_name(&name) {
            return Err(invalid_param(op, "output_prefix", "produce un nome vuoto"));
        }
        ensure_name_free(op, &fields, &name)?;
        fields.push(Field::new(name, data_type.clone(), true));
    }
    rebuild(input, fields, input.properties.clone())
}

/// `line_locate_point`: punto da config (`point_wkb` hex, D16) validato
/// strutturalmente e per tipo (deve essere un Point); aggiunge `fraction`.
pub(in crate::analyze) fn analyze_line_locate_point(
    op: &str,
    input: &DataContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: LineLocatePointConfig = parse_config(op, config)?;
    let bytes = validate_wkb_hex(op, "point_wkb", &parsed.point_wkb)?;
    let point = crate::geometry_from_wkb(&bytes)
        .map_err(|_| invalid_param(op, "point_wkb", "WKB non decodificabile"))?;
    if !matches!(point, geo::Geometry::Point(_)) {
        return Err(invalid_param(op, "point_wkb", "deve essere un Point"));
    }
    let name = output_name(op, parsed.output_column.as_deref(), FRACTION_COLUMN)?;
    analyze_add_column(op, input, name, DataType::Float64)
}

/// Misure con colonna scalare aggiunta (`area`, `length`, ...).
pub(in crate::analyze) fn analyze_measure(op: &str, input: &DataContract, config: &Value) -> Result<DataContract> {
    let parsed: OutputColumnConfig = parse_config(op, config)?;
    let name = output_name(op, parsed.output_column.as_deref(), short_id(op))?;
    analyze_add_column(op, input, name, DataType::Float64)
}
