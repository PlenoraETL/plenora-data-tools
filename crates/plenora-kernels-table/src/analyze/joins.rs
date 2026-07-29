//! Analyzer a secco di join e set operation
//! (kernel `joins.rs` / `setops.rs` / `fuzzy.rs`).

use std::collections::HashSet;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractProperties, ContractProperty, DataContract, FieldAllocator, PropertyConfidence,
};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    check_output_name, clone_fields, contract_error, field_of, finish, is_scalar_string,
    merge_geometry, merge_schema_metadata, merge_schema_metadata_many, propagate_geometry,
    require_utf8, sorted_only, typed,
};
use super::quality::check_foreign_keys;
use crate::{fuzzy, joins, setops, Limits};

// ---------------------------------------------------------------------------
// joins.rs / setops.rs
// ---------------------------------------------------------------------------

/// Chiavi di join con pari cardinalita' obbligatoria e tipi identici.
pub(in crate::analyze) fn check_key_pairs(
    op: &str,
    left: &DataContract,
    right: &DataContract,
    left_keys: &[String],
    right_keys: &[String],
) -> Result<()> {
    if left_keys.is_empty() || left_keys.len() != right_keys.len() {
        return contract_error(op, "chiavi non valide (vuote o cardinalita' diversa)");
    }
    check_foreign_keys(op, left, right, left_keys, right_keys)
}

/// Replica `combine_horizontal`: tutte le colonne left (con naming per
/// variante), poi le colonne right non omesse; nullability forzata a true;
/// collisioni -> errore. Restituisce (campi, nome left, nome right) della
/// colonna geometrica propagata per ciascun ramo.
#[allow(clippy::too_many_arguments)]
fn combine_horizontal_fields(
    op: &str,
    left: &DataContract,
    right: &DataContract,
    omitted_right: &HashSet<usize>,
    naming: HorizontalNaming<'_>,
) -> Result<(Vec<Field>, Option<String>, Option<String>)> {
    let right_names: HashSet<&str> = right
        .schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(index, _)| !omitted_right.contains(index))
        .map(|(_, field)| field.name().as_str())
        .collect();
    let left_key_indices: HashSet<usize> = match naming {
        HorizontalNaming::ManipolaJoin(indices) => indices.iter().copied().collect(),
        _ => HashSet::new(),
    };
    let mut fields_out: Vec<Field> = Vec::new();
    let mut left_geometry_name = None;
    for (index, field) in clone_fields(left).into_iter().enumerate() {
        let original = field.name().clone();
        let name = match naming {
            HorizontalNaming::ManipolaJoin(_) if !left_key_indices.contains(&index) => {
                format!("{original}_L")
            }
            HorizontalNaming::PandasCross if right_names.contains(original.as_str()) => {
                format!("{original}_x")
            }
            _ => original.clone(),
        };
        check_output_name(op, &name)?;
        if left.geometries.first().is_some_and(|g| g.name == original) {
            left_geometry_name = Some(name.clone());
        }
        fields_out.push(field.with_name(name).with_nullable(true));
    }
    let mut names: HashSet<String> = fields_out
        .iter()
        .map(|field| field.name().clone())
        .collect();
    if names.len() != fields_out.len() {
        return contract_error(op, "collisione nomi nelle colonne sinistre del join");
    }
    let left_source_names: HashSet<&str> = left
        .schema
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .collect();
    let mut right_geometry_name = None;
    for (index, field) in clone_fields(right).into_iter().enumerate() {
        if omitted_right.contains(&index) {
            continue;
        }
        let original = field.name().clone();
        let name = match naming {
            HorizontalNaming::ManipolaJoin(_) => format!("{original}_R"),
            HorizontalNaming::PandasCross if left_source_names.contains(original.as_str()) => {
                format!("{original}_y")
            }
            HorizontalNaming::AsOf if left_source_names.contains(original.as_str()) => {
                format!("{original}_R")
            }
            _ => original.clone(),
        };
        check_output_name(op, &name)?;
        if !names.insert(name.clone()) {
            return contract_error(op, format!("collisione join: {name}"));
        }
        if right.geometries.first().is_some_and(|g| g.name == original) {
            right_geometry_name = Some(name.clone());
        }
        fields_out.push(field.with_name(name).with_nullable(true));
    }
    if fields_out.len() > Limits::default().max_columns {
        return contract_error(op, "join supera max_columns");
    }
    Ok((fields_out, left_geometry_name, right_geometry_name))
}

