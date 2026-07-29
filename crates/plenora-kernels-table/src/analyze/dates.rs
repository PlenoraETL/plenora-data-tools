//! Analyzer a secco delle op su date e timezone (kernel `dates.rs`).

use plenora_core::arrow::schema::DataType;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{analyze_append, check_output_name, require_scalar_string, typed};
use crate::dates;

// ---------------------------------------------------------------------------
// dates.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_date_op(
    op: &str,
    input: &DataContract,
    fields: &mut FieldAllocator,
    source_columns: &[&str],
    output_column: &str,
    data_type: DataType,
) -> Result<DataContract> {
    for name in source_columns {
        require_scalar_string(op, input, name)?;
    }
    check_output_name(op, output_column)?;
    analyze_append(input, fields, &[(output_column.to_owned(), data_type, true)])
}

pub(in crate::analyze) fn analyze_date_format(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: dates::DateFormat = typed(op, config)?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &[&config.column],
        &config.output_column,
        DataType::Utf8,
    )
}

pub(in crate::analyze) fn analyze_date_add(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: dates::DateAdd = typed(op, config)?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &[&config.column],
        &config.output_column,
        DataType::Utf8,
    )
}

pub(in crate::analyze) fn analyze_date_diff(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: dates::DateDiff = typed(op, config)?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &[&config.start_column, &config.end_column],
        &config.output_column,
        DataType::Float64,
    )
}

pub(in crate::analyze) fn analyze_timezone_convert(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: dates::TimezoneConvert = typed(op, config)?;
    for timezone in [&config.source_timezone, &config.target_timezone] {
        timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| PlenoraError::InvalidPlan(format!("{op}: timezone non valida: {timezone}")))?;
    }
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &[&config.column],
        &config.output_column,
        DataType::Utf8,
    )
}

