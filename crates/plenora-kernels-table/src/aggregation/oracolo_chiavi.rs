//! Oracolo delle chiavi binarie di raggruppamento.
//!
//! Il percorso di riferimento e' quello testuale di `row_key` (i byte di
//! `scalar_as_string` con prefisso di tipo e lunghezza): la chiave binaria di
//! [`BinaryKeyEncoder`] e gli indici di [`visit_key_ids`] devono avere la
//! stessa identita' e gli stessi errori, e `aggregate`/`distinct` lo stesso
//! output byte per byte degli oracoli `aggregate_reference` e
//! `distinct_reference` (in `tests` di `mod.rs`, che passano da `row_key`).
//!
//! I dati sono pseudocasuali ma deterministici (xorshift a seme fisso): un
//! fallimento si riproduce sempre uguale.

use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray,
    TimestampMillisecondArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};

use super::compare::row_key;
use super::grouping::{visit_key_ids, BinaryKeyEncoder};
use super::{aggregate, distinct, AggFunction, Aggregate, Aggregation, Distinct, Keep};
use crate::test_support::assert_batches_identical;
use plenora_core::PlenoraError;

/// Generatore xorshift64 a seme fisso.
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

    fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len())]
    }
}

/// Double che il testo distingue o confonde: NaN con payload e segni
/// diversi (un solo testo `NaN`), zeri con segno (`-0` e `0`), infiniti,
/// valori a esponente.
const FLOATS: [Option<f64>; 12] = [
    None,
    Some(f64::NAN),
    Some(-f64::NAN),
    Some(-0.0),
    Some(0.0),
    Some(1.5),
    Some(-2.25),
    Some(f64::INFINITY),
    Some(f64::NEG_INFINITY),
    Some(1e21),
    Some(0.1),
    Some(5e-324),
];

/// Testi che una chiave concatenata senza cornice confonderebbe: ("ab","c")
/// e ("a","bc"), vuoto e null.
const TEXTS: [Option<&str>; 22] = [
    None,
    Some(""),
    Some("a"),
    Some("ab"),
    Some("b"),
    Some("bc"),
    Some("c"),
    Some("abc"),
    Some("\u{e4}"),
    // Testi che imitano i delimitatori della chiave testuale (`\u{1e}`,
    // `\u{1f}`, il marcatore `0` del null, il tag `{len}:`) e della chiave
    // binaria (byte 0 e 1 del marcatore, una lunghezza big-endian a 8 byte),
    // il testo "null" e testi prefissi l'uno dell'altro.
    Some("null"),
    Some("0"),
    Some("1"),
    Some("1:a"),
    Some("0\u{1f}"),
    Some("\u{1e}"),
    Some("\u{1f}"),
    Some("a\u{1f}Utf8\u{1e}1"),
    Some("\u{0}"),
    Some("\u{1}"),
    Some("\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{1}a"),
    Some("NaN"),
    Some("abcd"),
];

/// NaN con payload non canonico, per verificare che ogni NaN sia lo stesso
/// valore di chiave.
fn nan_con_payload(payload: u64) -> f64 {
    f64::from_bits(0x7ff8_0000_0000_0000 | (payload & 0x0007_ffff_ffff_ffff))
}

