use std::collections::HashMap;
use std::sync::Arc;

use chrono::format::{Item, Parsed};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use plenora_core::arrow::array::{Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;
use uuid::Uuid;

use crate::dates::{compile_items, parse_with_items};
use crate::{
    column_index, reject_rows, replace_or_append, scalar_as_string, validate_output_name,
    RowRejection,
};
use plenora_core::{PlenoraError, Result};

/// Config di `table.add_row_number`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddRowNumber {
    /// Colonna d'uscita (default `"row_number"`); se esiste si sostituisce.
    #[serde(default = "default_row_name")]
    pub output_column: String,
    /// Numero della prima riga di ogni partizione (default 1).
    #[serde(default = "default_start")]
    pub start: i64,
    /// Colonna le cui celle con lo stesso testo formano una partizione
    /// (i null sono una partizione); assente: una numerazione sola.
    pub partition_column: Option<String>,
    /// Non supportato: se scritto (non nullo) si rifiuta. Per numerare
    /// secondo un ordine serve un `table.sort` prima.
    pub order_column: Option<String>,
    /// Verso di `order_column`. `order_column` si rifiuta, quindi un verso
    /// dichiarato non avrebbe effetto: si rifiuta ([`verifica_ascending`])
    /// invece di essere ignorato.
    #[serde(default)]
    pub ascending: Option<bool>,
}

fn default_row_name() -> String {
    "row_number".into()
}
const fn default_start() -> i64 {
    1
}
/// `ascending` ha senso solo con `order_column`, che il profilo corrente
/// rifiuta: un verso dichiarato sarebbe ignorato in silenzio. La chiamano il
/// kernel e l'analisi dei contratti.
///
/// # Errors
///
/// `InvalidPlan` se `ascending` e' dichiarato senza `order_column`.
pub fn verifica_ascending(config: &AddRowNumber) -> Result<()> {
    if config.ascending.is_some() && config.order_column.is_none() {
        return Err(PlenoraError::InvalidPlan(
            "ascending senza order_column non ha effetto".into(),
        ));
    }
    Ok(())
}

/// Colonna `Int64` non nullable con il numero progressivo di ogni riga
/// nell'ordine d'ingresso, a partire da `config.start`
/// (`table.add_row_number`).
///
/// Con `partition_column` il conteggio riparte per ogni partizione (chiave =
/// testo della cella, [`scalar_as_string`]; tutti i null sono una
/// partizione); l'ordinamento non e' gestito da questo kernel.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna d'uscita non valido, `order_column`
///   valorizzato, `ascending` dichiarato;
/// - `ResourceLimit`: numero oltre `i64::MAX`;
/// - `Schema`: `partition_column` assente dal batch, o cella di partizione
///   che non si legge come testo;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
pub fn add_row_number(batch: &RecordBatch, config: &AddRowNumber) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    if config.order_column.is_some() {
        return Err(PlenoraError::InvalidPlan("add_row_number con ordinamento verra' eseguito dal kernel blocking sort; profilo corrente richiede order_column nullo".into()));
    }
    verifica_ascending(config)?;
    let values = if let Some(partition) = &config.partition_column {
        let index = column_index(batch, partition)?;
        let source = batch.column(index);
        let mut counters: HashMap<Option<String>, i64> = HashMap::new();
        (0..batch.num_rows())
            .map(|row| {
                let key = scalar_as_string(source.as_ref(), row)?;
                let value = counters.entry(key).or_insert(config.start);
                let current = *value;
                *value = value
                    .checked_add(1)
                    .ok_or_else(|| PlenoraError::ResourceLimit("overflow row number".into()))?;
                Ok(Some(current))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        (0..batch.num_rows())
            .map(|row| {
                i64::try_from(row)
                    .ok()
                    .and_then(|row| config.start.checked_add(row))
                    .map(Some)
                    .ok_or_else(|| PlenoraError::ResourceLimit("overflow row number".into()))
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Int64,
        false,
        Arc::new(Int64Array::from(values)),
    )
}

/// Parte estratta da `table.date_extract`, in una colonna `Int64`
/// `<prefix><parte>`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatePart {
    /// Anno del calendario gregoriano (`"year"`).
    Year,
    /// Mese, 1-12 (`"month"`).
    Month,
    /// Giorno del mese, 1-31 (`"day"`).
    Day,
    /// Trimestre, 1-4 (`"quarter"`).
    Quarter,
    /// Giorno della settimana, 0 = lunedi' ... 6 = domenica (`"weekday"`).
    Weekday,
    /// Numero di settimana ISO 8601, 1-53 (`"week"`).
    Week,
    /// Ora, 0-23 (`"hour"`).
    Hour,
    /// Minuto, 0-59 (`"minute"`).
    Minute,
    /// Secondo (`"second"`).
    Second,
}

/// Config di `table.date_extract`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateExtract {
    /// Colonna da leggere come data, leggibile come testo (obbligatorio).
    pub column: String,
    /// Parti da estrarre, nell'ordine delle colonne d'uscita (default
    /// `["year"]`).
    #[serde(default = "default_parts")]
    pub parts: Vec<DatePart>,
    /// Prefisso dei nomi d'uscita (default vuoto: `<column>_`).
    #[serde(default)]
    pub prefix: String,
    /// Formato strftime di chrono esplicito, provato come data e ora e poi
    /// come sola data. Se omesso si usano i formati di default, nell'ordine
    /// di `parse_datetime`.
    pub date_format: Option<String>,
    /// Accettato per compatibilita', senza effetto: un valore non
    /// interpretabile fa sempre fallire il passo (default `null`).
    #[serde(default = "default_invalid_date_policy")]
    pub invalid: InvalidDatePolicy,
}

/// Token di `DateExtract::invalid`; nessuno dei due cambia il risultato.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidDatePolicy {
    /// `"null"`: il nome suggerisce un null per i valori non validi, ma il
    /// passo fallisce comunque.
    Null,
    /// `"error"`: il passo fallisce sui valori non validi.
    Error,
}

