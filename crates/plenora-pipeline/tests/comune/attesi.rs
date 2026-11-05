//! Oracolo delle sole fixture CASI: valori e schemi dalle schede, senza kernel.
use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array,
    UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use std::sync::Arc;

fn testo(v: Vec<Option<&str>>) -> ArrayRef {
    Arc::new(StringArray::from(v))
}
fn costante(v: &str) -> ArrayRef {
    testo(vec![Some(v); 6])
}
fn numeri(v: Vec<f64>) -> ArrayRef {
    Arc::new(Float64Array::from(v))
}
fn interi(v: Vec<i64>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}
fn righe(b: &RecordBatch, ids: &[u32]) -> RecordBatch {
    let indices = UInt32Array::from(ids.to_vec());
    RecordBatch::try_new(
        b.schema(),
        b.columns()
            .iter()
            .map(|a| {
                plenora_core::arrow::select::take::take(a.as_ref(), &indices, None)
                    .expect("selezione dell'oracolo")
            })
            .collect(),
    )
    .expect("righe attese")
}

/// Il modello e' deliberatamente chiuso sulle configurazioni di CASI.
#[allow(clippy::too_many_lines)]
pub fn tabella(op: &str, ingressi: &[RecordBatch]) -> RecordBatch {
    let source = &ingressi[0];
    let mut base = source.clone();
    match op {
        "table.sort" => base = righe(source, &[5, 4, 3, 2, 1, 0]),
        "table.top_n" => base = righe(source, &[5, 4, 3]),
        "table.filter" => base = righe(source, &[3, 4, 5]),
        "table.limit" => base = righe(source, &[1, 2, 3, 4]),
        "table.sample" => base = righe(source, &[3, 0, 5]),
        "table.anti_join" | "table.except" => base = source.slice(0, 0),
        "table.concat" | "table.concat_by_name" => {
            base = righe(source, &[0, 1, 2, 3, 4, 5, 0, 1, 2, 3, 4, 5]);
        }
        "table.explode" => base = righe(source, &[0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5]),
        _ => {}
    }
    let mut fields: Vec<Field> = base
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    let mut columns = base.columns().to_vec();
    let mut metadata = base.schema().metadata().clone();
    let mut add: Vec<(&str, bool, ArrayRef)> = Vec::new();
    let alternating = || {
        testo(vec![
            Some("a"),
            Some("b"),
            Some("a"),
            Some("b"),
            Some("a"),
            Some("b"),
        ])
    };
    match op {
        "table.sort"
        | "table.top_n"
        | "table.filter"
        | "table.limit"
        | "table.sample"
        | "table.anti_join"
        | "table.except"
        | "table.concat"
        | "table.concat_by_name"
        | "table.distinct"
        | "table.dedup_advanced"
        | "table.intersect"
        | "table.union_distinct"
        | "table.semi_join"
        | "table.assert_cardinality"
        | "table.assert_foreign_key"
        | "table.assert_metadata"
        | "table.assert_not_null"
        | "table.assert_range"
        | "table.assert_regex"
        | "table.assert_schema"
        | "table.assert_unique" => {}
        "table.add_row_number" => add.push(("row_number", false, interi(vec![1, 2, 3, 4, 5, 6]))),
        "table.aggregate" => {
            fields = vec![
                fields[1].clone(),
                Field::new("value", DataType::Float64, true),
            ];
            columns = vec![
                testo(vec![Some("a"), Some("b")]),
                numeri(vec![103.5, 154.5]),
            ];
        }
        "table.align_schema" => {
            fields = vec![fields[0].clone(), Field::new("note", DataType::Utf8, false)];
            columns = vec![columns[0].clone(), costante("n/d")];
        }
        "table.bin" => add.push((
            "value_bin",
            true,
            testo(vec![
                Some("(0.415, 43]"),
                Some("(0.415, 43]"),
                Some("(0.415, 43]"),
                Some("(43, 85.5]"),
                Some("(43, 85.5]"),
                Some("(43, 85.5]"),
            ]),
        )),
        "table.coalesce" => add.push(("coalesced", true, columns[1].clone())),
        "table.concat_columns" => add.push((
            "joined",
            true,
            testo(vec![
                Some("a|2024-01-02"),
                Some("b|2024-01-02"),
                Some("a|2024-01-02"),
                Some("b|2024-01-02"),
                Some("a|2024-01-02"),
                Some("b|2024-01-02"),
            ]),
        )),
        "table.conditional" => {
            add.push(("conditioned", true, numeri(vec![0., 0., 0., 1., 1., 1.])));
        }
        "table.date_add" => add.push(("date_added", true, costante("2024-01-03 00:00:00"))),
        "table.date_diff" => add.push(("date_delta", true, numeri(vec![1.; 6]))),
        "table.date_extract" => {
            add.push(("date_year", true, interi(vec![2024; 6])));
            add.push(("date_month", true, interi(vec![1; 6])));
        }
        "table.date_format" => add.push(("date_formatted", true, costante("2024-01-02 00:00:00"))),
        "table.drop_columns" => {
            fields.remove(7);
            columns.remove(7);
        }
        "table.expression" => add.push((
            "expression_value",
            true,
            numeri(vec![1.5, 18.5, 35.5, 52.5, 69.5, 86.5]),
        )),
        "table.fill_na" => {
            columns[1] = testo(vec![
                Some("missing"),
                Some("b"),
                Some("a"),
                Some("missing"),
                Some("a"),
                Some("b"),
            ]);
        }
        "table.flatten_json" => add.push(("json_a", true, costante("1"))),
        "table.formula" => add.push((
            "formula_value",
            true,
            numeri(vec![1., 35., 69., 103., 137., 171.]),
        )),
        "table.lookup" => add.push((
            "mapped",
            true,
            testo(vec![
                Some("A"),
                Some("b"),
                Some("A"),
                Some("b"),
                Some("A"),
                Some("b"),
            ]),
        )),
        "table.mask_data" => add.push(("name_masked", true, alternating())),
        "table.md5_hash"
        | "table.sha256_hash"
        | "table.hmac_sha256"
        | "table.stable_fingerprint" => {
            // Hashlib/HMAC Python, framing delle schede; nessun valore dal kernel.
            let hashes: serde_json::Value =
                serde_json::from_str(include_str!("hash_attesi.json")).expect("hash indipendenti");
            let (key, name) = match op {
                "table.md5_hash" => ("md5", "md5"),
                "table.sha256_hash" => ("sha256", "sha256"),
                "table.hmac_sha256" => ("hmac", "hmac"),
                _ => ("fingerprint", "fingerprint"),
            };
            let values: Vec<&str> = hashes[key]
                .as_array()
                .expect("hash")
                .iter()
                .map(|v| v.as_str().expect("hex"))
                .collect();
            add.push((name, false, Arc::new(StringArray::from(values))));
        }
        "table.melt" => {
            fields = vec![
                fields[0].clone(),
                Field::new("variable", DataType::Utf8, false),
                Field::new("value_1", DataType::Float64, true),
            ];
            columns = vec![columns[0].clone(), costante("value"), columns[2].clone()];
        }
        "table.pivot" => {
            fields = vec![
                fields[0].clone(),
                Field::new("value_a", DataType::Float64, true),
                Field::new("value_b", DataType::Float64, true),
            ];
            columns = vec![
                columns[0].clone(),
                Arc::new(Float64Array::from(vec![
                    Some(0.5),
                    None,
                    Some(34.5),
                    None,
                    Some(68.5),
                    None,
                ])),
                Arc::new(Float64Array::from(vec![
                    None,
                    Some(17.5),
                    None,
                    Some(51.5),
                    None,
                    Some(85.5),
                ])),
            ];
        }
        "table.reconcile" => {
            metadata.clear();
            fields = vec![
                Field::new("metric", DataType::Utf8, false),
                Field::new("value", DataType::UInt64, false),
            ];
            columns = vec![
                testo(vec![
                    Some("matched_rows"),
                    Some("left_only_rows"),
                    Some("right_only_rows"),
                    Some("left_duplicate_rows"),
                    Some("right_duplicate_rows"),
                ]),
                Arc::new(UInt64Array::from(vec![6, 0, 0, 0, 0])),
            ];
        }
        "table.rename" => fields[1] = fields[1].clone().with_name("label"),
        "table.reorder_columns" => {
            let f = fields.remove(1);
            fields.insert(0, f);
            let c = columns.remove(1);
            columns.insert(0, c);
        }
        "table.replace" => {
            fields[1] = Field::new("name", DataType::Utf8, true);
            columns[1] = testo(vec![
                Some("z"),
                Some("b"),
                Some("z"),
                Some("b"),
                Some("z"),
                Some("b"),
            ]);
        }
        "table.rolling_window" => add.push((
            "rolling",
            true,
            numeri(vec![0.5, 18., 52.5, 103.5, 154.5, 205.5]),
        )),
        "table.select_columns" => {
            fields = vec![fields[0].clone(), fields[2].clone()];
            columns = vec![columns[0].clone(), columns[2].clone()];
        }
        "table.split_column" => {
            add.push(("year", true, costante("2024")));
            add.push(("month", true, costante("01")));
            add.push(("day", true, costante("02")));
        }
        "table.statistics" => {
            add.push(("value_count", true, numeri(vec![3.; 6])));
            add.push((
                "value_mean",
                true,
                numeri(vec![34.5, 51.5, 34.5, 51.5, 34.5, 51.5]),
            ));
        }
        "table.string_extract" => add.push(("letter", true, alternating())),
        "table.string_length" => add.push(("name_length", true, interi(vec![1; 6]))),
        "table.string_pad" => add.push((
            "padded",
            true,
            testo(vec![
                Some("0000000a"),
                Some("0000000b"),
                Some("0000000a"),
                Some("0000000b"),
                Some("0000000a"),
                Some("0000000b"),
            ]),
        )),
        "table.table_diff" => {
            fields = vec![
                Field::new("id", DataType::Int64, true),
                Field::new("_diff_status", DataType::Utf8, false),
                Field::new("_diff_columns", DataType::Utf8, true),
                Field::new("_diff_old_values", DataType::Utf8, true),
            ];
            columns = vec![interi(vec![]), testo(vec![]), testo(vec![]), testo(vec![])];
        }
        "table.text_normalize" => {
            fields[1] = Field::new("name", DataType::Utf8, true);
            columns[1] = alternating();
        }
        "table.timezone_convert" => add.push(("converted", true, costante("2024-01-02 01:00:00"))),
        "table.type_cast" => {
            fields[0] = Field::new("id", DataType::Utf8, true);
            columns[0] = testo(vec![
                Some("0"),
                Some("1"),
                Some("2"),
                Some("3"),
                Some("4"),
                Some("5"),
            ]);
        }
        "table.uuid_generator" => add.push(("uuid", false, costante("UUID"))),
        "table.validate_rules" => {
            add.push(("_valid", false, Arc::new(BooleanArray::from(vec![true; 6]))));
            add.push(("_errors", false, costante("")));
            add.push(("_warnings", false, costante("")));
        }
        "table.window_function" => add.push(("ranked", true, numeri(vec![1., 2., 3., 4., 5., 6.]))),
        "table.explode" => {
            fields[1] = Field::new("lst", DataType::Int64, true);
            columns[1] = interi(vec![0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6]);
        }
        "table.unnest" => {
            fields.pop();
            columns.pop();
            add.push(("nested_id", true, interi(vec![0, 1, 2, 3, 4, 5])));
            add.push(("nested_name", true, alternating()));
        }
        "table.join" | "table.asof_join" | "table.cross_join" | "table.fuzzy_join" => {
            let pairs: Vec<(u32, u32)> = if op == "table.cross_join" {
                (0..6).flat_map(|i| (0..6).map(move |j| (i, j))).collect()
            } else if op == "table.fuzzy_join" {
                (0..6)
                    .flat_map(|i| (0..6).filter(move |j| j % 2 == i % 2).map(move |j| (i, j)))
                    .collect()
            } else {
                (0..6).map(|i| (i, i)).collect()
            };
            let left = righe(source, &pairs.iter().map(|p| p.0).collect::<Vec<_>>());
            let right = righe(&ingressi[1], &pairs.iter().map(|p| p.1).collect::<Vec<_>>());
            fields = left
                .schema()
                .fields()
                .iter()
                .map(|f| {
                    let name = f.name();
                    let out = match op {
                        "table.join" if name != "id" => format!("{name}_L"),
                        "table.fuzzy_join" if name != "name" => format!("{name}_L"),
                        _ => name.clone(),
                    };
                    f.as_ref().clone().with_name(out).with_nullable(true)
                })
                .collect();
            columns = left.columns().to_vec();
            for (i, f) in right.schema().fields().iter().enumerate() {
                if matches!(op, "table.join" | "table.asof_join") && i == 0 {
                    continue;
                }
                let name = if matches!(op, "table.join" | "table.fuzzy_join") {
                    format!("{}_R", f.name())
                } else {
                    f.name().clone()
                };
                fields.push(f.as_ref().clone().with_name(name).with_nullable(true));
                columns.push(right.column(i).clone());
            }
            if op == "table.fuzzy_join" {
                add.push(("score", false, numeri(vec![1.; 18])));
            }
        }
        _ => panic!("scenario senza atteso indipendente: {op}"),
    }
    for (name, nullable, array) in add {
        fields.push(Field::new(name, array.data_type().clone(), nullable));
        columns.push(array);
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, metadata)),
        columns,
    )
    .expect("batch atteso")
}

