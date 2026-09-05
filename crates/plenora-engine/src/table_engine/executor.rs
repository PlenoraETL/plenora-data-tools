//! Executor tabellare: validazione delle config per singolo passo ed
//! esecuzione della catena.

use std::collections::HashSet;
use std::path::Path;

use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, LargeStringArray, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Schema, SchemaRef};
use plenora_core::error::ReplayedError;
use plenora_core::{PlenoraError, Result};
use serde::de::DeserializeOwned;

use plenora_kernels_table::validate_output_name;
use plenora_kernels_table::{
    aggregation, analysis, cleansing, columns, dates, expressions, filtering, formula, fuzzy,
    governance, joins, quality, reshape, security, setops, spill, strings, utility,
};

use super::contract::{dispatch_name, Step, ValidatedPlan};
use super::Limits;

#[cfg(test)]
thread_local! {
    /// Contatore delle deserializzazioni di config (i test su configurazioni
    /// preparate e hot path minimale): thread-local
    /// per non interferire con i test paralleli; il percorso per batch non
    /// deve mai incrementarlo.
    static DECODE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn decode<T: DeserializeOwned>(step: &Step) -> Result<T> {
    #[cfg(test)]
    DECODE_CALLS.with(|calls| calls.set(calls.get() + 1));
    serde_json::from_value(step.config.clone()).map_err(PlenoraError::from)
}

fn validate_name_list(names: &[String], max: usize, label: &str, allow_empty: bool) -> Result<()> {
    if (!allow_empty && names.is_empty()) || names.len() > max {
        return Err(PlenoraError::InvalidPlan(format!(
            "numero colonne {label} non valido"
        )));
    }
    let unique: HashSet<_> = names.iter().collect();
    if unique.len() != names.len() {
        return Err(PlenoraError::InvalidPlan(format!(
            "{label} contiene nomi duplicati"
        )));
    }
    names.iter().try_for_each(|name| validate_output_name(name))
}

fn validate_rename(config: &columns::Rename, limits: &Limits) -> Result<()> {
    if config.renames.len() > limits.max_columns {
        return Err(PlenoraError::InvalidPlan("troppe rinomine".into()));
    }
    let old: Vec<_> = config
        .renames
        .iter()
        .map(|pair| pair.old_name.clone())
        .collect();
    let new: Vec<_> = config
        .renames
        .iter()
        .map(|pair| pair.new_name.clone())
        .collect();
    validate_name_list(&old, limits.max_columns, "rename origine", true)?;
    validate_name_list(&new, limits.max_columns, "rename destinazione", true)
}

fn validate_split(config: &columns::SplitColumn, limits: &Limits) -> Result<()> {
    validate_output_name(&config.column)?;
    validate_name_list(
        &config.new_columns,
        limits.max_split_columns,
        "split_column",
        false,
    )?;
    if config.delimiter.is_empty() || config.delimiter.len() > limits.max_string_bytes {
        return Err(PlenoraError::InvalidPlan("delimiter non valido".into()));
    }
    Ok(())
}

fn validate_pad(config: &strings::StringPad, limits: &Limits) -> Result<()> {
    validate_output_name(&config.column)?;
    if let Some(output) = &config.output_column {
        validate_output_name(output)?;
    }
    let mut characters = config.fill_char.chars();
    if characters.next().is_none() || characters.next().is_some() {
        return Err(PlenoraError::InvalidPlan(
            "fill_char deve essere un carattere Unicode".into(),
        ));
    }
    if config.width > limits.max_string_bytes {
        return Err(PlenoraError::InvalidPlan("width oltre il limite".into()));
    }
    Ok(())
}

fn validate_type_cast(config: &cleansing::TypeCast) -> Result<()> {
    validate_output_name(&config.column)?;
    match config.target_type {
        cleansing::TargetType::Decimal128 => {
            let precision = config
                .precision
                .ok_or_else(|| PlenoraError::InvalidPlan("decimal128 richiede precision".into()))?;
            let scale = config
                .scale
                .ok_or_else(|| PlenoraError::InvalidPlan("decimal128 richiede scale".into()))?;
            if !(1..=38).contains(&precision) || scale < 0 || scale > precision.cast_signed() {
                return Err(PlenoraError::InvalidPlan(
                    "decimal128 richiede 1 <= precision <= 38 e 0 <= scale <= precision".into(),
                ));
            }
            if config.timezone.is_some() {
                return Err(PlenoraError::InvalidPlan(
                    "timezone non ammessa per decimal128".into(),
                ));
            }
        }
        cleansing::TargetType::TimestampMillis => {
            if config.precision.is_some() || config.scale.is_some() {
                return Err(PlenoraError::InvalidPlan(
                    "precision/scale non ammessi per timestamp".into(),
                ));
            }
            if let Some(timezone) = &config.timezone {
                timezone
                    .parse::<chrono_tz::Tz>()
                    .map_err(|_| PlenoraError::InvalidPlan("timezone cast non valida".into()))?;
            }
        }
        _ if config.precision.is_some() || config.scale.is_some() || config.timezone.is_some() => {
            return Err(PlenoraError::InvalidPlan(
                "precision, scale e timezone non ammessi per questo target_type".into(),
            ));
        }
        _ => {}
    }
    Ok(())
}

/// Valida la config di un singolo passo tabellare contro i limiti, senza
/// toccare i dati (dispatcher per nome di operazione).
///
/// # Errors
///
/// - `PlenoraError::DataMapping`: config non deserializzabile nel tipo atteso
///   dall'operazione;
/// - `PlenoraError::InvalidPlan`: nomi, valori o limiti del passo non validi;
/// - `PlenoraError::Unsupported`: operazione sconosciuta al dispatcher.
#[allow(clippy::too_many_lines)] // Exhaustive contract dispatcher kept in one audited match.
pub fn validate_step_contract(step: &Step, limits: &Limits) -> Result<()> {
    match dispatch_name(&step.operation) {
        "drop_columns" => validate_name_list(
            &decode::<columns::DropColumns>(step)?.columns,
            limits.max_columns,
            "drop_columns",
            true,
        ),
        "rename" => validate_rename(&decode(step)?, limits),
        "reorder_columns" => validate_name_list(
            &decode::<columns::ReorderColumns>(step)?.columns,
            limits.max_columns,
            "reorder_columns",
            true,
        ),
        "select_columns" => validate_name_list(
            &decode::<columns::SelectColumns>(step)?.columns,
            limits.max_columns,
            "select_columns",
            false,
        ),
        "align_schema" => {
            let config = decode::<columns::AlignSchema>(step)?;
            let names: Vec<_> = config
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect();
            validate_name_list(&names, limits.max_columns, "align_schema", false)
        }
        "concat_columns" => {
            let config = decode::<columns::ConcatColumns>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "concat_columns", false)?;
            validate_output_name(&config.output_column)?;
            if config.separator.len() > limits.max_string_bytes {
                return Err(PlenoraError::InvalidPlan("separatore troppo grande".into()));
            }
            Ok(())
        }
        "split_column" => validate_split(&decode(step)?, limits),
        "string_pad" => validate_pad(&decode(step)?, limits),
        "string_length" => {
            let config = decode::<strings::StringLength>(step)?;
            validate_output_name(&config.column)?;
            config
                .output_column
                .as_deref()
                .map_or(Ok(()), validate_output_name)
        }
        "text_normalize" => validate_name_list(
            &decode::<strings::TextNormalize>(step)?.columns,
            limits.max_columns,
            "text_normalize",
            false,
        ),
        "fill_na" => {
            decode::<cleansing::FillNa>(step)?;
            Ok(())
        }
        "replace" => {
            let config = decode::<cleansing::Replace>(step)?;
            validate_output_name(&config.column)?;
            if config.old_value.len() > limits.max_regex_bytes
                || config.new_value.len() > limits.max_string_bytes
            {
                return Err(PlenoraError::InvalidPlan("replace oltre i limiti".into()));
            }
            if config.regex {
                regex::Regex::new(&config.old_value)
                    .map_err(|e| PlenoraError::InvalidPlan(format!("regex non valida: {e}")))?;
            }
            Ok(())
        }
        "type_cast" => {
            let config = decode::<cleansing::TypeCast>(step)?;
            validate_type_cast(&config)
        }
        "filter" => {
            let config = decode::<filtering::Filter>(step)?;
            validate_output_name(&config.column)
        }
        "conditional" => {
            let config = decode::<filtering::Conditional>(step)?;
            validate_output_name(&config.column)?;
            validate_output_name(&config.output_column)?;
            if config.conditions.is_empty() || config.conditions.len() > limits.max_columns {
                return Err(PlenoraError::InvalidPlan(
                    "numero condizioni non valido".into(),
                ));
            }
            Ok(())
        }
        "string_extract" => {
            let config = decode::<strings::StringExtract>(step)?;
            validate_output_name(&config.column)?;
            if config.pattern.is_empty() || config.pattern.len() > limits.max_regex_bytes {
                return Err(PlenoraError::InvalidPlan("pattern non valido".into()));
            }
            regex::Regex::new(&config.pattern)
                .map_err(|e| PlenoraError::InvalidPlan(format!("regex non valida: {e}")))?;
            if let Some(output) = config.output_column {
                validate_output_name(&output)?;
            }
            Ok(())
        }
        "date_extract" => {
            let config = decode::<utility::DateExtract>(step)?;
            validate_output_name(&config.column)?;
            if config
                .date_format
                .as_ref()
                .is_some_and(|format| format.is_empty() || format.len() > limits.max_string_bytes)
            {
                return Err(PlenoraError::InvalidPlan("date_format non valido".into()));
            }
            Ok(())
        }
        "uuid_generator" => {
            let config = decode::<utility::UuidGenerator>(step)?;
            validate_output_name(&config.output_column)
        }
        "limit" => {
            let config = decode::<utility::Limit>(step)?;
            let max_rows: u64 = limits.max_rows.try_into().unwrap_or(u64::MAX);
            if config.n > max_rows || config.offset > max_rows {
                return Err(PlenoraError::InvalidPlan(
                    "limit: n/offset oltre max_rows".into(),
                ));
            }
            Ok(())
        }
        "lookup" => {
            let config = decode::<analysis::Lookup>(step)?;
            validate_output_name(&config.column)?;
            if config.mapping.len() > limits.max_rows {
                return Err(PlenoraError::InvalidPlan("mapping oltre max_rows".into()));
            }
            if let Some(output) = config.output_column {
                validate_output_name(&output)?;
            }
            Ok(())
        }
        "flatten_json" => {
            let config = decode::<analysis::FlattenJson>(step)?;
            validate_output_name(&config.column)?;
            validate_name_list(
                &config.output_columns,
                limits.max_columns,
                "flatten_json",
                true,
            )
        }
        "mask_data" => {
            let config = decode::<security::MaskData>(step)?;
            if config.maskings.is_empty() || config.maskings.len() > limits.max_columns {
                return Err(PlenoraError::InvalidPlan(
                    "numero masking non valido".into(),
                ));
            }
            config
                .maskings
                .iter()
                .try_for_each(|masking| validate_output_name(&masking.column))
        }
        "md5_hash" => {
            let config = decode::<security::Md5Hash>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "md5_hash", false)?;
            validate_output_name(&config.output_column)?;
            if config.null_literal.len() > limits.max_string_bytes {
                return Err(PlenoraError::InvalidPlan(
                    "null_literal troppo grande".into(),
                ));
            }
            Ok(())
        }
        "add_row_number" => {
            let config = decode::<utility::AddRowNumber>(step)?;
            validate_output_name(&config.output_column)?;
            if let Some(column) = config.partition_column {
                validate_output_name(&column)?;
            }
            if config.order_column.is_some() {
                return Err(PlenoraError::InvalidPlan(
                    "add_row_number: order_column non ancora nel safe profile".into(),
                ));
            }
            Ok(())
        }
        "bin" => {
            let config = decode::<analysis::Bin>(step)?;
            validate_output_name(&config.column)
        }
        "sample" => {
            decode::<analysis::Sample>(step)?;
            Ok(())
        }
        "statistics" => {
            let config = decode::<analysis::Statistics>(step)?;
            validate_output_name(&config.column)
        }
        "sort" => {
            let config = decode::<aggregation::Sort>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "sort", false)
        }
        "top_n" => {
            let config = decode::<aggregation::TopN>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "top_n", false)?;
            let max_rows: u64 = limits.max_rows.try_into().unwrap_or(u64::MAX);
            if config.n > max_rows {
                return Err(PlenoraError::InvalidPlan("top_n: n oltre max_rows".into()));
            }
            Ok(())
        }
        "distinct" => {
            let config = decode::<aggregation::Distinct>(step)?;
            validate_name_list(&config.subset, limits.max_columns, "distinct", true)
        }
        "dedup_advanced" => {
            let config = decode::<aggregation::DedupAdvanced>(step)?;
            validate_name_list(&config.subset, limits.max_columns, "dedup", false)?;
            if let Some(column) = &config.order_column {
                validate_output_name(column)?;
            } else if !config.ascending {
                return Err(PlenoraError::InvalidPlan(
                    "dedup_advanced: ascending richiede order_column".into(),
                ));
            }
            Ok(())
        }
        "aggregate" => {
            let config = decode::<aggregation::Aggregate>(step)?;
            validate_name_list(&config.group_by, limits.max_columns, "aggregate", false)?;
            if config.aggregations.len() > limits.max_columns {
                return Err(PlenoraError::InvalidPlan("troppe aggregazioni".into()));
            }
            for aggregation in &config.aggregations {
                validate_output_name(&aggregation.column)?;
                if !aggregation.alias.is_empty() {
                    validate_output_name(&aggregation.alias)?;
                }
                if aggregation.separator.len() > limits.max_string_bytes {
                    return Err(PlenoraError::InvalidPlan(
                        "separatore aggregazione troppo grande".into(),
                    ));
                }
                if matches!(aggregation.function, aggregation::AggFunction::Quantile) {
                    if !aggregation
                        .quantile
                        .is_some_and(|value| value.is_finite() && (0.0..=1.0).contains(&value))
                    {
                        return Err(PlenoraError::InvalidPlan(
                            "quantile deve essere compreso tra 0 e 1".into(),
                        ));
                    }
                } else if aggregation.quantile.is_some() {
                    return Err(PlenoraError::InvalidPlan(
                        "quantile ammesso solo con function=quantile".into(),
                    ));
                }
            }
            Ok(())
        }
        "window_function" => {
            let config = decode::<aggregation::WindowFunction>(step)?;
            validate_output_name(&config.column)?;
            if let Some(name) = &config.group_by {
                validate_output_name(name)?;
            }
            if let Some(name) = &config.order_column {
                validate_output_name(name)?;
            }
            if let Some(name) = &config.output_column {
                validate_output_name(name)?;
            }
            if config.offset == 0 {
                return Err(PlenoraError::InvalidPlan(
                    "offset deve essere positivo".into(),
                ));
            }
            if matches!(config.function, aggregation::WindowKind::Ntile) {
                if !config
                    .buckets
                    .is_some_and(|buckets| buckets > 0 && buckets <= limits.max_rows)
                {
                    return Err(PlenoraError::InvalidPlan(
                        "ntile richiede buckets valido".into(),
                    ));
                }
            } else if config.buckets.is_some() {
                return Err(PlenoraError::InvalidPlan(
                    "buckets e' ammesso solo per ntile".into(),
                ));
            }
            Ok(())
        }
        "rolling_window" => {
            let config = decode::<aggregation::RollingWindow>(step)?;
            validate_output_name(&config.column)?;
            validate_output_name(&config.output_column)?;
            if let Some(name) = &config.group_by {
                validate_output_name(name)?;
            }
            if let Some(name) = &config.order_column {
                validate_output_name(name)?;
            }
            if config.window > limits.max_rows {
                return Err(PlenoraError::InvalidPlan(
                    "rolling_window: finestra oltre max_rows".into(),
                ));
            }
            if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
                return Err(PlenoraError::InvalidPlan(
                    "rolling_window: finestra non valida".into(),
                ));
            }
            Ok(())
        }
        "melt" => {
            let config = decode::<reshape::Melt>(step)?;
            if config.var_name == config.value_name {
                return Err(PlenoraError::InvalidPlan(
                    "melt richiede nomi distinti per variabile e valore".into(),
                ));
            }
            validate_name_list(&config.id_columns, limits.max_columns, "melt id", true)?;
            validate_name_list(
                &config.value_columns,
                limits.max_columns,
                "melt value",
                true,
            )?;
            validate_output_name(&config.var_name)?;
            validate_output_name(&config.value_name)
        }
        "pivot" => {
            let config = decode::<reshape::Pivot>(step)?;
            validate_output_name(&config.column)?;
            validate_output_name(&config.value_col)
        }
        "transpose" => {
            let config = decode::<reshape::Transpose>(step)?;
            validate_name_list(
                &config.output_columns,
                limits.max_columns,
                "transpose",
                true,
            )
        }
        "formula" => {
            let config = decode::<formula::Formula>(step)?;
            formula::validate(&config, limits.max_string_bytes)
        }
        "expression" => {
            let config = decode::<expressions::ExpressionTransform>(step)?;
            expressions::validate(&config, limits.max_columns.saturating_mul(16))
        }
        "assert_cardinality" => {
            let config = decode::<governance::AssertCardinality>(step)?;
            if config.exact_rows.is_none() && config.min_rows.is_none() && config.max_rows.is_none()
            {
                return Err(PlenoraError::InvalidPlan(
                    "assert_cardinality richiede exact_rows, min_rows o max_rows".into(),
                ));
            }
            if config.exact_rows.is_some()
                && (config.min_rows.is_some() || config.max_rows.is_some())
            {
                return Err(PlenoraError::InvalidPlan(
                    "exact_rows non puo' essere combinato con min_rows/max_rows".into(),
                ));
            }
            if config
                .min_rows
                .zip(config.max_rows)
                .is_some_and(|(min, max)| min > max)
                || [config.exact_rows, config.min_rows, config.max_rows]
                    .into_iter()
                    .flatten()
                    .any(|rows| rows > limits.max_rows)
            {
                return Err(PlenoraError::InvalidPlan(
                    "assert_cardinality: limiti non validi".into(),
                ));
            }
            Ok(())
        }
        "assert_metadata" => {
            let config = decode::<governance::AssertMetadata>(step)?;
            if config.expected.is_empty() || config.expected.len() > limits.max_columns {
                return Err(PlenoraError::InvalidPlan(
                    "assert_metadata: numero elementi non valido".into(),
                ));
            }
            if config.expected.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > limits.max_string_bytes
                    || value.len() > limits.max_string_bytes
            }) {
                return Err(PlenoraError::InvalidPlan(
                    "assert_metadata: chiave o valore oltre i limiti".into(),
                ));
            }
            Ok(())
        }
        "assert_schema" => {
            let config = decode::<quality::AssertSchema>(step)?;
            if config.fields.is_empty() || config.fields.len() > limits.max_columns {
                return Err(PlenoraError::InvalidPlan(
                    "assert_schema: numero campi non valido".into(),
                ));
            }
            let names = config
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect::<Vec<_>>();
            validate_name_list(&names, limits.max_columns, "assert_schema", false)?;
            for field in &config.fields {
                if !matches!(
                    field.data_type.trim().to_ascii_lowercase().as_str(),
                    "utf8"
                        | "string"
                        | "int64"
                        | "integer"
                        | "float64"
                        | "float"
                        | "double"
                        | "boolean"
                        | "bool"
                        | "uint64"
                        | "unsigned"
                        | "date32"
                        | "timestamp_millis"
                        | "decimal128"
                        | "binary"
                        | "dictionary_utf8"
                        | "list"
                        | "struct"
                ) {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "assert_schema: tipo non supportato {}",
                        field.data_type
                    )));
                }
            }
            Ok(())
        }
        "assert_not_null" => validate_name_list(
            &decode::<quality::AssertNotNull>(step)?.columns,
            limits.max_columns,
            "assert_not_null",
            false,
        ),
        "assert_unique" => validate_name_list(
            &decode::<quality::AssertUnique>(step)?.columns,
            limits.max_columns,
            "assert_unique",
            false,
        ),
        "assert_range" => {
            let config = decode::<quality::AssertRange>(step)?;
            validate_output_name(&config.column)?;
            if config.min.is_none() && config.max.is_none() {
                return Err(PlenoraError::InvalidPlan(
                    "assert_range richiede min o max".into(),
                ));
            }
            if config.min.is_some_and(|value| !value.is_finite())
                || config.max.is_some_and(|value| !value.is_finite())
                || config
                    .min
                    .zip(config.max)
                    .is_some_and(|(min, max)| min > max)
            {
                return Err(PlenoraError::InvalidPlan(
                    "assert_range: estremi non validi".into(),
                ));
            }
            Ok(())
        }
        "assert_regex" => {
            let config = decode::<quality::AssertRegex>(step)?;
            validate_output_name(&config.column)?;
            if config.pattern.is_empty() || config.pattern.len() > limits.max_regex_bytes {
                return Err(PlenoraError::InvalidPlan(
                    "assert_regex: pattern non valido".into(),
                ));
            }
            regex::Regex::new(&config.pattern)
                .map_err(|error| PlenoraError::InvalidPlan(format!("regex non valida: {error}")))?;
            Ok(())
        }
        "coalesce" => {
            let config = decode::<quality::Coalesce>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "coalesce", false)?;
            validate_output_name(&config.output_column)
        }
        "date_format" => {
            let config = decode::<dates::DateFormat>(step)?;
            validate_output_name(&config.column)?;
            dates::validate_format(
                &config.input_format,
                "input_format",
                limits.max_string_bytes,
            )?;
            dates::validate_format(
                &config.output_format,
                "output_format",
                limits.max_string_bytes,
            )?;
            validate_output_name(&config.output_column)
        }
        "date_add" => {
            let config = decode::<dates::DateAdd>(step)?;
            validate_output_name(&config.column)?;
            dates::validate_format(
                &config.input_format,
                "input_format",
                limits.max_string_bytes,
            )?;
            dates::validate_format(
                &config.output_format,
                "output_format",
                limits.max_string_bytes,
            )?;
            validate_output_name(&config.output_column)
        }
        "date_diff" => {
            let config = decode::<dates::DateDiff>(step)?;
            validate_output_name(&config.start_column)?;
            validate_output_name(&config.end_column)?;
            dates::validate_format(
                &config.input_format,
                "input_format",
                limits.max_string_bytes,
            )?;
            validate_output_name(&config.output_column)
        }
        "timezone_convert" => {
            let config = decode::<dates::TimezoneConvert>(step)?;
            validate_output_name(&config.column)?;
            dates::validate_format(
                &config.input_format,
                "input_format",
                limits.max_string_bytes,
            )?;
            dates::validate_format(
                &config.output_format,
                "output_format",
                limits.max_string_bytes,
            )?;
            config
                .source_timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| PlenoraError::InvalidPlan("source_timezone non valida".into()))?;
            config
                .target_timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| PlenoraError::InvalidPlan("target_timezone non valida".into()))?;
            validate_output_name(&config.output_column)
        }
        "sha256_hash" => {
            let config = decode::<security::Sha256Hash>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "sha256_hash", false)?;
            validate_output_name(&config.output_column)?;
            if config.null_literal.len() > limits.max_string_bytes {
                return Err(PlenoraError::InvalidPlan(
                    "null_literal troppo grande".into(),
                ));
            }
            Ok(())
        }
        "stable_fingerprint" => {
            let config = decode::<security::StableFingerprint>(step)?;
            validate_name_list(
                &config.columns,
                limits.max_columns,
                "stable_fingerprint",
                true,
            )?;
            validate_output_name(&config.output_column)
        }
        "hmac_sha256" => {
            let config = decode::<security::HmacSha256>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "hmac_sha256", false)?;
            validate_output_name(&config.output_column)?;
            if config.key_env.trim().is_empty() {
                return Err(PlenoraError::InvalidPlan(
                    "hmac_sha256: key_env vuoto".into(),
                ));
            }
            Ok(())
        }
        "explode" => {
            let config = decode::<reshape::Explode>(step)?;
            validate_output_name(&config.column)?;
            config
                .output_column
                .as_deref()
                .map_or(Ok(()), validate_output_name)
        }
        "unnest" => {
            let config = decode::<reshape::Unnest>(step)?;
            validate_output_name(&config.column)?;
            if config.prefix.len() > limits.max_string_bytes {
                return Err(PlenoraError::InvalidPlan(
                    "unnest: prefisso troppo grande".into(),
                ));
            }
            Ok(())
        }
        "join" => {
            let config = decode::<joins::Join>(step)?;
            validate_name_list(&config.left_keys, limits.max_columns, "join left", false)?;
            validate_name_list(&config.right_keys, limits.max_columns, "join right", false)
        }
        "semi_join" | "anti_join" => {
            let config = decode::<joins::MembershipJoin>(step)?;
            validate_name_list(
                &config.left_keys,
                limits.max_columns,
                "membership left",
                false,
            )?;
            validate_name_list(
                &config.right_keys,
                limits.max_columns,
                "membership right",
                false,
            )?;
            if config.left_keys.len() != config.right_keys.len() {
                return Err(PlenoraError::InvalidPlan(
                    "membership join: cardinalita' chiavi diversa".into(),
                ));
            }
            Ok(())
        }
        "asof_join" => {
            let config = decode::<joins::AsOfJoin>(step)?;
            validate_output_name(&config.left_on)?;
            validate_output_name(&config.right_on)?;
            validate_name_list(&config.left_by, limits.max_columns, "asof left_by", true)?;
            validate_name_list(&config.right_by, limits.max_columns, "asof right_by", true)?;
            if config.left_by.len() != config.right_by.len()
                || config
                    .tolerance
                    .is_some_and(|value| !value.is_finite() || value < 0.0)
            {
                return Err(PlenoraError::InvalidPlan(
                    "asof_join: configurazione non valida".into(),
                ));
            }
            Ok(())
        }
        "fuzzy_join" => {
            let config = decode::<fuzzy::FuzzyJoin>(step)?;
            validate_output_name(&config.left_key)?;
            validate_output_name(&config.right_key)?;
            if let Some(name) = &config.score_column {
                validate_output_name(name)?;
            }
            fuzzy::validate_config(&config)
        }
        "assert_foreign_key" => {
            let config = decode::<governance::ForeignKey>(step)?;
            validate_name_list(
                &config.left_keys,
                limits.max_columns,
                "foreign key left",
                false,
            )?;
            validate_name_list(
                &config.right_keys,
                limits.max_columns,
                "foreign key right",
                false,
            )?;
            if config.left_keys.len() != config.right_keys.len() {
                return Err(PlenoraError::InvalidPlan(
                    "foreign key: cardinalita' chiavi diversa".into(),
                ));
            }
            Ok(())
        }
        "reconcile" => {
            let config = decode::<governance::Reconcile>(step)?;
            validate_name_list(
                &config.left_keys,
                limits.max_columns,
                "reconcile left",
                false,
            )?;
            validate_name_list(
                &config.right_keys,
                limits.max_columns,
                "reconcile right",
                false,
            )?;
            if config.left_keys.len() != config.right_keys.len() {
                return Err(PlenoraError::InvalidPlan(
                    "reconcile: cardinalita' chiavi diversa".into(),
                ));
            }
            Ok(())
        }
        "validate_rules" => {
            let config = decode::<governance::ValidateRules>(step)?;
            let names: Vec<_> = config.rules.iter().map(|rule| rule.name.clone()).collect();
            validate_name_list(&names, limits.max_columns, "validate_rules", false)
        }
        "union_distinct" | "intersect" | "except" => {
            decode::<setops::SetOperation>(step)?;
            Ok(())
        }
        "concat" => {
            decode::<joins::Concat>(step)?;
            Ok(())
        }
        "concat_by_name" => {
            decode::<joins::ConcatByName>(step)?;
            Ok(())
        }
        "cross_join" => {
            decode::<joins::CrossJoin>(step)?;
            Ok(())
        }
        "table_diff" => {
            let config = decode::<reshape::TableDiff>(step)?;
            validate_name_list(&config.left_keys, limits.max_columns, "diff left", false)?;
            validate_name_list(&config.right_keys, limits.max_columns, "diff right", false)
        }
        operation => Err(PlenoraError::Unsupported(operation.into())),
    }
}

