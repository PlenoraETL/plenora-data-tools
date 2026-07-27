//! `table.fuzzy_join` (estensione table v1.3): join per similarita' testuale
//! su anagrafiche sporche (BinaryOrdered, Blocking, BoundaryOnly).
//!
//! Semantica documentata (v1, deliberatamente semplice):
//! - coppie candidate SOLO via blocking sul lato destro (`prefix` = primi N
//!   caratteri della chiave normalizzata uguali, `soundex` = stesso codice
//!   soundex, `none` = tutte le coppie: il blocco unico copre l'intero lato
//!   destro e scatta subito il limite `max_candidates`);
//! - ogni coppia candidata con score >= `threshold` produce UNA riga di
//!   output (come un join, nessun best-match per riga); la colonna score e'
//!   Float64 (`score_column`, default `score`);
//! - `how = inner` (default): solo le coppie che superano soglia; `how =
//!   left`: le righe sinistre senza alcuna coppia a soglia compaiono una
//!   volta con colonne destre e score null;
//! - chiavi null non matchano mai (in `left` compaiono come non matchate);
//! - normalizzazione: case-insensitive di default (lowercase Unicode) sia per
//!   le metriche sia per il blocking; `case_sensitive = true` la disattiva
//!   (soundex resta intrinsecamente case-insensitive);
//! - schema di output: naming del join Manipola (`combine_horizontal` con
//!   `left_keys`): la chiave sinistra conserva il nome, le altre colonne
//!   sinistre prendono suffisso `_L`, TUTTE le colonne destre (chiave
//!   inclusa, a differenza del join esatto: nel fuzzy le due chiavi differiscono
//!   e il valore destro e' parte del risultato) prendono suffisso `_R`;
//!   in coda la colonna score (nullable solo con `how = left`);
//! - ordine di output deterministico: scansione delle righe sinistre in
//!   ordine, poi candidate destre in ordine di indice destro; nessuna
//!   iterazione su hash map influenza l'ordine (i blocchi sono `Vec` in
//!   ordine di inserzione);
//! - limiti fail-closed: un blocco destro con piu' di `max_candidates` righe
//!   abortisce con errore `Contract` (default 50); le righe di output non
//!   possono superare `limits.max_rows`.
//!
//! Metriche (implementate a mano, nessuna dipendenza nuova):
//! - `jaro_winkler`: similarita' di Jaro su caratteri Unicode con boost di
//!   prefisso di Winkler (p = 0.1, prefisso massimo 4 caratteri, nessuna
//!   soglia minima di attivazione del boost);
//! - `levenshtein`: distanza di edit su caratteri Unicode normalizzata come
//!   `1 - dist/max_len` (due stringhe vuote -> 1.0);
//! - `jaccard`: coefficiente di Jaccard sui token (split su whitespace,
//!   insiemi; due stringhe senza token -> 1.0).
//!
//! Soundex: American Soundex classico sulle sole lettere ASCII (le altre,
//!   cifre e caratteri non ASCII inclusi, sono ignorate e non interrompono le
//!   run); vocali che separano lettere dello stesso codice le fanno codificare
//!   due volte, `h`/`w` no.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde::Deserialize;

