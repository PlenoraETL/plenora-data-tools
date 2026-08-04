//! Analyzer a secco di ordinamento, distinct e aggregazioni
//! (kernel `aggregation.rs`).

use std::collections::HashMap;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{ContractProperties, DataContract, FieldAllocator, FieldId};
use plenora_core::Result;
use serde_json::Value;

use super::helpers::{
    analyze_append, check_output_name, contract_error, field_of, finish, is_scalar_string,
    map_row_count, produce, propagate_geometry, proven_sorted, require_numeric,
    require_scalar_string, sorted_only, typed,
};
use crate::aggregation;

// ---------------------------------------------------------------------------
// aggregation.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_sort(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::Sort = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    for name in &config.columns {
        field_of(op, input, name)?;
    }
    let keys: Vec<FieldId> = config
        .columns
        .iter()
        .map(|name| fields.intern(name))
        .collect();
    let mut output = input.clone();
    // Sort blocking: l'intero stream di output e' ordinato sulle chiavi.
    output.properties = ContractProperties {
        sorted_by: Some(proven_sorted(keys)),
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}

pub(in crate::analyze) fn analyze_top_n(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::TopN = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    for name in &config.columns {
        field_of(op, input, name)?;
    }
    let keys: Vec<FieldId> = config
        .columns
        .iter()
        .map(|name| fields.intern(name))
        .collect();
    let mut output = input.clone();
    // Come sort, ma emesse esattamente min(n, righe) righe.
    output.properties = ContractProperties {
        sorted_by: Some(proven_sorted(keys)),
        row_count: map_row_count(input, |rows| rows.min(config.n)),
    };
    Ok(output)
}

pub(in crate::analyze) fn analyze_distinct(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::Distinct = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    for name in &config.subset {
        require_scalar_string(op, input, name)?;
    }
    let mut output = input.clone();
    // Righe rimosse; l'ordine relativo delle occorrenze mantenute e' preservato.
    output.properties = sorted_only(input);
    Ok(output)
}

pub(in crate::analyze) fn analyze_dedup_advanced(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::DedupAdvanced = typed(op, config)?;
    let input = &inputs[0];
    if matches!(config.keep, aggregation::Keep::False) {
        return contract_error(op, "keep=false non supportato");
    }
    for name in &config.subset {
        require_scalar_string(op, input, name)?;
    }
    let sorted_by = if let Some(order_column) = &config.order_column {
        field_of(op, input, order_column)?;
        // Sort interno ascendente su order_column prima della deduplica.
        Some(proven_sorted(vec![fields.intern(order_column)]))
    } else {
        input.properties.sorted_by.clone()
    };
    let mut output = input.clone();
    output.properties = ContractProperties {
        sorted_by,
        row_count: None,
    };
    Ok(output)
}

