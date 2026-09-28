//! Analyzer a secco delle op di cleansing (kernel `cleansing.rs`).

use plenora_core::arrow::schema::{DataType, Schema, TimeUnit};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    analyze_append, clone_fields, contract_error, field_of, finish, produce, propagate_geometry,
    require_scalar_string, require_utf8, rows_only, typed,
};
use crate::cleansing;

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
            format!("valore di fill non valido per il tipo {data_type:?}"),
        )
    }
}

pub(in crate::analyze) fn analyze_fill_na(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: cleansing::FillNa = typed(op, config)?;
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
                    "fill_na non supporta il tipo {data_type:?} della colonna {}",
                    fields_out[index].name()
                ),
            );
        }
        check_fill_value(op, &data_type, &config.value)?;
        // R2.4 type-preserving: il tipo non muta, quindi i metadata del campo
        // sorgente restano validi e si conservano (clone); cambia solo la
        // nullability (i valori riempiti possono non essere piu' null).
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
) -> Result<DataContract> {
    let config: cleansing::Replace = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    if config.regex {
        regex::Regex::new(&config.old_value).map_err(|error| {
            PlenoraError::InvalidPlan(format!("{op}: regex non valida: {error}"))
        })?;
    }
    // R2.4 type-preserving: Utf8 -> Utf8 (tipo invariato), i metadata del
    // campo sorgente restano validi; `produce` ricostruisce il campo e li
    // azzera, quindi vanno ripristinati dal sorgente.
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

pub(in crate::analyze) fn analyze_type_cast(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: cleansing::TypeCast = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
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
            let precision = config.precision.ok_or_else(|| {
                PlenoraError::InvalidPlan(format!("{op}: decimal128 richiede precision"))
            })?;
            let scale = config.scale.ok_or_else(|| {
                PlenoraError::InvalidPlan(format!("{op}: decimal128 richiede scale"))
            })?;
            if precision == 0 || precision > 38 {
                return contract_error(op, "precision decimal128 fuori da 1..=38");
            }
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
    // tabellare (analyze_append lo gestisce). errors=Ignore puo' fallire a
    // runtime su dati non omogenei: rischio documentato, non errore statico.
    let mut output = analyze_append(input, fields, &[(config.column, target, true)])?;
    output.properties = rows_only(input);
    Ok(output)
}
