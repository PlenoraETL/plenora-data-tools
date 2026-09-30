//! Esecuzione: ogni passo uguale alla chiamata diretta del kernel,
//! vivibilità delle tabelle, byte vivi del resoconto, limiti sui dati.

mod comune;

use std::sync::Arc;

use plenora_core::arrow::array::{Array, ArrayRef, LargeStringArray, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::{PlenoraError, Result};
use plenora_pipeline::{byte_vivi, Esito, LimitiParziali, Passo, Pipeline};
use serde_json::{json, Value};

use comune::{chiamata_diretta, destra, nomi_input, tabelle, wide, CASI, CHIAVE_HMAC};

fn passo(out: &str, op: &str, inputs: &[&str], config: Value) -> Passo {
    Passo {
        out: out.to_owned(),
        op: op.to_owned(),
        inputs: inputs.iter().map(|nome| (*nome).to_owned()).collect(),
        config,
    }
}

fn piano(inputs: &[&str], steps: Vec<Passo>, outputs: &[&str]) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: inputs.iter().map(|nome| (*nome).to_owned()).collect(),
        crs: None,
        limits: None,
        steps,
        outputs: outputs.iter().map(|nome| (*nome).to_owned()).collect(),
    }
}

/// Valida contro gli schemi delle tabelle ed esegue.
fn esegui(pipeline: &Pipeline, tabelle: &[(&str, RecordBatch)]) -> Result<Esito> {
    let schemi: Vec<(&str, SchemaRef)> = tabelle
        .iter()
        .map(|(nome, tabella)| (*nome, tabella.schema()))
        .collect();
    pipeline.validate(&schemi)?.run(
        tabelle
            .iter()
            .map(|(nome, tabella)| ((*nome).to_owned(), tabella.clone()))
            .collect(),
    )
}

fn output<'a>(esito: &'a Esito, nome: &str) -> &'a RecordBatch {
    &esito
        .outputs
        .iter()
        .find(|(candidato, _)| candidato == nome)
        .expect("output del piano")
        .1
}

fn righe(tabella: &RecordBatch) -> u64 {
    u64::try_from(tabella.num_rows()).expect("righe")
}

#[test]
fn ogni_passo_e_uguale_alla_chiamata_diretta_del_kernel() {
    std::env::set_var(CHIAVE_HMAC, "chiave-di-test-del-runner");
    for caso in CASI {
        if caso.op == "table.transpose" {
            continue; // Rifiutata in validazione: schema dipendente dai dati.
        }
        let config: Value = serde_json::from_str(caso.config).expect("config del caso");
        let tavole = tabelle(caso.fixture);
        let nomi = nomi_input(tavole.len());
        let riferimenti: Vec<&str> = nomi.iter().map(String::as_str).collect();
        let pipeline = piano(
            &riferimenti,
            vec![passo("uscita", caso.op, &riferimenti, config.clone())],
            &["uscita"],
        );
        let coppie: Vec<(&str, RecordBatch)> = riferimenti
            .iter()
            .copied()
            .zip(tavole.iter().cloned())
            .collect();
        let esito =
            esegui(&pipeline, &coppie).unwrap_or_else(|errore| panic!("{}: {errore}", caso.op));
        let runner = output(&esito, "uscita");
        let diretta = chiamata_diretta(caso.op, &config, &tavole)
            .unwrap_or_else(|errore| panic!("{} diretta: {errore}", caso.op));

        assert_eq!(runner.num_rows(), diretta.num_rows(), "{}", caso.op);
        let nomi_e_tipi = |tabella: &RecordBatch| -> Vec<(String, DataType)> {
            tabella
                .schema()
                .fields()
                .iter()
                .map(|campo| (campo.name().clone(), campo.data_type().clone()))
                .collect()
        };
        assert_eq!(nomi_e_tipi(runner), nomi_e_tipi(&diretta), "{}", caso.op);

        let [resoconto] = esito.report.passi.as_slice() else {
            panic!("un passo, un resoconto");
        };
        assert_eq!(resoconto.op, caso.op);
        assert_eq!(resoconto.righe_out, righe(&diretta));
        let righe_in: Vec<u64> = tavole.iter().map(righe).collect();
        assert_eq!(resoconto.righe_in, righe_in);
        // Solo l'output e' residente a fine passo: i byte vivi sono i suoi.
        assert_eq!(
            resoconto.byte_vivi,
            byte_vivi([runner]).expect("byte"),
            "{}",
            caso.op
        );
        if caso.op != "table.uuid_generator" {
            // Valori casuali per contratto: per uuid bastano schema e righe.
            assert_eq!(runner.columns(), diretta.columns(), "{}", caso.op);
        }
    }
}

