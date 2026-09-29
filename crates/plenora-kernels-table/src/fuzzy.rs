//! `table.fuzzy_join`: join per similarita' testuale su anagrafiche sporche.
//!
//! Config, schema e ordine di output: la scheda
//! `docs/schede/table.fuzzy_join.md` (in `docs/operazioni.md`).
//! Qui i punti che il codice deve garantire:
//! - le coppie candidate vengono solo dal blocking sul lato destro; un blocco
//!   oltre `max_candidates` e' un errore, non un troncamento;
//! - ogni coppia con score >= `threshold` produce una riga (nessun
//!   best-match); le chiavi null non matchano mai;
//! - la normalizzazione (lowercase Unicode, salvo `case_sensitive`) vale sia
//!   per le metriche sia per il blocking;
//! - l'ordine di output non dipende da hash map: i blocchi sono `Vec` in
//!   ordine di inserzione.
//!
//! Metriche su caratteri Unicode: Jaro-Winkler (p = 0.1, prefisso massimo 4,
//! nessuna soglia di attivazione del boost); Levenshtein normalizzato come
//! `1 - dist/max_len`; Jaccard sui token separati da whitespace. Due stringhe
//! vuote valgono 1.0. Soundex: American Soundex sulle sole lettere ASCII (le
//! altre sono ignorate e non interrompono le run); `h`/`w` non separano
//! lettere dello stesso codice, le vocali si'.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use plenora_core::arrow::array::{Float64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use serde::Deserialize;

use crate::hashing::FastHasher;
use crate::joins::{combine_horizontal, HorizontalNames};
use crate::{utf8_column, validate_output_name, Limits};
use plenora_core::{PlenoraError, Result};

/// Misura di somiglianza di `table.fuzzy_join` (`metric`, obbligatorio), in
/// `[0, 1]`, sui caratteri Unicode dei testi normalizzati.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuzzyMetric {
    /// `jaro_winkler`: Jaro piu' il bonus del prefisso comune (p = 0.1, al
    /// piu' 4 caratteri), senza soglia di attivazione.
    JaroWinkler,
    /// `levenshtein`: `1 - distanza / lunghezza della piu' lunga`.
    Levenshtein,
    /// `jaccard`: token in comune su token totali, come insiemi, separati da
    /// spazi.
    Jaccard,
}

/// Come si formano i blocchi di candidati (`blocking`, obbligatorio): si
/// confrontano solo le coppie dello stesso blocco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuzzyBlocking {
    /// `prefix`: i primi `blocking_param` caratteri (default 2).
    Prefix,
    /// `soundex`: il codice American Soundex delle sole lettere ASCII.
    Soundex,
    /// `none`: un solo blocco con tutte le righe destre non nulle.
    None,
}

/// Righe sinistre senza coppie (`how`, default `inner`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuzzyHow {
    /// `inner`: si scartano.
    Inner,
    /// `left`: restano una volta, con destra e score nulli.
    Left,
}
const fn default_how() -> FuzzyHow {
    FuzzyHow::Inner
}

/// Config di `table.fuzzy_join`; campi sconosciuti rifiutati, regole in
/// [`validate_config`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuzzyJoin {
    /// Colonna Utf8 della sinistra (obbligatorio).
    pub left_key: String,
    /// Colonna Utf8 della destra (obbligatorio).
    pub right_key: String,
    /// Misura di somiglianza (obbligatorio).
    pub metric: FuzzyMetric,
    /// Somiglianza minima di una coppia, in `(0, 1]` (obbligatorio).
    pub threshold: f64,
    /// Formazione dei blocchi (obbligatorio).
    pub blocking: FuzzyBlocking,
    /// Caratteri del prefisso, `>= 1`, solo con `blocking = prefix`;
    /// default [`DEFAULT_PREFIX_LEN`].
    pub blocking_param: Option<usize>,
    /// Righe sinistre senza coppie; default `inner`.
    #[serde(default = "default_how")]
    pub how: FuzzyHow,
    /// Nome della colonna score; default [`DEFAULT_SCORE_COLUMN`].
    pub score_column: Option<String>,
    /// Righe massime di un blocco destro, `>= 1`; default
    /// [`DEFAULT_MAX_CANDIDATES`].
    pub max_candidates: Option<usize>,
    /// Default `false`: i testi si confrontano dopo `to_lowercase` Unicode.
    #[serde(default)]
    pub case_sensitive: bool,
}

// Default documentati della config: un'unica fonte per kernel e analisi.