/// Validazione di un batch contro i limiti del piano.
///
/// I nomi duplicati sono proprieta' dello SCHEMA, non del batch: il
/// chiamante passa una cache (`names_validated`) con l'`Arc` dell'ultimo
/// schema verificato — uno stesso puntatore di schema non e' riesaminato
/// (hot path minimale: niente `HashSet` ricostruito a ogni batch sullo stesso schema).
fn validate_batch(
    batch: &RecordBatch,
    limits: &Limits,
    names_validated: &mut Option<SchemaRef>,
) -> Result<()> {
    if batch.num_rows() > limits.max_rows {
        return Err(PlenoraError::ResourceLimit(format!(
            "batch con {} righe oltre il limite {}",
            batch.num_rows(),
            limits.max_rows
        )));
    }
    if batch.num_columns() > limits.max_columns {
        return Err(PlenoraError::ResourceLimit(format!(
            "schema con {} colonne oltre il limite {}",
            batch.num_columns(),
            limits.max_columns
        )));
    }
    if names_validated
        .as_ref()
        .is_some_and(|schema| Arc::ptr_eq(schema, &batch.schema()))
    {
        return Ok(());
    }
    let mut names = HashSet::new();
    for field in batch.schema().fields() {
        if !names.insert(field.name()) {
            return Err(PlenoraError::Schema(format!(
                "nome colonna duplicato: {}",
                field.name()
            )));
        }
    }
    *names_validated = Some(batch.schema());
    Ok(())
}

