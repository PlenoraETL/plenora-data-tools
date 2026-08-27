use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{
    Array, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};

use crate::{column_index, replace_or_append, scalar_as_string, select_rows, validate_output_name};

use super::grouping::{
    build_native_groups, build_string_groups, cmp_i64_group_key, cmp_str_group_key,
    cmp_u64_group_key, map_groups, NumericSource, TextSource, PARALLEL_THRESHOLD,
};
use super::sort::default_true;

/// Cardinalita' di un gruppo come `i64`, in modo **fallibile**.
///
/// La colonna `count` e' dichiarata non-nullable: convertire con
/// `i64::try_from(..).ok()` trasformava un fallimento di conversione in un
/// `null`, cioe' in una violazione silenziosa dello schema appena dichiarato.
/// Il caso e' irraggiungibile sulle piattaforme correnti — non esiste un
/// `Vec` con piu' di `i64::MAX` elementi — ma «irraggiungibile» non e' «esatto
/// per costruzione», e la stessa forma era duplicata su due percorsi di
/// aggregazione. Qui la conversione o riesce o produce un errore esplicito.
///
/// # Errors
///
/// `PlenoraError::Internal` se la cardinalita' non e' rappresentabile in
/// `i64`.
pub(super) fn conteggio_gruppo(righe: usize) -> Result<i64> {
    i64::try_from(righe).map_err(|_| {
        PlenoraError::Internal(
            "cardinalita' di un gruppo non rappresentabile in i64 per la colonna `count`"
                .to_owned(),
        )
    })
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
pub(in crate::aggregation) const fn default_ddof() -> usize {
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

/// Riduzione numerica di un gruppo: logica IDENTICA al percorso generico
/// originale (stesso ordine di somma, `total_cmp` per distinct/quantile,
/// null esclusi o gruppo nullo secondo `skip_null`).
/// Riduzione Sum/Avg/Min/Max/Variance/Stddev senza materializzare il
/// gruppo (hot path minimale): stesse operazioni f64 nello stesso ordine del percorso
/// materializzato (`values.iter().sum()`, due passate per la varianza) —
/// risultato bit-identico, nessuna seconda allocazione per gruppo.
fn reduce_numeric_streaming(raw: &[Option<f64>], aggregation: &Aggregation) -> Result<Option<f64>> {
    let mut len = 0_usize;
    // Inizializza a -0.0: `Iterator::sum` sui float in std fa fold da -0.0
    // (per preservare il segno dello zero); la parita' bit-a-bit con il
    // percorso materializzato include il segno dello zero della somma.
    let mut sum = -0.0_f64;
    for value in raw.iter().flatten() {
        sum += *value;
        len += 1;
    }
    if len == 0 {
        return Ok(None);
    }
    Ok(Some(match aggregation.function {
        AggFunction::Sum => sum,
        AggFunction::Avg | AggFunction::Mean => {
            sum / len.to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione gruppo non rappresentabile".into())
            })?
        }
        AggFunction::Min => raw
            .iter()
            .flatten()
            .copied()
            .reduce(f64::min)
            .unwrap_or_default(),
        AggFunction::Max => raw
            .iter()
            .flatten()
            .copied()
            .reduce(f64::max)
            .unwrap_or_default(),
        AggFunction::Variance | AggFunction::Stddev => {
            if len <= aggregation.ddof {
                return Ok(None);
            }
            let length = len.to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione gruppo non rappresentabile".into())
            })?;
            let mean = sum / length;
            let divisor = (len - aggregation.ddof).to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("divisore statistico non rappresentabile".into())
            })?;
            let variance = raw
                .iter()
                .flatten()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / divisor;
            if matches!(aggregation.function, AggFunction::Stddev) {
                variance.sqrt()
            } else {
                variance
            }
        }
        _ => {
            return Err(PlenoraError::Internal(
                "funzione fuori dal percorso streaming di reduce_numeric".into(),
            ));
        }
    }))
}

