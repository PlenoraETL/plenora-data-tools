//! Tipo statico di un'espressione: l'insieme dei tipi che puo' produrre,
//! ricavato dal solo SCHEMA.
//!
//! Il tipo prodotto e' parte del contratto e non puo' dipendere dai valori:
//! risolto sui valori, un batch vuoto o tutto null ripiegherebbe su `Utf8`
//! anche dove l'analisi ha promesso `Boolean` o `Float64`. Questo modulo e'
//! la sorgente UNICA della regola, per l'analizzatore e per il kernel.

use plenora_core::arrow::schema::{DataType, TimeUnit};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::{BinaryOperator, Expression, Function, OutputType, UnaryOperator};

/// Un tipo che l'interprete puo' produrre (`scalar::Scalar` meno il null,
/// che non e' un tipo ma la sua assenza).
///
/// `Date32` e `TimestampMs` sono i tipi temporali NATIVI prodotti da
/// `date_trunc` (l'uscita di `date_trunc` non degrada a Number; le colonne
/// Date32/Timestamp lette direttamente restano `Number`, come nel kernel).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Numero, colonna `Float64`.
    Number,
    /// Booleano, colonna `Boolean`.
    Boolean,
    /// Testo, colonna `Utf8`.
    Text,
    /// Data, colonna `Date32` (solo da `date_trunc`).
    Date32,
    /// Istante, colonna `Timestamp(ms)` senza fuso (solo da `date_trunc`).
    TimestampMs,
}

impl Kind {
    const ALL: [Self; 5] = [
        Self::Number,
        Self::Boolean,
        Self::Text,
        Self::Date32,
        Self::TimestampMs,
    ];

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }

    /// Nome del tipo nei messaggi d'errore.
    #[must_use]
    pub const fn nome(self) -> &'static str {
        match self {
            Self::Number => "Number",
            Self::Boolean => "Boolean",
            Self::Text => "Text",
            Self::Date32 => "Date32",
            Self::TimestampMs => "TimestampMs",
        }
    }

    /// Tipo Arrow della colonna prodotta.
    #[must_use]
    pub const fn data_type(self) -> DataType {
        match self {
            Self::Number => DataType::Float64,
            Self::Boolean => DataType::Boolean,
            Self::Text => DataType::Utf8,
            Self::Date32 => DataType::Date32,
            // Timestamp timezone-aware rifiutati in ingresso da
            // `date_trunc`: l'output e' sempre Timestamp(ms) senza timezone.
            Self::TimestampMs => DataType::Timestamp(TimeUnit::Millisecond, None),
        }
    }
}

/// L'INSIEME dei tipi che un sotto-albero puo' produrre.
///
/// # Perche' un insieme e non un tipo
///
/// Un solo stato `Any` confonderebbe il letterale null (compatibile con
/// tutto) con un sotto-albero eterogeneo, e l'eterogeneita' sparirebbe al
/// passo successivo: `equal(coalesce(nullo, testo), numero)` passerebbe e a
/// runtime confronterebbe testo con numero. Con un insieme l'unione di
/// `coalesce`/`case` accumula e i confronti pretendono al piu' UN tipo.
///
/// L'insieme VUOTO non significa «sconosciuto»: significa **solo null**. E'
/// l'unico stato compatibile con qualunque richiesta, perche' il null
/// attraversa ogni conversione del runtime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TypeSet(u8);

impl TypeSet {
    /// Solo null: nessun tipo prodotto.
    pub const NULL_ONLY: Self = Self(0);

    const fn of(kind: Kind) -> Self {
        Self(kind.bit())
    }