fn normalize_large_utf8(batch: &RecordBatch) -> Result<RecordBatch> {
    let has_large_utf8 = batch
        .schema()
        .fields()
        .iter()
        .any(|field| field.data_type() == &DataType::LargeUtf8);
    // Test prima del clone della mappa (hot path minimale): la copia dei metadata serve
    // solo se c'e' davvero una voce da rimuovere.
    let has_pandas_metadata = batch.schema().metadata().contains_key("pandas");
    if !has_large_utf8 && !has_pandas_metadata {
        return Ok(batch.clone());
    }
    let mut metadata = batch.schema().metadata().clone();
    // Pandas stores a second, independent dtype/name schema in this opaque
    // entry. Any transform can make it stale, causing PyArrow to reinterpret
    // correct physical Arrow columns on read. Physical Arrow fields are the
    // engine contract; retain application metadata but drop this cache.
    let _ = metadata.remove("pandas");
    let mut fields = Vec::with_capacity(batch.num_columns());
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(batch.num_columns());
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        if field.data_type() == &DataType::LargeUtf8 {
            let strings = column
                .as_any()
                .downcast_ref::<LargeStringArray>()
                .ok_or_else(|| PlenoraError::Schema("downcast LargeUtf8 fallito".into()))?;
            let bytes = strings.iter().flatten().try_fold(0_usize, |total, value| {
                total.checked_add(value.len()).ok_or_else(|| {
                    PlenoraError::ResourceLimit("overflow dimensione colonna LargeUtf8".into())
                })
            })?;
            if bytes > i32::MAX as usize {
                return Err(PlenoraError::ResourceLimit(
                    "colonna LargeUtf8 oltre il limite sicuro Utf8 di Arrow".into(),
                ));
            }
            fields.push(field.as_ref().clone().with_data_type(DataType::Utf8));
            columns.push(Arc::new(strings.iter().collect::<StringArray>()));
        } else {
            fields.push(field.as_ref().clone());
            columns.push(column.clone());
        }
    }
    // Le colonne derivano dall'input: con un batch a zero colonne e i
    // metadata `pandas` da rimuovere, `try_new` RIFIUTEREBBE di costruire il
    // batch, e un'operazione legittima fallirebbe.
    plenora_core::batch_with_rows(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        columns,
        batch.num_rows(),
    )
}

