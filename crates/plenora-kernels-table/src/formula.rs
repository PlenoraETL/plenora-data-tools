use std::borrow::Cow;
use std::sync::Arc;

use num_traits::ToPrimitive;
use plenora_core::arrow::array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use plenora_core::{PlenoraError, Result};
use crate::{
    column_index, replace_or_append, scalar_as_f64, scalar_as_string, validate_output_name,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Formula {
    pub new_column: String,
    pub formula: String,
}

#[derive(Debug, Clone)]
enum Expr {
    Number(f64),
    Text(String),
    Column(String),
    Neg(Box<Self>),
    Binary(Box<Self>, char, Box<Self>),
}

struct Parser<'a> {
    input: &'a [u8],
    position: usize,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self
            .input
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
    }
    fn expression(&mut self) -> Result<Expr> {
        let mut left = self.term()?;
        loop {
            self.skip_space();
            let Some(operator @ (b'+' | b'-')) = self.input.get(self.position).copied() else {
                break;
            };
            self.position += 1;
            left = Expr::Binary(Box::new(left), char::from(operator), Box::new(self.term()?));
        }
        Ok(left)
    }
    fn term(&mut self) -> Result<Expr> {
        let mut left = self.factor()?;
        loop {
            self.skip_space();
            let Some(operator @ (b'*' | b'/')) = self.input.get(self.position).copied() else {
                break;
            };
            self.position += 1;
            left = Expr::Binary(
                Box::new(left),
                char::from(operator),
                Box::new(self.factor()?),
            );
        }
        Ok(left)
    }
    fn factor(&mut self) -> Result<Expr> {
        self.skip_space();
        let Some(current) = self.input.get(self.position).copied() else {
            return Err(PlenoraError::Contract("formula incompleta".into()));
        };
        if current == b'-' {
            self.position += 1;
            return Ok(Expr::Neg(Box::new(self.factor()?)));
        }
        if current == b'(' {
            self.position += 1;
            let expression = self.expression()?;
            self.skip_space();
            if self.input.get(self.position) != Some(&b')') {
                return Err(PlenoraError::Contract(
                    "parentesi formula non bilanciate".into(),
                ));
            }
            self.position += 1;
            return Ok(expression);
        }
        if current == b'\'' || current == b'"' {
            return self.text(current);
        }
        if current.is_ascii_digit() || current == b'.' {
            return self.number();
        }
        if current.is_ascii_alphabetic() || current == b'_' {
            return self.identifier();
        }
        Err(PlenoraError::Contract(format!(
            "carattere formula non ammesso: {}",
            char::from(current)
        )))
    }
    fn text(&mut self, quote: u8) -> Result<Expr> {
        self.position += 1;
        let start = self.position;
        while self
            .input
            .get(self.position)
            .is_some_and(|value| *value != quote)
        {
            if self.input[self.position] == b'\\' {
                return Err(PlenoraError::Contract(
                    "escape nelle stringhe formula non supportato".into(),
                ));
            }
            self.position += 1;
        }
        if self.input.get(self.position) != Some(&quote) {
            return Err(PlenoraError::Contract(
                "stringa formula non terminata".into(),
            ));
        }
        let value = std::str::from_utf8(&self.input[start..self.position])
            .map_err(|_| PlenoraError::Contract("formula non UTF-8".into()))?
            .to_owned();
        self.position += 1;
        Ok(Expr::Text(value))
    }
    fn number(&mut self) -> Result<Expr> {
        let start = self.position;
        while self
            .input
            .get(self.position)
            .is_some_and(|value| value.is_ascii_digit() || *value == b'.')
        {
            self.position += 1;
        }
        if self
            .input
            .get(self.position)
            .is_some_and(|value| matches!(value, b'e' | b'E'))
        {
            self.position += 1;
            if self
                .input
                .get(self.position)
                .is_some_and(|value| matches!(value, b'+' | b'-'))
            {
                self.position += 1;
            }
            let exponent = self.position;
            while self
                .input
                .get(self.position)
                .is_some_and(u8::is_ascii_digit)
            {
                self.position += 1;
            }
            if exponent == self.position {
                return Err(PlenoraError::Contract("esponente formula non valido".into()));
            }
        }
        let value = std::str::from_utf8(&self.input[start..self.position])
            .ok()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| PlenoraError::Contract("numero formula non valido".into()))?;
        Ok(Expr::Number(value))
    }
    fn identifier(&mut self) -> Result<Expr> {
        let start = self.position;
        while self
            .input
            .get(self.position)
            .is_some_and(|value| value.is_ascii_alphanumeric() || *value == b'_')
        {
            self.position += 1;
        }
        Ok(Expr::Column(
            std::str::from_utf8(&self.input[start..self.position])
                .map_err(|_| PlenoraError::Contract("identificatore non UTF-8".into()))?
                .to_owned(),
        ))
    }
}

