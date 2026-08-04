use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use plenora_core::arrow::array::{
    new_null_array, Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    Float64Array, Int64Array, RecordBatch, RecordBatchOptions, StringArray,
    TimestampMillisecondArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};
use serde::Deserialize;
use serde_json::Value;

use crate::Limits;
use plenora_core::{PlenoraError, Result};

use super::{replace_or_append, utf8_column, validate_output_name};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropColumns {
    pub columns: Vec<String>,
}

/// Batch senza le colonne elencate in `config`.
///
/// I nomi assenti nello schema sono ignorati (no-op); metadati di schema
/// e numero di righe sono preservati. Zero-copy: gli array Arrow sono
/// riusati (Arc clone).
///
/// # Errors
///
/// - `Arrow`: lo schema risultante non e' coerente con le colonne
///   (guardia Arrow in costruzione del batch).
pub fn drop_columns(batch: &RecordBatch, config: &DropColumns) -> Result<RecordBatch> {
    let removed: HashSet<&str> = config.columns.iter().map(String::as_str).collect();
    let mut fields = Vec::new();
    let mut columns = Vec::new();
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        if !removed.contains(field.name().as_str()) {
            fields.push(field.as_ref().clone());
            columns.push(Arc::clone(column));
        }
    }
    let schema = Arc::new(Schema::new_with_metadata(
        fields,
        batch.schema().metadata().clone(),
    ));
    let options = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
    Ok(RecordBatch::try_new_with_options(
        schema, columns, &options,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectColumns {
    pub columns: Vec<String>,
}

/// Proiezione positiva (estensione v1.1): output = le colonne elencate,
/// nell'ordine dato.
///
/// Zero-copy: gli array Arrow sono riusati (Arc clone), nessuna copia
/// dei dati.
///
/// # Errors
///
/// - `InvalidPlan`: elenco `columns` vuoto o colonna ripetuta nella
///   proiezione;
/// - `Schema`: colonna non trovata nello schema;
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna).
pub fn select_columns(batch: &RecordBatch, config: &SelectColumns) -> Result<RecordBatch> {
    if config.columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "select_columns richiede almeno una colonna".into(),
        ));
    }
    let schema = batch.schema();
    let mut seen = HashSet::new();
    let mut fields = Vec::with_capacity(config.columns.len());
    let mut columns = Vec::with_capacity(config.columns.len());
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return Err(PlenoraError::InvalidPlan(format!(
                "colonna ripetuta nella proiezione: {name}"
            )));
        }
        let index = schema
            .index_of(name)
            .map_err(|_| PlenoraError::Schema(format!("colonna non trovata: {name}")))?;
        fields.push(schema.field(index).clone());
        columns.push(Arc::clone(batch.column(index)));
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        columns,
    )?)
}

// ---------------------------------------------------------------------------
// table.align_schema (estensione v1.2)
// ---------------------------------------------------------------------------

/// Tipo dichiarato di `align_schema`: set chiuso di nomi, nessuna sintassi
/// libera. Il mapping su `DataType` e' fisso e documentato in
/// [`AlignType::data_type`]: mai cast implicito tra tipi diversi.
#[derive(Debug, Clone, Copy, Deserialize)]
pub enum AlignType {
    Utf8,
    Int64,
    UInt64,
    Float64,
    Boolean,
    Date32,
    Timestamp,
    Decimal128,
    Binary,
}