#[test]
fn una_catena_di_rinomine_e_riordini_non_cambia_i_byte_vivi() {
    let tabella = wide(false);
    let iniziali = byte_vivi([&tabella]).expect("byte dell'input");
    let mut passi = Vec::new();
    let mut precedente = "t".to_owned();
    for indice in 0..12 {
        let out = format!("p{indice}");
        let (op, config) = match indice % 3 {
            0 => (
                "table.rename",
                json!({"renames": [{"old_name": "json", "new_name": format!("json{indice}")}]}),
            ),
            1 => ("table.reorder_columns", json!({"columns": ["value", "id"]})),
            _ => (
                "table.rename",
                json!({"renames": [{"old_name": format!("json{}", indice - 2), "new_name": "json"}]}),
            ),
        };
        passi.push(passo(&out, op, &[&precedente], config));
        precedente = out;
    }
    let pipeline = piano(&["t"], passi, &[&precedente]);
    let esito = esegui(&pipeline, &[("t", tabella)]).expect("catena");
    assert_eq!(esito.report.byte_vivi_iniziali, iniziali);
    for resoconto in &esito.report.passi {
        assert_eq!(resoconto.byte_vivi, iniziali, "{}", resoconto.out);
        assert_eq!(resoconto.byte_output_esclusivi, 0, "{}", resoconto.out);
    }
    // Ogni tabella intermedia muore al passo che la consuma.
    for (indice, resoconto) in esito.report.passi.iter().enumerate() {
        let atteso = if indice == 0 {
            "t".to_owned()
        } else {
            format!("p{}", indice - 1)
        };
        assert_eq!(resoconto.liberati, [atteso]);
    }
}

#[test]
fn dopo_l_ultimo_consumatore_i_byte_scendono_della_quota_esclusiva() {
    let a = wide(false);
    let b = destra();
    let pipeline = piano(
        &["a", "b", "inutile"],
        vec![
            passo(
                "sel",
                "table.select_columns",
                &["a"],
                json!({"columns": ["id", "value"]}),
            ),
            passo("ord", "table.sort", &["a"], json!({"columns": ["value"]})),
            passo(
                "scarto",
                "table.formula",
                &["b"],
                json!({"new_column": "doppio", "formula": "rvalue * 2"}),
            ),
        ],
        &["sel", "ord", "b"],
    );
    let esito = esegui(
        &pipeline,
        &[("a", a.clone()), ("b", b.clone()), ("inutile", destra())],
    )
    .expect("piano");
    let report = &esito.report;
    let byte = |tabelle: &[&RecordBatch]| byte_vivi(tabelle.iter().copied()).expect("byte");

    // Un input mai usato si libera prima del primo passo.
    assert_eq!(report.liberati_all_avvio, ["inutile"]);
    assert_eq!(report.byte_vivi_iniziali, byte(&[&a, &b]));

    let [p_sel, p_ord, p_scarto] = report.passi.as_slice() else {
        panic!("tre passi");
    };
    // `sel` condivide le colonne di `a`: nessun byte nuovo.
    assert_eq!(p_sel.byte_output_esclusivi, 0);
    assert!(p_sel.liberati.is_empty());
    assert_eq!(p_sel.byte_vivi, byte(&[&a, &b]));

    // `ord` e' l'ultimo consumatore di `a`: `a` muore, e delle sue
    // allocazioni restano vive solo quelle raggiunte da `sel`.
    let sel = output(&esito, "sel");
    let ord = output(&esito, "ord");
    assert_eq!(p_ord.liberati, ["a"]);
    assert_eq!(p_ord.byte_output_esclusivi, byte(&[ord]));
    let quota_esclusiva_di_a = byte(&[&a]) - byte(&[sel]);
    assert!(quota_esclusiva_di_a > 0);
    assert_eq!(
        p_ord.byte_vivi,
        p_sel.byte_vivi + p_ord.byte_output_esclusivi - quota_esclusiva_di_a
    );
    assert_eq!(p_ord.byte_vivi, byte(&[sel, ord, &b]));

    // Un'uscita che nessuno usa muore subito: i byte vivi non cambiano.
    assert_eq!(p_scarto.liberati, ["scarto"]);
    assert!(p_scarto.byte_output_esclusivi > 0);
    assert_eq!(p_scarto.byte_vivi, p_ord.byte_vivi);

    // Gli output nell'ordine del piano.
    let nomi: Vec<&str> = esito
        .outputs
        .iter()
        .map(|(nome, _)| nome.as_str())
        .collect();
    assert_eq!(nomi, ["sel", "ord", "b"]);
}