use crate::joins::{combine_horizontal, FastHasher, HorizontalNames};
use crate::{utf8_column, validate_output_name, Limits};
use plenora_core::{PlenoraError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuzzyMetric {
    JaroWinkler,
    Levenshtein,
    Jaccard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuzzyBlocking {
    Prefix,
    Soundex,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuzzyHow {
    Inner,
    Left,
}
const fn default_how() -> FuzzyHow {
    FuzzyHow::Inner
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuzzyJoin {
    pub left_key: String,
    pub right_key: String,
    pub metric: FuzzyMetric,
    pub threshold: f64,
    pub blocking: FuzzyBlocking,
    pub blocking_param: Option<usize>,
    #[serde(default = "default_how")]
    pub how: FuzzyHow,
    pub score_column: Option<String>,
    pub max_candidates: Option<usize>,
    #[serde(default)]
    pub case_sensitive: bool,
}

/// Default documentati della config (usati da kernel, analisi e validazione
/// engine: un'unica fonte).
pub const DEFAULT_PREFIX_LEN: usize = 2;
pub const DEFAULT_MAX_CANDIDATES: usize = 50;
pub const DEFAULT_SCORE_COLUMN: &str = "score";

impl FuzzyJoin {
    /// Lunghezza effettiva del prefisso di blocking.
    pub(crate) fn prefix_len(&self) -> usize {
        self.blocking_param.unwrap_or(DEFAULT_PREFIX_LEN)
    }

    /// Limite effettivo di candidati per blocco.
    pub(crate) fn max_candidates(&self) -> usize {
        self.max_candidates.unwrap_or(DEFAULT_MAX_CANDIDATES)
    }

    /// Nome effettivo della colonna score.
    pub(crate) fn score_name(&self) -> &str {
        self.score_column.as_deref().unwrap_or(DEFAULT_SCORE_COLUMN)
    }
}

/// Validazioni statiche della config (replicate dall'analisi a secco e dalla
/// validazione engine: stesse regole, stessi messaggi).
pub fn validate_config(config: &FuzzyJoin) -> Result<()> {
    if !config.threshold.is_finite()
        || config.threshold <= 0.0
        || config.threshold > 1.0
    {
        return Err(PlenoraError::Contract(
            "threshold deve essere in (0, 1]".into(),
        ));
    }
    match config.blocking {
        FuzzyBlocking::Prefix => {
            if config.prefix_len() == 0 {
                return Err(PlenoraError::Contract(
                    "blocking_param deve essere >= 1".into(),
                ));
            }
        }
        FuzzyBlocking::Soundex | FuzzyBlocking::None => {
            if config.blocking_param.is_some() {
                return Err(PlenoraError::Contract(
                    "blocking_param ammesso solo con blocking=prefix".into(),
                ));
            }
        }
    }
    if config.max_candidates() == 0 {
        return Err(PlenoraError::Contract(
            "max_candidates deve essere >= 1".into(),
        ));
    }
    validate_output_name(config.score_name())
}

fn soundex_code(letter: char) -> u8 {
    match letter {
        'b' | 'f' | 'p' | 'v' => 1,
        'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => 2,
        'd' | 't' => 3,
        'l' => 4,
        'm' | 'n' => 5,
        'r' => 6,
        _ => 0,
    }
}

/// American Soundex classico: prima lettera + 3 cifre (padding di zeri).
/// Solo lettere ASCII; gli altri caratteri sono ignorati e non interrompono
/// le run di lettere con lo stesso codice (come `h`/`w`, che non azzerano il
/// codice precedente; le vocali invece si').
pub(crate) fn soundex(text: &str) -> String {
    let mut letters = text
        .chars()
        .filter_map(|c| {
            let upper = c.to_ascii_uppercase();
            upper.is_ascii_uppercase().then_some(upper)
        })
        .map(|c| c.to_ascii_lowercase());
    let Some(first) = letters.next() else {
        return String::new();
    };
    let mut out = String::with_capacity(4);
    out.push(first.to_ascii_uppercase());
    let mut previous = soundex_code(first);
    for letter in letters {
        let code = soundex_code(letter);
        if code != 0 && code != previous {
            out.push(char::from(b'0' + code));
        }
        if !matches!(letter, 'h' | 'w') {
            previous = code;
        }
    }
    out.truncate(4);
    while out.len() < 4 {
        out.push('0');
    }
    out
}

fn jaro_similarity(left: &[char], right: &[char]) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let window = (left.len().max(right.len()) / 2).saturating_sub(1);
    let mut left_matched = vec![false; left.len()];
    let mut right_matched = vec![false; right.len()];
    let mut matches = 0_usize;
    for (index, &letter) in left.iter().enumerate() {
        let start = index.saturating_sub(window);
        let end = (index + window + 1).min(right.len());
        for candidate in start..end {
            if !right_matched[candidate] && right[candidate] == letter {
                left_matched[index] = true;
                right_matched[candidate] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }
    let mut transpositions = 0_usize;
    let mut cursor = 0_usize;
    for (index, &letter) in left.iter().enumerate() {
        if left_matched[index] {
            while !right_matched[cursor] {
                cursor += 1;
            }
            if letter != right[cursor] {
                transpositions += 1;
            }
            cursor += 1;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let matches = matches as f64;
    #[allow(clippy::cast_precision_loss)]
    let score = (matches / left.len() as f64
        + matches / right.len() as f64
        + (matches - transpositions as f64 / 2.0) / matches)
        / 3.0;
    score
}

/// Jaro-Winkler: Jaro + boost di prefisso comune (p = 0.1, massimo 4
/// caratteri). Nessuna soglia minima di attivazione del boost.
pub(crate) fn jaro_winkler(left: &str, right: &str) -> f64 {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let jaro = jaro_similarity(&left, &right);
    let prefix = left
        .iter()
        .zip(&right)
        .take(4)
        .take_while(|(a, b)| a == b)
        .count();
    #[allow(clippy::cast_precision_loss)]
    let boost = prefix as f64 * 0.1 * (1.0 - jaro);
    jaro + boost
}

/// Distanza di Levenshtein su caratteri Unicode (DP a due righe).
fn levenshtein_distance(left: &[char], right: &[char]) -> usize {
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0_usize; right.len() + 1];
    for (row, &a) in left.iter().enumerate() {
        current[0] = row + 1;
        for (col, &b) in right.iter().enumerate() {
            let substitution = previous[col] + usize::from(a != b);
            current[col + 1] = (previous[col + 1] + 1).min(current[col] + 1).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// Levenshtein normalizzato: `1 - dist/max_len`; due stringhe vuote -> 1.0.
pub(crate) fn levenshtein_normalized(left: &str, right: &str) -> f64 {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let max_len = left.len().max(right.len());
    if max_len == 0 {
        return 1.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let score = 1.0 - levenshtein_distance(&left, &right) as f64 / max_len as f64;
    score
}

/// Jaccard sui token (split whitespace, insiemi); due stringhe senza token
/// -> 1.0.
pub(crate) fn jaccard_tokens(left: &str, right: &str) -> f64 {
    let left_tokens: std::collections::HashSet<&str> = left.split_whitespace().collect();
    let right_tokens: std::collections::HashSet<&str> = right.split_whitespace().collect();
    if left_tokens.is_empty() && right_tokens.is_empty() {
        return 1.0;
    }
    let intersection = left_tokens.intersection(&right_tokens).count();
    let union = left_tokens.len() + right_tokens.len() - intersection;
    #[allow(clippy::cast_precision_loss)]
    let score = intersection as f64 / union as f64;
    score
}

fn score(metric: FuzzyMetric, left: &str, right: &str) -> f64 {
    match metric {
        FuzzyMetric::JaroWinkler => jaro_winkler(left, right),
        FuzzyMetric::Levenshtein => levenshtein_normalized(left, right),
        FuzzyMetric::Jaccard => jaccard_tokens(left, right),
    }
}

/// Join per similarita' testuale (estensione v1.3): vedi la documentazione di
/// modulo per la semantica completa.
pub fn fuzzy_join(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &FuzzyJoin,
    limits: &Limits,
) -> Result<RecordBatch> {
    validate_config(config)?;
    let left_index = left
        .schema()
        .index_of(&config.left_key)
        .map_err(|_| PlenoraError::Schema(format!("colonna non trovata: {}", config.left_key)))?;
    let left_keys = utf8_column(left, &config.left_key)?;
    let right_keys = utf8_column(right, &config.right_key)?;
    let normalize = |value: &str| {
        if config.case_sensitive {
            value.to_owned()
        } else {
            value.to_lowercase()
        }
    };
    let block_key = |text: &str| match config.blocking {
        FuzzyBlocking::Prefix => text.chars().take(config.prefix_len()).collect::<String>(),
        FuzzyBlocking::Soundex => soundex(text),
        FuzzyBlocking::None => String::new(),
    };
    // Build sul lato destro: blocco -> righe destre in ordine di indice.
    let left_norm: Vec<Option<String>> = left_keys
        .iter()
        .map(|value| value.map(&normalize))
        .collect();
    let right_norm: Vec<Option<String>> = right_keys
        .iter()
        .map(|value| value.map(&normalize))
        .collect();
    let mut blocks: HashMap<String, Vec<usize>, FastHasher> = HashMap::default();
    for (row, value) in right_norm.iter().enumerate() {
        if let Some(value) = value {
            blocks.entry(block_key(value)).or_default().push(row);
        }
    }
    let max_candidates = config.max_candidates();
    for rows in blocks.values() {
        if rows.len() > max_candidates {
            return Err(PlenoraError::Contract(format!(
                "fuzzy_join: blocco con {} candidati oltre max_candidates {max_candidates}",
                rows.len()
            )));
        }
    }
    // Probe: scansione sinistra in ordine, candidate destre in ordine di
    // indice (i `Vec` dei blocchi preservano l'ordine di inserzione).
    let mut left_rows: Vec<Option<usize>> = Vec::new();
    let mut right_rows: Vec<Option<usize>> = Vec::new();
    let mut scores: Vec<Option<f64>> = Vec::new();
    for (left_row, value) in left_norm.iter().enumerate() {
        let mut matched = false;
        if let Some(value) = value {
            if let Some(candidates) = blocks.get(&block_key(value)) {
                for &right_row in candidates {
                    let right_value = right_norm[right_row].as_ref().ok_or_else(|| {
                        PlenoraError::Contract(
                            "internal error: righe destre nei blocchi non sono null".into(),
                        )
                    })?;
                    let similarity = score(config.metric, value, right_value);
                    if similarity >= config.threshold {
                        left_rows.push(Some(left_row));
                        right_rows.push(Some(right_row));
                        scores.push(Some(similarity));
                        matched = true;
                    }
                }
            }
        }
        if !matched && config.how == FuzzyHow::Left {
            left_rows.push(Some(left_row));
            right_rows.push(None);
            scores.push(None);
        }
        if left_rows.len() > limits.max_rows {
            return Err(PlenoraError::Contract("fuzzy_join supera max_rows".into()));
        }
    }
    let mut output = combine_horizontal(
        left,
        right,
        &left_rows,
        &right_rows,
        &[],
        HorizontalNames::ManipolaJoin {
            left_keys: &[left_index],
        },
        limits,
    )?;
    // Colonna score in coda: Float64, nullable solo con how=left (righe
    // sinistre non matchate); collisione di nome -> fail-closed.
    let score_name = config.score_name();
    if output.schema().index_of(score_name).is_ok() {
        return Err(PlenoraError::Schema(format!(
            "collisione fuzzy_join: {score_name}"
        )));
    }
    let mut fields: Vec<Field> = output
        .schema()
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    fields.push(Field::new(
        score_name,
        DataType::Float64,
        config.how == FuzzyHow::Left,
    ));
    if fields.len() > limits.max_columns {
        return Err(PlenoraError::Contract(
            "fuzzy_join supera max_columns".into(),
        ));
    }
    let mut columns = output.columns().to_vec();
    columns.push(Arc::new(Float64Array::from(scores)));
    let schema = Schema::new_with_metadata(fields, output.schema().metadata().clone());
    output = RecordBatch::try_new(Arc::new(schema), columns)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plenora_core::arrow::array::{Array, ArrayRef, Int64Array, StringArray};

    fn batch(pairs: Vec<(&str, ArrayRef)>) -> RecordBatch {
        let fields = pairs
            .iter()
            .map(|(name, column)| Field::new(*name, column.data_type().clone(), true))
            .collect::<Vec<_>>();
        let columns = pairs.into_iter().map(|(_, column)| column).collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("batch di test")
    }

    fn utf8_column_of(values: &[Option<&str>]) -> ArrayRef {
        Arc::new(StringArray::from(values.to_vec()))
    }

    fn i64_column_of(values: &[i64]) -> ArrayRef {
        Arc::new(Int64Array::from(values.to_vec()))
    }

    fn config(json: serde_json::Value) -> FuzzyJoin {
        serde_json::from_value(json).expect("config di test")
    }

    fn base_config() -> serde_json::Value {
        serde_json::json!({
            "left_key": "name",
            "right_key": "name",
            "metric": "jaro_winkler",
            "threshold": 0.9,
            "blocking": "prefix",
        })
    }

    fn people() -> (RecordBatch, RecordBatch) {
        let left = batch(vec![
            ("name", utf8_column_of(&[Some("Martha"), Some("Müller"), None])),
            ("lv", i64_column_of(&[1, 2, 3])),
        ]);
        let right = batch(vec![
            ("name", utf8_column_of(&[Some("Marhta"), Some("Muller"), None])),
            ("rv", i64_column_of(&[10, 20, 30])),
        ]);
        (left, right)
    }

    fn scores_of(output: &RecordBatch) -> Vec<Option<f64>> {
        output
            .column(output.num_columns() - 1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("colonna score Float64")
            .iter()
            .collect()
    }

    // -- Metriche su coppie note ---------------------------------------------

    #[test]
    fn jaro_winkler_matches_reference_values() {
        let martha = jaro_winkler("MARTHA", "MARHTA");
        assert!((martha - 0.9611).abs() < 1e-3, "jaro_winkler(MARTHA, MARHTA) = {martha}");
        assert!((jaro_winkler("abc", "abc") - 1.0).abs() < f64::EPSILON);
        assert!((jaro_winkler("", "")).abs() - 1.0 < f64::EPSILON);
        assert!((jaro_winkler("abc", "xyz")).abs() < f64::EPSILON);
        assert!((jaro_winkler("", "abc")).abs() < f64::EPSILON);
        // Simmetrica.
        let dwight = jaro_winkler("DWAYNE", "DUANE");
        assert!((dwight - 0.84).abs() < 1e-2, "jaro_winkler(DWAYNE, DUANE) = {dwight}");
    }

    #[test]
    fn levenshtein_normalized_matches_reference_values() {
        let kitten = levenshtein_normalized("kitten", "sitting");
        assert!((kitten - (1.0 - 3.0 / 7.0)).abs() < 1e-9, "kitten/sitting = {kitten}");
        assert!((levenshtein_normalized("abc", "abc") - 1.0).abs() < f64::EPSILON);
        assert!((levenshtein_normalized("", "abc")).abs() < f64::EPSILON);
        assert!((levenshtein_normalized("", "") - 1.0).abs() < f64::EPSILON);
        // Unicode: distanza su caratteri, non byte.
        assert!((levenshtein_normalized("müller", "muller") - (1.0 - 1.0 / 6.0)).abs() < 1e-9);
    }

    #[test]
    fn jaccard_matches_reference_values() {
        let score = jaccard_tokens("new york", "new jersey");
        assert!((score - 1.0 / 3.0).abs() < 1e-9, "jaccard = {score}");
        assert!((jaccard_tokens("a b", "a b") - 1.0).abs() < f64::EPSILON);
        assert!((jaccard_tokens("a b", "c d")).abs() < f64::EPSILON);
        assert!((jaccard_tokens("  ", "") - 1.0).abs() < f64::EPSILON);
        // Token ripetuti contano una volta (insiemi).
        assert!((jaccard_tokens("a a b", "a b") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn soundex_matches_reference_codes() {
        assert_eq!(soundex("Robert"), "R163");
        assert_eq!(soundex("Rupert"), "R163");
        assert_eq!(soundex("Ashcraft"), "A261");
        assert_eq!(soundex("Tymczak"), "T522");
        assert_eq!(soundex("Pfister"), "P236");
        assert_eq!(soundex(""), "");
        assert_eq!(soundex("123"), "");
        assert_eq!(soundex("a"), "A000");
    }

    // -- Config fail-closed ----------------------------------------------------

    #[test]
    fn config_is_strict_and_validated() {
        // Campo ignoto rifiutato (deny_unknown_fields).
        assert!(serde_json::from_value::<FuzzyJoin>(serde_json::json!({
            "left_key": "a", "right_key": "b", "metric": "jaccard",
            "threshold": 0.5, "blocking": "prefix", "surprise": 1
        }))
        .is_err());
        // Soglia ai bordi: 0 escluso, 1 incluso (NaN/infinito non sono
        // rappresentabili in JSON: la deserializzazione fail-closed li
        // rifiuta prima della validazione).
        for bad in [0.0, -0.5, 1.000_000_1] {
            let mut cfg = base_config();
            cfg["threshold"] = serde_json::json!(bad);
            assert!(validate_config(&config(cfg)).is_err(), "threshold {bad}");
        }
        let mut nan = base_config();
        nan["threshold"] = serde_json::json!(f64::NAN);
        assert!(serde_json::from_value::<FuzzyJoin>(nan).is_err());
        let mut one = base_config();
        one["threshold"] = serde_json::json!(1.0);
        assert!(validate_config(&config(one)).is_ok());
        // blocking_param solo con prefix, e >= 1.
        let mut cfg = base_config();
        cfg["blocking_param"] = serde_json::json!(0);
        assert!(validate_config(&config(cfg)).is_err());
        let mut cfg = base_config();
        cfg["blocking"] = serde_json::json!("soundex");
        cfg["blocking_param"] = serde_json::json!(3);
        assert!(validate_config(&config(cfg)).is_err());
        // max_candidates >= 1.
        let mut cfg = base_config();
        cfg["max_candidates"] = serde_json::json!(0);
        assert!(validate_config(&config(cfg)).is_err());
    }

    // -- Semantica del kernel ---------------------------------------------------

    #[test]
    fn inner_join_emits_pairs_over_threshold_with_score() {
        let (left, right) = people();
        let cfg = config(base_config());
        let output = fuzzy_join(&left, &right, &cfg, &Limits::default()).expect("join");
        // "martha"/"marhta" matchano (prefix "ma"), "müller"/"muller" no
        // (prefix "mü" vs "mu"); il null non matcha.
        assert_eq!(output.num_rows(), 1);
        let scores = scores_of(&output);
        assert!((scores[0].expect("score") - 0.9611).abs() < 1e-3);
        // Naming: chiave sinistra conserva il nome, altre `_L`, destre `_R`
        // (chiave destra inclusa), score in coda.
        let schema = output.schema();
        let names: Vec<_> = schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["name", "lv_L", "name_R", "rv_R", "score"]);
        let score_field = schema.field_with_name("score").expect("score");
        assert!(!score_field.is_nullable(), "score non nullable in inner");
    }

    #[test]
    fn left_join_keeps_unmatched_left_rows_with_null_score() {
        let (left, right) = people();
        let mut json = base_config();
        json["how"] = serde_json::json!("left");
        let output = fuzzy_join(&left, &right, &config(json), &Limits::default()).expect("join");
        assert_eq!(output.num_rows(), 3, "1 match + 2 sinistre non matchate");
        let scores = scores_of(&output);
        assert!(scores[0].is_some());
        assert!(scores[1].is_none(), "müller senza match: score null");
        assert!(scores[2].is_none(), "chiave null: score null");
        let right_names = output
            .column(output.schema().index_of("name_R").expect("name_R"))
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("name_R Utf8");
        assert!(right_names.is_null(1) && right_names.is_null(2));
        assert!(output.schema().field_with_name("score").expect("score").is_nullable());
    }

    #[test]
    fn blocking_strategies_and_candidate_reduction() {
        // Soundex: Robert/Rupert condividono R163 ma non il prefisso.
        let left = batch(vec![("name", utf8_column_of(&[Some("Robert")]))]);
        let right = batch(vec![("name", utf8_column_of(&[Some("Rupert")]))]);
        let mut json = base_config();
        json["metric"] = serde_json::json!("levenshtein");
        json["threshold"] = serde_json::json!(0.5);
        let prefix = fuzzy_join(&left, &right, &config(json.clone()), &Limits::default())
            .expect("prefix");
        assert_eq!(prefix.num_rows(), 0, "prefix diverso: nessun candidato");
        json["blocking"] = serde_json::json!("soundex");
        let soundex = fuzzy_join(&left, &right, &config(json.clone()), &Limits::default())
            .expect("soundex");
        assert_eq!(soundex.num_rows(), 1, "soundex uguale: candidato trovato");
        // none: tutte le coppie candidate (qui 1x1).
        json["blocking"] = serde_json::json!("none");
        let all = fuzzy_join(&left, &right, &config(json), &Limits::default()).expect("none");
        assert_eq!(all.num_rows(), 1);
    }

    #[test]
    fn threshold_edges_include_one_and_exclude_below() {
        let left = batch(vec![("name", utf8_column_of(&[Some("abc"), Some("abd")]))]);
        let right = batch(vec![("name", utf8_column_of(&[Some("abc"), Some("xyz")]))]);
        let mut json = base_config();
        json["threshold"] = serde_json::json!(1.0);
        let output = fuzzy_join(&left, &right, &config(json), &Limits::default()).expect("join");
        assert_eq!(output.num_rows(), 1, "threshold 1.0: solo l'identica");
        let scores = scores_of(&output);
        assert!((scores[0].expect("score") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn case_sensitivity_changes_matches() {
        let left = batch(vec![("name", utf8_column_of(&[Some("MARTHA")]))]);
        let right = batch(vec![("name", utf8_column_of(&[Some("marhta")]))]);
        let output = fuzzy_join(&left, &right, &config(base_config()), &Limits::default())
            .expect("case-insensitive");
        assert_eq!(output.num_rows(), 1, "default case-insensitive");
        let mut json = base_config();
        json["case_sensitive"] = serde_json::json!(true);
        let output = fuzzy_join(&left, &right, &config(json), &Limits::default())
            .expect("case-sensitive");
        assert_eq!(output.num_rows(), 0, "case-sensitive: prefisso 'MA' vs 'ma'");
    }

    #[test]
    fn duplicate_right_keys_match_in_right_index_order() {
        let left = batch(vec![("name", utf8_column_of(&[Some("Martha")]))]);
        let right = batch(vec![
            ("name", utf8_column_of(&[Some("Marhta"), Some("Marhta"), Some("xyz")])),
            ("rv", i64_column_of(&[5, 9, 1])),
        ]);
        let output = fuzzy_join(&left, &right, &config(base_config()), &Limits::default())
            .expect("duplicati");
        assert_eq!(output.num_rows(), 2);
        let rv = output
            .column(output.schema().index_of("rv_R").expect("rv_R"))
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("rv_R Int64");
        assert_eq!((rv.value(0), rv.value(1)), (5, 9), "ordine indice destro");
    }

    #[test]
    fn unicode_keys_compare_per_character() {
        let left = batch(vec![("name", utf8_column_of(&[Some("André"), Some("東京タワー")]))]);
        let right = batch(vec![("name", utf8_column_of(&[Some("Andre"), Some("東京タワー")]))]);
        let mut json = base_config();
        json["metric"] = serde_json::json!("levenshtein");
        json["threshold"] = serde_json::json!(0.8);
        json["blocking"] = serde_json::json!("none");
        json["max_candidates"] = serde_json::json!(10);
        let output = fuzzy_join(&left, &right, &config(json), &Limits::default()).expect("unicode");
        // "andré"/"andre" (5/6 ≈ 0.833) e l'identica giapponese (1.0).
        assert_eq!(output.num_rows(), 2);
    }

    #[test]
    fn execution_is_deterministic_across_runs() {
        let (left, right) = people();
        let cfg = config(base_config());
        let first = fuzzy_join(&left, &right, &cfg, &Limits::default()).expect("prima");
        let second = fuzzy_join(&left, &right, &cfg, &Limits::default()).expect("seconda");
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
    }

    #[test]
    fn max_candidates_fails_closed_on_oversized_block() {
        let left = batch(vec![("name", utf8_column_of(&[Some("martha")]))]);
        let right = batch(vec![(
            "name",
            utf8_column_of(&[Some("marhta"), Some("marta"), Some("maria")]),
        )]);
        let mut json = base_config();
        json["max_candidates"] = serde_json::json!(2);
        let error = fuzzy_join(&left, &right, &config(json), &Limits::default())
            .expect_err("blocco da 3 con max_candidates 2");
        assert!(error.to_string().contains("max_candidates"), "{error}");
    }

    #[test]
    fn limits_and_collisions_are_fail_closed() {
        let (left, right) = people();
        let cfg = config(base_config());
        let mut limits = Limits::default();
        limits.max_rows = 1;
        // how=left produrrebbe 3 righe: scatta max_rows.
        let mut json = base_config();
        json["how"] = serde_json::json!("left");
        let error = fuzzy_join(&left, &right, &config(json), &limits).expect_err("max_rows");
        assert!(error.to_string().contains("max_rows"), "{error}");
        // Chiave non Utf8 -> errore di schema.
        let bad_left = batch(vec![("name", i64_column_of(&[1]))]);
        assert!(fuzzy_join(&bad_left, &right, &cfg, &Limits::default()).is_err());
        // Colonna score che collide con l'output -> errore. Le colonne
        // destre prendono sempre `_R` e le sinistre non chiave `_L`: l'unica
        // collisione possibile e' una chiave sinistra chiamata come la
        // colonna score (la chiave conserva il nome).
        let colliding_left = batch(vec![("score", utf8_column_of(&[Some("martha")]))]);
        let mut json = base_config();
        json["left_key"] = serde_json::json!("score");
        let error = fuzzy_join(&colliding_left, &right, &config(json), &Limits::default())
            .expect_err("collisione score");
        assert!(error.to_string().contains("score"), "{error}");
        // Nome score personalizzato.
        let mut json = base_config();
        json["score_column"] = serde_json::json!("similarity");
        let output = fuzzy_join(&left, &right, &config(json), &Limits::default()).expect("custom");
        assert!(output.schema().field_with_name("similarity").is_ok());
    }
}
