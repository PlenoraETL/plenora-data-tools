//! Controlli statici delle config tabellari contro i limiti del piano.
//!
//! Porting di `validate_step_contract` (e dei suoi helper) di
//! `plenora-engine/src/table_engine/executor.rs` a `190c493`, applicato alla
//! config già tipizzata invece che al JSON. L'analisi dei contratti
//! (`analyze_table_contract`) non riceve i limiti del piano e non ripete tutti
//! questi rifiuti: senza, una config che il motore d'origine rifiutava
//! passerebbe con un significato diverso da quello scritto (un parametro
//! ignorato, un'asserzione vacua, righe duplicate). La compilazione di regex
//! e fusi orari non è ripetuta qui: la fa già l'analisi.
//!
//! Il posto giusto di questi controlli è l'analisi dei kernel; finché non ci
//! sono, il runner li applica qui, prima di ogni esecuzione.

use std::collections::HashSet;

use plenora_core::arrow::schema::{DataType, Field};
use plenora_core::contract::DataContract;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_table::{
    aggregation, cleansing, dates, filtering, fuzzy, reshape, strings, validate_output_name,
    validate_text_convertible, Limits,
};

use crate::dispatch::PassoPreparato;

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

fn max_rows_u64(limits: &Limits) -> Result<u64> {
    u64::try_from(limits.max_rows)
        .map_err(|_| PlenoraError::Internal("max_rows non rappresentabile in u64".to_owned()))
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

fn validate_keys(left: &[String], right: &[String], limits: &Limits, label: &str) -> Result<()> {
    validate_name_list(left, limits.max_columns, &format!("{label} left"), false)?;
    validate_name_list(right, limits.max_columns, &format!("{label} right"), false)?;
    if left.len() != right.len() {
        return Err(PlenoraError::InvalidPlan(format!(
            "{label}: cardinalita' chiavi diversa"
        )));
    }
    Ok(())
}

fn validate_quantile(aggregation: &aggregation::Aggregation) -> Result<()> {
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
    Ok(())
}

fn validate_date_formats(input: &str, output: Option<&str>, limits: &Limits) -> Result<()> {
    dates::validate_format(input, "input_format", limits.max_string_bytes)?;
    if let Some(output) = output {
        dates::validate_format(output, "output_format", limits.max_string_bytes)?;
    }
    Ok(())
}

impl PassoPreparato {
    /// Controlli statici della config contro i limiti, senza i dati.
    ///
    /// # Errors
    ///
    /// `InvalidPlan`: nomi, valori o limiti del passo non validi.
    #[allow(clippy::too_many_lines)] // Un braccio per operazione, come il dispatcher d'origine.
    pub fn verifica(&self, limits: &Limits) -> Result<()> {
        match self {
            Self::DropColumns(config) => {
                validate_name_list(&config.columns, limits.max_columns, "drop_columns", true)
            }
            Self::Rename(config) => {
                if config.renames.len() > limits.max_columns {
                    return Err(PlenoraError::InvalidPlan("troppe rinomine".into()));
                }
                let old: Vec<_> = config.renames.iter().map(|p| p.old_name.clone()).collect();
                let new: Vec<_> = config.renames.iter().map(|p| p.new_name.clone()).collect();
                validate_name_list(&old, limits.max_columns, "rename origine", true)?;
                validate_name_list(&new, limits.max_columns, "rename destinazione", true)
            }
            Self::ReorderColumns(config) => {
                validate_name_list(&config.columns, limits.max_columns, "reorder_columns", true)
            }
            Self::SelectColumns(config) => {
                validate_name_list(&config.columns, limits.max_columns, "select_columns", false)
            }
            Self::AlignSchema(config) => {
                let names: Vec<_> = config.columns.iter().map(|c| c.name.clone()).collect();
                validate_name_list(&names, limits.max_columns, "align_schema", false)
            }
            Self::ConcatColumns(config) => {
                validate_name_list(&config.columns, limits.max_columns, "concat_columns", false)?;
                validate_output_name(&config.output_column)?;
                if config.separator.len() > limits.max_string_bytes {
                    return Err(PlenoraError::InvalidPlan("separatore troppo grande".into()));
                }
                Ok(())
            }
            Self::SplitColumn(config) => {
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
            Self::StringPad(config) => validate_pad(config, limits),
            Self::StringLength(config) => {
                validate_output_name(&config.column)?;
                config
                    .output_column
                    .as_deref()
                    .map_or(Ok(()), validate_output_name)
            }
            Self::TextNormalize(config) => {
                validate_name_list(&config.columns, limits.max_columns, "text_normalize", false)
            }
            Self::FillNa(_)
            | Self::Sample(_)
            | Self::UnionDistinct(_)
            | Self::Intersect(_)
            | Self::Except(_)
            | Self::Concat(_)
            | Self::ConcatByName(_)
            | Self::CrossJoin(_) => Ok(()),
            Self::Replace(config) => {
                validate_output_name(&config.column)?;
                if config.old_value.len() > limits.max_regex_bytes
                    || config.new_value.len() > limits.max_string_bytes
                {
                    return Err(PlenoraError::InvalidPlan("replace oltre i limiti".into()));
                }
                Ok(())
            }
            Self::TypeCast(config) => validate_type_cast(config),
            Self::Filter(config) => validate_output_name(&config.column),
            Self::Conditional(config) => {
                validate_output_name(&config.column)?;
                validate_output_name(&config.output_column)?;
                if config.conditions.is_empty() || config.conditions.len() > limits.max_columns {
                    return Err(PlenoraError::InvalidPlan(
                        "numero condizioni non valido".into(),
                    ));
                }
                Ok(())
            }
            Self::StringExtract(config) => {
                validate_output_name(&config.column)?;
                if config.pattern.is_empty() || config.pattern.len() > limits.max_regex_bytes {
                    return Err(PlenoraError::InvalidPlan("pattern non valido".into()));
                }
                config
                    .output_column
                    .as_deref()
                    .map_or(Ok(()), validate_output_name)
            }
            Self::DateExtract(config) => {
                validate_output_name(&config.column)?;
                if config.date_format.as_ref().is_some_and(|format| {
                    format.is_empty() || format.len() > limits.max_string_bytes
                }) {
                    return Err(PlenoraError::InvalidPlan("date_format non valido".into()));
                }
                Ok(())
            }
            Self::UuidGenerator(config) => validate_output_name(&config.output_column),
            Self::Limit(config) => {
                let max_rows = max_rows_u64(limits)?;
                if config.n > max_rows || config.offset > max_rows {
                    return Err(PlenoraError::InvalidPlan(
                        "limit: n/offset oltre max_rows".into(),
                    ));
                }
                Ok(())
            }
            Self::Lookup(config) => {
                validate_output_name(&config.column)?;
                if config.mapping.len() > limits.max_rows {
                    return Err(PlenoraError::InvalidPlan("mapping oltre max_rows".into()));
                }
                config
                    .output_column
                    .as_deref()
                    .map_or(Ok(()), validate_output_name)
            }
            Self::FlattenJson(config) => {
                validate_output_name(&config.column)?;
                validate_name_list(
                    &config.output_columns,
                    limits.max_columns,
                    "flatten_json",
                    true,
                )
            }
            Self::MaskData(config) => {
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
            Self::Md5Hash(config) => {
                validate_name_list(&config.columns, limits.max_columns, "md5_hash", false)?;
                validate_output_name(&config.output_column)?;
                if config.null_literal.len() > limits.max_string_bytes {
                    return Err(PlenoraError::InvalidPlan(
                        "null_literal troppo grande".into(),
                    ));
                }
                Ok(())
            }
            Self::AddRowNumber(config) => {
                validate_output_name(&config.output_column)?;
                if let Some(column) = &config.partition_column {
                    validate_output_name(column)?;
                }
                if config.order_column.is_some() {
                    return Err(PlenoraError::InvalidPlan(
                        "add_row_number: order_column non ancora nel safe profile".into(),
                    ));
                }
                Ok(())
            }
            Self::Bin(config) => validate_output_name(&config.column),
            Self::Statistics(config) => validate_output_name(&config.column),
            Self::Sort(config) => {
                validate_name_list(&config.columns, limits.max_columns, "sort", false)
            }
            Self::TopN(config) => {
                validate_name_list(&config.columns, limits.max_columns, "top_n", false)?;
                if config.n > max_rows_u64(limits)? {
                    return Err(PlenoraError::InvalidPlan("top_n: n oltre max_rows".into()));
                }
                Ok(())
            }
            Self::Distinct(config) => {
                validate_name_list(&config.subset, limits.max_columns, "distinct", true)
            }
            Self::DedupAdvanced(config) => {
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
            Self::Aggregate(config) => {
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
                    validate_quantile(aggregation)?;
                }
                Ok(())
            }
            Self::WindowFunction(config) => {
                validate_output_name(&config.column)?;
                for name in [
                    &config.group_by,
                    &config.order_column,
                    &config.output_column,
                ]
                .into_iter()
                .flatten()
                {
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
            Self::RollingWindow(config) => {
                validate_output_name(&config.column)?;
                validate_output_name(&config.output_column)?;
                for name in [&config.group_by, &config.order_column]
                    .into_iter()
                    .flatten()
                {
                    validate_output_name(name)?;
                }
                if config.window > limits.max_rows {
                    return Err(PlenoraError::InvalidPlan(
                        "rolling_window: finestra oltre max_rows".into(),
                    ));
                }
                if config.window == 0
                    || config.min_periods == 0
                    || config.min_periods > config.window
                {
                    return Err(PlenoraError::InvalidPlan(
                        "rolling_window: finestra non valida".into(),
                    ));
                }
                Ok(())
            }
            Self::Melt(config) => {
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
            Self::Pivot(config) => {
                validate_output_name(&config.column)?;
                validate_output_name(&config.value_col)
            }
            Self::Transpose(config) => validate_name_list(
                &config.output_columns,
                limits.max_columns,
                "transpose",
                true,
            ),
            Self::Formula(config) => {
                plenora_kernels_table::formula::validate(config, limits.max_string_bytes)
            }
            Self::Expression(config) => plenora_kernels_table::expressions::validate(
                config,
                limits.max_columns.saturating_mul(16),
            ),
            Self::AssertCardinality(config) => {
                if config.exact_rows.is_none()
                    && config.min_rows.is_none()
                    && config.max_rows.is_none()
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
            Self::AssertMetadata(config) => {
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
            Self::AssertSchema(config) => {
                if config.fields.is_empty() || config.fields.len() > limits.max_columns {
                    return Err(PlenoraError::InvalidPlan(
                        "assert_schema: numero campi non valido".into(),
                    ));
                }
                let names: Vec<_> = config.fields.iter().map(|f| f.name.clone()).collect();
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
            Self::AssertNotNull(config) => validate_name_list(
                &config.columns,
                limits.max_columns,
                "assert_not_null",
                false,
            ),
            Self::AssertUnique(config) => {
                validate_name_list(&config.columns, limits.max_columns, "assert_unique", false)
            }
            Self::AssertRange(config) => {
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
            Self::AssertRegex(config) => {
                validate_output_name(&config.column)?;
                if config.pattern.is_empty() || config.pattern.len() > limits.max_regex_bytes {
                    return Err(PlenoraError::InvalidPlan(
                        "assert_regex: pattern non valido".into(),
                    ));
                }
                Ok(())
            }
            Self::Coalesce(config) => {
                validate_name_list(&config.columns, limits.max_columns, "coalesce", false)?;
                validate_output_name(&config.output_column)
            }
            Self::DateFormat(config) => {
                validate_output_name(&config.column)?;
                validate_date_formats(&config.input_format, Some(&config.output_format), limits)?;
                validate_output_name(&config.output_column)
            }
            Self::DateAdd(config) => {
                validate_output_name(&config.column)?;
                validate_date_formats(&config.input_format, Some(&config.output_format), limits)?;
                validate_output_name(&config.output_column)
            }
            Self::DateDiff(config) => {
                validate_output_name(&config.start_column)?;
                validate_output_name(&config.end_column)?;
                validate_date_formats(&config.input_format, None, limits)?;
                validate_output_name(&config.output_column)
            }
            Self::TimezoneConvert(config) => {
                validate_output_name(&config.column)?;
                validate_date_formats(&config.input_format, Some(&config.output_format), limits)?;
                validate_output_name(&config.output_column)
            }
            Self::Sha256Hash(config) => {
                validate_name_list(&config.columns, limits.max_columns, "sha256_hash", false)?;
                validate_output_name(&config.output_column)?;
                if config.null_literal.len() > limits.max_string_bytes {
                    return Err(PlenoraError::InvalidPlan(
                        "null_literal troppo grande".into(),
                    ));
                }
                Ok(())
            }
            Self::StableFingerprint(config) => {
                validate_name_list(
                    &config.columns,
                    limits.max_columns,
                    "stable_fingerprint",
                    true,
                )?;
                validate_output_name(&config.output_column)
            }
            Self::HmacSha256(config) => {
                validate_name_list(&config.columns, limits.max_columns, "hmac_sha256", false)?;
                validate_output_name(&config.output_column)?;
                if config.key_env.trim().is_empty() {
                    return Err(PlenoraError::InvalidPlan(
                        "hmac_sha256: key_env vuoto".into(),
                    ));
                }
                Ok(())
            }
            Self::ValidateRules(config) => {
                let names: Vec<_> = config.rules.iter().map(|rule| rule.name.clone()).collect();
                validate_name_list(&names, limits.max_columns, "validate_rules", false)
            }
            Self::Explode(config) => {
                validate_output_name(&config.column)?;
                config
                    .output_column
                    .as_deref()
                    .map_or(Ok(()), validate_output_name)
            }
            Self::Unnest(config) => {
                validate_output_name(&config.column)?;
                if config.prefix.len() > limits.max_string_bytes {
                    return Err(PlenoraError::InvalidPlan(
                        "unnest: prefisso troppo grande".into(),
                    ));
                }
                Ok(())
            }
            Self::Join(config) => {
                validate_name_list(&config.left_keys, limits.max_columns, "join left", false)?;
                validate_name_list(&config.right_keys, limits.max_columns, "join right", false)
            }
            Self::SemiJoin(config) | Self::AntiJoin(config) => validate_keys(
                &config.left_keys,
                &config.right_keys,
                limits,
                "membership join",
            ),
            Self::AsOfJoin(config) => {
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
            Self::FuzzyJoin(config) => {
                validate_output_name(&config.left_key)?;
                validate_output_name(&config.right_key)?;
                if let Some(name) = &config.score_column {
                    validate_output_name(name)?;
                }
                fuzzy::validate_config(config)
            }
            Self::AssertForeignKey(config) => {
                validate_keys(&config.left_keys, &config.right_keys, limits, "foreign key")
            }
            Self::Reconcile(config) => {
                validate_keys(&config.left_keys, &config.right_keys, limits, "reconcile")
            }
            Self::TableDiff(config) => {
                validate_name_list(&config.left_keys, limits.max_columns, "diff left", false)?;
                validate_name_list(&config.right_keys, limits.max_columns, "diff right", false)
            }
        }
    }
}

/// Campo di un contratto per nome; dopo l'analisi le colonne esistono, e
/// un'assenza qui è un difetto del runner.
fn campo<'a>(contratto: &'a DataContract, nome: &str) -> Result<&'a Field> {
    contratto
        .schema
        .field_with_name(nome)
        .map_err(|_| PlenoraError::Internal(format!("colonna `{nome}` assente dopo l'analisi")))
}

/// La colonna deve essere leggibile come scalare testuale: è il percorso che
/// il kernel prende, e un tipo che non lo è fallirebbe alla prima cella non
/// nulla.
fn testuale(contratto: &DataContract, nome: &str, ruolo: &str) -> Result<()> {
    let campo = campo(contratto, nome)?;
    validate_text_convertible(campo.data_type(), nome)
        .map_err(|errore| PlenoraError::InvalidPlan(format!("{ruolo}: {errore}")))
}

fn testuali<'a>(
    contratto: &DataContract,
    nomi: impl IntoIterator<Item = &'a String>,
    ruolo: &str,
) -> Result<()> {
    nomi.into_iter()
        .try_for_each(|nome| testuale(contratto, nome, ruolo))
}

/// Un formato d'uscita scritto da una data senza fuso: `%z`, `%Z` e simili
/// fanno fallire la scrittura, e il kernel va in panico alla prima riga.
/// Si prova qui, scrivendo una data fissa.
fn formato_senza_fuso(formato: &str, etichetta: &str) -> Result<()> {
    use std::fmt::Write as _;
    let data = chrono::NaiveDate::from_ymd_opt(2000, 1, 1)
        .and_then(|giorno| giorno.and_hms_opt(0, 0, 0))
        .ok_or_else(|| PlenoraError::Internal("data di prova non costruibile".to_owned()))?;
    let mut uscita = String::new();
    write!(uscita, "{}", data.format(formato)).map_err(|_| {
        PlenoraError::InvalidPlan(format!(
            "{etichetta} non scrivibile da una data senza fuso (per esempio %z, %Z)"
        ))
    })
}

/// Gli operatori che il kernel valuta come testo richiedono una colonna
/// leggibile come scalare testuale: `==`/`!=` fuori da Int64 e Float64,
/// `contains`, `startswith`, `endswith`. Gli operatori ordinati passano da
/// `scalar_compare`, senza un predicato pubblico: restano un limite
/// dichiarato.
fn verifica_operatore(
    contratto: &DataContract,
    colonna: &str,
    operatore: &filtering::Operator,
) -> Result<()> {
    let tipo = campo(contratto, colonna)?.data_type();
    let testo = match operatore {
        filtering::Operator::Eq | filtering::Operator::Ne => {
            !matches!(tipo, DataType::Int64 | DataType::Float64)
        }
        filtering::Operator::Contains
        | filtering::Operator::Startswith
        | filtering::Operator::Endswith => true,
        _ => false,
    };
    if testo {
        testuale(contratto, colonna, "confronto testuale")?;
    }
    Ok(())
}

impl PassoPreparato {
    /// Controlli che dipendono dagli schemi d'ingresso e dalla config, fatti
    /// dopo l'analisi: condizioni per cui il kernel fallirebbe comunque, su
    /// qualunque dato non vuoto. Regola del runner: tutto ciò che schemi e
    /// config rendono prevedibile fallisce in validazione, mai dopo che
    /// qualche passo ha girato.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` o `Unsupported` per la condizione trovata;
    /// `ResourceLimit` per colonne oltre il limite; `Internal` se una colonna
    /// validata manca.
    #[allow(clippy::too_many_lines)] // Un braccio per famiglia di controllo.
    pub fn verifica_con_schema(&self, ingressi: &[DataContract], limits: &Limits) -> Result<()> {
        let (primo, secondo) = match ingressi {
            [primo] => (primo, None),
            [primo, secondo] => (primo, Some(secondo)),
            _ => {
                return Err(PlenoraError::Internal(
                    "arieta' diversa da quella validata".to_owned(),
                ))
            }
        };
        let destro = || {
            secondo.ok_or_else(|| {
                PlenoraError::Internal("passo binario senza secondo input".to_owned())
            })
        };
        match self {
            Self::Explode(config) => {
                if matches!(config.empty_policy, reshape::EmptyListPolicy::Drop) {
                    return Err(PlenoraError::InvalidPlan(
                        "explode.empty_policy=drop non e' ammessa: usare un passo esplicito di \
                         selezione"
                            .to_owned(),
                    ));
                }
                let in_place = config
                    .output_column
                    .as_deref()
                    .is_none_or(|uscita| uscita == config.column);
                if in_place && !campo(primo, &config.column)?.is_nullable() {
                    // Difetto del kernel (`select_rows_except` lascia un
                    // segnaposto nullo sotto lo schema d'ingresso): fallirebbe
                    // su ogni input non vuoto.
                    return Err(PlenoraError::Unsupported(
                        "explode in place su una colonna List non nullabile: usare un \
                         output_column diverso"
                            .to_owned(),
                    ));
                }
                Ok(())
            }
            Self::DateExtract(config) => config.date_format.as_deref().map_or(Ok(()), |formato| {
                dates::validate_format(formato, "date_format", limits.max_string_bytes)
            }),
            Self::TypeCast(config) => {
                let usa_il_formato = matches!(
                    config.target_type,
                    cleansing::TargetType::Date
                        | cleansing::TargetType::Datetime
                        | cleansing::TargetType::Date32
                        | cleansing::TargetType::TimestampMillis
                );
                if config.date_format.is_empty() {
                    Ok(())
                } else if usa_il_formato {
                    dates::validate_format(
                        &config.date_format,
                        "date_format",
                        limits.max_string_bytes,
                    )
                } else {
                    Err(PlenoraError::InvalidPlan(
                        "date_format ammesso solo per i target data e timestamp".to_owned(),
                    ))
                }
            }
            Self::DateFormat(config) => formato_senza_fuso(&config.output_format, "output_format"),
            Self::DateAdd(config) => formato_senza_fuso(&config.output_format, "output_format"),
            Self::Aggregate(config) => testuali(primo, &config.group_by, "aggregate.group_by"),
            Self::WindowFunction(config) => {
                testuali(primo, &config.group_by, "window_function.group_by")
            }
            Self::RollingWindow(config) => {
                testuali(primo, &config.group_by, "rolling_window.group_by")
            }
            Self::Statistics(config) => testuali(primo, &config.group_by, "statistics.group_by"),
            Self::Distinct(config) => {
                if config.subset.is_empty() {
                    // Senza subset la chiave e' la riga intera.
                    let tutte: Vec<String> = primo
                        .schema
                        .fields()
                        .iter()
                        .map(|f| f.name().clone())
                        .collect();
                    testuali(primo, &tutte, "distinct")
                } else {
                    testuali(primo, &config.subset, "distinct.subset")
                }
            }
            Self::Filter(config) => verifica_operatore(primo, &config.column, &config.operator),
            Self::Conditional(config) => config.conditions.iter().try_for_each(|condizione| {
                verifica_operatore(primo, &config.column, &condizione.operator)
            }),
            Self::Join(config) => {
                testuali(primo, &config.left_keys, "join.left_keys")?;
                testuali(destro()?, &config.right_keys, "join.right_keys")
            }
            Self::SemiJoin(config) | Self::AntiJoin(config) => {
                testuali(primo, &config.left_keys, "left_keys")?;
                testuali(destro()?, &config.right_keys, "right_keys")
            }
            Self::AsOfJoin(config) => {
                testuali(primo, &config.left_by, "asof_join.left_by")?;
                testuali(destro()?, &config.right_by, "asof_join.right_by")
            }
            Self::TableDiff(config) => {
                testuali(primo, &config.left_keys, "table_diff.left_keys")?;
                testuali(destro()?, &config.right_keys, "table_diff.right_keys")?;
                testuali(primo, &config.compare_columns, "table_diff.compare_columns")?;
                testuali(
                    destro()?,
                    &config.compare_columns,
                    "table_diff.compare_columns",
                )
            }
            Self::AssertForeignKey(config) => {
                testuali(primo, &config.left_keys, "assert_foreign_key.left_keys")?;
                testuali(
                    destro()?,
                    &config.right_keys,
                    "assert_foreign_key.right_keys",
                )
            }
            Self::Reconcile(config) => {
                testuali(primo, &config.left_keys, "reconcile.left_keys")?;
                testuali(destro()?, &config.right_keys, "reconcile.right_keys")
            }
            Self::StableFingerprint(config) => {
                if config.columns.is_empty() && primo.schema.fields().is_empty() {
                    return Err(PlenoraError::InvalidPlan(
                        "stable_fingerprint richiede almeno una colonna".to_owned(),
                    ));
                }
                Ok(())
            }
            Self::FlattenJson(config) => {
                let colonne = primo
                    .schema
                    .fields()
                    .len()
                    .saturating_add(config.output_columns.len());
                if colonne > limits.max_columns {
                    return Err(PlenoraError::ResourceLimit(
                        "flatten_json supera max_columns".to_owned(),
                    ));
                }
                Ok(())
            }
            Self::HmacSha256(config) => {
                // Dipende dall'ambiente, non dal piano: si controlla qui
                // perche' l'errore non arrivi dopo i passi a monte. La
                // variabile puo' ancora cambiare fra validazione ed
                // esecuzione (README, «Runner»).
                match std::env::var_os(&config.key_env) {
                    Some(valore) if !valore.is_empty() => Ok(()),
                    _ => Err(PlenoraError::InvalidPlan(
                        "hmac_sha256: chiave HMAC non disponibile".to_owned(),
                    )),
                }
            }
            _ => Ok(()),
        }
    }
}
