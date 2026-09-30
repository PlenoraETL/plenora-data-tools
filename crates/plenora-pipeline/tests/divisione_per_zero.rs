//! Divisione per zero in `table.formula` e `table.expression`: di default
//! vale null (decisione dell'utente), con `on_division_by_zero = "error"` la
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
