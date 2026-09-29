//! Oracolo di `window_function`: la funzione com'era prima del calcolo dei
//! ranghi per sequenze di pari merito (`ranghi`), copiata alla lettera, e i
//! confronti degli esiti completi (batch per bit, errori per categoria e
//! messaggio) su input avversari e casuali.

use std::sync::Arc;

use plenora_core::arrow::array::{
    Date32Array, Decimal128Array, Int64Array, StringArray, TimestampMillisecondArray, UInt64Array,
};
use proptest::prelude::*;

use super::*;
use crate::test_support::{assert_same_outcome_bits, nullable_batch};

// Copia letterale del percorso precedente: ogni variante di rango ordina la
// partizione e poi fa due ricerche binarie per riga.
#[allow(clippy::too_many_lines)]
fn window_function_riferimento(
    batch: &RecordBatch,
    config: &WindowFunction,
) -> Result<RecordBatch> {
    config.verifica_offset()?;
    if matches!(config.function, WindowKind::Ntile) {
        if config.buckets.is_none_or(|buckets| buckets == 0) {
            return Err(PlenoraError::InvalidPlan(
                "ntile richiede buckets maggiore di zero".into(),
            ));
        }
    } else if config.buckets.is_some() {
        return Err(PlenoraError::InvalidPlan(
            "buckets e' ammesso solo per ntile".into(),
        ));
    }
    let source_index = column_index(batch, &config.column)?;
    let ordered = if let Some(column) = &config.order_column {
        sort(
            batch,
            &Sort {
                columns: vec![column.clone()],
                ascending: true,
            },
        )?
    } else {
        batch.clone()
    };
    let group_index = config
        .group_by
        .as_deref()
        .map(|name| column_index(&ordered, name))
        .transpose()?;
    // Partizionamento condiviso con `rolling_window`.
    let partitions = build_partitions(&ordered, group_index)?;
    let colonna = ordered.column(source_index);
    // Le varianti di VALORE leggono `Float64` (arrotondamento dichiarato),
    // quelle di RANGO il dominio originale (vedi il doc sopra). Il dominio
    // numerico si valida sempre, anche per le varianti di posizione.
    let strategia = strategia(&config.function);
    let source = (strategia == Strategia::Valore).then(|| Float64Source::new(colonna));
    let ordine = match strategia {
        Strategia::Rango => Some(OrdineNumerico::new(colonna)?),
        Strategia::Valore => None,
        // Non legge i valori, ma il contratto della colonna vale lo stesso:
        // una colonna dichiarata numerica e piena di parole e' un ingresso
        // invalido a prescindere da cosa il kernel ne fa.
        Strategia::Posizione => {
            valida_valori_numerici(colonna)?;
            None
        }
    };
    let compute = |rows: &[usize]| -> Result<Vec<Option<f64>>> {
        let numbers = match &source {
            Some(source) => rows
                .iter()
                .map(|row| source.value(*row))
                .collect::<Result<Vec<_>>>()?,
            None => Vec::new(),
        };
        // Righe non nulle della partizione, ordinate per VALORE della cella:
        // il rango si legge dalle posizioni, e nessun valore viene convertito.
        let mut sorted: Vec<usize> = Vec::new();
        let mut dense: Vec<usize> = Vec::new();
        if let Some(ordine) = &ordine {
            sorted = rows
                .iter()
                .copied()
                .filter(|row| !colonna.is_null(*row))
                .collect();
            let mut guasto = None;
            sorted.sort_by(|sinistra, destra| confronta(ordine, *sinistra, *destra, &mut guasto));
            if let Some(errore) = guasto {
                return Err(errore);
            }
            dense.clone_from(&sorted);
            let mut guasto = None;
            dense.dedup_by(|sinistra, destra| {
                confronta(ordine, *sinistra, *destra, &mut guasto) == Ordering::Equal
            });
            if let Some(errore) = guasto {
                return Err(errore);
            }
        }
        let mut sum = 0.0;
        let mut count = 0.0_f64;
        // I confronti dentro il ciclo sono fallibili: l'errore si raccoglie e
        // si rende PRIMA dei valori, che altrimenti sarebbero calcolati su un
        // ordinamento che non si e' potuto stabilire.
        let mut errore: Option<PlenoraError> = None;
        let mut values = Vec::with_capacity(rows.len());
        let ordine = ordine.as_ref();
        for position in 0..rows.len() {
            values.push(match config.function {
                WindowKind::Cumcount => position.to_f64(),
                WindowKind::Cumsum => numbers[position].map(|value| {
                    sum += value;
                    sum
                }),
                WindowKind::RunningMean => numbers[position].map(|value| {
                    sum += value;
                    count += 1.0;
                    sum / count
                }),
                WindowKind::Lag => position
                    .checked_sub(config.offset())
                    .and_then(|other| numbers[other]),
                WindowKind::Lead => numbers.get(position + config.offset()).copied().flatten(),
                WindowKind::PctChange => position
                    .checked_sub(1)
                    .and_then(|previous| numbers[previous])
                    .and_then(|previous| {
                        numbers[position]
                            .filter(|_| previous != 0.0)
                            .map(|current| (current - previous) / previous)
                    }),
                WindowKind::Rank | WindowKind::DenseRank => ordine.and_then(|ordine| {
                    let riga = rows[position];
                    if colonna.is_null(riga) {
                        None
                    } else if matches!(config.function, WindowKind::DenseRank) {
                        dense
                            .binary_search_by(|altra| confronta(ordine, *altra, riga, &mut errore))
                            .ok()
                            .and_then(|index| (index + 1).to_f64())
                    } else {
                        let first = sorted.partition_point(|altra| {
                            confronta(ordine, *altra, riga, &mut errore).is_lt()
                        });
                        sorted
                            .partition_point(|altra| {
                                !confronta(ordine, *altra, riga, &mut errore).is_gt()
                            })
                            .checked_sub(1)
                            .and_then(|last| (first + last + 2).to_f64().map(|sum| sum / 2.0))
                    }
                }),
                WindowKind::PercentRank => ordine.and_then(|ordine| {
                    let riga = rows[position];
                    if colonna.is_null(riga) {
                        None
                    } else if sorted.len() <= 1 {
                        Some(0.0)
                    } else {
                        let rank = sorted.partition_point(|altra| {
                            confronta(ordine, *altra, riga, &mut errore).is_lt()
                        });
                        rank.to_f64()
                            .zip((sorted.len() - 1).to_f64())
                            .map(|(numeratore, denominatore)| numeratore / denominatore)
                    }
                }),
                WindowKind::CumeDist => ordine.and_then(|ordine| {
                    let riga = rows[position];
                    if colonna.is_null(riga) {
                        None
                    } else {
                        sorted
                            .partition_point(|altra| {
                                !confronta(ordine, *altra, riga, &mut errore).is_gt()
                            })
                            .checked_sub(1)
                            .and_then(|last| {
                                (last + 1)
                                    .to_f64()
                                    .zip(sorted.len().to_f64())
                                    .map(|(numeratore, denominatore)| numeratore / denominatore)
                            })
                    }
                }),
                WindowKind::Ntile => {
                    let buckets = config.buckets.unwrap_or(1);
                    let effective = buckets.min(rows.len());
                    position
                        .checked_mul(effective)
                        .and_then(|value| value.checked_div(rows.len()))
                        .and_then(|value| (value + 1).to_f64())
                }
            });
        }
        if let Some(errore) = errore {
            return Err(errore);
        }
        Ok(values)
    };
    let mut output = vec![None; ordered.num_rows()];
    scatter_partitions(&ordered, &partitions, &mut output, compute)?;
    let suffix = match config.function {
        WindowKind::Rank => "rank",
        WindowKind::DenseRank => "dense_rank",
        WindowKind::Cumsum => "cumsum",
        WindowKind::Cumcount => "cumcount",
        WindowKind::Lag => "lag",
        WindowKind::Lead => "lead",
        WindowKind::PctChange => "pct_change",
        WindowKind::RunningMean => "running_mean",
        WindowKind::PercentRank => "percent_rank",
        WindowKind::CumeDist => "cume_dist",
        WindowKind::Ntile => "ntile",
    };
    let name = config
        .output_column
        .clone()
        .unwrap_or_else(|| format!("{}_{}", config.column, suffix));
    replace_or_append(
        &ordered,
        &name,
        DataType::Float64,
        true,
        Arc::new(Float64Array::from(output)),
    )
}

