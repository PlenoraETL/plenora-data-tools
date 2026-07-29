use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Mutex;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use rayon::prelude::*;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};

use crate::{column_index, select_rows};

use super::compare::compare_at;
use super::grouping::{KeyColumn, KeyHasher};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sort {
    pub columns: Vec<String>,
    #[serde(default = "default_true")]
    pub ascending: bool,
}
pub(in crate::aggregation) const fn default_true() -> bool {
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
    // Match esaustivo sulle quattro combinazioni di null: nessun braccio
    // impossibile, il confronto vero e proprio resta nel caso (false, false).
    match (values.is_null(left), values.is_null(right)) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => compare(values, left, right),
    }
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
/// - `InvalidPlan`: `columns` vuoto;
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
        return Err(PlenoraError::InvalidPlan("sort richiede colonne".into()));
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
/// - `InvalidPlan`: `columns` vuoto, oppure `n` non rappresentabile come
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
        return Err(PlenoraError::InvalidPlan("top_n richiede colonne".into()));
    }
    let n = usize::try_from(config.n)
        .map_err(|_| PlenoraError::InvalidPlan("top_n: n oltre usize".into()))?
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
/// - `InvalidPlan`: `keep` e' `Keep::False` (non supportato);
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
                    return Err(PlenoraError::InvalidPlan(
                        "dedup_advanced non supporta keep=false".into(),
                    ))
                }
            },
        },
    )
}
