//! Oracolo di `validate_rules`: la valutazione com'era prima delle colonne
//! risolte per regola e dei builder diretti (`rule_passes`,
//! `evaluate_rules`, `validate_rules`), copiata alla lettera sopra la stessa
//! compilazione delle regole, e i confronti degli esiti completi (batch per
//! byte e per bit, errori per categoria e messaggio).

use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, BinaryArray, Date32Array, Decimal128Array, DictionaryArray,
    TimestampMillisecondArray,
};
use proptest::prelude::*;
use serde_json::{json, Value};

use super::*;
use crate::test_support::{assert_same_outcome_bits, nullable_batch};

// Copia letterale del percorso precedente.
/// Valuta una regola su una riga.
///
/// MAI un errore sui dati: qualunque valore non interpretabile (incluso null
/// per gli operatori a valore) e' un fallimento della regola, non un errore
/// del kernel. Il `Result` copre solo invarianti interne violate (errore
/// Internal), mai i dati.
///
/// Confronti numerici tutti esatti nel dominio nativo del tipo (vedi
/// [`rule_compare`]); l'uguaglianza sulle colonne non numeriche resta
/// testuale, come da contratto della regola.
fn rule_passes_riferimento(batch: &RecordBatch, rule: &CompiledRule, row: usize) -> Result<bool> {
    let array = batch.column(rule.column_index).as_ref();
    match rule.operator {
        // Stessa nozione di null del filtro: due kernel che rispondono
        // diversamente sulla stessa riga sarebbero peggio di entrambi.
        RuleOperator::Isnull => return Ok(crate::is_logically_null(array, row)),
        RuleOperator::Notnull => return Ok(!crate::is_logically_null(array, row)),
        _ if crate::is_logically_null(array, row) => return Ok(false),
        _ => {}
    }
    Ok(match rule.operator {
        RuleOperator::Eq | RuleOperator::Ne => {
            let equal = if rule.numeric_column {
                rule_compare(array, row, rule.expected_bound).equality()
            } else {
                // Colonna non numerica: uguaglianza testuale. Una cella che
                // non si riesce a leggere e' `None` (non interpretabile), mai
                // "diversa dal valore atteso".
                match scalar_as_string(array, row) {
                    Ok(Some(actual)) => Some(actual == rule.expected),
                    Ok(None) | Err(_) => None,
                }
            };
            // `None` = cella non interpretabile: la regola fallisce sia con
            // `eq` sia con `ne`.
            equal.is_some_and(|equal| matches!(rule.operator, RuleOperator::Ne) != equal)
        }
        RuleOperator::Gt | RuleOperator::Ge | RuleOperator::Lt | RuleOperator::Le => rule_ordered(
            rule_compare(array, row, rule.expected_bound).ordering(),
            rule.operator,
        )
        .ok_or_else(|| PlenoraError::Internal("operatore di regola non ordinato".into()))?,
        RuleOperator::Range => within_rule_range(
            rule_compare(array, row, rule.expected_bound).ordering(),
            rule_compare(array, row, rule.expected_high_bound).ordering(),
        ),
        // La colonna di una regola regex e' garantita Utf8 da
        // `compile_rules` (percorso caldo): prestito diretto sulla `StringArray`,
        // senza l'allocazione per riga di `scalar_as_string`. Tipi diversi
        // (mai raggiunti per contratto) restano sul percorso scalare.
        RuleOperator::Regex => rule.regex.as_ref().is_some_and(|regex| {
            array.as_any().downcast_ref::<StringArray>().map_or_else(
                || {
                    scalar_as_string(array, row)
                        .ok()
                        .flatten()
                        .is_some_and(|actual| regex.is_match(&actual))
                },
                |values| !values.is_null(row) && regex.is_match(values.value(row)),
            )
        }),
        RuleOperator::Isnull | RuleOperator::Notnull => {
            return Err(PlenoraError::Internal(
                "isnull/notnull sono valutati prima del confronto scalare".into(),
            ));
        }
    })
}