#[test]
fn stesso_input_stesso_esito() {
    let pipeline = piano(
        &["t"],
        vec![
            passo(
                "ord",
                "table.sort",
                &["t"],
                json!({"columns": ["value"], "ascending": false}),
            ),
            passo(
                "agg",
                "table.aggregate",
                &["ord"],
                json!({"group_by": ["name"],
                       "aggregations": [{"column": "value", "function": "sum"}]}),
            ),
        ],
        &["agg", "ord"],
    );
    let primo = esegui(&pipeline, &[("t", wide(false))]).expect("primo");
    let secondo = esegui(&pipeline, &[("t", wide(false))]).expect("secondo");
    assert_eq!(primo.outputs, secondo.outputs);
    assert_eq!(primo.report, secondo.report);
}

#[test]
fn le_tabelle_devono_essere_quelle_validate() {
    let pipeline = piano(&["t"], vec![], &["t"]);
    let validata = || {
        pipeline
            .validate(&[("t", wide(false).schema())])
            .expect("valida")
    };
    assert!(matches!(
        validata().run(vec![]),
        Err(PlenoraError::InvalidPlan(_))
    ));
    assert!(matches!(
        validata().run(vec![("t".into(), wide(false)), ("t".into(), wide(false))]),
        Err(PlenoraError::InvalidPlan(_))
    ));
    assert!(matches!(
        validata().run(vec![("t".into(), wide(false)), ("u".into(), wide(false))]),
        Err(PlenoraError::InvalidPlan(_))
    ));
    // Stesso nome, schema diverso da quello validato.
    assert!(matches!(
        validata().run(vec![("t".into(), destra())]),
        Err(PlenoraError::Schema(_))
    ));
}

#[test]
fn large_utf8_e_metadati_pandas_si_normalizzano_come_in_validazione() {
    let schema = Arc::new(Schema::new_with_metadata(
        vec![Field::new("testo", DataType::LargeUtf8, true)],
        [("pandas".to_owned(), "{}".to_owned())].into(),
    ));
    let colonna: ArrayRef = Arc::new(LargeStringArray::from(vec![Some("b"), None, Some("a")]));
    let tabella = RecordBatch::try_new(schema, vec![colonna]).expect("tabella");
    let pipeline = piano(
        &["t"],
        vec![passo(
            "ord",
            "table.sort",
            &["t"],
            json!({"columns": ["testo"]}),
        )],
        &["ord"],
    );
    let esito = esegui(&pipeline, &[("t", tabella)]).expect("piano");
    let ord = output(&esito, "ord");
    assert_eq!(ord.schema().field(0).data_type(), &DataType::Utf8);
    assert!(!ord.schema().metadata().contains_key("pandas"));
    let testo = ord
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("Utf8");
    assert_eq!(testo.iter().flatten().collect::<Vec<_>>(), ["a", "b"]);
}

const fn con_limiti(mut pipeline: Pipeline, limiti: LimitiParziali) -> Pipeline {
    pipeline.limits = Some(limiti);
    pipeline
}

