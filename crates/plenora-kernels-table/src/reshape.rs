use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use plenora_core::arrow::array::{
    builder::StringBuilder, Array, ArrayRef, BooleanArray, Float64Array, Int64Array, ListArray,
    RecordBatch, StringArray, StructArray, UInt32Array, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use num_traits::ToPrimitive;
use serde::Deserialize;

use crate::Limits;
use plenora_core::{PlenoraError, Result};
use crate::{
    column_index, replace_or_append, scalar_as_f64, scalar_as_string, select_rows,
    validate_output_name,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Melt {
    pub id_columns: Vec<String>,
    #[serde(default)]
    pub value_columns: Vec<String>,
    #[serde(default = "default_variable")]
    pub var_name: String,
    #[serde(default = "default_value")]
    pub value_name: String,
    #[serde(default = "default_type_policy")]
    pub type_policy: HeterogeneousTypePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeterogeneousTypePolicy {
    Reject,
    String,
}

const fn default_type_policy() -> HeterogeneousTypePolicy {
    HeterogeneousTypePolicy::Reject
}
fn default_variable() -> String {
    "variable".into()
}
fn default_value() -> String {
    "value".into()
}

// ---------------------------------------------------------------------------
// Fast path di `melt`/`pivot` (ultimo batch ottimizzazioni kernel).
//
// `TextColumn`/`NumericColumn` preparano una sola volta il downcast Arrow
// per colonna e iterano sui valori nativi, producendo gli STESSI byte dei
// percorsi scalari originali (`scalar_as_string` / `scalar_as_f64`):
// stesso formato Display per numerici e booleani (NaN -> "NaN",
// -0.0 -> "-0" distinto da "0"), stessi null, stessi errori. I tipi fuori
// dal fast path ricadono sul percorso generico, invariato.
// ---------------------------------------------------------------------------

/// Sorgente testuale tipizzata per `melt` (policy string) e `pivot`
/// (chiavi composte, valori pivot, concat).
enum TextColumn<'a> {
    Utf8(&'a StringArray),
    Int64(&'a Int64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    UInt64(&'a UInt64Array),
    Generic(&'a ArrayRef),
}

impl<'a> TextColumn<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Self::Boolean(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64(values);
        }
        Self::Generic(array)
    }

    /// Scrive in `out` gli stessi byte di `scalar_as_string` per `row`;
    /// restituisce `false` (senza scrivere) se il valore e' null.
    fn write_value(&self, row: usize, out: &mut String) -> Result<bool> {
        match self {
            Self::Utf8(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                out.push_str(values.value(row));
            }
            Self::Int64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                write!(out, "{}", values.value(row)).expect("fmt su String");
            }
            Self::Float64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                write!(out, "{}", values.value(row)).expect("fmt su String");
            }
            Self::Boolean(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                write!(out, "{}", values.value(row)).expect("fmt su String");
            }
            Self::UInt64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                write!(out, "{}", values.value(row)).expect("fmt su String");
            }
            Self::Generic(array) => {
                let Some(value) = scalar_as_string(array.as_ref(), row)? else {
                    return Ok(false);
                };
                out.push_str(&value);
            }
        }
        Ok(true)
    }
}

/// Sorgente numerica tipizzata per le aggregazioni Float64 di `pivot`.
enum NumericColumn<'a> {
    Float64(&'a Float64Array),
    Int64(&'a Int64Array),
    UInt64(&'a UInt64Array),
    Generic(&'a ArrayRef),
}

impl<'a> NumericColumn<'a> {
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

    /// Stesso valore di `scalar_as_f64` (null inclusi), senza downcast per riga.
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

/// Colonna di chiave composta (usata da `pivot` e `table_diff`): tag di
/// tipo precomputato una sola volta + valore tipizzato. Scrive gli STESSI
/// byte di `composite_key` (mantenuta come oracolo dei test).
struct PivotKeyColumn<'a> {
    tag: String,
    source: TextColumn<'a>,
}

impl<'a> PivotKeyColumn<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        Self {
            tag: array.data_type().to_string(),
            source: TextColumn::new(array),
        }
    }

    fn write_key(&self, row: usize, key: &mut String, value: &mut String) -> Result<()> {
        key.push_str(&self.tag);
        key.push('\u{1e}');
        value.clear();
        if self.source.write_value(row, value)? {
            key.push('1');
            write!(key, "{}", value.len()).expect("fmt su String");
            key.push(':');
            key.push_str(value);
        } else {
            key.push('0');
        }
        key.push('\u{1f}');
        Ok(())
    }
}

