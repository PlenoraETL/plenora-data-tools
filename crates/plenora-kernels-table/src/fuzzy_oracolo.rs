//! Oracolo di `fuzzy_join`: il percorso sequenziale com'era prima delle
//! ottimizzazioni (probe parallelo, filtro di Levenshtein, bound di
//! Jaro-Winkler), copiato alla lettera come riferimento.
//!
//! Il percorso veloce deve dare lo stesso batch (righe, ordine, score bit per
//! bit) e lo stesso errore (variante e messaggio) su ogni input: casi
//! avversari deterministici e casi casuali proptest, per tutte le metriche e
//! tutte le opzioni della config.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::{PlenoraError, Result};
use proptest::prelude::*;

use super::{soundex, validate_config, FuzzyBlocking, FuzzyHow, FuzzyJoin, FuzzyMetric};
use crate::hashing::FastHasher;
use crate::joins::{combine_horizontal, HorizontalNames};
use crate::test_support::{assert_batches_identical, casi, nullable_batch, test_lunghi};
use crate::{utf8_column, Limits};

// -- Riferimento: copia letterale del percorso sequenziale -------------------

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

/// DP di Levenshtein con righe fornite dal chiamante (riuso buffer nel
/// probe di `fuzzy_join`, hot path minimale): stesso ordine di calcolo, risultato identico.
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
    let prefix = left
        .iter()
        .zip(right)
        .take(4)
        .take_while(|(a, b)| a == b)
        .count();
    #[allow(clippy::cast_precision_loss)]
    let boost = prefix as f64 * 0.1 * (1.0 - jaro);
    jaro + boost
}