/// Caratteri del prefisso di blocking senza `blocking_param`.
pub const DEFAULT_PREFIX_LEN: usize = 2;
/// Righe massime di un blocco destro senza `max_candidates`.
pub const DEFAULT_MAX_CANDIDATES: usize = 50;
/// Nome della colonna score senza `score_column`.
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

/// Validazioni statiche della config, chiamate dal kernel e dall'analisi
/// del contratto: stesse regole, stessi messaggi.
///
/// # Errors
///
/// - `InvalidPlan`: `threshold` fuori da (0, 1] o non finita; `blocking_param`
///   nullo con blocking `prefix` o presente con `soundex`/`none`;
///   `max_candidates` nullo; nome della colonna score vuoto o oltre 1024
///   byte (come `validate_output_name`).
pub fn validate_config(config: &FuzzyJoin) -> Result<()> {
    if !config.threshold.is_finite() || config.threshold <= 0.0 || config.threshold > 1.0 {
        return Err(PlenoraError::InvalidPlan(
            "threshold deve essere in (0, 1]".into(),
        ));
    }
    match config.blocking {
        FuzzyBlocking::Prefix => {
            if config.prefix_len() == 0 {
                return Err(PlenoraError::InvalidPlan(
                    "blocking_param deve essere >= 1".into(),
                ));
            }
        }
        FuzzyBlocking::Soundex | FuzzyBlocking::None => {
            if config.blocking_param.is_some() {
                return Err(PlenoraError::InvalidPlan(
                    "blocking_param ammesso solo con blocking=prefix".into(),
                ));
            }
        }
    }
    if config.max_candidates() == 0 {
        return Err(PlenoraError::InvalidPlan(
            "max_candidates deve essere >= 1".into(),
        ));
    }
    validate_output_name(config.score_name())
}

const fn soundex_code(letter: char) -> u8 {
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
///
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

#[cfg(test)]
fn jaro_similarity(left: &[char], right: &[char]) -> f64 {
    let mut left_matched = vec![false; left.len()];
    let mut right_matched = vec![false; right.len()];
    jaro_similarity_scratch(left, right, &mut left_matched, &mut right_matched)
}

/// Jaro con flag di match forniti dal chiamante (riuso buffer nel probe di
/// `fuzzy_join`, hot path minimale): stessa sequenza di confronti, risultato bit-identico.
fn jaro_similarity_scratch(
    left: &[char],
    right: &[char],
    left_matched: &mut [bool],
    right_matched: &mut [bool],
) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let window = (left.len().max(right.len()) / 2).saturating_sub(1);
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
#[cfg(test)]
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
#[cfg(test)]
fn levenshtein_distance(left: &[char], right: &[char]) -> usize {
    let mut previous: Vec<usize> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    levenshtein_distance_scratch(left, right, &mut previous, &mut current)
}

/// DP di Levenshtein con righe fornite dal chiamante (riuso buffer nel
/// probe di `fuzzy_join`, hot path minimale): stesso ordine di calcolo, risultato identico.
/// Il probe usa `levenshtein_entro`; questa DP completa resta il confronto
/// dei test.
#[cfg(test)]
fn levenshtein_distance_scratch(
    left: &[char],
    right: &[char],
    previous: &mut Vec<usize>,
    current: &mut Vec<usize>,
) -> usize {
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }
    previous.clear();
    previous.extend(0..=right.len());
    current.clear();
    current.resize(right.len() + 1, 0);
    for (row, &a) in left.iter().enumerate() {
        current[0] = row + 1;
        for (col, &b) in right.iter().enumerate() {
            let substitution = previous[col] + usize::from(a != b);
            current[col + 1] = (previous[col + 1] + 1)
                .min(current[col] + 1)
                .min(substitution);
        }
        std::mem::swap(previous, current);
    }
    previous[right.len()]
}

/// Levenshtein normalizzato: `1 - dist/max_len`; due stringhe vuote -> 1.0.
#[cfg(test)]
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
#[cfg(test)]
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

/// Buffer riusati per le metriche di coppia nel probe di `fuzzy_join`
/// (hot path minimale: nessuna allocazione per coppia candidata). I risultati sono
/// bit-identici alle versioni allocanti: stesse operazioni f64 nello
/// stesso ordine.
#[derive(Default)]
struct FuzzyScratch {
    left_matched: Vec<bool>,
    right_matched: Vec<bool>,
    previous: Vec<usize>,
    current: Vec<usize>,
    /// Distanza di Levenshtein massima ammessa dalla soglia, per lunghezza
    /// massima della coppia (calcolata alla prima coppia di quella
    /// lunghezza; la soglia e' la stessa per tutto il probe).
    distanze_massime: Vec<Option<usize>>,
}

