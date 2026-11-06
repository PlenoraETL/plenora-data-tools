//! Modello indipendente delle operazioni strutturali del corpus di config.
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    UInt32Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde_json::Value;
use std::{cmp::Ordering, collections::BTreeMap, sync::Arc};

fn rows(b: &RecordBatch, ids: &[usize]) -> RecordBatch {
    let indices = UInt32Array::from(
        ids.iter()
            .map(|&i| u32::try_from(i).unwrap())
            .collect::<Vec<_>>(),
    );
    RecordBatch::try_new(
        b.schema(),
        b.columns()
            .iter()
            .map(|a| plenora_core::arrow::select::take::take(a.as_ref(), &indices, None).unwrap())
            .collect(),
    )
    .unwrap()
}
fn names(c: &Value, key: &str) -> Vec<String> {
    c[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}
fn text(a: &dyn Array, i: usize) -> Option<String> {
    if a.is_null(i) {
        return None;
    }
    Some(match a.data_type() {
        DataType::Int64 => a
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(i)
            .to_string(),
        DataType::Float64 => a
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(i)
            .to_string(),
        DataType::Boolean => a
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap()
            .value(i)
            .to_string(),
        DataType::Utf8 => a
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(i)
            .to_owned(),
        DataType::Binary => String::from_utf8(
            a.as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap()
                .value(i)
                .to_vec(),
        )
        .unwrap(),
        _ => panic!("tipo fuori corpus scalare"),
    })
}
fn cmp(a: &dyn Array, i: usize, j: usize) -> Ordering {
    match (a.is_null(i), a.is_null(j)) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    match a.data_type() {
        DataType::Int64 => {
            let v = a.as_any().downcast_ref::<Int64Array>().unwrap();
            v.value(i).cmp(&v.value(j))
        }
        DataType::Float64 => {
            let v = a.as_any().downcast_ref::<Float64Array>().unwrap();
            v.value(i).total_cmp(&v.value(j))
        }
        DataType::Boolean => {
            let v = a.as_any().downcast_ref::<BooleanArray>().unwrap();
            v.value(i).cmp(&v.value(j))
        }
        DataType::Utf8 => {
            let v = a.as_any().downcast_ref::<StringArray>().unwrap();
            v.value(i).cmp(v.value(j))
        }
        DataType::Binary => {
            let v = a.as_any().downcast_ref::<BinaryArray>().unwrap();
            v.value(i).cmp(v.value(j))
        }
        _ => panic!("tipo non ordinabile nel corpus"),
    }
}
fn build(base: &RecordBatch, fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            base.schema().metadata().clone(),
        )),
        columns,
    )
    .unwrap()
}
fn append(base: &RecordBatch, name: &str, nullable: bool, array: ArrayRef) -> RecordBatch {
    let mut fields: Vec<Field> = base
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    let mut columns = base.columns().to_vec();
    let f = Field::new(name, array.data_type().clone(), nullable);
    if let Some(i) = fields.iter().position(|f| f.name() == name) {
        fields[i] = f;
        columns[i] = array;
    } else {
        fields.push(f);
        columns.push(array);
    }
    build(base, fields, columns)
}

