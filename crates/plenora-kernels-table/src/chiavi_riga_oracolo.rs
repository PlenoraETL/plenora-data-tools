//! Oracolo di `reconcile` e `assert_foreign_key` con chiavi binarie: le
//! funzioni com'erano con le chiavi testuali di `RowKeyEncoder` in mappe
//! per lato, copiate alla lettera, e i confronti degli esiti completi
//! (batch, errori per categoria, messaggio e diagnostica di riga), anche
//! su ogni soglia di `max_governed_memory_bytes` e `max_rows`.

use std::hash::BuildHasherDefault;
use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, BinaryArray, Date32Array, Decimal128Array, DictionaryArray,
    TimestampMillisecondArray,
};
use proptest::prelude::*;

use super::*;
use crate::test_support::{assert_same_outcome_bits, nullable_batch};

// Copia letterale del percorso precedente.
type KeySet = HashSet<Vec<u8>, FastHasher>;
type KeyFreqMap = HashMap<Vec<u8>, usize, FastHasher>;

fn assert_foreign_key_riferimento(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &ForeignKey,
    limits: &Limits,
) -> Result<RecordBatch> {
    let left_indices = key_indices(left, &config.left_keys)?;
    let right_indices = key_indices(right, &config.right_keys)?;
    validate_key_types(left, right, &left_indices, &right_indices)?;
    let mut right_encoder = RowKeyEncoder::new(right, &right_indices);
    let mut referenced =
        KeySet::with_capacity_and_hasher(right.num_rows(), BuildHasherDefault::default());
    let mut memory_used = 0_usize;
    let mut key = Vec::new();
    for row in 0..right.num_rows() {
        if !has_null(right, &right_indices, row) {
            right_encoder.encode_into(row, &mut key)?;
            let key_bytes = key.len();
            if referenced.insert(std::mem::take(&mut key)) {
                memory_used = memory_used
                    .checked_add(key_bytes.saturating_add(64))
                    .ok_or_else(|| {
                        PlenoraError::ResourceLimit("overflow memoria foreign key".into())
                    })?;
                if memory_used > limits.max_governed_memory_bytes {
                    return Err(PlenoraError::ResourceLimit(
                        "assert_foreign_key oltre max_governed_memory_bytes".into(),
                    ));
                }
            }
        }
    }
    let mut left_encoder = RowKeyEncoder::new(left, &left_indices);
    let mut rejections = Vec::new();
    for row in 0..left.num_rows() {
        if has_null(left, &left_indices, row) {
            if config.allow_null {
                continue;
            }
            rejections.push(RowRejection {
                row,
                cause: "validation.foreign_key_null",
                column: None,
            });
            continue;
        }
        left_encoder.encode_into(row, &mut key)?;
        if !referenced.contains(key.as_slice()) {
            rejections.push(RowRejection {
                row,
                cause: "validation.foreign_key_missing",
                column: None,
            });
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(left.clone())
}

fn frequencies_riferimento(
    batch: &RecordBatch,
    indices: &[usize],
    nulls_equal: bool,
    side_nulls: &mut usize,
    memory_used: &mut usize,
    limits: &Limits,
) -> Result<KeyFreqMap> {
    let mut output = KeyFreqMap::default();
    let mut encoder = RowKeyEncoder::new(batch, indices);
    let mut key = Vec::new();
    for row in 0..batch.num_rows() {
        if !nulls_equal && has_null(batch, indices, row) {
            *side_nulls = side_nulls.checked_add(1).ok_or_else(|| {
                PlenoraError::ResourceLimit("overflow null reconciliation".into())
            })?;
            continue;
        }
        encoder.encode_into(row, &mut key)?;
        if let Some(count) = output.get_mut(key.as_slice()) {
            *count = count
                .checked_add(1)
                .ok_or_else(|| PlenoraError::ResourceLimit("overflow reconciliation".into()))?;
        } else {
            *memory_used = memory_used
                .checked_add(key.len().saturating_add(64))
                .ok_or_else(|| {
                    PlenoraError::ResourceLimit("overflow memoria reconciliation".into())
                })?;
            if *memory_used > limits.max_governed_memory_bytes {
                return Err(PlenoraError::ResourceLimit(
                    "reconcile oltre max_governed_memory_bytes".into(),
                ));
            }
            output.insert(std::mem::take(&mut key), 1);
            if output.len() > limits.max_rows {
                return Err(PlenoraError::ResourceLimit(
                    "reconcile supera max_rows chiavi distinte".into(),
                ));
            }
        }
    }
    Ok(output)
}

fn reconcile_riferimento(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &Reconcile,
    limits: &Limits,
) -> Result<RecordBatch> {
    let left_indices = key_indices(left, &config.left_keys)?;
    let right_indices = key_indices(right, &config.right_keys)?;
    validate_key_types(left, right, &left_indices, &right_indices)?;
    let mut left_nulls = 0;
    let mut right_nulls = 0;
    let mut memory_used = 0_usize;
    let left_counts = frequencies_riferimento(
        left,
        &left_indices,
        config.nulls_equal,
        &mut left_nulls,
        &mut memory_used,
        limits,
    )?;
    let right_counts = frequencies_riferimento(
        right,
        &right_indices,
        config.nulls_equal,
        &mut right_nulls,
        &mut memory_used,
        limits,
    )?;
    let mut matched = 0_usize;
    let mut left_only = left_nulls;
    let mut right_only = right_nulls;
    let mut left_duplicates = 0_usize;
    let mut right_duplicates = 0_usize;
    for (key, left_count) in &left_counts {
        let right_count = right_counts.get(key).copied().unwrap_or_default();
        let common = (*left_count).min(right_count);
        matched = matched.saturating_add(common);
        left_only = left_only.saturating_add(left_count - common);
        left_duplicates = left_duplicates.saturating_add(left_count.saturating_sub(1));
    }
    for (key, right_count) in &right_counts {
        let left_count = left_counts.get(key).copied().unwrap_or_default();
        right_only = right_only.saturating_add(right_count.saturating_sub(left_count));
        right_duplicates = right_duplicates.saturating_add(right_count.saturating_sub(1));
    }
    let metrics = [
        "matched_rows",
        "left_only_rows",
        "right_only_rows",
        "left_duplicate_rows",
        "right_duplicate_rows",
    ];
    let values = [
        matched,
        left_only,
        right_only,
        left_duplicates,
        right_duplicates,
    ]
    .into_iter()
    .map(as_u64)
    .collect::<Result<Vec<_>>>()?;
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("metric", DataType::Utf8, false),
            Field::new("value", DataType::UInt64, false),
        ])),
        vec![
            Arc::new(StringArray::from(metrics.to_vec())),
            Arc::new(UInt64Array::from(values)),
        ],
    )?)
}

