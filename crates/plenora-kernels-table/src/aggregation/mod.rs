//! Kernel di ordinamento, selezione, deduplicazione, aggregazione e
//! funzioni finestra del motore tabellare.
//!
//! I sottomoduli e cio' che ciascuno possiede:
//!
//! - [`compare`]: confronto tipizzato tra celle condiviso da sort e spill;
//! - [`sort`]: `table.sort`, `table.top_n`, `table.distinct`,
//!   `table.dedup_advanced` e i comparatori tipizzati;
//! - [`grouping`]: infrastruttura di raggruppamento (chiavi di gruppo,
//!   hasher, sorgenti tipizzate, partizioni) condivisa da aggregazione e
//!   finestre;
//! - [`aggregate`]: `table.aggregate` (config e riduzioni numeriche);
//! - [`window`]: `table.rolling_window` e `table.window_function`.

mod aggregate;
mod compare;
mod grouping;
mod sort;
mod window;

pub use aggregate::{aggregate, AggFunction, Aggregate, Aggregation};
// Il comparatore tipizzato e' pubblico: e' il contratto d'ordine dei kernel
// (`sort`, top-N, merge dello spill) e va verificabile dall'esterno.
pub(crate) use crate::hashing::KeyHasher;
pub use compare::{compare_cells_typed, is_sortable, validate_sortable};
pub(crate) use grouping::KeyColumn;
pub use sort::{dedup_advanced, distinct, sort, top_n, DedupAdvanced, Distinct, Keep, Sort, TopN};
pub use window::{
    rolling_window, window_function, RollingKind, RollingWindow, WindowFunction, WindowKind,
};
// La classificazione delle varianti serve all'analizzatore, non a chi usa il
// crate: `aggregation` e' un modulo pubblico, quindi il re-export va
// ristretto qui — altrimenti un dettaglio interno diventa API.
pub(crate) use window::{strategia, Strategia};

