//! Oracolo delle chiavi `Binary` non UTF-8 (WKB) nelle operazioni a chiave.
//!
//! Prima il percorso testuale (`scalar_as_string`) rifiutava un `Binary` non
//! UTF-8 con `binary non contiene UTF-8 valido`; ora la chiave sono i byte
//! grezzi. Per ogni input che il testo accettava (binari UTF-8 validi) gli
//! oracoli testuali esistenti (`row_key`, `key_for_row`, `composite_key`,
//! `RowKeyEncoder`) restano la prova che identita' e ordine non cambiano.
//! Qui si provano gli input prima rifiutati, con due oracoli indipendenti
//! dal codice di produzione:
//!
//! - **rimappatura**: ogni valore non UTF-8 si sostituisce con un testo
//!   ASCII della stessa lunghezza che non compare in nessun input. La
//!   sostituzione e' iniettiva e conserva le lunghezze, quindi le
//!   operazioni che non ordinano per chiave (`distinct`, `dedup_advanced`, `assert_unique`,
//!   join, semi/anti, asof, `reconcile`, foreign key, `table_diff`, set
//!   operation) devono dare `rimappa(op(x)) == op(rimappa(x))`: stesse
//!   righe, stessi errori, stessa diagnostica;
//! - **ordine**: `aggregate` ordina i gruppi. L'atteso si calcola con una
//!   `BTreeMap` sui byte della chiave di `row_key` estesa ai byte grezzi
//!   (`{tipo}\u{1e}1{len}:{byte}\u{1f}`, null `0`), la definizione
//!   dell'ordine canonico; su un binario UTF-8 valido quei byte sono quelli
//!   del testo.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde_json::{json, Value};

use crate::aggregation::{self, Aggregate};
use crate::{governance, joins, quality, reshape, setops, Limits};

/// Valori `Binary` della fixture: WKB di un punto (non UTF-8), byte non
/// UTF-8 singoli e in coppia, prefissi l'uno dell'altro, lunghezze che il
/// tag decimale ordina al contrario (1 byte contro 10), UTF-8 validi, vuoto.
fn pool() -> Vec<Option<Vec<u8>>> {
    let mut wkb = vec![0x01, 0x01, 0x00, 0x00, 0x00];
    wkb.extend_from_slice(&1.5_f64.to_le_bytes());
    wkb.extend_from_slice(&(-2.0_f64).to_le_bytes());
    let mut wkb2 = wkb.clone();
    wkb2[5] = 0xff;
    vec![
        None,
        Some(Vec::new()),
        Some(b"a".to_vec()),
        Some(b"ab".to_vec()),
        Some(vec![0xff]),
        Some(vec![0xff, 0xfe]),
        Some(vec![0xc3, 0x28]),
        Some(vec![0x80]),
        Some(vec![0x00, 0xff]),
        Some(vec![0xff; 10]),
        Some(vec![0xfe; 9]),
        Some(b"\xc3\xa4".to_vec()),
        Some(wkb),
        Some(wkb2),
    ]
}

struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).expect("limite")).expect("indice")
    }
}

/// Batch `g` (Binary), `s` (Utf8), `id` (Int64, unico), `v` (Float64).
fn fixture(rows: usize, seed: u64, id_base: i64) -> RecordBatch {
    let pool = pool();
    let mut rng = Xorshift(seed | 1);
    let g = (0..rows)
        .map(|_| pool[rng.below(pool.len())].clone())
        .collect::<Vec<_>>();
    let s = (0..rows)
        .map(|_| [None, Some("x"), Some("y")][rng.below(3)])
        .collect::<Vec<_>>();
    let id = (0..rows)
        .map(|row| id_base + i64::try_from(row).expect("id"))
        .collect::<Vec<_>>();
    // Il corpus usa solo interi finiti non negativi: bits e uguaglianza
    // numerica coincidono, senza NaN o signed zero.
    let v = (0..rows)
        .map(|_| f64::from(u32::try_from(rng.below(5)).expect("v")))
        .collect::<Vec<_>>();
    assert!(v
        .iter()
        .all(|v| v.is_finite() && !v.is_sign_negative() && v.fract() == 0.0));
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("g", DataType::Binary, true),
            Field::new("s", DataType::Utf8, true),
            Field::new("id", DataType::Int64, false),
            Field::new("v", DataType::Float64, false),
        ])),
        vec![
            Arc::new(BinaryArray::from(
                g.iter().map(Option::as_deref).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(s)),
            Arc::new(Int64Array::from(id)),
            Arc::new(Float64Array::from(v)),
        ],
    )
    .expect("fixture")
}

/// Sostituzione iniettiva dei valori non UTF-8 di tutte le colonne Binary
/// degli input con testi ASCII stampabili della stessa lunghezza, assenti
/// da ogni input.
struct Rimappa(HashMap<Vec<u8>, Vec<u8>>);