/// Batch con una colonna per tipo di chiave, null ovunque, molti duplicati.
/// `wide` e' un Int64 ad alta cardinalita' (`0..wide_range`) per portare il
/// raggruppamento multi-colonna oltre le soglie della forma compatta e
/// dell'ordinamento parallelo.
#[allow(clippy::too_many_lines)] // Fixture: una colonna esplicita per tipo.
fn fixture(rows: usize, seed: u64, wide_range: u64) -> RecordBatch {
    let mut rng = Xorshift(seed | 1);
    let mut ints = Vec::with_capacity(rows);
    let mut uints = Vec::with_capacity(rows);
    let mut floats = Vec::with_capacity(rows);
    let mut texts = Vec::with_capacity(rows);
    let mut bools = Vec::with_capacity(rows);
    let mut dates = Vec::with_capacity(rows);
    let mut stamps = Vec::with_capacity(rows);
    let mut decimals = Vec::with_capacity(rows);
    let mut binaries: Vec<Option<&[u8]>> = Vec::with_capacity(rows);
    let mut dictionary_keys = Vec::with_capacity(rows);
    let mut wides = Vec::with_capacity(rows);
    let mut values = Vec::with_capacity(rows);
    for row in 0..rows {
        ints.push(rng.pick(&[
            None,
            Some(-10_i64),
            Some(-1),
            Some(0),
            Some(7),
            Some(10),
            Some(i64::MIN),
            Some(i64::MAX),
        ]));
        uints.push(rng.pick(&[None, Some(0_u64), Some(9), Some(10), Some(u64::MAX)]));
        let float = rng.pick(&FLOATS);
        // Meta' dei NaN con un payload diverso: stesso testo, bit diversi.
        floats.push(match float {
            Some(value) if value.is_nan() && rng.below(2) == 0 => Some(nan_con_payload(rng.next())),
            other => other,
        });
        texts.push(rng.pick(&TEXTS));
        bools.push(rng.pick(&[None, Some(true), Some(false)]));
        dates.push(rng.pick(&[
            None,
            Some(-719_162_i32),
            Some(-1),
            Some(0),
            Some(19_000),
            Some(2_932_896),
        ]));
        // Attorno al cambio d'ora di Europe/Rome (2024-10-27 01:00 UTC): due
        // istanti con la stessa ora locale e offset diverso.
        stamps.push(rng.pick(&[
            None,
            Some(1_729_989_000_000_i64),
            Some(1_729_992_600_000),
            Some(0),
            Some(-1),
        ]));
        decimals.push(rng.pick(&[
            None,
            Some(-5_i128),
            Some(5),
            Some(-100),
            Some(100),
            Some(0),
            Some(12_345),
        ]));
        binaries.push(rng.pick(&[
            None,
            Some(&b""[..]),
            Some(&b"ab"[..]),
            Some(&b"a"[..]),
            Some(&b"bc"[..]),
        ]));
        dictionary_keys.push(rng.pick(&[None, Some(0_i32), Some(1), Some(2), Some(3)]));
        wides.push(Some(
            i64::try_from(rng.next() % wide_range.max(1)).expect("wide"),
        ));
        values.push(Some(
            f64::from(u32::try_from(row % 1_000).expect("valore")) * 0.25,
        ));
    }
    // Dizionario con una entry nulla: la chiave 2 e' un null logico.
    let dictionary = DictionaryArray::<Int32Type>::try_new(
        Int32Array::from(dictionary_keys),
        Arc::new(StringArray::from(vec![
            Some("x"),
            Some(""),
            None,
            Some("xy"),
        ])),
    )
    .expect("dizionario");
    let ids = (0..rows)
        .map(|row| i64::try_from(row).expect("id"))
        .collect::<Vec<_>>();
    let rome = DataType::Timestamp(TimeUnit::Millisecond, Some("Europe/Rome".into()));
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("i", DataType::Int64, true),
            Field::new("u", DataType::UInt64, true),
            Field::new("f", DataType::Float64, true),
            Field::new("s", DataType::Utf8, true),
            Field::new("b", DataType::Boolean, true),
            Field::new("d", DataType::Date32, true),
            Field::new("t", rome, true),
            Field::new("dec", DataType::Decimal128(10, 2), true),
            Field::new("bin", DataType::Binary, true),
            Field::new(
                "dict",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                true,
            ),
            Field::new("wide", DataType::Int64, true),
            Field::new("val", DataType::Float64, true),
            Field::new("id", DataType::Int64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ints)),
            Arc::new(UInt64Array::from(uints)),
            Arc::new(Float64Array::from(floats)),
            Arc::new(StringArray::from(texts)),
            Arc::new(BooleanArray::from(bools)),
            Arc::new(Date32Array::from(dates)),
            Arc::new(
                TimestampMillisecondArray::from(stamps).with_timezone_opt(Some("Europe/Rome")),
            ),
            Arc::new(
                Decimal128Array::from(decimals)
                    .with_precision_and_scale(10, 2)
                    .expect("decimal"),
            ),
            Arc::new(BinaryArray::from(binaries)),
            Arc::new(dictionary),
            Arc::new(Int64Array::from(wides)),
            Arc::new(Float64Array::from(values)),
            Arc::new(Int64Array::from(ids)),
        ],
    )
    .expect("fixture chiavi")
}

