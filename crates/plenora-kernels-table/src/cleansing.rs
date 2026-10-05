//! Pulizia dei valori: `table.fill_na`, `table.replace`, `table.type_cast`,
//! e il percorso veloce di `table.coalesce` (il kernel generico è in
//! [`crate::quality`]).
//!
//! Semantica, schema, ordine ed errori per operazione: le schede
//! `docs/schede/<id>.md`, raccolte in `docs/operazioni.md`.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use num_traits::ToPrimitive;
use plenora_core::arrow::array::{
    builder::{
        BinaryBuilder, BooleanBuilder, PrimitiveBuilder, StringBuilder, StringDictionaryBuilder,
    },
    types::{ArrowPrimitiveType, Float64Type, Int32Type, Int64Type, UInt64Type},
    Array, ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int64Array,
    PrimitiveArray, RecordBatch, StringArray, TimestampMillisecondArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::{column_index, replace_keeping_field_metadata, replace_or_append, scalar_as_string};
use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::{PlenoraError, Result};

/// Metodo di riempimento di `table.fill_na`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillMethod {
    /// Valore fisso `value` (`"value"`).
    Value,
    /// Ultimo valore non nullo precedente, nell'ordine delle righe
    /// (`"ffill"`); i null iniziali restano null.
    Ffill,
    /// Primo valore non nullo seguente (`"bfill"`); i null finali restano
    /// null.
    Bfill,
}

const fn default_fill_method() -> FillMethod {
    FillMethod::Value
}

/// Config di `table.fill_na`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillNa {
    /// Colonna da riempire; assente o `null`: tutte le colonne, che devono
    /// essere tutte `Utf8`, `Int64`, `Float64` o `Boolean`.
    pub column: Option<String>,
    /// Metodo di riempimento (default `value`).
    #[serde(default = "default_fill_method")]
    pub method: FillMethod,
    /// Valore di riempimento di `method = value`. Assente: nessun valore
    /// (null, quindi nessun cambiamento). Scritto, anche `null`, e' `Some`:
    /// con `ffill`/`bfill` non avrebbe effetto e si rifiuta
    /// ([`FillNa::verifica_parametri`]). Si converte nel tipo della colonna:
    /// per `Utf8` una stringa com'e' e ogni altro valore come testo JSON; per
    /// `Int64` un intero JSON o una stringa intera; per `Float64` un numero
    /// JSON o una stringa con la virgola decimale ammessa; per `Boolean` un
    /// booleano JSON o `"true"`/`"false"` senza distinzione di maiuscole.
    #[serde(default, deserialize_with = "valore_scritto")]
    pub value: Option<Value>,
}

/// `Some` per ogni valore scritto, `null` compreso: serde renderebbe `None`
/// anche un `null` esplicito, e un parametro scritto non sarebbe piu'
/// distinguibile da uno assente.
pub(crate) fn valore_scritto<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl FillNa {
    /// Il valore di riempimento (null se assente).
    #[must_use]
    pub fn valore(&self) -> &Value {
        self.value.as_ref().unwrap_or(&Value::Null)
    }

    /// `value` vale solo per `method = value`: con `ffill` e `bfill` si
    /// rifiuta invece di essere ignorato. La chiamano il kernel e l'analisi
    /// dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` se `value` accompagna `ffill` o `bfill`.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.value.is_some() && !matches!(self.method, FillMethod::Value) {
            return Err(PlenoraError::InvalidPlan(
                "value ammesso solo con method=value".into(),
            ));
        }
        Ok(())
    }
}

/// Config di `table.replace`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replace {
    /// Colonna `Utf8` da modificare (obbligatorio).
    pub column: String,
    /// Senza `regex`, la cella intera da sostituire; con `regex`, il pattern
    /// (sintassi del crate `regex`). Obbligatorio.
    pub old_value: String,
    /// Testo sostitutivo (obbligatorio); con `regex` riconosce `$1`, `${1}`,
    /// `$nome`, `${nome}` e `$$`.
    pub new_value: String,
    /// Interpreta `old_value` come regex e sostituisce ogni match (default
    /// `false`: confronto della cella intera).
    #[serde(default)]
    pub regex: bool,
}

/// Tipo d'arrivo di `table.type_cast`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    /// `Utf8`, il testo della cella (`"str"`).
    Str,
    /// `Int64` da un intero decimale (`"int"`).
    Int,
    /// `Float64` dal parse `f64`, virgola decimale ammessa (`"float"`).
    Float,
    /// `Boolean` dai token vero/falso (`"bool"`).
    Bool,
    /// `Utf8` `AAAA-MM-GG` da una data (`"date"`).
    Date,
    /// `Utf8` `AAAA-MM-GGTHH:MM:SS` da una data e ora (`"datetime"`).
    Datetime,
    /// `Date32` da una data (`"date32"`).
    Date32,
    /// `Timestamp(Millisecond, timezone)` (`"timestamp_millis"`).
    TimestampMillis,
    /// `Decimal128(precision, scale)`, senza arrotondamento
    /// (`"decimal128"`).
    Decimal128,
    /// `Binary`, i byte UTF-8 del testo (`"binary_utf8"`).
    BinaryUtf8,
    /// `UInt64` da un intero decimale non negativo (`"uint64"`).
    Uint64,
    /// `Dictionary(Int32, Utf8)` (`"dictionary_utf8"`).
    DictionaryUtf8,
}

/// Politica di `table.type_cast` per le celle non convertibili.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CastErrors {
    /// Come `Raise`: nessuna cella diventa null; una cella non convertibile
    /// fa fallire il passo con la diagnostica per riga (`"coerce"`).
    Coerce,
    /// Una cella non convertibile fa fallire il passo con la diagnostica per
    /// riga (`"raise"`).
    Raise,
    /// Nessun controllo preventivo: una cella non convertibile fa fallire il
    /// passo con `InvalidPlan`, senza diagnostica per riga (`"ignore"`).
    Ignore,
}

/// Config di `table.type_cast`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeCast {
    /// Colonna da convertire, leggibile come testo (obbligatorio).
    pub column: String,
    /// Tipo d'arrivo (default `str`).
    #[serde(default = "default_target")]
    pub target_type: TargetType,
    /// Formato strftime delle date (default vuoto: formati di default);
    /// l'analisi lo ammette solo con `date`, `datetime`, `date32`,
    /// `timestamp_millis`.
    #[serde(default)]
    pub date_format: String,
    /// Politica per le celle non convertibili (default `coerce`,
    /// [`TypeCast::errori`]). Con `str`, `binary_utf8` e `dictionary_utf8`
    /// nessuna cella fallisce la conversione, quindi scritta non avrebbe
    /// effetto e si rifiuta ([`TypeCast::verifica_parametri`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub errors: Option<CastErrors>,
    /// Cifre totali di `decimal128` (obbligatorio e ammesso solo li',
    /// da 1 a 38).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub precision: Option<u8>,
    /// Cifre decimali di `decimal128` (obbligatorio e ammesso solo li',
    /// da 0 a `precision`).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub scale: Option<i8>,
    /// Fuso IANA di `timestamp_millis` (ammesso solo li'): fuso dei testi
    /// senza fuso e della colonna d'uscita.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub timezone: Option<String>,
}

const fn default_target() -> TargetType {
    TargetType::Str
}

impl TypeCast {
    /// La politica sulle celle non convertibili: `errors`, o `coerce` se
    /// assente.
    #[must_use]
    pub fn errori(&self) -> CastErrors {
        self.errors.unwrap_or(CastErrors::Coerce)
    }

