//! Ingressi vuoti: nessuna operazione tabellare fallisce per il fattore di
//! espansione quando le sue tabelle d'ingresso non hanno righe.
//!
//! Il fattore di espansione misura quante righe l'operazione genera dai dati;
//! con zero righe d'ingresso un'uscita non vuota (le cinque metriche di
//! `reconcile`, una riga per regola di `validate_rules` in `summary`) non e'
//! un'espansione dei dati, e rifiutarla come «espansione infinita» sarebbe un
//! falso `ResourceLimit`. Per ogni operazione si esegue la config
//! rappresentativa di `CASI` sulle fixture svuotate (stesso schema, zero
//! righe), e anche con un solo lato vuoto per le binarie.

#[path = "comune/attesi.rs"]
mod attesi;
mod comune;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::PlenoraError;
use plenora_pipeline::{Passo, Pipeline};
use serde_json::Value;

use comune::{nomi_input, tabelle, CASI, CHIAVE_HMAC};

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

fn esegui(op: &str, config: &Value, tavole: &[RecordBatch]) -> Result<RecordBatch, PlenoraError> {
    let ingressi = nomi_input(tavole.len());
    let riferimenti: Vec<&str> = ingressi.iter().map(String::as_str).collect();
    let schemi: Vec<(&str, SchemaRef)> = riferimenti
        .iter()
        .copied()
        .zip(tavole.iter().map(RecordBatch::schema))
        .collect();
    let validata = piano(op, &riferimenti, config.clone()).validate(&schemi)?;
    validata
        .run(ingressi.into_iter().zip(tavole.iter().cloned()).collect())
        .map(|mut esito| esito.outputs.remove(0).1)
}

fn e_espansione(errore: &PlenoraError) -> bool {
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit
    );
    assert_eq!(errore.phase(), plenora_core::ErrorPhase::Write);
    errore.to_string().contains("max_expansion_factor")
}

#[test]
fn nessuna_operazione_rifiuta_ingressi_vuoti_per_il_fattore_di_espansione() {
    std::env::set_var(CHIAVE_HMAC, "chiave-di-test-del-runner");

    let mut provate = 0_usize;
    for caso in CASI {
        let config: Value = serde_json::from_str(caso.config).expect("config del caso");
        let piene = tabelle(caso.fixture);
        let mut combinazioni: Vec<Vec<RecordBatch>> =
            vec![piene.iter().map(|tabella| tabella.slice(0, 0)).collect()];
        if piene.len() == 2 {
            combinazioni.push(vec![piene[0].slice(0, 0), piene[1].clone()]);
            combinazioni.push(vec![piene[0].clone(), piene[1].slice(0, 0)]);
        }
        for tavole in combinazioni {
            provate += 1;
            let risultato = esegui(caso.op, &config, &tavole);
            let categoria = match caso.op {
                "table.assert_cardinality" => Some(plenora_core::ErrorCategory::InvalidPlan),
                "table.bin" => Some(plenora_core::ErrorCategory::Schema),
                "table.transpose" => Some(plenora_core::ErrorCategory::Unsupported),
                "table.assert_foreign_key" if tavole[0].num_rows() > 0 => {
                    Some(plenora_core::ErrorCategory::DataMapping)
                }
                _ => None,
            };
            if let Some(categoria) = categoria {
                let errore = risultato.expect_err("vincolo non soddisfatto dalla fixture vuota");
                assert_eq!(errore.category(), categoria, "{}", caso.op);
                assert_eq!(
                    errore.phase(),
                    if matches!(
                        caso.op,
                        "table.assert_cardinality" | "table.bin" | "table.transpose"
                    ) {
                        plenora_core::ErrorPhase::Validate
                    } else {
                        plenora_core::ErrorPhase::Write
                    },
                    "{}",
                    caso.op
                );
            } else {
                let ottenuto = risultato.unwrap_or_else(|errore| panic!("{}: {errore}", caso.op));
                let atteso = atteso_vuoto(caso.op, &tavole, &piene);
                attesi::verifica(caso.op, &ottenuto, &atteso);
            }
        }
    }
    assert!(provate > CASI.len(), "casi provati: {provate}");
}

/// Schema dalla fixture indipendente; valori determinati dai lati vuoti.
fn atteso_vuoto(op: &str, tavole: &[RecordBatch], piene: &[RecordBatch]) -> RecordBatch {
    use plenora_core::arrow::array::{new_null_array, ArrayRef, StringArray, UInt64Array};
    use std::sync::Arc;
    let schema = attesi::tabella(op, piene).schema();
    let sinistra = &tavole[0];
    let nr = tavole.get(1).map_or(0, RecordBatch::num_rows);
    let nl = sinistra.num_rows();
    let colonne: Vec<ArrayRef> = match op {
        "table.concat" | "table.concat_by_name" | "table.union_distinct" => {
            if nl > 0 {
                sinistra.columns().to_vec()
            } else {
                tavole[1].columns().to_vec()
            }
        }
        "table.anti_join" | "table.except" => sinistra.columns().to_vec(),
        "table.asof_join" if nl > 0 => {
            let mut colonne = sinistra.columns().to_vec();
            for field in schema.fields().iter().skip(colonne.len()) {
                colonne.push(new_null_array(field.data_type(), nl));
            }
            colonne
        }
        "table.reconcile" => vec![
            Arc::new(StringArray::from(vec![
                "matched_rows",
                "left_only_rows",
                "right_only_rows",
                "left_duplicate_rows",
                "right_duplicate_rows",
            ])),
            Arc::new(UInt64Array::from(vec![
                0,
                u64::try_from(nl).expect("righe"),
                u64::try_from(nr).expect("righe"),
                0,
                0,
            ])),
        ],
        "table.table_diff" if nl + nr > 0 => vec![
            if nl > 0 {
                sinistra.column(0).clone()
            } else {
                tavole[1].column(0).clone()
            },
            Arc::new(StringArray::from(vec![
                if nl > 0 {
                    "DELETED"
                } else {
                    "ADDED"
                };
                nl + nr
            ])),
            Arc::new(StringArray::from(vec![None::<&str>; nl + nr])),
            Arc::new(StringArray::from(vec![None::<&str>; nl + nr])),
        ],
        _ => return RecordBatch::new_empty(schema),
    };
    RecordBatch::try_new(schema, colonne).expect("atteso vuoto")
}

