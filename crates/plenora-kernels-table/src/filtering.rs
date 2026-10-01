use std::cmp::Ordering;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use crate::{
    column_index, compare_f64, compare_i64, compare_u64, replace_or_append, scalar_as_string,
    scalar_compare, select_rows, NumericBound,
};
use plenora_core::{PlenoraError, Result};

/// Operatore di confronto di `table.filter` e `table.conditional`.
///
/// Una cella nulla (null logico, voce nulla di un dizionario compresa) non
/// soddisfa nessun operatore tranne `Isnull`. `value` vale il suo testo
/// JSON (una stringa com'e', `null` come testo vuoto).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operator {
    /// `"=="`: su `Int64` e `Float64` confronto numerico esatto con `value`
    /// (su `Float64` `0.0 == -0.0` e `NaN == NaN`); su ogni altro tipo
    /// confronto fra il testo della cella e il testo di `value`.
    #[serde(rename = "==")]
    Eq,
    /// `"!="`: negazione di `Eq` sulle celle non nulle.
    #[serde(rename = "!=")]
    Ne,
    /// `">"`: confronto ordinato nel dominio nativo del tipo
    /// ([`scalar_compare`]), `value` numerico; un `NaN` rende falso.
    #[serde(rename = ">")]
    Gt,
    /// `">="`: come `Gt`.
    #[serde(rename = ">=")]
    Ge,
    /// `"<"`: come `Gt`.
    #[serde(rename = "<")]
    Lt,
    /// `"<="`: come `Gt`.
    #[serde(rename = "<=")]
    Le,
    /// `"contains"`: il testo della cella contiene `value`, senza
    /// distinzione fra maiuscole e minuscole (minuscole Unicode).
    Contains,
    /// `"startswith"`: il testo della cella comincia con `value`.
    Startswith,
    /// `"endswith"`: il testo della cella finisce con `value`.
    Endswith,
    /// `"isnull"`: la cella e' nulla; `value` non conta.
    Isnull,
    /// `"notnull"`: la cella non e' nulla; `value` non conta.
    Notnull,
    /// `"between"`: `value` e' il testo `"min,max"`, estremi inclusi,
    /// confrontati come `Ge` e `Le`.
    Between,
}

/// Config di `table.filter`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    /// Colonna su cui si valuta la condizione (obbligatorio).
    pub column: String,
    /// Operatore di confronto (obbligatorio).
    pub operator: Operator,
    /// Termine di confronto (assente vale `null`, cioe' testo vuoto). Con
    /// `isnull` e `notnull` non ha effetto: scritto, anche `null`, si
    /// rifiuta ([`verifica_valore`]).
    #[serde(default, deserialize_with = "crate::cleansing::valore_scritto")]
    pub value: Option<serde_json::Value>,
}

impl Filter {
    /// Il termine di confronto (`null` se assente).
    #[must_use]
    pub fn valore(&self) -> &serde_json::Value {
        self.value.as_ref().unwrap_or(&serde_json::Value::Null)
    }
}

/// `value` con `isnull` o `notnull` non si legge: scritto (anche `null`) si
/// rifiuta invece di essere ignorato. La chiamano i kernel `filter` e
/// `conditional` e l'analisi dei contratti.
///
/// # Errors
///
/// `InvalidPlan` se `value` e' scritto con `isnull` o `notnull`.
pub fn verifica_valore(operator: &Operator, value: Option<&serde_json::Value>) -> Result<()> {
    if value.is_some() && matches!(operator, Operator::Isnull | Operator::Notnull) {
        return Err(PlenoraError::InvalidPlan(
            "value non ha effetto con isnull e notnull".into(),
        ));
    }
    Ok(())
}

/// Una condizione di `table.conditional`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// Operatore (default `"=="`), valutato come in `table.filter`.
    #[serde(default = "default_operator")]
    pub operator: Operator,
    /// Termine di confronto (assente vale `null`); con `isnull` e `notnull`
    /// scritto si rifiuta ([`verifica_valore`]).
    #[serde(default, deserialize_with = "crate::cleansing::valore_scritto")]
    pub value: Option<serde_json::Value>,
    /// Valore scritto se questa e' la prima condizione vera (default
    /// `null`); vale il suo testo JSON.
    #[serde(default)]
    pub result: serde_json::Value,
}

impl Condition {
    /// Il termine di confronto (`null` se assente).
    #[must_use]
    pub fn valore(&self) -> &serde_json::Value {
        self.value.as_ref().unwrap_or(&serde_json::Value::Null)
    }
}

const fn default_operator() -> Operator {
    Operator::Eq
}

/// Config di `table.conditional`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conditional {
    /// Colonna su cui si valutano le condizioni (obbligatorio).
    pub column: String,
    /// Condizioni in ordine di precedenza (obbligatorio; l'analisi rifiuta
    /// la lista vuota).
    pub conditions: Vec<Condition>,
    /// Valore delle righe senza condizioni vere (default `null`).
    #[serde(default)]
    pub default_value: serde_json::Value,
    /// Colonna d'uscita (default `"result"`); se esiste si sostituisce.
    #[serde(default = "default_output")]
    pub output_column: String,
}

fn default_output() -> String {
    "result".into()
}

fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(v) => v.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl Conditional {
    /// Un `result` o `default_value` che si legge come numero non finito
    /// (`"NaN"`, `"inf"`, `"1e999"`) renderebbe un `Float64` non finito:
    /// si rifiuta. La chiamano il kernel e l'analisi dei contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per il primo risultato numerico non finito.
    pub fn verifica_risultati(&self) -> Result<()> {
        let non_finito = |valore: &serde_json::Value| {
            json_text(valore)
                .replace(',', ".")
                .parse::<f64>()
                .is_ok_and(|numero| !numero.is_finite())
        };
        if self
            .conditions
            .iter()
            .map(|condition| &condition.result)
            .chain(std::iter::once(&self.default_value))
            .any(non_finito)
        {
            return Err(PlenoraError::InvalidPlan(
                "result o default_value numerico non finito".into(),
            ));
        }
        Ok(())
    }
}

