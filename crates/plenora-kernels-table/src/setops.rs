use std::collections::HashSet;
use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, Array, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
    UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Schema};
use serde::Deserialize;

use crate::joins::FastHasher;
use crate::select_rows;
use crate::Limits;
use plenora_core::{PlenoraError, Result};

/// Insieme delle chiavi di riga gia' viste/emesse: hash FxHash-style con
/// finalizer splitmix64 (`FastHasher` di `joins.rs`, stesso profilo di costo
/// dei join/aggregate: throughput su milioni di chiavi, non resistenza
/// avversaria). `SipHash` (default std) dominerebbe build e probe.
type KeySet = HashSet<Box<[u8]>, FastHasher>;

fn key_set(capacity: usize) -> KeySet {
    HashSet::with_capacity_and_hasher(capacity, FastHasher::default())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetOperation {}

enum KeyColumn<'a> {
    Utf8(&'a StringArray),
    Int64(&'a Int64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    UInt64(&'a UInt64Array),
    Date32(&'a Date32Array),
    TimestampMillis(&'a TimestampMillisecondArray),
    Decimal128(&'a Decimal128Array),
    Binary(&'a BinaryArray),
    DictionaryUtf8(&'a DictionaryArray<Int32Type>, &'a StringArray),
}

impl KeyColumn<'_> {
    fn encode(&self, row: usize, output: &mut Vec<u8>) -> Result<()> {
        let is_null = match self {
            Self::Utf8(values) => values.is_null(row),
            Self::Int64(values) => values.is_null(row),
            Self::Float64(values) => values.is_null(row),
            Self::Boolean(values) => values.is_null(row),
            Self::UInt64(values) => values.is_null(row),
            Self::Date32(values) => values.is_null(row),
            Self::TimestampMillis(values) => values.is_null(row),
            Self::Decimal128(values) => values.is_null(row),
            Self::Binary(values) => values.is_null(row),
            Self::DictionaryUtf8(values, _) => values.is_null(row),
        };
        if is_null {
            output.push(0);
            return Ok(());
        }
        output.push(1);
        match self {
            Self::Utf8(values) => {
                let value = values.value(row).as_bytes();
                let length = value.len() as u64;
                output.extend_from_slice(&length.to_be_bytes());
                output.extend_from_slice(value);
            }
            Self::Int64(values) => output.extend_from_slice(&values.value(row).to_be_bytes()),
            Self::Float64(values) => {
                let value = values.value(row);
                // The previous string representation treated every NaN as the
                // same value but kept +0 and -0 distinct. Preserve that exact
                // contract while avoiding a temporary String allocation.
                let bits = if value.is_nan() {
                    f64::NAN.to_bits()
                } else {
                    value.to_bits()
                };
                output.extend_from_slice(&bits.to_be_bytes());
            }
            Self::Boolean(values) => output.push(u8::from(values.value(row))),
            Self::UInt64(values) => output.extend_from_slice(&values.value(row).to_be_bytes()),
            Self::Date32(values) => output.extend_from_slice(&values.value(row).to_be_bytes()),
            Self::TimestampMillis(values) => {
                output.extend_from_slice(&values.value(row).to_be_bytes());
            }
            Self::Decimal128(values) => {
                output.extend_from_slice(&values.value(row).to_be_bytes());
            }
            Self::Binary(values) => encode_variable(values.value(row), output),
            Self::DictionaryUtf8(values, dictionary) => {
                let key = usize::try_from(values.keys().value(row))
                    .map_err(|_| PlenoraError::Schema("chiave dictionary negativa".into()))?;
                encode_variable(dictionary.value(key).as_bytes(), output);
            }
        }
        Ok(())
    }
}

fn encode_variable(value: &[u8], output: &mut Vec<u8>) {
    let length = value.len() as u64;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
}

pub struct CompactRowEncoder<'a> {
    columns: Vec<KeyColumn<'a>>,
}

impl<'a> CompactRowEncoder<'a> {
    /// Costruisce l'encoder sulle colonne del batch, nell'ordine dello
    /// schema.
    ///
    /// # Errors
    ///
    /// - `Schema`: array incoerente col tipo dichiarato nello schema,
    ///   dictionary non Utf8 o tipo non supportato dalle set operation.
    pub fn try_new(batch: &'a RecordBatch) -> Result<Self> {
        let columns = batch
            .columns()
            .iter()
            .map(|column| match column.data_type() {
                DataType::Utf8 => column
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .map(KeyColumn::Utf8)
                    .ok_or_else(|| PlenoraError::Schema("array Utf8 incoerente".into())),
                DataType::Int64 => column
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .map(KeyColumn::Int64)
                    .ok_or_else(|| PlenoraError::Schema("array Int64 incoerente".into())),
                DataType::Float64 => column
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .map(KeyColumn::Float64)
                    .ok_or_else(|| PlenoraError::Schema("array Float64 incoerente".into())),
                DataType::Boolean => column
                    .as_any()
                    .downcast_ref::<BooleanArray>()
                    .map(KeyColumn::Boolean)
                    .ok_or_else(|| PlenoraError::Schema("array Boolean incoerente".into())),
                DataType::UInt64 => column
                    .as_any()
                    .downcast_ref::<UInt64Array>()
                    .map(KeyColumn::UInt64)
                    .ok_or_else(|| PlenoraError::Schema("array UInt64 incoerente".into())),
                DataType::Date32 => column
                    .as_any()
                    .downcast_ref::<Date32Array>()
                    .map(KeyColumn::Date32)
                    .ok_or_else(|| PlenoraError::Schema("array Date32 incoerente".into())),
                DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, _) => {
                    column
                        .as_any()
                        .downcast_ref::<TimestampMillisecondArray>()
                        .map(KeyColumn::TimestampMillis)
                        .ok_or_else(|| {
                            PlenoraError::Schema("array TimestampMillis incoerente".into())
                        })
                }
                DataType::Decimal128(_, _) => column
                    .as_any()
                    .downcast_ref::<Decimal128Array>()
                    .map(KeyColumn::Decimal128)
                    .ok_or_else(|| PlenoraError::Schema("array Decimal128 incoerente".into())),
                DataType::Binary => column
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .map(KeyColumn::Binary)
                    .ok_or_else(|| PlenoraError::Schema("array Binary incoerente".into())),
                DataType::Dictionary(key, value)
                    if key.as_ref() == &DataType::Int32 && value.as_ref() == &DataType::Utf8 =>
                {
                    let values = column
                        .as_any()
                        .downcast_ref::<DictionaryArray<Int32Type>>()
                        .ok_or_else(|| {
                            PlenoraError::Schema("array Dictionary incoerente".into())
                        })?;
                    let dictionary = values
                        .values()
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| PlenoraError::Schema("dictionary non Utf8".into()))?;
                    Ok(KeyColumn::DictionaryUtf8(values, dictionary))
                }
                other => Err(PlenoraError::Schema(format!(
                    "tipo {other:?} non supportato dalle set operation"
                ))),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { columns })
    }

    /// Scrive in `output` (riusato fra le righe) la chiave compatta della
    /// riga `row`.
    ///
    /// # Errors
    ///
    /// - `Schema`: chiave dictionary negativa (array dictionary incoerente).
    pub fn encode_into(&self, row: usize, output: &mut Vec<u8>) -> Result<()> {
        output.clear();
        for column in &self.columns {
            column.encode(row, output)?;
        }
        Ok(())
    }
}

/// Verifica che i due batch abbiano nomi e tipi Arrow identici, colonna per
/// colonna.
///
/// # Errors
///
/// - `Schema`: numero di colonne, nomi o tipi diversi fra i due batch.
pub fn validate_schema(left: &RecordBatch, right: &RecordBatch) -> Result<()> {
    if left.num_columns() != right.num_columns()
        || left
            .schema()
            .fields()
            .iter()
            .zip(right.schema().fields())
            .any(|(left, right)| {
                left.name() != right.name() || left.data_type() != right.data_type()
            })
    {
        return Err(PlenoraError::Schema(
            "set operation richiede nomi e tipi Arrow identici".into(),
        ));
    }
    Ok(())
}

/// Concatena i due batch (righe di `left` poi di `right`); la nullability
/// di ogni colonna e' l'OR dei due input, i metadati quelli di `left`.
///
/// # Errors
///
/// - `Schema`: schemi incompatibili (come `validate_schema`) o errore Arrow
///   nella concat o nella costruzione del batch;
/// - `InvalidPlan`: overflow nel conteggio delle righe o totale oltre
///   `limits.max_rows`.
pub fn concat_compatible(
    left: &RecordBatch,
    right: &RecordBatch,
    limits: &Limits,
) -> Result<RecordBatch> {
    validate_schema(left, right)?;
    let rows = left
        .num_rows()
        .checked_add(right.num_rows())
        .ok_or_else(|| PlenoraError::InvalidPlan("overflow union_distinct".into()))?;
    if rows > limits.max_rows {
        return Err(PlenoraError::InvalidPlan(
            "union_distinct supera max_rows".into(),
        ));
    }
    let columns = left
        .columns()
        .iter()
        .zip(right.columns())
        .map(|(left, right)| {
            plenora_core::arrow::select::concat::concat(&[left.as_ref(), right.as_ref()])
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let fields = left
        .schema()
        .fields()
        .iter()
        .zip(right.schema().fields())
        .map(|(left, right)| {
            left.as_ref()
                .clone()
                .with_nullable(left.is_nullable() || right.is_nullable())
        })
        .collect::<Vec<_>>();
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            left.schema().metadata().clone(),
        )),
        columns,
    )?)
}

