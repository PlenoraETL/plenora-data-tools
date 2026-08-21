use std::borrow::Cow;
use std::sync::Arc;

use plenora_core::arrow::array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use crate::{
    column_index, replace_or_append, scalar_as_f64_rounded, scalar_as_string, validate_output_name,
    DIVISION_BY_ZERO_MESSAGE,
};
use plenora_core::{PlenoraError, Result};

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
            return Err(PlenoraError::InvalidPlan("formula incompleta".into()));
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
                return Err(PlenoraError::InvalidPlan(
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
        Err(PlenoraError::InvalidPlan(format!(
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
                return Err(PlenoraError::InvalidPlan(
                    "escape nelle stringhe formula non supportato".into(),
                ));
            }
            self.position += 1;
        }
        if self.input.get(self.position) != Some(&quote) {
            return Err(PlenoraError::InvalidPlan(
                "stringa formula non terminata".into(),
            ));
        }
        let value = std::str::from_utf8(&self.input[start..self.position])
            .map_err(|_| PlenoraError::InvalidPlan("formula non UTF-8".into()))?
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
                return Err(PlenoraError::InvalidPlan(
                    "esponente formula non valido".into(),
                ));
            }
        }
        let value = std::str::from_utf8(&self.input[start..self.position])
            .ok()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| PlenoraError::InvalidPlan("numero formula non valido".into()))?;
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
                .map_err(|_| PlenoraError::InvalidPlan("identificatore non UTF-8".into()))?
                .to_owned(),
        ))
    }
}

/// Valore intero come double, con arrotondamento DICHIARATO (errori-e-limiti.md#limiti-dichiarati):
/// `formula` produce `Float64` per contratto e il tier generico applica la
/// stessa conversione, cosi' i due percorsi non divergono.
#[allow(clippy::cast_precision_loss)] // Arrotondamento voluto: vedi doc.
const fn integer_rounded(value: i64) -> f64 {
    value as f64
}

/// Vero se l'espressione e' un letterale numerico zero (anche negato,
/// `-0.0 == 0.0`): un divisore costante zero e' una proprieta' del piano,
/// non delle righe.
fn is_literal_zero(expr: &Expr) -> bool {
    match expr {
        Expr::Number(value) => *value == 0.0,
        Expr::Neg(inner) => is_literal_zero(inner),
        _ => false,
    }
}

/// P2 review 2026-08-03: divisione con divisore LETTERALE zero -> errore di
/// configurazione (`InvalidPlan`), mai un rifiuto row-scoped attribuito a
/// tutte le righe. Un divisore dipendente dalla riga (es. `i * 0`) resta
/// row-scoped.
fn reject_literal_zero_divisor(expr: &Expr) -> Result<()> {
    match expr {
        Expr::Binary(left, '/', right) if is_literal_zero(right) => {
            let _ = left;
            Err(PlenoraError::InvalidPlan(
                "divisione per zero letterale nella formula: errore di configurazione, non di riga"
                    .into(),
            ))
        }
        Expr::Binary(left, _, right) => {
            reject_literal_zero_divisor(left)?;
            reject_literal_zero_divisor(right)
        }
        Expr::Neg(inner) => reject_literal_zero_divisor(inner),
        Expr::Number(_) | Expr::Text(_) | Expr::Column(_) => Ok(()),
    }
}

