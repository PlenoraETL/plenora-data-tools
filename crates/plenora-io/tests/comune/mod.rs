//! Tabelle di prova condivise dai test di `plenora-io`.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::array::builder::{
    Int32Builder, LargeStringBuilder, ListBuilder, StringDictionaryBuilder,
};
use plenora_core::arrow::array::types::{Decimal256Type, Int32Type};

type I256 = <Decimal256Type as ArrowPrimitiveType>::Native;
use plenora_core::arrow::array::{
    Array, ArrayRef, ArrowPrimitiveType, BinaryArray, BooleanArray, Date32Array, Date64Array,
    Decimal128Array, Decimal256Array, DurationMillisecondArray, FixedSizeBinaryArray, Float32Array,
    Float64Array, Int16Array, Int32Array, Int64Array, Int8Array, LargeBinaryArray, NullArray,
    RecordBatch, StringArray, StructArray, Time32SecondArray, Time64NanosecondArray,
    TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use plenora_core::arrow::schema::{DataType, Field, Fields, Schema};

/// Directory temporanea per un test.
pub fn cartella() -> tempfile::TempDir {
    tempfile::tempdir().expect("directory temporanea")
}

fn con_meta(campo: Field, chiave: &str, valore: &str) -> Field {
    campo.with_metadata(HashMap::from([(chiave.to_owned(), valore.to_owned())]))
}

/// Tabella larga: un tipo per colonna, null, estremi, NaN e -0.0,
/// metadati di campo e di schema.
#[allow(clippy::too_many_lines)] // Un tipo per colonna, in un posto solo.
pub fn tabella_larga() -> RecordBatch {
    let mut colonne: Vec<(Field, ArrayRef)> = Vec::new();
    let mut aggiungi = |campo: Field, colonna: ArrayRef| colonne.push((campo, colonna));

    aggiungi(
        Field::new("i8", DataType::Int8, true),
        Arc::new(Int8Array::from(vec![
            Some(i8::MIN),
            None,
            Some(0),
            Some(i8::MAX),
        ])),
    );
    aggiungi(
        Field::new("i16", DataType::Int16, false),
        Arc::new(Int16Array::from(vec![i16::MIN, -1, 0, i16::MAX])),
    );
    aggiungi(
        con_meta(
            Field::new("i32", DataType::Int32, true),
            "descrizione",
            "chiave",
        ),
        Arc::new(Int32Array::from(vec![
            Some(i32::MIN),
            Some(7),
            None,
            Some(i32::MAX),
        ])),
    );
    aggiungi(
        Field::new("i64", DataType::Int64, true),
        Arc::new(Int64Array::from(vec![
            Some(i64::MIN),
            Some(-1),
            Some(1),
            Some(i64::MAX),
        ])),
    );
    aggiungi(
        Field::new("u8", DataType::UInt8, true),
        Arc::new(UInt8Array::from(vec![
            Some(0),
            Some(u8::MAX),
            None,
            Some(1),
        ])),
    );
    aggiungi(
        Field::new("u16", DataType::UInt16, true),
        Arc::new(UInt16Array::from(vec![0, u16::MAX, 2, 3])),
    );
    aggiungi(
        Field::new("u32", DataType::UInt32, true),
        Arc::new(UInt32Array::from(vec![0, u32::MAX, 2, 3])),
    );
    aggiungi(
        Field::new("u64", DataType::UInt64, true),
        Arc::new(UInt64Array::from(vec![
            Some(0),
            Some(u64::MAX),
            None,
            Some(1 << 63),
        ])),
    );
    aggiungi(
        Field::new("f32", DataType::Float32, true),
        Arc::new(Float32Array::from(vec![
            Some(f32::NAN),
            Some(-0.0),
            Some(f32::INFINITY),
            None,
        ])),
    );
    aggiungi(
        Field::new("f64", DataType::Float64, true),
        Arc::new(Float64Array::from(vec![
            Some(f64::from_bits(0x7ff8_0000_0000_0001)),
            Some(-0.0),
            Some(f64::NEG_INFINITY),
            Some(f64::MIN_POSITIVE / 2.0),
        ])),
    );
    aggiungi(
        Field::new("bool", DataType::Boolean, true),
        Arc::new(BooleanArray::from(vec![
            Some(true),
            None,
            Some(false),
            Some(true),
        ])),
    );
    aggiungi(
        Field::new("testo", DataType::Utf8, true),
        Arc::new(StringArray::from(vec![
            Some("à€𝄞"),
            Some(""),
            None,
            Some("x"),
        ])),
    );
    let mut lungo = LargeStringBuilder::new();
    lungo.append_value("grande");
    lungo.append_null();
    lungo.append_value("");
    lungo.append_value("z".repeat(1000));
    aggiungi(
        Field::new("testo_large", DataType::LargeUtf8, true),
        Arc::new(lungo.finish()),
    );
    aggiungi(
        Field::new("bin", DataType::Binary, true),
        Arc::new(BinaryArray::from(vec![
            Some(&b"\x00\xff"[..]),
            None,
            Some(&b""[..]),
            Some(&b"abc"[..]),
        ])),
    );
    aggiungi(
        Field::new("bin_large", DataType::LargeBinary, true),
        Arc::new(LargeBinaryArray::from(vec![
            Some(&b"\x01"[..]),
            Some(&b""[..]),
            None,
            Some(&b"\x00\x00"[..]),
        ])),
    );
    aggiungi(
        Field::new("bin_fisso", DataType::FixedSizeBinary(3), true),
        Arc::new(
            FixedSizeBinaryArray::try_from_sparse_iter_with_size(
                vec![Some(b"abc"), None, Some(b"\x00\x00\x00"), Some(b"xyz")].into_iter(),
                3,
            )
            .expect("fixed size"),
        ),
    );
    let mut dizionario = StringDictionaryBuilder::<Int32Type>::new();
    dizionario.append_value("rosso");
    dizionario.append_null();
    dizionario.append_value("verde");
    dizionario.append_value("rosso");
    aggiungi(
        Field::new(
            "dizionario",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ),
        Arc::new(dizionario.finish()),
    );
    aggiungi(
        Field::new("data32", DataType::Date32, true),
        Arc::new(Date32Array::from(vec![
            Some(-719_162),
            Some(0),
            None,
            Some(2_932_896),
        ])),
    );
    aggiungi(
        Field::new("data64", DataType::Date64, true),
        Arc::new(Date64Array::from(vec![
            Some(0),
            Some(86_400_000),
            None,
            Some(-86_400_000),
        ])),
    );
    aggiungi(
        Field::new(
            "ts_s_roma",
            DataType::Timestamp(
                plenora_core::arrow::schema::TimeUnit::Second,
                Some("Europe/Rome".into()),
            ),
            true,
        ),
        Arc::new(
            TimestampSecondArray::from(vec![Some(0), Some(1_700_000_000), None, Some(-1)])
                .with_timezone("Europe/Rome"),
        ),
    );
    aggiungi(
        Field::new(
            "ts_ms",
            DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, None),
            true,
        ),
        Arc::new(TimestampMillisecondArray::from(vec![
            Some(i64::MIN / 2),
            Some(1),
            None,
            Some(i64::MAX / 2),
        ])),
    );
    aggiungi(
        Field::new(
            "ts_us_utc",
            DataType::Timestamp(
                plenora_core::arrow::schema::TimeUnit::Microsecond,
                Some("UTC".into()),
            ),
            true,
        ),
        Arc::new(TimestampMicrosecondArray::from(vec![1, 2, 3, 4]).with_timezone("UTC")),
    );
    aggiungi(
        Field::new(
            "ts_ns_offset",
            DataType::Timestamp(
                plenora_core::arrow::schema::TimeUnit::Nanosecond,
                Some("+01:00".into()),
            ),
            true,
        ),
        Arc::new(
            TimestampNanosecondArray::from(vec![Some(i64::MIN), None, Some(0), Some(i64::MAX)])
                .with_timezone("+01:00"),
        ),
    );
    aggiungi(
        Field::new(
            "ora32",
            DataType::Time32(plenora_core::arrow::schema::TimeUnit::Second),
            true,
        ),
        Arc::new(Time32SecondArray::from(vec![0, 1, 86_399, 3600])),
    );
    aggiungi(
        Field::new(
            "ora64",
            DataType::Time64(plenora_core::arrow::schema::TimeUnit::Nanosecond),
            true,
        ),
        Arc::new(Time64NanosecondArray::from(vec![
            0,
            1,
            86_399_999_999_999,
            5,
        ])),
    );
    aggiungi(
        Field::new(
            "durata",
            DataType::Duration(plenora_core::arrow::schema::TimeUnit::Millisecond),
            true,
        ),
        Arc::new(DurationMillisecondArray::from(vec![
            Some(-5),
            None,
            Some(0),
            Some(i64::MAX),
        ])),
    );
    aggiungi(
        Field::new("dec_5_2", DataType::Decimal128(5, 2), true),
        Arc::new(
            Decimal128Array::from(vec![Some(-99_999), Some(0), None, Some(12_345)])
                .with_precision_and_scale(5, 2)
                .expect("decimale"),
        ),
    );
    aggiungi(
        Field::new("dec_38_10", DataType::Decimal128(38, 10), true),
        Arc::new(
            Decimal128Array::from(vec![
                Some(-(10_i128.pow(38) - 1)),
                Some(1),
                None,
                Some(10_i128.pow(38) - 1),
            ])
            .with_precision_and_scale(38, 10)
            .expect("decimale"),
        ),
    );
    aggiungi(
        Field::new("dec256", DataType::Decimal256(50, 5), true),
        Arc::new(
            Decimal256Array::from(vec![
                Some(I256::from_i128(-7)),
                None,
                Some(I256::from_i128(i128::MAX)),
                Some(I256::from_i128(0)),
            ])
            .with_precision_and_scale(50, 5)
            .expect("decimale"),
        ),
    );
    let mut lista = ListBuilder::new(Int32Builder::new());
    lista.append_value([Some(1), None, Some(3)]);
    lista.append_null();
    lista.append_value([] as [Option<i32>; 0]);
    lista.append_value([Some(i32::MAX)]);
    aggiungi(
        Field::new(
            "lista",
            DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
            true,
        ),
        Arc::new(lista.finish()),
    );
    let campi_struct = Fields::from(vec![
        Field::new("a", DataType::Int32, true),
        con_meta(Field::new("b", DataType::Utf8, true), "annidato", "si"),
    ]);
    aggiungi(
        Field::new("struttura", DataType::Struct(campi_struct.clone()), true),
        Arc::new(StructArray::new(
            campi_struct,
            vec![
                Arc::new(Int32Array::from(vec![Some(1), None, Some(3), Some(4)])) as ArrayRef,
                Arc::new(StringArray::from(vec![
                    Some("p"),
                    Some("q"),
                    None,
                    Some("s"),
                ])),
            ],
            Some(vec![true, true, false, true].into()),
        )),
    );
    aggiungi(
        Field::new("nulla", DataType::Null, true),
        Arc::new(NullArray::new(4)),
    );

    let (campi, array): (Vec<Field>, Vec<ArrayRef>) = colonne.into_iter().unzip();
    let schema = Schema::new_with_metadata(
        campi,
        HashMap::from([
            ("origine".to_owned(), "prova".to_owned()),
            ("vuoto".to_owned(), String::new()),
        ]),
    );
    RecordBatch::try_new(Arc::new(schema), array).expect("tabella larga")
}

