//! Operazioni sui testi: `table.string_pad`, `table.string_length`,
//! `table.string_extract`, `table.text_normalize`.
//!
//! Semantica, schema, ordine ed errori per operazione: le schede
//! `docs/schede/<id>.md`, raccolte in `docs/operazioni.md`.

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

/// Config di `table.string_pad`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringPad {
    /// Colonna `Utf8` da allungare (obbligatorio).
    pub column: String,
    /// Lunghezza minima in code point (default 5), almeno 1 (con 0 nessun
    /// valore si allunga e `side` e `fill_char` non avrebbero effetto,
    /// [`StringPad::verifica_parametri`]); l'analisi la limita a
    /// `max_string_bytes`.
    #[serde(default = "default_width")]
    pub width: usize,
    /// Lato del riempimento (default `left`).
    #[serde(default = "default_side")]
    pub side: PadSide,
    /// Carattere di riempimento, esattamente un code point (default `"0"`).
    #[serde(default = "default_fill")]
    pub fill_char: String,
    /// Colonna d'uscita; assente o `null`: si sostituisce `column`.
    pub output_column: Option<String>,
}

/// Lato del riempimento di `table.string_pad` (`"left"`, `"right"`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadSide {
    /// Riempimento a sinistra (`"left"`).
    Left,
    /// Riempimento a destra (`"right"`).
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

impl StringPad {
    /// Regole sulla sola config, condivise da kernel e analisi dei
    /// contratti: `fill_char` di un solo code point; `width` almeno 1 (con 0
    /// il passo non cambia nessun valore). Il tetto di `width` contro
    /// `max_string_bytes` lo mette l'analisi; nel kernel un valore allungato
    /// oltre il tetto e' un `ResourceLimit`.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per ciascuna delle regole.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.fill_char.chars().count() != 1 {
            return Err(PlenoraError::InvalidPlan(
                "fill_char deve essere un singolo carattere".into(),
            ));
        }
        if self.width == 0 {
            return Err(PlenoraError::InvalidPlan(
                "width 0 non allunga nessun valore: side e fill_char senza effetto".into(),
            ));
        }
        Ok(())
    }
}