/// Confronta valori e schema completo delle fixture tabellari senza geometrie.
pub fn verifica(op: &str, ottenuto: &RecordBatch, atteso: &RecordBatch) {
    verifica_da(op, ottenuto, atteso, 0);
}

/// Senza ID negli ingressi, gli output ricevono ID consecutivi nell'ordine
/// del piano; il cursore continua dal numero di campi degli output precedenti.
pub fn verifica_da(op: &str, ottenuto: &RecordBatch, atteso: &RecordBatch, primo_id: usize) {
    assert_eq!(ottenuto.num_rows(), atteso.num_rows(), "{op}: righe");
    let fields: Vec<Field> = atteso
        .schema()
        .fields()
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut metadata = f.metadata().clone();
            metadata.insert("plenora.field_id".to_owned(), (primo_id + i).to_string());
            f.as_ref().clone().with_metadata(metadata)
        })
        .collect();
    let mut metadata = atteso.schema().metadata().clone();
    metadata.insert("plenora.contract.version".to_owned(), "1".to_owned());
    assert_eq!(
        ottenuto.schema().as_ref(),
        &Schema::new_with_metadata(fields, metadata),
        "{op}: schema completo"
    );
    for (i, (a, b)) in ottenuto.columns().iter().zip(atteso.columns()).enumerate() {
        if op == "table.uuid_generator" && i == 8 {
            let a = a.as_any().downcast_ref::<StringArray>().expect("uuid");
            for text in a {
                let text = text.expect("uuid non nullo");
                assert_eq!(text.len(), 36);
                for (i, c) in text.bytes().enumerate() {
                    if [8, 13, 18, 23].contains(&i) {
                        assert_eq!(c, b'-');
                    } else {
                        assert!(c.is_ascii_hexdigit());
                    }
                }
                assert_eq!(text.as_bytes()[14], b'4');
                assert!(b"89ab".contains(&text.as_bytes()[19]));
            }
        } else {
            assert_eq!(a, b, "{op}: colonna {i}");
        }
    }
}
