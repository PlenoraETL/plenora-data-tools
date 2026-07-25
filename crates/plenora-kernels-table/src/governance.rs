use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde::Deserialize;

use crate::Limits;
use plenora_core::{PlenoraError, Result};
use crate::{column_index, replace_or_append, scalar_as_f64, scalar_as_string};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertCardinality {
    #[serde(default)]
    pub exact_rows: Option<usize>,
    #[serde(default)]
    pub min_rows: Option<usize>,
    #[serde(default)]
    pub max_rows: Option<usize>,
}

pub fn assert_cardinality(
    batch: &RecordBatch,
    config: &AssertCardinality,
) -> Result<RecordBatch> {
    let valid = config.exact_rows.map_or_else(
        || {
            config.min_rows.is_none_or(|min| batch.num_rows() >= min)
                && config.max_rows.is_none_or(|max| batch.num_rows() <= max)
        },
        |exact| batch.num_rows() == exact,
    );
    if valid {
        Ok(batch.clone())
    } else {
        Err(PlenoraError::Contract(format!(
            "assert_cardinality: {} righe fuori contratto",
            batch.num_rows()
        )))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertMetadata {
    pub expected: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub allow_extra: bool,
}

const fn default_true() -> bool {
    true
}

pub fn assert_metadata(batch: &RecordBatch, config: &AssertMetadata) -> Result<RecordBatch> {
    let schema = batch.schema();
    let metadata = schema.metadata();
    if (!config.allow_extra && metadata.len() != config.expected.len())
        || config
            .expected
            .iter()
            .any(|(key, value)| metadata.get(key) != Some(value))
    {
        return Err(PlenoraError::Schema(
            "assert_metadata: metadata non conforme".into(),
        ));
    }
    Ok(batch.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForeignKey {
    pub left_keys: Vec<String>,
    pub right_keys: Vec<String>,
    #[serde(default)]
    pub allow_null: bool,
}

fn key_indices(batch: &RecordBatch, names: &[String]) -> Result<Vec<usize>> {
    names.iter().map(|name| column_index(batch, name)).collect()
}

fn validate_key_types(
    left: &RecordBatch,
    right: &RecordBatch,
    left_indices: &[usize],
    right_indices: &[usize],
) -> Result<()> {
    if left_indices
        .iter()
        .zip(right_indices)
        .any(|(left_index, right_index)| {
            left.column(*left_index).data_type() != right.column(*right_index).data_type()
        })
    {
        return Err(PlenoraError::Schema(
            "foreign key richiede tipi Arrow identici".into(),
        ));
    }
    Ok(())
}

fn has_null(batch: &RecordBatch, indices: &[usize], row: usize) -> bool {
    indices
        .iter()
        .any(|index| batch.column(*index).is_null(row))
}

// ---------------------------------------------------------------------------
// Fast path chiavi di riga (batch 4 ottimizzazioni kernel: `reconcile`,
// `assert_foreign_key`).
//
// `RowKeyEncoder` prepara una sola volta il tag di tipo per colonna e itera
// sui valori nativi, scrivendo in un buffer riusato gli STESSI byte di
// `quality::key_for_row` (che resta invariata come oracolo dei test):
// prefisso `len(tipo)+tipo`, marcatore null 0/1, `len(valore)+valore` con il
// valore formattato come `scalar_as_string` (stesso Display per numerici e
// booleani, NaN -> "NaN", -0.0 -> "-0"). I tipi fuori dal fast path ricadono
// sul percorso scalare generico, con gli stessi errori.
// ---------------------------------------------------------------------------

/// Colonna di chiave tipizzata per `RowKeyEncoder`.
enum KeyValueColumn<'a> {
    Utf8(&'a StringArray),
    Int64(&'a Int64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    UInt64(&'a UInt64Array),
    Generic(&'a ArrayRef),
}

impl<'a> KeyValueColumn<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Self::Boolean(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64(values);
        }
        Self::Generic(array)
    }

    /// Scrive in `text` gli stessi byte di `scalar_as_string` per `row`;
    /// restituisce `false` (senza scrivere) se il valore e' null.
    fn write_value(&self, row: usize, text: &mut String) -> Result<bool> {
        match self {
            Self::Utf8(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                text.push_str(values.value(row));
            }
            Self::Int64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                write!(text, "{}", values.value(row)).expect("fmt su String");
            }
            Self::Float64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                write!(text, "{}", values.value(row)).expect("fmt su String");
            }
            Self::Boolean(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                write!(text, "{}", values.value(row)).expect("fmt su String");
            }
            Self::UInt64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                write!(text, "{}", values.value(row)).expect("fmt su String");
            }
            Self::Generic(array) => {
                let Some(value) = scalar_as_string(array.as_ref(), row)? else {
                    return Ok(false);
                };
                text.push_str(&value);
            }
        }
        Ok(true)
    }
}

/// Encoder zero-copy di chiavi di riga: stessi byte di `quality::key_for_row`
/// senza allocare una String per colonna per riga.
struct RowKeyEncoder<'a> {
    columns: Vec<(Vec<u8>, KeyValueColumn<'a>)>,
    text: String,
}

/// Hasher moltiplicativo a blocchi (stile `FxHash`) con finalizer splitmix64,
/// come nei fast path di `aggregate` e `join`: `SipHash` (default std)
/// dominerebbe il costo di build/probe su milioni di righe; qui il throughput
/// conta piu' della resistenza a input avversari. Le mappe che lo usano non
/// sono mai iterate in modo osservabile: semantica invariata.
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let value = u64::from_le_bytes(chunk.try_into().expect("blocco di 8 byte"));
            self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(K);
        }
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            let mut tail = 0_u64;
            for &byte in remainder {
                tail = (tail << 8) | u64::from(byte);
            }
            self.0 = (self.0.rotate_left(5) ^ tail).wrapping_mul(K);
        }
    }
}

type KeySet = HashSet<Vec<u8>, BuildHasherDefault<KeyHasher>>;
type KeyFreqMap = HashMap<Vec<u8>, usize, BuildHasherDefault<KeyHasher>>;