    const fn con(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn is_null_only(self) -> bool {
        self.0 == 0
    }

    const fn contains(self, kind: Kind) -> bool {
        self.0 & kind.bit() != 0
    }

    const fn quanti(self) -> u32 {
        self.0.count_ones()
    }

    /// Il tipo, se e' uno solo. `None` per «solo null» e per gli insiemi
    /// eterogenei: sono due casi diversi, distinguibili con
    /// [`Self::is_null_only`].
    fn singolo(self) -> Option<Kind> {
        if self.quanti() == 1 {
            Kind::ALL.into_iter().find(|kind| self.contains(*kind))
        } else {
            None
        }
    }
}

impl std::fmt::Display for TypeSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_null_only() {
            return formatter.write_str("solo null");
        }
        let nomi: Vec<&str> = Kind::ALL
            .into_iter()
            .filter(|kind| self.contains(*kind))
            .map(Kind::nome)
            .collect();
        formatter.write_str(&nomi.join(" | "))
    }
}

fn errore<T>(op: &str, message: impl Into<String>) -> Result<T> {
    Err(PlenoraError::InvalidPlan(format!(
        "{op}: {}",
        message.into()
    )))
}

/// Unione dei tipi possibili: `coalesce` e `case` scelgono un ramo senza
/// confrontare niente, quindi il risultato puo' essere uno QUALSIASI dei
/// rami, deciso dai dati.
///
/// Non e' un errore e non e' un incontro: e' un accumulo. Perdere qui
/// l'informazione e' esattamente il difetto che questo tipo esiste per
/// evitare.
const fn unione(left: TypeSet, right: TypeSet) -> TypeSet {
    left.con(right)
}

/// Due sotto-alberi finiscono in `scalar::compare`, che rifiuta i tipi
/// incompatibili **a prescindere** da `output_type`.
///
/// La condizione e' sull'UNIONE: con piu' di un tipo possibile esiste una
/// combinazione di righe in cui `compare` riceve tipi diversi.
///
/// «Solo null» passa: `compare` con un null restituisce `None`, non un
/// errore.
fn require_comparable(op: &str, left: TypeSet, right: TypeSet) -> Result<TypeSet> {
    let insieme = unione(left, right);
    if insieme.quanti() <= 1 {
        return Ok(insieme);
    }
    errore(
        op,
        format!(
            "confronto fra tipi eterogenei ({insieme}): `compare` li rifiuta \
             qualunque sia output_type"
        ),
    )
}

/// L'operando deve essere di un tipo preciso.
///
/// Un insieme con piu' di un tipo NON soddisfa una richiesta di tipo singolo:
/// il runtime applicherebbe `boolean()`/`number()`/`text()` alle righe
/// dell'altro tipo e fallirebbe. «Solo null» invece passa sempre.
fn require_kind(op: &str, actual: TypeSet, expected: Kind, context: &str) -> Result<TypeSet> {
    if actual.is_null_only() || actual == TypeSet::of(expected) {
        Ok(TypeSet::of(expected))
    } else {
        errore(
            op,
            format!(
                "{context} richiede un operando {} (trovato {actual})",
                expected.nome()
            ),
        )
    }
}

/// Tipi che una COLONNA puo' produrre, con le stesse tre porte di
/// [`super::scalar::column`].
fn column_kind(op: &str, data_type: &DataType, name: &str) -> Result<TypeSet> {
    Ok(match data_type {
        DataType::Boolean => TypeSet::of(Kind::Boolean),
        DataType::Int64
        | DataType::UInt64
        | DataType::Float64
        | DataType::Decimal128(_, _)
        | DataType::Date32
        | DataType::Date64
        // Ogni unita', con o senza fuso: il numero e' il valore nativo
        // nell'unita' della colonna (`scalar_as_numero`), come in `column`.
        | DataType::Timestamp(_, _) => TypeSet::of(Kind::Number),
        data_type => {
            crate::validate_text_convertible(data_type, name)
                .map_err(|errore| PlenoraError::InvalidPlan(format!("{op}: {errore}")))?;
            TypeSet::of(Kind::Text)
        }
    })
}