fn parse(input: &str) -> Result<Expr> {
    let mut parser = Parser {
        input: input.as_bytes(),
        position: 0,
    };
    let expression = parser.expression()?;
    parser.skip_space();
    if parser.position != parser.input.len() {
        return Err(PlenoraError::Contract("token extra nella formula".into()));
    }
    Ok(expression)
}

#[derive(Debug)]
enum Evaluated {
    Null,
    Number(f64),
    Text(String),
}

fn evaluate(expression: &Expr, batch: &RecordBatch, row: usize) -> Result<Evaluated> {
    Ok(match expression {
        Expr::Number(value) => Evaluated::Number(*value),
        Expr::Text(value) => Evaluated::Text(value.clone()),
        Expr::Column(name) => {
            let index = column_index(batch, name)?;
            if batch.column(index).is_null(row) {
                Evaluated::Null
            } else if matches!(
                batch.column(index).data_type(),
                DataType::Int64 | DataType::Float64
            ) {
                Evaluated::Number(
                    scalar_as_f64(batch.column(index).as_ref(), row)?.unwrap_or_default(),
                )
            } else {
                Evaluated::Text(
                    scalar_as_string(batch.column(index).as_ref(), row)?.unwrap_or_default(),
                )
            }
        }
        Expr::Neg(value) => match evaluate(value, batch, row)? {
            Evaluated::Number(value) => Evaluated::Number(-value),
            Evaluated::Null => Evaluated::Null,
            Evaluated::Text(_) => return Err(PlenoraError::Schema("negazione di testo".into())),
        },
        Expr::Binary(left, operator, right) => {
            let left = evaluate(left, batch, row)?;
            let right = evaluate(right, batch, row)?;
            match (left, right, operator) {
                (Evaluated::Null, _, _) | (_, Evaluated::Null, _) => Evaluated::Null,
                (Evaluated::Number(left), Evaluated::Number(right), '+') => {
                    Evaluated::Number(left + right)
                }
                (Evaluated::Number(left), Evaluated::Number(right), '-') => {
                    Evaluated::Number(left - right)
                }
                (Evaluated::Number(left), Evaluated::Number(right), '*') => {
                    Evaluated::Number(left * right)
                }
                (Evaluated::Number(_), Evaluated::Number(0.0), '/') => {
                    return Err(PlenoraError::Schema("divisione per zero".into()))
                }
                (Evaluated::Number(left), Evaluated::Number(right), '/') => {
                    Evaluated::Number(left / right)
                }
                (left, right, '+') => {
                    Evaluated::Text(format!("{}{}", display(left), display(right)))
                }
                _ => {
                    return Err(PlenoraError::Schema(
                        "operatore formula incompatibile con testo".into(),
                    ))
                }
            }
        }
    })
}

fn display(value: Evaluated) -> String {
    match value {
        Evaluated::Null => String::new(),
        Evaluated::Number(value) => value.to_string(),
        Evaluated::Text(value) => value,
    }
}

// ---------------------------------------------------------------------------
// Fast path compilato (ottimizzazione kernel `table.formula`, ultimo batch).
//
// L'AST viene compilato UNA VOLTA in bytecode postfix: indici di colonna
// risolti e downcast degli array fatti in compilazione, letterali
// pre-materializzati, nessuna allocazione per riga sul tier numerico.
// Due tier:
// - numerico: tutte le foglie sono numeri/colonne Int64-Float64 e tutti gli
//   operatori sono aritmetici: stack di (f64, null) e output Float64 diretto;
// - generale: stack di `Slot` con testo preso in prestito (`Cow`) dalle
//   colonne Utf8 e dai letterali, stessa logica di output del generico.
// Semantica IDENTICA a `evaluate`: stessa propagazione dei null, stessa
// divisione per zero (`-0.0 == 0.0` incluso), nessun controllo di finitezza,
// stessi errori nello stesso ordine di valutazione (postfix = ordine
// sinistra-destra del generico; una colonna mancante errore al suo op, non
// in compilazione). Ricade sul percorso generico quando una colonna non e'
// Int64/Float64/Utf8 o quando il batch e' vuoto (colonne mai risolte dal
// generico su zero righe).
// ---------------------------------------------------------------------------