/// Allunga i valori a `width` code point con `fill_char` (`table.string_pad`).
///
/// I valori gia' lunghi almeno `width` restano invariati (nessun
/// troncamento); i null restano null. L'uscita (`Utf8` nullable) sostituisce
/// la colonna omonima o si aggiunge in coda.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna d'uscita non valido; le regole di
///   [`StringPad::verifica_parametri`] (`fill_char` di un solo code point,
///   `width` almeno 1);
/// - `ResourceLimit`: risultato oltre `limits.max_string_bytes`;
/// - `Schema`: colonna assente o non `Utf8`;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
pub fn string_pad(batch: &RecordBatch, config: &StringPad, limits: &Limits) -> Result<RecordBatch> {
    let output_name = config.output_column.as_deref().unwrap_or(&config.column);
    validate_output_name(output_name)?;
    config.verifica_parametri()?;
    let fill_char = config
        .fill_char
        .chars()
        .next()
        .ok_or_else(|| PlenoraError::Internal("fill_char verificato e vuoto".into()))?;
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
            return Err(PlenoraError::ResourceLimit(
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

/// Config di `table.string_length`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringLength {
    /// Colonna `Utf8` da misurare (obbligatorio).
    pub column: String,
    /// Colonna d'uscita; assente o `null`: `<column>_length`.
    pub output_column: Option<String>,
}

/// Colonna `Int64` con la lunghezza dei valori in code point
/// (`table.string_length`); null -> null.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna d'uscita non valido;
/// - `ResourceLimit`: conteggio non rappresentabile come `i64`;
/// - `Schema`: colonna assente o non `Utf8`;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
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
                    .map_err(|_| PlenoraError::ResourceLimit("stringa troppo lunga".into()))
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

/// Config di `table.string_extract`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringExtract {
    /// Colonna `Utf8` in cui cercare (obbligatorio).
    pub column: String,
    /// Espressione regolare (sintassi del crate `regex`, obbligatorio);
    /// l'analisi rifiuta il pattern vuoto.
    pub pattern: String,
    /// Colonna d'uscita senza gruppi con nome; assente o `null`:
    /// `<column>_extracted`. Con gruppi con nome si rifiuta.
    pub output_column: Option<String>,
    /// Unisce con `","` i valori di tutti i match (default `false`). Con
    /// gruppi con nome si rifiuta.
    #[serde(default)]
    pub extract_all: bool,
}

// Fast path `string_extract`: una ricerca per riga con `CaptureLocations`
// riusato e slice scritte direttamente negli `StringBuilder`; `extract_all`
// usa `captures_iter` (avanzamento dei match vuoti per code point) con uno
// scratch riusato. Semantica byte-identica al percorso generico: null,
// nessun match e gruppo non partecipante danno null; stessi errori nello
// stesso ordine.

fn utf8_data_len(values: &StringArray) -> usize {
    let offsets = values.offsets();
    usize::try_from(offsets[values.len()] - offsets[0]).unwrap_or(0)
}

/// Rifiuta `output_column` ed `extract_all` insieme ai gruppi con nome.
///
/// Con gruppi con nome `string_extract` produce una colonna per gruppo, col
/// nome del gruppo, dal primo match: i due parametri non avrebbero effetto.
/// Si rifiutano invece di essere ignorati: dare loro un significato
/// (prefisso? match concatenati per gruppo?) sarebbe una semantica nuova,
/// non scritta da nessuna parte.
///
/// La chiamano il kernel e l'analisi dei contratti.
///
/// # Errors
///
/// `InvalidPlan`: gruppi con nome insieme a `output_column` o a
/// `extract_all`.
pub fn verifica_gruppi_con_nome(config: &StringExtract, regex: &Regex) -> Result<()> {
    if regex.capture_names().flatten().next().is_none() {
        return Ok(());
    }
    if config.output_column.is_some() {
        return Err(PlenoraError::InvalidPlan(
            "output_column non ammesso con gruppi con nome: le colonne prendono il nome dei gruppi"
                .into(),
        ));
    }
    if config.extract_all {
        return Err(PlenoraError::InvalidPlan(
            "extract_all non ammesso con gruppi con nome: si estrae il primo match".into(),
        ));
    }
    Ok(())
}

/// Estrazione regex dalla colonna (`table.string_extract`).
///
/// Gruppi con nome -> una colonna per gruppo, dal primo match; altrimenti
/// una colonna con il primo gruppo di cattura (o il match intero se non ci
/// sono gruppi).
///
/// Con `extract_all` i valori di tutti i match si uniscono con una virgola,
/// saltando i match in cui il gruppo non partecipa; nessun match, gruppo non
/// partecipante e null producono null.
///
/// # Errors
///
/// - `InvalidPlan`: pattern oltre `limits.max_regex_bytes`, regex non valida,
///   nome di colonna d'uscita (esplicito o da gruppo con nome) non valido,
///   gruppi con nome insieme a `output_column` o `extract_all`
///   ([`verifica_gruppi_con_nome`]);
/// - `Schema`: colonna assente o non `Utf8`;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
// Tre forme di output (gruppi nominati, gruppo singolo, extract_all) su una
// sola passata di righe: sequenza lineare di casi, lunga per costruzione.
#[allow(clippy::too_many_lines)]
pub fn string_extract(
    batch: &RecordBatch,
    config: &StringExtract,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.pattern.len() > limits.max_regex_bytes {
        return Err(PlenoraError::InvalidPlan(
            "pattern oltre max_regex_bytes".into(),
        ));
    }
    let regex = Regex::new(&config.pattern).map_err(|error| {
        PlenoraError::InvalidPlan(crate::motivo_regex_non_valida(&error).into())
    })?;
    verifica_gruppi_con_nome(config, &regex)?;
    let input = utf8_column(batch, &config.column)?;
    let named: Vec<(usize, String)> = regex
        .capture_names()
        .enumerate()
        .filter_map(|(index, name)| name.map(|name| (index, name.to_owned())))
        .collect();
    if !named.is_empty() {
        // Validazione dei nomi in anticipo nello stesso ordine del percorso
        // generico: la costruzione dei valori non puo' fallire, quindi il
        // primo nome non valido produce lo stesso errore nei due percorsi.
        for (_, name) in &named {
            validate_output_name(name)?;
        }
        let per_group_capacity = utf8_data_len(input) / named.len() + 16;
        let mut builders: Vec<StringBuilder> = named
            .iter()
            .map(|_| StringBuilder::with_capacity(batch.num_rows(), per_group_capacity))
            .collect();
        let mut locations = regex.capture_locations();
        for value in input {
            match value {
                Some(text) if regex.captures_read(&mut locations, text).is_some() => {
                    for (builder, (capture_index, _)) in builders.iter_mut().zip(&named) {
                        match locations.get(*capture_index) {
                            Some((start, end)) => builder.append_value(&text[start..end]),
                            None => builder.append_null(),
                        }
                    }
                }
                _ => builders.iter_mut().for_each(StringBuilder::append_null),
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
        for value in input {
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
                        // I match uniti con la virgola possono superare la
                        // cella (match vuoti): testo prodotto.
                        crate::verifica_testo_prodotto("string_extract", scratch.len(), limits)?;
                        builder.append_value(&scratch);
                    } else {
                        builder.append_null();
                    }
                }
            }
        }
    } else {
        let mut locations = regex.capture_locations();
        for value in input {
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

/// Regola di `table.text_normalize` (una sola per passo).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizeOperation {
    /// Toglie gli spazi Unicode ai lati (`"trim"`).
    Trim,
    /// Minuscole Unicode (`"lower"`).
    Lower,
    /// Maiuscole Unicode (`"upper"`).
    Upper,
    /// Maiuscola la prima lettera o cifra di ogni parola, minuscole le altre;
    /// una parola comincia dopo ogni carattere non alfanumerico (`"title"`).
    Title,
    /// Decomposizione NFKD e rimozione dei segni combinanti
    /// (`"strip_accents"`).
    StripAccents,
    /// Spezza sugli spazi Unicode e riunisce con uno spazio solo
    /// (`"strip_double_spaces"`).
    StripDoubleSpaces,
    /// `trim`, `lower`, `strip_accents`, `strip_double_spaces` in sequenza
    /// (`"full"`).
    Full,
}

/// Config di `table.text_normalize`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextNormalize {
    /// Colonne `Utf8` da normalizzare (obbligatorio, almeno una).
    pub columns: Vec<String>,
    /// La regola da applicare, una sola nonostante il plurale (default
    /// `full`).
    #[serde(default = "default_normalize")]
    pub operations: NormalizeOperation,
    /// Sostituisce le colonne (default `true`); con `false` scrive
    /// `<colonna>_norm`.
    #[serde(default = "default_true")]
    pub overwrite: bool,
}

const fn default_normalize() -> NormalizeOperation {
    NormalizeOperation::Full
}
const fn default_true() -> bool {
    true
}

// Fast path `text_normalize`:
// le regole di normalizzazione scrivono in un buffer riusato tra le righe,
// senza allocazioni intermedie (niente `Vec<char>` per carattere in title
// case, niente `Vec<&str>` + join nel collapse, passata unica nfkd + filtro
// combining + collapse in `Full`). Semantica byte-identica: il lowercase
// resta `str::to_lowercase` (regola contestuale del sigma greco finale).

fn strip_accents_into(value: &str, out: &mut String) {
    out.extend(
        value
            .nfkd()
            .filter(|character| !is_combining_mark(*character)),
    );
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
    for character in lowered
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
    {
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

/// Normalizza le colonne di testo secondo `config.operations`
/// (`table.text_normalize`).
///
/// Con `overwrite` le colonne si sostituiscono, altrimenti il risultato va in
/// `<colonna>_norm`. I null restano null.
///
/// # Errors
///
/// - `InvalidPlan`: `columns` vuoto o nome della colonna d'uscita non
///   valido;
/// - `ResourceLimit`: risultato oltre `limits.max_string_bytes`;
/// - `Schema`: colonna assente o non `Utf8`;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
pub fn text_normalize(
    batch: &RecordBatch,
    config: &TextNormalize,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
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
                return Err(PlenoraError::ResourceLimit(
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
    use serde_json::json;

    use super::*;
    use crate::test_support::single_column_batch;

    // -----------------------------------------------------------------------
    // Implementazioni generiche, indipendenti dai fast path: riferimento
    // per l'equivalenza
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
            NormalizeOperation::Full => reference_collapse_whitespace(&reference_strip_accents(
                &value.trim().to_lowercase(),
            )),
        }
    }

    fn reference_text_normalize(
        batch: &RecordBatch,
        config: &TextNormalize,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        if config.columns.is_empty() {
            return Err(PlenoraError::InvalidPlan(
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
                        // Limite di risorsa, come documentato dal kernel.
                        return Err(PlenoraError::ResourceLimit(
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
    // Implementazione di riferimento di `string_extract`, indipendente dal
    // fast path: per
    // l'equivalenza semantica (oracolo) del fast path.
    // -----------------------------------------------------------------------

    fn reference_string_extract(
        batch: &RecordBatch,
        config: &StringExtract,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        if config.pattern.len() > limits.max_regex_bytes {
            return Err(PlenoraError::InvalidPlan(
                "pattern oltre max_regex_bytes".into(),
            ));
        }
        let regex = Regex::new(&config.pattern).map_err(|error| {
            PlenoraError::InvalidPlan(crate::motivo_regex_non_valida(&error).into())
        })?;
        verifica_gruppi_con_nome(config, &regex)?;
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
        single_column_batch(
            "text",
            Arc::new(StringArray::from(vec![
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
            ])),
            DataType::Utf8,
            true,
        )
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

    /// Config di prova: `output_column` solo senza gruppi con nome, dove ha
    /// effetto (con i gruppi con nome e' un errore di config).
    fn extract_config(pattern: &str, extract_all: bool) -> StringExtract {
        let con_nome =
            Regex::new(pattern).is_ok_and(|regex| regex.capture_names().flatten().next().is_some());
        StringExtract {
            column: "text".into(),
            pattern: pattern.into(),
            output_column: (!con_nome).then(|| "out".into()),
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
        let output = string_extract(&batch, &extract_config("(\\d+)", true), &Limits::default())
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
        let unicode_batch = single_column_batch(
            "text",
            Arc::new(StringArray::from(vec![
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
            ])),
            DataType::Utf8,
            true,
        );
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
            // Input tutto valido: l'unico esito accettato e' lo stesso batch.
            let fast = text_normalize(&unicode_batch, &config, &Limits::default())
                .expect("fast path rifiuta input valido");
            let reference = reference_text_normalize(&unicode_batch, &config, &Limits::default())
                .expect("riferimento rifiuta input valido");
            assert_eq!(fast, reference);
        }
        // max_string_bytes: stesso errore del riferimento, non solo un errore.
        let tiny = Limits {
            max_string_bytes: 2,
            ..Limits::default()
        };
        let config = TextNormalize {
            columns: vec!["text".into()],
            operations: NormalizeOperation::Full,
            overwrite: true,
        };
        let fast = text_normalize(&unicode_batch, &config, &tiny)
            .expect_err("fast path oltre max_string_bytes");
        let reference = reference_text_normalize(&unicode_batch, &config, &tiny)
            .expect_err("riferimento oltre max_string_bytes");
        assert_eq!(fast.category(), reference.category());
        assert_eq!(fast.to_string(), reference.to_string());
        assert!(
            matches!(fast, PlenoraError::ResourceLimit(_)),
            "attesa ResourceLimit: {fast}"
        );
    }

    fn batch() -> RecordBatch {
        single_column_batch(
            "text",
            Arc::new(StringArray::from(vec![Some("  élÈVE   d'ÉCOLE  "), None])),
            DataType::Utf8,
            true,
        )
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
