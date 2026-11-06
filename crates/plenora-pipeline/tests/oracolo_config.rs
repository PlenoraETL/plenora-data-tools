//! Oracolo sul catalogo tabellare: una config che l'analisi dei kernel
//! accetta non fallisce in esecuzione per la config.
//!
//! Per ogni operazione si parte dalla config rappresentativa di `CASI` e se
//! ne generano varianti: ogni stringa della config che nomina una colonna
//! viene sostituita, una alla volta, con ogni colonna delle fixture (cosi'
//! ogni regola di tipo si prova su ogni famiglia di colonne), piu' varianti
//! scritte per le regole di confine (operatori, target, arieta', verso,
//! gruppi con nome, `amount`). Ogni variante che `validate` accetta si
//! esegue su tabelle di poche righe.
//!
//! In esecuzione non sono ammessi errori di config (`InvalidPlan`,
//! `Unsupported`, `Internal`): sarebbero una regola che manca all'analisi.
//! Categoria e fase attese vengono dal modello di config e fixture in
//! `comune/config_attesi.rs`, prima di chiamare validazione ed esecuzione.

mod comune;
#[path = "comune/config_attesi.rs"]
mod config_attesi;
#[path = "comune/config_successi.rs"]
mod config_successi;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::{ErrorCategory, ErrorPhase};
#[macro_use]
#[path = "../../../tests/comune/esito.rs"]
mod esito;
use plenora_pipeline::{Passo, Pipeline};
use serde_json::{json, Value};

use comune::{chiamata_diretta, nomi_input, tabelle, Fixture, CASI, CHIAVE_HMAC};

/// Nomi di colonna di tutte le fixture.
fn nomi_di_colonna() -> Vec<String> {
    let mut nomi: Vec<String> = [
        Fixture::Wide,
        Fixture::Nullable,
        Fixture::Nested,
        Fixture::Binary,
    ]
    .into_iter()
    .flat_map(tabelle)
    .flat_map(|tabella| {
        tabella
            .schema()
            .fields()
            .iter()
            .map(|campo| campo.name().clone())
            .collect::<Vec<_>>()
    })
    .collect();
    nomi.sort();
    nomi.dedup();
    nomi
}

/// Le varianti di `config` con una stringa che nomina una colonna sostituita
/// da ciascuna di `colonne`.
fn sostituzioni(config: &Value, nomi: &[String], colonne: &[String]) -> Vec<Value> {
    fn percorsi(valore: &Value, corrente: &mut Vec<Value>, uscita: &mut Vec<Vec<Value>>) {
        match valore {
            Value::String(_) => uscita.push(corrente.clone()),
            Value::Array(elementi) => {
                for (indice, elemento) in elementi.iter().enumerate() {
                    corrente.push(json!(indice));
                    percorsi(elemento, corrente, uscita);
                    corrente.pop();
                }
            }
            Value::Object(campi) => {
                for (chiave, elemento) in campi {
                    corrente.push(json!(chiave));
                    percorsi(elemento, corrente, uscita);
                    corrente.pop();
                }
            }
            _ => {}
        }
    }
    fn punta<'a>(valore: &'a mut Value, percorso: &[Value]) -> Option<&'a mut Value> {
        percorso
            .iter()
            .try_fold(valore, |corrente, passo| match passo {
                Value::Number(indice) => corrente.get_mut(usize::try_from(indice.as_u64()?).ok()?),
                Value::String(chiave) => corrente.get_mut(chiave.as_str()),
                _ => None,
            })
    }
    let mut tutti = Vec::new();
    percorsi(config, &mut Vec::new(), &mut tutti);
    let mut varianti = Vec::new();
    for percorso in tutti {
        let mut copia = config.clone();
        let Some(Value::String(testo)) = punta(&mut copia, &percorso) else {
            continue;
        };
        if !nomi.contains(testo) {
            continue;
        }
        for colonna in colonne {
            let mut sostituita = config.clone();
            if let Some(valore) = punta(&mut sostituita, &percorso) {
                *valore = Value::String(colonna.clone());
            }
            varianti.push(sostituita);
        }
    }
    varianti
}