const KEY_COLUMNS: [&str; 10] = ["i", "u", "f", "s", "b", "d", "t", "dec", "bin", "dict"];

fn indices_of(batch: &RecordBatch, names: &[&str]) -> Vec<usize> {
    names
        .iter()
        .map(|name| batch.schema().index_of(name).expect("colonna"))
        .collect()
}

/// Sottoinsiemi di chiave: ogni colonna da sola, coppie adiacenti,
/// terne e sottoinsiemi pseudocasuali in ordine permutato.
fn subsets(rng: &mut Xorshift) -> Vec<Vec<&'static str>> {
    let mut output = KEY_COLUMNS
        .iter()
        .map(|name| vec![*name])
        .collect::<Vec<_>>();
    output.extend(KEY_COLUMNS.windows(2).map(<[&str]>::to_vec));
    output.push(vec!["s", "s"]);
    output.push(vec!["f", "s", "i"]);
    output.push(vec!["dict", "bin", "s"]);
    for _ in 0..12 {
        let mut subset = Vec::new();
        for _ in 0..=rng.below(4) {
            subset.push(rng.pick(&KEY_COLUMNS));
        }
        output.push(subset);
    }
    output
}

/// Identita' della chiave binaria contro quella testuale, a coppie di righe.
fn assert_same_identity(batch: &RecordBatch, names: &[&str]) {
    let indices = indices_of(batch, names);
    let encoder = BinaryKeyEncoder::new(batch, &indices);
    let mut binary = Vec::with_capacity(batch.num_rows());
    let mut text = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let mut key = Vec::new();
        encoder.encode_into(row, &mut key).expect("chiave binaria");
        binary.push(key);
        text.push(row_key(batch, &indices, row).expect("chiave testuale"));
    }
    for left in 0..batch.num_rows() {
        for right in left..batch.num_rows() {
            assert_eq!(
                binary[left] == binary[right],
                text[left] == text[right],
                "identita' diversa su {names:?} fra le righe {left} e {right}"
            );
        }
    }
    // Gli indici di `visit_key_ids` (ramo nativo o arena) seguono la stessa
    // identita', densi e in ordine di prima apparizione.
    let mut first_seen: Vec<String> = Vec::new();
    visit_key_ids(batch, &indices, |row, indice, nuova| {
        if let Some(position) = first_seen.iter().position(|key| *key == text[row]) {
            assert!(!nuova, "chiave gia' vista marcata nuova alla riga {row}");
            assert_eq!(indice, position, "indice di chiave alla riga {row}");
        } else {
            assert!(nuova, "chiave nuova non marcata alla riga {row}");
            assert_eq!(indice, first_seen.len(), "indice denso alla riga {row}");
            first_seen.push(text[row].clone());
        }
        Ok(())
    })
    .expect("visita");
}

#[test]
fn la_chiave_binaria_ha_l_identita_di_row_key_su_ogni_tipo() {
    let batch = fixture(160, 0x5eed, 5);
    let mut rng = Xorshift(0x00c0_ffee);
    for subset in subsets(&mut rng) {
        assert_same_identity(&batch, &subset);
    }
}

