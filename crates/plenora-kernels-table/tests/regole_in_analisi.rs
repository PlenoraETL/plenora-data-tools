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
    // Nomi d'uscita di `aggregate`: due aggregazioni con lo stesso nome, un
    // alias uguale a una chiave, una colonna aggregata senza alias che e'
    // anche chiave, la colonna `count` implicita con una chiave `count`. Il
    // messaggio dice quale colonna sparirebbe (la chiave o l'aggregazione).
    let stesso_nome = "sparirebbe";
    for config in [
        json!({"group_by": ["id"], "aggregations": [
            {"column": "value", "function": "sum", "alias": "x"},
            {"column": "value", "function": "mean", "alias": "x"}]}),
        json!({"group_by": ["id"], "aggregations": [
            {"column": "value", "function": "sum", "alias": "id"}]}),
        json!({"group_by": ["name"], "aggregations": [
            {"column": "name", "function": "count"}]}),
        json!({"group_by": ["id"], "aggregations": [
            {"column": "value", "function": "sum"},
            {"column": "grande", "function": "sum", "alias": "value"}]}),
    ] {
        rifiuta("table.aggregate", &[&w], config, stesso_nome);
    }
    let conta = schema(vec![Field::new("count", DataType::Int64, true)]);
    rifiuta(
        "table.aggregate",
        &[&conta],
        json!({"group_by": ["count"]}),
        stesso_nome,
    );
    accetta(
        "table.aggregate",
        &[&w],
        json!({"group_by": ["id"], "aggregations": [
            {"column": "value", "function": "sum"},
            {"column": "value", "function": "mean"}]}),
    );
    // `transpose`: una voce di `output_columns` vuota si rifiuta (il kernel
    // la trattava come assente), e le voci sono distinte.
    rifiuta(
        "table.transpose",
        &[&w],
        json!({"output_columns": ["a", ""]}),
        "vuoto",
    );
    rifiuta(
        "table.transpose",
        &[&w],
        json!({"output_columns": ["a", "a"]}),
        "",
    );
}

