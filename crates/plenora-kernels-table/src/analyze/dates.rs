//! Analyzer a secco delle op su date e timezone (kernel `dates.rs`).

use plenora_core::arrow::schema::DataType;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    analyze_append, check_output_name, con_op, contract_error, field_of, require_scalar_string,
    typed,
};
use crate::{dates, Limits};

// ---------------------------------------------------------------------------
// dates.rs
// ---------------------------------------------------------------------------

/// Un formato strftime della config: non vuoto (un formato vuoto non legge e
/// non scrive niente) ed entro `max_string_bytes`. Gli item non
/// riconosciuti li rifiuta `dates::validate_format_items`.
pub(in crate::analyze) fn check_format_text(
    op: &str,
    format: &str,
    limits: &Limits,
    label: &str,
) -> Result<()> {
    if format.is_empty() || format.len() > limits.max_string_bytes {
        return contract_error(op, format!("{label} non valido"));
    }
    Ok(())
}

/// Un formato di scrittura: come [`check_format_text`], e il testo che
/// scrive per ogni riga ([`dates::byte_massimi_scritti`], larghezza massima
/// di ogni campo) entro
/// `max_string_bytes`. Il valore formattato non cresce con la cella, quindi
/// il controllo sulla config vale per ogni riga.
pub(in crate::analyze) fn check_output_format(
    op: &str,
    format: &str,
    limits: &Limits,
) -> Result<()> {
    check_format_text(op, format, limits, "output_format")?;
    if dates::byte_massimi_scritti(format) > limits.max_string_bytes {
        return contract_error(op, "output_format: testo scritto oltre max_string_bytes");
    }
    Ok(())
}

/// La colonna d'ingresso di un'operazione su date e il suo `input_format`,
/// con la regola del kernel (`dates::verifica_input_format`): una colonna
/// temporale (`Date32`, `Timestamp` di ogni unita') si legge dal valore
/// nativo, senza formato; ogni altra come testo, con un formato valido.
fn check_input(
    op: &str,
    input: &DataContract,
    column: &str,
    input_format: Option<&str>,
    limits: &Limits,
) -> Result<()> {
    let campo = field_of(op, input, column)?;
    let temporale = crate::temporale::tipo_temporale(campo.data_type());
    if temporale {
        con_op(
            op,
            crate::temporale::verifica_tipo_temporale(campo.data_type(), column)
                .map_err(|errore| PlenoraError::InvalidPlan(errore.to_string())),
        )?;
    } else {
        require_scalar_string(op, input, column)?;
    }
    if let Some(formato) = input_format {
        check_format_text(op, formato, limits, "input_format")?;
    }
    con_op(op, dates::verifica_input_format(temporale, input_format))
}

pub(in crate::analyze) fn analyze_date_op(
    op: &str,
    input: &DataContract,
    fields: &mut FieldAllocator,
    output_column: &str,
    data_type: DataType,
) -> Result<DataContract> {
    check_output_name(op, output_column)?;
    analyze_append(
        input,
        fields,
        &[(output_column.to_owned(), data_type, true)],
    )
}

pub(in crate::analyze) fn analyze_date_format(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: dates::DateFormat = typed(op, config)?;
    con_op(op, dates::verifica_politiche(config.invalid.as_ref(), None))?;
    check_input(
        op,
        &inputs[0],
        &config.column,
        config.input_format.as_deref(),
        limits,
    )?;
    check_output_format(op, &config.output_format, limits)?;
    con_op(op, dates::FormatoUscita::senza_fuso(&config.output_format))?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &config.output_column,
        DataType::Utf8,
    )
}

pub(in crate::analyze) fn analyze_date_add(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: dates::DateAdd = typed(op, config)?;
    con_op(op, dates::verifica_politiche(config.invalid.as_ref(), None))?;
    check_input(
        op,
        &inputs[0],
        &config.column,
        config.input_format.as_deref(),
        limits,
    )?;
    check_output_format(op, &config.output_format, limits)?;
    con_op(op, dates::verifica_amount(config.amount, &config.unit))?;
    con_op(op, dates::FormatoUscita::senza_fuso(&config.output_format))?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &config.output_column,
        DataType::Utf8,
    )
}

pub(in crate::analyze) fn analyze_date_diff(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: dates::DateDiff = typed(op, config)?;
    con_op(op, dates::verifica_politiche(config.invalid.as_ref(), None))?;
    for column in [&config.start_column, &config.end_column] {
        check_input(
            op,
            &inputs[0],
            column,
            config.input_format.as_deref(),
            limits,
        )?;
    }
    con_op(
        op,
        dates::verifica_generi_date_diff(
            field_of(op, &inputs[0], &config.start_column)?.data_type(),
            field_of(op, &inputs[0], &config.end_column)?.data_type(),
        ),
    )?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &config.output_column,
        DataType::Float64,
    )
}

pub(in crate::analyze) fn analyze_timezone_convert(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: dates::TimezoneConvert = typed(op, config)?;
    con_op(
        op,
        dates::verifica_politiche(config.invalid.as_ref(), config.ambiguous.as_ref()),
    )?;
    check_output_format(op, &config.output_format, limits)?;
    let mut fusi = Vec::with_capacity(2);
    for timezone in [&config.source_timezone, &config.target_timezone] {
        fusi.push(timezone.parse::<chrono_tz::Tz>().map_err(|_| {
            PlenoraError::InvalidPlan(format!("{op}: timezone non valida: {timezone}"))
        })?);
    }
    check_input(
        op,
        &inputs[0],
        &config.column,
        config.input_format.as_deref(),
        limits,
    )?;
    if let [sorgente, destinazione] = fusi.as_slice() {
        con_op(
            op,
            dates::verifica_fuso_sorgente(
                field_of(op, &inputs[0], &config.column)?.data_type(),
                *sorgente,
            )
            .map_err(|errore| match errore {
                PlenoraError::Schema(messaggio) => PlenoraError::InvalidPlan(messaggio),
                altro => altro,
            }),
        )?;
        con_op(
            op,
            dates::FormatoUscita::con_fuso(&config.output_format, *destinazione),
        )?;
    }
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &config.output_column,
        DataType::Utf8,
    )
}
