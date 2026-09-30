//! `max_string_bytes` e `max_regex_bytes` su ogni testo prodotto e ogni
//! regex compilata: i testi e i pattern della config si rifiutano in
//! validazione (`InvalidPlan`), i testi che crescono con i dati e i pattern
//! calcolati dalle colonne nel kernel (`ResourceLimit`), prima o durante il
//! passo, mai un testo oltre il limite nell'uscita.

use std::sync::Arc;

use plenora_core::arrow::array::{Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::{PlenoraError, Result};
use plenora_pipeline::{Esito, LimitiParziali, Passo, Pipeline};
use serde_json::{json, Value};

/// Due colonne di testo: `s` (tre righe di 8 byte) e `p` (un pattern).
fn tabella() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("s", DataType::Utf8, false),
            Field::new("p", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["a", "a", "b"])),
            Arc::new(StringArray::from(vec!["abcdefgh", "ijklmnop", "qrstuvwx"])),
            Arc::new(StringArray::from(vec!["(a|b|c|d|e|f)+"; 3])),
        ],
    )
    .expect("tabella")
}

fn piano(op: &str, config: Value, limiti: LimitiParziali) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: vec!["t".to_owned()],
        crs: None,
        limits: Some(limiti),
        steps: vec![Passo {
            out: "x".to_owned(),
            op: op.to_owned(),
            inputs: vec!["t".to_owned()],
            config,
        }],
        outputs: vec!["x".to_owned()],
    }
}

fn stringhe(byte: usize) -> LimitiParziali {
    LimitiParziali {
        max_string_bytes: Some(byte),
        ..LimitiParziali::default()
    }
}

fn regex(byte: usize) -> LimitiParziali {
    LimitiParziali {
        max_regex_bytes: Some(byte),
        ..LimitiParziali::default()
    }
}

fn valida(pipeline: &Pipeline) -> Result<plenora_pipeline::PipelineValidata> {
    let tabella = tabella();
    let schemi: Vec<(&str, SchemaRef)> = vec![("t", tabella.schema())];
    pipeline.validate(&schemi)
}

fn esegui(pipeline: &Pipeline) -> Result<Esito> {
    valida(pipeline)?.run(vec![("t".to_owned(), tabella())])
}

#[allow(clippy::needless_pass_by_value)] // `json!` in linea nei casi.
fn funzione(nome: &str, args: Value) -> Value {
    json!({"output_column": "e", "expression": {"kind": "function", "name": nome, "args": args}})
}

fn col(nome: &str) -> Value {
    json!({"kind": "column", "name": nome})
}

#[allow(clippy::needless_pass_by_value)] // `json!` in linea nei casi.
fn lit(valore: Value) -> Value {
    json!({"kind": "literal", "value": valore})
}

