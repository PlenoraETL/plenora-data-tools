//! Divisione per zero in `table.formula` e `table.expression`: di default
//! vale null (docs/runner.md, «Divisione per zero»), con
//! `on_division_by_zero = "error"` la
//! riga si rifiuta. Il resoconto di ogni passo conta le righe con un
//! divisore zero in entrambi i modi, mai i valori.

use std::sync::Arc;

use plenora_core::arrow::array::{Array, Float64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::{PlenoraError, Result};
use plenora_pipeline::{Esito, Passo, Pipeline};
use serde_json::{json, Value};

/// `v` e `d`: `d` e' zero alle righe 1, 3 e 4, null alla riga 5 (null, non
/// una divisione per zero).
fn tabella() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("v", DataType::Float64, false),
            Field::new("d", DataType::Float64, true),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![6.0, 1.0, 8.0, 2.0, 3.0, 4.0])),
            Arc::new(Float64Array::from(vec![
                Some(2.0),
                Some(0.0),
                Some(4.0),
                Some(-0.0),
                Some(0.0),
                None,
            ])),
        ],
    )
    .expect("tabella")
}

fn piano(passi: Vec<(&str, &str, Value)>) -> Pipeline {
    let mut ingresso = "t".to_owned();
    let steps = passi
        .into_iter()
        .map(|(out, op, config)| {
            let nuovo = Passo {
                out: out.to_owned(),
                op: op.to_owned(),
                inputs: vec![ingresso.clone()],
                config,
            };
            out.clone_into(&mut ingresso);
            nuovo
        })
        .collect();
    Pipeline {
        version: 1,
        inputs: vec!["t".to_owned()],
        crs: None,
        limits: None,
        steps,
        outputs: vec![ingresso],
    }
}

fn esegui(pipeline: &Pipeline) -> Result<Esito> {
    let tabella = tabella();
    let schemi: Vec<(&str, SchemaRef)> = vec![("t", tabella.schema())];
    pipeline
        .validate(&schemi)?
        .run(vec![("t".to_owned(), tabella)])
}

fn formula(politica: Option<&str>) -> Value {
    let mut config = json!({"new_column": "q", "formula": "v / d"});
    if let Some(politica) = politica {
        config["on_division_by_zero"] = json!(politica);
    }
    config
}

fn expression(politica: Option<&str>) -> Value {
    let mut config = json!({"output_column": "q", "expression": {
        "kind": "binary", "op": "divide",
        "left": {"kind": "column", "name": "v"},
        "right": {"kind": "column", "name": "d"}}});
    if let Some(politica) = politica {
        config["on_division_by_zero"] = json!(politica);
    }
    config
}

#[test]
fn di_default_la_divisione_per_zero_vale_null_e_il_resoconto_la_conta() {
    for (op, config) in [
        ("table.formula", formula(None)),
        ("table.formula", formula(Some("null"))),
        ("table.expression", expression(None)),
        ("table.expression", expression(Some("null"))),
    ] {
        let esito = esegui(&piano(vec![("x", op, config.clone())])).expect(op);
        let passo = &esito.report.passi[0];
        assert_eq!(passo.righe_divisione_per_zero, 3, "{op} {config}");
        let uscita = &esito.outputs[0].1;
        let q = uscita
            .column_by_name("q")
            .expect("q")
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("f64")
            .clone();
        // Righe 1, 3, 4 per la divisione, riga 5 per il divisore null.
        assert_eq!(q.null_count(), 4, "{op}");
        assert_eq!(q.value(0).to_bits(), 3.0_f64.to_bits());
        assert!(q.is_null(1) && q.is_null(3) && q.is_null(4) && q.is_null(5));
    }
}

#[test]
fn con_error_il_passo_fallisce_con_la_diagnostica_per_riga() {
    for (op, config) in [
        ("table.formula", formula(Some("error"))),
        ("table.expression", expression(Some("error"))),
    ] {
        let errore = esegui(&piano(vec![("x", op, config)])).expect_err(op);
        let diagnostica = errore.row_diagnostics().expect("diagnostica per riga");
        assert_eq!(diagnostica.counts["evaluation.division_by_zero"], 3, "{op}");
        // Nessun valore nel messaggio: solo conteggi e indici.
        assert!(!errore.to_string().contains("6.0"), "{errore}");
    }
}

#[test]
fn le_altre_operazioni_e_le_divisioni_senza_zero_contano_zero() {
    let esito = esegui(&piano(vec![
        (
            "a",
            "table.formula",
            json!({"new_column": "q", "formula": "v / 2"}),
        ),
        (
            "b",
            "table.rename",
            json!({"renames": [{"old_name": "q", "new_name": "r"}]}),
        ),
    ]))
    .expect("piano");
    assert!(esito
        .report
        .passi
        .iter()
        .all(|passo| passo.righe_divisione_per_zero == 0));
}

#[test]
fn validazione_ed_esecuzione_concordano_sulle_regole_della_politica() {
    // Politica senza divisioni, valore sconosciuto, divisore letterale zero
    // (anche con la politica `null`): tutti in validazione, prima dei dati.
    let casi = [
        (
            "table.formula",
            json!({"new_column": "q", "formula": "v * 2", "on_division_by_zero": "null"}),
        ),
        (
            "table.formula",
            json!({"new_column": "q", "formula": "v / d", "on_division_by_zero": "zero"}),
        ),
        (
            "table.formula",
            json!({"new_column": "q", "formula": "v / 0", "on_division_by_zero": "null"}),
        ),
        (
            "table.expression",
            json!({"output_column": "q", "on_division_by_zero": "null", "expression": {
                "kind": "binary", "op": "divide",
                "left": {"kind": "column", "name": "v"},
                "right": {"kind": "literal", "value": 0}}}),
        ),
        (
            "table.expression",
            json!({"output_column": "q", "on_division_by_zero": "error",
                   "expression": {"kind": "column", "name": "v"}}),
        ),
    ];
    let tabella = tabella();
    let schemi: Vec<(&str, SchemaRef)> = vec![("t", tabella.schema())];
    for (op, config) in casi {
        let errore = piano(vec![("x", op, config.clone())])
            .validate(&schemi)
            .expect_err("rifiuto in validazione");
        assert!(
            matches!(errore, PlenoraError::InvalidPlan(_)),
            "{op} {config}: {errore:?}"
        );
    }
}

