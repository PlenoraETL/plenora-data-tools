//! Oracolo di `split_column`: la funzione com'era prima dei builder Arrow
//! diretti, copiata alla lettera, e i confronti degli esiti completi (batch
//! per byte, errori per categoria e messaggio).

use std::sync::Arc;

use proptest::prelude::*;

use super::*;
use crate::test_support::{assert_same_outcome_bits, nullable_batch};

// Copia letterale del percorso precedente: un `Vec` di parti e una `String`
// per cella.
fn split_column_riferimento(
    batch: &RecordBatch,
    config: &SplitColumn,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.delimiter.is_empty() {
        return Err(PlenoraError::InvalidPlan("delimiter vuoto".into()));
    }
    if config.new_columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "new_columns e' obbligatorio nel percorso streaming".into(),
        ));
    }
    if config.new_columns.len() > limits.max_split_columns {
        return Err(PlenoraError::InvalidPlan(
            "split_column supera max_split_columns".into(),
        ));
    }
    let unique: HashSet<_> = config.new_columns.iter().collect();
    if unique.len() != config.new_columns.len() {
        return Err(PlenoraError::Schema(
            "split_column contiene nomi output duplicati".into(),
        ));
    }
    for name in &config.new_columns {
        validate_output_name(name)?;
    }
    let input = utf8_column(batch, &config.column)?;
    let requested_parts = config.new_columns.len();
    let split_limit = if config.max_splits > 0 {
        usize::try_from(config.max_splits)
            .unwrap_or(usize::MAX)
            .saturating_add(1)
            .min(requested_parts)
    } else {
        requested_parts
    };
    let mut outputs = vec![Vec::<Option<String>>::with_capacity(batch.num_rows()); requested_parts];
    for row in 0..batch.num_rows() {
        if input.is_null(row) {
            for output in &mut outputs {
                output.push(None);
            }
            continue;
        }
        let parts: Vec<&str> = input
            .value(row)
            .splitn(split_limit, &config.delimiter)
            .collect();
        for (index, output) in outputs.iter_mut().enumerate() {
            output.push(parts.get(index).map(|value| (*value).to_owned()));
        }
    }
    let mut result = batch.clone();
    for (name, output) in config.new_columns.iter().zip(outputs) {
        result = replace_or_append(
            &result,
            name,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(output)),
        )?;
    }
    Ok(result)
}

fn confronta(batch: &RecordBatch, config: &SplitColumn, limits: &Limits) {
    assert_same_outcome_bits(
        split_column(batch, config, limits),
        split_column_riferimento(batch, config, limits),
    );
}

fn config(delimiter: &str, new_columns: &[&str], max_splits: i64) -> SplitColumn {
    SplitColumn {
        column: "s".into(),
        delimiter: delimiter.into(),
        new_columns: new_columns.iter().map(|nome| (*nome).to_owned()).collect(),
        max_splits,
    }
}

fn testi() -> RecordBatch {
    nullable_batch(vec![
        (
            "s",
            Arc::new(StringArray::from(vec![
                Some("a/b/c"),
                None,
                Some(""),
                Some("/"),
                Some("//"),
                Some("a//b"),
                Some("aaaa"),
                Some("città/perché/ü"),
                Some("senza"),
                Some("a/b/c/d/e/f"),
                Some("x::y::::z"),
            ])),
        ),
        ("altra", Arc::new(Int64Array::from(vec![Some(1); 11]))),
    ])
}

#[test]
fn split_come_il_riferimento_su_delimitatori_e_limiti() {
    let batch = testi();
    let limits = Limits::default();
    let colonne: [&[&str]; 5] = [
        &["p"],
        &["p", "q"],
        &["p", "q", "r"],
        &["p", "q", "r", "t", "u", "v", "w"],
        &["altra", "p"],
    ];
    for delimiter in ["/", "//", "a", "aa", "::", "é", "ü", "x", ""] {
        for nuove in colonne {
            for max_splits in [-1, 0, 1, 2, 3, 6, i64::MAX, i64::MIN] {
                confronta(&batch, &config(delimiter, nuove, max_splits), &limits);
            }
        }
    }
}

#[test]
fn split_come_il_riferimento_sugli_errori() {
    let batch = testi();
    let limits = Limits::default();
    confronta(&batch, &config("/", &[], -1), &limits);
    confronta(&batch, &config("/", &["p", "p"], -1), &limits);
    confronta(&batch, &config("/", &["", "q"], -1), &limits);
    let stretti = Limits {
        max_split_columns: 2,
        ..Limits::default()
    };
    confronta(&batch, &config("/", &["p", "q", "r"], -1), &stretti);
    confronta(&batch, &config("/", &["p", "q"], -1), &stretti);
    let mut assente = config("/", &["p"], -1);
    assente.column = "assente".into();
    confronta(&batch, &assente, &limits);
    let mut non_testo = config("/", &["p"], -1);
    non_testo.column = "altra".into();
    confronta(&batch, &non_testo, &limits);
}

#[test]
fn split_come_il_riferimento_su_batch_vuoto_e_tutto_null() {
    let limits = Limits::default();
    for valori in [Vec::<Option<&str>>::new(), vec![None, None]] {
        let batch = nullable_batch(vec![("s", Arc::new(StringArray::from(valori)) as ArrayRef)]);
        confronta(&batch, &config("/", &["p", "q"], -1), &limits);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn split_come_il_riferimento_su_input_casuali(
        valori in prop::collection::vec(
            prop::option::of(prop::collection::vec(prop::sample::select(vec!["a", "b", "/", "é", "//", ""]), 0..8)),
            0..30,
        ),
        delimiter in prop::sample::select(vec!["/", "a", "//", "é", "b/"]),
        colonne in 1_usize..6,
        max_splits in -2_i64..7,
    ) {
        let valori = valori
            .into_iter()
            .map(|parti| parti.map(|parti| parti.concat()))
            .collect::<Vec<_>>();
        let batch = nullable_batch(vec![("s", Arc::new(StringArray::from(valori)) as ArrayRef)]);
        let nomi = (0..colonne).map(|indice| format!("c{indice}")).collect::<Vec<_>>();
        let config = SplitColumn {
            column: "s".into(),
            delimiter: delimiter.into(),
            new_columns: nomi,
            max_splits,
        };
        confronta(&batch, &config, &Limits::default());
    }
}