impl<'a> RowKeyEncoder<'a> {
    fn new(batch: &'a RecordBatch, indices: &[usize]) -> Self {
        let columns = indices
            .iter()
            .map(|index| {
                let column = batch.column(*index);
                let type_name = column.data_type().to_string();
                let mut prefix = Vec::with_capacity(8 + type_name.len());
                prefix.extend_from_slice(&(type_name.len() as u64).to_be_bytes());
                prefix.extend_from_slice(type_name.as_bytes());
                (prefix, KeyValueColumn::new(column))
            })
            .collect();
        Self {
            columns,
            text: String::new(),
        }
    }

    /// Scrive in `output` (riusato tra le righe) gli stessi byte di
    /// `quality::key_for_row` per `row`.
    fn encode_into(&mut self, row: usize, output: &mut Vec<u8>) -> Result<()> {
        output.clear();
        for (prefix, column) in &self.columns {
            output.extend_from_slice(prefix);
            self.text.clear();
            if column.write_value(row, &mut self.text)? {
                output.push(1);
                output.extend_from_slice(&(self.text.len() as u64).to_be_bytes());
                output.extend_from_slice(self.text.as_bytes());
            } else {
                output.push(0);
            }
        }
        Ok(())
    }
}

pub fn assert_foreign_key(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &ForeignKey,
    limits: &Limits,
) -> Result<RecordBatch> {
    let left_indices = key_indices(left, &config.left_keys)?;
    let right_indices = key_indices(right, &config.right_keys)?;
    validate_key_types(left, right, &left_indices, &right_indices)?;
    let mut right_encoder = RowKeyEncoder::new(right, &right_indices);
    let mut referenced =
        KeySet::with_capacity_and_hasher(right.num_rows(), BuildHasherDefault::default());
    let mut memory_used = 0_usize;
    let mut key = Vec::new();
    for row in 0..right.num_rows() {
        if !has_null(right, &right_indices, row) {
            right_encoder.encode_into(row, &mut key)?;
            let key_bytes = key.len();
            if referenced.insert(key.clone()) {
                memory_used = memory_used
                    .checked_add(key_bytes.saturating_add(64))
                    .ok_or_else(|| PlenoraError::Contract("overflow memoria foreign key".into()))?;
                if memory_used > limits.max_memory_bytes {
                    return Err(PlenoraError::Contract(
                        "assert_foreign_key oltre max_memory_bytes".into(),
                    ));
                }
            }
        }
    }
    let mut left_encoder = RowKeyEncoder::new(left, &left_indices);
    for row in 0..left.num_rows() {
        if has_null(left, &left_indices, row) {
            if config.allow_null {
                continue;
            }
            return Err(PlenoraError::Contract(format!(
                "assert_foreign_key: chiave null alla riga {row}"
            )));
        }
        left_encoder.encode_into(row, &mut key)?;
        if !referenced.contains(key.as_slice()) {
            return Err(PlenoraError::Contract(format!(
                "assert_foreign_key: riferimento mancante alla riga {row}"
            )));
        }
    }
    Ok(left.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconcile {
    pub left_keys: Vec<String>,
    pub right_keys: Vec<String>,
    #[serde(default = "default_true")]
    pub nulls_equal: bool,
}

fn frequencies(
    batch: &RecordBatch,
    indices: &[usize],
    nulls_equal: bool,
    side_nulls: &mut usize,
    memory_used: &mut usize,
    limits: &Limits,
) -> Result<KeyFreqMap> {
    let mut output = KeyFreqMap::default();
    let mut encoder = RowKeyEncoder::new(batch, indices);
    let mut key = Vec::new();
    for row in 0..batch.num_rows() {
        if !nulls_equal && has_null(batch, indices, row) {
            *side_nulls = side_nulls
                .checked_add(1)
                .ok_or_else(|| PlenoraError::Contract("overflow null reconciliation".into()))?;
            continue;
        }
        encoder.encode_into(row, &mut key)?;
        if let Some(count) = output.get_mut(key.as_slice()) {
            *count = count
                .checked_add(1)
                .ok_or_else(|| PlenoraError::Contract("overflow reconciliation".into()))?;
        } else {
            *memory_used = memory_used
                .checked_add(key.len().saturating_add(64))
                .ok_or_else(|| PlenoraError::Contract("overflow memoria reconciliation".into()))?;
            if *memory_used > limits.max_memory_bytes {
                return Err(PlenoraError::Contract(
                    "reconcile oltre max_memory_bytes".into(),
                ));
            }
            output.insert(key.clone(), 1);
            if output.len() > limits.max_rows {
                return Err(PlenoraError::Contract(
                    "reconcile supera max_rows chiavi distinte".into(),
                ));
            }
        }
    }
    Ok(output)
}

fn as_u64(value: usize) -> Result<u64> {
    u64::try_from(value).map_err(|_| PlenoraError::Contract("conteggio oltre u64".into()))
}

pub fn reconcile(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &Reconcile,
    limits: &Limits,
) -> Result<RecordBatch> {
    let left_indices = key_indices(left, &config.left_keys)?;
    let right_indices = key_indices(right, &config.right_keys)?;
    validate_key_types(left, right, &left_indices, &right_indices)?;
    let mut left_nulls = 0;
    let mut right_nulls = 0;
    let mut memory_used = 0_usize;
    let left_counts = frequencies(
        left,
        &left_indices,
        config.nulls_equal,
        &mut left_nulls,
        &mut memory_used,
        limits,
    )?;
    let right_counts = frequencies(
        right,
        &right_indices,
        config.nulls_equal,
        &mut right_nulls,
        &mut memory_used,
        limits,
    )?;
    let mut matched = 0_usize;
    let mut left_only = left_nulls;
    let mut right_only = right_nulls;
    let mut left_duplicates = 0_usize;
    let mut right_duplicates = 0_usize;
    for (key, left_count) in &left_counts {
        let right_count = right_counts.get(key).copied().unwrap_or_default();
        let common = (*left_count).min(right_count);
        matched = matched.saturating_add(common);
        left_only = left_only.saturating_add(left_count - common);
        left_duplicates = left_duplicates.saturating_add(left_count.saturating_sub(1));
    }
    for (key, right_count) in &right_counts {
        let left_count = left_counts.get(key).copied().unwrap_or_default();
        right_only = right_only.saturating_add(right_count.saturating_sub(left_count));
        right_duplicates = right_duplicates.saturating_add(right_count.saturating_sub(1));
    }
    let metrics = [
        "matched_rows",
        "left_only_rows",
        "right_only_rows",
        "left_duplicate_rows",
        "right_duplicate_rows",
    ];
    let values = [
        matched,
        left_only,
        right_only,
        left_duplicates,
        right_duplicates,
    ]
    .into_iter()
    .map(as_u64)
    .collect::<Result<Vec<_>>>()?;
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("metric", DataType::Utf8, false),
            Field::new("value", DataType::UInt64, false),
        ])),
        vec![
            Arc::new(StringArray::from(metrics.to_vec())),
            Arc::new(UInt64Array::from(values)),
        ],
    )?)
}

