use std::cmp::Ordering;

use plenora_core::arrow::array::{
    types::Int32Type, Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    DictionaryArray, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
    UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use plenora_core::{PlenoraError, Result};

use crate::compare_decimal128_values;
#[cfg(test)]
use crate::scalar_as_string;

#[cfg(test)] // Solo i test-oracolo usano il percorso testuale originale.
pub fn row_key(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<String> {
    let mut key = String::new();
    for index in indices {
        let value = scalar_as_string(batch.column(*index).as_ref(), row)?;
        key.push_str(batch.column(*index).data_type().to_string().as_str());
        key.push('\u{1e}');
        match value {
            Some(value) => {
                key.push('1');
                key.push_str(&value.len().to_string());
                key.push(':');
                key.push_str(&value);
            }
            None => key.push('0'),
        }
        key.push('\u{1f}');
    }
    Ok(key)
}

/// Confronto tipizzato tra due celle: il contratto d'ordine di `table.sort`,
/// `table.top_n`, del merge dello spill e dei ranghi di
/// `table.window_function`.
///
/// Semantica: null dopo i valori (uguaglianza tra null); confronto nel
/// dominio NATIVO di ogni tipo supportato, mai sulla forma testuale.
///
/// Un ripiego testuale sbaglierebbe l'ordine di `Decimal128` ("10" prima di
/// "9"), dei `Timestamp` con timezone (ora locale, non istante UTC) e
/// fallirebbe sui `Binary` non UTF-8. I tipi fuori dal profilo scalare sono
/// rifiutati esplicitamente, invece di ricevere un ordine arbitrario.
///
/// # Errors
///
/// `PlenoraError::Schema` se un indice di riga e' fuori dall'array, se un
/// tipo non ha un confronto nativo definito, se i due tipi non appartengono
/// alla stessa famiglia di confronto, se un Decimal128 e' incoerente con il
/// proprio schema o se una chiave dictionary non e' risolvibile.
///
/// Questi controlli precedono la decisione sui null: due celle nulle di tipi
/// non confrontabili sono un errore, non `Equal`.
///
/// I siti d'uso sono `compare_at` (stesso batch; lo usa anche il ramo
/// generico di `ColumnComparator`, i cui rami tipizzati riproducono la
/// stessa semantica), `OrdineNumerico` dei ranghi e il merge k-way dello
/// spill (`spill::compare_cells`, batch diversi: da qui la forma a due
/// array).
// Dispatch lineare per tipo Arrow: spezzarlo renderebbe piu' difficile
// verificare che ogni tipo sia trattato una volta sola.
#[allow(clippy::too_many_lines)]
pub fn compare_cells_typed(
    left: &ArrayRef,
    left_row: usize,
    right: &ArrayRef,
    right_row: usize,
) -> Result<Ordering> {
    // Prima il dominio, poi i null: decidendo sui null in testa, due celle
    // nulle di tipi incompatibili darebbero `Equal` invece di `Schema`, e la
    // stessa coppia di colonne avrebbe due contratti a seconda dei valori.
    //
    // 1. Indici: l'API e' pubblica e `is_null`/`value` di arrow vanno in
    //    panico fuori intervallo, e un panico non e' ammesso.
    riga_in_intervallo(left, left_row, "sinistra")?;
    riga_in_intervallo(right, right_row, "destra")?;

    // 2. Dominio: i due tipi devono stare nella stessa famiglia di confronto
    //    (vedi `ComparisonFamily`).
    let (Some(left_family), Some(right_family)) = (
        comparison_family(left.data_type()),
        comparison_family(right.data_type()),
    ) else {
        return Err(PlenoraError::Schema(format!(
            "tipo {:?} o {:?} non ordinabile: nessun confronto nativo definito",
            left.data_type(),
            right.data_type()
        )));
    };
    if left_family != right_family {
        return Err(PlenoraError::Schema(format!(
            "tipi non confrontabili fra loro: {:?} e {:?}",
            left.data_type(),
            right.data_type()
        )));
    }

    // 3. Integrita' di ENTRAMBE le celle prima di qualunque uscita
    //    anticipata: una chiave dictionary malformata non deve passare in
    //    silenzio perche' l'altra cella e' nulla.
    let left_null = cella_logicamente_nulla(left, left_row)?;
    let right_null = cella_logicamente_nulla(right, right_row)?;

    // 4. Solo a questo punto l'ordinamento dei null. Match esaustivo sulle quattro
    //    combinazioni: il caso (false, false) prosegue con il confronto
    //    tipizzato, nessun braccio impossibile.
    match (left_null, right_null) {
        (true, true) => return Ok(Ordering::Equal),
        (true, false) => return Ok(Ordering::Greater),
        (false, true) => return Ok(Ordering::Less),
        (false, false) => {}
    }

    // 5. Dispatch esaustivo sulla famiglia: una variante nuova di
    //    `ComparisonFamily` senza braccio non compila.
    match left_family {
        ComparisonFamily::Int64 => {
            let (left_values, right_values) = coppia::<Int64Array>(left, right)?;
            Ok(left_values
                .value(left_row)
                .cmp(&right_values.value(right_row)))
        }
        ComparisonFamily::UInt64 => {
            let (left_values, right_values) = coppia::<UInt64Array>(left, right)?;
            Ok(left_values
                .value(left_row)
                .cmp(&right_values.value(right_row)))
        }
        ComparisonFamily::Float64 => {
            let (left_values, right_values) = coppia::<Float64Array>(left, right)?;
            Ok(left_values
                .value(left_row)
                .total_cmp(&right_values.value(right_row)))
        }
        ComparisonFamily::Utf8 => {
            // Stesso ordine del ripiego testuale, senza allocare per confronto.
            let (left_values, right_values) = coppia::<StringArray>(left, right)?;
            Ok(left_values
                .value(left_row)
                .cmp(right_values.value(right_row)))
        }
        ComparisonFamily::Boolean => {
            let (left_values, right_values) = coppia::<BooleanArray>(left, right)?;
            Ok(left_values
                .value(left_row)
                .cmp(&right_values.value(right_row)))
        }
        ComparisonFamily::Date32 => {
            // Giorni dall'epoch: ordine cronologico esatto, anche fuori dalle
            // date che la formattazione `%Y-%m-%d` rappresenta su quattro cifre.
            let (left_values, right_values) = coppia::<Date32Array>(left, right)?;
            Ok(left_values
                .value(left_row)
                .cmp(&right_values.value(right_row)))
        }
        ComparisonFamily::TimestampMillis => {
            // Millisecondi dall'epoch: e' l'ordine degli ISTANTI, indipendente
            // dalla timezone dichiarata nello schema — che infatti non si legge.
            let (left_values, right_values) = coppia::<TimestampMillisecondArray>(left, right)?;
            Ok(left_values
                .value(left_row)
                .cmp(&right_values.value(right_row)))
        }
        ComparisonFamily::Decimal128 => {
            let (left_values, right_values) = coppia::<Decimal128Array>(left, right)?;
            let (DataType::Decimal128(_, left_scale), DataType::Decimal128(_, right_scale)) =
                (left_values.data_type(), right_values.data_type())
            else {
                return Err(PlenoraError::Schema("decimal128 incoerente".into()));
            };
            Ok(compare_decimal128_values(
                left_values.value(left_row),
                *left_scale,
                right_values.value(right_row),
                *right_scale,
            ))
        }
        ComparisonFamily::Binary => {
            // Ordine lessicografico sui byte: definito su qualunque contenuto,
            // anche non UTF-8.
            let (left_values, right_values) = coppia::<BinaryArray>(left, right)?;
            Ok(left_values
                .value(left_row)
                .cmp(right_values.value(right_row)))
        }
        ComparisonFamily::DictionaryUtf8 => {
            // Il confronto e' sui valori decodificati, non sulle chiavi: due
            // dizionari diversi possono codificare la stessa stringa. I casi
            // con un `None` restano come difesa: il null logico e' gia' stato
            // deciso sopra, quindi qui non dovrebbero presentarsi.
            let (left_values, right_values) = coppia::<DictionaryArray<Int32Type>>(left, right)?;
            let left_text = crate::dictionary_utf8_value(left_values, left_row)?;
            let right_text = crate::dictionary_utf8_value(right_values, right_row)?;
            Ok(match (left_text, right_text) {
                (Some(left_text), Some(right_text)) => left_text.cmp(right_text),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            })
        }
    }
}

/// Downcast dei due lati allo stesso tipo concreto.
///
/// La famiglia e' gia' stata verificata uguale per entrambi, quindi il
/// fallimento qui significa che il `DataType` mente sul contenuto: e' un
/// errore di schema, non un tipo non supportato.
fn coppia<'a, A: 'static>(left: &'a ArrayRef, right: &'a ArrayRef) -> Result<(&'a A, &'a A)> {
    let (Some(left_values), Some(right_values)) = (
        left.as_any().downcast_ref::<A>(),
        right.as_any().downcast_ref::<A>(),
    ) else {
        return Err(PlenoraError::Schema(format!(
            "array incoerente col proprio tipo dichiarato: {:?} / {:?}",
            left.data_type(),
            right.data_type()
        )));
    };
    Ok((left_values, right_values))
}

/// Famiglia di confronto di un tipo Arrow.
///
/// Due celle sono confrontabili se e solo se stanno nella stessa famiglia.
/// La famiglia non coincide col `DataType`: `Decimal128(10, 2)` e
/// `Decimal128(12, 3)` si confrontano per VALORE, due `Timestamp` con
/// timezone diverse per ISTANTE.
///
/// `None` per i tipi che [`compare_cells_typed`] non sa confrontare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparisonFamily {
    Int64,
    UInt64,
    Float64,
    Utf8,
    Boolean,
    Date32,
    TimestampMillis,
    Decimal128,
    Binary,
    DictionaryUtf8,
}

/// Tabella UNICA dei tipi confrontabili.
///
/// La usano [`compare_cells_typed`], [`is_sortable`] e
/// [`validate_sortable`]. I dispatch sono `match` esaustivi sulla famiglia:
/// una variante senza bracci non compila.
const fn comparison_family(data_type: &DataType) -> Option<ComparisonFamily> {
    match data_type {
        DataType::Int64 => Some(ComparisonFamily::Int64),
        DataType::UInt64 => Some(ComparisonFamily::UInt64),
        DataType::Float64 => Some(ComparisonFamily::Float64),
        DataType::Utf8 => Some(ComparisonFamily::Utf8),
        DataType::Boolean => Some(ComparisonFamily::Boolean),
        DataType::Date32 => Some(ComparisonFamily::Date32),
        DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, _) => {
            Some(ComparisonFamily::TimestampMillis)
        }
        DataType::Decimal128(_, _) => Some(ComparisonFamily::Decimal128),
        DataType::Binary => Some(ComparisonFamily::Binary),
        DataType::Dictionary(key, value) => {
            if matches!(**key, DataType::Int32) && matches!(**value, DataType::Utf8) {
                Some(ComparisonFamily::DictionaryUtf8)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// L'indice e' dentro l'array, oppure un errore.
///
/// `Array::is_null` e `value` di arrow vanno in PANICO fuori intervallo.
/// `compare_cells_typed` e' pubblica e riceve due array e due indici
/// indipendenti — in particolare dal merge k-way dello spill, dove gli indici
/// vengono da file — quindi l'intervallo va verificato, non assunto.
fn riga_in_intervallo(array: &ArrayRef, row: usize, lato: &str) -> Result<()> {
    if row >= array.len() {
        return Err(PlenoraError::Schema(format!(
            "indice di riga {row} fuori dall'array {lato}: {} righe",
            array.len()
        )));
    }
    Ok(())
}

/// Null LOGICO della cella, con la risoluzione dictionary FALLIBILE.
///
/// Differenza da [`crate::is_logically_null`], che risponde `bool`: li' una
/// chiave malformata vale `false` — «non e' nulla, e' l'array a essere
/// incoerente» — e tocca al chiamante produrre l'errore. Qui il chiamante
/// siamo noi, e lo produciamo.
fn cella_logicamente_nulla(array: &ArrayRef, row: usize) -> Result<bool> {
    if array.is_null(row) {
        return Ok(true);
    }
    if let Some(values) = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>() {
        return Ok(crate::dictionary_utf8_value(values, row)?.is_none());
    }
    Ok(false)
}

/// `true` se [`compare_cells_typed`] ha un confronto nativo per il tipo.
///
/// I tipi: `Int64`, `UInt64`, `Float64`, `Utf8`, `Boolean`, `Date32`,
/// `Timestamp(Millisecond, _)` con qualunque timezone, `Decimal128`,
/// `Binary`, `Dictionary(Int32, Utf8)`.
///
/// Decide dallo schema: e' il controllo con cui l'analisi rifiuta le
/// colonne d'ordinamento prima dell'esecuzione.
#[must_use]
pub const fn is_sortable(data_type: &DataType) -> bool {
    comparison_family(data_type).is_some()
}
/// Verifica che una colonna sia ordinabile da [`compare_cells_typed`], senza
/// confrontare nulla.
///
/// Quasi tutti i tipi si decidono dallo schema; solo i dictionary richiedono
/// una passata sulle righe, perche' una chiave fuori dal dizionario e'
/// proprieta' della singola cella.
///
/// # Errors
///
/// `Schema` se il tipo non ha un confronto nativo, se un Decimal128 e'
/// incoerente con il proprio schema o se una chiave dictionary non e'
/// risolvibile.
pub fn validate_sortable(array: &ArrayRef, rows: usize) -> Result<()> {
    // Stessa tabella del comparatore.
    let Some(family) = comparison_family(array.data_type()) else {
        return Err(PlenoraError::Schema(format!(
            "tipo {:?} non ordinabile: nessun confronto nativo definito",
            array.data_type()
        )));
    };
    // Match esaustivo: una famiglia nuova senza il proprio braccio non
    // compila.
    match family {
        ComparisonFamily::Int64
        | ComparisonFamily::UInt64
        | ComparisonFamily::Float64
        | ComparisonFamily::Utf8
        | ComparisonFamily::Boolean
        | ComparisonFamily::Date32
        | ComparisonFamily::TimestampMillis
        | ComparisonFamily::Binary => Ok(()),
        ComparisonFamily::Decimal128 => {
            let values = array
                .as_any()
                .downcast_ref::<Decimal128Array>()
                .ok_or_else(|| PlenoraError::Schema("decimal128 incoerente".into()))?;
            if matches!(values.data_type(), DataType::Decimal128(_, _)) {
                Ok(())
            } else {
                Err(PlenoraError::Schema("decimal128 incoerente".into()))
            }
        }
        ComparisonFamily::DictionaryUtf8 => {
            let values = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .ok_or_else(|| PlenoraError::Schema("dictionary incoerente".into()))?;
            for row in 0..rows.min(values.len()) {
                // Il risolutore convalida chiave e dizionario e distingue il
                // null logico: qui interessa solo che nessuna riga sia
                // malformata.
                crate::dictionary_utf8_value(values, row)?;
            }
            Ok(())
        }
    }
}

pub(in crate::aggregation) fn compare_at(
    batch: &RecordBatch,
    index: usize,
    left: usize,
    right: usize,
) -> Result<Ordering> {
    let array = batch.column(index);
    compare_cells_typed(array, left, array, right)
}
