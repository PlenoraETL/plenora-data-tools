//! Executor tabellare: validazione delle config per singolo passo ed
//! esecuzione della catena (port da `plenora-nogeo-tools/src/engine.rs`).

use std::collections::HashSet;

use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, LargeStringArray, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Schema};
use plenora_core::{PlenoraError, Result};
use serde::de::DeserializeOwned;

use plenora_kernels_table::validate_output_name;
use plenora_kernels_table::{
    aggregation, analysis, cleansing, columns, dates, expressions, filtering, formula, governance,
    joins, quality, reshape, security, setops, spill, strings, utility,
};

use super::contract::{dispatch_name, Step, ValidatedPlan};
use super::Limits;

fn decode<T: DeserializeOwned>(step: &Step) -> Result<T> {
    serde_json::from_value(step.config.clone()).map_err(PlenoraError::from)
}

fn validate_name_list(names: &[String], max: usize, label: &str, allow_empty: bool) -> Result<()> {
    if (!allow_empty && names.is_empty()) || names.len() > max {
        return Err(PlenoraError::Contract(format!(
            "numero colonne {label} non valido"
        )));
    }
    let unique: HashSet<_> = names.iter().collect();
    if unique.len() != names.len() {
        return Err(PlenoraError::Contract(format!(
            "{label} contiene nomi duplicati"
        )));
    }
    names.iter().try_for_each(|name| validate_output_name(name))
}

fn validate_rename(config: &columns::Rename, limits: &Limits) -> Result<()> {
    if config.renames.len() > limits.max_columns {
        return Err(PlenoraError::Contract("troppe rinomine".into()));
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
        return Err(PlenoraError::Contract("delimiter non valido".into()));
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
        return Err(PlenoraError::Contract(
            "fill_char deve essere un carattere Unicode".into(),
        ));
    }
    if config.width > limits.max_string_bytes {
        return Err(PlenoraError::Contract("width oltre il limite".into()));
    }
    Ok(())
}