#[derive(Clone, Copy)]
enum HorizontalNaming<'a> {
    ManipolaJoin(&'a [usize]),
    PandasCross,
    AsOf,
}

pub(in crate::analyze) fn analyze_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: joins::Join = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    check_key_pairs(op, left, right, &config.left_keys, &config.right_keys)?;
    let left_indices: Vec<usize> = config
        .left_keys
        .iter()
        .map(|name| {
            left.schema.index_of(name).map_err(|_| {
                PlenoraError::Internal(format!(
                    "{op}: chiave verificata assente nello schema"
                ))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let right_indices: HashSet<usize> = config
        .right_keys
        .iter()
        .map(|name| {
            right.schema.index_of(name).map_err(|_| {
                PlenoraError::Internal(format!(
                    "{op}: chiave verificata assente nello schema"
                ))
            })
        })
        .collect::<Result<HashSet<_>>>()?;
    let (fields_out, left_geometry, right_geometry) = combine_horizontal_fields(
        op,
        left,
        right,
        &right_indices,
        HorizontalNaming::ManipolaJoin(&left_indices),
    )?;
    // R2.4: i metadata di schema delle due sorgenti si fondono; stessa chiave
    // con valori diversi -> errore, mai precedenza implicita.
    let metadata = merge_schema_metadata(op, &left.schema, &right.schema)?;
    let schema = Schema::new_with_metadata(fields_out, metadata);
    let left_geometry = propagate_geometry(left, &schema, left_geometry.as_deref());
    let right_geometry = propagate_geometry(right, &schema, right_geometry.as_deref());
    let active = left
        .active_geometry
        .filter(|id| left_geometry.as_ref().is_some_and(|g| &g.field_id == id))
        .or_else(|| {
            right
                .active_geometry
                .filter(|id| right_geometry.as_ref().is_some_and(|g| &g.field_id == id))
        });
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    finish(schema, geometry, active, ContractProperties::default())
}

/// `table.fuzzy_join` (estensione v1.3): replica le validazioni statiche del
/// kernel (`fuzzy::validate_config`, chiavi Utf8 esistenti) e inferisce lo
/// schema del join Manipola con la chiave destra INCLUSA (suffisso `_R`,
/// nel fuzzy le due chiavi differiscono) piu' la colonna score Float64 in
/// coda (nullable solo con `how = left`). Proprieta' declassate: il numero di
/// righe dipende dai dati e l'ordine non e' quello di nessun input.
pub(in crate::analyze) fn analyze_fuzzy_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: fuzzy::FuzzyJoin = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    fuzzy::validate_config(&config)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: {error}")))?;
    require_utf8(op, left, &config.left_key)?;
    require_utf8(op, right, &config.right_key)?;
    let left_index = left.schema.index_of(&config.left_key).map_err(|_| {
        PlenoraError::Internal(format!(
            "{op}: chiave verificata assente nello schema"
        ))
    })?;
    let (mut fields_out, left_geometry, right_geometry) = combine_horizontal_fields(
        op,
        left,
        right,
        &HashSet::new(),
        HorizontalNaming::ManipolaJoin(&[left_index]),
    )?;
    let score_name = config.score_name();
    check_output_name(op, score_name)?;
    if fields_out.iter().any(|field| field.name() == score_name) {
        return contract_error(op, format!("collisione fuzzy_join: {score_name}"));
    }
    fields_out.push(Field::new(
        score_name,
        DataType::Float64,
        config.how == fuzzy::FuzzyHow::Left,
    ));
    if fields_out.len() > Limits::default().max_columns {
        return contract_error(op, "fuzzy_join supera max_columns");
    }
    // R2.4: merge dei metadata di schema delle due sorgenti (come `table.join`).
    let metadata = merge_schema_metadata(op, &left.schema, &right.schema)?;
    let schema = Schema::new_with_metadata(fields_out, metadata);
    let left_geometry = propagate_geometry(left, &schema, left_geometry.as_deref());
    let right_geometry = propagate_geometry(right, &schema, right_geometry.as_deref());
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    finish(schema, geometry, None, ContractProperties::default())
}

pub(in crate::analyze) fn analyze_cross_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let _config: joins::CrossJoin = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    let (fields_out, left_geometry, right_geometry) = combine_horizontal_fields(
        op,
        left,
        right,
        &HashSet::new(),
        HorizontalNaming::PandasCross,
    )?;
    // R2.4: merge dei metadata di schema delle due sorgenti (come `table.join`).
    let metadata = merge_schema_metadata(op, &left.schema, &right.schema)?;
    let schema = Schema::new_with_metadata(fields_out, metadata);
    let left_geometry = propagate_geometry(left, &schema, left_geometry.as_deref());
    let right_geometry = propagate_geometry(right, &schema, right_geometry.as_deref());
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    // Righe = prodotto esatto delle cardinalita' (se note e stesso scope).
    let row_count = product_row_count(left, right);
    finish(
        schema,
        geometry,
        None,
        ContractProperties {
            sorted_by: None,
            row_count,
        },
    )
}

fn product_row_count(
    left: &DataContract,
    right: &DataContract,
) -> Option<ContractProperty<u64>> {
    let left_property = left.properties.row_count.as_ref()?;
    let right_property = right.properties.row_count.as_ref()?;
    if left_property.scope != right_property.scope {
        return None;
    }
    let combine = |a: &u64, b: &u64| a.checked_mul(*b);
    let confidence = match (&left_property.confidence, &right_property.confidence) {
        (PropertyConfidence::Proven(a), PropertyConfidence::Proven(b)) => {
            PropertyConfidence::Proven(combine(a, b)?)
        }
        (PropertyConfidence::Estimated(a), PropertyConfidence::Estimated(b)) => {
            PropertyConfidence::Estimated(combine(a, b)?)
        }
        _ => return None,
    };
    Some(ContractProperty::new(confidence, left_property.scope))
}

pub(in crate::analyze) fn analyze_membership_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: joins::MembershipJoin = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    check_key_pairs(op, left, right, &config.left_keys, &config.right_keys)?;
    // Output = left via select_rows: schema invariato, ordine preservato.
    let mut output = left.clone();
    output.properties = sorted_only(left);
    Ok(output)
}