/// Jaro-Winkler su caratteri pre-decodificati con flag riusati.
fn jaro_winkler_chars(left: &[char], right: &[char], scratch: &mut FuzzyScratch) -> f64 {
    scratch.left_matched.clear();
    scratch.left_matched.resize(left.len(), false);
    scratch.right_matched.clear();
    scratch.right_matched.resize(right.len(), false);
    let jaro = jaro_similarity_scratch(
        left,
        right,
        &mut scratch.left_matched,
        &mut scratch.right_matched,
    );
    winkler(jaro, prefisso_comune(left, right))
}

/// Prefisso comune del boost di Winkler: al piu' 4 caratteri.
fn prefisso_comune(left: &[char], right: &[char]) -> usize {
    left.iter()
        .zip(right)
        .take(4)
        .take_while(|(a, b)| a == b)
        .count()
}

/// Boost di Winkler (p = 0.1): la stessa espressione f64 del riferimento.
fn winkler(jaro: f64, prefix: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let boost = prefix as f64 * 0.1 * (1.0 - jaro);
    jaro + boost
}

/// Jaro massimo con al piu' `matches` caratteri abbinati e nessuna
/// trasposizione, per stringhe non vuote: la formula di
/// `jaro_similarity_scratch` con il terzo termine `(m - t/2)/m` al suo
/// massimo 1.
///
/// In f64 e' un maggiorante esatto del Jaro calcolato: con `m <= matches`
/// i termini `m/|a|` e `m/|b|` non crescono (conversione e divisione
/// arrotondate al piu' vicino sono monotone), `(m - t/2)/m` e' al piu'
/// `m/m = 1` per la stessa monotonia, e somme e divisione per 3 sono
/// monotone. Senza caratteri abbinati il Jaro e' 0.
fn jaro_massimo(matches: usize, left_len: usize, right_len: usize) -> f64 {
    if matches == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let matches = matches as f64;
    #[allow(clippy::cast_precision_loss)]
    let score = (matches / left_len as f64 + matches / right_len as f64 + 1.0) / 3.0;
    score
}

/// Caratteri in comune come multinsiemi, da due sequenze ordinate: ogni
/// abbinamento di Jaro unisce due posizioni distinte con lo stesso
/// carattere, quindi i caratteri abbinati sono al piu' questi.
fn caratteri_comuni(left_sorted: &[char], right_sorted: &[char]) -> usize {
    let (mut i, mut j, mut comuni) = (0, 0, 0);
    while let (Some(a), Some(b)) = (left_sorted.get(i), right_sorted.get(j)) {
        match a.cmp(b) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                comuni += 1;
                i += 1;
                j += 1;
            }
        }
    }
    comuni
}

/// Margine del bound di Jaro-Winkler sotto la soglia.
///
/// Il boost `j + c (1 - j)` (con `c = fl(p * 0.1) <= 0.4`) e' crescente in
/// `j` nei reali, ma calcolato in f64 non e' garantito monotono: fra il
/// valore f64 e quello reale, per `j` in [0, 1], ci sono al piu' 2 ulp di 1
/// (un arrotondamento sulla somma, due sul prodotto e uno su `1 - j`,
/// pesati da `c <= 0.4`). Il JW calcolato di una coppia supera quindi il
/// bound calcolato al piu' di 4 ulp (circa 9e-16): una coppia si scarta
/// solo se il bound e' sotto la soglia di piu' di 1e-12, e ogni coppia piu'
/// vicina alla soglia si calcola per intero.
const MARGINE_WINKLER: f64 = 1e-12;

/// Score di Jaro-Winkler se e' >= `threshold`, `None` se e' certamente
/// sotto.
///
/// Due maggioranti del numero di caratteri abbinati, dal piu' economico: la
/// lunghezza minore (in caratteri Unicode dopo la normalizzazione, come la
/// metrica) e i caratteri in comune come multinsiemi. Il Jaro massimo
/// (`jaro_massimo`, maggiorante esatto in f64) con il prefisso esatto
/// della coppia da' il JW massimo; sotto `threshold - MARGINE_WINKLER` la
/// coppia si scarta. Le altre si calcolano con `jaro_winkler_chars`, lo
/// stesso calcolo del riferimento, bit per bit. Le coppie con una stringa
/// vuota si calcolano sempre (costo nullo).
fn jaro_winkler_sopra_soglia(
    (left, left_sorted): (&[char], &[char]),
    (right, right_sorted): (&[char], &[char]),
    threshold: f64,
    scratch: &mut FuzzyScratch,
) -> Option<f64> {
    if !left.is_empty() && !right.is_empty() {
        let soglia_sicura = threshold - MARGINE_WINKLER;
        let prefix = prefisso_comune(left, right);
        let massimo = |matches| winkler(jaro_massimo(matches, left.len(), right.len()), prefix);
        if massimo(left.len().min(right.len())) < soglia_sicura {
            return None;
        }
        if massimo(caratteri_comuni(left_sorted, right_sorted)) < soglia_sicura {
            return None;
        }
    }
    Some(jaro_winkler_chars(left, right, scratch))
}