const FUNZIONI: [WindowKind; 11] = [
    WindowKind::Rank,
    WindowKind::DenseRank,
    WindowKind::PercentRank,
    WindowKind::CumeDist,
    WindowKind::Cumsum,
    WindowKind::Cumcount,
    WindowKind::Lag,
    WindowKind::Lead,
    WindowKind::PctChange,
    WindowKind::RunningMean,
    WindowKind::Ntile,
];

/// Tutte le combinazioni di funzione, partizione e ordinamento su `batch`,
/// con la colonna `v` come sorgente.
fn confronta_tutto(batch: &RecordBatch) {
    for funzione in FUNZIONI {
        for group_by in [None, Some("g")] {
            for order_column in [None, Some("o"), Some("v")] {
                let buckets = matches!(funzione, WindowKind::Ntile).then_some(3);
                let config = WindowFunction {
                    column: "v".into(),
                    function: funzione.clone(),
                    group_by: group_by.map(Into::into),
                    order_column: order_column.map(Into::into),
                    offset: matches!(funzione, WindowKind::Lag | WindowKind::Lead).then_some(2),
                    buckets,
                    output_column: None,
                };
                assert_same_outcome_bits(
                    window_function(batch, &config),
                    window_function_riferimento(batch, &config),
                );
            }
        }
    }
}

