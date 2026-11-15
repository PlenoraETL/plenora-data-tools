//! Colonne temporali a 64 bit lette come **intero nativo**: `Timestamp` di
//! ogni unita' (secondi, millisecondi, microsecondi, nanosecondi), con o
//! senza fuso, e `Date64` (millisecondi dall'epoca).
//!
//! Autorita' unica, fuori dalle operazioni su date (`temporale`), per chi
//! deve decidere su un istante senza passare dal testo: chiavi, confronti,
//! estremi, dominio numerico. Il valore e' quello scritto nella colonna,
//! nell'unita' della colonna: nessuna conversione verso un'unita' comune, che
//! renderebbe uguali due microsecondi distinti dello stesso millisecondo. Il
//! fuso non cambia l'istante (Arrow memorizza sempre l'istante UTC), quindi
//! non entra nel valore; entra solo nel testo
//! ([`crate::scalar_as_string`]).
//!
//! Quando due colonne di unita' diverse si confrontano (il comparatore del
//! sort, `compare_cells_typed`), il confronto passa dai nanosecondi in
//! `i128`: `|i64| * 10^9` sta largamente in `i128`, quindi e' esatto.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use plenora_core::arrow::array::{
    Array, Date64Array, TimestampMicrosecondArray, TimestampMillisecondArray,
    TimestampNanosecondArray, TimestampSecondArray,
};
use plenora_core::arrow::schema::{DataType, TimeUnit};

/// Millisecondi in un giorno: il passo di un `Date64` allineato.
pub const MILLISECONDI_AL_GIORNO: i64 = 86_400_000;