/// Tipo di un letterale JSON scalare.
fn literal_kind(op: &str, value: &Value) -> Result<TypeSet> {
    match value {
        // Il null non e' un tipo: e' l'insieme vuoto.
        Value::Null => Ok(TypeSet::NULL_ONLY),
        Value::Bool(_) => Ok(TypeSet::of(Kind::Boolean)),
        Value::Number(_) => Ok(TypeSet::of(Kind::Number)),
        Value::String(_) => Ok(TypeSet::of(Kind::Text)),
        Value::Array(_) | Value::Object(_) => errore(op, "literal expression deve essere scalare"),
    }
}

/// I letterali che la valutazione rifiuterebbe: pattern letterale di
/// `regex_replace` non compilabile, indice letterale negativo di
/// `substring`.
///
/// Come la valutazione, che guarda il pattern solo con valore, pattern e
/// sostituzione non nulli, e l'indice solo con gli argomenti che lo
/// precedono non nulli: un argomento che puo' essere solo null
/// (`TypeSet::NULL_ONLY`) rende la chiamata null e il letterale non viene
/// mai guardato. Una regex o un indice calcolati dalle colonne dipendono dai
/// dati e restano un errore di valutazione.
///
/// # Errors
///
/// `InvalidPlan` con il prefisso `op`; gli errori di [`infer`] sugli
/// argomenti.
pub fn verifica_letterali(
    op: &str,
    expression: &Expression,
    lookup: &dyn Fn(&str) -> Result<DataType>,
) -> Result<()> {
    match expression {
        Expression::Column { .. } | Expression::Literal { .. } => Ok(()),
        Expression::Unary { value, .. } => verifica_letterali(op, value, lookup),
        Expression::Binary { left, right, .. } => {
            verifica_letterali(op, left, lookup)?;
            verifica_letterali(op, right, lookup)
        }
        Expression::Case {
            branches,
            else_value,
        } => {
            for branch in branches {
                verifica_letterali(op, &branch.when, lookup)?;
                verifica_letterali(op, &branch.then, lookup)?;
            }
            verifica_letterali(op, else_value, lookup)
        }
        Expression::Function { name, args } => {
            for arg in args {
                verifica_letterali(op, arg, lookup)?;
            }
            verifica_letterali_della_funzione(op, *name, args, lookup)
        }
    }
}