/// Score di Levenshtein normalizzato da distanza e lunghezza massima (in
/// caratteri Unicode): `1 - dist/max_len`, la stessa espressione f64 del
/// percorso di riferimento. Unica fonte sia per lo score emesso sia per la
/// distanza massima ammessa dalla soglia (`distanza_massima`).
fn punteggio_levenshtein(distanza: usize, max_len: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let score = 1.0 - distanza as f64 / max_len as f64;
    score
}

/// La distanza piu' grande con score >= `threshold` per stringhe la cui
/// piu' lunga ha `max_len >= 1` caratteri.
///
/// Lo score e' monotono non crescente nella distanza anche in f64: la
/// conversione `usize -> f64`, la divisione per lo stesso `max_len` e la
/// sottrazione da 1 sono arrotondate al piu' vicino, e l'arrotondamento e'
/// monotono. Le distanze ammesse sono quindi un prefisso `0..=k`, e `k` si
/// trova per bisezione valutando `punteggio_levenshtein` stesso: nessuna
/// formula chiusa, nessun margine, nessuna coppia al bordo trattata
/// diversamente dal riferimento. La distanza 0 e' sempre ammessa (score 1,
/// soglia <= 1).
fn distanza_massima(max_len: usize, threshold: f64) -> usize {
    let (mut ammessa, mut esclusa) = (0_usize, max_len + 1);
    while esclusa - ammessa > 1 {
        let mezzo = ammessa + (esclusa - ammessa) / 2;
        if punteggio_levenshtein(mezzo, max_len) >= threshold {
            ammessa = mezzo;
        } else {
            esclusa = mezzo;
        }
    }
    ammessa
}

/// Distanza di Levenshtein se e' al piu' `limite`, `None` altrimenti.
///
/// - Filtro sulle lunghezze: la distanza e' almeno la differenza delle
///   lunghezze, quindi oltre `limite` la coppia non puo' rientrarvi.
/// - DP a banda (Ukkonen): un cammino di edit che passa per la cella
///   `(i, j)` costa almeno `|i - j|`, quindi ogni cammino di costo
///   `<= limite` resta nella banda `|i - j| <= limite`. Le celle fuori banda
///   valgono `limite + 1` (infinito); dentro la banda il valore calcolato e'
///   il minimo sui soli cammini in banda: esatto quando la distanza vera e'
///   `<= limite`, oltre `limite` altrimenti (i valori sono tagliati a
///   `limite + 1`).
/// - Uscita anticipata: ogni cammino attraversa ogni riga e i costi lungo
///   un cammino non decrescono, quindi se tutta la banda di una riga supera
///   `limite` la distanza finale lo supera.
fn levenshtein_entro(
    left: &[char],
    right: &[char],
    limite: usize,
    previous: &mut Vec<usize>,
    current: &mut Vec<usize>,
) -> Option<usize> {
    if left.len().abs_diff(right.len()) > limite {
        return None;
    }
    if left.is_empty() {
        return Some(right.len());
    }
    if right.is_empty() {
        return Some(left.len());
    }
    let infinito = limite.saturating_add(1);
    previous.clear();
    previous.resize(right.len() + 1, infinito);
    current.clear();
    current.resize(right.len() + 1, infinito);
    for (col, cella) in previous
        .iter_mut()
        .enumerate()
        .take(limite.min(right.len()) + 1)
    {
        *cella = col;
    }
    for (row, &a) in left.iter().enumerate() {
        let riga = row + 1;
        // Banda della riga: colonne `riga - limite ..= riga + limite`. La
        // colonna 0 vale `riga` se in banda e la cella appena a sinistra
        // della banda vale infinito (il buffer contiene valori di due righe
        // prima). Le celle a destra della banda non sono mai state scritte:
        // la banda si sposta solo verso destra, quindi valgono ancora
        // infinito dal `resize`.
        let prima = riga.saturating_sub(limite).max(1);
        let ultima = riga.saturating_add(limite).min(right.len());
        current[0] = if riga <= limite { riga } else { infinito };
        current[prima - 1] = if prima == 1 { current[0] } else { infinito };
        let mut minimo = current[prima - 1];
        for col in prima..=ultima {
            let substitution = previous[col - 1] + usize::from(a != right[col - 1]);
            let valore = (previous[col] + 1)
                .min(current[col - 1] + 1)
                .min(substitution)
                .min(infinito);
            current[col] = valore;
            minimo = minimo.min(valore);
        }
        if minimo > limite {
            return None;
        }
        std::mem::swap(previous, current);
    }
    let distanza = previous[right.len()];
    (distanza <= limite).then_some(distanza)
}

