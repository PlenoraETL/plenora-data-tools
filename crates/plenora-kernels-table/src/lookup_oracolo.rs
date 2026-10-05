//! Oracolo di `table.lookup`: la funzione com'era prima della mappa in
//! prestito e dei chunk paralleli, copiata alla lettera, e i confronti degli
//! esiti completi (batch per byte, buffer dell'uscita, errori per categoria,
//! messaggio e diagnostica).

use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, Array, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, Int32Array, LargeStringArray, TimestampMillisecondArray, UInt64Array,
};
use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::test_support::{assert_same_outcome_bits, nullable_batch};

// Copia del percorso precedente: una `String` per riga e il testo della
// voce convertito a ogni riga. Unica modifica, con la 2.0.0: una voce `null`
// da' la cella nulla invece del testo vuoto.
fn value_text_riferimento(value: &Value) -> Option<String> {
    match value {
        Value::String(v) => Some(v.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn lookup_riferimento(batch: &RecordBatch, config: &Lookup) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let output = config.output_column.as_deref().unwrap_or(&config.column);
    validate_output_name(output)?;
    let values = (0..batch.num_rows())
        .map(|row| {
            scalar_as_string(source.as_ref(), row).map(|value| {
                value.and_then(|value| {
                    config.mapping.get(&value).map_or_else(
                        || {
                            if config.default.is_null() {
                                Some(value)
                            } else {
                                value_text_riferimento(&config.default)
                            }
                        },
                        value_text_riferimento,
                    )
                })
            })
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        output,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

/// Confronto completo, e in piu' i buffer della colonna d'uscita: offset,
/// byte dei valori e bitmap dei null (presente o assente) identici.
fn confronta(batch: &RecordBatch, config: &Lookup) {
    let veloce = lookup(batch, config);
    let riferimento = lookup_riferimento(batch, config);
    if let (Ok(veloce), Ok(riferimento)) = (&veloce, &riferimento) {
        let nome = config.output_column.as_deref().unwrap_or(&config.column);
        let colonna = |uscita: &RecordBatch| {
            let indice = uscita.schema().index_of(nome).expect("colonna d'uscita");
            uscita
                .column(indice)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8")
                .clone()
        };
        let (a, b) = (colonna(veloce), colonna(riferimento));
        assert_eq!(a.value_offsets(), b.value_offsets(), "offset");
        assert_eq!(a.value_data(), b.value_data(), "byte dei valori");
        assert_eq!(a.nulls(), b.nulls(), "bitmap dei null");
    }
    assert_same_outcome_bits(veloce, riferimento);
}

/// Mappa avversaria: chiavi vuote, con spazi, Unicode, simili ai testi dei
/// numeri; valori di ogni tipo JSON (il null diventa la cella nulla, il `""` il
/// testo vuoto).
fn mappa() -> BTreeMap<String, Value> {
    [
        ("", json!("vuota")),
        (" a", json!("spazio")),
        ("A", json!(null)),
        ("a", json!(1)),
        ("ΣΑΣ", json!(-0.5)),
        ("\u{0}", json!(true)),
        ("1", json!([1, "x"])),
        ("-1", json!({"k": "v"})),
        ("NaN", json!("nan")),
        ("inf", json!("∞")),
        ("true", json!("vero")),
        ("1970-01-01", json!("epoca")),
        ("Uno ", json!("uno")),
        ("0.000", json!("zero")),
        ("ok", json!("")),
    ]
    .into_iter()
    .map(|(chiave, valore)| (chiave.to_owned(), valore))
    .collect()
}

const DEFAULT: [fn() -> Value; 5] = [
    || json!(null),
    || json!("altro"),
    || json!(""),
    || json!(42),
    || json!({"a": [1, 2]}),
];

fn config(colonna: &str, uscita: Option<&str>, default: Value) -> Lookup {
    Lookup {
        column: colonna.into(),
        mapping: mappa(),
        default,
        output_column: uscita.map(ToOwned::to_owned),
    }
}

/// Tutti i default, uscita in sostituzione e in una colonna nuova.
fn confronta_tutto(batch: &RecordBatch, colonna: &str) {
    for default in DEFAULT {
        for uscita in [None, Some("tradotta")] {
            confronta(batch, &config(colonna, uscita, default()));
        }
    }
}

const TESTI: [&str; 12] = [
    "", " a", "A", "a", "ΣΑΣ", "\u{0}", "1", "assente", "Σ", "ok", "Uno ", "true",
];

fn batch_tutti_i_tipi(righe: usize) -> RecordBatch {
    let nulla = |riga: usize, periodo: usize| riga % periodo == periodo - 1;
    let testi = (0..righe)
        .map(|riga| (!nulla(riga, 7)).then(|| TESTI[riga % TESTI.len()]))
        .collect::<Vec<_>>();
    let chiavi = (0..righe)
        .map(|riga| (!nulla(riga, 5)).then(|| i32::try_from(riga % 3).unwrap_or_default()))
        .collect::<Vec<_>>();
    let dizionario = DictionaryArray::<Int32Type>::try_new(
        Int32Array::from(chiavi),
        Arc::new(StringArray::from(vec![Some("Uno "), None, Some("ΣΑΣ")])),
    )
    .expect("dizionario");
    nullable_batch(vec![
        ("s", Arc::new(StringArray::from(testi)) as ArrayRef),
        (
            "i",
            Arc::new(Int64Array::from(
                (0..righe)
                    .map(|riga| (!nulla(riga, 4)).then(|| [1_i64, -1, 7, i64::MIN][riga % 4]))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "f",
            Arc::new(Float64Array::from(
                (0..righe)
                    .map(|riga| {
                        (!nulla(riga, 6)).then(|| [f64::NAN, f64::INFINITY, 1.0, -0.0][riga % 4])
                    })
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "b",
            Arc::new(BooleanArray::from(
                (0..righe)
                    .map(|riga| (!nulla(riga, 3)).then_some(riga % 2 == 0))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "u",
            Arc::new(UInt64Array::from(
                (0..righe)
                    .map(|riga| (!nulla(riga, 8)).then(|| [1_u64, u64::MAX][riga % 2]))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "d",
            Arc::new(Date32Array::from(
                (0..righe)
                    .map(|riga| (!nulla(riga, 9)).then(|| [0_i32, 19_000][riga % 2]))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "t",
            Arc::new(
                TimestampMillisecondArray::from(
                    (0..righe)
                        .map(|riga| (!nulla(riga, 10)).then(|| [0_i64, -1][riga % 2]))
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
                        .map(|riga| (!nulla(riga, 11)).then(|| [0_i128, -1][riga % 2]))
                        .collect::<Vec<_>>(),
                )
                .with_precision_and_scale(10, 3)
                .expect("decimal"),
            ),
        ),
        (
            "y",
            Arc::new(BinaryArray::from(
                (0..righe)
                    .map(|riga| (!nulla(riga, 3)).then(|| TESTI[riga % TESTI.len()].as_bytes()))
                    .collect::<Vec<_>>(),
            )),
        ),
        ("k", Arc::new(dizionario)),
    ])
}

#[test]
fn tutti_i_tipi_su_piu_chunk() {
    // Piu' chunk (percorso rayon), un chunk solo, una riga, nessuna riga.
    for righe in [RIGHE_PER_CHUNK_LOOKUP * 7 + 3, RIGHE_PER_CHUNK_LOOKUP, 1, 0] {
        let batch = batch_tutti_i_tipi(righe);
        for colonna in ["s", "i", "f", "b", "u", "d", "t", "m", "y", "k", "assente"] {
            confronta_tutto(&batch, colonna);
        }
        // Una fetta con offset diverso da zero: gli indici di riga restano
        // quelli della fetta.
        if righe > 5 {
            let fetta = batch.slice(3, righe - 5);
            for colonna in ["s", "k", "i"] {
                confronta_tutto(&fetta, colonna);
            }
        }
    }
}

#[test]
fn errori_come_il_riferimento() {
    let righe = RIGHE_PER_CHUNK_LOOKUP * 3;
    let binari = (0..righe)
        .map(|riga| {
            Some(if riga == righe - 2 {
                &[0xff_u8][..]
            } else {
                b"ok".as_slice()
            })
        })
        .collect::<Vec<_>>();
    let date = (0..righe)
        .map(|riga| Some(if riga == 20 { i32::MAX } else { 0 }))
        .collect::<Vec<_>>();
    let batch = nullable_batch(vec![
        ("y", Arc::new(BinaryArray::from(binari)) as ArrayRef),
        ("d", Arc::new(Date32Array::from(date))),
        ("z", Arc::new(Int32Array::from(vec![Some(1); righe]))),
        (
            "l",
            Arc::new(LargeStringArray::from(vec![Some("a"); righe])),
        ),
        ("s", Arc::new(StringArray::from(vec![Some("a"); righe]))),
    ]);
    for colonna in ["y", "d", "z", "l", "assente"] {
        confronta_tutto(&batch, colonna);
    }
    // Nome d'uscita non valido.
    for uscita in ["", "a\u{0}b"] {
        confronta(&batch, &config("s", Some(uscita), json!(null)));
    }
}

/// La mappa del piano con chiavi ripetute: resta quella che `serde_json`
/// deserializza, per entrambi i percorsi.
#[test]
fn chiavi_ripetute_nel_piano() {
    let config: Lookup = serde_json::from_value(json!({
        "column": "s",
        "mapping": {"a": "primo", "b": null},
        "default": "d",
    }))
    .expect("config");
    let testo = r#"{"column": "s", "mapping": {"a": "primo", "a": "secondo", "b": 3}}"#;
    let ripetuta: Lookup = serde_json::from_str(testo).expect("config ripetuta");
    let batch = nullable_batch(vec![(
        "s",
        Arc::new(StringArray::from(vec![
            Some("a"),
            None,
            Some("b"),
            Some("c"),
        ])) as ArrayRef,
    )]);
    confronta(&batch, &config);
    confronta(&batch, &ripetuta);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn come_il_riferimento_su_input_casuali(
        valori in prop::collection::vec(prop::option::weighted(0.85, "[aAΣ ]{0,2}"), 0..120),
        voci in prop::collection::btree_map("[aAΣ ]{0,2}", prop_oneof![
            Just(json!(null)),
            "\\PC{0,3}".prop_map(Value::from),
            any::<i32>().prop_map(Value::from),
        ], 0..8),
        default in prop_oneof![Just(json!(null)), "\\PC{0,3}".prop_map(Value::from)],
        interi in any::<bool>(),
    ) {
        let colonna: ArrayRef = if interi {
            Arc::new(Int64Array::from(
                valori.iter().map(|valore| valore.as_ref().map(|testo| i64::try_from(testo.len()).unwrap_or_default())).collect::<Vec<_>>(),
            ))
        } else {
            Arc::new(StringArray::from(valori))
        };
        let batch = nullable_batch(vec![("s", colonna)]);
        let config = Lookup {
            column: "s".into(),
            mapping: voci.into_iter().chain([("1".to_owned(), json!("uno"))]).collect(),
            default,
            output_column: Some("o".into()),
        };
        confronta(&batch, &config);
    }
}