/// Condizione di filtro con il valore atteso risolto UNA volta per batch
/// (hot path minimale: nessun parse del letterale per riga nel percorso generico).
///
/// Il parse avviene alla prima valutazione di riga e il risultato, successo
/// o errore, vale per tutte le successive: il punto di errore e' lo stesso
/// del parse per riga. `PlenoraError` non e' `Clone`, quindi la cella
/// conserva il messaggio e ricostruisce la variante `InvalidPlan`.
struct PreparedCondition {
    operator: Operator,
    /// `json_text` del valore di configurazione, calcolato al costruttore.
    expected: String,
    /// Letterale numerico parsato (rami Eq/Ne e ordinati), alla prima riga.
    bound: std::cell::OnceCell<std::result::Result<NumericBound, String>>,
    /// Estremi di `between` parsati, alla prima riga del ramo.
    between: std::cell::OnceCell<std::result::Result<(NumericBound, NumericBound), String>>,
    /// Forma minuscola per `contains`, alla prima riga del ramo.
    expected_lowercase: std::cell::OnceCell<String>,
    /// `json_text` del risultato (solo `conditional`): calcolato al
    /// costruttore, clonato per riga invece che ri-serializzato.
    result: String,
}

impl PreparedCondition {
    fn new(operator: &Operator, value: &serde_json::Value, result: &serde_json::Value) -> Self {
        Self {
            operator: operator.clone(),
            expected: json_text(value),
            bound: std::cell::OnceCell::new(),
            between: std::cell::OnceCell::new(),
            expected_lowercase: std::cell::OnceCell::new(),
            result: json_text(result),
        }
    }

    /// Il letterale numerico condiviso (parse alla prima riga, errore con
    /// testo identico al percorso per riga).
    fn numeric_bound(&self, message: &str) -> Result<NumericBound> {
        match self
            .bound
            .get_or_init(|| NumericBound::parse(&self.expected).ok_or_else(|| message.to_owned()))
        {
            Ok(bound) => Ok(*bound),
            Err(stored) => Err(PlenoraError::InvalidPlan(stored.clone())),
        }
    }

    /// Gli estremi di `between` condivisi (stessi tre errori del percorso
    /// per riga, nello stesso ordine di valutazione).
    fn between_bounds(&self) -> Result<(NumericBound, NumericBound)> {
        match self.between.get_or_init(|| {
            let expected = &self.expected;
            let Some((low, high)) = expected.split_once(',') else {
                return Err("between richiede min,max".to_owned());
            };
            let low = NumericBound::parse(low.trim())
                .ok_or_else(|| "min between non valido".to_owned())?;
            let high = NumericBound::parse(high.trim())
                .ok_or_else(|| "max between non valido".to_owned())?;
            Ok((low, high))
        }) {
            Ok(bounds) => Ok(*bounds),
            Err(stored) => Err(PlenoraError::InvalidPlan(stored.clone())),
        }
    }

    /// La forma minuscola del valore atteso per `contains`.
    fn expected_lowercase(&self) -> &str {
        self.expected_lowercase
            .get_or_init(|| self.expected.to_lowercase())
    }
}