fn unique_rows(batch: &RecordBatch, predicate: impl Fn(&[u8]) -> bool) -> Result<Vec<usize>> {
    let encoder = CompactRowEncoder::try_new(batch)?;
    let mut emitted = key_set(batch.num_rows());
    let mut rows = Vec::new();
    let mut key = Vec::new();
    for row in 0..batch.num_rows() {
        encoder.encode_into(row, &mut key)?;
        if predicate(&key) && !emitted.contains(key.as_slice()) {
            emitted.insert(std::mem::take(&mut key).into_boxed_slice());
            rows.push(row);
        }
    }
    Ok(rows)
}

/// UNION DISTINCT dei due batch: righe di `left` seguite dalle righe di
/// `right` non gia' presenti, senza duplicati.
///
/// # Errors
///
/// - `Schema`: schemi incompatibili (come `validate_schema`), tipo non
///   supportato dall'encoder di chiavi (come `CompactRowEncoder::try_new`)
///   o errore nella selezione o nella concat finale (come `select_rows` e
///   `concat_compatible`);
/// - `InvalidPlan`: overflow nel conteggio delle righe o totale oltre
///   `limits.max_rows`.
pub fn union_distinct(
    left: &RecordBatch,
    right: &RecordBatch,
    _config: &SetOperation,
    limits: &Limits,
) -> Result<RecordBatch> {
    // Fast path in-memory: stessi controlli e stessi errori del percorso
    // originale (concat completa + dedup), ma la concatena fisica dei due
    // input e' rimandata alla selezione finale: le chiavi di riga si
    // accumulano sullo stesso set scandendo prima `left` poi `right`, cosi'
    // la prima occorrenza (ordine di output) e' esattamente quella del
    // batch combinato e si evita una copia completa degli input.
    validate_schema(left, right)?;
    let rows = left
        .num_rows()
        .checked_add(right.num_rows())
        .ok_or_else(|| PlenoraError::InvalidPlan("overflow union_distinct".into()))?;
    if rows > limits.max_rows {
        return Err(PlenoraError::InvalidPlan(
            "union_distinct supera max_rows".into(),
        ));
    }
    let left_encoder = CompactRowEncoder::try_new(left)?;
    let right_encoder = CompactRowEncoder::try_new(right)?;
    let mut emitted = key_set(rows);
    let mut key = Vec::new();
    let mut left_rows = Vec::new();
    for row in 0..left.num_rows() {
        left_encoder.encode_into(row, &mut key)?;
        if !emitted.contains(key.as_slice()) {
            emitted.insert(std::mem::take(&mut key).into_boxed_slice());
            left_rows.push(row);
        }
    }
    let mut right_rows = Vec::new();
    for row in 0..right.num_rows() {
        right_encoder.encode_into(row, &mut key)?;
        if !emitted.contains(key.as_slice()) {
            emitted.insert(std::mem::take(&mut key).into_boxed_slice());
            right_rows.push(row);
        }
    }
    let selected_left = select_rows(left, &left_rows)?;
    let selected_right = select_rows(right, &right_rows)?;
    concat_compatible(&selected_left, &selected_right, limits)
}

