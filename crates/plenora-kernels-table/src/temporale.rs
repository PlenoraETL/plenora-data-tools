//! Lettura dei valori temporali, autorita' unica dei kernel su date e ore.
//!
//! Due strade, mai mescolate:
//!
//! - una colonna **temporale** (`Date32`, `Timestamp` di ogni unita' —
//!   secondi, millisecondi, microsecondi, nanosecondi — con o senza fuso) si
//!   legge dal valore nativo, senza passare dal testo: il testo RFC 3339 che
//!   il profilo scalare scrive per un istante (`...+00:00`) non e' una forma
//!   che i formati di una data debbano indovinare;
//! - un **testo** si legge con il formato dichiarato o, senza formato, con i
//!   soli formati ISO 8601 di [`leggi_iso`]: l'ordine giorno/mese non si
//!   indovina mai (`01/02/2024` non e' una data di default).
//!
//! Il valore letto e' un [`Momento`]: l'ora locale (del fuso della colonna,
//! o quella scritta nel testo) e, quando si conosce, l'istante.

use chrono::format::{Fixed, Item, Parsed};
use chrono::{
    DateTime, NaiveDate, NaiveDateTime, Offset, SecondsFormat, TimeDelta, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;
use plenora_core::arrow::array::{
    Array, ArrayRef, Date32Array, TimestampMicrosecondArray, TimestampMillisecondArray,
    TimestampNanosecondArray, TimestampSecondArray,
};
use plenora_core::arrow::schema::{DataType, TimeUnit};
use plenora_core::{PlenoraError, Result};

/// Un valore temporale letto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Momento {
    /// L'ora locale: del fuso della colonna, o quella scritta nel testo
    /// (con un offset, l'ora prima dell'offset). Una data e' la sua
    /// mezzanotte.
    pub locale: NaiveDateTime,
    /// L'istante, quando si conosce: un `Timestamp` (senza fuso vale come
    /// UTC, la convenzione del profilo testuale) o un testo con offset. Una
    /// data o un testo senza offset non ne hanno.
    pub istante: Option<DateTime<Utc>>,
}

/// L'ora locale di un istante nel fuso `fuso`, con l'offset sommato in
/// aritmetica controllata: `None` se esce dall'intervallo di chrono.
///
/// `DateTime::naive_local` e la scrittura di un valore con fuso sommano
/// l'offset senza controllo e vanno in panico oltre `NaiveDateTime::MAX` (un
/// istante al massimo in un fuso a est di UTC): chi scrive o legge l'ora
/// locale di un istante passa di qui prima.
#[must_use]
pub fn ora_locale(istante: &DateTime<Utc>, fuso: Tz) -> Option<NaiveDateTime> {
    let utc = istante.naive_utc();
    let offset = fuso.offset_from_utc_datetime(&utc).fix().local_minus_utc();
    utc.checked_add_signed(TimeDelta::seconds(i64::from(offset)))
}

/// Il testo ha una frazione di secondo con cifre significative oltre il
/// nanosecondo.
///
/// chrono le legge e le **scarta** (RFC 3339 e `%.f`): `...00.0000000001Z`
/// diventerebbe l'epoca esatta, un valore diverso da quello scritto. Si
/// cerca prima della lettura ogni sequenza di cifre dopo un punto; le cifre
/// oltre la nona devono essere zeri.
#[must_use]
pub fn frazione_oltre_i_nanosecondi(testo: &str) -> bool {
    let byte = testo.as_bytes();
    let mut indice = 0;
    while indice < byte.len() {
        if byte[indice] == b'.' {
            let inizio = indice + 1;
            let mut fine = inizio;
            while fine < byte.len() && byte[fine].is_ascii_digit() {
                fine += 1;
            }
            if fine > inizio + 9 && byte[inizio + 9..fine].iter().any(|cifra| *cifra != b'0') {
                return true;
            }
            indice = fine;
        } else {
            indice += 1;
        }
    }
    false
}