/// Accessore di colonna pre-risolto (indice + downcast fatti una volta).
#[derive(Clone, Copy)]
enum FastColumn<'a> {
    F64(&'a Float64Array),
    I64(&'a Int64Array),
    Str(&'a StringArray),
}

impl FastColumn<'_> {
    const fn is_numeric(&self) -> bool {
        matches!(self, Self::F64(_) | Self::I64(_))
    }
}

#[derive(Clone, Copy)]
enum FastOp<'a> {
    Number(f64),
    Text(&'a str),
    Column(FastColumn<'a>),
    /// Colonna assente: errore rilasciato quando l'op viene eseguito, nello
    /// stesso punto in cui il generico lo rilascerebbe a riga 0.
    MissingColumn(&'a str),
    Neg,
    Add,
    Subtract,
    Multiply,
    Divide,
}

struct FastProgram<'a> {
    ops: Vec<FastOp<'a>>,
    /// Profondita' massima dello stack, calcolata in compilazione.
    depth: usize,
    /// Vero se ogni valore prodotto e' certamente Number|Null.
    numeric: bool,
}

impl<'a> FastProgram<'a> {
    /// Compila l'AST; `None` se una colonna non e' Int64/Float64/Utf8
    /// (il chiamante ricade sul percorso generico).
    fn compile(expression: &'a Expr, batch: &'a RecordBatch) -> Option<Self> {
        let mut program = Self {
            ops: Vec::new(),
            depth: 0,
            numeric: true,
        };
        let mut depth = 0_usize;
        let numeric = program.emit(expression, batch, &mut depth)?;
        program.numeric = numeric;
        Some(program)
    }

    fn push(&mut self, op: FastOp<'a>, depth: &mut usize, delta: isize) {
        if delta > 0 {
            *depth += 1;
            self.depth = self.depth.max(*depth);
        } else if delta < 0 {
            *depth -= 1;
        }
        self.ops.push(op);
    }

    /// Emette gli op in postfix (ordine di valutazione del generico);
    /// restituisce `None` per il fallback, `Some(pure)` con `pure` vero se il
    /// sotto-albero produce certamente solo Number|Null.
    fn emit(
        &mut self,
        expression: &'a Expr,
        batch: &'a RecordBatch,
        depth: &mut usize,
    ) -> Option<bool> {
        match expression {
            Expr::Number(value) => {
                self.push(FastOp::Number(*value), depth, 1);
                Some(true)
            }
            Expr::Text(value) => {
                self.push(FastOp::Text(value.as_str()), depth, 1);
                Some(false)
            }
            Expr::Column(name) => {
                let Ok(index) = column_index(batch, name) else {
                    // Colonna assente: errore rilasciato quando l'op viene
                    // eseguito, nello stesso punto del generico a riga 0.
                    self.push(FastOp::MissingColumn(name.as_str()), depth, 1);
                    return Some(false);
                };
                let column = batch.column(index);
                let fast = match column.data_type() {
                    DataType::Float64 => column
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .map(FastColumn::F64),
                    DataType::Int64 => column
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .map(FastColumn::I64),
                    DataType::Utf8 => column
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .map(FastColumn::Str),
                    _ => None,
                }?;
                let numeric = fast.is_numeric();
                self.push(FastOp::Column(fast), depth, 1);
                Some(numeric)
            }
            Expr::Neg(inner) => {
                let numeric = self.emit(inner, batch, depth)?;
                self.push(FastOp::Neg, depth, 0);
                Some(numeric)
            }
            Expr::Binary(left, operator, right) => {
                let left_numeric = self.emit(left, batch, depth)?;
                let right_numeric = self.emit(right, batch, depth)?;
                let op = match operator {
                    '+' => FastOp::Add,
                    '-' => FastOp::Subtract,
                    '*' => FastOp::Multiply,
                    '/' => FastOp::Divide,
                    _ => return None,
                };
                self.push(op, depth, -1);
                Some(left_numeric && right_numeric)
            }
        }
    }

    fn run(&self, batch: &RecordBatch, config: &Formula) -> Result<RecordBatch> {
        if self.numeric {
            self.run_numeric(batch, config)
        } else {
            self.run_slots(batch, config)
        }
    }

    /// Tier numerico: stack di `(valore, null)`, nessuna allocazione per
    /// riga; gli unici errori possibili sono colonna mancante (mai in un
    /// programma `numeric`) e divisione per zero.
    fn run_numeric(&self, batch: &RecordBatch, config: &Formula) -> Result<RecordBatch> {
        let mut stack: Vec<(f64, bool)> = Vec::with_capacity(self.depth);
        let mut output = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            stack.clear();
            for op in &self.ops {
                match *op {
                    FastOp::Number(value) => stack.push((value, false)),
                    FastOp::Column(column) => match column {
                        FastColumn::F64(values) => {
                            stack.push((values.value(row), values.is_null(row)));
                        }
                        FastColumn::I64(values) => {
                            stack.push((values.value(row).to_f64().unwrap_or_default(), values.is_null(row)));
                        }
                        FastColumn::Str(_) => {
                            return Err(PlenoraError::Contract(
                                "internal error: programma numerico senza testo".into(),
                            ));
                        }
                    },
                    FastOp::Neg => {
                        let (value, null) = stack.pop().unwrap_or_default();
                        stack.push((-value, null));
                    }
                    FastOp::Add | FastOp::Subtract | FastOp::Multiply | FastOp::Divide => {
                        let (right, right_null) = stack.pop().unwrap_or_default();
                        let (left, left_null) = stack.pop().unwrap_or_default();
                        if left_null || right_null {
                            stack.push((0.0, true));
                            continue;
                        }
                        let value = match op {
                            FastOp::Add => left + right,
                            FastOp::Subtract => left - right,
                            FastOp::Multiply => left * right,
                            FastOp::Divide if right == 0.0 => {
                                return Err(PlenoraError::Schema("divisione per zero".into()));
                            }
                            FastOp::Divide => left / right,
                            _ => {
                                return Err(PlenoraError::Contract(
                                    "internal error: operatore non aritmetico nel ramo aritmetico"
                                        .into(),
                                ));
                            }
                        };
                        stack.push((value, false));
                    }
                    FastOp::Text(_) | FastOp::MissingColumn(_) => {
                        return Err(PlenoraError::Contract(
                            "internal error: programma numerico senza testo".into(),
                        ));
                    }
                }
            }
            let (value, null) = stack.pop().unwrap_or_default();
            output.push(if null { None } else { Some(value) });
        }
        replace_or_append(
            batch,
            &config.new_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(output)),
        )
    }

    /// Tier generale: stack di `Slot` con testo in prestito; replica
    /// `evaluate` + logica di output del generico.
    fn run_slots(&self, batch: &RecordBatch, config: &Formula) -> Result<RecordBatch> {
        let mut stack: Vec<Slot<'a>> = Vec::with_capacity(self.depth);
        let mut values: Vec<Slot<'a>> = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            stack.clear();
            for op in &self.ops {
                match *op {
                    FastOp::Number(value) => stack.push(Slot::Number(value)),
                    FastOp::Text(value) => stack.push(Slot::Text(Cow::Borrowed(value))),
                    FastOp::Column(column) => stack.push(column.slot(row)),
                    FastOp::MissingColumn(name) => {
                        return Err(PlenoraError::Schema(format!(
                            "colonna non trovata: {name}"
                        )));
                    }
                    FastOp::Neg => {
                        let value = stack.pop().unwrap_or(Slot::Null);
                        stack.push(match value {
                            Slot::Number(value) => Slot::Number(-value),
                            Slot::Null => Slot::Null,
                            Slot::Text(_) => {
                                return Err(PlenoraError::Schema("negazione di testo".into()));
                            }
                        });
                    }
                    FastOp::Add | FastOp::Subtract | FastOp::Multiply | FastOp::Divide => {
                        let right = stack.pop().unwrap_or(Slot::Null);
                        let left = stack.pop().unwrap_or(Slot::Null);
                        stack.push(binary_slot(*op, left, right)?);
                    }
                }
            }
            values.push(stack.pop().unwrap_or(Slot::Null));
        }
        if values
            .iter()
            .all(|value| matches!(value, Slot::Number(_) | Slot::Null))
        {
            let values = values
                .into_iter()
                .map(|value| match value {
                    Slot::Number(value) => Some(value),
                    _ => None,
                })
                .collect::<Vec<_>>();
            replace_or_append(
                batch,
                &config.new_column,
                DataType::Float64,
                true,
                Arc::new(Float64Array::from(values)),
            )
        } else {
            let values = values
                .into_iter()
                .map(|value| match value {
                    Slot::Null => None,
                    other => Some(display_slot(other)),
                })
                .collect::<Vec<_>>();
            replace_or_append(
                batch,
                &config.new_column,
                DataType::Utf8,
                true,
                Arc::new(StringArray::from(values)),
            )
        }
    }
}

