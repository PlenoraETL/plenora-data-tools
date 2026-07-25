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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskType {
    Cf,
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

    #[test]
    fn mask_middle_unicode_e_casi_limite() {
        let cases: &[(&str, usize, usize, char)] = &[
            ("héllo wörld", 3, 3, '*'),
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