// ---------------------------------------------------------------------------
// table.validate_rules (estensione v1.2)
// ---------------------------------------------------------------------------

/// Operatore di una regola di validazione: sottoinsieme degli operatori di
/// `filtering` con nomi testuali stabili.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleOperator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Isnull,
    Notnull,
    Regex,
    Range,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSeverity {
    #[default]
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidateOutputMode {
    /// Per riga: colonne `_valid`/`_errors`/`_warnings` aggiunte all'input.
    #[default]
    Annotate,
    /// Una riga aggregata per regola: `name`, `errors`, `warnings`.
    Summary,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateRule {
    pub name: String,
    pub operator: RuleOperator,
    #[serde(default)]
    pub column: Option<String>,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub severity: RuleSeverity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateRules {
    pub rules: Vec<ValidateRule>,
    #[serde(default)]
    pub output_mode: ValidateOutputMode,
}

/// Regola compilata: tutti i controlli statici (colonna, tipo-operatore,
/// valore atteso, regex) sono gia' stati fatti, prima di toccare i dati.
struct CompiledRule {
    name: String,
    operator: RuleOperator,
    column_index: usize,
    numeric_column: bool,
    severity: RuleSeverity,
    expected: String,
    expected_number: f64,
    expected_high: f64,
    regex: Option<regex::Regex>,
}

/// Replica `json_text` del kernel filtering.
fn rule_json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Tipi su cui i confronti ordinati/range hanno senso: leggibili da
/// `scalar_as_f64`, escluso `Utf8` (un testo da parsare per riga non e' una
/// colonna numerica: meglio un errore in validazione che un fallimento per
/// riga). Riusata dall'analisi a secco del contratto.
pub fn is_rule_numeric(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Float64
            | DataType::Int64
            | DataType::UInt64
            | DataType::Date32
            | DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, _)
            | DataType::Decimal128(_, _)
    )
}

/// Tipi confrontabili per uguaglianza (profilo scalare testuale o numerico).
/// Riusata dall'analisi a secco del contratto.
pub fn is_rule_comparable(data_type: &DataType) -> bool {
    is_rule_numeric(data_type)
        || matches!(
            data_type,
            DataType::Utf8 | DataType::Boolean | DataType::Binary
        )
}

/// Compila le regole: ogni errore di configurazione (colonna mancante, tipo
/// incompatibile con l'operatore, valore atteso non parsabile, regex
/// invalida) esce QUI, prima di leggere una sola riga di dati.
fn compile_rules(batch: &RecordBatch, config: &ValidateRules) -> Result<Vec<CompiledRule>> {
    if config.rules.is_empty() {
        return Err(PlenoraError::Contract(
            "validate_rules richiede almeno una regola".into(),
        ));
    }
    let mut seen = HashSet::new();
    let mut compiled = Vec::with_capacity(config.rules.len());
    for rule in &config.rules {
        if rule.name.trim().is_empty() {
            return Err(PlenoraError::Contract(
                "validate_rules: nome regola vuoto".into(),
            ));
        }
        if !seen.insert(rule.name.as_str()) {
            return Err(PlenoraError::Contract(format!(
                "validate_rules: regola ripetuta: {}",
                rule.name
            )));
        }
        let column = rule.column.as_deref().ok_or_else(|| {
            PlenoraError::Contract(format!(
                "validate_rules: regola {} senza column",
                rule.name
            ))
        })?;
        let column_index = column_index(batch, column)?;
        let data_type = batch.column(column_index).data_type().clone();
        let needs_value = !matches!(rule.operator, RuleOperator::Isnull | RuleOperator::Notnull);
        if needs_value != rule.value.is_some() {
            return Err(PlenoraError::Contract(format!(
                "validate_rules: regola {}: value {} per l'operatore {:?}",
                rule.name,
                if needs_value { "obbligatorio" } else { "non ammesso" },
                rule.operator
            )));
        }
        let expected = rule.value.as_ref().map_or_else(String::new, rule_json_text);
        let mut expected_number = 0.0;
        let mut expected_high = 0.0;
        let mut regex = None;
        let numeric_column = is_rule_numeric(&data_type);
        match rule.operator {
            RuleOperator::Isnull | RuleOperator::Notnull => {}
            RuleOperator::Eq | RuleOperator::Ne => {
                if !is_rule_comparable(&data_type) {
                    return Err(PlenoraError::Contract(format!(
                        "validate_rules: regola {}: tipo {data_type:?} non confrontabile",
                        rule.name
                    )));
                }
                if numeric_column {
                    expected_number = expected.parse::<f64>().map_err(|_| {
                        PlenoraError::Contract(format!(
                            "validate_rules: regola {}: confronto numerico con valore non numerico",
                            rule.name
                        ))
                    })?;
                }
            }
            RuleOperator::Gt | RuleOperator::Ge | RuleOperator::Lt | RuleOperator::Le => {
                if !numeric_column {
                    return Err(PlenoraError::Contract(format!(
                        "validate_rules: regola {}: confronto ordinato richiede colonna numerica (tipo {data_type:?})",
                        rule.name
                    )));
                }
                expected_number = expected.parse::<f64>().map_err(|_| {
                    PlenoraError::Contract(format!(
                        "validate_rules: regola {}: confronto ordinato richiede un valore numerico",
                        rule.name
                    ))
                })?;
            }
            RuleOperator::Range => {
                if !numeric_column {
                    return Err(PlenoraError::Contract(format!(
                        "validate_rules: regola {}: range richiede colonna numerica (tipo {data_type:?})",
                        rule.name
                    )));
                }
                let Some((low, high)) = expected.split_once(',') else {
                    return Err(PlenoraError::Contract(format!(
                        "validate_rules: regola {}: range richiede min,max",
                        rule.name
                    )));
                };
                let low = low.trim().parse::<f64>().map_err(|_| {
                    PlenoraError::Contract(format!(
                        "validate_rules: regola {}: estremi range non numerici",
                        rule.name
                    ))
                })?;
                let high = high.trim().parse::<f64>().map_err(|_| {
                    PlenoraError::Contract(format!(
                        "validate_rules: regola {}: estremi range non numerici",
                        rule.name
                    ))
                })?;
                expected_number = low;
                expected_high = high;
            }
            RuleOperator::Regex => {
                if data_type != DataType::Utf8 {
                    return Err(PlenoraError::Contract(format!(
                        "validate_rules: regola {}: regex richiede colonna Utf8 (tipo {data_type:?})",
                        rule.name
                    )));
                }
                regex = Some(regex::Regex::new(&expected).map_err(|error| {
                    PlenoraError::Contract(format!(
                        "validate_rules: regola {}: regex non valida: {error}",
                        rule.name
                    ))
                })?);
            }
        }
        compiled.push(CompiledRule {
            name: rule.name.clone(),
            operator: rule.operator,
            column_index,
            numeric_column,
            severity: rule.severity,
            expected,
            expected_number,
            expected_high,
            regex,
        });
    }
    Ok(compiled)
}

/// Valuta una regola su una riga. MAI un errore sui dati: qualunque valore
/// non interpretabile (incluso null per gli operatori a valore) e' un
/// fallimento della regola, non un errore del kernel.
fn rule_passes(batch: &RecordBatch, rule: &CompiledRule, row: usize) -> bool {
    let array = batch.column(rule.column_index).as_ref();
    match rule.operator {
        RuleOperator::Isnull => return array.is_null(row),
        RuleOperator::Notnull => return !array.is_null(row),
        _ if array.is_null(row) => return false,
        _ => {}
    }
    match rule.operator {
        RuleOperator::Eq | RuleOperator::Ne => {
            let equal = if rule.numeric_column {
                scalar_as_f64(array, row).ok().flatten().is_some_and(|actual| {
                    actual == rule.expected_number || (actual.is_nan() && rule.expected_number.is_nan())
                })
            } else {
                scalar_as_string(array, row)
                    .ok()
                    .flatten()
                    .is_some_and(|actual| actual == rule.expected)
            };
            matches!(rule.operator, RuleOperator::Ne) != equal
        }
        RuleOperator::Gt | RuleOperator::Ge | RuleOperator::Lt | RuleOperator::Le => {
            scalar_as_f64(array, row).ok().flatten().is_some_and(|actual| {
                match rule.operator {
                    RuleOperator::Gt => actual > rule.expected_number,
                    RuleOperator::Ge => actual >= rule.expected_number,
                    RuleOperator::Lt => actual < rule.expected_number,
                    RuleOperator::Le => actual <= rule.expected_number,
                    _ => unreachable!(),
                }
            })
        }
        RuleOperator::Range => scalar_as_f64(array, row)
            .ok()
            .flatten()
            .is_some_and(|actual| actual >= rule.expected_number && actual <= rule.expected_high),
        RuleOperator::Regex => rule.regex.as_ref().is_some_and(|regex| {
            scalar_as_string(array, row)
                .ok()
                .flatten()
                .is_some_and(|actual| regex.is_match(&actual))
        }),
        RuleOperator::Isnull | RuleOperator::Notnull => unreachable!(),
    }
}

/// Valuta tutte le regole su tutte le righe; restituisce per riga gli
/// INDICI delle regole fallite per gravita' (nomi risolti dal chiamante).
fn evaluate_rules(
    batch: &RecordBatch,
    rules: &[CompiledRule],
) -> (Vec<bool>, Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let mut valid = Vec::with_capacity(batch.num_rows());
    let mut errors: Vec<Vec<usize>> = Vec::with_capacity(batch.num_rows());
    let mut warnings: Vec<Vec<usize>> = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let mut row_errors = Vec::new();
        let mut row_warnings = Vec::new();
        for (index, rule) in rules.iter().enumerate() {
            if !rule_passes(batch, rule, row) {
                match rule.severity {
                    RuleSeverity::Error => row_errors.push(index),
                    RuleSeverity::Warning => row_warnings.push(index),
                }
            }
        }
        valid.push(row_errors.is_empty());
        errors.push(row_errors);
        warnings.push(row_warnings);
    }
    (valid, errors, warnings)
}