fn verifica_letterali_della_funzione(
    op: &str,
    name: Function,
    args: &[Expression],
    lookup: &dyn Fn(&str) -> Result<DataType>,
) -> Result<()> {
    let solo_null = |indice: usize| -> Result<bool> {
        args.get(indice)
            .map_or(Ok(true), |arg| Ok(infer(op, arg, lookup)?.is_null_only()))
    };
    let con_op = |errore: PlenoraError| match errore {
        PlenoraError::InvalidPlan(messaggio) => {
            PlenoraError::InvalidPlan(format!("{op}: {messaggio}"))
        }
        altro => altro,
    };
    match name {
        Function::RegexReplace if args.len() == 3 => {
            if let Expression::Literal {
                value: Value::String(pattern),
            } = &args[1]
            {
                if !solo_null(0)? && !solo_null(2)? {
                    regex::Regex::new(pattern).map_err(|error| {
                        PlenoraError::InvalidPlan(format!(
                            "{op}: regex_replace: {}",
                            crate::motivo_regex_non_valida(&error)
                        ))
                    })?;
                }
            }
        }
        Function::Substring if (2..=3).contains(&args.len()) => {
            // `start` si guarda con il valore non nullo, `len` con valore e
            // `start` non nulli.
            for (indice, contesto) in [(1, "substring: start"), (2, "substring: len")] {
                for prima in 0..indice {
                    if solo_null(prima)? {
                        return Ok(());
                    }
                }
                if let Some(Expression::Literal { value }) = args.get(indice) {
                    super::interpreter::substring_index(
                        &super::scalar::literal(value).map_err(con_op)?,
                        contesto,
                    )
                    .map_err(con_op)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Unita' del numero che una colonna temporale da' in un'espressione
/// ([`column_kind`]: il valore nativo dall'epoca); `None` per gli altri tipi.
const fn dominio_temporale(data_type: &DataType) -> Option<&'static str> {
    match data_type {
        DataType::Date32 => Some("giorni"),
        DataType::Date64 | DataType::Timestamp(TimeUnit::Millisecond, _) => Some("millisecondi"),
        DataType::Timestamp(TimeUnit::Second, _) => Some("secondi"),
        DataType::Timestamp(TimeUnit::Microsecond, _) => Some("microsecondi"),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => Some("nanosecondi"),
        _ => None,
    }
}

/// Le colonne temporali lette come numero in un'espressione hanno tutte la
/// stessa unita'.
///
/// Una colonna temporale vale il suo intero nativo: secondi, millisecondi,
/// microsecondi, nanosecondi o giorni dall'epoca. Due colonne di unita'
/// diverse nella stessa espressione darebbero confronti e differenze
/// sbagliati senza errore (`1` secondo non e' minore di `1000`
/// microsecondi), quindi l'espressione si rifiuta. La regola e'
/// conservativa: guarda le colonne, non gli operatori che le legano. La
/// sorgente di `date_trunc` non conta: il troncamento la porta ai
/// millisecondi e non la legge come numero.
///
/// La chiamano l'analisi e il kernel (`static_output_kind`), prima di
/// [`infer`]: stessa regola sullo stesso schema. Una colonna assente si
/// salta: la riporta [`infer`].
///
/// # Errors
///
/// `InvalidPlan` con il prefisso `op` e i nomi delle colonne (nessun
/// valore).
pub fn verifica_domini_temporali(
    op: &str,
    expression: &Expression,
    lookup: &dyn Fn(&str) -> Result<DataType>,
) -> Result<()> {
    let mut domini: std::collections::BTreeMap<&'static str, &str> =
        std::collections::BTreeMap::new();
    let mut da_visitare = vec![expression];
    while let Some(nodo) = da_visitare.pop() {
        match nodo {
            Expression::Column { name } => {
                if let Some(dominio) = lookup(name).ok().as_ref().and_then(dominio_temporale) {
                    domini.entry(dominio).or_insert(name.as_str());
                }
            }
            Expression::Function {
                name: Function::DateTrunc,
                ..
            } => {}
            altro => da_visitare.extend(super::figli(altro)),
        }
    }
    if domini.len() > 1 {
        let elenco = domini
            .iter()
            .map(|(dominio, colonna)| format!("{colonna} in {dominio}"))
            .collect::<Vec<_>>()
            .join(", ");
        return errore(
            op,
            format!(
                "colonne temporali di unita' diverse nella stessa espressione ({elenco}): \
                 il numero di una colonna temporale e' nella sua unita'; convertirle prima \
                 a un'unita' comune"
            ),
        );
    }
    Ok(())
}

/// Tipo statico di un'espressione.
///
/// `lookup` risolve il tipo Arrow di una colonna referenziata: e' il solo
/// punto in cui l'analizzatore (che ha un contratto) e il kernel (che ha un
/// batch) differiscono, e ciascuno conserva cosi' il proprio errore di
/// «colonna assente».
///
/// # Errors
///
/// `InvalidPlan` con il prefisso `op`: colonna assente o di tipo non
/// valutabile, arieta' errata, operando di tipo sbagliato, confronto fra tipi
/// eterogenei, letterale non scalare.
// Le regole di tipo dell'AST sono intrinsecamente ramificate; bracci
// identici di funzioni distinte restano separati, un caso per funzione.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
pub fn infer(
    op: &str,
    expression: &Expression,
    lookup: &dyn Fn(&str) -> Result<DataType>,
) -> Result<TypeSet> {
    match expression {
        Expression::Column { name } => column_kind(op, &lookup(name)?, name),
        Expression::Literal { value } => literal_kind(op, value),
        Expression::Unary {
            op: operator,
            value,
        } => {
            let operand = infer(op, value, lookup)?;
            match operator {
                UnaryOperator::Not => require_kind(op, operand, Kind::Boolean, "not"),
                UnaryOperator::Negate => require_kind(op, operand, Kind::Number, "negate"),
                // `is_null`/`is_not_null` guardano la presenza del valore,
                // non il tipo: qualunque operando va bene.
                UnaryOperator::IsNull | UnaryOperator::IsNotNull => Ok(TypeSet::of(Kind::Boolean)),
            }
        }
        Expression::Binary {
            op: operator,
            left,
            right,
        } => {
            let left = infer(op, left, lookup)?;
            let right = infer(op, right, lookup)?;
            match operator {
                BinaryOperator::Add
                | BinaryOperator::Subtract
                | BinaryOperator::Multiply
                | BinaryOperator::Divide => {
                    require_kind(op, left, Kind::Number, "operatore aritmetico")?;
                    require_kind(op, right, Kind::Number, "operatore aritmetico")
                }
                BinaryOperator::And | BinaryOperator::Or => {
                    require_kind(op, left, Kind::Boolean, "operatore logico")?;
                    require_kind(op, right, Kind::Boolean, "operatore logico")
                }
                BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual
                | BinaryOperator::Less
                | BinaryOperator::LessEqual => {
                    require_comparable(op, left, right)?;
                    Ok(TypeSet::of(Kind::Boolean))
                }
            }
        }
        Expression::Function { name, args } => infer_function(op, *name, args, lookup),
        Expression::Case {
            branches,
            else_value,
        } => {
            // Come `coalesce`: il risultato e' il ramo scelto dai dati,
            // quindi i tipi si ACCUMULANO.
            let mut possibili = infer(op, else_value, lookup)?;
            for branch in branches {
                // `interpreter::evaluate` valuta la condizione con
                // `boolean(..., "case when")`, che non converte.
                let when = infer(op, &branch.when, lookup)?;
                require_kind(op, when, Kind::Boolean, "case when")?;
                possibili = unione(possibili, infer(op, &branch.then, lookup)?);
            }
            Ok(possibili)
        }
    }
}

#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn infer_function(
    op: &str,
    name: Function,
    args: &[Expression],
    lookup: &dyn Fn(&str) -> Result<DataType>,
) -> Result<TypeSet> {
    // Nodi speciali: la lista di `in` non e' uno scalare valutabile e
    // `date_trunc` ha regole di tipo temporali native dedicate.
    if matches!(name, Function::DateTrunc) {
        return infer_date_trunc(op, args, lookup);
    }
    if matches!(name, Function::In) {
        return infer_in(op, args, lookup);
    }
    let types = args
        .iter()
        .map(|argument| infer(op, argument, lookup))
        .collect::<Result<Vec<_>>>()?;
    // `coalesce` sceglie il primo non nullo SENZA confrontare.
    let accumula = |types: &[TypeSet]| types.iter().copied().fold(TypeSet::NULL_ONLY, unione);
    // `null_if`, `greatest`, `least` e `between` passano da `compare`.
    let accumula_comparabile = |types: &[TypeSet]| {
        types.iter().try_fold(TypeSet::NULL_ONLY, |acc, item| {
            require_comparable(op, acc, *item)
        })
    };
    match name {
        Function::Coalesce => Ok(accumula(&types)),
        Function::NullIf => accumula_comparabile(&types),
        Function::Lower | Function::Upper | Function::Trim => {
            for item in &types {
                require_kind(op, *item, Kind::Text, "funzione testuale")?;
            }
            Ok(TypeSet::of(Kind::Text))
        }
        Function::Concat => {
            // `interpreter::function` chiama `text()` su OGNI argomento, e
            // `text()` non converte: un numero e' un errore, non una stringa.
            for item in &types {
                require_kind(op, *item, Kind::Text, "concat")?;
            }
            Ok(TypeSet::of(Kind::Text))
        }
        Function::Length => {
            for item in &types {
                require_kind(op, *item, Kind::Text, "length")?;
            }
            Ok(TypeSet::of(Kind::Number))
        }
        Function::Contains | Function::StartsWith | Function::EndsWith => {
            for item in &types {
                require_kind(op, *item, Kind::Text, "predicato testuale")?;
            }
            Ok(TypeSet::of(Kind::Boolean))
        }
        Function::Abs | Function::Round => {
            for item in &types {
                require_kind(op, *item, Kind::Number, "funzione numerica")?;
            }
            Ok(TypeSet::of(Kind::Number))
        }
        // `year` sta nel ramo TESTUALE dell'interprete
        // (lower/upper/trim/length/year): legge una stringa e ne fa il
        // parsing come `%Y-%m-%d`.
        Function::Year => {
            for item in &types {
                require_kind(op, *item, Kind::Text, "year")?;
            }
            Ok(TypeSet::of(Kind::Number))
        }
        Function::Substring => {
            // (testo, numero, numero?) -> testo
            if let Some((first, rest)) = types.split_first() {
                require_kind(op, *first, Kind::Text, "substring")?;
                for item in rest {
                    require_kind(op, *item, Kind::Number, "substring")?;
                }
            }
            Ok(TypeSet::of(Kind::Text))
        }
        Function::RegexReplace => {
            for item in &types {
                require_kind(op, *item, Kind::Text, "regex_replace")?;
            }
            Ok(TypeSet::of(Kind::Text))
        }
        Function::Between => {
            // Omogeneita' degli operandi come i confronti binari.
            accumula_comparabile(&types)?;
            Ok(TypeSet::of(Kind::Boolean))
        }
        Function::Greatest | Function::Least => accumula_comparabile(&types),
        Function::Floor | Function::Ceil | Function::Power => {
            for item in &types {
                require_kind(op, *item, Kind::Number, "funzione numerica")?;
            }
            Ok(TypeSet::of(Kind::Number))
        }
        Function::DateTrunc | Function::In => Err(PlenoraError::Internal(format!(
            "{op}: date_trunc/in hanno nodi dedicati"
        ))),
    }
}

/// `in_generic` confronta il valore con OGNI letterale della lista via
/// `compare`: una lista di tipo diverso dal valore fallisce su ogni riga non
/// nulla.
fn infer_in(
    op: &str,
    args: &[Expression],
    lookup: &dyn Fn(&str) -> Result<DataType>,
) -> Result<TypeSet> {
    if args.len() != 2 {
        return errore(op, "in richiede 2 argomenti");
    }
    let valore = infer(op, &args[0], lookup)?;
    if let Expression::Literal {
        value: Value::Array(items),
    } = &args[1]
    {
        for item in items {
            match item {
                Value::Array(_) | Value::Object(_) => {
                    return errore(op, "in: la lista ammette solo letterali scalari");
                }
                scalare => {
                    require_comparable(op, valore, literal_kind(op, scalare)?)?;
                }
            }
        }
    }
    Ok(TypeSet::of(Kind::Boolean))
}

/// Regole di tipo di `date_trunc` (tipi temporali nativi): l'unita' e' un
/// letterale del set chiuso; il tipo di output discende dal tipo della
/// colonna di input (anche su dati tutti null); nessun parsing implicito di
/// stringhe; timestamp timezone-aware rifiutati (semantica tz del
/// troncamento non definibile in modo sicuro: l'output e' sempre naive).
fn infer_date_trunc(
    op: &str,
    args: &[Expression],
    lookup: &dyn Fn(&str) -> Result<DataType>,
) -> Result<TypeSet> {
    if args.len() != 2 {
        return errore(op, "date_trunc richiede 2 argomenti");
    }
    let Expression::Literal {
        value: Value::String(unit),
    } = &args[0]
    else {
        return errore(op, "date_trunc: unit deve essere un letterale stringa");
    };
    if !matches!(
        unit.as_str(),
        "year" | "month" | "day" | "hour" | "minute" | "second"
    ) {
        return errore(op, super::temporal::MESSAGGIO_UNITA_TRUNC_NON_VALIDA);
    }
    temporal_kind(op, &args[1], unit, lookup)
}

/// Tipo temporale statico della sorgente di `date_trunc`; `unit` e' l'unita'
/// del livello corrente (sub-day rifiutata su Date32).
fn temporal_kind(
    op: &str,
    expression: &Expression,
    unit: &str,
    lookup: &dyn Fn(&str) -> Result<DataType>,
) -> Result<TypeSet> {
    match expression {
        Expression::Column { name } => match lookup(name)? {
            DataType::Date32 => {
                if matches!(unit, "hour" | "minute" | "second") {
                    return errore(op, "date_trunc: unita' sub-day non ammessa su Date32");
                }
                Ok(TypeSet::of(Kind::Date32))
            }
            // Ogni unita': il troncamento e' almeno al secondo, quindi
            // l'uscita in millisecondi e' esatta (`millisecondi_per_difetto`).
            DataType::Timestamp(_, timezone) => {
                if timezone.is_some() {
                    return errore(op, "date_trunc: timestamp timezone-aware non supportato");
                }
                Ok(TypeSet::of(Kind::TimestampMs))
            }
            other => errore(
                op,
                format!(
                    "date_trunc richiede una colonna Date32 o Timestamp, trovato {}",
                    plenora_core::tipo_arrow::descrivi_tipo(&other)
                ),
            ),
        },
        Expression::Function {
            name: Function::DateTrunc,
            args,
        } => {
            let kind = infer_date_trunc(op, args, lookup)?;
            if kind == TypeSet::of(Kind::Date32) && matches!(unit, "hour" | "minute" | "second") {
                return errore(op, "date_trunc: unita' sub-day non ammessa su Date32");
            }
            Ok(kind)
        }
        Expression::Literal { value: Value::Null } => Ok(TypeSet::NULL_ONLY),
        _ => errore(
            op,
            "date_trunc: il valore deve essere una colonna temporale",
        ),
    }
}

/// Il tipo della colonna prodotta, dallo SCHEMA soltanto.
///
/// Con `auto` l'insieme dev'essere un singoletto; «solo null» diventa `Text`,
/// anche per un batch pieno, perche' la decisione e' sullo SCHEMA.
///
/// Con un `output_type` dichiarato il kernel NON converte: accetta il proprio
/// tipo e il null e rifiuta il resto. La politica e' decisa qui:
///
/// - «solo null»: accettato, il runtime scrive null;
/// - il tipo dichiarato **appartiene** all'insieme: accettato. Se l'insieme
///   ne contiene altri, le righe di quegli altri tipi falliranno;
/// - il tipo dichiarato **non appartiene** all'insieme: rifiutato. Il runtime
///   non potrebbe produrlo su NESSUNA riga.
///
/// # Errors
///
/// `InvalidPlan`: insieme eterogeneo con `auto`, oppure tipo dichiarato fuori
/// dall'insieme dei tipi possibili.
pub fn resolve_output(op: &str, possibili: TypeSet, configured: OutputType) -> Result<Kind> {
    let atteso = match configured {
        OutputType::Auto => {
            if possibili.is_null_only() {
                return Ok(Kind::Text);
            }
            return possibili.singolo().map_or_else(
                || {
                    errore(
                        op,
                        format!(
                            "tipi eterogenei nell'espressione ({possibili}): dichiarare \
                             output_type esplicito"
                        ),
                    )
                },
                Ok,
            );
        }
        OutputType::Number => Kind::Number,
        OutputType::Boolean => Kind::Boolean,
        OutputType::Text => Kind::Text,
        OutputType::Date32 => Kind::Date32,
        OutputType::TimestampMs => Kind::TimestampMs,
    };
    if possibili.is_null_only() || possibili.contains(atteso) {
        return Ok(atteso);
    }
    errore(
        op,
        format!(
            "output_type dichiarato {} ma l'espressione produce {possibili}: \
             il runtime non converte fra i due",
            atteso.nome()
        ),
    )
}
