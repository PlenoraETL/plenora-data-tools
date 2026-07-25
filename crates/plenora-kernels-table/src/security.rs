use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

use plenora_core::arrow::array::{builder::StringBuilder, Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use md5::{Digest, Md5};
use serde::Deserialize;
use sha2::Sha256;

use plenora_core::{PlenoraError, Result};
use crate::{column_index, replace_or_append, scalar_as_string, validate_output_name};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Md5Hash {
    pub columns: Vec<String>,
    #[serde(default = "default_hash_name")]
    pub output_column: String,
    #[serde(default = "default_true")]
    pub normalize: bool,
    #[serde(default = "default_null_policy")]
    pub null_policy: HashNullPolicy,
    #[serde(default = "default_null_literal")]
    pub null_literal: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HashNullPolicy {
    Empty,
    Literal,
    Error,
}

const fn default_null_policy() -> HashNullPolicy {
    HashNullPolicy::Empty
}

fn default_null_literal() -> String {
    "<null>".into()
}

fn default_hash_name() -> String {
    "md5_hash".into()
}
const fn default_true() -> bool {
    true
}

pub fn md5_hash(batch: &RecordBatch, config: &Md5Hash) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    if config.columns.is_empty() {
        return Err(PlenoraError::Contract("md5_hash richiede colonne".into()));
    }
    let mut columns = config.columns.clone();
    columns.sort();
    columns.dedup();
    let indices = columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let values = (0..batch.num_rows())
        .map(|row| {
            let parts = indices
                .iter()
                .map(|index| {
                    let value = scalar_as_string(batch.column(*index).as_ref(), row)?;
                    let value = match (value, &config.null_policy) {
                        (Some(value), _) => value,
                        (None, HashNullPolicy::Empty) => String::new(),
                        (None, HashNullPolicy::Literal) => config.null_literal.clone(),
                        (None, HashNullPolicy::Error) => {
                            return Err(PlenoraError::Contract(format!(
                                "md5_hash: null alla riga {row}"
                            )))
                        }
                    };
                    Ok(if config.normalize {
                        value.trim().to_lowercase()
                    } else {
                        value
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let mut digest = Md5::new();
            digest.update(parts.join("\u{1f}").as_bytes());
            Ok(format!("{:x}", digest.finalize()))
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(StringArray::from(values)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sha256Hash {
    pub columns: Vec<String>,
    #[serde(default = "default_sha256_name")]
    pub output_column: String,
    #[serde(default = "default_true")]
    pub normalize: bool,
    #[serde(default = "default_null_policy")]
    pub null_policy: HashNullPolicy,
    #[serde(default = "default_null_literal")]
    pub null_literal: String,
}

fn default_sha256_name() -> String {
    "sha256_hash".into()
}

fn framed_part(digest: &mut Sha256, value: &[u8]) -> Result<()> {
    let length = u64::try_from(value.len())
        .map_err(|_| PlenoraError::Contract("sha256_hash: valore troppo grande".into()))?;
    digest.update(length.to_be_bytes());
    digest.update(value);
    Ok(())
}

pub fn sha256_hash(batch: &RecordBatch, config: &Sha256Hash) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    let mut names = config.columns.clone();
    names.sort();
    let indices = names
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let values = (0..batch.num_rows())
        .map(|row| {
            let mut digest = Sha256::new();
            digest.update(b"plenora-sha256-v1\0");
            for (name, index) in names.iter().zip(&indices) {
                framed_part(&mut digest, name.as_bytes())?;
                framed_part(
                    &mut digest,
                    batch.column(*index).data_type().to_string().as_bytes(),
                )?;
                let value = scalar_as_string(batch.column(*index).as_ref(), row)?;
                match (value, &config.null_policy) {
                    (Some(value), _) => {
                        digest.update([1]);
                        let value = if config.normalize {
                            value.trim().to_lowercase()
                        } else {
                            value
                        };
                        framed_part(&mut digest, value.as_bytes())?;
                    }
                    (None, HashNullPolicy::Empty) => {
                        digest.update([1]);
                        framed_part(&mut digest, b"")?;
                    }
                    (None, HashNullPolicy::Literal) => {
                        digest.update([1]);
                        let literal = if config.normalize {
                            config.null_literal.trim().to_lowercase()
                        } else {
                            config.null_literal.clone()
                        };
                        framed_part(&mut digest, literal.as_bytes())?;
                    }
                    (None, HashNullPolicy::Error) => {
                        return Err(PlenoraError::Contract(format!(
                            "sha256_hash: null alla riga {row}"
                        )))
                    }
                }
            }
            Ok(format!("{:x}", digest.finalize()))
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(StringArray::from(values)),
    )
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintAlgorithm {
    #[default]
    Sha256,
    Md5,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StableFingerprint {
    /// Colonne hashate, nell'ordine dato. Vuoto (default) = tutte le colonne
    /// nell'ordine dello schema.
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default = "default_fingerprint_name")]
    pub output_column: String,
    #[serde(default)]
    pub algorithm: FingerprintAlgorithm,
}

fn default_fingerprint_name() -> String {
    "fingerprint".into()
}

/// Frame lunghezza+valore (u64 big-endian): nessuna ambiguita' di
/// concatenazione tra parti adiacenti.
fn framed_digest<D: Digest>(digest: &mut D, value: &[u8]) -> Result<()> {
    let length = u64::try_from(value.len())
        .map_err(|_| PlenoraError::Contract("stable_fingerprint: valore troppo grande".into()))?;
    digest.update(length.to_be_bytes());
    digest.update(value);
    Ok(())
}

/// Codifica canonica di una riga (estensione v1.1), byte esatti:
///
/// - separatore di dominio `b"plenora-fingerprint-v1\0"`;
/// - per ogni colonna, nell'ordine di config (o dello schema se omessa):
///   `framed(nome)`, `framed(tipo Arrow)`, poi un byte di presenza: `0x00`
///   per null, `0x01` seguito da `framed(valore)` per un valore;
/// - il valore e' la rappresentazione testuale di `scalar_as_string`, senza
///   alcuna normalizzazione (trim/case): null e stringa vuota restano
///   distinti, e righe diverse non collidono per costruzione.
///
/// Determinismo assoluto: stessi byte in input -> stesso digest su qualunque
/// run o macchina (gli algoritmi sono sha2/md5 su un byte stream fisso).
fn fingerprint_rows<D: Digest>(
    batch: &RecordBatch,
    names: &[String],
    indices: &[usize],
) -> Result<Vec<String>> {
    (0..batch.num_rows())
        .map(|row| {
            let mut digest = D::new();
            digest.update(b"plenora-fingerprint-v1\0");
            for (name, index) in names.iter().zip(indices) {
                framed_digest(&mut digest, name.as_bytes())?;
                framed_digest(
                    &mut digest,
                    batch.column(*index).data_type().to_string().as_bytes(),
                )?;
                match scalar_as_string(batch.column(*index).as_ref(), row)? {
                    Some(value) => {
                        digest.update([1]);
                        framed_digest(&mut digest, value.as_bytes())?;
                    }
                    None => digest.update([0]),
                }
            }
            // Esadecimale minuscolo, byte per byte: identico al formato
            // `{:x}` dei digest md5/sha2, ma senza bound generico `LowerHex`.
            let digest = digest.finalize();
            let mut hex = String::with_capacity(digest.len() * 2);
            for byte in digest {
                let _ = write!(hex, "{byte:02x}");
            }
            Ok(hex)
        })
        .collect()
}

pub fn stable_fingerprint(batch: &RecordBatch, config: &StableFingerprint) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    let names: Vec<String> = if config.columns.is_empty() {
        batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect()
    } else {
        let mut seen = HashSet::new();
        for name in &config.columns {
            if !seen.insert(name.as_str()) {
                return Err(PlenoraError::Contract(format!(
                    "stable_fingerprint: colonna ripetuta: {name}"
                )));
            }
        }
        config.columns.clone()
    };
    if names.is_empty() {
        return Err(PlenoraError::Contract(
            "stable_fingerprint richiede almeno una colonna".into(),
        ));
    }
    let indices = names
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let values = match config.algorithm {
        FingerprintAlgorithm::Sha256 => fingerprint_rows::<Sha256>(batch, &names, &indices)?,
        FingerprintAlgorithm::Md5 => fingerprint_rows::<Md5>(batch, &names, &indices)?,
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(StringArray::from(values)),
    )
}

// ---------------------------------------------------------------------------
// table.hmac_sha256 (estensione v1.2)
// ---------------------------------------------------------------------------

/// Politica sui null per `hmac_sha256`: `empty` = il null contribuisce come
/// stringa vuota (indistinguibile da ""), `null` = la riga produce un hmac
/// null, `skip` = la colonna null e' omessa dal framing della riga.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HmacNullPolicy {
    #[default]
    Empty,
    Null,
    Skip,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HmacSha256 {
    /// Colonne hashate, nell'ordine dato (come `stable_fingerprint`).
    pub columns: Vec<String>,
    /// NOME della variabile d'ambiente che contiene la chiave. La chiave non
    /// compare mai nel piano, negli errori o nei log: solo il nome.
    pub key_env: String,
    #[serde(default = "default_hmac_name")]
    pub output_column: String,
    #[serde(default)]
    pub null_policy: HmacNullPolicy,
}

fn default_hmac_name() -> String {
    "hmac".into()
}

/// HMAC-SHA256 (RFC 2104) implementato sopra `sha2` — due round di hash con
/// ipad/opad su blocco da 64 byte, nessuna dipendenza aggiuntiva.
fn hmac_sha256_digest(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = Sha256::digest(key);
        block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    for byte in block {
        inner.update([byte ^ 0x36]);
    }
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    for byte in block {
        outer.update([byte ^ 0x5c]);
    }
    outer.update(inner);
    let digest = outer.finalize();
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

/// Legge la chiave dalla variabile d'ambiente indicata. L'errore e'
/// volutamente generico: non rivela ne' il nome della variabile ne' alcun
/// frammento del valore.
fn load_hmac_key(key_env: &str) -> Result<Vec<u8>> {
    match std::env::var(key_env) {
        Ok(value) if !value.is_empty() => Ok(value.into_bytes()),
        _ => Err(PlenoraError::Contract(
            "hmac_sha256: chiave HMAC non disponibile".into(),
        )),
    }
}

fn framed_bytes(message: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let length = u64::try_from(value.len())
        .map_err(|_| PlenoraError::Contract("hmac_sha256: valore troppo grande".into()))?;
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(value);
    Ok(())
}

/// HMAC-SHA256 per riga della concatenazione canonica dei valori (stesso
/// framing di `stable_fingerprint`: separatore di dominio, `framed(nome)`,
/// `framed(tipo)`, byte di presenza, `framed(valore)`), con separatore
/// `b"plenora-hmac-sha256-v1\0"`. La chiave arriva SOLO dalla variabile
/// d'ambiente il cui nome e' `key_env`.
pub fn hmac_sha256(batch: &RecordBatch, config: &HmacSha256) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    if config.key_env.trim().is_empty() {
        return Err(PlenoraError::Contract("hmac_sha256: key_env vuoto".into()));
    }
    if config.columns.is_empty() {
        return Err(PlenoraError::Contract(
            "hmac_sha256 richiede almeno una colonna".into(),
        ));
    }
    let mut seen = HashSet::new();
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return Err(PlenoraError::Contract(format!(
                "hmac_sha256: colonna ripetuta: {name}"
            )));
        }
    }
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let key = load_hmac_key(&config.key_env)?;
    let values = (0..batch.num_rows())
        .map(|row| {
            let mut message = b"plenora-hmac-sha256-v1\0".to_vec();
            for (name, index) in config.columns.iter().zip(&indices) {
                let array = batch.column(*index).as_ref();
                match scalar_as_string(array, row)? {
                    Some(value) => {
                        framed_bytes(&mut message, name.as_bytes())?;
                        framed_bytes(&mut message, array.data_type().to_string().as_bytes())?;
                        message.push(1);
                        framed_bytes(&mut message, value.as_bytes())?;
                    }
                    None => match config.null_policy {
                        HmacNullPolicy::Empty => {
                            framed_bytes(&mut message, name.as_bytes())?;
                            framed_bytes(&mut message, array.data_type().to_string().as_bytes())?;
                            message.push(1);
                            framed_bytes(&mut message, b"")?;
                        }
                        HmacNullPolicy::Null => return Ok(None),
                        HmacNullPolicy::Skip => {}
                    },
                }
            }
            let digest = hmac_sha256_digest(&key, &message);
            let mut hex = String::with_capacity(digest.len() * 2);
            for byte in digest {
                let _ = write!(hex, "{byte:02x}");
            }
            Ok(Some(hex))
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        matches!(config.null_policy, HmacNullPolicy::Null),
        Arc::new(StringArray::from(values)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskType {    Cf,
    Email,
    Phone,
    Iban,
    Custom,
}

const fn default_mask_type() -> MaskType {
    MaskType::Custom
}
const fn default_three() -> usize {
    3
}
fn default_mask_char() -> String {
    "*".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Masking {
    pub column: String,
    #[serde(default = "default_mask_type")]
    pub mask_type: MaskType,
    #[serde(default = "default_three")]
    pub chars_start: usize,
    #[serde(default = "default_three")]
    pub chars_end: usize,
    #[serde(default = "default_mask_char")]
    pub mask_char: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaskData {
    pub maskings: Vec<Masking>,
    #[serde(default)]
    pub overwrite: bool,
}

/// Maschera i caratteri centrali di `value` mantenendo `start` caratteri
/// iniziali ed `end` finali. Lavora su indici di byte (via `char_indices`)
/// senza materializzare un `Vec<char>`: stessi byte in output della versione
/// originale (Unicode incluso), stessa condizione di ritorno anticipato.
fn mask_middle(value: &str, start: usize, end: usize, mask: char) -> String {
    let char_count = value.chars().count();
    if char_count <= start.saturating_add(end) {
        return value.to_owned();
    }
    // Offset di byte dopo i primi `start` caratteri e all'inizio degli
    // ultimi `end` caratteri.
    let start_byte = value
        .char_indices()
        .nth(start)
        .map_or(value.len(), |(index, _)| index);
    let end_byte = if end == 0 {
        value.len()
    } else {
        value
            .char_indices()
            .rev()
            .nth(end - 1)
            .map_or(0, |(index, _)| index)
    };
    let mask_count = char_count - start - end;
    let mut out = String::with_capacity(
        start_byte + mask_count * mask.len_utf8() + (value.len() - end_byte),
    );
    out.push_str(&value[..start_byte]);
    out.extend(std::iter::repeat_n(mask, mask_count));
    out.push_str(&value[end_byte..]);
    out
}

fn mask(value: &str, config: &Masking) -> Result<String> {
    Ok(match config.mask_type {
        MaskType::Cf => mask_middle(value, 3, 3, '*'),
        MaskType::Iban => mask_middle(value, 4, 4, '*'),
        MaskType::Email => {
            if let Some((local, domain)) = value.rsplit_once('@') {
                let first = match local.chars().count() {
                    0 | 1 => "*".into(),
                    count => format!(
                        "{}{}",
                        local.chars().next().unwrap_or_default(),
                        "*".repeat(count - 1)
                    ),
                };
                format!("{first}@{domain}")
            } else {
                value.to_owned()
            }
        }
        MaskType::Phone => {
            let compact: String = value
                .chars()
                .filter(|ch| ch.is_ascii_digit() || *ch == '+')
                .collect();
            if compact.chars().count() < 6 {
                value.to_owned()
            } else {
                mask_middle(&compact, 3, 4, '*')
            }
        }
        MaskType::Custom => {
            let mut chars = config.mask_char.chars();
            let character = chars
                .next()
                .ok_or_else(|| PlenoraError::Contract("mask_char vuoto".into()))?;
            if chars.next().is_some() {
                return Err(PlenoraError::Contract(
                    "mask_char deve essere un carattere".into(),
                ));
            }
            mask_middle(value, config.chars_start, config.chars_end, character)
        }
    })
}

pub fn mask_data(batch: &RecordBatch, config: &MaskData) -> Result<RecordBatch> {
    if config.maskings.is_empty() {
        return Err(PlenoraError::Contract(
            "mask_data richiede configurazioni".into(),
        ));
    }
    let mut result = batch.clone();
    for masking in &config.maskings {
        let index = column_index(&result, &masking.column)?;
        let output = if config.overwrite {
            masking.column.clone()
        } else {
            format!("{}_masked", masking.column)
        };
        validate_output_name(&output)?;
        let column = result.column(index).clone();
        // Fast path Utf8 (batch 4 ottimizzazioni kernel): valori presi in
        // prestito dallo StringArray senza `scalar_as_string` per riga e
        // output costruito con StringBuilder. Stessi byte, stessi null,
        // stessi errori per riga; gli altri tipi ricadono sul percorso
        // scalare originale, invariato.
        let values = if let Some(strings) = column.as_any().downcast_ref::<StringArray>() {
            let mut builder = StringBuilder::with_capacity(
                result.num_rows(),
                result.num_rows().saturating_mul(8).min(64 * 1024 * 1024),
            );
            for row in 0..result.num_rows() {
                if strings.is_null(row) {
                    builder.append_null();
                } else {
                    builder.append_value(&mask(strings.value(row), masking)?);
                }
            }
            Arc::new(builder.finish())
        } else {
            let values = (0..result.num_rows())
                .map(|row| {
                    scalar_as_string(column.as_ref(), row)
                        .and_then(|value| value.map(|value| mask(&value, masking)).transpose())
                })
                .collect::<Result<Vec<_>>>()?;
            Arc::new(StringArray::from(values))
        };
        result = replace_or_append(
            &result,
            &output,
            DataType::Utf8,
            true,
            values,
        )?;
    }
    Ok(result)
}


#[cfg(test)]
mod tests {
    // -------------------------------------------------------------------
    // Test-oracolo di `mask_data` (batch 4 ottimizzazioni kernel): le
    // implementazioni pre-ottimizzazione di `mask_middle`/`mask`/
    // `mask_data` sono copiate verbatim qui sotto come riferimento
    // indipendente.
    // -------------------------------------------------------------------

    use super::*;
    use plenora_core::arrow::array::{Array, Int64Array};
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::json;

    /// Copia verbatim di `mask_middle` pre-ottimizzazione.
    fn mask_middle_reference(value: &str, start: usize, end: usize, mask: char) -> String {
        let chars: Vec<char> = value.chars().collect();
        if chars.len() <= start.saturating_add(end) {
            return value.to_owned();
        }
        let mut out: String = chars[..start].iter().collect();
        out.extend(std::iter::repeat_n(mask, chars.len() - start - end));
        out.extend(chars[chars.len() - end..].iter());
        out
    }

    /// Copia verbatim di `mask` pre-ottimizzazione (richiama
    /// `mask_middle_reference`).
    fn mask_reference(value: &str, config: &Masking) -> Result<String> {
        Ok(match config.mask_type {
            MaskType::Cf => mask_middle_reference(value, 3, 3, '*'),
            MaskType::Iban => mask_middle_reference(value, 4, 4, '*'),
            MaskType::Email => {
                if let Some((local, domain)) = value.rsplit_once('@') {
                    let first = match local.chars().count() {
                        0 | 1 => "*".into(),
                        count => format!(
                            "{}{}",
                            local.chars().next().unwrap_or_default(),
                            "*".repeat(count - 1)
                        ),
                    };
                    format!("{first}@{domain}")
                } else {
                    value.to_owned()
                }
            }
            MaskType::Phone => {
                let compact: String = value
                    .chars()
                    .filter(|ch| ch.is_ascii_digit() || *ch == '+')
                    .collect();
                if compact.chars().count() < 6 {
                    value.to_owned()
                } else {
                    mask_middle_reference(&compact, 3, 4, '*')
                }
            }
            MaskType::Custom => {
                let mut chars = config.mask_char.chars();
                let character = chars
                    .next()
                    .ok_or_else(|| PlenoraError::Contract("mask_char vuoto".into()))?;
                if chars.next().is_some() {
                    return Err(PlenoraError::Contract(
                        "mask_char deve essere un carattere".into(),
                    ));
                }
                mask_middle_reference(value, config.chars_start, config.chars_end, character)
            }
        })
    }

    /// Copia verbatim di `mask_data` pre-ottimizzazione.
    fn mask_data_reference(batch: &RecordBatch, config: &MaskData) -> Result<RecordBatch> {
        if config.maskings.is_empty() {
            return Err(PlenoraError::Contract(
                "mask_data richiede configurazioni".into(),
            ));
        }
        let mut result = batch.clone();
        for masking in &config.maskings {
            let index = column_index(&result, &masking.column)?;
            let output = if config.overwrite {
                masking.column.clone()
            } else {
                format!("{}_masked", masking.column)
            };
            validate_output_name(&output)?;
            let values = (0..result.num_rows())
                .map(|row| {
                    scalar_as_string(result.column(index).as_ref(), row).and_then(|value| {
                        value.map(|value| mask_reference(&value, masking)).transpose()
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            result = replace_or_append(
                &result,
                &output,
                DataType::Utf8,
                true,
                Arc::new(StringArray::from(values)),
            )?;
        }
        Ok(result)
    }

    /// Confronto rigoroso: schema (nomi, tipi, nullabilita'), maschera null
    /// e valori via profilo scalare.
    fn assert_batches_identical(fast: &RecordBatch, reference: &RecordBatch) {
        assert_eq!(fast.num_rows(), reference.num_rows(), "righe");
        assert_eq!(fast.num_columns(), reference.num_columns(), "colonne");
        let fast_schema = fast.schema();
        let reference_schema = reference.schema();
        for index in 0..fast.num_columns() {
            let fast_field = fast_schema.field(index);
            let reference_field = reference_schema.field(index);
            assert_eq!(fast_field.name(), reference_field.name(), "nome colonna {index}");
            assert_eq!(
                fast_field.data_type(),
                reference_field.data_type(),
                "tipo colonna {index}"
            );
            assert_eq!(
                fast_field.is_nullable(),
                reference_field.is_nullable(),
                "nullabilita' colonna {index}"
            );
            for row in 0..fast.num_rows() {
                assert_eq!(
                    fast.column(index).is_null(row),
                    reference.column(index).is_null(row),
                    "null riga {row} colonna {index}"
                );
                assert_eq!(
                    scalar_as_string(fast.column(index).as_ref(), row).expect("fast"),
                    scalar_as_string(reference.column(index).as_ref(), row).expect("ref"),
                    "valore riga {row} colonna {index}"
                );
            }
        }
    }

    fn masking(column: &str, mask_type: MaskType) -> Masking {
        Masking {
            column: column.into(),
            mask_type,
            chars_start: 3,
            chars_end: 3,
            mask_char: "*".into(),
        }
    }

    // -------------------------------------------------------------------
    // Test di `stable_fingerprint` (estensione v1.1): known-answer calcolata
    // a mano sull'encoding canonico documentato nel kernel, piu' sensibilita'
    // e determinismo.
    // -------------------------------------------------------------------

    fn fingerprint_fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x"), None, Some("")])),
                Arc::new(Int64Array::from(vec![Some(42), Some(7), Some(7)])),
            ],
        )
        .expect("fixture")
    }

    fn fingerprint_values(batch: &RecordBatch, config: &StableFingerprint) -> Vec<String> {
        let output = stable_fingerprint(batch, config).expect("fingerprint");
        let column = output
            .column_by_name(&config.output_column)
            .expect("colonna fingerprint")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        (0..column.len()).map(|row| column.value(row).to_owned()).collect()
    }

    fn fingerprint_config(columns: &[&str]) -> StableFingerprint {
        StableFingerprint {
            columns: columns.iter().map(|name| (*name).to_owned()).collect(),
            output_column: "fingerprint".into(),
            algorithm: FingerprintAlgorithm::Sha256,
        }
    }

    #[test]
    fn stable_fingerprint_known_answer_sha256_and_md5() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x")])),
                Arc::new(Int64Array::from(vec![Some(42)])),
            ],
        )
        .expect("fixture");
        // Known-answer sull'encoding: "plenora-fingerprint-v1\0" +
        // framed("a") framed("Utf8") 0x01 framed("x") +
        // framed("b") framed("Int64") 0x01 framed("42").
        let sha = fingerprint_values(&batch, &fingerprint_config(&["a", "b"]));
        assert_eq!(
            sha,
            vec!["4e1f09e49d945536920b917446d005d90e62d4bb2cc231557ed6a8bef6c4f21f"]
        );
        let md5 = fingerprint_values(
            &batch,
            &StableFingerprint {
                algorithm: FingerprintAlgorithm::Md5,
                ..fingerprint_config(&["a", "b"])
            },
        );
        assert_eq!(md5, vec!["79a680c00d6257fa3364bb4b40e7963d"]);
    }

    #[test]
    fn stable_fingerprint_is_deterministic_and_sensitive() {
        let batch = fingerprint_fixture();
        let config = fingerprint_config(&["a", "b"]);
        let first = fingerprint_values(&batch, &config);
        let second = fingerprint_values(&batch, &config);
        assert_eq!(first, second, "stesso input -> stesso hash");
        assert!(first.iter().all(|value| value.len() == 64));
        // Righe diverse -> hash diversi; null ed empty string distinti.
        assert_ne!(first[0], first[1]);
        assert_ne!(first[1], first[2], "null e stringa vuota devono differire");
        // L'ordine delle colonne di config cambia il digest.
        let swapped = fingerprint_values(&batch, &fingerprint_config(&["b", "a"]));
        assert_ne!(first, swapped);
        // Un subset di colonne ignora le altre.
        let subset = fingerprint_values(&batch, &fingerprint_config(&["b"]));
        assert_eq!(subset[1], subset[2], "stessa colonna b -> stesso hash");
        assert_ne!(subset[0], subset[1]);
    }

    #[test]
    fn stable_fingerprint_defaults_and_validation() {
        let batch = fingerprint_fixture();
        // Default: tutte le colonne in ordine di schema, output "fingerprint",
        // algoritmo sha256.
        let decoded: StableFingerprint = serde_json::from_value(json!({})).expect("defaults");
        assert!(decoded.columns.is_empty());
        assert_eq!(decoded.output_column, "fingerprint");
        assert!(matches!(decoded.algorithm, FingerprintAlgorithm::Sha256));
        let output = stable_fingerprint(&batch, &decoded).expect("fingerprint");
        assert_eq!(output.num_columns(), 3);
        assert_eq!(output.num_rows(), batch.num_rows());
        let schema = output.schema();
        let field = schema
            .field_with_name("fingerprint")
            .expect("colonna output");
        assert_eq!(field.data_type(), &DataType::Utf8);
        assert!(!field.is_nullable());
        // Tutte le colonne di default = stesso hash di columns esplicite nello
        // stesso ordine.
        assert_eq!(
            fingerprint_values(&batch, &decoded),
            fingerprint_values(&batch, &fingerprint_config(&["a", "b"]))
        );
        // Errori: colonna mancante, duplicata, campo config sconosciuto.
        assert!(stable_fingerprint(&batch, &fingerprint_config(&["missing"])).is_err());
        assert!(stable_fingerprint(&batch, &fingerprint_config(&["a", "a"])).is_err());
        assert!(serde_json::from_value::<StableFingerprint>(json!({"algo": "sha256"})).is_err());
        assert!(stable_fingerprint(
            &batch,
            &StableFingerprint {
                output_column: " ".into(),
                ..fingerprint_config(&["a"])
            },
        )
        .is_err());
    }

    // -------------------------------------------------------------------
    // Test di `hmac_sha256` (estensione v1.2): known-answer RFC 2104
    // calcolate esternamente sul framing canonico documentato nel kernel,
    // null policy, e la garanzia che la chiave non compaia MAI negli errori.
    // -------------------------------------------------------------------

    fn hmac_fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x"), None])),
                Arc::new(Int64Array::from(vec![Some(42), Some(7)])),
            ],
        )
        .expect("fixture")
    }

    fn hmac_config(columns: &[&str], key_env: &str) -> HmacSha256 {
        HmacSha256 {
            columns: columns.iter().map(|name| (*name).to_owned()).collect(),
            key_env: key_env.into(),
            output_column: "hmac".into(),
            null_policy: HmacNullPolicy::Empty,
        }
    }

    fn hmac_values(batch: &RecordBatch, config: &HmacSha256) -> Vec<Option<String>> {
        let output = hmac_sha256(batch, config).expect("hmac");
        let column = output
            .column_by_name(&config.output_column)
            .expect("colonna hmac")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        (0..column.len())
            .map(|row| (!column.is_null(row)).then(|| column.value(row).to_owned()))
            .collect()
    }

    #[test]
    fn hmac_sha256_known_answers() {
        // Known-answer (Python hmac/hashlib) su: "plenora-hmac-sha256-v1\0" +
        // framed("a") framed("Utf8") 0x01 framed("x") + framed("b")
        // framed("Int64") 0x01 framed("42"), chiave da env.
        std::env::set_var("PLENORA_HMAC_KAT_KEY", "plenora-hmac-test-key");
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Utf8, true),
                Field::new("b", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x")])),
                Arc::new(Int64Array::from(vec![Some(42)])),
            ],
        )
        .expect("fixture");
        let values = hmac_values(&batch, &hmac_config(&["a", "b"], "PLENORA_HMAC_KAT_KEY"));
        assert_eq!(
            values,
            vec![Some(
                "46424a38201bbe2cf03b90d3d48444bd54dc41a70002f4991a146138ca4e0d10".to_owned()
            )]
        );
        // Chiave piu' lunga del blocco (100 byte): percorso key = H(key).
        std::env::set_var("PLENORA_HMAC_KAT_LONG", "k".repeat(100));
        let values = hmac_values(&batch, &hmac_config(&["a", "b"], "PLENORA_HMAC_KAT_LONG"));
        assert_eq!(
            values,
            vec![Some(
                "d9893ec92c4162280504db2c21c83dd75ae2106033f716baa1fcc6804a2b1a82".to_owned()
            )]
        );
    }

    #[test]
    fn hmac_sha256_null_policies() {
        std::env::set_var("PLENORA_HMAC_NULL_KEY", "plenora-hmac-test-key");
        let batch = hmac_fixture();
        // empty: null -> stringa vuota nel framing (known-answer Python).
        let values = hmac_values(&batch, &hmac_config(&["a"], "PLENORA_HMAC_NULL_KEY"));
        assert_eq!(
            values[1],
            Some("c863b8595ce5ae9a7b3a05849cdc952b19d26ce2e96201a612c4d2d759cb429d".to_owned())
        );
        // null: la riga con un null produce hmac null (colonna nullable).
        let config = HmacSha256 {
            null_policy: HmacNullPolicy::Null,
            ..hmac_config(&["a"], "PLENORA_HMAC_NULL_KEY")
        };
        let output = hmac_sha256(&batch, &config).expect("hmac null policy");
        assert!(output.schema().field_with_name("hmac").expect("hmac").is_nullable());
        let values = hmac_values(&batch, &config);
        assert!(values[0].is_some());
        assert_eq!(values[1], None);
        // skip: la colonna null e' omessa dal framing -> digest diverso da
        // empty, deterministico.
        let config = HmacSha256 {
            null_policy: HmacNullPolicy::Skip,
            ..hmac_config(&["a"], "PLENORA_HMAC_NULL_KEY")
        };
        let skipped = hmac_values(&batch, &config);
        assert!(skipped[1].is_some());
        assert_ne!(skipped[1], values[0]);
    }

    #[test]
    fn hmac_sha256_is_deterministic_and_sensitive() {
        std::env::set_var("PLENORA_HMAC_DET_KEY", "plenora-hmac-test-key");
        let batch = hmac_fixture();
        let config = hmac_config(&["a", "b"], "PLENORA_HMAC_DET_KEY");
        let first = hmac_values(&batch, &config);
        let second = hmac_values(&batch, &config);
        assert_eq!(first, second, "stesso input -> stesso hmac");
        // Ordine delle colonne e chiave diversa cambiano il digest.
        let swapped = hmac_values(&batch, &hmac_config(&["b", "a"], "PLENORA_HMAC_DET_KEY"));
        assert_ne!(first, swapped);
        std::env::set_var("PLENORA_HMAC_DET_KEY2", "altra-chiave");
        let other_key = hmac_values(&batch, &hmac_config(&["a", "b"], "PLENORA_HMAC_DET_KEY2"));
        assert_ne!(first, other_key);
    }

    #[test]
    fn hmac_sha256_key_never_leaks_in_errors() {
        let secret = "chiave-segreta-DA-NON-RIVELARE-12345";
        std::env::set_var("PLENORA_HMAC_LEAK_NAME", secret);
        let batch = hmac_fixture();
        // 1. Variabile assente: l'errore non rivela ne' il NOME della
        //    variabile ne' (ovviamente) il valore.
        std::env::remove_var("PLENORA_HMAC_LEAK_MISSING");
        let error = hmac_sha256(
            &batch,
            &hmac_config(&["a"], "PLENORA_HMAC_LEAK_MISSING"),
        )
        .expect_err("variabile assente");
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(!display.contains("PLENORA_HMAC_LEAK_MISSING"), "nome variabile in {display}");
        assert!(!debug.contains("PLENORA_HMAC_LEAK_MISSING"), "nome variabile in {debug}");
        // 2. Variabile vuota: stesso errore generico, niente valore/nome.
        std::env::set_var("PLENORA_HMAC_LEAK_EMPTY", "");
        let error = hmac_sha256(&batch, &hmac_config(&["a"], "PLENORA_HMAC_LEAK_EMPTY"))
            .expect_err("variabile vuota");
        assert!(!error.to_string().contains("PLENORA_HMAC_LEAK_EMPTY"));
        // 3. Errori successivi alla lettura della chiave (colonna mancante):
        //    il valore segreto non compare in nessuna forma.
        let error = hmac_sha256(
            &batch,
            &hmac_config(&["missing_column"], "PLENORA_HMAC_LEAK_NAME"),
        )
        .expect_err("colonna mancante");
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(!display.contains(secret), "chiave in {display}");
        assert!(!debug.contains(secret), "chiave in {debug}");
        // 4. Errori di validazione config: idem.
        let config = HmacSha256 {
            columns: vec!["a".into(), "a".into()],
            ..hmac_config(&["a"], "PLENORA_HMAC_LEAK_NAME")
        };
        let error = hmac_sha256(&batch, &config).expect_err("colonna ripetuta");
        assert!(!error.to_string().contains(secret));
    }

    #[test]
    fn hmac_sha256_config_validation() {
        std::env::set_var("PLENORA_HMAC_CFG_KEY", "k");
        let batch = hmac_fixture();
        // Config strict: campo sconosciuto rifiutato.
        assert!(serde_json::from_value::<HmacSha256>(
            json!({"columns": ["a"], "key_env": "PLENORA_HMAC_CFG_KEY", "surprise": 1})
        )
        .is_err());
        // Defaults: output "hmac", null_policy empty.
        let decoded: HmacSha256 = serde_json::from_value(
            json!({"columns": ["a"], "key_env": "PLENORA_HMAC_CFG_KEY"}),
        )
        .expect("defaults");
        assert_eq!(decoded.output_column, "hmac");
        assert!(matches!(decoded.null_policy, HmacNullPolicy::Empty));
        // Colonne vuote, ripetute, key_env vuoto, nome output non valido.
        assert!(hmac_sha256(&batch, &hmac_config(&[], "PLENORA_HMAC_CFG_KEY")).is_err());
        assert!(hmac_sha256(&batch, &hmac_config(&["a", "a"], "PLENORA_HMAC_CFG_KEY")).is_err());
        assert!(hmac_sha256(&batch, &hmac_config(&["a"], " ")).is_err());
        let config = HmacSha256 {
            output_column: " ".into(),
            ..hmac_config(&["a"], "PLENORA_HMAC_CFG_KEY")
        };
        assert!(hmac_sha256(&batch, &config).is_err());
    }

    #[test]
    fn mask_middle_unicode_e_casi_limite() {
        let cases: &[(&str, usize, usize, char)] = &[
            ("🦀🦀🦀🦀🦀🦀🦀🦀", 2, 2, '*'),
            ("héllo", 3, 3, '*'),   // corta: 5 <= 3+3
            ("héllow", 3, 3, '*'),  // estremo: 6 > 3+3 falso -> invariata
            ("héllowo", 3, 3, '*'), // 7 > 6: un solo carattere mascherato
            ("", 3, 3, '*'),
            ("abcdefgh", 0, 0, '*'),
            ("abcdefgh", 0, 3, '*'),
            ("abcdefgh", 3, 0, '*'),
            ("abcdefgh", 10, 0, '*'),
            ("abcdefgh", 0, 10, '*'),
            ("abcdefgh", 2, 2, '•'), // mask multi-byte
            ("RSSRA85M01H501Z", 3, 3, '*'),
            ("IT60X0542811101000000123456", 4, 4, '*'),
        ];
        for (value, start, end, mask) in cases {
            assert_eq!(
                mask_middle(value, *start, *end, *mask),
                mask_middle_reference(value, *start, *end, *mask),
                "mask_middle({value:?}, {start}, {end}, {mask:?})"
            );
        }
    }

    #[test]
    fn mask_data_tutti_i_tipi_oracle() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("cf", DataType::Utf8, true),
                Field::new("email", DataType::Utf8, true),
                Field::new("phone", DataType::Utf8, true),
                Field::new("iban", DataType::Utf8, true),
                Field::new("text", DataType::Utf8, true),
                Field::new("num", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("RSSRA85M01H501Z"),
                    Some("corto"),
                    None,
                    Some("🦀🦀🦀🦀🦀🦀🦀🦀🦀"),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("mario.rossi@example.com"),
                    Some("a@b.it"),
                    Some("@dominio.it"),
                    Some("senza-chiocciola"),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("+39 333 123 4567"),
                    Some("12345"),
                    Some("12 34"),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    Some("IT60X0542811101000000123456"),
                    Some("IT60X0"),
                    Some("IT60X"),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    Some("héllo wörld"),
                    Some(""),
                    None,
                    Some("x"),
                ])),
                Arc::new(Int64Array::from(vec![Some(123_456), None, Some(-7), Some(0)])),
            ],
        )
        .expect("fixture");
        let config = MaskData {
            maskings: vec![
                masking("cf", MaskType::Cf),
                masking("email", MaskType::Email),
                masking("phone", MaskType::Phone),
                masking("iban", MaskType::Iban),
                masking("text", MaskType::Custom),
                masking("num", MaskType::Custom), // percorso generico non-Utf8
            ],
            overwrite: true,
        };
        let fast = mask_data(&batch, &config).expect("fast");
        let reference = mask_data_reference(&batch, &config).expect("ref");
        assert_batches_identical(&fast, &reference);
        // overwrite=false: colonne *_masked aggiunte.
        let config = MaskData {
            maskings: config.maskings,
            overwrite: false,
        };
        let fast = mask_data(&batch, &config).expect("fast");
        let reference = mask_data_reference(&batch, &config).expect("ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn mask_data_input_vuoto_oracle() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("text", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(Vec::<Option<&str>>::new()))],
        )
        .expect("fixture");
        let config = MaskData {
            maskings: vec![masking("text", MaskType::Custom)],
            overwrite: true,
        };
        let fast = mask_data(&batch, &config).expect("fast");
        let reference = mask_data_reference(&batch, &config).expect("ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn mask_data_errori_identici() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("text", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![Some("abcdefgh")]))],
        )
        .expect("fixture");
        // mask_char vuoto e multi-carattere: stesso errore.
        for mask_char in ["", "**"] {
            let config = MaskData {
                maskings: vec![Masking {
                    mask_char: mask_char.into(),
                    ..masking("text", MaskType::Custom)
                }],
                overwrite: true,
            };
            let fast = mask_data(&batch, &config);
            let reference = mask_data_reference(&batch, &config);
            assert_eq!(
                format!("{:?}", fast.expect_err("fast deve fallire")),
                format!("{:?}", reference.expect_err("ref deve fallire"))
            );
        }
        // Nessuna configurazione e colonna mancante: stesso errore.
        let config = MaskData {
            maskings: Vec::new(),
            overwrite: true,
        };
        let fast = mask_data(&batch, &config);
        let reference = mask_data_reference(&batch, &config);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        let config = MaskData {
            maskings: vec![masking("manca", MaskType::Custom)],
            overwrite: true,
        };
        let fast = mask_data(&batch, &config);
        let reference = mask_data_reference(&batch, &config);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
    }
}