/// Score di Levenshtein normalizzato se e' >= `threshold`, `None` se e'
/// certamente sotto.
///
/// La distanza massima ammessa viene dalla stessa espressione dello score
/// (`distanza_massima`, in cache per lunghezza): una coppia si scarta solo
/// se la sua distanza supera quel massimo, cioe' solo se il suo score del
/// riferimento e' sotto soglia. Per le altre la distanza e' esatta e lo
/// score e' la stessa espressione f64 del riferimento, bit per bit.
fn levenshtein_sopra_soglia(
    left: &[char],
    right: &[char],
    threshold: f64,
    scratch: &mut FuzzyScratch,
) -> Option<f64> {
    let max_len = left.len().max(right.len());
    if max_len == 0 {
        return Some(1.0);
    }
    if scratch.distanze_massime.len() <= max_len {
        scratch.distanze_massime.resize(max_len + 1, None);
    }
    let limite = *scratch.distanze_massime[max_len]
        .get_or_insert_with(|| distanza_massima(max_len, threshold));
    let distanza = levenshtein_entro(
        left,
        right,
        limite,
        &mut scratch.previous,
        &mut scratch.current,
    )?;
    Some(punteggio_levenshtein(distanza, max_len))
}

/// Jaccard su insiemi di token pre-costruiti (nessuna allocazione per
/// coppia).
fn jaccard_sets(
    left: &std::collections::HashSet<&str>,
    right: &std::collections::HashSet<&str>,
) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let intersection = left.intersection(right).count();
    let union = left.len() + right.len() - intersection;
    #[allow(clippy::cast_precision_loss)]
    let score = intersection as f64 / union as f64;
    score
}

/// Lato destro decodificato una tantum per la metrica scelta: nessuna
/// decodifica per coppia candidata nel probe. Le righe null restano vuote e
/// non sono mai lette: i blocchi contengono solo righe non null. La metrica
/// e' la variante stessa, quindi forma e metrica non possono divergere.
enum DestraDecodificata<'a> {
    /// Caratteri e caratteri ordinati (per il bound sui caratteri comuni).
    JaroWinkler {
        chars: Vec<Vec<char>>,
        ordinati: Vec<Vec<char>>,
    },
    Levenshtein(Vec<Vec<char>>),
    Jaccard(Vec<std::collections::HashSet<&'a str>>),
}

/// Righe sinistre per chunk del probe. Il costo di una riga e' quello del
/// suo blocco destro (centinaia di coppie): chunk piccoli bilanciano i
/// thread anche con blocchi di dimensioni molto diverse.
const CHUNK_PROBE_FUZZY: usize = 256;

/// Output di un chunk del probe: le coppie delle sue righe sinistre, in
/// ordine di riga sinistra e, per riga, di indice destro.
#[derive(Default)]
struct UscitaChunk {
    left_rows: Vec<Option<usize>>,
    right_rows: Vec<Option<usize>>,
    scores: Vec<Option<f64>>,
}

/// Stato condiviso fra i chunk per `max_rows`: il numero di righe di output
/// gia' prodotte (da qualsiasi chunk) e il segnale di superamento.
struct ContatoreUscita {
    prodotte: AtomicUsize,
    superato: AtomicBool,
    max_rows: usize,
}

impl ContatoreUscita {
    /// Somma le righe prodotte da una riga sinistra; `false` se il totale
    /// supera `max_rows` (o `usize`, che lo supera comunque).
    ///
    /// Ogni riga contata appartiene all'output finale (il probe non ha altri
    /// errori), quindi un totale parziale oltre il limite implica un totale
    /// finale oltre il limite; e se il totale finale lo supera, l'ultima
    /// somma lo vede. Il sequenziale controlla dopo ogni riga un conteggio
    /// monotono, quindi fallisce esattamente quando il totale supera
    /// `max_rows`: lo stesso esito, qualunque sia l'ordine dei thread.
    fn aggiungi(&self, righe: usize) -> bool {
        let totale = self
            .prodotte
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |prodotte| {
                prodotte.checked_add(righe)
            })
            .ok()
            .and_then(|prima| prima.checked_add(righe));
        let entro = totale.is_some_and(|totale| totale <= self.max_rows);
        if !entro {
            self.superato.store(true, Ordering::Relaxed);
        }
        entro
    }
}