const fn default_invalid_date_policy() -> InvalidDatePolicy {
    InvalidDatePolicy::Null
}

fn default_parts() -> Vec<DatePart> {
    vec![DatePart::Year]
}

fn parse_datetime(value: &str, explicit_format: Option<&str>) -> Option<NaiveDateTime> {
    if let Some(format) = explicit_format {
        return NaiveDateTime::parse_from_str(value, format)
            .ok()
            .or_else(|| {
                NaiveDate::parse_from_str(value, format)
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
            });
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%d/%m/%Y %H:%M:%S",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed);
        }
    }
    for format in ["%Y-%m-%d", "%d/%m/%Y", "%d-%m-%Y", "%Y/%m/%d"] {
        if let Ok(parsed) = NaiveDate::parse_from_str(value, format) {
            return parsed.and_hms_opt(0, 0, 0);
        }
    }
    None
}

// Fast path `date_extract`:
// formati chrono compilati una volta e loop sui `&str` nativi della colonna
// Utf8; per gli altri tipi Arrow si ricade sul percorso generico.

/// Item precompilati dei formati datetime del parser multi-formato di
/// default (stesso ordine di `parse_datetime`).
fn default_datetime_items() -> [Vec<Item<'static>>; 3] {
    [
        compile_items("%Y-%m-%dT%H:%M:%S"),
        compile_items("%Y-%m-%d %H:%M:%S"),
        compile_items("%d/%m/%Y %H:%M:%S"),
    ]
}

/// Item precompilati dei formati date-only del parser multi-formato di
/// default (stesso ordine di `parse_datetime`).
fn default_date_items() -> [Vec<Item<'static>>; 4] {
    [
        compile_items("%Y-%m-%d"),
        compile_items("%d/%m/%Y"),
        compile_items("%d-%m-%Y"),
        compile_items("%Y/%m/%d"),
    ]
}

