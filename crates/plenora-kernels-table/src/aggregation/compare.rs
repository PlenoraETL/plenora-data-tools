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
pub(in crate::aggregation) fn row_key(
    batch: &RecordBatch,
    indices: &[usize],
    row: usize,
) -> Result<String> {
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

/// Confronto tipizzato tra due celle, condiviso dai tre siti di confronto di
/// `table.sort`.
///
/// Semantica: null dopo i valori (uguaglianza tra null); confronto nel
/// dominio NATIVO di ogni tipo supportato, mai sulla forma testuale.
///
/// Un ripiego testuale per tutti i tipi diversi da Int64/UInt64/Float64
/// darebbe un ordine sbagliato dove conta di piu':
///
/// - **Decimal128** — "10" ordina prima di "9", e i negativi si dispongono
///   al contrario del loro valore;
/// - **Timestamp con timezone** — la stringa e' l'ora LOCALE, e all'ora
///   legale l'ordine lessicografico delle ore locali non e' l'ordine degli
///   istanti UTC;
/// - **Binary** — pretenderebbe UTF-8 valido e fallirebbe su qualunque
///   colonna binaria che non lo sia (una geometria WKB, per esempio).
///
/// Lo stesso comparatore alimenta `sort`, il top-N e il merge k-way dello
/// spill, quindi l'errore si propagherebbe a tutti e tre. I confronti nativi
/// eliminano anche la costruzione di stringhe e il parsing della timezone
/// dentro un sort O(n log n).
///
/// I tipi fuori dal profilo scalare sono rifiutati esplicitamente, invece di
/// ricevere un ordine arbitrario.
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
/// I tre siti d'uso sono `compare_at` (stesso batch), `ColumnComparator`
/// (fast path tipizzato) e il merge k-way dello spill
/// (`spill::compare_cells`, batch diversi — da qui la forma a due array).
// Il corpo e' un dispatch lineare per tipo Arrow: e' cresciuto di dieci righe
// con il null logico della dictionary, e spezzarlo in due meta' arbitrarie
// renderebbe piu' difficile verificare che ogni tipo sia trattato una volta
// sola.
#[allow(clippy::too_many_lines)]
pub fn compare_cells_typed(
    left: &ArrayRef,
    left_row: usize,
    right: &ArrayRef,
    right_row: usize,
) -> Result<Ordering> {
    // ORDINE DEI CONTROLLI. Prima il dominio, poi i null. Il contrario e'
    // fail-open: decidendo sui null in testa si risponderebbe `Equal` a due
    // celle nulle di tipi INCOMPATIBILI o non ordinabili, cioe' proprio dove
    // la funzione deve rifiutare — e con gli stessi tipi, ma valori non
    // nulli, si risponderebbe `Schema`. La stessa coppia di colonne darebbe
    // due contratti diversi a seconda del contenuto delle celle. Un difetto
    // mascherato dai null e' un difetto che si manifesta piu' tardi, altrove.
    //
    // 1. Indici. L'API e' pubblica e prende due array e due indici
    //    indipendenti: nulla garantisce che siano in intervallo. `is_null` e
    //    `value` di arrow vanno in panico fuori intervallo, e un panico e'
    //    vietato dal gate R6 oltre che inutile a chi ci chiama.
    riga_in_intervallo(left, left_row, "sinistra")?;
    riga_in_intervallo(right, right_row, "destra")?;

    // 2. Dominio confrontabile. Non basta che i due tipi siano ordinabili:
    //    devono appartenere alla STESSA famiglia di confronto, altrimenti
    //    non esiste un ordine fra loro. La famiglia non e' il tipo esatto —
    //    `Decimal128(10, 2)` e `Decimal128(12, 3)` si confrontano per valore,
    //    e due timestamp con timezone diverse si confrontano per istante —
    //    quindi il criterio e' la famiglia, non l'uguaglianza dei `DataType`.
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

    // 3. Integrita' della cella, ENTRAMBE. `is_logically_null` risponde
    //    `false` su una chiave dictionary malformata — per non trasformare un
    //    difetto in un silenzio. Uscendo prima di risolvere questa cella
    //    perche' l'altra e' nulla, il silenzio tornerebbe: la risoluzione
    //    e' fallibile e avviene per ENTRAMBE le celle prima di qualunque
    //    uscita anticipata.
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

    // 5. Dispatch sulla FAMIGLIA, non su una catena di `downcast_ref`. Il
    //    match e' esaustivo: aggiungere una variante a `ComparisonFamily`
    //    senza aggiungere il braccio corrispondente non compila. L'elenco
    //    dei tipi vive in un posto solo: in tre copie — la tabella, questa
    //    catena e `validate_sortable` — resterebbe allineato per
    //    raccomandazione, e una raccomandazione non e' un vincolo.
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
/// timezone diverse per ISTANTE. Chiedere l'uguaglianza dei `DataType`
/// rifiuterebbe confronti corretti; chiedere solo che siano entrambi
/// ordinabili accetterebbe un Int64 contro una stringa.
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
/// La usano [`compare_cells_typed`] (dispatch), [`is_sortable`] e
/// [`validate_sortable`]: l'elenco dei tipi esiste in un punto solo, e i due
/// dispatch sono `match` ESAUSTIVI sulla famiglia, quindi aggiungere una
/// variante senza aggiungere i bracci non compila. Tre elenchi scritti a
/// mano resterebbero allineati per raccomandazione, e una raccomandazione
/// non impedisce nulla.
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

#[must_use]
pub const fn is_sortable(data_type: &DataType) -> bool {
    comparison_family(data_type).is_some()
}
/// Verifica che una colonna sia ordinabile da [`compare_cells_typed`], senza
/// confrontare nulla.
///
/// Vive accanto a `compare_cells_typed` apposta: e' l'elenco dei tipi che
/// quella funzione sa confrontare, e le due devono restare allineate.
///
/// Quasi tutti i tipi si decidono guardando solo lo schema, quindi il costo
/// e' costante; le sole colonne che richiedono una passata sulle righe sono i
/// dictionary, dove una chiave fuori dal dizionario e' una proprieta' della
/// singola cella.
///
/// # Errors
///
/// `Schema` se il tipo non ha un confronto nativo, se un Decimal128 e'
/// incoerente con il proprio schema o se una chiave dictionary non e'
/// risolvibile.
pub fn validate_sortable(array: &ArrayRef, rows: usize) -> Result<()> {
    // Stessa tabella del comparatore: l'elenco dei tipi ordinabili non esiste
    // piu' in una seconda copia scritta a mano.
    let Some(family) = comparison_family(array.data_type()) else {
        return Err(PlenoraError::Schema(format!(
            "tipo {:?} non ordinabile: nessun confronto nativo definito",
            array.data_type()
        )));
    };
    // Match esaustivo: una famiglia nuova senza il proprio braccio non
    // compila. Quasi tutti i tipi si decidono guardando solo lo schema; le
    // sole colonne che richiedono una passata sulle righe sono i dictionary.
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
