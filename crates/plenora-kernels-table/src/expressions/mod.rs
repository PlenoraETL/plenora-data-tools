//! Kernel `table.expression`: valutazione di espressioni scalari su colonne
//! (interprete generico e fast path compilato, stessa semantica).
//!
//! I sottomoduli e cio' che ciascuno possiede:
//!
//! - [`scalar`]: valore scalare generico (`Scalar`) con coercizioni,
//!   confronti e aritmetica/logica dell'interprete;
//! - [`temporal`]: macchina temporale di `date_trunc` (troncamenti
//!   Date32/Timestamp ms) e valutazione di `in`;
//! - [`interpreter`]: inferenza del tipo temporale, funzioni scalari,
//!   interprete ricorsivo sull'AST, validazione statica e percorso generico
//!   di output;
//! - [`fast`]: fast path compilato (`FastNode`/`FastProgram`), verificato
//!   dagli test-oracolo contro il percorso generico;
//! - [`static_type`]: tipo statico dell'AST ricavato dal solo SCHEMA,
//!   sorgente unica per il kernel e per l'analizzatore del contratto.

mod fast;
mod interpreter;
mod scalar;
pub mod static_type;
mod temporal;

use serde::Deserialize;
use serde_json::Value;

pub use interpreter::{expression, validate};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    Auto,
    Number,
    Boolean,
    Text,
    /// Date32 nativo (prodotto da `date_trunc` su colonna Date32).
    Date32,
    /// Timestamp(ms) nativo senza timezone (prodotto da `date_trunc`).
    TimestampMs,
}

const fn default_output_type() -> OutputType {
    OutputType::Auto
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionTransform {
    pub output_column: String,
    pub expression: Expression,
    #[serde(default = "default_output_type")]
    pub output_type: OutputType,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expression {
    Column {
        name: String,
    },
    Literal {
        value: Value,
    },
    Unary {
        op: UnaryOperator,
        value: Box<Self>,
    },
    Binary {
        op: BinaryOperator,
        left: Box<Self>,
        right: Box<Self>,
    },
    Function {
        name: Function,
        args: Vec<Self>,
    },
    Case {
        branches: Vec<CaseBranch>,
        else_value: Box<Self>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseBranch {
    pub when: Expression,
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

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOperator {
    Not,
    Negate,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Function {
    Coalesce,
    NullIf,
    Lower,
    Upper,
    Trim,
    Length,
    Concat,
    Contains,
    StartsWith,
    EndsWith,
    Abs,
    Round,
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
    Greatest,
    Least,
    Floor,
    Ceil,
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

    /// Fixture con null, -0.0, zeri, testi (anche data-like) e booleani.
    ///
    /// La colonna `nan` contiene NaN: la lettura deve fallire in entrambi i
    /// percorsi ("expression non accetta numeri non finiti"). `ts` e `tstz`
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
                // 1970-01-04 05:01:01.007 UTC, epoca, pre-1970, 2023-11-14.
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
            // Numeri: include -0.0 vs 0.0 (total_cmp: -0.0 < 0.0).
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
        // Controllo: divisore dipendente dalla riga -> row-scoped invariato.
        let cfg = config(
            bin(
                "divide",
                col("i"),
                bin("multiply", col("i"), lit(json!(0.0))),
            ),
            None,
        );
        let error = expression(&batch, &cfg).expect_err("divisione calcolata");
        assert!(error.row_diagnostics().is_some());
    }

    #[test]
    fn divisione_per_zero_riporta_diagnostica_row_scoped() {
        let batch = fixture();
        // Divisore dipendente dalla riga (i * 0): il rifiuto resta row-scoped.
        let cfg = config(
            bin(
                "divide",
                col("i"),
                bin("multiply", col("i"), lit(json!(0.0))),
            ),
            None,
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
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("n", DataType::Float64, true)])),
            vec![Arc::new(Float64Array::from(Vec::<f64>::new()))],
        )
        .expect("empty");
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
            // greatest/least: N-ari, null propagato, total_cmp su -0.0.
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
            // Timezone-aware rifiutato (decisione documentata).
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
        // Date32: year/month sul 2022-01-08 (19000) -> 2022-01-01 (18993).
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
}