#[test]
fn nan_e_zeri_con_segno_hanno_l_identita_del_testo() {
    // Bit pseudocasuali piu' i vicini immediati: `Display` e' iniettivo sui
    // non-NaN e scrive `NaN` per ogni NaN, esattamente come i bit canonici.
    let mut rng = Xorshift(0xf10a7);
    let mut values = vec![
        0.0,
        -0.0,
        f64::NAN,
        -f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for _ in 0..400 {
        let value = f64::from_bits(rng.next());
        values.push(value);
        values.push(value.next_up());
        values.push(value.next_down());
    }
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("f", DataType::Float64, false)])),
        vec![Arc::new(Float64Array::from(values))],
    )
    .expect("batch float");
    assert_same_identity(&batch, &["f"]);
}

/// Stesso esito riga per riga: chiave uguale o stesso messaggio d'errore.
fn assert_same_errors(column: &ArrayRef) {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", column.data_type().clone(), true),
            Field::new("s", DataType::Utf8, true),
        ])),
        vec![
            column.clone(),
            Arc::new(StringArray::from(vec![Some("x"); column.len()])),
        ],
    )
    .expect("batch errori");
    for indices in [vec![0], vec![1, 0], vec![0, 1]] {
        let encoder = BinaryKeyEncoder::new(&batch, &indices);
        for row in 0..batch.num_rows() {
            let mut key = Vec::new();
            let binary = encoder
                .encode_into(row, &mut key)
                .map_err(|e| e.to_string());
            let text = row_key(&batch, &indices, row).map_err(|e| e.to_string());
            assert_eq!(
                binary.err(),
                text.err(),
                "esito diverso alla riga {row} su {:?}",
                column.data_type()
            );
        }
    }
}

#[test]
fn la_chiave_binaria_ha_gli_errori_di_row_key() {
    // Il Binary non UTF-8 non e' piu' un errore della chiave: i suoi byte
    // sono la chiave (`crate::binari_oracolo`).
    let colonne: [ArrayRef; 3] = [
        // Tipo fuori dal profilo scalare: errore solo sulle celle non nulle.
        Arc::new(Int32Array::from(vec![None, Some(1), None])),
        // Timezone Arrow non valida.
        Arc::new(TimestampMillisecondArray::from(vec![None, Some(0)]).with_timezone("Marte/Base")),
        // Date32 fuori dall'intervallo di chrono.
        Arc::new(Date32Array::from(vec![Some(0), Some(i32::MAX)])),
    ];
    for colonna in &colonne {
        assert_same_errors(colonna);
    }
}

fn aggregations() -> Vec<Aggregation> {
    let aggregation = |column: &str, function: AggFunction, alias: &str| Aggregation {
        column: column.into(),
        function,
        separator: matches!(function, AggFunction::Concat).then(|| "|".into()),
        distinct: None,
        skip_null: None,
        alias: alias.into(),
        quantile: None,
        ddof: None,
    };
    vec![
        // Le righe di ogni gruppo, in ordine: `concat` degli id le fissa
        // tutte, non solo la prima.
        aggregation("id", AggFunction::Concat, "righe"),
        aggregation("id", AggFunction::Count, "conta"),
        aggregation("s", AggFunction::First, "primo"),
        aggregation("s", AggFunction::Last, "ultimo"),
        aggregation("val", AggFunction::Sum, "somma"),
    ]
}

fn assert_aggregate_and_distinct_parity(batch: &RecordBatch, subset: &[&str]) {
    let group_by = subset
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let config = Aggregate {
        group_by: group_by.clone(),
        aggregations: aggregations(),
    };
    let ripetute = group_by
        .iter()
        .enumerate()
        .any(|(indice, nome)| group_by[..indice].contains(nome));
    if ripetute {
        // Una chiave ripetuta darebbe due colonne d'uscita con lo stesso
        // nome: `aggregate` la rifiuta (lo fa gia' l'analisi); `distinct`
        // sotto la accetta, perche' non produce colonne.
        assert!(matches!(
            aggregate(batch, &config),
            Err(PlenoraError::InvalidPlan(_))
        ));
    } else {
        let reference = super::tests::aggregate_reference(batch, &config).expect("riferimento");
        let fast = aggregate(batch, &config).expect("aggregate");
        assert_batches_identical(&fast, &reference);
    }
    for keep in [Keep::First, Keep::Last, Keep::False] {
        let config = Distinct {
            subset: group_by.clone(),
            keep,
        };
        let reference = super::tests::distinct_reference(batch, &config).expect("riferimento");
        let fast = distinct(batch, &config).expect("distinct");
        assert_batches_identical(&fast, &reference);
    }
}