#[allow(clippy::too_many_lines)] // dispatcher esaustivo per operatore: un solo corpo tiene allineati generico e fast path
fn evaluate(array: &dyn Array, row: usize, condition: &PreparedCondition) -> Result<bool> {
    let operator = &condition.operator;
    match operator {
        // Null LOGICO: per una dictionary una chiave valida puo' puntare a
        // una entry nulla, e `is_null` risponderebbe `false` su una riga che
        // e' nulla: `isnull` la perderebbe e `notnull` la terrebbe.
        Operator::Isnull => return Ok(crate::is_logically_null(array, row)),
        Operator::Notnull => return Ok(!crate::is_logically_null(array, row)),
        _ if crate::is_logically_null(array, row) => return Ok(false),
        _ => {}
    }
    let expected = condition.expected.as_str();
    match operator {
        Operator::Eq | Operator::Ne => {
            let equal = if array.data_type() == &DataType::Int64 {
                // Confronto esatto nativo: il letterale intero resta intero
                // (nessun collasso oltre 2^53); il misto intero<->double e'
                // esatto (vedi `NumericBound` in lib.rs).
                let bound =
                    condition.numeric_bound("confronto numerico con valore non numerico")?;
                let values = array
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| PlenoraError::Schema("array Int64 incoerente".into()))?;
                compare_i64(values.value(row), bound) == Some(Ordering::Equal)
            } else if array.data_type() == &DataType::Float64 {
                // Semantica sui double (`total_cmp`, 0.0 == -0.0,
                // NaN uguale a NaN); un letterale intero oltre 2^53 usa il
                // confronto misto esatto invece dell'arrotondamento a f64.
                let bound =
                    condition.numeric_bound("confronto numerico con valore non numerico")?;
                let values = array
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| PlenoraError::Schema("array Float64 incoerente".into()))?;
                let actual = values.value(row);
                match bound {
                    NumericBound::F64(number) => {
                        actual.total_cmp(&number) == Ordering::Equal
                            || (actual.abs().total_cmp(&0.0) == Ordering::Equal
                                && number.abs().total_cmp(&0.0) == Ordering::Equal)
                    }
                    bound => compare_f64(actual, bound) == Some(Ordering::Equal),
                }
            } else {
                scalar_as_string(array, row)?.is_some_and(|actual| actual == expected)
            };
            Ok(if matches!(operator, Operator::Ne) {
                !equal
            } else {
                equal
            })
        }
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
            let bound =
                condition.numeric_bound("confronto ordinato richiede un valore numerico")?;
            // Confronto nel dominio nativo di ogni tipo (Int64/UInt64/Float64,
            // Date32, Date64 e Timestamp di ogni unita' nel valore nativo,
            // Decimal128 scalato in i128, Utf8 numerico
            // via `NumericBound`): nessun passaggio per f64, quindi nessun
            // collasso oltre 2^53 e nessun arrotondamento sui decimal.
            let ordering = scalar_compare(array, row, bound)?;
            let operator = OrderedOperator::from_operator(operator).ok_or_else(|| {
                PlenoraError::Internal("operatore non ordinato nel ramo ordinato".into())
            })?;
            Ok(ordered_typed(ordering, operator))
        }
        Operator::Contains | Operator::Startswith | Operator::Endswith => {
            let actual = scalar_as_string(array, row)?.unwrap_or_default();
            Ok(match operator {
                Operator::Contains => actual
                    .to_lowercase()
                    .contains(condition.expected_lowercase()),
                Operator::Startswith => actual.starts_with(expected),
                Operator::Endswith => actual.ends_with(expected),
                _ => {
                    return Err(PlenoraError::Internal(
                        "operatore non testuale nel ramo testuale".into(),
                    ));
                }
            })
        }
        Operator::Between => {
            let (low, high) = condition.between_bounds()?;
            Ok(within_bounds(
                scalar_compare(array, row, low)?,
                scalar_compare(array, row, high)?,
            ))
        }
        Operator::Isnull | Operator::Notnull => Err(PlenoraError::Internal(
            "isnull/notnull sono valutati prima del confronto scalare".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Fast path tipizzati di `table.filter`.
//
// Confronto sui valori nativi, con semantica identica a `evaluate`: null,
// NaN, -0.0, confronto esatto per Int64/UInt64 oltre 2^53, confronto misto
// via `NumericBound`, ordine delle righe. `fast_rows` rende `None` quando
// tipo, operatore o valore di confronto non sono coperti, e il chiamante
// ricade sul percorso generico riga per riga.
// ---------------------------------------------------------------------------

/// Righe non nulle di `array` per cui `pred` e' vera, in ordine crescente.
fn rows_where<A: Array>(array: &A, mut pred: impl FnMut(usize) -> bool) -> Vec<usize> {
    (0..array.len())
        .filter(|row| !crate::is_logically_null(array, *row) && pred(*row))
        .collect()
}

/// Uguaglianza numerica di `evaluate` sui double: `total_cmp`, con
/// `0.0 == -0.0` (i valori assoluti nulli sono considerati uguali) e NaN
/// uguale a NaN.
fn numeric_eq(actual: f64, expected: f64) -> bool {
    actual.total_cmp(&expected) == Ordering::Equal || (actual == 0.0 && expected == 0.0)
}

/// Sottoinsieme ordinato di `Operator` (`>`/`>=`/`<`/`<=`).
///
/// Codificato nel tipo: cosi' `ordered_typed` e' totale e il compilatore
/// dimostra che i chiamanti passano solo operatori ordinati (invariante
/// interna, senza un ramo di panico).
#[derive(Clone, Copy)]
enum OrderedOperator {
    Gt,
    Ge,
    Lt,
    Le,
}

impl OrderedOperator {
    /// Conversione dal generico: `None` se l'operatore non e' ordinato
    /// (invariante violata dal chiamante, segnalata come errore Internal).
    const fn from_operator(operator: &Operator) -> Option<Self> {
        match operator {
            Operator::Gt => Some(Self::Gt),
            Operator::Ge => Some(Self::Ge),
            Operator::Lt => Some(Self::Lt),
            Operator::Le => Some(Self::Le),
            _ => None,
        }
    }
}

/// Confronto ordinato condiviso (`>`/`>=`/`<`/`<=`): `None` (NaN) rende falso
/// ogni confronto, come nell'IEEE 754.
const fn ordered_typed(ordering: Option<Ordering>, operator: OrderedOperator) -> bool {
    match operator {
        OrderedOperator::Gt => matches!(ordering, Some(Ordering::Greater)),
        OrderedOperator::Ge => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        OrderedOperator::Lt => matches!(ordering, Some(Ordering::Less)),
        OrderedOperator::Le => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
    }
}

/// `between`: il valore non e' sotto il minimo ne' sopra il massimo; `None`
/// (estremo o valore NaN) esclude la riga, come `>=`/`<=` IEEE.
const fn within_bounds(low: Option<Ordering>, high: Option<Ordering>) -> bool {
    !matches!(low, None | Some(Ordering::Less)) && !matches!(high, None | Some(Ordering::Greater))
}

#[allow(clippy::too_many_lines)] // specchio di `evaluate`: la simmetria riga a riga e' la garanzia di parita'
fn fast_rows(
    array: &ArrayRef,
    operator: &Operator,
    value: &serde_json::Value,
) -> Option<Result<Vec<usize>>> {
    let rows = match operator {
        // Stessa nozione di null del percorso generico: due risposte diverse
        // sulla stessa riga sarebbero peggio di entrambe.
        Operator::Isnull => {
            return Some(Ok((0..array.len())
                .filter(|r| crate::is_logically_null(array.as_ref(), *r))
                .collect()))
        }
        Operator::Notnull => {
            return Some(Ok((0..array.len())
                .filter(|r| !crate::is_logically_null(array.as_ref(), *r))
                .collect()))
        }
        Operator::Eq | Operator::Ne => {
            let negate = matches!(operator, Operator::Ne);
            if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                // Il ramo numerico del generico fallisce il parse solo su
                // righe non nulle: in caso di valore non numerico si ricade
                // sul generico, che riproduce errore o selezione vuota.
                let bound = NumericBound::parse(&json_text(value))?;
                rows_where(values, |row| {
                    (compare_i64(values.value(row), bound) == Some(Ordering::Equal)) != negate
                })
            } else if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
                let bound = NumericBound::parse(&json_text(value))?;
                rows_where(values, |row| {
                    let equal = match bound {
                        NumericBound::F64(number) => numeric_eq(values.value(row), number),
                        bound => compare_f64(values.value(row), bound) == Some(Ordering::Equal),
                    };
                    equal != negate
                })
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                // Il generico confronta UInt64 come stringa (`to_string`):
                // il fast path vale solo per la forma decimale canonica.
                let expected = json_text(value);
                let parsed = expected.parse::<u64>().ok()?;
                if parsed.to_string() != expected {
                    return None;
                }
                rows_where(values, |row| (values.value(row) == parsed) != negate)
            } else if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
                let expected = json_text(value);
                rows_where(values, |row| (values.value(row) == expected) != negate)
            } else {
                // Ultimo tipo della catena: downcast fallito = tipo non
                // gestito, quindi `None`.
                let values = array.as_any().downcast_ref::<BooleanArray>()?;
                // Il generico confronta "true"/"false": equivalente al
                // confronto nativo sui soli valori booleani possibili.
                let expected = json_text(value);
                rows_where(values, |row| {
                    ((expected == "true" && values.value(row))
                        || (expected == "false" && !values.value(row)))
                        != negate
                })
            }
        }
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
            let bound = NumericBound::parse(&json_text(value))?;
            let Some(operator) = OrderedOperator::from_operator(operator) else {
                return Some(Err(PlenoraError::Internal(
                    "operatore non ordinato nel ramo ordinato".into(),
                )));
            };
            if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                rows_where(values, |row| {
                    ordered_typed(compare_i64(values.value(row), bound), operator)
                })
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                rows_where(values, |row| {
                    ordered_typed(compare_u64(values.value(row), bound), operator)
                })
            } else {
                // Ultimo tipo della catena: downcast fallito = tipo non
                // gestito, quindi `None`.
                let values = array.as_any().downcast_ref::<Float64Array>()?;
                rows_where(values, |row| {
                    ordered_typed(compare_f64(values.value(row), bound), operator)
                })
            }
        }
        Operator::Between => {
            let expected = json_text(value);
            let (low, high) = expected.split_once(',')?;
            let low = NumericBound::parse(low.trim())?;
            let high = NumericBound::parse(high.trim())?;
            if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
                rows_where(values, |row| {
                    within_bounds(
                        compare_i64(values.value(row), low),
                        compare_i64(values.value(row), high),
                    )
                })
            } else if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
                rows_where(values, |row| {
                    within_bounds(
                        compare_u64(values.value(row), low),
                        compare_u64(values.value(row), high),
                    )
                })
            } else {
                // Ultimo tipo della catena: downcast fallito = tipo non
                // gestito, quindi `None`.
                let values = array.as_any().downcast_ref::<Float64Array>()?;
                rows_where(values, |row| {
                    within_bounds(
                        compare_f64(values.value(row), low),
                        compare_f64(values.value(row), high),
                    )
                })
            }
        }
        Operator::Contains | Operator::Startswith | Operator::Endswith => {
            let values = array.as_any().downcast_ref::<StringArray>()?;
            let expected = json_text(value);
            match operator {
                Operator::Contains => {
                    let needle = expected.to_lowercase();
                    rows_where(values, |row| {
                        values.value(row).to_lowercase().contains(&needle)
                    })
                }
                Operator::Startswith => {
                    rows_where(values, |row| values.value(row).starts_with(&expected))
                }
                Operator::Endswith => {
                    rows_where(values, |row| values.value(row).ends_with(&expected))
                }
                _ => {
                    return Some(Err(PlenoraError::Internal(
                        "solo operatori testuali".into(),
                    )));
                }
            }
        }
    };
    Some(Ok(rows))
}

