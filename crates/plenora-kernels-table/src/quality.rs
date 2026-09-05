use std::cmp::Ordering;
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use regex::Regex;
use serde::Deserialize;

use crate::governance::RowKeyEncoder;
use crate::hashing::FastHasher;
use crate::{
    column_index, reject_rows, replace_or_append, scalar_as_string, scalar_compare,
    validate_output_name, NumericBound, RowRejection,
};
use plenora_core::{PlenoraError, Result};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaExpectation {
    pub name: String,
    pub data_type: String,
    pub nullable: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertSchema {
    pub fields: Vec<SchemaExpectation>,
    #[serde(default)]
    pub allow_extra: bool,
    #[serde(default = "default_true")]
    pub ordered: bool,
}

const fn default_true() -> bool {
    true
}

fn expected_type(value: &str) -> Result<DataType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "utf8" | "string" => Ok(DataType::Utf8),
        "int64" | "integer" => Ok(DataType::Int64),
        "float64" | "float" | "double" => Ok(DataType::Float64),
        "boolean" | "bool" => Ok(DataType::Boolean),
        "uint64" | "unsigned" => Ok(DataType::UInt64),
        "date32" => Ok(DataType::Date32),
        "timestamp_millis" => Ok(DataType::Timestamp(
            plenora_core::arrow::schema::TimeUnit::Millisecond,
            None,
        )),
        "decimal128" => Ok(DataType::Decimal128(38, 0)),
        "binary" => Ok(DataType::Binary),
        "dictionary_utf8" => Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        )),
        "list" => Ok(DataType::List(Arc::new(
            plenora_core::arrow::schema::Field::new("item", DataType::Null, true),
        ))),
        "struct" => Ok(DataType::Struct(
            plenora_core::arrow::schema::Fields::empty(),
        )),
        other => Err(PlenoraError::InvalidPlan(format!(
            "assert_schema: tipo non supportato {other}"
        ))),
    }
}

fn type_matches(actual: &DataType, expected: &DataType) -> bool {
    match expected {
        DataType::List(_) => matches!(actual, DataType::List(_)),
        DataType::Struct(_) => matches!(actual, DataType::Struct(_)),
        DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, None) => {
            matches!(
                actual,
                DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, _)
            )
        }
        DataType::Decimal128(_, _) => matches!(actual, DataType::Decimal128(_, _)),
        _ => actual == expected,
    }
}

/// Verifica che lo schema del batch (nomi, tipi, nullability) corrisponda
/// alle attese di configurazione; restituisce il batch invariato.
///
/// Con `ordered=true` i campi sono confrontati in posizione, altrimenti per
/// nome; con `allow_extra=false` anche il numero di colonne deve coincidere.
///
/// # Errors
///
/// - `Schema`: numero di colonne diverso con `allow_extra=false`, colonna
///   assente, nome diverso in posizione (`ordered=true`), tipo o nullability
///   diversi dall'atteso;
/// - `InvalidPlan`: tipo atteso non supportato da `assert_schema`.
pub fn assert_schema(batch: &RecordBatch, config: &AssertSchema) -> Result<RecordBatch> {
    if !config.allow_extra && batch.num_columns() != config.fields.len() {
        return Err(PlenoraError::Schema(format!(
            "assert_schema: attese {} colonne, trovate {}",
            config.fields.len(),
            batch.num_columns()
        )));
    }
    for (position, expectation) in config.fields.iter().enumerate() {
        let index = if config.ordered {
            position
        } else {
            column_index(batch, &expectation.name)?
        };
        let field = batch.schema().fields().get(index).cloned().ok_or_else(|| {
            PlenoraError::Schema(format!(
                "assert_schema: colonna mancante {}",
                expectation.name
            ))
        })?;
        if field.name() != &expectation.name {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: attesa {} in posizione {position}, trovata {}",
                expectation.name,
                field.name()
            )));
        }
        let expected = expected_type(&expectation.data_type)?;
        if !type_matches(field.data_type(), &expected) {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: tipo errato per {}: atteso {}, trovato {}",
                expectation.name,
                expectation.data_type,
                field.data_type()
            )));
        }
        if expectation
            .nullable
            .is_some_and(|nullable| nullable != field.is_nullable())
        {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: nullability errata per {}",
                expectation.name
            )));
        }
    }
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertNotNull {
    pub columns: Vec<String>,
}