/// Valore di lavoro del tier generale: come `Evaluated`, ma il testo delle
/// colonne Utf8 e dei letterali e' preso in prestito (nessun clone per riga).
enum Slot<'a> {
    Null,
    Number(f64),
    Text(Cow<'a, str>),
}

impl<'a> FastColumn<'a> {
    fn slot(&self, row: usize) -> Slot<'a> {
        match self {
            Self::F64(values) => {
                if values.is_null(row) {
                    Slot::Null
                } else {
                    Slot::Number(values.value(row))
                }
            }
            Self::I64(values) => {
                if values.is_null(row) {
                    Slot::Null
                } else {
                    Slot::Number(values.value(row).to_f64().unwrap_or_default())
                }
            }
            Self::Str(values) => {
                if values.is_null(row) {
                    Slot::Null
                } else {
                    Slot::Text(Cow::Borrowed(values.value(row)))
                }
            }
        }
    }
}

/// Equivalente di `display` su `Slot`.
fn display_slot(value: Slot<'_>) -> String {
    match value {
        Slot::Null => String::new(),
        Slot::Number(value) => value.to_string(),
        Slot::Text(value) => value.into_owned(),
    }
}

/// Equivalente del ramo `Expr::Binary` di `evaluate` su `Slot`.
fn binary_slot<'a>(op: FastOp<'a>, left: Slot<'a>, right: Slot<'a>) -> Result<Slot<'a>> {
    Ok(match (left, right) {
        (Slot::Null, _) | (_, Slot::Null) => Slot::Null,
        (Slot::Number(left), Slot::Number(right)) => match op {
            FastOp::Add => Slot::Number(left + right),
            FastOp::Subtract => Slot::Number(left - right),
            FastOp::Multiply => Slot::Number(left * right),
            FastOp::Divide if right == 0.0 => {
                return Err(PlenoraError::Schema("divisione per zero".into()));
            }
            FastOp::Divide => Slot::Number(left / right),
            _ => {
                return Err(PlenoraError::Contract(
                    "internal error: operatore non aritmetico su operandi numerici".into(),
                ));
            }
        },
        (left, right) if matches!(op, FastOp::Add) => Slot::Text(Cow::Owned(format!(
            "{}{}",
            display_slot(left),
            display_slot(right)
        ))),
        _ => {
            return Err(PlenoraError::Schema(
                "operatore formula incompatibile con testo".into(),
            ));
        }
    })
}