fn parse(input: &str) -> Result<Expr> {
    let mut parser = Parser {
        input: input.as_bytes(),
        position: 0,
    };
    let expression = parser.expression()?;
    reject_literal_zero_divisor(&expression)?;
    parser.skip_space();
    if parser.position != parser.input.len() {
        return Err(PlenoraError::InvalidPlan(
            "token extra nella formula".into(),
        ));
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
            // Null LOGICO: senza, una cella dictionary nulla entrava nella
            // formula come stringa vuota invece che come null.
            if crate::is_logically_null(batch.column(index).as_ref(), row) {
                Evaluated::Null
            } else if matches!(
                batch.column(index).data_type(),
                DataType::Int64 | DataType::Float64
            ) {
                Evaluated::Number(
                    scalar_as_f64_rounded(batch.column(index).as_ref(), row)?.unwrap_or_default(),
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
                    return Err(PlenoraError::Schema(DIVISION_BY_ZERO_MESSAGE.into()))
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
// Int64/Float64/Utf8 o quando il batch e' vuoto (nessuna riga da
// compilare). Le colonne referenziate sono comunque risolte prima, per
// decidere il tipo di output dallo schema: un batch vuoto non e' piu' un
// caso permissivo.
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

    /// Emette gli op in postfix (ordine di valutazione del generico).
    ///
    /// Restituisce `None` per il fallback, `Some(pure)` con `pure` vero se il
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

    /// Come [`Self::run`] col tipo ricavato dallo schema (test-oracolo).
    #[cfg(test)]
    fn run_auto(
        &self,
        batch: &RecordBatch,
        config: &Formula,
        expression: &Expr,
    ) -> Result<RecordBatch> {
        let kind = infer_expr_type(expression, &|name| {
            let index = column_index(batch, name)?;
            column_formula_type(batch.schema_ref().field(index).data_type(), name)
        })?;
        self.run(batch, config, kind)
    }

    fn run(&self, batch: &RecordBatch, config: &Formula, kind: FormulaType) -> Result<RecordBatch> {
        if self.numeric {
            // Un programma `numeric` nasce solo da colonne Int64/Float64 e
            // letterali numerici: il tipo statico non puo' che essere Number.
            if kind != FormulaType::Number {
                return Err(PlenoraError::Internal(format!(
                    "formula: tier numerico con tipo statico {kind:?}"
                )));
            }
            self.run_numeric(batch, config)
        } else {
            self.run_slots(batch, config, kind)
        }
    }

    /// Tier numerico: stack di `(valore, null)`, nessuna allocazione per
    /// riga; gli unici errori possibili sono colonna mancante (mai in un
    /// programma `numeric`) e divisione per zero.
    fn run_numeric(&self, batch: &RecordBatch, config: &Formula) -> Result<RecordBatch> {
        let mut stack: Vec<(f64, bool)> = Vec::with_capacity(self.depth);
        let mut output = Vec::with_capacity(batch.num_rows());
        let mut rejections = Vec::new();
        for row in 0..batch.num_rows() {
            match self.eval_numeric_row(&mut stack, row) {
                Ok((value, null)) => output.push(if null { None } else { Some(value) }),
                Err(error) => {
                    let Some(cause) = crate::row_eval_failure_cause(&error) else {
                        return Err(error);
                    };
                    rejections.push(crate::RowRejection {
                        row,
                        cause,
                        column: None,
                    });
                    // Placeholder mai pubblicato: `reject_rows` chiude prima
                    // dell'uso di `output`.
                    output.push(None);
                }
            }
        }
        crate::reject_rows(
            &rejections,
            "valori formula rifiutati; consultare row_diagnostics",
        )?;
        replace_or_append(
            batch,
            &config.new_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(output)),
        )
    }

    /// Valutazione di una riga sul tier numerico (estratta per la raccolta
    /// row-scoped: semantica identica al loop originale).
    fn eval_numeric_row(&self, stack: &mut Vec<(f64, bool)>, row: usize) -> Result<(f64, bool)> {
        // Underflow dello stack: il programma e' costruito dal parser, che
        // garantisce l'arieta' di ogni operatore. Uno stack vuoto qui e'
        // quindi un difetto NOSTRO, e va segnalato: mascherarlo con
        // `unwrap_or_default()` trasformava un'invariante violata in uno zero
        // silenzioso, pubblicato come risultato della formula.
        let underflow = || PlenoraError::Internal("stack della formula in underflow".to_owned());
        stack.clear();
        for op in &self.ops {
            match *op {
                FastOp::Number(value) => stack.push((value, false)),
                FastOp::Column(column) => match column {
                    FastColumn::F64(values) => {
                        stack.push((values.value(row), values.is_null(row)));
                    }
                    FastColumn::I64(values) => {
                        // Nullo: il valore non viene letto, il posto sullo
                        // stack lo tiene lo zero. Non nullo: conversione
                        // esatta o errore, in parita' con `scalar_as_f64_rounded` del
                        // tier generico (che dal canto suo non arrotonda piu').
                        // Arrotondamento dichiarato (errori-e-limiti.md#limiti-dichiarati): `formula`
                        // produce `Float64` per contratto, come il tier
                        // generico (`scalar_as_f64_rounded`) — e i due
                        // percorsi devono dare la stessa risposta.
                        let value = if values.is_null(row) {
                            0.0
                        } else {
                            integer_rounded(values.value(row))
                        };
                        stack.push((value, values.is_null(row)));
                    }
                    FastColumn::Str(_) => {
                        return Err(PlenoraError::Internal(
                            "programma numerico senza testo".into(),
                        ));
                    }
                },
                FastOp::Neg => {
                    let (value, null) = stack.pop().ok_or_else(underflow)?;
                    stack.push((-value, null));
                }
                FastOp::Add | FastOp::Subtract | FastOp::Multiply | FastOp::Divide => {
                    let (right, right_null) = stack.pop().ok_or_else(underflow)?;
                    let (left, left_null) = stack.pop().ok_or_else(underflow)?;
                    if left_null || right_null {
                        stack.push((0.0, true));
                        continue;
                    }
                    let value = match op {
                        FastOp::Add => left + right,
                        FastOp::Subtract => left - right,
                        FastOp::Multiply => left * right,
                        FastOp::Divide if right == 0.0 => {
                            return Err(PlenoraError::Schema(DIVISION_BY_ZERO_MESSAGE.into()));
                        }
                        FastOp::Divide => left / right,
                        _ => {
                            return Err(PlenoraError::Internal(
                                "operatore non aritmetico nel ramo aritmetico".into(),
                            ));
                        }
                    };
                    stack.push((value, false));
                }
                FastOp::Text(_) | FastOp::MissingColumn(_) => {
                    return Err(PlenoraError::Internal(
                        "programma numerico senza testo".into(),
                    ));
                }
            }
        }
        // Lo stack deve contenere ESATTAMENTE il risultato: operandi
        // residui significano bytecode malformato o un difetto del
        // compilatore, e scartarli produrrebbe un risultato apparentemente
        // valido calcolato su una parte sola dell'espressione.
        let value = stack.pop().ok_or_else(underflow)?;
        if stack.is_empty() {
            Ok(value)
        } else {
            Err(PlenoraError::Internal(
                "stack della formula con operandi residui".to_owned(),
            ))
        }
    }

    /// Tier generale: stack di `Slot` con testo in prestito; replica
    /// `evaluate` + logica di output del generico.
    fn run_slots(
        &self,
        batch: &RecordBatch,
        config: &Formula,
        kind: FormulaType,
    ) -> Result<RecordBatch> {
        let mut stack: Vec<Slot<'a>> = Vec::with_capacity(self.depth);
        let mut values: Vec<Slot<'a>> = Vec::with_capacity(batch.num_rows());
        let mut rejections = Vec::new();
        for row in 0..batch.num_rows() {
            match self.eval_slot_row(&mut stack, row) {
                Ok(value) => values.push(value),
                Err(error) => {
                    let Some(cause) = crate::row_eval_failure_cause(&error) else {
                        return Err(error);
                    };
                    rejections.push(crate::RowRejection {
                        row,
                        cause,
                        column: None,
                    });
                    // Placeholder mai pubblicato: `reject_rows` chiude prima
                    // dell'uso di `values`.
                    values.push(Slot::Null);
                }
            }
        }
        crate::reject_rows(
            &rejections,
            "valori formula rifiutati; consultare row_diagnostics",
        )?;
        if kind == FormulaType::Number {
            let values = values
                .into_iter()
                .map(|value| match value {
                    Slot::Number(value) => Ok(Some(value)),
                    Slot::Null => Ok(None),
                    // L'inferenza e' derivata dalle stesse regole di
                    // `eval_slot_row`: se diverge, e' un'invariante nostra.
                    other @ Slot::Text(_) => Err(PlenoraError::Internal(format!(
                        "formula: tipo statico Number ma valore {}",
                        display_slot(other)
                    ))),
                })
                .collect::<Result<Vec<_>>>()?;
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
                    Slot::Null => Ok(None),
                    // `display_slot` su un `Text` e' esattamente questo.
                    Slot::Text(testo) => Ok(Some(testo.into_owned())),
                    // Simmetrica alla guardia del ramo Number: formattare qui
                    // un numero sarebbe una conversione silenziosa, cioe' il
                    // tipo che si adatta ai dati invece dei dati che
                    // rispettano il tipo.
                    Slot::Number(_) => Err(PlenoraError::Internal(
                        "formula: tipo statico Text ma valore Number".to_owned(),
                    )),
                })
                .collect::<Result<Vec<_>>>()?;
            replace_or_append(
                batch,
                &config.new_column,
                DataType::Utf8,
                true,
                Arc::new(StringArray::from(values)),
            )
        }
    }

    /// Valutazione di una riga sul tier generale (estratta per la raccolta
    /// row-scoped: semantica identica al loop originale).
    fn eval_slot_row<'b>(&self, stack: &mut Vec<Slot<'b>>, row: usize) -> Result<Slot<'b>>
    where
        'a: 'b,
    {
        // Vedi `eval_numeric_row`: un underflow qui e' un'invariante nostra
        // violata, non un dato mancante — `Slot::Null` lo avrebbe pubblicato
        // come un null legittimo.
        let underflow = || PlenoraError::Internal("stack della formula in underflow".to_owned());
        stack.clear();
        for op in &self.ops {
            match *op {
                FastOp::Number(value) => stack.push(Slot::Number(value)),
                FastOp::Text(value) => stack.push(Slot::Text(Cow::Borrowed(value))),
                FastOp::Column(column) => stack.push(column.slot(row)),
                FastOp::MissingColumn(name) => {
                    return Err(PlenoraError::Schema(format!("colonna non trovata: {name}")));
                }
                FastOp::Neg => {
                    let value = stack.pop().ok_or_else(underflow)?;
                    stack.push(match value {
                        Slot::Number(value) => Slot::Number(-value),
                        Slot::Null => Slot::Null,
                        Slot::Text(_) => {
                            return Err(PlenoraError::Schema("negazione di testo".into()));
                        }
                    });
                }
                FastOp::Add | FastOp::Subtract | FastOp::Multiply | FastOp::Divide => {
                    let right = stack.pop().ok_or_else(underflow)?;
                    let left = stack.pop().ok_or_else(underflow)?;
                    stack.push(binary_slot(*op, left, right)?);
                }
            }
        }
        let value = stack.pop().ok_or_else(underflow)?;
        if stack.is_empty() {
            Ok(value)
        } else {
            Err(PlenoraError::Internal(
                "stack della formula con operandi residui".to_owned(),
            ))
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
    /// Valore della cella come slot del tier generale.
    ///
    /// Infallibile: `formula` produce `Float64` per contratto e le
    /// conversioni sono arrotondate per dichiarazione (errori-e-limiti.md#limiti-dichiarati), come nel
    /// tier generico — non c'e' un caso di errore da propagare.
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
                    Slot::Number(integer_rounded(values.value(row)))
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
                return Err(PlenoraError::Schema(DIVISION_BY_ZERO_MESSAGE.into()));
            }
            FastOp::Divide => Slot::Number(left / right),
            _ => {
                return Err(PlenoraError::Internal(
                    "operatore non aritmetico su operandi numerici".into(),
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

/// Validazione statica della config: nome della nuova colonna e sintassi
/// della formula, senza toccare i dati.
///
/// # Errors
///
/// - `InvalidPlan`: nome colonna vuoto o oltre 1024 byte (come
///   `validate_output_name`); formula vuota o oltre `max_bytes`; errori di
///   sintassi (formula incompleta, parentesi non bilanciate, stringa non
///   terminata o con escape, numero o esponente non valido, carattere non
///   ammesso, token extra, testo non UTF-8).
pub fn validate(config: &Formula, max_bytes: usize) -> Result<()> {
    validate_output_name(&config.new_column)?;
    if config.formula.is_empty() || config.formula.len() > max_bytes {
        return Err(PlenoraError::InvalidPlan(
            "formula vuota o troppo grande".into(),
        ));
    }
    parse(&config.formula).map(|_| ())
}

/// Valuta la formula su ogni riga e appende/sostituisce `new_column`
/// (Float64 se tutti i valori sono numerici, Utf8 altrimenti).
///
/// Su batch con righe e colonne Int64/Float64/Utf8 usa il fast path
/// compilato (stessa semantica del generico, oracolo dei test); negli altri
/// casi valuta il percorso generico.
///
/// Il tipo della colonna prodotta e' deciso dallo SCHEMA prima di valutare
/// qualunque riga (stessa classificazione dell'analizzatore del contratto),
/// quindi non dipende dai valori: batch pieni, tutti null e vuoti con lo
/// stesso schema producono lo stesso tipo.
///
/// # Errors
///
/// - `InvalidPlan`: errori di sintassi della formula (come `validate`);
///   invarianti interne violate (errore Internal), fra cui un valore
///   calcolato di tipo diverso da quello statico — in ENTRAMBI i versi;
/// - `Schema`: colonna assente — anche su un batch VUOTO, perche' senza
///   risolverla il tipo di output non e' determinabile; tipo non convertibile
///   in testo (tipo o timezone); divisione per zero; negazione di testo;
///   operatore aritmetico su testo; valore non convertibile (via
///   `scalar_as_f64_rounded`/`scalar_as_string`); errore Arrow nella sostituzione.
pub fn formula(batch: &RecordBatch, config: &Formula) -> Result<RecordBatch> {
    let expression = parse(&config.formula)?;
    // Il tipo della colonna prodotta si decide dallo SCHEMA, MAI dai valori
    // osservati. Deciderlo dai valori significava che un batch vuoto o tutto
    // null produceva un tipo diverso dallo stesso piano su dati pieni — e
    // diverso da quello che l'analisi aveva dichiarato nel contratto.
    //
    // Conseguenza voluta: una formula che nomina una colonna assente ora
    // fallisce anche su un batch VUOTO. Senza risolvere le colonne non
    // esiste un tipo di output da dichiarare, quindi non esiste una risposta
    // giusta da dare.
    let kind = infer_expr_type(&expression, &|name| {
        let index = column_index(batch, name)?;
        column_formula_type(batch.schema_ref().field(index).data_type(), name)
    })?;
    if batch.num_rows() > 0 {
        if let Some(program) = FastProgram::compile(&expression, batch) {
            return program.run(batch, config, kind);
        }
    }
    formula_generic(batch, config, &expression, kind)
}

/// Come [`formula_generic`] col tipo ricavato dallo schema: usato dai
/// test-oracolo, che confrontano fast e generico sulla stessa config.
#[cfg(test)]
fn formula_generic_auto(
    batch: &RecordBatch,
    config: &Formula,
    expression: &Expr,
) -> Result<RecordBatch> {
    let kind = infer_expr_type(expression, &|name| {
        let index = column_index(batch, name)?;
        column_formula_type(batch.schema_ref().field(index).data_type(), name)
    })?;
    formula_generic(batch, config, expression, kind)
}

/// Percorso generico originale: interprete ricorsivo sull'AST, usato come
/// fallback per le colonne non Int64/Float64/Utf8 e come oracolo dei test.
///
/// `kind` e' il tipo gia' deciso dallo SCHEMA dal chiamante: qui non si
/// guardano i valori per sceglierlo, li si converte al tipo dichiarato e si
/// fallisce fail-closed se non corrispondono.
fn formula_generic(
    batch: &RecordBatch,
    config: &Formula,
    expression: &Expr,
    kind: FormulaType,
) -> Result<RecordBatch> {
    let mut values = Vec::with_capacity(batch.num_rows());
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        match evaluate(expression, batch, row) {
            Ok(value) => values.push(value),
            Err(error) => {
                let Some(cause) = crate::row_eval_failure_cause(&error) else {
                    return Err(error);
                };
                rejections.push(crate::RowRejection {
                    row,
                    cause,
                    column: None,
                });
                // Placeholder mai pubblicato: `reject_rows` chiude prima
                // dell'uso di `values`.
                values.push(Evaluated::Null);
            }
        }
    }
    crate::reject_rows(
        &rejections,
        "valori formula rifiutati; consultare row_diagnostics",
    )?;
    if kind == FormulaType::Number {
        let values = values
            .into_iter()
            .map(|value| match value {
                Evaluated::Number(value) => Ok(Some(value)),
                Evaluated::Null => Ok(None),
                other @ Evaluated::Text(_) => Err(PlenoraError::Internal(format!(
                    "formula: tipo statico Number ma valore {}",
                    display(other)
                ))),
            })
            .collect::<Result<Vec<_>>>()?;
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
                Evaluated::Null => Ok(None),
                // `display` su un `Text` e' esattamente questo.
                Evaluated::Text(testo) => Ok(Some(testo)),
                // Simmetrica alla guardia del ramo Number.
                Evaluated::Number(_) => Err(PlenoraError::Internal(
                    "formula: tipo statico Text ma valore Number".to_owned(),
                )),
            })
            .collect::<Result<Vec<_>>>()?;
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

impl FormulaType {
    /// Tipo Arrow della colonna prodotta.
    pub(crate) const fn data_type(self) -> DataType {
        match self {
            Self::Number => DataType::Float64,
            Self::Text => DataType::Utf8,
        }
    }
}

/// Tipo statico di una colonna referenziata da una formula.
///
/// Le stesse due righe di [`evaluate`]: numero solo per `Int64`/`Float64`,
/// tutto il resto letto via `scalar_as_string` — quindi profilo testuale
/// COMPLETO, timezone inclusa.
///
/// Vive qui, accanto al kernel che la applica, e la usa anche l'analizzatore
/// del contratto: una seconda copia sarebbe una copia che puo' divergere.
///
/// # Errors
///
/// `Schema`: il tipo non e' convertibile in testo (tipo o timezone).
pub(crate) fn column_formula_type(data_type: &DataType, name: &str) -> Result<FormulaType> {
    if matches!(data_type, DataType::Int64 | DataType::Float64) {
        Ok(FormulaType::Number)
    } else {
        crate::validate_text_convertible(data_type, name)?;
        Ok(FormulaType::Text)
    }
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
            FormulaType::Text => Err(PlenoraError::InvalidPlan(
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
                _ => Err(PlenoraError::InvalidPlan(
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

    /// La guardia sul tipo statico e' SIMMETRICA, in entrambi i percorsi.
    ///
    /// Con un AST inferito correttamente il caso non e' raggiungibile: e'
    /// proprio per questo che il test forza il tipo, invece di cercare una
    /// formula che lo produca. L'invariante che si sta fissando e' «il tipo
    /// dichiarato non si adatta ai dati», e vale nei due versi — prima il
    /// ramo Text formattava in silenzio un valore numerico, e una futura
    /// divergenza dell'inferenza sarebbe passata inosservata.
    #[test]
    fn la_guardia_sul_tipo_statico_e_simmetrica() {
        let batch = fixture();

        // Percorso generico, tipo statico Text su valori Number.
        let numerica = parse("f * 2").expect("parse");
        let errore = formula_generic(&batch, &config("f * 2"), &numerica, FormulaType::Text)
            .expect_err("Text dichiarato, valori Number");
        assert!(
            matches!(errore, PlenoraError::Internal(_)),
            "categoria: {errore}"
        );

        // Percorso generico, verso opposto.
        let testuale = parse("s + '!'").expect("parse");
        let errore = formula_generic(&batch, &config("s + '!'"), &testuale, FormulaType::Number)
            .expect_err("Number dichiarato, valori Text");
        assert!(
            matches!(errore, PlenoraError::Internal(_)),
            "categoria: {errore}"
        );

        // Fast path, tier generale (`run_slots`), entrambi i versi.
        let programma = FastProgram::compile(&testuale, &batch).expect("programma compilato");
        let errore = programma
            .run_slots(&batch, &config("s + '!'"), FormulaType::Number)
            .expect_err("Number dichiarato, valori Text");
        assert!(
            matches!(errore, PlenoraError::Internal(_)),
            "categoria: {errore}"
        );
        let programma = FastProgram::compile(&numerica, &batch).expect("programma compilato");
        let errore = programma
            .run_slots(&batch, &config("f * 2"), FormulaType::Text)
            .expect_err("Text dichiarato, valori Number");
        assert!(
            matches!(errore, PlenoraError::Internal(_)),
            "categoria: {errore}"
        );

        // Il tier numerico rifiuta un tipo statico che non sia Number.
        let errore = programma
            .run(&batch, &config("f * 2"), FormulaType::Text)
            .expect_err("tier numerico con tipo statico Text");
        assert!(
            matches!(errore, PlenoraError::Internal(_)),
            "categoria: {errore}"
        );

        // E col tipo GIUSTO i due percorsi continuano a coincidere.
        let via_generico =
            formula_generic(&batch, &config("f * 2"), &numerica, FormulaType::Number)
                .expect("generico");
        let via_fast = programma
            .run(&batch, &config("f * 2"), FormulaType::Number)
            .expect("fast");
        assert_eq!(via_generico, via_fast);
    }

    fn config(text: &str) -> Formula {
        Formula {
            new_column: "out".into(),
            formula: text.into(),
        }
    }

    /// Risultato del fast path (None se il programma non copre il caso).
    fn fast(
        batch: &RecordBatch,
        expression: &Expr,
        config: &Formula,
    ) -> Option<Result<RecordBatch>> {
        FastProgram::compile(expression, batch)
            .map(|program| program.run_auto(batch, config, expression))
    }

    fn assert_equivalent(batch: &RecordBatch, text: &str) {
        let config = config(text);
        let expression = parse(text).expect("formula valida");
        let fast = fast(batch, &expression, &config).expect("caso coperto dal fast path");
        let generic = formula_generic_auto(batch, &config, &expression);
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
    fn errore_non_classificabile_propaga_senza_diagnostica_inventata() {
        // P2 review 2026-08-03 (gap documentato): la classificazione delle
        // cause row-scoped e' per messaggio (costanti condivise). Un errore
        // di valutazione NON classificabile (qui sottrazione fra testi)
        // propaga fail-closed cosi' com'e': mai una causa inventata, mai un
        // report parziale. Una variante tipizzata richiederebbe un nuovo
        // discriminatore in `PlenoraError` (API pubblica, non non_exhaustive)
        // — rimandato per non rompere i consumatori.
        let batch = fixture();
        let error = formula(&batch, &config("s - s")).expect_err("sottrazione fra testi");
        assert!(
            error.row_diagnostics().is_none(),
            "errore non classificabile: nessuna diagnostica inventata"
        );
        // Controllo: il fast path ha lo stesso esito (parita' fail-closed).
        let expression = parse("s - s").expect("formula valida");
        let generic = formula_generic_auto(&batch, &config("s - s"), &expression)
            .expect_err("sottrazione fra testi (generico)");
        assert!(generic.row_diagnostics().is_none());
        assert_eq!(
            error.to_string(),
            generic.to_string(),
            "fast e generico devono propagare lo stesso errore grezzo"
        );
    }

    #[test]
    fn divisione_per_zero_letterale_e_errore_di_configurazione() {
        // P2 review 2026-08-03: un divisore LETTERALE zero (anche -0) e' una
        // proprieta' del piano, non delle righe: errore di configurazione
        // senza diagnostica row-scoped (mai un rifiuto attribuito a TUTTE
        // le righe). Il divisore calcolato (es. i * 0) resta row-scoped.
        let batch = fixture();
        for text in ["f / 0", "f / -0", "f / 0.0", "1 + f / (0)", "f / -(-0)"] {
            let error = formula(&batch, &config(text)).expect_err(text);
            assert!(
                matches!(error, PlenoraError::InvalidPlan(_)),
                "{text}: atteso InvalidPlan (config), trovato {error:?}"
            );
            assert!(
                error.row_diagnostics().is_none(),
                "{text}: nessuna diagnostica row-scoped per errore di configurazione"
            );
        }
        // Controllo: divisore dipendente dalla riga -> row-scoped invariato.
        let error = formula(&batch, &config("i / (i * 0)")).expect_err("divisione calcolata");
        assert!(error.row_diagnostics().is_some());
    }

    #[test]
    fn divisione_per_zero_riporta_diagnostica_row_scoped() {
        let batch = fixture();
        let cfg = config("i / (i * 0)");
        // Righe difettose: 0, 2, 3, 4, 5 (riga 1 null -> null, nessun errore).
        let error = formula(&batch, &cfg).expect_err("divisione per zero");
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
        let expression = parse("i / (i * 0)").expect("formula valida");
        let generic =
            formula_generic_auto(&batch, &cfg, &expression).expect_err("divisione per zero");
        assert_eq!(error.row_diagnostics(), generic.row_diagnostics());
    }

    #[test]
    fn oracle_errori_di_dominio_identici() {
        let batch = fixture();
        // Divisione per zero (anche -0.0), a riga 0 e a riga successiva.
        // I divisori LETTERALI zero ("f / 0" e simili) sono errori di
        // configurazione dal 2026-08-03 (P2 review): coperti dal test
        // dedicato, non da questo oracolo di errori di dominio per riga.
        for text in [
            "f / (f - f)",
            "i / (i * 0)",
            "s - s",
            "s * f",
            "-s",
            "missing + 1",
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
            .map(|index| {
                if index % 3 == 0 {
                    "f".to_string()
                } else {
                    "2".to_string()
                }
            })
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
        let via_generic =
            formula_generic_auto(&batch, &config("b + ''"), &expression).expect("generico");
        assert_eq!(via_kernel, via_generic);

        // Batch vuoto: il tipo di output si ricava dallo SCHEMA, quindi la
        // colonna va risolta anche senza righe. Prima una formula con una
        // colonna inesistente riusciva sul batch vuoto e falliva su quello
        // pieno.
        let empty = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("f", DataType::Float64, true)])),
            vec![Arc::new(Float64Array::from(Vec::<f64>::new()))],
        )
        .expect("empty");
        formula(&empty, &config("missing + 1"))
            .expect_err("colonna assente: il tipo non e' determinabile");
        let output = formula(&empty, &config("f + 1")).expect("zero righe");
        assert_eq!(output.num_rows(), 0);
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
    fn lo_stack_con_operandi_residui_e_un_errore_non_un_risultato() {
        // Bytecode malformato: due costanti e nessun operatore. Prendendo
        // solo la cima dello stack la formula avrebbe pubblicato `2` — un
        // valore plausibile calcolato su una parte sola dell'espressione, cioe'
        // esattamente il difetto che un componente fail-closed non deve
        // produrre. Il programma non e' costruibile dal parser: il test
        // protegge l'invariante contro un difetto futuro del compilatore.
        let program = FastProgram {
            ops: vec![FastOp::Number(1.0), FastOp::Number(2.0)],
            depth: 2,
            numeric: true,
        };
        let mut stack = Vec::with_capacity(program.depth);
        let error = program
            .eval_numeric_row(&mut stack, 0)
            .expect_err("operandi residui");
        assert!(error.to_string().contains("operandi residui"), "{error}");

        // Stesso controllo sul tier generico, che condivide l'invariante.
        let generic = FastProgram {
            ops: vec![FastOp::Number(1.0), FastOp::Number(2.0)],
            depth: 2,
            numeric: false,
        };
        let mut slots = Vec::with_capacity(generic.depth);
        // `Slot` non e' `Debug` (contiene prestiti sul batch): niente
        // `expect_err`, si ispeziona l'esito a mano.
        let Err(error) = generic.eval_slot_row(&mut slots, 0) else {
            panic!("il tier generico ha accettato operandi residui");
        };
        assert!(error.to_string().contains("operandi residui"), "{error}");
    }
}
