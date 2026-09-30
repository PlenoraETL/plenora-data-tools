use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use plenora_core::arrow::array::{
    builder::StringBuilder, new_null_array, Array, ArrayRef, BinaryArray, BooleanArray,
    Date32Array, Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampMillisecondArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};
use serde::Deserialize;
use serde_json::Value;

use crate::Limits;
use plenora_core::{PlenoraError, Result};

use super::{replace_or_append, utf8_column, validate_output_name};

/// Config di `table.drop_columns`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropColumns {
    /// Colonne da togliere (obbligatorio, almeno una, senza ripetizioni:
    /// [`DropColumns::verifica_parametri`]). Un nome assente dallo schema non
    /// toglie niente e si accetta: dipende dall'ingresso, e lo stesso piano
    /// deve poter girare su tabelle con e senza quella colonna.
    pub columns: Vec<String>,
}

impl DropColumns {
    /// Parametri senza effetto per la sola config, rifiutati: `columns` vuoto
    /// (non toglie niente con nessun ingresso) e un nome ripetuto. La
    /// chiamano il kernel e l'analisi dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per ciascuna delle regole.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.columns.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "columns vuoto: nessuna colonna da togliere".into(),
            ));
        }
        let mut visti = HashSet::new();
        for name in &self.columns {
            if !visti.insert(name.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "columns: colonna ripetuta: {name}"
                )));
            }
        }
        Ok(())
    }
}

/// Batch senza le colonne elencate in `config` (`table.drop_columns`).
///
/// Le colonne rimaste restano nell'ordine d'ingresso, con i metadati di
/// campo e di schema e lo stesso numero di righe, anche se non resta
/// nessuna colonna. Gli array si condividono (clone dell'`Arc`), nessun dato
/// si copia.
///
/// # Errors
///
/// - `InvalidPlan`: le regole di [`DropColumns::verifica_parametri`]
///   (`columns` vuoto, un nome ripetuto); un nome assente non toglie niente;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
pub fn drop_columns(batch: &RecordBatch, config: &DropColumns) -> Result<RecordBatch> {
    config.verifica_parametri()?;
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
    // La cardinalita' la dichiara il costruttore condiviso, non una copia
    // locale che potrebbe divergere.
    crate::batch_with_rows(schema, columns, batch.num_rows())
}

/// Config di `table.select_columns`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectColumns {
    /// Colonne da tenere, nell'ordine d'uscita (obbligatorio): almeno una,
    /// senza ripetizioni, tutte presenti nell'ingresso.
    pub columns: Vec<String>,
}

/// Batch con le sole colonne elencate, nell'ordine dato
/// (`table.select_columns`).
///
/// Gli array si condividono (clone dell'`Arc`), nessun dato si copia; i
/// metadati di campo e di schema restano.
///
/// # Errors
///
/// - `InvalidPlan`: elenco `columns` vuoto o colonna ripetuta;
/// - `Schema`: colonna non trovata nello schema;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
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
    // Righe DICHIARATE: la lista di colonne deriva dall'input e puo'
    // essere vuota (batch a zero colonne, legittimi in Arrow).
    crate::batch_with_rows(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        columns,
        batch.num_rows(),
    )
}

// ---------------------------------------------------------------------------
// table.align_schema
// ---------------------------------------------------------------------------

/// Tipo dichiarato di `table.align_schema`.
///
/// Insieme chiuso di nomi, scritti esattamente come le varianti (`"Utf8"`,
/// `"Int64"`, …). La corrispondenza con il `DataType` Arrow e' fissa
/// ([`AlignType::data_type`]) e una colonna esistente deve averlo
/// identico: nessuna conversione implicita.
#[derive(Debug, Clone, Copy, Deserialize)]
pub enum AlignType {
    /// `Utf8`.
    Utf8,
    /// `Int64`.
    Int64,
    /// `UInt64`.
    UInt64,
    /// `Float64`.
    Float64,
    /// `Boolean`.
    Boolean,
    /// `Date32` (giorni dall'epoca).
    Date32,
    /// `Timestamp(Millisecond, None)`: millisecondi, senza fuso.
    Timestamp,
    /// `Decimal128(38, 10)`.
    Decimal128,
    /// `Binary`.
    Binary,
}

