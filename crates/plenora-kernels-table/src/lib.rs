//! plenora-kernels-table — kernel tabellari puri `&RecordBatch -> Result<RecordBatch>`
//! (Architetture.md par. 3.2).
//!
//! Fase 1 "coesistenza": trasloco meccanico dei 17 moduli kernel da
//! `plenora-nogeo-tools/src/kernels/` (`columns`, `strings`, `cleansing`,
//! `filtering`, `dates`, `utility`, `analysis`, `aggregation`, `reshape`,
//! `joins`, `setops`, `security`, `quality`, `governance`, `formula`,
//! `expressions`, `spill`) con gli helper condivisi, senza modifiche di
//! comportamento.

use serde::{Deserialize, Serialize};

/// Limiti dei kernel tabellari, traslocati identici da
/// `plenora-nogeo-tools/src/contract.rs` (Fase 1, zero modifiche di
/// comportamento).
///
/// NOTA (punto aperto per la fase engine): il `Limits` unificato di
/// `plenora_core::limits` (decisione D19, ADR 6) non copre `max_columns` e
/// `max_split_columns`, e sostituisce il singolo `max_rows` con la famiglia
/// semantica `RowLimits` (`max_input_rows` / `max_output_rows` /
/// `max_rows_per_edge`). La mappatura di questa struct su
/// `plenora_core::limits::Limits` e' una decisione semantica demandata alla
/// fase engine, non un adattamento meccanico.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_rows: usize,
    pub max_columns: usize,
    pub max_string_bytes: usize,
    pub max_regex_bytes: usize,
    pub max_split_columns: usize,
    pub max_memory_bytes: usize,
    pub max_temp_bytes: u64,
    pub spill_partitions: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rows: 10_000_000,
            max_columns: 4_096,
            max_string_bytes: 16 * 1024 * 1024,
            max_regex_bytes: 4_096,
            max_split_columns: 256,
            max_memory_bytes: 512 * 1024 * 1024,
            max_temp_bytes: 8 * 1024 * 1024 * 1024,
            spill_partitions: 64,
        }
    }
}

pub mod aggregation;
pub mod analysis;
pub mod analyze;
pub mod cleansing;
pub mod columns;
pub mod dates;
pub mod expressions;
pub mod filtering;
pub mod formula;
pub mod fuzzy;
pub mod governance;
pub mod joins;
pub mod quality;
pub mod reshape;
pub mod security;
pub mod setops;
pub mod spill;
pub mod strings;
pub mod utility;

use std::cmp::Ordering;
use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
    UInt32Array, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use num_traits::ToPrimitive;

use plenora_core::{PlenoraError, Result};

/// Indice della colonna `name` nel batch.
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema.
pub fn column_index(batch: &RecordBatch, name: &str) -> Result<usize> {
    batch
        .schema()
        .index_of(name)
        .map_err(|_| PlenoraError::Schema(format!("colonna non trovata: {name}")))
}

/// Colonna `name` del batch come `StringArray` (Utf8).
///
/// # Errors
///
/// - `Schema`: colonna assente o non di tipo Utf8.
pub fn utf8_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a plenora_core::arrow::array::StringArray> {
    let index = column_index(batch, name)?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::StringArray>()
        .ok_or_else(|| PlenoraError::Schema(format!("la colonna {name} deve essere Utf8")))
}

/// Batch con la colonna `name` sostituita da `array` (o aggiunta in coda se
/// assente), preservando i metadati dello schema.
///
/// # Errors
///
/// - `Schema`: `array` ha un numero di righe diverso dal batch, oppure lo
///   schema risultante non e' coerente con le colonne.
pub fn replace_or_append(
    batch: &RecordBatch,
    name: &str,
    data_type: DataType,
    nullable: bool,
    array: ArrayRef,
) -> Result<RecordBatch> {
    if array.len() != batch.num_rows() {
        return Err(PlenoraError::Schema(format!(
            "lunghezza output {} diversa dalle righe {}",
            array.len(),
            batch.num_rows()
        )));
    }
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect();
    let mut columns = batch.columns().to_vec();
    if let Ok(index) = batch.schema().index_of(name) {
        fields[index] = Field::new(name, data_type, nullable);
        columns[index] = array;
    } else {
        fields.push(Field::new(name, data_type, nullable));
        columns.push(array);
    }
    let schema = Schema::new_with_metadata(fields, batch.schema().metadata().clone());
    Ok(RecordBatch::try_new(Arc::new(schema), columns)?)
}

