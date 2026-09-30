//! Kernel `table.expression`: valutazione di espressioni scalari su colonne
//! (interprete generico e fast path compilato, stessa semantica).
//!
//! I sottomoduli e cio' che ciascuno possiede:
//!
//! - `scalar`: valore scalare generico (`Scalar`) con coercizioni,
//!   confronti e aritmetica/logica dell'interprete;
//! - `temporal`: macchina temporale di `date_trunc` (troncamenti
//!   Date32/Timestamp ms) e valutazione di `in`;
//! - `interpreter`: inferenza del tipo temporale, funzioni scalari,
//!   interprete ricorsivo sull'AST, validazione statica e percorso generico
//!   di output;
//! - `fast`: fast path compilato (`FastNode`/`FastProgram`), verificato
//!   dai test-oracolo contro il percorso generico;
//! - [`static_type`]: tipo statico dell'AST ricavato dal solo SCHEMA,
//!   sorgente unica per il kernel e per l'analizzatore del contratto.

mod fast;
mod interpreter;
mod scalar;
pub mod static_type;
mod temporal;

use serde::Deserialize;
use serde_json::Value;

pub use interpreter::{expression, expression_con_effetti, validate};

/// Tipo della colonna prodotta da `table.expression`. Con un tipo
/// dichiarato l'espressione deve poterlo produrre: il kernel non converte.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    /// L'unico tipo che l'espressione puo' produrre; solo null da' `Text`
    /// (default).
    Auto,
    /// `Float64`.
    Number,
    /// `Boolean`.
    Boolean,
    /// `Utf8`.
    Text,
    /// Date32 nativo (prodotto da `date_trunc` su colonna Date32).
    Date32,
    /// Timestamp(ms) nativo senza timezone (prodotto da `date_trunc`).
    TimestampMs,
}

const fn default_output_type() -> OutputType {
    OutputType::Auto
}

/// Config di `table.expression`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionTransform {
    /// Colonna d'uscita, nullable.
    pub output_column: String,
    /// Albero dell'espressione (profondita' al piu' 64; nodi al piu' quelli
    /// passati a [`validate`]).
    pub expression: Expression,
    /// Tipo della colonna d'uscita; default `auto`.
    #[serde(default = "default_output_type")]
    pub output_type: OutputType,
    /// Che cosa rende una divisione per un divisore zero: `"null"` (default)
    /// o `"error"` ([`crate::OnDivisionByZero`]). Senza divisioni
    /// nell'espressione non avrebbe effetto: scritto si rifiuta
    /// ([`ExpressionTransform::verifica_parametri`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub on_division_by_zero: Option<crate::OnDivisionByZero>,
}

impl ExpressionTransform {
    /// La politica sulla divisione per zero: quella scritta, o `null`.
    #[must_use]
    pub fn divisione_per_zero(&self) -> crate::OnDivisionByZero {
        self.on_division_by_zero.unwrap_or_default()
    }

    /// Regole sulla sola config, condivise da kernel e analisi dei
    /// contratti: `on_division_by_zero` scritto senza una divisione
    /// nell'espressione non ha effetto e si rifiuta; un divisore letterale
    /// zero e' un errore di piano con ogni politica
    /// ([`reject_literal_zero_divisor`]); i letterali contro i limiti
    /// ([`verifica_limiti_letterali`]).
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per ciascuna delle regole.
    pub fn verifica_parametri(&self, limits: &crate::Limits) -> plenora_core::Result<()> {
        if self.on_division_by_zero.is_some() && !contiene_divisione(&self.expression) {
            return Err(plenora_core::PlenoraError::InvalidPlan(
                "on_division_by_zero senza effetto: l'espressione non divide".into(),
            ));
        }
        reject_literal_zero_divisor(&self.expression)?;
        verifica_limiti_letterali(&self.expression, limits)
    }
}

/// Figli diretti di un nodo, nell'ordine dell'albero.
fn figli(expr: &Expression) -> Vec<&Expression> {
    match expr {
        Expression::Column { .. } | Expression::Literal { .. } => Vec::new(),
        Expression::Unary { value, .. } => vec![value],
        Expression::Binary { left, right, .. } => vec![left, right],
        Expression::Function { args, .. } => args.iter().collect(),
        Expression::Case {
            branches,
            else_value,
        } => branches
            .iter()
            .flat_map(|branch| [&branch.when, &branch.then])
            .chain(std::iter::once(else_value.as_ref()))
            .collect(),
    }
}

/// `true` se l'albero contiene una divisione.
fn contiene_divisione(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::Binary {
            op: BinaryOperator::Divide,
            ..
        }
    ) || figli(expr).into_iter().any(contiene_divisione)
}

/// I letterali contro i limiti, prima dei dati.
///
/// Un testo letterale oltre `max_string_bytes`, un pattern letterale di
/// `regex_replace` oltre `max_regex_bytes`. Un pattern calcolato dalle
/// colonne si controlla riga per riga in valutazione (`ResourceLimit`).
///
/// # Errors
///
/// `InvalidPlan` per il primo letterale oltre il suo limite.
pub fn verifica_limiti_letterali(
    expr: &Expression,
    limits: &crate::Limits,
) -> plenora_core::Result<()> {
    use plenora_core::error::PlenoraError;
    /// Il testo piu' lungo dentro un valore JSON (anche nelle liste di `in`).
    fn testo_massimo(valore: &Value) -> usize {
        match valore {
            Value::String(testo) => testo.len(),
            Value::Array(elementi) => elementi.iter().map(testo_massimo).max().unwrap_or(0),
            Value::Object(campi) => campi.values().map(testo_massimo).max().unwrap_or(0),
            Value::Null | Value::Bool(_) | Value::Number(_) => 0,
        }
    }
    match expr {
        Expression::Literal { value } if testo_massimo(value) > limits.max_string_bytes => {
            return Err(PlenoraError::InvalidPlan(
                "testo letterale oltre max_string_bytes".into(),
            ));
        }
        Expression::Function {
            name: Function::RegexReplace,
            args,
        } => {
            if let Some(Expression::Literal {
                value: Value::String(pattern),
            }) = args.get(1)
            {
                if pattern.len() > limits.max_regex_bytes {
                    return Err(PlenoraError::InvalidPlan(
                        "regex_replace: pattern oltre max_regex_bytes".into(),
                    ));
                }
            }
        }
        _ => {}
    }
    for figlio in figli(expr) {
        verifica_limiti_letterali(figlio, limits)?;
    }
    Ok(())
}

/// Contesto di una valutazione: la politica sulla divisione per zero, se
/// nella riga corrente una divisione l'ha applicata, e i limiti dei testi e
/// dei pattern calcolati. Lo usano entrambi i percorsi (generico e fast),
/// con lo stesso effetto.
pub(crate) struct Contesto<'l> {
    divisione: crate::OnDivisionByZero,
    zero_nella_riga: std::cell::Cell<bool>,
    limits: &'l crate::Limits,
}

impl<'l> Contesto<'l> {
    pub(crate) fn new(config: &ExpressionTransform, limits: &'l crate::Limits) -> Self {
        Self {
            divisione: config.divisione_per_zero(),
            zero_nella_riga: std::cell::Cell::new(false),
            limits,
        }
    }

    /// Esito di una divisione: con la politica `null` l'errore di divisione
    /// per zero diventa `null` (dato da `nullo`) e la riga si segna; ogni
    /// altro esito passa invariato.
    pub(crate) fn dividi<T>(
        &self,
        esito: plenora_core::Result<T>,
        nullo: impl FnOnce() -> T,
    ) -> plenora_core::Result<T> {
        match esito {
            Err(errore)
                if self.divisione == crate::OnDivisionByZero::Null
                    && crate::e_divisione_per_zero(&errore) =>
            {
                self.zero_nella_riga.set(true);
                Ok(nullo())
            }
            altro => altro,
        }
    }

    /// Un testo calcolato entro `max_string_bytes`.
    pub(crate) fn verifica_testo(&self, byte: usize) -> plenora_core::Result<()> {
        crate::verifica_testo_prodotto("table.expression", byte, self.limits)
    }

