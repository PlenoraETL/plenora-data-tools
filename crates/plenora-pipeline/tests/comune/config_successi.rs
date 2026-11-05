//! Valori attesi per le varianti finite di `oracolo_config`, senza kernel.
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    UInt32Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
#[path = "config_successi_base.rs"]
mod base;
#[allow(dead_code)]
#[path = "attesi.rs"]
mod casi;
#[path = "config_successi_numeri.rs"]
mod numeri;

fn s<'a>(c: &'a Value, key: &str) -> &'a str {
    c[key].as_str().expect("testo config")
}
fn out(c: &Value, key: &str, fallback: &str) -> String {
    c[key].as_str().unwrap_or(fallback).to_owned()
}
fn strings(v: Vec<Option<String>>) -> ArrayRef {
    Arc::new(StringArray::from(v))
}
fn floats(v: Vec<Option<f64>>) -> ArrayRef {
    Arc::new(Float64Array::from(v))
}
fn ints(v: Vec<Option<i64>>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}
fn texts(t: &RecordBatch, name: &str) -> Vec<Option<String>> {
    let a = t.column_by_name(name).expect("colonna input");
    (0..a.len())
        .map(|i| {
            if a.is_null(i) {
                None
            } else {
                Some(match a.data_type() {
                    DataType::Utf8 => a
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap()
                        .value(i)
                        .to_owned(),
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
                    DataType::Binary => std::str::from_utf8(
                        a.as_any().downcast_ref::<BinaryArray>().unwrap().value(i),
                    )
                    .expect("byte fixture UTF-8")
                    .to_owned(),
                    _ => panic!("scalare fuori corpus"),
                })
            }
        })
        .collect()
}
fn numbers(t: &RecordBatch, name: &str) -> Vec<Option<f64>> {
    texts(t, name)
        .into_iter()
        .map(|v| {
            v.map(|v| {
                let n = v.parse::<f64>().expect("numero fixture");
                assert!((0. ..=100.).contains(&n), "dominio numerico del corpus");
                n
            })
        })
        .collect()
}
fn put(t: &RecordBatch, name: &str, array: ArrayRef, nullable: bool) -> RecordBatch {
    let mut fs = t
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect::<Vec<_>>();
    let mut cols = t.columns().to_vec();
    let f = Field::new(name, array.data_type().clone(), nullable);
    if let Some(i) = fs.iter().position(|f| f.name() == name) {
        fs[i] = f;
        cols[i] = array;
    } else {
        fs.push(f);
        cols.push(array);
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fs, t.schema().metadata().clone())),
        cols,
    )
    .expect("schema atteso")
}
fn take(t: &RecordBatch, ids: &[usize]) -> RecordBatch {
    let ids = UInt32Array::from(
        ids.iter()
            .map(|i| u32::try_from(*i).unwrap())
            .collect::<Vec<_>>(),
    );
    RecordBatch::try_new(
        t.schema(),
        t.columns()
            .iter()
            .map(|a| plenora_core::arrow::select::take::take(a.as_ref(), &ids, None).unwrap())
            .collect(),
    )
    .unwrap()
}
fn json_text(v: &Value) -> Option<String> {
    if v.is_null() {
        None
    } else {
        Some(v.as_str().map_or_else(|| v.to_string(), str::to_owned))
    }
}
#[allow(clippy::float_cmp)] // Numeri piccoli esatti: il confronto deve restare esatto.
fn predicate(t: &RecordBatch, name: &str, row: usize, op: &str, value: &Value) -> bool {
    let v = texts(t, name)[row].clone();
    if op == "isnull" {
        return v.is_none();
    }
    if op == "notnull" {
        return v.is_some();
    }
    let Some(v) = v else {
        return false;
    };
    let expected = json_text(value).expect("valore non null");
    let numeric = matches!(
        t.column_by_name(name).unwrap().data_type(),
        DataType::Int64 | DataType::Float64
    );
    match op {
        "==" => {
            if numeric {
                v.parse::<f64>().unwrap() == expected.parse::<f64>().unwrap()
            } else {
                v == expected
            }
        }
        "!=" => {
            if numeric {
                v.parse::<f64>().unwrap() != expected.parse::<f64>().unwrap()
            } else {
                v != expected
            }
        }
        ">" => v.parse::<f64>().unwrap() > expected.parse::<f64>().unwrap(),
        ">=" => v.parse::<f64>().unwrap() >= expected.parse::<f64>().unwrap(),
        "<" => v.parse::<f64>().unwrap() < expected.parse::<f64>().unwrap(),
        "<=" => v.parse::<f64>().unwrap() <= expected.parse::<f64>().unwrap(),
        "contains" => v.contains(&expected),
        "startswith" => v.starts_with(&expected),
        "endswith" => v.ends_with(&expected),
        "between" => {
            let (lo, hi) = expected.split_once(',').unwrap();
            let n = v.parse::<f64>().unwrap();
            n >= lo.parse::<f64>().unwrap() && n <= hi.parse::<f64>().unwrap()
        }
        _ => panic!("predicato fuori corpus"),
    }
}
fn digest_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for byte in Sha256::digest(bytes) {
        write!(text, "{byte:02x}").unwrap();
    }
    text
}
fn frame(v: &str) -> Vec<u8> {
    let mut o = u64::try_from(v.len()).unwrap().to_be_bytes().to_vec();
    o.extend_from_slice(v.as_bytes());
    o
}
fn hash(t: &RecordBatch, op: &str, c: &Value) -> ArrayRef {
    let mut names = c["columns"].as_array().map_or_else(
        || {
            t.schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect::<Vec<_>>()
        },
        |v| v.iter().map(|v| v.as_str().unwrap().to_owned()).collect(),
    );
    if matches!(op, "table.md5_hash" | "table.sha256_hash") {
        names.sort();
    }
    let md5: Value = serde_json::from_str(include_str!("config_md5_attesi.json")).unwrap();
    strings(
        (0..t.num_rows())
            .map(|r| {
                if op == "table.md5_hash" {
                    let msg = names
                        .iter()
                        .map(|n| texts(t, n)[r].clone().unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join("\u{1f}");
                    return Some(
                        md5[&msg]
                            .as_str()
                            .expect("digest MD5 del corpus indipendente")
                            .to_owned(),
                    );
                }
                let tag = match op {
                    "table.sha256_hash" => "plenora-sha256-v1\0",
                    "table.hmac_sha256" => "plenora-hmac-sha256-v1\0",
                    "table.stable_fingerprint" => "plenora-fingerprint-v1\0",
                    _ => panic!("hash"),
                };
                let mut bytes = tag.as_bytes().to_vec();
                for n in &names {
                    bytes.extend(frame(n));
                    bytes.extend(frame(&t.column_by_name(n).unwrap().data_type().to_string()));
                    let value = texts(t, n)[r].clone();
                    if value.is_none() && op == "table.stable_fingerprint" {
                        bytes.push(0);
                    } else {
                        bytes.push(1);
                        bytes.extend(frame(&value.unwrap_or_default()));
                    }
                }
                if op == "table.hmac_sha256" {
                    let key = b"chiave-di-test-del-runner";
                    let mut inner = vec![0x36; 64];
                    let mut outer = vec![0x5c; 64];
                    for (i, b) in key.iter().enumerate() {
                        inner[i] ^= b;
                        outer[i] ^= b;
                    }
                    inner.extend(bytes);
                    outer.extend(Sha256::digest(&inner));
                    Some(digest_hex(&outer))
                } else {
                    Some(digest_hex(&bytes))
                }
            })
            .collect(),
    )
}

#[allow(clippy::too_many_lines)]
pub fn atteso(op: &str, c: &Value, inputs: &[RecordBatch]) -> RecordBatch {
    for input in inputs {
        assert_eq!(input.num_rows(), 6, "dominio di sei righe");
        for field in input.schema().fields() {
            if matches!(field.data_type(), DataType::Int64 | DataType::Float64) {
                let _ = numbers(input, field.name());
            }
            if matches!(field.name().as_str(), "date" | "date2") {
                assert!(texts(input, field.name())
                    .iter()
                    .all(|value| value
                        .as_ref()
                        .is_some_and(|value| value.starts_with("2024-01-")
                            && matches!(&value[8..], "02" | "03"))));
            }
        }
    }
    if let Some(b) = numeri::atteso(op, c, inputs) {
        return b;
    }
    if let Some(b) = base::atteso(op, c, inputs) {
        return b;
    }
    let t = &inputs[0];
    let name = c["column"].as_str().unwrap_or("");
    match op {
        "table.anti_join" | "table.except" => t.slice(0, 0),
        "table.semi_join" | "table.intersect" | "table.union_distinct" => t.clone(),
        "table.distinct" | "table.dedup_advanced" => {
            let b = c["order_column"].as_str().map_or_else(||t.clone(), |order|
                base::atteso("table.sort", &serde_json::json!({"columns":[order],"ascending":c["ascending"].as_bool().unwrap_or(true)}),inputs).unwrap());
            let ns = c["subset"].as_array().map_or_else(
                || {
                    b.schema()
                        .fields()
                        .iter()
                        .map(|f| f.name().clone())
                        .collect::<Vec<_>>()
                },
                |v| v.iter().map(|v| v.as_str().unwrap().to_owned()).collect(),
            );
            let mut seen = Vec::new();
            let ids = (0..b.num_rows())
                .filter(|r| {
                    let key = ns
                        .iter()
                        .map(|n| texts(&b, n)[*r].clone())
                        .collect::<Vec<_>>();
                    if seen.contains(&key) {
                        false
                    } else {
                        seen.push(key);
                        true
                    }
                })
                .collect::<Vec<_>>();
            take(&b, &ids)
        }
        "table.join" | "table.asof_join" | "table.fuzzy_join" => join(t, &inputs[1], op, c),
        "table.reconcile" => {
            let fields = vec![
                Field::new("metric", DataType::Utf8, false),
                Field::new("value", DataType::UInt64, false),
            ];
            RecordBatch::try_new(
                Arc::new(Schema::new(fields)),
                vec![
                    strings(
                        [
                            "matched_rows",
                            "left_only_rows",
                            "right_only_rows",
                            "left_duplicate_rows",
                            "right_duplicate_rows",
                        ]
                        .iter()
                        .map(|v| Some((*v).to_owned()))
                        .collect(),
                    ),
                    Arc::new(plenora_core::arrow::array::UInt64Array::from(vec![
                        6, 0, 0, 0, 0,
                    ])),
                ],
            )
            .unwrap()
        }
        "table.table_diff" => {
            let key = c["left_keys"][0].as_str().unwrap();
            RecordBatch::try_new(
                Arc::new(Schema::new_with_metadata(
                    vec![
                        Field::new(
                            key,
                            t.column_by_name(key).unwrap().data_type().clone(),
                            true,
                        ),
                        Field::new("_diff_status", DataType::Utf8, false),
                        Field::new("_diff_columns", DataType::Utf8, true),
                        Field::new("_diff_old_values", DataType::Utf8, true),
                    ],
                    t.schema().metadata().clone(),
                )),
                vec![
                    t.column_by_name(key).unwrap().slice(0, 0),
                    strings(vec![]),
                    strings(vec![]),
                    strings(vec![]),
                ],
            )
            .unwrap()
        }
        "table.melt" => {
            let id = c["id_columns"][0].as_str().unwrap();
            let value = c["value_columns"][0].as_str().unwrap();
            let fields = vec![
                t.schema().field_with_name(id).unwrap().clone(),
                Field::new("variable", DataType::Utf8, false),
                Field::new(
                    if t.schema().field_with_name("value").is_ok() {
                        "value_1"
                    } else {
                        "value"
                    },
                    t.column_by_name(value).unwrap().data_type().clone(),
                    true,
                ),
            ];
            RecordBatch::try_new(
                Arc::new(Schema::new_with_metadata(
                    fields,
                    t.schema().metadata().clone(),
                )),
                vec![
                    t.column_by_name(id).unwrap().clone(),
                    strings(vec![Some(value.into()); 6]),
                    t.column_by_name(value).unwrap().clone(),
                ],
            )
            .unwrap()
        }
        "table.pivot" => pivot(t, c),
        "table.explode" | "table.unnest" => casi::tabella(op, inputs),
        "table.expression" => {
            let node = &c["expression"];
            let values = (0..t.num_rows())
                .map(|r| eval(t, node, r))
                .collect::<Vec<_>>();
            let numeric = node["kind"] == "binary" || node["name"] == "null_if";
            let arr = if numeric {
                floats(values.iter().map(Value::as_f64).collect())
            } else {
                strings(values.iter().map(json_text).collect())
            };
            put(t, s(c, "output_column"), arr, true)
        }
        "table.type_cast" => type_cast(t, c),
        "table.uuid_generator" => put(
            t,
            &out(c, "output_column", "uuid"),
            strings(vec![Some("UUID".into()); 6]),
            false,
        ),
        "table.validate_rules" => {
            let b = put(
                t,
                "_valid",
                Arc::new(BooleanArray::from(vec![true; 6])),
                false,
            );
            let b = put(&b, "_errors", strings(vec![Some(String::new()); 6]), false);
            put(
                &b,
                "_warnings",
                strings(vec![Some(String::new()); 6]),
                false,
            )
        }
        "table.split_column" => {
            let mut b = t.clone();
            let cols = c["new_columns"].as_array().unwrap();
            let split = texts(t, name)
                .into_iter()
                .map(|v| {
                    v.map(|v| {
                        v.split(s(c, "delimiter"))
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();
            for (i, n) in cols.iter().enumerate() {
                b = put(
                    &b,
                    n.as_str().unwrap(),
                    strings(
                        split
                            .iter()
                            .map(|v| v.as_ref().and_then(|v| v.get(i).cloned()))
                            .collect(),
                    ),
                    true,
                );
            }
            b
        }
        "table.filter" => {
            let ids = (0..t.num_rows())
                .filter(|r| predicate(t, name, *r, s(c, "operator"), &c["value"]))
                .collect::<Vec<_>>();
            take(t, &ids)
        }
        "table.conditional" => {
            let v = (0..t.num_rows())
                .map(|r| {
                    let chosen = c["conditions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|condition| {
                            predicate(t, name, r, s(condition, "operator"), &condition["value"])
                        })
                        .map_or(&c["default_value"], |x| &x["result"]);
                    chosen.as_f64()
                })
                .collect();
            put(t, &out(c, "output_column", "result"), floats(v), true)
        }
        "table.formula" => {
            let v = numbers(t, "value")
                .iter()
                .map(|v| v.map(|x| x * 2.))
                .collect();
            put(t, s(c, "new_column"), floats(v), true)
        }
        "table.concat_columns" => {
            let names = c["columns"].as_array().unwrap();
            let sep = c["separator"].as_str().unwrap_or("");
            let values = (0..t.num_rows())
                .map(|r| {
                    let parts = names
                        .iter()
                        .map(|n| texts(t, n.as_str().unwrap())[r].clone())
                        .collect::<Vec<_>>();
                    if c["skip_null"] != false {
                        Some(parts.into_iter().flatten().collect::<Vec<_>>().join(sep))
                    } else if parts.iter().any(Option::is_none) {
                        None
                    } else {
                        Some(parts.into_iter().flatten().collect::<Vec<_>>().join(sep))
                    }
                })
                .collect();
            put(t, s(c, "output_column"), strings(values), true)
        }
        "table.lookup" => {
            let values = texts(t, name)
                .into_iter()
                .map(|v| {
                    v.and_then(|v| {
                        c["mapping"]
                            .get(&v)
                            .map_or_else(|| json_text(&c["default"]).or(Some(v)), json_text)
                    })
                })
                .collect();
            put(t, &out(c, "output_column", name), strings(values), true)
        }
        "table.replace" => put(
            t,
            name,
            strings(
                texts(t, name)
                    .into_iter()
                    .map(|v| {
                        v.map(|v| {
                            if v == s(c, "old_value") {
                                s(c, "new_value").to_owned()
                            } else {
                                v
                            }
                        })
                    })
                    .collect(),
            ),
            true,
        ),
        "table.string_length" => put(
            t,
            &out(c, "output_column", &format!("{name}_length")),
            ints(
                texts(t, name)
                    .iter()
                    .map(|v| {
                        v.as_ref()
                            .map(|v| i64::try_from(v.chars().count()).unwrap())
                    })
                    .collect(),
            ),
            true,
        ),
        "table.string_pad" => {
            let width = usize::try_from(c["width"].as_u64().unwrap()).unwrap();
            let fill = c["fill_char"].as_str().unwrap_or(" ");
            let values = texts(t, name)
                .into_iter()
                .map(|v| {
                    v.map(|v| {
                        format!(
                            "{}{v}",
                            fill.repeat(width.saturating_sub(v.chars().count()))
                        )
                    })
                })
                .collect();
            put(t, &out(c, "output_column", name), strings(values), true)
        }
        "table.string_extract" => {
            let pat = s(c, "pattern");
            let output = if pat.contains("?P<letter>") {
                "letter".into()
            } else if pat.contains("?P<l>") {
                "l".into()
            } else {
                out(c, "output_column", &format!("{name}_extracted"))
            };
            let values = texts(t, name)
                .into_iter()
                .map(|v| {
                    v.and_then(|v| {
                        let matches = v
                            .chars()
                            .filter(|ch| matches!(ch, 'a' | 'b'))
                            .map(|ch| ch.to_string())
                            .collect::<Vec<_>>();
                        if matches.is_empty() {
                            None
                        } else if c["extract_all"] == true {
                            Some(matches.join(","))
                        } else {
                            Some(matches[0].clone())
                        }
                    })
                })
                .collect();
            put(t, &output, strings(values), true)
        }
        "table.text_normalize" => {
            let mut b = t.clone();
            for n in c["columns"].as_array().unwrap() {
                let name = n.as_str().unwrap();
                let values = texts(t, name)
                    .into_iter()
                    .map(|v| {
                        v.map(|v| {
                            v.trim()
                                .to_ascii_lowercase()
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                    })
                    .collect();
                b = put(&b, name, strings(values), true);
            }
            b
        }
        "table.mask_data" => {
            let mut b = t.clone();
            for m in c["maskings"].as_array().unwrap() {
                let name = s(m, "column");
                let kind = m["mask_type"].as_str().unwrap_or("custom");
                let values = texts(t, name)
                    .into_iter()
                    .map(|v| {
                        v.map(|v| {
                            let (start, end, mask) = match kind {
                                "custom" => (
                                    usize::try_from(m["chars_start"].as_u64().unwrap_or(3))
                                        .unwrap(),
                                    usize::try_from(m["chars_end"].as_u64().unwrap_or(3)).unwrap(),
                                    m["mask_char"].as_str().unwrap_or("*"),
                                ),
                                "cf" => (3, 3, "*"),
                                "iban" => (4, 4, "*"),
                                "email" => return v,
                                "phone" => {
                                    let filtered = v
                                        .chars()
                                        .filter(|c| c.is_ascii_digit() || *c == '+')
                                        .collect::<String>();
                                    if filtered.len() < 6 {
                                        return v;
                                    }
                                    return format!(
                                        "{}{}{}",
                                        &filtered[..3],
                                        "*".repeat(filtered.len().saturating_sub(7)),
                                        &filtered[filtered.len() - 4..]
                                    );
                                }
                                _ => panic!("maschera"),
                            };
                            if v.len() <= start + end {
                                v
                            } else {
                                format!(
                                    "{}{}{}",
                                    &v[..start],
                                    mask.repeat(v.len() - start - end),
                                    &v[v.len() - end..]
                                )
                            }
                        })
                    })
                    .collect();
                let output = if c["overwrite"] == true {
                    name.to_owned()
                } else {
                    format!("{name}_masked")
                };
                b = put(&b, &output, strings(values), true);
            }
            b
        }
        "table.md5_hash"
        | "table.sha256_hash"
        | "table.hmac_sha256"
        | "table.stable_fingerprint" => {
            let fallback = match op {
                "table.md5_hash" => "md5_hash",
                "table.sha256_hash" => "sha256_hash",
                "table.hmac_sha256" => "hmac",
                _ => "fingerprint",
            };
            put(t, &out(c, "output_column", fallback), hash(t, op, c), false)
        }
        "table.flatten_json" => put(t, "json_a", strings(vec![Some("1".into()); 6]), true),
        "table.fill_na" => {
            let a = t.column_by_name(name).unwrap();
            let method = c["method"].as_str().unwrap_or("value");
            if a.data_type() != &DataType::Utf8 {
                return put(t, name, a.clone(), true);
            }
            let mut v = texts(t, name);
            let fill = json_text(&c["value"]);
            let order: Vec<usize> = if method == "bfill" {
                (0..v.len()).rev().collect()
            } else {
                (0..v.len()).collect()
            };
            let mut last = None;
            for i in order {
                if v[i].is_none() {
                    v[i] = if method == "value" {
                        fill.clone()
                    } else {
                        last.clone()
                    };
                } else {
                    last.clone_from(&v[i]);
                }
            }
            put(t, name, strings(v), true)
        }
        "table.date_format" | "table.timezone_convert" => {
            let hour = if op == "table.timezone_convert" {
                "01"
            } else {
                "00"
            };
            put(
                t,
                s(c, "output_column"),
                strings(
                    texts(t, name)
                        .iter()
                        .map(|v| v.as_ref().map(|v| format!("{v} {hour}:00:00")))
                        .collect(),
                ),
                true,
            )
        }
        "table.date_diff" => {
            let values = texts(t, s(c, "start_column"))
                .iter()
                .zip(texts(t, s(c, "end_column")))
                .map(|(a, b)| {
                    Some(f64::from(
                        b.unwrap()[8..].parse::<i32>().unwrap()
                            - a.as_ref().unwrap()[8..].parse::<i32>().unwrap(),
                    ))
                })
                .collect();
            put(t, s(c, "output_column"), floats(values), true)
        }
        "table.date_extract" => {
            let mut b = t.clone();
            for part in c["parts"].as_array().unwrap() {
                let part = part.as_str().unwrap();
                let value = match part {
                    "year" => 2024,
                    "month" => 1,
                    _ => panic!("parte"),
                };
                b = put(
                    &b,
                    &format!("{name}_{part}"),
                    ints(vec![Some(value); 6]),
                    true,
                );
            }
            b
        }
        "table.date_add" => {
            let amount = c["amount"].as_i64().unwrap();
            assert!((-1..=1).contains(&amount), "amount del corpus");
            let unit = s(c, "unit");
            let values = texts(t, name)
                .into_iter()
                .map(|v| {
                    v.map(|v| {
                        let mut year = 2024_i64;
                        let mut month = 1_i64;
                        let mut day = v[8..].parse::<i64>().unwrap();
                        let mut second = 0_i64;
                        match unit {
                            "years" => year += amount,
                            "months" => {
                                month += amount;
                                if month == 0 {
                                    year -= 1;
                                    month = 12;
                                }
                            }
                            "weeks" => day += amount * 7,
                            "days" => day += amount,
                            "hours" => second += amount * 3600,
                            "minutes" => second += amount * 60,
                            "seconds" => second += amount,
                            _ => panic!("unita"),
                        }
                        day += second.div_euclid(86400);
                        second = second.rem_euclid(86400);
                        if day <= 0 {
                            year = 2023;
                            month = 12;
                            day += 31;
                        }
                        format!(
                            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
                            second / 3600,
                            second % 3600 / 60,
                            second % 60
                        )
                    })
                })
                .collect();
            put(t, s(c, "output_column"), strings(values), true)
        }
        _ => panic!("modello valori da completare: {op}"),
    }
}

fn join(l: &RecordBatch, r: &RecordBatch, op: &str, c: &Value) -> RecordBatch {
    let fuzzy = op == "table.fuzzy_join";
    let asof = op == "table.asof_join";
    let ln = if fuzzy {
        s(c, "left_key")
    } else if asof {
        s(c, "left_on")
    } else {
        c["left_keys"][0].as_str().unwrap()
    };
    let rn = if fuzzy {
        s(c, "right_key")
    } else if asof {
        s(c, "right_on")
    } else {
        c["right_keys"][0].as_str().unwrap()
    };
    let lv = texts(l, ln);
    let rv = texts(r, rn);
    // Nel corpus fuzzy il destro contiene soltanto a/b: ogni altro testo
    // sinistro ha Jaro-Winkler sotto 0.8; i testi uguali hanno score 1.
    let pairs = (0..l.num_rows())
        .flat_map(|i| (0..r.num_rows()).map(move |j| (i, j)))
        .filter(|(i, j)| lv[*i].is_some() && lv[*i] == rv[*j])
        .collect::<Vec<_>>();
    let left = take(l, &pairs.iter().map(|p| p.0).collect::<Vec<_>>());
    let right = take(r, &pairs.iter().map(|p| p.1).collect::<Vec<_>>());
    let mut fields = left
        .schema()
        .fields()
        .iter()
        .map(|f| {
            let name = if !asof && f.name() != ln {
                format!("{}_L", f.name())
            } else {
                f.name().clone()
            };
            f.as_ref().clone().with_name(name).with_nullable(true)
        })
        .collect::<Vec<_>>();
    let mut columns = left.columns().to_vec();
    for (i, f) in right.schema().fields().iter().enumerate() {
        if !fuzzy && f.name() == rn {
            continue;
        }
        let name = if asof {
            f.name().clone()
        } else {
            format!("{}_R", f.name())
        };
        fields.push(f.as_ref().clone().with_name(name).with_nullable(true));
        columns.push(right.column(i).clone());
    }
    let b = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            l.schema().metadata().clone(),
        )),
        columns,
    )
    .unwrap();
    if fuzzy {
        put(&b, "score", floats(vec![Some(1.); pairs.len()]), false)
    } else {
        b
    }
}
fn pivot(t: &RecordBatch, c: &Value) -> RecordBatch {
    let index = s(c, "index_col");
    let p = s(c, "pivot_col");
    let value = s(c, "value_col");
    let keys = texts(t, index);
    let mut groups = std::collections::BTreeMap::<Option<String>, Vec<usize>>::new();
    for (r, k) in keys.iter().enumerate() {
        groups
            .entry(k.as_ref().map(|s| format!("{}:{s}", s.len())))
            .or_default()
            .push(r);
    }
    let first = groups.values().map(|g| g[0]).collect::<Vec<_>>();
    let selected = take(t, &first);
    let mut fs = vec![t.schema().field_with_name(index).unwrap().clone()];
    let mut cs = vec![selected.column_by_name(index).unwrap().clone()];
    let pv = texts(t, p);
    let numbers = numbers(t, value);
    let integer = t.column_by_name(value).unwrap().data_type() == &DataType::Int64;
    for (k, out) in c["mapping"].as_object().unwrap() {
        let values = groups
            .values()
            .map(|rows| {
                let vs = rows
                    .iter()
                    .filter(|r| pv[**r].as_ref() == Some(k))
                    .filter_map(|r| numbers[*r])
                    .collect::<Vec<_>>();
                if vs.is_empty() {
                    None
                } else {
                    Some(vs.iter().sum::<f64>())
                }
            })
            .collect::<Vec<_>>();
        let a = if integer {
            ints(
                values
                    .into_iter()
                    .map(|v| v.map(|v| v.to_string().parse::<i64>().unwrap()))
                    .collect(),
            )
        } else {
            floats(values)
        };
        fs.push(Field::new(
            out.as_str().unwrap(),
            a.data_type().clone(),
            true,
        ));
        cs.push(a);
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fs, t.schema().metadata().clone())),
        cs,
    )
    .unwrap()
}
fn eval(table: &RecordBatch, expression: &Value, row: usize) -> Value {
    match s(expression, "kind") {
        "literal" => expression["value"].clone(),
        "column" => {
            let n = s(expression, "name");
            let Some(v) = texts(table, n)[row].clone() else {
                return Value::Null;
            };
            match table.column_by_name(n).unwrap().data_type() {
                DataType::Int64 | DataType::Float64 => serde_json::json!(v.parse::<f64>().unwrap()),
                DataType::Boolean => serde_json::json!(v == "true"),
                _ => serde_json::json!(v),
            }
        }
        "binary" => {
            assert_eq!(expression["op"], "add", "operatore binario del corpus");
            let a = eval(table, &expression["left"], row);
            let b = eval(table, &expression["right"], row);
            if a.is_null() || b.is_null() {
                Value::Null
            } else {
                serde_json::json!(a.as_f64().unwrap() + b.as_f64().unwrap())
            }
        }
        "function" => {
            let a = expression["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|expression| eval(table, expression, row))
                .collect::<Vec<_>>();
            let f = s(expression, "name");
            if a.iter().any(Value::is_null) {
                return Value::Null;
            }
            match f {
                "lower" => serde_json::json!(a[0].as_str().unwrap().to_ascii_lowercase()),
                "concat" => {
                    serde_json::json!(a.iter().map(|a| a.as_str().unwrap()).collect::<String>())
                }
                "substring" => {
                    assert_eq!(a[1].as_f64(), Some(-0.0));
                    a[0].clone()
                }
                "regex_replace" => {
                    assert_eq!(a[1], "(a)");
                    assert_eq!(a[2], "$1");
                    a[0].clone()
                }
                "null_if" => {
                    if a[0].as_f64() == a[1].as_f64() {
                        Value::Null
                    } else {
                        a[0].clone()
                    }
                }
                _ => panic!("fonction hors corpus"),
            }
        }
        _ => panic!("noeud hors corpus"),
    }
}
#[allow(clippy::too_many_lines)] // Un ramo indipendente per tipo del corpus.
fn type_cast(table: &RecordBatch, config: &Value) -> RecordBatch {
    use plenora_core::arrow::array::{
        Date32Array, Decimal128Array, TimestampMillisecondArray, UInt64Array,
    };
    let name = s(config, "column");
    let values = texts(table, name);
    let target = s(config, "target_type");
    let a: ArrayRef = match target {
        "str" | "date" => strings(values),
        "int" => ints(
            values
                .iter()
                .map(|values| values.as_ref().map(|values| values.parse().unwrap()))
                .collect(),
        ),
        "uint64" => Arc::new(UInt64Array::from(
            values
                .iter()
                .map(|values| values.as_ref().map(|values| values.parse::<u64>().unwrap()))
                .collect::<Vec<_>>(),
        )),
        "float" => floats(
            values
                .iter()
                .map(|values| values.as_ref().map(|values| values.parse().unwrap()))
                .collect(),
        ),
        "bool" => Arc::new(BooleanArray::from(
            values
                .iter()
                .map(|values| values.as_ref().map(|values| values == "true"))
                .collect::<Vec<_>>(),
        )),
        "datetime" => strings(
            values
                .into_iter()
                .map(|values| values.map(|values| format!("{values}T00:00:00")))
                .collect(),
        ),
        "date32" => Arc::new(Date32Array::from(
            values
                .iter()
                .map(|values| {
                    values
                        .as_ref()
                        .map(|values| 19722 + values[8..].parse::<i32>().unwrap())
                })
                .collect::<Vec<_>>(),
        )),
        "timestamp_millis" => {
            let values = values
                .iter()
                .map(|values| {
                    values.as_ref().map(|values| {
                        1_704_067_200_000_i64 + values[8..].parse::<i64>().unwrap() * 86_400_000
                            - 86_400_000
                    })
                })
                .collect::<Vec<_>>();
            let a = TimestampMillisecondArray::from(values);
            Arc::new(if let Some(tz) = config["timezone"].as_str() {
                a.with_timezone(tz)
            } else {
                a
            })
        }
        "decimal128" => {
            let vals = values
                .iter()
                .map(|values| {
                    values.as_ref().map(|values| {
                        let (a, b) = values.split_once('.').unwrap_or((values, ""));
                        a.parse::<i128>().unwrap() * 100
                            + format!("{b:0<2}").parse::<i128>().unwrap()
                    })
                })
                .collect::<Vec<_>>();
            Arc::new(
                Decimal128Array::from(vals)
                    .with_precision_and_scale(10, 2)
                    .unwrap(),
            )
        }
        "binary_utf8" => {
            let mut b = plenora_core::arrow::array::builder::BinaryBuilder::new();
            for values in values {
                if let Some(values) = values {
                    b.append_value(values.as_bytes());
                } else {
                    b.append_null();
                }
            }
            Arc::new(b.finish())
        }
        "dictionary_utf8" => {
            let mut b = plenora_core::arrow::array::builder::StringDictionaryBuilder::<
                plenora_core::arrow::array::types::Int32Type,
            >::new();
            for values in values {
                if let Some(values) = values {
                    b.append(values).unwrap();
                } else {
                    b.append_null();
                }
            }
            Arc::new(b.finish())
        }
        _ => panic!("target fuori corpus"),
    };
    put(table, name, a, true)
}

/// Schema e valori completi; UUID e' verificato come `UUIDv4` per contratto.
pub fn verifica(op: &str, c: &Value, ottenuto: &RecordBatch, atteso: &RecordBatch) {
    let fields = atteso
        .schema()
        .fields()
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut m = f.metadata().clone();
            m.insert("plenora.field_id".to_owned(), i.to_string());
            f.as_ref().clone().with_metadata(m)
        })
        .collect::<Vec<_>>();
    let mut m = atteso.schema().metadata().clone();
    m.insert("plenora.contract.version".to_owned(), "1".to_owned());
    assert_eq!(
        *ottenuto.schema(),
        Schema::new_with_metadata(fields, m),
        "schema {op} {c}"
    );
    assert_eq!(ottenuto.num_rows(), atteso.num_rows(), "righe {op} {c}");
    for (i, (a, b)) in ottenuto.columns().iter().zip(atteso.columns()).enumerate() {
        if op == "table.uuid_generator"
            && ottenuto.schema().field(i).name() == c["output_column"].as_str().unwrap_or("uuid")
        {
            let a = a.as_any().downcast_ref::<StringArray>().unwrap();
            for v in a {
                let v = v.expect("uuid presente");
                assert_eq!(v.len(), 36);
                for (i, ch) in v.bytes().enumerate() {
                    if [8, 13, 18, 23].contains(&i) {
                        assert_eq!(ch, b'-');
                    } else {
                        assert!(ch.is_ascii_hexdigit());
                    }
                }
                assert_eq!(v.as_bytes()[14], b'4');
                assert!(b"89ab".contains(&v.as_bytes()[19]));
            }
        } else {
            assert_eq!(a, b, "valori {op} {c}, colonna {i}");
        }
    }
}