fn validate_type_cast(config: &cleansing::TypeCast) -> Result<()> {
    validate_output_name(&config.column)?;
    match config.target_type {
        cleansing::TargetType::Decimal128 => {
            let precision = config
                .precision
                .ok_or_else(|| PlenoraError::Contract("decimal128 richiede precision".into()))?;
            let scale = config
                .scale
                .ok_or_else(|| PlenoraError::Contract("decimal128 richiede scale".into()))?;
            if !(1..=38).contains(&precision) || scale < 0 || scale > precision.cast_signed() {
                return Err(PlenoraError::Contract(
                    "decimal128 richiede 1 <= precision <= 38 e 0 <= scale <= precision".into(),
                ));
            }
            if config.timezone.is_some() {
                return Err(PlenoraError::Contract(
                    "timezone non ammessa per decimal128".into(),
                ));
            }
        }
        cleansing::TargetType::TimestampMillis => {
            if config.precision.is_some() || config.scale.is_some() {
                return Err(PlenoraError::Contract(
                    "precision/scale non ammessi per timestamp".into(),
                ));
            }
            if let Some(timezone) = &config.timezone {
                timezone
                    .parse::<chrono_tz::Tz>()
                    .map_err(|_| PlenoraError::Contract("timezone cast non valida".into()))?;
            }
        }
        _ if config.precision.is_some() || config.scale.is_some() || config.timezone.is_some() => {
            return Err(PlenoraError::Contract(
                "precision, scale e timezone non ammessi per questo target_type".into(),
            ));
        }
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Exhaustive contract dispatcher kept in one audited match.
pub(crate) fn validate_step_contract(step: &Step, limits: &Limits) -> Result<()> {
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
        "concat_columns" => {
            let config = decode::<columns::ConcatColumns>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "concat_columns", false)?;
            validate_output_name(&config.output_column)?;
            if config.separator.len() > limits.max_string_bytes {
                return Err(PlenoraError::Contract("separatore troppo grande".into()));
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
                return Err(PlenoraError::Contract("replace oltre i limiti".into()));
            }
            if config.regex {
                regex::Regex::new(&config.old_value)
                    .map_err(|e| PlenoraError::Contract(format!("regex non valida: {e}")))?;
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
                return Err(PlenoraError::Contract("numero condizioni non valido".into()));
            }
            Ok(())
        }
        "string_extract" => {
            let config = decode::<strings::StringExtract>(step)?;
            validate_output_name(&config.column)?;
            if config.pattern.is_empty() || config.pattern.len() > limits.max_regex_bytes {
                return Err(PlenoraError::Contract("pattern non valido".into()));
            }
            regex::Regex::new(&config.pattern)
                .map_err(|e| PlenoraError::Contract(format!("regex non valida: {e}")))?;
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
                return Err(PlenoraError::Contract("date_format non valido".into()));
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
                return Err(PlenoraError::Contract(
                    "limit: n/offset oltre max_rows".into(),
                ));
            }
            Ok(())
        }
        "lookup" => {
            let config = decode::<analysis::Lookup>(step)?;
            validate_output_name(&config.column)?;
            if config.mapping.len() > limits.max_rows {
                return Err(PlenoraError::Contract("mapping oltre max_rows".into()));
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
                return Err(PlenoraError::Contract("numero masking non valido".into()));
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
                return Err(PlenoraError::Contract("null_literal troppo grande".into()));
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
                return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract("top_n: n oltre max_rows".into()));
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
                return Err(PlenoraError::Contract(
                    "dedup_advanced: ascending richiede order_column".into(),
                ));
            }
            Ok(())
        }
        "aggregate" => {
            let config = decode::<aggregation::Aggregate>(step)?;
            validate_name_list(&config.group_by, limits.max_columns, "aggregate", false)?;
            if config.aggregations.len() > limits.max_columns {
                return Err(PlenoraError::Contract("troppe aggregazioni".into()));
            }
            for aggregation in &config.aggregations {
                validate_output_name(&aggregation.column)?;
                if !aggregation.alias.is_empty() {
                    validate_output_name(&aggregation.alias)?;
                }
                if aggregation.separator.len() > limits.max_string_bytes {
                    return Err(PlenoraError::Contract(
                        "separatore aggregazione troppo grande".into(),
                    ));
                }
                if matches!(aggregation.function, aggregation::AggFunction::Quantile) {
                    if !aggregation
                        .quantile
                        .is_some_and(|value| value.is_finite() && (0.0..=1.0).contains(&value))
                    {
                        return Err(PlenoraError::Contract(
                            "quantile deve essere compreso tra 0 e 1".into(),
                        ));
                    }
                } else if aggregation.quantile.is_some() {
                    return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract("offset deve essere positivo".into()));
            }
            if matches!(config.function, aggregation::WindowKind::Ntile) {
                if !config
                    .buckets
                    .is_some_and(|buckets| buckets > 0 && buckets <= limits.max_rows)
                {
                    return Err(PlenoraError::Contract(
                        "ntile richiede buckets valido".into(),
                    ));
                }
            } else if config.buckets.is_some() {
                return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract(
                    "rolling_window: finestra oltre max_rows".into(),
                ));
            }
            if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
                return Err(PlenoraError::Contract(
                    "rolling_window: finestra non valida".into(),
                ));
            }
            Ok(())
        }
        "melt" => {
            let config = decode::<reshape::Melt>(step)?;
            if config.var_name == config.value_name {
                return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract(
                    "assert_cardinality richiede exact_rows, min_rows o max_rows".into(),
                ));
            }
            if config.exact_rows.is_some()
                && (config.min_rows.is_some() || config.max_rows.is_some())
            {
                return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract(
                    "assert_cardinality: limiti non validi".into(),
                ));
            }
            Ok(())
        }
        "assert_metadata" => {
            let config = decode::<governance::AssertMetadata>(step)?;
            if config.expected.is_empty() || config.expected.len() > limits.max_columns {
                return Err(PlenoraError::Contract(
                    "assert_metadata: numero elementi non valido".into(),
                ));
            }
            if config.expected.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > limits.max_string_bytes
                    || value.len() > limits.max_string_bytes
            }) {
                return Err(PlenoraError::Contract(
                    "assert_metadata: chiave o valore oltre i limiti".into(),
                ));
            }
            Ok(())
        }
        "assert_schema" => {
            let config = decode::<quality::AssertSchema>(step)?;
            if config.fields.is_empty() || config.fields.len() > limits.max_columns {
                return Err(PlenoraError::Contract(
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
                    return Err(PlenoraError::Contract(format!(
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
                return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract(
                    "assert_range: estremi non validi".into(),
                ));
            }
            Ok(())
        }
        "assert_regex" => {
            let config = decode::<quality::AssertRegex>(step)?;
            validate_output_name(&config.column)?;
            if config.pattern.is_empty() || config.pattern.len() > limits.max_regex_bytes {
                return Err(PlenoraError::Contract(
                    "assert_regex: pattern non valido".into(),
                ));
            }
            regex::Regex::new(&config.pattern)
                .map_err(|error| PlenoraError::Contract(format!("regex non valida: {error}")))?;
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
                .map_err(|_| PlenoraError::Contract("source_timezone non valida".into()))?;
            config
                .target_timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| PlenoraError::Contract("target_timezone non valida".into()))?;
            validate_output_name(&config.output_column)
        }
        "sha256_hash" => {
            let config = decode::<security::Sha256Hash>(step)?;
            validate_name_list(&config.columns, limits.max_columns, "sha256_hash", false)?;
            validate_output_name(&config.output_column)?;
            if config.null_literal.len() > limits.max_string_bytes {
                return Err(PlenoraError::Contract("null_literal troppo grande".into()));
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
                return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract(
                    "asof_join: configurazione non valida".into(),
                ));
            }
            Ok(())
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
                return Err(PlenoraError::Contract(
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
                return Err(PlenoraError::Contract(
                    "reconcile: cardinalita' chiavi diversa".into(),
                ));
            }
            Ok(())
        }
        "union_distinct" | "intersect" | "except" => {
            decode::<setops::SetOperation>(step)?;
            Ok(())
        }
        "concat" => {
            decode::<joins::Concat>(step)?;
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

fn validate_batch(batch: &RecordBatch, limits: &Limits) -> Result<()> {
    if batch.num_rows() > limits.max_rows {
        return Err(PlenoraError::Contract(format!(
            "batch con {} righe oltre il limite {}",
            batch.num_rows(),
            limits.max_rows
        )));
    }
    if batch.num_columns() > limits.max_columns {
        return Err(PlenoraError::Contract(format!(
            "schema con {} colonne oltre il limite {}",
            batch.num_columns(),
            limits.max_columns
        )));
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
    Ok(())
}

fn normalize_large_utf8(batch: &RecordBatch) -> Result<RecordBatch> {
    let has_large_utf8 = batch
        .schema()
        .fields()
        .iter()
        .any(|field| field.data_type() == &DataType::LargeUtf8);
    let mut metadata = batch.schema().metadata().clone();
    // Pandas stores a second, independent dtype/name schema in this opaque
    // entry. Any transform can make it stale, causing PyArrow to reinterpret
    // correct physical Arrow columns on read. Physical Arrow fields are the
    // engine contract; retain application metadata but drop this cache.
    let removed_pandas_metadata = metadata.remove("pandas").is_some();
    if !has_large_utf8 && !removed_pandas_metadata {
        return Ok(batch.clone());
    }
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
                    PlenoraError::Contract("overflow dimensione colonna LargeUtf8".into())
                })
            })?;
            if bytes > i32::MAX as usize {
                return Err(PlenoraError::Contract(
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
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        columns,
    )?)
}

fn execute_step(batch: &RecordBatch, step: &Step, limits: &Limits) -> Result<RecordBatch> {
    match dispatch_name(&step.operation) {
        "drop_columns" => columns::drop_columns(batch, &decode(step)?),
        "rename" => columns::rename(batch, &decode(step)?),
        "reorder_columns" => columns::reorder_columns(batch, &decode(step)?),
        "select_columns" => columns::select_columns(batch, &decode(step)?),
        "concat_columns" => columns::concat_columns(batch, &decode(step)?, limits),
        "split_column" => columns::split_column(batch, &decode(step)?, limits),
        "string_pad" => strings::string_pad(batch, &decode(step)?, limits),
        "string_length" => strings::string_length(batch, &decode(step)?),
        "text_normalize" => strings::text_normalize(batch, &decode(step)?, limits),
        "fill_na" => cleansing::fill_na(batch, &decode(step)?),
        "replace" => cleansing::replace(batch, &decode(step)?),
        "type_cast" => cleansing::type_cast(batch, &decode(step)?),
        "filter" => filtering::filter(batch, &decode(step)?),
        "conditional" => filtering::conditional(batch, &decode(step)?),
        "string_extract" => strings::string_extract(batch, &decode(step)?, limits),
        "date_extract" => utility::date_extract(batch, &decode(step)?),
        "uuid_generator" => utility::uuid_generator(batch, &decode(step)?),
        "limit" => utility::limit(batch, &decode(step)?),
        "lookup" => analysis::lookup(batch, &decode(step)?),
        "flatten_json" => analysis::flatten_json(batch, &decode(step)?, limits),
        "mask_data" => security::mask_data(batch, &decode(step)?),
        "md5_hash" => security::md5_hash(batch, &decode(step)?),
        "add_row_number" => utility::add_row_number(batch, &decode(step)?),
        "bin" => analysis::bin(batch, &decode(step)?),
        "sample" => analysis::sample(batch, &decode(step)?),
        "statistics" => analysis::statistics(batch, &decode(step)?),
        "sort" => aggregation::sort(batch, &decode(step)?),
        "top_n" => aggregation::top_n(batch, &decode(step)?),
        "distinct" => aggregation::distinct(batch, &decode(step)?),
        "dedup_advanced" => aggregation::dedup_advanced(batch, &decode(step)?),
        "aggregate" => aggregation::aggregate(batch, &decode(step)?),
        "window_function" => aggregation::window_function(batch, &decode(step)?),
        "rolling_window" => aggregation::rolling_window(batch, &decode(step)?),
        "melt" => reshape::melt(batch, &decode(step)?, limits),
        "pivot" => reshape::pivot(batch, &decode(step)?, limits),
        "transpose" => reshape::transpose(batch, &decode(step)?, limits),
        "formula" => formula::formula(batch, &decode(step)?),
        "expression" => expressions::expression(batch, &decode(step)?),
        "assert_cardinality" => governance::assert_cardinality(batch, &decode(step)?),
        "assert_metadata" => governance::assert_metadata(batch, &decode(step)?),
        "assert_schema" => quality::assert_schema(batch, &decode(step)?),
        "assert_not_null" => quality::assert_not_null(batch, &decode(step)?),
        "assert_unique" => quality::assert_unique(batch, &decode(step)?),
        "assert_range" => quality::assert_range(batch, &decode(step)?),
        "assert_regex" => quality::assert_regex(batch, &decode(step)?),
        "coalesce" => quality::coalesce(batch, &decode(step)?),
        "date_format" => dates::date_format(batch, &decode(step)?),
        "date_add" => dates::date_add(batch, &decode(step)?),
        "date_diff" => dates::date_diff(batch, &decode(step)?),
        "timezone_convert" => dates::timezone_convert(batch, &decode(step)?),
        "sha256_hash" => security::sha256_hash(batch, &decode(step)?),
        "stable_fingerprint" => security::stable_fingerprint(batch, &decode(step)?),
        "explode" => reshape::explode(batch, &decode(step)?, limits),
        "unnest" => reshape::unnest(batch, &decode(step)?, limits),
        operation => Err(PlenoraError::Unsupported(operation.into())),
    }
}

/// Esegue una catena gia' validata su una singola batch Arrow.
///
/// # Errors
///
/// Restituisce un errore contestualizzato col passo se schema, limiti o kernel
/// non possono garantire un risultato deterministico.
pub fn execute_batch(mut batch: RecordBatch, plan: &ValidatedPlan) -> Result<RecordBatch> {
    if plan.requires_secondary() {
        return Err(PlenoraError::Contract(
            "il piano richiede un secondo input".into(),
        ));
    }
    batch = normalize_large_utf8(&batch)?;
    validate_batch(&batch, plan.limits())?;
    for (index, step) in plan.steps().iter().enumerate() {
        batch = execute_step(&batch, step, plan.limits()).map_err(|error| PlenoraError::Step {
            node: index.to_string(),
            operation: step.operation.clone(),
            reason: error.to_string(),
        })?;
        validate_batch(&batch, plan.limits()).map_err(|error| PlenoraError::Step {
            node: index.to_string(),
            operation: step.operation.clone(),
            reason: error.to_string(),
        })?;
    }
    Ok(batch)
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
        return Err(PlenoraError::Contract("piano binario non valido".into()));
    }
    let left = normalize_large_utf8(left)?;
    let right = normalize_large_utf8(right)?;
    validate_batch(&left, plan.limits())?;
    validate_batch(&right, plan.limits())?;
    let step = &plan.steps()[0];
    if matches!(
        dispatch_name(&step.operation),
        "union_distinct" | "intersect" | "except"
    ) && spill::should_spill(&left, &right, plan.limits())
    {
        let output = spill::execute_set_operation(
            dispatch_name(&step.operation),
            &left,
            &right,
            plan.limits(),
        )?;
        validate_batch(&output, plan.limits())?;
        return Ok(output);
    }
    let output = match dispatch_name(&step.operation) {
        "join" => joins::join(&left, &right, &decode(step)?, plan.limits()),
        "concat" => joins::concat(&left, &right, &decode(step)?, plan.limits()),
        "cross_join" => joins::cross_join(&left, &right, &decode(step)?, plan.limits()),
        "table_diff" => reshape::table_diff(&left, &right, &decode(step)?, plan.limits()),
        "semi_join" => joins::semi_join(&left, &right, &decode(step)?),
        "anti_join" => joins::anti_join(&left, &right, &decode(step)?),
        "asof_join" => joins::asof_join(&left, &right, &decode(step)?, plan.limits()),
        "union_distinct" => setops::union_distinct(&left, &right, &decode(step)?, plan.limits()),
        "intersect" => setops::intersect(&left, &right, &decode(step)?),
        "except" => setops::except(&left, &right, &decode(step)?),
        "assert_foreign_key" => {
            governance::assert_foreign_key(&left, &right, &decode(step)?, plan.limits())
        }
        "reconcile" => governance::reconcile(&left, &right, &decode(step)?, plan.limits()),
        operation => Err(PlenoraError::Unsupported(operation.into())),
    }?;
    validate_batch(&output, plan.limits())?;
    Ok(output)
}