/// Parser multi-formato di default con item precompilati, semantica identica
/// a `parse_datetime(value, None)`: prima i formati datetime, poi quelli
/// date-only (mezzanotte).
fn parse_datetime_default(
    value: &str,
    datetime_items: &[Vec<Item<'static>>],
    date_items: &[Vec<Item<'static>>],
) -> Option<NaiveDateTime> {
    for items in datetime_items {
        let mut parsed = Parsed::new();
        if chrono::format::parse(&mut parsed, value, items.iter()).is_ok() {
            // Stessa risoluzione di `NaiveDateTime::parse_from_str`.
            if let Ok(datetime) = parsed.to_naive_datetime_with_offset(0) {
                return Some(datetime);
            }
        }
    }
    for items in date_items {
        let mut parsed = Parsed::new();
        if chrono::format::parse(&mut parsed, value, items.iter()).is_ok() {
            if let Ok(date) = parsed.to_naive_date() {
                return date.and_hms_opt(0, 0, 0);
            }
        }
    }
    None
}

/// Estrae le parti di data e ora richieste in colonne `Int64`
/// `<prefix><parte>` (`table.date_extract`).
///
/// Ogni cella non nulla si legge come testo ([`scalar_as_string`]) e si
/// interpreta con `date_format` se dato, altrimenti con i formati di
/// default. Il token `config.invalid` resta deserializzabile per
/// compatibilita', ma ogni valore non interpretabile rifiuta l'uscita con la
/// diagnostica per riga.
///
/// # Errors
///
/// - `InvalidPlan`: `date_format` con un elemento non riconosciuto; nome di
///   colonna d'uscita non valido;
/// - `DataMapping`: almeno un valore non interpretabile come data (causa
///   `conversion.invalid_datetime`, con la diagnostica per riga); errore
///   Arrow nella costruzione del batch (guardia interna, non attesa);
/// - `Schema`: colonna assente dal batch, o cella che non si legge come
///   testo;
/// - `Internal`: guardie interne (parser compilato, controllo preventivo
///   incoerente).
// Sequenza lineare, parsing poi estrazione: lunga per costruzione. I due
// bracci del parser sono blocchi completi, troppo grandi per `map_or_else`.
#[allow(clippy::too_many_lines, clippy::option_if_let_else)]
pub fn date_extract(batch: &RecordBatch, config: &DateExtract) -> Result<RecordBatch> {
    if let Some(format) = &config.date_format {
        crate::dates::validate_format_items(format, "date_format")?;
    }
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        if scalar_as_string(source.as_ref(), row)?
            .is_some_and(|value| parse_datetime(&value, config.date_format.as_deref()).is_none())
        {
            rejections.push(RowRejection {
                row,
                cause: "conversion.invalid_datetime",
                column: Some(&config.column),
            });
        }
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
    let parsed: Vec<Option<NaiveDateTime>> =
        if let Some(column) = source.as_any().downcast_ref::<StringArray>() {
            let explicit_items = config.date_format.as_deref().map(compile_items);
            let default_items = if explicit_items.is_none() {
                Some((default_datetime_items(), default_date_items()))
            } else {
                None
            };
            let mut parsed = Vec::with_capacity(column.len());
            for row in 0..column.len() {
                if column.is_null(row) {
                    parsed.push(None);
                    continue;
                }
                let value = column.value(row);
                let parsed_value = match (&explicit_items, &default_items) {
                    (Some(items), None) => parse_with_items(value, items),
                    (None, Some((datetime_items, date_items))) => {
                        parse_datetime_default(value, datetime_items, date_items)
                    }
                    _ => {
                        return Err(PlenoraError::Internal("un solo parser compilato".into()));
                    }
                };
                match parsed_value {
                    Some(value) => parsed.push(Some(value)),
                    None => {
                        return Err(PlenoraError::Internal(
                            "prevalidazione row-scoped incoerente in date_extract".into(),
                        ))
                    }
                }
            }
            parsed
        } else {
            (0..batch.num_rows())
                .map(|row| {
                    let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                        return Ok(None);
                    };
                    match parse_datetime(&value, config.date_format.as_deref()) {
                        Some(parsed) => Ok(Some(parsed)),
                        None => Err(PlenoraError::Internal(
                            "prevalidazione row-scoped incoerente in date_extract".into(),
                        )),
                    }
                })
                .collect::<Result<Vec<_>>>()?
        };
    let prefix = if config.prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.prefix.clone()
    };
    let mut result = batch.clone();
    for part in &config.parts {
        let suffix = match part {
            DatePart::Year => "year",
            DatePart::Month => "month",
            DatePart::Day => "day",
            DatePart::Quarter => "quarter",
            DatePart::Weekday => "weekday",
            DatePart::Week => "week",
            DatePart::Hour => "hour",
            DatePart::Minute => "minute",
            DatePart::Second => "second",
        };
        let name = format!("{prefix}{suffix}");
        validate_output_name(&name)?;
        let values = parsed
            .iter()
            .map(|value| {
                value.map(|value| match part {
                    DatePart::Year => i64::from(value.year()),
                    DatePart::Month => i64::from(value.month()),
                    DatePart::Day => i64::from(value.day()),
                    DatePart::Quarter => i64::from((value.month() - 1) / 3 + 1),
                    DatePart::Weekday => i64::from(value.weekday().num_days_from_monday()),
                    DatePart::Week => i64::from(value.iso_week().week()),
                    DatePart::Hour => i64::from(value.hour()),
                    DatePart::Minute => i64::from(value.minute()),
                    DatePart::Second => i64::from(value.second()),
                })
            })
            .collect::<Vec<_>>();
        result = replace_or_append(
            &result,
            &name,
            DataType::Int64,
            true,
            Arc::new(Int64Array::from(values)),
        )?;
    }
    Ok(result)
}