/// Confronto bit per bit: schema (metadati compresi) e colonne. Per i float
/// `RecordBatch::eq` confronta i buffer, quindi NaN e -0.0 sono esatti.
pub fn identiche(attesa: &RecordBatch, letta: &RecordBatch) {
    assert_eq!(attesa.schema(), letta.schema(), "schema diverso");
    assert_eq!(attesa.num_rows(), letta.num_rows(), "righe diverse");
    for (indice, (a, b)) in attesa.columns().iter().zip(letta.columns()).enumerate() {
        assert_eq!(
            a.to_data(),
            b.to_data(),
            "colonna {} diversa",
            attesa.schema().field(indice).name()
        );
    }
    for nome in ["f32", "f64"] {
        if let (Ok(i), Ok(j)) = (
            attesa.schema().index_of(nome),
            letta.schema().index_of(nome),
        ) {
            let bit = |colonna: &ArrayRef| -> Vec<Option<u64>> {
                colonna.as_any().downcast_ref::<Float64Array>().map_or_else(
                    || {
                        let c = colonna.as_any().downcast_ref::<Float32Array>().unwrap();
                        c.iter()
                            .map(|v| v.map(|x| u64::from(x.to_bits())))
                            .collect()
                    },
                    |c| c.iter().map(|v| v.map(f64::to_bits)).collect(),
                )
            };
            assert_eq!(bit(attesa.column(i)), bit(letta.column(j)), "bit di {nome}");
        }
    }
}

/// Tabella ordini per l'end-to-end.
pub fn ordini() -> RecordBatch {
    let schema = Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("cliente", DataType::Utf8, false),
        Field::new("importo", DataType::Float64, true),
    ]);
    RecordBatch::try_new(
        Arc::new(schema),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5, 6])),
            Arc::new(StringArray::from(vec!["a", "b", "a", "c", "b", "a"])),
            Arc::new(Float64Array::from(vec![
                Some(10.0),
                Some(-3.0),
                Some(2.5),
                None,
                Some(7.0),
                Some(0.5),
            ])),
        ],
    )
    .expect("ordini")
}