/// Config tipizzata di un passo tabellare (configurazioni preparate, hot path minimale): deserializzata UNA VOLTA
/// in `Plan::validate` e riusata da ogni batch — niente JSON nel percorso
/// caldo. Ogni variante e' `Box`ata: le config hanno dimensioni molto
/// eterogenee e l'allocazione avviene una sola volta a monte dello stream.
///
/// Il dispatch resta quello esistente, per nome di operazione: `prepare_step`
/// mappa il nome sulla variante (stessa tabella di `validate_step_contract`),
/// `execute_step`/`execute_binary` fanno match sulla variante senza parsing.
#[derive(Debug)]
pub enum PreparedStep {
    /// `drop_columns`.
    DropColumns(Box<columns::DropColumns>),
    /// `rename`.
    Rename(Box<columns::Rename>),
    /// `reorder_columns`.
    ReorderColumns(Box<columns::ReorderColumns>),
    /// `select_columns`.
    SelectColumns(Box<columns::SelectColumns>),
    /// `align_schema`.
    AlignSchema(Box<columns::AlignSchema>),
    /// `concat_columns`.
    ConcatColumns(Box<columns::ConcatColumns>),
    /// `split_column`.
    SplitColumn(Box<columns::SplitColumn>),
    /// `string_pad`.
    StringPad(Box<strings::StringPad>),
    /// `string_length`.
    StringLength(Box<strings::StringLength>),
    /// `text_normalize`.
    TextNormalize(Box<strings::TextNormalize>),
    /// `fill_na`.
    FillNa(Box<cleansing::FillNa>),
    /// `replace`.
    Replace(Box<cleansing::Replace>),
    /// `type_cast`.
    TypeCast(Box<cleansing::TypeCast>),
    /// `filter`.
    Filter(Box<filtering::Filter>),
    /// `conditional`.
    Conditional(Box<filtering::Conditional>),
    /// `string_extract`.
    StringExtract(Box<strings::StringExtract>),
    /// `date_extract`.
    DateExtract(Box<utility::DateExtract>),
    /// `uuid_generator`.
    UuidGenerator(Box<utility::UuidGenerator>),
    /// `limit`.
    Limit(Box<utility::Limit>),
    /// `lookup`.
    Lookup(Box<analysis::Lookup>),
    /// `flatten_json`.
    FlattenJson(Box<analysis::FlattenJson>),
    /// `mask_data`.
    MaskData(Box<security::MaskData>),
    /// `md5_hash`.
    Md5Hash(Box<security::Md5Hash>),
    /// `add_row_number`.
    AddRowNumber(Box<utility::AddRowNumber>),
    /// `bin`.
    Bin(Box<analysis::Bin>),
    /// `sample`.
    Sample(Box<analysis::Sample>),
    /// `statistics`.
    Statistics(Box<analysis::Statistics>),
    /// `sort`.
    Sort(Box<aggregation::Sort>),
    /// `top_n`.
    TopN(Box<aggregation::TopN>),
    /// `distinct`.
    Distinct(Box<aggregation::Distinct>),
    /// `dedup_advanced`.
    DedupAdvanced(Box<aggregation::DedupAdvanced>),
    /// `aggregate`.
    Aggregate(Box<aggregation::Aggregate>),
    /// `window_function`.
    WindowFunction(Box<aggregation::WindowFunction>),
    /// `rolling_window`.
    RollingWindow(Box<aggregation::RollingWindow>),
    /// `melt`.
    Melt(Box<reshape::Melt>),
    /// `pivot`.
    Pivot(Box<reshape::Pivot>),
    /// `transpose`.
    Transpose(Box<reshape::Transpose>),
    /// `formula`.
    Formula(Box<formula::Formula>),
    /// `expression`.
    Expression(Box<expressions::ExpressionTransform>),
    /// `assert_cardinality`.
    AssertCardinality(Box<governance::AssertCardinality>),
    /// `assert_metadata`.
    AssertMetadata(Box<governance::AssertMetadata>),
    /// `assert_schema`.
    AssertSchema(Box<quality::AssertSchema>),
    /// `assert_not_null`.
    AssertNotNull(Box<quality::AssertNotNull>),
    /// `assert_unique`.
    AssertUnique(Box<quality::AssertUnique>),
    /// `assert_range`.
    AssertRange(Box<quality::AssertRange>),
    /// `assert_regex`.
    AssertRegex(Box<quality::AssertRegex>),
    /// `coalesce`.
    Coalesce(Box<quality::Coalesce>),
    /// `date_format`.
    DateFormat(Box<dates::DateFormat>),
    /// `date_add`.
    DateAdd(Box<dates::DateAdd>),
    /// `date_diff`.
    DateDiff(Box<dates::DateDiff>),
    /// `timezone_convert`.
    TimezoneConvert(Box<dates::TimezoneConvert>),
    /// `sha256_hash`.
    Sha256Hash(Box<security::Sha256Hash>),
    /// `stable_fingerprint`.
    StableFingerprint(Box<security::StableFingerprint>),
    /// `hmac_sha256`.
    HmacSha256(Box<security::HmacSha256>),
    /// `validate_rules`.
    ValidateRules(Box<governance::ValidateRules>),
    /// `explode`.
    Explode(Box<reshape::Explode>),
    /// `unnest`.
    Unnest(Box<reshape::Unnest>),
    /// `join`.
    Join(Box<joins::Join>),
    /// `concat`.
    Concat(Box<joins::Concat>),
    /// `concat_by_name`.
    ConcatByName(Box<joins::ConcatByName>),
    /// `cross_join`.
    CrossJoin(Box<joins::CrossJoin>),
    /// `table_diff`.
    TableDiff(Box<reshape::TableDiff>),
    /// `semi_join`.
    SemiJoin(Box<joins::MembershipJoin>),
    /// `anti_join`.
    AntiJoin(Box<joins::MembershipJoin>),
    /// `asof_join`.
    AsOfJoin(Box<joins::AsOfJoin>),
    /// `fuzzy_join`.
    FuzzyJoin(Box<fuzzy::FuzzyJoin>),
    /// `union_distinct`.
    UnionDistinct(Box<setops::SetOperation>),
    /// `intersect`.
    Intersect(Box<setops::SetOperation>),
    /// `except`.
    Except(Box<setops::SetOperation>),
    /// `assert_foreign_key`.
    AssertForeignKey(Box<governance::ForeignKey>),
    /// `reconcile`.
    Reconcile(Box<governance::Reconcile>),
}

