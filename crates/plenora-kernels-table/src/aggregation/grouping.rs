use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write as _;

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use rayon::prelude::*;

use plenora_core::{PlenoraError, Result};

use crate::{scalar_as_f64, scalar_as_string};

// ---------------------------------------------------------------------------
// Fast path di `table.aggregate` (ottimizzazione kernel, secondo batch).
//
// Tre interventi, semantica byte-identica al percorso generico:
// 1. chiavi di gruppo scritte da formattatori tipizzati (stessi byte di
//    `row_key`, nessuna conversione scalare ripetuta ne' stringhe intermedie)
//    e raggruppamento HashMap + ordinamento finale delle chiavi (stesso
//    ordine del BTreeMap: lessicografico sui byte della chiave);
// 2. aggregazioni numeriche su valori nativi Arrow (Int64/UInt64/Float64)
//    con la stessa sequenza di operazioni del generico (stesso ordine di
//    somma, `total_cmp` per distinct/quantile, null esclusi o gruppo nullo
//    secondo `skip_null`); gli altri tipi ricadono su `scalar_as_f64`;
// 3. nunique/concat su Utf8 con valori presi in prestito (nessuna copia);
//    gli altri tipi ricadono su `scalar_as_string`.
// ---------------------------------------------------------------------------

/// Colonna di group-by con formattatore tipizzato: produce gli stessi byte
/// di `row_key` (`{tipo}\u{1e}{1|0}{len}:{value}\u{1f}`).
///
/// `pub(crate)` per il modulo `spill` (Fase 2B): il partizionamento hash e la
/// ricostruzione dell'ordine canonico dei gruppi riusano gli stessi byte di
/// chiave, cosi' i percorsi spilled hanno identita' di gruppo identica.
pub enum KeyColumn {
    Int64 { prefix: String, values: Int64Array },
    UInt64 { prefix: String, values: UInt64Array },
    Float64 { prefix: String, values: Float64Array },
    Utf8 { prefix: String, values: StringArray },
    Boolean { prefix: String, values: BooleanArray },
    /// Qualunque altro tipo: chiave via `scalar_as_string`, come prima.
    Generic { prefix: String, array: ArrayRef },
}

impl KeyColumn {
    pub(crate) fn new(array: &ArrayRef) -> Self {
        let prefix = array.data_type().to_string();
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64 {
                prefix,
                values: values.clone(),
            };
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64 {
                prefix,
                values: values.clone(),
            };
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64 {
                prefix,
                values: values.clone(),
            };
        }
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8 {
                prefix,
                values: values.clone(),
            };
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Self::Boolean {
                prefix,
                values: values.clone(),
            };
        }
        Self::Generic {
            prefix,
            array: array.clone(),
        }
    }

    pub(crate) fn write_key(&self, row: usize, key: &mut String, scratch: &mut String) -> Result<()> {
        match self {
            Self::Int64 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    scratch.clear();
                    // La scrittura su String non fallisce mai; l'errore resta
                    // esplicito perche' fmt::Result non lo dimostra (R6).
                    write!(scratch, "{}", values.value(row)).map_err(|_| {
                        PlenoraError::Internal("formattazione chiave di gruppo su String".into())
                    })?;
                    push_key_value(key, scratch)?;
                }
            }
            Self::UInt64 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    scratch.clear();
                    write!(scratch, "{}", values.value(row)).map_err(|_| {
                        PlenoraError::Internal("formattazione chiave di gruppo su String".into())
                    })?;
                    push_key_value(key, scratch)?;
                }
            }
            Self::Float64 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    scratch.clear();
                    write!(scratch, "{}", values.value(row)).map_err(|_| {
                        PlenoraError::Internal("formattazione chiave di gruppo su String".into())
                    })?;
                    push_key_value(key, scratch)?;
                }
            }
            Self::Boolean { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    // "true"/"false": stessi byte di bool::to_string.
                    push_key_value(key, if values.value(row) { "true" } else { "false" })?;
                }
            }
            Self::Utf8 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    push_key_value(key, values.value(row))?;
                }
            }
            Self::Generic { prefix, array } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                match scalar_as_string(array.as_ref(), row)? {
                    Some(value) => push_key_value(key, &value)?,
                    None => key.push('0'),
                }
            }
        }
        key.push('\u{1f}');
        Ok(())
    }
}

/// Frammento `1{len}:{value}` della chiave di `row_key`.
fn push_key_value(key: &mut String, value: &str) -> Result<()> {
    key.push('1');
    // Come sopra: fmt su String e' infallibile, ma l'errore e' esplicito.
    write!(key, "{}", value.len()).map_err(|_| {
        PlenoraError::Internal("formattazione chiave di gruppo su String".into())
    })?;
    key.push(':');
    key.push_str(value);
    Ok(())
}

