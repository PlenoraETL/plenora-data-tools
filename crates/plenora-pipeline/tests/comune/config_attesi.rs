//! Modello del dominio del corpus di `oracolo_config`, senza analizzatori o kernel.
use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::DataType;
use plenora_core::ErrorCategory;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tipo {
    Int,
    Float,
    Text,
    Bool,
    Binary,
    List,
    Struct,
    Null,
}
fn tipo(t: &RecordBatch, name: &str) -> Result<Tipo, ()> {
    let s = t.schema();
    let f = s.field_with_name(name).map_err(|_| ())?;
    Ok(match f.data_type() {
        DataType::Int64 => Tipo::Int,
        DataType::Float64 => Tipo::Float,
        DataType::Utf8 => Tipo::Text,
        DataType::Boolean => Tipo::Bool,
        DataType::Binary => Tipo::Binary,
        DataType::List(_) => Tipo::List,
        DataType::Struct(_) => Tipo::Struct,
        _ => panic!("tipo fuori corpus"),
    })
}
fn text(v: &Value) -> Result<&str, ()> {
    v.as_str().ok_or(())
}
fn arr(v: &Value) -> Result<&Vec<Value>, ()> {
    v.as_array().ok_or(())
}
const fn check(b: bool) -> Result<(), ()> {
    if b {
        Ok(())
    } else {
        Err(())
    }
}
const fn primitive(t: Tipo) -> bool {
    !matches!(t, Tipo::List | Tipo::Struct)
}
const fn numeric(t: Tipo) -> bool {
    matches!(t, Tipo::Int | Tipo::Float | Tipo::Text)
}
const fn ordered(t: Tipo) -> bool {
    matches!(t, Tipo::Int | Tipo::Float | Tipo::Text)
}
fn column(t: &RecordBatch, c: &Value, key: &str) -> Result<Tipo, ()> {
    tipo(t, text(&c[key])?)
}
fn names(t: &RecordBatch, c: &Value, key: &str, scalar: bool) -> Result<(), ()> {
    let ns = arr(&c[key])?;
    check(!ns.is_empty())?;
    let mut seen = std::collections::BTreeSet::new();
    for n in ns {
        let n = text(n)?;
        check(seen.insert(n))?;
        let ty = tipo(t, n)?;
        if scalar {
            check(primitive(ty))?;
        }
    }
    Ok(())
}
#[expect(
    clippy::disallowed_methods,
    reason = "Classificatore indipendente della configurazione della fixture: determina latteso prima di eseguire il prodotto."
)]
fn number(v: &Value) -> bool {
    v.is_number() || v.as_str().is_some_and(|s| s.parse::<f64>().is_ok())
}
#[expect(
    clippy::disallowed_methods,
    reason = "Classificatore indipendente della configurazione della fixture: determina latteso prima di eseguire il prodotto."
)]
fn filter(t: Tipo, c: &Value) -> Result<(), ()> {
    let op = text(&c["operator"])?;
    let v = &c["value"];
    if matches!(op, "isnull" | "notnull") {
        return check(c.get("value").is_none());
    }
    check(!v.is_null())?;
    match op {
        "==" | "!=" => check(if matches!(t, Tipo::Int | Tipo::Float) {
            number(v)
        } else {
            primitive(t)
        }),
        "contains" | "startswith" | "endswith" => check(primitive(t)),
        ">" | ">=" | "<" | "<=" => check(ordered(t) && number(v)),
        "between" => check(
            ordered(t)
                && v.as_str()
                    .and_then(|s| s.split_once(','))
                    .is_some_and(|(a, b)| {
                        a.trim().parse::<f64>().is_ok() && b.trim().parse::<f64>().is_ok()
                    }),
        ),
        _ => panic!("operatore fuori corpus"),
    }
}
fn expression(t: &RecordBatch, e: &Value) -> Result<Tipo, ()> {
    match text(&e["kind"])? {
        "column" => {
            let ty = tipo(t, text(&e["name"])?)?;
            check(primitive(ty))?;
            Ok(if ty == Tipo::Binary { Tipo::Text } else { ty })
        }
        "literal" => Ok(if e["value"].is_null() {
            Tipo::Null
        } else if e["value"].is_number() {
            Tipo::Float
        } else {
            Tipo::Text
        }),
        "binary" => {
            let l = expression(t, &e["left"])?;
            let r = expression(t, &e["right"])?;
            check(matches!(l, Tipo::Int | Tipo::Float) && matches!(r, Tipo::Int | Tipo::Float))?;
            Ok(Tipo::Float)
        }
        "function" => {
            let f = text(&e["name"])?;
            let a = arr(&e["args"])?;
            let ts = a
                .iter()
                .map(|x| expression(t, x))
                .collect::<Result<Vec<_>, _>>()?;
            let literal = |i: usize| {
                a.get(i)
                    .filter(|x| x["kind"] == "literal")
                    .map(|x| &x["value"])
            };
            match f {
                "lower" => check(a.len() == 1 && matches!(ts[0], Tipo::Text | Tipo::Null))?,
                "coalesce" => check(!a.is_empty())?,
                "concat" => {
                    check(a.len() >= 2 && ts.iter().all(|x| matches!(x, Tipo::Text | Tipo::Null)))?;
                }
                "power" => check(a.len() == 2)?,
                "null_if" => {
                    check(a.len() == 2)?;
                    check(
                        ts[0] == ts[1]
                            || matches!(
                                (ts[0], ts[1]),
                                (Tipo::Int, Tipo::Float) | (Tipo::Float, Tipo::Int)
                            ),
                    )?;
                }
                "between" => check(a.len() == 3)?,
                "substring" => {
                    check((2..=3).contains(&a.len()) && matches!(ts[0], Tipo::Text | Tipo::Null))?;
                    for i in 1..a.len() {
                        if let Some(v) = literal(i).filter(|_| !ts.contains(&Tipo::Null)) {
                            check(v.is_null() || v.as_f64().is_some_and(|n| n >= 0.))?;
                        }
                    }
                }
                "regex_replace" => {
                    check(a.len() == 3 && ts.iter().all(|x| matches!(x, Tipo::Text | Tipo::Null)))?;
                    if let Some(v) = literal(1).filter(|_| !ts.contains(&Tipo::Null)) {
                        check(v.is_null() || v.as_str() != Some("("))?;
                    }
                }
                _ => panic!("funzione fuori corpus"),
            }
            Ok(Tipo::Text)
        }
        _ => panic!("espressione fuori corpus"),
    }
}

