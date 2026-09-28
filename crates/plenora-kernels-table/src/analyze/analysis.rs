//! Analyzer a secco delle op di analisi (kernel `analysis.rs`).

use plenora_core::arrow::schema::DataType;
use plenora_core::contract::{ContractProperties, DataContract, FieldAllocator};
use plenora_core::Result;
use serde_json::Value;

use super::helpers::{
    analyze_append, check_output_name, contract_error, field_of, map_row_count, require_numeric,
    require_scalar_string, round_scaled, typed, unsupported,
};
use crate::analysis;

// ---------------------------------------------------------------------------
// analysis.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_lookup(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::Lookup = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
    // Default del kernel: sovrascrive la colonna sorgente in place.
    let name = config.output_column.unwrap_or(config.column);
    check_output_name(op, &name)?;
    analyze_append(input, fields, &[(name, DataType::Utf8, true)])
}

pub(in crate::analyze) fn analyze_bin(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::Bin = typed(op, config)?;
    let input = &inputs[0];
    require_numeric(op, input, &config.column)?;
    let bins = match &config.bins {
        analysis::Bins::Count(count) => {
            if !(2..=100).contains(count) {
                return contract_error(op, "bins count fuori da 2..=100");
            }
            *count
        }
        analysis::Bins::Edges(edges) => {
            if !(3..=101).contains(&edges.len()) {
                return contract_error(op, "edges fuori da 3..=101");
            }
            if edges.windows(2).any(|pair| pair[0] >= pair[1]) {
                return contract_error(op, "edges non strettamente crescenti");
            }
            edges.len() - 1
        }
    };
    if let Some(labels) = &config.labels {
        if labels.len() != bins {
            return contract_error(
                op,
                format!("labels ({}) diversi dai bin ({bins})", labels.len()),
            );
        }
    }
    let name = config
        .output_column
        .unwrap_or_else(|| format!("{}_bin", config.column));
    analyze_append(input, fields, &[(name, DataType::Utf8, true)])
}

pub(in crate::analyze) fn analyze_flatten_json(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::FlattenJson = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
    if config.max_level > 5 {
        return contract_error(op, "max_level oltre 5");
    }
    if config.output_columns.is_empty() {
        return unsupported(
            op,
            "senza output_columns i nomi delle colonne derivano dai dati: schema non inferibile a secco",
        );
    }
    let prefix = if config.prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.prefix.clone()
    };
    let mut produced = Vec::with_capacity(config.output_columns.len());
    for name in &config.output_columns {
        if !name.starts_with(&prefix) {
            return contract_error(
                op,
                format!("output column {name:?} non inizia con il prefix {prefix:?}"),
            );
        }
        check_output_name(op, name)?;
        produced.push((name.clone(), DataType::Utf8, true));
    }
    analyze_append(input, fields, &produced)
}

pub(in crate::analyze) fn analyze_statistics(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::Statistics = typed(op, config)?;
    let input = &inputs[0];
    require_numeric(op, input, &config.column)?;
    if let Some(group_by) = &config.group_by {
        field_of(op, input, group_by)?;
    }
    let prefix = if config.output_prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.output_prefix.clone()
    };
    let produced: Vec<(String, DataType, bool)> = config
        .stats
        .iter()
        .map(|stat| {
            let suffix = match stat {
                analysis::Stat::Count => "count",
                analysis::Stat::Min => "min",
                analysis::Stat::Max => "max",
                analysis::Stat::Sum => "sum",
                analysis::Stat::Mean => "mean",
                analysis::Stat::Median => "median",
                analysis::Stat::Std => "std",
                analysis::Stat::Var => "var",
                analysis::Stat::Q25 => "q25",
                analysis::Stat::Q75 => "q75",
            };
            (format!("{prefix}{suffix}"), DataType::Float64, true)
        })
        .collect();
    // Statistiche broadcast per riga: righe invariate.
    analyze_append(input, fields, &produced)
}

pub(in crate::analyze) fn analyze_sample(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::Sample = typed(op, config)?;
    let input = &inputs[0];
    if config
        .fraction
        .is_some_and(|fraction| !(0.0..=1.0).contains(&fraction))
    {
        return contract_error(op, "fraction fuori 0..=1");
    }
    if let Some(stratify) = &config.stratify_column {
        require_scalar_string(op, input, stratify)?;
    }
    let _ = fields;
    let mut output = input.clone();
    // Il kernel mescola le righe (shuffle): nessun ordinamento preservato.
    // Senza stratify il conteggio e' esatto: min(n, righe) o round(righe*f)
    // (stessa aritmetica f64 del kernel).
    let row_count = if config.stratify_column.is_none() {
        map_row_count(input, |rows| match config.fraction {
            None => rows.min(u64::try_from(config.n).unwrap_or(u64::MAX)),
            Some(fraction) => round_scaled(rows, fraction),
        })
    } else {
        None
    };
    output.properties = ContractProperties {
        sorted_by: None,
        row_count,
    };
    Ok(output)
}
