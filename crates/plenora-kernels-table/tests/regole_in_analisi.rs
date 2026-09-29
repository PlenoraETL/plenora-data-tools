//! Le regole sulle config stanno nell'analisi dei contratti
//! (`analyze_table_contract`), una volta sola, per ogni chiamante dei
//! kernel: una config accettata non si esegue con un significato diverso da
//! quello scritto e non fallisce in esecuzione per un motivo che config,
//! schema e limiti rendono prevedibile.
//!
//! Qui ogni regola ha un caso rifiutato (con il frammento del messaggio che
//! la identifica, perche' un rifiuto per un altro motivo non conti) e, dove
//! il confine e' sottile, un caso accettato accanto.

use std::sync::Arc;

use plenora_core::arrow::array::new_null_array;
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema, SchemaRef, TimeUnit};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use plenora_kernels_table::analyze::analyze_table_contract;
use plenora_kernels_table::{
    limiti_interni, scalar_compare, scalar_compare_supported, Limits, NumericBound,
};
use serde_json::{json, Value};

fn schema(campi: Vec<Field>) -> SchemaRef {
    Arc::new(Schema::new(campi))
}

/// Colonne di ogni famiglia che le regole distinguono.
fn largo() -> SchemaRef {
    schema(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("value", DataType::Float64, false),
        Field::new("flag", DataType::Boolean, false),
        Field::new("date", DataType::Utf8, false),
        Field::new("json", DataType::Utf8, false),
        Field::new("geom", DataType::Binary, false),
        Field::new("giorno", DataType::Date32, true),
        Field::new(
            "istante",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        ),
        Field::new("importo", DataType::Decimal128(10, 2), true),
        Field::new("grande", DataType::UInt64, true),
    ])
}

fn annidato() -> SchemaRef {
    schema(vec![
        Field::new("id", DataType::Int64, false),
        Field::new(
            "lst",
            DataType::List(Arc::new(Field::new("item", DataType::Int64, true))),
            false,
        ),
        Field::new(
            "st",
            DataType::Struct(Fields::from(vec![Field::new("a", DataType::Int64, false)])),
            false,
        ),
    ])
}

fn analizza(op: &str, schemi: &[&SchemaRef], config: &Value) -> Result<DataContract> {
    let ingressi: Vec<DataContract> = schemi
        .iter()
        .map(|schema| DataContract::tabular((*schema).clone()))
        .collect();
    analyze_table_contract(
        op,
        &ingressi,
        config,
        &mut FieldAllocator::default(),
        &Limits::default(),
    )
}

/// Il rifiuto deve essere un errore di piano (o di risorsa, per le colonne)
/// e nominare la regola.
#[allow(clippy::needless_pass_by_value)] // Config posseduta: `json!` in linea nei casi.
fn rifiuta(op: &str, schemi: &[&SchemaRef], config: Value, frammento: &str) {
    match analizza(op, schemi, &config) {
        Err(PlenoraError::InvalidPlan(messaggio) | PlenoraError::ResourceLimit(messaggio))
            if messaggio.contains(frammento) => {}
        altro => panic!("{op} {config}: atteso il rifiuto `{frammento}`, avuto {altro:?}"),
    }
}

#[allow(clippy::needless_pass_by_value)] // Config posseduta: `json!` in linea nei casi.
fn accetta(op: &str, schemi: &[&SchemaRef], config: Value) {
    if let Err(errore) = analizza(op, schemi, &config) {
        panic!("{op} {config}: atteso accettato, avuto {errore}");
    }
}