/// Probe di un intervallo di righe sinistre: le candidate destre del blocco
/// in ordine di indice, una riga di output per coppia con score >= soglia,
/// e con `how=left` una riga senza match per le righe sinistre senza coppie.
/// Si ferma appena il contatore segnala `max_rows` superato.
fn probe_chunk(
    righe: std::ops::Range<usize>,
    left_norm: &[Option<String>],
    blocks: &HashMap<String, Vec<usize>, FastHasher>,
    block_key: &(dyn Fn(&str) -> String + Sync),
    destra: &DestraDecodificata<'_>,
    config: &FuzzyJoin,
    contatore: &ContatoreUscita,
) -> UscitaChunk {
    let mut uscita = UscitaChunk::default();
    let mut scratch = FuzzyScratch::default();
    let mut left_chars: Vec<char> = Vec::new();
    let mut left_sorted: Vec<char> = Vec::new();
    for left_row in righe {
        if contatore.superato.load(Ordering::Relaxed) {
            break;
        }
        let inizio = uscita.left_rows.len();
        let candidates = left_norm
            .get(left_row)
            .and_then(Option::as_ref)
            .and_then(|value| blocks.get(&block_key(value)).map(|rows| (value, rows)));
        if let Some((value, candidates)) = candidates {
            let mut emetti = |right_row: usize, similarity: f64| {
                if similarity >= config.threshold {
                    uscita.left_rows.push(Some(left_row));
                    uscita.right_rows.push(Some(right_row));
                    uscita.scores.push(Some(similarity));
                }
            };
            match destra {
                DestraDecodificata::JaroWinkler {
                    chars: right_chars,
                    ordinati: right_sorted,
                } => {
                    left_chars.clear();
                    left_chars.extend(value.chars());
                    left_sorted.clear();
                    left_sorted.extend_from_slice(&left_chars);
                    left_sorted.sort_unstable();
                    for &right_row in candidates {
                        // `None`: score certamente sotto soglia, nessuna riga
                        // come nel riferimento.
                        if let Some(similarity) = jaro_winkler_sopra_soglia(
                            (&left_chars, &left_sorted),
                            (&right_chars[right_row], &right_sorted[right_row]),
                            config.threshold,
                            &mut scratch,
                        ) {
                            emetti(right_row, similarity);
                        }
                    }
                }
                DestraDecodificata::Levenshtein(right_chars) => {
                    left_chars.clear();
                    left_chars.extend(value.chars());
                    for &right_row in candidates {
                        // `None`: score certamente sotto soglia, nessuna riga
                        // come nel riferimento.
                        if let Some(similarity) = levenshtein_sopra_soglia(
                            &left_chars,
                            &right_chars[right_row],
                            config.threshold,
                            &mut scratch,
                        ) {
                            emetti(right_row, similarity);
                        }
                    }
                }
                DestraDecodificata::Jaccard(right_tokens) => {
                    let left_tokens: std::collections::HashSet<&str> =
                        value.split_whitespace().collect();
                    for &right_row in candidates {
                        emetti(
                            right_row,
                            jaccard_sets(&left_tokens, &right_tokens[right_row]),
                        );
                    }
                }
            }
        }
        if uscita.left_rows.len() == inizio && config.how == FuzzyHow::Left {
            uscita.left_rows.push(Some(left_row));
            uscita.right_rows.push(None);
            uscita.scores.push(None);
        }
        let prodotte = uscita.left_rows.len() - inizio;
        // Una riga senza output non cambia il totale: il controllo del
        // sequenziale dopo quella riga ripete l'esito della precedente.
        if prodotte > 0 && !contatore.aggiungi(prodotte) {
            break;
        }
    }
    uscita
}

/// Join per similarita' testuale; semantica nella documentazione di modulo.
///
/// Uscita: le colonne sinistre (la chiave con il suo nome, le altre `_L`),
/// tutte le destre con `_R`, la colonna score Float64 in coda; righe
/// nell'ordine della sinistra e, per riga, della destra.
///
/// # Errors
///
/// - `InvalidPlan`: config non valida (come `validate_config`).
/// - `ResourceLimit`: blocco destro oltre `max_candidates` (anche se nessuna
///   riga sinistra vi cade); output oltre `limits.max_rows` o
///   `limits.max_columns`.
/// - `Schema`: chiave sinistra o destra assente o non Utf8; collisione di
///   nomi nello schema di output (colonna score compresa); metadati di
///   schema in conflitto.
/// - `DataMapping` (`arrow error: …`): errore Arrow nella costruzione del batch.
pub fn fuzzy_join(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &FuzzyJoin,
    limits: &Limits,
) -> Result<RecordBatch> {
    fuzzy_join_con_chunk(left, right, config, limits, CHUNK_PROBE_FUZZY)
}

