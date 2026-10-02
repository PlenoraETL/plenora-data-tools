//! Analyzer a secco delle op di cleansing (kernel `cleansing.rs`).

use plenora_core::arrow::schema::{DataType, Schema, TimeUnit};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    analyze_append, check_json_text, check_text_len, clone_fields, con_op, contract_error,
    field_of, finish, produce, propagate_geometry, require_scalar_string, require_utf8, rows_only,
    typed,
};
use crate::{cleansing, Limits};

// ---------------------------------------------------------------------------
// cleansing.rs
// ---------------------------------------------------------------------------

/// Coerenza valore-tipo di `fill_na` (il kernel calcola il valore fisso per
/// ogni metodo, quindi l'errore e' deterministico a secco).
fn check_fill_value(op: &str, data_type: &DataType, value: &Value) -> Result<()> {
    let valid = match data_type {
        DataType::Int64 => match value {
            Value::Null => true,
            Value::Number(number) => number.as_i64().is_some(),
            Value::String(text) => text.parse::<i64>().is_ok(),
            _ => false,
        },
        DataType::Float64 => match value {
            Value::Null => true,
            Value::Number(number) => number.as_f64().is_some(),
            Value::String(text) => text.replace(',', ".").parse::<f64>().is_ok(),
            _ => false,
        },
        DataType::Boolean => match value {
            Value::Null | Value::Bool(_) => true,
            Value::String(text) => {
                text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false")
            }
            _ => false,
        },
        DataType::Utf8 => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        contract_error(
            op,
            format!(
                "valore di fill non valido per il tipo {}",
                plenora_core::tipo_arrow::descrivi_tipo(data_type)
            ),
        )
    }
}

