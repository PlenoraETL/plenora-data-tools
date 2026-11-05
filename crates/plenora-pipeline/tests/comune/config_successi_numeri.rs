//! Modello indipendente delle riduzioni sul corpus di sei righe.
//! I numeri sono interi piccoli o seminteri; le conversioni intere sotto
//! controllano esplicitamente dominio e parte frazionaria.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    UInt32Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
fn count(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}
fn integer(n: f64) -> i64 {
    assert!(n.is_finite() && n.abs() < 1_000_000.0 && n.fract() == 0.0);
    n as i64
}
fn text(a: &dyn Array, row: usize) -> Option<String> {
    if a.is_null(row) {
        return None;
    }
    Some(match a.data_type() {
        DataType::Utf8 => a
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(row)
            .to_owned(),
        DataType::Binary => String::from_utf8(
            a.as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap()
                .value(row)
                .to_vec(),
        )
        .unwrap(),
        DataType::Int64 => a
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(row)
            .to_string(),
        DataType::Float64 => a
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(row)
            .to_string(),
        DataType::Boolean => a
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap()
            .value(row)
            .to_string(),
        _ => panic!("chiave fuori corpus"),
    })
}
fn numbers(a: &dyn Array) -> Vec<Option<f64>> {
    (0..a.len())
        .map(|i| {
            if a.is_null(i) {
                return None;
            }
            Some(match a.data_type() {
                DataType::Int64 => f64::from(
                    i32::try_from(a.as_any().downcast_ref::<Int64Array>().unwrap().value(i))
                        .unwrap(),
                ),
                DataType::Float64 => a.as_any().downcast_ref::<Float64Array>().unwrap().value(i),
                _ => panic!("numero fuori corpus di successo"),
            })
        })
        .collect()
}
fn take(a: &dyn Array, rows: Vec<Option<usize>>) -> ArrayRef {
    let indices = UInt32Array::from(
        rows.into_iter()
            .map(|r| r.map(|i| u32::try_from(i).unwrap()))
            .collect::<Vec<_>>(),
    );
    plenora_core::arrow::select::take::take(a, &indices, None).unwrap()
}
fn put(batch: &RecordBatch, name: &str, array: ArrayRef, nullable: bool) -> RecordBatch {
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    let mut columns = batch.columns().to_vec();
    let field = Field::new(name, array.data_type().clone(), nullable);
    if let Some(i) = fields.iter().position(|f| f.name() == name) {
        fields[i] = field;
        columns[i] = array;
    } else {
        fields.push(field);
        columns.push(array);
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        )),
        columns,
    )
    .unwrap()
}
fn group_rows(batch: &RecordBatch, names: &[&str], canonical: bool) -> Vec<Vec<usize>> {
    let mut groups: BTreeMap<Vec<Option<String>>, Vec<usize>> = BTreeMap::new();
    for row in 0..batch.num_rows() {
        let key = names
            .iter()
            .map(|name| {
                text(batch.column_by_name(name).unwrap().as_ref(), row).map(|t| {
                    if canonical {
                        format!("{}:{t}", t.len())
                    } else {
                        t
                    }
                })
            })
            .collect();
        groups.entry(key).or_default().push(row);
    }
    groups.into_values().collect()
}
#[allow(clippy::suboptimal_flops)] // Ordine aritmetico esplicito dell oracolo, senza FMA.
fn reduce(values: &[f64], function: &str, ddof: usize, quantile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let sum: f64 = values.iter().sum();
    Some(match function {
        "sum" => sum,
        "mean" | "avg" => sum / count(values.len()),
        "min" => values.iter().copied().min_by(f64::total_cmp).unwrap(),
        "max" => values.iter().copied().max_by(f64::total_cmp).unwrap(),
        "variance" | "stddev" => {
            if values.len() <= ddof {
                return None;
            }
            let mean = sum / count(values.len());
            let variance = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>()
                / count(values.len() - ddof);
            if function == "stddev" {
                variance.sqrt()
            } else {
                variance
            }
        }
        "quantile" => {
            let mut sorted = values.to_vec();
            sorted.sort_by(f64::total_cmp);
            let position = quantile * count(values.len() - 1);
            let low = usize::try_from(integer(position.floor())).unwrap();
            let high = usize::try_from(integer(position.ceil())).unwrap();
            sorted[low] + (sorted[high] - sorted[low]) * (position - position.floor())
        }
        _ => panic!("riduzione fuori corpus"),
    })
}
#[allow(clippy::too_many_lines)]
fn aggregate(c: &Value, batch: &RecordBatch) -> RecordBatch {
    let names: Vec<_> = c["group_by"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let groups = group_rows(batch, &names, true);
    let first: Vec<_> = groups.iter().map(|g| Some(g[0])).collect();
    let mut fields: Vec<_> = names
        .iter()
        .map(|n| batch.schema().field_with_name(n).unwrap().clone())
        .collect();
    let mut columns: Vec<_> = names
        .iter()
        .map(|n| take(batch.column_by_name(n).unwrap().as_ref(), first.clone()))
        .collect();
    let aggregations = c["aggregations"].as_array().unwrap();
    assert_eq!(aggregations.len(), 1, "corpus una riduzione per config");
    for cfg in aggregations {
        let name = cfg["column"].as_str().unwrap();
        let a = batch.column_by_name(name).unwrap();
        let fun = cfg["function"].as_str().unwrap();
        let output = cfg["alias"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(name);
        let skip = cfg["skip_null"] != false;
        let distinct = cfg["distinct"] == true;
        let result: ArrayRef = match fun {
            "count" => Arc::new(Int64Array::from(
                groups
                    .iter()
                    .map(|g| i64::try_from(g.iter().filter(|&&r| !a.is_null(r)).count()).unwrap())
                    .collect::<Vec<_>>(),
            )),
            "nunique" => Arc::new(Int64Array::from(
                groups
                    .iter()
                    .map(|g| {
                        let unique: BTreeSet<_> = g
                            .iter()
                            .map(|&r| text(a.as_ref(), r))
                            .filter(|v| !skip || v.is_some())
                            .collect();
                        i64::try_from(unique.len()).unwrap()
                    })
                    .collect::<Vec<_>>(),
            )),
            "first" | "last" => take(
                a.as_ref(),
                groups
                    .iter()
                    .map(|g| Some(if fun == "first" { g[0] } else { g[g.len() - 1] }))
                    .collect(),
            ),
            "concat" => Arc::new(StringArray::from(
                groups
                    .iter()
                    .map(|g| {
                        let mut seen = BTreeSet::new();
                        let parts: Vec<_> = g
                            .iter()
                            .map(|&r| text(a.as_ref(), r))
                            .filter(|v| !skip || v.is_some())
                            .map(Option::unwrap_or_default)
                            .filter(|v| !distinct || seen.insert(v.clone()))
                            .collect();
                        Some(parts.join(cfg["separator"].as_str().unwrap_or(", ")))
                    })
                    .collect::<Vec<_>>(),
            )),
            _ => {
                let native = numbers(a.as_ref());
                let results: Vec<_> = groups
                    .iter()
                    .map(|g| {
                        if !skip && g.iter().any(|&r| native[r].is_none()) {
                            return None;
                        }
                        let mut values: Vec<_> = g.iter().filter_map(|&r| native[r]).collect();
                        if distinct {
                            values.sort_by(f64::total_cmp);
                            values.dedup_by(|a, b| a.to_bits() == b.to_bits());
                        }
                        reduce(
                            &values,
                            fun,
                            usize::try_from(cfg["ddof"].as_u64().unwrap_or(1)).unwrap(),
                            cfg["quantile"].as_f64().unwrap_or(0.5),
                        )
                    })
                    .collect();
                if a.data_type() == &DataType::Int64 && matches!(fun, "sum" | "min" | "max") {
                    Arc::new(Int64Array::from(
                        results
                            .into_iter()
                            .map(|v| v.map(integer))
                            .collect::<Vec<_>>(),
                    ))
                } else {
                    Arc::new(Float64Array::from(results))
                }
            }
        };
        let nullable = !matches!(fun, "count" | "nunique");
        fields.push(Field::new(output, result.data_type().clone(), nullable));
        columns.push(result);
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        )),
        columns,
    )
    .unwrap()
}
fn statistics(c: &Value, batch: &RecordBatch) -> RecordBatch {
    let name = c["column"].as_str().unwrap();
    let values = numbers(batch.column_by_name(name).unwrap().as_ref());
    let names: Vec<_> = c["group_by"].as_str().into_iter().collect();
    let groups = group_rows(batch, &names, false);
    let mut result = batch.clone();
    for stat in c["stats"].as_array().unwrap() {
        let stat = stat.as_str().unwrap();
        let mut output = vec![None; batch.num_rows()];
        for group in &groups {
            let present: Vec<_> = group.iter().filter_map(|&r| values[r]).collect();
            let value = match stat {
                "count" => Some(count(present.len())),
                "mean" => reduce(&present, "mean", 1, 0.5),
                _ => panic!("statistica fuori corpus"),
            };
            for &row in group {
                output[row] = value;
            }
        }
        result = put(
            &result,
            &format!("{name}_{stat}"),
            Arc::new(Float64Array::from(output)),
            true,
        );
    }
    result
}
fn rolling(c: &Value, batch: &RecordBatch) -> RecordBatch {
    let name = c["column"].as_str().unwrap();
    let source = batch.column_by_name(name).unwrap();
    let values = numbers(source.as_ref());
    let width = usize::try_from(c["window"].as_u64().unwrap()).unwrap();
    let fun = c["function"].as_str().unwrap();
    assert!(c.get("group_by").is_none() && c.get("order_column").is_none());
    let output: Vec<_> = (0..values.len())
        .map(|i| {
            reduce(
                &values[i.saturating_sub(width - 1)..=i]
                    .iter()
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>(),
                fun,
                usize::try_from(c["ddof"].as_u64().unwrap_or(1)).unwrap(),
                0.5,
            )
        })
        .collect();
    let array: ArrayRef =
        if source.data_type() == &DataType::Int64 && matches!(fun, "sum" | "min" | "max") {
            Arc::new(Int64Array::from(
                output
                    .into_iter()
                    .map(|v| v.map(integer))
                    .collect::<Vec<_>>(),
            ))
        } else {
            Arc::new(Float64Array::from(output))
        };
    put(batch, c["output_column"].as_str().unwrap(), array, true)
}
fn window(c: &Value, batch: &RecordBatch) -> RecordBatch {
    let name = c["column"].as_str().unwrap();
    let source = batch.column_by_name(name).unwrap();
    let values = numbers(source.as_ref());
    let fun = c["function"].as_str().unwrap();
    let offset = usize::try_from(c["offset"].as_u64().unwrap_or(1)).unwrap();
    assert!(c.get("group_by").is_none() && c.get("order_column").is_none());
    let array: ArrayRef = if matches!(fun, "lag" | "lead") {
        take(
            source.as_ref(),
            (0..values.len())
                .map(|i| {
                    if fun == "lag" {
                        i.checked_sub(offset)
                    } else {
                        i.checked_add(offset).filter(|&r| r < values.len())
                    }
                })
                .collect(),
        )
    } else {
        let output: Vec<_> = (0..values.len())
            .map(|i| {
                let current = values[i]?;
                let present: Vec<_> = values.iter().flatten().copied().collect();
                let below = present.iter().filter(|&&v| v < current).count();
                let equal = present
                    .iter()
                    .filter(|&&v| v.to_bits() == current.to_bits())
                    .count();
                match fun {
                    "rank" => Some(count(below) + 1.0 + (count(equal) - 1.0) / 2.0),
                    "dense_rank" => {
                        let unique: BTreeSet<_> = present
                            .iter()
                            .filter(|&&v| v < current)
                            .map(|v| v.to_bits())
                            .collect();
                        Some(count(unique.len() + 1))
                    }
                    "percent_rank" => Some(if present.len() <= 1 {
                        0.0
                    } else {
                        count(below) / count(present.len() - 1)
                    }),
                    "cume_dist" => Some(count(below + equal) / count(present.len())),
                    "cumsum" => reduce(
                        &values[..=i].iter().flatten().copied().collect::<Vec<_>>(),
                        "sum",
                        1,
                        0.5,
                    ),
                    "running_mean" => reduce(
                        &values[..=i].iter().flatten().copied().collect::<Vec<_>>(),
                        "mean",
                        1,
                        0.5,
                    ),
                    "cumcount" => Some(count(i)),
                    "pct_change" => {
                        let prev = values.get(i.checked_sub(1)?).copied().flatten()?;
                        if prev == 0.0 {
                            None
                        } else {
                            Some((current - prev) / prev)
                        }
                    }
                    _ => panic!("window fuori corpus"),
                }
            })
            .collect();
        if fun == "cumsum" && source.data_type() == &DataType::Int64 {
            Arc::new(Int64Array::from(
                output
                    .into_iter()
                    .map(|v| v.map(integer))
                    .collect::<Vec<_>>(),
            ))
        } else {
            Arc::new(Float64Array::from(output))
        }
    };
    put(
        batch,
        c["output_column"]
            .as_str()
            .unwrap_or(&format!("{name}_{fun}")),
        array,
        true,
    )
}
#[allow(clippy::suboptimal_flops)] // Gli estremi seguono le operazioni separate della scheda.
fn bin(c: &Value, batch: &RecordBatch) -> RecordBatch {
    let name = c["column"].as_str().unwrap();
    let values = numbers(batch.column_by_name(name).unwrap().as_ref());
    assert_eq!(c["bins"], 2, "corpus due classi");
    let min = values
        .iter()
        .flatten()
        .copied()
        .min_by(f64::total_cmp)
        .unwrap();
    let max = values
        .iter()
        .flatten()
        .copied()
        .max_by(f64::total_cmp)
        .unwrap();
    assert!(min < max);
    let edges = [min - (max - min) * 0.001, min + (max - min) / 2.0, max];
    let labels = [
        format!("({}, {}]", edges[0], edges[1]),
        format!("({}, {}]", edges[1], edges[2]),
    ];
    let output: Vec<_> = values
        .into_iter()
        .map(|v| v.map(|v| labels[usize::from(v > edges[1])].clone()))
        .collect();
    put(
        batch,
        c["output_column"]
            .as_str()
            .unwrap_or(&format!("{name}_bin")),
        Arc::new(StringArray::from(output)),
        true,
    )
}
pub fn atteso(op: &str, c: &Value, inputs: &[RecordBatch]) -> Option<RecordBatch> {
    let batch = &inputs[0];
    assert_eq!(batch.num_rows(), 6);
    Some(match op {
        "table.aggregate" => aggregate(c, batch),
        "table.statistics" => statistics(c, batch),
        "table.rolling_window" => rolling(c, batch),
        "table.window_function" => window(c, batch),
        "table.bin" => bin(c, batch),
        _ => return None,
    })
}