fn con_gruppi(valori: ArrayRef) -> RecordBatch {
    let righe = valori.len();
    let gruppi = (0..righe)
        .map(|riga| match riga % 4 {
            0 => None,
            1 => Some("a"),
            _ => Some("b"),
        })
        .collect::<Vec<_>>();
    let ordine = (0..righe)
        .map(|riga| i64::try_from((riga * 7) % 5).ok())
        .collect::<Vec<_>>();
    nullable_batch(vec![
        ("v", valori),
        ("g", Arc::new(StringArray::from(gruppi))),
        ("o", Arc::new(Int64Array::from(ordine))),
    ])
}

/// Double che il preordine distingue o accomuna: zeri con segno, NaN con
/// segni e payload diversi, infiniti, subnormali, duplicati.
fn double_avversari() -> Vec<Option<f64>> {
    vec![
        Some(1.5),
        Some(-0.0),
        Some(0.0),
        Some(f64::NAN),
        Some(f64::from_bits(0xfff8_0000_0000_0000)),
        Some(f64::from_bits(0x7ff0_0000_0000_0001)),
        None,
        Some(f64::INFINITY),
        Some(f64::NEG_INFINITY),
        Some(f64::MIN_POSITIVE / 2.0),
        Some(1.5),
        Some(-0.0),
        None,
        Some(f64::NAN),
        Some(f64::MAX),
        Some(f64::MIN),
        Some(1.5),
    ]
}

#[test]
fn ranghi_come_il_riferimento_sui_double_avversari() {
    confronta_tutto(&con_gruppi(Arc::new(
        Float64Array::from(double_avversari()),
    )));
}

#[test]
fn ranghi_come_il_riferimento_su_ogni_tipo_ordinabile() {
    let interi = vec![
        Some(i64::MAX),
        Some(i64::MIN),
        None,
        Some(0),
        Some(i64::MAX - 1),
        Some(9_007_199_254_740_993),
        Some(9_007_199_254_740_992),
        Some(0),
        Some(-1),
        Some(i64::MAX),
    ];
    let colonne: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(interi.clone())),
        Arc::new(UInt64Array::from(
            interi
                .iter()
                .map(|valore| valore.map(i64::unsigned_abs))
                .collect::<Vec<_>>(),
        )),
        Arc::new(Date32Array::from(
            interi
                .iter()
                .map(|valore| valore.map(|valore| i32::try_from(valore % 100_000).unwrap_or(0)))
                .collect::<Vec<_>>(),
        )),
        Arc::new(TimestampMillisecondArray::from(interi.clone()).with_timezone("Europe/Rome")),
        Arc::new(
            Decimal128Array::from(
                interi
                    .iter()
                    .map(|valore| valore.map(|valore| i128::from(valore) * 1_000 + 7))
                    .collect::<Vec<_>>(),
            )
            .with_precision_and_scale(38, 3)
            .expect("decimal"),
        ),
    ];
    for colonna in colonne {
        confronta_tutto(&con_gruppi(colonna));
    }
}

