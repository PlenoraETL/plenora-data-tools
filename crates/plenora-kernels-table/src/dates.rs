use std::sync::Arc;

use chrono::format::{Item, Numeric, Parsed, StrftimeItems};
use chrono::{
    DateTime, Datelike, LocalResult, Months, NaiveDate, NaiveDateTime, TimeDelta, TimeZone,
};
use chrono_tz::Tz;
use plenora_core::arrow::array::{Array, Float64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use crate::utility::InvalidDatePolicy;
use crate::{column_index, reject_rows, replace_or_append, scalar_as_string, RowRejection};
use plenora_core::{PlenoraError, Result};

fn default_output_format() -> String {
    "%Y-%m-%d %H:%M:%S".into()
}

fn parse(value: &str, format: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, format)
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(value, format)
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
        })
}

// ---------------------------------------------------------------------------
// Fast path dei kernel data.
//
// Gli item strftime sono compilati una volta (`compile_items`) e il loop
// lavora sui `&str` nativi di una colonna Utf8; gli altri tipi Arrow
// ricadono sul percorso generico riga per riga, con lo stesso comportamento.
// ---------------------------------------------------------------------------

/// Item strftime precompilati di un formato chrono (prestazione, semantica
/// invariata: stessi item che chrono ri-parserizzerebbe a ogni riga).
pub(crate) fn compile_items(format: &str) -> Vec<Item<'_>> {
    StrftimeItems::new(format).collect()
}

/// Byte scritti al massimo da un valore formattato con `format`, per eccesso.
///
/// I letterali contano per la loro lunghezza, ogni campo per
/// [`BYTE_PER_CAMPO`] (il campo piu' largo di chrono, `%+` con i
/// nanosecondi e l'offset, sta sotto i 40 byte; un nome di fuso sotto i 64).
///
/// Un valore formattato non dipende dal testo della cella se non per la
/// larghezza dei campi, quindi questo limite vale per ogni riga: l'analisi
/// lo confronta con `max_string_bytes` e i kernel non devono controllare
/// l'uscita riga per riga.
#[must_use]
pub fn byte_massimi_scritti(format: &str) -> usize {
    StrftimeItems::new(format).fold(0_usize, |totale, item| {
        let byte = match item {
            Item::Literal(testo) | Item::Space(testo) => testo.len(),
            Item::OwnedLiteral(testo) | Item::OwnedSpace(testo) => testo.len(),
            _ => BYTE_PER_CAMPO,
        };
        totale.saturating_add(byte)
    })
}

/// Tetto per eccesso dei byte di un campo strftime ([`byte_massimi_scritti`]).
pub const BYTE_PER_CAMPO: usize = 64;

/// Parsing con item precompilati, semantica identica a `parse`: prima il
/// ramo `NaiveDateTime::parse_from_str` (campi orario di default a
/// mezzanotte), poi il fallback `NaiveDate::parse_from_str`.
pub(crate) fn parse_with_items(value: &str, items: &[Item<'_>]) -> Option<NaiveDateTime> {
    let mut parsed = Parsed::new();
    if chrono::format::parse(&mut parsed, value, items.iter()).is_ok() {
        // Stessa risoluzione di `NaiveDateTime::parse_from_str`
        // (`to_naive_datetime_with_offset(0)`; chrono e' pinnato a 0.4.45).
        if let Ok(datetime) = parsed.to_naive_datetime_with_offset(0) {
            return Some(datetime);
        }
    }
    let mut parsed = Parsed::new();
    if chrono::format::parse(&mut parsed, value, items.iter()).is_ok() {
        if let Ok(date) = parsed.to_naive_date() {
            return date.and_hms_opt(0, 0, 0);
        }
    }
    None
}

/// Un formato strftime di config e' riconosciuto da chrono.
///
/// Un item non riconosciuto (`%Q`, `%` finale) non fa mai combaciare il
/// parsing e manda in errore la scrittura: si rifiuta come errore di piano
/// invece di scartare ogni riga o, nei cast con `coerce`, di ignorarlo.
///
/// # Errors
///
/// `InvalidPlan` se il formato contiene item non riconosciuti.
pub fn validate_format_items(format: &str, label: &str) -> Result<()> {
    if StrftimeItems::new(format).any(|item| matches!(item, Item::Error)) {
        return Err(PlenoraError::InvalidPlan(format!(
            "{label} non riconosciuto"
        )));
    }
    Ok(())
}

/// Scrive un valore temporale formattato, senza panico.
///
/// `to_string()` su un formato chrono va in panico quando chrono rende
/// `fmt::Error` (item che chiede un fuso a un valore che non lo ha, anni
/// fuori dall'intervallo di RFC 2822); `write!` su una `String` lo rende
/// come errore.
fn scrivi_formattato(valore: impl std::fmt::Display) -> Result<String> {
    use std::fmt::Write as _;
    let mut testo = String::new();
    write!(testo, "{valore}").map_err(|_| {
        PlenoraError::DataMapping("valore temporale non rappresentabile con output_format".into())
    })?;
    Ok(testo)
}

/// Un `output_format` compilato e verificato per il tipo di valore che lo
/// scrive.
///
/// Rifiuta con errore di piano, prima di leggere i dati, ogni formato che
/// chrono non saprebbe scrivere per quel tipo: item non riconosciuti e, per i
/// valori senza fuso, gli item di offset e di fuso (`%z`, `%Z`, `%:z`,
/// `%+`), che chrono scrive solo per un valore con fuso. Per riga resta un
/// errore esplicito, mai un panico ne' un testo sbagliato: il secolo (`%C`)
/// fuori dagli anni 0..=9999, che chrono scriverebbe con un carattere non
/// numerico al posto delle decine, e gli anni fuori dall'intervallo di
/// RFC 2822.
pub(crate) struct FormatoUscita<'a> {
    items: Vec<Item<'a>>,
    /// Il formato scrive il secolo civile a due cifre (`%C`, anno / 100).
    secolo_civile: bool,
    /// Il formato scrive il secolo dell'anno ISO a due cifre (item chrono
    /// `IsoYearDiv100`, senza specificatore strftime ma costruibile).
    secolo_iso: bool,
}

impl<'a> FormatoUscita<'a> {
    fn compila(format: &'a str) -> Result<Self> {
        validate_format_items(format, "output_format")?;
        let items = compile_items(format);
        let secolo_civile = items
            .iter()
            .any(|item| matches!(item, Item::Numeric(Numeric::YearDiv100, _)));
        let secolo_iso = items
            .iter()
            .any(|item| matches!(item, Item::Numeric(Numeric::IsoYearDiv100, _)));
        Ok(Self {
            items,
            secolo_civile,
            secolo_iso,
        })
    }

    /// Istante di prova per scoprire, prima dei dati, gli item che chrono
    /// non sa scrivere per il tipo del valore: il risultato non dipende
    /// dall'istante se non per gli anni, e il 2000 e' dentro ogni intervallo.
    fn campione() -> Result<NaiveDateTime> {
        NaiveDate::from_ymd_opt(2000, 1, 1)
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .ok_or_else(|| PlenoraError::Internal("istante di prova non costruibile".into()))
    }

    /// Per valori senza fuso (`NaiveDateTime`).
    pub(crate) fn senza_fuso(format: &'a str) -> Result<Self> {
        let formato = Self::compila(format)?;
        if formato.scrivi_senza_fuso(&Self::campione()?).is_err() {
            return Err(PlenoraError::InvalidPlan(
                "output_format richiede un fuso orario o un offset, che il valore non ha".into(),
            ));
        }
        Ok(formato)
    }

    /// Per valori con il fuso `fuso`.
    pub(crate) fn con_fuso(format: &'a str, fuso: Tz) -> Result<Self> {
        let formato = Self::compila(format)?;
        let campione = fuso.from_utc_datetime(&Self::campione()?);
        if formato.scrivi_con_fuso(&campione).is_err() {
            return Err(PlenoraError::InvalidPlan(
                "output_format non applicabile ai valori con fuso".into(),
            ));
        }
        Ok(formato)
    }