fn right_keys(right: &RecordBatch) -> Result<KeySet> {
    let encoder = CompactRowEncoder::try_new(right)?;
    let mut keys = key_set(right.num_rows());
    let mut key = Vec::new();
    for row in 0..right.num_rows() {
        encoder.encode_into(row, &mut key)?;
        if !keys.contains(key.as_slice()) {
            keys.insert(std::mem::take(&mut key).into_boxed_slice());
        }
    }
    Ok(keys)
}

/// INTERSECT DISTINCT dei due batch: righe di `left` la cui chiave compare
/// in `right`, senza duplicati, nell'ordine di `left`.
///
/// # Errors
///
/// - `Schema`: schemi incompatibili (come `validate_schema`), tipo non
///   supportato dall'encoder di chiavi (come `CompactRowEncoder::try_new`)
///   o errore nella selezione finale (come `select_rows`).
pub fn intersect(
    left: &RecordBatch,
    right: &RecordBatch,
    _config: &SetOperation,
) -> Result<RecordBatch> {
    validate_schema(left, right)?;
    let mut right = right_keys(right)?;
    let encoder = CompactRowEncoder::try_new(left)?;
    let mut rows = Vec::with_capacity(left.num_rows().min(right.len()));
    let mut key = Vec::new();
    for row in 0..left.num_rows() {
        encoder.encode_into(row, &mut key)?;
        // Removing the exact byte key both proves membership and guarantees
        // DISTINCT semantics without retaining a second HashSet for the left.
        if right.remove(key.as_slice()) {
            rows.push(row);
        }
    }
    select_rows(left, &rows)
}

