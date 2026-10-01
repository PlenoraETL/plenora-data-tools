use std::sync::Arc;

use chrono::format::{Fixed, Item, Numeric, StrftimeItems};
use chrono::{
    DateTime, Datelike, LocalResult, Months, NaiveDate, NaiveDateTime, TimeDelta, TimeZone,
};
use chrono_tz::Tz;
use plenora_core::arrow::array::{Array, ArrayRef, Float64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use serde::Deserialize;

use crate::temporale::{ColonnaTemporale, Momento};
use crate::utility::InvalidDatePolicy;
use crate::{column_index, reject_rows, replace_or_append, scalar_as_string, RowRejection};
use plenora_core::{PlenoraError, Result};

fn default_output_format() -> String {
    "%Y-%m-%d %H:%M:%S".into()
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

/// La regola di `input_format` delle operazioni su date.
///
/// Del kernel e dell'analisi: obbligatorio per una colonna letta come
/// testo, rifiutato per una colonna temporale (`Date32`, `Timestamp` di ogni unita'), che si
/// legge dal valore nativo e non ha un testo da interpretare.
///
/// # Errors
///
/// `InvalidPlan` per `input_format` assente con un testo o scritto con una
/// colonna temporale; gli errori di [`validate_format_items`].
pub fn verifica_input_format(temporale: bool, input_format: Option<&str>) -> Result<()> {
    match (temporale, input_format) {
        (true, Some(_)) => Err(PlenoraError::InvalidPlan(
            "input_format legge un testo e non si applica a una colonna temporale".into(),
        )),
        (false, None) => Err(PlenoraError::InvalidPlan(
            "input_format obbligatorio per una colonna letta come testo".into(),
        )),
        (false, Some(formato)) => validate_format_items(formato, "input_format"),
        (true, None) => Ok(()),
    }
}

/// Una cella letta da [`Lettore::momento`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum Letto {
    /// Cella nulla.
    Nullo,
    /// Testo che il formato non legge.
    Illeggibile,
    /// Valore letto.
    Valore(Momento),
}

/// Lettura della colonna d'ingresso di `date_format`, `date_add`,
/// `date_diff` e `timezone_convert`: una colonna temporale dal valore
/// nativo (l'ora locale del suo fuso e l'istante), un testo con
/// `input_format` (l'ora scritta e, se il formato legge un offset,
/// l'istante).
pub(crate) enum Lettore<'a> {
    /// Colonna temporale.
    Temporale(ColonnaTemporale<'a>),
    /// Colonna letta come testo, con gli item del formato.
    Testo {
        array: &'a ArrayRef,
        items: Vec<Item<'a>>,
    },
}

impl<'a> Lettore<'a> {
    /// # Errors
    ///
    /// [`verifica_input_format`]; `Schema` per un fuso Arrow non valido.
    pub(crate) fn new(array: &'a ArrayRef, input_format: Option<&'a str>) -> Result<Self> {
        let temporale = ColonnaTemporale::new(array)?;
        verifica_input_format(temporale.is_some(), input_format)?;
        Ok(match (temporale, input_format) {
            (Some(temporale), _) => Self::Temporale(temporale),
            (None, formato) => Self::Testo {
                array,
                items: compile_items(formato.unwrap_or_default()),
            },
        })
    }

    /// Il valore della riga ([`Letto`]).
    ///
    /// # Errors
    ///
    /// Gli errori di lettura della colonna (`scalar_as_string`, valore
    /// temporale fuori intervallo).
    pub(crate) fn momento(&self, row: usize) -> Result<Letto> {
        let letto = |momento: Option<Momento>| momento.map_or(Letto::Illeggibile, Letto::Valore);
        match self {
            Self::Temporale(temporale) => {
                Ok(temporale.momento(row)?.map_or(Letto::Nullo, Letto::Valore))
            }
            Self::Testo { array, items } => {
                if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
                    if values.is_null(row) {
                        return Ok(Letto::Nullo);
                    }
                    return Ok(letto(crate::temporale::leggi_con_items(
                        values.value(row),
                        items,
                    )));
                }
                Ok(
                    scalar_as_string(array.as_ref(), row)?.map_or(Letto::Nullo, |valore| {
                        letto(crate::temporale::leggi_con_items(&valore, items))
                    }),
                )
            }
        }
    }

    /// Colonna temporale con un fuso: il suo valore e' un istante.
    pub(crate) const fn fuso_della_colonna(&self) -> Option<Tz> {
        match self {
            Self::Temporale(temporale) => temporale.fuso(),
            Self::Testo { .. } => None,
        }
    }
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
/// numerico al posto delle decine, gli anni fuori dall'intervallo di
/// RFC 2822 e l'offset del fuso piu' fine di quanto il formato lo scriva
/// ([`FormatoUscita::offset_esatto`]).
///
/// La validazione guarda solo la struttura del formato: l'offset e' una
/// proprieta' della cella (dell'istante e del fuso), non della config, e
/// non si controlla su un istante di prova. `Australia/Adelaide` vale
/// +09:00 nel 1896 e +10:30 nel 2000: con `%:::z` la prima cella si scrive
/// esatta, la seconda si rifiuta.
pub(crate) struct FormatoUscita<'a> {
    items: Vec<Item<'a>>,
    /// Il formato scrive il secolo civile a due cifre (`%C`, anno / 100).
    secolo_civile: bool,
    /// Il formato scrive il secolo dell'anno ISO a due cifre (item chrono
    /// `IsoYearDiv100`, senza specificatore strftime ma costruibile).
    secolo_iso: bool,
    /// Granularita' piu' grossa, in secondi, con cui il formato scrive
    /// l'offset del fuso: 60 per `%z`, `%:z`, `%#z`, `%+` e RFC 2822 (ore e
    /// minuti), 3600 per `%:::z` (solo ore), 1 se lo scrive coi secondi
    /// (`%::z`) o non lo scrive. chrono arrotonda un offset piu' fine al
    /// valore piu' vicino: un offset con i secondi (ora media locale) o i
    /// minuti scritto cosi' indicherebbe un altro istante.
    passo_offset: i32,
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
        let passo_offset = items
            .iter()
            .map(|item| match item {
                Item::Fixed(
                    Fixed::TimezoneOffset
                    | Fixed::TimezoneOffsetZ
                    | Fixed::TimezoneOffsetColon
                    | Fixed::TimezoneOffsetColonZ
                    | Fixed::RFC2822
                    | Fixed::RFC3339,
                ) => 60,
                Item::Fixed(Fixed::TimezoneOffsetTripleColon) => 3600,
                _ => 1,
            })
            .max()
            .unwrap_or(1);
        Ok(Self {
            items,
            secolo_civile,
            secolo_iso,
            passo_offset,
        })
    }

    /// Istante di prova per scoprire, prima dei dati, gli item che chrono
    /// non sa scrivere per il tipo del valore: il risultato non dipende
    /// dall'istante se non per gli anni, e il 2000 e' dentro ogni intervallo.
    /// Solo controlli strutturali: niente che dipenda dai dati si controlla
    /// su questo istante (l'offset del fuso, [`FormatoUscita::offset_esatto`]).
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

    /// Per valori con il fuso `fuso`. L'offset del campione non si
    /// controlla: e' quello del 2000, non quello delle celle.
    pub(crate) fn con_fuso(format: &'a str, fuso: Tz) -> Result<Self> {
        let formato = Self::compila(format)?;
        let campione = fuso.from_utc_datetime(&Self::campione()?);
        if formato.scrivi_istante(&campione).is_err() {
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

    /// L'offset del fuso di `valore` si scrive esatto con questo formato:
    /// e' un multiplo della granularita' con cui il formato lo scrive
    /// (`passo_offset`). Altrimenti chrono lo arrotonderebbe e il testo
    /// indicherebbe un altro istante. Si controlla per cella, in esecuzione.
    pub(crate) fn offset_esatto(&self, valore: &DateTime<Tz>) -> bool {
        chrono::Offset::fix(valore.offset()).local_minus_utc() % self.passo_offset == 0
    }

    /// Scrive `valore`; un offset non esatto ([`Self::offset_esatto`]) e'
    /// un errore, mai un testo arrotondato. Chi scrive per riga lo
    /// controlla prima, per una diagnostica per riga.
    pub(crate) fn scrivi_con_fuso(&self, valore: &DateTime<Tz>) -> Result<String> {
        if !self.offset_esatto(valore) {
            return Err(PlenoraError::DataMapping(
                "offset del fuso piu' fine di quanto output_format lo scriva: \
                 il testo indicherebbe un altro istante"
                    .into(),
            ));
        }
        self.scrivi_istante(valore)
    }

    /// Scrive `valore` senza il controllo dell'offset: la struttura del
    /// formato e i limiti di anno.
    fn scrivi_istante(&self, valore: &DateTime<Tz>) -> Result<String> {
        // L'ora locale oltre l'intervallo di chrono: `year()` e la scrittura
        // andrebbero in panico sommando l'offset senza controllo.
        crate::temporale::ora_locale(&valore.with_timezone(&chrono::Utc), valore.timezone())
            .ok_or_else(|| {
                PlenoraError::DataMapping(
                    "valore temporale fuori intervallo nel fuso d'uscita".into(),
                )
            })?;
        self.controlla_secolo(valore.year(), valore.iso_week().year())?;
        scrivi_formattato(valore.format_with_items(self.items.iter()))
    }
}

/// Byte scritti al massimo da un valore formattato con `format`.
///
/// I letterali contano per la loro lunghezza, ogni campo per la sua
/// larghezza massima ([`byte_massimi_campo`]): un limite superiore esatto
/// per campo, non una stima, quindi non rifiuta un formato che non puo'
/// superarlo (`%Y%m` scrive al piu' 9 byte).
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
            Item::Numeric(campo, _) => byte_massimi_numerico(&campo),
            Item::Fixed(campo) => byte_massimi_campo(&campo),
            Item::Error => 0,
        };
        totale.saturating_add(byte)
    })
}

