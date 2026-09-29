//! Oracolo di `pivot` con riduzione per cella in streaming.
//!
//! Riferimento: `pivot_reference` (in `tests` di `reshape.rs`), il percorso
//! testuale indipendente con `BTreeMap` di chiavi composte e il vettore di
//! righe per cella ridotto alla fine (`Iterator::sum`, `reduce(f64::min)`,
//! `join(",")`). Si confronta l'esito intero su input generati: batch byte
//! per byte (i `Float64` per bit), oppure categoria, messaggio e
//! diagnostica dell'errore. Gli input mettono insieme tutto cio' che una
//! riduzione diversa renderebbe visibile: somme che dipendono dall'ordine
//! (`1e308`, `0.1`, infiniti), NaN con payload e segno, zeri con segno,
//! interi oltre 2^53, testi non numerici (errore di conversione in una
//! cella), nomi di output invalidi, null ovunque, chiavi multi-colonna,
//! `mapping` che rinomina e filtra.

use std::collections::BTreeMap;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use proptest::prelude::*;

use super::tests::pivot_reference;
use super::{pivot, Pivot, PivotAgg};
use crate::test_support::{assert_same_outcome_bits, test_lunghi};

/// Come `assert_same_outcome_bits`, ma per `sum` e `mean` due NaN nella
/// stessa cella delle colonne dei valori pivot sono uguali qualunque sia il
/// payload.
///
/// Il payload del NaN risultato di una somma di NaN con payload diversi non
/// e' specificato da Rust (RFC 3514): dipende da come il compilatore ordina
/// gli operandi, e nel percorso precedente cambiava gia' fra debug
/// (`NaN(p1)`) e release (`NaN(p0)`) sullo stesso input. Ovunque non ci sia
/// aritmetica (colonne indice, `first`, `last`, `min`, `max`, `count`,
/// `concat`) il confronto resta sui bit.
fn assert_esiti_equivalenti(
    config: &Pivot,
    veloce: plenora_core::Result<RecordBatch>,
    riferimento: plenora_core::Result<RecordBatch>,
) {
    let aritmetica = matches!(config.aggr_func, PivotAgg::Sum | PivotAgg::Mean);
    let colonne_indice = config
        .index_col
        .split(',')
        .filter(|nome| !nome.trim().is_empty())
        .count();
    match (veloce, riferimento) {
        (Ok(veloce), Ok(riferimento)) if aritmetica => {
            let canonico = |batch: &RecordBatch| {
                let colonne = batch
                    .columns()
                    .iter()
                    .enumerate()
                    .map(|(posizione, colonna)| -> ArrayRef {
                        match colonna
                            .as_any()
                            .downcast_ref::<Float64Array>()
                            .filter(|_| posizione >= colonne_indice)
                        {
                            Some(valori) => Arc::new(
                                valori
                                    .iter()
                                    .map(|v| v.map(|v| if v.is_nan() { f64::NAN } else { v }))
                                    .collect::<Float64Array>(),
                            ),
                            None => colonna.clone(),
                        }
                    })
                    .collect::<Vec<_>>();
                crate::batch_with_rows(batch.schema(), colonne, batch.num_rows()).expect("batch")
            };
            assert_same_outcome_bits(Ok(canonico(&veloce)), Ok(canonico(&riferimento)));
        }
        (veloce, riferimento) => assert_same_outcome_bits(veloce, riferimento),
    }
}
use crate::Limits;
use plenora_core::PlenoraError;

const K1: [Option<i64>; 6] = [None, Some(0), Some(-1), Some(10), Some(9), Some(i64::MAX)];
const K2: [Option<&str>; 7] = [
    None,
    Some(""),
    Some("a"),
    Some("ab"),
    Some("10"),
    Some("9"),
    Some("b"),
];
const KF: [Option<f64>; 7] = [
    None,
    Some(f64::NAN),
    Some(-0.0),
    Some(0.0),
    Some(f64::INFINITY),
    Some(1.5),
    Some(-2.0),
];
const P: [Option<&str>; 6] = [None, Some("x"), Some("y"), Some("z"), Some("10"), Some("9")];
const PI: [Option<i64>; 5] = [None, Some(1), Some(2), Some(10), Some(-3)];
/// Valori pivot con nomi di output invalidi (vuoto, solo spazi).
const PBAD: [Option<&str>; 4] = [Some("x"), Some(""), Some("  "), None];
const V: [Option<f64>; 12] = [
    None,
    Some(0.1),
    Some(0.2),
    Some(1e308),
    Some(-1e308),
    Some(-0.0),
    Some(0.0),
    Some(f64::INFINITY),
    Some(f64::NEG_INFINITY),
    Some(f64::NAN),
    Some(-3.25),
    Some(1e-310),
];
const VI: [Option<i64>; 6] = [
    None,
    Some(1),
    Some(9_007_199_254_740_993),
    Some(i64::MAX),
    Some(-7),
    Some(0),
];
const VS: [Option<&str>; 6] = [
    None,
    Some("1.5"),
    Some(""),
    Some("abc"),
    Some("-0"),
    Some("x,y"),
];

