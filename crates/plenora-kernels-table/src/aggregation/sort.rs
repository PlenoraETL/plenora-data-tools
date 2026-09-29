use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use rayon::prelude::*;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};

use crate::{column_index, select_rows};

use super::compare::{compare_at, validate_sortable};
use super::grouping::visit_key_ids;

/// Config di `table.sort`. Campi sconosciuti rifiutati.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sort {
    /// Chiavi d'ordinamento, dalla piu' significativa: almeno una, senza
    /// ripetizioni, di tipo ordinabile ([`super::is_sortable`]).
    pub columns: Vec<String>,
    /// Verso, lo stesso per tutte le chiavi (default `true`). Il discendente
    /// rovescia l'intero confronto, null compresi: null in testa.
    #[serde(default = "default_true")]
    pub ascending: bool,
}
pub(in crate::aggregation) const fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Comparatori tipizzati di `table.sort`.
//
// Confronto sui valori nativi per i tipi Arrow principali, con semantica
// IDENTICA a `compare_cells_typed` (interi esatti oltre 2^53, `total_cmp`
// per i Float64). Gli altri tipi ricadono su `compare_at`.
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

/// Prevalidazione deterministica delle colonne di ordinamento.
///
/// Percorre le colonne nell'ordine del piano, cosi' il primo errore e'
/// sempre lo stesso. Dentro `par_sort_by` quale confronto fallisca per primo
/// dipende da come Rayon divide il lavoro: l'errore non sarebbe
/// deterministico.
fn prevalidate_sort_columns(batch: &RecordBatch, indices: &[usize]) -> Result<()> {
    for index in indices {
        validate_sortable(batch.column(*index), batch.num_rows())?;
    }
    Ok(())
}

/// Errore di un comparatore che la prevalidazione avrebbe dovuto escludere.
///
/// Il testo e' fisso e non dipende dai dati: anche in questo caso — che e' un
/// difetto nostro, non dell'input — l'identita' dell'errore resta
/// deterministica.
fn comparator_after_prevalidation() -> PlenoraError {
    PlenoraError::Internal(
        "comparatore del sort fallito dopo la prevalidazione delle colonne".to_owned(),
    )
}

/// `table.sort`: batch ordinato per `config.columns`.
///
/// Confronto sul valore nativo di ogni tipo ([`super::compare_cells_typed`]):
/// null dopo ogni valore, `total_cmp` sui `Float64`. Il discendente
/// rovescia l'intero confronto (null in testa). Sort stabile: a parita' di
/// chiavi resta l'ordine d'ingresso, in entrambi i versi. Da 32.768 righe
/// il merge sort e' parallelo, con la stessa permutazione.
///
/// # Errors
///
/// - `InvalidPlan`: `columns` vuoto;
/// - `Schema`: una colonna di `columns` assente dallo schema, di tipo non
///   ordinabile o dictionary con una chiave fuori dal dizionario
///   (prevalidazione deterministica); errore Arrow in `select_rows`;
/// - `ResourceLimit`: indice di riga oltre `u32::MAX` (`select_rows`);
/// - `Internal`: un confronto fallito dopo la prevalidazione (invariante
///   nostra).
pub fn sort(batch: &RecordBatch, config: &Sort) -> Result<RecordBatch> {
    select_rows(batch, &sort_permutation(batch, config)?)
}