/// Una colonna per tipo di chiave, con null, duplicati e i valori il cui
/// testo e' difficile (NaN di ogni payload, zeri con segno, estremi).
// Una colonna per tipo, elencata per esteso: spezzarla nasconderebbe i valori.
#[allow(clippy::too_many_lines)]
fn tabella(righe: usize, spostamento: usize) -> RecordBatch {
    let indice = |riga: usize| (riga + spostamento) % 7;
    let interi = [
        Some(1),
        None,
        Some(-5),
        Some(i64::MAX),
        Some(i64::MIN),
        Some(1),
        Some(0),
    ];
    let testi = [
        Some("a"),
        None,
        Some("héllo"),
        Some(""),
        Some("a"),
        Some("x;y"),
        Some("0"),
    ];
    let double = [
        Some(f64::NAN),
        Some(-0.0),
        Some(0.0),
        Some(f64::from_bits(0xfff8_0000_0000_0001)),
        None,
        Some(1e300),
        Some(5e-324),
    ];
    let booleani = [
        Some(true),
        None,
        Some(false),
        Some(true),
        Some(false),
        None,
        Some(true),
    ];
    let senza_segno = [
        Some(0),
        None,
        Some(u64::MAX),
        Some(7),
        Some(10),
        Some(0),
        Some(99),
    ];
    let date = [
        Some(0),
        None,
        Some(-1),
        Some(19_000),
        Some(0),
        Some(5),
        Some(-719_162),
    ];
    let chiavi_dizionario = [Some(0), Some(1), None, Some(2), Some(0), Some(1), Some(2)];
    let dizionario = DictionaryArray::<Int32Type>::try_new(
        (0..righe)
            .map(|riga| chiavi_dizionario[indice(riga)])
            .collect::<Vec<Option<i32>>>()
            .into(),
        Arc::new(StringArray::from(vec![Some("x"), None, Some("10")])),
    )
    .expect("dictionary");
    nullable_batch(vec![
        (
            "i",
            Arc::new(Int64Array::from(
                (0..righe)
                    .map(|riga| interi[indice(riga)])
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "s",
            Arc::new(StringArray::from(
                (0..righe)
                    .map(|riga| testi[indice(riga)])
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "f",
            Arc::new(Float64Array::from(
                (0..righe)
                    .map(|riga| double[indice(riga)])
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "b",
            Arc::new(BooleanArray::from(
                (0..righe)
                    .map(|riga| booleani[indice(riga)])
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "u",
            Arc::new(UInt64Array::from(
                (0..righe)
                    .map(|riga| senza_segno[indice(riga)])
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "d",
            Arc::new(Date32Array::from(
                (0..righe)
                    .map(|riga| date[indice(riga)])
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "t",
            Arc::new(
                TimestampMillisecondArray::from(
                    (0..righe)
                        .map(|riga| interi[indice(riga)].map(|valore| valore / 1_000))
                        .collect::<Vec<_>>(),
                )
                .with_timezone("Europe/Rome"),
            ),
        ),
        (
            "m",
            Arc::new(
                Decimal128Array::from(
                    (0..righe)
                        .map(|riga| interi[indice(riga)].map(i128::from))
                        .collect::<Vec<_>>(),
                )
                .with_precision_and_scale(38, 2)
                .expect("decimal"),
            ),
        ),
        ("k", Arc::new(dizionario)),
    ])
}

/// Binary con un valore non UTF-8: `scalar_as_string` fallisce su quella
/// riga, e l'errore deve essere lo stesso alla stessa riga.
fn tabella_binaria(con_errore: bool) -> RecordBatch {
    let valori: Vec<Option<&[u8]>> = vec![
        Some(b"ab"),
        None,
        Some(b""),
        Some(if con_errore { b"\xff" } else { b"c" }),
        Some(b"ab"),
    ];
    nullable_batch(vec![("x", Arc::new(BinaryArray::from(valori)))])
}

const CHIAVI: [&[&str]; 13] = [
    &["i"],
    &["s"],
    &["f"],
    &["b"],
    &["u"],
    &["d"],
    &["t"],
    &["m"],
    &["k"],
    &["i", "s"],
    &["s", "i"],
    &["f", "b", "k"],
    &["i", "s", "f", "b", "u", "d", "t", "m", "k"],
];

fn nomi(chiavi: &[&str]) -> Vec<String> {
    chiavi.iter().map(|nome| (*nome).to_owned()).collect()
}

fn confronta_reconcile(left: &RecordBatch, right: &RecordBatch, chiavi: &[&str], limits: &Limits) {
    for nulls_equal in [true, false] {
        let config = Reconcile {
            left_keys: nomi(chiavi),
            right_keys: nomi(chiavi),
            nulls_equal,
        };
        assert_same_outcome_bits(
            reconcile(left, right, &config, limits),
            reconcile_riferimento(left, right, &config, limits),
        );
    }
}

fn confronta_foreign_key(
    left: &RecordBatch,
    right: &RecordBatch,
    chiavi: &[&str],
    limits: &Limits,
) {
    for allow_null in [true, false] {
        let config = ForeignKey {
            left_keys: nomi(chiavi),
            right_keys: nomi(chiavi),
            allow_null,
        };
        assert_same_outcome_bits(
            assert_foreign_key(left, right, &config, limits),
            assert_foreign_key_riferimento(left, right, &config, limits),
        );
    }
}

/// `lunghezza_testuale` rende la lunghezza dei byte di `RowKeyEncoder` (e
/// quindi di `key_for_row`) su ogni tipo e combinazione di colonne.
#[test]
fn la_lunghezza_testuale_coincide_con_i_byte_della_chiave_testuale() {
    let batch = tabella(14, 0);
    let mut testo = String::new();
    for chiavi in CHIAVI {
        let indices = key_indices(&batch, &nomi(chiavi)).expect("chiavi");
        let lato = LatoChiavi::new(&batch, &indices);
        let mut encoder = RowKeyEncoder::new(&batch, &indices);
        let mut chiave = Vec::new();
        for row in 0..batch.num_rows() {
            // Un timestamp fuori dall'intervallo delle date non ha testo:
            // stesso errore dai due lati.
            match encoder.encode_into(row, &mut chiave) {
                Ok(()) => assert_eq!(
                    lato.lunghezza_testuale(row, &mut testo).expect("lunghezza"),
                    chiave.len(),
                    "chiavi {chiavi:?} riga {row}"
                ),
                Err(errore) => assert_eq!(
                    lato.lunghezza_testuale(row, &mut testo)
                        .expect_err("stesso errore")
                        .to_string(),
                    errore.to_string()
                ),
            }
        }
    }
    let binaria = tabella_binaria(false);
    let lato = LatoChiavi::new(&binaria, &[0]);
    let mut encoder = RowKeyEncoder::new(&binaria, &[0]);
    let mut chiave = Vec::new();
    for row in 0..binaria.num_rows() {
        encoder
            .encode_into(row, &mut chiave)
            .expect("chiave testuale");
        assert_eq!(
            lato.lunghezza_testuale(row, &mut testo).expect("lunghezza"),
            chiave.len()
        );
    }
}

#[test]
fn reconcile_e_foreign_key_come_il_riferimento_su_ogni_chiave() {
    let limits = Limits::default();
    for (sinistra, destra) in [(14, 0), (9, 3), (0, 2), (3, 0)] {
        let left = tabella(sinistra, 0);
        let right = tabella(destra.max(5), destra);
        for chiavi in CHIAVI {
            confronta_reconcile(&left, &right, chiavi, &limits);
            confronta_reconcile(&right, &left, chiavi, &limits);
            confronta_foreign_key(&left, &right, chiavi, &limits);
            confronta_foreign_key(&right, &left, chiavi, &limits);
            confronta_foreign_key(&left, &left, chiavi, &limits);
        }
    }
    // Errori di conversione alla stessa riga, sui due lati.
    for (left, right) in [
        (tabella_binaria(true), tabella_binaria(false)),
        (tabella_binaria(false), tabella_binaria(true)),
        (tabella_binaria(false), tabella_binaria(false)),
    ] {
        confronta_reconcile(&left, &right, &["x"], &limits);
        confronta_foreign_key(&left, &right, &["x"], &limits);
    }
    // Colonne assenti e tipi diversi.
    let left = tabella(5, 0);
    confronta_reconcile(&left, &left, &["assente"], &limits);
    let config = Reconcile {
        left_keys: nomi(&["i"]),
        right_keys: nomi(&["u"]),
        nulls_equal: true,
    };
    assert_same_outcome_bits(
        reconcile(&left, &left, &config, &limits),
        reconcile_riferimento(&left, &left, &config, &limits),
    );
}

/// Ogni soglia di memoria e di chiavi distinte scatta alla stessa riga e
/// con lo stesso errore: la contabilita' resta quella delle chiavi
/// testuali, per lato.
#[test]
fn reconcile_e_foreign_key_come_il_riferimento_su_ogni_soglia() {
    let left = tabella(14, 0);
    let right = tabella(10, 4);
    for chiavi in [&["i"][..], &["s"], &["f", "k"], &["i", "s", "d", "m"]] {
        for max_governed_memory_bytes in 0..=2_000 {
            let limits = Limits {
                max_governed_memory_bytes,
                ..Limits::default()
            };
            confronta_reconcile(&left, &right, chiavi, &limits);
            confronta_foreign_key(&left, &right, chiavi, &limits);
        }
        for max_rows in 0..=16 {
            let limits = Limits {
                max_rows,
                ..Limits::default()
            };
            confronta_reconcile(&left, &right, chiavi, &limits);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn reconcile_e_foreign_key_come_il_riferimento_su_input_casuali(
        sinistra in prop::collection::vec((prop::option::of(-3_i64..4), prop::option::of(0_u8..3)), 0..30),
        destra in prop::collection::vec((prop::option::of(-3_i64..4), prop::option::of(0_u8..3)), 0..30),
        composta in any::<bool>(),
        memoria in prop_oneof![Just(usize::MAX), 0_usize..3_000],
        max_rows in prop_oneof![Just(usize::MAX), 0_usize..12],
    ) {
        let lato = |righe: &[(Option<i64>, Option<u8>)]| {
            nullable_batch(vec![
                ("i", Arc::new(Int64Array::from(righe.iter().map(|riga| riga.0).collect::<Vec<_>>()))),
                (
                    "s",
                    Arc::new(StringArray::from(
                        righe
                            .iter()
                            .map(|riga| riga.1.map(|testo| format!("t{testo}")))
                            .collect::<Vec<_>>(),
                    )),
                ),
            ])
        };
        let (left, right) = (lato(&sinistra), lato(&destra));
        let chiavi: &[&str] = if composta { &["i", "s"] } else { &["i"] };
        let limits = Limits {
            max_governed_memory_bytes: memoria,
            max_rows,
            ..Limits::default()
        };
        confronta_reconcile(&left, &right, chiavi, &limits);
        confronta_foreign_key(&left, &right, chiavi, &limits);
    }
}
