//! Analyzer a secco delle op di generazione (`add_row_number`,
//! `uuid_generator`, `date_extract`, `limit`; kernel `utility.rs`).

use plenora_core::arrow::schema::DataType;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::dates::check_format_text;
use super::helpers::{
    analyze_append, check_output_name, check_rows, con_op, contract_error, field_of,
    require_scalar_string, sorted_only, typed,
};
use crate::{utility, Limits};

// ---------------------------------------------------------------------------
// utility.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_add_row_number(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: utility::AddRowNumber = typed(op, config)?;
    let input = &inputs[0];
    let _ = limits;
    check_output_name(op, &config.output_column)?;
    if config.order_column.is_some() {
        return contract_error(
            op,
            "order_column non supportato dal profilo streaming (deve essere nullo)",
        );
    }
    con_op(op, utility::verifica_ascending(&config))?;
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
    limits: &Limits,
) -> Result<DataContract> {
    let config: utility::DateExtract = typed(op, config)?;
    con_op(
        op,
        crate::dates::verifica_politiche(config.invalid.as_ref(), None),
    )?;
    con_op(op, config.verifica_parti())?;
    let input = &inputs[0];
    // Una colonna temporale si legge dal valore nativo, senza `date_format`;
    // ogni altra come testo. La regola del kernel.
    let campo = field_of(op, input, &config.column)?;
    if crate::temporale::tipo_temporale(campo.data_type()) {
        con_op(
            op,
            crate::temporale::verifica_tipo_temporale(campo.data_type(), &config.column)
                .map_err(|errore| PlenoraError::InvalidPlan(errore.to_string())),
        )?;
        con_op(op, utility::verifica_date_extract_temporale(&config))?;
    } else {
        require_scalar_string(op, input, &config.column)?;
    }
    if let Some(format) = &config.date_format {
        check_format_text(op, format, limits, "date_format")?;
        con_op(
            op,
            crate::dates::validate_format_items(format, "date_format"),
        )?;
    }
    // Nomi con la regola del kernel: validi e distinti.
    let produced = con_op(op, utility::nomi_date_extract(&config))?
        .into_iter()
        .map(|name| (name, DataType::Int64, true))
        .collect::<Vec<_>>();
    analyze_append(input, fields, &produced)
}

pub(in crate::analyze) fn analyze_limit(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: utility::Limit = typed(op, config)?;
    con_op(op, config.verifica_parametri())?;
    let input = &inputs[0];
    let _ = fields;
    check_rows(op, config.n, limits.max_rows, "n")?;
    check_rows(op, config.offset, limits.max_rows, "offset")?;
    let mut output = input.clone();
    // Righe rimosse, ordine relativo e schema invariati; il conteggio delle
    // righe d'uscita non si dichiara.
    output.properties = sorted_only(input);
    Ok(output)
}
