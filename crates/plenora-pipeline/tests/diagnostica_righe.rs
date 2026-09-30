//! Diagnostica per riga dopo passi che cambiano numero o ordine delle righe.
//!
//! Ogni ordine valido della catena si accetta. Gli indici della diagnostica
//! sono righe della sorgente quando la catena a monte le conserva, righe del
//! primo ingresso del passo altrimenti: la base si decide in validazione
//! (`PipelineValidata::base_indici`) e l'esecuzione la scrive nel payload.
//! L'oracolo delle posizioni è il piano spezzato: il prefisso della catena
//! eseguito da solo dice in quale riga del suo output sta la cella che
//! fallisce.

mod comune;

use std::sync::Arc;

use plenora_core::arrow::array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::catalog::{find_operation, SourceRowProvenance};
use plenora_core::diagnostics::{
    RowDiagnostics, ROW_DIAGNOSTICS_INDEX_BASIS, ROW_DIAGNOSTICS_INDEX_BASIS_STEP_INPUT,
};
use plenora_core::{PlenoraError, Result};
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
            json!({"new_column": "rapporto", "formula": "v / d"}),
        ),
        (
            "table.expression",
            json!({"output_column": "rapporto", "expression": {
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
    errore
        .row_diagnostics()
        .unwrap_or_else(|| panic!("diagnostica per riga attesa: {errore}"))
}

/// Posizione, nell'output del prefisso eseguito da solo, della riga che
/// viene dalla riga cattiva della sorgente.
fn posizione_nel_prefisso(forma: &Forma) -> u64 {
    let ingressi: Vec<&str> = if forma.usa_destra {
        vec!["t", "r"]
    } else {
        vec!["t"]
    };
    let prefisso = piano(&ingressi, forma.prefisso.clone(), &[forma.ingresso]);
    let esito = esegui(&prefisso, &tabelle(true, forma.usa_destra)).expect("prefisso");
    let uscita = output(&esito, forma.ingresso);
    let id = uscita
        .column_by_name("id")
        .expect("colonna id")
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("id Int64");
    let posizioni: Vec<usize> = (0..id.len())
        .filter(|riga| id.value(*riga) == RIGA_CATTIVA)
        .collect();
    assert_eq!(posizioni.len(), 1, "{}: una riga cattiva", forma.nome);
    u64::try_from(posizioni[0]).expect("posizione")
}

#[test]
fn dopo_un_passo_che_cambia_le_righe_la_diagnostica_punta_alla_riga_dell_ingresso() {
    for forma in forme() {
        let atteso = posizione_nel_prefisso(&forma);
        let ingressi: Vec<&str> = if forma.usa_destra {
            vec!["t", "r"]
        } else {
            vec!["t"]
        };
        for (op, config) in emettitori() {
            let mut passi = forma.prefisso.clone();
            passi.push(passo("x", op, &[forma.ingresso], config));
            let pipeline = piano(&ingressi, passi, &["x"]);
            let tavole = tabelle(true, forma.usa_destra);
            let validata = valida(&pipeline, &tavole)
                .unwrap_or_else(|errore| panic!("{} -> {op}: rifiutata: {errore}", forma.nome));
            assert_eq!(
                validata.base_indici("x"),
                Some(BaseIndici::IngressoDelPasso),
                "{} -> {op}",
                forma.nome
            );
            let errore = validata
                .run(
                    tavole
                        .iter()
                        .map(|(nome, tabella)| ((*nome).to_owned(), tabella.clone()))
                        .collect(),
                )
                .expect_err("cella cattiva");
            let report = diagnostica(&errore);
            // Validazione ed esecuzione concordano sulla base.
            assert_eq!(
                report.index_basis, ROW_DIAGNOSTICS_INDEX_BASIS_STEP_INPUT,
                "{} -> {op}",
                forma.nome
            );
            assert_eq!(report.observed_total, 1, "{} -> {op}", forma.nome);
            assert_eq!(report.examples.len(), 1, "{} -> {op}", forma.nome);
            assert_eq!(
                report.examples[0].source_index, atteso,
                "{} -> {op}: riga dell'ingresso del passo",
                forma.nome
            );
            assert!(report.validate_for_emission().is_ok());
            let testo = errore.to_string();
            assert!(testo.contains("passo `x`"), "{testo}");
            assert!(
                testo.contains(&format!("ingresso `{}`", forma.ingresso)),
                "{testo}"
            );
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
                !errore.to_string().contains("non alla sorgente"),
                "{op}: {errore}"
            );
        }
    }
}

#[test]
fn aggregate_poi_formula_riferisce_le_righe_dei_gruppi() {
    // Le righe dell'aggregazione sono gruppi nuovi: l'indice è la riga
    // del gruppo nell'output di `aggregate`.
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
            aggrega.clone(),
            passo(
                "x",
                "table.formula",
                &["g"],
                json!({"new_column": "rapporto", "formula": "vmax / dmin"}),
            ),
        ],
        &["x"],
    );
    let tavole = tabelle(true, false);
    let validata = valida(&pipeline, &tavole).expect("aggregate -> formula accettata");
    assert_eq!(
        validata.base_indici("x"),
        Some(BaseIndici::IngressoDelPasso)
    );
    let errore = esegui(&pipeline, &tavole).expect_err("divisore zero nel gruppo k7");
    let report = diagnostica(&errore);
    assert_eq!(report.index_basis, ROW_DIAGNOSTICS_INDEX_BASIS_STEP_INPUT);

    let gruppi = esegui(&piano(&["t"], vec![aggrega], &["g"]), &tavole).expect("prefisso");
    let gruppi = output(&gruppi, "g");
    let chiavi = gruppi
        .column_by_name("k")
        .expect("k")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("k Utf8");
    let posizione = (0..chiavi.len())
        .find(|riga| chiavi.value(*riga) == "k7")
        .expect("gruppo k7");
    assert_eq!(
        report.examples[0].source_index,
        u64::try_from(posizione).expect("posizione")
    );
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

    // Con il lato left filtrato gli indici sono righe dell'ingresso left.
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
        Some(BaseIndici::IngressoDelPasso)
    );
    let errore = esegui(&pipeline, &tavole).expect_err("chiavi 4 e 5 assenti");
    let report = diagnostica(&errore);
    assert_eq!(report.index_basis, ROW_DIAGNOSTICS_INDEX_BASIS_STEP_INPUT);
    let indici: Vec<u64> = report
        .examples
        .iter()
        .map(|esempio| esempio.source_index)
        .collect();
    // tf tiene gli id 4..=9: 4 e 5 sono le sue righe 0 e 1.
    assert_eq!(indici, vec![0, 1]);
    assert!(errore.to_string().contains("ingresso `tf`"), "{errore}");
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
                json!({"new_column": "rapporto", "formula": "v / d"}),
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
fn from_wkt_dopo_un_filtro_riferisce_le_righe_dell_ingresso() {
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
    for (prefisso, ingresso, base, atteso) in [
        (Vec::new(), "t", BaseIndici::Sorgente, 7_u64),
        (
            vec![filtro("f", "t", ">", 3)],
            "f",
            BaseIndici::IngressoDelPasso,
            3,
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
        let report = diagnostica(&errore);
        assert_eq!(report.index_basis, base.index_basis());
        assert_eq!(report.examples[0].source_index, atteso, "{ingresso}");
        assert!(!errore.to_string().contains("non wkt"), "{errore}");
    }
}

/// La base `Sorgente` si fida della classificazione `Preserved` del
/// catalogo: un'operazione dichiarata cosi' che cambiasse numero o ordine
/// delle righe farebbe leggere come righe della sorgente indici che non lo
/// sono. Oracolo sulle config rappresentative: stesse righe del primo
/// ingresso e, dove `id` resta com'era, nello stesso ordine.
#[test]
fn le_operazioni_che_conservano_le_righe_le_conservano_davvero() {
    std::env::set_var(comune::CHIAVE_HMAC, "chiave-di-test-del-runner");
    let mut provate = 0;
    for caso in comune::CASI {
        let descrittore = find_operation(caso.op).expect("operazione del catalogo");
        if descrittore.source_row_provenance() != SourceRowProvenance::Preserved {
            continue;
        }
        let config: Value = serde_json::from_str(caso.config).expect("config del caso");
        let ingressi = comune::tabelle(caso.fixture);
        let uscita = comune::chiamata_diretta(caso.op, &config, &ingressi)
            .unwrap_or_else(|errore| panic!("{}: {errore}", caso.op));
        let primo = &ingressi[0];
        assert_eq!(uscita.num_rows(), primo.num_rows(), "{}", caso.op);
        if let (Some(prima), Some(dopo)) = (primo.column_by_name("id"), uscita.column_by_name("id"))
        {
            if prima.data_type() == dopo.data_type() {
                assert_eq!(prima.as_ref(), dopo.as_ref(), "{}: ordine", caso.op);
            }
        }
        provate += 1;
    }
    assert!(provate > 30, "operazioni provate: {provate}");
}
