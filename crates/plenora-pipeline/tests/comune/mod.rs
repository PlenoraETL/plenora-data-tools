//! Fixture e config rappresentative per operazione, condivise dai test.
//!
//! Le config vengono da `plenora-memory-lab/operations/table_catalog/src/specs.rs`
//! (una per operazione tabellare del catalogo), le tabelle dalle sue
//! `fixtures.rs`, ridotte a poche righe.

#![allow(dead_code)] // Ogni file di test usa solo una parte delle fixture.

use std::sync::Arc;

use plenora_core::arrow::array::builder::{Int64Builder, ListBuilder, StringBuilder};
use plenora_core::arrow::array::{
    ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    StructArray,
};
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema};
use plenora_core::{PlenoraError, Result};
use plenora_kernels_table::{
    aggregation, analysis, cleansing, columns, dates, expressions, filtering, formula, fuzzy,
    governance, joins, quality, reshape, security, setops, strings, utility, Limits,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Righe delle fixture.
pub const RIGHE: usize = 6;

/// Variabile d'ambiente della chiave di `table.hmac_sha256`.
pub const CHIAVE_HMAC: &str = "PLENORA_PIPELINE_TEST_HMAC_KEY";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    Wide,
    Nullable,
    Nested,
    Binary,
    Set,
}

pub struct Caso {
    pub op: &'static str,
    pub fixture: Fixture,
    pub config: &'static str,
}

macro_rules! caso {
    ($op:literal, $fixture:ident, $config:literal) => {
        Caso {
            op: $op,
            fixture: Fixture::$fixture,
            config: $config,
        }
    };
}