#[test]
fn ranghi_come_il_riferimento_sui_casi_limite() {
    // Vuoto, una riga, tutti null, tutti uguali, testo (rifiutato dai
    // ranghi, accettato dalle altre varianti se numerico).
    let casi: Vec<ArrayRef> = vec![
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
        Arc::new(Float64Array::from(vec![Some(2.0)])),
        Arc::new(Float64Array::from(vec![None, None, None])),
        Arc::new(Int64Array::from(vec![Some(4); 9])),
        Arc::new(StringArray::from(vec![
            Some("1"),
            Some("2"),
            None,
            Some("1"),
        ])),
        Arc::new(StringArray::from(vec![Some("x"), Some("2")])),
    ];
    for colonna in casi {
        confronta_tutto(&con_gruppi(colonna));
    }
}

#[test]
fn ranghi_come_il_riferimento_su_una_partizione_grande_con_molti_pari_merito() {
    // Sopra la soglia del calcolo parallelo, con e senza partizioni: 100
    // valori distinti su 40 000 righe, come la fixture del catalogo.
    let valori = (0..40_000_u32)
        .map(|riga| (riga % 7 != 0).then(|| f64::from(riga % 100) + 0.5))
        .collect::<Vec<_>>();
    let batch = con_gruppi(Arc::new(Float64Array::from(valori)));
    for funzione in [
        WindowKind::Rank,
        WindowKind::DenseRank,
        WindowKind::PercentRank,
        WindowKind::CumeDist,
    ] {
        for group_by in [None, Some("g")] {
            let config = WindowFunction {
                column: "v".into(),
                function: funzione.clone(),
                group_by: group_by.map(Into::into),
                order_column: None,
                offset: None,
                buckets: None,
                output_column: Some("r".into()),
            };
            assert_same_outcome_bits(
                window_function(&batch, &config),
                window_function_riferimento(&batch, &config),
            );
        }
    }
}

/// Double da un piccolo insieme, cosi' i pari merito sono frequenti.
fn double_piccolo() -> impl Strategy<Value = Option<f64>> {
    prop_oneof![
        1 => Just(None),
        1 => Just(Some(f64::NAN)),
        1 => Just(Some(-0.0)),
        1 => Just(Some(f64::INFINITY)),
        6 => (-4_i8..5).prop_map(|valore| Some(f64::from(valore) / 2.0)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn ranghi_come_il_riferimento_su_input_casuali(
        valori in prop::collection::vec(double_piccolo(), 0..60),
        interi in prop::collection::vec(prop::option::of(-3_i64..4), 0..60),
        gruppi in prop::collection::vec(prop::option::of(0_u8..3), 60),
        ordine in prop::collection::vec(prop::option::of(0_i64..5), 60),
        funzione in prop::sample::select(FUNZIONI.to_vec()),
        group in any::<bool>(),
        ordina in any::<bool>(),
        su_interi in any::<bool>(),
    ) {
        let colonna: ArrayRef = if su_interi {
            Arc::new(Int64Array::from(interi))
        } else {
            Arc::new(Float64Array::from(valori))
        };
        let righe = colonna.len();
        let gruppi = gruppi[..righe]
            .iter()
            .map(|gruppo| gruppo.map(|gruppo| format!("g{gruppo}")))
            .collect::<Vec<_>>();
        let batch = nullable_batch(vec![
            ("v", colonna),
            ("g", Arc::new(StringArray::from(gruppi))),
            ("o", Arc::new(Int64Array::from(ordine[..righe].to_vec()))),
        ]);
        let config = WindowFunction {
            column: "v".into(),
            buckets: matches!(funzione, WindowKind::Ntile).then_some(4),
            function: funzione,
            group_by: group.then(|| "g".into()),
            order_column: ordina.then(|| "o".into()),
            offset: None,
            output_column: None,
        };
        assert_same_outcome_bits(
            window_function(&batch, &config),
            window_function_riferimento(&batch, &config),
        );
    }
}