/// Gli stessi rifiuti di nomi nel kernel, senza l'analisi: la regola e' una
/// (`Aggregate::nomi_uscita`, `Transpose::verifica_nomi`,
/// `verifica_nomi_distinti`), e un nome ripetuto che viene dai dati
/// (`id_column` di `transpose`) si rifiuta in esecuzione.
#[test]
fn i_kernel_rifiutano_i_nomi_d_uscita_ripetuti() {
    use plenora_core::arrow::array::{Int64Array, RecordBatch, StringArray};
    use plenora_kernels_table::aggregation::{aggregate, Aggregate};
    use plenora_kernels_table::reshape::{transpose, Transpose};

    let batch = RecordBatch::try_new(
        schema(vec![
            Field::new("k", DataType::Utf8, true),
            Field::new("v", DataType::Int64, true),
        ]),
        vec![
            Arc::new(StringArray::from(vec![Some("a"), Some("a")])),
            Arc::new(Int64Array::from(vec![Some(1), Some(2)])),
        ],
    )
    .expect("batch");
    for config in [
        json!({"group_by": ["k"], "aggregations": [{"column": "v", "function": "sum", "alias": "k"}]}),
        json!({"group_by": ["k"], "aggregations": [
            {"column": "v", "function": "sum", "alias": "s"},
            {"column": "v", "function": "max", "alias": "s"}]}),
        json!({"group_by": ["k", "k"]}),
    ] {
        let config: Aggregate = serde_json::from_value(config).expect("config");
        assert!(matches!(
            aggregate(&batch, &config),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }
    let limiti = Limits::default();
    // `type_policy: string`: il rifiuto e' per i nomi, non per i tipi.
    for (config, frammento) in [
        (json!({"output_columns": ["x", ""]}), "vuoto"),
        (json!({"output_columns": ["x", "x"]}), "stesso nome"),
        (json!({"id_column": "k"}), "stesso nome"),
        (json!({"output_columns": ["col_0"]}), "stesso nome"),
    ] {
        let mut config = config;
        config["type_policy"] = json!("string");
        let config: Transpose = serde_json::from_value(config).expect("config");
        let esito = transpose(&batch, &config, &limiti);
        assert!(
            matches!(&esito, Err(PlenoraError::InvalidPlan(messaggio)) if messaggio.contains(frammento)),
            "{config:?}: {esito:?}"
        );
    }
    let config: Transpose =
        serde_json::from_value(json!({"output_columns": ["x", "y"], "type_policy": "string"}))
            .expect("config");
    assert!(transpose(&batch, &config, &limiti).is_ok());
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
    // Con `regex` `old_value` e' un pattern (`max_regex_bytes`); senza, il
    // testo di una cella (`max_string_bytes`).
    rifiuta(
        "table.replace",
        &[&w],
        json!({"column": "name", "old_value": "a".repeat(limiti.max_regex_bytes + 1),
               "new_value": "b", "regex": true}),
        "old_value",
    );
    accetta(
        "table.replace",
        &[&w],
        json!({"column": "name", "old_value": "a".repeat(limiti.max_regex_bytes + 1),
               "new_value": "b"}),
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

#[test]
fn verifica_amount_ha_il_confine_dei_valori_leggibili() {
    use chrono::NaiveDate;
    use plenora_core::arrow::array::{Array, RecordBatch, StringArray};
    use plenora_kernels_table::dates::{date_add, verifica_amount, DateAdd, DateUnit};

    // Il secondo intercalare (`23:59:60`) si rifiuta in lettura: il confine
    // e' quello di `NaiveDateTime::MAX` (23:59:59.999999999), da cui
    // `giorni - 1` giorni all'indietro arrivano al primo giorno.
    let giorni = (NaiveDate::MAX - NaiveDate::MIN).num_days();
    assert!(verifica_amount(-giorni, &DateUnit::Days).is_ok());
    assert!(matches!(
        verifica_amount(-giorni - 1, &DateUnit::Days),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let intercalare = RecordBatch::try_new(
        schema(vec![Field::new("ts", DataType::Utf8, false)]),
        vec![Arc::new(StringArray::from(vec!["+262142-12-31 23:59:60"]))],
    )
    .expect("batch");
    let batch = RecordBatch::try_new(
        schema(vec![Field::new("ts", DataType::Utf8, false)]),
        vec![Arc::new(StringArray::from(vec!["+262142-12-31 23:59:59"]))],
    )
    .expect("batch");
    let config = |amount: i64| -> DateAdd {
        serde_json::from_value(json!({"column": "ts", "input_format": "%Y-%m-%d %H:%M:%S",
            "output_format": "%Y-%m-%d %H:%M:%S", "amount": amount, "unit": "days",
            "output_column": "d"}))
        .expect("config")
    };
    assert!(date_add(&intercalare, &config(0))
        .expect_err("secondo intercalare")
        .row_diagnostics()
        .is_some());
    let uscita = date_add(&batch, &config(-giorni)).expect("il kernel lo esegue");
    let colonna = uscita
        .column_by_name("d")
        .and_then(|colonna| colonna.as_any().downcast_ref::<StringArray>())
        .expect("colonna d");
    assert!(!colonna.is_null(0));
    assert!(
        colonna.value(0).starts_with("-262143-01-01"),
        "{}",
        colonna.value(0)
    );
    // Oltre: il kernel rifiuta anche la data piu' favorevole.
    assert!(date_add(&batch, &config(-giorni - 1)).is_err());
    // In analisi, lo stesso confine.
    let w = largo();
    let analisi = |amount: i64| {
        json!({"column": "date", "input_format": "%Y-%m-%d %H:%M:%S", "amount": amount,
               "unit": "days", "output_column": "d"})
    };
    accetta("table.date_add", &[&w], analisi(-giorni));
    rifiuta(
        "table.date_add",
        &[&w],
        analisi(-giorni - 1),
        "amount fuori scala",
    );
}

#[test]
fn i_letterali_di_expression_si_guardano_solo_se_valutati() {
    let w = largo();
    let chiamata =
        |nome: &str, args: Vec<Value>| json!({"kind": "function", "name": nome, "args": args});
    let funzione = |nome: &str, args: Vec<Value>| json!({"output_column": "e", "expression": chiamata(nome, args)});
    let col = |nome: &str| json!({"kind": "column", "name": nome});
    let lit = |valore: Value| json!({"kind": "literal", "value": valore});
    // Valore o sostituzione solo null: la regex non viene mai compilata.
    for args in [
        vec![lit(Value::Null), lit(json!("(")), lit(json!("x"))],
        vec![col("name"), lit(json!("(")), lit(Value::Null)],
    ] {
        accetta("table.expression", &[&w], funzione("regex_replace", args));
    }
    // Valore solo null: l'indice non viene guardato; start null: len no.
    accetta(
        "table.expression",
        &[&w],
        funzione("substring", vec![lit(Value::Null), lit(json!(-1))]),
    );
    accetta(
        "table.expression",
        &[&w],
        funzione(
            "substring",
            vec![col("name"), lit(Value::Null), lit(json!(-1))],
        ),
    );
    // Dentro un'altra funzione il controllo resta.
    rifiuta(
        "table.expression",
        &[&w],
        funzione(
            "upper",
            vec![chiamata("substring", vec![col("name"), lit(json!(-1))])],
        ),
        "start negativo",
    );
}

#[test]
fn i_nomi_delle_regole_hanno_il_limite_dei_nomi_di_colonna() {
    let w = largo();
    let regola =
        |nome: String| json!({"rules": [{"name": nome, "operator": "notnull", "column": "id"}]});
    accetta("table.validate_rules", &[&w], regola("a".repeat(1_024)));
    rifiuta(
        "table.validate_rules",
        &[&w],
        regola("a".repeat(1_025)),
        "oltre 1024 byte",
    );
}

#[test]
fn nessun_parametro_di_aggregate_mask_data_e_fill_na_si_ignora() {
    let w = largo();
    let aggrega =
        |aggregazione: Value| json!({"group_by": ["name"], "aggregations": [aggregazione]});
    for (aggregazione, frammento) in [
        (
            json!({"column": "value", "function": "sum", "separator": "|"}),
            "separator",
        ),
        (
            json!({"column": "value", "function": "count", "distinct": true}),
            "distinct",
        ),
        (
            json!({"column": "name", "function": "first", "skip_null": false}),
            "skip_null",
        ),
        (
            json!({"column": "value", "function": "mean", "ddof": 0}),
            "ddof",
        ),
    ] {
        rifiuta("table.aggregate", &[&w], aggrega(aggregazione), frammento);
    }
    for aggregazione in [
        json!({"column": "name", "function": "concat", "separator": "|", "distinct": true,
               "skip_null": false, "alias": "nomi"}),
        json!({"column": "value", "function": "variance", "ddof": 0, "distinct": true}),
        json!({"column": "value", "function": "sum"}),
    ] {
        accetta("table.aggregate", &[&w], aggrega(aggregazione));
    }
    for (chiave, valore) in [
        ("chars_start", json!(1)),
        ("chars_end", json!(1)),
        ("mask_char", json!("#")),
    ] {
        let mut masking = json!({"column": "name", "mask_type": "email"});
        masking[chiave] = valore;
        rifiuta(
            "table.mask_data",
            &[&w],
            json!({"maskings": [masking]}),
            "mask_type=custom",
        );
    }
    accetta(
        "table.mask_data",
        &[&w],
        json!({"maskings": [{"column": "name", "chars_start": 1, "mask_char": "#"}]}),
    );
    for metodo in ["ffill", "bfill"] {
        for valore in [json!(0), Value::Null] {
            rifiuta(
                "table.fill_na",
                &[&w],
                json!({"column": "id", "method": metodo, "value": valore}),
                "value ammesso solo",
            );
        }
        accetta(
            "table.fill_na",
            &[&w],
            json!({"column": "id", "method": metodo}),
        );
    }
    accetta(
        "table.fill_na",
        &[&w],
        json!({"column": "id", "method": "value", "value": 0}),
    );
}

#[test]
fn i_kernel_rifiutano_i_parametri_che_non_usano() {
    use plenora_core::arrow::array::{Float64Array, RecordBatch, StringArray};
    use plenora_kernels_table::aggregation::{aggregate, Aggregate};
    use plenora_kernels_table::cleansing::{fill_na, FillNa};
    use plenora_kernels_table::security::{mask_data, MaskData};

    let batch = RecordBatch::try_new(
        schema(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("value", DataType::Float64, true),
        ]),
        vec![
            Arc::new(StringArray::from(vec![Some("a@b.it"), None])),
            Arc::new(Float64Array::from(vec![Some(1.0), None])),
        ],
    )
    .expect("batch");
    let aggrega: Aggregate = serde_json::from_value(json!({"group_by": ["name"],
        "aggregations": [{"column": "value", "function": "sum", "separator": "|"}]}))
    .expect("config");
    assert!(matches!(
        aggregate(&batch, &aggrega),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let maschera: MaskData = serde_json::from_value(json!({"maskings": [{"column": "name",
        "mask_type": "email", "mask_char": "#"}]}))
    .expect("config");
    assert!(matches!(
        mask_data(&batch, &maschera),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let riempi: FillNa =
        serde_json::from_value(json!({"column": "value", "method": "ffill", "value": null}))
            .expect("config");
    assert!(matches!(
        fill_na(&batch, &riempi),
        Err(PlenoraError::InvalidPlan(_))
    ));
    // Parametri assenti: i default, nessun errore.
    let riempi: FillNa =
        serde_json::from_value(json!({"column": "value", "method": "ffill"})).expect("config");
    assert!(fill_na(&batch, &riempi).is_ok());
}

#[test]
fn offset_e_ddof_delle_finestre_solo_dove_hanno_effetto() {
    use plenora_core::arrow::array::{Float64Array, RecordBatch};
    use plenora_kernels_table::aggregation::{
        rolling_window, window_function, RollingWindow, WindowFunction,
    };

    let w = largo();
    rifiuta(
        "table.window_function",
        &[&w],
        json!({"column": "value", "function": "cumsum", "offset": 2}),
        "offset ammesso solo",
    );
    rifiuta(
        "table.window_function",
        &[&w],
        json!({"column": "value", "function": "lag", "offset": 0}),
        "offset deve essere positivo",
    );
    for funzione in ["lag", "lead"] {
        accetta(
            "table.window_function",
            &[&w],
            json!({"column": "value", "function": funzione, "offset": 2}),
        );
    }
    rifiuta(
        "table.rolling_window",
        &[&w],
        json!({"column": "value", "function": "mean", "window": 2, "ddof": 0,
               "output_column": "r"}),
        "ddof ammesso solo",
    );
    accetta(
        "table.rolling_window",
        &[&w],
        json!({"column": "value", "function": "stddev", "window": 2, "ddof": 0,
               "output_column": "r"}),
    );
    // Anche i kernel.
    let batch = RecordBatch::try_new(
        schema(vec![Field::new("value", DataType::Float64, false)]),
        vec![Arc::new(Float64Array::from(vec![1.0, 2.0]))],
    )
    .expect("batch");
    let finestra: WindowFunction =
        serde_json::from_value(json!({"column": "value", "function": "cumsum", "offset": 2}))
            .expect("config");
    assert!(matches!(
        window_function(&batch, &finestra),
        Err(PlenoraError::InvalidPlan(_))
    ));
    let mobile: RollingWindow = serde_json::from_value(json!({"column": "value",
        "function": "mean", "window": 2, "ddof": 0, "output_column": "r"}))
    .expect("config");
    assert!(matches!(
        rolling_window(&batch, &mobile),
        Err(PlenoraError::InvalidPlan(_))
    ));
}

/// `validate_rules`: analisi e kernel leggono il valore numerico di una
/// regola con lo stesso parse esatto (`governance::valore_regola`). Prima
/// l'analisi usava il parse `f64`: `gt` con `"1e-128"` su una colonna
/// `Int64` passava l'analisi e il kernel lo rifiutava sempre, anche su una
/// tabella vuota. Parita' valore per valore, operatore per operatore.
#[test]
fn validate_rules_analisi_e_kernel_leggono_i_valori_allo_stesso_modo() {
    use plenora_core::arrow::array::RecordBatch;
    use plenora_kernels_table::governance::{validate_rules, ValidateRules};
    let w = largo();
    let vuota = RecordBatch::new_empty(w.clone());
    let valori = [
        "5",
        "-0.5",
        "1e2",
        "1e-128",
        "1e400",
        "1e2147483647",
        "abc",
        "",
        "+-1",
        "inf",
    ];
    let mut casi = Vec::new();
    for colonna in ["id", "value", "importo", "grande"] {
        for operatore in ["eq", "ne", "gt", "ge", "lt", "le"] {
            for valore in valori {
                casi.push(json!({"rules": [{"name": "r", "column": colonna,
                    "operator": operatore, "value": valore}]}));
            }
        }
        for (basso, alto) in [("1", "2"), ("1", "1e-128"), ("1e-128", "2"), ("a", "2")] {
            casi.push(json!({"rules": [{"name": "r", "column": colonna,
                "operator": "range", "value": format!("{basso}, {alto}")}]}));
        }
    }
    let mut difformi = Vec::new();
    for config in &casi {
        let analisi = analizza("table.validate_rules", &[&w], config).is_ok();
        let kernel = serde_json::from_value::<ValidateRules>(config.clone())
            .map_err(|errore| PlenoraError::InvalidPlan(errore.to_string()))
            .and_then(|regole| validate_rules(&vuota, &regole))
            .is_ok();
        if analisi != kernel {
            difformi.push(format!("{config}: analisi {analisi}, kernel {kernel}"));
        }
    }
    assert!(difformi.is_empty(), "{}", difformi.join("\n"));
    // Il caso limite: rifiutato gia' in analisi.
    rifiuta(
        "table.validate_rules",
        &[&w],
        json!({"rules": [{"name": "r", "column": "id", "operator": "gt", "value": "1e-128"}]}),
        "valore numerico",
    );
}
