use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Date32Array, Float64Array, Int64Array, RecordBatch, StringArray,
    UInt32Array, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use rayon::prelude::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use serde::Deserialize;

use crate::hashing::FastHasher;
use crate::Limits;
use crate::{
    column_index, exact_f64_from_i64, scalar_as_f64, scalar_as_string, select_rows,
    validate_output_name,
};
use plenora_core::{PlenoraError, Result};

fn key(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<Option<String>> {
    let mut out = String::new();
    for index in indices {
        let Some(value) = scalar_as_string(batch.column(*index).as_ref(), row)? else {
            return Ok(None);
        };
        out.push_str(&value.len().to_string());
        out.push(':');
        out.push_str(&value);
        out.push('\u{1f}');
    }
    Ok(Some(out))
}

// ---------------------------------------------------------------------------
// Fast path di `join`/`semi_join`/`anti_join` (ottimizzazione kernel, terzo
// batch): chiavi tipizzate su valori nativi Arrow (Int64/UInt64/Float64/
// Boolean/Utf8, stringhe prese in prestito) al posto delle chiavi stringa di
// `key`, e hash FxHash-style al posto di SipHash. Semantica byte-identica al
// percorso generico: stessi match (ogni NaN matcha ogni NaN, perche'
// `f64::to_string` produce "NaN" per tutti; -0.0 distinto da 0.0, perche'
// produce "-0" vs "0"), null nella chiave che non matchano mai, stesso ordine
// di output (righe sinistre in ordine, match destri in ordine di riga, destri
// non matchati in coda), stessi errori. I tipi di chiave fuori dal fast path
// ricadono sul percorso generico a chiavi stringa.
// ---------------------------------------------------------------------------

/// Chiave nativa di una colonna di join: uguaglianza identica alla chiave
/// stringa prodotta da `key` per lo stesso tipo Arrow.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum KeyVal<'a> {
    Int64(i64),
    UInt64(u64),
    /// Bit canonici del valore: tutti i NaN collassano su un unico bit pattern
    /// (la chiave stringa e' "NaN" per ogni NaN); -0.0 resta distinto da 0.0.
    Float64(u64),
    Boolean(bool),
    Utf8(&'a str),
}

const fn f64_key_bits(value: f64) -> u64 {
    if value.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        value.to_bits()
    }
}

/// Colonna di chiave join tipizzata; `None` da `new` indica un tipo fuori dal
/// fast path (fallback al percorso generico).
enum KeyCol<'a> {
    Int64(&'a Int64Array),
    UInt64(&'a UInt64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    Utf8(&'a StringArray),
}

impl<'a> KeyCol<'a> {
    fn new(array: &'a dyn Array) -> Option<Self> {
        match array.data_type() {
            DataType::Int64 => array.as_any().downcast_ref::<Int64Array>().map(Self::Int64),
            DataType::UInt64 => array
                .as_any()
                .downcast_ref::<UInt64Array>()
                .map(Self::UInt64),
            DataType::Float64 => array
                .as_any()
                .downcast_ref::<Float64Array>()
                .map(Self::Float64),
            DataType::Boolean => array
                .as_any()
                .downcast_ref::<BooleanArray>()
                .map(Self::Boolean),
            DataType::Utf8 => array.as_any().downcast_ref::<StringArray>().map(Self::Utf8),
            _ => None,
        }
    }

    fn value(&self, row: usize) -> Option<KeyVal<'a>> {
        match self {
            Self::Int64(values) => values
                .is_valid(row)
                .then(|| KeyVal::Int64(values.value(row))),
            Self::UInt64(values) => values
                .is_valid(row)
                .then(|| KeyVal::UInt64(values.value(row))),
            Self::Float64(values) => values
                .is_valid(row)
                .then(|| KeyVal::Float64(f64_key_bits(values.value(row)))),
            Self::Boolean(values) => values
                .is_valid(row)
                .then(|| KeyVal::Boolean(values.value(row))),
            Self::Utf8(values) => values
                .is_valid(row)
                .then(|| KeyVal::Utf8(values.value(row))),
        }
    }
}

/// Colonne chiave di un lato del join, gia' risolte e tipizzate.
struct FastKeys<'a> {
    columns: Vec<KeyCol<'a>>,
    rows: usize,
}

impl<'a> FastKeys<'a> {
    fn new(batch: &'a RecordBatch, indices: &[usize]) -> Option<Self> {
        let columns = indices
            .iter()
            .map(|index| KeyCol::new(batch.column(*index).as_ref()))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            columns,
            rows: batch.num_rows(),
        })
    }

    /// Chiave composta della riga, scritta in `buffer` (riusato tra le righe);
    /// `None` se almeno una colonna e' null, come nella chiave stringa.
    fn get<'b>(&self, row: usize, buffer: &'b mut Vec<KeyVal<'a>>) -> Option<&'b [KeyVal<'a>]> {
        buffer.clear();
        for column in &self.columns {
            buffer.push(column.value(row)?);
        }
        Some(buffer.as_slice())
    }
}

fn take_optional(
    array: &dyn plenora_core::arrow::array::Array,
    rows: &[Option<usize>],
) -> Result<ArrayRef> {
    let indices: UInt32Array = rows
        .iter()
        .map(|row| {
            row.map(|row| {
                u32::try_from(row)
                    .map_err(|_| PlenoraError::ResourceLimit("indice oltre u32".into()))
            })
            .transpose()
        })
        .collect::<Result<Vec<_>>>()?
        .into();
    Ok(plenora_core::arrow::select::take::take(
        array, &indices, None,
    )?)
}

fn coalesce(left: &dyn Array, right: &dyn Array) -> Result<ArrayRef> {
    if left.data_type() != right.data_type() || left.len() != right.len() {
        return Err(PlenoraError::Schema(
            "chiavi join con tipi o lunghezze incompatibili".into(),
        ));
    }
    macro_rules! typed {
        ($kind:ty) => {{
            let left = left
                .as_any()
                .downcast_ref::<$kind>()
                .ok_or_else(|| PlenoraError::Schema("downcast chiave join fallito".into()))?;
            let right = right
                .as_any()
                .downcast_ref::<$kind>()
                .ok_or_else(|| PlenoraError::Schema("downcast chiave join fallito".into()))?;
            Arc::new(<$kind>::from(
                (0..left.len())
                    .map(|row| {
                        if left.is_null(row) {
                            right.is_valid(row).then(|| right.value(row))
                        } else {
                            Some(left.value(row))
                        }
                    })
                    .collect::<Vec<_>>(),
            )) as ArrayRef
        }};
    }
    Ok(match left.data_type() {
        plenora_core::arrow::schema::DataType::Utf8 => {
            let left = left
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PlenoraError::Schema("downcast chiave join fallito".into()))?;
            let right = right
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PlenoraError::Schema("downcast chiave join fallito".into()))?;
            Arc::new(StringArray::from(
                (0..left.len())
                    .map(|row| {
                        if left.is_null(row) {
                            right.is_valid(row).then(|| right.value(row).to_owned())
                        } else {
                            Some(left.value(row).to_owned())
                        }
                    })
                    .collect::<Vec<_>>(),
            ))
        }
        plenora_core::arrow::schema::DataType::Int64 => typed!(Int64Array),
        plenora_core::arrow::schema::DataType::UInt64 => typed!(UInt64Array),
        plenora_core::arrow::schema::DataType::Float64 => typed!(Float64Array),
        plenora_core::arrow::schema::DataType::Boolean => typed!(BooleanArray),
        plenora_core::arrow::schema::DataType::Date32 => typed!(Date32Array),
        other => {
            return Err(PlenoraError::Schema(format!(
                "tipo chiave join non supportato: {other}"
            )))
        }
    })
}

/// `true` se `coalesce` sa fondere questo tipo di chiave.
///
/// `coalesce` serve solo a `right`/`outer`, dove la chiave di output e' la
/// fusione dei due lati. Verificando la sola UGUAGLIANZA dei tipi delle
/// chiavi, un `right join` su una chiave `UInt64` passerebbe validazione e
/// `prepare`, e fallirebbe a meta' esecuzione. Questo predicato e'
/// la stessa lista che `coalesce` implementa, letta dallo schema.
#[must_use]
pub const fn coalesce_supported(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Utf8
            | DataType::Int64
            | DataType::UInt64
            | DataType::Float64
            | DataType::Boolean
            | DataType::Date32
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinHow {
    Inner,
    Left,
    Right,
    Outer,
}
const fn default_join() -> JoinHow {
    JoinHow::Inner
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub left_keys: Vec<String>,
    pub right_keys: Vec<String>,
    #[serde(default = "default_join")]
    pub how: JoinHow,
}

/// Join relazionale tra due batch sulle chiavi configurate (`how`: inner,
/// left, right, outer).
///
/// Le colonne sinistre non chiave prendono suffisso `_L`, le destre non
/// chiave `_R`; con `right`/`outer` le colonne chiave di output sono il
/// coalesce dei due lati.
///
/// # Errors
///
/// - `InvalidPlan`: chiavi vuote o cardinalita' diversa tra i due lati, oppure
///   righe/colonne di output oltre i limiti `max_rows`/`max_columns`;
/// - `Schema`: colonna chiave assente, tipi Arrow delle chiavi non identici
///   tra i due lati, collisione di nomi in output o, per `right`/`outer`,
///   tipo chiave non supportato dal coalesce.
pub fn join(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &Join,
    limits: &Limits,
) -> Result<RecordBatch> {
    join_impl(left, right, config, limits, true)
}

fn join_impl(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &Join,
    limits: &Limits,
    fast: bool,
) -> Result<RecordBatch> {
    if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
        return Err(PlenoraError::InvalidPlan("chiavi join non valide".into()));
    }
    let left_keys = config
        .left_keys
        .iter()
        .map(|name| column_index(left, name))
        .collect::<Result<Vec<_>>>()?;
    let right_keys = config
        .right_keys
        .iter()
        .map(|name| column_index(right, name))
        .collect::<Result<Vec<_>>>()?;
    for (left_index, right_index) in left_keys.iter().zip(&right_keys) {
        if left.column(*left_index).data_type() != right.column(*right_index).data_type() {
            return Err(PlenoraError::Schema(
                "join safe profile richiede chiavi con tipi Arrow identici".into(),
            ));
        }
    }
    let (left_rows, right_rows) = if fast {
        join_rows_fast(left, right, config, limits, &left_keys, &right_keys).unwrap_or_else(
            || join_rows_generic(left, right, config, limits, &left_keys, &right_keys),
        )?
    } else {
        join_rows_generic(left, right, config, limits, &left_keys, &right_keys)?
    };
    let mut output = combine_horizontal(
        left,
        right,
        &left_rows,
        &right_rows,
        &right_keys,
        HorizontalNames::ManipolaJoin {
            left_keys: &left_keys,
        },
        limits,
    )?;
    if matches!(config.how, JoinHow::Right | JoinHow::Outer) {
        let mut columns = output.columns().to_vec();
        for (left_index, right_index) in left_keys.iter().zip(&right_keys) {
            let fallback = take_optional(right.column(*right_index).as_ref(), &right_rows)?;
            columns[*left_index] = coalesce(columns[*left_index].as_ref(), fallback.as_ref())?;
        }
        let cardinalita = output.num_rows();
        output = crate::batch_with_rows(output.schema(), columns, cardinalita)?;
    }
    Ok(output)
}