impl PreparedStep {
    /// Nome di dispatch dell'operazione (attribution errori e scelta spill).
    #[must_use]
    pub(crate) const fn name(&self) -> &'static str {
        match self {
            Self::DropColumns(_) => "drop_columns",
            Self::Rename(_) => "rename",
            Self::ReorderColumns(_) => "reorder_columns",
            Self::SelectColumns(_) => "select_columns",
            Self::AlignSchema(_) => "align_schema",
            Self::ConcatColumns(_) => "concat_columns",
            Self::SplitColumn(_) => "split_column",
            Self::StringPad(_) => "string_pad",
            Self::StringLength(_) => "string_length",
            Self::TextNormalize(_) => "text_normalize",
            Self::FillNa(_) => "fill_na",
            Self::Replace(_) => "replace",
            Self::TypeCast(_) => "type_cast",
            Self::Filter(_) => "filter",
            Self::Conditional(_) => "conditional",
            Self::StringExtract(_) => "string_extract",
            Self::DateExtract(_) => "date_extract",
            Self::UuidGenerator(_) => "uuid_generator",
            Self::Limit(_) => "limit",
            Self::Lookup(_) => "lookup",
            Self::FlattenJson(_) => "flatten_json",
            Self::MaskData(_) => "mask_data",
            Self::Md5Hash(_) => "md5_hash",
            Self::AddRowNumber(_) => "add_row_number",
            Self::Bin(_) => "bin",
            Self::Sample(_) => "sample",
            Self::Statistics(_) => "statistics",
            Self::Sort(_) => "sort",
            Self::TopN(_) => "top_n",
            Self::Distinct(_) => "distinct",
            Self::DedupAdvanced(_) => "dedup_advanced",
            Self::Aggregate(_) => "aggregate",
            Self::WindowFunction(_) => "window_function",
            Self::RollingWindow(_) => "rolling_window",
            Self::Melt(_) => "melt",
            Self::Pivot(_) => "pivot",
            Self::Transpose(_) => "transpose",
            Self::Formula(_) => "formula",
            Self::Expression(_) => "expression",
            Self::AssertCardinality(_) => "assert_cardinality",
            Self::AssertMetadata(_) => "assert_metadata",
            Self::AssertSchema(_) => "assert_schema",
            Self::AssertNotNull(_) => "assert_not_null",
            Self::AssertUnique(_) => "assert_unique",
            Self::AssertRange(_) => "assert_range",
            Self::AssertRegex(_) => "assert_regex",
            Self::Coalesce(_) => "coalesce",
            Self::DateFormat(_) => "date_format",
            Self::DateAdd(_) => "date_add",
            Self::DateDiff(_) => "date_diff",
            Self::TimezoneConvert(_) => "timezone_convert",
            Self::Sha256Hash(_) => "sha256_hash",
            Self::StableFingerprint(_) => "stable_fingerprint",
            Self::HmacSha256(_) => "hmac_sha256",
            Self::ValidateRules(_) => "validate_rules",
            Self::Explode(_) => "explode",
            Self::Unnest(_) => "unnest",
            Self::Join(_) => "join",
            Self::Concat(_) => "concat",
            Self::ConcatByName(_) => "concat_by_name",
            Self::CrossJoin(_) => "cross_join",
            Self::TableDiff(_) => "table_diff",
            Self::SemiJoin(_) => "semi_join",
            Self::AntiJoin(_) => "anti_join",
            Self::AsOfJoin(_) => "asof_join",
            Self::FuzzyJoin(_) => "fuzzy_join",
            Self::UnionDistinct(_) => "union_distinct",
            Self::Intersect(_) => "intersect",
            Self::Except(_) => "except",
            Self::AssertForeignKey(_) => "assert_foreign_key",
            Self::Reconcile(_) => "reconcile",
        }
    }
}