/// Una config rappresentativa per ogni operazione tabellare del catalogo.
pub static CASI: &[Caso] = &[
    caso!("table.add_row_number", Wide, "{}"),
    caso!(
        "table.aggregate",
        Wide,
        r#"{"group_by":["name"],"aggregations":[{"column":"value","function":"sum"}]}"#
    ),
    caso!(
        "table.align_schema",
        Wide,
        r#"{"columns":[{"name":"id","type":"Int64"},{"name":"note","type":"Utf8","default":"n/d"}]}"#
    ),
    caso!(
        "table.anti_join",
        Binary,
        r#"{"left_keys":["id"],"right_keys":["rid"]}"#
    ),
    caso!(
        "table.asof_join",
        Binary,
        r#"{"left_on":"id","right_on":"rid"}"#
    ),
    caso!("table.assert_cardinality", Wide, r#"{"min_rows":1}"#),
    caso!(
        "table.assert_foreign_key",
        Binary,
        r#"{"left_keys":["id"],"right_keys":["rid"]}"#
    ),
    caso!(
        "table.assert_metadata",
        Wide,
        r#"{"expected":{"origine":"test"}}"#
    ),
    caso!("table.assert_not_null", Wide, r#"{"columns":["id"]}"#),
    caso!(
        "table.assert_range",
        Wide,
        r#"{"column":"value","min":0,"allow_null":true}"#
    ),
    caso!(
        "table.assert_regex",
        Wide,
        r#"{"column":"name","pattern":"^[ab]$","allow_null":true}"#
    ),
    caso!(
        "table.assert_schema",
        Wide,
        r#"{"fields":[{"name":"id","data_type":"int64","nullable":false}],"allow_extra":true}"#
    ),
    caso!("table.assert_unique", Wide, r#"{"columns":["id"]}"#),
    caso!("table.bin", Wide, r#"{"column":"value","bins":2}"#),
    caso!(
        "table.coalesce",
        Nullable,
        r#"{"columns":["name"],"output_column":"coalesced"}"#
    ),
    caso!("table.concat", Set, "{}"),
    caso!("table.concat_by_name", Set, "{}"),
    caso!(
        "table.concat_columns",
        Wide,
        r#"{"columns":["name","date"],"output_column":"joined","separator":"|"}"#
    ),
    caso!(
        "table.conditional",
        Wide,
        r#"{"column":"value","conditions":[{"operator":">","value":50,"result":1}],"default_value":0,"output_column":"conditioned"}"#
    ),
    caso!("table.cross_join", Binary, "{}"),
    caso!(
        "table.date_add",
        Wide,
        r#"{"column":"date","input_format":"%Y-%m-%d","amount":1,"unit":"days","output_column":"date_added"}"#
    ),
    caso!(
        "table.date_diff",
        Wide,
        r#"{"start_column":"date","end_column":"date2","input_format":"%Y-%m-%d","unit":"days","output_column":"date_delta"}"#
    ),
    caso!(
        "table.date_extract",
        Wide,
        r#"{"column":"date","parts":["year","month"]}"#
    ),
    caso!(
        "table.date_format",
        Wide,
        r#"{"column":"date","input_format":"%Y-%m-%d","output_column":"date_formatted"}"#
    ),
    caso!("table.dedup_advanced", Wide, r#"{"subset":["id"]}"#),
    caso!("table.distinct", Wide, r#"{"subset":["id"]}"#),
    caso!("table.drop_columns", Wide, r#"{"columns":["geom"]}"#),
    caso!("table.except", Set, "{}"),
    caso!("table.explode", Nested, r#"{"column":"lst"}"#),
    caso!(
        "table.expression",
        Wide,
        r#"{"output_column":"expression_value","expression":{"kind":"binary","op":"add","left":{"kind":"column","name":"value"},"right":{"kind":"literal","value":1}}}"#
    ),
    caso!(
        "table.fill_na",
        Nullable,
        r#"{"column":"name","value":"missing"}"#
    ),
    caso!(
        "table.filter",
        Wide,
        r#"{"column":"value","operator":">","value":50}"#
    ),
    caso!(
        "table.flatten_json",
        Wide,
        r#"{"column":"json","output_columns":["json_a"]}"#
    ),
    caso!(
        "table.formula",
        Wide,
        r#"{"new_column":"formula_value","formula":"value * 2"}"#
    ),
    caso!(
        "table.fuzzy_join",
        Binary,
        r#"{"left_key":"name","right_key":"rname","metric":"jaro_winkler","threshold":0.8,"blocking":"none","max_candidates":300}"#
    ),
    caso!(
        "table.hmac_sha256",
        Wide,
        r#"{"columns":["id","name"],"key_env":"PLENORA_PIPELINE_TEST_HMAC_KEY"}"#
    ),
    caso!("table.intersect", Set, "{}"),
    caso!(
        "table.join",
        Binary,
        r#"{"left_keys":["id"],"right_keys":["rid"],"how":"inner"}"#
    ),
    caso!("table.limit", Wide, r#"{"n":4,"offset":1}"#),
    caso!(
        "table.lookup",
        Wide,
        r#"{"column":"name","mapping":{"a":"A"},"output_column":"mapped"}"#
    ),
    caso!(
        "table.mask_data",
        Wide,
        r#"{"maskings":[{"column":"name"}],"overwrite":false}"#
    ),
    caso!(
        "table.md5_hash",
        Wide,
        r#"{"columns":["id","name"],"output_column":"md5"}"#
    ),
    caso!(
        "table.melt",
        Wide,
        r#"{"id_columns":["id"],"value_columns":["value"]}"#
    ),
    caso!(
        "table.pivot",
        Wide,
        r#"{"index_col":"id","pivot_col":"name","value_col":"value","aggr_func":"sum","mapping":{"a":"value_a","b":"value_b"}}"#
    ),
    caso!(
        "table.reconcile",
        Binary,
        r#"{"left_keys":["id"],"right_keys":["rid"]}"#
    ),
    caso!(
        "table.rename",
        Wide,
        r#"{"renames":[{"old_name":"name","new_name":"label"}]}"#
    ),
    caso!(
        "table.reorder_columns",
        Wide,
        r#"{"columns":["name","id"]}"#
    ),
    caso!(
        "table.replace",
        Wide,
        r#"{"column":"name","old_value":"a","new_value":"z"}"#
    ),
    caso!(
        "table.rolling_window",
        Wide,
        r#"{"column":"value","function":"sum","window":3,"output_column":"rolling"}"#
    ),
    caso!("table.sample", Wide, r#"{"n":3,"random_state":42}"#),
    caso!(
        "table.select_columns",
        Wide,
        r#"{"columns":["id","value"]}"#
    ),
    caso!(
        "table.semi_join",
        Binary,
        r#"{"left_keys":["id"],"right_keys":["rid"]}"#
    ),
    caso!(
        "table.sha256_hash",
        Wide,
        r#"{"columns":["id","name"],"output_column":"sha256"}"#
    ),
    caso!(
        "table.sort",
        Wide,
        r#"{"columns":["value"],"ascending":false}"#
    ),
    caso!(
        "table.split_column",
        Wide,
        r#"{"column":"date","delimiter":"-","new_columns":["year","month","day"]}"#
    ),
    caso!(
        "table.stable_fingerprint",
        Wide,
        r#"{"columns":["id","name"],"algorithm":"sha256"}"#
    ),
    caso!(
        "table.statistics",
        Wide,
        r#"{"column":"value","group_by":"name","stats":["count","mean"]}"#
    ),
    caso!(
        "table.string_extract",
        Wide,
        r#"{"column":"name","pattern":"(?P<letter>[ab])"}"#
    ),
    caso!(
        "table.string_length",
        Wide,
        r#"{"column":"name","output_column":"name_length"}"#
    ),
    caso!(
        "table.string_pad",
        Wide,
        r#"{"column":"name","width":8,"side":"left","fill_char":"0","output_column":"padded"}"#
    ),
    caso!(
        "table.table_diff",
        Binary,
        r#"{"left_keys":["id"],"right_keys":["rid"]}"#
    ),
    caso!(
        "table.text_normalize",
        Wide,
        r#"{"columns":["name"],"operations":"full","overwrite":true}"#
    ),
    caso!(
        "table.timezone_convert",
        Wide,
        r#"{"column":"date","input_format":"%Y-%m-%d","source_timezone":"UTC","target_timezone":"Europe/Rome","output_column":"converted"}"#
    ),
    caso!(
        "table.top_n",
        Wide,
        r#"{"columns":["value"],"n":3,"descending":true}"#
    ),
    caso!(
        "table.transpose",
        Wide,
        r#"{"output_columns":["r0","r1","r2","r3","r4","r5"],"type_policy":"string"}"#
    ),
    caso!(
        "table.type_cast",
        Wide,
        r#"{"column":"id","target_type":"str"}"#
    ),
    caso!("table.union_distinct", Set, "{}"),
    caso!("table.unnest", Nested, r#"{"column":"st"}"#),
    caso!("table.uuid_generator", Wide, r#"{"output_column":"uuid"}"#),
    caso!(
        "table.validate_rules",
        Wide,
        r#"{"rules":[{"name":"id_nonnegative","operator":"ge","column":"id","value":0}]}"#
    ),
    caso!(
        "table.window_function",
        Wide,
        r#"{"column":"value","function":"rank","output_column":"ranked"}"#
    ),
];

fn testo_alternato(righe: usize, con_null: bool) -> Vec<Option<&'static str>> {
    (0..righe)
        .map(|indice| {
            if con_null && indice % 3 == 0 {
                None
            } else if indice % 2 == 0 {
                Some("a")
            } else {
                Some("b")
            }
        })
        .collect()
}

fn interi(righe: usize) -> Vec<i64> {
    (0..righe)
        .map(|indice| i64::try_from(indice).expect("indice"))
        .collect()
}

pub fn wide(con_null: bool) -> RecordBatch {
    let valori: Vec<f64> = (0..RIGHE)
        .map(|indice| f64::from(u32::try_from(indice * 17 % 100).expect("valore")) + 0.5)
        .collect();
    let metadati = [("origine".to_owned(), "test".to_owned())].into();
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, con_null),
                Field::new("value", DataType::Float64, false),
                Field::new("flag", DataType::Boolean, false),
                Field::new("date", DataType::Utf8, false),
                Field::new("date2", DataType::Utf8, false),
                Field::new("json", DataType::Utf8, false),
                Field::new("geom", DataType::Binary, false),
            ],
            metadati,
        )),
        vec![
            Arc::new(Int64Array::from(interi(RIGHE))),
            Arc::new(StringArray::from(testo_alternato(RIGHE, con_null))),
            Arc::new(Float64Array::from(valori)),
            Arc::new(BooleanArray::from(
                (0..RIGHE).map(|indice| indice % 2 == 0).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(vec![Some("2024-01-02"); RIGHE])),
            Arc::new(StringArray::from(vec![Some("2024-01-03"); RIGHE])),
            Arc::new(StringArray::from(vec![Some("{\"a\":1}"); RIGHE])),
            Arc::new(BinaryArray::from(
                (0..RIGHE)
                    .map(|indice| {
                        if indice % 2 == 0 {
                            Some(b"wkb-a".as_slice())
                        } else {
                            Some(b"wkb-b".as_slice())
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("fixture wide")
}

pub fn destra() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("rid", DataType::Int64, false),
            Field::new("rname", DataType::Utf8, false),
            Field::new("rvalue", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(interi(RIGHE))),
            Arc::new(StringArray::from(testo_alternato(RIGHE, false))),
            Arc::new(Float64Array::from(
                (0..RIGHE)
                    .map(|indice| f64::from(u32::try_from(indice).expect("valore")) + 10.0)
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("fixture destra")
}

pub fn nested() -> RecordBatch {
    let mut liste = ListBuilder::new(Int64Builder::new());
    let mut testi = StringBuilder::new();
    for valore in interi(RIGHE) {
        liste.values().append_value(valore);
        liste.values().append_value(valore + 1);
        liste.append(true);
        testi.append_value(if valore % 2 == 0 { "a" } else { "b" });
    }
    let colonna_lista: ArrayRef = Arc::new(liste.finish());
    let struttura: ArrayRef = Arc::new(StructArray::new(
        Fields::from(vec![
            Field::new("nested_id", DataType::Int64, false),
            Field::new("nested_name", DataType::Utf8, false),
        ]),
        vec![
            Arc::new(Int64Array::from(interi(RIGHE))),
            Arc::new(testi.finish()),
        ],
        None,
    ));
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("lst", colonna_lista.data_type().clone(), false),
            Field::new("st", struttura.data_type().clone(), false),
        ])),
        vec![
            Arc::new(Int64Array::from(interi(RIGHE))),
            colonna_lista,
            struttura,
        ],
    )
    .expect("fixture nested")
}

/// Tabelle d'ingresso di una fixture: una per le unarie, due per le binarie.
pub fn tabelle(fixture: Fixture) -> Vec<RecordBatch> {
    match fixture {
        Fixture::Wide => vec![wide(false)],
        Fixture::Nullable => vec![wide(true)],
        Fixture::Nested => vec![nested()],
        Fixture::Binary => vec![wide(false), destra()],
        Fixture::Set => vec![wide(false), wide(false)],
    }
}

pub fn nomi_input(numero: usize) -> Vec<String> {
    ["sinistra", "destra"]
        .iter()
        .take(numero)
        .map(|nome| (*nome).to_owned())
        .collect()
}

fn de<T: DeserializeOwned>(config: &Value) -> Result<T> {
    T::deserialize(config)
        .map_err(|errore| PlenoraError::InvalidPlan(format!("config non valida: {errore}")))
}

/// La chiamata diretta del kernel, senza runner e senza analisi: l'oracolo
/// di ogni passo. Una config che non si deserializza e' `InvalidPlan`.
pub fn chiamata_diretta(op: &str, config: &Value, ingressi: &[RecordBatch]) -> Result<RecordBatch> {
    let limits = &Limits::default();
    match ingressi {
        [uno] => match op {
            "table.drop_columns" => columns::drop_columns(uno, &de(config)?),
            "table.rename" => columns::rename(uno, &de(config)?),
            "table.reorder_columns" => columns::reorder_columns(uno, &de(config)?),
            "table.select_columns" => columns::select_columns(uno, &de(config)?),
            "table.align_schema" => columns::align_schema(uno, &de(config)?),
            "table.concat_columns" => columns::concat_columns(uno, &de(config)?, limits),
            "table.split_column" => columns::split_column(uno, &de(config)?, limits),
            "table.string_pad" => strings::string_pad(uno, &de(config)?, limits),
            "table.string_length" => strings::string_length(uno, &de(config)?),
            "table.text_normalize" => strings::text_normalize(uno, &de(config)?, limits),
            "table.fill_na" => cleansing::fill_na(uno, &de(config)?),
            "table.replace" => cleansing::replace(uno, &de(config)?),
            "table.type_cast" => cleansing::type_cast(uno, &de(config)?),
            "table.filter" => filtering::filter(uno, &de(config)?),
            "table.conditional" => filtering::conditional(uno, &de(config)?),
            "table.string_extract" => strings::string_extract(uno, &de(config)?, limits),
            "table.date_extract" => utility::date_extract(uno, &de(config)?),
            "table.uuid_generator" => utility::uuid_generator(uno, &de(config)?),
            "table.limit" => utility::limit(uno, &de(config)?),
            "table.lookup" => analysis::lookup(uno, &de(config)?),
            "table.flatten_json" => analysis::flatten_json(uno, &de(config)?, limits),
            "table.mask_data" => security::mask_data(uno, &de(config)?),
            "table.md5_hash" => security::md5_hash(uno, &de(config)?),
            "table.add_row_number" => utility::add_row_number(uno, &de(config)?),
            "table.bin" => analysis::bin(uno, &de(config)?),
            "table.sample" => analysis::sample(uno, &de(config)?),
            "table.statistics" => analysis::statistics(uno, &de(config)?),
            "table.sort" => aggregation::sort(uno, &de(config)?),
            "table.top_n" => aggregation::top_n(uno, &de(config)?),
            "table.distinct" => aggregation::distinct(uno, &de(config)?),
            "table.dedup_advanced" => aggregation::dedup_advanced(uno, &de(config)?),
            "table.aggregate" => aggregation::aggregate(uno, &de(config)?),
            "table.window_function" => aggregation::window_function(uno, &de(config)?),
            "table.rolling_window" => aggregation::rolling_window(uno, &de(config)?),
            "table.melt" => reshape::melt(uno, &de(config)?, limits),
            "table.pivot" => reshape::pivot(uno, &de(config)?, limits),
            "table.transpose" => reshape::transpose(uno, &de(config)?, limits),
            "table.formula" => formula::formula(uno, &de(config)?),
            "table.expression" => expressions::expression(uno, &de(config)?),
            "table.assert_cardinality" => governance::assert_cardinality(uno, &de(config)?),
            "table.assert_metadata" => governance::assert_metadata(uno, &de(config)?),
            "table.assert_schema" => quality::assert_schema(uno, &de(config)?),
            "table.assert_not_null" => quality::assert_not_null(uno, &de(config)?),
            "table.assert_unique" => quality::assert_unique(uno, &de(config)?),
            "table.assert_range" => quality::assert_range(uno, &de(config)?),
            "table.assert_regex" => quality::assert_regex(uno, &de(config)?),
            "table.coalesce" => quality::coalesce(uno, &de(config)?),
            "table.date_format" => dates::date_format(uno, &de(config)?),
            "table.date_add" => dates::date_add(uno, &de(config)?),
            "table.date_diff" => dates::date_diff(uno, &de(config)?),
            "table.timezone_convert" => dates::timezone_convert(uno, &de(config)?),
            "table.sha256_hash" => security::sha256_hash(uno, &de(config)?),
            "table.stable_fingerprint" => security::stable_fingerprint(uno, &de(config)?),
            "table.hmac_sha256" => security::hmac_sha256(uno, &de(config)?),
            "table.validate_rules" => governance::validate_rules(uno, &de(config)?),
            "table.explode" => reshape::explode(uno, &de(config)?, limits),
            "table.unnest" => reshape::unnest(uno, &de(config)?, limits),
            altra => panic!("{altra}: non unaria"),
        },
        [sinistra, destra] => match op {
            "table.join" => joins::join(sinistra, destra, &de(config)?, limits),
            "table.concat" => joins::concat(sinistra, destra, &de(config)?, limits),
            "table.concat_by_name" => {
                joins::concat_by_name(&[sinistra, destra], &de(config)?, limits)
            }
            "table.cross_join" => joins::cross_join(sinistra, destra, &de(config)?, limits),
            "table.table_diff" => reshape::table_diff(sinistra, destra, &de(config)?, limits),
            "table.semi_join" => joins::semi_join(sinistra, destra, &de(config)?),
            "table.anti_join" => joins::anti_join(sinistra, destra, &de(config)?),
            "table.asof_join" => joins::asof_join(sinistra, destra, &de(config)?, limits),
            "table.fuzzy_join" => fuzzy::fuzzy_join(sinistra, destra, &de(config)?, limits),
            "table.union_distinct" => {
                setops::union_distinct(sinistra, destra, &de(config)?, limits)
            }
            "table.intersect" => setops::intersect(sinistra, destra, &de(config)?),
            "table.except" => setops::except(sinistra, destra, &de(config)?),
            "table.assert_foreign_key" => {
                governance::assert_foreign_key(sinistra, destra, &de(config)?, limits)
            }
            "table.reconcile" => governance::reconcile(sinistra, destra, &de(config)?, limits),
            altra => panic!("{altra}: non binaria"),
        },
        _ => panic!("arieta' non prevista"),
    }
}
