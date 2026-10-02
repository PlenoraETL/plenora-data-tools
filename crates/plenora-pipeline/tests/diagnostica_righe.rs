//! Diagnostica per riga dopo passi che cambiano numero o ordine delle righe.
//!
//! Ogni ordine valido della catena si accetta. Gli esempi della diagnostica
//! portano righe della sorgente quando la catena a monte le conserva;
//! altrimenti il payload ha i soli conteggi, con il limite di conoscenza
//! `read.row_attribution_unavailable` (DIAG-003: mai un indice di un'altra
//! base). La decisione si prende in validazione
//! (`PipelineValidata::base_indici`). L'oracolo dei conteggi è il piano
//! spezzato: l'emettitore eseguito da solo sull'output del prefisso.

mod comune;
mod comune_geo;

use std::collections::BTreeSet;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::catalog::{find_operation, Family, SourceRowProvenance, CATALOG};
use plenora_core::diagnostics::{
    RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    KNOWLEDGE_LIMIT_ROW_ATTRIBUTION_UNAVAILABLE, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::{ErrorPhase, PlenoraError, Result};
use plenora_pipeline::{byte_vivi, BaseIndici, Esito, Passo, Pipeline, PipelineValidata};
use serde_json::{json, Value};

/// Righe della sorgente.
const RIGHE: i64 = 10;
/// Riga della sorgente con la cella che fallisce in ogni operazione.
const RIGA_CATTIVA: i64 = 7;

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

/// La sorgente `t`: `id` 0..10, chiave testuale `k`, e in ogni colonna che
/// un'operazione legge una cella che fallisce alla riga [`RIGA_CATTIVA`]
/// (se `con_difetto`): `s` non numerico, `d` zero come divisore, `data` non
/// una data.
fn sorgente(con_difetto: bool) -> RecordBatch {
    let cattiva = |riga: i64| con_difetto && riga == RIGA_CATTIVA;
    let righe: Vec<i64> = (0..RIGHE).collect();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("k", DataType::Utf8, false),
            Field::new("s", DataType::Utf8, false),
            Field::new("v", DataType::Float64, false),
            Field::new("d", DataType::Float64, false),
            Field::new("data", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(righe.clone())),
            Arc::new(StringArray::from(
                righe
                    .iter()
                    .map(|riga| format!("k{riga}"))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                righe
                    .iter()
                    .map(|riga| {
                        if cattiva(*riga) {
                            "non un numero".to_owned()
                        } else {
                            (riga * 3).to_string()
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                righe
                    .iter()
                    .map(|riga| f64::from(i32::try_from(*riga).expect("riga")) + 0.5)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                righe
                    .iter()
                    .map(|riga| if cattiva(*riga) { 0.0 } else { 2.0 })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                righe
                    .iter()
                    .map(|riga| {
                        if cattiva(*riga) {
                            "non una data".to_owned()
                        } else {
                            format!("2024-01-{:02}", riga + 1)
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("sorgente")
}

/// La tabella destra dei join: `rid` 0..10 in ordine inverso, `rv`.
fn destra() -> RecordBatch {
    let righe: Vec<i64> = (0..RIGHE).rev().collect();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("rid", DataType::Int64, false),
            Field::new("rv", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(righe.clone())),
            Arc::new(Float64Array::from(
                righe
                    .iter()
                    .map(|riga| f64::from(i32::try_from(*riga).expect("riga")) * 10.0)
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("destra")
}

fn valida(pipeline: &Pipeline, tabelle: &[(&str, RecordBatch)]) -> Result<PipelineValidata> {
    let schemi: Vec<(&str, SchemaRef)> = tabelle
        .iter()
        .map(|(nome, tabella)| (*nome, tabella.schema()))
        .collect();
    pipeline.validate(&schemi)
}

fn esegui(pipeline: &Pipeline, tabelle: &[(&str, RecordBatch)]) -> Result<Esito> {
    valida(pipeline, tabelle)?.run(
        tabelle
            .iter()
            .map(|(nome, tabella)| ((*nome).to_owned(), tabella.clone()))
            .collect(),
    )
}

fn output(esito: &Esito, nome: &str) -> RecordBatch {
    esito
        .outputs
        .iter()
        .find(|(candidato, _)| candidato == nome)
        .expect("output del piano")
        .1
        .clone()
}

fn filtro(out: &str, input: &str, operatore: &str, valore: i64) -> Passo {
    passo(
        out,
        "table.filter",
        &[input],
        json!({"column": "id", "operator": operatore, "value": valore}),
    )
}

/// Le quattro operazioni con diagnostica per riga del difetto segnalato,
/// ognuna con una config che fallisce alla riga cattiva.
fn emettitori() -> Vec<(&'static str, Value)> {
    vec![
        (
            "table.type_cast",
            json!({"column": "s", "target_type": "int"}),
        ),
        (
            "table.formula",
            json!({"new_column": "rapporto", "formula": "v / d", "on_division_by_zero": "error"}),
        ),
        (
            "table.expression",
            json!({"output_column": "rapporto", "on_division_by_zero": "error", "expression": {
                "kind": "binary", "op": "divide",
                "left": {"kind": "column", "name": "v"},
                "right": {"kind": "column", "name": "d"}}}),
        ),
        (
            "table.date_extract",
            json!({"column": "data", "parts": ["year", "month"]}),
        ),
    ]
}

/// Una forma: gli input, i passi a monte dell'emettitore (il prefisso), il
/// nome della tabella che l'emettitore riceve.
struct Forma {
    nome: &'static str,
    prefisso: Vec<Passo>,
    ingresso: &'static str,
    usa_destra: bool,
}

fn forme() -> Vec<Forma> {
    vec![
        Forma {
            nome: "filter",
            prefisso: vec![filtro("f", "t", ">", 3)],
            ingresso: "f",
            usa_destra: false,
        },
        Forma {
            nome: "filtri multipli",
            prefisso: vec![filtro("f1", "t", ">", 3), filtro("f2", "f1", "!=", 5)],
            ingresso: "f2",
            usa_destra: false,
        },
        Forma {
            nome: "sort",
            prefisso: vec![passo(
                "o",
                "table.sort",
                &["t"],
                json!({"columns": ["id"], "ascending": false}),
            )],
            ingresso: "o",
            usa_destra: false,
        },
        Forma {
            nome: "join",
            // `r` a sinistra, in ordine inverso: la riga 7 della sorgente
            // finisce alla posizione 2. Il rename rimette i nomi della
            // sorgente (le destre non chiave prendono `_R`).
            prefisso: vec![
                passo(
                    "j",
                    "table.join",
                    &["r", "t"],
                    json!({"left_keys": ["rid"], "right_keys": ["id"], "how": "inner"}),
                ),
                passo(
                    "jn",
                    "table.rename",
                    &["j"],
                    json!({"renames": [
                        {"old_name": "rid", "new_name": "id"},
                        {"old_name": "k_R", "new_name": "k"},
                        {"old_name": "s_R", "new_name": "s"},
                        {"old_name": "v_R", "new_name": "v"},
                        {"old_name": "d_R", "new_name": "d"},
                        {"old_name": "data_R", "new_name": "data"}]}),
                ),
            ],
            ingresso: "jn",
            usa_destra: true,
        },
        Forma {
            nome: "filter, poi rename (conserva le righe)",
            prefisso: vec![
                filtro("f", "t", "<", 9),
                passo(
                    "n",
                    "table.rename",
                    &["f"],
                    json!({"renames": [{"old_name": "k", "new_name": "chiave"}]}),
                ),
            ],
            ingresso: "n",
            usa_destra: false,
        },
    ]
}

fn tabelle(con_difetto: bool, usa_destra: bool) -> Vec<(&'static str, RecordBatch)> {
    let mut tabelle = vec![("t", sorgente(con_difetto))];
    if usa_destra {
        tabelle.push(("r", destra()));
    }
    tabelle
}

fn diagnostica(errore: &PlenoraError) -> &RowDiagnostics {
    let report = errore
        .row_diagnostics()
        .unwrap_or_else(|| panic!("diagnostica per riga attesa: {errore}"));
    verifica_fase(errore, report);
    report
}

/// Un rifiuto per riga di un passo non nasce leggendo un supporto: fase
/// derivata `write` (l'esecuzione), nessun tag, e lo scope `read` del
/// payload (la riga rifiutata è d'ingresso).
fn verifica_fase(errore: &PlenoraError, report: &RowDiagnostics) {
    assert_eq!(errore.phase(), ErrorPhase::Write, "{errore}");
    assert_eq!(errore.phase_tag(), None, "{errore}");
    assert_eq!(report.scope, RowDiagnosticScope::Read, "{errore}");
}

/// La diagnostica del piano spezzato: il prefisso eseguito da solo, poi
/// l'emettitore sul suo output come input del piano.
fn diagnostica_spezzata(forma: &Forma, op: &str, config: Value) -> RowDiagnostics {
    let ingressi: Vec<&str> = if forma.usa_destra {
        vec!["t", "r"]
    } else {
        vec!["t"]
    };
    let prefisso = piano(&ingressi, forma.prefisso.clone(), &[forma.ingresso]);
    let esito = esegui(&prefisso, &tabelle(true, forma.usa_destra)).expect("prefisso");
    let intermedia = output(&esito, forma.ingresso);
    let seconda = piano(&["t"], vec![passo("x", op, &["t"], config)], &["x"]);
    let errore = esegui(&seconda, &[("t", intermedia)]).expect_err("cella cattiva");
    diagnostica(&errore).clone()
}

/// Il rifiuto di un passo `x` dopo un passo che cambia le righe: conteggi
/// senza esempi, con il limite di conoscenza, e il testo che nomina passo e
/// ingresso.
fn verifica_senza_attribuzione(errore: &PlenoraError, ingresso: &str) -> RowDiagnostics {
    let report = diagnostica(errore);
    assert_eq!(report.index_basis, ROW_DIAGNOSTICS_INDEX_BASIS, "{errore}");
    assert!(report.examples.is_empty(), "{errore}");
    // DIAG-007: gli esempi osservati sono stati omessi.
    assert!(report.examples_truncated, "{errore}");
    assert_eq!(
        report.completeness,
        RowDiagnosticsCompleteness::Partial,
        "{errore}"
    );
    assert_eq!(
        report.knowledge_limits,
        Some(vec![KNOWLEDGE_LIMIT_ROW_ATTRIBUTION_UNAVAILABLE.to_owned()]),
        "{errore}"
    );
    assert_eq!(report.total, Some(report.observed_total), "{errore}");
    assert!(report.validate_for_emission().is_ok());
    let testo = errore.to_string();
    assert!(testo.contains("passo `x`"), "{testo}");
    assert!(testo.contains(&format!("ingresso `{ingresso}`")), "{testo}");
    assert!(testo.contains("non riconducibili alla sorgente"), "{testo}");
    report.clone()
}

#[test]
fn dopo_un_passo_che_cambia_le_righe_la_diagnostica_ha_solo_i_conteggi() {
    for forma in forme() {
        let ingressi: Vec<&str> = if forma.usa_destra {
            vec!["t", "r"]
        } else {
            vec!["t"]
        };
        for (op, config) in emettitori() {
            let mut passi = forma.prefisso.clone();
            passi.push(passo("x", op, &[forma.ingresso], config.clone()));
            let pipeline = piano(&ingressi, passi, &["x"]);
            let tavole = tabelle(true, forma.usa_destra);
            let validata = valida(&pipeline, &tavole)
                .unwrap_or_else(|errore| panic!("{} -> {op}: rifiutata: {errore}", forma.nome));
            assert_eq!(
                validata.base_indici("x"),
                Some(BaseIndici::SenzaAttribuzione),
                "{} -> {op}",
                forma.nome
            );
            let spezzata = diagnostica_spezzata(&forma, op, config);
            let errore = validata
                .run(
                    tavole
                        .iter()
                        .map(|(nome, tabella)| ((*nome).to_owned(), tabella.clone()))
                        .collect(),
                )
                .expect_err("cella cattiva");
            let report = verifica_senza_attribuzione(&errore, forma.ingresso);
            // I conteggi sono quelli del piano spezzato, che ha anche
            // l'esempio (riga dell'ingresso del passo, non della sorgente:
            // per questo nella catena non si pubblica).
            assert_eq!(report.observed_total, 1, "{} -> {op}", forma.nome);
            assert_eq!(report.counts, spezzata.counts, "{} -> {op}", forma.nome);
            assert_eq!(spezzata.examples.len(), 1, "{} -> {op}", forma.nome);
            let testo = errore.to_string();
            // Errori senza dati: nessuna cella della riga cattiva nel testo.
            for valore in ["non un numero", "non una data", "7.5", "21"] {
                assert!(!testo.contains(valore), "{op}: dato nel messaggio: {testo}");
            }
        }
    }
}

#[test]
fn con_la_catena_che_conserva_le_righe_gli_indici_restano_della_sorgente() {
    for (op, config) in emettitori() {
        // Diretto sulla sorgente e dopo un passo che conserva le righe.
        for prefisso in [
            Vec::new(),
            vec![passo(
                "n",
                "table.rename",
                &["t"],
                json!({"renames": [{"old_name": "k", "new_name": "chiave"}]}),
            )],
        ] {
            let ingresso = if prefisso.is_empty() { "t" } else { "n" };
            let mut passi = prefisso.clone();
            passi.push(passo("x", op, &[ingresso], config.clone()));
            let pipeline = piano(&["t"], passi, &["x"]);
            let tavole = tabelle(true, false);
            let validata = valida(&pipeline, &tavole).expect("valida");
            assert_eq!(validata.base_indici("x"), Some(BaseIndici::Sorgente));
            let errore = esegui(&pipeline, &tavole).expect_err("cella cattiva");
            let report = diagnostica(&errore);
            assert_eq!(report.index_basis, ROW_DIAGNOSTICS_INDEX_BASIS, "{op}");
            assert_eq!(
                report.examples[0].source_index,
                u64::try_from(RIGA_CATTIVA).expect("riga"),
                "{op}"
            );
            // Il testo resta quello del kernel.
            assert!(
                !errore.to_string().contains("non riconducibili"),
                "{op}: {errore}"
            );
        }
    }
}

#[test]
fn aggregate_poi_formula_conta_senza_esempi() {
    // Le righe dell'aggregazione sono gruppi nuovi: nessuna riga della
    // sorgente da riportare, solo il conteggio.
    let aggrega = passo(
        "g",
        "table.aggregate",
        &["t"],
        json!({"group_by": ["k"], "aggregations": [
            {"column": "d", "function": "min", "alias": "dmin"},
            {"column": "v", "function": "max", "alias": "vmax"}]}),
    );
    let pipeline = piano(
        &["t"],
        vec![
            aggrega,
            passo(
                "x",
                "table.formula",
                &["g"],
                json!({"new_column": "rapporto", "formula": "vmax / dmin",
                        "on_division_by_zero": "error"}),
            ),
        ],
        &["x"],
    );
    let tavole = tabelle(true, false);
    let validata = valida(&pipeline, &tavole).expect("aggregate -> formula accettata");
    assert_eq!(
        validata.base_indici("x"),
        Some(BaseIndici::SenzaAttribuzione)
    );
    let errore = esegui(&pipeline, &tavole).expect_err("divisore zero nel gruppo k7");
    let report = verifica_senza_attribuzione(&errore, "g");
    assert_eq!(report.observed_total, 1);
}

#[test]
fn le_catene_prima_rifiutate_danno_gli_stessi_risultati_dei_piani_spezzati() {
    // Senza celle cattive ogni forma riesce, e l'output e' quello del piano
    // spezzato in due (prefisso, poi l'emettitore sul suo output come input):
    // la catena unica non cambia i risultati.
    for forma in forme() {
        let ingressi: Vec<&str> = if forma.usa_destra {
            vec!["t", "r"]
        } else {
            vec!["t"]
        };
        let tavole = tabelle(false, forma.usa_destra);
        let prefisso = esegui(
            &piano(&ingressi, forma.prefisso.clone(), &[forma.ingresso]),
            &tavole,
        )
        .expect("prefisso");
        let intermedia = output(&prefisso, forma.ingresso);
        for (op, config) in emettitori() {
            let mut passi = forma.prefisso.clone();
            passi.push(passo("x", op, &[forma.ingresso], config.clone()));
            let unica = esegui(&piano(&ingressi, passi, &["x"]), &tavole)
                .unwrap_or_else(|errore| panic!("{} -> {op}: {errore}", forma.nome));
            let seconda = piano(&["t"], vec![passo("x", op, &["t"], config)], &["x"]);
            let spezzata = esegui(&seconda, &[("t", intermedia.clone())])
                .unwrap_or_else(|errore| panic!("{} -> {op} spezzato: {errore}", forma.nome));
            assert_eq!(
                output(&unica, "x"),
                output(&spezzata, "x"),
                "{} -> {op}",
                forma.nome
            );
        }
    }
}

#[test]
fn filtro_e_ordinamento_commutano_con_gli_emettitori() {
    // Le catene riordinate a mano (l'emettitore prima del filtro o
    // dell'ordinamento) e quelle nell'ordine naturale danno lo stesso
    // output.
    let tavole = tabelle(false, false);
    let cambiano = [
        (
            "table.filter",
            json!({"column": "id", "operator": ">", "value": 3}),
        ),
        ("table.sort", json!({"columns": ["id"], "ascending": false})),
    ];
    for (cambia, config_cambia) in cambiano {
        for (op, config) in emettitori() {
            let prima = piano(
                &["t"],
                vec![
                    passo("a", cambia, &["t"], config_cambia.clone()),
                    passo("x", op, &["a"], config.clone()),
                ],
                &["x"],
            );
            let dopo = piano(
                &["t"],
                vec![
                    passo("a", op, &["t"], config),
                    passo("x", cambia, &["a"], config_cambia.clone()),
                ],
                &["x"],
            );
            let prima = esegui(&prima, &tavole).expect("naturale");
            let dopo = esegui(&dopo, &tavole).expect("riordinata");
            assert_eq!(output(&prima, "x"), output(&dopo, "x"), "{cambia} / {op}");
        }
    }
}

#[test]
fn foreign_key_con_destra_filtrata_resta_sulla_sorgente_di_sinistra() {
    // Gli indici di assert_foreign_key sono righe del lato left: un filtro
    // sulla destra non li tocca.
    let pipeline = piano(
        &["t", "r"],
        vec![
            passo(
                "rf",
                "table.filter",
                &["r"],
                json!({"column": "rid", "operator": ">", "value": 5}),
            ),
            passo(
                "x",
                "table.assert_foreign_key",
                &["t", "rf"],
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
            ),
        ],
        &["x"],
    );
    let tavole = tabelle(false, true);
    let validata = valida(&pipeline, &tavole).expect("valida");
    assert_eq!(validata.base_indici("x"), Some(BaseIndici::Sorgente));
    let errore = esegui(&pipeline, &tavole).expect_err("chiavi 0..=5 assenti");
    let report = diagnostica(&errore);
    assert_eq!(report.index_basis, ROW_DIAGNOSTICS_INDEX_BASIS);
    assert_eq!(report.observed_total, 6);
    let indici: Vec<u64> = report
        .examples
        .iter()
        .map(|esempio| esempio.source_index)
        .collect();
    assert_eq!(indici, vec![0, 1, 2, 3, 4, 5]);

    // Con il lato left filtrato le righe non si riconducono alla sorgente:
    // solo il conteggio.
    let pipeline = piano(
        &["t", "r"],
        vec![
            filtro("tf", "t", ">", 3),
            passo(
                "rf",
                "table.filter",
                &["r"],
                json!({"column": "rid", "operator": ">", "value": 5}),
            ),
            passo(
                "x",
                "table.assert_foreign_key",
                &["tf", "rf"],
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
            ),
        ],
        &["x"],
    );
    let validata = valida(&pipeline, &tavole).expect("valida");
    assert_eq!(
        validata.base_indici("x"),
        Some(BaseIndici::SenzaAttribuzione)
    );
    let errore = esegui(&pipeline, &tavole).expect_err("chiavi 4 e 5 assenti");
    // tf tiene gli id 4..=9: mancano a destra 4 e 5.
    let report = verifica_senza_attribuzione(&errore, "tf");
    assert_eq!(report.observed_total, 2);
}

#[test]
fn la_base_non_aggiunge_memoria_ai_byte_vivi() {
    // La base degli indici è una decisione di validazione, non una mappa
    // per riga: i byte vivi dopo ogni passo sono quelli delle tabelle.
    let pipeline = piano(
        &["t"],
        vec![
            filtro("f", "t", ">", 3),
            passo(
                "x",
                "table.formula",
                &["f"],
                json!({"new_column": "rapporto", "formula": "v / d", "on_division_by_zero": "error"}),
            ),
        ],
        &["x"],
    );
    let esito = esegui(&pipeline, &tabelle(false, false)).expect("riuscita");
    let x = output(&esito, "x");
    let ultimo = esito.report.passi.last().expect("passi");
    assert_eq!(ultimo.liberati, vec!["f".to_owned()]);
    assert_eq!(
        ultimo.byte_vivi,
        byte_vivi(std::iter::once(&x)).expect("byte")
    );
}

#[test]
fn from_wkt_dopo_un_filtro_conta_senza_esempi() {
    // L'unica geo con diagnostica per riga nel runner segue la stessa base.
    let testi = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("wkt", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from((0..RIGHE).collect::<Vec<_>>())),
            Arc::new(StringArray::from(
                (0..RIGHE)
                    .map(|riga| {
                        if riga == RIGA_CATTIVA {
                            "non wkt".to_owned()
                        } else {
                            format!("POINT ({} 5000000)", 500_000 + riga)
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("testi");
    for (prefisso, ingresso, base) in [
        (Vec::new(), "t", BaseIndici::Sorgente),
        (
            vec![filtro("f", "t", ">", 3)],
            "f",
            BaseIndici::SenzaAttribuzione,
        ),
    ] {
        let mut passi = prefisso;
        passi.push(passo(
            "x",
            "geo.from_wkt",
            &[ingresso],
            json!({"wkt_column": "wkt"}),
        ));
        let mut pipeline = piano(&["t"], passi, &["x"]);
        pipeline.crs = Some("EPSG:32632".to_owned());
        let tavole = [("t", testi.clone())];
        let validata = valida(&pipeline, &tavole).expect("valida");
        assert_eq!(validata.base_indici("x"), Some(base));
        let errore = esegui(&pipeline, &tavole).expect_err("cella non WKT");
        if base == BaseIndici::Sorgente {
            let report = diagnostica(&errore);
            assert_eq!(report.index_basis, ROW_DIAGNOSTICS_INDEX_BASIS);
            assert_eq!(report.examples[0].source_index, 7);
        } else {
            assert_eq!(
                verifica_senza_attribuzione(&errore, ingresso).observed_total,
                1
            );
        }
        assert!(!errore.to_string().contains("non wkt"), "{errore}");
    }
}

#[test]
fn flatten_json_rifiuta_con_la_fase_dell_esecuzione() {
    // L'altro rifiuto per riga dei kernel (documenti JSON), sulla stessa
    // regola di fase: prima lo taggava `read`.
    let documenti = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("json", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec![
            r#"{"a": 1}"#,
            "non json",
            r#"{"a": 2}"#,
        ]))],
    )
    .expect("documenti");
    let pipeline = piano(
        &["t"],
        vec![passo(
            "x",
            "table.flatten_json",
            &["t"],
            json!({"column": "json", "output_columns": ["json_a"]}),
        )],
        &["x"],
    );
    let errore = esegui(&pipeline, &[("t", documenti)]).expect_err("documento non JSON");
    let report = diagnostica(&errore);
    assert_eq!(report.examples[0].source_index, 1);
}

/// Colonna nascosta con il numero di riga d'ingresso, per l'oracolo della
/// classificazione `Preserved`.
const RIGA_NASCOSTA: &str = "__riga_oracolo";

/// La tabella con in coda [`RIGA_NASCOSTA`] (`Int64`, 0..n), metadati di
/// schema e di campo invariati.
fn con_riga_nascosta(tabella: &RecordBatch) -> RecordBatch {
    let schema = tabella.schema();
    let mut campi: Vec<Field> = schema
        .fields()
        .iter()
        .map(|campo| campo.as_ref().clone())
        .collect();
    campi.push(Field::new(RIGA_NASCOSTA, DataType::Int64, false));
    let mut colonne = tabella.columns().to_vec();
    colonne.push(Arc::new(Int64Array::from(
        (0..tabella.num_rows())
            .map(|riga| i64::try_from(riga).expect("riga"))
            .collect::<Vec<_>>(),
    )));
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(campi, schema.metadata().clone())),
        colonne,
    )
    .expect("tabella con la riga nascosta")
}

/// Operazioni `Preserved` che per config proiettano le colonne e quindi
/// tolgono la riga nascosta: per loro l'identita' si prova colonna per
/// colonna. L'elenco e' chiuso: un'operazione nuova che perde la colonna
/// fa fallire l'oracolo finche' non la si aggiunge qui con il motivo.
const PROIEZIONI: &[&str] = &[
    // `columns` elenca le colonne tenute.
    "table.select_columns",
    // Lo schema d'uscita e' quello dichiarato in `columns`.
    "table.align_schema",
];

/// Un passo `Preserved` eseguito dal runner sulla tabella con la riga
/// nascosta (il primo ingresso). Le righe d'uscita devono essere quelle
/// d'ingresso, una per una e nello stesso ordine: la riga nascosta vale
/// 0..n; dove l'operazione la proietta via, ogni colonna rimasta uguale
/// per nome e tipo a una d'ingresso ha gli stessi valori.
fn verifica_righe_conservate(
    op: &str,
    config: &Value,
    ingressi: &[RecordBatch],
    crs: Option<&str>,
) {
    let marcato = con_riga_nascosta(&ingressi[0]);
    let mut tavole = vec![marcato.clone()];
    tavole.extend(ingressi[1..].iter().cloned());
    let nomi: Vec<&str> = ["t", "u"].into_iter().take(tavole.len()).collect();
    let mut pipeline = piano(&nomi, vec![passo("x", op, &nomi, config.clone())], &["x"]);
    pipeline.crs = crs.map(str::to_owned);
    let coppie: Vec<(&str, RecordBatch)> = nomi.iter().copied().zip(tavole).collect();
    let esito = esegui(&pipeline, &coppie).unwrap_or_else(|errore| panic!("{op}: {errore}"));
    let uscita = output(&esito, "x");
    assert_eq!(uscita.num_rows(), marcato.num_rows(), "{op}: righe");
    if let Some(riga) = uscita.column_by_name(RIGA_NASCOSTA) {
        assert_eq!(
            riga.as_ref(),
            marcato
                .column_by_name(RIGA_NASCOSTA)
                .expect("riga nascosta")
                .as_ref(),
            "{op}: identita' e ordine delle righe"
        );
        return;
    }
    assert!(
        PROIEZIONI.contains(&op),
        "{op}: la riga nascosta sparisce e l'operazione non e' fra le proiezioni"
    );
    let mut confrontate = 0;
    for campo in uscita.schema().fields() {
        if let Ok(indice) = marcato.schema().index_of(campo.name()) {
            let prima = marcato.column(indice);
            if prima.data_type() == campo.data_type() {
                let dopo = uscita.column_by_name(campo.name()).expect("colonna");
                assert_eq!(
                    prima.as_ref(),
                    dopo.as_ref(),
                    "{op}: colonna {}",
                    campo.name()
                );
                confrontate += 1;
            }
        }
    }
    assert!(confrontate > 0, "{op}: nessuna colonna da confrontare");
}

/// La base `Sorgente` si fida della classificazione `Preserved` del
/// catalogo: un'operazione dichiarata cosi' che cambiasse numero, ordine o
/// identita' delle righe farebbe leggere come righe della sorgente indici
/// che non lo sono. Oracolo sulle config rappresentative delle tabellari.
#[test]
fn le_tabellari_che_conservano_le_righe_le_conservano_davvero() {
    std::env::set_var(comune::CHIAVE_HMAC, "chiave-di-test-del-runner");
    let mut provate = BTreeSet::new();
    for caso in comune::CASI {
        let descrittore = find_operation(caso.op).expect("operazione del catalogo");
        if descrittore.source_row_provenance() != SourceRowProvenance::Preserved {
            continue;
        }
        let config: Value = serde_json::from_str(caso.config).expect("config del caso");
        verifica_righe_conservate(caso.op, &config, &comune::tabelle(caso.fixture), None);
        provate.insert(caso.op);
    }
    let attese: BTreeSet<&str> = CATALOG
        .iter()
        .filter(|op| {
            op.family == Family::Table
                && op.source_row_provenance() == SourceRowProvenance::Preserved
        })
        .map(|op| op.id)
        .collect();
    assert_eq!(provate, attese, "ogni tabellare Preserved ha un caso");
}

/// Lo stesso oracolo sulle geo `Preserved` (1:1, unarie e binarie, e
/// produttori), con geometrie nulle in mezzo alle righe.
#[test]
#[allow(clippy::too_many_lines)] // Un caso per operazione, in un solo elenco.
fn le_geo_che_conservano_le_righe_le_conservano_davvero() {
    use comune_geo::{esadecimale_di, quadrato, tabella, LONLAT, UTM, X0, Y0};
    use geo::{Geometry, LineString, Point};

    let poligoni = tabella(
        UTM,
        &[
            Some(Geometry::Polygon(quadrato(X0, Y0, 100.0))),
            None,
            Some(Geometry::Polygon(quadrato(X0 + 50.0, Y0 + 50.0, 100.0))),
            None,
            Some(Geometry::Polygon(quadrato(X0 + 500.0, Y0, 30.0))),
        ],
    );
    let segmento_utm = |dx: f64| {
        Some(Geometry::LineString(LineString::from(vec![
            (X0 + dx, Y0),
            (X0 + dx + 100.0, Y0 + 7.0),
            (X0 + dx + 200.0, Y0),
        ])))
    };
    let linee = tabella(
        UTM,
        &[
            segmento_utm(0.0),
            None,
            segmento_utm(300.0),
            None,
            segmento_utm(600.0),
        ],
    );
    let posizione_utm = |dx: f64, dy: f64| Some(Geometry::Point(Point::new(X0 + dx, Y0 + dy)));
    let punti = tabella(
        UTM,
        &[
            posizione_utm(0.0, 0.0),
            None,
            posizione_utm(10.0, 3.0),
            posizione_utm(400.0, -20.0),
            None,
            posizione_utm(405.0, -22.0),
        ],
    );
    let punti_lonlat = tabella(
        LONLAT,
        &[
            Some(Geometry::Point(Point::new(9.19, 45.46))),
            None,
            Some(Geometry::Point(Point::new(12.49, 41.9))),
            Some(Geometry::Point(Point::new(11.25, 43.77))),
        ],
    );
    let segmento_geografico = |dy: f64| {
        Some(Geometry::LineString(LineString::from(vec![
            (9.0, 45.0 + dy),
            (10.0, 45.5 + dy),
            (11.0, 45.0 + dy),
        ])))
    };
    let linee_lonlat = tabella(
        LONLAT,
        &[
            segmento_geografico(0.0),
            None,
            segmento_geografico(-1.0),
            segmento_geografico(-2.0),
        ],
    );
    let poligoni_lonlat = tabella(
        LONLAT,
        &[
            Some(Geometry::Polygon(quadrato(9.0, 45.0, 0.5))),
            None,
            Some(Geometry::Polygon(quadrato(11.0, 43.0, 0.2))),
        ],
    );
    let coordinate = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![X0, X0 + 1.5, X0 + 7.0])),
            Arc::new(Float64Array::from(vec![Y0, Y0 + 10.0, Y0 - 3.0])),
        ],
    )
    .expect("coordinate");
    let testi = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("wkt", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(vec![
            Some("POINT (500000 5000000)"),
            None,
            Some("LINESTRING (500000 5000000, 500010 5000010)"),
            None,
            Some("POINT (500005 5000005)"),
        ]))],
    )
    .expect("testi");

    let altra_linea = Geometry::LineString(LineString::from(vec![
        (X0, Y0 + 20.0),
        (X0 + 150.0, Y0 + 30.0),
    ]));
    let altro_poligono = Geometry::Polygon(quadrato(X0 + 20.0, Y0 + 20.0, 60.0));
    // Torino: nessun punto della fixture coincide (fra punti coincidenti
    // l'azimut non e' definito e il passo si ferma).
    let milano = Geometry::Point(Point::new(7.686, 45.07));
    let riferimento = Geometry::Point(Point::new(X0 + 120.0, Y0 + 40.0));
    let altro = esadecimale_di(&altro_poligono);

    let mut casi: Vec<(&str, Value, &RecordBatch, Option<&str>)> = vec![
        ("geo.centroid", json!({}), &poligoni, None),
        ("geo.convex_hull", json!({}), &poligoni, None),
        ("geo.envelope", json!({}), &poligoni, None),
        ("geo.area", json!({}), &poligoni, None),
        ("geo.boundary", json!({}), &poligoni, None),
        ("geo.bounds_extractor", json!({}), &poligoni, None),
        ("geo.buffer", json!({"distance": 3.0}), &poligoni, None),
        (
            "geo.distance",
            json!({"other_wkb": esadecimale_di(&altra_linea)}),
            &linee,
            None,
        ),
        ("geo.from_coords", json!({}), &coordinate, Some(UTM)),
        ("geo.length", json!({}), &linee, None),
        ("geo.perimeter", json!({}), &poligoni, None),
        ("geo.point_on_surface", json!({}), &poligoni, None),
        ("geo.simplify", json!({"tolerance": 10.0}), &linee, None),
        ("geo.to_wkt", json!({}), &linee, None),
        ("geo.vertex_count", json!({}), &poligoni, None),
        ("geo.make_valid", json!({}), &poligoni, None),
        (
            "geo.reproject",
            json!({"target_crs": "EPSG:4326"}),
            &linee,
            None,
        ),
        (
            "geo.affine_transform",
            json!({"coefficients": [1.0, 0.0, 0.0, 1.0, 5.0, -5.0]}),
            &linee,
            None,
        ),
        (
            "geo.translate",
            json!({"x_offset": 10.0, "y_offset": -3.0}),
            &linee,
            None,
        ),
        (
            "geo.scale",
            json!({"x_factor": 1.5, "y_factor": 1.0, "x_origin": X0, "y_origin": Y0}),
            &linee,
            None,
        ),
        (
            "geo.rotate",
            json!({"degrees": 30.0, "x_origin": X0, "y_origin": Y0}),
            &linee,
            None,
        ),
        (
            "geo.concave_hull",
            json!({"concavity": 2.0}),
            &poligoni,
            None,
        ),
        (
            "geo.hausdorff_distance",
            json!({"other_wkb": esadecimale_di(&altra_linea)}),
            &linee,
            None,
        ),
        (
            "geo.haversine_distance",
            json!({"other_wkb": esadecimale_di(&milano)}),
            &punti_lonlat,
            None,
        ),
        (
            "geo.geodesic_distance",
            json!({"other_wkb": esadecimale_di(&milano)}),
            &punti_lonlat,
            None,
        ),
        ("geo.geodesic_line_length", json!({}), &linee_lonlat, None),
        (
            "geo.densify",
            json!({"max_segment_length": 25.0}),
            &linee,
            None,
        ),
        ("geo.snap_to_grid", json!({"grid_size": 4.0}), &linee, None),
        (
            "geo.line_substring",
            json!({"start_ratio": 0.25, "end_ratio": 0.75}),
            &linee,
            None,
        ),
        (
            "geo.line_interpolate_point",
            json!({"ratio": 0.4}),
            &linee,
            None,
        ),
        (
            "geo.frechet_distance",
            json!({"other_wkb": esadecimale_di(&altra_linea)}),
            &linee,
            None,
        ),
        (
            "geo.bearing",
            json!({"other_wkb": esadecimale_di(&milano)}),
            &punti_lonlat,
            None,
        ),
        ("geo.geodesic_area", json!({}), &poligoni_lonlat, None),
        (
            "geo.from_wkt",
            json!({"wkt_column": "wkt"}),
            &testi,
            Some(UTM),
        ),
        (
            "geo.geometry_accessors",
            json!({"fields": ["is_closed", "geometry_type"], "output_prefix": "a_"}),
            &linee,
            None,
        ),
        (
            "geo.line_locate_point",
            json!({"point_wkb": esadecimale_di(&riferimento)}),
            &linee,
            None,
        ),
        (
            "geo.snap",
            json!({"reference_wkb": esadecimale_di(&riferimento), "tolerance": 2.0}),
            &linee,
            None,
        ),
        (
            "geo.cluster_dbscan",
            json!({"eps": 15.0, "min_points": 2}),
            &punti,
            None,
        ),
        (
            "geo.clean_topology",
            json!({"snap_tolerance": 0, "remove_overlaps": true, "fill_gaps": false}),
            &poligoni,
            None,
        ),
        ("geo.voronoi", json!({}), &punti, None),
    ];
    for predicato in [
        "geo.predicate_intersects",
        "geo.predicate_disjoint",
        "geo.predicate_contains",
        "geo.predicate_within",
        "geo.predicate_equals_topo",
        "geo.predicate_covers",
        "geo.predicate_covered_by",
        "geo.predicate_contains_properly",
        "geo.predicate_touches",
        "geo.predicate_crosses",
        "geo.predicate_overlaps",
    ] {
        casi.push((
            predicato,
            json!({"other_wkb": altro.clone()}),
            &poligoni,
            None,
        ));
    }
    let mut provate = BTreeSet::new();
    for (op, config, ingresso, crs) in &casi {
        verifica_righe_conservate(op, config, std::slice::from_ref(*ingresso), *crs);
        provate.insert(*op);
    }
    // Binarie 1:1: la riga nascosta sta a sinistra. La maschera di `clip` ha
    // una riga sola, meno della sinistra (il vincolo `LeftRelative` la
    // accetta); le booleane allineate hanno lati con le stesse righe.
    let maschera = tabella(
        UTM,
        &[Some(Geometry::Polygon(quadrato(
            X0 - 10.0,
            Y0 - 10.0,
            120.0,
        )))],
    );
    let allineata = tabella(
        UTM,
        &[
            Some(Geometry::Polygon(quadrato(X0 + 20.0, Y0 + 20.0, 60.0))),
            Some(Geometry::Polygon(quadrato(X0, Y0, 10.0))),
            None,
            None,
            Some(Geometry::Polygon(quadrato(X0 + 510.0, Y0, 30.0))),
        ],
    );
    let binarie: Vec<(&str, &RecordBatch, &RecordBatch)> = vec![
        ("geo.clip", &poligoni, &maschera),
        ("geo.difference", &poligoni, &allineata),
        ("geo.intersection", &poligoni, &allineata),
        ("geo.symmetric_difference", &poligoni, &allineata),
        ("geo.union", &poligoni, &allineata),
        ("geo.count_points_in_polygons", &poligoni, &punti),
        ("geo.within", &punti, &poligoni),
    ];
    for (op, sinistra, destra) in binarie {
        verifica_righe_conservate(op, &json!({}), &[sinistra.clone(), destra.clone()], None);
        provate.insert(op);
    }
    let attese: BTreeSet<&str> = CATALOG
        .iter()
        .filter(|op| {
            op.family == Family::Geo && op.source_row_provenance() == SourceRowProvenance::Preserved
        })
        .map(|op| op.id)
        .collect();
    assert_eq!(provate, attese, "ogni geo Preserved ha un caso");
}

#[test]
fn aggregate_che_fonde_piu_righe_poi_formula_conta_senza_esempi() {
    // Sei righe in tre gruppi: `b` fonde due righe, una con divisore zero.
    // Nessuna riga d'origine e' "la" riga rifiutata: il payload ha il solo
    // conteggio, non un indice inventato.
    let righe = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("g", DataType::Utf8, false),
            Field::new("v", DataType::Float64, false),
            Field::new("d", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["c", "b", "a", "b", "a", "c"])),
            Arc::new(Float64Array::from(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
            Arc::new(Float64Array::from(vec![1.0, 2.0, 1.0, 0.0, 3.0, 2.0])),
        ],
    )
    .expect("righe");
    let aggrega = passo(
        "gruppi",
        "table.aggregate",
        &["t"],
        json!({"group_by": ["g"], "aggregations": [
            {"column": "d", "function": "min", "alias": "dmin"},
            {"column": "v", "function": "sum", "alias": "vsum"},
            {"column": "v", "function": "count", "alias": "n"}]}),
    );
    let tavole = [("t", righe)];
    let prefisso =
        esegui(&piano(&["t"], vec![aggrega.clone()], &["gruppi"]), &tavole).expect("prefisso");
    let gruppi = output(&prefisso, "gruppi");
    assert_eq!(gruppi.num_rows(), 3, "sei righe in tre gruppi");
    let chiavi = gruppi
        .column_by_name("g")
        .expect("g")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("g Utf8");
    let posizione = (0..chiavi.len())
        .find(|riga| chiavi.value(*riga) == "b")
        .expect("gruppo b");
    let conteggi = gruppi.column_by_name("n").expect("n");
    let conteggio = conteggi
        .as_any()
        .downcast_ref::<Int64Array>()
        .map(|interi| interi.value(posizione).to_string())
        .or_else(|| {
            conteggi
                .as_any()
                .downcast_ref::<UInt64Array>()
                .map(|interi| interi.value(posizione).to_string())
        })
        .expect("conteggio intero");
    assert_eq!(conteggio, "2", "il gruppo b fonde due righe");

    let pipeline = piano(
        &["t"],
        vec![
            aggrega,
            passo(
                "x",
                "table.formula",
                &["gruppi"],
                json!({"new_column": "rapporto", "formula": "vsum / dmin",
                        "on_division_by_zero": "error"}),
            ),
        ],
        &["x"],
    );
    let validata = valida(&pipeline, &tavole).expect("aggregate -> formula accettata");
    assert_eq!(
        validata.base_indici("x"),
        Some(BaseIndici::SenzaAttribuzione)
    );
    let errore = esegui(&pipeline, &tavole).expect_err("divisore zero nel gruppo b");
    let report = verifica_senza_attribuzione(&errore, "gruppi");
    assert_eq!(report.observed_total, 1);
}