    /// Un pattern calcolato di `regex_replace` entro `max_regex_bytes`.
    pub(crate) fn verifica_pattern(&self, byte: usize) -> plenora_core::Result<()> {
        if byte > self.limits.max_regex_bytes {
            return Err(plenora_core::PlenoraError::ResourceLimit(
                "regex_replace: pattern calcolato oltre max_regex_bytes".into(),
            ));
        }
        Ok(())
    }

    /// Azzera il segno della riga e dice se la riga appena valutata ha
    /// applicato la politica `null`.
    pub(crate) const fn chiudi_riga(&self) -> bool {
        self.zero_nella_riga.replace(false)
    }
}

/// Nodo dell'espressione (JSON con il campo `kind`).
///
/// Un campo che il nodo non conosce si rifiuta: senza `deny_unknown_fields`
/// serde lo ignorerebbe (un `{"kind": "column", "name": "a", "type": "int"}`
/// si leggeva come la sola colonna).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expression {
    /// Valore di una colonna: `Boolean` e' booleano; `Int64`, `UInt64`,
    /// `Float64`, `Decimal128`, `Date32` e `Timestamp(ms)` sono numeri; gli
    /// altri tipi leggibili come testo sono testo.
    Column {
        /// Nome della colonna.
        name: String,
    },
    /// Letterale scalare: null, booleano, numero finito o stringa (una
    /// lista solo come secondo argomento di `in`).
    Literal {
        /// Valore JSON.
        value: Value,
    },
    /// Operatore unario.
    Unary {
        /// Operatore.
        op: UnaryOperator,
        /// Operando.
        value: Box<Self>,
    },
    /// Operatore binario.
    Binary {
        /// Operatore.
        op: BinaryOperator,
        /// Operando sinistro.
        left: Box<Self>,
        /// Operando destro.
        right: Box<Self>,
    },
    /// Chiamata di funzione, al piu' 64 argomenti.
    Function {
        /// Funzione.
        name: Function,
        /// Argomenti, nell'ordine.
        args: Vec<Self>,
    },
    /// Primo ramo con `when` vero, altrimenti `else_value`; i rami non scelti
    /// non si valutano.
    Case {
        /// Rami, da 1 a 64.
        branches: Vec<CaseBranch>,
        /// Valore se nessun `when` e' vero (un `when` null vale falso).
        else_value: Box<Self>,
    },
}

/// Ramo di un `case`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseBranch {
    /// Condizione booleana.
    pub when: Expression,
    /// Valore del ramo.
    pub then: Expression,
}

/// Vero se l'espressione e' un letterale numerico zero (anche negato):
/// un divisore costante zero e' una proprieta' del piano, non delle righe.
fn is_literal_zero(expr: &Expression) -> bool {
    match expr {
        Expression::Literal { value } => value.as_f64() == Some(0.0),
        Expression::Unary {
            op: UnaryOperator::Negate,
            value,
        } => is_literal_zero(value),
        _ => false,
    }
}

/// Rifiuta le divisioni con divisore letterale zero.
///
/// Errore di configurazione (`InvalidPlan`), mai un rifiuto row-scoped
/// attribuito a tutte le righe. Un divisore dipendente dalla riga resta
/// row-scoped.
///
/// # Errors
/// - `InvalidPlan`: divisore letterale zero in qualunque punto dell'albero.
pub fn reject_literal_zero_divisor(expr: &Expression) -> plenora_core::error::Result<()> {
    use plenora_core::error::PlenoraError;
    match expr {
        Expression::Binary {
            op: BinaryOperator::Divide,
            right,
            ..
        } if is_literal_zero(right) => Err(PlenoraError::InvalidPlan(
            "divisione per zero letterale nell'espressione: errore di configurazione, non di riga"
                .into(),
        )),
        Expression::Binary { left, right, .. } => {
            reject_literal_zero_divisor(left)?;
            reject_literal_zero_divisor(right)
        }
        Expression::Unary { value, .. } => reject_literal_zero_divisor(value),
        Expression::Function { args, .. } => {
            for arg in args {
                reject_literal_zero_divisor(arg)?;
            }
            Ok(())
        }
        Expression::Case {
            branches,
            else_value,
        } => {
            for branch in branches {
                reject_literal_zero_divisor(&branch.when)?;
                reject_literal_zero_divisor(&branch.then)?;
            }
            reject_literal_zero_divisor(else_value)
        }
        Expression::Column { .. } | Expression::Literal { .. } => Ok(()),
    }
}

/// Operatore unario di un'espressione.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOperator {
    /// Negazione logica di un booleano; null resta null.
    Not,
    /// Opposto di un numero, esatto sul valore d'origine.
    Negate,
    /// Vero se l'operando e' null (mai null).
    IsNull,
    /// Vero se l'operando non e' null (mai null).
    IsNotNull,
}

/// Operatore binario di un'espressione. Un operando null rende null il
/// risultato, salvo `and`/`or` (logica a tre valori).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOperator {
    /// Somma di numeri, in `f64`.
    Add,
    /// Differenza di numeri, in `f64`.
    Subtract,
    /// Prodotto di numeri, in `f64`.
    Multiply,
    /// Quoziente di numeri, in `f64`; un divisore zero rifiuta la riga.
    Divide,
    /// Uguaglianza fra operandi dello stesso tipo (numeri sul valore esatto).
    Equal,
    /// Disuguaglianza fra operandi dello stesso tipo.
    NotEqual,
    /// Maggiore.
    Greater,
    /// Maggiore o uguale.
    GreaterEqual,
    /// Minore.
    Less,
    /// Minore o uguale.
    LessEqual,
    /// Congiunzione: `false` se un operando e' `false`, anche con l'altro null.
    And,
    /// Disgiunzione: `true` se un operando e' `true`, anche con l'altro null.
    Or,
}

/// Funzione di un'espressione. L'arieta' e i tipi degli argomenti si
/// verificano sullo schema prima dell'esecuzione.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Function {
    /// `coalesce(a, ...)`: il primo argomento non null (almeno uno).
    Coalesce,
    /// `null_if(a, b)`: null se `a` e' uguale a `b`, altrimenti `a`.
    NullIf,
    /// `lower(testo)`: minuscolo Unicode.
    Lower,
    /// `upper(testo)`: maiuscolo Unicode.
    Upper,
    /// `trim(testo)`: senza spazi Unicode ai lati.
    Trim,
    /// `length(testo)`: numero di caratteri Unicode.
    Length,
    /// `concat(testo, ...)`: concatenazione; null se un argomento e' null.
    Concat,
    /// `contains(testo, sotto)`: booleano, con distinzione di maiuscole.
    Contains,
    /// `starts_with(testo, prefisso)`: booleano.
    StartsWith,
    /// `ends_with(testo, suffisso)`: booleano.
    EndsWith,
    /// `abs(numero)`: valore assoluto, esatto sul valore d'origine.
    Abs,
    /// `round(numero)`: arrotondamento all'intero, meta' lontano da zero.
    Round,
    /// `year(testo)`: anno dei primi 10 byte letti come `%Y-%m-%d`.
    Year,
    /// `substring(string, start, len?)`: `start` 0-based, conteggio per
    /// carattere Unicode; `len` omesso = fino a fine stringa.
    Substring,
    /// `regex_replace(string, pattern, replacement)`: sintassi della crate
    /// `regex`, gruppi di cattura espansibili con `$1`/`$name` nel replacement.
    RegexReplace,
    /// `between(value, low, high)`: inclusivo su entrambi gli estremi.
    Between,
    /// `in(value, [letterali])`: membership su lista di letterali scalari.
    In,
    /// `greatest(a, ...)`: il massimo di argomenti dello stesso tipo; null
    /// se un argomento e' null.
    Greatest,
    /// `least(a, ...)`: il minimo di argomenti dello stesso tipo; null se un
    /// argomento e' null.
    Least,
    /// `floor(numero)`: parte intera per difetto.
    Floor,
    /// `ceil(numero)`: parte intera per eccesso.
    Ceil,
    /// `power(base, esponente)`: un risultato non finito rifiuta la riga.
    Power,
    /// `date_trunc(unit, value)`: `unit` letterale del set chiuso
    /// year/month/day/hour/minute/second; `value` colonna Date32 o
    /// Timestamp(ms) letta NATIVAMENTE (output Date32/TimestampMs).
    DateTrunc,
}

