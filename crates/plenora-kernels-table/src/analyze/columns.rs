//! Analyzer a secco delle op sulle colonne (kernel `columns.rs`).

use std::collections::{HashMap, HashSet};

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{ContractProperties, DataContract, FieldAllocator};
use plenora_core::Result;
use serde_json::Value;

use super::helpers::{
    analyze_append, check_output_name, clone_fields, contract_error, field_of, finish,
    propagate_geometry, require_utf8, scrub_dropped_geometry, typed,
};
use crate::{columns, Limits};

// ---------------------------------------------------------------------------
// columns.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_drop_columns(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::DropColumns = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let to_drop: HashSet<&str> = config.columns.iter().map(String::as_str).collect();
    let kept: Vec<Field> = clone_fields(input)
        .into_iter()
        .filter(|field| !to_drop.contains(field.name().as_str()))
        .collect();
    let removed_any = kept.len() != input.schema.fields().len();
    let schema = Schema::new_with_metadata(kept, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    let dropped_geometry = if geometry.is_none() {
        input.geometries.first().map(|g| g.field_id)
    } else {
        None
    };
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    // Conservativo: una colonna rimossa puo' essere una chiave di ordinamento.
    let properties = scrub_dropped_geometry(
        ContractProperties {
            sorted_by: if removed_any {
                None
            } else {
                input.properties.sorted_by.clone()
            },
            row_count: input.properties.row_count.clone(),
        },
        dropped_geometry,
    );
    finish(schema, geometry, active, properties)
}

pub(in crate::analyze) fn analyze_rename(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::Rename = typed(op, config)?;
    let input = &inputs[0];
    // Come il kernel: old_name inesistente ignorato, duplicati -> vince l'ultimo.
    let renames: HashMap<&str, &str> = config
        .renames
        .iter()
        .map(|pair| (pair.old_name.as_str(), pair.new_name.as_str()))
        .collect();
    let mut fields_out = Vec::with_capacity(input.schema.fields().len());
    let mut names: HashSet<String> = HashSet::new();
    for field in clone_fields(input) {
        let name = renames
            .get(field.name().as_str())
            .copied()
            .unwrap_or_else(|| field.name().as_str())
            .to_owned();
        check_output_name(op, &name)?;
        if !names.insert(name.clone()) {
            return contract_error(op, format!("rename produce il nome duplicato: {name}"));
        }
        fields_out.push(field.with_name(name));
    }
    for pair in &config.renames {
        fields.rename(&pair.old_name, &pair.new_name);
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let renamed_geometry = input.geometries.first().map(|geometry| {
        renames
            .get(geometry.name.as_str())
            .copied()
            .unwrap_or(geometry.name.as_str())
    });
    let geometry = propagate_geometry(input, &schema, renamed_geometry);
    // Rinomina: FieldId preservati (D16), righe e ordine invariati.
    finish(schema, geometry, input.active_geometry, input.properties.clone())
}

pub(in crate::analyze) fn analyze_reorder_columns(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::ReorderColumns = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let mut seen: HashSet<&str> = HashSet::new();
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("colonna ripetuta in columns: {name}"));
        }
        field_of(op, input, name)?;
    }
    let mut ordered: Vec<Field> = config
        .columns
        .iter()
        .map(|name| field_of(op, input, name).cloned())
        .collect::<Result<_>>()?;
    let mut rest: Vec<Field> = clone_fields(input)
        .into_iter()
        .filter(|field| !seen.contains(field.name().as_str()))
        .collect();
    if config.alphabetical {
        rest.sort_by_key(|field| field.name().to_lowercase());
    }
    ordered.extend(rest);
    let schema = Schema::new_with_metadata(ordered, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    finish(schema, geometry, input.active_geometry, input.properties.clone())
}

pub(in crate::analyze) fn analyze_concat_columns(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::ConcatColumns = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    check_output_name(op, &config.output_column)?;
    for name in &config.columns {
        require_utf8(op, input, name)?;
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, true)],
    )
}