#[test]
fn le_chiavi_lette_come_testo_devono_esserlo() {
    let n = annidato();
    let testo = "non leggibile come scalare testuale";
    for op in [
        "table.join",
        "table.semi_join",
        "table.anti_join",
        "table.assert_foreign_key",
        "table.reconcile",
        "table.table_diff",
    ] {
        rifiuta(
            op,
            &[&n, &n],
            json!({"left_keys": ["lst"], "right_keys": ["lst"]}),
            testo,
        );
        if op != "table.table_diff" {
            accetta(
                op,
                &[&n, &n],
                json!({"left_keys": ["id"], "right_keys": ["id"]}),
            );
        }
    }
    rifiuta(
        "table.asof_join",
        &[&n, &n],
        json!({"left_on": "id", "right_on": "id", "left_by": ["lst"], "right_by": ["lst"]}),
        testo,
    );
    rifiuta(
        "table.table_diff",
        &[&n, &n],
        json!({"left_keys": ["id"], "right_keys": ["id"], "compare_columns": ["st"]}),
        testo,
    );
    // Senza compare_columns si confrontano le colonne comuni non chiave: le
    // stesse regole valgono per quelle.
    rifiuta(
        "table.table_diff",
        &[&n, &n],
        json!({"left_keys": ["id"], "right_keys": ["id"]}),
        testo,
    );
    // Liste di chiavi: stessa lunghezza, niente ripetizioni.
    let w = largo();
    for op in ["table.assert_foreign_key", "table.reconcile"] {
        rifiuta(
            op,
            &[&w, &w],
            json!({"left_keys": ["id", "name"], "right_keys": ["id"]}),
            "cardinalita'",
        );
    }
    rifiuta(
        "table.join",
        &[&w, &w],
        json!({"left_keys": ["id", "id"], "right_keys": ["id", "id"]}),
        "ripetuta",
    );
}

#[test]
fn distinct_senza_subset_legge_come_testo_la_riga_intera() {
    let n = annidato();
    rifiuta(
        "table.distinct",
        &[&n],
        json!({}),
        "non leggibile come scalare testuale",
    );
    accetta("table.distinct", &[&n], json!({"subset": ["id"]}));
    rifiuta(
        "table.distinct",
        &[&n],
        json!({"subset": ["id", "id"]}),
        "ripetuta",
    );
}

#[test]
fn gli_operatori_di_filter_e_conditional_seguono_il_tipo_della_colonna() {
    let w = largo();
    let n = annidato();
    // Testuali su colonne che non sono testo.
    for operatore in ["contains", "startswith", "endswith", "==", "!="] {
        rifiuta(
            "table.filter",
            &[&n],
            json!({"column": "st", "operator": operatore, "value": "a"}),
            "non leggibile come scalare testuale",
        );
        rifiuta(
            "table.conditional",
            &[&n],
            json!({"column": "lst", "conditions": [{"operator": operatore, "value": "a",
                   "result": 1}]}),
            "non leggibile come scalare testuale",
        );
    }
    // Ordinati: solo i tipi di `scalar_compare`.
    for operatore in [">", ">=", "<", "<="] {
        for colonna in ["flag", "geom"] {
            rifiuta(
                "table.filter",
                &[&w],
                json!({"column": colonna, "operator": operatore, "value": 1}),
                "nessun confronto ordinato",
            );
        }
        for colonna in [
            "id", "name", "value", "giorno", "istante", "importo", "grande",
        ] {
            accetta(
                "table.filter",
                &[&w],
                json!({"column": colonna, "operator": operatore, "value": 1}),
            );
        }
    }
    rifiuta(
        "table.conditional",
        &[&w],
        json!({"column": "flag", "conditions": [{"operator": "between", "value": "0,1",
               "result": 1}]}),
        "nessun confronto ordinato",
    );
    // `==`/`!=` numerici in conditional come in filter.
    rifiuta(
        "table.conditional",
        &[&w],
        json!({"column": "value", "conditions": [{"operator": "==", "value": "abc",
               "result": 1}]}),
        "valore non numerico",
    );
    accetta(
        "table.conditional",
        &[&w],
        json!({"column": "name", "conditions": [{"operator": "==", "value": "abc",
               "result": 1}]}),
    );
    // Senza condizioni la config non esprime niente.
    rifiuta(
        "table.conditional",
        &[&w],
        json!({"column": "value", "conditions": [], "default_value": 0}),
        "almeno una condizione",
    );
}