/// Colonna temporale a 64 bit, gia' risolta (downcast fatto una volta).
#[derive(Clone, Copy)]
pub enum InteriTemporali<'a> {
    /// `Timestamp(Second, _)`.
    Secondi(&'a TimestampSecondArray),
    /// `Timestamp(Millisecond, _)`.
    Millisecondi(&'a TimestampMillisecondArray),
    /// `Timestamp(Microsecond, _)`.
    Microsecondi(&'a TimestampMicrosecondArray),
    /// `Timestamp(Nanosecond, _)`.
    Nanosecondi(&'a TimestampNanosecondArray),
    /// `Date64`, in millisecondi.
    Date64(&'a Date64Array),
}

impl<'a> InteriTemporali<'a> {
    /// `None` se la colonna non e' un intero temporale, o se l'array non e'
    /// quello che il suo tipo dichiara (il chiamante ricade sul proprio
    /// errore di tipo).
    #[must_use]
    ///
    /// Precondizione Arrow: dati gia' validati con `plenora_core::valida_arrow`.
    /// Limite: `docs/limiti.md#helper-arrow-con-precondizione`; gli ingressi
    /// operativi validano i dati prima di chiamare questo helper.
    pub fn new(array: &'a dyn Array) -> Option<Self> {
        let any = array.as_any();
        match array.data_type() {
            DataType::Timestamp(TimeUnit::Second, _) => any
                .downcast_ref::<TimestampSecondArray>()
                .map(Self::Secondi),
            DataType::Timestamp(TimeUnit::Millisecond, _) => any
                .downcast_ref::<TimestampMillisecondArray>()
                .map(Self::Millisecondi),
            DataType::Timestamp(TimeUnit::Microsecond, _) => any
                .downcast_ref::<TimestampMicrosecondArray>()
                .map(Self::Microsecondi),
            DataType::Timestamp(TimeUnit::Nanosecond, _) => any
                .downcast_ref::<TimestampNanosecondArray>()
                .map(Self::Nanosecondi),
            DataType::Date64 => any.downcast_ref::<Date64Array>().map(Self::Date64),
            _ => None,
        }
    }

    /// `true` se la riga e' null.
    #[must_use]
    pub fn is_null(&self, row: usize) -> bool {
        match self {
            Self::Secondi(values) => values.is_null(row),
            Self::Millisecondi(values) => values.is_null(row),
            Self::Microsecondi(values) => values.is_null(row),
            Self::Nanosecondi(values) => values.is_null(row),
            Self::Date64(values) => values.is_null(row),
        }
    }

    /// Il valore nativo della riga, null ignorato (il chiamante l'ha gia'
    /// escluso).
    #[must_use]
    pub fn value(&self, row: usize) -> i64 {
        match self {
            Self::Secondi(values) => values.value(row),
            Self::Millisecondi(values) => values.value(row),
            Self::Microsecondi(values) => values.value(row),
            Self::Nanosecondi(values) => values.value(row),
            Self::Date64(values) => values.value(row),
        }
    }

    /// Il valore nativo della riga, `None` se null.
    #[must_use]
    pub fn valore(&self, row: usize) -> Option<i64> {
        (!self.is_null(row)).then(|| self.value(row))
    }

    /// L'istante della riga in nanosecondi dall'epoca, `None` se null:
    /// esatto per ogni unita' (`|i64| * 10^9` sta in `i128`).
    #[must_use]
    pub fn nanosecondi(&self, row: usize) -> Option<i128> {
        self.valore(row)
            .map(|valore| i128::from(valore) * self.nanosecondi_per_unita())
    }

    /// Unita' della colonna; `Date64` usa millisecondi.
    #[must_use]
    pub const fn unita(&self) -> TimeUnit {
        match self {
            Self::Secondi(_) => TimeUnit::Second,
            Self::Millisecondi(_) | Self::Date64(_) => TimeUnit::Millisecond,
            Self::Microsecondi(_) => TimeUnit::Microsecond,
            Self::Nanosecondi(_) => TimeUnit::Nanosecond,
        }
    }

    /// Nanosecondi in un'unita' della colonna (`Date64`: millisecondi).
    #[must_use]
    pub const fn nanosecondi_per_unita(&self) -> i128 {
        scala(self.unita())
    }
}

/// Ordine di due valori nativi di unita' anche diverse
/// (solo le quattro unita' Arrow): esatto, in nanosecondi `i128`.
#[must_use]
pub fn confronta(
    sinistra: i64,
    unita_sinistra: TimeUnit,
    destra: i64,
    unita_destra: TimeUnit,
) -> Ordering {
    if unita_sinistra == unita_destra {
        return sinistra.cmp(&destra);
    }
    // |i64| * 10^9 < 2^63 * 2^30 = 2^93: nessun trabocco in `i128`.
    (i128::from(sinistra) * scala(unita_sinistra)).cmp(&(i128::from(destra) * scala(unita_destra)))
}

const fn scala(unita: TimeUnit) -> i128 {
    match unita {
        TimeUnit::Second => 1_000_000_000,
        TimeUnit::Millisecond => 1_000_000,
        TimeUnit::Microsecond => 1_000,
        TimeUnit::Nanosecond => 1,
    }
}

/// Frammento di chiave di un istante, in nanosecondi dall'epoca: 32 cifre
/// esadecimali dell'`i128` col bit di segno invertito, quindi a larghezza
/// fissa e con l'ordine dei byte uguale all'ordine cronologico.
///
/// E' l'identita' di una cella `Timestamp` in ogni chiave testuale
/// (raggruppamenti, `distinct`, join, indici, partizioni): il testo RFC 3339
/// non lo e', perche' dipende dal fuso e non ogni offset ha una forma
/// RFC 3339 (`scalar_as_string`). Due istanti distinti hanno frammenti
/// distinti, lo stesso istante lo stesso frammento in ogni unita'.
#[must_use]
pub fn frammento_chiave(nanosecondi: i128) -> String {
    format!("{:032x}", nanosecondi.cast_unsigned() ^ (1_u128 << 127))
}

/// L'istante UTC di un valore nativo di `Timestamp` nell'unita' data;
/// `None` fuori dall'intervallo di chrono (aritmetica controllata).
#[must_use]
pub const fn istante(valore: i64, unita: TimeUnit) -> Option<DateTime<Utc>> {
    match unita {
        TimeUnit::Second => DateTime::from_timestamp(valore, 0),
        TimeUnit::Millisecond => DateTime::from_timestamp_millis(valore),
        TimeUnit::Microsecond => DateTime::from_timestamp_micros(valore),
        TimeUnit::Nanosecond => Some(DateTime::from_timestamp_nanos(valore)),
    }
}

#[deny(clippy::disallowed_methods)]
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use plenora_core::arrow::array::ArrayRef;

    use super::*;

    #[test]
    fn estremi_arrow_non_si_restringono_a_nanosecondi_i64() {
        for value in [i64::MIN, i64::MAX, -9_223_372_036_855, 9_223_372_036_855] {
            let array = TimestampMillisecondArray::from(vec![value]);
            let values = InteriTemporali::new(&array).unwrap();
            assert_eq!(values.nanosecondi(0), Some(i128::from(value) * 1_000_000));
        }
        for unit in [
            TimeUnit::Second,
            TimeUnit::Millisecond,
            TimeUnit::Microsecond,
        ] {
            assert_eq!(istante(i64::MIN, unit), None);
            assert_eq!(istante(i64::MAX, unit), None);
        }
        for value in [i64::MIN, i64::MAX] {
            assert_eq!(
                istante(value, TimeUnit::Nanosecond)
                    .unwrap()
                    .timestamp_nanos_opt(),
                Some(value)
            );
        }
        assert_eq!(
            confronta(
                i64::MAX,
                TimeUnit::Millisecond,
                i64::MAX,
                TimeUnit::Nanosecond
            ),
            Ordering::Greater
        );
        assert_eq!(
            confronta(
                i64::MIN,
                TimeUnit::Millisecond,
                i64::MIN,
                TimeUnit::Nanosecond
            ),
            Ordering::Less
        );
    }

    #[test]
    fn ogni_unita_si_legge_col_suo_valore_e_il_confronto_e_esatto() {
        let colonne: Vec<ArrayRef> = vec![
            Arc::new(TimestampSecondArray::from(vec![Some(1), None])),
            Arc::new(TimestampMillisecondArray::from(vec![Some(1_000), None])),
            Arc::new(
                TimestampMicrosecondArray::from(vec![Some(1_000_000), None])
                    .with_timezone("Europe/Rome"),
            ),
            Arc::new(TimestampNanosecondArray::from(vec![
                Some(1_000_000_000),
                None,
            ])),
            Arc::new(Date64Array::from(vec![Some(1_000), None])),
        ];
        for colonna in &colonne {
            let interi = InteriTemporali::new(colonna.as_ref()).expect("intero temporale");
            assert!(interi.is_null(1));
            assert_eq!(interi.valore(1), None);
            // Lo stesso istante, un secondo dopo l'epoca, in ogni unita'.
            let nanosecondi = i128::from(interi.value(0)) * interi.nanosecondi_per_unita();
            assert_eq!(nanosecondi, 1_000_000_000);
        }
        // Distinti in microsecondi, uguali in millisecondi: restano distinti.
        assert_eq!(
            confronta(
                1_000_001,
                TimeUnit::Microsecond,
                1_000_000,
                TimeUnit::Microsecond
            ),
            Ordering::Greater
        );
        assert_eq!(
            confronta(
                1_000,
                TimeUnit::Millisecond,
                1_000_001,
                TimeUnit::Microsecond
            ),
            Ordering::Less
        );
        assert_eq!(
            confronta(1, TimeUnit::Second, 1_000, TimeUnit::Millisecond),
            Ordering::Equal
        );
        assert_eq!(
            confronta(i64::MAX, TimeUnit::Second, i64::MIN, TimeUnit::Nanosecond),
            Ordering::Greater
        );
        assert_eq!(
            confronta(i64::MIN, TimeUnit::Second, i64::MAX, TimeUnit::Nanosecond),
            Ordering::Less
        );
        // Il frammento di chiave: iniettivo, a larghezza fissa, in ordine.
        let campioni = [
            i128::from(i64::MIN) * 1_000_000_000,
            -1,
            0,
            1,
            1_000,
            i128::from(i64::MAX) * 1_000_000_000,
        ];
        for coppia in campioni.windows(2) {
            let (a, b) = (frammento_chiave(coppia[0]), frammento_chiave(coppia[1]));
            assert_eq!(a.len(), 32);
            assert!(a < b, "{} < {}", coppia[0], coppia[1]);
        }
        let testo: ArrayRef = Arc::new(plenora_core::arrow::array::StringArray::from(vec!["x"]));
        assert!(InteriTemporali::new(testo.as_ref()).is_none());
        assert!(istante(i64::MAX, TimeUnit::Second).is_none());
        assert!(istante(i64::MAX, TimeUnit::Nanosecond).is_some());
    }
}