pub fn melt(batch: &RecordBatch, config: &Melt, limits: &Limits) -> Result<RecordBatch> {
    let id_indices = config
        .id_columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let value_indices = if config.value_columns.is_empty() {
        (0..batch.num_columns())
            .filter(|index| !id_indices.contains(index))
            .collect::<Vec<_>>()
    } else {
        config
            .value_columns
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?
    };
    if value_indices.is_empty() {
        return Err(PlenoraError::Contract("melt senza value_columns".into()));
    }
    let output_rows = batch
        .num_rows()
        .checked_mul(value_indices.len())
        .ok_or_else(|| PlenoraError::Contract("overflow righe melt".into()))?;
    if output_rows > limits.max_rows {
        return Err(PlenoraError::Contract("melt supera max_rows".into()));
    }
    // Fast path: indici di ripetizione materializzati una sola volta come
    // UInt32 e `take` SOLO sulle colonne id (il percorso originale
    // replicava via `select_rows` l'intero batch, incluse le value_columns
    // poi scartate). Stesso controllo di overflow di `select_rows`.
    let row_count = u32::try_from(batch.num_rows())
        .map_err(|_| PlenoraError::Contract("indice riga oltre u32".into()))?;
    let mut repeated_indices = Vec::with_capacity(output_rows);
    for _ in 0..value_indices.len() {
        repeated_indices.extend(0..row_count);
    }
    let repeated_indices = UInt32Array::from(repeated_indices);
    let mut fields = id_indices
        .iter()
        .map(|index| batch.schema().field(*index).as_ref().clone())
        .collect::<Vec<_>>();
    let mut columns = id_indices
        .iter()
        .map(|index| {
            plenora_core::arrow::select::take::take(
                batch.column(*index).as_ref(),
                &repeated_indices,
                None,
            )
            .map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    let var_name = collision_free(&config.var_name, batch)?;
    let value_name = collision_free(&config.value_name, batch)?;
    fields.push(Field::new(&var_name, DataType::Utf8, false));
    // Fast path: nomi di colonna presi in prestito dallo schema, senza un
    // clone di String per riga.
    let schema = batch.schema();
    let mut variables: Vec<&str> = Vec::with_capacity(output_rows);
    for index in &value_indices {
        variables.extend(std::iter::repeat_n(
            schema.field(*index).name().as_str(),
            batch.num_rows(),
        ));
    }
    columns.push(Arc::new(StringArray::from(variables)));
    let value_type = batch.column(value_indices[0]).data_type().clone();
    let homogeneous = value_indices
        .iter()
        .all(|index| batch.column(*index).data_type() == &value_type);
    if homogeneous {
        let arrays = value_indices
            .iter()
            .map(|index| batch.column(*index).as_ref())
            .collect::<Vec<_>>();
        fields.push(Field::new(&value_name, value_type, true));
        columns.push(plenora_core::arrow::select::concat::concat(&arrays)?);
    } else if matches!(config.type_policy, HeterogeneousTypePolicy::String) {
        // Fast path: downcast una volta per colonna e loop tipizzato sulle
        // righe; stessi byte, stessi null, stesso ordine di scansione e
        // stesso controllo max_string_bytes del percorso scalare originale.
        let mut builder = StringBuilder::with_capacity(
            output_rows,
            output_rows.saturating_mul(8).min(64 * 1024 * 1024),
        );
        let mut text = String::new();
        for index in &value_indices {
            let source = TextColumn::new(batch.column(*index));
            for row in 0..batch.num_rows() {
                text.clear();
                if source.write_value(row, &mut text)? {
                    if text.len() > limits.max_string_bytes {
                        return Err(PlenoraError::Contract(
                            "melt: valore testuale oltre max_string_bytes".into(),
                        ));
                    }
                    builder.append_value(&text);
                } else {
                    builder.append_null();
                }
            }
        }
        fields.push(Field::new(&value_name, DataType::Utf8, true));
        columns.push(Arc::new(builder.finish()));
    } else {
        return Err(PlenoraError::Contract(
            "melt: value_columns eterogenee; impostare type_policy='string' per la conversione esplicita".into(),
        ));
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

fn collision_free(name: &str, batch: &RecordBatch) -> Result<String> {
    validate_output_name(name)?;
    if batch.schema().index_of(name).is_err() {
        return Ok(name.into());
    }
    (1..100)
        .map(|index| format!("{name}_{index}"))
        .find(|candidate| batch.schema().index_of(candidate).is_err())
        .ok_or_else(|| PlenoraError::Contract(format!("impossibile evitare collisione {name}")))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PivotAgg {
    First,
    Last,
    Max,
    Min,
    Sum,
    Mean,
    Count,
    Concat,
}
const fn default_pivot_agg() -> PivotAgg {
    PivotAgg::First
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pivot {
    pub index_col: String,
    #[serde(rename = "pivot_col")]
    pub column: String,
    pub value_col: String,
    #[serde(default = "default_pivot_agg")]
    pub aggr_func: PivotAgg,
    #[serde(default)]
    pub mapping: BTreeMap<String, String>,
}

fn pivot_column(
    source: &ArrayRef,
    groups: &[Option<&Vec<usize>>],
    function: &PivotAgg,
) -> Result<(DataType, ArrayRef)> {
    Ok(match function {
        PivotAgg::First | PivotAgg::Last => {
            let indices = groups
                .iter()
                .map(|rows| {
                    rows.and_then(|rows| {
                        if matches!(function, PivotAgg::First) {
                            rows.first()
                        } else {
                            rows.last()
                        }
                    })
                    .copied()
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| PlenoraError::Contract("indice pivot oltre u32".into()))
                })
                .collect::<Result<Vec<_>>>()?;
            (
                source.data_type().clone(),
                plenora_core::arrow::select::take::take(source.as_ref(), &UInt32Array::from(indices), None)?,
            )
        }
        PivotAgg::Count => {
            let values = groups
                .iter()
                .map(|rows| {
                    rows.map(|rows| {
                        i64::try_from(rows.iter().filter(|row| !source.is_null(**row)).count())
                            .map_err(|_| PlenoraError::Contract("conteggio pivot oltre i64".into()))
                    })
                    .transpose()
                })
                .collect::<Result<Vec<_>>>()?;
            (DataType::Int64, Arc::new(Int64Array::from(values)))
        }
        PivotAgg::Concat => {
            // Fast path: downcast una volta per colonna valore; stessi byte
            // (valori uniti da ",", null saltati) del percorso scalare.
            let text = TextColumn::new(source);
            let mut value = String::new();
            let mut joined = String::new();
            let values = groups
                .iter()
                .map(|rows| {
                    rows.map(|rows| {
                        joined.clear();
                        let mut first = true;
                        for row in rows {
                            value.clear();
                            if text.write_value(*row, &mut value)? {
                                if first {
                                    first = false;
                                } else {
                                    joined.push(',');
                                }
                                joined.push_str(&value);
                            }
                        }
                        Ok(joined.clone())
                    })
                    .transpose()
                })
                .collect::<Result<Vec<_>>>()?;
            (DataType::Utf8, Arc::new(StringArray::from(values)))
        }
        PivotAgg::Sum | PivotAgg::Mean | PivotAgg::Min | PivotAgg::Max => {
            // Fast path: riduzione in streaming sulle righe del gruppo nello
            // STESSO ordine del Vec<f64> originale. Il seme della somma e'
            // -0.0, identico a `Iterator::sum` per f64 (fold(-0.0, +)):
            // cosi' anche i segni di zero restano bit-identici
            // ([-0.0] -> -0.0, [+0.0] -> +0.0). f64::min/f64::max
            // passo-passo danno gli stessi bit di reduce, NaN incluso.
            let numeric = NumericColumn::new(source);
            let values = groups
                .iter()
                .map(|rows| {
                    let Some(rows) = rows else { return Ok(None) };
                    let mut sum = -0.0_f64;
                    let mut extremum = 0.0_f64;
                    let mut count = 0_usize;
                    for row in rows.iter() {
                        let Some(value) = numeric.value(*row)? else { continue };
                        if count == 0 {
                            extremum = value;
                        } else if matches!(function, PivotAgg::Min) {
                            extremum = f64::min(extremum, value);
                        } else {
                            extremum = f64::max(extremum, value);
                        }
                        sum += value;
                        count += 1;
                    }
                    if count == 0 {
                        return Ok(None);
                    }
                    Ok(Some(match function {
                        PivotAgg::Sum => sum,
                        PivotAgg::Mean => sum / count.to_f64().ok_or_else(|| {
                            PlenoraError::Contract("gruppo pivot non rappresentabile".into())
                        })?,
                        PivotAgg::Min | PivotAgg::Max => extremum,
                        _ => unreachable!(),
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            (DataType::Float64, Arc::new(Float64Array::from(values)))
        }
    })
}

pub fn pivot(batch: &RecordBatch, config: &Pivot, limits: &Limits) -> Result<RecordBatch> {
    let index_names = config
        .index_col
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    let index_indices = index_names
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let pivot_index = column_index(batch, &config.column)?;
    let value_index = column_index(batch, &config.value_col)?;
    // Fast path (ultimo batch ottimizzazioni kernel): UNA passata sulle
    // righe (il percorso originale ne faceva due, ricalcolando le chiavi
    // composte riga per riga), con chiavi scritte in un buffer riusato e
    // gruppi di celle indicizzati dagli interi (chiave, pivot) invece che da
    // stringhe formattate ad ogni lookup. Ordini di output identici:
    // chiavi e pivot ordinati lessicograficamente come nel BTreeMap
    // originale, righe di gruppo in ordine crescente, rappresentante =
    // prima riga incontrata per chiave.
    let key_columns = index_indices
        .iter()
        .map(|index| PivotKeyColumn::new(batch.column(*index)))
        .collect::<Vec<_>>();
    let pivot_source = TextColumn::new(batch.column(pivot_index));
    let mut key = String::new();
    let mut scratch = String::new();
    let mut key_ids: HashMap<String, usize> = HashMap::new();
    let mut representatives: Vec<usize> = Vec::new();
    let mut pivot_ids: HashMap<String, usize> = HashMap::new();
    let mut cells: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for row in 0..batch.num_rows() {
        key.clear();
        for column in &key_columns {
            column.write_key(row, &mut key, &mut scratch)?;
        }
        let key_id = match key_ids.get(key.as_str()) {
            Some(id) => *id,
            None => {
                let id = representatives.len();
                key_ids.insert(key.clone(), id);
                representatives.push(row);
                id
            }
        };
        scratch.clear();
        if !pivot_source.write_value(row, &mut scratch)? {
            continue;
        }
        if config.mapping.is_empty() || config.mapping.contains_key(scratch.as_str()) {
            let pivot_id = match pivot_ids.get(scratch.as_str()) {
                Some(id) => *id,
                None => {
                    let id = pivot_ids.len();
                    pivot_ids.insert(scratch.clone(), id);
                    id
                }
            };
            cells.entry((key_id, pivot_id)).or_default().push(row);
        }
    }
    let mut sorted_keys = key_ids.into_iter().collect::<Vec<_>>();
    sorted_keys.sort_by(|left, right| left.0.cmp(&right.0));
    let mut sorted_pivots = pivot_ids.into_iter().collect::<Vec<_>>();
    sorted_pivots.sort_by(|left, right| left.0.cmp(&right.0));
    if sorted_keys.len() > limits.max_rows
        || index_indices.len().saturating_add(sorted_pivots.len()) > limits.max_columns
    {
        return Err(PlenoraError::Contract(
            "pivot supera i limiti di output".into(),
        ));
    }
    // Fast path: `take` SOLO sulle colonne indice (il percorso originale
    // replicava l'intero batch). Stesso controllo di overflow di
    // `select_rows`.
    let representative_indices = sorted_keys
        .iter()
        .map(|(_, id)| {
            u32::try_from(representatives[*id])
                .map_err(|_| PlenoraError::Contract("indice riga oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let representative_indices = UInt32Array::from(representative_indices);
    let mut fields = index_indices
        .iter()
        .map(|index| batch.schema().field(*index).as_ref().clone())
        .collect::<Vec<_>>();
    let mut columns = index_indices
        .iter()
        .map(|index| {
            plenora_core::arrow::select::take::take(
                batch.column(*index).as_ref(),
                &representative_indices,
                None,
            )
            .map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    let value_source = batch.column(value_index);
    for (pivot_value, pivot_id) in &sorted_pivots {
        let output = config
            .mapping
            .get(pivot_value)
            .cloned()
            .unwrap_or_else(|| pivot_value.clone());
        validate_output_name(&output)?;
        let grouped_rows = sorted_keys
            .iter()
            .map(|(_, key_id)| cells.get(&(*key_id, *pivot_id)))
            .collect::<Vec<_>>();
        let (data_type, values) = pivot_column(value_source, &grouped_rows, &config.aggr_func)?;
        fields.push(Field::new(&output, data_type, true));
        columns.push(values);
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transpose {
    pub id_column: Option<String>,
    #[serde(default)]
    pub output_columns: Vec<String>,
    #[serde(default = "default_type_policy")]
    pub type_policy: HeterogeneousTypePolicy,
}

pub fn transpose(
    batch: &RecordBatch,
    config: &Transpose,
    limits: &Limits,
) -> Result<RecordBatch> {
    if batch.num_rows() == 0 {
        return Ok(batch.clone());
    }
    let id_index = config
        .id_column
        .as_deref()
        .map(|name| column_index(batch, name))
        .transpose()?;
    let data_indices = (0..batch.num_columns())
        .filter(|index| Some(*index) != id_index)
        .collect::<Vec<_>>();
    let output_columns = batch.num_rows().saturating_add(1);
    if data_indices.len() > limits.max_rows || output_columns > limits.max_columns {
        return Err(PlenoraError::Contract("transpose supera i limiti".into()));
    }
    let first_name = config.id_column.clone().unwrap_or_else(|| "col_0".into());
    let mut fields = vec![Field::new(&first_name, DataType::Utf8, false)];
    let mut columns: Vec<Arc<dyn plenora_core::arrow::array::Array>> = vec![Arc::new(StringArray::from(
        data_indices
            .iter()
            .map(|index| batch.schema().field(*index).name().clone())
            .collect::<Vec<_>>(),
    ))];
    let data_type = data_indices.first().map_or(DataType::Utf8, |index| {
        batch.column(*index).data_type().clone()
    });
    let homogeneous = data_indices
        .iter()
        .all(|index| batch.column(*index).data_type() == &data_type);
    if !homogeneous && matches!(config.type_policy, HeterogeneousTypePolicy::Reject) {
        return Err(PlenoraError::Contract(
            "transpose: colonne eterogenee; impostare type_policy='string' per la conversione esplicita".into(),
        ));
    }
    let combined = if homogeneous && !data_indices.is_empty() {
        let arrays = data_indices
            .iter()
            .map(|index| batch.column(*index).as_ref())
            .collect::<Vec<_>>();
        Some(plenora_core::arrow::select::concat::concat(&arrays)?)
    } else {
        None
    };
    for row in 0..batch.num_rows() {
        let default_name = id_index
            .map(|index| scalar_as_string(batch.column(index).as_ref(), row))
            .transpose()?
            .flatten()
            .unwrap_or_else(|| format!("col_{}", row + 1));
        let name = config
            .output_columns
            .get(row)
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or(default_name);
        validate_output_name(&name)?;
        if let Some(combined) = &combined {
            fields.push(Field::new(&name, data_type.clone(), true));
            let indices = data_indices
                .iter()
                .enumerate()
                .map(|(position, _)| {
                    position
                        .checked_mul(batch.num_rows())
                        .and_then(|base| base.checked_add(row))
                        .and_then(|index| u64::try_from(index).ok())
                        .map(Some)
                        .ok_or_else(|| PlenoraError::Contract("indice transpose oltre u64".into()))
                })
                .collect::<Result<Vec<_>>>()?;
            columns.push(plenora_core::arrow::select::take::take(
                combined.as_ref(),
                &UInt64Array::from(indices),
                None,
            )?);
        } else {
            fields.push(Field::new(&name, DataType::Utf8, true));
            let values = data_indices
                .iter()
                .map(|index| scalar_as_string(batch.column(*index).as_ref(), row))
                .collect::<Result<Vec<_>>>()?;
            if values
                .iter()
                .flatten()
                .any(|value| value.len() > limits.max_string_bytes)
            {
                return Err(PlenoraError::Contract(
                    "transpose: valore testuale oltre max_string_bytes".into(),
                ));
            }
            columns.push(Arc::new(StringArray::from(values)));
        }
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmptyListPolicy {
    Drop,
    Null,
}

const fn default_empty_list_policy() -> EmptyListPolicy {
    EmptyListPolicy::Null
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Explode {
    pub column: String,
    pub output_column: Option<String>,
    #[serde(default = "default_empty_list_policy")]
    pub empty_policy: EmptyListPolicy,
}

pub fn explode(
    batch: &RecordBatch,
    config: &Explode,
    limits: &Limits,
) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let list = batch
        .column(index)
        .as_any()
        .downcast_ref::<ListArray>()
        .ok_or_else(|| PlenoraError::Schema("explode richiede una colonna List".into()))?;
    let output_name = config.output_column.as_deref().unwrap_or(&config.column);
    validate_output_name(output_name)?;
    let offsets = list.value_offsets();
    let mut rows = Vec::new();
    let mut values = Vec::new();
    for row in 0..batch.num_rows() {
        let start = usize::try_from(offsets[row])
            .map_err(|_| PlenoraError::Schema("offset List negativo".into()))?;
        let end = usize::try_from(offsets[row + 1])
            .map_err(|_| PlenoraError::Schema("offset List negativo".into()))?;
        if list.is_null(row) || start == end {
            if matches!(config.empty_policy, EmptyListPolicy::Null) {
                rows.push(row);
                values.push(None);
            }
        } else {
            for value in start..end {
                rows.push(row);
                values.push(Some(u32::try_from(value).map_err(|_| {
                    PlenoraError::Contract("indice explode oltre u32".into())
                })?));
            }
        }
        if rows.len() > limits.max_rows {
            return Err(PlenoraError::Contract("explode supera max_rows".into()));
        }
    }
    let repeated = select_rows(batch, &rows)?;
    let output =
        plenora_core::arrow::select::take::take(list.values().as_ref(), &UInt32Array::from(values), None)?;
    replace_or_append(&repeated, output_name, list.value_type(), true, output)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unnest {
    pub column: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default = "default_true")]
    pub drop_source: bool,
}

const fn default_true() -> bool {
    true
}

pub fn unnest(batch: &RecordBatch, config: &Unnest, limits: &Limits) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let structure = batch
        .column(index)
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| PlenoraError::Schema("unnest richiede una colonna Struct".into()))?;
    let projected_columns = batch
        .num_columns()
        .saturating_sub(usize::from(config.drop_source))
        .saturating_add(structure.num_columns());
    if projected_columns > limits.max_columns {
        return Err(PlenoraError::Contract("unnest supera max_columns".into()));
    }
    let mut fields = Vec::with_capacity(projected_columns);
    let mut columns = Vec::with_capacity(projected_columns);
    let mut names = std::collections::HashSet::new();
    for (position, field) in batch.schema().fields().iter().enumerate() {
        if position == index && config.drop_source {
            continue;
        }
        names.insert(field.name().clone());
        fields.push(field.as_ref().clone());
        columns.push(batch.column(position).clone());
    }
    let parent_indices = UInt32Array::from(
        (0..batch.num_rows())
            .map(|row| {
                (!structure.is_null(row))
                    .then(|| u32::try_from(row).ok())
                    .flatten()
            })
            .collect::<Vec<_>>(),
    );
    for (child, field) in structure.columns().iter().zip(structure.fields()) {
        let name = format!("{}{}", config.prefix, field.name());
        validate_output_name(&name)?;
        if !names.insert(name.clone()) {
            return Err(PlenoraError::Schema(format!(
                "unnest: collisione colonna {name}"
            )));
        }
        fields.push(field.as_ref().clone().with_name(name).with_nullable(true));
        columns.push(plenora_core::arrow::select::take::take(
            child.as_ref(),
            &parent_indices,
            None,
        )?);
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        )),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableDiff {
    pub left_keys: Vec<String>,
    pub right_keys: Vec<String>,
    #[serde(default)]
    pub compare_columns: Vec<String>,
    #[serde(default = "default_no")]
    pub include_unchanged: String,
    #[serde(default = "default_separator")]
    pub separator: String,
}

struct DiffRow {
    old_row: Option<usize>,
    new_row: Option<usize>,
    status: String,
    changed: Option<String>,
    old_values: Option<String>,
}
fn default_no() -> String {
    "no".into()
}
fn default_separator() -> String {
    "#".into()
}

/// Codifica la chiave composta di `table_diff` per `row` in `key` (buffer
/// riusato tra le righe), con gli stessi byte di `composite_key`.
fn encode_diff_key(
    columns: &[PivotKeyColumn],
    row: usize,
    key: &mut String,
    value: &mut String,
) -> Result<()> {
    key.clear();
    for column in columns {
        column.write_key(row, key, value)?;
    }
    Ok(())
}

/// Hasher moltiplicativo a blocchi (stile `FxHash`) con finalizer splitmix64,
/// come nei fast path di `aggregate` e `join`: `SipHash` (default std)
/// dominerebbe il costo di build/probe su milioni di righe. Le mappe di
/// `table_diff` non sono mai iterate: semantica invariata.
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
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

type DiffKeyMap = HashMap<String, usize, BuildHasherDefault<KeyHasher>>;

/// Implementazione originale (pre-ottimizzazione) della chiave composta:
/// resta come oracolo dei test di equivalenza di `table_diff` e `pivot`.
#[cfg(test)]
fn composite_key(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<String> {
    let mut key = String::new();
    for index in indices {
        key.push_str(batch.column(*index).data_type().to_string().as_str());
        key.push('\u{1e}');
        match scalar_as_string(batch.column(*index).as_ref(), row)? {
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

fn diff_values(left: &ArrayRef, right: &ArrayRef, rows: &[DiffRow]) -> Result<ArrayRef> {
    if left.data_type() != right.data_type() {
        return Err(PlenoraError::Schema(format!(
            "table_diff richiede tipi Arrow identici, trovati {} e {}",
            left.data_type(),
            right.data_type()
        )));
    }
    let combined = plenora_core::arrow::select::concat::concat(&[left.as_ref(), right.as_ref()])?;
    let indices = rows
        .iter()
        .map(|row| {
            let index = if let Some(new_row) = row.new_row {
                left.len().saturating_add(new_row)
            } else {
                row.old_row
                    .ok_or_else(|| PlenoraError::Contract("riga table_diff senza sorgente".into()))?
            };
            u32::try_from(index)
                .map(Some)
                .map_err(|_| PlenoraError::Contract("indice table_diff oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(plenora_core::arrow::select::take::take(
        combined.as_ref(),
        &UInt32Array::from(indices),
        None,
    )?)
}

#[allow(clippy::too_many_lines)] // Diff phases remain adjacent to preserve auditability.
pub fn table_diff(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &TableDiff,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
        return Err(PlenoraError::Contract("chiavi table_diff non valide".into()));
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
    let compare = if config.compare_columns.is_empty() {
        left.schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .filter(|name| {
                !config.left_keys.contains(name) && right.schema().index_of(name).is_ok()
            })
            .collect::<Vec<_>>()
    } else {
        config.compare_columns.clone()
    };
    let left_compare = compare
        .iter()
        .map(|name| column_index(left, name))
        .collect::<Result<Vec<_>>>()?;
    let right_compare = compare
        .iter()
        .map(|name| column_index(right, name))
        .collect::<Result<Vec<_>>>()?;
    // Fast path (batch 4 ottimizzazioni kernel): chiavi codificate una sola
    // volta per colonna (`PivotKeyColumn`, stessi byte di `composite_key`) in
    // un buffer riusato; mappe hash con hasher FxHash-style (mai iterate,
    // solo lookup/insert, quindi equivalenti alle BTreeMap originali);
    // confronto valori via `TextColumn` senza `scalar_as_string` per cella.
    // Stesso ordine righe in output (sorgente sinistra, poi righe solo a
    // destra), stessi errori.
    let left_key_columns = left_keys
        .iter()
        .map(|index| PivotKeyColumn::new(left.column(*index)))
        .collect::<Vec<_>>();
    let right_key_columns = right_keys
        .iter()
        .map(|index| PivotKeyColumn::new(right.column(*index)))
        .collect::<Vec<_>>();
    let left_text_columns = left_compare
        .iter()
        .map(|index| TextColumn::new(left.column(*index)))
        .collect::<Vec<_>>();
    let right_text_columns = right_compare
        .iter()
        .map(|index| TextColumn::new(right.column(*index)))
        .collect::<Vec<_>>();
    let mut key = String::new();
    let mut text = String::new();
    let mut old = DiffKeyMap::with_capacity_and_hasher(
        left.num_rows(),
        BuildHasherDefault::default(),
    );
    let mut new = DiffKeyMap::with_capacity_and_hasher(
        right.num_rows(),
        BuildHasherDefault::default(),
    );
    for row in 0..left.num_rows() {
        encode_diff_key(&left_key_columns, row, &mut key, &mut text)?;
        if old.insert(key.clone(), row).is_some() {
            return Err(PlenoraError::Contract(
                "chiavi duplicate nella tabella sinistra".into(),
            ));
        }
    }
    for row in 0..right.num_rows() {
        encode_diff_key(&right_key_columns, row, &mut key, &mut text)?;
        if new.insert(key.clone(), row).is_some() {
            return Err(PlenoraError::Contract(
                "chiavi duplicate nella tabella destra".into(),
            ));
        }
    }
    // Preserve source order: old rows first, then new-only rows. Sorting the
    // encoded key would place nulls first and reorder otherwise stable data.
    let mut matched = Vec::with_capacity(old.len().saturating_add(new.len()));
    for row in 0..left.num_rows() {
        encode_diff_key(&left_key_columns, row, &mut key, &mut text)?;
        matched.push((Some(row), new.get(key.as_str()).copied()));
    }
    for row in 0..right.num_rows() {
        encode_diff_key(&right_key_columns, row, &mut key, &mut text)?;
        if !old.contains_key(key.as_str()) {
            matched.push((None, Some(row)));
        }
    }
    let mut rows = Vec::new();
    let mut before = String::new();
    let mut after = String::new();
    for (old_row, new_row) in matched {
        let (status, changed, old_values) = match (old_row, new_row) {
            (None, Some(_)) => ("ADDED".to_owned(), None, None),
            (Some(_), None) => ("DELETED".to_owned(), None, None),
            (Some(old_row), Some(new_row)) => {
                let mut changed = Vec::new();
                let mut old_values = Vec::new();
                for ((name, left_column), right_column) in
                    compare.iter().zip(&left_text_columns).zip(&right_text_columns)
                {
                    before.clear();
                    after.clear();
                    let has_before = left_column.write_value(old_row, &mut before)?;
                    let has_after = right_column.write_value(new_row, &mut after)?;
                    if has_before != has_after || (has_before && before != after) {
                        changed.push(name.clone());
                        old_values.push(before.clone());
                    }
                }
                if changed.is_empty() {
                    ("UNCHANGED".to_owned(), None, None)
                } else {
                    (
                        "MODIFIED".to_owned(),
                        Some(changed.join(&config.separator)),
                        Some(old_values.join(&config.separator)),
                    )
                }
            }
            (None, None) => unreachable!(),
        };
        if status != "UNCHANGED" || config.include_unchanged == "yes" {
            rows.push(DiffRow {
                old_row,
                new_row,
                status,
                changed,
                old_values,
            });
        }
    }
    if rows.len() > limits.max_rows {
        return Err(PlenoraError::Contract("table_diff supera max_rows".into()));
    }
    let output_count = config
        .right_keys
        .len()
        .saturating_add(compare.len())
        .saturating_add(3);
    if output_count > limits.max_columns {
        return Err(PlenoraError::Contract(
            "table_diff supera max_columns".into(),
        ));
    }
    let mut fields = Vec::new();
    let mut columns: Vec<Arc<dyn plenora_core::arrow::array::Array>> = Vec::new();
    for (position, name) in config.left_keys.iter().enumerate() {
        let left_column = left.column(left_keys[position]);
        let right_column = right.column(right_keys[position]);
        fields.push(
            left.schema()
                .field(left_keys[position])
                .as_ref()
                .clone()
                .with_name(name)
                .with_nullable(true),
        );
        columns.push(diff_values(left_column, right_column, &rows)?);
    }
    for (position, name) in compare.iter().enumerate() {
        let left_column = left.column(left_compare[position]);
        let right_column = right.column(right_compare[position]);
        fields.push(
            right
                .schema()
                .field(right_compare[position])
                .as_ref()
                .clone()
                .with_name(name)
                .with_nullable(true),
        );
        columns.push(diff_values(left_column, right_column, &rows)?);
    }
    for (name, selector) in [
        ("_diff_status", 0_usize),
        ("_diff_columns", 1),
        ("_diff_old_values", 2),
    ] {
        fields.push(Field::new(
            name,
            DataType::Utf8,
            selector == 1 || selector == 2,
        ));
        columns.push(Arc::new(StringArray::from(
            rows.iter()
                .map(|row| match selector {
                    0 => Some(row.status.clone()),
                    1 => row.changed.clone(),
                    _ => row.old_values.clone(),
                })
                .collect::<Vec<_>>(),
        )));
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[cfg(test)]
mod tests {
    // -------------------------------------------------------------------
    // Test-oracolo di `melt`/`pivot` (fast path, ultimo batch
    // ottimizzazioni kernel): l'output deve essere byte-identico al
    // percorso generico originale, copiato verbatim qui sotto come
    // riferimento indipendente.
    // -------------------------------------------------------------------

    use super::*;
    use std::collections::BTreeSet;

    use plenora_core::arrow::array::Date32Array;

    /// Copia verbatim dell'implementazione di `melt` pre-ottimizzazione.
    fn melt_reference(batch: &RecordBatch, config: &Melt, limits: &Limits) -> Result<RecordBatch> {
        let id_indices = config
            .id_columns
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        let value_indices = if config.value_columns.is_empty() {
            (0..batch.num_columns())
                .filter(|index| !id_indices.contains(index))
                .collect::<Vec<_>>()
        } else {
            config
                .value_columns
                .iter()
                .map(|name| column_index(batch, name))
                .collect::<Result<Vec<_>>>()?
        };
        if value_indices.is_empty() {
            return Err(PlenoraError::Contract("melt senza value_columns".into()));
        }
        let output_rows = batch
            .num_rows()
            .checked_mul(value_indices.len())
            .ok_or_else(|| PlenoraError::Contract("overflow righe melt".into()))?;
        if output_rows > limits.max_rows {
            return Err(PlenoraError::Contract("melt supera max_rows".into()));
        }
        let row_indices = value_indices
            .iter()
            .flat_map(|_| 0..batch.num_rows())
            .collect::<Vec<_>>();
        let repeated = select_rows(batch, &row_indices)?;
        let mut fields = id_indices
            .iter()
            .map(|index| repeated.schema().field(*index).as_ref().clone())
            .collect::<Vec<_>>();
        let mut columns = id_indices
            .iter()
            .map(|index| repeated.column(*index).clone())
            .collect::<Vec<_>>();
        let var_name = collision_free(&config.var_name, batch)?;
        let value_name = collision_free(&config.value_name, batch)?;
        fields.push(Field::new(&var_name, DataType::Utf8, false));
        let variables = value_indices
            .iter()
            .flat_map(|index| {
                std::iter::repeat_n(
                    batch.schema().field(*index).name().clone(),
                    batch.num_rows(),
                )
            })
            .collect::<Vec<_>>();
        columns.push(Arc::new(StringArray::from(variables)));
        let value_type = batch.column(value_indices[0]).data_type().clone();
        let homogeneous = value_indices
            .iter()
            .all(|index| batch.column(*index).data_type() == &value_type);
        if homogeneous {
            let arrays = value_indices
                .iter()
                .map(|index| batch.column(*index).as_ref())
                .collect::<Vec<_>>();
            fields.push(Field::new(&value_name, value_type, true));
            columns.push(plenora_core::arrow::select::concat::concat(&arrays)?);
        } else if matches!(config.type_policy, HeterogeneousTypePolicy::String) {
            let mut values = Vec::with_capacity(output_rows);
            for index in value_indices {
                for row in 0..batch.num_rows() {
                    let value = scalar_as_string(batch.column(index).as_ref(), row)?;
                    if value
                        .as_ref()
                        .is_some_and(|value| value.len() > limits.max_string_bytes)
                    {
                        return Err(PlenoraError::Contract(
                            "melt: valore testuale oltre max_string_bytes".into(),
                        ));
                    }
                    values.push(value);
                }
            }
            fields.push(Field::new(&value_name, DataType::Utf8, true));
            columns.push(Arc::new(StringArray::from(values)));
        } else {
            return Err(PlenoraError::Contract(
                "melt: value_columns eterogenee; impostare type_policy='string' per la conversione esplicita".into(),
            ));
        }
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            columns,
        )?)
    }

    /// Copia verbatim di `pivot_column` pre-ottimizzazione.
    fn pivot_column_reference(
        batch: &RecordBatch,
        index: usize,
        groups: &[Option<&Vec<usize>>],
        function: &PivotAgg,
    ) -> Result<(DataType, ArrayRef)> {
        let source = batch.column(index);
        Ok(match function {
            PivotAgg::First | PivotAgg::Last => {
                let indices = groups
                    .iter()
                    .map(|rows| {
                        rows.and_then(|rows| {
                            if matches!(function, PivotAgg::First) {
                                rows.first()
                            } else {
                                rows.last()
                            }
                        })
                        .copied()
                        .map(u32::try_from)
                        .transpose()
                        .map_err(|_| PlenoraError::Contract("indice pivot oltre u32".into()))
                    })
                    .collect::<Result<Vec<_>>>()?;
                (
                    source.data_type().clone(),
                    plenora_core::arrow::select::take::take(
                        source.as_ref(),
                        &UInt32Array::from(indices),
                        None,
                    )?,
                )
            }
            PivotAgg::Count => {
                let values = groups
                    .iter()
                    .map(|rows| {
                        rows.map(|rows| {
                            i64::try_from(
                                rows.iter().filter(|row| !source.is_null(**row)).count(),
                            )
                            .map_err(|_| {
                                PlenoraError::Contract("conteggio pivot oltre i64".into())
                            })
                        })
                        .transpose()
                    })
                    .collect::<Result<Vec<_>>>()?;
                (DataType::Int64, Arc::new(Int64Array::from(values)))
            }
            PivotAgg::Concat => {
                let values = groups
                    .iter()
                    .map(|rows| {
                        rows.map(|rows| {
                            rows.iter()
                                .filter_map(|row| {
                                    scalar_as_string(source.as_ref(), *row).transpose()
                                })
                                .collect::<Result<Vec<_>>>()
                                .map(|values| values.join(","))
                        })
                        .transpose()
                    })
                    .collect::<Result<Vec<_>>>()?;
                (DataType::Utf8, Arc::new(StringArray::from(values)))
            }
            PivotAgg::Sum | PivotAgg::Mean | PivotAgg::Min | PivotAgg::Max => {
                let values = groups
                    .iter()
                    .map(|rows| {
                        let Some(rows) = rows else { return Ok(None) };
                        let values = rows
                            .iter()
                            .filter_map(|row| scalar_as_f64(source.as_ref(), *row).transpose())
                            .collect::<Result<Vec<_>>>()?;
                        if values.is_empty() {
                            return Ok(None);
                        }
                        Ok(Some(match function {
                            PivotAgg::Sum => values.iter().sum(),
                            PivotAgg::Mean => {
                                values.iter().sum::<f64>()
                                    / values.len().to_f64().ok_or_else(|| {
                                        PlenoraError::Contract(
                                            "gruppo pivot non rappresentabile".into(),
                                        )
                                    })?
                            }
                            PivotAgg::Min => {
                                values.into_iter().reduce(f64::min).unwrap_or_default()
                            }
                            PivotAgg::Max => {
                                values.into_iter().reduce(f64::max).unwrap_or_default()
                            }
                            _ => unreachable!(),
                        }))
                    })
                    .collect::<Result<Vec<_>>>()?;
                (DataType::Float64, Arc::new(Float64Array::from(values)))
            }
        })
    }

    /// Copia verbatim dell'implementazione di `pivot` pre-ottimizzazione.
    fn pivot_reference(
        batch: &RecordBatch,
        config: &Pivot,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        let index_names = config
            .index_col
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>();
        let index_indices = index_names
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        let pivot_index = column_index(batch, &config.column)?;
        let value_index = column_index(batch, &config.value_col)?;
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut pivot_values = BTreeSet::new();
        for row in 0..batch.num_rows() {
            let key = composite_key(batch, &index_indices, row)?;
            let Some(pivot) = scalar_as_string(batch.column(pivot_index).as_ref(), row)? else {
                continue;
            };
            if config.mapping.is_empty() || config.mapping.contains_key(&pivot) {
                pivot_values.insert(pivot.clone());
                groups
                    .entry(format!("{key}{}:{pivot}", pivot.len()))
                    .or_default()
                    .push(row);
            }
        }
        let mut index_rows: BTreeMap<String, usize> = BTreeMap::new();
        for row in 0..batch.num_rows() {
            let key = composite_key(batch, &index_indices, row)?;
            index_rows.entry(key).or_insert(row);
        }
        if index_rows.len() > limits.max_rows
            || index_indices.len().saturating_add(pivot_values.len()) > limits.max_columns
        {
            return Err(PlenoraError::Contract(
                "pivot supera i limiti di output".into(),
            ));
        }
        let representatives = index_rows.values().copied().collect::<Vec<_>>();
        let selected = select_rows(batch, &representatives)?;
        let mut fields = index_indices
            .iter()
            .map(|index| selected.schema().field(*index).as_ref().clone())
            .collect::<Vec<_>>();
        let mut columns = index_indices
            .iter()
            .map(|index| selected.column(*index).clone())
            .collect::<Vec<_>>();
        for pivot_value in pivot_values {
            let output = config
                .mapping
                .get(&pivot_value)
                .cloned()
                .unwrap_or_else(|| pivot_value.clone());
            validate_output_name(&output)?;
            let grouped_rows = index_rows
                .keys()
                .map(|key| groups.get(&format!("{key}{}:{pivot_value}", pivot_value.len())))
                .collect::<Vec<_>>();
            let (data_type, values) =
                pivot_column_reference(batch, value_index, &grouped_rows, &config.aggr_func)?;
            fields.push(Field::new(&output, data_type, true));
            columns.push(values);
        }
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            columns,
        )?)
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
                DataType::Utf8 => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("utf8");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("utf8");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row),
                                reference_values.value(row),
                                "testo riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                DataType::Boolean => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<BooleanArray>()
                        .expect("bool");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<BooleanArray>()
                        .expect("bool");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row),
                                reference_values.value(row),
                                "bool riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                _ => {
                    // Tipi senza confronto nativo dedicato (es. Date32 in
                    // first/last): stessi byte via profilo scalare.
                    for row in 0..fast.num_rows() {
                        assert_eq!(
                            scalar_as_string(fast_column.as_ref(), row).expect("fast"),
                            scalar_as_string(reference_column.as_ref(), row).expect("ref"),
                            "valore riga {row} colonna {}",
                            fast_field.name()
                        );
                    }
                }
            }
        }
    }

    fn batch_of(fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("fixture")
    }

    fn melt_config(
        id_columns: &[&str],
        value_columns: &[&str],
        type_policy: HeterogeneousTypePolicy,
    ) -> Melt {
        Melt {
            id_columns: id_columns.iter().map(|name| (*name).into()).collect(),
            value_columns: value_columns.iter().map(|name| (*name).into()).collect(),
            var_name: "variable".into(),
            value_name: "value".into(),
            type_policy,
        }
    }

    fn pivot_config(index: &str, column: &str, value: &str, aggr_func: PivotAgg) -> Pivot {
        Pivot {
            index_col: index.into(),
            column: column.into(),
            value_col: value.into(),
            aggr_func,
            mapping: BTreeMap::new(),
        }
    }

    fn schema_names(batch: &RecordBatch) -> Vec<String> {
        batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect()
    }

    #[test]
    fn melt_homogeneous_float64_oracle() {
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("a", DataType::Float64, true),
                Field::new("b", DataType::Float64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3, 4])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    None,
                    Some(f64::NAN),
                    Some(-0.0),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(-2.25),
                    Some(0.0),
                    None,
                    Some(f64::INFINITY),
                ])),
            ],
        );
        let config = melt_config(&["id"], &["a", "b"], HeterogeneousTypePolicy::Reject);
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn melt_default_value_columns_e_naming_collision_oracle() {
        // Quirk di naming: la colonna id si chiama gia' "value", quindi la
        // colonna valore di output diventa "value_1"; "variable" resta libero.
        let batch = batch_of(
            vec![
                Field::new("value", DataType::Int64, false),
                Field::new("x", DataType::Int64, true),
                Field::new("y", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![10, 20])),
                Arc::new(Int64Array::from(vec![Some(1), None])),
                Arc::new(Int64Array::from(vec![None, Some(2)])),
            ],
        );
        let config = melt_config(&["value"], &[], HeterogeneousTypePolicy::Reject);
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        assert_eq!(
            schema_names(&fast),
            vec!["value", "variable", "value_1"],
            "quirk value->value_1"
        );
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);

        // Collisione doppia: anche "variable" occupata -> "variable_1".
        let batch = batch_of(
            vec![
                Field::new("variable", DataType::Int64, false),
                Field::new("value", DataType::Int64, false),
                Field::new("x", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(Int64Array::from(vec![3, 4])),
                Arc::new(Int64Array::from(vec![Some(5), None])),
            ],
        );
        let config = melt_config(
            &["variable", "value"],
            &["x"],
            HeterogeneousTypePolicy::Reject,
        );
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        assert_eq!(
            schema_names(&fast),
            vec!["variable", "value", "variable_1", "value_1"],
            "doppia collisione"
        );
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn melt_heterogeneous_string_policy_oracle() {
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("n", DataType::Int64, true),
                Field::new("t", DataType::Utf8, true),
                Field::new("f", DataType::Float64, true),
                Field::new("b", DataType::Boolean, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(Int64Array::from(vec![Some(-7), None, Some(42)])),
                Arc::new(StringArray::from(vec![Some("alfa"), Some(""), None])),
                Arc::new(Float64Array::from(vec![
                    Some(-0.0),
                    Some(f64::NAN),
                    Some(2.5e300),
                ])),
                Arc::new(BooleanArray::from(vec![Some(true), None, Some(false)])),
            ],
        );
        let config = melt_config(
            &["id"],
            &["n", "t", "f", "b"],
            HeterogeneousTypePolicy::String,
        );
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn melt_heterogeneous_generic_fallback_oracle() {
        // Date32 non ha fast path: ricade sul percorso scalare generico.
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("d", DataType::Date32, true),
                Field::new("n", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(Date32Array::from(vec![Some(0), None, Some(19_700)])),
                Arc::new(Int64Array::from(vec![Some(9), Some(-1), None])),
            ],
        );
        let config = melt_config(&["id"], &["d", "n"], HeterogeneousTypePolicy::String);
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn melt_errori_identici() {
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("n", DataType::Int64, true),
                Field::new("t", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(Int64Array::from(vec![Some(7)])),
                Arc::new(StringArray::from(vec![Some("x")])),
            ],
        );
        // Eterogenee senza policy string: stesso errore.
        let config = melt_config(&["id"], &["n", "t"], HeterogeneousTypePolicy::Reject);
        let fast = melt(&batch, &config, &Limits::default());
        let reference = melt_reference(&batch, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // max_string_bytes: stesso errore.
        let limits = Limits {
            max_string_bytes: 0,
            ..Limits::default()
        };
        let config = melt_config(&["id"], &["n", "t"], HeterogeneousTypePolicy::String);
        let fast = melt(&batch, &config, &limits);
        let reference = melt_reference(&batch, &config, &limits);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // Batch senza colonne valore: stesso errore.
        let only_ids = batch_of(
            vec![Field::new("id", DataType::Int64, false)],
            vec![Arc::new(Int64Array::from(vec![1]))],
        );
        let config = melt_config(&["id"], &[], HeterogeneousTypePolicy::Reject);
        let fast = melt(&only_ids, &config, &Limits::default());
        let reference = melt_reference(&only_ids, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
    }

    #[test]
    fn melt_input_vuoto_oracle() {
        let batch = batch_of(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("a", DataType::Float64, true),
            ],
            vec![
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
            ],
        );
        let config = melt_config(&["id"], &["a"], HeterogeneousTypePolicy::Reject);
        let fast = melt(&batch, &config, &Limits::default()).expect("melt fast");
        let reference = melt_reference(&batch, &config, &Limits::default()).expect("melt ref");
        assert_batches_identical(&fast, &reference);
    }

    /// Fixture pivot: chiavi utf8 (con null), pivot utf8 (con null),
    /// valori float64 (con null, NaN, -0.0). Copre first/last/count/concat
    /// e tutte le riduzioni numeriche.
    fn pivot_fixture_utf8() -> RecordBatch {
        let keys: Vec<Option<&str>> = vec![
            Some("b"),
            Some("a"),
            None,
            Some("a"),
            Some("b"),
            None,
            Some("a"),
            Some("b"),
        ];
        let pivots: Vec<Option<&str>> = vec![
            Some("y"),
            Some("x"),
            Some("x"),
            Some("y"),
            Some("x"),
            None,
            Some("x"),
            Some("y"),
        ];
        let values: Vec<Option<f64>> = vec![
            Some(1.0),
            Some(-0.0),
            Some(10.0),
            Some(f64::NAN),
            Some(3.5),
            Some(20.0),
            None,
            Some(-7.25),
        ];
        batch_of(
            vec![
                Field::new("k", DataType::Utf8, true),
                Field::new("p", DataType::Utf8, true),
                Field::new("v", DataType::Float64, true),
            ],
            vec![
                Arc::new(StringArray::from(keys)),
                Arc::new(StringArray::from(pivots)),
                Arc::new(Float64Array::from(values)),
            ],
        )
    }

    #[test]
    fn pivot_tutte_le_aggr_oracle() {
        let batch = pivot_fixture_utf8();
        for function in [
            PivotAgg::First,
            PivotAgg::Last,
            PivotAgg::Min,
            PivotAgg::Max,
            PivotAgg::Sum,
            PivotAgg::Mean,
            PivotAgg::Count,
            PivotAgg::Concat,
        ] {
            let config = pivot_config("k", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_valori_int64_e_testo_oracle() {
        let batch = batch_of(
            vec![
                Field::new("k", DataType::Int64, true),
                Field::new("p", DataType::Utf8, true),
                Field::new("n", DataType::Int64, true),
                Field::new("t", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(2),
                    Some(1),
                    Some(2),
                    None,
                    Some(1),
                    Some(2),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("u"),
                    Some("u"),
                    Some("v"),
                    Some("u"),
                    Some("v"),
                    Some("u"),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(5),
                    None,
                    Some(-3),
                    Some(100),
                    Some(8),
                    Some(2),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("z"),
                    Some("c"),
                    Some("d"),
                ])),
            ],
        );
        for (value, function) in [
            ("n", PivotAgg::Sum),
            ("n", PivotAgg::Mean),
            ("n", PivotAgg::Min),
            ("n", PivotAgg::Max),
            ("n", PivotAgg::Count),
            ("n", PivotAgg::First),
            ("n", PivotAgg::Last),
            ("n", PivotAgg::Concat),
            ("t", PivotAgg::Concat),
            ("t", PivotAgg::First),
            ("t", PivotAgg::Last),
        ] {
            let config = pivot_config("k", "p", value, function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_mapping_rinomina_e_filtra_oracle() {
        let batch = pivot_fixture_utf8();
        let mut mapping = BTreeMap::new();
        mapping.insert("x".to_owned(), "col_x".to_owned());
        // "y" escluso dal mapping -> non produce colonne.
        let config = Pivot {
            mapping,
            ..pivot_config("k", "p", "v", PivotAgg::Sum)
        };
        let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
        assert_eq!(
            schema_names(&fast),
            vec!["k", "col_x"],
            "mapping rinomina e filtra"
        );
        let reference = pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn pivot_multi_indice_e_float_keys_oracle() {
        // Indice composto + pivot float64: -0.0 e 0.0 restano chiavi
        // distinte ("-0" vs "0"), NaN e' una chiave ("NaN").
        let batch = batch_of(
            vec![
                Field::new("a", DataType::Int64, true),
                Field::new("b", DataType::Utf8, true),
                Field::new("p", DataType::Float64, true),
                Field::new("v", DataType::Float64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    Some(1),
                    None,
                    Some(1),
                    None,
                    Some(2),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("s"),
                    Some("s"),
                    None,
                    Some("t"),
                    None,
                    Some("s"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(-0.0),
                    Some(0.0),
                    Some(f64::NAN),
                    Some(-0.0),
                    Some(1.5),
                    Some(0.0),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.0),
                    Some(2.0),
                    Some(3.0),
                    Some(4.0),
                    Some(5.0),
                    None,
                ])),
            ],
        );
        for function in [PivotAgg::Sum, PivotAgg::First, PivotAgg::Count, PivotAgg::Concat] {
            let config = pivot_config("a, b", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_tipici_generici_oracle() {
        // Date32 (indice e valore) e Boolean (pivot): fuori dai fast path
        // chiave/numerico, ricadono sul percorso scalare generico.
        let batch = batch_of(
            vec![
                Field::new("d", DataType::Date32, true),
                Field::new("p", DataType::Boolean, true),
                Field::new("v", DataType::Date32, true),
            ],
            vec![
                Arc::new(Date32Array::from(vec![
                    Some(0),
                    Some(1),
                    Some(0),
                    None,
                    Some(1),
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    Some(false),
                    Some(true),
                    Some(true),
                    Some(true),
                ])),
                Arc::new(Date32Array::from(vec![
                    Some(10),
                    Some(20),
                    Some(30),
                    Some(40),
                    None,
                ])),
            ],
        );
        for function in [PivotAgg::Sum, PivotAgg::Mean, PivotAgg::Count, PivotAgg::First] {
            let config = pivot_config("d", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_molti_distinti_oracle() {
        // 500 chiavi x 30 pivot distinti, valori con null sparsi.
        let rows = 15_000;
        let keys = (0..rows)
            .map(|row| format!("k{:04}", row % 500))
            .collect::<Vec<_>>();
        let pivots = (0..rows)
            .map(|row| format!("p{:02}", (row / 500) % 30))
            .collect::<Vec<_>>();
        let values = (0..rows)
            .map(|row| {
                if row % 7 == 0 {
                    None
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    Some((row % 997) as f64 / 3.0)
                }
            })
            .collect::<Vec<_>>();
        let batch = batch_of(
            vec![
                Field::new("k", DataType::Utf8, false),
                Field::new("p", DataType::Utf8, false),
                Field::new("v", DataType::Float64, true),
            ],
            vec![
                Arc::new(StringArray::from(keys)),
                Arc::new(StringArray::from(pivots)),
                Arc::new(Float64Array::from(values)),
            ],
        );
        for function in [PivotAgg::Sum, PivotAgg::First, PivotAgg::Count] {
            let config = pivot_config("k", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_un_pivot_solo_oracle() {
        // Estremo opposto: un solo valore pivot distinto su molte chiavi.
        let rows = 1_000;
        let keys = (0..rows)
            .map(|row| i64::try_from(row).expect("fixture"))
            .collect::<Vec<_>>();
        let pivots = (0..rows).map(|_| "solo").collect::<Vec<_>>();
        let values = (0..rows).map(|row| i64::try_from(row % 50).ok()).collect::<Vec<_>>();
        let batch = batch_of(
            vec![
                Field::new("k", DataType::Int64, false),
                Field::new("p", DataType::Utf8, false),
                Field::new("v", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(keys)),
                Arc::new(StringArray::from(pivots)),
                Arc::new(Int64Array::from(values)),
            ],
        );
        for function in [PivotAgg::Sum, PivotAgg::Last, PivotAgg::Concat] {
            let config = pivot_config("k", "p", "v", function.clone());
            let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
            let reference =
                pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn pivot_input_vuoto_oracle() {
        let batch = batch_of(
            vec![
                Field::new("k", DataType::Utf8, true),
                Field::new("p", DataType::Utf8, true),
                Field::new("v", DataType::Float64, true),
            ],
            vec![
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
            ],
        );
        let config = pivot_config("k", "p", "v", PivotAgg::Sum);
        let fast = pivot(&batch, &config, &Limits::default()).expect("pivot fast");
        let reference = pivot_reference(&batch, &config, &Limits::default()).expect("pivot ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn pivot_errori_identici() {
        let batch = pivot_fixture_utf8();
        // Colonna inesistente: stesso errore.
        let config = pivot_config("k", "p", "manca", PivotAgg::Sum);
        let fast = pivot(&batch, &config, &Limits::default());
        let reference = pivot_reference(&batch, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // Limiti di output: stesso errore.
        let limits = Limits {
            max_columns: 2,
            ..Limits::default()
        };
        let config = pivot_config("k", "p", "v", PivotAgg::Sum);
        let fast = pivot(&batch, &config, &limits);
        let reference = pivot_reference(&batch, &config, &limits);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
    }

    // -------------------------------------------------------------------
    // Test-oracolo di `table_diff` (batch 4 ottimizzazioni kernel): la
    // implementazione pre-ottimizzazione e' copiata verbatim qui sotto come
    // riferimento indipendente (usa `composite_key`, mantenuta come oracolo).
    // -------------------------------------------------------------------

    /// Copia verbatim dell'implementazione di `table_diff`
    /// pre-ottimizzazione.
    #[allow(clippy::too_many_lines)]
    fn table_diff_reference(
        left: &RecordBatch,
        right: &RecordBatch,
        config: &TableDiff,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
            return Err(PlenoraError::Contract("chiavi table_diff non valide".into()));
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
        let compare = if config.compare_columns.is_empty() {
            left.schema()
                .fields()
                .iter()
                .map(|field| field.name().clone())
                .filter(|name| {
                    !config.left_keys.contains(name) && right.schema().index_of(name).is_ok()
                })
                .collect::<Vec<_>>()
        } else {
            config.compare_columns.clone()
        };
        let left_compare = compare
            .iter()
            .map(|name| column_index(left, name))
            .collect::<Result<Vec<_>>>()?;
        let right_compare = compare
            .iter()
            .map(|name| column_index(right, name))
            .collect::<Result<Vec<_>>>()?;
        let mut old = BTreeMap::new();
        let mut new = BTreeMap::new();
        for row in 0..left.num_rows() {
            let key = composite_key(left, &left_keys, row)?;
            if old.insert(key, row).is_some() {
                return Err(PlenoraError::Contract(
                    "chiavi duplicate nella tabella sinistra".into(),
                ));
            }
        }
        for row in 0..right.num_rows() {
            let key = composite_key(right, &right_keys, row)?;
            if new.insert(key, row).is_some() {
                return Err(PlenoraError::Contract(
                    "chiavi duplicate nella tabella destra".into(),
                ));
            }
        }
        // Preserve source order: old rows first, then new-only rows. Sorting the
        // encoded key would place nulls first and reorder otherwise stable data.
        let mut all_keys = Vec::with_capacity(old.len().saturating_add(new.len()));
        for row in 0..left.num_rows() {
            all_keys.push(composite_key(left, &left_keys, row)?);
        }
        for row in 0..right.num_rows() {
            let key = composite_key(right, &right_keys, row)?;
            if !old.contains_key(&key) {
                all_keys.push(key);
            }
        }
        let mut rows = Vec::new();
        for key in all_keys {
            let old_row = old.get(&key).copied();
            let new_row = new.get(&key).copied();
            let (status, changed, old_values) = match (old_row, new_row) {
                (None, Some(_)) => ("ADDED".to_owned(), None, None),
                (Some(_), None) => ("DELETED".to_owned(), None, None),
                (Some(old_row), Some(new_row)) => {
                    let mut changed = Vec::new();
                    let mut old_values = Vec::new();
                    for ((name, left_index), right_index) in
                        compare.iter().zip(&left_compare).zip(&right_compare)
                    {
                        let before = scalar_as_string(left.column(*left_index).as_ref(), old_row)?;
                        let after =
                            scalar_as_string(right.column(*right_index).as_ref(), new_row)?;
                        if before != after {
                            changed.push(name.clone());
                            old_values.push(before.unwrap_or_default());
                        }
                    }
                    if changed.is_empty() {
                        ("UNCHANGED".to_owned(), None, None)
                    } else {
                        (
                            "MODIFIED".to_owned(),
                            Some(changed.join(&config.separator)),
                            Some(old_values.join(&config.separator)),
                        )
                    }
                }
                (None, None) => unreachable!(),
            };
            if status != "UNCHANGED" || config.include_unchanged == "yes" {
                rows.push(DiffRow {
                    old_row,
                    new_row,
                    status,
                    changed,
                    old_values,
                });
            }
        }
        if rows.len() > limits.max_rows {
            return Err(PlenoraError::Contract("table_diff supera max_rows".into()));
        }
        let output_count = config
            .right_keys
            .len()
            .saturating_add(compare.len())
            .saturating_add(3);
        if output_count > limits.max_columns {
            return Err(PlenoraError::Contract(
                "table_diff supera max_columns".into(),
            ));
        }
        let mut fields = Vec::new();
        let mut columns: Vec<Arc<dyn plenora_core::arrow::array::Array>> = Vec::new();
        for (position, name) in config.left_keys.iter().enumerate() {
            let left_column = left.column(left_keys[position]);
            let right_column = right.column(right_keys[position]);
            fields.push(
                left.schema()
                    .field(left_keys[position])
                    .as_ref()
                    .clone()
                    .with_name(name)
                    .with_nullable(true),
            );
            columns.push(diff_values(left_column, right_column, &rows)?);
        }
        for (position, name) in compare.iter().enumerate() {
            let left_column = left.column(left_compare[position]);
            let right_column = right.column(right_compare[position]);
            fields.push(
                right
                    .schema()
                    .field(right_compare[position])
                    .as_ref()
                    .clone()
                    .with_name(name)
                    .with_nullable(true),
            );
            columns.push(diff_values(left_column, right_column, &rows)?);
        }
        for (name, selector) in [
            ("_diff_status", 0_usize),
            ("_diff_columns", 1),
            ("_diff_old_values", 2),
        ] {
            fields.push(Field::new(
                name,
                DataType::Utf8,
                selector == 1 || selector == 2,
            ));
            columns.push(Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| match selector {
                        0 => Some(row.status.clone()),
                        1 => row.changed.clone(),
                        _ => row.old_values.clone(),
                    })
                    .collect::<Vec<_>>(),
            )));
        }
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            columns,
        )?)
    }

    /// Fixture diff: chiavi composite `id` int64 + `grp` utf8 nullable,
    /// colonne confrontate `num` float64 (con NaN, -0.0 e null) e `txt`.
    /// Stati coperti: UNCHANGED (anche NaN==NaN), MODIFIED (valore, null e
    /// -0.0 vs 0.0), DELETED, ADDED.
    fn diff_fixtures() -> (RecordBatch, RecordBatch) {
        let left = batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("txt", DataType::Utf8, true),
                Field::new("solo_sinistra", DataType::Int64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    Some(2),
                    Some(3),
                    Some(4),
                    Some(5),
                    Some(6),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("d"),
                    Some("e"),
                    Some("f"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(f64::NAN),
                    Some(-0.0),
                    None,
                    Some(5.0),
                    Some(6.0),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("uno"),
                    Some("due"),
                    Some("tre"),
                    Some("quattro"),
                    Some("cinque"),
                    Some("sei"),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(10),
                    Some(20),
                    Some(30),
                    Some(40),
                    Some(50),
                    Some(60),
                ])),
            ],
        );
        let right = batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("txt", DataType::Utf8, true),
                Field::new("solo_destra", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    Some(2),
                    Some(3),
                    Some(4),
                    Some(7),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("d"),
                    Some("g"),
                    None,
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(f64::NAN),
                    Some(0.0),
                    Some(4.0),
                    Some(7.0),
                    None,
                ])),
                Arc::new(StringArray::from(vec![
                    Some("uno"),
                    Some("due"),
                    Some("tre"),
                    None,
                    Some("sette"),
                    Some("null-key"),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("r1"),
                    Some("r2"),
                    Some("r3"),
                    Some("r4"),
                    Some("r7"),
                    Some("rn"),
                ])),
            ],
        );
        (left, right)
    }

    fn diff_config(include_unchanged: &str) -> TableDiff {
        TableDiff {
            left_keys: vec!["id".into(), "grp".into()],
            right_keys: vec!["id".into(), "grp".into()],
            compare_columns: vec!["num".into(), "txt".into()],
            include_unchanged: include_unchanged.into(),
            separator: ", ".into(),
        }
    }

    #[test]
    fn table_diff_stati_misti_oracle() {
        let (left, right) = diff_fixtures();
        for include_unchanged in ["no", "yes"] {
            let config = diff_config(include_unchanged);
            let fast = table_diff(&left, &right, &config, &Limits::default()).expect("fast");
            let reference =
                table_diff_reference(&left, &right, &config, &Limits::default()).expect("ref");
            assert_batches_identical(&fast, &reference);
        }
        // Verifica puntuale degli stati attesi (ordine: sinistra, poi
        // solo-destra): id 1 UNCHANGED, id 2 UNCHANGED (NaN==NaN),
        // id 3 MODIFIED (-0.0 vs 0.0), id 4 MODIFIED (null vs valore),
        // id 5 DELETED, id 6 DELETED, id 7 ADDED, chiave null ADDED.
        let config = diff_config("no");
        let fast = table_diff(&left, &right, &config, &Limits::default()).expect("fast");
        let status = fast
            .column(fast.schema().index_of("_diff_status").expect("status"))
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("status utf8");
        let states: Vec<&str> = (0..status.len()).map(|row| status.value(row)).collect();
        assert_eq!(
            states,
            vec!["MODIFIED", "MODIFIED", "DELETED", "DELETED", "ADDED", "ADDED"]
        );
    }

    #[test]
    fn table_diff_compare_default_oracle() {
        let (left, right) = diff_fixtures();
        // compare_columns vuoto: confronta tutte le colonne condivise non
        // chiave (num, txt); le colonne non condivise restano fuori.
        let config = TableDiff {
            compare_columns: Vec::new(),
            separator: " | ".into(),
            ..diff_config("yes")
        };
        let fast = table_diff(&left, &right, &config, &Limits::default()).expect("fast");
        let reference =
            table_diff_reference(&left, &right, &config, &Limits::default()).expect("ref");
        assert_batches_identical(&fast, &reference);
    }

    #[test]
    fn table_diff_input_vuoti_oracle() {
        let (left, right) = diff_fixtures();
        let empty_left = left.slice(0, 0);
        let empty_right = right.slice(0, 0);
        let config = diff_config("yes");
        for (l, r) in [
            (&empty_left, &right),
            (&left, &empty_right),
            (&empty_left, &empty_right),
        ] {
            let fast = table_diff(l, r, &config, &Limits::default()).expect("fast");
            let reference = table_diff_reference(l, r, &config, &Limits::default()).expect("ref");
            assert_batches_identical(&fast, &reference);
        }
    }

    #[test]
    fn table_diff_errori_identici() {
        let (left, right) = diff_fixtures();
        // Chiavi duplicate a sinistra e a destra: stesso messaggio.
        let dup_left = batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("txt", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(1)])),
                Arc::new(StringArray::from(vec![Some("a"), Some("a")])),
                Arc::new(Float64Array::from(vec![Some(1.0), Some(2.0)])),
                Arc::new(StringArray::from(vec![Some("x"), Some("y")])),
            ],
        );
        let config = diff_config("no");
        let fast = table_diff(&dup_left, &right, &config, &Limits::default());
        let reference = table_diff_reference(&dup_left, &right, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        let fast = table_diff(&left, &dup_left, &config, &Limits::default());
        let reference = table_diff_reference(&left, &dup_left, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // Configurazioni non valide: stesso errore.
        for config in [
            TableDiff {
                left_keys: Vec::new(),
                ..diff_config("no")
            },
            TableDiff {
                left_keys: vec!["id".into()],
                right_keys: vec!["id".into(), "grp".into()],
                ..diff_config("no")
            },
            TableDiff {
                left_keys: vec!["manca".into(), "grp".into()],
                ..diff_config("no")
            },
            TableDiff {
                compare_columns: vec!["manca".into()],
                ..diff_config("no")
            },
        ] {
            let fast = table_diff(&left, &right, &config, &Limits::default());
            let reference = table_diff_reference(&left, &right, &config, &Limits::default());
            assert_eq!(
                format!("{:?}", fast.expect_err("fast deve fallire")),
                format!("{:?}", reference.expect_err("ref deve fallire"))
            );
        }
        // Tipi diversi sulle colonne confrontate: stesso errore di
        // diff_values.
        let typed_right = batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
                Field::new("num", DataType::Utf8, true),
                Field::new("txt", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![Some(1)])),
                Arc::new(StringArray::from(vec![Some("a")])),
                Arc::new(StringArray::from(vec![Some("1.5")])),
                Arc::new(StringArray::from(vec![Some("uno")])),
            ],
        );
        let config = diff_config("no");
        let fast = table_diff(&left, &typed_right, &config, &Limits::default());
        let reference = table_diff_reference(&left, &typed_right, &config, &Limits::default());
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        // Limiti: stesso errore.
        let limits = Limits {
            max_rows: 1,
            ..Limits::default()
        };
        let fast = table_diff(&left, &right, &config, &limits);
        let reference = table_diff_reference(&left, &right, &config, &limits);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
        let limits = Limits {
            max_columns: 2,
            ..Limits::default()
        };
        let fast = table_diff(&left, &right, &config, &limits);
        let reference = table_diff_reference(&left, &right, &config, &limits);
        assert_eq!(
            format!("{:?}", fast.expect_err("fast deve fallire")),
            format!("{:?}", reference.expect_err("ref deve fallire"))
        );
    }
}