/// Categoria in validazione, scelta soltanto da config e schema delle fixture.
pub fn validazione(op: &str, c: &Value, inputs: &[RecordBatch]) -> Option<ErrorCategory> {
    if op == "table.transpose" {
        return Some(ErrorCategory::Unsupported);
    }
    match valido(op, c, inputs) {
        Ok(()) => None,
        Err(()) => Some(ErrorCategory::InvalidPlan),
    }
}
#[allow(clippy::too_many_lines, clippy::match_same_arms)] // Una regola esplicita per famiglia.
#[expect(
    clippy::disallowed_methods,
    reason = "Classificatore indipendente della configurazione della fixture: determina latteso prima di eseguire il prodotto."
)]
fn valido(op: &str, c: &Value, inputs: &[RecordBatch]) -> Result<(), ()> {
    let t = &inputs[0];
    let col = |key| column(t, c, key);
    let list = |key, scalar| names(t, c, key, scalar);
    match op {
        "table.add_row_number" => {
            check(c.get("ascending").is_none() && c.get("order_column").is_none())?;
            if c.get("partition_column").is_some() {
                check(primitive(col("partition_column")?))?;
            }
        }
        "table.aggregate" => {
            list("group_by", true)?;
            for a in arr(&c["aggregations"])? {
                let ty = column(t, a, "column")?;
                let f = text(&a["function"])?;
                if f != "count" {
                    check(primitive(ty))?;
                }
                if matches!(f, "sum" | "mean" | "variance" | "stddev" | "quantile") {
                    check(numeric(ty))?;
                }
                if matches!(f, "min" | "max") {
                    check(ordered(ty))?;
                }
                check(a.get("separator").is_none() || f == "concat")?;
                check(
                    a.get("distinct").is_none()
                        || !matches!(f, "count" | "nunique" | "first" | "last"),
                )?;
                check(a.get("skip_null").is_none() || !matches!(f, "count" | "first" | "last"))?;
                check(a.get("ddof").is_none() || matches!(f, "variance" | "stddev"))?;
                check(a.get("quantile").is_none() || f == "quantile")?;
                let output = a["alias"].as_str().unwrap_or(text(&a["column"])?);
                check(
                    !arr(&c["group_by"])?
                        .iter()
                        .any(|g| g.as_str() == Some(output)),
                )?;
            }
        }
        "table.align_schema" => {
            let mut seen = std::collections::BTreeSet::new();
            for a in arr(&c["columns"])? {
                let n = text(&a["name"])?;
                check(seen.insert(n))?;
                if let Ok(ty) = tipo(t, n) {
                    check(matches!(
                        (ty, text(&a["type"])?),
                        (Tipo::Int, "Int64") | (Tipo::Text, "Utf8")
                    ))?;
                }
            }
        }
        "table.anti_join"
        | "table.semi_join"
        | "table.join"
        | "table.assert_foreign_key"
        | "table.reconcile"
        | "table.table_diff" => {
            list("left_keys", true)?;
            names(&inputs[1], c, "right_keys", true)?;
            check(arr(&c["left_keys"])?.len() == arr(&c["right_keys"])?.len())?;
            for (a, b) in arr(&c["left_keys"])?.iter().zip(arr(&c["right_keys"])?) {
                check(tipo(t, text(a)?)? == tipo(&inputs[1], text(b)?)?)?;
            }
        }
        "table.asof_join" => {
            let a = col("left_on")?;
            let b = column(&inputs[1], c, "right_on")?;
            check(matches!(a, Tipo::Int | Tipo::Float) && a == b)?;
            for (i, k) in [(0, "left_by"), (1, "right_by")] {
                if c.get(k).is_some() {
                    names(&inputs[i], c, k, true)?;
                }
            }
        }
        "table.assert_cardinality"
        | "table.limit"
        | "table.sample"
        | "table.uuid_generator"
        | "table.cross_join" => {}
        "table.assert_metadata" => {
            check(t.schema().metadata().get("origine").map(String::as_str) == Some("test"))?;
        }
        "table.assert_not_null" | "table.assert_unique" => {
            list("columns", op == "table.assert_unique")?;
        }
        "table.assert_range" => {
            check(numeric(col("column")?))?;
            check(c.get("min").is_some() || c.get("max").is_some())?;
            check(c.get("inclusive_min").is_none() || c.get("min").is_some())?;
        }
        "table.assert_regex" => check(col("column")? == Tipo::Text)?,
        "table.assert_schema" => {
            for f in arr(&c["fields"])? {
                check(tipo(t, text(&f["name"])?)? == Tipo::Int)?;
            }
        }
        "table.bin" => check(numeric(col("column")?))?,
        "table.coalesce" => list("columns", false)?,
        "table.concat"
        | "table.concat_by_name"
        | "table.except"
        | "table.intersect"
        | "table.union_distinct" => {}
        "table.concat_columns" => {
            list("columns", false)?;
            for n in arr(&c["columns"])? {
                check(tipo(t, text(n)?)? == Tipo::Text)?;
            }
        }
        "table.conditional" => {
            let ty = col("column")?;
            for cond in arr(&c["conditions"])? {
                filter(ty, cond)?;
            }
        }
        "table.filter" => filter(col("column")?, c)?,
        "table.date_add" => {
            check(primitive(col("column")?))?;
            let a = c["amount"].as_i64().ok_or(())?;
            check(a != i64::MIN && a != i64::MAX)?;
        }
        "table.date_format" | "table.timezone_convert" | "table.date_extract" => {
            check(primitive(col("column")?))?;
        }
        "table.date_diff" => {
            check(primitive(col("start_column")?))?;
            check(primitive(col("end_column")?))?;
        }
        "table.dedup_advanced" | "table.distinct" => {
            if c.get("subset").is_some() {
                list("subset", true)?;
            } else {
                for f in t.schema().fields() {
                    check(primitive(tipo(t, f.name())?))?;
                }
            }
            if c.get("order_column").is_some() {
                check(primitive(col("order_column")?))?;
            } else {
                check(c.get("ascending").is_none())?;
            }
        }
        "table.drop_columns" => {}
        "table.explode" => {
            check(col("column")? == Tipo::List)?;
            check(c.get("empty_policy").is_none())?;
        }
        "table.expression" => {
            expression(t, &c["expression"])?;
        }
        "table.fill_na" => {
            if c.get("column").is_some() {
                col("column")?;
            }
            let method = c["method"].as_str().unwrap_or("value");
            check(matches!(method, "value" | "ffill" | "bfill"))?;
            check(method == "value" || c.get("value").is_none())?;
            if c.get("column").is_some() {
                let ty = col("column")?;
                check(matches!(
                    ty,
                    Tipo::Int | Tipo::Float | Tipo::Bool | Tipo::Text
                ))?;
                if method == "value" && !c["value"].is_null() {
                    check(match ty {
                        Tipo::Text => true,
                        Tipo::Int => {
                            c["value"].as_i64().is_some()
                                || c["value"]
                                    .as_str()
                                    .is_some_and(|s| s.parse::<i64>().is_ok())
                        }
                        Tipo::Float => number(&c["value"]),
                        Tipo::Bool => c["value"].is_boolean(),
                        _ => false,
                    })?;
                }
            }
        }
        "table.flatten_json" => {
            check(primitive(col("column")?))?;
            let prefix = format!("{}_", text(&c["column"])?);
            check(
                arr(&c["output_columns"])?
                    .iter()
                    .all(|n| n.as_str().is_some_and(|n| n.starts_with(&prefix))),
            )?;
        }
        "table.formula" => check(numeric(tipo(t, "value")?))?,
        "table.fuzzy_join" => {
            check(col("left_key")? == Tipo::Text)?;
            check(column(&inputs[1], c, "right_key")? == Tipo::Text)?;
        }
        "table.hmac_sha256"
        | "table.md5_hash"
        | "table.sha256_hash"
        | "table.stable_fingerprint" => {
            if c.get("columns").is_some() {
                list("columns", true)?;
            } else {
                for f in t.schema().fields() {
                    check(primitive(tipo(t, f.name())?))?;
                }
            }
        }
        "table.lookup" => check(primitive(col("column")?))?,
        "table.mask_data" => {
            for m in arr(&c["maskings"])? {
                check(primitive(column(t, m, "column")?))?;
                let custom = m["mask_type"].as_str() == Some("custom");
                check(
                    custom
                        || m.get("chars_start").is_none()
                            && m.get("chars_end").is_none()
                            && m.get("mask_char").is_none(),
                )?;
            }
        }
        "table.melt" => {
            list("id_columns", false)?;
            list("value_columns", false)?;
        }
        "table.pivot" => {
            let a = col("index_col")?;
            let b = col("pivot_col")?;
            check(primitive(a) && primitive(b) && numeric(col("value_col")?))?;
            check(b == Tipo::Text)?;
        }
        "table.rename" => {
            for r in arr(&c["renames"])? {
                let new = text(&r["new_name"])?;
                let old = text(&r["old_name"])?;
                check(new != old)?;
                if tipo(t, old).is_ok() {
                    check(matches!(tipo(t, new), Err(())))?;
                }
            }
        }
        "table.reorder_columns" | "table.select_columns" => list("columns", false)?,
        "table.replace" | "table.string_length" | "table.string_pad" => {
            check(col("column")? == Tipo::Text)?;
        }
        "table.rolling_window" => {
            check(numeric(col("column")?))?;
            check(c.get("ddof").is_none() || c["function"] == "stddev")?;
        }
        "table.sort" | "table.top_n" => {
            list("columns", false)?;
            for n in arr(&c["columns"])? {
                check(primitive(tipo(t, text(n)?)?))?;
            }
        }
        "table.split_column" => check(col("column")? == Tipo::Text)?,
        "table.statistics" => {
            check(numeric(col("column")?))?;
            if c.get("group_by").is_some() {
                check(primitive(col("group_by")?))?;
            }
        }
        "table.string_extract" => {
            check(col("column")? == Tipo::Text)?;
            let p = text(&c["pattern"])?;
            check(!p.is_empty())?;
            if p.contains("?P<") {
                check(c.get("output_column").is_none() && c["extract_all"] != true)?;
            }
        }
        "table.text_normalize" => {
            list("columns", false)?;
            for n in arr(&c["columns"])? {
                check(tipo(t, text(n)?)? == Tipo::Text)?;
            }
        }
        "table.type_cast" => {
            let ty = col("column")?;
            check(primitive(ty))?;
            let target = text(&c["target_type"])?;
            check(matches!(
                target,
                "str"
                    | "int"
                    | "float"
                    | "bool"
                    | "date"
                    | "datetime"
                    | "date32"
                    | "timestamp_millis"
                    | "decimal128"
                    | "binary_utf8"
                    | "uint64"
                    | "dictionary_utf8"
            ))?;
            check(
                c.get("date_format").is_none()
                    || matches!(target, "date" | "datetime" | "date32" | "timestamp_millis"),
            )?;
        }
        "table.unnest" => check(col("column")? == Tipo::Struct)?,
        "table.validate_rules" => {
            for r in arr(&c["rules"])? {
                check(matches!(column(t, r, "column")?, Tipo::Int | Tipo::Float))?;
            }
        }
        "table.window_function" => {
            let f = text(&c["function"])?;
            let ty = col("column")?;
            check(
                if matches!(f, "rank" | "dense_rank" | "percent_rank" | "cume_dist") {
                    matches!(ty, Tipo::Int | Tipo::Float)
                } else {
                    numeric(ty)
                },
            )?;
            check(c.get("offset").is_none() || matches!(f, "lag" | "lead"))?;
            check(c["offset"].as_u64() != Some(0))?;
        }
        _ => panic!("operazione non coperta dal modello: {op}"),
    }
    Ok(())
}