/// Valida il nome di una colonna di output (non vuoto, <= 1024 byte).
///
/// # Errors
///
/// - `InvalidPlan`: nome vuoto (o solo spazi) oppure oltre 1024 byte.
pub fn validate_output_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "il nome della colonna di output e' vuoto".into(),
        ));
    }
    if name.len() > 1_024 {
        return Err(PlenoraError::InvalidPlan(
            "il nome della colonna supera 1024 byte".into(),
        ));
    }
    Ok(())
}

/// Valore scalare della riga come `String` (profilo scalare testuale).
/// `None` se la riga e' null.
///
/// # Errors
///
/// - `InvalidPlan`: epoch date32 non valida (guardia interna);
/// - `Schema`: valore date32/timestamp fuori intervallo, timezone Arrow non
///   valida, decimal128 incoerente o con scala non supportata, binary non
///   UTF-8, dictionary non Utf8, tipo non supportato dal profilo scalare.
pub fn scalar_as_string(array: &dyn Array, row: usize) -> Result<Option<String>> {
    if array.is_null(row) {
        return Ok(None);
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(Some(values.value(row).to_owned()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return Ok(Some(values.value(row).to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Date32Array>() {
        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
            .ok_or_else(|| PlenoraError::InvalidPlan("epoch date32 non valida".into()))?;
        let date = epoch
            .checked_add_signed(chrono::TimeDelta::days(i64::from(values.value(row))))
            .ok_or_else(|| PlenoraError::Schema("date32 fuori intervallo".into()))?;
        return Ok(Some(date.format("%Y-%m-%d").to_string()));
    }
    if let Some(values) = array.as_any().downcast_ref::<TimestampMillisecondArray>() {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(values.value(row))
            .ok_or_else(|| PlenoraError::Schema("timestamp fuori intervallo".into()))?;
        if let DataType::Timestamp(_, Some(timezone)) = values.data_type() {
            let timezone = timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| PlenoraError::Schema("timezone Arrow non valida".into()))?;
            return Ok(Some(timestamp.with_timezone(&timezone).to_rfc3339()));
        }
        return Ok(Some(timestamp.to_rfc3339()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        let value = values.value(row);
        let scale = u32::try_from(*scale)
            .map_err(|_| PlenoraError::Schema("scala decimal negativa non supportata".into()))?;
        let factor = 10_i128
            .checked_pow(scale)
            .ok_or_else(|| PlenoraError::Schema("scala decimal fuori intervallo".into()))?;
        let magnitude = value.unsigned_abs();
        let whole = magnitude / factor.unsigned_abs();
        let fraction = magnitude % factor.unsigned_abs();
        let sign = if value < 0 { "-" } else { "" };
        return if scale == 0 {
            Ok(Some(format!("{sign}{whole}")))
        } else {
            Ok(Some(format!(
                "{sign}{whole}.{fraction:0width$}",
                width = usize::try_from(scale).unwrap_or_default()
            )))
        };
    }
    if let Some(values) = array.as_any().downcast_ref::<BinaryArray>() {
        return std::str::from_utf8(values.value(row))
            .map(|value| Some(value.to_owned()))
            .map_err(|_| PlenoraError::Schema("binary non contiene UTF-8 valido".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>() {
        let dictionary = values
            .values()
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| PlenoraError::Schema("dictionary non contiene Utf8".into()))?;
        let key = usize::try_from(values.keys().value(row))
            .map_err(|_| PlenoraError::Schema("chiave dictionary negativa".into()))?;
        return Ok(Some(dictionary.value(key).to_owned()));
    }
    Err(PlenoraError::Schema(format!(
        "tipo {:?} non supportato dal profilo scalare",
        array.data_type()
    )))
}

/// Valore scalare della riga come `f64`. `None` se la riga e' null.
///
/// # Errors
///
/// - `Schema`: intero/timestamp/decimal128 non rappresentabile come f64,
///   decimal128 incoerente, testo non convertibile in numero, tipo non
///   convertibile in numero.
pub fn scalar_as_f64(array: &dyn Array, row: usize) -> Result<Option<f64>> {
    if array.is_null(row) {
        return Ok(None);
    }
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return Ok(Some(values.value(row)));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return values
            .value(row)
            .to_f64()
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("intero non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return values
            .value(row)
            .to_f64()
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("uint64 non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Date32Array>() {
        return Ok(Some(f64::from(values.value(row))));
    }
    if let Some(values) = array.as_any().downcast_ref::<TimestampMillisecondArray>() {
        return values
            .value(row)
            .to_f64()
            .map(Some)
            .ok_or_else(|| PlenoraError::Schema("timestamp non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<Decimal128Array>() {
        let DataType::Decimal128(_, scale) = values.data_type() else {
            return Err(PlenoraError::Schema("decimal128 incoerente".into()));
        };
        let factor = 10_f64.powi(i32::from(*scale));
        return values
            .value(row)
            .to_f64()
            .map(|value| Some(value / factor))
            .ok_or_else(|| PlenoraError::Schema("decimal128 non rappresentabile come f64".into()));
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return values
            .value(row)
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .map(Some)
            .map_err(|_| PlenoraError::Schema("valore non convertibile in numero".into()));
    }
    Err(PlenoraError::Schema(format!(
        "tipo {:?} non convertibile in numero",
        array.data_type()
    )))
}

// ---------------------------------------------------------------------------
// Confronti scalari tipizzati (filtri, regole di governance, assert_range).
//
// Classe di bug chiusa (review 2026-07-27, stessa classe dei comparatori di
// `table.sort`): i confronti fatti via `scalar_as_f64` collassano interi
// distinti oltre 2^53 sullo stesso double (9007199254740992 e
// 9007199254740993 risultavano uguali) e il confronto testuale disordinava
// gli UInt64 ("10" < "9"). Il predicato condiviso qui sotto e' esatto per
// costruzione: nessuna conversione a f64 quando un lato e' un intero.
//
// Regola mista interi <-> float (decisione documentata): il valore di
// configurazione e' un letterale JSON reso testo; un letterale INTERO resta
// un intero esatto (`I64`, poi `U64`), ogni altra forma numerica
// (frazionaria, esponenziale, inf, NaN) e' `F64`. Il confronto intero <-> F64
// e' esatto: un double frazionario non e' mai uguale a un intero e ordina
// per floor; un double intero fuori gamma ordina per segno (2^63 > ogni
// i64); NaN rende falso ogni confronto (`None`), come in IEEE 754. Quindi
// 9007199254740993 (intero) > 9007199254740992.0 (double), mentre un
// letterale frazionario JSON (es. 9007199254740993.0) e' un double gia'
// arrotondato in deserializzazione e vale come tale.
// ---------------------------------------------------------------------------

/// Estremo di un confronto scalare, parsato dal valore di configurazione.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumericBound {
    /// Letterale intero in gamma i64: confronto nativo esatto.
    I64(i64),
    /// Letterale intero oltre `i64::MAX` (gamma u64): confronto nativo esatto.
    U64(u64),
    /// Qualunque altra forma numerica (frazionaria, esponenziale, inf, NaN).
    F64(f64),
}

impl NumericBound {
    /// Parse del valore atteso: intero esatto se il testo e' un letterale
    /// intero, altrimenti f64. `None` se il testo non e' numerico: il
    /// chiamante lo traduce nello stesso errore di contratto del percorso
    /// storico (che parsava solo f64, un sottoinsieme stretto di questi casi).
    pub fn parse(text: &str) -> Option<Self> {
        if let Ok(value) = text.parse::<i64>() {
            return Some(Self::I64(value));
        }
        if let Ok(value) = text.parse::<u64>() {
            return Some(Self::U64(value));
        }
        text.parse::<f64>().ok().map(Self::F64)
    }
}

/// Confronto esatto i64 <-> bound. `None` solo con bound NaN (ogni confronto
/// falso, come IEEE): i chiamanti lo trattano come "confronto non soddisfatto".
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
// I cast intero<->intero sono guardati dai rami precedenti (gamma verificata).
#[must_use]
pub fn compare_i64(actual: i64, bound: NumericBound) -> Option<Ordering> {
    match bound {
        NumericBound::I64(expected) => Some(actual.cmp(&expected)),
        NumericBound::U64(expected) => Some(if expected > i64::MAX as u64 {
            Ordering::Less // ogni i64 e' minore di un u64 oltre i64::MAX
        } else {
            actual.cmp(&(expected as i64))
        }),
        NumericBound::F64(expected) => compare_i64_f64(actual, expected),
    }
}

/// Confronto esatto u64 <-> bound. `None` solo con bound NaN.
#[allow(clippy::cast_sign_loss)] // cast i64->u64 guardato dal ramo `expected < 0`
#[must_use]
pub fn compare_u64(actual: u64, bound: NumericBound) -> Option<Ordering> {
    match bound {
        NumericBound::U64(expected) => Some(actual.cmp(&expected)),
        NumericBound::I64(expected) => Some(if expected < 0 {
            Ordering::Greater // ogni u64 e' maggiore di un intero negativo
        } else {
            actual.cmp(&(expected as u64))
        }),
        NumericBound::F64(expected) => compare_u64_f64(actual, expected),
    }
}

/// Confronto esatto f64 <-> bound, duale di `compare_i64`/`compare_u64`.
///
/// Usato per colonne Float64 contro letterali interi di configurazione:
/// entro 2^53 coincide col confronto IEEE storico, oltre resta esatto.
/// Con bound `F64` vale la semantica IEEE (`partial_cmp`: NaN -> `None`).
pub fn compare_f64(actual: f64, bound: NumericBound) -> Option<Ordering> {
    match bound {
        NumericBound::I64(expected) => compare_i64_f64(expected, actual).map(Ordering::reverse),
        NumericBound::U64(expected) => compare_u64_f64(expected, actual).map(Ordering::reverse),
        NumericBound::F64(expected) => actual.partial_cmp(&expected),
    }
}

#[allow(clippy::float_cmp, clippy::cast_possible_truncation)]
// I confronti con inf e i cast f64->i64 sono esatti per costruzione: i rami
// sopra garantiscono finitezza, gamma e (dove richiesto) integrita'.
fn compare_i64_f64(actual: i64, expected: f64) -> Option<Ordering> {
    if expected.is_nan() {
        return None;
    }
    if expected == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if expected == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    // Oltre 2^63 (in valore assoluto) il double e' certamente intero (non
    // esistono doppi frazionari oltre 2^52) e fuori gamma i64: ordina per segno.
    if expected >= 9_223_372_036_854_775_808.0 {
        return Some(Ordering::Less);
    }
    if expected < -9_223_372_036_854_775_808.0 {
        return Some(Ordering::Greater);
    }
    if expected.fract() == 0.0 {
        // Double intero in gamma i64 (2^63 negativo incluso): cast esatto.
        return Some(actual.cmp(&(expected as i64)));
    }
    // Double frazionario (qui |expected| < 2^52, floor esatto in i64): mai
    // uguale a un intero, ordina per floor.
    let floor = expected.floor() as i64;
    Some(if actual <= floor {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}

// Come `compare_i64_f64`: guardie di finitezza, segno, gamma e integrita'.
// I cast f64 -> u64 nel corpo sono esatti per costruzione: NaN e infiniti
// sono esclusi dalle guardie iniziali, il segno negativo dalla guardia
// `expected < 0.0`, l'overflow dalla guardia `expected >= 2^64`.
#[allow(clippy::float_cmp, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn compare_u64_f64(actual: u64, expected: f64) -> Option<Ordering> {
    if expected.is_nan() {
        return None;
    }
    if expected == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if expected == f64::NEG_INFINITY || expected < 0.0 {
        return Some(Ordering::Greater);
    }
    if expected >= 18_446_744_073_709_551_616.0 {
        return Some(Ordering::Less);
    }
    if expected.fract() == 0.0 {
        // Double intero in [0, 2^64): cast esatto.
        return Some(actual.cmp(&(expected as u64)));
    }
    let floor = expected.floor() as u64;
    Some(if actual <= floor {
        Ordering::Less
    } else {
        Ordering::Greater
    })
}

/// Batch con le sole righe indicate, nell'ordine dato.
///
/// # Errors
///
/// - `InvalidPlan`: indice di riga oltre `u32::MAX`;
/// - `Schema`: errore Arrow nella `take` o nella costruzione del batch.
pub fn select_rows(batch: &RecordBatch, rows: &[usize]) -> Result<RecordBatch> {
    let indices: UInt32Array = rows
        .iter()
        .map(|row| {
            u32::try_from(*row).map_err(|_| PlenoraError::InvalidPlan("indice riga oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?
        .into();
    let columns = batch
        .columns()
        .iter()
        .map(|column| {
            plenora_core::arrow::select::take::take(column.as_ref(), &indices, None).map_err(PlenoraError::from)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RecordBatch::try_new(batch.schema(), columns)?)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{Int64Array, StringArray};
    use plenora_core::arrow::schema::{DataType, Field, Schema};

    use super::*;

    fn batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("a", DataType::Utf8, true)])),
            vec![Arc::new(StringArray::from(vec![Some("x"), None]))],
        )
        .expect("fixture")
    }

    #[test]
    fn helper_guards_cover_type_length_and_names() {
        let input = batch();
        assert!(replace_or_append(
            &input,
            "bad",
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(vec![Some("only one")]))
        )
        .is_err());
        assert!(validate_output_name(" ").is_err());
        assert!(validate_output_name(&"x".repeat(1_025)).is_err());
        assert!(column_index(&input, "missing").is_err());

        let integers = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1]))],
        )
        .expect("integers");
        assert!(utf8_column(&integers, "n").is_err());
    }

    #[test]
    fn numeric_bound_parse_prefers_exact_integers() {
        assert_eq!(NumericBound::parse("42"), Some(NumericBound::I64(42)));
        assert_eq!(NumericBound::parse("-7"), Some(NumericBound::I64(-7)));
        assert_eq!(
            NumericBound::parse("9007199254740993"),
            Some(NumericBound::I64(9_007_199_254_740_993))
        );
        assert_eq!(
            NumericBound::parse("18446744073709551615"),
            Some(NumericBound::U64(u64::MAX))
        );
        assert_eq!(
            NumericBound::parse("9223372036854775808"),
            Some(NumericBound::U64(9_223_372_036_854_775_808))
        );
        assert_eq!(NumericBound::parse("1.5"), Some(NumericBound::F64(1.5)));
        assert_eq!(NumericBound::parse("1e3"), Some(NumericBound::F64(1_000.0)));
        assert_eq!(NumericBound::parse("64.0"), Some(NumericBound::F64(64.0)));
        assert!(NumericBound::parse("x").is_none());
        assert!(NumericBound::parse("").is_none());
    }

    #[test]
    fn compare_i64_is_exact_beyond_2_pow_53() {
        let lo = 9_007_199_254_740_992_i64; // 2^53
        let hi = 9_007_199_254_740_993_i64; // 2^53 + 1: stesso double di lo
        assert_eq!(compare_i64(hi, NumericBound::I64(lo)), Some(Ordering::Greater));
        assert_eq!(compare_i64(lo, NumericBound::I64(hi)), Some(Ordering::Less));
        assert_eq!(compare_i64(hi, NumericBound::I64(hi)), Some(Ordering::Equal));
        // Bound f64: lo e hi collassano sullo stesso double, il confronto
        // resta esatto.
        let collapsed = NumericBound::F64(9_007_199_254_740_992.0);
        assert_eq!(compare_i64(lo, collapsed), Some(Ordering::Equal));
        assert_eq!(compare_i64(hi, collapsed), Some(Ordering::Greater));
        assert_eq!(compare_i64(-hi, collapsed), Some(Ordering::Less));
    }

    #[test]
    fn compare_i64_mixed_covers_fraction_inf_nan_and_ranges() {
        assert_eq!(compare_i64(5, NumericBound::F64(5.5)), Some(Ordering::Less));
        assert_eq!(compare_i64(5, NumericBound::F64(4.5)), Some(Ordering::Greater));
        assert_eq!(compare_i64(-5, NumericBound::F64(-5.5)), Some(Ordering::Greater));
        assert_eq!(compare_i64(-6, NumericBound::F64(-5.5)), Some(Ordering::Less));
        assert_eq!(compare_i64(0, NumericBound::F64(-0.0)), Some(Ordering::Equal));
        assert_eq!(compare_i64(i64::MAX, NumericBound::F64(f64::INFINITY)), Some(Ordering::Less));
        assert_eq!(compare_i64(i64::MIN, NumericBound::F64(f64::NEG_INFINITY)), Some(Ordering::Greater));
        assert_eq!(compare_i64(0, NumericBound::F64(f64::NAN)), None);
        assert_eq!(compare_i64(i64::MAX, NumericBound::F64(1e30)), Some(Ordering::Less));
        assert_eq!(compare_i64(i64::MIN, NumericBound::F64(-1e30)), Some(Ordering::Greater));
        // -2^63 e' un double intero esatto in gamma i64.
        assert_eq!(
            compare_i64(i64::MIN, NumericBound::F64(-9_223_372_036_854_775_808.0)),
            Some(Ordering::Equal)
        );
        // Bound u64 oltre i64::MAX: maggiore di ogni i64.
        assert_eq!(
            compare_i64(i64::MAX, NumericBound::U64(9_223_372_036_854_775_808)),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_i64(-1, NumericBound::U64(u64::MAX)),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn compare_u64_orders_natively_not_textually() {
        assert_eq!(compare_u64(10, NumericBound::U64(9)), Some(Ordering::Greater));
        assert_eq!(compare_u64(9, NumericBound::U64(10)), Some(Ordering::Less));
        assert_eq!(compare_u64(u64::MAX, NumericBound::U64(u64::MAX)), Some(Ordering::Equal));
        assert_eq!(compare_u64(0, NumericBound::I64(-1)), Some(Ordering::Greater));
        assert_eq!(compare_u64(10, NumericBound::I64(9)), Some(Ordering::Greater));
        // Bound f64 oltre 2^53: 2^64 e' maggiore di ogni u64.
        let top = NumericBound::F64(18_446_744_073_709_551_616.0); // 2^64
        assert_eq!(compare_u64(u64::MAX, top), Some(Ordering::Less));
        assert_eq!(compare_u64(0, NumericBound::F64(0.5)), Some(Ordering::Less));
        assert_eq!(compare_u64(1, NumericBound::F64(0.5)), Some(Ordering::Greater));
        assert_eq!(compare_u64(0, NumericBound::F64(f64::NAN)), None);
    }

    #[test]
    fn compare_f64_is_the_exact_dual_for_float_columns() {
        // Letterale intero oltre 2^53 contro colonna Float64: il double
        // 9007199254740992.0 e' minore dell'intero 9007199254740993.
        assert_eq!(
            compare_f64(9_007_199_254_740_992.0, NumericBound::I64(9_007_199_254_740_993)),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_f64(9_007_199_254_740_992.0, NumericBound::I64(9_007_199_254_740_992)),
            Some(Ordering::Equal)
        );
        assert_eq!(compare_f64(f64::NAN, NumericBound::I64(1)), None);
        assert_eq!(
            compare_f64(1.5, NumericBound::F64(1.5)),
            Some(Ordering::Equal)
        );
    }
}
