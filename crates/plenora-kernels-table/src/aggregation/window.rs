use std::cmp::Ordering;
use std::sync::Arc;

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{Float64Array, RecordBatch};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};

use crate::{column_index, replace_or_append};

use super::aggregate::default_ddof;
use super::grouping::{build_partitions, scatter_partitions, NumericSource};
use super::sort::{sort, Sort};

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
/// - `InvalidPlan`: `window` o `min_periods` nulli, `min_periods > window`;
/// - `ResourceLimit`: dimensioni/divisori della finestra non rappresentabili
///   come `f64` (dipendono dal numero di righe nella finestra);
/// - `Schema`: colonna `column`, `group_by` o `order_column` assente dallo
///   schema; valore intero non rappresentabile come `f64`; in piu' gli
///   errori di `sort`, `scalar_as_string`/`scalar_as_f64` e
///   `replace_or_append`.
pub fn rolling_window(batch: &RecordBatch, config: &RollingWindow) -> Result<RecordBatch> {
    if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
        return Err(PlenoraError::InvalidPlan(
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
                        PlenoraError::ResourceLimit("dimensione rolling non rappresentabile".into())
                    })?;
                    let mean = sum / length;
                    let divisor = (count - config.ddof).to_f64().ok_or_else(|| {
                        PlenoraError::ResourceLimit("divisore rolling non rappresentabile".into())
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
/// - `InvalidPlan`: `offset` nullo; `ntile` senza `buckets` maggiore di zero;
///   `buckets` specificato per una funzione diversa da `ntile`;
/// - `Schema`: colonna `column`, `group_by` o `order_column` assente dallo
///   schema; valore intero non rappresentabile come `f64`; in piu' gli
///   errori di `sort`, `scalar_as_string`/`scalar_as_f64` e
///   `replace_or_append`.
pub fn window_function(batch: &RecordBatch, config: &WindowFunction) -> Result<RecordBatch> {
    if config.offset == 0 {
        return Err(PlenoraError::InvalidPlan(
            "offset deve essere positivo".into(),
        ));
    }
    if matches!(config.function, WindowKind::Ntile) {
        if config.buckets.is_none_or(|buckets| buckets == 0) {
            return Err(PlenoraError::InvalidPlan(
                "ntile richiede buckets maggiore di zero".into(),
            ));
        }
    } else if config.buckets.is_some() {
        return Err(PlenoraError::InvalidPlan(
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
            WindowKind::Rank
                | WindowKind::DenseRank
                | WindowKind::PercentRank
                | WindowKind::CumeDist
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
