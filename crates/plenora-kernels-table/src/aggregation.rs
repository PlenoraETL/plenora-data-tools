use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use num_traits::ToPrimitive;
use rayon::prelude::*;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};
use crate::{
    column_index, replace_or_append, scalar_as_f64, scalar_as_string, select_rows,
    validate_output_name,
};

#[cfg(test)] // Solo i test-oracolo usano il percorso testuale originale.
fn row_key(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<String> {
    let mut key = String::new();
    for index in indices {
        let value = scalar_as_string(batch.column(*index).as_ref(), row)?;
        key.push_str(batch.column(*index).data_type().to_string().as_str());
        key.push('\u{1e}');
        match value {
            Some(value) => {
                key.push('1');
                key.push_str(&value.len().to_string());
                key.push(':');
                key.push_str(&value);
            }
            None => key.push('0'),
        }
        key.push('\u{1f}');
    }
    Ok(key)
}

/// Confronto tipizzato tra due celle, condiviso dai tre siti di confronto di
/// `table.sort`.
///
/// I tre siti sono `compare_at` (stesso batch), `ColumnComparator` (fast
/// path tipizzato) e il merge k-way dello spill (`spill::compare_cells`,
/// batch diversi — da qui la forma a due array).
///
/// Semantica: null dopo i valori (uguaglianza tra null); confronto nativo
/// esatto per Int64 (`i64::cmp`: la conversione a f64 collasserebbe valori
/// distinti oltre 2^53 sullo stesso double) e per `UInt64` (`u64::cmp`: il
/// fallback testuale ordinerebbe "10" prima di "9"); `total_cmp` per
/// Float64 (invariato); fallback `scalar_as_string` per gli altri tipi
/// (invariato, stesso ordine storico del kernel). Le colonne confrontate
/// appartengono allo stesso schema; se i tipi non coincidono con quello
/// atteso si ricade comunque sul fallback testuale.
pub(crate) fn compare_cells_typed(
    left: &ArrayRef,
    left_row: usize,
    right: &ArrayRef,
    right_row: usize,
) -> Result<Ordering> {
    let left_null = left.is_null(left_row);
    let right_null = right.is_null(right_row);
    if left_null || right_null {
        return Ok(match (left_null, right_null) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => unreachable!(),
        });
    }
    match left.data_type() {
        DataType::Int64 => {
            if let (Some(left_values), Some(right_values)) = (
                left.as_any().downcast_ref::<Int64Array>(),
                right.as_any().downcast_ref::<Int64Array>(),
            ) {
                return Ok(left_values.value(left_row).cmp(&right_values.value(right_row)));
            }
        }
        DataType::UInt64 => {
            if let (Some(left_values), Some(right_values)) = (
                left.as_any().downcast_ref::<UInt64Array>(),
                right.as_any().downcast_ref::<UInt64Array>(),
            ) {
                return Ok(left_values.value(left_row).cmp(&right_values.value(right_row)));
            }
        }
        DataType::Float64 => {
            if let (Some(left_values), Some(right_values)) = (
                left.as_any().downcast_ref::<Float64Array>(),
                right.as_any().downcast_ref::<Float64Array>(),
            ) {
                return Ok(left_values
                    .value(left_row)
                    .total_cmp(&right_values.value(right_row)));
            }
        }
        _ => {}
    }
    Ok(scalar_as_string(left.as_ref(), left_row)?
        .cmp(&scalar_as_string(right.as_ref(), right_row)?))
}

fn compare_at(batch: &RecordBatch, index: usize, left: usize, right: usize) -> Result<Ordering> {
    let array = batch.column(index);
    compare_cells_typed(array, left, array, right)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sort {
    pub columns: Vec<String>,
    #[serde(default = "default_true")]
    pub ascending: bool,
}
const fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Comparatori tipizzati di `table.sort` (ottimizzazione kernel, Fase post-2A).
//
// Per i tipi Arrow principali (Int64, UInt64, Float64, Utf8, Boolean) il
// confronto avviene sui valori nativi, senza conversione scalare ad ogni
// confronto; la semantica e' IDENTICA a `compare_at`/`compare_cells_typed`
// (null dopo i valori in ascendente, `i64::cmp`/`u64::cmp` esatti per gli
// interi — nessuna perdita di precisione oltre 2^53 — `total_cmp` per i
// Float64, confronto testuale "false" < "true" per i booleani). Gli altri
// tipi ricadono su `compare_at`, invariato.
// ---------------------------------------------------------------------------

enum ColumnComparator {
    Int64(Int64Array),
    UInt64(UInt64Array),
    Float64(Float64Array),
    Utf8(StringArray),
    Boolean(BooleanArray),
    /// Colonna gestita dal percorso generico (indice nel batch).
    Generic(usize),
}

impl ColumnComparator {
    fn new(index: usize, array: &ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64(values.clone());
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64(values.clone());
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64(values.clone());
        }
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values.clone());
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Self::Boolean(values.clone());
        }
        Self::Generic(index)
    }

    fn compare(&self, batch: &RecordBatch, left: usize, right: usize) -> Result<Ordering> {
        match self {
            Self::Int64(values) => Ok(compare_nullable(values, left, right, |values, l, r| {
                values.value(l).cmp(&values.value(r))
            })),
            Self::UInt64(values) => Ok(compare_nullable(values, left, right, |values, l, r| {
                values.value(l).cmp(&values.value(r))
            })),
            Self::Float64(values) => Ok(compare_nullable(values, left, right, |values, l, r| {
                values.value(l).total_cmp(&values.value(r))
            })),
            Self::Utf8(values) => Ok(compare_nullable(values, left, right, |values, l, r| {
                values.value(l).cmp(values.value(r))
            })),
            // "false" < "true" lessicografico coincide con false < true nativo.
            Self::Boolean(values) => Ok(compare_nullable(values, left, right, |values, l, r| {
                values.value(l).cmp(&values.value(r))
            })),
            Self::Generic(index) => compare_at(batch, *index, left, right),
        }
    }
}

/// Regola null di `compare_at`: null dopo i non-null (poi rovesciata in
/// discendente), uguaglianza tra null.
fn compare_nullable<A: Array>(
    values: &A,
    left: usize,
    right: usize,
    compare: impl Fn(&A, usize, usize) -> Ordering,
) -> Ordering {
    let left_null = values.is_null(left);
    let right_null = values.is_null(right);
    if left_null || right_null {
        return match (left_null, right_null) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => unreachable!(),
        };
    }
    compare(values, left, right)
}

// Il guard del mutex nel comparatore e' gia' a scope minimo (blocco
// `Err`): stringerlo non cambia la concorrenza e la riscrittura suggerita
// peggiorerebbe il percorso di errore condiviso tra sort seriale e
// parallela.
#[allow(clippy::significant_drop_tightening)]
/// Batch ordinato per `config.columns` (sort stabile, null in coda in
/// ascendente).
///
/// # Errors
///
/// - `Contract`: `columns` vuoto;
/// - `Schema`: una colonna di `columns` assente dallo schema; in piu' gli
///   errori di `scalar_as_string` (fallback testuale per i tipi fuori dal
///   fast path) e di `select_rows`.
pub fn sort(batch: &RecordBatch, config: &Sort) -> Result<RecordBatch> {
    // Sotto soglia il merge sort parallelo di rayon non ripaga l'overhead;
    // entrambi i percorsi sono stabili, quindi la permutazione e' identica.
    const PARALLEL_THRESHOLD: usize = 32_768;
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    if indices.is_empty() {
        return Err(PlenoraError::Contract("sort richiede colonne".into()));
    }
    let comparators = indices
        .iter()
        .map(|index| ColumnComparator::new(*index, batch.column(*index)))
        .collect::<Vec<_>>();
    let mut rows: Vec<usize> = (0..batch.num_rows()).collect();
    let failure = Mutex::new(None);
    let compare = |left: &usize, right: &usize| {
        for comparator in &comparators {
            match comparator.compare(batch, *left, *right) {
                Ok(Ordering::Equal) => {}
                Ok(ordering) => {
                    return if config.ascending {
                        ordering
                    } else {
                        ordering.reverse()
                    };
                }
                Err(error) => {
                    let mut slot = failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if slot.is_none() {
                        *slot = Some(error);
                    }
                    return Ordering::Equal;
                }
            }
        }
        left.cmp(right)
    };
    if rows.len() >= PARALLEL_THRESHOLD {
        rows.par_sort_by(compare);
    } else {
        rows.sort_by(compare);
    }
    if let Some(error) = failure
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return Err(error);
    }
    select_rows(batch, &rows)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopN {
    pub columns: Vec<String>,
    pub n: u64,
    #[serde(default)]
    pub descending: bool,
}

/// Prime `n` righe secondo l'ordinamento di `sort` (estensione v1.1).
///
/// Stessa semantica null (null in coda in ascendente), `total_cmp` sui
/// numerici e stabilita' (spareggio sull'indice originale). L'output e'
/// identico a `sort` seguito da `limit(n)`, ma con `n << righe` evita il
/// sort completo: `select_nth_unstable_by` partiziona gli indici in O(righe)
/// e solo i primi `n` selezionati vengono ordinati. Il confronto include lo
/// spareggio sull'indice, quindi e' un ordine totale e la permutazione
/// finale coincide esattamente con quella dello stable sort completo.
///
/// # Errors
///
/// - `Contract`: `columns` vuoto, oppure `n` non rappresentabile come
///   `usize`;
/// - `Schema`: una colonna di `columns` assente dallo schema; in piu' gli
///   errori di `scalar_as_string` (fallback testuale per i tipi fuori dal
///   fast path) e di `select_rows`.
pub fn top_n(batch: &RecordBatch, config: &TopN) -> Result<RecordBatch> {
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    if indices.is_empty() {
        return Err(PlenoraError::Contract("top_n richiede colonne".into()));
    }
    let n = usize::try_from(config.n)
        .map_err(|_| PlenoraError::Contract("top_n: n oltre usize".into()))?
        .min(batch.num_rows());
    if n == 0 {
        // n = 0: batch vuoto con schema invariato (colonne gia' validate).
        return Ok(batch.slice(0, 0));
    }
    let comparators = indices
        .iter()
        .map(|index| ColumnComparator::new(*index, batch.column(*index)))
        .collect::<Vec<_>>();
    let mut rows: Vec<usize> = (0..batch.num_rows()).collect();
    let mut failure: Option<PlenoraError> = None;
    let mut compare = |left: &usize, right: &usize| {
        for comparator in &comparators {
            match comparator.compare(batch, *left, *right) {
                Ok(Ordering::Equal) => {}
                Ok(ordering) => {
                    return if config.descending {
                        ordering.reverse()
                    } else {
                        ordering
                    };
                }
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                    return Ordering::Equal;
                }
            }
        }
        left.cmp(right)
    };
    if n < rows.len() {
        rows.select_nth_unstable_by(n - 1, &mut compare);
        rows.truncate(n);
    }
    rows.sort_by(&mut compare);
    if let Some(error) = failure {
        return Err(error);
    }
    select_rows(batch, &rows)
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    First,
    Last,
    False,
}
const fn default_keep() -> Keep {
    Keep::First
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distinct {
    #[serde(default)]
    pub subset: Vec<String>,
    #[serde(default = "default_keep")]
    pub keep: Keep,
}

/// Righe distinte sulle colonne di `subset` (default: tutte le colonne),
/// selezionate secondo `keep` (prima/ultima occorrenza o solo righe senza
/// duplicati).
///
/// # Errors
///
/// - `Schema`: una colonna di `subset` assente dallo schema; in piu' gli
///   errori di `scalar_as_string` (colonne fuori dal fast path tipizzato)
///   e di `select_rows`.
pub fn distinct(batch: &RecordBatch, config: &Distinct) -> Result<RecordBatch> {
    struct KeyStats {
        first: usize,
        last: usize,
        count: usize,
    }
    let indices = if config.subset.is_empty() {
        (0..batch.num_columns()).collect()
    } else {
        config
            .subset
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?
    };
    // Una sola passata sulle righe: chiave con gli stessi byte di `row_key`
    // (formattatori tipizzati di `KeyColumn`) scritta in un buffer riusato,
    // hash FxHash+splitmix64 (`KeyHasher`) al posto di SipHash. L'originale
    // materializzava una `String` per riga piu' tre mappe SipHash; qui una
    // sola mappa chiave -> statistiche (prima/ultima occorrenza, conteggio).
    // Le righe in uscita restano in ordine crescente di indice per ogni
    // variante di `keep`, esattamente come il filtro sull'indice originale.
    let key_columns = indices
        .iter()
        .map(|index| KeyColumn::new(batch.column(*index)))
        .collect::<Vec<_>>();
    let mut stats: HashMap<Box<[u8]>, KeyStats, std::hash::BuildHasherDefault<KeyHasher>> =
        HashMap::default();
    let mut key = String::new();
    let mut scratch = String::new();
    for row in 0..batch.num_rows() {
        key.clear();
        for column in &key_columns {
            column.write_key(row, &mut key, &mut scratch)?;
        }
        match stats.get_mut(key.as_bytes()) {
            Some(entry) => {
                entry.last = row;
                entry.count += 1;
            }
            None => {
                stats.insert(
                    key.clone().into_bytes().into_boxed_slice(),
                    KeyStats {
                        first: row,
                        last: row,
                        count: 1,
                    },
                );
            }
        }
    }
    let mut rows = stats
        .values()
        .filter_map(|entry| match config.keep {
            Keep::First => Some(entry.first),
            Keep::Last => Some(entry.last),
            Keep::False => (entry.count == 1).then_some(entry.first),
        })
        .collect::<Vec<_>>();
    rows.sort_unstable();
    select_rows(batch, &rows)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DedupAdvanced {
    pub subset: Vec<String>,
    #[serde(default = "default_keep")]
    pub keep: Keep,
    pub order_column: Option<String>,
    #[serde(default = "default_true")]
    pub ascending: bool,
}

/// `distinct` con pre-ordinamento su `order_column`: prima/ultima
/// occorrenza si riferiscono all'ordine dato.
///
/// # Errors
///
/// - `Contract`: `keep` e' `Keep::False` (non supportato);
/// - come `sort` (se `order_column` e' presente) e `distinct`: colonne
///   assenti (`Schema`), errori del fallback testuale e di `select_rows`.
pub fn dedup_advanced(batch: &RecordBatch, config: &DedupAdvanced) -> Result<RecordBatch> {
    let ordered = if let Some(column) = &config.order_column {
        sort(
            batch,
            &Sort {
                columns: vec![column.clone()],
                ascending: config.ascending,
            },
        )?
    } else {
        batch.clone()
    };
    distinct(
        &ordered,
        &Distinct {
            subset: config.subset.clone(),
            keep: match config.keep {
                Keep::First => Keep::First,
                Keep::Last => Keep::Last,
                Keep::False => {
                    return Err(PlenoraError::Contract(
                        "dedup_advanced non supporta keep=false".into(),
                    ))
                }
            },
        },
    )
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggFunction {
    Count,
    Sum,
    Avg,
    Mean,
    Min,
    Max,
    First,
    Last,
    Concat,
    Nunique,
    Variance,
    Stddev,
    Quantile,
}

const fn default_agg() -> AggFunction {
    AggFunction::Count
}
fn default_separator() -> String {
    ", ".into()
}
const fn default_ddof() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregation {
    pub column: String,
    #[serde(default = "default_agg")]
    pub function: AggFunction,
    #[serde(default = "default_separator")]
    pub separator: String,
    #[serde(default)]
    pub distinct: bool,
    #[serde(default = "default_true")]
    pub skip_null: bool,
    #[serde(default)]
    pub alias: String,
    pub quantile: Option<f64>,
    #[serde(default = "default_ddof")]
    pub ddof: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    pub group_by: Vec<String>,
    #[serde(default)]
    pub aggregations: Vec<Aggregation>,
}

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
pub(crate) enum KeyColumn {
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
                    write!(scratch, "{}", values.value(row)).expect("fmt su String");
                    push_key_value(key, scratch);
                }
            }
            Self::UInt64 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    scratch.clear();
                    write!(scratch, "{}", values.value(row)).expect("fmt su String");
                    push_key_value(key, scratch);
                }
            }
            Self::Float64 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    scratch.clear();
                    write!(scratch, "{}", values.value(row)).expect("fmt su String");
                    push_key_value(key, scratch);
                }
            }
            Self::Boolean { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    // "true"/"false": stessi byte di bool::to_string.
                    push_key_value(key, if values.value(row) { "true" } else { "false" });
                }
            }
            Self::Utf8 { prefix, values } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                if values.is_null(row) {
                    key.push('0');
                } else {
                    push_key_value(key, values.value(row));
                }
            }
            Self::Generic { prefix, array } => {
                key.push_str(prefix);
                key.push('\u{1e}');
                match scalar_as_string(array.as_ref(), row)? {
                    Some(value) => push_key_value(key, &value),
                    None => key.push('0'),
                }
            }
        }
        key.push('\u{1f}');
        Ok(())
    }
}