/// Esito della valutazione delle regole: validita' per riga e indici delle
/// regole fallite per gravita' (errori, warning).
type RuleEvaluation = (Vec<bool>, Vec<Vec<usize>>, Vec<Vec<usize>>);

/// Valuta tutte le regole su tutte le righe; restituisce per riga gli
/// INDICI delle regole fallite per gravita' (nomi risolti dal chiamante).
fn evaluate_rules_riferimento(
    batch: &RecordBatch,
    rules: &[CompiledRule],
) -> Result<RuleEvaluation> {
    let mut valid = Vec::with_capacity(batch.num_rows());
    let mut errors: Vec<Vec<usize>> = Vec::with_capacity(batch.num_rows());
    let mut warnings: Vec<Vec<usize>> = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let mut row_errors = Vec::new();
        let mut row_warnings = Vec::new();
        for (index, rule) in rules.iter().enumerate() {
            if !rule_passes_riferimento(batch, rule, row)? {
                match rule.severity {
                    RuleSeverity::Error => row_errors.push(index),
                    RuleSeverity::Warning => row_warnings.push(index),
                }
            }
        }
        valid.push(row_errors.is_empty());
        errors.push(row_errors);
        warnings.push(row_warnings);
    }
    Ok((valid, errors, warnings))
}

/// Valida le righe contro un set di regole dichiarative.
///
/// NON fallisce mai sui dati: `annotate` aggiunge `_valid` (Boolean, false
/// se almeno una regola error e' fallita), `_errors` e `_warnings` (nomi
/// delle regole fallite separati da `;`, stringa vuota se nessuna);
/// `summary` emette una riga per regola (`name`, `errors`, `warnings` con i
/// conteggi delle righe fallite per gravita').
///
/// # Errors
///
/// - `InvalidPlan`: nessuna regola; nome regola vuoto o ripetuto; regola senza
///   `column`; `value` mancante o non ammesso per l'operatore; tipo della
///   colonna incompatibile con l'operatore; valore atteso non numerico o
///   range malformato; regex non valida; invarianti interne violate (errore
///   Internal);
/// - `Schema`: colonna di una regola assente; errore Arrow nella
///   costruzione dell'output.
fn validate_rules_riferimento(batch: &RecordBatch, config: &ValidateRules) -> Result<RecordBatch> {
    let rules = compile_rules(batch, config)?;
    let (valid, errors, warnings) = evaluate_rules_riferimento(batch, &rules)?;
    let join_names = |indices: &[usize]| {
        indices
            .iter()
            .map(|index| rules[*index].name.as_str())
            .collect::<Vec<_>>()
            .join(";")
    };
    match config.output_mode {
        ValidateOutputMode::Annotate => {
            let result = replace_or_append(
                batch,
                "_valid",
                DataType::Boolean,
                false,
                Arc::new(BooleanArray::from(valid)),
            )?;
            let result = replace_or_append(
                &result,
                "_errors",
                DataType::Utf8,
                false,
                Arc::new(StringArray::from(
                    errors
                        .iter()
                        .map(|indices| join_names(indices))
                        .collect::<Vec<_>>(),
                )),
            )?;
            replace_or_append(
                &result,
                "_warnings",
                DataType::Utf8,
                false,
                Arc::new(StringArray::from(
                    warnings
                        .iter()
                        .map(|indices| join_names(indices))
                        .collect::<Vec<_>>(),
                )),
            )
        }
        ValidateOutputMode::Summary => {
            let mut error_counts = vec![0_i64; rules.len()];
            let mut warning_counts = vec![0_i64; rules.len()];
            for row_errors in &errors {
                for index in row_errors {
                    error_counts[*index] += 1;
                }
            }
            for row_warnings in &warnings {
                for index in row_warnings {
                    warning_counts[*index] += 1;
                }
            }
            Ok(RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("name", DataType::Utf8, false),
                    Field::new("errors", DataType::Int64, false),
                    Field::new("warnings", DataType::Int64, false),
                ])),
                vec![
                    Arc::new(StringArray::from(
                        rules
                            .iter()
                            .map(|rule| rule.name.as_str())
                            .collect::<Vec<_>>(),
                    )),
                    Arc::new(Int64Array::from(error_counts)),
                    Arc::new(Int64Array::from(warning_counts)),
                ],
            )?)
        }
    }
}