/// Larghezza massima di un campo numerico di chrono. Gli anni vanno da
/// -262143 a +262143 (`NaiveDate`), con il segno fuori da 0..=9999; il
/// riempimento (`Pad`) non supera mai queste larghezze.
#[allow(clippy::match_same_arms)] // Un braccio per famiglia di campi, con il suo motivo.
const fn byte_massimi_numerico(campo: &Numeric) -> usize {
    match campo {
        Numeric::Year | Numeric::IsoYear => 7,
        // Il secolo si scrive solo per anni in 0..=9999 (`FormatoUscita`
        // rifiuta gli altri): due cifre.
        Numeric::YearDiv100 | Numeric::IsoYearDiv100 => 2,
        Numeric::Quarter | Numeric::NumDaysFromSun | Numeric::WeekdayFromMon => 1,
        Numeric::Ordinal => 3,
        Numeric::Nanosecond => 9,
        // Secondi dall'epoca di un `NaiveDateTime`: al piu' 14 cifre e il
        // segno.
        Numeric::Timestamp => 20,
        Numeric::YearMod100
        | Numeric::IsoYearMod100
        | Numeric::Month
        | Numeric::Day
        | Numeric::WeekFromSun
        | Numeric::WeekFromMon
        | Numeric::IsoWeek
        | Numeric::Hour
        | Numeric::Hour12
        | Numeric::Minute
        | Numeric::Second => 2,
        // Varianti interne o future: per eccesso.
        _ => BYTE_PER_CAMPO,
    }
}