    /// Ogni secolo si controlla sul proprio anno: `%C` sul civile, il
    /// secolo ISO sull'anno ISO. 0000-01-01 ha anno civile 0 (secolo "00")
    /// e anno ISO -1.
    fn controlla_secolo(&self, anno: i32, anno_iso: i32) -> Result<()> {
        let fuori = |anno: i32| !(0..=9999).contains(&anno);
        if (self.secolo_civile && fuori(anno)) || (self.secolo_iso && fuori(anno_iso)) {
            return Err(PlenoraError::DataMapping(
                "anno fuori da 0..=9999: il secolo di output_format non e' rappresentabile".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn scrivi_senza_fuso(&self, valore: &NaiveDateTime) -> Result<String> {
        self.controlla_secolo(valore.year(), valore.iso_week().year())?;
        scrivi_formattato(valore.format_with_items(self.items.iter()))
    }

    pub(crate) fn scrivi_con_fuso(&self, valore: &DateTime<Tz>) -> Result<String> {
        self.controlla_secolo(valore.year(), valore.iso_week().year())?;
        scrivi_formattato(valore.format_with_items(self.items.iter()))
    }
}

/// Riga non leggibile dopo la prevalidazione per riga: irraggiungibile.
fn invalid<T>(operation: &str) -> Result<Option<T>> {
    Err(PlenoraError::Internal(format!(
        "prevalidazione row-scoped incoerente in {operation}"
    )))
}

/// Config di `table.date_format`: riscrittura di date e ore da un formato
/// `chrono` a un altro.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateFormat {
    /// Colonna da leggere, come testo.
    pub column: String,
    /// Formato strftime di lettura; deve consumare tutto il testo. Senza
    /// campi orari la data si pone a mezzanotte.
    pub input_format: String,
    /// Formato strftime di scrittura, senza campi di fuso; default
    /// `%Y-%m-%d %H:%M:%S`.
    #[serde(default = "default_output_format")]
    pub output_format: String,
    /// Colonna d'uscita (`Utf8` nullable).
    pub output_column: String,
    /// Non ammesso: un valore non leggibile rifiuta sempre la riga, quindi
    /// nessuna politica avrebbe effetto. Scritto si rifiuta
    /// ([`verifica_politiche`]).
    #[serde(default)]
    pub invalid: Option<InvalidDatePolicy>,
}

/// `invalid` e `ambiguous` delle operazioni sulle date: scritti si rifiutano.
///
/// Non hanno effetto con nessun valore: un valore non leggibile, un'ora
/// ambigua o inesistente rifiutano sempre la riga con la diagnostica per
/// riga. Scritti si
/// rifiutano invece di promettere un null o una scelta che non avviene. La
/// chiamano i kernel `date_format`, `date_add`, `date_diff`,
/// `timezone_convert`, `date_extract` e l'analisi dei contratti.
///
/// # Errors
///
/// `InvalidPlan` se `invalid` o `ambiguous` e' scritto.
pub fn verifica_politiche(
    invalid: Option<&InvalidDatePolicy>,
    ambiguous: Option<&AmbiguousPolicy>,
) -> Result<()> {
    if invalid.is_some() {
        return Err(PlenoraError::InvalidPlan(
            "invalid non ha effetto: un valore non leggibile rifiuta sempre la riga".into(),
        ));
    }
    if ambiguous.is_some() {
        return Err(PlenoraError::InvalidPlan(
            "ambiguous non ha effetto: un'ora ambigua o inesistente rifiuta sempre la riga".into(),
        ));
    }
    Ok(())
}

/// Riformatta la colonna `column` da `input_format` a `output_format`
/// nella colonna `output_column`.
///
/// Fast path su colonne Utf8 (item strftime precompilati), percorso
/// generico riga-per-riga sugli altri tipi Arrow. I valori non parsabili
/// sono sempre rifiutati con diagnostica row-scoped; `invalid` scritto si
/// rifiuta ([`verifica_politiche`]).
///
/// # Errors
///
/// - `InvalidPlan`: formato con item non riconosciuti, o `output_format`
///   con item di fuso o di offset;
/// - `DataMapping`: uno o piu' valori non parsabili, con row diagnostics
///   (`conversion.invalid_datetime`); senza diagnostica, un valore che
///   `output_format` non sa scrivere (secolo `%C` fuori da 0..=9999);
/// - `Schema`: colonna assente (come `column_index`) o valore non
///   convertibile in testo (come `scalar_as_string`); gli errori di
///   `replace_or_append`.
pub fn date_format(batch: &RecordBatch, config: &DateFormat) -> Result<RecordBatch> {
    verifica_politiche(config.invalid.as_ref(), None)?;
    validate_format_items(&config.input_format, "input_format")?;
    let uscita = FormatoUscita::senza_fuso(&config.output_format)?;
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        if scalar_as_string(source.as_ref(), row)?
            .is_some_and(|value| parse(&value, &config.input_format).is_none())
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
    let values = if let Some(column) = source.as_any().downcast_ref::<StringArray>() {
        let input_items = compile_items(&config.input_format);
        let mut values = Vec::with_capacity(column.len());
        for row in 0..column.len() {
            if column.is_null(row) {
                values.push(None);
                continue;
            }
            let parsed = parse_with_items(column.value(row), &input_items);
            values.push(match parsed {
                Some(value) => Some(uscita.scrivi_senza_fuso(&value)?),
                None => invalid("date_format")?,
            });
        }
        values
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    return Ok(None);
                };
                parse(&value, &config.input_format).map_or_else(
                    || invalid("date_format"),
                    |value| uscita.scrivi_senza_fuso(&value).map(Some),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

/// Unita' di `amount` in `table.date_add`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateUnit {
    /// Anni di calendario (12 mesi).
    Years,
    /// Mesi di calendario: il giorno oltre la fine del mese diventa l'ultimo
    /// giorno del mese.
    Months,
    /// Settimane di 7 giorni.
    Weeks,
    /// Giorni di 24 ore (il valore non ha fuso).
    Days,
    /// Ore.
    Hours,
    /// Minuti.
    Minutes,
    /// Secondi.
    Seconds,
}

/// Config di `table.date_add`: somma di una quantita' fissa a date e ore.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateAdd {
    /// Colonna da leggere, come testo.
    pub column: String,
    /// Formato strftime di lettura; deve consumare tutto il testo.
    pub input_format: String,
    /// Formato strftime di scrittura, senza campi di fuso; default
    /// `%Y-%m-%d %H:%M:%S`.
    #[serde(default = "default_output_format")]
    pub output_format: String,
    /// Quantita' da aggiungere, con segno; l'analisi rifiuta quella che
    /// nessuna data sopporta ([`verifica_amount`]).
    pub amount: i64,
    /// Unita' di `amount`.
    pub unit: DateUnit,
    /// Colonna d'uscita (`Utf8` nullable).
    pub output_column: String,
    /// Non ammesso: un valore non leggibile rifiuta sempre la riga, quindi
    /// nessuna politica avrebbe effetto. Scritto si rifiuta
    /// ([`verifica_politiche`]).
    #[serde(default)]
    pub invalid: Option<InvalidDatePolicy>,
}

fn shift_months(value: NaiveDateTime, amount: i64, multiplier: u32) -> Option<NaiveDateTime> {
    let count = amount.unsigned_abs().checked_mul(u64::from(multiplier))?;
    let months = Months::new(u32::try_from(count).ok()?);
    if amount < 0 {
        value.checked_sub_months(months)
    } else {
        value.checked_add_months(months)
    }
}

fn shift(value: NaiveDateTime, amount: i64, unit: &DateUnit) -> Option<NaiveDateTime> {
    let delta = match unit {
        DateUnit::Years => return shift_months(value, amount, 12),
        DateUnit::Months => return shift_months(value, amount, 1),
        DateUnit::Weeks => TimeDelta::try_weeks(amount),
        DateUnit::Days => TimeDelta::try_days(amount),
        DateUnit::Hours => TimeDelta::try_hours(amount),
        DateUnit::Minutes => TimeDelta::try_minutes(amount),
        DateUnit::Seconds => TimeDelta::try_seconds(amount),
    }?;
    value.checked_add_signed(delta)
}

/// Rifiuta un `amount` che nessuna data rappresentabile sopporta.
///
/// Lo spostamento fallisce anche dall'estremo da cui c'e' piu' spazio (il
/// minimo di `NaiveDateTime` per un `amount` positivo, il massimo o il
/// secondo intercalare dell'ultimo giorno per uno negativo), quindi su ogni
/// riga valida. `date_add` lo rifiuterebbe riga per
/// riga come `conversion.datetime_range`; l'analisi dei contratti lo rifiuta
/// prima, dalla sola config.
///
/// Lo spostamento e' monotono nel valore di partenza (durate fisse e mesi di
/// calendario), quindi il controllo non rifiuta un `amount` che anche una
/// sola data potrebbe sopportare.
///
/// # Errors
///
/// `InvalidPlan` se `amount` in `unit` non e' applicabile a nessuna data.
pub fn verifica_amount(amount: i64, unit: &DateUnit) -> Result<()> {
    // Gli estremi di ogni rappresentazione: chrono ammette il secondo
    // intercalare (`23:59:60`, nanosecondi oltre 10^9), e da quello del
    // giorno massimo uno spostamento all'indietro arriva piu' lontano che da
    // `NaiveDateTime::MAX` (23:59:59.999999999). Si rifiuta solo cio' che
    // fallisce da tutti.
    let intercalare = |data: NaiveDate| data.and_hms_nano_opt(23, 59, 59, 1_999_999_999);
    let estremi = [
        Some(NaiveDateTime::MIN),
        Some(NaiveDateTime::MAX),
        intercalare(NaiveDate::MIN),
        intercalare(NaiveDate::MAX),
    ];
    if estremi
        .into_iter()
        .flatten()
        .all(|partenza| shift(partenza, amount, unit).is_none())
    {
        return Err(PlenoraError::InvalidPlan(
            "amount fuori scala: nessuna data rappresentabile lo sopporta".into(),
        ));
    }
    Ok(())
}

/// Somma `amount` unita' (`unit`) ai valori della colonna `column`,
/// riscritti con `output_format` nella colonna `output_column`.
///
/// Anni e mesi usano aritmetica di calendario (`Months`), le altre
/// unita' durate fisse; valori non parsabili e overflow di data o delta sono
/// sempre rifiutati con diagnostica row-scoped.
///
/// # Errors
///
/// - `InvalidPlan`: formato con item non riconosciuti, o `output_format`
///   con item di fuso o di offset;
/// - `DataMapping`: valore non parsabile (`conversion.invalid_datetime`),
///   oppure data risultante o delta fuori range
///   (`conversion.datetime_range`), con row diagnostics; senza diagnostica,
///   un valore che `output_format` non sa scrivere;
/// - `Schema`: colonna assente (come `column_index`) o valore non
///   convertibile in testo (come `scalar_as_string`); gli errori di
///   `replace_or_append`.
pub fn date_add(batch: &RecordBatch, config: &DateAdd) -> Result<RecordBatch> {
    verifica_politiche(config.invalid.as_ref(), None)?;
    validate_format_items(&config.input_format, "input_format")?;
    let uscita = FormatoUscita::senza_fuso(&config.output_format)?;
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        let Some(value) = scalar_as_string(source.as_ref(), row)? else {
            continue;
        };
        let cause = match parse(&value, &config.input_format) {
            None => "conversion.invalid_datetime",
            Some(value) if shift(value, config.amount, &config.unit).is_none() => {
                "conversion.datetime_range"
            }
            Some(_) => continue,
        };
        rejections.push(RowRejection {
            row,
            cause,
            column: Some(&config.column),
        });
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
    let values = if let Some(column) = source.as_any().downcast_ref::<StringArray>() {
        let input_items = compile_items(&config.input_format);
        // Delta precomputato per le unita' a durata fissa (mai ricalcolato
        // per riga); anni/mesi restano sul percorso `Months` per-riga.
        let fixed_delta = match config.unit {
            DateUnit::Years | DateUnit::Months => None,
            DateUnit::Weeks => Some(TimeDelta::try_weeks(config.amount)),
            DateUnit::Days => Some(TimeDelta::try_days(config.amount)),
            DateUnit::Hours => Some(TimeDelta::try_hours(config.amount)),
            DateUnit::Minutes => Some(TimeDelta::try_minutes(config.amount)),
            DateUnit::Seconds => Some(TimeDelta::try_seconds(config.amount)),
        };
        let shift_row = |value: NaiveDateTime| -> Option<NaiveDateTime> {
            match fixed_delta {
                Some(delta) => delta.and_then(|delta| value.checked_add_signed(delta)),
                None if matches!(config.unit, DateUnit::Years) => {
                    shift_months(value, config.amount, 12)
                }
                None => shift_months(value, config.amount, 1),
            }
        };
        let mut values = Vec::with_capacity(column.len());
        for row in 0..column.len() {
            if column.is_null(row) {
                values.push(None);
                continue;
            }
            let shifted = parse_with_items(column.value(row), &input_items).and_then(shift_row);
            values.push(match shifted {
                Some(value) => Some(uscita.scrivi_senza_fuso(&value)?),
                None => invalid("date_add")?,
            });
        }
        values
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    return Ok(None);
                };
                let shifted = parse(&value, &config.input_format)
                    .and_then(|value| shift(value, config.amount, &config.unit));
                shifted.map_or_else(
                    || invalid("date_add"),
                    |value| uscita.scrivi_senza_fuso(&value).map(Some),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

/// Unita' della differenza di `table.date_diff` (durate fisse).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffUnit {
    /// Giorni di 86 400 secondi.
    Days,
    /// Ore.
    Hours,
    /// Minuti.
    Minutes,
    /// Secondi.
    Seconds,
}

/// Config di `table.date_diff`: differenza `end - start` in unita'
/// frazionarie.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateDiff {
    /// Colonna dell'istante iniziale, letta come testo.
    pub start_column: String,
    /// Colonna dell'istante finale, letta come testo.
    pub end_column: String,
    /// Formato strftime di lettura di entrambe le colonne.
    pub input_format: String,
    /// Unita' della differenza.
    pub unit: DiffUnit,
    /// Colonna d'uscita (`Float64` nullable).
    pub output_column: String,
    /// Non ammesso: un valore non leggibile rifiuta sempre la riga, quindi
    /// nessuna politica avrebbe effetto. Scritto si rifiuta
    /// ([`verifica_politiche`]).
    #[serde(default)]
    pub invalid: Option<InvalidDatePolicy>,
}

/// Differenza in unita' frazionarie, identica al percorso generico
/// (errore "intervallo fuori scala" incluso, che la prevalidazione per riga
/// rende irraggiungibile).
///
/// # Arrotondamento dichiarato
///
/// Il risultato e' per contratto un `Float64`, quindi la conversione dei
/// nanosecondi e' arrotondata: oltre 2^53 nanosecondi (circa 104 giorni) il
/// conteggio esatto non entra in un double. «Fuori scala» riguarda solo i
/// nanosecondi oltre `i64` (circa 292 anni).
#[allow(clippy::cast_precision_loss)] // Arrotondamento voluto: l'output e' Float64 per contratto.
fn diff_value(start: NaiveDateTime, end: NaiveDateTime, divisor: f64, _row: usize) -> Result<f64> {
    end.signed_duration_since(start)
        .num_nanoseconds()
        .map(|nanoseconds| nanoseconds as f64 / 1_000_000_000.0 / divisor)
        .ok_or_else(|| PlenoraError::InvalidPlan("date_diff: intervallo fuori scala".into()))
}

/// Differenza `end_column - start_column` in unita' frazionarie
/// (`unit`), scritta come Float64 in `output_column`.
///
/// Un estremo null propaga null; un estremo non parsabile o un intervallo fuori
/// scala rifiuta sempre l'output con diagnostica row-scoped.
///
/// # Errors
///
/// - `InvalidPlan`: `input_format` con item non riconosciuti;
/// - `DataMapping`: valore non parsabile (`conversion.invalid_datetime`,
///   sulla colonna iniziale se e' quella a non leggersi) oppure intervallo
///   fuori scala, cioe' nanosecondi oltre `i64`
///   (`conversion.datetime_range`), con row diagnostics;
/// - `Schema`: colonna assente (come `column_index`) o valore non
///   convertibile in testo (come `scalar_as_string`); gli errori di
///   `replace_or_append`.
pub fn date_diff(batch: &RecordBatch, config: &DateDiff) -> Result<RecordBatch> {
    verifica_politiche(config.invalid.as_ref(), None)?;
    validate_format_items(&config.input_format, "input_format")?;
    let start_index = column_index(batch, &config.start_column)?;
    let end_index = column_index(batch, &config.end_column)?;
    let divisor = match config.unit {
        DiffUnit::Days => 86_400.0,
        DiffUnit::Hours => 3_600.0,
        DiffUnit::Minutes => 60.0,
        DiffUnit::Seconds => 1.0,
    };
    let start_source = batch.column(start_index);
    let end_source = batch.column(end_index);
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        let start = scalar_as_string(start_source.as_ref(), row)?;
        let end = scalar_as_string(end_source.as_ref(), row)?;
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        let parsed_start = parse(&start, &config.input_format);
        let parsed_end = parse(&end, &config.input_format);
        let (cause, column) = match (parsed_start, parsed_end) {
            (None, _) => (
                "conversion.invalid_datetime",
                Some(config.start_column.as_str()),
            ),
            (_, None) => (
                "conversion.invalid_datetime",
                Some(config.end_column.as_str()),
            ),
            (Some(start), Some(end))
                if end.signed_duration_since(start).num_nanoseconds().is_none() =>
            {
                ("conversion.datetime_range", None)
            }
            (Some(_), Some(_)) => continue,
        };
        rejections.push(RowRejection { row, cause, column });
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
    let values = if let (Some(starts), Some(ends)) = (
        start_source.as_any().downcast_ref::<StringArray>(),
        end_source.as_any().downcast_ref::<StringArray>(),
    ) {
        let input_items = compile_items(&config.input_format);
        let mut values = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            if starts.is_null(row) || ends.is_null(row) {
                values.push(None);
                continue;
            }
            let parsed = parse_with_items(starts.value(row), &input_items)
                .zip(parse_with_items(ends.value(row), &input_items));
            values.push(match parsed {
                Some((start, end)) => Some(diff_value(start, end, divisor, row)?),
                None => invalid("date_diff")?,
            });
        }
        values
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let start = scalar_as_string(start_source.as_ref(), row)?;
                let end = scalar_as_string(end_source.as_ref(), row)?;
                if start.is_none() || end.is_none() {
                    return Ok(None);
                }
                let parsed = start
                    .as_deref()
                    .and_then(|value| parse(value, &config.input_format))
                    .zip(
                        end.as_deref()
                            .and_then(|value| parse(value, &config.input_format)),
                    );
                parsed.map_or_else(
                    || invalid("date_diff"),
                    |(start, end)| diff_value(start, end, divisor, row).map(Some),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Float64,
        true,
        Arc::new(Float64Array::from(values)),
    )
}

/// Politica sulle ore locali ambigue di `table.timezone_convert`.
///
/// Nessun valore ha effetto (un'ora ambigua o inesistente rifiuta sempre la
/// riga), quindi scritta si rifiuta ([`verifica_politiche`]). Resta un tipo
/// perche' il rifiuto nomini il motivo invece di un campo sconosciuto.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguousPolicy {
    /// Rifiuto (default).
    Error,
    /// Senza effetto.
    Null,
    /// Senza effetto.
    Earliest,
    /// Senza effetto.
    Latest,
}

/// Config di `table.timezone_convert`: ora locale di un fuso riscritta come
/// ora locale di un altro.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimezoneConvert {
    /// Colonna da leggere, come testo.
    pub column: String,
    /// Formato strftime di lettura; il valore letto e' ora locale di
    /// `source_timezone`.
    pub input_format: String,
    /// Formato strftime di scrittura, anche con campi di fuso; default
    /// `%Y-%m-%d %H:%M:%S`.
    #[serde(default = "default_output_format")]
    pub output_format: String,
    /// Fuso dei valori letti, nome IANA (`chrono-tz`).
    pub source_timezone: String,
    /// Fuso dei valori scritti, nome IANA (`chrono-tz`).
    pub target_timezone: String,
    /// Colonna d'uscita (`Utf8` nullable).
    pub output_column: String,
    /// Non ammesso: un valore non leggibile rifiuta sempre la riga, quindi
    /// nessuna politica avrebbe effetto. Scritto si rifiuta
    /// ([`verifica_politiche`]).
    #[serde(default)]
    pub invalid: Option<InvalidDatePolicy>,
    /// Non ammesso, come `invalid` ([`verifica_politiche`]).
    #[serde(default)]
    pub ambiguous: Option<AmbiguousPolicy>,
}

fn localize(timezone: Tz, value: NaiveDateTime) -> Result<Option<chrono::DateTime<Tz>>> {
    match timezone.from_local_datetime(&value) {
        LocalResult::Single(value) => Ok(Some(value)),
        LocalResult::Ambiguous(_, _) | LocalResult::None => Err(PlenoraError::Internal(
            "prevalidazione row-scoped incoerente in timezone_convert".into(),
        )),
    }
}

/// Converte la colonna `column` da `source_timezone` a
/// `target_timezone`, riscritta con `output_format` nella colonna
/// `output_column`.
///
/// Ore ambigue/inesistenti e valori non parsabili sono sempre rifiutati con
/// diagnostica row-scoped, senza scelta o null sintetico: `ambiguous` e
/// `invalid` scritti si rifiutano ([`verifica_politiche`]).
///
/// # Errors
///
/// - `InvalidPlan`: `source_timezone` o `target_timezone` non valida;
///   formato con item non riconosciuti o che non si sa scrivere per un
///   valore con fuso;
/// - `DataMapping`: ora ambigua (`conversion.ambiguous_local_time`) o
///   inesistente (`conversion.nonexistent_local_time`) o valore non
///   parsabile (`conversion.invalid_datetime`), con row diagnostics; senza
///   diagnostica, un valore che `output_format` non sa scrivere;
/// - `Schema`: colonna assente (come `column_index`) o valore non
///   convertibile in testo (come `scalar_as_string`); gli errori di
///   `replace_or_append`.
pub fn timezone_convert(batch: &RecordBatch, config: &TimezoneConvert) -> Result<RecordBatch> {
    verifica_politiche(config.invalid.as_ref(), config.ambiguous.as_ref())?;
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let source_tz: Tz = config
        .source_timezone
        .parse()
        .map_err(|_| PlenoraError::InvalidPlan("source_timezone non valida".into()))?;
    let target_tz: Tz = config
        .target_timezone
        .parse()
        .map_err(|_| PlenoraError::InvalidPlan("target_timezone non valida".into()))?;
    validate_format_items(&config.input_format, "input_format")?;
    let uscita = FormatoUscita::con_fuso(&config.output_format, target_tz)?;
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        let Some(value) = scalar_as_string(source.as_ref(), row)? else {
            continue;
        };
        let Some(parsed) = parse(&value, &config.input_format) else {
            rejections.push(RowRejection {
                row,
                cause: "conversion.invalid_datetime",
                column: Some(&config.column),
            });
            continue;
        };
        let cause = match source_tz.from_local_datetime(&parsed) {
            LocalResult::Single(_) => continue,
            LocalResult::Ambiguous(_, _) => "conversion.ambiguous_local_time",
            LocalResult::None => "conversion.nonexistent_local_time",
        };
        rejections.push(RowRejection {
            row,
            cause,
            column: Some(&config.column),
        });
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
    let values = if let Some(column) = source.as_any().downcast_ref::<StringArray>() {
        let input_items = compile_items(&config.input_format);
        let mut values = Vec::with_capacity(column.len());
        for row in 0..column.len() {
            if column.is_null(row) {
                values.push(None);
                continue;
            }
            let Some(parsed) = parse_with_items(column.value(row), &input_items) else {
                values.push(invalid("timezone_convert")?);
                continue;
            };
            let localized = localize(source_tz, parsed)?;
            values.push(match localized {
                Some(value) => Some(uscita.scrivi_con_fuso(&value.with_timezone(&target_tz))?),
                None => invalid("timezone_convert")?,
            });
        }
        values
    } else {
        (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(source.as_ref(), row)? else {
                    return Ok(None);
                };
                let Some(parsed) = parse(&value, &config.input_format) else {
                    return invalid("timezone_convert");
                };
                let localized = localize(source_tz, parsed)?;
                localized.map_or_else(
                    || invalid("timezone_convert"),
                    |value| {
                        uscita
                            .scrivi_con_fuso(&value.with_timezone(&target_tz))
                            .map(Some)
                    },
                )
            })
            .collect::<Result<Vec<_>>>()?
    };
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{ArrayRef, Int64Array};
    use plenora_core::arrow::schema::{Field, Schema};
    use plenora_core::diagnostics::RowDiagnosticsCompleteness;