/// Permutazione stabile di `sort`: l'i-esimo elemento e' l'indice, in
/// `batch`, della riga che `sort` mette in posizione i.
///
/// E' la stessa permutazione che `sort` applica, esposta al crate perche'
/// lo spill ordina le run con lo stesso comparatore e conserva, per ogni
/// riga della run, l'indice originale.
///
/// # Errors
///
/// Come [`sort`], esclusi quelli di `select_rows`.
pub fn sort_permutation(batch: &RecordBatch, config: &Sort) -> Result<Vec<usize>> {
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
    prevalidate_sort_columns(batch, &indices)?;
    let comparators = indices
        .iter()
        .map(|index| ColumnComparator::new(*index, batch.column(*index)))
        .collect::<Vec<_>>();
    let mut rows: Vec<usize> = (0..batch.num_rows()).collect();
    // Dopo la prevalidazione il comparatore non puo' fallire sui dati: resta
    // solo un flag di backstop, senza payload e quindi senza dipendenza
    // dall'ordine in cui i thread hanno incontrato il problema.
    let unexpected = AtomicBool::new(false);
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
                Err(_) => {
                    unexpected.store(true, AtomicOrdering::Relaxed);
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
    if unexpected.load(AtomicOrdering::Relaxed) {
        return Err(comparator_after_prevalidation());
    }
    Ok(rows)
}

/// Config di `table.top_n`. Campi sconosciuti rifiutati: il verso si
/// scrive `descending`, e `ascending` e' un errore.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopN {
    /// Chiavi d'ordinamento, come [`Sort::columns`].
    pub columns: Vec<String>,
    /// Righe da tenere, da `0` (l'analisi lo limita a `max_rows`).
    pub n: u64,
    /// `true` tiene i valori piu' grandi (default `false`): come
    /// `ascending: false` di [`Sort`], con i null in testa.
    #[serde(default)]
    pub descending: bool,
}

/// `table.top_n`: le prime `n` righe secondo l'ordinamento di [`sort`].
///
/// Output identico a `sort` (con `ascending = !descending`) seguito dalle
/// prime `min(n, righe)` righe: `select_nth_unstable_by` partiziona in
/// O(righe) e ordina solo le prime `n`. Lo spareggio sull'indice originale
/// rende l'ordine totale, quindi la permutazione coincide con quella del
/// sort stabile completo. `n = 0` da' un batch vuoto con lo stesso schema.
///
/// # Errors
///
/// - `InvalidPlan`: `columns` vuoto, oppure `n` non rappresentabile come
///   `usize`;
/// - `Schema`: una colonna di `columns` assente dallo schema, di tipo non
///   ordinabile o dictionary con una chiave fuori dal dizionario (stessa
///   prevalidazione deterministica di `sort`); errore Arrow in
///   `select_rows`;
/// - `ResourceLimit`: indice di riga oltre `u32::MAX` (`select_rows`);
/// - `Internal`: un confronto fallito dopo la prevalidazione.
pub fn top_n(batch: &RecordBatch, config: &TopN) -> Result<RecordBatch> {
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    if indices.is_empty() {
        return Err(PlenoraError::InvalidPlan("top_n richiede colonne".into()));
    }
    // Stessa prevalidazione di `sort`: l'errore nasce dall'ordine dichiarato
    // delle colonne, non dall'ordine in cui `select_nth_unstable_by` capita
    // di confrontare le righe.
    prevalidate_sort_columns(batch, &indices)?;
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
    let unexpected = AtomicBool::new(false);
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
                Err(_) => {
                    unexpected.store(true, AtomicOrdering::Relaxed);
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
    if unexpected.load(AtomicOrdering::Relaxed) {
        return Err(comparator_after_prevalidation());
    }
    select_rows(batch, &rows)
}

/// Quale occorrenza di una chiave ripetuta tengono `table.distinct` e
/// `table.dedup_advanced` (in JSON `"first"`, `"last"`, `"false"`).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    /// La prima occorrenza (default).
    First,
    /// L'ultima occorrenza.
    Last,
    /// Nessuna: solo le righe la cui chiave compare una volta sola. Solo
    /// per `table.distinct`: `table.dedup_advanced` lo rifiuta.
    False,
}
const fn default_keep() -> Keep {
    Keep::First
}

/// Config di `table.distinct`. Campi sconosciuti rifiutati.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distinct {
    /// Colonne della chiave, leggibili come testo e senza ripetizioni;
    /// vuoto (default) vale tutte le colonne.
    #[serde(default)]
    pub subset: Vec<String>,
    /// Occorrenza tenuta (default [`Keep::First`]).
    #[serde(default = "default_keep")]
    pub keep: Keep,
}

