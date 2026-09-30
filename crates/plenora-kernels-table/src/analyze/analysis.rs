//! Analyzer a secco delle op di analisi (kernel `analysis.rs`).

use plenora_core::arrow::schema::DataType;
use plenora_core::contract::{ContractProperties, DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    analyze_append, check_json_text, check_name_list, check_output_name, check_text_len, con_op,
    contract_error, field_of, map_row_count, require_numeric, require_scalar_string, round_scaled,
    typed, unsupported,
};
use crate::{analysis, Limits};

// ---------------------------------------------------------------------------
// analysis.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_lookup(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: analysis::Lookup = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
    if config.mapping.len() > limits.max_rows {
        return contract_error(op, "mapping oltre max_rows");
    }
    // I valori del mapping e il default finiscono nelle celle: testi della
    // config entro `max_string_bytes`.
    for valore in config.mapping.values() {
        check_json_text(op, valore, limits.max_string_bytes, "mapping")?;
    }
    check_json_text(op, &config.default, limits.max_string_bytes, "default")?;
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
    limits: &Limits,
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
            // Sul valore esatto, come il kernel: due bordi interi distinti
            // oltre 2^53 non sono uguali.
            if edges.windows(2).any(|pair| {
                crate::compare_bounds(pair[0].esatto(), pair[1].esatto())
                    != Some(std::cmp::Ordering::Less)
            }) {
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
        for label in labels {
            check_text_len(op, label, limits.max_string_bytes, "labels")?;
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
    limits: &Limits,
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
    check_name_list(
        op,
        &config.output_columns,
        limits.max_columns,
        "output_columns",
        false,
    )?;
    // Lo stesso conto del kernel: colonne dell'input piu' colonne prodotte,
    // anche quando una prodotta sostituisce una esistente.
    if input
        .schema
        .fields()
        .len()
        .saturating_add(config.output_columns.len())
        > limits.max_columns
    {
        return Err(PlenoraError::ResourceLimit(format!(
            "{op}: flatten_json supera max_columns"
        )));
    }
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
    con_op(op, config.verifica_parametri())?;
    let input = &inputs[0];
    require_numeric(op, input, &config.column)?;
    // Il kernel legge `group_by` come scalare testuale: stessi tipi ammessi.
    if let Some(group_by) = &config.group_by {
        require_scalar_string(op, input, group_by)?;
    }
    // Nomi e tipi del kernel (`nomi_uscita`, `Stat::tipo_uscita`).
    let nomi = con_op(op, config.nomi_uscita())?;
    let tipo_ingresso = field_of(op, input, &config.column)?.data_type().clone();
    if config
        .stats
        .iter()
        .any(|stat| matches!(stat, analysis::Stat::Sum))
    {
        con_op(op, crate::float64_source::verifica_somma(&tipo_ingresso))?;
    }
    let produced: Vec<(String, DataType, bool)> = config
        .stats
        .iter()
        .zip(nomi)
        .map(|(stat, nome)| (nome, stat.tipo_uscita(&tipo_ingresso), true))
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
    con_op(op, config.verifica_parametri())?;
    if let Some(stratify) = &config.stratify_column {
        require_scalar_string(op, input, stratify)?;
    }
    let _ = fields;
    let mut output = input.clone();
    // Il kernel mescola le righe (shuffle): nessun ordinamento preservato.
    // Senza stratify il conteggio e' esatto: min(n, righe) o round(righe*f)
    // (stessa aritmetica f64 del kernel).
    let row_count = if config.stratify_column.is_none() {
        map_row_count(input, |rows| {
            config.fraction.map_or_else(
                || rows.min(u64::try_from(config.righe()).unwrap_or(u64::MAX)),
                |fraction| round_scaled(rows, fraction),
            )
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
