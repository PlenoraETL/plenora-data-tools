//! Operazioni di servizio: `table.add_row_number`, `table.date_extract`,
//! `table.limit`, `table.uuid_generator`.
//!
//! Semantica, schema, ordine ed errori per operazione: le schede
//! `docs/schede/<id>.md`, raccolte in `docs/operazioni.md`.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Datelike, NaiveDateTime, Timelike};
use plenora_core::arrow::array::{Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;
use uuid::Uuid;

use crate::dates::compile_items;
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

/// `start + posizione`, l'unica aritmetica dei numeri di riga, con e senza
/// partizione: `i64::MAX` si raggiunge, solo oltre e' un errore.
fn numero_di_riga(start: i64, position: usize) -> Result<i64> {
    i64::try_from(position)
        .ok()
        .and_then(|position| start.checked_add(position))
        .ok_or_else(|| PlenoraError::ResourceLimit("overflow row number".into()))
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
        // Per partizione si conta la posizione (da 0), e il numero e'
        // `start + posizione` come senza partizione: incrementare il numero
        // dopo averlo reso fallirebbe sull'ultimo rappresentabile
        // (`i64::MAX`), che invece e' un numero valido.
        let mut positions: HashMap<Option<String>, usize> = HashMap::new();
        (0..batch.num_rows())
            .map(|row| {
                let key = crate::scalar_key_string(source.as_ref(), row)?;
                let position = positions.entry(key).or_insert(0);
                let current = numero_di_riga(config.start, *position)?;
                *position += 1;
                Ok(Some(current))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        (0..batch.num_rows())
            .map(|row| numero_di_riga(config.start, row).map(Some))
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
    /// Colonna da leggere come data (obbligatorio): una colonna temporale
    /// (`Date32`, `Timestamp` di ogni unita', con o senza fuso) si legge dal
    /// valore nativo, ogni altra come testo.
    pub column: String,
    /// Parti da estrarre, nell'ordine delle colonne d'uscita (default
    /// `["year"]`).
    #[serde(default = "default_parts")]
    pub parts: Vec<DatePart>,
    /// Prefisso dei nomi d'uscita (default vuoto: `<column>_`).
    #[serde(default)]
    pub prefix: String,
    /// Formato strftime di chrono esplicito, provato come data e ora e poi
    /// come sola data. Se omesso si usano i soli formati ISO 8601 di
    /// default (`parse_datetime`). Legge un testo: con una colonna temporale
    /// si rifiuta.
    pub date_format: Option<String>,
    /// Non ammesso: un valore non interpretabile fa sempre fallire il passo,
    /// quindi nessuna politica avrebbe effetto. Scritto si rifiuta
    /// ([`crate::dates::verifica_politiche`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub invalid: Option<InvalidDatePolicy>,
}

/// Token di `invalid` delle operazioni sulle date.
///
/// Nessuno dei due cambia il risultato, quindi scritti si rifiutano
/// ([`crate::dates::verifica_politiche`]). Resta un tipo perche' il rifiuto
/// nomini il motivo invece di un campo sconosciuto.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidDatePolicy {
    /// `"null"`: il nome suggerisce un null per i valori non validi, ma il
    /// passo fallisce comunque.
    Null,
    /// `"error"`: il passo fallisce sui valori non validi.
    Error,
}

fn default_parts() -> Vec<DatePart> {
    vec![DatePart::Year]
}

impl DateExtract {
    /// Una parte ripetuta in `parts` scriverebbe due volte la stessa
    /// colonna, e la prima non avrebbe effetto; `parts` vuoto non produce
    /// nulla. Entrambi si rifiutano. La chiamano il kernel e l'analisi dei
    /// contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per `parts` vuoto o con una parte ripetuta.
    pub fn verifica_parti(&self) -> Result<()> {
        if self.parts.is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "parts vuoto: nessuna colonna prodotta".into(),
            ));
        }
        for (posizione, parte) in self.parts.iter().enumerate() {
            let discriminante = std::mem::discriminant(parte);
            if self.parts[..posizione]
                .iter()
                .any(|prima| std::mem::discriminant(prima) == discriminante)
            {
                return Err(PlenoraError::InvalidPlan(
                    "parte ripetuta in parts: la prima non avrebbe effetto".into(),
                ));
            }
        }
        Ok(())
    }
}