/// Coppie di indici di riga (sinistri, destri) prodotte dai percorsi join.
type JoinRowPairs = (Vec<Option<usize>>, Vec<Option<usize>>);

/// Coppie di righe del percorso generico (chiavi stringa di `key`), in DUE
/// FASI come il fast path.
///
/// Inserendo tutte le corrispondenze di una riga nei vettori e controllando
/// `max_rows` solo dopo, una sola chiave ad alta molteplicita'
/// materializzerebbe milioni di coppie anche con un limite bassissimo — e
/// le righe destre non abbinate si aggiungerebbero dopo ancora.
fn join_rows_generic(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &Join,
    limits: &Limits,
    left_keys: &[usize],
    right_keys: &[usize],
) -> Result<JoinRowPairs> {
    let supera = || PlenoraError::ResourceLimit("join supera max_rows".into());
    let mut right_map: HashMap<String, Vec<usize>> = HashMap::new();
    for row in 0..right.num_rows() {
        if let Some(key) = key(right, right_keys, row)? {
            right_map.entry(key).or_default().push(row);
        }
    }
    let serve_destra = matches!(config.how, JoinHow::Right | JoinHow::Outer);

    // Primo passo: conteggio, nessuna coppia materializzata. Le chiavi
    // sinistre si calcolano una volta sola e si riusano nel secondo.
    let left_keys_by_row: Vec<Option<String>> = (0..left.num_rows())
        .map(|row| key(left, left_keys, row))
        .collect::<Result<_>>()?;
    let mut matched_right = vec![false; right.num_rows()];
    let mut probe_total = 0_usize;
    for key_value in &left_keys_by_row {
        match key_value.as_ref().and_then(|key| right_map.get(key)) {
            Some(matches) => {
                probe_total = probe_total.checked_add(matches.len()).ok_or_else(supera)?;
                if serve_destra {
                    for &right_row in matches {
                        if let Some(flag) = matched_right.get_mut(right_row) {
                            *flag = true;
                        }
                    }
                }
            }
            None if matches!(config.how, JoinHow::Left | JoinHow::Outer) => {
                probe_total = probe_total.checked_add(1).ok_or_else(supera)?;
            }
            None => {}
        }
        if probe_total > limits.max_rows {
            return Err(supera());
        }
    }
    let unmatched_right = if serve_destra {
        matched_right.iter().filter(|matched| !**matched).count()
    } else {
        0
    };
    let total = probe_total
        .checked_add(unmatched_right)
        .ok_or_else(supera)?;
    if total > limits.max_rows {
        return Err(supera());
    }

    // Secondo passo: allocazione unica ed esatta.
    let mut left_rows: Vec<Option<usize>> = Vec::with_capacity(total);
    let mut right_rows: Vec<Option<usize>> = Vec::with_capacity(total);
    for (left_row, key_value) in left_keys_by_row.iter().enumerate() {
        match key_value.as_ref().and_then(|key| right_map.get(key)) {
            Some(matches) => {
                for &right_row in matches {
                    left_rows.push(Some(left_row));
                    right_rows.push(Some(right_row));
                }
            }
            None if matches!(config.how, JoinHow::Left | JoinHow::Outer) => {
                left_rows.push(Some(left_row));
                right_rows.push(None);
            }
            None => {}
        }
    }
    if serve_destra {
        for (right_row, matched) in matched_right.iter().enumerate() {
            if !matched {
                left_rows.push(None);
                right_rows.push(Some(right_row));
            }
        }
    }
    if left_rows.len() != total {
        return Err(PlenoraError::Internal(
            "join generico: conteggio e riempimento non coincidono".to_owned(),
        ));
    }
    Ok((left_rows, right_rows))
}

/// Coppie di righe del fast path: `None` se un tipo chiave non e' supportato
/// (il chiamante ricade sul generico).
///
/// Il lato di build resta il destro: costruire sul lato piccolo cambierebbe
/// l'ordine di output.
fn join_rows_fast(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &Join,
    limits: &Limits,
    left_keys: &[usize],
    right_keys: &[usize],
) -> Option<Result<JoinRowPairs>> {
    let right_keys_typed = FastKeys::new(right, right_keys)?;
    let left_keys_typed = FastKeys::new(left, left_keys)?;
    Some(join_rows_fast_inner(
        config,
        limits,
        &left_keys_typed,
        &right_keys_typed,
    ))
}

/// Probe del fast path in DUE FASI: prima si conta, poi si riempie.
///
/// In un solo passo ogni chunk accumulerebbe le proprie coppie in vettori
/// locali e verificherebbe `max_rows` sul PROPRIO conteggio: molti chunk
/// singolarmente sotto soglia danno pero' un totale enormemente sopra, e il
/// controllo globale arriverebbe solo dopo la concatenazione — a coppie
/// gia' materializzate. Sarebbe una verifica a valle dell'allocazione che
/// deve impedire.
///
/// La fase di conteggio non materializza niente, il totale si somma in
/// aritmetica controllata e `max_rows` si applica PRIMA di allocare. La
/// fase di riempimento scrive poi in un'unica allocazione esatta, per
/// offset di prefisso, senza vettori intermedi ne' concatenazione finale.
///
/// L'ordine dell'output resta quello sequenziale: i chunk coprono intervalli
/// contigui di righe sinistre e ciascuno scrive nel proprio segmento.
fn join_rows_fast_inner<'a>(
    config: &Join,
    limits: &Limits,
    left_keys: &FastKeys<'a>,
    right_keys: &FastKeys<'a>,
) -> Result<JoinRowPairs> {
    let right_map = RightMap::build(right_keys);
    let parallelo = left_keys.rows >= MIN_RIGHE_PROBE_PARALLELO;
    let chunk_size = if parallelo {
        CHUNK_PROBE
    } else {
        left_keys.rows.max(1)
    };
    let ranges: Vec<std::ops::Range<usize>> = (0..left_keys.rows.div_ceil(chunk_size))
        .map(|chunk| {
            let start = chunk * chunk_size;
            start..start.saturating_add(chunk_size).min(left_keys.rows)
        })
        .collect();

    // Con `right`/`outer` servono anche le righe destre SENZA match, e vanno
    // contate nel primo passo: aggiungerle dopo l'allocazione rimetterebbe il
    // controllo del limite a valle di cio' che deve limitare. La mappa dei match si riempie durante il conteggio,
    // in scritture idempotenti (solo `true`), quindi l'ordine fra i thread e'
    // irrilevante.
    let serve_destra = matches!(config.how, JoinHow::Right | JoinHow::Outer);
    let matched_right: Vec<AtomicBool> = if serve_destra {
        (0..right_keys.rows)
            .map(|_| AtomicBool::new(false))
            .collect()
    } else {
        Vec::new()
    };
    let matched = serve_destra.then_some(matched_right.as_slice());

    // Primo passo: conteggio, nessuna coppia materializzata.
    let counts: Vec<Result<usize>> = if parallelo {
        ranges
            .par_iter()
            .map(|range| count_range(config, left_keys, &right_map, range.clone(), matched))
            .collect()
    } else {
        ranges
            .iter()
            .map(|range| count_range(config, left_keys, &right_map, range.clone(), matched))
            .collect()
    };
    let counts: Vec<usize> = counts.into_iter().collect::<Result<_>>()?;
    let supera = || PlenoraError::ResourceLimit("join supera max_rows".into());
    let probe_total = counts
        .iter()
        .try_fold(0_usize, |acc, count| acc.checked_add(*count))
        .ok_or_else(supera)?;
    let unmatched_right = matched_right
        .iter()
        .filter(|flag| !flag.load(AtomicOrdering::Relaxed))
        .count();
    let total = probe_total
        .checked_add(unmatched_right)
        .ok_or_else(supera)?;
    if total > limits.max_rows {
        return Err(supera());
    }

    // Secondo passo: una sola allocazione, esatta, riempita per offset.
    let mut left_rows: Vec<Option<usize>> = vec![None; total];
    let mut right_rows: Vec<Option<usize>> = vec![None; total];
    let (probe_left, tail_left) = left_rows.split_at_mut(probe_total);
    let (probe_right, tail_right) = right_rows.split_at_mut(probe_total);
    fill_ranges(
        config,
        left_keys,
        &right_map,
        &ranges,
        &counts,
        probe_left,
        probe_right,
        parallelo,
    )?;
    // Coda: le righe destre non abbinate, in ordine di indice.
    let mut position = 0_usize;
    for (right_row, flag) in matched_right.iter().enumerate() {
        if flag.load(AtomicOrdering::Relaxed) {
            continue;
        }
        let (Some(left_slot), Some(right_slot)) =
            (tail_left.get_mut(position), tail_right.get_mut(position))
        else {
            return Err(PlenoraError::Internal(
                "probe del join: coda delle righe destre incoerente col conteggio".to_owned(),
            ));
        };
        *left_slot = None;
        *right_slot = Some(right_row);
        position += 1;
    }
    if position != tail_left.len() {
        return Err(PlenoraError::Internal(
            "probe del join: coda delle righe destre incoerente col conteggio".to_owned(),
        ));
    }
    Ok((left_rows, right_rows))
}

/// Soglia/dimensione dei chunk del probe parallelo: sotto soglia il probe
/// resta sequenziale (il costo di avvio di rayon non si ripaga).
const MIN_RIGHE_PROBE_PARALLELO: usize = 1 << 16;
const CHUNK_PROBE: usize = 1 << 16;