fn tabella_int(nome: &str, valori: Vec<i64>) -> RecordBatch {
    use plenora_core::arrow::array::Int64Array;
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    RecordBatch::try_new(
        std::sync::Arc::new(Schema::new(vec![Field::new(nome, DataType::Int64, false)])),
        vec![std::sync::Arc::new(Int64Array::from(valori))],
    )
    .expect("tabella")
}

/// `validate_rules` in `summary` rende una riga per regola: su un ingresso
/// vuoto, e su un ingresso di una riga con piu' regole del fattore di
/// espansione, non e' un'espansione dei dati.
#[test]
fn validate_rules_summary_non_e_un_espansione() {
    let regole = |numero: usize| -> Value {
        let regole: Vec<Value> = (0..numero)
            .map(|indice| {
                serde_json::json!({"name": format!("r{indice}"), "operator": "ge",
                                   "column": "id", "value": 0})
            })
            .collect();
        serde_json::json!({"rules": regole, "output_mode": "summary"})
    };
    let vuota = tabella_int("id", Vec::new());
    esegui("table.validate_rules", &regole(1), &[vuota]).expect("summary su ingresso vuoto");
    let una = tabella_int("id", vec![1]);
    esegui("table.validate_rules", &regole(150), &[una]).expect("150 regole su una riga");
}

/// `melt` rende righe per colonne valore: 150 colonne su una riga sono
/// un'espansione fissata dalla config, non dai dati.
#[test]
fn melt_con_molte_colonne_valore_non_e_un_espansione() {
    use plenora_core::arrow::array::{ArrayRef, Int64Array};
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    let campi: Vec<Field> = (0..150)
        .map(|indice| Field::new(format!("c{indice}"), DataType::Int64, false))
        .collect();
    let colonne: Vec<ArrayRef> = (0..150)
        .map(|indice| std::sync::Arc::new(Int64Array::from(vec![indice])) as ArrayRef)
        .collect();
    let tabella =
        RecordBatch::try_new(std::sync::Arc::new(Schema::new(campi)), colonne).expect("larga");
    esegui(
        "table.melt",
        &serde_json::json!({"id_columns": []}),
        &[tabella],
    )
    .expect("150 colonne valore");
}

/// Un join in cui la chiave e' unica su un lato non espande: con il vincolo
/// `MaxRelative` un left join di 10 600 righe su una dimensione di 100
/// righe valeva 106 volte il lato destro e superava il default 100.
#[test]
fn un_join_con_chiave_unica_su_un_lato_non_supera_il_fattore() {
    let fatti = tabella_int("k", (0..10_600).map(|riga| riga % 100).collect());
    let dimensione = tabella_int("d", (0..100).collect());
    for how in ["inner", "left"] {
        let config = serde_json::json!({"left_keys": ["k"], "right_keys": ["d"], "how": how});
        let uscita = esegui("table.join", &config, &[fatti.clone(), dimensione.clone()])
            .unwrap_or_else(|errore| panic!("{how}: {errore}"));
        verifica_join_unico(&uscita, "k", (0..10_600).map(|riga| riga % 100).collect());
        // Anche a lati invertiti (1:N): l'uscita e' il lato maggiore.
        let config = serde_json::json!({"left_keys": ["d"], "right_keys": ["k"], "how": how});
        let uscita = esegui("table.join", &config, &[dimensione.clone(), fatti.clone()])
            .unwrap_or_else(|errore| panic!("{how} invertito: {errore}"));
        verifica_join_unico(
            &uscita,
            "d",
            (0..100)
                .flat_map(|id| std::iter::repeat_n(id, 106))
                .collect(),
        );
    }
    // Un molti-a-molti che moltiplica le righe resta un rifiuto: 300 righe
    // con la stessa chiave per lato danno 90 000 righe, 150 volte la somma.
    let molti = tabella_int("k", vec![7; 300]);
    let altri = tabella_int("d", vec![7; 300]);
    let config = serde_json::json!({"left_keys": ["k"], "right_keys": ["d"], "how": "inner"});
    let errore = esegui("table.join", &config, &[molti, altri]).expect_err("molti-a-molti");
    assert!(e_espansione(&errore), "{errore}");
}

fn verifica_join_unico(uscita: &RecordBatch, nome: &str, valori: Vec<i64>) {
    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use std::sync::Arc;
    let atteso = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(nome, DataType::Int64, true)])),
        vec![Arc::new(plenora_core::arrow::array::Int64Array::from(
            valori,
        ))],
    )
    .expect("join atteso");
    attesi::verifica("join_unico", uscita, &atteso);
}