/// Categoria in esecuzione delle fixture (sei righe), dopo ammissione statica.
/// I testi delle fixture sono a/b, date ISO, JSON e WKB UTF-8: nessuno e'
/// un numero. Le uniche colonne numeriche sono id/value e rid/rvalue.
#[allow(clippy::too_many_lines, clippy::float_cmp)] // Confini esatti delle fixture, senza tolleranze.
pub fn esecuzione(op: &str, c: &Value, inputs: &[RecordBatch]) -> Option<ErrorCategory> {
    use ErrorCategory::{DataMapping, Schema};
    let t = &inputs[0];
    assert_eq!(
        t.num_rows(),
        6,
        "il modello richiede le fixture di sei righe"
    );
    let column = |key: &str| c[key].as_str().expect("colonna della config ammessa");
    let is_text = |name: &str| tipo(t, name).expect("colonna ammessa") == Tipo::Text;
    let date = |name: &str| matches!(name, "date" | "date2");
    match op {
        "table.aggregate" => arr(&c["aggregations"])
            .expect("aggregazioni")
            .iter()
            .any(|a| {
                matches!(
                    a["function"].as_str(),
                    Some("sum" | "mean" | "min" | "max" | "variance" | "stddev" | "quantile")
                ) && is_text(a["column"].as_str().expect("colonna"))
            })
            .then_some(Schema),
        "table.assert_not_null" => arr(&c["columns"])
            .expect("colonne")
            .iter()
            .any(|n| {
                t.column_by_name(n.as_str().expect("nome"))
                    .expect("colonna")
                    .null_count()
                    > 0
            })
            .then_some(DataMapping),
        "table.assert_unique" => {
            let cols = arr(&c["columns"]).expect("colonne");
            (!cols
                .iter()
                .any(|n| matches!(n.as_str(), Some("id" | "value"))))
            .then_some(DataMapping)
        }
        "table.assert_range" => {
            let name = column("column");
            if is_text(name) {
                Some(Schema)
            } else {
                let min = c["min"].as_f64();
                let max = c["max"].as_f64();
                let values = if name == "id" {
                    [0., 1., 2., 3., 4., 5.]
                } else {
                    [0.5, 17.5, 34.5, 51.5, 68.5, 85.5]
                };
                values
                    .iter()
                    .any(|v| {
                        min.is_some_and(|m| *v < m || *v == m && c["inclusive_min"] == false)
                            || max.is_some_and(|m| *v > m || *v == m && c["inclusive_max"] == false)
                    })
                    .then_some(DataMapping)
            }
        }
        "table.assert_regex" => (column("column") != "name").then_some(DataMapping),
        "table.bin" | "table.statistics" | "table.rolling_window" => {
            is_text(column("column")).then_some(Schema)
        }
        "table.filter" => (is_text(column("column"))
            && matches!(
                c["operator"].as_str(),
                Some(">" | ">=" | "<" | "<=" | "between")
            ))
        .then_some(Schema),
        "table.conditional" => (is_text(column("column"))
            && arr(&c["conditions"]).expect("condizioni").iter().any(|a| {
                matches!(
                    a["operator"].as_str(),
                    Some(">" | ">=" | "<" | "<=" | "between")
                )
            }))
        .then_some(Schema),
        "table.date_add"
        | "table.date_extract"
        | "table.date_format"
        | "table.timezone_convert" => (!date(column("column"))).then_some(DataMapping),
        "table.date_diff" => {
            (!date(column("start_column")) || !date(column("end_column"))).then_some(DataMapping)
        }
        "table.pivot" => is_text(column("value_col")).then_some(Schema),
        "table.window_function" => is_text(column("column")).then_some(Schema),
        "table.type_cast" => {
            let name = column("column");
            let ty = tipo(t, name).expect("tipo");
            let target = column("target_type");
            let good = match target {
                "str" | "binary_utf8" | "dictionary_utf8" => true,
                "int" | "uint64" => ty == Tipo::Int,
                "float" | "decimal128" => matches!(ty, Tipo::Int | Tipo::Float),
                "bool" => ty == Tipo::Bool,
                "date" | "date32" => date(name),
                "datetime" | "timestamp_millis" => date(name) && c.get("date_format").is_none(),
                _ => panic!("target non ammesso dal modello"),
            };
            (!good).then_some(DataMapping)
        }
        "table.add_row_number"
        | "table.align_schema"
        | "table.anti_join"
        | "table.asof_join"
        | "table.assert_cardinality"
        | "table.assert_foreign_key"
        | "table.assert_metadata"
        | "table.assert_schema"
        | "table.coalesce"
        | "table.concat"
        | "table.concat_by_name"
        | "table.concat_columns"
        | "table.cross_join"
        | "table.dedup_advanced"
        | "table.distinct"
        | "table.drop_columns"
        | "table.except"
        | "table.explode"
        | "table.expression"
        | "table.fill_na"
        | "table.flatten_json"
        | "table.formula"
        | "table.fuzzy_join"
        | "table.hmac_sha256"
        | "table.intersect"
        | "table.join"
        | "table.limit"
        | "table.lookup"
        | "table.mask_data"
        | "table.md5_hash"
        | "table.melt"
        | "table.reconcile"
        | "table.rename"
        | "table.reorder_columns"
        | "table.replace"
        | "table.sample"
        | "table.select_columns"
        | "table.semi_join"
        | "table.sha256_hash"
        | "table.sort"
        | "table.split_column"
        | "table.stable_fingerprint"
        | "table.string_extract"
        | "table.string_length"
        | "table.string_pad"
        | "table.table_diff"
        | "table.text_normalize"
        | "table.top_n"
        | "table.union_distinct"
        | "table.unnest"
        | "table.uuid_generator"
        | "table.validate_rules" => None,
        _ => panic!("operazione senza modello d'esecuzione"),
    }
}