impl AlignType {
    /// `DataType` Arrow corrispondente: `Timestamp` = millisecondi senza
    /// fuso (il profilo scalare testuale legge solo i millisecondi),
    /// `Decimal128` = precisione 38, scala 10.
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

/// Una colonna dichiarata di `table.align_schema`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignColumn {
    /// Nome della colonna (obbligatorio, non vuoto, al piu' 1024 byte).
    pub name: String,
    /// Tipo dichiarato (obbligatorio; chiave JSON `type`).
    #[serde(rename = "type")]
    pub align_type: AlignType,
    /// Valore di ogni cella se la colonna manca nell'ingresso: la colonna
    /// aggiunta e' costante e non nullable. Assente (o `null`): colonna di
    /// null, nullable. Su una colonna che l'ingresso ha gia' non si legge:
    /// dipende dall'ingresso, quindi si accetta (lo stesso piano allinea
    /// tabelle con e senza la colonna). Le conversioni ammesse sono quelle
    /// di [`check_align_default`].
    #[serde(default)]
    pub default: Option<Value>,
}

/// Config di `table.align_schema`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlignSchema {
    /// Schema d'uscita, nell'ordine d'uscita (obbligatorio, almeno una
    /// colonna, nomi senza ripetizioni).
    pub columns: Vec<AlignColumn>,
    /// Colonne d'ingresso non dichiarate: scartate (`false`, default) o
    /// tenute in coda nell'ordine d'ingresso (`true`). Senza colonne non
    /// dichiarate non cambia niente, e si accetta: dipende dall'ingresso.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub keep_extra: Option<bool>,
}

impl AlignSchema {
    /// `true` se le colonne non dichiarate restano in coda.
    #[must_use]
    pub fn tiene_extra(&self) -> bool {
        self.keep_extra.unwrap_or(false)
    }
}

/// Precisione/scala fisse del mapping `AlignType::Decimal128`.
const ALIGN_DECIMAL_PRECISION: u8 = 38;
const ALIGN_DECIMAL_SCALE: i8 = 10;

/// Parsing `Decimal128(38, 10)` di un letterale testuale: segno opzionale,
/// parte intera e frazionaria solo cifre, al massimo 10 decimali (nessun
/// arrotondamento: piu' cifre della scala -> errore).
///
/// I segni iniziali si tolgono tutti e conta solo il primo carattere
/// (`"--5"` vale -5, `"+-5"` vale 5): limite dichiarato nella scheda di
/// `table.align_schema`.
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