/// L'ora locale scritta in un testo: con `explicit_format` quel formato
/// (data e ora, o sola data a mezzanotte), altrimenti i soli formati ISO
/// 8601 (`crate::temporale::leggi_iso`: offset RFC 3339 ammesso, nessun
/// ordine giorno/mese da indovinare).
fn parse_datetime(value: &str, explicit_format: Option<&str>) -> Option<NaiveDateTime> {
    explicit_format
        .map_or_else(
            || crate::temporale::leggi_iso(value),
            |format| crate::temporale::leggi_con_items(value, &compile_items(format)),
        )
        .map(|momento| momento.locale)
}

/// I nomi d'uscita di `date_extract`, uno per parte: validi e distinti (una
/// parte ripetuta si rifiuta, come ogni coppia di colonne prodotte
/// insieme). La chiamano il kernel e l'analisi.
///
/// # Errors
///
/// `InvalidPlan` per un nome non valido o ripetuto.
pub fn nomi_date_extract(config: &DateExtract) -> Result<Vec<String>> {
    let prefix = if config.prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.prefix.clone()
    };
    let nomi = config
        .parts
        .iter()
        .map(|part| {
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
            format!("{prefix}{suffix}")
        })
        .collect::<Vec<_>>();
    for nome in &nomi {
        validate_output_name(nome)?;
    }
    crate::verifica_nomi_distinti("date_extract", nomi.iter().map(String::as_str))?;
    Ok(nomi)
}

/// `date_format` legge un testo: con una colonna temporale non ha effetto,
/// e si rifiuta invece di essere ignorato. La chiamano il kernel e
/// l'analisi.
///
/// # Errors
///
/// `InvalidPlan` se `date_format` accompagna una colonna temporale.
pub fn verifica_date_extract_temporale(config: &DateExtract) -> Result<()> {
    if config.date_format.is_some() {
        return Err(PlenoraError::InvalidPlan(
            "date_extract: date_format legge un testo e non si applica a una colonna temporale"
                .into(),
        ));
    }
    Ok(())
}

/// Estrae le parti di data e ora richieste in colonne `Int64`
/// `<prefix><parte>` (`table.date_extract`).
///
/// Una colonna temporale (`Date32`, `Timestamp` di ogni unita', con o senza
/// fuso) si legge dal valore nativo: le parti sono quelle dell'ora locale
/// della colonna (del suo fuso; senza fuso, il valore com'e'). Ogni altra
/// cella non nulla si legge come testo ([`scalar_as_string`]) e si
/// interpreta con `date_format` se dato, altrimenti con i soli formati ISO
/// 8601 (un offset si legge, e le parti sono dell'ora scritta). Ogni valore
/// non interpretabile rifiuta l'uscita con la diagnostica per riga;
/// `invalid` scritto si rifiuta ([`crate::dates::verifica_politiche`]).
///
/// # Errors
///
/// - `InvalidPlan`: `date_format` con un elemento non riconosciuto, o con
///   una colonna temporale; `parts` vuoto o con una parte ripetuta
///   ([`DateExtract::verifica_parti`]); nome di colonna d'uscita non valido
///   o ripetuto; `invalid` scritto;
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
    crate::dates::verifica_politiche(config.invalid.as_ref(), None)?;
    config.verifica_parti()?;
    if let Some(format) = &config.date_format {
        crate::dates::validate_format_items(format, "date_format")?;
    }
    let nomi = nomi_date_extract(config)?;
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    if let Some(temporale) = crate::temporale::ColonnaTemporale::new(source)? {
        verifica_date_extract_temporale(config)?;
        let parsed = (0..batch.num_rows())
            .map(|row| Ok(temporale.momento(row)?.map(|momento| momento.locale)))
            .collect::<Result<Vec<_>>>()?;
        return scrivi_parti(batch, config, &nomi, &parsed);
    }
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
            let mut parsed = Vec::with_capacity(column.len());
            for row in 0..column.len() {
                if column.is_null(row) {
                    parsed.push(None);
                    continue;
                }
                let value = column.value(row);
                let parsed_value = match &explicit_items {
                    Some(items) => crate::temporale::leggi_con_items(value, items)
                        .map(|momento| momento.locale),
                    None => crate::temporale::leggi_iso(value).map(|momento| momento.locale),
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
    scrivi_parti(batch, config, &nomi, &parsed)
}

/// Le colonne delle parti, una per nome d'uscita.
fn scrivi_parti(
    batch: &RecordBatch,
    config: &DateExtract,
    nomi: &[String],
    parsed: &[Option<NaiveDateTime>],
) -> Result<RecordBatch> {
    let mut result = batch.clone();
    for (part, name) in config.parts.iter().zip(nomi) {
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
            name,
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
    /// `max_rows`). Con `n = 0` l'uscita e' vuota comunque: un `offset`
    /// positivo non avrebbe effetto e si rifiuta
    /// ([`Limit::verifica_parametri`]).
    #[serde(default)]
    pub offset: u64,
}

impl Limit {
    /// `offset` positivo con `n = 0` non ha effetto (l'uscita e' vuota
    /// comunque): si rifiuta. La chiamano il kernel e l'analisi dei
    /// contratti.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` se `offset > 0` e `n = 0`.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.n == 0 && self.offset > 0 {
            return Err(PlenoraError::InvalidPlan(
                "offset senza effetto con n 0: l'uscita e' vuota comunque".into(),
            ));
        }
        Ok(())
    }
}