/// Hasher moltiplicativo a blocchi (stile `FxHash`) con finalizer splitmix64
/// per le chiavi di gruppo.
///
/// `SipHash` (default std) domina il costo di raggruppamento su milioni di
/// righe; qui il throughput conta piu' della resistenza a input avversari,
/// come gia' accettato da `distinct` (`HashMap` sulle stesse chiavi). Il
/// finalizer e' necessario: senza, le chiavi con prefisso comune lungo
/// (stesso tipo, stessa lunghezza) si concentrano in pochi bucket
/// (verificato: max 328 contro 7 con finalizer).
#[derive(Default)]
pub struct KeyHasher(u64);

impl std::hash::Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            // chunks_exact(8) garantisce blocchi pieni: la copia su array
            // fisso non puo' fallire e non serve alcuna conversione.
            let mut block = [0_u8; 8];
            block.copy_from_slice(chunk);
            let value = u64::from_le_bytes(block);
            self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(K);
        }
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            let mut tail = 0_u64;
            for &byte in remainder {
                tail = (tail << 8) | u64::from(byte);
            }
            self.0 = (self.0.rotate_left(5) ^ tail).wrapping_mul(K);
        }
    }
}

type KeyMap = HashMap<String, usize, std::hash::BuildHasherDefault<KeyHasher>>;

/// Soglia condivisa per l'uso di rayon (ordinamento chiavi e calcolo per
/// gruppo): sotto soglia l'overhead non ripaga.
pub(in crate::aggregation) const PARALLEL_THRESHOLD: usize = 32_768;

/// Cifre decimali di un intero senza segno (0 conta come una cifra).
const fn decimal_digits(value: u64) -> u32 {
    if value == 0 { 1 } else { value.ilog10() + 1 }
}

/// Confronto lessicografico dei "tag di lunghezza" `{decimal(len)}:` delle
/// chiavi di `row_key`.
///
/// Quando una rappresentazione finisce, il suo byte successivo nella chiave
/// e' ':' (0x3A), maggiore di ogni cifra: il piu' corto, a parita' di
/// prefisso, e' quindi MAGGIORE.
fn cmp_len_tag(a: u64, b: u64) -> Ordering {
    let digits_a = decimal_digits(a);
    let digits_b = decimal_digits(b);
    if digits_a == digits_b {
        return a.cmp(&b);
    }
    if digits_a < digits_b {
        let prefix = b / 10_u64.pow(digits_b - digits_a);
        a.cmp(&prefix).then(Ordering::Greater)
    } else {
        let prefix = a / 10_u64.pow(digits_a - digits_b);
        prefix.cmp(&b).then(Ordering::Less)
    }
}

/// Ordine delle chiavi di `row_key` per una colonna Int64, senza
/// materializzare le stringhe.
///
/// Null in testa (gestito dal chiamante), poi tag di lunghezza, poi forma
/// decimale (il segno '-' precede le cifre, tra negativi l'ordine
/// lessicografico e' l'inverso di quello numerico).
pub(in crate::aggregation) fn cmp_i64_group_key(a: i64, b: i64) -> Ordering {
    let digits_a = u64::from(decimal_digits(a.unsigned_abs()));
    let digits_b = u64::from(decimal_digits(b.unsigned_abs()));
    cmp_len_tag(
        digits_a + u64::from(a < 0),
        digits_b + u64::from(b < 0),
    )
    .then_with(|| match (a < 0, b < 0) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) => b.cmp(&a),
        (false, false) => a.cmp(&b),
    })
}

/// Ordine delle chiavi di `row_key` per una colonna `UInt64`: tag di
/// lunghezza della forma decimale, poi valore (a parita' di cifre
/// lessicografico e numerico coincidono).
pub(in crate::aggregation) fn cmp_u64_group_key(a: u64, b: u64) -> Ordering {
    cmp_len_tag(
        u64::from(decimal_digits(a)),
        u64::from(decimal_digits(b)),
    )
    .then_with(|| a.cmp(&b))
}

/// Ordine delle chiavi di `row_key` per una colonna Utf8: tag di lunghezza
/// in byte, poi confronto per byte.
pub(in crate::aggregation) fn cmp_str_group_key(a: &str, b: &str) -> Ordering {
    cmp_len_tag(a.len() as u64, b.len() as u64).then_with(|| a.cmp(b))
}