pub(in crate::analyze) fn analyze_split_column(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::SplitColumn = typed(op, config)?;
    let input = &inputs[0];
    if config.delimiter.is_empty() {
        return contract_error(op, "delimiter vuoto");
    }
    if config.new_columns.is_empty() {
        return contract_error(op, "new_columns vuoto");
    }
    if config.new_columns.len() > Limits::default().max_split_columns {
        return contract_error(op, "new_columns oltre max_split_columns");
    }
    require_utf8(op, input, &config.column)?;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut produced = Vec::with_capacity(config.new_columns.len());
    for name in &config.new_columns {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("new_columns duplicato: {name}"));
        }
        check_output_name(op, name)?;
        produced.push((name.clone(), DataType::Utf8, true));
    }
    analyze_append(input, fields, &produced)
}

pub(in crate::analyze) fn analyze_select_columns(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::SelectColumns = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut fields_out = Vec::with_capacity(config.columns.len());
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("colonna ripetuta in columns: {name}"));
        }
        fields_out.push(field_of(op, input, name)?.clone());
    }
    let removed_any = fields_out.len() != input.schema.fields().len();
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    let dropped_geometry = if geometry.is_none() {
        input.geometries.first().map(|g| g.field_id)
    } else {
        None
    };
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    // Come drop_columns: una colonna rimossa puo' essere chiave di ordinamento.
    let properties = scrub_dropped_geometry(
        ContractProperties {
            sorted_by: if removed_any {
                None
            } else {
                input.properties.sorted_by.clone()
            },
            row_count: input.properties.row_count.clone(),
        },
        dropped_geometry,
    );
    finish(schema, geometry, active, properties)
}

pub(in crate::analyze) fn analyze_align_schema(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::AlignSchema = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut fields_out = Vec::with_capacity(config.columns.len());
    let mut added_any = false;
    for declared in &config.columns {
        check_output_name(op, &declared.name)?;
        if !seen.insert(declared.name.as_str()) {
            return contract_error(op, format!("colonna ripetuta in columns: {}", declared.name));
        }
        let data_type = declared.align_type.data_type();
        if let Ok(field) = input.schema.field_with_name(&declared.name) {
            // Mai cast implicito: il tipo dichiarato deve essere identico.
            if field.data_type() != &data_type {
                return contract_error(
                    op,
                    format!(
                        "colonna {} di tipo {:?}, atteso {:?} (nessun cast implicito)",
                        declared.name,
                        field.data_type(),
                        data_type
                    ),
                );
            }
            fields_out.push(field.clone());
        } else {
            added_any = true;
            // Il default e' validato a secco con la stessa conversione del
            // kernel: mai un errore a meta' dei dati.
            if let Some(default) = &declared.default {
                columns::check_align_default(default, declared.align_type)?;
            }
            fields.derive(&declared.name);
            fields_out.push(Field::new(
                &declared.name,
                data_type,
                declared.default.is_none(),
            ));
        }
    }
    let removed_any = !config.keep_extra
        && input
            .schema
            .fields()
            .iter()
            .any(|field| !seen.contains(field.name().as_str()));
    if config.keep_extra {
        for field in input.schema.fields() {
            if !seen.contains(field.name().as_str()) {
                fields_out.push(field.as_ref().clone());
            }
        }
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let geometry =
        propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    let dropped_geometry = if geometry.is_none() {
        input.geometries.first().map(|g| g.field_id)
    } else {
        None
    };
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    // 1:1 sulle righe: row_count invariato. sorted_by solo se l'insieme delle
    // colonne e' invariato (permutazione pura, nessuna chiave rimossa).
    let properties = scrub_dropped_geometry(
        ContractProperties {
            sorted_by: if added_any || removed_any {
                None
            } else {
                input.properties.sorted_by.clone()
            },
            row_count: input.properties.row_count.clone(),
        },
        dropped_geometry,
    );
    finish(schema, geometry, active, properties)
}