/// Prime `n` righe dopo `offset` (`table.limit`).
///
/// Schema e ordine invariati; l'uscita e' una finestra sull'ingresso
/// (nessuna copia). Nessuno stato fra chiamate: chiamato su un blocco di
/// righe, limita quel blocco. Il runner passa la tabella intera.
///
/// # Errors
///
/// - `InvalidPlan`: `offset` senza effetto ([`Limit::verifica_parametri`]);
/// - `ResourceLimit`: numero di righe non rappresentabile come `u64`,
///   `offset` o `n` non rappresentabili come `usize`.
pub fn limit(batch: &RecordBatch, config: &Limit) -> Result<RecordBatch> {
    config.verifica_parametri()?;
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
                invalid: None,
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
            Some("2019-12-30 23:59:59"),
            Some("1969-12-31"),
            // L'offset si legge; le parti sono dell'ora scritta.
            Some("2024-02-29T00:00:00+05:00"),
        ]);
        let output = date_extract(
            &batch,
            &DateExtract {
                column: "ts".into(),
                parts: all_parts(),
                prefix: "p_".into(),
                date_format: None,
                invalid: None,
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
        {
            let invalid = None;
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
        {
            let invalid = None;
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
        // Tutti i formati ISO del parser di default, solo valori validi.
        let valid = utf8_batch(vec![
            Some("2024-01-15T10:30:00"),
            Some("2024-01-15 10:30:00"),
            Some("2024-01-15T10:30:00.250"),
            Some("2024-01-15T10:30:00Z"),
            Some("2024-01-15 10:30:00+01:00"),
            Some("2024-01-15"),
            Some("1970-01-01"),
            Some("1969-12-31 23:59:59"),
            Some("2000-02-29"),
            None,
        ]);
        // Valori che nessun formato di default legge, intercalati: 1, 2, 3,
        // 5, 6 e 7. Giorno e mese in un ordine da indovinare non sono un
        // default.
        let with_invalid = utf8_batch(vec![
            Some("2024-01-15"),
            Some("2023-02-29"), // inesistente
            Some("2024-13-01"), // mese 13
            Some("non una data"),
            None,
            Some(""),
            Some("15/01/2024"),
            Some("2024/01/15"),
        ]);
        {
            let invalid = None;
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
            assert_rejected_rows(date_extract(&with_invalid, &config), &[1, 2, 3, 5, 6, 7]);
        }
    }

    /// Regressione: una colonna `Timestamp` si legge dal valore nativo (il
    /// suo testo RFC 3339 con `+00:00` falliva con i formati di default, e
    /// un `Timestamp` in microsecondi non si leggeva). Le parti sono dell'ora
    /// locale della colonna; `date_format` con una colonna temporale si
    /// rifiuta, e una parte ripetuta anche.
    #[test]
    fn date_extract_legge_le_colonne_temporali_senza_testo() {
        use plenora_core::arrow::array::{ArrayRef, Date32Array, TimestampMicrosecondArray};
        let micro = 1_706_696_430_123_456_i64; // 2024-01-31T10:20:30.123456Z
        let config = |date_format: Option<&str>, parts: Vec<DatePart>| DateExtract {
            column: "ts".into(),
            parts,
            prefix: "p_".into(),
            date_format: date_format.map(ToOwned::to_owned),
            invalid: None,
        };
        let richieste = vec![
            DatePart::Year,
            DatePart::Day,
            DatePart::Hour,
            DatePart::Second,
        ];
        for (colonna, ora) in [
            (
                Arc::new(TimestampMicrosecondArray::from(vec![Some(micro), None])) as ArrayRef,
                10,
            ),
            (
                Arc::new(
                    TimestampMicrosecondArray::from(vec![Some(micro), None])
                        .with_timezone("Europe/Rome"),
                ),
                11,
            ),
        ] {
            let batch =
                single_column_batch("ts", colonna.clone(), colonna.data_type().clone(), true);
            let uscita = date_extract(&batch, &config(None, richieste.clone())).expect("nativo");
            let valori_parte = |nome: &str| {
                uscita
                    .column_by_name(nome)
                    .expect("valori_parte")
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .expect("int64")
                    .iter()
                    .collect::<Vec<_>>()
            };
            assert_eq!(valori_parte("p_year"), vec![Some(2024), None]);
            assert_eq!(valori_parte("p_day"), vec![Some(31), None]);
            assert_eq!(valori_parte("p_hour"), vec![Some(ora), None]);
            assert_eq!(valori_parte("p_second"), vec![Some(30), None]);
            assert!(matches!(
                date_extract(&batch, &config(Some("%Y-%m-%d"), richieste.clone())),
                Err(PlenoraError::InvalidPlan(_))
            ));
        }
        let giorni: ArrayRef = Arc::new(Date32Array::from(vec![19_753]));
        let batch = single_column_batch("ts", giorni, DataType::Date32, true);
        let uscita = date_extract(&batch, &config(None, vec![DatePart::Month])).expect("date32");
        assert_eq!(
            uscita
                .column_by_name("p_month")
                .expect("mese")
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("int64")
                .value(0),
            1
        );
        assert!(matches!(
            date_extract(
                &batch,
                &config(None, vec![DatePart::Month, DatePart::Month])
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }

    /// Regressione: con e senza `partition_column` la numerazione raggiunge
    /// esattamente `i64::MAX` e fallisce solo oltre. Con la partizione il
    /// contatore veniva incrementato prima di rendere il numero, e l'ultimo
    /// rappresentabile falliva.
    #[test]
    fn add_row_number_arriva_a_i64_max_con_e_senza_partizione() {
        let batch = single_column_batch(
            "p",
            Arc::new(StringArray::from(vec![Some("a"), Some("b"), Some("a")])),
            DataType::Utf8,
            true,
        );
        let numeri = |config: &AddRowNumber| -> Result<Vec<i64>> {
            let uscita = add_row_number(&batch, config)?;
            let colonna = uscita
                .column(1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("Int64")
                .clone();
            Ok(colonna.values().to_vec())
        };
        let config = |start: i64, partition: Option<&str>| AddRowNumber {
            output_column: "n".into(),
            start,
            partition_column: partition.map(ToOwned::to_owned),
            order_column: None,
            ascending: None,
        };
        // Partizione: "a" ha due righe, "b" una.
        assert_eq!(
            numeri(&config(i64::MAX - 1, Some("p"))).expect("i64::MAX si raggiunge"),
            vec![i64::MAX - 1, i64::MAX - 1, i64::MAX]
        );
        assert!(matches!(
            numeri(&config(i64::MAX, Some("p"))),
            Err(PlenoraError::ResourceLimit(_))
        ));
        // Senza partizione: tre righe, l'ultima e' i64::MAX.
        assert_eq!(
            numeri(&config(i64::MAX - 2, None)).expect("i64::MAX si raggiunge"),
            vec![i64::MAX - 2, i64::MAX - 1, i64::MAX]
        );
        assert!(matches!(
            numeri(&config(i64::MAX - 1, None)),
            Err(PlenoraError::ResourceLimit(_))
        ));
    }
}