    use super::*;
    use crate::test_support::{assert_same_outcome as assert_equivalent, single_column_batch};

    // -----------------------------------------------------------------------
    // Percorsi generici, indipendenti dai fast path: sono l'oracolo della
    // loro equivalenza semantica. Codificano la semantica corrente: ogni
    // valore non parsabile, fuori range o con ora locale ambigua/inesistente
    // rifiuta l'intero batch con diagnostica row-scoped, qualunque sia il
    // token `invalid`; null resta null.
    //
    // Che cosa garantiscono: che i fast path (item precompilati, loop nativi
    // Utf8, delta precalcolato) diano lo stesso batch del percorso per riga, e
    // che il rifiuto abbia le stesse righe, cause e colonne.
    // Che cosa NON garantiscono: condividono con la produzione `parse`
    // (chrono `parse_from_str`), `shift` (quindi `shift_months` per anni e
    // mesi) e la localizzazione chrono-tz; un difetto li' colpirebbe fast
    // path e oracolo allo stesso modo. Li coprono i valori attesi scritti a
    // mano: `date_add_hand_written_calendar_cases` e
    // `timezone_convert_hand_written_rome_transitions`.
    // -----------------------------------------------------------------------

    /// Esito per riga dell'oracolo: valore (o null) oppure rifiuto con causa
    /// e colonna.
    type RowOutcome<'a, T> = std::result::Result<Option<T>, (&'static str, Option<&'a str>)>;