/// Batch con le sole righe che soddisfano la condizione, nell'ordine
/// d'ingresso (`table.filter`).
///
/// # Errors
///
/// - `Schema`: colonna assente; tipo della colonna che l'operatore non sa
///   leggere (via [`scalar_as_string`] o [`scalar_compare`]); cella `Utf8`
///   non numerica sotto un operatore ordinato o `between`;
/// - `InvalidPlan`: valore di confronto non numerico per i confronti
///   numerici o ordinati; `between` senza estremi `min,max` validi; `value`
///   scritto con `isnull` o `notnull` ([`verifica_valore`]);
/// - `ResourceLimit`: riga tenuta con indice oltre `u32::MAX`
///   ([`select_rows`]);
/// - `DataMapping`: errore Arrow nella selezione delle righe;
/// - `Internal`: invariante interna violata.
pub fn filter(batch: &RecordBatch, config: &Filter) -> Result<RecordBatch> {
    verifica_valore(&config.operator, config.value.as_ref())?;
    let index = column_index(batch, &config.column)?;
    let array = batch.column(index);
    let rows = if let Some(result) = fast_rows(array, &config.operator, config.valore()) {
        result?
    } else {
        // Il valore atteso e' risolto una volta per batch (hot path minimale): il
        // primo errore di parse scatta alla prima riga valutata, come
        // nel percorso per riga.
        let condition = PreparedCondition::new(&config.operator, config.valore(), config.valore());
        (0..batch.num_rows())
            .filter_map(|row| match evaluate(array.as_ref(), row, &condition) {
                Ok(true) => Some(Ok(row)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?
    };
    select_rows(batch, &rows)
}

/// Il tipo d'uscita di `table.conditional` e, se numerico, il valore di ogni
/// testo di risultato: l'unica regola, del kernel e dell'analisi.
///
/// Numerico se ogni testo e' vuoto (null) o un numero per il parse `f64`
/// (virgola decimale ammessa). Un risultato scritto come **intero** che il
/// double non rappresenta esattamente (oltre 2^53) e' un errore: diventando
/// il double piu' vicino sarebbe un altro intero, senza errore. Un decimale
/// diventa il double piu' vicino, come ogni `Float64`.
///
/// Rende `None` se l'uscita e' testuale, altrimenti i valori nell'ordine
/// dei testi.
///
/// # Errors
///
/// `InvalidPlan` per un risultato intero non rappresentabile in `Float64`.
pub fn risultati_numerici<'a>(
    testi: impl IntoIterator<Item = &'a str>,
) -> Result<Option<Vec<Option<f64>>>> {
    // Due passate: il tipo si decide su tutti i testi prima di giudicare
    // l'esattezza, che conta solo se l'uscita e' numerica.
    let normalizzati = testi
        .into_iter()
        .map(|testo| testo.replace(',', "."))
        .collect::<Vec<_>>();
    if !normalizzati
        .iter()
        .all(|testo| testo.is_empty() || testo.parse::<f64>().is_ok())
    {
        return Ok(None);
    }
    let mut numeri = Vec::with_capacity(normalizzati.len());
    for normalizzato in normalizzati {
        if normalizzato.is_empty() {
            numeri.push(None);
            continue;
        }
        let Ok(valore) = normalizzato.parse::<f64>() else {
            return Ok(None);
        };
        let intero_inesatto = intero_scritto(&normalizzato).map_or_else(
            || {
                matches!(
                    NumericBound::parse(&normalizzato),
                    Some(NumericBound::Decimal { unscaled, scale: 0 })
                        if crate::exact_f64_from_i128(unscaled).is_none()
                )
            },
            // Il double e' esatto se la sua espansione decimale completa
            // (`{:.0}` scrive tutte le cifre) e' l'intero scritto: vale per
            // interi di qualunque lunghezza, anche oltre `i128`.
            |cifre| format!("{:.0}", valore.abs()) != cifre,
        );
        if intero_inesatto {
            return Err(PlenoraError::InvalidPlan(
                "conditional: un risultato intero non e' rappresentabile esattamente in Float64"
                    .into(),
            ));
        }
        numeri.push(Some(valore));
    }
    Ok(Some(numeri))
}

/// Le cifre di un testo scritto come intero (segno facoltativo, sole cifre),
/// senza segno e zeri iniziali; `None` per ogni altra forma.
fn intero_scritto(testo: &str) -> Option<String> {
    let (_, cifre) = crate::separa_segno(testo);
    if cifre.is_empty() || !cifre.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let senza_zeri = cifre.trim_start_matches('0');
    Some(if senza_zeri.is_empty() {
        "0".to_owned()
    } else {
        senza_zeri.to_owned()
    })
}

/// Colonna calcolata dalla prima condizione vera di ogni riga, o da
/// `default_value` (`table.conditional`).
///
/// Il tipo d'uscita dipende solo dai letterali: se ogni `result` e il
/// `default_value`, letti come testo, sono vuoti o numeri (virgola
/// decimale ammessa: `"1,5"` vale 1,5), l'uscita e' `Float64` nullable e il
/// testo vuoto da' null; altrimenti e' `Utf8` non nullable e il null da'
/// `""`. Un risultato intero oltre la precisione del double si rifiuta
/// ([`risultati_numerici`]).
///
/// # Errors
///
/// - `Schema`: colonna assente; tipo della colonna che gli operatori non
///   sanno leggere; cella `Utf8` non numerica sotto un operatore ordinato;
/// - `InvalidPlan`: come [`filter`] per la valutazione delle condizioni
///   (valore di confronto non numerico, `between` malformato); un risultato
///   intero non rappresentabile esattamente in `Float64`;
/// - `DataMapping`: errore Arrow nella sostituzione (guardia interna, non
///   attesa);
/// - `Internal`: invariante interna violata.
pub fn conditional(batch: &RecordBatch, config: &Conditional) -> Result<RecordBatch> {
    for condition in &config.conditions {
        verifica_valore(&condition.operator, condition.value.as_ref())?;
    }
    config.verifica_risultati()?;
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    // Valori attesi e testi di risultato risolti una volta per batch (hot path minimale):
    // per riga restano solo valutazione e clone della stringa di output.
    let conditions: Vec<PreparedCondition> = config
        .conditions
        .iter()
        .map(|condition| {
            PreparedCondition::new(&condition.operator, condition.valore(), &condition.result)
        })
        .collect();
    let default_text = json_text(&config.default_value);
    // Il tipo e i valori numerici si decidono sui letterali, prima dei dati:
    // un risultato intero inesatto si rifiuta anche su un batch vuoto.
    let numerici = risultati_numerici(
        conditions
            .iter()
            .map(|condition| condition.result.as_str())
            .chain(std::iter::once(default_text.as_str())),
    )?;
    // Per riga, l'indice del risultato scelto: le condizioni, poi il default.
    let scelte = (0..batch.num_rows())
        .map(|row| {
            for (indice, condition) in conditions.iter().enumerate() {
                if evaluate(source.as_ref(), row, condition)? {
                    return Ok(indice);
                }
            }
            Ok(conditions.len())
        })
        .collect::<Result<Vec<_>>>()?;
    if let Some(numerici) = numerici {
        let out = scelte
            .into_iter()
            .map(|indice| numerici[indice])
            .collect::<Vec<Option<f64>>>();
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(out)),
        )
    } else {
        let testi = scelte
            .into_iter()
            .map(|indice| {
                conditions
                    .get(indice)
                    .map_or(default_text.as_str(), |condition| condition.result.as_str())
            })
            .collect::<Vec<_>>();
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            false,
            Arc::new(StringArray::from(testi)),
        )
    }
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{
        types::Int32Type, DictionaryArray, LargeStringArray, UInt64Array,
    };
    use plenora_core::arrow::schema::{Field, Schema};
    use serde_json::json;

    use super::*;
    use crate::test_support::{assert_same_outcome, single_column_batch};

    /// Percorso generico, indipendente dai fast path: riferimento per
    /// l'equivalenza semantica del fast path.
    ///
    /// Che cosa garantisce: che i loop nativi del fast path (downcast unico,
    /// `rows_where`) selezionino le stesse righe del percorso per riga, e che
    /// i rifiuti coincidano parola per parola.
    /// Che cosa NON garantisce: condivide con la produzione `evaluate`,
    /// `PreparedCondition`, `NumericBound::parse` e i comparatori
    /// `compare_i64`/`compare_u64`/`compare_f64`; un difetto li' colpirebbe
    /// entrambi. Li coprono i risultati scritti a mano di
    /// `filter_hand_written_boundaries` e i messaggi esatti dei letterali
    /// non validi.
    fn generic_filter(batch: &RecordBatch, config: &Filter) -> Result<RecordBatch> {
        // Regola condivisa sulla config, non oracolata.
        verifica_valore(&config.operator, config.value.as_ref())?;
        let index = column_index(batch, &config.column)?;
        let array = batch.column(index);
        let condition = PreparedCondition::new(&config.operator, config.valore(), config.valore());
        let rows = (0..batch.num_rows())
            .filter_map(|row| match evaluate(array.as_ref(), row, &condition) {
                Ok(true) => Some(Ok(row)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?;
        select_rows(batch, &rows)
    }

    fn config(operator: Operator, value: serde_json::Value) -> Filter {
        Filter {
            column: "c".into(),
            operator,
            // `null` qui vale assente: con `isnull`/`notnull` un valore
            // scritto si rifiuta ([`verifica_valore`]).
            value: (!value.is_null()).then_some(value),
        }
    }

    /// Equivalenza fast path / generico (risultato o errore) su una matrice
    /// di operatori e valori.
    fn assert_equivalent(batch: &RecordBatch, operator: Operator, value: serde_json::Value) {
        let config = config(operator, value);
        assert_same_outcome(filter(batch, &config), generic_filter(batch, &config));
    }

    #[test]
    fn fast_path_matches_generic_on_float64_edge_values() {
        let batch = single_column_batch(
            "c",
            Arc::new(Float64Array::from(vec![
                Some(1.0),
                Some(f64::NAN),
                Some(-0.0),
                Some(0.0),
                None,
                Some(-1.5),
                Some(f64::INFINITY),
            ])),
            DataType::Float64,
            true,
        );
        // NaN e' uguale a NaN (total_cmp) e -0.0 == 0.0: la matrice fissa la
        // semantica esistente, inclusi i casi limite.
        for value in [
            json!(0.0),
            json!(-0.0),
            json!("NaN"),
            json!(1.0),
            json!(-1.5),
        ] {
            assert_equivalent(&batch, Operator::Eq, value.clone());
            assert_equivalent(&batch, Operator::Ne, value);
        }
        for value in [json!(0.0), json!(-1.5), json!("NaN")] {
            assert_equivalent(&batch, Operator::Gt, value.clone());
            assert_equivalent(&batch, Operator::Ge, value.clone());
            assert_equivalent(&batch, Operator::Lt, value.clone());
            assert_equivalent(&batch, Operator::Le, value);
        }
        assert_equivalent(&batch, Operator::Between, json!("-1,1"));
        assert_equivalent(&batch, Operator::Isnull, json!(null));
        assert_equivalent(&batch, Operator::Notnull, json!(null));
        // Valore non numerico: il generico fallisce sulle righe non nulle.
        assert!(filter(&batch, &config(Operator::Eq, json!("x"))).is_err());
        assert_equivalent(&batch, Operator::Eq, json!("x"));
    }

    #[test]
    fn signed_zero_equality_and_nan_match_are_exact() {
        let batch = single_column_batch(
            "c",
            Arc::new(Float64Array::from(vec![
                Some(-0.0),
                Some(f64::NAN),
                Some(2.0),
            ])),
            DataType::Float64,
            true,
        );
        let zero = filter(&batch, &config(Operator::Eq, json!(0.0))).expect("eq 0");
        assert_eq!(zero.num_rows(), 1); // -0.0 == 0.0
        let nan = filter(&batch, &config(Operator::Eq, json!("NaN"))).expect("eq NaN");
        assert_eq!(nan.num_rows(), 1); // NaN uguale a NaN (total_cmp)
        let gt_zero = filter(&batch, &config(Operator::Gt, json!(0.0))).expect("gt 0");
        assert_eq!(gt_zero.num_rows(), 1); // solo 2.0: -0.0 e NaN esclusi
    }

    #[test]
    fn filter_hand_written_boundaries() {
        // Righe attese scritte a mano, indipendenti dai comparatori.
        let ints = single_column_batch(
            "c",
            Arc::new(Int64Array::from(vec![
                Some(i64::MIN),
                Some(-1),
                Some(0),
                None,
                Some(9_007_199_254_740_993),
                Some(i64::MAX),
            ])),
            DataType::Int64,
            true,
        );
        let righe_intere = |operator: Operator, value: serde_json::Value| {
            let output = filter(&ints, &config(operator, value)).expect("filter");
            output
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("int64")
                .iter()
                .collect::<Vec<_>>()
        };
        // Oltre 2^53 il confronto e' esatto: 2^53 + 1 > 2^53.
        assert_eq!(
            righe_intere(Operator::Gt, json!(9_007_199_254_740_992_i64)),
            vec![Some(9_007_199_254_740_993), Some(i64::MAX)]
        );
        assert_eq!(
            righe_intere(Operator::Eq, json!(9_007_199_254_740_992_i64)),
            Vec::<Option<i64>>::new()
        );
        assert_eq!(
            righe_intere(Operator::Ge, json!(i64::MAX)),
            vec![Some(i64::MAX)]
        );
        assert_eq!(
            righe_intere(Operator::Le, json!(i64::MIN)),
            vec![Some(i64::MIN)]
        );
        assert_eq!(
            righe_intere(Operator::Lt, json!(i64::MIN)),
            Vec::<Option<i64>>::new()
        );
        // `between` inclusivo su entrambi gli estremi; null mai selezionato.
        assert_eq!(
            righe_intere(Operator::Between, json!("-1,0")),
            vec![Some(-1), Some(0)]
        );
        assert_eq!(
            righe_intere(Operator::Ne, json!(0)),
            vec![
                Some(i64::MIN),
                Some(-1),
                Some(9_007_199_254_740_993),
                Some(i64::MAX)
            ]
        );

        let uints = single_column_batch(
            "c",
            Arc::new(UInt64Array::from(vec![
                Some(0),
                Some(u64::MAX - 1),
                Some(u64::MAX),
            ])),
            DataType::UInt64,
            true,
        );
        let righe_naturali = |operator: Operator, value: serde_json::Value| {
            let output = filter(&uints, &config(operator, value)).expect("filter");
            output
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .expect("uint64")
                .values()
                .to_vec()
        };
        assert_eq!(
            righe_naturali(Operator::Gt, json!(u64::MAX - 1)),
            vec![u64::MAX]
        );
        assert_eq!(righe_naturali(Operator::Lt, json!(-1)), Vec::<u64>::new());
        assert_eq!(
            righe_naturali(Operator::Ge, json!(-1)),
            vec![0, u64::MAX - 1, u64::MAX]
        );
    }

    #[test]
    fn fast_path_matches_generic_on_int64_and_uint64() {
        let ints = single_column_batch(
            "c",
            Arc::new(Int64Array::from(vec![
                Some(i64::MIN),
                Some(-1),
                Some(0),
                None,
                Some(9_007_199_254_740_993), // oltre 2^53: confronto esatto, nessun rounding
                Some(i64::MAX),
            ])),
            DataType::Int64,
            true,
        );
        for value in [json!(0), json!(-1), json!(9_007_199_254_740_992_i64)] {
            assert_equivalent(&ints, Operator::Eq, value.clone());
            assert_equivalent(&ints, Operator::Ne, value.clone());
            assert_equivalent(&ints, Operator::Gt, value.clone());
            assert_equivalent(&ints, Operator::Le, value);
        }
        assert_equivalent(&ints, Operator::Between, json!("-10, 10"));

        let uints = single_column_batch(
            "c",
            Arc::new(UInt64Array::from(vec![Some(10), Some(9), None, Some(42)])),
            DataType::UInt64,
            true,
        );
        // == canonico: numerico; forme non canoniche ricadono sul generico.
        assert_equivalent(&uints, Operator::Eq, json!(42));
        assert_equivalent(&uints, Operator::Eq, json!("42"));
        assert_equivalent(&uints, Operator::Eq, json!("042"));
        assert_equivalent(&uints, Operator::Ne, json!(9));
        // Ordinati: numerici anche su UInt64 (ramo f64 del generico).
        assert_equivalent(&uints, Operator::Gt, json!(9));
        assert_equivalent(&uints, Operator::Le, json!(10));

        // Letterali non validi: il rifiuto del fast path e' lo stesso del
        // generico, parola per parola.
        for batch in [&ints, &uints] {
            for operator in [Operator::Gt, Operator::Ge, Operator::Lt, Operator::Le] {
                assert_equivalent(batch, operator.clone(), json!("x"));
                let error =
                    filter(batch, &config(operator, json!("x"))).expect_err("letterale accettato");
                assert_eq!(
                    error.to_string(),
                    "contract violation: confronto ordinato richiede un valore numerico"
                );
            }
            for (bounds, message) in [
                ("10", "between richiede min,max"),
                ("x,10", "min between non valido"),
                ("0,x", "max between non valido"),
            ] {
                assert_equivalent(batch, Operator::Between, json!(bounds));
                let error = filter(batch, &config(Operator::Between, json!(bounds)))
                    .expect_err("estremi accettati");
                assert_eq!(error.to_string(), format!("contract violation: {message}"));
            }
        }
    }

    #[test]
    fn fast_path_matches_generic_on_utf8_and_boolean() {
        let strings = single_column_batch(
            "c",
            Arc::new(StringArray::from(vec![
                Some("Alpha"),
                Some("beta"),
                None,
                Some("alphabet"),
                Some("BETA"),
            ])),
            DataType::Utf8,
            true,
        );
        assert_equivalent(&strings, Operator::Eq, json!("beta"));
        assert_equivalent(&strings, Operator::Ne, json!("beta"));
        assert_equivalent(&strings, Operator::Contains, json!("ALPH"));
        assert_equivalent(&strings, Operator::Startswith, json!("Al"));
        assert_equivalent(&strings, Operator::Endswith, json!("TA"));

        let booleans = single_column_batch(
            "c",
            Arc::new(BooleanArray::from(vec![Some(true), Some(false), None])),
            DataType::Boolean,
            true,
        );
        assert_equivalent(&booleans, Operator::Eq, json!(true));
        assert_equivalent(&booleans, Operator::Eq, json!("false"));
        assert_equivalent(&booleans, Operator::Ne, json!(false));
        assert_equivalent(&booleans, Operator::Eq, json!("x"));
    }

    #[test]
    fn null_handling_is_identical_across_operators() {
        let batch = single_column_batch(
            "c",
            Arc::new(Int64Array::from(vec![None, Some(1), None])),
            DataType::Int64,
            true,
        );
        let nulls = filter(&batch, &config(Operator::Isnull, json!(null))).expect("isnull");
        assert_eq!(nulls.num_rows(), 2);
        let not_nulls = filter(&batch, &config(Operator::Notnull, json!(null))).expect("notnull");
        assert_eq!(not_nulls.num_rows(), 1);
        // I null sono esclusi da ogni altro operatore, != compreso.
        let ne = filter(&batch, &config(Operator::Ne, json!(7))).expect("ne");
        assert_eq!(ne.num_rows(), 1);
    }

    #[test]
    fn dictionary_column_uses_generic_path_with_same_results() {
        let keys =
            plenora_core::arrow::array::Int32Array::from(vec![Some(0), Some(1), None, Some(0)]);
        let values = StringArray::from(vec!["a", "b"]);
        let dictionary = DictionaryArray::<Int32Type>::new(keys, Arc::new(values));
        let batch = single_column_batch(
            "c",
            Arc::new(dictionary),
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        );
        let eq = filter(&batch, &config(Operator::Eq, json!("a"))).expect("dictionary eq");
        assert_eq!(eq.num_rows(), 2);
        assert_equivalent(&batch, Operator::Eq, json!("a"));
        assert_equivalent(&batch, Operator::Ne, json!("a"));
        assert_equivalent(&batch, Operator::Contains, json!("a"));
    }

    #[test]
    fn large_utf8_keeps_the_generic_error_on_comparison() {
        let batch = single_column_batch(
            "c",
            Arc::new(LargeStringArray::from(vec![Some("a"), None])),
            DataType::LargeUtf8,
            true,
        );
        // LargeUtf8 non e' nel profilo scalare: il confronto resta un errore
        // di schema (semantica invariata), isnull/notnull funzionano.
        assert!(filter(&batch, &config(Operator::Eq, json!("a"))).is_err());
        assert_equivalent(&batch, Operator::Eq, json!("a"));
        let nulls = filter(&batch, &config(Operator::Isnull, json!(null))).expect("isnull");
        assert_eq!(nulls.num_rows(), 1);
    }

    #[test]
    fn empty_and_single_row_inputs() {
        let empty = single_column_batch(
            "c",
            Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
            DataType::Int64,
            true,
        );
        assert_eq!(
            filter(&empty, &config(Operator::Eq, json!(1)))
                .expect("empty")
                .num_rows(),
            0
        );
        let single = single_column_batch(
            "c",
            Arc::new(Int64Array::from(vec![Some(1)])),
            DataType::Int64,
            true,
        );
        assert_eq!(
            filter(&single, &config(Operator::Eq, json!(1)))
                .expect("single")
                .num_rows(),
            1
        );
    }

    #[test]
    fn int64_comparisons_are_exact_beyond_2_pow_53() {
        // Classe "confronti via f64": 2^53 e 2^53+1 collassano sullo stesso
        // double; eq/ordine/between devono restare esatti (fast e generico).
        let batch = single_column_batch(
            "c",
            Arc::new(Int64Array::from(vec![
                Some(9_007_199_254_740_992), // 2^53
                Some(9_007_199_254_740_993), // 2^53 + 1
                None,
            ])),
            DataType::Int64,
            true,
        );
        let eq_hi = filter(
            &batch,
            &config(Operator::Eq, json!(9_007_199_254_740_993_i64)),
        )
        .expect("eq 2^53+1");
        assert_eq!(eq_hi.num_rows(), 1);
        let eq_lo =
            filter(&batch, &config(Operator::Eq, json!("9007199254740992"))).expect("eq 2^53");
        assert_eq!(eq_lo.num_rows(), 1);
        // Il bound double 9007199254740992.0 e' minore dell'intero 2^53+1.
        let gt = filter(
            &batch,
            &config(Operator::Gt, json!(9_007_199_254_740_992.0)),
        )
        .expect("gt 2^53.0");
        assert_eq!(gt.num_rows(), 1);
        let lt = filter(
            &batch,
            &config(Operator::Lt, json!(9_007_199_254_740_993_i64)),
        )
        .expect("lt 2^53+1");
        assert_eq!(lt.num_rows(), 1);
        let between = filter(
            &batch,
            &config(
                Operator::Between,
                json!("9007199254740993, 9007199254740993"),
            ),
        )
        .expect("between esatto");
        assert_eq!(between.num_rows(), 1);
        // Parita' fast/generico su tutta la matrice di questi valori.
        for value in [
            json!(9_007_199_254_740_992_i64),
            json!(9_007_199_254_740_993_i64),
            json!("9007199254740993"),
            json!(9_007_199_254_740_992.0),
        ] {
            assert_equivalent(&batch, Operator::Eq, value.clone());
            assert_equivalent(&batch, Operator::Ne, value.clone());
            assert_equivalent(&batch, Operator::Gt, value.clone());
            assert_equivalent(&batch, Operator::Ge, value.clone());
            assert_equivalent(&batch, Operator::Lt, value.clone());
            assert_equivalent(&batch, Operator::Le, value);
        }
        assert_equivalent(
            &batch,
            Operator::Between,
            json!("9007199254740992,9007199254740993"),
        );
    }

    #[test]
    fn uint64_ordered_comparisons_are_native_not_textual() {
        // Ordine numerico (9 < 10), non testuale ("10" < "9"), e nessun
        // collasso oltre 2^53: u64::MAX-1 e u64::MAX sono lo stesso double.
        let batch = single_column_batch(
            "c",
            Arc::new(UInt64Array::from(vec![
                Some(10),
                Some(9),
                Some(u64::MAX - 1),
                Some(u64::MAX),
                None,
            ])),
            DataType::UInt64,
            true,
        );
        let gt = filter(&batch, &config(Operator::Gt, json!(9))).expect("gt 9");
        assert_eq!(gt.num_rows(), 3);
        let le = filter(&batch, &config(Operator::Le, json!(10))).expect("le 10");
        assert_eq!(le.num_rows(), 2);
        let top = filter(&batch, &config(Operator::Eq, json!("18446744073709551615")))
            .expect("eq u64::MAX");
        assert_eq!(top.num_rows(), 1);
        let gt_max_minus_one = filter(&batch, &config(Operator::Gt, json!("18446744073709551614")))
            .expect("gt u64::MAX-1");
        assert_eq!(gt_max_minus_one.num_rows(), 1);
        let between = filter(
            &batch,
            &config(
                Operator::Between,
                json!("18446744073709551614,18446744073709551615"),
            ),
        )
        .expect("between u64 top");
        assert_eq!(between.num_rows(), 2);
        for value in [json!(9), json!(10), json!("18446744073709551614")] {
            assert_equivalent(&batch, Operator::Gt, value.clone());
            assert_equivalent(&batch, Operator::Ge, value.clone());
            assert_equivalent(&batch, Operator::Lt, value.clone());
            assert_equivalent(&batch, Operator::Le, value);
        }
        assert_equivalent(&batch, Operator::Between, json!("9,10"));
        assert_equivalent(
            &batch,
            Operator::Between,
            json!("18446744073709551614,18446744073709551615"),
        );
    }

    #[test]
    fn float64_column_compares_exactly_against_integer_literals() {
        // Letterale intero oltre 2^53 contro colonna Float64: il double
        // 9007199254740992.0 NON e' uguale all'intero 9007199254740993.
        let batch = single_column_batch(
            "c",
            Arc::new(Float64Array::from(vec![
                Some(9_007_199_254_740_992.0),
                None,
            ])),
            DataType::Float64,
            true,
        );
        let eq = filter(
            &batch,
            &config(Operator::Eq, json!(9_007_199_254_740_993_i64)),
        )
        .expect("eq intero oltre 2^53");
        assert_eq!(eq.num_rows(), 0);
        let lt = filter(
            &batch,
            &config(Operator::Lt, json!(9_007_199_254_740_993_i64)),
        )
        .expect("lt intero oltre 2^53");
        assert_eq!(lt.num_rows(), 1);
        assert_equivalent(&batch, Operator::Eq, json!(9_007_199_254_740_993_i64));
        assert_equivalent(&batch, Operator::Ne, json!(9_007_199_254_740_993_i64));
        assert_equivalent(&batch, Operator::Le, json!(9_007_199_254_740_993_i64));
        assert_equivalent(&batch, Operator::Gt, json!(9_007_199_254_740_993_i64));
    }

    #[test]
    fn mixed_columns_and_row_order_are_preserved() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("c", DataType::Float64, true),
                Field::new("label", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![10, 20, 30, 40])),
                Arc::new(Float64Array::from(vec![
                    Some(2.0),
                    None,
                    Some(1.0),
                    Some(3.0),
                ])),
                Arc::new(StringArray::from(vec!["a", "b", "c", "d"])),
            ],
        )
        .expect("fixture");
        let config = Filter {
            column: "c".into(),
            operator: Operator::Gt,
            value: Some(json!(1.5)),
        };
        let fast = filter(&batch, &config).expect("fast");
        let generic = generic_filter(&batch, &config).expect("generic");
        assert_eq!(fast, generic);
        let ids = fast
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("ids");
        // Ordine originale delle righe selezionate: 10, 40 (riga null esclusa).
        assert_eq!(ids.values(), &[10, 40]);
    }

    /// Regressione: un risultato intero oltre 2^53 diventava in silenzio il
    /// double piu' vicino (un altro intero) nella colonna `Float64`. Ora si
    /// rifiuta, nel kernel e nell'analisi, con la stessa regola; il testo
    /// resta testo e un intero esatto resta valido.
    #[test]
    fn conditional_rifiuta_un_risultato_intero_inesatto() {
        let batch = single_column_batch(
            "x",
            Arc::new(Int64Array::from(vec![Some(1), Some(2)])),
            DataType::Int64,
            true,
        );
        let config = |risultato: serde_json::Value| -> Conditional {
            serde_json::from_value(json!({
                "column": "x",
                "conditions": [{"operator": "==", "value": 1, "result": risultato}],
                "default_value": 0
            }))
            .expect("config")
        };
        for risultato in [json!(9_007_199_254_740_993_i64), json!("9007199254740993")] {
            assert!(matches!(
                conditional(&batch, &config(risultato)),
                Err(PlenoraError::InvalidPlan(_))
            ));
        }
        let esatto = conditional(&batch, &config(json!(9_007_199_254_740_992_i64))).expect("2^53");
        let colonna = esatto
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("Float64")
            .clone();
        assert_eq!(
            colonna.values().to_vec(),
            vec![9_007_199_254_740_992.0, 0.0]
        );
        let testo = conditional(&batch, &config(json!("alto"))).expect("testo");
        assert_eq!(testo.schema().field(1).data_type(), &DataType::Utf8);
        assert_eq!(
            risultati_numerici(["9007199254740993", "x"]).expect("uscita testuale"),
            None
        );
        // Oltre `i128` (seconda revisione): 10^40 + 1 non e' un double.
        assert!(risultati_numerici(["10000000000000000000000000000000000000001"]).is_err());
        // 2^140 lo e', anche se non sta in `i128`.
        assert!(risultati_numerici(["1393796574908163946345982392040522594123776"]).is_ok());
        assert!(risultati_numerici(["-0007", "0"]).is_ok());
    }
}
