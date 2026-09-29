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
//!   del testo. Il percorso spilled (che ordina sugli stessi byte con
//!   `KeyColumn`) deve coincidere con quello in memoria.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde_json::{json, Value};

use crate::aggregation::{self, Aggregate};
use crate::test_support::assert_same_outcome_bits;
use crate::{governance, joins, quality, reshape, setops, spill, Limits};

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
    let v = (0..rows)
        .map(|_| f64::from(u32::try_from(rng.below(5)).expect("v")))
        .collect::<Vec<_>>();
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

    fn esito(&self, esito: plenora_core::Result<RecordBatch>) -> plenora_core::Result<RecordBatch> {
        esito.map(|batch| self.batch(&batch))
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
    assert!((0..batch.num_rows()).any(|row| crate::scalar_as_string(
        batch.column(0).as_ref(),
        row
    )
    .is_err()));
    aggregation::distinct(&batch, &config(json!({"subset": ["g"]}))).expect("distinct");
    aggregation::dedup_advanced(&batch, &config(json!({"subset": ["g"]}))).expect("dedup");
    aggregation::aggregate(&batch, &config(json!({"group_by": ["g"]}))).expect("aggregate");
    let unico = fixture(1, 7, 0);
    quality::assert_unique(&unico, &config(json!({"columns": ["g"]}))).expect("assert_unique");
}

#[test]
fn distinct_dedup_assert_unique_come_sul_testo_rimappato() {
    for (rows, seed) in [(0_usize, 1_u64), (1, 2), (13, 3), (200, 4), (1_000, 5)] {
        let batch = fixture(rows, seed, 0);
        let rimappa = Rimappa::new(&[&batch]);
        let rimappato = rimappa.batch(&batch);
        for subset in [
            json!(["g"]),
            json!(["g", "s"]),
            json!(["s", "g"]),
            json!([]),
        ] {
            for keep in ["first", "last", "false"] {
                let cfg = json!({"subset": subset, "keep": keep});
                assert_same_outcome_bits(
                    rimappa.esito(aggregation::distinct(&batch, &config(cfg.clone()))),
                    aggregation::distinct(&rimappato, &config(cfg.clone())),
                );
                let limits = Limits {
                    spill_partitions: 3,
                    ..Limits::default()
                };
                assert_same_outcome_bits(
                    rimappa.esito(
                        spill::distinct_spilled(&batch, &config(cfg.clone()), &limits)
                            .map(|(batch, _)| batch),
                    ),
                    aggregation::distinct(&rimappato, &config(cfg)),
                );
            }
            for cfg in [
                json!({"subset": subset, "keep": "first"}),
                json!({"subset": subset, "keep": "last", "order_column": "v"}),
                json!({"subset": subset, "keep": "first", "order_column": "v", "ascending": false}),
            ] {
                assert_same_outcome_bits(
                    rimappa.esito(aggregation::dedup_advanced(&batch, &config(cfg.clone()))),
                    aggregation::dedup_advanced(&rimappato, &config(cfg)),
                );
            }
        }
        for columns in [json!(["g"]), json!(["g", "s"])] {
            for nulls_equal in [true, false] {
                let cfg = json!({"columns": columns, "nulls_equal": nulls_equal});
                // Con duplicati: stesso errore e stessa diagnostica di riga.
                assert_same_outcome_bits(
                    rimappa.esito(quality::assert_unique(&batch, &config(cfg.clone()))),
                    quality::assert_unique(&rimappato, &config(cfg.clone())),
                );
                // Senza duplicati: il batch passa invariato.
                let distinti = aggregation::distinct(
                    &batch,
                    &config(json!({"subset": columns, "keep": "first"})),
                )
                .expect("distinct");
                let rimappa = Rimappa::new(&[&distinti]);
                assert_same_outcome_bits(
                    rimappa.esito(quality::assert_unique(&distinti, &config(cfg.clone()))),
                    quality::assert_unique(&rimappa.batch(&distinti), &config(cfg)),
                );
            }
        }
    }
}

#[test]
fn join_semi_anti_asof_come_sul_testo_rimappato() {
    let limits = Limits::default();
    for (left_rows, right_rows, seed) in [(0_usize, 5_usize, 1_u64), (17, 0, 2), (60, 45, 3)] {
        let left = fixture(left_rows, seed, 0);
        let right = fixture(right_rows, seed.wrapping_mul(31), 1_000);
        let rimappa = Rimappa::new(&[&left, &right]);
        let (left_r, right_r) = (rimappa.batch(&left), rimappa.batch(&right));
        for keys in [json!(["g"]), json!(["g", "s"])] {
            for how in ["inner", "left", "right", "outer"] {
                let cfg = json!({"left_keys": keys, "right_keys": keys, "how": how});
                assert_same_outcome_bits(
                    rimappa.esito(joins::join(&left, &right, &config(cfg.clone()), &limits)),
                    joins::join(&left_r, &right_r, &config(cfg), &limits),
                );
            }
            let cfg = json!({"left_keys": keys, "right_keys": keys});
            assert_same_outcome_bits(
                rimappa.esito(joins::semi_join(&left, &right, &config(cfg.clone()))),
                joins::semi_join(&left_r, &right_r, &config(cfg.clone())),
            );
            assert_same_outcome_bits(
                rimappa.esito(joins::anti_join(&left, &right, &config(cfg.clone()))),
                joins::anti_join(&left_r, &right_r, &config(cfg)),
            );
            let cfg = json!({"left_on": "v", "right_on": "v", "left_by": keys, "right_by": keys});
            assert_same_outcome_bits(
                rimappa.esito(joins::asof_join(
                    &left,
                    &right,
                    &config(cfg.clone()),
                    &limits,
                )),
                joins::asof_join(&left_r, &right_r, &config(cfg), &limits),
            );
        }
    }
}