/// Mappa del lato destro: chiave nativa diretta per chiave a colonna singola
/// (nessuna allocazione per riga), slice boxed per chiavi multi-colonna.
enum RightMap<'a> {
    One(HashMap<KeyVal<'a>, Vec<usize>, FastHasher>),
    Many(HashMap<Box<[KeyVal<'a>]>, Vec<usize>, FastHasher>),
}

impl<'a> RightMap<'a> {
    fn build(right_keys: &FastKeys<'a>) -> Self {
        if right_keys.columns.len() == 1 {
            let mut map: HashMap<KeyVal<'a>, Vec<usize>, FastHasher> = HashMap::default();
            let column = &right_keys.columns[0];
            for row in 0..right_keys.rows {
                if let Some(key) = column.value(row) {
                    map.entry(key).or_default().push(row);
                }
            }
            return Self::One(map);
        }
        let mut buffer: Vec<KeyVal<'a>> = Vec::with_capacity(right_keys.columns.len());
        let mut map: HashMap<Box<[KeyVal<'a>]>, Vec<usize>, FastHasher> = HashMap::default();
        for row in 0..right_keys.rows {
            if let Some(key) = right_keys.get(row, &mut buffer) {
                map.entry(key.into()).or_default().push(row);
            }
        }
        Self::Many(map)
    }

    fn get(
        &self,
        left_keys: &FastKeys<'a>,
        row: usize,
        buffer: &mut Vec<KeyVal<'a>>,
    ) -> Option<&Vec<usize>> {
        match self {
            Self::One(map) => left_keys.columns[0]
                .value(row)
                .and_then(|key| map.get(&key)),
            Self::Many(map) => left_keys.get(row, buffer).and_then(|key| map.get(key)),
        }
    }
}

/// Primo passo del probe: quante coppie produce un intervallo di righe
/// sinistre,
/// senza materializzarne nessuna.
///
/// Deve restare l'esatto specchio di [`fill_range`]: stessa condizione di
/// match, stesso ramo per le righe sinistre senza match. Se le due divergono
/// il riempimento se ne accorge (segmento non saturato) e produce `Internal`.
/// `matched` e' presente solo con `right`/`outer`: registra quali righe
/// destre hanno trovato un match, cosi' le non abbinate entrano nel conteggio
/// PRIMA dell'allocazione. Le scritture sono idempotenti (solo `true`),
/// quindi l'esito non dipende dall'ordine fra i thread.
///
/// # Errors
///
/// `ResourceLimit` se la somma delle coppie esce da `usize`: un `saturating_add`
/// trasformerebbe l'overflow in un conteggio apparentemente valido, e il
/// riempimento allocherebbe meno di quanto poi scrive.
fn count_range(
    config: &Join,
    left_keys: &FastKeys<'_>,
    right_map: &RightMap<'_>,
    range: std::ops::Range<usize>,
    matched: Option<&[AtomicBool]>,
) -> Result<usize> {
    let supera = || PlenoraError::ResourceLimit("join supera max_rows".into());
    let mut buffer = Vec::with_capacity(left_keys.columns.len());
    let mut count = 0_usize;
    for left_row in range {
        match right_map.get(left_keys, left_row, &mut buffer) {
            Some(candidate_rows) => {
                count = count.checked_add(candidate_rows.len()).ok_or_else(supera)?;
                if let Some(flags) = matched {
                    for &right_row in candidate_rows {
                        if let Some(flag) = flags.get(right_row) {
                            flag.store(true, AtomicOrdering::Relaxed);
                        }
                    }
                }
            }
            None if matches!(config.how, JoinHow::Left | JoinHow::Outer) => {
                count = count.checked_add(1).ok_or_else(supera)?;
            }
            None => {}
        }
    }
    Ok(count)
}

/// Secondo passo del probe: scrive le coppie di un intervallo nel proprio
/// segmento
/// dei buffer finali, nello stesso ordine del percorso generico.
fn fill_range(
    config: &Join,
    left_keys: &FastKeys<'_>,
    right_map: &RightMap<'_>,
    range: std::ops::Range<usize>,
    left_out: &mut [Option<usize>],
    right_out: &mut [Option<usize>],
) -> Result<()> {
    let incoerente = || {
        PlenoraError::Internal("probe del join: conteggio e riempimento non coincidono".to_owned())
    };
    let mut buffer = Vec::with_capacity(left_keys.columns.len());
    let mut position = 0_usize;
    let mut write = |left: Option<usize>, right: Option<usize>, position: &mut usize| {
        // Scrittura guardata invece di indicizzata: un segmento troppo corto
        // e' un difetto nostro da segnalare, non un panico.
        let (Some(left_slot), Some(right_slot)) =
            (left_out.get_mut(*position), right_out.get_mut(*position))
        else {
            return Err(incoerente());
        };
        *left_slot = left;
        *right_slot = right;
        *position += 1;
        Ok(())
    };
    for left_row in range {
        match right_map.get(left_keys, left_row, &mut buffer) {
            Some(matches) => {
                for &right_row in matches {
                    write(Some(left_row), Some(right_row), &mut position)?;
                }
            }
            None if matches!(config.how, JoinHow::Left | JoinHow::Outer) => {
                write(Some(left_row), None, &mut position)?;
            }
            None => {}
        }
    }
    if position == left_out.len() {
        Ok(())
    } else {
        Err(incoerente())
    }
}

/// Segmento dei buffer finali assegnato a un chunk: la fetta sinistra e la
/// fetta destra, della stessa lunghezza.
type RowPairSegment<'a> = (&'a mut [Option<usize>], &'a mut [Option<usize>]);

/// Ripartisce i buffer finali nei segmenti dei chunk (offset di prefisso) e
/// li riempie, in parallelo o in sequenza.
#[allow(clippy::too_many_arguments)] // Parametri gia' risolti dal chiamante: nessun ricalcolo.
fn fill_ranges(
    config: &Join,
    left_keys: &FastKeys<'_>,
    right_map: &RightMap<'_>,
    ranges: &[std::ops::Range<usize>],
    counts: &[usize],
    left_rows: &mut [Option<usize>],
    right_rows: &mut [Option<usize>],
    parallelo: bool,
) -> Result<()> {
    let incoerente =
        || PlenoraError::Internal("probe del join: offset di prefisso incoerenti".to_owned());
    let mut segments: Vec<RowPairSegment<'_>> = Vec::with_capacity(counts.len());
    let mut left_rest: &mut [Option<usize>] = left_rows;
    let mut right_rest: &mut [Option<usize>] = right_rows;
    for count in counts {
        if *count > left_rest.len() || *count > right_rest.len() {
            return Err(incoerente());
        }
        let (left_head, left_tail) = left_rest.split_at_mut(*count);
        let (right_head, right_tail) = right_rest.split_at_mut(*count);
        segments.push((left_head, right_head));
        left_rest = left_tail;
        right_rest = right_tail;
    }
    if !left_rest.is_empty() || !right_rest.is_empty() {
        return Err(incoerente());
    }
    let jobs: Vec<_> = ranges.iter().cloned().zip(segments).collect();
    if parallelo {
        jobs.into_par_iter()
            .try_for_each(|(range, (left_out, right_out))| {
                fill_range(config, left_keys, right_map, range, left_out, right_out)
            })
    } else {
        jobs.into_iter()
            .try_for_each(|(range, (left_out, right_out))| {
                fill_range(config, left_keys, right_map, range, left_out, right_out)
            })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum HorizontalNames<'a> {
    ManipolaJoin { left_keys: &'a [usize] },
    PandasCross,
    AsOf,
}

/// `take_optional` su piu' colonne: in parallelo (rayon) oltre una soglia di
/// righe. L'output e' identico al sequenziale: ogni colonna e' indipendente e
/// `collect` preserva l'ordine delle colonne.
fn take_columns(columns: &[ArrayRef], rows: &[Option<usize>]) -> Result<Vec<ArrayRef>> {
    const MIN_RIGHE_PARALLELO: usize = 1 << 20;
    if rows.len() >= MIN_RIGHE_PARALLELO && columns.len() > 1 {
        columns
            .par_iter()
            .map(|column| take_optional(column.as_ref(), rows))
            .collect()
    } else {
        columns
            .iter()
            .map(|column| take_optional(column.as_ref(), rows))
            .collect()
    }
}

pub(crate) fn combine_horizontal(
    left: &RecordBatch,
    right: &RecordBatch,
    left_rows: &[Option<usize>],
    right_rows: &[Option<usize>],
    omitted_right: &[usize],
    naming: HorizontalNames<'_>,
    limits: &Limits,
) -> Result<RecordBatch> {
    let left_schema = left.schema();
    let right_schema = right.schema();
    // `max_columns` PRIMA di qualunque allocazione proporzionale alle righe.
    // Il numero di colonne di output si conosce dagli schemi e da
    // `omitted_right`: non serve aver fatto un solo `take` per saperlo. Il
    // controllo stava in fondo, dopo i take di ENTRAMBI i lati — cioe' dopo
    // aver materializzato esattamente le colonne che si stava per rifiutare.
    let colonne_destre = (0..right.num_columns())
        .filter(|index| !omitted_right.contains(index))
        .count();
    let colonne_uscita = left
        .num_columns()
        .checked_add(colonne_destre)
        .ok_or_else(|| PlenoraError::ResourceLimit("overflow conteggio colonne join".into()))?;
    if colonne_uscita > limits.max_columns {
        return Err(PlenoraError::ResourceLimit(format!(
            "join supera max_columns: {colonne_uscita} colonne di output > {}",
            limits.max_columns
        )));
    }
    let left_source_names = left_schema
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect::<HashSet<_>>();
    let right_source_names = right_schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(index, _)| !omitted_right.contains(index))
        .map(|(_, field)| field.name().clone())
        .collect::<HashSet<_>>();
    let mut fields = left_schema
        .fields()
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let original = field.name();
            let name = match naming {
                HorizontalNames::ManipolaJoin { left_keys } if !left_keys.contains(&index) => {
                    format!("{original}_L")
                }
                HorizontalNames::PandasCross if right_source_names.contains(original) => {
                    format!("{original}_x")
                }
                _ => original.clone(),
            };
            validate_output_name(&name)?;
            Ok(field.as_ref().clone().with_name(name).with_nullable(true))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut columns = take_columns(left.columns(), left_rows)?;
    let mut names: HashSet<String> = fields.iter().map(|field| field.name().clone()).collect();
    if names.len() != fields.len() {
        return Err(PlenoraError::Schema(
            "collisione nomi nelle colonne sinistre del join".into(),
        ));
    }
    // Take delle colonne destre incluso una sola volta, prima del giro dei
    // nomi: l'unico errore possibile (indice oltre u32) e' irraggiungibile con
    // input reali, quindi l'anticipo non cambia il comportamento osservabile.
    let included_right = (0..right.num_columns())
        .filter(|index| !omitted_right.contains(index))
        .collect::<Vec<_>>();
    let taken_right = take_columns(
        &included_right
            .iter()
            .map(|index| right.column(*index).clone())
            .collect::<Vec<_>>(),
        right_rows,
    )?;
    for (position, index) in included_right.iter().enumerate() {
        let original = right_schema.field(*index).name();
        let name = match naming {
            HorizontalNames::ManipolaJoin { .. } => format!("{original}_R"),
            HorizontalNames::PandasCross if left_source_names.contains(original) => {
                format!("{original}_y")
            }
            HorizontalNames::AsOf if left_source_names.contains(original) => {
                format!("{original}_R")
            }
            HorizontalNames::PandasCross | HorizontalNames::AsOf => original.clone(),
        };
        validate_output_name(&name)?;
        if !names.insert(name.clone()) {
            return Err(PlenoraError::Schema(format!("collisione join: {name}")));
        }
        fields.push(
            right_schema
                .field(*index)
                .as_ref()
                .clone()
                .with_name(name)
                .with_nullable(true),
        );
        columns.push(taken_right[position].clone());
    }
    // Il tetto sulle colonne e' gia' stato applicato in testa, prima dei
    // take: qui `fields.len()` vale per costruzione `colonne_uscita`.
    //
    // Righe DICHIARATE: con due input a zero colonne l'output ha zero colonne
    // e arrow non saprebbe da dove dedurre la cardinalita'. Il numero giusto
    // e' quello degli indici di riga costruiti sopra.
    crate::batch_with_rows(Arc::new(Schema::new(fields)), columns, left_rows.len())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Concat {
    #[serde(default = "default_true")]
    pub ignore_index: bool,
}
const fn default_true() -> bool {
    true
}

/// Concatenazione verticale di due batch con schema identico (safe profile).
///
/// Nomi e tipi delle colonne devono coincidere posizione per posizione; la
/// nullabilita' risultante e' l'OR dei due input.
///
/// # Errors
///
/// - `Schema`: numero di colonne, nomi o tipi non identici tra i due batch;
/// - `ResourceLimit`: overflow nel conteggio delle righe o righe totali oltre
///   `max_rows`; propaga inoltre gli errori Arrow di concatenazione.
pub fn concat(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &Concat,
    limits: &Limits,
) -> Result<RecordBatch> {
    let _ = config.ignore_index;
    if left.num_columns() != right.num_columns()
        || left
            .schema()
            .fields()
            .iter()
            .zip(right.schema().fields())
            .any(|(left, right)| {
                left.name() != right.name() || left.data_type() != right.data_type()
            })
    {
        return Err(PlenoraError::Schema(
            "concat safe profile richiede nomi e tipi colonna identici".into(),
        ));
    }
    let rows = left
        .num_rows()
        .checked_add(right.num_rows())
        .ok_or_else(|| PlenoraError::ResourceLimit("overflow concat".into()))?;
    if rows > limits.max_rows {
        return Err(PlenoraError::ResourceLimit("concat supera max_rows".into()));
    }
    // MODELLO: `concat` IMPILA le righe — schemi identici, nessuna colonna
    // nuova — quindi la larghezza della riga di output e' quella di una riga
    // di input. Si prende il massimo fra i due lati perche' i tipi a
    // lunghezza variabile possono avere larghezze medie diverse.
    let byte_per_riga = crate::batch_bytes_per_row(left)?.max(crate::batch_bytes_per_row(right)?);
    crate::preflight_output_bytes("concat", rows, byte_per_riga, limits)?;
    let columns = left
        .columns()
        .iter()
        .zip(right.columns())
        .map(|(left, right)| {
            plenora_core::arrow::select::concat::concat(&[left.as_ref(), right.as_ref()])
                .map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    let fields = left
        .schema()
        .fields()
        .iter()
        .zip(right.schema().fields())
        .map(|(left, right)| {
            left.as_ref()
                .clone()
                .with_nullable(left.is_nullable() || right.is_nullable())
        })
        .collect::<Vec<_>>();
    let schema = Schema::new_with_metadata(fields, left.schema().metadata().clone());
    // `rows` e' la somma gia' calcolata sopra: e' la cardinalita' corretta
    // anche quando non c'e' alcuna colonna da cui dedurla.
    crate::batch_with_rows(Arc::new(schema), columns, rows)
}

// ---------------------------------------------------------------------------
// table.concat_by_name (estensione v1.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcatByName {
    /// strict = true: tutti gli schemi devono essere identici (stesse colonne,
    /// stessi tipi, stesso ordine) — stesso vincolo di `concat`.
    #[serde(default)]
    pub strict: bool,
}

/// Schema unione per `concat_by_name`: colonne nell'ordine di prima
/// apparizione sugli input, in ordine di input.
///
/// Per nome: stesso `DataType` ovunque (altrimenti errore, nessun cast);
/// nullable se almeno un input non ha la colonna o ce l'ha nullable. In
/// strict mode gli schemi devono essere identici per sequenza (nome + tipo).
fn union_schema_by_name(inputs: &[&RecordBatch], strict: bool) -> Result<Vec<Field>> {
    let first = inputs[0];
    if strict {
        for other in &inputs[1..] {
            if first.num_columns() != other.num_columns()
                || first
                    .schema()
                    .fields()
                    .iter()
                    .zip(other.schema().fields())
                    .any(|(left, right)| {
                        left.name() != right.name() || left.data_type() != right.data_type()
                    })
            {
                return Err(PlenoraError::Schema(
                    "concat_by_name strict richiede schemi identici".into(),
                ));
            }
        }
        return Ok(first
            .schema()
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let nullable = inputs
                    .iter()
                    .any(|input| input.schema().field(index).is_nullable());
                field.as_ref().clone().with_nullable(nullable)
            })
            .collect());
    }
    let mut fields: Vec<Field> = Vec::new();
    for input in inputs {
        for field in input.schema().fields() {
            if let Some(existing) = fields.iter_mut().find(|f| f.name() == field.name()) {
                if existing.data_type() != field.data_type() {
                    return Err(PlenoraError::Schema(format!(
                        "concat_by_name: tipi incompatibili per la colonna {} ({:?} vs {:?})",
                        field.name(),
                        existing.data_type(),
                        field.data_type()
                    )));
                }
                if field.is_nullable() && !existing.is_nullable() {
                    *existing = existing.clone().with_nullable(true);
                }
            } else {
                fields.push(field.as_ref().clone());
            }
        }
    }
    // Colonna assente in almeno un input -> nullable (le righe di quell'input
    // sono null su quella colonna).
    for field in &mut fields {
        if !field.is_nullable()
            && inputs
                .iter()
                .any(|input| input.schema().index_of(field.name()).is_err())
        {
            *field = field.clone().with_nullable(true);
        }
    }
    Ok(fields)
}

/// Concatenazione N-aria per NOME colonna, non per posizione (estensione
/// v1.2).
///
/// Per ogni colonna dello schema unione tutte le righe di tutti gli input,
/// nell'ordine degli input; gli input senza quella colonna contribuiscono
/// con null. Con `strict` gli schemi devono essere identici.
///
/// # Errors
///
/// - `InvalidPlan`: nessun input.
/// - `ResourceLimit`: overflow nel conteggio delle righe o righe
///   totali oltre `max_rows`;
/// - `Schema`: con `strict` schemi non identici, o tipi incompatibili per
///   una stessa colonna (nessun cast); propaga inoltre gli errori Arrow di
///   concatenazione.
pub fn concat_by_name(
    inputs: &[&RecordBatch],
    config: &ConcatByName,
    limits: &Limits,
) -> Result<RecordBatch> {
    if inputs.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "concat_by_name richiede almeno un input".into(),
        ));
    }
    let first = inputs[0];
    let mut rows = 0_usize;
    for input in inputs {
        rows = rows
            .checked_add(input.num_rows())
            .ok_or_else(|| PlenoraError::ResourceLimit("overflow concat_by_name".into()))?;
    }
    if rows > limits.max_rows {
        return Err(PlenoraError::ResourceLimit(
            "concat_by_name supera max_rows".into(),
        ));
    }
    // Lo schema unione si calcola sui soli SCHEMI: nessuna allocazione
    // proporzionale alle righe. Va quindi prima del tetto sulle colonne e
    // prima della stima dei byte, che senza il conteggio reale delle colonne
    // ignorerebbe proprio quelle aggiunte dagli input con schemi disgiunti.
    let fields = union_schema_by_name(inputs, config.strict)?;
    if fields.len() > limits.max_columns {
        return Err(PlenoraError::ResourceLimit(format!(
            "concat_by_name supera max_columns: {} colonne di output > {}",
            fields.len(),
            limits.max_columns
        )));
    }

    // MODELLO: l'output ha lo schema UNIONE, quindi puo' avere colonne che
    // nessun input misurato possiede — per gli input che non ce l'hanno viene
    // materializzata una colonna di null lunga quanto le loro righe, che
    // occupa memoria. La stima si costruisce quindi PER CAMPO DELLO SCHEMA DI
    // USCITA: per ciascuno, la larghezza massima osservata fra gli input che
    // lo hanno, e il pavimento del tipo per quelli che non lo hanno o che non
    // hanno righe da misurare.
    //
    // Misurare i soli input e prendere il massimo fra le loro larghezze
    // TOTALI darebbe zero a un input vuoto che porta venti colonne nello
    // schema di uscita — cioe' proprio nel caso peggiore.
    let byte_per_riga = fields
        .iter()
        .map(|field| {
            inputs
                .iter()
                .filter_map(|input| {
                    input
                        .schema()
                        .index_of(field.name())
                        .ok()
                        .map(|index| crate::column_bytes_per_row(input.column(index).as_ref()))
                })
                .max()
                .unwrap_or_else(|| crate::type_bytes_floor(field.data_type()))
                .max(crate::type_bytes_floor(field.data_type()))
        })
        .try_fold(0_usize, |totale, larghezza| {
            totale.checked_add(larghezza).ok_or_else(|| {
                PlenoraError::ResourceLimit(
                    "concat_by_name: larghezza di riga non rappresentabile: stima non affidabile"
                        .into(),
                )
            })
        })?;
    crate::preflight_output_bytes("concat_by_name", rows, byte_per_riga, limits)?;
    let columns = fields
        .iter()
        .map(|field| {
            let parts: Vec<ArrayRef> = inputs
                .iter()
                .map(|input| {
                    input.schema().index_of(field.name()).map_or_else(
                        |_| {
                            plenora_core::arrow::array::new_null_array(
                                field.data_type(),
                                input.num_rows(),
                            )
                        },
                        |index| Arc::clone(input.column(index)),
                    )
                })
                .collect();
            let refs: Vec<&dyn Array> = parts.iter().map(AsRef::as_ref).collect();
            plenora_core::arrow::select::concat::concat(&refs).map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    let schema = Schema::new_with_metadata(fields, first.schema().metadata().clone());
    crate::batch_with_rows(Arc::new(schema), columns, rows)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossJoin {}

/// Prodotto cartesiano tra due batch (ogni riga sinistra x ogni destra).
///
/// Nomi in output secondo la convenzione pandas: suffissi `_x`/`_y` sulle
/// colonne omonime.
///
/// # Errors
///
/// - `ResourceLimit`: overflow nel prodotto delle righe, righe oltre `max_rows`
///   o colonne oltre `max_columns`;
/// - `Schema`: collisione di nomi nelle colonne di output.
pub fn cross_join(
    left: &RecordBatch,
    right: &RecordBatch,
    _config: &CrossJoin,
    limits: &Limits,
) -> Result<RecordBatch> {
    let rows = left
        .num_rows()
        .checked_mul(right.num_rows())
        .ok_or_else(|| PlenoraError::ResourceLimit("overflow cross_join".into()))?;
    if rows > limits.max_rows {
        return Err(PlenoraError::ResourceLimit(
            "cross_join supera max_rows".into(),
        ));
    }
    // Preventivo, non consuntivo: `rows` e' esatto (prodotto delle due
    // cardinalita') e qui non e' stato ancora allocato nulla. E' il punto in
    // cui `max_governed_memory_bytes` puo' ancora impedire l'allocazione invece di
    // constatarla.
    //
    // MODELLO della riga di output. Ogni riga del prodotto cartesiano
    // affianca una riga sinistra E una destra: i byte si SOMMANO. Prendere
    // il massimo fra i due lati e' il modello di un impilamento, e
    // sottostima di quasi meta' un affiancamento.
    //
    // Ai buffer del risultato vanno aggiunti i due vettori di indici che
    // questa funzione costruisce PRIMA di chiamare `combine_horizontal`: sono
    // lunghi quanto l'output e non fanno parte di alcun batch, quindi nessuna
    // misura su `left`/`right` li vedrebbe.
    let indici_per_riga = 2 * std::mem::size_of::<Option<usize>>();
    // Somma CONTROLLATA: una stima saturata a `usize::MAX` passerebbe il
    // confronto con un budget anch'esso a fondo scala, cioe' autorizzerebbe
    // l'allocazione proprio quando il conto e' andato perduto.
    let byte_per_riga = crate::batch_bytes_per_row(left)?
        .checked_add(crate::batch_bytes_per_row(right)?)
        .and_then(|somma| somma.checked_add(indici_per_riga))
        .ok_or_else(|| {
            PlenoraError::ResourceLimit(
                "cross_join: larghezza di riga non rappresentabile: stima non affidabile".into(),
            )
        })?;
    crate::preflight_output_bytes("cross_join", rows, byte_per_riga, limits)?;
    let left_rows = (0..left.num_rows())
        .flat_map(|left| std::iter::repeat_n(Some(left), right.num_rows()))
        .collect::<Vec<_>>();
    let right_rows = (0..left.num_rows())
        .flat_map(|_| (0..right.num_rows()).map(Some))
        .collect::<Vec<_>>();
    combine_horizontal(
        left,
        right,
        &left_rows,
        &right_rows,
        &[],
        HorizontalNames::PandasCross,
        limits,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipJoin {
    pub left_keys: Vec<String>,
    pub right_keys: Vec<String>,
}

fn membership_join(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &MembershipJoin,
    keep_matches: bool,
) -> Result<RecordBatch> {
    membership_impl(left, right, config, keep_matches, true)
}

fn membership_impl(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &MembershipJoin,
    keep_matches: bool,
    fast: bool,
) -> Result<RecordBatch> {
    if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
        return Err(PlenoraError::InvalidPlan(
            "chiavi membership join non valide".into(),
        ));
    }
    let left_keys = config
        .left_keys
        .iter()
        .map(|name| column_index(left, name))
        .collect::<Result<Vec<_>>>()?;
    let right_keys = config
        .right_keys
        .iter()
        .map(|name| column_index(right, name))
        .collect::<Result<Vec<_>>>()?;
    for (left_index, right_index) in left_keys.iter().zip(&right_keys) {
        if left.column(*left_index).data_type() != right.column(*right_index).data_type() {
            return Err(PlenoraError::Schema(
                "membership join richiede chiavi con tipi Arrow identici".into(),
            ));
        }
    }
    if fast {
        if let (Some(left_typed), Some(right_typed)) = (
            FastKeys::new(left, &left_keys),
            FastKeys::new(right, &right_keys),
        ) {
            return membership_fast(left, &left_typed, &right_typed, keep_matches);
        }
    }
    let right_keys = (0..right.num_rows())
        .filter_map(|row| key(right, &right_keys, row).transpose())
        .collect::<Result<HashSet<_>>>()?;
    let rows = (0..left.num_rows())
        .filter_map(|row| {
            key(left, &left_keys, row)
                .map(|value| {
                    (value.is_some_and(|value| right_keys.contains(&value)) == keep_matches)
                        .then_some(row)
                })
                .transpose()
        })
        .collect::<Result<Vec<_>>>()?;
    select_rows(left, &rows)
}

/// Fast path di `semi_join`/`anti_join`: `HashSet` su chiavi native, stesse
/// righe (e stesso ordine) del percorso generico.
fn membership_fast<'a>(
    left: &'a RecordBatch,
    left_keys: &FastKeys<'a>,
    right_keys: &FastKeys<'a>,
    keep_matches: bool,
) -> Result<RecordBatch> {
    let right_set = RightSet::build(right_keys);
    let rows = if left_keys.rows >= MIN_RIGHE_PROBE_PARALLELO {
        // Probe in parallelo per chunk: gli indici restano in ordine crescente
        // perche' i chunk sono concatenati in ordine.
        let chunks = left_keys.rows.div_ceil(CHUNK_PROBE);
        (0..chunks)
            .into_par_iter()
            .map(|chunk| {
                let start = chunk * CHUNK_PROBE;
                let end = (start + CHUNK_PROBE).min(left_keys.rows);
                membership_range(&right_set, left_keys, start..end, keep_matches)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
    } else {
        membership_range(&right_set, left_keys, 0..left_keys.rows, keep_matches)
    };
    select_rows(left, &rows)
}

/// Insieme delle chiavi destre di semi/anti join: come `RightMap`, ma senza
/// le liste di righe (basta l'appartenenza).
enum RightSet<'a> {
    One(HashSet<KeyVal<'a>, FastHasher>),
    Many(HashSet<Box<[KeyVal<'a>]>, FastHasher>),
}

impl<'a> RightSet<'a> {
    fn build(right_keys: &FastKeys<'a>) -> Self {
        if right_keys.columns.len() == 1 {
            let mut set: HashSet<KeyVal<'a>, FastHasher> = HashSet::default();
            let column = &right_keys.columns[0];
            for row in 0..right_keys.rows {
                if let Some(key) = column.value(row) {
                    set.insert(key);
                }
            }
            return Self::One(set);
        }
        let mut buffer: Vec<KeyVal<'a>> = Vec::with_capacity(right_keys.columns.len());
        let mut set: HashSet<Box<[KeyVal<'a>]>, FastHasher> = HashSet::default();
        for row in 0..right_keys.rows {
            if let Some(key) = right_keys.get(row, &mut buffer) {
                set.insert(key.into());
            }
        }
        Self::Many(set)
    }

    fn contains(&self, left_keys: &FastKeys<'a>, row: usize, buffer: &mut Vec<KeyVal<'a>>) -> bool {
        match self {
            Self::One(set) => left_keys.columns[0]
                .value(row)
                .is_some_and(|key| set.contains(&key)),
            Self::Many(set) => left_keys
                .get(row, buffer)
                .is_some_and(|key| set.contains(key)),
        }
    }
}

/// Righe sinistre di un intervallo che soddisfano la membership, in ordine.
fn membership_range(
    right_set: &RightSet<'_>,
    left_keys: &FastKeys<'_>,
    range: std::ops::Range<usize>,
    keep_matches: bool,
) -> Vec<usize> {
    let mut buffer = Vec::with_capacity(left_keys.columns.len());
    range
        .filter(|row| right_set.contains(left_keys, *row, &mut buffer) == keep_matches)
        .collect()
}

/// Righe di `left` la cui chiave ha almeno un match in `right` (ordine
/// sinistro conservato).
///
/// # Errors
///
/// - `InvalidPlan`: chiavi vuote o cardinalita' diversa tra i due lati;
/// - `Schema`: colonna chiave assente o tipi Arrow delle chiavi non
///   identici; inoltre gli errori di `select_rows` sull'output.
pub fn semi_join(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &MembershipJoin,
) -> Result<RecordBatch> {
    membership_join(left, right, config, true)
}

/// Righe di `left` la cui chiave NON ha match in `right` (ordine sinistro
/// conservato).
///
/// # Errors
///
/// Come `semi_join`.
pub fn anti_join(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &MembershipJoin,
) -> Result<RecordBatch> {
    membership_join(left, right, config, false)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsOfDirection {
    Backward,
    Forward,
    Nearest,
}

const fn default_asof_direction() -> AsOfDirection {
    AsOfDirection::Backward
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsOfJoin {
    pub left_on: String,
    pub right_on: String,
    #[serde(default)]
    pub left_by: Vec<String>,
    #[serde(default)]
    pub right_by: Vec<String>,
    #[serde(default = "default_asof_direction")]
    pub direction: AsOfDirection,
    pub tolerance: Option<f64>,
    #[serde(default = "default_true")]
    pub allow_exact: bool,
}

/// Valore `on` della riga da array tipizzato (fast path di `asof_join`).
///
/// Stessa conversione di `scalar_as_f64`: un Int64 senza `f64` esatto e' un
/// errore `Schema` (mai arrotondato), null in ingresso -> null in uscita.
///
/// La verifica e' `exact_f64_from_i64`, non `to_f64()`: quest'ultimo non
/// fallisce mai e arrotonda in silenzio. Su una chiave `on` di
/// `asof_join` l'arrotondamento e' particolarmente dannoso — due istanti
/// distinti diventano lo stesso valore, i gruppi collassano e la ricerca del
/// candidato piu' vicino sceglie la riga sbagliata.
fn asof_on_value(array: &dyn Array, row: usize) -> Result<Option<f64>> {
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return values
            .is_valid(row)
            .then(|| {
                exact_f64_from_i64(values.value(row)).ok_or_else(|| {
                    PlenoraError::Schema("intero non rappresentabile come f64".into())
                })
            })
            .transpose();
    }
    let values = array
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| PlenoraError::Schema("asof_join richiede chiavi Int64 o Float64".into()))?;
    Ok(values.is_valid(row).then(|| values.value(row)))
}

/// Righe destre del fast path di `asof_join`: build con chiavi native
/// (`FastKeys`, nessuna stringa per riga) e probe con buffer riusato.
#[allow(clippy::too_many_arguments)] // Parametri gia' risolti dal chiamante (hot path minimale): nessun ricalcolo.
fn asof_right_rows_fast(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &AsOfJoin,
    left_on: usize,
    right_on: usize,
    left_keys: &FastKeys,
    right_keys: &FastKeys,
) -> Result<Vec<Option<usize>>> {
    // Build sul lato destro: la chiave e' clonata solo al primo inserimento
    // del gruppo (pattern gia' usato nei contatori dei limiti).
    let mut groups: HashMap<Vec<KeyVal>, Vec<(f64, usize)>, FastHasher> = HashMap::default();
    let mut buffer: Vec<KeyVal> = Vec::new();
    for row in 0..right.num_rows() {
        let Some(group) = right_keys.get(row, &mut buffer) else {
            continue;
        };
        let Some(value) = asof_on_value(right.column(right_on).as_ref(), row)? else {
            continue;
        };
        if value.is_finite() {
            if let Some(rows) = groups.get_mut(group) {
                rows.push((value, row));
            } else {
                groups.insert(group.to_vec(), vec![(value, row)]);
            }
        }
    }
    for rows in groups.values_mut() {
        rows.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
    }
    let mut right_rows = Vec::with_capacity(left.num_rows());
    for row in 0..left.num_rows() {
        let matched = match (
            left_keys.get(row, &mut buffer),
            asof_on_value(left.column(left_on).as_ref(), row)?,
        ) {
            (Some(group), Some(value)) if value.is_finite() => groups
                .get(group)
                .and_then(|rows| choose_asof(rows, value, &config.direction, config.allow_exact))
                .filter(|(candidate, _)| {
                    config
                        .tolerance
                        .is_none_or(|limit| (candidate - value).abs() <= limit)
                })
                .map(|(_, row)| row),
            _ => None,
        };
        right_rows.push(matched);
    }
    Ok(right_rows)
}

/// Righe destre del percorso generico di `asof_join` (chiave testuale
/// `group_key` + `scalar_as_f64`): riferimento di parita' del fast path
/// tipizzato — stesso risultato per costruzione, usato come fallback per i
/// tipi `by` fuori dal set nativo.
#[allow(clippy::too_many_arguments)] // Parametri gia' risolti dal chiamante: nessun ricalcolo.
fn asof_right_rows_generic(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &AsOfJoin,
    left_by: &[usize],
    right_by: &[usize],
    left_on: usize,
    right_on: usize,
) -> Result<Vec<Option<usize>>> {
    let mut groups: HashMap<String, Vec<(f64, usize)>> = HashMap::new();
    for row in 0..right.num_rows() {
        let Some(group) = group_key(right, right_by, row)? else {
            continue;
        };
        let Some(value) = scalar_as_f64(right.column(right_on).as_ref(), row)? else {
            continue;
        };
        if value.is_finite() {
            groups.entry(group).or_default().push((value, row));
        }
    }
    for rows in groups.values_mut() {
        rows.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
    }
    let mut right_rows = Vec::with_capacity(left.num_rows());
    for row in 0..left.num_rows() {
        let matched = match (
            group_key(left, left_by, row)?,
            scalar_as_f64(left.column(left_on).as_ref(), row)?,
        ) {
            (Some(group), Some(value)) if value.is_finite() => groups
                .get(&group)
                .and_then(|rows| choose_asof(rows, value, &config.direction, config.allow_exact))
                .filter(|(candidate, _)| {
                    config
                        .tolerance
                        .is_none_or(|limit| (candidate - value).abs() <= limit)
                })
                .map(|(_, row)| row),
            _ => None,
        };
        right_rows.push(matched);
    }
    Ok(right_rows)
}

fn group_key(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<Option<String>> {
    if indices.is_empty() {
        Ok(Some(String::new()))
    } else {
        key(batch, indices, row)
    }
}

fn choose_asof(
    candidates: &[(f64, usize)],
    needle: f64,
    direction: &AsOfDirection,
    allow_exact: bool,
) -> Option<(f64, usize)> {
    let backward_split = candidates.partition_point(|(value, _)| {
        if allow_exact {
            *value <= needle
        } else {
            *value < needle
        }
    });
    let forward_split = candidates.partition_point(|(value, _)| {
        if allow_exact {
            *value < needle
        } else {
            *value <= needle
        }
    });
    let forward = candidates.get(forward_split).copied();
    let backward = backward_split
        .checked_sub(1)
        .and_then(|index| candidates.get(index))
        .copied();
    match direction {
        AsOfDirection::Backward => backward,
        AsOfDirection::Forward => forward,
        AsOfDirection::Nearest => match (backward, forward) {
            (Some(before), Some(after)) => {
                if needle - before.0 <= after.0 - needle {
                    Some(before)
                } else {
                    Some(after)
                }
            }
            (before, after) => before.or(after),
        },
    }
}

/// Join asof: ogni riga sinistra e' associata alla riga destra piu' vicina
/// sulla chiave ordinata, per gruppo `by`.
///
/// Chiavi `on` numeriche (Int64/Float64) con lo stesso tipo Arrow sui due
/// lati; il match rispetta `direction`, `tolerance` e `allow_exact`. Le
/// colonne destre `on`/`by` sono omesse dall'output, le altre omonime
/// prendono suffisso `_R`.
///
/// # Errors
///
/// - `InvalidPlan`: cardinalita' diversa tra `left_by` e `right_by`, oppure
///   `tolerance` non finita o negativa;
/// - `Schema`: colonna assente, chiavi `on` non numeriche o di tipo diverso
///   tra i lati, tipi `by` incompatibili, collisione di nomi in output;
/// - inoltre gli errori di limite di `combine_horizontal` (`max_columns`).
pub fn asof_join(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &AsOfJoin,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.left_by.len() != config.right_by.len() {
        return Err(PlenoraError::InvalidPlan(
            "asof_join: cardinalita' by diversa".into(),
        ));
    }
    if config
        .tolerance
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(PlenoraError::InvalidPlan(
            "asof_join: tolerance non valida".into(),
        ));
    }
    let left_on = column_index(left, &config.left_on)?;
    let right_on = column_index(right, &config.right_on)?;
    if left.column(left_on).data_type() != right.column(right_on).data_type()
        || !matches!(
            left.column(left_on).data_type(),
            plenora_core::arrow::schema::DataType::Int64
                | plenora_core::arrow::schema::DataType::Float64
        )
    {
        return Err(PlenoraError::Schema(
            "asof_join richiede chiavi numeriche con tipi Arrow identici".into(),
        ));
    }
    let left_by = config
        .left_by
        .iter()
        .map(|name| column_index(left, name))
        .collect::<Result<Vec<_>>>()?;
    let right_by = config
        .right_by
        .iter()
        .map(|name| column_index(right, name))
        .collect::<Result<Vec<_>>>()?;
    for (left_index, right_index) in left_by.iter().zip(&right_by) {
        if left.column(*left_index).data_type() != right.column(*right_index).data_type() {
            return Err(PlenoraError::Schema(
                "asof_join: tipi by incompatibili".into(),
            ));
        }
    }
    // Fast path tipizzato (hot path minimale): chiavi `by` native (uguaglianza identica
    // alla chiave stringa — invariante documentata di `KeyVal`) e `on` da
    // array tipizzati; se un tipo `by` non e' coperto, il percorso
    // generico per chiave testuale (stesso risultato, per costruzione).
    if let (Some(left_keys), Some(right_keys)) = (
        FastKeys::new(left, &left_by),
        FastKeys::new(right, &right_by),
    ) {
        let right_rows = asof_right_rows_fast(
            left,
            right,
            config,
            left_on,
            right_on,
            &left_keys,
            &right_keys,
        )?;
        let left_rows = (0..left.num_rows()).map(Some).collect::<Vec<_>>();
        let mut omitted = vec![right_on];
        omitted.extend(right_by);
        return combine_horizontal(
            left,
            right,
            &left_rows,
            &right_rows,
            &omitted,
            HorizontalNames::AsOf,
            limits,
        );
    }
    let right_rows =
        asof_right_rows_generic(left, right, config, &left_by, &right_by, left_on, right_on)?;
    let left_rows = (0..left.num_rows()).map(Some).collect::<Vec<_>>();
    let mut omitted = vec![right_on];
    omitted.extend(right_by);
    combine_horizontal(
        left,
        right,
        &left_rows,
        &right_rows,
        &omitted,
        HorizontalNames::AsOf,
        limits,
    )
}

// ---------------------------------------------------------------------------
// Test-oracolo del fast path di join/semi_join/anti_join: l'output del fast
// path (`join_impl`/`membership_impl` con `fast = true`) dev'essere
// byte-identico a quello del percorso generico
// (`fast = false`): stesse righe nello stesso ordine, stessi tipi e nomi,
// stesse null mask, stessi valori (Float64 confrontato sui bit).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use plenora_core::arrow::array::Date32Array;
    use plenora_core::arrow::schema::Field;

    fn batch(pairs: Vec<(&str, ArrayRef)>) -> RecordBatch {
        let fields = pairs
            .iter()
            .map(|(name, column)| Field::new(*name, column.data_type().clone(), true))
            .collect::<Vec<_>>();
        let columns = pairs.into_iter().map(|(_, column)| column).collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("batch di test")
    }

    fn i64_column(values: &[Option<i64>]) -> ArrayRef {
        Arc::new(Int64Array::from(values.to_vec()))
    }

    fn utf8_column(values: &[Option<&str>]) -> ArrayRef {
        Arc::new(StringArray::from(values.to_vec()))
    }

    fn f64_column(values: &[Option<f64>]) -> ArrayRef {
        Arc::new(Float64Array::from(values.to_vec()))
    }

    /// Parita' fast/generico di `asof_join`: stesse righe destre, stessi
    /// indici, su tutte le direzioni, `allow_exact` e tolleranze.
    fn assert_asof_parity(on_is_float: bool) {
        let right = batch(vec![
            (
                "on",
                if on_is_float {
                    f64_column(&[
                        Some(10.0),
                        Some(4.0),
                        Some(7.0),
                        Some(f64::NAN),
                        Some(-0.0),
                        Some(3.0),
                        None,
                    ])
                } else {
                    i64_column(&[Some(10), Some(4), Some(7), Some(0), Some(0), Some(3), None])
                },
            ),
            (
                "by",
                utf8_column(&[
                    Some("a"),
                    Some("a"),
                    Some("b"),
                    Some("b"),
                    Some("b"),
                    None,
                    Some("a"),
                ]),
            ),
            (
                "by2",
                i64_column(&[
                    Some(1),
                    Some(1),
                    Some(2),
                    Some(2),
                    Some(2),
                    Some(9),
                    Some(1),
                ]),
            ),
        ]);
        let left = batch(vec![
            (
                "on",
                if on_is_float {
                    f64_column(&[Some(5.0), Some(8.0), Some(-0.0), Some(1.0), Some(6.0)])
                } else {
                    i64_column(&[Some(5), Some(8), Some(0), Some(1), Some(6)])
                },
            ),
            (
                "by",
                utf8_column(&[Some("a"), Some("b"), Some("b"), None, Some("a")]),
            ),
            (
                "by2",
                i64_column(&[Some(1), Some(2), Some(2), Some(9), Some(1)]),
            ),
        ]);
        let left_by = [1, 2];
        let right_by = [1, 2];
        let left_keys = FastKeys::new(&left, &left_by).expect("fast keys left");
        let right_keys = FastKeys::new(&right, &right_by).expect("fast keys right");
        for (direction, allow_exact, tolerance) in [
            (AsOfDirection::Backward, true, None),
            (AsOfDirection::Backward, false, None),
            (AsOfDirection::Forward, true, None),
            (AsOfDirection::Forward, false, Some(1.5)),
            (AsOfDirection::Nearest, true, Some(1.5)),
            (AsOfDirection::Nearest, false, None),
        ] {
            let config = AsOfJoin {
                left_on: "on".into(),
                right_on: "on".into(),
                left_by: vec!["by".into(), "by2".into()],
                right_by: vec!["by".into(), "by2".into()],
                direction,
                tolerance,
                allow_exact,
            };
            let fast = asof_right_rows_fast(&left, &right, &config, 0, 0, &left_keys, &right_keys)
                .expect("fast");
            let reference =
                asof_right_rows_generic(&left, &right, &config, &left_by, &right_by, 0, 0)
                    .expect("generic");
            assert_eq!(
                fast, reference,
                "parita' fast/generico (on_is_float={on_is_float}): {config:?}"
            );
        }
    }

    #[test]
    fn asof_fast_matches_generic_on_composite_keys_float_and_int() {
        assert_asof_parity(true);
        assert_asof_parity(false);
    }

    #[test]
    fn asof_falls_back_to_generic_for_unsupported_by_types() {
        // Una colonna `by` Date32 non e' nel set nativo di `FastKeys`: il
        // join usa il percorso generico (fallback), senza errori.
        let right = batch(vec![
            ("on", f64_column(&[Some(1.0), Some(5.0), Some(9.0)])),
            (
                "by",
                Arc::new(Date32Array::from(vec![Some(10), Some(10), Some(20)])) as ArrayRef,
            ),
        ]);
        let left = batch(vec![
            ("on", f64_column(&[Some(4.0), Some(8.0)])),
            (
                "by",
                Arc::new(Date32Array::from(vec![Some(10), Some(20)])) as ArrayRef,
            ),
        ]);
        let config = AsOfJoin {
            left_on: "on".into(),
            right_on: "on".into(),
            left_by: vec!["by".into()],
            right_by: vec!["by".into()],
            direction: AsOfDirection::Backward,
            tolerance: None,
            allow_exact: true,
        };
        let output = asof_join(&left, &right, &config, &Limits::default()).expect("fallback");
        assert_eq!(output.num_rows(), 2);
    }

    fn assert_batches_identical(fast: &RecordBatch, reference: &RecordBatch) {
        assert_eq!(fast.num_rows(), reference.num_rows(), "righe");
        assert_eq!(fast.num_columns(), reference.num_columns(), "colonne");
        let fast_schema = fast.schema();
        let reference_schema = reference.schema();
        for index in 0..fast.num_columns() {
            let fast_field = fast_schema.field(index);
            let reference_field = reference_schema.field(index);
            assert_eq!(
                fast_field.name(),
                reference_field.name(),
                "nome colonna {index}"
            );
            assert_eq!(
                fast_field.data_type(),
                reference_field.data_type(),
                "tipo colonna {}",
                fast_field.name()
            );
            assert_eq!(
                fast_field.is_nullable(),
                reference_field.is_nullable(),
                "nullable colonna {}",
                fast_field.name()
            );
            let fast_column = fast.column(index);
            let reference_column = reference.column(index);
            for row in 0..fast.num_rows() {
                assert_eq!(
                    fast_column.is_null(row),
                    reference_column.is_null(row),
                    "null mask colonna {} riga {row}",
                    fast_field.name()
                );
            }
            macro_rules! assert_values {
                ($kind:ty, $value:ident, $converted:expr) => {{
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<$kind>()
                        .expect("downcast colonna fast");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<$kind>()
                        .expect("downcast colonna reference");
                    for row in 0..fast_values.len() {
                        if fast_values.is_valid(row) {
                            let $value = fast_values.value(row);
                            let fast_value = $converted;
                            let $value = reference_values.value(row);
                            let reference_value = $converted;
                            assert_eq!(
                                fast_value,
                                reference_value,
                                "valore colonna {} riga {row}",
                                fast_field.name()
                            );
                        }
                    }
                }};
            }
            match fast_field.data_type() {
                DataType::Int64 => assert_values!(Int64Array, v, v),
                DataType::UInt64 => assert_values!(UInt64Array, v, v),
                DataType::Float64 => assert_values!(Float64Array, v, v.to_bits()),
                DataType::Boolean => assert_values!(BooleanArray, v, v),
                DataType::Utf8 => assert_values!(StringArray, v, v.to_owned()),
                DataType::Date32 => assert_values!(Date32Array, v, v),
                other => panic!("tipo non gestito dal test-oracolo: {other}"),
            }
        }
    }

    fn assert_join_identical(
        left: &RecordBatch,
        right: &RecordBatch,
        left_keys: &[&str],
        right_keys: &[&str],
        hows: &[JoinHow],
    ) {
        for how in hows {
            let config = Join {
                left_keys: left_keys.iter().map(|key| (*key).to_owned()).collect(),
                right_keys: right_keys.iter().map(|key| (*key).to_owned()).collect(),
                how: match how {
                    JoinHow::Inner => JoinHow::Inner,
                    JoinHow::Left => JoinHow::Left,
                    JoinHow::Right => JoinHow::Right,
                    JoinHow::Outer => JoinHow::Outer,
                },
            };
            let fast = join_impl(left, right, &config, &Limits::default(), true);
            let reference = join_impl(left, right, &config, &Limits::default(), false);
            match (fast, reference) {
                (Ok(fast), Ok(reference)) => assert_batches_identical(&fast, &reference),
                (Err(fast), Err(reference)) => {
                    assert_eq!(fast.to_string(), reference.to_string(), "errore join");
                }
                (fast, reference) => panic!(
                    "esiti diversi: fast {}, reference {}",
                    fast.is_ok(),
                    reference.is_ok()
                ),
            }
        }
    }

    fn assert_membership_identical(
        left: &RecordBatch,
        right: &RecordBatch,
        left_keys: &[&str],
        right_keys: &[&str],
    ) {
        let config = MembershipJoin {
            left_keys: left_keys.iter().map(|key| (*key).to_owned()).collect(),
            right_keys: right_keys.iter().map(|key| (*key).to_owned()).collect(),
        };
        for keep_matches in [true, false] {
            let fast =
                membership_impl(left, right, &config, keep_matches, true).expect("membership fast");
            let reference = membership_impl(left, right, &config, keep_matches, false)
                .expect("membership reference");
            assert_batches_identical(&fast, &reference);
        }
    }

    const ALL_HOWS: [JoinHow; 4] = [
        JoinHow::Inner,
        JoinHow::Left,
        JoinHow::Right,
        JoinHow::Outer,
    ];

    /// Chiavi Int64 con duplicati su entrambi i lati (molti-a-molti) e null.
    fn left_i64() -> RecordBatch {
        batch(vec![
            (
                "k",
                i64_column(&[Some(1), Some(2), Some(2), None, Some(3), Some(2), Some(7)]),
            ),
            (
                "lv",
                i64_column(&[
                    Some(10),
                    Some(11),
                    Some(12),
                    Some(13),
                    Some(14),
                    Some(15),
                    Some(16),
                ]),
            ),
        ])
    }

    fn right_i64() -> RecordBatch {
        batch(vec![
            (
                "k",
                i64_column(&[Some(2), Some(2), Some(3), Some(5), None, Some(7)]),
            ),
            (
                "rv",
                i64_column(&[Some(20), Some(21), Some(22), Some(23), Some(24), Some(25)]),
            ),
        ])
    }

    #[test]
    fn fast_join_int64_matches_generic() {
        assert_join_identical(&left_i64(), &right_i64(), &["k"], &["k"], &ALL_HOWS);
    }

    #[test]
    fn fast_join_uint64_matches_generic() {
        let left = batch(vec![(
            "k",
            Arc::new(UInt64Array::from(vec![
                Some(1),
                Some(2),
                Some(2),
                None,
                Some(9),
            ])) as ArrayRef,
        )]);
        let right = batch(vec![(
            "k",
            Arc::new(UInt64Array::from(vec![Some(2), Some(2), Some(9), None])) as ArrayRef,
        )]);
        // Inner/Left: Right/Outer falliscono in `coalesce` (UInt64 non
        // supportato) in entrambi i percorsi, verificato sotto.
        assert_join_identical(
            &left,
            &right,
            &["k"],
            &["k"],
            &[JoinHow::Inner, JoinHow::Left],
        );
        assert_join_identical(
            &left,
            &right,
            &["k"],
            &["k"],
            &[JoinHow::Right, JoinHow::Outer],
        );
    }

    #[test]
    fn fast_join_utf8_matches_generic() {
        let left = batch(vec![
            (
                "k",
                utf8_column(&[Some("a"), Some("b"), Some("b"), None, Some("c"), Some("b")]),
            ),
            (
                "lt",
                utf8_column(&[
                    Some("x"),
                    Some("y"),
                    Some("z"),
                    Some("w"),
                    Some("v"),
                    Some("u"),
                ]),
            ),
        ]);
        let right = batch(vec![
            ("k", utf8_column(&[Some("b"), Some("b"), None, Some("c")])),
            ("rv", i64_column(&[Some(1), Some(2), Some(3), Some(4)])),
        ]);
        assert_join_identical(&left, &right, &["k"], &["k"], &ALL_HOWS);
    }

    #[test]
    fn fast_join_float64_special_values() {
        let nan_a = f64::from_bits(0x7ff8_0000_0000_0000);
        let nan_b = f64::from_bits(0x7ff8_0000_0000_0001);
        let nan_c = f64::from_bits(0xfff8_0000_0000_0000);
        let left = batch(vec![(
            "k",
            Arc::new(Float64Array::from(vec![
                Some(0.0),
                Some(-0.0),
                Some(nan_a),
                Some(1.5),
                Some(f64::INFINITY),
                None,
                Some(nan_c),
                Some(f64::NEG_INFINITY),
            ])) as ArrayRef,
        )]);
        let right = batch(vec![
            (
                "k",
                Arc::new(Float64Array::from(vec![
                    Some(-0.0),
                    Some(0.0),
                    Some(nan_b),
                    Some(nan_b),
                    Some(1.5),
                    Some(f64::NEG_INFINITY),
                    Some(f64::INFINITY),
                    None,
                ])) as ArrayRef,
            ),
            (
                "rv",
                i64_column(&[
                    Some(0),
                    Some(1),
                    Some(2),
                    Some(3),
                    Some(4),
                    Some(5),
                    Some(6),
                    Some(7),
                ]),
            ),
        ]);
        // NaN matcha NaN (stringa "NaN" per ogni bit pattern), -0.0 non matcha
        // 0.0 ("-0" vs "0"), infiniti distinti per segno.
        assert_join_identical(&left, &right, &["k"], &["k"], &ALL_HOWS);
    }

    #[test]
    fn fast_join_boolean_matches_generic() {
        let left = batch(vec![(
            "k",
            Arc::new(BooleanArray::from(vec![
                Some(true),
                Some(false),
                Some(true),
                None,
            ])) as ArrayRef,
        )]);
        let right = batch(vec![
            (
                "k",
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    Some(true),
                    None,
                    Some(false),
                ])) as ArrayRef,
            ),
            ("rv", i64_column(&[Some(1), Some(2), Some(3), Some(4)])),
        ]);
        assert_join_identical(&left, &right, &["k"], &["k"], &ALL_HOWS);
    }

    #[test]
    fn fast_join_multi_column_mixed_matches_generic() {
        let left = batch(vec![
            (
                "ka",
                i64_column(&[Some(1), Some(1), Some(2), None, Some(3), Some(1)]),
            ),
            (
                "kb",
                utf8_column(&[Some("a"), Some("a"), None, Some("b"), Some("c"), Some("b")]),
            ),
            (
                "kf",
                Arc::new(Float64Array::from(vec![
                    Some(0.5),
                    Some(0.5),
                    Some(1.5),
                    Some(2.5),
                    Some(f64::NAN),
                    Some(0.5),
                ])) as ArrayRef,
            ),
        ]);
        let right = batch(vec![
            (
                "ka",
                i64_column(&[Some(1), Some(1), Some(2), Some(3), None, Some(1)]),
            ),
            (
                "kb",
                utf8_column(&[Some("a"), Some("a"), None, Some("c"), Some("b"), Some("b")]),
            ),
            (
                "kf",
                Arc::new(Float64Array::from(vec![
                    Some(0.5),
                    Some(0.5),
                    Some(1.5),
                    Some(f64::NAN),
                    Some(2.5),
                    Some(0.5),
                ])) as ArrayRef,
            ),
            (
                "rv",
                i64_column(&[Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]),
            ),
        ]);
        assert_join_identical(
            &left,
            &right,
            &["ka", "kb", "kf"],
            &["ka", "kb", "kf"],
            &ALL_HOWS,
        );
    }

    #[test]
    fn fast_join_empty_sides_match_generic() {
        let empty_left = batch(vec![("k", i64_column(&[])), ("lv", i64_column(&[]))]);
        let empty_right = batch(vec![("k", i64_column(&[])), ("rv", i64_column(&[]))]);
        assert_join_identical(&left_i64(), &empty_right, &["k"], &["k"], &ALL_HOWS);
        assert_join_identical(&empty_left, &right_i64(), &["k"], &["k"], &ALL_HOWS);
        assert_join_identical(&empty_left, &empty_right, &["k"], &["k"], &ALL_HOWS);
    }

    #[test]
    fn fast_join_fallback_date32_matches_generic() {
        let left = batch(vec![(
            "k",
            Arc::new(Date32Array::from(vec![
                Some(19_000),
                Some(19_000),
                None,
                Some(1),
            ])) as ArrayRef,
        )]);
        let right = batch(vec![
            (
                "k",
                Arc::new(Date32Array::from(vec![Some(19_000), None, Some(2)])) as ArrayRef,
            ),
            ("rv", i64_column(&[Some(1), Some(2), Some(3)])),
        ]);
        // Tipo fuori dal fast path: entrambi i percorsi usano il generico.
        assert_join_identical(
            &left,
            &right,
            &["k"],
            &["k"],
            &[JoinHow::Inner, JoinHow::Left],
        );
        assert_membership_identical(&left, &right, &["k"], &["k"]);
    }

    #[test]
    fn fast_membership_matches_generic() {
        assert_membership_identical(&left_i64(), &right_i64(), &["k"], &["k"]);
        let left = batch(vec![(
            "k",
            utf8_column(&[Some("a"), Some("b"), None, Some("b"), Some("z")]),
        )]);
        let right = batch(vec![("k", utf8_column(&[Some("b"), Some("b"), None]))]);
        assert_membership_identical(&left, &right, &["k"], &["k"]);
        // Chiavi multi-colonna miste con null.
        let left_multi = batch(vec![
            ("ka", i64_column(&[Some(1), Some(1), None, Some(2)])),
            ("kb", utf8_column(&[Some("a"), None, Some("b"), Some("c")])),
        ]);
        let right_multi = batch(vec![
            ("ka", i64_column(&[Some(1), Some(2), None])),
            ("kb", utf8_column(&[Some("a"), Some("c"), Some("b")])),
        ]);
        assert_membership_identical(&left_multi, &right_multi, &["ka", "kb"], &["ka", "kb"]);
        // Lati vuoti.
        let empty = batch(vec![("k", i64_column(&[]))]);
        assert_membership_identical(&left_i64(), &empty, &["k"], &["k"]);
        assert_membership_identical(&empty, &right_i64(), &["k"], &["k"]);
    }

    #[test]
    fn fast_join_molti_a_molti_ordine_stabile() {
        // Ogni chiave ripetuta m x n volte: l'ordine di output deve seguire
        // (riga sinistra, riga destra) in modo lessicografico, come il generico.
        let left = batch(vec![
            ("k", i64_column(&[Some(5); 12])),
            ("lv", i64_column(&(0..12_i64).map(Some).collect::<Vec<_>>())),
        ]);
        let right = batch(vec![
            ("k", i64_column(&[Some(5); 9])),
            ("rv", i64_column(&(0..9_i64).map(Some).collect::<Vec<_>>())),
        ]);
        assert_join_identical(&left, &right, &["k"], &["k"], &ALL_HOWS);
    }

    #[test]
    fn fast_join_parallel_probe_matches_generic() {
        // Oltre MIN_RIGHE_PROBE_PARALLELO: copre il probe rayon a chunk (join e
        // membership), con duplicati (3 per chiave) e null (una riga su 17).
        let left_keys = (0..200_000_i64)
            .map(|row| (row % 17 != 0).then_some(row % 5_000))
            .collect::<Vec<_>>();
        let left_values = (0..200_000_i64).map(Some).collect::<Vec<_>>();
        let left = batch(vec![
            ("k", i64_column(&left_keys)),
            ("lv", i64_column(&left_values)),
        ]);
        let mut right_keys = Vec::new();
        let mut right_values = Vec::new();
        for key in 0..5_000_i64 {
            for dup in 0..3_i64 {
                right_keys.push(Some(key));
                right_values.push(Some(key * 10 + dup));
            }
        }
        let right = batch(vec![
            ("k", i64_column(&right_keys)),
            ("rv", i64_column(&right_values)),
        ]);
        assert_join_identical(&left, &right, &["k"], &["k"], &ALL_HOWS);
        assert_membership_identical(&left, &right, &["k"], &["k"]);
    }

    // -------------------------------------------------------------------
    // table.concat_by_name (estensione v1.2)
    // -------------------------------------------------------------------

    fn concat_config(strict: bool) -> ConcatByName {
        ConcatByName { strict }
    }

    #[test]
    fn concat_by_name_permuted_schemas_and_missing_columns() {
        // Schemi permutati e colonne disgiunte: unione per nome, null dove
        // la colonna manca, ordine degli input conservato.
        let first = batch(vec![
            ("a", i64_column(&[Some(1), Some(2)])),
            ("b", utf8_column(&[Some("x"), Some("y")])),
        ]);
        let second = batch(vec![
            ("b", utf8_column(&[Some("z")])),
            ("a", i64_column(&[Some(3)])),
            ("c", utf8_column(&[Some("only-second")])),
        ]);
        let output = concat_by_name(
            &[&first, &second],
            &concat_config(false),
            &Limits::default(),
        )
        .expect("concat_by_name");
        let schema = output.schema();
        let names: Vec<_> = schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["a", "b", "c"], "ordine di prima apparizione");
        assert_eq!(output.num_rows(), 3);
        let a = output
            .column_by_name("a")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .expect("a");
        assert_eq!(
            (0..3)
                .map(|row| (!a.is_null(row)).then(|| a.value(row)))
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3)]
        );
        let b = output
            .column_by_name("b")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .expect("b");
        assert_eq!(b.value(2), "z");
        let c = output
            .column_by_name("c")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .expect("c");
        assert!(
            c.is_null(0) && c.is_null(1),
            "input senza la colonna -> null"
        );
        assert_eq!(c.value(2), "only-second");
        assert!(output
            .schema()
            .field_with_name("c")
            .expect("c")
            .is_nullable());
        // Input singolo: passthrough (schema e valori identici).
        let single = concat_by_name(&[&first], &concat_config(false), &Limits::default())
            .expect("input singolo");
        assert_eq!(single.num_rows(), first.num_rows());
        assert_eq!(single.num_columns(), first.num_columns());
        // Tre input: ancora per nome.
        let third = batch(vec![("a", i64_column(&[Some(4)]))]);
        let output = concat_by_name(
            &[&first, &second, &third],
            &concat_config(false),
            &Limits::default(),
        )
        .expect("tre input");
        assert_eq!(output.num_rows(), 4);
        let a = output
            .column_by_name("a")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .expect("a");
        assert_eq!(a.value(3), 4);
    }

    #[test]
    fn concat_by_name_rejects_incompatible_types_and_strict_mismatch() {
        let left = batch(vec![("a", i64_column(&[Some(1)]))]);
        let right = batch(vec![("a", utf8_column(&[Some("x")]))]);
        let error = concat_by_name(&[&left, &right], &concat_config(false), &Limits::default())
            .expect_err("tipi incompatibili");
        assert!(error.to_string().contains("tipi incompatibili"));
        // Strict: stessa unione di colonne ma ordine permutato -> errore.
        let permuted = batch(vec![
            ("b", utf8_column(&[Some("x")])),
            ("a", i64_column(&[Some(1)])),
        ]);
        let wide = batch(vec![
            ("a", i64_column(&[Some(1)])),
            ("b", utf8_column(&[Some("x")])),
        ]);
        assert!(concat_by_name(
            &[&wide, &permuted],
            &concat_config(true),
            &Limits::default()
        )
        .is_err());
        // Strict con schemi identici: ok.
        let other = batch(vec![
            ("a", i64_column(&[Some(2)])),
            ("b", utf8_column(&[Some("y")])),
        ]);
        let output = concat_by_name(&[&wide, &other], &concat_config(true), &Limits::default())
            .expect("strict ok");
        assert_eq!(output.num_rows(), 2);
        // Zero input: errore di contratto.
        assert!(concat_by_name(&[], &concat_config(false), &Limits::default()).is_err());
        // Config strict: campo sconosciuto rifiutato.
        assert!(serde_json::from_value::<ConcatByName>(
            serde_json::json!({"strict": false, "surprise": 1})
        )
        .is_err());
    }
}