pub(in crate::analyze) fn analyze_asof_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: joins::AsOfJoin = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    if config.left_by.len() != config.right_by.len() {
        return contract_error(op, "left_by/right_by di cardinalita' diversa");
    }
    if config
        .tolerance
        .is_some_and(|tolerance| !tolerance.is_finite() || tolerance < 0.0)
    {
        return contract_error(op, "tolerance deve essere finita e >= 0");
    }
    let left_on = field_of(op, left, &config.left_on)?;
    let right_on = field_of(op, right, &config.right_on)?;
    if left_on.data_type() != right_on.data_type()
        || !matches!(left_on.data_type(), DataType::Int64 | DataType::Float64)
    {
        return contract_error(op, "left_on/right_on devono avere tipo identico Int64 o Float64");
    }
    check_foreign_keys(op, left, right, &config.left_by, &config.right_by)?;
    let omitted: HashSet<usize> = config
        .right_by
        .iter()
        .chain(std::iter::once(&config.right_on))
        .map(|name| {
            right.schema.index_of(name).map_err(|_| {
                PlenoraError::Internal(format!(
                    "{op}: chiave verificata assente nello schema"
                ))
            })
        })
        .collect::<Result<HashSet<_>>>()?;
    let (fields_out, left_geometry, right_geometry) =
        combine_horizontal_fields(op, left, right, &omitted, HorizontalNaming::AsOf)?;
    // R2.4: merge dei metadata di schema delle due sorgenti (come `table.join`).
    let metadata = merge_schema_metadata(op, &left.schema, &right.schema)?;
    let schema = Schema::new_with_metadata(fields_out, metadata);
    let left_geometry = propagate_geometry(left, &schema, left_geometry.as_deref());
    let right_geometry = propagate_geometry(right, &schema, right_geometry.as_deref());
    let active = left
        .active_geometry
        .filter(|id| left_geometry.as_ref().is_some_and(|g| &g.field_id == id))
        .or_else(|| {
            right
                .active_geometry
                .filter(|id| right_geometry.as_ref().is_some_and(|g| &g.field_id == id))
        });
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    // Una riga di output per riga left, nell'ordine di left.
    finish(schema, geometry, active, left.properties.clone())
}

