//! Analyzer a secco delle op di reshape (kernel `reshape.rs`).

use std::collections::HashSet;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{ContractProperties, DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    analyze_append, check_output_name, clone_fields, contract_error, field_of, finish,
    map_row_count, merge_geometry, merge_schema_metadata, propagate_geometry, typed, unsupported,
};
use super::joins::check_key_pairs;
use crate::{reshape, Limits};

// ---------------------------------------------------------------------------
// reshape.rs
// ---------------------------------------------------------------------------

/// Replica `collision_free` del kernel melt: nome libero nello schema di
/// input, altrimenti `{name}_1..{name}_99`.
fn collision_free(op: &str, input: &DataContract, name: &str) -> Result<String> {
    check_output_name(op, name)?;
    if input.schema.index_of(name).is_err() {
        return Ok(name.to_owned());
    }
    (1..100)
        .map(|index| format!("{name}_{index}"))
        .find(|candidate| input.schema.index_of(candidate).is_err())
        .ok_or_else(|| {
            PlenoraError::InvalidPlan(format!("{op}: impossibile evitare collisione {name}"))
        })
}

pub(in crate::analyze) fn analyze_melt(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::Melt = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let id_indices: Vec<usize> = config
        .id_columns
        .iter()
        .map(|name| {
            input
                .schema
                .index_of(name)
                .map_err(|_| PlenoraError::InvalidPlan(format!("{op}: colonna non trovata: {name}")))
        })
        .collect::<Result<_>>()?;
    let value_indices: Vec<usize> = if config.value_columns.is_empty() {
        (0..input.schema.fields().len())
            .filter(|index| !id_indices.contains(index))
            .collect()
    } else {
        config
            .value_columns
            .iter()
            .map(|name| {
                input.schema.index_of(name).map_err(|_| {
                    PlenoraError::InvalidPlan(format!("{op}: colonna non trovata: {name}"))
                })
            })
            .collect::<Result<_>>()?
    };
    if value_indices.is_empty() {
        return contract_error(op, "melt senza value_columns");
    }
    let source_fields = clone_fields(input);
    let value_type = source_fields[value_indices[0]].data_type().clone();
    let homogeneous = value_indices
        .iter()
        .all(|index| source_fields[*index].data_type() == &value_type);
    let value_data_type = if homogeneous {
        value_type
    } else if matches!(config.type_policy, reshape::HeterogeneousTypePolicy::String) {
        DataType::Utf8
    } else {
        return contract_error(
            op,
            "value_columns eterogenee; impostare type_policy='string' per la conversione esplicita",
        );
    };
    let var_name = collision_free(op, input, &config.var_name)?;
    let value_name = collision_free(op, input, &config.value_name)?;
    let mut fields_out: Vec<Field> = id_indices
        .iter()
        .map(|index| source_fields[*index].clone())
        .collect();
    fields_out.push(Field::new(&var_name, DataType::Utf8, false));
    fields_out.push(Field::new(&value_name, value_data_type, true));
    // R2.4: i metadata dello schema di input si conservano (le colonne id
    // sono passthrough); `variable`/`value` sono colonne derivate e non
    // ereditano metadata di campo.
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    // La geometria sopravvive solo come colonna id (valori passthrough).
    let preserved = input
        .geometries
        .first()
        .filter(|geometry| config.id_columns.contains(&geometry.name))
        .map(|geometry| geometry.name.as_str());
    let geometry = propagate_geometry(input, &schema, preserved);
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    let value_count = u64::try_from(value_indices.len()).unwrap_or(u64::MAX);
    finish(
        schema,
        geometry,
        active,
        ContractProperties {
            sorted_by: None,
            row_count: map_row_count(input, |rows| rows.saturating_mul(value_count)),
        },
    )
}

pub(in crate::analyze) fn analyze_pivot(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    // Valida la config e le colonne referenziate, poi fallisce: i nomi delle
    // colonne pivot derivano dai valori presenti nei dati (anche con mapping,
    // che filtra ma non garantisce la presenza).
    let config: reshape::Pivot = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    for name in config
        .index_col
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        field_of(op, input, name)?;
    }
    field_of(op, input, &config.column)?;
    field_of(op, input, &config.value_col)?;
    unsupported(
        op,
        "le colonne di output dipendono dai valori distinti della pivot_col: schema non inferibile a secco",
    )
}

pub(in crate::analyze) fn analyze_transpose(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::Transpose = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    if let Some(id_column) = &config.id_column {
        field_of(op, input, id_column)?;
    }
    unsupported(
        op,
        "il numero di colonne di output dipende dal numero di righe dell'input: schema non inferibile a secco",
    )
}