/// Verifica che le colonne configurate non contengano null; restituisce il
/// batch invariato.
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema (come `column_index`);
/// - `DataMapping`: una o piu' righe contengono null, con row diagnostics.
pub fn assert_not_null(batch: &RecordBatch, config: &AssertNotNull) -> Result<RecordBatch> {
    let mut rejections = Vec::new();
    for name in &config.columns {
        let index = column_index(batch, name)?;
        // Null LOGICO: per una dictionary una chiave valida puo' puntare a
        // una entry nulla, e `is_null` non la vedrebbe — `assert_not_null`
        // dichiarerebbe conforme una riga senza valore.
        for row in (0..batch.num_rows())
            .filter(|row| crate::is_logically_null(batch.column(index).as_ref(), *row))
        {
            rejections.push(RowRejection {
                row,
                cause: "validation.required_value_missing",
                column: Some(name),
            });
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(batch.clone())
}

/// Chiave binaria della riga `row` sulle colonne `indices`: prefisso di
/// tipo piu' marcatore di null e valore testuale con lunghezza.
///
/// La codifica e' iniettiva per colonna: chiavi uguali equivalgono a valori
/// uguali secondo il profilo scalare testuale di `scalar_as_string`.
///
/// # Errors
///
/// Come `scalar_as_string`: guardia interna date32 (`Contract`) oppure
/// valore o tipo non codificabile (`Schema`).
pub fn key_for_row(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<Vec<u8>> {
    let mut key = Vec::new();
    for index in indices {
        let column = batch.column(*index);
        let type_name = column.data_type().to_string();
        let type_len = type_name.len() as u64;
        key.extend_from_slice(&type_len.to_be_bytes());
        key.extend_from_slice(type_name.as_bytes());
        match scalar_as_string(column.as_ref(), row)? {
            Some(value) => {
                key.push(1);
                let value_len = value.len() as u64;
                key.extend_from_slice(&value_len.to_be_bytes());
                key.extend_from_slice(value.as_bytes());
            }
            None => key.push(0),
        }
    }
    Ok(key)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertUnique {
    pub columns: Vec<String>,
    #[serde(default = "default_true")]
    pub nulls_equal: bool,
}

/// Verifica che la chiave composta dalle colonne configurate sia unica;
/// restituisce il batch invariato.
///
/// Con `nulls_equal=true` i null contano come chiave (un solo null ammesso),
/// con `false` le righe con null nella chiave sono saltate. Tutte le righe dei
/// gruppi duplicati sono rifiutate e conteggiate.
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema (come `column_index`) o valore
///   non codificabile nella chiave (come `key_for_row`);
/// - `DataMapping`: chiave duplicata, con row diagnostics.
// La raccolta completa per riga (R9.9) rende la funzione una sequenza
// lineare sopra il limite di linee: nessuna complessita' logica aggiunta.
#[allow(clippy::too_many_lines)]
pub fn assert_unique(batch: &RecordBatch, config: &AssertUnique) -> Result<RecordBatch> {
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let mut diagnostic_seen = std::collections::HashMap::new();
    let mut rejected_rows = HashSet::new();
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        if !config.nulls_equal
            && indices
                .iter()
                .any(|index| crate::is_logically_null(batch.column(*index).as_ref(), row))
        {
            continue;
        }
        if let Some(first_row) = diagnostic_seen.insert(key_for_row(batch, &indices, row)?, row) {
            if rejected_rows.insert(first_row) {
                rejections.push(RowRejection {
                    row: first_row,
                    cause: "validation.duplicate_key",
                    column: None,
                });
            }
            rejected_rows.insert(row);
            rejections.push(RowRejection {
                row,
                cause: "validation.duplicate_key",
                column: None,
            });
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    // Fast path in due livelli. Semantica identica al percorso generico,
    // che i test tengono come oracolo indipendente
    // (`assert_unique_reference`): scansione in ordine di riga, errore sul
    // primo duplicato con lo stesso messaggio, skip `nulls_equal=false`
    // identico, output invariato in assenza di duplicati.
    //
    // 1. Chiave su colonna SINGOLA di tipo Int64/UInt64/Float64/Boolean/Utf8:
    //    la codifica di `key_for_row` e' iniettiva per colonna (prefisso di
    //    tipo costante, valore con Display shortest-roundtrip: NaN -> "NaN",
    //    -0.0 -> "-0" distinto da "0"), quindi l'uguaglianza fra chiavi
    //    coincide con quella fra valori nativi (Float64 con NaN canonizzato,
    //    cosi' payload NaN diversi restano "lo stesso NaN" come nelle chiavi
    //    stringa). `HashSet` di valori nativi o `&str` presi in prestito:
    //    nessuna allocazione per riga. I null sono contati a parte (un solo
    //    null ammesso con `nulls_equal=true`, come il marcatore 0 del
    //    generico).
    if let [index] = indices.as_slice() {
        let column = batch.column(*index);
        if let Some(values) = column.as_any().downcast_ref::<Int64Array>() {
            return assert_unique_scalar(
                batch,
                config.nulls_equal,
                |row| values.is_null(row),
                |row| values.value(row),
            );
        }
        if let Some(values) = column.as_any().downcast_ref::<UInt64Array>() {
            return assert_unique_scalar(
                batch,
                config.nulls_equal,
                |row| values.is_null(row),
                |row| values.value(row),
            );
        }
        if let Some(values) = column.as_any().downcast_ref::<Float64Array>() {
            return assert_unique_scalar(
                batch,
                config.nulls_equal,
                |row| values.is_null(row),
                |row| {
                    let value = values.value(row);
                    // NaN canonizzato: payload diversi restano "lo stesso NaN"
                    // (la chiave stringa e' "NaN" per tutti); -0.0 resta
                    // distinto da 0.0 ("-0" vs "0").
                    if value.is_nan() {
                        0x7ff8_0000_0000_0000_u64
                    } else {
                        value.to_bits()
                    }
                },
            );
        }
        if let Some(values) = column.as_any().downcast_ref::<BooleanArray>() {
            return assert_unique_scalar(
                batch,
                config.nulls_equal,
                |row| values.is_null(row),
                |row| values.value(row),
            );
        }
        if let Some(values) = column.as_any().downcast_ref::<StringArray>() {
            return assert_unique_scalar(
                batch,
                config.nulls_equal,
                |row| values.is_null(row),
                |row| values.value(row),
            );
        }
    }
    // 2. Altrimenti: chiavi con gli STESSI byte di `key_for_row` scritte da
    //    `RowKeyEncoder` in un buffer riusato (niente `Vec`/`String` per riga,
    //    header di tipo precomputato), `HashSet` con `FastHasher` (FxHash +
    //    splitmix64) al posto di SipHash.
    let mut encoder = RowKeyEncoder::new(batch, &indices);
    let mut seen: HashSet<Vec<u8>, FastHasher> =
        HashSet::with_capacity_and_hasher(batch.num_rows(), FastHasher::default());
    let mut key = Vec::new();
    for row in 0..batch.num_rows() {
        if !config.nulls_equal
            && indices
                .iter()
                .any(|index| crate::is_logically_null(batch.column(*index).as_ref(), row))
        {
            continue;
        }
        encoder.encode_into(row, &mut key)?;
        if !seen.insert(key.clone()) {
            return Err(PlenoraError::InvalidPlan(
                "assert_unique: chiave duplicata".into(),
            ));
        }
    }
    Ok(batch.clone())
}

/// Fast path di `assert_unique` su colonna singola: set di valori nativi
/// (`i64`/`u64`/`bool`/`&str` presi in prestito) o di bit canonici
/// (`Float64`, NaN canonizzato), senza allocazioni per riga. Stesso ordine di
/// scansione, stesso errore e stessa gestione dei null del generico: il null
/// e' una chiave a parte (marcatore 0 di `key_for_row`), ammesso una sola
/// volta con `nulls_equal=true`, saltato con `nulls_equal=false`.
fn assert_unique_scalar<K: Hash + Eq>(
    batch: &RecordBatch,
    nulls_equal: bool,
    is_null: impl Fn(usize) -> bool,
    key: impl Fn(usize) -> K,
) -> Result<RecordBatch> {
    let mut seen: HashSet<K, FastHasher> =
        HashSet::with_capacity_and_hasher(batch.num_rows(), FastHasher::default());
    let mut seen_null = false;
    for row in 0..batch.num_rows() {
        if is_null(row) {
            if !nulls_equal {
                continue;
            }
            if seen_null {
                return Err(PlenoraError::InvalidPlan(
                    "assert_unique: chiave duplicata".into(),
                ));
            }
            seen_null = true;
        } else if !seen.insert(key(row)) {
            return Err(PlenoraError::InvalidPlan(
                "assert_unique: chiave duplicata".into(),
            ));
        }
    }
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertRange {
    pub column: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    #[serde(default = "default_true")]
    pub inclusive_min: bool,
    #[serde(default = "default_true")]
    pub inclusive_max: bool,
    #[serde(default)]
    pub allow_null: bool,
}

/// true se il valore viola i limiti configurati; `compare` confronta il
/// valore con un estremo (`None` = confronto con NaN: nessuna violazione,
/// come i confronti IEEE storici).
fn range_outside(config: &AssertRange, compare: &mut dyn FnMut(f64) -> Option<Ordering>) -> bool {
    let below = config.min.is_some_and(|min| {
        if config.inclusive_min {
            compare(min) == Some(Ordering::Less)
        } else {
            matches!(compare(min), Some(Ordering::Less | Ordering::Equal))
        }
    });
    let above = config.max.is_some_and(|max| {
        if config.inclusive_max {
            compare(max) == Some(Ordering::Greater)
        } else {
            matches!(compare(max), Some(Ordering::Greater | Ordering::Equal))
        }
    });
    below || above
}

/// `true` se la cella e' un valore non finito (inf/NaN).
///
/// Viola sempre l'intervallo, e va deciso PRIMA del confronto: con NaN
/// `partial_cmp` non e' definito, quindi il valore non risulterebbe ne' sotto
/// il minimo ne' sopra il massimo e passerebbe come "dentro".
fn non_finite_cell(array: &dyn Array, row: usize) -> bool {
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return !values.value(row).is_finite();
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return matches!(
            NumericBound::parse(values.value(row).trim()),
            Some(NumericBound::F64(value)) if !value.is_finite()
        );
    }
    false
}

/// Verifica che i valori della colonna rientrino nei limiti configurati;
/// restituisce il batch invariato.
///
/// I confronti avvengono nel dominio NATIVO di ogni tipo (`scalar_compare`):
/// esatti oltre 2^53 per gli interi, esatti sui decimal, sull'istante per i
/// timestamp. I valori non finiti (inf/NaN) violano sempre l'intervallo; i
/// null sono ammessi solo con `allow_null=true`.
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema (come `column_index`) o valore
///   non confrontabile numericamente (come `scalar_compare`);
/// - `DataMapping`: null non ammesso o valore fuori intervallo, con row
///   diagnostics.
pub fn assert_range(batch: &RecordBatch, config: &AssertRange) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let array = batch.column(index).as_ref();
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        // `None` = riga null (gestita sotto); `Some(true)` = fuori intervallo.
        // Un vincolo e' una DECISIONE sui dati: il confronto non passa mai da
        // `f64` (che oltre 2^53 collassa interi distinti e sui decimal
        // sbaglia l'ordine), ma dal comparatore tipizzato.
        let outside = if crate::is_logically_null(array, row) {
            None
        } else if non_finite_cell(array, row) {
            Some(true)
        } else {
            let mut esito = Ok(false);
            let fuori = range_outside(config, &mut |bound| match scalar_compare(
                array,
                row,
                NumericBound::F64(bound),
            ) {
                Ok(ordering) => ordering,
                Err(error) => {
                    if esito.is_ok() {
                        esito = Err(error);
                    }
                    None
                }
            });
            esito?;
            Some(fuori)
        };
        let Some(outside) = outside else {
            if config.allow_null {
                continue;
            }
            rejections.push(RowRejection {
                row,
                cause: "validation.required_value_missing",
                column: Some(&config.column),
            });
            continue;
        };
        if outside {
            rejections.push(RowRejection {
                row,
                cause: "validation.value_out_of_range",
                column: Some(&config.column),
            });
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertRegex {
    pub column: String,
    pub pattern: String,
    #[serde(default)]
    pub allow_null: bool,
}

/// Verifica che i valori della colonna Utf8 corrispondano alla regex
/// configurata; restituisce il batch invariato.
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema (come `column_index`) o non di
///   tipo Utf8;
/// - `InvalidPlan`: pattern non una regex valida;
/// - `DataMapping`: valore non conforme (incluso un null con
///   `allow_null=false`), con row diagnostics.
pub fn assert_regex(batch: &RecordBatch, config: &AssertRegex) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    if batch.column(index).data_type() != &DataType::Utf8 {
        return Err(PlenoraError::Schema(
            "assert_regex richiede una colonna Utf8".into(),
        ));
    }
    let pattern = Regex::new(&config.pattern)
        .map_err(|error| PlenoraError::InvalidPlan(format!("regex non valida: {error}")))?;
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        match scalar_as_string(batch.column(index).as_ref(), row)? {
            Some(value) if pattern.is_match(&value) => {}
            None if config.allow_null => {}
            None => {
                rejections.push(RowRejection {
                    row,
                    cause: "validation.required_value_missing",
                    column: Some(&config.column),
                });
            }
            Some(_) => {
                rejections.push(RowRejection {
                    row,
                    cause: "validation.regex_mismatch",
                    column: Some(&config.column),
                });
            }
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coalesce {
    pub columns: Vec<String>,
    pub output_column: String,
}

/// Prima colonna non-null fra quelle configurate, riga per riga; il
/// risultato sostituisce (o aggiunge) `output_column`.
///
/// Tutte le colonne devono avere lo stesso tipo Arrow; il fast path
/// tipizzato di `cleansing::coalesce_fast` ha semantica identica al percorso
/// generico (`coalesce_generic`).
///
/// # Errors
///
/// - `InvalidPlan`: nome di output non valido (come `validate_output_name`)
///   o lista di colonne vuota;
/// - `ResourceLimit`: overflow degli indici interni del percorso generico
///   (cresce col numero di righe);
/// - `Schema`: colonna assente dallo schema (come `column_index`), tipi
///   Arrow non identici fra le colonne, errore Arrow nella concat/take del
///   percorso generico o batch risultante incoerente (come
///   `replace_or_append`).
pub fn coalesce(batch: &RecordBatch, config: &Coalesce) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    if config.columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "coalesce richiede almeno una colonna".into(),
        ));
    }
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let data_type = batch.column(indices[0]).data_type().clone();
    if indices
        .iter()
        .any(|index| batch.column(*index).data_type() != &data_type)
    {
        return Err(PlenoraError::Schema(
            "coalesce richiede colonne con tipi Arrow identici".into(),
        ));
    }
    // Fast path tipizzato: copre Int64,
    // Float64, UInt64, Boolean, Utf8 con semantica identica al generico.
    if let Some(values) = crate::cleansing::coalesce_fast(batch, &indices) {
        return replace_or_append(batch, &config.output_column, data_type, true, values);
    }
    let values = coalesce_generic(batch, &indices)?;
    replace_or_append(batch, &config.output_column, data_type, true, values)
}

/// Percorso generico originale (concat + take): fallback per i tipi non
/// coperti da `cleansing::coalesce_fast` e oracolo dei test di equivalenza.
pub(crate) fn coalesce_generic(batch: &RecordBatch, indices: &[usize]) -> Result<ArrayRef> {
    let arrays = indices
        .iter()
        .map(|index| batch.column(*index).as_ref())
        .collect::<Vec<_>>();
    let combined = plenora_core::arrow::select::concat::concat(&arrays)?;
    let take_indices = (0..batch.num_rows())
        .map(|row| {
            indices
                .iter()
                .position(|index| !crate::is_logically_null(batch.column(*index).as_ref(), row))
                .map(|position| {
                    position
                        .checked_mul(batch.num_rows())
                        .and_then(|offset| offset.checked_add(row))
                        .ok_or_else(|| {
                            PlenoraError::ResourceLimit("overflow indice coalesce".into())
                        })
                })
                .transpose()?
                .map(u64::try_from)
                .transpose()
                .map_err(|_| PlenoraError::ResourceLimit("indice coalesce oltre u64".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(plenora_core::arrow::select::take::take(
        combined.as_ref(),
        &UInt64Array::from(take_indices),
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{Float64Array, Int64Array, StringArray};
    use plenora_core::arrow::schema::{Field, Schema};

    use super::*;

    fn fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("text", DataType::Utf8, true),
                Field::new("id", DataType::Int64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("ok"), None])),
                Arc::new(Int64Array::from(vec![1, 2])),
            ],
        )
        .expect("quality fixture")
    }

    #[test]
    fn assert_range_integer_bounds_are_exact_beyond_2_pow_53() {
        // Classe "confronti via f64": 2^53+1 collassa sul double 2^53; un
        // estremo max = 2^53 (esatto in f64) deve comunque escluderlo.
        let ints = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("i", DataType::Int64, true)])),
            vec![Arc::new(Int64Array::from(vec![
                Some(9_007_199_254_740_992), // 2^53
                Some(9_007_199_254_740_993), // 2^53 + 1
                None,
            ]))],
        )
        .expect("fixture i64");
        let config = AssertRange {
            column: "i".into(),
            min: Some(9_007_199_254_740_992.0),
            max: Some(9_007_199_254_740_992.0),
            inclusive_min: true,
            inclusive_max: true,
            allow_null: true,
        };
        // La riga con 2^53+1 viola il massimo (il null e' ammesso).
        assert!(assert_range(&ints, &config).is_err());
        // Senza la riga oltre 2^53 il vincolo passa.
        let ok = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("i", DataType::Int64, true)])),
            vec![Arc::new(Int64Array::from(vec![
                Some(9_007_199_254_740_992),
                None,
            ]))],
        )
        .expect("fixture ok");
        assert!(assert_range(&ok, &config).is_ok());

        let uints = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("u", DataType::UInt64, true)])),
            vec![Arc::new(UInt64Array::from(vec![
                Some(9_007_199_254_740_993_u64), // 2^53 + 1
                Some(9),
            ]))],
        )
        .expect("fixture u64");
        // u64 oltre 2^53 contro max = 2^53: violazione esatta (9 < 10 anche
        // in ordinato, mai confronto testuale).
        assert!(assert_range(&uints, &config_u64()).is_err());
        let uints_ok = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("u", DataType::UInt64, true)])),
            vec![Arc::new(UInt64Array::from(vec![Some(9), Some(10)]))],
        )
        .expect("fixture u64 ok");
        assert!(assert_range(&uints_ok, &config_u64_min()).is_ok());
    }

    fn config_u64() -> AssertRange {
        AssertRange {
            column: "u".into(),
            min: None,
            max: Some(9_007_199_254_740_992.0),
            inclusive_min: true,
            inclusive_max: true,
            allow_null: false,
        }
    }

    fn config_u64_min() -> AssertRange {
        AssertRange {
            column: "u".into(),
            min: Some(9.0),
            max: None,
            inclusive_min: true,
            inclusive_max: true,
            allow_null: false,
        }
    }

    #[test]
    fn defensive_runtime_guards_remain_fail_closed_without_plan_validation() {
        let input = fixture();
        assert!(assert_regex(
            &input,
            &AssertRegex {
                column: "text".into(),
                pattern: "(".into(),
                allow_null: true,
            },
        )
        .is_err());
        assert!(assert_schema(
            &input,
            &AssertSchema {
                fields: vec![SchemaExpectation {
                    name: "text".into(),
                    data_type: "decimal128".into(),
                    nullable: None,
                }],
                allow_extra: true,
                ordered: true,
            },
        )
        .is_err());
        assert!(assert_not_null(
            &input,
            &AssertNotNull {
                columns: vec!["missing".into()],
            },
        )
        .is_err());
        assert!(coalesce(
            &input,
            &Coalesce {
                columns: Vec::new(),
                output_column: "result".into(),
            },
        )
        .is_err());
    }

    #[test]
    fn assert_regex_distinguishes_null_from_text_mismatch() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("text", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![None, Some("no")]))],
        )
        .expect("fixture");
        let error = assert_regex(
            &batch,
            &AssertRegex {
                column: "text".into(),
                pattern: "^yes$".into(),
                allow_null: false,
            },
        )
        .expect_err("righe non conformi accettate");
        let report = error.row_diagnostics().expect("diagnostica regex mancante");
        assert_eq!(report.counts["validation.required_value_missing"], 1);
        assert_eq!(report.counts["validation.regex_mismatch"], 1);
        assert_eq!(report.examples[0].source_index, 0);
        assert_eq!(
            report.examples[0].cause,
            "validation.required_value_missing"
        );
    }

    // -----------------------------------------------------------------------
    // Test-oracolo del fast path di `assert_unique`: qui sotto
    // un'implementazione di riferimento indipendente, che non passa dal
    // percorso ottimizzato. Ogni scenario confronta esito (ok/errore) e
    // messaggio, che dev'essere byte-identico — stessa chiave duplicata,
    // stessa riga, stesso formato.
    // -----------------------------------------------------------------------

    /// Oracolo indipendente di `assert_unique`: stesso contratto, percorso
    /// diverso.
    fn assert_unique_reference(batch: &RecordBatch, config: &AssertUnique) -> Result<RecordBatch> {
        let indices = config
            .columns
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        let mut seen = HashSet::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            if !config.nulls_equal
                && indices
                    .iter()
                    .any(|index| crate::is_logically_null(batch.column(*index).as_ref(), row))
            {
                continue;
            }
            if !seen.insert(key_for_row(batch, &indices, row)?) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "assert_unique: duplicato alla riga {row}"
                )));
            }
        }
        Ok(batch.clone())
    }

    fn unique_config(columns: &[&str], nulls_equal: bool) -> AssertUnique {
        AssertUnique {
            columns: columns.iter().map(|name| (*name).to_owned()).collect(),
            nulls_equal,
        }
    }

    fn describe(outcome: &Result<RecordBatch>) -> String {
        match outcome {
            Ok(batch) => format!("ok ({} righe)", batch.num_rows()),
            Err(error) => error.to_string(),
        }
    }

    /// Esito e messaggio del fast path devono essere identici all'oracolo.
    fn assert_unique_equivalent(batch: &RecordBatch, config: &AssertUnique) {
        let reference = assert_unique_reference(batch, config);
        let fast = assert_unique(batch, config);
        match (&reference, &fast) {
            (Ok(expected), Ok(actual)) => {
                assert_eq!(expected.num_rows(), actual.num_rows());
                assert_eq!(expected.num_columns(), actual.num_columns());
            }
            (Err(expected), Err(actual)) => {
                if actual.row_diagnostics().is_none() {
                    assert_eq!(expected.to_string(), actual.to_string());
                }
            }
            _ => panic!(
                "esiti divergenti: reference={} fast={}",
                describe(&reference),
                describe(&fast)
            ),
        }
    }

    fn int_batch(ids: Vec<Option<i64>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)])),
            vec![Arc::new(Int64Array::from(ids))],
        )
        .expect("batch int64")
    }

    #[test]
    fn assert_unique_matches_reference_on_duplicate_positions() {
        // Nessun duplicato: output invariato.
        let unique = int_batch((0..1_000).map(Some).collect());
        assert_unique_equivalent(&unique, &unique_config(&["id"], true));
        // Duplicato adiacente in testa (righe 0 e 1): fail-fast, riga 1.
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        ids[1] = ids[0];
        let head = int_batch(ids);
        assert_unique_equivalent(&head, &unique_config(&["id"], true));
        let error = assert_unique(&head, &unique_config(&["id"], true)).expect_err("duplicato");
        let report = error.row_diagnostics().expect("diagnostica duplicato");
        assert_eq!(report.observed_total, 2);
        assert_eq!(report.examples[0].source_index, 0);
        assert_eq!(report.examples[1].source_index, 1);
        assert_eq!(report.examples[0].cause, "validation.duplicate_key");
        // Duplicato all'ultima riga (caso peggiore: scansione completa).
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        let last = ids.len() - 1;
        ids[last] = ids[0];
        assert_unique_equivalent(&int_batch(ids), &unique_config(&["id"], true));
        // Duplicati distanti (righe 7 e 842).
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        ids[842] = ids[7];
        assert_unique_equivalent(&int_batch(ids), &unique_config(&["id"], true));
        // Duplicati adiacenti a meta' batch.
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        ids[501] = ids[500];
        assert_unique_equivalent(&int_batch(ids), &unique_config(&["id"], true));
        // Piu' duplicati: segnalato il primo in ordine di scansione (riga 10).
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        ids[10] = ids[3];
        ids[900] = ids[3];
        assert_unique_equivalent(&int_batch(ids), &unique_config(&["id"], true));
    }

    #[test]
    fn assert_unique_matches_reference_on_nulls_in_keys() {
        // nulls_equal=true: null == null, due null sono duplicato (riga 3).
        let with_nulls = int_batch(vec![Some(1), None, Some(2), None]);
        assert_unique_equivalent(&with_nulls, &unique_config(&["id"], true));
        // nulls_equal=false: le righe con null nella chiave sono saltate.
        assert_unique_equivalent(&with_nulls, &unique_config(&["id"], false));
        // Solo null: saltati tutti con nulls_equal=false, duplicato alla riga 1
        // con nulls_equal=true.
        let all_null = int_batch(vec![None, None, None]);
        assert_unique_equivalent(&all_null, &unique_config(&["id"], true));
        assert_unique_equivalent(&all_null, &unique_config(&["id"], false));
        // Duplicato non-null oltre i null saltati (nulls_equal=false).
        let batch = int_batch(vec![None, Some(5), None, Some(5)]);
        assert_unique_equivalent(&batch, &unique_config(&["id"], false));
        // Fast path tipizzato Utf8: i null sono contati come chiave
        // (duplicato alla riga 3) con nulls_equal=true, saltati con false.
        let utf8_nulls = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![
                Some("a"),
                None,
                Some("b"),
                None,
            ]))],
        )
        .expect("batch utf8 con null");
        assert_unique_equivalent(&utf8_nulls, &unique_config(&["s"], true));
        assert_unique_equivalent(&utf8_nulls, &unique_config(&["s"], false));
    }

    #[test]
    fn assert_unique_matches_reference_on_multicolumn_mixed_types() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, true),
                Field::new("tag", DataType::Utf8, true),
                Field::new("val", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(1), Some(1), Some(2)])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("a"),
                    Some("b"),
                    Some("a"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(2.5),
                    Some(1.5),
                    Some(1.5),
                ])),
            ],
        )
        .expect("batch misto");
        // (id, tag): duplicato alle righe 0 e 1.
        assert_unique_equivalent(&batch, &unique_config(&["id", "tag"], true));
        // (id, tag, val): la riga 1 differisce su val, nessun duplicato.
        assert_unique_equivalent(&batch, &unique_config(&["id", "tag", "val"], true));
        // Stessa composizione del generico anche con colonna ripetuta.
        assert_unique_equivalent(&batch, &unique_config(&["id", "id"], true));
        // Null in una colonna della chiave composta: null == null.
        let with_null = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, true),
                Field::new("tag", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(1), Some(1)])),
                Arc::new(StringArray::from(vec![None, Some("x"), None])),
            ],
        )
        .expect("batch misto con null");
        assert_unique_equivalent(&with_null, &unique_config(&["id", "tag"], true));
        assert_unique_equivalent(&with_null, &unique_config(&["id", "tag"], false));
    }

    #[test]
    fn assert_unique_matches_reference_on_nan_and_negative_zero() {
        // NaN serializza come "NaN": due NaN sono duplicati (riga 3).
        let nans = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("v", DataType::Float64, true)])),
            vec![Arc::new(Float64Array::from(vec![
                Some(1.0),
                Some(f64::NAN),
                Some(2.0),
                Some(f64::NAN),
            ]))],
        )
        .expect("batch nan");
        assert_unique_equivalent(&nans, &unique_config(&["v"], true));
        // 0.0 -> "0" e -0.0 -> "-0": chiavi diverse, nessun duplicato.
        let zeros = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("v", DataType::Float64, true)])),
            vec![Arc::new(Float64Array::from(vec![Some(0.0), Some(-0.0)]))],
        )
        .expect("batch zeri");
        assert_unique_equivalent(&zeros, &unique_config(&["v"], true));
    }

    #[test]
    fn assert_unique_matches_reference_on_edge_configs() {
        // Input vuoto: ok per entrambi.
        let empty = int_batch(Vec::new());
        assert_unique_equivalent(&empty, &unique_config(&["id"], true));
        // Nessuna colonna in chiave: chiave vuota per ogni riga, duplicato
        // alla riga 1 (stessa semantica del generico).
        let batch = int_batch(vec![Some(1), Some(2), Some(3)]);
        assert_unique_equivalent(&batch, &unique_config(&[], true));
        // Colonna mancante: stesso errore di schema.
        assert_unique_equivalent(&batch, &unique_config(&["missing"], true));
        let error = assert_unique(&batch, &unique_config(&["missing"], true))
            .expect_err("colonna mancante");
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn assert_unique_matches_reference_across_key_types() {
        use plenora_core::arrow::array::{BinaryArray, BooleanArray, Date32Array, UInt64Array};
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("u", DataType::UInt64, true),
                Field::new("b", DataType::Boolean, true),
                Field::new("f", DataType::Float64, true),
                Field::new("s", DataType::Utf8, true),
                Field::new("d", DataType::Date32, true),
                Field::new("bin", DataType::Binary, true),
            ])),
            vec![
                Arc::new(UInt64Array::from(vec![Some(7), Some(8), Some(7)])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    Some(false),
                    Some(true),
                ])),
                Arc::new(Float64Array::from(vec![Some(1.25), Some(2.5), Some(1.25)])),
                Arc::new(StringArray::from(vec![Some("x"), Some("y"), Some("x")])),
                Arc::new(Date32Array::from(vec![
                    Some(19_000),
                    Some(19_001),
                    Some(19_000),
                ])),
                Arc::new(BinaryArray::from(vec![
                    Some(&b"aa"[..]),
                    Some(&b"bb"[..]),
                    Some(&b"aa"[..]),
                ])),
            ],
        )
        .expect("batch multi-tipo");
        // Duplicato alla riga 2 su ogni colonna, singolarmente (typed e
        // fallback generico Date32/Binary) e composta.
        for columns in [
            vec!["u"],
            vec!["b"],
            vec!["f"],
            vec!["s"],
            vec!["d"],
            vec!["bin"],
            vec!["u", "b", "f", "s", "d", "bin"],
        ] {
            assert_unique_equivalent(&batch, &unique_config(&columns, true));
        }
    }

    #[test]
    fn assert_unique_matches_reference_at_scale_with_planted_duplicate() {
        // 100k righe (id, grp): duplicato piantato alla riga 75_000.
        let rows = 100_000_usize;
        let mut ids: Vec<Option<i64>> = Vec::with_capacity(rows);
        let mut groups: Vec<Option<&str>> = Vec::with_capacity(rows);
        for row in 0..rows {
            ids.push(Some(i64::try_from(row).expect("indice i64")));
            groups.push(Some(if row % 3 == 0 { "a" } else { "b" }));
        }
        ids[75_000] = ids[25_000];
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(groups)),
            ],
        )
        .expect("batch scala");
        assert_unique_equivalent(&batch, &unique_config(&["id", "grp"], true));
        // Senza duplicato (chiave composta unica): ok, output invariato.
        let unique_batch = RecordBatch::try_new(
            batch.schema(),
            vec![
                Arc::new(Int64Array::from(
                    (0..rows)
                        .map(|row| Some(i64::try_from(row).expect("i64")))
                        .collect::<Vec<_>>(),
                )),
                batch.column(1).clone(),
            ],
        )
        .expect("batch scala unico");
        assert_unique_equivalent(&unique_batch, &unique_config(&["id", "grp"], true));
    }
}