/// Config di `table.limit`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    /// Righe da tenere al piu' (obbligatorio; l'analisi lo limita a
    /// `max_rows`).
    pub n: u64,
    /// Righe da saltare in testa (default 0; l'analisi lo limita a
    /// `max_rows`).
    #[serde(default)]
    pub offset: u64,
}

/// Prime `n` righe dopo `offset` (`table.limit`).
///
/// Schema e ordine invariati; l'uscita e' una finestra sull'ingresso
/// (nessuna copia). Nessuno stato fra chiamate: chiamato su un blocco di
/// righe, limita quel blocco. Il runner passa la tabella intera.
///
/// # Errors
///
/// - `ResourceLimit`: numero di righe non rappresentabile come `u64`,
///   `offset` o `n` non rappresentabili come `usize`.
pub fn limit(batch: &RecordBatch, config: &Limit) -> Result<RecordBatch> {
    let rows = u64::try_from(batch.num_rows())
        .map_err(|_| PlenoraError::ResourceLimit("limit: righe oltre u64".into()))?;
    let start = config.offset.min(rows);
    let count = config.n.min(rows - start);
    let start = usize::try_from(start)
        .map_err(|_| PlenoraError::ResourceLimit("limit: offset oltre usize".into()))?;
    let count = usize::try_from(count)
        .map_err(|_| PlenoraError::ResourceLimit("limit: n oltre usize".into()))?;
    if start == 0 && count == batch.num_rows() {
        // Finestra che copre l'intero batch: nessuna copia.
        return Ok(batch.clone());
    }
    Ok(batch.slice(start, count))
}

/// Config di `table.uuid_generator`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UuidGenerator {
    /// Colonna d'uscita (default `"uuid"`); se esiste si sostituisce.
    #[serde(default = "default_uuid_name")]
    pub output_column: String,
}
fn default_uuid_name() -> String {
    "uuid".into()
}