#[test]
fn i_limiti_sulle_righe_si_applicano_ai_dati() {
    // `x` e' un arco intermedio: `max_rows_per_edge` vale li', non
    // sull'arco d'uscita (che ha `max_output_rows`).
    let incrocio = || {
        piano(
            &["a", "b"],
            vec![
                passo("x", "table.cross_join", &["a", "b"], json!({})),
                passo(
                    "y",
                    "table.rename",
                    &["x"],
                    json!({"renames": [{"old_name": "rid", "new_name": "chiave"}]}),
                ),
            ],
            &["y"],
        )
    };
    let tabelle = [("a", wide(false)), ("b", destra())];
    // 6 x 6 = 36 righe.
    let esito = esegui(&incrocio(), &tabelle).expect("incrocio");
    assert_eq!(output(&esito, "y").num_rows(), 36);

    let limitati = [
        LimitiParziali {
            max_rows_per_edge: Some(35),
            ..LimitiParziali::default()
        },
        LimitiParziali {
            max_output_rows: Some(35),
            ..LimitiParziali::default()
        },
        LimitiParziali {
            max_input_rows: Some(5),
            ..LimitiParziali::default()
        },
        // Vincolo SumRelative del catalogo: 36 righe su 6 + 6, fattore 3.
        LimitiParziali {
            max_expansion_factor: Some(2.5),
            ..LimitiParziali::default()
        },
    ];
    for limiti in limitati {
        let pipeline = con_limiti(incrocio(), limiti.clone());
        assert!(
            matches!(
                esegui(&pipeline, &tabelle),
                Err(PlenoraError::ResourceLimit(_))
            ),
            "{limiti:?}"
        );
    }
    // L'errore nomina il passo, non i dati.
    let per_arco = con_limiti(
        incrocio(),
        LimitiParziali {
            max_rows_per_edge: Some(35),
            ..LimitiParziali::default()
        },
    );
    let errore = esegui(&per_arco, &tabelle).expect_err("oltre max_rows_per_edge");
    assert!(errore.to_string().contains("passo `x`"), "{errore}");

    // Sull'arco d'uscita vale solo `max_output_rows`.
    let solo_uscita = con_limiti(
        piano(
            &["a", "b"],
            vec![passo("x", "table.cross_join", &["a", "b"], json!({}))],
            &["x"],
        ),
        LimitiParziali {
            max_rows_per_edge: Some(35),
            ..LimitiParziali::default()
        },
    );
    assert!(esegui(&solo_uscita, &tabelle).is_ok());

    let ammessa = con_limiti(
        incrocio(),
        LimitiParziali {
            max_expansion_factor: Some(3.0),
            ..LimitiParziali::default()
        },
    );
    assert!(esegui(&ammessa, &tabelle).is_ok());
}

#[test]
fn l_errore_di_un_kernel_non_porta_valori_di_riga() {
    let pipeline = piano(
        &["t"],
        vec![passo(
            "controllo",
            "table.assert_cardinality",
            &["t"],
            json!({"exact_rows": 7}),
        )],
        &["controllo"],
    );
    let errore = esegui(&pipeline, &[("t", wide(false))]).expect_err("6 righe, non 7");
    let testo = errore.to_string();
    for valore in ["wkb-a", "2024-01-02", "{\"a\":1}"] {
        assert!(!testo.contains(valore), "{testo}");
    }
}

/// `pivot` con `mapping`: lo schema validato e' quello eseguito, anche con
/// un valore mappato che i dati non contengono (colonna tutta null), e
/// l'esito e' quello della chiamata diretta; senza `mapping` lo schema
/// dipende dai dati e la validazione rifiuta.
#[test]
fn pivot_con_mapping_esegue_lo_schema_validato() {
    let tabella = wide(true);
    for aggr_func in [
        "first", "last", "max", "min", "sum", "mean", "count", "concat",
    ] {
        let config = json!({"index_col": "id", "pivot_col": "name", "value_col": "value",
                            "aggr_func": aggr_func,
                            "mapping": {"b": "vb", "assente": "vz", "a": "va"}});
        let pipeline = piano(
            &["t"],
            vec![passo("p", "table.pivot", &["t"], config.clone())],
            &["p"],
        );
        let validata = pipeline
            .validate(&[("t", tabella.schema())])
            .unwrap_or_else(|errore| panic!("{aggr_func}: {errore}"));
        let atteso = validata.contratto("p").expect("contratto").schema.clone();
        let esito = validata
            .run(vec![("t".to_owned(), tabella.clone())])
            .unwrap_or_else(|errore| panic!("{aggr_func}: {errore}"));
        let uscita = output(&esito, "p");
        assert_eq!(uscita.schema(), atteso, "{aggr_func}");
        let nomi: Vec<&str> = atteso
            .fields()
            .iter()
            .map(|campo| campo.name().as_str())
            .collect();
        assert_eq!(nomi, ["id", "va", "vz", "vb"], "ordine delle chiavi");
        let diretta = chiamata_diretta("table.pivot", &config, std::slice::from_ref(&tabella))
            .expect("chiamata diretta");
        assert_eq!(uscita.columns(), diretta.columns(), "{aggr_func}");
        let assente = uscita.column_by_name("vz").expect("colonna del mapping");
        assert_eq!(assente.null_count(), uscita.num_rows(), "{aggr_func}");
    }
    let senza_mapping = piano(
        &["t"],
        vec![passo(
            "p",
            "table.pivot",
            &["t"],
            json!({"index_col": "id", "pivot_col": "name", "value_col": "value"}),
        )],
        &["p"],
    );
    let errore = senza_mapping
        .validate(&[("t", tabella.schema())])
        .expect_err("schema dipendente dai dati");
    assert!(matches!(errore, PlenoraError::Unsupported(_)), "{errore}");
}