impl Rimappa {
    fn new(batches: &[&RecordBatch]) -> Self {
        let mut presenti = BTreeSet::new();
        for batch in batches {
            for column in batch.columns() {
                if let Some(values) = column.as_any().downcast_ref::<BinaryArray>() {
                    presenti.extend(values.iter().flatten().map(<[u8]>::to_vec));
                }
            }
        }
        let mut mappa = HashMap::new();
        let mut contatore = 0_u64;
        for valore in &presenti {
            if std::str::from_utf8(valore).is_ok() {
                continue;
            }
            let sostituto = loop {
                // Cifre in base 95 sui caratteri stampabili, lunghezza fissa.
                let mut n = contatore;
                contatore += 1;
                let candidato = (0..valore.len())
                    .map(|_| {
                        let cifra = u8::try_from(n % 95).expect("cifra");
                        n /= 95;
                        b' ' + cifra
                    })
                    .collect::<Vec<_>>();
                assert_eq!(n, 0, "sostituti esauriti per la lunghezza {}", valore.len());
                if !presenti.contains(&candidato) && !mappa.values().any(|v| v == &candidato) {
                    break candidato;
                }
            };
            mappa.insert(valore.clone(), sostituto);
        }
        Self(mappa)
    }

    fn batch(&self, batch: &RecordBatch) -> RecordBatch {
        let columns = batch
            .columns()
            .iter()
            .map(|column| -> ArrayRef {
                match column.as_any().downcast_ref::<BinaryArray>() {
                    Some(values) => Arc::new(BinaryArray::from(
                        values
                            .iter()
                            .map(|valore| valore.map(|v| self.0.get(v).map_or(v, Vec::as_slice)))
                            .collect::<Vec<_>>(),
                    )),
                    None => column.clone(),
                }
            })
            .collect::<Vec<_>>();
        crate::batch_with_rows(batch.schema(), columns, batch.num_rows()).expect("batch rimappato")
    }
}

fn config<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("config")
}

/// Ci sono davvero valori non UTF-8 nell'input: senza, la prova e' vuota.
fn ha_non_utf8(batch: &RecordBatch) -> bool {
    batch
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("g")
        .iter()
        .flatten()
        .any(|v| std::str::from_utf8(v).is_err())
}

#[test]
fn prima_erano_errori_ora_sono_chiavi() {
    // Il caso del difetto: una colonna di geometrie WKB.
    let batch = fixture(40, 7, 0);
    assert!(ha_non_utf8(&batch));
    // Il testo la rifiuta ancora: e' `scalar_as_string` a non avere un testo.
    let valori = batch
        .column(0)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    for (row, valore) in valori.iter().enumerate() {
        let atteso = valore.map(std::str::from_utf8).transpose();
        let risultato = crate::scalar_as_string(batch.column(0).as_ref(), row);
        match atteso {
            Ok(testo) => {
                assert_successo!(risultato, testo.map(str::to_owned));
            }
            Err(_) => {
                assert_rifiuto!(
                    risultato,
                    plenora_core::ErrorCategory::Schema,
                    plenora_core::ErrorPhase::Validate
                );
            }
        }
    }
    let mut gruppi: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
    for row in 0..batch.num_rows() {
        gruppi
            .entry(chiave_estesa(&batch, &[0], row))
            .or_default()
            .push(row);
    }
    let mut primi: Vec<_> = gruppi.values().map(|righe| righe[0]).collect();
    primi.sort_unstable();
    let atteso = crate::select_rows(&batch, &primi).unwrap();
    assert_successo!(
        aggregation::distinct(&batch, &config(json!({"subset": ["g"]}))),
        atteso
    );
    assert_successo!(
        aggregation::dedup_advanced(&batch, &config(json!({"subset": ["g"]}))),
        atteso
    );
    let ordinati: Vec<_> = gruppi.values().map(|righe| righe[0]).collect();
    let chiavi = crate::select_rows(&batch, &ordinati)
        .unwrap()
        .project(&[0])
        .unwrap();
    let conteggi = Int64Array::from(
        gruppi
            .values()
            .map(|righe| i64::try_from(righe.len()).unwrap())
            .collect::<Vec<_>>(),
    );
    let atteso = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("g", DataType::Binary, true),
            Field::new("count", DataType::Int64, false),
        ])),
        vec![chiavi.column(0).clone(), Arc::new(conteggi)],
    )
    .unwrap();
    assert_successo!(
        aggregation::aggregate(&batch, &config(json!({"group_by": ["g"]}))),
        atteso
    );
    let unico = fixture(1, 7, 0);
    assert_successo!(
        quality::assert_unique(&unico, &config(json!({"columns": ["g"]}))),
        unico
    );
}