pub fn validate(config: &Formula, max_bytes: usize) -> Result<()> {
    validate_output_name(&config.new_column)?;
    if config.formula.is_empty() || config.formula.len() > max_bytes {
        return Err(PlenoraError::Contract(
            "formula vuota o troppo grande".into(),
        ));
    }
    parse(&config.formula).map(|_| ())
}

pub fn formula(batch: &RecordBatch, config: &Formula) -> Result<RecordBatch> {
    let expression = parse(&config.formula)?;
    // Batch vuoto: il generico non risolve mai le colonne (zero valutazioni),
    // quindi anche una formula con colonne assenti ha successo; si mantiene
    // quel comportamento saltando la compilazione.
    if batch.num_rows() > 0 {
        if let Some(program) = FastProgram::compile(&expression, batch) {
            return program.run(batch, config);
        }
    }
    formula_generic(batch, config, &expression)
}

/// Percorso generico originale: interprete ricorsivo sull'AST, usato come
/// fallback per le colonne non Int64/Float64/Utf8 e come oracolo dei test.
fn formula_generic(batch: &RecordBatch, config: &Formula, expression: &Expr) -> Result<RecordBatch> {
    let values = (0..batch.num_rows())
        .map(|row| evaluate(expression, batch, row))
        .collect::<Result<Vec<_>>>()?;
    if values
        .iter()
        .all(|value| matches!(value, Evaluated::Number(_) | Evaluated::Null))
    {
        let values = values
            .into_iter()
            .map(|value| match value {
                Evaluated::Number(value) => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>();
        replace_or_append(
            batch,
            &config.new_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(values)),
        )
    } else {
        let values = values
            .into_iter()
            .map(|value| match value {
                Evaluated::Null => None,
                other => Some(display(other)),
            })
            .collect::<Vec<_>>();
        replace_or_append(
            batch,
            &config.new_column,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(values)),
        )
    }
}