pub(in crate::analyze) fn analyze_explode(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::Explode = typed(op, config)?;
    let input = &inputs[0];
    let field = field_of(op, input, &config.column)?;
    let DataType::List(child) = field.data_type() else {
        return contract_error(op, "explode richiede una colonna List");
    };
    let element_type = child.data_type().clone();
    let output_name = config.output_column.unwrap_or_else(|| config.column.clone());
    check_output_name(op, &output_name)?;
    let mut output = analyze_append(input, fields, &[(output_name, element_type, true)])?;
    // Le righe cambiano (una per elemento); l'espansione puo' rompere
    // l'ordinamento se la colonna esplosa era una chiave: conservativo.
    output.properties = ContractProperties::default();
    Ok(output)
}

pub(in crate::analyze) fn analyze_unnest(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::Unnest = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let index = input
        .schema
        .index_of(&config.column)
        .map_err(|_| PlenoraError::InvalidPlan(format!("{op}: colonna non trovata: {}", config.column)))?;
    let field = input.schema.field(index);
    let DataType::Struct(children) = field.data_type() else {
        return contract_error(op, "unnest richiede una colonna Struct");
    };
    let projected = input
        .schema
        .fields()
        .len()
        .saturating_sub(usize::from(config.drop_source))
        .saturating_add(children.len());
    if projected > Limits::default().max_columns {
        return contract_error(op, "unnest supera max_columns");
    }
    let mut fields_out: Vec<Field> = Vec::with_capacity(projected);
    let mut names: HashSet<String> = HashSet::new();
    for (position, field) in clone_fields(input).into_iter().enumerate() {
        if position == index && config.drop_source {
            continue;
        }
        names.insert(field.name().clone());
        fields_out.push(field);
    }
    for child in children {
        let name = format!("{}{}", config.prefix, child.name());
        check_output_name(op, &name)?;
        if !names.insert(name.clone()) {
            return contract_error(op, format!("unnest: collisione colonna {name}"));
        }
        fields_out.push(child.as_ref().clone().with_name(name).with_nullable(true));
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    finish(
        schema,
        geometry,
        active,
        ContractProperties {
            // Con drop_source la colonna struct (possibile chiave) e' rimossa.
            sorted_by: if config.drop_source {
                None
            } else {
                input.properties.sorted_by.clone()
            },
            row_count: input.properties.row_count.clone(),
        },
    )
}

pub(in crate::analyze) fn analyze_table_diff(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::TableDiff = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
        return contract_error(op, "chiavi table_diff non valide");
    }
    check_key_pairs(op, left, right, &config.left_keys, &config.right_keys)?;
    let compare: Vec<String> = if config.compare_columns.is_empty() {
        // Default: colonne di left non chiave presenti anche in right.
        left.schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .filter(|name| {
                !config.left_keys.contains(name) && right.schema.index_of(name).is_ok()
            })
            .collect()
    } else {
        config.compare_columns.clone()
    };
    let mut fields_out: Vec<Field> = Vec::new();
    for name in &config.left_keys {
        let field = field_of(op, left, name)?;
        fields_out.push(Field::new(name, field.data_type().clone(), true));
    }
    for name in &compare {
        field_of(op, left, name)?;
        let right_field = field_of(op, right, name)?;
        if field_of(op, left, name)?.data_type() != right_field.data_type() {
            return contract_error(
                op,
                format!("colonna {name} con tipi Arrow diversi tra left e right"),
            );
        }
        fields_out.push(Field::new(name, right_field.data_type().clone(), true));
    }
    fields_out.push(Field::new("_diff_status", DataType::Utf8, false));
    fields_out.push(Field::new("_diff_columns", DataType::Utf8, true));
    fields_out.push(Field::new("_diff_old_values", DataType::Utf8, true));
    // R2.4: chiavi e colonne di confronto sono ricostruite (nullability
    // forzata, valori provenienti da entrambe le sorgenti) — classificate
    // derivate: nessun metadata di campo ereditato. I metadata di SCHEMA
    // delle due sorgenti si fondono invece con la merge-policy dei join.
    let metadata = merge_schema_metadata(op, &left.schema, &right.schema)?;
    let schema = Schema::new_with_metadata(fields_out, metadata);
    let emitted = |name: &str| config.left_keys.contains(&name.to_owned()) || compare.iter().any(|c| c == name);
    let left_geometry = left
        .geometries
        .first()
        .filter(|geometry| emitted(&geometry.name))
        .and_then(|geometry| propagate_geometry(left, &schema, Some(&geometry.name)));
    let right_geometry = if left_geometry.is_none() {
        right
            .geometries
            .first()
            .filter(|geometry| emitted(&geometry.name))
            .and_then(|geometry| propagate_geometry(right, &schema, Some(&geometry.name)))
    } else {
        None
    };
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    finish(schema, geometry, None, ContractProperties::default())
}