/// Varianti scritte per le regole di confine, per operazione.
#[allow(clippy::too_many_lines)] // Un elenco per regola.
fn varianti_di_confine(op: &str) -> Vec<Value> {
    let operatori = [
        "==",
        "!=",
        ">",
        ">=",
        "<",
        "<=",
        "contains",
        "startswith",
        "endswith",
        "isnull",
        "notnull",
        "between",
    ];
    let valori = [json!(1), json!("a"), json!("0,100"), Value::Null];
    let funzione = |nome: &str, args: Vec<Value>| {
        json!({"output_column": "e",
               "expression": {"kind": "function", "name": nome, "args": args}})
    };
    let col = |nome: &str| json!({"kind": "column", "name": nome});
    let lit = |valore: Value| json!({"kind": "literal", "value": valore});
    match op {
        "table.filter" => operatori
            .iter()
            .flat_map(|operatore| {
                valori.iter().map(
                    move |valore| json!({"column": "id", "operator": operatore, "value": valore}),
                )
            })
            .collect(),
        "table.conditional" => operatori
            .iter()
            .flat_map(|operatore| {
                valori.iter().map(move |valore| {
                    json!({"column": "id", "conditions": [{"operator": operatore,
                           "value": valore, "result": 1}], "default_value": 0})
                })
            })
            .collect(),
        "table.type_cast" => [
            "str",
            "int",
            "float",
            "bool",
            "date",
            "datetime",
            "date32",
            "timestamp_millis",
            "binary_utf8",
            "uint64",
            "dictionary_utf8",
        ]
        .iter()
        .flat_map(|target| {
            [
                json!({"column": "id", "target_type": target}),
                json!({"column": "date", "target_type": target, "date_format": "%Y-%m-%d"}),
            ]
        })
        .chain([
            json!({"column": "id", "target_type": "decimal128", "precision": 10, "scale": 2}),
            json!({"column": "date", "target_type": "timestamp_millis", "timezone": "UTC"}),
        ])
        .collect(),
        "table.dedup_advanced" => vec![
            json!({"subset": ["name"], "order_column": "value", "ascending": false}),
            json!({"subset": ["name"], "ascending": false}),
            json!({"subset": ["name"], "ascending": true}),
        ],
        "table.add_row_number" => vec![
            json!({"ascending": false}),
            json!({"partition_column": "name"}),
        ],
        "table.string_extract" => vec![
            json!({"column": "name", "pattern": "(?P<l>[ab])", "output_column": "x"}),
            json!({"column": "name", "pattern": "(?P<l>[ab])", "extract_all": true}),
            json!({"column": "name", "pattern": "([ab])", "output_column": "x", "extract_all": true}),
            json!({"column": "name", "pattern": ""}),
        ],
        "table.date_add" => [i64::MAX, i64::MIN, 1, -1, 0]
            .iter()
            .flat_map(|amount| {
                [
                    "years", "months", "weeks", "days", "hours", "minutes", "seconds",
                ]
                .iter()
                .map(move |unit| {
                    json!({"column": "date", "input_format": "%Y-%m-%d", "amount": amount,
                               "unit": unit, "output_column": "d"})
                })
            })
            .collect(),
        "table.expression" => vec![
            funzione("lower", vec![col("name")]),
            funzione("lower", vec![col("name"), col("name")]),
            funzione("lower", vec![]),
            funzione("coalesce", vec![]),
            funzione("concat", vec![col("name"), col("name")]),
            funzione("power", vec![col("value")]),
            funzione("between", vec![col("value"), lit(json!(0))]),
            funzione("null_if", vec![col("value"), lit(json!(0))]),
            funzione("substring", vec![col("name"), lit(json!(-1))]),
            funzione(
                "substring",
                vec![col("name"), lit(json!(0)), lit(json!(-1))],
            ),
            funzione("substring", vec![col("name"), lit(json!(-0.0))]),
            funzione(
                "substring",
                vec![col("name"), lit(json!(0)), lit(json!(1)), lit(json!(1))],
            ),
            funzione(
                "regex_replace",
                vec![col("name"), lit(json!("(")), lit(json!("x"))],
            ),
            funzione(
                "regex_replace",
                vec![col("name"), lit(json!("(a)")), lit(json!("$1"))],
            ),
            funzione(
                "regex_replace",
                vec![col("name"), lit(Value::Null), lit(json!("x"))],
            ),
            funzione(
                "regex_replace",
                vec![lit(Value::Null), lit(json!("(")), lit(json!("x"))],
            ),
            funzione(
                "regex_replace",
                vec![col("name"), lit(json!("(")), lit(Value::Null)],
            ),
            funzione("substring", vec![lit(Value::Null), lit(json!(-1))]),
            funzione(
                "substring",
                vec![col("name"), lit(Value::Null), lit(json!(-1))],
            ),
        ],
        "table.assert_range" => vec![
            json!({"column": "value"}),
            json!({"column": "value", "max": 1000, "inclusive_min": false}),
            json!({"column": "value", "min": 0, "max": 1000, "inclusive_min": false}),
        ],
        "table.aggregate" => [
            "sum", "mean", "min", "max", "count", "nunique", "first", "last", "concat", "variance",
            "stddev",
        ]
        .iter()
        .flat_map(|funzione| {
            [
                json!({"column": "value", "function": funzione}),
                json!({"column": "value", "function": funzione, "separator": "|"}),
                json!({"column": "value", "function": funzione, "distinct": true}),
                json!({"column": "value", "function": funzione, "skip_null": false}),
                json!({"column": "value", "function": funzione, "ddof": 0}),
            ]
        })
        .map(|aggregazione| json!({"group_by": ["name"], "aggregations": [aggregazione]}))
        .chain([
            json!({"group_by": ["name"],
                       "aggregations": [{"column": "value", "function": "quantile",
                                         "quantile": 0.5}]}),
            json!({"group_by": ["name"],
                       "aggregations": [{"column": "value", "function": "sum",
                                         "quantile": 0.5}]}),
        ])
        .collect(),
        "table.distinct" | "table.stable_fingerprint" => vec![json!({})],
        "table.sha256_hash" => vec![json!({"columns": [], "output_column": "h"})],
        "table.explode" => vec![json!({"column": "lst", "empty_policy": "drop"})],
        "table.window_function" => [
            "rank",
            "dense_rank",
            "cumsum",
            "cumcount",
            "lag",
            "lead",
            "pct_change",
            "running_mean",
            "percent_rank",
            "cume_dist",
        ]
        .iter()
        .flat_map(|funzione| {
            [
                json!({"column": "value", "function": funzione}),
                json!({"column": "value", "function": funzione, "offset": 2}),
                json!({"column": "value", "function": funzione, "offset": 0}),
            ]
        })
        .collect(),
        "table.rolling_window" => ["sum", "mean", "min", "max", "stddev"]
            .iter()
            .flat_map(|funzione| {
                [
                    json!({"column": "value", "function": funzione, "window": 2,
                           "output_column": "r"}),
                    json!({"column": "value", "function": funzione, "window": 2, "ddof": 0,
                           "output_column": "r"}),
                ]
            })
            .collect(),
        "table.mask_data" => ["email", "phone", "cf", "iban", "custom"]
            .iter()
            .flat_map(|tipo| {
                [
                    json!({"maskings": [{"column": "name", "mask_type": tipo}]}),
                    json!({"maskings": [{"column": "name", "mask_type": tipo, "chars_start": 1}]}),
                    json!({"maskings": [{"column": "name", "mask_type": tipo, "chars_end": 1}]}),
                    json!({"maskings": [{"column": "name", "mask_type": tipo, "mask_char": "#"}]}),
                ]
            })
            .collect(),
        "table.fill_na" => ["value", "ffill", "bfill"]
            .iter()
            .flat_map(|metodo| {
                [
                    json!({"column": "name", "method": metodo}),
                    json!({"column": "name", "method": metodo, "value": "x"}),
                    json!({"column": "name", "method": metodo, "value": null}),
                ]
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn piano(op: &str, ingressi: &[&str], config: Value) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: ingressi.iter().map(|nome| (*nome).to_owned()).collect(),
        crs: None,
        limits: None,
        steps: vec![Passo {
            out: "uscita".to_owned(),
            op: op.to_owned(),
            inputs: ingressi.iter().map(|nome| (*nome).to_owned()).collect(),
            config,
        }],
        outputs: vec!["uscita".to_owned()],
    }
}

#[test]
fn le_config_accettate_dall_analisi_non_falliscono_per_la_config() {
    std::env::set_var(CHIAVE_HMAC, "chiave-di-test-del-runner");
    let nomi = nomi_di_colonna();
    let mut accettate = 0_usize;
    let mut rifiutate = 0_usize;
    for caso in CASI {
        let base: Value = serde_json::from_str(caso.config).expect("config del caso");
        let fixture_da_provare: Vec<Fixture> = match caso.fixture {
            Fixture::Binary | Fixture::Set => vec![caso.fixture],
            _ => vec![Fixture::Wide, Fixture::Nullable, Fixture::Nested],
        };
        for fixture in fixture_da_provare {
            let tavole = tabelle(fixture);
            let colonne: Vec<String> = tavole
                .iter()
                .flat_map(|tabella| {
                    tabella
                        .schema()
                        .fields()
                        .iter()
                        .map(|campo| campo.name().clone())
                        .collect::<Vec<_>>()
                })
                .collect();
            let mut configs = vec![base.clone()];
            configs.extend(sostituzioni(&base, &nomi, &colonne));
            for confine in varianti_di_confine(caso.op) {
                configs.extend(sostituzioni(&confine, &nomi, &colonne));
                configs.push(confine);
            }
            let ingressi = nomi_input(tavole.len());
            let riferimenti: Vec<&str> = ingressi.iter().map(String::as_str).collect();
            let schemi: Vec<(&str, SchemaRef)> = riferimenti
                .iter()
                .copied()
                .zip(tavole.iter().map(RecordBatch::schema))
                .collect();
            for config in configs {
                let atteso = config_attesi::validazione(caso.op, &config, &tavole);
                let runtime_atteso = if atteso.is_none() {
                    config_attesi::esecuzione(caso.op, &config, &tavole)
                } else {
                    None
                };
                let validazione = piano(caso.op, &riferimenti, config.clone()).validate(&schemi);
                if let Some(categoria) = atteso {
                    assert_rifiuto!(validazione, categoria, ErrorPhase::Validate);
                    rifiutate += 1;
                    continue;
                }
                let validata = validazione.expect("config ammessa dal modello indipendente");
                accettate += 1;
                let tabelle_run: Vec<(String, RecordBatch)> = ingressi
                    .iter()
                    .cloned()
                    .zip(tavole.iter().cloned())
                    .collect();
                let runtime = validata.run(tabelle_run);
                if let Some(categoria) = runtime_atteso {
                    let fase = match categoria {
                        ErrorCategory::DataMapping => ErrorPhase::Write,
                        ErrorCategory::Schema => ErrorPhase::Validate,
                        _ => panic!("categoria fuori dal modello di esecuzione"),
                    };
                    assert_rifiuto!(runtime, categoria, fase);
                } else {
                    let output = runtime.expect("scenario di successo indipendente");
                    let atteso = config_successi::atteso(caso.op, &config, &tavole);
                    config_successi::verifica(caso.op, &config, &output.outputs[0].1, &atteso);
                    let diretto = chiamata_diretta(caso.op, &config, &tavole)
                        .expect("stesso scenario ammesso nell'API diretta");
                    assert_eq!(output.outputs.len(), 1);
                    assert_eq!(output.outputs[0].0, "uscita");
                    assert_eq!(output.outputs[0].1.num_rows(), diretto.num_rows());
                    if caso.op != "table.uuid_generator" {
                        assert_eq!(output.outputs[0].1.columns(), diretto.columns());
                    }
                }
            }
        }
    }
    assert!(
        accettate > 1_000 && rifiutate > 1_000,
        "l'oracolo deve provare molte config: {accettate} accettate, {rifiutate} rifiutate"
    );
}
