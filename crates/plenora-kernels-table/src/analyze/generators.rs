//! Analyzer a secco delle op di generazione (`add_row_number`,
//! `uuid_generator`, `date_extract`, `limit`; kernel `utility.rs`).

use plenora_core::arrow::schema::DataType;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::Result;
use serde_json::Value;

use super::helpers::{
    analyze_append, check_output_name, contract_error, require_scalar_string, sorted_only, typed,
};
use crate::utility;

// ---------------------------------------------------------------------------
// utility.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_add_row_number(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: utility::AddRowNumber = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.order_column.is_some() {
        return contract_error(
            op,
            "order_column non supportato dal profilo streaming (deve essere nullo)",
        );
    }
    if let Some(partition) = &config.partition_column {
        require_scalar_string(op, input, partition)?;
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Int64, false)],
    )
}

pub(in crate::analyze) fn analyze_uuid_generator(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: utility::UuidGenerator = typed(op, config)?;
    check_output_name(op, &config.output_column)?;
    analyze_append(
        &inputs[0],
        fields,
        &[(config.output_column, DataType::Utf8, false)],
    )
}

pub(in crate::analyze) fn analyze_date_extract(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: utility::DateExtract = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
    let prefix = if config.prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.prefix.clone()
    };
    let mut produced = Vec::with_capacity(config.parts.len());
    for part in &config.parts {
        let suffix = match part {
            utility::DatePart::Year => "year",
            utility::DatePart::Month => "month",
            utility::DatePart::Day => "day",
            utility::DatePart::Quarter => "quarter",
            utility::DatePart::Weekday => "weekday",
            utility::DatePart::Week => "week",
            utility::DatePart::Hour => "hour",
            utility::DatePart::Minute => "minute",
            utility::DatePart::Second => "second",
        };
        let name = format!("{prefix}{suffix}");
        check_output_name(op, &name)?;
        produced.push((name, DataType::Int64, true));
    }
    analyze_append(input, fields, &produced)
}

pub(in crate::analyze) fn analyze_limit(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: utility::Limit = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let _ = config;
    let mut output = input.clone();
    // Righe rimosse (per-batch), ordine relativo e schema invariati.
    output.properties = sorted_only(input);
    Ok(output)
}