/// Colonna `Utf8` non nullable con un UUID v4 casuale per riga, in forma
/// minuscola con i trattini (`table.uuid_generator`). Non deterministica per
/// contratto.
///
/// # Errors
///
/// - `InvalidPlan`: nome della colonna d'uscita non valido;
/// - `DataMapping`: errore Arrow nella costruzione del batch (guardia
///   interna, non attesa).
pub fn uuid_generator(batch: &RecordBatch, config: &UuidGenerator) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    let values = (0..batch.num_rows())
        .map(|_| Uuid::new_v4().hyphenated().to_string())
        .collect::<Vec<_>>();
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        false,
        Arc::new(StringArray::from(values)),
    )
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::schema::{Field, Schema};
    use plenora_core::diagnostics::RowDiagnosticsCompleteness;
    use serde_json::json;

    use super::*;
    use crate::test_support::{assert_same_outcome as assert_equivalent, single_column_batch};

    /// Percorso generico, indipendente dal fast path: riferimento per
    /// l'equivalenza semantica (oracolo) del fast path di `date_extract`.
    /// Codifica la semantica corrente: ogni valore non parsabile rifiuta
    /// l'intero batch con diagnostica row-scoped, qualunque sia il token
    /// `invalid`; null resta null.
    ///
    /// Che cosa garantisce: che gli item precompilati e il loop Utf8 del fast
    /// path diano le stesse parti e gli stessi rifiuti del percorso per riga.
    /// Che cosa NON garantisce: condivide con la produzione `parse_datetime`
    /// (formati e loro ordine) e l'estrazione chrono delle parti; li coprono
    /// i valori scritti a mano di `date_extract_hand_written_parts`.
    fn generic_date_extract(batch: &RecordBatch, config: &DateExtract) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let source = batch.column(index);
        let mut rejections = Vec::new();
        let mut parsed = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                parsed.push(None);
                continue;
            };
            let value = parse_datetime(&value, config.date_format.as_deref());
            if value.is_none() {
                rejections.push(RowRejection {
                    row,
                    cause: "conversion.invalid_datetime",
                    column: Some(&config.column),
                });
            }
            parsed.push(value);
        }
        reject_rows(
            &rejections,
            "valori temporali rifiutati; consultare row_diagnostics",
        )?;
        let prefix = if config.prefix.is_empty() {
            format!("{}_", config.column)
        } else {
            config.prefix.clone()
        };
        let mut result = batch.clone();
        for part in &config.parts {
            let suffix = match part {
                DatePart::Year => "year",
                DatePart::Month => "month",
                DatePart::Day => "day",
                DatePart::Quarter => "quarter",
                DatePart::Weekday => "weekday",
                DatePart::Week => "week",
                DatePart::Hour => "hour",
                DatePart::Minute => "minute",
                DatePart::Second => "second",
            };
            let name = format!("{prefix}{suffix}");
            validate_output_name(&name)?;
            let values = parsed
                .iter()
                .map(|value| {
                    value.map(|value| match part {
                        DatePart::Year => i64::from(value.year()),
                        DatePart::Month => i64::from(value.month()),
                        DatePart::Day => i64::from(value.day()),
                        DatePart::Quarter => i64::from((value.month() - 1) / 3 + 1),
                        DatePart::Weekday => i64::from(value.weekday().num_days_from_monday()),
                        DatePart::Week => i64::from(value.iso_week().week()),
                        DatePart::Hour => i64::from(value.hour()),
                        DatePart::Minute => i64::from(value.minute()),
                        DatePart::Second => i64::from(value.second()),
                    })
                })
                .collect::<Vec<_>>();
            result = replace_or_append(
                &result,
                &name,
                DataType::Int64,
                true,
                Arc::new(Int64Array::from(values)),
            )?;
        }
        Ok(result)
    }

    fn all_parts() -> Vec<DatePart> {
        vec![
            DatePart::Year,
            DatePart::Month,
            DatePart::Day,
            DatePart::Quarter,
            DatePart::Weekday,
            DatePart::Week,
            DatePart::Hour,
            DatePart::Minute,
            DatePart::Second,
        ]
    }

    fn utf8_batch(values: Vec<Option<&str>>) -> RecordBatch {
        single_column_batch(
            "ts",
            Arc::new(StringArray::from(values)),
            DataType::Utf8,
            true,
        )
    }

    /// Equivalenza fast/generico dove entrambi devono riuscire: il confronto
    /// dei batch e' l'unico esito accettato.
    fn assert_same_output(fast: Result<RecordBatch>, generic: Result<RecordBatch>) {
        let fast = fast.expect("fast path rifiuta valori validi");
        let generic = generic.expect("oracolo generico rifiuta valori validi");
        assert_eq!(fast, generic);
    }

    /// Rifiuto row-scoped atteso: esattamente `rows`, causa
    /// `invalid_datetime` sulla colonna `ts`.
    fn assert_rejected_rows(result: Result<RecordBatch>, rows: &[u64]) {
        let error = result.expect_err("righe invalide accettate");
        let report = error
            .row_diagnostics()
            .expect("diagnostica row-scoped mancante");
        assert_eq!(report.completeness, RowDiagnosticsCompleteness::Complete);
        assert_eq!(
            report.observed_total,
            u64::try_from(rows.len()).expect("fixture")
        );
        assert_eq!(
            report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            rows
        );
        assert!(report.examples.iter().all(|example| {
            example.cause == "conversion.invalid_datetime"
                && example.column.as_deref() == Some("ts")
        }));
    }

    #[test]
    fn date_extract_rejects_every_invalid_source_row() {
        let batch = single_column_batch(
            "ts",
            Arc::new(StringArray::from(vec![
                Some("2024-01-15"),
                Some("2024-13-01"),
                None,
                Some("non una data"),
            ])),
            DataType::Utf8,
            true,
        );
        let error = date_extract(
            &batch,
            &DateExtract {
                column: "ts".into(),
                parts: vec![DatePart::Year],
                prefix: String::new(),
                date_format: Some("%Y-%m-%d".into()),
                invalid: InvalidDatePolicy::Null,
            },
        )
        .expect_err("date_extract ha pubblicato null sintetici");
        let report = error
            .row_diagnostics()
            .expect("diagnostica row-scoped mancante");
        assert_eq!(report.completeness, RowDiagnosticsCompleteness::Complete);
        assert_eq!(report.observed_total, 2);
        assert_eq!(report.total, Some(2));
        assert_eq!(
            report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(report.examples.iter().all(|example| {
            example.cause == "conversion.invalid_datetime"
                && example.column.as_deref() == Some("ts")
        }));
    }

    #[test]
    fn date_extract_hand_written_parts() {
        // Valori attesi scritti a mano. 2021-01-01 e' un venerdi' della
        // settimana ISO 53 del 2020; 2019-12-30 un lunedi' della settimana
        // ISO 1 del 2020; 1969-12-31 un mercoledi' della settimana ISO 1 del
        // 1970.
        let batch = utf8_batch(vec![
            Some("2021-01-01T06:07:08"),
            Some("30/12/2019 23:59:59"),
            Some("1969-12-31"),
            Some("2024/02/29"),
        ]);
        let output = date_extract(
            &batch,
            &DateExtract {
                column: "ts".into(),
                parts: all_parts(),
                prefix: "p_".into(),
                date_format: None,
                invalid: InvalidDatePolicy::Error,
            },
        )
        .expect("date valide");
        let part = |name: &str| {
            output
                .column_by_name(name)
                .expect("parte")
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("int64")
                .values()
                .to_vec()
        };
        assert_eq!(part("p_year"), vec![2021, 2019, 1969, 2024]);
        assert_eq!(part("p_month"), vec![1, 12, 12, 2]);
        assert_eq!(part("p_day"), vec![1, 30, 31, 29]);
        assert_eq!(part("p_quarter"), vec![1, 4, 4, 1]);
        // Lunedi' = 0.
        assert_eq!(part("p_weekday"), vec![4, 0, 2, 3]);
        assert_eq!(part("p_week"), vec![53, 1, 1, 9]);
        assert_eq!(part("p_hour"), vec![6, 23, 0, 0]);
        assert_eq!(part("p_minute"), vec![7, 59, 0, 0]);
        assert_eq!(part("p_second"), vec![8, 59, 0, 0]);
    }

    #[test]
    fn limit_slices_rows_and_preserves_schema() {
        let input = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2, 3, 4])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                    None,
                    Some("d"),
                    Some("e"),
                ])),
            ],
        )
        .expect("fixture");
        let ids = |batch: &RecordBatch| {
            batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("ids")
                .values()
                .to_vec()
        };
        // n semplice.
        let first = limit(&input, &Limit { n: 2, offset: 0 }).expect("limit");
        assert_eq!(ids(&first), vec![0, 1]);
        assert_eq!(first.schema(), input.schema());
        // offset + n.
        let window = limit(&input, &Limit { n: 2, offset: 1 }).expect("window");
        assert_eq!(ids(&window), vec![1, 2]);
        // null preservato nella finestra.
        assert!(window.column(1).is_null(1));
        // n = 0: batch vuoto con schema invariato.
        let empty = limit(&input, &Limit { n: 0, offset: 0 }).expect("empty");
        assert_eq!(empty.num_rows(), 0);
        assert_eq!(empty.schema(), input.schema());
        // n > righe: tutto il batch.
        let all = limit(&input, &Limit { n: 100, offset: 0 }).expect("all");
        assert_eq!(all.num_rows(), 5);
        // offset oltre le righe: vuoto; offset parziale: clampa.
        assert_eq!(
            limit(&input, &Limit { n: 3, offset: 10 })
                .expect("beyond")
                .num_rows(),
            0
        );
        assert_eq!(
            ids(&limit(&input, &Limit { n: 3, offset: 3 }).expect("tail")),
            vec![3, 4]
        );
        // Default serde: offset 0; config strict.
        let decoded: Limit = serde_json::from_value(json!({"n": 1})).expect("default offset");
        assert_eq!(decoded.offset, 0);
        assert!(serde_json::from_value::<Limit>(json!({"n": 1, "bogus": 1})).is_err());
    }

    #[test]
    fn date_extract_fast_path_matches_generic_with_explicit_format() {
        // Date limite valide: epoch, pre-1970, bisestile, confine ISO week.
        let valid = utf8_batch(vec![
            Some("1970-01-01 00:00:00"), // epoch, ISO week 1, giovedi'
            Some("1969-12-31 23:59:59"), // pre-epoch
            Some("2000-02-29 12:30:45"), // bisestile
            Some("2021-01-01 06:07:08"), // ISO week 53 del 2020
            Some("2019-12-30 23:59:59"), // ISO week 1 del 2020
            None,
        ]);
        // Righe non parsabili intercalate: 1, 2 e 4.
        let with_invalid = utf8_batch(vec![
            Some("1970-01-01 00:00:00"),
            Some("2023-02-29 00:00:00"), // inesistente
            Some("2024-02-29"),          // date-only: fallisce
            None,
            Some(""),
        ]);
        for invalid in [InvalidDatePolicy::Null, InvalidDatePolicy::Error] {
            let config = DateExtract {
                column: "ts".into(),
                parts: all_parts(),
                prefix: String::new(),
                date_format: Some("%Y-%m-%d %H:%M:%S".into()),
                invalid,
            };
            assert_same_output(
                date_extract(&valid, &config),
                generic_date_extract(&valid, &config),
            );
            assert_equivalent(
                date_extract(&with_invalid, &config),
                generic_date_extract(&with_invalid, &config),
            );
            assert_rejected_rows(date_extract(&with_invalid, &config), &[1, 2, 4]);
        }
        // Formato esplicito date-only (fallback `NaiveDate`).
        let dates_only = utf8_batch(vec![
            Some("1970-01-01"),
            Some("2000-02-29"),
            Some("2020-12-31"), // ISO week 53
            None,
        ]);
        let dates_only_invalid = utf8_batch(vec![
            Some("1970-01-01"),
            Some("2024-02-29 10:00:00"), // trailing input: fallisce
            Some("2023-02-29"),
        ]);
        for invalid in [InvalidDatePolicy::Null, InvalidDatePolicy::Error] {
            let config = DateExtract {
                column: "ts".into(),
                parts: all_parts(),
                prefix: "p_".into(),
                date_format: Some("%Y-%m-%d".into()),
                invalid,
            };
            assert_same_output(
                date_extract(&dates_only, &config),
                generic_date_extract(&dates_only, &config),
            );
            assert_equivalent(
                date_extract(&dates_only_invalid, &config),
                generic_date_extract(&dates_only_invalid, &config),
            );
            assert_rejected_rows(date_extract(&dates_only_invalid, &config), &[1, 2]);
        }
    }

    #[test]
    fn date_extract_fast_path_matches_generic_with_default_multi_format() {
        // Tutti i formati del parser di default, solo valori validi.
        let valid = utf8_batch(vec![
            Some("2024-01-15T10:30:00"),
            Some("2024-01-15 10:30:00"),
            Some("15/01/2024 10:30:00"),
            Some("2024-01-15"),
            Some("15/01/2024"),
            Some("15-01-2024"),
            Some("2024/01/15"),
            Some("1970-01-01"),
            Some("1969-12-31 23:59:59"),
            Some("29/02/2000"),
            None,
        ]);
        // Valori non parsabili da nessun formato, intercalati: 1, 2, 3 e 5.
        let with_invalid = utf8_batch(vec![
            Some("2024-01-15"),
            Some("29/02/2023"), // inesistente
            Some("2024-13-01"), // mese 13
            Some("non una data"),
            None,
            Some(""),
        ]);
        for invalid in [InvalidDatePolicy::Null, InvalidDatePolicy::Error] {
            let config = DateExtract {
                column: "ts".into(),
                parts: all_parts(),
                prefix: String::new(),
                date_format: None,
                invalid,
            };
            assert_same_output(
                date_extract(&valid, &config),
                generic_date_extract(&valid, &config),
            );
            assert_equivalent(
                date_extract(&with_invalid, &config),
                generic_date_extract(&with_invalid, &config),
            );
            assert_rejected_rows(date_extract(&with_invalid, &config), &[1, 2, 3, 5]);
        }
    }
}