/// Nome della forma JSON di un valore: e' una proprieta' strutturale, non il
/// contenuto, e puo' comparire in un messaggio d'errore.
const fn forma_json(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Materializza una colonna costante di `rows` righe dal `default` JSON:
/// stessa conversione validata da [`check_align_default`] (fail-closed in
/// validazione, mai a meta' dei dati).
fn align_default_column(value: &Value, align_type: AlignType, rows: usize) -> Result<ArrayRef> {
    // Il messaggio nomina la FORMA del default, mai il suo contenuto: il
    // default e' materializzato in ogni cella della colonna, quindi citarlo
    // significherebbe scrivere un valore di cella in un errore (regola
    // «errori senza dati»).
    let invalid = || {
        PlenoraError::InvalidPlan(format!(
            "align_schema: default di tipo JSON {} non convertibile in {align_type:?}",
            forma_json(value)
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
    // Invariante interna, verificata SEMPRE e in modo fallibile: un
    // `debug_assert_eq!` sarebbe una primitiva di panico nel codice di
    // produzione (nessun panico ammesso) e in build debug farebbe abortire
    // il chiamante invece di restituirgli un errore.
    if array.data_type() != &align_type.data_type() {
        return Err(PlenoraError::Internal(format!(
            "align_schema: colonna costruita con un tipo diverso da {align_type:?}"
        )));
    }
    Ok(array)
}

/// Valida il `default` di una colonna dichiarata con la stessa conversione
/// del kernel, su una colonna di una riga (per l'analisi del contratto).
///
/// Conversioni ammesse: `Utf8` e `Binary` da una stringa JSON; `Int64` e
/// `UInt64` da un intero JSON del dominio o da una stringa che lo e' (spazi
/// ai lati ignorati); `Float64` da un numero JSON o da una stringa con la
/// virgola decimale ammessa; `Boolean` da un booleano JSON o da
/// `"true"`/`"false"` senza distinzione di maiuscole; `Date32` da una
/// stringa `AAAA-MM-GG`; `Timestamp` da una stringa RFC 3339 con fuso;
/// `Decimal128` da un numero JSON o da una stringa, senza esponente e con al
/// piu' 10 decimali.
///
/// # Errors
///
/// - `InvalidPlan`: `default` non convertibile in `align_type`;
/// - `DataMapping`: errore Arrow sulla precisione/scala `Decimal128`
///   (guardia interna, non attesa);
/// - `Internal`: colonna costruita con un tipo diverso da quello dichiarato
///   (guardia interna).
pub fn check_align_default(value: &Value, align_type: AlignType) -> Result<()> {
    align_default_column(value, align_type, 1).map(|_| ())
}

/// Allinea lo schema dell'ingresso all'elenco dichiarato
/// (`table.align_schema`).
///
/// Riordina e proietta secondo `columns`; una colonna assente si aggiunge
/// come colonna di null (nullable) o costante col `default` (non nullable);
/// una colonna presente con tipo diverso dal dichiarato e' un errore (mai
/// conversione implicita). Le colonne non elencate si scartano, salvo
/// `keep_extra` (in coda nell'ordine d'ingresso).
///
/// # Errors
///
/// - `InvalidPlan`: elenco `columns` vuoto, colonna ripetuta, nome non
///   valido ([`validate_output_name`]), tipo presente diverso dal
///   dichiarato o `default` non convertibile ([`check_align_default`]);
/// - `DataMapping`: errore Arrow nella costruzione del batch o sulla
///   precisione/scala `Decimal128` (guardie interne, non attese);
/// - `Internal`: colonna costruita con un tipo diverso da quello dichiarato
///   (guardia interna).
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
    if config.tiene_extra() {
        for (field, column) in schema.fields().iter().zip(batch.columns()) {
            if !seen.contains(field.name().as_str()) {
                fields.push(field.as_ref().clone());
                columns.push(Arc::clone(column));
            }
        }
    }
    // Righe DICHIARATE: la lista di colonne deriva dall'input e puo'
    // essere vuota (batch a zero colonne, legittimi in Arrow).
    crate::batch_with_rows(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        columns,
        batch.num_rows(),
    )
}

/// Una rinomina di `table.rename`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenamePair {
    /// Nome attuale (obbligatorio); se non e' una colonna dell'ingresso la
    /// coppia non rinomina niente e si accetta: dipende dall'ingresso.
    pub old_name: String,
    /// Nuovo nome (obbligatorio, non vuoto, al piu' 1024 byte).
    pub new_name: String,
}

/// Config di `table.rename`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
    /// Rinomine, applicate tutte insieme (obbligatorio, almeno una). Un
    /// `old_name` ripetuto, o uguale al suo `new_name`, si rifiuta
    /// ([`Rename::verifica_parametri`]).
    pub renames: Vec<RenamePair>,
}

impl Rename {
    /// Rinomine senza effetto per la sola config, rifiutate: `renames`
    /// vuoto, un `old_name` uguale al suo `new_name`, due coppie della
    /// stessa sorgente (una delle due non avrebbe effetto) o della stessa
    /// destinazione. Un `old_name` che
    /// l'ingresso non ha si accetta: dipende dall'ingresso. La chiamano il
    /// kernel e l'analisi dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per ciascuna delle regole.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.renames.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "renames vuoto: nessuna rinomina".into(),
            ));
        }
        let mut visti = HashSet::new();
        let mut destinazioni = HashSet::new();
        for pair in &self.renames {
            if !visti.insert(pair.old_name.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "rename origine: colonna ripetuta: {}",
                    pair.old_name
                )));
            }
            // Due sorgenti verso la stessa destinazione: con una sola delle
            // due nell'ingresso una rinomina sparirebbe; con entrambe il nome
            // sarebbe duplicato. Si decide dalla config.
            if !destinazioni.insert(pair.new_name.as_str()) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "rename destinazione: colonna ripetuta: {}",
                    pair.new_name
                )));
            }
            if pair.old_name == pair.new_name {
                return Err(PlenoraError::InvalidPlan(format!(
                    "rinomina di {} su se stessa senza effetto",
                    pair.old_name
                )));
            }
        }
        Ok(())
    }
}