pub(in crate::analyze) fn analyze_concat(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let _config: joins::Concat = typed(op, config)?;
    let _ = fields;
    let first = &inputs[0];
    for other in &inputs[1..] {
        check_same_schema(op, first, other)?;
    }
    let fields_out: Vec<Field> = first
        .schema
        .fields()
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let nullable = inputs
                .iter()
                .any(|input| input.schema.field(index).is_nullable());
            field.as_ref().clone().with_nullable(nullable)
        })
        .collect();
    // R2.4 multi-sorgente: merge dei metadata di schema di tutti gli
    // input (conflitto su valori diversi -> errore, mai "vince il primo").
    let schema = Schema::new_with_metadata(fields_out, merge_schema_metadata_many(op, inputs)?);
    // Geometria del primo input (gli schemi sono identici per nome/tipo).
    let geometry = propagate_geometry(first, &schema, first.geometries.first().map(|g| g.name.as_str()));
    let row_count = sum_row_count(inputs);
    finish(
        schema,
        geometry,
        first.active_geometry,
        ContractProperties {
            sorted_by: None,
            row_count,
        },
    )
}

fn sum_row_count(inputs: &[DataContract]) -> Option<ContractProperty<u64>> {
    let first = inputs.first()?.properties.row_count.as_ref()?;
    if inputs.iter().any(|input| {
        input
            .properties
            .row_count
            .as_ref()
            .is_none_or(|property| property.scope != first.scope)
    }) {
        return None;
    }
    let confidence = match &first.confidence {
        PropertyConfidence::Proven(_) => {
            let mut total = 0_u64;
            for input in inputs {
                let value = input.properties.row_count.as_ref()?.confidence.proven_value()?;
                total = total.checked_add(*value)?;
            }
            PropertyConfidence::Proven(total)
        }
        PropertyConfidence::Estimated(_) => {
            let mut total = 0_u64;
            for input in inputs {
                let value = input.properties.row_count.as_ref()?.confidence.value().copied()?;
                if !matches!(
                    input.properties.row_count.as_ref()?.confidence,
                    PropertyConfidence::Estimated(_)
                ) {
                    return None;
                }
                total = total.checked_add(value)?;
            }
            PropertyConfidence::Estimated(total)
        }
        _ => return None,
    };
    Some(ContractProperty::new(confidence, first.scope))
}

/// Replica `validate_schema`/`concat_compatible` del kernel: stesso numero di
/// colonne, nomi e `DataType` identici campo per campo (nullability ignorata).
fn check_same_schema(op: &str, left: &DataContract, right: &DataContract) -> Result<()> {
    if left.schema.fields().len() != right.schema.fields().len() {
        return contract_error(op, "schemi con numero di colonne diverso");
    }
    for (left_field, right_field) in left
        .schema
        .fields()
        .iter()
        .zip(right.schema.fields().iter())
    {
        if left_field.name() != right_field.name()
            || left_field.data_type() != right_field.data_type()
        {
            return contract_error(
                op,
                format!(
                    "schemi incompatibili: {} ({:?}) vs {} ({:?})",
                    left_field.name(),
                    left_field.data_type(),
                    right_field.name(),
                    right_field.data_type()
                ),
            );
        }
    }
    Ok(())
}