/// Raggruppamento su chiave nativa di colonna singola (Int64/UInt64/Utf8).
///
/// Nessuna stringa di chiave, hash del valore nativo, ordinamento finale
/// con il comparatore che riproduce l'ordine lessicografico delle chiavi
/// di `row_key`. Il gruppo dei null, se presente, e' sempre in testa
/// (`"...0"` precede `"...1..."` nelle chiavi testuali).
pub(in crate::aggregation) fn build_native_groups<K: Copy + Eq + std::hash::Hash + Send + Sync>(
    rows: usize,
    key_at: impl Fn(usize) -> Option<K> + Sync,
    cmp: impl Fn(&K, &K) -> Ordering + Sync,
) -> Vec<Vec<usize>> {
    let mut lookup: HashMap<K, usize, std::hash::BuildHasherDefault<KeyHasher>> = HashMap::default();
    let mut null_group: Option<usize> = None;
    let mut group_rows: Vec<Vec<usize>> = Vec::new();
    for row in 0..rows {
        match key_at(row) {
            Some(key) => {
                if let Some(group) = lookup.get(&key) {
                    group_rows[*group].push(row);
                } else {
                    lookup.insert(key, group_rows.len());
                    group_rows.push(vec![row]);
                }
            }
            None => {
                if let Some(group) = null_group {
                    group_rows[group].push(row);
                } else {
                    null_group = Some(group_rows.len());
                    group_rows.push(vec![row]);
                }
            }
        }
    }
    let mut keyed = lookup.into_iter().collect::<Vec<_>>();
    if keyed.len() >= PARALLEL_THRESHOLD {
        keyed.par_sort_by(|left, right| cmp(&left.0, &right.0));
    } else {
        keyed.sort_by(|left, right| cmp(&left.0, &right.0));
    }
    let mut groups = Vec::with_capacity(group_rows.len());
    if let Some(null_group) = null_group {
        groups.push(std::mem::take(&mut group_rows[null_group]));
    }
    groups.extend(
        keyed
            .iter()
            .map(|(_, index)| std::mem::take(&mut group_rows[*index])),
    );
    groups
}

/// Raggruppamento generico sulle chiavi testuali di `row_key` (multi-colonna
/// o tipi fuori dal fast path nativo).
///
/// Stessi byte, `HashMap` + ordinamento finale (stesso ordine lessicografico
/// del `BTreeMap` originale).
pub(in crate::aggregation) fn build_string_groups(batch: &RecordBatch, group_indices: &[usize]) -> Result<Vec<Vec<usize>>> {
    let key_columns = group_indices
        .iter()
        .map(|index| KeyColumn::new(batch.column(*index)))
        .collect::<Vec<_>>();
    let mut lookup: KeyMap = KeyMap::default();
    let mut group_rows: Vec<Vec<usize>> = Vec::new();
    let mut key = String::new();
    let mut scratch = String::new();
    for row in 0..batch.num_rows() {
        key.clear();
        for column in &key_columns {
            column.write_key(row, &mut key, &mut scratch)?;
        }
        if let Some(group) = lookup.get(key.as_str()) {
            group_rows[*group].push(row);
        } else {
            lookup.insert(key.clone(), group_rows.len());
            group_rows.push(vec![row]);
        }
    }
    // Ordine canonico del BTreeMap originale: lessicografico sui byte della
    // chiave. Le chiavi sono univoche, il risultato e' deterministico.
    let mut keyed = lookup.into_iter().collect::<Vec<_>>();
    if keyed.len() >= PARALLEL_THRESHOLD {
        keyed.par_sort_by(|left, right| left.0.cmp(&right.0));
    } else {
        keyed.sort_by(|left, right| left.0.cmp(&right.0));
    }
    let groups = keyed
        .iter()
        .map(|(_, index)| std::mem::take(&mut group_rows[*index]))
        .collect::<Vec<_>>();
    drop(keyed);
    Ok(groups)
}