#[test]
fn i_testi_e_i_pattern_della_config_si_rifiutano_in_validazione() {
    let lungo = "x".repeat(40);
    let casi = [
        // Pattern letterale di `regex_replace` oltre `max_regex_bytes`: prima
        // l'analisi lo lasciava passare.
        (
            "table.expression",
            funzione(
                "regex_replace",
                json!([col("s"), lit(json!(lungo)), lit(json!("y"))]),
            ),
            regex(16),
            "pattern oltre max_regex_bytes",
        ),
        (
            "table.expression",
            funzione("concat", json!([col("s"), lit(json!(lungo))])),
            stringhe(16),
            "testo letterale oltre max_string_bytes",
        ),
        // Anche un testo dentro la lista di `in`.
        (
            "table.expression",
            funzione("in", json!([col("s"), lit(json!(["a", lungo]))])),
            stringhe(16),
            "testo letterale oltre max_string_bytes",
        ),
        (
            "table.lookup",
            json!({"column": "k", "mapping": {"a": lungo}}),
            stringhe(16),
            "mapping oltre il limite",
        ),
        (
            "table.conditional",
            json!({"column": "k", "conditions": [{"operator": "==", "value": "a",
                                                    "result": lungo}]}),
            stringhe(16),
            "result oltre il limite",
        ),
        (
            "table.fill_na",
            json!({"column": "s", "method": "value", "value": lungo}),
            stringhe(16),
            "value oltre il limite",
        ),
        (
            "table.date_format",
            json!({"column": "s", "input_format": "%Y", "output_column": "d",
                   "output_format": "%Y-%m-%d %H:%M:%S"}),
            stringhe(18),
            "output_format: testo scritto oltre max_string_bytes",
        ),
        (
            "table.replace",
            json!({"column": "s", "old_value": lungo, "new_value": "y"}),
            stringhe(16),
            "old_value oltre il limite",
        ),
        (
            "table.table_diff",
            json!({"left_keys": ["k"], "right_keys": ["k"], "compare_columns": ["s", "p"],
                   "separator": lungo}),
            stringhe(16),
            "separator oltre il limite",
        ),
    ];
    // Il limite del formato e' esatto per campo: `%Y%m` (al piu' 9 byte)
    // passa con 16.
    valida(&piano(
        "table.date_format",
        json!({"column": "s", "input_format": "%Y", "output_column": "d",
               "output_format": "%Y%m"}),
        stringhe(16),
    ))
    .expect("%Y%m entro 16 byte");
    for (op, config, limiti, frammento) in casi {
        let mut pipeline = piano(op, config.clone(), limiti);
        if op == "table.table_diff" {
            pipeline.inputs.push("u".to_owned());
            pipeline.steps[0].inputs.push("u".to_owned());
            let tabella = tabella();
            let schemi: Vec<(&str, SchemaRef)> =
                vec![("t", tabella.schema()), ("u", tabella.schema())];
            let errore = pipeline.validate(&schemi).expect_err(op);
            assert!(errore.to_string().contains(frammento), "{op}: {errore}");
            continue;
        }
        let errore = valida(&pipeline).expect_err(op);
        assert!(
            matches!(errore, PlenoraError::InvalidPlan(_))
                && errore.to_string().contains(frammento),
            "{op} {config}: {errore:?}"
        );
    }
}

#[test]
fn i_testi_che_crescono_con_i_dati_si_fermano_nel_kernel() {
    // Ogni caso produce da `s` (8 byte per cella) un testo di piu' di 12
    // byte: con `max_string_bytes = 12` il passo fallisce con
    // `ResourceLimit`, con il default passa.
    let casi = [
        (
            "table.replace",
            json!({"column": "s", "old_value": "", "new_value": "--", "regex": true}),
        ),
        (
            "table.expression",
            funzione("concat", json!([col("s"), col("s"), col("s")])),
        ),
        (
            "table.expression",
            funzione(
                "regex_replace",
                json!([col("s"), lit(json!("")), lit(json!("--"))]),
            ),
        ),
        (
            "table.formula",
            json!({"new_column": "f", "formula": "s + s + s"}),
        ),
        (
            "table.aggregate",
            json!({"group_by": ["k"], "aggregations": [{"column": "s", "function": "concat",
                                                          "alias": "tutti"}]}),
        ),
        (
            "table.string_extract",
            json!({"column": "s", "pattern": "(.)", "extract_all": true}),
        ),
        (
            "table.mask_data",
            json!({"maskings": [{"column": "s", "mask_type": "custom", "mask_char": "\u{1F600}"}]}),
        ),
    ];
    for (op, config) in casi {
        let errore = esegui(&piano(op, config.clone(), stringhe(12))).expect_err(op);
        assert!(
            matches!(errore, PlenoraError::ResourceLimit(_)),
            "{op} {config}: {errore:?}"
        );
        assert!(
            errore.to_string().contains("max_string_bytes"),
            "{op}: {errore}"
        );
        esegui(&piano(op, config, LimitiParziali::default())).unwrap_or_else(|errore| {
            panic!("{op} con i limiti di default: {errore}");
        });
    }
}

#[test]
fn un_pattern_calcolato_dalle_colonne_oltre_il_limite_ferma_il_passo() {
    // Il pattern di `p` ha 14 byte: con `max_regex_bytes = 8` il passo
    // fallisce nel kernel (non e' noto in validazione).
    let config = funzione(
        "regex_replace",
        json!([col("s"), col("p"), lit(json!("y"))]),
    );
    let errore = esegui(&piano("table.expression", config.clone(), regex(8))).expect_err("pattern");
    assert!(
        matches!(errore, PlenoraError::ResourceLimit(_)),
        "{errore:?}"
    );
    let esito = esegui(&piano("table.expression", config, regex(64))).expect("pattern nel limite");
    assert_eq!(esito.outputs[0].1.column_by_name("e").expect("e").len(), 3);
}