/// EXCEPT DISTINCT dei due batch: righe di `left` la cui chiave non compare
/// in `right`, senza duplicati, nell'ordine di `left`.
///
/// # Errors
///
/// - `Schema`: schemi incompatibili (come `validate_schema`), tipo non
///   supportato dall'encoder di chiavi (come `CompactRowEncoder::try_new`)
///   o errore nella selezione finale (come `select_rows`).
pub fn except(
    left: &RecordBatch,
    right: &RecordBatch,
    _config: &SetOperation,
) -> Result<RecordBatch> {
    validate_schema(left, right)?;
    let right = right_keys(right)?;
    let rows = unique_rows(left, |key| !right.contains(key))?;
    select_rows(left, &rows)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::builder::StringDictionaryBuilder;
    use plenora_core::arrow::array::types::Int32Type;
    use plenora_core::arrow::schema::{DataType, Field};

    use super::*;

    // -----------------------------------------------------------------------
    // Oracolo: copia verbatim dell'implementazione pre-ottimizzazione
    // (ondata stabilizzazione setops). I test confrontano il fast path con
    // l'oracolo in modo rigoroso: schema (nomi, tipi, nullability, metadata),
    // valori, null, bit f64 e ordine delle righe (via uguaglianza degli
    // `ArrayData`, che confronta i buffer bit a bit).
    // -----------------------------------------------------------------------

    fn oracle_unique_rows(
        batch: &RecordBatch,
        predicate: impl Fn(&[u8]) -> bool,
    ) -> Result<Vec<usize>> {
        let encoder = CompactRowEncoder::try_new(batch)?;
        let mut emitted: HashSet<Box<[u8]>> = HashSet::new();
        let mut rows = Vec::new();
        let mut key = Vec::new();
        for row in 0..batch.num_rows() {
            encoder.encode_into(row, &mut key)?;
            if predicate(&key) && !emitted.contains(key.as_slice()) {
                emitted.insert(key.into_boxed_slice());
                rows.push(row);
                key = Vec::new();
            }
        }
        Ok(rows)
    }

    fn oracle_union_distinct(
        left: &RecordBatch,
        right: &RecordBatch,
        _config: &SetOperation,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        let combined = concat_compatible(left, right, limits)?;
        let rows = oracle_unique_rows(&combined, |_| true)?;
        select_rows(&combined, &rows)
    }

    fn oracle_right_keys(right: &RecordBatch) -> Result<HashSet<Box<[u8]>>> {
        let encoder = CompactRowEncoder::try_new(right)?;
        let mut keys = HashSet::with_capacity(right.num_rows());
        let mut key = Vec::new();
        for row in 0..right.num_rows() {
            encoder.encode_into(row, &mut key)?;
            if !keys.contains(key.as_slice()) {
                keys.insert(key.into_boxed_slice());
                key = Vec::new();
            }
        }
        Ok(keys)
    }

    fn oracle_intersect(
        left: &RecordBatch,
        right: &RecordBatch,
        _config: &SetOperation,
    ) -> Result<RecordBatch> {
        validate_schema(left, right)?;
        let mut right = oracle_right_keys(right)?;
        let encoder = CompactRowEncoder::try_new(left)?;
        let mut rows = Vec::with_capacity(left.num_rows().min(right.len()));
        let mut key = Vec::new();
        for row in 0..left.num_rows() {
            encoder.encode_into(row, &mut key)?;
            // Removing the exact byte key both proves membership and guarantees
            // DISTINCT semantics without retaining a second HashSet for the left.
            if right.remove(key.as_slice()) {
                rows.push(row);
            }
        }
        select_rows(left, &rows)
    }

    fn oracle_except(
        left: &RecordBatch,
        right: &RecordBatch,
        _config: &SetOperation,
    ) -> Result<RecordBatch> {
        validate_schema(left, right)?;
        let right = oracle_right_keys(right)?;
        let rows = oracle_unique_rows(left, |key| !right.contains(key))?;
        select_rows(left, &rows)
    }

    /// Confronto rigoroso: schema (nomi, tipi, nullability, metadata) e
    /// buffer di ogni colonna bit a bit (valori, null, bit f64, ordine).
    fn assert_batches_identical(expected: &RecordBatch, actual: &RecordBatch) {
        let expected_schema = expected.schema();
        let actual_schema = actual.schema();
        assert_eq!(
            expected_schema.fields().len(),
            actual_schema.fields().len(),
            "numero di colonne diverso"
        );
        for (expected_field, actual_field) in
            expected_schema.fields().iter().zip(actual_schema.fields())
        {
            assert_eq!(expected_field.name(), actual_field.name(), "nome colonna");
            assert_eq!(
                expected_field.data_type(),
                actual_field.data_type(),
                "tipo colonna {}",
                expected_field.name()
            );
            assert_eq!(
                expected_field.is_nullable(),
                actual_field.is_nullable(),
                "nullability colonna {}",
                expected_field.name()
            );
        }
        assert_eq!(
            expected_schema.metadata(),
            actual_schema.metadata(),
            "metadata schema"
        );
        assert_eq!(expected.num_rows(), actual.num_rows(), "numero righe");
        for (expected_column, actual_column) in expected.columns().iter().zip(actual.columns()) {
            assert_eq!(
                expected_column.to_data(),
                actual_column.to_data(),
                "buffer colonna non identici"
            );
        }
    }

    /// Fixture multi-tipo con casi limite: null in ogni colonna, stringhe
    /// vuote, NaN con payload diversi, +0.0/-0.0, duplicati esatti.
    // Lunga per costruzione: una colonna esplicita per tipo coperto, con i
    // casi limite elencati valore per valore (fixture di test, niente logica).
    #[allow(clippy::too_many_lines)]
    fn mixed_fixture() -> RecordBatch {
        let nan_a = f64::from_bits(0x7ff8_0000_0000_0001);
        let nan_b = f64::from_bits(0x7ff8_0000_0000_0042);
        let mut dictionary = StringDictionaryBuilder::<Int32Type>::new();
        for value in [
            Some("alpha"),
            None,
            Some("beta"),
            Some("alpha"),
            Some(""),
            None,
            Some("gamma"),
            Some("alpha"),
        ] {
            dictionary.append_option(value);
        }
        let dictionary: DictionaryArray<Int32Type> = dictionary.finish();
        RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                vec![
                    Field::new("s", DataType::Utf8, true),
                    Field::new("i", DataType::Int64, true),
                    Field::new("f", DataType::Float64, true),
                    Field::new("b", DataType::Boolean, true),
                    Field::new("u", DataType::UInt64, true),
                    Field::new("d", DataType::Date32, true),
                    Field::new(
                        "t",
                        DataType::Timestamp(
                            plenora_core::arrow::schema::TimeUnit::Millisecond,
                            None,
                        ),
                        true,
                    ),
                    Field::new("dec", DataType::Decimal128(38, 2), true),
                    Field::new("bin", DataType::Binary, true),
                    Field::new(
                        "dict",
                        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                        true,
                    ),
                ],
                std::iter::once(("origin".to_owned(), "oracle".to_owned())).collect(),
            )),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("dup"),
                    None,
                    Some(""),
                    Some("dup"),
                    Some("x"),
                    None,
                    Some("dup"),
                    Some("y"),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    None,
                    Some(2),
                    Some(1),
                    Some(-7),
                    None,
                    Some(1),
                    Some(9),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(0.0),
                    Some(-0.0),
                    Some(nan_a),
                    Some(0.0),
                    Some(1.5),
                    Some(nan_b),
                    Some(0.0),
                    None,
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    Some(false),
                    None,
                    Some(true),
                    Some(false),
                    Some(true),
                    Some(true),
                    None,
                ])),
                Arc::new(UInt64Array::from(vec![
                    Some(3),
                    Some(4),
                    Some(5),
                    Some(3),
                    None,
                    Some(6),
                    Some(3),
                    Some(7),
                ])),
                Arc::new(Date32Array::from(vec![
                    Some(10),
                    None,
                    Some(11),
                    Some(10),
                    Some(12),
                    Some(13),
                    Some(10),
                    Some(14),
                ])),
                Arc::new(TimestampMillisecondArray::from(vec![
                    Some(1000),
                    Some(2000),
                    None,
                    Some(1000),
                    Some(3000),
                    Some(4000),
                    Some(1000),
                    Some(5000),
                ])),
                Arc::new(
                    Decimal128Array::from(vec![
                        Some(12_345),
                        None,
                        Some(-1),
                        Some(12_345),
                        Some(0),
                        Some(777),
                        Some(12_345),
                        Some(-1),
                    ])
                    .with_precision_and_scale(38, 2)
                    .expect("precisione decimal"),
                ),
                Arc::new(BinaryArray::from(vec![
                    Some(&b"aa"[..]),
                    None,
                    Some(&b""[..]),
                    Some(&b"aa"[..]),
                    Some(&b"bb"[..]),
                    Some(&b"cc"[..]),
                    Some(&b"aa"[..]),
                    None,
                ])),
                Arc::new(dictionary),
            ],
        )
        .expect("fixture mista")
    }

    /// Fixture con stesso schema della mista ma valori completamente diversi
    /// (overlap 0%).
    fn disjoint_fixture() -> RecordBatch {
        let mut dictionary = StringDictionaryBuilder::<Int32Type>::new();
        for value in [Some("omega"), Some("psi"), None] {
            dictionary.append_option(value);
        }
        let dictionary: DictionaryArray<Int32Type> = dictionary.finish();
        let schema = mixed_fixture().schema();
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![Some("zz"), Some("ww"), Some("vv")])),
                Arc::new(Int64Array::from(vec![
                    Some(1_000_001),
                    Some(1_000_002),
                    Some(1_000_003),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(9_999.5),
                    Some(8_888.5),
                    Some(7_777.5),
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(false),
                    Some(true),
                    Some(false),
                ])),
                Arc::new(UInt64Array::from(vec![
                    Some(u64::MAX),
                    Some(u64::MAX - 1),
                    Some(u64::MAX - 2),
                ])),
                Arc::new(Date32Array::from(vec![
                    Some(99_001),
                    Some(99_002),
                    Some(99_003),
                ])),
                Arc::new(TimestampMillisecondArray::from(vec![
                    Some(9_000_001),
                    Some(9_000_002),
                    Some(9_000_003),
                ])),
                Arc::new(
                    Decimal128Array::from(vec![Some(9_000_001), Some(9_000_002), Some(9_000_003)])
                        .with_precision_and_scale(38, 2)
                        .expect("precisione decimal"),
                ),
                Arc::new(BinaryArray::from(vec![
                    Some(&b"zz"[..]),
                    Some(&b"yy"[..]),
                    Some(&b"xx"[..]),
                ])),
                Arc::new(dictionary),
            ],
        )
        .expect("fixture disgiunta")
    }

    fn empty_like(batch: &RecordBatch) -> RecordBatch {
        select_rows(batch, &[]).expect("batch vuoto")
    }

    /// Righe [0..4) della fixture mista + righe nuove: overlap 50% circa.
    fn half_overlap_fixture() -> RecordBatch {
        let mixed = mixed_fixture();
        let shared = select_rows(&mixed, &[0, 1, 2, 3]).expect("meta' condivisa");
        let disjoint = disjoint_fixture();
        concat_compatible(&shared, &disjoint, &Limits::default()).expect("half overlap")
    }

    fn assert_ops_match_oracle(left: &RecordBatch, right: &RecordBatch, scenario: &str) {
        let config = SetOperation {};
        let limits = Limits::default();

        let expected = oracle_union_distinct(left, right, &config, &limits)
            .unwrap_or_else(|error| panic!("oracolo union_distinct ({scenario}): {error}"));
        let actual = union_distinct(left, right, &config, &limits)
            .unwrap_or_else(|error| panic!("union_distinct ({scenario}): {error}"));
        assert_batches_identical(&expected, &actual);

        let expected = oracle_intersect(left, right, &config)
            .unwrap_or_else(|error| panic!("oracolo intersect ({scenario}): {error}"));
        let actual = intersect(left, right, &config)
            .unwrap_or_else(|error| panic!("intersect ({scenario}): {error}"));
        assert_batches_identical(&expected, &actual);

        let expected = oracle_except(left, right, &config)
            .unwrap_or_else(|error| panic!("oracolo except ({scenario}): {error}"));
        let actual = except(left, right, &config)
            .unwrap_or_else(|error| panic!("except ({scenario}): {error}"));
        assert_batches_identical(&expected, &actual);
    }

    #[test]
    fn fast_path_matches_oracle_full_overlap() {
        let mixed = mixed_fixture();
        assert_ops_match_oracle(&mixed, &mixed, "overlap 100%");
    }

    #[test]
    fn fast_path_matches_oracle_no_overlap() {
        let mixed = mixed_fixture();
        let disjoint = disjoint_fixture();
        assert_ops_match_oracle(&mixed, &disjoint, "overlap 0%");
    }

    #[test]
    fn fast_path_matches_oracle_half_overlap() {
        let mixed = mixed_fixture();
        let half = half_overlap_fixture();
        assert_ops_match_oracle(&mixed, &half, "overlap 50%");
    }

    #[test]
    fn fast_path_matches_oracle_empty_inputs() {
        let mixed = mixed_fixture();
        let empty = empty_like(&mixed);
        assert_ops_match_oracle(&empty, &mixed, "sinistra vuota");
        assert_ops_match_oracle(&mixed, &empty, "destra vuota");
        assert_ops_match_oracle(&empty, &empty, "entrambe vuote");
    }

    #[test]
    fn fast_path_matches_oracle_swapped_sides() {
        // intersect/except non sono commutative: verifica anche l'altro verso.
        let mixed = mixed_fixture();
        let half = half_overlap_fixture();
        assert_ops_match_oracle(&half, &mixed, "lati scambiati");
    }

    #[test]
    fn errors_match_oracle() {
        let config = SetOperation {};
        let mixed = mixed_fixture();

        // Schema incompatibile: stesso errore nei due percorsi.
        let renamed = {
            let mut fields = mixed
                .schema()
                .fields()
                .iter()
                .map(|field| field.as_ref().clone())
                .collect::<Vec<_>>();
            fields[0] = fields[0].clone().with_name("rinominata");
            RecordBatch::try_new(Arc::new(Schema::new(fields)), mixed.columns().to_vec())
                .expect("schema rinominato")
        };
        for (name, oracle, fast) in [
            (
                "union_distinct",
                oracle_union_distinct(&mixed, &renamed, &config, &Limits::default())
                    .map(|batch| batch.num_rows()),
                union_distinct(&mixed, &renamed, &config, &Limits::default())
                    .map(|batch| batch.num_rows()),
            ),
            (
                "intersect",
                oracle_intersect(&mixed, &renamed, &config).map(|batch| batch.num_rows()),
                intersect(&mixed, &renamed, &config).map(|batch| batch.num_rows()),
            ),
            (
                "except",
                oracle_except(&mixed, &renamed, &config).map(|batch| batch.num_rows()),
                except(&mixed, &renamed, &config).map(|batch| batch.num_rows()),
            ),
        ] {
            let oracle_error = oracle.expect_err("oracolo deve fallire").to_string();
            let fast_error = fast.expect_err("fast path deve fallire").to_string();
            assert_eq!(oracle_error, fast_error, "errore schema {name}");
        }

        // Tipo non supportato (Int32): stesso errore nei due percorsi.
        let unsupported = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("k", DataType::Int32, true)])),
            vec![Arc::new(plenora_core::arrow::array::Int32Array::from(
                vec![Some(1), None],
            ))],
        )
        .expect("fixture int32");
        for (name, oracle, fast) in [
            (
                "union_distinct",
                oracle_union_distinct(&unsupported, &unsupported, &config, &Limits::default())
                    .map(|batch| batch.num_rows()),
                union_distinct(&unsupported, &unsupported, &config, &Limits::default())
                    .map(|batch| batch.num_rows()),
            ),
            (
                "intersect",
                oracle_intersect(&unsupported, &unsupported, &config).map(|batch| batch.num_rows()),
                intersect(&unsupported, &unsupported, &config).map(|batch| batch.num_rows()),
            ),
            (
                "except",
                oracle_except(&unsupported, &unsupported, &config).map(|batch| batch.num_rows()),
                except(&unsupported, &unsupported, &config).map(|batch| batch.num_rows()),
            ),
        ] {
            let oracle_error = oracle.expect_err("oracolo deve fallire").to_string();
            let fast_error = fast.expect_err("fast path deve fallire").to_string();
            assert_eq!(oracle_error, fast_error, "errore tipo {name}");
        }

        // max_rows superato su union_distinct: stesso errore.
        let tight = Limits {
            max_rows: 4,
            ..Limits::default()
        };
        let oracle_error = oracle_union_distinct(&mixed, &mixed, &config, &tight)
            .expect_err("oracolo deve fallire")
            .to_string();
        let fast_error = union_distinct(&mixed, &mixed, &config, &tight)
            .expect_err("fast path deve fallire")
            .to_string();
        assert_eq!(oracle_error, fast_error, "errore max_rows union_distinct");
    }

    #[test]
    fn compact_keys_are_unambiguous_and_preserve_float_contract() {
        let nan_a = f64::from_bits(0x7ff8_0000_0000_0001);
        let nan_b = f64::from_bits(0x7ff8_0000_0000_0042);
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("left", DataType::Utf8, true),
                Field::new("right", DataType::Utf8, true),
                Field::new("number", DataType::Int64, false),
                Field::new("float", DataType::Float64, false),
                Field::new("flag", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("ab"),
                    Some("a"),
                    Some(""),
                    None,
                    Some("same"),
                    Some("same"),
                    Some("same"),
                    Some("same"),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("c"),
                    Some("bc"),
                    None,
                    Some(""),
                    Some("same"),
                    Some("same"),
                    Some("same"),
                    Some("same"),
                ])),
                Arc::new(Int64Array::from(vec![7; 8])),
                Arc::new(Float64Array::from(vec![
                    1.0, 1.0, 1.0, 1.0, nan_a, nan_b, 0.0, -0.0,
                ])),
                Arc::new(BooleanArray::from(vec![true; 8])),
            ],
        )
        .expect("set key fixture");
        let encoder = CompactRowEncoder::try_new(&batch).expect("encoder");
        let keys = (0..batch.num_rows())
            .map(|row| {
                let mut key = Vec::new();
                encoder.encode_into(row, &mut key).expect("key");
                key
            })
            .collect::<Vec<_>>();

        assert_ne!(keys[0], keys[1], "column boundaries must be framed");
        assert_ne!(keys[2], keys[3], "null and empty must remain distinct");
        assert_eq!(keys[4], keys[5], "all NaN payloads share set semantics");
        assert_ne!(keys[6], keys[7], "+0 and -0 preserve the prior contract");
    }
}