/// Sorgente numerica per le aggregazioni Float64: valori nativi Arrow per i
/// tipi principali, `scalar_as_f64` (invariato) per gli altri.
pub(in crate::aggregation) enum NumericSource<'a> {
    Float64(&'a Float64Array),
    Int64(&'a Int64Array),
    UInt64(&'a UInt64Array),
    Generic(&'a ArrayRef),
}

impl<'a> NumericSource<'a> {
    pub(in crate::aggregation) fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64(values);
        }
        Self::Generic(array)
    }

    pub(in crate::aggregation) fn value(&self, row: usize) -> Result<Option<f64>> {
        match self {
            Self::Float64(values) => Ok(if values.is_null(row) {
                None
            } else {
                Some(values.value(row))
            }),
            Self::Int64(values) => {
                if values.is_null(row) {
                    return Ok(None);
                }
                values
                    .value(row)
                    .to_f64()
                    .map(Some)
                    .ok_or_else(|| PlenoraError::Schema("intero non rappresentabile come f64".into()))
            }
            Self::UInt64(values) => {
                if values.is_null(row) {
                    return Ok(None);
                }
                values.value(row).to_f64().map(Some).ok_or_else(|| {
                    PlenoraError::Schema("uint64 non rappresentabile come f64".into())
                })
            }
            Self::Generic(array) => scalar_as_f64(array.as_ref(), row),
        }
    }
}

/// Sorgente testuale per nunique/concat: valori Utf8 presi in prestito,
/// `scalar_as_string` (invariato) per gli altri tipi.
pub(in crate::aggregation) enum TextSource<'a> {
    Utf8(&'a StringArray),
    Generic(&'a ArrayRef),
}

impl<'a> TextSource<'a> {
    pub(in crate::aggregation) fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        Self::Generic(array)
    }

    pub(in crate::aggregation) fn value(&self, row: usize) -> Result<Option<Cow<'a, str>>> {
        match self {
            Self::Utf8(values) => Ok(if values.is_null(row) {
                None
            } else {
                Some(Cow::Borrowed(values.value(row)))
            }),
            Self::Generic(array) => Ok(scalar_as_string(array.as_ref(), row)?.map(Cow::Owned)),
        }
    }
}

/// Applica `f` ai gruppi in ordine canonico; sopra soglia in parallelo con
/// rayon (raccolta posizionale: output identico al sequenziale).
pub(in crate::aggregation) fn map_groups<T>(
    groups: &[Vec<usize>],
    parallel: bool,
    f: impl Fn(&[usize]) -> Result<T> + Sync,
) -> Result<Vec<T>>
where
    T: Send,
{
    if parallel {
        groups.par_iter().map(|rows| f(rows)).collect()
    } else {
        groups.iter().map(|rows| f(rows)).collect()
    }
}

/// Partizioni di `build_partitions`: chiave testuale della colonna di
/// partizione e indici di riga della partizione.
type KeyPartitions<'a> = Vec<(Option<Cow<'a, str>>, Vec<usize>)>;

/// Partizioni di `window_function`/`rolling_window` (ottimizzazione kernel,
/// batch 4).
///
/// Righe raggruppate per la chiave testuale della colonna di partizione
/// (`TextSource`: Utf8 preso in prestito, nessuna `String` per riga;
/// `scalar_as_string` per gli altri tipi), hash FxHash+splitmix64
/// (`KeyHasher`) al posto del `BTreeMap` `SipHash`. Le partizioni sono
/// restituite nello STESSO ordine di iterazione del `BTreeMap` originale
/// (chiave `Option<String>` crescente): gli errori per partizione emergono
/// nello stesso ordine e il comportamento resta deterministico.
pub(in crate::aggregation) fn build_partitions(batch: &RecordBatch, group: Option<usize>) -> Result<KeyPartitions<'_>> {
    let source = group.map(|index| TextSource::new(batch.column(index)));
    let mut lookup: HashMap<Option<Cow<'_, str>>, usize, std::hash::BuildHasherDefault<KeyHasher>> =
        HashMap::default();
    let mut partitions: Vec<(Option<Cow<'_, str>>, Vec<usize>)> = Vec::new();
    for row in 0..batch.num_rows() {
        let key = source
            .as_ref()
            .map(|source| source.value(row))
            .transpose()?
            .flatten();
        if let Some(index) = lookup.get(&key) { partitions[*index].1.push(row) } else {
            lookup.insert(key.clone(), partitions.len());
            partitions.push((key, vec![row]));
        }
    }
    // Le chiavi sono univoche: l'ordinamento e' esatto e deterministico.
    partitions.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(partitions)
}

/// Calcola i valori di output per partizione e li scrive alle posizioni di
/// riga originali.
///
/// Sopra soglia il calcolo va in parallelo con raccolta posizionale (stessi
/// valori del sequenziale); la scrittura riproduce il riempimento diretto
/// originale.
pub(in crate::aggregation) fn scatter_partitions(
    batch: &RecordBatch,
    partitions: &[(Option<Cow<'_, str>>, Vec<usize>)],
    output: &mut [Option<f64>],
    compute: impl Fn(&[usize]) -> Result<Vec<Option<f64>>> + Sync,
) -> Result<()> {
    let parallel = batch.num_rows() >= PARALLEL_THRESHOLD && partitions.len() > 1;
    let partials = if parallel {
        partitions
            .par_iter()
            .map(|(_, rows)| compute(rows))
            .collect::<Result<Vec<_>>>()?
    } else {
        partitions
            .iter()
            .map(|(_, rows)| compute(rows))
            .collect::<Result<Vec<_>>>()?
    };
    for ((_, rows), values) in partitions.iter().zip(&partials) {
        for (row, value) in rows.iter().zip(values) {
            output[*row] = *value;
        }
    }
    Ok(())
}