/// I kernel senza un passo del runner che li esegua qui (`pivot` senza
/// `mapping`, `flatten_json` senza `output_columns`) e `table_diff`: la
/// chiamata diretta con limiti piccoli.
#[test]
fn pivot_table_diff_e_flatten_json_fermano_i_testi_oltre_il_limite() {
    use plenora_kernels_table::{analysis, reshape, Limits};
    let limiti = Limits {
        max_string_bytes: 12,
        ..Limits::default()
    };
    let tabella = tabella();
    // `concat` del gruppo `a`: "abcdefgh,ijklmnop", 17 byte.
    let pivot: reshape::Pivot = serde_json::from_value(json!({
        "index_col": "k", "pivot_col": "k", "value_col": "s", "aggr_func": "concat"}))
    .expect("pivot");
    let errore = reshape::pivot(&tabella, &pivot, &limiti).expect_err("pivot");
    assert!(
        matches!(errore, PlenoraError::ResourceLimit(_)),
        "{errore:?}"
    );
    assert!(reshape::pivot(&tabella, &pivot, &Limits::default()).is_ok());
    // `_diff_old_values` unisce `s` e `p` della riga cambiata: 8 + 1 + 14.
    let diversa = RecordBatch::try_new(
        tabella.schema(),
        vec![
            tabella.column(0).clone(),
            Arc::new(StringArray::from(vec!["x", "y", "z"])),
            Arc::new(StringArray::from(vec!["q"; 3])),
        ],
    )
    .expect("destra");
    let diff: reshape::TableDiff = serde_json::from_value(json!({
        "left_keys": ["s"], "right_keys": ["s"], "compare_columns": ["k", "p"]}))
    .expect("diff");
    let sinistra = RecordBatch::try_new(
        tabella.schema(),
        vec![
            tabella.column(1).clone(),
            tabella.column(1).clone(),
            tabella.column(2).clone(),
        ],
    )
    .expect("sinistra");
    let destra = RecordBatch::try_new(
        tabella.schema(),
        vec![
            tabella.column(1).clone(),
            tabella.column(1).clone(),
            diversa.column(1).clone(),
        ],
    )
    .expect("destra");
    let errore = reshape::table_diff(&sinistra, &destra, &diff, &limiti).expect_err("diff");
    assert!(
        matches!(errore, PlenoraError::ResourceLimit(_)),
        "{errore:?}"
    );
    assert!(reshape::table_diff(&sinistra, &destra, &diff, &Limits::default()).is_ok());
    // Un oggetto annidato si riscrive come testo JSON.
    let json_tabella = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("j", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec![
            "{\"a\": [1, 2, 3, 4, 5, 6, 7, 8]}",
        ]))],
    )
    .expect("json");
    let flatten: analysis::FlattenJson =
        serde_json::from_value(json!({"column": "j"})).expect("flatten");
    let errore = analysis::flatten_json(&json_tabella, &flatten, &limiti).expect_err("flatten");
    assert!(
        matches!(errore, PlenoraError::ResourceLimit(_)),
        "{errore:?}"
    );
    assert!(analysis::flatten_json(&json_tabella, &flatten, &Limits::default()).is_ok());
}

/// Le etichette automatiche di `bin` scrivono i bordi interi: con bordi
/// calcolati da valori enormi superano `max_string_bytes`.
#[test]
fn le_etichette_automatiche_di_bin_rispettano_il_limite() {
    use plenora_core::arrow::array::Float64Array;
    use plenora_kernels_table::{analysis, Limits};
    let tabella = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("v", DataType::Float64, false)])),
        vec![Arc::new(Float64Array::from(vec![-1e300, 1e300]))],
    )
    .expect("tabella");
    let config: analysis::Bin =
        serde_json::from_value(json!({"column": "v", "bins": 2})).expect("bin");
    let stretti = Limits {
        max_string_bytes: 64,
        ..Limits::default()
    };
    let errore = analysis::bin_con_limiti(&tabella, &config, &stretti).expect_err("etichetta");
    assert!(
        matches!(errore, PlenoraError::ResourceLimit(_)),
        "{errore:?}"
    );
    assert!(analysis::bin(&tabella, &config).is_ok());
}