#[test]
fn reconcile_foreign_key_table_diff_set_operation_come_sul_testo_rimappato() {
    let limits = Limits::default();
    for (left_rows, right_rows, seed) in [(0_usize, 6_usize, 11_u64), (9, 0, 12), (50, 70, 13)] {
        let left = fixture(left_rows, seed, 0);
        let right = fixture(right_rows, seed.wrapping_mul(17), 1_000);
        let rimappa = Rimappa::new(&[&left, &right]);
        let (left_r, right_r) = (rimappa.batch(&left), rimappa.batch(&right));
        for keys in [json!(["g"]), json!(["g", "s"])] {
            for nulls_equal in [true, false] {
                let cfg =
                    json!({"left_keys": keys, "right_keys": keys, "nulls_equal": nulls_equal});
                assert_same_outcome_bits(
                    governance::reconcile(&left, &right, &config(cfg.clone()), &limits),
                    governance::reconcile(&left_r, &right_r, &config(cfg), &limits),
                );
            }
            for allow_null in [true, false] {
                let cfg = json!({"left_keys": keys, "right_keys": keys, "allow_null": allow_null});
                assert_same_outcome_bits(
                    rimappa.esito(governance::assert_foreign_key(
                        &left,
                        &right,
                        &config(cfg.clone()),
                        &limits,
                    )),
                    governance::assert_foreign_key(&left_r, &right_r, &config(cfg), &limits),
                );
                // Lato destro che contiene il sinistro: la chiave c'e'.
                let cfg = json!({"left_keys": keys, "right_keys": keys, "allow_null": allow_null});
                assert_same_outcome_bits(
                    rimappa.esito(governance::assert_foreign_key(
                        &left,
                        &left,
                        &config(cfg.clone()),
                        &limits,
                    )),
                    governance::assert_foreign_key(&left_r, &left_r, &config(cfg), &limits),
                );
            }
            // Chiavi uniche per lato (altrimenti `table_diff` rifiuta, e
            // anche quel rifiuto deve coincidere).
            let unici = |batch: &RecordBatch| {
                aggregation::distinct(batch, &config(json!({"subset": keys}))).expect("distinct")
            };
            for (l, r) in [(left.clone(), right.clone()), (unici(&left), unici(&right))] {
                let (l_r, r_r) = (rimappa.batch(&l), rimappa.batch(&r));
                for include in ["no", "yes"] {
                    let cfg = json!({"left_keys": keys, "right_keys": keys,
                                     "compare_columns": ["v"], "include_unchanged": include});
                    assert_same_outcome_bits(
                        rimappa.esito(reshape::table_diff(&l, &r, &config(cfg.clone()), &limits)),
                        reshape::table_diff(&l_r, &r_r, &config(cfg), &limits),
                    );
                }
            }
        }
        // Set operation su (g, s): le chiavi compatte erano gia' in byte.
        let proietta = |batch: &RecordBatch| batch.project(&[0, 1]).expect("proiezione");
        let (l, r) = (proietta(&left), proietta(&right));
        let (l_r, r_r) = (rimappa.batch(&l), rimappa.batch(&r));
        let vuota = setops::SetOperation {};
        assert_same_outcome_bits(
            rimappa.esito(setops::union_distinct(&l, &r, &vuota, &limits)),
            setops::union_distinct(&l_r, &r_r, &vuota, &limits),
        );
        assert_same_outcome_bits(
            rimappa.esito(setops::intersect(&l, &r, &vuota)),
            setops::intersect(&l_r, &r_r, &vuota),
        );
        assert_same_outcome_bits(
            rimappa.esito(setops::except(&l, &r, &vuota)),
            setops::except(&l_r, &r_r, &vuota),
        );
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
            // Spilled: stesso ordine dai byte di `KeyColumn`.
            for partitions in [1_usize, 4] {
                let limits = Limits {
                    spill_partitions: partitions,
                    ..Limits::default()
                };
                let (spilled, _) =
                    spill::aggregate_spilled(&batch, &cfg, &limits).expect("aggregate spilled");
                assert_same_outcome_bits(Ok(spilled), Ok(output.clone()));
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