/// Deserializza la config di un passo nella sua forma tipizzata (configurazioni preparate, hot path minimale).
/// Chiamata una sola volta per passo da `Plan::validate`, mai per batch;
/// il dispatch per nome e' lo stesso di `validate_step_contract`.
///
/// # Errors
///
/// - `PlenoraError::DataMapping`: config non deserializzabile nel tipo atteso
///   dall'operazione;
/// - `PlenoraError::Unsupported`: operazione sconosciuta al dispatcher.
#[allow(clippy::too_many_lines)] // Mirror of the audited dispatcher, one arm per operation.
pub fn prepare_step(step: &Step) -> Result<PreparedStep> {
    Ok(match dispatch_name(&step.operation) {
        "drop_columns" => PreparedStep::DropColumns(Box::new(decode(step)?)),
        "rename" => PreparedStep::Rename(Box::new(decode(step)?)),
        "reorder_columns" => PreparedStep::ReorderColumns(Box::new(decode(step)?)),
        "select_columns" => PreparedStep::SelectColumns(Box::new(decode(step)?)),
        "align_schema" => PreparedStep::AlignSchema(Box::new(decode(step)?)),
        "concat_columns" => PreparedStep::ConcatColumns(Box::new(decode(step)?)),
        "split_column" => PreparedStep::SplitColumn(Box::new(decode(step)?)),
        "string_pad" => PreparedStep::StringPad(Box::new(decode(step)?)),
        "string_length" => PreparedStep::StringLength(Box::new(decode(step)?)),
        "text_normalize" => PreparedStep::TextNormalize(Box::new(decode(step)?)),
        "fill_na" => PreparedStep::FillNa(Box::new(decode(step)?)),
        "replace" => PreparedStep::Replace(Box::new(decode(step)?)),
        "type_cast" => PreparedStep::TypeCast(Box::new(decode(step)?)),
        "filter" => PreparedStep::Filter(Box::new(decode(step)?)),
        "conditional" => PreparedStep::Conditional(Box::new(decode(step)?)),
        "string_extract" => PreparedStep::StringExtract(Box::new(decode(step)?)),
        "date_extract" => PreparedStep::DateExtract(Box::new(decode(step)?)),
        "uuid_generator" => PreparedStep::UuidGenerator(Box::new(decode(step)?)),
        "limit" => PreparedStep::Limit(Box::new(decode(step)?)),
        "lookup" => PreparedStep::Lookup(Box::new(decode(step)?)),
        "flatten_json" => PreparedStep::FlattenJson(Box::new(decode(step)?)),
        "mask_data" => PreparedStep::MaskData(Box::new(decode(step)?)),
        "md5_hash" => PreparedStep::Md5Hash(Box::new(decode(step)?)),
        "add_row_number" => PreparedStep::AddRowNumber(Box::new(decode(step)?)),
        "bin" => PreparedStep::Bin(Box::new(decode(step)?)),
        "sample" => PreparedStep::Sample(Box::new(decode(step)?)),
        "statistics" => PreparedStep::Statistics(Box::new(decode(step)?)),
        "sort" => PreparedStep::Sort(Box::new(decode(step)?)),
        "top_n" => PreparedStep::TopN(Box::new(decode(step)?)),
        "distinct" => PreparedStep::Distinct(Box::new(decode(step)?)),
        "dedup_advanced" => PreparedStep::DedupAdvanced(Box::new(decode(step)?)),
        "aggregate" => PreparedStep::Aggregate(Box::new(decode(step)?)),
        "window_function" => PreparedStep::WindowFunction(Box::new(decode(step)?)),
        "rolling_window" => PreparedStep::RollingWindow(Box::new(decode(step)?)),
        "melt" => PreparedStep::Melt(Box::new(decode(step)?)),
        "pivot" => PreparedStep::Pivot(Box::new(decode(step)?)),
        "transpose" => PreparedStep::Transpose(Box::new(decode(step)?)),
        "formula" => PreparedStep::Formula(Box::new(decode(step)?)),
        "expression" => PreparedStep::Expression(Box::new(decode(step)?)),
        "assert_cardinality" => PreparedStep::AssertCardinality(Box::new(decode(step)?)),
        "assert_metadata" => PreparedStep::AssertMetadata(Box::new(decode(step)?)),
        "assert_schema" => PreparedStep::AssertSchema(Box::new(decode(step)?)),
        "assert_not_null" => PreparedStep::AssertNotNull(Box::new(decode(step)?)),
        "assert_unique" => PreparedStep::AssertUnique(Box::new(decode(step)?)),
        "assert_range" => PreparedStep::AssertRange(Box::new(decode(step)?)),
        "assert_regex" => PreparedStep::AssertRegex(Box::new(decode(step)?)),
        "coalesce" => PreparedStep::Coalesce(Box::new(decode(step)?)),
        "date_format" => PreparedStep::DateFormat(Box::new(decode(step)?)),
        "date_add" => PreparedStep::DateAdd(Box::new(decode(step)?)),
        "date_diff" => PreparedStep::DateDiff(Box::new(decode(step)?)),
        "timezone_convert" => PreparedStep::TimezoneConvert(Box::new(decode(step)?)),
        "sha256_hash" => PreparedStep::Sha256Hash(Box::new(decode(step)?)),
        "stable_fingerprint" => PreparedStep::StableFingerprint(Box::new(decode(step)?)),
        "hmac_sha256" => PreparedStep::HmacSha256(Box::new(decode(step)?)),
        "validate_rules" => PreparedStep::ValidateRules(Box::new(decode(step)?)),
        "explode" => PreparedStep::Explode(Box::new(decode(step)?)),
        "unnest" => PreparedStep::Unnest(Box::new(decode(step)?)),
        "join" => PreparedStep::Join(Box::new(decode(step)?)),
        "concat" => PreparedStep::Concat(Box::new(decode(step)?)),
        "concat_by_name" => PreparedStep::ConcatByName(Box::new(decode(step)?)),
        "cross_join" => PreparedStep::CrossJoin(Box::new(decode(step)?)),
        "table_diff" => PreparedStep::TableDiff(Box::new(decode(step)?)),
        "semi_join" => PreparedStep::SemiJoin(Box::new(decode(step)?)),
        "anti_join" => PreparedStep::AntiJoin(Box::new(decode(step)?)),
        "asof_join" => PreparedStep::AsOfJoin(Box::new(decode(step)?)),
        "fuzzy_join" => PreparedStep::FuzzyJoin(Box::new(decode(step)?)),
        "union_distinct" => PreparedStep::UnionDistinct(Box::new(decode(step)?)),
        "intersect" => PreparedStep::Intersect(Box::new(decode(step)?)),
        "except" => PreparedStep::Except(Box::new(decode(step)?)),
        "assert_foreign_key" => PreparedStep::AssertForeignKey(Box::new(decode(step)?)),
        "reconcile" => PreparedStep::Reconcile(Box::new(decode(step)?)),
        operation => return Err(PlenoraError::Unsupported(operation.into())),
    })
}

/// Esegue un passo unario sulla batch usando la config gia' tipizzata
/// (configurazioni preparate, hot path minimale: nessuna deserializzazione nel percorso caldo).
///
/// `spill_dir` e' la directory di spill del chiamante (`None`: tempdir
/// posseduta per operazione), `spill_metrics` accumula le metriche degli
/// spill attivati nella catena (architettura.md#memoria).
#[allow(clippy::too_many_lines)] // Exhaustive contract dispatcher kept in one audited match.
fn execute_step(
    batch: &RecordBatch,
    step: &PreparedStep,
    limits: &Limits,
    spill_dir: Option<&Path>,
    spill_metrics: &mut spill::SpillMetrics,
) -> Result<RecordBatch> {
    match step {
        PreparedStep::DropColumns(config) => columns::drop_columns(batch, config),
        PreparedStep::Rename(config) => columns::rename(batch, config),
        PreparedStep::ReorderColumns(config) => columns::reorder_columns(batch, config),
        PreparedStep::SelectColumns(config) => columns::select_columns(batch, config),
        PreparedStep::AlignSchema(config) => columns::align_schema(batch, config),
        PreparedStep::ConcatColumns(config) => columns::concat_columns(batch, config, limits),
        PreparedStep::SplitColumn(config) => columns::split_column(batch, config, limits),
        PreparedStep::StringPad(config) => strings::string_pad(batch, config, limits),
        PreparedStep::StringLength(config) => strings::string_length(batch, config),
        PreparedStep::TextNormalize(config) => strings::text_normalize(batch, config, limits),
        PreparedStep::FillNa(config) => cleansing::fill_na(batch, config),
        PreparedStep::Replace(config) => cleansing::replace(batch, config),
        PreparedStep::TypeCast(config) => cleansing::type_cast(batch, config),
        PreparedStep::Filter(config) => filtering::filter(batch, config),
        PreparedStep::Conditional(config) => filtering::conditional(batch, config),
        PreparedStep::StringExtract(config) => strings::string_extract(batch, config, limits),
        PreparedStep::DateExtract(config) => utility::date_extract(batch, config),
        PreparedStep::UuidGenerator(config) => utility::uuid_generator(batch, config),
        PreparedStep::Limit(config) => utility::limit(batch, config),
        PreparedStep::Lookup(config) => analysis::lookup(batch, config),
        PreparedStep::FlattenJson(config) => analysis::flatten_json(batch, config, limits),
        PreparedStep::MaskData(config) => security::mask_data(batch, config),
        PreparedStep::Md5Hash(config) => security::md5_hash(batch, config),
        PreparedStep::AddRowNumber(config) => utility::add_row_number(batch, config),
        PreparedStep::Bin(config) => analysis::bin(batch, config),
        PreparedStep::Sample(config) => analysis::sample(batch, config),
        PreparedStep::Statistics(config) => analysis::statistics(batch, config),
        PreparedStep::Sort(config) => {
            sort_dispatch(batch, config, limits, spill_dir, spill_metrics)
        }
        PreparedStep::TopN(config) => aggregation::top_n(batch, config),
        PreparedStep::Distinct(config) => {
            distinct_dispatch(batch, config, limits, spill_dir, spill_metrics)
        }
        PreparedStep::DedupAdvanced(config) => aggregation::dedup_advanced(batch, config),
        PreparedStep::Aggregate(config) => {
            aggregate_dispatch(batch, config, limits, spill_dir, spill_metrics)
        }
        PreparedStep::WindowFunction(config) => aggregation::window_function(batch, config),
        PreparedStep::RollingWindow(config) => aggregation::rolling_window(batch, config),
        PreparedStep::Melt(config) => reshape::melt(batch, config, limits),
        PreparedStep::Pivot(config) => reshape::pivot(batch, config, limits),
        PreparedStep::Transpose(config) => reshape::transpose(batch, config, limits),
        PreparedStep::Formula(config) => formula::formula(batch, config),
        PreparedStep::Expression(config) => expressions::expression(batch, config),
        PreparedStep::AssertCardinality(config) => governance::assert_cardinality(batch, config),
        PreparedStep::AssertMetadata(config) => governance::assert_metadata(batch, config),
        PreparedStep::AssertSchema(config) => quality::assert_schema(batch, config),
        PreparedStep::AssertNotNull(config) => quality::assert_not_null(batch, config),
        PreparedStep::AssertUnique(config) => quality::assert_unique(batch, config),
        PreparedStep::AssertRange(config) => quality::assert_range(batch, config),
        PreparedStep::AssertRegex(config) => quality::assert_regex(batch, config),
        PreparedStep::Coalesce(config) => quality::coalesce(batch, config),
        PreparedStep::DateFormat(config) => dates::date_format(batch, config),
        PreparedStep::DateAdd(config) => dates::date_add(batch, config),
        PreparedStep::DateDiff(config) => dates::date_diff(batch, config),
        PreparedStep::TimezoneConvert(config) => dates::timezone_convert(batch, config),
        PreparedStep::Sha256Hash(config) => security::sha256_hash(batch, config),
        PreparedStep::StableFingerprint(config) => security::stable_fingerprint(batch, config),
        PreparedStep::HmacSha256(config) => security::hmac_sha256(batch, config),
        PreparedStep::ValidateRules(config) => governance::validate_rules(batch, config),
        PreparedStep::Explode(config) => reshape::explode(batch, config, limits),
        PreparedStep::Unnest(config) => reshape::unnest(batch, config, limits),
        binary => Err(PlenoraError::Unsupported(binary.name().into())),
    }
}

