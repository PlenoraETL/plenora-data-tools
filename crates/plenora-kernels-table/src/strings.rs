use std::sync::Arc;

use plenora_core::arrow::array::builder::StringBuilder;
use plenora_core::arrow::array::{Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use regex::Regex;
use serde::Deserialize;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::Limits;
use plenora_core::{PlenoraError, Result};

use super::{replace_or_append, utf8_column, validate_output_name};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringPad {
    pub column: String,
    #[serde(default = "default_width")]
    pub width: usize,
    #[serde(default = "default_side")]
    pub side: PadSide,
    #[serde(default = "default_fill")]
    pub fill_char: String,
    pub output_column: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadSide {
    Left,
    Right,
}

const fn default_width() -> usize {
    5
}
const fn default_side() -> PadSide {
    PadSide::Left
}
fn default_fill() -> String {
    "0".into()
}

pub fn string_pad(
    batch: &RecordBatch,
    config: &StringPad,
    limits: &Limits,
) -> Result<RecordBatch> {
    let output_name = config.output_column.as_deref().unwrap_or(&config.column);
    validate_output_name(output_name)?;
    let mut fill = config.fill_char.chars();
    let fill_char = fill
        .next()
        .ok_or_else(|| PlenoraError::Contract("fill_char e' vuoto".into()))?;
    if fill.next().is_some() {
        return Err(PlenoraError::Contract(
            "fill_char deve contenere un solo carattere Unicode".into(),
        ));
    }
    let input = utf8_column(batch, &config.column)?;
    let mut output = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        if input.is_null(row) {
            output.push(None);
            continue;
        }
        let value = input.value(row);
        let length = value.chars().count();
        let padding = config.width.saturating_sub(length);
        let mut padded = String::with_capacity(
            value
                .len()
                .saturating_add(padding.saturating_mul(fill_char.len_utf8())),
        );
        match config.side {
            PadSide::Left => {
                padded.extend(std::iter::repeat_n(fill_char, padding));
                padded.push_str(value);
            }
            PadSide::Right => {
                padded.push_str(value);
                padded.extend(std::iter::repeat_n(fill_char, padding));
            }
        }
        if padded.len() > limits.max_string_bytes {
            return Err(PlenoraError::Contract(
                "string_pad supera max_string_bytes".into(),
            ));
        }
        output.push(Some(padded));
    }
    replace_or_append(
        batch,
        output_name,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(output)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringLength {
    pub column: String,
    pub output_column: Option<String>,
}

pub fn string_length(batch: &RecordBatch, config: &StringLength) -> Result<RecordBatch> {
    let output_name = config
        .output_column
        .clone()
        .unwrap_or_else(|| format!("{}_length", config.column));
    validate_output_name(&output_name)?;
    let input = utf8_column(batch, &config.column)?;
    let values: Result<Vec<Option<i64>>> = (0..batch.num_rows())
        .map(|row| {
            if input.is_null(row) {
                Ok(None)
            } else {
                i64::try_from(input.value(row).chars().count())
                    .map(Some)
                    .map_err(|_| PlenoraError::Contract("stringa troppo lunga".into()))
            }
        })
        .collect();
    replace_or_append(
        batch,
        &output_name,
        DataType::Int64,
        true,
        Arc::new(Int64Array::from(values?)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringExtract {
    pub column: String,
    pub pattern: String,
    pub output_column: Option<String>,
    #[serde(default)]
    pub extract_all: bool,
}

// Fast path `string_extract` (ultimo batch filone ottimizzazioni kernel): la
// regex era gia' compilata una sola volta per chiamata; qui si eliminano i
// costi per riga rimasti:
// - gruppi nominati: una sola ricerca per riga (prima una per gruppo) con
//   `CaptureLocations` riusato, e scrittura delle slice direttamente negli
//   `StringBuilder` senza `String` intermedie;
// - gruppo singolo/intero match: stessa ricerca con locations riusate,
//   nessuna allocazione per riga;
// - `extract_all`: `captures_iter` invariato (gestisce l'avanzamento dei
//   match vuoti per code point Unicode) ma l'accumulo avviene in uno scratch
//   riusato invece di `Vec<&str>` + `join`.
// Semantica byte-identica: null -> null, nessun match -> null, gruppo non
// partecipante -> null, stessi errori (pattern oltre max_regex_bytes, regex
// non valida, nomi di output non validi nello stesso ordine).

fn utf8_data_len(values: &StringArray) -> usize {
    let offsets = values.offsets();
    usize::try_from(offsets[values.len()] - offsets[0]).unwrap_or(0)
}

pub fn string_extract(
    batch: &RecordBatch,
    config: &StringExtract,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.pattern.len() > limits.max_regex_bytes {
        return Err(PlenoraError::Contract(
            "pattern oltre max_regex_bytes".into(),
        ));
    }
    let regex = Regex::new(&config.pattern)
        .map_err(|error| PlenoraError::Contract(format!("regex non valida: {error}")))?;
    let input = utf8_column(batch, &config.column)?;
    let named: Vec<(usize, String)> = regex
        .capture_names()
        .enumerate()
        .filter_map(|(index, name)| name.map(|name| (index, name.to_owned())))
        .collect();
    if !named.is_empty() {
        // Validazione dei nomi in anticipo nello stesso ordine del percorso
        // originale: la costruzione dei valori non puo' fallire, quindi il
        // primo nome non valido produceva (e produce) lo stesso errore.
        for (_, name) in &named {
            validate_output_name(name)?;
        }
        let per_group_capacity = utf8_data_len(input) / named.len() + 16;
        let mut builders: Vec<StringBuilder> = named
            .iter()
            .map(|_| StringBuilder::with_capacity(batch.num_rows(), per_group_capacity))
            .collect();
        let mut locations = regex.capture_locations();
        for value in input.iter() {
            match value {
                Some(text) if regex.captures_read(&mut locations, text).is_some() => {
                    for (builder, (capture_index, _)) in builders.iter_mut().zip(&named) {
                        match locations.get(*capture_index) {
                            Some((start, end)) => builder.append_value(&text[start..end]),
                            None => builder.append_null(),
                        }
                    }
                }
                _ => builders
                    .iter_mut()
                    .for_each(StringBuilder::append_null),
            }
        }
        let mut result = batch.clone();
        for ((_, name), mut builder) in named.iter().zip(builders) {
            result = replace_or_append(
                &result,
                name,
                DataType::Utf8,
                true,
                Arc::new(builder.finish()),
            )?;
        }
        return Ok(result);
    }
    let output = config
        .output_column
        .clone()
        .unwrap_or_else(|| format!("{}_extracted", config.column));
    validate_output_name(&output)?;
    let capture_index = usize::from(regex.captures_len() > 1);
    let mut builder = StringBuilder::with_capacity(batch.num_rows(), utf8_data_len(input));
    if config.extract_all {
        let mut scratch = String::new();
        for value in input.iter() {
            match value {
                None => builder.append_null(),
                Some(text) => {
                    scratch.clear();
                    let mut matched = false;
                    for captures in regex.captures_iter(text) {
                        if let Some(value) = captures.get(capture_index) {
                            if matched {
                                scratch.push(',');
                            }
                            scratch.push_str(value.as_str());
                            matched = true;
                        }
                    }
                    if matched {
                        builder.append_value(&scratch);
                    } else {
                        builder.append_null();
                    }
                }
            }
        }
    } else {
        let mut locations = regex.capture_locations();
        for value in input.iter() {
            match value {
                Some(text) if regex.captures_read(&mut locations, text).is_some() => {
                    match locations.get(capture_index) {
                        Some((start, end)) => builder.append_value(&text[start..end]),
                        None => builder.append_null(),
                    }
                }
                _ => builder.append_null(),
            }
        }
    }
    replace_or_append(
        batch,
        &output,
        DataType::Utf8,
        true,
        Arc::new(builder.finish()),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizeOperation {
    Trim,
    Lower,
    Upper,
    Title,
    StripAccents,
    StripDoubleSpaces,
    Full,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextNormalize {
    pub columns: Vec<String>,
    #[serde(default = "default_normalize")]
    pub operations: NormalizeOperation,
    #[serde(default = "default_true")]
    pub overwrite: bool,
}

const fn default_normalize() -> NormalizeOperation {
    NormalizeOperation::Full
}
const fn default_true() -> bool {
    true
}

// Fast path `text_normalize` (secondo batch filone ottimizzazioni kernel):
// le regole di normalizzazione scrivono in un buffer riusato tra le righe,
// senza allocazioni intermedie (niente `Vec<char>` per carattere in title
// case, niente `Vec<&str>` + join nel collapse, passata unica nfkd + filtro
// combining + collapse in `Full`). Semantica byte-identica: il lowercase
// resta `str::to_lowercase` (regola contestuale del sigma greco finale).

fn strip_accents_into(value: &str, out: &mut String) {
    out.extend(value.nfkd().filter(|character| !is_combining_mark(*character)));
}

fn title_case_into(value: &str, out: &mut String) {
    let mut at_word_start = true;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if at_word_start {
                out.extend(character.to_uppercase());
            } else {
                out.extend(character.to_lowercase());
            }
        } else {
            out.push(character);
        }
        at_word_start = !character.is_alphanumeric();
    }
}

/// `split_whitespace().join(" ")` senza il `Vec` intermedio.
fn collapse_whitespace_into(value: &str, out: &mut String) {
    let mut pending_space = false;
    for word in value.split_whitespace() {
        if pending_space {
            out.push(' ');
        }
        out.push_str(word);
        pending_space = true;
    }
}

/// `collapse_whitespace(strip_accents(value.trim().to_lowercase()))` fuso in
/// una passata sola sullo stream nfkd gia' privo di combining mark.
fn full_normalize_into(value: &str, out: &mut String) {
    let lowered = value.trim().to_lowercase();
    let mut pending_space = false;
    for character in lowered.nfkd().filter(|character| !is_combining_mark(*character)) {
        if character.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(character);
        }
    }
}

fn normalize_into(value: &str, operation: &NormalizeOperation, out: &mut String) {
    match operation {
        NormalizeOperation::Trim => out.push_str(value.trim()),
        NormalizeOperation::Lower => out.push_str(&value.to_lowercase()),
        NormalizeOperation::Upper => out.push_str(&value.to_uppercase()),
        NormalizeOperation::Title => title_case_into(value, out),
        NormalizeOperation::StripAccents => strip_accents_into(value, out),
        NormalizeOperation::StripDoubleSpaces => collapse_whitespace_into(value, out),
        NormalizeOperation::Full => full_normalize_into(value, out),
    }
}

pub fn text_normalize(
    batch: &RecordBatch,
    config: &TextNormalize,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.columns.is_empty() {
        return Err(PlenoraError::Contract(
            "text_normalize richiede almeno una colonna".into(),
        ));
    }
    let mut result = batch.clone();
    for name in &config.columns {
        let input = utf8_column(&result, name)?;
        let output_name = if config.overwrite {
            name.clone()
        } else {
            format!("{name}_norm")
        };
        validate_output_name(&output_name)?;
        let mut values = Vec::with_capacity(result.num_rows());
        let mut scratch = String::new();
        for row in 0..result.num_rows() {
            if input.is_null(row) {
                values.push(None);
                continue;
            }
            let value = input.value(row);
            scratch.clear();
            scratch.reserve(value.len());
            normalize_into(value, &config.operations, &mut scratch);
            if scratch.len() > limits.max_string_bytes {
                return Err(PlenoraError::Contract(
                    "text_normalize supera max_string_bytes".into(),
                ));
            }
            values.push(Some(scratch.clone()));
        }
        result = replace_or_append(
            &result,
            &output_name,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(values)),
        )?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::json;

    use super::*;

    // -----------------------------------------------------------------------
    // Implementazioni pre-ottimizzazione: riferimento per l'equivalenza
    // semantica (oracolo) del fast path di `text_normalize`.
    // -----------------------------------------------------------------------

    fn reference_strip_accents(value: &str) -> String {
        value
            .nfkd()
            .filter(|character| !is_combining_mark(*character))
            .collect()
    }

    fn reference_title_case(value: &str) -> String {
        let mut at_word_start = true;
        value
            .chars()
            .flat_map(|character| {
                let converted: Vec<char> = if character.is_alphanumeric() && at_word_start {
                    character.to_uppercase().collect()
                } else if character.is_alphanumeric() {
                    character.to_lowercase().collect()
                } else {
                    vec![character]
                };
                at_word_start = !character.is_alphanumeric();
                converted
            })
            .collect()
    }

    fn reference_collapse_whitespace(value: &str) -> String {
        value.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn reference_normalize(value: &str, operation: &NormalizeOperation) -> String {
        match operation {
            NormalizeOperation::Trim => value.trim().to_owned(),
            NormalizeOperation::Lower => value.to_lowercase(),
            NormalizeOperation::Upper => value.to_uppercase(),
            NormalizeOperation::Title => reference_title_case(value),
            NormalizeOperation::StripAccents => reference_strip_accents(value),
            NormalizeOperation::StripDoubleSpaces => reference_collapse_whitespace(value),
            NormalizeOperation::Full => {
                reference_collapse_whitespace(&reference_strip_accents(
                    &value.trim().to_lowercase(),
                ))
            }
        }
    }

    fn reference_text_normalize(
        batch: &RecordBatch,
        config: &TextNormalize,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        if config.columns.is_empty() {
            return Err(PlenoraError::Contract(
                "text_normalize richiede almeno una colonna".into(),
            ));
        }
        let mut result = batch.clone();
        for name in &config.columns {
            let input = utf8_column(&result, name)?;
            let output_name = if config.overwrite {
                name.clone()
            } else {
                format!("{name}_norm")
            };
            validate_output_name(&output_name)?;
            let mut values = Vec::with_capacity(result.num_rows());
            for row in 0..result.num_rows() {
                if input.is_null(row) {
                    values.push(None);
                } else {
                    let value = reference_normalize(input.value(row), &config.operations);
                    if value.len() > limits.max_string_bytes {
                        return Err(PlenoraError::Contract(
                            "text_normalize supera max_string_bytes".into(),
                        ));
                    }
                    values.push(Some(value));
                }
            }
            result = replace_or_append(
                &result,
                &output_name,
                DataType::Utf8,
                true,
                Arc::new(StringArray::from(values)),
            )?;
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Implementazione pre-ottimizzazione di `string_extract`: riferimento per
    // l'equivalenza semantica (oracolo) del fast path.
    // -----------------------------------------------------------------------

    fn reference_string_extract(
        batch: &RecordBatch,
        config: &StringExtract,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        if config.pattern.len() > limits.max_regex_bytes {
            return Err(PlenoraError::Contract(
                "pattern oltre max_regex_bytes".into(),
            ));
        }
        let regex = Regex::new(&config.pattern)
            .map_err(|error| PlenoraError::Contract(format!("regex non valida: {error}")))?;
        let input = utf8_column(batch, &config.column)?;
        let named: Vec<(usize, String)> = regex
            .capture_names()
            .enumerate()
            .filter_map(|(index, name)| name.map(|name| (index, name.to_owned())))
            .collect();
        if !named.is_empty() {
            let mut result = batch.clone();
            for (capture_index, name) in named {
                validate_output_name(&name)?;
                let values: Vec<Option<String>> = input
                    .iter()
                    .map(|value| {
                        value
                            .and_then(|value| regex.captures(value))
                            .and_then(|captures| {
                                captures
                                    .get(capture_index)
                                    .map(|value| value.as_str().to_owned())
                            })
                    })
                    .collect();
                result = replace_or_append(
                    &result,
                    &name,
                    DataType::Utf8,
                    true,
                    Arc::new(StringArray::from(values)),
                )?;
            }
            return Ok(result);
        }
        let output = config
            .output_column
            .clone()
            .unwrap_or_else(|| format!("{}_extracted", config.column));
        validate_output_name(&output)?;
        let capture_index = usize::from(regex.captures_len() > 1);
        let values: Vec<Option<String>> = input
            .iter()
            .map(|value| {
                value.and_then(|value| {
                    if config.extract_all {
                        let matches = regex
                            .captures_iter(value)
                            .filter_map(|captures| {
                                captures.get(capture_index).map(|value| value.as_str())
                            })
                            .collect::<Vec<_>>();
                        (!matches.is_empty()).then(|| matches.join(","))
                    } else {
                        regex.captures(value).and_then(|captures| {
                            captures
                                .get(capture_index)
                                .map(|value| value.as_str().to_owned())
                        })
                    }
                })
            })
            .collect();
        replace_or_append(
            batch,
            &output,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(values)),
        )
    }

    fn extract_batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("text", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("LO2244_FV01_II01_GEO001"),
                Some("LO0000_XX00_YY00_GEO000"),
                Some("👨\u{200D}👩\u{200D}👧\u{200D}👦 emoji 🎉 123 456"),
                Some("straße 99 München 100"),
                Some("e\u{0301} cafe\u{0300} 42"),
                Some("àéîõü Çñ 7"),
                Some(""),
                Some("   "),
                Some("nessun numero qui"),
                None,
            ]))],
        )
        .expect("fixture")
    }

    fn assert_extract_equivalent(config: &StringExtract, limits: &Limits) {
        let batch = extract_batch();
        let fast = string_extract(&batch, config, limits);
        let reference = reference_string_extract(&batch, config, limits);
        match (fast, reference) {
            (Ok(fast), Ok(reference)) => assert_eq!(fast, reference),
            (fast, reference) => {
                assert_eq!(
                    fast.err().map(|error| error.to_string()),
                    reference.err().map(|error| error.to_string()),
                );
            }
        }
    }

    fn extract_config(pattern: &str, extract_all: bool) -> StringExtract {
        StringExtract {
            column: "text".into(),
            pattern: pattern.into(),
            output_column: Some("out".into()),
            extract_all,
        }
    }

    #[test]
    fn string_extract_fast_path_matches_reference() {
        // Gruppo anonimo singolo, match intero senza gruppi, extract_all con
        // join, gruppi nominati multipli, gruppo nominato opzionale non
        // partecipante, nomi duplicati, pattern vuoto (match vuoti con
        // avanzamento per code point Unicode), pattern su unicode.
        let configs = [
            extract_config("GEO(\\d{3})", false),
            extract_config("GEO\\d{3}", false),
            extract_config("(\\d+)", true),
            extract_config("\\d+", true),
            extract_config(
                "(?P<site>LO\\d{4})_(?P<area>[A-Z]{2}\\d{2})_(?P<sys>[A-Z]{2}\\d{2})_GEO(?P<num>\\d{3})",
                false,
            ),
            extract_config("(?P<word>\\p{L}+)(?P<tail>\\d+)?", false),
            extract_config("(?P<x>\\d+)|(?P<x>[a-z]+)", false),
            extract_config("", false),
            extract_config("", true),
            extract_config("(.)", true),
        ];
        for config in &configs {
            assert_extract_equivalent(config, &Limits::default());
        }

        // Regex non valida: stesso errore.
        assert_extract_equivalent(&extract_config("(", false), &Limits::default());

        // Nome di gruppo oltre 1024 byte: stesso errore di validazione.
        let long_name = format!("(?P<{}>x)", "a".repeat(1_050));
        assert_extract_equivalent(&extract_config(&long_name, false), &Limits::default());

        // max_regex_bytes: al limite passa, oltre fallisce con lo stesso errore.
        let tight = Limits {
            max_regex_bytes: 8,
            ..Limits::default()
        };
        assert_extract_equivalent(&extract_config(&"a".repeat(8), false), &tight);
        assert_extract_equivalent(&extract_config(&"a".repeat(9), false), &tight);
    }

    #[test]
    fn string_extract_semantics_on_unicode_and_nulls() {
        let batch = extract_batch();
        // extract_all con gruppi multipli su unicode: join con virgola dei
        // soli match, null propagato, nessun match -> null.
        let output = string_extract(
            &batch,
            &extract_config("(\\d+)", true),
            &Limits::default(),
        )
        .expect("extract_all");
        let column = output
            .column_by_name("out")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .expect("colonna out");
        assert_eq!(column.value(3), "99,100");
        assert_eq!(column.value(4), "42");
        assert!(column.is_null(8));
        assert!(column.is_null(9));

        // Gruppi nominati: una colonna per gruppo, nessun match -> null su
        // tutte le colonne del gruppo.
        let named = string_extract(
            &batch,
            &extract_config(
                "(?P<site>LO\\d{4})_(?P<area>[A-Z]{2}\\d{2})_(?P<sys>[A-Z]{2}\\d{2})_GEO(?P<num>\\d{3})",
                false,
            ),
            &Limits::default(),
        )
        .expect("named");
        let site = named
            .column_by_name("site")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .expect("colonna site");
        assert_eq!(site.value(0), "LO2244");
        assert!(site.is_null(2));
        let num = named
            .column_by_name("num")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .expect("colonna num");
        assert_eq!(num.value(0), "001");
        assert!(num.is_null(9));
    }

    #[test]
    fn text_normalize_fast_path_matches_reference_on_unicode_edges() {
        // Unicode complessi: accenti scomposti e precomposti, sigma greco
        // finale (regola contestuale di `str::to_lowercase`), İ turco
        // (combining dot), ß tedesco, legature, emoji con ZWJ, NBSP e figure
        // space (NFKD -> spazio), vuoto, solo spazi, null.
        let unicode_batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("text", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("  élÈVE   d'ÉCOLE  "),
                Some("e\u{0301} cafe\u{0300}"),
                Some("\u{00C9}\u{0301}"),
                Some("ΣΊΣΥΦΟΣ"),
                Some("ΟΔΥΣΣΕΎΣ Σας"),
                Some("İstanbul IĞDIR"),
                Some("straße straße"),
                Some("\u{FB01}le \u{FB00}nal"), // legature fi/ff
                Some("👨\u{200D}👩\u{200D}👧\u{200D}👦 emoji 🎉!"),
                Some("\u{00A0}nbsp\u{00A0}\u{00A0}"),
                Some("a\u{2007}b\tc\nd"),
                Some("hello WORLD-rust_lang 2024"),
                Some(""),
                Some("   \t  "),
                Some("àéîõü Çñ"),
                None,
            ]))],
        )
        .expect("fixture");
        let modes = [
            NormalizeOperation::Trim,
            NormalizeOperation::Lower,
            NormalizeOperation::Upper,
            NormalizeOperation::Title,
            NormalizeOperation::StripAccents,
            NormalizeOperation::StripDoubleSpaces,
            NormalizeOperation::Full,
        ];
        for (index, operation) in modes.into_iter().enumerate() {
            let config = TextNormalize {
                columns: vec!["text".into()],
                operations: operation,
                overwrite: index % 2 == 0,
            };
            let fast = text_normalize(&unicode_batch, &config, &Limits::default());
            let reference = reference_text_normalize(&unicode_batch, &config, &Limits::default());
            match (fast, reference) {
                (Ok(fast), Ok(reference)) => assert_eq!(fast, reference),
                (fast, reference) => assert_eq!(fast.is_err(), reference.is_err()),
            }
        }
        // max_string_bytes: stesso errore del riferimento.
        let tiny = Limits {
            max_string_bytes: 2,
            ..Limits::default()
        };
        let config = TextNormalize {
            columns: vec!["text".into()],
            operations: NormalizeOperation::Full,
            overwrite: true,
        };
        assert!(text_normalize(&unicode_batch, &config, &tiny).is_err());
        assert!(reference_text_normalize(&unicode_batch, &config, &tiny).is_err());
    }

    fn batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("text", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("  élÈVE   d'ÉCOLE  "),
                None,
            ]))],
        )
        .expect("fixture")
    }

    #[test]
    fn serde_defaults_and_padding_guards_are_covered() {
        let pad: StringPad = serde_json::from_value(json!({
            "column": "text", "output_column": null
        }))
        .expect("defaults");
        assert_eq!(pad.width, 5);
        assert_eq!(pad.fill_char, "0");
        assert!(matches!(pad.side, PadSide::Left));
        let output = string_pad(&batch(), &pad, &Limits::default()).expect("default pad");
        assert_eq!(output.num_columns(), 1);

        for fill_char in ["", "xx"] {
            let invalid = StringPad {
                column: "text".into(),
                width: 4,
                side: PadSide::Left,
                fill_char: fill_char.into(),
                output_column: None,
            };
            assert!(string_pad(&batch(), &invalid, &Limits::default()).is_err());
        }
        let right = StringPad {
            column: "text".into(),
            width: 24,
            side: PadSide::Right,
            fill_char: "x".into(),
            output_column: Some("right".into()),
        };
        assert!(string_pad(&batch(), &right, &Limits::default()).is_ok());

        let tiny = Limits {
            max_string_bytes: 3,
            ..Limits::default()
        };
        assert!(string_pad(&batch(), &right, &tiny).is_err());
    }

    #[test]
    fn every_normalization_mode_and_defaults_are_exercised() {
        let default: TextNormalize =
            serde_json::from_value(json!({"columns": ["text"]})).expect("defaults");
        assert!(default.overwrite);
        assert!(matches!(default.operations, NormalizeOperation::Full));

        let modes = [
            NormalizeOperation::Trim,
            NormalizeOperation::Lower,
            NormalizeOperation::Upper,
            NormalizeOperation::Title,
            NormalizeOperation::StripAccents,
            NormalizeOperation::StripDoubleSpaces,
            NormalizeOperation::Full,
        ];
        for (index, operation) in modes.into_iter().enumerate() {
            let output = text_normalize(
                &batch(),
                &TextNormalize {
                    columns: vec!["text".into()],
                    operations: operation,
                    overwrite: index % 2 == 0,
                },
                &Limits::default(),
            )
            .expect("normalize");
            assert_eq!(output.num_rows(), 2);
        }

        assert!(text_normalize(
            &batch(),
            &TextNormalize {
                columns: vec![],
                operations: NormalizeOperation::Full,
                overwrite: true,
            },
            &Limits::default(),
        )
        .is_err());
        let tiny = Limits {
            max_string_bytes: 2,
            ..Limits::default()
        };
        assert!(text_normalize(
            &batch(),
            &TextNormalize {
                columns: vec!["text".into()],
                operations: NormalizeOperation::Full,
                overwrite: true,
            },
            &tiny,
        )
        .is_err());
    }

    #[test]
    fn string_length_uses_default_output_name() {
        let output = string_length(
            &batch(),
            &StringLength {
                column: "text".into(),
                output_column: None,
            },
        )
        .expect("length");
        assert!(output.column_by_name("text_length").is_some());
    }
}