/// `fuzzy_join` con la dimensione dei chunk del probe esplicita: i test la
/// forzano piccola per esercitare il percorso parallelo su input piccoli.
// Fasi in sequenza lineare: la lunghezza e' nella pipeline, non nella logica.
#[allow(clippy::too_many_lines)]
fn fuzzy_join_con_chunk(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &FuzzyJoin,
    limits: &Limits,
    chunk: usize,
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
    // Il blocco segnalato e' scelto deterministicamente: il piu' grande, e a
    // parita' di dimensione quello con la chiave lessicograficamente minore.
    // Iterando `blocks.values()` il blocco incontrato per primo dipenderebbe
    // dall'ordine di visita della `HashMap` — un dettaglio di
    // implementazione dell'hasher, non una proprieta' dell'input — e con
    // piu' blocchi sovradimensionati il conteggio nel messaggio potrebbe
    // cambiare fra esecuzioni: anche l'identita' dell'errore e'
    // deterministica.
    let worst = blocks
        .iter()
        .max_by(|(left_key, left_rows), (right_key, right_rows)| {
            left_rows
                .len()
                .cmp(&right_rows.len())
                .then_with(|| right_key.cmp(left_key))
        });
    if let Some((_, rows)) = worst {
        if rows.len() > max_candidates {
            return Err(PlenoraError::ResourceLimit(format!(
                "fuzzy_join: blocco con {} candidati oltre max_candidates {max_candidates}",
                rows.len()
            )));
        }
    }
    // Forme decodificate per la metrica: una tantum sul lato destro, una per
    // riga sul lato sinistro (nel probe); nessuna allocazione per coppia.
    let chars_of = |value: &Option<String>| {
        value
            .as_deref()
            .map_or_else(Vec::new, |text| text.chars().collect())
    };
    let destra = match config.metric {
        FuzzyMetric::JaroWinkler => {
            let chars: Vec<Vec<char>> = right_norm.iter().map(chars_of).collect();
            let ordinati = chars
                .iter()
                .map(|chars| {
                    let mut ordinati = chars.clone();
                    ordinati.sort_unstable();
                    ordinati
                })
                .collect();
            DestraDecodificata::JaroWinkler { chars, ordinati }
        }
        FuzzyMetric::Levenshtein => {
            DestraDecodificata::Levenshtein(right_norm.iter().map(chars_of).collect())
        }
        FuzzyMetric::Jaccard => DestraDecodificata::Jaccard(
            right_norm
                .iter()
                .map(|value| {
                    value
                        .as_deref()
                        .map(|text| text.split_whitespace().collect())
                        .unwrap_or_default()
                })
                .collect(),
        ),
    };
    // Probe per chunk contigui di righe sinistre, in parallelo (rayon) se i
    // chunk sono piu' di uno. Ogni coppia dipende solo dalle sue due chiavi,
    // e i chunk si concatenano nell'ordine delle righe sinistre: righe,
    // ordine e score sono quelli della scansione sequenziale. `max_rows` si
    // applica al totale prodotto da tutti i chunk (`ContatoreUscita`):
    // stesso errore del sequenziale, e memoria entro il limite piu' le righe
    // in corso.
    let chunk = chunk.max(1);
    let intervalli: Vec<std::ops::Range<usize>> = (0..left_norm.len().div_ceil(chunk))
        .map(|indice| {
            let inizio = indice * chunk;
            inizio..inizio.saturating_add(chunk).min(left_norm.len())
        })
        .collect();
    let contatore = ContatoreUscita {
        prodotte: AtomicUsize::new(0),
        superato: AtomicBool::new(false),
        max_rows: limits.max_rows,
    };
    let probe = |righe: &std::ops::Range<usize>| {
        probe_chunk(
            righe.clone(),
            &left_norm,
            &blocks,
            &block_key,
            &destra,
            config,
            &contatore,
        )
    };
    let uscite: Vec<UscitaChunk> = if intervalli.len() > 1 {
        intervalli.par_iter().map(probe).collect()
    } else {
        intervalli.iter().map(probe).collect()
    };
    if contatore.superato.load(Ordering::Relaxed) {
        return Err(PlenoraError::ResourceLimit(
            "fuzzy_join supera max_rows".into(),
        ));
    }
    let totale: usize = uscite.iter().map(|uscita| uscita.left_rows.len()).sum();
    let mut left_rows: Vec<Option<usize>> = Vec::with_capacity(totale);
    let mut right_rows: Vec<Option<usize>> = Vec::with_capacity(totale);
    let mut scores: Vec<Option<f64>> = Vec::with_capacity(totale);
    for uscita in uscite {
        left_rows.extend(uscita.left_rows);
        right_rows.extend(uscita.right_rows);
        scores.extend(uscita.scores);
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
        return Err(PlenoraError::ResourceLimit(
            "fuzzy_join supera max_columns".into(),
        ));
    }
    let mut columns = output.columns().to_vec();
    columns.push(Arc::new(Float64Array::from(scores)));
    let schema = Schema::new_with_metadata(fields, output.schema().metadata().clone());
    let cardinalita = output.num_rows();
    output = crate::batch_with_rows(Arc::new(schema), columns, cardinalita)?;
    Ok(output)
}