    /// Chiude l'oracolo: rifiuti row-scoped se ce ne sono, altrimenti i valori.
    fn collect_or_reject<T>(outcomes: Vec<RowOutcome<'_, T>>) -> Result<Vec<Option<T>>> {
        let rejections = outcomes
            .iter()
            .enumerate()
            .filter_map(|(row, outcome)| {
                outcome
                    .as_ref()
                    .err()
                    .map(|&(cause, column)| RowRejection { row, cause, column })
            })
            .collect::<Vec<_>>();
        reject_rows(
            &rejections,
            "valori temporali rifiutati; consultare row_diagnostics",
        )?;
        Ok(outcomes
            .into_iter()
            .map(|outcome| outcome.unwrap_or(None))
            .collect())
    }

    /// Colonna `d` Utf8 con i valori dati.
    fn date_testuali(valori: &[&str]) -> RecordBatch {
        single_column_batch(
            "d",
            Arc::new(StringArray::from(valori.to_vec())),
            DataType::Utf8,
            true,
        )
    }

    /// Colonna `d` Date32 (percorso generico, non Utf8): 2020-01-01.
    fn date_native() -> RecordBatch {
        single_column_batch(
            "d",
            Arc::new(plenora_core::arrow::array::Date32Array::from(vec![18_262])),
            DataType::Date32,
            true,
        )
    }

    fn config<T: serde::de::DeserializeOwned>(valore: serde_json::Value) -> T {
        serde_json::from_value(valore).expect("config")
    }

    #[test]
    fn un_formato_con_fuso_su_un_valore_senza_fuso_e_un_errore_non_un_panico() {
        // chrono non sa scrivere un offset o un nome di fuso per un
        // NaiveDateTime: `to_string()` sul formato andava in panico.
        for formato in ["%z", "%Z", "%:z", "%#z", "%+", "%Y-%m-%d %z"] {
            for batch in [date_testuali(&["2020-01-01"]), date_native()] {
                let esito = date_format(
                    &batch,
                    &config(serde_json::json!({
                        "column": "d", "input_format": "%Y-%m-%d",
                        "output_format": formato, "output_column": "o"})),
                );
                assert!(
                    matches!(esito, Err(PlenoraError::InvalidPlan(_))),
                    "date_format {formato}: {esito:?}"
                );
                let esito = date_add(
                    &batch,
                    &config(serde_json::json!({
                        "column": "d", "input_format": "%Y-%m-%d", "amount": 1,
                        "unit": "days", "output_format": formato, "output_column": "o"})),
                );
                assert!(
                    matches!(esito, Err(PlenoraError::InvalidPlan(_))),
                    "date_add {formato}: {esito:?}"
                );
            }
        }
        // Con un fuso il valore lo ha: timezone_convert li scrive.
        let convertito = timezone_convert(
            &date_testuali(&["2020-01-01 12:00:00"]),
            &config(serde_json::json!({
                "column": "d", "input_format": "%Y-%m-%d %H:%M:%S",
                "output_format": "%Y-%m-%d %H:%M %z %Z", "source_timezone": "UTC",
                "target_timezone": "Europe/Rome", "output_column": "o"})),
        )
        .expect("timezone_convert con %z");
        let uscita = convertito
            .column_by_name("o")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .expect("o");
        assert_eq!(uscita.value(0), "2020-01-01 13:00 +0100 CET");
    }