// Simboli usati solo dai test-oracolo, che li importano con
// `use super::*`.
#[cfg(test)]
use fast::FastProgram;
#[cfg(test)]
use interpreter::expression_generic;
#[cfg(test)]
use plenora_core::arrow::array::{Array, RecordBatch, TimestampMillisecondArray};
#[cfg(test)]
use plenora_core::arrow::schema::{DataType, TimeUnit};
#[cfg(test)]
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Test-oracolo: fast path compilato vs interprete generico.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{
        BooleanArray, Date32Array, Float64Array, Int64Array, StringArray, UInt64Array,
    };
    use plenora_core::arrow::schema::{Field, Schema};
    use plenora_core::error::PlenoraError;
    use serde_json::json;

    use super::*;
    use crate::test_support::single_column_batch;

    /// Fixture con null, -0.0, zeri, testi (anche data-like) e booleani.
    ///
    /// La colonna `nan` contiene NaN: la lettura deve fallire in entrambi i
    /// percorsi ("numero non finito in ingresso"). `ts` e `tstz`
    /// coprono i timestamp nativi (naive e timezone-aware) di `date_trunc`.
    fn fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("n", DataType::Float64, true),
                Field::new("nan", DataType::Float64, true),
                Field::new("i", DataType::Int64, true),
                Field::new("u", DataType::UInt64, true),
                Field::new("d", DataType::Date32, true),
                Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
                Field::new(
                    "tstz",
                    DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                    true,
                ),
                Field::new("s", DataType::Utf8, true),
                Field::new("b", DataType::Boolean, true),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    None,
                    Some(-0.0),
                    Some(4.0),
                    Some(0.0),
                    Some(7.25),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.0),
                    Some(f64::NAN),
                    Some(2.0),
                    None,
                    Some(0.0),
                    Some(-0.0),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(3),
                    None,
                    Some(0),
                    Some(-2),
                    Some(10),
                    Some(1),
                ])),
                Arc::new(UInt64Array::from(vec![
                    Some(5),
                    None,
                    Some(u64::MAX),
                    Some(2),
                    Some(0),
                    Some(9),
                ])),
                Arc::new(Date32Array::from(vec![
                    Some(0),
                    None,
                    Some(19_000),
                    Some(-1),
                    Some(1),
                    Some(20_000),
                ])),
                // 1970-01-04 05:01:01.007 UTC, epoca, pre-1970,
                // 2023-11-14 UTC.
                Arc::new(TimestampMillisecondArray::from(vec![
                    Some(86_400_000 * 3 + 3_600_000 * 5 + 61_000 + 7),
                    None,
                    Some(0),
                    Some(-1),
                    Some(1_700_000_000_123),
                    Some(-86_400_000),
                ])),
                Arc::new(
                    TimestampMillisecondArray::from(vec![
                        Some(0),
                        None,
                        Some(1),
                        Some(-1),
                        Some(1_000),
                        Some(86_400_000),
                    ])
                    .with_timezone("UTC"),
                ),
                Arc::new(StringArray::from(vec![
                    Some("Ciao"),
                    None,
                    Some("2024-05-06"),
                    Some(""),
                    Some("x42y"),
                    Some("ab"),
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    None,
                    Some(false),
                    Some(true),
                    Some(false),
                    None,
                ])),
            ],
        )
        .expect("fixture")
    }

    fn col(name: &str) -> Value {
        json!({"kind": "column", "name": name})
    }

    // Builder di fixture per i test: il passaggio per valore dei `Value`
    // JSON (piccoli, costruiti al volo) e' l'ergonomia voluta dei casi di
    // test; il borrow suggerito dal lint complicherebbe ~180 call site
    // senza alcun beneficio di correttezza.
    #[allow(clippy::needless_pass_by_value)]
    fn lit(value: Value) -> Value {
        json!({"kind": "literal", "value": value})
    }

    #[allow(clippy::needless_pass_by_value)]
    fn bin(op: &str, left: Value, right: Value) -> Value {
        json!({"kind": "binary", "op": op, "left": left, "right": right})
    }

    #[allow(clippy::needless_pass_by_value)]
    fn un(op: &str, value: Value) -> Value {
        json!({"kind": "unary", "op": op, "value": value})
    }

    #[allow(clippy::needless_pass_by_value)]
    fn func(name: &str, args: Vec<Value>) -> Value {
        json!({"kind": "function", "name": name, "args": args})
    }

    #[allow(clippy::needless_pass_by_value)]
    fn case(branches: Vec<(Value, Value)>, else_value: Value) -> Value {
        json!({
            "kind": "case",
            "branches": branches
                .into_iter()
                .map(|(when, then)| json!({"when": when, "then": then}))
                .collect::<Vec<_>>(),
            "else_value": else_value,
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    fn config(expression: Value, output_type: Option<&str>) -> ExpressionTransform {
        let mut value = json!({"output_column": "out", "expression": expression});
        if let Some(output_type) = output_type {
            value["output_type"] = json!(output_type);
        }
        serde_json::from_value(value).expect("config valida")
    }

    fn assert_equivalent(batch: &RecordBatch, expression: Value, output_type: Option<&str>) {
        let config = config(expression, output_type);
        let fast = FastProgram::compile(&config.expression, batch).run_auto(batch, &config);
        let generic = expression_generic(batch, &config);
        match (fast, generic) {
            (Ok(fast), Ok(generic)) => assert_eq!(fast, generic),
            (fast, generic) => {
                assert_eq!(
                    fast.as_ref().map_err(ToString::to_string).map(|_| ()),
                    generic.as_ref().map_err(ToString::to_string).map(|_| ()),
                );
            }
        }
    }

    #[test]
    fn oracle_aritmetica_e_confronti() {
        let batch = fixture();
        for op in ["add", "subtract", "multiply", "divide"] {
            assert_equivalent(&batch, bin(op, col("n"), col("i")), None);
            assert_equivalent(&batch, bin(op, col("n"), lit(json!(2.5))), None);
            assert_equivalent(&batch, bin(op, lit(json!(10)), col("i")), None);
        }
        for op in [
            "equal",
            "not_equal",
            "greater",
            "greater_equal",
            "less",
            "less_equal",
        ] {
            // Numeri: include -0.0 vs 0.0 (confronto esatto: uguali).
            assert_equivalent(&batch, bin(op, col("n"), lit(json!(0.0))), None);
            assert_equivalent(&batch, bin(op, col("n"), col("i")), None);
            // Testi e booleani.
            assert_equivalent(&batch, bin(op, col("s"), lit(json!("x42y"))), None);
            assert_equivalent(&batch, bin(op, col("b"), lit(json!(true))), None);
            // Tipi misti: errore di confronto incompatibile.
            assert_equivalent(&batch, bin(op, col("n"), col("s")), None);
            assert_equivalent(&batch, bin(op, col("b"), col("n")), None);
        }
        // UInt64 (anche u64::MAX) e Date32 come numeri.
        assert_equivalent(&batch, bin("add", col("u"), lit(json!(1))), None);
        assert_equivalent(&batch, bin("add", col("d"), lit(json!(1))), None);
        assert_equivalent(&batch, bin("greater", col("u"), col("i")), None);
    }

    #[test]
    fn oracle_logica_e_unari() {
        let batch = fixture();
        for op in ["and", "or"] {
            assert_equivalent(&batch, bin(op, col("b"), lit(json!(true))), None);
            assert_equivalent(&batch, bin(op, col("b"), col("b")), None);
            // Logica su non booleani: errore.
            assert_equivalent(&batch, bin(op, col("n"), col("b")), None);
        }
        for op in ["not", "negate", "is_null", "is_not_null"] {
            assert_equivalent(&batch, un(op, col("b")), None);
            assert_equivalent(&batch, un(op, col("n")), None);
            assert_equivalent(&batch, un(op, col("s")), None);
            assert_equivalent(&batch, un(op, lit(Value::Null)), None);
        }
    }

    #[test]
    fn oracle_funzioni() {
        let batch = fixture();
        let cases = vec![
            func("coalesce", vec![col("s"), lit(json!("fb"))]),
            func("coalesce", vec![col("n"), col("i"), lit(json!(0))]),
            func("coalesce", vec![lit(Value::Null)]),
            func("null_if", vec![col("s"), lit(json!("ab"))]),
            func("null_if", vec![col("n"), lit(json!(0.0))]),
            func("lower", vec![col("s")]),
            func("upper", vec![col("s")]),
            func("trim", vec![col("s")]),
            func("length", vec![col("s")]),
            func("year", vec![col("s")]),
            func("year", vec![lit(json!("2024-12-31"))]),
            func("concat", vec![col("s"), lit(json!("-")), col("s")]),
            func("concat", vec![lit(json!("solo"))]),
            func("contains", vec![col("s"), lit(json!("42"))]),
            func("starts_with", vec![col("s"), lit(json!("Ci"))]),
            func("ends_with", vec![col("s"), lit(json!("y"))]),
            func("abs", vec![col("n")]),
            func("round", vec![col("n")]),
            func("abs", vec![col("s")]),
            func("lower", vec![col("n")]),
            func("null_if", vec![col("s")]),
            func("concat", vec![]),
            func("coalesce", vec![]),
            func("contains", vec![col("s")]),
        ];
        for expression in cases {
            assert_equivalent(&batch, expression, None);
        }
    }

    #[test]
    fn oracle_case_e_errori_lazy() {
        let batch = fixture();
        // Case base su booleani con null.
        assert_equivalent(
            &batch,
            case(
                vec![(col("b"), col("n")), (lit(json!(true)), col("i"))],
                lit(json!(0)),
            ),
            None,
        );
        // Ramo non percorso con colonna mancante: nessun errore (lazy).
        assert_equivalent(
            &batch,
            case(vec![(lit(json!(false)), col("missing"))], lit(json!(1))),
            None,
        );
        // Ramo percorso con colonna mancante: errore identico.
        assert_equivalent(
            &batch,
            case(vec![(lit(json!(true)), col("missing"))], lit(json!(1))),
            None,
        );
        // Ramo non percorso con letterale non scalare: nessun errore (lazy).
        assert_equivalent(
            &batch,
            case(vec![(lit(json!(false)), lit(json!([1, 2])))], lit(json!(1))),
            None,
        );
        // Letterale non scalare valutato: errore identico.
        assert_equivalent(&batch, lit(json!([1, 2])), None);
        assert_equivalent(&batch, lit(json!({"a": 1})), None);
        // When non booleano: errore identico.
        assert_equivalent(
            &batch,
            case(vec![(col("n"), col("i"))], lit(json!(0))),
            None,
        );
        // Output eterogeneo in auto: errore identico.
        assert_equivalent(&batch, case(vec![(col("b"), col("n"))], col("s")), None);
    }

    #[test]
    fn oracle_errori_di_dominio() {
        let batch = fixture();
        // Divisione per zero (anche -0.0) a righe diverse.
        assert_equivalent(&batch, bin("divide", col("n"), lit(json!(0.0))), None);
        assert_equivalent(&batch, bin("divide", col("n"), lit(json!(-0.0))), None);
        assert_equivalent(&batch, bin("divide", col("n"), col("i")), None);
        // Risultato non finito.
        assert_equivalent(
            &batch,
            bin("multiply", lit(json!(1e308)), lit(json!(10.0))),
            None,
        );
        // NaN in colonna: lettura rifiutata in entrambi i percorsi.
        assert_equivalent(&batch, col("nan"), None);
        assert_equivalent(&batch, bin("equal", col("nan"), col("nan")), None);
        // Colonna mancante in testa.
        assert_equivalent(&batch, bin("add", col("missing"), lit(json!(1))), None);
        // output_type dichiarato con conversione impossibile.
        assert_equivalent(&batch, col("s"), Some("number"));
        assert_equivalent(&batch, col("n"), Some("text"));
        assert_equivalent(&batch, col("b"), Some("boolean"));
        assert_equivalent(&batch, col("n"), Some("number"));
    }

    #[test]
    fn divisione_per_zero_letterale_e_errore_di_configurazione() {
        // Divisore LETTERALE zero (anche negato) ->
        // errore di configurazione senza diagnostica row-scoped; mai un
        // rifiuto attribuito a tutte le righe.
        let batch = fixture();
        for divisor in [
            lit(json!(0.0)),
            lit(json!(0)),
            json!({"kind":"unary","op":"negate","value": lit(json!(0.0))}),
        ] {
            let cfg = config(bin("divide", col("i"), divisor), None);
            let error = expression(&batch, &cfg).expect_err("divisore letterale zero");
            assert!(
                matches!(error, PlenoraError::InvalidPlan(_)),
                "atteso InvalidPlan (config), trovato {error:?}"
            );
            assert!(
                error.row_diagnostics().is_none(),
                "nessuna diagnostica row-scoped per errore di configurazione"
            );
        }
        // Controllo: divisore dipendente dalla riga -> row-scoped con `error`.
        let cfg = con_politica(
            bin(
                "divide",
                col("i"),
                bin("multiply", col("i"), lit(json!(0.0))),
            ),
            "error",
        );
        let error = expression(&batch, &cfg).expect_err("divisione calcolata");
        assert!(error.row_diagnostics().is_some());
        // Letterale zero: errore di piano anche con la politica `null`.
        let cfg = con_politica(bin("divide", col("n"), lit(json!(0))), "null");
        assert!(matches!(
            expression(&batch, &cfg),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }

    #[allow(clippy::needless_pass_by_value)] // Valore in linea con `json!` nei casi.
    fn con_politica(expression: Value, politica: &str) -> ExpressionTransform {
        serde_json::from_value(json!({"output_column": "out", "expression": expression,
                                      "on_division_by_zero": politica}))
        .expect("config valida")
    }

    #[test]
    fn divisione_per_zero_di_default_vale_null_nel_nodo_e_si_conta() {
        // Default (decisione dell'utente): la divisione vale null nel nodo, e
        // il null segue le regole dei null. Righe 0, 2, 3, 4, 5 dividono per
        // zero; la riga 1 ha `i` null e non conta.
        let batch = fixture();
        let divisione = bin(
            "divide",
            col("i"),
            bin("multiply", col("i"), lit(json!(0.0))),
        );
        for cfg in [
            config(divisione.clone(), None),
            con_politica(divisione.clone(), "null"),
        ] {
            let (uscita, effetti) =
                expression_con_effetti(&batch, &cfg, &crate::Limits::default()).expect("null");
            assert_eq!(effetti.righe_divisione_per_zero, 5);
            assert_eq!(uscita.column_by_name("out").expect("out").null_count(), 6);
            // Fast e generico: stessa uscita, stesso conteggio.
            let (generico, effetti_generico) =
                interpreter::expression_generic_con_effetti(&batch, &cfg).expect("generico");
            assert_eq!(uscita, generico);
            assert_eq!(effetti, effetti_generico);
            let (fast, effetti_fast) = FastProgram::compile(&cfg.expression, &batch)
                .run_auto_con_effetti(&batch, &cfg)
                .expect("fast");
            assert_eq!(uscita, fast);
            assert_eq!(effetti, effetti_fast);
        }
        // Null nel nodo, non nella riga: `coalesce(a / 0, -1)` da' -1 e la
        // riga si conta comunque.
        let cfg = config(
            json!({"kind": "function", "name": "coalesce",
                   "args": [divisione, lit(json!(-1))]}),
            None,
        );
        let (uscita, effetti) =
            expression_con_effetti(&batch, &cfg, &crate::Limits::default()).expect("coalesce");
        assert_eq!(effetti.righe_divisione_per_zero, 5);
        let valori = uscita
            .column_by_name("out")
            .expect("out")
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::Float64Array>()
            .expect("f64")
            .clone();
        assert_eq!(valori.null_count(), 0);
        assert!(valori.iter().all(|valore| valore == Some(-1.0)));
        let (generico, effetti_generico) =
            interpreter::expression_generic_con_effetti(&batch, &cfg).expect("generico");
        assert_eq!(uscita, generico);
        assert_eq!(effetti, effetti_generico);
        // Due divisioni per zero nella stessa riga: la riga si conta una volta.
        let cfg = config(bin("add", divisione.clone(), divisione.clone()), None);
        let (_, effetti) =
            expression_con_effetti(&batch, &cfg, &crate::Limits::default()).expect("due");
        assert_eq!(effetti.righe_divisione_per_zero, 5);
        // Un ramo `case` non scelto non conta.
        let cfg = config(
            json!({"kind": "case",
                   "branches": [{"when": lit(json!(false)), "then": divisione}],
                   "else_value": lit(json!(1))}),
            None,
        );
        let (_, effetti) =
            expression_con_effetti(&batch, &cfg, &crate::Limits::default()).expect("case");
        assert_eq!(effetti.righe_divisione_per_zero, 0);
    }

    #[test]
    fn on_division_by_zero_senza_divisioni_si_rifiuta() {
        let batch = fixture();
        for politica in ["null", "error"] {
            let cfg = con_politica(bin("add", col("n"), lit(json!(1))), politica);
            let errore = expression(&batch, &cfg).expect_err("senza divisioni");
            assert!(
                errore
                    .to_string()
                    .contains("on_division_by_zero senza effetto"),
                "{errore}"
            );
        }
        // Un valore sconosciuto si rifiuta dalla config.
        assert!(serde_json::from_value::<ExpressionTransform>(json!({
            "output_column": "out", "on_division_by_zero": "zero",
            "expression": bin("divide", col("n"), col("i"))}))
        .is_err());
    }

    #[test]
    fn divisione_per_zero_riporta_diagnostica_row_scoped() {
        let batch = fixture();
        // Divisore dipendente dalla riga (i * 0): con `error` il rifiuto
        // resta row-scoped.
        let cfg = con_politica(
            bin(
                "divide",
                col("i"),
                bin("multiply", col("i"), lit(json!(0.0))),
            ),
            "error",
        );
        // Righe difettose: 0, 2, 3, 4, 5 (riga 1 null -> null, nessun errore).
        let error = expression(&batch, &cfg).expect_err("divisione per zero");
        let report = error
            .row_diagnostics()
            .expect("diagnostica row-scoped presente");
        assert_eq!(
            report.completeness,
            plenora_core::diagnostics::RowDiagnosticsCompleteness::Complete
        );
        assert_eq!(report.observed_total, 5);
        assert_eq!(report.total, Some(5));
        assert_eq!(report.counts["evaluation.division_by_zero"], 5);
        assert_eq!(report.counts.len(), 1);
        let indices: Vec<u64> = report.examples.iter().map(|row| row.source_index).collect();
        assert_eq!(indices, vec![0, 2, 3, 4, 5]);
        assert!(!report.examples_truncated);
        assert!(report.validate_for_emission().is_ok());
        // Parita' fast/generico anche sul payload, non solo sul testo.
        let generic = expression_generic(&batch, &cfg).expect_err("divisione per zero");
        assert_eq!(error.row_diagnostics(), generic.row_diagnostics());
    }

    #[test]
    fn numeri_non_finiti_riportano_diagnostica_row_scoped() {
        let batch = fixture();
        let cfg = config(col("nan"), None);
        let error = expression(&batch, &cfg).expect_err("NaN in colonna");
        let report = error
            .row_diagnostics()
            .expect("diagnostica row-scoped presente");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.counts["evaluation.non_finite_input"], 1);
        assert_eq!(report.examples[0].source_index, 1);
        let generic = expression_generic(&batch, &cfg).expect_err("NaN in colonna");
        assert_eq!(error.row_diagnostics(), generic.row_diagnostics());
    }

    #[test]
    fn oracle_ast_profondo_e_batch_vuoto() {
        let batch = fixture();
        // Catena di negate annidati (profondita' 60, entro il limite di audit).
        let mut deep = col("n");
        for _ in 0..60 {
            deep = un("negate", deep);
        }
        assert_equivalent(&batch, deep, None);
        // Catena binaria profonda a sinistra.
        let mut left_deep = lit(json!(1));
        for _ in 0..60 {
            left_deep = bin("add", left_deep, col("i"));
        }
        assert_equivalent(&batch, left_deep, None);

        // Batch vuoto: il tipo di output si ricava dallo SCHEMA, quindi le
        // colonne vanno risolte anche senza righe da valutare: altrimenti un
        // batch vuoto accetterebbe una colonna inesistente e un letterale
        // non scalare, cioe' lo stesso piano riuscirebbe o fallirebbe a
        // seconda dei dati.
        let empty = single_column_batch(
            "n",
            Arc::new(Float64Array::from(Vec::<f64>::new())),
            DataType::Float64,
            true,
        );
        for ast in [col("missing"), lit(json!([1, 2]))] {
            let config = config(ast, None);
            expression(&empty, &config)
                .expect_err("il tipo non e' determinabile: nessuna risposta giusta");
        }
        // Un'espressione valida resta valida, con lo stesso tipo che
        // avrebbe su dati pieni.
        let config = config(col("n"), None);
        let output = expression(&empty, &config).expect("zero righe");
        assert_eq!(output.num_rows(), 0);
        assert_eq!(
            output
                .schema()
                .field_with_name("out")
                .expect("out")
                .data_type(),
            &DataType::Float64
        );
        let generic = expression_generic(&empty, &config).expect("generico");
        assert_eq!(output, generic);
    }

    #[test]
    fn oracle_nuove_funzioni() {
        let batch = fixture();
        let cases = vec![
            // substring: start 0-based, conteggio per carattere Unicode.
            func("substring", vec![col("s"), lit(json!(1))]),
            func("substring", vec![col("s"), lit(json!(1)), lit(json!(2))]),
            func(
                "substring",
                vec![
                    lit(json!("héllo\u{1F600}world")),
                    lit(json!(0)),
                    lit(json!(6)),
                ],
            ),
            func(
                "substring",
                vec![
                    lit(json!("héllo\u{1F600}world")),
                    lit(json!(6)),
                    lit(json!(1)),
                ],
            ),
            // start oltre la lunghezza -> vuota; len oltre -> troncata.
            func("substring", vec![col("s"), lit(json!(99))]),
            func("substring", vec![col("s"), lit(json!(0)), lit(json!(99))]),
            // Non interi troncati verso zero; -0.0 vale 0.
            func(
                "substring",
                vec![col("s"), lit(json!(1.9)), lit(json!(2.5))],
            ),
            func("substring", vec![col("s"), lit(json!(-0.0))]),
            // start negativo -> errore; null propagati (anche len null).
            func("substring", vec![col("s"), lit(json!(-1))]),
            func("substring", vec![col("s"), lit(Value::Null)]),
            func("substring", vec![col("s"), lit(json!(0)), lit(Value::Null)]),
            func("substring", vec![col("n"), lit(json!(0))]),
            func("substring", vec![col("s")]),
            // regex_replace: gruppi $1 e $name, regex non valida, null.
            func(
                "regex_replace",
                vec![col("s"), lit(json!("(\\d+)")), lit(json!("[$1]"))],
            ),
            func(
                "regex_replace",
                vec![
                    col("s"),
                    lit(json!("(?P<letter>[a-z]+)")),
                    lit(json!("$letter$letter")),
                ],
            ),
            func(
                "regex_replace",
                vec![
                    lit(json!("abc123")),
                    lit(json!("(bc)(\\d)")),
                    lit(json!("$2$1")),
                ],
            ),
            func(
                "regex_replace",
                vec![col("s"), lit(json!("([")), lit(json!("x"))],
            ),
            func("regex_replace", vec![col("s"), col("s"), lit(json!("x"))]),
            func("regex_replace", vec![col("s"), col("n"), lit(json!("x"))]),
            // Null nel valore: nessuna compilazione della regex (lazy).
            func(
                "regex_replace",
                vec![lit(Value::Null), lit(json!("([")), lit(json!("x"))],
            ),
            // between: inclusivo, numerico e testuale; null -> Null.
            func("between", vec![col("n"), lit(json!(0.0)), lit(json!(4.0))]),
            func("between", vec![col("s"), lit(json!("a")), lit(json!("c"))]),
            func("between", vec![col("n"), lit(Value::Null), lit(json!(1))]),
            func("between", vec![col("n"), lit(json!(1)), col("s")]),
            func("between", vec![col("i"), lit(json!(10)), lit(json!(0))]),
            func("between", vec![col("n"), lit(json!(0.0))]),
            // in: membership su letterali; null propagato; errori di forma.
            func("in", vec![col("i"), lit(json!([1, 3, 10]))]),
            func("in", vec![col("s"), lit(json!(["ab", "x42y", null]))]),
            func("in", vec![col("n"), lit(json!([]))]),
            func("in", vec![col("s"), lit(json!([1, 2]))]),
            func("in", vec![col("s"), lit(json!("ab"))]),
            func("in", vec![col("s"), lit(json!([[1]]))]),
            func("in", vec![col("s")]),
            // greatest/least: N-ari, null propagato, -0.0 uguale a 0.0.
            func("greatest", vec![col("n"), lit(json!(2.0)), col("i")]),
            func("least", vec![col("n"), lit(json!(2.0)), col("i")]),
            func("greatest", vec![lit(json!(-0.0)), lit(json!(0.0))]),
            func("least", vec![lit(json!(-0.0)), lit(json!(0.0))]),
            func("greatest", vec![col("s"), lit(json!("m"))]),
            func("greatest", vec![lit(json!(5))]),
            func("greatest", vec![]),
            func("greatest", vec![col("n"), col("s")]),
            func("least", vec![lit(Value::Null), lit(json!(1))]),
            // floor/ceil/power: numeriche, risultati non finiti rifiutati.
            func("floor", vec![col("n")]),
            func("ceil", vec![col("n")]),
            func("floor", vec![lit(json!(-2.5))]),
            func("ceil", vec![lit(json!(-2.5))]),
            func("power", vec![col("n"), lit(json!(2))]),
            func("power", vec![lit(json!(0.0)), lit(json!(0.0))]),
            func("power", vec![lit(json!(-2.0)), lit(json!(0.5))]),
            func("power", vec![lit(json!(1e308)), lit(json!(2))]),
            func("power", vec![col("s"), lit(json!(2))]),
            func("power", vec![col("n")]),
        ];
        for expression in cases {
            assert_equivalent(&batch, expression, None);
        }
    }

    #[test]
    fn oracle_date_trunc() {
        let batch = fixture();
        for unit in ["year", "month", "day"] {
            assert_equivalent(
                &batch,
                func("date_trunc", vec![lit(json!(unit)), col("d")]),
                None,
            );
            assert_equivalent(
                &batch,
                func("date_trunc", vec![lit(json!(unit)), col("ts")]),
                None,
            );
        }
        for unit in ["hour", "minute", "second"] {
            assert_equivalent(
                &batch,
                func("date_trunc", vec![lit(json!(unit)), col("ts")]),
                None,
            );
            // Unita' sub-day su Date32: errore in entrambi i percorsi.
            assert_equivalent(
                &batch,
                func("date_trunc", vec![lit(json!(unit)), col("d")]),
                None,
            );
        }
        let cases = vec![
            // Unita' non valida o non letterale.
            func("date_trunc", vec![lit(json!("week")), col("ts")]),
            func("date_trunc", vec![col("s"), col("ts")]),
            // Input testuale: nessun parsing implicito -> errore.
            func("date_trunc", vec![lit(json!("day")), col("s")]),
            // Timezone-aware rifiutato: la semantica di fuso del troncamento
            // non e' definita.
            func("date_trunc", vec![lit(json!("day")), col("tstz")]),
            func("date_trunc", vec![lit(json!("day")), col("missing")]),
            func("date_trunc", vec![lit(json!("day"))]),
            // Annidamento e letterale null.
            func(
                "date_trunc",
                vec![
                    lit(json!("year")),
                    func("date_trunc", vec![lit(json!("month")), col("ts")]),
                ],
            ),
            func("date_trunc", vec![lit(json!("day")), lit(Value::Null)]),
            // Ramo case non percorso: nessun errore (lazy); percorso: errore.
            case(
                vec![(
                    lit(json!(false)),
                    func("date_trunc", vec![lit(json!("day")), col("missing")]),
                )],
                lit(json!(1)),
            ),
            case(
                vec![(
                    lit(json!(true)),
                    func("date_trunc", vec![lit(json!("day")), col("s")]),
                )],
                lit(json!(1)),
            ),
        ];
        for expression in cases {
            assert_equivalent(&batch, expression, None);
        }
        // output_type espliciti (coerenti e non).
        assert_equivalent(
            &batch,
            func("date_trunc", vec![lit(json!("month")), col("d")]),
            Some("date32"),
        );
        assert_equivalent(
            &batch,
            func("date_trunc", vec![lit(json!("month")), col("d")]),
            Some("timestamp_ms"),
        );
        assert_equivalent(
            &batch,
            func("date_trunc", vec![lit(json!("hour")), col("ts")]),
            Some("timestamp_ms"),
        );
        assert_equivalent(
            &batch,
            func("date_trunc", vec![lit(json!("hour")), col("ts")]),
            Some("text"),
        );
    }

    #[test]
    fn date_trunc_valori_e_tipi_nativi() {
        let batch = fixture();
        // Date32: year/month sul 2022-01-08 = 19000 -> 2022-01-01 = 18993.
        let cfg = config(func("date_trunc", vec![lit(json!("year")), col("d")]), None);
        let output = expression(&batch, &cfg).expect("date_trunc year");
        let values = output
            .column(output.schema().index_of("out").expect("out"))
            .as_any()
            .downcast_ref::<Date32Array>()
            .expect("Date32");
        assert_eq!(values.data_type(), &DataType::Date32);
        assert_eq!(values.value(0), 0); // 1970-01-01
        assert!(values.is_null(1));
        assert_eq!(values.value(2), 18_993); // 2022-01-01
        assert_eq!(values.value(3), -365); // 1969-01-01
        assert_eq!(values.value(4), 0); // 1970-01-01
        assert_eq!(values.value(5), 19_723); // 2024-01-01

        let cfg = config(func("date_trunc", vec![lit(json!("day")), col("d")]), None);
        let output = expression(&batch, &cfg).expect("date_trunc day");
        let values = output
            .column(output.schema().index_of("out").expect("out"))
            .as_any()
            .downcast_ref::<Date32Array>()
            .expect("Date32");
        assert_eq!(values.value(2), 19_000); // day: identita'

        // Timestamp(ms): troncamenti aritmetici con rem_euclid (pre-1970).
        let cfg = config(
            func("date_trunc", vec![lit(json!("second")), col("ts")]),
            None,
        );
        let output = expression(&batch, &cfg).expect("date_trunc second");
        let values = output
            .column(output.schema().index_of("out").expect("out"))
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .expect("TimestampMs");
        assert_eq!(
            values.data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None)
        );
        assert_eq!(values.value(0), 86_400_000 * 3 + 3_600_000 * 5 + 61_000);
        assert!(values.is_null(1));
        assert_eq!(values.value(2), 0);
        assert_eq!(values.value(3), -1_000); // -1 ms -> secondo precedente
        assert_eq!(values.value(4), 1_700_000_000_000);
        assert_eq!(values.value(5), -86_400_000);

        // month/year su timestamp via calendario UTC.
        let cfg = config(
            func("date_trunc", vec![lit(json!("month")), col("ts")]),
            None,
        );
        let output = expression(&batch, &cfg).expect("date_trunc month");
        let values = output
            .column(output.schema().index_of("out").expect("out"))
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .expect("TimestampMs");
        assert_eq!(values.value(0), 0); // 1970-01-01
        assert_eq!(values.value(4), 1_698_796_800_000); // 2023-11-01
        assert_eq!(values.value(5), -2_678_400_000); // 1969-12-01
    }

    #[test]
    fn date_trunc_all_null_e_batch_vuoto_tipizzati() {
        // Tutto null: il tipo esce dalla colonna di input, MAI Utf8.
        let all_null = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("d", DataType::Date32, true),
                Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            ])),
            vec![
                Arc::new(Date32Array::from(vec![None::<i32>, None])),
                Arc::new(TimestampMillisecondArray::from(vec![None::<i64>, None])),
            ],
        )
        .expect("all-null");
        let cfg = config(
            func("date_trunc", vec![lit(json!("month")), col("d")]),
            None,
        );
        let output = expression(&all_null, &cfg).expect("all-null Date32");
        assert_eq!(output.num_rows(), 2);
        assert_eq!(
            output
                .schema()
                .field_with_name("out")
                .expect("out")
                .data_type(),
            &DataType::Date32
        );
        let generic = expression_generic(&all_null, &cfg).expect("generico");
        assert_eq!(output, generic);
        let cfg = config(
            func("date_trunc", vec![lit(json!("hour")), col("ts")]),
            None,
        );
        let output = expression(&all_null, &cfg).expect("all-null TimestampMs");
        assert_eq!(
            output
                .schema()
                .field_with_name("out")
                .expect("out")
                .data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None)
        );
        let generic = expression_generic(&all_null, &cfg).expect("generico");
        assert_eq!(output, generic);

        // Batch vuoto: stessa tipizzazione dalla colonna di input.
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("d", DataType::Date32, true),
                Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            ])),
            vec![
                Arc::new(Date32Array::from(Vec::<i32>::new())),
                Arc::new(TimestampMillisecondArray::from(Vec::<i64>::new())),
            ],
        )
        .expect("empty");
        let cfg = config(func("date_trunc", vec![lit(json!("year")), col("d")]), None);
        let output = expression(&empty, &cfg).expect("vuoto Date32");
        assert_eq!(output.num_rows(), 0);
        assert_eq!(
            output
                .schema()
                .field_with_name("out")
                .expect("out")
                .data_type(),
            &DataType::Date32
        );
        let cfg = config(
            func("date_trunc", vec![lit(json!("minute")), col("ts")]),
            None,
        );
        let output = expression(&empty, &cfg).expect("vuoto TimestampMs");
        assert_eq!(
            output
                .schema()
                .field_with_name("out")
                .expect("out")
                .data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None)
        );
        // Radice non date_trunc: il tipo viene dallo SCHEMA, non dai valori
        // osservati. Una colonna Date32 letta direttamente e' un numero per
        // il runtime, quindi l'output e' Float64 — su batch vuoto come su
        // batch pieno. Deciderlo dai valori darebbe Utf8, cioe' uno schema
        // diverso a parita' di configurazione e di schema d'ingresso.
        let cfg = config(col("d"), None);
        let output = expression(&empty, &cfg).expect("vuoto non temporale");
        assert_eq!(
            output
                .schema()
                .field_with_name("out")
                .expect("out")
                .data_type(),
            &DataType::Float64
        );
    }

    #[test]
    fn validate_rifiuta_unita_e_liste_non_valide() {
        // Unita' fuori dal set chiuso o non letterale: errore in validazione.
        let bad = config(func("date_trunc", vec![lit(json!("week")), col("d")]), None);
        assert!(validate(&bad, 100).is_err());
        let bad = config(func("date_trunc", vec![col("s"), col("d")]), None);
        assert!(validate(&bad, 100).is_err());
        let bad = config(func("date_trunc", vec![lit(json!("day"))]), None);
        assert!(validate(&bad, 100).is_err());
        // in: il secondo argomento deve essere una lista di letterali scalari.
        let bad = config(func("in", vec![col("s"), col("s")]), None);
        assert!(validate(&bad, 100).is_err());
        let bad = config(func("in", vec![col("s"), lit(json!([[1]]))]), None);
        assert!(validate(&bad, 100).is_err());
        // Forme valide accettate.
        let good = config(
            func("date_trunc", vec![lit(json!("month")), col("d")]),
            None,
        );
        validate(&good, 100).expect("date_trunc valido");
        let good = config(
            func("in", vec![col("s"), lit(json!(["a", 1, null, true]))]),
            None,
        );
        validate(&good, 100).expect("in valido");
    }

    /// Colonne i cui valori un double non distingue: interi oltre 2^53,
    /// decimali frazionari, gli estremi di `i64`/`u64` e lo zero negativo.
    fn fixture_esatta() -> RecordBatch {
        use plenora_core::arrow::array::Decimal128Array;
        let decimali = Decimal128Array::from(vec![Some(10_i128), Some(10), Some(-5), Some(0)])
            .with_precision_and_scale(38, 2)
            .expect("decimal128");
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("i", DataType::Int64, true),
                Field::new("u", DataType::UInt64, true),
                Field::new("dec", DataType::Decimal128(38, 2), true),
                Field::new("f", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![
                    Some((1_i64 << 53) + 1),
                    Some(i64::MAX),
                    Some(i64::MIN),
                    Some(-((1_i64 << 53) + 1)),
                ])),
                Arc::new(UInt64Array::from(vec![
                    Some((1_u64 << 53) + 1),
                    Some(u64::MAX),
                    Some(0),
                    Some(1_u64 << 63),
                ])),
                Arc::new(decimali),
                Arc::new(Float64Array::from(vec![
                    Some(0.1),
                    Some(9_007_199_254_740_992.0),
                    Some(-0.0),
                    Some(0.0),
                ])),
            ],
        )
        .expect("fixture esatta")
    }

    /// Gli esiti di un'espressione booleana, dal percorso generico e dal fast
    /// path, che devono coincidere.
    fn booleani(batch: &RecordBatch, expression: Value) -> Vec<Option<bool>> {
        assert_equivalent(batch, expression.clone(), None);
        let config = config(expression, None);
        let risultato = expression_generic(batch, &config).expect("espressione valida");
        let colonna = risultato
            .column_by_name("out")
            .expect("out")
            .as_any()
            .downcast_ref::<BooleanArray>()
            .expect("booleani")
            .clone();
        (0..colonna.len())
            .map(|riga| (!colonna.is_null(riga)).then(|| colonna.value(riga)))
            .collect()
    }

    #[test]
    fn i_confronti_decidono_sul_valore_esatto() {
        let batch = fixture_esatta();
        // 2^53 + 1 non e' 2^53, anche se il double dei due e' lo stesso.
        assert_eq!(
            booleani(
                &batch,
                bin("equal", col("i"), lit(json!(9_007_199_254_740_992_i64)))
            )[0],
            Some(false)
        );
        assert_eq!(
            booleani(
                &batch,
                bin("greater", col("i"), lit(json!(9_007_199_254_740_992_i64)))
            )[0],
            Some(true)
        );
        assert_eq!(
            booleani(&batch, bin("equal", col("u"), col("i")))[0],
            Some(true),
            "stesso intero in Int64 e UInt64"
        );
        // i64::MAX e u64::MAX contro il double 2^63 e 2^64 in cui arrotondano.
        assert_eq!(
            booleani(
                &batch,
                bin("less", col("i"), lit(json!(9_223_372_036_854_775_808_f64)))
            )[1],
            Some(true)
        );
        assert_eq!(
            booleani(&batch, bin("equal", col("u"), lit(json!(u64::MAX))))[1],
            Some(true)
        );
        // Un intero oltre 2^53 contro un Float64: il double 2^53 e' minore.
        assert_eq!(
            booleani(&batch, bin("greater", col("i"), col("f")))[0],
            Some(true)
        );
    }

    #[test]
    fn un_letterale_decimale_vale_come_e_scritto() {
        let batch = fixture_esatta();
        // Decimal128 0.10 e il letterale 0.1: stesso valore, come in table.filter.
        assert_eq!(
            booleani(&batch, bin("equal", col("dec"), lit(json!(0.1))))[0],
            Some(true)
        );
        // Il double 0.1 e' 0.1000000000000000055…: maggiore del decimale 0,1.
        assert_eq!(
            booleani(&batch, bin("greater", col("f"), lit(json!(0.1))))[0],
            Some(true)
        );
        assert_eq!(
            booleani(&batch, bin("equal", col("f"), col("dec")))[0],
            Some(false)
        );
        // Lo zero negativo e' zero.
        assert_eq!(
            booleani(&batch, bin("equal", col("f"), lit(json!(0.0))))[2..],
            [Some(true), Some(true)]
        );
    }

    #[test]
    fn negate_e_abs_restano_esatti() {
        let batch = fixture_esatta();
        // -(2^53 + 1) e' l'opposto esatto, e |i64::MIN| e' 2^63.
        assert_eq!(
            booleani(&batch, bin("equal", un("negate", col("i")), col("i")))[..],
            [Some(false), Some(false), Some(false), Some(false)]
        );
        assert_eq!(
            booleani(
                &batch,
                bin(
                    "equal",
                    un("negate", col("i")),
                    lit(json!(-9_007_199_254_740_993_i64))
                )
            )[0],
            Some(true)
        );
        assert_eq!(
            booleani(
                &batch,
                bin(
                    "equal",
                    func("abs", vec![col("i")]),
                    lit(json!(9_223_372_036_854_775_808_u64))
                )
            )[2],
            Some(true)
        );
        assert_eq!(
            booleani(&batch, bin("equal", func("abs", vec![col("i")]), col("u")))[0],
            Some(true)
        );
    }

    #[test]
    fn oracle_confronti_esatti() {
        let batch = fixture_esatta();
        let operandi = [
            col("i"),
            col("u"),
            col("dec"),
            col("f"),
            un("negate", col("i")),
            func("abs", vec![col("dec")]),
            bin("add", col("i"), lit(json!(0))),
            lit(json!(0.1)),
            lit(json!(9_007_199_254_740_993_i64)),
            lit(json!(u64::MAX)),
            lit(json!(-0.0)),
        ];
        for op in [
            "equal",
            "not_equal",
            "greater",
            "greater_equal",
            "less",
            "less_equal",
        ] {
            for sinistro in &operandi {
                for destro in &operandi {
                    assert_equivalent(&batch, bin(op, sinistro.clone(), destro.clone()), None);
                }
            }
        }
        for nome in ["greatest", "least"] {
            assert_equivalent(&batch, func(nome, operandi.to_vec()), None);
        }
        assert_equivalent(
            &batch,
            func("between", vec![col("i"), col("f"), col("u")]),
            None,
        );
        assert_equivalent(
            &batch,
            func("null_if", vec![col("dec"), lit(json!(0.1))]),
            None,
        );
    }

    /// Scala negativa, timestamp oltre 2^53, e un `Decimal128` a `i128::MIN`
    /// (fuori da ogni precisione, ma Arrow non verifica i valori).
    fn fixture_estremi() -> RecordBatch {
        use plenora_core::arrow::array::{Decimal128Array, TimestampMillisecondArray};
        let scala_negativa = Decimal128Array::from(vec![Some(5_i128), Some(-3)])
            .with_precision_and_scale(10, -2)
            .expect("decimal128 a scala negativa");
        let minimo = Decimal128Array::from(vec![Some(i128::MIN), Some(1)])
            .with_precision_and_scale(38, 0)
            .expect("decimal128");
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("neg", DataType::Decimal128(10, -2), true),
                Field::new("min", DataType::Decimal128(38, 0), true),
                Field::new(
                    "ts",
                    DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                    true,
                ),
                Field::new("i", DataType::Int64, true),
            ])),
            vec![
                Arc::new(scala_negativa),
                Arc::new(minimo),
                Arc::new(
                    TimestampMillisecondArray::from(vec![Some((1_i64 << 53) + 1), Some(0)])
                        .with_timezone("UTC"),
                ),
                Arc::new(Int64Array::from(vec![Some((1_i64 << 53) + 1), Some(-1)])),
            ],
        )
        .expect("fixture estremi")
    }

    #[test]
    fn scale_negative_esponenziali_e_timestamp_restano_esatti() {
        let batch = fixture_estremi();
        // 5 * 10^2 = 500.
        assert_eq!(
            booleani(&batch, bin("equal", col("neg"), lit(json!(500))))[..],
            [Some(true), Some(false)]
        );
        // Un timestamp oltre 2^53 non e' il suo vicino.
        assert_eq!(
            booleani(
                &batch,
                bin("equal", col("ts"), lit(json!(9_007_199_254_740_992_i64)))
            )[0],
            Some(false)
        );
        assert_eq!(
            booleani(&batch, bin("equal", col("ts"), col("i")))[0],
            Some(true)
        );
        // La notazione esponenziale e' un decimale esatto: 1e30 supera ogni
        // i64. Oltre la forma esatta (1e300: 301 cifre) il letterale si
        // rifiuta invece di diventare un double (revisione Codex).
        assert_eq!(
            booleani(&batch, bin("less", col("i"), lit(json!(1e30))))[..],
            [Some(true), Some(true)]
        );
        let oltre = config(bin("less", col("i"), lit(json!(1e300))), None);
        assert!(matches!(
            expression_generic(&batch, &oltre),
            Err(PlenoraError::InvalidPlan(_))
        ));
        assert_eq!(
            booleani(&batch, bin("greater", col("i"), lit(json!(1e-7))))[..],
            [Some(true), Some(false)]
        );
    }

    #[test]
    fn in_coalesce_e_case_portano_il_valore_esatto() {
        let batch = fixture_estremi();
        assert_eq!(
            booleani(
                &batch,
                func(
                    "in",
                    vec![col("i"), lit(json!([9_007_199_254_740_992_i64]))]
                )
            )[0],
            Some(false)
        );
        assert_eq!(
            booleani(
                &batch,
                bin(
                    "equal",
                    func("coalesce", vec![col("i")]),
                    lit(json!(9_007_199_254_740_993_i64))
                )
            )[0],
            Some(true)
        );
        assert_eq!(
            booleani(
                &batch,
                bin(
                    "equal",
                    case(vec![(lit(json!(true)), col("i"))], lit(json!(0))),
                    lit(json!(9_007_199_254_740_992_i64))
                )
            )[0],
            Some(false)
        );
    }

    #[test]
    fn l_opposto_di_un_decimale_fuori_dominio_e_un_errore() {
        let batch = fixture_estremi();
        for espressione in [un("negate", col("min")), func("abs", vec![col("min")])] {
            assert_equivalent(&batch, espressione.clone(), None);
            let config = config(espressione, None);
            let errore = expression_generic(&batch, &config).expect_err("fuori dominio");
            assert!(matches!(errore, PlenoraError::Schema(_)), "{errore}");
        }
    }

    /// Regressione (revisione Codex, classe dei panici di intervallo di
    /// chrono): una Date32 oltre l'intervallo di chrono e' un errore di
    /// `date_trunc`, non un panico della somma.
    #[test]
    fn date_trunc_su_date32_fuori_da_chrono_e_un_errore() {
        assert!(
            super::temporal::trunc_date32_days(i32::MAX, super::temporal::TruncUnit::Year).is_err()
        );
        assert!(
            super::temporal::trunc_date32_days(i32::MIN, super::temporal::TruncUnit::Day).is_err()
        );
    }

    /// Regressione (revisione Codex): il troncamento di un timestamp vicino
    /// a `i64::MIN` usciva dalla gamma nella sottrazione; ora e' un errore.
    #[test]
    fn date_trunc_su_timestamp_al_minimo_e_un_errore() {
        for unit in [
            super::temporal::TruncUnit::Second,
            super::temporal::TruncUnit::Minute,
            super::temporal::TruncUnit::Hour,
            super::temporal::TruncUnit::Day,
        ] {
            assert!(super::temporal::trunc_timestamp_ms_value(i64::MIN, unit).is_err());
            assert!(super::temporal::trunc_timestamp_ms_value(i64::MAX, unit).is_ok());
        }
    }
}