#[cfg(test)]
#[path = "fuzzy_oracolo.rs"]
mod oracolo;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::nullable_batch as batch;
    use plenora_core::arrow::array::{Array, ArrayRef, Int64Array, StringArray};

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
            (
                "name",
                utf8_column_of(&[Some("Martha"), Some("Müller"), None]),
            ),
            ("lv", i64_column_of(&[1, 2, 3])),
        ]);
        let right = batch(vec![
            (
                "name",
                utf8_column_of(&[Some("Marhta"), Some("Muller"), None]),
            ),
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
        assert!(
            (martha - 0.9611).abs() < 1e-3,
            "jaro_winkler(MARTHA, MARHTA) = {martha}"
        );
        assert!((jaro_winkler("abc", "abc") - 1.0).abs() < f64::EPSILON);
        assert!((jaro_winkler("", "") - 1.0).abs() < f64::EPSILON);
        assert!((jaro_winkler("abc", "xyz")).abs() < f64::EPSILON);
        assert!((jaro_winkler("", "abc")).abs() < f64::EPSILON);
        // Simmetrica.
        let dwight = jaro_winkler("DWAYNE", "DUANE");
        assert!(
            (dwight - 0.84).abs() < 1e-2,
            "jaro_winkler(DWAYNE, DUANE) = {dwight}"
        );
    }

    #[test]
    fn levenshtein_normalized_matches_reference_values() {
        let kitten = levenshtein_normalized("kitten", "sitting");
        assert!(
            (kitten - (1.0 - 3.0 / 7.0)).abs() < 1e-9,
            "kitten/sitting = {kitten}"
        );
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
        assert!(output
            .schema()
            .field_with_name("score")
            .expect("score")
            .is_nullable());
    }

    #[test]
    fn blocking_strategies_and_candidate_reduction() {
        // Soundex: Robert/Rupert condividono R163 ma non il prefisso.
        let left = batch(vec![("name", utf8_column_of(&[Some("Robert")]))]);
        let right = batch(vec![("name", utf8_column_of(&[Some("Rupert")]))]);
        let mut json = base_config();
        json["metric"] = serde_json::json!("levenshtein");
        json["threshold"] = serde_json::json!(0.5);
        let prefix =
            fuzzy_join(&left, &right, &config(json.clone()), &Limits::default()).expect("prefix");
        assert_eq!(prefix.num_rows(), 0, "prefix diverso: nessun candidato");
        json["blocking"] = serde_json::json!("soundex");
        let soundex =
            fuzzy_join(&left, &right, &config(json.clone()), &Limits::default()).expect("soundex");
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
        let output =
            fuzzy_join(&left, &right, &config(json), &Limits::default()).expect("case-sensitive");
        assert_eq!(
            output.num_rows(),
            0,
            "case-sensitive: prefisso 'MA' vs 'ma'"
        );
    }

    #[test]
    fn duplicate_right_keys_match_in_right_index_order() {
        let left = batch(vec![("name", utf8_column_of(&[Some("Martha")]))]);
        let right = batch(vec![
            (
                "name",
                utf8_column_of(&[Some("Marhta"), Some("Marhta"), Some("xyz")]),
            ),
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
        let left = batch(vec![(
            "name",
            utf8_column_of(&[Some("André"), Some("東京タワー")]),
        )]);
        let right = batch(vec![(
            "name",
            utf8_column_of(&[Some("Andre"), Some("東京タワー")]),
        )]);
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
        let limits = Limits {
            max_rows: 1,
            ..Limits::default()
        };
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