/// Un overflow di `f64` da operandi finiti non si pubblica come infinito:
/// `evaluation.non_finite_result`, con ogni `on_division_by_zero` (la
/// politica vale solo per il divisore zero). Anche intermedio: `a / (a * a)`
/// darebbe 0 dopo `a * a` = infinito.
#[test]
fn un_overflow_da_operandi_finiti_rifiuta_la_riga_in_formula_ed_expression() {
    let grandi = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("a", DataType::Float64, false),
            Field::new("b", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![1e308, 2.0])),
            Arc::new(Float64Array::from(vec![1e-308, 1.0])),
        ],
    )
    .expect("tabella");
    let schemi: Vec<(&str, SchemaRef)> = vec![("t", grandi.schema())];
    let divisione = json!({"kind": "binary", "op": "divide",
                           "left": {"kind": "column", "name": "a"},
                           "right": {"kind": "column", "name": "b"}});
    for (op, config) in [
        (
            "table.formula",
            json!({"new_column": "q", "formula": "a / b"}),
        ),
        (
            "table.formula",
            json!({"new_column": "q", "formula": "a / (a * a)"}),
        ),
        (
            "table.formula",
            json!({"new_column": "q", "formula": "a / b", "on_division_by_zero": "error"}),
        ),
        (
            "table.expression",
            json!({"output_column": "q", "expression": divisione}),
        ),
    ] {
        let errore = piano_su("t", vec![("x", op, config.clone())])
            .validate(&schemi)
            .expect("piano valido")
            .run(vec![("t".to_owned(), grandi.clone())])
            .expect_err("overflow");
        let diagnostica = errore.row_diagnostics().expect("diagnostica per riga");
        assert_eq!(
            diagnostica.counts.get("evaluation.non_finite_result"),
            Some(&1),
            "{op} {config}"
        );
    }
}

/// Un pattern di `regex_replace` calcolato da una cella e non valido: il
/// messaggio non contiene il pattern (il crate `regex` lo riporterebbe), la
/// riga si rifiuta con la causa `evaluation.invalid_regex`.
#[test]
fn una_regex_calcolata_non_valida_non_porta_il_pattern_nel_messaggio() {
    use plenora_core::arrow::array::StringArray;
    let tabella = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("s", DataType::Utf8, false),
            Field::new("p", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["abc", "abc"])),
            Arc::new(StringArray::from(vec!["b", "(SEGRETO"])),
        ],
    )
    .expect("tabella");
    let schemi: Vec<(&str, SchemaRef)> = vec![("t", tabella.schema())];
    let config = json!({"output_column": "e", "expression": {"kind": "function",
        "name": "regex_replace", "args": [{"kind": "column", "name": "s"},
        {"kind": "column", "name": "p"}, {"kind": "literal", "value": "x"}]}});
    let errore = piano_su("t", vec![("x", "table.expression", config)])
        .validate(&schemi)
        .expect("piano valido")
        .run(vec![("t".to_owned(), tabella)])
        .expect_err("regex non valida");
    assert!(!format!("{errore:?}").contains("SEGRETO"), "{errore:?}");
    let diagnostica = errore.row_diagnostics().expect("diagnostica per riga");
    assert_eq!(diagnostica.counts.get("evaluation.invalid_regex"), Some(&1));
}

/// `window_function` e `rolling_window`: un overflow da valori finiti si
/// rifiuta; un `NaN` gia' nei dati si propaga come dichiarato.
#[test]
fn window_e_rolling_rifiutano_l_overflow_da_valori_finiti() {
    let grandi = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("v", DataType::Float64, true)])),
        vec![Arc::new(Float64Array::from(vec![1e308, 1e308, 1.0]))],
    )
    .expect("tabella");
    let con_nan = RecordBatch::try_new(
        grandi.schema(),
        vec![Arc::new(Float64Array::from(vec![f64::NAN, 1.0, 2.0]))],
    )
    .expect("tabella");
    let schemi: Vec<(&str, SchemaRef)> = vec![("t", grandi.schema())];
    for (op, config) in [
        (
            "table.window_function",
            json!({"column": "v", "function": "cumsum"}),
        ),
        (
            "table.rolling_window",
            json!({"column": "v", "function": "sum", "window": 2, "output_column": "r"}),
        ),
    ] {
        let errore = piano_su("t", vec![("x", op, config.clone())])
            .validate(&schemi)
            .expect("piano valido")
            .run(vec![("t".to_owned(), grandi.clone())])
            .expect_err("overflow");
        assert!(errore.to_string().contains("overflow"), "{op}: {errore}");
        piano_su("t", vec![("x", op, config)])
            .validate(&schemi)
            .expect("piano valido")
            .run(vec![("t".to_owned(), con_nan.clone())])
            .expect("NaN dei dati propagato");
    }
}

fn piano_su(ingresso: &str, passi: Vec<(&str, &str, Value)>) -> Pipeline {
    let mut pipeline = piano(passi);
    pipeline.inputs = vec![ingresso.to_owned()];
    pipeline
}