#[allow(clippy::too_many_lines)]
pub fn atteso(op: &str, c: &Value, inputs: &[RecordBatch]) -> Option<RecordBatch> {
    let b = &inputs[0];
    Some(match op {
        "table.assert_cardinality"
        | "table.assert_foreign_key"
        | "table.assert_metadata"
        | "table.assert_not_null"
        | "table.assert_range"
        | "table.assert_regex"
        | "table.assert_schema"
        | "table.assert_unique" => b.clone(),
        "table.add_row_number" => {
            let mut seen = BTreeMap::<Option<String>, i64>::new();
            let values = (0..b.num_rows())
                .map(|i| {
                    let key = c["partition_column"]
                        .as_str()
                        .and_then(|n| text(b.column_by_name(n).unwrap().as_ref(), i));
                    let n = seen
                        .entry(key)
                        .or_insert_with(|| c["start"].as_i64().unwrap_or(1));
                    let v = *n;
                    *n += 1;
                    v
                })
                .collect::<Vec<_>>();
            append(
                b,
                c["output_column"].as_str().unwrap_or("row_number"),
                false,
                Arc::new(Int64Array::from(values)),
            )
        }
        "table.align_schema" => {
            let mut fields = Vec::new();
            let mut columns: Vec<ArrayRef> = Vec::new();
            for spec in c["columns"].as_array().unwrap() {
                let name = spec["name"].as_str().unwrap();
                if let Ok(i) = b.schema().index_of(name) {
                    fields.push(b.schema().field(i).clone());
                    columns.push(b.column(i).clone());
                } else {
                    let ty = match spec["type"].as_str().unwrap() {
                        "Int64" => DataType::Int64,
                        "Utf8" => DataType::Utf8,
                        _ => panic!("type fuori corpus"),
                    };
                    let default = spec.get("default");
                    let array = default.map_or_else(
                        || plenora_core::arrow::array::new_null_array(&ty, b.num_rows()),
                        |v| match ty {
                            DataType::Utf8 => {
                                Arc::new(StringArray::from(vec![v.as_str().unwrap(); b.num_rows()]))
                                    as ArrayRef
                            }
                            _ => panic!("default fuori corpus"),
                        },
                    );
                    fields.push(Field::new(name, ty, default.is_none()));
                    columns.push(array);
                }
            }
            if c["keep_extra"] == true {
                for (i, f) in b.schema().fields().iter().enumerate() {
                    if !fields.iter().any(|out| out.name() == f.name()) {
                        fields.push(f.as_ref().clone());
                        columns.push(b.column(i).clone());
                    }
                }
            }
            build(b, fields, columns)
        }
        "table.coalesce" => {
            let cols = names(c, "columns");
            assert_eq!(cols.len(), 1, "corpus coalesce monocolonna");
            append(
                b,
                c["output_column"].as_str().unwrap(),
                true,
                b.column_by_name(&cols[0]).unwrap().clone(),
            )
        }
        "table.concat" | "table.concat_by_name" => {
            // Le due fixture Set sono identiche; la concatenazione attesa e'
            // due copie in ordine, senza invocare concatenazione di produzione.
            assert_eq!(inputs.len(), 2);
            assert_eq!(inputs[0], inputs[1]);
            rows(
                b,
                &(0..b.num_rows()).chain(0..b.num_rows()).collect::<Vec<_>>(),
            )
        }
        "table.cross_join" => {
            let r = &inputs[1];
            let left = rows(
                b,
                &(0..b.num_rows())
                    .flat_map(|i| std::iter::repeat_n(i, r.num_rows()))
                    .collect::<Vec<_>>(),
            );
            let right = rows(
                r,
                &(0..b.num_rows())
                    .flat_map(|_| 0..r.num_rows())
                    .collect::<Vec<_>>(),
            );
            let mut fields = Vec::new();
            let mut columns = Vec::new();
            for (side, other, suffix) in [(&left, &right, "_x"), (&right, &left, "_y")] {
                for (i, f) in side.schema().fields().iter().enumerate() {
                    let name = if other
                        .schema()
                        .fields()
                        .iter()
                        .any(|other| other.name() == f.name())
                    {
                        format!("{}{suffix}", f.name())
                    } else {
                        f.name().clone()
                    };
                    fields.push(f.as_ref().clone().with_name(name).with_nullable(true));
                    columns.push(side.column(i).clone());
                }
            }
            build(b, fields, columns)
        }
        "table.drop_columns" | "table.select_columns" | "table.reorder_columns" => {
            let ns = names(c, "columns");
            let mut selected = Vec::new();
            if op == "table.drop_columns" {
                selected.extend(
                    b.schema()
                        .fields()
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| !ns.contains(f.name()))
                        .map(|(i, _)| i),
                );
            } else {
                selected.extend(ns.iter().map(|n| b.schema().index_of(n).unwrap()));
                if op == "table.reorder_columns" {
                    for i in 0..b.num_columns() {
                        if !selected.contains(&i) {
                            selected.push(i);
                        }
                    }
                }
            }
            b.project(&selected).unwrap()
        }
        "table.rename" => {
            let fields = b
                .schema()
                .fields()
                .iter()
                .map(|f| {
                    let new = c["renames"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["old_name"].as_str() == Some(f.name()))
                        .and_then(|r| r["new_name"].as_str())
                        .unwrap_or(f.name());
                    f.as_ref().clone().with_name(new)
                })
                .collect();
            build(b, fields, b.columns().to_vec())
        }
        "table.limit" => {
            let start = usize::try_from(c["offset"].as_u64().unwrap_or(0))
                .unwrap()
                .min(b.num_rows());
            let n = usize::try_from(c["n"].as_u64().unwrap()).unwrap();
            b.slice(start, n.min(b.num_rows() - start))
        }
        "table.sort" | "table.top_n" => {
            let cols = names(c, "columns");
            let descending = if op == "table.top_n" {
                c["descending"].as_bool().unwrap_or(true)
            } else {
                !c["ascending"].as_bool().unwrap_or(true)
            };
            let mut ids: Vec<_> = (0..b.num_rows()).collect();
            ids.sort_by(|&i, &j| {
                let order = cols
                    .iter()
                    .map(|name| cmp(b.column_by_name(name).unwrap().as_ref(), i, j))
                    .find(|o| *o != Ordering::Equal)
                    .unwrap_or(Ordering::Equal);
                if descending {
                    order.reverse()
                } else {
                    order
                }
            });
            if op == "table.top_n" {
                ids.truncate(usize::try_from(c["n"].as_u64().unwrap()).unwrap());
            }
            rows(b, &ids)
        }
        "table.sample" => {
            assert_eq!(b.num_rows(), 6);
            assert_eq!(c["n"], 3);
            assert_eq!(c["random_state"], 42);
            rows(b, &[3, 0, 5])
        }
        _ => return None,
    })
}