    /// Parametri che il target non usa, rifiutati invece di essere ignorati;
    /// parametri obbligatori di `decimal128`. La chiamano il kernel e
    /// l'analisi dei contratti.
    ///
    /// - `date_format` solo con `date`, `datetime`, `date32`,
    ///   `timestamp_millis`;
    /// - `precision` e `scale` obbligatori con `decimal128` (1 <= precision
    ///   <= 38, 0 <= scale <= precision) e ammessi solo li';
    /// - `timezone` solo con `timestamp_millis`;
    /// - `errors` non con `str`, `binary_utf8` e `dictionary_utf8`, dove
    ///   nessuna cella fallisce.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per ciascuna delle regole.
    pub fn verifica_parametri(&self) -> Result<()> {
        let piano = |messaggio: &str| Err(PlenoraError::InvalidPlan(messaggio.to_owned()));
        let usa_il_formato = matches!(
            self.target_type,
            TargetType::Date
                | TargetType::Datetime
                | TargetType::Date32
                | TargetType::TimestampMillis
        );
        if !self.date_format.is_empty() && !usa_il_formato {
            return piano("date_format ammesso solo per i target data e timestamp");
        }
        if self.errors.is_some()
            && matches!(
                self.target_type,
                TargetType::Str | TargetType::BinaryUtf8 | TargetType::DictionaryUtf8
            )
        {
            return piano("errors non ha effetto: con questo target nessuna cella fallisce");
        }
        match self.target_type {
            TargetType::Decimal128 => {
                let (Some(precision), Some(scale)) = (self.precision, self.scale) else {
                    return piano("decimal128 richiede precision e scale");
                };
                if !(1..=38).contains(&precision) || scale < 0 || scale > precision.cast_signed() {
                    return piano(
                        "decimal128 richiede 1 <= precision <= 38 e 0 <= scale <= precision",
                    );
                }
                if self.timezone.is_some() {
                    return piano("timezone non ammessa per decimal128");
                }
            }
            TargetType::TimestampMillis => {
                if self.precision.is_some() || self.scale.is_some() {
                    return piano("precision e scale non ammessi per timestamp");
                }
            }
            _ if self.precision.is_some() || self.scale.is_some() || self.timezone.is_some() => {
                return piano("precision, scale e timezone non ammessi per questo target_type");
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fast path tipizzati di `fill_na`: lavorano sui valori nativi Arrow e
// copiano l'`Arc` quando l'operazione e' l'identita'. Semantica identica al
// generico: stessi errori, stessi tipi rifiutati (UInt64 incluso), null in
// testa (ffill) o in coda (bfill) che restano null.
// ---------------------------------------------------------------------------

fn utf8_data_len(values: &StringArray) -> usize {
    let offsets = values.offsets();
    usize::try_from(offsets[values.len()] - offsets[0]).unwrap_or(0)
}

fn fill_utf8(values: &StringArray, method: &FillMethod, value: &Value) -> ArrayRef {
    let fixed = match value {
        Value::Null => None,
        Value::String(v) => Some(v.clone()),
        other => Some(other.to_string()),
    };
    if values.null_count() == 0 || (matches!(method, FillMethod::Value) && fixed.is_none()) {
        return Arc::new(values.clone());
    }
    match method {
        FillMethod::Value => {
            // La guardia sopra esce gia' per `Value` con fill `null`
            // (identita'): qui `fixed` e' sempre `Some`, come impone il tipo.
            let Some(fixed) = fixed else {
                return Arc::new(values.clone());
            };
            let capacity = utf8_data_len(values) + fixed.len() * values.null_count();
            let mut builder = StringBuilder::with_capacity(values.len(), capacity);
            for row in 0..values.len() {
                if values.is_null(row) {
                    builder.append_value(&fixed);
                } else {
                    builder.append_value(values.value(row));
                }
            }
            Arc::new(builder.finish())
        }
        FillMethod::Ffill => {
            let mut builder = StringBuilder::with_capacity(values.len(), utf8_data_len(values));
            let mut previous: Option<&str> = None;
            for row in 0..values.len() {
                if values.is_null(row) {
                    match previous {
                        Some(value) => builder.append_value(value),
                        None => builder.append_null(),
                    }
                } else {
                    let value = values.value(row);
                    builder.append_value(value);
                    previous = Some(value);
                }
            }
            Arc::new(builder.finish())
        }
        FillMethod::Bfill => {
            let mut out: Vec<Option<&str>> = vec![None; values.len()];
            let mut following = None;
            for row in (0..values.len()).rev() {
                if values.is_null(row) {
                    out[row] = following;
                } else {
                    let value = values.value(row);
                    out[row] = Some(value);
                    following = Some(value);
                }
            }
            Arc::new(StringArray::from(out))
        }
    }
}

fn fill_primitive<T>(
    values: &PrimitiveArray<T>,
    method: &FillMethod,
    fixed: Option<T::Native>,
) -> ArrayRef
where
    T: ArrowPrimitiveType,
{
    if values.null_count() == 0 || (matches!(method, FillMethod::Value) && fixed.is_none()) {
        return Arc::new(values.clone());
    }
    match method {
        FillMethod::Value => {
            // La guardia sopra esce gia' per `Value` con fill `null`
            // (identita'): qui `fixed` e' sempre `Some`, come impone il tipo.
            let Some(fixed) = fixed else {
                return Arc::new(values.clone());
            };
            let mut buffer = values.values().to_vec();
            for (row, slot) in buffer.iter_mut().enumerate() {
                if values.is_null(row) {
                    *slot = fixed;
                }
            }
            Arc::new(PrimitiveArray::<T>::new(buffer.into(), None))
        }
        FillMethod::Ffill => {
            let mut builder = PrimitiveBuilder::<T>::with_capacity(values.len());
            let mut previous = None;
            for row in 0..values.len() {
                if values.is_null(row) {
                    match previous {
                        Some(value) => builder.append_value(value),
                        None => builder.append_null(),
                    }
                } else {
                    let value = values.value(row);
                    builder.append_value(value);
                    previous = Some(value);
                }
            }
            Arc::new(builder.finish())
        }
        FillMethod::Bfill => {
            let mut out = vec![None; values.len()];
            let mut following = None;
            for row in (0..values.len()).rev() {
                if values.is_null(row) {
                    out[row] = following;
                } else {
                    let value = values.value(row);
                    out[row] = Some(value);
                    following = Some(value);
                }
            }
            Arc::new(out.into_iter().collect::<PrimitiveArray<T>>())
        }
    }
}

fn fill_boolean(values: &BooleanArray, method: &FillMethod, fixed: Option<bool>) -> ArrayRef {
    if values.null_count() == 0 || (matches!(method, FillMethod::Value) && fixed.is_none()) {
        return Arc::new(values.clone());
    }
    match method {
        FillMethod::Value => {
            // La guardia sopra esce gia' per `Value` con fill `null`
            // (identita'): qui `fixed` e' sempre `Some`, come impone il tipo.
            let Some(fixed) = fixed else {
                return Arc::new(values.clone());
            };
            let out: Vec<bool> = (0..values.len())
                .map(|row| {
                    if values.is_null(row) {
                        fixed
                    } else {
                        values.value(row)
                    }
                })
                .collect();
            Arc::new(BooleanArray::from(out))
        }
        FillMethod::Ffill => {
            let mut out = Vec::with_capacity(values.len());
            let mut previous = None;
            for row in 0..values.len() {
                if values.is_null(row) {
                    out.push(previous);
                } else {
                    let value = values.value(row);
                    out.push(Some(value));
                    previous = Some(value);
                }
            }
            Arc::new(BooleanArray::from(out))
        }
        FillMethod::Bfill => {
            let mut out = vec![None; values.len()];
            let mut following = None;
            for row in (0..values.len()).rev() {
                if values.is_null(row) {
                    out[row] = following;
                } else {
                    let value = values.value(row);
                    out[row] = Some(value);
                    following = Some(value);
                }
            }
            Arc::new(BooleanArray::from(out))
        }
    }
}

fn fill_array(array: &dyn Array, method: &FillMethod, value: &Value) -> Result<ArrayRef> {
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(fill_utf8(values, method, value));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        let fixed = match value {
            Value::Null => None,
            // Un numero non intero o fuori da `i64` e' un errore: come
            // assente (`as_i64` a `None`) la colonna restava com'era, senza
            // riempimento e senza errore.
            Value::Number(n) => Some(
                n.as_i64()
                    .ok_or_else(|| PlenoraError::InvalidPlan("fill int non valido".into()))?,
            ),
            Value::String(s) => Some(
                s.parse()
                    .map_err(|_| PlenoraError::InvalidPlan("fill int non valido".into()))?,
            ),
            _ => return Err(PlenoraError::InvalidPlan("fill int non valido".into())),
        };
        return Ok(fill_primitive(values, method, fixed));
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        let fixed = match value {
            Value::Null => None,
            Value::Number(n) => n.as_f64(),
            Value::String(s) => Some(
                s.replace(',', ".")
                    .parse()
                    .map_err(|_| PlenoraError::InvalidPlan("fill float non valido".into()))?,
            ),
            _ => return Err(PlenoraError::InvalidPlan("fill float non valido".into())),
        };
        return Ok(fill_primitive(values, method, fixed));
    }
    if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
        let fixed = match value {
            Value::Null => None,
            Value::Bool(v) => Some(*v),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
            _ => return Err(PlenoraError::InvalidPlan("fill bool non valido".into())),
        };
        return Ok(fill_boolean(values, method, fixed));
    }
    Err(PlenoraError::Schema(format!(
        "fill_na non supporta {}",
        plenora_core::tipo_arrow::descrivi_tipo(array.data_type())
    )))
}

/// Sostituisce i null (valore fisso, ffill o bfill) nella colonna indicata
/// o in tutte, se `column` e' assente (`table.fill_na`).
///
/// Tipo e metadati di campo restano; le colonne trattate diventano
/// nullable. Si riempie solo il null (un `NaN` resta).
///
/// Un `value` numerico non intero o fuori da `i64` su una colonna `Int64` si
/// rifiuta, anche chiamato senza l'analisi.
///
/// # Errors
///
/// - `Schema`: colonna assente; tipo di colonna non supportato (coperti
///   `Utf8`, `Int64`, `Float64`, `Boolean`);
/// - `InvalidPlan`: valore di riempimento non convertibile nel tipo della
///   colonna; `value` con `ffill`/`bfill`;
/// - `DataMapping`: errore Arrow nella sostituzione (guardia interna, non
///   attesa).
pub fn fill_na(batch: &RecordBatch, config: &FillNa) -> Result<RecordBatch> {
    config.verifica_parametri()?;
    let targets: Vec<usize> = if let Some(name) = &config.column {
        vec![column_index(batch, name)?]
    } else {
        (0..batch.num_columns()).collect()
    };
    let mut out = batch.clone();
    for index in targets {
        let name = out.schema().field(index).name().clone();
        let array = fill_array(out.column(index).as_ref(), &config.method, config.valore())?;
        out = replace_keeping_field_metadata(&out, &name, array.data_type().clone(), true, array)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Fast path per `table.coalesce`: quality.rs chiama `coalesce_fast` e ricade
// sul generico concat+take quando torna `None`. Per ogni riga vince il primo
// valore non nullo nell'ordine delle colonne, come nel generico.
// ---------------------------------------------------------------------------

fn coalesce_primitive<T>(batch: &RecordBatch, indices: &[usize]) -> Option<ArrayRef>
where
    T: ArrowPrimitiveType,
{
    let columns = indices
        .iter()
        .map(|index| {
            batch
                .column(*index)
                .as_any()
                .downcast_ref::<PrimitiveArray<T>>()
        })
        .collect::<Option<Vec<_>>>()?;
    let mut builder = PrimitiveBuilder::<T>::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        match columns.iter().find(|column| !column.is_null(row)) {
            Some(column) => builder.append_value(column.value(row)),
            None => builder.append_null(),
        }
    }
    Some(Arc::new(builder.finish()))
}

/// Fast path di `table.coalesce`: per ogni riga, il primo valore non nullo.
///
/// Le colonne `indices` (non vuoto) si guardano nell'ordine dato. `None` se
/// il tipo non ha un ramo tipizzato (`Dictionary` compresa, per il null
/// logico) e il chiamante deve usare il percorso generico.
#[must_use]
pub fn coalesce_fast(batch: &RecordBatch, indices: &[usize]) -> Option<ArrayRef> {
    let first = batch.column(indices[0]);
    // La scorciatoia «nessun null, restituisco la prima colonna» guarda i
    // null LOGICI: `null_count()` conta solo la bitmap di primo livello, e
    // non vede i valori nulli di una dictionary (celle nulle rese come
    // valori). Run-end e union non arrivano qui: il runner li rifiuta al
    // confine. Gli altri tipi senza ramo qui sotto ricadono sul percorso
    // generico, che usa `is_logically_null`.
    if first.logical_null_count() == 0 {
        return Some(first.clone());
    }
    match first.data_type() {
        DataType::Int64 => coalesce_primitive::<Int64Type>(batch, indices),
        DataType::Float64 => coalesce_primitive::<Float64Type>(batch, indices),
        DataType::UInt64 => coalesce_primitive::<UInt64Type>(batch, indices),
        DataType::Boolean => {
            let columns = indices
                .iter()
                .map(|index| batch.column(*index).as_any().downcast_ref::<BooleanArray>())
                .collect::<Option<Vec<_>>>()?;
            let mut builder = BooleanBuilder::with_capacity(batch.num_rows());
            for row in 0..batch.num_rows() {
                match columns.iter().find(|column| !column.is_null(row)) {
                    Some(column) => builder.append_value(column.value(row)),
                    None => builder.append_null(),
                }
            }
            Some(Arc::new(builder.finish()))
        }
        DataType::Utf8 => {
            let columns = indices
                .iter()
                .map(|index| batch.column(*index).as_any().downcast_ref::<StringArray>())
                .collect::<Option<Vec<_>>>()?;
            let capacity = columns.first().map_or(0, |column| utf8_data_len(column));
            let mut builder = StringBuilder::with_capacity(batch.num_rows(), capacity);
            for row in 0..batch.num_rows() {
                match columns.iter().find(|column| !column.is_null(row)) {
                    Some(column) => builder.append_value(column.value(row)),
                    None => builder.append_null(),
                }
            }
            Some(Arc::new(builder.finish()))
        }
        _ => None,
    }
}

/// Sostituisce `old_value` con `new_value` in una colonna `Utf8`
/// (`table.replace`).
///
/// Senza `regex` il confronto e' sulla cella intera (nessuna sostituzione di
/// sottostringhe); con `regex` ogni match si sostituisce con `new_value`,
/// che riconosce i riferimenti ai gruppi (`$1`, `${nome}`, `$$`). Il null
/// resta null; la colonna resta nella sua posizione con i metadati di campo.
/// Con `Limits::default()`: [`replace_con_limiti`] con i limiti del
/// chiamante.
///
/// # Errors
///
/// Come [`replace_con_limiti`].
pub fn replace(batch: &RecordBatch, config: &Replace) -> Result<RecordBatch> {
    replace_con_limiti(batch, config, &crate::Limits::default())
}

/// [`replace`] con i limiti del chiamante (il runner passa i suoi).
///
/// Con `regex` il pattern non supera `limits.max_regex_bytes`, e ogni cella
/// sostituita non supera `limits.max_string_bytes`. Una regex che accetta la
/// stringa vuota inserisce `new_value` a ogni posizione: la cella cresce con
/// i dati, e il controllo e' sul risultato di ogni cella.
///
/// # Errors
///
/// - `Schema`: colonna assente o non `Utf8`;
/// - `InvalidPlan`: `old_value` non e' una regex valida o supera
///   `max_regex_bytes` (con `regex = true`);
/// - `ResourceLimit`: una cella sostituita oltre `max_string_bytes`;
/// - `DataMapping`: errore Arrow nella sostituzione (guardia interna, non
///   attesa).
pub fn replace_con_limiti(
    batch: &RecordBatch,
    config: &Replace,
    limits: &crate::Limits,
) -> Result<RecordBatch> {
    if config.regex && config.old_value.len() > limits.max_regex_bytes {
        return Err(PlenoraError::InvalidPlan(
            "replace: pattern oltre max_regex_bytes".into(),
        ));
    }
    let index = column_index(batch, &config.column)?;
    let values = batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PlenoraError::Schema("replace safe profile richiede Utf8".into()))?;
    let regex = config
        .regex
        .then(|| Regex::new(&config.old_value))
        .transpose()
        .map_err(|e| PlenoraError::InvalidPlan(crate::motivo_regex_non_valida(&e).into()))?;
    let out: StringArray = values
        .iter()
        .map(|item| {
            item.map(|text| {
                regex.as_ref().map_or_else(
                    || {
                        Ok(if text == config.old_value {
                            config.new_value.clone()
                        } else {
                            text.to_owned()
                        })
                    },
                    |pattern| {
                        let sostituito = pattern
                            .replace_all(text, config.new_value.as_str())
                            .into_owned();
                        crate::verifica_testo_prodotto("replace", sostituito.len(), limits)?;
                        Ok(sostituito)
                    },
                )
            })
            .transpose()
        })
        .collect::<Result<StringArray>>()?;
    replace_keeping_field_metadata(batch, &config.column, DataType::Utf8, true, Arc::new(out))
}

fn cast_failure<T>(errors: CastErrors, message: &str) -> Result<Option<T>> {
    match errors {
        CastErrors::Coerce | CastErrors::Raise => Err(PlenoraError::Internal(format!(
            "prevalidazione row-scoped incoerente: {message}"
        ))),
        CastErrors::Ignore => Err(PlenoraError::InvalidPlan(
            "errors=ignore non puo' garantire un tipo Arrow omogeneo; usare coerce o raise".into(),
        )),
    }
}

/// Il testo di una cella letto come data o data e ora: con `format` vuoto i
/// soli formati ISO 8601 (`crate::temporale::leggi_iso`: nessun ordine
/// giorno/mese da indovinare, offset RFC 3339 ammesso), altrimenti quel
/// formato, che con `serve_ora` deve leggere anche l'ora.
fn leggi_testo_temporale(
    value: &str,
    format: &str,
    serve_ora: bool,
) -> Option<crate::temporale::Momento> {
    if format.is_empty() {
        return crate::temporale::leggi_iso(value);
    }
    let items = crate::dates::compile_items(format);
    let (momento, ha_ora) = crate::temporale::leggi_con_items_e_ora(value, &items)?;
    // Un formato di sola data non legge un'ora: per `datetime` e
    // `timestamp_millis` resta un rifiuto. La lettura e' una sola, con
    // l'offset letto (una seconda ricostruzione in UTC rifiutava un `%s`
    // coerente con i campi e l'offset).
    (!serve_ora || ha_ora).then_some(momento)
}

/// `date` (`AAAA-MM-GG`) o `datetime` (`AAAA-MM-GGTHH:MM:SS`, con la
/// frazione di secondo quando c'e') dal testo: l'ora locale scritta.
fn parse_date(value: &str, format: &str, datetime: bool) -> Option<String> {
    let momento = leggi_testo_temporale(value, format, datetime)?;
    Some(if datetime {
        crate::temporale::testo_datetime(&momento.locale)
    } else {
        momento.locale.format("%Y-%m-%d").to_string()
    })
}

/// Giorni dall'epoca della data locale di un momento.
fn giorni_dall_epoca(locale: &NaiveDateTime) -> Option<i32> {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    i32::try_from(locale.date().signed_duration_since(epoch).num_days()).ok()
}

fn parse_date32(value: &str, format: &str) -> Option<i32> {
    giorni_dall_epoca(&leggi_testo_temporale(value, format, false)?.locale)
}

/// Millisecondi di un momento: l'istante se c'e' (un offset nel testo, un
/// `Timestamp`), altrimenti l'ora locale nel fuso `timezone` (un'ora
/// ambigua o inesistente nel cambio d'ora non ha un istante) o in UTC.
///
/// `None` anche se l'istante ha una parte sotto il millisecondo: il
/// millisecondo non la tiene, e troncarla sarebbe un valore diverso da
/// quello letto.
fn millisecondi_di(momento: &crate::temporale::Momento, timezone: Option<&str>) -> Option<i64> {
    let istante = match momento.istante {
        Some(istante) => istante,
        None => match timezone {
            Some(name) => {
                let zone = name.parse::<chrono_tz::Tz>().ok()?;
                match zone.from_local_datetime(&momento.locale) {
                    LocalResult::Single(value) => value.with_timezone(&Utc),
                    LocalResult::Ambiguous(_, _) | LocalResult::None => return None,
                }
            }
            None => Utc.from_utc_datetime(&momento.locale),
        },
    };
    (istante.timestamp_subsec_nanos() % 1_000_000 == 0).then(|| istante.timestamp_millis())
}

fn parse_timestamp_millis(value: &str, format: &str, timezone: Option<&str>) -> Option<i64> {
    millisecondi_di(&leggi_testo_temporale(value, format, true)?, timezone)
}

fn parse_decimal128(value: &str, precision: u8, scale: i8) -> Option<i128> {
    let scale = u32::try_from(scale).ok()?;
    let value = value.trim();
    // Un segno solo: `"-+5"` e `"+-5"` si rifiutano (il resto non e' di
    // sole cifre), come nei `parse` della libreria standard.
    let (negative, unsigned) = crate::separa_segno(value);
    let mut pieces = unsigned.split('.');
    let whole = pieces.next()?;
    let fraction = pieces.next().unwrap_or("");
    if pieces.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > usize::try_from(scale).ok()?
    {
        return None;
    }
    let factor = 10_i128.checked_pow(scale)?;
    let whole = whole.parse::<i128>().ok()?.checked_mul(factor)?;
    let missing = scale.checked_sub(u32::try_from(fraction.len()).ok()?)?;
    let fractional = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i128>()
            .ok()?
            .checked_mul(10_i128.checked_pow(missing)?)?
    };
    let magnitude = whole.checked_add(fractional)?;
    let signed = if negative {
        magnitude.checked_neg()?
    } else {
        magnitude
    };
    let digits = signed
        .unsigned_abs()
        .to_string()
        .trim_start_matches('0')
        .len()
        .max(1);
    (digits <= usize::from(precision)).then_some(signed)
}

fn source_strings(source: &ArrayRef) -> Result<Vec<Option<String>>> {
    (0..source.len())
        .map(|row| scalar_as_string(source.as_ref(), row))
        .collect()
}

// ---------------------------------------------------------------------------
// Fast path per `table.type_cast`: downcast una volta sola e cast numerici
// sui valori nativi. Le combinazioni non coperte ricadono su
// `type_cast_generic`, che e' anche l'oracolo dei test di equivalenza; la
// semantica e' byte-identica. I cast da f64 riproducono
// `to_string().trim().parse()` (il `Display` di f64 non usa notazione
// esponenziale).
// ---------------------------------------------------------------------------

/// `to_string(value).trim().parse::<i64>()` del generico su valori nativi.
///
/// Riesce per i finiti interi. Sotto 2^53 il Display e' esatto e il parse e'
/// sempre in range; sopra 2^53 il Display stampa la rappresentazione decimale
/// piu' corta (es. -2^63 -> "-9223372036854776000", che NON parsa come i64),
/// quindi si riproduce il parse testuale. "-0" (da -0.0) parse a 0.
fn cast_f64_i64(value: f64) -> Option<i64> {
    const EXACT: f64 = 9_007_199_254_740_992.0; // 2^53
                                                // Confronto esatto voluto: riproduce il parse testuale del generico
                                                // (finito e intero per bit), come documentato sopra la funzione.
    #[allow(clippy::float_cmp)]
    if !value.is_finite() || value != value.trunc() {
        return None;
    }
    if value.abs() < EXACT {
        return value.to_i64();
    }
    value.to_string().parse::<i64>().ok()
}

/// `to_string(value).trim().parse::<u64>()` del generico: come sopra, con il
/// segno: i negativi falliscono sempre, -0.0 fallisce ("-0").
fn cast_f64_u64(value: f64) -> Option<u64> {
    const EXACT: f64 = 9_007_199_254_740_992.0; // 2^53
                                                // Come `cast_f64_i64`: confronti esatti voluti, riproducono il parse
                                                // testuale del generico incluso il rifiuto di "-0".
    #[allow(clippy::float_cmp)]
    if !value.is_finite()
        || value != value.trunc()
        || value < 0.0
        || (value == 0.0 && value.is_sign_negative())
    {
        return None;
    }
    if value < EXACT {
        return value.to_u64();
    }
    value.to_string().parse::<u64>().ok()
}

fn cast_or_failure<T>(parsed: Option<T>, errors: CastErrors, message: &str) -> Result<Option<T>> {
    parsed.map_or_else(|| cast_failure(errors, message), |value| Ok(Some(value)))
}

/// `parse_date32` veloce per il formato ISO canonico "YYYY-MM-DD".
///
/// Il formato ISO e' il primo provato da `parse_date` a formato vuoto: il
/// parse manuale usa la stessa validazione di `NaiveDate` (`from_ymd_opt`),
/// quindi produce lo stesso risultato senza doppio parse chrono; ogni altra
/// stringa ricade su `parse_date32`.
fn parse_date32_fast(value: &str, format: &str) -> Option<i32> {
    if format.is_empty() {
        let bytes = value.as_bytes();
        if bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[..4]
                .iter()
                .chain(&bytes[5..7])
                .chain(&bytes[8..10])
                .all(u8::is_ascii_digit)
        {
            let year = value[..4].parse::<i32>().ok()?;
            let month = value[5..7].parse::<u32>().ok()?;
            let day = value[8..10].parse::<u32>().ok()?;
            let date = NaiveDate::from_ymd_opt(year, month, day)?;
            let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
            return i32::try_from(date.signed_duration_since(epoch).num_days()).ok();
        }
    }
    parse_date32(value, format)
}

/// Target `str`: i null sorgente restano null.
fn cast_to_str(source: &ArrayRef) -> Option<ArrayRef> {
    let len = source.len();
    let mut builder = StringBuilder::with_capacity(len, len * 8);
    if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_null();
            } else {
                builder.append_value(values.value(row));
            }
        }
    } else if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_null();
            } else {
                builder.append_value(values.value(row).to_string());
            }
        }
    } else if let Some(values) = source.as_any().downcast_ref::<UInt64Array>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_null();
            } else {
                builder.append_value(values.value(row).to_string());
            }
        }
    } else if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        for row in 0..len {
            if values.is_null(row) {
                builder.append_null();
            } else {
                builder.append_value(values.value(row).to_string());
            }
        }
    } else {
        // Ultimo tipo della catena: un downcast fallito significa tipo non
        // gestito, quindi `None` via `?`.
        let values = source.as_any().downcast_ref::<BooleanArray>()?;
        for row in 0..len {
            if values.is_null(row) {
                builder.append_null();
            } else {
                builder.append_value(values.value(row).to_string());
            }
        }
    }
    Some(Arc::new(builder.finish()))
}