/// Levenshtein normalizzato su caratteri pre-decodificati con righe DP
/// riusate.
fn levenshtein_normalized_chars(left: &[char], right: &[char], scratch: &mut FuzzyScratch) -> f64 {
    let max_len = left.len().max(right.len());
    if max_len == 0 {
        return 1.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let score = 1.0
        - levenshtein_distance_scratch(left, right, &mut scratch.previous, &mut scratch.current)
            as f64
            / max_len as f64;
    score
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

/// Forma decodificata del lato destro, costruita una tantum per la metrica
/// scelta (hot path minimale): nessuna decodifica per coppia candidata nel probe.
enum FuzzyForm<'a> {
    Chars(Vec<Option<Vec<char>>>),
    Tokens(Vec<Option<std::collections::HashSet<&'a str>>>),
}

/// Forma decodificata della riga sinistra corrente (una per riga, non per
/// coppia).
enum FuzzyRowForm<'a> {
    Chars(Vec<char>),
    Tokens(std::collections::HashSet<&'a str>),
}

/// Join per similarita' testuale; semantica nella documentazione di modulo.
///
/// # Errors
///
/// - `InvalidPlan`: config non valida (come `validate_config`).
/// - `ResourceLimit`: blocco destro oltre `max_candidates`; output oltre
///   `limits.max_rows` o `limits.max_columns`.
/// - `Schema`: chiave sinistra o destra assente o non Utf8; collisione del
///   nome della colonna score con lo schema di output; errore Arrow nella
///   costruzione del batch.
// Fasi in sequenza lineare: la lunghezza e' nella pipeline, non nella logica.
#[allow(clippy::too_many_lines)]
fn fuzzy_join_riferimento(
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
    // Il blocco segnalato e' scelto deterministicamente: il piu' grande, e a
    // parita' di dimensione quello con la chiave lessicograficamente minore.
    // Iterando `blocks.values()` il blocco incontrato per primo dipenderebbe
    // dall'ordine di visita della `HashMap` — un dettaglio di
    // implementazione dell'hasher, non una proprieta' dell'input — e con
    // piu' blocchi sovradimensionati il conteggio nel messaggio potrebbe
    // cambiare fra esecuzioni (architettura.md#determinismo: l'identita' dell'errore e' stabile).
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
    // Probe: scansione sinistra in ordine, candidate destre in ordine di
    // indice (i `Vec` dei blocchi preservano l'ordine di inserzione). Le
    // forme decodificate per la metrica sono costruite una tantum (lato
    // destro) e una per riga (lato sinistro): nessuna allocazione per
    // coppia candidata (hot path minimale).
    let right_decoded = match config.metric {
        FuzzyMetric::JaroWinkler | FuzzyMetric::Levenshtein => FuzzyForm::Chars(
            right_norm
                .iter()
                .map(|value| value.as_ref().map(|text| text.chars().collect()))
                .collect(),
        ),
        FuzzyMetric::Jaccard => FuzzyForm::Tokens(
            right_norm
                .iter()
                .map(|value| value.as_ref().map(|text| text.split_whitespace().collect()))
                .collect(),
        ),
    };
    let mut scratch = FuzzyScratch::default();
    let mut left_rows: Vec<Option<usize>> = Vec::new();
    let mut right_rows: Vec<Option<usize>> = Vec::new();
    let mut scores: Vec<Option<f64>> = Vec::new();
    for (left_row, value) in left_norm.iter().enumerate() {
        let mut matched = false;
        if let Some(value) = value {
            if let Some(candidates) = blocks.get(&block_key(value)) {
                let left_decoded = match config.metric {
                    FuzzyMetric::JaroWinkler | FuzzyMetric::Levenshtein => {
                        FuzzyRowForm::Chars(value.chars().collect())
                    }
                    FuzzyMetric::Jaccard => {
                        FuzzyRowForm::Tokens(value.split_whitespace().collect())
                    }
                };
                for &right_row in candidates {
                    let null_block_error =
                        || PlenoraError::Internal("righe destre nei blocchi non sono null".into());
                    let similarity = match (config.metric, &left_decoded, &right_decoded) {
                        (
                            FuzzyMetric::JaroWinkler,
                            FuzzyRowForm::Chars(left_chars),
                            FuzzyForm::Chars(right_chars),
                        ) => {
                            let right_chars = right_chars[right_row]
                                .as_ref()
                                .ok_or_else(null_block_error)?;
                            jaro_winkler_chars(left_chars, right_chars, &mut scratch)
                        }
                        (
                            FuzzyMetric::Levenshtein,
                            FuzzyRowForm::Chars(left_chars),
                            FuzzyForm::Chars(right_chars),
                        ) => {
                            let right_chars = right_chars[right_row]
                                .as_ref()
                                .ok_or_else(null_block_error)?;
                            levenshtein_normalized_chars(left_chars, right_chars, &mut scratch)
                        }
                        (
                            FuzzyMetric::Jaccard,
                            FuzzyRowForm::Tokens(left_tokens),
                            FuzzyForm::Tokens(right_tokens),
                        ) => {
                            let right_tokens = right_tokens[right_row]
                                .as_ref()
                                .ok_or_else(null_block_error)?;
                            jaccard_sets(left_tokens, right_tokens)
                        }
                        _ => {
                            return Err(PlenoraError::Internal(
                                "forma decodificata incoerente con la metrica".into(),
                            ));
                        }
                    };
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
            return Err(PlenoraError::ResourceLimit(
                "fuzzy_join supera max_rows".into(),
            ));
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

// -- Confronto -----------------------------------------------------------------

/// Chunk del probe forzati nei test: `None` per la scelta automatica, poi
/// chunk di una riga e chunk piccoli che spezzano l'output di un blocco.
const CHUNK_DI_PROVA: [Option<usize>; 4] = [None, Some(1), Some(2), Some(5)];

/// Confronta percorso veloce e riferimento su un input: stesso batch (score
/// bit per bit) o stesso errore (variante e messaggio).
fn confronta(left: &RecordBatch, right: &RecordBatch, config: &FuzzyJoin, limits: &Limits) {
    let atteso = fuzzy_join_riferimento(left, right, config, limits);
    for chunk in CHUNK_DI_PROVA {
        let ottenuto = chunk.map_or_else(
            || super::fuzzy_join(left, right, config, limits),
            |chunk| super::fuzzy_join_con_chunk(left, right, config, limits, chunk),
        );
        match (&ottenuto, &atteso) {
            (Ok(ottenuto), Ok(atteso)) => assert_batches_identical(ottenuto, atteso),
            (Err(ottenuto), Err(atteso)) => {
                assert_eq!(format!("{ottenuto:?}"), format!("{atteso:?}"), "errore");
            }
            _ => panic!("esito diverso (chunk {chunk:?}): {ottenuto:?} contro {atteso:?}"),
        }
    }
}

fn colonna(values: &[Option<&str>]) -> ArrayRef {
    Arc::new(StringArray::from(values.to_vec()))
}

fn tabella(values: &[Option<&str>]) -> RecordBatch {
    let payload: Vec<i64> = (0..values.len())
        .map(|row| i64::try_from(row).expect("riga"))
        .collect();
    nullable_batch(vec![
        ("name", colonna(values)),
        ("payload", Arc::new(Int64Array::from(payload))),
    ])
}

fn config_di(
    metric: FuzzyMetric,
    threshold: f64,
    (blocking, blocking_param): (FuzzyBlocking, Option<usize>),
    how: FuzzyHow,
    case_sensitive: bool,
    max_candidates: Option<usize>,
) -> FuzzyJoin {
    FuzzyJoin {
        left_key: "name".into(),
        right_key: "name".into(),
        metric,
        threshold,
        blocking,
        blocking_param,
        how,
        score_column: None,
        max_candidates,
        case_sensitive,
    }
}

const METRICHE: [FuzzyMetric; 3] = [
    FuzzyMetric::JaroWinkler,
    FuzzyMetric::Levenshtein,
    FuzzyMetric::Jaccard,
];

/// Blocking con il loro parametro: prefissi di 1 e 3 caratteri, soundex,
/// nessuno.
const BLOCKING: [(FuzzyBlocking, Option<usize>); 4] = [
    (FuzzyBlocking::Prefix, Some(1)),
    (FuzzyBlocking::Prefix, Some(3)),
    (FuzzyBlocking::Soundex, None),
    (FuzzyBlocking::None, None),
];

const SENZA_BLOCKING: (FuzzyBlocking, Option<usize>) = (FuzzyBlocking::None, None);

/// Chiavi avversarie: vuote, soli spazi, identiche, maiuscole, Unicode
/// (combinanti, CJK, maiuscole che cambiano lunghezza in minuscolo), lunghe
/// e corte, prefissi l'una dell'altra, trasposizioni, token ripetuti,
/// stringhe oltre 64 caratteri.
const CHIAVI: [Option<&str>; 30] = [
    None,
    Some(""),
    Some(" "),
    Some("a"),
    Some("A"),
    Some("ab"),
    Some("ba"),
    Some("abc"),
    Some("abd"),
    Some("acb"),
    Some("abcd"),
    Some("abcdefgh"),
    Some("abcdefghij"),
    Some("martha"),
    Some("marhta"),
    Some("MARTHA"),
    Some("dwayne"),
    Some("duane"),
    Some("m\u{fc}ller"),
    Some("muller"),
    Some("mu\u{308}ller"),
    Some("\u{130}stanbul"),
    Some("istanbul"),
    Some("\u{6771}\u{4eac}\u{30bf}\u{30ef}\u{30fc}"),
    Some("\u{6771}\u{4eac}"),
    Some("new york"),
    Some("new  york new"),
    Some("york new"),
    Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab"),
    Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
];

/// Soglie ai bordi: il minimo positivo, valori tipici, i punteggi esatti di
/// Levenshtein `1 - d/m` per lunghezze piccole (coppie esattamente sulla
/// soglia) e 1.
fn soglie_fisse() -> Vec<f64> {
    let mut soglie = vec![f64::MIN_POSITIVE, 0.1, 0.5, 0.8, 0.85, 0.9, 0.95, 1.0];
    for m in 1_u32..=10 {
        for d in 0..m {
            soglie.push(1.0 - f64::from(d) / f64::from(m));
        }
    }
    soglie
}

/// Tutti i punteggi del riferimento sulle coppie di `CHIAVI` per una
/// metrica, con il successivo e il precedente: ogni coppia finisce
/// esattamente sulla soglia, appena sopra e appena sotto.
fn soglie_dai_punteggi(metric: FuzzyMetric, case_sensitive: bool) -> Vec<f64> {
    let tutte = tabella(&CHIAVI);
    let config = config_di(
        metric,
        f64::MIN_POSITIVE,
        SENZA_BLOCKING,
        FuzzyHow::Inner,
        case_sensitive,
        Some(CHIAVI.len()),
    );
    let output =
        fuzzy_join_riferimento(&tutte, &tutte, &config, &Limits::default()).expect("riferimento");
    let scores = output
        .column(output.num_columns() - 1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("score");
    let mut soglie: Vec<f64> = scores
        .values()
        .iter()
        .flat_map(|&score| [score, score.next_up(), score.next_down()])
        .filter(|&soglia| soglia > 0.0 && soglia <= 1.0)
        .collect();
    soglie.sort_by(f64::total_cmp);
    soglie.dedup();
    soglie
}

#[test]
fn oracolo_casi_avversari() {
    let left = tabella(&CHIAVI);
    let right = tabella(&CHIAVI[..24]);
    // Suite di default: le soglie fisse e una su otto di quelle attorno ai
    // punteggi, con uno sfasamento diverso per metrica e maiuscole.
    let passo = if test_lunghi() { 1 } else { 8 };
    for (i_metric, metric) in METRICHE.into_iter().enumerate() {
        for (i_case, case_sensitive) in [false, true].into_iter().enumerate() {
            let mut soglie = soglie_fisse();
            soglie.extend(
                soglie_dai_punteggi(metric, case_sensitive)
                    .into_iter()
                    .skip((i_metric * 2 + i_case) % passo)
                    .step_by(passo),
            );
            for &threshold in &soglie {
                for blocking in BLOCKING {
                    for how in [FuzzyHow::Inner, FuzzyHow::Left] {
                        let config = config_di(
                            metric,
                            threshold,
                            blocking,
                            how,
                            case_sensitive,
                            Some(CHIAVI.len()),
                        );
                        confronta(&left, &right, &config, &Limits::default());
                    }
                }
            }
        }
    }
}

#[test]
fn oracolo_molti_pari_merito() {
    // Molte chiavi uguali o equidistanti: ogni riga sinistra ha molte coppie
    // con lo stesso punteggio, l'ordine deve restare quello del riferimento.
    let chiavi: Vec<Option<&str>> = ["abc", "abd", "abc", "abe", "abc", "xbc", "abc", "abd"]
        .iter()
        .cycle()
        .take(64)
        .map(|&chiave| Some(chiave))
        .collect();
    let left = tabella(&chiavi);
    let right = tabella(&chiavi[..16]);
    for metric in METRICHE {
        for threshold in [f64::MIN_POSITIVE, 2.0 / 3.0, 1.0] {
            for how in [FuzzyHow::Inner, FuzzyHow::Left] {
                let config = config_di(metric, threshold, SENZA_BLOCKING, how, false, None);
                confronta(&left, &right, &config, &Limits::default());
            }
        }
    }
}

#[test]
fn oracolo_errori_di_limite_identici() {
    let left = tabella(&CHIAVI);
    let right = tabella(&CHIAVI[..24]);
    for metric in METRICHE {
        for how in [FuzzyHow::Inner, FuzzyHow::Left] {
            let config = config_di(metric, 0.5, SENZA_BLOCKING, how, false, Some(CHIAVI.len()));
            let totale = fuzzy_join_riferimento(&left, &right, &config, &Limits::default())
                .expect("riferimento")
                .num_rows();
            // Ogni `max_rows` da 0 a oltre il totale: sotto il totale entrambi
            // falliscono con lo stesso errore, dal totale in su entrambi
            // riescono. Suite di default: i bordi e il mezzo.
            let tutti: Vec<usize> = if test_lunghi() {
                (0..=totale + 1).collect()
            } else {
                vec![
                    0,
                    1,
                    totale / 2,
                    totale.saturating_sub(1),
                    totale,
                    totale + 1,
                ]
            };
            for max_rows in tutti {
                let limits = Limits {
                    max_rows,
                    ..Limits::default()
                };
                confronta(&left, &right, &config, &limits);
            }
            // max_columns: fallisce dopo il probe in entrambi.
            for max_columns in 0..6 {
                let limits = Limits {
                    max_columns,
                    ..Limits::default()
                };
                confronta(&left, &right, &config, &limits);
            }
            // max_candidates attorno al blocco unico (23 chiavi non null).
            for max_candidates in [1, 22, 23, 24] {
                let config = config_di(
                    metric,
                    0.5,
                    SENZA_BLOCKING,
                    how,
                    false,
                    Some(max_candidates),
                );
                confronta(&left, &right, &config, &Limits::default());
            }
        }
    }
}

#[test]
fn oracolo_max_rows_al_bordo_su_molti_chunk() {
    // Molte righe sinistre e molti chunk in parallelo: con `max_rows` pari al
    // totale l'output e' identico, con uno in meno l'errore e' identico (e
    // nessun output parziale), a ogni ripetizione, qualunque sia l'ordine in
    // cui i thread finiscono.
    let chiavi: Vec<Option<&str>> = CHIAVI.iter().copied().cycle().take(3_000).collect();
    let left = tabella(&chiavi);
    let right = tabella(&CHIAVI[..24]);
    for metric in METRICHE {
        for how in [FuzzyHow::Inner, FuzzyHow::Left] {
            let config = config_di(metric, 0.5, SENZA_BLOCKING, how, false, None);
            let atteso = fuzzy_join_riferimento(&left, &right, &config, &Limits::default())
                .expect("riferimento");
            let totale = atteso.num_rows();
            for max_rows in [totale - 1, totale] {
                let limits = Limits {
                    max_rows,
                    ..Limits::default()
                };
                let riferimento = fuzzy_join_riferimento(&left, &right, &config, &limits);
                // Ripetizioni: l'ordine in cui i thread finiscono cambia.
                for _ in 0..casi(2, 10) {
                    for chunk in [1, 7, super::CHUNK_PROBE_FUZZY] {
                        let ottenuto =
                            super::fuzzy_join_con_chunk(&left, &right, &config, &limits, chunk);
                        match (&ottenuto, &riferimento) {
                            (Ok(ottenuto), Ok(riferimento)) => {
                                assert_batches_identical(ottenuto, riferimento);
                            }
                            (Err(ottenuto), Err(riferimento)) => {
                                assert_eq!(format!("{ottenuto:?}"), format!("{riferimento:?}"));
                            }
                            _ => panic!("esito diverso con max_rows {max_rows}, chunk {chunk}"),
                        }
                    }
                }
                assert_eq!(riferimento.is_ok(), max_rows == totale, "bordo del limite");
            }
        }
    }
}

#[test]
fn oracolo_config_e_schema_invalidi() {
    let left = tabella(&CHIAVI);
    let right = tabella(&CHIAVI[..5]);
    let mut config = config_di(
        FuzzyMetric::Levenshtein,
        0.0,
        SENZA_BLOCKING,
        FuzzyHow::Inner,
        false,
        None,
    );
    confronta(&left, &right, &config, &Limits::default());
    config.threshold = 0.5;
    config.left_key = "assente".into();
    confronta(&left, &right, &config, &Limits::default());
    config.left_key = "payload".into();
    confronta(&left, &right, &config, &Limits::default());
}

// -- Casi casuali ----------------------------------------------------------

/// Chiave casuale su un alfabeto piccolo (collisioni frequenti) con
/// maiuscole, spazi e caratteri non ASCII.
fn chiave() -> impl Strategy<Value = Option<String>> {
    let carattere = prop::sample::select(vec![
        'a', 'b', 'c', 'A', ' ', '\u{e9}', '\u{c9}', '\u{6771}', '\u{df}',
    ]);
    prop::option::weighted(
        0.9,
        prop::collection::vec(carattere, 0..14).prop_map(|chars| chars.into_iter().collect()),
    )
}

/// Soglia casuale: meta' delle volte un punteggio esatto `1 - d/m`
/// (coppie sulla soglia), altrimenti un valore qualsiasi in (0, 1].
fn soglia() -> impl Strategy<Value = f64> {
    prop_oneof![
        (1_u32..14, 0_u32..14).prop_map(|(m, d)| 1.0 - f64::from(d % m) / f64::from(m)),
        (1_u32..=1_000_000).prop_map(|n| f64::from(n) / 1_000_000.0),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(casi(128, 512)))]

    #[test]
    fn oracolo_casuale(
        sinistra in prop::collection::vec(chiave(), 0..40),
        destra in prop::collection::vec(chiave(), 0..20),
        metric in prop::sample::select(METRICHE.to_vec()),
        blocking in prop::sample::select(BLOCKING.to_vec()),
        threshold in soglia(),
        left_join in any::<bool>(),
        case_sensitive in any::<bool>(),
        max_candidates in prop::option::of(1_usize..25),
        max_rows in prop_oneof![Just(usize::MAX), 0_usize..60],
    ) {
        let sinistra: Vec<Option<&str>> = sinistra.iter().map(Option::as_deref).collect();
        let destra: Vec<Option<&str>> = destra.iter().map(Option::as_deref).collect();
        let how = if left_join { FuzzyHow::Left } else { FuzzyHow::Inner };
        let config = config_di(metric, threshold, blocking, how, case_sensitive, max_candidates);
        let limits = Limits { max_rows, ..Limits::default() };
        confronta(&tabella(&sinistra), &tabella(&destra), &config, &limits);
    }
}

// -- Oracoli dei filtri per coppia -------------------------------------------

/// Soglie per i filtri: fisse, i punteggi esatti `1 - d/m` fino a `m = 40`
/// e i loro vicini `next_up`/`next_down`.
fn soglie_dei_filtri() -> Vec<f64> {
    let mut soglie = soglie_fisse();
    for m in 1_u32..=40 {
        for d in 0..=m {
            let score = 1.0 - f64::from(d) / f64::from(m);
            soglie.extend([score, score.next_up(), score.next_down()]);
        }
    }
    soglie.retain(|&soglia| soglia > 0.0 && soglia <= 1.0);
    soglie
}

#[test]
fn oracolo_distanza_massima_esaustivo() {
    // Per ogni lunghezza e soglia: una distanza e' ammessa dal filtro se e
    // solo se lo score del riferimento (stessa espressione, scritta qui alla
    // lettera) e' >= soglia.
    for threshold in soglie_dei_filtri() {
        for max_len in 1_usize..=300 {
            let limite = super::distanza_massima(max_len, threshold);
            for distanza in 0..=max_len {
                #[allow(clippy::cast_precision_loss)]
                let score = 1.0 - distanza as f64 / max_len as f64;
                assert_eq!(
                    distanza <= limite,
                    score >= threshold,
                    "max_len {max_len}, distanza {distanza}, soglia {threshold}"
                );
            }
        }
    }
}

/// `levenshtein_entro` contro la DP completa del riferimento per ogni
/// limite da 0 a oltre la lunghezza, con buffer riusati fra le coppie.
fn confronta_levenshtein_entro(coppie: &[(Vec<char>, Vec<char>)]) {
    let (mut previous, mut current) = (Vec::new(), Vec::new());
    let (mut ref_previous, mut ref_current) = (Vec::new(), Vec::new());
    for (left, right) in coppie {
        let distanza =
            levenshtein_distance_scratch(left, right, &mut ref_previous, &mut ref_current);
        for limite in 0..=left.len().max(right.len()) + 1 {
            assert_eq!(
                super::levenshtein_entro(left, right, limite, &mut previous, &mut current),
                (distanza <= limite).then_some(distanza),
                "{left:?} {right:?} limite {limite}"
            );
        }
    }
}

#[test]
fn oracolo_levenshtein_entro_casi_avversari() {
    let chiavi: Vec<Vec<char>> = CHIAVI
        .iter()
        .flatten()
        .map(|chiave| chiave.chars().collect())
        .collect();
    let coppie: Vec<(Vec<char>, Vec<char>)> = chiavi
        .iter()
        .flat_map(|left| {
            chiavi
                .iter()
                .map(move |right| (left.clone(), right.clone()))
        })
        .collect();
    confronta_levenshtein_entro(&coppie);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn oracolo_levenshtein_entro_casuale(
        coppie in prop::collection::vec(
            (
                prop::collection::vec(prop::sample::select(vec!['a', 'b', 'c', '\u{e9}']), 0..30),
                prop::collection::vec(prop::sample::select(vec!['a', 'b', 'c', '\u{e9}']), 0..30),
            ),
            1..6,
        ),
    ) {
        confronta_levenshtein_entro(&coppie);
    }
}

/// `jaro_winkler_sopra_soglia` contro Jaro-Winkler del riferimento, con la
/// soglia sullo score della coppia e sui suoi vicini: sopra soglia lo
/// score e' lo stesso bit per bit, sotto soglia la coppia si scarta o si
/// calcola (il probe la filtra comunque). Conta le coppie scartate.
fn confronta_jaro_winkler_sopra_soglia(coppie: &[(Vec<char>, Vec<char>)], soglie: &[f64]) -> usize {
    let mut scratch = super::FuzzyScratch::default();
    let mut riferimento = FuzzyScratch::default();
    let mut scartate = 0;
    for (left, right) in coppie {
        let atteso = jaro_winkler_chars(left, right, &mut riferimento);
        let mut left_sorted = left.clone();
        left_sorted.sort_unstable();
        let mut right_sorted = right.clone();
        right_sorted.sort_unstable();
        let vicine = [atteso, atteso.next_up(), atteso.next_down()];
        for &threshold in soglie.iter().chain(&vicine) {
            let ottenuto = super::jaro_winkler_sopra_soglia(
                (left, &left_sorted),
                (right, &right_sorted),
                threshold,
                &mut scratch,
            );
            if let Some(score) = ottenuto {
                assert_eq!(score.to_bits(), atteso.to_bits(), "{left:?} {right:?}");
            } else {
                assert!(atteso < threshold, "{left:?} {right:?} soglia {threshold}");
                scartate += 1;
            }
        }
    }
    scartate
}

#[test]
fn oracolo_jaro_winkler_sopra_soglia_casi_avversari() {
    let mut chiavi: Vec<Vec<char>> = CHIAVI
        .iter()
        .flatten()
        .map(|chiave| chiave.chars().collect())
        .collect();
    chiavi.extend(
        [
            "dixon",
            "dicksonx",
            "jellyfish",
            "smellyfish",
            "abcdefghijkl",
            "lkjihgfedcba",
        ]
        .iter()
        .map(|chiave| chiave.chars().collect()),
    );
    let coppie: Vec<(Vec<char>, Vec<char>)> = chiavi
        .iter()
        .flat_map(|left| {
            chiavi
                .iter()
                .map(move |right| (left.clone(), right.clone()))
        })
        .collect();
    let scartate = confronta_jaro_winkler_sopra_soglia(&coppie, &soglie_fisse());
    assert!(scartate > 0, "il bound deve scartare qualche coppia");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn oracolo_jaro_winkler_sopra_soglia_casuale(
        coppie in prop::collection::vec(
            (
                prop::collection::vec(prop::sample::select(vec!['a', 'b', 'c', 'd', '\u{e9}']), 0..20),
                prop::collection::vec(prop::sample::select(vec!['a', 'b', 'c', 'd', '\u{e9}']), 0..20),
            ),
            1..6,
        ),
        soglia in soglia(),
    ) {
        confronta_jaro_winkler_sopra_soglia(&coppie, &[soglia]);
    }
}