// ---------------------------------------------------------------------------
// Selezione dello spill unario (architettura.md#memoria)
// ---------------------------------------------------------------------------

/// Il piano unario e' spill-capable: un solo passo `sort`/`distinct`/
/// `aggregate` (la forma prodotta da `prepare` per i nodi del DAG), cioe' le
/// operazioni con variante `*_spilled` in `plenora_kernels_table::spill`.
/// Usata dall'executor del DAG per la gestione della quota governor.
pub fn unary_spill_capable(plan: &ValidatedPlan) -> bool {
    matches!(
        plan.prepared_steps(),
        [PreparedStep::Sort(_) | PreparedStep::Distinct(_) | PreparedStep::Aggregate(_)]
    )
}

/// Accumulatore delle metriche di spill della catena: satura invece di
/// avvolgere o abortire, e registra la saturazione in `SpillMetrics` perche'
/// arrivi fino alle metriche di esecuzione.
fn accumulate_spill(total: &mut spill::SpillMetrics, delta: spill::SpillMetrics) {
    total.accumulate(delta);
}

/// Workspace di spill per un'operazione: sulla directory del chiamante
/// (`Some`, es. il `TempStore` condiviso per `execution_id` di
/// plenora-engine — mai rimossa da qui) o su una tempdir posseduta (`None`,
/// percorso legacy); la quota resta `limits.max_temp_bytes` in entrambi i
/// casi.
fn spill_workspace(spill_dir: Option<&Path>, limits: &Limits) -> Result<spill::RowSpillWorkspace> {
    spill_dir.map_or_else(
        || spill::RowSpillWorkspace::new(limits.max_temp_bytes),
        |directory| spill::RowSpillWorkspace::with_directory(directory, limits.max_temp_bytes),
    )
}

/// `table.sort` con attivazione preventiva dello spill (architettura.md#memoria): stessa
/// soglia deterministica del set-op spilled (byte stimati dell'input vs
/// `max_governed_memory_bytes`); sopra soglia external merge sort su disco, sotto il
/// percorso in memoria. Output identico nei due casi.
fn sort_dispatch(
    batch: &RecordBatch,
    config: &aggregation::Sort,
    limits: &Limits,
    spill_dir: Option<&Path>,
    spill_metrics: &mut spill::SpillMetrics,
) -> Result<RecordBatch> {
    if !spill::should_spill_unary(batch, limits) {
        return aggregation::sort(batch, config);
    }
    let mut workspace = spill_workspace(spill_dir, limits)?;
    let (output, metrics) = spill::sort_spilled_in(batch, config, limits, &mut workspace)?;
    accumulate_spill(spill_metrics, metrics);
    Ok(output)
}

/// Come [`sort_dispatch`], per `table.distinct`.
fn distinct_dispatch(
    batch: &RecordBatch,
    config: &aggregation::Distinct,
    limits: &Limits,
    spill_dir: Option<&Path>,
    spill_metrics: &mut spill::SpillMetrics,
) -> Result<RecordBatch> {
    if !spill::should_spill_unary(batch, limits) {
        return aggregation::distinct(batch, config);
    }
    let mut workspace = spill_workspace(spill_dir, limits)?;
    let (output, metrics) = spill::distinct_spilled_in(batch, config, limits, &mut workspace)?;
    accumulate_spill(spill_metrics, metrics);
    Ok(output)
}

/// Come [`sort_dispatch`], per `table.aggregate`.
fn aggregate_dispatch(
    batch: &RecordBatch,
    config: &aggregation::Aggregate,
    limits: &Limits,
    spill_dir: Option<&Path>,
    spill_metrics: &mut spill::SpillMetrics,
) -> Result<RecordBatch> {
    if !spill::should_spill_unary(batch, limits) {
        return aggregation::aggregate(batch, config);
    }
    let mut workspace = spill_workspace(spill_dir, limits)?;
    let (output, metrics) = spill::aggregate_spilled_in(batch, config, limits, &mut workspace)?;
    accumulate_spill(spill_metrics, metrics);
    Ok(output)
}

/// Esegue una catena gia' validata su una singola batch Arrow.
///
/// # Errors
///
/// Restituisce un errore contestualizzato col passo se schema, limiti o kernel
/// non possono garantire un risultato deterministico.
pub fn execute_batch(batch: RecordBatch, plan: &ValidatedPlan) -> Result<RecordBatch> {
    execute_batch_with_spill(batch, plan, None).map(|(output, _)| output)
}

/// Esegue un input che il chiamante dichiara materializzato per l'intera
/// operazione.
///
/// Solo questo confine legacy puo' pubblicare diagnostics `complete`;
/// [`execute_batch`] rifiuta invece diagnostics batch-local.
///
/// # Errors
///
/// Come [`execute_batch`]; in piu' `InvalidPlan` se il piano produce
/// diagnostics batch-locali non aggregabili a questo confine.
pub fn execute_complete_batch(batch: RecordBatch, plan: &ValidatedPlan) -> Result<RecordBatch> {
    execute_batch_with_spill_impl(batch, plan, None, true).map(|(output, _)| output)
}

/// Come [`execute_batch`], ma con la directory di spill decisa dal
/// chiamante (architettura.md#memoria, spill generalizzato).
///
/// `Some(dir)` instrada i file di spill di
/// `sort`/`distinct`/`aggregate` nella directory condivisa dell'esecuzione
/// (il `TempStore` di plenora-engine — creata se manca, mai rimossa da qui;
/// i file di spill sono comunque ripuliti a fine operazione), `None`
/// corrisponde al comportamento storico (tempdir posseduta per operazione).
///
/// Restituisce anche le metriche di spill aggregate sulla catena (byte
/// scritti/letti e file materializzati): azzerate se nessun passo ha
/// spillato.
///
/// # Errors
///
/// Come [`execute_batch`]; in piu' l'errore dedicato `Contract`
/// `max_temp_bytes` se la quota temp dello spill e' superata.
pub fn execute_batch_with_spill(
    batch: RecordBatch,
    plan: &ValidatedPlan,
    spill_dir: Option<&Path>,
) -> Result<(RecordBatch, spill::SpillMetrics)> {
    execute_batch_with_spill_impl(batch, plan, spill_dir, false)
}

/// Percorso interno del DAG: il chiamante aggrega deterministicamente tutti i
/// batch e applica gli offset assoluti prima di rendere pubblico l'errore.
// Re-esportato `pub(crate)` da `table_engine::mod`: la visibilita' dichiarata
// e' quella necessaria al confine crate (il lint non vede la re-export).
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn execute_batch_with_spill_row_diagnostics(
    batch: RecordBatch,
    plan: &ValidatedPlan,
    spill_dir: Option<&Path>,
) -> Result<(RecordBatch, spill::SpillMetrics)> {
    execute_batch_with_spill_impl(batch, plan, spill_dir, true)
}