/// Valida le righe contro un set di regole dichiarative (estensione v1.2).
/// NON fallisce mai sui dati: `annotate` aggiunge `_valid` (Boolean, false
/// se almeno una regola error e' fallita), `_errors` e `_warnings` (nomi
/// delle regole fallite separati da `;`, stringa vuota se nessuna);
/// `summary` emette una riga per regola (`name`, `errors`, `warnings` con i
/// conteggi delle righe fallite per gravita').
pub fn validate_rules(batch: &RecordBatch, config: &ValidateRules) -> Result<RecordBatch> {
    let rules = compile_rules(batch, config)?;
    let (valid, errors, warnings) = evaluate_rules(batch, &rules);
    let join_names = |indices: &[usize]| {
        indices
            .iter()
            .map(|index| rules[*index].name.as_str())
            .collect::<Vec<_>>()
            .join(";")
    };
    match config.output_mode {
        ValidateOutputMode::Annotate => {
            let result = replace_or_append(
                batch,
                "_valid",
                DataType::Boolean,
                false,
                Arc::new(BooleanArray::from(valid)),
            )?;
            let result = replace_or_append(
                &result,
                "_errors",
                DataType::Utf8,
                false,
                Arc::new(StringArray::from(
                    errors
                        .iter()
                        .map(|indices| join_names(indices))
                        .collect::<Vec<_>>(),
                )),
            )?;
            replace_or_append(
                &result,
                "_warnings",
                DataType::Utf8,
                false,
                Arc::new(StringArray::from(
                    warnings
                        .iter()
                        .map(|indices| join_names(indices))
                        .collect::<Vec<_>>(),
                )),
            )
        }
        ValidateOutputMode::Summary => {
            let mut error_counts = vec![0_i64; rules.len()];
            let mut warning_counts = vec![0_i64; rules.len()];
            for row_errors in &errors {
                for index in row_errors {
                    error_counts[*index] += 1;
                }
            }
            for row_warnings in &warnings {
                for index in row_warnings {
                    warning_counts[*index] += 1;
                }
            }
            Ok(RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("name", DataType::Utf8, false),
                    Field::new("errors", DataType::Int64, false),
                    Field::new("warnings", DataType::Int64, false),
                ])),
                vec![
                    Arc::new(StringArray::from(
                        rules.iter().map(|rule| rule.name.as_str()).collect::<Vec<_>>(),
                    )),
                    Arc::new(Int64Array::from(error_counts)),
                    Arc::new(Int64Array::from(warning_counts)),
                ],
            )?)
        }
    }
}