fn reduce_numeric(raw: Vec<Option<f64>>, aggregation: &Aggregation) -> Result<Option<f64>> {
    if !aggregation.skip_null && raw.iter().any(Option::is_none) {
        return Ok(None);
    }
    // Solo `distinct` e `quantile` hanno bisogno del gruppo materializzato
    // (ordinamento); per le altre funzioni il secondo `Vec` e' lavoro
    // evitabile (hot path minimale): si riduce sull'iteratore flatten, stesse operazioni
    // f64 nello stesso ordine — parita' bit-a-bit per costruzione.
    if !aggregation.distinct && !matches!(aggregation.function, AggFunction::Quantile) {
        return reduce_numeric_streaming(&raw, aggregation);
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
                PlenoraError::ResourceLimit("dimensione gruppo non rappresentabile".into())
            })?
        }
        AggFunction::Min => values.iter().copied().reduce(f64::min).unwrap_or_default(),
        AggFunction::Max => values.iter().copied().reduce(f64::max).unwrap_or_default(),
        AggFunction::Variance | AggFunction::Stddev => {
            if values.len() <= aggregation.ddof {
                return Ok(None);
            }
            let length = values.len().to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione gruppo non rappresentabile".into())
            })?;
            let mean = sum / length;
            let divisor = (values.len() - aggregation.ddof).to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("divisore statistico non rappresentabile".into())
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
                PlenoraError::InvalidPlan("quantile richiede il parametro quantile".into())
            })?;
            values.sort_by(f64::total_cmp);
            let last = (values.len() - 1).to_f64().ok_or_else(|| {
                PlenoraError::ResourceLimit("dimensione quantile non rappresentabile".into())
            })?;
            let position = quantile * last;
            let lower = position
                .floor()
                .to_usize()
                .ok_or_else(|| PlenoraError::InvalidPlan("indice quantile non valido".into()))?;
            let upper = position
                .ceil()
                .to_usize()
                .ok_or_else(|| PlenoraError::InvalidPlan("indice quantile non valido".into()))?;
            let weight = position - position.floor();
            // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
            // violerebbe il determinismo bit-esatto (architettura.md#determinismo); la forma non
            // fusa e' il contratto numerico. Produzione e oracolo usano la
            // STESSA forma: l'equivalenza bit-a-bit resta per costruzione.
            #[allow(clippy::suboptimal_flops)]
            let interpolated = (values[upper] - values[lower]) * weight + values[lower];
            interpolated
        }
        // Il dispatch di `aggregate` instrada a `reduce_numeric` solo
        // Sum/Avg/Mean/Min/Max/Variance/Stddev/Quantile; le altre funzioni
        // hanno percorsi dedicati. Il compilatore non puo' dimostrarlo:
        // invariante interna, errore esplicito (R6).
        _ => {
            return Err(PlenoraError::Internal(
                "funzione fuori dal percorso numerico di reduce_numeric".into(),
            ));
        }
    }))
}

#[allow(clippy::too_many_lines)] // Aggregation variants share one grouping pass and its invariants.
/// Batch aggregato per `group_by` con le aggregazioni di
/// `config.aggregations` (default: solo conteggio per gruppo).
///
/// # Errors
///
/// - `InvalidPlan`: `group_by` vuoto; funzione `quantile` senza il parametro
///   `quantile` o con valore fuori `[0, 1]`; indice di quantile non valido;
///   nome di output non valido (come `validate_output_name`);
/// - `ResourceLimit`: conteggi e dimensioni di gruppo non rappresentabili
///   (`i64`/`f64`): crescono col numero di righe del gruppo;
/// - `Schema`: una colonna di `group_by` o delle aggregazioni assente dallo
///   schema; in piu' gli errori di `scalar_as_string`/`scalar_as_f64_rounded`
///   (tipi fuori dal fast path), `select_rows` e `replace_or_append`.
///
/// Un intero oltre 2^53 **non** e' un errore nelle aggregazioni numeriche —
/// quelle il cui risultato e' un `Float64` per contratto: li' la conversione
/// arrotonda
/// (errori-e-limiti.md#arrotondamento-nelle-operazioni-a-risultato-float64).
/// Le altre aggregazioni non attraversano quella conversione e rendono il
/// proprio tipo.
pub fn aggregate(batch: &RecordBatch, config: &Aggregate) -> Result<RecordBatch> {
    let group_indices = config
        .group_by
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    if group_indices.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "aggregate richiede group_by".into(),
        ));
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
            return Err(PlenoraError::InvalidPlan(
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
    let righe_gruppi = group_columns
        .first()
        .map_or(0, plenora_core::arrow::array::Array::len);
    let mut result = crate::batch_with_rows(
        Arc::new(plenora_core::arrow::schema::Schema::new(group_fields)),
        group_columns,
        righe_gruppi,
    )?;
    if config.aggregations.is_empty() {
        let counts = groups
            .iter()
            .map(|rows| conteggio_gruppo(rows.len()))
            .collect::<Result<Vec<_>>>()?;
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
                    // `count` conta i valori, non le chiavi: una chiave
                    // dictionary valida che punta a una entry nulla e' una
                    // riga senza valore e non va contata.
                    let count = rows
                        .iter()
                        .filter(|row| !crate::is_logically_null(column.as_ref(), **row))
                        .count();
                    i64::try_from(count).map(Some).map_err(|_| {
                        PlenoraError::ResourceLimit("conteggio gruppo oltre i64".into())
                    })
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
                    i64::try_from(count).map(Some).map_err(|_| {
                        PlenoraError::ResourceLimit("conteggio gruppo oltre i64".into())
                    })
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