/// La frazione di secondo che il formato legge con `%.f` o dentro `%+`
/// (le forme che chrono accetta di qualunque lunghezza, scartando le cifre
/// oltre il nanosecondo) ha cifre significative oltre la nona.
///
/// Si guarda solo il campo davvero letto: si leggono gli item che lo
/// precedono (`parse_and_remainder`) e si esamina la sequenza di cifre dopo
/// il punto da li'. Un `%f` a larghezza fissa non ne ha bisogno: cifre in
/// piu' non sono consumate e la lettura fallisce.
fn frazione_letta_oltre_i_nanosecondi(testo: &str, items: &[Item<'_>]) -> bool {
    items.iter().enumerate().any(|(indice, item)| {
        let rfc3339 = match item {
            Item::Fixed(Fixed::Nanosecond) => false,
            Item::Fixed(Fixed::RFC3339) => true,
            _ => return false,
        };
        let mut parsed = Parsed::new();
        let Ok(resto) =
            chrono::format::parse_and_remainder(&mut parsed, testo, items[..indice].iter())
        else {
            return false;
        };
        let frazione = if rfc3339 {
            // Solo la parte consumata da `%+`: un punto letterale dopo il
            // campo non e' la sua frazione.
            let mut dopo = Parsed::new();
            let Ok(resto_dopo) =
                chrono::format::parse_and_remainder(&mut dopo, testo, items[..=indice].iter())
            else {
                return false;
            };
            let consumato = &resto[..resto.len() - resto_dopo.len()];
            consumato.find('.').map(|punto| &consumato[punto..])
        } else {
            resto.starts_with('.').then_some(resto)
        };
        frazione.is_some_and(|frazione| {
            let cifre = frazione[1..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .collect::<Vec<_>>();
            cifre.len() > 9 && cifre[9..].iter().any(|cifra| *cifra != b'0')
        })
    })
}

/// L'offset di un istante nel suo fuso ha una forma RFC 3339, cioe' minuti
/// interi.
///
/// RFC 3339 scrive l'offset in ore e minuti, e chrono arrotonderebbe un
/// offset con i secondi (l'ora media locale di molti fusi prima dei fusi
/// standard, `America/Anchorage` fino al 1900: `-09:59:36`) al minuto piu'
/// vicino, lasciando l'ora locale esatta: il testo indicherebbe un altro
/// istante, e due istanti distinti potrebbero avere lo stesso testo.
///
/// # Errors
///
/// `Schema` per un offset con i secondi.
pub fn verifica_offset_rfc3339<T: TimeZone>(istante: &DateTime<T>) -> Result<()> {
    if istante.offset().fix().local_minus_utc() % 60 != 0 {
        return Err(PlenoraError::Schema(
            "offset del fuso con i secondi (ora media locale): nessuna forma RFC 3339".into(),
        ));
    }
    Ok(())
}

/// Un secondo intercalare (`23:59:60`): chrono lo rappresenta con i
/// nanosecondi oltre `10^9`, che un istante Arrow/POSIX non ha. Convertito,
/// cadrebbe sul secondo dopo: si rifiuta.
fn intercalare(locale: &NaiveDateTime) -> bool {
    locale.nanosecond() >= 1_000_000_000
}

/// Il tipo e' temporale e si legge nativamente: `Date32` o `Timestamp` di
/// qualunque unita', con o senza fuso.
#[must_use]
pub const fn tipo_temporale(data_type: &DataType) -> bool {
    matches!(data_type, DataType::Date32 | DataType::Timestamp(_, _))
}

/// Verifica sullo schema che una colonna temporale si possa leggere: un
/// fuso Arrow dev'essere un nome IANA.
///
/// # Errors
///
/// `Schema` con il nome della colonna per un fuso non riconosciuto.
pub fn verifica_tipo_temporale(data_type: &DataType, column: &str) -> Result<()> {
    if let DataType::Timestamp(_, Some(fuso)) = data_type {
        // Il fuso e' testo libero dello schema ricevuto: non entra nel
        // messaggio («errori senza dati»).
        fuso.parse::<Tz>().map_err(|_| {
            PlenoraError::Schema(format!(
                "colonna `{column}`: timezone Arrow del tipo non valida"
            ))
        })?;
    }
    Ok(())
}

/// Colonna temporale letta nativamente.
pub struct ColonnaTemporale<'a> {
    array: &'a ArrayRef,
    fuso: Option<Tz>,
}

impl<'a> ColonnaTemporale<'a> {
    /// `None` se la colonna non e' temporale.
    ///
    /// # Errors
    ///
    /// `Schema` per un fuso Arrow non valido.
    pub fn new(array: &'a ArrayRef) -> Result<Option<Self>> {
        let fuso = match array.data_type() {
            DataType::Timestamp(_, Some(fuso)) => Some(
                fuso.parse::<Tz>()
                    .map_err(|_| PlenoraError::Schema("timezone Arrow non valida".into()))?,
            ),
            DataType::Date32 | DataType::Timestamp(_, None) => None,
            _ => return Ok(None),
        };
        Ok(Some(Self { array, fuso }))
    }

    /// Il fuso della colonna, se ne ha uno.
    #[must_use]
    pub const fn fuso(&self) -> Option<Tz> {
        self.fuso
    }

    /// L'istante UTC della riga di un `Timestamp`, dall'unita' della
    /// colonna; `None` per una data.
    fn istante(&self, row: usize) -> Result<Option<DateTime<Utc>>> {
        let fuori = || PlenoraError::Schema("timestamp fuori intervallo".into());
        let any = self.array.as_any();
        let istante = match self.array.data_type() {
            DataType::Timestamp(TimeUnit::Second, _) => any
                .downcast_ref::<TimestampSecondArray>()
                .and_then(|valori| DateTime::from_timestamp(valori.value(row), 0)),
            DataType::Timestamp(TimeUnit::Millisecond, _) => any
                .downcast_ref::<TimestampMillisecondArray>()
                .and_then(|valori| DateTime::from_timestamp_millis(valori.value(row))),
            DataType::Timestamp(TimeUnit::Microsecond, _) => any
                .downcast_ref::<TimestampMicrosecondArray>()
                .and_then(|valori| DateTime::from_timestamp_micros(valori.value(row))),
            DataType::Timestamp(TimeUnit::Nanosecond, _) => any
                .downcast_ref::<TimestampNanosecondArray>()
                .map(|valori| DateTime::from_timestamp_nanos(valori.value(row))),
            _ => return Ok(None),
        };
        istante.map(Some).ok_or_else(fuori)
    }

    /// Il valore della riga, `None` se null.
    ///
    /// # Errors
    ///
    /// `Schema` per un valore fuori dall'intervallo di chrono o un array
    /// incoerente con il suo tipo.
    pub fn momento(&self, row: usize) -> Result<Option<Momento>> {
        if self.array.is_null(row) {
            return Ok(None);
        }
        if let Some(date) = self.array.as_any().downcast_ref::<Date32Array>() {
            let giorno = NaiveDate::from_ymd_opt(1970, 1, 1)
                .and_then(|epoca| {
                    epoca.checked_add_signed(TimeDelta::days(i64::from(date.value(row))))
                })
                .ok_or_else(|| PlenoraError::Schema("date32 fuori intervallo".into()))?;
            return Ok(Some(Momento {
                locale: giorno.and_time(chrono::NaiveTime::MIN),
                istante: None,
            }));
        }
        let istante = self
            .istante(row)?
            .ok_or_else(|| PlenoraError::Schema("colonna temporale incoerente".into()))?;
        let locale = match self.fuso {
            None => istante.naive_utc(),
            Some(fuso) => ora_locale(&istante, fuso).ok_or_else(|| {
                PlenoraError::Schema("timestamp fuori intervallo nel fuso della colonna".into())
            })?,
        };
        Ok(Some(Momento {
            locale,
            istante: Some(istante),
        }))
    }

    /// Il testo della riga, lo stesso del profilo scalare per `Date32` e
    /// `Timestamp` di ogni unita': la data `AAAA-MM-GG`, l'istante in
    /// RFC 3339 nel fuso della colonna (senza fuso, `+00:00`) con le cifre
    /// frazionarie che servono.
    ///
    /// # Errors
    ///
    /// Come [`ColonnaTemporale::momento`]; `Schema` per un offset con i
    /// secondi ([`verifica_offset_rfc3339`]).
    pub fn testo(&self, row: usize) -> Result<Option<String>> {
        let Some(momento) = self.momento(row)? else {
            return Ok(None);
        };
        // `momento` ha gia' verificato che l'ora locale sta nell'intervallo:
        // la scrittura con fuso non va in panico.
        let Some(istante) = momento.istante else {
            return Ok(Some(momento.locale.format("%Y-%m-%d").to_string()));
        };
        let Some(fuso) = self.fuso else {
            return Ok(Some(istante.to_rfc3339_opts(SecondsFormat::AutoSi, false)));
        };
        let locale = istante.with_timezone(&fuso);
        verifica_offset_rfc3339(&locale)?;
        Ok(Some(locale.to_rfc3339_opts(SecondsFormat::AutoSi, false)))
    }
}

/// Lettura di un testo con i formati di **default**, solo ISO 8601:
///
/// 1. RFC 3339 con offset (`2024-01-31T10:00:00Z`,
///    `2024-01-31 10:00:00.123+01:00`): l'ora scritta e l'istante;
/// 2. data e ora senza offset, con `T` o con uno spazio, secondi
///    obbligatori e frazione facoltativa (`2024-01-31T10:00:00`,
///    `2024-01-31 10:00:00.5`);
/// 3. data sola (`2024-01-31`), a mezzanotte.
///
/// Nessun formato con giorno e mese in un ordine da indovinare: un testo
/// come `01/02/2024` si legge solo con un formato esplicito. Una frazione
/// oltre il nanosecondo e un secondo intercalare si rifiutano.
#[must_use]
pub fn leggi_iso(testo: &str) -> Option<Momento> {
    if frazione_oltre_i_nanosecondi(testo) {
        return None;
    }
    leggi_iso_senza_controlli(testo).filter(|momento| !intercalare(&momento.locale))
}

fn leggi_iso_senza_controlli(testo: &str) -> Option<Momento> {
    if let Ok(valore) = DateTime::parse_from_rfc3339(testo) {
        return Some(Momento {
            locale: valore.naive_local(),
            istante: Some(valore.with_timezone(&Utc)),
        });
    }
    for formato in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(locale) = NaiveDateTime::parse_from_str(testo, formato) {
            return Some(Momento {
                locale,
                istante: None,
            });
        }
    }
    NaiveDate::parse_from_str(testo, "%Y-%m-%d")
        .ok()
        .map(|giorno| Momento {
            locale: giorno.and_time(chrono::NaiveTime::MIN),
            istante: None,
        })
}