#[test]
fn aggregate_e_distinct_coincidono_con_il_percorso_testuale() {
    let mut rng = Xorshift(0xa66);
    for (rows, seed) in [(0_usize, 1_u64), (1, 2), (2, 3), (57, 4), (400, 5)] {
        let batch = fixture(rows, seed, 7);
        for subset in subsets(&mut rng) {
            assert_aggregate_and_distinct_parity(&batch, &subset);
        }
    }
}

#[test]
fn aggregate_e_distinct_coincidono_oltre_le_soglie_di_forma_e_parallelismo() {
    // 70_000 righe, `wide` fino a 60_000 valori: gruppi oltre la soglia della
    // forma compatta (70_000 / 16) e oltre quella dell'ordinamento
    // parallelo (32_768), sia sul ramo nativo sia sull'arena.
    let batch = fixture(70_000, 0xb16, 60_000);
    for subset in [
        vec!["wide"],
        vec!["wide", "f"],
        vec!["s", "wide"],
        vec!["f"],
        vec!["dict", "s"],
    ] {
        assert_aggregate_and_distinct_parity(&batch, &subset);
    }
}

#[test]
fn chiavi_con_errore_di_conversione_falliscono_come_il_percorso_testuale() {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", DataType::Date32, true),
            Field::new("id", DataType::Int64, false),
        ])),
        vec![
            // Date32 fuori dall'intervallo di chrono solo all'ultima riga
            // (il Binary non UTF-8 che c'era qui ora ha una chiave).
            Arc::new(Date32Array::from(vec![Some(0), None, Some(i32::MAX)])),
            Arc::new(Int64Array::from(vec![0, 1, 2])),
        ],
    )
    .expect("batch");
    let config = Aggregate {
        group_by: vec!["k".into()],
        aggregations: Vec::new(),
    };
    let reference = super::tests::aggregate_reference(&batch, &config)
        .expect_err("riferimento")
        .to_string();
    let fast = aggregate(&batch, &config)
        .expect_err("aggregate")
        .to_string();
    assert_eq!(fast, reference);
    let config = Distinct {
        subset: vec!["k".into()],
        keep: Keep::First,
    };
    let reference = super::tests::distinct_reference(&batch, &config)
        .expect_err("riferimento")
        .to_string();
    let fast = distinct(&batch, &config).expect_err("distinct").to_string();
    assert_eq!(fast, reference);
}

#[test]
fn con_due_colonne_in_errore_vince_la_prima_colonna_come_nel_testo() {
    // Stessa riga, due colonne che falliscono con errori diversi: l'ordine
    // delle colonne nella chiave decide quale errore esce, in entrambi i
    // percorsi.
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("date", DataType::Date32, true),
            Field::new("int32", DataType::Int32, true),
        ])),
        vec![
            Arc::new(Date32Array::from(vec![Some(i32::MAX), None])),
            Arc::new(Int32Array::from(vec![Some(1), Some(2)])),
        ],
    )
    .expect("batch");
    for indices in [vec![0, 1], vec![1, 0]] {
        let encoder = BinaryKeyEncoder::new(&batch, &indices);
        for row in 0..batch.num_rows() {
            let mut key = Vec::new();
            let binary = encoder
                .encode_into(row, &mut key)
                .map_err(|e| e.to_string());
            let text = row_key(&batch, &indices, row).map_err(|e| e.to_string());
            assert!(binary.is_err(), "la riga {row} deve fallire");
            assert_eq!(binary.err(), text.err(), "colonne {indices:?}, riga {row}");
        }
    }
}
