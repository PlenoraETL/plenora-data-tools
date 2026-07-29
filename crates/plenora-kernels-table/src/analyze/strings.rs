//! Analyzer a secco delle op su stringhe e sicurezza
//! (kernel `strings.rs` / `security.rs`).

use std::collections::HashSet;

use plenora_core::arrow::schema::DataType;
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    analyze_append, check_output_name, contract_error, is_scalar_string, require_scalar_string,
    require_utf8, typed,
};
use crate::{security, strings, Limits};

// ---------------------------------------------------------------------------
// strings.rs / security.rs
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn analyze_string_pad(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: strings::StringPad = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    if config.fill_char.chars().count() != 1 {
        return contract_error(op, "fill_char deve essere un singolo carattere");
    }
    let name = config.output_column.unwrap_or(config.column);
    check_output_name(op, &name)?;
    analyze_append(input, fields, &[(name, DataType::Utf8, true)])
}

pub(in crate::analyze) fn analyze_string_length(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: strings::StringLength = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    let name = config
        .output_column
        .unwrap_or_else(|| format!("{}_length", config.column));
    check_output_name(op, &name)?;
    analyze_append(input, fields, &[(name, DataType::Int64, true)])
}

pub(in crate::analyze) fn analyze_string_extract(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: strings::StringExtract = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    if config.pattern.len() > Limits::default().max_regex_bytes {
        return contract_error(op, "pattern oltre max_regex_bytes");
    }
    let pattern = regex::Regex::new(&config.pattern)
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: regex non valida: {error}")))?;
    let named: Vec<String> = pattern
        .capture_names()
        .flatten()
        .map(str::to_owned)
        .collect();
    let produced: Vec<(String, DataType, bool)> = if named.is_empty() {
        let name = config
            .output_column
            .unwrap_or_else(|| format!("{}_extracted", config.column));
        vec![(name, DataType::Utf8, true)]
    } else {
        // Con gruppi con nome: una colonna per gruppo; output_column ignorato.
        named.into_iter().map(|n| (n, DataType::Utf8, true)).collect()
    };
    for (name, _, _) in &produced {
        check_output_name(op, name)?;
    }
    analyze_append(input, fields, &produced)
}

pub(in crate::analyze) fn analyze_text_normalize(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: strings::TextNormalize = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    let mut produced = Vec::with_capacity(config.columns.len());
    for name in &config.columns {
        require_utf8(op, input, name)?;
        let output = if config.overwrite {
            name.clone()
        } else {
            format!("{name}_norm")
        };
        check_output_name(op, &output)?;
        produced.push((output, DataType::Utf8, true));
    }
    analyze_append(input, fields, &produced)
}

pub(in crate::analyze) fn analyze_md5_hash(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::Md5Hash = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    for name in &config.columns {
        require_scalar_string(op, input, name)?;
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, false)],
    )
}

pub(in crate::analyze) fn analyze_sha256_hash(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::Sha256Hash = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    // A differenza di md5_hash il kernel non vieta columns vuoto.
    for name in &config.columns {
        require_scalar_string(op, input, name)?;
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, false)],
    )
}

pub(in crate::analyze) fn analyze_stable_fingerprint(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::StableFingerprint = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.columns.is_empty() {
        // Default: tutte le colonne dello schema; il kernel legge ogni valore
        // via `scalar_as_string`, quindi i tipi fuori profilo (List, Struct)
        // fallirebbero a runtime: fail-closed gia' in validazione.
        for field in input.schema.fields() {
            if !is_scalar_string(field.data_type()) {
                return contract_error(
                    op,
                    format!(
                        "colonna {}: tipo {:?} non leggibile come scalare testuale",
                        field.name(),
                        field.data_type()
                    ),
                );
            }
        }
    } else {
        let mut seen: HashSet<&str> = HashSet::new();
        for name in &config.columns {
            if !seen.insert(name.as_str()) {
                return contract_error(op, format!("colonna ripetuta in columns: {name}"));
            }
            require_scalar_string(op, input, name)?;
        }
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, false)],
    )
}

pub(in crate::analyze) fn analyze_hmac_sha256(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::HmacSha256 = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.key_env.trim().is_empty() {
        return contract_error(op, "key_env vuoto");
    }
    // La chiave NON e' mai letta in analisi: nel contratto passa solo il nome
    // della variabile d'ambiente, mai il valore.
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("colonna ripetuta in columns: {name}"));
        }
        require_scalar_string(op, input, name)?;
    }
    let nullable = matches!(config.null_policy, security::HmacNullPolicy::Null);
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, nullable)],
    )
}

pub(in crate::analyze) fn analyze_mask_data(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::MaskData = typed(op, config)?;
    let input = &inputs[0];
    if config.maskings.is_empty() {
        return contract_error(op, "maskings vuoto");
    }
    // Le masking sono applicate in sequenza: una masking puo' riferirsi a una
    // colonna `_masked` creata da una precedente.
    let mut available: HashSet<String> = input
        .schema
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();
    let mut produced = Vec::with_capacity(config.maskings.len());
    for masking in &config.maskings {
        if !available.contains(&masking.column) {
            return contract_error(op, format!("colonna non trovata: {}", masking.column));
        }
        require_scalar_string(op, input, &masking.column)?;
        if matches!(masking.mask_type, security::MaskType::Custom)
            && masking.mask_char.chars().count() != 1
        {
            return contract_error(op, "mask_char deve essere un singolo carattere");
        }
        let output = if config.overwrite {
            masking.column.clone()
        } else {
            format!("{}_masked", masking.column)
        };
        check_output_name(op, &output)?;
        available.insert(output.clone());
        produced.push((output, DataType::Utf8, true));
    }
    analyze_append(input, fields, &produced)
}