pub(in crate::analyze) fn analyze_fill_na(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: cleansing::FillNa = typed(op, config)?;
    con_op(op, config.verifica_parametri())?;
    // Il valore di riempimento finisce nelle celle.
    check_json_text(op, config.valore(), limits.max_string_bytes, "value")?;
    let input = &inputs[0];
    let _ = fields;
    let targets: Vec<usize> = if let Some(name) = &config.column {
        vec![input
            .schema
            .index_of(name)
            .map_err(|_| PlenoraError::InvalidPlan(format!("{op}: colonna non trovata: {name}")))?]
    } else {
        (0..input.schema.fields().len()).collect()
    };
    let mut fields_out = clone_fields(input);
    for index in targets {
        let data_type = fields_out[index].data_type().clone();
        if !matches!(
            data_type,
            DataType::Utf8 | DataType::Int64 | DataType::Float64 | DataType::Boolean
        ) {
            return contract_error(
                op,
                format!(
                    "fill_na non supporta il tipo {} della colonna {}",
                    plenora_core::tipo_arrow::descrivi_tipo(&data_type),
                    fields_out[index].name()
                ),
            );
        }
        check_fill_value(op, &data_type, config.valore())?;
        // Il tipo non muta, quindi i metadati del campo sorgente restano
        // validi e si conservano (clone), come nel kernel; la colonna diventa
        // nullable (anche il kernel la dichiara cosi', e con ffill/bfill
        // possono restare null).
        fields_out[index] = fields_out[index].clone().with_nullable(true);
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    // La colonna geometrica (Binary) non e' mai un target valido: preservata.
    let geometry = propagate_geometry(
        input,
        &schema,
        input.geometries.first().map(|g| g.name.as_str()),
    );
    // Valori modificati, righe e ordine invariati.
    finish(schema, geometry, input.active_geometry, rows_only(input))
}

pub(in crate::analyze) fn analyze_replace(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: cleansing::Replace = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    // Con `regex` `old_value` e' un pattern; senza, il testo di una cella.
    let limite_old = if config.regex {
        limits.max_regex_bytes
    } else {
        limits.max_string_bytes
    };
    check_text_len(op, &config.old_value, limite_old, "old_value")?;
    check_text_len(op, &config.new_value, limits.max_string_bytes, "new_value")?;
    if config.regex {
        regex::Regex::new(&config.old_value).map_err(|error| {
            PlenoraError::InvalidPlan(format!("{op}: regex non valida: {error}"))
        })?;
    }
    // Tipo invariato (Utf8 -> Utf8): i metadati del campo sorgente restano
    // validi, come nel kernel; `produce` ricostruisce il campo e li azzera,
    // quindi vanno ripristinati dal sorgente.
    let source_metadata = field_of(op, input, &config.column)?.metadata().clone();
    let mut fields_out = clone_fields(input);
    produce(
        &mut fields_out,
        fields,
        &config.column,
        DataType::Utf8,
        true,
    )?;
    if let Some(replaced) = fields_out
        .iter_mut()
        .find(|field| field.name() == &config.column)
    {
        *replaced = replaced.clone().with_metadata(source_metadata);
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let geometry = propagate_geometry(
        input,
        &schema,
        input.geometries.first().map(|g| g.name.as_str()),
    );
    finish(schema, geometry, input.active_geometry, rows_only(input))
}

/// Parametri di `type_cast` che solo alcuni target usano: su un altro target
/// il kernel li ignorerebbe, e la config direbbe una cosa che non accade.
///
/// - `date_format` solo per i target data e timestamp;
/// - `precision` e `scale` solo per `decimal128`, dove sono obbligatori con
///   `1 <= precision <= 38` e `0 <= scale <= precision`;
/// - `timezone` solo per `timestamp_millis`.
fn check_type_cast_parameters(op: &str, config: &cleansing::TypeCast) -> Result<()> {
    con_op(op, config.verifica_parametri())
}

pub(in crate::analyze) fn analyze_type_cast(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: cleansing::TypeCast = typed(op, config)?;
    let input = &inputs[0];
    // Una colonna temporale (Date32, Timestamp di ogni unita') si converte
    // dal valore nativo, con i target che il kernel ammette
    // (`verifica_cast_temporale`); ogni altra colonna si legge come testo.
    let campo = field_of(op, input, &config.column)?;
    if crate::temporale::tipo_temporale(campo.data_type()) {
        con_op(
            op,
            crate::temporale::verifica_tipo_temporale(campo.data_type(), &config.column)
                .map_err(|errore| PlenoraError::InvalidPlan(errore.to_string())),
        )?;
        con_op(op, cleansing::verifica_cast_temporale(&config))?;
    } else {
        require_scalar_string(op, input, &config.column)?;
    }
    check_type_cast_parameters(op, &config)?;
    // Vuoto e' il parser multi-formato di default.
    if !config.date_format.is_empty() {
        check_text_len(
            op,
            &config.date_format,
            limits.max_string_bytes,
            "date_format",
        )?;
        con_op(
            op,
            crate::dates::validate_format_items(&config.date_format, "date_format"),
        )?;
    }
    let target = match config.target_type {
        cleansing::TargetType::Str
        | cleansing::TargetType::Date
        | cleansing::TargetType::Datetime => DataType::Utf8,
        cleansing::TargetType::Int => DataType::Int64,
        cleansing::TargetType::Float => DataType::Float64,
        cleansing::TargetType::Bool => DataType::Boolean,
        cleansing::TargetType::Date32 => DataType::Date32,
        cleansing::TargetType::TimestampMillis => {
            if let Some(timezone) = &config.timezone {
                timezone.parse::<chrono_tz::Tz>().map_err(|_| {
                    PlenoraError::InvalidPlan(format!("{op}: timezone non valida: {timezone}"))
                })?;
            }
            DataType::Timestamp(
                TimeUnit::Millisecond,
                config.timezone.as_deref().map(Into::into),
            )
        }
        cleansing::TargetType::Decimal128 => {
            // Presenza e intervalli verificati da `check_type_cast_parameters`.
            let (Some(precision), Some(scale)) = (config.precision, config.scale) else {
                return Err(PlenoraError::Internal(format!(
                    "{op}: decimal128 senza precision o scale dopo la verifica"
                )));
            };
            DataType::Decimal128(precision, scale)
        }
        cleansing::TargetType::BinaryUtf8 => DataType::Binary,
        cleansing::TargetType::Uint64 => DataType::UInt64,
        cleansing::TargetType::DictionaryUtf8 => {
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        }
    };
    // Sostituzione in place: i metadati di campo (geoarrow.wkb inclusi) vanno
    // persi -> se il target e' la colonna geometrica il contratto diventa
    // tabellare (analyze_append lo gestisce). errors=Ignore puo' fallire in
    // esecuzione su dati non convertibili: dipende dalle celle, non e' un
    // errore di piano prevedibile (scheda di `table.type_cast`).
    let mut output = analyze_append(input, fields, &[(config.column, target, true)])?;
    output.properties = rows_only(input);
    Ok(output)
}