/// Rinomina le colonne secondo `renames` (`table.rename`).
///
/// I nomi non mappati restano; le rinomine valgono insieme, quindi due nomi
/// si possono scambiare. Tipi, nullabilita', metadati di campo e di schema
/// restano; gli array si condividono.
///
/// # Errors
///
/// - `InvalidPlan`: nome d'uscita non valido ([`validate_output_name`]);
///   rinomine senza effetto ([`Rename::verifica_parametri`]);
/// - `Schema`: la rinomina produce un nome duplicato;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
pub fn rename(batch: &RecordBatch, config: &Rename) -> Result<RecordBatch> {
    config.verifica_parametri()?;
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
    // Righe DICHIARATE: la lista di colonne deriva dall'input e puo'
    // essere vuota (batch a zero colonne, legittimi in Arrow).
    crate::batch_with_rows(Arc::new(schema), batch.columns().to_vec(), batch.num_rows())
}

/// Config di `table.reorder_columns`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorderColumns {
    /// Colonne da mettere in testa, nell'ordine dato (default vuoto): senza
    /// ripetizioni, tutte presenti nell'ingresso.
    #[serde(default)]
    pub columns: Vec<String>,
    /// Ordina alfabeticamente le colonne non elencate (default `false`,
    /// [`ReorderColumns::alfabetico`]; alias `sort_alphabetical`). Scritto
    /// quando restano al piu' una colonna non elencata non avrebbe effetto
    /// ([`ReorderColumns::verifica_parametri`]).
    #[serde(
        default,
        alias = "sort_alphabetical",
        deserialize_with = "crate::mai_null"
    )]
    pub alphabetical: Option<bool>,
}

impl ReorderColumns {
    /// `true` se le colonne non elencate si ordinano per nome.
    #[must_use]
    pub fn alfabetico(&self) -> bool {
        self.alphabetical.unwrap_or(false)
    }

    /// Una config che non sposta niente con nessun ingresso (`columns`
    /// vuoto senza `alphabetical = true`) si rifiuta. `alphabetical` con al
    /// piu' una colonna restante si accetta: dipende dall'ingresso. La
    /// chiamano il kernel e l'analisi dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per la config che non sposta niente.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.columns.is_empty() && !self.alfabetico() {
            return Err(PlenoraError::InvalidPlan(
                "columns vuoto senza alphabetical: il riordino non sposta niente".into(),
            ));
        }
        Ok(())
    }
}

/// Riordina le colonne del batch (`table.reorder_columns`).
///
/// Prima quelle elencate in `columns`, nell'ordine dato, poi le restanti
/// nell'ordine d'ingresso o, con `alphabetical`, per nome in minuscolo
/// (confronto dei byte UTF-8, ordinamento stabile). Gli array si
/// condividono.
///
/// # Errors
///
/// - `InvalidPlan`: colonna ripetuta nell'elenco; parametri senza effetto
///   ([`ReorderColumns::verifica_parametri`]);
/// - `Schema`: colonna non trovata nello schema;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
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
    config.verifica_parametri()?;
    let mut remaining: Vec<usize> = (0..batch.num_columns())
        .filter(|index| !selected.contains(index))
        .collect();
    if config.alfabetico() {
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
    // Righe DICHIARATE: la lista di colonne deriva dall'input e puo'
    // essere vuota (batch a zero colonne, legittimi in Arrow).
    crate::batch_with_rows(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        columns,
        batch.num_rows(),
    )
}

/// Config di `table.concat_columns`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcatColumns {
    /// Colonne `Utf8` da unire, nell'ordine (obbligatorio, almeno una).
    pub columns: Vec<String>,
    /// Colonna d'uscita (default `"concatenated"`); se esiste si sostituisce.
    #[serde(default = "default_concat_output")]
    pub output_column: String,
    /// Testo fra due parti (default `" "`, [`ConcatColumns::separatore`];
    /// puo' essere vuoto). Con una sola colonna non ci sono due parti:
    /// scritto si rifiuta ([`ConcatColumns::verifica_parametri`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub separator: Option<String>,
    /// Salta i null (default `true`); con `false` un null vale `""`.
    #[serde(default = "default_true")]
    pub skip_null: bool,
}

fn default_concat_output() -> String {
    "concatenated".into()
}
const DEFAULT_SEPARATOR: &str = " ";