    #[test]
    fn un_formato_malformato_e_un_errore_di_piano() {
        let batch = date_testuali(&["2020-01-01"]);
        for formato in ["%Q", "%Y-%", "%Ez"] {
            let esiti = [
                date_format(
                    &batch,
                    &config(serde_json::json!({
                        "column": "d", "input_format": "%Y-%m-%d",
                        "output_format": formato, "output_column": "o"})),
                ),
                date_format(
                    &batch,
                    &config(serde_json::json!({
                        "column": "d", "input_format": formato, "output_column": "o"})),
                ),
                date_add(
                    &batch,
                    &config(serde_json::json!({
                        "column": "d", "input_format": formato, "amount": 1,
                        "unit": "days", "output_column": "o"})),
                ),
                date_diff(
                    &batch,
                    &config(serde_json::json!({
                        "start_column": "d", "end_column": "d", "input_format": formato,
                        "unit": "days", "output_column": "o"})),
                ),
                timezone_convert(
                    &batch,
                    &config(serde_json::json!({
                        "column": "d", "input_format": "%Y-%m-%d", "output_format": formato,
                        "source_timezone": "UTC", "target_timezone": "UTC",
                        "output_column": "o"})),
                ),
            ];
            for (indice, esito) in esiti.into_iter().enumerate() {
                assert!(
                    matches!(esito, Err(PlenoraError::InvalidPlan(_))),
                    "{formato}, caso {indice}: {esito:?}"
                );
            }
        }
    }

    #[test]
    fn date_extract_e_type_cast_rifiutano_un_date_format_malformato() {
        let batch = date_testuali(&["2020-01-01"]);
        let esito = crate::utility::date_extract(
            &batch,
            &config(serde_json::json!({"column": "d", "parts": ["year"], "date_format": "%Q"})),
        );
        assert!(
            matches!(esito, Err(PlenoraError::InvalidPlan(_))),
            "date_extract: {esito:?}"
        );
        for errors in ["coerce", "raise"] {
            for target in ["date", "datetime", "date32", "timestamp_millis"] {
                let esito = crate::cleansing::type_cast(
                    &batch,
                    &config(serde_json::json!({
                        "column": "d", "target_type": target,
                        "date_format": "%Y-%m-%d %Q", "errors": errors})),
                );
                assert!(
                    matches!(esito, Err(PlenoraError::InvalidPlan(_))),
                    "type_cast {target} {errors}: {esito:?}"
                );
            }
        }
    }

    /// L'analisi rifiuta gli stessi formati del kernel.
    #[test]
    fn l_analisi_rifiuta_i_formati_che_il_kernel_rifiuta() {
        use plenora_core::contract::{DataContract, FieldAllocator};

        let batch = date_testuali(&["2020-01-01"]);
        let contratto = DataContract::tabular(batch.schema());
        let casi: Vec<(&str, serde_json::Value)> = ["%Q", "%z", "%Y %Z", "%C", "%Y-%m-%d"]
            .into_iter()
            .flat_map(|formato| {
                [
                    (
                        "table.date_format",
                        serde_json::json!({
                        "column": "d", "input_format": "%Y-%m-%d",
                        "output_format": formato, "output_column": "o"}),
                    ),
                    (
                        "table.date_format",
                        serde_json::json!({
                        "column": "d", "input_format": formato, "output_column": "o"}),
                    ),
                    (
                        "table.date_add",
                        serde_json::json!({
                        "column": "d", "input_format": "%Y-%m-%d", "amount": 1,
                        "unit": "days", "output_format": formato, "output_column": "o"}),
                    ),
                    (
                        "table.date_diff",
                        serde_json::json!({
                        "start_column": "d", "end_column": "d", "input_format": formato,
                        "unit": "days", "output_column": "o"}),
                    ),
                    (
                        "table.timezone_convert",
                        serde_json::json!({
                        "column": "d", "input_format": "%Y-%m-%d", "output_format": formato,
                        "source_timezone": "UTC", "target_timezone": "UTC",
                        "output_column": "o"}),
                    ),
                    (
                        "table.date_extract",
                        serde_json::json!({
                        "column": "d", "parts": ["year"], "date_format": formato}),
                    ),
                    (
                        "table.type_cast",
                        serde_json::json!({
                        "column": "d", "target_type": "date", "date_format": formato}),
                    ),
                ]
            })
            .collect();
        for (op, json) in casi {
            let analisi = crate::analyze::analyze_table_contract(
                op,
                std::slice::from_ref(&contratto),
                &json,
                &mut FieldAllocator::default(),
                &crate::Limits::default(),
            );
            let kernel: Result<RecordBatch> = match op {
                "table.date_format" => date_format(&batch, &config(json.clone())),
                "table.date_add" => date_add(&batch, &config(json.clone())),
                "table.date_diff" => date_diff(&batch, &config(json.clone())),
                "table.timezone_convert" => timezone_convert(&batch, &config(json.clone())),
                "table.date_extract" => crate::utility::date_extract(&batch, &config(json.clone())),
                _ => crate::cleansing::type_cast(&batch, &config(json.clone())),
            };
            // Il kernel puo' anche rifiutare le righe (DataMapping): conta solo
            // l'errore di piano, che l'analisi deve anticipare.
            let kernel_piano = matches!(kernel, Err(PlenoraError::InvalidPlan(_)));
            assert_eq!(
                analisi.is_err(),
                kernel_piano,
                "{op} {json}: analisi {analisi:?}, kernel {kernel:?}"
            );
        }
    }

    #[test]
    fn il_secolo_civile_non_dipende_dall_anno_iso() {
        // 0000-01-01 e 0000-01-02: anno civile 0, anno ISO -1. 9999-12-31:
        // anno civile e ISO 9999.
        for (valore, atteso) in [
            ("0000-01-01", "00"),
            ("0000-01-02", "00"),
            ("9999-12-31", "99"),
        ] {
            let uscita = date_format(
                &date_testuali(&[valore]),
                &config(serde_json::json!({
                    "column": "d", "input_format": "%Y-%m-%d",
                    "output_format": "%C", "output_column": "o"})),
            )
            .unwrap_or_else(|errore| panic!("{valore}: {errore}"));
            let testo = uscita
                .column_by_name("o")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .expect("o");
            assert_eq!(testo.value(0), atteso, "{valore}");
        }
    }

    #[test]
    fn il_secolo_iso_si_controlla_sull_anno_iso() {
        use chrono::format::Pad;

        let formato = FormatoUscita {
            items: vec![Item::Numeric(Numeric::IsoYearDiv100, Pad::Zero)],
            secolo_civile: false,
            secolo_iso: true,
        };
        let data = |a, m, g| {
            NaiveDate::from_ymd_opt(a, m, g)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .expect("data")
        };
        // 0000-01-01 (sabato) e 0000-01-02 sono nell'anno ISO -1, 0000-01-03 (lunedi')
        // apre la settimana 1 dell'anno ISO 0.
        assert!(matches!(
            formato.scrivi_senza_fuso(&data(0, 1, 2)),
            Err(PlenoraError::DataMapping(_))
        ));
        assert_eq!(
            formato.scrivi_senza_fuso(&data(0, 1, 3)).expect("iso 0"),
            "00"
        );
        assert_eq!(
            formato
                .scrivi_senza_fuso(&data(9999, 12, 31))
                .expect("iso 9999"),
            "99"
        );
    }

    #[test]
    fn il_secolo_fuori_da_0_9999_e_un_errore_non_testo_sbagliato() {
        // `%C` scrive year/100 come due cifre: oltre l'anno 9999 chrono
        // produceva un carattere non numerico al posto delle decine.
        let esito = date_add(
            &date_testuali(&["9999-12-31"]),
            &config(serde_json::json!({
                "column": "d", "input_format": "%Y-%m-%d", "amount": 1,
                "unit": "days", "output_format": "%C", "output_column": "o"})),
        );
        assert!(
            matches!(esito, Err(PlenoraError::DataMapping(_))),
            "{esito:?}"
        );
        // Dentro l'intervallo resta il secolo.
        let esito = date_add(
            &date_testuali(&["1999-12-31"]),
            &config(serde_json::json!({
                "column": "d", "input_format": "%Y-%m-%d", "amount": 1,
                "unit": "days", "output_format": "%C", "output_column": "o"})),
        )
        .expect("secolo");
        let uscita = esito
            .column_by_name("o")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .expect("o");
        assert_eq!(uscita.value(0), "20");
    }