/// `table.distinct`: righe distinte sulle colonne di `subset` (vuoto: tutte
/// le colonne), scelte secondo `keep`.
///
/// Uguaglianza delle chiavi sul testo di ogni cella (`scalar_as_string`),
/// colonna per colonna: `-0.0` e `0.0` diverse, ogni NaN uguale agli altri,
/// `Binary` sui byte, null uguale solo a null. Le righe tenute restano in
/// ordine d'ingresso per ogni valore di `keep`.
///
/// # Errors
///
/// - `Schema`: una colonna di `subset` assente dallo schema; una cella che
///   non si converte in testo (`scalar_as_string`: tipo fuori dal profilo
///   scalare, date fuori intervallo, dictionary malformato); errore Arrow
///   in `select_rows`;
/// - `ResourceLimit`: indice di riga oltre `u32::MAX` (`select_rows`);
/// - `Internal`: statistiche senza la chiave (invariante nostra).
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
    // Una sola passata: indice di chiave per riga con la stessa identita'
    // dei byte di `row_key` (`visit_key_ids`: valore nativo su colonna
    // singola, chiave binaria altrimenti) e statistiche per indice. Le righe
    // in uscita restano in ordine crescente di indice per ogni variante di
    // `keep`.
    let mut stats: Vec<KeyStats> = Vec::new();
    visit_key_ids(batch, &indices, |row, indice, nuova| {
        if nuova {
            stats.push(KeyStats {
                first: row,
                last: row,
                count: 1,
            });
        } else {
            let entry = stats.get_mut(indice).ok_or_else(|| {
                PlenoraError::Internal("statistiche distinct senza la chiave".into())
            })?;
            entry.last = row;
            entry.count += 1;
        }
        Ok(())
    })?;
    let mut rows = stats
        .iter()
        .filter_map(|entry| match config.keep {
            Keep::First => Some(entry.first),
            Keep::Last => Some(entry.last),
            Keep::False => (entry.count == 1).then_some(entry.first),
        })
        .collect::<Vec<_>>();
    rows.sort_unstable();
    select_rows(batch, &rows)
}

/// Config di `table.dedup_advanced`. Campi sconosciuti rifiutati.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DedupAdvanced {
    /// Colonne della chiave, obbligatorie (l'analisi le vuole non vuote,
    /// senza ripetizioni, leggibili come testo).
    pub subset: Vec<String>,
    /// Occorrenza tenuta nell'ordine dopo `order_column` (default
    /// [`Keep::First`]; [`Keep::False`] si rifiuta).
    #[serde(default = "default_keep")]
    pub keep: Keep,
    /// Colonna ordinabile su cui si ordina (sort stabile) prima della
    /// deduplica; assente, nessun ordinamento.
    pub order_column: Option<String>,
    /// Verso di `order_column` (assente: ascendente). Senza `order_column`
    /// non c'e' ordinamento, e un verso dichiarato si rifiuta
    /// ([`verifica_verso_dedup`]) invece di essere ignorato.
    #[serde(default)]
    pub ascending: Option<bool>,
}

/// `ascending` di `dedup_advanced` ha effetto solo con `order_column`. La
/// chiamano il kernel e l'analisi dei contratti.
///
/// # Errors
///
/// `InvalidPlan` se `ascending` e' dichiarato senza `order_column`.
pub fn verifica_verso_dedup(config: &DedupAdvanced) -> Result<()> {
    if config.ascending.is_some() && config.order_column.is_none() {
        return Err(PlenoraError::InvalidPlan(
            "ascending senza order_column non ha effetto".into(),
        ));
    }
    Ok(())
}

/// `table.dedup_advanced`: [`distinct`] dopo un [`sort`] stabile su
/// `order_column`.
///
/// Prima e ultima occorrenza si riferiscono all'ordine del sort, e le righe
/// tenute escono in quell'ordine. Senza `order_column` e' `distinct`
/// sull'ordine d'ingresso.
///
/// # Errors
///
/// - `InvalidPlan`: `ascending` senza `order_column`
///   ([`verifica_verso_dedup`]); `keep` e' [`Keep::False`];
/// - gli errori di [`sort`] (se c'e' `order_column`) e di [`distinct`].
pub fn dedup_advanced(batch: &RecordBatch, config: &DedupAdvanced) -> Result<RecordBatch> {
    verifica_verso_dedup(config)?;
    let ordered = if let Some(column) = &config.order_column {
        sort(
            batch,
            &Sort {
                columns: vec![column.clone()],
                ascending: config.ascending.unwrap_or(true),
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