/// Frammento `1{len}:{value}` della chiave di `row_key`.
fn push_key_value(key: &mut String, value: &str) {
    key.push('1');
    write!(key, "{}", value.len()).expect("fmt su String");
    key.push(':');
    key.push_str(value);
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
pub(crate) struct KeyHasher(u64);

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
            let value = u64::from_le_bytes(chunk.try_into().expect("blocco di 8 byte"));
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
const PARALLEL_THRESHOLD: usize = 32_768;

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
fn cmp_i64_group_key(a: i64, b: i64) -> Ordering {
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
fn cmp_u64_group_key(a: u64, b: u64) -> Ordering {
    cmp_len_tag(
        u64::from(decimal_digits(a)),
        u64::from(decimal_digits(b)),
    )
    .then_with(|| a.cmp(&b))
}

/// Ordine delle chiavi di `row_key` per una colonna Utf8: tag di lunghezza
/// in byte, poi confronto per byte.
fn cmp_str_group_key(a: &str, b: &str) -> Ordering {
    cmp_len_tag(a.len() as u64, b.len() as u64).then_with(|| a.cmp(b))
}

/// Raggruppamento su chiave nativa di colonna singola (Int64/UInt64/Utf8).
///
/// Nessuna stringa di chiave, hash del valore nativo, ordinamento finale
/// con il comparatore che riproduce l'ordine lessicografico delle chiavi
/// di `row_key`. Il gruppo dei null, se presente, e' sempre in testa
/// (`"...0"` precede `"...1..."` nelle chiavi testuali).
fn build_native_groups<K: Copy + Eq + std::hash::Hash + Send + Sync>(
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
fn build_string_groups(batch: &RecordBatch, group_indices: &[usize]) -> Result<Vec<Vec<usize>>> {
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
enum NumericSource<'a> {
    Float64(&'a Float64Array),
    Int64(&'a Int64Array),
    UInt64(&'a UInt64Array),
    Generic(&'a ArrayRef),
}

impl<'a> NumericSource<'a> {
    fn new(array: &'a ArrayRef) -> Self {
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

    fn value(&self, row: usize) -> Result<Option<f64>> {
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
enum TextSource<'a> {
    Utf8(&'a StringArray),
    Generic(&'a ArrayRef),
}

impl<'a> TextSource<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        Self::Generic(array)
    }

    fn value(&self, row: usize) -> Result<Option<Cow<'a, str>>> {
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
fn map_groups<T>(
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
fn build_partitions(batch: &RecordBatch, group: Option<usize>) -> Result<KeyPartitions<'_>> {
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
fn scatter_partitions(
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

/// Riduzione numerica di un gruppo: logica IDENTICA al percorso generico
/// originale (stesso ordine di somma, `total_cmp` per distinct/quantile,
/// null esclusi o gruppo nullo secondo `skip_null`).
fn reduce_numeric(raw: Vec<Option<f64>>, aggregation: &Aggregation) -> Result<Option<f64>> {
    if !aggregation.skip_null && raw.iter().any(Option::is_none) {
        return Ok(None);
    }
    let mut values = raw.into_iter().flatten().collect::<Vec<_>>();
    if aggregation.distinct {
        values.sort_by(f64::total_cmp);
        values.dedup_by(|left, right| left.total_cmp(right) == Ordering::Equal);
    }
    if values.is_empty() {
        return Ok(None);
    }
    let sum: f64 = values.iter().sum();
    Ok(Some(match aggregation.function {
        AggFunction::Sum => sum,
        AggFunction::Avg | AggFunction::Mean => {
            sum / values.len().to_f64().ok_or_else(|| {
                PlenoraError::Contract("dimensione gruppo non rappresentabile".into())
            })?
        }
        AggFunction::Min => values.iter().copied().reduce(f64::min).unwrap_or_default(),
        AggFunction::Max => values.iter().copied().reduce(f64::max).unwrap_or_default(),
        AggFunction::Variance | AggFunction::Stddev => {
            if values.len() <= aggregation.ddof {
                return Ok(None);
            }
            let length = values.len().to_f64().ok_or_else(|| {
                PlenoraError::Contract("dimensione gruppo non rappresentabile".into())
            })?;
            let mean = sum / length;
            let divisor = (values.len() - aggregation.ddof).to_f64().ok_or_else(|| {
                PlenoraError::Contract("divisore statistico non rappresentabile".into())
            })?;
            let variance = values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / divisor;
            if matches!(aggregation.function, AggFunction::Stddev) {
                variance.sqrt()
            } else {
                variance
            }
        }
        AggFunction::Quantile => {
            let quantile = aggregation.quantile.ok_or_else(|| {
                PlenoraError::Contract("quantile richiede il parametro quantile".into())
            })?;
            values.sort_by(f64::total_cmp);
            let last = (values.len() - 1).to_f64().ok_or_else(|| {
                PlenoraError::Contract("dimensione quantile non rappresentabile".into())
            })?;
            let position = quantile * last;
            let lower = position.floor().to_usize().ok_or_else(|| {
                PlenoraError::Contract("indice quantile non valido".into())
            })?;
            let upper = position.ceil().to_usize().ok_or_else(|| {
                PlenoraError::Contract("indice quantile non valido".into())
            })?;
            let weight = position - position.floor();
            // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
            // violerebbe il determinismo bit-esatto (ADR-0001); la forma non
            // fusa e' il contratto numerico. Produzione e oracolo usano la
            // STESSA forma: l'equivalenza bit-a-bit resta per costruzione.
            #[allow(clippy::suboptimal_flops)]
            let interpolated = (values[upper] - values[lower]) * weight + values[lower];
            interpolated
        }
        _ => unreachable!(),
    }))
}

#[allow(clippy::too_many_lines)] // Aggregation variants share one grouping pass and its invariants.
/// Batch aggregato per `group_by` con le aggregazioni di
/// `config.aggregations` (default: solo conteggio per gruppo).
///
/// # Errors
///
/// - `Contract`: `group_by` vuoto; funzione `quantile` senza il parametro
///   `quantile` o con valore fuori `[0, 1]`; conteggi, dimensioni o indici
///   non rappresentabili
///   (`i64`/`f64`/`usize`); nome di output non valido (come
///   `validate_output_name`);
/// - `Schema`: una colonna di `group_by` o delle aggregazioni assente dallo
///   schema; valore intero non rappresentabile come `f64`; in piu' gli
///   errori di `scalar_as_string`/`scalar_as_f64` (tipi fuori dal fast
///   path), `select_rows` e `replace_or_append`.
pub fn aggregate(batch: &RecordBatch, config: &Aggregate) -> Result<RecordBatch> {
    let group_indices = config
        .group_by
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    if group_indices.is_empty() {
        return Err(PlenoraError::Contract("aggregate richiede group_by".into()));
    }
    // Fail-closed prima dei dati (regola 1): un quantile fuori [0, 1]
    // produrrebbe indici oltre il gruppo ordinato — errore esplicito, mai
    // indexing out-of-bounds a meta' esecuzione.
    for aggregation in &config.aggregations {
        if matches!(aggregation.function, AggFunction::Quantile)
            && aggregation
                .quantile
                .is_some_and(|quantile| !(0.0..=1.0).contains(&quantile))
        {
            return Err(PlenoraError::Contract(
                "quantile fuori dall'intervallo 0..=1".into(),
            ));
        }
    }
    // Raggruppamento: fast path nativo per colonna singola
    // Int64/UInt64/Utf8 (nessuna stringa di chiave), percorso testuale
    // generico altrimenti. Stesso ordine canonico dei gruppi in uscita.
    let groups = if group_indices.len() == 1 {
        let array = batch.column(group_indices[0]);
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            build_native_groups(
                batch.num_rows(),
                |row| {
                    if values.is_null(row) {
                        None
                    } else {
                        Some(values.value(row))
                    }
                },
                |a, b| cmp_i64_group_key(*a, *b),
            )
        } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            build_native_groups(
                batch.num_rows(),
                |row| {
                    if values.is_null(row) {
                        None
                    } else {
                        Some(values.value(row))
                    }
                },
                |a, b| cmp_u64_group_key(*a, *b),
            )
        } else if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            build_native_groups(
                batch.num_rows(),
                |row| {
                    if values.is_null(row) {
                        None
                    } else {
                        Some(values.value(row))
                    }
                },
                |a, b| cmp_str_group_key(a, b),
            )
        } else {
            build_string_groups(batch, &group_indices)?
        }
    } else {
        build_string_groups(batch, &group_indices)?
    };
    // Il calcolo per gruppo va in parallelo solo se la dimensione media dei
    // gruppi ripaga il costo di dispatch dei task (gruppi minuscoli restano
    // sequenziali).
    let parallel = batch.num_rows() >= PARALLEL_THRESHOLD
        && groups.len() > 1
        && batch.num_rows() / groups.len() >= 8;
    let representatives = groups.iter().map(|rows| rows[0]).collect::<Vec<_>>();
    let projected = select_rows(batch, &representatives)?;
    let group_columns = group_indices
        .iter()
        .map(|index| projected.column(*index).clone())
        .collect::<Vec<_>>();
    let group_fields = group_indices
        .iter()
        .map(|index| batch.schema().field(*index).as_ref().clone())
        .collect::<Vec<_>>();
    let mut result = RecordBatch::try_new(
        Arc::new(plenora_core::arrow::schema::Schema::new(group_fields)),
        group_columns,
    )?;
    if config.aggregations.is_empty() {
        let counts = groups
            .iter()
            .map(|rows| i64::try_from(rows.len()).ok())
            .collect::<Vec<_>>();
        return replace_or_append(
            &result,
            "count",
            DataType::Int64,
            false,
            Arc::new(Int64Array::from(counts)),
        );
    }
    let mut duplicate_names = HashMap::new();
    for aggregation in &config.aggregations {
        *duplicate_names
            .entry(&aggregation.column)
            .or_insert(0_usize) += 1;
    }
    for aggregation in &config.aggregations {
        let index = column_index(batch, &aggregation.column)?;
        let function_name = match aggregation.function {
            AggFunction::Count => "count",
            AggFunction::Sum => "sum",
            AggFunction::Avg | AggFunction::Mean => "mean",
            AggFunction::Min => "min",
            AggFunction::Max => "max",
            AggFunction::First => "first",
            AggFunction::Last => "last",
            AggFunction::Concat => "concat",
            AggFunction::Nunique => "nunique",
            AggFunction::Variance => "variance",
            AggFunction::Stddev => "stddev",
            AggFunction::Quantile => "quantile",
        };
        let name = if !aggregation.alias.is_empty() {
            aggregation.alias.clone()
        } else if duplicate_names[&aggregation.column] > 1 {
            format!("{}_{}", aggregation.column, function_name)
        } else {
            aggregation.column.clone()
        };
        validate_output_name(&name)?;
        match aggregation.function {
            AggFunction::Count => {
                let column = batch.column(index);
                let values = map_groups(&groups, parallel, |rows| {
                    let count = rows.iter().filter(|row| !column.is_null(**row)).count();
                    i64::try_from(count)
                        .map(Some)
                        .map_err(|_| PlenoraError::Contract("conteggio gruppo oltre i64".into()))
                })?;
                result = replace_or_append(
                    &result,
                    &name,
                    DataType::Int64,
                    false,
                    Arc::new(Int64Array::from(values)),
                )?;
            }
            AggFunction::Nunique => {
                let source = TextSource::new(batch.column(index));
                let values = map_groups(&groups, parallel, |rows| {
                    let mut seen = HashSet::new();
                    let mut null_seen = false;
                    for row in rows {
                        if let Some(value) = source.value(*row)? {
                            seen.insert(value);
                        } else {
                            null_seen = true;
                        }
                    }
                    // Come il generico: valori distinti piu' una voce per il
                    // null solo quando `skip_null` e' falso.
                    let count = seen.len() + usize::from(null_seen && !aggregation.skip_null);
                    i64::try_from(count)
                        .map(Some)
                        .map_err(|_| PlenoraError::Contract("conteggio gruppo oltre i64".into()))
                })?;
                result = replace_or_append(
                    &result,
                    &name,
                    DataType::Int64,
                    false,
                    Arc::new(Int64Array::from(values)),
                )?;
            }
            AggFunction::First | AggFunction::Last => {
                let column = batch.column(index);
                let first = matches!(aggregation.function, AggFunction::First);
                let values = map_groups(&groups, parallel, |rows| {
                    let row = if first {
                        rows[0]
                    } else {
                        *rows.last().unwrap_or(&rows[0])
                    };
                    scalar_as_string(column.as_ref(), row)
                })?;
                result = replace_or_append(
                    &result,
                    &name,
                    DataType::Utf8,
                    true,
                    Arc::new(StringArray::from(values)),
                )?;
            }
            AggFunction::Concat => {
                let source = TextSource::new(batch.column(index));
                let values = map_groups(&groups, parallel, |rows| {
                    let mut seen = HashSet::new();
                    let mut values = Vec::new();
                    for row in rows {
                        if let Some(value) = source.value(*row)? {
                            if !aggregation.distinct || seen.insert(value.clone()) {
                                values.push(value);
                            }
                        } else if !aggregation.skip_null {
                            values.push(Cow::Borrowed(""));
                        }
                    }
                    Ok(Some(values.join(&aggregation.separator)))
                })?;
                result = replace_or_append(
                    &result,
                    &name,
                    DataType::Utf8,
                    true,
                    Arc::new(StringArray::from(values)),
                )?;
            }
            _ => {
                let source = NumericSource::new(batch.column(index));
                let values = map_groups(&groups, parallel, |rows| {
                    let raw = rows
                        .iter()
                        .map(|row| source.value(*row))
                        .collect::<Result<Vec<_>>>()?;
                    reduce_numeric(raw, aggregation)
                })?;
                result = replace_or_append(
                    &result,
                    &name,
                    DataType::Float64,
                    true,
                    Arc::new(Float64Array::from(values)),
                )?;
            }
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollingKind {
    Sum,
    Mean,
    Min,
    Max,
    Stddev,
}

const fn default_min_periods() -> usize {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollingWindow {
    pub column: String,
    pub function: RollingKind,
    #[serde(default)]
    pub group_by: Option<String>,
    #[serde(default)]
    pub order_column: Option<String>,
    pub window: usize,
    #[serde(default = "default_min_periods")]
    pub min_periods: usize,
    #[serde(default = "default_ddof")]
    pub ddof: usize,
    pub output_column: String,
}

/// Finestra mobile (`window` righe) su `column`, opzionalmente partizionata
/// per `group_by` e ordinata per `order_column`.
///
/// # Errors
///
/// - `Contract`: `window` o `min_periods` nulli, `min_periods > window`,
///   oppure dimensioni/divisori non rappresentabili come `f64`;
/// - `Schema`: colonna `column`, `group_by` o `order_column` assente dallo
///   schema; valore intero non rappresentabile come `f64`; in piu' gli
///   errori di `sort`, `scalar_as_string`/`scalar_as_f64` e
///   `replace_or_append`.
pub fn rolling_window(batch: &RecordBatch, config: &RollingWindow) -> Result<RecordBatch> {
    if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
        return Err(PlenoraError::Contract(
            "rolling_window: finestra non valida".into(),
        ));
    }
    let ordered = if let Some(column) = &config.order_column {
        sort(
            batch,
            &Sort {
                columns: vec![column.clone()],
                ascending: true,
            },
        )?
    } else {
        batch.clone()
    };
    let source = column_index(&ordered, &config.column)?;
    let group = config
        .group_by
        .as_deref()
        .map(|name| column_index(&ordered, name))
        .transpose()?;
    // Partizionamento condiviso con `window_function`: chiavi testuali
    // prese in prestito (`TextSource`, nessuna String per riga), hash
    // FxHash+splitmix64, iterazione delle partizioni nello stesso ordine
    // del BTreeMap originale (chiave `Option<String>` crescente).
    let partitions = build_partitions(&ordered, group)?;
    let numbers = NumericSource::new(ordered.column(source));
    let compute = |rows: &[usize]| -> Result<Vec<Option<f64>>> {
        let numbers = rows
            .iter()
            .map(|row| numbers.value(*row))
            .collect::<Result<Vec<_>>>()?;
        let track_extrema = matches!(config.function, RollingKind::Min | RollingKind::Max);
        let mut values = Vec::with_capacity(rows.len());
        for position in 0..rows.len() {
            let start = (position + 1).saturating_sub(config.window);
            let window = &numbers[start..=position];
            // Aggregazione della finestra senza allocazioni: una passata per
            // conteggio/somma/estremi (due per stddev), replicando ESATTAMENTE
            // le riduzioni originali sul `Vec` ricostruito a ogni riga:
            // - `Iterator::sum::<f64>` parte da -0.0 (sum([-0.0]) = -0.0);
            // - `reduce(f64::min/max)` parte dal primo elemento (finestra di
            //   solo NaN -> NaN, non +/-inf).
            let mut count = 0_usize;
            let mut sum = -0.0_f64;
            let mut minimum: Option<f64> = None;
            let mut maximum: Option<f64> = None;
            for value in window.iter().flatten() {
                count += 1;
                sum += value;
                if track_extrema {
                    minimum = Some(minimum.map_or(*value, |min| f64::min(min, *value)));
                    maximum = Some(maximum.map_or(*value, |max| f64::max(max, *value)));
                }
            }
            if count < config.min_periods {
                values.push(None);
                continue;
            }
            values.push(match config.function {
                RollingKind::Sum => Some(sum),
                RollingKind::Mean => count.to_f64().map(|length| sum / length),
                RollingKind::Min => minimum,
                RollingKind::Max => maximum,
                RollingKind::Stddev if count <= config.ddof => None,
                RollingKind::Stddev => {
                    let length = count.to_f64().ok_or_else(|| {
                        PlenoraError::Contract("dimensione rolling non rappresentabile".into())
                    })?;
                    let mean = sum / length;
                    let divisor = (count - config.ddof).to_f64().ok_or_else(|| {
                        PlenoraError::Contract("divisore rolling non rappresentabile".into())
                    })?;
                    Some(
                        (window
                            .iter()
                            .flatten()
                            .map(|value| (value - mean).powi(2))
                            .sum::<f64>()
                            / divisor)
                            .sqrt(),
                    )
                }
            });
        }
        Ok(values)
    };
    let mut output = vec![None; ordered.num_rows()];
    scatter_partitions(&ordered, &partitions, &mut output, compute)?;
    replace_or_append(
        &ordered,
        &config.output_column,
        DataType::Float64,
        true,
        Arc::new(Float64Array::from(output)),
    )
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Rank,
    DenseRank,
    Cumsum,
    Cumcount,
    Lag,
    Lead,
    PctChange,
    RunningMean,
    PercentRank,
    CumeDist,
    Ntile,
}
const fn default_window() -> WindowKind {
    WindowKind::Rank
}
const fn default_offset() -> usize {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowFunction {
    pub column: String,
    #[serde(default = "default_window")]
    pub function: WindowKind,
    pub group_by: Option<String>,
    pub order_column: Option<String>,
    #[serde(default = "default_offset")]
    pub offset: usize,
    #[serde(default)]
    pub buckets: Option<usize>,
    pub output_column: Option<String>,
}

#[allow(clippy::too_many_lines)] // All window variants share partition/order state and one output pass.
/// Funzione finestra (`rank`, `lag`, `ntile`, ...) su `column`,
/// opzionalmente partizionata per `group_by` e ordinata per
/// `order_column`.
///
/// # Errors
///
/// - `Contract`: `offset` nullo; `ntile` senza `buckets` maggiore di zero;
///   `buckets` specificato per una funzione diversa da `ntile`;
/// - `Schema`: colonna `column`, `group_by` o `order_column` assente dallo
///   schema; valore intero non rappresentabile come `f64`; in piu' gli
///   errori di `sort`, `scalar_as_string`/`scalar_as_f64` e
///   `replace_or_append`.
pub fn window_function(batch: &RecordBatch, config: &WindowFunction) -> Result<RecordBatch> {
    if config.offset == 0 {
        return Err(PlenoraError::Contract("offset deve essere positivo".into()));
    }
    if matches!(config.function, WindowKind::Ntile) {
        if config.buckets.is_none_or(|buckets| buckets == 0) {
            return Err(PlenoraError::Contract(
                "ntile richiede buckets maggiore di zero".into(),
            ));
        }
    } else if config.buckets.is_some() {
        return Err(PlenoraError::Contract(
            "buckets e' ammesso solo per ntile".into(),
        ));
    }
    let source_index = column_index(batch, &config.column)?;
    let ordered = if let Some(column) = &config.order_column {
        sort(
            batch,
            &Sort {
                columns: vec![column.clone()],
                ascending: true,
            },
        )?
    } else {
        batch.clone()
    };
    let group_index = config
        .group_by
        .as_deref()
        .map(|name| column_index(&ordered, name))
        .transpose()?;
    // Partizionamento condiviso con `rolling_window`: chiavi testuali prese
    // in prestito (`TextSource`, nessuna String per riga), hash
    // FxHash+splitmix64, iterazione delle partizioni nello stesso ordine
    // del BTreeMap originale (chiave `Option<String>` crescente).
    let partitions = build_partitions(&ordered, group_index)?;
    let source = NumericSource::new(ordered.column(source_index));
    let compute = |rows: &[usize]| -> Result<Vec<Option<f64>>> {
        let numbers = rows
            .iter()
            .map(|row| source.value(*row))
            .collect::<Result<Vec<_>>>()?;
        // `sorted`/`dense` servono solo alle funzioni di rango: costruiti
        // solo per quelle (l'originale li calcolava comunque, senza
        // effetti osservabili).
        let needs_rank = matches!(
            config.function,
            WindowKind::Rank | WindowKind::DenseRank | WindowKind::PercentRank | WindowKind::CumeDist
        );
        let mut sorted = Vec::new();
        let mut dense = Vec::new();
        if needs_rank {
            sorted = numbers.iter().flatten().copied().collect::<Vec<_>>();
            sorted.sort_by(f64::total_cmp);
            dense.clone_from(&sorted);
            dense.dedup_by(|left, right| left.total_cmp(right) == Ordering::Equal);
        }
        let mut sum = 0.0;
        let mut count = 0.0_f64;
        let mut values = Vec::with_capacity(rows.len());
        for position in 0..rows.len() {
            values.push(match config.function {
                WindowKind::Cumcount => position.to_f64(),
                WindowKind::Cumsum => numbers[position].map(|value| {
                    sum += value;
                    sum
                }),
                WindowKind::RunningMean => numbers[position].map(|value| {
                    sum += value;
                    count += 1.0;
                    sum / count
                }),
                WindowKind::Lag => position
                    .checked_sub(config.offset)
                    .and_then(|other| numbers[other]),
                WindowKind::Lead => numbers.get(position + config.offset).copied().flatten(),
                WindowKind::PctChange => position
                    .checked_sub(1)
                    .and_then(|previous| numbers[previous])
                    .and_then(|previous| {
                        numbers[position]
                            .filter(|_| previous != 0.0)
                            .map(|current| (current - previous) / previous)
                    }),
                WindowKind::Rank | WindowKind::DenseRank => numbers[position].and_then(|current| {
                    if matches!(config.function, WindowKind::DenseRank) {
                        dense
                            .binary_search_by(|value| value.total_cmp(&current))
                            .ok()
                            .and_then(|index| (index + 1).to_f64())
                    } else {
                        let first =
                            sorted.partition_point(|value| value.total_cmp(&current).is_lt());
                        let last = sorted
                            .partition_point(|value| !value.total_cmp(&current).is_gt())
                            .checked_sub(1)?;
                        (first + last + 2).to_f64().map(|sum| sum / 2.0)
                    }
                }),
                WindowKind::PercentRank => numbers[position].and_then(|current| {
                    if sorted.len() <= 1 {
                        return Some(0.0);
                    }
                    let rank = sorted.partition_point(|value| value.total_cmp(&current).is_lt());
                    let numerator = rank.to_f64()?;
                    let denominator = (sorted.len() - 1).to_f64()?;
                    Some(numerator / denominator)
                }),
                WindowKind::CumeDist => numbers[position].and_then(|current| {
                    let last = sorted
                        .partition_point(|value| !value.total_cmp(&current).is_gt())
                        .checked_sub(1)?;
                    let numerator = (last + 1).to_f64()?;
                    let denominator = sorted.len().to_f64()?;
                    Some(numerator / denominator)
                }),
                WindowKind::Ntile => {
                    let buckets = config.buckets.unwrap_or(1);
                    let effective = buckets.min(rows.len());
                    position
                        .checked_mul(effective)
                        .and_then(|value| value.checked_div(rows.len()))
                        .and_then(|value| (value + 1).to_f64())
                }
            });
        }
        Ok(values)
    };
    let mut output = vec![None; ordered.num_rows()];
    scatter_partitions(&ordered, &partitions, &mut output, compute)?;
    let suffix = match config.function {
        WindowKind::Rank => "rank",
        WindowKind::DenseRank => "dense_rank",
        WindowKind::Cumsum => "cumsum",
        WindowKind::Cumcount => "cumcount",
        WindowKind::Lag => "lag",
        WindowKind::Lead => "lead",
        WindowKind::PctChange => "pct_change",
        WindowKind::RunningMean => "running_mean",
        WindowKind::PercentRank => "percent_rank",
        WindowKind::CumeDist => "cume_dist",
        WindowKind::Ntile => "ntile",
    };
    let name = config
        .output_column
        .clone()
        .unwrap_or_else(|| format!("{}_{}", config.column, suffix));
    replace_or_append(
        &ordered,
        &name,
        DataType::Float64,
        true,
        Arc::new(Float64Array::from(output)),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use plenora_core::arrow::array::{types::Int32Type, DictionaryArray, LargeStringArray};
    use plenora_core::arrow::schema::{Field, Schema};

    use super::*;

    /// Oracolo indipendente: permutazione attesa per una colonna Float64
    /// (null dopo i valori, `total_cmp`, stabile sull'indice originale).
    fn expected_permutation(values: &[Option<f64>], ascending: bool) -> Vec<usize> {
        let mut rows: Vec<usize> = (0..values.len()).collect();
        rows.sort_by(|left, right| {
            let ordering = match (values[*left], values[*right]) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.total_cmp(&b),
            };
            let ordering = if ascending {
                ordering
            } else {
                ordering.reverse()
            };
            ordering.then(left.cmp(right))
        });
        rows
    }

    fn sorted_ids(batch: &RecordBatch, config: &Sort) -> Vec<i64> {
        let sorted = sort(batch, config).expect("sort");
        sorted
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("id column")
            .values()
            .to_vec()
    }

    #[test]
    fn sort_is_stable_with_duplicate_keys_at_parallel_scale() {
        // Sopra la soglia parallela (32_768): chiavi molto duplicate (97
        // valori distinti) verificano stabilita' e determinismo del merge
        // sort rayon contro l'oracolo sequenziale.
        let rows = 50_000_usize;
        let ids = (0..rows)
            .map(|row| i64::try_from(row).expect("id"))
            .collect::<Vec<_>>();
        let values = (0..rows)
            .map(|row| {
                if row % 11 == 0 {
                    None
                } else {
                    Some(f64::from(u32::try_from(row % 97).expect("key")))
                }
            })
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(Float64Array::from(values.clone())),
            ],
        )
        .expect("fixture");
        let config = Sort {
            columns: vec!["num".into()],
            ascending: true,
        };
        let sorted = sorted_ids(&batch, &config);
        let expected = expected_permutation(&values, true)
            .into_iter()
            .map(|row| i64::try_from(row).expect("id"))
            .collect::<Vec<_>>();
        assert_eq!(sorted, expected);
        // Determinismo assoluto: stesso input, stesso output.
        assert_eq!(sorted_ids(&batch, &config), expected);
    }

    #[test]
    fn sort_nan_signed_zero_and_null_ordering_is_exact() {
        let values = vec![
            Some(0.0),
            Some(f64::NAN),
            Some(-0.0),
            None,
            Some(1.0),
        ];
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3, 4])),
                Arc::new(Float64Array::from(values.clone())),
            ],
        )
        .expect("fixture");
        // total_cmp: -0.0 < 0.0 < 1.0 < NaN; null in coda.
        let ascending = sorted_ids(
            &batch,
            &Sort {
                columns: vec!["num".into()],
                ascending: true,
            },
        );
        assert_eq!(ascending, vec![2, 0, 4, 1, 3]);
        // Discendente rovescia tutto: null in testa, poi NaN.
        let descending = sorted_ids(
            &batch,
            &Sort {
                columns: vec!["num".into()],
                ascending: false,
            },
        );
        assert_eq!(
            descending,
            expected_permutation(&values, false)
                .into_iter()
                .map(|row| i64::try_from(row).expect("id"))
                .collect::<Vec<_>>()
        );
        assert_eq!(descending, vec![3, 1, 4, 0, 2]);
    }

    #[test]
    fn sort_handles_dictionary_and_rejects_large_utf8_like_before() {
        let keys = DictionaryArray::<Int32Type>::new(
            plenora_core::arrow::array::Int32Array::from(vec![Some(1), Some(0), None, Some(1)]),
            Arc::new(StringArray::from(vec!["a", "b"])),
        );
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new(
                    "c",
                    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                    true,
                ),
            ])),
            vec![Arc::new(Int64Array::from(vec![0, 1, 2, 3])), Arc::new(keys)],
        )
        .expect("fixture");
        let sorted = sorted_ids(
            &batch,
            &Sort {
                columns: vec!["c".into()],
                ascending: true,
            },
        );
        assert_eq!(sorted, vec![1, 0, 3, 2]); // "a" < "b", null in coda

        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("c", DataType::LargeUtf8, true)])),
            vec![Arc::new(LargeStringArray::from(vec![Some("b"), Some("a"), None]))],
        )
        .expect("fixture");
        // LargeUtf8 non e' nel profilo scalare: confrontando due valori non
        // nulli il percorso generico fallisce, errore di schema invariato.
        assert!(sort(
            &large,
            &Sort {
                columns: vec!["c".into()],
                ascending: true,
            }
        )
        .is_err());
    }

    #[test]
    fn sort_empty_single_row_and_mixed_columns() {
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("num", DataType::Float64, false)])),
            vec![Arc::new(Float64Array::from(Vec::<f64>::new()))],
        )
        .expect("empty fixture");
        assert_eq!(
            sort(
                &empty,
                &Sort {
                    columns: vec!["num".into()],
                    ascending: true,
                }
            )
            .expect("empty sort")
            .num_rows(),
            0
        );
        assert!(sort(
            &empty,
            &Sort {
                columns: vec![],
                ascending: true,
            }
        )
        .is_err());

        let mixed = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("group", DataType::Utf8, false),
                Field::new("num", DataType::Float64, false),
                Field::new("flag", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3])),
                Arc::new(StringArray::from(vec!["b", "a", "b", "a"])),
                Arc::new(Float64Array::from(vec![2.0, 2.0, 1.0, 1.0])),
                Arc::new(plenora_core::arrow::array::BooleanArray::from(vec![
                    true, false, false, true,
                ])),
            ],
        )
        .expect("mixed fixture");
        let sorted = sorted_ids(
            &mixed,
            &Sort {
                columns: vec!["group".into(), "num".into()],
                ascending: true,
            },
        );
        assert_eq!(sorted, vec![3, 1, 2, 0]); // (a,1),(a,2),(b,1),(b,2)
        // Booleano: "false" < "true" come il confronto testuale.
        let by_flag = sorted_ids(
            &mixed,
            &Sort {
                columns: vec!["flag".into()],
                ascending: true,
            },
        );
        assert_eq!(by_flag, vec![1, 2, 0, 3]);
    }

    // -------------------------------------------------------------------
    // Test-oracolo di `top_n` (estensione v1.1): l'oracolo e' la coppia
    // `sort` + slice delle prime n posizioni, eseguita sul kernel `sort`
    // gia' validato. Il confronto e' sull'intero RecordBatch (schema,
    // valori, null mask), non solo sugli id.
    // -------------------------------------------------------------------

    fn oracle_sort_then_limit(batch: &RecordBatch, config: &TopN) -> RecordBatch {
        let sorted = sort(
            batch,
            &Sort {
                columns: config.columns.clone(),
                ascending: !config.descending,
            },
        )
        .expect("oracle sort");
        let n = usize::try_from(config.n)
            .expect("n")
            .min(sorted.num_rows());
        sorted.slice(0, n)
    }

    fn assert_top_n_matches_oracle(batch: &RecordBatch, config: &TopN) {
        let fast = top_n(batch, config).expect("top_n");
        let oracle = oracle_sort_then_limit(batch, config);
        assert_eq!(fast, oracle, "config: {config:?}");
    }

    fn numeric_batch(values: &[Option<f64>]) -> RecordBatch {
        let ids = (0..values.len())
            .map(|row| i64::try_from(row).expect("id"))
            .collect::<Vec<_>>();
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(Float64Array::from(values.to_vec())),
            ],
        )
        .expect("fixture")
    }

    #[test]
    fn top_n_matches_sort_plus_limit_on_duplicates_nan_signed_zero_and_nulls() {
        // Duplicati, NaN, -0.0, null: ogni n e ogni direzione contro l'oracolo.
        let values = vec![
            Some(0.0),
            Some(f64::NAN),
            Some(-0.0),
            None,
            Some(1.0),
            Some(1.0),
            Some(f64::NAN),
            None,
            Some(-0.0),
            Some(0.0),
        ];
        let batch = numeric_batch(&values);
        for n in [0_u64, 1, 3, 5, 9, 10, 11, 1_000] {
            for descending in [false, true] {
                assert_top_n_matches_oracle(
                    &batch,
                    &TopN {
                        columns: vec!["num".into()],
                        n,
                        descending,
                    },
                );
            }
        }
    }

    #[test]
    fn top_n_matches_oracle_on_duplicate_heavy_multi_column_input() {
        // Chiavi molto duplicate su due colonne (testo + numerico): la
        // stabilita' sullo spareggio d'indice deve riprodurre il sort.
        let rows = 2_000_usize;
        let ids = (0..rows)
            .map(|row| i64::try_from(row).expect("id"))
            .collect::<Vec<_>>();
        let groups = (0..rows)
            .map(|row| ["a", "b", "c"][row % 3].to_owned())
            .collect::<Vec<_>>();
        let nums = (0..rows)
            .map(|row| match row % 7 {
                0 => None,
                5 => Some(f64::NAN),
                _ => Some(f64::from(u32::try_from(row % 11).expect("key")) - 5.0),
            })
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("group", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(groups)),
                Arc::new(Float64Array::from(nums)),
            ],
        )
        .expect("fixture");
        for n in [1_u64, 7, 100, 2_000, 5_000] {
            for descending in [false, true] {
                assert_top_n_matches_oracle(
                    &batch,
                    &TopN {
                        columns: vec!["group".into(), "num".into()],
                        n,
                        descending,
                    },
                );
            }
        }
    }

    #[test]
    fn top_n_edge_cases_and_config_validation() {
        let batch = numeric_batch(&[Some(2.0), Some(1.0), None]);
        // n = 0: batch vuoto, schema invariato.
        let empty = top_n(
            &batch,
            &TopN {
                columns: vec!["num".into()],
                n: 0,
                descending: false,
            },
        )
        .expect("n=0");
        assert_eq!(empty.num_rows(), 0);
        assert_eq!(empty.schema(), batch.schema());
        // Errori: colonne vuote, colonna mancante, config non strict.
        assert!(top_n(
            &batch,
            &TopN {
                columns: vec![],
                n: 1,
                descending: false,
            },
        )
        .is_err());
        assert!(top_n(
            &batch,
            &TopN {
                columns: vec!["missing".into()],
                n: 1,
                descending: false,
            },
        )
        .is_err());
        let decoded: TopN = serde_json::from_value(serde_json::json!({"columns": ["num"], "n": 2}))
            .expect("default descending");
        assert!(!decoded.descending);
        assert!(serde_json::from_value::<TopN>(
            serde_json::json!({"columns": ["num"], "n": 2, "ascending": true})
        )
        .is_err());
        // Tipo non confrontabile (LargeUtf8): stesso errore di `sort`.
        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("c", DataType::LargeUtf8, true)])),
            vec![Arc::new(LargeStringArray::from(vec![Some("b"), Some("a")]))],
        )
        .expect("fixture");
        assert!(top_n(
            &large,
            &TopN {
                columns: vec!["c".into()],
                n: 1,
                descending: false,
            },
        )
        .is_err());
    }

    // -------------------------------------------------------------------
    // Test-oracolo di `aggregate` (fast path, secondo batch ottimizzazioni):
    // l'output deve essere byte-identico al percorso generico originale,
    // copiato verbatim qui sotto come riferimento indipendente.
    // -------------------------------------------------------------------

    /// Copia verbatim dell'implementazione generica pre-ottimizzazione.
    #[allow(clippy::too_many_lines)]
    fn aggregate_reference(batch: &RecordBatch, config: &Aggregate) -> Result<RecordBatch> {
        let group_indices = config
            .group_by
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        if group_indices.is_empty() {
            return Err(PlenoraError::Contract("aggregate richiede group_by".into()));
        }
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for row in 0..batch.num_rows() {
            groups
                .entry(row_key(batch, &group_indices, row)?)
                .or_default()
                .push(row);
        }
        let representatives = groups.values().map(|rows| rows[0]).collect::<Vec<_>>();
        let projected = select_rows(batch, &representatives)?;
        let group_columns = group_indices
            .iter()
            .map(|index| projected.column(*index).clone())
            .collect::<Vec<_>>();
        let group_fields = group_indices
            .iter()
            .map(|index| batch.schema().field(*index).as_ref().clone())
            .collect::<Vec<_>>();
        let mut result = RecordBatch::try_new(
            Arc::new(plenora_core::arrow::schema::Schema::new(group_fields)),
            group_columns,
        )?;
        if config.aggregations.is_empty() {
            let counts = groups
                .values()
                .map(|rows| i64::try_from(rows.len()).ok())
                .collect::<Vec<_>>();
            return replace_or_append(
                &result,
                "count",
                DataType::Int64,
                false,
                Arc::new(Int64Array::from(counts)),
            );
        }
        let mut duplicate_names = HashMap::new();
        for aggregation in &config.aggregations {
            *duplicate_names
                .entry(&aggregation.column)
                .or_insert(0_usize) += 1;
        }
        for aggregation in &config.aggregations {
            let index = column_index(batch, &aggregation.column)?;
            let function_name = match aggregation.function {
                AggFunction::Count => "count",
                AggFunction::Sum => "sum",
                AggFunction::Avg | AggFunction::Mean => "mean",
                AggFunction::Min => "min",
                AggFunction::Max => "max",
                AggFunction::First => "first",
                AggFunction::Last => "last",
                AggFunction::Concat => "concat",
                AggFunction::Nunique => "nunique",
                AggFunction::Variance => "variance",
                AggFunction::Stddev => "stddev",
                AggFunction::Quantile => "quantile",
            };
            let name = if !aggregation.alias.is_empty() {
                aggregation.alias.clone()
            } else if duplicate_names[&aggregation.column] > 1 {
                format!("{}_{}", aggregation.column, function_name)
            } else {
                aggregation.column.clone()
            };
            validate_output_name(&name)?;
            match aggregation.function {
                AggFunction::Count | AggFunction::Nunique => {
                    let values = groups
                        .values()
                        .map(|rows| {
                            let count = if matches!(aggregation.function, AggFunction::Count) {
                                rows.iter()
                                    .filter(|row| !batch.column(index).is_null(**row))
                                    .count()
                            } else {
                                let mut seen = HashSet::new();
                                for row in rows {
                                    if let Some(value) =
                                        scalar_as_string(batch.column(index).as_ref(), *row)?
                                    {
                                        seen.insert(format!("1{}:{value}", value.len()));
                                    } else if !aggregation.skip_null {
                                        seen.insert("0".to_owned());
                                    }
                                }
                                seen.len()
                            };
                            i64::try_from(count).map(Some).map_err(|_| {
                                PlenoraError::Contract("conteggio gruppo oltre i64".into())
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    result = replace_or_append(
                        &result,
                        &name,
                        DataType::Int64,
                        false,
                        Arc::new(Int64Array::from(values)),
                    )?;
                }
                AggFunction::Concat | AggFunction::First | AggFunction::Last => {
                    let values = groups
                        .values()
                        .map(|rows| {
                            if matches!(aggregation.function, AggFunction::First | AggFunction::Last)
                            {
                                let row = if matches!(aggregation.function, AggFunction::First) {
                                    rows[0]
                                } else {
                                    *rows.last().unwrap_or(&rows[0])
                                };
                                return scalar_as_string(batch.column(index).as_ref(), row);
                            }
                            let mut seen = HashSet::new();
                            let mut values = Vec::new();
                            for row in rows {
                                if let Some(value) =
                                    scalar_as_string(batch.column(index).as_ref(), *row)?
                                {
                                    if !aggregation.distinct || seen.insert(value.clone()) {
                                        values.push(value);
                                    }
                                } else if !aggregation.skip_null {
                                    values.push(String::new());
                                }
                            }
                            Ok(Some(values.join(&aggregation.separator)))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    result = replace_or_append(
                        &result,
                        &name,
                        DataType::Utf8,
                        true,
                        Arc::new(StringArray::from(values)),
                    )?;
                }
                _ => {
                    let values = groups
                        .values()
                        .map(|rows| {
                            let raw = rows
                                .iter()
                                .map(|row| scalar_as_f64(batch.column(index).as_ref(), *row))
                                .collect::<Result<Vec<_>>>()?;
                            if !aggregation.skip_null && raw.iter().any(Option::is_none) {
                                return Ok(None);
                            }
                            let mut values = raw.into_iter().flatten().collect::<Vec<_>>();
                            if aggregation.distinct {
                                values.sort_by(f64::total_cmp);
                                values
                                    .dedup_by(|left, right| left.total_cmp(right) == Ordering::Equal);
                            }
                            if values.is_empty() {
                                return Ok(None);
                            }
                            let sum: f64 = values.iter().sum();
                            Ok(Some(match aggregation.function {
                                AggFunction::Sum => sum,
                                AggFunction::Avg | AggFunction::Mean => {
                                    sum / values.len().to_f64().ok_or_else(|| {
                                        PlenoraError::Contract(
                                            "dimensione gruppo non rappresentabile".into(),
                                        )
                                    })?
                                }
                                AggFunction::Min => {
                                    values.iter().copied().reduce(f64::min).unwrap_or_default()
                                }
                                AggFunction::Max => {
                                    values.iter().copied().reduce(f64::max).unwrap_or_default()
                                }
                                AggFunction::Variance | AggFunction::Stddev => {
                                    if values.len() <= aggregation.ddof {
                                        return Ok(None);
                                    }
                                    let length = values.len().to_f64().ok_or_else(|| {
                                        PlenoraError::Contract(
                                            "dimensione gruppo non rappresentabile".into(),
                                        )
                                    })?;
                                    let mean = sum / length;
                                    let divisor = (values.len() - aggregation.ddof)
                                        .to_f64()
                                        .ok_or_else(|| {
                                            PlenoraError::Contract(
                                                "divisore statistico non rappresentabile".into(),
                                            )
                                        })?;
                                    let variance = values
                                        .iter()
                                        .map(|value| (value - mean).powi(2))
                                        .sum::<f64>()
                                        / divisor;
                                    if matches!(aggregation.function, AggFunction::Stddev) {
                                        variance.sqrt()
                                    } else {
                                        variance
                                    }
                                }
                                AggFunction::Quantile => {
                                    let quantile = aggregation.quantile.ok_or_else(|| {
                                        PlenoraError::Contract(
                                            "quantile richiede il parametro quantile".into(),
                                        )
                                    })?;
                                    values.sort_by(f64::total_cmp);
                                    let last = (values.len() - 1).to_f64().ok_or_else(|| {
                                        PlenoraError::Contract(
                                            "dimensione quantile non rappresentabile".into(),
                                        )
                                    })?;
                                    let position = quantile * last;
                                    let lower = position.floor().to_usize().ok_or_else(|| {
                                        PlenoraError::Contract("indice quantile non valido".into())
                                    })?;
                                    let upper = position.ceil().to_usize().ok_or_else(|| {
                                        PlenoraError::Contract("indice quantile non valido".into())
                                    })?;
                                    let weight = position - position.floor();
                                    // Niente mul_add/FMA: forma non fusa
                                    // (contratto numerico, ADR-0001) — la
                                    // STESSA della produzione, equivalenza
                                    // bit-a-bit per costruzione.
                                    #[allow(clippy::suboptimal_flops)]
                                    let interpolated = (values[upper] - values[lower]) * weight
                                        + values[lower];
                                    interpolated
                                }
                                _ => unreachable!(),
                            }))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    result = replace_or_append(
                        &result,
                        &name,
                        DataType::Float64,
                        true,
                        Arc::new(Float64Array::from(values)),
                    )?;
                }
            }
        }
        Ok(result)
    }

    /// Confronto rigoroso: schema (nomi, tipi, nullabilita'), numero righe,
    /// maschera null e valori (bit a bit per i Float64, NaN incluso).
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
                "tipo colonna {}",
                fast_field.name()
            );
            assert_eq!(
                fast_field.is_nullable(),
                reference_field.is_nullable(),
                "nullabilita' colonna {}",
                fast_field.name()
            );
            let fast_column = fast.column(index);
            let reference_column = reference.column(index);
            for row in 0..fast.num_rows() {
                assert_eq!(
                    fast_column.is_null(row),
                    reference_column.is_null(row),
                    "null riga {row} colonna {}",
                    fast_field.name()
                );
            }
            match fast_field.data_type() {
                DataType::Float64 => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("float64");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("float64");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row).to_bits(),
                                reference_values.value(row).to_bits(),
                                "bits riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                DataType::Int64 => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .expect("int64");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .expect("int64");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row),
                                reference_values.value(row),
                                "valore riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                _ => {
                    // Chiavi di gruppo di altri tipi (Utf8, UInt64, Boolean,
                    // Date32, ...): confronto sulla forma scalare, che per
                    // questi tipi e' iniettiva sui valori.
                    for row in 0..fast.num_rows() {
                        assert_eq!(
                            scalar_as_string(fast_column.as_ref(), row).expect("scalare"),
                            scalar_as_string(reference_column.as_ref(), row).expect("scalare"),
                            "valore riga {row} colonna {}",
                            fast_field.name()
                        );
                    }
                }
            }
        }
    }

    /// Esegue fast path e riferimento e verifica l'uguaglianza esatta;
    /// il fast path viene eseguito due volte (determinismo assoluto).
    fn assert_aggregate_parity(batch: &RecordBatch, config: &Aggregate) {
        let reference = aggregate_reference(batch, config).expect("riferimento");
        let fast = aggregate(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = aggregate(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    fn agg(column: &str, function: AggFunction) -> Aggregation {
        Aggregation {
            column: column.into(),
            function,
            separator: ", ".into(),
            distinct: false,
            skip_null: true,
            alias: String::new(),
            quantile: None,
            ddof: 1,
        }
    }

    #[test]
    fn quantile_fuori_range_rifiutato_prima_dei_dati() {
        // Regressione (analisi 2026-07-28): quantile > 1.0 produceva un
        // indice oltre il gruppo ordinato — panic out-of-bounds nel
        // percorso lib, invisibile al gate R6 (indicizzazione, non
        // primitiva esplicita) e mascherato dall'oracolo a specchio. Ora
        // il range e' validato fail-closed prima di toccare i dati.
        let batch = numeric_batch(&[Some(1.0), Some(2.0), Some(3.0)]);
        for quantile in [-0.5, 1.5, f64::NAN, f64::INFINITY] {
            let config = Aggregate {
                group_by: vec!["id".into()],
                aggregations: vec![Aggregation {
                    quantile: Some(quantile),
                    ..agg("num", AggFunction::Quantile)
                }],
            };
            let result = aggregate(&batch, &config);
            let Err(PlenoraError::Contract(message)) = &result else {
                panic!("quantile {quantile}: atteso rifiuto fuori range, ottenuto {result:?}");
            };
            assert!(
                message.contains("0..=1"),
                "quantile {quantile}: messaggio inatteso: {message}"
            );
        }
        // I bordi 0.0 e 1.0 restano validi.
        let config = Aggregate {
            group_by: vec!["id".into()],
            aggregations: [0.0, 1.0]
                .iter()
                .map(|quantile| Aggregation {
                    quantile: Some(*quantile),
                    ..agg("num", AggFunction::Quantile)
                })
                .collect(),
        };
        aggregate(&batch, &config).expect("bordi 0 e 1 validi");
    }

    /// Batteria completa di aggregazioni su `num`/`val`/`txt`, con duplicati
    /// di colonna (nomi `{colonna}_{funzione}`) e varianti `skip_null`/`distinct`.
    fn full_aggregations() -> Vec<Aggregation> {
        vec![
            agg("num", AggFunction::Sum),
            agg("num", AggFunction::Mean),
            agg("num", AggFunction::Min),
            agg("num", AggFunction::Max),
            agg("num", AggFunction::Variance),
            agg("num", AggFunction::Stddev),
            Aggregation {
                quantile: Some(0.3),
                ..agg("num", AggFunction::Quantile)
            },
            agg("num", AggFunction::Count),
            agg("num", AggFunction::Nunique),
            Aggregation {
                skip_null: false,
                ..agg("num", AggFunction::Mean)
            },
            Aggregation {
                distinct: true,
                ..agg("num", AggFunction::Sum)
            },
            agg("val", AggFunction::Sum),
            agg("val", AggFunction::Nunique),
            agg("txt", AggFunction::First),
            agg("txt", AggFunction::Last),
            Aggregation {
                distinct: true,
                ..agg("txt", AggFunction::Concat)
            },
        ]
    }

    /// Fixture mista: chiavi Utf8 con null, Float64 con NaN/-0.0/null/inf,
    /// Int64 con negativi e null, Utf8 con duplicati e null.
    fn mixed_fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("b"),
                    None,
                    Some("a"),
                    Some("b"),
                    None,
                    Some("a"),
                    Some("b"),
                    Some("a"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(f64::NAN),
                    Some(-0.0),
                    None,
                    Some(0.0),
                    Some(2.5),
                    Some(f64::INFINITY),
                    Some(f64::NAN),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(10),
                    None,
                    Some(-3),
                    Some(7),
                    Some(10),
                    Some(-3),
                    Some(0),
                    Some(42),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("x"),
                    Some("y"),
                    None,
                    Some("x"),
                    Some("z"),
                    Some("y"),
                    None,
                    Some("w"),
                ])),
            ],
        )
        .expect("fixture mista")
    }

    #[test]
    fn aggregate_fast_path_matches_reference_on_mixed_float() {
        let batch = mixed_fixture();
        let config = Aggregate {
            group_by: vec!["g".into()],
            aggregations: full_aggregations(),
        };
        assert_aggregate_parity(&batch, &config);
    }

    #[test]
    fn aggregate_fast_path_matches_reference_on_numeric_and_bool_keys() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("ki", DataType::Int64, true),
                Field::new("ku", DataType::UInt64, true),
                Field::new("kb", DataType::Boolean, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(-5),
                    Some(10),
                    None,
                    Some(-5),
                    Some(2),
                    Some(10),
                ])),
                Arc::new(UInt64Array::from(vec![
                    Some(9),
                    Some(10),
                    Some(9),
                    None,
                    Some(2),
                    Some(10),
                ])),
                Arc::new(plenora_core::arrow::array::BooleanArray::from(vec![
                    Some(true),
                    None,
                    Some(false),
                    Some(true),
                    Some(false),
                    None,
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(3.25),
                    Some(-0.0),
                    Some(0.0),
                    None,
                    Some(f64::NAN),
                    Some(1.0),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(-1),
                    Some(2),
                    None,
                    Some(-1),
                    Some(5),
                    Some(2),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("aa"),
                    Some("b"),
                    Some("aa"),
                    None,
                    Some("c"),
                    Some("b"),
                ])),
            ],
        )
        .expect("fixture chiavi numeriche");
        // Chiave singola Int64 (ordine chiavi per lunghezza stringa: -5, 2,
        // 10 con null in testa).
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["ki".into()],
                aggregations: full_aggregations(),
            },
        );
        // Chiavi miste multi-colonna: UInt64 + Boolean + Int64.
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["ku".into(), "kb".into(), "ki".into()],
                aggregations: full_aggregations(),
            },
        );
    }

    #[test]
    fn aggregate_fast_path_matches_reference_on_float_group_keys() {
        // Chiavi Float64: NaN collide con NaN nella chiave, -0.0 e 0.0 sono
        // chiavi distinte ("-0" vs "0").
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Float64, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![
                    Some(-0.0),
                    Some(0.0),
                    Some(f64::NAN),
                    None,
                    Some(f64::NAN),
                    Some(1.5),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.0),
                    Some(2.0),
                    Some(3.0),
                    Some(4.0),
                    Some(f64::NAN),
                    None,
                ])),
                Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5, 6])),
                Arc::new(StringArray::from(vec!["a", "b", "c", "d", "e", "f"])),
            ],
        )
        .expect("fixture chiavi float");
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );
    }

    #[test]
    fn aggregate_fast_path_matches_reference_on_empty_and_single_group() {
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
                Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
            ],
        )
        .expect("fixture vuota");
        assert_aggregate_parity(
            &empty,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );

        // Gruppo singolo: tutte le righe con la stessa chiave, con e senza
        // aggregazioni esplicite (ramo `count` implicito).
        let single = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["k", "k", "k"])),
                Arc::new(Float64Array::from(vec![Some(-0.0), None, Some(f64::NAN)])),
                Arc::new(Int64Array::from(vec![Some(4), Some(-2), None])),
                Arc::new(StringArray::from(vec![Some("s"), None, Some("s")])),
            ],
        )
        .expect("fixture gruppo singolo");
        assert_aggregate_parity(
            &single,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );
        assert_aggregate_parity(
            &single,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: vec![],
            },
        );
    }

    /// Fixture a scala: sopra la soglia parallela (`32_768` righe) e, con
    /// `groups` grande, sopra la soglia anche per l'ordinamento chiavi.
    fn scale_fixture(rows: usize, groups: i64) -> RecordBatch {
        let keys = (0..rows)
            .map(|row| {
                if row % 17 == 0 {
                    None
                } else {
                    let row = i64::try_from(row).expect("riga");
                    Some((row * 7_919) % groups)
                }
            })
            .collect::<Vec<_>>();
        let nums = (0..rows)
            .map(|row| {
                if row % 7 == 0 {
                    None
                } else if row % 13 == 0 {
                    Some(f64::NAN)
                } else if row % 11 == 0 {
                    Some(-0.0)
                } else {
                    Some(f64::from(u32::try_from(row % 10_000).expect("valore")) * 0.5)
                }
            })
            .collect::<Vec<_>>();
        let ints = (0..rows)
            .map(|row| {
                if row % 5 == 0 {
                    None
                } else {
                    let row = i64::try_from(row).expect("riga");
                    Some(row % 1_000 - 500)
                }
            })
            .collect::<Vec<_>>();
        let texts = (0..rows)
            .map(|row| {
                if row % 23 == 0 {
                    None
                } else {
                    Some(format!("t{}", row % 3_000))
                }
            })
            .collect::<Vec<_>>();
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Int64, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(keys)),
                Arc::new(Float64Array::from(nums)),
                Arc::new(Int64Array::from(ints)),
                Arc::new(StringArray::from(texts)),
            ],
        )
        .expect("fixture a scala")
    }

    #[test]
    fn aggregate_fast_path_matches_reference_at_parallel_scale_small_groups() {
        // 70_000 righe, 100 gruppi: calcolo gruppi in parallelo, chiavi
        // ordinate in sequenziale.
        let batch = scale_fixture(70_000, 100);
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );
    }

    #[test]
    fn aggregate_fast_path_matches_reference_at_parallel_scale_large_groups() {
        // 70_000 righe, 35_000 gruppi: anche l'ordinamento chiavi va in
        // parallelo (sopra soglia).
        let batch = scale_fixture(70_000, 35_000);
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );
    }

    #[test]
    fn aggregate_fast_path_preserves_errors() {
        // Aggregazione numerica su Utf8 non numerico: stesso errore.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["a", "b"])),
                Arc::new(StringArray::from(vec!["non-numero", "xyz"])),
            ],
        )
        .expect("fixture errori");
        let config = Aggregate {
            group_by: vec!["g".into()],
            aggregations: vec![agg("txt", AggFunction::Sum)],
        };
        let fast_error = aggregate(&batch, &config).expect_err("fast path errore");
        let reference_error = aggregate_reference(&batch, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());

        // Chiave di gruppo LargeUtf8: il percorso generico delle chiavi
        // fallisce nello stesso modo (tipo fuori profilo scalare).
        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::LargeUtf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(LargeStringArray::from(vec![Some("a"), Some("b")])),
                Arc::new(Float64Array::from(vec![1.0, 2.0])),
            ],
        )
        .expect("fixture large utf8");
        let config = Aggregate {
            group_by: vec!["g".into()],
            aggregations: vec![agg("num", AggFunction::Sum)],
        };
        let fast_error = aggregate(&large, &config).expect_err("fast path errore");
        let reference_error =
            aggregate_reference(&large, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());

        // group_by vuoto: errore di contratto invariato.
        let empty_group = Aggregate {
            group_by: vec![],
            aggregations: vec![],
        };
        assert!(aggregate(&batch, &empty_group).is_err());
    }

    #[test]
    fn aggregate_fast_path_matches_reference_with_alias_and_date_keys() {
        // Chiavi Date32 (percorso generico delle chiavi) + alias espliciti.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("d", DataType::Date32, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(plenora_core::arrow::array::Date32Array::from(vec![
                    Some(19_000),
                    None,
                    Some(19_000),
                    Some(-1),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.0),
                    Some(2.0),
                    Some(3.0),
                    Some(4.0),
                ])),
            ],
        )
        .expect("fixture date");
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["d".into()],
                aggregations: vec![
                    Aggregation {
                        alias: "totale".into(),
                        ..agg("num", AggFunction::Sum)
                    },
                    Aggregation {
                        alias: "media".into(),
                        ..agg("num", AggFunction::Mean)
                    },
                ],
            },
        );
    }

    /// Chiave testuale di `row_key` per un valore Int64 non null.
    fn i64_text_key(value: i64) -> String {
        let text = value.to_string();
        format!("Int64\u{1e}1{}:{text}\u{1f}", text.len())
    }

    fn u64_text_key(value: u64) -> String {
        let text = value.to_string();
        format!("UInt64\u{1e}1{}:{text}\u{1f}", text.len())
    }

    fn str_text_key(value: &str) -> String {
        format!("Utf8\u{1e}1{}:{value}\u{1f}", value.len())
    }

    #[test]
    fn native_group_key_order_matches_text_key_order() {
        // Confini di lunghezza decimale, segni, estremi di dominio.
        let edge_i64 = [
            0,
            1,
            -1,
            5,
            -5,
            9,
            10,
            -9,
            -10,
            42,
            -42,
            99,
            100,
            -99,
            -100,
            999,
            1000,
            999_999_999,
            1_000_000_000,
            -999_999_999,
            -1_000_000_000,
            i64::MAX,
            i64::MAX - 1,
            i64::MIN,
            i64::MIN + 1,
        ];
        // Sweep pseudocasuale deterministico (seed 42): meta' valori pieni,
        // meta' corti per battere i confini di lunghezza.
        let mut state = 42_u64;
        let mut samples_i64 = Vec::new();
        for _ in 0..4_096 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let value = (state >> 16).cast_signed();
            samples_i64.push(if state & 1 == 0 { value % 2_001 - 1_000 } else { value });
        }
        for &a in &samples_i64 {
            for &b in edge_i64.iter().chain(samples_i64.iter().take(64)) {
                assert_eq!(
                    cmp_i64_group_key(a, b),
                    i64_text_key(a).cmp(&i64_text_key(b)),
                    "i64 {a} vs {b}"
                );
            }
        }
        for &a in &edge_i64 {
            for &b in &edge_i64 {
                assert_eq!(
                    cmp_i64_group_key(a, b),
                    i64_text_key(a).cmp(&i64_text_key(b)),
                    "i64 edge {a} vs {b}"
                );
            }
        }

        let unsigned_edges = [
            0,
            1,
            9,
            10,
            99,
            100,
            999,
            1000,
            999_999_999,
            1_000_000_000,
            u64::MAX,
            u64::MAX - 1,
        ];
        for &a in &unsigned_edges {
            for &b in &unsigned_edges {
                assert_eq!(
                    cmp_u64_group_key(a, b),
                    u64_text_key(a).cmp(&u64_text_key(b)),
                    "u64 {a} vs {b}"
                );
            }
        }

        // Stringhe: confini 9/10/11 e 99/100/101 byte, prefissi comuni,
        // multibyte (la lunghezza della chiave e' in byte).
        let mut text_edges: Vec<String> = vec![
            String::new(),
            "a".into(),
            "ab".into(),
            "abc".into(),
            "abd".into(),
            "é".into(),
            "🚀".into(),
        ];
        for len in [8_usize, 9, 10, 11, 12, 98, 99, 100, 101] {
            text_edges.push("x".repeat(len));
            text_edges.push("y".repeat(len));
        }
        for a in &text_edges {
            for b in &text_edges {
                assert_eq!(
                    cmp_str_group_key(a, b),
                    str_text_key(a).cmp(&str_text_key(b)),
                    "str {a:?} vs {b:?}"
                );
            }
        }
    }

    // -------------------------------------------------------------------
    // Test-oracolo di `distinct`/`dedup_advanced`/`window_function`/
    // `rolling_window` (batch 4 ottimizzazioni): output byte-identico alle
    // implementazioni pre-ottimizzazione, copiate verbatim qui sotto come
    // riferimento indipendente.
    // -------------------------------------------------------------------

    /// Copia verbatim dell'implementazione pre-ottimizzazione di `distinct`.
    fn distinct_reference(batch: &RecordBatch, config: &Distinct) -> Result<RecordBatch> {
        let indices = if config.subset.is_empty() {
            (0..batch.num_columns()).collect()
        } else {
            config
                .subset
                .iter()
                .map(|name| column_index(batch, name))
                .collect::<Result<Vec<_>>>()?
        };
        let keys = (0..batch.num_rows())
            .map(|row| row_key(batch, &indices, row))
            .collect::<Result<Vec<_>>>()?;
        let mut counts = HashMap::new();
        for key in &keys {
            *counts.entry(key).or_insert(0_usize) += 1;
        }
        let mut seen = HashSet::new();
        let mut last = HashMap::new();
        for (row, key) in keys.iter().enumerate() {
            last.insert(key, row);
        }
        let rows = keys
            .iter()
            .enumerate()
            .filter_map(|(row, key)| match config.keep {
                Keep::First if seen.insert(key) => Some(row),
                Keep::Last if last.get(key) == Some(&row) => Some(row),
                Keep::False if counts.get(key) == Some(&1) => Some(row),
                _ => None,
            })
            .collect::<Vec<_>>();
        select_rows(batch, &rows)
    }

    /// Copia verbatim dell'implementazione pre-ottimizzazione di
    /// `dedup_advanced` (delegava a `sort` + `distinct`).
    fn dedup_advanced_reference(batch: &RecordBatch, config: &DedupAdvanced) -> Result<RecordBatch> {
        let ordered = if let Some(column) = &config.order_column {
            sort(
                batch,
                &Sort {
                    columns: vec![column.clone()],
                    ascending: config.ascending,
                },
            )?
        } else {
            batch.clone()
        };
        distinct_reference(
            &ordered,
            &Distinct {
                subset: config.subset.clone(),
                keep: match config.keep {
                    Keep::First => Keep::First,
                    Keep::Last => Keep::Last,
                    Keep::False => {
                        return Err(PlenoraError::Contract(
                            "dedup_advanced non supporta keep=false".into(),
                        ))
                    }
                },
            },
        )
    }

    /// Copia verbatim dell'implementazione pre-ottimizzazione di
    /// `rolling_window` (finestra ricostruita in un `Vec` a ogni riga).
    fn rolling_window_reference(batch: &RecordBatch, config: &RollingWindow) -> Result<RecordBatch> {
        if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
            return Err(PlenoraError::Contract(
                "rolling_window: finestra non valida".into(),
            ));
        }
        let ordered = if let Some(column) = &config.order_column {
            sort(
                batch,
                &Sort {
                    columns: vec![column.clone()],
                    ascending: true,
                },
            )?
        } else {
            batch.clone()
        };
        let source = column_index(&ordered, &config.column)?;
        let group = config
            .group_by
            .as_deref()
            .map(|name| column_index(&ordered, name))
            .transpose()?;
        let mut partitions: BTreeMap<Option<String>, Vec<usize>> = BTreeMap::new();
        for row in 0..ordered.num_rows() {
            let key = group
                .map(|index| scalar_as_string(ordered.column(index).as_ref(), row))
                .transpose()?
                .flatten();
            partitions.entry(key).or_default().push(row);
        }
        let mut output = vec![None; ordered.num_rows()];
        for rows in partitions.values() {
            let numbers = rows
                .iter()
                .map(|row| scalar_as_f64(ordered.column(source).as_ref(), *row))
                .collect::<Result<Vec<_>>>()?;
            for (position, row) in rows.iter().enumerate() {
                let start = (position + 1).saturating_sub(config.window);
                let values = numbers[start..=position]
                    .iter()
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>();
                if values.len() < config.min_periods {
                    continue;
                }
                let result = match config.function {
                    RollingKind::Sum => Some(values.iter().sum()),
                    RollingKind::Mean => values
                        .len()
                        .to_f64()
                        .map(|length| values.iter().sum::<f64>() / length),
                    RollingKind::Min => values.iter().copied().reduce(f64::min),
                    RollingKind::Max => values.iter().copied().reduce(f64::max),
                    RollingKind::Stddev if values.len() <= config.ddof => None,
                    RollingKind::Stddev => {
                        let length = values.len().to_f64().ok_or_else(|| {
                            PlenoraError::Contract("dimensione rolling non rappresentabile".into())
                        })?;
                        let mean = values.iter().sum::<f64>() / length;
                        let divisor = (values.len() - config.ddof).to_f64().ok_or_else(|| {
                            PlenoraError::Contract("divisore rolling non rappresentabile".into())
                        })?;
                        Some(
                            (values
                                .iter()
                                .map(|value| (value - mean).powi(2))
                                .sum::<f64>()
                                / divisor)
                                .sqrt(),
                        )
                    }
                };
                output[*row] = result;
            }
        }
        replace_or_append(
            &ordered,
            &config.output_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(output)),
        )
    }

    /// Copia verbatim dell'implementazione pre-ottimizzazione di
    /// `window_function` (partizioni in `BTreeMap` su `Option<String>`).
    #[allow(clippy::too_many_lines)]
    fn window_function_reference(batch: &RecordBatch, config: &WindowFunction) -> Result<RecordBatch> {
        if config.offset == 0 {
            return Err(PlenoraError::Contract("offset deve essere positivo".into()));
        }
        if matches!(config.function, WindowKind::Ntile) {
            if config.buckets.is_none_or(|buckets| buckets == 0) {
                return Err(PlenoraError::Contract(
                    "ntile richiede buckets maggiore di zero".into(),
                ));
            }
        } else if config.buckets.is_some() {
            return Err(PlenoraError::Contract(
                "buckets e' ammesso solo per ntile".into(),
            ));
        }
        let source_index = column_index(batch, &config.column)?;
        let ordered = if let Some(column) = &config.order_column {
            sort(
                batch,
                &Sort {
                    columns: vec![column.clone()],
                    ascending: true,
                },
            )?
        } else {
            batch.clone()
        };
        let group_index = config
            .group_by
            .as_deref()
            .map(|name| column_index(&ordered, name))
            .transpose()?;
        let mut partitions: BTreeMap<Option<String>, Vec<usize>> = BTreeMap::new();
        for row in 0..ordered.num_rows() {
            let key = group_index
                .map(|index| scalar_as_string(ordered.column(index).as_ref(), row))
                .transpose()?
                .flatten();
            partitions.entry(key).or_default().push(row);
        }
        let mut output = vec![None; ordered.num_rows()];
        for rows in partitions.values() {
            let numbers = rows
                .iter()
                .map(|row| scalar_as_f64(ordered.column(source_index).as_ref(), *row))
                .collect::<Result<Vec<_>>>()?;
            let mut sorted = numbers.iter().flatten().copied().collect::<Vec<_>>();
            sorted.sort_by(f64::total_cmp);
            let mut dense = sorted.clone();
            dense.dedup_by(|left, right| left.total_cmp(right) == Ordering::Equal);
            let mut sum = 0.0;
            let mut count = 0.0_f64;
            for (position, row) in rows.iter().enumerate() {
                output[*row] = match config.function {
                    WindowKind::Cumcount => position.to_f64(),
                    WindowKind::Cumsum => numbers[position].map(|value| {
                        sum += value;
                        sum
                    }),
                    WindowKind::RunningMean => numbers[position].map(|value| {
                        sum += value;
                        count += 1.0;
                        sum / count
                    }),
                    WindowKind::Lag => position
                        .checked_sub(config.offset)
                        .and_then(|other| numbers[other]),
                    WindowKind::Lead => numbers.get(position + config.offset).copied().flatten(),
                    WindowKind::PctChange => position
                        .checked_sub(1)
                        .and_then(|previous| numbers[previous])
                        .and_then(|previous| {
                            numbers[position]
                                .filter(|_| previous != 0.0)
                                .map(|current| (current - previous) / previous)
                        }),
                    WindowKind::Rank | WindowKind::DenseRank => {
                        numbers[position].and_then(|current| {
                            if matches!(config.function, WindowKind::DenseRank) {
                                dense
                                    .binary_search_by(|value| value.total_cmp(&current))
                                    .ok()
                                    .and_then(|index| (index + 1).to_f64())
                            } else {
                                let first =
                                    sorted.partition_point(|value| value.total_cmp(&current).is_lt());
                                let last = sorted
                                    .partition_point(|value| !value.total_cmp(&current).is_gt())
                                    .checked_sub(1)?;
                                (first + last + 2).to_f64().map(|sum| sum / 2.0)
                            }
                        })
                    }
                    WindowKind::PercentRank => numbers[position].and_then(|current| {
                        if sorted.len() <= 1 {
                            return Some(0.0);
                        }
                        let rank = sorted.partition_point(|value| value.total_cmp(&current).is_lt());
                        let numerator = rank.to_f64()?;
                        let denominator = (sorted.len() - 1).to_f64()?;
                        Some(numerator / denominator)
                    }),
                    WindowKind::CumeDist => numbers[position].and_then(|current| {
                        let last = sorted
                            .partition_point(|value| !value.total_cmp(&current).is_gt())
                            .checked_sub(1)?;
                        let numerator = (last + 1).to_f64()?;
                        let denominator = sorted.len().to_f64()?;
                        Some(numerator / denominator)
                    }),
                    WindowKind::Ntile => {
                        let buckets = config.buckets.unwrap_or(1);
                        let effective = buckets.min(rows.len());
                        position
                            .checked_mul(effective)
                            .and_then(|value| value.checked_div(rows.len()))
                            .and_then(|value| (value + 1).to_f64())
                    }
                };
            }
        }
        let suffix = match config.function {
            WindowKind::Rank => "rank",
            WindowKind::DenseRank => "dense_rank",
            WindowKind::Cumsum => "cumsum",
            WindowKind::Cumcount => "cumcount",
            WindowKind::Lag => "lag",
            WindowKind::Lead => "lead",
            WindowKind::PctChange => "pct_change",
            WindowKind::RunningMean => "running_mean",
            WindowKind::PercentRank => "percent_rank",
            WindowKind::CumeDist => "cume_dist",
            WindowKind::Ntile => "ntile",
        };
        let name = config
            .output_column
            .clone()
            .unwrap_or_else(|| format!("{}_{}", config.column, suffix));
        replace_or_append(
            &ordered,
            &name,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(output)),
        )
    }

    fn assert_distinct_parity(batch: &RecordBatch, config: &Distinct) {
        let reference = distinct_reference(batch, config).expect("riferimento");
        let fast = distinct(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = distinct(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    fn assert_dedup_parity(batch: &RecordBatch, config: &DedupAdvanced) {
        let reference = dedup_advanced_reference(batch, config).expect("riferimento");
        let fast = dedup_advanced(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = dedup_advanced(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    fn assert_rolling_parity(batch: &RecordBatch, config: &RollingWindow) {
        let reference = rolling_window_reference(batch, config).expect("riferimento");
        let fast = rolling_window(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = rolling_window(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    fn assert_window_parity(batch: &RecordBatch, config: &WindowFunction) {
        let reference = window_function_reference(batch, config).expect("riferimento");
        let fast = window_function(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = window_function(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    #[test]
    fn distinct_matches_reference_on_mixed_fixture_all_keeps_and_subsets() {
        let batch = mixed_fixture();
        let subsets: Vec<Vec<String>> = vec![
            vec![],
            vec!["g".into()],
            vec!["num".into()],
            vec!["val".into()],
            vec!["txt".into()],
            vec!["g".into(), "val".into()],
            // Colonna ripetuta nel subset: la chiave la include due volte.
            vec!["g".into(), "g".into()],
        ];
        for subset in &subsets {
            for keep in [Keep::First, Keep::Last, Keep::False] {
                assert_distinct_parity(
                    &batch,
                    &Distinct {
                        subset: subset.clone(),
                        keep,
                    },
                );
            }
        }
    }

    #[test]
    fn distinct_matches_reference_on_empty_and_single_row() {
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
            ],
        )
        .expect("fixture vuota");
        for keep in [Keep::First, Keep::Last, Keep::False] {
            assert_distinct_parity(&empty, &Distinct {
                subset: vec![],
                keep,
            });
        }

        // Riga singola con valori null in tutte le colonne.
        let single = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![None::<&str>])),
                Arc::new(Float64Array::from(vec![None::<f64>])),
            ],
        )
        .expect("fixture riga singola");
        for keep in [Keep::First, Keep::Last, Keep::False] {
            assert_distinct_parity(&single, &Distinct {
                subset: vec![],
                keep,
            });
        }
    }

    #[test]
    fn distinct_matches_reference_at_parallel_scale() {
        // Sopra la soglia parallela condivisa (32_768 righe): chiavi
        // numeriche e testuali, con null e valori ripetuti.
        let batch = scale_fixture(70_000, 100);
        for keep in [Keep::First, Keep::Last, Keep::False] {
            assert_distinct_parity(&batch, &Distinct {
                subset: vec!["txt".into()],
                keep,
            });
            assert_distinct_parity(&batch, &Distinct {
                subset: vec!["val".into(), "txt".into()],
                keep,
            });
            assert_distinct_parity(&batch, &Distinct {
                subset: vec![],
                keep,
            });
        }
    }

    #[test]
    fn distinct_preserves_errors() {
        // Colonna LargeUtf8 nel subset: il profilo scalare fallisce allo
        // stesso modo nel riferimento.
        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("c", DataType::LargeUtf8, true)])),
            vec![Arc::new(LargeStringArray::from(vec![Some("a"), Some("b")]))],
        )
        .expect("fixture large utf8");
        let config = Distinct {
            subset: vec![],
            keep: Keep::First,
        };
        let fast_error = distinct(&large, &config).expect_err("fast path errore");
        let reference_error = distinct_reference(&large, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());

        // Colonna inesistente: errore di schema invariato.
        let batch = mixed_fixture();
        let missing = Distinct {
            subset: vec!["manca".into()],
            keep: Keep::First,
        };
        let fast_error = distinct(&batch, &missing).expect_err("fast path errore");
        let reference_error = distinct_reference(&batch, &missing).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
    }

    #[test]
    fn dedup_advanced_matches_reference() {
        let batch = mixed_fixture();
        for keep in [Keep::First, Keep::Last] {
            for ascending in [true, false] {
                assert_dedup_parity(&batch, &DedupAdvanced {
                    subset: vec!["g".into()],
                    keep,
                    order_column: Some("val".into()),
                    ascending,
                });
                assert_dedup_parity(&batch, &DedupAdvanced {
                    subset: vec!["g".into(), "val".into()],
                    keep,
                    order_column: Some("num".into()),
                    ascending,
                });
                // Senza order_column: nessun ordinamento preliminare.
                assert_dedup_parity(&batch, &DedupAdvanced {
                    subset: vec!["txt".into()],
                    keep,
                    order_column: None,
                    ascending,
                });
            }
        }
        // keep=false: errore di contratto identico.
        let config = DedupAdvanced {
            subset: vec!["g".into()],
            keep: Keep::False,
            order_column: None,
            ascending: true,
        };
        let fast_error = dedup_advanced(&batch, &config).expect_err("fast path errore");
        let reference_error =
            dedup_advanced_reference(&batch, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
    }

    /// Config rolling di base sulla fixture mista (`num` con NaN/-0.0/null,
    /// partizione `g` con null, ordine `val`).
    fn rolling_config(function: RollingKind, window: usize, min_periods: usize) -> RollingWindow {
        RollingWindow {
            column: "num".into(),
            function,
            group_by: Some("g".into()),
            order_column: Some("val".into()),
            window,
            min_periods,
            ddof: 1,
            output_column: "num_roll".into(),
        }
    }

    #[test]
    fn rolling_window_matches_reference_all_functions_and_partial_windows() {
        let batch = mixed_fixture();
        for function in [
            RollingKind::Sum,
            RollingKind::Mean,
            RollingKind::Min,
            RollingKind::Max,
            RollingKind::Stddev,
        ] {
            // Finestra piena, parziale (min_periods < window) e ddof 0.
            assert_rolling_parity(&batch, &rolling_config(function, 3, 1));
            assert_rolling_parity(&batch, &rolling_config(function, 3, 3));
            assert_rolling_parity(&batch, &rolling_config(function, 8, 2));
            assert_rolling_parity(&batch, &RollingWindow {
                ddof: 0,
                ..rolling_config(function, 3, 1)
            });
        }
        // Colonna Int64, senza partizione e senza ordinamento.
        assert_rolling_parity(&batch, &RollingWindow {
            column: "val".into(),
            function: RollingKind::Stddev,
            group_by: None,
            order_column: None,
            window: 2,
            min_periods: 1,
            ddof: 1,
            output_column: "val_roll".into(),
        });
        // Partizione singola esplicita (tutte le righe nella stessa chiave).
        let single = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("k"), Some("k"), Some("k")])),
                Arc::new(Float64Array::from(vec![Some(-0.0), None, Some(f64::NAN)])),
            ],
        )
        .expect("fixture partizione singola");
        assert_rolling_parity(&single, &RollingWindow {
            column: "num".into(),
            function: RollingKind::Mean,
            group_by: Some("g".into()),
            order_column: None,
            window: 2,
            min_periods: 1,
            ddof: 1,
            output_column: "num_roll".into(),
        });
        // Finestra di un solo elemento NaN: min/max devono restituire NaN
        // (reduce dal primo elemento), non +/-inf; somma NaN invariata.
        for function in [
            RollingKind::Sum,
            RollingKind::Mean,
            RollingKind::Min,
            RollingKind::Max,
            RollingKind::Stddev,
        ] {
            assert_rolling_parity(&single, &RollingWindow {
                column: "num".into(),
                function,
                group_by: Some("g".into()),
                order_column: None,
                window: 1,
                min_periods: 1,
                ddof: 0,
                output_column: "num_roll".into(),
            });
        }
    }

    #[test]
    fn rolling_window_matches_reference_at_parallel_scale() {
        let batch = scale_fixture(70_000, 100);
        assert_rolling_parity(&batch, &rolling_config(RollingKind::Mean, 10, 1));
        assert_rolling_parity(&batch, &rolling_config(RollingKind::Stddev, 10, 3));
        assert_rolling_parity(&batch, &rolling_config(RollingKind::Sum, 5, 2));
    }

    #[test]
    fn rolling_window_preserves_errors() {
        let batch = mixed_fixture();
        // Finestra non valida: window 0, min_periods 0, min_periods > window.
        let mut config = rolling_config(RollingKind::Mean, 3, 1);
        config.window = 0;
        assert!(rolling_window(&batch, &config).is_err());
        config = rolling_config(RollingKind::Mean, 3, 1);
        config.min_periods = 0;
        assert!(rolling_window(&batch, &config).is_err());
        config = rolling_config(RollingKind::Mean, 3, 1);
        config.min_periods = 4;
        assert!(rolling_window(&batch, &config).is_err());
        // Sorgente non numerica: stesso errore del riferimento.
        let config = RollingWindow {
            column: "txt".into(),
            ..rolling_config(RollingKind::Mean, 3, 1)
        };
        let fast_error = rolling_window(&batch, &config).expect_err("fast path errore");
        let reference_error =
            rolling_window_reference(&batch, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
    }

    /// Config window di base sulla fixture mista (partizione `g` con null,
    /// ordine `val` con null, sorgente `num` con NaN/-0.0/null).
    fn window_config(function: WindowKind) -> WindowFunction {
        WindowFunction {
            column: "num".into(),
            function,
            group_by: Some("g".into()),
            order_column: Some("val".into()),
            offset: 1,
            buckets: None,
            output_column: Some("num_win".into()),
        }
    }

    #[test]
    fn window_function_matches_reference_all_kinds() {
        let batch = mixed_fixture();
        for function in [
            WindowKind::Rank,
            WindowKind::DenseRank,
            WindowKind::Cumsum,
            WindowKind::Cumcount,
            WindowKind::Lag,
            WindowKind::Lead,
            WindowKind::PctChange,
            WindowKind::RunningMean,
            WindowKind::PercentRank,
            WindowKind::CumeDist,
        ] {
            assert_window_parity(&batch, &window_config(function));
        }
        // Offset maggiore di 1 per lag/lead.
        assert_window_parity(&batch, &WindowFunction {
            offset: 3,
            ..window_config(WindowKind::Lag)
        });
        assert_window_parity(&batch, &WindowFunction {
            offset: 2,
            ..window_config(WindowKind::Lead)
        });
        // Ntile: bucket minori, uguali e maggiori della partizione.
        for buckets in [1_usize, 2, 3, 100] {
            assert_window_parity(&batch, &WindowFunction {
                buckets: Some(buckets),
                ..window_config(WindowKind::Ntile)
            });
        }
        // Nome output di default (`{colonna}_{suffix}`), senza partizione,
        // senza ordinamento.
        assert_window_parity(&batch, &WindowFunction {
            column: "num".into(),
            function: WindowKind::RunningMean,
            group_by: None,
            order_column: None,
            offset: 1,
            buckets: None,
            output_column: None,
        });
        // Partizione singola esplicita con NaN nella sorgente.
        let single = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("k"), Some("k"), Some("k")])),
                Arc::new(Float64Array::from(vec![Some(-0.0), None, Some(f64::NAN)])),
            ],
        )
        .expect("fixture partizione singola");
        for function in [WindowKind::Rank, WindowKind::DenseRank, WindowKind::CumeDist] {
            assert_window_parity(&single, &WindowFunction {
                column: "num".into(),
                function,
                group_by: Some("g".into()),
                order_column: None,
                offset: 1,
                buckets: None,
                output_column: None,
            });
        }
    }

    #[test]
    fn window_function_matches_reference_at_parallel_scale() {
        let batch = scale_fixture(70_000, 100);
        assert_window_parity(&batch, &window_config(WindowKind::Rank));
        assert_window_parity(&batch, &window_config(WindowKind::DenseRank));
        assert_window_parity(&batch, &window_config(WindowKind::PercentRank));
        assert_window_parity(&batch, &window_config(WindowKind::Cumsum));
        assert_window_parity(&batch, &WindowFunction {
            buckets: Some(7),
            ..window_config(WindowKind::Ntile)
        });
        // Scala senza partizione: un unico gruppo oltre soglia.
        assert_window_parity(&batch, &WindowFunction {
            group_by: None,
            order_column: Some("num".into()),
            ..window_config(WindowKind::CumeDist)
        });
    }

    #[test]
    fn window_function_preserves_errors() {
        let batch = mixed_fixture();
        // offset 0.
        let config = WindowFunction {
            offset: 0,
            ..window_config(WindowKind::Lag)
        };
        assert!(window_function(&batch, &config).is_err());
        // buckets su funzione diversa da ntile.
        let config = WindowFunction {
            buckets: Some(2),
            ..window_config(WindowKind::Rank)
        };
        assert!(window_function(&batch, &config).is_err());
        // ntile senza buckets o con buckets 0.
        let config = window_config(WindowKind::Ntile);
        assert!(window_function(&batch, &config).is_err());
        let config = WindowFunction {
            buckets: Some(0),
            ..window_config(WindowKind::Ntile)
        };
        assert!(window_function(&batch, &config).is_err());
        // Sorgente non numerica: stesso errore del riferimento.
        let config = WindowFunction {
            column: "txt".into(),
            ..window_config(WindowKind::Rank)
        };
        let fast_error = window_function(&batch, &config).expect_err("fast path errore");
        let reference_error =
            window_function_reference(&batch, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
        // Colonna di partizione LargeUtf8: errore di profilo scalare identico.
        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::LargeUtf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(LargeStringArray::from(vec![Some("a"), Some("b")])),
                Arc::new(Float64Array::from(vec![1.0, 2.0])),
            ],
        )
        .expect("fixture large utf8");
        let config = WindowFunction {
            column: "num".into(),
            function: WindowKind::Rank,
            group_by: Some("g".into()),
            order_column: None,
            offset: 1,
            buckets: None,
            output_column: None,
        };
        let fast_error = window_function(&large, &config).expect_err("fast path errore");
        let reference_error =
            window_function_reference(&large, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
    }

    #[test]
    fn sort_orders_i64_beyond_f64_precision_exactly() {
        // Regressione (bug 6): 2^53 e 2^53+1 collassano sullo stesso double
        // (9007199254740992): il vecchio confronto via f64 li considerava
        // uguali e ricadeva sull'indice di riga, ordinando silenziosamente
        // male. Il confronto nativo `i64::cmp` li distingue.
        let big: i64 = 1 << 53;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("v", DataType::Int64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3, 4])),
                Arc::new(Int64Array::from(vec![big + 1, big, big + 2, -big - 1, -big])),
            ],
        )
        .expect("fixture");
        let ascending = Sort {
            columns: vec!["v".into()],
            ascending: true,
        };
        assert_eq!(sorted_ids(&batch, &ascending), vec![3, 4, 1, 0, 2]);
        let descending = Sort {
            columns: vec!["v".into()],
            ascending: false,
        };
        assert_eq!(sorted_ids(&batch, &descending), vec![2, 0, 1, 4, 3]);
    }

    #[test]
    fn sort_and_top_n_order_u64_numerically() {
        // Regressione (bug 7): UInt64 cadeva nel fallback testuale
        // ("10" < "9"); il confronto nativo `u64::cmp` ordina numericamente.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("u", DataType::UInt64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3])),
                Arc::new(UInt64Array::from(vec![9_u64, 10, 100, 99])),
            ],
        )
        .expect("fixture");
        let ascending = Sort {
            columns: vec!["u".into()],
            ascending: true,
        };
        assert_eq!(sorted_ids(&batch, &ascending), vec![0, 1, 3, 2]);
        let descending = Sort {
            columns: vec!["u".into()],
            ascending: false,
        };
        assert_eq!(sorted_ids(&batch, &descending), vec![2, 3, 1, 0]);

        let top_ascending = top_n(
            &batch,
            &TopN {
                columns: vec!["u".into()],
                n: 2,
                descending: false,
            },
        )
        .expect("top_n ascendente");
        let top_ids = top_ascending
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("id column")
            .values()
            .to_vec();
        assert_eq!(top_ids, vec![0, 1]);
        let top_descending = top_n(
            &batch,
            &TopN {
                columns: vec!["u".into()],
                n: 2,
                descending: true,
            },
        )
        .expect("top_n discendente");
        let top_ids = top_descending
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("id column")
            .values()
            .to_vec();
        assert_eq!(top_ids, vec![2, 3]);
    }

    #[test]
    fn compare_at_generic_path_orders_u64_numerically() {
        // `compare_at` (percorso `ColumnComparator::Generic`) delega allo
        // stesso comparatore tipizzato unico: UInt64 resta numerico anche
        // fuori dal fast path tipizzato.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("u", DataType::UInt64, false)])),
            vec![Arc::new(UInt64Array::from(vec![9_u64, 10]))],
        )
        .expect("fixture");
        assert_eq!(compare_at(&batch, 0, 0, 1).expect("cmp"), Ordering::Less);
        assert_eq!(compare_at(&batch, 0, 1, 0).expect("cmp"), Ordering::Greater);
    }
}