#[test]
fn chiave_di_partizione_preserva_uguaglianza_e_ordine_di_ogni_byte() {
    let mut valori = pool();
    for byte in 0..=u8::MAX {
        valori.push(Some(vec![byte]));
        valori.push(Some(vec![byte, 0]));
        valori.push(Some(vec![byte, 255]));
    }
    let array = BinaryArray::from(valori.iter().map(Option::as_deref).collect::<Vec<_>>());
    let chiavi: Vec<_> = (0..valori.len())
        .map(|row| crate::scalar_partition_key(&array, row).expect("chiave"))
        .collect();
    for (a, valore_a) in valori.iter().enumerate() {
        for (b, valore_b) in valori.iter().enumerate() {
            assert_eq!(chiavi[a].cmp(&chiavi[b]), valore_a.cmp(valore_b));
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn partizioni_e_nunique_contro_ranghi_testuali_indipendenti() {
    for righe in [0, 1, 40, 200] {
        let batch = fixture(righe, 7, 0);
        let binary = batch
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let distinti: BTreeSet<_> = binary.iter().flatten().collect();
        let ranghi: BTreeMap<_, _> = distinti
            .into_iter()
            .enumerate()
            .map(|(i, bytes)| (bytes, format!("{i:08}")))
            .collect();
        let mut campi: Vec<_> = batch
            .schema()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect();
        campi[0] = Field::new("g", DataType::Utf8, true);
        let mut colonne = batch.columns().to_vec();
        colonne[0] = Arc::new(StringArray::from(
            binary
                .iter()
                .map(|v| v.map(|bytes| ranghi[bytes].clone()))
                .collect::<Vec<_>>(),
        ));
        let riferimento =
            plenora_core::batch_with_rows(Arc::new(Schema::new(campi)), colonne, batch.num_rows())
                .unwrap();
        let operazioni: Vec<(&str, Value)> = vec![
            (
                "statistics",
                json!({"column":"v","group_by":"g","stats":["count","sum"]}),
            ),
            (
                "sample",
                json!({"n":12,"stratify_column":"g","random_state":9}),
            ),
            (
                "add_row_number",
                json!({"partition_column":"g","output_column":"numero"}),
            ),
            (
                "rolling_window",
                json!({"column":"v","group_by":"g","function":"sum","window":3,"output_column":"finestra"}),
            ),
            (
                "window_function",
                json!({"column":"v","group_by":"g","function":"cumcount"}),
            ),
            (
                "aggregate",
                json!({"group_by":["s"],"aggregations":[{"column":"g","function":"nunique","alias":"numero"}]}),
            ),
        ];
        for (nome, configurazione) in operazioni {
            let esegui = |input: &RecordBatch| match nome {
                "statistics" => crate::analysis::statistics(input, &config(configurazione.clone())),
                "sample" => crate::analysis::sample(input, &config(configurazione.clone())),
                "add_row_number" => {
                    crate::utility::add_row_number(input, &config(configurazione.clone()))
                }
                "rolling_window" => {
                    aggregation::rolling_window(input, &config(configurazione.clone()))
                }
                "window_function" => {
                    aggregation::window_function(input, &config(configurazione.clone()))
                }
                "aggregate" => aggregation::aggregate(input, &config(configurazione.clone())),
                _ => unreachable!(),
            };
            let mut gruppi: BTreeMap<Option<&[u8]>, Vec<usize>> = BTreeMap::new();
            for (row, key) in binary.iter().enumerate() {
                gruppi.entry(key).or_default().push(row);
            }
            let values = batch
                .column(3)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            let mut fields: Vec<Field> = batch
                .schema()
                .fields()
                .iter()
                .map(|f| f.as_ref().clone())
                .collect();
            let mut columns = batch.columns().to_vec();
            let expected = match nome {
                "sample" => {
                    let mut selected = Vec::new();
                    for (offset, group) in gruppi.values().enumerate() {
                        let mut rows = group.clone();
                        let mut rng = Xorshift(9 + u64::try_from(offset).unwrap());
                        for i in (1..rows.len()).rev() {
                            rows.swap(i, rng.below(i + 1));
                        }
                        let count = (12 * rows.len() / righe).max(1).min(rows.len());
                        selected.extend(rows.into_iter().take(count));
                    }
                    crate::select_rows(&batch, &selected).unwrap()
                }
                "aggregate" => {
                    let strings = batch
                        .column(1)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap();
                    let mut groups: BTreeMap<Option<&str>, BTreeSet<Option<&[u8]>>> =
                        BTreeMap::new();
                    for (r, key) in strings.iter().enumerate() {
                        groups
                            .entry(key)
                            .or_default()
                            .insert(binary.iter().nth(r).unwrap());
                    }
                    RecordBatch::try_new(
                        Arc::new(Schema::new(vec![
                            Field::new("s", DataType::Utf8, true),
                            Field::new("numero", DataType::Int64, false),
                        ])),
                        vec![
                            Arc::new(StringArray::from(
                                groups.keys().copied().collect::<Vec<_>>(),
                            )),
                            Arc::new(Int64Array::from(
                                groups
                                    .values()
                                    .map(|g| {
                                        i64::try_from(g.iter().filter(|v| v.is_some()).count())
                                            .unwrap()
                                    })
                                    .collect::<Vec<_>>(),
                            )),
                        ],
                    )
                    .unwrap()
                }
                _ => {
                    let mut numbers = vec![0.0; righe];
                    let mut positions = vec![0_i64; righe];
                    let mut sums = vec![0.0; righe];
                    for group in gruppi.values() {
                        for (position, &row) in group.iter().enumerate() {
                            positions[row] = i64::try_from(position + 1).unwrap();
                            numbers[row] = match nome {
                                "statistics" => f64::from(u32::try_from(group.len()).unwrap()),
                                "add_row_number" => f64::from(u32::try_from(position + 1).unwrap()),
                                "window_function" => f64::from(u32::try_from(position).unwrap()),
                                "rolling_window" => group[position.saturating_sub(2)..=position]
                                    .iter()
                                    .map(|&r| values.value(r))
                                    .sum(),
                                _ => panic!("scenario non previsto"),
                            };
                            sums[row] = group.iter().map(|&r| values.value(r)).sum();
                        }
                    }
                    let name = match nome {
                        "statistics" => "v_count",
                        "add_row_number" => "numero",
                        "window_function" => "v_cumcount",
                        _ => "finestra",
                    };
                    if nome == "add_row_number" {
                        fields.push(Field::new(name, DataType::Int64, false));
                        columns.push(Arc::new(Int64Array::from(positions)));
                    } else {
                        fields.push(Field::new(name, DataType::Float64, true));
                        columns.push(Arc::new(Float64Array::from(numbers)));
                    }
                    if nome == "statistics" {
                        fields.push(Field::new("v_sum", DataType::Float64, true));
                        columns.push(Arc::new(Float64Array::from(sums)));
                    }
                    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
                }
            };
            assert_successo!(esegui(&batch), expected);
            // La rimappatura deve conservare anche chiave, schema e bitmap.
            let mut fields: Vec<Field> = expected
                .schema()
                .fields()
                .iter()
                .map(|f| f.as_ref().clone())
                .collect();
            let mut columns = expected.columns().to_vec();
            if let Some(index) = fields.iter().position(|f| f.name() == "g") {
                let binary = expected
                    .column(index)
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .unwrap();
                fields[index] = Field::new("g", DataType::Utf8, true);
                columns[index] = Arc::new(StringArray::from(
                    binary
                        .iter()
                        .map(|v| v.map(|bytes| ranghi[bytes].clone()))
                        .collect::<Vec<_>>(),
                ));
            }
            let expected = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap();
            assert_successo!(esegui(&riferimento), expected);
        }
    }
}

/// Chiave del corpus ricavata direttamente dagli array, senza encoder del kernel.
fn chiave_fixture(batch: &RecordBatch, columns: &[usize], row: usize) -> Vec<Option<Vec<u8>>> {
    columns
        .iter()
        .map(|&column| {
            let values = batch.column(column);
            if values.is_null(row) {
                return None;
            }
            Some(match column {
                0 => values
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .unwrap()
                    .value(row)
                    .to_vec(),
                1 => values
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .value(row)
                    .as_bytes()
                    .to_vec(),
                2 => values
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .value(row)
                    .to_le_bytes()
                    .to_vec(),
                3 => values
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap()
                    .value(row)
                    .to_bits()
                    .to_le_bytes()
                    .to_vec(),
                _ => panic!("colonna fuori fixture"),
            })
        })
        .collect()
}

fn righe_distinte(
    batch: &RecordBatch,
    columns: &[usize],
    order: &[usize],
    keep: &str,
) -> Vec<usize> {
    order
        .iter()
        .copied()
        .filter(|&row| {
            let key = chiave_fixture(batch, columns, row);
            let equal: Vec<_> = order
                .iter()
                .copied()
                .filter(|&r| chiave_fixture(batch, columns, r) == key)
                .collect();
            match keep {
                "first" => equal[0] == row,
                "last" => equal[equal.len() - 1] == row,
                "false" => equal.len() == 1,
                _ => panic!("keep fuori fixture"),
            }
        })
        .collect()
}

#[test]
fn distinct_dedup_assert_unique_come_sul_testo_rimappato() {
    for (rows, seed) in [(0_usize, 1_u64), (1, 2), (13, 3), (200, 4), (1_000, 5)] {
        let original = fixture(rows, seed, 0);
        let rimappato = Rimappa::new(&[&original]).batch(&original);
        for batch in [&original, &rimappato] {
            for subset in [vec!["g"], vec!["g", "s"], vec!["s", "g"], vec![]] {
                let columns: Vec<_> = if subset.is_empty() {
                    vec![0, 1, 2, 3]
                } else {
                    subset
                        .iter()
                        .map(|name| usize::from(*name != "g"))
                        .collect()
                };
                let order: Vec<_> = (0..rows).collect();
                for keep in ["first", "last", "false"] {
                    let expected =
                        crate::select_rows(batch, &righe_distinte(batch, &columns, &order, keep))
                            .unwrap();
                    assert_successo!(
                        aggregation::distinct(
                            batch,
                            &config(json!({"subset": subset, "keep": keep}))
                        ),
                        expected
                    );
                }
                for cfg in [
                    json!({"subset": subset, "keep": "first"}),
                    json!({"subset": subset, "keep": "last", "order_column": "v"}),
                    json!({"subset": subset, "keep": "first", "order_column": "v", "ascending": false}),
                ] {
                    let result = aggregation::dedup_advanced(batch, &config(cfg.clone()));
                    // Il kernel diretto e' documentato come sort + distinct:
                    // subset vuoto seleziona tutte le colonne, come distinct.
                    {
                        let mut order = order.clone();
                        if cfg.get("order_column").is_some() {
                            let values = batch
                                .column(3)
                                .as_any()
                                .downcast_ref::<Float64Array>()
                                .unwrap();
                            order.sort_by(|&a, &b| {
                                let cmp = values.value(a).total_cmp(&values.value(b));
                                if cfg["ascending"] == false {
                                    cmp.reverse()
                                } else {
                                    cmp
                                }
                            });
                        }
                        let expected = crate::select_rows(
                            batch,
                            &righe_distinte(batch, &columns, &order, cfg["keep"].as_str().unwrap()),
                        )
                        .unwrap();
                        assert_successo!(result, expected);
                    }
                }
            }
            for columns in [vec![0], vec![0, 1]] {
                let names: Vec<_> = columns
                    .iter()
                    .map(|&i| if i == 0 { "g" } else { "s" })
                    .collect();
                for nulls_equal in [true, false] {
                    let cfg = config(json!({"columns": names, "nulls_equal": nulls_equal}));
                    let keys: Vec<_> = (0..rows)
                        .map(|r| chiave_fixture(batch, &columns, r))
                        .filter(|key| nulls_equal || key.iter().all(Option::is_some))
                        .collect();
                    let duplicate = keys.iter().enumerate().any(|(i, k)| keys[..i].contains(k));
                    let result = quality::assert_unique(batch, &cfg);
                    if duplicate {
                        assert_rifiuto!(
                            result,
                            plenora_core::ErrorCategory::DataMapping,
                            plenora_core::ErrorPhase::Write
                        );
                    } else {
                        assert_successo!(result, *batch);
                    }
                    let unique = crate::select_rows(
                        batch,
                        &righe_distinte(batch, &columns, &(0..rows).collect::<Vec<_>>(), "first"),
                    )
                    .unwrap();
                    assert_successo!(quality::assert_unique(&unique, &cfg), unique);
                }
            }
        }
    }
}

/// Materializza le coppie scelte dall'oracolo con il solo take Arrow.
fn batch_coppie(
    left: &RecordBatch,
    right: &RecordBatch,
    keys: &[usize],
    pairs: &[(usize, Option<usize>)],
    asof: bool,
) -> RecordBatch {
    use plenora_core::arrow::array::UInt32Array;
    assert!(left.schema().metadata().is_empty() && right.schema().metadata().is_empty());
    assert!(left
        .schema()
        .fields()
        .iter()
        .chain(right.schema().fields())
        .all(|f| f.metadata().is_empty()));
    let li = UInt32Array::from(
        pairs
            .iter()
            .map(|p| u32::try_from(p.0).unwrap())
            .collect::<Vec<_>>(),
    );
    let ri = UInt32Array::from(
        pairs
            .iter()
            .map(|p| p.1.map(|r| u32::try_from(r).unwrap()))
            .collect::<Vec<_>>(),
    );
    let mut fields = Vec::new();
    let mut columns = Vec::new();
    for (i, field) in left.schema().fields().iter().enumerate() {
        let name = if asof || keys.contains(&i) {
            field.name().clone()
        } else {
            format!("{}_L", field.name())
        };
        fields.push(field.as_ref().clone().with_name(name).with_nullable(true));
        columns.push(
            plenora_core::arrow::select::take::take(left.column(i).as_ref(), &li, None).unwrap(),
        );
    }
    for (i, field) in right.schema().fields().iter().enumerate() {
        if keys.contains(&i) || asof && i == 3 {
            continue;
        }
        fields.push(
            field
                .as_ref()
                .clone()
                .with_name(format!("{}_R", field.name()))
                .with_nullable(true),
        );
        columns.push(
            plenora_core::arrow::select::take::take(right.column(i).as_ref(), &ri, None).unwrap(),
        );
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn join_semi_anti_asof_come_sul_testo_rimappato() {
    let limits = Limits::default();
    for (left_rows, right_rows, seed) in [(0_usize, 5_usize, 1_u64), (17, 0, 2), (60, 45, 3)] {
        let left = fixture(left_rows, seed, 0);
        let right = fixture(right_rows, seed.wrapping_mul(31), 1_000);
        let rimappa = Rimappa::new(&[&left, &right]);
        for (left, right) in [
            (left.clone(), right.clone()),
            (rimappa.batch(&left), rimappa.batch(&right)),
        ] {
            for keys in [vec![0], vec![0, 1]] {
                let names: Vec<_> = keys
                    .iter()
                    .map(|&i| if i == 0 { "g" } else { "s" })
                    .collect();
                let matches: Vec<Vec<usize>> = (0..left_rows)
                    .map(|l| {
                        let key = chiave_fixture(&left, &keys, l);
                        (0..right_rows)
                            .filter(|&r| {
                                key.iter().all(Option::is_some)
                                    && key == chiave_fixture(&right, &keys, r)
                            })
                            .collect()
                    })
                    .collect();
                for how in ["inner", "left", "right", "outer"] {
                    let result = joins::join(
                        &left,
                        &right,
                        &config(json!({"left_keys": names, "right_keys": names, "how": how})),
                        &limits,
                    );
                    if matches!(how, "right" | "outer") {
                        assert_rifiuto!(
                            result,
                            plenora_core::ErrorCategory::Schema,
                            plenora_core::ErrorPhase::Validate
                        );
                    } else {
                        let pairs: Vec<_> = matches
                            .iter()
                            .enumerate()
                            .flat_map(|(l, rs)| {
                                if rs.is_empty() && how == "left" {
                                    vec![(l, None)]
                                } else {
                                    rs.iter().map(|&r| (l, Some(r))).collect()
                                }
                            })
                            .collect();
                        assert_successo!(result, batch_coppie(&left, &right, &keys, &pairs, false));
                    }
                }
                let cfg = config(json!({"left_keys": names, "right_keys": names}));
                let semi: Vec<_> = matches
                    .iter()
                    .enumerate()
                    .filter(|(_, rs)| !rs.is_empty())
                    .map(|(l, _)| l)
                    .collect();
                let anti: Vec<_> = matches
                    .iter()
                    .enumerate()
                    .filter(|(_, rs)| rs.is_empty())
                    .map(|(l, _)| l)
                    .collect();
                assert_successo!(
                    joins::semi_join(&left, &right, &cfg),
                    crate::select_rows(&left, &semi).unwrap()
                );
                assert_successo!(
                    joins::anti_join(&left, &right, &cfg),
                    crate::select_rows(&left, &anti).unwrap()
                );
                let lv = left
                    .column(3)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap();
                let rv = right
                    .column(3)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap();
                let pairs: Vec<_> = matches
                    .iter()
                    .enumerate()
                    .map(|(l, rs)| {
                        let best = rs
                            .iter()
                            .copied()
                            .filter(|&r| rv.value(r) <= lv.value(l))
                            .max_by(|&a, &b| rv.value(a).total_cmp(&rv.value(b)).then(a.cmp(&b)));
                        (l, best)
                    })
                    .collect();
                assert_successo!(
                    joins::asof_join(
                        &left,
                        &right,
                        &config(
                            json!({"left_on": "v", "right_on": "v", "left_by": names, "right_by": names})
                        ),
                        &limits
                    ),
                    batch_coppie(&left, &right, &keys, &pairs, true)
                );
            }
        }
    }
}

/// Resoconto indipendente, una riga per chiave univoca nell'ordine documentato.
#[allow(clippy::too_many_lines)]
fn differenze_attese(
    left: &RecordBatch,
    right: &RecordBatch,
    keys: &[usize],
    include: bool,
) -> RecordBatch {
    let lk: Vec<_> = (0..left.num_rows())
        .map(|r| chiave_fixture(left, keys, r))
        .collect();
    let rk: Vec<_> = (0..right.num_rows())
        .map(|r| chiave_fixture(right, keys, r))
        .collect();
    let lv = left
        .column(3)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    let rv = right
        .column(3)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    let mut sources = Vec::new();
    let mut status = Vec::new();
    let mut changed = Vec::new();
    let mut old = Vec::new();
    for (l, key) in lk.iter().enumerate() {
        let r = rk.iter().position(|k| k == key);
        match r {
            Some(r) if lv.value(l).to_bits() != rv.value(r).to_bits() => {
                sources.push((right, r));
                status.push("MODIFIED");
                changed.push(Some("v"));
                old.push(Some(lv.value(l).to_string()));
            }
            Some(r) if include => {
                sources.push((right, r));
                status.push("UNCHANGED");
                changed.push(None);
                old.push(None);
            }
            Some(_) => {}
            None => {
                sources.push((left, l));
                status.push("DELETED");
                changed.push(None);
                old.push(None);
            }
        }
    }
    for (r, key) in rk.iter().enumerate() {
        if !lk.contains(key) {
            sources.push((right, r));
            status.push("ADDED");
            changed.push(None);
            old.push(None);
        }
    }
    let mut fields = Vec::new();
    let mut columns: Vec<ArrayRef> = Vec::new();
    for &i in keys {
        fields.push(Field::new(
            if i == 0 { "g" } else { "s" },
            if i == 0 {
                DataType::Binary
            } else {
                DataType::Utf8
            },
            true,
        ));
        if i == 0 {
            columns.push(Arc::new(BinaryArray::from(
                sources
                    .iter()
                    .map(|(b, r)| {
                        let a = b.column(0).as_any().downcast_ref::<BinaryArray>().unwrap();
                        (!a.is_null(*r)).then(|| a.value(*r))
                    })
                    .collect::<Vec<_>>(),
            )));
        } else {
            columns.push(Arc::new(StringArray::from(
                sources
                    .iter()
                    .map(|(b, r)| {
                        let a = b.column(1).as_any().downcast_ref::<StringArray>().unwrap();
                        (!a.is_null(*r)).then(|| a.value(*r))
                    })
                    .collect::<Vec<_>>(),
            )));
        }
    }
    fields.extend([
        Field::new("v", DataType::Float64, true),
        Field::new("_diff_status", DataType::Utf8, false),
        Field::new("_diff_columns", DataType::Utf8, true),
        Field::new("_diff_old_values", DataType::Utf8, true),
    ]);
    columns.extend([
        Arc::new(Float64Array::from(
            sources
                .iter()
                .map(|(b, r)| {
                    b.column(3)
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .unwrap()
                        .value(*r)
                })
                .collect::<Vec<_>>(),
        )) as ArrayRef,
        Arc::new(StringArray::from(status)),
        Arc::new(StringArray::from(changed)),
        Arc::new(StringArray::from(old)),
    ]);
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn reconcile_foreign_key_table_diff_set_operation_come_sul_testo_rimappato() {
    use plenora_core::arrow::array::UInt64Array;
    let limits = Limits::default();
    for (left_rows, right_rows, seed) in [(0_usize, 6_usize, 11_u64), (9, 0, 12), (50, 70, 13)] {
        let left = fixture(left_rows, seed, 0);
        let right = fixture(right_rows, seed.wrapping_mul(17), 1_000);
        let rimappa = Rimappa::new(&[&left, &right]);
        for (left, right) in [
            (left.clone(), right.clone()),
            (rimappa.batch(&left), rimappa.batch(&right)),
        ] {
            for keys in [vec![0], vec![0, 1]] {
                let names: Vec<_> = keys
                    .iter()
                    .map(|&i| if i == 0 { "g" } else { "s" })
                    .collect();
                let lk: Vec<_> = (0..left_rows)
                    .map(|r| chiave_fixture(&left, &keys, r))
                    .collect();
                let rk: Vec<_> = (0..right_rows)
                    .map(|r| chiave_fixture(&right, &keys, r))
                    .collect();
                for nulls_equal in [true, false] {
                    let counts = |rows: &[Vec<Option<Vec<u8>>>]| {
                        let mut counts = BTreeMap::new();
                        for key in rows {
                            if nulls_equal || key.iter().all(Option::is_some) {
                                *counts.entry(key.clone()).or_insert(0_u64) += 1;
                            }
                        }
                        counts
                    };
                    let lc = counts(&lk);
                    let rc = counts(&rk);
                    let matched: u64 = lc
                        .iter()
                        .map(|(key, &n)| n.min(*rc.get(key).unwrap_or(&0)))
                        .sum();
                    let expected = RecordBatch::try_new(
                        Arc::new(Schema::new(vec![
                            Field::new("metric", DataType::Utf8, false),
                            Field::new("value", DataType::UInt64, false),
                        ])),
                        vec![
                            Arc::new(StringArray::from(vec![
                                "matched_rows",
                                "left_only_rows",
                                "right_only_rows",
                                "left_duplicate_rows",
                                "right_duplicate_rows",
                            ])),
                            Arc::new(UInt64Array::from(vec![
                                matched,
                                u64::try_from(left_rows).unwrap() - matched,
                                u64::try_from(right_rows).unwrap() - matched,
                                lc.values().map(|n| n - 1).sum(),
                                rc.values().map(|n| n - 1).sum(),
                            ])),
                        ],
                    )
                    .unwrap();
                    assert_successo!(
                        governance::reconcile(
                            &left,
                            &right,
                            &config(
                                json!({"left_keys": names, "right_keys": names, "nulls_equal": nulls_equal})
                            ),
                            &limits
                        ),
                        expected
                    );
                }
                for allow_null in [true, false] {
                    let cfg = config(
                        json!({"left_keys": names, "right_keys": names, "allow_null": allow_null}),
                    );
                    for (right, right_keys) in [(&right, &rk), (&left, &lk)] {
                        let fails = lk.iter().any(|key| {
                            if key.iter().any(Option::is_none) {
                                !allow_null
                            } else {
                                !right_keys.contains(key)
                            }
                        });
                        let result = governance::assert_foreign_key(&left, right, &cfg, &limits);
                        if fails {
                            assert_rifiuto!(
                                result,
                                plenora_core::ErrorCategory::DataMapping,
                                plenora_core::ErrorPhase::Write
                            );
                        } else {
                            assert_successo!(result, left);
                        }
                    }
                }
                let unique = |b: &RecordBatch| {
                    crate::select_rows(
                        b,
                        &righe_distinte(b, &keys, &(0..b.num_rows()).collect::<Vec<_>>(), "first"),
                    )
                    .unwrap()
                };
                for (l, r) in [
                    (left.clone(), right.clone()),
                    (unique(&left), unique(&right)),
                ] {
                    let duplicate = [&l, &r].iter().any(|b| {
                        let keys: Vec<_> = (0..b.num_rows())
                            .map(|row| chiave_fixture(b, &keys, row))
                            .collect();
                        keys.iter()
                            .enumerate()
                            .any(|(i, key)| keys[..i].contains(key))
                    });
                    for include in ["no", "yes"] {
                        let cfg = config(
                            json!({"left_keys": names, "right_keys": names, "compare_columns": ["v"], "include_unchanged": include}),
                        );
                        let result = reshape::table_diff(&l, &r, &cfg, &limits);
                        if duplicate {
                            assert_rifiuto!(
                                result,
                                plenora_core::ErrorCategory::InvalidPlan,
                                plenora_core::ErrorPhase::Validate
                            );
                        } else {
                            assert_successo!(
                                result,
                                differenze_attese(&l, &r, &keys, include == "yes")
                            );
                        }
                    }
                }
            }
            let l = left.project(&[0, 1]).unwrap();
            let r = right.project(&[0, 1]).unwrap();
            let joined =
                plenora_core::arrow::select::concat::concat_batches(&l.schema(), [&l, &r]).unwrap();
            let lk: Vec<_> = (0..l.num_rows())
                .map(|row| chiave_fixture(&l, &[0, 1], row))
                .collect();
            let rk: Vec<_> = (0..r.num_rows())
                .map(|row| chiave_fixture(&r, &[0, 1], row))
                .collect();
            let unique = righe_distinte(
                &joined,
                &[0, 1],
                &(0..joined.num_rows()).collect::<Vec<_>>(),
                "first",
            );
            assert_successo!(
                setops::union_distinct(&l, &r, &setops::SetOperation {}, &limits),
                crate::select_rows(&joined, &unique).unwrap()
            );
            let unique_left =
                righe_distinte(&l, &[0, 1], &(0..l.num_rows()).collect::<Vec<_>>(), "first");
            let intersection: Vec<_> = unique_left
                .iter()
                .copied()
                .filter(|&row| rk.contains(&lk[row]))
                .collect();
            let except: Vec<_> = unique_left
                .iter()
                .copied()
                .filter(|&row| !rk.contains(&lk[row]))
                .collect();
            assert_successo!(
                setops::intersect(&l, &r, &setops::SetOperation {}),
                crate::select_rows(&l, &intersection).unwrap()
            );
            assert_successo!(
                setops::except(&l, &r, &setops::SetOperation {}),
                crate::select_rows(&l, &except).unwrap()
            );
        }
    }
}

/// Byte della chiave di `row_key` con i byte grezzi al posto del testo per
/// le colonne Binary: la definizione dell'ordine canonico dei gruppi.
fn chiave_estesa(batch: &RecordBatch, indices: &[usize], row: usize) -> Vec<u8> {
    let mut key = Vec::new();
    for index in indices {
        let column = batch.column(*index);
        key.extend_from_slice(column.data_type().to_string().as_bytes());
        key.push(0x1e);
        let valore: Option<Vec<u8>> = if column.is_null(row) {
            None
        } else if let Some(values) = column.as_any().downcast_ref::<BinaryArray>() {
            Some(values.value(row).to_vec())
        } else {
            crate::scalar_as_string(column.as_ref(), row)
                .expect("testo")
                .map(String::into_bytes)
        };
        match valore {
            Some(valore) => {
                key.push(b'1');
                key.extend_from_slice(valore.len().to_string().as_bytes());
                key.push(b':');
                key.extend_from_slice(&valore);
            }
            None => key.push(b'0'),
        }
        key.push(0x1f);
    }
    key
}

#[test]
fn aggregate_ordina_i_gruppi_sui_byte_della_chiave() {
    for (rows, seed) in [(1_usize, 21_u64), (30, 22), (500, 23), (5_000, 24)] {
        let batch = fixture(rows, seed, 0);
        for group_by in [vec!["g"], vec!["g", "s"], vec!["s", "g"]] {
            let indices = group_by
                .iter()
                .map(|name| batch.schema().index_of(name).expect("colonna"))
                .collect::<Vec<_>>();
            // Atteso: gruppi nell'ordine dei byte della chiave estesa, righe
            // di ogni gruppo in ordine crescente.
            let mut attesi: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
            for row in 0..batch.num_rows() {
                attesi
                    .entry(chiave_estesa(&batch, &indices, row))
                    .or_default()
                    .push(row);
            }
            let cfg: Aggregate = config(json!({
                "group_by": group_by,
                "aggregations": [
                    {"column": "id", "function": "concat", "separator": "|", "alias": "righe"},
                    {"column": "id", "function": "count", "alias": "n"}
                ]
            }));
            let output = aggregation::aggregate(&batch, &cfg).expect("aggregate");
            let righe = output
                .column_by_name("righe")
                .expect("righe")
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("concat")
                .iter()
                .map(|v| v.expect("concat non nullo").to_owned())
                .collect::<Vec<_>>();
            let concat_attesi = attesi
                .values()
                .map(|rows| {
                    rows.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("|")
                })
                .collect::<Vec<_>>();
            assert_eq!(righe, concat_attesi, "group_by {group_by:?}, {rows} righe");
            // Le colonne di gruppo sono quelle della prima riga del gruppo.
            let primi = attesi.values().map(|rows| rows[0]).collect::<Vec<_>>();
            let rappresentanti = crate::select_rows(&batch, &primi).expect("righe");
            for (posizione, name) in group_by.iter().enumerate() {
                assert_eq!(
                    output.column(posizione).as_ref(),
                    rappresentanti
                        .column_by_name(name)
                        .expect("colonna")
                        .as_ref()
                );
            }
        }
    }
}

#[test]
fn su_binari_utf8_validi_la_chiave_estesa_e_quella_testuale() {
    // Sanita' dell'oracolo d'ordine: senza valori non UTF-8 la chiave
    // estesa e' byte per byte quella di `row_key`, e l'ordine atteso e'
    // quello che il percorso testuale dava.
    let valori: Vec<Option<&[u8]>> = vec![
        Some(b"a"),
        None,
        Some(b""),
        Some(b"aaaaaaaaaa"),
        Some(b"\xc3\xa4"),
        Some(b"b"),
    ];
    let batch = crate::test_support::nullable_batch(vec![(
        "g",
        Arc::new(BinaryArray::from(valori)) as ArrayRef,
    )]);
    for row in 0..batch.num_rows() {
        let testo = aggregation::row_key_per_test(&batch, &[0], row).expect("testo");
        assert_eq!(chiave_estesa(&batch, &[0], row), testo.into_bytes());
    }
}