impl ConcatColumns {
    /// Il separatore: `separator`, o uno spazio se assente.
    #[must_use]
    pub fn separatore(&self) -> &str {
        self.separator.as_deref().unwrap_or(DEFAULT_SEPARATOR)
    }

    /// `separator` con una sola colonna non si scrive mai (sta solo fra due
    /// parti): scritto si rifiuta. La chiamano il kernel e l'analisi dei
    /// contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` se `separator` e' scritto con una sola colonna.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.separator.is_some() && self.columns.len() == 1 {
            return Err(PlenoraError::InvalidPlan(
                "separator senza effetto con una sola colonna".into(),
            ));
        }
        Ok(())
    }
}
const fn default_true() -> bool {
    true
}

/// Concatena le colonne `Utf8` elencate nella colonna `output_column`
/// (`table.concat_columns`).
///
/// Le parti si uniscono con `separator`, che sta solo fra parti incluse;
/// con `skip_null` i null si saltano (riga di soli null -> null),
/// altrimenti contano come stringa vuota e il risultato non e' mai null.
/// Se `output_column` esiste gia' si sostituisce nella sua posizione,
/// altrimenti si aggiunge in coda (`Utf8` nullable).
///
/// # Errors
///
/// - `InvalidPlan`: elenco `columns` vuoto o nome d'uscita non valido
///   ([`validate_output_name`]); `separator` senza effetto
///   ([`ConcatColumns::verifica_parametri`]);
/// - `ResourceLimit`: valore concatenato oltre `limits.max_string_bytes`;
/// - `Schema`: colonna assente o non `Utf8` ([`utf8_column`]);
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna di [`replace_or_append`], non attesa).
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
    config.verifica_parametri()?;
    let separator = config.separatore();
    let arrays = config
        .columns
        .iter()
        .map(|name| utf8_column(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let mut output = Vec::with_capacity(batch.num_rows());
    // String di lavoro riusata fra le righe (hot path minimale): stessi byte di
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
                joined.push_str(separator);
            }
            if !array.is_null(row) {
                joined.push_str(array.value(row));
            }
            included += 1;
        }
        if config.skip_null && included == 0 {
            output.push(None);
        } else if joined.len() > limits.max_string_bytes {
            return Err(PlenoraError::ResourceLimit(
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

/// Config di `table.split_column`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitColumn {
    /// Colonna `Utf8` da dividere (obbligatorio).
    pub column: String,
    /// Separatore letterale, non vuoto (default `","`,
    /// [`SplitColumn::delimitatore`]). Con una sola colonna d'uscita il testo
    /// non si divide: scritto si rifiuta ([`SplitColumn::verifica_parametri`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub delimiter: Option<String>,
    /// Colonne d'uscita, una per parte (obbligatorio, da 1 a
    /// `limits.max_split_columns`, senza ripetizioni).
    pub new_columns: Vec<String>,
    /// Divisioni massime (`max_splits + 1` parti, le colonne oltre restano
    /// null); assente: tante parti quante `new_columns`. Scritto ha effetto
    /// solo fra 1 e `len(new_columns) - 2`: un valore non positivo o da
    /// `len(new_columns) - 1` in su da' le stesse parti dell'assente, e si
    /// rifiuta ([`SplitColumn::verifica_parametri`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub max_splits: Option<i64>,
}

const DEFAULT_DELIMITER: &str = ",";

impl SplitColumn {
    /// Il separatore: `delimiter`, o `","` se assente.
    #[must_use]
    pub fn delimitatore(&self) -> &str {
        self.delimiter.as_deref().unwrap_or(DEFAULT_DELIMITER)
    }

    /// Parametri senza effetto, rifiutati: `delimiter` con una sola colonna
    /// d'uscita (il testo intero va in quella colonna), `max_splits` che non
    /// riduce le parti (non positivo, o almeno `len(new_columns) - 1`). La
    /// chiamano il kernel e l'analisi dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per ciascuna delle due regole.
    pub fn verifica_parametri(&self) -> Result<()> {
        let parti = self.new_columns.len();
        if self.delimiter.is_some() && parti == 1 {
            return Err(PlenoraError::InvalidPlan(
                "delimiter senza effetto con una sola colonna d'uscita".into(),
            ));
        }
        if let Some(max_splits) = self.max_splits {
            let riduce = usize::try_from(max_splits)
                .is_ok_and(|divisioni| divisioni > 0 && divisioni.saturating_add(1) < parti);
            if !riduce {
                return Err(PlenoraError::InvalidPlan(
                    "max_splits senza effetto: non riduce le parti di new_columns".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Divide la colonna `Utf8` `column` sul `delimiter` nelle `new_columns`
/// (`table.split_column`).
///
/// Le parti sono al piu' `len(new_columns)`, e al piu' `max_splits + 1` se
/// `max_splits` e' scritto: l'ultima tiene il resto del testo, delimitatori
/// compresi; le colonne senza parte ricevono null, e un null d'ingresso da'
/// null ovunque. Una colonna d'uscita gia' esistente si sostituisce nella
/// sua posizione, le altre si aggiungono in coda.
///
/// # Errors
///
/// - `InvalidPlan`: `delimiter` vuoto, `new_columns` vuoto o oltre
///   `limits.max_split_columns`, nome d'uscita non valido
///   ([`validate_output_name`]); parametri senza effetto
///   ([`SplitColumn::verifica_parametri`]);
/// - `Schema`: nomi d'uscita duplicati, oppure colonna assente o non `Utf8`
///   ([`utf8_column`]);
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna di [`replace_or_append`], non attesa).
pub fn split_column(
    batch: &RecordBatch,
    config: &SplitColumn,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.delimitatore().is_empty() {
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
    config.verifica_parametri()?;
    let delimiter = config.delimitatore();
    let input = utf8_column(batch, &config.column)?;
    let requested_parts = config.new_columns.len();
    let split_limit = match config.max_splits {
        Some(max_splits) if max_splits > 0 => usize::try_from(max_splits)
            .unwrap_or(usize::MAX)
            .saturating_add(1)
            .min(requested_parts),
        _ => requested_parts,
    };
    // Un builder Arrow per colonna di output: le parti si copiano dal testo
    // d'ingresso ai buffer, senza una `String` per cella ne' un `Vec` di
    // parti per riga. Le parti oltre `split_limit` (e le colonne senza una
    // parte) sono null, come nel `Vec` di parti indicizzato di prima.
    let mut outputs = (0..requested_parts)
        .map(|_| StringBuilder::with_capacity(batch.num_rows(), 0))
        .collect::<Vec<_>>();
    for row in 0..batch.num_rows() {
        if input.is_null(row) {
            for output in &mut outputs {
                output.append_null();
            }
            continue;
        }
        let mut parts = input.value(row).splitn(split_limit, delimiter);
        for output in &mut outputs {
            match parts.next() {
                Some(part) => output.append_value(part),
                None => output.append_null(),
            }
        }
    }
    let mut result = batch.clone();
    for (name, mut output) in config.new_columns.iter().zip(outputs) {
        result = replace_or_append(
            &result,
            name,
            DataType::Utf8,
            true,
            Arc::new(output.finish()),
        )?;
    }
    Ok(result)
}

#[cfg(test)]
// Confronti float esatti intenzionali: i default `Float64` delle fixture
// sono letterali esatti; il confronto per bit e' il contratto verificato,
// non un'approssimazione.
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
        // Un nome assente non toglie niente e si accetta (dipende
        // dall'ingresso); una lista vuota si rifiuta.
        let dropped = drop_columns(
            &input,
            &DropColumns {
                columns: vec!["missing".into()],
            },
        )
        .expect("unknown drop is no-op");
        assert_eq!(dropped.num_columns(), 2);
        assert!(matches!(
            drop_columns(&input, &DropColumns { columns: vec![] }),
            Err(PlenoraError::InvalidPlan(_))
        ));

        // Una rinomina vuota e un riordino che non sposta niente non hanno
        // effetto: si rifiutano.
        assert!(matches!(
            rename(&input, &Rename { renames: vec![] }),
            Err(PlenoraError::InvalidPlan(_))
        ));
        let reorder: ReorderColumns = serde_json::from_value(json!({})).expect("defaults");
        assert!(!reorder.alfabetico());
        assert!(matches!(
            reorder_columns(&input, &reorder),
            Err(PlenoraError::InvalidPlan(_))
        ));

        let concat: ConcatColumns =
            serde_json::from_value(json!({"columns": ["a"]})).expect("defaults");
        assert_eq!(concat.output_column, "concatenated");
        assert_eq!(concat.separatore(), " ");
        assert!(concat.skip_null);

        let split: SplitColumn =
            serde_json::from_value(json!({"column": "z", "new_columns": ["one", "two", "three"]}))
                .expect("defaults");
        assert_eq!(split.delimitatore(), ",");
        assert_eq!(split.max_splits, None);
        let output = split_column(&input, &split, &Limits::default()).expect("unbounded split");
        assert_eq!(output.num_columns(), 5);
    }

    /// Errore della guardia attesa: variante e messaggio esatti (il `Display`
    /// porta il prefisso della variante), non un rifiuto qualunque.
    fn assert_guard(result: Result<RecordBatch>, expected: &str) {
        let error = result.expect_err("guardia non scattata");
        assert_eq!(error.to_string(), expected);
    }

    #[test]
    fn runtime_guards_remain_defensive() {
        let input = batch();
        let limits = Limits::default();
        assert_guard(
            concat_columns(
                &input,
                &ConcatColumns {
                    columns: vec![],
                    output_column: "x".into(),
                    separator: Some(String::new()),
                    skip_null: true,
                },
                &limits,
            ),
            "contract violation: concat_columns richiede almeno una colonna",
        );
        assert_guard(
            reorder_columns(
                &input,
                &ReorderColumns {
                    columns: vec!["a".into(), "a".into()],
                    alphabetical: None,
                },
            ),
            "contract violation: colonna ripetuta nel riordino: a",
        );

        let base = SplitColumn {
            column: "z".into(),
            delimiter: Some(",".into()),
            new_columns: vec!["x".into()],
            max_splits: None,
        };
        let empty_delimiter = SplitColumn {
            delimiter: Some(String::new()),
            ..base
        };
        assert_guard(
            split_column(&input, &empty_delimiter, &limits),
            "contract violation: delimiter vuoto",
        );
        let empty_outputs = SplitColumn {
            column: "z".into(),
            delimiter: Some(",".into()),
            new_columns: vec![],
            max_splits: None,
        };
        assert_guard(
            split_column(&input, &empty_outputs, &limits),
            "contract violation: new_columns e' obbligatorio nel percorso streaming",
        );
        let duplicates = SplitColumn {
            new_columns: vec!["x".into(), "x".into()],
            ..empty_outputs
        };
        assert_guard(
            split_column(&input, &duplicates, &limits),
            "schema violation: split_column contiene nomi output duplicati",
        );
        let one_output = Limits {
            max_split_columns: 1,
            ..Limits::default()
        };
        let too_many = SplitColumn {
            column: "z".into(),
            delimiter: Some(",".into()),
            new_columns: vec!["x".into(), "y".into()],
            max_splits: None,
        };
        assert_guard(
            split_column(&input, &too_many, &one_output),
            "contract violation: split_column supera max_split_columns",
        );
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
        assert_guard(
            select_columns(&input, &SelectColumns { columns: vec![] }),
            "contract violation: select_columns richiede almeno una colonna",
        );
        assert_guard(
            select_columns(
                &input,
                &SelectColumns {
                    columns: vec!["missing".into()],
                },
            ),
            "schema violation: colonna non trovata: missing",
        );
        assert_guard(
            select_columns(
                &input,
                &SelectColumns {
                    columns: vec!["a".into(), "a".into()],
                },
            ),
            "contract violation: colonna ripetuta nella proiezione: a",
        );
        // Config strict: campo sconosciuto rifiutato, e solo per quello.
        assert!(serde_json::from_value::<SelectColumns>(json!({"columns": ["a"]})).is_ok());
        let error =
            serde_json::from_value::<SelectColumns>(json!({"columns": ["a"], "surprise": true}))
                .expect_err("campo sconosciuto accettato");
        assert!(
            error.to_string().contains("unknown field `surprise`"),
            "{error}"
        );
    }

    #[test]
    fn concat_null_modes_and_alphabetical_reorder_are_exact() {
        let input = batch();
        let output = concat_columns(
            &input,
            &ConcatColumns {
                columns: vec!["z".into(), "a".into()],
                output_column: "joined".into(),
                separator: Some("|".into()),
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
                alphabetical: Some(true),
            },
        )
        .expect("alphabetical");
        assert_eq!(reordered.schema().field(0).name(), "a");
    }

    // -------------------------------------------------------------------
    // table.align_schema
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

#[cfg(test)]
#[path = "split_oracolo.rs"]
mod split_oracolo;