/// Larghezza massima di un campo testuale di chrono.
#[allow(clippy::match_same_arms)] // Un braccio per famiglia di campi, con il suo motivo.
fn byte_massimi_campo(campo: &Fixed) -> usize {
    match campo {
        Fixed::ShortMonthName | Fixed::ShortWeekdayName => 3,
        // `September`, `Wednesday`.
        Fixed::LongMonthName | Fixed::LongWeekdayName => 9,
        Fixed::LowerAmPm | Fixed::UpperAmPm => 2,
        // `.123456789`.
        Fixed::Nanosecond | Fixed::Nanosecond9 => 10,
        Fixed::Nanosecond3 => 4,
        Fixed::Nanosecond6 => 7,
        // `+0530`, `+05:30`, `+05:30:00`, `+05`.
        Fixed::TimezoneOffset | Fixed::TimezoneOffsetZ => 5,
        Fixed::TimezoneOffsetColon | Fixed::TimezoneOffsetColonZ => 6,
        Fixed::TimezoneOffsetDoubleColon => 9,
        Fixed::TimezoneOffsetTripleColon => 3,
        // Abbreviazioni dei fusi di chrono-tz (`CEST`, `+0530`, `LMT`): per
        // eccesso, la larghezza varia con il fuso.
        Fixed::TimezoneName => 32,
        Fixed::RFC2822 | Fixed::RFC3339 => 48,
        // Varianti interne (`%3f`, `%6f`, `%9f` sono i nanosecondi senza
        // punto): hanno larghezza fissa, che si misura scrivendo un valore
        // qualunque; se chrono non le sa scrivere, per eccesso.
        altro => larghezza_misurata(altro).unwrap_or(BYTE_PER_CAMPO),
    }
}

/// Larghezza di un campo a larghezza fissa, misurata su un valore
/// campione; `None` se chrono non lo sa scrivere senza fuso.
fn larghezza_misurata(campo: &Fixed) -> Option<usize> {
    use std::fmt::Write as _;
    let campione = NaiveDate::from_ymd_opt(2024, 1, 1)?.and_hms_nano_opt(1, 2, 3, 4)?;
    let mut testo = String::new();
    write!(
        testo,
        "{}",
        campione.format_with_items(std::iter::once(Item::Fixed(campo.clone())))
    )
    .ok()?;
    Some(testo.len())
}