pub(in crate::analyze) fn analyze_concat_by_name(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: joins::ConcatByName = typed(op, config)?;
    let _ = fields;
    let first = &inputs[0];
    // Schema unione: stesse regole del kernel (ordine di prima apparizione,
    // tipi identici per nome, nullable se assente in almeno un input).
    let fields_out: Vec<Field> = if config.strict {
        for other in &inputs[1..] {
            check_same_schema(op, first, other)?;
        }
        first
            .schema
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let nullable = inputs
                    .iter()
                    .any(|input| input.schema.field(index).is_nullable());
                field.as_ref().clone().with_nullable(nullable)
            })
            .collect()
    } else {
        let mut union: Vec<Field> = Vec::new();
        for input in inputs {
            for field in input.schema.fields() {
                if let Some(existing) = union.iter_mut().find(|f| f.name() == field.name()) {
                    if existing.data_type() != field.data_type() {
                        return contract_error(
                            op,
                            format!(
                                "tipi incompatibili per la colonna {} ({:?} vs {:?})",
                                field.name(),
                                existing.data_type(),
                                field.data_type()
                            ),
                        );
                    }
                    if field.is_nullable() && !existing.is_nullable() {
                        *existing = existing.clone().with_nullable(true);
                    }
                } else {
                    union.push(field.as_ref().clone());
                }
            }
        }
        for field in &mut union {
            if !field.is_nullable()
                && inputs
                    .iter()
                    .any(|input| input.schema.field_with_name(field.name()).is_err())
            {
                *field = field.clone().with_nullable(true);
            }
        }
        union
    };
    // R2.4 multi-sorgente: merge dei metadata di schema di tutti gli
    // input (conflitto su valori diversi -> errore, mai "vince il primo").
    let schema = Schema::new_with_metadata(fields_out, merge_schema_metadata_many(op, inputs)?);
    // Geometria: propagata solo se TUTTI gli input hanno la colonna con lo
    // stesso tipo (altrimenti le righe degli input senza la colonna
    // sarebbero null materializzati, non passthrough D16).
    let geometry = first
        .geometries
        .first()
        .filter(|geometry| {
            inputs.iter().all(|input| {
                input
                    .schema
                    .field_with_name(&geometry.name)
                    .is_ok_and(|field| {
                        Some(field.data_type())
                            == first
                                .schema
                                .field_with_name(&geometry.name)
                                .ok()
                                .map(plenora_core::arrow::Field::data_type)
                    })
            })
        })
        .and_then(|geometry| propagate_geometry(first, &schema, Some(&geometry.name)));
    let active = first
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    finish(
        schema,
        geometry,
        active,
        ContractProperties {
            sorted_by: None,
            row_count: sum_row_count(inputs),
        },
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::analyze) enum SetOp {
    UnionDistinct,
    Intersect,
    Except,
}

pub(in crate::analyze) fn analyze_set_operation(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    set_op: SetOp,
) -> Result<DataContract> {
    let _config: setops::SetOperation = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    check_same_schema(op, left, right)?;
    // L'encoder delle chiavi di riga scatta prima dei dati: fail-closed.
    for field in left.schema.fields() {
        if !is_scalar_string(field.data_type()) {
            return contract_error(
                op,
                format!("tipo {:?} non supportato dalle set operation", field.data_type()),
            );
        }
    }
    if set_op == SetOp::UnionDistinct {
        let fields_out: Vec<Field> = left
            .schema
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| {
                field
                    .as_ref()
                    .clone()
                    .with_nullable(field.is_nullable() || right.schema.field(index).is_nullable())
            })
            .collect();
        // R2.4: due sorgenti — merge dei metadata di schema; stessa chiave
        // con valori diversi -> errore (come i join, mai precedenza implicita).
        let metadata = merge_schema_metadata(op, &left.schema, &right.schema)?;
        let schema = Schema::new_with_metadata(fields_out, metadata);
        let geometry = propagate_geometry(left, &schema, left.geometries.first().map(|g| g.name.as_str()));
        finish(schema, geometry, left.active_geometry, ContractProperties::default())
    } else {
        // intersect/except: output = left via select_rows, ordine canonico.
        let mut output = left.clone();
        output.properties = ContractProperties::default();
        Ok(output)
    }
}