    fn generic_date_format(batch: &RecordBatch, config: &DateFormat) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let outcomes = (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(batch.column(index).as_ref(), row)? else {
                    return Ok(Ok(None));
                };
                Ok(parse(&value, &config.input_format)
                    .map(|value| Some(value.format(&config.output_format).to_string()))
                    .ok_or(("conversion.invalid_datetime", Some(config.column.as_str()))))
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(collect_or_reject(outcomes)?)),
        )
    }

    fn generic_date_add(batch: &RecordBatch, config: &DateAdd) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let outcomes = (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(batch.column(index).as_ref(), row)? else {
                    return Ok(Ok(None));
                };
                let column = Some(config.column.as_str());
                let Some(value) = parse(&value, &config.input_format) else {
                    return Ok(Err(("conversion.invalid_datetime", column)));
                };
                Ok(shift(value, config.amount, &config.unit)
                    .map(|value| Some(value.format(&config.output_format).to_string()))
                    .ok_or(("conversion.datetime_range", column)))
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(collect_or_reject(outcomes)?)),
        )
    }

    #[allow(clippy::cast_precision_loss)] // Come `diff_value`: l'output e' Float64.
    fn generic_date_diff(batch: &RecordBatch, config: &DateDiff) -> Result<RecordBatch> {
        let start_index = column_index(batch, &config.start_column)?;
        let end_index = column_index(batch, &config.end_column)?;
        let divisor = match config.unit {
            DiffUnit::Days => 86_400.0,
            DiffUnit::Hours => 3_600.0,
            DiffUnit::Minutes => 60.0,
            DiffUnit::Seconds => 1.0,
        };
        let outcomes = (0..batch.num_rows())
            .map(|row| {
                let start = scalar_as_string(batch.column(start_index).as_ref(), row)?;
                let end = scalar_as_string(batch.column(end_index).as_ref(), row)?;
                let (Some(start), Some(end)) = (start, end) else {
                    return Ok(Ok(None));
                };
                let Some(start) = parse(&start, &config.input_format) else {
                    return Ok(Err((
                        "conversion.invalid_datetime",
                        Some(config.start_column.as_str()),
                    )));
                };
                let Some(end) = parse(&end, &config.input_format) else {
                    return Ok(Err((
                        "conversion.invalid_datetime",
                        Some(config.end_column.as_str()),
                    )));
                };
                Ok(end
                    .signed_duration_since(start)
                    .num_nanoseconds()
                    .map(|nanoseconds| Some(nanoseconds as f64 / 1_000_000_000.0 / divisor))
                    .ok_or(("conversion.datetime_range", None)))
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Float64,
            true,
            Arc::new(Float64Array::from(collect_or_reject(outcomes)?)),
        )
    }

    fn generic_timezone_convert(
        batch: &RecordBatch,
        config: &TimezoneConvert,
    ) -> Result<RecordBatch> {
        let index = column_index(batch, &config.column)?;
        let source: Tz = config
            .source_timezone
            .parse()
            .map_err(|_| PlenoraError::InvalidPlan("source_timezone non valida".into()))?;
        let target: Tz = config
            .target_timezone
            .parse()
            .map_err(|_| PlenoraError::InvalidPlan("target_timezone non valida".into()))?;
        let outcomes = (0..batch.num_rows())
            .map(|row| {
                let Some(value) = scalar_as_string(batch.column(index).as_ref(), row)? else {
                    return Ok(Ok(None));
                };
                let column = Some(config.column.as_str());
                let Some(parsed) = parse(&value, &config.input_format) else {
                    return Ok(Err(("conversion.invalid_datetime", column)));
                };
                Ok(match source.from_local_datetime(&parsed) {
                    LocalResult::Single(value) => Ok(Some(
                        value
                            .with_timezone(&target)
                            .format(&config.output_format)
                            .to_string(),
                    )),
                    LocalResult::Ambiguous(_, _) => {
                        Err(("conversion.ambiguous_local_time", column))
                    }
                    LocalResult::None => Err(("conversion.nonexistent_local_time", column)),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        replace_or_append(
            batch,
            &config.output_column,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(collect_or_reject(outcomes)?)),
        )
    }

    fn utf8_batch(values: Vec<Option<&str>>) -> RecordBatch {
        single_column_batch(
            "ts",
            Arc::new(StringArray::from(values)),
            DataType::Utf8,
            true,
        )
    }

    /// Date limite tutte valide per `%Y-%m-%d %H:%M:%S`: epoch, pre-1970,
    /// anni bisestili e non, transizioni DST Europe/Rome (ora ambigua e ora
    /// inesistente, valide come naive e rifiutate solo dalla localizzazione
    /// in Europe/Rome), estremo alto, null.
    fn valid_edge_values() -> Vec<Option<&'static str>> {
        vec![
            Some("1970-01-01 00:00:00"), // epoch
            Some("1969-12-31 23:59:59"), // pre-epoch
            Some("1900-01-01 00:00:00"),
            Some("2000-02-29 12:30:45"), // bisestile (divisibile per 400)
            Some("2100-02-28 23:59:59"), // 2100 NON bisestile
            Some("2024-10-27 02:30:00"), // ambigua Europe/Rome (fine DST)
            Some("2024-03-31 02:30:00"), // inesistente Europe/Rome (inizio DST)
            Some("9999-12-31 23:59:59"),
            None,
        ]
    }

    /// Righe di `edge_values_with_invalid` non parsabili con
    /// `%Y-%m-%d %H:%M:%S`.
    const INVALID_EDGE_ROWS: [u64; 4] = [1, 3, 5, 7];

    /// Date limite con righe non parsabili intercalate a righe valide e null.
    fn edge_values_with_invalid() -> Vec<Option<&'static str>> {
        vec![
            Some("1970-01-01 00:00:00"),
            Some("2023-02-29 00:00:00"), // data inesistente
            Some("2000-02-29 12:30:45"),
            Some("2024-02-29"), // date-only (formato datetime: fallisce)
            None,
            Some(""), // vuota
            Some("9999-12-31 23:59:59"),
            Some("non una data"),
        ]
    }

    /// Equivalenza fast/generico dove entrambi devono riuscire: il confronto
    /// dei batch e' l'unico esito accettato.
    fn assert_same_output(fast: Result<RecordBatch>, generic: Result<RecordBatch>) {
        let fast = fast.expect("fast path rifiuta valori validi");
        let generic = generic.expect("oracolo generico rifiuta valori validi");
        assert_eq!(fast, generic);
    }

    /// Rifiuto row-scoped atteso: righe, cause e colonne esatte.
    fn assert_rejected(result: Result<RecordBatch>, expected: &[(u64, &str, Option<&str>)]) {
        let error = result.expect_err("righe invalide accettate");
        let report = error
            .row_diagnostics()
            .expect("diagnostica row-scoped mancante");
        assert_eq!(report.completeness, RowDiagnosticsCompleteness::Complete);
        assert_eq!(
            report.observed_total,
            u64::try_from(expected.len()).expect("fixture")
        );
        assert_eq!(
            report
                .examples
                .iter()
                .map(|example| (
                    example.source_index,
                    example.cause.as_str(),
                    example.column.as_deref()
                ))
                .collect::<Vec<_>>(),
            expected
        );
    }

    /// Righe invalide di `edge_values_with_invalid`, tutte con la stessa causa.
    fn invalid_edge_rejections(
        cause: &'static str,
    ) -> Vec<(u64, &'static str, Option<&'static str>)> {
        INVALID_EDGE_ROWS
            .iter()
            .map(|row| (*row, cause, Some("ts")))
            .collect()
    }

    fn assert_complete_rows(error: &PlenoraError, expected: &[u64], column: &str) {
        let report = error
            .row_diagnostics()
            .expect("diagnostica row-scoped mancante");
        assert_eq!(report.completeness, RowDiagnosticsCompleteness::Complete);
        assert_eq!(
            report.observed_total,
            u64::try_from(expected.len()).expect("fixture")
        );
        assert_eq!(report.total, Some(report.observed_total));
        assert_eq!(
            report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(report
            .examples
            .iter()
            .all(|example| example.column.as_deref() == Some(column)));
    }

    #[test]
    fn temporal_transforms_reject_all_invalid_rows_even_with_legacy_null_policy() {
        let batch = utf8_batch(vec![
            Some("2024-01-01 00:00:00"),
            Some("non una data"),
            None,
            Some("2023-02-29 00:00:00"),
        ]);
        let format = format_config(None);
        assert_complete_rows(
            &date_format(&batch, &format).expect_err("date_format ha accettato righe invalide"),
            &[1, 3],
            "ts",
        );
        let add = DateAdd {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%Y-%m-%d %H:%M:%S".into(),
            amount: 1,
            unit: DateUnit::Days,
            output_column: "out".into(),
            invalid: None,
        };
        assert_complete_rows(
            &date_add(&batch, &add).expect_err("date_add ha accettato righe invalide"),
            &[1, 3],
            "ts",
        );

        let timezone = TimezoneConvert {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%Y-%m-%d %H:%M:%S".into(),
            source_timezone: "Europe/Rome".into(),
            target_timezone: "UTC".into(),
            output_column: "out".into(),
            invalid: None,
            ambiguous: None,
        };
        let dst_batch = utf8_batch(vec![
            Some("2024-01-01 00:00:00"),
            Some("2024-10-27 02:30:00"),
            Some("non una data"),
        ]);
        assert_complete_rows(
            &timezone_convert(&dst_batch, &timezone)
                .expect_err("timezone_convert ha rimediato righe invalide"),
            &[1, 2],
            "ts",
        );
    }

    #[test]
    fn date_diff_preserves_null_as_valid_missing_data() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("start", DataType::Utf8, true),
                Field::new("end", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![None, Some("2024-01-01 00:00:00")])),
                Arc::new(StringArray::from(vec![Some("2024-01-02 00:00:00"), None])),
            ],
        )
        .expect("fixture");
        let output = date_diff(
            &batch,
            &DateDiff {
                start_column: "start".into(),
                end_column: "end".into(),
                input_format: "%Y-%m-%d %H:%M:%S".into(),
                unit: DiffUnit::Days,
                output_column: "out".into(),
                invalid: None,
            },
        )
        .expect("null ammessi");
        assert_eq!(output.column(2).null_count(), 2);
    }

    fn format_config(invalid: Option<InvalidDatePolicy>) -> DateFormat {
        DateFormat {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%d/%m/%Y %H:%M:%S".into(),
            output_column: "out".into(),
            invalid,
        }
    }

    #[test]
    fn date_format_fast_path_matches_generic_on_edge_dates() {
        let valid = utf8_batch(valid_edge_values());
        let with_invalid = utf8_batch(edge_values_with_invalid());
        {
            let invalid = None;
            let config = format_config(invalid);
            assert_same_output(
                date_format(&valid, &config),
                generic_date_format(&valid, &config),
            );
            assert_equivalent(
                date_format(&with_invalid, &config),
                generic_date_format(&with_invalid, &config),
            );
            assert_rejected(
                date_format(&with_invalid, &config),
                &invalid_edge_rejections("conversion.invalid_datetime"),
            );
        }
        // Formato date-only: il fallback `NaiveDate` deve coincidere.
        let dates_only = utf8_batch(vec![
            Some("1970-01-01"),
            Some("2000-02-29"),
            Some("1900-02-28"),
            None,
        ]);
        let dates_only_invalid = utf8_batch(vec![
            Some("1970-01-01"),
            Some("2023-02-29"),
            Some("2024-02-29 10:00:00"), // trailing input: fallisce
            None,
        ]);
        {
            let invalid = None;
            let config = DateFormat {
                input_format: "%Y-%m-%d".into(),
                ..format_config(invalid)
            };
            assert_same_output(
                date_format(&dates_only, &config),
                generic_date_format(&dates_only, &config),
            );
            assert_equivalent(
                date_format(&dates_only_invalid, &config),
                generic_date_format(&dates_only_invalid, &config),
            );
            assert_rejected(
                date_format(&dates_only_invalid, &config),
                &[
                    (1, "conversion.invalid_datetime", Some("ts")),
                    (2, "conversion.invalid_datetime", Some("ts")),
                ],
            );
        }
    }

    fn date_unit(code: u8) -> DateUnit {
        match code {
            0 => DateUnit::Years,
            1 => DateUnit::Months,
            2 => DateUnit::Weeks,
            3 => DateUnit::Days,
            4 => DateUnit::Hours,
            5 => DateUnit::Minutes,
            _ => DateUnit::Seconds,
        }
    }

    fn diff_unit(code: u8) -> DiffUnit {
        match code {
            0 => DiffUnit::Days,
            1 => DiffUnit::Hours,
            2 => DiffUnit::Minutes,
            _ => DiffUnit::Seconds,
        }
    }

    #[test]
    fn date_add_fast_path_matches_generic_on_all_units() {
        let valid = utf8_batch(valid_edge_values());
        let with_invalid = utf8_batch(edge_values_with_invalid());
        for unit_code in 0..7 {
            for amount in [7, -30, 90_061, 0] {
                let config = DateAdd {
                    column: "ts".into(),
                    input_format: "%Y-%m-%d %H:%M:%S".into(),
                    output_format: "%Y-%m-%d %H:%M:%S".into(),
                    amount,
                    unit: date_unit(unit_code),
                    output_column: "out".into(),
                    invalid: None,
                };
                assert_same_output(date_add(&valid, &config), generic_date_add(&valid, &config));
                assert_equivalent(
                    date_add(&with_invalid, &config),
                    generic_date_add(&with_invalid, &config),
                );
                assert_rejected(
                    date_add(&with_invalid, &config),
                    &invalid_edge_rejections("conversion.invalid_datetime"),
                );
            }
            // Overflow del delta: ogni riga valida e' fuori range, ogni riga
            // non parsabile resta `invalid_datetime`.
            let config = DateAdd {
                column: "ts".into(),
                input_format: "%Y-%m-%d %H:%M:%S".into(),
                output_format: "%Y-%m-%d %H:%M:%S".into(),
                amount: i64::MAX,
                unit: date_unit(unit_code),
                output_column: "out".into(),
                invalid: None,
            };
            assert_equivalent(
                date_add(&with_invalid, &config),
                generic_date_add(&with_invalid, &config),
            );
            assert_rejected(
                date_add(&with_invalid, &config),
                &[
                    (0, "conversion.datetime_range", Some("ts")),
                    (1, "conversion.invalid_datetime", Some("ts")),
                    (2, "conversion.datetime_range", Some("ts")),
                    (3, "conversion.invalid_datetime", Some("ts")),
                    (5, "conversion.invalid_datetime", Some("ts")),
                    (6, "conversion.datetime_range", Some("ts")),
                    (7, "conversion.invalid_datetime", Some("ts")),
                ],
            );
        }
    }

    #[test]
    fn date_diff_fast_path_matches_generic_including_out_of_scale() {
        let pair_batch = |starts: Vec<Option<&str>>, ends: Vec<Option<&str>>| {
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("start", DataType::Utf8, true),
                    Field::new("end", DataType::Utf8, true),
                ])),
                vec![
                    Arc::new(StringArray::from(starts)),
                    Arc::new(StringArray::from(ends)),
                ],
            )
            .expect("fixture")
        };
        let valid = pair_batch(
            vec![
                Some("1970-01-01 00:00:00"),
                Some("2024-03-31 01:59:59"), // attraversa il cambio DST
                Some("2024-10-27 03:00:00"),
                Some("1900-01-01 00:00:00"),
                Some("2024-02-29 00:00:00"),
                None,
                Some("2024-01-01 00:00:00"),
                Some("1900-01-01 00:00:00"), // ~291 anni: ultimo intervallo in scala
            ],
            vec![
                Some("1969-12-31 23:59:59"), // differenza negativa
                Some("2024-03-31 03:00:01"),
                Some("2024-10-27 01:30:00"),
                Some("2100-01-01 00:00:00"),
                Some("2024-02-29 00:00:00"), // zero
                Some("2024-01-01 00:00:00"),
                None,
                Some("2192-01-01 00:00:00"),
            ],
        );
        let with_invalid = pair_batch(
            vec![
                Some("1970-01-01 00:00:00"),
                Some("non una data"),
                Some("2024-01-01 00:00:00"),
                Some("1900-01-01 00:00:00"), // ~500 anni: nanosecondi fuori i64
                Some("non una data"),        // entrambi invalidi: vince start
            ],
            vec![
                Some("1970-01-02 00:00:00"),
                Some("2024-01-01 00:00:00"),
                Some("2023-02-29 00:00:00"),
                Some("2400-01-01 00:00:00"),
                Some(""),
            ],
        );
        for unit_code in 0..4 {
            {
                let config = DateDiff {
                    start_column: "start".into(),
                    end_column: "end".into(),
                    input_format: "%Y-%m-%d %H:%M:%S".into(),
                    unit: diff_unit(unit_code),
                    output_column: "out".into(),
                    invalid: None,
                };
                assert_same_output(
                    date_diff(&valid, &config),
                    generic_date_diff(&valid, &config),
                );
                assert_equivalent(
                    date_diff(&with_invalid, &config),
                    generic_date_diff(&with_invalid, &config),
                );
                assert_rejected(
                    date_diff(&with_invalid, &config),
                    &[
                        (1, "conversion.invalid_datetime", Some("start")),
                        (2, "conversion.invalid_datetime", Some("end")),
                        (3, "conversion.datetime_range", None),
                        (4, "conversion.invalid_datetime", Some("start")),
                    ],
                );
            }
        }
    }

    #[test]
    fn timezone_convert_fast_path_matches_generic_on_dst_transitions() {
        // In Europe/Rome le righe 5 (ambigua) e 6 (inesistente) di
        // `valid_edge_values` sono rifiutate: il confronto `Ok` usa le altre.
        let mut rome_valid = valid_edge_values();
        rome_valid.drain(5..7);
        let rome_valid = utf8_batch(rome_valid);
        let batch = utf8_batch(valid_edge_values());
        let with_invalid = utf8_batch(edge_values_with_invalid());
        {
            {
                let config = TimezoneConvert {
                    column: "ts".into(),
                    input_format: "%Y-%m-%d %H:%M:%S".into(),
                    output_format: "%Y-%m-%d %H:%M:%S".into(),
                    source_timezone: "Europe/Rome".into(),
                    target_timezone: "UTC".into(),
                    output_column: "out".into(),
                    invalid: None,
                    ambiguous: None,
                };
                assert_same_output(
                    timezone_convert(&rome_valid, &config),
                    generic_timezone_convert(&rome_valid, &config),
                );
                assert_equivalent(
                    timezone_convert(&batch, &config),
                    generic_timezone_convert(&batch, &config),
                );
                assert_rejected(
                    timezone_convert(&batch, &config),
                    &[
                        (5, "conversion.ambiguous_local_time", Some("ts")),
                        (6, "conversion.nonexistent_local_time", Some("ts")),
                    ],
                );
                assert_equivalent(
                    timezone_convert(&with_invalid, &config),
                    generic_timezone_convert(&with_invalid, &config),
                );
                assert_rejected(
                    timezone_convert(&with_invalid, &config),
                    &invalid_edge_rejections("conversion.invalid_datetime"),
                );
            }
        }
        // Coppia di timezone con DST diverse: le transizioni di Europe/Rome
        // sono ore ordinarie in America/New_York, quindi tutte valide.
        let config = TimezoneConvert {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%Y-%m-%d %H:%M:%S".into(),
            source_timezone: "America/New_York".into(),
            target_timezone: "Asia/Tokyo".into(),
            output_column: "out".into(),
            invalid: None,
            ambiguous: None,
        };
        assert_same_output(
            timezone_convert(&batch, &config),
            generic_timezone_convert(&batch, &config),
        );
        // Timezone non valida: stesso errore prima del loop.
        let bad = TimezoneConvert {
            source_timezone: "Marte/Olympus".into(),
            ..TimezoneConvert {
                column: "ts".into(),
                input_format: "%Y-%m-%d %H:%M:%S".into(),
                output_format: "%Y-%m-%d %H:%M:%S".into(),
                source_timezone: String::new(),
                target_timezone: "UTC".into(),
                output_column: "out".into(),
                invalid: None,
                ambiguous: None,
            }
        };
        assert_equivalent(
            timezone_convert(&batch, &bad),
            generic_timezone_convert(&batch, &bad),
        );
        assert!(matches!(
            timezone_convert(&batch, &bad),
            Err(PlenoraError::InvalidPlan(message)) if message == "source_timezone non valida"
        ));
    }

    fn add_one(value: &str, amount: i64, unit: DateUnit) -> Option<String> {
        let batch = utf8_batch(vec![Some(value)]);
        let config = DateAdd {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%Y-%m-%d %H:%M:%S".into(),
            amount,
            unit,
            output_column: "out".into(),
            invalid: None,
        };
        let fast = date_add(&batch, &config).expect("date_add");
        let generic = generic_date_add(&batch, &config).expect("oracolo");
        assert_eq!(fast, generic);
        fast.column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8")
            .iter()
            .next()
            .flatten()
            .map(ToOwned::to_owned)
    }

    #[test]
    fn date_add_hand_written_calendar_cases() {
        // Risultati scritti a mano dal calendario gregoriano prolettico, non
        // da `shift_months`: fine mese troncato all'ultimo giorno valido.
        let cases: [(&str, i64, u8, &str); 16] = [
            // Fine mese.
            ("2024-01-31 10:00:00", 1, 1, "2024-02-29 10:00:00"),
            ("2023-01-31 10:00:00", 1, 1, "2023-02-28 10:00:00"),
            ("2024-03-31 00:00:00", -1, 1, "2024-02-29 00:00:00"),
            ("2024-05-31 12:00:00", 1, 1, "2024-06-30 12:00:00"),
            // 29 febbraio verso anni non bisestili e bisestili.
            ("2024-02-29 08:30:00", 1, 0, "2025-02-28 08:30:00"),
            ("2024-02-29 08:30:00", -1, 0, "2023-02-28 08:30:00"),
            ("2024-02-29 08:30:00", 4, 0, "2028-02-29 08:30:00"),
            ("2000-02-29 00:00:00", 100, 0, "2100-02-28 00:00:00"),
            // Attraversamento di dicembre, in avanti e all'indietro.
            ("2023-12-15 10:00:00", 1, 1, "2024-01-15 10:00:00"),
            ("2024-01-15 10:00:00", -1, 1, "2023-12-15 10:00:00"),
            ("2023-12-31 23:59:59", 1, 6, "2024-01-01 00:00:00"),
            ("2024-01-01 00:00:00", -1, 3, "2023-12-31 00:00:00"),
            ("2023-12-28 00:00:00", 1, 2, "2024-01-04 00:00:00"),
            // Prima dell'epoch e anni negativi (anno 0 bisestile, -1 no).
            ("1969-12-31 23:59:59", 1, 6, "1970-01-01 00:00:00"),
            ("1900-03-01 00:00:00", -1, 3, "1900-02-28 00:00:00"),
            ("0000-03-01 00:00:00", -1, 3, "0000-02-29 00:00:00"),
        ];
        for (value, amount, unit, expected) in cases {
            assert_eq!(
                add_one(value, amount, date_unit(unit)).as_deref(),
                Some(expected),
                "{value} {amount} unita' {unit}"
            );
        }
        assert_eq!(
            add_one("0000-02-29 00:00:00", -1, date_unit(0)).as_deref(),
            Some("-0001-02-28 00:00:00")
        );
        assert_eq!(
            add_one("-0001-12-31 12:00:00", 12, date_unit(4)).as_deref(),
            Some("0000-01-01 00:00:00")
        );
    }

    #[test]
    fn timezone_convert_hand_written_rome_transitions() {
        // Europe/Rome 2024: il 31 marzo alle 02:00 CET (+01) si salta alle
        // 03:00 CEST (+02); il 27 ottobre alle 03:00 CEST si torna alle 02:00
        // CET. UTC attesi scritti a mano.
        let valid = utf8_batch(vec![
            Some("2024-03-31 01:59:59"), // ultimo secondo CET
            Some("2024-03-31 03:00:00"), // primo secondo CEST
            Some("2024-10-27 01:59:59"), // ultimo secondo univoco CEST
            Some("2024-10-27 03:00:00"), // primo secondo univoco CET
            Some("2024-07-01 12:00:00"),
            Some("2024-01-01 00:30:00"),
        ]);
        let config = TimezoneConvert {
            column: "ts".into(),
            input_format: "%Y-%m-%d %H:%M:%S".into(),
            output_format: "%Y-%m-%d %H:%M:%S".into(),
            source_timezone: "Europe/Rome".into(),
            target_timezone: "UTC".into(),
            output_column: "out".into(),
            invalid: None,
            ambiguous: None,
        };
        let output = timezone_convert(&valid, &config).expect("orari validi");
        assert_eq!(
            output
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8")
                .iter()
                .collect::<Vec<_>>(),
            vec![
                Some("2024-03-31 00:59:59"),
                Some("2024-03-31 01:00:00"),
                Some("2024-10-26 23:59:59"),
                Some("2024-10-27 02:00:00"),
                Some("2024-07-01 10:00:00"),
                Some("2023-12-31 23:30:00"),
            ]
        );
        assert_eq!(
            output,
            generic_timezone_convert(&valid, &config).expect("oracolo")
        );
        // Estremi delle due finestre: il salto [02:00, 03:00) non esiste,
        // la ripetizione [02:00, 03:00) e' ambigua.
        let edges = utf8_batch(vec![
            Some("2024-03-31 02:00:00"),
            Some("2024-03-31 02:59:59"),
            Some("2024-10-27 02:00:00"),
            Some("2024-10-27 02:59:59"),
        ]);
        assert_rejected(
            timezone_convert(&edges, &config),
            &[
                (0, "conversion.nonexistent_local_time", Some("ts")),
                (1, "conversion.nonexistent_local_time", Some("ts")),
                (2, "conversion.ambiguous_local_time", Some("ts")),
                (3, "conversion.ambiguous_local_time", Some("ts")),
            ],
        );
        // All'indietro: UTC verso Europe/Rome sugli stessi istanti.
        let reverse = TimezoneConvert {
            source_timezone: "UTC".into(),
            target_timezone: "Europe/Rome".into(),
            ..config
        };
        let back = timezone_convert(
            &utf8_batch(vec![
                Some("2024-03-31 00:59:59"),
                Some("2024-03-31 01:00:00"),
                Some("2024-10-27 00:59:59"),
                Some("2024-10-27 01:00:00"),
            ]),
            &reverse,
        )
        .expect("UTC sempre univoco");
        assert_eq!(
            back.column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8")
                .iter()
                .collect::<Vec<_>>(),
            vec![
                Some("2024-03-31 01:59:59"),
                Some("2024-03-31 03:00:00"),
                Some("2024-10-27 02:59:59"),
                Some("2024-10-27 02:00:00"),
            ]
        );
    }

    #[test]
    fn non_utf8_columns_fall_back_to_generic_path() {
        // Colonna Int64: nessun fast path, comportamento del generico.
        let columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(vec![Some(2_024), None]))];
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, true)])),
            columns,
        )
        .expect("fixture");
        let config = DateFormat {
            column: "n".into(),
            ..format_config(None)
        };
        let error = date_format(&batch, &config).expect_err("valore non temporale accettato");
        let report = error
            .row_diagnostics()
            .expect("diagnostica fallback mancante");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.examples[0].cause, "conversion.invalid_datetime");
    }
}