/// Larghezza di un campo che chrono non elenca (varianti interne o nuove):
/// per eccesso.
pub const BYTE_PER_CAMPO: usize = 64;

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
    /// Colonna da leggere: una colonna temporale (`Date32`, `Timestamp` di
    /// ogni unita') dal valore nativo, ogni altra come testo.
    pub column: String,
    /// Formato strftime di lettura di un testo; deve consumare tutto il
    /// testo, e senza campi orari la data si pone a mezzanotte. Obbligatorio
    /// per un testo, rifiutato per una colonna temporale
    /// ([`verifica_input_format`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub input_format: Option<String>,
    /// Formato strftime di scrittura, senza campi di fuso; default
    /// `%Y-%m-%d %H:%M:%S`.
    #[serde(default = "default_output_format")]
    pub output_format: String,
    /// Colonna d'uscita (`Utf8` nullable).
    pub output_column: String,
    /// Non ammesso: un valore non leggibile rifiuta sempre la riga, quindi
    /// nessuna politica avrebbe effetto. Scritto si rifiuta
    /// ([`verifica_politiche`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
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
/// Una colonna temporale (`Date32`, `Timestamp` di ogni unita', con o senza
/// fuso) si legge dal valore nativo, senza `input_format`: si scrive l'ora
/// locale della colonna (del suo fuso; senza fuso, il valore com'e'). Un
/// testo si legge con `input_format` (item strftime precompilati; con un
/// offset, si scrive l'ora scritta). I valori non parsabili sono sempre
/// rifiutati con diagnostica row-scoped; `invalid` scritto si rifiuta
/// ([`verifica_politiche`]).
///
/// # Errors
///
/// - `InvalidPlan`: formato con item non riconosciuti, o `output_format`
///   con item di fuso o di offset; `input_format` assente per un testo o
///   scritto per una colonna temporale ([`verifica_input_format`]);
/// - `DataMapping`: uno o piu' valori non parsabili, con row diagnostics
///   (`conversion.invalid_datetime`); senza diagnostica, un valore che
///   `output_format` non sa scrivere (secolo `%C` fuori da 0..=9999);
/// - `Schema`: colonna assente (come `column_index`) o valore non
///   convertibile in testo (come `scalar_as_string`); gli errori di
///   `replace_or_append`.
pub fn date_format(batch: &RecordBatch, config: &DateFormat) -> Result<RecordBatch> {
    verifica_politiche(config.invalid.as_ref(), None)?;
    let uscita = FormatoUscita::senza_fuso(&config.output_format)?;
    let index = column_index(batch, &config.column)?;
    let lettore = Lettore::new(batch.column(index), config.input_format.as_deref())?;
    let celle = (0..batch.num_rows())
        .map(|row| lettore.momento(row))
        .collect::<Result<Vec<_>>>()?;
    let rejections = celle
        .iter()
        .enumerate()
        .filter(|(_, letto)| matches!(letto, Letto::Illeggibile))
        .map(|(row, _)| RowRejection {
            row,
            cause: "conversion.invalid_datetime",
            column: Some(&config.column),
        })
        .collect::<Vec<_>>();
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
    let values = celle
        .iter()
        .map(|letto| match letto {
            Letto::Nullo => Ok(None),
            Letto::Valore(momento) => uscita.scrivi_senza_fuso(&momento.locale).map(Some),
            Letto::Illeggibile => invalid("date_format"),
        })
        .collect::<Result<Vec<_>>>()?;
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
    /// Colonna da leggere: una colonna temporale dal valore nativo, ogni
    /// altra come testo.
    pub column: String,
    /// Formato strftime di lettura di un testo; deve consumare tutto il
    /// testo. Obbligatorio per un testo, rifiutato per una colonna temporale
    /// ([`verifica_input_format`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub input_format: Option<String>,
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
    #[serde(default, deserialize_with = "crate::mai_null")]
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
    // Gli estremi dei valori leggibili: il secondo intercalare (`23:59:60`)
    // si rifiuta in lettura (`temporale`), quindi gli estremi sono quelli di
    // `NaiveDateTime`. Si rifiuta solo cio' che fallisce da entrambi.
    let estremi = [NaiveDateTime::MIN, NaiveDateTime::MAX];
    if estremi
        .into_iter()
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
/// sempre rifiutati con diagnostica row-scoped. Una colonna temporale si
/// legge dal valore nativo (l'ora locale del suo fuso), un testo con
/// `input_format` (l'ora scritta).
///
/// # Errors
///
/// - `InvalidPlan`: formato con item non riconosciuti, o `output_format`
///   con item di fuso o di offset; `input_format` assente per un testo o
///   scritto per una colonna temporale;
/// - `DataMapping`: valore non parsabile (`conversion.invalid_datetime`),
///   oppure data risultante o delta fuori range
///   (`conversion.datetime_range`), con row diagnostics; senza diagnostica,
///   un valore che `output_format` non sa scrivere;
/// - `Schema`: colonna assente (come `column_index`) o valore non
///   convertibile in testo (come `scalar_as_string`); gli errori di
///   `replace_or_append`.
pub fn date_add(batch: &RecordBatch, config: &DateAdd) -> Result<RecordBatch> {
    verifica_politiche(config.invalid.as_ref(), None)?;
    let uscita = FormatoUscita::senza_fuso(&config.output_format)?;
    let index = column_index(batch, &config.column)?;
    let lettore = Lettore::new(batch.column(index), config.input_format.as_deref())?;
    // Delta precomputato per le unita' a durata fissa (mai ricalcolato per
    // riga); anni e mesi restano sull'aritmetica di calendario per riga.
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
    let mut rejections = Vec::new();
    let mut valori_spostati = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let risultato = match lettore.momento(row)? {
            Letto::Nullo => None,
            Letto::Illeggibile => {
                rejections.push(RowRejection {
                    row,
                    cause: "conversion.invalid_datetime",
                    column: Some(&config.column),
                });
                None
            }
            Letto::Valore(momento) => {
                let dopo = shift_row(momento.locale);
                if dopo.is_none() {
                    rejections.push(RowRejection {
                        row,
                        cause: "conversion.datetime_range",
                        column: Some(&config.column),
                    });
                }
                dopo
            }
        };
        valori_spostati.push(risultato);
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
    let values = valori_spostati
        .iter()
        .map(|risultato| {
            risultato
                .map(|valore| uscita.scrivi_senza_fuso(&valore))
                .transpose()
        })
        .collect::<Result<Vec<_>>>()?;
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
    /// Colonna dell'istante iniziale: temporale dal valore nativo, ogni
    /// altra come testo.
    pub start_column: String,
    /// Colonna dell'istante finale, come `start_column`.
    pub end_column: String,
    /// Formato strftime di lettura dei testi: obbligatorio per colonne di
    /// testo, rifiutato per colonne temporali ([`verifica_input_format`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub input_format: Option<String>,
    /// Unita' della differenza.
    pub unit: DiffUnit,
    /// Colonna d'uscita (`Float64` nullable).
    pub output_column: String,
    /// Non ammesso: un valore non leggibile rifiuta sempre la riga, quindi
    /// nessuna politica avrebbe effetto. Scritto si rifiuta
    /// ([`verifica_politiche`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
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
fn diff_value(delta: TimeDelta, divisor: f64, _row: usize) -> Result<f64> {
    delta
        .num_nanoseconds()
        .map(|nanoseconds| nanoseconds as f64 / 1_000_000_000.0 / divisor)
        .ok_or_else(|| PlenoraError::InvalidPlan("date_diff: intervallo fuori scala".into()))
}

/// Differenza `end_column - start_column` in unita' frazionarie
/// (`unit`), scritta come Float64 in `output_column`.
///
/// Due istanti (colonne `Timestamp` di ogni unita', o testi con un offset)
/// si sottraggono come istanti; date e ore senza offset come ore locali.
/// Un estremo null propaga null; un estremo non parsabile o un intervallo
/// fuori scala rifiuta sempre l'output con diagnostica row-scoped.
///
/// # Errors
///
/// - `InvalidPlan`: `input_format` con item non riconosciuti, assente per
///   un testo o scritto per colonne temporali; colonne di generi diversi
///   ([`verifica_generi_date_diff`]);
/// - `DataMapping`: valore non parsabile (`conversion.invalid_datetime`,
///   sulla colonna iniziale se e' quella a non leggersi) oppure intervallo
///   fuori scala, cioe' nanosecondi oltre `i64`
///   (`conversion.datetime_range`), con row diagnostics;
/// - `Schema`: colonna assente (come `column_index`) o valore non
///   convertibile in testo (come `scalar_as_string`); gli errori di
///   `replace_or_append`.
pub fn date_diff(batch: &RecordBatch, config: &DateDiff) -> Result<RecordBatch> {
    verifica_politiche(config.invalid.as_ref(), None)?;
    let start_index = column_index(batch, &config.start_column)?;
    let end_index = column_index(batch, &config.end_column)?;
    verifica_generi_date_diff(
        batch.column(start_index).data_type(),
        batch.column(end_index).data_type(),
    )?;
    let divisor = match config.unit {
        DiffUnit::Days => 86_400.0,
        DiffUnit::Hours => 3_600.0,
        DiffUnit::Minutes => 60.0,
        DiffUnit::Seconds => 1.0,
    };
    let inizio = Lettore::new(batch.column(start_index), config.input_format.as_deref())?;
    let fine = Lettore::new(batch.column(end_index), config.input_format.as_deref())?;
    let mut rejections = Vec::new();
    let mut coppie = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let (cause, column) = match (inizio.momento(row)?, fine.momento(row)?) {
            (Letto::Nullo, _) | (_, Letto::Nullo) => {
                coppie.push(None);
                continue;
            }
            (Letto::Illeggibile, _) => (
                "conversion.invalid_datetime",
                Some(config.start_column.as_str()),
            ),
            (_, Letto::Illeggibile) => (
                "conversion.invalid_datetime",
                Some(config.end_column.as_str()),
            ),
            (Letto::Valore(start), Letto::Valore(end)) => match durata(&start, &end) {
                Some(delta) if delta.num_nanoseconds().is_some() => {
                    coppie.push(Some(delta));
                    continue;
                }
                _ => ("conversion.datetime_range", None),
            },
        };
        coppie.push(None);
        rejections.push(RowRejection { row, cause, column });
    }
    reject_rows(
        &rejections,
        "valori temporali rifiutati; consultare row_diagnostics",
    )?;
    let values = coppie
        .iter()
        .enumerate()
        .map(|(row, delta)| {
            delta
                .map(|delta| diff_value(delta, divisor, row))
                .transpose()
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Float64,
        true,
        Arc::new(Float64Array::from(values)),
    )
}

/// Le due colonne di `date_diff` sono dello stesso genere.
///
/// Entrambe istanti (`Timestamp`), entrambe date (`Date32`), o entrambe
/// testo. Un istante e
/// una data non hanno una differenza scritta: si rifiuta. La chiamano il
/// kernel e l'analisi.
///
/// # Errors
///
/// `InvalidPlan` per colonne di generi diversi.
pub fn verifica_generi_date_diff(inizio: &DataType, fine: &DataType) -> Result<()> {
    let genere = |tipo: &DataType| match tipo {
        DataType::Timestamp(_, _) => 1,
        DataType::Date32 => 2,
        _ => 0,
    };
    if genere(inizio) != genere(fine) {
        return Err(PlenoraError::InvalidPlan(
            "date_diff: start_column e end_column devono essere entrambe istanti, date o testo"
                .into(),
        ));
    }
    Ok(())
}

/// La durata `end - start`: fra gli istanti se entrambi ne hanno uno (due
/// `Timestamp`, due testi con offset), fra le ore locali altrimenti. `None`
/// se uno solo ha l'istante (irraggiungibile con colonne dello stesso
/// genere e lo stesso formato).
fn durata(start: &Momento, end: &Momento) -> Option<TimeDelta> {
    match (start.istante, end.istante) {
        (Some(start), Some(end)) => Some(end.signed_duration_since(start)),
        (None, None) => Some(end.locale.signed_duration_since(start.locale)),
        _ => None,
    }
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
    /// Colonna da leggere: una colonna temporale dal valore nativo, ogni
    /// altra come testo.
    pub column: String,
    /// Formato strftime di lettura di un testo; il valore letto e' ora
    /// locale di `source_timezone`, salvo un offset letto dal formato, che
    /// da' l'istante. Obbligatorio per un testo, rifiutato per una colonna
    /// temporale ([`verifica_input_format`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub input_format: Option<String>,
    /// Formato strftime di scrittura, anche con campi di fuso; default
    /// `%Y-%m-%d %H:%M:%S`.
    #[serde(default = "default_output_format")]
    pub output_format: String,
    /// Fuso dei valori letti, nome IANA (`chrono-tz`); con una colonna
    /// `Timestamp` con fuso dev'essere quel fuso
    /// ([`verifica_fuso_sorgente`]).
    pub source_timezone: String,
    /// Fuso dei valori scritti, nome IANA (`chrono-tz`).
    pub target_timezone: String,
    /// Colonna d'uscita (`Utf8` nullable).
    pub output_column: String,
    /// Non ammesso: un valore non leggibile rifiuta sempre la riga, quindi
    /// nessuna politica avrebbe effetto. Scritto si rifiuta
    /// ([`verifica_politiche`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub invalid: Option<InvalidDatePolicy>,
    /// Non ammesso, come `invalid` ([`verifica_politiche`]).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub ambiguous: Option<AmbiguousPolicy>,
}

/// Converte la colonna `column` da `source_timezone` a
/// `target_timezone`, riscritta con `output_format` nella colonna
/// `output_column`.
///
/// Un valore con un istante (colonna `Timestamp` con fuso, che dev'essere
/// `source_timezone`; testo con un offset letto da `input_format`) si
/// converte dall'istante; ogni altro (testo senza offset, `Date32` a
/// mezzanotte, `Timestamp` senza fuso) e' l'ora locale di
/// `source_timezone`.
///
/// Ore ambigue/inesistenti e valori non parsabili sono sempre rifiutati con
/// diagnostica row-scoped, senza scelta o null sintetico: `ambiguous` e
/// `invalid` scritti si rifiutano ([`verifica_politiche`]).
///
/// # Errors
///
/// - `InvalidPlan`: `source_timezone` o `target_timezone` non valida, o
///   `source_timezone` diversa dal fuso della colonna; formato con item non
///   riconosciuti o che non si sa scrivere per un valore con fuso;
///   `input_format` assente per un testo o scritto per una colonna
///   temporale;
/// - `DataMapping`: ora ambigua (`conversion.ambiguous_local_time`) o
///   inesistente (`conversion.nonexistent_local_time`), valore non
///   parsabile (`conversion.invalid_datetime`) o offset del fuso d'arrivo
///   piu' fine di quanto `output_format` lo scriva
///   (`conversion.offset_precision`), con row diagnostics; senza
///   diagnostica, un valore che `output_format` non sa scrivere;
/// - `Schema`: colonna assente (come `column_index`) o valore non
///   convertibile in testo (come `scalar_as_string`); gli errori di
///   `replace_or_append`.
pub fn timezone_convert(batch: &RecordBatch, config: &TimezoneConvert) -> Result<RecordBatch> {
    verifica_politiche(config.invalid.as_ref(), config.ambiguous.as_ref())?;
    let index = column_index(batch, &config.column)?;
    let source_tz: Tz = config
        .source_timezone
        .parse()
        .map_err(|_| PlenoraError::InvalidPlan("source_timezone non valida".into()))?;
    let target_tz: Tz = config
        .target_timezone
        .parse()
        .map_err(|_| PlenoraError::InvalidPlan("target_timezone non valida".into()))?;
    verifica_fuso_sorgente(batch.column(index).data_type(), source_tz)?;
    let lettore = Lettore::new(batch.column(index), config.input_format.as_deref())?;
    let uscita = FormatoUscita::con_fuso(&config.output_format, target_tz)?;
    // L'istante di ogni riga: quello del valore se lo ha (colonna con fuso,
    // testo con offset), altrimenti l'ora locale in `source_timezone`, che
    // nel cambio d'ora puo' essere ambigua o inesistente.
    let colonna_con_fuso = lettore.fuso_della_colonna().is_some();
    let testo = matches!(lettore, Lettore::Testo { .. });
    let mut rejections = Vec::new();
    let mut convertiti = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let istante = match lettore.momento(row)? {
            Letto::Nullo => {
                convertiti.push(None);
                continue;
            }
            Letto::Illeggibile => Err("conversion.invalid_datetime"),
            Letto::Valore(Momento {
                istante: Some(noto),
                ..
            }) if colonna_con_fuso || testo => Ok(noto),
            Letto::Valore(momento) => match source_tz.from_local_datetime(&momento.locale) {
                LocalResult::Single(valore) => Ok(valore.with_timezone(&chrono::Utc)),
                LocalResult::Ambiguous(_, _) => Err("conversion.ambiguous_local_time"),
                LocalResult::None => Err("conversion.nonexistent_local_time"),
            },
        };
        // L'offset d'arrivo e' della cella, non della config: un offset che
        // `output_format` scriverebbe arrotondato rifiuta la riga.
        let cause = match istante {
            Ok(istante) if uscita.offset_esatto(&istante.with_timezone(&target_tz)) => {
                convertiti.push(Some(istante));
                continue;
            }
            Ok(_) => "conversion.offset_precision",
            Err(cause) => cause,
        };
        convertiti.push(None);
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
    let values = convertiti
        .iter()
        .map(|convertito| {
            convertito
                .map(|valore| uscita.scrivi_con_fuso(&valore.with_timezone(&target_tz)))
                .transpose()
        })
        .collect::<Result<Vec<_>>>()?;
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

/// Con una colonna `Timestamp` con fuso il valore e' un istante in quel
/// fuso: un `source_timezone` diverso direbbe un'altra cosa e sarebbe
/// ignorato, quindi si rifiuta. La chiamano il kernel e l'analisi.
///
/// # Errors
///
/// `InvalidPlan` se la colonna ha un fuso diverso da `source_timezone`;
/// `Schema` per un fuso Arrow non valido.
pub fn verifica_fuso_sorgente(data_type: &DataType, source_timezone: Tz) -> Result<()> {
    if let DataType::Timestamp(_, Some(fuso)) = data_type {
        let fuso: Tz = fuso
            .parse()
            .map_err(|_| PlenoraError::Schema("timezone Arrow non valida".into()))?;
        if fuso != source_timezone {
            return Err(PlenoraError::InvalidPlan(
                "timezone_convert: source_timezone diverso dal fuso della colonna".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{ArrayRef, Int64Array};
    use plenora_core::arrow::schema::{Field, Schema};
    use plenora_core::diagnostics::RowDiagnosticsCompleteness;

    use super::*;
    use crate::test_support::{assert_same_outcome as assert_equivalent, single_column_batch};

    /// Il limite di `byte_massimi_scritti` e' esatto per ogni specificatore:
    /// nessun valore scritto lo supera (date estreme, ogni mese e giorno
    /// della settimana, fusi con offset di minuti, secondi storici e nomi), e
    /// per gli specificatori a larghezza fissa il limite e' la larghezza
    /// osservata, non una stima.
    #[test]
    fn byte_massimi_scritti_e_un_limite_superiore_esatto_per_campo() {
        use std::fmt::Write as _;
        assert_eq!(byte_massimi_scritti("%Y%m"), 9);
        assert_eq!(byte_massimi_scritti("%3f"), 3);
        assert_eq!(byte_massimi_scritti("%:z"), 6);
        let mut valori = vec![NaiveDateTime::MIN, NaiveDateTime::MAX];
        for mese in 1..=12 {
            for giorno in [1, 2, 3, 4, 5, 6, 7, 28] {
                let data = NaiveDate::from_ymd_opt(2024, mese, giorno).expect("data");
                valori.push(data.and_hms_nano_opt(23, 59, 59, 999_999_999).expect("ora"));
                valori.push(data.and_hms_opt(0, 0, 0).expect("ora"));
            }
        }
        valori.push(
            NaiveDate::from_ymd_opt(1850, 1, 1)
                .and_then(|data| data.and_hms_opt(12, 0, 0))
                .expect("data storica"),
        );
        let fusi: Vec<Tz> = [
            "Asia/Kolkata",
            "America/St_Johns",
            "Pacific/Chatham",
            "Europe/Amsterdam",
            "Pacific/Kiritimati",
            "UTC",
        ]
        .iter()
        .map(|nome| nome.parse().expect("fuso"))
        .collect();
        // Specificatori a larghezza fissa: il limite deve essere raggiunto.
        let fissi = [
            "%C", "%y", "%m", "%b", "%h", "%d", "%e", "%a", "%w", "%u", "%U", "%W", "%G", "%g",
            "%V", "%j", "%D", "%x", "%F", "%v", "%H", "%k", "%I", "%l", "%P", "%p", "%M", "%S",
            "%3f", "%6f", "%9f", "%.3f", "%.6f", "%.9f", "%R", "%T", "%X", "%r", "%z", "%:z",
            "%::z", "%:::z", "%B", "%A", "%Y",
        ];
        let variabili = ["%f", "%.f", "%s", "%Z", "%c", "%+", "%t", "%n", "%%"];
        let mut difetti = Vec::new();
        for formato in fissi.iter().chain(variabili.iter()) {
            let limite = byte_massimi_scritti(formato);
            let mut massimo = 0;
            for valore in &valori {
                for fuso in &fusi {
                    let mut testo = String::new();
                    if write!(testo, "{}", fuso.from_utc_datetime(valore).format(formato)).is_ok() {
                        massimo = massimo.max(testo.len());
                    }
                }
            }
            if massimo > limite {
                difetti.push(format!("{formato}: scritto {massimo} > limite {limite}"));
            }
            if fissi.contains(formato) && massimo != limite {
                difetti.push(format!(
                    "{formato}: limite {limite} non stretto ({massimo})"
                ));
            }
        }
        assert!(
            difetti.is_empty(),
            "{}",
            difetti.join(
                "
"
            )
        );
    }

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

    /// Lettura di riferimento di un testo con un formato, indipendente da
    /// `temporale::leggi_con_items`: `parse_from_str` di chrono, data e ora
    /// e poi sola data a mezzanotte (l'ora scritta, offset scartato).
    fn parse(value: &str, format: &str) -> Option<NaiveDateTime> {
        NaiveDateTime::parse_from_str(value, format)
            .ok()
            .or_else(|| {
                NaiveDate::parse_from_str(value, format)
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
            })
    }

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
            // Il testo con il suo formato, la colonna Date32 senza (si legge
            // dal valore nativo).
            for (batch, ingresso) in [
                (date_testuali(&["2020-01-01"]), Some("%Y-%m-%d")),
                (date_native(), None),
            ] {
                // `input_format` si omette per la colonna temporale: `null`
                // si rifiuta come ogni parametro facoltativo.
                let con_ingresso = |mut valore: serde_json::Value| {
                    if let Some(ingresso) = ingresso {
                        valore["input_format"] = serde_json::json!(ingresso);
                    }
                    valore
                };
                let esito = date_format(
                    &batch,
                    &config(con_ingresso(serde_json::json!({
                        "column": "d", "output_format": formato, "output_column": "o"}))),
                );
                assert!(
                    matches!(esito, Err(PlenoraError::InvalidPlan(_))),
                    "date_format {formato}: {esito:?}"
                );
                let esito = date_add(
                    &batch,
                    &config(con_ingresso(serde_json::json!({
                        "column": "d", "amount": 1,
                        "unit": "days", "output_format": formato, "output_column": "o"}))),
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

    /// Regressione (difetto 3): le operazioni su date leggono le colonne
    /// temporali dal valore nativo, per ogni unita' e fuso. Prima un
    /// `Timestamp` passava dal suo testo RFC 3339 (`...+00:00`), che un
    /// formato come `%Y-%m-%d %H:%M:%S` non legge, e un `Timestamp` in
    /// microsecondi non si leggeva affatto. Valori attesi scritti a mano.
    #[test]
    #[allow(clippy::too_many_lines, clippy::float_cmp)] // Valori scritti a mano, esatti.
    fn le_operazioni_su_date_leggono_le_colonne_temporali_senza_testo() {
        use plenora_core::arrow::array::{Date32Array, TimestampMicrosecondArray};
        let micro = 1_706_696_430_123_456_i64; // 2024-01-31T10:20:30.123456Z
        let colonna = |fuso: Option<&str>| -> RecordBatch {
            let array = TimestampMicrosecondArray::from(vec![Some(micro), None])
                .with_timezone_opt(fuso.map(ToOwned::to_owned));
            let tipo = array.data_type().clone();
            single_column_batch("d", Arc::new(array), tipo, true)
        };
        let testo = |batch: &RecordBatch| -> Vec<Option<String>> {
            batch
                .column_by_name("o")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .expect("o")
                .iter()
                .map(|valore| valore.map(ToOwned::to_owned))
                .collect()
        };
        let formato = date_format(
            &colonna(Some("Europe/Rome")),
            &config(serde_json::json!({
                "column": "d", "output_format": "%Y-%m-%d %H:%M:%S%.6f", "output_column": "o"})),
        )
        .expect("date_format nativo");
        assert_eq!(
            testo(&formato),
            vec![Some("2024-01-31 11:20:30.123456".to_owned()), None]
        );
        let aggiunto = date_add(
            &colonna(None),
            &config(serde_json::json!({
                "column": "d", "amount": 1, "unit": "days", "output_column": "o"})),
        )
        .expect("date_add nativo");
        assert_eq!(
            testo(&aggiunto),
            vec![Some("2024-02-01 10:20:30".to_owned()), None]
        );
        let convertito = timezone_convert(
            &colonna(Some("Europe/Rome")),
            &config(serde_json::json!({
                "column": "d", "source_timezone": "Europe/Rome",
                "target_timezone": "UTC", "output_column": "o"})),
        )
        .expect("timezone_convert nativo");
        assert_eq!(
            testo(&convertito),
            vec![Some("2024-01-31 10:20:30".to_owned()), None]
        );
        // Un source_timezone diverso dal fuso della colonna si rifiuta.
        assert!(matches!(
            timezone_convert(
                &colonna(Some("Europe/Rome")),
                &config(serde_json::json!({
                    "column": "d", "source_timezone": "UTC",
                    "target_timezone": "UTC", "output_column": "o"})),
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
        // input_format con una colonna temporale: rifiutato, non ignorato;
        // senza, con un testo: obbligatorio.
        assert!(matches!(
            date_format(
                &colonna(None),
                &config(serde_json::json!({
                    "column": "d", "input_format": "%Y-%m-%d", "output_column": "o"})),
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
        assert!(matches!(
            date_format(
                &date_testuali(&["2024-01-31"]),
                &config(serde_json::json!({"column": "d", "output_column": "o"})),
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
        // date_diff: due istanti si sottraggono come istanti, due date come
        // giorni; un istante e una data no.
        let due = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("a", DataType::Date32, true),
                Field::new("b", DataType::Date32, true),
                Field::new(
                    "t",
                    DataType::Timestamp(
                        plenora_core::arrow::schema::TimeUnit::Microsecond,
                        Some("Europe/Rome".into()),
                    ),
                    true,
                ),
            ])),
            vec![
                Arc::new(Date32Array::from(vec![19_753])),
                Arc::new(Date32Array::from(vec![19_755])),
                Arc::new(TimestampMicrosecondArray::from(vec![micro]).with_timezone("Europe/Rome")),
            ],
        )
        .expect("batch");
        let giorni = date_diff(
            &due,
            &config(serde_json::json!({
                "start_column": "a", "end_column": "b", "unit": "days", "output_column": "o"})),
        )
        .expect("date_diff su date");
        assert_eq!(
            giorni
                .column_by_name("o")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
                .expect("o")
                .value(0),
            2.0
        );
        assert!(matches!(
            date_diff(
                &due,
                &config(serde_json::json!({
                    "start_column": "a", "end_column": "t", "unit": "days", "output_column": "o"})),
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }

    /// Regressione: con un formato che legge l'offset, `date_diff` sottrae gli
    /// istanti. Prima l'offset si scartava: 10:00+01:00 e 10:00+02:00
    /// davano zero ore invece di -1.
    #[test]
    #[allow(clippy::float_cmp)] // -1 ora e' esatto in f64.
    fn date_diff_sottrae_gli_istanti_dei_testi_con_offset() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("s", DataType::Utf8, true),
                Field::new("e", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["2024-01-31 10:00:00 +01:00"])),
                Arc::new(StringArray::from(vec!["2024-01-31 10:00:00 +02:00"])),
            ],
        )
        .expect("batch");
        let uscita = date_diff(
            &batch,
            &config(serde_json::json!({
                "start_column": "s", "end_column": "e", "input_format": "%Y-%m-%d %H:%M:%S %:z",
                "unit": "hours", "output_column": "o"})),
        )
        .expect("date_diff");
        assert_eq!(
            uscita
                .column_by_name("o")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
                .expect("o")
                .value(0),
            -1.0
        );
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
            passo_offset: 1,
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
                Ok(
                    parse(&value, config.input_format.as_deref().unwrap_or_default())
                        .map(|value| Some(value.format(&config.output_format).to_string()))
                        .ok_or(("conversion.invalid_datetime", Some(config.column.as_str()))),
                )
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
                let Some(value) = parse(&value, config.input_format.as_deref().unwrap_or_default())
                else {
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
                let Some(start) = parse(&start, config.input_format.as_deref().unwrap_or_default())
                else {
                    return Ok(Err((
                        "conversion.invalid_datetime",
                        Some(config.start_column.as_str()),
                    )));
                };
                let Some(end) = parse(&end, config.input_format.as_deref().unwrap_or_default())
                else {
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
                let Some(parsed) =
                    parse(&value, config.input_format.as_deref().unwrap_or_default())
                else {
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
            input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
            input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
                input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
            input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
                input_format: Some("%Y-%m-%d".into()),
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
                    input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
                input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
                    input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
                    input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
            input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
                input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
            input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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
            input_format: Some("%Y-%m-%d %H:%M:%S".into()),
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

    /// Regressione: l'offset d'arrivo e' della cella, non della config.
    /// `Australia/Adelaide` vale +09:00 dal 1895 al 1899 e +10:30 d'estate
    /// nel 2000. Prima la validazione scriveva un istante di prova del 2000
    /// e rifiutava `%:::z` (solo ore) per ogni dato, anche per il 1896, che
    /// si scrive esatto. Ora la validazione accetta, il 1896 si scrive e il
    /// 2000 rifiuta la sua riga. Valori attesi scritti a mano.
    #[test]
    fn l_offset_d_arrivo_si_controlla_per_cella_non_in_validazione() {
        use plenora_core::contract::{DataContract, FieldAllocator};

        let json = serde_json::json!({
            "column": "ts", "input_format": "%Y-%m-%d %H:%M:%S",
            "output_format": "%Y-%m-%d %H:%M %:::z", "source_timezone": "UTC",
            "target_timezone": "Australia/Adelaide", "output_column": "out"});
        let config: TimezoneConvert = config(json.clone());
        let ottocento = utf8_batch(vec![Some("1896-01-01 00:00:00"), None]);
        let analisi = crate::analyze::analyze_table_contract(
            "table.timezone_convert",
            &[DataContract::tabular(ottocento.schema())],
            &json,
            &mut FieldAllocator::default(),
            &crate::Limits::default(),
        );
        assert!(analisi.is_ok(), "{analisi:?}");
        let output = timezone_convert(&ottocento, &config).expect("offset a ore intere");
        assert_eq!(
            output
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8")
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("1896-01-01 09:00 +09"), None]
        );
        // Il 2000 (+10:30) rifiuta solo la sua riga; con `%::z` (coi
        // secondi) si scrive.
        let misto = utf8_batch(vec![
            Some("1896-01-01 00:00:00"),
            Some("2000-01-01 00:00:00"),
            None,
        ]);
        assert_rejected(
            timezone_convert(&misto, &config),
            &[(1, "conversion.offset_precision", Some("ts"))],
        );
        let esatto = timezone_convert(
            &misto,
            &TimezoneConvert {
                output_format: "%Y-%m-%d %H:%M %::z".into(),
                ..config
            },
        )
        .expect("offset coi secondi");
        assert_eq!(
            esatto
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8")
                .iter()
                .collect::<Vec<_>>(),
            vec![
                Some("1896-01-01 09:00 +09:00:00"),
                Some("2000-01-01 10:30 +10:30:00"),
                None
            ]
        );
        // La validazione resta strutturale: un item che chrono non scrive si
        // rifiuta prima dei dati, qualunque sia il fuso.
        for formato in ["%Q", "%Y-%"] {
            let mut json = json.clone();
            json["output_format"] = serde_json::json!(formato);
            let analisi = crate::analyze::analyze_table_contract(
                "table.timezone_convert",
                &[DataContract::tabular(ottocento.schema())],
                &json,
                &mut FieldAllocator::default(),
                &crate::Limits::default(),
            );
            assert!(
                matches!(analisi, Err(PlenoraError::InvalidPlan(_))),
                "{formato}: {analisi:?}"
            );
        }
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