/// Contesto del passo per il percorso LEGACY (piani `schema_version <= 3`).
///
/// L'equivalente DAG e' `executor::step_error`. Qui vale la stessa regola: un
/// errore che porta una CATEGORIA da preservare non viene avvolto in
/// `Execution`, perche' l'involucro la sostituirebbe con `execution` e
/// l'exit code 6. Con un avvolgimento incondizionato un limite di risorsa
/// alzato da un kernel uscirebbe dal percorso legacy come `execution` mentre
/// il percorso DAG lo conserva: la stessa esecuzione darebbe due categorie
/// diverse a seconda della versione del piano.
///
/// Le categorie preservate sono quelle su cui il chiamante DECIDE in modo
/// diverso da «il passo e' fallito»: `ResourceLimit` (rilancia con piu'
/// budget, exit 4), `Internal` (un'invariante NOSTRA e' rotta: exit 70, ed e'
/// un difetto da segnalare a noi) e `Cancelled` (nessuna decisione: e' stato
/// lui a fermare). Il contesto — indice del passo e operazione — non va
/// perso: viaggia in `Replayed`, che porta categoria e attribuzione insieme.
fn legacy_step_error(error: &PlenoraError, index: usize, operation: &str) -> PlenoraError {
    let categoria = error.category();
    if crate::error_propagation::categoria_preservata(categoria) {
        return PlenoraError::Replayed(Box::new(ReplayedError {
            category: categoria,
            phase: error.phase(),
            remote_effect: error.remote_effect(),
            retry: error.retry_disposition(),
            message: error.to_string(),
            node: Some(index.to_string()),
            operation: Some(operation.to_owned()),
            execution_id: None,
            execution_reason: error.execution_reason().map(ToOwned::to_owned),
        }));
    }
    PlenoraError::Execution {
        node: index.to_string(),
        operation: operation.to_owned(),
        // Percorso legacy: nessuna esecuzione DAG, nessun execution_id.
        execution_id: String::new(),
        reason: error.to_string(),
    }
}

fn execute_batch_with_spill_impl(
    mut batch: RecordBatch,
    plan: &ValidatedPlan,
    spill_dir: Option<&Path>,
    allow_batch_local_diagnostics: bool,
) -> Result<(RecordBatch, spill::SpillMetrics)> {
    if plan.requires_secondary() {
        return Err(PlenoraError::InvalidPlan(
            "il piano richiede un secondo input".into(),
        ));
    }
    batch = normalize_large_utf8(&batch)?;
    // Cache dello schema gia' validato (hot path minimale): i passi che conservano lo
    // schema (stesso `Arc`) non riesaminano i nomi duplicati.
    let mut names_validated: Option<SchemaRef> = None;
    validate_batch(&batch, plan.limits(), &mut names_validated)?;
    let mut spill_metrics = spill::SpillMetrics::default();
    for (index, (step, prepared)) in plan.steps().iter().zip(plan.prepared_steps()).enumerate() {
        batch = execute_step(
            &batch,
            prepared,
            plan.limits(),
            spill_dir,
            &mut spill_metrics,
        )
        .map_err(|error| {
            if error.row_diagnostics().is_some() {
                if allow_batch_local_diagnostics {
                    error
                } else {
                    PlenoraError::Unsupported(
                        "row diagnostics batch-local non pubblicabili: usare l'executor DAG"
                            .to_owned(),
                    )
                }
            } else {
                legacy_step_error(&error, index, &step.operation)
            }
        })?;
        validate_batch(&batch, plan.limits(), &mut names_validated)
            .map_err(|error| legacy_step_error(&error, index, &step.operation))?;
    }
    Ok((batch, spill_metrics))
}

/// Esegue un piano binario validato su due batch complete.
///
/// # Errors
/// Restituisce un errore se il piano non e' binario o supera i limiti.
pub fn execute_binary(
    left: &RecordBatch,
    right: &RecordBatch,
    plan: &ValidatedPlan,
) -> Result<RecordBatch> {
    if !plan.requires_secondary() || plan.steps().len() != 1 {
        return Err(PlenoraError::InvalidPlan("piano binario non valido".into()));
    }
    let left = normalize_large_utf8(left)?;
    let right = normalize_large_utf8(right)?;
    // Cache dello schema gia' validato (hot path minimale), come `execute_batch_with_spill`.
    let mut names_validated: Option<SchemaRef> = None;
    validate_batch(&left, plan.limits(), &mut names_validated)?;
    validate_batch(&right, plan.limits(), &mut names_validated)?;
    let prepared = &plan.prepared_steps()[0];
    if matches!(
        prepared,
        PreparedStep::UnionDistinct(_) | PreparedStep::Intersect(_) | PreparedStep::Except(_)
    ) && spill::should_spill(&left, &right, plan.limits())
    {
        // NOTA (spill generalizzato): il set-op spilled usa ancora una tempdir
        // posseduta interna a `execute_set_operation` — kernels-table non
        // espone una variante `*_in` con workspace del chiamante per i
        // set-op, quindi questo percorso NON transita dalla directory
        // condivisa del `TempStore` (diversamente da sort/distinct/
        // aggregate, cfr. `execute_batch_with_spill`).
        let output = spill::execute_set_operation(prepared.name(), &left, &right, plan.limits())?;
        validate_batch(&output, plan.limits(), &mut names_validated)?;
        return Ok(output);
    }
    let output = match prepared {
        PreparedStep::Join(config) => joins::join(&left, &right, config, plan.limits()),
        PreparedStep::Concat(config) => joins::concat(&left, &right, config, plan.limits()),
        PreparedStep::ConcatByName(config) => {
            joins::concat_by_name(&[&left, &right], config, plan.limits())
        }
        PreparedStep::CrossJoin(config) => joins::cross_join(&left, &right, config, plan.limits()),
        PreparedStep::TableDiff(config) => {
            reshape::table_diff(&left, &right, config, plan.limits())
        }
        PreparedStep::SemiJoin(config) => joins::semi_join(&left, &right, config),
        PreparedStep::AntiJoin(config) => joins::anti_join(&left, &right, config),
        PreparedStep::AsOfJoin(config) => joins::asof_join(&left, &right, config, plan.limits()),
        PreparedStep::FuzzyJoin(config) => fuzzy::fuzzy_join(&left, &right, config, plan.limits()),
        PreparedStep::UnionDistinct(config) => {
            setops::union_distinct(&left, &right, config, plan.limits())
        }
        PreparedStep::Intersect(config) => setops::intersect(&left, &right, config),
        PreparedStep::Except(config) => setops::except(&left, &right, config),
        PreparedStep::AssertForeignKey(config) => {
            governance::assert_foreign_key(&left, &right, config, plan.limits())
        }
        PreparedStep::Reconcile(config) => {
            governance::reconcile(&left, &right, config, plan.limits())
        }
        unary => Err(PlenoraError::Unsupported(unary.name().into())),
    }?;
    validate_batch(&output, plan.limits(), &mut names_validated)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::Int64Array;
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use serde_json::json;

    use super::*;
    use crate::table_engine::{Plan, Step, SCHEMA_VERSION};

    fn ids_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        RecordBatch::try_new(
            schema,
            vec![Arc::new(Int64Array::from(vec![3, 1, 2])) as ArrayRef],
        )
        .expect("batch di test")
    }

    fn decode_calls() -> usize {
        DECODE_CALLS.with(std::cell::Cell::get)
    }

    /// configurazioni preparate, hot path minimale: la config di un passo e' deserializzata una sola volta in
    /// `Plan::validate`; l'esecuzione di N batch non deve mai ri-parsare.
    #[test]
    fn config_deserialized_once_at_validation_never_per_batch() {
        let plan = Plan {
            schema_version: SCHEMA_VERSION,
            limits: Limits::default(),
            steps: vec![Step {
                operation: "sort".into(),
                config: json!({"columns": ["id"]}),
            }],
        }
        .validate()
        .expect("piano valido");

        // La validazione deserializza (contratto + forma preparata): il
        // contatore deve osservarlo, altrimenti il test non prova nulla.
        let at_prepare = decode_calls();
        assert!(
            at_prepare > 0,
            "atteso almeno un parse in validate, osservati {at_prepare}"
        );
        assert_eq!(plan.prepared_steps().len(), 1);
        assert_eq!(plan.prepared_steps()[0].name(), "sort");

        DECODE_CALLS.with(|calls| calls.set(0));
        let mut last_rows = 0;
        for _ in 0..1000 {
            let output = execute_batch(ids_batch(), &plan).expect("batch");
            last_rows = output.num_rows();
        }
        assert_eq!(last_rows, 3);
        assert_eq!(
            decode_calls(),
            0,
            "deserializzazioni nel percorso per batch"
        );
    }

    /// Anche il percorso binario usa la config preparata: nessun parse in
    /// `execute_binary`.
    #[test]
    fn binary_execution_does_not_reparse_config() {
        let plan = Plan {
            schema_version: SCHEMA_VERSION,
            limits: Limits::default(),
            steps: vec![Step {
                operation: "concat".into(),
                config: json!({}),
            }],
        }
        .validate()
        .expect("piano binario valido");
        assert!(plan.requires_secondary());

        DECODE_CALLS.with(|calls| calls.set(0));
        let output = execute_binary(&ids_batch(), &ids_batch(), &plan).expect("concat");
        assert_eq!(output.num_rows(), 6);
        assert_eq!(decode_calls(), 0, "deserializzazioni nel percorso binario");
    }
}