/// Una colonna per tipo, con i valori difficili di ciascuno.
// Una colonna per tipo, elencata per esteso: spezzarla nasconderebbe i valori.
#[allow(clippy::too_many_lines)]
fn tabella() -> RecordBatch {
    let chiavi = vec![
        Some(0),
        Some(1),
        None,
        Some(2),
        Some(0),
        Some(1),
        Some(2),
        None,
    ];
    let dizionario = DictionaryArray::<Int32Type>::try_new(
        chiavi.into(),
        Arc::new(StringArray::from(vec![Some("x"), None, Some("10")])),
    )
    .expect("dictionary");
    nullable_batch(vec![
        (
            "i",
            Arc::new(Int64Array::from(vec![
                Some(0),
                Some(-1),
                None,
                Some(i64::MAX),
                Some(i64::MIN),
                Some(9_007_199_254_740_993),
                Some(10),
                Some(5),
            ])),
        ),
        (
            "f",
            Arc::new(Float64Array::from(vec![
                Some(0.0),
                Some(-0.0),
                Some(f64::NAN),
                None,
                Some(f64::INFINITY),
                Some(1.5),
                Some(f64::NEG_INFINITY),
                Some(9_007_199_254_740_992.0),
            ])),
        ),
        (
            "u",
            Arc::new(UInt64Array::from(vec![
                Some(0),
                Some(u64::MAX),
                None,
                Some(10),
                Some(1 << 63),
                Some(5),
                Some(1),
                Some(2),
            ])),
        ),
        (
            "s",
            Arc::new(StringArray::from(vec![
                Some("abc"),
                Some(""),
                None,
                Some("10"),
                Some("NaN"),
                Some("ABC"),
                Some("0"),
                Some("x;y"),
            ])),
        ),
        (
            "b",
            Arc::new(BooleanArray::from(vec![
                Some(true),
                Some(false),
                None,
                Some(true),
                Some(false),
                Some(true),
                None,
                Some(false),
            ])),
        ),
        (
            "d",
            Arc::new(Date32Array::from(vec![
                Some(0),
                Some(-1),
                None,
                Some(10),
                Some(19_000),
                Some(5),
                Some(1),
                Some(2),
            ])),
        ),
        (
            "t",
            Arc::new(
                TimestampMillisecondArray::from(vec![
                    Some(0),
                    Some(-1),
                    None,
                    Some(10),
                    Some(i64::MAX),
                    Some(5),
                    Some(1),
                    Some(2),
                ])
                .with_timezone("UTC"),
            ),
        ),
        (
            "m",
            Arc::new(
                Decimal128Array::from(vec![
                    Some(0),
                    Some(-150),
                    None,
                    Some(1000),
                    Some(105),
                    Some(i128::from(i64::MAX)),
                    Some(1),
                    Some(99),
                ])
                .with_precision_and_scale(38, 2)
                .expect("decimal"),
            ),
        ),
        (
            "y",
            Arc::new(BinaryArray::from(vec![
                Some(b"abc".as_slice()),
                Some(b"\xff\xfe".as_slice()),
                None,
                Some(b"10".as_slice()),
                Some(b"".as_slice()),
                Some(b"x".as_slice()),
                Some(b"0".as_slice()),
                Some(b"\xc3".as_slice()),
            ])),
        ),
        ("k", Arc::new(dizionario)),
    ])
}

const COLONNE: [&str; 11] = ["i", "f", "u", "s", "b", "d", "t", "m", "y", "k", "assente"];

const OPERATORI: [&str; 10] = [
    "eq", "ne", "gt", "ge", "lt", "le", "isnull", "notnull", "regex", "range",
];