fn nan_con_payload(payload: u64, negativo: bool) -> f64 {
    let bits = 0x7ff8_0000_0000_0000 | (payload & 0x0007_ffff_ffff_ffff);
    f64::from_bits(if negativo { bits | (1 << 63) } else { bits })
}

/// Una riga: un indice per ogni pool, piu' payload e segno dei NaN.
type Riga = (
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    u64,
    bool,
);

fn riga() -> impl Strategy<Value = Riga> {
    (
        (0..K1.len(), 0..K2.len(), 0..KF.len(), 0..P.len()),
        (
            0..PI.len(),
            0..PBAD.len(),
            0..V.len(),
            0..VI.len(),
            0..VS.len(),
        ),
        any::<u64>(),
        any::<bool>(),
    )
        .prop_map(|((k1, k2, kf, p), (pi, pbad, v, vi, vs), payload, segno)| {
            (k1, k2, kf, p, pi, pbad, v, vi, vs, payload, segno)
        })
}

fn batch_da(righe: &[Riga]) -> RecordBatch {
    let nan = |valore: Option<f64>, payload: u64, segno: bool| {
        valore.map(|v| {
            if v.is_nan() {
                nan_con_payload(payload, segno)
            } else {
                v
            }
        })
    };
    let colonne: Vec<(&str, ArrayRef)> = vec![
        (
            "k1",
            Arc::new(Int64Array::from(
                righe.iter().map(|r| K1[r.0]).collect::<Vec<_>>(),
            )),
        ),
        (
            "k2",
            Arc::new(StringArray::from(
                righe.iter().map(|r| K2[r.1]).collect::<Vec<_>>(),
            )),
        ),
        (
            "kf",
            Arc::new(Float64Array::from(
                righe
                    .iter()
                    .map(|r| nan(KF[r.2], r.9, r.10))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "p",
            Arc::new(StringArray::from(
                righe.iter().map(|r| P[r.3]).collect::<Vec<_>>(),
            )),
        ),
        (
            "pi",
            Arc::new(Int64Array::from(
                righe.iter().map(|r| PI[r.4]).collect::<Vec<_>>(),
            )),
        ),
        (
            "pbad",
            Arc::new(StringArray::from(
                righe.iter().map(|r| PBAD[r.5]).collect::<Vec<_>>(),
            )),
        ),
        (
            "v",
            Arc::new(Float64Array::from(
                righe
                    .iter()
                    .map(|r| nan(V[r.6], r.9 >> 7, !r.10))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "vi",
            Arc::new(Int64Array::from(
                righe.iter().map(|r| VI[r.7]).collect::<Vec<_>>(),
            )),
        ),
        (
            "vs",
            Arc::new(StringArray::from(
                righe.iter().map(|r| VS[r.8]).collect::<Vec<_>>(),
            )),
        ),
        // Binary UTF-8 valido: la chiave passa dall'encoder binario.
        (
            "kb",
            Arc::new(BinaryArray::from(
                righe
                    .iter()
                    .map(|r| K2[r.1].map(str::as_bytes))
                    .collect::<Vec<_>>(),
            )),
        ),
    ];
    let schema = Schema::new(
        colonne
            .iter()
            .map(|(nome, colonna)| Field::new(*nome, colonna.data_type().clone(), true))
            .collect::<Vec<_>>(),
    );
    RecordBatch::try_new(
        Arc::new(schema),
        colonne.into_iter().map(|(_, colonna)| colonna).collect(),
    )
    .expect("batch pivot")
}

const AGGREGAZIONI: [PivotAgg; 8] = [
    PivotAgg::First,
    PivotAgg::Last,
    PivotAgg::Max,
    PivotAgg::Min,
    PivotAgg::Sum,
    PivotAgg::Mean,
    PivotAgg::Count,
    PivotAgg::Concat,
];

fn mappings() -> Vec<BTreeMap<String, String>> {
    vec![
        BTreeMap::new(),
        [("x", "X"), ("10", "dieci"), ("1", "uno")]
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        [("y", "Y"), ("  ", "spazi"), ("", "vuoto")]
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        // Solo chiavi intere canoniche: sulle pivot_col intere il mapping
        // esegue (le altre mappe li' si rifiutano), con una chiave assente.
        [
            ("1", "uno"),
            ("10", "dieci"),
            ("-3", "meno_tre"),
            ("987654", "assente"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect(),
    ]
}

fn confronta(batch: &RecordBatch, index: &str, pivot_col: &str) {
    for limits in [
        Limits::default(),
        Limits {
            max_columns: 4,
            ..Limits::default()
        },
        Limits {
            max_rows: 3,
            ..Limits::default()
        },
    ] {
        for value in ["v", "vi", "vs"] {
            for aggr_func in AGGREGAZIONI {
                for mapping in mappings() {
                    let config = Pivot {
                        index_col: index.into(),
                        column: pivot_col.into(),
                        value_col: value.into(),
                        aggr_func: aggr_func.clone(),
                        mapping,
                    };
                    assert_esiti_equivalenti(
                        &config,
                        pivot(batch, &config, &limits),
                        pivot_reference(batch, &config, &limits),
                    );
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn pivot_come_il_riferimento_su_input_generati(
        righe in prop::collection::vec(riga(), 0..40),
        index in prop::sample::select(vec!["k1", "k2", "kf", "kb", "k1,k2", "k2, kf ,k1", "", "kb,k1"]),
        pivot_col in prop::sample::select(vec!["p", "pi", "pbad", "kf"]),
    ) {
        confronta(&batch_da(&righe), index, pivot_col);
    }
}

#[test]
fn pivot_come_il_riferimento_oltre_le_soglie() {
    // Molte chiavi (oltre la soglia dell'ordinamento parallelo, 32_768) e
    // celle con molte righe: somme lunghe in ordine di riga.
    let righe = (0..70_000_u64)
        .map(|i| {
            let n = |m: u64| usize::try_from(i.wrapping_mul(2_654_435_761) % m).expect("n");
            (
                n(6),
                n(7),
                n(7),
                n(6),
                n(5),
                0,
                n(12),
                n(6),
                n(6),
                i,
                i % 2 == 0,
            )
        })
        .collect::<Vec<_>>();
    let mut batch = batch_da(&righe);
    // Chiave ad alta cardinalita' al posto di k1.
    let alta = Int64Array::from((0..70_000_i64).map(|i| i % 40_000).collect::<Vec<_>>());
    let mut colonne = batch.columns().to_vec();
    colonne[0] = Arc::new(alta);
    batch = crate::batch_with_rows(batch.schema(), colonne, batch.num_rows()).expect("batch");
    // Suite di default: una combinazione su quattro, a rotazione, cosi' ogni
    // indice, aggregazione e colonna valore resta coperta almeno una volta.
    let lunghi = test_lunghi();
    for (i_index, (index, pivot_col)) in [("k1", "p"), ("k1,k2", "pi"), ("kf", "p")]
        .into_iter()
        .enumerate()
    {
        for (i_aggr, aggr_func) in AGGREGAZIONI.into_iter().enumerate() {
            for (i_value, value) in ["v", "vi"].into_iter().enumerate() {
                if !lunghi && !(i_index + i_aggr + i_value).is_multiple_of(4) {
                    continue;
                }
                let config = Pivot {
                    index_col: index.into(),
                    column: pivot_col.into(),
                    value_col: value.into(),
                    aggr_func: aggr_func.clone(),
                    mapping: BTreeMap::new(),
                };
                assert_esiti_equivalenti(
                    &config,
                    pivot(&batch, &config, &Limits::default()),
                    pivot_reference(&batch, &config, &Limits::default()),
                );
            }
        }
    }
}

#[test]
fn l_errore_di_cella_esce_nell_ordine_delle_colonne() {
    // L'errore di conversione di un valore resta alla sua cella e si rende
    // quando la costruzione dell'output arriva a quella colonna, come nel
    // riferimento che riduce il vettore di righe colonna per colonna. Due
    // celle in errore in colonne diverse: stesso esito del riferimento.
    let righe: Vec<Riga> = vec![
        (1, 1, 0, 2, 0, 0, 0, 0, 3, 0, false), // p=y, vs=abc
        (1, 1, 0, 1, 0, 0, 0, 0, 1, 0, false), // p=x, vs=1.5
        (2, 1, 0, 1, 0, 0, 0, 0, 3, 0, false), // p=x, vs=abc
    ];
    let batch = batch_da(&righe);
    for aggr_func in [PivotAgg::Sum, PivotAgg::Mean, PivotAgg::Min, PivotAgg::Max] {
        let config = Pivot {
            index_col: "k1".into(),
            column: "p".into(),
            value_col: "vs".into(),
            aggr_func,
            mapping: BTreeMap::new(),
        };
        let veloce = pivot(&batch, &config, &Limits::default());
        assert!(veloce.is_err());
        assert_same_outcome_bits(veloce, pivot_reference(&batch, &config, &Limits::default()));
    }
    // Nome invalido ("  ") in una colonna che precede quella in errore.
    let righe: Vec<Riga> = vec![
        (1, 1, 0, 0, 0, 2, 0, 0, 1, 0, false), // pbad="  ", vs=1.5
        (1, 1, 0, 0, 0, 0, 0, 0, 3, 0, false), // pbad="x", vs=abc
    ];
    let batch = batch_da(&righe);
    let config = Pivot {
        index_col: "k1".into(),
        column: "pbad".into(),
        value_col: "vs".into(),
        aggr_func: PivotAgg::Sum,
        mapping: BTreeMap::new(),
    };
    let atteso = pivot_reference(&batch, &config, &Limits::default());
    assert!(matches!(atteso, Err(PlenoraError::InvalidPlan(_))));
    assert_same_outcome_bits(pivot(&batch, &config, &Limits::default()), atteso);
    // Errore del valore alla riga 0, errore del testo pivot (Binary non
    // UTF-8) alla riga 1: la scansione arriva al secondo, che vince, come
    // nel riferimento che converte tutte le chiavi e i pivot prima di
    // aggregare.
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", DataType::Int64, false),
            Field::new("p", DataType::Binary, false),
            Field::new("vs", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])),
            Arc::new(BinaryArray::from(vec![&b"x"[..], &[0xff][..]])),
            Arc::new(StringArray::from(vec!["abc", "1"])),
        ],
    )
    .expect("batch");
    let config = Pivot {
        index_col: "k".into(),
        column: "p".into(),
        value_col: "vs".into(),
        aggr_func: PivotAgg::Sum,
        mapping: BTreeMap::new(),
    };
    let atteso = pivot_reference(&batch, &config, &Limits::default());
    assert_eq!(
        atteso
            .as_ref()
            .map_err(ToString::to_string)
            .err()
            .as_deref(),
        Some("schema violation: binary non contiene UTF-8 valido")
    );
    assert_same_outcome_bits(pivot(&batch, &config, &Limits::default()), atteso);
}

#[test]
fn indice_binary_non_utf8_ordinato_come_aggregate() {
    // Prima un indice Binary non UTF-8 era un errore di conversione; ora e'
    // una chiave di byte, nello stesso ordine canonico di `aggregate`.
    let chiavi: Vec<Option<&[u8]>> = vec![
        Some(&[0xff]),
        Some(b"a"),
        None,
        Some(&[0xff; 10]),
        Some(&[0xff]),
        Some(&[0xc3, 0x28]),
    ];
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("g", DataType::Binary, true),
            Field::new("p", DataType::Utf8, false),
            Field::new("v", DataType::Int64, false),
        ])),
        vec![
            Arc::new(BinaryArray::from(chiavi)),
            Arc::new(StringArray::from(vec!["x"; 6])),
            Arc::new(Int64Array::from((0..6).collect::<Vec<i64>>())),
        ],
    )
    .expect("batch");
    let config = Pivot {
        index_col: "g".into(),
        column: "p".into(),
        value_col: "v".into(),
        aggr_func: PivotAgg::Concat,
        mapping: BTreeMap::new(),
    };
    let output = pivot(&batch, &config, &Limits::default()).expect("pivot");
    let aggregato = crate::aggregation::aggregate(
        &batch,
        &serde_json::from_value(serde_json::json!({
            "group_by": ["g"],
            "aggregations": [{"column": "v", "function": "concat", "separator": ",", "alias": "x"}]
        }))
        .expect("config"),
    )
    .expect("aggregate");
    assert_eq!(output.column(0).as_ref(), aggregato.column(0).as_ref());
    assert_eq!(output.column(1).as_ref(), aggregato.column(1).as_ref());
    assert_eq!(output.num_rows(), 5);
}