impl AlignType {
    /// `DataType` Arrow corrispondente: `Timestamp` = millisecondi senza
    /// timezone (coerente col profilo scalare), `Decimal128` = precisione 38,
    /// scala 10.
    #[must_use]
    pub const fn data_type(self) -> DataType {
        match self {
            Self::Utf8 => DataType::Utf8,
            Self::Int64 => DataType::Int64,
            Self::UInt64 => DataType::UInt64,
            Self::Float64 => DataType::Float64,
            Self::Boolean => DataType::Boolean,
            Self::Date32 => DataType::Date32,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Millisecond, None),
            Self::Decimal128 => DataType::Decimal128(38, 10),
            Self::Binary => DataType::Binary,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub align_type: AlignType,
    /// Valore scalare per una colonna assente in input (default: colonna di
    /// null). Ignorato se la colonna esiste gia'.
    #[serde(default)]
    pub default: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignSchema {
    pub columns: Vec<AlignColumn>,
    /// Colonne di input non elencate: scartate (default) o preservate in coda
    /// nell'ordine originale.
    #[serde(default)]
    pub keep_extra: bool,
}

/// Precisione/scala fisse del mapping `AlignType::Decimal128`.
const ALIGN_DECIMAL_PRECISION: u8 = 38;
const ALIGN_DECIMAL_SCALE: i8 = 10;

/// Parsing `Decimal128(38, 10)` di un letterale testuale: segno opzionale,
/// parte intera e frazionaria solo cifre, al massimo 10 decimali (nessun
/// arrotondamento: piu' cifre della scala -> errore).
fn parse_align_decimal(text: &str) -> Option<i128> {
    let negative = text.starts_with('-');
    let digits = text.trim_start_matches(['-', '+']);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > usize::from(ALIGN_DECIMAL_SCALE.cast_unsigned())
    {
        return None;
    }
    let whole: i128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let scale = 10_i128.checked_pow(u32::try_from(ALIGN_DECIMAL_SCALE).ok()?)?;
    let mut fraction_value = 0_i128;
    for byte in fraction.bytes() {
        fraction_value = fraction_value
            .checked_mul(10)?
            .checked_add(i128::from(byte - b'0'))?;
    }
    for _ in fraction.len()..usize::from(ALIGN_DECIMAL_SCALE.cast_unsigned()) {
        fraction_value = fraction_value.checked_mul(10)?;
    }
    let scaled = whole.checked_mul(scale)?.checked_add(fraction_value)?;
    Some(if negative { -scaled } else { scaled })
}

/// Materializza una colonna costante di `rows` righe dal `default` JSON:
/// stessa conversione validata da [`check_align_default`] (fail-closed in
/// validazione, mai a meta' dei dati).
fn align_default_column(value: &Value, align_type: AlignType, rows: usize) -> Result<ArrayRef> {
    let invalid = || {
        PlenoraError::InvalidPlan(format!(
            "align_schema: default {value} non convertibile in {align_type:?}"
        ))
    };
    let string = || match value {
        Value::String(text) => Ok(text.clone()),
        _ => Err(invalid()),
    };
    let array: ArrayRef = match align_type {
        AlignType::Utf8 => Arc::new(StringArray::from(vec![string()?; rows])),
        AlignType::Int64 => {
            let parsed = match value {
                Value::Number(number) => number.as_i64(),
                Value::String(text) => text.trim().parse().ok(),
                _ => None,
            }
            .ok_or_else(invalid)?;
            Arc::new(Int64Array::from(vec![parsed; rows]))
        }
        AlignType::UInt64 => {
            let parsed = match value {
                Value::Number(number) => number.as_u64(),
                Value::String(text) => text.trim().parse().ok(),
                _ => None,
            }
            .ok_or_else(invalid)?;
            Arc::new(UInt64Array::from(vec![parsed; rows]))
        }
        AlignType::Float64 => {
            let parsed = match value {
                Value::Number(number) => number.as_f64(),
                Value::String(text) => text.trim().replace(',', ".").parse().ok(),
                _ => None,
            }
            .ok_or_else(invalid)?;
            Arc::new(Float64Array::from(vec![parsed; rows]))
        }
        AlignType::Boolean => {
            let parsed = match value {
                Value::Bool(flag) => Some(*flag),
                Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
                Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
                _ => None,
            }
            .ok_or_else(invalid)?;
            Arc::new(BooleanArray::from(vec![parsed; rows]))
        }
        AlignType::Date32 => {
            let text = string()?;
            let date = chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
                .map_err(|_| invalid())?;
            let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .ok_or_else(|| PlenoraError::InvalidPlan("epoch date32 non valida".into()))?;
            let days = i32::try_from((date - epoch).num_days()).map_err(|_| invalid())?;
            Arc::new(Date32Array::from(vec![days; rows]))
        }
        AlignType::Timestamp => {
            let text = string()?;
            let timestamp =
                chrono::DateTime::parse_from_rfc3339(text.trim()).map_err(|_| invalid())?;
            Arc::new(TimestampMillisecondArray::from(vec![
                timestamp
                    .timestamp_millis();
                rows
            ]))
        }
        AlignType::Decimal128 => {
            let text = match value {
                Value::Number(number) => number.to_string(),
                Value::String(text) => text.trim().to_owned(),
                _ => return Err(invalid()),
            };
            let parsed = parse_align_decimal(&text).ok_or_else(invalid)?;
            Arc::new(
                Decimal128Array::from(vec![parsed; rows])
                    .with_precision_and_scale(ALIGN_DECIMAL_PRECISION, ALIGN_DECIMAL_SCALE)
                    .map_err(PlenoraError::from)?,
            )
        }
        AlignType::Binary => {
            let text = string()?;
            Arc::new(BinaryArray::from(vec![text.as_bytes(); rows]))
        }
    };
    debug_assert_eq!(array.data_type(), &align_type.data_type());
    Ok(array)
}

/// Valida il `default` di una colonna dichiarata senza materializzarlo
/// (per l'analisi a secco del contratto).
///
/// # Errors
///
/// - `InvalidPlan`: `default` non convertibile in `align_type` (stessa
///   conversione di `align_default_column`, guardia epoch date32
///   inclusa);
/// - `Arrow`: errore Arrow sulla precisione/scala `Decimal128`.
pub fn check_align_default(value: &Value, align_type: AlignType) -> Result<()> {
    align_default_column(value, align_type, 1).map(|_| ())
}

/// Allinea lo schema dell'input all'elenco dichiarato (estensione v1.2).
///
/// Riordina/proietta secondo `columns`; una colonna assente e' aggiunta
/// come colonna di null (o riempita col `default` scalare, non nullable);
/// una colonna presente con tipo diverso dal dichiarato e' un errore di
/// contratto (mai cast implicito). Le colonne non elencate sono scartate,
/// salvo `keep_extra` (appendono in coda nell'ordine originale).
///
/// # Errors
///
/// - `InvalidPlan`: elenco `columns` vuoto, colonna ripetuta, nome di
///   output non valido (come `validate_output_name`), tipo presente
///   diverso dal dichiarato (nessun cast implicito) o `default` non
///   convertibile (come `check_align_default`);
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna)
///   o sulla precisione/scala `Decimal128`.
pub fn align_schema(batch: &RecordBatch, config: &AlignSchema) -> Result<RecordBatch> {
    if config.columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "align_schema richiede almeno una colonna".into(),
        ));
    }
    let schema = batch.schema();
    let mut seen = HashSet::new();
    let mut fields = Vec::with_capacity(config.columns.len());
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(config.columns.len());
    for declared in &config.columns {
        validate_output_name(&declared.name)?;
        if !seen.insert(declared.name.as_str()) {
            return Err(PlenoraError::InvalidPlan(format!(
                "align_schema: colonna ripetuta: {}",
                declared.name
            )));
        }
        let data_type = declared.align_type.data_type();
        if let Ok(index) = schema.index_of(&declared.name) {
            let field = schema.field(index);
            if field.data_type() != &data_type {
                return Err(PlenoraError::InvalidPlan(format!(
                    "align_schema: colonna {} di tipo {:?}, atteso {:?} (nessun cast implicito)",
                    declared.name,
                    field.data_type(),
                    data_type
                )));
            }
            fields.push(field.clone());
            columns.push(Arc::clone(batch.column(index)));
        } else if let Some(default) = &declared.default {
            columns.push(align_default_column(
                default,
                declared.align_type,
                batch.num_rows(),
            )?);
            fields.push(Field::new(&declared.name, data_type, false));
        } else {
            fields.push(Field::new(&declared.name, data_type.clone(), true));
            columns.push(new_null_array(&data_type, batch.num_rows()));
        }
    }
    if config.keep_extra {
        for (field, column) in schema.fields().iter().zip(batch.columns()) {
            if !seen.contains(field.name().as_str()) {
                fields.push(field.as_ref().clone());
                columns.push(Arc::clone(column));
            }
        }
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenamePair {
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
    pub renames: Vec<RenamePair>,
}

/// Rinomina le colonne secondo `renames` (i nomi non mappati restano
/// invariati), preservando metadati di schema e array: zero-copy.
///
/// # Errors
///
/// - `InvalidPlan`: nome di output non valido (come
///   `validate_output_name`);
/// - `Schema`: la rinomina produce un nome duplicato;
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna).
pub fn rename(batch: &RecordBatch, config: &Rename) -> Result<RecordBatch> {
    let mapping: HashMap<&str, &str> = config
        .renames
        .iter()
        .map(|item| (item.old_name.as_str(), item.new_name.as_str()))
        .collect();
    let mut names = HashSet::new();
    let mut fields = Vec::with_capacity(batch.num_columns());
    for field in batch.schema().fields() {
        let name = mapping
            .get(field.name().as_str())
            .copied()
            .unwrap_or(field.name());
        validate_output_name(name)?;
        if !names.insert(name.to_owned()) {
            return Err(PlenoraError::Schema(format!(
                "rename produce il nome duplicato: {name}"
            )));
        }
        fields.push(field.as_ref().clone().with_name(name));
    }
    let schema = Schema::new_with_metadata(fields, batch.schema().metadata().clone());
    Ok(RecordBatch::try_new(
        Arc::new(schema),
        batch.columns().to_vec(),
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorderColumns {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default, alias = "sort_alphabetical")]
    pub alphabetical: bool,
}

/// Riordina le colonne del batch.
///
/// Prima quelle elencate in `columns` (nell'ordine dato), poi le restanti
/// nell'ordine originale (alfabetico, case-insensitive, con
/// `alphabetical`). Zero-copy: gli array Arrow sono riusati (Arc clone).
///
/// # Errors
///
/// - `InvalidPlan`: colonna ripetuta nell'elenco;
/// - `Schema`: colonna non trovata nello schema;
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna).
pub fn reorder_columns(batch: &RecordBatch, config: &ReorderColumns) -> Result<RecordBatch> {
    let schema = batch.schema();
    let mut selected = Vec::with_capacity(batch.num_columns());
    let mut seen = HashSet::new();
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return Err(PlenoraError::InvalidPlan(format!(
                "colonna ripetuta nel riordino: {name}"
            )));
        }
        let index = schema
            .index_of(name)
            .map_err(|_| PlenoraError::Schema(format!("colonna non trovata: {name}")))?;
        selected.push(index);
    }
    let mut remaining: Vec<usize> = (0..batch.num_columns())
        .filter(|index| !selected.contains(index))
        .collect();
    if config.alphabetical {
        remaining.sort_by_key(|index| schema.field(*index).name().to_lowercase());
    }
    selected.extend(remaining);
    let fields: Vec<Field> = selected
        .iter()
        .map(|index| schema.field(*index).clone())
        .collect();
    let columns: Vec<ArrayRef> = selected
        .iter()
        .map(|index| Arc::clone(batch.column(*index)))
        .collect();
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcatColumns {
    pub columns: Vec<String>,
    #[serde(default = "default_concat_output")]
    pub output_column: String,
    #[serde(default = "default_separator")]
    pub separator: String,
    #[serde(default = "default_true")]
    pub skip_null: bool,
}