/// Lettura di un testo con un formato **esplicito** (item compilati): l'ora
/// scritta e, se il formato legge un offset (`%z`, `%:z`, `%+`), l'istante.
///
/// Senza campi orari la data si pone a mezzanotte; con campi orari che non
/// bastano a un'ora (`%H` senza `%M`, `%I` senza `%p`) il testo si rifiuta,
/// invece di perdere l'ora letta. Con `%s` il timestamp e' l'istante (UTC,
/// con o senza offset), e l'ora locale e' quella dell'offset letto. Una
/// frazione oltre il nanosecondo e un secondo intercalare si rifiutano.
#[must_use]
pub fn leggi_con_items(testo: &str, items: &[Item<'_>]) -> Option<Momento> {
    leggi_con_items_e_ora(testo, items).map(|(momento, _)| momento)
}

/// Come [`leggi_con_items`], con in piu' se il formato ha letto un'ora
/// (campi orari o `%s`): un formato di sola data non basta a un target che
/// chiede l'ora.
#[must_use]
pub fn leggi_con_items_e_ora(testo: &str, items: &[Item<'_>]) -> Option<(Momento, bool)> {
    if frazione_letta_oltre_i_nanosecondi(testo, items) {
        return None;
    }
    let mut parsed = Parsed::new();
    chrono::format::parse(&mut parsed, testo, items.iter()).ok()?;
    let offset = parsed.offset().unwrap_or(0);
    let campi_orari = parsed.hour_div_12().is_some()
        || parsed.hour_mod_12().is_some()
        || parsed.minute().is_some()
        || parsed.second().is_some()
        || parsed.nanosecond().is_some();
    let locale = match parsed.to_naive_datetime_with_offset(offset) {
        Ok(locale) => locale,
        Err(_) if !campi_orari && parsed.timestamp().is_none() => parsed
            .to_naive_date()
            .ok()?
            .and_time(chrono::NaiveTime::MIN),
        Err(_) => return None,
    };
    if intercalare(&locale) {
        return None;
    }
    // Con `%s` l'istante e' il timestamp letto, sempre UTC. Altrimenti e'
    // l'ora scritta meno l'offset letto, se c'e': vale anche per un formato
    // di sola data con offset (mezzanotte di quel giorno).
    let istante = if let Some(secondi) = parsed.timestamp() {
        Some(DateTime::from_timestamp(
            secondi,
            parsed.nanosecond().unwrap_or(0),
        )?)
    } else {
        match parsed.offset() {
            Some(offset) => Some(
                locale
                    .checked_sub_signed(TimeDelta::seconds(i64::from(offset)))?
                    .and_utc(),
            ),
            None => None,
        }
    };
    let ha_ora = campi_orari || parsed.timestamp().is_some();
    Some((Momento { locale, istante }, ha_ora))
}

/// Il testo di un'ora locale nel formato del target `datetime` di
/// `table.type_cast`: `AAAA-MM-GGTHH:MM:SS`, piu' la frazione di secondo
/// quando non e' zero (tre, sei o nove cifre). La frazione non si tronca: un
/// istante al millisecondo resta al millisecondo.
#[must_use]
pub fn testo_datetime(locale: &NaiveDateTime) -> String {
    locale.format("%Y-%m-%dT%H:%M:%S%.f").to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn il_fuso_non_valido_non_compare_nei_messaggi() {
        // Il fuso del tipo e' testo libero dello schema ricevuto.
        let tipo = DataType::Timestamp(TimeUnit::Second, Some("SEGRETO".into()));
        for errore in [
            verifica_tipo_temporale(&tipo, "t").expect_err("fuso non valido"),
            crate::validate_text_convertible(&tipo, "t").expect_err("fuso non valido"),
        ] {
            let testo = errore.to_string();
            assert!(testo.contains("timezone"), "{testo}");
            assert!(!testo.contains("SEGRETO"), "{testo}");
            assert!(
                !errore.public_projection().message().contains("SEGRETO"),
                "{testo}"
            );
        }
    }

    #[test]
    fn i_formati_di_default_sono_solo_iso() {
        let momento = |testo: &str| leggi_iso(testo).map(|momento| momento.locale.to_string());
        assert_eq!(momento("2024-01-31"), Some("2024-01-31 00:00:00".into()));
        assert_eq!(
            momento("2024-01-31T10:20:30"),
            Some("2024-01-31 10:20:30".into())
        );
        assert_eq!(
            momento("2024-01-31 10:20:30.5"),
            Some("2024-01-31 10:20:30.500".into())
        );
        // L'offset si legge: l'ora resta quella scritta, l'istante c'e'.
        let con_offset = leggi_iso("2024-01-31T10:20:30.123+01:00").expect("rfc 3339");
        assert_eq!(con_offset.locale.to_string(), "2024-01-31 10:20:30.123");
        assert_eq!(
            con_offset.istante.map(|istante| istante.timestamp_millis()),
            Some(1_706_692_830_123)
        );
        assert!(leggi_iso("2024-01-31T10:20:30Z").is_some());
        // Giorno e mese in un ordine da indovinare: nessun default.
        for testo in [
            "31/01/2024",
            "01/02/2024",
            "31-01-2024",
            "2024/01/31",
            "31/01/2024 10:00:00",
        ] {
            assert_eq!(leggi_iso(testo), None, "{testo}");
        }
    }

    /// Regressioni: un'ora incompleta si rifiuta
    /// invece di diventare mezzanotte; con `%s` e un offset l'istante e' il
    /// timestamp, non il timestamp meno l'offset.
    #[test]
    fn i_formati_espliciti_non_perdono_l_ora_ne_spostano_l_istante() {
        let leggi = |testo: &str, formato: &str| {
            leggi_con_items(
                testo,
                &chrono::format::StrftimeItems::new(formato).collect::<Vec<_>>(),
            )
        };
        assert_eq!(leggi("2024-01-31 10", "%Y-%m-%d %H"), None);
        assert_eq!(leggi("2024-01-31 10:30", "%Y-%m-%d %I:%M"), None);
        assert!(leggi("2024-01-31", "%Y-%m-%d").is_some());
        for (testo, locale) in [
            ("0 +0100", "1970-01-01 01:00:00"),
            ("0 +0000", "1970-01-01 00:00:00"),
        ] {
            let momento = leggi(testo, "%s %z").expect("timestamp con offset");
            assert_eq!(momento.istante.map(|istante| istante.timestamp()), Some(0));
            assert_eq!(momento.locale.to_string(), locale);
        }
    }

    /// Regressioni: frazione oltre il nanosecondo,
    /// secondo intercalare, `%s` senza offset, ora locale oltre il massimo.
    #[test]
    fn frazioni_intercalari_timestamp_e_limiti() {
        fn items(formato: &str) -> Vec<Item<'_>> {
            chrono::format::StrftimeItems::new(formato).collect()
        }
        assert!(leggi_iso("1970-01-01T00:00:00.0000000001Z").is_none());
        assert!(leggi_iso("1970-01-01T00:00:00.1234567890Z").is_some());
        assert!(leggi_iso("1970-01-01 00:00:00.0000000001").is_none());
        assert!(leggi_iso("2016-12-31T23:59:60Z").is_none());
        assert!(leggi_con_items("2016-12-31 23:59:60", &items("%Y-%m-%d %H:%M:%S")).is_none());
        assert!(leggi_con_items("10:00:00.0000000001", &items("%H:%M:%S%.f")).is_none());
        assert!(leggi_con_items("2024-01-31T10:00:00.0000000001Z", &items("%+")).is_none());
        // Un punto letterale dopo `%+` non e' la frazione del campo.
        assert!(
            leggi_con_items("2024-01-31T10:00:00Z .1234567891", &items("%+ .1234567891")).is_some()
        );
        // Un punto che non precede la frazione letta non conta: `%f` a nove
        // cifre dopo un punto letterale.
        let valido = leggi_con_items("2024-01-31.123456123456789", &items("%Y-%m-%d.%H%M%S%f"))
            .expect("12:34:56.123456789");
        assert_eq!(valido.locale.to_string(), "2024-01-31 12:34:56.123456789");
        let momento = leggi_con_items("0", &items("%s")).expect("%s");
        assert_eq!(momento.istante.map(|istante| istante.timestamp()), Some(0));
        let (coerente, ha_ora) = leggi_con_items_e_ora(
            "1970-01-01 01:00:00 0 +0100",
            &items("%Y-%m-%d %H:%M:%S %s %z"),
        )
        .expect("campi e timestamp coerenti");
        assert!(ha_ora);
        assert_eq!(coerente.istante.map(|istante| istante.timestamp()), Some(0));
        // Un istante al massimo in un fuso a est di UTC: errore, non panico.
        let massimo: ArrayRef = Arc::new(
            TimestampMillisecondArray::from(vec![NaiveDateTime::MAX.and_utc().timestamp_millis()])
                .with_timezone("Etc/GMT-1"),
        );
        let lettore = ColonnaTemporale::new(&massimo)
            .expect("fuso")
            .expect("temporale");
        assert!(matches!(lettore.momento(0), Err(PlenoraError::Schema(_))));
        assert!(lettore.testo(0).is_err());
    }

    #[test]
    fn ogni_unita_di_timestamp_si_legge_senza_testo() {
        let millisecondi = 1_706_696_430_123_i64;
        let colonne: Vec<ArrayRef> = vec![
            Arc::new(TimestampSecondArray::from(vec![millisecondi / 1000])),
            Arc::new(TimestampMillisecondArray::from(vec![millisecondi])),
            Arc::new(TimestampMicrosecondArray::from(vec![millisecondi * 1000])),
            Arc::new(TimestampNanosecondArray::from(vec![
                millisecondi * 1_000_000,
            ])),
        ];
        for colonna in &colonne {
            let lettore = ColonnaTemporale::new(colonna)
                .expect("fuso valido")
                .expect("temporale");
            let momento = lettore
                .momento(0)
                .expect("in intervallo")
                .expect("non null");
            assert_eq!(
                momento.locale.format("%Y-%m-%d %H:%M:%S").to_string(),
                "2024-01-31 10:20:30"
            );
        }
        // Con fuso: l'ora locale del fuso, lo stesso istante.
        let roma: ArrayRef = Arc::new(
            TimestampMicrosecondArray::from(vec![millisecondi * 1000]).with_timezone("Europe/Rome"),
        );
        let lettore = ColonnaTemporale::new(&roma)
            .expect("fuso")
            .expect("temporale");
        let momento = lettore.momento(0).expect("ok").expect("valore");
        assert_eq!(momento.locale.to_string(), "2024-01-31 11:20:30.123");
        assert_eq!(
            lettore.testo(0).expect("ok").as_deref(),
            Some("2024-01-31T11:20:30.123+01:00")
        );
    }
}