#[test]
fn scalar_compare_supported_e_l_elenco_di_scalar_compare() {
    // Oracolo: per ogni tipo, il predicato dice di no esattamente quando
    // `scalar_compare` rifiuta il tipo (il testo non numerico di una cella
    // Utf8 e' un errore dei dati, non del tipo).
    let tipi = [
        DataType::Null,
        DataType::Boolean,
        DataType::Int8,
        DataType::Int16,
        DataType::Int32,
        DataType::Int64,
        DataType::UInt8,
        DataType::UInt16,
        DataType::UInt32,
        DataType::UInt64,
        DataType::Float32,
        DataType::Float64,
        DataType::Date32,
        DataType::Date64,
        DataType::Timestamp(TimeUnit::Second, None),
        DataType::Timestamp(TimeUnit::Millisecond, None),
        DataType::Timestamp(TimeUnit::Millisecond, Some("Europe/Rome".into())),
        DataType::Timestamp(TimeUnit::Microsecond, None),
        DataType::Timestamp(TimeUnit::Nanosecond, None),
        DataType::Decimal128(10, 2),
        DataType::Decimal256(10, 2),
        DataType::Utf8,
        DataType::LargeUtf8,
        DataType::Binary,
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
        DataType::List(Arc::new(Field::new("item", DataType::Int64, true))),
        DataType::Struct(Fields::from(vec![Field::new("a", DataType::Int64, true)])),
    ];
    for tipo in tipi {
        let colonna = new_null_array(&tipo, 1);
        let rifiuta_il_tipo = matches!(
            scalar_compare(colonna.as_ref(), 0, NumericBound::I64(0)),
            Err(PlenoraError::Schema(messaggio)) if messaggio.contains("non confrontabile")
        );
        assert_eq!(
            scalar_compare_supported(&tipo),
            !rifiuta_il_tipo,
            "{tipo:?}"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Un caso per regola.
fn i_parametri_che_il_kernel_ignorerebbe_si_rifiutano() {
    let w = largo();
    // type_cast: parametri che il target non usa.
    rifiuta(
        "table.type_cast",
        &[&w],
        json!({"column": "id", "target_type": "str", "date_format": "%Y"}),
        "date_format ammesso solo",
    );
    accetta(
        "table.type_cast",
        &[&w],
        json!({"column": "date", "target_type": "date32", "date_format": "%Y-%m-%d"}),
    );
    rifiuta(
        "table.type_cast",
        &[&w],
        json!({"column": "id", "target_type": "int", "precision": 10}),
        "non ammessi per questo target_type",
    );
    rifiuta(
        "table.type_cast",
        &[&w],
        json!({"column": "id", "target_type": "timestamp_millis", "scale": 2}),
        "non ammessi per timestamp",
    );
    rifiuta(
        "table.type_cast",
        &[&w],
        json!({"column": "id", "target_type": "decimal128", "precision": 5, "scale": 6}),
        "scale <= precision",
    );
    // dedup_advanced e add_row_number: un verso senza colonna d'ordine.
    for verso in [true, false] {
        rifiuta(
            "table.dedup_advanced",
            &[&w],
            json!({"subset": ["id"], "ascending": verso}),
            "ascending senza order_column",
        );
        accetta(
            "table.dedup_advanced",
            &[&w],
            json!({"subset": ["id"], "order_column": "value", "ascending": verso}),
        );
        rifiuta(
            "table.add_row_number",
            &[&w],
            json!({"ascending": verso}),
            "ascending senza order_column",
        );
    }
    // aggregate: quantile su un'altra funzione.
    rifiuta(
        "table.aggregate",
        &[&w],
        json!({"group_by": ["name"],
               "aggregations": [{"column": "value", "function": "sum", "quantile": 0.5}]}),
        "quantile ammesso solo",
    );
    // string_extract con gruppi con nome: una colonna per gruppo, dal primo
    // match.
    rifiuta(
        "table.string_extract",
        &[&w],
        json!({"column": "name", "pattern": "(?P<l>[ab])", "output_column": "x"}),
        "output_column non ammesso",
    );
    rifiuta(
        "table.string_extract",
        &[&w],
        json!({"column": "name", "pattern": "(?P<l>[ab])", "extract_all": true}),
        "extract_all non ammesso",
    );
    accetta(
        "table.string_extract",
        &[&w],
        json!({"column": "name", "pattern": "(?P<l>[ab])"}),
    );
    accetta(
        "table.string_extract",
        &[&w],
        json!({"column": "name", "pattern": "([ab])", "output_column": "x", "extract_all": true}),
    );
    // explode: righe che sparirebbero dentro l'espansione.
    rifiuta(
        "table.explode",
        &[&annidato()],
        json!({"column": "lst", "empty_policy": "drop"}),
        "empty_policy=drop",
    );
    // assert_range: un estremo esclusivo senza l'estremo.
    rifiuta(
        "table.assert_range",
        &[&w],
        json!({"column": "value", "max": 1, "inclusive_min": false}),
        "senza l'estremo",
    );
    rifiuta(
        "table.assert_range",
        &[&w],
        json!({"column": "value", "min": 0, "inclusive_max": true}),
        "senza l'estremo",
    );
    accetta(
        "table.assert_range",
        &[&w],
        json!({"column": "value", "min": 0, "max": 1, "inclusive_min": false,
               "inclusive_max": true}),
    );
}

#[test]
fn nomi_ripetuti_e_liste_vuote_si_rifiutano() {
    let w = largo();
    rifiuta(
        "table.rename",
        &[&w],
        json!({"renames": [{"old_name": "name", "new_name": "x"},
                           {"old_name": "name", "new_name": "y"}]}),
        "rename origine",
    );
    rifiuta(
        "table.melt",
        &[&w],
        json!({"id_columns": ["id"], "value_columns": ["value"], "var_name": "x",
               "value_name": "x"}),
        "nomi distinti",
    );
    rifiuta(
        "table.melt",
        &[&w],
        json!({"id_columns": ["id"], "value_columns": ["value", "value"]}),
        "ripetuta",
    );
    rifiuta(
        "table.sort",
        &[&w],
        json!({"columns": ["id", "id"]}),
        "ripetuta",
    );
    rifiuta(
        "table.md5_hash",
        &[&w],
        json!({"columns": ["id", "id"], "output_column": "h"}),
        "ripetuta",
    );
    rifiuta(
        "table.sha256_hash",
        &[&w],
        json!({"columns": [], "output_column": "h"}),
        "vuoto",
    );
    rifiuta(
        "table.dedup_advanced",
        &[&w],
        json!({"subset": []}),
        "vuoto",
    );
}

#[test]
fn le_asserzioni_vacue_si_rifiutano() {
    let w = largo();
    rifiuta(
        "table.assert_not_null",
        &[&w],
        json!({"columns": []}),
        "vuoto",
    );
    rifiuta(
        "table.assert_unique",
        &[&w],
        json!({"columns": []}),
        "vuoto",
    );
    rifiuta(
        "table.assert_range",
        &[&w],
        json!({"column": "value"}),
        "min o max",
    );
    rifiuta(
        "table.assert_range",
        &[&w],
        json!({"column": "value", "min": 2, "max": 1}),
        "estremi",
    );
    rifiuta("table.assert_cardinality", &[&w], json!({}), "richiede");
    rifiuta(
        "table.assert_cardinality",
        &[&w],
        json!({"exact_rows": 1, "min_rows": 0}),
        "exact_rows",
    );
    rifiuta("table.assert_schema", &[&w], json!({"fields": []}), "vuoto");
    rifiuta(
        "table.assert_metadata",
        &[&w],
        json!({"expected": {}}),
        "metadati attesi",
    );
    rifiuta(
        "table.assert_regex",
        &[&w],
        json!({"column": "name", "pattern": ""}),
        "pattern vuoto",
    );
    let senza_colonne = schema(vec![]);
    rifiuta(
        "table.stable_fingerprint",
        &[&senza_colonne],
        json!({}),
        "almeno una colonna",
    );
}

#[test]
fn formati_vuoti_e_limiti_si_rifiutano_in_analisi() {
    let w = largo();
    rifiuta(
        "table.date_format",
        &[&w],
        json!({"column": "date", "input_format": "%Y-%m-%d", "output_format": "",
               "output_column": "d"}),
        "output_format non valido",
    );
    rifiuta(
        "table.date_diff",
        &[&w],
        json!({"start_column": "date", "end_column": "date", "input_format": "",
               "unit": "days", "output_column": "d"}),
        "input_format non valido",
    );
    rifiuta(
        "table.date_extract",
        &[&w],
        json!({"column": "date", "parts": ["year"], "date_format": ""}),
        "date_format non valido",
    );
    let limiti = Limits::default();
    rifiuta(
        "table.string_pad",
        &[&w],
        json!({"column": "name", "width": limiti.max_string_bytes + 1, "fill_char": "0"}),
        "width",
    );
    rifiuta(
        "table.limit",
        &[&w],
        json!({"n": limiti.max_rows + 1}),
        "max_rows",
    );
    rifiuta(
        "table.top_n",
        &[&w],
        json!({"columns": ["id"], "n": limiti.max_rows + 1}),
        "max_rows",
    );
    rifiuta(
        "table.replace",
        &[&w],
        json!({"column": "name", "old_value": "a".repeat(limiti.max_regex_bytes + 1),
               "new_value": "b"}),
        "old_value",
    );
    // flatten_json oltre max_columns: lo stesso conto del kernel.
    let massimo = limiti_interni::MAX_COLUMNS;
    let mut campi: Vec<Field> = (0..massimo - 1)
        .map(|indice| Field::new(format!("c{indice}"), DataType::Int64, false))
        .collect();
    campi.push(Field::new("json", DataType::Utf8, false));
    let pieno = schema(campi);
    match analizza(
        "table.flatten_json",
        &[&pieno],
        &json!({"column": "json", "output_columns": ["json_a"]}),
    ) {
        Err(PlenoraError::ResourceLimit(messaggio)) if messaggio.contains("max_columns") => {}
        altro => panic!("flatten_json oltre max_columns: {altro:?}"),
    }
}

#[test]
fn i_limiti_sono_quelli_del_chiamante() {
    // Gli stessi limiti che il kernel ricevera': una regex al limite passa
    // con i limiti del chiamante e non con limiti piu' stretti.
    let w = largo();
    let config = json!({"column": "name", "pattern": "a".repeat(9)});
    let ingresso = [DataContract::tabular(w)];
    let stretti = Limits {
        max_regex_bytes: 8,
        ..Limits::default()
    };
    assert!(analyze_table_contract(
        "table.string_extract",
        &ingresso,
        &config,
        &mut FieldAllocator::default(),
        &Limits::default(),
    )
    .is_ok());
    assert!(matches!(
        analyze_table_contract(
            "table.string_extract",
            &ingresso,
            &config,
            &mut FieldAllocator::default(),
            &stretti,
        ),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

#[test]
fn date_add_rifiuta_un_amount_che_nessuna_data_sopporta() {
    let w = largo();
    let config = |amount: i64, unit: &str| {
        json!({"column": "date", "input_format": "%Y-%m-%d", "amount": amount, "unit": unit,
               "output_column": "d"})
    };
    for unit in [
        "years", "months", "weeks", "days", "hours", "minutes", "seconds",
    ] {
        rifiuta(
            "table.date_add",
            &[&w],
            config(i64::MAX, unit),
            "amount fuori scala",
        );
        rifiuta(
            "table.date_add",
            &[&w],
            config(i64::MIN, unit),
            "amount fuori scala",
        );
        accetta("table.date_add", &[&w], config(100, unit));
        accetta("table.date_add", &[&w], config(-100, unit));
    }
    // Circa 400 mila anni in giorni: dall'inizio dell'intervallo di chrono
    // si arriva ancora alla fine.
    accetta("table.date_add", &[&w], config(100_000_000, "days"));
}

#[test]
fn expression_arieta_regex_e_substring_si_controllano_sull_ast() {
    let w = largo();
    let funzione = |nome: &str, args: Vec<Value>| {
        json!({"output_column": "e",
               "expression": {"kind": "function", "name": nome, "args": args}})
    };
    let col = |nome: &str| json!({"kind": "column", "name": nome});
    let lit = |valore: Value| json!({"kind": "literal", "value": valore});
    rifiuta(
        "table.expression",
        &[&w],
        funzione("lower", vec![col("name"), col("name")]),
        "numero di argomenti",
    );
    rifiuta(
        "table.expression",
        &[&w],
        funzione("power", vec![col("value")]),
        "numero di argomenti",
    );
    rifiuta(
        "table.expression",
        &[&w],
        funzione("concat", vec![]),
        "numero di argomenti",
    );
    rifiuta(
        "table.expression",
        &[&w],
        funzione(
            "regex_replace",
            vec![col("name"), lit(json!("(")), lit(json!("x"))],
        ),
        "regex non valida",
    );
    accetta(
        "table.expression",
        &[&w],
        funzione(
            "regex_replace",
            vec![col("name"), lit(json!("(a)")), lit(json!("x"))],
        ),
    );
    rifiuta(
        "table.expression",
        &[&w],
        funzione("substring", vec![col("name"), lit(json!(-1))]),
        "start negativo",
    );
    rifiuta(
        "table.expression",
        &[&w],
        funzione(
            "substring",
            vec![col("name"), lit(json!(0)), lit(json!(-2))],
        ),
        "len negativo",
    );
    // -0.0 vale 0, e un null propaga null: nessun errore.
    accetta(
        "table.expression",
        &[&w],
        funzione("substring", vec![col("name"), lit(json!(-0.0))]),
    );
    accetta(
        "table.expression",
        &[&w],
        funzione("substring", vec![col("name"), lit(Value::Null)]),
    );
}

#[test]
fn i_kernel_rifiutano_gli_stessi_parametri_ignorati() {
    use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
    use plenora_kernels_table::aggregation::{dedup_advanced, DedupAdvanced, Keep};
    use plenora_kernels_table::strings::{string_extract, StringExtract};
    use plenora_kernels_table::utility::{add_row_number, AddRowNumber};

    let batch = RecordBatch::try_new(
        schema(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]),
        vec![
            Arc::new(Int64Array::from(vec![1, 2])),
            Arc::new(StringArray::from(vec!["a", "b"])),
        ],
    )
    .expect("batch");
    let righe = AddRowNumber {
        output_column: "r".into(),
        start: 1,
        partition_column: None,
        order_column: None,
        ascending: Some(false),
    };
    assert!(matches!(
        add_row_number(&batch, &righe),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let dedup = DedupAdvanced {
        subset: vec!["name".into()],
        keep: Keep::First,
        order_column: None,
        ascending: Some(true),
    };
    assert!(matches!(
        dedup_advanced(&batch, &dedup),
        Err(PlenoraError::InvalidPlan(_))
    ));
    for (output_column, extract_all) in [(Some("x".to_owned()), false), (None, true)] {
        let estrai = StringExtract {
            column: "name".into(),
            pattern: "(?P<l>[ab])".into(),
            output_column,
            extract_all,
        };
        assert!(matches!(
            string_extract(&batch, &estrai, &Limits::default()),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }
}