fn default_concat_output() -> String {
    "concatenated".into()
}
fn default_separator() -> String {
    " ".into()
}
const fn default_true() -> bool {
    true
}

/// Concatena le colonne Utf8 elencate nella colonna `output_column`.
///
/// Le parti sono unite con `separator`; con `skip_null` i null sono
/// saltati (riga di soli null -> null), altrimenti contano come stringa
/// vuota. Se `output_column` esiste gia' e' sostituita.
///
/// # Errors
///
/// - `InvalidPlan`: elenco `columns` vuoto, nome di output non valido
///   (come `validate_output_name`) o valore concatenato oltre
///   `limits.max_string_bytes`;
/// - `Schema`: colonna assente o non Utf8 (come `utf8_column`);
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna
///   di `replace_or_append`).
pub fn concat_columns(
    batch: &RecordBatch,
    config: &ConcatColumns,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "concat_columns richiede almeno una colonna".into(),
        ));
    }
    validate_output_name(&config.output_column)?;
    let arrays = config
        .columns
        .iter()
        .map(|name| utf8_column(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let mut output = Vec::with_capacity(batch.num_rows());
    // String di lavoro riusata fra le righe (V2): stessi byte di
    // `parts.join(separator)` — separatore solo FRA le parti incluse —
    // senza il `Vec` di parti e il join allocati a ogni riga.
    let mut joined = String::new();
    for row in 0..batch.num_rows() {
        joined.clear();
        let mut included = 0_usize;
        for array in &arrays {
            if array.is_null(row) && config.skip_null {
                continue;
            }
            if included > 0 {
                joined.push_str(&config.separator);
            }
            if !array.is_null(row) {
                joined.push_str(array.value(row));
            }
            included += 1;
        }
        if config.skip_null && included == 0 {
            output.push(None);
        } else if joined.len() > limits.max_string_bytes {
            return Err(PlenoraError::InvalidPlan(
                "concat_columns supera max_string_bytes".into(),
            ));
        } else {
            output.push(Some(joined.clone()));
        }
    }
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(output)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitColumn {
    pub column: String,
    #[serde(default = "default_delimiter")]
    pub delimiter: String,
    pub new_columns: Vec<String>,
    #[serde(default = "default_max_splits")]
    pub max_splits: i64,
}

fn default_delimiter() -> String {
    ",".into()
}
const fn default_max_splits() -> i64 {
    -1
}

/// Divide la colonna Utf8 `column` sul `delimiter` nelle `new_columns`.
///
/// Al piu' `max_splits` parti se positivo: le parti mancanti sono null,
/// quelle in eccesso restano nell'ultima parte. Una colonna di output
/// gia' esistente e' sostituita.
///
/// # Errors
///
/// - `InvalidPlan`: `delimiter` vuoto, `new_columns` vuoto o oltre
///   `limits.max_split_columns`, nome di output non valido (come
///   `validate_output_name`);
/// - `Schema`: nomi di output duplicati, oppure colonna assente o non
///   Utf8 (come `utf8_column`);
/// - `Arrow`: errore Arrow nella costruzione del batch (guardia interna
///   di `replace_or_append`).
pub fn split_column(
    batch: &RecordBatch,
    config: &SplitColumn,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.delimiter.is_empty() {
        return Err(PlenoraError::InvalidPlan("delimiter vuoto".into()));
    }
    if config.new_columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "new_columns e' obbligatorio nel percorso streaming".into(),
        ));
    }
    if config.new_columns.len() > limits.max_split_columns {
        return Err(PlenoraError::InvalidPlan(
            "split_column supera max_split_columns".into(),
        ));
    }
    let unique: HashSet<_> = config.new_columns.iter().collect();
    if unique.len() != config.new_columns.len() {
        return Err(PlenoraError::Schema(
            "split_column contiene nomi output duplicati".into(),
        ));
    }
    for name in &config.new_columns {
        validate_output_name(name)?;
    }
    let input = utf8_column(batch, &config.column)?;
    let requested_parts = config.new_columns.len();
    let split_limit = if config.max_splits > 0 {
        usize::try_from(config.max_splits)
            .unwrap_or(usize::MAX)
            .saturating_add(1)
            .min(requested_parts)
    } else {
        requested_parts
    };
    let mut outputs = vec![Vec::<Option<String>>::with_capacity(batch.num_rows()); requested_parts];
    for row in 0..batch.num_rows() {
        if input.is_null(row) {
            for output in &mut outputs {
                output.push(None);
            }
            continue;
        }
        let parts: Vec<&str> = input
            .value(row)
            .splitn(split_limit, &config.delimiter)
            .collect();
        for (index, output) in outputs.iter_mut().enumerate() {
            output.push(parts.get(index).map(|value| (*value).to_owned()));
        }
    }
    let mut result = batch.clone();
    for (name, output) in config.new_columns.iter().zip(outputs) {
        result = replace_or_append(
            &result,
            name,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(output)),
        )?;
    }
    Ok(result)
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use serde_json::json;

    use super::*;

    fn batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("z", DataType::Utf8, true),
                Field::new("a", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x,y,z"), None])),
                Arc::new(StringArray::from(vec![Some("1"), Some("2")])),
            ],
        )
        .expect("fixture")
    }

    #[test]
    fn defaults_and_non_destructive_paths_work() {
        let input = batch();
        let dropped = drop_columns(
            &input,
            &DropColumns {
                columns: vec!["missing".into()],
            },
        )
        .expect("unknown drop is no-op");
        assert_eq!(dropped.num_columns(), 2);

        let renamed = rename(&input, &Rename { renames: vec![] }).expect("empty rename");
        assert_eq!(renamed.schema(), input.schema());

        let reorder: ReorderColumns = serde_json::from_value(json!({})).expect("defaults");
        assert!(!reorder.alphabetical);
        assert_eq!(
            reorder_columns(&input, &reorder)
                .expect("no-op")
                .num_columns(),
            2
        );

        let concat: ConcatColumns =
            serde_json::from_value(json!({"columns": ["a"]})).expect("defaults");
        assert_eq!(concat.output_column, "concatenated");
        assert_eq!(concat.separator, " ");
        assert!(concat.skip_null);

        let split: SplitColumn =
            serde_json::from_value(json!({"column": "z", "new_columns": ["one", "two", "three"]}))
                .expect("defaults");
        assert_eq!(split.delimiter, ",");
        assert_eq!(split.max_splits, -1);
        let output = split_column(&input, &split, &Limits::default()).expect("unbounded split");
        assert_eq!(output.num_columns(), 5);
    }

    #[test]
    fn runtime_guards_remain_defensive() {
        let input = batch();
        let limits = Limits::default();
        assert!(concat_columns(
            &input,
            &ConcatColumns {
                columns: vec![],
                output_column: "x".into(),
                separator: String::new(),
                skip_null: true,
            },
            &limits,
        )
        .is_err());
        assert!(reorder_columns(
            &input,
            &ReorderColumns {
                columns: vec!["a".into(), "a".into()],
                alphabetical: false,
            },
        )
        .is_err());

        let base = SplitColumn {
            column: "z".into(),
            delimiter: ",".into(),
            new_columns: vec!["x".into()],
            max_splits: -1,
        };
        let empty_delimiter = SplitColumn {
            delimiter: String::new(),
            ..base
        };
        assert!(split_column(&input, &empty_delimiter, &limits).is_err());
        let empty_outputs = SplitColumn {
            column: "z".into(),
            delimiter: ",".into(),
            new_columns: vec![],
            max_splits: -1,
        };
        assert!(split_column(&input, &empty_outputs, &limits).is_err());
        let duplicates = SplitColumn {
            new_columns: vec!["x".into(), "x".into()],
            ..empty_outputs
        };
        assert!(split_column(&input, &duplicates, &limits).is_err());
        let one_output = Limits {
            max_split_columns: 1,
            ..Limits::default()
        };
        let too_many = SplitColumn {
            column: "z".into(),
            delimiter: ",".into(),
            new_columns: vec!["x".into(), "y".into()],
            max_splits: -1,
        };
        assert!(split_column(&input, &too_many, &one_output).is_err());
    }

    #[test]
    fn select_columns_projects_in_given_order_zero_copy() {
        let input = batch();
        // Proiezione con ordine rovesciato rispetto allo schema.
        let projected = select_columns(
            &input,
            &SelectColumns {
                columns: vec!["a".into(), "z".into()],
            },
        )
        .expect("projection");
        assert_eq!(projected.schema().field(0).name(), "a");
        assert_eq!(projected.schema().field(1).name(), "z");
        assert_eq!(projected.num_rows(), input.num_rows());
        // Zero-copy: stessi array Arrow (stesso puntatore dati).
        assert!(Arc::ptr_eq(
            projected.column(0),
            input.column_by_name("a").expect("a")
        ));
        assert!(Arc::ptr_eq(
            projected.column(1),
            input.column_by_name("z").expect("z")
        ));
        // Metadata di schema preservati.
        assert_eq!(projected.schema().metadata(), input.schema().metadata());
        // Selezione di una sola colonna.
        let single = select_columns(
            &input,
            &SelectColumns {
                columns: vec!["z".into()],
            },
        )
        .expect("single");
        assert_eq!(single.num_columns(), 1);
    }

    #[test]
    fn select_columns_rejects_empty_missing_and_duplicates() {
        let input = batch();
        assert!(select_columns(&input, &SelectColumns { columns: vec![] }).is_err());
        assert!(select_columns(
            &input,
            &SelectColumns {
                columns: vec!["missing".into()],
            },
        )
        .is_err());
        assert!(select_columns(
            &input,
            &SelectColumns {
                columns: vec!["a".into(), "a".into()],
            },
        )
        .is_err());
        // Config strict: campo sconosciuto rifiutato.
        assert!(serde_json::from_value::<SelectColumns>(
            json!({"columns": ["a"], "surprise": true})
        )
        .is_err());
    }

    #[test]
    fn concat_null_modes_and_alphabetical_reorder_are_exact() {
        let input = batch();
        let output = concat_columns(
            &input,
            &ConcatColumns {
                columns: vec!["z".into(), "a".into()],
                output_column: "joined".into(),
                separator: "|".into(),
                skip_null: false,
            },
            &Limits::default(),
        )
        .expect("concat");
        let joined = output
            .column_by_name("joined")
            .expect("joined")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        assert_eq!(joined.value(1), "|2");

        let reordered = reorder_columns(
            &input,
            &ReorderColumns {
                columns: vec![],
                alphabetical: true,
            },
        )
        .expect("alphabetical");
        assert_eq!(reordered.schema().field(0).name(), "a");
    }

    // -------------------------------------------------------------------
    // table.align_schema (estensione v1.2)
    // -------------------------------------------------------------------

    fn align_fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, true),
                Field::new("extra", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec![Some("x"), None])),
                Arc::new(Float64Array::from(vec![Some(1.5), Some(2.5)])),
            ],
        )
        .expect("fixture")
    }

    fn align(config: serde_json::Value) -> AlignSchema {
        serde_json::from_value(config).expect("config align_schema")
    }

    #[test]
    fn align_schema_reorders_projects_and_fills_missing() {
        let input = align_fixture();
        let output = align_schema(
            &input,
            &align(json!({
                "columns": [
                    {"name": "name", "type": "Utf8"},
                    {"name": "id", "type": "Int64"},
                    {"name": "note", "type": "Utf8"}
                ]
            })),
        )
        .expect("align");
        // Riordino + proiezione: `extra` scartata, `note` aggiunta di null.
        let schema = output.schema();
        let names: Vec<_> = schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["name", "id", "note"]);
        assert_eq!(output.num_rows(), 2);
        let note = output
            .column_by_name("note")
            .expect("note")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        assert!(note.is_null(0) && note.is_null(1));
        assert!(output
            .schema()
            .field_with_name("note")
            .expect("note")
            .is_nullable());
        // Zero-copy sulle colonne passthrough.
        assert!(Arc::ptr_eq(
            output.column_by_name("name").expect("name"),
            input.column_by_name("name").expect("name")
        ));
        // keep_extra: le colonne non elencate sopravvivono in coda.
        let output = align_schema(
            &input,
            &align(json!({
                "columns": [{"name": "id", "type": "Int64"}],
                "keep_extra": true
            })),
        )
        .expect("align keep_extra");
        let schema = output.schema();
        let names: Vec<_> = schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["id", "name", "extra"]);
    }

    #[test]
    fn align_schema_default_values_per_type() {
        let input = align_fixture();
        let output = align_schema(
            &input,
            &align(json!({
                "columns": [
                    {"name": "id", "type": "Int64"},
                    {"name": "s", "type": "Utf8", "default": "n/d"},
                    {"name": "i", "type": "Int64", "default": -7},
                    {"name": "u", "type": "UInt64", "default": "42"},
                    {"name": "f", "type": "Float64", "default": "2,5"},
                    {"name": "b", "type": "Boolean", "default": "true"},
                    {"name": "d", "type": "Date32", "default": "2026-07-25"},
                    {"name": "t", "type": "Timestamp", "default": "2026-07-25T00:00:00Z"},
                    {"name": "dc", "type": "Decimal128", "default": "-12.34"},
                    {"name": "bin", "type": "Binary", "default": "abc"}
                ]
            })),
        )
        .expect("align defaults");
        assert_eq!(output.num_columns(), 10);
        assert_eq!(output.num_rows(), 2);
        // Le colonne da default sono non nullable e costanti.
        let schema = output.schema();
        for name in ["s", "i", "u", "f", "b", "d", "t", "dc", "bin"] {
            let field = schema.field_with_name(name).expect(name);
            assert!(!field.is_nullable(), "{name} nullable");
            assert_eq!(output.column_by_name(name).expect(name).null_count(), 0);
        }
        assert_eq!(
            output
                .column_by_name("s")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .expect("s")
                .value(0),
            "n/d"
        );
        assert_eq!(
            output
                .column_by_name("i")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
                .expect("i")
                .value(1),
            -7
        );
        assert_eq!(
            output
                .column_by_name("u")
                .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                .expect("u")
                .value(0),
            42
        );
        assert_eq!(
            output
                .column_by_name("f")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
                .expect("f")
                .value(0),
            2.5
        );
        assert!(output
            .column_by_name("b")
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
            .expect("b")
            .value(0));
        // 2026-07-25 = 20659 giorni dall'epoch.
        assert_eq!(
            output
                .column_by_name("d")
                .and_then(|c| c.as_any().downcast_ref::<Date32Array>())
                .expect("d")
                .value(0),
            20_659
        );
        assert_eq!(
            output
                .column_by_name("t")
                .and_then(|c| c.as_any().downcast_ref::<TimestampMillisecondArray>())
                .expect("t")
                .value(0),
            1_784_937_600_000
        );
        let decimal = output
            .column_by_name("dc")
            .and_then(|c| c.as_any().downcast_ref::<Decimal128Array>())
            .expect("dc");
        assert_eq!(decimal.data_type(), &DataType::Decimal128(38, 10));
        // -12.34 con scala 10 = -12.34 * 10^10.
        assert_eq!(decimal.value(0), -123_400_000_000_i128);
        assert_eq!(
            output
                .column_by_name("bin")
                .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
                .expect("bin")
                .value(0),
            b"abc"
        );
    }

    #[test]
    fn align_schema_rejects_type_mismatch_and_bad_configs() {
        let input = align_fixture();
        // Tipo diverso dal dichiarato: errore, mai cast implicito.
        let error = align_schema(
            &input,
            &align(json!({"columns": [{"name": "id", "type": "Utf8"}]})),
        )
        .expect_err("mismatch");
        assert!(error.to_string().contains("nessun cast implicito"));
        // Anche un tipo "vicino" ma diverso (Int64 vs Timestamp) e' un errore.
        assert!(align_schema(
            &input,
            &align(json!({"columns": [{"name": "id", "type": "Timestamp"}]})),
        )
        .is_err());
        // Colonne vuote, ripetute, default non convertibile.
        assert!(align_schema(&input, &align(json!({"columns": []}))).is_err());
        assert!(align_schema(
            &input,
            &align(json!({"columns": [
                {"name": "id", "type": "Int64"},
                {"name": "id", "type": "Int64"}
            ]})),
        )
        .is_err());
        assert!(align_schema(
            &input,
            &align(json!({"columns": [{"name": "n", "type": "Int64", "default": "abc"}]})),
        )
        .is_err());
        // Decimal con piu' cifre della scala: nessun arrotondamento implicito.
        assert!(align_schema(
            &input,
            &align(json!({"columns": [
                {"name": "dc", "type": "Decimal128", "default": "1.00000000001"}
            ]})),
        )
        .is_err());
        // Config strict: tipo fuori dal set chiuso e campo sconosciuto.
        assert!(serde_json::from_value::<AlignSchema>(
            json!({"columns": [{"name": "x", "type": "Int32"}]})
        )
        .is_err());
        assert!(serde_json::from_value::<AlignSchema>(
            json!({"columns": [{"name": "x", "type": "Utf8", "surprise": 1}]})
        )
        .is_err());
    }
}