// ---------------------------------------------------------------------------
// Fase 2A: analisi statica del tipo prodotto (analyze_contract).
//
// Aggiunta `pub(crate)` senza modifiche di comportamento al kernel: riusa il
// parser privato per classificare a secco il tipo della colonna derivata.
// Regole identiche a `evaluate`: colonne Int64/Float64 -> Number, ogni altro
// tipo scalare -> Text; `+` con un operando Text -> Text (concatenazione),
// `-` `*` `/` su Text -> errore certo a runtime (fail-closed).
// ---------------------------------------------------------------------------

/// Tipo statico della colonna prodotta da `formula`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormulaType {
    Number,
    Text,
}

/// Classifica il tipo prodotto senza leggere dati. `column_kind` decide il
/// tipo statico di una colonna referenziata (esistenza inclusa).
pub(crate) fn infer_formula_type(
    config: &Formula,
    column_kind: &dyn Fn(&str) -> Result<FormulaType>,
) -> Result<FormulaType> {
    let expression = parse(&config.formula)?;
    infer_expr_type(&expression, column_kind)
}

fn infer_expr_type(
    expression: &Expr,
    column_kind: &dyn Fn(&str) -> Result<FormulaType>,
) -> Result<FormulaType> {
    match expression {
        Expr::Number(_) => Ok(FormulaType::Number),
        Expr::Text(_) => Ok(FormulaType::Text),
        Expr::Column(name) => column_kind(name),
        Expr::Neg(inner) => match infer_expr_type(inner, column_kind)? {
            FormulaType::Number => Ok(FormulaType::Number),
            FormulaType::Text => Err(PlenoraError::Contract(
                "formula: negazione di testo".into(),
            )),
        },
        Expr::Binary(left, operator, right) => {
            let left = infer_expr_type(left, column_kind)?;
            let right = infer_expr_type(right, column_kind)?;
            match (left, operator, right) {
                (FormulaType::Number, '+', FormulaType::Number) => Ok(FormulaType::Number),
                (_, '+', _) => Ok(FormulaType::Text),
                (FormulaType::Number, '-' | '*' | '/', FormulaType::Number) => {
                    Ok(FormulaType::Number)
                }
                _ => Err(PlenoraError::Contract(
                    "formula: operatore numerico su testo".into(),
                )),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test-oracolo: fast path compilato vs interprete generico.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{BooleanArray, Float64Array, Int64Array, StringArray};
    use plenora_core::arrow::schema::{DataType, Field, Schema};

    use super::*;

    /// Fixture con null, -0.0, NaN, zero in coda e testi (anche vuoti).
    fn fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("f", DataType::Float64, true),
                Field::new("g", DataType::Float64, true),
                Field::new("i", DataType::Int64, true),
                Field::new("s", DataType::Utf8, true),
                Field::new("b", DataType::Boolean, true),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    None,
                    Some(-0.0),
                    Some(f64::NAN),
                    Some(2.75),
                    Some(-4.5),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(2.0),
                    Some(1.0),
                    Some(0.0),
                    Some(4.0),
                    Some(1.0),
                    Some(2.0),
                ])),
                Arc::new(Int64Array::from(vec![
                    Some(10),
                    None,
                    Some(0),
                    Some(-3),
                    Some(7),
                    Some(1),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    None,
                    Some("x"),
                    Some(""),
                    Some("ab"),
                    Some("z"),
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    Some(false),
                    None,
                    Some(true),
                    Some(false),
                    Some(true),
                ])),
            ],
        )
        .expect("fixture")
    }

    fn config(text: &str) -> Formula {
        Formula {
            new_column: "out".into(),
            formula: text.into(),
        }
    }

    /// Risultato del fast path (None se il programma non copre il caso).
    fn fast(batch: &RecordBatch, expression: &Expr, config: &Formula) -> Option<Result<RecordBatch>> {
        FastProgram::compile(expression, batch).map(|program| program.run(batch, config))
    }

    fn assert_equivalent(batch: &RecordBatch, text: &str) {
        let config = config(text);
        let expression = parse(text).expect("formula valida");
        let fast = fast(batch, &expression, &config).expect("caso coperto dal fast path");
        let generic = formula_generic(batch, &config, &expression);
        match (fast, generic) {
            (Ok(fast), Ok(generic)) => assert_eq!(fast, generic, "output diverso: {text}"),
            (fast, generic) => {
                assert_eq!(
                    fast.as_ref().map_err(ToString::to_string).map(|_| ()),
                    generic.as_ref().map_err(ToString::to_string).map(|_| ()),
                    "errore diverso: {text}"
                );
            }
        }
    }

    #[test]
    fn oracle_arithmetica_e_precedenze() {
        let batch = fixture();
        for text in [
            "f * 2 + i / 3 - 1",
            "f + i * 2",
            "(f + i) * 2",
            "-f",
            "- -f",
            "-(f + i)",
            "2 * (3 + 4)",
            "1 + 2 - 3 * 4 / 5",
            "f - g - i",
            "3.14",
            "i",
            "f / g",
            "f + -0.0",
            "0.1 + 0.2",
            "1e3 * f + 2E-2",
        ] {
            assert_equivalent(&batch, text);
        }
    }

    #[test]
    fn oracle_errori_di_dominio_identici() {
        let batch = fixture();
        // Divisione per zero (anche -0.0), a riga 0 e a riga successiva.
        for text in [
            "f / 0",
            "f / -0",
            "f / (f - f)",
            "i / (i * 0)",
            "s - s",
            "s * f",
            "-s",
            "missing + 1",
            "1 / 0 + missing",
            "missing + 1 / 0",
        ] {
            assert_equivalent(&batch, text);
        }
    }

    #[test]
    fn oracle_concatenazione_e_tipi_misti() {
        let batch = fixture();
        for text in [
            "s + '-' + s",
            "s + f",
            "f + s",
            "s + 'x'",
            "'pre' + s + f * 2",
            "s + ''",
            "(s + 'a') + (s + 'b')",
        ] {
            assert_equivalent(&batch, text);
        }
    }

    #[test]
    fn oracle_formula_lunga_e_null_heavy() {
        let batch = fixture();
        // Catena lunga (AST profondo a sinistra) e colonne tutte nulle.
        let long = (0..200)
            .map(|index| if index % 3 == 0 { "f".to_string() } else { "2".to_string() })
            .collect::<Vec<_>>()
            .join(" + ");
        assert_equivalent(&batch, &long);

        let all_null = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("f", DataType::Float64, true)])),
            vec![Arc::new(Float64Array::from(vec![None, None, None]))],
        )
        .expect("all null");
        assert_equivalent(&all_null, "f * 2 + 1");
    }

    #[test]
    fn fallback_e_batch_vuoto_preservati() {
        let batch = fixture();
        // Colonna Boolean: non coperta, il kernel ricade sul generico.
        let expression = parse("b + ''").expect("parse");
        assert!(fast(&batch, &expression, &config("b + ''")).is_none());
        let via_kernel = formula(&batch, &config("b + ''")).expect("fallback generico");
        let via_generic = formula_generic(&batch, &config("b + ''"), &expression).expect("generico");
        assert_eq!(via_kernel, via_generic);

        // Batch vuoto: il generico non risolve le colonne; la formula con
        // colonna assente ha comunque successo con output Float64 vuoto.
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("f", DataType::Float64, true)])),
            vec![Arc::new(Float64Array::from(Vec::<f64>::new()))],
        )
        .expect("empty");
        let output = formula(&empty, &config("missing + 1")).expect("zero righe: nessun errore");
        assert_eq!(output.num_rows(), 0);
        assert_eq!(output.schema().field_with_name("out").expect("out").data_type(), &DataType::Float64);
    }
}