pub(in crate::analyze) fn analyze_aggregate(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::Aggregate = typed(op, config)?;
    let input = &inputs[0];
    if config.group_by.is_empty() {
        return contract_error(op, "aggregate richiede group_by");
    }
    let mut fields_out: Vec<Field> = config
        .group_by
        .iter()
        .map(|name| field_of(op, input, name).cloned())
        .collect::<Result<_>>()?;
    let mut duplicates: HashMap<&str, usize> = HashMap::new();
    for aggregation in &config.aggregations {
        *duplicates.entry(aggregation.column.as_str()).or_insert(0) += 1;
    }
    for aggregation in &config.aggregations {
        let field = field_of(op, input, &aggregation.column)?;
        match aggregation.function {
            aggregation::AggFunction::Count => {}
            aggregation::AggFunction::Nunique
            | aggregation::AggFunction::Concat
            | aggregation::AggFunction::First
            | aggregation::AggFunction::Last => {
                if !is_scalar_string(field.data_type()) {
                    return contract_error(
                        op,
                        format!("colonna {} non aggregabile come testo", aggregation.column),
                    );
                }
            }
            aggregation::AggFunction::Quantile => {
                if aggregation.quantile.is_none() {
                    return contract_error(op, "quantile richiede il parametro quantile");
                }
                // Come il kernel (`aggregate`): il range e' parte del
                // contratto; fuori [0, 1] l'indice nel gruppo ordinato
                // uscirebbe dai limiti — rifiuto a compile-plan.
                if aggregation
                    .quantile
                    .is_some_and(|quantile| !(0.0..=1.0).contains(&quantile))
                {
                    return contract_error(op, "quantile fuori dall'intervallo 0..=1");
                }
                require_numeric(op, input, &aggregation.column)?;
            }
            _ => require_numeric(op, input, &aggregation.column)?,
        }
        let function_name = match aggregation.function {
            aggregation::AggFunction::Count => "count",
            aggregation::AggFunction::Sum => "sum",
            aggregation::AggFunction::Avg | aggregation::AggFunction::Mean => "mean",
            aggregation::AggFunction::Min => "min",
            aggregation::AggFunction::Max => "max",
            aggregation::AggFunction::First => "first",
            aggregation::AggFunction::Last => "last",
            aggregation::AggFunction::Concat => "concat",
            aggregation::AggFunction::Nunique => "nunique",
            aggregation::AggFunction::Variance => "variance",
            aggregation::AggFunction::Stddev => "stddev",
            aggregation::AggFunction::Quantile => "quantile",
        };
        let name = if !aggregation.alias.is_empty() {
            aggregation.alias.clone()
        } else if duplicates[aggregation.column.as_str()] > 1 {
            format!("{}_{function_name}", aggregation.column)
        } else {
            aggregation.column.clone()
        };
        check_output_name(op, &name)?;
        let (data_type, nullable) = match aggregation.function {
            aggregation::AggFunction::Count | aggregation::AggFunction::Nunique => {
                (DataType::Int64, false)
            }
            aggregation::AggFunction::Concat
            | aggregation::AggFunction::First
            | aggregation::AggFunction::Last => (DataType::Utf8, true),
            _ => (DataType::Float64, true),
        };
        produce(&mut fields_out, fields, &name, data_type, nullable);
    }
    if config.aggregations.is_empty() {
        produce(&mut fields_out, fields, "count", DataType::Int64, false);
    }
    // R2.4: i metadata dello schema di input si conservano sempre (le chiavi
    // sconosciute non sono giudicabili dal centro; perderle rompe i
    // round-trip). Le colonne aggregate restano derivate: nessun metadata di
    // campo ereditato.
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let preserved = input
        .geometries
        .first()
        .filter(|geometry| config.group_by.contains(&geometry.name))
        .map(|geometry| geometry.name.as_str());
    let geometry = propagate_geometry(input, &schema, preserved);
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    finish(schema, geometry, active, ContractProperties::default())
}

pub(in crate::analyze) fn analyze_rolling_window(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::RollingWindow = typed(op, config)?;
    let input = &inputs[0];
    if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
        return contract_error(op, "window/min_periods non validi");
    }
    require_numeric(op, input, &config.column)?;
    if let Some(group_by) = &config.group_by {
        field_of(op, input, group_by)?;
    }
    let sorted_by = if let Some(order_column) = &config.order_column {
        field_of(op, input, order_column)?;
        Some(proven_sorted(vec![fields.intern(order_column)]))
    } else {
        input.properties.sorted_by.clone()
    };
    let mut output = analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Float64, true)],
    )?;
    output.properties = ContractProperties {
        sorted_by,
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}

pub(in crate::analyze) fn analyze_window_function(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::WindowFunction = typed(op, config)?;
    let input = &inputs[0];
    if config.offset == 0 {
        return contract_error(op, "offset deve essere > 0");
    }
    match (&config.function, config.buckets) {
        (aggregation::WindowKind::Ntile, Some(buckets)) if buckets > 0 => {}
        (aggregation::WindowKind::Ntile, _) => {
            return contract_error(op, "ntile richiede buckets > 0");
        }
        (_, Some(_)) => {
            return contract_error(op, "buckets ammesso solo con ntile");
        }
        _ => {}
    }
    require_numeric(op, input, &config.column)?;
    if let Some(group_by) = &config.group_by {
        field_of(op, input, group_by)?;
    }
    if let Some(order_column) = &config.order_column {
        field_of(op, input, order_column)?;
    }
    let suffix = match config.function {
        aggregation::WindowKind::Rank => "rank",
        aggregation::WindowKind::DenseRank => "dense_rank",
        aggregation::WindowKind::Cumsum => "cumsum",
        aggregation::WindowKind::Cumcount => "cumcount",
        aggregation::WindowKind::Lag => "lag",
        aggregation::WindowKind::Lead => "lead",
        aggregation::WindowKind::PctChange => "pct_change",
        aggregation::WindowKind::RunningMean => "running_mean",
        aggregation::WindowKind::PercentRank => "percent_rank",
        aggregation::WindowKind::CumeDist => "cume_dist",
        aggregation::WindowKind::Ntile => "ntile",
    };
    let name = config
        .output_column
        .unwrap_or_else(|| format!("{}_{suffix}", config.column));
    let sorted_by = config.order_column.as_ref().map_or_else(
        || input.properties.sorted_by.clone(),
        |order_column| Some(proven_sorted(vec![fields.intern(order_column)])),
    );
    let mut output = analyze_append(input, fields, &[(name, DataType::Float64, true)])?;
    output.properties = ContractProperties {
        sorted_by,
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}