fn valori() -> Vec<Option<Value>> {
    vec![
        None,
        Some(json!(0)),
        Some(json!(10)),
        Some(json!(-1)),
        Some(json!(1.5)),
        Some(json!(9_007_199_254_740_993_i64)),
        Some(json!(u64::MAX)),
        Some(json!("NaN")),
        Some(json!("abc")),
        Some(json!("x;y")),
        Some(json!("")),
        Some(json!("0,10")),
        Some(json!(" -1 , 1e400 ")),
        Some(json!("NaN,5")),
        Some(json!("^[a-z]*$")),
        Some(json!("(")),
        Some(json!(true)),
        Some(json!(null)),
        Some(json!("10.00")),
    ]
}

fn regola(
    nome: &str,
    operatore: &str,
    colonna: &str,
    valore: Option<&Value>,
    warning: bool,
) -> Value {
    let mut regola = json!({
        "name": nome,
        "operator": operatore,
        "column": colonna,
        "severity": if warning { "warning" } else { "error" },
    });
    if let (Some(valore), Some(oggetto)) = (valore, regola.as_object_mut()) {
        oggetto.insert("value".into(), valore.clone());
    }
    regola
}

fn confronta(batch: &RecordBatch, regole: &[Value]) {
    for modo in ["annotate", "summary"] {
        let config: ValidateRules =
            serde_json::from_value(json!({"rules": regole, "output_mode": modo})).expect("config");
        assert_same_outcome_bits(
            validate_rules(batch, &config),
            validate_rules_riferimento(batch, &config),
        );
    }
}

#[test]
fn regole_come_il_riferimento_su_ogni_colonna_operatore_e_valore() {
    let batch = tabella();
    let valori = valori();
    for colonna in COLONNE {
        for operatore in OPERATORI {
            for valore in &valori {
                confronta(
                    &batch,
                    &[regola("r", operatore, colonna, valore.as_ref(), false)],
                );
            }
        }
    }
}

#[test]
fn regole_come_il_riferimento_con_piu_regole_e_gravita_miste() {
    let batch = tabella();
    let insiemi = [
        vec![
            regola("a", "ge", "i", Some(&json!(0)), false),
            regola("b;c", "ne", "s", Some(&json!("abc")), true),
            regola("c", "range", "f", Some(&json!("0,10")), false),
            regola("d", "regex", "s", Some(&json!("^[0-9]+$")), true),
            regola("e", "notnull", "k", None, false),
            regola("f", "eq", "b", Some(&json!(true)), true),
            regola("g", "lt", "m", Some(&json!("10.5")), false),
        ],
        vec![
            regola("solo_warning", "isnull", "u", None, true),
            regola("altro_warning", "gt", "d", Some(&json!(1)), true),
        ],
        vec![regola("x", "ge", "i", Some(&json!(0)), false)],
        vec![
            regola("x", "ge", "i", Some(&json!(0)), false),
            regola("x", "le", "i", Some(&json!(0)), false),
        ],
        vec![regola("  ", "ge", "i", Some(&json!(0)), false)],
        vec![],
    ];
    for regole in insiemi {
        confronta(&batch, &regole);
    }
    // Batch vuoto con lo stesso schema.
    let vuoto = batch.slice(0, 0);
    confronta(
        &vuoto,
        &[
            regola("a", "ge", "i", Some(&json!(0)), false),
            regola("b", "eq", "s", Some(&json!("abc")), true),
        ],
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn regole_come_il_riferimento_su_insiemi_casuali(
        scelte in prop::collection::vec(
            (0_usize..10, 0_usize..10, 0_usize..19, any::<bool>()),
            1..6,
        ),
        inizio in 0_usize..8,
        lunghezza in 0_usize..8,
    ) {
        let batch = tabella();
        let inizio = inizio.min(batch.num_rows());
        let lunghezza = lunghezza.min(batch.num_rows() - inizio);
        let batch = batch.slice(inizio, lunghezza);
        let valori = valori();
        let regole = scelte
            .iter()
            .enumerate()
            .map(|(indice, (colonna, operatore, valore, warning))| {
                regola(
                    &format!("r{indice}"),
                    OPERATORI[*operatore],
                    COLONNE[*colonna],
                    valori[*valore].as_ref(),
                    *warning,
                )
            })
            .collect::<Vec<_>>();
        confronta(&batch, &regole);
    }
}