/// Target `int` (Int64).
fn cast_to_int(source: &ArrayRef, errors: CastErrors) -> Result<Option<ArrayRef>> {
    const MESSAGE: &str = "conversione int fallita";
    if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        // to_string + parse e' sempre l'identita' su Int64.
        return Ok(Some(Arc::new(values.clone())));
    }
    let len = source.len();
    let out: Vec<Option<i64>> = if let Some(values) = source.as_any().downcast_ref::<UInt64Array>()
    {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_or_failure(i64::try_from(values.value(row)).ok(), errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_or_failure(cast_f64_i64(values.value(row)), errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        values
            .iter()
            .map(|value| {
                value.map_or(Ok(None), |value| {
                    cast_or_failure(value.trim().parse::<i64>().ok(), errors, MESSAGE)
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        // "true"/"false" non parsano come i64: ogni riga non nulla fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_failure(errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else {
        return Ok(None);
    };
    Ok(Some(Arc::new(Int64Array::from(out))))
}

/// Target `float` (Float64).
///
/// # Arrotondamento dichiarato
///
/// `target_type = "float"` chiede un `Float64`: l'arrotondamento al double
/// piu' vicino e' la sua semantica, e un `i64`/`u64` oltre 2^53 perde le
/// cifre basse (limite dichiarato nella scheda di `table.type_cast`). Dove
/// il double partecipa a una decisione vale `exact_f64_from_*`.
#[allow(clippy::cast_precision_loss)] // Arrotondamento voluto: e' la semantica di `cast(to: "float")`.
fn cast_to_float(source: &ArrayRef, errors: CastErrors) -> Result<Option<ArrayRef>> {
    const MESSAGE: &str = "conversione float fallita";
    if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        return Ok(Some(Arc::new(values.clone())));
    }
    let len = source.len();
    let out: Vec<Option<f64>> = if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        (0..len)
            .map(|row| {
                Ok(if values.is_null(row) {
                    None
                } else {
                    Some(values.value(row) as f64)
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<UInt64Array>() {
        (0..len)
            .map(|row| {
                Ok(if values.is_null(row) {
                    None
                } else {
                    Some(values.value(row) as f64)
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        values
            .iter()
            .map(|value| {
                value.map_or(Ok(None), |value| {
                    let trimmed = value.trim();
                    let parsed = if trimmed.contains(',') {
                        trimmed.replace(',', ".").parse::<f64>()
                    } else {
                        trimmed.parse::<f64>()
                    };
                    cast_or_failure(parsed.ok(), errors, MESSAGE)
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        // "true"/"false" non parsano come f64: ogni riga non nulla fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_failure(errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else {
        return Ok(None);
    };
    Ok(Some(Arc::new(Float64Array::from(out))))
}

/// Target `bool` (Boolean).
fn cast_to_bool(source: &ArrayRef, errors: CastErrors) -> Result<Option<ArrayRef>> {
    const MESSAGE: &str = "conversione bool fallita";
    if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        return Ok(Some(Arc::new(values.clone())));
    }
    let len = source.len();
    let out: Vec<Option<bool>> = if let Some(values) = source.as_any().downcast_ref::<Int64Array>()
    {
        // Il generico parsa la stringa: "1" -> true, "0" -> false, altro fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    match values.value(row) {
                        1 => Ok(Some(true)),
                        0 => Ok(Some(false)),
                        _ => cast_failure(errors, MESSAGE),
                    }
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<UInt64Array>() {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    match values.value(row) {
                        1 => Ok(Some(true)),
                        0 => Ok(Some(false)),
                        _ => cast_failure(errors, MESSAGE),
                    }
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        // to_string: "1" -> true, "0" -> false; "-0" (da -0.0) fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    let value = values.value(row);
                    // Uguaglianze esatte volute: riproducono il parse di
                    // "1"/"0" ("-0" fallisce), vedi commento sopra il ciclo.
                    #[allow(clippy::float_cmp)]
                    if value == 1.0 {
                        Ok(Some(true))
                    } else if value == 0.0 && value.is_sign_positive() {
                        Ok(Some(false))
                    } else {
                        cast_failure(errors, MESSAGE)
                    }
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        values
            .iter()
            .map(|value| {
                value.map_or(Ok(None), |value| {
                    let lower = value.trim().to_lowercase();
                    match lower.as_str() {
                        "true" | "1" | "yes" | "si" | "sì" | "vero" | "t" | "y" | "s" => {
                            Ok(Some(true))
                        }
                        "false" | "0" | "no" | "falso" | "f" | "n" => Ok(Some(false)),
                        _ => cast_failure(errors, MESSAGE),
                    }
                })
            })
            .collect::<Result<_>>()?
    } else {
        return Ok(None);
    };
    Ok(Some(Arc::new(BooleanArray::from(out))))
}

/// Target `uint64` (`UInt64`).
fn cast_to_uint64(source: &ArrayRef, errors: CastErrors) -> Result<Option<ArrayRef>> {
    const MESSAGE: &str = "conversione uint64 fallita";
    if let Some(values) = source.as_any().downcast_ref::<UInt64Array>() {
        return Ok(Some(Arc::new(values.clone())));
    }
    let len = source.len();
    let out: Vec<Option<u64>> = if let Some(values) = source.as_any().downcast_ref::<Int64Array>() {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_or_failure(u64::try_from(values.value(row)).ok(), errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<Float64Array>() {
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_or_failure(cast_f64_u64(values.value(row)), errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<StringArray>() {
        values
            .iter()
            .map(|value| {
                value.map_or(Ok(None), |value| {
                    cast_or_failure(value.trim().parse::<u64>().ok(), errors, MESSAGE)
                })
            })
            .collect::<Result<_>>()?
    } else if let Some(values) = source.as_any().downcast_ref::<BooleanArray>() {
        // "true"/"false" non parsano come u64: ogni riga non nulla fallisce.
        (0..len)
            .map(|row| {
                if values.is_null(row) {
                    Ok(None)
                } else {
                    cast_failure(errors, MESSAGE)
                }
            })
            .collect::<Result<_>>()?
    } else {
        return Ok(None);
    };
    Ok(Some(Arc::new(UInt64Array::from(out))))
}

/// Fast path: `Ok(None)` se la combinazione sorgente/target non e' coperta
/// (il chiamante ricade sul generico).
// Dispatcher esaustivo su tutti i TargetType del contratto: un blocco unico
// tiene le politiche di cast revisionabili insieme, come `type_cast_generic`;
// la lunghezza e' la sequenza lineare dei casi (niente refactor in pulizia).
#[allow(clippy::too_many_lines)]
fn type_cast_fast(source: &ArrayRef, config: &TypeCast) -> Result<Option<ArrayRef>> {
    let array: ArrayRef = match config.target_type {
        TargetType::Str => match cast_to_str(source) {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Int => match cast_to_int(source, config.errori())? {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Float => match cast_to_float(source, config.errori())? {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Bool => match cast_to_bool(source, config.errori())? {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Uint64 => match cast_to_uint64(source, config.errori())? {
            Some(array) => array,
            None => return Ok(None),
        },
        TargetType::Date | TargetType::Datetime => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let datetime = matches!(config.target_type, TargetType::Datetime);
            Arc::new(StringArray::from(
                values
                    .iter()
                    .map(|value| {
                        value.map_or(Ok(None), |value| {
                            cast_or_failure(
                                parse_date(value, &config.date_format, datetime),
                                config.errori(),
                                "conversione data fallita",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        TargetType::Date32 => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            Arc::new(Date32Array::from(
                values
                    .iter()
                    .map(|value| {
                        value.map_or(Ok(None), |value| {
                            cast_or_failure(
                                parse_date32_fast(value, &config.date_format),
                                config.errori(),
                                "conversione date32 fallita",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        TargetType::TimestampMillis => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let out = values
                .iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        cast_or_failure(
                            parse_timestamp_millis(
                                value,
                                &config.date_format,
                                config.timezone.as_deref(),
                            ),
                            config.errori(),
                            "conversione timestamp fallita",
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Arc::new(
                TimestampMillisecondArray::from(out).with_timezone_opt(config.timezone.clone()),
            )
        }
        TargetType::Decimal128 => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let precision = config
                .precision
                .ok_or_else(|| PlenoraError::InvalidPlan("decimal128 richiede precision".into()))?;
            let scale = config
                .scale
                .ok_or_else(|| PlenoraError::InvalidPlan("decimal128 richiede scale".into()))?;
            let out = values
                .iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        cast_or_failure(
                            parse_decimal128(value, precision, scale),
                            config.errori(),
                            "conversione decimal128 fallita",
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Arc::new(Decimal128Array::from(out).with_precision_and_scale(precision, scale)?)
        }
        TargetType::BinaryUtf8 => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let mut builder = BinaryBuilder::new();
            for value in values {
                if let Some(value) = value {
                    builder.append_value(value);
                } else {
                    builder.append_null();
                }
            }
            Arc::new(builder.finish())
        }
        TargetType::DictionaryUtf8 => {
            let Some(values) = source.as_any().downcast_ref::<StringArray>() else {
                return Ok(None);
            };
            let mut builder = StringDictionaryBuilder::<Int32Type>::new();
            for value in values {
                if let Some(value) = value {
                    builder.append(value)?;
                } else {
                    builder.append_null();
                }
            }
            Arc::new(builder.finish())
        }
    };
    Ok(Some(array))
}

/// Converte la colonna nel tipo `target_type`, nella stessa posizione
/// (`table.type_cast`).
///
/// Ogni cella non nulla si legge come testo ([`scalar_as_string`]) e il testo
/// si interpreta nel tipo d'arrivo. I token storici `coerce` e `raise` sono
/// entrambi fail-closed: nessun valore non convertibile diventa null, e
/// tutte le righe rifiutate entrano nella diagnostica per riga. La colonna
/// d'uscita e' nullable e perde i metadati di campo.
///
/// # Errors
///
/// - `Schema`: colonna assente; cella che non si legge come testo (tipo
///   fuori dal profilo scalare, `Binary` non UTF-8, data fuori intervallo);
/// - `DataMapping`: almeno una riga non convertibile (con `coerce` o
///   `raise`), con la diagnostica per riga; errore Arrow nella costruzione
///   (precisione e scala `decimal128`, builder del dizionario);
/// - `InvalidPlan`: con `errors = ignore`, una cella non convertibile
///   (nessun tipo Arrow omogeneo garantibile); `decimal128` senza
///   `precision`/`scale`; `date_format` con un elemento non riconosciuto.
pub fn type_cast(batch: &RecordBatch, config: &TypeCast) -> Result<RecordBatch> {
    type_cast_with_source_offset(batch, config, 0)
}

/// Come [`type_cast`], con gli indici di riga della diagnostica spostati di
/// `source_offset` (la posizione del batch nella sorgente).
///
/// # Errors
///
/// Come [`type_cast`], piu' `Internal` se un indice assoluto trabocca `u64`.
pub fn type_cast_with_source_offset(
    batch: &RecordBatch,
    config: &TypeCast,
    source_offset: u64,
) -> Result<RecordBatch> {
    config.verifica_parametri()?;
    // Un formato non riconosciuto non combacia con nessun valore: errore di
    // piano, non righe rifiutate. Vuoto e' il parser di default.
    if !config.date_format.is_empty() {
        crate::dates::validate_format_items(&config.date_format, "date_format")?;
    }
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    // Una colonna temporale (Date32, Timestamp di ogni unita') si converte
    // dal valore nativo, non dal suo testo.
    if let Some(temporale) = crate::temporale::ColonnaTemporale::new(source)? {
        let array = type_cast_temporale(&temporale, source, config, source_offset)?;
        return replace_or_append(
            batch,
            &config.column,
            array.data_type().clone(),
            true,
            array,
        );
    }
    if matches!(config.errori(), CastErrors::Coerce | CastErrors::Raise) {
        if let Some(report) = cast_row_diagnostics(source, &config.column, config, source_offset)? {
            return Err(PlenoraError::DataMapping(
                "conversione rifiutata; consultare row_diagnostics".to_owned(),
            )
            .with_row_diagnostics(report));
        }
    }
    let array = match type_cast_fast(source, config)? {
        Some(array) => array,
        None => type_cast_generic(source, config)?,
    };
    replace_or_append(
        batch,
        &config.column,
        array.data_type().clone(),
        true,
        array,
    )
}

fn cast_row_diagnostics(
    source: &ArrayRef,
    column: &str,
    config: &TypeCast,
    source_offset: u64,
) -> Result<Option<RowDiagnostics>> {
    diagnostica_cast(source.len(), column, source_offset, |row| {
        if source.is_null(row) {
            return Ok(None);
        }
        Ok(scalar_as_string(source.as_ref(), row)?
            .and_then(|value| string_cast_rejection(&value, config)))
    })
}

/// La diagnostica per riga di `type_cast`: la causa di ogni riga rifiutata
/// (`causa`), i conteggi e i primi esempi, con gli indici spostati di
/// `source_offset`. `None` se nessuna riga e' rifiutata.
fn diagnostica_cast(
    righe: usize,
    column: &str,
    source_offset: u64,
    mut causa_di: impl FnMut(usize) -> Result<Option<&'static str>>,
) -> Result<Option<RowDiagnostics>> {
    const EXAMPLES_LIMIT: u64 = 10;
    let mut observed_total = 0_u64;
    let mut counts = BTreeMap::new();
    let mut examples = Vec::new();
    for row in 0..righe {
        let Some(cause) = causa_di(row)? else {
            continue;
        };
        observed_total = observed_total.checked_add(1).ok_or_else(|| {
            PlenoraError::Internal("overflow del conteggio diagnostico".to_owned())
        })?;
        let count = counts.entry(cause.to_owned()).or_insert(0_u64);
        *count = count
            .checked_add(1)
            .ok_or_else(|| PlenoraError::Internal("overflow del conteggio causa".to_owned()))?;
        let local_index = u64::try_from(row).map_err(|_| {
            PlenoraError::Internal("indice sorgente non rappresentabile".to_owned())
        })?;
        let source_index = source_offset
            .checked_add(local_index)
            .ok_or_else(|| PlenoraError::Internal("overflow dell'indice sorgente".to_owned()))?;
        if u64::try_from(examples.len()).map_err(|_| {
            PlenoraError::Internal("conteggio esempi non rappresentabile".to_owned())
        })? < EXAMPLES_LIMIT
        {
            examples.push(RowDiagnosticExample {
                source_index,
                cause: cause.to_owned(),
                column: Some(column.to_owned()),
                key: None,
                write_state: None,
            });
        }
    }
    if observed_total == 0 {
        return Ok(None);
    }
    Ok(Some(RowDiagnostics {
        contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
        scope: RowDiagnosticScope::Read,
        index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
        completeness: RowDiagnosticsCompleteness::Complete,
        knowledge_limits: None,
        total: Some(observed_total),
        observed_total,
        counts,
        examples_limit: EXAMPLES_LIMIT,
        examples_truncated: observed_total > EXAMPLES_LIMIT,
        examples,
        input_total: None,
        diagnostic_state_counts: None,
        write_outcome: None,
    }))
}

/// I target di `type_cast` che una colonna temporale ammette.
///
/// Il testo (`str`, `binary_utf8`, `dictionary_utf8`), le date e le ore
/// (`date`, `datetime`, `date32`, `timestamp_millis`). Un numero o un
/// booleano da un
/// istante non ha un significato scritto: si rifiuta in analisi e nel
/// kernel. Autorita' unica di entrambi.
///
/// # Errors
///
/// `InvalidPlan` per un target numerico o booleano, o per un `date_format`
/// (che legge un testo, e qui non c'e').
pub fn verifica_cast_temporale(config: &TypeCast) -> Result<()> {
    if matches!(
        config.target_type,
        TargetType::Int
            | TargetType::Float
            | TargetType::Bool
            | TargetType::Decimal128
            | TargetType::Uint64
    ) {
        return Err(PlenoraError::InvalidPlan(
            "type_cast: una colonna temporale si converte solo in testo, date o istanti".into(),
        ));
    }
    if !config.date_format.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "type_cast: date_format legge un testo e non si applica a una colonna temporale".into(),
        ));
    }
    Ok(())
}

/// `type_cast` di una colonna temporale dal valore nativo, per ogni unita' e
/// fuso: il testo come il profilo scalare, la data e l'ora **locali** della
/// colonna (del suo fuso; senza fuso, il valore com'e') per `date`,
/// `datetime` e `date32`, l'istante per `timestamp_millis` (una data e' la
/// sua mezzanotte nel fuso `timezone`, o in UTC).
///
/// Un istante con una parte sotto il millisecondo non entra in
/// `timestamp_millis` senza perderla: la riga si rifiuta
/// (`conversion.timestamp_precision`), come un'ora ambigua o inesistente
/// di una data (`conversion.invalid_timestamp`).
// Prevalidazione e costruzione per target: una sequenza lineare, lunga per
// costruzione.
#[allow(clippy::too_many_lines)]
fn type_cast_temporale(
    temporale: &crate::temporale::ColonnaTemporale<'_>,
    source: &ArrayRef,
    config: &TypeCast,
    source_offset: u64,
) -> Result<ArrayRef> {
    verifica_cast_temporale(config)?;
    let timezone = config.timezone.as_deref();
    // Le cause per riga, prima di costruire: stesso contratto del testo.
    let causa = |row: usize| -> Result<Option<&'static str>> {
        let Some(momento) = temporale.momento(row)? else {
            return Ok(None);
        };
        Ok(match config.target_type {
            TargetType::TimestampMillis if millisecondi_di(&momento, timezone).is_none() => {
                Some(if momento.istante.is_some() {
                    "conversion.timestamp_precision"
                } else {
                    "conversion.invalid_timestamp"
                })
            }
            TargetType::Date | TargetType::Date32
                if giorni_dall_epoca(&momento.locale).is_none() =>
            {
                Some("conversion.invalid_date")
            }
            _ => None,
        })
    };
    if let Some(report) = diagnostica_cast(source.len(), &config.column, source_offset, causa)? {
        if matches!(config.errori(), CastErrors::Ignore) {
            return Err(PlenoraError::InvalidPlan(
                "errors=ignore non puo' garantire un tipo Arrow omogeneo; usare coerce o raise"
                    .into(),
            ));
        }
        return Err(PlenoraError::DataMapping(
            "conversione rifiutata; consultare row_diagnostics".to_owned(),
        )
        .with_row_diagnostics(report));
    }
    let momenti = (0..source.len())
        .map(|row| temporale.momento(row))
        .collect::<Result<Vec<_>>>()?;
    let rese_testuali = || -> Result<Vec<Option<String>>> {
        (0..source.len()).map(|row| temporale.testo(row)).collect()
    };
    let interna = || PlenoraError::Internal("prevalidazione temporale incoerente".into());
    Ok(match config.target_type {
        TargetType::Str => Arc::new(StringArray::from(rese_testuali()?)),
        TargetType::BinaryUtf8 => {
            let mut builder = BinaryBuilder::new();
            for testo in rese_testuali()? {
                match testo {
                    Some(testo) => builder.append_value(testo),
                    None => builder.append_null(),
                }
            }
            Arc::new(builder.finish())
        }
        TargetType::DictionaryUtf8 => {
            let mut builder = StringDictionaryBuilder::<Int32Type>::new();
            for testo in rese_testuali()? {
                match testo {
                    Some(testo) => {
                        builder.append(testo)?;
                    }
                    None => builder.append_null(),
                }
            }
            Arc::new(builder.finish())
        }
        TargetType::Date => Arc::new(StringArray::from(
            momenti
                .iter()
                .map(|momento| momento.map(|momento| momento.locale.format("%Y-%m-%d").to_string()))
                .collect::<Vec<_>>(),
        )),
        TargetType::Datetime => Arc::new(StringArray::from(
            momenti
                .iter()
                .map(|momento| {
                    momento.map(|momento| crate::temporale::testo_datetime(&momento.locale))
                })
                .collect::<Vec<_>>(),
        )),
        TargetType::Date32 => Arc::new(Date32Array::from(
            momenti
                .iter()
                .map(|momento| {
                    momento
                        .map(|momento| giorni_dall_epoca(&momento.locale).ok_or_else(interna))
                        .transpose()
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::TimestampMillis => Arc::new(
            TimestampMillisecondArray::from(
                momenti
                    .iter()
                    .map(|momento| {
                        momento
                            .map(|momento| millisecondi_di(&momento, timezone).ok_or_else(interna))
                            .transpose()
                    })
                    .collect::<Result<Vec<_>>>()?,
            )
            .with_timezone_opt(timezone.map(ToOwned::to_owned)),
        ),
        TargetType::Int
        | TargetType::Float
        | TargetType::Bool
        | TargetType::Decimal128
        | TargetType::Uint64 => return Err(interna()),
    })
}

fn string_cast_rejection(value: &str, config: &TypeCast) -> Option<&'static str> {
    let trimmed = value.trim();
    let invalid = match config.target_type {
        TargetType::Str | TargetType::BinaryUtf8 | TargetType::DictionaryUtf8 => return None,
        TargetType::Int => trimmed.parse::<i64>().is_err(),
        TargetType::Float => {
            if trimmed.contains(',') {
                trimmed.replace(',', ".").parse::<f64>().is_err()
            } else {
                trimmed.parse::<f64>().is_err()
            }
        }
        TargetType::Bool => !matches!(
            trimmed.to_lowercase().as_str(),
            "true"
                | "1"
                | "yes"
                | "si"
                | "sì"
                | "vero"
                | "t"
                | "y"
                | "s"
                | "false"
                | "0"
                | "no"
                | "falso"
                | "f"
                | "n"
        ),
        TargetType::Uint64 => trimmed.parse::<u64>().is_err(),
        TargetType::Date => parse_date(value, &config.date_format, false).is_none(),
        TargetType::Datetime => parse_date(value, &config.date_format, true).is_none(),
        TargetType::Date32 => parse_date32_fast(value, &config.date_format).is_none(),
        TargetType::TimestampMillis => {
            parse_timestamp_millis(value, &config.date_format, config.timezone.as_deref()).is_none()
        }
        TargetType::Decimal128 => match (config.precision, config.scale) {
            (Some(precision), Some(scale)) => parse_decimal128(value, precision, scale).is_none(),
            _ => return None,
        },
    };
    if !invalid {
        return None;
    }
    Some(match config.target_type {
        TargetType::Int => "conversion.invalid_integer",
        TargetType::Float => "conversion.invalid_float",
        TargetType::Bool => "conversion.invalid_boolean",
        TargetType::Uint64 => "conversion.invalid_unsigned_integer",
        TargetType::Date | TargetType::Date32 => "conversion.invalid_date",
        TargetType::Datetime => "conversion.invalid_datetime",
        TargetType::TimestampMillis => "conversion.invalid_timestamp",
        TargetType::Decimal128 => "conversion.invalid_decimal",
        TargetType::Str | TargetType::BinaryUtf8 | TargetType::DictionaryUtf8 => return None,
    })
}

/// Percorso generico (conversione scalare per riga): fallback per le
/// combinazioni non coperte dal fast path e oracolo dei test di equivalenza.
#[allow(clippy::too_many_lines)] // Un dispatcher esaustivo tiene revisionabili insieme tutte le politiche di cast.
fn type_cast_generic(source: &ArrayRef, config: &TypeCast) -> Result<ArrayRef> {
    let array: ArrayRef = match config.target_type {
        TargetType::Str => Arc::new(StringArray::from(
            (0..source.len())
                .map(|row| scalar_as_string(source.as_ref(), row))
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::Int => Arc::new(Int64Array::from(
            (0..source.len())
                .map(|row| {
                    scalar_as_string(source.as_ref(), row)?.map_or_else(
                        || Ok(None),
                        |value| {
                            value.trim().parse::<i64>().map_or_else(
                                |_| cast_failure(config.errori(), "conversione int fallita"),
                                |value| Ok(Some(value)),
                            )
                        },
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::Float => Arc::new(Float64Array::from(
            (0..source.len())
                .map(|row| {
                    scalar_as_string(source.as_ref(), row)?.map_or_else(
                        || Ok(None),
                        |value| {
                            value.trim().replace(',', ".").parse::<f64>().map_or_else(
                                |_| cast_failure(config.errori(), "conversione float fallita"),
                                |value| Ok(Some(value)),
                            )
                        },
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::Bool => Arc::new(BooleanArray::from(
            (0..source.len())
                .map(|row| {
                    scalar_as_string(source.as_ref(), row)?.map_or_else(
                        || Ok(None),
                        |value| {
                            let lower = value.trim().to_lowercase();
                            match lower.as_str() {
                                "true" | "1" | "yes" | "si" | "sì" | "vero" | "t" | "y" | "s" => {
                                    Ok(Some(true))
                                }
                                "false" | "0" | "no" | "falso" | "f" | "n" => Ok(Some(false)),
                                _ => cast_failure(config.errori(), "conversione bool fallita"),
                            }
                        },
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::Date | TargetType::Datetime => {
            let datetime = matches!(config.target_type, TargetType::Datetime);
            Arc::new(StringArray::from(
                (0..source.len())
                    .map(|row| {
                        scalar_as_string(source.as_ref(), row)?.map_or_else(
                            || Ok(None),
                            |value| {
                                parse_date(&value, &config.date_format, datetime).map_or_else(
                                    || cast_failure(config.errori(), "conversione data fallita"),
                                    |value| Ok(Some(value)),
                                )
                            },
                        )
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        TargetType::Date32 => Arc::new(Date32Array::from(
            source_strings(source)?
                .into_iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        parse_date32(&value, &config.date_format).map_or_else(
                            || cast_failure(config.errori(), "conversione date32 fallita"),
                            |value| Ok(Some(value)),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::TimestampMillis => {
            let values = source_strings(source)?
                .into_iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        parse_timestamp_millis(
                            &value,
                            &config.date_format,
                            config.timezone.as_deref(),
                        )
                        .map_or_else(
                            || cast_failure(config.errori(), "conversione timestamp fallita"),
                            |value| Ok(Some(value)),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let array =
                TimestampMillisecondArray::from(values).with_timezone_opt(config.timezone.clone());
            Arc::new(array)
        }
        TargetType::Decimal128 => {
            let precision = config
                .precision
                .ok_or_else(|| PlenoraError::InvalidPlan("decimal128 richiede precision".into()))?;
            let scale = config
                .scale
                .ok_or_else(|| PlenoraError::InvalidPlan("decimal128 richiede scale".into()))?;
            let values = source_strings(source)?
                .into_iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        parse_decimal128(&value, precision, scale).map_or_else(
                            || cast_failure(config.errori(), "conversione decimal128 fallita"),
                            |value| Ok(Some(value)),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Arc::new(Decimal128Array::from(values).with_precision_and_scale(precision, scale)?)
        }
        TargetType::BinaryUtf8 => {
            let mut builder = BinaryBuilder::new();
            for value in source_strings(source)? {
                if let Some(value) = value {
                    builder.append_value(value);
                } else {
                    builder.append_null();
                }
            }
            Arc::new(builder.finish())
        }
        TargetType::Uint64 => Arc::new(UInt64Array::from(
            source_strings(source)?
                .into_iter()
                .map(|value| {
                    value.map_or(Ok(None), |value| {
                        value.trim().parse::<u64>().map_or_else(
                            |_| cast_failure(config.errori(), "conversione uint64 fallita"),
                            |value| Ok(Some(value)),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        TargetType::DictionaryUtf8 => {
            let mut builder = StringDictionaryBuilder::<Int32Type>::new();
            for value in source_strings(source)? {
                if let Some(value) = value {
                    builder.append(value)?;
                } else {
                    builder.append_null();
                }
            }
            Arc::new(builder.finish())
        }
    };
    Ok(array)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{
        ArrayRef, BooleanArray, Date32Array, Float64Array, Int64Array, LargeStringArray,
        StringArray, TimestampMillisecondArray, UInt64Array,
    };
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::{json, Value};

    use super::*;
    use crate::test_support::nullable_batch as batch_of;

    // ------------------------------------------------------------------
    // Oracolo indipendente di fill_na (Vec<Option<T>> riga per riga +
    // fill_options), tenuto per i test di equivalenza.
    // ------------------------------------------------------------------

    fn fill_options<T: Clone>(out: &mut [Option<T>], method: &FillMethod, fixed: Option<T>) {
        match method {
            FillMethod::Value => {
                if let Some(value) = fixed {
                    for item in out {
                        if item.is_none() {
                            *item = Some(value.clone());
                        }
                    }
                }
            }
            FillMethod::Ffill => {
                let mut previous = None;
                for item in out {
                    if item.is_none() {
                        *item = previous.clone();
                    } else {
                        previous.clone_from(item);
                    }
                }
            }
            FillMethod::Bfill => {
                let mut following = None;
                for item in out.iter_mut().rev() {
                    if item.is_none() {
                        *item = following.clone();
                    } else {
                        following.clone_from(item);
                    }
                }
            }
        }
    }

    fn oracle_fill_array(
        array: &dyn Array,
        method: &FillMethod,
        value: &Value,
    ) -> Result<ArrayRef> {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            let fixed = match value {
                Value::Null => None,
                Value::String(v) => Some(v.clone()),
                other => Some(other.to_string()),
            };
            let mut out: Vec<Option<String>> =
                values.iter().map(|v| v.map(str::to_owned)).collect();
            fill_options(&mut out, method, fixed);
            return Ok(Arc::new(StringArray::from(out)));
        }
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            let fixed = match value {
                Value::Null => None,
                Value::Number(n) => Some(
                    n.as_i64()
                        .ok_or_else(|| PlenoraError::InvalidPlan("fill int non valido".into()))?,
                ),
                Value::String(s) => Some(
                    s.parse()
                        .map_err(|_| PlenoraError::InvalidPlan("fill int non valido".into()))?,
                ),
                _ => return Err(PlenoraError::InvalidPlan("fill int non valido".into())),
            };
            let mut out: Vec<Option<i64>> = values.iter().collect();
            fill_options(&mut out, method, fixed);
            return Ok(Arc::new(Int64Array::from(out)));
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            let fixed = match value {
                Value::Null => None,
                Value::Number(n) => n.as_f64(),
                Value::String(s) => Some(
                    s.replace(',', ".")
                        .parse()
                        .map_err(|_| PlenoraError::InvalidPlan("fill float non valido".into()))?,
                ),
                _ => return Err(PlenoraError::InvalidPlan("fill float non valido".into())),
            };
            let mut out: Vec<Option<f64>> = values.iter().collect();
            fill_options(&mut out, method, fixed);
            return Ok(Arc::new(Float64Array::from(out)));
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            let fixed = match value {
                Value::Null => None,
                Value::Bool(v) => Some(*v),
                Value::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
                Value::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
                _ => return Err(PlenoraError::InvalidPlan("fill bool non valido".into())),
            };
            let mut out: Vec<Option<bool>> = values.iter().collect();
            fill_options(&mut out, method, fixed);
            return Ok(Arc::new(BooleanArray::from(out)));
        }
        Err(PlenoraError::Schema(format!(
            "fill_na non supporta {:?}",
            array.data_type()
        )))
    }

    // ------------------------------------------------------------------
    // Helper di equivalenza
    // ------------------------------------------------------------------

    fn single_batch(array: ArrayRef) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "c",
                array.data_type().clone(),
                true,
            )])),
            vec![array],
        )
        .expect("batch")
    }

    fn fill_method(index: usize) -> FillMethod {
        match index {
            0 => FillMethod::Value,
            1 => FillMethod::Ffill,
            _ => FillMethod::Bfill,
        }
    }

    fn assert_fill_equiv(array: &ArrayRef, method: &FillMethod, value: &Value) {
        let fast = fill_array(array.as_ref(), method, value).map_err(|e| format!("{e:?}"));
        let generic =
            oracle_fill_array(array.as_ref(), method, value).map_err(|e| format!("{e:?}"));
        match (fast, generic) {
            (Ok(fast), Ok(generic)) => assert_eq!(single_batch(fast), single_batch(generic)),
            (Err(fast), Err(generic)) => assert_eq!(fast, generic),
            (fast, generic) => panic!(
                "fill_na: fast/generico divergono (fast err={}, generico err={})",
                fast.is_err(),
                generic.is_err()
            ),
        }
    }

    fn cast_config(target_type: TargetType, errors: CastErrors) -> TypeCast {
        let decimale = matches!(target_type, TargetType::Decimal128);
        let infallibile = matches!(
            target_type,
            TargetType::Str | TargetType::BinaryUtf8 | TargetType::DictionaryUtf8
        );
        TypeCast {
            column: "c".into(),
            target_type,
            date_format: String::new(),
            errors: (!infallibile).then_some(errors),
            precision: decimale.then_some(10),
            scale: decimale.then_some(2),
            timezone: None,
        }
    }

    fn assert_cast_equiv(array: &ArrayRef, config: &TypeCast) {
        let fast = type_cast_fast(array, config).map_err(|e| format!("{e:?}"));
        let generic = type_cast_generic(array, config).map_err(|e| format!("{e:?}"));
        match (fast, generic) {
            (Ok(Some(fast)), Ok(generic)) => assert_eq!(
                single_batch(fast),
                single_batch(generic),
                "type_cast verso {:?} diverge",
                config.target_type
            ),
            // Combinazione non coperta: il fallback e' il generico stesso.
            (Ok(None), _) => {}
            (Err(fast), Err(generic)) => assert_eq!(fast, generic),
            (fast, generic) => panic!(
                "type_cast verso {:?}: fast/generico divergono (fast ok={}, generico ok={})",
                config.target_type,
                fast.is_ok(),
                generic.is_ok()
            ),
        }
    }

    fn assert_cast_covered(array: &ArrayRef, config: &TypeCast) {
        if let Ok(result) = type_cast_fast(array, config) {
            // Con raise/ignore l'errore puo' essere legittimo: lo verifica
            // l'equivalenza; qui si controlla solo la copertura.
            assert!(
                result.is_some(),
                "combinazione {:?} attesa coperta dal fast path",
                config.target_type
            );
        }
        assert_cast_equiv(array, config);
    }

    fn all_targets() -> Vec<TargetType> {
        vec![
            TargetType::Str,
            TargetType::Int,
            TargetType::Float,
            TargetType::Bool,
            TargetType::Date,
            TargetType::Datetime,
            TargetType::Date32,
            TargetType::TimestampMillis,
            TargetType::Decimal128,
            TargetType::BinaryUtf8,
            TargetType::Uint64,
            TargetType::DictionaryUtf8,
        ]
    }

    fn all_errors() -> Vec<CastErrors> {
        vec![CastErrors::Coerce, CastErrors::Raise, CastErrors::Ignore]
    }

    // ------------------------------------------------------------------
    // fill_na
    // ------------------------------------------------------------------

    #[test]
    fn fill_fast_matches_oracle_on_int64_matrices() {
        let cases: Vec<Vec<Option<i64>>> = vec![
            vec![],
            vec![Some(1)],
            vec![None],
            vec![Some(1), None, Some(3), None, None, Some(6)],
            vec![None, None, Some(2)],
            vec![Some(i64::MIN), Some(i64::MAX), None],
            vec![None, None, None],
        ];
        let values = [
            Value::Null,
            json!(0),
            json!(-7),
            json!("11"),
            json!(1.5),
            json!(true),
        ];
        for case in &cases {
            let array: ArrayRef = Arc::new(Int64Array::from(case.clone()));
            for method in 0..3 {
                for value in &values {
                    assert_fill_equiv(&array, &fill_method(method), value);
                }
            }
        }
    }

    #[test]
    fn fill_fast_matches_oracle_on_float64_matrices() {
        let cases: Vec<Vec<Option<f64>>> = vec![
            vec![],
            vec![Some(0.0)],
            vec![None],
            vec![
                Some(f64::NAN),
                Some(-0.0),
                None,
                Some(f64::INFINITY),
                Some(f64::MIN),
                Some(f64::MAX),
            ],
            vec![None, Some(2.5), None, None],
            vec![None, None, None],
        ];
        let values = [Value::Null, json!(0), json!(2.5), json!("3,5"), json!("x")];
        for case in &cases {
            let array: ArrayRef = Arc::new(Float64Array::from(case.clone()));
            for method in 0..3 {
                for value in &values {
                    assert_fill_equiv(&array, &fill_method(method), value);
                }
            }
        }
    }

    #[test]
    fn fill_fast_matches_oracle_on_boolean_and_utf8_matrices() {
        let bool_cases: Vec<Vec<Option<bool>>> = vec![
            vec![],
            vec![Some(true)],
            vec![None],
            vec![Some(true), None, Some(false), None],
            vec![None, None],
        ];
        let bool_values = [
            Value::Null,
            json!(true),
            json!(false),
            json!("TRUE"),
            json!("no"),
            json!(1),
        ];
        for case in &bool_cases {
            let array: ArrayRef = Arc::new(BooleanArray::from(case.clone()));
            for method in 0..3 {
                for value in &bool_values {
                    assert_fill_equiv(&array, &fill_method(method), value);
                }
            }
        }
        let utf8_cases: Vec<Vec<Option<&str>>> = vec![
            vec![],
            vec![Some("a")],
            vec![None],
            vec![Some("x"), None, Some(""), None, None, Some("fine")],
            vec![None, None, Some("coda")],
            vec![None, None, None],
        ];
        let utf8_values = [Value::Null, json!("riempi"), json!(42), json!(false)];
        for case in &utf8_cases {
            let array: ArrayRef = Arc::new(StringArray::from(case.clone()));
            for method in 0..3 {
                for value in &utf8_values {
                    assert_fill_equiv(&array, &fill_method(method), value);
                }
            }
        }
    }

    /// Regressione: su una colonna `Int64` un `value` numerico non intero o
    /// fuori da `i64` valeva come assente e la colonna restava con i suoi
    /// null, senza errore. Ora si rifiuta anche senza l'analisi.
    #[test]
    fn fill_int_rifiuta_un_numero_non_intero_invece_di_ignorarlo() {
        let array: ArrayRef = Arc::new(Int64Array::from(vec![Some(1), None]));
        for valore in [
            serde_json::json!(1.5),
            serde_json::json!(9_223_372_036_854_775_808_u64),
            serde_json::json!(1e300),
        ] {
            assert!(
                matches!(
                    fill_array(array.as_ref(), &FillMethod::Value, &valore),
                    Err(PlenoraError::InvalidPlan(_))
                ),
                "{valore}"
            );
        }
        let pieno = fill_array(array.as_ref(), &FillMethod::Value, &serde_json::json!(7))
            .expect("intero valido");
        assert_eq!(pieno.null_count(), 0);
    }

    #[test]
    fn fill_unsupported_types_keep_the_schema_error() {
        let array: ArrayRef = Arc::new(UInt64Array::from(vec![Some(1_u64), None]));
        for method in 0..3 {
            for value in [Value::Null, json!(7)] {
                assert_fill_equiv(&array, &fill_method(method), &value);
            }
        }
    }

    // ------------------------------------------------------------------
    // type_cast
    // ------------------------------------------------------------------

    #[test]
    fn cast_fast_matches_generic_on_utf8_matrix() {
        let texts: Vec<Option<&str>> = vec![
            Some("42"),
            Some("-7"),
            Some(" 8 "),
            Some("abc"),
            Some(""),
            Some("  "),
            Some("9.5"),
            Some("3,14"),
            Some("1e3"),
            Some("NaN"),
            Some("inf"),
            Some("-inf"),
            Some("true"),
            Some("FALSE"),
            Some("sì"),
            Some("SÌ"),
            Some("no"),
            Some("vero"),
            Some("0"),
            Some("1"),
            Some("+5"),
            Some("-0"),
            Some("18446744073709551615"),
            Some("18446744073709551616"),
            Some("9223372036854775807"),
            Some("9223372036854775808"),
            Some("-9223372036854775809"),
            Some("2024-01-31"),
            Some("31/01/2024"),
            Some("2024-13-40"),
            Some("2024-01-31T10:20:30"),
            Some("2024-01-31 10:20:30"),
            None,
        ];
        let array: ArrayRef = Arc::new(StringArray::from(texts));
        for target in all_targets() {
            for errors in all_errors() {
                assert_cast_covered(&array, &cast_config(target, errors));
            }
        }
    }

    #[test]
    fn cast_fast_matches_generic_with_date_format_and_timezone() {
        let dates: Vec<Option<&str>> = vec![
            Some("31/01/2024"),
            Some("2024-01-31"),
            Some("30/02/2024"),
            Some("bad"),
            None,
        ];
        let array: ArrayRef = Arc::new(StringArray::from(dates));
        for target in [
            TargetType::Date,
            TargetType::Datetime,
            TargetType::Date32,
            TargetType::TimestampMillis,
        ] {
            for errors in all_errors() {
                let mut config = cast_config(target, errors);
                config.date_format = "%d/%m/%Y".into();
                assert_cast_covered(&array, &config);
            }
        }
        // Timestamp con timezone, incluse date inesistenti (buco DST) e rfc3339.
        let stamps: Vec<Option<&str>> = vec![
            Some("2024-01-31 10:20:30"),
            Some("2024-03-31 02:30:00"),
            Some("2024-10-27 02:30:00"),
            Some("2024-01-31T10:20:30+01:00"),
            Some("bad"),
            None,
        ];
        let array: ArrayRef = Arc::new(StringArray::from(stamps));
        for errors in all_errors() {
            let mut config = cast_config(TargetType::TimestampMillis, errors);
            config.timezone = Some("Europe/Rome".into());
            assert_cast_covered(&array, &config);
        }
    }

    #[test]
    fn cast_fast_matches_generic_on_numeric_matrices() {
        let int64s: ArrayRef = Arc::new(Int64Array::from(vec![
            Some(0),
            Some(1),
            Some(-1),
            Some(i64::MAX),
            Some(i64::MIN),
            None,
            Some(42),
        ]));
        let uint64s: ArrayRef = Arc::new(UInt64Array::from(vec![
            Some(0),
            Some(1),
            Some(u64::MAX),
            None,
            Some(7),
        ]));
        let float64s: ArrayRef = Arc::new(Float64Array::from(vec![
            Some(0.0),
            Some(-0.0),
            Some(1.0),
            Some(-1.0),
            Some(42.0),
            Some(42.5),
            Some(-0.5),
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(f64::NEG_INFINITY),
            Some(9_223_372_036_854_775_808.0),
            Some(-9_223_372_036_854_775_808.0),
            Some(18_446_744_073_709_551_616.0),
            Some(1e300),
            Some(1e-300),
            None,
        ]));
        let bools: ArrayRef = Arc::new(BooleanArray::from(vec![Some(true), Some(false), None]));
        for array in [&int64s, &uint64s, &float64s, &bools] {
            for target in all_targets() {
                for errors in all_errors() {
                    assert_cast_equiv(array, &cast_config(target, errors));
                }
            }
            // Copertura esplicita dei fast path numerici.
            for target in [
                TargetType::Str,
                TargetType::Int,
                TargetType::Float,
                TargetType::Bool,
                TargetType::Uint64,
            ] {
                assert_cast_covered(array, &cast_config(target, CastErrors::Coerce));
            }
        }
    }

    #[test]
    fn cast_fast_matches_generic_on_empty_and_single_row() {
        let cases: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
            Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
            Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
            Arc::new(BooleanArray::from(Vec::<Option<bool>>::new())),
            Arc::new(UInt64Array::from(Vec::<Option<u64>>::new())),
            Arc::new(StringArray::from(vec![Some("42")])),
            Arc::new(StringArray::from(vec![Option::<&str>::None])),
            Arc::new(Int64Array::from(vec![Option::<i64>::None])),
            Arc::new(Float64Array::from(vec![Some(-0.0)])),
            Arc::new(BooleanArray::from(vec![Some(true)])),
        ];
        for array in &cases {
            for target in all_targets() {
                for errors in all_errors() {
                    assert_cast_equiv(array, &cast_config(target, errors));
                }
            }
        }
    }

    /// Le colonne temporali si convertono dal valore nativo, non dal testo
    /// (regressione: `datetime` su un `Timestamp` falliva su ogni riga,
    /// perche' il testo RFC 3339 con `+00:00` non era un formato di
    /// default, e un `Timestamp` in microsecondi non si leggeva affatto).
    ///
    /// Oracolo: per `str`, `binary_utf8`, `dictionary_utf8` su `Date32` e
    /// `Timestamp(ms)` il percorso testuale di prima (`generic_type_cast_entry`,
    /// che passa da `scalar_as_string`); per date e istanti valori scritti a
    /// mano, su ogni unita' e con e senza fuso.
    #[test]
    #[allow(clippy::too_many_lines)] // Una matrice di tipi e target scritta a mano.
    fn type_cast_da_colonne_temporali_senza_passare_dal_testo() {
        use plenora_core::arrow::array::{
            TimestampMicrosecondArray, TimestampNanosecondArray, TimestampSecondArray,
        };
        let ms = 1_706_696_430_123_i64; // 2024-01-31T10:20:30.123Z
        let date_ms: Vec<ArrayRef> = vec![
            Arc::new(Date32Array::from(vec![
                Some(0),
                Some(19_753),
                Some(-1),
                None,
            ])),
            Arc::new(TimestampMillisecondArray::from(vec![
                Some(0),
                Some(ms),
                Some(-1),
                None,
            ])),
            Arc::new(
                TimestampMillisecondArray::from(vec![Some(0), Some(ms), None])
                    .with_timezone("Europe/Rome"),
            ),
        ];
        for source in &date_ms {
            let batch = single_batch(source.clone());
            for target in [
                TargetType::Str,
                TargetType::BinaryUtf8,
                TargetType::DictionaryUtf8,
            ] {
                let config = cast_config(target, CastErrors::Raise);
                assert_eq!(
                    type_cast(&batch, &config).expect("testo"),
                    generic_type_cast_entry(&batch, &config).expect("percorso testuale"),
                    "{target:?}"
                );
            }
            // Un numero o un booleano da una data non ha un significato
            // scritto: errore di piano, non righe rifiutate.
            for target in [
                TargetType::Int,
                TargetType::Float,
                TargetType::Bool,
                TargetType::Uint64,
                TargetType::Decimal128,
            ] {
                for errors in all_errors() {
                    assert!(matches!(
                        type_cast(&batch, &cast_config(target, errors)),
                        Err(PlenoraError::InvalidPlan(_))
                    ));
                }
            }
            // `date_format` legge un testo: con una colonna temporale si
            // rifiuta invece di essere ignorato.
            let mut con_formato = cast_config(TargetType::Date, CastErrors::Raise);
            con_formato.date_format = "%Y-%m-%d".into();
            assert!(matches!(
                type_cast(&batch, &con_formato),
                Err(PlenoraError::InvalidPlan(_))
            ));
        }

        let cast = |array: ArrayRef, target: TargetType, timezone: Option<&str>| {
            let mut config = cast_config(target, CastErrors::Raise);
            config.timezone = timezone.map(ToOwned::to_owned);
            type_cast(&single_batch(array), &config)
        };
        let unita: Vec<(ArrayRef, &str, i64)> = vec![
            (
                Arc::new(TimestampSecondArray::from(vec![ms / 1000])),
                "2024-01-31T10:20:30",
                1_706_696_430_000,
            ),
            (
                Arc::new(TimestampMillisecondArray::from(vec![ms])),
                "2024-01-31T10:20:30.123",
                ms,
            ),
            (
                Arc::new(TimestampMicrosecondArray::from(vec![ms * 1000])),
                "2024-01-31T10:20:30.123",
                ms,
            ),
            (
                Arc::new(TimestampNanosecondArray::from(vec![ms * 1_000_000])),
                "2024-01-31T10:20:30.123",
                ms,
            ),
            // Con fuso: l'ora locale del fuso, lo stesso istante.
            (
                Arc::new(
                    TimestampMicrosecondArray::from(vec![ms * 1000]).with_timezone("Europe/Rome"),
                ),
                "2024-01-31T11:20:30.123",
                ms,
            ),
        ];
        for (array, datetime, millis) in unita {
            let testo = |target| {
                cast(array.clone(), target, None)
                    .expect("cast")
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .expect("utf8")
                    .value(0)
                    .to_owned()
            };
            assert_eq!(testo(TargetType::Datetime), datetime);
            assert_eq!(testo(TargetType::Date), "2024-01-31");
            let giorni = cast(array.clone(), TargetType::Date32, None).expect("date32");
            assert_eq!(cast_column::<Date32Array>(&giorni).value(0), 19_753);
            let istanti = cast(
                array.clone(),
                TargetType::TimestampMillis,
                Some("Europe/Rome"),
            )
            .expect("timestamp_millis");
            assert_eq!(
                istanti.schema().field(0).data_type(),
                &DataType::Timestamp(
                    plenora_core::arrow::schema::TimeUnit::Millisecond,
                    Some("Europe/Rome".into())
                )
            );
            assert_eq!(
                cast_column::<TimestampMillisecondArray>(&istanti).value(0),
                millis
            );
        }
        // Sotto il millisecondo: la riga si rifiuta invece di troncare.
        let micro: ArrayRef = Arc::new(TimestampMicrosecondArray::from(vec![
            Some(ms * 1000),
            Some(ms * 1000 + 456),
        ]));
        let errore = cast(micro, TargetType::TimestampMillis, None).expect_err("sotto il ms");
        let report = errore.row_diagnostics().expect("diagnostica");
        assert_eq!(
            report
                .examples
                .iter()
                .map(|esempio| (esempio.source_index, esempio.cause.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "conversion.timestamp_precision")]
        );
        // Una data e' la sua mezzanotte nel fuso dichiarato: 2024-01-31
        // 00:00 a Roma e' 2024-01-30T23:00Z.
        let giorno: ArrayRef = Arc::new(Date32Array::from(vec![19_753]));
        let istante = cast(giorno, TargetType::TimestampMillis, Some("Europe/Rome")).expect("data");
        assert_eq!(
            cast_column::<TimestampMillisecondArray>(&istante).value(0),
            1_706_655_600_000
        );
    }

    /// Oracolo del punto d'ingresso pubblico: con `coerce`/`raise` ogni riga
    /// non convertibile rifiuta il batch con diagnostica row-scoped (costruita
    /// da `reject_rows`, indipendente da `cast_row_diagnostics`); altrimenti
    /// il risultato e' quello del percorso generico.
    ///
    /// Che cosa garantisce: l'aggregazione dei rifiuti (righe, ordine, cause,
    /// conteggi, completezza) e la scelta fra rifiuto e conversione.
    /// Che cosa NON garantisce: condivide con la produzione
    /// `string_cast_rejection` (la decisione riga per riga) e
    /// `type_cast_generic` (il valore convertito), quindi un difetto in quei
    /// due non emerge qui. Li coprono i valori scritti a mano di
    /// `type_cast_hand_written_values_per_*`.
    fn generic_type_cast_entry(batch: &RecordBatch, config: &TypeCast) -> Result<RecordBatch> {
        let source = batch.column(column_index(batch, &config.column)?);
        if matches!(config.errori(), CastErrors::Coerce | CastErrors::Raise) {
            let mut rejections = Vec::new();
            for row in 0..source.len() {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    continue;
                };
                if let Some(cause) = string_cast_rejection(&value, config) {
                    rejections.push(crate::RowRejection {
                        row,
                        cause,
                        column: Some(&config.column),
                    });
                }
            }
            crate::reject_rows(
                &rejections,
                "conversione rifiutata; consultare row_diagnostics",
            )?;
        }
        type_cast_generic(source, config).map(single_batch)
    }

    /// Cast di una colonna Utf8 con `raise` (i token `coerce`/`raise` sono
    /// equivalenti per i target fallibili).
    fn cast_utf8(values: &[&str], target: TargetType) -> Result<RecordBatch> {
        let batch = single_batch(Arc::new(StringArray::from(values.to_vec())));
        type_cast(&batch, &cast_config(target, CastErrors::Raise))
    }

    /// Righe rifiutate, tutte con `cause` sulla colonna `c`.
    fn assert_cast_rejected(values: &[&str], target: TargetType, rows: &[u64], cause: &str) {
        for errors in [CastErrors::Coerce, CastErrors::Raise] {
            let batch = single_batch(Arc::new(StringArray::from(values.to_vec())));
            let error = type_cast(&batch, &cast_config(target, errors))
                .expect_err("valori non convertibili accettati");
            let report = error.row_diagnostics().expect("diagnostica row-scoped");
            assert_eq!(
                report
                    .examples
                    .iter()
                    .map(|example| (
                        example.source_index,
                        example.cause.as_str(),
                        example.column.as_deref()
                    ))
                    .collect::<Vec<_>>(),
                rows.iter()
                    .map(|row| (*row, cause, Some("c")))
                    .collect::<Vec<_>>(),
                "{target:?}"
            );
        }
    }

    fn cast_column<T: 'static>(batch: &RecordBatch) -> &T {
        batch
            .column(0)
            .as_any()
            .downcast_ref::<T>()
            .expect("tipo di output")
    }

    #[test]
    fn type_cast_hand_written_values_per_numeric_target() {
        // Valori attesi scritti a mano, indipendenti da `string_cast_rejection`
        // e da `type_cast_generic`: sono la specifica, non un'altra copia.
        let ints = cast_utf8(&[" 8 ", "-7", "+5", "9223372036854775807"], TargetType::Int)
            .expect("int validi");
        assert_eq!(
            cast_column::<Int64Array>(&ints).values().to_vec(),
            vec![8, -7, 5, i64::MAX]
        );
        assert_cast_rejected(
            &["1", "9223372036854775808", "1.5", "abc", ""],
            TargetType::Int,
            &[1, 2, 3, 4],
            "conversion.invalid_integer",
        );

        let floats =
            cast_utf8(&["2,25", "1e3", "-0.5", "inf"], TargetType::Float).expect("float validi");
        assert_eq!(
            cast_column::<Float64Array>(&floats).values().to_vec(),
            vec![2.25, 1000.0, -0.5, f64::INFINITY]
        );
        assert_cast_rejected(
            &["abc", "1,2,3", "2.5"],
            TargetType::Float,
            &[0, 1],
            "conversion.invalid_float",
        );

        let bools = cast_utf8(
            &["sì", "SÌ", "vero", "t", "Y", "falso", "0", "N"],
            TargetType::Bool,
        )
        .expect("bool validi");
        assert_eq!(
            cast_column::<BooleanArray>(&bools)
                .iter()
                .collect::<Vec<_>>(),
            vec![
                Some(true),
                Some(true),
                Some(true),
                Some(true),
                Some(true),
                Some(false),
                Some(false),
                Some(false)
            ]
        );
        assert_cast_rejected(
            &["true", "2", "maybe"],
            TargetType::Bool,
            &[1, 2],
            "conversion.invalid_boolean",
        );

        let uints =
            cast_utf8(&["0", "18446744073709551615"], TargetType::Uint64).expect("uint validi");
        assert_eq!(
            cast_column::<UInt64Array>(&uints).values().to_vec(),
            vec![0, u64::MAX]
        );
        assert_cast_rejected(
            &["-1", "7", "18446744073709551616"],
            TargetType::Uint64,
            &[0, 2],
            "conversion.invalid_unsigned_integer",
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Valori scritti a mano per ogni target.
    fn type_cast_hand_written_values_per_temporal_and_decimal_target() {
        // Valori attesi scritti a mano, come per i target numerici.
        // Default solo ISO 8601: l'offset si legge (l'ora resta quella
        // scritta), giorno e mese in un ordine da indovinare no.
        let dates = cast_utf8(
            &[
                "2024-01-31",
                "2024-01-31T23:30:00+05:00",
                "2024-01-31 10:00:00",
            ],
            TargetType::Date,
        )
        .expect("date valide");
        assert_eq!(
            cast_column::<StringArray>(&dates)
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("2024-01-31"); 3]
        );
        assert_cast_rejected(
            &[
                "2024-13-40",
                "2024-02-29",
                "2023-02-29",
                "31/01/2024",
                "31-01-2024",
                "2024/01/31",
            ],
            TargetType::Date,
            &[0, 2, 3, 4, 5],
            "conversion.invalid_date",
        );

        let datetimes = cast_utf8(
            &[
                "2024-01-31 10:20:30",
                "2024-01-31T10:20:30.250",
                "2024-01-31",
                "2024-01-31T10:20:30+00:00",
            ],
            TargetType::Datetime,
        )
        .expect("datetime validi");
        // La frazione di secondo resta (prima si troncava in silenzio).
        assert_eq!(
            cast_column::<StringArray>(&datetimes)
                .iter()
                .collect::<Vec<_>>(),
            vec![
                Some("2024-01-31T10:20:30"),
                Some("2024-01-31T10:20:30.250"),
                Some("2024-01-31T00:00:00"),
                Some("2024-01-31T10:20:30")
            ]
        );
        assert_cast_rejected(
            &["31/01/2024 10:20:30", "2024-01-31 10:20:30"],
            TargetType::Datetime,
            &[0],
            "conversion.invalid_datetime",
        );
        assert_cast_rejected(
            &["2024-01-31 25:00:00", "bad"],
            TargetType::Datetime,
            &[0, 1],
            "conversion.invalid_datetime",
        );

        // 2024-01-31: 54 anni dal 1970 con 13 bisestili (1972..=2020), piu' 30.
        let days = cast_utf8(
            &["1970-01-01", "1969-12-31", "2024-01-31"],
            TargetType::Date32,
        )
        .expect("date32 valide");
        assert_eq!(
            cast_column::<Date32Array>(&days).values().to_vec(),
            vec![0, -1, 19_753]
        );
        assert_cast_rejected(
            &["2024-13-40", "1970-01-01"],
            TargetType::Date32,
            &[0],
            "conversion.invalid_date",
        );

        // 19753 giorni * 86_400 s + 10:20:30 = 1_706_696_430 s; l'offset
        // +01:00 toglie un'ora.
        let stamps = cast_utf8(
            &[
                "2024-01-31 10:20:30",
                "2024-01-31T10:20:30+01:00",
                "1970-01-01",
            ],
            TargetType::TimestampMillis,
        )
        .expect("timestamp validi");
        assert_eq!(
            cast_column::<TimestampMillisecondArray>(&stamps)
                .values()
                .to_vec(),
            vec![1_706_696_430_000, 1_706_692_830_000, 0]
        );
        assert_cast_rejected(
            &["bad", "2024-01-31"],
            TargetType::TimestampMillis,
            &[0],
            "conversion.invalid_timestamp",
        );

        // precision 10, scale 2.
        let decimals = cast_utf8(&["12.34", "-0.5", "12345678.9"], TargetType::Decimal128)
            .expect("decimal validi");
        assert_eq!(
            cast_column::<Decimal128Array>(&decimals).values().to_vec(),
            vec![1_234, -50, 1_234_567_890]
        );
        assert_cast_rejected(
            &["12.345", "1", "123456789.1", "abc", "1.2.3"],
            TargetType::Decimal128,
            &[0, 2, 3, 4],
            "conversion.invalid_decimal",
        );
    }

    /// Regressione (classe «segni ripetuti»): ogni target numerico rifiuta
    /// un secondo segno. Il decimale toglieva `-` e poi `+`, e leggeva
    /// `"-+5"` come -5; interi e float passano dai `parse` della libreria
    /// standard, che gia' rifiutavano: il test fissa che restino allineati.
    #[test]
    fn type_cast_rifiuta_i_segni_ripetuti_su_ogni_target_numerico() {
        let valori = ["-+5", "+-5", "--5", "++5", "-5", "+5"];
        for (target, causa) in [
            (TargetType::Decimal128, "conversion.invalid_decimal"),
            (TargetType::Int, "conversion.invalid_integer"),
            (TargetType::Float, "conversion.invalid_float"),
        ] {
            assert_cast_rejected(&valori, target, &[0, 1, 2, 3], causa);
        }
        assert_cast_rejected(
            &["-+5", "+-5", "--5", "++5", "+5"],
            TargetType::Uint64,
            &[0, 1, 2, 3],
            "conversion.invalid_unsigned_integer",
        );
        let decimali = cast_utf8(&["-5", "+5"], TargetType::Decimal128).expect("un segno solo");
        assert_eq!(
            cast_column::<Decimal128Array>(&decimali).values().to_vec(),
            vec![-500, 500]
        );
    }

    #[test]
    fn cast_large_utf8_keeps_the_scalar_profile_error() {
        let batch = single_batch(Arc::new(LargeStringArray::from(vec![Some("42"), None])));
        for target in all_targets() {
            let config = cast_config(target, CastErrors::Coerce);
            assert!(type_cast(&batch, &config).is_err());
        }
    }

    // ------------------------------------------------------------------
    // coalesce
    // ------------------------------------------------------------------

    fn assert_coalesce_equiv(columns: Vec<(&str, ArrayRef)>) {
        let batch = batch_of(columns);
        let indices: Vec<usize> = (0..batch.num_columns()).collect();
        let generic = crate::quality::coalesce_generic(&batch, &indices).expect("generico");
        if let Some(fast) = coalesce_fast(&batch, &indices) {
            assert_eq!(single_batch(fast), single_batch(generic));
        }
    }

    #[test]
    fn coalesce_fast_matches_generic_on_numeric_types() {
        assert_coalesce_equiv(vec![
            (
                "a",
                Arc::new(Int64Array::from(vec![None, Some(1), None, Some(4)])),
            ),
            (
                "b",
                Arc::new(Int64Array::from(vec![None, None, Some(3), Some(5)])),
            ),
            (
                "c",
                Arc::new(Int64Array::from(vec![Some(9), Some(9), Some(9), None])),
            ),
        ]);
        assert_coalesce_equiv(vec![
            ("a", Arc::new(UInt64Array::from(vec![None, Some(u64::MAX)]))),
            ("b", Arc::new(UInt64Array::from(vec![Some(1), None]))),
        ]);
        assert_coalesce_equiv(vec![
            (
                "a",
                Arc::new(Float64Array::from(vec![None, Some(f64::NAN), Some(-0.0)])),
            ),
            (
                "b",
                Arc::new(Float64Array::from(vec![
                    Some(2.5),
                    None,
                    Some(f64::INFINITY),
                ])),
            ),
        ]);
        assert_coalesce_equiv(vec![
            (
                "a",
                Arc::new(BooleanArray::from(vec![None, Some(true), None])),
            ),
            (
                "b",
                Arc::new(BooleanArray::from(vec![Some(false), None, Some(true)])),
            ),
        ]);
    }

    #[test]
    fn coalesce_fast_matches_generic_on_utf8_and_edge_cases() {
        assert_coalesce_equiv(vec![
            (
                "a",
                Arc::new(StringArray::from(vec![None, Some("x"), None])),
            ),
            (
                "b",
                Arc::new(StringArray::from(vec![None, None, Some("y")])),
            ),
            (
                "c",
                Arc::new(StringArray::from(vec![Some("z"), Some("z"), None])),
            ),
        ]);
        // Prima colonna senza null: scorciatoia identita'.
        assert_coalesce_equiv(vec![
            ("a", Arc::new(Int64Array::from(vec![Some(1), Some(2)]))),
            ("b", Arc::new(Int64Array::from(vec![None, Some(3)]))),
        ]);
        // Colonna singola, tutti null, batch vuoto, riga singola.
        assert_coalesce_equiv(vec![(
            "a",
            Arc::new(Int64Array::from(vec![Option::<i64>::None])),
        )]);
        assert_coalesce_equiv(vec![(
            "a",
            Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
        )]);
        assert_coalesce_equiv(vec![("a", Arc::new(StringArray::from(vec![Some("solo")])))]);
    }

    #[test]
    fn invalid_dates_return_complete_row_diagnostics_instead_of_coerced_nulls() {
        let values = (0..1_025)
            .map(|source_index| {
                if source_index == 4 {
                    Some("2026-02-30".to_owned())
                } else if source_index == 1_004 {
                    Some("not-a-date".to_owned())
                } else {
                    Some("2026-08-02".to_owned())
                }
            })
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_from_iter([(
            "effective_date",
            Arc::new(StringArray::from(values)) as ArrayRef,
        )])
        .unwrap();
        let config = TypeCast {
            column: "effective_date".to_owned(),
            target_type: TargetType::Date32,
            errors: Some(CastErrors::Coerce),
            date_format: String::new(),
            timezone: None,
            precision: None,
            scale: None,
        };

        let error = type_cast(&batch, &config).expect_err("date invalide accettate");
        let diagnostics = error.row_diagnostics().expect("diagnostica persa");
        assert_eq!(diagnostics.contract, "plenora-row-diagnostics-v1");
        assert_eq!(diagnostics.index_basis, "source_row_zero_based");
        assert_eq!(diagnostics.total, Some(2));
        assert_eq!(diagnostics.observed_total, 2);
        assert_eq!(diagnostics.counts["conversion.invalid_date"], 2);
        assert_eq!(
            diagnostics
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            vec![4, 1_004]
        );
        assert!(diagnostics
            .examples
            .iter()
            .all(|example| example.column.as_deref() == Some("effective_date")));
        assert!(!error.to_string().contains("2026-02-30"));
        assert!(!error.to_string().contains("not-a-date"));
    }

    #[test]
    fn invalid_date_indices_include_the_stream_source_offset() {
        let batch = RecordBatch::try_from_iter([(
            "effective_date",
            Arc::new(StringArray::from(vec![
                Some("2026-08-02"),
                Some("2026-08-02"),
                Some("2026-08-02"),
                Some("2026-08-02"),
                Some("not-a-date"),
            ])) as ArrayRef,
        )])
        .unwrap();
        let config = TypeCast {
            column: "effective_date".to_owned(),
            target_type: TargetType::Date32,
            errors: Some(CastErrors::Coerce),
            date_format: String::new(),
            timezone: None,
            precision: None,
            scale: None,
        };

        let error = type_cast_with_source_offset(&batch, &config, 1_000)
            .expect_err("date invalida accettata");
        let diagnostics = error.row_diagnostics().expect("diagnostica persa");
        assert_eq!(diagnostics.observed_total, 1);
        assert_eq!(diagnostics.examples[0].source_index, 1_004);
    }

    #[test]
    fn invalid_dates_raise_policy_also_returns_complete_row_diagnostics() {
        let batch = RecordBatch::try_from_iter([(
            "effective_date",
            Arc::new(StringArray::from(vec![
                Some("bad"),
                Some("2026-08-02"),
                Some("bad"),
            ])) as ArrayRef,
        )])
        .unwrap();
        let config = TypeCast {
            column: "effective_date".to_owned(),
            target_type: TargetType::Date32,
            errors: Some(CastErrors::Raise),
            date_format: String::new(),
            timezone: None,
            precision: None,
            scale: None,
        };

        let error = type_cast(&batch, &config).expect_err("date invalide accettate con Raise");
        let report = error.row_diagnostics().expect("diagnostica mancante");
        assert_eq!(report.observed_total, 2);
        assert_eq!(report.total, Some(2));
        assert_eq!(
            report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
    }

    #[test]
    fn invalid_date_examples_are_bounded_without_truncating_counts() {
        let batch = RecordBatch::try_from_iter([(
            "effective_date",
            Arc::new(StringArray::from(vec![Some("bad"); 12])) as ArrayRef,
        )])
        .unwrap();
        let config = TypeCast {
            column: "effective_date".to_owned(),
            target_type: TargetType::Date32,
            errors: Some(CastErrors::Coerce),
            date_format: String::new(),
            timezone: None,
            precision: None,
            scale: None,
        };

        let error = type_cast(&batch, &config).expect_err("date invalide accettate");
        let report = error.row_diagnostics().expect("diagnostica mancante");
        assert_eq!(report.observed_total, 12);
        assert_eq!(report.total, Some(12));
        assert_eq!(report.counts["conversion.invalid_date"], 12);
        assert_eq!(report.examples.len(), 10);
        assert!(report.examples_truncated);
    }

    #[test]
    fn string_cast_targets_reject_invalid_rows_with_stable_causes() {
        let cases = [
            (TargetType::Int, "conversion.invalid_integer", None, None),
            (TargetType::Float, "conversion.invalid_float", None, None),
            (TargetType::Bool, "conversion.invalid_boolean", None, None),
            (
                TargetType::Uint64,
                "conversion.invalid_unsigned_integer",
                None,
                None,
            ),
            (TargetType::Date, "conversion.invalid_date", None, None),
            (
                TargetType::Datetime,
                "conversion.invalid_datetime",
                None,
                None,
            ),
            (TargetType::Date32, "conversion.invalid_date", None, None),
            (
                TargetType::TimestampMillis,
                "conversion.invalid_timestamp",
                None,
                None,
            ),
            (
                TargetType::Decimal128,
                "conversion.invalid_decimal",
                Some(18),
                Some(2),
            ),
        ];
        for (target_type, cause, precision, scale) in cases {
            let batch = RecordBatch::try_from_iter([(
                "value",
                Arc::new(StringArray::from(vec![Some("not-valid")])) as ArrayRef,
            )])
            .unwrap();
            let config = TypeCast {
                column: "value".to_owned(),
                target_type,
                errors: Some(CastErrors::Coerce),
                date_format: String::new(),
                timezone: None,
                precision,
                scale,
            };
            let error = type_cast(&batch, &config).expect_err("valore invalido accettato");
            let report = error.row_diagnostics().expect("diagnostica mancante");
            assert_eq!(report.counts[cause], 1, "target {target_type:?}");
            assert_eq!(report.examples[0].cause, cause);
        }
    }

    #[test]
    fn type_cast_str_preserves_null_in_fast_and_generic_paths() {
        let config = TypeCast {
            column: "value".to_owned(),
            target_type: TargetType::Str,
            errors: None,
            date_format: String::new(),
            timezone: None,
            precision: None,
            scale: None,
        };
        for source in [
            Arc::new(Int64Array::from(vec![Some(7), None])) as ArrayRef,
            Arc::new(Date32Array::from(vec![Some(0), None])) as ArrayRef,
        ] {
            let output = type_cast_fast(&source, &config)
                .expect("fast path")
                .unwrap_or_else(|| type_cast_generic(&source, &config).expect("fallback"));
            let strings = output
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("output Utf8");
            assert!(strings.is_null(1));

            let generic = type_cast_generic(&source, &config).expect("oracolo generico");
            let generic = generic
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("output generico Utf8");
            assert!(generic.is_null(1));
        }
    }

    #[test]
    fn coalesce_uncovered_types_fall_back_to_the_generic_path() {
        let batch = batch_of(vec![
            ("a", Arc::new(Date32Array::from(vec![None, Some(0)]))),
            ("b", Arc::new(Date32Array::from(vec![Some(19000), None]))),
        ]);
        assert!(coalesce_fast(&batch, &[0, 1]).is_none());
        let config = crate::quality::Coalesce {
            columns: vec!["a".into(), "b".into()],
            output_column: "out".into(),
        };
        let production = crate::quality::coalesce(&batch, &config).expect("coalesce");
        let generic = crate::quality::coalesce_generic(&batch, &[0, 1]).expect("generico");
        assert_eq!(
            single_batch(production.column(2).clone()),
            single_batch(generic)
        );
    }
}