#[cfg(test)]
mod tests {
    // -------------------------------------------------------------------
    // Test-oracolo di `reconcile`/`assert_foreign_key` (batch 4
    // ottimizzazioni kernel): le implementazioni pre-ottimizzazione sono
    // copiate verbatim qui sotto come riferimento indipendente, e i byte
    // delle chiavi dell'encoder sono confrontati direttamente con
    // `quality::key_for_row` (rimasta invariata).
    // -------------------------------------------------------------------

    use super::*;
    use crate::quality::key_for_row;
    use plenora_core::arrow::array::{
        BinaryArray, Date32Array, Decimal128Array, TimestampMillisecondArray,
    };

    /// Copia verbatim dell'implementazione di `frequencies`
    /// pre-ottimizzazione (riferimento di `reconcile_reference`).
    fn frequencies_reference(
        batch: &RecordBatch,
        indices: &[usize],
        nulls_equal: bool,
        side_nulls: &mut usize,
        memory_used: &mut usize,
        limits: &Limits,
    ) -> Result<HashMap<Vec<u8>, usize>> {
        let mut output: HashMap<Vec<u8>, usize> = HashMap::new();
        for row in 0..batch.num_rows() {
            if !nulls_equal && has_null(batch, indices, row) {
                *side_nulls = side_nulls
                    .checked_add(1)
                    .ok_or_else(|| PlenoraError::Contract("overflow null reconciliation".into()))?;
                continue;
            }
            let key = key_for_row(batch, indices, row)?;
            if let Some(count) = output.get_mut(&key) {
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| PlenoraError::Contract("overflow reconciliation".into()))?;
            } else {
                *memory_used = memory_used
                    .checked_add(key.len().saturating_add(64))
                    .ok_or_else(|| {
                        PlenoraError::Contract("overflow memoria reconciliation".into())
                    })?;
                if *memory_used > limits.max_memory_bytes {
                    return Err(PlenoraError::Contract(
                        "reconcile oltre max_memory_bytes".into(),
                    ));
                }
                output.insert(key, 1);
                if output.len() > limits.max_rows {
                    return Err(PlenoraError::Contract(
                        "reconcile supera max_rows chiavi distinte".into(),
                    ));
                }
            }
        }
        Ok(output)
    }

    /// Copia verbatim dell'implementazione di `reconcile` pre-ottimizzazione.
    fn reconcile_reference(
        left: &RecordBatch,
        right: &RecordBatch,
        config: &Reconcile,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        let left_indices = key_indices(left, &config.left_keys)?;
        let right_indices = key_indices(right, &config.right_keys)?;
        validate_key_types(left, right, &left_indices, &right_indices)?;
        let mut left_nulls = 0;
        let mut right_nulls = 0;
        let mut memory_used = 0_usize;
        let left_counts = frequencies_reference(
            left,
            &left_indices,
            config.nulls_equal,
            &mut left_nulls,
            &mut memory_used,
            limits,
        )?;
        let right_counts = frequencies_reference(
            right,
            &right_indices,
            config.nulls_equal,
            &mut right_nulls,
            &mut memory_used,
            limits,
        )?;
        let mut matched = 0_usize;
        let mut left_only = left_nulls;
        let mut right_only = right_nulls;
        let mut left_duplicates = 0_usize;
        let mut right_duplicates = 0_usize;
        for (key, left_count) in &left_counts {
            let right_count = right_counts.get(key).copied().unwrap_or_default();
            let common = (*left_count).min(right_count);
            matched = matched.saturating_add(common);
            left_only = left_only.saturating_add(left_count - common);
            left_duplicates = left_duplicates.saturating_add(left_count.saturating_sub(1));
        }
        for (key, right_count) in &right_counts {
            let left_count = left_counts.get(key).copied().unwrap_or_default();
            right_only = right_only.saturating_add(right_count.saturating_sub(left_count));
            right_duplicates = right_duplicates.saturating_add(right_count.saturating_sub(1));
        }
        let metrics = [
            "matched_rows",
            "left_only_rows",
            "right_only_rows",
            "left_duplicate_rows",
            "right_duplicate_rows",
        ];
        let values = [
            matched,
            left_only,
            right_only,
            left_duplicates,
            right_duplicates,
        ]
        .into_iter()
        .map(as_u64)
        .collect::<Result<Vec<_>>>()?;
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("metric", DataType::Utf8, false),
                Field::new("value", DataType::UInt64, false),
            ])),
            vec![
                Arc::new(StringArray::from(metrics.to_vec())),
                Arc::new(UInt64Array::from(values)),
            ],
        )?)
    }

    /// Copia verbatim dell'implementazione di `assert_foreign_key`
    /// pre-ottimizzazione.
    fn assert_foreign_key_reference(
        left: &RecordBatch,
        right: &RecordBatch,
        config: &ForeignKey,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        let left_indices = key_indices(left, &config.left_keys)?;
        let right_indices = key_indices(right, &config.right_keys)?;
        validate_key_types(left, right, &left_indices, &right_indices)?;
        let mut referenced = HashSet::with_capacity(right.num_rows());
        let mut memory_used = 0_usize;
        for row in 0..right.num_rows() {
            if !has_null(right, &right_indices, row) {
                let key = key_for_row(right, &right_indices, row)?;
                let key_bytes = key.len();
                if referenced.insert(key) {
                    memory_used = memory_used
                        .checked_add(key_bytes.saturating_add(64))
                        .ok_or_else(|| {
                            PlenoraError::Contract("overflow memoria foreign key".into())
                        })?;
                    if memory_used > limits.max_memory_bytes {
                        return Err(PlenoraError::Contract(
                            "assert_foreign_key oltre max_memory_bytes".into(),
                        ));
                    }
                }
            }
        }
        for row in 0..left.num_rows() {
            if has_null(left, &left_indices, row) {
                if config.allow_null {
                    continue;
                }
                return Err(PlenoraError::Contract(format!(
                    "assert_foreign_key: chiave null alla riga {row}"
                )));
            }
            if !referenced.contains(&key_for_row(left, &left_indices, row)?) {
                return Err(PlenoraError::Contract(format!(
                    "assert_foreign_key: riferimento mancante alla riga {row}"
                )));
            }
        }
        Ok(left.clone())
    }

    fn batch_of(fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("fixture")
    }

    /// Fixture con chiavi composite su tipi misti (int64 + utf8 nullable)
    /// con duplicati e null.
    fn mixed_batch(ids: Vec<Option<i64>>, tags: Vec<Option<&str>>) -> RecordBatch {
        batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("tag", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(tags)),
            ],
        )
    }

    /// Confronto rigoroso dell'output di `reconcile`: schema, righe, valori.
    fn assert_reconcile_identical(fast: &RecordBatch, reference: &RecordBatch) {
        assert_eq!(fast.num_rows(), reference.num_rows(), "righe");
        assert_eq!(fast.num_columns(), reference.num_columns(), "colonne");
        let metrics_fast = fast
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("metric fast");
        let metrics_ref = reference
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("metric ref");
        let values_fast = fast
            .column(1)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("value fast");
        let values_ref = reference
            .column(1)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("value ref");
        for row in 0..fast.num_rows() {
            assert_eq!(metrics_fast.value(row), metrics_ref.value(row), "metrica {row}");
            assert_eq!(values_fast.value(row), values_ref.value(row), "valore {row}");
        }
    }

    #[test]
    fn row_key_encoder_bytes_match_key_for_row_on_all_types() {
        let timestamp = TimestampMillisecondArray::from(vec![
            Some(0),
            None,
            Some(-1_000),
            Some(1_700_000_000_000),
        ]);
        let decimal = Decimal128Array::from(vec![Some(12_345), None, Some(-1), Some(0)])
            .with_precision_and_scale(38, 2)
            .expect("decimal");
        let batch = batch_of(
            vec![
                Field::new("i", DataType::Int64, true),
                Field::new("s", DataType::Utf8, true),
                Field::new("f", DataType::Float64, true),
                Field::new("b", DataType::Boolean, true),
                Field::new("u", DataType::UInt64, true),
                Field::new("d", DataType::Date32, true),
                Field::new("t", timestamp.data_type().clone(), true),
                Field::new("m", decimal.data_type().clone(), true),
                Field::new("x", DataType::Binary, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![Some(1), None, Some(-5), Some(i64::MAX)])),
                Arc::new(StringArray::from(vec![Some("a"), None, Some("héllo"), Some("")])),
                Arc::new(Float64Array::from(vec![
                    Some(f64::NAN),
                    Some(-0.0),
                    Some(f64::INFINITY),
                    Some(1.5),
                ])),
                Arc::new(BooleanArray::from(vec![Some(true), None, Some(false), Some(true)])),
                Arc::new(UInt64Array::from(vec![Some(0), None, Some(u64::MAX), Some(7)])),
                Arc::new(Date32Array::from(vec![Some(0), None, Some(-1), Some(19_000)])),
                Arc::new(timestamp),
                Arc::new(decimal),
                Arc::new(BinaryArray::from(vec![
                    Some(&b"ab"[..]),
                    None,
                    Some(&b""[..]),
                    Some(&b"c"[..]),
                ])),
            ],
        );
        let indices: Vec<usize> = (0..batch.num_columns()).collect();
        let mut encoder = RowKeyEncoder::new(&batch, &indices);
        let mut key = Vec::new();
        for row in 0..batch.num_rows() {
            encoder.encode_into(row, &mut key).expect("encode");
            let reference = key_for_row(&batch, &indices, row).expect("key_for_row");
            assert_eq!(key, reference, "byte chiave riga {row}");
        }
    }

    #[test]
    fn reconcile_matches_reference_with_duplicates_and_nulls() {
        let left = mixed_batch(
            vec![Some(1), Some(2), Some(2), None, Some(3), Some(3), Some(3)],
            vec![Some("a"), Some("b"), Some("b"), Some("n"), None, None, None],
        );
        let right = mixed_batch(
            vec![Some(2), Some(2), Some(9), None, Some(3), Some(3)],
            vec![Some("b"), Some("b"), Some("z"), Some("n"), None, Some("x")],
        );
        for nulls_equal in [true, false] {
            let config = Reconcile {
                left_keys: vec!["id".into(), "tag".into()],
                right_keys: vec!["id".into(), "tag".into()],
                nulls_equal,
            };
            let fast = reconcile(&left, &right, &config, &Limits::default()).expect("fast");
            let reference =
                reconcile_reference(&left, &right, &config, &Limits::default()).expect("reference");
            assert_reconcile_identical(&fast, &reference);
        }
    }

    #[test]
    fn reconcile_matches_reference_on_empty_inputs() {
        let left = mixed_batch(Vec::new(), Vec::new());
        let right = mixed_batch(Vec::new(), Vec::new());
        let config = Reconcile {
            left_keys: vec!["id".into()],
            right_keys: vec!["id".into()],
            nulls_equal: true,
        };
        let fast = reconcile(&left, &right, &config, &Limits::default()).expect("fast");
        let reference =
            reconcile_reference(&left, &right, &config, &Limits::default()).expect("reference");
        assert_reconcile_identical(&fast, &reference);
    }

    #[test]
    fn reconcile_errors_match_reference() {
        let left = mixed_batch(
            vec![Some(1), Some(2), Some(3)],
            vec![Some("a"), Some("b"), Some("c")],
        );
        let right = mixed_batch(vec![Some(1)], vec![Some("a")]);
        let config = Reconcile {
            left_keys: vec!["id".into()],
            right_keys: vec!["id".into()],
            nulls_equal: true,
        };
        // max_memory_bytes esaurito: stesso errore nelle due versioni.
        let tight_memory = Limits {
            max_memory_bytes: 4,
            ..Limits::default()
        };
        let fast = reconcile(&left, &right, &config, &tight_memory).expect_err("fast err");
        let reference =
            reconcile_reference(&left, &right, &config, &tight_memory).expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "errore memoria");
        // max_rows sulle chiavi distinte: stesso errore.
        let tight_rows = Limits {
            max_rows: 2,
            ..Limits::default()
        };
        let fast = reconcile(&left, &right, &config, &tight_rows).expect_err("fast err");
        let reference =
            reconcile_reference(&left, &right, &config, &tight_rows).expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "errore max_rows");
        // Tipi chiave diversi: stesso errore di schema.
        let wrong = batch_of(
            vec![Field::new("id", DataType::Utf8, true)],
            vec![Arc::new(StringArray::from(vec![Some("1")]))],
        );
        let fast = reconcile(&left, &wrong, &config, &Limits::default()).expect_err("fast err");
        let reference =
            reconcile_reference(&left, &wrong, &config, &Limits::default()).expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "errore tipi");
    }

    #[test]
    fn assert_foreign_key_matches_reference() {
        let right = mixed_batch(
            vec![Some(1), Some(2), Some(2), None, Some(3)],
            vec![Some("a"), Some("b"), Some("b"), Some("n"), Some("c")],
        );
        let left = mixed_batch(
            vec![Some(2), Some(1), Some(3), Some(2)],
            vec![Some("b"), Some("a"), Some("c"), Some("b")],
        );
        let config = ForeignKey {
            left_keys: vec!["id".into(), "tag".into()],
            right_keys: vec!["id".into(), "tag".into()],
            allow_null: false,
        };
        let fast = assert_foreign_key(&left, &right, &config, &Limits::default()).expect("fast");
        let reference =
            assert_foreign_key_reference(&left, &right, &config, &Limits::default())
                .expect("reference");
        assert_eq!(fast.num_rows(), reference.num_rows());
        assert_eq!(fast.schema(), reference.schema());
    }

    #[test]
    fn assert_foreign_key_errors_match_reference() {
        let right = mixed_batch(vec![Some(1), Some(2)], vec![Some("a"), Some("b")]);
        // Riferimento mancante: stessa riga e stesso messaggio.
        let left = mixed_batch(vec![Some(1), Some(9)], vec![Some("a"), Some("z")]);
        let config = ForeignKey {
            left_keys: vec!["id".into()],
            right_keys: vec!["id".into()],
            allow_null: false,
        };
        let fast = assert_foreign_key(&left, &right, &config, &Limits::default())
            .expect_err("fast err");
        let reference = assert_foreign_key_reference(&left, &right, &config, &Limits::default())
            .expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "riferimento mancante");
        // Chiave null con allow_null=false: stesso messaggio.
        let left_null = mixed_batch(vec![Some(1), None], vec![Some("a"), Some("x")]);
        let fast = assert_foreign_key(&left_null, &right, &config, &Limits::default())
            .expect_err("fast err");
        let reference =
            assert_foreign_key_reference(&left_null, &right, &config, &Limits::default())
                .expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "chiave null");
        // allow_null=true: le null vengono saltate in entrambe.
        let config_allow = ForeignKey {
            left_keys: vec!["id".into()],
            right_keys: vec!["id".into()],
            allow_null: true,
        };
        assert!(assert_foreign_key(&left_null, &right, &config_allow, &Limits::default()).is_ok());
        assert!(
            assert_foreign_key_reference(&left_null, &right, &config_allow, &Limits::default())
                .is_ok()
        );
        // max_memory_bytes: stesso errore (contabilita' basata sui byte chiave).
        let tight = Limits {
            max_memory_bytes: 4,
            ..Limits::default()
        };
        let fast = assert_foreign_key(&left, &right, &config, &tight).expect_err("fast err");
        let reference =
            assert_foreign_key_reference(&left, &right, &config, &tight).expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "errore memoria");
    }

    // -------------------------------------------------------------------
    // table.validate_rules (estensione v1.2)
    // -------------------------------------------------------------------

    fn rules_batch() -> RecordBatch {
        batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("name", DataType::Utf8, true),
                Field::new("score", DataType::Float64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(-2), None, Some(4)])),
                Arc::new(StringArray::from(vec![
                    Some("alfa"),
                    Some("beta"),
                    Some(""),
                    None,
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(0.5),
                    Some(9.9),
                    Some(f64::NAN),
                    Some(-1.0),
                ])),
            ],
        )
    }

    fn rules(config: serde_json::Value) -> ValidateRules {
        serde_json::from_value(config).expect("config validate_rules")
    }

    fn annotated_strings(output: &RecordBatch, name: &str) -> Vec<String> {
        let column = output
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .expect("utf8");
        (0..column.len()).map(|row| column.value(row).to_owned()).collect()
    }

    #[test]
    fn validate_rules_annotate_never_fails_on_data() {
        let output = validate_rules(
            &rules_batch(),
            &rules(serde_json::json!({
                "rules": [
                    {"name": "id_pos", "operator": "gt", "column": "id", "value": 0},
                    {"name": "name_fmt", "operator": "regex", "column": "name", "value": "^[a-z]+$", "severity": "warning"},
                    {"name": "score_range", "operator": "range", "column": "score", "value": "0,10"},
                    {"name": "name_present", "operator": "notnull", "column": "name"}
                ]
            })),
        )
        .expect("annotate");
        let valid = output
            .column_by_name("_valid")
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
            .expect("_valid");
        let flags: Vec<bool> = (0..4).map(|row| valid.value(row)).collect();
        // riga 0: tutto ok; riga 1: id -2 fallisce; riga 2: id null fallisce
        // (NaN nello score fallisce il range); riga 3: name null -> warning +
        // notnull fallita, score -1 fuori range -> errore.
        assert_eq!(flags, [true, false, false, false]);
        assert_eq!(
            annotated_strings(&output, "_errors"),
            [
                "",
                "id_pos",
                "id_pos;score_range",
                "score_range;name_present"
            ]
        );
        assert_eq!(
            annotated_strings(&output, "_warnings"),
            ["", "", "name_fmt", "name_fmt"]
        );
        // Colonne non nullable, schema input preservato.
        let schema = output.schema();
        for name in ["_valid", "_errors", "_warnings"] {
            assert!(!schema.field_with_name(name).expect(name).is_nullable());
        }
        assert!(schema.field_with_name("id").is_ok());
    }

    #[test]
    fn validate_rules_summary_counts_per_rule() {
        let output = validate_rules(
            &rules_batch(),
            &rules(serde_json::json!({
                "output_mode": "summary",
                "rules": [
                    {"name": "id_pos", "operator": "gt", "column": "id", "value": 0},
                    {"name": "name_fmt", "operator": "regex", "column": "name", "value": "^[a-z]+$", "severity": "warning"},
                    {"name": "name_missing", "operator": "isnull", "column": "name", "severity": "warning"}
                ]
            })),
        )
        .expect("summary");
        assert_eq!(output.num_rows(), 3);
        assert_eq!(annotated_strings(&output, "name"), ["id_pos", "name_fmt", "name_missing"]);
        let errors = output
            .column_by_name("errors")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .expect("errors");
        let warnings = output
            .column_by_name("warnings")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .expect("warnings");
        assert_eq!((0..3).map(|r| errors.value(r)).collect::<Vec<_>>(), [2, 0, 0]);
        assert_eq!((0..3).map(|r| warnings.value(r)).collect::<Vec<_>>(), [0, 2, 3]);
    }

    #[test]
    fn validate_rules_static_errors_before_data() {
        let batch = rules_batch();
        // Regex invalida: errore di validazione, non a meta' dei dati.
        let error = validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "regex", "column": "name", "value": "("}]
            })),
        )
        .expect_err("regex invalida");
        assert!(error.to_string().contains("regex non valida"));
        // Confronto ordinato su colonna non numerica: errore di validazione.
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "gt", "column": "name", "value": 1}]
            })),
        )
        .is_err());
        // Regex su colonna non Utf8.
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "regex", "column": "id", "value": "1"}]
            })),
        )
        .is_err());
        // Eq numerica con valore non numerico; range malformato.
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "eq", "column": "id", "value": "abc"}]
            })),
        )
        .is_err());
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "range", "column": "id", "value": "1-2"}]
            })),
        )
        .is_err());
        // value mancante / non ammesso; colonna mancante; regole vuote;
        // nomi duplicati.
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "eq", "column": "id"}]
            })),
        )
        .is_err());
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "isnull", "column": "id", "value": 1}]
            })),
        )
        .is_err());
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "eq", "column": "missing", "value": 1}]
            })),
        )
        .is_err());
        assert!(validate_rules(&batch, &rules(serde_json::json!({"rules": []}))).is_err());
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [
                    {"name": "dup", "operator": "isnull", "column": "id"},
                    {"name": "dup", "operator": "notnull", "column": "id"}
                ]
            })),
        )
        .is_err());
        // Config strict: campo sconosciuto rifiutato.
        assert!(serde_json::from_value::<ValidateRules>(serde_json::json!({
            "rules": [{"name": "r", "operator": "eq", "column": "id", "value": 1, "surprise": 1}]
        }))
        .is_err());
    }

    #[test]
    fn validate_rules_eq_ne_and_empty_batch() {
        let output = validate_rules(
            &rules_batch(),
            &rules(serde_json::json!({
                "rules": [
                    {"name": "is_alfa", "operator": "eq", "column": "name", "value": "alfa"},
                    {"name": "not_beta", "operator": "ne", "column": "name", "value": "beta"}
                ]
            })),
        )
        .expect("eq/ne");
        assert_eq!(
            annotated_strings(&output, "_errors"),
            ["", "is_alfa;not_beta", "is_alfa", "is_alfa;not_beta"]
        );
        // Batch vuoto: annotate con zero righe, summary con i conteggi a zero.
        let empty = batch_of(
            vec![Field::new("id", DataType::Int64, true)],
            vec![Arc::new(Int64Array::from(Vec::<Option<i64>>::new()))],
        );
        let config = rules(serde_json::json!({
            "rules": [{"name": "r", "operator": "gt", "column": "id", "value": 0}]
        }));
        let annotated = validate_rules(&empty, &config).expect("empty annotate");
        assert_eq!(annotated.num_rows(), 0);
        assert_eq!(annotated.num_columns(), 4);
        let summary = validate_rules(
            &empty,
            &rules(serde_json::json!({
                "output_mode": "summary",
                "rules": [{"name": "r", "operator": "gt", "column": "id", "value": 0}]
            })),
        )
        .expect("empty summary");
        assert_eq!(summary.num_rows(), 1);
    }
}