// Simboli usati solo dai test-oracolo, che li importano con
// `use super::*`.
#[cfg(test)]
use crate::{
    column_index, replace_or_append, scalar_as_f64_rounded, scalar_as_string, select_rows,
    validate_output_name,
};
#[cfg(test)]
use compare::{compare_at, row_key};
#[cfg(test)]
use grouping::{cmp_i64_group_key, cmp_str_group_key, cmp_u64_group_key};
#[cfg(test)]
use num_traits::ToPrimitive;
#[cfg(test)]
use plenora_core::arrow::array::{
    Array, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
#[cfg(test)]
use plenora_core::arrow::schema::DataType;
#[cfg(test)]
use plenora_core::{PlenoraError, Result};
#[cfg(test)]
use std::cmp::Ordering;
#[cfg(test)]
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use plenora_core::arrow::array::{types::Int32Type, DictionaryArray, LargeStringArray};
    use plenora_core::arrow::schema::{Field, Schema};

    use super::*;

    /// Oracolo indipendente: permutazione attesa per una colonna Float64
    /// (null dopo i valori, `total_cmp`, stabile sull'indice originale).
    fn expected_permutation(values: &[Option<f64>], ascending: bool) -> Vec<usize> {
        let mut rows: Vec<usize> = (0..values.len()).collect();
        rows.sort_by(|left, right| {
            let ordering = match (values[*left], values[*right]) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.total_cmp(&b),
            };
            let ordering = if ascending {
                ordering
            } else {
                ordering.reverse()
            };
            ordering.then(left.cmp(right))
        });
        rows
    }

    fn sorted_ids(batch: &RecordBatch, config: &Sort) -> Vec<i64> {
        let sorted = sort(batch, config).expect("sort");
        sorted
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("id column")
            .values()
            .to_vec()
    }

    #[test]
    fn sort_is_stable_with_duplicate_keys_at_parallel_scale() {
        // Sopra la soglia parallela (32_768): chiavi molto duplicate (97
        // valori distinti) verificano stabilita' e determinismo del merge
        // sort rayon contro l'oracolo sequenziale.
        let rows = 50_000_usize;
        let ids = (0..rows)
            .map(|row| i64::try_from(row).expect("id"))
            .collect::<Vec<_>>();
        let values = (0..rows)
            .map(|row| {
                if row % 11 == 0 {
                    None
                } else {
                    Some(f64::from(u32::try_from(row % 97).expect("key")))
                }
            })
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(Float64Array::from(values.clone())),
            ],
        )
        .expect("fixture");
        let config = Sort {
            columns: vec!["num".into()],
            ascending: true,
        };
        let sorted = sorted_ids(&batch, &config);
        let expected = expected_permutation(&values, true)
            .into_iter()
            .map(|row| i64::try_from(row).expect("id"))
            .collect::<Vec<_>>();
        assert_eq!(sorted, expected);
        // Determinismo assoluto: stesso input, stesso output.
        assert_eq!(sorted_ids(&batch, &config), expected);
    }

    #[test]
    fn sort_nan_signed_zero_and_null_ordering_is_exact() {
        let values = vec![Some(0.0), Some(f64::NAN), Some(-0.0), None, Some(1.0)];
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3, 4])),
                Arc::new(Float64Array::from(values.clone())),
            ],
        )
        .expect("fixture");
        // total_cmp: -0.0 < 0.0 < 1.0 < NaN; null in coda.
        let ascending = sorted_ids(
            &batch,
            &Sort {
                columns: vec!["num".into()],
                ascending: true,
            },
        );
        assert_eq!(ascending, vec![2, 0, 4, 1, 3]);
        // Discendente rovescia tutto: null in testa, poi NaN.
        let descending = sorted_ids(
            &batch,
            &Sort {
                columns: vec!["num".into()],
                ascending: false,
            },
        );
        assert_eq!(
            descending,
            expected_permutation(&values, false)
                .into_iter()
                .map(|row| i64::try_from(row).expect("id"))
                .collect::<Vec<_>>()
        );
        assert_eq!(descending, vec![3, 1, 4, 0, 2]);
    }

    #[test]
    fn sort_handles_dictionary_and_rejects_large_utf8_like_before() {
        let keys = DictionaryArray::<Int32Type>::new(
            plenora_core::arrow::array::Int32Array::from(vec![Some(1), Some(0), None, Some(1)]),
            Arc::new(StringArray::from(vec!["a", "b"])),
        );
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new(
                    "c",
                    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                    true,
                ),
            ])),
            vec![Arc::new(Int64Array::from(vec![0, 1, 2, 3])), Arc::new(keys)],
        )
        .expect("fixture");
        let sorted = sorted_ids(
            &batch,
            &Sort {
                columns: vec!["c".into()],
                ascending: true,
            },
        );
        assert_eq!(sorted, vec![1, 0, 3, 2]); // "a" < "b", null in coda

        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "c",
                DataType::LargeUtf8,
                true,
            )])),
            vec![Arc::new(LargeStringArray::from(vec![
                Some("b"),
                Some("a"),
                None,
            ]))],
        )
        .expect("fixture");
        // LargeUtf8 non e' nel profilo scalare: confrontando due valori non
        // nulli il percorso generico fallisce, errore di schema invariato.
        assert!(sort(
            &large,
            &Sort {
                columns: vec!["c".into()],
                ascending: true,
            }
        )
        .is_err());
    }

    #[test]
    fn sort_empty_single_row_and_mixed_columns() {
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "num",
                DataType::Float64,
                false,
            )])),
            vec![Arc::new(Float64Array::from(Vec::<f64>::new()))],
        )
        .expect("empty fixture");
        assert_eq!(
            sort(
                &empty,
                &Sort {
                    columns: vec!["num".into()],
                    ascending: true,
                }
            )
            .expect("empty sort")
            .num_rows(),
            0
        );
        assert!(sort(
            &empty,
            &Sort {
                columns: vec![],
                ascending: true,
            }
        )
        .is_err());

        let mixed = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("group", DataType::Utf8, false),
                Field::new("num", DataType::Float64, false),
                Field::new("flag", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3])),
                Arc::new(StringArray::from(vec!["b", "a", "b", "a"])),
                Arc::new(Float64Array::from(vec![2.0, 2.0, 1.0, 1.0])),
                Arc::new(plenora_core::arrow::array::BooleanArray::from(vec![
                    true, false, false, true,
                ])),
            ],
        )
        .expect("mixed fixture");
        let sorted = sorted_ids(
            &mixed,
            &Sort {
                columns: vec!["group".into(), "num".into()],
                ascending: true,
            },
        );
        assert_eq!(sorted, vec![3, 1, 2, 0]); // (a,1),(a,2),(b,1),(b,2)
                                              // Booleano: "false" < "true" come il confronto testuale.
        let by_flag = sorted_ids(
            &mixed,
            &Sort {
                columns: vec!["flag".into()],
                ascending: true,
            },
        );
        assert_eq!(by_flag, vec![1, 2, 0, 3]);
    }

    // -------------------------------------------------------------------
    // Test-oracolo di `top_n` (estensione v1.1): l'oracolo e' la coppia
    // `sort` + slice delle prime n posizioni, eseguita sul kernel `sort`
    // gia' validato. Il confronto e' sull'intero RecordBatch (schema,
    // valori, null mask), non solo sugli id.
    // -------------------------------------------------------------------

    fn oracle_sort_then_limit(batch: &RecordBatch, config: &TopN) -> RecordBatch {
        let sorted = sort(
            batch,
            &Sort {
                columns: config.columns.clone(),
                ascending: !config.descending,
            },
        )
        .expect("oracle sort");
        let n = usize::try_from(config.n).expect("n").min(sorted.num_rows());
        sorted.slice(0, n)
    }

    fn assert_top_n_matches_oracle(batch: &RecordBatch, config: &TopN) {
        let fast = top_n(batch, config).expect("top_n");
        let oracle = oracle_sort_then_limit(batch, config);
        assert_eq!(fast, oracle, "config: {config:?}");
    }

    fn numeric_batch(values: &[Option<f64>]) -> RecordBatch {
        let ids = (0..values.len())
            .map(|row| i64::try_from(row).expect("id"))
            .collect::<Vec<_>>();
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(Float64Array::from(values.to_vec())),
            ],
        )
        .expect("fixture")
    }

    #[test]
    fn top_n_matches_sort_plus_limit_on_duplicates_nan_signed_zero_and_nulls() {
        // Duplicati, NaN, -0.0, null: ogni n e ogni direzione contro l'oracolo.
        let values = vec![
            Some(0.0),
            Some(f64::NAN),
            Some(-0.0),
            None,
            Some(1.0),
            Some(1.0),
            Some(f64::NAN),
            None,
            Some(-0.0),
            Some(0.0),
        ];
        let batch = numeric_batch(&values);
        for n in [0_u64, 1, 3, 5, 9, 10, 11, 1_000] {
            for descending in [false, true] {
                assert_top_n_matches_oracle(
                    &batch,
                    &TopN {
                        columns: vec!["num".into()],
                        n,
                        descending,
                    },
                );
            }
        }
    }

    #[test]
    fn top_n_matches_oracle_on_duplicate_heavy_multi_column_input() {
        // Chiavi molto duplicate su due colonne (testo + numerico): la
        // stabilita' sullo spareggio d'indice deve riprodurre il sort.
        let rows = 2_000_usize;
        let ids = (0..rows)
            .map(|row| i64::try_from(row).expect("id"))
            .collect::<Vec<_>>();
        let groups = (0..rows)
            .map(|row| ["a", "b", "c"][row % 3].to_owned())
            .collect::<Vec<_>>();
        let nums = (0..rows)
            .map(|row| match row % 7 {
                0 => None,
                5 => Some(f64::NAN),
                _ => Some(f64::from(u32::try_from(row % 11).expect("key")) - 5.0),
            })
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("group", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(groups)),
                Arc::new(Float64Array::from(nums)),
            ],
        )
        .expect("fixture");
        for n in [1_u64, 7, 100, 2_000, 5_000] {
            for descending in [false, true] {
                assert_top_n_matches_oracle(
                    &batch,
                    &TopN {
                        columns: vec!["group".into(), "num".into()],
                        n,
                        descending,
                    },
                );
            }
        }
    }

    #[test]
    fn top_n_edge_cases_and_config_validation() {
        let batch = numeric_batch(&[Some(2.0), Some(1.0), None]);
        // n = 0: batch vuoto, schema invariato.
        let empty = top_n(
            &batch,
            &TopN {
                columns: vec!["num".into()],
                n: 0,
                descending: false,
            },
        )
        .expect("n=0");
        assert_eq!(empty.num_rows(), 0);
        assert_eq!(empty.schema(), batch.schema());
        // Errori: colonne vuote, colonna mancante, config non strict.
        assert!(top_n(
            &batch,
            &TopN {
                columns: vec![],
                n: 1,
                descending: false,
            },
        )
        .is_err());
        assert!(top_n(
            &batch,
            &TopN {
                columns: vec!["missing".into()],
                n: 1,
                descending: false,
            },
        )
        .is_err());
        let decoded: TopN = serde_json::from_value(serde_json::json!({"columns": ["num"], "n": 2}))
            .expect("default descending");
        assert!(!decoded.descending);
        assert!(serde_json::from_value::<TopN>(
            serde_json::json!({"columns": ["num"], "n": 2, "ascending": true})
        )
        .is_err());
        // Tipo non confrontabile (LargeUtf8): stesso errore di `sort`.
        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "c",
                DataType::LargeUtf8,
                true,
            )])),
            vec![Arc::new(LargeStringArray::from(vec![Some("b"), Some("a")]))],
        )
        .expect("fixture");
        assert!(top_n(
            &large,
            &TopN {
                columns: vec!["c".into()],
                n: 1,
                descending: false,
            },
        )
        .is_err());
    }

    // -------------------------------------------------------------------
    // Test-oracolo del fast path di `aggregate`: l'output dev'essere
    // byte-identico a quello dell'implementazione di riferimento qui
    // sotto, che non passa dal percorso ottimizzato.
    // -------------------------------------------------------------------

    /// Oracolo del fast path: implementazione di riferimento indipendente
    /// dello stesso contratto. Non passa dal percorso ottimizzato, cosi' una
    /// sua deviazione resta osservabile.
    #[allow(clippy::too_many_lines)]
    fn aggregate_reference(batch: &RecordBatch, config: &Aggregate) -> Result<RecordBatch> {
        let group_indices = config
            .group_by
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        if group_indices.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "aggregate richiede group_by".into(),
            ));
        }
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for row in 0..batch.num_rows() {
            groups
                .entry(row_key(batch, &group_indices, row)?)
                .or_default()
                .push(row);
        }
        let representatives = groups.values().map(|rows| rows[0]).collect::<Vec<_>>();
        let projected = select_rows(batch, &representatives)?;
        let group_columns = group_indices
            .iter()
            .map(|index| projected.column(*index).clone())
            .collect::<Vec<_>>();
        let group_fields = group_indices
            .iter()
            .map(|index| batch.schema().field(*index).as_ref().clone())
            .collect::<Vec<_>>();
        let mut result = RecordBatch::try_new(
            Arc::new(plenora_core::arrow::schema::Schema::new(group_fields)),
            group_columns,
        )?;
        if config.aggregations.is_empty() {
            let counts = groups
                .values()
                .map(|rows| aggregate::conteggio_gruppo(rows.len()))
                .collect::<Result<Vec<_>>>()?;
            return replace_or_append(
                &result,
                "count",
                DataType::Int64,
                false,
                Arc::new(Int64Array::from(counts)),
            );
        }
        let mut duplicate_names = HashMap::new();
        for aggregation in &config.aggregations {
            *duplicate_names
                .entry(&aggregation.column)
                .or_insert(0_usize) += 1;
        }
        for aggregation in &config.aggregations {
            let index = column_index(batch, &aggregation.column)?;
            let function_name = match aggregation.function {
                AggFunction::Count => "count",
                AggFunction::Sum => "sum",
                AggFunction::Avg | AggFunction::Mean => "mean",
                AggFunction::Min => "min",
                AggFunction::Max => "max",
                AggFunction::First => "first",
                AggFunction::Last => "last",
                AggFunction::Concat => "concat",
                AggFunction::Nunique => "nunique",
                AggFunction::Variance => "variance",
                AggFunction::Stddev => "stddev",
                AggFunction::Quantile => "quantile",
            };
            let name = if !aggregation.alias.is_empty() {
                aggregation.alias.clone()
            } else if duplicate_names[&aggregation.column] > 1 {
                format!("{}_{}", aggregation.column, function_name)
            } else {
                aggregation.column.clone()
            };
            validate_output_name(&name)?;
            match aggregation.function {
                AggFunction::Count | AggFunction::Nunique => {
                    let values = groups
                        .values()
                        .map(|rows| {
                            let count = if matches!(aggregation.function, AggFunction::Count) {
                                rows.iter()
                                    .filter(|row| {
                                        !crate::is_logically_null(
                                            batch.column(index).as_ref(),
                                            **row,
                                        )
                                    })
                                    .count()
                            } else {
                                let mut seen = HashSet::new();
                                for row in rows {
                                    if let Some(value) =
                                        scalar_as_string(batch.column(index).as_ref(), *row)?
                                    {
                                        seen.insert(format!("1{}:{value}", value.len()));
                                    } else if !aggregation.skip_null {
                                        seen.insert("0".to_owned());
                                    }
                                }
                                seen.len()
                            };
                            i64::try_from(count).map(Some).map_err(|_| {
                                PlenoraError::ResourceLimit("conteggio gruppo oltre i64".into())
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    result = replace_or_append(
                        &result,
                        &name,
                        DataType::Int64,
                        false,
                        Arc::new(Int64Array::from(values)),
                    )?;
                }
                AggFunction::Concat | AggFunction::First | AggFunction::Last => {
                    let values = groups
                        .values()
                        .map(|rows| {
                            if matches!(
                                aggregation.function,
                                AggFunction::First | AggFunction::Last
                            ) {
                                let row = if matches!(aggregation.function, AggFunction::First) {
                                    rows[0]
                                } else {
                                    *rows.last().unwrap_or(&rows[0])
                                };
                                return scalar_as_string(batch.column(index).as_ref(), row);
                            }
                            let mut seen = HashSet::new();
                            let mut values = Vec::new();
                            for row in rows {
                                if let Some(value) =
                                    scalar_as_string(batch.column(index).as_ref(), *row)?
                                {
                                    if !aggregation.distinct || seen.insert(value.clone()) {
                                        values.push(value);
                                    }
                                } else if !aggregation.skip_null {
                                    values.push(String::new());
                                }
                            }
                            Ok(Some(values.join(&aggregation.separator)))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    result = replace_or_append(
                        &result,
                        &name,
                        DataType::Utf8,
                        true,
                        Arc::new(StringArray::from(values)),
                    )?;
                }
                _ => {
                    let values = groups
                        .values()
                        .map(|rows| {
                            let raw = rows
                                .iter()
                                .map(|row| {
                                    scalar_as_f64_rounded(batch.column(index).as_ref(), *row)
                                })
                                .collect::<Result<Vec<_>>>()?;
                            if !aggregation.skip_null && raw.iter().any(Option::is_none) {
                                return Ok(None);
                            }
                            let mut values = raw.into_iter().flatten().collect::<Vec<_>>();
                            if aggregation.distinct {
                                values.sort_by(f64::total_cmp);
                                values.dedup_by(|left, right| {
                                    left.total_cmp(right) == Ordering::Equal
                                });
                            }
                            if values.is_empty() {
                                return Ok(None);
                            }
                            let sum: f64 = values.iter().sum();
                            Ok(Some(match aggregation.function {
                                AggFunction::Sum => sum,
                                AggFunction::Avg | AggFunction::Mean => {
                                    sum / values.len().to_f64().ok_or_else(|| {
                                        PlenoraError::InvalidPlan(
                                            "dimensione gruppo non rappresentabile".into(),
                                        )
                                    })?
                                }
                                AggFunction::Min => {
                                    values.iter().copied().reduce(f64::min).unwrap_or_default()
                                }
                                AggFunction::Max => {
                                    values.iter().copied().reduce(f64::max).unwrap_or_default()
                                }
                                AggFunction::Variance | AggFunction::Stddev => {
                                    if values.len() <= aggregation.ddof {
                                        return Ok(None);
                                    }
                                    let length = values.len().to_f64().ok_or_else(|| {
                                        PlenoraError::InvalidPlan(
                                            "dimensione gruppo non rappresentabile".into(),
                                        )
                                    })?;
                                    let mean = sum / length;
                                    let divisor = (values.len() - aggregation.ddof)
                                        .to_f64()
                                        .ok_or_else(|| {
                                            PlenoraError::InvalidPlan(
                                                "divisore statistico non rappresentabile".into(),
                                            )
                                        })?;
                                    let variance = values
                                        .iter()
                                        .map(|value| (value - mean).powi(2))
                                        .sum::<f64>()
                                        / divisor;
                                    if matches!(aggregation.function, AggFunction::Stddev) {
                                        variance.sqrt()
                                    } else {
                                        variance
                                    }
                                }
                                AggFunction::Quantile => {
                                    let quantile = aggregation.quantile.ok_or_else(|| {
                                        PlenoraError::InvalidPlan(
                                            "quantile richiede il parametro quantile".into(),
                                        )
                                    })?;
                                    values.sort_by(f64::total_cmp);
                                    let last = (values.len() - 1).to_f64().ok_or_else(|| {
                                        PlenoraError::InvalidPlan(
                                            "dimensione quantile non rappresentabile".into(),
                                        )
                                    })?;
                                    let position = quantile * last;
                                    let lower = position.floor().to_usize().ok_or_else(|| {
                                        PlenoraError::InvalidPlan(
                                            "indice quantile non valido".into(),
                                        )
                                    })?;
                                    let upper = position.ceil().to_usize().ok_or_else(|| {
                                        PlenoraError::InvalidPlan(
                                            "indice quantile non valido".into(),
                                        )
                                    })?;
                                    let weight = position - position.floor();
                                    // Niente mul_add/FMA: forma non fusa
                                    // (contratto numerico, architettura.md#determinismo) — la
                                    // STESSA della produzione, equivalenza
                                    // bit-a-bit per costruzione.
                                    #[allow(clippy::suboptimal_flops)]
                                    let interpolated =
                                        (values[upper] - values[lower]) * weight + values[lower];
                                    interpolated
                                }
                                _ => unreachable!(),
                            }))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    result = replace_or_append(
                        &result,
                        &name,
                        DataType::Float64,
                        true,
                        Arc::new(Float64Array::from(values)),
                    )?;
                }
            }
        }
        Ok(result)
    }

    /// Confronto rigoroso: schema (nomi, tipi, nullabilita'), numero righe,
    /// maschera null e valori (bit a bit per i Float64, NaN incluso).
    fn assert_batches_identical(fast: &RecordBatch, reference: &RecordBatch) {
        assert_eq!(fast.num_rows(), reference.num_rows(), "righe");
        assert_eq!(fast.num_columns(), reference.num_columns(), "colonne");
        let fast_schema = fast.schema();
        let reference_schema = reference.schema();
        for index in 0..fast.num_columns() {
            let fast_field = fast_schema.field(index);
            let reference_field = reference_schema.field(index);
            assert_eq!(
                fast_field.name(),
                reference_field.name(),
                "nome colonna {index}"
            );
            assert_eq!(
                fast_field.data_type(),
                reference_field.data_type(),
                "tipo colonna {}",
                fast_field.name()
            );
            assert_eq!(
                fast_field.is_nullable(),
                reference_field.is_nullable(),
                "nullabilita' colonna {}",
                fast_field.name()
            );
            let fast_column = fast.column(index);
            let reference_column = reference.column(index);
            for row in 0..fast.num_rows() {
                assert_eq!(
                    fast_column.is_null(row),
                    reference_column.is_null(row),
                    "null riga {row} colonna {}",
                    fast_field.name()
                );
            }
            match fast_field.data_type() {
                DataType::Float64 => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("float64");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("float64");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row).to_bits(),
                                reference_values.value(row).to_bits(),
                                "bits riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                DataType::Int64 => {
                    let fast_values = fast_column
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .expect("int64");
                    let reference_values = reference_column
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .expect("int64");
                    for row in 0..fast.num_rows() {
                        if !fast_values.is_null(row) {
                            assert_eq!(
                                fast_values.value(row),
                                reference_values.value(row),
                                "valore riga {row} colonna {}",
                                fast_field.name()
                            );
                        }
                    }
                }
                _ => {
                    // Chiavi di gruppo di altri tipi (Utf8, UInt64, Boolean,
                    // Date32, ...): confronto sulla forma scalare, che per
                    // questi tipi e' iniettiva sui valori.
                    for row in 0..fast.num_rows() {
                        assert_eq!(
                            scalar_as_string(fast_column.as_ref(), row).expect("scalare"),
                            scalar_as_string(reference_column.as_ref(), row).expect("scalare"),
                            "valore riga {row} colonna {}",
                            fast_field.name()
                        );
                    }
                }
            }
        }
    }

    /// Esegue fast path e riferimento e verifica l'uguaglianza esatta;
    /// il fast path viene eseguito due volte (determinismo assoluto).
    fn assert_aggregate_parity(batch: &RecordBatch, config: &Aggregate) {
        let reference = aggregate_reference(batch, config).expect("riferimento");
        let fast = aggregate(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = aggregate(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    fn agg(column: &str, function: AggFunction) -> Aggregation {
        Aggregation {
            column: column.into(),
            function,
            separator: ", ".into(),
            distinct: false,
            skip_null: true,
            alias: String::new(),
            quantile: None,
            ddof: 1,
        }
    }

    #[test]
    fn quantile_fuori_range_rifiutato_prima_dei_dati() {
        // Regressione: un quantile > 1.0 darebbe un indice oltre il gruppo
        // ordinato — panic out-of-bounds nel percorso lib, invisibile al
        // gate R6 perche' e' un'indicizzazione e non una primitiva
        // esplicita. Il range e' validato fail-closed prima di toccare i
        // dati.
        let batch = numeric_batch(&[Some(1.0), Some(2.0), Some(3.0)]);
        for quantile in [-0.5, 1.5, f64::NAN, f64::INFINITY] {
            let config = Aggregate {
                group_by: vec!["id".into()],
                aggregations: vec![Aggregation {
                    quantile: Some(quantile),
                    ..agg("num", AggFunction::Quantile)
                }],
            };
            let result = aggregate(&batch, &config);
            let Err(PlenoraError::InvalidPlan(message)) = &result else {
                panic!("quantile {quantile}: atteso rifiuto fuori range, ottenuto {result:?}");
            };
            assert!(
                message.contains("0..=1"),
                "quantile {quantile}: messaggio inatteso: {message}"
            );
        }
        // I bordi 0.0 e 1.0 restano validi.
        let config = Aggregate {
            group_by: vec!["id".into()],
            aggregations: [0.0, 1.0]
                .iter()
                .map(|quantile| Aggregation {
                    quantile: Some(*quantile),
                    ..agg("num", AggFunction::Quantile)
                })
                .collect(),
        };
        aggregate(&batch, &config).expect("bordi 0 e 1 validi");
    }

    #[test]
    fn quantile_senza_parametro_rifiutato_in_riduzione() {
        // Il range e' validato a monte solo se il parametro e' presente;
        // `quantile` assente e' rifiutato fail-closed dalla riduzione.
        let batch = numeric_batch(&[Some(1.0), Some(2.0), Some(3.0)]);
        let config = Aggregate {
            group_by: vec!["id".into()],
            aggregations: vec![agg("num", AggFunction::Quantile)],
        };
        let result = aggregate(&batch, &config);
        let Err(PlenoraError::InvalidPlan(message)) = &result else {
            panic!("atteso rifiuto per quantile senza parametro, ottenuto {result:?}");
        };
        assert!(
            message.contains("richiede il parametro quantile"),
            "messaggio inatteso: {message}"
        );
    }

    #[test]
    fn aggregate_distinct_su_tutte_le_riduzioni_numeriche_materializzate() {
        // `distinct` (e `quantile`) percorrono la riduzione materializzata:
        // parita' bit-a-bit con l'oracolo per ogni funzione numerica.
        let batch = mixed_fixture();
        let distinct = |function| Aggregation {
            distinct: true,
            ..agg("num", function)
        };
        let config = Aggregate {
            group_by: vec!["g".into()],
            aggregations: vec![
                Aggregation {
                    alias: "d_avg".into(),
                    ..distinct(AggFunction::Avg)
                },
                Aggregation {
                    alias: "d_mean".into(),
                    ..distinct(AggFunction::Mean)
                },
                Aggregation {
                    alias: "d_min".into(),
                    ..distinct(AggFunction::Min)
                },
                Aggregation {
                    alias: "d_max".into(),
                    ..distinct(AggFunction::Max)
                },
                Aggregation {
                    alias: "d_variance".into(),
                    ..distinct(AggFunction::Variance)
                },
                Aggregation {
                    alias: "d_stddev".into(),
                    ..distinct(AggFunction::Stddev)
                },
            ],
        };
        assert_aggregate_parity(&batch, &config);
        // Gruppo con un solo valore distinto: len <= ddof -> null
        // (varianza campionaria non definita), come l'oracolo.
        let batch = numeric_batch(&[Some(7.0), Some(7.0), Some(7.0)]);
        let single = Aggregate {
            group_by: vec!["id".into()],
            aggregations: vec![
                Aggregation {
                    distinct: true,
                    alias: "dv".into(),
                    ..agg("num", AggFunction::Variance)
                },
                Aggregation {
                    distinct: true,
                    alias: "ds".into(),
                    ..agg("num", AggFunction::Stddev)
                },
                Aggregation {
                    alias: "v".into(),
                    ..agg("num", AggFunction::Variance)
                },
                Aggregation {
                    alias: "s".into(),
                    ..agg("num", AggFunction::Stddev)
                },
            ],
        };
        assert_aggregate_parity(&batch, &single);
    }

    #[test]
    fn aggregate_chiave_uint64_singola_usa_il_fast_path_nativo() {
        // Chiave di gruppo UInt64 (valori oltre i64::MAX e null): fast path
        // nativo con ordine u64, parita' con l'oracolo testuale.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("u", DataType::UInt64, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(UInt64Array::from(vec![
                    Some(u64::MAX),
                    Some(10),
                    None,
                    Some(9),
                    Some(u64::MAX),
                    None,
                    Some(10),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(2.0),
                    Some(3.0),
                    Some(4.0),
                    None,
                    Some(5.0),
                    Some(6.0),
                ])),
            ],
        )
        .expect("fixture uint64");
        let config = Aggregate {
            group_by: vec!["u".into()],
            aggregations: vec![
                agg("num", AggFunction::Sum),
                agg("num", AggFunction::Count),
                agg("num", AggFunction::Nunique),
                Aggregation {
                    quantile: Some(0.5),
                    ..agg("num", AggFunction::Quantile)
                },
            ],
        };
        assert_aggregate_parity(&batch, &config);
    }

    /// Batteria completa di aggregazioni su `num`/`val`/`txt`, con duplicati
    /// di colonna (nomi `{colonna}_{funzione}`) e varianti `skip_null`/`distinct`.
    fn full_aggregations() -> Vec<Aggregation> {
        vec![
            agg("num", AggFunction::Sum),
            agg("num", AggFunction::Mean),
            agg("num", AggFunction::Min),
            agg("num", AggFunction::Max),
            agg("num", AggFunction::Variance),
            agg("num", AggFunction::Stddev),
            Aggregation {
                quantile: Some(0.3),
                ..agg("num", AggFunction::Quantile)
            },
            agg("num", AggFunction::Count),
            agg("num", AggFunction::Nunique),
            Aggregation {
                skip_null: false,
                ..agg("num", AggFunction::Mean)
            },
            Aggregation {
                distinct: true,
                ..agg("num", AggFunction::Sum)
            },
            agg("val", AggFunction::Sum),
            agg("val", AggFunction::Nunique),
            agg("txt", AggFunction::First),
            agg("txt", AggFunction::Last),
            Aggregation {
                distinct: true,
                ..agg("txt", AggFunction::Concat)
            },
        ]
    }

    /// Fixture mista: chiavi Utf8 con null, Float64 con NaN/-0.0/null/inf,
    /// Int64 con negativi e null, Utf8 con duplicati e null.
    fn mixed_fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("b"),
                    None,
                    Some("a"),
                    Some("b"),
                    None,
                    Some("a"),
                    Some("b"),
                    Some("a"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(f64::NAN),
                    Some(-0.0),
                    None,
                    Some(0.0),
                    Some(2.5),
                    Some(f64::INFINITY),
                    Some(f64::NAN),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(10),
                    None,
                    Some(-3),
                    Some(7),
                    Some(10),
                    Some(-3),
                    Some(0),
                    Some(42),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("x"),
                    Some("y"),
                    None,
                    Some("x"),
                    Some("z"),
                    Some("y"),
                    None,
                    Some("w"),
                ])),
            ],
        )
        .expect("fixture mista")
    }

    #[test]
    fn aggregate_fast_path_matches_reference_on_mixed_float() {
        let batch = mixed_fixture();
        let config = Aggregate {
            group_by: vec!["g".into()],
            aggregations: full_aggregations(),
        };
        assert_aggregate_parity(&batch, &config);
    }

    #[test]
    fn aggregate_fast_path_matches_reference_on_numeric_and_bool_keys() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("ki", DataType::Int64, true),
                Field::new("ku", DataType::UInt64, true),
                Field::new("kb", DataType::Boolean, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(-5),
                    Some(10),
                    None,
                    Some(-5),
                    Some(2),
                    Some(10),
                ])),
                Arc::new(UInt64Array::from(vec![
                    Some(9),
                    Some(10),
                    Some(9),
                    None,
                    Some(2),
                    Some(10),
                ])),
                Arc::new(plenora_core::arrow::array::BooleanArray::from(vec![
                    Some(true),
                    None,
                    Some(false),
                    Some(true),
                    Some(false),
                    None,
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(3.25),
                    Some(-0.0),
                    Some(0.0),
                    None,
                    Some(f64::NAN),
                    Some(1.0),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(-1),
                    Some(2),
                    None,
                    Some(-1),
                    Some(5),
                    Some(2),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("aa"),
                    Some("b"),
                    Some("aa"),
                    None,
                    Some("c"),
                    Some("b"),
                ])),
            ],
        )
        .expect("fixture chiavi numeriche");
        // Chiave singola Int64 (ordine chiavi per lunghezza stringa: -5, 2,
        // 10 con null in testa).
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["ki".into()],
                aggregations: full_aggregations(),
            },
        );
        // Chiavi miste multi-colonna: UInt64 + Boolean + Int64.
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["ku".into(), "kb".into(), "ki".into()],
                aggregations: full_aggregations(),
            },
        );
    }

    #[test]
    fn aggregate_fast_path_matches_reference_on_float_group_keys() {
        // Chiavi Float64: NaN collide con NaN nella chiave, -0.0 e 0.0 sono
        // chiavi distinte ("-0" vs "0").
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Float64, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![
                    Some(-0.0),
                    Some(0.0),
                    Some(f64::NAN),
                    None,
                    Some(f64::NAN),
                    Some(1.5),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.0),
                    Some(2.0),
                    Some(3.0),
                    Some(4.0),
                    Some(f64::NAN),
                    None,
                ])),
                Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5, 6])),
                Arc::new(StringArray::from(vec!["a", "b", "c", "d", "e", "f"])),
            ],
        )
        .expect("fixture chiavi float");
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );
    }

    #[test]
    fn aggregate_fast_path_matches_reference_on_empty_and_single_group() {
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
                Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
            ],
        )
        .expect("fixture vuota");
        assert_aggregate_parity(
            &empty,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );

        // Gruppo singolo: tutte le righe con la stessa chiave, con e senza
        // aggregazioni esplicite (ramo `count` implicito).
        let single = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["k", "k", "k"])),
                Arc::new(Float64Array::from(vec![Some(-0.0), None, Some(f64::NAN)])),
                Arc::new(Int64Array::from(vec![Some(4), Some(-2), None])),
                Arc::new(StringArray::from(vec![Some("s"), None, Some("s")])),
            ],
        )
        .expect("fixture gruppo singolo");
        assert_aggregate_parity(
            &single,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );
        assert_aggregate_parity(
            &single,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: vec![],
            },
        );
    }

    /// Fixture a scala: sopra la soglia parallela (`32_768` righe) e, con
    /// `groups` grande, sopra la soglia anche per l'ordinamento chiavi.
    fn scale_fixture(rows: usize, groups: i64) -> RecordBatch {
        let keys = (0..rows)
            .map(|row| {
                if row % 17 == 0 {
                    None
                } else {
                    let row = i64::try_from(row).expect("riga");
                    Some((row * 7_919) % groups)
                }
            })
            .collect::<Vec<_>>();
        let nums = (0..rows)
            .map(|row| {
                if row % 7 == 0 {
                    None
                } else if row % 13 == 0 {
                    Some(f64::NAN)
                } else if row % 11 == 0 {
                    Some(-0.0)
                } else {
                    Some(f64::from(u32::try_from(row % 10_000).expect("valore")) * 0.5)
                }
            })
            .collect::<Vec<_>>();
        let ints = (0..rows)
            .map(|row| {
                if row % 5 == 0 {
                    None
                } else {
                    let row = i64::try_from(row).expect("riga");
                    Some(row % 1_000 - 500)
                }
            })
            .collect::<Vec<_>>();
        let texts = (0..rows)
            .map(|row| {
                if row % 23 == 0 {
                    None
                } else {
                    Some(format!("t{}", row % 3_000))
                }
            })
            .collect::<Vec<_>>();
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Int64, true),
                Field::new("num", DataType::Float64, true),
                Field::new("val", DataType::Int64, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(keys)),
                Arc::new(Float64Array::from(nums)),
                Arc::new(Int64Array::from(ints)),
                Arc::new(StringArray::from(texts)),
            ],
        )
        .expect("fixture a scala")
    }

    #[test]
    fn aggregate_fast_path_matches_reference_at_parallel_scale_small_groups() {
        // 70_000 righe, 100 gruppi: calcolo gruppi in parallelo, chiavi
        // ordinate in sequenziale.
        let batch = scale_fixture(70_000, 100);
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );
    }

    #[test]
    fn aggregate_fast_path_matches_reference_at_parallel_scale_large_groups() {
        // 70_000 righe, 35_000 gruppi: anche l'ordinamento chiavi va in
        // parallelo (sopra soglia).
        let batch = scale_fixture(70_000, 35_000);
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["g".into()],
                aggregations: full_aggregations(),
            },
        );
    }

    #[test]
    fn aggregate_fast_path_preserves_errors() {
        // Aggregazione numerica su Utf8 non numerico: stesso errore.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("txt", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["a", "b"])),
                Arc::new(StringArray::from(vec!["non-numero", "xyz"])),
            ],
        )
        .expect("fixture errori");
        let config = Aggregate {
            group_by: vec!["g".into()],
            aggregations: vec![agg("txt", AggFunction::Sum)],
        };
        let fast_error = aggregate(&batch, &config).expect_err("fast path errore");
        let reference_error = aggregate_reference(&batch, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());

        // Chiave di gruppo LargeUtf8: il percorso generico delle chiavi
        // fallisce nello stesso modo (tipo fuori profilo scalare).
        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::LargeUtf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(LargeStringArray::from(vec![Some("a"), Some("b")])),
                Arc::new(Float64Array::from(vec![1.0, 2.0])),
            ],
        )
        .expect("fixture large utf8");
        let config = Aggregate {
            group_by: vec!["g".into()],
            aggregations: vec![agg("num", AggFunction::Sum)],
        };
        let fast_error = aggregate(&large, &config).expect_err("fast path errore");
        let reference_error = aggregate_reference(&large, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());

        // group_by vuoto: errore di contratto invariato.
        let empty_group = Aggregate {
            group_by: vec![],
            aggregations: vec![],
        };
        assert!(aggregate(&batch, &empty_group).is_err());
    }

    #[test]
    fn aggregate_fast_path_matches_reference_with_alias_and_date_keys() {
        // Chiavi Date32 (percorso generico delle chiavi) + alias espliciti.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("d", DataType::Date32, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(plenora_core::arrow::array::Date32Array::from(vec![
                    Some(19_000),
                    None,
                    Some(19_000),
                    Some(-1),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.0),
                    Some(2.0),
                    Some(3.0),
                    Some(4.0),
                ])),
            ],
        )
        .expect("fixture date");
        assert_aggregate_parity(
            &batch,
            &Aggregate {
                group_by: vec!["d".into()],
                aggregations: vec![
                    Aggregation {
                        alias: "totale".into(),
                        ..agg("num", AggFunction::Sum)
                    },
                    Aggregation {
                        alias: "media".into(),
                        ..agg("num", AggFunction::Mean)
                    },
                ],
            },
        );
    }

    /// Chiave testuale di `row_key` per un valore Int64 non null.
    fn i64_text_key(value: i64) -> String {
        let text = value.to_string();
        format!("Int64\u{1e}1{}:{text}\u{1f}", text.len())
    }

    fn u64_text_key(value: u64) -> String {
        let text = value.to_string();
        format!("UInt64\u{1e}1{}:{text}\u{1f}", text.len())
    }

    fn str_text_key(value: &str) -> String {
        format!("Utf8\u{1e}1{}:{value}\u{1f}", value.len())
    }

    #[test]
    // Batteria di confini sequenziale: la lunghezza e' nel numero di casi.
    #[allow(clippy::too_many_lines)]
    fn native_group_key_order_matches_text_key_order() {
        // Confini di lunghezza decimale, segni, estremi di dominio.
        let edge_i64 = [
            0,
            1,
            -1,
            5,
            -5,
            9,
            10,
            -9,
            -10,
            42,
            -42,
            99,
            100,
            -99,
            -100,
            999,
            1000,
            999_999_999,
            1_000_000_000,
            -999_999_999,
            -1_000_000_000,
            i64::MAX,
            i64::MAX - 1,
            i64::MIN,
            i64::MIN + 1,
        ];
        // Sweep pseudocasuale deterministico (seed 42): meta' valori pieni,
        // meta' corti per battere i confini di lunghezza.
        let mut state = 42_u64;
        let mut samples_i64 = Vec::new();
        for _ in 0..4_096 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let value = (state >> 16).cast_signed();
            samples_i64.push(if state & 1 == 0 {
                value % 2_001 - 1_000
            } else {
                value
            });
        }
        for &a in &samples_i64 {
            for &b in edge_i64.iter().chain(samples_i64.iter().take(64)) {
                assert_eq!(
                    cmp_i64_group_key(a, b),
                    i64_text_key(a).cmp(&i64_text_key(b)),
                    "i64 {a} vs {b}"
                );
            }
        }
        for &a in &edge_i64 {
            for &b in &edge_i64 {
                assert_eq!(
                    cmp_i64_group_key(a, b),
                    i64_text_key(a).cmp(&i64_text_key(b)),
                    "i64 edge {a} vs {b}"
                );
            }
        }

        let unsigned_edges = [
            0,
            1,
            9,
            10,
            99,
            100,
            999,
            1000,
            999_999_999,
            1_000_000_000,
            u64::MAX,
            u64::MAX - 1,
        ];
        for &a in &unsigned_edges {
            for &b in &unsigned_edges {
                assert_eq!(
                    cmp_u64_group_key(a, b),
                    u64_text_key(a).cmp(&u64_text_key(b)),
                    "u64 {a} vs {b}"
                );
            }
        }

        // Stringhe: confini 9/10/11 e 99/100/101 byte, prefissi comuni,
        // multibyte (la lunghezza della chiave e' in byte).
        let mut text_edges: Vec<String> = vec![
            String::new(),
            "a".into(),
            "ab".into(),
            "abc".into(),
            "abd".into(),
            "é".into(),
            "🚀".into(),
        ];
        for len in [8_usize, 9, 10, 11, 12, 98, 99, 100, 101] {
            text_edges.push("x".repeat(len));
            text_edges.push("y".repeat(len));
        }
        for a in &text_edges {
            for b in &text_edges {
                assert_eq!(
                    cmp_str_group_key(a, b),
                    str_text_key(a).cmp(&str_text_key(b)),
                    "str {a:?} vs {b:?}"
                );
            }
        }
    }

    // -------------------------------------------------------------------
    // Test-oracolo di `distinct`/`dedup_advanced`/`window_function`/
    // `rolling_window`: output byte-identico alle implementazioni di
    // riferimento qui sotto, indipendenti dal percorso ottimizzato.
    // -------------------------------------------------------------------

    /// Oracolo indipendente di `distinct`: stesso contratto, percorso
    /// diverso.
    fn distinct_reference(batch: &RecordBatch, config: &Distinct) -> Result<RecordBatch> {
        let indices = if config.subset.is_empty() {
            (0..batch.num_columns()).collect()
        } else {
            config
                .subset
                .iter()
                .map(|name| column_index(batch, name))
                .collect::<Result<Vec<_>>>()?
        };
        let keys = (0..batch.num_rows())
            .map(|row| row_key(batch, &indices, row))
            .collect::<Result<Vec<_>>>()?;
        let mut counts = HashMap::new();
        for key in &keys {
            *counts.entry(key).or_insert(0_usize) += 1;
        }
        let mut seen = HashSet::new();
        let mut last = HashMap::new();
        for (row, key) in keys.iter().enumerate() {
            last.insert(key, row);
        }
        let rows = keys
            .iter()
            .enumerate()
            .filter_map(|(row, key)| match config.keep {
                Keep::First if seen.insert(key) => Some(row),
                Keep::Last if last.get(key) == Some(&row) => Some(row),
                Keep::False if counts.get(key) == Some(&1) => Some(row),
                _ => None,
            })
            .collect::<Vec<_>>();
        select_rows(batch, &rows)
    }

    /// Oracolo indipendente di `dedup_advanced`: compone `sort` e `distinct`
    /// invece di attraversare il percorso ottimizzato dell'operazione.
    fn dedup_advanced_reference(
        batch: &RecordBatch,
        config: &DedupAdvanced,
    ) -> Result<RecordBatch> {
        let ordered = if let Some(column) = &config.order_column {
            sort(
                batch,
                &Sort {
                    columns: vec![column.clone()],
                    ascending: config.ascending,
                },
            )?
        } else {
            batch.clone()
        };
        distinct_reference(
            &ordered,
            &Distinct {
                subset: config.subset.clone(),
                keep: match config.keep {
                    Keep::First => Keep::First,
                    Keep::Last => Keep::Last,
                    Keep::False => {
                        return Err(PlenoraError::InvalidPlan(
                            "dedup_advanced non supporta keep=false".into(),
                        ))
                    }
                },
            },
        )
    }

    /// Oracolo indipendente di `rolling_window`: ricostruisce la finestra in
    /// un `Vec` a ogni riga, invece di attraversare il percorso ottimizzato.
    fn rolling_window_reference(
        batch: &RecordBatch,
        config: &RollingWindow,
    ) -> Result<RecordBatch> {
        if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
            return Err(PlenoraError::InvalidPlan(
                "rolling_window: finestra non valida".into(),
            ));
        }
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
        let source = column_index(&ordered, &config.column)?;
        let group = config
            .group_by
            .as_deref()
            .map(|name| column_index(&ordered, name))
            .transpose()?;
        let mut partitions: BTreeMap<Option<String>, Vec<usize>> = BTreeMap::new();
        for row in 0..ordered.num_rows() {
            let key = group
                .map(|index| scalar_as_string(ordered.column(index).as_ref(), row))
                .transpose()?
                .flatten();
            partitions.entry(key).or_default().push(row);
        }
        let mut output = vec![None; ordered.num_rows()];
        for rows in partitions.values() {
            let numbers = rows
                .iter()
                .map(|row| scalar_as_f64_rounded(ordered.column(source).as_ref(), *row))
                .collect::<Result<Vec<_>>>()?;
            for (position, row) in rows.iter().enumerate() {
                let start = (position + 1).saturating_sub(config.window);
                let values = numbers[start..=position]
                    .iter()
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>();
                if values.len() < config.min_periods {
                    continue;
                }
                let result = match config.function {
                    RollingKind::Sum => Some(values.iter().sum()),
                    RollingKind::Mean => values
                        .len()
                        .to_f64()
                        .map(|length| values.iter().sum::<f64>() / length),
                    RollingKind::Min => values.iter().copied().reduce(f64::min),
                    RollingKind::Max => values.iter().copied().reduce(f64::max),
                    RollingKind::Stddev if values.len() <= config.ddof => None,
                    RollingKind::Stddev => {
                        let length = values.len().to_f64().ok_or_else(|| {
                            PlenoraError::ResourceLimit(
                                "dimensione rolling non rappresentabile".into(),
                            )
                        })?;
                        let mean = values.iter().sum::<f64>() / length;
                        let divisor = (values.len() - config.ddof).to_f64().ok_or_else(|| {
                            PlenoraError::ResourceLimit(
                                "divisore rolling non rappresentabile".into(),
                            )
                        })?;
                        Some(
                            (values
                                .iter()
                                .map(|value| (value - mean).powi(2))
                                .sum::<f64>()
                                / divisor)
                                .sqrt(),
                        )
                    }
                };
                output[*row] = result;
            }
        }
        replace_or_append(
            &ordered,
            &config.output_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(output)),
        )
    }

    /// Oracolo indipendente di `window_function`: partiziona in un
    /// `BTreeMap` su `Option<String>`, invece di attraversare il percorso
    /// ottimizzato.
    #[allow(clippy::too_many_lines)]
    fn window_function_reference(
        batch: &RecordBatch,
        config: &WindowFunction,
    ) -> Result<RecordBatch> {
        if config.offset == 0 {
            return Err(PlenoraError::InvalidPlan(
                "offset deve essere positivo".into(),
            ));
        }
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
        // Il riferimento incarna il contratto, e il contratto dice che le
        // funzioni di rango non ordinano il testo numerico: non ha un ordine
        // esatto, e interpretarlo come double renderebbe a pari merito numeri
        // distinti. Scritto qui in modo indipendente, non chiamando il
        // kernel.
        if matches!(
            config.function,
            WindowKind::Rank
                | WindowKind::DenseRank
                | WindowKind::PercentRank
                | WindowKind::CumeDist
        ) && batch.column(source_index).data_type() == &DataType::Utf8
        {
            return Err(PlenoraError::Schema(
                "il testo numerico non ha un ordine esatto: le funzioni di rango non lo accettano"
                    .to_owned(),
            ));
        }
        // Per i tipi nativi il riferimento ordina ancora confrontando `f64`:
        // resta un oracolo valido finche' i valori stanno nell'intervallo
        // esattamente rappresentabile, che e' dove vive la fixture. Fuori da
        // li' l'oracolo e' `tests/arrotondamento_float64.rs`, con attese
        // letterali.
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
        let mut partitions: BTreeMap<Option<String>, Vec<usize>> = BTreeMap::new();
        for row in 0..ordered.num_rows() {
            let key = group_index
                .map(|index| scalar_as_string(ordered.column(index).as_ref(), row))
                .transpose()?
                .flatten();
            partitions.entry(key).or_default().push(row);
        }
        let mut output = vec![None; ordered.num_rows()];
        for rows in partitions.values() {
            let numbers = rows
                .iter()
                .map(|row| scalar_as_f64_rounded(ordered.column(source_index).as_ref(), *row))
                .collect::<Result<Vec<_>>>()?;
            let mut sorted = numbers.iter().flatten().copied().collect::<Vec<_>>();
            sorted.sort_by(f64::total_cmp);
            let mut dense = sorted.clone();
            dense.dedup_by(|left, right| left.total_cmp(right) == Ordering::Equal);
            let mut sum = 0.0;
            let mut count = 0.0_f64;
            for (position, row) in rows.iter().enumerate() {
                output[*row] = match config.function {
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
                        .checked_sub(config.offset)
                        .and_then(|other| numbers[other]),
                    WindowKind::Lead => numbers.get(position + config.offset).copied().flatten(),
                    WindowKind::PctChange => position
                        .checked_sub(1)
                        .and_then(|previous| numbers[previous])
                        .and_then(|previous| {
                            numbers[position]
                                .filter(|_| previous != 0.0)
                                .map(|current| (current - previous) / previous)
                        }),
                    WindowKind::Rank | WindowKind::DenseRank => {
                        numbers[position].and_then(|current| {
                            if matches!(config.function, WindowKind::DenseRank) {
                                dense
                                    .binary_search_by(|value| value.total_cmp(&current))
                                    .ok()
                                    .and_then(|index| (index + 1).to_f64())
                            } else {
                                let first = sorted
                                    .partition_point(|value| value.total_cmp(&current).is_lt());
                                let last = sorted
                                    .partition_point(|value| !value.total_cmp(&current).is_gt())
                                    .checked_sub(1)?;
                                (first + last + 2).to_f64().map(|sum| sum / 2.0)
                            }
                        })
                    }
                    WindowKind::PercentRank => numbers[position].and_then(|current| {
                        if sorted.len() <= 1 {
                            return Some(0.0);
                        }
                        let rank =
                            sorted.partition_point(|value| value.total_cmp(&current).is_lt());
                        let numerator = rank.to_f64()?;
                        let denominator = (sorted.len() - 1).to_f64()?;
                        Some(numerator / denominator)
                    }),
                    WindowKind::CumeDist => numbers[position].and_then(|current| {
                        let last = sorted
                            .partition_point(|value| !value.total_cmp(&current).is_gt())
                            .checked_sub(1)?;
                        let numerator = (last + 1).to_f64()?;
                        let denominator = sorted.len().to_f64()?;
                        Some(numerator / denominator)
                    }),
                    WindowKind::Ntile => {
                        let buckets = config.buckets.unwrap_or(1);
                        let effective = buckets.min(rows.len());
                        position
                            .checked_mul(effective)
                            .and_then(|value| value.checked_div(rows.len()))
                            .and_then(|value| (value + 1).to_f64())
                    }
                };
            }
        }
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

    fn assert_distinct_parity(batch: &RecordBatch, config: &Distinct) {
        let reference = distinct_reference(batch, config).expect("riferimento");
        let fast = distinct(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = distinct(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    fn assert_dedup_parity(batch: &RecordBatch, config: &DedupAdvanced) {
        let reference = dedup_advanced_reference(batch, config).expect("riferimento");
        let fast = dedup_advanced(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = dedup_advanced(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    fn assert_rolling_parity(batch: &RecordBatch, config: &RollingWindow) {
        let reference = rolling_window_reference(batch, config).expect("riferimento");
        let fast = rolling_window(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = rolling_window(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    fn assert_window_parity(batch: &RecordBatch, config: &WindowFunction) {
        let reference = window_function_reference(batch, config).expect("riferimento");
        let fast = window_function(batch, config).expect("fast path");
        assert_batches_identical(&fast, &reference);
        let fast_again = window_function(batch, config).expect("fast path bis");
        assert_batches_identical(&fast_again, &reference);
    }

    #[test]
    fn distinct_matches_reference_on_mixed_fixture_all_keeps_and_subsets() {
        let batch = mixed_fixture();
        let subsets: Vec<Vec<String>> = vec![
            vec![],
            vec!["g".into()],
            vec!["num".into()],
            vec!["val".into()],
            vec!["txt".into()],
            vec!["g".into(), "val".into()],
            // Colonna ripetuta nel subset: la chiave la include due volte.
            vec!["g".into(), "g".into()],
        ];
        for subset in &subsets {
            for keep in [Keep::First, Keep::Last, Keep::False] {
                assert_distinct_parity(
                    &batch,
                    &Distinct {
                        subset: subset.clone(),
                        keep,
                    },
                );
            }
        }
    }

    #[test]
    fn distinct_matches_reference_on_empty_and_single_row() {
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
            ],
        )
        .expect("fixture vuota");
        for keep in [Keep::First, Keep::Last, Keep::False] {
            assert_distinct_parity(
                &empty,
                &Distinct {
                    subset: vec![],
                    keep,
                },
            );
        }

        // Riga singola con valori null in tutte le colonne.
        let single = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![None::<&str>])),
                Arc::new(Float64Array::from(vec![None::<f64>])),
            ],
        )
        .expect("fixture riga singola");
        for keep in [Keep::First, Keep::Last, Keep::False] {
            assert_distinct_parity(
                &single,
                &Distinct {
                    subset: vec![],
                    keep,
                },
            );
        }
    }

    #[test]
    fn distinct_matches_reference_at_parallel_scale() {
        // Sopra la soglia parallela condivisa (32_768 righe): chiavi
        // numeriche e testuali, con null e valori ripetuti.
        let batch = scale_fixture(70_000, 100);
        for keep in [Keep::First, Keep::Last, Keep::False] {
            assert_distinct_parity(
                &batch,
                &Distinct {
                    subset: vec!["txt".into()],
                    keep,
                },
            );
            assert_distinct_parity(
                &batch,
                &Distinct {
                    subset: vec!["val".into(), "txt".into()],
                    keep,
                },
            );
            assert_distinct_parity(
                &batch,
                &Distinct {
                    subset: vec![],
                    keep,
                },
            );
        }
    }

    #[test]
    fn distinct_preserves_errors() {
        // Colonna LargeUtf8 nel subset: il profilo scalare fallisce allo
        // stesso modo nel riferimento.
        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "c",
                DataType::LargeUtf8,
                true,
            )])),
            vec![Arc::new(LargeStringArray::from(vec![Some("a"), Some("b")]))],
        )
        .expect("fixture large utf8");
        let config = Distinct {
            subset: vec![],
            keep: Keep::First,
        };
        let fast_error = distinct(&large, &config).expect_err("fast path errore");
        let reference_error = distinct_reference(&large, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());

        // Colonna inesistente: errore di schema invariato.
        let batch = mixed_fixture();
        let missing = Distinct {
            subset: vec!["manca".into()],
            keep: Keep::First,
        };
        let fast_error = distinct(&batch, &missing).expect_err("fast path errore");
        let reference_error = distinct_reference(&batch, &missing).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
    }

    #[test]
    fn dedup_advanced_matches_reference() {
        let batch = mixed_fixture();
        for keep in [Keep::First, Keep::Last] {
            for ascending in [true, false] {
                assert_dedup_parity(
                    &batch,
                    &DedupAdvanced {
                        subset: vec!["g".into()],
                        keep,
                        order_column: Some("val".into()),
                        ascending,
                    },
                );
                assert_dedup_parity(
                    &batch,
                    &DedupAdvanced {
                        subset: vec!["g".into(), "val".into()],
                        keep,
                        order_column: Some("num".into()),
                        ascending,
                    },
                );
                // Senza order_column: nessun ordinamento preliminare.
                assert_dedup_parity(
                    &batch,
                    &DedupAdvanced {
                        subset: vec!["txt".into()],
                        keep,
                        order_column: None,
                        ascending,
                    },
                );
            }
        }
        // keep=false: errore di contratto identico.
        let config = DedupAdvanced {
            subset: vec!["g".into()],
            keep: Keep::False,
            order_column: None,
            ascending: true,
        };
        let fast_error = dedup_advanced(&batch, &config).expect_err("fast path errore");
        let reference_error =
            dedup_advanced_reference(&batch, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
    }

    /// Config rolling di base sulla fixture mista (`num` con NaN/-0.0/null,
    /// partizione `g` con null, ordine `val`).
    fn rolling_config(function: RollingKind, window: usize, min_periods: usize) -> RollingWindow {
        RollingWindow {
            column: "num".into(),
            function,
            group_by: Some("g".into()),
            order_column: Some("val".into()),
            window,
            min_periods,
            ddof: 1,
            output_column: "num_roll".into(),
        }
    }

    #[test]
    fn rolling_window_matches_reference_all_functions_and_partial_windows() {
        let batch = mixed_fixture();
        for function in [
            RollingKind::Sum,
            RollingKind::Mean,
            RollingKind::Min,
            RollingKind::Max,
            RollingKind::Stddev,
        ] {
            // Finestra piena, parziale (min_periods < window) e ddof 0.
            assert_rolling_parity(&batch, &rolling_config(function, 3, 1));
            assert_rolling_parity(&batch, &rolling_config(function, 3, 3));
            assert_rolling_parity(&batch, &rolling_config(function, 8, 2));
            assert_rolling_parity(
                &batch,
                &RollingWindow {
                    ddof: 0,
                    ..rolling_config(function, 3, 1)
                },
            );
        }
        // Colonna Int64, senza partizione e senza ordinamento.
        assert_rolling_parity(
            &batch,
            &RollingWindow {
                column: "val".into(),
                function: RollingKind::Stddev,
                group_by: None,
                order_column: None,
                window: 2,
                min_periods: 1,
                ddof: 1,
                output_column: "val_roll".into(),
            },
        );
        // Partizione singola esplicita (tutte le righe nella stessa chiave).
        let single = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("k"), Some("k"), Some("k")])),
                Arc::new(Float64Array::from(vec![Some(-0.0), None, Some(f64::NAN)])),
            ],
        )
        .expect("fixture partizione singola");
        assert_rolling_parity(
            &single,
            &RollingWindow {
                column: "num".into(),
                function: RollingKind::Mean,
                group_by: Some("g".into()),
                order_column: None,
                window: 2,
                min_periods: 1,
                ddof: 1,
                output_column: "num_roll".into(),
            },
        );
        // Finestra di un solo elemento NaN: min/max devono restituire NaN
        // (reduce dal primo elemento), non +/-inf; somma NaN invariata.
        for function in [
            RollingKind::Sum,
            RollingKind::Mean,
            RollingKind::Min,
            RollingKind::Max,
            RollingKind::Stddev,
        ] {
            assert_rolling_parity(
                &single,
                &RollingWindow {
                    column: "num".into(),
                    function,
                    group_by: Some("g".into()),
                    order_column: None,
                    window: 1,
                    min_periods: 1,
                    ddof: 0,
                    output_column: "num_roll".into(),
                },
            );
        }
    }

    #[test]
    fn rolling_window_matches_reference_at_parallel_scale() {
        let batch = scale_fixture(70_000, 100);
        assert_rolling_parity(&batch, &rolling_config(RollingKind::Mean, 10, 1));
        assert_rolling_parity(&batch, &rolling_config(RollingKind::Stddev, 10, 3));
        assert_rolling_parity(&batch, &rolling_config(RollingKind::Sum, 5, 2));
    }

    #[test]
    fn rolling_window_preserves_errors() {
        let batch = mixed_fixture();
        // Finestra non valida: window 0, min_periods 0, min_periods > window.
        let mut config = rolling_config(RollingKind::Mean, 3, 1);
        config.window = 0;
        assert!(rolling_window(&batch, &config).is_err());
        config = rolling_config(RollingKind::Mean, 3, 1);
        config.min_periods = 0;
        assert!(rolling_window(&batch, &config).is_err());
        config = rolling_config(RollingKind::Mean, 3, 1);
        config.min_periods = 4;
        assert!(rolling_window(&batch, &config).is_err());
        // Sorgente non numerica: stesso errore del riferimento.
        let config = RollingWindow {
            column: "txt".into(),
            ..rolling_config(RollingKind::Mean, 3, 1)
        };
        let fast_error = rolling_window(&batch, &config).expect_err("fast path errore");
        let reference_error =
            rolling_window_reference(&batch, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
    }

    /// Config window di base sulla fixture mista (partizione `g` con null,
    /// ordine `val` con null, sorgente `num` con NaN/-0.0/null).
    fn window_config(function: WindowKind) -> WindowFunction {
        WindowFunction {
            column: "num".into(),
            function,
            group_by: Some("g".into()),
            order_column: Some("val".into()),
            offset: 1,
            buckets: None,
            output_column: Some("num_win".into()),
        }
    }

    #[test]
    fn window_function_matches_reference_all_kinds() {
        let batch = mixed_fixture();
        for function in [
            WindowKind::Rank,
            WindowKind::DenseRank,
            WindowKind::Cumsum,
            WindowKind::Cumcount,
            WindowKind::Lag,
            WindowKind::Lead,
            WindowKind::PctChange,
            WindowKind::RunningMean,
            WindowKind::PercentRank,
            WindowKind::CumeDist,
        ] {
            assert_window_parity(&batch, &window_config(function));
        }
        // Offset maggiore di 1 per lag/lead.
        assert_window_parity(
            &batch,
            &WindowFunction {
                offset: 3,
                ..window_config(WindowKind::Lag)
            },
        );
        assert_window_parity(
            &batch,
            &WindowFunction {
                offset: 2,
                ..window_config(WindowKind::Lead)
            },
        );
        // Ntile: bucket minori, uguali e maggiori della partizione.
        for buckets in [1_usize, 2, 3, 100] {
            assert_window_parity(
                &batch,
                &WindowFunction {
                    buckets: Some(buckets),
                    ..window_config(WindowKind::Ntile)
                },
            );
        }
        // Nome output di default (`{colonna}_{suffix}`), senza partizione,
        // senza ordinamento.
        assert_window_parity(
            &batch,
            &WindowFunction {
                column: "num".into(),
                function: WindowKind::RunningMean,
                group_by: None,
                order_column: None,
                offset: 1,
                buckets: None,
                output_column: None,
            },
        );
        // Partizione singola esplicita con NaN nella sorgente.
        let single = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::Utf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("k"), Some("k"), Some("k")])),
                Arc::new(Float64Array::from(vec![Some(-0.0), None, Some(f64::NAN)])),
            ],
        )
        .expect("fixture partizione singola");
        for function in [
            WindowKind::Rank,
            WindowKind::DenseRank,
            WindowKind::CumeDist,
        ] {
            assert_window_parity(
                &single,
                &WindowFunction {
                    column: "num".into(),
                    function,
                    group_by: Some("g".into()),
                    order_column: None,
                    offset: 1,
                    buckets: None,
                    output_column: None,
                },
            );
        }
    }

    #[test]
    fn window_function_matches_reference_at_parallel_scale() {
        let batch = scale_fixture(70_000, 100);
        assert_window_parity(&batch, &window_config(WindowKind::Rank));
        assert_window_parity(&batch, &window_config(WindowKind::DenseRank));
        assert_window_parity(&batch, &window_config(WindowKind::PercentRank));
        assert_window_parity(&batch, &window_config(WindowKind::Cumsum));
        assert_window_parity(
            &batch,
            &WindowFunction {
                buckets: Some(7),
                ..window_config(WindowKind::Ntile)
            },
        );
        // Scala senza partizione: un unico gruppo oltre soglia.
        assert_window_parity(
            &batch,
            &WindowFunction {
                group_by: None,
                order_column: Some("num".into()),
                ..window_config(WindowKind::CumeDist)
            },
        );
    }

    #[test]
    fn window_function_preserves_errors() {
        let batch = mixed_fixture();
        // offset 0.
        let config = WindowFunction {
            offset: 0,
            ..window_config(WindowKind::Lag)
        };
        assert!(window_function(&batch, &config).is_err());
        // buckets su funzione diversa da ntile.
        let config = WindowFunction {
            buckets: Some(2),
            ..window_config(WindowKind::Rank)
        };
        assert!(window_function(&batch, &config).is_err());
        // ntile senza buckets o con buckets 0.
        let config = window_config(WindowKind::Ntile);
        assert!(window_function(&batch, &config).is_err());
        let config = WindowFunction {
            buckets: Some(0),
            ..window_config(WindowKind::Ntile)
        };
        assert!(window_function(&batch, &config).is_err());
        // Sorgente non numerica: stesso errore del riferimento.
        let config = WindowFunction {
            column: "txt".into(),
            ..window_config(WindowKind::Rank)
        };
        let fast_error = window_function(&batch, &config).expect_err("fast path errore");
        let reference_error =
            window_function_reference(&batch, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
        // Colonna di partizione LargeUtf8: errore di profilo scalare identico.
        let large = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("g", DataType::LargeUtf8, true),
                Field::new("num", DataType::Float64, true),
            ])),
            vec![
                Arc::new(LargeStringArray::from(vec![Some("a"), Some("b")])),
                Arc::new(Float64Array::from(vec![1.0, 2.0])),
            ],
        )
        .expect("fixture large utf8");
        let config = WindowFunction {
            column: "num".into(),
            function: WindowKind::Rank,
            group_by: Some("g".into()),
            order_column: None,
            offset: 1,
            buckets: None,
            output_column: None,
        };
        let fast_error = window_function(&large, &config).expect_err("fast path errore");
        let reference_error =
            window_function_reference(&large, &config).expect_err("riferimento errore");
        assert_eq!(fast_error.to_string(), reference_error.to_string());
    }

    #[test]
    fn sort_orders_i64_beyond_f64_precision_exactly() {
        // Regressione: 2^53 e 2^53+1 collassano sullo stesso double
        // (9007199254740992), quindi un confronto via f64 li direbbe
        // uguali e ricadrebbe sull'indice di riga, ordinando in silenzio
        // male. Il confronto nativo `i64::cmp` li distingue.
        let big: i64 = 1 << 53;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("v", DataType::Int64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3, 4])),
                Arc::new(Int64Array::from(vec![
                    big + 1,
                    big,
                    big + 2,
                    -big - 1,
                    -big,
                ])),
            ],
        )
        .expect("fixture");
        let ascending = Sort {
            columns: vec!["v".into()],
            ascending: true,
        };
        assert_eq!(sorted_ids(&batch, &ascending), vec![3, 4, 1, 0, 2]);
        let descending = Sort {
            columns: vec!["v".into()],
            ascending: false,
        };
        assert_eq!(sorted_ids(&batch, &descending), vec![2, 0, 1, 4, 3]);
    }

    #[test]
    fn sort_and_top_n_order_u64_numerically() {
        // Regressione: senza confronto nativo UInt64 cade nel fallback
        // testuale ("10" < "9"); `u64::cmp` ordina numericamente.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("u", DataType::UInt64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3])),
                Arc::new(UInt64Array::from(vec![9_u64, 10, 100, 99])),
            ],
        )
        .expect("fixture");
        let ascending = Sort {
            columns: vec!["u".into()],
            ascending: true,
        };
        assert_eq!(sorted_ids(&batch, &ascending), vec![0, 1, 3, 2]);
        let descending = Sort {
            columns: vec!["u".into()],
            ascending: false,
        };
        assert_eq!(sorted_ids(&batch, &descending), vec![2, 3, 1, 0]);

        let top_ascending = top_n(
            &batch,
            &TopN {
                columns: vec!["u".into()],
                n: 2,
                descending: false,
            },
        )
        .expect("top_n ascendente");
        let top_ids = top_ascending
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("id column")
            .values()
            .to_vec();
        assert_eq!(top_ids, vec![0, 1]);
        let top_descending = top_n(
            &batch,
            &TopN {
                columns: vec!["u".into()],
                n: 2,
                descending: true,
            },
        )
        .expect("top_n discendente");
        let top_ids = top_descending
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("id column")
            .values()
            .to_vec();
        assert_eq!(top_ids, vec![2, 3]);
    }

    #[test]
    fn compare_at_generic_path_orders_u64_numerically() {
        // `compare_at` (percorso `ColumnComparator::Generic`) delega allo
        // stesso comparatore tipizzato unico: UInt64 resta numerico anche
        // fuori dal fast path tipizzato.
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("u", DataType::UInt64, false)])),
            vec![Arc::new(UInt64Array::from(vec![9_u64, 10]))],
        )
        .expect("fixture");
        assert_eq!(compare_at(&batch, 0, 0, 1).expect("cmp"), Ordering::Less);
        assert_eq!(compare_at(&batch, 0, 1, 0).expect("cmp"), Ordering::Greater);
    }
}
